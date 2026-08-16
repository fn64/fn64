//! M4.3.3a: allocation-free direct-format texel decode.
//!
//! [`RawTexel`] is a format-neutral raw-value carrier: it validates only that
//! a numeric value fits its `size`'s bit width (4/8/16/32), independent of
//! `format`. It is deliberately not scoped to the seven direct pairs below —
//! later CI/TLUT (M4.3.4/M4.3.5) and YUV layers reuse the same carrier for
//! their own raw texel values before running a palette lookup or chroma
//! conversion instead of a direct decode.
//!
//! [`decode_direct_texel`] is the layer that is scoped to exactly the seven
//! console "direct" `(format, size)` pairs that read one texel's color
//! straight out of TMEM without a palette lookup: RGBA16, RGBA32, IA4, IA8,
//! IA16, I4, and I8. Which `(format, size)` pairs are legal at all is the
//! public SGI *Nintendo 64 RDP Command Summary* Table 4 image-data-format
//! legality matrix. The format and size selector *encodings* dispatched on
//! here are not owned by this module: SetTextureImage (Table 3) and SetTile
//! (Table 6) each define both the `G_IM_FMT_*` format field and the
//! `G_IM_SIZ_*` size field on their own command word, and both are already
//! transcribed at [`crate::ImageFormat`]/[`crate::PixelSize`]
//! (`wire.rs::image_format`/`wire.rs::pixel_size`) — this module reuses that
//! prior transcription rather than re-deriving selector values. `ColorIndex`
//! and `Yuv` pairs are rejected here as typed, named scope exclusions, not
//! silent fallthroughs: `ColorIndex` decodes through a separate, TLUT-mode-
//! aware indexed path this module never runs, and `Yuv` requires chroma
//! conversion. That separate path's behavior is not unconditionally "a
//! palette lookup" — the pinned RT64 source's own dispatch (`sampleTMEM`,
//! TextureDecoder.hlsli:149-208) branches first on whether a TLUT is active
//! at all (`usesTlut`, line 153/174), and only then, when TLUT is *not*
//! active, falls through to `sampleTMEM8b`/`16b`/`32b`, which alias
//! `G_IM_FMT_CI` to the same intensity decode as `G_IM_FMT_I` (line 74: "CI
//! behaves like I when a TLUT is not active"). This module does not resolve
//! that TLUT-mode branch and makes no claim about which side of it applies;
//! it only names `ColorIndex` as out of its own direct-decode scope.
//!
//! Decode formulas are transcribed from the permitted MIT RT64 Rust-port
//! source pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/Formats.hlsli` (digest
//! `9b5765371d19de1e410dbe919433922db975994e2a6077bf9e499a8a94f33b7b`):
//! `I4ToFloat4` (lines 56-59), `IA4ToFloat4` (61-65), `I8ToFloat4` (71-73),
//! `IA8ToFloat4` (75-81), `RGBA16ToFloat4` (83-93), `IA16ToFloat4`
//! (108-112), and `RGBA32ToFloat4` (114-120); and
//! `src/shaders/TextureDecoder.hlsli` (digest
//! `63b2c1ce683e7e7880c9508d3232d90e90236157ac86ae91947c62ae1d359f07`),
//! whose `sampleTMEM4b`/`sampleTMEM8b`/`sampleTMEM16b`/`sampleTMEM32b`
//! (lines 45-58, 68-80, 99-113, 135-147) select `I*ToFloat4` for
//! `G_IM_FMT_I` and reuse it for `G_IM_FMT_RGBA` at 4/8 bit, citing hardware
//! observation rather than a distinct real format; that RGBA/I aliasing at
//! 4/8 bit is out of scope here; direct RGBA decode is defined by this module
//! only at its two real sizes, 16 and 32 bit. fn64's own prior transcription
//! of the same RDP field widths lives in
//! [`crate::tmem::wire`] (`docs/RENDER-WGPU-PORT-PLAN.md` M4.1/M4.2).
//!
//! This module claims none of: TMEM addressing or storage, RDRAM/physical
//! source validity, CI palette lookup, TLUT contents, YUV/chroma conversion,
//! bilinear or box filtering, GPU upload, or RT64 pixel-for-pixel parity. It
//! is a pure function from one already-isolated raw texel value plus its
//! `(format, size)` pair to one RGBA8888 color.

