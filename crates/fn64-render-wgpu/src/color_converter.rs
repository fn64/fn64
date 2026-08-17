//! `ColorConverter::RGBA16`, `ColorConverter::RGBA32`, and `ColorConverter::D16`:
//! a literal port of the permitted MIT RT64 source pinned at commit
//! `f0728a2520d5aa735886240de3fee75cc805f6d6` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/hle/rt64_color_converter.h` + `src/hle/rt64_color_converter.cpp`
//! (SHA-256 of the whole files,
//! `0e774419ca842b7d085a9fdbbcdb9cd2394d061c18b4969d11afc8eec1c05bef` /
//! `b40f82b3534380171489183a7240ea5a82c3d464f346e45538ad13e25f44b939`; this
//! module's inventory-authoritative port input is actually the *port*
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/rt64-port-authority.json`'s `port_source`), cited above by the
//! oracle commit instead because that is the commit this module's prose was
//! originally reviewed against; both commits' versions of these two files
//! are byte-identical -- the port commit is 9 commits ahead of the oracle
//! commit overall, but carries no diff to `rt64_color_converter.{h,cpp}`, so
//! the two SHA-256 digests above are simultaneously each file's oracle *and*
//! port digest per `docs/rt64-port-inventory.json`'s `sources.oracle.sha256`
//! / `sources.port.sha256`, confirmed independently here by `shasum -a 256`
//! against the pinned port-commit checkout)
//! (code, comments elided for brevity -- see the pinned checkout for the full
//! files with their header and section comments; the two `D16::toF`
//! explanatory comments are carried forward below verbatim):
//!
//! ```text
//! struct ColorConverter {
//!     struct RGBA16 {
//!         static uint16_t toRGBA(hlslpp::float4 src);
//!         static hlslpp::float4 toRGBAF(uint16_t src);
//!     };
//!
//!     struct RGBA32 {
//!         static uint32_t toRGBA(hlslpp::float4 src);
//!         static hlslpp::float4 toRGBAF(uint32_t src);
//!     };
//!
//!     struct D16 {
//!         static float toF(uint16_t src);
//!     };
//! };
//!
//! #define DEPTH_EXPONENT_MASK     0xE000
//! #define DEPTH_MANTISSA_MASK     0x1FFC
//! #define DEPTH_EXPONENT_SHIFT    13
//! #define DEPTH_MANTISSA_SHIFT    2
//!
//! uint16_t ColorConverter::RGBA16::toRGBA(hlslpp::float4 src) {
//!     uint16_t r = uint16_t(hlslpp::round(hlslpp::clamp(src.r * 255.0f, 0.0f, 255.0f)));
//!     uint16_t g = uint16_t(hlslpp::round(hlslpp::clamp(src.g * 255.0f, 0.0f, 255.0f)));
//!     uint16_t b = uint16_t(hlslpp::round(hlslpp::clamp(src.b * 255.0f, 0.0f, 255.0f)));
//!     uint16_t a = uint16_t(hlslpp::round(hlslpp::clamp(src.a * 255.0f, 0.0f, 255.0f)));
//!     r = std::min(r, uint16_t(255)) >> 3;
//!     g = std::min(g, uint16_t(255)) >> 3;
//!     b = std::min(b, uint16_t(255)) >> 3;
//!     a = (a > 0) ? 1 : 0;
//!     return (r << 11) | (g << 6) | (b << 1) | a;
//! }
//!
//! hlslpp::float4 ColorConverter::RGBA16::toRGBAF(uint16_t src) {
//!     uint8_t r = (src >> 11) & 0x1F;
//!     uint8_t g = (src >> 6)  & 0x1F;
//!     uint8_t b = (src >> 1)  & 0x1F;
//!     return {
//!         ((r << 3) | (r >> 2)) / 255.0f,
//!         ((g << 3) | (g >> 2)) / 255.0f,
//!         ((b << 3) | (b >> 2)) / 255.0f,
//!         (src & 1) ? 1.0f : 0.0f
//!     };
//! }
//!
//! uint32_t ColorConverter::RGBA32::toRGBA(hlslpp::float4 src) {
//!     hlslpp::uint4 srcUint = hlslpp::uint4(hlslpp::round(hlslpp::clamp(src * 255.0f, 0.0f, 255.0f)));
//!     return (srcUint[0] << 24) | (srcUint[1] << 16) | (srcUint[2] << 8) | (srcUint[3] << 0);
//! }
//!
//! hlslpp::float4 ColorConverter::RGBA32::toRGBAF(uint32_t src) {
//!     return {
//!         ((src >> 24) & 0xFF) / 255.0f,
//!         ((src >> 16) & 0xFF) / 255.0f,
//!         ((src >> 8)  & 0xFF) / 255.0f,
//!         ((src >> 0)  & 0xFF) / 255.0f
//!     };
//! }
//!
//! float ColorConverter::D16::toF(uint16_t src) {
//!     // Extract the exponent and mantissa from the depth buffer value.
//!     uint32_t exponent = (src & DEPTH_EXPONENT_MASK) >> DEPTH_EXPONENT_SHIFT;
//!     uint32_t mantissa = (src & DEPTH_MANTISSA_MASK) >> DEPTH_MANTISSA_SHIFT;
//!     // Convert the exponent and mantissa into a fixed-point value.
//!     uint32_t shiftedMantissa = mantissa << (6 - std::min(6U, exponent));
//!     uint32_t mantissaBias = 0x40000U - (0x40000U >> exponent);
//!     return (shiftedMantissa + mantissaBias) / (32768.0f * 8.0f - 1);
//! }
//! ```
//!
//! (`RGBA16::toRGBA` lines 19-29, `RGBA16::toRGBAF` lines 31-41,
//! `RGBA32::toRGBA` lines 45-48, `RGBA32::toRGBAF` lines 50-57,
//! `D16::toF` lines 61-70 of the `.cpp`; struct declarations lines 10-24 of
//! the `.h`. Elided from both quotes above, present in the real pinned
//! files, with no independent semantic content: each file's 3-line `//\n//
//! RT64\n//\n` header comment, a blank line after each of the `.cpp`'s three
//! `#include`s, and three section-divider comments inside the `namespace
//! RT64 { ... }` body -- `// ColorConverter::RGBA16`, `//
//! ColorConverter::RGBA32`, `// ColorConverter::D16`. The SHA-256 digests
//! cited below cover the real files, not this abbreviated quote.)
//!
//! `fn64-render-wgpu` has no crate dependency on `hlslpp`
//! (`src/contrib/hlslpp/`, vendored MIT header-only, admitted `allowed` in
//! `docs/RT64-PORT-AUTHORITY.md`), so `hlslpp::float4`/`hlslpp::uint4` are
//! translated directly to plain scalar `f32` arguments / `[f32; 4]` return
//! values, matching this crate's existing style (no vector-math dependency
//! used elsewhere here). `src/common/rt64_math.h`/`.cpp` is not ported by
//! this module: it is only transitively pulled in for the `hlslpp` typedef
//! chain (`rt64_common.h:12` -> `rt64_hlslpp.h`), and none of
//! `rt64_math.cpp`'s actual functions (matrix decompose, pseudoRandom, etc.)
//! are called here. `rt64_common.h` itself is not ported either.
//! `DEPTH_EXPONENT_MASK`/`DEPTH_MANTISSA_MASK`/`DEPTH_EXPONENT_SHIFT`/
//! `DEPTH_MANTISSA_SHIFT` have no external Rust home and are local `const`s
//! in this module.
//!
//! ## Rounding: `hlslpp::round` is round-half-to-even
//!
//! `hlslpp::round` (`src/contrib/hlslpp/include/hlsl++/platforms/sse.h:164`,
//! `_mm_round_ps(..., _MM_FROUND_TO_NEAREST_INT)`) is round-half-to-even, not
//! `f32::round()`'s round-half-away-from-zero. This module uses
//! `f32::round_ties_even()` (stable since Rust 1.77), matching the same
//! rounding-mode requirement `crate::formats_dither::float_to_uint8`
//! (`formats_dither.rs:82-96`) already documents and decided for a sibling
//! float-to-fixed conversion; cited here, not re-derived.
//!
//! ## Clamp NaN policy: a disclosed departure, not inherited hlslpp behavior
//!
//! `hlslpp::clamp`'s own NaN behavior is platform-dependent within the one
//! pinned commit: the SSE backend (`_mm_min_ps`/`_mm_max_ps`,
//! Intel-SDM-documented second-operand-on-NaN rule) saturates a NaN input to
//! the high bound (`255.0f` here); the NEON backend (`vminq_f32`/
//! `vmaxq_f32`, default NaN propagation) returns NaN; the portable scalar
//! fallback (`platforms/scalar.h:247`) passes a NaN input through unchanged.
//! There is no single hlslpp NaN ground truth to port literally. This module
//! adopts `formats_dither.rs:88-91`'s existing "clamp NaN to 0.0" policy as
//! a **deliberate, disclosed departure** for consistency with this crate's
//! other float-to-fixed conversions, not as a transcription of hlslpp's own
//! (multi-valued, and in the SSE case *not* zero) behavior. RT64's combiner
//! pipeline is not expected to ever feed a NaN channel into
//! `ColorConverter` (same assumption `formats_dither.rs` makes), so the
//! departure is very likely dead-code-equivalent in practice -- but it is
//! stated here honestly rather than implied as inherited prior art.
//!
//! ## Kinship risks (formula cousins -- not overlap)
//!
//! 1. `crate::tmem::texel`'s `decode_rgba16`/`decode_rgba32` use the same
//!    `(x<<3)|(x>>2)` 5-to-8-bit expansion and the same big-endian RGBA32
//!    unpack as this module's [`rgba16_packed_to_float`]/
//!    [`rgba32_packed_to_float`] halves, but on the GPU-TMEM-sample decode
//!    path only; this module's encode halves ([`rgba16_to_packed`],
//!    [`rgba32_to_packed`]) are genuinely new, not a duplicate.
//! 2. `crate::formats_dither::float_to_uint8` shares the same
//!    round-half-to-even rounding idiom; cited above as prior art, not
//!    re-derived.
//! 3. `crate::rgb_dither`'s `quantize_post_float_rgba16_non_hdr`/
//!    `quantize_channel` compute a **different** RGBA16 encode formula: they
//!    start from an already-8-bit channel, add a `0..=7` dither threshold,
//!    and take no `round` or float input -- the GPU-shader dithered variant
//!    (`Formats.hlsli::Float4ToRGBA16`), not this module's plain
//!    [`rgba16_to_packed`]. Same output domain, disjoint formula.
//!
//! ## Nonclaims
//!
//! This module does not wire into `production.rs`, `raw_dpc/`, `targets/`,
//! any WGSL shader, `state.rs`, or `shader_manifest.rs` -- pure host-side
//! functions, callable but uncalled, matching this crate's established
//! pattern of landing formula modules ahead of their call-site wiring. It
//! does not port `rt64_math.cpp`/`.h` beyond the two `hlslpp` primitives
//! already covered by existing prior art, and does not port `rt64_common.h`
//! or `hlslpp` itself. It does not resolve, confirm, or claim anything about
//! `rt64_rdp.cpp`'s Z encode/decode functions (not read in this pass)
//! despite the plausible 18-bit-layout kinship with [`d16_to_float`] noted
//! by `crate::depth_mode`'s own module doc comment (`depth_mode.rs:32-34`):
//! that is a follow-up cross-check, not resolved here. It does not claim
//! parity and does not claim any real framebuffer/texture readback call
//! site yet -- RT64's actual callers (`rt64_framebuffer.cpp`,
//! `rt64_framebuffer_manager.cpp`) are separate, still-not-started M4 cards
//! this module unblocks but does not include.

