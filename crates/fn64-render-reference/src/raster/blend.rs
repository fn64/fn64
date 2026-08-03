use crate::gbi::*;
use super::*;

pub(super) fn require_supported_alpha_compare(other_mode: OtherMode, primitive: &str) {
    match other_mode.alpha_compare() {
        AlphaCompare::None | AlphaCompare::Threshold | AlphaCompare::Dither => {}
        AlphaCompare::Reserved => {
            panic!("{primitive} selected reserved G_AC alpha-compare mode 2")
        }
    }
}

pub(super) fn require_safe_fill_cycle_bypass(other_mode: OtherMode, primitive: &str) {
    if let Err(hazards) = other_mode.validate_fill_cycle_bypass() {
        crate::render_unsupported_panic(
            "render.rdp.fill-cycle-hazard-state",
            format!(
                "{primitive} in Fill cycle retains unsafe {hazards} state; the public fill contract requires G_RM_NOOP/G_RM_NOOP2, and retaining Z/framebuffer consumers is outside that safe contract (a depth read can hang the RDP)"
            ),
        );
    }
}

/// Screen-registered three-bit thresholds for the two ordered RGB modes.
/// Each 4x4 tile contains every threshold 0..=7 twice. The standard Bayer
/// tile maximizes spatial separation; the magic-square tile gives every row
/// and column the same threshold sum for use with the VI dither filter.
pub(super) fn ordered_rgb_dither_threshold(mode: RgbDither, x: i32, y: i32) -> u8 {
    const MAGIC_SQUARE: [[u8; 4]; 4] = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
    const BAYER: [[u8; 4]; 4] = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
    let row = y.rem_euclid(4) as usize;
    let column = x.rem_euclid(4) as usize;
    match mode {
        RgbDither::MagicSquare => MAGIC_SQUARE[row][column],
        RgbDither::Bayer => BAYER[row][column],
        RgbDither::Noise | RgbDither::Disabled => {
            unreachable!("ordered dither threshold requested for {mode:?}")
        }
    }
}

/// Apply the memory-interface dither decision while retaining an RGBA8888
/// working surface. Programming Manual Chapter 15.5 places this RGB
/// perturbation before the selected color-image format is written and says it
/// remains active for RGBA32 even though that layout does not discard the low
/// bits. Keeping the destination layout out of this function makes that
/// pre-write ordering structural: I8, RGBA16, and RGBA32 all consume the same
/// dithered working color. For RGBA16, a component reaches the next five-bit
/// bucket exactly when its low three bits exceed the selected threshold; the
/// eventual writer performs the common `>> 3` packing.
pub(super) fn apply_rgb_dither(
    mut rgba: [u8; 4],
    mode: RgbDither,
    x: i32,
    y: i32,
    noise: NoiseSample,
) -> [u8; 4] {
    let threshold = match mode {
        RgbDither::MagicSquare | RgbDither::Bayer => ordered_rgb_dither_threshold(mode, x, y),
        RgbDither::Disabled => return rgba,
        RgbDither::Noise => noise.dither(),
    };
    for component in &mut rgba[..3] {
        if (*component & 7) > threshold {
            *component = (*component & !7).saturating_add(8);
        }
    }
    rgba
}

/// Reduce post-combiner pixel alpha to the blender's five-bit input. Public
/// `gDPSetAlphaDither` defines PATTERN as the selected RGB matrix, with Bayer
/// substituted when RGB dither is disabled and magic square substituted when
/// RGB noise is selected. NOTPATTERN reverses the three-bit threshold.
pub(super) fn apply_alpha_dither(
    alpha: u8,
    alpha_mode: AlphaDither,
    rgb_mode: RgbDither,
    x: i32,
    y: i32,
    noise: NoiseSample,
) -> u8 {
    let threshold = match alpha_mode {
        AlphaDither::Disabled => return alpha,
        AlphaDither::Noise => noise.dither(),
        AlphaDither::Pattern | AlphaDither::InversePattern => {
            let pattern = match rgb_mode {
                RgbDither::MagicSquare | RgbDither::Bayer => rgb_mode,
                RgbDither::Noise => RgbDither::MagicSquare,
                RgbDither::Disabled => RgbDither::Bayer,
            };
            let threshold = ordered_rgb_dither_threshold(pattern, x, y);
            if alpha_mode == AlphaDither::InversePattern {
                7 - threshold
            } else {
                threshold
            }
        }
    };
    let rounded = u16::from(alpha >> 3) + u16::from((alpha & 7) > threshold);
    let five = rounded.min(31) as u8;
    (five << 3) | (five >> 2)
}

