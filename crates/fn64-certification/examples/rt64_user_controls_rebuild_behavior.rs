use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RefreshRateTarget, RenderAntialiasing, RenderAspectRatio,
    RenderBackend, RenderConfig, RenderDisplayBuffering, RenderFiltering, RenderGraphicsApi,
    RenderHardwareResolve, RenderInternalColorFormat, RenderRefreshRate, RenderRestartField,
    RenderRuntimeSettings, RenderSettingsApply, RenderUpscale2d, ViFilterControl, ViPixelType,
    ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentSelection, Rt64PresentedPixels,
    Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TEXTURE: u32 = 0x1000;
const TARGET: u32 = 0x2000;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const TEXTURE_SIZE: u32 = 16;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const BASELINE_PIXEL_SHA256: &str =
    "6a845eadee1c6f62daded66c7812f6734f4ceafdb088af053d34c5cc28992180";
const LIVE_PIXEL_SHA256: &str = "25cb0d6e7a7c9e640d0b25e2d07c214af0b762b802dba33ae98060e5e2ede1c5";
const RECREATED_PIXEL_SHA256: &str =
    "4ea9265bc885aca9e29a5ee2dcd84af6c5723fcbc916154dc40634f1c360dc3f";
const BASELINE_POLICY_SHA256: &str =
    "a5bdcc9de677a51858991938ecaeb40aaa3fb8e6c7801d827bd0cacd65befc9c";
const LIVE_POLICY_SHA256: &str = "dda300e3b6b431e15975347ee30628441ef5559c19c9bf16e798c174e1db6122";
const RECREATED_POLICY_SHA256: &str =
    "545aa07cae6ef081709f1d32205ad652724d7bde3c44677bb2c4b6c17d148636";
const MANUAL_REFRESH_POLICY_SHA256: &str =
    "ec5f8cf857ccd1a9043b179b2895ab6f5e78a8cde771522ddff4f3722e8ee55c";
const HARDWARE_RESOLVE_POLICY_SHA256: &str =
    "73b9faa8816bdae848d30f1f9edc016dbe9487595f128526e8f3f62f37ed1ebb";
const IDLE_WORK_POLICY_SHA256: &str =
    "4236678e89ed815e0fe58c63bb68ed96d123fcce0b5c5f2ef1dc5e84fb761577";
const DEVELOPER_MODE_POLICY_SHA256: &str =
    "ac4a48da879d3acd5087e09a4e2066c275e78c04f13bf11f370f64c0ef7ff9d2";
const SOURCE_OVERLAY_ID: &str =
    "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1";

#[derive(Clone, Debug)]
struct Observation {
    pixels: Rt64PresentedPixels,
    selection: Rt64PresentSelection,
    sha256: String,
}

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fixture() -> (Vec<u8>, u32) {
    let commands = [
        (0xfc8f_ff1f, 0x88fc_f279),
        (0xff10_000f, TARGET),
        (0xfd10_000f, TEXTURE),
        (0xf510_0000, 7 << 24),
        (0xf300_0000, (7 << 24) | (255 << 12) | 0x200),
        (0xf510_0800, 0x0008_0200),
        (0xf200_0000, (60 << 12) | 60),
        (0xe400_0000 | (57 << 12) | 58, (5 << 12) | 6),
        (0, 0x04ec_04ec),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for y in 0..TEXTURE_SIZE {
            for x in 0..TEXTURE_SIZE {
                let color = if x > y { RED } else { BLUE };
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + (y * TEXTURE_SIZE + x) * 2),
                    color,
                );
            }
        }
    }
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    (rdram, (COMMANDS + commands.len() * 8) as u32)
}

fn baseline_settings() -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: fn64_render::RenderResolution::Manual,
        display_buffering: RenderDisplayBuffering::Double,
        antialiasing: RenderAntialiasing::None,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        upscale_2d: RenderUpscale2d::Original,
        three_point_filtering: false,
        internal_color_format: RenderInternalColorFormat::Standard,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn live_settings(baseline: &RenderRuntimeSettings) -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        antialiasing: RenderAntialiasing::Msaa4x,
        filtering: RenderFiltering::Linear,
        upscale_2d: RenderUpscale2d::All,
        three_point_filtering: true,
        ..baseline.clone()
    }
}