const DEPTH_EXPONENT_MASK: u16 = 0xE000;
const DEPTH_MANTISSA_MASK: u16 = 0x1FFC;
const DEPTH_EXPONENT_SHIFT: u32 = 13;
const DEPTH_MANTISSA_SHIFT: u32 = 2;

/// Literal port of `ColorConverter::RGBA16::toRGBA(hlslpp::float4 src)`
/// (`rt64_color_converter.cpp:19-29`): four independent scalar
/// clamp-then-round conversions to a `[0,255]` byte, a redundant
/// re-clamp-to-255 before the 5-bit truncating shift (provably a no-op given
/// the prior `[0,255]` clamp, but ported literally rather than silently
/// dropped as dead code), and a hard 1-bit alpha coverage test: any
/// rounded/clamped alpha in `(0,255]` sets the bit, only exact `0` clears
/// it. `NaN` input clamps to `0.0` -- see the module doc comment's disclosed
/// departure from hlslpp's own (multi-valued) NaN behavior.
pub fn rgba16_to_packed(r: f32, g: f32, b: f32, a: f32) -> u16 {
    fn channel_to_5bit(c: f32) -> u16 {
        let clamped = if c.is_nan() {
            0.0
        } else {
            (c * 255.0).clamp(0.0, 255.0)
        };
        let rounded = clamped.round_ties_even() as u16;
        rounded.min(255) >> 3
    }
    let r = channel_to_5bit(r);
    let g = channel_to_5bit(g);
    let b = channel_to_5bit(b);
    let a_clamped = if a.is_nan() {
        0.0
    } else {
        (a * 255.0).clamp(0.0, 255.0)
    };
    let a_rounded = a_clamped.round_ties_even() as u16;
    let a_bit: u16 = if a_rounded > 0 { 1 } else { 0 };
    (r << 11) | (g << 6) | (b << 1) | a_bit
}

