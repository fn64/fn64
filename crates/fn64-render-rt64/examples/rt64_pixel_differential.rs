use std::error::Error;
use std::io;

use fn64_boot_harness::{LiveRenderEvidence, RenderPixelFormat};
use fn64_render::{
    AspectTarget, FrameStatus, OsTask, ReleaseCaptureFormat, RenderAspectRatio, RenderBackend,
    RenderConfig, RenderEmulatorSettings, RenderEnhancementSettings, RenderFiltering,
    RenderGraphicsApi, RenderPolicyApply, RenderPresentationMode, RenderReleaseCapture,
    RenderResolution, RenderRestartField, RenderRuntimePolicy, RenderRuntimeSettings,
    RenderSettingsApply, ResolutionMultiplier, ViFilterControl, ViPixelType, ViPresentation,
    M_GFXTASK,
};
use fn64_render_rt64::{
    capture_rt64_adapter_inputs, roundtrip_rt64_emulator_settings,
    roundtrip_rt64_enhancement_settings, roundtrip_rt64_runtime_settings, ReferenceBackend,
    Rt64Backend, Rt64ReplacementPackInput,
};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: usize = 0x400;
const WIDTH: usize = 4;
const HEIGHT: usize = 2;
const PRESENT_GUEST_CYCLE: u64 = 0x0123_4567_89ab_cdef;

struct SyntheticReplacementPack(std::path::PathBuf);

impl SyntheticReplacementPack {
    fn new() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!("fn64-rt64-live-pack-{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        std::fs::create_dir(&path)?;
        let pack = Self(path);
        pack.write_database("rt64", "stream")?;
        Ok(pack)
    }

    fn write_database(&self, auto_path: &str, operation: &str) -> io::Result<()> {
        std::fs::write(
            self.0.join("rt64.json"),
            format!(
                "{{\"configuration\":{{\"configurationVersion\":3,\"autoPath\":\"{auto_path}\",\"defaultOperation\":\"{operation}\",\"defaultShift\":\"half\",\"hashVersion\":5}},\"textures\":[],\"operationFilters\":[],\"shiftFilters\":[],\"extraFiles\":[]}}"
            ),
        )
    }
}

impl Drop for SyntheticReplacementPack {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> Vec<u8> {
    let commands: [(u32, u32); 5] = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0003, TARGET as u32),
        (0xf700_0000, 0xf801_f801),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0; RDRAM_LEN];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    rdram
}

fn presentation() -> ViPresentation {
    ViPresentation {
        noise_seed: PRESENT_GUEST_CYCLE,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn submit(backend: &mut impl RenderBackend) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut rdram = fixture();
    let end = COMMANDS + 5 * 8;
    let status =
        backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end as u32, TARGET as u32)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "raw RDP fixture returned {status:?} instead of Complete"
        ))
        .into());
    }
    backend.present(presentation())?;
    Ok(rdram[TARGET..TARGET + WIDTH * HEIGHT * 2].to_vec())
}

fn render_reference() -> Result<Vec<u8>, Box<dyn Error>> {
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::new(WIDTH as u32, HEIGHT as u32))?;
    submit(&mut backend)
}

