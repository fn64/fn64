//! Alpha compare (port card §3): the `AlphaCompare`/`AlphaDither` gate.
//!
//! Characterization-first, selective literal port of
//! `crate::raster::blend::{alpha_compare_value, copy_alpha_compare_value,
//! apply_alpha_dither}` (`crates/fn64-render-reference/src/raster/blend.rs:28-149`),
//! per `/private/tmp/rt64-blender-depth-port-card.md` §3 ("Alpha compare").
//! Covers: `None`/`Threshold`/`Dither` general compare, the copy-cycle
//! RGBA16-alpha-bit special case with ordinary fallback for every other
//! format, loud typed rejection of the reserved encoding, and the four
//! alpha-dither modes (`Pattern`/`InversePattern`/`Noise`/`Disabled`) with
//! their exact 4x4 ordered-matrix substitution rule and 5-bit
//! quantize/expand rounding.
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference`
//! (see `depth_strict_less.rs`), so this is a self-contained literal
//! re-expression citing the reference's line numbers, matching this crate's
//! existing citation-comment convention.
//!
//! ## OtherMode-lane integration
//!
//! The OtherMode decode lane landed at `aff1279b` ("port full RT64
//! OtherMode bitfield contract (decode-only)"), adding
//! `crate::state::{AlphaCompare, AlphaDither, RgbDither}` plus
//! `OtherMode::alpha_compare()`/`alpha_dither()`/`rgb_dither()` accessors.
//! This module consumes those landed types directly (aliased below to the
//! `Mode`-suffixed names this file's functions were originally written
//! against, so no call-site churn was needed at integration time) rather
//! than defining its own copies -- the wire encodings and variant order are
//! byte-identical to what this module verified against that lane's working
//! tree pre-land.
//!
//! Explicitly out of scope, per the port card: the general blend
//! equation/coverage/depth pipeline, `BlendColor` register storage
//! (`threshold_alpha` below is an explicit caller-supplied parameter, not
//! crate-owned state -- this module itself still does not own it; the
//! draw-path collectors do, see below), RDP noise-generator architecture
//! (`AlphaCompareNoise` below is a narrow typed byte carrier, not a PRNG),
//! and any native GPU execution.
//!
//! **Draw-path integration is no longer fully out of scope** (2026-08-17,
//! `fn64-alpha-compare-production-card.md`): `None`/`Threshold` are wired
//! into the real triangle fragment path (`targets/triangle_pipeline.rs`'s
//! `submit_admitted_triangle`, `shaders/triangle_pipeline_fragment.wgsl`'s
//! `fs_main`, reusing `alpha_compare_fragment_fn.wgsl`'s callable verbatim),
//! with `BlendColor` snapshotted per-triangle by
//! `raw_dpc::triangle_draw_data::TriangleDrawStateCollector` and
//! `production.rs`'s `PlanCollector` (this module still does not own that
//! state itself). `Dither` remains a loud, named, unimplemented trap
//! (no fragment-callable RT64 PRNG binding and no frame-count concept in
//! this pipeline); `Reserved` is rejected at retrieval time via
//! `require_supported_alpha_compare`'s first real call site.

use crate::state::{
    AlphaCompare as AlphaCompareMode, AlphaDither as AlphaDitherMode, RgbDither as RgbDitherMode,
};

/// One eight-bit pseudo-random sample routed to a covered fragment's
/// alpha-compare dither test and alpha-dither pattern. Mirrors the
/// reference's private `NoiseSample` accessor shapes (`byte()`, `dither()`)
/// as a typed carrier; this module does not generate noise, matching the
/// port card's explicit non-claim that the RDP's generator is unpublished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaCompareNoise(pub u8);

impl AlphaCompareNoise {
    pub const fn byte(self) -> u8 {
        self.0
    }

    /// Low three bits only, the alpha-dither `Noise` mode's threshold input.
    pub const fn dither(self) -> u8 {
        self.0 & 7
    }
}

/// Minimal copy-cycle source-texture shape needed by
/// [`copy_alpha_compare_value`]: whether the texel format/size is exactly
/// RGBA16, the one combination with the hard-alpha-bit special case.
/// Deliberately not the reference's full `Texture` struct -- this module
/// does not own texture state, per its narrow-adapter scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyCycleSourceFormat {
    pub is_rgba16: bool,
}

impl CopyCycleSourceFormat {
    pub const RGBA16: Self = Self { is_rgba16: true };
    pub const OTHER: Self = Self { is_rgba16: false };
}

/// Loud typed rejection of the reserved alpha-compare encoding. Literal port
/// of `require_supported_alpha_compare`
/// (`crates/fn64-render-reference/src/raster/blend.rs:4-11`): a
/// GBI-decode-time validation seam, not a runtime fragment-shader branch.
/// Panics naming the offending primitive rather than silently coercing
/// encoding 2 into `None` or `Threshold`.
pub fn require_supported_alpha_compare(mode: AlphaCompareMode, primitive: &str) {
    match mode {
        AlphaCompareMode::None | AlphaCompareMode::Threshold | AlphaCompareMode::Dither => {}
        AlphaCompareMode::Reserved => {
            panic!("{primitive} selected reserved G_AC alpha-compare mode 2")
        }
    }
}

/// General (triangle/line/rect) alpha-compare gate. Literal port of
/// `alpha_compare_value` (`blend.rs:105-123`).
///
/// - `None` always passes.
/// - `Threshold` passes iff `alpha >= threshold_alpha`, where
///   `threshold_alpha` is `G_SETBLENDCOLOR.a` (caller-supplied; this module
///   does not own `BlendColor` state).
/// - `Dither` cross-multiplies `alpha*256 > noise_byte*255` so `alpha=0`
///   always rejects and `alpha=255` always passes -- Programming Manual
///   §15.5.4's "alpha greater than a random value in [0,1)".
///
/// # Panics
/// If `mode` is `Reserved` -- callers must reject that encoding at
/// GBI-decode time via [`require_supported_alpha_compare`] before ever
/// reaching a fragment with this mode set.
pub const fn alpha_compare_value(
    mode: AlphaCompareMode,
    alpha: u8,
    threshold_alpha: u8,
    noise: AlphaCompareNoise,
) -> bool {
    match mode {
        AlphaCompareMode::None => true,
        AlphaCompareMode::Threshold => alpha >= threshold_alpha,
        AlphaCompareMode::Dither => alpha as u32 * 256 > noise.byte() as u32 * 255,
        AlphaCompareMode::Reserved => {
            panic!("reserved alpha compare is rejected before rasterization")
        }
    }
}

/// Copy-cycle alpha-compare gate. Literal port of
/// `copy_alpha_compare_value` (`blend.rs:129-149`). Programming Manual
/// §15.5.4: an RGBA16 source texel does not enter the eight-bit comparator
/// at all -- its single alpha bit is a hard write-enable (`alpha != 0`).
/// Every other source format falls through to the ordinary
/// threshold/dither arithmetic in [`alpha_compare_value`].
///
/// # Panics
/// If `mode` is `Reserved`.
pub const fn copy_alpha_compare_value(
    mode: AlphaCompareMode,
    source: CopyCycleSourceFormat,
    alpha: u8,
    threshold_alpha: u8,
    noise: AlphaCompareNoise,
) -> bool {
    match mode {
        AlphaCompareMode::None => true,
        AlphaCompareMode::Threshold | AlphaCompareMode::Dither if source.is_rgba16 => alpha != 0,
        AlphaCompareMode::Threshold => alpha >= threshold_alpha,
        AlphaCompareMode::Dither => alpha as u32 * 256 > noise.byte() as u32 * 255,
        AlphaCompareMode::Reserved => {
            panic!("reserved alpha compare is rejected before copy rasterization")
        }
    }
}

/// Screen-registered three-bit thresholds for the two ordered RGB dither
/// tiles, consumed by the alpha-dither `Pattern`/`InversePattern`
/// substitution rule below.
///
/// **Reads [`crate::rgb_dither`]'s tables; does not carry its own.** This
/// function used to duplicate the tables as local `MAGIC_SQUARE`/`BAYER`
/// constants ported from `ordered_rgb_dither_threshold` (`blend.rs:28-38`),
/// while [`crate::rgb_dither`] carried RT64's `DitherPatternBayer`
/// (`Formats.hlsli:9-14`). The two Bayer tiles disagree at rows 1 and 2,
/// so this crate carried **two different Bayer tables for one hardware
/// quantity**, and whichever is right, at most one of the two sites could
/// have been.
///
/// The split is a defect independent of which tile matches silicon, because
/// libultra defines the alpha-dither `G_AD_PATTERN` threshold as *the
/// currently selected RGB dither matrix* (`gbi.h:674-678`; the substitution
/// rule is restated at [`apply_alpha_dither`]). The two paths are therefore
/// required to read the **same** tile by definition, whatever it contains.
///
/// [`crate::rgb_dither`] is the site kept, and the reason is that same
/// definition rather than a judgement about the tables: it *is* this
/// crate's RGB dither module, so "the currently selected RGB dither matrix"
/// is the thing it owns, and alpha dither is downstream of it. Deleting the
/// duplicate here removes the possibility of the two drifting again;
/// keeping this side instead would have inverted the dependency libultra
/// states.
///
/// **This resolves no hardware question and claims none.** Which Bayer
/// arrangement the RDP actually uses is the open frontier
/// [`crate::rgb_dither`]'s module header records and
/// `docs/RT64-LANE-DIVERGENCES.md` D19 scores UNKNOWN -- `gbi.h` publishes
/// the `G_CD_BAYER` selector bit and no table, and RT64 is one of the two
/// disputants, not an adjudicator. If that question is ever settled against
/// RT64's arrangement, exactly one table changes and both paths follow it,
/// which is the whole point of removing the copy.
///
/// # Panics
/// If `mode` is `Noise` or `Disabled` -- both lack an ordered tile and must
/// be resolved to `MagicSquare`/`Bayer` by the caller (see
/// [`apply_alpha_dither`]'s substitution rule) before reaching this
/// function. [`crate::rgb_dither::dither_pattern_value`] answers those two
/// modes rather than panicking, so this narrowing is enforced here and the
/// noise byte it would otherwise need is deliberately not threaded in.
const fn ordered_dither_threshold(mode: RgbDitherMode, x: i32, y: i32) -> u8 {
    match mode {
        RgbDitherMode::MagicSquare | RgbDitherMode::Bayer => {
            crate::rgb_dither::ordered_tile_value(mode, x, y)
        }
        RgbDitherMode::Noise | RgbDitherMode::Disabled => {
            panic!("ordered dither threshold requested for a non-ordered RgbDitherMode")
        }
    }
}

/// [`ordered_dither_threshold`], reachable from `rgb_dither`'s
/// cross-module agreement test.
///
/// A `#[cfg(test)]` accessor rather than making the function itself
/// `pub(crate)`: the production surface of this module is
/// [`apply_alpha_dither`], and widening the private helper's visibility to
/// serve a test would invite a caller that bypasses the substitution rule.
/// The test needs the tile lookup specifically, so that is what is exposed.
#[cfg(test)]
pub(crate) const fn alpha_dither_pattern_threshold_for_tests(
    mode: RgbDitherMode,
    x: i32,
    y: i32,
) -> u8 {
    ordered_dither_threshold(mode, x, y)
}

