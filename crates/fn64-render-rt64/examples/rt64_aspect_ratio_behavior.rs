use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderResolution, RenderRuntimeSettings, RenderSettingsApply,
    ResolutionMultiplier, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: u32 = 0x2000;
const SOURCE_WIDTH: u32 = 64;
const SOURCE_HEIGHT: u32 = 48;
const PRESENT_WIDTH: u32 = 112;
const PRESENT_HEIGHT: u32 = 48;
const TARGET_BYTES: usize = SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 2;
const GUARD: u32 = 0xa5c3_7e19;
const RED: u16 = 0xf801;
const GREEN: u16 = 0x07c1;
const BLUE: u16 = 0x003f;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ColorStats {
    bounds: [u32; 4],
    pixels: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CaptureStats {
    present_id: u64,
    red: ColorStats,
    green: ColorStats,
    blue: ColorStats,
    sha256: [u8; 32],
    policy_sha256: [u8; 32],
}

#[derive(Copy, Clone)]
struct ExpectedStats {
    red: ColorStats,
    green: ColorStats,
    blue: ColorStats,
    sha256: &'static str,
}

const EXPECTED: [ExpectedStats; 4] = [
    ExpectedStats {
        red: ColorStats {
            bounds: [29, 7, 37, 17],
            pixels: 99,
        },
        green: ColorStats {
            bounds: [47, 26, 55, 36],
            pixels: 99,
        },
        blue: ColorStats {
            bounds: [24, 0, 60, 43],
            pixels: 1430,
        },
        sha256: "6600d15768d99ff607d67e605505e23b6009b06dac2e3c6f8c49a38ba32d1789",
    },
    ExpectedStats {
        red: ColorStats {
            bounds: [32, 7, 40, 17],
            pixels: 99,
        },
        green: ColorStats {
            bounds: [48, 26, 55, 36],
            pixels: 88,
        },
        blue: ColorStats {
            bounds: [29, 0, 59, 43],
            pixels: 1177,
        },
        sha256: "d420b54a9e3c2180c25eaad538e5f9db23b56b86ed6df2b27994a344d7bc39ff",
    },
    ExpectedStats {
        red: ColorStats {
            bounds: [30, 7, 38, 17],
            pixels: 99,
        },
        green: ColorStats {
            bounds: [47, 26, 55, 36],
            pixels: 99,
        },
        blue: ColorStats {
            bounds: [26, 0, 59, 43],
            pixels: 1298,
        },
        sha256: "b98b969542d7dc68cd1e4c29246fd3d52346c5e55ae75b5139945e3346492f4a",
    },
    ExpectedStats {
        red: ColorStats {
            bounds: [26, 7, 35, 17],
            pixels: 110,
        },
        green: ColorStats {
            bounds: [46, 26, 55, 36],
            pixels: 110,
        },
        blue: ColorStats {
            bounds: [21, 0, 60, 43],
            pixels: 1540,
        },
        sha256: "357f00115a03cb681c92f5e7a8cf48812d78f40efeb40c09434509d660b5a9c1",
    },
];

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fill_rect(upper_left: [u32; 2], lower_right: [u32; 2]) -> (u32, u32) {
    (
        0xf600_0000 | (lower_right[0] * 4) << 12 | (lower_right[1] * 4),
        ((upper_left[0] * 4) << 12) | (upper_left[1] * 4),
    )
}

fn fixture() -> (Vec<u8>, u32) {
    let commands = [
        (0xef30_00f0, 0),
        (0xff10_0000 | (SOURCE_WIDTH - 1), TARGET),
        (0xf700_0000, u32::from(BLUE) * 0x1_0001),
        fill_rect([0, 0], [SOURCE_WIDTH - 1, SOURCE_HEIGHT - 1]),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        fill_rect([8, 8], [23, 19]),
        (0xf700_0000, u32::from(GREEN) * 0x1_0001),
        fill_rect([40, 28], [55, 39]),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0; RDRAM_LEN];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    let target = TARGET as usize;
    rdram[target - 4..target].copy_from_slice(&GUARD.to_ne_bytes());
    rdram[target + TARGET_BYTES..target + TARGET_BYTES + 4].copy_from_slice(&GUARD.to_ne_bytes());
    (rdram, (COMMANDS + commands.len() * 8) as u32)
}

fn settings(
    aspect_ratio: RenderAspectRatio,
    aspect_target: f64,
) -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(1.0)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio,
        aspect_target: AspectTarget::new(aspect_target)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn active_policy_sha256(
    backend: &Rt64Backend,
    requested: &RenderRuntimeSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    let active = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("RT64 backend has no complete active runtime policy"))?;
    if active.user != *requested || active != backend.configured_runtime_policy() {
        return Err(io::Error::other(
            "active RT64 aspect policy did not exactly match the complete configured policy",
        )
        .into());
    }
    Ok(active.sha256())
}

