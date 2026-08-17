//! Typed RDP state staged by one raw-DPC transaction.
//!
//! [`OtherMode`]'s field accessors are a characterization-first, literal port
//! of the already clean-room-admitted `fn64-render-reference`
//! `crates/fn64-render-reference/src/gbi/types.rs` `OtherMode` (bit shifts,
//! field widths, and variant ordering match exactly), cross-checked against
//! the permitted MIT RT64 source pinned by `docs/RT64-PORT-AUTHORITY.md`:
//! `src/shared/rt64_other_mode.h` (field selection: `alphaCompare`,
//! `blenderInputs`, `cycleType`, `combKey`, `cvgDst`, `clrOnCvg`, `cvgXAlpha`,
//! `alphaCvgSel`, `forceBlend`, `textPersp`, `textFilt`, `textLOD`,
//! `textDetail`, `textLUT`, `alphaDither`, `rgbDither`, `aaEn`, `zCmp`,
//! `zUpd`, `zMode`, `zSource`) and
//! `src/shared/rt64_f3d_defines.h` (the `G_MDSFT_*` shift constants and
//! `AA_EN`/`Z_CMP`/`Z_UPD`/`IM_RD`/`CLR_ON_CVG`/`CVG_DST_*`/`ZMODE_*`/
//! `CVG_X_ALPHA`/`ALPHA_CVG_SEL`/`FORCE_BL` masks), pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`. `one_primitive_pipeline` (high
//! bit 23) is not present in RT64's `OtherMode` struct; it is carried over
//! from the reference's public `ultra64/gbi.h` provenance only.
//!
//! ## Corroboration against the pinned RT64 header
//!
//! The cross-checked source is `src/shared/rt64_other_mode.h` at that pinned
//! commit, SHA-256 of the whole file
//! `01096cbd3ff147bba9bdc334d0112e3a1dfa0f09a87b858bac9965bdcf38ca67` (104
//! lines). That digest was computed here with `shasum -a 256` against the
//! pinned checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/shared/rt64_other_mode.h"].sources.port.sha256`, which
//! records the identical digest; the inventory's `sources.oracle.sha256`
//! agrees, so the oracle and port trees hold this file byte for byte alike.
//!
//! **The digest citation is not a transcription claim.** The bit positions,
//! field widths, and encoding values here descend from the admitted
//! reference, whose own provenance is the public SGI SDK header
//! `ultra64/gbi.h` (`gbi.h:497-627` for the field shifts, values, and
//! coverage/Z/blender packing). RT64 is the *second* authority the same facts
//! were checked against, not their origin.
//!
//! A bit-by-bit comparison of the two found **21/21 of the header's accessors
//! agreeing in bit position, width, and mask, with zero divergences** -- two
//! independently derived readings of the same public hardware contract
//! corroborating each other. `blenderInputs` (low 16:31) agrees as a
//! decomposition rather than a single accessor: RT64 returns the whole
//! 16-bit window and `shared/rt64_blender.h` shifts within it, while
//! `blender_cycle_1`/`blender_cycle_2` here pre-split it into the same eight
//! two-bit selectors at the same absolute positions.
//!
//! fn64 is a strict **superset** by three accessors the pinned header has no
//! accessor for:
//! - `image_read_enabled` (low bit 6). `IM_RD 0x40` is *defined* at
//!   `shared/rt64_f3d_defines.h:87` but never read anywhere in the pinned
//!   tree.
//! - `texture_convert` (high bits 9:11). No `G_MDSFT_TEXTCONV` appears
//!   anywhere in the pinned tree.
//! - `one_primitive_pipeline` (high bit 23), whose `ultra64/gbi.h` provenance
//!   is disclosed above.
//!
//! The calling convention differs, losslessly. RT64's multi-bit accessors
//! return the field **masked in place, unshifted**, leaving callers to shift
//! or to compare against pre-shifted constants (`cvgDst() == CVG_DST_WRAP`);
//! `blenderInputs` is the one exception that does shift down. The accessors
//! here shift down and decode to typed enums instead, preserving reserved
//! encodings as distinct variants (`TextureFilter::Reserved`,
//! `AlphaCompare::Reserved`, `TextureLutModeError::ReservedEncoding`) rather
//! than folding them into a neighboring mode. No claim is made about byte
//! layout, `repr(C)`, or ABI compatibility with the C++ struct.
//!
//! This module does not import `fn64-render-reference` (no such crate
//! dependency exists for `fn64-render-wgpu`); it is a self-contained literal
//! re-expression, matching this crate's existing citation-comment convention
//! (see `depth_strict_less.rs`, `src/tmem/sample.rs`).
//!
//! Decode only: no combiner, alpha-compare, coverage, depth, or blender
//! *consumer* wired to these accessors yet. That is future work; this slice
//! is the shared bitfield contract those consumers will read.

use fn64_render_ir::{PhysicalAddress, QueueIdentity};

use crate::combiner::CombineParams;
use crate::tmem::TmemState;

/// Texture lookup-table interpretation selected by `SetOtherModes` high bits
/// 15:14 (`G_MDSFT_TEXTLUT`).
///
/// The encodings follow the permitted MIT RT64 source pinned by
/// `docs/RT64-PORT-AUTHORITY.md` (`shared/rt64_f3d_defines.h` and
/// `shared/rt64_other_mode.h`): zero disables the TLUT, two selects RGBA16,
/// and three selects IA16. Encoding one is reserved and is rejected rather
/// than treated as a disabled table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureLutMode {
    Disabled,
    Rgba16,
    Ia16,
}

/// Why `SetOtherModes`' texture-LUT field could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureLutModeError {
    ReservedEncoding { encoding: u8 },
}

impl core::fmt::Display for TextureLutModeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ReservedEncoding { encoding } => write!(
                formatter,
                "SetOtherModes texture-LUT field uses reserved encoding {encoding}"
            ),
        }
    }
}

impl std::error::Error for TextureLutModeError {}

/// RDP cycle type, other-mode high bits 20:21 (`G_MDSFT_CYCLETYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleType {
    OneCycle,
    TwoCycle,
    Copy,
    Fill,
}

/// RDP texture filter, other-mode high bits 12:13 (`G_MDSFT_TEXTFILT`).
/// Encoding 1 is reserved; RT64 and the admitted reference both surface it
/// as a distinct variant rather than folding it into `Point`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFilter {
    Point,
    Reserved,
    Bilinear,
    Average,
}

/// RGB dither selector, other-mode high bits 6:7 (`G_MDSFT_RGBDITHER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RgbDither {
    MagicSquare,
    Bayer,
    Noise,
    Disabled,
}

/// Alpha dither selector, other-mode high bits 4:5 (`G_MDSFT_ALPHADITHER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaDither {
    Pattern,
    InversePattern,
    Noise,
    Disabled,
}

/// Alpha-compare mode, other-mode low bits 0:1 (`G_MDSFT_ALPHACOMPARE`).
/// Encoding 2 is reserved (`G_AC_NONE=0`, `G_AC_THRESHOLD=1`,
/// `G_AC_DITHER=3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaCompare {
    None,
    Threshold,
    Reserved,
    Dither,
}

