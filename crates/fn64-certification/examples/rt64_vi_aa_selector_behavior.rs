use std::collections::BTreeSet;
use std::error::Error;
use std::io;

use fn64_render::{
    ActiveRenderGraphicsApi, FrameStatus, ReleaseCaptureFormat, RenderBackend, RenderConfig,
    RenderFiltering, RenderGraphicsApi, RenderRuntimeSettings, RenderSettingsApply, ViAaMode,
    ViFilterControl, ViPixelType, ViPresentation, ViScaleAxis, ViScanoutRegisters, ViScanoutState,
};
use fn64_render_rt64::{Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels};
use fn64_runtime::TvType;
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: u32 = 0x8000;
const WIDTH: u32 = 32;
const HEIGHT: u32 = 5;
// Pinned RT64's public VI-size heuristic expands this five-line viewport to
// an eight-row source lattice. Populate the complete lattice so nearest
// sampling never relies on an uninitialized border.
const SOURCE_HEIGHT: u32 = 8;
const PARTIALS: [(u32, u32, u8); 6] = [
    (3, 2, 1),
    (8, 2, 2),
    (13, 2, 3),
    (18, 2, 4),
    (23, 2, 5),
    (27, 2, 6),
];
const PATCH_SHIFTS: [[u8; 3]; 6] = [
    [0, 0, 0],
    [0, 1, 0],
    [0, 0, 1],
    [0, 1, 1],
    [1, 0, 0],
    [1, 1, 0],
];
const EXPECTED_AA_RGB8: [[u8; 3]; 6] = [
    [50, 45, 35],
    [76, 60, 53],
    [102, 70, 75],
    [128, 87, 95],
    [158, 95, 109],
    [185, 113, 128],
];
const STATUS_RGBA16_AA_ALWAYS: u32 = 0x002;
const STATUS_RGBA16_AA_NEEDED: u32 = 0x102;
const STATUS_RGBA16_RESAMPLE: u32 = 0x202;
const STATUS_RGBA16_REPLICATE: u32 = 0x302;
const STATUS_DIVOT: u32 = 1 << 4;
const CVG_DST_CLAMP: u32 = 0;
const CVG_X_ALPHA: u32 = 0x1000;

const BASELINE_SHA256: &str = "6639c251163aa9dc6d660abf9da11a20bf29222b5d6d16ba0743f599e0666730";
const AA_SHA256: &str = "83cf93557a7ad54d2a3d6badee86664b07f3df46383b28e16393a032ca9895f9";
const DIVOT_SHA256: &str = "8220a101f0de0ffdcefef798c2cec0fd46d3ff653de584a062c8fa86785e1801";
const AA_DIVOT_SHA256: &str = "af2739c8bb26869cafbf62f62f52e343b61a38addb0df31aba3e09b5f4bda17b";

#[derive(Clone, Copy, Debug)]
struct Phase {
    label: &'static str,
    scanout: PhaseScanout,
    cycle: u64,
}

#[derive(Clone, Copy, Debug)]
enum PhaseScanout {
    Registers(u32),
    BackendOnly(ViAaMode),
}