use core::fmt;

use crate::{ImageFormat, PixelSize};

/// One raw texel value, validated only against its `size`'s bit width.
///
/// This carrier is format-neutral by design: it takes no position on which
/// `(format, size)` pairs are meaningful, so CI/TLUT and YUV decode layers
/// can reuse it for their own raw values. [`RawTexel::try_new`] loudly
/// rejects a `value` that does not fit `size`'s bit width rather than masking
/// it. Fields are private so no caller can construct an already-invalid
/// instance by other means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawTexel {
    size: PixelSize,
    value: u32,
}

/// Why a raw value could not be carried as a [`RawTexel`].
///
/// This error is width-only: it says nothing about whether a `(format,
/// size)` pair is meaningful to any particular decode layer, only whether
/// `value` fits `size`'s defined bit width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawTexelError {
    size: PixelSize,
    width_bits: u32,
    value: u32,
}

impl RawTexelError {
    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn width_bits(self) -> u32 {
        self.width_bits
    }

    pub const fn value(self) -> u32 {
        self.value
    }
}

impl fmt::Display for RawTexelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "texel value {:#x} does not fit the {}-bit width of {:?}",
            self.value, self.width_bits, self.size
        )
    }
}

impl std::error::Error for RawTexelError {}

const fn size_width_bits(size: PixelSize) -> u32 {
    match size {
        PixelSize::Bits4 => 4,
        PixelSize::Bits8 => 8,
        PixelSize::Bits16 => 16,
        PixelSize::Bits32 => 32,
    }
}

impl RawTexel {
    /// Builds a raw texel from a `size` and a big-endian-combined numeric
    /// `value`.
    ///
    /// `value` must fit `size`'s bit width: 4 bits, 8 bits, 16 bits, or 32
    /// bits. A caller that already has raw big-endian bytes combines them
    /// with `u32::from_be_bytes` (padded with leading zero bytes for 8/4-bit
    /// texels) before calling this constructor; this module never reads
    /// memory itself.
    pub const fn try_new(size: PixelSize, value: u32) -> Result<Self, RawTexelError> {
        let width_bits = size_width_bits(size);
        let max = if width_bits == 32 {
            u32::MAX
        } else {
            (1u32 << width_bits) - 1
        };
        if value > max {
            return Err(RawTexelError {
                size,
                width_bits,
                value,
            });
        }
        Ok(Self { size, value })
    }

    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn value(self) -> u32 {
        self.value
    }
}

/// A decoded texel's RGBA8888 color, expanded from a direct-format
/// [`RawTexel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedTexel {
    rgba8888: [u8; 4],
}

impl DecodedTexel {
    const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            rgba8888: [r, g, b, a],
        }
    }

    /// Returns the decoded color as `[r, g, b, a]`, each an 8-bit channel.
    pub const fn rgba8888(self) -> [u8; 4] {
        self.rgba8888
    }
}

/// Why a `(format, size)` pair could not be decoded as a direct texel.
///
/// Variants separate the three reasons a pair is out of this module's scope,
/// so a caller can distinguish "this needs a palette" from "this needs
/// chroma conversion" from "this pair is neither of those and not direct
/// either". Width validation is [`RawTexel`]'s job ([`RawTexelError`]), not
/// this enum's — a `RawTexel` is already width-valid by construction, so
/// this error only ever reports a scope rejection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectTexelDecodeError {
    /// `ColorIndex` pairs are decoded through a separate, TLUT-mode-aware
    /// indexed path, not through this module's direct decode. That separate
    /// path is not unconditionally a palette lookup: per the pinned RT64
    /// source, it resolves against the TLUT only while a TLUT is active, and
    /// otherwise (`CI8`/`CI16`/`CI32`) decodes identically to the
    /// corresponding `Intensity` pair. This module takes no position on
    /// which side applies; it only declares `ColorIndex` out of scope here.
    IndexedDecodeIsSeparate { size: PixelSize },
    /// `Yuv` pairs require chroma conversion, deferred per
    /// `docs/RENDER-WGPU-PORT-PLAN.md` M4.3.
    YuvConversionDeferred { size: PixelSize },
    /// A `(format, size)` pair that is neither one of the seven direct
    /// pairs, `ColorIndex`, nor `Yuv` (for example 4-bit or 8-bit direct
    /// RGBA, which the console does not define as real formats).
    UnsupportedPair {
        format: ImageFormat,
        size: PixelSize,
    },
}

