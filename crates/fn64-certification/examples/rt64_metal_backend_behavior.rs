use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, ReleaseCaptureFormat, RenderBackend, RenderConfig, RenderGraphicsApi,
    RenderReleaseCapture, RenderRuntimeSettings, RenderSettingsApply, ViFilterControl, ViPixelType,
    ViPresentation, ViScaleAxis, ViScanoutRegisters, ViScanoutState,
};
use fn64_render_rt64::{Rt64Backend, Rt64BackendIdentity, Rt64SourceProvenance};
use fn64_runtime::TvType;
use sha2::{Digest, Sha256};

#[path = "rt64_vi_aa_selector_behavior.rs"]
mod vi_aa_selector_behavior;
#[path = "rt64_vi_filter_behavior.rs"]
mod vi_filter_behavior;

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const COMMAND_COUNT: usize = 5;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";

#[derive(Copy, Clone, Debug)]
struct FixtureSpec {
    width: u32,
    height: u32,
    target: u32,
    rgba16: u16,
    guest_cycle: u64,
}

fn fixture(spec: FixtureSpec) -> Vec<u8> {
    assert!(spec.width > 0 && spec.width <= 1024);
    assert!(spec.height > 0 && spec.height <= 1024);
    let lower_right = (((spec.width - 1) * 4) << 12) | ((spec.height - 1) * 4);
    let fill = u32::from(spec.rgba16) << 16 | u32::from(spec.rgba16);
    let commands = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0000 | (spec.width - 1), spec.target),
        (0xf700_0000, fill),
        (0xf600_0000 | lower_right, 0),
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

fn presentation(spec: FixtureSpec) -> ViPresentation {
    let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
    words[0] = 0x302;
    words[1] = spec.target;
    words[2] = spec.width;
    words[9] = (100 << 16) | (100 + spec.width);
    words[10] = (20 << 16) | (20 + spec.height * 2);
    words[12] = u32::from(ViScaleAxis::ONE);
    words[13] = u32::from(ViScaleAxis::ONE);
    ViPresentation {
        noise_seed: spec.guest_cycle,
        scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
        ..ViPresentation::default()
    }
}

fn submit_raw(
    backend: &mut Rt64Backend,
    spec: FixtureSpec,
) -> Result<(Vec<u8>, RenderReleaseCapture), Box<dyn Error>> {
    let mut rdram = fixture(spec);
    let end = COMMANDS + COMMAND_COUNT * 8;
    let status = backend.process_rdp_commands(
        &mut rdram,
        COMMANDS as u32,
        end as u32,
        spec.target,
        true,
    )?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("Metal raw-RDP submission returned {status:?}")).into(),
        );
    }
    backend.present_live(&rdram, presentation(spec))?;
    let capture = backend.release_capture()?;
    let byte_len = (spec.width * spec.height * 2) as usize;
    let start = spec.target as usize;
    Ok((rdram[start..start + byte_len].to_vec(), capture))
}

fn validate_native(spec: FixtureSpec, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let expected_len = (spec.width * spec.height * 2) as usize;
    if bytes.len() != expected_len
        || bytes
            .chunks_exact(2)
            .any(|pixel| u16::from_ne_bytes([pixel[0], pixel[1]]) != spec.rgba16)
    {
        return Err(io::Error::other(format!(
            "raw-RDP fill did not write {}x{} RGBA16 {:#06x}",
            spec.width, spec.height, spec.rgba16
        ))
        .into());
    }
    Ok(())
}

