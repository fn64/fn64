use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi, RenderPresentationMode,
    RenderRuntimeSettings, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentSelection, Rt64PresentedPixels,
    Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const COMMAND_COUNT: usize = 5;
const A: u32 = 0x10_0000;
const B: u32 = 0x14_0000;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const PIXEL_COUNT: u32 = WIDTH * HEIGHT;

const RED: u16 = 0xf801;
const GREEN: u16 = 0x07c1;
const BLUE: u16 = 0x003f;
const STALE: u16 = 0xffff;
const GUARD: u16 = 0x4211;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";

#[derive(Debug)]
struct ModeEvidence {
    mode: RenderPresentationMode,
    seed_present_ids: [u64; 3],
    final_capture: Rt64PresentedPixels,
    final_selection: Rt64PresentSelection,
    policy_sha256: [u8; 32],
    a_pixels: Vec<u16>,
    b_pixels: Vec<u16>,
}

#[derive(Debug)]
struct PresentedFrame {
    capture: Rt64PresentedPixels,
    selection: Rt64PresentSelection,
}

fn write_command(rdram: &mut [u8], index: usize, word0: u32, word1: u32) {
    let offset = COMMANDS + index * 8;
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn seed_rdram() -> Vec<u8> {
    let mut rdram = vec![0; RDRAM_LEN];
    let mut view = RdramViewMut::from_storage(&mut rdram);
    for target in [A, B] {
        for index in 0..PIXEL_COUNT {
            view.write_u16(RdramAddr::from_offset(target + index * 2), STALE);
        }
        view.write_u16(RdramAddr::from_offset(target - 2), GUARD);
        view.write_u16(RdramAddr::from_offset(target + PIXEL_COUNT * 2), GUARD);
    }
    rdram
}

fn submit_fill(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    target: u32,
    color: u16,
    vi_output: u32,
) -> Result<(), Box<dyn Error>> {
    let lower_right = (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4);
    for (index, (word0, word1)) in [
        (0xef30_00f0, 0), // Fill cycle, RGB/alpha dither disabled.
        (0xff10_0000 | (WIDTH - 1), target),
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
    let status = backend.process_rdp_commands(rdram, COMMANDS as u32, end as u32, vi_output)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "latency fixture returned {status:?} while filling {target:#010x}"
        ))
        .into());
    }
    Ok(())
}

fn present(backend: &mut Rt64Backend, guest_cycle: u64) -> Result<PresentedFrame, Box<dyn Error>> {
    backend.present(ViPresentation {
        noise_seed: guest_cycle,
        filters: ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        },
        ..ViPresentation::default()
    })?;
    let capture = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    if selection.present_id != capture.present_id {
        return Err(io::Error::other(format!(
            "present selection/capture association differs: selection={selection:?}, capture_id={}",
            capture.present_id
        ))
        .into());
    }
    Ok(PresentedFrame { capture, selection })
}

fn require_selection(
    label: &str,
    selection: &Rt64PresentSelection,
    target_address: u32,
) -> Result<(), Box<dyn Error>> {
    if selection.present_id == 0
        || selection.source_texture_identity == 0
        || selection.target_address != target_address
        || selection.target_width != WIDTH
        || selection.target_height != HEIGHT
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "{label} did not bind exact RGBA16 target {target_address:#010x}: {selection:?}"
        ))
        .into());
    }
    Ok(())
}

fn pixels(rdram: &[u8], target: u32) -> Vec<u16> {
    let view = RdramView::from_storage(rdram);
    (0..PIXEL_COUNT)
        .map(|index| view.read_u16(RdramAddr::from_offset(target + index * 2)))
        .collect()
}

fn require_uniform_rdram(
    rdram: &[u8],
    target: u32,
    expected: u16,
) -> Result<Vec<u16>, Box<dyn Error>> {
    let actual = pixels(rdram, target);
    if let Some(index) = actual.iter().position(|pixel| *pixel != expected) {
        return Err(io::Error::other(format!(
            "framebuffer {target:#010x} first differs from {expected:#06x} at pixel {index}: actual={:#06x}, digest={}",
            actual[index],
            digest_u16(&actual)
        ))
        .into());
    }
    Ok(actual)
}