fn apply_settings(
    backend: &mut Rt64Backend,
    requested: &RenderRuntimeSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    let outcome = backend.apply_runtime_settings(requested)?;
    let expected = RenderSettingsApply::LiveApplied {
        settings_sha256: requested.sha256(),
        framebuffers_discarded: true,
    };
    if outcome != expected {
        return Err(io::Error::other(format!(
            "aspect switch did not take RT64's live framebuffer-discard path: expected {expected:?}, got {outcome:?}"
        ))
        .into());
    }
    active_policy_sha256(backend, requested)
}

fn pixel(capture: &Rt64PresentedPixels, x: u32, y: u32) -> [u8; 4] {
    let offset = (y * capture.row_bytes + x * 4) as usize;
    capture.bytes[offset..offset + 4]
        .try_into()
        .expect("validated tightly packed BGRA8 capture")
}

fn classify(sample: [u8; 4]) -> Option<usize> {
    let [blue, green, red, alpha] = sample;
    if alpha < 192 {
        return None;
    }
    let channels = [red, green, blue];
    let (winner, value) = channels
        .into_iter()
        .enumerate()
        .max_by_key(|(_, value)| *value)
        .expect("three color channels");
    (value >= 192
        && channels
            .into_iter()
            .enumerate()
            .all(|(index, other)| index == winner || value >= other.saturating_add(96)))
    .then_some(winner)
}

fn inspect(
    capture: &Rt64PresentedPixels,
    policy_sha256: [u8; 32],
) -> Result<CaptureStats, Box<dyn Error>> {
    if capture.width != PRESENT_WIDTH
        || capture.height != PRESENT_HEIGHT
        || capture.row_bytes != PRESENT_WIDTH * 4
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.bytes.len() != (capture.row_bytes * capture.height) as usize
    {
        return Err(io::Error::other(format!(
            "aspect fixture presentation layout changed: {capture:?}"
        ))
        .into());
    }
    let mut minima = [[PRESENT_WIDTH, PRESENT_HEIGHT]; 3];
    let mut maxima = [[0, 0]; 3];
    let mut counts = [0usize; 3];
    for y in 0..PRESENT_HEIGHT {
        for x in 0..PRESENT_WIDTH {
            if let Some(channel) = classify(pixel(capture, x, y)) {
                minima[channel][0] = minima[channel][0].min(x);
                minima[channel][1] = minima[channel][1].min(y);
                maxima[channel][0] = maxima[channel][0].max(x);
                maxima[channel][1] = maxima[channel][1].max(y);
                counts[channel] += 1;
            }
        }
    }
    if counts.contains(&0) {
        return Err(io::Error::other(format!(
            "aspect fixture lost a commanded color: counts={counts:?}"
        ))
        .into());
    }
    let stats = |channel: usize| ColorStats {
        bounds: [
            minima[channel][0],
            minima[channel][1],
            maxima[channel][0],
            maxima[channel][1],
        ],
        pixels: counts[channel],
    };
    Ok(CaptureStats {
        present_id: capture.present_id,
        red: stats(0),
        green: stats(1),
        blue: stats(2),
        sha256: Sha256::digest(&capture.bytes).into(),
        policy_sha256,
    })
}

