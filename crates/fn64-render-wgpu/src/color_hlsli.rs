//! `HUEtoRGB`/`RGBtoHCV`/`HSLtoRGB`/`RGBtoHSL`/`ModRGBWithHSL`/
//! `RGBtoLuminance`/`LinearToSrgb`/`SrgbToLinear`: a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shaders/Color.hlsli` (SHA-256
//! `41b0be36513eb829c215da6ca6e94a1b113fa550fcf87c443ff0f141010c3b48`), whole
//! file (72 lines of content):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! // Sourced from https://www.shadertoy.com/view/4dKcWK
//!
//! static const float EPS = 1e-10;
//!
//! float3 HUEtoRGB(in float hue) {
//!     // Hue [0..1] to RGB [0..1]
//!     // See http://www.chilliant.com/rgb2hsv.html
//!     float3 rgb = abs(hue * 6. - float3(3, 2, 4)) * float3(1, -1, -1) + float3(-1, 2, 2);
//!     return clamp(rgb, 0., 1.);
//! }
//!
//! float3 RGBtoHCV(in float3 rgb) {
//!     // RGB [0..1] to Hue-Chroma-Value [0..1]
//!     // Based on work by Sam Hocevar and Emil Persson
//!     float4 p = (rgb.g < rgb.b) ? float4(rgb.bg, -1., 2. / 3.) : float4(rgb.gb, 0., -1. / 3.);
//!     float4 q = (rgb.r < p.x) ? float4(p.xyw, rgb.r) : float4(rgb.r, p.yzx);
//!     float c = q.x - min(q.w, q.y);
//!     float h = abs((q.w - q.y) / (6. * c + EPS) + q.z);
//!     return float3(h, c, q.x);
//! }
//!
//! float3 HSLtoRGB(in float3 hsl) {
//!     // Hue-Saturation-Lightness [0..1] to RGB [0..1]
//!     float3 rgb = HUEtoRGB(hsl.x);
//!     float c = (1. - abs(2. * hsl.z - 1.)) * hsl.y;
//!     return (rgb - 0.5) * c + hsl.z;
//! }
//!
//! float3 RGBtoHSL(in float3 rgb) {
//!     // RGB [0..1] to Hue-Saturation-Lightness [0..1]
//!     float3 hcv = RGBtoHCV(rgb);
//!     float z = hcv.z - hcv.y * 0.5;
//!     float s = hcv.y / (1. - abs(z * 2. - 1.) + EPS);
//!     return float3(hcv.x, s, z);
//! }
//!
//! float3 ModRGBWithHSL(in float3 rgb, in float3 hslMod) {
//!     return saturate(HSLtoRGB(RGBtoHSL(rgb) + hslMod));
//! }
//!
//! #define STRONG_GREEN_LUMINANCE
//!
//! // Taken from https://64.github.io/tonemapping/
//! float RGBtoLuminance(float3 rgb) {
//! #ifdef STRONG_GREEN_LUMINANCE
//!     // ITU BT.601
//!     return dot(rgb, float3(0.299f, 0.587f, 0.114f));
//! #else
//!     // ITU BT.709
//!     return dot(rgb, float3(0.2127f, 0.7152f, 0.0722f));
//! #endif
//! }
//!
//! float3 LinearToSrgb(in float3 lin) {
//!     return pow(lin, 1. / 2.2);
//! }
//!
//! float3 SrgbToLinear(in float3 srgb) {
//!     return pow(srgb.rgb, 2.2);
//! }
//!
//! float4 LinearToSrgb(in float4 lin) {
//!     return float4(LinearToSrgb(lin.rgb), lin.a);
//! }
//!
//! float4 SrgbToLinear(in float4 srgb) {
//!     return float4(SrgbToLinear(srgb.rgb), srgb.a);
//! }
//! ```
//!
//! Clean-room note: the pinned file's own banner cites
//! <https://www.shadertoy.com/view/4dKcWK> (`HUEtoRGB`/`RGBtoHCV`/
//! `HSLtoRGB`/`RGBtoHSL`/`ModRGBWithHSL` provenance) and
//! <https://64.github.io/tonemapping/> (`RGBtoLuminance` provenance) — RT64's
//! own cited sources, carried forward here as doc-comment attribution only,
//! per the `math_hlsli.rs` `getPerpendicularVector` precedent.
//!
//! Rust cannot overload by parameter type the way HLSL does; the `float4`
//! overloads of `LinearToSrgb`/`SrgbToLinear` get the `4`-suffixed names
//! [`linear_to_srgb4`]/[`srgb_to_linear4`] below — a Rust-forced deviation
//! from the source's own overload-by-type naming, not a behavior change.
//!
//! `RGBtoLuminance`'s `#ifdef STRONG_GREEN_LUMINANCE` is unconditionally
//! `#define`d at file scope in the pinned file with no observed `#undef`, so
//! the `#else` (BT.709) branch is dead code as pinned. This port hard-codes
//! only the BT.601 branch as literal, unconditional Rust — no `#[cfg]`, no
//! runtime flag, no dead alternate-branch code path. The BT.709 branch is
//! not ported.
//!
//! `x.clamp(lo, hi)` (used in [`hue_to_rgb`] and [`mod_rgb_with_hsl`]) is
//! used as-is with literal `0.0`/`1.0` bounds, never computed, so Rust's
//! `f32::clamp` panic-on-`min > max`/NaN-bound conditions can never trigger
//! here. Rust's `f32::clamp` on a NaN `self` returns the NaN unchanged (both
//! bound comparisons are IEEE-754-unordered-false), and this port makes the
//! literal choice to let that NaN propagate rather than special-casing it —
//! unlike `formats_dither.rs`'s `float_to_uint8`, which special-cases NaN to
//! `0.0` only because its return type is `u8` (no NaN representation);
//! `Color.hlsli`'s functions return `f32`/`[f32; 3]`, which can represent
//! NaN, so no special-case is added here.
//!
//! Unwired CPU-only literal port: zero external callers of this module in
//! fn64, no GPU wiring, no shader-manifest entry, no production-path caller,
//! no RT64 numeric-parity claim for the cases needing an actual HLSL/WGSL
//! oracle (NaN cross-language comparison, negative-base `pow`, `dot`
//! summation order, or the two multi-division `mod_rgb_with_hsl`
//! compositions) — see `.claude-handoffs/color-hlsli-implementation-card.md`
//! for the full oracle-required accounting.

