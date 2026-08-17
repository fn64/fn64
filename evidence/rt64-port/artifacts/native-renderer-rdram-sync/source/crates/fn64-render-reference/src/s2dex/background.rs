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
use super::common::*;


pub(super) struct BackgroundScratch {
    pub(super) bytes: Vec<u8>,
}

/// One exact load/draw partition of a Copy-mode scrolling window. All fields
/// are whole texels/pixels. Construction is private to [`BackgroundCopyWindow`]
/// so an admitted slice is nonempty, in-bounds, and maps one output rectangle
/// to one contiguous source rectangle without crossing a wrapped image edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BackgroundCopySlice {
    pub(super) output_x_start: u32,
    pub(super) output_x_end: u32,
    pub(super) output_y_start: u32,
    pub(super) output_y_end: u32,
    pub(super) source_x_start: u32,
    pub(super) source_x_end: u32,
    pub(super) source_y_start: u32,
    pub(super) source_y_end: u32,
    pub(super) reverse_s: bool,
}

/// Validated integer Copy-mode window from public S2DEX section 4.1.2. The
/// manual defines the right-edge successor `(imageW - 1, y) -> (0, y + 1)`
/// and vertically/horizontally looped closed areas. Keeping that topology in
/// a typed planner prevents the loader and rectangle emitter from inventing
/// separate wrap rules.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BackgroundCopyWindow {
    pub(super) image_width: u32,
    pub(super) image_height: u32,
    pub(super) frame_width: u32,
    pub(super) frame_height: u32,
    pub(super) image_x: u32,
    pub(super) image_y: u32,
    pub(super) reverse_s: bool,
    pub(super) max_source_rows: u32,
}

impl BackgroundCopyWindow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
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

    pub(super) fn slices(self) -> Vec<BackgroundCopySlice> {
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
pub(super) enum BackgroundFilterFootprint {
    Point,
    Bilinear,
}

impl BackgroundFilterFootprint {
    pub(super) fn from_rdp(filter: TextureFilter, command: &str) -> Result<Self, RenderError> {
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
pub(super) struct BackgroundSubplaneOrigin(i32);

impl BackgroundSubplaneOrigin {
    pub(super) fn new(raw: i32, command: &str) -> Result<Self, RenderError> {
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
pub(super) struct ScaledBackgroundSlice {
    pub(super) output_x_start: u32,
    pub(super) output_x_end: u32,
    pub(super) output_y: u32,
    pub(super) source_x_start: u32,
    pub(super) source_x_end: u32,
    pub(super) source_y: u32,
    pub(super) s_start_10: u32,
    pub(super) t_start_10: u32,
    pub(super) dsdx_10: i32,
}

/// Validated point-sampled `gSPBgRect1Cyc` window. Public S2DEX section 4.1.3
/// specifies u10.5 image origins, u5.10 scale, horizontal flip, and a closed
/// row-major image. The planner splits whenever scaled S crosses an image-row
/// edge, then wraps the row carry through image Y exactly.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ScaledBackgroundWindow {
    pub(super) image_width: u32,
    pub(super) image_height: u32,
    pub(super) frame_width: u32,
    pub(super) frame_height: u32,
    pub(super) image_x_5: u16,
    pub(super) image_y_5: u16,
    pub(super) scale_w_10: u16,
    pub(super) scale_h_10: u16,
    pub(super) reverse_s: bool,
    pub(super) _subplane_origin: BackgroundSubplaneOrigin,
}

impl ScaledBackgroundWindow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
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

    pub(super) fn slices(
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

pub(super) struct ObjectTextureScratch {
    pub(super) bytes: Vec<u8>,
}

pub(super) struct ObjectTextureRdpLoad {
    pub(super) commands: Vec<(u32, u32)>,
    pub(super) image: u32,
    pub(super) image_bytes: usize,
}

impl ObjectTextureScratch {
    pub(super) fn new() -> Self {
        Self {
            bytes: vec![0; OBJECT_TEXTURE_SCRATCH_BYTES],
        }
    }
}

impl BackgroundScratch {
    pub(super) fn new() -> Self {
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
pub(crate) enum ObjectTexture {
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
    pub(super) fn common(self) -> ObjectTextureCommon {
        match self {
            Self::Block { common, .. } | Self::Tile { common, .. } | Self::Tlut { common, .. } => {
                common
            }
        }
    }
}