#[derive(Clone, Debug)]
struct Observation {
    label: &'static str,
    workload_id: u64,
    present_id: u64,
    sha256: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViAaSelectorRunSummary {
    pub workload_id: u64,
    pub first_present_id: u64,
    pub last_present_id: u64,
}

fn rgb5(x: u32, y: u32) -> [u8; 3] {
    const PATCH: [((i32, i32), [u8; 3]); 9] = [
        ((0, 0), [28, 16, 20]),
        ((-1, -1), [3, 4, 2]),
        ((1, -1), [3, 4, 2]),
        ((-2, 0), [3, 4, 2]),
        ((2, 0), [3, 4, 2]),
        ((-1, 1), [3, 4, 2]),
        ((1, 1), [3, 4, 2]),
        ((-1, 0), [10, 6, 8]),
        ((1, 0), [22, 13, 16]),
    ];
    for (index, &(center_x, center_y, _)) in PARTIALS.iter().enumerate() {
        let relative = (x as i32 - center_x as i32, y as i32 - center_y as i32);
        if let Some((_, color)) = PATCH.iter().find(|(offset, _)| *offset == relative) {
            let shift = if relative.1 == 0 && relative.0.abs() <= 1 {
                PATCH_SHIFTS[index]
            } else {
                [0, 0, 0]
            };
            return [
                color[0] + shift[0],
                color[1] + shift[1],
                color[2] + shift[2],
            ];
        }
    }
    [2 + y as u8, 3 + y as u8, 1 + y as u8]
}

fn coverage_code(x: u32, y: u32) -> u8 {
    PARTIALS
        .iter()
        .find_map(|&(center_x, center_y, coverage)| {
            ((x, y) == (center_x, center_y)).then_some(coverage)
        })
        .unwrap_or(7)
}

fn expand5(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

fn rgba16(color: [u8; 3]) -> u16 {
    (u16::from(color[0]) << 11) | (u16::from(color[1]) << 6) | (u16::from(color[2]) << 1) | 1
}

fn push(commands: &mut Vec<(u32, u32)>, word0: u32, word1: u32) {
    commands.push((word0, word1));
}

fn fixture() -> (Vec<u8>, u32) {
    assert_eq!(
        PARTIALS
            .iter()
            .map(|&(_, _, coverage)| coverage)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2, 3, 4, 5, 6]),
        "fixture must cover every qualified managed code exactly once"
    );
    let mut commands = Vec::new();
    push(&mut commands, 0xff10_0000 | (WIDTH - 1), TARGET);
    push(
        &mut commands,
        0xed00_0000,
        ((WIDTH * 4) << 12) | (SOURCE_HEIGHT * 4),
    );
    push(&mut commands, 0xef30_00f0, CVG_DST_CLAMP); // Fill cycle, dither off.
    for y in 0..SOURCE_HEIGHT {
        for x in 0..WIDTH {
            let color = u32::from(rgba16(rgb5(x, y)));
            push(&mut commands, 0xf700_0000, (color << 16) | color);
            push(
                &mut commands,
                0xf600_0000 | ((x * 4) << 12) | (y * 4),
                ((x * 4) << 12) | (y * 4),
            );
        }
    }
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xfcff_ffff, 0xfffd_f6fb); // PRIMITIVE RGBA.
    push(&mut commands, 0xef00_00f0, CVG_X_ALPHA | CVG_DST_CLAMP);
    push(
        &mut commands,
        0xed00_0000,
        ((WIDTH * 4) << 12) | (SOURCE_HEIGHT * 4),
    );
    for (x, y, coverage) in PARTIALS {
        let [red, green, blue] = rgb5(x, y).map(expand5);
        push(
            &mut commands,
            0xfa00_0000,
            (u32::from(red) << 24)
                | (u32::from(green) << 16)
                | (u32::from(blue) << 8)
                | u32::from(coverage * 0x20),
        );
        // One-cycle rectangles use an exclusive lower-right edge. Keep each
        // alpha-derived managed-coverage probe isolated to one source pixel.
        push(
            &mut commands,
            0xf600_0000 | (((x + 1) * 4) << 12) | ((y + 1) * 4),
            ((x * 4) << 12) | (y * 4),
        );
    }
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xef30_00f0, CVG_DST_CLAMP);
    for y in 0..SOURCE_HEIGHT {
        for x in 0..WIDTH {
            if coverage_code(x, y) != 7 {
                continue;
            }
            let color = u32::from(rgba16(rgb5(x, y)));
            push(&mut commands, 0xf700_0000, (color << 16) | color);
            push(
                &mut commands,
                0xf600_0000 | ((x * 4) << 12) | (y * 4),
                ((x * 4) << 12) | (y * 4),
            );
        }
    }
    push(&mut commands, 0xe900_0000, 0);

    let mut rdram = vec![0; RDRAM_LEN];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    let end = rdram[COMMANDS..]
        .chunks_exact(8)
        .position(|command| u32::from_ne_bytes(command[..4].try_into().unwrap()) == 0xe900_0000)
        .expect("fixture has FullSync");
    (rdram, (COMMANDS + (end + 1) * 8) as u32)
}