fn require_capture(
    label: &str,
    capture: &Rt64PresentedPixels,
    active_bgra: [u8; 4],
    active_rows: std::ops::Range<u32>,
) -> Result<(), Box<dyn Error>> {
    let expected_len = (WIDTH * HEIGHT * 4) as usize;
    // The pinned synthetic VI register image exposes rows 0..235. Its final
    // four 320x240 swapchain rows are outside that active/crop region and are
    // deterministically cleared to opaque black. This exact geometry is held
    // constant across both latency policies.
    let first_mismatch = capture
        .bytes
        .chunks_exact(4)
        .enumerate()
        .position(|(index, pixel)| {
            let y = index as u32 / WIDTH;
            let expected = if active_rows.contains(&y) {
                active_bgra
            } else {
                [0x00, 0x00, 0x00, 0xff]
            };
            pixel != expected
        });
    if capture.width != WIDTH
        || capture.height != HEIGHT
        || capture.row_bytes != WIDTH * 4
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.present_id == 0
        || capture.bytes.len() != expected_len
        || first_mismatch.is_some()
    {
        let first_actual = first_mismatch.map(|index| {
            let start = index * 4;
            capture.bytes[start..start + 4].to_vec()
        });
        let counts = [
            [0x00, 0x00, 0x00, 0xff],
            [0x00, 0x00, 0xff, 0xff],
            [0x00, 0xff, 0x00, 0xff],
            [0xff, 0x00, 0x00, 0xff],
        ]
        .map(|candidate| {
            capture
                .bytes
                .chunks_exact(4)
                .filter(|pixel| *pixel == candidate)
                .count()
        });
        let matching_rows: Vec<u32> = capture
            .bytes
            .chunks_exact((WIDTH * 4) as usize)
            .enumerate()
            .filter_map(|(row, bytes)| {
                bytes
                    .chunks_exact(4)
                    .all(|pixel| pixel == active_bgra)
                    .then_some(row as u32)
            })
            .collect();
        return Err(io::Error::other(format!(
            "{label} capture did not contain {active_bgra:02x?} in rows {active_rows:?} and opaque black elsewhere: dimensions={}x{}, row_bytes={}, format={:?}, present_id={}, byte_len={}, first_mismatch={first_mismatch:?}, first_actual={first_actual:02x?}, matching_rows={:?}..={:?}/count={}, bgra_black_red_green_blue_counts={counts:?}, digest={}",
            capture.width,
            capture.height,
            capture.row_bytes,
            capture.format,
            capture.present_id,
            capture.bytes.len(),
            matching_rows.first(),
            matching_rows.last(),
            matching_rows.len(),
            digest(&capture.bytes)
        ))
        .into());
    }
    Ok(())
}

fn require_guards(rdram: &[u8]) -> Result<(), Box<dyn Error>> {
    let view = RdramView::from_storage(rdram);
    for address in [A - 2, A + PIXEL_COUNT * 2, B - 2, B + PIXEL_COUNT * 2] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(io::Error::other(format!(
                "latency fixture write escaped at {address:#010x}: {actual:#06x}"
            ))
            .into());
        }
    }
    Ok(())
}