impl fmt::Display for DirectTexelDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexedDecodeIsSeparate { size } => write!(
                formatter,
                "color-index texel at {size:?} decodes through a separate TLUT-aware indexed path, not direct decode"
            ),
            Self::YuvConversionDeferred { size } => write!(
                formatter,
                "YUV texel at {size:?} requires chroma conversion, deferred per M4.3"
            ),
            Self::UnsupportedPair { format, size } => write!(
                formatter,
                "(format, size) pair {format:?}/{size:?} is not one of the seven direct texel pairs"
            ),
        }
    }
}

impl std::error::Error for DirectTexelDecodeError {}

/// Decodes one raw texel into its RGBA8888 color, or the typed reason
/// `format`/`raw`'s size are not a direct pair.
///
/// This is a pure function; it performs no TMEM, RDRAM, or GPU access and
/// allocates nothing. `raw` is already width-valid for its size by
/// construction ([`RawTexel::try_new`]); this function classifies exactly
/// the seven direct `(format, size)` pairs and rejects every other pair with
/// a typed [`DirectTexelDecodeError`].
pub fn decode_direct_texel(
    format: ImageFormat,
    raw: RawTexel,
) -> Result<DecodedTexel, DirectTexelDecodeError> {
    match (format, raw.size) {
        (ImageFormat::Rgba, PixelSize::Bits16) => Ok(decode_rgba16(raw.value)),
        (ImageFormat::Rgba, PixelSize::Bits32) => Ok(decode_rgba32(raw.value)),
        (ImageFormat::IntensityAlpha, PixelSize::Bits4) => Ok(decode_ia4(raw.value)),
        (ImageFormat::IntensityAlpha, PixelSize::Bits8) => Ok(decode_ia8(raw.value)),
        (ImageFormat::IntensityAlpha, PixelSize::Bits16) => Ok(decode_ia16(raw.value)),
        (ImageFormat::Intensity, PixelSize::Bits4) => Ok(decode_i4(raw.value)),
        (ImageFormat::Intensity, PixelSize::Bits8) => Ok(decode_i8(raw.value)),
        (ImageFormat::ColorIndex, size) => {
            Err(DirectTexelDecodeError::IndexedDecodeIsSeparate { size })
        }
        (ImageFormat::Yuv, size) => Err(DirectTexelDecodeError::YuvConversionDeferred { size }),
        (format, size) => Err(DirectTexelDecodeError::UnsupportedPair { format, size }),
    }
}

/// `RGBA16ToFloat4`, Formats.hlsli:83-93. 5 bits each of R/G/B, left-shifted
/// by 3 and OR'd with their own top 2 bits to expand 5-bit to 8-bit; the
/// low bit is a 1-bit coverage/alpha flag expanded to 0x00 or 0xff.
fn decode_rgba16(value: u32) -> DecodedTexel {
    let r5 = (value >> 11) & 0x1f;
    let g5 = (value >> 6) & 0x1f;
    let b5 = (value >> 1) & 0x1f;
    let a = value & 1;
    DecodedTexel::new(
        expand_5_to_8(r5),
        expand_5_to_8(g5),
        expand_5_to_8(b5),
        if a != 0 { 0xff } else { 0x00 },
    )
}

const fn expand_5_to_8(bits5: u32) -> u8 {
    (((bits5 << 3) | (bits5 >> 2)) & 0xff) as u8
}

