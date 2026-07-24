//! ROM-free native evidence for the bounded S2DEX2 G_OBJ_LDTX_RECT overlay.

use std::{error::Error, io};

use fn64_render::{
    FrameStatus, M_GFXTASK, OsTask, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings, ViFilterControl, ViPixelType,
    ViPresentation,
};
use fn64_render_rt64::{Rt64Backend, Rt64SourceProvenance};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut, RspMemory};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const OVERLAY_ID: &str = "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1+s2dex-object-rect:v3";
const ADAPTER_SHA256: &str = "6ec2849acf1b4d129f290f0f1dee996140bf16048494a15a8aa44298fd751ed5";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;
const S2DEX_DL: usize = 0x1000;
const LOAD_ONLY_DL: usize = 0x1100;
const REJECT_DL: usize = 0x1200;
const LEGACY_REJECT_DL: usize = 0x1300;
const RAW_DL: usize = 0x1400;
const TXSP: u32 = 0x2000;
const IMAGE: u32 = 0x3000;
const S2DEX_TARGET: u32 = 0x4000;
const RAW_TARGET: u32 = 0x5000;
const LOAD_ONLY_TARGET: u32 = 0x6000;
const UNKNOWN_UCODE: u32 = 0x7000;
const UNKNOWN_UCODE_DATA: u32 = 0x8000;
const PRODUCTION_DL: usize = 0x9000;
const GUARD: u16 = 0x39cf;
const TARGET_SHA256: &str = "dd1694195986db0ca633c44727c0bf23f76e3feb1810b19f3b8799b6efab9c6a";
const POST_VI_SHA256: &str = "394924cd4165863fbb78e503486bcba6291f8994931beb08d8d666a114b79bef";
const ROUTE_DIGEST: u64 = 0x7438_8d65_3ac3_227f;
const INITIAL_CONTENT_DIGEST: u64 = 0xa36f_d57d_c7fc_7019;
const FINAL_CONTENT_DIGEST: u64 = 0x28cb_374e_edfe_64b3;

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn write_compound(rdram: &mut [u8], flags: u8) {
    let mut view = RdramViewMut::from_storage(rdram);
    let tx = RdramAddr::from_offset(TXSP);
    view.write_u32(tx, 0x0000_1033); // public G_OBJLT_TXTRBLOCK
    view.write_u32(tx.checked_add(4).unwrap(), IMAGE);
    view.write_u16(tx.checked_add(8).unwrap(), 0);
    view.write_u16(tx.checked_add(10).unwrap(), 1); // two 64-bit words
    view.write_u16(tx.checked_add(12).unwrap(), 1 << 11); // one word per row
    view.write_u16(tx.checked_add(14).unwrap(), 0);
    view.write_u32(tx.checked_add(16).unwrap(), 1);
    view.write_u32(tx.checked_add(20).unwrap(), u32::MAX);

    let sprite = tx.checked_add(24).unwrap();
    for (offset, value) in [
        (0, 2 * 4),
        (2, 1 << 10),
        (4, 4 << 5),
        (6, 0),
        (8, 4),
        (10, 1 << 10),
        (12, 2 << 5),
        (14, 0),
        (16, 1),
        (18, 0),
    ] {
        view.write_u16(sprite.checked_add(offset).unwrap(), value);
    }
    view.write_u8(sprite.checked_add(20).unwrap(), 0);
    view.write_u8(sprite.checked_add(21).unwrap(), 2);
    view.write_u8(sprite.checked_add(22).unwrap(), 0);
    view.write_u8(sprite.checked_add(23).unwrap(), flags);
}

fn write_texture(rdram: &mut [u8]) -> [u16; 8] {
    let pixels = [
        0xf801, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    let mut view = RdramViewMut::from_storage(rdram);
    for (index, pixel) in pixels.into_iter().enumerate() {
        view.write_u16(RdramAddr::from_offset(IMAGE + index as u32 * 2), pixel);
    }
    pixels
}

fn write_s2dex_list(
    rdram: &mut [u8],
    start: usize,
    target: u32,
    command: u8,
    dma_length: u32,
    other_mode_high: u32,
    legacy_wire: bool,
) {
    let commands = [
        (0xff10_0000 | (WIDTH - 1), target),
        (0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4)),
        (0xfc8f_ff1f, 0x88fc_f279),
        (other_mode_high, 0),
        ((u32::from(command) << 24) | dma_length, TXSP),
        (0xe900_0000, 0),
        (
            if legacy_wire {
                0xb800_0000
            } else {
                0xdf00_0000
            },
            0,
        ),
    ];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        write_command(rdram, start + index * 8, w0, w1);
    }
}

