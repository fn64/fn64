use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, RenderBackend, RenderConfig, RenderEmulatorSettings, RenderRuntimeSettings,
};
use fn64_render_rt64::{Rt64Backend, Rt64SourceProvenance};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const SOURCE: u32 = 0x2000;
const SAMPLED: u32 = 0x3000;
const WIDTH: u32 = 8;
const SOURCE_HEIGHT: u32 = 4;
const SAMPLED_HEIGHT: u32 = 2;

const STALE_SOURCE: u16 = 0x07c1; // Green.
const STALE_SAMPLED: u16 = 0xffff; // White.
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const GUARD: u16 = 0x4211;

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
        for index in 0..WIDTH * SOURCE_HEIGHT {
            view.write_u16(RdramAddr::from_offset(SOURCE + index * 2), STALE_SOURCE);
        }
        for index in 0..WIDTH * SAMPLED_HEIGHT {
            view.write_u16(RdramAddr::from_offset(SAMPLED + index * 2), STALE_SAMPLED);
        }
        view.write_u16(RdramAddr::from_offset(SOURCE - 2), GUARD);
        view.write_u16(
            RdramAddr::from_offset(SOURCE + WIDTH * SOURCE_HEIGHT * 2),
            GUARD,
        );
        view.write_u16(RdramAddr::from_offset(SAMPLED - 2), GUARD);
        view.write_u16(
            RdramAddr::from_offset(SAMPLED + WIDTH * SAMPLED_HEIGHT * 2),
            GUARD,
        );
    }

    let mut commands = Vec::new();

    // Build an 8x4 RGBA16 framebuffer: two red rows followed by two blue
    // rows. Fill-cycle rectangles use an inclusive lower-right edge.
    push_command(&mut commands, 0xef00_0000 | (3 << 20), 0);
    push_command(&mut commands, 0xff10_0007, SOURCE);
    push_command(&mut commands, 0xf700_0000, u32::from(RED) * 0x1_0001);
    commands.push(fill_rect(WIDTH - 1, SOURCE_HEIGHT - 1, 0, 0));
    push_command(&mut commands, 0xf700_0000, u32::from(BLUE) * 0x1_0001);
    commands.push(fill_rect(WIDTH - 1, SOURCE_HEIGHT - 1, 0, 2));

    // Load rows 1..=2 from that still-pending framebuffer. At this point the
    // guest-visible SOURCE bytes remain green: RT64 parses the complete DPC
    // range before its render-to-RAM workload runs. Therefore a normal RDRAM
    // texture upload would produce green, while the framebuffer-detection
    // path copies the just-rendered red/blue region from the native target.
    push_command(&mut commands, 0xe700_0000, 0); // PipeSync.
    push_command(&mut commands, 0xfd10_0007, SOURCE);
    push_command(&mut commands, 0xe800_0000, 0); // TileSync.
    push_command(&mut commands, 0xf510_0400, 7 << 24); // RGBA16, line=2, tile 7.
    push_command(&mut commands, 0xe600_0000, 0); // LoadSync.
    push_command(
        &mut commands,
        0xf400_0000 | 4,            // uls=0, ult=1.
        (7 << 24) | (28 << 12) | 8, // lrs=7, lrt=2.
    );

    // Sample the copied 8x2 region one-for-one into a distinct framebuffer.
    push_command(&mut commands, 0xf510_0400, 0x0008_0200); // Clamp S/T, tile 0.
    push_command(&mut commands, 0xf200_0000, (28 << 12) | 4);
    push_command(&mut commands, 0xfc8f_ff1f, 0x88fc_f279); // TEXEL0.
    push_command(&mut commands, 0xef00_0000, 0); // One-cycle.
    push_command(&mut commands, 0xff10_0007, SAMPLED);
    push_command(
        &mut commands,
        0xe400_0000 | ((WIDTH * 4) << 12) | (SAMPLED_HEIGHT * 4),
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

fn require_guards(rdram: &[u8]) -> Result<(), Box<dyn Error>> {
    let view = RdramView::from_storage(rdram);
    for address in [
        SOURCE - 2,
        SOURCE + WIDTH * SOURCE_HEIGHT * 2,
        SAMPLED - 2,
        SAMPLED + WIDTH * SAMPLED_HEIGHT * 2,
    ] {
        let actual = view.read_u16(RdramAddr::from_offset(address));
        if actual != GUARD {
            return Err(io::Error::other(format!(
                "framebuffer write escaped its declared region at {address:#010x}: {actual:#06x}"
            ))
            .into());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "framebuffer behavior evidence requires clean pinned RT64: {source:?}"
        ))
        .into());
    }

    let mut backend = Rt64Backend::new()
        .with_runtime_settings(RenderRuntimeSettings::default())
        .with_emulator_settings(RenderEmulatorSettings {
            post_blend_noise: false,
            post_blend_noise_negative: false,
            framebuffer_render_to_ram: true,
            framebuffer_copy_with_gpu: true,
        });
    backend.create(&RenderConfig::new(WIDTH, SOURCE_HEIGHT))?;

    let (mut rdram, end) = fixture();
    let source_before = pixels(&rdram, SOURCE, WIDTH * SOURCE_HEIGHT);
    let sampled_before = pixels(&rdram, SAMPLED, WIDTH * SAMPLED_HEIGHT);
    if source_before.iter().any(|pixel| *pixel != STALE_SOURCE)
        || sampled_before.iter().any(|pixel| *pixel != STALE_SAMPLED)
    {
        return Err(io::Error::other("fixture seed regions are not uniform").into());
    }

    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, SAMPLED)?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("framebuffer behavior fixture returned {status:?}")).into(),
        );
    }

    let source_after = pixels(&rdram, SOURCE, WIDTH * SOURCE_HEIGHT);
    let sampled_after = pixels(&rdram, SAMPLED, WIDTH * SAMPLED_HEIGHT);
    let expected_source: Vec<u16> = (0..SOURCE_HEIGHT)
        .flat_map(|row| {
            let color = if row < 2 { RED } else { BLUE };
            std::iter::repeat_n(color, WIDTH as usize)
        })
        .collect();
    let expected_sampled: Vec<u16> = [RED, BLUE]
        .into_iter()
        .flat_map(|color| std::iter::repeat_n(color, WIDTH as usize))
        .collect();

    if source_after != expected_source {
        return Err(io::Error::other(format!(
            "native framebuffer did not reach game-visible RDRAM: expected={}, actual={}",
            digest_u16(&expected_source),
            digest_u16(&source_after)
        ))
        .into());
    }
    if sampled_after != expected_sampled {
        return Err(io::Error::other(format!(
            "framebuffer region copy did not sample rendered rows 1..=2: expected={}, actual={}",
            digest_u16(&expected_sampled),
            digest_u16(&sampled_after)
        ))
        .into());
    }
    if sampled_after.contains(&STALE_SOURCE) {
        return Err(io::Error::other(
            "sampled framebuffer contains stale guest-memory texels instead of the native region",
        )
        .into());
    }
    if source_after == source_before || sampled_after == sampled_before {
        return Err(io::Error::other(
            "render-to-RAM synchronization did not produce both required guest-visible transitions",
        )
        .into());
    }
    require_guards(&rdram)?;

    println!(
        "source_before={} source_after={} sampled_before={} sampled_after={} source_transitions={} sampled_transitions={} sampled_region=({},{})->({},{}), source={:?}",
        digest_u16(&source_before),
        digest_u16(&source_after),
        digest_u16(&sampled_before),
        digest_u16(&sampled_after),
        source_before
            .iter()
            .zip(&source_after)
            .filter(|(before, after)| before != after)
            .count(),
        sampled_before
            .iter()
            .zip(&sampled_after)
            .filter(|(before, after)| before != after)
            .count(),
        0,
        1,
        WIDTH - 1,
        2,
        source,
    );
    Ok(())
}
