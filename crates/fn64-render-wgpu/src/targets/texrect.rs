//! CPU-side `TextureRectangle` executor sampling a packet's **own** TMEM.
//!
//! This is the seam that lets a WM2000-title-screen-shaped packet -- a
//! `FillRectangle`, a `LoadBlock`, and a `TextureRectangle` sampling the tile
//! that load filled -- execute end to end. It is the composition half; the
//! texel-fetch half is [`crate::tmem`]'s existing reader
//! ([`crate::sample_point`] over [`crate::TmemByteSource`]), reused verbatim
//! rather than reimplemented here.
//!
//! ## Why the pending post-image, and not committed state
//!
//! Measured, not assumed: of WM2000's 219 decode entries, 86 carry both a
//! `G_TEXRECT` and a TMEM load, 133 carry loads only, and **zero** carry a
//! texrect without a load in the same entry. Every texrecting packet also
//! loads TMEM, so "sample a prior packet's already-committed TMEM" is a shape
//! that never occurs and the blocked case is the only case. A texrect must
//! therefore read texels its own packet loaded, which exist only in the
//! sealed-but-unpublished
//! [`crate::PendingTmemTransaction`]'s post-image until
//! `into_physical_successor`/`publish` runs -- strictly after staging, i.e.
//! after the point this executor has to produce pixels.
//!
//! [`crate::PendingTmemTransaction::pending_image`] is that read path, and
//! its own doc states the three things it does not relax (no publication, no
//! forged snapshot identity, no effect-report participation).
//!
//! ## Coordinate derivation, and why it is integer-only
//!
//! The rasterized pixel extent comes from
//! [`crate::texture_rectangle_vertices`] -- this crate's ported RT64
//! `drawTexRect`/`drawRect` -- via the `RectViewportPixels` the decoder
//! already attached to each texrect-sourced triangle, **never** re-derived
//! from the wire corners. That distinction is load-bearing: fill-cycle UL
//! rounding (`ulx &= !3`) plus RT64's `(coord + 3) >> 2` ceil make the naive
//! wire-corner footprint wrong, and the journal's declared write rows are
//! derived from the same `texture_rectangle_vertices` call
//! (`raw_dpc::mod`'s `plan_texture_rectangle`), so any second derivation here
//! would be the exact drift the access-for-access check exists to catch.
//!
//! Texture coordinates step in integer S10.5, not in the vertices' `f32`
//! texcoords. RT64 produces those floats as `uls as f32 / 32.0` and
//! `lrs as f32 / 32.0` from S10.5 integers; this executor recovers the
//! integer endpoints by the inverse scaling and steps between them in
//! integer arithmetic, because [`crate::sample_point`] consumes
//! [`crate::TextureCoordinateS10_5`] and a float round-trip would introduce
//! a rounding lane the reader's own fixed-point addressing does not have.
//!
//! ## Nonclaims
//!
//! - **No color combiner.** The sampled texel's RGBA8888 is written to the
//!   destination directly. One-cycle/two-cycle combiner evaluation, blending,
//!   alpha compare, dither, and coverage are all outside this executor and
//!   are refused by name rather than silently approximated
//!   ([`TexrectExecutionError::UnsupportedCycleType`]).
//! - **No filtering.** Point sampling only. Three-nearest/bilerp exists in
//!   [`crate::filter_three_nearest_committed_cell`] and is not selected here.
//! - **No GPU work.** This produces the same [`DeviceColorBytes`] domain the
//!   fill executor produces, composing at the identical
//!   `CompletedColorTargetWrite` seam. No pixel-shader parity is claimed
//!   against `tmem_sample.wgsl`.
//! - **No scissor, no perspective, no LOD, no mip.**
//!
//! ## Reuse, not new type
//!
//! No new sampler, no new reader, no new decoder. The texel path is
//! [`crate::sample_point`] over [`crate::TmemByteSource`], which is
//! [`crate::sample_committed_point`]'s own body generalized over the byte
//! source rather than copied: the same shift/mask/mirror/clamp addressing,
//! the same validity-gated physical read with XOR4 and RGBA32 bank
//! splitting, the same format and TLUT decode. A pending read that
//! disagreed with a committed read of the same bytes would be a defect no
//! test comparing them could distinguish from a deliberate difference, so
//! the two are made incapable of disagreeing rather than checked for
//! agreement.
//!
//! `write_pixel` below is the one deliberate duplication, of
//! `targets::fill`'s private function of the same name, and it duplicates
//! it exactly -- see its own doc for why a second, different packing would
//! make the composed image's two halves disagree about their shared format.
//!
//! ## Admitted domain
//!
//! Copy cycle only; one rectangle per packet; a non-negative, non-empty,
//! in-target pixel extent; point sampling; texcoords that recover integer
//! S10.5 endpoints. Everything outside that is a named
//! [`TexrectExecutionError`], never an approximation.
//!
//! ## Scope status
//!
//! DONE for the composed `fill + LoadBlock + texrect` shape this module was
//! written for, proven end to end into guest RDRAM. One-cycle and two-cycle
//! texrects are **deliberately not ported** (a scope boundary this slice
//! chose, not work this module waits on): they need the color combiner
//! evaluated per fragment, which is a separate executor with its own
//! evidence.
//!
//! ## Open questions
//!
//! `step_axis`'s truncating division is a preserved convention, not a
//! verified silicon tie-break; public documentation does not establish the
//! RDP's rounding for interpolated texture coordinates. Likewise
//! `TmemFirstRowParity::Even` is passed unconditionally, which is correct
//! for every tile whose first row is even (all this crate's fixtures) and
//! is a frontier for a tile loaded at an odd row parity.

