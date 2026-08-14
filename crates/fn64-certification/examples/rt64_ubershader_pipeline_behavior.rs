use std::collections::BTreeSet;
use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig, RenderFiltering,
    RenderGraphicsApi, RenderRuntimeSettings, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64SourceProvenance, Rt64UbershaderEvidence,
};
use fn64_runtime::{RdramAddr, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const RED_TEXTURE: u32 = 0x1000;
const GREEN_TEXTURE: u32 = 0x1100;
const TARGET: u32 = 0x10_0000;
const DEPTH: u32 = 0x14_0000;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const BAND_WIDTH: u32 = WIDTH / 5;
const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";

const RED: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const GREEN: [u8; 4] = [0x00, 0xff, 0x00, 0xff];
const BLUE: [u8; 4] = [0xff, 0x00, 0x00, 0xff];
const YELLOW: [u8; 4] = [0x00, 0xff, 0xff, 0xff];
const MAGENTA: [u8; 4] = [0xff, 0x00, 0xff, 0xff];

fn push(commands: &mut Vec<(u32, u32)>, word0: u32, word1: u32) {
    commands.push((word0, word1));
}

fn fill_rect(commands: &mut Vec<(u32, u32)>, ulx: u32, lrx: u32) {
    push(
        commands,
        0xf600_0000 | ((lrx * 4) << 12) | (HEIGHT * 4),
        (ulx * 4) << 12,
    );
}

fn texture_rect(commands: &mut Vec<(u32, u32)>, ulx: u32, lrx: u32) {
    push(
        commands,
        0xe400_0000 | ((lrx * 4) << 12) | (HEIGHT * 4),
        (ulx * 4) << 12,
    );
    push(commands, 0, 0x0400_0400);
}

fn load_one_texel(commands: &mut Vec<(u32, u32)>, address: u32) {
    push(commands, 0xfd10_0000, address); // RGBA16 texture image, width 1.
    push(commands, 0xe800_0000, 0); // TileSync.
    push(commands, 0xf510_0000, 7 << 24); // Load tile 7.
    push(commands, 0xe600_0000, 0); // LoadSync.
    push(commands, 0xf300_0000, 7 << 24); // One texel into TMEM.
    push(commands, 0xe800_0000, 0); // TileSync.
    push(commands, 0xf510_0200, 0x0008_0200); // Clamp RGBA16 render tile 0.
    push(commands, 0xf200_0000, 0); // 1x1 render extent.
}

fn fixture() -> (Vec<u8>, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        view.write_u16(RdramAddr::from_offset(RED_TEXTURE), 0xf801);
        view.write_u16(RdramAddr::from_offset(GREEN_TEXTURE), 0x07c1);
        for index in 0..(WIDTH * HEIGHT) {
            view.write_u16(RdramAddr::from_offset(DEPTH + index * 2), 0xfffc);
        }
    }

    let mut commands = Vec::new();
    push(&mut commands, 0xef30_00f0, 0); // Fill cycle, deterministic dither off.
    push(&mut commands, 0xff10_0000 | (WIDTH - 1), TARGET);
    push(&mut commands, 0xf700_0000, 0x0001_0001); // Opaque black RGBA16.
    push(
        &mut commands,
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
        0,
    );
    push(&mut commands, 0xe700_0000, 0); // PipeSync into raster calls.

    // Two novel texture descriptions differ in source image and filter mode.
    load_one_texel(&mut commands, RED_TEXTURE);
    push(&mut commands, 0xfc8f_ff1f, 0x88fc_f279); // TEXEL0 RGBA.
    push(&mut commands, 0xef00_00f0, 0); // Point-filtered one-cycle.
    texture_rect(&mut commands, 0, BAND_WIDTH);

    push(&mut commands, 0xe700_0000, 0);
    load_one_texel(&mut commands, GREEN_TEXTURE);
    push(&mut commands, 0xef00_20f0, 0); // Average-filtered one-cycle.
    texture_rect(&mut commands, BAND_WIDTH, BAND_WIDTH * 2);

    // Primitive output with the public two-source blend tuple and FORCE_BL.
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xfcff_ffff, 0xfffd_f6fb); // PRIMITIVE RGBA.
    push(&mut commands, 0xfa00_0000, 0x0000_ffff); // Opaque blue.
    push(&mut commands, 0xef00_00f0, (1 << 22) | (1 << 20) | 0x4000);
    fill_rect(&mut commands, BAND_WIDTH * 2, BAND_WIDTH * 3);

    // A distinct precreated ubershader PSO: depth update, primitive Z.
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xfe00_0000, DEPTH);
    push(&mut commands, 0xee00_0000, (8 << 16) | 4);
    push(&mut commands, 0xfa00_0000, 0xffff_00ff); // Opaque yellow.
    push(&mut commands, 0xef00_00f0, 0x24); // Z_UPD | G_ZS_PRIM.
    fill_rect(&mut commands, BAND_WIDTH * 3, BAND_WIDTH * 4);

    // Another distinct precreated PSO: additive coverage-wrap destination.
    push(&mut commands, 0xe700_0000, 0);
    push(&mut commands, 0xfa00_0000, 0xff00_ffff); // Opaque magenta.
    push(&mut commands, 0xef00_00f0, 0x100); // CVG_DST_WRAP.
    fill_rect(&mut commands, BAND_WIDTH * 4, WIDTH);
    push(&mut commands, 0xe900_0000, 0); // FullSync.

    let end = COMMANDS + commands.len() * 8;
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = COMMANDS + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    (rdram, end as u32)
}

