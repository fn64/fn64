#![allow(
    clippy::excessive_precision,
    clippy::identity_op,
    clippy::needless_range_loop
)]
use super::support::*;
use crate::gbi::*;
use crate::gbi::wire::*;
use crate::gbi::types::*;
use crate::gbi::matrix::*;
use crate::gbi::tmem::*;
use crate::gbi::state::*;
use crate::gbi::entries::*;
use crate::gbi::stream::*;
use crate::gbi::geometry::*;
use fn64_render::{
    GeometryUcodeProfile, MicrocodeDataImageIdentity, RenderError, TaskAdmissionGeneration,
    TaskAdmissionSource, UcodeId,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt::Write as _};
use fn64_render::{F3dzex2Variant, TaskAdmissionUcode};

#[test]
fn simple_decoder_rejects_unknown_opcode_with_wire_context() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let mut rdram = vec![0u8; 16];
    rdram[..4].copy_from_slice(&0x7f12_3456u32.to_be_bytes());
    rdram[4..8].copy_from_slice(&0x89ab_cdefu32.to_be_bytes());
    rdram[8..12].copy_from_slice(&((G_ENDDL as u32) << 24).to_be_bytes());

    let error = decode_display_list(&rdram, 0).unwrap_err().to_string();

    assert!(error.contains("reference-simple"));
    assert!(error.contains("opcode 0x7f"));
    assert!(error.contains("w0=0x7f123456"));
    assert!(error.contains("w1=0x89abcdef"));
    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].subsystem,
        fn64_runtime::UnsupportedSubsystem::Render
    );
    assert_eq!(events[0].operation, "render.gbi.simple.command");
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::ReturnedError
    );
    assert_eq!(events[0].guest_cycle, None);
}


#[test]
fn simple_decoder_rejects_truncation_without_end_command() {
    let error = decode_display_list(&[0; 4], 0).unwrap_err().to_string();
    assert!(error.contains("truncated command"));
    assert!(error.contains("without G_ENDDL"));
}


#[test]
fn simple_decoder_rejects_out_of_bounds_vertex_range() {
    let mut rdram = vec![0u8; 16];
    let w0 = ((G_VTX as u32) << 24) | (1 << 12);
    rdram[..4].copy_from_slice(&w0.to_be_bytes());
    rdram[4..8].copy_from_slice(&0x100u32.to_be_bytes());
    rdram[8..12].copy_from_slice(&((G_ENDDL as u32) << 24).to_be_bytes());

    let error = decode_display_list(&rdram, 0).unwrap_err().to_string();
    assert!(error.contains("G_VTX"));
    assert!(error.contains("RDRAM=0x00000100"));
}


#[test]
fn command_trace_follows_segmented_calls_and_fingerprints_targets() {
    let mut rdram = vec![0u8; 0x4000];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | ((G_MW_SEGMENT as u32) << 16) | 0x0c,
        0x8000_3000,
    );
    wr_cmd(&mut rdram, 0x1008, (G_DL as u32) << 24, 0x0300_0100);
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
    wr_cmd(
        &mut rdram,
        0x3100,
        ((G_VTX as u32) << 24) | (1 << 12) | (1 << 1),
        0x0300_0200,
    );
    wr_cmd(&mut rdram, 0x3108, (G_ENDDL as u32) << 24, 0);
    rdram[0x3200] = 0x5a;

    let trace = trace_display_list_f3dex2(&rdram, 0x1000);
    assert!(trace.contains("SEG depth=0 segment=3 base=0x003000"));
    assert!(trace.contains("ENTER depth=1 segmented=0x03000100 resolved=0x003100"));
    assert!(trace.contains("target=0x003200 bytes=16 nonzero=1"));
    assert!(trace.contains("SUMMARY commands=5"));
    assert!(trace.contains("OPCODE op=0xdf count=2"));
}


#[test]
fn dma_io_round_trips_logical_rdram_bytes_through_persistent_imem() {
    const DL: usize = 0x1000;
    const SOURCE: usize = 0x1800;
    const DESTINATION: usize = 0x1900;
    const IMEM_ADDRESS: u16 = 0x1040;
    let expected = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0];
    let mut rdram = vec![0u8; 0x2000];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(SOURCE as u32),
        &expected,
    );
    wr_cmd(
        &mut rdram,
        DL,
        dma_io_word(false, IMEM_ADDRESS, expected.len() as u16),
        SOURCE as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 8,
        dma_io_word(true, IMEM_ADDRESS, expected.len() as u16),
        DESTINATION as u32,
    );
    wr_cmd(&mut rdram, DL + 16, (G_ENDDL as u32) << 24, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    execute_display_list_f3dex2_ops(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

    let rsp_address = fn64_runtime::RspMemAddr::from_register(u32::from(IMEM_ADDRESS));
    assert_eq!(rsp_memory.read_bytes(rsp_address, 8).unwrap(), expected);
    assert_eq!(rsp_memory.imem_generation(), 1);
    let mut round_trip = [0; 8];
    fn64_runtime::RdramView::from_storage(&rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(DESTINATION as u32),
        &mut round_trip,
    );
    assert_eq!(round_trip, expected);
}


#[test]
fn dma_io_write_is_visible_to_the_next_command_in_the_same_stream() {
    const DL: usize = 0x1000;
    const DMEM_ADDRESS: u16 = 0x0080;
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(
        &mut rdram,
        DL,
        dma_io_word(true, DMEM_ADDRESS, 8),
        (DL + 8) as u32,
    );
    wr_cmd(&mut rdram, DL + 8, (G_SPECIAL_1 as u32) << 24, 0);
    wr_cmd(&mut rdram, DL + 16, (G_SPECIAL_1 as u32) << 24, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_register(u32::from(DMEM_ADDRESS)),
            &[G_ENDDL, 0, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
    let state =
        execute_display_list_f3dex2_state(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

    assert_eq!(state.cmds_decoded, 2);
    assert_eq!(read_u32(&rdram, DL + 8), (G_ENDDL as u32) << 24);
}


#[test]
fn load_ucode_transfers_data_and_text_before_the_next_command() {
    const DL: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3800;
    const ROUND_TRIP: usize = 0x4800;
    const DATA_SIZE: usize = 32;
    let text: Vec<u8> = (0..SP_UCODE_SIZE)
        .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
        .collect();
    let data: Vec<u8> = (0..DATA_SIZE)
        .map(|index| (index as u8).wrapping_mul(13).wrapping_add(7))
        .collect();
    let mut rdram = vec![0u8; 0x5000];
    let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
    view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT as u32), &text);
    view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA as u32), &data);
    wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(
        &mut rdram,
        DL + 8,
        load_ucode_word(DATA_SIZE as u16),
        TEXT as u32,
    );
    wr_cmd(
        &mut rdram,
        DL + 16,
        dma_io_word(true, 0x1000, 8),
        ROUND_TRIP as u32,
    );
    wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(
                fn64_runtime::RspMemoryBank::Dmem,
                DATA_SIZE as u16,
            ),
            &[0xa5; 8],
        )
        .unwrap();
    execute_display_list_f3dex2_ops(&mut rdram, &mut rsp_memory, DL as u32).unwrap();

    assert_eq!(
        rsp_memory
            .read_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0,),
                DATA_SIZE,
            )
            .unwrap(),
        data
    );
    assert_eq!(
        rsp_memory
            .read_bytes(
                fn64_runtime::RspMemAddr::from_parts(
                    fn64_runtime::RspMemoryBank::Dmem,
                    DATA_SIZE as u16,
                ),
                8,
            )
            .unwrap(),
        [0xa5; 8],
        "the command replaces only the declared DMEM data section"
    );
    assert_eq!(
        rsp_memory
            .bank(fn64_runtime::RspMemoryBank::Imem)
            .as_slice(),
        text
    );
    assert_eq!(rsp_memory.imem_generation(), 1);
    let mut round_trip = [0; 8];
    fn64_runtime::RdramView::from_storage(&rdram).copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(ROUND_TRIP as u32),
        &mut round_trip,
    );
    assert_eq!(round_trip, text[..8]);
}


