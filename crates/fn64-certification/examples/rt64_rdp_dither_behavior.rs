use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, RenderBackend, RenderConfig, RenderGraphicsApi, RenderRuntimeSettings,
    RenderSettingsApply,
};
use fn64_render_rt64::{Rt64Backend, Rt64SourceProvenance};
use fn64_runtime::{RdramAddr, RdramView};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TARGET: u32 = 0x4000;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const BACKGROUND: u16 = 0x003f;
const RGB_MAGIC_SHA256: &str = "c3391c2f090b4bb04a50a2a5ed903694ec15775c91a7eed1ec450813fca3e45d";
const RGB_BAYER_SHA256: &str = "0c94c59e6bc4b7173c5c3920db9d1ab138e2763fead27e2015b1ca0905c5b18e";
const RGB_NOISE_SHA256: &str = "9810f42c4f737b5b3ad532441a62d7c9c42872fe4b21dfb7db9aff13500e3afb";
const RGB_DISABLED_SHA256: &str =
    "8f9bf5c1f44445ba8acf0868c17b1bd47f0332205821438947d866399b36eb51";
const ALPHA_PATTERN_SHA256: &str =
    "a9cfd7e658721807e81c2a96aeb874c1f28e9dfc973c11109e6c7c7d87bf59ae";
const ALPHA_INVERSE_SHA256: &str =
    "1d4571614294533e0767fbff84176a8bd6a3a4161f4e02c80bcb3dc10be33fda";
const ALPHA_NOISE_SHA256: &str = "303ad447f114bab719b0504e9c4fcfbe48da869f4b82ef3f1b2410b272d0ebd7";
const ALPHA_DISABLED_SHA256: &str =
    "cd48436ead99c7e4d42d1a940855fee742c16396b4c32a1e2ab3f4518923ae6c";
const AC_NONE_SHA256: &str = "82d4429f93a2052ee806eab29b7a256097e3675e1c14263a8fe05452e83c9978";
const AC_DITHER_SHA256: &str = "1493e7af74f80caff7a0c645b0f522ec347ce38a198237ab3cbd802394e0c793";
const SHARED_NOISE_AC_NONE_SHA256: &str =
    "0268d9c2410c25067f144983829a5a091525f357e2981fc53f25e3d2c054da7f";
const SHARED_NOISE_AC_DITHER_SHA256: &str =
    "70289db3267cb703e806ee9ba86635ec651aab0ec56f434db1cf7988cbb34251";

#[derive(Clone, Copy, Debug)]
struct Case {
    label: &'static str,
    rgb_dither: u32,
    alpha_dither: u32,
    other_mode_low: u32,
    primitive: u32,
    blend_alpha: u8,
    combiner_noise_rgb: bool,
}

fn push(commands: &mut Vec<(u32, u32)>, word0: u32, word1: u32) {
    commands.push((word0, word1));
}

fn fixture(case: Case) -> (Vec<u8>, u32) {
    let mut commands = Vec::new();
    push(&mut commands, 0xff10_0000 | (WIDTH - 1), TARGET);
    push(
        &mut commands,
        0xed00_0000,
        ((WIDTH * 4) << 12) | (HEIGHT * 4),
    );

    // Establish a loud reject marker without asking the one-cycle color path
    // to read uninitialized target memory. Fill-cycle lower/right coordinates
    // are inclusive.
    push(&mut commands, 0xef30_00f0, 0);
    push(&mut commands, 0xf700_0000, u32::from(BACKGROUND) * 0x1_0001);
    push(
        &mut commands,
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
        0,
    );
    push(&mut commands, 0xe700_0000, 0);

    // Public gbi.h selector topology: RGB bits 6..7, alpha bits 4..5,
    // alpha-compare bits 0..1. The default combiner selects PRIMITIVE for
    // RGBA; the correlated-noise cases replace only its RGB equation below.
    let other_mode_high = (case.rgb_dither << 6) | (case.alpha_dither << 4);
    push(
        &mut commands,
        0xef00_0000 | other_mode_high,
        case.other_mode_low,
    );
    if case.combiner_noise_rgb {
        // Public gsDPSetCombineLERP packing for one-cycle
        // (NOISE - 0) * PRIMITIVE + 0 in RGB and PRIMITIVE alpha. This
        // exposes the fragment's combiner-noise sample in the written RGB
        // while leaving alpha comparison on a caller-selected constant.
        push(&mut commands, 0xfc00_00e3, 0x08fc_01fb);
    } else {
        push(&mut commands, 0xfcff_ffff, 0xfffd_f6fb);
    }
    push(&mut commands, 0xf900_0000, u32::from(case.blend_alpha));
    push(&mut commands, 0xfa00_0000, case.primitive);
    // One-cycle lower/right coordinates are exclusive.
    push(
        &mut commands,
        0xf600_0000 | ((WIDTH * 4) << 12) | (HEIGHT * 4),
        0,
    );
    push(&mut commands, 0xe900_0000, 0);

    let end = COMMANDS + commands.len() * 8;
    let mut rdram = vec![0; RDRAM_LEN];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    (rdram, end as u32)
}

