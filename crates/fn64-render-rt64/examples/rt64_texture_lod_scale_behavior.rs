//! Synthetic, non-ROM causal evidence for RT64's texture-LOD scale enhancement.

use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, OsTask, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi, RenderPolicyApply,
    RenderResolution, RenderRuntimeSettings, ResolutionMultiplier, ViFilterControl, ViPixelType,
    ViPresentation, M_GFXTASK,
};
use fn64_render_reference::gbi;
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SEGMENT: u8 = 6;
const SEGMENT_BASE: usize = 0x0000_1000;
const VERTICES: usize = SEGMENT_BASE;
const BASE_TEXTURE: usize = SEGMENT_BASE + 0x0100;
const MIP_TEXTURE: usize = SEGMENT_BASE + 0x0200;
const PROJECTION: usize = SEGMENT_BASE + 0x0300;
const MODEL: usize = SEGMENT_BASE + 0x0340;
const VIEWPORT: usize = SEGMENT_BASE + 0x0380;
const DISPLAY_LIST: usize = 0x0000_3000;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const WARM_PIXELS: &str = "a9f0945067d0ff1cc63378ec82eaf582c8762318e5552a9b66131731822fd12f";
const CONTROL_POLICY: &str = "7a426cc2a30f5b5f16bf356996e65591934bf617363411898bc8d72b5558baa5";
const SCALED_POLICY: &str = "25ac93b536bcfc3b7b07094106d44d5f2cf5ee988931fb44b5985b30beb6fc3b";
const CONTROL_PIXELS: &str = "254d73f02da9dfed4700f700b6af553d41ef7f4e680a793eaacaf2ae04b0e22c";
const SCALED_PIXELS: &str = "cd42bc830ce59f02afd8734cc2d8b6dec6f0dd459f23b33473cfaec88203b2f6";
const TRIANGLE_PIXELS: u32 = 259;

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

fn push(commands: &mut Vec<u32>, word0: u32, word1: u32) {
    commands.push(word0);
    commands.push(word1);
}