#[test]
fn admission_inspection_preserves_same_address_a_b_a_generations() {
    const DL: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3800;
    const A_PREFIX: usize = 0x3900;
    const B_PREFIX: usize = 0x3a00;
    let a = (0..SP_UCODE_SIZE)
        .map(|index| ((index * 37 + 11) & 0xff) as u8)
        .collect::<Vec<_>>();
    let mut b = a.clone();
    b[..8].copy_from_slice(&[0xe1, 0x02, 0xc3, 0x24, 0xa5, 0x46, 0x87, 0x68]);
    let data = [0x7d, 0x1e, 0xb3, 0x54, 0x99, 0x2a, 0xc7, 0x60];
    let mut rdram = vec![0u8; 0x5000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT as u32), &a);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA as u32), &data);
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(A_PREFIX as u32),
            &a[..8],
        );
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(B_PREFIX as u32),
            &b[..8],
        );
    }
    {
        let mut pc = DL;
        let mut command = |w0, w1| {
            wr_cmd(&mut rdram, pc, w0, w1);
            pc += 8;
        };
        command((G_RDPHALF_1 as u32) << 24, DATA as u32);
        command(load_ucode_word(8), TEXT as u32);
        command(dma_io_word(false, 0, 8), B_PREFIX as u32);
        command(dma_io_word(true, 0, 8), TEXT as u32);
        command((G_RDPHALF_1 as u32) << 24, DATA as u32);
        command(load_ucode_word(8), TEXT as u32);
        command(dma_io_word(false, 0, 8), A_PREFIX as u32);
        command(dma_io_word(true, 0, 8), TEXT as u32);
        command((G_RDPHALF_1 as u32) << 24, DATA as u32);
        command(load_ucode_word(8), TEXT as u32);
        command((G_ENDDL as u32) << 24, 0);
    }

    let mut catalog = GeometryUcodeCatalog::default();
    catalog.admit_text(&a);
    catalog.admit_text(&b);
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let mut rdp_state = RdpDecodeState::default();
    let inspection = inspect_display_list_geometry_admission_with_raw_windows(
        &mut rdram,
        &mut rsp_memory,
        DL as u32,
        &catalog,
        &mut rdp_state,
        GeometryWireFamily::F3dex2,
        GeometryTaskAdmissionOptions {
            raw_window_size: TaskAdmissionRawWindowSize { text: 16, data: 8 },
            force_branch: false,
        },
    )
    .unwrap();

    assert_eq!(inspection.self_loads.len(), 3);
    assert!(inspection
        .self_loads
        .iter()
        .all(|generation| generation.text_address == TEXT as u32));
    assert_eq!(
        inspection
            .self_loads
            .iter()
            .map(|generation| generation.text_sha256)
            .collect::<Vec<_>>(),
        [
            UcodeDigest::from_text(&a),
            UcodeDigest::from_text(&b),
            UcodeDigest::from_text(&a),
        ]
    );
    let data_sha256: [u8; 32] = Sha256::digest(data).into();
    assert!(inspection.self_loads.iter().all(|generation| {
        generation.source == TaskAdmissionSource::SelfLoad
            && generation.data.bytes == 8
            && generation.data.sha256 == data_sha256
            && generation.family() == UcodeId::F3dex2
    }));
    assert_eq!(inspection.self_load_raw_windows.len(), 3);
    assert_eq!(
        inspection.self_load_raw_windows[0], inspection.self_load_raw_windows[2],
        "A -> B -> A must retain the original raw recognition bytes twice"
    );
    assert_ne!(
        inspection.self_load_raw_windows[0], inspection.self_load_raw_windows[1],
        "same-address B bytes must not collapse into RT64's address cache"
    );
    assert_ne!(
        inspection.self_load_raw_windows[0].text,
        a[..16],
        "raw RT64 recognition bytes must remain distinct from the logical RSP DMA image"
    );
}


