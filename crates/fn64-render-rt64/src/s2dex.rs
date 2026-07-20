//! Minimal, content-admitted S2DEX decoder.
//!
//! This slice implements public legacy S2DEX and S2DEX2 backgrounds, object
//! rectangles, matrix, rotating sprite, and object texture-load wire forms.
//! Exact admitted microcode identity selects the colliding GBI envelope. Loads
//! use the existing raw-RDP TMEM path; draws lower through existing rectangle/
//! triangle paths.

use crate::gbi::{
    CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, UcodeDigest, Vertex,
};
use fn64_render::{RenderError, UcodeId};
use std::collections::HashMap;

pub const SUPPORTED: &[UcodeId] = &[UcodeId::S2dex, UcodeId::S2dex2];
const SUPPORTED_S2DEX: &[UcodeId] = &[UcodeId::S2dex];
const SUPPORTED_S2DEX2: &[UcodeId] = &[UcodeId::S2dex2];

/// Public `gs2dex.h` wire families. Their payload structures are shared, but
/// their opcode assignments and `gMoveWd` packing are not interchangeable.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum S2dexWireFamily {
    S2dex,
    S2dex2,
}

impl S2dexWireFamily {
    pub const fn ucode_id(self) -> UcodeId {
        match self {
            Self::S2dex => UcodeId::S2dex,
            Self::S2dex2 => UcodeId::S2dex2,
        }
    }
}

const G_OBJ_RECTANGLE: u8 = 0x01;
const G_OBJ_SPRITE: u8 = 0x02;
const G_SELECT_DL: u8 = 0x04;
const G_OBJ_LOADTXTR: u8 = 0x05;
const G_OBJ_LDTX_SPRITE: u8 = 0x06;
const G_OBJ_LDTX_RECT: u8 = 0x07;
const G_OBJ_LDTX_RECT_R: u8 = 0x08;
const G_BG_1CYC: u8 = 0x09;
const G_BG_COPY: u8 = 0x0a;
const G_OBJ_RENDERMODE: u8 = 0x0b;
const G_OBJ_RECTANGLE_R: u8 = 0xda;
const G_MOVEWORD: u8 = 0xdb;
const G_OBJ_MOVEMEM: u8 = 0xdc;
const G_RDPHALF_0: u8 = 0xe4;
const G_ENDDL: u8 = 0xdf;

const S2DEX_G_BG_1CYC: u8 = 0x01;
const S2DEX_G_BG_COPY: u8 = 0x02;
const S2DEX_G_OBJ_RECTANGLE: u8 = 0x03;
const S2DEX_G_OBJ_SPRITE: u8 = 0x04;
const S2DEX_G_OBJ_MOVEMEM: u8 = 0x05;
const S2DEX_G_SELECT_DL: u8 = 0xb0;
const S2DEX_G_OBJ_RENDERMODE: u8 = 0xb1;
const S2DEX_G_OBJ_RECTANGLE_R: u8 = 0xb2;
const S2DEX_G_ENDDL: u8 = 0xb8;
const S2DEX_G_MOVEWORD: u8 = 0xbc;
const S2DEX_G_OBJ_LOADTXTR: u8 = 0xc1;
const S2DEX_G_OBJ_LDTX_SPRITE: u8 = 0xc2;
const S2DEX_G_OBJ_LDTX_RECT: u8 = 0xc3;
const S2DEX_G_OBJ_LDTX_RECT_R: u8 = 0xc4;

const MAX_COMMANDS: usize = 1 << 20;
const MAX_DL_DEPTH: usize = 18;
const PHYSICAL_RDRAM_BYTES: usize = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
const OBJ_SPRITE_BYTES: usize = 24;
const OBJ_TEXTURE_BYTES: usize = 24;
const OBJ_TX_SPRITE_BYTES: usize = OBJ_TEXTURE_BYTES + OBJ_SPRITE_BYTES;
const OBJ_BG_BYTES: usize = 40;
const OBJECT_TEXTURE_SCRATCH_BYTES: usize = 4096 + 40;
const BACKGROUND_SCRATCH_BYTES: usize = 8192 + 48;

const G_BGLT_LOADBLOCK: u16 = 0x0033;
const G_BGLT_LOADTILE: u16 = 0xfff4;
const G_BG_FLAG_FLIPS: u16 = 1;

const G_OBJLT_TXTRBLOCK: u32 = 0x0000_1033;
const G_OBJLT_TXTRTILE: u32 = 0x00fc_1034;
const G_OBJLT_TLUT: u32 = 0x0000_0030;

const G_MW_SEGMENT: u8 = 0x06;
const G_MW_GENSTAT: u8 = 0x08;

const G_OBJ_FLAG_FLIPS: u8 = 1 << 0;
const G_OBJ_FLAG_FLIPT: u8 = 1 << 4;
const G_OBJRM_NOTXCLAMP: u32 = 0x01;
const G_OBJRM_XLU: u32 = 0x02;
const G_OBJRM_ANTIALIAS: u32 = 0x04;
const G_OBJRM_BILERP: u32 = 0x08;
const G_OBJRM_SHRINKSIZE_1: u32 = 0x10;
const G_OBJRM_SHRINKSIZE_2: u32 = 0x20;
const G_OBJRM_WIDEN: u32 = 0x40;
const G_OBJRM_ALL: u32 = G_OBJRM_NOTXCLAMP
    | G_OBJRM_XLU
    | G_OBJRM_ANTIALIAS
    | G_OBJRM_BILERP
    | G_OBJRM_SHRINKSIZE_1
    | G_OBJRM_SHRINKSIZE_2
    | G_OBJRM_WIDEN;

const RDP_SETTIMG: u8 = 0xfd;
const RDP_SETTILE: u8 = 0xf5;
const RDP_LOADSYNC: u8 = 0xe6;
const RDP_LOADBLOCK: u8 = 0xf3;
const RDP_LOADTILE: u8 = 0xf4;
const RDP_LOADTLUT: u8 = 0xf0;

#[derive(Clone, Debug, Default)]
pub struct UcodeCatalog {
    digests: HashMap<UcodeDigest, S2dexWireFamily>,
}

impl UcodeCatalog {
    pub fn admit_sha256(&mut self, digest: [u8; 32]) {
        self.admit_sha256_for(S2dexWireFamily::S2dex2, digest);
    }

    pub fn admit_sha256_for(&mut self, family: S2dexWireFamily, digest: [u8; 32]) {
        self.admit(UcodeDigest::from_sha256(digest), family);
    }

    pub fn admit_text(&mut self, text: &[u8]) -> UcodeDigest {
        self.admit_text_for(S2dexWireFamily::S2dex2, text)
    }

    pub fn admit_text_for(&mut self, family: S2dexWireFamily, text: &[u8]) -> UcodeDigest {
        let digest = UcodeDigest::from_text(text);
        self.admit(digest, family);
        digest
    }

    fn admit(&mut self, digest: UcodeDigest, family: S2dexWireFamily) {
        if let Some(previous) = self.digests.insert(digest, family) {
            assert_eq!(
                previous, family,
                "one S2DEX microcode digest cannot identify two wire families"
            );
        }
    }

    pub fn require_text(&self, text: &[u8]) -> Result<S2dexWireFamily, RenderError> {
        let digest = UcodeDigest::from_text(text);
        self.digests
            .get(&digest)
            .copied()
            .ok_or(RenderError::RequiresLle {
                ucode_sha256: digest.as_bytes(),
            })
    }

    pub(crate) fn identify_text(
        &self,
        text: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        self.digests
            .get(&UcodeDigest::from_text(text))
            .copied()
            .map(S2dexWireFamily::ucode_id)
    }

    pub fn supported_ucodes(&self) -> &'static [UcodeId] {
        let s2dex = self
            .digests
            .values()
            .any(|family| *family == S2dexWireFamily::S2dex);
        let s2dex2 = self
            .digests
            .values()
            .any(|family| *family == S2dexWireFamily::S2dex2);
        match (s2dex, s2dex2) {
            (false, false) => &[],
            (true, false) => SUPPORTED_S2DEX,
            (false, true) => SUPPORTED_S2DEX2,
            (true, true) => SUPPORTED,
        }
    }
}

/// Public `uObjSprite_t` wire fields from `gs2dex.h` / Programming Manual
/// Chapter 25, "S2DEX Microcode", section 4.2.1.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectSprite {
    pub obj_x: i16,
    pub scale_w: u16,
    pub image_w: u16,
    pub padding_x: u16,
    pub obj_y: i16,
    pub scale_h: u16,
    pub image_h: u16,
    pub padding_y: u16,
    pub image_stride: u16,
    pub image_address: u16,
    pub image_format: u8,
    pub image_size: u8,
    pub image_palette: u8,
    pub image_flags: u8,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ObjectTextureClamp {
    #[default]
    Perimeter,
    Disabled,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ObjectFilterCorrection {
    #[default]
    PointOrAverage,
    Bilinear,
}

/// Independent public S2DEX perimeter corrections. The object-render-mode
/// manual permits flags to be ORed and excludes only SHRINKSIZE_1 together
/// with SHRINKSIZE_2, so WIDEN cannot be represented as an enum alternative
/// to shrinking.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectPerimeter {
    shrink_half_texels: u8,
    widen_three_eighths_texel: bool,
}

impl ObjectPerimeter {
    pub(crate) fn is_none(self) -> bool {
        self.shrink_half_texels == 0 && !self.widen_three_eighths_texel
    }

    fn corrected_image_5(
        self,
        image_5: u16,
        axis: &str,
        command: &str,
    ) -> Result<u16, RenderError> {
        let shrink_5 = u16::from(self.shrink_half_texels) * 32;
        let image_5 = image_5
            .checked_sub(shrink_5)
            .ok_or_else(|| reject(format!("{command} shrink exceeds the source image {axis}")))?;
        if self.widen_three_eighths_texel {
            image_5.checked_add(12).ok_or_else(|| {
                reject(format!(
                    "{command} G_OBJRM_WIDEN exceeds the u10.5 {axis} image domain"
                ))
            })
        } else {
            Ok(image_5)
        }
    }

    fn exact_screen_adjustments(
        self,
        scale_10: u16,
        axis: &str,
        command: &str,
    ) -> Result<(f32, f32), RenderError> {
        if scale_10 == 0 {
            return Err(reject(format!(
                "{command} {axis} scale must be nonzero before perimeter correction"
            )));
        }
        let shrink_quarter_numerator = u32::from(self.shrink_half_texels) * 4096;
        if !shrink_quarter_numerator.is_multiple_of(u32::from(scale_10)) {
            return Err(unsupported(
                "render.s2dex.perimeter-shrink-precision",
                format!(
                    "{command} G_OBJRM_SHRINKSIZE {axis} edge requires unpublished sub-quarter-pixel rounding: shrink_half_texels={} scale={scale_10}",
                    self.shrink_half_texels
                ),
            ));
        }
        let widen_quarter_numerator: u32 = if self.widen_three_eighths_texel {
            3 * 1024 / 2
        } else {
            0
        };
        if !widen_quarter_numerator.is_multiple_of(u32::from(scale_10)) {
            return Err(unsupported(
                "render.s2dex.perimeter-widen-precision",
                format!(
                    "{command} G_OBJRM_WIDEN {axis} edge requires unpublished sub-quarter-pixel rounding: scale={scale_10}"
                ),
            ));
        }
        Ok((
            (shrink_quarter_numerator / u32::from(scale_10)) as f32 / 4.0,
            (widen_quarter_numerator / u32::from(scale_10)) as f32 / 4.0,
        ))
    }