fn write_fixture(rdram: &mut [u8]) {
    let vertices = [
        ([-8_i16, -6_i16, 0_i16], [0_i16, 0_i16]),
        ([8, -6, 0], [64 * 32, 0]),
        ([0, 8, 0], [32 * 32, 56 * 32]),
    ];
    for (index, (position, texture)) in vertices.into_iter().enumerate() {
        let offset = VERTICES + index * 16;
        wr_i16(rdram, offset, position[0]);
        wr_i16(rdram, offset + 2, position[1]);
        wr_i16(rdram, offset + 4, position[2]);
        wr_i16(rdram, offset + 8, texture[0]);
        wr_i16(rdram, offset + 10, texture[1]);
        for channel in 0..4 {
            wr_u8(rdram, offset + 12 + channel, 255);
        }
    }

    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        for index in 0..64_u32 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(BASE_TEXTURE as u32 + index * 2),
                0xf801,
            );
        }
        for index in 0..16_u32 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(MIP_TEXTURE as u32 + index * 2),
                0x07c1,
            );
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
    model[15] = 1.0;
    write_matrix(rdram, MODEL, model);
    for (index, value) in [
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        wr_i16(rdram, VIEWPORT + index * 2, value);
    }

    let mut commands = Vec::new();
    push(
        &mut commands,
        ((gbi::G_MOVEWORD as u32) << 24) | (0x06 << 16) | (u32::from(SEGMENT) * 4),
        SEGMENT_BASE as u32,
    );
    push(
        &mut commands,
        ((gbi::G_MOVEMEM as u32) << 24) | (1 << 19) | 8,
        (u32::from(SEGMENT) << 24) | 0x0380,
    );
    push(&mut commands, 0xff10_0000 | (WIDTH - 1), TARGET as u32);
    push(&mut commands, 0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4));
    push(&mut commands, 0xef30_00f0, 0);
    push(&mut commands, 0xf700_0000, 0x0001_0001);
    push(
        &mut commands,
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
        0,
    );
    push(&mut commands, 0xe700_0000, 0);

    // Public RDP texture-image/tile/load-block sequence: tile 0 is an 8x8
    // red base image at TMEM word 0; tile 1 is a 4x4 green mip at word 16.
    push(&mut commands, 0xfd10_0007, BASE_TEXTURE as u32);
    push(&mut commands, 0xf510_0000, 7 << 24);
    push(&mut commands, 0xf300_0000, (7 << 24) | (63 << 12) | 0x400);
    push(&mut commands, 0xf510_0400, 0x0008_0200);
    push(&mut commands, 0xf200_0000, (28 << 12) | 28);
    push(&mut commands, 0xfd10_0003, MIP_TEXTURE as u32);
    push(&mut commands, 0xf510_0010, 7 << 24);
    push(&mut commands, 0xf300_0000, (7 << 24) | (15 << 12) | 0x800);
    push(&mut commands, 0xf510_0210, 0x0108_0200);
    push(&mut commands, 0xf200_0000, (1 << 24) | (12 << 12) | 12);

    let matrix_length = (((64_u32 - 1) / 8) & 0x1f) << 19;
    push(
        &mut commands,
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x07,
        (u32::from(SEGMENT) << 24) | 0x0300,
    );
    push(
        &mut commands,
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x03,
        (u32::from(SEGMENT) << 24) | 0x0340,
    );
    // (0-0)*0+TEXEL0 in both combiner slots, G_TL_LOD, and one mip after
    // primitive tile zero. Full texture scale preserves vertex S10.5 values.
    push(&mut commands, 0xfc8f_ff1f, 0x88fc_f279);
    push(&mut commands, 0xef01_00f0, 0);
    push(&mut commands, 0xd700_0802, 0xffff_ffff);
    push(
        &mut commands,
        ((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        u32::from(SEGMENT) << 24,
    );
    push(
        &mut commands,
        ((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    push(&mut commands, 0xe900_0000, 0);
    push(&mut commands, (gbi::G_ENDDL as u32) << 24, 0);
    for (index, word) in commands.into_iter().enumerate() {
        wr_u32(rdram, DISPLAY_LIST + index * 4, word);
    }
}

fn settings() -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(2.0)?,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        filtering: RenderFiltering::Nearest,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn presentation() -> ViPresentation {
    ViPresentation {
        noise_seed: 0x4c4f_4421,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn require_production_rejection(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
) -> Result<(), Box<dyn Error>> {
    let status = backend.process_task(
        rdram,
        &mut fn64_runtime::RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..Default::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "synthetic texture-LOD evidence bypassed production microcode admission",
        )
        .into());
    }
    Ok(())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Observation {
    policy: [u8; 32],
    pixels: [u8; 32],
    red: u32,
    green: u32,
}

fn capture(backend: &mut Rt64Backend, rdram: &mut [u8]) -> Result<Observation, Box<dyn Error>> {
    write_fixture(rdram);
    backend.process_synthetic_hfr_f3dex2(rdram, DISPLAY_LIST as u32, TARGET as u32, 60)?;
    backend.present_physical_compatibility(&*rdram, presentation())?;
    let pixels = backend.presented_pixels()?;
    if pixels.width != WIDTH
        || pixels.height != HEIGHT
        || pixels.row_bytes != WIDTH * 4
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
    {
        return Err(
            io::Error::other(format!("texture-LOD capture layout changed: {pixels:?}")).into(),
        );
    }
    let (red, green) = classify(&pixels);
    let policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("texture-LOD capture has no active policy"))?
        .sha256();
    Ok(Observation {
        policy,
        pixels: Sha256::digest(&pixels.bytes).into(),
        red,
        green,
    })
}

fn classify(pixels: &Rt64PresentedPixels) -> (u32, u32) {
    let mut red = 0;
    let mut green = 0;
    for pixel in pixels.bytes.chunks_exact(4) {
        if pixel[2] > 192 && pixel[1] < 64 && pixel[0] < 64 && pixel[3] > 192 {
            red += 1;
        }
        if pixel[1] > 192 && pixel[2] < 64 && pixel[0] < 64 && pixel[3] > 192 {
            green += 1;
        }
    }
    (red, green)
}

fn apply(
    backend: &mut Rt64Backend,
    enhancement: &RenderEnhancementSettings,
) -> Result<[u8; 32], Box<dyn Error>> {
    match backend.apply_enhancement_settings(enhancement)? {
        RenderPolicyApply::LiveApplied { policy_sha256 } => {
            if policy_sha256 == backend.configured_runtime_policy().sha256() {
                Ok(policy_sha256)
            } else {
                Err(io::Error::other("texture-LOD live policy identity mismatched").into())
            }
        }
        result => Err(io::Error::other(format!(
            "texture-LOD enhancement did not apply live: {result:?}"
        ))
        .into()),
    }
}

fn hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(
            io::Error::other("texture-LOD evidence requires clean pinned Metal RT64").into(),
        );
    }

    let disabled = RenderEnhancementSettings::default();
    let enabled = RenderEnhancementSettings {
        texture_lod_scale: true,
        ..disabled.clone()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(settings()?);
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let mut rdram = vec![0_u8; RDRAM_LEN];
    require_production_rejection(&mut backend, &mut rdram)?;

    let warm = capture(&mut backend, &mut rdram)?;
    let control = capture(&mut backend, &mut rdram)?;
    let enabled_policy = apply(&mut backend, &enabled)?;
    let scaled = capture(&mut backend, &mut rdram)?;
    let restored_policy = apply(&mut backend, &disabled)?;
    let restored = capture(&mut backend, &mut rdram)?;
    require_production_rejection(&mut backend, &mut rdram)?;

    if warm.policy != control.policy
        || hex(&warm.pixels) != WARM_PIXELS
        || warm.red != 0
        || warm.green != 290
        || restored != control
        || scaled == control
        || enabled_policy != scaled.policy
        || restored_policy != restored.policy
        || enabled_policy == restored_policy
        || hex(&control.policy) != CONTROL_POLICY
        || hex(&scaled.policy) != SCALED_POLICY
        || hex(&control.pixels) != CONTROL_PIXELS
        || hex(&scaled.pixels) != SCALED_PIXELS
        || control.red != 0
        || control.green != TRIANGLE_PIXELS
        || scaled.red != TRIANGLE_PIXELS
        || scaled.green != 0
    {
        return Err(io::Error::other(format!(
            "texture-LOD causal evidence mismatch: warm={warm:?}, control={control:?}, scaled={scaled:?}, restored={restored:?}"
        ))
        .into());
    }
    println!(
        "RT64 texture-LOD pass: control={} scaled={} pixels={}/{} colors=green:{}/red:{}",
        hex(&control.policy),
        hex(&scaled.policy),
        hex(&control.pixels),
        hex(&scaled.pixels),
        control.green,
        scaled.red,
    );
    Ok(())
}