fn recreate_settings(live: &RenderRuntimeSettings) -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        display_buffering: RenderDisplayBuffering::Triple,
        internal_color_format: RenderInternalColorFormat::High,
        ..live.clone()
    }
}

fn remaining_live_settings(
    live: &RenderRuntimeSettings,
) -> [(&'static str, &'static str, RenderRuntimeSettings); 4] {
    [
        (
            "manual_refresh_72hz",
            MANUAL_REFRESH_POLICY_SHA256,
            RenderRuntimeSettings {
                refresh_rate: RenderRefreshRate::Manual,
                refresh_rate_target: RefreshRateTarget::new(72)
                    .expect("72 Hz is in the typed refresh-rate range"),
                ..live.clone()
            },
        ),
        (
            "hardware_resolve_disabled",
            HARDWARE_RESOLVE_POLICY_SHA256,
            RenderRuntimeSettings {
                hardware_resolve: RenderHardwareResolve::Disabled,
                ..live.clone()
            },
        ),
        (
            "idle_work_enabled",
            IDLE_WORK_POLICY_SHA256,
            RenderRuntimeSettings {
                idle_work_active: true,
                ..live.clone()
            },
        ),
        (
            "developer_mode_enabled",
            DEVELOPER_MODE_POLICY_SHA256,
            RenderRuntimeSettings {
                developer_mode: true,
                ..live.clone()
            },
        ),
    ]
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn active_policy_sha256(
    backend: &Rt64Backend,
    expected: &RenderRuntimeSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    let policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("RT64 has no complete active runtime policy"))?;
    if policy.user != *expected || backend.active_settings() != Some(expected) {
        return Err(io::Error::other(format!(
            "active user settings mismatch: expected={expected:?}, active={:?}",
            backend.active_settings()
        ))
        .into());
    }
    Ok(policy.sha256())
}

fn render(backend: &mut Rt64Backend, guest_cycle: u64) -> Result<Observation, Box<dyn Error>> {
    let (mut rdram, end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET, true)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "user-control fixture returned {status:?} instead of Complete"
        ))
        .into());
    }
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: guest_cycle,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    let pixels = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    if pixels.width != WIDTH
        || pixels.height != HEIGHT
        || pixels.row_bytes != WIDTH * 4
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
        || pixels.bytes.len() != (WIDTH * HEIGHT * 4) as usize
    {
        return Err(io::Error::other(format!("invalid post-VI layout: {pixels:?}")).into());
    }
    if selection.present_id != pixels.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != TARGET
        || selection.target_width != WIDTH
        || selection.target_height != 15
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "present/source-resource identity mismatch: pixels={pixels:?}, selection={selection:?}"
        ))
        .into());
    }
    let sha256 = digest(&pixels.bytes);
    Ok(Observation {
        pixels,
        selection,
        sha256,
    })
}

fn expect_digest(label: &str, actual: &str, expected: &str) -> Result<(), Box<dyn Error>> {
    if actual != expected {
        return Err(io::Error::other(format!(
            "{label} exact pixel digest drifted: expected={expected}, actual={actual}"
        ))
        .into());
    }
    Ok(())
}

fn expect_policy(label: &str, actual: &[u8; 32], expected: &str) -> Result<(), Box<dyn Error>> {
    let actual = hex(actual);
    if actual != expected {
        return Err(io::Error::other(format!(
            "{label} active-policy digest drifted: expected={expected}, actual={actual}"
        ))
        .into());
    }
    Ok(())
}

fn print_observation(label: &str, policy: &[u8; 32], observation: &Observation) {
    println!(
        "phase={label} classification=active policy_sha256={} present_id={} resource_id={} target={:#010x}/{}x{}/{} sha256={}",
        hex(policy),
        observation.pixels.present_id,
        observation.selection.source_texture_identity,
        observation.selection.target_address,
        observation.selection.target_width,
        observation.selection.target_height,
        observation.selection.target_size,
        observation.sha256,
    );
}