fn presentation(phase: Phase) -> ViPresentation {
    if let PhaseScanout::BackendOnly(antialias_mode) = phase.scanout {
        return ViPresentation {
            noise_seed: phase.cycle,
            scanout: ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                antialias_mode,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        };
    }
    let PhaseScanout::Registers(status) = phase.scanout else {
        unreachable!()
    };
    let mut words = [0; ViScanoutRegisters::WORD_COUNT];
    words[0] = status;
    words[1] = TARGET;
    words[2] = WIDTH;
    words[6] = 525;
    words[7] = 3093;
    words[9] = (121 << 16) | (121 + WIDTH);
    words[10] = (37 << 16) | (37 + HEIGHT * 2);
    words[12] = u32::from(ViScaleAxis::ONE);
    words[13] = u32::from(ViScaleAxis::ONE);
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

fn pixel(bytes: &[u8], x: u32, y: u32) -> &[u8] {
    let start = ((y * WIDTH + x) * 4) as usize;
    &bytes[start..start + 4]
}

fn validate_projected_lattice(baseline: &[u8]) -> Vec<(u32, u32, u8)> {
    assert_eq!(baseline.len(), (WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let observed = pixel(baseline, x, y);
            assert_eq!(observed[3], 0xff, "post-VI alpha drifted at ({x}, {y})");
        }
    }
    let mut projected = Vec::new();
    for (source_x, source_y, coverage) in PARTIALS {
        let center = rgb5(source_x, source_y);
        let matches = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let observed = pixel(baseline, x, y);
                [observed[2] >> 3, observed[1] >> 3, observed[0] >> 3] == center
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "declared coverage-{coverage} foreground did not project exactly once"
        );
        let projected_center = matches[0];
        assert_eq!(
            projected_center.1, 1,
            "coverage-{coverage} vertical projection drifted"
        );
        projected.push((projected_center.0, projected_center.1, coverage));
    }
    projected
}

fn coverage_aa_center_rgb(center: (u32, u32), coverage: u8, foreground: [u8; 3]) -> [u8; 3] {
    const OFFSETS: [(i32, i32); 6] = [(-1, -1), (1, -1), (-2, 0), (2, 0), (-1, 1), (1, 1)];
    let mut neighbors = Vec::new();
    for (delta_x, delta_y) in OFFSETS {
        let neighbor_x = (center.0 as i32 + delta_x) as u32;
        let neighbor_y = (center.1 as i32 + delta_y) as u32;
        assert_eq!(coverage_code(neighbor_x, neighbor_y), 7);
        neighbors.push(rgb5(neighbor_x, neighbor_y).map(expand5));
    }
    assert_eq!(coverage_code(center.0, center.1), coverage);
    let mut filtered = foreground;
    for channel in 0..3 {
        let mut components: Vec<u8> = neighbors.iter().map(|neighbor| neighbor[channel]).collect();
        components.sort_unstable();
        let penultimate_minimum = components[1];
        let penultimate_maximum = components[components.len() - 2];
        let low = foreground[channel].min(penultimate_minimum);
        let high = foreground[channel].max(penultimate_maximum);
        let background = (i16::from(low) + i16::from(high) - i16::from(foreground[channel]))
            .clamp(0, 255) as u16;
        filtered[channel] = ((u16::from(coverage) * u16::from(foreground[channel])
            + u16::from(8 - coverage) * background
            + 4)
            / 8) as u8;
    }
    filtered
}

fn coverage_aa_oracle(
    source: &[u8],
    projected: &[(u32, u32, u8)],
    expected_rgb: Option<&[[u8; 3]; 6]>,
) -> Vec<u8> {
    let mut output = source.to_vec();
    for (
        index,
        (&(source_x, source_y, coverage), &(projected_x, projected_y, projected_coverage)),
    ) in PARTIALS.iter().zip(projected).enumerate()
    {
        assert_eq!(projected_coverage, coverage);
        let raw = pixel(source, projected_x, projected_y);
        let filtered =
            coverage_aa_center_rgb((source_x, source_y), coverage, [raw[2], raw[1], raw[0]]);
        if let Some(expected_rgb) = expected_rgb {
            assert_eq!(
                filtered, expected_rgb[index],
                "coverage-{coverage} Equation-4 oracle drifted"
            );
        }
        let start = ((projected_y * WIDTH + projected_x) * 4) as usize;
        output[start..start + 3].copy_from_slice(&[filtered[2], filtered[1], filtered[0]]);
    }
    output
}

fn median(left: u8, center: u8, right: u8) -> u8 {
    let mut values = [left, center, right];
    values.sort_unstable();
    values[1]
}

fn divot_oracle(source: &[u8], partials: &[(u32, u32, u8)]) -> Vec<u8> {
    let mut output = source.to_vec();
    for (&(source_x, source_y, _), &(projected_x, projected_y, _)) in PARTIALS.iter().zip(partials)
    {
        let left_rgb = rgb5(source_x - 1, source_y).map(expand5);
        let right_rgb = rgb5(source_x + 1, source_y).map(expand5);
        let left = [left_rgb[2], left_rgb[1], left_rgb[0]];
        let right = [right_rgb[2], right_rgb[1], right_rgb[0]];
        let center = pixel(source, projected_x, projected_y);
        let start = ((projected_y * WIDTH + projected_x) * 4) as usize;
        for channel in 0..3 {
            output[start + channel] = median(left[channel], center[channel], right[channel]);
        }
    }
    output
}