fn validate_capture(
    spec: FixtureSpec,
    capture: &RenderReleaseCapture,
    identity: &Rt64BackendIdentity,
    settings_sha256: [u8; 32],
) -> Result<(), Box<dyn Error>> {
    let expected_len = (spec.width * spec.height * 4) as usize;
    if capture.guest_cycle != spec.guest_cycle
        || capture.backend_identity != identity.canonical_id()
        || !capture.source_authoritative
        || capture.settings_sha256 != settings_sha256
        || capture.width != spec.width
        || capture.height != spec.height
        || capture.row_bytes != spec.width * 4
        || capture.format != ReleaseCaptureFormat::PostViBgra8Unorm
        || capture.present_id == 0
        || capture.bytes.len() != expected_len
        || !capture
            .bytes
            .chunks_exact(4)
            .any(|pixel| pixel[..3] != [0, 0, 0])
    {
        return Err(io::Error::other(format!(
            "invalid Metal post-VI capture for {spec:?}: {capture:?}"
        ))
        .into());
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "Metal behavior evidence requires clean pinned RT64: {identity:?}"
        ))
        .into());
    }

    let settings = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new();
    if backend.release_environment().tv_type().is_some() {
        return Err(io::Error::other("uncreated RT64 backend claimed TV authority").into());
    }
    let staged = backend.apply_runtime_settings(&settings)?;
    if staged
        != (RenderSettingsApply::StagedForCreate {
            settings_sha256: settings.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "Metal settings were not staged for initialization: {staged:?}"
        ))
        .into());
    }

    let initial = FixtureSpec {
        width: 4,
        height: 2,
        target: 0x400,
        rgba16: 0xf801,
        guest_cycle: 101,
    };
    backend.create(&RenderConfig::for_tv(
        initial.width,
        initial.height,
        TvType::Pal,
    ))?;
    if backend.active_settings() != Some(&settings) {
        return Err(
            io::Error::other("Metal initialization did not activate staged settings").into(),
        );
    }
    backend.enable_present_capture()?;
    let policy_sha256 = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("Metal initialization has no active runtime policy"))?
        .sha256();

    // Real games can run their VI thread before their graphics thread has
    // published a first workload. Exercise two such presents so the
    // present-worker/readback interleaving remains successful while the
    // zero-workload pixels stay unavailable as release evidence.
    let pre_workload_rdram = fixture(initial);
    for guest_cycle in [91, 92] {
        backend.present_live(
            &pre_workload_rdram,
            presentation(FixtureSpec {
                guest_cycle,
                ..initial
            }),
        )?;
        match backend.release_capture() {
            Err(fn64_render::RenderError::NotReady(
                "RT64 has no completed post-workload present capture",
            )) => {}
            result => {
                return Err(io::Error::other(format!(
                    "pre-workload VI pixels became release capture evidence: {result:?}"
                ))
                .into())
            }
        }
        if backend.release_environment().tv_type().is_some() {
            return Err(io::Error::other(
                "pre-workload VI present claimed release-environment authority",
            )
            .into());
        }
    }
    let (initial_native, initial_capture) = submit_raw(&mut backend, initial)?;
    validate_native(initial, &initial_native)?;
    validate_capture(initial, &initial_capture, &identity, policy_sha256)?;
    if initial_capture.workload_id.get() != 1 || initial_capture.present_id != 3 {
        return Err(io::Error::other(format!(
            "first post-workload capture did not follow the two unpublished VI presents: {}/{}",
            initial_capture.workload_id, initial_capture.present_id
        ))
        .into());
    }
    if backend.release_environment().tv_type() != Some(TvType::Pal) {
        return Err(io::Error::other("Metal initialization lost PAL TV authority").into());
    }

    let resize_transition = FixtureSpec {
        width: 8,
        height: 4,
        target: 0x800,
        rgba16: 0x07c1,
        guest_cycle: 202,
    };
    backend.resize(resize_transition.width, resize_transition.height);
    let (transition_native, transition_capture) = submit_raw(&mut backend, resize_transition)?;
    validate_native(resize_transition, &transition_native)?;
    validate_capture(
        resize_transition,
        &transition_capture,
        &identity,
        policy_sha256,
    )?;
    if backend.release_environment().tv_type() != Some(TvType::Pal) {
        return Err(io::Error::other("Metal resize changed PAL TV authority").into());
    }

    // A second distinct frame proves the new drawable geometry survives the
    // next present-worker pass instead of reverting to its old Cocoa cache.
    let resized = FixtureSpec {
        width: 8,
        height: 4,
        target: 0xa00,
        rgba16: 0xffc1,
        guest_cycle: 203,
    };
    let (resized_native, resized_capture) = submit_raw(&mut backend, resized)?;
    validate_native(resized, &resized_native)?;
    validate_capture(resized, &resized_capture, &identity, policy_sha256)?;
    if !(initial_capture.present_id < transition_capture.present_id
        && transition_capture.present_id < resized_capture.present_id
        && initial_capture.workload_id < transition_capture.workload_id
        && transition_capture.workload_id < resized_capture.workload_id)
    {
        return Err(io::Error::other(
            "distinct Metal submissions did not advance both workload and presentation identity",
        )
        .into());
    }

    let recreated = FixtureSpec {
        width: 4,
        height: 2,
        target: 0xc00,
        rgba16: 0x003f,
        guest_cycle: 303,
    };
    backend.create(&RenderConfig::for_tv(
        recreated.width,
        recreated.height,
        TvType::Mpal,
    ))?;
    if backend.active_settings() != Some(&settings) {
        return Err(
            io::Error::other("Metal recreation did not preserve configured settings").into(),
        );
    }
    backend.enable_present_capture()?;
    let recreated_policy_sha256 = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("Metal recreation has no active runtime policy"))?
        .sha256();
    let (recreated_native, recreated_capture) = submit_raw(&mut backend, recreated)?;
    validate_native(recreated, &recreated_native)?;
    validate_capture(
        recreated,
        &recreated_capture,
        &identity,
        recreated_policy_sha256,
    )?;
    if backend.release_environment().tv_type() != Some(TvType::Mpal) {
        return Err(io::Error::other("Metal recreation lost MPAL TV authority").into());
    }
    if recreated_policy_sha256 != policy_sha256 {
        return Err(io::Error::other("Metal recreation changed the active policy identity").into());
    }

    let compatibility_rdram = fixture(recreated);
    backend.present_physical_compatibility(
        &compatibility_rdram,
        ViPresentation {
            noise_seed: 404,
            scanout: ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    let compatibility_pixels = backend.presented_pixels()?;
    if recreated_capture.workload_id.get() != 1
        || recreated_capture.present_id != 1
        || compatibility_pixels.workload_id != 1
        || compatibility_pixels.present_id != 2
    {
        return Err(io::Error::other(format!(
            "recreated and compatibility presents lost exact identity: recreated={}/{} compatibility={}/{}",
            recreated_capture.workload_id,
            recreated_capture.present_id,
            compatibility_pixels.workload_id,
            compatibility_pixels.present_id
        ))
        .into());
    }
    let compatibility_error = backend.release_capture().unwrap_err();
    if !compatibility_error
        .to_string()
        .contains("requires a completed live-register VI present")
    {
        return Err(io::Error::other(format!(
            "compatibility presentation entered release evidence: {compatibility_error}"
        ))
        .into());
    }

    // Keep the platform's existing backend-lifecycle denominator while making
    // its native VI pixel coverage non-shrinking. Reuse the recreated context
    // so this end-to-end gate exercises live policy and drawable transitions
    // without turning one runtime lifecycle into three artificial Cocoa/Metal
    // ownership cycles.
    let vi_filter_summary = vi_filter_behavior::run_on_backend(&mut backend)?;
    let vi_aa_summary = vi_aa_selector_behavior::run_on_backend(&mut backend)?;
    if vi_filter_summary
        != (vi_filter_behavior::ViFilterRunSummary {
            workload_id: 2,
            first_present_id: 3,
            last_present_id: 22,
        })
        || vi_aa_summary
            != (vi_aa_selector_behavior::ViAaSelectorRunSummary {
                workload_id: 3,
                first_present_id: 23,
                last_present_id: 33,
            })
    {
        return Err(io::Error::other(format!(
            "native VI suites did not preserve exact cross-gate identity: filter={vi_filter_summary:?} aa={vi_aa_summary:?}"
        ))
        .into());
    }

    let failed_recreate = backend
        .create(&RenderConfig::for_tv(0, recreated.height, TvType::Ntsc))
        .unwrap_err();
    if !failed_recreate.to_string().contains("non-zero") {
        return Err(io::Error::other(format!(
            "invalid Metal recreation lost its named diagnostic: {failed_recreate}"
        ))
        .into());
    }
    if backend.release_environment().tv_type().is_some() {
        return Err(io::Error::other("failed Metal recreation retained stale TV authority").into());
    }

    println!(
        "metal_backend_evidence source={} provenance={:?} pre_workload_presents=2 initial_native={} initial_post_vi={} initial_workload_id={} initial_present_id={} transition_native={} transition_post_vi={} transition={}x{} transition_workload_id={} transition_present_id={} resized_native={} resized_post_vi={} resized={}x{} resized_workload_id={} resized_present_id={} recreated_native={} recreated_post_vi={} recreated={}x{} recreated_workload_id={} recreated_present_id={} policy_sha256={}",
        identity.source_id,
        identity.source_provenance,
        digest(&initial_native),
        digest(&initial_capture.bytes),
        initial_capture.workload_id,
        initial_capture.present_id,
        digest(&transition_native),
        digest(&transition_capture.bytes),
        transition_capture.width,
        transition_capture.height,
        transition_capture.workload_id,
        transition_capture.present_id,
        digest(&resized_native),
        digest(&resized_capture.bytes),
        resized_capture.width,
        resized_capture.height,
        resized_capture.workload_id,
        resized_capture.present_id,
        digest(&recreated_native),
        digest(&recreated_capture.bytes),
        recreated_capture.width,
        recreated_capture.height,
        recreated_capture.workload_id,
        recreated_capture.present_id,
        digest(&policy_sha256),
    );
    fn64_boot_harness::emit_rt64_platform_child_identity(
        identity.source_id,
        identity.is_source_authoritative(),
        identity.adapter_source_sha256,
        identity.post_vi_api,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_fixture_binds_dimensions_target_and_fill_color() {
        let spec = FixtureSpec {
            width: 8,
            height: 4,
            target: 0x12340,
            rgba16: 0x07c1,
            guest_cycle: 9,
        };
        let rdram = fixture(spec);
        let word = |index: usize, half: usize| {
            let offset = COMMANDS + index * 8 + half * 4;
            u32::from_ne_bytes(rdram[offset..offset + 4].try_into().unwrap())
        };
        assert_eq!(word(1, 0), 0xff10_0007);
        assert_eq!(word(1, 1), spec.target);
        assert_eq!(word(2, 1), 0x07c1_07c1);
        assert_eq!(word(3, 0), 0xf601_c00c);
    }

    #[test]
    fn capture_validation_rejects_unbound_geometry() {
        let spec = FixtureSpec {
            width: 4,
            height: 2,
            target: 0x400,
            rgba16: 0xf801,
            guest_cycle: 7,
        };
        let identity = Rt64BackendIdentity {
            adapter: "fn64-render-rt64/rt64",
            adapter_source_sha256:
                "1111111111111111111111111111111111111111111111111111111111111111",
            source_id: PINNED_SOURCE,
            source_provenance: Rt64SourceProvenance::GitClean,
            source_overlay_id:
                "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-dither-filter:v1+vi-divot:v1+vi-silhouette-aa:v1+vi-retrace-cadence:v1+rdp-alpha-dither:v1+rdp-shared-fragment-noise:v1",
            post_vi_api: "metal-bgra8-unorm",
        };
        let capture = RenderReleaseCapture {
            guest_cycle: spec.guest_cycle,
            backend_identity: identity.canonical_id(),
            source_authoritative: true,
            settings_sha256: [3; 32],
            width: spec.width + 1,
            height: spec.height,
            row_bytes: (spec.width + 1) * 4,
            format: ReleaseCaptureFormat::PostViBgra8Unorm,
            workload_id: std::num::NonZeroU64::new(1).unwrap(),
            present_id: 1,
            bytes: vec![1; ((spec.width + 1) * spec.height * 4) as usize],
        };
        assert!(validate_capture(spec, &capture, &identity, [3; 32]).is_err());
    }
}
