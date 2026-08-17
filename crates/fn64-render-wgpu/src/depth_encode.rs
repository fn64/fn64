//! `FloatToFixedDepth`, `ExponentFromFixedDepth`, `FloatToDepth16`,
//! `Depth16ToFloat`, `CoplanarDepthTolerance`, `DepthToRGBA8888`, and
//! `RGBA8888ToDepth`: a literal port of the permitted MIT RT64 source pinned
//! at commit `f0728a2520d5aa735886240de3fee75cc805f6d6`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/Depth.hlsli` (80 lines,
//! SHA-256 `7b66c5e78ce9e6d2118d4541bb6d92fd49a65875cbfe947794cf8e1080ed3a11`,
//! reverified against the live pinned checkout for this module):
//!
//! ```text
//! #define DEPTH_EXPONENT_MASK     0xE000
//! #define DEPTH_MANTISSA_MASK     0x1FFC
//! #define DEPTH_EXPONENT_SHIFT    13
//! #define DEPTH_MANTISSA_SHIFT    2
//!
//! // Convert to 15.3 bit fixed-point, which is what the RSP uses.
//! uint FloatToFixedDepth(float i) {
//!     return round(i * (32768.0f * 8.0f - 1));
//! }
//!
//! // Determine the exponent value based on the leading ones in the fixed-point depth value.
//! uint ExponentFromFixedDepth(uint depthFixed) {
//!     uint depthShifted = depthFixed << 14;
//!     int firstZero = firstbithigh(~depthShifted);
//!     return (uint)(clamp(31 - firstZero, 0, 7));
//! }
//!
//! uint FloatToDepth16(float z, float dz) {
//!     uint zFixed = FloatToFixedDepth(z);
//!     uint exponent = ExponentFromFixedDepth(zFixed);
//!
//!     // Determine the mantissa by shifting by the exponent. Cap the shift at 6 here, as an
//!     // exponent of 7 still only shifts by 6.
//!     uint mantissa = zFixed >> (6 - min(6, exponent));
//!
//!     // Determine dz by finding the next largest power of two.
//!     uint dzFixed = FloatToFixedDepth(dz);
//!     dzFixed = clamp(dzFixed, 0x1, 0x8000);
//!     uint dzBit = firstbithigh(dzFixed);
//!
//!     // Encode the two most significant bits in the visible bits.
//!     dzBit = (dzBit >> 2) & 0x3;
//!
//!     // Pack dz, the exponent and mantissa into the floating point format.
//!     return dzBit | (exponent << DEPTH_EXPONENT_SHIFT) | ((mantissa << DEPTH_MANTISSA_SHIFT) & DEPTH_MANTISSA_MASK);
//! }
//!
//! float Depth16ToFloat(uint i) {
//!     // Extract the exponent and mantissa from the depth buffer value.
//!     uint exponent = (i & DEPTH_EXPONENT_MASK) >> DEPTH_EXPONENT_SHIFT;
//!     uint mantissa = (i & DEPTH_MANTISSA_MASK) >> DEPTH_MANTISSA_SHIFT;
//!
//!     // Convert the exponent and mantissa into a fixed-point value.
//!     uint shiftedMantissa = mantissa << (6 - min(6, exponent));
//!     uint mantissaBias = 0x40000U - (0x40000U >> exponent);
//!
//!     return (shiftedMantissa + mantissaBias) / (32768.0f * 8.0f - 1);
//! }
//!
//! float CoplanarDepthTolerance(float i) {
//!     uint depthFixed = FloatToFixedDepth(i);
//!     uint exponent = ExponentFromFixedDepth(depthFixed);
//!     const float MaxTolerance = 0.0005f;
//!     const uint MaxExponent = 3;
//!     return MaxTolerance / pow(2.0f, min(exponent, MaxExponent));
//! }
//!
//! // Used for packing and unpacking back and forth from the color to the depth buffer at full precision.
//! // Only used if a path that does not rely on accurate writeback is detected.
//! // Sourced from https://skytiger.wordpress.com/2010/12/01/packing-depth-into-color/
//!
//! float4 DepthToRGBA8888(float depth) {
//!     const float4 factor = float4(1.0f, 255.0f, 65025.0f, 16581375.0f);
//!     const float mask = 1.0f / 256.0f;
//!     float4 color = depth * factor;
//!     color.gba = frac(color.gba);
//!     color.rgb -= color.gba * mask;
//!     return color;
//! }
//!
//! float RGBA8888ToDepth(float4 color) {
//!     const float4 factor = 1.0f / float4(1.0f, 255.0f, 65025.0f, 16581375.0f);
//!     return dot(color, factor);
//! }
//! ```
//!
//! Elided from the quote above, present in the real pinned file, with no
//! independent semantic content: the file's leading 3-line `//\n// RT64\n//\n`
//! header comment and `#pragma once`. Every other comment above is RT64's
//! own, present verbatim at those lines; the SHA-256 digest cited above
//! covers the real 80-line file, not this abbreviated-header quote.
//!
//! `fn64-render-wgpu` has no crate dependency on a vector-math library, so
//! HLSL's `float4` is translated to `[f32; 4]`, matching this crate's
//! established convention (`crate::color_converter`).
//! `DEPTH_EXPONENT_MASK`/`DEPTH_MANTISSA_MASK`/`DEPTH_EXPONENT_SHIFT`/
//! `DEPTH_MANTISSA_SHIFT` have no external Rust home and are local `const`s
//! in this module.
//!
//! ## Rounding: `round()` is round-half-to-even
//!
//! `Depth.hlsli`'s `round()` is the raw GPU-shader-HLSL intrinsic (not
//! `hlslpp`'s CPU-side SSE `round`), documented by the primary Microsoft
//! HLSL intrinsic-function reference
//! (<https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/dx-graphics-hlsl-round>)
//! as "Rounds the specified value to the nearest integer. Halfway cases are
//! rounded to the nearest even integer", compiling to DXIL `Round_ne`
//! ("round to nearest, ties to even") consistently across the D3D12/Vulkan/
//! Metal backends RT64 targets. This module uses `f32::round_ties_even()`
//! (stable since Rust 1.77), matching the same rounding-mode choice
//! `crate::formats_dither::float_to_uint8` (`formats_dither.rs:87-91`)
//! already made for the same intrinsic on a sibling `src/shaders/*.hlsli`
//! file (`Formats.hlsli`) -- on-point same-intrinsic prior art, cited here
//! as corroboration, not as a substitute for the primary-source citation
//! above.
//!
//! ## Input domain: finite `[0.0, 1.0]`, not unconditionally infallible
//!
//! `FloatToFixedDepth`'s `return round(i * ...)` implicitly converts the
//! `float` result to `uint`. HLSL/DXIL do not document this cast as
//! saturating for NaN, infinite, negative, or otherwise out-of-range
//! inputs -- it is language-undefined/implementation-defined outside a
//! finite, representable range, unlike Rust's own `as u32` cast (total and
//! saturating since the 2018-era float-to-int cast RFC: NaN maps to `0`,
//! negative maps to `0`, overflow maps to the type's `MAX`, never UB, never
//! panics). This module therefore freezes its documented input domain to
//! **finite `z`/`dz`/`depth` in `[0.0, 1.0]`** (RT64's only documented
//! caller contract: normalized RDP depth) and claims HLSL-faithful behavior
//! only inside that domain. Outside it, the Rust port takes whatever value
//! Rust's own `as`-cast semantics produce -- never a panic, but not claimed
//! to match undocumented HLSL behavior either. Every function below states
//! its own domain in its doc comment. `pow(2.0f, min(exponent, MaxExponent))`
//! in `CoplanarDepthTolerance` has no such risk: a fixed positive base and a
//! `min`-saturated small non-negative integer exponent (`[0, 3]`) are always
//! finite regardless of the `i`/`depth` domain question.
//!
//! ## `firstbithigh`: HLSL bit-scan intrinsic, zero sentinel
//!
//! `firstbithigh(uint)` (Microsoft HLSL intrinsic-functions reference,
//! <https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/dx-graphics-hlsl-firstbithigh>):
//! for nonzero `v`, returns the 0-indexed bit position of the
//! most-significant set bit, counted from the LSB (`firstbithigh(0x1) == 0`,
//! `firstbithigh(0x80000000) == 31`, `firstbithigh(0xFFFFFFFF) == 31` since
//! bit 31 is the highest set bit of an all-ones value); for `v == 0`, returns
//! the sentinel `0xFFFFFFFF` (`-1` read as the function's actual signed
//! `int` return type). This is specified language behavior, fixed
//! identically across every backend RT64 targets, not silicon-dependent.
//! [`firstbithigh`] implements both cases with an explicit zero-guard rather
//! than relying on `31u32.wrapping_sub(32)`'s coincidental wraparound to
//! `0xFFFFFFFF` -- matching this program's "port literally, do not rely on
//! incidental integer behavior" discipline.
//!
//! `ExponentFromFixedDepth`'s `int firstZero = firstbithigh(~depthShifted)`
//! reads the signed `int` overload: [`exponent_from_fixed_depth`] computes
//! `first_zero` as `i32` and evaluates `31i32 - first_zero` in signed
//! arithmetic before the final `clamp(0, 7)`, so the (call-site-unreachable,
//! see below) `firstZero == -1` case would still clamp correctly to `31 -
//! (-1) = 32 -> 7`, matching HLSL's signed semantics rather than an
//! incidental unsigned wraparound that happens to produce the same `32`.
//!
//! `ExponentFromFixedDepth`'s call site can never actually reach
//! `firstbithigh`'s zero-sentinel branch: `depth_shifted = depth_fixed <<
//! 14` always zero-fills the low 14 bits of a `u32`, so `depth_shifted` can
//! never equal `0xFFFFFFFF` for any `u32 depth_fixed`, and `!depth_shifted`
//! can therefore never equal `0`. This is a reachability fact about that one
//! call site, not about `firstbithigh`'s own contract -- the general-purpose
//! [`firstbithigh`] still implements and tests the zero-guard, since a future
//! caller may pass `0` directly.
//!
//! ## Kinship risks (formula cousins -- not overlap)
//!
//! 1. [`depth16_to_float`] is byte-for-byte the same formula as
//!    `crate::color_converter::d16_to_float` (`ColorConverter::D16::toF`,
//!    `rt64_color_converter.cpp`): `DEPTH_EXPONENT_MASK`/
//!    `DEPTH_MANTISSA_MASK`/`DEPTH_EXPONENT_SHIFT`/`DEPTH_MANTISSA_SHIFT` are
//!    defined identically in both source files, and both compute
//!    `(shiftedMantissa + mantissaBias) / (32768*8-1)`. This is RT64's own
//!    duplication across a CPU-side `.cpp` copy and a GPU-side `.hlsli`
//!    copy of the same decode, not an invention by either port -- both are
//!    ported literally and independently here, matching
//!    `docs/RENDER-WGPU-PORT-PLAN.md`'s "preserve RT64's structure, don't
//!    silently deduplicate upstream's own duplication" governing principle.
//!    [`depth16_to_float`] does not call `color_converter::d16_to_float` or
//!    vice versa.
//! 2. `crate::depth_mode`'s `decode_delta_z`/`relations`
//!    (`depth_mode.rs:31-34`) operate on already-decoded working-space `u32`
//!    Z and `u16` DeltaZ values; this module's [`float_to_depth16`]/
//!    [`depth16_to_float`] are the encode/decode layer that produces/
//!    consumes the packed 16-bit representation those functions never
//!    touch. Zero shared symbol.
//! 3. `crate::formats_dither::float_to_uint8`'s `round_ties_even` citation is
//!    the same GPU-shader-HLSL `round()` intrinsic as this module's (both
//!    compile through DXIL `Round_ne` against a sibling `src/shaders/*.hlsli`
//!    file), not `hlslpp`'s CPU-side SSE `round` (that citation belongs to
//!    `crate::color_converter`'s module doc, a distinct C++ CPU-side
//!    source). See "Rounding" above.
//!
//! ## Nonclaims
//!
//! This module does not wire into `production.rs`, `raw_dpc/`, `targets/`,
//! any WGSL shader, `state.rs`, `shader_manifest.rs`, or `depth_mode.rs`
//! itself (no import either direction) -- pure host-side functions, callable
//! but uncalled, matching this crate's established landed-ahead-of-wiring
//! pattern. It does not claim GPU execution, pipeline, or any draw-path/
//! silicon parity. It does not claim HLSL-faithful behavior for `z`/`dz`/
//! `depth`/`color` inputs outside the frozen finite `[0.0, 1.0]` domain (see
//! "Input domain" above) -- the Rust port never panics on any input, but is
//! only claimed behaviorally equivalent to RT64 within the documented
//! domain. It does not resolve, confirm, or claim anything about
//! `crate::color_converter::d16_to_float` beyond the kinship noted above.

