//! M4.3.3: allocation-free direct and indexed-value texel decode.
//!
//! [`RawTexel`] is a format-neutral raw-value carrier: it validates only that
//! a numeric value fits its `size`'s bit width (4/8/16/32), independent of
//! `format`. It is deliberately not scoped to the seven direct pairs below —
//! M4.3.3b's CI/TLUT functions below and a future YUV layer reuse the same
//! carrier for their own raw values instead of inventing format-specific raw
//! wrappers.
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
//! and `Yuv` pairs are rejected by `decode_direct_texel` as typed, named scope
//! exclusions, not silent fallthroughs: `ColorIndex` instead enters
//! M4.3.3b's TLUT-mode-aware [`resolve_indexed_texel`] path below, while `Yuv`
//! still requires deferred chroma conversion. The pinned RT64 `sampleTMEM`
//! dispatch (TextureDecoder.hlsli:149-208) first tests whether a TLUT is
//! active and, when disabled, aliases CI to intensity. M4.3.3b implements
//! that branch only for the admitted CI4/CI8 pairs; despite RT64 also
//! exhibiting disabled CI16/CI32 aliases, this slice rejects CI16/CI32
//! explicitly rather than broadening its contract.
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
//! M4.3.3b adds the pure value boundary on the other side of that typed
//! refusal. It extracts an already-isolated CI4 packed byte, normalizes CI4
//! palette plus nibble or a CI8 byte into an eight-bit index, and either
//! aliases the normalized index to I8 while the TLUT is disabled or returns
//! a typed lookup naming the canonical quadricated high-bank entry address
//! and RGBA16/IA16 interpretation. A separately supplied big-endian 16-bit
//! entry is decoded through the existing direct conversion. The selector and
//! CI behavior follow the permitted MIT RT64 source pinned at
//! `5473732a822a4423b5696e7cb18fecc425a59875`: `shared/rt64_f3d_defines.h`,
//! `shared/rt64_other_mode.h`, and `src/shaders/TextureDecoder.hlsli`.
//!
//! This module claims none of: physical TMEM addressing or reads, validity,
//! epoch or generation binding, snapshot identity, tile-coordinate mapping,
//! sub-16-entry footprints, sampling, filtering, bilerp, LOD, cache identity,
//! RDRAM, GPU upload, production dispatch, YUV conversion, non-CI TLUT-mode
//! behavior, RT64 pixel-for-pixel parity, or performance. Its indexed API is
//! pure over already-isolated index and entry values; a later physical reader
//! must bind both values to one immutable physical-state identity/generation.

use core::fmt;

use crate::{ImageFormat, PixelSize, TextureLutMode};

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

/// A checked four-bit CI palette selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ci4Palette(u8);

impl Ci4Palette {
    pub const fn try_new(value: u8) -> Result<Self, Ci4PaletteError> {
        if value <= 0x0f {
            Ok(Self(value))
        } else {
            Err(Ci4PaletteError { value })
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Why a raw palette selector could not become a [`Ci4Palette`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ci4PaletteError {
    value: u8,
}

impl Ci4PaletteError {
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl fmt::Display for Ci4PaletteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "CI4 palette selector {} exceeds its four-bit field",
            self.value
        )
    }
}

impl std::error::Error for Ci4PaletteError {}

/// Which texel in a high-nibble-first CI4 packed byte is requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexelColumnParity {
    Even,
    Odd,
}

/// Why a packed CI4 byte could not be unpacked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ci4UnpackError {
    PackedByteMustBeBits8 { size: PixelSize },
}

impl fmt::Display for Ci4UnpackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PackedByteMustBeBits8 { size } => write!(
                formatter,
                "CI4 packed source must be an eight-bit byte, not {size:?}"
            ),
        }
    }
}

impl std::error::Error for Ci4UnpackError {}

