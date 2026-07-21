use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderEmulatorSettings, RenderFiltering, RenderResolution, RenderRuntimeSettings,
    RenderUpscale2d, ResolutionMultiplier,
};
use fn64_render_rt64::{Rt64Backend, Rt64SourceProvenance};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const SOURCE: u32 = 0x4000;
const REINTERPRETED: u32 = 0x5000;
const SOURCE_WIDTH: u32 = 8;
const SOURCE_HEIGHT: u32 = 4;
const REGION_LEFT: u32 = 2;
const REGION_TOP: u32 = 1;
const REGION_WIDTH: u32 = 4;
const REGION_HEIGHT: u32 = 2;

const BLUE: u16 = 0x003f;
const RED_8: u16 = 0x4001;
const RED_16: u16 = 0x8001;
const RED_24: u16 = 0xc001;
const RED_31: u16 = 0xf801;
const STALE_SOURCE: u16 = 0x07c1;
const STALE_REINTERPRETED: u16 = 0xf83f;
const GUARD: u16 = 0x4211;

// RT64's RGBA16-to-IA16 reinterpret shader encodes the input as R10G10B10A2,
// then decodes the high and low 16-bit halves as intensity and alpha. These
// grayscale RGBA16 values are consequently different from both the original
// red texels and an ordinary RGBA framebuffer-region copy. The low bit is one
// because pinned RT64 emits full fragment coverage and RGBA16 stores the
// resulting coverage bit in that position.
const EXPECTED_REINTERPRETED: [u16; 8] = [
    0x4211, 0x8421, 0xc631, 0xffff, 0xffff, 0xc631, 0x8421, 0x4211,
];

fn push_command(commands: &mut Vec<(u32, u32)>, word0: u32, word1: u32) {
    commands.push((word0, word1));
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
        for index in 0..SOURCE_WIDTH * SOURCE_HEIGHT {
            view.write_u16(RdramAddr::from_offset(SOURCE + index * 2), STALE_SOURCE);
        }
        for index in 0..REGION_WIDTH * REGION_HEIGHT {
            view.write_u16(
                RdramAddr::from_offset(REINTERPRETED + index * 2),
                STALE_REINTERPRETED,
            );
        }
        for address in [
            SOURCE - 2,
            SOURCE + SOURCE_WIDTH * SOURCE_HEIGHT * 2,
            REINTERPRETED - 2,
            REINTERPRETED + REGION_WIDTH * REGION_HEIGHT * 2,
        ] {
            view.write_u16(RdramAddr::from_offset(address), GUARD);
        }
    }

    let mut commands = Vec::new();
    push_command(&mut commands, 0xef30_00f0, 0); // Fill cycle, RGB/alpha dither off.
    push_command(&mut commands, 0xff10_0007, SOURCE); // RGBA16, width 8.
    push_command(&mut commands, 0xf700_0000, u32::from(BLUE) * 0x1_0001);
    commands.push(fill_rect(SOURCE_WIDTH - 1, SOURCE_HEIGHT - 1, 0, 0));

    // The requested nonzero subregion contains four red intensities in each
    // direction; every texel outside it remains blue. A high-resolution copy
    // using native (unscaled) left/top coordinates therefore cannot satisfy
    // the expected result by accidentally copying a similar neighboring row.
    for (row, colors) in [
        [RED_8, RED_16, RED_24, RED_31],
        [RED_31, RED_24, RED_16, RED_8],
    ]
    .into_iter()
    .enumerate()
    {
        for (column, color) in colors.into_iter().enumerate() {
            push_command(&mut commands, 0xf700_0000, u32::from(color) * 0x1_0001);
            let x = REGION_LEFT + column as u32;
            let y = REGION_TOP + row as u32;
            commands.push(fill_rect(x, y, x, y));
        }
    }

    push_command(&mut commands, 0xe700_0000, 0); // PipeSync.
    push_command(&mut commands, 0xfd70_0007, SOURCE); // Treat source bytes as IA16.
    push_command(&mut commands, 0xe800_0000, 0); // TileSync.
    push_command(&mut commands, 0xf570_0400, 7 << 24); // IA16, line=2, load tile.
    push_command(&mut commands, 0xe600_0000, 0); // LoadSync.
    push_command(
        &mut commands,
        0xf400_0000 | ((REGION_LEFT * 4) << 12) | (REGION_TOP * 4),
        (7 << 24)
            | (((REGION_LEFT + REGION_WIDTH - 1) * 4) << 12)
            | ((REGION_TOP + REGION_HEIGHT - 1) * 4),
    );

    // Sampling the IA16 descriptor forces the framebuffer manager to dispatch
    // its format-changing RGBA16-to-IA16 compute reinterpretation. The output
    // is then rendered one-for-one into a distinct guest-visible framebuffer.
    push_command(&mut commands, 0xf570_0400, 0x0008_0200); // Clamp S/T, tile 0.
    push_command(
        &mut commands,
        0xf200_0000,
        ((REGION_WIDTH - 1) * 4) << 12 | ((REGION_HEIGHT - 1) * 4),
    );
    push_command(&mut commands, 0xfc8f_ff1f, 0x88fc_f279); // TEXEL0.
    push_command(&mut commands, 0xef00_00f0, 0); // One-cycle, RGB/alpha dither off.
    push_command(&mut commands, 0xff10_0003, REINTERPRETED); // RGBA16, width 4.
    push_command(
        &mut commands,
        0xe400_0000 | ((REGION_WIDTH * 4) << 12) | (REGION_HEIGHT * 4),
        0,
    );
    push_command(&mut commands, 0, 0x0400_0400); // s=t=0, dsdx=dtdy=1.
    push_command(&mut commands, 0xe900_0000, 0); // FullSync.

    let end = COMMANDS + commands.len() * 8;
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    (rdram, end as u32)
}