    fn source_bounds(self, image_5: u16) -> (f32, f32) {
        let shrink = f32::from(self.shrink_half_texels) * 0.5;
        let widen = if self.widen_three_eighths_texel {
            0.375
        } else {
            0.0
        };
        (shrink, f32::from(image_5) / 32.0 - shrink + widen)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IgnoredObjectEdgeFlags {
    pub xlu: bool,
    pub antialias: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ObjectRenderMode {
    pub texture_clamp: ObjectTextureClamp,
    pub filter_correction: ObjectFilterCorrection,
    pub perimeter: ObjectPerimeter,
    pub ignored_edge_flags: IgnoredObjectEdgeFlags,
}

impl ObjectRenderMode {
    fn bilerp(self) -> bool {
        self.filter_correction == ObjectFilterCorrection::Bilinear
    }

    fn shrink_half_texels(self) -> u8 {
        self.perimeter.shrink_half_texels
    }

    fn widens(self) -> bool {
        self.perimeter.widen_three_eighths_texel
    }
}

/// Public Average filtering and public inward perimeter correction are two
/// independent stages. S2DEX Microcode manual section 4.4.1,
/// "gSPObjRenderMode", permits render-mode flags to be ORed and defines
/// SHRINKSIZE_1/2 as removing 0.5/1.0 texel from every edge. The
/// `gDPSetTextureFilter` manual, "gDPSetTextureFilter", defines Average as the
/// four surrounding texels.
/// This marker exists only when the inward rectangle footprint is therefore
/// bounded. WIDEN remains outside this marker. NOTXCLAMP is admitted only
/// when the independent [`ObjectUnclampedAverageFootprint`] proves that the
/// clamp cannot affect any four-texel cell.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ObjectAverageShrinkFootprint {
    inset_half_texels: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ObjectAverageCell {
    Interior,
    PositiveEdgeClamped,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ObjectAverageAxisFootprint {
    Empty,
    Samples {
        first: ObjectAverageCell,
        last: ObjectAverageCell,
    },
}

impl ObjectAverageShrinkFootprint {
    fn from_mode(
        mode: ObjectRenderMode,
        filter: TextureFilter,
        command: &str,
    ) -> Result<Option<Self>, RenderError> {
        if filter != TextureFilter::Average
            || mode.filter_correction != ObjectFilterCorrection::PointOrAverage
            || mode.shrink_half_texels() == 0
        {
            return Ok(None);
        }
        if mode.widens() {
            return Err(unsupported(
                "render.s2dex.average-widen-footprint",
                format!(
                    "{command} Average plus G_OBJRM_WIDEN requires unpublished positive-edge four-texel footprint arithmetic"
                ),
            ));
        }
        Ok(Some(Self {
            inset_half_texels: mode.shrink_half_texels(),
        }))
    }

    /// The first Average cell in the unflipped or flipped rectangle.
    fn rectangle_start(self, image_5: u16, flipped: bool) -> f32 {
        let inset = f32::from(self.inset_half_texels) * 0.5;
        if flipped {
            f32::from(image_5) / 32.0 - 1.0 - inset
        } else {
            inset
        }
    }

    /// The base RDP rectangle builder validates filter/correction matching.
    /// Perimeter geometry remains owned by the separately typed S2DEX stage
    /// below, so only the already-proven inward marker is erased for that
    /// validation call.
    fn filter_validation_mode(self, mut mode: ObjectRenderMode) -> ObjectRenderMode {
        debug_assert_eq!(self.inset_half_texels, mode.shrink_half_texels());
        mode.perimeter = ObjectPerimeter::default();
        // Base lowering always constructs the public perimeter-clamped tile.
        // If NOTXCLAMP was requested, the separate four-neighbour proof below
        // establishes that this retained clamp is unobservable.
        mode.texture_clamp = ObjectTextureClamp::Perimeter;
        mode
    }

    fn classify_cell(
        self,
        coordinate: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<ObjectAverageCell, RenderError> {
        let first = coordinate.floor() as i32;
        let second = first + 1;
        let image_texels = i32::from(image_texels);
        if first >= 0 && second < image_texels {
            Ok(ObjectAverageCell::Interior)
        } else if first == image_texels - 1 && second == image_texels {
            // Average always addresses the positive neighbour. The retained
            // public perimeter clamp maps this one-past coordinate back to
            // the final texel; NOTXCLAMP cannot construct this marker.
            Ok(ObjectAverageCell::PositiveEdgeClamped)
        } else {
            Err(reject(format!(
                "{command} Average plus G_OBJRM_SHRINKSIZE {axis} cell ({first},{second}) exceeds the public interior-or-positive-edge-clamp footprint for {image_texels} texels"
            )))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_axis(
        self,
        texture_start: f32,
        texture_gradient: f32,
        screen_start: f32,
        screen_end: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<ObjectAverageAxisFootprint, RenderError> {
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let first_pixel = pixel_min(screen_start);
        let last_pixel = pixel_min(screen_end) - 1;
        if first_pixel > last_pixel {
            return Ok(ObjectAverageAxisFootprint::Empty);
        }
        let coordinate =
            |pixel: i32| texture_start + (pixel as f32 - screen_start.floor()) * texture_gradient;
        Ok(ObjectAverageAxisFootprint::Samples {
            first: self.classify_cell(coordinate(first_pixel), image_texels, axis, command)?,
            last: self.classify_cell(coordinate(last_pixel), image_texels, axis, command)?,
        })
    }
}

/// A point-filtered rectangle whose RSP perimeter clamp is disabled only
/// needs an addressing rule when an emitted sample leaves the public image.
/// S2DEX Microcode manual section 4.4.1, "gSPObjRenderMode", permits
/// NOTXCLAMP and perimeter flags to be ORed. This marker is constructed only
/// when the actual raster sample sequence can prove that clamp state is
/// unobservable; flag composability alone never admits an outward correction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ObjectUnclampedPointFootprint;

/// An Average-filtered rectangle whose disabled RSP perimeter clamp is
/// observationally irrelevant because both neighbours on each axis remain
/// inside the public source image for every emitted sample. The public filter
/// definition supplies the two neighbours per axis; monotonic affine texture
/// coordinates make the endpoint cells a complete proof for the sequence.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ObjectUnclampedAverageFootprint;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ObjectPointDirection {
    Increasing,
    Decreasing,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ObjectPointAxisFootprint {
    Empty,
    MonotonicInterior {
        direction: ObjectPointDirection,
        first_texel: u16,
        last_texel: u16,
    },
}

impl ObjectUnclampedPointFootprint {
    fn from_mode(
        mode: ObjectRenderMode,
        filter: TextureFilter,
        command: &str,
    ) -> Result<Option<Self>, RenderError> {
        if mode.texture_clamp == ObjectTextureClamp::Perimeter {
            return Ok(None);
        }
        if filter == TextureFilter::Average {
            return Ok(None);
        }
        if filter != TextureFilter::Point {
            return Err(unsupported(
                "render.s2dex.unclamped-filter-footprint",
                format!(
                    "{command} G_OBJRM_NOTXCLAMP with {filter:?} filtering requires unpublished filter-footprint arithmetic"
                ),
            ));
        }
        Ok(Some(Self))
    }

    fn classify_texel(
        self,
        coordinate: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<u16, RenderError> {
        let texel = coordinate.floor() as i32;
        if (0..i32::from(image_texels)).contains(&texel) {
            Ok(texel as u16)
        } else {
            Err(reject(format!(
                "{command} G_OBJRM_NOTXCLAMP point sample on {axis} addresses texel {texel} outside 0..{image_texels}"
            )))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_axis(
        self,
        texture_start: f32,
        texture_gradient: f32,
        screen_start: f32,
        screen_end: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<ObjectPointAxisFootprint, RenderError> {
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let first_pixel = pixel_min(screen_start);
        let last_pixel = pixel_min(screen_end) - 1;
        if first_pixel > last_pixel {
            return Ok(ObjectPointAxisFootprint::Empty);
        }
        let direction = if texture_gradient > 0.0 {
            ObjectPointDirection::Increasing
        } else if texture_gradient < 0.0 {
            ObjectPointDirection::Decreasing
        } else {
            return Err(reject(format!(
                "{command} G_OBJRM_NOTXCLAMP point sample sequence on {axis} has a zero gradient"
            )));
        };
        let coordinate =
            |pixel: i32| texture_start + (pixel as f32 - screen_start.floor()) * texture_gradient;
        // The emitted rectangle evaluates this affine coordinate at every
        // integer raster position. Its typed nonzero direction makes the
        // sequence monotonic, so interior endpoint texels bound every actual
        // point sample without a potentially multi-million-pixel decode loop.
        let first_texel =
            self.classify_texel(coordinate(first_pixel), image_texels, axis, command)?;
        let last_texel =
            self.classify_texel(coordinate(last_pixel), image_texels, axis, command)?;
        Ok(ObjectPointAxisFootprint::MonotonicInterior {
            direction,
            first_texel,
            last_texel,
        })
    }
}

impl ObjectUnclampedAverageFootprint {
    fn from_mode(
        mode: ObjectRenderMode,
        filter: TextureFilter,
        command: &str,
    ) -> Result<Option<Self>, RenderError> {
        if mode.texture_clamp == ObjectTextureClamp::Perimeter || filter != TextureFilter::Average {
            return Ok(None);
        }
        if mode.filter_correction != ObjectFilterCorrection::PointOrAverage {
            return Err(reject(format!(
                "{command} Average texture filter does not use G_OBJRM_BILERP correction"
            )));
        }
        Ok(Some(Self))
    }

    fn classify_cell(
        self,
        coordinate: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<(u16, u16), RenderError> {
        let first = coordinate.floor() as i32;
        let second = first + 1;
        if first >= 0 && second < i32::from(image_texels) {
            Ok((first as u16, second as u16))
        } else {
            Err(reject(format!(
                "{command} G_OBJRM_NOTXCLAMP Average four-texel cell on {axis} addresses ({first},{second}) outside 0..{image_texels}"
            )))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_axis(
        self,
        texture_start: f32,
        texture_gradient: f32,
        screen_start: f32,
        screen_end: f32,
        image_texels: u16,
        axis: &str,
        command: &str,
    ) -> Result<ObjectPointAxisFootprint, RenderError> {
        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
        let first_pixel = pixel_min(screen_start);
        let last_pixel = pixel_min(screen_end) - 1;
        if first_pixel > last_pixel {
            return Ok(ObjectPointAxisFootprint::Empty);
        }
        let direction = if texture_gradient > 0.0 {
            ObjectPointDirection::Increasing
        } else if texture_gradient < 0.0 {
            ObjectPointDirection::Decreasing
        } else {
            return Err(reject(format!(
                "{command} G_OBJRM_NOTXCLAMP Average sample sequence on {axis} has a zero gradient"
            )));
        };
        let coordinate =
            |pixel: i32| texture_start + (pixel as f32 - screen_start.floor()) * texture_gradient;
        let (first_texel, _) =
            self.classify_cell(coordinate(first_pixel), image_texels, axis, command)?;
        let (last_texel, _) =
            self.classify_cell(coordinate(last_pixel), image_texels, axis, command)?;
        Ok(ObjectPointAxisFootprint::MonotonicInterior {
            direction,
            first_texel,
            last_texel,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PendingSelectDl {
    sid: u8,
    flag: u32,
    target_low: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ObjectTextureCommon {
    image: u32,
    sid: u16,
    flag: u32,
    mask: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ObjectMatrix {
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    x: i16,
    y: i16,
    base_scale_x: u16,
    base_scale_y: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BackgroundCommon {
    image_x: u16,
    image_w: u16,
    frame_x: i16,
    frame_w: u16,
    image_y: u16,
    image_h: u16,
    frame_y: i16,
    frame_h: u16,
    image: u32,
    image_load: u16,
    image_format: u8,
    image_size: u8,
    image_palette: u16,
    image_flip: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Background {
    Copy {
        common: BackgroundCommon,
        tmem_w: u16,
        tmem_h: u16,
        tmem_load_sh: u16,
        tmem_load_th: u16,
        tmem_size_w: u16,
        tmem_size: u16,
    },
    Scale {
        common: BackgroundCommon,
        scale_w: u16,
        scale_h: u16,
        image_y_origin: i32,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum S2dexCommand {
    ObjRectangle,
    ObjSprite,
    SelectDl,
    ObjLoadTxtr,
    ObjLdTxSprite,
    ObjLdTxRect,
    ObjLdTxRectR,
    Bg1Cyc,
    BgCopy,
    ObjRenderMode,
    ObjRectangleR,
    MoveWord,
    ObjMoveMem,
    RdpHalf0,
    EndDl,
}

impl S2dexCommand {
    const fn name(self) -> &'static str {
        match self {
            Self::ObjRectangle => "G_OBJ_RECTANGLE",
            Self::ObjSprite => "G_OBJ_SPRITE",
            Self::SelectDl => "G_SELECT_DL",
            Self::ObjLoadTxtr => "G_OBJ_LOADTXTR",
            Self::ObjLdTxSprite => "G_OBJ_LDTX_SPRITE",
            Self::ObjLdTxRect => "G_OBJ_LDTX_RECT",
            Self::ObjLdTxRectR => "G_OBJ_LDTX_RECT_R",
            Self::Bg1Cyc => "G_BG_1CYC",
            Self::BgCopy => "G_BG_COPY",
            Self::ObjRenderMode => "G_OBJ_RENDERMODE",
            Self::ObjRectangleR => "G_OBJ_RECTANGLE_R",
            Self::MoveWord => "G_MOVEWORD",
            Self::ObjMoveMem => "G_OBJ_MOVEMEM",
            Self::RdpHalf0 => "G_RDPHALF_0",
            Self::EndDl => "G_ENDDL",
        }
    }
}

fn decode_command(family: S2dexWireFamily, opcode: u8) -> Option<S2dexCommand> {
    use S2dexCommand as Command;
    match family {
        S2dexWireFamily::S2dex2 => match opcode {
            G_OBJ_RECTANGLE => Some(Command::ObjRectangle),
            G_OBJ_SPRITE => Some(Command::ObjSprite),
            G_SELECT_DL => Some(Command::SelectDl),
            G_OBJ_LOADTXTR => Some(Command::ObjLoadTxtr),
            G_OBJ_LDTX_SPRITE => Some(Command::ObjLdTxSprite),
            G_OBJ_LDTX_RECT => Some(Command::ObjLdTxRect),
            G_OBJ_LDTX_RECT_R => Some(Command::ObjLdTxRectR),
            G_BG_1CYC => Some(Command::Bg1Cyc),
            G_BG_COPY => Some(Command::BgCopy),
            G_OBJ_RENDERMODE => Some(Command::ObjRenderMode),
            G_OBJ_RECTANGLE_R => Some(Command::ObjRectangleR),
            G_MOVEWORD => Some(Command::MoveWord),
            G_OBJ_MOVEMEM => Some(Command::ObjMoveMem),
            G_RDPHALF_0 => Some(Command::RdpHalf0),
            G_ENDDL => Some(Command::EndDl),
            _ => None,
        },
        S2dexWireFamily::S2dex => match opcode {
            S2DEX_G_BG_1CYC => Some(Command::Bg1Cyc),
            S2DEX_G_BG_COPY => Some(Command::BgCopy),
            S2DEX_G_OBJ_RECTANGLE => Some(Command::ObjRectangle),
            S2DEX_G_OBJ_SPRITE => Some(Command::ObjSprite),
            S2DEX_G_OBJ_MOVEMEM => Some(Command::ObjMoveMem),
            S2DEX_G_SELECT_DL => Some(Command::SelectDl),
            S2DEX_G_OBJ_RENDERMODE => Some(Command::ObjRenderMode),
            S2DEX_G_OBJ_RECTANGLE_R => Some(Command::ObjRectangleR),
            S2DEX_G_ENDDL => Some(Command::EndDl),
            S2DEX_G_MOVEWORD => Some(Command::MoveWord),
            S2DEX_G_OBJ_LOADTXTR => Some(Command::ObjLoadTxtr),
            S2DEX_G_OBJ_LDTX_SPRITE => Some(Command::ObjLdTxSprite),
            S2DEX_G_OBJ_LDTX_RECT => Some(Command::ObjLdTxRect),
            S2DEX_G_OBJ_LDTX_RECT_R => Some(Command::ObjLdTxRectR),
            G_RDPHALF_0 => Some(Command::RdpHalf0),
            _ => None,
        },
    }
}

fn move_word_fields(family: S2dexWireFamily, word: u32) -> (u8, u16) {
    match family {
        S2dexWireFamily::S2dex => ((word & 0xff) as u8, ((word >> 8) & 0xffff) as u16),
        S2dexWireFamily::S2dex2 => (((word >> 16) & 0xff) as u8, (word & 0xffff) as u16),
    }
}

struct BackgroundScratch {
    bytes: Vec<u8>,
}

/// One exact load/draw partition of a Copy-mode scrolling window. All fields
/// are whole texels/pixels. Construction is private to [`BackgroundCopyWindow`]
/// so an admitted slice is nonempty, in-bounds, and maps one output rectangle
/// to one contiguous source rectangle without crossing a wrapped image edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BackgroundCopySlice {
    output_x_start: u32,
    output_x_end: u32,
    output_y_start: u32,
    output_y_end: u32,
    source_x_start: u32,
    source_x_end: u32,
    source_y_start: u32,
    source_y_end: u32,
    reverse_s: bool,
}

/// Validated integer Copy-mode window from public S2DEX section 4.1.2. The
/// manual defines the right-edge successor `(imageW - 1, y) -> (0, y + 1)`
/// and vertically/horizontally looped closed areas. Keeping that topology in
/// a typed planner prevents the loader and rectangle emitter from inventing
/// separate wrap rules.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BackgroundCopyWindow {
    image_width: u32,
    image_height: u32,
    frame_width: u32,
    frame_height: u32,
    image_x: u32,
    image_y: u32,
    reverse_s: bool,
    max_source_rows: u32,
}

impl BackgroundCopyWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        image_width: u32,
        image_height: u32,
        frame_width: u32,
        frame_height: u32,
        image_x: u32,
        image_y: u32,
        reverse_s: bool,
        max_source_rows: u32,
        command: &str,
    ) -> Result<Self, RenderError> {
        if image_width == 0 || image_height == 0 || frame_width == 0 || frame_height == 0 {
            return Err(reject(format!(
                "{command} copy-window dimensions must all be nonzero"
            )));
        }
        if frame_width > image_width || frame_height > image_height {
            return Err(reject(format!(
                "{command} transfer frame {frame_width}x{frame_height} exceeds the public copy-background image {image_width}x{image_height}"
            )));
        }
        if image_x >= image_width || image_y >= image_height {
            return Err(reject(format!(
                "{command} copy-window origin ({image_x},{image_y}) must be wrapped into image {image_width}x{image_height} before submission"
            )));
        }
        if max_source_rows == 0 {
            return Err(reject(format!(
                "{command} initialized TMEM geometry admits zero source rows"
            )));
        }
        Ok(Self {
            image_width,
            image_height,
            frame_width,
            frame_height,
            image_x,
            image_y,
            reverse_s,
            max_source_rows,
        })
    }

    fn slices(self) -> Vec<BackgroundCopySlice> {
        let image_texels = u64::from(self.image_width) * u64::from(self.image_height);
        let mut slices: Vec<BackgroundCopySlice> = Vec::new();
        for output_y in 0..self.frame_height {
            let row_start = (u64::from((self.image_y + output_y) % self.image_height)
                * u64::from(self.image_width)
                + u64::from(self.image_x))
                % image_texels;
            let mut output_x = 0;
            while output_x < self.frame_width {
                let mapped_x = if self.reverse_s {
                    self.frame_width - 1 - output_x
                } else {
                    output_x
                };
                let source_linear = (row_start + u64::from(mapped_x)) % image_texels;
                let source_y = u32::try_from(source_linear / u64::from(self.image_width))
                    .expect("validated background source Y fits u32");
                let source_x = u32::try_from(source_linear % u64::from(self.image_width))
                    .expect("validated background source X fits u32");
                let remaining = self.frame_width - output_x;
                let run = if self.reverse_s {
                    remaining.min(source_x + 1)
                } else {
                    remaining.min(self.image_width - source_x)
                };
                let (source_x_start, source_x_end) = if self.reverse_s {
                    (source_x + 1 - run, source_x + 1)
                } else {
                    (source_x, source_x + run)
                };

                let matching = slices.iter_mut().rev().find(|slice| {
                    slice.output_x_start == output_x
                        && slice.output_x_end == output_x + run
                        && slice.source_x_start == source_x_start
                        && slice.source_x_end == source_x_end
                        && slice.reverse_s == self.reverse_s
                        && slice.output_y_end == output_y
                        && slice.source_y_end == source_y
                        && slice.source_y_end - slice.source_y_start < self.max_source_rows
                });
                if let Some(slice) = matching {
                    slice.output_y_end += 1;
                    slice.source_y_end += 1;
                } else {
                    slices.push(BackgroundCopySlice {
                        output_x_start: output_x,
                        output_x_end: output_x + run,
                        output_y_start: output_y,
                        output_y_end: output_y + 1,
                        source_x_start,
                        source_x_end,
                        source_y_start: source_y,
                        source_y_end: source_y + 1,
                        reverse_s: self.reverse_s,
                    });
                }
                output_x += run;
            }
        }
        slices
    }
}

/// The source footprint needed to evaluate one scaled-background sample.
/// Public S2DEX documents point and bilinear selection, but does not publish
/// the bilinear neighbour/strip-boundary rounding needed to partition TMEM
/// loads without seams. Keeping the footprint explicit prevents the point
/// path from silently standing in for that missing contract.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BackgroundFilterFootprint {
    Point,
    Bilinear,
}

impl BackgroundFilterFootprint {
    fn from_rdp(filter: TextureFilter, command: &str) -> Result<Self, RenderError> {
        match filter {
            TextureFilter::Point => Ok(Self::Point),
            TextureFilter::Bilinear => Ok(Self::Bilinear),
            TextureFilter::Reserved | TextureFilter::Average => Err(unsupported(
                "render.s2dex.background-filter-footprint",
                format!(
                    "{command} texture filter {filter:?} has no public scaled-background footprint"
                ),
            )),
        }
    }
}

/// Public `uObjScaleBg_t.imageYorig`, retained separately from the image-Y
/// sample origin. S2DEX section 4.1.3 requires callers to update the two
/// independently while scrolling. Its private sub-plane boundary equation is
/// not needed by the zero-neighbour point footprint, but sub-texel origins are
/// kept loud because their strip rounding is not published.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BackgroundSubplaneOrigin(i32);

impl BackgroundSubplaneOrigin {
    fn new(raw: i32, command: &str) -> Result<Self, RenderError> {
        if raw & 0x1f != 0 {
            return Err(unsupported(
                "render.s2dex.background-subplane-precision",
                format!(
                    "{command} imageYorig={raw} requires unpublished sub-texel strip-origin rounding"
                ),
            ));
        }
        Ok(Self(raw))
    }
}

/// One point-sampled fixed-point load/draw slice. S/T starts use 1/1024 texel
/// units so construction and tests never route identity through `f32`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ScaledBackgroundSlice {
    output_x_start: u32,
    output_x_end: u32,
    output_y: u32,
    source_x_start: u32,
    source_x_end: u32,
    source_y: u32,
    s_start_10: u32,
    t_start_10: u32,
    dsdx_10: i32,
}

/// Validated point-sampled `gSPBgRect1Cyc` window. Public S2DEX section 4.1.3
/// specifies u10.5 image origins, u5.10 scale, horizontal flip, and a closed
/// row-major image. The planner splits whenever scaled S crosses an image-row
/// edge, then wraps the row carry through image Y exactly.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ScaledBackgroundWindow {
    image_width: u32,
    image_height: u32,
    frame_width: u32,
    frame_height: u32,
    image_x_5: u16,
    image_y_5: u16,
    scale_w_10: u16,
    scale_h_10: u16,
    reverse_s: bool,
    _subplane_origin: BackgroundSubplaneOrigin,
}

impl ScaledBackgroundWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        image_width: u32,
        image_height: u32,
        frame_width: u32,
        frame_height: u32,
        image_x_5: u16,
        image_y_5: u16,
        scale_w_10: u16,
        scale_h_10: u16,
        reverse_s: bool,
        image_y_origin_5: i32,
        command: &str,
    ) -> Result<Self, RenderError> {
        if image_width == 0 || image_height == 0 || frame_width == 0 || frame_height == 0 {
            return Err(reject(format!(
                "{command} scaled-window dimensions must all be nonzero"
            )));
        }
        if u32::from(image_x_5) >= image_width * 32 || u32::from(image_y_5) >= image_height * 32 {
            return Err(reject(format!(
                "{command} scaled-window image origin ({image_x_5},{image_y_5}) must be wrapped into image {image_width}x{image_height} before submission"
            )));
        }
        if image_y_5 & 0x1f != 0 {
            return Err(unsupported(
                "render.s2dex.background-subpixel",
                format!(
                    "{command} imageY={image_y_5} requests unsupported vertical subpixel movement"
                ),
            ));
        }
        if scale_w_10 == 0
            || scale_h_10 == 0
            || scale_w_10 > i16::MAX as u16
            || scale_h_10 > i16::MAX as u16
        {
            return Err(reject(format!(
                "{command} scaleW={scale_w_10} scaleH={scale_h_10} is outside the nonzero RDP S5.10 gradient range"
            )));
        }
        Ok(Self {
            image_width,
            image_height,
            frame_width,
            frame_height,
            image_x_5,
            image_y_5,
            scale_w_10,
            scale_h_10,
            reverse_s,
            _subplane_origin: BackgroundSubplaneOrigin::new(image_y_origin_5, command)?,
        })
    }

    fn slices(
        self,
        footprint: BackgroundFilterFootprint,
        command: &str,
    ) -> Result<Vec<ScaledBackgroundSlice>, RenderError> {
        if footprint == BackgroundFilterFootprint::Bilinear {
            return Err(unsupported(
                "render.s2dex.background-bilinear-partition",
                format!(
                    "{command} bilinear scaled-background partitioning requires unpublished neighbour and imageYorig strip-boundary rounding"
                ),
            ));
        }

        let row_extent_10 = u64::from(self.image_width) * 1024;
        let image_height_10 = u64::from(self.image_height) * 1024;
        let source_s = |output_x: u32| {
            let mapped_x = if self.reverse_s {
                self.frame_width - 1 - output_x
            } else {
                output_x
            };
            u64::from(self.image_x_5) * 32 + u64::from(mapped_x) * u64::from(self.scale_w_10)
        };
        let mut slices = Vec::new();
        for output_y in 0..self.frame_height {
            let mut output_x = 0;
            while output_x < self.frame_width {
                let first_s = source_s(output_x);
                let row_carry = first_s / row_extent_10;
                let mut output_x_end = output_x + 1;
                while output_x_end < self.frame_width
                    && source_s(output_x_end) / row_extent_10 == row_carry
                {
                    output_x_end += 1;
                }

                let first_in_row = first_s % row_extent_10;
                let last_in_row = source_s(output_x_end - 1) % row_extent_10;
                let source_x_start = u32::try_from(first_in_row.min(last_in_row) / 1024)
                    .expect("validated scaled background source X fits u32");
                let source_x_end = u32::try_from(first_in_row.max(last_in_row) / 1024 + 1)
                    .expect("validated scaled background source X fits u32");
                let source_y_10 = (u64::from(self.image_y_5) * 32
                    + u64::from(output_y) * u64::from(self.scale_h_10)
                    + row_carry * 1024)
                    % image_height_10;
                let source_y = u32::try_from(source_y_10 / 1024)
                    .expect("validated scaled background source Y fits u32");
                let s_start_10 = u32::try_from(first_in_row)
                    .expect("validated scaled background S fits u32")
                    - source_x_start * 1024;
                slices.push(ScaledBackgroundSlice {
                    output_x_start: output_x,
                    output_x_end,
                    output_y,
                    source_x_start,
                    source_x_end,
                    source_y,
                    s_start_10,
                    t_start_10: u32::try_from(source_y_10 % 1024)
                        .expect("scaled background T fraction fits u32"),
                    dsdx_10: if self.reverse_s {
                        -i32::from(self.scale_w_10)
                    } else {
                        i32::from(self.scale_w_10)
                    },
                });
                output_x = output_x_end;
            }
        }
        Ok(slices)
    }
}

struct ObjectTextureScratch {
    bytes: Vec<u8>,
}

struct ObjectTextureRdpLoad {
    commands: Vec<(u32, u32)>,
    image: u32,
    image_bytes: usize,
}

impl ObjectTextureScratch {
    fn new() -> Self {
        Self {
            bytes: vec![0; OBJECT_TEXTURE_SCRATCH_BYTES],
        }
    }
}

impl BackgroundScratch {
    fn new() -> Self {
        Self {
            bytes: vec![0; BACKGROUND_SCRATCH_BYTES],
        }
    }
}

impl Default for ObjectMatrix {
    fn default() -> Self {
        Self {
            a: 1 << 16,
            b: 0,
            c: 0,
            d: 1 << 16,
            x: 0,
            y: 0,
            base_scale_x: 1 << 10,
            base_scale_y: 1 << 10,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ObjectTexture {
    Block {
        common: ObjectTextureCommon,
        tmem: u16,
        tsize: u16,
        tline: u16,
    },
    Tile {
        common: ObjectTextureCommon,
        tmem: u16,
        twidth: u16,
        theight: u16,
    },
    Tlut {
        common: ObjectTextureCommon,
        phead: u16,
        pnum: u16,
    },
}

impl ObjectTexture {
    fn common(self) -> ObjectTextureCommon {
        match self {
            Self::Block { common, .. } | Self::Tile { common, .. } | Self::Tlut { common, .. } => {
                common
            }
        }
    }
}

#[cfg(test)]
fn decode_ops(
    rdram: &[u8],
    start: u32,
    rdp: &mut RdpDecodeState,
) -> Result<Vec<RenderOp>, RenderError> {
    decode_ops_for_family(rdram, start, rdp, S2dexWireFamily::S2dex2)
}

pub(crate) fn decode_ops_for_family(
    rdram: &[u8],
    start: u32,
    rdp: &mut RdpDecodeState,
    family: S2dexWireFamily,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut pc = (start & 0x00ff_ffff) as usize;
    let mut operations = Vec::new();
    let mut speculative_rdp = rdp.clone();
    let mut object_status = [0u32; 4];
    let mut object_texture_scratch = ObjectTextureScratch::new();
    let mut object_matrix = None;
    let mut rotation_matrix_loaded = false;
    let mut object_render_mode = ObjectRenderMode::default();
    let mut pending_select = None;
    let mut return_stack = Vec::new();
    let mut segments = [0u32; 16];
    for command_index in 0..MAX_COMMANDS {
        let end = pc
            .checked_add(8)
            .ok_or_else(|| reject("display-list PC overflow"))?;
        if end > rdram.len() {
            return Err(reject(format!(
                "display list is truncated at RDRAM {pc:#010x}: need 8 command bytes, rdram_bytes={}",
                rdram.len()
            )));
        }
        let w0 = read_u32(rdram, pc);
        let w1 = read_u32(rdram, pc + 4);
        let opcode = (w0 >> 24) as u8;
        let decoded = decode_command(family, opcode);
        let command_pc = pc;
        pc = end;

        if pending_select.is_some() && decoded != Some(S2dexCommand::SelectDl) {
            return Err(reject(format!(
                "G_RDPHALF_0 at the preceding command must be followed immediately by G_SELECT_DL, got {} at RDRAM {command_pc:#010x}",
                decoded.map_or("UNKNOWN", S2dexCommand::name)
            )));
        }

        match decoded {
            Some(S2dexCommand::ObjRectangle) => {
                if w0 & 0x00ff_ffff != 0 {
                    return Err(reject(format!(
                        "G_OBJ_RECTANGLE at {command_pc:#010x} has nonzero reserved/length payload {:#08x}; public gsSPObjRectangle uses gDma0p length zero",
                        w0 & 0x00ff_ffff
                    )));
                }
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_RECTANGLE", "uObjSprite")?,
                    "G_OBJ_RECTANGLE",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_RECTANGLE",
                )?);
            }
            Some(S2dexCommand::ObjSprite) => {
                require_dma_length(w0, 0, "G_OBJ_SPRITE", command_pc)?;
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_SPRITE", "uObjSprite")?,
                    "G_OBJ_SPRITE",
                )?;
                let matrix = require_rotation_matrix(
                    object_matrix,
                    rotation_matrix_loaded,
                    "G_OBJ_SPRITE",
                    command_pc,
                    false,
                )?;
                operations.extend(object_sprite_ops(
                    &mut speculative_rdp,
                    sprite,
                    matrix,
                    object_render_mode,
                    "G_OBJ_SPRITE",
                )?);
            }
            Some(S2dexCommand::ObjRectangleR) => {
                require_dma_length(w0, 0, "G_OBJ_RECTANGLE_R", command_pc)?;
                let sprite = read_object_sprite(
                    rdram,
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_RECTANGLE_R", "uObjSprite")?,
                    "G_OBJ_RECTANGLE_R",
                )?;
                let matrix = object_matrix.ok_or_else(|| {
                    reject(format!(
                        "G_OBJ_RECTANGLE_R at RDRAM {command_pc:#010x} requires a preceding G_OBJ_MOVEMEM matrix command"
                    ))
                })?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    matrix_relative_sprite(sprite, matrix)?,
                    object_render_mode,
                    "G_OBJ_RECTANGLE_R",
                )?);
            }
            Some(S2dexCommand::ObjMoveMem) => {
                let matrix_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_MOVEMEM", "matrix")?;
                let (matrix, loads_rotation) = read_object_matrix_command(
                    rdram,
                    w0,
                    matrix_address,
                    object_matrix,
                    command_pc,
                )?;
                object_matrix = Some(matrix);
                rotation_matrix_loaded |= loads_rotation;
            }
            Some(S2dexCommand::ObjLoadTxtr) => {
                require_dma_length(w0, 23, "G_OBJ_LOADTXTR", command_pc)?;
                let texture_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LOADTXTR", "uObjTxtr")?;
                let texture =
                    read_object_texture(rdram, texture_address, &segments, "G_OBJ_LOADTXTR")?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LOADTXTR",
                )?;
            }
            Some(S2dexCommand::ObjLdTxRect) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_RECT", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_RECT", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_RECT",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_RECT")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_RECT uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_RECT")?;

                // Section 4.6.2 defines this command as LoadTxtr then
                // Rectangle. Both changes remain task-local until G_ENDDL,
                // so a rejected draw cannot commit its preceding load.
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_RECT",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_LDTX_RECT",
                )?);
            }
            Some(S2dexCommand::ObjLdTxRectR) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_RECT_R", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_RECT_R", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_RECT_R",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_RECT_R")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_RECT_R uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_RECT_R")?;
                let matrix = object_matrix.ok_or_else(|| {
                    reject(format!(
                        "G_OBJ_LDTX_RECT_R at RDRAM {command_pc:#010x} requires a preceding G_OBJ_MOVEMEM matrix command; texture load was not applied"
                    ))
                })?;
                let sprite = matrix_relative_sprite(sprite, matrix)?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_RECT_R",
                )?;
                operations.push(object_rectangle_op(
                    &mut speculative_rdp,
                    sprite,
                    object_render_mode,
                    "G_OBJ_LDTX_RECT_R",
                )?);
            }
            Some(S2dexCommand::ObjLdTxSprite) => {
                require_dma_length(w0, 47, "G_OBJ_LDTX_SPRITE", command_pc)?;
                let compound_address =
                    resolve_s2dex_pointer(&segments, w1, "G_OBJ_LDTX_SPRITE", "uObjTxSprite")?;
                require_object_range(
                    rdram,
                    compound_address,
                    OBJ_TX_SPRITE_BYTES,
                    "G_OBJ_LDTX_SPRITE",
                )?;
                let texture =
                    read_object_texture(rdram, compound_address, &segments, "G_OBJ_LDTX_SPRITE")?;
                let sprite_address = compound_address
                    .checked_add(OBJ_TEXTURE_BYTES as u32)
                    .ok_or_else(|| reject("G_OBJ_LDTX_SPRITE uObjSprite address overflow"))?;
                let sprite = read_object_sprite(rdram, sprite_address, "G_OBJ_LDTX_SPRITE")?;
                let matrix = require_rotation_matrix(
                    object_matrix,
                    rotation_matrix_loaded,
                    "G_OBJ_LDTX_SPRITE",
                    command_pc,
                    true,
                )?;
                apply_object_texture(
                    rdram,
                    texture,
                    &mut object_status,
                    &mut object_texture_scratch,
                    &mut speculative_rdp,
                    "G_OBJ_LDTX_SPRITE",
                )?;
                operations.extend(object_sprite_ops(
                    &mut speculative_rdp,
                    sprite,
                    matrix,
                    object_render_mode,
                    "G_OBJ_LDTX_SPRITE",
                )?);
            }
            Some(command @ (S2dexCommand::BgCopy | S2dexCommand::Bg1Cyc)) => {
                let name = command.name();
                require_dma_length(w0, 0, name, command_pc)?;
                let background_address = resolve_s2dex_pointer(&segments, w1, name, "uObjBg")?;
                let background = read_background(rdram, background_address, &segments, command)?;
                operations.extend(background_ops(
                    rdram,
                    background,
                    &mut speculative_rdp,
                    name,
                )?);
            }
            Some(S2dexCommand::ObjRenderMode) => {
                if w0 & 0x00ff_ffff != 0 {
                    return Err(reject(format!(
                        "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} has nonzero reserved payload {:#08x}",
                        w0 & 0x00ff_ffff
                    )));
                }
                object_render_mode = read_object_render_mode(w1, command_pc)?;
            }
            Some(S2dexCommand::MoveWord) => {
                let (index, offset) = move_word_fields(family, w0);
                match index {
                    G_MW_SEGMENT => {
                        if !offset.is_multiple_of(4) || offset / 4 >= 16 {
                            return Err(reject(format!(
                                "G_MOVEWORD G_MW_SEGMENT at RDRAM {command_pc:#010x} has offset {offset:#06x}; public segment offsets are aligned 0..=60"
                            )));
                        }
                        segments[usize::from(offset / 4)] = w1 & 0x00ff_ffff;
                    }
                    G_MW_GENSTAT => {
                        if !matches!(offset, 0 | 4 | 8 | 12) {
                            return Err(reject(format!(
                                "G_MOVEWORD G_MW_GENSTAT at RDRAM {command_pc:#010x} has status ID {offset}, outside 0,4,8,12"
                            )));
                        }
                        object_status[usize::from(offset / 4)] = w1;
                    }
                    _ => {
                        return Err(unsupported(
                            "render.s2dex.moveword-index",
                            format!(
                                "unsupported S2DEX G_MOVEWORD index {index:#04x} at RDRAM {command_pc:#010x}: offset={offset:#06x} data={w1:#010x}"
                            ),
                        ));
                    }
                }
            }
            Some(S2dexCommand::RdpHalf0) => {
                let sid = ((w0 >> 16) & 0xff) as u8;
                if !matches!(sid, 0 | 4 | 8 | 12) {
                    return Err(reject(format!(
                        "G_RDPHALF_0 at RDRAM {command_pc:#010x} stages G_SELECT_DL status ID {sid}, outside 0,4,8,12"
                    )));
                }
                pending_select = Some(PendingSelectDl {
                    sid,
                    flag: w1,
                    target_low: w0 as u16,
                });
            }
            Some(S2dexCommand::SelectDl) => {
                let staged = pending_select.take().ok_or_else(|| {
                    reject(format!(
                        "G_SELECT_DL at RDRAM {command_pc:#010x} is missing its preceding G_RDPHALF_0"
                    ))
                })?;
                let push = ((w0 >> 16) & 0xff) as u8;
                if !matches!(push, 0 | 1) {
                    return Err(reject(format!(
                        "G_SELECT_DL at RDRAM {command_pc:#010x} has push selector {push}, expected G_DL_PUSH=0 or G_DL_NOPUSH=1"
                    )));
                }
                let slot = usize::from(staged.sid / 4);
                if object_status[slot] & w1 != staged.flag {
                    object_status[slot] = (object_status[slot] & !w1) | (staged.flag & w1);
                    let target = (u32::from(w0 as u16) << 16) | u32::from(staged.target_low);
                    let target = resolve_s2dex_pointer(&segments, target, "G_SELECT_DL", "target")?;
                    if !target.is_multiple_of(8) {
                        return Err(reject(format!(
                            "G_SELECT_DL target {target:#010x} is not 8-byte aligned"
                        )));
                    }
                    let target = target as usize;
                    if target >= PHYSICAL_RDRAM_BYTES || target + 8 > rdram.len() {
                        return Err(reject(format!(
                            "G_SELECT_DL target {target:#010x} lies outside physical/backed RDRAM"
                        )));
                    }
                    if push == 0 {
                        if return_stack.len() == MAX_DL_DEPTH {
                            return Err(reject(format!(
                                "G_SELECT_DL call depth exceeds the public {MAX_DL_DEPTH}-entry F3DEX_GBI_2 stack"
                            )));
                        }
                        return_stack.push(pc);
                    }
                    pc = target;
                }
            }
            Some(S2dexCommand::EndDl) => {
                if w0 & 0x00ff_ffff != 0 || w1 != 0 {
                    return Err(reject(format!(
                        "G_ENDDL at {command_pc:#010x} has nonzero reserved payload: w0={w0:#010x} w1={w1:#010x}"
                    )));
                }
                if let Some(return_pc) = return_stack.pop() {
                    pc = return_pc;
                } else {
                    *rdp = speculative_rdp;
                    return Ok(operations);
                }
            }
            None => {
                return Err(unsupported(
                    "render.s2dex.command",
                    format!(
                        "unsupported {family:?} command byte {opcode:#04x} at RDRAM {command_pc:#010x}: w0={w0:#010x} w1={w1:#010x}"
                    ),
                ));
            }
        }

        if command_index + 1 == MAX_COMMANDS {
            return Err(reject(format!(
                "display list exceeded the {MAX_COMMANDS}-command budget; missing G_ENDDL or cyclic graph"
            )));
        }
    }
    unreachable!("bounded S2DEX command loop exits through a result")
}

fn read_object_matrix_command(
    rdram: &[u8],
    w0: u32,
    address: u32,
    previous: Option<ObjectMatrix>,
    command_pc: usize,
) -> Result<(ObjectMatrix, bool), RenderError> {
    let parameter = ((w0 >> 16) & 0xff) as u8;
    let length = (w0 & 0xffff) as u16;
    match (parameter, length) {
        (0, 23) => {
            let address = require_object_range(rdram, address, 24, "G_OBJ_MOVEMEM ObjMatrix")?;
            let view = fn64_runtime::RdramView::from_storage(rdram);
            let base = fn64_runtime::RdramAddr::from_offset(address as u32);
            let word = |offset| {
                view.read_u32(base.checked_add(offset).expect("uObjMtx offset fits")) as i32
            };
            let half =
                |offset| view.read_u16(base.checked_add(offset).expect("uObjMtx offset fits"));
            Ok((
                ObjectMatrix {
                    a: word(0),
                    b: word(4),
                    c: word(8),
                    d: word(12),
                    x: half(16) as i16,
                    y: half(18) as i16,
                    base_scale_x: half(20),
                    base_scale_y: half(22),
                },
                true,
            ))
        }
        (2, 7) => {
            let address = require_object_range(rdram, address, 8, "G_OBJ_MOVEMEM ObjSubMatrix")?;
            let view = fn64_runtime::RdramView::from_storage(rdram);
            let base = fn64_runtime::RdramAddr::from_offset(address as u32);
            let half =
                |offset| view.read_u16(base.checked_add(offset).expect("uObjSubMtx offset fits"));
            Ok((
                ObjectMatrix {
                    x: half(0) as i16,
                    y: half(2) as i16,
                    base_scale_x: half(4),
                    base_scale_y: half(6),
                    ..previous.unwrap_or_default()
                },
                false,
            ))
        }
        _ => Err(reject(format!(
            "G_OBJ_MOVEMEM at RDRAM {command_pc:#010x} has parameter={parameter} length={length}; public S2DEX admits ObjMatrix (0,23) or ObjSubMatrix (2,7)"
        ))),
    }
}

fn require_rotation_matrix(
    matrix: Option<ObjectMatrix>,
    rotation_loaded: bool,
    command: &str,
    command_pc: usize,
    compound: bool,
) -> Result<ObjectMatrix, RenderError> {
    if !rotation_loaded {
        let suffix = if compound {
            "; texture load was not applied"
        } else {
            ""
        };
        return Err(reject(format!(
            "{command} at RDRAM {command_pc:#010x} requires a preceding full G_OBJ_MOVEMEM ObjMatrix for A/B/C/D{suffix}"
        )));
    }
    matrix.ok_or_else(|| reject(format!("{command} rotation matrix state is missing")))
}

fn read_object_render_mode(mode: u32, command_pc: usize) -> Result<ObjectRenderMode, RenderError> {
    if mode & !G_OBJRM_ALL != 0 {
        return Err(reject(format!(
            "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} has unknown flags {:#010x}",
            mode & !G_OBJRM_ALL
        )));
    }
    if mode & G_OBJRM_SHRINKSIZE_1 != 0 && mode & G_OBJRM_SHRINKSIZE_2 != 0 {
        return Err(reject(format!(
            "G_OBJ_RENDERMODE at RDRAM {command_pc:#010x} combines mutually exclusive G_OBJRM_SHRINKSIZE_1 and G_OBJRM_SHRINKSIZE_2"
        )));
    }
    Ok(ObjectRenderMode {
        texture_clamp: if mode & G_OBJRM_NOTXCLAMP != 0 {
            ObjectTextureClamp::Disabled
        } else {
            ObjectTextureClamp::Perimeter
        },
        filter_correction: if mode & G_OBJRM_BILERP != 0 {
            ObjectFilterCorrection::Bilinear
        } else {
            ObjectFilterCorrection::PointOrAverage
        },
        perimeter: ObjectPerimeter {
            shrink_half_texels: if mode & G_OBJRM_SHRINKSIZE_2 != 0 {
                2
            } else if mode & G_OBJRM_SHRINKSIZE_1 != 0 {
                1
            } else {
                0
            },
            widen_three_eighths_texel: mode & G_OBJRM_WIDEN != 0,
        },
        ignored_edge_flags: IgnoredObjectEdgeFlags {
            xlu: mode & G_OBJRM_XLU != 0,
            antialias: mode & G_OBJRM_ANTIALIAS != 0,
        },
    })
}

fn object_rectangle_op(
    rdp: &mut RdpDecodeState,
    mut sprite: ObjectSprite,
    object_mode: ObjectRenderMode,
    command: &str,
) -> Result<RenderOp, RenderError> {
    if sprite.image_flags & !(G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT) != 0 {
        return Err(reject(format!(
            "{command} imageFlags={:#04x} contains bits outside G_OBJ_FLAG_FLIPS|G_OBJ_FLAG_FLIPT",
            sprite.image_flags
        )));
    }
    let image_flags = sprite.image_flags;
    sprite.image_flags = 0;
    let average_shrink =
        ObjectAverageShrinkFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let unclamped_point =
        ObjectUnclampedPointFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let unclamped_average =
        ObjectUnclampedAverageFootprint::from_mode(object_mode, rdp.texture_filter(), command)?;
    let mut filter_validation_mode = average_shrink
        .map(|footprint| footprint.filter_validation_mode(object_mode))
        .unwrap_or(object_mode);
    if unclamped_average.is_some() {
        // Lowering retains a clamped tile, but the typed proof below makes
        // that state unobservable for every addressed Average neighbour.
        filter_validation_mode.texture_clamp = ObjectTextureClamp::Perimeter;
    }
    let mut operation = rdp.object_rectangle_with_mode(sprite, filter_validation_mode)?;
    let RenderOp::TextureRectangle(rectangle) = &mut operation else {
        unreachable!("object rectangle lowering has one typed result")
    };
    // The current public gs2dex.h marks both legacy edge flags Ignored. Keep
    // them typed so an older revision can never acquire guessed behavior.
    let _ignored_by_current_public_header = object_mode.ignored_edge_flags;
    debug_assert_eq!(
        unclamped_point.is_some() || unclamped_average.is_some(),
        object_mode.texture_clamp == ObjectTextureClamp::Disabled
    );
    if object_mode.widens() && rectangle.other_mode.texture_filter() != TextureFilter::Point {
        return Err(unsupported(
            "render.s2dex.widen-filter-footprint",
            format!(
                "{command} G_OBJRM_WIDEN with filtered sampling requires unpublished perimeter filter arithmetic"
            ),
        ));
    }
    let source_width = f32::from(sprite.image_w / 32);
    let source_height = f32::from(sprite.image_h / 32);
    let shrink_half_texels = object_mode.shrink_half_texels();
    let shrink = f32::from(shrink_half_texels) * 0.5;
    if (shrink_half_texels != 0 || object_mode.widens())
        && rectangle.other_mode.cycle_type() == CycleType::Copy
    {
        return Err(unsupported(
            "render.s2dex.copy-perimeter",
            format!(
                "{command} Copy cycle does not support G_OBJRM_SHRINKSIZE/G_OBJRM_WIDEN subpixel perimeter processing"
            ),
        ));
    }
    if shrink * 2.0 >= source_width || shrink * 2.0 >= source_height {
        return Err(reject(format!(
            "{command} shrink perimeter {shrink} texels leaves no positive image area in {source_width}x{source_height}"
        )));
    }
    if object_mode.widens() && image_flags != 0 {
        return Err(unsupported(
            "render.s2dex.widen-flip-edge",
            format!(
                "{command} G_OBJRM_WIDEN with flipped S/T requires unpublished positive-edge selection"
            ),
        ));
    }
    let (shrink_x, widen_x) =
        object_mode
            .perimeter
            .exact_screen_adjustments(sprite.scale_w, "X", command)?;
    let (shrink_y, widen_y) =
        object_mode
            .perimeter
            .exact_screen_adjustments(sprite.scale_h, "Y", command)?;
    rectangle.lrx += widen_x - shrink_x;
    rectangle.lry += widen_y - shrink_y;
    if image_flags & G_OBJ_FLAG_FLIPS != 0 {
        rectangle.s = average_shrink.map_or(source_width - 1.0 - shrink, |footprint| {
            footprint.rectangle_start(sprite.image_w, true)
        });
        rectangle.dsdx = -rectangle.dsdx;
    } else {
        rectangle.s = average_shrink
            .map(|footprint| footprint.rectangle_start(sprite.image_w, false))
            .unwrap_or(shrink);
    }
    if image_flags & G_OBJ_FLAG_FLIPT != 0 {
        rectangle.t = average_shrink.map_or(source_height - 1.0 - shrink, |footprint| {
            footprint.rectangle_start(sprite.image_h, true)
        });
        rectangle.dtdy = -rectangle.dtdy;
    } else {
        rectangle.t = average_shrink
            .map(|footprint| footprint.rectangle_start(sprite.image_h, false))
            .unwrap_or(shrink);
    }
    if let Some(footprint) = average_shrink {
        let _average_axis_footprints = (
            footprint.validate_axis(
                rectangle.s,
                f32::from(rectangle.dsdx) / 1024.0,
                rectangle.ulx,
                rectangle.lrx,
                sprite.image_w / 32,
                "S",
                command,
            )?,
            footprint.validate_axis(
                rectangle.t,
                f32::from(rectangle.dtdy) / 1024.0,
                rectangle.uly,
                rectangle.lry,
                sprite.image_h / 32,
                "T",
                command,
            )?,
        );
    }
    if let Some(footprint) = unclamped_point {
        // Copy has its own inclusive raster command and cannot combine with
        // subpixel perimeter processing. Preserve the already-admitted
        // no-perimeter Copy case; this proof owns one/two-cycle point samples.
        if rectangle.other_mode.cycle_type() != CycleType::Copy {
            let _unclamped_axis_footprints = (
                footprint.validate_axis(
                    rectangle.s,
                    f32::from(rectangle.dsdx) / 1024.0,
                    rectangle.ulx,
                    rectangle.lrx,
                    sprite.image_w / 32,
                    "S",
                    command,
                )?,
                footprint.validate_axis(
                    rectangle.t,
                    f32::from(rectangle.dtdy) / 1024.0,
                    rectangle.uly,
                    rectangle.lry,
                    sprite.image_h / 32,
                    "T",
                    command,
                )?,
            );
        }
    }
    if let Some(footprint) = unclamped_average {
        let _unclamped_axis_footprints = (
            footprint.validate_axis(
                rectangle.s,
                f32::from(rectangle.dsdx) / 1024.0,
                rectangle.ulx,
                rectangle.lrx,
                sprite.image_w / 32,
                "S",
                command,
            )?,
            footprint.validate_axis(
                rectangle.t,
                f32::from(rectangle.dtdy) / 1024.0,
                rectangle.uly,
                rectangle.lry,
                sprite.image_h / 32,
                "T",
                command,
            )?,
        );
    }
    Ok(operation)
}

fn object_sprite_ops(
    rdp: &mut RdpDecodeState,
    sprite: ObjectSprite,
    matrix: ObjectMatrix,
    object_mode: ObjectRenderMode,
    command: &str,
) -> Result<[RenderOp; 2], RenderError> {
    if rdp.texture_filter() == TextureFilter::Average && object_mode.shrink_half_texels() != 0 {
        return Err(unsupported(
            "render.s2dex.sprite-precision",
            format!(
                "{command} Average plus G_OBJRM_SHRINKSIZE on a rotating polygon requires a separately evidenced pixel-center coordinate correction"
            ),
        ));
    }
    let RenderOp::TextureRectangle(snapshot) =
        object_rectangle_op(rdp, sprite, object_mode, command)?
    else {
        unreachable!("object rectangle lowering has one typed result")
    };
    let cycle_type = snapshot.other_mode.cycle_type();
    if object_mode.texture_clamp == ObjectTextureClamp::Disabled {
        return Err(unsupported(
            "render.s2dex.sprite-tmem-addressing",
            format!(
                "{command} G_OBJRM_NOTXCLAMP on a polygon requires unpublished out-of-domain TMEM addressing"
            ),
        ));
    }
    if !matches!(cycle_type, CycleType::OneCycle | CycleType::TwoCycle) {
        return Err(unsupported(
            "render.s2dex.sprite-cycle",
            format!(
                "{command} polygon lowering supports one-cycle or two-cycle mode, got {cycle_type:?}"
            ),
        ));
    }
    if snapshot.other_mode.depth_compare_enabled()
        || snapshot.other_mode.depth_update_enabled()
        || snapshot.other_mode.primitive_depth_source()
    {
        return Err(unsupported(
            "render.s2dex.sprite-depth",
            format!("{command} depth state requires an evidenced S2DEX sprite Z policy"),
        ));
    }
    let texture0 = snapshot.texture.ok_or_else(|| {
        reject(format!(
            "{command} requires loaded TMEM for its documented textured quad"
        ))
    })?;
    let requires_texel1 = snapshot.combiner.mode.uses_texel1(cycle_type);
    if requires_texel1 && snapshot.texture1.is_none() {
        return Err(unsupported(
            "render.s2dex.sprite-combiner",
            format!("{command} combiner selects TEXEL1 without an initialized tile 1 image"),
        ));
    }
    // Section 4.2.5 defines the rotating object as two ordinary textured
    // polygons and assigns it the same texture settings as G_OBJ_RECTANGLE.
    // Preserve both no-LOD tiles in the shared immutable triangle snapshot:
    // the public RDP combiner defines TEXEL1 as the tile after TEXEL0.
    let mut tiles = std::array::from_fn(|_| None);
    tiles[0] = Some(texture0.clone());
    tiles[1] = snapshot.texture1;
    let texture = texture0.with_lod_snapshot(tiles, 0, 0);

    let exact_extent = |image: u16, scale: u16, axis: &str| -> Result<i64, RenderError> {
        let numerator = i64::from(image) * 128;
        if numerator % i64::from(scale) != 0 {
            return Err(unsupported(
                "render.s2dex.sprite-precision",
                format!(
                    "{command} {axis} extent requires unimplemented sub-quarter-pixel division: image={image} scale={scale}"
                ),
            ));
        }
        Ok(numerator / i64::from(scale))
    };
    let width = exact_extent(
        object_mode
            .perimeter
            .corrected_image_5(sprite.image_w, "width", command)?,
        sprite.scale_w,
        "X",
    )?;
    let height = exact_extent(
        object_mode
            .perimeter
            .corrected_image_5(sprite.image_h, "height", command)?,
        sprite.scale_h,
        "Y",
    )?;
    let x0 = i64::from(sprite.obj_x);
    let y0 = i64::from(sprite.obj_y);
    let x1 = x0 + width;
    let y1 = y0 + height;
    let transform = |x: i64, y: i64, axis: &str| -> Result<i16, RenderError> {
        let (first, second, origin) = if axis == "X" {
            (matrix.a, matrix.b, matrix.x)
        } else {
            (matrix.c, matrix.d, matrix.y)
        };
        let numerator = i64::from(first) * x + i64::from(second) * y;
        if numerator % (1 << 16) != 0 {
            return Err(unsupported(
                "render.s2dex.sprite-precision",
                format!(
                    "{command} transformed {axis} requires unimplemented sub-quarter-pixel matrix rounding: numerator={numerator}"
                ),
            ));
        }
        i16::try_from(i64::from(origin) + numerator / (1 << 16)).map_err(|_| {
            reject(format!(
                "{command} transformed {axis} coordinate exceeds s10.2"
            ))
        })
    };
    let corner = |x: i64, y: i64, s: f32, t: f32| -> Result<Vertex, RenderError> {
        Ok(Vertex {
            x: f32::from(transform(x, y, "X")?) / 4.0,
            y: f32::from(transform(x, y, "Y")?) / 4.0,
            r: 255,
            g: 255,
            b: 255,
            a: 255,
            s,
            t,
            w: 1.0,
            ..Vertex::default()
        })
    };
    // Rectangle rasterization evaluates S/T at its integer upper-left, while
    // triangle interpolation evaluates attributes at pixel centers. Apply the
    // public bilerp half-texel correction in the latter coordinate domain so
    // both object primitives address the same texel centers.
    let filter_correction = if object_mode.bilerp() { -0.5 } else { 0.0 };
    let (source_s_start, source_s_end) = object_mode.perimeter.source_bounds(sprite.image_w);
    let (source_t_start, source_t_end) = object_mode.perimeter.source_bounds(sprite.image_h);
    let (left_s, right_s) = if sprite.image_flags & G_OBJ_FLAG_FLIPS != 0 {
        (
            source_s_end + filter_correction,
            source_s_start + filter_correction,
        )
    } else {
        (
            source_s_start + filter_correction,
            source_s_end + filter_correction,
        )
    };
    let (top_t, bottom_t) = if sprite.image_flags & G_OBJ_FLAG_FLIPT != 0 {
        (
            source_t_end + filter_correction,
            source_t_start + filter_correction,
        )
    } else {
        (
            source_t_start + filter_correction,
            source_t_end + filter_correction,
        )
    };
    let corners = [
        corner(x0, y0, left_s, top_t)?,
        corner(x1, y0, right_s, top_t)?,
        corner(x1, y1, right_s, bottom_t)?,
        corner(x0, y1, left_s, bottom_t)?,
    ];
    let triangle = |indices: [usize; 3]| {
        RenderOp::Triangle(Triangle {
            v: [
                corners[indices[0]],
                corners[indices[1]],
                corners[indices[2]],
            ],
            scissor: snapshot.scissor,
            cull: CullMode::None,
            texture: Some(texture.clone()),
            other_mode: snapshot.other_mode,
            combiner: snapshot.combiner,
            blender: snapshot.blender,
        })
    };
    Ok([triangle([0, 1, 2]), triangle([0, 2, 3])])
}

fn matrix_relative_sprite(
    mut sprite: ObjectSprite,
    matrix: ObjectMatrix,
) -> Result<ObjectSprite, RenderError> {
    // RectangleR deliberately ignores rotation, but retaining these fields
    // makes a SubMatrix update preserve the complete public matrix for the
    // later rotating-sprite slice.
    let _rotation_terms_reserved_for_sprite = (matrix.a, matrix.b, matrix.c, matrix.d);
    let position = |object: i16, origin: i16, base_scale: u16, axis: &str| {
        if base_scale == 0 {
            return Err(reject(format!(
                "G_OBJ_RECTANGLE_R BaseScale{axis} must be nonzero"
            )));
        }
        let numerator = i64::from(object) * (1 << 10);
        if numerator % i64::from(base_scale) != 0 {
            return Err(unsupported(
                "render.s2dex.rectangle-r-precision",
                format!(
                    "G_OBJ_RECTANGLE_R {axis} position requires unimplemented sub-fixed-point division: object={object} BaseScale{axis}={base_scale}"
                ),
            ));
        }
        i16::try_from(i64::from(origin) + numerator / i64::from(base_scale)).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE_R transformed {axis} position exceeds s10.2"
            ))
        })
    };
    let scale = |object_scale: u16, base_scale: u16, axis: &str| {
        let product = u32::from(object_scale) * u32::from(base_scale);
        if product % (1 << 10) != 0 {
            return Err(unsupported(
                "render.s2dex.rectangle-r-precision",
                format!(
                    "G_OBJ_RECTANGLE_R {axis} scale requires unimplemented sub-fixed-point multiplication: scale={object_scale} BaseScale{axis}={base_scale}"
                ),
            ));
        }
        u16::try_from(product >> 10).map_err(|_| {
            reject(format!(
                "G_OBJ_RECTANGLE_R transformed {axis} scale exceeds u5.10"
            ))
        })
    };
    sprite.obj_x = position(sprite.obj_x, matrix.x, matrix.base_scale_x, "X")?;
    sprite.obj_y = position(sprite.obj_y, matrix.y, matrix.base_scale_y, "Y")?;
    sprite.scale_w = scale(sprite.scale_w, matrix.base_scale_x, "X")?;
    sprite.scale_h = scale(sprite.scale_h, matrix.base_scale_y, "Y")?;
    Ok(sprite)
}

fn require_dma_length(
    w0: u32,
    expected: u32,
    command: &str,
    command_pc: usize,
) -> Result<(), RenderError> {
    let length = w0 & 0x00ff_ffff;
    if length != expected {
        return Err(reject(format!(
            "{command} at {command_pc:#010x} has DMA length {length}, expected {expected} from public gs2dex.h"
        )));
    }
    Ok(())
}

fn require_object_range(
    rdram: &[u8],
    address: u32,
    bytes: usize,
    command: &str,
) -> Result<usize, RenderError> {
    let address_class = address >> 24;
    if !matches!(address_class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} object address {address:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let address = (address & 0x00ff_ffff) as usize;
    let end = address
        .checked_add(bytes)
        .ok_or_else(|| reject(format!("{command} object address overflow")))?;
    if !address.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} object address {address:#010x} is not 8-byte aligned"
        )));
    }
    if end > PHYSICAL_RDRAM_BYTES {
        return Err(reject(format!(
            "{command} object range [{address:#010x}, {end:#010x}) exceeds physical 8 MiB RDRAM"
        )));
    }
    if end > rdram.len() {
        return Err(reject(format!(
            "{command} object range [{address:#010x}, {end:#010x}) exceeds RDRAM length {}",
            rdram.len()
        )));
    }
    Ok(address)
}