/// Literal port of `ColorConverter::RGBA16::toRGBAF(uint16_t src)`
/// (`rt64_color_converter.cpp:31-41`): pure bit extraction, no branches.
/// Each 5-bit RGB channel expands to 8 bits via `(x<<3)|(x>>2)` -- the same
/// formula already landed in `crate::tmem::texel`'s decode path (see the
/// module doc comment's Kinship section). Alpha is a hard 1-bit -> `{0.0,
/// 1.0}`, no smooth quantization.
pub fn rgba16_packed_to_float(src: u16) -> [f32; 4] {
    let r = ((src >> 11) & 0x1F) as u8;
    let g = ((src >> 6) & 0x1F) as u8;
    let b = ((src >> 1) & 0x1F) as u8;
    [
        (((r << 3) | (r >> 2)) as f32) / 255.0,
        (((g << 3) | (g >> 2)) as f32) / 255.0,
        (((b << 3) | (b >> 2)) as f32) / 255.0,
        if (src & 1) != 0 { 1.0 } else { 0.0 },
    ]
}

/// Literal port of `ColorConverter::RGBA32::toRGBA(hlslpp::float4 src)`
/// (`rt64_color_converter.cpp:45-48`): single vectorized round+clamp per
/// channel, big-endian `R:G:B:A` pack from bit 31 down. No conditional
/// logic. `NaN` input clamps to `0.0` -- see the module doc comment's
/// disclosed departure from hlslpp's own (multi-valued) NaN behavior.
pub fn rgba32_to_packed(r: f32, g: f32, b: f32, a: f32) -> u32 {
    fn channel_to_byte(c: f32) -> u32 {
        let clamped = if c.is_nan() {
            0.0
        } else {
            (c * 255.0).clamp(0.0, 255.0)
        };
        clamped.round_ties_even() as u32
    }
    (channel_to_byte(r) << 24)
        | (channel_to_byte(g) << 16)
        | (channel_to_byte(b) << 8)
        | channel_to_byte(a)
}