const DEPTH_EXPONENT_MASK: u32 = 0xE000;
const DEPTH_MANTISSA_MASK: u32 = 0x1FFC;
const DEPTH_EXPONENT_SHIFT: u32 = 13;
const DEPTH_MANTISSA_SHIFT: u32 = 2;

/// HLSL `firstbithigh(uint)`: 0-indexed bit position of the
/// most-significant set bit, counted from the LSB; sentinel `0xFFFFFFFF`
/// (HLSL's `-1i32`) for `v == 0`. See the module doc comment's
/// "`firstbithigh`" section for the primary-source citation and rationale
/// for the explicit zero-guard.
fn firstbithigh(v: u32) -> i32 {
    if v == 0 {
        -1
    } else {
        (31 - v.leading_zeros()) as i32
    }
}

/// Literal port of `FloatToFixedDepth(float i)` (`Depth.hlsli:12-14`):
/// convert to 15.3-bit fixed-point. `i` must be finite and in `[0.0, 1.0]`
/// (normalized RDP depth) -- see the module doc comment's "Input domain"
/// section. Outside that domain, matches Rust's own `as u32` cast
/// semantics, not HLSL's (language-undefined there); never panics.
pub fn float_to_fixed_depth(i: f32) -> u32 {
    (i * (32768.0 * 8.0 - 1.0)).round_ties_even() as u32
}

