//! Synthetic, non-ROM causal evidence for RT64's two S2DEX enhancements.

use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, OsTask, RenderAspectRatio, RenderBackend, RenderConfig, RenderEnhancementSettings,
    RenderFiltering, RenderGraphicsApi, RenderPolicyApply, RenderRuntimeSettings, ViFilterControl,
    ViPixelType, ViPresentation, M_GFXTASK,
};
use fn64_render_rt64::{Rt64Backend, Rt64S2dexFastPathEvidence, Rt64SourceProvenance};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut, RspMemory};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SOURCE_COMMANDS: usize = 0x1000;
const S2DEX_COMMANDS: usize = 0x2000;
const BACKGROUND: u32 = 0x2800;
const ORDINARY_SOURCE: u32 = 0x0030_0000;
const SOURCE: u32 = 0x0040_0000;
const TARGET: u32 = 0x0041_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const INSET_WIDTH: u32 = 60;
const INSET_HEIGHT: u32 = 60;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const GUARD: u16 = 0x39cf;
const CONTROL_POLICY: &str = "0ae411439f1b742ee2017a8f537212767925b71810b4813461becefdee40f3e9";
const FAST_POLICY: &str = "7b73fb8c7d547e59aecb67b2d0a001ce21d0bad6f3bf2730d7dffdfa6237ca54";
const FIX_POLICY: &str = "a3b4dafb32caa764476b2bc5138c7df4ab57aba0ffd863d639dd90cf793dc917";
const PIXELS: &str = "1c6bb9863b124ebc394d9ce73d1287d9ad651814e655c949a70118f44c334df2";
const TARGET_BYTES: &str = "d41df4d6472ee3f7a8440e3f92b1f9bc96d6aaee4fb6a23e813cdf2208118e81";
const MATCHED_BILERP_PIXELS: &str =
    "1c6bb9863b124ebc394d9ce73d1287d9ad651814e655c949a70118f44c334df2";
const MATCHED_BILERP_TARGET_BYTES: &str =
    "d41df4d6472ee3f7a8440e3f92b1f9bc96d6aaee4fb6a23e813cdf2208118e81";
const CONTROL_LOAD_DIGEST: u64 = 11_081_332_784_341_843_569;
const INSET_POINT_LOAD_DIGEST: u64 = 404_009_634_700_744_000;
const BILERP_LOAD_DIGEST: u64 = 11_746_348_200_741_240_963;
const FAST_LOAD_DIGEST: u64 = 13_074_734_122_227_382_117;
const ORDINARY_LOAD_DIGEST: u64 = 17_405_972_784_478_231_233;

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

fn write_source_commands(rdram: &mut [u8]) -> u32 {
    let commands = [
        (0xef30_00f0, 0),
        (0xff10_0000 | (WIDTH - 1), SOURCE),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
        (0xf700_0000, u32::from(BLUE) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, 0, HEIGHT / 2),
        (0xe900_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(rdram, SOURCE_COMMANDS + index * 8, word0, word1);
    }
    (SOURCE_COMMANDS + commands.len() * 8) as u32
}

fn write_ordinary_source(rdram: &mut [u8]) {
    let mut view = RdramViewMut::from_storage(rdram);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            view.write_u16(
                RdramAddr::from_offset(ORDINARY_SOURCE + (y * WIDTH + x) * 2),
                if y < HEIGHT / 2 { RED } else { BLUE },
            );
        }
    }
}

fn write_background(rdram: &mut [u8], image: u32, frame_width: u32, frame_height: u32) {
    let mut view = RdramViewMut::from_storage(rdram);
    let base = RdramAddr::from_offset(BACKGROUND);
    for (offset, value) in [
        (0, 0),
        (2, WIDTH as u16 * 4),
        (4, 0),
        (6, frame_width as u16 * 4),
        (8, 0),
        (10, HEIGHT as u16 * 4),
        (12, 0),
        (14, frame_height as u16 * 4),
        (20, 0xfff4),
        (24, 0),
        (26, 0),
        (28, 1 << 10),
        (30, 1 << 10),
    ] {
        view.write_u16(base.checked_add(offset).unwrap(), value);
    }
    view.write_u32(base.checked_add(16).unwrap(), image);
    view.write_u8(base.checked_add(22).unwrap(), 0);
    view.write_u8(base.checked_add(23).unwrap(), 2);
    view.write_u32(base.checked_add(32).unwrap(), 0);
    view.write_u32(base.checked_add(36).unwrap(), 0);
}

