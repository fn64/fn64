#![allow(
    clippy::excessive_precision,
    clippy::identity_op,
    clippy::needless_range_loop
)]
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


pub(super) fn f3dzex2_profile(variant: F3dzex2Variant) -> GeometryUcodeProfile {
    GeometryUcodeProfile::from_admission_ucode(TaskAdmissionUcode::F3dzex2(variant))
        .expect("typed F3DZEX2 admission identity is a geometry profile")
}


pub(super) fn profile_for_test_family(family: GeometryWireFamily) -> GeometryUcodeProfile {
    match family {
        GeometryWireFamily::F3dzex2 => f3dzex2_profile(F3dzex2Variant::NoNFifo206H),
        _ => GeometryUcodeProfile::from_public_family(family),
    }
}


pub(super) fn set_convert_command(coefficients: [i16; 6]) -> (u32, u32) {
    let field = |value: i16| u32::from(value as u16) & 0x1ff;
    let [k0, k1, k2, k3, k4, k5] = coefficients.map(field);
    (
        ((G_SETCONVERT as u32) << 24) | (k0 << 13) | (k1 << 4) | ((k2 >> 5) & 0x0f),
        ((k2 & 0x1f) << 27) | (k3 << 18) | (k4 << 9) | k5,
    )
}


/// Write a logical big-endian s16 at `off` through the recomp `^3` byte
/// swizzle (mirrors the decoder's `read_i16`/`read_u16` memory model).
pub(super) fn wr_i16(rdram: &mut [u8], off: usize, v: i16) {
    let b = (v as u16).to_be_bytes();
    rdram[off ^ 3] = b[0];
    rdram[(off + 1) ^ 3] = b[1];
}


pub(super) fn centered_viewport() -> Viewport {
    Viewport {
        sx: 160.0,
        sy: 120.0,
        sz: 127.75,
        tx: 160.0,
        ty: 120.0,
        tz: 127.75,
    }
}


pub(super) fn wr_centered_viewport(rdram: &mut [u8], off: usize) {
    for (index, value) in [640, 480, 511, 0, 640, 480, 511, 0].into_iter().enumerate() {
        wr_i16(rdram, off + index * 2, value);
    }
}


pub(super) fn movemem_viewport_word() -> u32 {
    ((G_MOVEMEM as u32) << 24) | (1 << 19) | u32::from(G_MV_VIEWPORT)
}


/// Write an aligned logical 32-bit word (recomp `MEM_W`: native-endian,
/// no swizzle), matching the decoder's `read_u32`. Used to plant raw
/// display-list command words.
pub(super) fn wr_u32(rdram: &mut [u8], off: usize, v: u32) {
    rdram[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}


/// Plant one 8-byte F3DEX2 command (`w0`, `w1`) at byte offset `off`.
pub(super) fn wr_cmd(rdram: &mut [u8], off: usize, w0: u32, w1: u32) {
    wr_u32(rdram, off, w0);
    wr_u32(rdram, off + 4, w1);
}


pub(super) fn dma_io_word(write_to_dram: bool, rsp_address: u16, size: u16) -> u32 {
    assert!(rsp_address < 0x2000 && rsp_address.is_multiple_of(8));
    assert!((1..=0x1000).contains(&size));
    ((G_DMA_IO as u32) << 24)
        | (u32::from(write_to_dram) << 23)
        | ((u32::from(rsp_address) / 8) << 13)
        | (u32::from(size) - 1)
}


pub(super) fn load_ucode_word(data_size: u16) -> u32 {
    assert!((1..=0x1000).contains(&data_size));
    ((G_LOAD_UCODE as u32) << 24) | (u32::from(data_size) - 1)
}


#[derive(Debug, PartialEq, Eq)]
pub(super) struct AdmissionProjection {
    pub(super) generations: Vec<TaskAdmissionGeneration>,
    pub(super) plan_sha256: [u8; 32],
    pub(super) raw_windows: Vec<(Vec<u8>, Vec<u8>)>,
    pub(super) dp_full_sync: fn64_render::DpFullSyncStatus,
    pub(super) full_sync_count: u64,
}


pub(super) fn geometry_admission_fixture() -> (
    Vec<u8>,
    fn64_runtime::RspMemory,
    fn64_render::OsTask,
    GeometryUcodeCatalog,
) {
    const TEXT: u32 = 0x2000;
    const DATA: u32 = 0x4000;
    let mut rdram = vec![0; 0xa000];
    let text = (0..SP_UCODE_SIZE)
        .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
        .collect::<Vec<_>>();
    let data = [0x31, 0x42, 0x53, 0x64, 0x75, 0x86, 0x97, 0xa8];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(TEXT), &text);
        view.write_logical_bytes(fn64_runtime::RdramAddr::from_offset(DATA), &data);
    }
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &text,
        )
        .unwrap();
    let mut catalog = GeometryUcodeCatalog::default();
    catalog.admit_text(&text);
    let task = fn64_render::OsTask {
        ucode: TEXT,
        ucode_data: DATA,
        ucode_data_size: data.len() as u32,
        data_ptr: 0x1000,
        ..fn64_render::OsTask::default()
    };
    (rdram, rsp_memory, task, catalog)
}


