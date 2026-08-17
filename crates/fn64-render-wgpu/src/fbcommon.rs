//! `UINT16ToFloat4`/`UINT32ToFloat4`/`UINTToFloat4`/`Float4ToUINT8`/
//! `Float4ToUINT32`: a literal port of five of the seven still-unported
//! dispatch functions in the permitted MIT RT64 Rust-port source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/FbCommon.hlsli:38-68` and
//! `:89-142` (SHA-256 of the whole file,
//! `6ffa6f2d3e2cbb9ce92943ef9965ddefff0e5f4a4c936130308fbed646fc3591`,
//! matching `docs/rt64-port-inventory.json`'s `sources.port.sha256` for that
//! path -- which for this file is identical to its `sources.oracle.sha256`,
//! so the digest is simultaneously the oracle and port digest, confirmed
//! independently here by `shasum -a 256` against the pinned port-commit
//! checkout. The digest names the whole file; this module ports five of its
//! ten functions and the sibling `endian_swap.rs` ports three more, leaving
//! `Float4ToUINT16`/`Float4ToUINT` -- see "Nonclaims" below):
//!
//! ```text
//! float4 UINT16ToFloat4(uint i, uint fmt) {
//!     switch (fmt) {
//!     case G_IM_FMT_RGBA: return RGBA16ToFloat4(i);
//!     case G_IM_FMT_CI:   return 0.0f; // TODO
//!     case G_IM_FMT_IA:   return 0.0f; // TODO
//!     case G_IM_FMT_I:    return 0.0f; // TODO
//!     default:            return 0.0f;
//!     }
//! }
//!
//! float4 UINT32ToFloat4(uint i, uint fmt) {
//!     switch (fmt) {
//!     case G_IM_FMT_RGBA: return RGBA32ToFloat4(i);
//!     case G_IM_FMT_CI:   return 0.0f; // TODO
//!     case G_IM_FMT_IA:   return 0.0f; // TODO
//!     case G_IM_FMT_I:    return 0.0f; // TODO
//!     default:            return 0.0f;
//!     }
//! }
//!
//! float4 UINTToFloat4(uint i, uint siz, uint fmt) {
//!     switch (siz) {
//!     case G_IM_SIZ_4b:  return 0.0f; // TODO
//!     case G_IM_SIZ_8b:  return I8ToFloat4(i); // note: RT64 ignores fmt for 8b
//!     case G_IM_SIZ_16b: return UINT16ToFloat4(i, fmt);
//!     case G_IM_SIZ_32b: return UINT32ToFloat4(i, fmt);
//!     default:           return 0.0f;
//!     }
//! }
//!
//! uint Float4ToUINT8(float4 i, uint fmt, bool oddColumn) {
//!     switch (fmt) {
//!     case G_IM_FMT_I:
//!     case G_IM_FMT_CI:   return FloatToUINT8(oddColumn ? i.g : i.r);
//!     case G_IM_FMT_RGBA: return 0; // TODO
//!     case G_IM_FMT_IA:   return 0; // TODO
//!     default:            return 0;
//!     }
//! }
//!
//! uint Float4ToUINT32(float4 i, uint fmt) {
//!     switch (fmt) {
//!     case G_IM_FMT_RGBA: return Float4ToRGBA32(i);
//!     case G_IM_FMT_CI:   return 0; // TODO
//!     case G_IM_FMT_IA:   return 0; // TODO
//!     case G_IM_FMT_I:    return 0; // TODO
//!     default:            return 0;
//!     }
//! }
//! ```
//!
//! Every `// TODO` above is RT64 upstream's own, present verbatim in the
//! pinned source at those exact lines -- RT64's documented incompleteness,
//! not an fn64 gap. This module ports those stub returns literally
//! (`[0.0; 4]` / `0`), not invented CI/IA/I/4-bit behavior.
//!
//! ## Fallibility
//!
//! All five functions below are **infallible**, matching RT64's own total
//! `float4`/`uint` HLSL signatures exactly: none returns `Result`, and none
//! constructs, returns, or characterizes [`crate::tmem::RawTexelError`] or
//! [`crate::tmem::DirectTexelDecodeError`]. `RawTexel::try_new` is called
//! only after `i` has already been masked to fit its target width, so its
//! `.unwrap()` never observes an `Err` -- see each function's doc for the
//! exact masking step that makes this true.
//!
//! ## `uint_to_float4`'s `Bits8` arm reimplements `I8ToFloat4` locally
//!
//! `I8ToFloat4(uint i) { return i / 255.0f; }` (`Formats.hlsli:71-73`) has no
//! mask and no clamp -- it is total over the full 32-bit domain and its
//! result exceeds `1.0` for any `i > 255`. The crate's existing
//! `tmem::texel::decode_i8` is not usable as this arm's implementation: it
//! computes `(value & 0xff) as u8` first, a different, masked result for any
//! `i > 0xFF`. This module reimplements the unmasked division directly
//! instead of calling `decode_i8`.
//!
//! ## Nonclaims
//!
//! This module does not port
//! [`Float4ToUINT16`](crate::rt64_float4_quantize::float4_to_uint16)'s or
//! [`Float4ToUINT`](crate::rt64_float4_quantize::float4_to_uint)'s RGBA
//! branch *here*: both call `Float4ToRGBA16(float4 i, uint dither, bool
//! usesHDR)`, whose full-`float4`+HDR signature `rgb_dither::
//! quantize_post_float_rgba16_non_hdr` does not provide (it only accepts
//! already-`u8` channels, a precomputed `CoverageModulo8`, and declines the
//! `usesHDR == true` branch entirely -- see its own "Frontier" doc,
//! `rgb_dither.rs:280-322`). Closing that frontier is a separate, larger
//! slice and is not attempted here. Read that as a refusal by *this*
//! module, not as a claim the crate lacks the symbols: both functions,
//! RGBA branch included, are fully ported in `rt64_float4_quantize.rs`,
//! which does provide `usesHDR` handling for that branch. This module does not claim the
//! `rt64-port-m4-src-shaders-fbcommon-hlsli` task card in
//! `docs/rt64-port-inventory.json` is complete (5 of 7 named functions land;
//! 2 do not). It does not claim GPU, WGSL-pipeline, TMEM-wiring, combiner,
//! blend, triangle, texture-rectangle, or production parity of any kind --
//! pure CPU-side dispatch/decode functions only, matching this crate's
//! `formats_dither.rs`/`rgb_dither.rs`/`endian_swap.rs` precedent. It does
//! not claim the CI/IA/I/4-bit `// TODO` stub branches are behaviorally
//! complete -- they are literal ports of RT64's own incomplete upstream code.

