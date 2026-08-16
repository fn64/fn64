// FloatToUINT8 / Float4ToRGBA32 / AlphaDitherValue. Characterization-only;
// not wired into any draw path or bind group layout used elsewhere in this
// crate.
//
// Literal WGSL re-expression of `float_to_uint8`/`float4_to_rgba32`/
// `alpha_dither_value` (`fn64-render-wgpu/src/formats_dither.rs`), itself a
// literal port of RT64's `FloatToUINT8`/`Float4ToRGBA32`/`AlphaDitherValue`
// (`Formats.hlsli:41-54,67,122-127`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). `alphaDither` selector:
// 0=PATTERN, 1=NOTPATTERN, 2=NOISE, 3=DISABLE, matching
// `crate::state::AlphaDither`'s wire order. `color_dither_bit` selects
// between the same two ordered tables `DitherPatternValue` itself selects
// between (0=MagicSquare, 1=Bayer), reusing `rgb_dither.wgsl`'s tables
// rather than re-declaring them a third time.

struct FormatsDitherInput {
    mode: u32,
    color_dither_bit: u32,
    x: i32,
    y: i32,
    noise_byte: u32,
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

struct FormatsDitherOutput {
    rgba32: u32,
    alpha_dither: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<FormatsDitherInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<FormatsDitherOutput>;

const DITHER_PATTERN_BAYER: array<u32, 16> = array<u32, 16>(
    0u, 4u, 1u, 5u,
    4u, 0u, 5u, 1u,
    3u, 7u, 2u, 6u,
    7u, 3u, 6u, 2u,
);

const DITHER_PATTERN_MAGIC_SQUARE: array<u32, 16> = array<u32, 16>(
    0u, 6u, 1u, 7u,
    4u, 2u, 5u, 3u,
    3u, 5u, 2u, 4u,
    7u, 1u, 6u, 0u,
);

fn euclid_rem4(value: i32) -> u32 {
    var wrapped = value % 4;
    if (wrapped < 0) {
        wrapped = wrapped + 4;
    }
    return u32(wrapped);
}

fn dither_pattern_index(x: i32, y: i32) -> u32 {
    let wrapped_x = euclid_rem4(x);
    let wrapped_y = euclid_rem4(y);
    return (wrapped_y << 2u) + wrapped_x;
}

fn dither_pattern_value(pattern: u32, x: i32, y: i32, noise_byte: u32) -> u32 {
    let index = dither_pattern_index(x, y);
    if (pattern == 0u) {
        return DITHER_PATTERN_MAGIC_SQUARE[index];
    }
    if (pattern == 1u) {
        return DITHER_PATTERN_BAYER[index];
    }
    return 0u;
}

fn alpha_dither_value(color_dither_bit: u32, alpha_dither: u32, x: i32, y: i32, noise_byte: u32) -> u32 {
    let pattern = color_dither_bit & 1u;
    if (alpha_dither == 0u) {
        return dither_pattern_value(pattern, x, y, noise_byte);
    }
    if (alpha_dither == 1u) {
        return (~dither_pattern_value(pattern, x, y, noise_byte)) & 7u;
    }
    if (alpha_dither == 2u) {
        return noise_byte & 7u;
    }
    return 0u;
}

fn float_to_uint8(i: f32) -> u32 {
    let clamped = clamp(i, 0.0, 1.0);
    return u32(round(clamped * 255.0));
}

fn float4_to_rgba32(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let rq = float_to_uint8(r) << 24u;
    let gq = float_to_uint8(g) << 16u;
    let bq = float_to_uint8(b) << 8u;
    let aq = float_to_uint8(a);
    return (rq | gq | bq | aq);
}

@compute @workgroup_size(64)
fn formats_dither_compute(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    let input = inputs[index];
    var result: FormatsDitherOutput;
    result.rgba32 = float4_to_rgba32(input.r, input.g, input.b, input.a);
    result.alpha_dither = alpha_dither_value(input.color_dither_bit, input.mode, input.x, input.y, input.noise_byte);
    outputs[index] = result;
}