/// Extracts one CI4 texel from an already-isolated packed byte.
///
/// The even-column texel occupies bits 7:4 and the odd-column texel bits 3:0,
/// following the permitted pinned RT64 `TextureDecoder.hlsli` CI4 path. No
/// physical TMEM address or tile-coordinate mapping is performed here.
pub fn unpack_ci4_texel(
    packed_byte: RawTexel,
    parity: TexelColumnParity,
) -> Result<RawTexel, Ci4UnpackError> {
    if packed_byte.size() != PixelSize::Bits8 {
        return Err(Ci4UnpackError::PackedByteMustBeBits8 {
            size: packed_byte.size(),
        });
    }
    let value = match parity {
        TexelColumnParity::Even => packed_byte.value() >> 4,
        TexelColumnParity::Odd => packed_byte.value() & 0x0f,
    };
    Ok(RawTexel {
        size: PixelSize::Bits4,
        value,
    })
}

/// One enabled-TLUT lookup requested by a normalized CI index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlutLookup {
    index: u8,
    entry_format: ImageFormat,
    byte_address: u16,
}

impl TlutLookup {
    pub const fn index(self) -> u8 {
        self.index
    }

    pub const fn entry_format(self) -> ImageFormat {
        self.entry_format
    }

    /// Canonical physical byte address of the quadricated entry's first
    /// 16-bit lane: `0x800 + index * 8`.
    pub const fn byte_address(self) -> u16 {
        self.byte_address
    }
}

/// The only two legal outcomes of resolving one CI texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedIndexedTexel {
    /// TLUT-disabled CI aliases the normalized eight-bit index to I8.
    Direct(DecodedTexel),
    /// TLUT-enabled CI requires exactly one separately supplied 16-bit entry.
    Tlut(TlutLookup),
}

/// Why an index value could not be resolved as CI4 or CI8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexedTexelResolveError {
    FormatMustBeColorIndex { format: ImageFormat },
    UnsupportedIndexSize { size: PixelSize },
}

impl fmt::Display for IndexedTexelResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatMustBeColorIndex { format } => {
                write!(
                    formatter,
                    "indexed decode requires ColorIndex, not {format:?}"
                )
            }
            Self::UnsupportedIndexSize { size } => {
                write!(
                    formatter,
                    "indexed decode supports only CI4 and CI8, not {size:?}"
                )
            }
        }
    }
}

impl std::error::Error for IndexedTexelResolveError {}

/// Resolves one already-isolated CI4 nibble or CI8 byte.
///
/// CI4 combines its four-bit palette selector with the nibble. CI8 ignores
/// the palette selector and uses the byte unchanged. With no TLUT, both paths
/// decode that normalized eight-bit index as I8. With a TLUT, the result is a
/// lookup authority and cannot be mistaken for a direct color.
pub fn resolve_indexed_texel(
    format: ImageFormat,
    raw_index: RawTexel,
    palette: Ci4Palette,
    lut_mode: TextureLutMode,
) -> Result<ResolvedIndexedTexel, IndexedTexelResolveError> {
    if format != ImageFormat::ColorIndex {
        return Err(IndexedTexelResolveError::FormatMustBeColorIndex { format });
    }
    let index = match raw_index.size() {
        PixelSize::Bits4 => (palette.value() << 4) | raw_index.value() as u8,
        PixelSize::Bits8 => raw_index.value() as u8,
        size => return Err(IndexedTexelResolveError::UnsupportedIndexSize { size }),
    };
    match lut_mode {
        TextureLutMode::Disabled => Ok(ResolvedIndexedTexel::Direct(decode_i8(u32::from(index)))),
        TextureLutMode::Rgba16 => Ok(ResolvedIndexedTexel::Tlut(TlutLookup {
            index,
            entry_format: ImageFormat::Rgba,
            byte_address: 0x0800 + u16::from(index) * 8,
        })),
        TextureLutMode::Ia16 => Ok(ResolvedIndexedTexel::Tlut(TlutLookup {
            index,
            entry_format: ImageFormat::IntensityAlpha,
            byte_address: 0x0800 + u16::from(index) * 8,
        })),
    }
}

/// Why a supplied TLUT entry could not be decoded for its typed lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlutEntryDecodeError {
    EntryMustBeBits16 { size: PixelSize },
    Direct(DirectTexelDecodeError),
}