#[test]
fn shared_walker_matches_reference_nested_same_address_a_b_a() {
    const ROOT: usize = 0x1000;
    const CHILD: usize = 0x1800;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x4000;
    const A_PREFIX: usize = 0x7000;
    const B_PREFIX: usize = 0x7100;
    let (mut rdram, rsp_memory, task, mut catalog) = geometry_admission_fixture();
    let a = (0..SP_UCODE_SIZE)
        .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
        .collect::<Vec<_>>();
    let mut b = a.clone();
    b[..8].copy_from_slice(&[0xe1, 0x02, 0xc3, 0x24, 0xa5, 0x46, 0x87, 0x68]);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(A_PREFIX as u32),
            &a[..8],
        );
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(B_PREFIX as u32),
            &b[..8],
        );
    }
    catalog.admit_text(&b);

    wr_cmd(&mut rdram, ROOT, (G_DL as u32) << 24, CHILD as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 8,
        dma_io_word(false, 0, 8),
        B_PREFIX as u32,
    );
    wr_cmd(&mut rdram, ROOT + 16, dma_io_word(true, 0, 8), TEXT as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 24,
        (G_RDPHALF_1 as u32) << 24,
        DATA as u32,
    );
    wr_cmd(&mut rdram, ROOT + 32, load_ucode_word(8), TEXT as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 40,
        dma_io_word(false, 0, 8),
        A_PREFIX as u32,
    );
    wr_cmd(&mut rdram, ROOT + 48, dma_io_word(true, 0, 8), TEXT as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 56,
        (G_RDPHALF_1 as u32) << 24,
        DATA as u32,
    );
    wr_cmd(&mut rdram, ROOT + 64, load_ucode_word(8), TEXT as u32);
    wr_cmd(&mut rdram, ROOT + 72, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, ROOT + 80, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, CHILD, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(&mut rdram, CHILD + 8, load_ucode_word(8), TEXT as u32);
    wr_cmd(&mut rdram, CHILD + 16, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, CHILD + 24, (G_ENDDL as u32) << 24, 0);

    let projection =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, false);
    assert_eq!(projection.generations.len(), 4);
    assert_eq!(projection.raw_windows.len(), 4);
    assert_eq!(projection.full_sync_count, 2);
    assert_eq!(
        projection
            .generations
            .iter()
            .map(|generation| generation.text_sha256)
            .collect::<Vec<_>>(),
        [
            UcodeDigest::from_text(&a),
            UcodeDigest::from_text(&a),
            UcodeDigest::from_text(&b),
            UcodeDigest::from_text(&a),
        ]
    );
}


#[test]
fn shared_walker_matches_reference_force_branch_paths() {
    const ROOT: usize = 0x1000;
    const TARGET: usize = 0x1800;
    const OTHER_TEXT: usize = 0x5000;
    const OTHER_DATA: usize = 0x6800;
    let (mut rdram, rsp_memory, task, mut catalog) = geometry_admission_fixture();
    let other = vec![0x5d; SP_UCODE_SIZE];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_TEXT as u32),
            &other,
        );
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_DATA as u32),
            &[2, 4, 6, 8, 10, 12, 14, 16],
        );
    }
    catalog.admit_text(&other);
    wr_cmd(
        &mut rdram,
        ROOT,
        ((G_MODIFYVTX as u32) << 24) | ((G_MWO_POINT_ZSCREEN as u32) << 16),
        0x0002_0000,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 8,
        (G_RDPHALF_1 as u32) << 24,
        TARGET as u32,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 16,
        (G_BRANCH_Z as u32) << 24,
        0x0001_ffff,
    );
    wr_cmd(&mut rdram, ROOT + 24, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, ROOT + 32, (G_ENDDL as u32) << 24, 0);
    wr_cmd(
        &mut rdram,
        TARGET,
        (G_RDPHALF_1 as u32) << 24,
        OTHER_DATA as u32,
    );
    wr_cmd(
        &mut rdram,
        TARGET + 8,
        load_ucode_word(8),
        OTHER_TEXT as u32,
    );
    wr_cmd(&mut rdram, TARGET + 16, (G_ENDDL as u32) << 24, 0);

    let ordinary =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, false);
    let forced =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, true);
    assert_eq!(ordinary.full_sync_count, 1);
    assert_eq!(ordinary.generations.len(), 1);
    assert_eq!(forced.full_sync_count, 0);
    assert_eq!(forced.generations.len(), 2);
}


#[test]
fn shared_walker_matches_reference_culldl_child_return() {
    const ROOT: usize = 0x1000;
    const CHILD: usize = 0x1800;
    const MATRIX: usize = 0x7000;
    const VIEWPORT: usize = 0x7100;
    const VERTICES: usize = 0x7200;
    const OTHER_TEXT: usize = 0x5000;
    const OTHER_DATA: usize = 0x6800;
    let (mut rdram, rsp_memory, task, mut catalog) = geometry_admission_fixture();
    let other = vec![0x5d; SP_UCODE_SIZE];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_TEXT as u32),
            &other,
        );
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_DATA as u32),
            &[2, 4, 6, 8, 10, 12, 14, 16],
        );
    }
    catalog.admit_text(&other);
    wr_mtx(&mut rdram, MATRIX, identity());
    wr_centered_viewport(&mut rdram, VIEWPORT);
    for (slot, (x, y)) in [(2, 0), (2, 1)].into_iter().enumerate() {
        wr_vtx(
            &mut rdram,
            VERTICES + slot * VTX_STRIDE,
            x,
            y,
            0,
            [255, 255, 255, 255],
        );
    }
    let mtx_len = ((64u32 - 1) / 8) << 19;
    wr_cmd(&mut rdram, ROOT, movemem_viewport_word(), VIEWPORT as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 8,
        ((G_MTX as u32) << 24) | mtx_len | 0x07,
        MATRIX as u32,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 16,
        ((G_VTX as u32) << 24) | (2 << 12) | (2 << 1),
        VERTICES as u32,
    );
    wr_cmd(&mut rdram, ROOT + 24, (G_DL as u32) << 24, CHILD as u32);
    wr_cmd(&mut rdram, ROOT + 32, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, ROOT + 40, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, CHILD, (G_CULLDL as u32) << 24, 2);
    wr_cmd(
        &mut rdram,
        CHILD + 8,
        (G_RDPHALF_1 as u32) << 24,
        OTHER_DATA as u32,
    );
    wr_cmd(
        &mut rdram,
        CHILD + 16,
        load_ucode_word(8),
        OTHER_TEXT as u32,
    );
    wr_cmd(&mut rdram, CHILD + 24, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, CHILD + 32, (G_ENDDL as u32) << 24, 0);

    let projection =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, false);
    assert_eq!(projection.generations.len(), 1);
    assert_eq!(projection.full_sync_count, 1);
}


