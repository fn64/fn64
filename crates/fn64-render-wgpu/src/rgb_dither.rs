//! RGB dither and `Float4ToRGBA16` quantization semantics: a literal port of
//! the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shaders/Formats.hlsli` (`DitherPatternBayer`, `DitherPatternMagicSquare`,
//! `DitherPatternIndex`, `DitherPatternValue`, `Float4ToRGBA16`).
//!
//! [`quantize_post_float_rgba16_non_hdr`] ports only `Float4ToRGBA16`'s
//! post-float, non-HDR integer tail, not the complete function -- see its
//! own doc for the exact frontier and why the float-facing parameter list
//! (`float4 i`, `bool usesHDR`) is deliberately not reproduced here.
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference` (see
//! `depth_strict_less.rs`, `alpha_compare.rs`), so this is a self-contained
//! literal re-expression citing RT64's source, not a re-derivation of the
//! reference's own `apply_rgb_dither`/`ordered_rgb_dither_threshold`
//! (`crates/fn64-render-reference/src/raster/blend.rs:24-69`).
//!
//! ## Matrix cross-check against the existing reference oracle (frontier)
//!
//! This module independently transcribes RT64's flat 16-element
//! `DitherPatternBayer`/`DitherPatternMagicSquare` tables and RT64's own
//! `((coord.y & 3) << 2) + (coord.x & 3)` (row-major, `row*4+col`) index
//! rule. Comparing cell-by-cell against `fn64-render-reference`'s existing
//! `[[u8;4];4]` `MAGIC_SQUARE`/`BAYER` tables
//! (`crates/fn64-render-reference/src/raster/blend.rs:29-30`, duplicated at
//! `crates/fn64-render-wgpu/src/alpha_compare.rs:161-162`) at every `(x, y)`
//! in `0..4`:
//!
//! - **MagicSquare is byte-identical** between RT64 and the reference oracle
//!   at every cell.
//! - **Bayer disagrees at rows 1 and 2** (RT64 row 1 is `[4, 0, 5, 1]`, the
//!   reference's row 1 is `[6, 2, 7, 3]`; RT64 row 2 is `[3, 7, 2, 6]`, the
//!   reference's row 2 is `[1, 5, 0, 4]`). Rows 0 and 3 agree. Both tables
//!   remain valid Bayer-shaped tiles -- each covers every threshold `0..=7`
//!   exactly twice -- so this is a *phase/arrangement* difference, not a
//!   malformed table on either side.
//!
//! This module ports RT64's literal tables (the task's supplied authority)
//! rather than silently reconciling the two, and pins the disagreement with
//! an exhaustive comparison test
//! ([`tests::bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`])
//! so a future change to either table's arrangement fails loudly here
//! instead of silently drifting further apart. Resolving *which* table (if
//! either) matches real hardware is out of this slice's scope -- both
//! `blend.rs`'s own comment and this module cite no silicon measurement for
//! the exact Bayer phase, only that the tile must contain each threshold
//! twice. libultra publishes the `G_CD_BAYER` selector bit and no table
//! (`gbi.h:661-671`), and RT64 is one of the two disputants rather than an
//! adjudicator, so nothing available here settles it.
//!
//! ## These tables are this crate's only ordered tiles
//!
//! `alpha_compare.rs` used to carry a second `MAGIC_SQUARE`/`BAYER` pair,
//! ported from the reference lane, for the `G_AD_PATTERN` alpha-dither
//! substitution rule. `MagicSquare` agreed at every cell; `Bayer` disagreed
//! at rows 1 and 2 -- so this one crate answered "what is the Bayer tile"
//! two different ways, and at most one of them could have been right.
//!
//! libultra defines the alpha-dither `PATTERN` threshold as *the currently
//! selected RGB dither matrix* (`gbi.h:674-678`), so the alpha path is a
//! consumer of these tables by definition. `alpha_compare.rs` now reads
//! [`ordered_tile_value`] instead of transcribing its own, and
//! [`tests::the_alpha_dither_path_reads_this_modules_tables`] pins the
//! agreement at every cell of both tiles.
//!
//! That resolves the self-inconsistency and **only** the
//! self-inconsistency. The frontier above is unchanged: if the Bayer phase
//! is ever settled against RT64's arrangement, one table changes here and
//! both paths follow it.

/// One eight-bit pseudo-random sample. RT64's shaders derive this from a
/// per-fragment `initRand(...)` PRNG seeded by screen position
/// (`FbWriteColorCS.hlsl:19`, `FbReinterpretCS.hlsl:21`); this module does
/// not generate noise, matching `alpha_compare.rs`'s `AlphaCompareNoise`
/// convention -- the caller supplies a deterministic byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DitherNoiseByte(pub u8);

impl DitherNoiseByte {
    /// Low three bits only, already narrowed to [`DitherThreshold`]'s
    /// invariant. `DitherPatternValue`'s `NOISE` case (`Formats.hlsli:34`)
    /// and the post-float `dither` parameter below both consume only these
    /// bits when the noise selector is active.
    pub const fn low_three_bits(self) -> DitherThreshold {
        DitherThreshold(self.0 & 7)
    }
}

/// An RGB/RGBA16 dither threshold or added quantum, invariant-carrying: RT64
/// never produces or consumes a value outside `0..=7` on this seam --
/// [`dither_pattern_value`]'s four branches (two 4x4 ordered-tile lookups
/// whose every cell is `0..=7`, `randomSeed & 7`, and the constant `0`) are
/// exhaustive and each one already fits. The private field means a caller
/// cannot construct an out-of-range threshold and feed it to
/// [`quantize_post_float_rgba16_non_hdr`]; the only public routes are
/// [`DitherThreshold::try_new`] (loud rejection, for values not already
/// proven in range by this module's own functions) and this module's own
/// constructors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DitherThreshold(u8);

/// Why a caller-supplied byte cannot be a [`DitherThreshold`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DitherThresholdError {
    pub value: u8,
}

impl core::fmt::Display for DitherThresholdError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "dither threshold {} is outside the RDP's 0..=7 domain",
            self.value
        )
    }
}

impl std::error::Error for DitherThresholdError {}

impl DitherThreshold {
    /// Loud, checked constructor for a threshold not already proven in
    /// range by this module's own [`dither_pattern_value`]. Rejects `>= 8`
    /// rather than masking or saturating -- AGENTS.md's "loud traps, no
    /// silent shrugs".
    pub const fn try_new(value: u8) -> Result<Self, DitherThresholdError> {
        if value <= 7 {
            Ok(Self(value))
        } else {
            Err(DitherThresholdError { value })
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// The RDP's coverage-accumulator value taken modulo 8, [`Float4ToRGBA16`]'s
/// `cvgModulo & 0x4` alpha-bit input (`Formats.hlsli:100-101`) narrowed to
/// its own `0..=7` invariant. Never publicly constructible with an
/// out-of-range value; the private field is the enforcement mechanism, not
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverageModulo8(u8);

/// Why a caller-supplied byte cannot be a [`CoverageModulo8`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoverageModulo8Error {
    pub value: u8,
}

impl core::fmt::Display for CoverageModulo8Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "coverage modulo-8 value {} is outside the 0..=7 domain a `% 8` result must produce",
            self.value
        )
    }
}

impl std::error::Error for CoverageModulo8Error {}

impl CoverageModulo8 {
    /// Loud, checked constructor. Rejects `>= 8` -- a value that could not
    /// have come from RT64's own `... % 8` (`Formats.hlsli:100`) -- rather
    /// than silently truncating it with a bitmask.
    pub const fn try_new(value: u8) -> Result<Self, CoverageModulo8Error> {
        if value <= 7 {
            Ok(Self(value))
        } else {
            Err(CoverageModulo8Error { value })
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// RGB dither pattern selector, `Formats.hlsli`'s `DitherPatternValue` switch
/// (lines 27-39). Reuses `crate::state::RgbDither`'s existing four-variant
/// decode (`MagicSquare`, `Bayer`, `Noise`, `Disabled`) rather than defining
/// a duplicate enum -- the wire encodings and variant order are
/// byte-identical (`state.rs`'s `rgb_dither()`: `0=MAGICSQ, 1=BAYER,
/// 2=NOISE, 3=DISABLE`, matching RT64's own switch cases exactly).
pub use crate::state::RgbDither;

