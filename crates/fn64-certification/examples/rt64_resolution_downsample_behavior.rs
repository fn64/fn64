use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, DownsampleMultiplier, FrameStatus, RenderAspectRatio, RenderBackend,
    RenderConfig, RenderFiltering, RenderResolution, RenderRuntimeSettings, RenderSettingsApply,
    ResolutionMultiplier, ViFilterControl, ViPixelType, ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64PresentPixelFormat, Rt64PresentedPixels, Rt64SourceProvenance,
};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TEXTURE: u32 = 0x1000;
const TARGET: u32 = 0x2000;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const DOWNSAMPLE_PRESENT_WIDTH: u32 = 32;
const DOWNSAMPLE_PRESENT_HEIGHT: u32 = 32;
const CONTROL_TEXTURE_SIZE: u32 = 4;
const DIAGONAL_TEXTURE_SIZE: u32 = 16;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const PRESENT_GUEST_CYCLE: u64 = 1;
const CERTIFIED_BASE_SHA256: &str =
    "bdf1766c20ee4127abea7d57741cf0682252e6e32262897e3cda7150ab7f7902";
const NATIVE_DIAGONAL_SHA256: &str =
    "fe756dbf4d56ecb78bc93cdbbdb57e429ceb7b15d206863e6be40e0a261e038f";
const HIGH_DIAGONAL_SHA256: &str =
    "0deba793bc94c4104b12eac980facd350c98699ecd50a56d25dea94848c5baa3";
const HIGH_32_DIAGONAL_SHA256: &str =
    "f06f2043401a7f14a8c2f27faf00e7ff4051eb39a0db743bfe1a571a2769c491";
const DOWNSAMPLE_32_DIAGONAL_SHA256: &str =
    "867d0aabc98b566a8af7cdf6ae352df1cb2bbe5d6a8fbc8d9036bbc9f0642299";

#[derive(Copy, Clone)]
enum TexturePattern {
    CertifiedControl,
    Diagonal,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PixelStats {
    bounds: [u32; 4],
    red_pixels: usize,
    mixed_red_pixels: usize,
}

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fixture(pattern: TexturePattern) -> (Vec<u8>, u32) {
    let commands = match pattern {
        TexturePattern::CertifiedControl => [
            (0xfc8f_ff1f, 0x88fc_f279),
            (0xff10_000f, TARGET),
            (0xfd10_0003, TEXTURE),
            (0xf510_0000, 7 << 24),
            (0xf300_0000, (7 << 24) | (15 << 12) | 0x800),
            (0xf510_0200, 0x0008_0200),
            (0xf200_0000, 0x0000_c00c),
            (0xe400_0000 | ((WIDTH * 4) << 12) | (HEIGHT * 4), 0),
            (0, 0x0100_0100),
            (0xe900_0000, 0),
        ],
        TexturePattern::Diagonal => [
            (0xfc8f_ff1f, 0x88fc_f279),
            (0xff10_000f, TARGET),
            (0xfd10_000f, TEXTURE),
            (0xf510_0000, 7 << 24),
            (0xf300_0000, (7 << 24) | (255 << 12) | 0x200),
            (0xf510_0800, 0x0008_0200),
            (0xf200_0000, (60 << 12) | 60),
            (0xe400_0000 | (57 << 12) | 58, (5 << 12) | 6),
            (0, 0x04ec_04ec),
            (0xe900_0000, 0),
        ],
    };
    let mut rdram = vec![0; RDRAM_LEN];
    let certified = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001, 0xffff, 0x003f, 0x07c1,
        0xf801, 0x0001, 0xffc1, 0xf83f, 0x07ff,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let texture_size = match pattern {
            TexturePattern::CertifiedControl => CONTROL_TEXTURE_SIZE,
            TexturePattern::Diagonal => DIAGONAL_TEXTURE_SIZE,
        };
        for y in 0..texture_size {
            for x in 0..texture_size {
                let color = match pattern {
                    TexturePattern::CertifiedControl => certified[(y * texture_size + x) as usize],
                    TexturePattern::Diagonal => {
                        if x > y {
                            RED
                        } else {
                            BLUE
                        }
                    }
                };
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + (y * texture_size + x) * 2),
                    color,
                );
            }
        }
    }
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    (rdram, (COMMANDS + commands.len() * 8) as u32)
}