fn overwrite_compound_pointer(rdram: &mut [u8], start: usize, pointer: u32) {
    write_command(rdram, start + 4 * 8, 0x0700_002f, pointer);
}

fn write_raw_control(rdram: &mut [u8]) -> u32 {
    let texrect = (0xe400_0000 | ((6 * 4) << 12) | (3 * 4), ((2 * 4) << 12) | 4);
    let commands = [
        (0xff10_0000 | (WIDTH - 1), RAW_TARGET),
        (0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4)),
        (0xfc8f_ff1f, 0x88fc_f279),
        (0xef00_0000, 0),
        (0xfd10_0003, IMAGE),
        (0xf510_0000, 7 << 24),
        (0xf300_0000, (7 << 24) | (7 << 12) | 0x800),
        (0xf510_0200, 0x0008_0200),
        (0xf200_0000, 0x0000_c004),
        texrect,
        (0, 0x0400_0400),
        (0xe900_0000, 0),
    ];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        write_command(rdram, RAW_DL + index * 8, w0, w1);
    }
    (RAW_DL + commands.len() * 8) as u32
}

fn presentation() -> ViPresentation {
    ViPresentation {
        noise_seed: 0x5332_4f42,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn target_bytes(rdram: &[u8], address: u32) -> Vec<u8> {
    let mut bytes = vec![0; (WIDTH * HEIGHT * 2) as usize];
    RdramView::from_storage(rdram).copy_logical_bytes(RdramAddr::from_offset(address), &mut bytes);
    bytes
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn production_rejects(backend: &mut Rt64Backend, rdram: &mut [u8]) -> Result<(), Box<dyn Error>> {
    write_command(rdram, PRODUCTION_DL, 0xdf00_0000, 0);
    let status = backend.process_task(
        rdram,
        &mut RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ucode: UNKNOWN_UCODE,
            ucode_size: fn64_runtime::RSP_MEMORY_BANK_SIZE as u32,
            ucode_data: UNKNOWN_UCODE_DATA,
            ucode_data_size: 8,
            data_ptr: PRODUCTION_DL as u32,
            data_size: 8,
            ..OsTask::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other("synthetic S2DEX2 bypassed production admission").into());
    }
    Ok(())
}

fn require_named_rejection(
    result: Result<(), fn64_render::RenderError>,
    expected: &str,
) -> Result<(), Box<dyn Error>> {
    let rejection = result
        .expect_err("unsupported S2DEX object case must reject")
        .to_string();
    if !rejection.contains("G_OBJ_LDTX_RECT unsupported by bounded fn64 slice")
        || !rejection.contains(expected)
    {
        return Err(
            io::Error::other(format!("object rejection was not named: {rejection}")).into(),
        );
    }
    Ok(())
}

fn exact_route(
    route: &fn64_render_rt64::Rt64S2dexFastPathEvidence,
    workload_id: u64,
    sync_framebuffer_pair_count: u32,
) -> bool {
    route.workload_id == workload_id
        && route.source_framebuffer_identity == 0
        && route.source_address == IMAGE
        && route.source_width == 0
        && route.source_height == 0
        && route.source_size == 0
        && route.gpu_create_tile_copy_operation_count == 0
        && route.gpu_tile_dispatch_count == 0
        && route.cpu_rdram_tmem_upload_count == 1
        && route.raw_tmem_tile_count == 0
        && route.sync_framebuffer_pair_count == sync_framebuffer_pair_count
        && route.framebuffer_pair_count == 1
        && route.valid_tile_count == 1
        && route.load_operation_count == 1
        && route.distinct_source_address_count == 1
        && route.minimum_source_address == IMAGE
        && route.maximum_source_address == IMAGE
        && route.base_source_load_count == 1
        && route.offset_source_load_count == 0
        && route.load_operation_digest == ROUTE_DIGEST
        && !route.source_is_managed_framebuffer
}

fn exact_workload(
    workload: &fn64_render_rt64::Rt64DeferredWorkloadSnapshot,
    workload_id: u64,
    target: u32,
    content_digest: u64,
) -> bool {
    workload.workload_id == workload_id
        && workload.framebuffer_pair_count == 1
        && workload.content_digest == content_digest
        && workload.projection_count == 1
        && workload.game_call_count == 1
        && workload.triangle_count == 2
        && workload.vertex_count == 0
        && workload.face_index_count == 0
        && workload.rdp_param_count == 1
        && workload.load_operation_count == 1
        && workload.pair_color_addresses == [target, 0, 0, 0]
        && workload.pair_game_call_counts == [1, 0, 0, 0]
        && workload.pair_projection_counts == [1, 0, 0, 0]
        && workload.call_triangle_counts[0] == 2
        && workload.call_triangle_counts[1..]
            .iter()
            .all(|count| *count == 0)
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
        || identity.source_overlay_id != OVERLAY_ID
        || identity.adapter_source_sha256 != ADAPTER_SHA256
    {
        return Err(
            io::Error::other("object evidence requires clean pinned Metal RT64 overlay").into(),
        );
    }

    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Original,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let mut rdram = vec![0; RDRAM_LEN];
    let expected_source = write_texture(&mut rdram);
    for target in [S2DEX_TARGET, RAW_TARGET, LOAD_ONLY_TARGET] {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        view.write_u16(RdramAddr::from_offset(target - 2), GUARD);
        view.write_u16(RdramAddr::from_offset(target + WIDTH * HEIGHT * 2), GUARD);
    }
    production_rejects(&mut backend, &mut rdram)?;

    write_compound(&mut rdram, 0);
    write_s2dex_list(
        &mut rdram,
        LOAD_ONLY_DL,
        LOAD_ONLY_TARGET,
        0x05,
        0x17,
        0xef00_0000,
        false,
    );
    backend.process_synthetic_s2dex2(&mut rdram, LOAD_ONLY_DL as u32, LOAD_ONLY_TARGET)?;
    if target_bytes(&rdram, LOAD_ONLY_TARGET)
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(io::Error::other("G_OBJ_LOADTXTR control drew pixels").into());
    }

    write_s2dex_list(
        &mut rdram,
        S2DEX_DL,
        S2DEX_TARGET,
        0x07,
        0x2f,
        0xef00_0000,
        false,
    );
    backend.enable_deferred_workload_capture_for_evidence()?;
    backend.process_synthetic_s2dex2(&mut rdram, S2DEX_DL as u32, S2DEX_TARGET)?;
    let route = backend.s2dex_fast_path_evidence()?;
    let workload = backend.deferred_workload_evidence()?;
    backend.present_physical_compatibility(&rdram, presentation())?;
    let s2dex_present = backend.presented_pixels()?;
    let s2dex_bytes = target_bytes(&rdram, S2DEX_TARGET);

    let raw_end = write_raw_control(&mut rdram);
    if backend.process_rdp_commands(&mut rdram, RAW_DL as u32, raw_end, RAW_TARGET)?
        != FrameStatus::Complete
    {
        return Err(io::Error::other("independent raw-RDP control did not complete").into());
    }
    backend.present_physical_compatibility(&rdram, presentation())?;
    let raw_present = backend.presented_pixels()?;
    let raw_bytes = target_bytes(&rdram, RAW_TARGET);

    let view = RdramView::from_storage(&rdram);
    let guards_ok = [S2DEX_TARGET, RAW_TARGET, LOAD_ONLY_TARGET]
        .into_iter()
        .all(|target| {
            view.read_u16(RdramAddr::from_offset(target - 2)) == GUARD
                && view.read_u16(RdramAddr::from_offset(target + WIDTH * HEIGHT * 2)) == GUARD
        });
    let drawn = s2dex_bytes
        .chunks_exact(2)
        .filter(|pixel| *pixel != [0, 0])
        .count();
    let target_sha256 = hex(Sha256::digest(&s2dex_bytes));
    let post_vi_sha256 = hex(Sha256::digest(&s2dex_present.bytes));
    if !guards_ok
        || s2dex_bytes != raw_bytes
        || s2dex_present.bytes != raw_present.bytes
        || target_sha256 != TARGET_SHA256
        || post_vi_sha256 != POST_VI_SHA256
        || drawn != expected_source.len()
        || !exact_route(&route, 2, 1)
        || !exact_workload(&workload.current, 2, S2DEX_TARGET, INITIAL_CONTENT_DIGEST)
        || s2dex_present.workload_id != route.workload_id
        || s2dex_present.workload_id != 2
        || raw_present.workload_id != 3
        || s2dex_present.present_id != 1
        || raw_present.present_id != 2
    {
        return Err(io::Error::other(format!(
            "object/raw differential failed: guards={guards_ok} targets={} presents={} drawn={drawn} route={route:?} workload={:?}",
            s2dex_bytes == raw_bytes,
            s2dex_present.bytes == raw_present.bytes,
            workload.current,
        ))
        .into());
    }

    write_compound(&mut rdram, 1);
    write_s2dex_list(
        &mut rdram,
        REJECT_DL,
        S2DEX_TARGET,
        0x07,
        0x2f,
        0xef00_0000,
        false,
    );
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "image flip flags are nonzero",
    )?;

    write_compound(&mut rdram, 0);
    RdramViewMut::from_storage(&mut rdram).write_u32(RdramAddr::from_offset(TXSP), 0x8000_1033);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "texture command is not exact public G_OBJLT_TXTRBLOCK",
    )?;

    write_compound(&mut rdram, 0);
    RdramViewMut::from_storage(&mut rdram).write_u16(RdramAddr::from_offset(TXSP + 10), 2);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "block tsize, TMEM origin, and sprite stride/extent disagree",
    )?;

    write_compound(&mut rdram, 0);
    write_s2dex_list(
        &mut rdram,
        REJECT_DL,
        S2DEX_TARGET,
        0x07,
        0x2f,
        0xef00_0000,
        false,
    );
    overwrite_compound_pointer(&mut rdram, REJECT_DL, 0x007f_fff8);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "compound source range escapes physical RDRAM",
    )?;

    overwrite_compound_pointer(&mut rdram, REJECT_DL, TXSP + 1);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "compound source is not public 8-byte aligned",
    )?;

    overwrite_compound_pointer(&mut rdram, REJECT_DL, 0x1000_2000);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "compound source uses non-public segment bits",
    )?;
    overwrite_compound_pointer(&mut rdram, REJECT_DL, TXSP);

    write_compound(&mut rdram, 0);
    RdramViewMut::from_storage(&mut rdram).write_u32(RdramAddr::from_offset(TXSP + 4), IMAGE + 2);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "block source is not public 8-byte aligned",
    )?;

    write_compound(&mut rdram, 0);
    RdramViewMut::from_storage(&mut rdram)
        .write_u32(RdramAddr::from_offset(TXSP + 4), 0x1000_3000);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "block source uses non-public segment bits",
    )?;

    write_compound(&mut rdram, 0);
    RdramViewMut::from_storage(&mut rdram)
        .write_u32(RdramAddr::from_offset(TXSP + 4), 0x007f_fff8);
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "block source range escapes physical RDRAM",
    )?;

    // A valid compound command ran above, so a short read would otherwise
    // retain a plausible sprite tail in RT64's persistent structure buffer.
    write_compound(&mut rdram, 0);
    write_s2dex_list(
        &mut rdram,
        REJECT_DL,
        S2DEX_TARGET,
        0x07,
        0x17,
        0xef00_0000,
        false,
    );
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "DMA length low24 is not exact public 0x2f",
    )?;

    write_compound(&mut rdram, 0);
    write_s2dex_list(
        &mut rdram,
        REJECT_DL,
        S2DEX_TARGET,
        0x07,
        0x2f,
        0xef00_2000,
        false,
    );
    require_named_rejection(
        backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
        "RDP state is not point-filtered one-cycle mode",
    )?;

    for (other_mode_high, label) in [
        (0xef01_0000, "texture LOD"),
        (0xef02_0000, "texture sharpen"),
        (0xef04_0000, "texture detail"),
        (0xef00_8000, "texture lookup table"),
        (0xef08_0000, "texture perspective"),
    ] {
        write_s2dex_list(
            &mut rdram,
            REJECT_DL,
            S2DEX_TARGET,
            0x07,
            0x2f,
            other_mode_high,
            false,
        );
        require_named_rejection(
            backend.process_synthetic_s2dex2(&mut rdram, REJECT_DL as u32, S2DEX_TARGET),
            "RDP sampler state is not tile LOD, clamp detail, no TLUT, and no perspective",
        )
        .map_err(|error| io::Error::other(format!("{label} negative failed: {error}")))?;
    }

    write_s2dex_list(
        &mut rdram,
        LEGACY_REJECT_DL,
        S2DEX_TARGET,
        0xc3,
        0x2f,
        0xef00_0000,
        true,
    );
    require_named_rejection(
        backend.process_synthetic_legacy_s2dex_for_evidence(
            &mut rdram,
            LEGACY_REJECT_DL as u32,
            S2DEX_TARGET,
        ),
        "active microcode is not S2DEX2",
    )?;
    production_rejects(&mut backend, &mut rdram)?;

    // A reject must not poison persistent display-list, texture, queue, or
    // presentation state. End with another exact successful fused command.
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for offset in (0..WIDTH * HEIGHT * 2).step_by(2) {
            view.write_u16(RdramAddr::from_offset(S2DEX_TARGET + offset), 0);
        }
    }
    write_compound(&mut rdram, 0);
    write_s2dex_list(
        &mut rdram,
        S2DEX_DL,
        S2DEX_TARGET,
        0x07,
        0x2f,
        0xef00_0000,
        false,
    );
    backend.enable_deferred_workload_capture_for_evidence()?;
    backend.process_synthetic_s2dex2(&mut rdram, S2DEX_DL as u32, S2DEX_TARGET)?;
    let final_route = backend.s2dex_fast_path_evidence()?;
    let final_workload = backend.deferred_workload_evidence()?;
    backend.present_physical_compatibility(&rdram, presentation())?;
    let final_present = backend.presented_pixels()?;
    let final_bytes = target_bytes(&rdram, S2DEX_TARGET);
    let final_target_sha256 = hex(Sha256::digest(&final_bytes));
    let final_post_vi_sha256 = hex(Sha256::digest(&final_present.bytes));
    if !exact_route(&final_route, 4, 0)
        || !exact_workload(
            &final_workload.current,
            4,
            S2DEX_TARGET,
            FINAL_CONTENT_DIGEST,
        )
        || final_route.load_operation_digest != route.load_operation_digest
        || final_present.workload_id != 4
        || final_present.present_id != 3
        || final_bytes != s2dex_bytes
        || final_present.bytes != s2dex_present.bytes
        || final_target_sha256 != TARGET_SHA256
        || final_post_vi_sha256 != POST_VI_SHA256
    {
        return Err(io::Error::other(format!(
            "post-rejection recovery failed: route={final_route:?} workload={:?} present={}/{} target={} post_vi={}",
            final_workload.current,
            final_present.workload_id,
            final_present.present_id,
            final_target_sha256,
            final_post_vi_sha256,
        ))
        .into());
    }
    production_rejects(&mut backend, &mut rdram)?;

    println!(
        "rt64 S2DEX2 object rectangle verified: workload={} raw_workload={} final_workload={} route_digest={:016x} content_digest={:016x} target_sha256={} post_vi_sha256={} adapter_sha256={} overlay={}",
        s2dex_present.workload_id,
        raw_present.workload_id,
        final_present.workload_id,
        final_route.load_operation_digest,
        final_workload.current.content_digest,
        target_sha256,
        post_vi_sha256,
        identity.adapter_source_sha256,
        identity.source_overlay_id,
    );
    Ok(())
}