fn expected_pixel(x: u32, y: u32) -> [u8; 4] {
    if y >= 236 {
        [0x00, 0x00, 0x00, 0xff]
    } else {
        [RED, GREEN, BLUE, YELLOW, MAGENTA][(x / BAND_WIDTH).min(4) as usize]
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn require_evidence(evidence: &Rt64UbershaderEvidence) -> Result<(), Box<dyn Error>> {
    let call_count = evidence.raster_call_count as usize;
    let unique_descriptions = evidence.shader_hashes[..call_count]
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let pipeline_indices = &evidence.pipeline_state_indices[..call_count];
    let critical_events = evidence.caller_construction_events
        + evidence.workload_construction_events
        + evidence.present_construction_events;
    if evidence.workload_id == 0
        || evidence.present_id == 0
        || evidence.descriptor_digest == 0
        || evidence.pipeline_digest == 0
        || evidence.precreated_pipeline_count != 8
        || evidence.raster_call_count != 5
        || evidence.matched_ubershader_call_count != evidence.raster_call_count
        || !evidence.ubershaders_only
        || unique_descriptions.len() != 5
        || !pipeline_indices.contains(&0)
        || !pipeline_indices.contains(&2)
        || !pipeline_indices.contains(&4)
        || evidence.pipeline_identities[..call_count].contains(&0)
        || critical_events != 0
        || evidence.graphics_pipeline_construction_events != evidence.background_construction_events
    {
        return Err(io::Error::other(format!(
            "ubershader mechanism evidence is not exact: {evidence:#?}, unique={unique_descriptions:?}"
        ))
        .into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_id != PINNED_SOURCE
        || source.source_provenance != Rt64SourceProvenance::GitClean
        || source.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(format!(
            "ubershader evidence requires clean pinned Metal RT64: {source:?}"
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
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    if backend.active_settings() != Some(&runtime) {
        return Err(
            io::Error::other("RT64 did not activate exact ubershader fixture policy").into(),
        );
    }
    backend.enable_present_capture()?;
    backend.enable_ubershader_evidence()?;

    let (mut rdram, end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET, true)?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("ubershader raw workload returned {status:?}")).into(),
        );
    }
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: 0x55aa_1234,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;

    let capture = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    let first_mismatch = capture
        .bytes
        .chunks_exact(4)
        .enumerate()
        .position(|(index, pixel)| {
            let x = index as u32 % WIDTH;
            let y = index as u32 / WIDTH;
            pixel != expected_pixel(x, y)
        });
    if capture.width != WIDTH
        || capture.height != HEIGHT
        || capture.row_bytes != WIDTH * 4
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.present_id == 0
        || capture.present_id != selection.present_id
        || selection.target_address != TARGET
        || selection.target_width != WIDTH
        || selection.target_height != HEIGHT
        || selection.target_size != 2
        || first_mismatch.is_some()
    {
        let first_actual = first_mismatch.map(|index| &capture.bytes[index * 4..index * 4 + 4]);
        return Err(io::Error::other(format!(
            "ubershader output/present association differs: capture={}x{} id={} selection={selection:?} first_mismatch={first_mismatch:?} first_actual={first_actual:02x?} digest={}",
            capture.width,
            capture.height,
            capture.present_id,
            digest(&capture.bytes)
        ))
        .into());
    }

    let evidence = backend.ubershader_evidence()?;
    require_evidence(&evidence)?;
    if evidence.present_id != capture.present_id {
        return Err(io::Error::other(format!(
            "pipeline evidence present {} differs from capture {}",
            evidence.present_id, capture.present_id
        ))
        .into());
    }

    println!(
        "workload={} present={} descriptors={:016x} pipelines={:016x} indices={:?} specialized={} construction_total={} background={} critical=[{},{},{}] capture={} source={source:?}",
        evidence.workload_id,
        evidence.present_id,
        evidence.descriptor_digest,
        evidence.pipeline_digest,
        &evidence.pipeline_state_indices[..evidence.raster_call_count as usize],
        evidence.specialized_shader_count,
        evidence.graphics_pipeline_construction_events,
        evidence.background_construction_events,
        evidence.caller_construction_events,
        evidence.workload_construction_events,
        evidence.present_construction_events,
        digest(&capture.bytes),
    );
    fn64_boot_harness::emit_rt64_platform_child_identity(
        source.source_id,
        source.is_source_authoritative(),
        source.adapter_source_sha256,
        source.post_vi_api,
    )?;
    Ok(())
}