#[test]
fn shared_walker_matches_reference_dma_rewritten_commands() {
    const ROOT: usize = 0x1000;
    const DMEM_ADDRESS: u16 = 0x0080;
    let (mut rdram, mut rsp_memory, task, catalog) = geometry_admission_fixture();
    wr_cmd(
        &mut rdram,
        ROOT,
        dma_io_word(true, DMEM_ADDRESS, 16),
        (ROOT + 8) as u32,
    );
    wr_cmd(&mut rdram, ROOT + 8, (G_SPECIAL_1 as u32) << 24, 0);
    wr_cmd(&mut rdram, ROOT + 16, (G_SPECIAL_1 as u32) << 24, 0);
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_register(u32::from(DMEM_ADDRESS)),
            &[
                G_RDPFULLSYNC,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                G_ENDDL,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        )
        .unwrap();

    let projection =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, false);
    assert_eq!(projection.full_sync_count, 1);
    assert_eq!(
        projection.dp_full_sync,
        fn64_render::DpFullSyncStatus::Reached
    );
}


#[test]
fn shared_walker_matches_reference_texture_and_line_envelopes() {
    const ROOT: usize = 0x1000;
    for enveloped in [false, true] {
        let (mut rdram, rsp_memory, task, catalog) = geometry_admission_fixture();
        wr_cmd(&mut rdram, ROOT, (G_TEXRECT as u32) << 24, 0);
        if enveloped {
            wr_cmd(
                &mut rdram,
                ROOT + 8,
                (G_RDPHALF_1 as u32) << 24,
                0xe900_0000,
            );
            wr_cmd(
                &mut rdram,
                ROOT + 16,
                (G_RDPHALF_2 as u32) << 24,
                0xe900_0000,
            );
        } else {
            wr_cmd(&mut rdram, ROOT + 8, 0xe900_0000, 0);
        }
        let sync = if enveloped { ROOT + 24 } else { ROOT + 16 };
        wr_cmd(&mut rdram, sync, (G_RDPFULLSYNC as u32) << 24, 0);
        wr_cmd(&mut rdram, sync + 8, (G_ENDDL as u32) << 24, 0);
        let projection = assert_shared_admission_matches_reference(
            &rdram,
            &rsp_memory,
            &task,
            &catalog,
            false,
        );
        assert_eq!(projection.full_sync_count, 1);
    }

    let (mut rdram, rsp_memory, task, catalog) = geometry_admission_fixture();
    const VERTICES: usize = 0x7000;
    wr_vtx(&mut rdram, VERTICES, 2, 4, 0, [255, 0, 0, 255]);
    wr_vtx(
        &mut rdram,
        VERTICES + VTX_STRIDE,
        10,
        4,
        0,
        [0, 0, 255, 255],
    );
    wr_cmd(
        &mut rdram,
        ROOT,
        ((G_VTX as u32) << 24) | (2 << 12) | (5 << 1),
        VERTICES as u32,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 8,
        ((G_LINE3D as u32) << 24) | (8 << 16) | (6 << 8) | 3,
        0,
    );
    wr_cmd(&mut rdram, ROOT + 16, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, ROOT + 24, (G_ENDDL as u32) << 24, 0);
    let projection =
        assert_shared_admission_matches_reference(&rdram, &rsp_memory, &task, &catalog, false);
    assert_eq!(projection.full_sync_count, 1);
}


#[test]
fn shared_walker_matches_reference_late_failure_digest_transactionally() {
    const ROOT: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x4000;
    const OTHER_TEXT: usize = 0x5000;
    const OTHER_DATA: usize = 0x6800;
    let (mut rdram, rsp_memory, task, catalog) = geometry_admission_fixture();
    let other = vec![0x5d; SP_UCODE_SIZE];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_TEXT as u32),
            &other,
        );
        view.write_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(OTHER_DATA as u32),
            &[2, 4, 6, 8, 10, 12, 14, 16],
        );
    }
    wr_cmd(&mut rdram, ROOT, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(&mut rdram, ROOT + 8, load_ucode_word(8), TEXT as u32);
    wr_cmd(
        &mut rdram,
        ROOT + 16,
        (G_RDPHALF_1 as u32) << 24,
        OTHER_DATA as u32,
    );
    wr_cmd(&mut rdram, ROOT + 24, load_ucode_word(8), OTHER_TEXT as u32);
    wr_cmd(&mut rdram, ROOT + 32, (G_ENDDL as u32) << 24, 0);
    let original_rdram = rdram.clone();
    let original_rsp = rsp_memory.clone();

    let reference_error =
        reference_admission_projection(&rdram, &rsp_memory, &task, &catalog, false)
            .unwrap_err();
    let shared_error =
        shared_admission_projection(&rdram, &rsp_memory, &task, &catalog, false).unwrap_err();
    let RenderError::RequiresLle {
        ucode_sha256: reference_digest,
    } = reference_error
    else {
        panic!("reference walker returned {reference_error}");
    };
    let RenderError::RequiresLle {
        ucode_sha256: shared_digest,
    } = shared_error
    else {
        panic!("shared walker returned {shared_error}");
    };
    assert_eq!(reference_digest, UcodeDigest::from_text(&other).as_bytes());
    assert_eq!(shared_digest, reference_digest);
    assert_eq!(rdram, original_rdram);
    assert_eq!(rsp_memory, original_rsp);
}