/// Literal port of `ColorConverter::RGBA32::toRGBAF(uint32_t src)`
/// (`rt64_color_converter.cpp:50-57`): pure bit extraction, big-endian
/// unpack, `/255.0`. No branches.
pub fn rgba32_packed_to_float(src: u32) -> [f32; 4] {
    [
        (((src >> 24) & 0xFF) as f32) / 255.0,
        (((src >> 16) & 0xFF) as f32) / 255.0,
        (((src >> 8) & 0xFF) as f32) / 255.0,
        ((src & 0xFF) as f32) / 255.0,
    ]
}

/// Literal port of `ColorConverter::D16::toF(uint16_t src)`
/// (`rt64_color_converter.cpp:61-70`) -- the one function in this module
/// with real branch structure: an 8-value (3-bit) exponent selects a
/// per-band mantissa shift and additive bias, both exhaustive over every
/// reachable `u16` bit pattern (no unreachable `match` arm to guard
/// against). The shift amount is `6 - exponent` for `exponent in 0..=6`;
/// for `exponent == 7`, `min(6,7) == 6` so the shift is also `0` --
/// exponent 6 and 7 share the same shift, a documented N64 RDP
/// piecewise-float depth quirk, not an implementation choice, preserved
/// literally rather than "fixed."
pub fn d16_to_float(src: u16) -> f32 {
    // Extract the exponent and mantissa from the depth buffer value.
    let exponent = ((src & DEPTH_EXPONENT_MASK) >> DEPTH_EXPONENT_SHIFT) as u32;
    let mantissa = ((src & DEPTH_MANTISSA_MASK) >> DEPTH_MANTISSA_SHIFT) as u32;
    // Convert the exponent and mantissa into a fixed-point value.
    let shifted_mantissa = mantissa << (6 - 6u32.min(exponent));
    let mantissa_bias = 0x40000u32 - (0x40000u32 >> exponent);
    (shifted_mantissa + mantissa_bias) as f32 / (32768.0 * 8.0 - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- rgba16_to_packed: independently hand-derived oracle ---

    #[test]
    fn rgba16_to_packed_full_red_opaque() {
        assert_eq!(rgba16_to_packed(1.0, 0.0, 0.0, 1.0), 0xF801);
    }

    #[test]
    fn rgba16_to_packed_all_zero() {
        assert_eq!(rgba16_to_packed(0.0, 0.0, 0.0, 0.0), 0x0000);
    }

    #[test]
    fn rgba16_to_packed_round_tie_regresses_to_even_not_up() {
        // r*255.0 lands exactly on 4.5; round_ties_even(4.5) = 4 (even), not
        // 5 -- distinguishes round_ties_even from f32::round()/round-half-up.
        let r = 4.5 / 255.0;
        let packed = rgba16_to_packed(r, 0.0, 0.0, 0.0);
        let r5 = (packed >> 11) & 0x1F;
        assert_eq!(r5, 0); // 4 >> 3 == 0
    }

    #[test]
    fn rgba16_to_packed_alpha_half_ulp_rounds_to_zero_coverage() {
        // a=0.5/255.0 -> a*255.0 == 0.5 exactly in f32 -> round_ties_even
        // sends the tie to the even integer 0, not 1 -- coverage bit stays
        // 0. Same round-half-to-even hazard as the r=4.5/255.0 regression
        // above, but gating the 1-bit alpha channel instead of an RGB value.
        let cases: [(f32, u16); 4] = [(0.0, 0), (0.5 / 255.0, 0), (0.5, 1), (1.0, 1)];
        for (a, expected_bit) in cases {
            let packed = rgba16_to_packed(0.0, 0.0, 0.0, a);
            assert_eq!(packed & 1, expected_bit, "a={a}");
        }
    }

    #[test]
    fn rgba16_to_packed_rgb_tie_regresses_to_even() {
        // g*255.0 == 12.5 exactly for g=12.5/255.0 -> round_ties_even(12.5)
        // = 12 (even), not 13.
        let g = 12.5 / 255.0;
        let packed = rgba16_to_packed(0.0, g, 0.0, 0.0);
        let g5 = (packed >> 6) & 0x1F;
        assert_eq!(g5, 12 >> 3); // 12 -> 5-bit value 1
    }

    #[test]
    fn rgba16_to_packed_clamps_out_of_range_and_nan() {
        for r in [-1.0f32, -100.0, f32::NEG_INFINITY] {
            assert_eq!(rgba16_to_packed(r, 0.0, 0.0, 0.0) >> 11, 0, "r={r}");
        }
        for r in [1.5f32, 100.0, f32::INFINITY] {
            assert_eq!(rgba16_to_packed(r, 0.0, 0.0, 0.0) >> 11, 31, "r={r}");
        }
        assert_eq!(rgba16_to_packed(f32::NAN, 0.0, 0.0, 0.0) >> 11, 0);
        assert_eq!(rgba16_to_packed(0.0, 0.0, 0.0, f32::NAN) & 1, 0);
        assert_eq!(rgba16_to_packed(-0.0, 0.0, 0.0, 0.0) >> 11, 0);
    }

    #[test]
    fn mutation_distinguishes_channel_shift_positions() {
        let r_only = rgba16_to_packed(1.0, 0.0, 0.0, 0.0);
        let g_only = rgba16_to_packed(0.0, 1.0, 0.0, 0.0);
        let b_only = rgba16_to_packed(0.0, 0.0, 1.0, 0.0);
        let a_only = rgba16_to_packed(0.0, 0.0, 0.0, 1.0);
        assert_eq!(r_only, 31 << 11);
        assert_eq!(g_only, 31 << 6);
        assert_eq!(b_only, 31 << 1);
        assert_eq!(a_only, 1);
        // All four masks are pairwise disjoint bit ranges.
        assert_eq!(r_only & g_only & b_only & a_only, 0);
    }

    #[test]
    fn mutation_distinguishes_redundant_reclamp_did_not_change_no_op_behavior() {
        // The redundant std::min(_,255) re-clamp before >>3 is provably a
        // no-op given the prior [0,255] clamp; confirm the max representable
        // 5-bit output (31) is still reachable, i.e. the re-clamp doesn't
        // accidentally cap below 255.
        assert_eq!(rgba16_to_packed(1.0, 1.0, 1.0, 1.0), 0xFFFF);
    }

    // --- rgba16_packed_to_float: independently hand-derived oracle ---

    #[test]
    fn rgba16_packed_to_float_round_trips_full_red_opaque() {
        assert_eq!(rgba16_packed_to_float(0xF801), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn rgba16_packed_to_float_all_zero() {
        assert_eq!(rgba16_packed_to_float(0x0000), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn rgba16_packed_to_float_alpha_is_hard_one_bit_not_smooth() {
        assert_eq!(rgba16_packed_to_float(0x0000)[3], 0.0);
        assert_eq!(rgba16_packed_to_float(0x0001)[3], 1.0);
    }

    #[test]
    fn rgba16_packed_to_float_five_to_eight_expansion_matches_independent_formula() {
        for five_bit in 0u16..32 {
            let src = five_bit << 11;
            let expected = (((five_bit << 3) | (five_bit >> 2)) as f32) / 255.0;
            assert_eq!(
                rgba16_packed_to_float(src)[0],
                expected,
                "five_bit={five_bit}"
            );
        }
    }

    #[test]
    fn mutation_distinguishes_unpack_field_positions() {
        let r_field = rgba16_packed_to_float(31 << 11);
        let g_field = rgba16_packed_to_float(31 << 6);
        let b_field = rgba16_packed_to_float(31 << 1);
        assert!(r_field[0] > 0.0 && r_field[1] == 0.0 && r_field[2] == 0.0);
        assert!(g_field[0] == 0.0 && g_field[1] > 0.0 && g_field[2] == 0.0);
        assert!(b_field[0] == 0.0 && b_field[1] == 0.0 && b_field[2] > 0.0);
    }

    // --- rgba32_to_packed: independently hand-derived oracle ---

    #[test]
    fn rgba32_to_packed_full_red_opaque() {
        assert_eq!(rgba32_to_packed(1.0, 0.0, 0.0, 1.0), 0xFF0000FF);
    }

    #[test]
    fn rgba32_to_packed_all_zero() {
        assert_eq!(rgba32_to_packed(0.0, 0.0, 0.0, 0.0), 0);
    }

    #[test]
    fn rgba32_to_packed_all_ones() {
        assert_eq!(rgba32_to_packed(1.0, 1.0, 1.0, 1.0), 0xFFFFFFFF);
    }

    #[test]
    fn rgba32_to_packed_clamps_out_of_range_and_nan() {
        assert_eq!(rgba32_to_packed(-1.0, 2.0, 0.5, -0.5) >> 24 & 0xFF, 0);
        assert_eq!((rgba32_to_packed(-1.0, 2.0, 0.5, -0.5) >> 16) & 0xFF, 255);
        assert_eq!(rgba32_to_packed(f32::NAN, 0.0, 0.0, 0.0) >> 24, 0);
        assert_eq!(rgba32_to_packed(f32::INFINITY, 0.0, 0.0, 0.0) >> 24, 255);
        assert_eq!(rgba32_to_packed(f32::NEG_INFINITY, 0.0, 0.0, 0.0) >> 24, 0);
    }

    #[test]
    fn mutation_distinguishes_byte_shift_positions() {
        let r_only = rgba32_to_packed(1.0, 0.0, 0.0, 0.0);
        let g_only = rgba32_to_packed(0.0, 1.0, 0.0, 0.0);
        let b_only = rgba32_to_packed(0.0, 0.0, 1.0, 0.0);
        let a_only = rgba32_to_packed(0.0, 0.0, 0.0, 1.0);
        assert_eq!(r_only, 0xFF000000);
        assert_eq!(g_only, 0x00FF0000);
        assert_eq!(b_only, 0x0000FF00);
        assert_eq!(a_only, 0x000000FF);
    }

    // --- rgba32_packed_to_float: independently hand-derived oracle ---

    #[test]
    fn rgba32_packed_to_float_matches_independently_composed_bytes() {
        let expected = [
            0x80 as f32 / 255.0,
            0x40 as f32 / 255.0,
            0x20 as f32 / 255.0,
            1.0,
        ];
        assert_eq!(rgba32_packed_to_float(0x804020FF), expected);
    }

    #[test]
    fn rgba32_packed_to_float_round_trips_all_zero_and_all_ones() {
        assert_eq!(rgba32_packed_to_float(0x00000000), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(rgba32_packed_to_float(0xFFFFFFFF), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn mutation_distinguishes_unpack_byte_positions() {
        let r_field = rgba32_packed_to_float(0xFF000000);
        let g_field = rgba32_packed_to_float(0x00FF0000);
        let b_field = rgba32_packed_to_float(0x0000FF00);
        let a_field = rgba32_packed_to_float(0x000000FF);
        assert_eq!(r_field, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(g_field, [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(b_field, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(a_field, [0.0, 0.0, 0.0, 1.0]);
    }

    // --- rgba16/rgba32 round-trip cross-check ---

    #[test]
    fn rgba32_round_trips_through_pack_and_unpack_for_byte_aligned_values() {
        for (r, g, b, a) in [
            (0x80u32, 0x40u32, 0x20u32, 0xFFu32),
            (0, 0, 0, 0),
            (255, 255, 255, 255),
        ] {
            let packed = (r << 24) | (g << 16) | (b << 8) | a;
            let floats = rgba32_packed_to_float(packed);
            let repacked = rgba32_to_packed(floats[0], floats[1], floats[2], floats[3]);
            assert_eq!(repacked, packed, "r={r} g={g} b={b} a={a}");
        }
    }

    // --- d16_to_float: full 16-point exponent x endpoint-mantissa matrix ---
    // (shiftedMantissa = mantissa << (6 - min(6, exponent)),
    //  bias = 0x40000 - (0x40000 >> exponent), value = (shiftedMantissa +
    //  bias) / 262143.0 -- hand-derived independently of this module's
    //  implementation, verified exact per the accepted dispatch card.)

    #[test]
    fn d16_to_float_zero_is_zero() {
        assert_eq!(d16_to_float(0x0000), 0.0);
    }

    #[test]
    fn d16_to_float_max_is_one_exactly() {
        assert_eq!(d16_to_float(0xFFFF), 1.0);
    }

    #[test]
    fn d16_to_float_exhaustive_exponent_by_endpoint_mantissa_matrix() {
        // (exponent, mantissa, expected value), all 16 hand-derived fixed
        // points from the accepted dispatch card.
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
            let src =
                ((exponent << DEPTH_EXPONENT_SHIFT) | (mantissa << DEPTH_MANTISSA_SHIFT)) as u16;
            let actual = d16_to_float(src) as f64;
            assert!(
                (actual - expected).abs() < 1e-6,
                "exponent={exponent} mantissa={mantissa:#x} src={src:#06x} expected={expected} actual={actual}"
            );
        }
    }

    #[test]
    fn d16_to_float_exponent_six_and_seven_share_shift_but_differ_in_bias() {
        // exponent 6 and 7 both compute shift = 6 - min(6, exponent) = 0, so
        // the same nonzero mantissa produces the same shiftedMantissa in
        // both bands -- but bias still differs, so the two fixed points are
        // distinct. This is the documented N64 RDP piecewise-float depth
        // quirk (min(6,exponent) saturation), not an implementation bug.
        let exp6 = (6u16 << DEPTH_EXPONENT_SHIFT) | (0x7FFu16 << DEPTH_MANTISSA_SHIFT);
        let exp7 = (7u16 << DEPTH_EXPONENT_SHIFT) | (0x7FFu16 << DEPTH_MANTISSA_SHIFT);
        assert_ne!(d16_to_float(exp6), d16_to_float(exp7));
    }

    #[test]
    fn d16_to_float_is_monotonically_nondecreasing_over_all_u16_values() {
        let mut prev = d16_to_float(0);
        for src in 1u32..=u16::MAX as u32 {
            let cur = d16_to_float(src as u16);
            assert!(cur >= prev, "src={src:#06x} prev={prev} cur={cur}");
            prev = cur;
        }
    }

    #[test]
    fn mutation_distinguishes_exponent_mask_from_mantissa_mask() {
        // Flipping only exponent bits vs only mantissa bits must produce
        // different results, proving the two masks/shifts are not swapped.
        let exponent_only = d16_to_float(0xE000); // exponent=7, mantissa=0
        let mantissa_only = d16_to_float(0x1FFC); // exponent=0, mantissa=0x7FF
        assert_ne!(exponent_only, mantissa_only);
        assert!(exponent_only > mantissa_only);
    }

    #[test]
    fn mutation_distinguishes_denominator_262143_from_262144() {
        // The exponent=3, mantissa=0 boundary is provably above the naive
        // quarter-point 0.875 (which would result from an off-by-one
        // denominator of 262144 instead of 262143): 229376/262144.0 == 0.875
        // exactly, but 229376/262143.0 == 0.8750033378728403, strictly
        // greater.
        let value = d16_to_float(0x6000) as f64; // exponent=3, mantissa=0
        assert!(value > 0.875, "value={value}");
        assert!((value - 0.8750033378728403).abs() < 1e-9, "value={value}");
    }
}