const EPS: f32 = 1e-10;

/// Literal port of `float3 HUEtoRGB(in float hue)` (`Color.hlsli:9-14`).
pub fn hue_to_rgb(hue: f32) -> [f32; 3] {
    let rgb = [
        (hue * 6.0 - 3.0).abs() - 1.0,
        -(hue * 6.0 - 2.0).abs() + 2.0,
        -(hue * 6.0 - 4.0).abs() + 2.0,
    ];
    [
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
    ]
}

/// Literal port of `float3 RGBtoHCV(in float3 rgb)` (`Color.hlsli:16-24`).
pub fn rgb_to_hcv(rgb: [f32; 3]) -> [f32; 3] {
    let p = if rgb[1] < rgb[2] {
        [rgb[2], rgb[1], -1.0, 2.0 / 3.0]
    } else {
        [rgb[1], rgb[2], 0.0, -1.0 / 3.0]
    };
    let q = if rgb[0] < p[0] {
        [p[0], p[1], p[3], rgb[0]]
    } else {
        [rgb[0], p[1], p[2], p[0]]
    };
    let c = q[0] - q[3].min(q[1]);
    let h = ((q[3] - q[1]) / (6.0 * c + EPS) + q[2]).abs();
    [h, c, q[0]]
}

/// Literal port of `float3 HSLtoRGB(in float3 hsl)` (`Color.hlsli:26-31`).
pub fn hsl_to_rgb(hsl: [f32; 3]) -> [f32; 3] {
    let rgb = hue_to_rgb(hsl[0]);
    let c = (1.0 - (2.0 * hsl[2] - 1.0).abs()) * hsl[1];
    [
        (rgb[0] - 0.5) * c + hsl[2],
        (rgb[1] - 0.5) * c + hsl[2],
        (rgb[2] - 0.5) * c + hsl[2],
    ]
}

/// Literal port of `float3 RGBtoHSL(in float3 rgb)` (`Color.hlsli:33-39`).
pub fn rgb_to_hsl(rgb: [f32; 3]) -> [f32; 3] {
    let hcv = rgb_to_hcv(rgb);
    let z = hcv[2] - hcv[1] * 0.5;
    let s = hcv[1] / (1.0 - (z * 2.0 - 1.0).abs() + EPS);
    [hcv[0], s, z]
}

/// Literal port of `float3 ModRGBWithHSL(in float3 rgb, in float3 hslMod)`
/// (`Color.hlsli:41-43`).
pub fn mod_rgb_with_hsl(rgb: [f32; 3], hsl_mod: [f32; 3]) -> [f32; 3] {
    let hsl = rgb_to_hsl(rgb);
    let summed = [
        hsl[0] + hsl_mod[0],
        hsl[1] + hsl_mod[1],
        hsl[2] + hsl_mod[2],
    ];
    let rgb = hsl_to_rgb(summed);
    [
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
    ]
}