impl fmt::Display for TlutEntryDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryMustBeBits16 { size } => {
                write!(
                    formatter,
                    "TLUT entry must be a big-endian 16-bit value, not {size:?}"
                )
            }
            Self::Direct(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TlutEntryDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EntryMustBeBits16 { .. } => None,
            Self::Direct(error) => Some(error),
        }
    }
}

/// Decodes one caller-supplied, big-endian 16-bit entry for an enabled-TLUT
/// lookup. A disabled-mode result has no [`TlutLookup`] and therefore cannot
/// be passed here without fabricating a value whose fields are private.
pub fn decode_tlut_entry(
    lookup: TlutLookup,
    entry: RawTexel,
) -> Result<DecodedTexel, TlutEntryDecodeError> {
    if entry.size() != PixelSize::Bits16 {
        return Err(TlutEntryDecodeError::EntryMustBeBits16 { size: entry.size() });
    }
    decode_direct_texel(lookup.entry_format, entry).map_err(TlutEntryDecodeError::Direct)
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
    /// path is not unconditionally a palette lookup: M4.3.3b resolves CI4/CI8
    /// against the TLUT only while it is active and aliases the normalized
    /// index to I8 otherwise. The pinned RT64 source also exhibits disabled
    /// CI16/CI32 intensity aliases, but M4.3.3b deliberately rejects those
    /// sizes; this direct decoder declares every `ColorIndex` size out of its
    /// own scope.
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

    fn raw(size: PixelSize, value: u32) -> RawTexel {
        RawTexel::try_new(size, value).unwrap()
    }

    fn palette(value: u8) -> Ci4Palette {
        Ci4Palette::try_new(value).unwrap()
    }

    fn resolve(
        size: PixelSize,
        value: u32,
        palette_value: u8,
        mode: TextureLutMode,
    ) -> ResolvedIndexedTexel {
        resolve_indexed_texel(
            ImageFormat::ColorIndex,
            raw(size, value),
            palette(palette_value),
            mode,
        )
        .unwrap()
    }

    fn lookup(size: PixelSize, value: u32, palette_value: u8, mode: TextureLutMode) -> TlutLookup {
        match resolve(size, value, palette_value, mode) {
            ResolvedIndexedTexel::Tlut(lookup) => lookup,
            ResolvedIndexedTexel::Direct(_) => panic!("enabled TLUT resolved directly"),
        }
    }

    #[test]
    fn ci4_unpack_is_high_nibble_first() {
        let packed = raw(PixelSize::Bits8, 0x1f);
        assert_eq!(
            unpack_ci4_texel(packed, TexelColumnParity::Even)
                .unwrap()
                .value(),
            0x1
        );
        assert_eq!(
            unpack_ci4_texel(packed, TexelColumnParity::Odd)
                .unwrap()
                .value(),
            0xf
        );
    }

    #[test]
    fn ci4_unpack_rejects_every_non_byte_width() {
        for size in [PixelSize::Bits4, PixelSize::Bits16, PixelSize::Bits32] {
            assert_eq!(
                unpack_ci4_texel(raw(size, 0), TexelColumnParity::Even),
                Err(Ci4UnpackError::PackedByteMustBeBits8 { size })
            );
        }
    }

    #[test]
    fn ci4_palette_is_exactly_four_bits() {
        for value in 0..=0x0f {
            assert_eq!(palette(value).value(), value);
        }
        let error = Ci4Palette::try_new(0x10).unwrap_err();
        assert_eq!(error.value(), 0x10);
        assert!(!error.to_string().is_empty());
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn disabled_ci4_decodes_composite_palette_index_as_i8() {
        let ResolvedIndexedTexel::Direct(decoded) =
            resolve(PixelSize::Bits4, 0x1, 0x2, TextureLutMode::Disabled)
        else {
            panic!("disabled CI4 requested a TLUT entry");
        };
        assert_eq!(decoded.rgba8888(), [0x21; 4]);

        let ResolvedIndexedTexel::Direct(decoded) =
            resolve(PixelSize::Bits4, 0xf, 0xf, TextureLutMode::Disabled)
        else {
            panic!("disabled CI4 requested a TLUT entry");
        };
        assert_eq!(decoded.rgba8888(), [0xff; 4]);
    }

    #[test]
    fn disabled_ci8_ignores_palette_and_decodes_as_i8() {
        let mut first = None;
        for palette_value in [0x0, 0xf] {
            let ResolvedIndexedTexel::Direct(value) = resolve(
                PixelSize::Bits8,
                0x42,
                palette_value,
                TextureLutMode::Disabled,
            ) else {
                panic!("disabled CI8 requested a TLUT entry");
            };
            assert_eq!(value.rgba8888(), [0x42; 4]);
            if let Some(first) = first {
                assert_eq!(value, first);
            } else {
                first = Some(value);
            }
        }
    }

    #[test]
    fn enabled_ci_literal_lookups_and_entries_decode_exactly() {
        let cases = [
            (
                PixelSize::Bits4,
                0x1,
                0x2,
                TextureLutMode::Rgba16,
                0x21,
                0x0908,
                0xf801,
                [0xff, 0x00, 0x00, 0xff],
            ),
            (
                PixelSize::Bits8,
                0xff,
                0xf,
                TextureLutMode::Rgba16,
                0xff,
                0x0ff8,
                0x003f,
                [0x00, 0x00, 0xff, 0xff],
            ),
            (
                PixelSize::Bits4,
                0x5,
                0xa,
                TextureLutMode::Ia16,
                0xa5,
                0x0d28,
                0x8040,
                [0x80, 0x80, 0x80, 0x40],
            ),
            (
                PixelSize::Bits8,
                0x7f,
                0,
                TextureLutMode::Ia16,
                0x7f,
                0x0bf8,
                0x00ff,
                [0x00, 0x00, 0x00, 0xff],
            ),
        ];
        for (size, value, palette_value, mode, index, address, entry, expected) in cases {
            let lookup = lookup(size, value, palette_value, mode);
            assert_eq!(lookup.index(), index);
            assert_eq!(lookup.byte_address(), address);
            assert_eq!(
                decode_tlut_entry(lookup, raw(PixelSize::Bits16, entry))
                    .unwrap()
                    .rgba8888(),
                expected
            );
        }
    }

    #[test]
    fn enabled_modes_cover_both_boundary_indices_for_ci4_and_ci8() {
        for mode in [TextureLutMode::Rgba16, TextureLutMode::Ia16] {
            for (size, value, palette_value, expected_index, expected_address) in [
                (PixelSize::Bits4, 0x0, 0x0, 0x00, 0x0800),
                (PixelSize::Bits4, 0xf, 0xf, 0xff, 0x0ff8),
                (PixelSize::Bits8, 0x00, 0xf, 0x00, 0x0800),
                (PixelSize::Bits8, 0xff, 0x0, 0xff, 0x0ff8),
            ] {
                let lookup = lookup(size, value, palette_value, mode);
                assert_eq!(lookup.index(), expected_index);
                assert_eq!(lookup.byte_address(), expected_address);
            }
        }
    }

    #[test]
    fn identical_entry_bits_have_distinct_rgba16_and_ia16_meanings() {
        let rgba = lookup(PixelSize::Bits8, 0, 0, TextureLutMode::Rgba16);
        let ia = lookup(PixelSize::Bits8, 0, 0, TextureLutMode::Ia16);
        assert_eq!(rgba.entry_format(), ImageFormat::Rgba);
        assert_eq!(ia.entry_format(), ImageFormat::IntensityAlpha);
        let entry = raw(PixelSize::Bits16, 0xf801);
        let rgba = decode_tlut_entry(rgba, entry).unwrap();
        let ia = decode_tlut_entry(ia, entry).unwrap();
        assert_eq!(rgba.rgba8888(), [0xff, 0x00, 0x00, 0xff]);
        assert_eq!(ia.rgba8888(), [0xf8, 0xf8, 0xf8, 0x01]);
        assert_ne!(rgba, ia);
    }

    #[test]
    fn big_endian_entry_mutation_changes_the_decoded_color() {
        let lookup = lookup(PixelSize::Bits8, 0, 0, TextureLutMode::Rgba16);
        let big_endian = decode_tlut_entry(lookup, raw(PixelSize::Bits16, 0xf801)).unwrap();
        let byte_swapped = decode_tlut_entry(lookup, raw(PixelSize::Bits16, 0x01f8)).unwrap();
        assert_eq!(big_endian.rgba8888(), [0xff, 0x00, 0x00, 0xff]);
        assert_ne!(byte_swapped, big_endian);
    }

    #[test]
    fn tlut_entry_rejects_every_non_16_bit_width() {
        let lookup = lookup(PixelSize::Bits8, 0, 0, TextureLutMode::Rgba16);
        for size in [PixelSize::Bits4, PixelSize::Bits8, PixelSize::Bits32] {
            assert_eq!(
                decode_tlut_entry(lookup, raw(size, 0)),
                Err(TlutEntryDecodeError::EntryMustBeBits16 { size })
            );
        }
    }

    #[test]
    fn enabled_mode_cannot_resolve_directly_and_disabled_mode_cannot_request_entry() {
        for mode in [TextureLutMode::Rgba16, TextureLutMode::Ia16] {
            assert!(matches!(
                resolve(PixelSize::Bits8, 0, 0, mode),
                ResolvedIndexedTexel::Tlut(_)
            ));
        }
        assert!(matches!(
            resolve(PixelSize::Bits8, 0, 0, TextureLutMode::Disabled),
            ResolvedIndexedTexel::Direct(_)
        ));
    }

    #[test]
    fn ci8_palette_mutation_cannot_change_lookup() {
        for mode in [TextureLutMode::Rgba16, TextureLutMode::Ia16] {
            assert_eq!(
                lookup(PixelSize::Bits8, 0x42, 0, mode),
                lookup(PixelSize::Bits8, 0x42, 0xf, mode)
            );
        }
    }

    #[test]
    fn indexed_resolution_rejects_ci16_ci32_and_every_non_ci_format() {
        for size in [PixelSize::Bits16, PixelSize::Bits32] {
            assert_eq!(
                resolve_indexed_texel(
                    ImageFormat::ColorIndex,
                    raw(size, 0),
                    palette(0),
                    TextureLutMode::Disabled,
                ),
                Err(IndexedTexelResolveError::UnsupportedIndexSize { size })
            );
        }
        for format in [
            ImageFormat::Rgba,
            ImageFormat::Yuv,
            ImageFormat::IntensityAlpha,
            ImageFormat::Intensity,
        ] {
            assert_eq!(
                resolve_indexed_texel(
                    format,
                    raw(PixelSize::Bits8, 0),
                    palette(0),
                    TextureLutMode::Disabled,
                ),
                Err(IndexedTexelResolveError::FormatMustBeColorIndex { format })
            );
        }
    }

    #[test]
    fn exhaustive_format_size_mode_matrix_has_only_six_legal_cells() {
        let modes = [
            TextureLutMode::Disabled,
            TextureLutMode::Rgba16,
            TextureLutMode::Ia16,
        ];
        for format in ALL_FORMATS {
            for size in ALL_SIZES {
                for mode in modes {
                    let result = resolve_indexed_texel(format, raw(size, 0), palette(0), mode);
                    match (format, size) {
                        (ImageFormat::ColorIndex, PixelSize::Bits4 | PixelSize::Bits8) => {
                            assert!(result.is_ok(), "{format:?}/{size:?}/{mode:?}: {result:?}");
                        }
                        (ImageFormat::ColorIndex, _) => assert_eq!(
                            result,
                            Err(IndexedTexelResolveError::UnsupportedIndexSize { size })
                        ),
                        _ => assert_eq!(
                            result,
                            Err(IndexedTexelResolveError::FormatMustBeColorIndex { format })
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn indexed_error_types_render_and_implement_error() {
        let errors: [&dyn std::error::Error; 3] = [
            &Ci4UnpackError::PackedByteMustBeBits8 {
                size: PixelSize::Bits16,
            },
            &IndexedTexelResolveError::UnsupportedIndexSize {
                size: PixelSize::Bits32,
            },
            &TlutEntryDecodeError::EntryMustBeBits16 {
                size: PixelSize::Bits8,
            },
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