pub(super) fn reference_admission_projection(
    rdram: &[u8],
    rsp_memory: &fn64_runtime::RspMemory,
    task: &fn64_render::OsTask,
    catalog: &GeometryUcodeCatalog,
    force_branch: bool,
) -> Result<AdmissionProjection, RenderError> {
    const WINDOW: TaskAdmissionRawWindowSize = TaskAdmissionRawWindowSize { text: 16, data: 8 };
    let profile =
        catalog.require_profile_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))?;
    let family = profile.wire_family();
    let text_address = task.ucode & 0x00ff_ffff;
    let data_address = task.ucode_data & 0x00ff_ffff;
    let mut text = vec![0; SP_UCODE_SIZE];
    let mut data = vec![0; task.ucode_data_size as usize];
    let view = fn64_runtime::RdramView::from_storage(rdram);
    view.copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(text_address),
        &mut text,
    );
    view.copy_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(data_address),
        &mut data,
    );
    let entry = TaskAdmissionGeneration {
        source: TaskAdmissionSource::TaskEntry,
        text_address,
        data_address,
        text_sha256: UcodeDigest::from_text(&text),
        data: MicrocodeDataImageIdentity {
            bytes: task.ucode_data_size,
            sha256: Sha256::digest(data).into(),
        },
        ucode: profile.admission_ucode(),
    };

    let mut scratch_rdram = rdram.to_vec();
    let mut scratch_rsp = rsp_memory.clone();
    let inspection = inspect_display_list_geometry_admission_with_raw_windows(
        &mut scratch_rdram,
        &mut scratch_rsp,
        task.data_ptr,
        catalog,
        &mut RdpDecodeState::default(),
        family,
        GeometryTaskAdmissionOptions {
            raw_window_size: WINDOW,
            force_branch,
        },
    )?;
    let full_sync_count = inspection
        .operations
        .iter()
        .filter(|operation| matches!(operation, RenderOp::FullSync))
        .count() as u64;
    let plan = fn64_render::TaskAdmissionPlan::new(entry, inspection.self_loads);
    let mut raw_windows = vec![(
        rdram[text_address as usize..text_address as usize + WINDOW.text].to_vec(),
        rdram[data_address as usize..data_address as usize + WINDOW.data].to_vec(),
    )];
    raw_windows.extend(
        inspection
            .self_load_raw_windows
            .into_iter()
            .map(|window| (window.text, window.data)),
    );
    Ok(AdmissionProjection {
        generations: plan.generations().to_vec(),
        plan_sha256: plan.sha256(),
        raw_windows,
        dp_full_sync: if full_sync_count == 0 {
            fn64_render::DpFullSyncStatus::NotReached
        } else {
            fn64_render::DpFullSyncStatus::Reached
        },
        full_sync_count,
    })
}


pub(super) fn shared_admission_projection(
    rdram: &[u8],
    rsp_memory: &fn64_runtime::RspMemory,
    task: &fn64_render::OsTask,
    catalog: &GeometryUcodeCatalog,
    force_branch: bool,
) -> Result<AdmissionProjection, RenderError> {
    let inspection = fn64_render::inspect_geometry_task(
        rdram,
        rsp_memory,
        task,
        catalog,
        fn64_render::GeometryTaskInspectionPolicy { force_branch },
        Some(fn64_render::TaskAdmissionRawWindowSize { text: 16, data: 8 }),
    )?;
    Ok(AdmissionProjection {
        generations: inspection.admission_plan.generations().to_vec(),
        plan_sha256: inspection.admission_plan.sha256(),
        raw_windows: inspection
            .raw_windows
            .into_vec()
            .into_iter()
            .map(|window| (window.text, window.data))
            .collect(),
        dp_full_sync: inspection.dp_full_sync,
        full_sync_count: inspection.full_sync_count,
    })
}