/// Coverage destination, other-mode low bits 8:9 (`CVG_DST_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageDestination {
    Clamp,
    Wrap,
    Full,
    Save,
}

/// Z-buffer compositing mode, other-mode low bits 10:11 (`ZMODE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthMode {
    Opaque,
    Interpenetrating,
    Translucent,
    Decal,
}

/// One RDP blender cycle's four two-bit selectors (`B = (P*A + M*B) / (A+B)`
/// packing). Values are kept as raw wire selectors -- resolving them to
/// semantic blend inputs is a future consumer's job, not this decode slice's;
/// this mirrors the admitted reference's `BlenderCycle` (`gbi/types.rs`) and
/// RT64's `blenderInputs()` 16-bit low-word slice
/// (`shared/rt64_other_mode.h:22-24`), which these four fields exactly cover:
/// low-word bits 16:31, interleaved two bits at a time between the cycles --
/// cycle 1 reads {30:31, 26:27, 22:23, 18:19} as `color_a, alpha_a, color_b,
/// alpha_b`; cycle 2 reads the same shape shifted down two bits, {28:29,
/// 24:25, 20:21, 16:17}.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlenderCycle {
    pub color_a: u8,
    pub alpha_a: u8,
    pub color_b: u8,
    pub alpha_b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtherMode {
    high: u32,
    low: u32,
}

impl OtherMode {
    pub(crate) const fn from_wire(high: u32, low: u32) -> Self {
        Self { high, low }
    }

    pub const fn high(self) -> u32 {
        self.high
    }

    pub const fn low(self) -> u32 {
        self.low
    }

    pub const fn cycle_type(self) -> CycleType {
        match (self.high >> 20) & 0x3 {
            0 => CycleType::OneCycle,
            1 => CycleType::TwoCycle,
            2 => CycleType::Copy,
            _ => CycleType::Fill,
        }
    }

    /// Decodes the two-bit texture-LUT selector without normalizing its
    /// reserved encoding into a supported mode.
    pub const fn texture_lut_mode(self) -> Result<TextureLutMode, TextureLutModeError> {
        let encoding = ((self.high >> 14) & 0x3) as u8;
        match encoding {
            0 => Ok(TextureLutMode::Disabled),
            2 => Ok(TextureLutMode::Rgba16),
            3 => Ok(TextureLutMode::Ia16),
            _ => Err(TextureLutModeError::ReservedEncoding { encoding }),
        }
    }

    /// Other-mode high bits 12:13 (`G_MDSFT_TEXTFILT`).
    pub const fn texture_filter(self) -> TextureFilter {
        match (self.high >> 12) & 0x3 {
            0 => TextureFilter::Point,
            1 => TextureFilter::Reserved,
            2 => TextureFilter::Bilinear,
            _ => TextureFilter::Average,
        }
    }

    /// Other-mode high bits 6:7 (`G_MDSFT_RGBDITHER`).
    pub const fn rgb_dither(self) -> RgbDither {
        match (self.high >> 6) & 0x3 {
            0 => RgbDither::MagicSquare,
            1 => RgbDither::Bayer,
            2 => RgbDither::Noise,
            _ => RgbDither::Disabled,
        }
    }

    /// Other-mode high bits 4:5 (`G_MDSFT_ALPHADITHER`).
    pub const fn alpha_dither(self) -> AlphaDither {
        match (self.high >> 4) & 0x3 {
            0 => AlphaDither::Pattern,
            1 => AlphaDither::InversePattern,
            2 => AlphaDither::Noise,
            _ => AlphaDither::Disabled,
        }
    }

    /// Other-mode high bit 8 (`G_MDSFT_COMBKEY`): chroma-key enable consumed
    /// by the color combiner's `KeyCenter`/`KeyScale` inputs.
    pub const fn combine_key(self) -> bool {
        self.high & (1 << 8) != 0
    }

    /// Other-mode high bits 9:11 (`G_MDSFT_TEXTCONV`). Kept as a raw 3-bit
    /// selector -- resolving it to a semantic texel-conversion mode is a
    /// future texture-sampling consumer's job, matching the admitted
    /// reference's `texture_convert`.
    pub const fn texture_convert(self) -> u8 {
        ((self.high >> 9) & 0x7) as u8
    }

    /// Other-mode high bit 16 (`G_MDSFT_TEXTLOD`).
    pub const fn texture_lod(self) -> bool {
        self.high & (1 << 16) != 0
    }

    /// Other-mode high bits 17:18 (`G_MDSFT_TEXTDETAIL`).
    pub const fn texture_detail(self) -> u8 {
        ((self.high >> 17) & 0x3) as u8
    }

    /// Other-mode high bit 19 (`G_MDSFT_TEXTPERSP`).
    pub const fn texture_perspective(self) -> bool {
        self.high & (1 << 19) != 0
    }

    /// Other-mode high bit 23. Not present in RT64's `OtherMode` struct;
    /// carried over from the admitted reference's public `ultra64/gbi.h`
    /// provenance only (see module doc).
    pub const fn one_primitive_pipeline(self) -> bool {
        self.high & (1 << 23) != 0
    }

    /// Other-mode low bits 0:1 (`G_MDSFT_ALPHACOMPARE`). Encoding 2 is
    /// reserved and is surfaced as `AlphaCompare::Reserved` rather than
    /// normalized into `None` or `Threshold`.
    pub const fn alpha_compare(self) -> AlphaCompare {
        match self.low & 0x3 {
            0 => AlphaCompare::None,
            1 => AlphaCompare::Threshold,
            2 => AlphaCompare::Reserved,
            _ => AlphaCompare::Dither,
        }
    }

    /// Other-mode low bit 2 (`G_MDSFT_ZSRCSEL` / RT64 `zSource()`): `true`
    /// selects the primitive-supplied depth (`gDPSetPrimDepth`) over the
    /// per-pixel rasterized depth.
    pub const fn primitive_depth_source(self) -> bool {
        self.low & (1 << 2) != 0
    }

    /// Other-mode low bit 3 (`AA_EN`).
    pub const fn antialias_enabled(self) -> bool {
        self.low & 0x0008 != 0
    }

    /// Other-mode low bit 4 (`Z_CMP`).
    pub const fn depth_compare_enabled(self) -> bool {
        self.low & 0x0010 != 0
    }

    /// Other-mode low bit 5 (`Z_UPD`).
    pub const fn depth_update_enabled(self) -> bool {
        self.low & 0x0020 != 0
    }

    /// Other-mode low bit 6 (`IM_RD`).
    pub const fn image_read_enabled(self) -> bool {
        self.low & 0x0040 != 0
    }

    /// Other-mode low bit 7 (`CLR_ON_CVG`).
    pub const fn clear_on_coverage(self) -> bool {
        self.low & 0x0080 != 0
    }

