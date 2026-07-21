//! Synthetic, non-ROM causal evidence for RT64's F3DEX force-branch enhancement.

use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, OsTask, RenderBackend, RenderConfig, RenderEnhancementSettings, RenderFiltering,
    RenderGraphicsApi, RenderPolicyApply, RenderRuntimeSettings, ViFilterControl, ViPixelType,
    ViPresentation, M_GFXTASK,
};
use fn64_render_rt64::{
    gbi, Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SEGMENT: u8 = 6;
const SEGMENT_BASE: usize = 0x0000_1000;
const VERTICES: usize = SEGMENT_BASE;
const PROJECTION: usize = SEGMENT_BASE + 0x0200;
const MODEL: usize = SEGMENT_BASE + 0x0240;
const VIEWPORT: usize = SEGMENT_BASE + 0x0280;
const ROOT_DISPLAY_LIST: usize = 0x0000_3000;
const BRANCH_DISPLAY_LIST: usize = 0x0000_3400;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const CONTROL_POLICY: &str = "0ae411439f1b742ee2017a8f537212767925b71810b4813461becefdee40f3e9";
const FORCED_POLICY: &str = "62563b745de9c410e35f8b472388eb51c7146c9260aa748688de8b11c5547b97";
const CONTROL_PIXELS: &str = "b7116e2234e90cc2eaa468cd8506204c1015285bcb03c5b5672c118c38b22e61";
const FORCED_PIXELS: &str = "17899a3ce23323c0c0c84b4d26afa12a5d0664ab85dd7cbe22156f5569c1692b";
const TRIANGLE_PIXELS: u32 = 161;

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
        ([-5_i16, -5_i16, 0_i16], [255_u8, 255_u8, 255_u8, 255_u8]),
        ([5, -5, 0], [255, 255, 255, 255]),
        ([0, 6, 0], [255, 255, 255, 255]),
    ];
    for (index, (position, color)) in vertices.into_iter().enumerate() {
        let offset = VERTICES + index * 16;
        wr_i16(rdram, offset, position[0]);
        wr_i16(rdram, offset + 2, position[1]);
        wr_i16(rdram, offset + 4, position[2]);
        for (channel, value) in color.into_iter().enumerate() {
            wr_u8(rdram, offset + 12 + channel, value);
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

    let mut root = Vec::new();
    push(
        &mut root,
        ((gbi::G_MOVEWORD as u32) << 24) | (0x06 << 16) | (u32::from(SEGMENT) * 4),
        SEGMENT_BASE as u32,
    );
    push(
        &mut root,
        ((gbi::G_MOVEMEM as u32) << 24) | (1 << 19) | 8,
        (u32::from(SEGMENT) << 24) | 0x0280,
    );
    push(&mut root, 0xff10_0000 | (WIDTH - 1), TARGET as u32);
    push(&mut root, 0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4));
    push(&mut root, 0xef30_00f0, 0);
    push(&mut root, 0xf700_0000, 0x0001_0001);
    push(
        &mut root,
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
        0,
    );
    push(&mut root, 0xe700_0000, 0);
    let matrix_length = (((64_u32 - 1) / 8) & 0x1f) << 19;
    push(
        &mut root,
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x07,
        (u32::from(SEGMENT) << 24) | 0x0200,
    );
    push(
        &mut root,
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x03,
        (u32::from(SEGMENT) << 24) | 0x0240,
    );
    push(&mut root, 0xfcff_ffff, 0xfffd_f6fb);
    push(&mut root, 0xef00_00f0, 0);
    push(
        &mut root,
        ((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        u32::from(SEGMENT) << 24,
    );
    // Public gSPBranchLessZraw envelope: stage the branch target in
    // G_RDPHALF_1, then compare vertex zero against raw screen-Z zero. The
    // transformed vertex has positive screen Z, so the console condition is
    // false and only the enhancement can select the branch target.
    push(&mut root, 0xe100_0000, BRANCH_DISPLAY_LIST as u32);
    push(&mut root, 0x0400_0000, 0);
    push(&mut root, 0xfa00_0000, 0xf800_00ff);
    push(
        &mut root,
        ((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    push(&mut root, 0xe900_0000, 0);
    push(&mut root, (gbi::G_ENDDL as u32) << 24, 0);
    for (index, word) in root.into_iter().enumerate() {
        wr_u32(rdram, ROOT_DISPLAY_LIST + index * 4, word);
    }

    let mut branch = Vec::new();
    push(&mut branch, 0xfa00_0000, 0x00f8_00ff);
    push(
        &mut branch,
        ((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    push(&mut branch, 0xe900_0000, 0);
    push(&mut branch, (gbi::G_ENDDL as u32) << 24, 0);
    for (index, word) in branch.into_iter().enumerate() {
        wr_u32(rdram, BRANCH_DISPLAY_LIST + index * 4, word);
    }
}

fn settings() -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    }
}

fn presentation() -> ViPresentation {
    ViPresentation {
        noise_seed: 0x4252_414e,
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
            "synthetic force-branch evidence bypassed production microcode admission",
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
    backend.process_synthetic_hfr_f3dex2(rdram, ROOT_DISPLAY_LIST as u32, TARGET as u32, 60)?;
    backend.present_physical_compatibility(&*rdram, presentation())?;
    let pixels = backend.presented_pixels()?;
    if pixels.width != WIDTH
        || pixels.height != HEIGHT
        || pixels.row_bytes != WIDTH * 4
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
    {
        return Err(
            io::Error::other(format!("force-branch capture layout changed: {pixels:?}")).into(),
        );
    }
    let (red, green) = classify(&pixels);
    let policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("force-branch capture has no active policy"))?
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
            let expected = backend.configured_runtime_policy().sha256();
            if policy_sha256 == expected {
                Ok(policy_sha256)
            } else {
                Err(io::Error::other("force-branch live policy identity mismatched").into())
            }
        }
        result => Err(io::Error::other(format!(
            "force-branch enhancement did not apply live: {result:?}"
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
            io::Error::other("force-branch evidence requires clean pinned Metal RT64").into(),
        );
    }

    let disabled = RenderEnhancementSettings::default();
    let enabled = RenderEnhancementSettings {
        f3dex_force_branch: true,
        ..disabled.clone()
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(settings());
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let mut rdram = vec![0_u8; RDRAM_LEN];
    require_production_rejection(&mut backend, &mut rdram)?;

    let control = capture(&mut backend, &mut rdram)?;
    let enabled_policy = apply(&mut backend, &enabled)?;
    let forced = capture(&mut backend, &mut rdram)?;
    let restored_policy = apply(&mut backend, &disabled)?;
    let restored = capture(&mut backend, &mut rdram)?;
    require_production_rejection(&mut backend, &mut rdram)?;

    if control.red == 0
        || control.green != 0
        || forced.green == 0
        || forced.red != 0
        || restored != control
        || forced.pixels == control.pixels
        || enabled_policy != forced.policy
        || restored_policy != restored.policy
        || enabled_policy == restored_policy
        || hex(&control.policy) != CONTROL_POLICY
        || hex(&forced.policy) != FORCED_POLICY
        || hex(&control.pixels) != CONTROL_PIXELS
        || hex(&forced.pixels) != FORCED_PIXELS
        || control.red != TRIANGLE_PIXELS
        || forced.green != TRIANGLE_PIXELS
    {
        return Err(io::Error::other(format!(
            "force-branch causal evidence mismatch: control={control:?}, forced={forced:?}, restored={restored:?}"
        ))
        .into());
    }
    println!(
        "RT64 force-branch pass: control={} forced={} pixels={}/{} colors=red:{}/green:{}",
        hex(&control.policy),
        hex(&forced.policy),
        hex(&control.pixels),
        hex(&forced.pixels),
        control.red,
        forced.green,
    );
    Ok(())
}
