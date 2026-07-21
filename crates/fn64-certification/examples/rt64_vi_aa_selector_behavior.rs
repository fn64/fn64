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
const WIDTH: u32 = 8;
const HEIGHT: u32 = 5;
// Pinned RT64's public VI-size heuristic expands this five-line viewport to
// an eight-row source lattice. Populate the complete lattice so nearest
// sampling never relies on an uninitialized border.
const SOURCE_HEIGHT: u32 = 8;
const CENTER: (u32, u32) = (3, 2);
const STATUS_RGBA16_AA_ALWAYS: u32 = 0x002;
const STATUS_RGBA16_AA_NEEDED: u32 = 0x102;
const STATUS_RGBA16_RESAMPLE: u32 = 0x202;
const STATUS_RGBA16_REPLICATE: u32 = 0x302;
const STATUS_DIVOT: u32 = 1 << 4;
const CVG_DST_CLAMP: u32 = 0;
const CVG_X_ALPHA: u32 = 0x1000;

const BASELINE_SHA256: &str = "dc2f73283b7663a236726bc08eb5c941cac5010cb91104e3b90bd1dab8ec12c3";
const AA_SHA256: &str = "bbc7e1e3c901e25bdc513246f119f1f6289f5cbc4994c5379dbdf0e6e07068a6";
const DIVOT_SHA256: &str = "50367cf303140a240d094981f54a6b1eaa2160f40890c4872edd6aa457f45156";
const AA_DIVOT_SHA256: &str = "3649aadadcfb778f6e163414dbee0a20b5a5f26845ca4542da3959580846a992";

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
    match (x, y) {
        CENTER => [28, 16, 20],
        (2, 1) => [2, 1, 2],
        (4, 1) => [24, 3, 4],
        (1, 2) => [4, 5, 6],
        (5, 2) => [22, 7, 8],
        (2, 3) => [6, 9, 10],
        (4, 3) => [20, 11, 12],
        (2, 2) => [10, 6, 8],
        (4, 2) => [22, 13, 16],
        _ => [2 + y as u8, 3 + y as u8, 1 + y as u8],
    }
}

fn coverage_code(x: u32, y: u32) -> u8 {
    if (x, y) == CENTER {
        4
    } else {
        7
    }
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
    let [red, green, blue] = rgb5(CENTER.0, CENTER.1).map(expand5);
    push(
        &mut commands,
        0xfa00_0000,
        (u32::from(red) << 24) | (u32::from(green) << 16) | (u32::from(blue) << 8) | 0x80,
    );
    push(
        &mut commands,
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((SOURCE_HEIGHT - 1) * 4),
        0,
    );
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xef30_00f0, CVG_DST_CLAMP);
    for y in 0..SOURCE_HEIGHT {
        for x in 0..WIDTH {
            if (x, y) == CENTER {
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

fn validate_projected_lattice(baseline: &[u8]) -> (u32, u32) {
    assert_eq!(baseline.len(), (WIDTH * HEIGHT * 4) as usize);
    let center = rgb5(CENTER.0, CENTER.1);
    let mut projected_center = None;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let observed = pixel(baseline, x, y);
            assert_eq!(observed[3], 0xff, "post-VI alpha drifted at ({x}, {y})");
            if [observed[2] >> 3, observed[1] >> 3, observed[0] >> 3] == center {
                assert!(
                    projected_center.replace((x, y)).is_none(),
                    "declared partial foreground projected more than once"
                );
            }
        }
    }
    let projected_center = projected_center.expect("declared partial foreground was not sampled");
    assert_eq!(projected_center, (3, 1));
    for (x, declared) in [
        (projected_center.0 - 1, rgb5(CENTER.0 - 1, CENTER.1)),
        (projected_center.0 + 1, rgb5(CENTER.0 + 1, CENTER.1)),
    ] {
        let observed = pixel(baseline, x, projected_center.1);
        assert_eq!(
            [observed[2] >> 3, observed[1] >> 3, observed[0] >> 3],
            declared,
            "projected horizontal source lattice drifted at x={x}"
        );
    }
    assert_eq!(
        baseline
            .chunks_exact(4)
            .filter(|pixel| pixel[..3] == [0, 0, 0])
            .count(),
        10,
        "pinned RT64 projection border drifted"
    );
    projected_center
}

fn coverage_aa_center_rgb() -> [u8; 3] {
    const OFFSETS: [(i32, i32); 6] = [(-1, -1), (1, -1), (-2, 0), (2, 0), (-1, 1), (1, 1)];
    let foreground = rgb5(CENTER.0, CENTER.1).map(expand5);
    let mut neighbors = Vec::new();
    for (delta_x, delta_y) in OFFSETS {
        let neighbor_x = (CENTER.0 as i32 + delta_x) as u32;
        let neighbor_y = (CENTER.1 as i32 + delta_y) as u32;
        assert_eq!(coverage_code(neighbor_x, neighbor_y), 7);
        neighbors.push(rgb5(neighbor_x, neighbor_y).map(expand5));
    }
    let coverage = coverage_code(CENTER.0, CENTER.1);
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
    assert_eq!(filtered, [132, 78, 99]);
    filtered
}

fn coverage_aa_oracle(source: &[u8], projected_center: (u32, u32)) -> Vec<u8> {
    let mut output = source.to_vec();
    let filtered = coverage_aa_center_rgb();
    let start = ((projected_center.1 * WIDTH + projected_center.0) * 4) as usize;
    output[start..start + 3].copy_from_slice(&[filtered[2], filtered[1], filtered[0]]);
    output
}

fn median(left: u8, center: u8, right: u8) -> u8 {
    let mut values = [left, center, right];
    values.sort_unstable();
    values[1]
}

fn divot_oracle(source: &[u8], partial: (u32, u32)) -> Vec<u8> {
    let mut output = source.to_vec();
    for y in 0..HEIGHT {
        for x in 1..WIDTH - 1 {
            if y != partial.1 || ![x - 1, x, x + 1].contains(&partial.0) {
                continue;
            }
            let left = pixel(source, x - 1, y);
            let center = pixel(source, x, y);
            let right = pixel(source, x + 1, y);
            let start = ((y * WIDTH + x) * 4) as usize;
            for channel in 0..3 {
                output[start + channel] = median(left[channel], center[channel], right[channel]);
            }
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
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, command_end, TARGET)?;
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
    let projected_center = validate_projected_lattice(baseline);
    let expected_aa = coverage_aa_oracle(baseline, projected_center);
    let expected_divot = divot_oracle(baseline, projected_center);
    let expected_aa_divot = divot_oracle(&expected_aa, projected_center);
    let reverse_divot_then_aa = coverage_aa_oracle(&expected_divot, projected_center);
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
    let compatibility_center = validate_projected_lattice(compatibility_baseline);
    let expected_compatibility_aa =
        coverage_aa_oracle(compatibility_baseline, compatibility_center);
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
    assert_eq!(changed_pixels(baseline, &expected_aa), 1);
    assert_eq!(changed_pixels(baseline, &expected_divot), 1);
    assert_eq!(changed_pixels(baseline, &expected_aa_divot), 2);
    for (left, right) in baseline.chunks_exact(4).zip(expected_aa.chunks_exact(4)) {
        assert_eq!(left[3], right[3], "AA oracle changed alpha");
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
        "vi_aa_selector_evidence baseline_sha256={} aa_sha256={} divot_sha256={} aa_divot_sha256={} compat_baseline_sha256={} compat_mode0_sha256={} aa_changed_pixels=1 divot_changed_pixels=1 aa_divot_changed_pixels=2 phases={} bounded_residual=rt64-managed-coverage",
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