/// Pre-blend alpha dither: reduce post-combiner pixel alpha to the
/// blender's five-bit input. Literal port of `apply_alpha_dither`
/// (`blend.rs:75-103`). Distinct from alpha *compare* dither above (same
/// noise byte, different consumer) -- do not conflate the two.
///
/// Public `gDPSetAlphaDither` defines `PATTERN` as the currently-selected
/// RGB dither matrix, with `Bayer` substituted when RGB dither is
/// `Disabled` and `MagicSquare` substituted when RGB dither is `Noise`.
/// `InversePattern` (`NOTPATTERN`) reverses the three-bit threshold
/// (`7 - threshold`).
///
/// Rounding: `rounded = (alpha>>3) + (1 if (alpha&7)>threshold else 0)`,
/// clamped to 31, then expanded back to eight bits via
/// `(five<<3)|(five>>2)` -- the standard N64 5-to-8-bit channel expansion
/// (replicate the top two bits into the low two bits of the result).
pub const fn apply_alpha_dither(
    alpha: u8,
    alpha_mode: AlphaDitherMode,
    rgb_mode: RgbDitherMode,
    x: i32,
    y: i32,
    noise: AlphaCompareNoise,
) -> u8 {
    let threshold = match alpha_mode {
        AlphaDitherMode::Disabled => return alpha,
        AlphaDitherMode::Noise => noise.dither(),
        AlphaDitherMode::Pattern | AlphaDitherMode::InversePattern => {
            let pattern = match rgb_mode {
                RgbDitherMode::MagicSquare | RgbDitherMode::Bayer => rgb_mode,
                RgbDitherMode::Noise => RgbDitherMode::MagicSquare,
                RgbDitherMode::Disabled => RgbDitherMode::Bayer,
            };
            let threshold = ordered_dither_threshold(pattern, x, y);
            if matches!(alpha_mode, AlphaDitherMode::InversePattern) {
                7 - threshold
            } else {
                threshold
            }
        }
    };
    let rounded = (alpha >> 3) as u16 + ((alpha & 7) > threshold) as u16;
    let five = if rounded > 31 { 31 } else { rounded as u8 };
    (five << 3) | (five >> 2)
}