fn apply_live_without_resource_churn(
    backend: &mut Rt64Backend,
    label: &str,
    settings: &RenderRuntimeSettings,
    expected_policy: &str,
    guest_cycle: u64,
    previous: &Observation,
) -> Result<Observation, Box<dyn Error>> {
    let result = backend.apply_runtime_settings(settings)?;
    let expected = RenderSettingsApply::LiveApplied {
        settings_sha256: settings.sha256(),
        framebuffers_discarded: false,
    };
    if result != expected || backend.configured_settings() != settings {
        return Err(io::Error::other(format!(
            "{label} classification mismatch: expected={expected:?}, actual={result:?}, configured={:?}",
            backend.configured_settings()
        ))
        .into());
    }
    let policy = active_policy_sha256(backend, settings)?;
    expect_policy(label, &policy, expected_policy)?;
    let observation = render(backend, guest_cycle)?;
    print_observation(label, &policy, &observation);
    expect_digest(label, &observation.sha256, LIVE_PIXEL_SHA256)?;
    if observation.pixels.present_id <= previous.pixels.present_id
        || observation.selection.source_texture_identity
            != previous.selection.source_texture_identity
    {
        return Err(io::Error::other(format!(
            "{label} changed the live source resource or failed to advance presentation: previous={:?}, current={:?}",
            previous.selection, observation.selection
        ))
        .into());
    }
    Ok(observation)
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
        || source.source_overlay_id != SOURCE_OVERLAY_ID
    {
        return Err(io::Error::other(format!(
            "user-control/rebuild evidence requires the clean pinned RT64 source: {source:?}"
        ))
        .into());
    }

    let baseline = baseline_settings()?;
    let live = live_settings(&baseline);
    let recreated = recreate_settings(&live);
    let mut backend = Rt64Backend::new();

    let staged = backend.apply_runtime_settings(&baseline)?;
    let expected_staged = RenderSettingsApply::StagedForCreate {
        settings_sha256: baseline.sha256(),
    };
    if staged != expected_staged {
        return Err(io::Error::other(format!(
            "baseline classification mismatch: expected={expected_staged:?}, actual={staged:?}"
        ))
        .into());
    }
    println!(
        "phase=baseline_stage classification={staged:?} settings_sha256={}",
        hex(&baseline.sha256())
    );

    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let baseline_policy = active_policy_sha256(&backend, &baseline)?;
    let baseline_observation = render(&mut backend, 1)?;
    print_observation("baseline", &baseline_policy, &baseline_observation);
    expect_policy("baseline", &baseline_policy, BASELINE_POLICY_SHA256)?;
    expect_digest(
        "baseline",
        &baseline_observation.sha256,
        BASELINE_PIXEL_SHA256,
    )?;

    let live_result = backend.apply_runtime_settings(&live)?;
    let expected_live = RenderSettingsApply::LiveApplied {
        settings_sha256: live.sha256(),
        framebuffers_discarded: true,
    };
    if live_result != expected_live || backend.configured_settings() != &live {
        return Err(io::Error::other(format!(
            "live rebuild classification mismatch: expected={expected_live:?}, actual={live_result:?}"
        ))
        .into());
    }
    println!(
        "phase=live_apply classification={live_result:?} settings_sha256={}",
        hex(&live.sha256())
    );
    let live_policy = active_policy_sha256(&backend, &live)?;
    let live_observation = render(&mut backend, 2)?;
    print_observation("live", &live_policy, &live_observation);
    expect_policy("live", &live_policy, LIVE_POLICY_SHA256)?;
    expect_digest("live", &live_observation.sha256, LIVE_PIXEL_SHA256)?;
    if live_observation.pixels.present_id <= baseline_observation.pixels.present_id
        || live_observation.selection.source_texture_identity
            == baseline_observation.selection.source_texture_identity
    {
        return Err(io::Error::other(format!(
            "live rebuild did not advance the present ID and replace its discarded source resource: baseline={:?}, live={:?}",
            baseline_observation.selection, live_observation.selection
        ))
        .into());
    }

    let mut previous = live_observation.clone();
    for (index, (label, expected_policy, settings)) in
        remaining_live_settings(&live).into_iter().enumerate()
    {
        previous = apply_live_without_resource_churn(
            &mut backend,
            label,
            &settings,
            expected_policy,
            10 + index as u64 * 2,
            &previous,
        )?;
        previous = apply_live_without_resource_churn(
            &mut backend,
            "remaining_control_restore",
            &live,
            LIVE_POLICY_SHA256,
            11 + index as u64 * 2,
            &previous,
        )?;
    }

    let restart_result = backend.apply_runtime_settings(&recreated)?;
    let expected_restart = RenderSettingsApply::RestartRequired {
        fields: vec![
            RenderRestartField::DisplayBuffering,
            RenderRestartField::InternalColorFormat,
        ],
        active_settings_sha256: live.sha256(),
        requested_settings_sha256: recreated.sha256(),
    };
    if restart_result != expected_restart
        || backend.configured_settings() != &recreated
        || backend.active_settings() != Some(&live)
    {
        return Err(io::Error::other(format!(
            "setup-owned classification mismatch: expected={expected_restart:?}, actual={restart_result:?}, configured={:?}, active={:?}",
            backend.configured_settings(),
            backend.active_settings()
        ))
        .into());
    }
    println!(
        "phase=recreate_required classification={restart_result:?} active_settings_sha256={} requested_settings_sha256={}",
        hex(&live.sha256()),
        hex(&recreated.sha256())
    );
    let pending_policy = active_policy_sha256(&backend, &live)?;
    let pending_observation = render(&mut backend, 3)?;
    print_observation("pending_recreate", &pending_policy, &pending_observation);
    expect_policy("pending recreate", &pending_policy, LIVE_POLICY_SHA256)?;
    expect_digest(
        "pending recreate",
        &pending_observation.sha256,
        LIVE_PIXEL_SHA256,
    )?;
    if pending_observation.pixels.present_id <= live_observation.pixels.present_id
        || pending_observation.selection.source_texture_identity
            != live_observation.selection.source_texture_identity
    {
        return Err(io::Error::other(format!(
            "restart-required settings disturbed the active resource: live={:?}, pending={:?}",
            live_observation.selection, pending_observation.selection
        ))
        .into());
    }

    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    if backend.configured_settings() != &recreated || backend.active_settings() != Some(&recreated)
    {
        return Err(io::Error::other(format!(
            "recreate did not activate setup-owned settings: configured={:?}, active={:?}",
            backend.configured_settings(),
            backend.active_settings()
        ))
        .into());
    }
    let recreated_policy = active_policy_sha256(&backend, &recreated)?;
    let recreated_observation = render(&mut backend, 4)?;
    print_observation("recreated", &recreated_policy, &recreated_observation);
    expect_policy("recreated", &recreated_policy, RECREATED_POLICY_SHA256)?;
    expect_digest(
        "recreated",
        &recreated_observation.sha256,
        RECREATED_PIXEL_SHA256,
    )?;
    if recreated_observation.pixels.present_id != 1 {
        return Err(io::Error::other(format!(
            "fresh RT64 context did not restart its present ID: {:?}",
            recreated_observation.selection
        ))
        .into());
    }

    println!(
        "rt64 user controls/rebuild behavior: pass baseline={{policy:{},present:{},resource:{},pixels:{}}} live={{classification:LiveApplied,policy:{},present:{},resource:{},pixels:{}}} pending={{classification:RestartRequired,policy:{},present:{},resource:{},pixels:{}}} recreated={{policy:{},present:{},resource:{},pixels:{}}}",
        hex(&baseline_policy),
        baseline_observation.pixels.present_id,
        baseline_observation.selection.source_texture_identity,
        baseline_observation.sha256,
        hex(&live_policy),
        live_observation.pixels.present_id,
        live_observation.selection.source_texture_identity,
        live_observation.sha256,
        hex(&pending_policy),
        pending_observation.pixels.present_id,
        pending_observation.selection.source_texture_identity,
        pending_observation.sha256,
        hex(&recreated_policy),
        recreated_observation.pixels.present_id,
        recreated_observation.selection.source_texture_identity,
        recreated_observation.sha256,
    );
    fn64_boot_harness::emit_rt64_platform_child_identity(
        source.source_id,
        source.is_source_authoritative(),
        source.adapter_source_sha256,
        source.post_vi_api,
    )?;
    Ok(())
}