use crate::targets::oracle::DeviceColorBytes;
use crate::targets::{
    CandidateColorTarget, ColorTargetFormat, ColorTargetKey, CompletedColorTargetWrite,
    TargetError, TargetRectangle,
};
use crate::tmem::{
    sample_point, PendingTmemImage, PointSampleCoordinates, PointSampleError, PointSampleRequest,
    TextureCoordinateS10_5, TileAddressMode, TileCoordinate, TileDescriptor, TileSize,
    TmemFirstRowParity, TmemWordAddress,
};
use crate::{CycleType, ImageFormat, OtherMode, PixelSize, TextureLutMode};

use fn64_render::RectViewportPixels;

/// The already-decoded S10.5 texture-coordinate endpoints and pixel extent
/// one admitted `TextureRectangle` rasterizes over.
///
/// Constructed by [`TexrectDraw::try_from_viewport_and_texcoords`] from the
/// decoder's own `RectViewportPixels` plus the two texcoord pairs RT64's
/// `texture_rectangle_vertices` produced, never from the wire corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectDraw {
    left: u32,
    top: u32,
    /// Half-open, matching `RectViewportPixels`' own convention.
    right: u32,
    bottom: u32,
    s_start: i16,
    t_start: i16,
    s_end: i16,
    t_end: i16,
}

impl TexrectDraw {
    /// Recovers the integer S10.5 endpoints from the `f32` texcoords RT64's
    /// `texture_rectangle_vertices` emitted, and validates the pixel extent.
    ///
    /// `texture_rectangle_vertices` computes `u1 = uls as f32 / 32.0` and
    /// `u2 = lrs as f32 / 32.0` from `i32` S10.5 values; multiplying by 32
    /// recovers the integer exactly for every value the 12-bit wire field
    /// and the `lrs`/`lrt` accumulation can produce, because those stay far
    /// inside f32's 24-bit exactly-representable integer range. A
    /// non-integral product means the caller supplied texcoords this
    /// executor did not produce, which is a named refusal rather than a
    /// silent round.
    pub fn try_from_viewport_and_texcoords(
        viewport: RectViewportPixels,
        upper_left: [f32; 2],
        lower_right: [f32; 2],
    ) -> Result<Self, TexrectExecutionError> {
        if viewport.left < 0 || viewport.top < 0 {
            return Err(TexrectExecutionError::NegativeViewportOrigin { viewport });
        }
        if viewport.right <= viewport.left || viewport.bottom <= viewport.top {
            return Err(TexrectExecutionError::EmptyViewport { viewport });
        }
        // `TextureCoordinateS10_5` is an `i16`, so the recovered endpoint
        // must fit one -- checked, never truncated by an `as` cast.
        let recover = |value: f32, axis: TexrectAxis| -> Result<i16, TexrectExecutionError> {
            let scaled = value * 32.0;
            if !scaled.is_finite() || scaled.fract() != 0.0 {
                return Err(TexrectExecutionError::NonIntegralTexcoord { axis, value });
            }
            if scaled < f32::from(i16::MIN) || scaled > f32::from(i16::MAX) {
                return Err(TexrectExecutionError::TexcoordOutOfRange { axis, value });
            }
            Ok(scaled as i16)
        };
        Ok(Self {
            left: viewport.left as u32,
            top: viewport.top as u32,
            right: viewport.right as u32,
            bottom: viewport.bottom as u32,
            s_start: recover(upper_left[0], TexrectAxis::S)?,
            t_start: recover(upper_left[1], TexrectAxis::T)?,
            s_end: recover(lower_right[0], TexrectAxis::S)?,
            t_end: recover(lower_right[1], TexrectAxis::T)?,
        })
    }