fn pixels(rdram: &[u8], address: u32, count: u32) -> Vec<u16> {
    let view = RdramView::from_storage(rdram);
    (0..count)
        .map(|index| view.read_u16(RdramAddr::from_offset(address + index * 2)))
        .collect()
}

fn digest_u16(values: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.to_be_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "framebuffer enhancement evidence requires clean pinned RT64: {source:?}"
        ))
        .into());
    }

    let settings = RenderRuntimeSettings {
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(2.0)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(SOURCE_WIDTH as f64 / SOURCE_HEIGHT as f64)?,
        upscale_2d: RenderUpscale2d::All,
        three_point_filtering: false,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let mut backend = Rt64Backend::new()
        .with_runtime_settings(settings.clone())
        .with_emulator_settings(RenderEmulatorSettings {
            post_blend_noise: false,
            post_blend_noise_negative: false,
            framebuffer_render_to_ram: true,
            framebuffer_copy_with_gpu: true,
        });
    backend.create(&RenderConfig::ntsc(SOURCE_WIDTH, SOURCE_HEIGHT))?;
    if backend.active_settings() != Some(&settings)
        || backend
            .active_settings()
            .is_none_or(|active| active.resolution_multiplier.get() != 2.0)
    {
        return Err(io::Error::other(
            "RT64 did not activate the exact non-unit 2x framebuffer policy",
        )
        .into());
    }

    let (mut rdram, end) = fixture();
    let source_before = pixels(&rdram, SOURCE, SOURCE_WIDTH * SOURCE_HEIGHT);
    let reinterpreted_before = pixels(&rdram, REINTERPRETED, REGION_WIDTH * REGION_HEIGHT);
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, REINTERPRETED)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "framebuffer enhancement fixture returned {status:?}"
        ))
        .into());
    }

    let source_after = pixels(&rdram, SOURCE, SOURCE_WIDTH * SOURCE_HEIGHT);
    let reinterpreted_after = pixels(&rdram, REINTERPRETED, REGION_WIDTH * REGION_HEIGHT);
    let expected_source: Vec<u16> = (0..SOURCE_HEIGHT)
        .flat_map(|y| {
            (0..SOURCE_WIDTH).map(move |x| match (x, y) {
                (2, 1) | (5, 2) => RED_8,
                (3, 1) | (4, 2) => RED_16,
                (4, 1) | (3, 2) => RED_24,
                (5, 1) | (2, 2) => RED_31,
                _ => BLUE,
            })
        })
        .collect();

    if source_after != expected_source {
        return Err(io::Error::other(format!(
            "2x source framebuffer writeback mismatch: expected={}, actual={}, pixels={source_after:04x?}",
            digest_u16(&expected_source),
            digest_u16(&source_after)
        ))
        .into());
    }
    if reinterpreted_after != EXPECTED_REINTERPRETED {
        return Err(io::Error::other(format!(
            "2x adjusted region plus RGBA16-to-IA16 reinterpret mismatch: expected={}, actual={}, pixels={reinterpreted_after:04x?}",
            digest_u16(&EXPECTED_REINTERPRETED),
            digest_u16(&reinterpreted_after)
        ))
        .into());
    }
    if reinterpreted_after
        .iter()
        .zip([RED_8, RED_16, RED_24, RED_31, RED_31, RED_24, RED_16, RED_8])
        .any(|(actual, ordinary_copy)| *actual == ordinary_copy)
    {
        return Err(io::Error::other(
            "reinterpret output retained an ordinary RGBA framebuffer-copy texel",
        )
        .into());
    }
    if source_after == source_before || reinterpreted_after == reinterpreted_before {
        return Err(io::Error::other(
            "both enhanced framebuffer regions must make guest-visible transitions",
        )
        .into());
    }

    let view = RdramView::from_storage(&rdram);
    for address in [
        SOURCE - 2,
        SOURCE + SOURCE_WIDTH * SOURCE_HEIGHT * 2,
        REINTERPRETED - 2,
        REINTERPRETED + REGION_WIDTH * REGION_HEIGHT * 2,
    ] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(io::Error::other(format!(
                "framebuffer enhancement write escaped at {address:#010x}: {actual:#06x}"
            ))
            .into());
        }
    }

    println!(
        "scale=2 region=({},{})->({},{}) source_before={} source_after={} reinterpreted_before={} reinterpreted_after={} source_transitions={} reinterpret_transitions={} source={:?}",
        REGION_LEFT,
        REGION_TOP,
        REGION_LEFT + REGION_WIDTH - 1,
        REGION_TOP + REGION_HEIGHT - 1,
        digest_u16(&source_before),
        digest_u16(&source_after),
        digest_u16(&reinterpreted_before),
        digest_u16(&reinterpreted_after),
        source_before
            .iter()
            .zip(&source_after)
            .filter(|(before, after)| before != after)
            .count(),
        reinterpreted_before
            .iter()
            .zip(&reinterpreted_after)
            .filter(|(before, after)| before != after)
            .count(),
        source,
    );
    Ok(())
}
