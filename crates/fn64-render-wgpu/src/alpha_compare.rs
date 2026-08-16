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
//! crate-owned state), RDP noise-generator architecture (`AlphaCompareNoise`
//! below is a narrow typed byte carrier, not a PRNG), draw-path integration,
//! and any native GPU execution.

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
/// tiles, shared by the alpha-dither `Pattern`/`InversePattern`
/// substitution rule below. Literal port of `ordered_rgb_dither_threshold`
/// (`blend.rs:28-38`).
///
/// # Panics
/// If `mode` is `Noise` or `Disabled` -- both lack an ordered tile and must
/// be resolved to `MagicSquare`/`Bayer` by the caller (see
/// [`apply_alpha_dither`]'s substitution rule) before reaching this
/// function.
const fn ordered_dither_threshold(mode: RgbDitherMode, x: i32, y: i32) -> u8 {
    const MAGIC_SQUARE: [[u8; 4]; 4] = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
    const BAYER: [[u8; 4]; 4] = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
    let row = y.rem_euclid(4) as usize;
    let column = x.rem_euclid(4) as usize;
    match mode {
        RgbDitherMode::MagicSquare => MAGIC_SQUARE[row][column],
        RgbDitherMode::Bayer => BAYER[row][column],
        RgbDitherMode::Noise | RgbDitherMode::Disabled => {
            panic!("ordered dither threshold requested for a non-ordered RgbDitherMode")
        }
    }
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
}