#[test]
fn f3dex2_load_ucode_preserves_the_display_list_return_stack() {
    const ROOT: usize = 0x1000;
    const CHILD: usize = 0x1100;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3000;
    let mut rdram = vec![0u8; 0x4000];
    wr_cmd(&mut rdram, ROOT, (G_DL as u32) << 24, CHILD as u32);
    wr_cmd(&mut rdram, ROOT + 8, (G_NOOP as u32) << 24, 0xfeed_beef);
    wr_cmd(&mut rdram, ROOT + 16, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, CHILD, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(&mut rdram, CHILD + 8, load_ucode_word(8), TEXT as u32);
    wr_cmd(&mut rdram, CHILD + 16, (G_ENDDL as u32) << 24, 0);

    let state = execute_display_list_f3dex2_state(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        ROOT as u32,
    )
    .unwrap();

    assert_eq!(state.cmds_decoded, 6);
    assert_eq!(state.dl_depth, 0);
}


#[test]
fn self_load_stops_before_decoding_an_unadmitted_microcode_family() {
    const DL: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3000;
    let target_text = vec![0x5a; SP_UCODE_SIZE];
    let mut rdram = vec![0u8; 0x4000];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(TEXT as u32),
        &target_text,
    );
    wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(&mut rdram, DL + 8, load_ucode_word(8), TEXT as u32);
    wr_cmd(&mut rdram, DL + 16, (G_SPECIAL_1 as u32) << 24, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let mut catalog = F3dex2UcodeCatalog::default();
    catalog.admit_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem));
    let error = execute_display_list_f3dex2_ops_admitted(
        &mut rdram,
        &mut rsp_memory,
        DL as u32,
        &catalog,
    )
    .expect_err("changed unadmitted IMEM must leave the F3DEX2 HLE lane");

    match error {
        RenderError::RequiresLle { ucode_sha256 } => assert_eq!(
            ucode_sha256,
            UcodeDigest::from_text(&target_text).as_bytes()
        ),
        other => panic!("expected typed LLE handoff, got {other}"),
    }
    assert_eq!(
        rsp_memory
            .bank(fn64_runtime::RspMemoryBank::Imem)
            .as_slice(),
        target_text
    );
    assert_eq!(rsp_memory.imem_generation(), 1);
}


#[test]
fn exact_digest_admits_a_compatible_self_load_target() {
    const DL: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3000;
    let target_text = vec![0xa6; SP_UCODE_SIZE];
    let mut rdram = vec![0u8; 0x4000];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(TEXT as u32),
        &target_text,
    );
    wr_cmd(&mut rdram, DL, (G_RDPHALF_1 as u32) << 24, DATA as u32);
    wr_cmd(&mut rdram, DL + 8, load_ucode_word(8), TEXT as u32);
    wr_cmd(&mut rdram, DL + 16, (G_NOOP as u32) << 24, 0xcafe_babe);
    wr_cmd(&mut rdram, DL + 24, (G_ENDDL as u32) << 24, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let mut catalog = F3dex2UcodeCatalog::default();
    let admitted = catalog.admit_text(&target_text);
    assert_eq!(admitted, UcodeDigest::from_text(&target_text));
    let operations = execute_display_list_f3dex2_ops_admitted(
        &mut rdram,
        &mut rsp_memory,
        DL as u32,
        &catalog,
    )
    .unwrap();

    assert!(operations.is_empty());
    assert_eq!(rsp_memory.imem_generation(), 1);
}


#[test]
fn public_rdp_tagged_noop_and_rsp_noop_are_intentional_commands() {
    let mut rdram = vec![0u8; 0x1020];
    wr_cmd(&mut rdram, 0x1000, (G_NOOP as u32) << 24, 0xfeed_beef);
    wr_cmd(&mut rdram, 0x1008, (G_SPNOOP as u32) << 24, 0);
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

    let state = decode_display_list_f3dex2_state(&rdram, 0x1000).unwrap();
    assert_eq!(state.cmds_decoded, 3);
    assert!(state.ops.is_empty());
}


#[test]
#[should_panic(expected = "G_SPNOOP reserved second word must be zero")]
fn rsp_noop_rejects_non_public_payload() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, (G_SPNOOP as u32) << 24, 1);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "reserved F3DEX2 command G_SPECIAL_2")]
fn reserved_special_commands_trap_by_public_name() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, (G_SPECIAL_2 as u32) << 24, 0);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
fn unrecognized_commands_trap_with_wire_context() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(&mut rdram, 0x1000, 0x0901_0203, 0x0405_0607);
    let panic = std::panic::catch_unwind(|| decode_display_list_f3dex2_ops(&rdram, 0x1000));
    assert!(panic.is_err());
    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation, "render.gbi.geometry.command");
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::LoudTrap
    );
    assert!(events[0].context.contains("F3DEX2"));
    assert!(events[0].context.contains("w0=0x09010203"));
}


#[test]
#[should_panic(expected = "G_MOVEWORD unsupported index 0xff")]
fn unknown_moveword_destination_traps_with_index() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEWORD as u32) << 24) | (0xff << 16),
        0,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_MOVEMEM unsupported index 0x0c")]
fn unsupported_movemem_point_destination_traps_with_index() {
    let mut rdram = vec![0u8; 0x1010];
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_MOVEMEM as u32) << 24) | (1 << 19) | 0x0c,
        0,
    );
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "requires execute_display_list_f3dex2_ops with live RSP memory")]
fn read_only_display_list_inspection_rejects_dma_io() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 8), 0x1080);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "G_LOAD_UCODE requires execute_display_list_f3dex2_ops")]
fn read_only_display_list_inspection_rejects_load_ucode() {
    let mut rdram = vec![0u8; 0x3000];
    wr_cmd(&mut rdram, 0x1000, (G_RDPHALF_1 as u32) << 24, 0x1800);
    wr_cmd(&mut rdram, 0x1008, load_ucode_word(8), 0x2000);
    let _ = decode_display_list_f3dex2_ops(&rdram, 0x1000);
}


#[test]
#[should_panic(expected = "without the compound command's preceding G_RDPHALF_1")]
fn load_ucode_rejects_a_missing_data_address_stage() {
    let mut rdram = vec![0u8; 0x3000];
    wr_cmd(&mut rdram, 0x1000, load_ucode_word(8), 0x2000);
    let _ = execute_display_list_f3dex2_ops(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
    );
}