/// RT64's `DitherPatternBayer` (`Formats.hlsli:9-14`), transcribed literally
/// as a flat 16-element row-major table (index `row*4+col`, matching RT64's
/// own storage order).
const DITHER_PATTERN_BAYER: [u8; 16] = [
    0, 4, 1, 5, //
    4, 0, 5, 1, //
    3, 7, 2, 6, //
    7, 3, 6, 2,
];

/// RT64's `DitherPatternMagicSquare` (`Formats.hlsli:16-21`), transcribed
/// literally as a flat 16-element row-major table.
const DITHER_PATTERN_MAGIC_SQUARE: [u8; 16] = [
    0, 6, 1, 7, //
    4, 2, 5, 3, //
    3, 5, 2, 4, //
    7, 1, 6, 0,
];

/// Literal port of `DitherPatternIndex` (`Formats.hlsli:23-25`):
/// `((coord.y & 3) << 2) + (coord.x & 3)`. RT64's `coord` is an already
/// nonnegative `uint2` screen coordinate; this module accepts signed `i32`
/// coordinates and applies Euclidean wrapping first (see
/// [`dither_pattern_index`]'s doc) rather than reinterpreting a negative
/// value's bit pattern as RT64's HLSL `uint` cast would.
const fn dither_pattern_index_from_wrapped(wrapped_x: u32, wrapped_y: u32) -> usize {
    (((wrapped_y & 3) << 2) + (wrapped_x & 3)) as usize
}

