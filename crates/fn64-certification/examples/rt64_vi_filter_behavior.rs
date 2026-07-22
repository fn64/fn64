use std::collections::BTreeSet;
use std::error::Error;
use std::io;

use fn64_render::{
    vi_public_filters::{
        gamma_dither_quantize_bounded_v1, reference_noise_bit_v1,
        restore_rgba16_component_bounded_v1,
    },
    ActiveRenderGraphicsApi, FrameStatus, ReleaseCaptureFormat, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings, RenderSettingsApply, ViPresentation,
    ViScaleAxis, ViScanoutRegisters, ViScanoutState,
};
use fn64_render_rt64::{Rt64Backend, Rt64PresentedPixels, Rt64SourceProvenance};
use fn64_runtime::TvType;
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: u32 = 0x4000;
const SOURCE_WIDTH: u32 = OUTPUT_WIDTH;
const SOURCE_HEIGHT: u32 = 10;
const OUTPUT_WIDTH: u32 = 8;
const OUTPUT_HEIGHT: u32 = 6;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const BASELINE_SHA256: &str = "c036fc22d5dd5eead9a44d88e3b3cf26c84056e498afcd2c4ccac91cff4e76fc";
const GAMMA_SHA256: &str = "213c4260c0b436ac5baf0418491f6004f4f9c35e6d9a01a9ee903f990eadc6d1";
const DITHER_ONLY_A_SHA256: &str =
    "902f1c66244b60396cb6f5b384f81635b9290a3faf7849761a33f81e45b21905";
const DITHER_ONLY_B_SHA256: &str =
    "5897abb814b071b47de0d75bae65b20a6b35c07228c4b0b4301079ec53c39475";
const GAMMA_DITHER_A_SHA256: &str =
    "d86ce308a87a7cfc8a0310b42116481e57eb09e3d76ae6c4e29ebf2e0f0e25ff";
const GAMMA_DITHER_B_SHA256: &str =
    "3525043c69328be735eb7eee82eced977e3181ad9ddf1b92ff1aaa23f60e6cd7";
const DIVOT_SHA256: &str = "2808f2fda9c324349b11a690540aeda16749851efbddb362e388870bd4c0310a";
const DITHER_FILTER_SHA256: &str =
    "884a1a2fdb0497d10bd8c77b094bc1f706c851a8e4529d90ae3f830f6ee87fe9";
const SCALED_SHA256: &str = "3aa430084d57b5cf4811a43367c215eda4eb3ac8b38451e75a961826c410bf46";
const RGB5: [[u8; 3]; SOURCE_WIDTH as usize] = [
    [2, 3, 1],
    [2, 3, 1],
    [2, 3, 1],
    [8, 9, 10],
    [11, 7, 12],
    [8, 13, 6],
    [12, 5, 14],
    [9, 15, 4],
];

const STATUS_RGBA16_AA_ALWAYS: u32 = 0x002;
const STATUS_RGBA16_AA_NEEDED: u32 = 0x102;
const STATUS_RGBA16_RESAMPLE: u32 = 0x202;
const STATUS_RGBA16_REPLICATE: u32 = 0x302;
const STATUS_GAMMA: u32 = 1 << 3;
const STATUS_GAMMA_DITHER: u32 = 1 << 2;
const STATUS_DIVOT: u32 = 1 << 4;
const STATUS_DITHER_FILTER: u32 = 1 << 16;
const CVG_DST_CLAMP: u32 = 0;
const CVG_DST_SAVE: u32 = 0x300;
const FULL_COVERAGE_SOURCE_ROWS: u32 = 4;
const EXPECTED_DIVOT_CHANGED_PIXELS: usize = 12;
const EXPECTED_RESTORATION_CHANGED_PIXELS: usize = 18;