pub(super) fn assert_shared_admission_matches_reference(
    rdram: &[u8],
    rsp_memory: &fn64_runtime::RspMemory,
    task: &fn64_render::OsTask,
    catalog: &GeometryUcodeCatalog,
    force_branch: bool,
) -> AdmissionProjection {
    let reference =
        reference_admission_projection(rdram, rsp_memory, task, catalog, force_branch).unwrap();
    let shared =
        shared_admission_projection(rdram, rsp_memory, task, catalog, force_branch).unwrap();
    assert_eq!(shared, reference);
    shared
}


pub(super) fn decode_branch_z_fixture(threshold: u32) -> Vec<Triangle> {
    const ROOT: usize = 0x1000;
    const TARGET: usize = 0x1200;
    const VERTICES: usize = 0x2000;
    let mut rdram = vec![0u8; 0x3000];
    for (slot, (x, y, color)) in [
        (0, 0, [255, 0, 0, 255]),
        (4, 0, [255, 0, 0, 255]),
        (0, 4, [255, 0, 0, 255]),
        (10, 10, [0, 255, 0, 255]),
        (14, 10, [0, 255, 0, 255]),
        (10, 14, [0, 255, 0, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        wr_vtx(&mut rdram, VERTICES + slot * VTX_STRIDE, x, y, 0, color);
    }
    let mut offset = ROOT;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_VTX as u32) << 24) | (6 << 12) | (6 << 1),
        VERTICES as u32,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_MODIFYVTX as u32) << 24) | ((G_MWO_POINT_ZSCREEN as u32) << 16),
        0x0002_0000,
    );
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        (G_RDPHALF_1 as u32) << 24,
        TARGET as u32,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_BRANCH_Z as u32) << 24, threshold);
    offset += 8;
    wr_cmd(
        &mut rdram,
        offset,
        ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    offset += 8;
    wr_cmd(&mut rdram, offset, (G_ENDDL as u32) << 24, 0);
    wr_cmd(
        &mut rdram,
        TARGET,
        ((G_TRI1 as u32) << 24) | (3 << 17) | (4 << 9) | (5 << 1),
        0,
    );
    wr_cmd(&mut rdram, TARGET + 8, (G_ENDDL as u32) << 24, 0);
    decode_display_list_f3dex2(&rdram, ROOT as u32).unwrap()
}


#[derive(Copy, Clone)]
pub(super) struct BranchWFixture {
    pub(super) family: GeometryWireFamily,
    pub(super) vertex_slot: usize,
    pub(super) vertex_w: f32,
    pub(super) vertex_z: u32,
    pub(super) threshold: u32,
    pub(super) force_branch: bool,
    pub(super) loaded: bool,
    pub(super) branch_payload_noise: u32,
    pub(super) staged_target: Option<u32>,
    pub(super) segment_three: u32,
    pub(super) modify_where: Option<u8>,
}


impl Default for BranchWFixture {
    fn default() -> Self {
        Self {
            family: GeometryWireFamily::F3dzex2,
            vertex_slot: 0,
            vertex_w: 1.0,
            vertex_z: 0,
            threshold: 2,
            force_branch: false,
            loaded: true,
            branch_payload_noise: 0,
            staged_target: Some(0x1200),
            segment_three: 0,
            modify_where: None,
        }
    }
}


