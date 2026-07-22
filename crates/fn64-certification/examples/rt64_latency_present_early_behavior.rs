use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi, RenderPolicyApply,
    RenderPresentationMode, RenderRuntimeSettings, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const COMMAND_COUNT: usize = 5;
const TARGET: u32 = 0x10_0000;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const SOURCE_HEIGHT: u32 = HEIGHT;
const SOURCE_PIXEL_COUNT: u32 = WIDTH * SOURCE_HEIGHT;
// Pinned RT64's synthetic VI mapping exposes these exact framebuffer rows in
// the two paths. Pixels outside the bands are opaque border black, not valid
// framebuffer samples, and remain checked just as strictly below.
const CONSOLE_VALID_END_ROW: u32 = 236;
const EARLY_VALID_START_ROW: u32 = 2;
const EARLY_VALID_END_ROW: u32 = 234;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const GREEN: u16 = 0x07c1;
const STALE: u16 = 0xffff;
const GUARD: u16 = 0x4211;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";

fn write_command(rdram: &mut [u8], index: usize, word0: u32, word1: u32) {
    let offset = COMMANDS + index * 8;
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn seed_rdram() -> Vec<u8> {
    let mut rdram = vec![0; RDRAM_LEN];
    let mut view = RdramViewMut::from_storage(&mut rdram);
    for index in 0..SOURCE_PIXEL_COUNT {
        view.write_u16(RdramAddr::from_offset(TARGET + index * 2), STALE);
    }
    view.write_u16(RdramAddr::from_offset(TARGET - 2), GUARD);
    view.write_u16(
        RdramAddr::from_offset(TARGET + SOURCE_PIXEL_COUNT * 2),
        GUARD,
    );
    rdram
}

fn submit_fill(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    color: u16,
) -> Result<(), Box<dyn Error>> {
    let lower_right = (((WIDTH - 1) * 4) << 12) | ((SOURCE_HEIGHT - 1) * 4);
    for (index, (word0, word1)) in [
        (0xef30_00f0, 0), // Fill cycle, RGB/alpha dither disabled.
        (0xff10_0000 | (WIDTH - 1), TARGET),
        (0xf700_0000, u32::from(color) * 0x1_0001),
        (0xf600_0000 | lower_right, 0),
        (0xe900_0000, 0),
    ]
    .into_iter()
    .enumerate()
    {
        write_command(rdram, index, word0, word1);
    }
    let end = COMMANDS + COMMAND_COUNT * 8;
    let status = backend.process_rdp_commands(rdram, COMMANDS as u32, end as u32, TARGET)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!("PresentEarly fixture returned {status:?}")).into());
    }
    Ok(())
}