use crate::formats_dither::{float4_to_rgba32, float_to_uint8};
use crate::state::{ImageFormat, PixelSize};
use crate::tmem::{decode_direct_texel, RawTexel};

/// Literal port of `UINT16ToFloat4(uint i, uint fmt)` (`FbCommon.hlsli:38-52`).
///
/// `Rgba` masks `i & 0xFFFF` before decoding -- faithful to
/// `RGBA16ToFloat4`'s own shift/mask chain (`>> 11 & 0x1F`, `>> 6 & 0x1F`,
/// `>> 1 & 0x1F`, `& 1`), which never reads any bit above bit 15 either, so
/// masking early and masking via shift/AND land on the identical result for
/// every `u32`. Every other `fmt` (RT64's `CI`/`IA`/`I` stub arms and its
/// `default` arm) returns `[0.0, 0.0, 0.0, 0.0]` unconditionally: `i` is
/// never read on any of those branches in the pinned HLSL, so no width check
/// or `RawTexel` construction happens for them either.
pub fn uint16_to_float4(i: u32, fmt: ImageFormat) -> [f32; 4] {
    match fmt {
        ImageFormat::Rgba => {
            let raw = RawTexel::try_new(PixelSize::Bits16, i & 0xFFFF).unwrap();
            let rgba8888 = decode_direct_texel(ImageFormat::Rgba, raw)
                .unwrap()
                .rgba8888();
            [
                rgba8888[0] as f32 / 255.0,
                rgba8888[1] as f32 / 255.0,
                rgba8888[2] as f32 / 255.0,
                rgba8888[3] as f32 / 255.0,
            ]
        }
        ImageFormat::ColorIndex
        | ImageFormat::IntensityAlpha
        | ImageFormat::Intensity
        | ImageFormat::Yuv => [0.0, 0.0, 0.0, 0.0],
    }
}

