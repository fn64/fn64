use std::collections::BTreeSet;
use std::error::Error;
use std::io;

use fn64_render::{
    ActiveRenderGraphicsApi, FrameStatus, ReleaseCaptureFormat, RenderBackend, RenderConfig,
    RenderGraphicsApi, RenderRuntimeSettings, ViPresentation, ViScaleAxis, ViScanoutRegisters,
    ViScanoutState,
};
use fn64_render_rt64::{Rt64Backend, Rt64PresentedPixels, Rt64SourceProvenance};
use fn64_runtime::TvType;
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: u32 = 0x4000;
const SOURCE_WIDTH: u32 = 12;
const SOURCE_HEIGHT: u32 = 10;
const OUTPUT_WIDTH: u32 = 8;
const OUTPUT_HEIGHT: u32 = 6;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const BASELINE_SHA256: &str = "e1d959640644ed83214b89a238802ac82fc7e4b853131ff250f1f16f842ca355";
const GAMMA_SHA256: &str = "2281b631918039786430a3831ec3a6237b51171c61852bc44cf2fa88c6089a15";
const SCALED_SHA256: &str = "9549acd7464bb0da111623f1d5162803e05a1c4892755446764cf3eb73a8070a";

const STATUS_RGBA16_AA_ALWAYS: u32 = 0x002;
const STATUS_RGBA16_AA_NEEDED: u32 = 0x102;
const STATUS_RGBA16_RESAMPLE: u32 = 0x202;
const STATUS_RGBA16_REPLICATE: u32 = 0x302;
const STATUS_GAMMA: u32 = 1 << 3;
const STATUS_GAMMA_DITHER: u32 = 1 << 2;
const STATUS_DIVOT: u32 = 1 << 4;
const STATUS_DITHER_FILTER: u32 = 1 << 16;

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

fn push_command(rdram: &mut [u8], cursor: &mut usize, w0: u32, w1: u32) {
    rdram[*cursor..*cursor + 4].copy_from_slice(&w0.to_ne_bytes());
    rdram[*cursor + 4..*cursor + 8].copy_from_slice(&w1.to_ne_bytes());
    *cursor += 8;
}

fn rgba16(x: u32, y: u32) -> u16 {
    let red = (x * 2 + y * 3 + 3) & 31;
    let green = (x * 5 + y * 2 + 7) & 31;
    let blue = (x * 3 + y * 7 + 11) & 31;
    ((red << 11) | (green << 6) | (blue << 1) | 1) as u16
}