    pub const fn left(self) -> u32 {
        self.left
    }

    pub const fn top(self) -> u32 {
        self.top
    }

    pub const fn right(self) -> u32 {
        self.right
    }

    pub const fn bottom(self) -> u32 {
        self.bottom
    }

    pub const fn width(self) -> u32 {
        self.right - self.left
    }

    pub const fn height(self) -> u32 {
        self.bottom - self.top
    }

    /// The S10.5 coordinate sampled at pixel column `column` of this
    /// rectangle (0-based within the rectangle, not the image).
    ///
    /// Linear in the rectangle's own span, matching the constant `dsdx` the
    /// wire command carries: `lrs = uls + dsdx * uvWidth >> 7`, so
    /// `(lrs - uls)` divided by the pixel width recovers `dsdx` scaled to
    /// one pixel. Computed as a rational step rather than an accumulated
    /// per-pixel add so a rounding error cannot compound across the row --
    /// the numerator stays exact in `i64` for every value the S10.5 range
    /// and a 12-bit rectangle width can produce.
    ///
    /// Truncating division (Rust's `/` on integers) matches the RDP's own
    /// fixed-point coordinate truncation, and is the same direction
    /// `TextureCoordinateS10_5`'s consumers already assume; it is a
    /// preserved convention here, not a verified silicon tie-break.
    pub fn s_at(self, column: u32) -> i16 {
        step_axis(self.s_start, self.s_end, column, self.width())
    }

    /// The S10.5 T coordinate sampled at pixel row `row` of this rectangle.
    pub fn t_at(self, row: u32) -> i16 {
        step_axis(self.t_start, self.t_end, row, self.height())
    }
}

fn step_axis(start: i16, end: i16, index: u32, span: u32) -> i16 {
    debug_assert!(span > 0, "an empty span is refused before this point");
    let delta = i64::from(end) - i64::from(start);
    let stepped = delta * i64::from(index) / i64::from(span);
    // Both endpoints fit `i16` and `index < span`, so the interpolated
    // value lies between them and fits too -- `saturating` names the
    // impossible case rather than wrapping it silently.
    i16::try_from(i64::from(start) + stepped).unwrap_or(if delta < 0 { i16::MIN } else { i16::MAX })
}

/// Which texture axis a texrect diagnostic names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexrectAxis {
    S,
    T,
}

impl core::fmt::Display for TexrectAxis {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::S => formatter.write_str("S"),
            Self::T => formatter.write_str("T"),
        }
    }
}

/// Why an admitted `TextureRectangle` could not be executed against a color
/// target. Every variant is a loud rejection; none mutates the target.
///
/// Not `Eq`: two variants carry the `f32` texcoord that was refused, and an
/// `Eq` impl over `f32` would have to claim a total equality NaN does not
/// satisfy. `PartialEq` is what the error genuinely supports.
#[derive(Clone, Debug, PartialEq)]
pub enum TexrectExecutionError {
    /// This executor writes the sampled texel straight to the destination,
    /// so it claims only the cycle types where that is the RDP's own
    /// behavior. One-cycle and two-cycle run the texel through the color
    /// combiner, which this slice does not evaluate -- refused by name
    /// rather than silently drawing an uncombined texel.
    UnsupportedCycleType {
        cycle_type: CycleType,
    },
    /// A texrect declares no journal write when its destination is not
    /// provable at decode time; reaching the executor with no declared row
    /// means the plan and the decoder disagree.
    NoDeclaredRows,
    NegativeViewportOrigin {
        viewport: RectViewportPixels,
    },
    EmptyViewport {
        viewport: RectViewportPixels,
    },
    NonIntegralTexcoord {
        axis: TexrectAxis,
        value: f32,
    },
    TexcoordOutOfRange {
        axis: TexrectAxis,
        value: f32,
    },
    /// The rectangle's rasterized extent is not inside the color target.
    /// Never clamped: a clamped rectangle would write pixels the RDP never
    /// covers, or drop pixels the journal already declared.
    OutsideTarget {
        key: ColorTargetKey,
        rectangle: TargetRectangle,
    },
    /// No `SetTile`/`SetTileSize` was staged for the sampled tile at this
    /// texrect's own stream position, so there is no descriptor to sample
    /// through. Never defaulted to a zeroed tile, which would silently
    /// sample TMEM word zero.
    UnboundTile,
    /// A resident target's untouched pixels must come from its prior
    /// generation's real bytes; treating their absence as zero would
    /// discard everything outside the rectangle.
    MissingResidentBytes {
        key: ColorTargetKey,
    },
    Sample {
        column: u32,
        row: u32,
        source: PointSampleError,
    },
    Target(TargetError),
}