pub const ALPHA_COMPARE_WGSL: &str = include_str!("alpha_compare.wgsl");
pub const ALPHA_COMPARE_ENTRY_POINT: &str = "alpha_compare_fragment";

/// Fragment-callable twin of [`ALPHA_COMPARE_WGSL`]'s existing
/// `general_compare`/`evaluate` compute-shader logic: an ordinary WGSL
/// function (`alpha_compare_fragment_fn`, no `@compute`, no
/// `@group`/`@binding`, no entry point) taking scalar arguments and
/// returning `bool`, concatenatable into a future `@fragment` entry point
/// the same way `color_combiner.wgsl` already is per
/// `shaders/triangle_pipeline_fragment.wgsl`'s header. Not wired into any
/// draw path, bind group layout, or pipeline used elsewhere in this crate --
/// see this module's doc comment and the sibling `alpha_compare.wgsl`'s own
/// header for the shared scope boundary. The existing `ALPHA_COMPARE_WGSL`
/// `@compute` entry point is untouched by this addition.
pub const ALPHA_COMPARE_FRAGMENT_FN_WGSL: &str = include_str!("alpha_compare_fragment_fn.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(byte: u8) -> AlphaCompareNoise {
        AlphaCompareNoise(byte)
    }

    #[test]
    fn none_mode_always_passes() {
        for alpha in [0u8, 1, 128, 254, 255] {
            for threshold in [0u8, 128, 255] {
                assert!(alpha_compare_value(
                    AlphaCompareMode::None,
                    alpha,
                    threshold,
                    noise(0)
                ));
                assert!(alpha_compare_value(
                    AlphaCompareMode::None,
                    alpha,
                    threshold,
                    noise(255)
                ));
            }
        }
    }

    #[test]
    fn threshold_mode_is_greater_or_equal_not_strict() {
        assert!(alpha_compare_value(
            AlphaCompareMode::Threshold,
            128,
            128,
            noise(0)
        ));
        assert!(!alpha_compare_value(
            AlphaCompareMode::Threshold,
            127,
            128,
            noise(0)
        ));
        assert!(alpha_compare_value(
            AlphaCompareMode::Threshold,
            129,
            128,
            noise(0)
        ));
    }

    #[test]
    fn threshold_mode_boundary_values() {
        assert!(alpha_compare_value(
            AlphaCompareMode::Threshold,
            0,
            0,
            noise(0)
        ));
        assert!(alpha_compare_value(
            AlphaCompareMode::Threshold,
            255,
            255,
            noise(0)
        ));
        assert!(!alpha_compare_value(
            AlphaCompareMode::Threshold,
            0,
            255,
            noise(0)
        ));
        assert!(alpha_compare_value(
            AlphaCompareMode::Threshold,
            255,
            0,
            noise(0)
        ));
    }

    #[test]
    fn dither_mode_alpha_zero_always_rejects() {
        for noise_byte in 0u16..=255 {
            assert!(!alpha_compare_value(
                AlphaCompareMode::Dither,
                0,
                0,
                noise(noise_byte as u8)
            ));
        }
    }

    #[test]
    fn dither_mode_alpha_max_always_passes() {
        for noise_byte in 0u16..=255 {
            assert!(alpha_compare_value(
                AlphaCompareMode::Dither,
                255,
                0,
                noise(noise_byte as u8)
            ));
        }
    }

    #[test]
    fn dither_mode_exhaustive_256x256_matches_cross_multiply() {
        // Full alpha x noise_byte cross-product, per the port card's
        // explicit "cheap and should be exhaustive" partition (§3).
        for alpha in 0u32..=255 {
            for noise_byte in 0u32..=255 {
                let expected = alpha * 256 > noise_byte * 255;
                assert_eq!(
                    alpha_compare_value(
                        AlphaCompareMode::Dither,
                        alpha as u8,
                        0,
                        noise(noise_byte as u8)
                    ),
                    expected,
                    "alpha={alpha} noise_byte={noise_byte}"
                );
            }
        }
    }

    #[test]
    fn dither_mode_ignores_threshold_alpha_entirely() {
        // Threshold-alpha is a Threshold-mode-only input; Dither mode must
        // not read it at all. Same (alpha, noise) pair, varying
        // threshold_alpha, must always agree.
        for threshold in [0u8, 1, 127, 128, 254, 255] {
            assert_eq!(
                alpha_compare_value(AlphaCompareMode::Dither, 200, threshold, noise(100)),
                200u32 * 256 > 100u32 * 255
            );
        }
    }

    #[test]
    #[should_panic(expected = "reserved alpha compare is rejected before rasterization")]
    fn reserved_mode_panics_loudly_in_general_path() {
        alpha_compare_value(AlphaCompareMode::Reserved, 255, 0, noise(0));
    }

    #[test]
    #[should_panic(expected = "reserved alpha compare is rejected before copy rasterization")]
    fn reserved_mode_panics_loudly_in_copy_path() {
        copy_alpha_compare_value(
            AlphaCompareMode::Reserved,
            CopyCycleSourceFormat::OTHER,
            255,
            0,
            noise(0),
        );
    }

    #[test]
    #[should_panic(expected = "selected reserved G_AC alpha-compare mode 2")]
    fn require_supported_alpha_compare_panics_naming_primitive() {
        require_supported_alpha_compare(AlphaCompareMode::Reserved, "gSPTriangle");
    }

    #[test]
    fn require_supported_alpha_compare_accepts_every_non_reserved_mode() {
        require_supported_alpha_compare(AlphaCompareMode::None, "p");
        require_supported_alpha_compare(AlphaCompareMode::Threshold, "p");
        require_supported_alpha_compare(AlphaCompareMode::Dither, "p");
    }

    #[test]
    fn copy_cycle_rgba16_uses_hard_alpha_bit_not_threshold() {
        // Threshold mode would normally reject alpha=1 against a high
        // threshold; RGBA16 copy-cycle bypasses that and passes on any
        // nonzero alpha.
        assert!(copy_alpha_compare_value(
            AlphaCompareMode::Threshold,
            CopyCycleSourceFormat::RGBA16,
            1,
            255,
            noise(0)
        ));
        assert!(!copy_alpha_compare_value(
            AlphaCompareMode::Threshold,
            CopyCycleSourceFormat::RGBA16,
            0,
            0,
            noise(0)
        ));
        assert!(copy_alpha_compare_value(
            AlphaCompareMode::Dither,
            CopyCycleSourceFormat::RGBA16,
            1,
            0,
            noise(255)
        ));
        assert!(!copy_alpha_compare_value(
            AlphaCompareMode::Dither,
            CopyCycleSourceFormat::RGBA16,
            0,
            0,
            noise(0)
        ));
    }

    #[test]
    fn copy_cycle_non_rgba16_falls_through_to_ordinary_arithmetic() {
        assert!(!copy_alpha_compare_value(
            AlphaCompareMode::Threshold,
            CopyCycleSourceFormat::OTHER,
            1,
            255,
            noise(0)
        ));
        assert_eq!(
            copy_alpha_compare_value(
                AlphaCompareMode::Threshold,
                CopyCycleSourceFormat::OTHER,
                200,
                128,
                noise(0)
            ),
            200 >= 128
        );
    }

    #[test]
    fn copy_cycle_none_mode_always_passes_regardless_of_format() {
        assert!(copy_alpha_compare_value(
            AlphaCompareMode::None,
            CopyCycleSourceFormat::RGBA16,
            0,
            255,
            noise(0)
        ));
        assert!(copy_alpha_compare_value(
            AlphaCompareMode::None,
            CopyCycleSourceFormat::OTHER,
            0,
            255,
            noise(0)
        ));
    }

    #[test]
    fn copy_cycle_exhaustive_256x256_dither_matches_general_for_non_rgba16() {
        for alpha in 0u32..=255 {
            for noise_byte in 0u32..=255 {
                let expected = alpha * 256 > noise_byte * 255;
                assert_eq!(
                    copy_alpha_compare_value(
                        AlphaCompareMode::Dither,
                        CopyCycleSourceFormat::OTHER,
                        alpha as u8,
                        0,
                        noise(noise_byte as u8)
                    ),
                    expected
                );
            }
        }
    }

    #[test]
    fn alpha_dither_disabled_passes_through_unchanged() {
        for alpha in 0u16..=255 {
            assert_eq!(
                apply_alpha_dither(
                    alpha as u8,
                    AlphaDitherMode::Disabled,
                    RgbDitherMode::MagicSquare,
                    0,
                    0,
                    noise(0)
                ),
                alpha as u8
            );
        }
    }

    #[test]
    fn alpha_dither_noise_uses_low_three_bits_of_noise_byte() {
        // noise byte 0b0000_1101 -> dither() = 0b101 = 5.
        let n = noise(0b0000_1101);
        assert_eq!(n.dither(), 5);
        // alpha low bits (alpha & 7) = 6 > threshold 5 -> rounds up one step.
        let alpha = 0b1111_0110u8; // high=0b11110=30, low=6
        let result =
            apply_alpha_dither(alpha, AlphaDitherMode::Noise, RgbDitherMode::Noise, 0, 0, n);
        let expected_five = 31u8;
        assert_eq!(result, (expected_five << 3) | (expected_five >> 2));
    }

    #[test]
    fn alpha_dither_pattern_substitutes_bayer_when_rgb_disabled() {
        // rgb_mode=Disabled -> pattern substitutes to Bayer per the rule.
        for x in 0..4 {
            for y in 0..4 {
                let via_pattern = apply_alpha_dither(
                    0b1111_0111, // low bits = 7, always exceeds any 0..=7 threshold except 7
                    AlphaDitherMode::Pattern,
                    RgbDitherMode::Disabled,
                    x,
                    y,
                    noise(0),
                );
                let bayer_threshold = ordered_dither_threshold(RgbDitherMode::Bayer, x, y);
                let expected_bump = (7 > bayer_threshold) as u16;
                let expected_five = (0b11110u16 + expected_bump).min(31) as u8;
                let expected = (expected_five << 3) | (expected_five >> 2);
                assert_eq!(via_pattern, expected, "x={x} y={y}");
            }
        }
    }

    #[test]
    fn alpha_dither_pattern_substitutes_magic_square_when_rgb_is_noise() {
        for x in 0..4 {
            for y in 0..4 {
                let via_pattern = apply_alpha_dither(
                    0b1111_0111,
                    AlphaDitherMode::Pattern,
                    RgbDitherMode::Noise,
                    x,
                    y,
                    noise(0),
                );
                let magic_threshold = ordered_dither_threshold(RgbDitherMode::MagicSquare, x, y);
                let expected_bump = (7 > magic_threshold) as u16;
                let expected_five = (0b11110u16 + expected_bump).min(31) as u8;
                let expected = (expected_five << 3) | (expected_five >> 2);
                assert_eq!(via_pattern, expected, "x={x} y={y}");
            }
        }
    }

    #[test]
    fn alpha_dither_pattern_uses_ordinary_matrices_when_rgb_ordered() {
        for (rgb_mode, matrix_mode) in [
            (RgbDitherMode::MagicSquare, RgbDitherMode::MagicSquare),
            (RgbDitherMode::Bayer, RgbDitherMode::Bayer),
        ] {
            for x in 0..4 {
                for y in 0..4 {
                    let via_pattern = apply_alpha_dither(
                        0b1111_0111,
                        AlphaDitherMode::Pattern,
                        rgb_mode,
                        x,
                        y,
                        noise(0),
                    );
                    let threshold = ordered_dither_threshold(matrix_mode, x, y);
                    let expected_bump = (7 > threshold) as u16;
                    let expected_five = (0b11110u16 + expected_bump).min(31) as u8;
                    let expected = (expected_five << 3) | (expected_five >> 2);
                    assert_eq!(via_pattern, expected, "rgb_mode={rgb_mode:?} x={x} y={y}");
                }
            }
        }
    }

    #[test]
    fn alpha_dither_inverse_pattern_reverses_threshold() {
        for x in 0..4 {
            for y in 0..4 {
                let base_threshold = ordered_dither_threshold(RgbDitherMode::Bayer, x, y);
                let inverse_threshold = 7 - base_threshold;
                let alpha = 0b0000_0100u8; // low bits = 4
                let pattern_bump = (4u8 > base_threshold) as u16;
                let inverse_bump = (4u8 > inverse_threshold) as u16;
                let pattern = apply_alpha_dither(
                    alpha,
                    AlphaDitherMode::Pattern,
                    RgbDitherMode::Bayer,
                    x,
                    y,
                    noise(0),
                );
                let inverse = apply_alpha_dither(
                    alpha,
                    AlphaDitherMode::InversePattern,
                    RgbDitherMode::Bayer,
                    x,
                    y,
                    noise(0),
                );
                let pattern_five = pattern_bump.min(31) as u8;
                let inverse_five = inverse_bump.min(31) as u8;
                assert_eq!(pattern, (pattern_five << 3) | (pattern_five >> 2));
                assert_eq!(inverse, (inverse_five << 3) | (inverse_five >> 2));
                if base_threshold != inverse_threshold {
                    // Different thresholds generally produce different bump
                    // decisions for at least some alpha low-bits value; this
                    // specific alpha=4 case need not always differ (both
                    // could round the same way), so this test only asserts
                    // both are internally consistent with their own
                    // threshold, checked above.
                }
            }
        }
    }

    #[test]
    fn alpha_dither_5_bit_quantize_expand_matches_standard_replication_exhaustively() {
        // Exhaustive over the function under test (not a re-derivation):
        // every alpha 0..=255 through Pattern/Bayer at every one of the 16
        // matrix cells must equal the documented (v<<3)|(v>>2) expansion of
        // its own rounded 5-bit value, and every produced 8-bit result must
        // be a valid replicated-top-bits expansion (low 3 bits equal the
        // high 3 bits of the whole byte).
        for alpha in 0u16..=255 {
            for x in 0..4 {
                for y in 0..4 {
                    let threshold = ordered_dither_threshold(RgbDitherMode::Bayer, x, y);
                    let bump = ((alpha as u8 & 7) > threshold) as u16;
                    let expected_five = ((alpha >> 3) + bump).min(31) as u8;
                    let expected_eight = (expected_five << 3) | (expected_five >> 2);
                    let actual = apply_alpha_dither(
                        alpha as u8,
                        AlphaDitherMode::Pattern,
                        RgbDitherMode::Bayer,
                        x,
                        y,
                        noise(0),
                    );
                    assert_eq!(actual, expected_eight, "alpha={alpha} x={x} y={y}");
                    // Standard 5-to-8-bit replication invariant: the result's
                    // low 3 bits equal its top 3 bits.
                    assert_eq!(actual & 0b111, actual >> 5, "alpha={alpha} x={x} y={y}");
                }
            }
        }
    }

    #[test]
    fn alpha_dither_rounded_value_clamps_at_31() {
        // alpha=0b1111_1111: high=31 already, any bump must clamp not
        // overflow into a 6th bit.
        let alpha = 0b1111_1111u8;
        let result = apply_alpha_dither(
            alpha,
            AlphaDitherMode::Pattern,
            RgbDitherMode::Bayer,
            0,
            0,
            noise(0),
        );
        // 31 clamped, expanded: (31<<3)|(31>>2) = 248|7 = 255.
        assert_eq!(result, 255);
    }

    #[test]
    #[should_panic(expected = "ordered dither threshold requested for a non-ordered RgbDitherMode")]
    fn ordered_dither_threshold_panics_for_noise_mode() {
        ordered_dither_threshold(RgbDitherMode::Noise, 0, 0);
    }

    #[test]
    #[should_panic(expected = "ordered dither threshold requested for a non-ordered RgbDitherMode")]
    fn ordered_dither_threshold_panics_for_disabled_mode() {
        ordered_dither_threshold(RgbDitherMode::Disabled, 0, 0);
    }

    #[test]
    fn ordered_dither_threshold_every_matrix_cell_covers_0_through_7_twice() {
        for mode in [RgbDitherMode::MagicSquare, RgbDitherMode::Bayer] {
            let mut seen = [0u8; 8];
            for y in 0..4 {
                for x in 0..4 {
                    let value = ordered_dither_threshold(mode, x, y);
                    assert!(value <= 7);
                    seen[value as usize] += 1;
                }
            }
            assert_eq!(seen, [2; 8], "mode={mode:?}");
        }
    }

    #[test]
    fn ordered_dither_threshold_wraps_position_modulo_four() {
        for mode in [RgbDitherMode::MagicSquare, RgbDitherMode::Bayer] {
            for x in 0..4 {
                for y in 0..4 {
                    assert_eq!(
                        ordered_dither_threshold(mode, x, y),
                        ordered_dither_threshold(mode, x + 4, y + 4)
                    );
                    assert_eq!(
                        ordered_dither_threshold(mode, x, y),
                        ordered_dither_threshold(mode, x - 4, y - 4)
                    );
                    assert_eq!(
                        ordered_dither_threshold(mode, x, y),
                        ordered_dither_threshold(mode, x - 400, y - 400)
                    );
                }
            }
        }
    }

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(ALPHA_COMPARE_WGSL.contains(&format!("fn {ALPHA_COMPARE_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(ALPHA_COMPARE_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn wgsl_source_uses_the_exact_cross_multiply_once() {
        // Loud structural guard: the dither compare's cross-multiply must
        // appear exactly once and use the documented 256/255 constants, not
        // e.g. a plain alpha/255.0 > noise/256.0 float comparison that could
        // round differently at the boundary.
        assert_eq!(
            ALPHA_COMPARE_WGSL
                .matches("alpha * 256u > noise_byte * 255u")
                .count(),
            1
        );
    }

    #[test]
    fn naga_cannot_catch_a_flipped_threshold_comparison_direction() {
        // A `>=` -> `>` mutation still parses and validates under naga --
        // naga catches syntax/typing errors, not semantic direction flips.
        // This documents (not enforces) that WGSL/Rust semantic equivalence
        // is carried by this file's source-text identity guards
        // (`wgsl_source_uses_the_exact_cross_multiply_once` and the
        // `contains` assertions below), not by naga validation alone --
        // matching `depth_strict_less.rs`'s identically-named-in-spirit
        // precedent test.
        let flipped = ALPHA_COMPARE_WGSL.replace(
            "return alpha >= threshold_alpha;",
            "return alpha > threshold_alpha;",
        );
        assert_ne!(flipped, ALPHA_COMPARE_WGSL);
        let module = naga::front::wgsl::parse_str(&flipped).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_ok());
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = ALPHA_COMPARE_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    #[test]
    fn malformed_wgsl_fails_to_parse() {
        let truncated = &ALPHA_COMPARE_WGSL[..ALPHA_COMPARE_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    /// Structural guard, not a GPU-executed differential: this crate has no
    /// compute-dispatch test harness (see `depth_strict_less.rs`'s
    /// identically-scoped precedent, which is explicit about the same
    /// limitation), so this asserts the WGSL source's frozen text contains
    /// the exact literal expressions the Rust oracle's arithmetic depends
    /// on -- `None` always-pass, the `>=` threshold comparison, and the
    /// `*256u`/`*255u` dither cross-multiply -- rather than re-deriving that
    /// arithmetic in Rust a second time (which would prove nothing about
    /// the WGSL text; the exhaustive 256x256 tests above already cover the
    /// Rust oracle's own correctness). A source-text change to any of these
    /// three literals fails this test even though naga's validator would
    /// still accept the mutated shader.
    #[test]
    fn wgsl_source_contains_the_exact_literal_expressions_the_oracle_depends_on() {
        assert!(ALPHA_COMPARE_WGSL.contains("return true;"));
        assert!(ALPHA_COMPARE_WGSL.contains("return alpha >= threshold_alpha;"));
        assert!(ALPHA_COMPARE_WGSL.contains("alpha * 256u > noise_byte * 255u"));
    }

    // -- Fragment-callable WGSL seam (port card
    // `/private/tmp/fn64-rt64-alpha-compare-fragment-seam-card.md` §2-§4) --
    //
    // `ALPHA_COMPARE_FRAGMENT_FN_WGSL` is a new sibling file, not an edit to
    // `alpha_compare.wgsl` above; every test in this section exercises only
    // the new file, leaving every existing test above untouched.

    #[test]
    fn fragment_fn_wgsl_declares_no_entry_point_or_bindings() {
        // The whole point of the fragment-callable form: no `@compute`, no
        // `@group`/`@binding`, so it is an ordinary concatenatable function,
        // not a standalone dispatchable shader.
        assert!(!ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("@compute"));
        assert!(!ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("@group"));
        assert!(!ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("@binding"));
        assert!(ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("fn alpha_compare_fragment_fn("));
    }

    #[test]
    fn fragment_fn_wgsl_parses_and_validates_under_closed_naga_profile() {
        // A bare function with no entry point is still a complete WGSL
        // translation unit naga can parse/validate on its own.
        let module = naga::front::wgsl::parse_str(ALPHA_COMPARE_FRAGMENT_FN_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn fragment_fn_wgsl_malformed_source_fails_to_parse() {
        // Truncate mid-declaration rather than at the file's exact
        // midpoint: this file's header comment is proportionally larger
        // than `alpha_compare.wgsl`'s (no storage-buffer struct/bindings to
        // document), so a byte-count half-split can land entirely inside
        // the comment and still parse as valid (comment-only) WGSL. Cutting
        // partway through the first function's body guarantees a genuine
        // parse failure while still exercising the same "truncated source"
        // shape as the sibling test.
        let cut = ALPHA_COMPARE_FRAGMENT_FN_WGSL
            .find("fn alpha_compare_general")
            .expect("fixture source must contain the first function")
            + "fn alpha_compare_general(mode: u32, alpha: u32, threshold_alpha".len();
        let truncated = &ALPHA_COMPARE_FRAGMENT_FN_WGSL[..cut];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    #[test]
    fn fragment_fn_wgsl_source_uses_the_exact_cross_multiply_once() {
        assert_eq!(
            ALPHA_COMPARE_FRAGMENT_FN_WGSL
                .matches("alpha * 256u > noise_byte * 255u")
                .count(),
            1
        );
    }

    #[test]
    fn fragment_fn_wgsl_source_contains_the_exact_literal_expressions_the_oracle_depends_on() {
        assert!(ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("return true;"));
        assert!(ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("return alpha >= threshold_alpha;"));
        assert!(ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("alpha * 256u > noise_byte * 255u"));
        assert!(ALPHA_COMPARE_FRAGMENT_FN_WGSL.contains("return alpha != 0u;"));
    }

    /// One frozen fixture case for the CPU-vs-WGSL differential (port card
    /// §4): mode as the raw wire encoding (`0=None,1=Threshold,2=Reserved,
    /// 3=Dither`, matching `alpha_compare.wgsl`'s own convention),
    /// `copy_cycle_rgba16` as `0u`/`1u`, and the hand-derived `expected`
    /// boolean stated in the port card, not re-derived here.
    struct AlphaCompareFixture {
        name: &'static str,
        mode: u32,
        alpha: u32,
        threshold_alpha: u32,
        noise_byte: u32,
        copy_cycle_rgba16: u32,
        expected: bool,
    }

    const fn wire_mode(mode: AlphaCompareMode) -> u32 {
        match mode {
            AlphaCompareMode::None => 0,
            AlphaCompareMode::Threshold => 1,
            AlphaCompareMode::Reserved => 2,
            AlphaCompareMode::Dither => 3,
        }
    }

    /// Frozen fixture partition, port card §4, literal values verified
    /// against `alpha_compare_value`/`copy_alpha_compare_value`'s own
    /// arithmetic above (`AlphaCompareMode::Threshold => alpha >=
    /// threshold_alpha`, `AlphaCompareMode::Dither => alpha*256 >
    /// noise_byte*255`, and `copy_alpha_compare_value`'s RGBA16
    /// hard-alpha-bit fallthrough), not placeholders.
    fn frozen_fixtures() -> Vec<AlphaCompareFixture> {
        vec![
            // Threshold mode, four boundary cases.
            AlphaCompareFixture {
                name: "threshold_equal_passes_not_strict",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 128,
                threshold_alpha: 128,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: true,
            },
            AlphaCompareFixture {
                name: "threshold_just_below_fails",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 127,
                threshold_alpha: 128,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: false,
            },
            AlphaCompareFixture {
                name: "threshold_zero_equal_passes",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 0,
                threshold_alpha: 0,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: true,
            },
            AlphaCompareFixture {
                name: "threshold_max_equal_passes",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 255,
                threshold_alpha: 255,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: true,
            },
            // Dither mode, the exact cross-multiply tie boundary.
            AlphaCompareFixture {
                name: "dither_tie_boundary_passes",
                mode: wire_mode(AlphaCompareMode::Dither),
                alpha: 128,
                threshold_alpha: 0,
                noise_byte: 128,
                copy_cycle_rgba16: 0,
                expected: true, // 128*256=32768 > 128*255=32640
            },
            AlphaCompareFixture {
                name: "dither_just_below_tie_fails",
                mode: wire_mode(AlphaCompareMode::Dither),
                alpha: 127,
                threshold_alpha: 0,
                noise_byte: 128,
                copy_cycle_rgba16: 0,
                expected: false, // 127*256=32512 < 128*255=32640
            },
            AlphaCompareFixture {
                name: "dither_alpha_zero_always_rejects",
                mode: wire_mode(AlphaCompareMode::Dither),
                alpha: 0,
                threshold_alpha: 0,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: false, // 0*256=0, not > 0*255=0
            },
            AlphaCompareFixture {
                name: "dither_alpha_max_always_passes",
                mode: wire_mode(AlphaCompareMode::Dither),
                alpha: 255,
                threshold_alpha: 0,
                noise_byte: 255,
                copy_cycle_rgba16: 0,
                expected: true, // 255*256=65280 > 255*255=65025
            },
            // None mode: unconditional pass regardless of the other inputs.
            AlphaCompareFixture {
                name: "none_mode_always_passes",
                mode: wire_mode(AlphaCompareMode::None),
                alpha: 0,
                threshold_alpha: 255,
                noise_byte: 0,
                copy_cycle_rgba16: 0,
                expected: true,
            },
            // Copy-cycle RGBA16 special case.
            AlphaCompareFixture {
                name: "copy_cycle_rgba16_nonzero_alpha_passes_ignoring_threshold",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 1,
                threshold_alpha: 255,
                noise_byte: 0,
                copy_cycle_rgba16: 1,
                expected: true,
            },
            AlphaCompareFixture {
                name: "copy_cycle_rgba16_zero_alpha_rejects",
                mode: wire_mode(AlphaCompareMode::Threshold),
                alpha: 0,
                threshold_alpha: 0,
                noise_byte: 0,
                copy_cycle_rgba16: 1,
                expected: false,
            },
        ]
    }

    /// CPU-side half of the differential (port card §4/§7): every fixture
    /// case computed via the Rust oracle
    /// (`alpha_compare_value`/`copy_alpha_compare_value`) and asserted equal
    /// to the hand-derived `expected` boolean frozen in `frozen_fixtures`.
    /// No GPU involved -- matches this crate's existing CPU-only test
    /// convention and runs under the ordinary `cargo test -p
    /// fn64-render-wgpu` loop.
    #[test]
    fn frozen_fixtures_match_rust_oracle() {
        for fixture in frozen_fixtures() {
            let mode = match fixture.mode {
                0 => AlphaCompareMode::None,
                1 => AlphaCompareMode::Threshold,
                3 => AlphaCompareMode::Dither,
                other => panic!("fixture {}: unexpected wire mode {other}", fixture.name),
            };
            let alpha = fixture.alpha as u8;
            let threshold_alpha = fixture.threshold_alpha as u8;
            let noise_byte = noise(fixture.noise_byte as u8);
            let actual = if fixture.copy_cycle_rgba16 != 0 {
                copy_alpha_compare_value(
                    mode,
                    CopyCycleSourceFormat::RGBA16,
                    alpha,
                    threshold_alpha,
                    noise_byte,
                )
            } else {
                alpha_compare_value(mode, alpha, threshold_alpha, noise_byte)
            };
            assert_eq!(
                actual, fixture.expected,
                "fixture {} diverged from the frozen expected boolean",
                fixture.name
            );
        }
    }

    #[cfg(feature = "host-gpu-tests")]
    mod host_gpu_tests {
        use super::*;
        use std::future::Future;
        use std::pin::pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        fn block_on<F: Future>(future: F) -> F::Output {
            struct ThreadWake(std::thread::Thread);
            impl Wake for ThreadWake {
                fn wake(self: Arc<Self>) {
                    self.0.unpark();
                }
            }
            let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
            let mut context = Context::from_waker(&waker);
            let mut future = pin!(future);
            loop {
                match Future::poll(future.as_mut(), &mut context) {
                    Poll::Ready(output) => return output,
                    Poll::Pending => std::thread::park(),
                }
            }
        }

        /// Minimal compute-shim harness (port card §4/§7): wraps
        /// `alpha_compare_fragment_fn` (the new fragment-callable function
        /// under test, unmodified) in a throwaway `@compute` entry point
        /// that reads one `AlphaCompareCase` per invocation from a storage
        /// buffer and writes its `bool` result (as `u32`) to a second
        /// storage buffer -- new test-only scaffolding, not a claim that the
        /// function runs inside any real fragment shader (see the port
        /// card's §6 nonclaims).
        const SHIM_WGSL_HEADER: &str = "\
struct AlphaCompareCase {
    mode: u32,
    alpha: u32,
    threshold_alpha: u32,
    noise_byte: u32,
    copy_cycle_rgba16: u32,
}

@group(0) @binding(0)
var<storage, read> cases: array<AlphaCompareCase>;

@group(0) @binding(1)
var<storage, read_write> results: array<u32>;

@compute @workgroup_size(1)
fn alpha_compare_fragment_fn_shim(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&cases)) {
        return;
    }
    let one_case = cases[index];
    let passed = alpha_compare_fragment_fn(
        one_case.mode,
        one_case.alpha,
        one_case.threshold_alpha,
        one_case.noise_byte,
        one_case.copy_cycle_rgba16,
    );
    if (passed) {
        results[index] = 1u;
    } else {
        results[index] = 0u;
    }
}
";

        fn shim_source() -> String {
            format!("{ALPHA_COMPARE_FRAGMENT_FN_WGSL}\n{SHIM_WGSL_HEADER}")
        }

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct RawCase {
            mode: u32,
            alpha: u32,
            threshold_alpha: u32,
            noise_byte: u32,
            copy_cycle_rgba16: u32,
        }

        /// Required host GPU evidence (port card §7's Host-GPU loop):
        /// dispatches the compute shim over every frozen fixture case on a
        /// real native adapter and asserts the WGSL side agrees with both
        /// the Rust oracle and the hand-derived `expected` boolean -- an
        /// independent, non-self-referential three-way check. Panics with
        /// the typed no-adapter reason if this host has no native GPU
        /// adapter, matching `targets/triangle_pipeline/tests.rs`'s
        /// required-host-GPU convention rather than silently skipping.
        #[test]
        fn required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures() {
            let fixtures = frozen_fixtures();
            let cases: Vec<RawCase> = fixtures
                .iter()
                .map(|fixture| RawCase {
                    mode: fixture.mode,
                    alpha: fixture.alpha,
                    threshold_alpha: fixture.threshold_alpha,
                    noise_byte: fixture.noise_byte,
                    copy_cycle_rgba16: fixture.copy_cycle_rgba16,
                })
                .collect();

            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: crate::device::adapter_selection::backends_for_request(
                    wgpu::Backends::METAL | wgpu::Backends::VULKAN | wgpu::Backends::DX12,
                ),
                flags: wgpu::InstanceFlags::VALIDATION,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })) {
                Ok(adapter) => adapter,
                Err(wgpu::RequestAdapterError::NotFound { .. }) => {
                    panic!("required host GPU evidence unavailable: typed no-adapter for AnyNative")
                }
                Err(error) => panic!("adapter request failed: {error}"),
            };
            crate::device::adapter_selection::assert_expected_adapter(&adapter);
            eprintln!(
                "fn64-alpha-compare-fragment-fn: adapter={:?}",
                adapter.get_info().name
            );
            let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-shim"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            }))
            .unwrap();

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-shim"),
                source: wgpu::ShaderSource::Wgsl(shim_source().into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-shim"),
                layout: None,
                module: &shader,
                entry_point: Some("alpha_compare_fragment_fn_shim"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

            let case_bytes = (cases.len() * std::mem::size_of::<RawCase>()) as u64;
            let result_bytes = (cases.len() * std::mem::size_of::<u32>()) as u64;
            let case_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-cases"),
                size: case_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let result_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-results"),
                size: result_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-readback"),
                size: result_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            let case_data: Vec<u8> = cases
                .iter()
                .flat_map(|case| {
                    [
                        case.mode,
                        case.alpha,
                        case.threshold_alpha,
                        case.noise_byte,
                        case.copy_cycle_rgba16,
                    ]
                })
                .flat_map(u32::to_le_bytes)
                .collect();
            queue.write_buffer(&case_buffer, 0, &case_data);

            let layout = pipeline.get_bind_group_layout(0);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fn64-alpha-compare-fragment-fn-bind-group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: case_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: result_buffer.as_entire_binding(),
                    },
                ],
            });

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("fn64-alpha-compare-fragment-fn-pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(cases.len() as u32, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&result_buffer, 0, &readback_buffer, 0, result_bytes);
            queue.submit(Some(encoder.finish()));

            let slice = readback_buffer.slice(..);
            let (sender, receiver) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            loop {
                let _ = device.poll(wgpu::PollType::Poll);
                if let Ok(result) = receiver.try_recv() {
                    result.unwrap();
                    break;
                }
            }
            let observed: Vec<u32> = {
                let mapped = slice.get_mapped_range().unwrap();
                mapped
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
                    .collect()
            };
            readback_buffer.unmap();

            assert_eq!(observed.len(), fixtures.len());
            for (fixture, &observed_u32) in fixtures.iter().zip(observed.iter()) {
                let observed_bool = observed_u32 != 0;
                assert_eq!(
                    observed_bool, fixture.expected,
                    "fixture {}: WGSL result diverged from the frozen expected boolean",
                    fixture.name
                );
                // Independently re-derive via the Rust oracle too, so a
                // three-way (WGSL, Rust oracle, hand-derived) agreement is
                // checked in the same assertion pass, not just WGSL-vs-
                // hand-derived.
                let mode = match fixture.mode {
                    0 => AlphaCompareMode::None,
                    1 => AlphaCompareMode::Threshold,
                    3 => AlphaCompareMode::Dither,
                    other => panic!("fixture {}: unexpected wire mode {other}", fixture.name),
                };
                let alpha = fixture.alpha as u8;
                let threshold_alpha = fixture.threshold_alpha as u8;
                let noise_byte = noise(fixture.noise_byte as u8);
                let cpu_result = if fixture.copy_cycle_rgba16 != 0 {
                    copy_alpha_compare_value(
                        mode,
                        CopyCycleSourceFormat::RGBA16,
                        alpha,
                        threshold_alpha,
                        noise_byte,
                    )
                } else {
                    alpha_compare_value(mode, alpha, threshold_alpha, noise_byte)
                };
                assert_eq!(
                    observed_bool, cpu_result,
                    "fixture {}: WGSL result diverged from Rust oracle",
                    fixture.name
                );
            }
        }
    }
}
