//! Public, non-ROM F3DEX2/Extended-GBI evidence for arbitrary aspect ratios.
//!
//! The opt-in synthetic admission substitutes only this fixture's F3DEX2
//! dialect. Production `process_task` hash recognition remains unchanged and
//! is checked before and after the live aspect transitions below.

use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, OsTask, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderResolution, RenderRuntimeSettings,
    RenderSettingsApply, ResolutionMultiplier, ViFilterControl, ViPixelType, ViPresentation,
    M_GFXTASK,
};
use fn64_render_rt64::{
    extended_gbi::{AspectMode, Availability, Origin, Policy, RectAlignment, Version1},
    gbi, Rt64Backend, Rt64ExtendedAspectMode, Rt64ExtendedGbiEvidence, Rt64PresentPixelFormat,
    Rt64PresentedPixels, Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_OVERLAY: &str = "fn64:raster-shader-start-stop:v1";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SEGMENT: u8 = 6;
const SEGMENT_BASE: usize = 0x0000_1000;
const VERTICES: usize = SEGMENT_BASE;
const PROJECTION: usize = SEGMENT_BASE + 0x0200;
const MODEL: usize = SEGMENT_BASE + 0x0240;
const VIEWPORT: usize = SEGMENT_BASE + 0x0280;
const VERSION_WORD: usize = 0x0000_1800;
const DISPLAY_LIST: usize = 0x0000_3000;
const TARGET: usize = 0x0040_0000;
const SOURCE_WIDTH: u32 = 64;
const SOURCE_HEIGHT: u32 = 48;
const PRESENT_WIDTH: u32 = 112;
const PRESENT_HEIGHT: u32 = 48;
const TARGET_BYTES: usize = SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 2;
const GUARD: u32 = 0xa5c3_7e19;

fn wr_u32(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn wr_u8(rdram: &mut [u8], offset: usize, value: u8) {
    rdram[offset ^ 3] = value;
}

fn wr_i16(rdram: &mut [u8], offset: usize, value: i16) {
    for (index, byte) in (value as u16).to_be_bytes().into_iter().enumerate() {
        wr_u8(rdram, offset + index, byte);
    }
}

fn write_matrix(rdram: &mut [u8], offset: usize, elements: [f32; 16]) {
    for (index, value) in elements.into_iter().enumerate() {
        let fixed = (value * 65536.0) as i32;
        wr_i16(rdram, offset + index * 2, (fixed >> 16) as i16);
        wr_i16(rdram, offset + 32 + index * 2, fixed as u16 as i16);
    }
}

fn push(commands: &mut Vec<(u32, u32)>, command: (u32, u32)) {
    commands.push(command);
}

fn fill_rect(upper_left: [u32; 2], lower_right: [u32; 2]) -> (u32, u32) {
    (
        0xf600_0000 | ((lower_right[0] * 4) << 12) | (lower_right[1] * 4),
        ((upper_left[0] * 4) << 12) | (upper_left[1] * 4),
    )
}

fn write_scene(rdram: &mut [u8], version: Version1) -> usize {
    let vertices = [
        ([-8_i16, -7_i16, 0_i16], [255_u8, 255_u8, 255_u8, 255_u8]),
        ([8, -7, 0], [255, 255, 255, 255]),
        ([0, 8, 0], [255, 255, 255, 255]),
    ];
    for (index, (position, color)) in vertices.into_iter().enumerate() {
        let offset = VERTICES + index * 16;
        wr_i16(rdram, offset, position[0]);
        wr_i16(rdram, offset + 2, position[1]);
        wr_i16(rdram, offset + 4, position[2]);
        for (channel, value) in color.into_iter().enumerate() {
            wr_u8(rdram, offset + 12 + channel, value);
        }
    }

    let mut projection = [0.0_f32; 16];
    projection[0] = 1.0 / 16.0;
    projection[5] = 1.0 / 16.0;
    projection[10] = 1.0;
    projection[15] = 1.0;
    write_matrix(rdram, PROJECTION, projection);

    let mut model = [0.0_f32; 16];
    model[0] = 1.0;
    model[5] = 1.0;
    model[10] = 1.0;
    model[12] = -0.125;
    model[15] = 1.0;
    write_matrix(rdram, MODEL, model);

    for (index, value) in [
        (SOURCE_WIDTH * 2) as i16,
        (SOURCE_HEIGHT * 2) as i16,
        511,
        0,
        (SOURCE_WIDTH * 2) as i16,
        (SOURCE_HEIGHT * 2) as i16,
        511,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        wr_i16(rdram, VIEWPORT + index * 2, value);
    }

    let alignment = version.set_rect_align(RectAlignment {
        left_origin: Origin::Left,
        right_origin: Origin::Right,
        left_offset: 6,
        top_offset: 0,
        right_offset: 6,
        bottom_offset: 0,
    });
    let mut commands = Vec::new();
    push(
        &mut commands,
        (
            ((gbi::G_MOVEWORD as u32) << 24) | (0x06 << 16) | (u32::from(SEGMENT) * 4),
            SEGMENT_BASE as u32,
        ),
    );
    push(
        &mut commands,
        (
            ((gbi::G_MOVEMEM as u32) << 24) | (1 << 19) | 8,
            (u32::from(SEGMENT) << 24) | 0x0280,
        ),
    );
    push(
        &mut commands,
        (0xff10_0000 | (SOURCE_WIDTH - 1), TARGET as u32),
    );
    push(
        &mut commands,
        (
            0xed00_0000 | (4 * 4) << 12 | (3 * 4),
            (60 * 4) << 12 | (45 * 4),
        ),
    );
    push(&mut commands, (0xef30_00f0, 0));
    push(&mut commands, (0xf700_0000, 0x003f_003f));
    push(
        &mut commands,
        fill_rect([0, 0], [SOURCE_WIDTH - 1, SOURCE_HEIGHT - 1]),
    );
    push(&mut commands, (0xe700_0000, 0));
    let matrix_length = (((64_u32 - 1) / 8) & 0x1f) << 19;
    push(
        &mut commands,
        (
            ((gbi::G_MTX as u32) << 24) | matrix_length | 0x07,
            (u32::from(SEGMENT) << 24) | 0x0200,
        ),
    );
    push(
        &mut commands,
        (
            ((gbi::G_MTX as u32) << 24) | matrix_length | 0x03,
            (u32::from(SEGMENT) << 24) | 0x0240,
        ),
    );
    push(&mut commands, (0xfcff_ffff, 0xfffd_f6fb));
    push(&mut commands, (0xfa00_0000, 0xf800_00ff));
    push(&mut commands, (0xef00_00f0, 0));
    push(
        &mut commands,
        (
            ((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
            u32::from(SEGMENT) << 24,
        ),
    );
    push(
        &mut commands,
        (((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0),
    );
    push(&mut commands, version.enable_command().words());
    push(
        &mut commands,
        version.set_rect_aspect(AspectMode::Adjust).words(),
    );
    push(&mut commands, alignment[0].words());
    push(&mut commands, alignment[1].words());
    push(&mut commands, (0xe700_0000, 0));
    push(&mut commands, (0xef30_00f0, 0));
    push(&mut commands, (0xf700_0000, 0x07c1_07c1));
    push(&mut commands, fill_rect([8, 31], [55, 40]));
    push(&mut commands, (0xe900_0000, 0));
    push(&mut commands, ((gbi::G_ENDDL as u32) << 24, 0));

    let end = DISPLAY_LIST + commands.len() * 8;
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        wr_u32(rdram, DISPLAY_LIST + index * 8, word0);
        wr_u32(rdram, DISPLAY_LIST + index * 8 + 4, word1);
    }
    end
}

fn settings(
    aspect_ratio: RenderAspectRatio,
    target: f64,
) -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(1.0)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio,
        aspect_target: AspectTarget::new(target)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn require_production_rejection(backend: &mut Rt64Backend) -> Result<(), Box<dyn Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    let status = backend.process_task(
        &mut rdram,
        &mut fn64_runtime::RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..OsTask::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "synthetic aspect fixture changed production microcode admission",
        )
        .into());
    }
    Ok(())
}

fn negotiate_v1(backend: &mut Rt64Backend) -> Result<Version1, Box<dyn Error>> {
    let probe = Policy::Required
        .probe(VERSION_WORD as u32)?
        .expect("required policy emits a probe");
    let mut rdram = vec![0; RDRAM_LEN];
    wr_u32(
        &mut rdram,
        VERSION_WORD,
        fn64_render_rt64::extended_gbi::Probe::RETURN_WORD_INITIALIZER,
    );
    let (word0, word1) = probe.command().words();
    let commands = [
        (word0, word1),
        (0xef30_00f0, 0),
        (0xff10_0000 | (SOURCE_WIDTH - 1), TARGET as u32),
        (0xf700_0000, 0x0001_0001),
        fill_rect([0, 0], [SOURCE_WIDTH - 1, SOURCE_HEIGHT - 1]),
        (0xe900_0000, 0),
        ((gbi::G_ENDDL as u32) << 24, 0),
    ];
    for (index, (command0, command1)) in commands.into_iter().enumerate() {
        wr_u32(&mut rdram, DISPLAY_LIST + index * 8, command0);
        wr_u32(&mut rdram, DISPLAY_LIST + index * 8 + 4, command1);
    }
    backend.process_synthetic_extended_f3dex2(&mut rdram, DISPLAY_LIST as u32, TARGET as u32)?;
    let response = u32::from_ne_bytes(
        rdram[VERSION_WORD..VERSION_WORD + 4]
            .try_into()
            .expect("four-byte version word"),
    );
    match probe.resolve(response)? {
        Availability::Version1(version) => Ok(version),
        Availability::Unavailable => {
            Err(io::Error::other("required synthetic Extended-GBI probe was unavailable").into())
        }
    }
}

fn active_policy_sha256(
    backend: &Rt64Backend,
    requested: &RenderRuntimeSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    let active = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("RT64 aspect fixture has no active runtime policy"))?;
    if active.user != *requested || active != backend.configured_runtime_policy() {
        return Err(
            io::Error::other("RT64 active aspect policy differs from requested policy").into(),
        );
    }
    Ok(active.sha256())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Shape {
    pixels: u32,
    bounds: [u32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Capture {
    present_id: u64,
    workload_id: u64,
    triangle: Shape,
    rectangle: Shape,
    scissor: Shape,
    sha256: [u8; 32],
    policy_sha256: [u8; 32],
}

#[derive(Copy, Clone)]
struct Expected {
    triangle: Shape,
    rectangle: Shape,
    scissor: Shape,
    sha256: &'static str,
}

const EXPECTED: [Expected; 4] = [
    Expected {
        triangle: Shape {
            pixels: 131,
            bounds: [35, 12, 49, 28],
        },
        rectangle: Shape {
            pixels: 279,
            bounds: [30, 29, 60, 37],
        },
        scissor: Shape {
            pixels: 1218,
            bounds: [24, 0, 60, 43],
        },
        sha256: "6c953ade5120bf0ba4aecc3c2df9da017a05356340bc37c387717a3f51ab24a8",
    },
    Expected {
        triangle: Shape {
            pixels: 137,
            bounds: [37, 12, 51, 28],
        },
        rectangle: Shape {
            pixels: 234,
            bounds: [34, 29, 59, 37],
        },
        scissor: Shape {
            pixels: 931,
            bounds: [29, 0, 59, 41],
        },
        sha256: "6aaf948790b0a233bb796404abb557d498dbf2335ab92a80aad28cf836da99d6",
    },
    Expected {
        triangle: Shape {
            pixels: 134,
            bounds: [36, 12, 50, 28],
        },
        rectangle: Shape {
            pixels: 261,
            bounds: [31, 29, 59, 37],
        },
        scissor: Shape {
            pixels: 1033,
            bounds: [26, 0, 59, 41],
        },
        sha256: "3ca118f7bdbab3adc5ce63e54fd39b6c146f5f5ab59d4035be6f52dddb436da2",
    },
    Expected {
        triangle: Shape {
            pixels: 135,
            bounds: [34, 12, 47, 28],
        },
        rectangle: Shape {
            pixels: 306,
            bounds: [27, 29, 60, 37],
        },
        scissor: Shape {
            pixels: 1239,
            bounds: [21, 0, 60, 41],
        },
        sha256: "c5aa2d5a1b57db7700e9ceaffad4b818a5ace1b0e61698d79032f45412501e9b",
    },
];

fn classify(capture: &Rt64PresentedPixels, x: u32, y: u32) -> Option<usize> {
    let offset = (y * capture.row_bytes + x * 4) as usize;
    let blue = capture.bytes[offset];
    let green = capture.bytes[offset + 1];
    let red = capture.bytes[offset + 2];
    if red > 160 && green < 96 && blue < 96 {
        Some(0)
    } else if green > 128 && red < 96 && blue < 96 {
        Some(1)
    } else if blue > 128 && red < 96 && green < 96 {
        Some(2)
    } else {
        None
    }
}

fn inspect_shape(capture: &Rt64PresentedPixels, channel: usize) -> Result<Shape, Box<dyn Error>> {
    let mut pixels = 0;
    let mut bounds = [capture.width, capture.height, 0, 0];
    for y in 0..capture.height {
        for x in 0..capture.width {
            if classify(capture, x, y) == Some(channel) {
                pixels += 1;
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
    }
    if pixels == 0 {
        return Err(
            io::Error::other(format!("aspect HLE fixture lost color channel {channel}")).into(),
        );
    }
    Ok(Shape { pixels, bounds })
}

fn validate_extended(evidence: &Rt64ExtendedGbiEvidence) -> Result<(), Box<dyn Error>> {
    if evidence.workload_id == 0
        || evidence.present_id == 0
        || evidence.enabled_opcode != Some(0x64)
        || evidence.hook_enable_count != 1
        || evidence.command_counts[0x33] != 1
        || evidence.command_counts[0x06] != 1
        || evidence.rects.len() < 2
    {
        return Err(
            io::Error::other(format!("incomplete Extended aspect evidence: {evidence:?}")).into(),
        );
    }
    let aligned = evidence
        .rects
        .last()
        .expect("at least the clear and aligned 2D rectangle");
    if aligned.aspect_mode != Rt64ExtendedAspectMode::Adjust
        || aligned.left_origin != 0
        || aligned.right_origin != 0x400
        || aligned.left_offset != 6
        || aligned.right_offset != 6
        || aligned.upper_left_x != 38
        || aligned.upper_left_y != 124
        || aligned.lower_right_x != 485
        || aligned.lower_right_y != 163
    {
        return Err(io::Error::other(format!(
            "explicit 2D origin/aspect evidence drifted: {aligned:?}"
        ))
        .into());
    }
    Ok(())
}

fn render(
    backend: &mut Rt64Backend,
    version: Version1,
    policy_sha256: [u8; 32],
    noise_seed: u64,
) -> Result<Capture, Box<dyn Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    let end = write_scene(&mut rdram, version);
    let target = TARGET;
    rdram[target - 4..target].copy_from_slice(&GUARD.to_ne_bytes());
    rdram[target + TARGET_BYTES..target + TARGET_BYTES + 4].copy_from_slice(&GUARD.to_ne_bytes());
    backend.enable_extended_gbi_evidence()?;
    backend.process_synthetic_extended_f3dex2(&mut rdram, DISPLAY_LIST as u32, TARGET as u32)?;
    if end > DISPLAY_LIST + 0x200 {
        return Err(io::Error::other(
            "aspect HLE display list exceeded its bounded fixture region",
        )
        .into());
    }
    backend.present(ViPresentation {
        noise_seed,
        filters: ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        },
        ..ViPresentation::default()
    })?;
    if u32::from_ne_bytes(rdram[target - 4..target].try_into()?) != GUARD
        || u32::from_ne_bytes(rdram[target + TARGET_BYTES..target + TARGET_BYTES + 4].try_into()?)
            != GUARD
    {
        return Err(io::Error::other("aspect HLE target guard changed").into());
    }
    let pixels = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    let evidence = backend.extended_gbi_evidence()?;
    let extended_pixels = backend.extended_presented_pixels()?;
    validate_extended(&evidence)?;
    if pixels.width != PRESENT_WIDTH
        || pixels.height != PRESENT_HEIGHT
        || pixels.row_bytes != PRESENT_WIDTH * 4
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
        || selection.present_id != pixels.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != TARGET as u32
        || selection.target_width != SOURCE_WIDTH
        || selection.target_height != 45
        || selection.target_size != 2
        || evidence.present_id != pixels.present_id
        || extended_pixels.len() != 1
        || extended_pixels[0].present_id != pixels.present_id
        || extended_pixels[0].workload_id != evidence.workload_id
        || extended_pixels[0].bytes != pixels.bytes
    {
        return Err(io::Error::other(format!(
            "aspect HLE workload/post-VI association drifted: pixels={{id:{},{}x{},row:{},format:{:?}}}, selection={selection:?}, evidence={{workload:{},present:{}}}, extended={:?}",
            pixels.present_id,
            pixels.width,
            pixels.height,
            pixels.row_bytes,
            pixels.format,
            evidence.workload_id,
            evidence.present_id,
            extended_pixels
                .iter()
                .map(|capture| (capture.workload_id, capture.present_id, capture.width, capture.height, capture.row_bytes, capture.format, capture.bytes == pixels.bytes))
                .collect::<Vec<_>>(),
        ))
        .into());
    }
    Ok(Capture {
        present_id: pixels.present_id,
        workload_id: evidence.workload_id,
        triangle: inspect_shape(&pixels, 0)?,
        rectangle: inspect_shape(&pixels, 1)?,
        scissor: inspect_shape(&pixels, 2)?,
        sha256: Sha256::digest(&pixels.bytes).into(),
        policy_sha256,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn shape_width(shape: Shape) -> u32 {
    shape.bounds[2] - shape.bounds[0] + 1
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.source_overlay_id != PINNED_OVERLAY
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "aspect HLE evidence requires clean pinned Metal RT64: {identity:?}"
        ))
        .into());
    }

    let cases = [
        (
            "original-4:3",
            settings(RenderAspectRatio::Original, 4.0 / 3.0)?,
        ),
        (
            "manual-16:9",
            settings(RenderAspectRatio::Manual, 16.0 / 9.0)?,
        ),
        ("manual-2:1", settings(RenderAspectRatio::Manual, 2.0)?),
        (
            "manual-21:9",
            settings(RenderAspectRatio::Manual, 21.0 / 9.0)?,
        ),
    ];
    let mut backend = Rt64Backend::new().with_runtime_settings(cases[0].1.clone());
    backend.create(&RenderConfig::new(PRESENT_WIDTH, PRESENT_HEIGHT))?;
    backend.enable_present_capture()?;
    require_production_rejection(&mut backend)?;
    let version = negotiate_v1(&mut backend)?;

    let mut captures = Vec::new();
    for (index, (label, requested)) in cases.iter().enumerate() {
        let policy_sha256 = if index == 0 {
            active_policy_sha256(&backend, requested)?
        } else {
            let outcome = backend.apply_runtime_settings(requested)?;
            let expected = RenderSettingsApply::LiveApplied {
                settings_sha256: requested.sha256(),
                framebuffers_discarded: true,
            };
            if outcome != expected {
                return Err(io::Error::other(format!(
                    "{label} did not take the live framebuffer-discard path: {outcome:?}"
                ))
                .into());
            }
            active_policy_sha256(&backend, requested)?
        };
        let transition = render(&mut backend, version, policy_sha256, index as u64 * 2 + 1)?;
        let stable = render(&mut backend, version, policy_sha256, index as u64 * 2 + 2)?;
        if transition.triangle != stable.triangle
            || transition.rectangle != stable.rectangle
            || transition.scissor != stable.scissor
            || transition.sha256 != stable.sha256
        {
            return Err(io::Error::other(format!(
                "{label} did not stabilize after its live transition: transition={transition:?}, stable={stable:?}"
            ))
            .into());
        }
        let expected = EXPECTED[index];
        if stable.triangle != expected.triangle
            || stable.rectangle != expected.rectangle
            || stable.scissor != expected.scissor
            || hex(&stable.sha256) != expected.sha256
        {
            return Err(io::Error::other(format!(
                "{label} exact post-VI output drifted: actual={stable:?}, expected={{triangle:{:?},rectangle:{:?},scissor:{:?},sha256:{}}}",
                expected.triangle,
                expected.rectangle,
                expected.scissor,
                expected.sha256,
            ))
            .into());
        }
        println!(
            "phase={label} present={} workload={} triangle={:?} rectangle={:?} scissor={:?} policy_sha256={} output_sha256={}",
            stable.present_id,
            stable.workload_id,
            stable.triangle,
            stable.rectangle,
            stable.scissor,
            hex(&stable.policy_sha256),
            hex(&stable.sha256),
        );
        captures.push(stable);
    }
    require_production_rejection(&mut backend)?;
    if !captures.windows(2).all(|pair| {
        pair[0].present_id < pair[1].present_id
            && pair[0].workload_id < pair[1].workload_id
            && pair[0].policy_sha256 != pair[1].policy_sha256
            && pair[0].sha256 != pair[1].sha256
    }) {
        return Err(io::Error::other(
            "aspect HLE cases did not advance or distinguish policy/workload/output identity",
        )
        .into());
    }
    let native_triangle_width = shape_width(captures[0].triangle);
    let native_rectangle_width = shape_width(captures[0].rectangle);
    if captures[1..].iter().any(|capture| {
        shape_width(capture.triangle) * native_rectangle_width
            == shape_width(capture.rectangle) * native_triangle_width
    }) {
        return Err(io::Error::other(
            "aspect HLE fixture reduced transformed 3D and explicit 2D alignment to one horizontal scale",
        )
        .into());
    }
    Ok(())
}