/// Screen-coordinate wrapping into `dither_pattern_index_from_wrapped`'s
/// domain. RT64's HLSL `coord` parameter is a `uint2`; a caller passing a
/// negative screen coordinate to an HLSL `uint2` parameter gets
/// implementation-defined bit-pattern reinterpretation, not the mathematical
/// `mod`. This module instead uses Euclidean wrapping
/// (`i32::rem_euclid`), matching this crate's own
/// `alpha_compare.rs`/`ordered_dither_threshold` and the admitted
/// `fn64-render-reference::ordered_rgb_dither_threshold` precedent
/// (`blend.rs:31-32`, `x.rem_euclid(4)`/`y.rem_euclid(4)`) rather than
/// RT64's own HLSL cast behavior -- both give identical results for
/// nonnegative coordinates (the only kind RT64's shader ever receives), and
/// Euclidean wrapping is the well-defined generalization this crate already
/// committed to for negative screen space.
pub const fn dither_pattern_index(x: i32, y: i32) -> usize {
    let wrapped_x = x.rem_euclid(4) as u32;
    let wrapped_y = y.rem_euclid(4) as u32;
    dither_pattern_index_from_wrapped(wrapped_x, wrapped_y)
}

/// The ordered-tile threshold for one screen pixel, for the two selectors
/// that *have* an ordered tile.
///
/// This is [`dither_pattern_value`]'s `MagicSquare`/`Bayer` arms with no
/// noise byte to thread through, and it exists so that
/// [`crate::alpha_compare`]'s `G_AD_PATTERN` substitution rule can read
/// **these** tables rather than a second copy of them. libultra defines
/// that rule's threshold as the currently selected RGB dither matrix
/// (`gbi.h:674-678`), so the alpha-dither path is a consumer of this
/// module's tables by definition, not a peer transcription of them.
///
/// Before this existed, `alpha_compare.rs` carried its own
/// `MAGIC_SQUARE`/`BAYER` pair ported from the reference lane while this
/// module carried RT64's. `MagicSquare` agreed; `Bayer` disagreed at rows 1
/// and 2, so one crate held two Bayer tiles for one hardware quantity.
/// Which tile the silicon uses is still the open frontier this module's
/// header records -- **that question is not answered by this function and
/// is not answered anywhere in this crate**. What this function fixes is
/// only that the two paths can no longer answer it differently.
///
/// # Panics
/// If `pattern` is `Noise` or `Disabled`. Both are real, handled selectors
/// -- [`dither_pattern_value`] answers them -- but neither has an ordered
/// tile, and the alpha-dither substitution rule resolves them to
/// `MagicSquare`/`Bayer` before reaching here.
pub const fn ordered_tile_value(pattern: RgbDither, x: i32, y: i32) -> u8 {
    match pattern {
        RgbDither::MagicSquare => DITHER_PATTERN_MAGIC_SQUARE[dither_pattern_index(x, y)],
        RgbDither::Bayer => DITHER_PATTERN_BAYER[dither_pattern_index(x, y)],
        RgbDither::Noise | RgbDither::Disabled => {
            panic!("ordered tile value requested for a selector with no ordered tile")
        }
    }
}

/// Literal port of `DitherPatternValue` (`Formats.hlsli:27-39`): selects a
/// 0..=7 dither threshold for one screen pixel under the four RGB dither
/// selector modes decoded by `crate::state::RgbDither`.
///
/// - `MagicSquare` / `Bayer`: exact 4x4 ordered-tile lookup via
///   [`dither_pattern_index`].
/// - `Noise`: `randomSeed & 7` (`Formats.hlsli:34`) -- [`DitherNoiseByte::low_three_bits`].
/// - `Disabled`: `0` (`Formats.hlsli:36-37`'s `default` case; RT64's switch
///   uses `case 3` for its own `DISABLE` and falls through the same `default`
///   arm, both returning `0`).
pub const fn dither_pattern_value(
    pattern: RgbDither,
    x: i32,
    y: i32,
    noise: DitherNoiseByte,
) -> DitherThreshold {
    match pattern {
        RgbDither::MagicSquare => {
            DitherThreshold(DITHER_PATTERN_MAGIC_SQUARE[dither_pattern_index(x, y)])
        }
        RgbDither::Bayer => DitherThreshold(DITHER_PATTERN_BAYER[dither_pattern_index(x, y)]),
        RgbDither::Noise => noise.low_three_bits(),
        RgbDither::Disabled => DitherThreshold(0),
    }
}

/// One already-8-bit-quantized RGB working color plus its alpha-derived
/// coverage input to [`quantize_post_float_rgba16_non_hdr`]. RT64's
/// `Float4ToRGBA16` takes a `float4 i` (a 0.0..=1.0-normalized
/// combiner/blend output) and re-derives `r/g/b = round(clamp(channel*255,
/// 0, 255))` (`Formats.hlsli:97-99`) before ever consulting `dither`; that
/// float-to-u8 rounding step is already covered by this crate's own
/// boundary (RGBA8888 working colors are `u8` throughout
/// `fn64-render-wgpu`, matching `alpha_compare.rs`'s and `coverage.rs`'s
/// existing `u8`-channel convention), so this module's integer seam starts
/// one step later, from an already-rounded `u8` triple -- see the
/// module-level frontier note on [`quantize_post_float_rgba16_non_hdr`] for
/// exactly where RT64's float path and this integer seam diverge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba16QuantizeInput {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// The pre-quantized alpha channel's *coverage* interpretation, RT64's
    /// `i.a` reinterpreted as an already-derived coverage-accumulator value
    /// taken modulo 8 for the non-HDR case (see
    /// [`quantize_post_float_rgba16_non_hdr`]'s frontier note). `Coverage`
    /// (this crate's `coverage.rs`) already models the RDP's 0..=8 subpixel
    /// population count; this field is that count's low three bits,
    /// matching `Formats.hlsli:100`'s `% 8` -- and, being a
    /// [`CoverageModulo8`], cannot itself be `>= 8`.
    pub coverage_modulo_8: CoverageModulo8,
}