fn settings(
    resolution_multiplier: f64,
    downsample_multiplier: u32,
) -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        resolution: RenderResolution::Manual,
        resolution_multiplier: ResolutionMultiplier::new(resolution_multiplier)?,
        downsample_multiplier: DownsampleMultiplier::new(downsample_multiplier)?,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(WIDTH as f64 / HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn apply_settings(
    backend: &mut Rt64Backend,
    requested: &RenderRuntimeSettings,
    framebuffers_discarded: bool,
) -> Result<[u8; 32], Box<dyn Error>> {
    let outcome = backend.apply_runtime_settings(requested)?;
    let expected = RenderSettingsApply::LiveApplied {
        settings_sha256: requested.sha256(),
        framebuffers_discarded,
    };
    if outcome != expected {
        return Err(io::Error::other(format!(
            "unexpected live settings result: expected {expected:?}, got {outcome:?}"
        ))
        .into());
    }
    if backend.configured_settings() != requested || backend.active_settings() != Some(requested) {
        return Err(io::Error::other(
            "configured and active RT64 settings did not retain the requested exact image",
        )
        .into());
    }
    active_policy_sha256(backend, requested)
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
            "active RT64 policy did not exactly match the complete configured policy",
        )
        .into());
    }
    Ok(active.sha256())
}

fn render(
    backend: &mut Rt64Backend,
    pattern: TexturePattern,
) -> Result<Rt64PresentedPixels, Box<dyn Error>> {
    let (mut rdram, end) = fixture(pattern);
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "resolution fixture returned {status:?} instead of Complete"
        ))
        .into());
    }
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: PRESENT_GUEST_CYCLE,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    Ok(backend.presented_pixels()?)
}

fn pixel(capture: &Rt64PresentedPixels, x: u32, y: u32) -> [u8; 4] {
    let offset = (y * capture.row_bytes + x * 4) as usize;
    capture.bytes[offset..offset + 4]
        .try_into()
        .expect("validated tightly packed BGRA8 capture")
}

fn is_red(pixel: [u8; 4]) -> bool {
    let [blue, green, red, alpha] = pixel;
    alpha >= 192 && red >= 96 && red > green.saturating_add(32) && red > blue.saturating_add(32)
}

fn is_mixed_red(pixel: [u8; 4]) -> bool {
    is_red(pixel) && pixel[2] < 240
}

fn validate_layout(
    label: &str,
    capture: &Rt64PresentedPixels,
    expected_width: u32,
    expected_height: u32,
) -> Result<(), Box<dyn Error>> {
    let expected_row_bytes = expected_width * 4;
    let expected_len = (expected_row_bytes * expected_height) as usize;
    if capture.width != expected_width
        || capture.height != expected_height
        || capture.row_bytes != expected_row_bytes
        || capture.format != Rt64PresentPixelFormat::Bgra8Unorm
        || capture.bytes.len() != expected_len
    {
        return Err(
            io::Error::other(format!("{label} presentation layout mismatch: {capture:?}")).into(),
        );
    }
    Ok(())
}

fn inspect(
    label: &str,
    capture: &Rt64PresentedPixels,
    expected_width: u32,
    expected_height: u32,
) -> Result<PixelStats, Box<dyn Error>> {
    validate_layout(label, capture, expected_width, expected_height)?;

    let mut min_x = expected_width;
    let mut min_y = expected_height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut red_pixels = 0;
    let mut mixed_red_pixels = 0;
    for y in 0..expected_height {
        for x in 0..expected_width {
            let sample = pixel(capture, x, y);
            if is_red(sample) {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                red_pixels += 1;
                mixed_red_pixels += usize::from(is_mixed_red(sample));
            }
        }
    }
    if red_pixels == 0 {
        return Err(
            io::Error::other(format!("{label} rendered no red diagonal-texture pixels")).into(),
        );
    }
    let stats = PixelStats {
        bounds: [min_x, min_y, max_x, max_y],
        red_pixels,
        mixed_red_pixels,
    };
    let (interior_x, interior_y) = match (expected_width, expected_height) {
        (WIDTH, HEIGHT) => (12, 4),
        (DOWNSAMPLE_PRESENT_WIDTH, DOWNSAMPLE_PRESENT_HEIGHT) => (12, 8),
        _ => {
            return Err(io::Error::other(format!(
                "{label} has no certified geometry anchors for {expected_width}x{expected_height}"
            ))
            .into());
        }
    };
    let interior = pixel(capture, interior_x, interior_y);
    let background = pixel(capture, 0, 0);
    if !is_red(interior) || is_red(background) {
        return Err(io::Error::other(format!(
            "{label} did not retain the commanded textured-rectangle interior/background geometry: stats={stats:?}, interior=({interior_x},{interior_y})/{interior:?}, background={background:?}"
        ))
        .into());
    }

    Ok(stats)
}

