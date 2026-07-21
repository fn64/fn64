use std::error::Error;
use std::io;
use std::ops::Range;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRuntimeSettings, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64DeferredWorkloadEvidence, Rt64PresentPixelFormat, Rt64PresentSelection,
    Rt64PresentedPixels, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
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
struct PresentedFrame {
    capture: Rt64PresentedPixels,
    selection: Rt64PresentSelection,
}

fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

fn fixture() -> (Vec<u8>, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for target in [A, B] {
            for index in 0..PIXEL_COUNT {
                view.write_u16(RdramAddr::from_offset(target + index * 2), STALE);
            }
            view.write_u16(RdramAddr::from_offset(target - 2), GUARD);
            view.write_u16(RdramAddr::from_offset(target + PIXEL_COUNT * 2), GUARD);
        }
    }

    let mut commands = vec![
        (0xef30_00f0, 0), // Fill cycle, RGB/alpha dither disabled.
        (0xff10_0000 | (WIDTH - 1), A),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
        (0xf700_0000, u32::from(BLUE) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, WIDTH / 2, 0),
        (0xe700_0000, 0), // PipeSync before the framebuffer boundary.
        (0xff10_0000 | (WIDTH - 1), B),
        (0xf700_0000, u32::from(GREEN) * 0x1_0001),
        fill_rect(WIDTH - 1, HEIGHT - 1, 0, 0),
        (0xe900_0000, 0), // FullSync.
    ];
    let end = COMMANDS + commands.len() * 8;
    for (index, (word0, word1)) in commands.drain(..).enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    (rdram, end as u32)
}

fn present(backend: &mut Rt64Backend, guest_cycle: u64) -> Result<PresentedFrame, Box<dyn Error>> {
    backend.present(ViPresentation {
        noise_seed: guest_cycle,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    })?;
    let capture = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    if capture.present_id != selection.present_id {
        return Err(io::Error::other(format!(
            "capture/selection association differs: capture={}, selection={selection:?}",
            capture.present_id
        ))
        .into());
    }
    Ok(PresentedFrame { capture, selection })
}

fn require_selection(
    label: &str,
    selection: &Rt64PresentSelection,
    address: u32,
) -> Result<(), Box<dyn Error>> {
    if selection.present_id == 0
        || selection.source_texture_identity == 0
        || selection.target_address != address
        || selection.target_width != WIDTH
        || selection.target_height != HEIGHT
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "{label} did not select exact RGBA16 framebuffer {address:#010x}: {selection:?}"
        ))
        .into());
    }
    Ok(())
}

fn require_capture(
    label: &str,
    capture: &Rt64PresentedPixels,
    active_rows: Range<u32>,
    left: [u8; 4],
    right: [u8; 4],
) -> Result<(), Box<dyn Error>> {
    let first_mismatch = capture
        .bytes
        .chunks_exact(4)
        .enumerate()
        .position(|(index, pixel)| {
            let x = index as u32 % WIDTH;
            let y = index as u32 / WIDTH;
            let expected = if !active_rows.contains(&y) {
                [0x00, 0x00, 0x00, 0xff]
            } else if x < WIDTH / 2 {
                left
            } else {
                right
            };
            pixel != expected
        });
    if capture.width != WIDTH
        || capture.height != HEIGHT
        || capture.row_bytes != WIDTH * 4
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.bytes.len() != (WIDTH * HEIGHT * 4) as usize
        || first_mismatch.is_some()
    {
        let first_actual = first_mismatch.map(|index| {
            let start = index * 4;
            &capture.bytes[start..start + 4]
        });
        return Err(io::Error::other(format!(
            "{label} capture differs: rows={active_rows:?}, left={left:02x?}, right={right:02x?}, dimensions={}x{}, row_bytes={}, format={:?}, present_id={}, first_mismatch={first_mismatch:?}, first_actual={first_actual:02x?}, digest={}",
            capture.width,
            capture.height,
            capture.row_bytes,
            capture.format,
            capture.present_id,
            digest(&capture.bytes)
        ))
        .into());
    }
    Ok(())
}