/// Literal port of `UINT32ToFloat4(uint i, uint fmt)` (`FbCommon.hlsli:54-68`).
///
/// Identical dispatch shape to [`uint16_to_float4`]. `Rgba` uses
/// `PixelSize::Bits32`, whose `RawTexel::try_new` never fails -- every `u32`
/// is a legal 32-bit value, so no masking is needed before the carrier
/// (`RGBA32ToFloat4` reads all 32 bits). Every other `fmt` returns
/// `[0.0, 0.0, 0.0, 0.0]` unconditionally, same as `uint16_to_float4`.
pub fn uint32_to_float4(i: u32, fmt: ImageFormat) -> [f32; 4] {
    match fmt {
        ImageFormat::Rgba => {
            let raw = RawTexel::try_new(PixelSize::Bits32, i).unwrap();
            let rgba8888 = decode_direct_texel(ImageFormat::Rgba, raw)
                .unwrap()
                .rgba8888();
            [
                rgba8888[0] as f32 / 255.0,
                rgba8888[1] as f32 / 255.0,
                rgba8888[2] as f32 / 255.0,
                rgba8888[3] as f32 / 255.0,
            ]
        }
        ImageFormat::ColorIndex
        | ImageFormat::IntensityAlpha
        | ImageFormat::Intensity
        | ImageFormat::Yuv => [0.0, 0.0, 0.0, 0.0],
    }
}

/// Literal port of `UINTToFloat4(uint i, uint siz, uint fmt)`
/// (`FbCommon.hlsli:70-87`).
///
/// `Bits4` is RT64's own upstream stub (`[0.0; 4]`, no `RawTexel`
/// construction -- RT64 has no 4-bit direct-texel path here). `Bits8` is
/// `I8ToFloat4(i)` reimplemented directly as `i as f32 / 255.0f32`,
/// replicated to all four channels, **unmasked** and **ignoring `fmt`**
/// exactly as the pinned source ignores it for `G_IM_SIZ_8b`. `Bits16`/
/// `Bits32` delegate to [`uint16_to_float4`]/[`uint32_to_float4`] unchanged.
pub fn uint_to_float4(i: u32, siz: PixelSize, fmt: ImageFormat) -> [f32; 4] {
    match siz {
        PixelSize::Bits4 => [0.0, 0.0, 0.0, 0.0],
        PixelSize::Bits8 => {
            let normalized = i as f32 / 255.0f32;
            [normalized, normalized, normalized, normalized]
        }
        PixelSize::Bits16 => uint16_to_float4(i, fmt),
        PixelSize::Bits32 => uint32_to_float4(i, fmt),
    }
}

/// Literal port of `Float4ToUINT8(float4 i, uint fmt, bool oddColumn)`
/// (`FbCommon.hlsli:89-104`).
///
/// `Intensity` and `ColorIndex` share RT64's fallthrough case
/// (`FbCommon.hlsli:91-93`) and both route to
/// `FloatToUINT8(oddColumn ? i.g : i.r)` -- `i[1]` (green) when
/// `odd_column`, else `i[0]` (red). `Rgba`/`IntensityAlpha` are RT64's own
/// stub arms and `Yuv` hits RT64's `default:` arm; all three return `0`.
pub fn float4_to_uint8(i: [f32; 4], fmt: ImageFormat, odd_column: bool) -> u8 {
    match fmt {
        ImageFormat::Intensity | ImageFormat::ColorIndex => {
            float_to_uint8(if odd_column { i[1] } else { i[0] })
        }
        ImageFormat::Rgba | ImageFormat::IntensityAlpha | ImageFormat::Yuv => 0,
    }
}