/// Packed RGBA16 output: RT64's `(r << 11) | (g << 6) | (b << 1) | a`
/// (`Formats.hlsli:105`), five bits per color channel and one alpha bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba16Packed(pub u16);

impl Rgba16Packed {
    pub const fn bits(self) -> u16 {
        self.0
    }
}

/// Literal integer port of the **post-float** tail of RT64's
/// `Float4ToRGBA16` (`Formats.hlsli:95-106`) for the non-HDR
/// (`usesHDR == false`) case: `cvgRange = 255.0f`. This function is
/// deliberately *not* named `float4_to_rgba16` -- it does not take RT64's
/// `float4 i` parameter and does not perform RT64's float rounding/clamp
/// steps at all (see the frontier note below); it is the exact integer-only
/// remainder that begins once `r`/`g`/`b` are already `u8` and the
/// coverage-alpha modulo is already computed, not a complete
/// re-implementation of `Float4ToRGBA16`'s signature.
///
/// ## Frontier: where this integer seam stops
///
/// RT64's real signature is `Float4ToRGBA16(float4 i, uint dither, bool
/// usesHDR)`. Three float-rounding steps precede the part this function
/// covers:
///
/// 1. `r/g/b = round(clamp(i.r * 255.0f, 0.0f, 255.0f))` -- already-`u8`
///    RGBA8888 working colors (this crate's existing convention) make this
///    step's `round`/`clamp` a no-op identity for any value that started as
///    a `u8` channel; this module's `r`/`g`/`b` inputs assume that identity
///    holds and does not re-derive it from a float.
/// 2. `cvgModulo = round(i.a * cvgRange) % 8` where `cvgRange` is
///    `usesHDR ? 65535.0f : 255.0f` -- **the HDR branch is out of scope
///    here.** `usesHDR` selects an HDR render-target's wider coverage
///    range, a target-format concern this module does not own (this crate
///    has no HDR target format yet: `crate::state::ColorImage`/`PixelSize`
///    do not encode one). Faithfully porting the `65535.0f` branch would
///    require inventing HDR coverage semantics this slice has no authority
///    for; this function accepts only the already-computed, range-checked
///    [`CoverageModulo8`] (RT64's `cvgModulo` for the `usesHDR == false`
///    case, where `round(i.a * 255.0) % 8` is exactly an 8-bit alpha
///    channel's low three bits when `i.a` is itself an exact `u8/255.0`
///    value) rather than reproducing the `round(_, cvgRange) % 8` float
///    arithmetic. A future HDR-target slice must port the `usesHDR == true`
///    branch separately; this module does not stub, approximate, or
///    silently normalize it.
/// 3. `int cvgModulo = ... % 8` uses a signed-`int` HLSL modulo of a
///    `round()` result that is always nonnegative for finite, in-range
///    input (`i.a` is combiner/blend output, itself derived from `u8`
///    channels elsewhere in this pipeline, so `round(i.a * 255.0)` is
///    always a nonnegative integer here in practice); this module's
///    checked `CoverageModulo8` input sidesteps the signed/unsigned
///    distinction entirely rather than re-deriving it, and its private
///    field statically rules out the values a signed `int % 8` could never
///    have produced.
///
/// From `cvgModulo` onward the arithmetic is exact, bounded integer
/// arithmetic with no float rounding, and is what this function ports
/// literally:
///
/// - `a = (cvgModulo & 0x4) ? 1 : 0` (`Formats.hlsli:101`) -- the coverage
///   modulo's bit 2 becomes the packed alpha bit.
/// - `r = min(r + dither, 255) >> 3`, likewise for `g`/`b`
///   (`Formats.hlsli:102-104`) -- add the 0..=7 dither threshold, saturate
///   at 255 (not wrap), then truncate (not round) to 5 bits by `>> 3`.
/// - `(r << 11) | (g << 6) | (b << 1) | a` (`Formats.hlsli:105`).
pub const fn quantize_post_float_rgba16_non_hdr(
    input: Rgba16QuantizeInput,
    dither: DitherThreshold,
) -> Rgba16Packed {
    let a: u16 = if input.coverage_modulo_8.value() & 0x4 != 0 {
        1
    } else {
        0
    };
    let r = quantize_channel(input.r, dither);
    let g = quantize_channel(input.g, dither);
    let b = quantize_channel(input.b, dither);
    Rgba16Packed(((r as u16) << 11) | ((g as u16) << 6) | ((b as u16) << 1) | a)
}

