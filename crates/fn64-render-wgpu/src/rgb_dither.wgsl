// RGB dither / Float4ToRGBA16 quantization seam. Characterization-only; not
// wired into any draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `dither_pattern_value`/`float4_to_rgba16`
// (`fn64-render-wgpu/src/rgb_dither.rs`), itself a literal port of RT64's
// `DitherPatternValue`/`Float4ToRGBA16` (`Formats.hlsli:27-39,95-106`, pinned
// commit 5473732a822a4423b5696e7cb18fecc425a59875). Pattern selector: 0=MAGICSQ,
// 1=BAYER, 2=NOISE, 3=DISABLE, matching `crate::state::RgbDither`'s wire order.
// This shader ports only the non-HDR `Float4ToRGBA16` frontier (`usesHDR ==
// false`); the caller supplies an already-derived `coverage_modulo_8`, not a
// raw alpha float -- see `rgb_dither.rs`'s module-level frontier note for why.

struct RgbDitherInput {
    pattern: u32,
    x: i32,
    y: i32,
    noise_byte: u32,
    r: u32,
    g: u32,
    b: u32,
    coverage_modulo_8: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<RgbDitherInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<u32>;

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
    if (pattern == 2u) {
        return noise_byte & 7u;
    }
    return 0u;
}

fn quantize_channel(channel: u32, dither: u32) -> u32 {
    let sum = channel + dither;
    return min(sum, 255u) >> 3u;
}

fn float4_to_rgba16(r: u32, g: u32, b: u32, coverage_modulo_8: u32, dither: u32) -> u32 {
    var a: u32 = 0u;
    if ((coverage_modulo_8 & 0x4u) != 0u) {
        a = 1u;
    }
    let rq = quantize_channel(r, dither);
    let gq = quantize_channel(g, dither);
    let bq = quantize_channel(b, dither);
    return (rq << 11u) | (gq << 6u) | (bq << 1u) | a;
}

@compute @workgroup_size(64)
fn rgb_dither_quantize(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    let input = inputs[index];
    let dither = dither_pattern_value(input.pattern, input.x, input.y, input.noise_byte);
    outputs[index] = float4_to_rgba16(input.r, input.g, input.b, input.coverage_modulo_8, dither);
}