/// Literal port of `Float4ToUINT32(float4 i, uint fmt)`
/// (`FbCommon.hlsli:125-142`).
///
/// `Rgba` packs all four channels through `Float4ToRGBA32`
/// ([`float4_to_rgba32`]). Every other `fmt` (RT64's `CI`/`IA`/`I` stub arms
/// and its `default` arm) returns `0`.
pub fn float4_to_uint32(i: [f32; 4], fmt: ImageFormat) -> u32 {
    match fmt {
        ImageFormat::Rgba => float4_to_rgba32(i[0], i[1], i[2], i[3]).bits(),
        ImageFormat::ColorIndex
        | ImageFormat::IntensityAlpha
        | ImageFormat::Intensity
        | ImageFormat::Yuv => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- uint16_to_float4 ---

    #[test]
    fn uint16_to_float4_rgba_zero_is_zero() {
        assert_eq!(
            uint16_to_float4(0x0000, ImageFormat::Rgba),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn uint16_to_float4_rgba_all_ones_is_white_opaque() {
        assert_eq!(
            uint16_to_float4(0xFFFF, ImageFormat::Rgba),
            [1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn uint16_to_float4_rgba_mixed_bit_pattern_matches_independent_derivation() {
        // 0x7C1F = 0111_1100_0001_1111 (16 bits, bit15..bit0).
        // r5 = bits[15:11] = 01111 = 15, g5 = bits[10:6] = 10000 = 16,
        // b5 = bits[5:1] = 01111 = 15, a = bit0 = 1.
        // expand_5_to_8(bits5) = (bits5<<3)|(bits5>>2):
        //   expand(15) = (15<<3)|(15>>2) = 120|3 = 123
        //   expand(16) = (16<<3)|(16>>2) = 128|4 = 132
        let value = 0x7C1Fu32;
        let expected_r = 123.0f32 / 255.0;
        let expected_g = 132.0f32 / 255.0;
        let expected_b = 123.0f32 / 255.0;
        let expected_a = 1.0f32;
        assert_eq!(
            uint16_to_float4(value, ImageFormat::Rgba),
            [expected_r, expected_g, expected_b, expected_a]
        );
    }

    #[test]
    fn uint16_to_float4_rgba_high_bit_is_ignored() {
        // 17-bit input, low 16 bits 0xFFFF -- HLSL's masks never observe bit
        // 16, so this must be bit-identical to i = 0xFFFF.
        assert_eq!(
            uint16_to_float4(0x1_FFFF, ImageFormat::Rgba),
            uint16_to_float4(0xFFFF, ImageFormat::Rgba)
        );
        assert_eq!(
            uint16_to_float4(0x1_FFFF, ImageFormat::Rgba),
            [1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn uint16_to_float4_stub_arms_return_zero_unconditionally() {
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
        ] {
            assert_eq!(uint16_to_float4(0x0000, fmt), [0.0, 0.0, 0.0, 0.0]);
            assert_eq!(uint16_to_float4(0xFFFF, fmt), [0.0, 0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn uint16_to_float4_yuv_default_arm_matches_stub_result_but_is_asserted_distinctly() {
        // Yuv hits RT64's `default:` arm, a different source reason than the
        // named CI/IA/I stub cases, but the same numeric result today.
        assert_eq!(
            uint16_to_float4(0x0000, ImageFormat::Yuv),
            [0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            uint16_to_float4(0xFFFF, ImageFormat::Yuv),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn uint16_to_float4_stub_arm_high_bit_characterization() {
        // i = 0x1_0000 (high bit set, low 16 bits zero) on a stub arm: i is
        // unconditionally unread, so this must equal i = 0x0.
        assert_eq!(
            uint16_to_float4(0x1_0000, ImageFormat::ColorIndex),
            uint16_to_float4(0x0, ImageFormat::ColorIndex)
        );
        assert_eq!(
            uint16_to_float4(0x1_0000, ImageFormat::ColorIndex),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn uint16_to_float4_dispatches_by_format_not_by_value() {
        // Mutation-shaped test: a nonzero Rgba input must diverge from every
        // stub arm's result for the same i, catching a swapped dispatch arm.
        let value = 0x7C1F;
        let rgba = uint16_to_float4(value, ImageFormat::Rgba);
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_ne!(rgba, uint16_to_float4(value, fmt));
        }
    }

    // --- uint32_to_float4 ---

    #[test]
    fn uint32_to_float4_rgba_zero_is_zero() {
        assert_eq!(
            uint32_to_float4(0x0000_0000, ImageFormat::Rgba),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn uint32_to_float4_rgba_all_ones_is_white_opaque() {
        assert_eq!(
            uint32_to_float4(0xFFFF_FFFF, ImageFormat::Rgba),
            [1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn uint32_to_float4_rgba_byte_exact_channel_extraction() {
        // R at bits 31-24, G at 23-16, B at 15-8, A at 7-0.
        let value = 0x11_22_33_44u32;
        let expected = [
            0x11 as f32 / 255.0,
            0x22 as f32 / 255.0,
            0x33 as f32 / 255.0,
            0x44 as f32 / 255.0,
        ];
        assert_eq!(uint32_to_float4(value, ImageFormat::Rgba), expected);
    }

    #[test]
    fn uint32_to_float4_stub_arms_return_zero_unconditionally() {
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_eq!(uint32_to_float4(0x0000_0000, fmt), [0.0, 0.0, 0.0, 0.0]);
            assert_eq!(uint32_to_float4(0xFFFF_FFFF, fmt), [0.0, 0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn uint32_to_float4_dispatches_by_format_not_by_value() {
        let value = 0x11_22_33_44;
        let rgba = uint32_to_float4(value, ImageFormat::Rgba);
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_ne!(rgba, uint32_to_float4(value, fmt));
        }
    }

    // --- uint_to_float4 ---

    #[test]
    fn uint_to_float4_bits4_is_a_stub() {
        assert_eq!(
            uint_to_float4(0x0, PixelSize::Bits4, ImageFormat::Rgba),
            [0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            uint_to_float4(0xFFFF_FFFF, PixelSize::Bits4, ImageFormat::Rgba),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn uint_to_float4_bits8_boundary_in_range_is_exact_one() {
        assert_eq!(
            uint_to_float4(0xFF, PixelSize::Bits8, ImageFormat::Intensity),
            [1.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn uint_to_float4_bits8_just_past_boundary_exceeds_one_and_is_not_zero() {
        let result = uint_to_float4(0x100, PixelSize::Bits8, ImageFormat::Intensity);
        let expected = 256.0f32 / 255.0f32;
        assert_eq!(result, [expected, expected, expected, expected]);
        assert!(expected > 1.0);
        assert_ne!(result, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn uint_to_float4_bits8_maximal_value_matches_exact_literal_f32_constant() {
        // (0xFFFFFFFFu32 as f32) rounds to 4294967296.0f32 (2^32 exactly,
        // round-to-nearest-ties-to-even -- 4294967295 is not representable
        // in f32's 24-bit mantissa, nearest representable is 2^32). Dividing
        // that rounded dividend by 255.0f32 gives exact quotient
        // 16_843_009 + 1/255; at that magnitude (in [2^24, 2^25)) the f32
        // ULP is 2, so the correctly-rounded result is 16_843_010.0f32, not
        // the dividend's own exact quotient.
        let result = uint_to_float4(0xFFFF_FFFF, PixelSize::Bits8, ImageFormat::Intensity);
        assert_eq!(result, [16_843_010.0f32; 4]);
        assert_eq!(0xFFFF_FFFFu32 as f32, 4_294_967_296.0f32);
        assert_eq!(4_294_967_296.0f32 / 255.0f32, 16_843_010.0f32);
    }

    #[test]
    fn uint_to_float4_bits8_ignores_fmt() {
        for i in [0x00u32, 0xFF, 0x100, 0xFFFF_FFFF] {
            let a = uint_to_float4(i, PixelSize::Bits8, ImageFormat::Intensity);
            let b = uint_to_float4(i, PixelSize::Bits8, ImageFormat::Rgba);
            assert_eq!(a, b, "i={i:#x}");
        }
    }

    #[test]
    fn uint_to_float4_bits8_diverges_from_the_masking_decode_i8_primitive() {
        // Load-bearing negative/contrast case: decode_i8/decode_direct_texel
        // would mask i & 0xff = 0 for i = 0x100, giving 0.0 -- proving this
        // arm does not silently fall back to that masking primitive.
        let result = uint_to_float4(0x100, PixelSize::Bits8, ImageFormat::Intensity);
        let masked_primitive_result = [0.0f32; 4];
        assert_ne!(result, masked_primitive_result);
    }

    #[test]
    fn uint_to_float4_bits16_delegates_to_uint16_to_float4_unchanged() {
        for i in [0x0000u32, 0x7C1F, 0xFFFF, 0x1_FFFF] {
            for fmt in [
                ImageFormat::Rgba,
                ImageFormat::ColorIndex,
                ImageFormat::IntensityAlpha,
                ImageFormat::Intensity,
                ImageFormat::Yuv,
            ] {
                assert_eq!(
                    uint_to_float4(i, PixelSize::Bits16, fmt),
                    uint16_to_float4(i, fmt),
                    "i={i:#x} fmt={fmt:?}"
                );
            }
        }
    }

    #[test]
    fn uint_to_float4_bits32_delegates_to_uint32_to_float4_unchanged() {
        for i in [0x0000_0000u32, 0x1122_3344, 0xFFFF_FFFF] {
            for fmt in [
                ImageFormat::Rgba,
                ImageFormat::ColorIndex,
                ImageFormat::IntensityAlpha,
                ImageFormat::Intensity,
                ImageFormat::Yuv,
            ] {
                assert_eq!(
                    uint_to_float4(i, PixelSize::Bits32, fmt),
                    uint32_to_float4(i, fmt),
                    "i={i:#x} fmt={fmt:?}"
                );
            }
        }
    }

    #[test]
    fn uint_to_float4_dispatches_by_size_not_by_value() {
        // Mutation-shaped test: the same input value must produce distinct
        // dispatch outcomes across sizes for a value where every size's
        // formula disagrees.
        let value = 0xFF;
        let bits4 = uint_to_float4(value, PixelSize::Bits4, ImageFormat::Rgba);
        let bits8 = uint_to_float4(value, PixelSize::Bits8, ImageFormat::Rgba);
        let bits16 = uint_to_float4(value, PixelSize::Bits16, ImageFormat::Rgba);
        assert_ne!(bits4, bits8);
        assert_ne!(bits8, bits16);
        assert_eq!(bits4, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(bits8, [1.0, 1.0, 1.0, 1.0]);
    }

    // --- float4_to_uint8 ---

    #[test]
    fn float4_to_uint8_intensity_and_color_index_share_one_body() {
        let value = [0.5f32, 0.25, 0.75, 1.0];
        assert_eq!(
            float4_to_uint8(value, ImageFormat::Intensity, false),
            float4_to_uint8(value, ImageFormat::ColorIndex, false)
        );
        assert_eq!(
            float4_to_uint8(value, ImageFormat::Intensity, true),
            float4_to_uint8(value, ImageFormat::ColorIndex, true)
        );
    }

    #[test]
    fn float4_to_uint8_odd_column_selects_green_else_red() {
        let value = [0.2f32, 0.8, 0.0, 0.0];
        let expected_red = float_to_uint8(0.2);
        let expected_green = float_to_uint8(0.8);
        assert_eq!(
            float4_to_uint8(value, ImageFormat::Intensity, false),
            expected_red
        );
        assert_eq!(
            float4_to_uint8(value, ImageFormat::Intensity, true),
            expected_green
        );
        assert_ne!(expected_red, expected_green);
    }

    #[test]
    fn float4_to_uint8_stub_and_default_arms_return_zero() {
        let value = [1.0f32, 1.0, 1.0, 1.0];
        for fmt in [
            ImageFormat::Rgba,
            ImageFormat::IntensityAlpha,
            ImageFormat::Yuv,
        ] {
            assert_eq!(float4_to_uint8(value, fmt, false), 0);
            assert_eq!(float4_to_uint8(value, fmt, true), 0);
        }
    }

    #[test]
    fn float4_to_uint8_dispatches_by_format_not_by_value() {
        let value = [1.0f32, 1.0, 1.0, 1.0];
        let intensity = float4_to_uint8(value, ImageFormat::Intensity, false);
        for fmt in [
            ImageFormat::Rgba,
            ImageFormat::IntensityAlpha,
            ImageFormat::Yuv,
        ] {
            assert_ne!(intensity, float4_to_uint8(value, fmt, false));
        }
    }

    // --- float4_to_uint32 ---

    #[test]
    fn float4_to_uint32_rgba_packs_via_float4_to_rgba32() {
        let value = [1.0f32, 0.0, 0.0, 1.0];
        assert_eq!(
            float4_to_uint32(value, ImageFormat::Rgba),
            float4_to_rgba32(value[0], value[1], value[2], value[3]).bits()
        );
    }

    #[test]
    fn float4_to_uint32_stub_and_default_arms_return_zero() {
        let value = [1.0f32, 1.0, 1.0, 1.0];
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_eq!(float4_to_uint32(value, fmt), 0);
        }
    }

    #[test]
    fn float4_to_uint32_dispatches_by_format_not_by_value() {
        let value = [1.0f32, 0.5, 0.25, 1.0];
        let rgba = float4_to_uint32(value, ImageFormat::Rgba);
        for fmt in [
            ImageFormat::ColorIndex,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
            ImageFormat::Yuv,
        ] {
            assert_ne!(rgba, float4_to_uint32(value, fmt));
        }
    }

    // --- RawTexelError / DirectTexelDecodeError must never appear ---

    #[test]
    fn rgba_arms_never_observe_an_err_from_raw_texel_try_new() {
        // Exhaustive over every u32 masked to Bits16 width plus every u32 at
        // Bits32 width would be too slow; instead assert the invariant the
        // card requires directly: try_new(Bits16, i & 0xFFFF) and
        // try_new(Bits32, i) both succeed for arbitrary i, including the
        // extremes this module's public functions are exercised against
        // above.
        for i in [0x0u32, 0xFFFF, 0x1_FFFF, 0xFFFF_FFFF, 0x1122_3344] {
            assert!(RawTexel::try_new(PixelSize::Bits16, i & 0xFFFF).is_ok());
            assert!(RawTexel::try_new(PixelSize::Bits32, i).is_ok());
        }
    }
}