pub(super) fn run_branch_w_fixture(fixture: BranchWFixture) -> DecodeState {
    let BranchWFixture {
        family,
        vertex_slot,
        vertex_w,
        vertex_z,
        threshold,
        force_branch,
        loaded,
        branch_payload_noise,
        staged_target,
        segment_three,
        modify_where,
    } = fixture;
    const ROOT: usize = 0x1000;
    const TARGET: usize = 0x1200;
    let mut rdram = vec![0u8; 0x1300];
    let mut pc = ROOT;
    if let Some(where_field) = modify_where {
        wr_cmd(
            &mut rdram,
            pc,
            ((G_MODIFYVTX as u32) << 24)
                | (u32::from(where_field) << 16)
                | (vertex_slot as u32 * 2),
            if where_field == G_MWO_POINT_ZSCREEN {
                0xffff_0000
            } else {
                0x0040_0080
            },
        );
        pc += 8;
    }
    if let Some(target) = staged_target {
        wr_cmd(&mut rdram, pc, (G_RDPHALF_1 as u32) << 24, target);
        pc += 8;
    }
    let branch_slot_payload = if family == GeometryWireFamily::F3dzex2 {
        (vertex_slot as u32) << 1
    } else {
        ((vertex_slot as u32 * 5) << 12) | (vertex_slot as u32 * 2)
    };
    wr_cmd(
        &mut rdram,
        pc,
        ((G_BRANCH_Z as u32) << 24) | branch_payload_noise | branch_slot_payload,
        threshold,
    );
    wr_cmd(&mut rdram, pc + 8, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET + 8, (G_ENDDL as u32) << 24, 0);

    let profile = profile_for_test_family(family);
    let mut state = fresh_decode_state_for_profile(profile);
    state.vtx_cache[vertex_slot].w = vertex_w;
    state.vtx_cache[vertex_slot].z_screen = vertex_z;
    state.vtx_cache[vertex_slot].clip_position = Some([0.0, 0.0, 0.0, vertex_w]);
    state.vtx_loaded[vertex_slot] = loaded;
    state.force_branch = force_branch;
    state.segments[3] = segment_three;
    initialize_geometry_profile_state(&mut state, profile);
    let mut active_family = family;
    decode_stream(
        &mut rdram,
        ROOT as u32,
        &mut state,
        None,
        None,
        &mut active_family,
    );
    state
}


pub(super) fn reached_branch_w_target(state: &DecodeState) -> bool {
    state
        .ops
        .iter()
        .any(|operation| matches!(operation, RenderOp::FullSync))
}


pub(super) fn force_branch_admission_reaches_target(force_branch: bool) -> bool {
    const ROOT: usize = 0x1000;
    const TARGET: usize = 0x1100;
    let mut rdram = vec![0u8; 0x2000];
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
    wr_cmd(&mut rdram, ROOT + 24, (G_ENDDL as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET, (G_RDPFULLSYNC as u32) << 24, 0);
    wr_cmd(&mut rdram, TARGET + 8, (G_ENDDL as u32) << 24, 0);

    let inspection = inspect_display_list_geometry_admission_with_raw_windows(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        ROOT as u32,
        &GeometryUcodeCatalog::default(),
        &mut RdpDecodeState::default(),
        GeometryWireFamily::F3dex2,
        GeometryTaskAdmissionOptions {
            raw_window_size: TaskAdmissionRawWindowSize { text: 1, data: 1 },
            force_branch,
        },
    )
    .unwrap();
    inspection
        .operations
        .iter()
        .any(|operation| matches!(operation, RenderOp::FullSync))
}


pub(super) fn decode_geometry_fixture(rdram: &[u8], family: GeometryWireFamily) -> Vec<RenderOp> {
    let mut scratch = rdram.to_vec();
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    let mut rdp_state = RdpDecodeState::default();
    execute_display_list_geometry_ops_admitted_with_rdp_state(
        &mut scratch,
        &mut rsp_memory,
        0x1000,
        &GeometryUcodeCatalog::default(),
        &mut rdp_state,
        family,
    )
    .unwrap()
}


pub(super) fn decode_geometry_fixture_profile(
    rdram: &[u8],
    profile: GeometryUcodeProfile,
) -> Vec<RenderOp> {
    let mut scratch = rdram.to_vec();
    execute_display_list_f3dex2_state_with_catalog_and_rdp(
        &mut scratch,
        &mut fn64_runtime::RspMemory::new(),
        0x1000,
        None,
        None,
        profile,
        DecodeAdmissionPolicy::default(),
    )
    .unwrap()
    .ops
}


pub(super) fn equivalent_l3dex_line_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
    let mut rdram = vec![0u8; 0x2100];
    wr_vtx(&mut rdram, 0x2000, 2, 4, 0, [255, 0, 0, 255]);
    wr_vtx(&mut rdram, 0x2000 + VTX_STRIDE, 10, 4, 0, [0, 0, 255, 255]);
    match family {
        GeometryWireFamily::L3dex => {
            // Public F3DEX envelope: v0*2 in bits 23:16 and
            // ((n << 10) | (sizeof(Vtx) * n - 1)) in the low halfword.
            wr_cmd(
                &mut rdram,
                0x1000,
                ((L3DEX_G_VTX as u32) << 24) | (6 << 16) | (2 << 10) | 31,
                0x2000,
            );
            wr_cmd(
                &mut rdram,
                0x1008,
                (L3DEX_G_LINE3D as u32) << 24,
                (8 << 16) | (6 << 8) | 3,
            );
            wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::L3dex2 => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((G_VTX as u32) << 24) | (2 << 12) | (5 << 1),
                0x2000,
            );
            wr_cmd(
                &mut rdram,
                0x1008,
                ((G_LINE3D as u32) << 24) | (8 << 16) | (6 << 8) | 3,
                0,
            );
            wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::Fast3d
        | GeometryWireFamily::F3dex
        | GeometryWireFamily::F3dlx
        | GeometryWireFamily::F3dlxRej
        | GeometryWireFamily::F3dex2
        | GeometryWireFamily::F3dex2NoN
        | GeometryWireFamily::F3dex2Rej
        | GeometryWireFamily::F3dlx2Rej
        | GeometryWireFamily::F3dzex2 => {
            panic!("fixture requires a line microcode family")
        }
    }
    decode_geometry_fixture(&rdram, family)
}