pub(super) fn alpha_compare_value(
    mode: AlphaCompare,
    alpha: u8,
    threshold_alpha: u8,
    noise: NoiseSample,
) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        // Programming Manual 15.5.4 describes this as alpha greater than a
        // random value in [0,1). Cross-multiply alpha/255 and noise/256 so
        // transparent always rejects and opaque always passes. The byte is the
        // shared per-fragment source, not an ordered screen-space matrix.
        AlphaCompare::Dither => u32::from(alpha) * 256 > u32::from(noise.byte()) * 255,
        AlphaCompare::Reserved => {
            unreachable!("reserved alpha compare is rejected before rasterization")
        }
    }
}

/// Copy-cycle alpha comparison is format-dependent. Programming Manual
/// section 15.5.4 states that an RGBA16 texel does not enter the eight-bit
/// comparator: its single alpha bit is the write enable. The supported direct
/// 8-bit source retains the ordinary blend-alpha threshold.
pub(super) fn copy_alpha_compare_value(
    mode: AlphaCompare,
    texture: &crate::gbi::Texture,
    alpha: u8,
    threshold_alpha: u8,
    noise: NoiseSample,
) -> bool {
    match mode {
        AlphaCompare::None => true,
        AlphaCompare::Threshold | AlphaCompare::Dither
            if texture.format == ColorImage::RGBA_FORMAT && texture.size == ColorImage::BITS_16 =>
        {
            alpha != 0
        }
        AlphaCompare::Threshold => alpha >= threshold_alpha,
        AlphaCompare::Dither => u32::from(alpha) * 256 > u32::from(noise.byte()) * 255,
        AlphaCompare::Reserved => {
            unreachable!("reserved alpha compare is rejected before copy rasterization")
        }
    }
}