fn write_s2dex_commands(
    rdram: &mut [u8],
    image: u32,
    frame_width: u32,
    frame_height: u32,
    obj_bilerp: bool,
    rdp_bilerp: bool,
) {
    write_background(rdram, image, frame_width, frame_height);
    let commands = [
        (0xff10_0000 | (WIDTH - 1), TARGET),
        (0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4)),
        (0xfc8f_ff1f, 0x88fc_f279),
        (if rdp_bilerp { 0xef00_20f0 } else { 0xef00_00f0 }, 0),
        (0x0b00_0000, if obj_bilerp { 0x08 } else { 0 }),
        (0x0900_0000, BACKGROUND),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(rdram, S2DEX_COMMANDS + index * 8, word0, word1);
    }
}

fn require_production_rejection(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
) -> Result<(), Box<dyn Error>> {
    let status = backend.process_task(
        rdram,
        &mut RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..Default::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "synthetic S2DEX2 evidence bypassed production microcode admission",
        )
        .into());
    }
    Ok(())
}

fn apply(
    backend: &mut Rt64Backend,
    settings: &RenderEnhancementSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    match backend.apply_enhancement_settings(settings)? {
        RenderPolicyApply::LiveApplied { policy_sha256 }
            if policy_sha256 == backend.configured_runtime_policy().sha256() =>
        {
            Ok(policy_sha256)
        }
        result => Err(io::Error::other(format!(
            "S2DEX enhancement did not apply live and exactly: {result:?}"
        ))
        .into()),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    policy: [u8; 32],
    pixels: [u8; 32],
    target: [u8; 32],
    route: Rt64S2dexFastPathEvidence,
}

fn capture(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    image: u32,
    frame_width: u32,
    frame_height: u32,
    obj_bilerp: bool,
    rdp_bilerp: bool,
) -> Result<Observation, Box<dyn Error>> {
    write_s2dex_commands(
        rdram,
        image,
        frame_width,
        frame_height,
        obj_bilerp,
        rdp_bilerp,
    );
    backend.enable_deferred_workload_capture_for_evidence()?;
    backend.process_synthetic_s2dex2(rdram, S2DEX_COMMANDS as u32, TARGET)?;
    let route = backend.s2dex_fast_path_evidence()?;
    backend.present_physical_compatibility(
        &*rdram,
        ViPresentation {
            noise_seed: 0x5332_4445,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    let pixels = backend.presented_pixels()?;
    let mut target = vec![0; (WIDTH * HEIGHT * 2) as usize];
    RdramView::from_storage(rdram).copy_logical_bytes(RdramAddr::from_offset(TARGET), &mut target);
    Ok(Observation {
        policy: backend
            .active_runtime_policy()
            .ok_or_else(|| io::Error::other("S2DEX capture has no active policy"))?
            .sha256(),
        pixels: Sha256::digest(&pixels.bytes).into(),
        target: Sha256::digest(target).into(),
        route,
    })
}

fn hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RouteIdentity {
    load_operation_digest: u64,
    source_address: u32,
    source_width: u32,
    source_height: u32,
    source_size: u32,
    gpu_operations: u32,
    gpu_tiles: u32,
    cpu_uploads: u32,
    raw_tmem_tiles: u32,
    sync_pairs: u32,
    framebuffer_pairs: u32,
    valid_tiles: u32,
    load_operations: u32,
    distinct_addresses: u32,
    base_loads: u32,
    offset_loads: u32,
    managed: bool,
}

fn route_without_identity_or_workload(route: &Rt64S2dexFastPathEvidence) -> RouteIdentity {
    RouteIdentity {
        load_operation_digest: route.load_operation_digest,
        source_address: route.source_address,
        source_width: route.source_width,
        source_height: route.source_height,
        source_size: route.source_size,
        gpu_operations: route.gpu_create_tile_copy_operation_count,
        gpu_tiles: route.gpu_tile_dispatch_count,
        cpu_uploads: route.cpu_rdram_tmem_upload_count,
        raw_tmem_tiles: route.raw_tmem_tile_count,
        sync_pairs: route.sync_framebuffer_pair_count,
        framebuffer_pairs: route.framebuffer_pair_count,
        valid_tiles: route.valid_tile_count,
        load_operations: route.load_operation_count,
        distinct_addresses: route.distinct_source_address_count,
        base_loads: route.base_source_load_count,
        offset_loads: route.offset_source_load_count,
        managed: route.source_is_managed_framebuffer,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other("S2DEX evidence requires clean pinned Metal RT64").into());
    }
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Original,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let disabled = RenderEnhancementSettings::default();
    let fast_enabled = RenderEnhancementSettings {
        s2dex_framebuffer_fast_path: true,
        ..disabled.clone()
    };
    let fix_enabled = RenderEnhancementSettings {
        s2dex_fix_bilerp_mismatch: true,
        ..disabled.clone()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let mut rdram = vec![0_u8; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for address in [
            ORDINARY_SOURCE - 2,
            ORDINARY_SOURCE + WIDTH * HEIGHT * 2,
            SOURCE - 2,
            SOURCE + WIDTH * HEIGHT * 2,
            TARGET - 2,
            TARGET + WIDTH * HEIGHT * 2,
        ] {
            view.write_u16(RdramAddr::from_offset(address), GUARD);
        }
    }
    require_production_rejection(&mut backend, &mut rdram)?;
    let source_end = write_source_commands(&mut rdram);
    write_ordinary_source(&mut rdram);
    if backend.process_rdp_commands(&mut rdram, SOURCE_COMMANDS as u32, source_end, SOURCE, true)?
        != FrameStatus::Complete
    {
        return Err(io::Error::other("S2DEX source workload did not complete").into());
    }

    let warm = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        WIDTH,
        HEIGHT,
        false,
        false,
    )?;
    let control = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        WIDTH,
        HEIGHT,
        false,
        false,
    )?;
    let enabled_policy = apply(&mut backend, &fast_enabled)?;
    let fast = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        WIDTH,
        HEIGHT,
        false,
        false,
    )?;
    let restored_policy = apply(&mut backend, &disabled)?;
    let restored = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        WIDTH,
        HEIGHT,
        false,
        false,
    )?;
    let negative_policy = apply(&mut backend, &fast_enabled)?;
    let ordinary = capture(
        &mut backend,
        &mut rdram,
        ORDINARY_SOURCE,
        WIDTH,
        HEIGHT,
        false,
        false,
    )?;

    apply(&mut backend, &disabled)?;
    let point_fix_disabled = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        false,
        false,
    )?;
    let fix_policy = apply(&mut backend, &fix_enabled)?;
    let point_fix_enabled = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        false,
        false,
    )?;
    apply(&mut backend, &disabled)?;
    let mismatch_off = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        true,
        false,
    )?;
    apply(&mut backend, &fix_enabled)?;
    let mismatch_on = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        true,
        false,
    )?;
    apply(&mut backend, &disabled)?;
    let mismatch_restored = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        true,
        false,
    )?;
    let matched_bilerp_off = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        true,
        true,
    )?;
    apply(&mut backend, &fix_enabled)?;
    let matched_bilerp_on = capture(
        &mut backend,
        &mut rdram,
        SOURCE,
        INSET_WIDTH,
        INSET_HEIGHT,
        true,
        true,
    )?;
    apply(&mut backend, &disabled)?;
    require_production_rejection(&mut backend, &mut rdram)?;

    let expected_control_route = RouteIdentity {
        load_operation_digest: CONTROL_LOAD_DIGEST,
        source_address: SOURCE,
        source_width: WIDTH,
        source_height: HEIGHT,
        source_size: 2,
        gpu_operations: 3,
        gpu_tiles: 3,
        cpu_uploads: 0,
        raw_tmem_tiles: 0,
        sync_pairs: 0,
        framebuffer_pairs: 1,
        valid_tiles: 3,
        load_operations: 3,
        distinct_addresses: 3,
        base_loads: 1,
        offset_loads: 2,
        managed: true,
    };
    let expected_fast_route = RouteIdentity {
        load_operation_digest: FAST_LOAD_DIGEST,
        source_address: SOURCE,
        source_width: WIDTH,
        source_height: HEIGHT,
        source_size: 2,
        gpu_operations: 1,
        gpu_tiles: 1,
        cpu_uploads: 0,
        raw_tmem_tiles: 0,
        sync_pairs: 0,
        framebuffer_pairs: 1,
        valid_tiles: 1,
        load_operations: 1,
        distinct_addresses: 1,
        base_loads: 1,
        offset_loads: 0,
        managed: true,
    };
    let expected_inset_point_route = RouteIdentity {
        load_operation_digest: INSET_POINT_LOAD_DIGEST,
        source_address: SOURCE,
        source_width: WIDTH,
        source_height: HEIGHT,
        source_size: 2,
        gpu_operations: 2,
        gpu_tiles: 2,
        cpu_uploads: 0,
        raw_tmem_tiles: 0,
        sync_pairs: 0,
        framebuffer_pairs: 1,
        valid_tiles: 2,
        load_operations: 2,
        distinct_addresses: 2,
        base_loads: 1,
        offset_loads: 1,
        managed: true,
    };
    let expected_bilerp_route = RouteIdentity {
        load_operation_digest: BILERP_LOAD_DIGEST,
        source_address: SOURCE,
        source_width: WIDTH,
        source_height: HEIGHT,
        source_size: 2,
        gpu_operations: 3,
        gpu_tiles: 3,
        cpu_uploads: 0,
        raw_tmem_tiles: 0,
        sync_pairs: 0,
        framebuffer_pairs: 1,
        valid_tiles: 3,
        load_operations: 3,
        distinct_addresses: 3,
        base_loads: 1,
        offset_loads: 2,
        managed: true,
    };
    let expected_ordinary_route = RouteIdentity {
        load_operation_digest: ORDINARY_LOAD_DIGEST,
        source_address: ORDINARY_SOURCE,
        source_width: 0,
        source_height: 0,
        source_size: 0,
        gpu_operations: 0,
        gpu_tiles: 0,
        cpu_uploads: 3,
        raw_tmem_tiles: 0,
        sync_pairs: 0,
        framebuffer_pairs: 1,
        valid_tiles: 3,
        load_operations: 3,
        distinct_addresses: 3,
        base_loads: 1,
        offset_loads: 2,
        managed: false,
    };
    let view = RdramView::from_storage(&rdram);
    let guards_valid = [
        ORDINARY_SOURCE - 2,
        ORDINARY_SOURCE + WIDTH * HEIGHT * 2,
        SOURCE - 2,
        SOURCE + WIDTH * HEIGHT * 2,
        TARGET - 2,
        TARGET + WIDTH * HEIGHT * 2,
    ]
    .into_iter()
    .all(|address| view.read_u16(RdramAddr::from_offset(address)) == GUARD);
    let ordinary_route = route_without_identity_or_workload(&ordinary.route);
    let managed_phases = [
        &control,
        &fast,
        &restored,
        &point_fix_disabled,
        &point_fix_enabled,
        &mismatch_off,
        &mismatch_on,
        &mismatch_restored,
        &matched_bilerp_off,
        &matched_bilerp_on,
    ];
    let managed_sources_valid = managed_phases.iter().all(|observation| {
        observation.route.source_framebuffer_identity == control.route.source_framebuffer_identity
            && observation.route.minimum_source_address >= SOURCE
            && observation.route.maximum_source_address < SOURCE + WIDTH * HEIGHT * 2
            && observation.route.base_source_load_count == 1
            && observation.route.sync_framebuffer_pair_count == 0
    });
    if warm.pixels != control.pixels
        || warm.target != control.target
        || control.pixels != fast.pixels
        || control.target != fast.target
        || control.pixels != restored.pixels
        || control.target != restored.target
        || control.pixels != ordinary.pixels
        || control.target != ordinary.target
        || hex(&control.pixels) != PIXELS
        || hex(&control.target) != TARGET_BYTES
        || hex(&control.policy) != CONTROL_POLICY
        || hex(&fast.policy) != FAST_POLICY
        || restored.policy != control.policy
        || enabled_policy != fast.policy
        || restored_policy != restored.policy
        || negative_policy != ordinary.policy
        || negative_policy != enabled_policy
        || hex(&fix_policy) != FIX_POLICY
        || point_fix_enabled.policy != fix_policy
        || mismatch_on.policy != fix_policy
        || matched_bilerp_on.policy != fix_policy
        || point_fix_disabled.policy != control.policy
        || mismatch_off.policy != control.policy
        || mismatch_restored.policy != control.policy
        || matched_bilerp_off.policy != control.policy
        || point_fix_disabled.pixels != control.pixels
        || point_fix_disabled.target != control.target
        || point_fix_enabled.pixels != point_fix_disabled.pixels
        || point_fix_enabled.target != point_fix_disabled.target
        || mismatch_off.pixels != point_fix_disabled.pixels
        || mismatch_on.pixels != mismatch_off.pixels
        || mismatch_restored.pixels != mismatch_off.pixels
        || mismatch_off.target != point_fix_disabled.target
        || mismatch_on.target != mismatch_off.target
        || mismatch_restored.target != mismatch_off.target
        || matched_bilerp_on.pixels != matched_bilerp_off.pixels
        || matched_bilerp_on.target != matched_bilerp_off.target
        || hex(&matched_bilerp_off.pixels) != MATCHED_BILERP_PIXELS
        || hex(&matched_bilerp_off.target) != MATCHED_BILERP_TARGET_BYTES
        || control.route.source_framebuffer_identity == 0
        || !managed_sources_valid
        || control.route.minimum_source_address != SOURCE
        || fast.route.minimum_source_address != SOURCE
        || fast.route.maximum_source_address != SOURCE
        || route_without_identity_or_workload(&control.route) != expected_control_route
        || route_without_identity_or_workload(&restored.route) != expected_control_route
        || route_without_identity_or_workload(&point_fix_disabled.route)
            != expected_inset_point_route
        || route_without_identity_or_workload(&point_fix_enabled.route)
            != expected_inset_point_route
        || route_without_identity_or_workload(&mismatch_on.route) != expected_inset_point_route
        || route_without_identity_or_workload(&mismatch_off.route) != expected_bilerp_route
        || route_without_identity_or_workload(&mismatch_restored.route) != expected_bilerp_route
        || route_without_identity_or_workload(&matched_bilerp_off.route) != expected_bilerp_route
        || route_without_identity_or_workload(&matched_bilerp_on.route) != expected_bilerp_route
        || route_without_identity_or_workload(&fast.route) != expected_fast_route
        || ordinary_route != expected_ordinary_route
        || ordinary.route.source_framebuffer_identity != 0
        || ordinary.route.minimum_source_address != ORDINARY_SOURCE
        || ordinary.route.maximum_source_address != ORDINARY_SOURCE + 0x1e00
        || !(warm.route.workload_id < control.route.workload_id
            && control.route.workload_id < fast.route.workload_id
            && fast.route.workload_id < restored.route.workload_id
            && restored.route.workload_id < ordinary.route.workload_id
            && ordinary.route.workload_id < point_fix_disabled.route.workload_id
            && point_fix_disabled.route.workload_id < point_fix_enabled.route.workload_id
            && point_fix_enabled.route.workload_id < mismatch_off.route.workload_id
            && mismatch_off.route.workload_id < mismatch_on.route.workload_id
            && mismatch_on.route.workload_id < mismatch_restored.route.workload_id
            && mismatch_restored.route.workload_id < matched_bilerp_off.route.workload_id
            && matched_bilerp_off.route.workload_id < matched_bilerp_on.route.workload_id)
        || !guards_valid
    {
        return Err(io::Error::other(format!(
            "S2DEX causal evidence mismatch: warm={warm:?} control={control:?} fast={fast:?} restored={restored:?} ordinary={ordinary:?} point_fix_disabled={point_fix_disabled:?} point_fix_enabled={point_fix_enabled:?} mismatch_off={mismatch_off:?} mismatch_on={mismatch_on:?} mismatch_restored={mismatch_restored:?} matched_bilerp_off={matched_bilerp_off:?} matched_bilerp_on={matched_bilerp_on:?} fix_policy={} managed_sources={managed_sources_valid} guards={guards_valid}",
            hex(&fix_policy),
        ))
        .into());
    }
    println!(
        "RT64 S2DEX enhancements pass: policy={}/{}/{} pixels={} target={} fast_routes=3/1/3 ordinary_cpu=3 bilerp_routes={}/{}/{}",
        hex(&control.policy),
        hex(&fast.policy),
        hex(&fix_policy),
        hex(&control.pixels),
        hex(&control.target),
        INSET_POINT_LOAD_DIGEST,
        BILERP_LOAD_DIGEST,
        INSET_POINT_LOAD_DIGEST,
    );
    Ok(())
}