/// Literal port of `ExponentFromFixedDepth(uint depthFixed)`
/// (`Depth.hlsli:17-21`): the exponent based on the leading ones in the
/// fixed-point depth value. The signed `int firstZero` and `31 - firstZero`
/// subtraction are computed in `i32` before the final `clamp`, matching
/// HLSL's signed-`int` `firstbithigh` overload -- see the module doc
/// comment's `firstbithigh` section for why this call site can never
/// actually reach the zero-sentinel branch, and why the signed-arithmetic
/// path is still ported explicitly rather than relying on incidental
/// unsigned wraparound.
pub fn exponent_from_fixed_depth(depth_fixed: u32) -> u32 {
    let depth_shifted = depth_fixed << 14;
    let first_zero = firstbithigh(!depth_shifted);
    (31i32 - first_zero).clamp(0, 7) as u32
}

/// Literal port of `FloatToDepth16(float z, float dz)` (`Depth.hlsli:24-41`):
/// pack a normalized depth and its screen-space delta into RT64's 16-bit
/// piecewise-float depth format. `z` and `dz` must each be finite and in
/// `[0.0, 1.0]` -- see the module doc comment's "Input domain" section
/// ([`float_to_fixed_depth`] is called on both).
pub fn float_to_depth16(z: f32, dz: f32) -> u32 {
    let z_fixed = float_to_fixed_depth(z);
    let exponent = exponent_from_fixed_depth(z_fixed);

    // Determine the mantissa by shifting by the exponent. Cap the shift at 6 here, as an
    // exponent of 7 still only shifts by 6.
    let mantissa = z_fixed >> (6 - 6u32.min(exponent));

    // Determine dz by finding the next largest power of two.
    let dz_fixed = float_to_fixed_depth(dz).clamp(0x1, 0x8000);
    let dz_bit = firstbithigh(dz_fixed) as u32;

    // Encode the two most significant bits in the visible bits.
    let dz_bit = (dz_bit >> 2) & 0x3;

    // Pack dz, the exponent and mantissa into the floating point format.
    dz_bit
        | (exponent << DEPTH_EXPONENT_SHIFT)
        | ((mantissa << DEPTH_MANTISSA_SHIFT) & DEPTH_MANTISSA_MASK)
}