/// `RGBA32ToFloat4`, Formats.hlsli:114-120. Four independent 8-bit channels
/// packed big-endian as R:G:B:A from bit 31 down to bit 0.
fn decode_rgba32(value: u32) -> DecodedTexel {
    let r = (value >> 24) & 0xff;
    let g = (value >> 16) & 0xff;
    let b = (value >> 8) & 0xff;
    let a = value & 0xff;
    DecodedTexel::new(r as u8, g as u8, b as u8, a as u8)
}

/// `IA4ToFloat4`, Formats.hlsli:61-65. The top 3 bits are intensity,
/// expanded to 8-bit by `(i << 4) | (i << 1) | (i >> 2)`; the low bit is a
/// 1-bit alpha flag expanded to 0x00 or 0xff. Intensity feeds all of R/G/B.
fn decode_ia4(value: u32) -> DecodedTexel {
    let i3 = value & 0b1110;
    let i8 = (((i3 << 4) | (i3 << 1) | (i3 >> 2)) & 0xff) as u8;
    let a = if value & 1 != 0 { 0xff } else { 0x00 };
    DecodedTexel::new(i8, i8, i8, a)
}

/// `I4ToFloat4`, Formats.hlsli:56-59. The 4-bit value is replicated into
/// both nibbles of one byte and feeds R/G/B/A alike.
fn decode_i4(value: u32) -> DecodedTexel {
    let i8 = (((value << 4) | value) & 0xff) as u8;
    DecodedTexel::new(i8, i8, i8, i8)
}

/// `IA8ToFloat4`, Formats.hlsli:75-81. High nibble is intensity, low nibble
/// is alpha; each nibble is replicated to fill its own byte.
fn decode_ia8(value: u32) -> DecodedTexel {
    let i_nibble = (value >> 4) & 0xf;
    let a_nibble = value & 0xf;
    let i8 = (((i_nibble << 4) | i_nibble) & 0xff) as u8;
    let a8 = (((a_nibble << 4) | a_nibble) & 0xff) as u8;
    DecodedTexel::new(i8, i8, i8, a8)
}

/// `I8ToFloat4`, Formats.hlsli:71-73. The byte feeds R/G/B/A alike, with no
/// expansion since it is already 8 bits.
fn decode_i8(value: u32) -> DecodedTexel {
    let i8 = (value & 0xff) as u8;
    DecodedTexel::new(i8, i8, i8, i8)
}