/// Literal port of `float RGBtoLuminance(float3 rgb)` (`Color.hlsli:45-56`),
/// pinned-file behavior only: `STRONG_GREEN_LUMINANCE` is unconditionally
/// `#define`d in the pinned file, so the `#else` (BT.709) branch is dead
/// code as pinned and is NOT ported.
pub fn rgb_to_luminance(rgb: [f32; 3]) -> f32 {
    // `f`-suffixed in source (the file's only explicitly-`f32`-suffixed
    // literal group); no additional semantic beyond that since HLSL's
    // default literal inference here is already `float`.
    rgb[0] * 0.299_f32 + rgb[1] * 0.587_f32 + rgb[2] * 0.114_f32
}

/// Literal port of `float3 LinearToSrgb(in float3 lin)` (`Color.hlsli:58-60`).
pub fn linear_to_srgb(lin: [f32; 3]) -> [f32; 3] {
    let exponent = 1.0_f32 / 2.2_f32;
    [
        lin[0].powf(exponent),
        lin[1].powf(exponent),
        lin[2].powf(exponent),
    ]
}

/// Literal port of `float3 SrgbToLinear(in float3 srgb)` (`Color.hlsli:62-64`).
pub fn srgb_to_linear(srgb: [f32; 3]) -> [f32; 3] {
    [srgb[0].powf(2.2), srgb[1].powf(2.2), srgb[2].powf(2.2)]
}

/// Literal port of `float4 LinearToSrgb(in float4 lin)` (`Color.hlsli:66-68`).
pub fn linear_to_srgb4(lin: [f32; 4]) -> [f32; 4] {
    let rgb = linear_to_srgb([lin[0], lin[1], lin[2]]);
    [rgb[0], rgb[1], rgb[2], lin[3]]
}