fn render_rt64(
    stage_replacements_before_create: bool,
) -> Result<(Vec<u8>, RenderReleaseCapture), Box<dyn Error>> {
    let replacement_pack = SyntheticReplacementPack::new()?;
    let mut backend = Rt64Backend::new();
    let replacement_inputs = [Rt64ReplacementPackInput::new(&replacement_pack.0)];
    if stage_replacements_before_create {
        let staged = backend.load_replacement_packs(&replacement_inputs, true)?;
        if staged
            != (RenderPolicyApply::StagedForCreate {
                policy_sha256: backend.configured_runtime_policy().sha256(),
            })
        {
            return Err(io::Error::other(format!(
                "unexpected staged replacement outcome: {staged:?}"
            ))
            .into());
        }
    }
    backend.create(&RenderConfig::new(WIDTH as u32, HEIGHT as u32))?;
    if stage_replacements_before_create {
        if backend.active_replacement_settings() != Some(&backend.configured_replacement_settings())
        {
            return Err(io::Error::other(
                "create did not activate the staged replacement identity",
            )
            .into());
        }
    } else {
        if backend.active_replacement_settings() != Some(&Default::default()) {
            return Err(io::Error::other(
                "empty/default create did not establish an active replacement policy",
            )
            .into());
        }
        let loaded = backend.load_replacement_packs(&replacement_inputs, true)?;
        let expected = backend
            .active_runtime_policy()
            .ok_or_else(|| io::Error::other("live replacement load erased active policy"))?;
        if loaded
            != (RenderPolicyApply::LiveApplied {
                policy_sha256: expected.sha256(),
            })
        {
            return Err(io::Error::other(format!(
                "unexpected live replacement load outcome: {loaded:?}"
            ))
            .into());
        }
    }
    for enabled in [false, true] {
        let applied = backend.set_replacements_enabled(enabled)?;
        let expected = backend
            .active_runtime_policy()
            .ok_or_else(|| io::Error::other("replacement enable erased active runtime policy"))?;
        if applied
            != (RenderPolicyApply::LiveApplied {
                policy_sha256: expected.sha256(),
            })
        {
            return Err(io::Error::other(format!(
                "unexpected replacement enable outcome: {applied:?}"
            ))
            .into());
        }
    }
    let before_reload = backend
        .active_replacement_settings()
        .expect("create installed replacement policy")
        .sha256();
    replacement_pack.write_database("rice", "stall")?;
    let reloaded = backend.reload_replacement_packs()?;
    let expected = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("replacement reload erased active runtime policy"))?;
    if reloaded
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: expected.sha256(),
        })
        || backend
            .active_replacement_settings()
            .expect("reload installed replacement policy")
            .sha256()
            == before_reload
    {
        return Err(io::Error::other(format!(
            "unexpected replacement reload outcome: {reloaded:?}"
        ))
        .into());
    }
    let live_settings = RenderRuntimeSettings {
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(1.0)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let applied = backend.apply_runtime_settings(&live_settings)?;
    if applied
        != (RenderSettingsApply::LiveApplied {
            settings_sha256: live_settings.sha256(),
            framebuffers_discarded: true,
        })
    {
        return Err(io::Error::other(format!(
            "unexpected live RT64 settings outcome: {applied:?}"
        ))
        .into());
    }
    let enhancement_settings = RenderEnhancementSettings {
        presentation_mode: RenderPresentationMode::PresentEarly,
        framebuffer_reinterpret_fix_uls: true,
        remove_black_borders: true,
        rect_fix_lower_right: true,
        f3dex_force_branch: true,
        s2dex_fix_bilerp_mismatch: true,
        s2dex_framebuffer_fast_path: true,
        texture_lod_scale: true,
    };
    let active_policy = RenderRuntimePolicy {
        enhancement: enhancement_settings.clone(),
        ..backend
            .active_runtime_policy()
            .ok_or_else(|| io::Error::other("missing active replacement-aware policy"))?
    };
    let applied = backend.apply_enhancement_settings(&enhancement_settings)?;
    if applied
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: active_policy.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "unexpected live RT64 enhancement outcome: {applied:?}"
        ))
        .into());
    }
    // PresentEarly is a valid live mutation, but this raw-DPC fixture has no
    // deferred game framebuffer from which to form an early-present candidate.
    // Restore the other enhanced latency policy before asking for pixels.
    let enhancement_settings = RenderEnhancementSettings {
        presentation_mode: RenderPresentationMode::SkipBuffering,
        ..enhancement_settings
    };
    let active_policy = RenderRuntimePolicy {
        enhancement: enhancement_settings.clone(),
        ..active_policy
    };
    let applied = backend.apply_enhancement_settings(&enhancement_settings)?;
    if applied
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: active_policy.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "unexpected second live RT64 latency outcome: {applied:?}"
        ))
        .into());
    }
    let emulator_settings = RenderEmulatorSettings {
        post_blend_noise: false,
        post_blend_noise_negative: true,
        framebuffer_render_to_ram: false,
        framebuffer_copy_with_gpu: false,
    };
    let active_policy = RenderRuntimePolicy {
        emulator: emulator_settings.clone(),
        ..active_policy
    };
    let applied = backend.apply_emulator_settings(&emulator_settings)?;
    if applied
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: active_policy.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "unexpected live RT64 emulator outcome: {applied:?}"
        ))
        .into());
    }
    // Exercise render-to-RAM as a live field, then restore fn64's integration
    // requirement before the workload whose RDRAM bytes are compared.
    let emulator_settings = RenderEmulatorSettings {
        framebuffer_render_to_ram: true,
        ..emulator_settings
    };
    let active_policy = RenderRuntimePolicy {
        emulator: emulator_settings.clone(),
        ..active_policy
    };
    let applied = backend.apply_emulator_settings(&emulator_settings)?;
    if applied
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: active_policy.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "unexpected restored render-to-RAM outcome: {applied:?}"
        ))
        .into());
    }
    backend.enable_present_capture()?;
    let render_to_ram = submit(&mut backend)?;
    let restart_settings = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        ..live_settings.clone()
    };
    let restart = backend.apply_runtime_settings(&restart_settings)?;
    if restart
        != (RenderSettingsApply::RestartRequired {
            fields: vec![RenderRestartField::GraphicsApi],
            active_settings_sha256: live_settings.sha256(),
            requested_settings_sha256: restart_settings.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "unexpected restart-required RT64 settings outcome: {restart:?}"
        ))
        .into());
    }
    let presented = backend.release_capture()?;
    if presented.settings_sha256 != active_policy.sha256() {
        return Err(io::Error::other(
            "release capture did not bind the composite active RT64 policy",
        )
        .into());
    }
    replacement_pack.write_database("rt64", "preload")?;
    let drift_error = backend.release_capture().unwrap_err();
    if !drift_error
        .to_string()
        .contains("replacement-pack bytes changed after activation")
    {
        return Err(io::Error::other(format!(
            "mutable replacement pack did not invalidate release evidence: {drift_error}"
        ))
        .into());
    }
    Ok((render_to_ram, presented))
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let settings = RenderRuntimeSettings::default();
    if roundtrip_rt64_runtime_settings(&settings)? != settings {
        return Err(io::Error::other("RT64 settings ABI roundtrip changed the typed image").into());
    }
    let enhancement = RenderEnhancementSettings::default();
    if roundtrip_rt64_enhancement_settings(&enhancement)? != enhancement {
        return Err(
            io::Error::other("RT64 enhancement ABI roundtrip changed the typed image").into(),
        );
    }
    let emulator = RenderEmulatorSettings::default();
    if roundtrip_rt64_emulator_settings(&emulator)? != emulator {
        return Err(io::Error::other("RT64 emulator ABI roundtrip changed the typed image").into());
    }
    let cfg = RenderConfig::new(WIDTH as u32, HEIGHT as u32);
    let task = OsTask {
        task_type: M_GFXTASK,
        data_ptr: COMMANDS as u32,
        data_size: 5 * 8,
        ..OsTask::default()
    };
    let capture = capture_rt64_adapter_inputs(&task, TARGET as u32, cfg, presentation())?;

    let reference = render_reference()?;
    let (rt64_first, presented_first) = render_rt64(false)?;
    let (rt64_second, presented_second) = render_rt64(true)?;
    if rt64_first != rt64_second {
        return Err(io::Error::other(format!(
            "RT64 render-to-RAM was nondeterministic: first={}, second={}",
            hex(&digest(&rt64_first)),
            hex(&digest(&rt64_second))
        ))
        .into());
    }
    if reference != rt64_first {
        return Err(io::Error::other(format!(
            "reference/RT64 pixel differential: reference={}, rt64={}",
            hex(&digest(&reference)),
            hex(&digest(&rt64_first))
        ))
        .into());
    }
    if reference
        .chunks_exact(2)
        .any(|pixel| u16::from_ne_bytes([pixel[0], pixel[1]]) != 0xf801)
    {
        return Err(io::Error::other("synthetic fill fixture did not produce opaque red").into());
    }
    if presented_first != presented_second {
        return Err(io::Error::other(format!(
            "RT64 post-VI capture was nondeterministic: first={}x{} {:?} id={} sha={}, second={}x{} {:?} id={} sha={}",
            presented_first.width,
            presented_first.height,
            presented_first.format,
            presented_first.present_id,
            hex(&digest(&presented_first.bytes)),
            presented_second.width,
            presented_second.height,
            presented_second.format,
            presented_second.present_id,
            hex(&digest(&presented_second.bytes))
        ))
        .into());
    }
    if (presented_first.width, presented_first.height) != (WIDTH as u32, HEIGHT as u32)
        || (presented_first.format != ReleaseCaptureFormat::PostViBgra8Unorm)
        || (presented_first.row_bytes != presented_first.width * 4)
        || (presented_first.bytes.len()
            != presented_first.row_bytes as usize * presented_first.height as usize)
    {
        return Err(io::Error::other(format!(
            "invalid post-VI capture metadata: {presented_first:?}"
        ))
        .into());
    }
    if presented_first.guest_cycle != PRESENT_GUEST_CYCLE {
        return Err(io::Error::other(format!(
            "release capture belongs to guest cycle {}, expected {PRESENT_GUEST_CYCLE}",
            presented_first.guest_cycle
        ))
        .into());
    }
    if !presented_first
        .bytes
        .chunks_exact(4)
        .any(|pixel| pixel[..3] != [0, 0, 0])
    {
        return Err(io::Error::other(
            "post-VI capture is uniformly black despite the visible red RDP fixture",
        )
        .into());
    }
    let render_evidence = LiveRenderEvidence::post_vi_swapchain(
        presented_first.guest_cycle,
        presented_first.backend_identity.clone(),
        presented_first.settings_sha256,
        presented_first.width,
        presented_first.height,
        presented_first.row_bytes,
        RenderPixelFormat::Bgra8Unorm,
        presented_first.workload_id.get(),
        presented_first.present_id,
        presented_first.bytes.clone(),
    )?;

    println!(
        "rt64_pixel_evidence adapter_capture_sha256={} framebuffer_sha256={} framebuffer_bytes={} post_vi_sha256={} post_vi={}x{} format={:?} workload_id={} present_id={} guest_cycle={} post_vi_bytes={} active_policy_sha256={} live_render_sha256={} backend_identity={} source_authoritative={}",
        hex(&capture.sha256()),
        hex(&digest(&rt64_first)),
        rt64_first.len(),
        hex(&digest(&presented_first.bytes)),
        presented_first.width,
        presented_first.height,
        presented_first.format,
        presented_first.workload_id,
        presented_first.present_id,
        presented_first.guest_cycle,
        presented_first.bytes.len(),
        hex(&presented_first.settings_sha256),
        hex(&render_evidence.sha256()),
        render_evidence.backend_identity(),
        presented_first.source_authoritative
    );
    Ok(())
}