fn fixture() -> (Vec<u8>, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    let mut cursor = COMMANDS;
    push_command(&mut rdram, &mut cursor, 0xef00_0000 | (3 << 20), 0);
    push_command(
        &mut rdram,
        &mut cursor,
        0xff10_0000 | (SOURCE_WIDTH - 1),
        TARGET,
    );

    for y in 0..SOURCE_HEIGHT {
        for x in 0..SOURCE_WIDTH {
            let color = u32::from(rgba16(x, y));
            push_command(&mut rdram, &mut cursor, 0xf700_0000, (color << 16) | color);
            push_command(
                &mut rdram,
                &mut cursor,
                0xf600_0000 | ((x * 4) << 12) | (y * 4),
                ((x * 4) << 12) | (y * 4),
            );
        }
    }

    // A public edge-only fill triangle introduces a partial-coverage diagonal
    // for the native VI divot/AA controls. The command words follow the SGI
    // RDP Command Summary edge-coefficient layout used by fn64's raw decoder.
    push_command(&mut rdram, &mut cursor, 0xf700_0000, 0xffff_ffff);
    let yh = 4;
    let ym = 4 * 4;
    let yl = 8 * 4;
    let major_slope = (5.0f32 / 7.0 * 65536.0).round() as u32;
    let lower_slope = (5.0f32 / 4.0 * 65536.0).round() as u32;
    push_command(&mut rdram, &mut cursor, 0x0880_0000 | yl, (ym << 16) | yh);
    push_command(&mut rdram, &mut cursor, 1 << 16, lower_slope);
    push_command(&mut rdram, &mut cursor, 1 << 16, major_slope);
    push_command(&mut rdram, &mut cursor, 1 << 16, 0);
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

pub fn run() -> Result<(), Box<dyn Error>> {
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
    let settings = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new();
    backend.apply_runtime_settings(&settings)?;
    backend.create(&RenderConfig::for_tv(
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
        TvType::Ntsc,
    ))?;
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
            label: "gamma",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA,
            x_scale: one,
            y_scale: one,
            cycle: 101,
        },
        Phase {
            label: "baseline-b",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 102,
        },
        Phase {
            label: "gamma-dither",
            status: STATUS_RGBA16_REPLICATE | STATUS_GAMMA | STATUS_GAMMA_DITHER,
            x_scale: one,
            y_scale: one,
            cycle: 103,
        },
        Phase {
            label: "baseline-c",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 104,
        },
        Phase {
            label: "divot",
            status: STATUS_RGBA16_REPLICATE | STATUS_DIVOT,
            x_scale: one,
            y_scale: one,
            cycle: 105,
        },
        Phase {
            label: "baseline-d",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 106,
        },
        Phase {
            label: "dither-filter",
            status: STATUS_RGBA16_REPLICATE | STATUS_DITHER_FILTER,
            x_scale: one,
            y_scale: one,
            cycle: 107,
        },
        Phase {
            label: "baseline-e",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 108,
        },
        Phase {
            label: "resample-mode-3",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 109,
        },
        Phase {
            label: "resample-mode-2",
            status: STATUS_RGBA16_RESAMPLE,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 110,
        },
        Phase {
            label: "resample-mode-1",
            status: STATUS_RGBA16_AA_NEEDED,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 111,
        },
        Phase {
            label: "resample-mode-0",
            status: STATUS_RGBA16_AA_ALWAYS,
            x_scale: three_halves,
            y_scale: three_halves,
            cycle: 112,
        },
        Phase {
            label: "baseline-f",
            status: STATUS_RGBA16_REPLICATE,
            x_scale: one,
            y_scale: one,
            cycle: 113,
        },
    ];

    let mut observations = Vec::with_capacity(phases.len());
    for phase in phases {
        observations.push(observe(&mut backend, &rdram, phase)?);
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
    for label in [
        "baseline-a",
        "baseline-b",
        "baseline-c",
        "baseline-d",
        "baseline-e",
        "baseline-f",
        "divot",
        "dither-filter",
    ] {
        let observation = by_label(label);
        if observation.sha256 != BASELINE_SHA256
            || observation.nonblack_pixels != 48
            || observation.unique_colors != 48
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned baseline: {observation:?}"
            ))
            .into());
        }
    }
    for label in ["gamma", "gamma-dither"] {
        let observation = by_label(label);
        if observation.sha256 != GAMMA_SHA256
            || observation.nonblack_pixels != 48
            || observation.unique_colors != 48
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned gamma result: {observation:?}"
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
            || observation.unique_colors != 41
        {
            return Err(io::Error::other(format!(
                "{label} drifted from the exact pinned scaled result: {observation:?}"
            ))
            .into());
        }
    }
    if by_label("baseline-a").bytes == by_label("gamma").bytes
        || by_label("baseline-a").bytes == by_label("resample-mode-3").bytes
    {
        return Err(io::Error::other(
            "native gamma and nonidentity scale must each change exact post-VI pixels",
        )
        .into());
    }
    if by_label("gamma").bytes != by_label("gamma-dither").bytes
        || by_label("baseline-a").bytes != by_label("divot").bytes
        || by_label("baseline-a").bytes != by_label("dither-filter").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-2").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-1").bytes
        || by_label("resample-mode-3").bytes != by_label("resample-mode-0").bytes
    {
        return Err(io::Error::other(
            "pinned RT64's explicit native VI residual changed; review gamma-dither/divot/restoration/AA-selector support",
        )
        .into());
    }

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

    let release = backend.release_capture()?;
    if release.format != ReleaseCaptureFormat::PostViBgra8Unorm
        || release.width != OUTPUT_WIDTH
        || release.height != OUTPUT_HEIGHT
        || release.guest_cycle != 113
        || release.bytes != observations.last().unwrap().bytes
    {
        return Err(
            io::Error::other("release capture did not bind the final live VI phase").into(),
        );
    }
    println!(
        "vi_filter_pixel_evidence source={} baseline_sha256={} gamma_sha256={} scaled_sha256={} phases={} native_residual=gamma-dither,divot,dither-filter,aa-selector",
        identity.source_id,
        BASELINE_SHA256,
        GAMMA_SHA256,
        SCALED_SHA256,
        observations.len()
    );
    Ok(())
}

#[allow(dead_code)]
fn main() -> Result<(), Box<dyn Error>> {
    run()
}