/// Literal port of `float4 SrgbToLinear(in float4 srgb)` (`Color.hlsli:70-72`).
pub fn srgb_to_linear4(srgb: [f32; 4]) -> [f32; 4] {
    let rgb = srgb_to_linear([srgb[0], srgb[1], srgb[2]]);
    [rgb[0], rgb[1], rgb[2], srgb[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_to_rgb_red_anchor() {
        assert_eq!(hue_to_rgb(0.0), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn hue_to_rgb_cyan_anchor() {
        assert_eq!(hue_to_rgb(0.5), [0.0, 1.0, 1.0]);
    }

    #[test]
    fn hue_to_rgb_out_of_domain_no_wrap() {
        // hue=1.5, unclamped: abs(9-[3,2,4])*[1,-1,-1]+[-1,2,2]
        // = abs([6,7,5])*[1,-1,-1]+[-1,2,2] = [6,-7,-5]+[-1,2,2] = [5,-5,-3]
        // clamp(0,1): [1.0, 0.0, 0.0]
        let result = hue_to_rgb(1.5);
        assert_eq!(result, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn hue_to_rgb_nan_propagates_through_clamp() {
        // Frozen design decision: `.clamp()` is used as-is with no NaN
        // special-case, so NaN propagates through every arithmetic op and
        // then through `.clamp(0.0, 1.0)` (both bound comparisons are
        // IEEE-754-unordered-false for a NaN `self`).
        let result = hue_to_rgb(f32::NAN);
        assert!(result.iter().all(|c| c.is_nan()));
    }

    #[test]
    fn rgb_to_hcv_black_is_achromatic_zero() {
        // Corrected false/false branch: q.z = p.z = 0, not p.w = -1/3.
        assert_eq!(rgb_to_hcv([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn rgb_to_hcv_red_anchor() {
        assert_eq!(rgb_to_hcv([1.0, 0.0, 0.0]), [0.0, 1.0, 1.0]);
    }

    #[test]
    fn rgb_to_hcv_gray_is_achromatic_zero() {
        // Corrected second instance of the same false/false branch.
        assert_eq!(rgb_to_hcv([0.5, 0.5, 0.5]), [0.0, 0.0, 0.5]);
    }

    #[test]
    fn rgb_to_hcv_nan_component_propagates() {
        // Both ternary conditions become false (NaN-hostile `<`), landing
        // in the same false/false branch as black/gray; q[0]=NaN, so
        // c=q[0]-q[3].min(q[1]) is NaN, and h's denominator is NaN, and h
        // itself is NaN. Same-language IEEE-754 arithmetic, not a `clamp`
        // call (this function has none) -- not oracle-required.
        let result = rgb_to_hcv([f32::NAN, 0.0, 0.0]);
        assert!(result[2].is_nan());
    }

    #[test]
    fn hsl_to_rgb_zero_saturation_is_gray() {
        assert_eq!(hsl_to_rgb([0.0, 0.0, 0.5]), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn hsl_to_rgb_full_saturation_mid_lightness_is_red() {
        assert_eq!(hsl_to_rgb([0.0, 1.0, 0.5]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn hsl_to_rgb_chroma_collapses_at_lightness_extremes() {
        assert_eq!(hsl_to_rgb([0.0, 1.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_eq!(hsl_to_rgb([0.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn rgb_to_hsl_red_saturation_near_one() {
        let result = rgb_to_hsl([1.0, 0.0, 0.0]);
        assert_eq!(result[0], 0.0);
        let expected_s = 1.0 / (1.0 + 1e-10_f32);
        assert!((result[1] - expected_s).abs() < 1e-12);
        assert_eq!(result[2], 0.5);
    }

    #[test]
    fn rgb_to_hsl_gray_is_achromatic() {
        // Corrected (was h=1/3 in the audit, inherited from RGBtoHCV).
        assert_eq!(rgb_to_hsl([0.5, 0.5, 0.5]), [0.0, 0.0, 0.5]);
    }

    #[test]
    fn rgb_to_hsl_black_is_zero() {
        // Corrected; second of the file's two EPS-denominator sites, hit at
        // its exact trigger condition (z exactly 0).
        assert_eq!(rgb_to_hsl([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn mod_rgb_with_hsl_zero_saturation_hue_term_cancels() {
        // From the corrected gray rgb_to_hsl fixture (h,s,z)=(0,0,0.5);
        // +hsl_mod=(0,0,0.7); saturation stays 0 so chroma collapses to 0
        // regardless of the (corrected) hue term.
        let result = mod_rgb_with_hsl([0.5, 0.5, 0.5], [0.0, 0.0, 0.2]);
        assert_eq!(result, [0.7, 0.7, 0.7]);
    }

    #[test]
    fn rgb_to_luminance_white_sums_to_one() {
        // Same-language cross-check against Rust's own independently
        // computed literal sum; does not claim parity with HLSL's `dot`
        // summation order (oracle-required for that separate question).
        let expected = 0.299_f32 + 0.587_f32 + 0.114_f32;
        assert_eq!(rgb_to_luminance([1.0, 1.0, 1.0]), expected);
    }

    #[test]
    fn rgb_to_luminance_red_only() {
        assert_eq!(rgb_to_luminance([1.0, 0.0, 0.0]), 0.299);
    }

    #[test]
    fn rgb_to_luminance_negative_component_no_clamp() {
        let result = rgb_to_luminance([-1.0, 1.0, 1.0]);
        assert!((result - 0.402).abs() < 1e-6);
    }

    #[test]
    fn linear_to_srgb_one_is_identity() {
        assert_eq!(linear_to_srgb([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn linear_to_srgb_zero_is_identity() {
        assert_eq!(linear_to_srgb([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn srgb_to_linear_endpoints_are_identity() {
        assert_eq!(srgb_to_linear([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0]);
        assert_eq!(srgb_to_linear([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn linear_to_srgb_negative_base_is_nan_no_parity_claim() {
        // Rust's f32::powf for a negative base with non-integer exponent is
        // documented to return NaN. HLSL's own spec calls this undefined,
        // not specifically NaN -- this asserts only the deterministic
        // Rust-side behavior, no RT64-parity claim.
        let result = linear_to_srgb([-0.5, 0.5, 0.5]);
        assert!(result[0].is_nan());
    }

    #[test]
    fn linear_to_srgb_round_trip_within_tolerance() {
        // f32::powf is not guaranteed correctly-rounded; same-language
        // tolerance check, not a cross-language HLSL-parity claim.
        let result = linear_to_srgb(srgb_to_linear([0.5, 0.5, 0.5]));
        for c in result {
            assert!((c - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn linear_to_srgb4_alpha_passes_through_verbatim() {
        assert_eq!(linear_to_srgb4([1.0, 1.0, 1.0, 0.3]), [1.0, 1.0, 1.0, 0.3]);
        assert_eq!(linear_to_srgb4([0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn linear_to_srgb4_alpha_out_of_range_no_clamp() {
        let result_high = linear_to_srgb4([1.0, 1.0, 1.0, 1.5]);
        assert_eq!(result_high[3], 1.5);
        let result_low = linear_to_srgb4([1.0, 1.0, 1.0, -0.2]);
        assert_eq!(result_low[3], -0.2);
    }

    #[test]
    fn srgb_to_linear4_alpha_passes_through_verbatim() {
        assert_eq!(srgb_to_linear4([1.0, 1.0, 1.0, 0.3]), [1.0, 1.0, 1.0, 0.3]);
        assert_eq!(srgb_to_linear4([0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn srgb_to_linear4_alpha_out_of_range_no_clamp() {
        let result_high = srgb_to_linear4([1.0, 1.0, 1.0, 1.5]);
        assert_eq!(result_high[3], 1.5);
        let result_low = srgb_to_linear4([1.0, 1.0, 1.0, -0.2]);
        assert_eq!(result_low[3], -0.2);
    }
}