fn require_rdram(rdram: &[u8]) -> Result<(), Box<dyn Error>> {
    let view = RdramView::from_storage(rdram);
    for index in 0..PIXEL_COUNT {
        let x = index % WIDTH;
        let expected_a = if x < WIDTH / 2 { RED } else { BLUE };
        let actual_a = view.read_u16(RdramAddr::from_offset(A + index * 2));
        let actual_b = view.read_u16(RdramAddr::from_offset(B + index * 2));
        if (actual_a, actual_b) != (expected_a, GREEN) {
            return Err(io::Error::other(format!(
                "deferred fixture RDRAM differs at pixel {index}: A={actual_a:#06x}, B={actual_b:#06x}"
            ))
            .into());
        }
    }
    for address in [A - 2, A + PIXEL_COUNT * 2, B - 2, B + PIXEL_COUNT * 2] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(io::Error::other(format!(
                "deferred fixture escaped at {address:#010x}: {actual:#06x}"
            ))
            .into());
        }
    }
    Ok(())
}

fn require_initial_history(evidence: &Rt64DeferredWorkloadEvidence) -> Result<(), Box<dyn Error>> {
    let pre = &evidence.pre_submission;
    if pre != &evidence.current
        || pre.workload_id == 0
        || pre.content_digest == 0
        || pre.identity_digest == 0
        || pre.framebuffer_pair_count != 2
        || pre.projection_count != 2
        || pre.game_call_count != 3
        || pre.triangle_count != 6
        || pre.selected_framebuffer_index != -1
        || pre.selected_draw_call_index != -1
        || pre.selected_framebuffer_address != 0
        || pre.paused
        || pre.pair_color_addresses[..2] != [A, B]
        || pre.pair_game_call_counts[..2] != [2, 1]
        || pre.pair_projection_counts[..2] != [1, 1]
        || pre.call_fill_colors[..3]
            != [
                u32::from(RED) * 0x1_0001,
                u32::from(BLUE) * 0x1_0001,
                u32::from(GREEN) * 0x1_0001,
            ]
        || pre.call_triangle_counts[..3] != [2, 2, 2]
    {
        return Err(io::Error::other(format!(
            "pre-submit deferred workload is not the exact ordered fixture: {evidence:#?}"
        ))
        .into());
    }
    Ok(())
}