fn render(backend: &mut Rt64Backend, case: Case) -> Result<Vec<u16>, Box<dyn Error>> {
    let (mut rdram, end) = fixture(case);
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET, true)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "{} raw-DPC submission returned {status:?}",
            case.label
        ))
        .into());
    }
    let view = RdramView::from_storage(&rdram);
    Ok((0..WIDTH * HEIGHT)
        .map(|index| view.read_u16(RdramAddr::from_offset(TARGET + index * 2)))
        .collect())
}

fn digest(pixels: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for pixel in pixels {
        hasher.update(pixel.to_be_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn repeated_tile(tile: [[u16; 4]; 4]) -> Vec<u16> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| tile[(y % 4) as usize][(x % 4) as usize]))
        .collect()
}

fn require_digest(label: &str, pixels: &[u16], expected: &str) -> Result<(), Box<dyn Error>> {
    let actual = digest(pixels);
    if actual != expected {
        return Err(io::Error::other(format!(
            "{label} native RGBA16 digest drifted: expected {expected}, got {actual}"
        ))
        .into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "RDP dither behavior requires clean pinned RT64 Metal: {identity:?}"
        ))
        .into());
    }
    let settings = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new();
    let staged = backend.apply_runtime_settings(&settings)?;
    if staged
        != (RenderSettingsApply::StagedForCreate {
            settings_sha256: settings.sha256(),
        })
    {
        return Err(io::Error::other(format!(
            "RDP dither Metal settings were not staged exactly: {staged:?}"
        ))
        .into());
    }
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    if backend.active_settings() != Some(&settings) {
        return Err(io::Error::other("RT64 did not activate exact Metal settings").into());
    }

    let rgb_cases = [
        ("rgb-magic", 0),
        ("rgb-bayer", 1),
        ("rgb-noise", 2),
        ("rgb-disabled", 3),
    ]
    .map(|(label, rgb_dither)| Case {
        label,
        rgb_dither,
        alpha_dither: 3,
        other_mode_low: 0,
        primitive: 0x0707_07ff,
        blend_alpha: 0,
        combiner_noise_rgb: false,
    });
    let alpha_cases = [
        ("alpha-pattern", 0),
        ("alpha-inverse", 1),
        ("alpha-noise", 2),
        ("alpha-disabled", 3),
    ]
    .map(|(label, alpha_dither)| Case {
        label,
        rgb_dither: 3,
        alpha_dither,
        // Public one-cycle source-over blender: IN*A_IN + MEM*(1-A), with
        // IM_RD and FORCE_BL. AC_NONE keeps this axis independent from
        // alpha comparison; the selector changes the blender's five-bit A.
        other_mode_low: 0x0040_4040,
        primitive: 0xff00_007f,
        blend_alpha: 0,
        combiner_noise_rgb: false,
    });
    let ac_dither = Case {
        label: "alpha-compare-dither",
        rgb_dither: 3,
        alpha_dither: 3,
        other_mode_low: 3,
        primitive: 0x00f8_0080,
        blend_alpha: 0,
        combiner_noise_rgb: false,
    };
    let ac_none = Case {
        label: "alpha-compare-none",
        other_mode_low: 0,
        ..ac_dither
    };
    let shared_noise_ac_none = Case {
        label: "shared-noise-alpha-compare-none",
        rgb_dither: 3,
        alpha_dither: 3,
        other_mode_low: 0,
        primitive: 0xffff_ff80,
        blend_alpha: 0,
        combiner_noise_rgb: true,
    };
    let shared_noise_ac_dither = Case {
        label: "shared-noise-alpha-compare-dither",
        other_mode_low: 3,
        ..shared_noise_ac_none
    };

    let cases = rgb_cases
        .into_iter()
        .chain(alpha_cases)
        .chain([
            ac_none,
            ac_dither,
            shared_noise_ac_none,
            shared_noise_ac_dither,
        ])
        .collect::<Vec<_>>();
    let mut observations = Vec::new();
    for case in cases {
        let pixels = render(&mut backend, case)?;
        observations.push((case, pixels));
    }
    let repeat_labels = [
        "rgb-magic",
        "rgb-bayer",
        "rgb-disabled",
        "alpha-pattern",
        "alpha-inverse",
        "alpha-disabled",
        "alpha-compare-none",
    ];
    for label in repeat_labels {
        let (case, expected) = observations
            .iter()
            .find(|(case, _)| case.label == label)
            .unwrap_or_else(|| panic!("missing repeatable RDP dither observation {label}"));
        let repeat = render(&mut backend, *case)?;
        if &repeat != expected {
            return Err(io::Error::other(format!(
                "{} did not reproduce exact native pixels on an identical same-context workload: first={} repeat={}",
                case.label,
                digest(expected),
                digest(&repeat)
            ))
            .into());
        }
    }
    let by_label = |label: &str| {
        observations
            .iter()
            .find(|(case, _)| case.label == label)
            .map(|(_, pixels)| pixels)
            .unwrap_or_else(|| panic!("missing RDP dither observation {label}"))
    };
    // These are pinned-RT64 black-box observations, not public matrix values
    // and not silicon authority. Exact spatial tiles keep the two ordered
    // selectors distinct instead of accepting any merely nonuniform result.
    let magic = repeated_tile([
        [0x0001, 0x0843, 0x0843, 0x0843],
        [0x0843, 0x0843, 0x0843, 0x0843],
        [0x0843, 0x0843, 0x0843, 0x0843],
        [0x0843, 0x0843, 0x0843, 0x0001],
    ]);
    let bayer = repeated_tile([
        [0x0001, 0x0843, 0x0843, 0x0843],
        [0x0843, 0x0001, 0x0843, 0x0843],
        [0x0843, 0x0843, 0x0843, 0x0843],
        [0x0843, 0x0843, 0x0843, 0x0843],
    ]);
    if by_label("rgb-magic") != &magic || by_label("rgb-bayer") != &bayer {
        return Err(io::Error::other(
            "pinned RT64 ordered RGB selector tile changed exact spatial values",
        )
        .into());
    }
    if by_label("rgb-disabled") != &vec![0x0001; (WIDTH * HEIGHT) as usize] {
        return Err(io::Error::other("RGB Disabled no longer truncates the low three bits").into());
    }
    let rgb_noise_values = by_label("rgb-noise")
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let expected_noise_values = std::collections::BTreeSet::from([
        0x0001, 0x0003, 0x0041, 0x0043, 0x0801, 0x0803, 0x0841, 0x0843, 0x0845, 0x0883, 0x0885,
        0x1043, 0x1045, 0x1085,
    ]);
    if rgb_noise_values != expected_noise_values {
        return Err(io::Error::other(format!(
            "pinned RT64 RGB Noise value set drifted: {rgb_noise_values:?}"
        ))
        .into());
    }
    for (label, expected) in [
        ("rgb-magic", RGB_MAGIC_SHA256),
        ("rgb-bayer", RGB_BAYER_SHA256),
        ("rgb-noise", RGB_NOISE_SHA256),
        ("rgb-disabled", RGB_DISABLED_SHA256),
    ] {
        require_digest(label, by_label(label), expected)?;
    }
    if by_label("rgb-magic") == by_label("rgb-bayer")
        || by_label("rgb-magic") == by_label("rgb-noise")
        || by_label("rgb-bayer") == by_label("rgb-noise")
        || by_label("rgb-disabled") == by_label("rgb-magic")
        || by_label("rgb-disabled") == by_label("rgb-bayer")
        || by_label("rgb-disabled") == by_label("rgb-noise")
    {
        return Err(io::Error::other("public RGB selector controls collapsed").into());
    }

    // AC_NONE plus the public source-over tuple isolates G_AD after alpha
    // compare and before blending. These tiles certify the fn64-owned native
    // overlay's target behavior; they are not claimed as physical matrices.
    let alpha_pattern = repeated_tile([
        [0x801f, 0x801f, 0x801f, 0x801f],
        [0x801f, 0x801f, 0x801f, 0x801f],
        [0x801f, 0x7821, 0x801f, 0x801f],
        [0x7821, 0x801f, 0x801f, 0x801f],
    ]);
    let alpha_inverse = repeated_tile([
        [0x7821, 0x801f, 0x801f, 0x801f],
        [0x801f, 0x7821, 0x801f, 0x801f],
        [0x801f, 0x801f, 0x801f, 0x801f],
        [0x801f, 0x801f, 0x801f, 0x801f],
    ]);
    let alpha_disabled = vec![0x7821; (WIDTH * HEIGHT) as usize];
    if by_label("alpha-pattern") != &alpha_pattern
        || by_label("alpha-inverse") != &alpha_inverse
        || by_label("alpha-disabled") != &alpha_disabled
    {
        return Err(io::Error::other(
            "native alpha Pattern/Inverse/Disabled target tiles changed exact source-over behavior",
        )
        .into());
    }
    let alpha_noise_values = by_label("alpha-noise")
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if alpha_noise_values != std::collections::BTreeSet::from([0x7821, 0x801f])
        || by_label("alpha-noise") == by_label("alpha-disabled")
        || by_label("alpha-pattern") == by_label("alpha-inverse")
        || by_label("alpha-pattern") == by_label("alpha-disabled")
        || by_label("alpha-inverse") == by_label("alpha-disabled")
    {
        return Err(io::Error::other(
            "public alpha selectors lost Pattern/Inverse/Disabled separation or live Noise",
        )
        .into());
    }
    for (label, expected) in [
        ("alpha-pattern", ALPHA_PATTERN_SHA256),
        ("alpha-inverse", ALPHA_INVERSE_SHA256),
        ("alpha-noise", ALPHA_NOISE_SHA256),
        ("alpha-disabled", ALPHA_DISABLED_SHA256),
    ] {
        require_digest(label, by_label(label), expected)?;
    }

    let ac_none_pixels = by_label("alpha-compare-none");
    let ac_dither_pixels = by_label("alpha-compare-dither");
    require_digest("alpha-compare-none", ac_none_pixels, AC_NONE_SHA256)?;
    require_digest("alpha-compare-dither", ac_dither_pixels, AC_DITHER_SHA256)?;
    let ac_passes = ac_dither_pixels
        .iter()
        .filter(|&&pixel| pixel == 0x07c1)
        .count();
    if ac_none_pixels != &vec![0x07c1; (WIDTH * HEIGHT) as usize]
        || ac_passes != 123
        || ac_dither_pixels
            .iter()
            .any(|&pixel| pixel != BACKGROUND && pixel != 0x07c1)
    {
        return Err(io::Error::other(format!(
            "G_AC_DITHER lost exact none/dither control separation: passes={ac_passes}"
        ))
        .into());
    }

    let shared_none_pixels = by_label("shared-noise-alpha-compare-none");
    let shared_dither_pixels = by_label("shared-noise-alpha-compare-dither");
    require_digest(
        "shared-noise-alpha-compare-none",
        shared_none_pixels,
        SHARED_NOISE_AC_NONE_SHA256,
    )?;
    require_digest(
        "shared-noise-alpha-compare-dither",
        shared_dither_pixels,
        SHARED_NOISE_AC_DITHER_SHA256,
    )?;
    let shared_none_foreground = shared_none_pixels
        .iter()
        .copied()
        .filter(|&pixel| pixel != BACKGROUND)
        .collect::<Vec<_>>();
    let shared_none_levels = shared_none_foreground
        .iter()
        .map(|pixel| pixel >> 11)
        .collect::<std::collections::BTreeSet<_>>();
    if shared_none_foreground.len() != (WIDTH * HEIGHT) as usize || shared_none_levels.len() < 16 {
        return Err(io::Error::other(format!(
            "combiner NOISE control stopped exposing a full live grayscale range: foreground={} levels={shared_none_levels:?} sha256={}",
            shared_none_foreground.len(),
            digest(shared_none_pixels)
        ))
        .into());
    }
    let mut shared_passes = 0usize;
    let mut shared_route_violations = Vec::new();
    for (index, &pixel) in shared_dither_pixels.iter().enumerate() {
        if pixel == BACKGROUND {
            continue;
        }
        shared_passes += 1;
        let red = pixel >> 11;
        let green = (pixel >> 6) & 0x1f;
        let blue = (pixel >> 1) & 0x1f;
        if red != green || red != blue || red > 16 {
            shared_route_violations.push((index, pixel, red, green, blue));
        }
    }
    if shared_passes != 146 || !shared_route_violations.is_empty() {
        return Err(io::Error::other(format!(
            "combiner NOISE and G_AC_DITHER did not route one shared fragment sample: passes={shared_passes} violations={} first={:?} none_sha256={} dither_sha256={}",
            shared_route_violations.len(),
            shared_route_violations.first(),
            digest(shared_none_pixels),
            digest(shared_dither_pixels)
        ))
        .into());
    }

    for (case, pixels) in &observations {
        println!(
            "rdp_dither_phase label={} sha256={} repeat={}",
            case.label,
            digest(pixels),
            if repeat_labels.contains(&case.label) {
                "same-context-exact"
            } else {
                "fresh-process-sequence-only"
            }
        );
    }
    println!(
        "rdp_dither_evidence rgb_selectors=distinct-exact alpha_selectors=distinct-exact ac_dither_passes=123 shared_noise_ac_passes={shared_passes} phases={} repeats={} target=rgba16 source=synthetic-raw-dpc bounded_residual=no-silicon-generator-seed-ties-or-reference-noise-parity",
        observations.len(),
        repeat_labels.len()
    );
    Ok(())
}