fn render(
    backend: &mut Rt64Backend,
    policy_sha256: [u8; 32],
    guest_cycle: u64,
) -> Result<CaptureStats, Box<dyn Error>> {
    let (mut rdram, end) = fixture();
    if backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET)?
        != FrameStatus::Complete
    {
        return Err(io::Error::other("aspect raw-RDP fixture did not complete").into());
    }
    let target = TARGET as usize;
    if u32::from_ne_bytes(rdram[target - 4..target].try_into()?) != GUARD
        || u32::from_ne_bytes(rdram[target + TARGET_BYTES..target + TARGET_BYTES + 4].try_into()?)
            != GUARD
    {
        return Err(io::Error::other("aspect fixture target guard changed").into());
    }
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
    if selection.present_id != capture.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != TARGET
        || selection.target_width != SOURCE_WIDTH
        || selection.target_height != SOURCE_HEIGHT
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "aspect capture is not causally associated with the commanded source target: capture_id={}, selection={selection:?}",
            capture.present_id
        ))
        .into());
    }
    inspect(&capture, policy_sha256)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "aspect evidence requires the clean pinned RT64 source: {source:?}"
        ))
        .into());
    }

    let cases = [
        (
            "original",
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

    let mut results = Vec::new();
    for (index, (label, requested)) in cases.iter().enumerate() {
        let policy_sha256 = if index == 0 {
            active_policy_sha256(&backend, requested)?
        } else {
            apply_settings(&mut backend, requested)?
        };
        let transition = render(&mut backend, policy_sha256, index as u64 * 2 + 1)?;
        let result = render(&mut backend, policy_sha256, index as u64 * 2 + 2)?;
        println!(
            "phase={label}-transition id={} red={:?} green={:?} blue={:?} output_sha256={}",
            transition.present_id,
            transition.red,
            transition.green,
            transition.blue,
            hex(&transition.sha256),
        );
        if transition.red != result.red
            || transition.green != result.green
            || transition.blue != result.blue
            || transition.sha256 != result.sha256
        {
            return Err(io::Error::other(format!(
                "aspect mode did not stabilize across two completed presentations: transition={transition:?}, second={result:?}"
            ))
            .into());
        }
        let expected = EXPECTED[index];
        if result.red != expected.red
            || result.green != expected.green
            || result.blue != expected.blue
            || hex(&result.sha256) != expected.sha256
        {
            return Err(io::Error::other(format!(
                "{label} raw-DPC aspect output drifted: actual={result:?}, expected={{red:{:?},green:{:?},blue:{:?},sha256:{}}}",
                expected.red, expected.green, expected.blue, expected.sha256
            ))
            .into());
        }
        println!(
            "phase={label} id={} red={:?} green={:?} blue={:?} settings_sha256={} policy_sha256={} output_sha256={}",
            result.present_id,
            result.red,
            result.green,
            result.blue,
            hex(&requested.sha256()),
            hex(&result.policy_sha256),
            hex(&result.sha256),
        );
        results.push(result);
    }

    if !results
        .windows(2)
        .all(|pair| pair[0].present_id < pair[1].present_id)
    {
        return Err(io::Error::other("aspect presentation IDs did not advance").into());
    }
    if results
        .windows(2)
        .any(|pair| pair[0].policy_sha256 == pair[1].policy_sha256)
    {
        return Err(
            io::Error::other("distinct aspect settings produced one policy identity").into(),
        );
    }

    // This fixture deliberately stops at raw-DPC/output behavior. Distinct
    // geometry and bytes reject a no-op settings path, but cannot certify the
    // recognized-HLE projection, viewport/scissor, or explicit 2D semantics
    // required by the public widescreen and ultrawide rows.
    if results.iter().enumerate().any(|(left, result)| {
        results
            .iter()
            .enumerate()
            .any(|(right, other)| left != right && result.sha256 == other.sha256)
    }) {
        return Err(io::Error::other("distinct aspect modes produced identical output").into());
    }

    println!(
        "rt64 aspect raw-DPC behavior: dimensions={}x{} source={}x{} modes={} anchors={{red:{:?},green:{:?}}} backgrounds={:?}",
        PRESENT_WIDTH,
        PRESENT_HEIGHT,
        SOURCE_WIDTH,
        SOURCE_HEIGHT,
        results.len(),
        results[0].red,
        results[0].green,
        results.iter().map(|result| result.blue).collect::<Vec<_>>(),
    );
    Ok(())
}