#[test]
#[should_panic(expected = "data-section size 7 is not a 64-bit multiple")]
fn load_ucode_rejects_a_non_dma_granular_data_section() {
    let mut rdram = vec![0u8; 0x3000];
    wr_cmd(&mut rdram, 0x1000, (G_RDPHALF_1 as u32) << 24, 0x1800);
    wr_cmd(&mut rdram, 0x1008, load_ucode_word(7), 0x2000);
    let _ = execute_display_list_f3dex2_ops(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
    );
}


#[test]
#[should_panic(expected = "is not a 64-bit multiple")]
fn dma_io_rejects_non_multiple_transfer_size() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 7), 0x1080);
    let _ = execute_display_list_f3dex2_ops(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
    );
}


#[test]
#[should_panic(expected = "is not 64-bit aligned")]
fn dma_io_rejects_unaligned_dram_address() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0, 8), 0x1084);
    let _ = execute_display_list_f3dex2_ops(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
    );
}


#[test]
#[should_panic(expected = "crosses its 4 KiB Dmem bank")]
fn dma_io_rejects_a_transfer_that_crosses_rsp_banks() {
    let mut rdram = vec![0u8; 0x1100];
    wr_cmd(&mut rdram, 0x1000, dma_io_word(false, 0x0ff8, 16), 0x1080);
    let _ = execute_display_list_f3dex2_ops(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
    );
}


#[test]
fn homogeneous_clip_codes_identify_each_shared_cull_plane() {
    assert_eq!(homogeneous_clip_code([0.0, 0.0, 0.0, 1.0]), 0);
    assert_eq!(homogeneous_clip_code([-2.0, 0.0, 0.0, 1.0]), CLIP_NEG_X);
    assert_eq!(homogeneous_clip_code([2.0, 0.0, 0.0, 1.0]), CLIP_POS_X);
    assert_eq!(homogeneous_clip_code([0.0, -2.0, 0.0, 1.0]), CLIP_NEG_Y);
    assert_eq!(homogeneous_clip_code([0.0, 2.0, 0.0, 1.0]), CLIP_POS_Y);
    assert_eq!(homogeneous_clip_code([0.0, 0.0, -2.0, 1.0]), CLIP_NEG_Z);
    assert_eq!(homogeneous_clip_code([0.0, 0.0, 2.0, 1.0]), CLIP_POS_Z);
}


#[test]
fn line_clipping_interpolates_homogeneous_boundary_and_attributes() {
    let outside_clip = [-2.0, 0.0, 0.0, 1.0];
    let inside_clip = [0.0, 0.0, 0.0, 1.0];
    let outside = Vertex {
        r: 255,
        a: 255,
        w: 1.0,
        clip_code: homogeneous_clip_code(outside_clip),
        clip_position: Some(outside_clip),
        ..Default::default()
    };
    let inside = Vertex {
        b: 255,
        a: 255,
        w: 1.0,
        clip_code: homogeneous_clip_code(inside_clip),
        clip_position: Some(inside_clip),
        ..Default::default()
    };

    let [clipped, retained] = clip_line_to_homogeneous_volume(
        outside,
        inside,
        Some(centered_viewport()),
        ClipRatio::default(),
    )
    .unwrap();
    assert_eq!(clipped.clip_position, Some([-1.0, 0.0, 0.0, 1.0]));
    assert_eq!(clipped.x, 0.0);
    assert_eq!([clipped.r, clipped.g, clipped.b], [128, 0, 128]);
    assert_eq!(retained.x, 160.0);
    assert_eq!(clipped.clip_code, 0);
}


#[test]
fn line_clipping_rejects_shared_outside_plane() {
    let a_clip = [-2.0, 0.0, 0.0, 1.0];
    let b_clip = [-3.0, 0.0, 0.0, 1.0];
    let vertex = |clip| Vertex {
        w: 1.0,
        clip_code: homogeneous_clip_code(clip),
        clip_position: Some(clip),
        ..Default::default()
    };
    assert!(clip_line_to_homogeneous_volume(
        vertex(a_clip),
        vertex(b_clip),
        Some(centered_viewport()),
        ClipRatio::default()
    )
    .is_none());
}


#[test]
fn clip_ratio_expands_line_clip_planes_without_changing_cull_codes() {
    let outside_clip = [-4.0, 0.0, 0.0, 1.0];
    let inside_clip = [0.0, 0.0, 0.0, 1.0];
    let vertex = |clip| Vertex {
        w: 1.0,
        clip_code: homogeneous_clip_code(clip),
        clip_position: Some(clip),
        ..Default::default()
    };
    let [standard, _] = clip_line_to_homogeneous_volume(
        vertex(outside_clip),
        vertex(inside_clip),
        Some(centered_viewport()),
        ClipRatio::default(),
    )
    .unwrap();
    let [expanded, _] = clip_line_to_homogeneous_volume(
        vertex(outside_clip),
        vertex(inside_clip),
        Some(centered_viewport()),
        ClipRatio {
            neg_x: 3,
            neg_y: 3,
            pos_x: 3,
            pos_y: 3,
        },
    )
    .unwrap();
    assert_eq!(standard.clip_position, Some([-1.0, 0.0, 0.0, 1.0]));
    assert_eq!(expanded.clip_position, Some([-3.0, 0.0, 0.0, 1.0]));

    let mut state = lit_state();
    state.mvp = Some(identity());
    state.viewport = Some(centered_viewport());
    state.clip_ratio = ClipRatio {
        neg_x: 6,
        neg_y: 6,
        pos_x: 6,
        pos_y: 6,
    };
    assert_eq!(
        project_vertex(&state, 2.0, 0.0, 0.0).5,
        CLIP_POS_X,
        "G_CULLDL retained frustum codes are independent of gSPClipRatio"
    );
}