/// Literal port of `Depth16ToFloat(uint i)` (`Depth.hlsli:44-52`) --
/// byte-for-byte the same formula as `crate::color_converter::d16_to_float`
/// (`ColorConverter::D16::toF`); see the module doc comment's Kinship
/// section for why both are ported independently rather than sharing an
/// implementation.
pub fn depth16_to_float(i: u32) -> f32 {
    // Extract the exponent and mantissa from the depth buffer value.
    let exponent = (i & DEPTH_EXPONENT_MASK) >> DEPTH_EXPONENT_SHIFT;
    let mantissa = (i & DEPTH_MANTISSA_MASK) >> DEPTH_MANTISSA_SHIFT;

    // Convert the exponent and mantissa into a fixed-point value.
    let shifted_mantissa = mantissa << (6 - 6u32.min(exponent));
    let mantissa_bias = 0x40000u32 - (0x40000u32 >> exponent);

    (shifted_mantissa + mantissa_bias) as f32 / (32768.0 * 8.0 - 1.0)
}

/// Literal port of `CoplanarDepthTolerance(float i)` (`Depth.hlsli:55-60`).
/// `i` must be finite and in `[0.0, 1.0]` -- see the module doc comment's
/// "Input domain" section ([`float_to_fixed_depth`] is called on `i`). The
/// `pow`/`min` chain itself has no NaN/domain risk regardless: a fixed
/// positive base and a `min`-saturated small non-negative integer exponent
/// (`[0, 3]`) are always finite.
pub fn coplanar_depth_tolerance(i: f32) -> f32 {
    let depth_fixed = float_to_fixed_depth(i);
    let exponent = exponent_from_fixed_depth(depth_fixed);
    const MAX_TOLERANCE: f32 = 0.0005;
    const MAX_EXPONENT: u32 = 3;
    MAX_TOLERANCE / 2.0f32.powf(exponent.min(MAX_EXPONENT) as f32)
}

