// GaussianFilterRGB3x3CS seam. Characterization-only; not wired into any
// dispatch path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of the region weight table, tap offsets, and
// per-channel combine ported in `crate::rt64_gaussian_filter` (mirroring
// RT64's `src/shaders/GaussianFilterRGB3x3CS.hlsl:27-81`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). Reproduces exactly:
// - the nine-region weight selection (interior, four corners, four borders)
// - the two literal renormalizing divisors (0.519827 corners, 0.720991 borders)
// - the three fractional tap offsets (`offsets[0..2]`)
// - the four-term left-to-right `dot(samples, weights)` combine, per channel
//
// Compute-dispatch scaffolding is NOT reproduced here: no compute entry
// point attribute or workgroup-size attribute, no texture/sampler bindings,
// no `SampleLevel`/texel-load calls. `region_weights`, `tap_offsets`, and
// `combine_channel` are plain WGSL functions taking pixel position / texture
// size / already-sampled values as arguments, matching the Rust oracle's
// admitted domain (`crate::rt64_gaussian_filter` module doc "Admitted
// domain"/"Nonclaims").

const KERNEL_A: f32 = 0.077847;
const KERNEL_B: f32 = 0.123317;
const KERNEL_C: f32 = 0.195346;

const CORNER_DIVISOR: f32 = 0.519827;
const BORDER_DIVISOR: f32 = 0.720991;

struct RegionWeights {
    w0: f32,
    w1: f32,
    w2: f32,
    w3: f32,
}

fn region_weights(x: u32, y: u32, width: u32, height: u32) -> RegionWeights {
    if (x > 0u && y > 0u && x < width - 1u && y < height - 1u) {
        // Non-border pixels: float4(a+b+b+c, a+b, a+b, a)
        return RegionWeights(
            KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C,
            KERNEL_A + KERNEL_B,
            KERNEL_A + KERNEL_B,
            KERNEL_A,
        );
    } else if (x == 0u && y == 0u) {
        // Top-left corner: float4(c, b, b, a) / 0.519827
        return RegionWeights(
            KERNEL_C / CORNER_DIVISOR,
            KERNEL_B / CORNER_DIVISOR,
            KERNEL_B / CORNER_DIVISOR,
            KERNEL_A / CORNER_DIVISOR,
        );
    } else if (x == width - 1u && y == 0u) {
        // Top-right corner: float4(b+c, 0, a+b, 0) / 0.519827
        return RegionWeights(
            (KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            (KERNEL_A + KERNEL_B) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    } else if (x == 0u && y == height - 1u) {
        // Bottom-left corner: float4(b+c, a+b, 0, 0) / 0.519827
        return RegionWeights(
            (KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            (KERNEL_A + KERNEL_B) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    } else if (x == width - 1u && y == height - 1u) {
        // Bottom-right corner: float4(a+b+b+c, 0, 0, 0) / 0.519827
        return RegionWeights(
            (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
            0.0 / CORNER_DIVISOR,
        );
    } else if (x == 0u) {
        // Left border: float4(b+c, a+b, b, a) / 0.720991
        return RegionWeights(
            (KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            KERNEL_B / BORDER_DIVISOR,
            KERNEL_A / BORDER_DIVISOR,
        );
    } else if (x == width - 1u) {
        // Right border: float4(a+b+b+c, 0, a+b, 0) / 0.720991
        return RegionWeights(
            (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
        );
    } else if (y == 0u) {
        // Top border: float4(b+c, b, a+b, a) / 0.720991
        return RegionWeights(
            (KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            KERNEL_B / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            KERNEL_A / BORDER_DIVISOR,
        );
    } else {
        // Bottom border (final else): float4(a+b+b+c, a+b, 0, 0) / 0.720991
        return RegionWeights(
            (KERNEL_A + KERNEL_B + KERNEL_B + KERNEL_C) / BORDER_DIVISOR,
            (KERNEL_A + KERNEL_B) / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
            0.0 / BORDER_DIVISOR,
        );
    }
}

fn tap_offsets() -> array<vec2<f32>, 3> {
    let d0 = KERNEL_B / (KERNEL_B + KERNEL_C);
    let d1 = KERNEL_A / (KERNEL_A + KERNEL_B);
    return array<vec2<f32>, 3>(
        vec2<f32>(0.5 + (-d0), 0.5 + (-d0)),
        vec2<f32>(0.5 + 1.0, 0.5 + (-d1)),
        vec2<f32>(0.5 + (-d1), 0.5 + 1.0),
    );
}

fn combine_channel(samples: vec4<f32>, weights: RegionWeights) -> f32 {
    return samples.x * weights.w0
        + samples.y * weights.w1
        + samples.z * weights.w2
        + samples.w * weights.w3;
}