#[test]
fn culled_nested_display_list_returns_to_its_caller() {
    const ROOT: usize = 0x1000;
    const CHILD: usize = 0x1100;
    const MATRIX: usize = 0x2000;
    const VIEWPORT: usize = 0x2100;
    const VERTICES: usize = 0x2200;
    let mut rdram = vec![0u8; 0x3000];
    wr_mtx(&mut rdram, MATRIX, identity());
    wr_centered_viewport(&mut rdram, VIEWPORT);
    // Slots 0..=1 share +X clipping. Slots 2..=4 form a visible triangle.
    for (slot, (x, y)) in [(2, 0), (2, 1), (-1, -1), (1, -1), (0, 1)]
        .into_iter()
        .enumerate()
    {
        wr_vtx(
            &mut rdram,
            VERTICES + slot * VTX_STRIDE,
            x,
            y,
            0,
            [255, 255, 255, 255],
        );
    }
    let mtx_len = ((64u32 - 1) / 8) << 19;
    let mut offset = ROOT;
    wr_cmd(&mut rdram, offset, movemem_viewport_word(), VIEWPORT as u32);
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_MTX as u32) << 24) | mtx_len | 0x07,
        MATRIX as u32,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_VTX as u32) << 24) | (5 << 12) | (5 << 1),
        VERTICES as u32,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_DL as u32) << 24, CHILD as u32);
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_TRI1 as u32) << 24) | (2 << 17) | (3 << 9) | (4 << 1),
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);

    wr_cmd(&mut rdram, CHILD, (G_CULLDL as u32) << 24, 2);
    wr_cmd(
        &mut rdram,
        CHILD + 8,
        ((G_TRI1 as u32) << 24) | (2 << 17) | (3 << 9) | (4 << 1),
        0,
    );
    wr_cmd(&mut rdram, CHILD + 16, (G_ENDDL as u32) << 24, 0);

    let triangles = decode_display_list_f3dex2(&rdram, ROOT as u32).unwrap();
    assert_eq!(
        triangles.len(),
        1,
        "G_CULLDL must end only the child and resume the caller"
    );
}


#[test]
fn branch_z_uses_exact_modified_screen_depth_and_tail_branches() {
    let taken = decode_branch_z_fixture(0x0002_0000);
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].v[0].x, 10.0, "equality must take branch");

    let not_taken = decode_branch_z_fixture(0x0001_ffff);
    assert_eq!(not_taken.len(), 1);
    assert_eq!(not_taken[0].v[0].x, 0.0);
}


#[test]
fn f3dzex2_branch_w_is_strict_across_below_equal_and_above_thresholds() {
    for (vertex_w, expected_taken) in [(1.0, true), (2.0, false), (3.0, false)] {
        let state = run_branch_w_fixture(BranchWFixture {
            vertex_w,
            ..Default::default()
        });
        assert_eq!(reached_branch_w_target(&state), expected_taken);
    }
}


#[test]
fn f3dzex2_branch_w_uses_cpp_u32_to_f32_threshold_rounding() {
    let state = run_branch_w_fixture(BranchWFixture {
        vertex_w: 16_777_216.0,
        threshold: 16_777_217,
        staged_target: None,
        ..Default::default()
    });

    assert_eq!(16_777_217_u32 as f32, 16_777_216.0);
    assert!(!reached_branch_w_target(&state));
}


#[test]
fn f3dzex2_branch_w_force_branch_takes_after_validating_vertex() {
    let state = run_branch_w_fixture(BranchWFixture {
        vertex_w: 3.0,
        force_branch: true,
        ..Default::default()
    });
    assert!(reached_branch_w_target(&state));
}


#[test]
fn f3dzex2_branch_w_uses_only_w0_bits_one_through_seven_for_the_slot() {
    let state = run_branch_w_fixture(BranchWFixture {
        vertex_slot: 73,
        branch_payload_noise: 0x00ff_ff01,
        ..Default::default()
    });
    assert!(reached_branch_w_target(&state));
}


#[test]
fn f3dzex2_g_vtx_marks_high_cache_slot_loaded_for_branch_w() {
    const ROOT: usize = 0x1000;
    const TARGET: usize = 0x1200;
    const VERTEX: usize = 0x2000;
    const SLOT: usize = 126;
    let mut rdram = vec![0u8; 0x2100];
    wr_vtx(&mut rdram, VERTEX, 0, 0, 0, [255; 4]);
    wr_cmd(
        &mut rdram,
        ROOT,
        ((G_VTX as u32) << 24) | (1 << 12) | (127 << 1),
        VERTEX as u32,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 8,
        (G_RDPHALF_1 as u32) << 24,
        TARGET as u32,
    );
    wr_cmd(
        &mut rdram,
        ROOT + 16,
        ((G_BRANCH_Z as u32) << 24) | ((SLOT as u32) << 1),
        2,
    );
    wr_cmd(&mut rdram, ROOT + 24, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET + 8, (G_ENDDL as u32) << 24, 0);

    let profile = f3dzex2_profile(F3dzex2Variant::NoNFifo206H);
    let mut state = fresh_decode_state_for_profile(profile);
    let mut family = GeometryWireFamily::F3dzex2;
    initialize_geometry_profile_state(&mut state, profile);
    decode_stream(&mut rdram, ROOT as u32, &mut state, None, None, &mut family);

    assert!(state.vtx_loaded[SLOT]);
    assert_eq!(state.vtx_cache[SLOT].w, 1.0);
    assert!(reached_branch_w_target(&state));
}


#[test]
fn opcode_0x04_keeps_f3dex2_branch_z_separate_from_f3dzex2_branch_w() {
    let opposite_predicates = BranchWFixture {
        vertex_w: 0.0,
        vertex_z: 10,
        threshold: 5,
        ..Default::default()
    };
    let f3dex2 = run_branch_w_fixture(BranchWFixture {
        family: GeometryWireFamily::F3dex2,
        ..opposite_predicates
    });
    let f3dzex2 = run_branch_w_fixture(opposite_predicates);
    assert!(!reached_branch_w_target(&f3dex2));
    assert!(reached_branch_w_target(&f3dzex2));
}