fn read_background(
    rdram: &[u8],
    address: u32,
    segments: &[u32; 16],
    background_command: S2dexCommand,
) -> Result<Background, RenderError> {
    let command = background_command.name();
    let address = require_object_range(rdram, address, OBJ_BG_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let word = |offset| view.read_u32(base.checked_add(offset).expect("uObjBg offset fits"));
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjBg offset fits"));
    let byte = |offset| view.read_u8(base.checked_add(offset).expect("uObjBg offset fits"));
    let common = BackgroundCommon {
        image_x: half(0),
        image_w: half(2),
        frame_x: half(4) as i16,
        frame_w: half(6),
        image_y: half(8),
        image_h: half(10),
        frame_y: half(12) as i16,
        frame_h: half(14),
        image: resolve_s2dex_pointer(segments, word(16), command, "background image")?,
        image_load: half(20),
        image_format: byte(22),
        image_size: byte(23),
        image_palette: half(24),
        image_flip: half(26),
    };
    if background_command == S2dexCommand::BgCopy {
        Ok(Background::Copy {
            common,
            tmem_w: half(28),
            tmem_h: half(30),
            tmem_load_sh: half(32),
            tmem_load_th: half(34),
            tmem_size_w: half(36),
            tmem_size: half(38),
        })
    } else {
        if (36..40).any(|offset| byte(offset) != 0) {
            return Err(reject(format!(
                "{command} uObjScaleBg padding[4] must be zero"
            )));
        }
        Ok(Background::Scale {
            common,
            scale_w: half(28),
            scale_h: half(30),
            image_y_origin: word(32) as i32,
        })
    }
}

fn background_ops(
    rdram: &[u8],
    background: Background,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let (common, scale_w, scale_h, copy_tmem_rows, image_y_origin) = match background {
        Background::Copy {
            common,
            tmem_w,
            tmem_h,
            tmem_load_sh,
            tmem_load_th,
            tmem_size_w,
            tmem_size,
        } => {
            validate_copy_background_init(
                common,
                [
                    tmem_w,
                    tmem_h,
                    tmem_load_sh,
                    tmem_load_th,
                    tmem_size_w,
                    tmem_size,
                ],
                command,
            )?;
            (common, 1 << 10, 1 << 10, Some(tmem_h / 4), None)
        }
        Background::Scale {
            common,
            scale_w,
            scale_h,
            image_y_origin,
        } => (common, scale_w, scale_h, None, Some(image_y_origin)),
    };
    if !matches!(common.image_load, G_BGLT_LOADBLOCK | G_BGLT_LOADTILE) {
        return Err(reject(format!(
            "{command} imageLoad={:#06x} is not G_BGLT_LOADBLOCK or G_BGLT_LOADTILE",
            common.image_load
        )));
    }
    if common.image_format > 4 || common.image_size > 3 {
        return Err(reject(format!(
            "{command} image format={} size={} is outside public G_IM_FMT/G_IM_SIZ encodings",
            common.image_format, common.image_size
        )));
    }
    if common.image_palette > 7 {
        return Err(reject(format!(
            "{command} imagePal={} is outside the public S2DEX range 0..=7",
            common.image_palette
        )));
    }
    if !matches!(common.image_flip, 0 | G_BG_FLAG_FLIPS) {
        return Err(unsupported(
            "render.s2dex.background-flags",
            format!(
                "{command} imageFlip={:#06x} requests unsupported vertical/reserved flags",
                common.image_flip
            ),
        ));
    }
    if common.image_w == 0
        || common.image_h == 0
        || common.frame_w == 0
        || common.frame_h == 0
        || common.image_w > 0x0fff
        || common.image_h > 0x0fff
        || common.frame_w > 0x0fff
        || common.frame_h > 0x0fff
        || !common.image_w.is_multiple_of(4)
        || !common.image_h.is_multiple_of(4)
        || !common.frame_w.is_multiple_of(4)
        || !common.frame_h.is_multiple_of(4)
    {
        return Err(reject(format!(
            "{command} requires positive whole-pixel u10.2 image/frame dimensions within 0x0fff"
        )));
    }
    if common.frame_y & 3 != 0 {
        return Err(unsupported(
            "render.s2dex.background-subpixel",
            format!(
                "{command} frameY={} requests unsupported vertical subpixel movement",
                common.frame_y
            ),
        ));
    }
    if command == "G_BG_COPY"
        && (common.image_x & 31 != 0 || common.image_y & 31 != 0 || common.frame_x & 3 != 0)
    {
        return Err(reject(format!(
            "{command} requires integer image/frame origins"
        )));
    }
    if scale_w == 0 || scale_h == 0 || scale_w > i16::MAX as u16 || scale_h > i16::MAX as u16 {
        return Err(reject(format!(
            "{command} scaleW={scale_w} scaleH={scale_h} is outside the nonzero RDP S5.10 gradient range"
        )));
    }

    let image_width = u32::from(common.image_w / 4);
    let image_height = u32::from(common.image_h / 4);
    let frame_width = u32::from(common.frame_w / 4);
    let frame_height = u32::from(common.frame_h / 4);
    let bits_per_texel = 4u32 << common.image_size;
    let row_bits = image_width
        .checked_mul(bits_per_texel)
        .ok_or_else(|| reject(format!("{command} image row size overflow")))?;
    if row_bits % 64 != 0 {
        return Err(reject(format!(
            "{command} imageW={} pixels does not satisfy the public 8-byte row alignment for size {}",
            image_width, common.image_size
        )));
    }
    let row_bytes = row_bits / 8;
    let image_bytes = row_bytes
        .checked_mul(image_height)
        .ok_or_else(|| reject(format!("{command} image byte size overflow")))?;
    let image = physical_pointer(common.image, command, "background image")?;
    if command == "G_BG_1CYC" && image < 0x1000 {
        return Err(reject(format!(
            "{command} background image {image:#010x} violates the public >=0x1000 physical-address restriction"
        )));
    }
    let image_end = image
        .checked_add(image_bytes)
        .ok_or_else(|| reject(format!("{command} image range overflow")))?;
    if image_end as usize > PHYSICAL_RDRAM_BYTES || image_end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} image range [{image:#010x}, {image_end:#010x}) exceeds physical/backed RDRAM"
        )));
    }

    if command == "G_BG_COPY" {
        let copy_tmem_rows =
            copy_tmem_rows.expect("validated G_BG_COPY carries initialized TMEM row capacity");
        let window = BackgroundCopyWindow::new(
            image_width,
            image_height,
            frame_width,
            frame_height,
            u32::from(common.image_x / 32),
            u32::from(common.image_y / 32),
            common.image_flip == G_BG_FLAG_FLIPS,
            u32::from(copy_tmem_rows),
            command,
        )?;
        return copy_background_ops(rdram, common, image, rdp, window, command);
    }
    let window = ScaledBackgroundWindow::new(
        image_width,
        image_height,
        frame_width,
        frame_height,
        common.image_x,
        common.image_y,
        scale_w,
        scale_h,
        common.image_flip == G_BG_FLAG_FLIPS,
        image_y_origin.expect("validated G_BG_1CYC carries imageYorig"),
        command,
    )?;
    scaled_background_ops(rdram, common, image, rdp, window, command)
}

fn copy_background_ops(
    rdram: &[u8],
    common: BackgroundCommon,
    image: u32,
    rdp: &mut RdpDecodeState,
    window: BackgroundCopyWindow,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let mut operations = Vec::new();
    let mut scratch = BackgroundScratch::new();
    for slice in window.slices() {
        let (load_source_x_start, load_source_x_end) = if common.image_load == G_BGLT_LOADBLOCK {
            (0, window.image_width)
        } else {
            (slice.source_x_start, slice.source_x_end)
        };
        let mut rectangle = load_background_tile(
            rdram,
            &mut scratch,
            common,
            image,
            window.image_width,
            load_source_x_start,
            slice.source_y_start,
            load_source_x_end,
            slice.source_y_end,
            rdp,
            command,
        )?;
        if rectangle.other_mode.cycle_type() != CycleType::Copy {
            return Err(reject(format!(
                "{command} requires Copy cycle, got {:?}",
                rectangle.other_mode.cycle_type()
            )));
        }
        if common.image_format == 2 && rectangle.other_mode.texture_lut() == 0 {
            return Err(reject(format!(
                "{command} CI background requires an active RGBA16 or IA16 texture-LUT mode"
            )));
        }

        let frame_x = f32::from(common.frame_x) / 4.0;
        let frame_y = f32::from(common.frame_y) / 4.0;
        rectangle.ulx = frame_x + slice.output_x_start as f32;
        rectangle.uly = frame_y + slice.output_y_start as f32;
        rectangle.lrx = frame_x + slice.output_x_end as f32 - 1.0;
        rectangle.lry = frame_y + slice.output_y_end as f32 - 1.0;
        rectangle.s = if slice.reverse_s {
            (slice.source_x_end - 1 - load_source_x_start) as f32
        } else {
            (slice.source_x_start - load_source_x_start) as f32
        };
        rectangle.t = 0.0;
        rectangle.dsdx = if slice.reverse_s { -(4 << 10) } else { 4 << 10 };
        rectangle.dtdy = 1 << 10;
        operations.push(RenderOp::TextureRectangle(rectangle));
    }
    Ok(operations)
}

fn scaled_background_ops(
    rdram: &[u8],
    common: BackgroundCommon,
    image: u32,
    rdp: &mut RdpDecodeState,
    window: ScaledBackgroundWindow,
    command: &str,
) -> Result<Vec<RenderOp>, RenderError> {
    let footprint = BackgroundFilterFootprint::from_rdp(rdp.texture_filter(), command)?;
    let slices = window.slices(footprint, command)?;
    let bits_per_texel = 4u32 << common.image_size;
    let tmem_capacity = if common.image_format == 2 { 256 } else { 512 };
    let mut operations = Vec::with_capacity(slices.len());
    let mut scratch = BackgroundScratch::new();
    for slice in slices {
        let (load_source_x_start, load_source_x_end) = if common.image_load == G_BGLT_LOADBLOCK {
            (0, window.image_width)
        } else {
            (slice.source_x_start, slice.source_x_end)
        };
        let source_width = load_source_x_end - load_source_x_start;
        let line_words = source_width
            .checked_mul(bits_per_texel)
            .and_then(|bits| bits.checked_add(63))
            .map(|bits| bits / 64)
            .ok_or_else(|| reject(format!("{command} TMEM line size overflow")))?;
        if line_words == 0 || line_words > tmem_capacity || line_words > 511 || source_width > 1024
        {
            return Err(reject(format!(
                "{command} source span width={source_width} line_words={line_words} exceeds one TMEM row"
            )));
        }

        let mut rectangle = load_background_tile(
            rdram,
            &mut scratch,
            common,
            image,
            window.image_width,
            load_source_x_start,
            slice.source_y,
            load_source_x_end,
            slice.source_y + 1,
            rdp,
            command,
        )?;
        if rectangle.other_mode.cycle_type() != CycleType::OneCycle {
            return Err(reject(format!(
                "{command} requires OneCycle mode, got {:?}",
                rectangle.other_mode.cycle_type()
            )));
        }
        debug_assert_eq!(rectangle.other_mode.texture_filter(), TextureFilter::Point);
        if common.image_format == 2 && rectangle.other_mode.texture_lut() == 0 {
            return Err(reject(format!(
                "{command} CI background requires an active RGBA16 or IA16 texture-LUT mode"
            )));
        }

        let frame_x = f32::from(common.frame_x) / 4.0;
        let frame_y = f32::from(common.frame_y) / 4.0;
        rectangle.ulx = frame_x + slice.output_x_start as f32;
        rectangle.uly = frame_y + slice.output_y as f32;
        rectangle.lrx = frame_x + slice.output_x_end as f32;
        rectangle.lry = frame_y + slice.output_y as f32 + 1.0;
        rectangle.s =
            (slice.source_x_start - load_source_x_start) as f32 + slice.s_start_10 as f32 / 1024.0;
        rectangle.t = slice.t_start_10 as f32 / 1024.0;
        rectangle.dsdx =
            i16::try_from(slice.dsdx_10).expect("validated scaled-background S gradient fits i16");
        rectangle.dtdy = window.scale_h_10 as i16;
        operations.push(RenderOp::TextureRectangle(rectangle));
    }
    Ok(operations)
}

fn physical_pointer(pointer: u32, command: &str, field: &str) -> Result<u32, RenderError> {
    let class = pointer >> 24;
    if !matches!(class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} {field} {pointer:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let pointer = pointer & 0x00ff_ffff;
    if !pointer.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} {field} {pointer:#010x} is not 8-byte aligned"
        )));
    }
    Ok(pointer)
}

fn resolve_s2dex_pointer(
    segments: &[u32; 16],
    pointer: u32,
    command: &str,
    field: &str,
) -> Result<u32, RenderError> {
    let class = pointer >> 24;
    if matches!(class, 0x80 | 0xa0) {
        return Ok(pointer & 0x00ff_ffff);
    }
    if class > 0x0f {
        return Err(reject(format!(
            "{command} {field} pointer {pointer:#010x} has non-public segment byte {class:#04x}"
        )));
    }
    segments[class as usize]
        .checked_add(pointer & 0x00ff_ffff)
        .filter(|resolved| *resolved < 0x0100_0000)
        .ok_or_else(|| {
            reject(format!(
                "{command} {field} pointer {pointer:#010x} overflows the 24-bit segmented address domain"
            ))
        })
}