impl core::fmt::Display for TexrectExecutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedCycleType { cycle_type } => write!(
                formatter,
                "execute_texture_rectangle does not evaluate the color combiner, so it admits \
                 only Copy cycle; got {cycle_type:?}"
            ),
            Self::NoDeclaredRows => formatter.write_str(
                "execute_texture_rectangle was given no declared destination rows; a texrect \
                 that declared none must not reach the executor",
            ),
            Self::NegativeViewportOrigin { viewport } => write!(
                formatter,
                "texture rectangle viewport origin is negative (scissoring is not implemented): \
                 {viewport:?}"
            ),
            Self::EmptyViewport { viewport } => write!(
                formatter,
                "texture rectangle viewport is empty or reversed: {viewport:?}"
            ),
            Self::NonIntegralTexcoord { axis, value } => write!(
                formatter,
                "texture rectangle {axis} texcoord {value} does not recover an integer S10.5 \
                 coordinate when scaled by 32"
            ),
            Self::TexcoordOutOfRange { axis, value } => write!(
                formatter,
                "texture rectangle {axis} texcoord {value} is outside the S10.5 range"
            ),
            Self::OutsideTarget { key, rectangle } => write!(
                formatter,
                "texture rectangle {rectangle:?} is not inside color target {key:?}"
            ),
            Self::UnboundTile => formatter.write_str(
                "execute_texture_rectangle requires a SetTile and SetTileSize staged for the \
                 sampled tile at this texrect's own stream position",
            ),
            Self::MissingResidentBytes { key } => write!(
                formatter,
                "execute_texture_rectangle requires resident_bytes for already-resident target \
                 {key:?}; treating a resident candidate as if it had no prior content would \
                 silently discard everything outside the rectangle"
            ),
            Self::Sample {
                column,
                row,
                source,
            } => write!(
                formatter,
                "texture rectangle texel fetch failed at pixel ({column}, {row}): {source}"
            ),
            Self::Target(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TexrectExecutionError {}

impl From<TargetError> for TexrectExecutionError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

/// The complete tile state one texrect samples through -- the typed
/// counterpart of `PlanCollector`'s neutral `SetTile`/`SetTileSize`
/// snapshot, converted once at the executor boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectTileBinding {
    descriptor: TileDescriptor,
    size: TileSize,
}

impl TexrectTileBinding {
    /// Converts `fn64_render`'s neutral `SetTile`/`SetTileSize` mirrors --
    /// the only shape a plan-walking visitor sees -- into this crate's typed
    /// tile pair.
    ///
    /// Field-for-field, with each field's own range check kept
    /// (`TmemWordAddress::try_new`'s nine-bit field,
    /// `TileCoordinate::try_new`'s twelve-bit field): the neutral mirrors are
    /// plain integers, so widening them into the typed newtypes without
    /// their checks would be the one place a wire value could escape the
    /// range its type promises.
    ///
    /// Deliberately carries `palette`, which
    /// [`crate::TileBindingParams::from_neutral`] does not: that struct is a
    /// GPU uniform layout and CI4 palette selection is not one of its
    /// fields, whereas the CPU reader's indexed path requires it.
    pub fn try_from_neutral(
        descriptor: fn64_render::NeutralTileDescriptor,
        size: fn64_render::NeutralTileSize,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            descriptor: TileDescriptor::from_neutral_parts(
                neutral_image_format(descriptor.format),
                neutral_pixel_size(descriptor.size),
                descriptor.line_words,
                TmemWordAddress::try_new(descriptor.tmem_word_address)?,
                descriptor.palette,
                TileAddressMode::from_mirror_clamp(
                    descriptor.t_mode.mirror,
                    descriptor.t_mode.clamp,
                ),
                descriptor.mask_t,
                descriptor.shift_t,
                TileAddressMode::from_mirror_clamp(
                    descriptor.s_mode.mirror,
                    descriptor.s_mode.clamp,
                ),
                descriptor.mask_s,
                descriptor.shift_s,
            ),
            size: TileSize::from_coordinates(
                TileCoordinate::try_new(size.low_s)?,
                TileCoordinate::try_new(size.low_t)?,
                TileCoordinate::try_new(size.high_s)?,
                TileCoordinate::try_new(size.high_t)?,
            ),
        })
    }

    pub const fn descriptor(self) -> TileDescriptor {
        self.descriptor
    }

    pub const fn size(self) -> TileSize {
        self.size
    }
}