#[test]
fn f3dzex2_branch_w_requires_half_one_only_when_taken() {
    let fallthrough = run_branch_w_fixture(BranchWFixture {
        vertex_w: 2.0,
        staged_target: None,
        ..Default::default()
    });
    assert!(!reached_branch_w_target(&fallthrough));

    let taken = std::panic::catch_unwind(|| {
        run_branch_w_fixture(BranchWFixture {
            staged_target: None,
            ..Default::default()
        })
    });
    assert!(taken.is_err());
}


#[test]
#[should_panic(expected = "F3DZEX2 G_BRANCH_W cache slot 127 has not been loaded")]
fn f3dzex2_branch_w_rejects_an_unloaded_cache_slot() {
    let _ = run_branch_w_fixture(BranchWFixture {
        vertex_slot: 127,
        force_branch: true,
        loaded: false,
        ..Default::default()
    });
}


#[test]
fn f3dzex2_branch_w_rejects_nonfinite_transformed_w_even_when_forced() {
    for vertex_w in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let result = std::panic::catch_unwind(|| {
            run_branch_w_fixture(BranchWFixture {
                vertex_w,
                force_branch: true,
                ..Default::default()
            })
        });
        assert!(result.is_err(), "accepted non-finite W {vertex_w}");
    }
}


#[test]
fn f3dzex2_branch_w_resolves_segment_then_masks_target_to_command_alignment() {
    let state = run_branch_w_fixture(BranchWFixture {
        staged_target: Some(0x0300_0207),
        segment_three: 0x1000,
        ..Default::default()
    });
    assert!(reached_branch_w_target(&state));
}


#[test]
fn f3dzex2_branch_w_rejects_a_taken_target_without_a_complete_command() {
    let result = std::panic::catch_unwind(|| {
        run_branch_w_fixture(BranchWFixture {
            staged_target: Some(0x00ff_fff8),
            ..Default::default()
        })
    });
    let panic = match result {
        Ok(_) => panic!("out-of-range BranchW target must trap"),
        Err(panic) => panic,
    };
    let message = if let Some(message) = panic.downcast_ref::<String>() {
        message.as_str()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        message
    } else {
        ""
    };
    assert!(message.contains("G_BRANCH_W target"));
    assert!(message.contains("no complete 8-byte command"));
}


#[test]
fn f3dzex2_branch_w_retains_w_across_modify_vertex_xy_and_z() {
    for where_field in [G_MWO_POINT_XYSCREEN, G_MWO_POINT_ZSCREEN] {
        let state = run_branch_w_fixture(BranchWFixture {
            modify_where: Some(where_field),
            ..Default::default()
        });
        assert!(reached_branch_w_target(&state));
        assert_eq!(state.vtx_cache[0].w, 1.0);
        assert_eq!(state.vtx_cache[0].clip_position, None);
    }
}


#[test]
fn admission_follows_active_rt64_force_branch_policy() {
    assert!(
        !force_branch_admission_reaches_target(false),
        "ordinary BranchZ must retain its false fallthrough"
    );
    assert!(
        force_branch_admission_reaches_target(true),
        "active RT64 forceBranch must inspect the forced target"
    );
}


#[test]
fn line3d_decodes_public_endpoint_and_half_pixel_width_fields() {
    let mut rdram = vec![0u8; 0x2100];
    wr_vtx(&mut rdram, 0x2000, 2, 4, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x2000 + VTX_STRIDE, 10, 4, 0, [0, 0, 255, 255]);
    // n=2, end=5 -> cache slots 3 and 4.
    wr_cmd(
        &mut rdram,
        0x1000,
        ((G_VTX as u32) << 24) | (2 << 12) | (5 << 1),
        0x2000,
    );
    // F3DEX2 packs slot*2 in each endpoint byte and wd in the low byte.
    wr_cmd(
        &mut rdram,
        0x1008,
        ((G_LINE3D as u32) << 24) | (8 << 16) | (6 << 8) | 3,
        0,
    );
    wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);

    let operations = decode_display_list_f3dex2_ops(&rdram, 0x1000).unwrap();
    let RenderOp::Line(line) = &operations[0] else {
        panic!("G_LINE3D must emit a typed line operation");
    };
    assert_eq!(line.v[0].x, 10.0, "wire endpoint order selects flat shade");
    assert_eq!(line.v[1].x, 2.0);
    assert_eq!(line.width, 3.0);
    assert!(!line.smooth_shading);
}


#[test]
fn l3dex_and_l3dex2_public_line_forms_normalize_to_identical_raster_input() {
    let legacy = equivalent_l3dex_line_fixture(GeometryWireFamily::L3dex);
    let modern = equivalent_l3dex_line_fixture(GeometryWireFamily::L3dex2);
    assert_eq!(legacy.len(), 1);
    assert_eq!(modern.len(), 1);

    let RenderOp::Line(legacy_line) = &legacy[0] else {
        panic!("L3DEX G_LINE3D must emit a typed line operation");
    };
    let RenderOp::Line(modern_line) = &modern[0] else {
        panic!("L3DEX2 G_LINE3D must emit a typed line operation");
    };
    assert_eq!(legacy_line.v[0].x, modern_line.v[0].x);
    assert_eq!(legacy_line.v[1].x, modern_line.v[1].x);
    assert_eq!(legacy_line.width, modern_line.width);
    assert_eq!(legacy_line.smooth_shading, modern_line.smooth_shading);
    let legacy_sample = crate::raster::test_line_attribute_sample(
        legacy_line,
        ScissorRect::framebuffer(16, 10),
        3,
        2,
    );
    let modern_sample = crate::raster::test_line_attribute_sample(
        modern_line,
        ScissorRect::framebuffer(16, 10),
        3,
        2,
    );
    assert_eq!(legacy_sample, modern_sample);
    assert_eq!(legacy_sample, (0xf0, Some((5, 5, 5))));
    let mut legacy_fb = crate::raster::Framebuffer::new(16, 10);
    legacy_fb.draw_line_no_depth(legacy_line);
    let mut modern_fb = crate::raster::Framebuffer::new(16, 10);
    modern_fb.draw_line_no_depth(modern_line);
    assert_eq!(legacy_fb.pixels, modern_fb.pixels);
    assert!(legacy_fb.pixels.iter().any(|component| *component != 0));
}