fn run_mode(mode: RenderPresentationMode) -> Result<ModeEvidence, Box<dyn Error>> {
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let enhancement = RenderEnhancementSettings {
        presentation_mode: mode,
        ..RenderEnhancementSettings::default()
    };
    let mut backend = Rt64Backend::new()
        .with_runtime_settings(runtime.clone())
        .with_enhancement_settings(enhancement.clone());
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    if backend.active_settings() != Some(&runtime)
        || backend.active_enhancement_settings() != Some(&enhancement)
    {
        return Err(io::Error::other(format!(
            "RT64 did not activate the exact {mode:?} latency policy"
        ))
        .into());
    }
    backend.enable_present_capture()?;
    let policy_sha256 = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("latency fixture has no active runtime policy"))?
        .sha256();

    let mut rdram = seed_rdram();

    // Seed both RT64 VI histories with A. This is the only presented image
    // before the differential event, and its exact red pixels also prove the
    // capture belongs to this backend instance.
    submit_fill(&mut backend, &mut rdram, A, RED, A)?;
    require_uniform_rdram(&rdram, A, RED)?;
    let initial = present(&mut backend, 100)?;
    require_selection("initial A", &initial.selection, A)?;
    require_capture(
        &format!("initial A selected {:?}", initial.selection),
        &initial.capture,
        [0x00, 0x00, 0xff, 0xff],
        0..236,
    )?;

    // Establish B as the next green screen buffer, then return to red A. This
    // models a native buffering ring whose members are both valid prior
    // presentation targets and makes the final VI change from A back to B.
    submit_fill(&mut backend, &mut rdram, B, GREEN, B)?;
    require_uniform_rdram(&rdram, B, GREEN)?;
    let b_seed = present(&mut backend, 110)?;
    require_selection("seed B", &b_seed.selection, B)?;
    require_capture(
        &format!("seed B selected {:?}", b_seed.selection),
        &b_seed.capture,
        [0x00, 0xff, 0x00, 0xff],
        2..234,
    )?;
    if b_seed.capture.present_id <= initial.capture.present_id {
        return Err(io::Error::other(format!(
            "{mode:?} B seed presentation did not advance: A={}, B={}",
            initial.capture.present_id, b_seed.capture.present_id
        ))
        .into());
    }

    submit_fill(&mut backend, &mut rdram, A, RED, A)?;
    require_uniform_rdram(&rdram, A, RED)?;
    let a_return = present(&mut backend, 120)?;
    require_selection("return to A", &a_return.selection, A)?;
    require_capture(
        &format!("return to A selected {:?}", a_return.selection),
        &a_return.capture,
        [0x00, 0x00, 0xff, 0xff],
        2..234,
    )?;
    if a_return.capture.present_id <= b_seed.capture.present_id {
        return Err(io::Error::other(format!(
            "{mode:?} A return presentation did not advance: B={}, A={}",
            b_seed.capture.present_id, a_return.capture.present_id
        ))
        .into());
    }

    // The newest workload modifies prior-history A to blue while the VI now
    // points at established green B. Console must retain B; SkipBuffering may
    // substitute only the freshly modified compatible history buffer A.
    submit_fill(&mut backend, &mut rdram, A, BLUE, B)?;
    let a_pixels = require_uniform_rdram(&rdram, A, BLUE)?;
    let b_pixels = require_uniform_rdram(&rdram, B, GREEN)?;
    require_guards(&rdram)?;

    let final_frame = present(&mut backend, 200)?;
    let expected_address = match mode {
        RenderPresentationMode::Console => B,
        RenderPresentationMode::SkipBuffering => A,
        RenderPresentationMode::PresentEarly => {
            return Err(io::Error::other("skip-buffering fixture received PresentEarly").into());
        }
    };
    require_selection(
        "final buffered selection",
        &final_frame.selection,
        expected_address,
    )?;
    if final_frame.capture.present_id <= a_return.capture.present_id {
        return Err(io::Error::other(format!(
            "{mode:?} did not publish a fresh final presentation: prior={}, final={}",
            a_return.capture.present_id, final_frame.capture.present_id
        ))
        .into());
    }
    let expected_bgra = match mode {
        RenderPresentationMode::Console => [0x00, 0xff, 0x00, 0xff],
        RenderPresentationMode::SkipBuffering => [0xff, 0x00, 0x00, 0xff],
        RenderPresentationMode::PresentEarly => {
            return Err(io::Error::other("skip-buffering fixture received PresentEarly").into());
        }
    };
    require_capture(
        &format!(
            "final buffered selection selected {:?}",
            final_frame.selection
        ),
        &final_frame.capture,
        expected_bgra,
        2..234,
    )?;

    Ok(ModeEvidence {
        mode,
        seed_present_ids: [
            initial.capture.present_id,
            b_seed.capture.present_id,
            a_return.capture.present_id,
        ],
        final_capture: final_frame.capture,
        final_selection: final_frame.selection,
        policy_sha256,
        a_pixels,
        b_pixels,
    })
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
            "skip-buffering evidence requires clean pinned Metal RT64: {source:?}"
        ))
        .into());
    }

    let console = run_mode(RenderPresentationMode::Console)?;
    let skip = run_mode(RenderPresentationMode::SkipBuffering)?;
    if console.final_capture.bytes == skip.final_capture.bytes {
        return Err(io::Error::other(
            "Console and SkipBuffering selected the same final presentation",
        )
        .into());
    }

    println!(
        "console_mode={:?} console_seed_presents={:?} console_final_present={} console_final_selection={:?} console_capture={} skip_mode={:?} skip_seed_presents={:?} skip_final_present={} skip_final_selection={:?} skip_capture={} a_pixels={} b_pixels={} console_policy={} skip_policy={} source={:?}",
        console.mode,
        console.seed_present_ids,
        console.final_capture.present_id,
        console.final_selection,
        digest(&console.final_capture.bytes),
        skip.mode,
        skip.seed_present_ids,
        skip.final_capture.present_id,
        skip.final_selection,
        digest(&skip.final_capture.bytes),
        digest_u16(&skip.a_pixels),
        digest_u16(&skip.b_pixels),
        digest(&console.policy_sha256),
        digest(&skip.policy_sha256),
        source,
    );
    Ok(())
}
