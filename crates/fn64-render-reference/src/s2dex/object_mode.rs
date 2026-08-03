// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::gbi::TextureFilter;
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

use super::*;

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
    pub(super) shrink_half_texels: u8,
    pub(super) widen_three_eighths_texel: bool,
}

impl ObjectPerimeter {
    pub(crate) fn is_none(self) -> bool {
        self.shrink_half_texels == 0 && !self.widen_three_eighths_texel
    }

    pub(super) fn corrected_image_5(
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

    pub(super) fn exact_screen_adjustments(
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

    pub(super) fn source_bounds(self, image_5: u16) -> (f32, f32) {
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
    pub(super) fn bilerp(self) -> bool {
        self.filter_correction == ObjectFilterCorrection::Bilinear
    }

    pub(super) fn shrink_half_texels(self) -> u8 {
        self.perimeter.shrink_half_texels
    }

    pub(super) fn widens(self) -> bool {
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
pub(super) struct ObjectAverageShrinkFootprint {
    pub(super) inset_half_texels: u8,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ObjectAverageCell {
    Interior,
    PositiveEdgeClamped,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ObjectAverageAxisFootprint {
    Empty,
    Samples {
        first: ObjectAverageCell,
        last: ObjectAverageCell,
    },
}

impl ObjectAverageShrinkFootprint {
    pub(super) fn from_mode(
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
    pub(super) fn rectangle_start(self, image_5: u16, flipped: bool) -> f32 {
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
    pub(super) fn filter_validation_mode(self, mut mode: ObjectRenderMode) -> ObjectRenderMode {
        debug_assert_eq!(self.inset_half_texels, mode.shrink_half_texels());
        mode.perimeter = ObjectPerimeter::default();
        // Base lowering always constructs the public perimeter-clamped tile.
        // If NOTXCLAMP was requested, the separate four-neighbour proof below
        // establishes that this retained clamp is unobservable.
        mode.texture_clamp = ObjectTextureClamp::Perimeter;
        mode
    }

    pub(super) fn classify_cell(
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
    pub(super) fn validate_axis(
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
pub(super) struct ObjectUnclampedPointFootprint;

/// An Average-filtered rectangle whose disabled RSP perimeter clamp is
/// observationally irrelevant because both neighbours on each axis remain
/// inside the public source image for every emitted sample. The public filter
/// definition supplies the two neighbours per axis; monotonic affine texture
/// coordinates make the endpoint cells a complete proof for the sequence.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ObjectUnclampedAverageFootprint;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ObjectPointDirection {
    Increasing,
    Decreasing,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ObjectPointAxisFootprint {
    Empty,
    MonotonicInterior {
        direction: ObjectPointDirection,
        first_texel: u16,
        last_texel: u16,
    },
}

impl ObjectUnclampedPointFootprint {
    pub(super) fn from_mode(
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

    pub(super) fn classify_texel(
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
    pub(super) fn validate_axis(
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
    pub(super) fn from_mode(
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

    pub(super) fn classify_cell(
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
    pub(super) fn validate_axis(
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