/// Evaluate the RDP blender selectors for one covered fragment. The public
/// GBI defines each cycle as `P*A + M*B` (`GBL_c1`/`GBL_c2`, gbi.h:612-627).
/// In a second cycle, `G_BL_CLR_IN` names the first cycle's blender result;
/// the framebuffer selector always names the pre-fragment destination.
/// RT64 models the same selector ordering and sequential cycle handoff in
/// `shared/rt64_blender.h:68-81,366-504`.
pub(super) fn blend_fragment(
    src: [u8; 4],
    memory: Option<ReadFramebufferMemory>,
    shade_alpha: u8,
    state: BlenderState,
    blend_enabled: bool,
) -> [u8; 4] {
    if state.cycle_count == 0 {
        return src;
    }

    let src_rgb = [src[0] as f32, src[1] as f32, src[2] as f32];
    let mut blender_rgb = src_rgb;
    let mut final_alpha = 1.0;

    for cycle_index in 0..state.cycle_count.min(2) as usize {
        let cycle = state.cycles[cycle_index];
        let is_last = cycle_index + 1 == state.cycle_count as usize;

        // Without FORCE_BL the last blender cycle is bypassed and selects P;
        // in two-cycle mode cycle 1 still runs (the standard fog-then-pass
        // arrangement). RT64's cycle count/bypass has the same structure at
        // shared/rt64_blender.h:45-65,370-383.
        if is_last && !blend_enabled {
            blender_rgb = blend_color(cycle.p, src_rgb, memory, state, blender_rgb, cycle_index);
            final_alpha = if cycle.p == BlendColorInput::Framebuffer {
                0.0
            } else {
                1.0
            };
            continue;
        }

        let a = blend_a(cycle.a, src[3], shade_alpha, state.fog_color[3]);
        let p = blend_color(cycle.p, src_rgb, memory, state, blender_rgb, cycle_index);
        let m = blend_color(cycle.m, src_rgb, memory, state, blender_rgb, cycle_index);

        // RT64 emits framebuffer terms through dual-source alpha blending
        // (`rt64_blender.h:414-424`; `rt64_raster_shader.cpp:332-339`). This
        // software target performs that final composite here instead: the
        // non-framebuffer input becomes the source color and A becomes its
        // source-alpha factor.
        if cycle.p == BlendColorInput::Framebuffer {
            blender_rgb = m;
            final_alpha = 1.0 - a;
        } else if cycle.m == BlendColorInput::Framebuffer {
            blender_rgb = p;
            final_alpha = a;
        } else {
            let b = blend_b(cycle.b, a, memory);
            if a == 0.0 {
                blender_rgb = m;
            } else if b == 0.0 {
                blender_rgb = p;
            } else {
                let divisor = a + b;
                for channel in 0..3 {
                    blender_rgb[channel] =
                        ((p[channel] * a + m[channel] * b) / divisor).clamp(0.0, 255.0);
                }
            }
            final_alpha = 1.0;
        }
    }

    let dst = memory.map(|sample| sample.rgba);
    assert!(
        dst.is_some() || final_alpha == 1.0,
        "IM_RD-disabled blender produced a framebuffer-dependent composite"
    );
    let mut out_rgb = [0u8; 3];
    for channel in 0..3 {
        let memory_channel = dst.map_or(0.0, |rgba| rgba[channel] as f32);
        out_rgb[channel] = (blender_rgb[channel] * final_alpha
            + memory_channel * (1.0 - final_alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    let memory_alpha = dst.map_or(0.0, |rgba| rgba[3] as f32);
    let alpha = (255.0 * final_alpha + memory_alpha * (1.0 - final_alpha))
        .round()
        .clamp(0.0, 255.0) as u8;
    [out_rgb[0], out_rgb[1], out_rgb[2], alpha]
}

pub(super) fn blend_color(
    input: BlendColorInput,
    src_rgb: [f32; 3],
    memory: Option<ReadFramebufferMemory>,
    state: BlenderState,
    blender_rgb: [f32; 3],
    cycle_index: usize,
) -> [f32; 3] {
    match input {
        BlendColorInput::Combined if cycle_index == 0 => src_rgb,
        BlendColorInput::Combined => blender_rgb,
        BlendColorInput::Framebuffer => {
            let rgba = read_framebuffer_memory(memory, "framebuffer color").rgba;
            [rgba[0] as f32, rgba[1] as f32, rgba[2] as f32]
        }
        BlendColorInput::Blend => [
            state.blend_color[0] as f32,
            state.blend_color[1] as f32,
            state.blend_color[2] as f32,
        ],
        BlendColorInput::Fog => [
            state.fog_color[0] as f32,
            state.fog_color[1] as f32,
            state.fog_color[2] as f32,
        ],
    }
}

pub(super) fn blend_a(input: BlendAlphaInput, combined: u8, shade: u8, fog: u8) -> f32 {
    let value = match input {
        BlendAlphaInput::Combined => combined,
        BlendAlphaInput::Fog => fog,
        BlendAlphaInput::Shade => shade,
        BlendAlphaInput::Zero => 0,
    };
    value as f32 / 255.0
}

pub(super) fn blend_b(input: BlendBInput, a: f32, memory: Option<ReadFramebufferMemory>) -> f32 {
    match input {
        BlendBInput::OneMinusA => 1.0 - a,
        BlendBInput::FramebufferAlpha => {
            read_framebuffer_memory(memory, "framebuffer coverage alpha")
                .coverage
                .count() as f32
                / 8.0
        }
        BlendBInput::One => 1.0,
        BlendBInput::Zero => 0.0,
    }
}

pub(super) fn read_framebuffer_memory(
    memory: Option<ReadFramebufferMemory>,
    selector: &str,
) -> ReadFramebufferMemory {
    memory.unwrap_or_else(|| {
        crate::render_unsupported_panic(
            "render.reference.raster.image-read-disabled",
            format!("blender selects {selector} while IM_RD is disabled"),
        )
    })
}

pub(super) fn edge(a: Vertex, b: Vertex, c: Vertex) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}