fn validate_copy_background_init(
    common: BackgroundCommon,
    observed: [u16; 6],
    command: &str,
) -> Result<(), RenderError> {
    if common.image_size > 3 || common.image_w == 0 || common.frame_w == 0 {
        return Err(reject(format!(
            "{command} cannot validate guS2DInitBg fields for imageSiz={} imageW={} frameW={}",
            common.image_size, common.image_w, common.frame_w
        )));
    }
    let image_width = u32::from(common.image_w / 4);
    let frame_width = u32::from(common.frame_w / 4);
    let shift = 4 - u32::from(common.image_size);
    let image_words = image_width >> shift;
    let frame_words = frame_width >> shift;
    let tmem_w = match common.image_load {
        G_BGLT_LOADBLOCK => image_words,
        G_BGLT_LOADTILE => frame_words + 1,
        _ => {
            return Err(reject(format!(
                "{command} imageLoad={:#06x} is not public",
                common.image_load
            )));
        }
    };
    if tmem_w == 0 {
        return Err(reject(format!("{command} guS2DInitBg computed zero tmemW")));
    }
    let capacity = if common.image_format == 2 { 256 } else { 512 };
    let tmem_h = (capacity / tmem_w) * 4;
    if tmem_h == 0 {
        return Err(reject(format!(
            "{command} guS2DInitBg geometry cannot fit one image row in TMEM"
        )));
    }
    let tmem_size_w = match common.image_load {
        G_BGLT_LOADBLOCK => tmem_w * 2,
        G_BGLT_LOADTILE => image_words * 2,
        _ => unreachable!(),
    };
    let tmem_size = tmem_size_w
        .checked_mul(tmem_h)
        .ok_or_else(|| reject(format!("{command} guS2DInitBg tmemSize overflow")))?;
    let tmem_load_sh = match common.image_load {
        G_BGLT_LOADBLOCK => tmem_size / 2 - 1,
        G_BGLT_LOADTILE => tmem_w * 16 - 1,
        _ => unreachable!(),
    };
    let tmem_load_th = match common.image_load {
        G_BGLT_LOADBLOCK => 2047 / tmem_w + 1,
        G_BGLT_LOADTILE => tmem_h - 1,
        _ => unreachable!(),
    };
    let expected_u32 = [
        tmem_w,
        tmem_h,
        tmem_load_sh,
        tmem_load_th,
        tmem_size_w,
        tmem_size,
    ];
    let mut expected = [0u16; 6];
    for (index, value) in expected_u32.into_iter().enumerate() {
        expected[index] = u16::try_from(value).map_err(|_| {
            reject(format!(
                "{command} guS2DInitBg derived field {index}={value} exceeds u16"
            ))
        })?;
    }
    if observed != expected {
        return Err(reject(format!(
            "{command} uObjBg guS2DInitBg fields are stale/uninitialized: observed={observed:?} expected={expected:?}"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn load_background_tile(
    rdram: &[u8],
    scratch: &mut BackgroundScratch,
    common: BackgroundCommon,
    image: u32,
    image_width: u32,
    source_x_start: u32,
    source_y_start: u32,
    source_x_end: u32,
    source_y_end: u32,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<crate::gbi::TextureRectangle, RenderError> {
    if source_x_end > 1024 || source_y_end > 1024 {
        return Err(reject(format!(
            "{command} source tile ({source_x_start},{source_y_start})..({source_x_end},{source_y_end}) exceeds public RDP tile coordinates"
        )));
    }
    let width = source_x_end - source_x_start;
    let height = source_y_end - source_y_start;
    let bits_per_texel = 4u32 << common.image_size;
    let line_words = (width * bits_per_texel).div_ceil(64);
    // Every independently loaded strip is rebased to staging T=0. LoadBlock
    // is a one-dimensional transfer: its low command coordinate is S, so
    // encoding source-row parity there would skip texels instead of selecting
    // an odd source row. Multi-row parity is retained naturally within the
    // rebased strip by DXT.
    let staged_y = 0;
    let staged_rows = staged_y + height;
    let staged_bytes = usize::try_from(
        width
            .checked_mul(staged_rows)
            .and_then(|texels| texels.checked_mul(bits_per_texel))
            .ok_or_else(|| reject(format!("{command} staged image size overflow")))?
            .div_ceil(8),
    )
    .expect("bounded background staging size fits usize");
    let command_start = (staged_bytes + 7) & !7;
    let command_end = command_start + 5 * 8;
    if command_end > scratch.bytes.len() {
        return Err(reject(format!(
            "{command} staged strip requires {command_end} bytes, exceeding the bounded {}-byte background scratch",
            scratch.bytes.len()
        )));
    }
    scratch.bytes[..command_end].fill(0);
    copy_background_texels(
        rdram,
        &mut scratch.bytes,
        common.image_size,
        image,
        image_width,
        source_x_start,
        source_y_start,
        width,
        height,
        staged_y,
    );
    let settimg = (u32::from(RDP_SETTIMG) << 24)
        | (u32::from(common.image_format) << 21)
        | (u32::from(common.image_size) << 19)
        | (width - 1);
    let load_line = if common.image_load == G_BGLT_LOADBLOCK {
        0
    } else {
        line_words
    };
    let settile = (u32::from(RDP_SETTILE) << 24)
        | (u32::from(common.image_format) << 21)
        | (u32::from(common.image_size) << 19)
        | (load_line << 9);
    let load_tile = 7 << 24;
    let load_command = if common.image_load == G_BGLT_LOADBLOCK {
        if source_x_start != 0 {
            return Err(reject(format!(
                "{command} LoadBlock lowering requires a full source row"
            )));
        }
        let count = width
            .checked_mul(height)
            .ok_or_else(|| reject(format!("{command} LoadBlock texel count overflow")))?;
        if count == 0 || count > 4096 {
            return Err(reject(format!(
                "{command} LoadBlock count={count} exceeds the public 12-bit span"
            )));
        }
        let dxt = 2047 / line_words + 1;
        (
            (u32::from(RDP_LOADBLOCK) << 24) | staged_y,
            load_tile | ((count - 1) << 12) | dxt,
        )
    } else {
        (
            (u32::from(RDP_LOADTILE) << 24) | (staged_y << 2),
            load_tile | ((width - 1) << 14) | ((staged_y + height - 1) << 2),
        )
    };
    let commands = [
        (settimg, 0),
        (settile, load_tile),
        (u32::from(RDP_LOADSYNC) << 24, 0),
        load_command,
        (u32::from(G_ENDDL) << 24, 0),
    ];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = command_start + index * 8;
        scratch.bytes[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        scratch.bytes[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let load_ops = crate::gbi::decode_raw_rdp_ops_with_state(
        &scratch.bytes[..command_end],
        command_start as u32,
        rdp,
    )?;
    if !load_ops.is_empty() {
        return Err(reject(format!(
            "{command} background load unexpectedly emitted {} operations",
            load_ops.len()
        )));
    }
    let sprite = ObjectSprite {
        obj_x: 0,
        scale_w: 1 << 10,
        image_w: u16::try_from(width * 32)
            .map_err(|_| reject(format!("{command} loaded tile width exceeds u10.5")))?,
        padding_x: 0,
        obj_y: 0,
        scale_h: 1 << 10,
        image_h: u16::try_from(height * 32)
            .map_err(|_| reject(format!("{command} loaded tile height exceeds u10.5")))?,
        padding_y: 0,
        image_stride: u16::try_from(line_words)
            .map_err(|_| reject(format!("{command} TMEM line exceeds u16")))?,
        image_address: 0,
        image_format: common.image_format,
        image_size: common.image_size,
        image_palette: common.image_palette as u8,
        image_flags: 0,
    };
    let RenderOp::TextureRectangle(rectangle) = rdp.object_rectangle(sprite).map_err(|error| {
        reject(format!(
            "{command} could not snapshot its loaded background tile: {error}"
        ))
    })?
    else {
        unreachable!("object rectangle lowering has one typed result")
    };
    Ok(rectangle)
}

#[allow(clippy::too_many_arguments)]
fn copy_background_texels(
    rdram: &[u8],
    scratch: &mut [u8],
    image_size: u8,
    image: u32,
    image_width: u32,
    source_x: u32,
    source_y: u32,
    width: u32,
    height: u32,
    staged_y: u32,
) {
    let source = fn64_runtime::RdramView::from_storage(rdram);
    let mut staged = fn64_runtime::RdramViewMut::from_storage(scratch);
    for y in 0..height {
        for x in 0..width {
            let source_texel = (source_y + y) * image_width + source_x + x;
            let staged_texel = (staged_y + y) * width + x;
            match image_size {
                0 => {
                    let source_byte = source.read_u8(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel / 2,
                    ));
                    let shift = if source_texel & 1 == 0 { 4 } else { 0 };
                    let texel = (source_byte >> shift) & 0x0f;
                    let staged_address = fn64_runtime::RdramAddr::from_offset(staged_texel / 2);
                    let old = staged.as_view().read_u8(staged_address);
                    let packed = if staged_texel & 1 == 0 {
                        (old & 0x0f) | (texel << 4)
                    } else {
                        (old & 0xf0) | texel
                    };
                    staged.write_u8(staged_address, packed);
                }
                1 => staged.write_u8(
                    fn64_runtime::RdramAddr::from_offset(staged_texel),
                    source.read_u8(fn64_runtime::RdramAddr::from_offset(image + source_texel)),
                ),
                2 => staged.write_u16(
                    fn64_runtime::RdramAddr::from_offset(staged_texel * 2),
                    source.read_u16(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel * 2,
                    )),
                ),
                3 => staged.write_u32(
                    fn64_runtime::RdramAddr::from_offset(staged_texel * 4),
                    source.read_u32(fn64_runtime::RdramAddr::from_offset(
                        image + source_texel * 4,
                    )),
                ),
                _ => unreachable!("background image size was validated"),
            }
        }
    }
}

fn read_object_sprite(
    rdram: &[u8],
    address: u32,
    command: &str,
) -> Result<ObjectSprite, RenderError> {
    let address = require_object_range(rdram, address, OBJ_SPRITE_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjSprite offset fits"));
    let byte = |offset| view.read_u8(base.checked_add(offset).expect("uObjSprite offset fits"));
    Ok(ObjectSprite {
        obj_x: half(0) as i16,
        scale_w: half(2),
        image_w: half(4),
        padding_x: half(6),
        obj_y: half(8) as i16,
        scale_h: half(10),
        image_h: half(12),
        padding_y: half(14),
        image_stride: half(16),
        image_address: half(18),
        image_format: byte(20),
        image_size: byte(21),
        image_palette: byte(22),
        image_flags: byte(23),
    })
}

fn read_object_texture(
    rdram: &[u8],
    address: u32,
    segments: &[u32; 16],
    command: &str,
) -> Result<ObjectTexture, RenderError> {
    let address = require_object_range(rdram, address, OBJ_TEXTURE_BYTES, command)?;
    let view = fn64_runtime::RdramView::from_storage(rdram);
    let base = fn64_runtime::RdramAddr::from_offset(address as u32);
    let word = |offset| view.read_u32(base.checked_add(offset).expect("uObjTxtr offset fits"));
    let half = |offset| view.read_u16(base.checked_add(offset).expect("uObjTxtr offset fits"));
    let common = ObjectTextureCommon {
        image: resolve_s2dex_pointer(segments, word(4), command, "texture image")?,
        sid: half(14),
        flag: word(16),
        mask: word(20),
    };
    if !matches!(common.sid, 0 | 4 | 8 | 12) {
        return Err(reject(format!(
            "{command} uObjTxtr sid={} is outside the public status IDs 0,4,8,12",
            common.sid
        )));
    }
    match word(0) {
        G_OBJLT_TXTRBLOCK => Ok(ObjectTexture::Block {
            common,
            tmem: half(8),
            tsize: half(10),
            tline: half(12),
        }),
        G_OBJLT_TXTRTILE => Ok(ObjectTexture::Tile {
            common,
            tmem: half(8),
            twidth: half(10),
            theight: half(12),
        }),
        G_OBJLT_TLUT => {
            let zero = half(12);
            if zero != 0 {
                return Err(reject(format!(
                    "{command} uObjTxtrTLUT zero field must be 0, got {zero}"
                )));
            }
            Ok(ObjectTexture::Tlut {
                common,
                phead: half(8),
                pnum: half(10),
            })
        }
        kind => Err(unsupported(
            "render.s2dex.object-texture-type",
            format!(
                "unsupported S2DEX command {command}: uObjTxtr type {kind:#010x} is not G_OBJLT_TXTRBLOCK, G_OBJLT_TXTRTILE, or G_OBJLT_TLUT"
            ),
        )),
    }
}

fn apply_object_texture(
    rdram: &[u8],
    texture: ObjectTexture,
    status: &mut [u32; 4],
    scratch: &mut ObjectTextureScratch,
    rdp: &mut RdpDecodeState,
    command: &str,
) -> Result<(), RenderError> {
    let common = texture.common();
    let slot = usize::from(common.sid / 4);
    if status[slot] & common.mask == common.flag {
        return Ok(());
    }
    let ObjectTextureRdpLoad {
        commands,
        image,
        image_bytes,
    } = object_texture_rdp_commands(rdram, texture, command)?;
    let command_start = (image_bytes + 7) & !7;
    let command_bytes = commands
        .len()
        .checked_mul(8)
        .ok_or_else(|| reject(format!("{command} synthesized RDP command length overflow")))?;
    let command_end = command_start
        .checked_add(command_bytes)
        .ok_or_else(|| reject(format!("{command} synthesized RDP range overflow")))?;
    if command_end > scratch.bytes.len() {
        return Err(reject(format!(
            "{command} bounded object-texture staging requires {command_end} bytes, exceeding its {}-byte scratch",
            scratch.bytes.len()
        )));
    }
    scratch.bytes[..command_end].fill(0);
    let source = fn64_runtime::RdramView::from_storage(rdram);
    let mut staged = fn64_runtime::RdramViewMut::from_storage(&mut scratch.bytes);
    for offset in 0..image_bytes {
        let offset = u32::try_from(offset).expect("bounded object texture size fits u32");
        staged.write_u8(
            fn64_runtime::RdramAddr::from_offset(offset),
            source.read_u8(fn64_runtime::RdramAddr::from_offset(image + offset)),
        );
    }
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = command_start + index * 8;
        scratch.bytes[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        scratch.bytes[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let operations = crate::gbi::decode_raw_rdp_ops_with_state(
        &scratch.bytes[..command_end],
        command_start as u32,
        rdp,
    )?;
    if !operations.is_empty() {
        return Err(reject(format!(
            "{command} texture-only lowering unexpectedly emitted {} render operations",
            operations.len()
        )));
    }
    status[slot] = (status[slot] & !common.mask) | (common.flag & common.mask);
    Ok(())
}

fn object_texture_rdp_commands(
    rdram: &[u8],
    texture: ObjectTexture,
    command: &str,
) -> Result<ObjectTextureRdpLoad, RenderError> {
    let common = texture.common();
    let (image, image_bytes) = require_image_range(rdram, common.image, texture, command)?;
    let settimg = (u32::from(RDP_SETTIMG) << 24) | (2 << 19);
    let settile = |line: u16, tmem: u16| {
        (u32::from(RDP_SETTILE) << 24) | (2 << 19) | (u32::from(line) << 9) | u32::from(tmem)
    };
    let load_tile = 7 << 24;
    let mut commands = match texture {
        ObjectTexture::Block {
            tmem, tsize, tline, ..
        } => {
            let high_s = (u32::from(tsize) + 1) * 4 - 1;
            vec![
                (settimg, 0),
                (settile(0, tmem), load_tile),
                (u32::from(RDP_LOADSYNC) << 24, 0),
                (
                    u32::from(RDP_LOADBLOCK) << 24,
                    load_tile | (high_s << 12) | u32::from(tline),
                ),
            ]
        }
        ObjectTexture::Tile {
            tmem,
            twidth,
            theight,
            ..
        } => {
            let width_16 = u32::from(twidth) + 1;
            let line = u16::try_from(width_16 / 4)
                .map_err(|_| reject(format!("{command} tile line exceeds u16")))?;
            vec![
                (settimg | (width_16 - 1), 0),
                (settile(line, tmem), load_tile),
                (u32::from(RDP_LOADSYNC) << 24, 0),
                (
                    u32::from(RDP_LOADTILE) << 24,
                    load_tile | ((u32::from(twidth) * 4) << 12) | u32::from(theight),
                ),
            ]
        }
        ObjectTexture::Tlut { phead, pnum, .. } => vec![
            (settimg, 0),
            (settile(0, phead), load_tile),
            (u32::from(RDP_LOADSYNC) << 24, 0),
            (
                u32::from(RDP_LOADTLUT) << 24,
                load_tile | (u32::from(pnum) << 14),
            ),
        ],
    };
    commands.push((u32::from(G_ENDDL) << 24, 0));
    Ok(ObjectTextureRdpLoad {
        commands,
        image,
        image_bytes,
    })
}

fn require_image_range(
    rdram: &[u8],
    image: u32,
    texture: ObjectTexture,
    command: &str,
) -> Result<(u32, usize), RenderError> {
    let class = image >> 24;
    if !matches!(class, 0x00 | 0x80 | 0xa0) {
        return Err(reject(format!(
            "{command} texture image {image:#010x} was not resolved to the physical 24-bit domain"
        )));
    }
    let image = image & 0x00ff_ffff;
    if !image.is_multiple_of(8) {
        return Err(reject(format!(
            "{command} texture image {image:#010x} is not 8-byte aligned"
        )));
    }
    let bytes = match texture {
        ObjectTexture::Block {
            tmem, tsize, tline, ..
        } => {
            let words = u32::from(tsize) + 1;
            if tmem > 511 || u32::from(tmem) + words > 512 || !(1..=0x0fff).contains(&tline) {
                return Err(reject(format!(
                    "{command} uObjTxtrBlock has invalid tmem={tmem} tsize={tsize} tline={tline}"
                )));
            }
            words * 8
        }
        ObjectTexture::Tile {
            tmem,
            twidth,
            theight,
            ..
        } => {
            if tmem > 511
                || twidth & 3 != 3
                || theight & 3 != 3
                || twidth > 0x03ff
                || theight > 0x0fff
            {
                return Err(reject(format!(
                    "{command} uObjTxtrTile has invalid tmem={tmem} twidth={twidth} theight={theight}"
                )));
            }
            let words_per_row = (u32::from(twidth) + 1) / 4;
            let rows = (u32::from(theight) + 1) / 4;
            let words = words_per_row * rows;
            if u32::from(tmem) + words > 512 {
                return Err(reject(format!(
                    "{command} uObjTxtrTile range tmem={tmem} words={words} exceeds TMEM"
                )));
            }
            words * 8
        }
        ObjectTexture::Tlut { phead, pnum, .. } => {
            let entries = u32::from(pnum) + 1;
            if !(256..=511).contains(&phead) || pnum > 255 || u32::from(phead) + entries > 512 {
                return Err(reject(format!(
                    "{command} uObjTxtrTLUT has invalid phead={phead} pnum={pnum}"
                )));
            }
            entries * 2
        }
    };
    let end = image
        .checked_add(bytes)
        .ok_or_else(|| reject(format!("{command} texture image range overflow")))?;
    if end as usize > PHYSICAL_RDRAM_BYTES {
        return Err(reject(format!(
            "{command} texture image range [{image:#010x}, {end:#010x}) exceeds physical 8 MiB RDRAM"
        )));
    }
    if end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} texture image range [{image:#010x}, {end:#010x}) exceeds RDRAM length {}",
            rdram.len()
        )));
    }
    // RdramView's halfword access uses the generated-C native-word layout:
    // a logical TLUT entry at offset zero occupies storage bytes 2..4.
    // Preserve the complete containing word when the logical image has a
    // two-byte tail; the other admitted load shapes already end on words.
    let storage_end = (end + 3) & !3;
    if storage_end as usize > rdram.len() {
        return Err(reject(format!(
            "{command} texture image native-word storage ends at {storage_end:#010x}, beyond RDRAM length {}",
            rdram.len()
        )));
    }
    Ok((
        image,
        usize::try_from(bytes).expect("physical S2DEX image size fits usize"),
    ))
}

fn read_u32(rdram: &[u8], address: usize) -> u32 {
    fn64_runtime::RdramView::from_storage(rdram).read_u32(fn64_runtime::RdramAddr::from_offset(
        u32::try_from(address).expect("S2DEX RDRAM address exceeds u32"),
    ))
}

fn reject(reason: impl Into<String>) -> RenderError {
    RenderError::Backend {
        backend: "reference-s2dex",
        reason: reason.into(),
    }
}

fn unsupported(operation: &'static str, reason: impl Into<String>) -> RenderError {
    crate::render_unsupported_error("reference-s2dex", operation, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gbi::{ConvertState, OtherMode, TextureRectangle};

    fn write_command(rdram: &mut [u8], offset: usize, w0: u32, w1: u32) {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }

    fn write_block_texture(rdram: &mut [u8], address: u32, image: u32, flag: u32) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u32(base, G_OBJLT_TXTRBLOCK);
        view.write_u32(base.checked_add(4).unwrap(), image);
        view.write_u16(base.checked_add(8).unwrap(), 0);
        view.write_u16(base.checked_add(10).unwrap(), 1); // two 64-bit words
        view.write_u16(base.checked_add(12).unwrap(), 1 << 11); // one word/row
        view.write_u16(base.checked_add(14).unwrap(), 0);
        view.write_u32(base.checked_add(16).unwrap(), flag);
        view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
    }

    fn write_tile_texture(rdram: &mut [u8], address: u32, image: u32, flag: u32) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u32(base, G_OBJLT_TXTRTILE);
        view.write_u32(base.checked_add(4).unwrap(), image);
        view.write_u16(base.checked_add(8).unwrap(), 0);
        view.write_u16(base.checked_add(10).unwrap(), 3); // one word/row
        view.write_u16(base.checked_add(12).unwrap(), 7); // two rows
        view.write_u16(base.checked_add(14).unwrap(), 0);
        view.write_u32(base.checked_add(16).unwrap(), flag);
        view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
    }

    fn write_tlut_texture(rdram: &mut [u8], address: u32, image: u32) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u32(base, G_OBJLT_TLUT);
        view.write_u32(base.checked_add(4).unwrap(), image);
        view.write_u16(base.checked_add(8).unwrap(), 256);
        view.write_u16(base.checked_add(10).unwrap(), 15);
        view.write_u16(base.checked_add(12).unwrap(), 0);
        view.write_u16(base.checked_add(14).unwrap(), 4);
        view.write_u32(base.checked_add(16).unwrap(), image);
        view.write_u32(base.checked_add(20).unwrap(), u32::MAX);
    }

    fn write_object_matrix(
        rdram: &mut [u8],
        address: u32,
        x: i16,
        y: i16,
        base_scale_x: u16,
        base_scale_y: u16,
    ) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u32(base, 1 << 16);
        view.write_u32(base.checked_add(4).unwrap(), 0);
        view.write_u32(base.checked_add(8).unwrap(), 0);
        view.write_u32(base.checked_add(12).unwrap(), 1 << 16);
        view.write_u16(base.checked_add(16).unwrap(), x as u16);
        view.write_u16(base.checked_add(18).unwrap(), y as u16);
        view.write_u16(base.checked_add(20).unwrap(), base_scale_x);
        view.write_u16(base.checked_add(22).unwrap(), base_scale_y);
    }

    fn write_object_rotation_matrix(
        rdram: &mut [u8],
        address: u32,
        rotation: [i32; 4],
        x: i16,
        y: i16,
    ) {
        write_object_matrix(rdram, address, x, y, 1 << 10, 1 << 10);
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        for (index, value) in rotation.into_iter().enumerate() {
            view.write_u32(base.checked_add((index * 4) as u32).unwrap(), value as u32);
        }
    }

    fn write_object_sub_matrix(
        rdram: &mut [u8],
        address: u32,
        x: i16,
        y: i16,
        base_scale_x: u16,
        base_scale_y: u16,
    ) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u16(base, x as u16);
        view.write_u16(base.checked_add(2).unwrap(), y as u16);
        view.write_u16(base.checked_add(4).unwrap(), base_scale_x);
        view.write_u16(base.checked_add(6).unwrap(), base_scale_y);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_background_common(
        rdram: &mut [u8],
        address: u32,
        image: u32,
        image_width: u16,
        image_height: u16,
        frame_width: u16,
        frame_height: u16,
        image_load: u16,
        image_size: u8,
    ) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u16(base, 0);
        view.write_u16(base.checked_add(2).unwrap(), image_width * 4);
        view.write_u16(base.checked_add(4).unwrap(), 0);
        view.write_u16(base.checked_add(6).unwrap(), frame_width * 4);
        view.write_u16(base.checked_add(8).unwrap(), 0);
        view.write_u16(base.checked_add(10).unwrap(), image_height * 4);
        view.write_u16(base.checked_add(12).unwrap(), 0);
        view.write_u16(base.checked_add(14).unwrap(), frame_height * 4);
        view.write_u32(base.checked_add(16).unwrap(), image);
        view.write_u16(base.checked_add(20).unwrap(), image_load);
        view.write_u8(base.checked_add(22).unwrap(), 0);
        view.write_u8(base.checked_add(23).unwrap(), image_size);
        view.write_u16(base.checked_add(24).unwrap(), 0);
        view.write_u16(base.checked_add(26).unwrap(), 0);
    }

    fn write_copy_background_init(
        rdram: &mut [u8],
        address: u32,
        image_width: u16,
        frame_width: u16,
        image_load: u16,
        image_size: u8,
    ) {
        let shift = 4 - u32::from(image_size);
        let image_words = u32::from(image_width) >> shift;
        let frame_words = u32::from(frame_width) >> shift;
        let tmem_w = if image_load == G_BGLT_LOADBLOCK {
            image_words
        } else {
            frame_words + 1
        };
        let tmem_h = (512 / tmem_w) * 4;
        let tmem_size_w = if image_load == G_BGLT_LOADBLOCK {
            tmem_w * 2
        } else {
            image_words * 2
        };
        let tmem_size = tmem_size_w * tmem_h;
        let tmem_load_sh = if image_load == G_BGLT_LOADBLOCK {
            tmem_size / 2 - 1
        } else {
            tmem_w * 16 - 1
        };
        let tmem_load_th = if image_load == G_BGLT_LOADBLOCK {
            2047 / tmem_w + 1
        } else {
            tmem_h - 1
        };
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        for (offset, value) in [
            (28, tmem_w),
            (30, tmem_h),
            (32, tmem_load_sh),
            (34, tmem_load_th),
            (36, tmem_size_w),
            (38, tmem_size),
        ] {
            view.write_u16(base.checked_add(offset).unwrap(), value as u16);
        }
    }

    fn write_background_window(
        rdram: &mut [u8],
        address: u32,
        image_x: u16,
        image_y: u16,
        flipped: bool,
    ) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u16(base, image_x << 5);
        view.write_u16(base.checked_add(8).unwrap(), image_y << 5);
        view.write_u16(
            base.checked_add(26).unwrap(),
            if flipped { G_BG_FLAG_FLIPS } else { 0 },
        );
    }

    fn write_scale_background_tail(
        rdram: &mut [u8],
        address: u32,
        scale_w: u16,
        scale_h: u16,
        image_y_origin: i32,
    ) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        view.write_u16(base.checked_add(28).unwrap(), scale_w);
        view.write_u16(base.checked_add(30).unwrap(), scale_h);
        view.write_u32(base.checked_add(32).unwrap(), image_y_origin as u32);
        view.write_u32(base.checked_add(36).unwrap(), 0);
    }

    fn write_sprite(rdram: &mut [u8], address: u32, width: u16, height: u16, format: u8, size: u8) {
        let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
        let base = fn64_runtime::RdramAddr::from_offset(address);
        for (offset, value) in [
            (0, 0),
            (2, 1 << 10),
            (4, width << 5),
            (6, 0),
            (8, 0),
            (10, 1 << 10),
            (12, height << 5),
            (14, 0),
            (16, 1),
            (18, 0),
        ] {
            view.write_u16(base.checked_add(offset).unwrap(), value);
        }
        view.write_u8(base.checked_add(20).unwrap(), format);
        view.write_u8(base.checked_add(21).unwrap(), size);
        view.write_u8(base.checked_add(22).unwrap(), 0);
        view.write_u8(base.checked_add(23).unwrap(), 0);
    }

    fn rectangle_texture(operation: &RenderOp) -> (&crate::gbi::Texture, OtherMode) {
        let RenderOp::TextureRectangle(rectangle) = operation else {
            panic!("expected texture rectangle, got {operation:?}");
        };
        (
            rectangle
                .texture
                .as_ref()
                .expect("object rectangle must bind loaded TMEM"),
            rectangle.other_mode,
        )
    }

    #[test]
    fn loadtx_rect_block_loads_before_rectangle_binding() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x200;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        let pixels = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
        ];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in pixels.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    pixel,
                );
            }
        }
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let sprite = read_object_sprite(&rdram, TXSP + 24, "test").unwrap();
        let RenderOp::TextureRectangle(before_load) = rdp.clone().object_rectangle(sprite).unwrap()
        else {
            panic!("object rectangle must lower to a texture rectangle");
        };
        assert!(
            before_load.texture.is_none(),
            "fresh RDP state must not already contain the compound texture"
        );
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        assert_eq!(operations.len(), 1);
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [255, 0, 0, 255]
        );
        assert_eq!(
            texture.sample_rdp(0.0, 1.0, other_mode, ConvertState::default()),
            [0, 255, 255, 255],
            "LoadBlock DXT row exchange must survive the object lowering"
        );
    }

    #[test]
    fn alias_backed_rdram_uses_only_the_required_physical_prefix() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const SPRITE: u32 = 0x240;
        const IMAGE: u32 = 0x400;
        let mut physical = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut physical).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0xf801,
            );
        }
        write_block_texture(&mut physical, TX, IMAGE, IMAGE);
        write_sprite(&mut physical, SPRITE, 4, 2, 0, 2);
        write_command(&mut physical, DL, 0x0500_0017, TX);
        write_command(&mut physical, DL + 8, 0x0100_0000, SPRITE);
        write_command(&mut physical, DL + 16, 0xdf00_0000, 0);

        let mut alias_backing = fn64_runtime::Rdram::new_with_mmio(PHYSICAL_RDRAM_BYTES);
        alias_backing.write_bytes(0, &physical);
        assert!(
            alias_backing.len() > 0x0100_0000,
            "the regression requires a generated-C alias backing larger than the raw decoder's 24-bit command space"
        );
        let rdram = alias_backing.read_bytes(0, alias_backing.len());
        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(rdram, DL as u32, &mut rdp).unwrap();
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn near_end_object_status_misses_reuse_bounded_staging() {
        const DL: usize = 0x100;
        const RED_TX: u32 = 0x200;
        const BLUE_TX: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const RED: u32 = (PHYSICAL_RDRAM_BYTES - 32) as u32;
        const BLUE: u32 = (PHYSICAL_RDRAM_BYTES - 16) as u32;
        let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES];
        assert_eq!(
            ObjectTextureScratch::new().bytes.len(),
            OBJECT_TEXTURE_SCRATCH_BYTES
        );
        assert!(OBJECT_TEXTURE_SCRATCH_BYTES < RED as usize);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                    0xf801,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                    0x003f,
                );
            }
        }
        write_block_texture(&mut rdram, RED_TX, RED, 1);
        write_block_texture(&mut rdram, BLUE_TX, BLUE, 2);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, RED_TX);
        write_command(&mut rdram, DL + 8, 0x0500_0017, BLUE_TX);
        write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

        let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        let (texture, _) = rectangle_texture(&operations[0]);
        assert_eq!(texture.sample(0.0, 0.0), [0, 0, 255, 255]);
    }

    #[test]
    fn texture_image_cannot_escape_physical_rdram_into_alias_backing() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES + 0x100];
        write_block_texture(&mut rdram, TX, PHYSICAL_RDRAM_BYTES as u32, 0x1234_5678);
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

        let error = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap_err();
        assert!(error.to_string().contains("exceeds physical 8 MiB RDRAM"));
    }

    #[test]
    fn standalone_loadtx_tile_then_rectangle_uses_loaded_tmem() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const SPRITE: u32 = 0x240;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        let pixels = [
            0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
        ];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in pixels.into_iter().enumerate() {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    pixel,
                );
            }
        }
        write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(3.0, 1.0, other_mode, ConvertState::default()),
            [0, 0, 0, 255]
        );
    }

    #[test]
    fn object_matrix_drives_standalone_matrix_relative_rectangle() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const SPRITE: u32 = 0x258;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0xf801,
            );
        }
        write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_object_matrix(&mut rdram, MATRIX, 8, 12, 2 << 10, 1 << 10);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, DL + 16, 0xda00_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

        let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("matrix-relative object must lower to a texture rectangle");
        };
        assert_eq!((rectangle.ulx, rectangle.uly), (2.0, 3.0));
        assert_eq!((rectangle.lrx, rectangle.lry), (4.0, 5.0));
        assert_eq!((rectangle.dsdx, rectangle.dtdy), (2 << 10, 1 << 10));
        assert!(rectangle.texture.is_some());
    }

    #[test]
    fn sub_matrix_then_compound_loadtx_rect_r_loads_before_drawing() {
        const DL: usize = 0x100;
        const SUB_MATRIX: u32 = 0x200;
        const TXSP: u32 = 0x240;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0x003f,
            );
        }
        write_object_sub_matrix(&mut rdram, SUB_MATRIX, 16, 20, 1 << 10, 1 << 10);
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0xdc02_0007, SUB_MATRIX);
        write_command(&mut rdram, DL + 8, 0x0800_002f, TXSP);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("matrix-relative compound must lower to a texture rectangle");
        };
        assert_eq!((rectangle.ulx, rectangle.uly), (4.0, 5.0));
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn full_matrix_rotates_standalone_sprite_into_two_textured_triangles() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const SPRITE: u32 = 0x258;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0xf801,
            );
        }
        write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_object_rotation_matrix(&mut rdram, MATRIX, [0, 1 << 16, -(1 << 16), 0], 32, 32);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, DL + 16, 0x0200_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

        let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(operations.len(), 2);
        let triangles = operations
            .iter()
            .map(|operation| match operation {
                RenderOp::Triangle(triangle) => triangle,
                _ => panic!("rotating sprite must lower only to triangles"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            triangles[0].v.map(|vertex| (vertex.x, vertex.y)),
            [(8.0, 8.0), (8.0, 4.0), (10.0, 4.0)]
        );
        assert_eq!(
            triangles[1].v.map(|vertex| (vertex.x, vertex.y)),
            [(8.0, 8.0), (10.0, 4.0), (10.0, 8.0)]
        );
        assert_eq!(
            triangles[0].v.map(|vertex| (vertex.s, vertex.t)),
            [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0)]
        );
        assert!(triangles.iter().all(|triangle| triangle.texture.is_some()));

        let mut framebuffer = crate::raster::Framebuffer::new(12, 12);
        framebuffer.clear(0, 0, 0, 0);
        for triangle in triangles {
            framebuffer.draw_triangle(triangle);
        }
        let pixel = |x: usize, y: usize| {
            let offset = (y * framebuffer.width as usize + x) * 4;
            &framebuffer.pixels[offset..offset + 4]
        };
        assert_eq!(pixel(8, 5), [255, 0, 0, 255]);
        assert_eq!(pixel(7, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn texel1_gap_rotating_sprite_preserves_tile_pair_across_both_wire_families() {
        const DL: usize = 0x100;
        const MATRIX: u32 = 0x200;
        const TXSP: u32 = 0x240;
        const SPRITE: u32 = TXSP + 24;
        const IMAGE: u32 = 0x400;
        const MODE: usize = 0x500;

        for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
            for compound in [false, true] {
                let mut rdram = vec![0u8; 0x600];
                {
                    let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                    for index in 0..8 {
                        view.write_u16(
                            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                            if index < 4 { 0xf801 } else { 0x003f },
                        );
                    }
                }
                write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 4, 4);
                write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
                write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);

                // Two-cycle mode; cycle one passes TEXEL0 and cycle two
                // passes TEXEL1. Tile 1 starts at the second TMEM word loaded
                // by the object's eight-texel block.
                write_command(&mut rdram, MODE, 0xef00_0000 | 0x0018_0cff, 0);
                write_command(&mut rdram, MODE + 8, 0xfc00_0000, 0x0000_8282);
                write_command(
                    &mut rdram,
                    MODE + 16,
                    0xf500_0000 | (2 << 19) | (1 << 9) | 1,
                    1 << 24,
                );
                write_command(&mut rdram, MODE + 24, 0xf200_0000, 1 << 24);
                write_command(&mut rdram, MODE + 32, 0xdf00_0000, 0);

                let mut rdp = RdpDecodeState::default();
                crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();

                let (matrix_opcode, load_opcode, sprite_opcode, compound_opcode, end_opcode) =
                    match family {
                        S2dexWireFamily::S2dex => (
                            S2DEX_G_OBJ_MOVEMEM,
                            S2DEX_G_OBJ_LOADTXTR,
                            S2DEX_G_OBJ_SPRITE,
                            S2DEX_G_OBJ_LDTX_SPRITE,
                            S2DEX_G_ENDDL,
                        ),
                        S2dexWireFamily::S2dex2 => (
                            G_OBJ_MOVEMEM,
                            G_OBJ_LOADTXTR,
                            G_OBJ_SPRITE,
                            G_OBJ_LDTX_SPRITE,
                            G_ENDDL,
                        ),
                    };
                write_command(
                    &mut rdram,
                    DL,
                    (u32::from(matrix_opcode) << 24) | 23,
                    MATRIX,
                );
                let mut offset = DL + 8;
                if compound {
                    write_command(
                        &mut rdram,
                        offset,
                        (u32::from(compound_opcode) << 24) | 47,
                        TXSP,
                    );
                    offset += 8;
                } else {
                    write_command(
                        &mut rdram,
                        offset,
                        (u32::from(load_opcode) << 24) | 23,
                        TXSP,
                    );
                    write_command(
                        &mut rdram,
                        offset + 8,
                        u32::from(sprite_opcode) << 24,
                        SPRITE,
                    );
                    offset += 16;
                }
                write_command(&mut rdram, offset, u32::from(end_opcode) << 24, 0);

                let operations =
                    decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap();
                assert_eq!(operations.len(), 2);
                let mut framebuffer = crate::raster::Framebuffer::new(4, 4);
                framebuffer.clear(0, 0, 0, 0);
                for operation in &operations {
                    let RenderOp::Triangle(triangle) = operation else {
                        panic!("rotating sprite must lower only to triangles")
                    };
                    framebuffer.draw_triangle(triangle);
                }
                let pixel_offset = (framebuffer.width as usize + 1) * 4;
                assert_eq!(
                    &framebuffer.pixels[pixel_offset..pixel_offset + 4],
                    &[0, 0, 255, 255],
                    "family={family:?} compound={compound} must source TEXEL1 from tile 1"
                );
            }
        }
    }

    #[test]
    fn texel1_gap_rotating_sprite_without_tile_one_stays_loud_and_transactional() {
        const DL: usize = 0x100;
        const MATRIX: u32 = 0x200;
        const TX: u32 = 0x240;
        const SPRITE: u32 = 0x258;
        const IMAGE: u32 = 0x400;
        const MODE: usize = 0x500;
        let mut rdram = vec![0u8; 0x600];
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 4, 4);
        write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, MODE, 0xef00_0000 | 0x0018_0cff, 0);
        write_command(&mut rdram, MODE + 8, 0xfc00_0000, 0x0000_8282);
        write_command(
            &mut rdram,
            MODE + 16,
            0xf500_0000 | (2 << 19) | (1 << 9) | 100,
            1 << 24,
        );
        write_command(&mut rdram, MODE + 24, 0xf200_0000 | (4 << 12), 1 << 24);
        write_command(&mut rdram, MODE + 32, 0xdf00_0000, 0);
        write_command(
            &mut rdram,
            DL,
            (u32::from(G_OBJ_MOVEMEM) << 24) | 23,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_LOADTXTR) << 24) | 23,
            TX,
        );
        write_command(&mut rdram, DL + 16, u32::from(G_OBJ_SPRITE) << 24, SPRITE);
        write_command(&mut rdram, DL + 24, u32::from(G_ENDDL) << 24, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
        let before = format!("{rdp:?}");
        let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("combiner selects TEXEL1 without an initialized tile 1 image"),
            "{error}"
        );
        assert_eq!(format!("{rdp:?}"), before);
    }

    #[test]
    fn compound_loadtx_sprite_loads_then_draws_and_rejected_tail_is_atomic() {
        const DL: usize = 0x100;
        const MATRIX: u32 = 0x200;
        const TXSP: u32 = 0x240;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0x003f,
            );
        }
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 20);
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, DL + 8, 0x0600_002f, TXSP);
        write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        assert_eq!(operations.len(), 2);
        let RenderOp::Triangle(triangle) = &operations[0] else {
            panic!("compound rotating sprite must lower to triangles");
        };
        assert_eq!((triangle.v[0].x, triangle.v[0].y), (4.0, 5.0));
        assert!(triangle.texture.is_some());

        write_command(&mut rdram, DL + 16, 0x0900_0000, 0);
        let mut fresh = RdpDecodeState::default();
        let before = format!("{fresh:?}");
        let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
        assert!(error.to_string().contains("G_BG_1CYC"));
        assert_eq!(format!("{fresh:?}"), before);
    }

    #[test]
    fn rotating_sprite_traps_unknown_matrix_rounding_without_committing() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const SPRITE: u32 = 0x258;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 15, 0, 0, 1 << 16], 0, 0);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(SPRITE), 1);
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, DL + 16, 0x0200_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let before = format!("{rdp:?}");
        let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
        assert!(error
            .to_string()
            .contains("sub-quarter-pixel matrix rounding"));
        assert_eq!(format!("{rdp:?}"), before);
    }

    #[test]
    fn object_render_mode_retains_public_modes_as_typed_state() {
        let mode = read_object_render_mode(
            G_OBJRM_NOTXCLAMP | G_OBJRM_XLU | G_OBJRM_ANTIALIAS | G_OBJRM_BILERP | G_OBJRM_WIDEN,
            0x100,
        )
        .unwrap();
        assert_eq!(mode.texture_clamp, ObjectTextureClamp::Disabled);
        assert_eq!(mode.filter_correction, ObjectFilterCorrection::Bilinear);
        assert_eq!(mode.perimeter.shrink_half_texels, 0);
        assert!(mode.perimeter.widen_three_eighths_texel);
        assert_eq!(
            mode.ignored_edge_flags,
            IgnoredObjectEdgeFlags {
                xlu: true,
                antialias: true,
            }
        );

        let combined =
            read_object_render_mode(G_OBJRM_WIDEN | G_OBJRM_SHRINKSIZE_1, 0x108).unwrap();
        assert_eq!(combined.perimeter.shrink_half_texels, 1);
        assert!(combined.perimeter.widen_three_eighths_texel);

        let error = read_object_render_mode(G_OBJRM_SHRINKSIZE_1 | G_OBJRM_SHRINKSIZE_2, 0x110)
            .unwrap_err();
        assert!(error.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn object_perimeter_composition_exhaustively_preserves_public_fixed_units() {
        for shrink_half_texels in 0..=2u8 {
            for widen_three_eighths_texel in [false, true] {
                let perimeter = ObjectPerimeter {
                    shrink_half_texels,
                    widen_three_eighths_texel,
                };
                let shrink_numerator = u32::from(shrink_half_texels) * 4096;
                let widen_numerator: u32 = if widen_three_eighths_texel { 1536 } else { 0 };
                for scale_10 in 1..=i16::MAX as u16 {
                    let result =
                        perimeter.exact_screen_adjustments(scale_10, "X", "G_OBJ_RECTANGLE");
                    let exact = shrink_numerator.is_multiple_of(u32::from(scale_10))
                        && widen_numerator.is_multiple_of(u32::from(scale_10));
                    assert_eq!(
                        result.is_ok(),
                        exact,
                        "shrink={shrink_half_texels} widen={widen_three_eighths_texel} scale={scale_10}"
                    );
                    if let Ok((shrink_pixels, widen_pixels)) = result {
                        assert_eq!(
                            shrink_pixels,
                            shrink_numerator as f32 / scale_10 as f32 / 4.0
                        );
                        assert_eq!(widen_pixels, widen_numerator as f32 / scale_10 as f32 / 4.0);
                    }
                }

                for image_texels in 1..=2047u32 {
                    let image_5 = (image_texels * 32) as u16;
                    let result = perimeter.corrected_image_5(image_5, "width", "G_OBJ_RECTANGLE");
                    let shrink_5 = u16::from(shrink_half_texels) * 32;
                    let expected = image_5.checked_sub(shrink_5).and_then(|value| {
                        value.checked_add(if widen_three_eighths_texel { 12 } else { 0 })
                    });
                    assert_eq!(result.ok(), expected);
                    let (source_start, source_end) = perimeter.source_bounds(image_5);
                    assert_eq!(source_start, f32::from(shrink_half_texels) * 0.5);
                    assert_eq!(
                        source_end,
                        image_texels as f32 - f32::from(shrink_half_texels) * 0.5
                            + if widen_three_eighths_texel {
                                0.375
                            } else {
                                0.0
                            }
                    );
                }
            }
        }
        let error = ObjectPerimeter::default()
            .exact_screen_adjustments(0, "X", "G_OBJ_RECTANGLE")
            .unwrap_err();
        assert!(error.to_string().contains("scale must be nonzero"));
    }

    #[test]
    fn object_render_mode_opcode_collision_is_selected_only_by_admitted_family() {
        const DL: usize = 0x100;
        let mut legacy = vec![0u8; 0x120];
        write_command(
            &mut legacy,
            DL,
            (u32::from(S2DEX_G_OBJ_RENDERMODE)) << 24,
            G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
        );
        write_command(&mut legacy, DL + 8, (u32::from(S2DEX_G_ENDDL)) << 24, 0);
        let mut modern = vec![0u8; 0x120];
        write_command(
            &mut modern,
            DL,
            (u32::from(G_OBJ_RENDERMODE)) << 24,
            G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
        );
        write_command(&mut modern, DL + 8, (u32::from(G_ENDDL)) << 24, 0);

        assert!(decode_ops_for_family(
            &legacy,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex,
        )
        .unwrap()
        .is_empty());
        assert!(decode_ops_for_family(
            &modern,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex2,
        )
        .unwrap()
        .is_empty());
        let legacy_as_modern = decode_ops_for_family(
            &legacy,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex2,
        )
        .unwrap_err();
        assert!(
            legacy_as_modern
                .to_string()
                .contains("unsupported S2dex2 command"),
            "{legacy_as_modern}"
        );
        let modern_as_legacy = decode_ops_for_family(
            &modern,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex,
        )
        .unwrap_err();
        assert!(
            modern_as_legacy
                .to_string()
                .contains("unsupported S2dex command"),
            "{modern_as_legacy}"
        );
    }

    #[test]
    fn current_header_ignored_edge_flags_and_safe_notxclamp_preserve_point_raster() {
        const BASE_DL: usize = 0x100;
        const EDGE_DL: usize = 0x120;
        const NO_CLAMP_DL: usize = 0x140;
        const TXSP: u32 = 0x200;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x500];
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, color) in [0xf801, 0x003f, 0x07c1, 0xffff]
                .into_iter()
                .cycle()
                .take(8)
                .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_command(&mut rdram, BASE_DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, BASE_DL + 8, 0xdf00_0000, 0);
        write_command(
            &mut rdram,
            EDGE_DL,
            0x0b00_0000,
            G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
        );
        write_command(&mut rdram, EDGE_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, EDGE_DL + 16, 0xdf00_0000, 0);
        write_command(&mut rdram, NO_CLAMP_DL, 0x0b00_0000, G_OBJRM_NOTXCLAMP);
        write_command(&mut rdram, NO_CLAMP_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, NO_CLAMP_DL + 16, 0xdf00_0000, 0);

        let draw = |operations: &[RenderOp]| {
            let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                panic!("object mode fixture must emit one rectangle")
            };
            let mut framebuffer = crate::raster::Framebuffer::new(4, 2);
            framebuffer.clear(0, 0, 0, 0);
            framebuffer.draw_texture_rectangle(rectangle);
            framebuffer.pixels
        };
        let base = decode_ops(&rdram, BASE_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let edge = decode_ops(&rdram, EDGE_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let no_clamp =
            decode_ops(&rdram, NO_CLAMP_DL as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(edge.len(), base.len());
        assert_eq!(no_clamp.len(), base.len());
        assert_eq!(draw(&edge), draw(&base));
        assert_eq!(draw(&no_clamp), draw(&base));
    }

    #[test]
    fn notxclamp_point_perimeters_exhaust_families_paths_flips_and_base_scales() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x700;
        let mut template = vec![0u8; 0x800];
        write_block_texture(&mut template, TXSP, IMAGE, 1);
        write_sprite(&mut template, TXSP + 24, 4, 4, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut template);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3); // four 64-bit words
            for (index, color) in [
                0xf801u16, 0xf801, 0x07c1, 0x07c1, 0xf801, 0xf801, 0x07c1, 0x07c1, 0x003f, 0x003f,
                0xffff, 0xffff, 0x003f, 0x003f, 0xffff, 0xffff,
            ]
            .into_iter()
            .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_command(&mut template, SETUP, 0xef00_0000 | 0x0008_0cff, 0);
        write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);

        let decode = |mode: u32,
                      family: S2dexWireFamily,
                      relative: bool,
                      compound: bool,
                      base_scale: u16,
                      effective_scale: u16,
                      image_flags: u8| {
            let (
                render_mode,
                load_texture,
                rectangle,
                rectangle_r,
                load_rectangle,
                load_rectangle_r,
                move_mem,
                end,
            ) = match family {
                S2dexWireFamily::S2dex => (
                    S2DEX_G_OBJ_RENDERMODE,
                    S2DEX_G_OBJ_LOADTXTR,
                    S2DEX_G_OBJ_RECTANGLE,
                    S2DEX_G_OBJ_RECTANGLE_R,
                    S2DEX_G_OBJ_LDTX_RECT,
                    S2DEX_G_OBJ_LDTX_RECT_R,
                    S2DEX_G_OBJ_MOVEMEM,
                    S2DEX_G_ENDDL,
                ),
                S2dexWireFamily::S2dex2 => (
                    G_OBJ_RENDERMODE,
                    G_OBJ_LOADTXTR,
                    G_OBJ_RECTANGLE,
                    G_OBJ_RECTANGLE_R,
                    G_OBJ_LDTX_RECT,
                    G_OBJ_LDTX_RECT_R,
                    G_OBJ_MOVEMEM,
                    G_ENDDL,
                ),
            };
            let mut rdram = template.clone();
            write_object_matrix(&mut rdram, MATRIX, 0, 0, base_scale, base_scale);
            let object_scale = if relative {
                let numerator = u32::from(effective_scale) * 1024;
                assert_eq!(numerator % u32::from(base_scale), 0);
                u16::try_from(numerator / u32::from(base_scale)).unwrap()
            } else {
                effective_scale
            };
            {
                let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
                view.write_u16(sprite.checked_add(2).unwrap(), object_scale);
                view.write_u16(sprite.checked_add(10).unwrap(), object_scale);
                view.write_u8(sprite.checked_add(23).unwrap(), image_flags);
            }
            let mut offset = DL;
            write_command(&mut rdram, offset, u32::from(render_mode) << 24, mode);
            offset += 8;
            if relative {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(move_mem) << 24) | 0x17,
                    MATRIX,
                );
                offset += 8;
            }
            if compound {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(if relative {
                        load_rectangle_r
                    } else {
                        load_rectangle
                    }) << 24)
                        | 0x2f,
                    TXSP,
                );
                offset += 8;
            } else {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(load_texture) << 24) | 0x17,
                    TXSP,
                );
                offset += 8;
                write_command(
                    &mut rdram,
                    offset,
                    u32::from(if relative { rectangle_r } else { rectangle }) << 24,
                    TXSP + 24,
                );
                offset += 8;
            }
            write_command(&mut rdram, offset, u32::from(end) << 24, 0);
            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
            decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap()
        };

        let mut saw_empty = false;
        let mut saw_nonidentity_base_scale = false;
        let footprint = ObjectUnclampedPointFootprint;
        for shrink_half_texels in [0u8, 1, 2] {
            let shrink_mode = match shrink_half_texels {
                0 => 0,
                1 => G_OBJRM_SHRINKSIZE_1,
                2 => G_OBJRM_SHRINKSIZE_2,
                _ => unreachable!(),
            };
            for widen in [false, true] {
                if !widen && shrink_half_texels == 0 {
                    continue;
                }
                // WIDEN and shrink must each land exactly in s10.2. The
                // no-shrink 768/1536 cases and the shrink+WIDEN scale-512
                // cases also exercise distinct admitted raster sequences.
                let effective_scales: &[u16] = match (widen, shrink_half_texels) {
                    (true, 0) => &[768, 1536],
                    (true, 1 | 2) => &[512],
                    (false, _) => &[512, 1024, 2048, 4096],
                    _ => unreachable!(),
                };
                let image_flags: &[u8] = if widen {
                    // Which screen edge owns positive S/T after a flip is not
                    // published, so this combination remains a loud frontier.
                    &[0]
                } else {
                    &[
                        0,
                        G_OBJ_FLAG_FLIPS,
                        G_OBJ_FLAG_FLIPT,
                        G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
                    ]
                };
                let perimeter_mode = shrink_mode | if widen { G_OBJRM_WIDEN } else { 0 };
                for &effective_scale in effective_scales {
                    for &image_flags in image_flags {
                        for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
                            for relative in [false, true] {
                                let base_scales: &[u16] = if relative {
                                    &[512, 1024, 2048]
                                } else {
                                    &[1024]
                                };
                                for &base_scale in base_scales {
                                    saw_nonidentity_base_scale |= relative && base_scale != 1024;
                                    for compound in [false, true] {
                                        let clamped = decode(
                                            perimeter_mode,
                                            family,
                                            relative,
                                            compound,
                                            base_scale,
                                            effective_scale,
                                            image_flags,
                                        );
                                        let unclamped = decode(
                                            perimeter_mode | G_OBJRM_NOTXCLAMP,
                                            family,
                                            relative,
                                            compound,
                                            base_scale,
                                            effective_scale,
                                            image_flags,
                                        );
                                        let RenderOp::TextureRectangle(clamped) = &clamped[0]
                                        else {
                                            panic!("point perimeter path must emit a texture rectangle")
                                        };
                                        let RenderOp::TextureRectangle(unclamped) = &unclamped[0]
                                        else {
                                            panic!("unclamped point perimeter path must emit a texture rectangle")
                                        };
                                        assert_eq!(
                                            (
                                                unclamped.ulx,
                                                unclamped.uly,
                                                unclamped.lrx,
                                                unclamped.lry,
                                                unclamped.s,
                                                unclamped.t,
                                                unclamped.dsdx,
                                                unclamped.dtdy,
                                            ),
                                            (
                                                clamped.ulx,
                                                clamped.uly,
                                                clamped.lrx,
                                                clamped.lry,
                                                clamped.s,
                                                clamped.t,
                                                clamped.dsdx,
                                                clamped.dtdy,
                                            )
                                        );

                                        for (start, gradient, screen_start, screen_end, axis) in [
                                            (
                                                unclamped.s,
                                                f32::from(unclamped.dsdx) / 1024.0,
                                                unclamped.ulx,
                                                unclamped.lrx,
                                                "S",
                                            ),
                                            (
                                                unclamped.t,
                                                f32::from(unclamped.dtdy) / 1024.0,
                                                unclamped.uly,
                                                unclamped.lry,
                                                "T",
                                            ),
                                        ] {
                                            let axis_footprint = footprint
                                                .validate_axis(
                                                    start,
                                                    gradient,
                                                    screen_start,
                                                    screen_end,
                                                    4,
                                                    axis,
                                                    "G_OBJ_RECTANGLE",
                                                )
                                                .unwrap();
                                            let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                                            let pixels =
                                                pixel_min(screen_start)..pixel_min(screen_end);
                                            let samples = pixels
                                                .map(|pixel| {
                                                    (start
                                                        + (pixel as f32 - screen_start.floor())
                                                            * gradient)
                                                        .floor()
                                                        as i32
                                                })
                                                .collect::<Vec<_>>();
                                            if samples.is_empty() {
                                                saw_empty = true;
                                                assert_eq!(
                                                    axis_footprint,
                                                    ObjectPointAxisFootprint::Empty
                                                );
                                            } else {
                                                assert!(samples
                                                    .iter()
                                                    .all(|texel| (0..4).contains(texel)));
                                                if gradient > 0.0 {
                                                    assert!(samples
                                                        .windows(2)
                                                        .all(|pair| pair[0] <= pair[1]));
                                                } else {
                                                    assert!(samples
                                                        .windows(2)
                                                        .all(|pair| pair[0] >= pair[1]));
                                                }
                                                assert_eq!(
                                                    axis_footprint,
                                                    ObjectPointAxisFootprint::MonotonicInterior {
                                                        direction: if gradient > 0.0 {
                                                            ObjectPointDirection::Increasing
                                                        } else {
                                                            ObjectPointDirection::Decreasing
                                                        },
                                                        first_texel: samples[0] as u16,
                                                        last_texel: samples[samples.len() - 1]
                                                            as u16,
                                                    }
                                                );
                                            }
                                        }

                                        let draw = |rectangle: &TextureRectangle| {
                                            let mut framebuffer =
                                                crate::raster::Framebuffer::new(8, 8);
                                            framebuffer.clear(0, 0, 0, 0);
                                            framebuffer.draw_texture_rectangle(rectangle);
                                            framebuffer.pixels
                                        };
                                        assert_eq!(draw(unclamped), draw(clamped));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            saw_empty,
            "exact subpixel extents must exercise the empty raster sequence"
        );
        assert!(
            saw_nonidentity_base_scale,
            "RectangleR must exercise non-identity BaseScale cross-terms"
        );
    }

    #[test]
    fn notxclamp_point_perimeters_reject_spills_and_unpublished_paths_transactionally() {
        let footprint = ObjectUnclampedPointFootprint;
        let negative = footprint
            .validate_axis(-0.5, 1.0, 0.0, 2.0, 4, "S", "G_OBJ_RECTANGLE")
            .unwrap_err();
        assert!(
            negative.to_string().contains("texel -1 outside"),
            "{negative}"
        );
        let positive = footprint
            .validate_axis(3.5, 1.0, 0.0, 2.0, 4, "T", "G_OBJ_RECTANGLE")
            .unwrap_err();
        assert!(
            positive.to_string().contains("texel 4 outside"),
            "{positive}"
        );

        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const BILERP_SETUP: usize = 0x700;
        const COPY_SETUP: usize = 0x720;
        let mut rdram = vec![0u8; 0x800];
        write_block_texture(&mut rdram, TXSP, IMAGE, 1);
        write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
        write_command(
            &mut rdram,
            BILERP_SETUP,
            0xef00_0000 | 0x0008_0cff | (2 << 12),
            0,
        );
        write_command(&mut rdram, BILERP_SETUP + 8, 0xdf00_0000, 0);
        write_command(&mut rdram, COPY_SETUP, 0xef00_0000 | (2 << 20), 0);
        write_command(&mut rdram, COPY_SETUP + 8, 0xdf00_0000, 0);

        let rectangle_error = |rdram: &mut [u8], mode, rdp: &mut RdpDecodeState| {
            write_command(rdram, DL, u32::from(G_OBJ_RENDERMODE) << 24, mode);
            write_command(
                rdram,
                DL + 8,
                (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
                TXSP,
            );
            write_command(rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
            let before = format!("{rdp:?}");
            let error = decode_ops(rdram, DL as u32, rdp).unwrap_err();
            assert_eq!(format!("{rdp:?}"), before);
            error.to_string()
        };

        let mut point_rdp = RdpDecodeState::default();
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 512);
            view.write_u16(sprite.checked_add(10).unwrap(), 512);
        }
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_NOTXCLAMP | G_OBJRM_WIDEN,
            &mut point_rdp,
        );
        assert!(error.contains("texel 4 outside"), "{error}");

        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1536);
            view.write_u16(sprite.checked_add(10).unwrap(), 1536);
        }
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
            &mut point_rdp,
        );
        assert!(error.contains("sub-quarter-pixel rounding"), "{error}");

        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
            fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
            G_OBJ_FLAG_FLIPS,
        );
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_NOTXCLAMP | G_OBJRM_WIDEN,
            &mut point_rdp,
        );
        assert!(error.contains("positive-edge selection"), "{error}");
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
            view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
            view.write_u8(sprite.checked_add(23).unwrap(), 0);
        }

        let mut bilerp_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, BILERP_SETUP as u32, &mut bilerp_rdp)
            .unwrap();
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1 | G_OBJRM_BILERP,
            &mut bilerp_rdp,
        );
        assert!(error.contains("filter-footprint arithmetic"), "{error}");

        let mut copy_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
            .unwrap();
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
            &mut copy_rdp,
        );
        assert!(error.contains("Copy cycle does not support"), "{error}");

        write_command(
            &mut rdram,
            DL,
            u32::from(G_OBJ_RENDERMODE) << 24,
            G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_LOADTXTR) << 24) | 0x17,
            TXSP,
        );
        write_command(
            &mut rdram,
            DL + 16,
            (u32::from(G_OBJ_MOVEMEM) << 24) | 0x17,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 24,
            u32::from(G_OBJ_SPRITE) << 24,
            TXSP + 24,
        );
        write_command(&mut rdram, DL + 32, u32::from(G_ENDDL) << 24, 0);
        let before = format!("{point_rdp:?}");
        let error = decode_ops(&rdram, DL as u32, &mut point_rdp).unwrap_err();
        assert!(error.to_string().contains("G_OBJRM_NOTXCLAMP on a polygon"));
        assert_eq!(format!("{point_rdp:?}"), before);
    }

    #[test]
    fn average_filter_uses_box_samples_and_loudly_rejects_unknown_corrections() {
        const AVERAGE_DL: usize = 0x100;
        const BILERP_DL: usize = 0x120;
        const NO_CLAMP_DL: usize = 0x140;
        const TXSP: u32 = 0x200;
        const IMAGE: u32 = 0x400;
        const SETUP: usize = 0x500;
        let mut rdram = vec![0u8; 0x600];
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, color) in [0xf801, 0x003f, 0x07c1, 0xffff]
                .into_iter()
                .cycle()
                .take(8)
                .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_command(&mut rdram, AVERAGE_DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, AVERAGE_DL + 8, 0xdf00_0000, 0);
        write_command(&mut rdram, BILERP_DL, 0x0b00_0000, G_OBJRM_BILERP);
        write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
        write_command(&mut rdram, NO_CLAMP_DL, 0x0b00_0000, G_OBJRM_NOTXCLAMP);
        write_command(&mut rdram, NO_CLAMP_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, NO_CLAMP_DL + 16, 0xdf00_0000, 0);
        write_command(&mut rdram, SETUP, 0xef00_0000 | 0x0008_0cff | (3 << 12), 0);
        write_command(&mut rdram, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut rdram, SETUP + 16, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, AVERAGE_DL as u32, &mut rdp).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("average fixture must emit one rectangle")
        };
        let mut framebuffer = crate::raster::Framebuffer::new(4, 2);
        framebuffer.clear(0, 0, 0, 0);
        framebuffer.draw_texture_rectangle(rectangle);
        assert_eq!(&framebuffer.pixels[..4], [128, 0, 128, 255]);

        let bilerp_error = decode_ops(&rdram, BILERP_DL as u32, &mut rdp.clone()).unwrap_err();
        assert!(bilerp_error
            .to_string()
            .contains("Average texture filter does not use G_OBJRM_BILERP"));
        let unclamped_error = decode_ops(&rdram, NO_CLAMP_DL as u32, &mut rdp.clone()).unwrap_err();
        assert!(unclamped_error
            .to_string()
            .contains("Average four-texel cell"));
    }

    #[test]
    fn average_shrink_footprint_exhaustively_classifies_public_inward_cells() {
        for filter in [
            TextureFilter::Point,
            TextureFilter::Average,
            TextureFilter::Bilinear,
            TextureFilter::Reserved,
        ] {
            for inset_half_texels in 0..=2 {
                for texture_clamp in [ObjectTextureClamp::Perimeter, ObjectTextureClamp::Disabled] {
                    for widen_three_eighths_texel in [false, true] {
                        let mode = ObjectRenderMode {
                            texture_clamp,
                            perimeter: ObjectPerimeter {
                                shrink_half_texels: inset_half_texels,
                                widen_three_eighths_texel,
                            },
                            ..ObjectRenderMode::default()
                        };
                        let result = ObjectAverageShrinkFootprint::from_mode(
                            mode,
                            filter,
                            "G_OBJ_RECTANGLE",
                        );
                        let admitted = filter == TextureFilter::Average
                            && inset_half_texels != 0
                            && !widen_three_eighths_texel;
                        if admitted {
                            let footprint = result.unwrap().expect("admitted Average inset");
                            assert_eq!(footprint.inset_half_texels, inset_half_texels);
                            for image_width in 3..=32u16 {
                                for flipped in [false, true] {
                                    let start =
                                        footprint.rectangle_start(image_width * 32, flipped);
                                    let first = start.floor() as i32;
                                    assert!(first >= 0);
                                    assert!(first + 1 < i32::from(image_width));
                                }
                            }
                        } else if filter == TextureFilter::Average
                            && inset_half_texels != 0
                            && widen_three_eighths_texel
                        {
                            assert!(result.is_err());
                        } else {
                            assert_eq!(result.unwrap(), None);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unclamped_average_endpoint_proof_matches_every_emitted_four_texel_cell() {
        let footprint = ObjectUnclampedAverageFootprint;
        for image_texels in 2..=16u16 {
            for texture_start_quarters in -4..=i32::from(image_texels) * 4 + 4 {
                for gradient_quarters in -8..=8 {
                    if gradient_quarters == 0 {
                        continue;
                    }
                    for screen_start_quarters in -3..=3 {
                        for pixel_count in 0..=12 {
                            let texture_start = texture_start_quarters as f32 / 4.0;
                            let gradient = gradient_quarters as f32 / 4.0;
                            let screen_start = screen_start_quarters as f32 / 4.0;
                            let screen_end = screen_start + pixel_count as f32;
                            let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                            let first_pixel = pixel_min(screen_start);
                            let last_pixel = pixel_min(screen_end) - 1;
                            let coordinate = |pixel: i32| {
                                texture_start + (pixel as f32 - screen_start.floor()) * gradient
                            };
                            let every_cell_is_interior = (first_pixel..=last_pixel).all(|pixel| {
                                let first = coordinate(pixel).floor() as i32;
                                first >= 0 && first + 1 < i32::from(image_texels)
                            });
                            let result = footprint.validate_axis(
                                texture_start,
                                gradient,
                                screen_start,
                                screen_end,
                                image_texels,
                                "S",
                                "G_OBJ_RECTANGLE",
                            );
                            assert_eq!(
                                result.is_ok(),
                                every_cell_is_interior,
                                "image={image_texels} start={texture_start} gradient={gradient} screen=({screen_start},{screen_end})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn average_shrink_notxclamp_matches_clamped_rectangles_across_public_paths() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x700;
        let mut template = vec![0u8; 0x800];
        write_block_texture(&mut template, TXSP, IMAGE, 1);
        write_sprite(&mut template, TXSP + 24, 8, 8, 0, 2);
        write_object_matrix(&mut template, MATRIX, 0, 0, 1 << 10, 1 << 10);
        write_command(
            &mut template,
            SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12),
            0,
        );
        write_command(&mut template, SETUP + 8, 0xdf00_0000, 0);

        let decode = |mode: u32,
                      family: S2dexWireFamily,
                      relative: bool,
                      compound: bool,
                      image_flags: u8| {
            let (
                render_mode,
                load_texture,
                rectangle,
                rectangle_r,
                load_rectangle,
                load_rectangle_r,
                move_mem,
                end,
            ) = match family {
                S2dexWireFamily::S2dex => (
                    S2DEX_G_OBJ_RENDERMODE,
                    S2DEX_G_OBJ_LOADTXTR,
                    S2DEX_G_OBJ_RECTANGLE,
                    S2DEX_G_OBJ_RECTANGLE_R,
                    S2DEX_G_OBJ_LDTX_RECT,
                    S2DEX_G_OBJ_LDTX_RECT_R,
                    S2DEX_G_OBJ_MOVEMEM,
                    S2DEX_G_ENDDL,
                ),
                S2dexWireFamily::S2dex2 => (
                    G_OBJ_RENDERMODE,
                    G_OBJ_LOADTXTR,
                    G_OBJ_RECTANGLE,
                    G_OBJ_RECTANGLE_R,
                    G_OBJ_LDTX_RECT,
                    G_OBJ_LDTX_RECT_R,
                    G_OBJ_MOVEMEM,
                    G_ENDDL,
                ),
            };
            let mut rdram = template.clone();
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
                fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
                image_flags,
            );
            let mut offset = DL;
            write_command(&mut rdram, offset, u32::from(render_mode) << 24, mode);
            offset += 8;
            if relative {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(move_mem) << 24) | 0x17,
                    MATRIX,
                );
                offset += 8;
            }
            if compound {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(if relative {
                        load_rectangle_r
                    } else {
                        load_rectangle
                    }) << 24)
                        | 0x2f,
                    TXSP,
                );
                offset += 8;
            } else {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(load_texture) << 24) | 0x17,
                    TXSP,
                );
                offset += 8;
                write_command(
                    &mut rdram,
                    offset,
                    u32::from(if relative { rectangle_r } else { rectangle }) << 24,
                    TXSP + 24,
                );
                offset += 8;
            }
            write_command(&mut rdram, offset, u32::from(end) << 24, 0);
            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
            decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap()
        };

        for shrink_mode in [G_OBJRM_SHRINKSIZE_1, G_OBJRM_SHRINKSIZE_2] {
            for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
                for relative in [false, true] {
                    for compound in [false, true] {
                        for image_flags in [
                            0,
                            G_OBJ_FLAG_FLIPS,
                            G_OBJ_FLAG_FLIPT,
                            G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
                        ] {
                            let clamped =
                                decode(shrink_mode, family, relative, compound, image_flags);
                            let unclamped = decode(
                                shrink_mode | G_OBJRM_NOTXCLAMP,
                                family,
                                relative,
                                compound,
                                image_flags,
                            );
                            let RenderOp::TextureRectangle(clamped) = &clamped[0] else {
                                panic!("Average shrink path must emit one rectangle")
                            };
                            let RenderOp::TextureRectangle(unclamped) = &unclamped[0] else {
                                panic!("unclamped Average shrink path must emit one rectangle")
                            };
                            assert_eq!(
                                (
                                    unclamped.ulx,
                                    unclamped.uly,
                                    unclamped.lrx,
                                    unclamped.lry,
                                    unclamped.s,
                                    unclamped.t,
                                    unclamped.dsdx,
                                    unclamped.dtdy,
                                ),
                                (
                                    clamped.ulx,
                                    clamped.uly,
                                    clamped.lrx,
                                    clamped.lry,
                                    clamped.s,
                                    clamped.t,
                                    clamped.dsdx,
                                    clamped.dtdy,
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn average_shrink_rectangles_exhaust_families_paths_scales_and_flips() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x700;
        let mut template = vec![0u8; 0x800];
        write_block_texture(&mut template, TXSP, IMAGE, 1);
        write_sprite(&mut template, TXSP + 24, 8, 8, 0, 2);
        write_object_matrix(&mut template, MATRIX, 0, 0, 1 << 10, 1 << 10);
        write_command(
            &mut template,
            SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12),
            0,
        );
        write_command(&mut template, SETUP + 8, 0xdf00_0000, 0);

        let mut saw_positive_edge_clamp = false;
        for shrink_half_texels in [1u8, 2] {
            let render_mode_value = if shrink_half_texels == 1 {
                G_OBJRM_SHRINKSIZE_1
            } else {
                G_OBJRM_SHRINKSIZE_2
            };
            for scale in [512u16, 1024, 2048, 4096] {
                for image_flags in [
                    0,
                    G_OBJ_FLAG_FLIPS,
                    G_OBJ_FLAG_FLIPT,
                    G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
                ] {
                    let mut expected = None;
                    for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
                        let (
                            render_mode,
                            load_texture,
                            rectangle,
                            rectangle_r,
                            load_rectangle,
                            load_rectangle_r,
                            move_mem,
                            end,
                        ) = match family {
                            S2dexWireFamily::S2dex => (
                                S2DEX_G_OBJ_RENDERMODE,
                                S2DEX_G_OBJ_LOADTXTR,
                                S2DEX_G_OBJ_RECTANGLE,
                                S2DEX_G_OBJ_RECTANGLE_R,
                                S2DEX_G_OBJ_LDTX_RECT,
                                S2DEX_G_OBJ_LDTX_RECT_R,
                                S2DEX_G_OBJ_MOVEMEM,
                                S2DEX_G_ENDDL,
                            ),
                            S2dexWireFamily::S2dex2 => (
                                G_OBJ_RENDERMODE,
                                G_OBJ_LOADTXTR,
                                G_OBJ_RECTANGLE,
                                G_OBJ_RECTANGLE_R,
                                G_OBJ_LDTX_RECT,
                                G_OBJ_LDTX_RECT_R,
                                G_OBJ_MOVEMEM,
                                G_ENDDL,
                            ),
                        };
                        for compound in [false, true] {
                            for relative in [false, true] {
                                let mut rdram = template.clone();
                                {
                                    let mut view =
                                        fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                                    let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
                                    view.write_u16(sprite.checked_add(2).unwrap(), scale);
                                    view.write_u16(sprite.checked_add(10).unwrap(), scale);
                                    view.write_u8(sprite.checked_add(23).unwrap(), image_flags);
                                }
                                let mut offset = DL;
                                write_command(
                                    &mut rdram,
                                    offset,
                                    u32::from(render_mode) << 24,
                                    render_mode_value,
                                );
                                offset += 8;
                                if relative {
                                    write_command(
                                        &mut rdram,
                                        offset,
                                        (u32::from(move_mem) << 24) | 0x17,
                                        MATRIX,
                                    );
                                    offset += 8;
                                }
                                if compound {
                                    let opcode = if relative {
                                        load_rectangle_r
                                    } else {
                                        load_rectangle
                                    };
                                    write_command(
                                        &mut rdram,
                                        offset,
                                        (u32::from(opcode) << 24) | 0x2f,
                                        TXSP,
                                    );
                                    offset += 8;
                                } else {
                                    write_command(
                                        &mut rdram,
                                        offset,
                                        (u32::from(load_texture) << 24) | 0x17,
                                        TXSP,
                                    );
                                    offset += 8;
                                    let opcode = if relative { rectangle_r } else { rectangle };
                                    write_command(
                                        &mut rdram,
                                        offset,
                                        u32::from(opcode) << 24,
                                        TXSP + 24,
                                    );
                                    offset += 8;
                                }
                                write_command(&mut rdram, offset, u32::from(end) << 24, 0);

                                let mut rdp = RdpDecodeState::default();
                                crate::gbi::decode_raw_rdp_ops_with_state(
                                    &rdram,
                                    SETUP as u32,
                                    &mut rdp,
                                )
                                .unwrap();
                                let operations =
                                    decode_ops_for_family(&rdram, DL as u32, &mut rdp, family)
                                        .unwrap();
                                let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                                    panic!("Average shrink path must emit one rectangle")
                                };
                                let identity = (
                                    rectangle.ulx,
                                    rectangle.uly,
                                    rectangle.lrx,
                                    rectangle.lry,
                                    rectangle.s,
                                    rectangle.t,
                                    rectangle.dsdx,
                                    rectangle.dtdy,
                                );
                                if let Some(expected) = expected {
                                    assert_eq!(identity, expected);
                                } else {
                                    expected = Some(identity);
                                }

                                let inset = f32::from(shrink_half_texels) * 0.5;
                                let expected_start = |flipped| {
                                    if flipped {
                                        8.0 - 1.0 - inset
                                    } else {
                                        inset
                                    }
                                };
                                assert_eq!(
                                    (rectangle.s, rectangle.t),
                                    (
                                        expected_start(image_flags & G_OBJ_FLAG_FLIPS != 0),
                                        expected_start(image_flags & G_OBJ_FLAG_FLIPT != 0),
                                    )
                                );
                                let full_extent = 8192.0 / f32::from(scale);
                                let shrink_extent =
                                    f32::from(shrink_half_texels) * 1024.0 / f32::from(scale);
                                assert_eq!(
                                    (rectangle.lrx, rectangle.lry),
                                    (full_extent - shrink_extent, full_extent - shrink_extent)
                                );

                                let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                                let footprint = ObjectAverageShrinkFootprint {
                                    inset_half_texels: shrink_half_texels,
                                };
                                for x in pixel_min(rectangle.ulx)..pixel_min(rectangle.lrx) {
                                    let s = rectangle.s
                                        + (x as f32 - rectangle.ulx.floor())
                                            * f32::from(rectangle.dsdx)
                                            / 1024.0;
                                    let s0 = s.floor() as i32;
                                    let cell = footprint
                                        .classify_cell(s, 8, "S", "G_OBJ_RECTANGLE")
                                        .unwrap();
                                    assert!((0..=8).contains(&s0));
                                    assert!((0..=8).contains(&(s0 + 1)));
                                    assert!((0..8).contains(&s0.clamp(0, 7)));
                                    assert!((0..8).contains(&(s0 + 1).clamp(0, 7)));
                                    saw_positive_edge_clamp |=
                                        cell == ObjectAverageCell::PositiveEdgeClamped;
                                }
                                for y in pixel_min(rectangle.uly)..pixel_min(rectangle.lry) {
                                    let t = rectangle.t
                                        + (y as f32 - rectangle.uly.floor())
                                            * f32::from(rectangle.dtdy)
                                            / 1024.0;
                                    let t0 = t.floor() as i32;
                                    let cell = footprint
                                        .classify_cell(t, 8, "T", "G_OBJ_RECTANGLE")
                                        .unwrap();
                                    assert!((0..=8).contains(&t0));
                                    assert!((0..=8).contains(&(t0 + 1)));
                                    assert!((0..8).contains(&t0.clamp(0, 7)));
                                    assert!((0..8).contains(&(t0 + 1).clamp(0, 7)));
                                    saw_positive_edge_clamp |=
                                        cell == ObjectAverageCell::PositiveEdgeClamped;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            saw_positive_edge_clamp,
            "the exhaustive sweep must exercise the public positive-edge clamp"
        );
    }

    #[test]
    fn average_shrink_matrix_relative_base_scale_cross_term_is_exact() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x700;
        let mut template = vec![0u8; 0x800];
        write_block_texture(&mut template, TXSP, IMAGE, 1);
        write_sprite(&mut template, TXSP + 24, 4, 4, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut template);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3); // four 64-bit words
            for (index, color) in [
                0xf801u16, 0xf801, 0x07c1, 0x07c1, 0xf801, 0xf801, 0x07c1, 0x07c1, 0x003f, 0x003f,
                0xffff, 0xffff, 0x003f, 0x003f, 0xffff, 0xffff,
            ]
            .into_iter()
            .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_command(
            &mut template,
            SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12),
            0,
        );
        write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);

        for (base_scale, effective_scale, expected_extent, drawn_pixels, expected_last) in [
            (
                512,
                512,
                6.0,
                6usize,
                ObjectAverageCell::PositiveEdgeClamped,
            ),
            (2048, 2048, 1.5, 1usize, ObjectAverageCell::Interior),
        ] {
            let mut rdram = template.clone();
            write_object_matrix(&mut rdram, MATRIX, 0, 0, base_scale, base_scale);
            {
                let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
                // matrix_relative_sprite applies object scale * BaseScale / 1024.
                view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
                view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
            }
            write_command(
                &mut rdram,
                DL,
                u32::from(G_OBJ_RENDERMODE) << 24,
                G_OBJRM_SHRINKSIZE_1,
            );
            write_command(
                &mut rdram,
                DL + 8,
                (u32::from(G_OBJ_MOVEMEM) << 24) | 0x17,
                MATRIX,
            );
            write_command(
                &mut rdram,
                DL + 16,
                (u32::from(G_OBJ_LDTX_RECT_R) << 24) | 0x2f,
                TXSP,
            );
            write_command(&mut rdram, DL + 24, u32::from(G_ENDDL) << 24, 0);

            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
            let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
            let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                panic!("Average shrink RectangleR must remain a texture rectangle")
            };
            assert_eq!(
                (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
                (0.0, 0.0, expected_extent, expected_extent)
            );
            assert_eq!(
                (rectangle.dsdx, rectangle.dtdy),
                (effective_scale, effective_scale)
            );
            assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
            let footprint = ObjectAverageShrinkFootprint {
                inset_half_texels: 1,
            };
            let expected_axis = ObjectAverageAxisFootprint::Samples {
                first: ObjectAverageCell::Interior,
                last: expected_last,
            };
            assert_eq!(
                footprint
                    .validate_axis(
                        rectangle.s,
                        f32::from(rectangle.dsdx) / 1024.0,
                        rectangle.ulx,
                        rectangle.lrx,
                        4,
                        "S",
                        "G_OBJ_LDTX_RECT_R",
                    )
                    .unwrap(),
                expected_axis
            );
            assert_eq!(
                footprint
                    .validate_axis(
                        rectangle.t,
                        f32::from(rectangle.dtdy) / 1024.0,
                        rectangle.uly,
                        rectangle.lry,
                        4,
                        "T",
                        "G_OBJ_LDTX_RECT_R",
                    )
                    .unwrap(),
                expected_axis
            );

            let mut framebuffer = crate::raster::Framebuffer::new(6, 6);
            framebuffer.clear(0, 0, 0, 0);
            framebuffer.draw_texture_rectangle(rectangle);
            assert_eq!(
                framebuffer
                    .pixels
                    .chunks_exact(4)
                    .filter(|pixel| pixel[3] != 0)
                    .count(),
                drawn_pixels * drawn_pixels
            );
            assert_eq!(&framebuffer.pixels[..4], [255, 0, 0, 255]);
            if base_scale == 512 {
                let pixel = |x: usize, y: usize| {
                    let offset = (y * 6 + x) * 4;
                    &framebuffer.pixels[offset..offset + 4]
                };
                assert_eq!(pixel(1, 0), [128, 128, 0, 255]);
                assert_eq!(pixel(5, 0), [0, 255, 0, 255]);
                assert_eq!(pixel(0, 1), [128, 0, 128, 255]);
                assert_eq!(pixel(1, 1), [128, 128, 128, 255]);
                assert_eq!(pixel(5, 5), [255, 255, 255, 255]);
            }
        }
    }

    #[test]
    fn average_shrink_one_raster_matches_exact_four_texel_cells_under_all_flips() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x700;
        let mut template = vec![0u8; 0x800];
        write_block_texture(&mut template, TXSP, IMAGE, 1);
        write_sprite(&mut template, TXSP + 24, 4, 4, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut template);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3); // four 64-bit words
            for (index, color) in [
                0xf801u16, 0xf801, 0x07c1, 0x07c1, 0xf801, 0xf801, 0x07c1, 0x07c1, 0x003f, 0x003f,
                0xffff, 0xffff, 0x003f, 0x003f, 0xffff, 0xffff,
            ]
            .into_iter()
            .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_command(
            &mut template,
            SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12),
            0,
        );
        write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);
        let expected = [
            [[255, 0, 0, 255], [128, 128, 0, 255], [0, 255, 0, 255]],
            [
                [128, 0, 128, 255],
                [128, 128, 128, 255],
                [128, 255, 128, 255],
            ],
            [[0, 0, 255, 255], [128, 128, 255, 255], [255, 255, 255, 255]],
        ];

        for image_flags in [
            0,
            G_OBJ_FLAG_FLIPS,
            G_OBJ_FLAG_FLIPT,
            G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
        ] {
            let mut rdram = template.clone();
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
                fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
                image_flags,
            );
            write_command(
                &mut rdram,
                DL,
                u32::from(G_OBJ_RENDERMODE) << 24,
                G_OBJRM_SHRINKSIZE_1,
            );
            write_command(
                &mut rdram,
                DL + 8,
                (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
                TXSP,
            );
            write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
            let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
            let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                panic!("Average shrink raster must remain a texture rectangle")
            };
            assert_eq!(
                (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
                (0.0, 0.0, 3.0, 3.0)
            );
            let mut framebuffer = crate::raster::Framebuffer::new(3, 3);
            framebuffer.clear(0, 0, 0, 0);
            framebuffer.draw_texture_rectangle(rectangle);
            for y in 0..3usize {
                for x in 0..3usize {
                    let source_x = if image_flags & G_OBJ_FLAG_FLIPS != 0 {
                        2 - x
                    } else {
                        x
                    };
                    let source_y = if image_flags & G_OBJ_FLAG_FLIPT != 0 {
                        2 - y
                    } else {
                        y
                    };
                    let offset = (y * 3 + x) * 4;
                    assert_eq!(
                        &framebuffer.pixels[offset..offset + 4],
                        expected[source_y][source_x],
                        "flags={image_flags:#04x} output=({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn average_shrink_keeps_unpublished_neighbor_classes_loud_and_transactional() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x300;
        const MATRIX: u32 = 0x340;
        const IMAGE: u32 = 0x500;
        const AVERAGE_SETUP: usize = 0x700;
        const COPY_SETUP: usize = 0x720;
        let mut rdram = vec![0u8; 0x800];
        write_block_texture(&mut rdram, TXSP, IMAGE, 1);
        write_sprite(&mut rdram, TXSP + 24, 8, 8, 0, 2);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
        write_command(
            &mut rdram,
            AVERAGE_SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12),
            0,
        );
        write_command(&mut rdram, AVERAGE_SETUP + 8, 0xdf00_0000, 0);
        write_command(
            &mut rdram,
            COPY_SETUP,
            0xef00_0000 | 0x0008_0cff | (3 << 12) | (2 << 20),
            0,
        );
        write_command(&mut rdram, COPY_SETUP + 8, 0xdf00_0000, 0);

        let rectangle_error = |rdram: &mut [u8], mode, rdp: &mut RdpDecodeState| {
            write_command(rdram, DL, u32::from(G_OBJ_RENDERMODE) << 24, mode);
            write_command(
                rdram,
                DL + 8,
                (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
                TXSP,
            );
            write_command(rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
            let before = format!("{rdp:?}");
            let error = decode_ops(rdram, DL as u32, rdp).unwrap_err();
            assert_eq!(format!("{rdp:?}"), before);
            error.to_string()
        };

        let mut average_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, AVERAGE_SETUP as u32, &mut average_rdp)
            .unwrap();
        write_command(
            &mut rdram,
            DL,
            u32::from(G_OBJ_RENDERMODE) << 24,
            G_OBJRM_SHRINKSIZE_1 | G_OBJRM_NOTXCLAMP,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
            TXSP,
        );
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        decode_ops(&rdram, DL as u32, &mut average_rdp.clone())
            .expect("inward Average cells make NOTXCLAMP unobservable");
        let error = rectangle_error(
            &mut rdram,
            G_OBJRM_SHRINKSIZE_1 | G_OBJRM_WIDEN,
            &mut average_rdp,
        );
        assert!(
            error.contains("positive-edge four-texel footprint"),
            "{error}"
        );

        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1536);
            view.write_u16(sprite.checked_add(10).unwrap(), 1536);
        }
        let error = rectangle_error(&mut rdram, G_OBJRM_SHRINKSIZE_1, &mut average_rdp);
        assert!(error.contains("sub-quarter-pixel rounding"), "{error}");
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
            view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
        }

        let mut copy_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
            .unwrap();
        let error = rectangle_error(&mut rdram, G_OBJRM_SHRINKSIZE_1, &mut copy_rdp);
        assert!(error.contains("Copy cycle does not support"), "{error}");

        write_command(
            &mut rdram,
            DL,
            u32::from(G_OBJ_RENDERMODE) << 24,
            G_OBJRM_SHRINKSIZE_1,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_LOADTXTR) << 24) | 0x17,
            TXSP,
        );
        write_command(
            &mut rdram,
            DL + 16,
            (u32::from(G_OBJ_MOVEMEM) << 24) | 0x17,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 24,
            u32::from(G_OBJ_SPRITE) << 24,
            TXSP + 24,
        );
        write_command(&mut rdram, DL + 32, u32::from(G_ENDDL) << 24, 0);
        let before = format!("{average_rdp:?}");
        let error = decode_ops(&rdram, DL as u32, &mut average_rdp).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rotating polygon requires a separately evidenced pixel-center"),
            "{error}"
        );
        assert_eq!(format!("{average_rdp:?}"), before);
    }

    #[test]
    fn widen_expands_only_exact_positive_edges_and_rasterizes() {
        const BASE_DL: usize = 0x100;
        const WIDEN_DL: usize = 0x120;
        const INEXACT_DL: usize = 0x140;
        const TXSP: u32 = 0x200;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x500];
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 6, 6, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1536);
            view.write_u16(sprite.checked_add(10).unwrap(), 1536);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(IMAGE), 0xffff);
        }
        write_command(&mut rdram, BASE_DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, BASE_DL + 8, 0xdf00_0000, 0);
        write_command(&mut rdram, WIDEN_DL, 0x0b00_0000, G_OBJRM_WIDEN);
        write_command(&mut rdram, WIDEN_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, WIDEN_DL + 16, 0xdf00_0000, 0);

        let base = decode_ops(&rdram, BASE_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let widened = decode_ops(&rdram, WIDEN_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(base) = &base[0] else {
            unreachable!()
        };
        let RenderOp::TextureRectangle(widened) = &widened[0] else {
            unreachable!()
        };
        assert_eq!((base.lrx, base.lry), (4.0, 4.0));
        assert_eq!((widened.lrx, widened.lry), (4.25, 4.25));
        let mut framebuffer = crate::raster::Framebuffer::new(1, 1);
        framebuffer.clear(0, 0, 0, 0);
        framebuffer.draw_texture_rectangle(widened);
        assert_ne!(&framebuffer.pixels[..4], [0, 0, 0, 0]);

        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1024);
            view.write_u16(sprite.checked_add(10).unwrap(), 1024);
        }
        write_command(&mut rdram, INEXACT_DL, 0x0b00_0000, G_OBJRM_WIDEN);
        write_command(&mut rdram, INEXACT_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, INEXACT_DL + 16, 0xdf00_0000, 0);
        let error =
            decode_ops(&rdram, INEXACT_DL as u32, &mut RdpDecodeState::default()).unwrap_err();
        assert!(error
            .to_string()
            .contains("unpublished sub-quarter-pixel rounding"));
    }

    #[test]
    fn object_perimeter_shrink_and_widen_compose_across_families_and_draw_paths() {
        const RECT_DL: usize = 0x100;
        const RELATIVE_DL: usize = 0x140;
        const ROTATING_DL: usize = 0x180;
        const STANDALONE_DL: usize = 0x1c0;
        const TXSP: u32 = 0x280;
        const MATRIX: u32 = 0x2b0;
        const IMAGE: u32 = 0x400;
        const COPY_SETUP: usize = 0x500;
        let mut rdram = vec![0u8; 0x600];
        write_block_texture(&mut rdram, TXSP, IMAGE, 1);
        write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 0);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3);
            view.write_u16(sprite.checked_add(2).unwrap(), 512);
            view.write_u16(sprite.checked_add(10).unwrap(), 512);
            for index in 0..16 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                    if index == 0 { 0xffff } else { 0xf801 },
                );
            }
        }
        let mode = G_OBJRM_SHRINKSIZE_1 | G_OBJRM_WIDEN;

        for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
            let (render_mode, load_rect, end) = match family {
                S2dexWireFamily::S2dex => {
                    (S2DEX_G_OBJ_RENDERMODE, S2DEX_G_OBJ_LDTX_RECT, S2DEX_G_ENDDL)
                }
                S2dexWireFamily::S2dex2 => (G_OBJ_RENDERMODE, G_OBJ_LDTX_RECT, G_ENDDL),
            };
            write_command(&mut rdram, RECT_DL, u32::from(render_mode) << 24, mode);
            write_command(
                &mut rdram,
                RECT_DL + 8,
                (u32::from(load_rect) << 24) | 0x2f,
                TXSP,
            );
            write_command(&mut rdram, RECT_DL + 16, u32::from(end) << 24, 0);
            let operations = decode_ops_for_family(
                &rdram,
                RECT_DL as u32,
                &mut RdpDecodeState::default(),
                family,
            )
            .unwrap();
            let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                panic!("combined perimeter rectangle must retain rectangle lowering")
            };
            assert_eq!(
                (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
                (0.0, 0.0, 6.75, 6.75),
                "family={family:?}"
            );
            assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
        }

        write_command(&mut rdram, RELATIVE_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, RELATIVE_DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, RELATIVE_DL + 16, 0x0800_002f, TXSP);
        write_command(&mut rdram, RELATIVE_DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, ROTATING_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, ROTATING_DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, ROTATING_DL + 16, 0x0600_002f, TXSP);
        write_command(&mut rdram, ROTATING_DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, STANDALONE_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, STANDALONE_DL + 8, 0x0500_0017, TXSP);
        write_command(&mut rdram, STANDALONE_DL + 16, 0x0100_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 24, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, STANDALONE_DL + 32, 0xda00_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 40, 0x0200_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 48, 0xdf00_0000, 0);

        let relative_ops =
            decode_ops(&rdram, RELATIVE_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let rotating_ops =
            decode_ops(&rdram, ROTATING_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let standalone =
            decode_ops(&rdram, STANDALONE_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(relative) = &relative_ops[0] else {
            unreachable!()
        };
        assert_eq!((relative.ulx, relative.lrx, relative.s), (4.0, 10.75, 0.5));
        let RenderOp::Triangle(rotating) = &rotating_ops[0] else {
            unreachable!()
        };
        assert_eq!(
            rotating
                .v
                .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
            [
                (4.0, 0.0, 0.5, 0.5),
                (10.75, 0.0, 3.875, 0.5),
                (10.75, 6.75, 3.875, 3.875),
            ]
        );
        let draw = |operations: &[RenderOp]| {
            let mut framebuffer = crate::raster::Framebuffer::new(12, 8);
            framebuffer.clear(0, 0, 0, 0);
            for operation in operations {
                match operation {
                    RenderOp::TextureRectangle(rectangle) => {
                        framebuffer.draw_texture_rectangle(rectangle)
                    }
                    RenderOp::Triangle(triangle) => framebuffer.draw_triangle(triangle),
                    _ => panic!("perimeter fixture emitted an unexpected operation"),
                }
            }
            framebuffer
        };
        let relative_fb = draw(&relative_ops);
        let rotating_fb = draw(&rotating_ops);
        for y in 0..7usize {
            for x in 4..11usize {
                let offset = (y * 12 + x) * 4;
                assert_ne!(&relative_fb.pixels[offset..offset + 4], [0, 0, 0, 0]);
                assert_ne!(&rotating_fb.pixels[offset..offset + 4], [0, 0, 0, 0]);
            }
        }
        let RenderOp::TextureRectangle(standalone_rectangle) = &standalone[0] else {
            unreachable!()
        };
        let RenderOp::TextureRectangle(standalone_relative) = &standalone[1] else {
            unreachable!()
        };
        let RenderOp::Triangle(standalone_rotating) = &standalone[2] else {
            unreachable!()
        };
        assert_eq!(
            (standalone_rectangle.lrx, standalone_rectangle.s),
            (6.75, 0.5)
        );
        assert_eq!(
            (standalone_relative.ulx, standalone_relative.lrx),
            (relative.ulx, relative.lrx)
        );
        assert_eq!(standalone_rotating.v, rotating.v);

        write_command(
            &mut rdram,
            RECT_DL,
            0x0b00_0000,
            G_OBJRM_SHRINKSIZE_2 | G_OBJRM_WIDEN,
        );
        write_command(&mut rdram, RECT_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, RECT_DL + 16, 0xdf00_0000, 0);
        let shrink_two =
            decode_ops(&rdram, RECT_DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(shrink_two) = &shrink_two[0] else {
            unreachable!()
        };
        assert_eq!((shrink_two.lrx, shrink_two.lry), (4.75, 4.75));
        assert_eq!((shrink_two.s, shrink_two.t), (1.0, 1.0));

        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
            fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
            G_OBJ_FLAG_FLIPS,
        );
        let error = decode_ops(&rdram, RECT_DL as u32, &mut RdpDecodeState::default()).unwrap_err();
        assert!(error.to_string().contains("positive-edge selection"));
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u8(fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23), 0);

        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
            view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
        }
        write_command(&mut rdram, COPY_SETUP, 0xef00_0000 | (2 << 20), 0);
        write_command(&mut rdram, COPY_SETUP + 8, 0xdf00_0000, 0);
        let mut copy_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
            .unwrap();
        let error = decode_ops(&rdram, RECT_DL as u32, &mut copy_rdp).unwrap_err();
        assert!(error.to_string().contains("Copy cycle does not support"));
    }

    #[test]
    fn object_bilerp_mode_matches_filter_and_preserves_corrected_texel_centers() {
        const POINT_DL: usize = 0x100;
        const BILERP_DL: usize = 0x140;
        const BILERP_SPRITE_DL: usize = 0x180;
        const TXSP: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const SPRITE: u32 = 0x258;
        const IMAGE: u32 = 0x400;
        const POINT_SETUP: usize = 0x500;
        const BILERP_SETUP: usize = 0x520;
        let mut rdram = vec![0u8; 0x600];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, color) in [0xf801, 0x003f, 0x07c1, 0xffff]
                .into_iter()
                .cycle()
                .take(8)
                .enumerate()
            {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                    color,
                );
            }
        }
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, POINT_DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, POINT_DL + 8, 0xdf00_0000, 0);
        write_command(&mut rdram, BILERP_DL, 0x0b00_0000, G_OBJRM_BILERP);
        write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
        write_command(&mut rdram, BILERP_SPRITE_DL, 0x0b00_0000, G_OBJRM_BILERP);
        write_command(&mut rdram, BILERP_SPRITE_DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, BILERP_SPRITE_DL + 16, 0x0200_0000, SPRITE);
        write_command(&mut rdram, BILERP_SPRITE_DL + 24, 0xdf00_0000, 0);
        let combine_texel0 = (0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut rdram, POINT_SETUP, combine_texel0.0, combine_texel0.1);
        write_command(&mut rdram, POINT_SETUP + 8, 0xdf00_0000, 0);
        write_command(
            &mut rdram,
            BILERP_SETUP,
            0xef00_0000 | 0x0008_0cff | (2 << 12),
            0,
        );
        write_command(
            &mut rdram,
            BILERP_SETUP + 8,
            combine_texel0.0,
            combine_texel0.1,
        );
        write_command(&mut rdram, BILERP_SETUP + 16, 0xdf00_0000, 0);

        let mut point_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, POINT_SETUP as u32, &mut point_rdp)
            .unwrap();
        let point = decode_ops(&rdram, POINT_DL as u32, &mut point_rdp).unwrap();
        let mut bilerp_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, BILERP_SETUP as u32, &mut bilerp_rdp)
            .unwrap();
        let bilerp = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap();
        let (RenderOp::TextureRectangle(point), RenderOp::TextureRectangle(bilerp)) =
            (&point[0], &bilerp[0])
        else {
            panic!("object rectangles must retain their typed operations")
        };
        assert_eq!((point.s, point.t), (0.0, 0.0));
        assert_eq!((bilerp.s, bilerp.t), (0.0, 0.0));

        let mut point_fb = crate::raster::Framebuffer::new(4, 2);
        point_fb.clear(0, 0, 0, 0);
        point_fb.draw_texture_rectangle(point);
        let mut bilerp_fb = crate::raster::Framebuffer::new(4, 2);
        bilerp_fb.clear(0, 0, 0, 0);
        bilerp_fb.draw_texture_rectangle(bilerp);
        assert_eq!(bilerp_fb.pixels, point_fb.pixels);
        assert_eq!(&bilerp_fb.pixels[..4], [255, 0, 0, 255]);
        assert_eq!(&bilerp_fb.pixels[4..8], [0, 0, 255, 255]);

        let sprite_ops = decode_ops(&rdram, BILERP_SPRITE_DL as u32, &mut bilerp_rdp).unwrap();
        let RenderOp::Triangle(first) = &sprite_ops[0] else {
            panic!("bilerp sprite must lower to triangles")
        };
        assert_eq!(
            first.v.map(|vertex| (vertex.s, vertex.t)),
            [(-0.5, -0.5), (3.5, -0.5), (3.5, 1.5)]
        );
        let mut sprite_fb = crate::raster::Framebuffer::new(4, 2);
        sprite_fb.clear(0, 0, 0, 0);
        for operation in &sprite_ops {
            let RenderOp::Triangle(triangle) = operation else {
                unreachable!("bilerp sprite emits only triangles")
            };
            sprite_fb.draw_triangle(triangle);
        }
        let sample_at = |operation: &RenderOp, x, y| {
            let RenderOp::Triangle(triangle) = operation else {
                unreachable!()
            };
            crate::raster::test_triangle_attribute_sample(
                triangle.v,
                triangle
                    .scissor
                    .unwrap_or_else(|| crate::gbi::ScissorRect::framebuffer(4, 2)),
                x,
                y,
            )
        };
        assert_eq!(sample_at(&sprite_ops[0], 1, 0), (0xaf, Some((2, 3, 3))));
        assert_eq!(sample_at(&sprite_ops[1], 1, 0), (0x50, Some((4, 1, 5))));
        assert_ne!(0xaf & (1 << 2), 0);
        assert_ne!(0x50 & (1 << 4), 0);
        assert_eq!(
            sprite_fb.pixels,
            [
                255, 0, 0, 255, 96, 0, 159, 255, 0, 255, 0, 255, 255, 255, 255, 255, 255, 0, 0,
                255, 0, 0, 255, 255, 0, 223, 32, 255, 159, 255, 159, 255,
            ]
        );

        write_command(&mut rdram, BILERP_DL, 0x0700_002f, TXSP);
        write_command(&mut rdram, BILERP_DL + 8, 0xdf00_0000, 0);
        let before = format!("{bilerp_rdp:?}");
        let error = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap_err();
        assert!(error.to_string().contains("requires G_OBJRM_BILERP"));
        assert_eq!(format!("{bilerp_rdp:?}"), before);

        write_command(
            &mut rdram,
            BILERP_DL,
            0x0b00_0000,
            G_OBJRM_BILERP | G_OBJRM_WIDEN,
        );
        write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
        let error = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap_err();
        assert!(error
            .to_string()
            .contains("G_OBJRM_WIDEN with filtered sampling"));
    }

    #[test]
    fn shrink_modes_match_across_compound_rectangle_matrix_and_rotating_paths() {
        const RECT_DL: usize = 0x100;
        const RELATIVE_DL: usize = 0x120;
        const ROTATING_DL: usize = 0x148;
        const STANDALONE_DL: usize = 0x170;
        const TXSP: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const IMAGE: u32 = 0x500;
        const SETUP: usize = 0x600;
        let mut rdram = vec![0u8; 0x700];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for y in 0..4 {
                for x in 0..4 {
                    let color = match (x, y) {
                        (0, _) => 0xf801,
                        (3, _) => 0x003f,
                        (_, 0) => 0x07c1,
                        (_, 3) => 0xffff,
                        _ => 0xffc1,
                    };
                    view.write_u16(
                        fn64_runtime::RdramAddr::from_offset(IMAGE + (y * 4 + x) * 2),
                        color,
                    );
                }
            }
        }
        write_block_texture(&mut rdram, TXSP, IMAGE, 1);
        write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 0);
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3);
        let mode = G_OBJRM_BILERP | G_OBJRM_SHRINKSIZE_1;
        write_command(&mut rdram, RECT_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, RECT_DL + 8, 0x0700_002f, TXSP);
        write_command(&mut rdram, RECT_DL + 16, 0xdf00_0000, 0);
        write_command(&mut rdram, RELATIVE_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, RELATIVE_DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, RELATIVE_DL + 16, 0x0800_002f, TXSP);
        write_command(&mut rdram, RELATIVE_DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, ROTATING_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, ROTATING_DL + 8, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, ROTATING_DL + 16, 0x0600_002f, TXSP);
        write_command(&mut rdram, ROTATING_DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, STANDALONE_DL, 0x0b00_0000, mode);
        write_command(&mut rdram, STANDALONE_DL + 8, 0x0500_0017, TXSP);
        write_command(&mut rdram, STANDALONE_DL + 16, 0x0100_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 24, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, STANDALONE_DL + 32, 0xda00_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 40, 0x0200_0000, TXSP + 24);
        write_command(&mut rdram, STANDALONE_DL + 48, 0xdf00_0000, 0);
        write_command(&mut rdram, SETUP, 0xef00_0000 | 0x0008_0cff | (2 << 12), 0);
        write_command(&mut rdram, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
        write_command(&mut rdram, SETUP + 16, 0xdf00_0000, 0);

        let mut base_rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut base_rdp).unwrap();
        let rectangle_ops = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp.clone()).unwrap();
        let relative_ops = decode_ops(&rdram, RELATIVE_DL as u32, &mut base_rdp.clone()).unwrap();
        let rotating_ops = decode_ops(&rdram, ROTATING_DL as u32, &mut base_rdp.clone()).unwrap();
        let standalone_ops =
            decode_ops(&rdram, STANDALONE_DL as u32, &mut base_rdp.clone()).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &rectangle_ops[0] else {
            panic!("compound rectangle must remain a rectangle")
        };
        let RenderOp::TextureRectangle(relative) = &relative_ops[0] else {
            panic!("compound matrix-relative rectangle must remain a rectangle")
        };
        assert_eq!(
            (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
            (0.0, 0.0, 3.0, 3.0)
        );
        assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
        assert_eq!(
            (relative.ulx, relative.uly, relative.lrx, relative.lry),
            (4.0, 0.0, 7.0, 3.0)
        );
        let RenderOp::Triangle(first) = &rotating_ops[0] else {
            panic!("compound rotating sprite must lower to triangles")
        };
        assert_eq!(
            first
                .v
                .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
            [
                (4.0, 0.0, 0.0, 0.0),
                (7.0, 0.0, 3.0, 0.0),
                (7.0, 3.0, 3.0, 3.0),
            ]
        );
        let RenderOp::TextureRectangle(standalone_rectangle) = &standalone_ops[0] else {
            unreachable!()
        };
        let RenderOp::TextureRectangle(standalone_relative) = &standalone_ops[1] else {
            unreachable!()
        };
        let RenderOp::Triangle(standalone_rotating) = &standalone_ops[2] else {
            unreachable!()
        };
        assert_eq!(
            (
                standalone_rectangle.ulx,
                standalone_rectangle.lrx,
                standalone_rectangle.s,
            ),
            (rectangle.ulx, rectangle.lrx, rectangle.s)
        );
        assert_eq!(
            (
                standalone_relative.ulx,
                standalone_relative.lrx,
                standalone_relative.s,
            ),
            (relative.ulx, relative.lrx, relative.s)
        );
        assert_eq!(
            standalone_rotating
                .v
                .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
            first
                .v
                .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t))
        );

        let draw = |operations: &[RenderOp]| {
            let mut framebuffer = crate::raster::Framebuffer::new(8, 4);
            framebuffer.clear(0, 0, 0, 0);
            for operation in operations {
                match operation {
                    RenderOp::TextureRectangle(rectangle) => {
                        framebuffer.draw_texture_rectangle(rectangle)
                    }
                    RenderOp::Triangle(triangle) => framebuffer.draw_triangle(triangle),
                    _ => panic!("object path emitted an unexpected operation"),
                }
            }
            framebuffer
        };
        let rectangle_fb = draw(&rectangle_ops);
        let relative_fb = draw(&relative_ops);
        let rotating_fb = draw(&rotating_ops);
        let pixel = |framebuffer: &crate::raster::Framebuffer, x: usize, y: usize| {
            let offset = (y * framebuffer.width as usize + x) * 4;
            framebuffer.pixels[offset..offset + 4].to_vec()
        };
        let triangle_sample = |operation: &RenderOp, x, y| {
            let RenderOp::Triangle(triangle) = operation else {
                unreachable!()
            };
            crate::raster::test_triangle_attribute_sample(
                triangle.v,
                triangle
                    .scissor
                    .unwrap_or_else(|| crate::gbi::ScissorRect::framebuffer(8, 4)),
                x,
                y,
            )
        };
        for coordinate in 0..3 {
            assert_eq!(
                triangle_sample(&rotating_ops[0], coordinate + 4, coordinate),
                (0xaf, Some((2, 3, 3)))
            );
            assert_eq!(
                triangle_sample(&rotating_ops[1], coordinate + 4, coordinate),
                (0x50, Some((4, 1, 5)))
            );
        }
        let corrected_diagonal = [[223, 32, 0, 255], [255, 255, 0, 255], [223, 223, 191, 255]];
        for y in 0..3 {
            for (x, diagonal) in corrected_diagonal.iter().enumerate() {
                let expected = pixel(&rectangle_fb, x, y);
                assert_ne!(expected, [0, 0, 0, 0]);
                assert_eq!(
                    pixel(&relative_fb, x + 4, y),
                    expected,
                    "relative ({x},{y})"
                );
                if x == y {
                    assert_eq!(pixel(&rotating_fb, x + 4, y), *diagonal);
                } else {
                    assert_eq!(pixel(&rotating_fb, x + 4, y), expected);
                }
            }
        }

        write_command(
            &mut rdram,
            RECT_DL,
            0x0b00_0000,
            G_OBJRM_BILERP | G_OBJRM_SHRINKSIZE_2,
        );
        let shrink_two = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp.clone()).unwrap();
        let RenderOp::TextureRectangle(shrink_two) = &shrink_two[0] else {
            unreachable!()
        };
        assert_eq!((shrink_two.lrx, shrink_two.lry), (2.0, 2.0));
        assert_eq!((shrink_two.s, shrink_two.t), (1.0, 1.0));

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 2), 1536);
        let error = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp).unwrap_err();
        assert!(error.to_string().contains("sub-quarter-pixel rounding"));
    }

    #[test]
    fn object_st_flips_reach_rectangle_matrix_and_rotating_paths() {
        const DL: usize = 0x100;
        const TX: u32 = 0x200;
        const MATRIX: u32 = 0x240;
        const RECT: u32 = 0x258;
        const RECT_R: u32 = 0x270;
        const SPRITE: u32 = 0x288;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        for index in 0..8 {
            fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                if index == 3 { 0x003f } else { 0xf801 },
            );
        }
        write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 20);
        write_sprite(&mut rdram, RECT, 4, 2, 0, 2);
        write_sprite(&mut rdram, RECT_R, 4, 2, 0, 2);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(RECT + 23),
                G_OBJ_FLAG_FLIPS,
            );
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(RECT_R + 23),
                G_OBJ_FLAG_FLIPT,
            );
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(SPRITE + 23),
                G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
            );
        }
        write_command(&mut rdram, DL, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 8, 0x0100_0000, RECT);
        write_command(&mut rdram, DL + 16, 0xdc00_0017, MATRIX);
        write_command(&mut rdram, DL + 24, 0xda00_0000, RECT_R);
        write_command(&mut rdram, DL + 32, 0x0200_0000, SPRITE);
        write_command(&mut rdram, DL + 40, 0xdf00_0000, 0);

        let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("base object must lower to a rectangle")
        };
        assert_eq!((rectangle.s, rectangle.dsdx), (3.0, -(1 << 10)));
        assert_eq!(
            rectangle.texture.as_ref().unwrap().sample(rectangle.s, 0.0),
            [0, 0, 255, 255]
        );
        let RenderOp::TextureRectangle(relative) = &operations[1] else {
            panic!("matrix-relative object must lower to a rectangle")
        };
        assert_eq!((relative.ulx, relative.uly), (4.0, 5.0));
        assert_eq!((relative.t, relative.dtdy), (1.0, -(1 << 10)));
        let RenderOp::Triangle(rotating) = &operations[2] else {
            panic!("rotating object must lower to triangles")
        };
        assert_eq!(
            rotating.v.map(|vertex| (vertex.s, vertex.t)),
            [(4.0, 2.0), (0.0, 2.0), (0.0, 0.0)]
        );
    }

    #[test]
    fn conditional_display_lists_call_branch_and_skip_from_public_status_equation() {
        const ROOT: usize = 0x100;
        const CALLEE: usize = 0x180;
        const BRANCH: usize = 0x1c0;
        const ROOT_SPRITE: u32 = 0x240;
        const CALLEE_SPRITE: u32 = 0x258;
        const BRANCH_SPRITE: u32 = 0x270;
        let mut rdram = vec![0u8; 0x400];
        for sprite in [ROOT_SPRITE, CALLEE_SPRITE, BRANCH_SPRITE] {
            write_sprite(&mut rdram, sprite, 4, 2, 0, 2);
        }
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(CALLEE_SPRITE), 16);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(BRANCH_SPRITE), 32);
        }
        let select_pair = |rdram: &mut [u8], pc: usize, target: usize, push: u32| {
            write_command(
                rdram,
                pc,
                (u32::from(G_RDPHALF_0) << 24) | target as u32 & 0xffff,
                1,
            );
            write_command(
                rdram,
                pc + 8,
                (u32::from(G_SELECT_DL) << 24) | (push << 16) | ((target as u32 >> 16) & 0xffff),
                1,
            );
        };
        select_pair(&mut rdram, ROOT, CALLEE, 0);
        select_pair(&mut rdram, ROOT + 16, CALLEE, 0);
        write_command(&mut rdram, ROOT + 32, 0x0100_0000, ROOT_SPRITE);
        write_command(&mut rdram, ROOT + 40, 0xdf00_0000, 0);
        write_command(&mut rdram, CALLEE, 0x0100_0000, CALLEE_SPRITE);
        write_command(&mut rdram, CALLEE + 8, 0xdf00_0000, 0);

        let called = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(called.len(), 2, "second matching select must skip the call");
        let rectangle_x = |operation: &RenderOp| match operation {
            RenderOp::TextureRectangle(rectangle) => rectangle.ulx,
            _ => panic!("selected object lists must emit rectangles"),
        };
        assert_eq!(
            (rectangle_x(&called[0]), rectangle_x(&called[1])),
            (4.0, 0.0)
        );

        select_pair(&mut rdram, ROOT, BRANCH, 1);
        write_command(&mut rdram, BRANCH, 0x0100_0000, BRANCH_SPRITE);
        write_command(&mut rdram, BRANCH + 8, 0xdf00_0000, 0);
        let branched = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(branched.len(), 1, "branch must not resume the root list");
        assert_eq!(rectangle_x(&branched[0]), 8.0);

        write_command(&mut rdram, ROOT, 0x0400_0000, 0);
        let error = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap_err();
        assert!(error.to_string().contains("preceding G_RDPHALF_0"));
    }

    #[test]
    fn segment_and_general_status_writes_drive_compound_objects_and_selected_lists() {
        const ROOT: usize = 0x100;
        const CALLEE: usize = 0x180;
        const BLUE_TX: u32 = 0x200;
        const RED_TXSP: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const MATRIX: u32 = 0x248;
        const BLUE: u32 = 0x400;
        const RED: u32 = 0x410;
        const RED_STATUS: u32 = 0x55;
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                    0x003f,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                    0xf801,
                );
            }
        }
        write_block_texture(&mut rdram, BLUE_TX, 0x0200_0000, 0x22);
        write_block_texture(&mut rdram, RED_TXSP, 0x0200_0010, RED_STATUS);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_object_matrix(&mut rdram, MATRIX, 16, 20, 1 << 10, 1 << 10);

        write_command(&mut rdram, ROOT, 0xdb06_0004, 0x200);
        write_command(&mut rdram, ROOT + 8, 0xdb06_0008, 0x400);
        write_command(&mut rdram, ROOT + 16, 0xdb06_000c, 0x100);
        write_command(&mut rdram, ROOT + 24, 0x0500_0017, 0x0100_0000);
        write_command(&mut rdram, ROOT + 32, 0xdc00_0017, 0x0100_0048);
        write_command(&mut rdram, ROOT + 40, 0xdb08_0000, RED_STATUS);
        write_command(&mut rdram, ROOT + 48, 0x0800_002f, 0x0100_0018);
        write_command(&mut rdram, ROOT + 56, 0xe404_0080, 1);
        write_command(&mut rdram, ROOT + 64, 0x0400_0300, 1);
        write_command(&mut rdram, ROOT + 72, 0xdf00_0000, 0);
        write_command(&mut rdram, CALLEE, 0x0100_0000, 0x0100_0030);
        write_command(&mut rdram, CALLEE + 8, 0xdf00_0000, 0);

        let operations = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(operations.len(), 2);
        let RenderOp::TextureRectangle(relative) = &operations[0] else {
            panic!("segmented compound rectangle must remain typed")
        };
        let RenderOp::TextureRectangle(callee) = &operations[1] else {
            panic!("segmented selected list must emit its rectangle")
        };
        assert_eq!((relative.ulx, relative.uly), (4.0, 5.0));
        assert_eq!((callee.ulx, callee.uly), (0.0, 0.0));
        for rectangle in [relative, callee] {
            assert_eq!(
                rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
                [0, 0, 255, 255],
                "G_MW_GENSTAT must make the red compound reload a cache hit"
            );
        }
    }

    #[test]
    fn segment_table_resolves_background_payload_and_image_together() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        const MODE: usize = 0x300;
        const IMAGE: u32 = 0x1000;
        let mut rdram = vec![0u8; 0x1100];
        write_background_common(&mut rdram, BG, 0x0200_0000, 4, 2, 4, 2, G_BGLT_LOADTILE, 2);
        write_copy_background_init(&mut rdram, BG, 4, 4, G_BGLT_LOADTILE, 2);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                    0x07c1,
                );
            }
        }
        write_command(&mut rdram, DL, 0xdb06_0004, BG);
        write_command(&mut rdram, DL + 8, 0xdb06_0008, IMAGE);
        write_command(&mut rdram, DL + 16, 0x0a00_0000, 0x0100_0000);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
        write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("segmented background must remain a texture rectangle")
        };
        assert_eq!(
            rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
            [0, 255, 0, 255]
        );
    }

    #[test]
    fn legacy_s2dex_move_word_packing_shares_segment_and_status_mechanisms() {
        const DL: usize = 0x100;
        const BLUE_TX: u32 = 0x200;
        const RED_TXSP: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const BLUE: u32 = 0x400;
        const RED: u32 = 0x410;
        const RED_STATUS: u32 = 0x55;
        let mut rdram = vec![0u8; 0x500];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                    0x003f,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                    0xf801,
                );
            }
        }
        write_block_texture(&mut rdram, BLUE_TX, 0x0200_0000, 0x22);
        write_block_texture(&mut rdram, RED_TXSP, 0x0200_0010, RED_STATUS);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);

        // Legacy gMoveWd packs offset in bits 23:8 and index in bits 7:0.
        write_command(&mut rdram, DL, 0xbc00_0406, 0x200);
        write_command(&mut rdram, DL + 8, 0xbc00_0806, 0x400);
        write_command(&mut rdram, DL + 16, 0xc100_0017, 0x0100_0000);
        write_command(&mut rdram, DL + 24, 0xbc00_0008, RED_STATUS);
        write_command(&mut rdram, DL + 32, 0xc300_002f, 0x0100_0018);
        write_command(&mut rdram, DL + 40, 0x0300_0000, 0x0100_0030);
        write_command(&mut rdram, DL + 48, 0xb800_0000, 0);

        let operations = decode_ops_for_family(
            &rdram,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex,
        )
        .unwrap();
        assert_eq!(operations.len(), 2);
        for operation in operations {
            let RenderOp::TextureRectangle(rectangle) = operation else {
                panic!("legacy object rectangles must use the shared typed path")
            };
            assert_eq!(
                rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
                [0, 0, 255, 255],
                "legacy G_MW_GENSTAT must make the red reload a status hit"
            );
        }
    }

    #[test]
    fn colliding_opcode_requires_an_admitted_wire_family() {
        const DL: usize = 0x100;
        const SPRITE: u32 = 0x200;
        let mut rdram = vec![0u8; 0x300];
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

        let operations = decode_ops_for_family(
            &rdram,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex2,
        )
        .unwrap();
        assert_eq!(operations.len(), 1, "S2DEX2 byte 0x01 is ObjRectangle");

        let error = decode_ops_for_family(
            &rdram,
            DL as u32,
            &mut RdpDecodeState::default(),
            S2dexWireFamily::S2dex,
        )
        .unwrap_err();
        assert!(error.to_string().contains("G_BG_1CYC"));
    }

    #[test]
    fn digest_catalog_reports_exact_admitted_wire_families() {
        let text1 = [1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let text2 = [2; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = UcodeCatalog::default();
        assert!(catalog.supported_ucodes().is_empty());
        catalog.admit_text_for(S2dexWireFamily::S2dex, &text1);
        assert_eq!(catalog.supported_ucodes(), &[UcodeId::S2dex]);
        catalog.admit_text(&text2);
        assert_eq!(catalog.supported_ucodes(), SUPPORTED);
        assert_eq!(
            catalog.require_text(&text1).unwrap(),
            S2dexWireFamily::S2dex
        );
        assert_eq!(
            catalog.require_text(&text2).unwrap(),
            S2dexWireFamily::S2dex2
        );
    }

    #[test]
    #[should_panic(expected = "one S2DEX microcode digest cannot identify two wire families")]
    fn digest_catalog_rejects_conflicting_wire_family_metadata() {
        let text = [3; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = UcodeCatalog::default();
        catalog.admit_text_for(S2dexWireFamily::S2dex, &text);
        catalog.admit_text_for(S2dexWireFamily::S2dex2, &text);
    }

    #[test]
    fn copy_background_window_partitions_exhaustively_preserve_wrapped_sample_identity() {
        for image_width in 1..=8 {
            for image_height in 1..=6 {
                for frame_width in 1..=image_width {
                    for frame_height in 1..=image_height {
                        for image_x in 0..image_width {
                            for image_y in 0..image_height {
                                for reverse_s in [false, true] {
                                    for max_source_rows in 1..=image_height {
                                        let window = BackgroundCopyWindow::new(
                                            image_width,
                                            image_height,
                                            frame_width,
                                            frame_height,
                                            image_x,
                                            image_y,
                                            reverse_s,
                                            max_source_rows,
                                            "G_BG_COPY",
                                        )
                                        .unwrap();
                                        let mut observed =
                                            vec![None; (frame_width * frame_height) as usize];
                                        for slice in window.slices() {
                                            assert!(slice.output_x_start < slice.output_x_end);
                                            assert!(slice.output_y_start < slice.output_y_end);
                                            assert!(slice.source_x_start < slice.source_x_end);
                                            assert!(slice.source_y_start < slice.source_y_end);
                                            assert!(slice.output_x_end <= frame_width);
                                            assert!(slice.output_y_end <= frame_height);
                                            assert!(slice.source_x_end <= image_width);
                                            assert!(slice.source_y_end <= image_height);
                                            assert!(
                                                slice.source_y_end - slice.source_y_start
                                                    <= max_source_rows
                                            );
                                            assert_eq!(
                                                slice.output_x_end - slice.output_x_start,
                                                slice.source_x_end - slice.source_x_start
                                            );
                                            assert_eq!(
                                                slice.output_y_end - slice.output_y_start,
                                                slice.source_y_end - slice.source_y_start
                                            );
                                            for output_y in slice.output_y_start..slice.output_y_end
                                            {
                                                for output_x in
                                                    slice.output_x_start..slice.output_x_end
                                                {
                                                    let local_x = output_x - slice.output_x_start;
                                                    let source_x = if slice.reverse_s {
                                                        slice.source_x_end - 1 - local_x
                                                    } else {
                                                        slice.source_x_start + local_x
                                                    };
                                                    let source_y = slice.source_y_start + output_y
                                                        - slice.output_y_start;
                                                    let slot = (output_y * frame_width + output_x)
                                                        as usize;
                                                    assert_eq!(observed[slot], None);
                                                    observed[slot] = Some((source_x, source_y));
                                                }
                                            }
                                        }

                                        for output_y in 0..frame_height {
                                            for output_x in 0..frame_width {
                                                let mapped_x = if reverse_s {
                                                    frame_width - 1 - output_x
                                                } else {
                                                    output_x
                                                };
                                                let expected_linear = (((image_y + output_y)
                                                    % image_height)
                                                    * image_width
                                                    + image_x
                                                    + mapped_x)
                                                    % (image_width * image_height);
                                                let expected = (
                                                    expected_linear % image_width,
                                                    expected_linear / image_width,
                                                );
                                                assert_eq!(
                                                    observed[(output_y * frame_width + output_x)
                                                        as usize],
                                                    Some(expected),
                                                    "image={image_width}x{image_height} frame={frame_width}x{frame_height} origin=({image_x},{image_y}) reverse={reverse_s} rows={max_source_rows} output=({output_x},{output_y})"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn copy_background_window_rejects_non_public_geometry_loudly() {
        for (arguments, expected) in [
            ((0, 2, 1, 1, 0, 0, 1), "dimensions must all be nonzero"),
            ((2, 2, 3, 1, 0, 0, 1), "transfer frame 3x1 exceeds"),
            ((2, 2, 1, 1, 2, 0, 1), "origin (2,0) must be wrapped"),
            ((2, 2, 1, 1, 0, 0, 0), "admits zero source rows"),
        ] {
            let (iw, ih, fw, fh, ix, iy, rows) = arguments;
            let error = BackgroundCopyWindow::new(iw, ih, fw, fh, ix, iy, false, rows, "G_BG_COPY")
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn scaled_background_point_window_exhaustively_preserves_fixed_point_identity() {
        let scales = [1, 511, 1024, 1536, 3072];
        let mut configurations = 0usize;
        for image_width in 1..=5u32 {
            for image_height in 1..=4u32 {
                for frame_width in 1..=7u32 {
                    for frame_height in 1..=4u32 {
                        for image_x_5 in [0, image_width * 16, image_width * 32 - 1] {
                            for image_y in 0..image_height {
                                for scale_w_10 in scales {
                                    for scale_h_10 in [1, 1024, 1536, 3072] {
                                        for reverse_s in [false, true] {
                                            let window = ScaledBackgroundWindow::new(
                                                image_width,
                                                image_height,
                                                frame_width,
                                                frame_height,
                                                image_x_5 as u16,
                                                (image_y * 32) as u16,
                                                scale_w_10,
                                                scale_h_10,
                                                reverse_s,
                                                -64,
                                                "G_BG_1CYC",
                                            )
                                            .unwrap();
                                            let slices = window
                                                .slices(
                                                    BackgroundFilterFootprint::Point,
                                                    "G_BG_1CYC",
                                                )
                                                .unwrap();
                                            let mut observed =
                                                vec![None; (frame_width * frame_height) as usize];
                                            for slice in slices {
                                                assert!(slice.output_x_start < slice.output_x_end);
                                                assert!(slice.output_x_end <= frame_width);
                                                assert!(slice.output_y < frame_height);
                                                assert!(slice.source_x_start < slice.source_x_end);
                                                assert!(slice.source_x_end <= image_width);
                                                assert!(slice.source_y < image_height);
                                                for output_x in
                                                    slice.output_x_start..slice.output_x_end
                                                {
                                                    let local_x = output_x - slice.output_x_start;
                                                    let source_s_10 = i64::from(slice.s_start_10)
                                                        + i64::from(local_x)
                                                            * i64::from(slice.dsdx_10);
                                                    let source_x = i64::from(slice.source_x_start)
                                                        + source_s_10.div_euclid(1024);
                                                    let source_y = i64::from(slice.source_y)
                                                        + i64::from(slice.t_start_10)
                                                            .div_euclid(1024);
                                                    let slot = (slice.output_y * frame_width
                                                        + output_x)
                                                        as usize;
                                                    assert_eq!(observed[slot], None);
                                                    observed[slot] =
                                                        Some((source_x as u32, source_y as u32));
                                                }
                                            }

                                            let row_extent_10 = image_width * 1024;
                                            for output_y in 0..frame_height {
                                                for output_x in 0..frame_width {
                                                    let mapped_x = if reverse_s {
                                                        frame_width - 1 - output_x
                                                    } else {
                                                        output_x
                                                    };
                                                    let source_s_10 = image_x_5 * 32
                                                        + mapped_x * u32::from(scale_w_10);
                                                    let row_carry = source_s_10 / row_extent_10;
                                                    let source_x =
                                                        source_s_10 % row_extent_10 / 1024;
                                                    let source_y_10 = (image_y * 1024
                                                        + output_y * u32::from(scale_h_10)
                                                        + row_carry * 1024)
                                                        % (image_height * 1024);
                                                    assert_eq!(
                                                        observed[(output_y * frame_width + output_x)
                                                            as usize],
                                                        Some((source_x, source_y_10 / 1024)),
                                                        "image={image_width}x{image_height} frame={frame_width}x{frame_height} imageX={image_x_5} scale=({scale_w_10},{scale_h_10}) reverse={reverse_s} output=({output_x},{output_y})"
                                                    );
                                                }
                                            }
                                            configurations += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(configurations, 168_000);
    }

    #[test]
    fn scaled_background_window_keeps_unpublished_footprints_loud() {
        let valid =
            || ScaledBackgroundWindow::new(4, 3, 5, 4, 1, 32, 1536, 1024, false, -64, "G_BG_1CYC");
        assert!(
            valid().is_ok(),
            "fractional imageX and distinct imageYorig are public"
        );
        let error = valid()
            .unwrap()
            .slices(BackgroundFilterFootprint::Bilinear, "G_BG_1CYC")
            .unwrap_err();
        assert!(
            error.to_string().contains("bilinear scaled-background"),
            "{error}"
        );

        for (arguments, expected) in [
            ((4, 3, 5, 4, 128, 32, 1536, 1024, -64), "must be wrapped"),
            ((4, 3, 5, 4, 0, 1, 1536, 1024, -64), "vertical subpixel"),
            ((4, 3, 5, 4, 0, 32, 0, 1024, -64), "nonzero RDP"),
            ((4, 3, 5, 4, 0, 32, 1536, 1024, 1), "sub-texel strip-origin"),
        ] {
            let (iw, ih, fw, fh, ix, iy, sw, sh, origin) = arguments;
            let error = ScaledBackgroundWindow::new(
                iw,
                ih,
                fw,
                fh,
                ix,
                iy,
                sw,
                sh,
                false,
                origin,
                "G_BG_1CYC",
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn admitted_s2dex_families_render_wrapped_copy_background_windows_identically() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        const MODE: usize = 0x300;
        const IMAGE: u32 = 0x1000;
        const COLORS: [u16; 12] = [
            0xf801, 0x07c1, 0x003f, 0xffff, 0xffc1, 0xf83f, 0x07ff, 0x0001, 0xf801, 0x07c1, 0x003f,
            0xffff,
        ];
        let rgba = |color: u16| {
            let expand = |value: u16| ((value << 3) | (value >> 2)) as u8;
            [
                expand((color >> 11) & 0x1f),
                expand((color >> 6) & 0x1f),
                expand((color >> 1) & 0x1f),
                if color & 1 != 0 { 255 } else { 0 },
            ]
        };

        for (family_index, family) in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2]
            .into_iter()
            .enumerate()
        {
            let text = vec![family_index as u8 + 1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
            let mut catalog = UcodeCatalog::default();
            catalog.admit_text_for(family, &text);
            let selected_family = catalog.require_text(&text).unwrap();
            assert_eq!(selected_family, family);
            for (image_load, load_name) in [
                (G_BGLT_LOADTILE, "LoadTile"),
                (G_BGLT_LOADBLOCK, "LoadBlock"),
            ] {
                for flipped in [false, true] {
                    let mut rdram = vec![0u8; 0x1100];
                    write_background_common(&mut rdram, BG, IMAGE, 4, 3, 3, 3, image_load, 2);
                    write_copy_background_init(&mut rdram, BG, 4, 3, image_load, 2);
                    write_background_window(&mut rdram, BG, 3, 2, flipped);
                    let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                    for (index, color) in COLORS.into_iter().enumerate() {
                        view.write_u16(
                            fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                            color,
                        );
                    }
                    let (background_opcode, end_opcode) = match family {
                        S2dexWireFamily::S2dex => (S2DEX_G_BG_COPY, S2DEX_G_ENDDL),
                        S2dexWireFamily::S2dex2 => (G_BG_COPY, G_ENDDL),
                    };
                    write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
                    write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);
                    write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
                    write_command(&mut rdram, MODE + 8, u32::from(G_ENDDL) << 24, 0);

                    let mut rdp = RdpDecodeState::default();
                    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp)
                        .unwrap();
                    let operations =
                        decode_ops_for_family(&rdram, DL as u32, &mut rdp, selected_family)
                            .unwrap();
                    let mut framebuffer = crate::raster::Framebuffer::new(3, 3);
                    for operation in &operations {
                        let RenderOp::TextureRectangle(rectangle) = operation else {
                            panic!("copy window must lower only to texture rectangles");
                        };
                        framebuffer.draw_copy_texture_rectangle(rectangle);
                    }
                    for output_y in 0..3u32 {
                        for output_x in 0..3u32 {
                            let mapped_x = if flipped { 2 - output_x } else { output_x };
                            let source = (((2 + output_y) % 3) * 4 + 3 + mapped_x) % 12;
                            let offset = ((output_y * 3 + output_x) * 4) as usize;
                            assert_eq!(
                            framebuffer.pixels[offset..offset + 4],
                            rgba(COLORS[source as usize]),
                            "family={family:?} load={load_name} flipped={flipped} output=({output_x},{output_y}) source={source}"
                        );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn copy_background_reuses_bounded_scratch_for_every_wire_and_loader_remainder() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        const MODE: usize = 0x300;
        const WIDTH: u16 = 320;
        const HEIGHT: u16 = 8;
        const IMAGE_BYTES: usize = WIDTH as usize * HEIGHT as usize * 2;
        const IMAGE: u32 = (PHYSICAL_RDRAM_BYTES - IMAGE_BYTES) as u32;
        assert_eq!(BackgroundScratch::new().bytes.len(), 8192 + 48);
        assert!(BACKGROUND_SCRATCH_BYTES < IMAGE as usize);
        for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
            for (image_load, load_name) in [
                (G_BGLT_LOADTILE, "LoadTile"),
                (G_BGLT_LOADBLOCK, "LoadBlock"),
            ] {
                let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES];
                write_background_common(
                    &mut rdram, BG, IMAGE, WIDTH, HEIGHT, WIDTH, HEIGHT, image_load, 2,
                );
                write_copy_background_init(&mut rdram, BG, WIDTH, WIDTH, image_load, 2);
                {
                    let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                    for y in 0..HEIGHT {
                        let color = if y < 6 { 0xf801 } else { 0x003f };
                        for x in 0..WIDTH {
                            view.write_u16(
                                fn64_runtime::RdramAddr::from_offset(
                                    IMAGE + (u32::from(y) * u32::from(WIDTH) + u32::from(x)) * 2,
                                ),
                                color,
                            );
                        }
                    }
                }
                let (background_opcode, end_opcode) = match family {
                    S2dexWireFamily::S2dex => (S2DEX_G_BG_COPY, S2DEX_G_ENDDL),
                    S2dexWireFamily::S2dex2 => (G_BG_COPY, G_ENDDL),
                };
                write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
                write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);
                write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
                write_command(&mut rdram, MODE + 8, u32::from(G_ENDDL) << 24, 0);

                let mut rdp = RdpDecodeState::default();
                crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
                let operations =
                    decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap();
                assert_eq!(
                    operations.len(),
                    2,
                    "family={family:?} load={load_name}: six TMEM rows plus two-row remainder"
                );
                let rectangles = operations
                    .iter()
                    .map(|operation| match operation {
                        RenderOp::TextureRectangle(rectangle) => rectangle,
                        _ => panic!("background must lower only to texture rectangles"),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    (
                        rectangles[0].ulx,
                        rectangles[0].uly,
                        rectangles[0].lrx,
                        rectangles[0].lry
                    ),
                    (0.0, 0.0, 319.0, 5.0),
                    "family={family:?} load={load_name} first strip"
                );
                assert_eq!(
                    (
                        rectangles[1].ulx,
                        rectangles[1].uly,
                        rectangles[1].lrx,
                        rectangles[1].lry
                    ),
                    (0.0, 6.0, 319.0, 7.0),
                    "family={family:?} load={load_name} remainder strip"
                );

                let mut framebuffer = crate::raster::Framebuffer::new(WIDTH.into(), HEIGHT.into());
                framebuffer.clear(0, 0, 0, 0);
                for rectangle in rectangles {
                    framebuffer.draw_copy_texture_rectangle(rectangle);
                }
                let pixel = |x: usize, y: usize| {
                    let offset = (y * usize::from(WIDTH) + x) * 4;
                    &framebuffer.pixels[offset..offset + 4]
                };
                assert_eq!(
                    pixel(0, 0),
                    [255, 0, 0, 255],
                    "family={family:?} load={load_name} first pixel"
                );
                assert_eq!(
                    pixel(319, 5),
                    [255, 0, 0, 255],
                    "family={family:?} load={load_name} final full-strip pixel"
                );
                assert_eq!(
                    pixel(0, 6),
                    [0, 0, 255, 255],
                    "family={family:?} load={load_name} first remainder pixel"
                );
                assert_eq!(
                    pixel(319, 7),
                    [0, 0, 255, 255],
                    "family={family:?} load={load_name} final remainder pixel"
                );
            }
        }
    }

    #[test]
    fn scaled_background_maps_source_gradient_and_is_transactional() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        const IMAGE: u32 = 0x1000;
        let mut rdram = vec![0u8; 0x1100];
        write_background_common(&mut rdram, BG, IMAGE, 8, 4, 4, 4, G_BGLT_LOADBLOCK, 2);
        write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 0);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..32 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                    if index & 1 == 0 { 0xf801 } else { 0x003f },
                );
            }
        }
        write_command(&mut rdram, DL, 0x0900_0000, BG);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        assert_eq!(operations.len(), 4);
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("scaled background must lower to a texture rectangle");
        };
        assert_eq!(
            (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
            (0.0, 0.0, 4.0, 1.0)
        );
        assert_eq!((rectangle.dsdx, rectangle.dtdy), (2 << 10, 1 << 10));
        let texture = rectangle.texture.as_ref().unwrap();
        assert_eq!(texture.sample(0.0, 0.0), [255, 0, 0, 255]);
        assert_eq!(texture.sample(1.0, 0.0), [0, 0, 255, 255]);

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 4), 1);
        let quarter_pixel = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(quarter_pixel) = &quarter_pixel[0] else {
            unreachable!()
        };
        assert_eq!((quarter_pixel.ulx, quarter_pixel.lrx), (0.25, 4.25));
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 4), 0);

        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(BG + 26),
            G_BG_FLAG_FLIPS,
        );
        let flipped = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        let RenderOp::TextureRectangle(flipped) = &flipped[0] else {
            panic!("flipped background must remain a texture rectangle");
        };
        assert_eq!((flipped.s, flipped.dsdx), (6.0, -(2 << 10)));

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 26), 0);
        write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 32);
        let distinct_origin =
            decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
        assert_eq!(distinct_origin.len(), operations.len());
        for (left, right) in operations.iter().zip(&distinct_origin) {
            let (RenderOp::TextureRectangle(left), RenderOp::TextureRectangle(right)) =
                (left, right)
            else {
                panic!("scaled backgrounds must lower only to texture rectangles")
            };
            assert_eq!(
                (left.ulx, left.uly, left.lrx, left.lry, left.s, left.t),
                (right.ulx, right.uly, right.lrx, right.lry, right.s, right.t)
            );
        }

        write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 1);
        let mut fresh = RdpDecodeState::default();
        let before = format!("{fresh:?}");
        let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
        assert!(error.to_string().contains("sub-texel strip-origin"));
        assert_eq!(format!("{fresh:?}"), before);
        write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 0);

        write_command(&mut rdram, 0x300, 0xef00_0000 | (2 << 12), 0);
        write_command(&mut rdram, 0x308, 0xdf00_0000, 0);
        let mut bilinear = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, 0x300, &mut bilinear).unwrap();
        let before = format!("{bilinear:?}");
        let error = decode_ops(&rdram, DL as u32, &mut bilinear).unwrap_err();
        assert!(
            error.to_string().contains("bilinear scaled-background"),
            "{error}"
        );
        assert_eq!(format!("{bilinear:?}"), before);

        write_command(&mut rdram, DL + 8, 0x0400_0000, 0);
        let before = format!("{fresh:?}");
        let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
        assert!(error.to_string().contains("G_SELECT_DL"));
        assert_eq!(format!("{fresh:?}"), before);
    }

    #[test]
    fn admitted_s2dex_families_load_scaled_background_wrapped_point_windows_identically() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        const IMAGE: u32 = 0x1000;
        const COLORS: [u16; 12] = [
            0x0801, 0x1001, 0x1801, 0x2001, 0x2801, 0x3001, 0x3801, 0x4001, 0x4801, 0x5001, 0x5801,
            0x6001,
        ];
        let rgba = |color: u16| {
            let expand = |value: u16| ((value << 3) | (value >> 2)) as u8;
            [
                expand((color >> 11) & 0x1f),
                expand((color >> 6) & 0x1f),
                expand((color >> 1) & 0x1f),
                255,
            ]
        };

        for (family_index, family) in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2]
            .into_iter()
            .enumerate()
        {
            let text = vec![family_index as u8 + 9; fn64_runtime::RSP_MEMORY_BANK_SIZE];
            let mut catalog = UcodeCatalog::default();
            catalog.admit_text_for(family, &text);
            let selected_family = catalog.require_text(&text).unwrap();
            for image_load in [G_BGLT_LOADTILE, G_BGLT_LOADBLOCK] {
                for flipped in [false, true] {
                    for image_y_origin in [-64, 64, 128] {
                        let mut rdram = vec![0u8; 0x1100];
                        write_background_common(&mut rdram, BG, IMAGE, 4, 3, 5, 4, image_load, 2);
                        {
                            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                            let base = fn64_runtime::RdramAddr::from_offset(BG);
                            view.write_u16(base, 3 * 32 + 16);
                            view.write_u16(base.checked_add(8).unwrap(), 2 * 32);
                            view.write_u16(
                                base.checked_add(26).unwrap(),
                                if flipped { G_BG_FLAG_FLIPS } else { 0 },
                            );
                            for (index, color) in COLORS.into_iter().enumerate() {
                                view.write_u16(
                                    fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                                    color,
                                );
                            }
                        }
                        write_scale_background_tail(&mut rdram, BG, 1536, 1536, image_y_origin);
                        let (background_opcode, end_opcode) = match family {
                            S2dexWireFamily::S2dex => (S2DEX_G_BG_1CYC, S2DEX_G_ENDDL),
                            S2dexWireFamily::S2dex2 => (G_BG_1CYC, G_ENDDL),
                        };
                        write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
                        write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);

                        let operations = decode_ops_for_family(
                            &rdram,
                            DL as u32,
                            &mut RdpDecodeState::default(),
                            selected_family,
                        )
                        .unwrap();
                        for output_y in 0..4u32 {
                            for output_x in 0..5u32 {
                                let rectangle = operations
                                    .iter()
                                    .find_map(|operation| match operation {
                                        RenderOp::TextureRectangle(rectangle)
                                            if rectangle.ulx <= output_x as f32
                                                && (output_x as f32) < rectangle.lrx
                                                && rectangle.uly <= output_y as f32
                                                && (output_y as f32) < rectangle.lry =>
                                        {
                                            Some(rectangle)
                                        }
                                        _ => None,
                                    })
                                    .expect("scaled slices cover each output pixel exactly once");
                                let local_x = output_x as f32 - rectangle.ulx;
                                let actual = rectangle.texture.as_ref().unwrap().sample(
                                    rectangle.s + local_x * f32::from(rectangle.dsdx) / 1024.0,
                                    rectangle.t,
                                );
                                let mapped_x = if flipped { 4 - output_x } else { output_x };
                                let source_s_10 = (3 * 32 + 16) * 32 + mapped_x * 1536;
                                let row_carry = source_s_10 / (4 * 1024);
                                let source_x = source_s_10 % (4 * 1024) / 1024;
                                let source_y = (2 * 1024 + output_y * 1536 + row_carry * 1024)
                                    % (3 * 1024)
                                    / 1024;
                                assert_eq!(
                                    actual,
                                    rgba(COLORS[(source_y * 4 + source_x) as usize]),
                                    "family={family:?} load={image_load:#06x} flipped={flipped} imageYorig={image_y_origin} output=({output_x},{output_y}) source=({source_x},{source_y})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn background_image_range_cannot_escape_physical_rdram() {
        const DL: usize = 0x100;
        const BG: u32 = 0x200;
        let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES + 16];
        write_background_common(
            &mut rdram,
            BG,
            PHYSICAL_RDRAM_BYTES as u32,
            4,
            2,
            4,
            2,
            G_BGLT_LOADTILE,
            2,
        );
        write_copy_background_init(&mut rdram, BG, 4, 4, G_BGLT_LOADTILE, 2);
        write_command(&mut rdram, DL, 0x0a00_0000, BG);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

        let error = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap_err();
        assert!(error.to_string().contains("exceeds physical/backed RDRAM"));
    }

    #[test]
    fn tlut_and_ci4_tile_loads_feed_object_rectangle() {
        const DL: usize = 0x100;
        const TLUT: u32 = 0x200;
        const TX: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const PALETTE: u32 = 0x400;
        const IMAGE: u32 = 0x500;
        const MODE: usize = 0x600;
        let mut rdram = vec![0u8; 0x700];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..16 {
                let color = if index == 1 { 0xf801 } else { 0x0001 };
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(PALETTE + index * 2),
                    color,
                );
            }
            view.write_u8(fn64_runtime::RdramAddr::from_offset(IMAGE), 0x10);
            for offset in 1..8 {
                view.write_u8(fn64_runtime::RdramAddr::from_offset(IMAGE + offset), 0);
            }
        }
        write_tlut_texture(&mut rdram, TLUT, PALETTE);
        write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_sprite(&mut rdram, SPRITE, 16, 1, 2, 0);
        write_command(&mut rdram, DL, 0x0500_0017, TLUT);
        write_command(&mut rdram, DL + 8, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, MODE, 0xef00_0000 | 0x0008_0cff | (2 << 14), 0);
        write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn single_tlut_entry_copies_its_complete_native_storage_word() {
        const DL: usize = 0x100;
        const TLUT: u32 = 0x200;
        const TX: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const PALETTE: u32 = 0x400;
        const IMAGE: u32 = 0x500;
        const MODE: usize = 0x600;
        let mut rdram = vec![0u8; 0x700];
        write_tlut_texture(&mut rdram, TLUT, PALETTE);
        write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
        write_sprite(&mut rdram, SPRITE, 16, 1, 2, 0);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(TLUT + 10), 0);
            view.write_u16(fn64_runtime::RdramAddr::from_offset(PALETTE), 0xf801);
        }
        write_command(&mut rdram, DL, 0x0500_0017, TLUT);
        write_command(&mut rdram, DL + 8, 0x0500_0017, TX);
        write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
        write_command(&mut rdram, MODE, 0xef00_0000 | 0x0008_0cff | (2 << 14), 0);
        write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn object_status_match_skips_redundant_texture_load() {
        const DL: usize = 0x100;
        const TX_RED: u32 = 0x200;
        const TX_BLUE: u32 = 0x218;
        const SPRITE: u32 = 0x230;
        const RED: u32 = 0x400;
        const BLUE: u32 = 0x500;
        let mut rdram = vec![0u8; 0x600];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for index in 0..8 {
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                    0xf801,
                );
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                    0x003f,
                );
            }
        }
        write_block_texture(&mut rdram, TX_RED, RED, 0x1234_5678);
        write_block_texture(&mut rdram, TX_BLUE, BLUE, 0x1234_5678);
        write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, TX_RED);
        write_command(&mut rdram, DL + 8, 0x0500_0017, TX_BLUE);
        write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
        write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

        let mut rdp = RdpDecodeState::default();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let (texture, other_mode) = rectangle_texture(&operations[0]);
        assert_eq!(
            texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
            [255, 0, 0, 255],
            "matching (Status & mask) == flag must skip the blue reload"
        );
    }

    #[test]
    fn rejected_tail_and_compound_without_matrix_are_transactional_and_named() {
        const DL: usize = 0x100;
        const TXSP: u32 = 0x200;
        const IMAGE: u32 = 0x400;
        let mut rdram = vec![0u8; 0x600];
        write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
        write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
        write_command(&mut rdram, DL, 0x0500_0017, TXSP);
        write_command(&mut rdram, DL + 8, 0x0900_0000, 0);
        let mut rdp = RdpDecodeState::default();
        let before = format!("{rdp:?}");
        let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
        assert!(error.to_string().contains("G_BG_1CYC"));
        assert_eq!(format!("{rdp:?}"), before);

        write_command(
            &mut rdram,
            DL,
            (u32::from(G_OBJ_LDTX_SPRITE) << 24) | 47,
            TXSP,
        );
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
        let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
        assert!(error.to_string().contains("G_OBJ_LDTX_SPRITE"));
        assert!(error.to_string().contains("texture load was not applied"));
        assert_eq!(format!("{rdp:?}"), before);

        write_command(&mut rdram, DL, 0x0800_002f, TXSP);
        write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
        let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
        assert!(error
            .to_string()
            .contains("requires a preceding G_OBJ_MOVEMEM"));
        assert!(error.to_string().contains("texture load was not applied"));
        assert_eq!(format!("{rdp:?}"), before);
    }
}