fn explicit_present(
    backend: &mut Rt64Backend,
    rdram: &[u8],
    guest_cycle: u64,
) -> Result<Rt64PresentedPixels, Box<dyn Error>> {
    backend.present_physical_compatibility(
        rdram,
        ViPresentation {
            noise_seed: guest_cycle,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    Ok(backend.presented_pixels()?)
}

fn require_capture_shape(label: &str, capture: &Rt64PresentedPixels) -> Result<(), Box<dyn Error>> {
    if capture.width != WIDTH
        || capture.height != HEIGHT
        || capture.row_bytes != WIDTH * 4
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.present_id == 0
        || capture.bytes.len() != (WIDTH * HEIGHT * 4) as usize
    {
        return Err(io::Error::other(format!(
            "{label} capture has the wrong shape: dimensions={}x{}, row_bytes={}, format={:?}, present_id={}, byte_len={}",
            capture.width,
            capture.height,
            capture.row_bytes,
            capture.format,
            capture.present_id,
            capture.bytes.len(),
        ))
        .into());
    }
    Ok(())
}

fn require_seed_capture(capture: &Rt64PresentedPixels) -> Result<usize, Box<dyn Error>> {
    require_capture_shape("seeded Console history", capture)?;
    let red = [0x00, 0x00, 0xff, 0xff];
    let black = [0x00, 0x00, 0x00, 0xff];
    for (index, pixel) in capture.bytes.chunks_exact(4).enumerate() {
        let row = index as u32 / WIDTH;
        let expected = if row < CONSOLE_VALID_END_ROW {
            red.as_slice()
        } else {
            black.as_slice()
        };
        if pixel != expected {
            return Err(io::Error::other(format!(
                "seeded Console history pixel {index} differs: expected={expected:02x?}, actual={pixel:02x?}"
            ))
            .into());
        }
    }
    Ok((WIDTH * CONSOLE_VALID_END_ROW) as usize)
}

fn require_recolored_capture(
    capture: &Rt64PresentedPixels,
    expected_bgra: [u8; 4],
) -> Result<(), Box<dyn Error>> {
    require_capture_shape("PresentEarly process-time result", capture)?;
    let black = [0x00, 0x00, 0x00, 0xff];
    for (index, actual) in capture.bytes.chunks_exact(4).enumerate() {
        let row = index as u32 / WIDTH;
        let expected = if (EARLY_VALID_START_ROW..EARLY_VALID_END_ROW).contains(&row) {
            expected_bgra.as_slice()
        } else {
            black.as_slice()
        };
        if actual != expected {
            return Err(io::Error::other(format!(
                "PresentEarly pixel {index} differs: expected={expected:02x?}, actual={actual:02x?}, present_id={}, digest={}",
                capture.present_id,
                digest(&capture.bytes),
            ))
            .into());
        }
    }
    Ok(())
}

fn require_rdram(rdram: &[u8], expected: u16) -> Result<Vec<u16>, Box<dyn Error>> {
    let view = RdramView::from_storage(rdram);
    let pixels: Vec<_> = (0..SOURCE_PIXEL_COUNT)
        .map(|index| view.read_u16(RdramAddr::from_offset(TARGET + index * 2)))
        .collect();
    if pixels.iter().any(|pixel| *pixel != expected) {
        return Err(io::Error::other(format!(
            "PresentEarly RDRAM is not uniformly {expected:#06x}: {pixels:04x?}"
        ))
        .into());
    }
    for address in [TARGET - 2, TARGET + SOURCE_PIXEL_COUNT * 2] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(io::Error::other(format!(
                "PresentEarly fill escaped at {address:#010x}: {actual:#06x}"
            ))
            .into());
        }
    }
    Ok(pixels)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_u16(values: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.to_be_bytes());
    }
    digest(&hasher.finalize())
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_id != PINNED_SOURCE
        || source.source_provenance != Rt64SourceProvenance::GitClean
        || source.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "PresentEarly evidence requires clean pinned Metal RT64: {source:?}"
        ))
        .into());
    }

    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let console = RenderEnhancementSettings {
        presentation_mode: RenderPresentationMode::Console,
        ..RenderEnhancementSettings::default()
    };
    let early = RenderEnhancementSettings {
        presentation_mode: RenderPresentationMode::PresentEarly,
        ..RenderEnhancementSettings::default()
    };
    let mut backend = Rt64Backend::new()
        .with_runtime_settings(runtime.clone())
        .with_enhancement_settings(console.clone());
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    if backend.active_settings() != Some(&runtime)
        || backend.active_enhancement_settings() != Some(&console)
    {
        return Err(io::Error::other("RT64 did not activate the Console control policy").into());
    }
    let console_policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("Console control has no active policy"))?
        .sha256();

    let mut rdram = seed_rdram();
    submit_fill(&mut backend, &mut rdram, RED)?;
    require_rdram(&rdram, RED)?;
    let history = explicit_present(&mut backend, &rdram, 100)?;
    let valid_pixels = require_seed_capture(&history)?;

    // Console control: raw FullSync completes and mutates the framebuffer,
    // but no process-time presentation is allowed. Reading capture therefore
    // returns the exact seeded history image and ID without calling present.
    submit_fill(&mut backend, &mut rdram, BLUE)?;
    let console_pixels = require_rdram(&rdram, BLUE)?;
    let console_after_process = backend.presented_pixels()?;
    require_capture_shape("Console process-time control", &console_after_process)?;
    if console_after_process.present_id != history.present_id
        || console_after_process.bytes != history.bytes
    {
        return Err(io::Error::other(format!(
            "Console raw process changed seeded presentation history: history={}, after={}",
            history.present_id, console_after_process.present_id
        ))
        .into());
    }

    let expected_early_policy = {
        let mut policy = backend.configured_runtime_policy();
        policy.enhancement = early.clone();
        policy.sha256()
    };
    let applied = backend.apply_enhancement_settings(&early)?;
    if applied
        != (RenderPolicyApply::LiveApplied {
            policy_sha256: expected_early_policy,
        })
        || backend.active_enhancement_settings() != Some(&early)
    {
        return Err(io::Error::other(format!(
            "RT64 did not live-apply exact PresentEarly policy: {applied:?}"
        ))
        .into());
    }

    // No explicit present follows this point. Pinned RT64 must match TARGET to
    // the seeded VI history during FullSync, and the shim must not return until
    // the resulting present hook has published its new capture generation.
    submit_fill(&mut backend, &mut rdram, GREEN)?;
    let early_pixels = require_rdram(&rdram, GREEN)?;
    let early_capture = backend.presented_pixels()?;
    require_recolored_capture(&early_capture, [0x00, 0xff, 0x00, 0xff])?;
    if early_capture.present_id <= console_after_process.present_id {
        return Err(io::Error::other(format!(
            "PresentEarly did not publish a fresh process-time capture: console={}, early={}",
            console_after_process.present_id, early_capture.present_id
        ))
        .into());
    }

    println!(
        "rt64 present-early behavior: history_id={} console_process_id={} early_process_id={} history_valid_pixels={} early_valid_pixels={} history_sha256={} console_process_sha256={} early_process_sha256={} console_rdram_sha256={} early_rdram_sha256={} console_policy_sha256={} early_policy_sha256={} source={:?}",
        history.present_id,
        console_after_process.present_id,
        early_capture.present_id,
        valid_pixels,
        WIDTH * (EARLY_VALID_END_ROW - EARLY_VALID_START_ROW),
        digest(&history.bytes),
        digest(&console_after_process.bytes),
        digest(&early_capture.bytes),
        digest_u16(&console_pixels),
        digest_u16(&early_pixels),
        digest(&console_policy),
        digest(&expected_early_policy),
        source,
    );
    fn64_boot_harness::emit_rt64_platform_child_identity(
        source.source_id,
        source.is_source_authoritative(),
        source.adapter_source_sha256,
        source.post_vi_api,
    )?;
    Ok(())
}