fn neutral_image_format(format: fn64_render::NeutralImageFormat) -> ImageFormat {
    match format {
        fn64_render::NeutralImageFormat::Rgba => ImageFormat::Rgba,
        fn64_render::NeutralImageFormat::Yuv => ImageFormat::Yuv,
        fn64_render::NeutralImageFormat::ColorIndex => ImageFormat::ColorIndex,
        fn64_render::NeutralImageFormat::IntensityAlpha => ImageFormat::IntensityAlpha,
        fn64_render::NeutralImageFormat::Intensity => ImageFormat::Intensity,
    }
}

fn neutral_pixel_size(size: fn64_render::NeutralPixelSize) -> PixelSize {
    match size {
        fn64_render::NeutralPixelSize::Bits4 => PixelSize::Bits4,
        fn64_render::NeutralPixelSize::Bits8 => PixelSize::Bits8,
        fn64_render::NeutralPixelSize::Bits16 => PixelSize::Bits16,
        fn64_render::NeutralPixelSize::Bits32 => PixelSize::Bits32,
    }
}

/// Executes one admitted `TextureRectangle` against `candidate`, sampling
/// every texel from `tmem` -- which is a
/// [`PendingTmemImage`], the post-image of the **same packet's** own TMEM
/// loads.
///
/// Produces the same [`CompletedColorTargetWrite`] the fill executor
/// produces, so the two compose at the identical
/// `admit_completed_initialization` seam. `resident_bytes` carries the
/// target's current full-extent device bytes and is **required**: a texrect
/// always writes a sub-rectangle, so every pixel outside it must come from
/// real prior content. In the composed fill+texrect shape those prior bytes
/// are the fill's own output, which is why this executor runs after the fill
/// and takes its bytes as input rather than writing into a separate buffer.
///
/// `already_initialized` is the region `resident_bytes` was itself proven
/// to cover -- the fill's own claimed rectangle in the composed shape,
/// `None` when the bytes come from a resident whose coverage this executor
/// does not re-establish. It only widens the claimed output rectangle; it
/// never changes a pixel.
///
/// Ordering is therefore load-bearing and observable: this reads `tmem`'s
/// post-image, so a `LoadBlock` staged before this call is visible and one
/// staged after is not.
#[allow(clippy::too_many_arguments)]
pub fn execute_texture_rectangle(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    draw: TexrectDraw,
    tile: TexrectTileBinding,
    tmem: &PendingTmemImage<'_>,
    lut_mode: TextureLutMode,
    resident_bytes: &[u8],
    already_initialized: Option<TargetRectangle>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    // Copy cycle blits the texel to the destination with no combiner, which
    // is exactly what this executor does. One/two-cycle would need the
    // color combiner evaluated; Fill cycle does not sample a texture at all.
    // Refused by name rather than drawing an uncombined texel and calling it
    // a rendered frame.
    if !matches!(other_mode.cycle_type(), CycleType::Copy) {
        return Err(TexrectExecutionError::UnsupportedCycleType {
            cycle_type: other_mode.cycle_type(),
        });
    }

    let key = candidate.key();
    let format = key.format();
    let extent = key.extent();
    let rectangle = TargetRectangle::try_new(draw.left(), draw.top(), draw.width(), draw.height())?;
    if draw.right() > extent.width() || draw.bottom() > extent.height() {
        return Err(TexrectExecutionError::OutsideTarget { key, rectangle });
    }
    // Planned, not just bounds-checked: `plan_rows` is the target's own
    // row-planning authority and rejects the same out-of-bounds cases with
    // its own named error. Calling it keeps this executor and the fill
    // executor on one row planner.
    let _plan = candidate.plan_rows(rectangle)?;

    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let full_len = (extent.pixels() as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: extent.pixels() as usize,
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;
    if resident_bytes.len() != full_len {
        return Err(TargetError::CompletedByteLengthMismatch {
            key,
            generation: candidate.generation(),
            expected: full_len,
            actual: resident_bytes.len(),
        }
        .into());
    }
    let mut bytes = resident_bytes.to_vec();

    for row in 0..draw.height() {
        let t = draw.t_at(row);
        for column in 0..draw.width() {
            let s = draw.s_at(column);
            // The one texel fetch. `sample_point` is `tmem/sample.rs`'s
            // existing sampler, monomorphized over the pending post-image
            // rather than over durable state -- the same shift/mask/mirror/
            // clamp addressing, the same validity-gated physical read, the
            // same format and TLUT decode. There is no second sampler.
            let request = PointSampleRequest::new(
                PointSampleCoordinates::new(
                    TextureCoordinateS10_5::from_raw(s),
                    TextureCoordinateS10_5::from_raw(t),
                ),
                TmemFirstRowParity::Even,
            );
            let decoded = sample_point(tmem, tile.descriptor(), tile.size(), request, lut_mode)
                .map_err(|source| TexrectExecutionError::Sample {
                    column,
                    row,
                    source,
                })?;
            let rgba = decoded.texel().rgba8888();
            let pixel_x = draw.left() + column;
            let pixel_y = draw.top() + row;
            let offset =
                (pixel_y as usize * extent.width() as usize + pixel_x as usize) * bytes_per_pixel;
            write_pixel(format, &mut bytes[offset..offset + bytes_per_pixel], rgba);
        }
    }

    let device_bytes = DeviceColorBytes::new_for_fill(key, candidate.generation(), format, bytes)?;
    // The claimed rectangle is the union of what this texrect covered and
    // what `already_initialized` says the incoming `resident_bytes` already
    // proved -- not the texrect's own rectangle alone.
    //
    // `admit_completed_initialization` reads this rectangle to decide
    // whether a brand-new target is fully initialized, and it is right to:
    // every byte of a new target must come from a proven write. In the
    // composed fill+texrect shape those bytes DO all come from proven
    // writes, just from two of them -- the fill initialized the whole
    // extent and this executor patched a sub-rectangle of that same buffer.
    // Reporting only the sub-rectangle would understate what the buffer
    // proves and be rejected; reporting the full extent unconditionally
    // would overstate it for a texrect with no fill under it. The union is
    // the honest answer, and the caller supplies the other half rather than
    // this executor assuming one.
    let claimed = union_rectangle(rectangle, already_initialized);
    Ok(CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        claimed,
        device_bytes,
    ))
}