    /// Other-mode low bits 8:9 (`CVG_DST_*`).
    pub const fn coverage_destination(self) -> CoverageDestination {
        match (self.low >> 8) & 0x3 {
            0 => CoverageDestination::Clamp,
            1 => CoverageDestination::Wrap,
            2 => CoverageDestination::Full,
            _ => CoverageDestination::Save,
        }
    }

    /// Other-mode low bits 10:11 (`ZMODE_*`).
    pub const fn depth_mode(self) -> DepthMode {
        match (self.low >> 10) & 0x3 {
            0 => DepthMode::Opaque,
            1 => DepthMode::Interpenetrating,
            2 => DepthMode::Translucent,
            _ => DepthMode::Decal,
        }
    }

    /// Other-mode low bit 12 (`CVG_X_ALPHA`).
    pub const fn coverage_times_alpha(self) -> bool {
        self.low & 0x1000 != 0
    }

    /// Other-mode low bit 13 (`ALPHA_CVG_SEL`).
    pub const fn alpha_coverage_select(self) -> bool {
        self.low & 0x2000 != 0
    }

    /// Other-mode low bit 14 (`FORCE_BL`).
    pub const fn force_blend(self) -> bool {
        self.low & 0x4000 != 0
    }

    /// Other-mode low bits 30:31 (color A), 26:27 (alpha A), 22:23 (color
    /// B), 18:19 (alpha B): cycle 1's four blender selectors.
    pub const fn blender_cycle_1(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 30) & 0x3) as u8,
            alpha_a: ((self.low >> 26) & 0x3) as u8,
            color_b: ((self.low >> 22) & 0x3) as u8,
            alpha_b: ((self.low >> 18) & 0x3) as u8,
        }
    }

    /// Other-mode low bits 28:29 (color A), 24:25 (alpha A), 20:21 (color
    /// B), 16:17 (alpha B): cycle 2's four blender selectors.
    pub const fn blender_cycle_2(self) -> BlenderCycle {
        BlenderCycle {
            color_a: ((self.low >> 28) & 0x3) as u8,
            alpha_a: ((self.low >> 24) & 0x3) as u8,
            color_b: ((self.low >> 20) & 0x3) as u8,
            alpha_b: ((self.low >> 16) & 0x3) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Rgba,
    Yuv,
    ColorIndex,
    IntensityAlpha,
    Intensity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelSize {
    Bits4,
    Bits8,
    Bits16,
    Bits32,
}

impl PixelSize {
    pub const fn bytes_per_pixel(self) -> Option<u32> {
        match self {
            Self::Bits4 => None,
            Self::Bits8 => Some(1),
            Self::Bits16 => Some(2),
            Self::Bits32 => Some(4),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorImage {
    format: ImageFormat,
    size: PixelSize,
    width: u32,
    address: PhysicalAddress,
}

impl ColorImage {
    pub(crate) const fn from_wire(
        format: ImageFormat,
        size: PixelSize,
        width: u32,
        address: PhysicalAddress,
    ) -> Self {
        Self {
            format,
            size,
            width,
            address,
        }
    }

    pub const fn format(self) -> ImageFormat {
        self.format
    }

    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn address(self) -> PhysicalAddress {
        self.address
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillColor(u32);

impl FillColor {
    pub(crate) const fn from_wire(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    pub const fn rgba32(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

/// A fragment constant-register RGBA color, decoded from one raw 32-bit wire
/// word exactly as RT64's `setEnvColor`/`setPrimColor`/`setBlendColor`/
/// `setFogColor` do (`src/hle/rt64_rdp.cpp:837-932`, pinned commit
/// `5473732a822a4423b5696e7cb18fecc425a59875`): byte 3 (bits 31:24) is red,
/// byte 2 (bits 23:16) is green, byte 1 (bits 15:8) is blue, byte 0 (bits
/// 7:0) is alpha -- the same big-endian RGBA8888 packing this crate's
/// existing `FillColor` already uses. RT64 additionally normalizes each byte
/// to `[0.0, 1.0]` by dividing by `255.0`; `normalized()` reproduces that
/// exact division while `rgba8()`/`value()` retain the raw byte and word
/// forms so the wire content stays mechanically auditable alongside the
/// derived float.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color4(u32);

impl Color4 {
    pub(crate) const fn from_wire(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// `[red, green, blue, alpha]`, matching `FillColor::rgba32`'s byte
    /// order.
    pub const fn rgba8(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// `[red, green, blue, alpha]` each divided by `255.0`, matching RT64's
    /// `setEnvColor`/`setPrimColor`/`setBlendColor`/`setFogColor` exactly
    /// (`color.x/y/z/w = ((color >> shift) & 0xFF) / 255.0f`).
    pub fn normalized(self) -> [f32; 4] {
        let [r, g, b, a] = self.rgba8();
        [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            f32::from(a) / 255.0,
        ]
    }
}

/// `SetPrimColor`'s minimum-LOD and primitive-LOD-fraction bytes, decoded
/// exactly as RT64's `GBI_RDP::setPrimColor` extracts them from the
/// command's first wire word (`src/gbi/rt64_gbi_rdp.cpp:100-106`, pinned
/// commit `5473732a822a4423b5696e7cb18fecc425a59875`):
///
/// ```text
/// lodFrac = w0 bits 0:7   (p0(0, 8), full byte)
/// lodMin  = w0 bits 8:12  (p0(8, 5), only 5 of the public 8 documented bits)
/// ```
///
/// RT64's own comment at that site: "While the manual states that lodMin has
/// 8 bits of precision, the RDP only uses 5 of them" -- so `lod_min()` is
/// deliberately a 5-bit narrow type (`0..=31`), not `u8`, and decode masks to
/// exactly those 5 bits rather than the full byte the command word reserves.
/// `RDP::setPrimColor` (`src/hle/rt64_rdp.cpp:860-871`) then normalizes:
/// `lodFrac / 256.0f`, `lodMin / 32.0f`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimLod {
    lod_frac: u8,
    lod_min: u8,
}

impl PrimLod {
    pub(crate) const fn from_wire(w0: u32) -> Self {
        Self {
            lod_frac: (w0 & 0xff) as u8,
            lod_min: ((w0 >> 8) & 0x1f) as u8,
        }
    }

    /// Raw primitive-LOD-fraction byte (`w0` bits 0:7, the full byte).
    pub const fn lod_frac(self) -> u8 {
        self.lod_frac
    }

    /// Raw minimum-LOD value, already masked to the 5 bits the RDP consults
    /// (`w0` bits 8:12) -- never the full 8-bit field the public command
    /// layout reserves.
    pub const fn lod_min(self) -> u8 {
        self.lod_min
    }

    /// `lod_frac() / 256.0`, matching RT64's `primLOD.x = lodFrac / 256.0f`.
    pub fn lod_frac_normalized(self) -> f32 {
        f32::from(self.lod_frac) / 256.0
    }

    /// `lod_min() / 32.0`, matching RT64's `primLOD.y = lodMin / 32.0f`.
    pub fn lod_min_normalized(self) -> f32 {
        f32::from(self.lod_min) / 32.0
    }
}

/// `SetPrimColor`'s complete decoded payload: RT64 stages this command's
/// `lodFrac`/`lodMin`/`color` fields together in one call
/// (`RDP::setPrimColor(uint8_t lodFrac, uint8_t lodMin, uint32_t color)`,
/// `src/hle/rt64_rdp.cpp:860`), so this type keeps them together rather than
/// splitting them into two independently-staged registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimColor {
    lod: PrimLod,
    color: Color4,
}

impl PrimColor {
    pub(crate) const fn from_wire(w0: u32, w1: u32) -> Self {
        Self {
            lod: PrimLod::from_wire(w0),
            color: Color4::from_wire(w1),
        }
    }

    pub const fn lod(self) -> PrimLod {
        self.lod
    }

    pub const fn color(self) -> Color4 {
        self.color
    }
}

/// `SetPrimDepth`'s primitive depth and delta-Z, decoded and normalized
/// exactly as RT64's `RDP::setPrimDepth` (`src/hle/rt64_rdp.cpp:961-968`,
/// pinned commit `5473732a822a4423b5696e7cb18fecc425a59875`):
///
/// ```text
/// primDepth.x = (z  & 0x7FFFU) * (1.0f / 32767.0f)   // NOTE: 15-bit mask
/// primDepth.y = (dz & 0xFFFFU) * (1.0f / 65535.0f)   // full 16-bit mask
/// ```
///
/// The `z` mask is deliberately `0x7FFF` (15 bits), not `0xFFFF` -- RT64
/// discards the wire word's top bit rather than treating it as part of the
/// depth value, and the normalizing divisor `32767.0` (`2^15 - 1`) matches
/// that narrower domain exactly. `dz` uses the full 16-bit mask and divisor
/// `65535.0` (`2^16 - 1`). Both raw fields are retained unmasked-input-proof:
/// `z()`/`dz()` return the already-masked values actually consulted, so a
/// caller inspecting this type sees precisely what RT64 saw, not the
/// command's raw, possibly-hostile wire bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrimDepth {
    z: u16,
    dz: u16,
}

impl PrimDepth {
    pub(crate) const fn from_wire(w1: u32) -> Self {
        let raw_z = (w1 >> 16) as u16;
        let raw_dz = w1 as u16;
        Self {
            z: raw_z & 0x7fff,
            dz: raw_dz,
        }
    }

    /// The masked 15-bit primitive depth (`w1` bits 16:30; bit 31 is
    /// discarded, matching RT64's `z & 0x7FFFU`).
    pub const fn z(self) -> u16 {
        self.z
    }

    /// The full 16-bit delta-Z (`w1` bits 0:15).
    pub const fn dz(self) -> u16 {
        self.dz
    }

    /// `z() / 32767.0`, matching RT64's `(z & 0x7FFFU) * (1.0f / 32767.0f)`.
    pub fn z_normalized(self) -> f32 {
        f32::from(self.z) / 32767.0
    }

    /// `dz() / 65535.0`, matching RT64's `(dz & 0xFFFFU) * (1.0f / 65535.0f)`.
    pub fn dz_normalized(self) -> f32 {
        f32::from(self.dz) / 65535.0
    }
}

/// Durable renderer state is immutable to the decoder. A caller can publish a
/// staged result only at a later owner-controlled commit boundary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RdpState {
    other_mode: Option<OtherMode>,
    color_image: Option<ColorImage>,
    fill_color: Option<FillColor>,
    env_color: Option<Color4>,
    prim_color: Option<PrimColor>,
    blend_color: Option<Color4>,
    fog_color: Option<Color4>,
    prim_depth: Option<PrimDepth>,
    combine: Option<CombineParams>,
    tmem: TmemState,
}

impl RdpState {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.other_mode
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.color_image
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.fill_color
    }

    pub const fn env_color(&self) -> Option<Color4> {
        self.env_color
    }

    pub const fn prim_color(&self) -> Option<PrimColor> {
        self.prim_color
    }

    pub const fn blend_color(&self) -> Option<Color4> {
        self.blend_color
    }

    pub const fn fog_color(&self) -> Option<Color4> {
        self.fog_color
    }

    pub const fn prim_depth(&self) -> Option<PrimDepth> {
        self.prim_depth
    }

    pub const fn combine(&self) -> Option<CombineParams> {
        self.combine
    }

    pub const fn tmem(&self) -> &TmemState {
        &self.tmem
    }

    pub(crate) fn tmem_mut(&mut self) -> &mut TmemState {
        &mut self.tmem
    }

    pub(crate) fn fork_for_decode(&self) -> Self {
        Self {
            other_mode: self.other_mode,
            color_image: self.color_image,
            fill_color: self.fill_color,
            env_color: self.env_color,
            prim_color: self.prim_color,
            blend_color: self.blend_color,
            fog_color: self.fog_color,
            prim_depth: self.prim_depth,
            combine: self.combine,
            tmem: self.tmem.clone(),
        }
    }

    pub(crate) fn apply(&mut self, delta: &RdpStateDelta) {
        if let Some(value) = delta.other_mode {
            self.other_mode = Some(value);
        }
        if let Some(value) = delta.color_image {
            self.color_image = Some(value);
        }
        if let Some(value) = delta.fill_color {
            self.fill_color = Some(value);
        }
        if let Some(value) = delta.env_color {
            self.env_color = Some(value);
        }
        if let Some(value) = delta.prim_color {
            self.prim_color = Some(value);
        }
        if let Some(value) = delta.blend_color {
            self.blend_color = Some(value);
        }
        if let Some(value) = delta.fog_color {
            self.fog_color = Some(value);
        }
        if let Some(value) = delta.prim_depth {
            self.prim_depth = Some(value);
        }
        if let Some(value) = delta.combine {
            self.combine = Some(value);
        }
        if let Some(value) = &delta.tmem {
            self.tmem = value.clone();
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RdpStateDelta {
    other_mode: Option<OtherMode>,
    color_image: Option<ColorImage>,
    fill_color: Option<FillColor>,
    env_color: Option<Color4>,
    prim_color: Option<PrimColor>,
    blend_color: Option<Color4>,
    fog_color: Option<Color4>,
    prim_depth: Option<PrimDepth>,
    combine: Option<CombineParams>,
    tmem: Option<TmemState>,
}

impl RdpStateDelta {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.other_mode
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.color_image
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.fill_color
    }

    pub const fn env_color(&self) -> Option<Color4> {
        self.env_color
    }

    pub const fn prim_color(&self) -> Option<PrimColor> {
        self.prim_color
    }

    pub const fn blend_color(&self) -> Option<Color4> {
        self.blend_color
    }

    pub const fn fog_color(&self) -> Option<Color4> {
        self.fog_color
    }

    pub const fn prim_depth(&self) -> Option<PrimDepth> {
        self.prim_depth
    }

    pub const fn combine(&self) -> Option<CombineParams> {
        self.combine
    }

    pub const fn tmem(&self) -> Option<&TmemState> {
        self.tmem.as_ref()
    }

    pub(crate) fn set_other_mode(&mut self, value: OtherMode) {
        self.other_mode = Some(value);
    }

    pub(crate) fn set_color_image(&mut self, value: ColorImage) {
        self.color_image = Some(value);
    }

    pub(crate) fn set_fill_color(&mut self, value: FillColor) {
        self.fill_color = Some(value);
    }

    pub(crate) fn set_env_color(&mut self, value: Color4) {
        self.env_color = Some(value);
    }

    pub(crate) fn set_prim_color(&mut self, value: PrimColor) {
        self.prim_color = Some(value);
    }

    pub(crate) fn set_blend_color(&mut self, value: Color4) {
        self.blend_color = Some(value);
    }

    pub(crate) fn set_fog_color(&mut self, value: Color4) {
        self.fog_color = Some(value);
    }

    pub(crate) fn set_prim_depth(&mut self, value: PrimDepth) {
        self.prim_depth = Some(value);
    }

    pub(crate) fn set_combine(&mut self, value: CombineParams) {
        self.combine = Some(value);
    }

    pub(crate) fn set_tmem(&mut self, value: TmemState) {
        self.tmem = Some(value);
    }
}

/// Transaction-local state. Its distinct type makes cross-packet chaining an
/// explicit choice and prevents decode from masquerading as durable commit.
#[derive(Debug, PartialEq, Eq)]
pub struct StagedRdpState {
    state: RdpState,
    queue: QueueIdentity,
    submission_ordinal: u64,
    transaction_sequence: u64,
}

impl StagedRdpState {
    pub const fn other_mode(&self) -> Option<OtherMode> {
        self.state.other_mode()
    }

    pub const fn color_image(&self) -> Option<ColorImage> {
        self.state.color_image()
    }

    pub const fn fill_color(&self) -> Option<FillColor> {
        self.state.fill_color()
    }

    pub const fn env_color(&self) -> Option<Color4> {
        self.state.env_color()
    }

    pub const fn prim_color(&self) -> Option<PrimColor> {
        self.state.prim_color()
    }

    pub const fn blend_color(&self) -> Option<Color4> {
        self.state.blend_color()
    }

    pub const fn fog_color(&self) -> Option<Color4> {
        self.state.fog_color()
    }

    pub const fn prim_depth(&self) -> Option<PrimDepth> {
        self.state.prim_depth()
    }

    pub const fn combine(&self) -> Option<CombineParams> {
        self.state.combine()
    }

    pub const fn tmem(&self) -> &TmemState {
        self.state.tmem()
    }

    pub const fn queue(&self) -> QueueIdentity {
        self.queue
    }

    pub const fn transaction_sequence(&self) -> u64 {
        self.transaction_sequence
    }

    pub const fn submission_ordinal(&self) -> u64 {
        self.submission_ordinal
    }

    pub(crate) fn from_transaction(
        state: RdpState,
        queue: QueueIdentity,
        submission_ordinal: u64,
        transaction_sequence: u64,
    ) -> Self {
        Self {
            state,
            queue,
            submission_ordinal,
            transaction_sequence,
        }
    }

    pub(crate) fn into_parts(self) -> (RdpState, QueueIdentity, u64, u64) {
        (
            self.state,
            self.queue,
            self.submission_ordinal,
            self.transaction_sequence,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_lut_mode_decodes_all_four_wire_encodings_without_normalization() {
        assert_eq!(
            OtherMode::from_wire(0 << 14, u32::MAX).texture_lut_mode(),
            Ok(TextureLutMode::Disabled)
        );
        assert_eq!(
            OtherMode::from_wire(1 << 14, 0).texture_lut_mode(),
            Err(TextureLutModeError::ReservedEncoding { encoding: 1 })
        );
        assert_eq!(
            OtherMode::from_wire(2 << 14, 0).texture_lut_mode(),
            Ok(TextureLutMode::Rgba16)
        );
        assert_eq!(
            OtherMode::from_wire(3 << 14, 0).texture_lut_mode(),
            Ok(TextureLutMode::Ia16)
        );
    }

    #[test]
    fn texture_lut_mode_ignores_unrelated_other_mode_bits() {
        let high = 0x00ff_ffff & !(0x3 << 14);
        assert_eq!(
            OtherMode::from_wire(high | (2 << 14), u32::MAX).texture_lut_mode(),
            Ok(TextureLutMode::Rgba16)
        );
    }

    #[test]
    fn reserved_texture_lut_encoding_is_a_public_typed_error() {
        let error = OtherMode::from_wire(1 << 14, 0)
            .texture_lut_mode()
            .unwrap_err();
        assert_eq!(error, TextureLutModeError::ReservedEncoding { encoding: 1 });
        assert!(!error.to_string().is_empty());
        let _: &dyn std::error::Error = &error;
    }

    #[test]
    fn cycle_type_decodes_all_four_wire_encodings() {
        assert_eq!(
            OtherMode::from_wire(0 << 20, 0).cycle_type(),
            CycleType::OneCycle
        );
        assert_eq!(
            OtherMode::from_wire(1 << 20, 0).cycle_type(),
            CycleType::TwoCycle
        );
        assert_eq!(
            OtherMode::from_wire(2 << 20, 0).cycle_type(),
            CycleType::Copy
        );
        assert_eq!(
            OtherMode::from_wire(3 << 20, 0).cycle_type(),
            CycleType::Fill
        );
    }

    #[test]
    fn texture_filter_decodes_all_four_wire_encodings_including_reserved() {
        assert_eq!(
            OtherMode::from_wire(0 << 12, 0).texture_filter(),
            TextureFilter::Point
        );
        assert_eq!(
            OtherMode::from_wire(1 << 12, 0).texture_filter(),
            TextureFilter::Reserved
        );
        assert_eq!(
            OtherMode::from_wire(2 << 12, 0).texture_filter(),
            TextureFilter::Bilinear
        );
        assert_eq!(
            OtherMode::from_wire(3 << 12, 0).texture_filter(),
            TextureFilter::Average
        );
    }

    #[test]
    fn rgb_dither_decodes_all_four_wire_encodings() {
        assert_eq!(
            OtherMode::from_wire(0 << 6, 0).rgb_dither(),
            RgbDither::MagicSquare
        );
        assert_eq!(
            OtherMode::from_wire(1 << 6, 0).rgb_dither(),
            RgbDither::Bayer
        );
        assert_eq!(
            OtherMode::from_wire(2 << 6, 0).rgb_dither(),
            RgbDither::Noise
        );
        assert_eq!(
            OtherMode::from_wire(3 << 6, 0).rgb_dither(),
            RgbDither::Disabled
        );
    }

    #[test]
    fn alpha_dither_decodes_all_four_wire_encodings() {
        assert_eq!(
            OtherMode::from_wire(0 << 4, 0).alpha_dither(),
            AlphaDither::Pattern
        );
        assert_eq!(
            OtherMode::from_wire(1 << 4, 0).alpha_dither(),
            AlphaDither::InversePattern
        );
        assert_eq!(
            OtherMode::from_wire(2 << 4, 0).alpha_dither(),
            AlphaDither::Noise
        );
        assert_eq!(
            OtherMode::from_wire(3 << 4, 0).alpha_dither(),
            AlphaDither::Disabled
        );
    }

    #[test]
    fn combine_key_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).combine_key());
        assert!(OtherMode::from_wire(1 << 8, 0).combine_key());
        assert!(!OtherMode::from_wire(!(1 << 8), 0).combine_key());
    }

    #[test]
    fn texture_convert_decodes_all_eight_wire_encodings() {
        for value in 0u32..8 {
            assert_eq!(
                OtherMode::from_wire(value << 9, 0).texture_convert(),
                value as u8
            );
        }
    }

    #[test]
    fn texture_lod_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).texture_lod());
        assert!(OtherMode::from_wire(1 << 16, 0).texture_lod());
        assert!(!OtherMode::from_wire(!(1 << 16), 0).texture_lod());
    }

    #[test]
    fn texture_detail_decodes_all_four_wire_encodings() {
        for value in 0u32..4 {
            assert_eq!(
                OtherMode::from_wire(value << 17, 0).texture_detail(),
                value as u8
            );
        }
    }

    #[test]
    fn texture_perspective_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).texture_perspective());
        assert!(OtherMode::from_wire(1 << 19, 0).texture_perspective());
        assert!(!OtherMode::from_wire(!(1 << 19), 0).texture_perspective());
    }

    #[test]
    fn one_primitive_pipeline_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).one_primitive_pipeline());
        assert!(OtherMode::from_wire(1 << 23, 0).one_primitive_pipeline());
        assert!(!OtherMode::from_wire(!(1 << 23), 0).one_primitive_pipeline());
    }

    #[test]
    fn alpha_compare_decodes_all_four_wire_encodings_including_reserved() {
        assert_eq!(
            OtherMode::from_wire(0, 0).alpha_compare(),
            AlphaCompare::None
        );
        assert_eq!(
            OtherMode::from_wire(0, 1).alpha_compare(),
            AlphaCompare::Threshold
        );
        assert_eq!(
            OtherMode::from_wire(0, 2).alpha_compare(),
            AlphaCompare::Reserved
        );
        assert_eq!(
            OtherMode::from_wire(0, 3).alpha_compare(),
            AlphaCompare::Dither
        );
    }

    #[test]
    fn primitive_depth_source_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).primitive_depth_source());
        assert!(OtherMode::from_wire(0, 1 << 2).primitive_depth_source());
        assert!(!OtherMode::from_wire(0, !(1 << 2)).primitive_depth_source());
    }

    #[test]
    fn antialias_enabled_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).antialias_enabled());
        assert!(OtherMode::from_wire(0, 0x0008).antialias_enabled());
        assert!(!OtherMode::from_wire(0, !0x0008).antialias_enabled());
    }

    #[test]
    fn depth_compare_enabled_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).depth_compare_enabled());
        assert!(OtherMode::from_wire(0, 0x0010).depth_compare_enabled());
        assert!(!OtherMode::from_wire(0, !0x0010).depth_compare_enabled());
    }

    #[test]
    fn depth_update_enabled_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).depth_update_enabled());
        assert!(OtherMode::from_wire(0, 0x0020).depth_update_enabled());
        assert!(!OtherMode::from_wire(0, !0x0020).depth_update_enabled());
    }

    #[test]
    fn image_read_enabled_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).image_read_enabled());
        assert!(OtherMode::from_wire(0, 0x0040).image_read_enabled());
        assert!(!OtherMode::from_wire(0, !0x0040).image_read_enabled());
    }

    #[test]
    fn clear_on_coverage_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).clear_on_coverage());
        assert!(OtherMode::from_wire(0, 0x0080).clear_on_coverage());
        assert!(!OtherMode::from_wire(0, !0x0080).clear_on_coverage());
    }

    #[test]
    fn coverage_destination_decodes_all_four_wire_encodings() {
        assert_eq!(
            OtherMode::from_wire(0, 0 << 8).coverage_destination(),
            CoverageDestination::Clamp
        );
        assert_eq!(
            OtherMode::from_wire(0, 1 << 8).coverage_destination(),
            CoverageDestination::Wrap
        );
        assert_eq!(
            OtherMode::from_wire(0, 2 << 8).coverage_destination(),
            CoverageDestination::Full
        );
        assert_eq!(
            OtherMode::from_wire(0, 3 << 8).coverage_destination(),
            CoverageDestination::Save
        );
    }

    #[test]
    fn depth_mode_decodes_all_four_wire_encodings() {
        assert_eq!(
            OtherMode::from_wire(0, 0 << 10).depth_mode(),
            DepthMode::Opaque
        );
        assert_eq!(
            OtherMode::from_wire(0, 1 << 10).depth_mode(),
            DepthMode::Interpenetrating
        );
        assert_eq!(
            OtherMode::from_wire(0, 2 << 10).depth_mode(),
            DepthMode::Translucent
        );
        assert_eq!(
            OtherMode::from_wire(0, 3 << 10).depth_mode(),
            DepthMode::Decal
        );
    }

    #[test]
    fn coverage_times_alpha_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).coverage_times_alpha());
        assert!(OtherMode::from_wire(0, 0x1000).coverage_times_alpha());
        assert!(!OtherMode::from_wire(0, !0x1000).coverage_times_alpha());
    }

    #[test]
    fn alpha_coverage_select_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).alpha_coverage_select());
        assert!(OtherMode::from_wire(0, 0x2000).alpha_coverage_select());
        assert!(!OtherMode::from_wire(0, !0x2000).alpha_coverage_select());
    }

    #[test]
    fn force_blend_is_a_single_isolated_bit() {
        assert!(!OtherMode::from_wire(0, 0).force_blend());
        assert!(OtherMode::from_wire(0, 0x4000).force_blend());
        assert!(!OtherMode::from_wire(0, !0x4000).force_blend());
    }

    #[test]
    fn blender_cycle_1_reads_bits_18_through_31() {
        let low = (0b01 << 30) | (0b10 << 26) | (0b11 << 22) | (0b01 << 18);
        assert_eq!(
            OtherMode::from_wire(0, low).blender_cycle_1(),
            BlenderCycle {
                color_a: 0b01,
                alpha_a: 0b10,
                color_b: 0b11,
                alpha_b: 0b01,
            }
        );
    }

    #[test]
    fn blender_cycle_2_reads_bits_16_through_29() {
        let low = (0b10 << 28) | (0b01 << 24) | (0b11 << 20) | (0b10 << 16);
        assert_eq!(
            OtherMode::from_wire(0, low).blender_cycle_2(),
            BlenderCycle {
                color_a: 0b10,
                alpha_a: 0b01,
                color_b: 0b11,
                alpha_b: 0b10,
            }
        );
    }

    #[test]
    fn blender_cycle_1_and_2_read_disjoint_non_overlapping_bit_ranges() {
        // cycle 1 and cycle 2 interleave: cycle_1 reads bits {30:31, 26:27,
        // 22:23, 18:19}, cycle_2 reads the shifted-down set {28:29, 24:25,
        // 20:21, 16:17}. Setting only bits 30:31 (cycle_1's color_a, not one
        // of cycle_2's bits) must leave every cycle-2 field at zero.
        let low = 0b11 << 30;
        assert_eq!(
            OtherMode::from_wire(0, low).blender_cycle_1(),
            BlenderCycle {
                color_a: 0b11,
                alpha_a: 0,
                color_b: 0,
                alpha_b: 0,
            }
        );
        assert_eq!(
            OtherMode::from_wire(0, low).blender_cycle_2(),
            BlenderCycle {
                color_a: 0,
                alpha_a: 0,
                color_b: 0,
                alpha_b: 0,
            }
        );
    }

    #[test]
    fn all_high_field_accessors_ignore_bits_outside_their_own_field() {
        // Set every high bit except the field under test, and confirm each
        // accessor still reports its zero encoding -- a hostile check that
        // no accessor accidentally reads a neighboring field's bits.
        assert_eq!(
            OtherMode::from_wire(!(0x3 << 20), 0).cycle_type(),
            CycleType::OneCycle
        );
        assert_eq!(
            OtherMode::from_wire(!(0x3 << 12), 0).texture_filter(),
            TextureFilter::Point
        );
        assert_eq!(
            OtherMode::from_wire(!(0x3 << 6), 0).rgb_dither(),
            RgbDither::MagicSquare
        );
        assert_eq!(
            OtherMode::from_wire(!(0x3 << 4), 0).alpha_dither(),
            AlphaDither::Pattern
        );
        assert_eq!(OtherMode::from_wire(!(0x7 << 9), 0).texture_convert(), 0);
        assert_eq!(OtherMode::from_wire(!(0x3 << 17), 0).texture_detail(), 0);
        assert_eq!(
            OtherMode::from_wire(!(0x3 << 14), 0).texture_lut_mode(),
            Ok(TextureLutMode::Disabled)
        );
    }

    #[test]
    fn all_low_field_accessors_ignore_bits_outside_their_own_field() {
        assert_eq!(
            OtherMode::from_wire(0, !0x3).alpha_compare(),
            AlphaCompare::None
        );
        assert_eq!(
            OtherMode::from_wire(0, !(0x3 << 8)).coverage_destination(),
            CoverageDestination::Clamp
        );
        assert_eq!(
            OtherMode::from_wire(0, !(0x3 << 10)).depth_mode(),
            DepthMode::Opaque
        );
        // blender_cycle_1 reads bits 30:31, 26:27, 22:23, 18:19 (its own
        // four selectors, interleaved with cycle_2's bits at 28:29, 24:25,
        // 20:21, 16:17); clear exactly cycle_1's bits and confirm every
        // field reads zero, proving the accessor reads none of cycle_2's
        // interleaved bits.
        assert_eq!(
            OtherMode::from_wire(0, !0xcccc_0000).blender_cycle_1(),
            BlenderCycle {
                color_a: 0,
                alpha_a: 0,
                color_b: 0,
                alpha_b: 0,
            }
        );
        // blender_cycle_2 reads bits 28:29, 24:25, 20:21, 16:17.
        assert_eq!(
            OtherMode::from_wire(0, !0x3333_0000).blender_cycle_2(),
            BlenderCycle {
                color_a: 0,
                alpha_a: 0,
                color_b: 0,
                alpha_b: 0,
            }
        );
    }

    #[test]
    fn default_other_mode_matches_reference_reset_state() {
        // RT64's F3DEX2 reset state, same value the admitted reference's
        // `Default for OtherMode` documents at `hle/rt64_rsp.cpp:88-89`:
        // high=0x0008_0cff, low=0. This crate's `OtherMode` has no `Default`
        // impl of its own (callers always decode from a real `SetOtherModes`
        // wire pair via `from_wire`); this test instead characterizes what
        // that literal reset word decodes to through every new accessor, so
        // a future `Default` impl (or a caller seeding this exact value) has
        // a pinned expectation.
        let reset = OtherMode::from_wire(0x0008_0cff, 0);
        assert_eq!(reset.cycle_type(), CycleType::OneCycle);
        assert_eq!(reset.texture_filter(), TextureFilter::Point);
        assert_eq!(reset.rgb_dither(), RgbDither::Disabled);
        assert_eq!(reset.alpha_dither(), AlphaDither::Disabled);
        assert!(!reset.combine_key());
        assert_eq!(reset.texture_convert(), 6);
        assert_eq!(reset.texture_lut_mode(), Ok(TextureLutMode::Disabled));
        assert!(!reset.texture_lod());
        assert_eq!(reset.texture_detail(), 0);
        assert!(reset.texture_perspective());
        assert!(!reset.one_primitive_pipeline());
        let zero_low = OtherMode::from_wire(0x0008_0cff, 0);
        assert_eq!(zero_low.alpha_compare(), AlphaCompare::None);
        assert!(!zero_low.primitive_depth_source());
        assert!(!zero_low.antialias_enabled());
        assert!(!zero_low.depth_compare_enabled());
        assert!(!zero_low.depth_update_enabled());
        assert!(!zero_low.image_read_enabled());
        assert!(!zero_low.clear_on_coverage());
        assert_eq!(zero_low.coverage_destination(), CoverageDestination::Clamp);
        assert_eq!(zero_low.depth_mode(), DepthMode::Opaque);
        assert!(!zero_low.coverage_times_alpha());
        assert!(!zero_low.alpha_coverage_select());
        assert!(!zero_low.force_blend());
    }

    #[test]
    fn all_reserved_and_enum_variants_are_reachable_and_distinct() {
        // Loud structural guard: every enum this module defines must have
        // every listed variant actually reachable from some wire encoding,
        // so a future edit that collapses two encodings into one variant (or
        // silently drops a reserved encoding) fails a test instead of
        // shipping unnoticed.
        let filters = [
            TextureFilter::Point,
            TextureFilter::Reserved,
            TextureFilter::Bilinear,
            TextureFilter::Average,
        ];
        for (value, expected) in filters.iter().enumerate() {
            assert_eq!(
                OtherMode::from_wire((value as u32) << 12, 0).texture_filter(),
                *expected
            );
        }
        let compares = [
            AlphaCompare::None,
            AlphaCompare::Threshold,
            AlphaCompare::Reserved,
            AlphaCompare::Dither,
        ];
        for (value, expected) in compares.iter().enumerate() {
            assert_eq!(
                OtherMode::from_wire(0, value as u32).alpha_compare(),
                *expected
            );
        }
    }

    #[test]
    fn color4_decodes_exact_rgba_byte_order_with_asymmetric_channel_values() {
        let color = Color4::from_wire(0x11223344);
        assert_eq!(color.value(), 0x11223344);
        assert_eq!(color.rgba8(), [0x11, 0x22, 0x33, 0x44]);
        let normalized = color.normalized();
        assert_eq!(normalized[0], 0x11 as f32 / 255.0);
        assert_eq!(normalized[1], 0x22 as f32 / 255.0);
        assert_eq!(normalized[2], 0x33 as f32 / 255.0);
        assert_eq!(normalized[3], 0x44 as f32 / 255.0);
    }

    #[test]
    fn color4_zero_and_max_boundaries_normalize_exactly() {
        let zero = Color4::from_wire(0);
        assert_eq!(zero.rgba8(), [0, 0, 0, 0]);
        assert_eq!(zero.normalized(), [0.0, 0.0, 0.0, 0.0]);

        let max = Color4::from_wire(u32::MAX);
        assert_eq!(max.rgba8(), [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(max.normalized(), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn color4_swapping_word_endianness_changes_channel_assignment() {
        // A hostile check that the byte order is exactly RT64's
        // ((color >> 24) & 0xFF) = R .. ((color >> 0) & 0xFF) = A, not some
        // other permutation: reversing the word's byte order must reverse
        // the decoded channel order too.
        let forward = Color4::from_wire(0xAABBCCDD);
        let reversed = Color4::from_wire(0xDDCCBBAA);
        assert_eq!(forward.rgba8(), [0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(reversed.rgba8(), [0xDD, 0xCC, 0xBB, 0xAA]);
    }

    #[test]
    fn prim_lod_reads_lod_frac_from_full_byte_and_lod_min_from_five_bits() {
        // w0 bits 8:12 = lodMin (5 bits), bits 0:7 = lodFrac (full byte),
        // matching RT64's `p0(0, 8)` / `p0(8, 5)` exactly
        // (`src/gbi/rt64_gbi_rdp.cpp:102-103`).
        let w0 = (0x17 << 8) | 0xab;
        let lod = PrimLod::from_wire(w0);
        assert_eq!(lod.lod_frac(), 0xab);
        assert_eq!(lod.lod_min(), 0x17);
    }

    #[test]
    fn prim_lod_min_masks_to_five_bits_discarding_the_public_eighth_bit() {
        // RT64's own comment: "the RDP only uses 5 of them" -- bit 13 (the
        // 6th bit of the public 8-bit lodMin field) must be discarded, not
        // folded into the 5-bit value.
        let w0 = 0xff << 8;
        let lod = PrimLod::from_wire(w0);
        assert_eq!(lod.lod_min(), 0x1f);

        let w0_with_high_bit = 0x20 << 8; // bit 13 set, bits 8:12 clear
        let lod = PrimLod::from_wire(w0_with_high_bit);
        assert_eq!(lod.lod_min(), 0);
    }

    #[test]
    fn prim_lod_normalizes_by_256_and_32() {
        let lod = PrimLod::from_wire((16 << 8) | 128);
        assert_eq!(lod.lod_frac_normalized(), 128.0 / 256.0);
        assert_eq!(lod.lod_min_normalized(), 16.0 / 32.0);
    }

    #[test]
    fn prim_lod_zero_and_max_boundaries() {
        let zero = PrimLod::from_wire(0);
        assert_eq!(zero.lod_frac(), 0);
        assert_eq!(zero.lod_min(), 0);
        assert_eq!(zero.lod_frac_normalized(), 0.0);
        assert_eq!(zero.lod_min_normalized(), 0.0);

        let max = PrimLod::from_wire((0x1f << 8) | 0xff);
        assert_eq!(max.lod_frac(), 0xff);
        assert_eq!(max.lod_min(), 0x1f);
        assert_eq!(max.lod_frac_normalized(), 0xff as f32 / 256.0);
        assert_eq!(max.lod_min_normalized(), 0x1f as f32 / 32.0);
    }

    #[test]
    fn prim_color_combines_w0_lod_bytes_and_w1_color_independently() {
        let w0 = (0x0a << 8) | 0x3c;
        let w1 = 0x11223344;
        let prim = PrimColor::from_wire(w0, w1);
        assert_eq!(prim.lod().lod_frac(), 0x3c);
        assert_eq!(prim.lod().lod_min(), 0x0a);
        assert_eq!(prim.color().rgba8(), [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn prim_color_unrelated_w0_bits_above_lod_min_do_not_leak_into_lod_fields() {
        // Bits 13:31 of w0 carry no PrimColor semantics; a hostile all-ones
        // w0 above the 5-bit lodMin field must not perturb lod_frac/lod_min.
        let hostile_w0 = 0xffff_e000; // bits 13:31 set, bits 0:12 clear
        let prim = PrimColor::from_wire(hostile_w0, 0);
        assert_eq!(prim.lod().lod_frac(), 0);
        assert_eq!(prim.lod().lod_min(), 0);
    }

    #[test]
    fn prim_depth_masks_z_to_fifteen_bits_and_dz_to_sixteen_bits() {
        // w1 bits 16:31 = z, bits 0:15 = dz, matching RT64's
        // `p1(16, 16)` / `p1(0, 16)` (`src/gbi/rt64_gbi_rdp.cpp:130-131`).
        // The z mask itself is only 15 bits (`z & 0x7FFFU`); bit 31 of w1
        // (the wire word's top bit) is discarded even though p1(16,16)
        // extracts it.
        let w1 = (0xffffu32 << 16) | 0x1234;
        let depth = PrimDepth::from_wire(w1);
        assert_eq!(depth.z(), 0x7fff);
        assert_eq!(depth.dz(), 0x1234);
    }

    #[test]
    fn prim_depth_masked_high_bit_is_hostile_and_must_be_discarded() {
        // Only the top bit of the 16-bit z field (wire bit 31) set; every
        // other z bit clear. This must decode to z()==0, not 0x8000.
        let w1 = 0x8000_0000;
        let depth = PrimDepth::from_wire(w1);
        assert_eq!(depth.z(), 0);
        assert_eq!(depth.dz(), 0);
    }

    #[test]
    fn prim_depth_zero_and_max_boundaries_normalize_exactly() {
        let zero = PrimDepth::from_wire(0);
        assert_eq!(zero.z(), 0);
        assert_eq!(zero.dz(), 0);
        assert_eq!(zero.z_normalized(), 0.0);
        assert_eq!(zero.dz_normalized(), 0.0);

        // Max representable z is 0x7FFF (15 bits), not 0xFFFF: set every
        // wire bit and confirm the top bit is discarded.
        let max_w1 = 0xffff_ffff;
        let max = PrimDepth::from_wire(max_w1);
        assert_eq!(max.z(), 0x7fff);
        assert_eq!(max.dz(), 0xffff);
        assert_eq!(max.z_normalized(), 0x7fff as f32 / 32767.0);
        assert_eq!(max.z_normalized(), 1.0);
        assert_eq!(max.dz_normalized(), 0xffff as f32 / 65535.0);
        assert_eq!(max.dz_normalized(), 1.0);
    }
}