fn require_replay_history(
    label: &str,
    evidence: &Rt64DeferredWorkloadEvidence,
    previous_workload_id: u64,
    framebuffer_index: i32,
    draw_call_index: i32,
    framebuffer_address: u32,
) -> Result<(), Box<dyn Error>> {
    let pre = &evidence.pre_submission;
    let current = &evidence.current;
    if current.content_digest != pre.content_digest
        || current.identity_digest == pre.identity_digest
        || current.workload_id <= previous_workload_id
        || current.submission_frame != pre.submission_frame
        || current.framebuffer_pair_count != pre.framebuffer_pair_count
        || current.game_call_count != pre.game_call_count
        || current.selected_framebuffer_index != framebuffer_index
        || current.selected_draw_call_index != draw_call_index
        || current.selected_framebuffer_address != framebuffer_address
        || !current.paused
    {
        return Err(io::Error::other(format!(
            "{label} did not preserve and replay the exact deferred history: {evidence:#?}"
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
    let source = Rt64Backend::release_identity();
    if source.source_id != PINNED_SOURCE
        || source.source_provenance != Rt64SourceProvenance::GitClean
        || source.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "deferred/debugger evidence requires clean pinned Metal RT64: {source:?}"
        ))
        .into());
    }

    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        developer_mode: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime.clone());
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    if backend.active_settings() != Some(&runtime) {
        return Err(io::Error::other("RT64 did not activate exact debugger fixture policy").into());
    }
    backend.enable_present_capture()?;
    backend.enable_deferred_workload_capture_for_evidence()?;

    let (mut rdram, end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, B)?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("deferred fixture raw workload returned {status:?}")).into(),
        );
    }
    require_rdram(&rdram)?;
    let initial_history = backend.deferred_workload_evidence()?;
    require_initial_history(&initial_history)?;

    // Establish an ordinary last Present. Headless debugger controls below
    // then exercise State::updateScreen's real paused repeat path.
    let initial = present(&mut backend, 100)?;
    require_selection("initial B", &initial.selection, B)?;
    require_capture(
        "initial B",
        &initial.capture,
        0..236,
        [0x00, 0xff, 0x00, 0xff],
        [0x00, 0xff, 0x00, 0xff],
    )?;

    backend.set_debugger_inspection_for_evidence(true, 0, -1, false)?;
    let full_a = present(&mut backend, 110)?;
    require_selection("paused full A", &full_a.selection, A)?;
    require_capture(
        "paused full A",
        &full_a.capture,
        0..236,
        [0x00, 0x00, 0xff, 0xff],
        [0xff, 0x00, 0x00, 0xff],
    )?;
    let full_a_history = backend.deferred_workload_evidence()?;
    require_replay_history(
        "paused full A",
        &full_a_history,
        initial_history.current.workload_id,
        0,
        -1,
        A,
    )?;

    backend.set_debugger_inspection_for_evidence(true, 0, 0, false)?;
    let first_call_a = present(&mut backend, 120)?;
    require_selection("paused A draw call 0", &first_call_a.selection, A)?;
    require_capture(
        "paused A draw call 0",
        &first_call_a.capture,
        0..236,
        [0x00, 0x00, 0xff, 0xff],
        [0x00, 0x00, 0xff, 0xff],
    )?;
    let first_call_history = backend.deferred_workload_evidence()?;
    require_replay_history(
        "paused A draw call 0",
        &first_call_history,
        full_a_history.current.workload_id,
        0,
        0,
        A,
    )?;

    backend.set_debugger_inspection_for_evidence(true, 1, -1, false)?;
    let full_b = present(&mut backend, 130)?;
    require_selection("paused full B", &full_b.selection, B)?;
    require_capture(
        "paused full B",
        &full_b.capture,
        0..236,
        [0x00, 0xff, 0x00, 0xff],
        [0x00, 0xff, 0x00, 0xff],
    )?;
    let full_b_history = backend.deferred_workload_evidence()?;
    require_replay_history(
        "paused full B",
        &full_b_history,
        first_call_history.current.workload_id,
        1,
        -1,
        B,
    )?;

    if !(initial.capture.present_id < full_a.capture.present_id
        && full_a.capture.present_id < first_call_a.capture.present_id
        && first_call_a.capture.present_id < full_b.capture.present_id)
    {
        return Err(io::Error::other("debugger replay present IDs did not advance exactly").into());
    }

    println!(
        "pre_workload={} pre_identity={:016x} replay_workload={} replay_identity={:016x} content={:016x} pairs={:?} calls={:?} initial_present={} initial_b={} full_a_present={} full_a={} call0_present={} call0={} full_b_present={} full_b={} source={source:?}",
        initial_history.pre_submission.workload_id,
        initial_history.pre_submission.identity_digest,
        full_b_history.current.workload_id,
        full_b_history.current.identity_digest,
        full_b_history.current.content_digest,
        &full_b_history.current.pair_color_addresses[..2],
        &full_b_history.current.pair_game_call_counts[..2],
        initial.capture.present_id,
        digest(&initial.capture.bytes),
        full_a.capture.present_id,
        digest(&full_a.capture.bytes),
        first_call_a.capture.present_id,
        digest(&first_call_a.capture.bytes),
        full_b.capture.present_id,
        digest(&full_b.capture.bytes),
    );
    Ok(())
}