/// The smallest rectangle containing both, or `covered` alone when there is
/// no prior proven region.
fn union_rectangle(
    covered: TargetRectangle,
    already_initialized: Option<TargetRectangle>,
) -> TargetRectangle {
    let Some(prior) = already_initialized else {
        return covered;
    };
    let x = covered.x().min(prior.x());
    let y = covered.y().min(prior.y());
    let right = (covered.x() + covered.width()).max(prior.x() + prior.width());
    let bottom = (covered.y() + covered.height()).max(prior.y() + prior.height());
    TargetRectangle::try_new(x, y, right - x, bottom - y)
        .expect("a union of two in-bounds rectangles is in bounds")
}

/// Packs one decoded RGBA8888 texel into the target's own pixel format.
///
/// Mirrors `targets::fill`'s private `write_pixel` exactly -- deliberately
/// the identical truncation (`>> 3` per color channel, `>> 7` for alpha into
/// RGBA16's single coverage bit), so a texel and a fill color written to the
/// same target agree on what a pixel value means. A second, different
/// packing would make the composed image's two halves disagree about the
/// format they share.
fn write_pixel(format: ColorTargetFormat, dest: &mut [u8], rgba: [u8; 4]) {
    let [red, green, blue, alpha] = rgba;
    match format {
        ColorTargetFormat::Rgba16 => {
            let packed = (u16::from(red >> 3) << 11)
                | (u16::from(green >> 3) << 6)
                | (u16::from(blue >> 3) << 1)
                | u16::from(alpha >> 7);
            dest.copy_from_slice(&packed.to_be_bytes());
        }
        ColorTargetFormat::Rgba32 => {
            dest.copy_from_slice(&[red, green, blue, alpha]);
        }
    }
}