pub(super) fn equivalent_polygon_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
    let mut rdram = vec![0u8; 0x2100];
    for (slot, (x, y, color)) in [
        (1, 1, [255, 0, 0, 255]),
        (6, 1, [0, 255, 0, 255]),
        (1, 6, [0, 0, 255, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        wr_vtx(&mut rdram, 0x2000 + slot * VTX_STRIDE, x, y, 0, color);
    }
    match family {
        GeometryWireFamily::Fast3d => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((L3DEX_G_VTX as u32) << 24) | (0x20 << 16) | 48,
                0x2000,
            );
            wr_cmd(
                &mut rdram,
                0x1008,
                (L3DEX_G_SETGEOMETRYMODE as u32) << 24,
                LEGACY_G_SHADING_SMOOTH,
            );
            wr_cmd(
                &mut rdram,
                0x1010,
                (L3DEX_G_TRI1 as u32) << 24,
                (10 << 8) | 20,
            );
            wr_cmd(&mut rdram, 0x1018, (L3DEX_G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::F3dex
        | GeometryWireFamily::F3dlx
        | GeometryWireFamily::F3dlxRej => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((L3DEX_G_VTX as u32) << 24) | (3 << 10) | 47,
                0x2000,
            );
            wr_cmd(
                &mut rdram,
                0x1008,
                (L3DEX_G_SETGEOMETRYMODE as u32) << 24,
                LEGACY_G_SHADING_SMOOTH,
            );
            wr_cmd(
                &mut rdram,
                0x1010,
                (L3DEX_G_TRI1 as u32) << 24,
                (2 << 8) | 4,
            );
            wr_cmd(&mut rdram, 0x1018, (L3DEX_G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::F3dex2
        | GeometryWireFamily::F3dex2NoN
        | GeometryWireFamily::F3dex2Rej
        | GeometryWireFamily::F3dlx2Rej => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
                0x2000,
            );
            wr_cmd(
                &mut rdram,
                0x1008,
                ((G_GEOMETRYMODE as u32) << 24) | 0x00ff_ffff,
                G_SHADING_SMOOTH,
            );
            wr_cmd(
                &mut rdram,
                0x1010,
                ((G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
                0,
            );
            wr_cmd(&mut rdram, 0x1018, (G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::F3dzex2
        | GeometryWireFamily::L3dex
        | GeometryWireFamily::L3dex2 => {
            panic!("fixture requires a polygon microcode family")
        }
    }
    decode_geometry_fixture(&rdram, family)
}


pub(super) fn published_quadrangle_fixture(family: GeometryWireFamily) -> Vec<RenderOp> {
    let mut rdram = vec![0u8; 0x2100];
    for (slot, (x, y)) in [(1, 1), (6, 1), (6, 6), (1, 6)].into_iter().enumerate() {
        wr_vtx(
            &mut rdram,
            0x2000 + slot * VTX_STRIDE,
            x,
            y,
            0,
            [255, 255, 255, 255],
        );
    }
    let first = (1 << 9) | (2 << 1);
    let second = (2 << 9) | (3 << 1);
    match family {
        GeometryWireFamily::F3dex => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((L3DEX_G_VTX as u32) << 24) | (4 << 10) | 63,
                0x2000,
            );
            // Current public gSP1Quadrangle lowers to G_TRI2 with the
            // v0,v1,v2 and v0,v2,v3 split for flat-shade flag zero.
            wr_cmd(
                &mut rdram,
                0x1008,
                ((F3DEX_G_TRI2 as u32) << 24) | first,
                second,
            );
            wr_cmd(&mut rdram, 0x1010, (L3DEX_G_ENDDL as u32) << 24, 0);
        }
        GeometryWireFamily::F3dex2 => {
            wr_cmd(
                &mut rdram,
                0x1000,
                ((G_VTX as u32) << 24) | (4 << 12) | (4 << 1),
                0x2000,
            );
            wr_cmd(&mut rdram, 0x1008, ((G_QUAD as u32) << 24) | first, second);
            wr_cmd(&mut rdram, 0x1010, (G_ENDDL as u32) << 24, 0);
        }
        _ => panic!("fixture requires a published F3DEX/F3DEX2 quadrangle form"),
    }
    decode_geometry_fixture(&rdram, family)
}


/// Pack raw `gsDPSetCombineLERP` selectors exactly like public gbi.h's
/// `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1` macros (lines 3543-3565).
pub(super) fn combine_cmd(
    rgb0: [u32; 4],
    alpha0: [u32; 4],
    rgb1: [u32; 4],
    alpha1: [u32; 4],
) -> (u32, u32) {
    let w0 = ((G_SETCOMBINE as u32) << 24)
        | ((rgb0[0] & 0x0f) << 20)
        | ((rgb0[2] & 0x1f) << 15)
        | ((alpha0[0] & 0x07) << 12)
        | ((alpha0[2] & 0x07) << 9)
        | ((rgb1[0] & 0x0f) << 5)
        | (rgb1[2] & 0x1f);
    let w1 = ((rgb0[1] & 0x0f) << 28)
        | ((rgb1[1] & 0x0f) << 24)
        | ((alpha1[0] & 0x07) << 21)
        | ((alpha1[2] & 0x07) << 18)
        | ((rgb0[3] & 0x07) << 15)
        | ((alpha0[1] & 0x07) << 12)
        | ((alpha0[3] & 0x07) << 9)
        | ((rgb1[3] & 0x07) << 6)
        | ((alpha1[1] & 0x07) << 3)
        | (alpha1[3] & 0x07);
    (w0, w1)
}


/// Plant a full 16-byte `Vtx` (`ob` x/y/z at 0/2/4, color at 12) at `off`
/// so a `G_VTX` + `G_TRI1` can resolve a real triangle.
pub(super) fn wr_vtx(rdram: &mut [u8], off: usize, x: i16, y: i16, z: i16, rgba: [u8; 4]) {
    wr_i16(rdram, off, x);
    wr_i16(rdram, off + 2, y);
    wr_i16(rdram, off + 4, z);
    for (i, &c) in rgba.iter().enumerate() {
        rdram[(off + 12 + i) ^ 3] = c;
    }
}


/// Write a 64-byte fixed-point `Mtx` at `off` from an f32 `[row][col]`
/// matrix, matching `read_mtx`'s layout: element (r,c) integer half at
/// `off + (r*4+c)*2`, fractional half at `off + 32 + (r*4+c)*2`, both
/// through the recomp `^3` swizzle (via `wr_i16`).
pub(super) fn wr_mtx(rdram: &mut [u8], off: usize, m: [[f32; 4]; 4]) {
    for (r, row) in m.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            let elem = r * 4 + c;
            let fixed = (*value * 65536.0).round() as i32;
            let int_half = (fixed >> 16) as i16;
            let frac_half = (fixed & 0xFFFF) as u16;
            wr_i16(rdram, off + elem * 2, int_half);
            wr_i16(rdram, off + 32 + elem * 2, frac_half as i16);
        }
    }
}


/// Encode the F3DEX2 partial other-mode range used by
/// `gSPSetOtherMode` (`gbi.h:3353-3369`).
pub(super) fn other_mode_cmd(opcode: u8, shift: u32, length: u32) -> u32 {
    ((opcode as u32) << 24) | ((32 - shift - length) << 8) | (length - 1)
}


/// Write a byte at logical offset `off` through the recomp `^3` swizzle
/// (mirrors `read_u8`'s memory model), so tests plant Light_t/Vtx bytes
/// the way a real DMA would.
pub(super) fn wr_u8(rdram: &mut [u8], off: usize, v: u8) {
    rdram[off ^ 3] = v;
}


/// A `DecodeState` with identity modelview and no MVP -- the minimal
/// harness for exercising the light math directly.
pub(super) fn lit_state() -> DecodeState {
    fresh_decode_state()
}


pub(super) fn texture_generation_state(linear: bool) -> DecodeState {
    let mut state = lit_state();
    state.geometry_mode =
        G_LIGHTING | G_TEXTURE_GEN | if linear { G_TEXTURE_GEN_LINEAR } else { 0 };
    state.look_at = LookAtState {
        x: Some([1.0, 0.0, 0.0]),
        y: Some([0.0, 1.0, 0.0]),
    };
    // Public manual example: gSPTexture(tex_max << 6, ...). A tex_max of
    // 31 must therefore be the generated endpoint at a +1 projection.
    state.tex.tex_scale_s = (31 << 6) as f32 / 65536.0;
    state.tex.tex_scale_t = (31 << 6) as f32 / 65536.0;
    state
}


pub(super) fn assert_texture_coords_close(actual: (f32, f32), expected: (f32, f32)) {
    assert!(
        (actual.0 - expected.0).abs() <= 1e-5 && (actual.1 - expected.1).abs() <= 1e-5,
        "texture coordinates differ: actual={actual:?}, expected={expected:?}"
    );
}


pub(super) fn vtx_w(w: f32) -> Vertex {
    Vertex {
        w,
        ..Default::default()
    }
}


/// Build a 2×2 RGBA8888 texture: TL=red, TR=green, BL=blue, BR=white.
pub(super) fn checker_2x2(clamp: bool) -> Texture {
    let texels = vec![
        255, 0, 0, 255, // (0,0) red
        0, 255, 0, 255, // (1,0) green
        0, 0, 255, 255, // (0,1) blue
        255, 255, 255, 255, // (1,1) white
    ];
    Texture {
        format: 0,
        size: 2,
        width: 2,
        height: 2,
        texels: std::rc::Rc::new(texels),
        clamp_s: clamp,
        clamp_t: clamp,
        mirror_s: false,
        mirror_t: false,
        mask_s: if clamp { 0 } else { 1 },
        mask_t: if clamp { 0 } else { 1 },
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    }
}


pub(super) fn indexed_texture(width: u32) -> Texture {
    let texels = (0..width)
        .flat_map(|index| [index as u8, 0, 0, 255])
        .collect();
    Texture {
        format: G_IM_FMT_RGBA,
        size: G_IM_SIZ_32B,
        width,
        height: 1,
        texels: std::rc::Rc::new(texels),
        clamp_s: false,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 0,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    }
}


pub(super) fn panic_text(f: impl FnOnce()) -> String {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .expect_err("operation did not panic");
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(text) => (*text).to_owned(),
            Err(_) => panic!("panic payload was not text"),
        },
    }
}


pub(super) fn assert_texture_row(
    bytes: &[u8],
    width: u16,
    fmt: u8,
    siz: u8,
    palette: u8,
    tlut: Vec<[u8; 4]>,
    expected: &[u8],
) {
    let base = 0x100usize;
    let mut rdram = vec![0u8; base + bytes.len() + 4];
    for (i, &byte) in bytes.iter().enumerate() {
        wr_u8(&mut rdram, base + i, byte);
    }

    let mut tex = TexState {
        timg_addr: base as u32,
        tlut,
        ..Default::default()
    };
    tex.tiles[0] = Tile {
        fmt,
        siz,
        palette,
        lrs: (width - 1) * 4,
        ..Default::default()
    };
    assert_eq!(
        decode_current_texture(&rdram, &tex, &[0; 16], 0, TextureLoad::Block)
            .texels
            .as_slice(),
        expected
    );
}