/// Literal port of `DepthToRGBA8888(float depth)` (`Depth.hlsli:66-73`),
/// sourced by RT64 itself from
/// <https://skytiger.wordpress.com/2010/12/01/packing-depth-into-color/>.
/// `depth` must be finite and in `[0.0, 1.0]` -- see the module doc
/// comment's "Input domain" section. The two HLSL statements
/// (`color.gba = frac(color.gba)` then `color.rgb -= color.gba * mask`) run
/// sequentially: the second reads the *already-`frac`'d* `.gba`, not the
/// pre-`frac` value -- ported here as two sequential steps for the same
/// reason.
pub fn depth_to_rgba8888(depth: f32) -> [f32; 4] {
    const FACTOR: [f32; 4] = [1.0, 255.0, 65025.0, 16581375.0];
    const MASK: f32 = 1.0 / 256.0;
    let mut color = [
        depth * FACTOR[0],
        depth * FACTOR[1],
        depth * FACTOR[2],
        depth * FACTOR[3],
    ];
    color[1] = color[1].fract();
    color[2] = color[2].fract();
    color[3] = color[3].fract();
    color[0] -= color[1] * MASK;
    color[1] -= color[2] * MASK;
    color[2] -= color[3] * MASK;
    color
}

/// Literal port of `RGBA8888ToDepth(float4 color)` (`Depth.hlsli:75-78`).
/// Each `color` component should be finite, nominally `[0.0, 1.0]` -- the
/// inverse-packed factors keep this finite for any finite input regardless,
/// but fidelity to [`depth_to_rgba8888`]'s inverse is only claimed for
/// `color` values it could have produced.
pub fn rgba8888_to_depth(color: [f32; 4]) -> f32 {
    const INV_FACTOR: [f32; 4] = [1.0, 1.0 / 255.0, 1.0 / 65025.0, 1.0 / 16581375.0];
    color[0] * INV_FACTOR[0]
        + color[1] * INV_FACTOR[1]
        + color[2] * INV_FACTOR[2]
        + color[3] * INV_FACTOR[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- firstbithigh: isolated sentinel pair (decoupled from any call site) ---

    #[test]
    fn firstbithigh_zero_is_sentinel() {
        assert_eq!(firstbithigh(0u32), -1i32);
        assert_eq!(firstbithigh(0u32) as u32, 0xFFFFFFFFu32);
    }

    #[test]
    fn firstbithigh_all_ones_is_31_not_a_second_sentinel() {
        // 0xFFFFFFFF is a nonzero value: bit 31 is its MSB, ordinary case.
        assert_eq!(firstbithigh(0xFFFFFFFFu32), 31i32);
    }

    #[test]
    fn firstbithigh_single_bit_positions() {
        assert_eq!(firstbithigh(0x1), 0);
        assert_eq!(firstbithigh(0x80000000), 31);
        assert_eq!(firstbithigh(0x3FFF), 13);
    }

    #[test]
    fn mutation_distinguishes_zero_sentinel_exponent_from_ordinary_msb_exponent() {
        // The two ends of firstbithigh's domain must drive
        // exponent_from_fixed_depth to different results downstream: the
        // zero-sentinel value (-1) would clamp `31 - (-1) = 32` to `7`,
        // while an ordinary MSB result like `31` clamps `31 - 31 = 0` to
        // `0`. Confirms the sentinel and the ordinary case are not
        // silently collapsed to the same downstream exponent by coincidence.
        assert_ne!(
            (31i32 - firstbithigh(0)).clamp(0, 7),
            (31i32 - firstbithigh(0xFFFFFFFF)).clamp(0, 7)
        );
    }

    // --- float_to_fixed_depth: domain [0.0, 1.0] ---

    #[test]
    fn float_to_fixed_depth_zero_is_zero() {
        assert_eq!(float_to_fixed_depth(0.0), 0);
    }

    #[test]
    fn float_to_fixed_depth_one_is_262143() {
        assert_eq!(float_to_fixed_depth(1.0), 262143);
    }

    #[test]
    fn float_to_fixed_depth_half_ulp_tie_regresses_to_even() {
        // z * 262143.0 == 0.5 exactly; round_ties_even(0.5) == 0 (even), not 1.
        let z = 0.5 / 262143.0;
        assert_eq!(float_to_fixed_depth(z), 0);
    }

    #[test]
    fn mutation_distinguishes_round_ties_even_from_round_half_away_from_zero() {
        // Confirms the tie case above is actually exercising round-to-even
        // behavior and not merely landing below 0.5 due to f32 rounding:
        // z chosen so z * 262143.0 == 1.5 exactly (odd/even midpoint between
        // 1 and 2) -- round_ties_even(1.5) == 2 (even), not 1.
        let z = 1.5 / 262143.0;
        assert_eq!(float_to_fixed_depth(z), 2);
    }

    // --- exponent_from_fixed_depth: the load-bearing sentinel-adjacent function ---

    #[test]
    fn exponent_from_fixed_depth_zero_input() {
        // depth_shifted = 0, !depth_shifted = 0xFFFFFFFF (bit 31 set, ordinary
        // nonzero MSB case -- NOT the zero-sentinel branch),
        // firstbithigh(0xFFFFFFFF) = 31, clamp(31-31,0,7) = 0.
        assert_eq!(exponent_from_fixed_depth(0), 0);
    }

    #[test]
    fn exponent_from_fixed_depth_all_low_18_bits_set() {
        // depth_fixed = 0x3FFFF (from float_to_fixed_depth(1.0)):
        // depth_shifted = 0x3FFFF << 14 = 0xFFFFC000,
        // !depth_shifted = 0x00003FFF, firstbithigh(0x3FFF) = 13,
        // clamp(31-13,0,7) = clamp(18,0,7) = 7.
        assert_eq!(exponent_from_fixed_depth(0x3FFFF), 7);
    }

    #[test]
    fn exponent_from_fixed_depth_never_reaches_firstbithigh_zero_sentinel() {
        // depth_shifted = depth_fixed << 14 always zero-fills the low 14
        // bits, so it can never equal 0xFFFFFFFF for any u32 depth_fixed,
        // and !depth_shifted can never equal 0 -- exhaustive-by-construction
        // argument, spot-checked here across representative inputs.
        for depth_fixed in [0u32, 1, 0x3FFFF, 0xFFFF, 0xFFFFFFFF] {
            let depth_shifted = depth_fixed << 14;
            assert_ne!(depth_shifted, 0xFFFFFFFFu32, "depth_fixed={depth_fixed:#x}");
        }
    }

    #[test]
    fn exponent_from_fixed_depth_result_always_clamped_to_0_7() {
        for depth_fixed in [0u32, 1, 0x100, 0x3FFFF, 0xFFFF_FFFF] {
            let e = exponent_from_fixed_depth(depth_fixed);
            assert!((0..=7).contains(&e), "depth_fixed={depth_fixed:#x} e={e}");
        }
    }

    // --- float_to_depth16 / depth16_to_float ---

    #[test]
    fn float_to_depth16_zero_z_zero_dz_floor() {
        // z=0.0 -> z_fixed=0, exponent=0 (see exponent_from_fixed_depth_zero_input).
        // dz=0.0 -> dz_fixed=0, clamped to 0x1 -> firstbithigh(1)=0 -> dz_bit=(0>>2)&3=0.
        // mantissa = 0 >> (6 - min(6,0)) = 0.
        // result = 0 | (0 << 13) | ((0 << 2) & 0x1FFC) = 0.
        assert_eq!(float_to_depth16(0.0, 0.0), 0);
    }

    #[test]
    fn float_to_depth16_dz_clamped_to_max_0x8000() {
        // dz=1.0 -> dz_fixed=262143, clamped to 0x8000 (32768) ->
        // firstbithigh(0x8000)=15 -> dz_bit=(15>>2)&3=3.
        let packed = float_to_depth16(0.0, 1.0);
        assert_eq!(packed & 0x3, 3);
    }

    // 16-row exponent x endpoint-mantissa matrix, independently re-derived
    // by hand from the accepted dispatch card's formula (same constants as
    // crate::color_converter::d16_to_float's identical table, independently
    // re-verified here rather than copy-pasted uncritically).
    #[test]
    fn depth16_to_float_exhaustive_exponent_by_endpoint_mantissa_matrix() {
        let cases: [(u32, u32, f64); 16] = [
            (0, 0x000, 0.0),
            (0, 0x7FF, 0.49975776579958264),
            (1, 0x000, 0.5000019073559088),
            (1, 0x7FF, 0.7498807902557001),
            (2, 0x000, 0.7500028610338632),
            (2, 0x7FF, 0.8749423024837588),
            (3, 0x000, 0.8750033378728403),
            (3, 0x7FF, 0.9374730585977882),
            (4, 0x000, 0.9375035762923289),
            (4, 0x7FF, 0.9687384366548029),
            (5, 0x000, 0.9687536955020732),
            (5, 0x7FF, 0.9843711256833102),
            (6, 0x000, 0.9843787551069454),
            (6, 0x7FF, 0.9921874701975639),
            (7, 0x000, 0.9921912849093815),
            (7, 0x7FF, 1.0),
        ];
        for (exponent, mantissa, expected) in cases {
            let src = (exponent << DEPTH_EXPONENT_SHIFT) | (mantissa << DEPTH_MANTISSA_SHIFT);
            let actual = depth16_to_float(src) as f64;
            assert!(
                (actual - expected).abs() < 1e-6,
                "exponent={exponent} mantissa={mantissa:#x} src={src:#06x} expected={expected} actual={actual}"
            );
        }
    }

    #[test]
    fn depth16_to_float_zero_is_zero_and_max_is_one() {
        assert_eq!(depth16_to_float(0x0000), 0.0);
        assert_eq!(depth16_to_float(0xFFFF), 1.0);
    }

    #[test]
    fn depth16_to_float_matches_color_converter_d16_to_float() {
        // Cross-check (optional per the accepted card, not a substitute for
        // the independent port above): both ports compute the same formula
        // from the same constants over the full u16 domain.
        for src in [0u16, 1, 0x1FFC, 0xE000, 0xFFFF, 0x1234, 0x8ACE] {
            let expected = crate::color_converter::d16_to_float(src);
            let actual = depth16_to_float(src as u32);
            assert_eq!(actual, expected, "src={src:#06x}");
        }
    }

    #[test]
    fn mutation_distinguishes_exponent_mask_from_mantissa_mask() {
        let exponent_only = depth16_to_float(0xE000);
        let mantissa_only = depth16_to_float(0x1FFC);
        assert_ne!(exponent_only, mantissa_only);
        assert!(exponent_only > mantissa_only);
    }

    // --- coplanar_depth_tolerance ---

    #[test]
    fn coplanar_depth_tolerance_zero_depth_uses_exponent_zero() {
        // depth=0.0 -> depth_fixed=0 -> exponent=0 (see
        // exponent_from_fixed_depth_zero_input) -> 0.0005 / 2^0 = 0.0005.
        assert_eq!(coplanar_depth_tolerance(0.0), 0.0005);
    }

    #[test]
    fn coplanar_depth_tolerance_one_depth_uses_exponent_seven_clamped_to_three() {
        // depth=1.0 -> depth_fixed=0x3FFFF -> exponent=7 (see
        // exponent_from_fixed_depth_all_low_18_bits_set), min(7,3)=3 ->
        // 0.0005 / 2^3 = 0.0000625.
        let v = coplanar_depth_tolerance(1.0);
        assert!((v - 0.0000625).abs() < 1e-9, "v={v}");
    }

    #[test]
    fn mutation_distinguishes_max_exponent_clamp_from_unclamped() {
        // If MaxExponent's min() were dropped, depth=1.0's exponent=7 would
        // give 0.0005 / 128 = 0.0000039... instead of the clamped
        // 0.0000625 -- an order of magnitude smaller, easily distinguished.
        let v = coplanar_depth_tolerance(1.0);
        assert!(
            v > 0.00005,
            "v={v} should reflect MaxExponent=3 clamp, not exponent=7"
        );
    }

    // --- depth_to_rgba8888 / rgba8888_to_depth ---

    #[test]
    fn depth_to_rgba8888_zero() {
        assert_eq!(depth_to_rgba8888(0.0), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn depth_to_rgba8888_one() {
        // color = [1,255,65025,16581375]; frac(gba) all land on exact
        // integers (255.0/65025.0/16581375.0 are all f32-exact) -> gba
        // becomes [0,0,0]; rgb -= gba*mask leaves rgb unchanged.
        assert_eq!(depth_to_rgba8888(1.0), [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn rgba8888_to_depth_round_trips_depth_to_rgba8888() {
        for depth in [0.0f32, 1.0, 0.25, 0.5, 0.75] {
            let color = depth_to_rgba8888(depth);
            let back = rgba8888_to_depth(color);
            assert!((back - depth).abs() < 2e-5, "depth={depth} back={back}");
        }
    }

    #[test]
    fn rgba8888_to_depth_full_byte_is_slightly_more_than_one() {
        // color=[1,1,1,1] is not depth_to_rgba8888's output domain, but the
        // formula itself is well-defined for any finite input: dot([1,1,1,1],
        // inv_factor) = 1 + 1/255 + 1/65025 + 1/16581375.
        let v = rgba8888_to_depth([1.0, 1.0, 1.0, 1.0]);
        assert!(v > 1.0, "v={v}");
    }

    #[test]
    fn mutation_distinguishes_factor_ordering() {
        // Each factor component must land in its own distinct channel --
        // catches a transposed factor array.
        let r_only = rgba8888_to_depth([1.0, 0.0, 0.0, 0.0]);
        let g_only = rgba8888_to_depth([0.0, 1.0, 0.0, 0.0]);
        let b_only = rgba8888_to_depth([0.0, 0.0, 1.0, 0.0]);
        let a_only = rgba8888_to_depth([0.0, 0.0, 0.0, 1.0]);
        assert_eq!(r_only, 1.0);
        assert!(g_only < r_only && g_only > b_only);
        assert!(b_only > a_only);
        assert!(a_only > 0.0);
    }
}