fn changed_pixels(left: &Rt64PresentedPixels, right: &Rt64PresentedPixels) -> usize {
    left.bytes
        .chunks_exact(4)
        .zip(right.bytes.chunks_exact(4))
        .filter(|(left, right)| left != right)
        .count()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "resolution/downsample evidence requires the clean pinned RT64 source: {source:?}"
        ))
        .into());
    }

    let native_settings = settings(1.0, 1)?;
    let high_settings = settings(2.0, 1)?;
    let downsample_settings = settings(2.0, 2)?;
    let mut backend = Rt64Backend::new().with_runtime_settings(native_settings.clone());
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;

    let control = render(&mut backend, TexturePattern::CertifiedControl)?;
    validate_layout("certified control", &control, WIDTH, HEIGHT)?;
    let control_digest = digest(&control.bytes);
    println!(
        "phase=control target={TARGET:#010x} id={} dimensions={}x{} row_bytes={} sha256={control_digest}",
        control.present_id, control.width, control.height, control.row_bytes
    );
    if control_digest != CERTIFIED_BASE_SHA256 {
        return Err(io::Error::other(format!(
            "paired certified control digest changed: expected={CERTIFIED_BASE_SHA256}, actual={control_digest}"
        ))
        .into());
    }
    let native_policy_sha256 = active_policy_sha256(&backend, &native_settings)?;
    let native = render(&mut backend, TexturePattern::Diagonal)?;
    let native_stats = inspect("native 1x", &native, WIDTH, HEIGHT)?;
    let source_changed_pixels = changed_pixels(&control, &native);
    let native_digest = digest(&native.bytes);
    println!(
        "phase=native target={TARGET:#010x} id={} bounds={:?} red={} mixed={} changed_from_control={} policy_sha256={} sha256={}",
        native.present_id,
        native_stats.bounds,
        native_stats.red_pixels,
        native_stats.mixed_red_pixels,
        source_changed_pixels,
        hex(&native_policy_sha256),
        native_digest
    );
    let expected_native = PixelStats {
        bounds: [4, 4, 14, 8],
        red_pixels: 32,
        mixed_red_pixels: 0,
    };
    if native_stats != expected_native
        || source_changed_pixels != 167
        || native_digest != NATIVE_DIAGONAL_SHA256
    {
        return Err(io::Error::other(format!(
            "native diagonal evidence drifted: stats={native_stats:?}, changed_from_control={source_changed_pixels}, sha256={native_digest}"
        ))
        .into());
    }

    let high_policy_sha256 = apply_settings(&mut backend, &high_settings, true)?;
    let high = render(&mut backend, TexturePattern::Diagonal)?;
    let high_stats = inspect("high-resolution 2x", &high, WIDTH, HEIGHT)?;
    let high_changed_pixels = changed_pixels(&native, &high);
    let high_digest = digest(&high.bytes);
    println!(
        "phase=high_2x target={TARGET:#010x} id={} bounds={:?} red={} mixed={} changed_from_native={} settings_sha256={} policy_sha256={} sha256={}",
        high.present_id,
        high_stats.bounds,
        high_stats.red_pixels,
        high_stats.mixed_red_pixels,
        high_changed_pixels,
        hex(&high_settings.sha256()),
        hex(&high_policy_sha256),
        high_digest
    );
    let expected_high = PixelStats {
        bounds: [4, 4, 14, 9],
        red_pixels: 35,
        mixed_red_pixels: 0,
    };
    if high_stats != expected_high
        || high_changed_pixels != 3
        || high_digest != HIGH_DIAGONAL_SHA256
    {
        return Err(io::Error::other(format!(
            "high-resolution diagonal evidence drifted: stats={high_stats:?}, changed_from_native={high_changed_pixels}, sha256={high_digest}"
        ))
        .into());
    }

    let downsample_policy_sha256 = apply_settings(&mut backend, &downsample_settings, false)?;
    let downsampled = render(&mut backend, TexturePattern::Diagonal)?;
    let downsampled_stats = inspect(
        "high-resolution 2x/downsample 2x",
        &downsampled,
        WIDTH,
        HEIGHT,
    )?;
    let downsample_changed_pixels = changed_pixels(&high, &downsampled);
    let downsample_digest = digest(&downsampled.bytes);
    println!(
        "phase=downsample_2x target={TARGET:#010x} id={} bounds={:?} red={} mixed={} changed_from_high={} settings_sha256={} policy_sha256={} sha256={}",
        downsampled.present_id,
        downsampled_stats.bounds,
        downsampled_stats.red_pixels,
        downsampled_stats.mixed_red_pixels,
        downsample_changed_pixels,
        hex(&downsample_settings.sha256()),
        hex(&downsample_policy_sha256),
        downsample_digest
    );

    if !(control.present_id < native.present_id
        && native.present_id < high.present_id
        && high.present_id < downsampled.present_id)
    {
        return Err(io::Error::other(format!(
            "presentation IDs did not advance: control={}, native={}, high={}, downsampled={}",
            control.present_id, native.present_id, high.present_id, downsampled.present_id
        ))
        .into());
    }
    println!(
        "rt64 resolution/downsample behavior: presentation={}x{} row_bytes={} control={{id:{},sha256:{}}} native={{id:{},bounds:{:?},red:{},mixed:{},changed_from_control:{},policy_sha256:{},sha256:{}}} high_2x={{id:{},bounds:{:?},red:{},mixed:{},changed_from_native:{},settings_sha256:{},policy_sha256:{},sha256:{}}} downsample_2x={{id:{},bounds:{:?},red:{},mixed:{},changed_from_high:{},settings_sha256:{},policy_sha256:{},sha256:{}}}",
        WIDTH,
        HEIGHT,
        WIDTH * 4,
        control.present_id,
        control_digest,
        native.present_id,
        native_stats.bounds,
        native_stats.red_pixels,
        native_stats.mixed_red_pixels,
        source_changed_pixels,
        hex(&native_policy_sha256),
        native_digest,
        high.present_id,
        high_stats.bounds,
        high_stats.red_pixels,
        high_stats.mixed_red_pixels,
        high_changed_pixels,
        hex(&high_settings.sha256()),
        hex(&high_policy_sha256),
        high_digest,
        downsampled.present_id,
        downsampled_stats.bounds,
        downsampled_stats.red_pixels,
        downsampled_stats.mixed_red_pixels,
        downsample_changed_pixels,
        hex(&downsample_settings.sha256()),
        hex(&downsample_policy_sha256),
        downsample_digest,
    );

    backend.resize(DOWNSAMPLE_PRESENT_WIDTH, DOWNSAMPLE_PRESENT_HEIGHT);
    let high_32_policy_sha256 = apply_settings(&mut backend, &high_settings, false)?;
    let resize_transition = render(&mut backend, TexturePattern::CertifiedControl)?;
    if ![
        (WIDTH, HEIGHT),
        (DOWNSAMPLE_PRESENT_WIDTH, DOWNSAMPLE_PRESENT_HEIGHT),
    ]
    .contains(&(resize_transition.width, resize_transition.height))
    {
        return Err(io::Error::other(format!(
            "32x32 resize handoff exposed unexpected layout: {resize_transition:?}"
        ))
        .into());
    }
    validate_layout(
        "32x32 resize handoff",
        &resize_transition,
        resize_transition.width,
        resize_transition.height,
    )?;
    println!(
        "phase=resize_handoff id={} dimensions={}x{} row_bytes={} sha256={}",
        resize_transition.present_id,
        resize_transition.width,
        resize_transition.height,
        resize_transition.row_bytes,
        digest(&resize_transition.bytes)
    );

    let high_32 = render(&mut backend, TexturePattern::Diagonal)?;
    let high_32_stats = inspect(
        "32x32 high-resolution 2x",
        &high_32,
        DOWNSAMPLE_PRESENT_WIDTH,
        DOWNSAMPLE_PRESENT_HEIGHT,
    )?;
    let high_32_digest = digest(&high_32.bytes);
    println!(
        "phase=high_2x_present_32 id={} bounds={:?} red={} mixed={} settings_sha256={} policy_sha256={} sha256={}",
        high_32.present_id,
        high_32_stats.bounds,
        high_32_stats.red_pixels,
        high_32_stats.mixed_red_pixels,
        hex(&high_settings.sha256()),
        hex(&high_32_policy_sha256),
        high_32_digest
    );
    let expected_32 = PixelStats {
        bounds: [5, 7, 14, 12],
        red_pixels: 33,
        mixed_red_pixels: 0,
    };
    if high_32_stats != expected_32 || high_32_digest != HIGH_32_DIAGONAL_SHA256 {
        return Err(io::Error::other(format!(
            "32x32 high-resolution evidence drifted: stats={high_32_stats:?}, sha256={high_32_digest}"
        ))
        .into());
    }

    let downsample_32_policy_sha256 = apply_settings(&mut backend, &downsample_settings, false)?;
    let downsample_32 = render(&mut backend, TexturePattern::Diagonal)?;
    let downsample_32_stats = inspect(
        "32x32 high-resolution 2x/downsample 2x",
        &downsample_32,
        DOWNSAMPLE_PRESENT_WIDTH,
        DOWNSAMPLE_PRESENT_HEIGHT,
    )?;
    let downsample_32_digest = digest(&downsample_32.bytes);
    let downsample_32_changed_pixels = changed_pixels(&high_32, &downsample_32);
    println!(
        "phase=downsample_2x_present_32 id={} bounds={:?} red={} mixed={} changed_from_high={} settings_sha256={} policy_sha256={} sha256={}",
        downsample_32.present_id,
        downsample_32_stats.bounds,
        downsample_32_stats.red_pixels,
        downsample_32_stats.mixed_red_pixels,
        downsample_32_changed_pixels,
        hex(&downsample_settings.sha256()),
        hex(&downsample_32_policy_sha256),
        downsample_32_digest
    );
    if downsample_32_stats != expected_32
        || downsample_32_changed_pixels != 7
        || downsample_32_digest != DOWNSAMPLE_32_DIAGONAL_SHA256
    {
        return Err(io::Error::other(format!(
            "32x32 downsample evidence drifted: stats={downsample_32_stats:?}, changed_from_high={downsample_32_changed_pixels}, sha256={downsample_32_digest}"
        ))
        .into());
    }
    if !(downsampled.present_id < resize_transition.present_id
        && resize_transition.present_id < high_32.present_id
        && high_32.present_id < downsample_32.present_id)
    {
        return Err(io::Error::other(format!(
            "32x32 downsample presentation IDs did not advance: previous={}, transition={}, high={}, downsample={}",
            downsampled.present_id,
            resize_transition.present_id,
            high_32.present_id,
            downsample_32.present_id
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_reference::ReferenceBackend;

    #[test]
    fn raw_fixture_has_the_expected_native_geometry() {
        let (mut rdram, end) = fixture(TexturePattern::Diagonal);
        let mut backend = ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT)).unwrap();
        assert_eq!(
            backend
                .process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET)
                .unwrap(),
            FrameStatus::Complete
        );

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let mut red = Vec::new();
        let mut blue = Vec::new();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                match view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    TARGET + (y * WIDTH + x) * 2,
                )) {
                    RED => red.push((x, y)),
                    BLUE => blue.push((x, y)),
                    _ => {}
                }
            }
        }
        assert_eq!(red.len(), 78);
        assert_eq!(blue.len(), 91);
        assert!(red.contains(&(12, 4)));
        assert!(blue.contains(&(4, 12)));
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
            0
        );
    }
}