#[derive(Clone, Copy, Debug)]
struct Phase {
    label: &'static str,
    status: u32,
    x_scale: u32,
    y_scale: u32,
    cycle: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    label: &'static str,
    workload_id: u64,
    present_id: u64,
    width: u32,
    height: u32,
    row_bytes: u32,
    sha256: String,
    nonblack_pixels: usize,
    unique_colors: usize,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViFilterRunSummary {
    pub workload_id: u64,
    pub first_present_id: u64,
    pub last_present_id: u64,
}

fn push_command(rdram: &mut [u8], cursor: &mut usize, w0: u32, w1: u32) {
    rdram[*cursor..*cursor + 4].copy_from_slice(&w0.to_ne_bytes());
    rdram[*cursor + 4..*cursor + 8].copy_from_slice(&w1.to_ne_bytes());
    *cursor += 8;
}

fn rgba16(x: u32, full_coverage: bool) -> u16 {
    let [red, green, blue] = RGB5[x as usize].map(u32::from);
    ((red << 11) | (green << 6) | (blue << 1) | u32::from(full_coverage)) as u16
}

fn fixture() -> (Vec<u8>, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    let mut cursor = COMMANDS;
    push_command(
        &mut rdram,
        &mut cursor,
        0xff10_0000 | (SOURCE_WIDTH - 1),
        TARGET,
    );

    for y in 0..SOURCE_HEIGHT {
        // The visible upper half is an otherwise identical full-coverage
        // control; the lower half preserves the clear target alpha as
        // non-full coverage. This proves the VI stage gates on RT64's retained
        // coverage estimate without pretending host edge samples are N64
        // coverage.
        let full_coverage = y < FULL_COVERAGE_SOURCE_ROWS;
        let cvg_dst = if full_coverage {
            CVG_DST_CLAMP
        } else {
            CVG_DST_SAVE
        };
        push_command(&mut rdram, &mut cursor, 0xef00_0000 | (3 << 20), cvg_dst);
        for x in 0..SOURCE_WIDTH {
            let color = u32::from(rgba16(x, full_coverage));
            push_command(&mut rdram, &mut cursor, 0xf700_0000, (color << 16) | color);
            push_command(
                &mut rdram,
                &mut cursor,
                0xf600_0000 | ((x * 4) << 12) | (y * 4),
                ((x * 4) << 12) | (y * 4),
            );
        }
    }

    push_command(&mut rdram, &mut cursor, 0xe900_0000, 0);

    (rdram, cursor as u32)
}

fn presentation(phase: Phase) -> ViPresentation {
    let mut words = [0u32; ViScanoutRegisters::WORD_COUNT];
    words[0] = phase.status;
    words[1] = TARGET;
    words[2] = SOURCE_WIDTH;
    words[6] = 525;
    words[7] = 3093;
    words[9] = (121 << 16) | (121 + OUTPUT_WIDTH);
    words[10] = (37 << 16) | (37 + OUTPUT_HEIGHT * 2);
    words[12] = phase.x_scale;
    words[13] = phase.y_scale;
    ViPresentation {
        noise_seed: phase.cycle,
        scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
        ..ViPresentation::default()
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn expected_gamma_dither(source: &[u8], noise_seed: u64) -> Vec<u8> {
    let mut expected = source.to_vec();
    for (pixel_index, bgra) in expected.chunks_exact_mut(4).enumerate() {
        for (byte_index, rgb_channel) in [(2, 0), (1, 1), (0, 2)] {
            bgra[byte_index] = gamma_dither_quantize_bounded_v1(
                bgra[byte_index],
                reference_noise_bit_v1(noise_seed, pixel_index as u64, rgb_channel),
            );
        }
    }
    expected
}

fn median(left: u8, center: u8, right: u8) -> u8 {
    let mut values = [left, center, right];
    values.sort_unstable();
    values[1]
}

fn validate_divot_median(baseline: &[u8], divot: &[u8], width: u32, height: u32) -> usize {
    assert_eq!(baseline.len(), divot.len());
    let width = width as usize;
    let height = height as usize;
    let mut changed = 0;
    for y in 0..height {
        for x in 0..width {
            let center = (y * width + x) * 4;
            let changed_pixel = divot[center..center + 4] != baseline[center..center + 4];
            if x == 0 || x + 1 == width || y < height / 2 {
                assert_eq!(
                    &divot[center..center + 4],
                    &baseline[center..center + 4],
                    "native divot changed border/full-coverage control pixel ({x}, {y})"
                );
            } else {
                let left = center - 4;
                let right = center + 4;
                let mut expected = baseline[center..center + 4].to_vec();
                for channel in 0..3 {
                    expected[channel] = median(
                        baseline[left + channel],
                        baseline[center + channel],
                        baseline[right + channel],
                    );
                }
                assert_eq!(
                    &divot[center..center + 4],
                    expected,
                    "native partial-coverage divot mismatch at ({x}, {y})"
                );
            }
            assert_eq!(
                divot[center + 3],
                baseline[center + 3],
                "native divot changed alpha at ({x}, {y})"
            );
            changed += usize::from(changed_pixel);
        }
    }
    assert_eq!(
        changed, EXPECTED_DIVOT_CHANGED_PIXELS,
        "native divot changed-pixel count drifted"
    );
    changed
}

fn validate_dither_restoration(baseline: &[u8], restored: &[u8], width: u32, height: u32) -> usize {
    assert_eq!(baseline.len(), restored.len());
    let width = width as usize;
    let height = height as usize;
    assert_eq!(width, RGB5.len());
    let mut changed = 0;
    let mut partial_controls = 0;
    let mut flat_full_controls = 0;
    for y in 0..height {
        // Pinned Metal nearest filtering reconstructs this integer source
        // lattice for restoration even though presentation owns a separate
        // progressive origin bias.
        let source_y = y;
        let full_coverage = y < height / 2;
        for (x, center_rgb5) in RGB5.iter().enumerate().take(width) {
            let pixel = (y * width + x) * 4;
            let mut expected = baseline[pixel..pixel + 4].to_vec();
            for (bgra_channel, rgb5_channel) in [(0, 2), (1, 1), (2, 0)] {
                let center = center_rgb5[rgb5_channel];
                assert_eq!(
                    baseline[pixel + bgra_channel] >> 3,
                    center,
                    "baseline lost declared RGB5 source at ({x}, {y})"
                );
                if !full_coverage {
                    continue;
                }
                let mut neighbors = Vec::with_capacity(8);
                for neighbor_y in
                    source_y.saturating_sub(1)..=(source_y + 1).min(SOURCE_HEIGHT as usize - 1)
                {
                    let first_x = x.saturating_sub(1);
                    let last_x = (x + 1).min(SOURCE_WIDTH as usize - 1);
                    for (neighbor_x, neighbor_rgb5) in
                        RGB5.iter().enumerate().take(last_x + 1).skip(first_x)
                    {
                        if neighbor_x == x && neighbor_y == source_y {
                            continue;
                        }
                        neighbors.push(neighbor_rgb5[rgb5_channel]);
                    }
                }
                expected[bgra_channel] = restore_rgba16_component_bounded_v1(center, &neighbors);
            }
            assert_eq!(
                &restored[pixel..pixel + 4],
                expected,
                "native RGBA16 restoration mismatch at ({x}, {y})"
            );
            assert_eq!(
                restored[pixel + 3],
                baseline[pixel + 3],
                "native RGBA16 restoration changed alpha at ({x}, {y})"
            );
            if !full_coverage {
                partial_controls += 1;
                assert_eq!(
                    &restored[pixel..pixel + 4],
                    &baseline[pixel..pixel + 4],
                    "native restoration changed non-full control ({x}, {y})"
                );
            } else if x < 2 {
                flat_full_controls += 1;
                assert_eq!(
                    &restored[pixel..pixel + 4],
                    &baseline[pixel..pixel + 4],
                    "native restoration changed flat full control ({x}, {y})"
                );
            }
            changed += usize::from(restored[pixel..pixel + 4] != baseline[pixel..pixel + 4]);
        }
    }
    assert_eq!(partial_controls, 24);
    assert_eq!(flat_full_controls, 6);
    assert_eq!(
        changed, EXPECTED_RESTORATION_CHANGED_PIXELS,
        "native RGBA16 restoration changed-pixel count drifted"
    );
    changed
}

fn observe(
    backend: &mut Rt64Backend,
    rdram: &[u8],
    phase: Phase,
) -> Result<Observation, Box<dyn Error>> {
    backend.present_live(rdram, presentation(phase))?;
    let Rt64PresentedPixels {
        width,
        height,
        row_bytes,
        format,
        graphics_api,
        present_id,
        workload_id,
        bytes,
    } = backend.presented_pixels()?;
    if width != OUTPUT_WIDTH
        || height != OUTPUT_HEIGHT
        || row_bytes != OUTPUT_WIDTH * 4
        || format != fn64_render_rt64::Rt64PresentPixelFormat::Bgra8Unorm
        || graphics_api != ActiveRenderGraphicsApi::Metal
        || bytes.len() != (row_bytes * height) as usize
        || workload_id == 0
        || present_id == 0
    {
        return Err(io::Error::other(format!(
            "{} returned invalid native post-VI metadata: {}x{} row={} format={format:?} api={graphics_api:?} workload={workload_id} present={present_id} bytes={}",
            phase.label,
            width,
            height,
            row_bytes,
            bytes.len()
        ))
        .into());
    }
    let nonblack_pixels = bytes
        .chunks_exact(4)
        .filter(|pixel| pixel[..3] != [0, 0, 0])
        .count();
    let unique_colors = bytes
        .chunks_exact(4)
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect::<BTreeSet<_>>()
        .len();
    if nonblack_pixels == 0 || unique_colors < 4 {
        return Err(io::Error::other(format!(
            "{} did not preserve the asymmetric source: nonblack={nonblack_pixels} unique={unique_colors}",
            phase.label
        ))
        .into());
    }
    Ok(Observation {
        label: phase.label,
        workload_id,
        present_id,
        width,
        height,
        row_bytes,
        sha256: digest(&bytes),
        nonblack_pixels,
        unique_colors,
        bytes,
    })
}

fn settings() -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        ..RenderRuntimeSettings::default()
    }
}

fn run_created(backend: &mut Rt64Backend) -> Result<ViFilterRunSummary, Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "native VI evidence requires clean pinned Metal RT64: {identity:?}"
        ))
        .into());
    }
    backend.resize(OUTPUT_WIDTH, OUTPUT_HEIGHT);
    backend.enable_present_capture()?;

    let (mut rdram, command_end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, command_end, TARGET)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "native VI fixture returned {status:?} instead of Complete"
        ))
        .into());
    }

    let one = u32::from(ViScaleAxis::ONE);
    let three_halves = one + one / 2;
    let phases = [
        Phase {
            label: "baseline-a",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 100,
        },
        Phase {
            label: "dither-only-a",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 101,
        },
        Phase {
            label: "dither-only-a-repeat",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 101,
        },
        Phase {
            label: "dither-only-b",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 102,
        },
        Phase {
            label: "baseline-b",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 103,
        },
        Phase {
            label: "gamma",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA,
            x_scale: one,
            y_scale: one,
            cycle: 104,
        },
        Phase {
            label: "gamma-dither-a",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 105,
        },
        Phase {
            label: "gamma-dither-a-repeat",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 105,
        },
        Phase {
            label: "gamma-dither-b",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 106,
        },
        Phase {
            label: "gamma-restore",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA,
            x_scale: one,
            y_scale: one,
            cycle: 107,
        },
        Phase {
            label: "baseline-c",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 108,
        },
        Phase {
            label: "divot",
            status: STATUS_RGBA16_REPLICATE | STATUS_DIVOT,
            x_scale: one,
            y_scale: one,
            cycle: 109,
        },
        Phase {
            label: "baseline-d",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 110,
        },
        Phase {
            label: "dither-filter",
            status: STATUS_RGBA16_REPLICATE | STATUS_DITHER_FILTER,
            x_scale: one,
            y_scale: one,
            cycle: 111,
        },
        Phase {
            label: "baseline-e",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 112,
        },
        Phase {
            label: "resample-mode-3",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 113,
        },
        Phase {
            label: "resample-mode-2",
            status: STATUS_RGBA16_RESAMPLE,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 114,
        },
        Phase {
            label: "resample-mode-1",
            status: STATUS_RGBA16_AA_NEEDED,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 115,
        },
        Phase {
            label: "resample-mode-0",
            status: STATUS_RGBA16_AA_ALWAYS,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 116,
        },
        Phase {
            label: "baseline-f",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 117,
        },
    ];

    let mut observations = Vec::with_capacity(phases.len());
    for phase in phases {
        observations.push(observe(backend, &rdram, phase)?);
    }
    for pair in observations.windows(2) {
        if pair[1].workload_id != pair[0].workload_id || pair[1].present_id <= pair[0].present_id {
            return Err(io::Error::other(format!(
                "native VI phase identity drifted between {} and {}",
                pair[0].label, pair[1].label
            ))
            .into());
        }
    }

    let by_label = |label: &str| {
        observations
            .iter()
            .find(|observation| observation.label == label)
            .unwrap_or_else(|| panic!("missing native VI observation {label}"))
    };
    for observation in &observations {
        println!(
            "vi_filter_phase label={} sha256={} nonblack={} unique={} workload_id={} present_id={}",
            observation.label,
            observation.sha256,
            observation.nonblack_pixels,
            observation.unique_colors,
            observation.workload_id,
            observation.present_id
        );
    }
    for label in [
        "baseline-a",
        "baseline-b",
        "baseline-c",
        "baseline-d",
        "baseline-e",
        "baseline-f",
    ] {
        let observation = by_label(label);
        if observation.sha256 != BASELINE_SHA256
            || observation.nonblack_pixels != 48
            || observation.unique_colors != 6
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned baseline: {observation:?}"
            ))
            .into());
        }
    }
    let divot = by_label("divot");
    if divot.sha256 != DIVOT_SHA256 || divot.nonblack_pixels != 48 || divot.unique_colors != 8 {
        return Err(io::Error::other(format!(
            "divot drifted from the exact pinned median result: {divot:?}"
        ))
        .into());
    }
    let divot_changed_pixels = validate_divot_median(
        &by_label("baseline-a").bytes,
        &divot.bytes,
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
    );
    let dither_filter = by_label("dither-filter");
    let restoration_changed_pixels = validate_dither_restoration(
        &by_label("baseline-d").bytes,
        &dither_filter.bytes,
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
    );
    if dither_filter.sha256 != DITHER_FILTER_SHA256
        || dither_filter.nonblack_pixels != 48
        || dither_filter.unique_colors != 18
    {
        return Err(io::Error::other(format!(
            "dither-filter drifted from the exact pinned restoration result: {dither_filter:?}"
        ))
        .into());
    }
    for label in ["gamma", "gamma-restore"] {
        let observation = by_label(label);
        if observation.sha256 != GAMMA_SHA256
            || observation.nonblack_pixels != 48
            || observation.unique_colors != 6
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned gamma result: {observation:?}"
            ))
            .into());
        }
    }
    for (label, expected, unique_colors) in [
        ("dither-only-a", DITHER_ONLY_A_SHA256, 15),
        ("dither-only-a-repeat", DITHER_ONLY_A_SHA256, 15),
        ("dither-only-b", DITHER_ONLY_B_SHA256, 15),
        ("gamma-dither-a", GAMMA_DITHER_A_SHA256, 17),
        ("gamma-dither-a-repeat", GAMMA_DITHER_A_SHA256, 17),
        ("gamma-dither-b", GAMMA_DITHER_B_SHA256, 17),
    ] {
        let observation = by_label(label);
        if observation.sha256 != expected
            || observation.nonblack_pixels != 48
            || observation.unique_colors != unique_colors
        {
            return Err(io::Error::other(format!(
                "{label} drifted from its exact pinned gamma-dither result: {observation:?}"
            ))
            .into());
        }
    }
    for label in [
        "resample-mode-3",
        "resample-mode-2",
        "resample-mode-1",
        "resample-mode-0",
    ] {
        let observation = by_label(label);
        if observation.sha256 != SCALED_SHA256
            || observation.nonblack_pixels != 40
            || observation.unique_colors != 7
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned scaled result: {observation:?}"
            ))
            .into());
        }
    }
    if by_label("baseline-a").bytes == by_label("gamma").bytes
        || by_label("baseline-a").bytes == by_label("dither-only-a").bytes
        || by_label("dither-only-a").bytes == by_label("dither-only-b").bytes
        || by_label("gamma").bytes == by_label("gamma-dither-a").bytes
        || by_label("gamma-dither-a").bytes == by_label("gamma-dither-b").bytes
        || by_label("baseline-a").bytes == by_label("resample-mode-3").bytes
        || by_label("baseline-a").bytes == by_label("divot").bytes
        || by_label("baseline-d").bytes == by_label("dither-filter").bytes
    {
        return Err(io::Error::other(
            "native gamma, seeded gamma-dither, divot, restoration, and nonidentity scale must each change exact post-VI pixels",
        )
        .into());
    }
    if by_label("dither-only-a").bytes != by_label("dither-only-a-repeat").bytes
        || by_label("gamma-dither-a").bytes != by_label("gamma-dither-a-repeat").bytes
    {
        return Err(io::Error::other(
            "an identical gamma-dither seed did not reproduce exact native post-VI pixels",
        )
        .into());
    }
    for (source_label, dithered_label, noise_seed) in [
        ("baseline-a", "dither-only-a", 101),
        ("baseline-a", "dither-only-b", 102),
        ("gamma", "gamma-dither-a", 105),
        ("gamma", "gamma-dither-b", 106),
    ] {
        let expected = expected_gamma_dither(&by_label(source_label).bytes, noise_seed);
        if by_label(dithered_label).bytes != expected {
            return Err(io::Error::other(format!(
                "{dithered_label} does not match the shared bounded-v1 quantizer over {source_label}"
            ))
            .into());
        }
    }
    if by_label("gamma").bytes != by_label("gamma-restore").bytes
        || by_label("baseline-d").bytes != by_label("baseline-e").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-2").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-1").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-0").bytes
    {
        return Err(
            io::Error::other("pinned RT64's code-0/save scaled negative control changed").into(),
        );
    }

    let release = backend.release_capture()?;
    if release.format != ReleaseCaptureFormat::PostViBgra8Unorm
        || release.width != OUTPUT_WIDTH
        || release.height != OUTPUT_HEIGHT
        || release.guest_cycle != 117
        || release.bytes != observations.last().unwrap().bytes
    {
        return Err(
            io::Error::other("release capture did not bind the final live VI phase").into(),
        );
    }
    println!(
        "vi_filter_pixel_evidence source={} baseline_sha256={} gamma_sha256={} dither_only_a_sha256={} dither_only_b_sha256={} gamma_dither_a_sha256={} gamma_dither_b_sha256={} divot_sha256={} divot_changed_pixels={} dither_filter_sha256={} restoration_changed_pixels={} scaled_sha256={} phases={} aa_selector_evidence=separate-qualified-coverage-gate",
        identity.source_id,
        BASELINE_SHA256,
        GAMMA_SHA256,
        DITHER_ONLY_A_SHA256,
        DITHER_ONLY_B_SHA256,
        GAMMA_DITHER_A_SHA256,
        GAMMA_DITHER_B_SHA256,
        DIVOT_SHA256,
        divot_changed_pixels,
        DITHER_FILTER_SHA256,
        restoration_changed_pixels,
        SCALED_SHA256,
        observations.len()
    );
    Ok(ViFilterRunSummary {
        workload_id: observations[0].workload_id,
        first_present_id: observations[0].present_id,
        last_present_id: observations.last().unwrap().present_id,
    })
}

pub fn run_on_backend(backend: &mut Rt64Backend) -> Result<ViFilterRunSummary, Box<dyn Error>> {
    let expected_settings_sha256 = settings().sha256();
    match backend.apply_runtime_settings(&settings())? {
        RenderSettingsApply::LiveApplied {
            settings_sha256, ..
        } if settings_sha256 == expected_settings_sha256 => {}
        result => {
            return Err(io::Error::other(format!(
                "native VI fixture requires an existing Metal backend that applies exact nearest settings live: {result:?}"
            ))
            .into());
        }
    }
    run_created(backend)
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut backend = Rt64Backend::new();
    backend.apply_runtime_settings(&settings())?;
    backend.create(&RenderConfig::for_tv(
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
        TvType::Ntsc,
    ))?;
    run_created(&mut backend).map(|_| ())
}

#[allow(dead_code)]
fn main() -> Result<(), Box<dyn Error>> {
    run()
}