fn changed_pixels(left: &[u8], right: &[u8]) -> usize {
    left.chunks_exact(4)
        .zip(right.chunks_exact(4))
        .filter(|(left, right)| left != right)
        .count()
}

fn observe(
    backend: &mut Rt64Backend,
    rdram: &[u8],
    phase: Phase,
) -> Result<Observation, Box<dyn Error>> {
    match phase.scanout {
        PhaseScanout::Registers(_) => backend.present_live(rdram, presentation(phase))?,
        PhaseScanout::BackendOnly(_) => {
            backend.present_physical_compatibility(rdram, presentation(phase))?
        }
    }
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
    if width != WIDTH
        || height != HEIGHT
        || row_bytes != WIDTH * 4
        || format != Rt64PresentPixelFormat::Bgra8Unorm
        || graphics_api != ActiveRenderGraphicsApi::Metal
        || bytes.len() != (row_bytes * height) as usize
        || workload_id == 0
        || present_id == 0
    {
        return Err(io::Error::other(format!(
            "{} returned invalid AA-selector capture metadata",
            phase.label
        ))
        .into());
    }
    Ok(Observation {
        label: phase.label,
        workload_id,
        present_id,
        sha256: digest(&bytes),
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

fn run_created(backend: &mut Rt64Backend) -> Result<ViAaSelectorRunSummary, Box<dyn Error>> {
    backend.resize(WIDTH, HEIGHT);
    backend.enable_present_capture()?;

    let (mut rdram, command_end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, command_end, TARGET, true)?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("native AA-selector fixture returned {status:?}")).into(),
        );
    }

    let phases = [
        Phase {
            label: "mode-3-baseline",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_REPLICATE),
            cycle: 200,
        },
        Phase {
            label: "mode-0-aa",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_AA_ALWAYS),
            cycle: 201,
        },
        Phase {
            label: "mode-1-aa",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_AA_NEEDED),
            cycle: 202,
        },
        Phase {
            label: "mode-2-off",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_RESAMPLE),
            cycle: 203,
        },
        Phase {
            label: "mode-3-off",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_REPLICATE),
            cycle: 204,
        },
        Phase {
            label: "mode-3-divot",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_REPLICATE | STATUS_DIVOT),
            cycle: 205,
        },
        Phase {
            label: "mode-0-aa-divot",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_AA_ALWAYS | STATUS_DIVOT),
            cycle: 206,
        },
        Phase {
            label: "compat-replicate",
            scanout: PhaseScanout::BackendOnly(ViAaMode::Replicate),
            cycle: 207,
        },
        Phase {
            label: "compat-unspecified",
            scanout: PhaseScanout::BackendOnly(ViAaMode::Unspecified),
            cycle: 208,
        },
        Phase {
            label: "compat-mode-0-aa",
            scanout: PhaseScanout::BackendOnly(ViAaMode::AaResampleAlways),
            cycle: 209,
        },
        Phase {
            label: "mode-3-final",
            scanout: PhaseScanout::Registers(STATUS_RGBA16_REPLICATE),
            cycle: 210,
        },
    ];
    let mut observations = Vec::new();
    for phase in phases {
        observations.push(observe(backend, &rdram, phase)?);
    }
    for pair in observations.windows(2) {
        if pair[0].workload_id != pair[1].workload_id || pair[0].present_id >= pair[1].present_id {
            return Err(
                io::Error::other("AA-selector phase identity did not advance exactly").into(),
            );
        }
    }
    let by_label = |label: &str| {
        observations
            .iter()
            .find(|observation| observation.label == label)
            .unwrap_or_else(|| panic!("missing AA-selector observation {label}"))
    };
    let baseline = &by_label("mode-3-baseline").bytes;
    let projected = validate_projected_lattice(baseline);
    let expected_aa = coverage_aa_oracle(baseline, &projected, Some(&EXPECTED_AA_RGB8));
    let expected_divot = divot_oracle(baseline, &projected);
    let expected_aa_divot = divot_oracle(&expected_aa, &projected);
    let reverse_divot_then_aa = coverage_aa_oracle(&expected_divot, &projected, None);
    assert_ne!(
        reverse_divot_then_aa, expected_aa_divot,
        "fixture no longer distinguishes AA-before-divot from the reverse order"
    );

    for label in [
        "mode-3-baseline",
        "mode-2-off",
        "mode-3-off",
        "mode-3-final",
    ] {
        assert_eq!(
            &by_label(label).bytes,
            baseline,
            "{label} did not restore baseline"
        );
    }
    for label in ["mode-0-aa", "mode-1-aa"] {
        assert_eq!(
            by_label(label).bytes,
            expected_aa,
            "{label} disagrees with independent Figure-11 oracle"
        );
    }
    assert_eq!(by_label("mode-3-divot").bytes, expected_divot);
    assert_eq!(by_label("mode-0-aa-divot").bytes, expected_aa_divot);
    let compatibility_baseline = &by_label("compat-replicate").bytes;
    let compatibility_projected = validate_projected_lattice(compatibility_baseline);
    let expected_compatibility_aa = coverage_aa_oracle(
        compatibility_baseline,
        &compatibility_projected,
        Some(&EXPECTED_AA_RGB8),
    );
    assert_eq!(compatibility_baseline, baseline);
    assert_eq!(expected_compatibility_aa, expected_aa);
    assert_eq!(
        by_label("compat-unspecified").bytes,
        *compatibility_baseline
    );
    assert_eq!(
        by_label("compat-mode-0-aa").bytes,
        expected_compatibility_aa
    );
    assert_eq!(changed_pixels(baseline, &expected_aa), 6);
    assert_eq!(changed_pixels(baseline, &expected_divot), 6);
    assert_eq!(changed_pixels(baseline, &expected_aa_divot), 6);
    for expected in [&expected_aa, &expected_divot, &expected_aa_divot] {
        for (left, right) in baseline.chunks_exact(4).zip(expected.chunks_exact(4)) {
            assert_eq!(left[3], right[3], "VI filter oracle changed alpha");
        }
    }

    let release = backend.release_capture()?;
    if release.format != ReleaseCaptureFormat::PostViBgra8Unorm
        || release.guest_cycle != 210
        || release.bytes != *baseline
    {
        return Err(
            io::Error::other("AA-selector release capture lost the final OFF phase").into(),
        );
    }
    let unique = |bytes: &[u8]| bytes.chunks_exact(4).collect::<BTreeSet<_>>().len();
    for observation in &observations {
        println!(
            "vi_aa_selector_phase label={} sha256={} unique={} workload_id={} present_id={}",
            observation.label,
            observation.sha256,
            unique(&observation.bytes),
            observation.workload_id,
            observation.present_id
        );
    }
    for (label, expected) in [
        ("mode-3-baseline", BASELINE_SHA256),
        ("mode-0-aa", AA_SHA256),
        ("mode-3-divot", DIVOT_SHA256),
        ("mode-0-aa-divot", AA_DIVOT_SHA256),
    ] {
        if by_label(label).sha256 != expected {
            return Err(io::Error::other(format!("{label} digest drifted")).into());
        }
    }
    println!(
        "vi_aa_selector_evidence baseline_sha256={} aa_sha256={} divot_sha256={} aa_divot_sha256={} compat_baseline_sha256={} compat_mode0_sha256={} coverage_codes=1-6 aa_changed_pixels=6 divot_changed_pixels=6 aa_divot_changed_pixels=6 phases={} bounded_residual=rt64-managed-coverage",
        by_label("mode-3-baseline").sha256,
        by_label("mode-0-aa").sha256,
        by_label("mode-3-divot").sha256,
        by_label("mode-0-aa-divot").sha256,
        by_label("compat-replicate").sha256,
        by_label("compat-mode-0-aa").sha256,
        observations.len()
    );
    Ok(ViAaSelectorRunSummary {
        workload_id: observations[0].workload_id,
        first_present_id: observations[0].present_id,
        last_present_id: observations.last().unwrap().present_id,
    })
}

pub fn run_on_backend(backend: &mut Rt64Backend) -> Result<ViAaSelectorRunSummary, Box<dyn Error>> {
    let expected_settings_sha256 = settings().sha256();
    match backend.apply_runtime_settings(&settings())? {
        RenderSettingsApply::LiveApplied {
            settings_sha256, ..
        } if settings_sha256 == expected_settings_sha256 => {}
        result => {
            return Err(io::Error::other(format!(
                "native AA-selector fixture requires an existing Metal backend that applies exact nearest settings live: {result:?}"
            ))
            .into());
        }
    }
    run_created(backend)
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut backend = Rt64Backend::new();
    backend.apply_runtime_settings(&settings())?;
    backend.create(&RenderConfig::for_tv(WIDTH, HEIGHT, TvType::Ntsc))?;
    run_created(&mut backend).map(|_| ())
}

#[allow(dead_code)]
fn main() -> Result<(), Box<dyn Error>> {
    run()
}