/// `min(channel as u32 + dither as u32, 255) >> 3`, RT64's per-channel
/// `min(r + dither, 255) >> 3` (`Formats.hlsli:102-104`) for one channel.
/// `dither` is a checked [`DitherThreshold`] (`0..=7`), so the widened
/// `u32` addition cannot overflow before the `min` saturates it back into
/// `u8` range; the final `>> 3` always produces a value `0..=31` (5 bits).
const fn quantize_channel(channel: u8, dither: DitherThreshold) -> u8 {
    let sum = channel as u32 + dither.value() as u32;
    let saturated = if sum > 255 { 255 } else { sum };
    (saturated >> 3) as u8
}

pub const RGB_DITHER_WGSL: &str = include_str!("rgb_dither.wgsl");
pub const RGB_DITHER_ENTRY_POINT: &str = "rgb_dither_quantize";

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(byte: u8) -> DitherNoiseByte {
        DitherNoiseByte(byte)
    }

    // --- Matrix pinning: independently transcribed from Formats.hlsli ---

    #[test]
    fn bayer_matrix_matches_pinned_source_flat_layout() {
        // Re-derive the flat table's row-major layout by hand at each cell,
        // independently of the module's own indexing helper, so a
        // transcription error in `dither_pattern_index_from_wrapped` cannot
        // also hide a transcription error in the table itself.
        let expected_rows: [[u8; 4]; 4] = [[0, 4, 1, 5], [4, 0, 5, 1], [3, 7, 2, 6], [7, 3, 6, 2]];
        for (row, expected_row) in expected_rows.iter().enumerate() {
            for (col, expected) in expected_row.iter().enumerate() {
                assert_eq!(DITHER_PATTERN_BAYER[row * 4 + col], *expected);
            }
        }
    }

    #[test]
    fn magic_square_matrix_matches_pinned_source_flat_layout() {
        let expected_rows: [[u8; 4]; 4] = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
        for (row, expected_row) in expected_rows.iter().enumerate() {
            for (col, expected) in expected_row.iter().enumerate() {
                assert_eq!(DITHER_PATTERN_MAGIC_SQUARE[row * 4 + col], *expected);
            }
        }
    }

    #[test]
    fn magic_square_matches_reference_oracle_at_every_cell() {
        // fn64-render-reference's own MAGIC_SQUARE (blend.rs:29), duplicated
        // here as a literal for cross-crate-free comparison, per this
        // module's own convention of not depending on fn64-render-reference.
        const REFERENCE_MAGIC_SQUARE: [[u8; 4]; 4] =
            [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
        for y in 0..4i32 {
            for x in 0..4i32 {
                let rt64_value =
                    dither_pattern_value(RgbDither::MagicSquare, x, y, noise(0)).value();
                let reference_value = REFERENCE_MAGIC_SQUARE[y as usize][x as usize];
                assert_eq!(
                    rt64_value, reference_value,
                    "x={x} y={y}: RT64 and reference oracle must agree for MagicSquare"
                );
            }
        }
    }

    /// **The alpha-dither path and the RGB-dither path read the same
    /// ordered tiles, at every cell of both.**
    ///
    /// libultra defines `G_AD_PATTERN`'s threshold as the currently
    /// selected RGB dither matrix (`gbi.h:674-678`), so this is a
    /// definitional requirement, not a convention: the two paths cannot
    /// legally hold different tiles no matter which tile is correct.
    ///
    /// This crate held different ones until this test existed.
    /// `alpha_compare.rs` transcribed the reference lane's tables and this
    /// module transcribed RT64's; `MagicSquare` matched, `Bayer` did not, at
    /// eight of sixteen cells. The `Bayer` half of this test fails against
    /// that older code, which is what makes it a pin rather than a
    /// restatement.
    ///
    /// Reached through [`crate::alpha_compare::alpha_dither_pattern_threshold_for_tests`],
    /// the alpha path's own consumer of the tile, so a future reintroduced
    /// duplicate is caught here and not merely a re-exported constant that
    /// nothing dithers with. Both selectors and all sixteen cells, not a
    /// sample.
    #[test]
    fn the_alpha_dither_path_reads_this_modules_tables() {
        for pattern in [RgbDither::MagicSquare, RgbDither::Bayer] {
            for y in 0..4i32 {
                for x in 0..4i32 {
                    assert_eq!(
                        crate::alpha_compare::alpha_dither_pattern_threshold_for_tests(
                            pattern, x, y
                        ),
                        ordered_tile_value(pattern, x, y),
                        "{pattern:?} at x={x} y={y}: the alpha-dither substitution rule must \
                         read this module's tile, since gbi.h defines it AS the selected RGB \
                         dither matrix"
                    );
                }
            }
        }
    }

    #[test]
    fn bayer_matrix_disagrees_with_reference_oracle_at_documented_cells() {
        // fn64-render-reference's own BAYER (blend.rs:30), duplicated here
        // as a literal for cross-crate-free comparison.
        const REFERENCE_BAYER: [[u8; 4]; 4] =
            [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
        let mut disagreements = Vec::new();
        for y in 0..4i32 {
            for x in 0..4i32 {
                let rt64_value = dither_pattern_value(RgbDither::Bayer, x, y, noise(0)).value();
                let reference_value = REFERENCE_BAYER[y as usize][x as usize];
                if rt64_value != reference_value {
                    disagreements.push((x, y, rt64_value, reference_value));
                }
            }
        }
        // Rows 0 and 3 agree (8 cells); rows 1 and 2 disagree (8 cells) --
        // see the module-level frontier note. This is a loud pin, not a
        // silent tolerance: if either table's arrangement changes, this
        // test's exact expected set must be re-justified, not just updated.
        let expected: Vec<(i32, i32, u8, u8)> = vec![
            (0, 1, 4, 6),
            (1, 1, 0, 2),
            (2, 1, 5, 7),
            (3, 1, 1, 3),
            (0, 2, 3, 1),
            (1, 2, 7, 5),
            (2, 2, 2, 0),
            (3, 2, 6, 4),
        ];
        assert_eq!(disagreements, expected);
    }

    #[test]
    fn both_matrices_are_permutations_of_zero_through_seven_twice() {
        for pattern in [RgbDither::MagicSquare, RgbDither::Bayer] {
            let mut seen = [0u8; 8];
            for y in 0..4 {
                for x in 0..4 {
                    let value = dither_pattern_value(pattern, x, y, noise(0)).value();
                    assert!(value <= 7);
                    seen[value as usize] += 1;
                }
            }
            assert_eq!(seen, [2; 8], "pattern={pattern:?}");
        }
    }

    // --- Indexing: coordinate periodicity and negative wrapping ---

    #[test]
    fn index_matches_rt64_formula_for_nonnegative_coordinates() {
        for y in 0..16i32 {
            for x in 0..16i32 {
                let expected = (((y & 3) << 2) + (x & 3)) as usize;
                assert_eq!(dither_pattern_index(x, y), expected, "x={x} y={y}");
            }
        }
    }

    #[test]
    fn index_wraps_every_four_pixels_positive_and_negative() {
        for base_x in -8..8i32 {
            for base_y in -8..8i32 {
                let base = dither_pattern_index(base_x, base_y);
                assert_eq!(dither_pattern_index(base_x + 4, base_y), base);
                assert_eq!(dither_pattern_index(base_x - 4, base_y), base);
                assert_eq!(dither_pattern_index(base_x, base_y + 4), base);
                assert_eq!(dither_pattern_index(base_x, base_y - 4), base);
                assert_eq!(dither_pattern_index(base_x + 4, base_y + 4), base);
            }
        }
    }

    #[test]
    fn index_negative_coordinates_use_euclidean_wrapping_not_truncation() {
        // -1.rem_euclid(4) == 3, matching how a real negative screen
        // coordinate one pixel left of the origin continues the same 4x4
        // tile rather than jumping to an unrelated cell (Rust's `%` would
        // instead give -1, which is not a valid array index at all).
        assert_eq!(dither_pattern_index(-1, 0), dither_pattern_index(3, 0));
        assert_eq!(dither_pattern_index(0, -1), dither_pattern_index(0, 3));
        assert_eq!(dither_pattern_index(-5, -5), dither_pattern_index(3, 3));
        assert_eq!(dither_pattern_index(-401, -401), dither_pattern_index(3, 3));
    }

    #[test]
    fn index_stays_in_bounds_for_extreme_coordinates() {
        for x in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
            for y in [i32::MIN, 0, i32::MAX] {
                let index = dither_pattern_index(x, y);
                assert!(index < 16, "x={x} y={y} index={index}");
            }
        }
    }

    // --- Selector modes ---

    #[test]
    fn magic_square_selector_ignores_noise() {
        for noise_byte in [0u8, 1, 100, 255] {
            assert_eq!(
                dither_pattern_value(RgbDither::MagicSquare, 2, 1, noise(noise_byte)).value(),
                DITHER_PATTERN_MAGIC_SQUARE[dither_pattern_index(2, 1)]
            );
        }
    }

    #[test]
    fn bayer_selector_ignores_noise() {
        for noise_byte in [0u8, 1, 100, 255] {
            assert_eq!(
                dither_pattern_value(RgbDither::Bayer, 2, 1, noise(noise_byte)).value(),
                DITHER_PATTERN_BAYER[dither_pattern_index(2, 1)]
            );
        }
    }

    #[test]
    fn noise_selector_ignores_coordinates() {
        for x in -3..3i32 {
            for y in -3..3i32 {
                assert_eq!(
                    dither_pattern_value(RgbDither::Noise, x, y, noise(0b1010_1101)).value(),
                    0b101
                );
            }
        }
    }

    #[test]
    fn noise_selector_uses_exactly_low_three_bits() {
        for byte in 0u16..=255 {
            let byte = byte as u8;
            assert_eq!(
                dither_pattern_value(RgbDither::Noise, 0, 0, noise(byte)).value(),
                byte & 7
            );
        }
    }

    #[test]
    fn disabled_selector_always_zero() {
        for x in -5..5i32 {
            for y in -5..5i32 {
                for noise_byte in [0u8, 255] {
                    assert_eq!(
                        dither_pattern_value(RgbDither::Disabled, x, y, noise(noise_byte)).value(),
                        0
                    );
                }
            }
        }
    }

    #[test]
    fn mutation_distinguishes_magic_square_from_bayer() {
        // At least one cell in 0..4x0..4 must differ between the two
        // ordered tables, or a selector-mode mixup would be undetectable.
        let mut any_differ = false;
        for y in 0..4i32 {
            for x in 0..4i32 {
                let magic = dither_pattern_value(RgbDither::MagicSquare, x, y, noise(0));
                let bayer = dither_pattern_value(RgbDither::Bayer, x, y, noise(0));
                if magic != bayer {
                    any_differ = true;
                }
            }
        }
        assert!(any_differ);
    }

    #[test]
    fn mutation_distinguishes_disabled_from_noise_when_noise_nonzero() {
        assert_ne!(
            dither_pattern_value(RgbDither::Disabled, 0, 0, noise(5)),
            dither_pattern_value(RgbDither::Noise, 0, 0, noise(5))
        );
    }

    // --- DitherThreshold / CoverageModulo8: checked-constructor boundaries ---

    #[test]
    fn dither_threshold_try_new_accepts_every_value_zero_through_seven() {
        for value in 0u8..=7 {
            assert_eq!(DitherThreshold::try_new(value).unwrap().value(), value);
        }
    }

    #[test]
    fn dither_threshold_try_new_rejects_eight_and_above() {
        for value in [8u8, 9, 100, 254, 255] {
            let error = DitherThreshold::try_new(value).unwrap_err();
            assert_eq!(error.value, value);
        }
    }

    #[test]
    fn dither_threshold_try_new_boundary_seven_accepts_eight_rejects() {
        assert!(DitherThreshold::try_new(7).is_ok());
        assert!(DitherThreshold::try_new(8).is_err());
    }

    #[test]
    fn coverage_modulo_8_try_new_accepts_every_value_zero_through_seven() {
        for value in 0u8..=7 {
            assert_eq!(CoverageModulo8::try_new(value).unwrap().value(), value);
        }
    }

    #[test]
    fn coverage_modulo_8_try_new_rejects_eight_and_above() {
        for value in [8u8, 9, 100, 254, 255] {
            let error = CoverageModulo8::try_new(value).unwrap_err();
            assert_eq!(error.value, value);
        }
    }

    #[test]
    fn coverage_modulo_8_try_new_boundary_seven_accepts_eight_rejects() {
        assert!(CoverageModulo8::try_new(7).is_ok());
        assert!(CoverageModulo8::try_new(8).is_err());
    }

    #[test]
    fn dither_threshold_error_display_names_the_offending_value() {
        let error = DitherThreshold::try_new(255).unwrap_err();
        assert!(error.to_string().contains("255"));
    }

    #[test]
    fn coverage_modulo_8_error_display_names_the_offending_value() {
        let error = CoverageModulo8::try_new(200).unwrap_err();
        assert!(error.to_string().contains("200"));
    }

    // --- quantize_post_float_rgba16_non_hdr: exhaustive channel x threshold ---

    fn threshold(value: u8) -> DitherThreshold {
        DitherThreshold::try_new(value).expect("test threshold must be 0..=7")
    }

    fn modulo8(value: u8) -> CoverageModulo8 {
        CoverageModulo8::try_new(value).expect("test coverage modulo must be 0..=7")
    }

    #[test]
    fn quantize_channel_exhaustive_every_byte_x_every_threshold() {
        for channel in 0u16..=255 {
            let channel = channel as u8;
            for dither in 0u8..8 {
                let expected = (((channel as u32 + dither as u32).min(255)) >> 3) as u8;
                assert_eq!(
                    quantize_channel(channel, threshold(dither)),
                    expected,
                    "channel={channel} dither={dither}"
                );
            }
        }
    }

    #[test]
    fn quantize_channel_saturates_at_255_not_wraps() {
        // 250 + 7 = 257, must saturate to 255 (>>3 = 31), never wrap to 1.
        assert_eq!(quantize_channel(250, threshold(7)), 255 >> 3);
        assert_eq!(quantize_channel(255, threshold(7)), 255 >> 3);
        assert_eq!(quantize_channel(255, threshold(0)), 255 >> 3);
    }

    #[test]
    fn quantize_channel_result_always_fits_five_bits() {
        for channel in 0u16..=255 {
            for dither in 0u8..8 {
                assert!(quantize_channel(channel as u8, threshold(dither)) <= 31);
            }
        }
    }

    #[test]
    fn quantize_channel_truncates_not_rounds() {
        // 8 + 6 = 14 -> 14>>3 = 1 (truncation), not round(14/8)=2.
        assert_eq!(quantize_channel(8, threshold(6)), 1);
        // 7 + 0 = 7 -> 7>>3 = 0, the dither threshold alone cannot bump a
        // channel already below the next 8-boundary without exceeding it.
        assert_eq!(quantize_channel(7, threshold(0)), 0);
    }

    // --- quantize_post_float_rgba16_non_hdr: full packing incl. alpha bit ---

    fn pack(r: u8, g: u8, b: u8, coverage_modulo_8: u8, dither: u8) -> u16 {
        quantize_post_float_rgba16_non_hdr(
            Rgba16QuantizeInput {
                r,
                g,
                b,
                coverage_modulo_8: modulo8(coverage_modulo_8),
            },
            threshold(dither),
        )
        .bits()
    }

    #[test]
    fn alpha_bit_is_coverage_modulo_bit_two() {
        for coverage_modulo_8 in 0u8..8 {
            let expected_alpha = if coverage_modulo_8 & 0x4 != 0 { 1 } else { 0 };
            let packed = pack(0, 0, 0, coverage_modulo_8, 0);
            assert_eq!(
                packed & 1,
                expected_alpha,
                "coverage_modulo_8={coverage_modulo_8}"
            );
        }
    }

    #[test]
    fn alpha_bit_ignores_low_two_bits_of_coverage_modulo() {
        // 0b100 (4), 0b101 (5), 0b110 (6), 0b111 (7) must all set the bit;
        // 0b000..0b011 must all clear it -- only bit 2 matters.
        for modulo in [0u8, 1, 2, 3] {
            assert_eq!(pack(0, 0, 0, modulo, 0) & 1, 0);
        }
        for modulo in [4u8, 5, 6, 7] {
            assert_eq!(pack(0, 0, 0, modulo, 0) & 1, 1);
        }
    }

    #[test]
    fn channel_placement_matches_r11_g6_b1_a0_layout() {
        // Isolate each channel by zeroing the others and a zero dither, then
        // confirm the bit position independently of quantize_channel's own
        // correctness (already covered above).
        let r_only = pack(255, 0, 0, 0, 0);
        assert_eq!(r_only, (31u16) << 11);
        let g_only = pack(0, 255, 0, 0, 0);
        assert_eq!(g_only, (31u16) << 6);
        let b_only = pack(0, 0, 255, 0, 0);
        assert_eq!(b_only, (31u16) << 1);
    }

    #[test]
    fn full_saturation_with_max_coverage_and_dither_packs_all_ones() {
        let packed = pack(255, 255, 255, 7, 7);
        assert_eq!(packed, 0xFFFF);
    }

    #[test]
    fn zero_input_packs_to_zero() {
        assert_eq!(pack(0, 0, 0, 0, 0), 0);
    }

    #[test]
    fn dither_applies_identically_and_independently_to_each_channel() {
        // Same base value, same dither, across all three channels must
        // quantize identically -- no cross-channel coupling.
        for base in [0u8, 100, 200, 255] {
            for dither in 0u8..8 {
                let packed = pack(base, base, base, 0, dither);
                let r = (packed >> 11) & 0x1F;
                let g = (packed >> 6) & 0x1F;
                let b = (packed >> 1) & 0x1F;
                assert_eq!(r, g);
                assert_eq!(g, b);
            }
        }
    }

    #[test]
    fn rgba16_packed_bits_accessor_matches_constructed_value() {
        let packed = Rgba16Packed(0xBEEF);
        assert_eq!(packed.bits(), 0xBEEF);
    }

    #[test]
    fn dither_noise_byte_low_three_bits_matches_bitwise_and_seven() {
        for byte in 0u16..=255 {
            let byte = byte as u8;
            assert_eq!(DitherNoiseByte(byte).low_three_bits().value(), byte & 7);
        }
    }

    // --- End-to-end: dither_pattern_value feeding quantize_post_float_rgba16_non_hdr ---

    #[test]
    fn end_to_end_magic_square_selector_at_every_screen_cell_and_channel_boundary() {
        for y in 0..4i32 {
            for x in 0..4i32 {
                let dither = dither_pattern_value(RgbDither::MagicSquare, x, y, noise(0));
                for channel in [0u8, 7, 8, 248, 250, 255] {
                    let expected = quantize_channel(channel, dither);
                    let packed = pack(channel, channel, channel, 0, dither.value());
                    assert_eq!(
                        (packed >> 11) & 0x1F,
                        expected as u16,
                        "x={x} y={y} channel={channel}"
                    );
                }
            }
        }
    }

    // --- WGSL companion: structural/parse/validation guards ---

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(RGB_DITHER_WGSL.contains(&format!("fn {RGB_DITHER_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(RGB_DITHER_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn wgsl_source_contains_the_exact_literal_expressions_the_oracle_depends_on() {
        assert!(RGB_DITHER_WGSL.contains("min(sum, 255u)"));
        assert!(RGB_DITHER_WGSL.contains(">> 3u"));
        assert!(RGB_DITHER_WGSL.contains("0x4u"));
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = RGB_DITHER_WGSL.replacen("@binding(1)", "@binding(0)", 1);
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
        let truncated = &RGB_DITHER_WGSL[..RGB_DITHER_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    #[test]
    fn naga_cannot_catch_a_flipped_saturation_bound() {
        // A `255u` -> `254u` mutation in the saturation clamp still parses
        // and validates under naga; semantic drift here is caught by this
        // file's exhaustive Rust oracle tests and the source-text guard
        // above, not by naga validation alone (matching
        // `alpha_compare.rs`'s identically-scoped precedent).
        let mutated = RGB_DITHER_WGSL.replacen("min(sum, 255u)", "min(sum, 254u)", 1);
        assert_ne!(mutated, RGB_DITHER_WGSL);
        let module = naga::front::wgsl::parse_str(&mutated).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_ok());
    }
}