/// `IA16ToFloat4`, Formats.hlsli:108-112. High byte is intensity, low byte
/// is alpha; both are already 8 bits, big-endian packed.
fn decode_ia16(value: u32) -> DecodedTexel {
    let i8 = ((value >> 8) & 0xff) as u8;
    let a8 = (value & 0xff) as u8;
    DecodedTexel::new(i8, i8, i8, a8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(format: ImageFormat, size: PixelSize, value: u32) -> DecodedTexel {
        let raw = RawTexel::try_new(size, value).unwrap();
        decode_direct_texel(format, raw).unwrap()
    }

    // -- RGBA16: ffff,0001,f800,f801,07c1,003f,0801 --

    #[test]
    fn rgba16_all_ones_is_opaque_white() {
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0xffff);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn rgba16_low_bit_only_is_opaque_black() {
        // 0x0001: r=g=b=0, coverage bit set -> opaque black, not transparent:
        // the low bit is alpha itself, independent of r/g/b.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0x0001);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn rgba16_red_channel_transparent() {
        // 0xf800: r5=0x1f, g5=0, b5=0, a=0.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0xf800);
        assert_eq!(decoded.rgba8888(), [0xff, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn rgba16_red_channel_opaque() {
        // 0xf801: r5=0x1f, g5=0, b5=0, a=1.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0xf801);
        assert_eq!(decoded.rgba8888(), [0xff, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn rgba16_mixed_bit_pattern() {
        // 0x07c1: r5=0, g5=0x1f, b5=0, a=1.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0x07c1);
        assert_eq!(decoded.rgba8888(), [0x00, 0xff, 0x00, 0xff]);
    }

    #[test]
    fn rgba16_blue_channel_opaque() {
        // 0x003f: r5=0, g5=0, b5=0x1f, a=1.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0x003f);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn rgba16_single_low_red_bit() {
        // 0x0801: r5=1 (bit 11), g5=0, b5=0, a=1 -> r expands (1<<3)|(1>>2) = 8.
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits16, 0x0801);
        assert_eq!(decoded.rgba8888(), [0x08, 0x00, 0x00, 0xff]);
    }

    // -- RGBA32: 804020ff --

    #[test]
    fn rgba32_exact_byte_literal() {
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits32, 0x804020ff);
        assert_eq!(decoded.rgba8888(), [0x80, 0x40, 0x20, 0xff]);
    }

    // -- IA16: 00ff,ff00 --

    #[test]
    fn ia16_zero_intensity_full_alpha() {
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits16, 0x00ff);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ia16_full_intensity_zero_alpha() {
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits16, 0xff00);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0x00]);
    }

    // -- IA8: f0,0f,81 --

    #[test]
    fn ia8_full_intensity_zero_alpha() {
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits8, 0xf0);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0x00]);
    }

    #[test]
    fn ia8_zero_intensity_full_alpha() {
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits8, 0x0f);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ia8_mixed_nibbles() {
        // 0x81: i_nibble=8 -> 0x88, a_nibble=1 -> 0x11.
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits8, 0x81);
        assert_eq!(decoded.rgba8888(), [0x88, 0x88, 0x88, 0x11]);
    }

    // -- IA4: e,1,4 --

    #[test]
    fn ia4_max_intensity_zero_alpha() {
        // 0xe = 0b1110: i3=0b1110, a bit=0.
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits4, 0xe);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0x00]);
    }

    #[test]
    fn ia4_zero_intensity_full_alpha() {
        // 0x1 = 0b0001: i3=0, a bit=1.
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits4, 0x1);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ia4_mid_intensity_zero_alpha() {
        // 0x4 = 0b0100: i3=0b0100 -> (4<<4)|(4<<1)|(4>>2) = 64|8|1 = 73 = 0x49.
        let decoded = decode(ImageFormat::IntensityAlpha, PixelSize::Bits4, 0x4);
        assert_eq!(decoded.rgba8888(), [0x49, 0x49, 0x49, 0x00]);
    }

    // -- I8: 00,80,ff --

    #[test]
    fn i8_zero() {
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits8, 0x00);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn i8_mid() {
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits8, 0x80);
        assert_eq!(decoded.rgba8888(), [0x80, 0x80, 0x80, 0x80]);
    }

    #[test]
    fn i8_max() {
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits8, 0xff);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0xff]);
    }

    // -- I4: 0,8,f --

    #[test]
    fn i4_zero() {
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits4, 0x0);
        assert_eq!(decoded.rgba8888(), [0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn i4_mid() {
        // 0x8 = 0b1000 -> (8<<4)|8 = 0x88.
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits4, 0x8);
        assert_eq!(decoded.rgba8888(), [0x88, 0x88, 0x88, 0x88]);
    }

    #[test]
    fn i4_max() {
        // 0xf -> (f<<4)|f = 0xff.
        let decoded = decode(ImageFormat::Intensity, PixelSize::Bits4, 0xf);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0xff]);
    }

    // -- width-overflow rejection --

    #[test]
    fn bits4_rejects_value_overflowing_4_bits() {
        let error = RawTexel::try_new(PixelSize::Bits4, 0x10).unwrap_err();
        assert_eq!(
            error,
            RawTexelError {
                size: PixelSize::Bits4,
                width_bits: 4,
                value: 0x10,
            }
        );
    }

    #[test]
    fn bits8_rejects_value_overflowing_8_bits() {
        let error = RawTexel::try_new(PixelSize::Bits8, 0x100).unwrap_err();
        assert_eq!(
            error,
            RawTexelError {
                size: PixelSize::Bits8,
                width_bits: 8,
                value: 0x100,
            }
        );
    }

    #[test]
    fn bits16_rejects_value_overflowing_16_bits() {
        let error = RawTexel::try_new(PixelSize::Bits16, 0x10000).unwrap_err();
        assert_eq!(
            error,
            RawTexelError {
                size: PixelSize::Bits16,
                width_bits: 16,
                value: 0x10000,
            }
        );
    }

    // RGBA32's u32 domain cannot overflow its own 32-bit width, so it has
    // no overflow case symmetric with the six narrower pairs above.
    #[test]
    fn rgba32_accepts_full_u32_range() {
        let decoded = decode(ImageFormat::Rgba, PixelSize::Bits32, u32::MAX);
        assert_eq!(decoded.rgba8888(), [0xff, 0xff, 0xff, 0xff]);
    }

    // -- exhaustive 20 (format, size) pairs --

    const ALL_FORMATS: [ImageFormat; 5] = [
        ImageFormat::Rgba,
        ImageFormat::Yuv,
        ImageFormat::ColorIndex,
        ImageFormat::IntensityAlpha,
        ImageFormat::Intensity,
    ];
    const ALL_SIZES: [PixelSize; 4] = [
        PixelSize::Bits4,
        PixelSize::Bits8,
        PixelSize::Bits16,
        PixelSize::Bits32,
    ];

    #[test]
    fn exhaustive_20_pairs_classify_correctly() {
        for &format in &ALL_FORMATS {
            for &size in &ALL_SIZES {
                let raw = RawTexel::try_new(size, 0).unwrap();
                let result = decode_direct_texel(format, raw);
                let is_direct = matches!(
                    (format, size),
                    (ImageFormat::Rgba, PixelSize::Bits16)
                        | (ImageFormat::Rgba, PixelSize::Bits32)
                        | (ImageFormat::IntensityAlpha, PixelSize::Bits4)
                        | (ImageFormat::IntensityAlpha, PixelSize::Bits8)
                        | (ImageFormat::IntensityAlpha, PixelSize::Bits16)
                        | (ImageFormat::Intensity, PixelSize::Bits4)
                        | (ImageFormat::Intensity, PixelSize::Bits8)
                );
                match (format, is_direct) {
                    (_, true) => {
                        assert!(
                            result.is_ok(),
                            "expected {format:?}/{size:?} to be a direct pair, got {result:?}"
                        );
                    }
                    (ImageFormat::ColorIndex, false) => {
                        assert_eq!(
                            result,
                            Err(DirectTexelDecodeError::IndexedDecodeIsSeparate { size })
                        );
                    }
                    (ImageFormat::Yuv, false) => {
                        assert_eq!(
                            result,
                            Err(DirectTexelDecodeError::YuvConversionDeferred { size })
                        );
                    }
                    (_, false) => {
                        assert_eq!(
                            result,
                            Err(DirectTexelDecodeError::UnsupportedPair { format, size })
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn error_display_and_error_trait_are_implemented() {
        let direct_errors = [
            DirectTexelDecodeError::IndexedDecodeIsSeparate {
                size: PixelSize::Bits8,
            },
            DirectTexelDecodeError::YuvConversionDeferred {
                size: PixelSize::Bits16,
            },
            DirectTexelDecodeError::UnsupportedPair {
                format: ImageFormat::Rgba,
                size: PixelSize::Bits4,
            },
        ];
        for error in direct_errors {
            let rendered = error.to_string();
            assert!(!rendered.is_empty());
            let _: &dyn std::error::Error = &error;
        }

        let raw_error = RawTexelError {
            size: PixelSize::Bits4,
            width_bits: 4,
            value: 0x10,
        };
        let rendered = raw_error.to_string();
        assert!(!rendered.is_empty());
        let _: &dyn std::error::Error = &raw_error;
    }

    #[test]
    fn raw_texel_accessors_round_trip() {
        let texel = RawTexel::try_new(PixelSize::Bits16, 0xffff).unwrap();
        assert_eq!(texel.size(), PixelSize::Bits16);
        assert_eq!(texel.value(), 0xffff);
    }
}
