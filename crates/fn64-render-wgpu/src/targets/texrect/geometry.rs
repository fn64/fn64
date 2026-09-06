use super::*;

/// The already-decoded S10.5 texture-coordinate endpoints and pixel extent
/// one admitted `TextureRectangle` rasterizes over.
///
/// Constructed by [`TexrectDraw::try_from_viewport_and_texcoords`] from the
/// decoder's own `RectViewportPixels` plus the two texcoord pairs RT64's
/// `texture_rectangle_vertices` produced, never from the wire corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectDraw {
    pub(super) left: u32,
    pub(super) top: u32,
    /// Half-open, matching `RectViewportPixels`' own convention.
    pub(super) right: u32,
    pub(super) bottom: u32,
    pub(super) s_start: i16,
    pub(super) t_start: i16,
    pub(super) s_end: i16,
    pub(super) t_end: i16,
    pub(super) flipped_axes: bool,
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
            flipped_axes: false,
        })
    }

    /// Selects `TextureRectangleFlip` stepping: S advances down rows and T
    /// advances across columns. The endpoint construction has already
    /// swapped the rectangle width/height domains, so the rasterizer only
    /// swaps which screen axis consumes each endpoint pair.
    pub const fn with_flipped_axes(mut self) -> Self {
        self.flipped_axes = true;
        self
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

    /// The S/T pair at one destination pixel, applying opcode `0x25`'s
    /// transposed screen-axis assignment when requested.
    pub fn coordinates_at(self, column: u32, row: u32) -> (i16, i16) {
        if self.flipped_axes {
            (
                step_axis(self.s_start, self.s_end, row, self.height()),
                step_axis(self.t_start, self.t_end, column, self.width()),
            )
        } else {
            (self.s_at(column), self.t_at(row))
        }
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

/// The RDP's latched scissor rectangle (`G_SETSCISSOR`, opcode `0x2d`), in
/// the **quarter-pixel (10.2 fixed-point) wire units the command carries**.
///
/// ## Why quarter-pixels and not pixels
///
/// Public libultra's `gDPSetScissor` macro encodes each coordinate multiplied
/// by four into one of four twelve-bit fields
/// (`/Users/jer/Code/aki-recomp/refs/oot-decomp/include/ultra64/gbi.h:4794-4804`),
/// while `gDPSetScissorFrac` places already-fractional values in those same
/// fields (`:4807-4817`). The command therefore carries all four bounds in
/// quarter-pixel units.
///
/// Storing pixels here instead would have to round at latch time, before the
/// comparison hardware performs, and a sub-pixel scissor edge would then clip
/// the wrong column.
///
/// `mode` is carried so the two-bit value survives the round trip, and is
/// **not** consulted by the texrect clip: this executor renders progressive
/// full-frame targets, where every scanline is drawn. This is fn64's own
/// reading of the mode's role in this path and is not independently confirmed
/// against an allowed hardware reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RdpScissorRect {
    upper_left_x: u16,
    upper_left_y: u16,
    lower_right_x: u16,
    lower_right_y: u16,
    mode: u8,
}

impl RdpScissorRect {
    /// Latches one decoded `G_SETSCISSOR`, in the quarter-pixel units the
    /// wire carries. No rounding, no reordering: the RDP latches whatever
    /// four values arrive, including a reversed or empty rect, and the
    /// *clip* -- not the latch -- is where an empty result becomes visible.
    pub const fn from_wire_quarter_pixels(
        mode: u8,
        upper_left_x: u16,
        upper_left_y: u16,
        lower_right_x: u16,
        lower_right_y: u16,
    ) -> Self {
        Self {
            upper_left_x,
            upper_left_y,
            lower_right_x,
            lower_right_y,
            mode,
        }
    }

    pub const fn mode(self) -> u8 {
        self.mode
    }

    pub const fn upper_left_x(self) -> u16 {
        self.upper_left_x
    }

    pub const fn upper_left_y(self) -> u16 {
        self.upper_left_y
    }

    pub const fn lower_right_x(self) -> u16 {
        self.lower_right_x
    }

    pub const fn lower_right_y(self) -> u16 {
        self.lower_right_y
    }

    /// The half-open pixel column range `[first, limit)` this scissor
    /// admits, and likewise for rows.
    ///
    /// **The rounding is angrylion's, derived from its comparison and not
    /// invented here.** The edgewalker's X clamp (`:2349-2363`) drives a
    /// span endpoint to `clipxhshift = clip.xh << 1` on the low side and to
    /// `clipxlshift = clip.xl << 1` on the high side, both in 1/8-pixel
    /// units, and the span is then consumed at whole-pixel granularity by
    /// `span[j].lx/rx`. A low edge at quarter-pixel `q` therefore first
    /// admits pixel `ceil(q / 4)` -- any coverage strictly left of the
    /// scissor edge is driven out -- and a high edge at `q` last admits the
    /// pixel below it, giving an exclusive limit of `ceil(q / 4)` as well.
    /// Both are the same ceiling because `clip.xl` is itself an exclusive
    /// bound: `curover` fires on `>= clipxlshift`, not `>`.
    ///
    /// fn64's own reference renderer computes the identical thing at
    /// `fn64-render-reference/src/raster/draw.rs:193-203`, differing only in
    /// that it takes the rect pre-divided into `f32` pixels and so writes
    /// the ceiling as `(scissor.ulx - 0.5).ceil()`.
    const fn quarter_to_pixel_ceil(quarter: u16) -> u32 {
        (quarter as u32).div_ceil(4)
    }

    /// First admitted pixel column, inclusive.
    pub const fn first_column(self) -> u32 {
        Self::quarter_to_pixel_ceil(self.upper_left_x)
    }

    /// One past the last admitted pixel column.
    pub const fn column_limit(self) -> u32 {
        Self::quarter_to_pixel_ceil(self.lower_right_x)
    }

    /// First admitted pixel row, inclusive.
    pub const fn first_row(self) -> u32 {
        Self::quarter_to_pixel_ceil(self.upper_left_y)
    }

    /// One past the last admitted pixel row.
    pub const fn row_limit(self) -> u32 {
        Self::quarter_to_pixel_ceil(self.lower_right_y)
    }
}

/// The result of clipping one texrect's rasterized extent against the
/// scissor and the colour target, expressed as offsets **into the
/// rectangle's own span** so the texture-coordinate ramp stays anchored at
/// the unclipped origin.
///
/// ## Why the ramp must not move
///
/// `rdp_tex_rect` loads the S/T origin into `ewdata[24]` and the per-pixel
/// steps into `ewdata[26..39]` once, from the *unclipped* command
/// (`rasterizer.c:2657-2677`), and the edgewalker then clips the span
/// without touching them (`:2349-2363` writes only `majorx`/`minorx`).
/// A clipped rectangle therefore samples the SAME texel at a given screen
/// pixel that the unclipped one would have -- the texture does not slide.
/// Recomputing `s_start` from the clipped left edge would slide it, which is
/// why this carries offsets rather than a narrowed [`TexrectDraw`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClippedTexrectExtent {
    pub(super) first_column: u32,
    pub(super) column_limit: u32,
    pub(super) first_row: u32,
    pub(super) row_limit: u32,
}

impl ClippedTexrectExtent {
    /// Column offsets, relative to the rectangle's own left edge.
    pub const fn columns(self) -> core::ops::Range<u32> {
        self.first_column..self.column_limit
    }

    /// Row offsets, relative to the rectangle's own top edge.
    pub const fn rows(self) -> core::ops::Range<u32> {
        self.first_row..self.row_limit
    }
}

/// Clips `draw`'s rasterized extent against `scissor` and then against the
/// colour target's extent, returning the surviving sub-span as offsets into
/// the rectangle.
///
/// ## Precedence: the scissor is the authority, the target is a second bound
///
/// Both are applied, and neither substitutes for the other. Pinned RT64
/// intersects its scissor rectangle with the draw rectangle before recording
/// the surviving colour/depth extent
/// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`),
/// so a tighter scissor suppresses pixels the draw rectangle otherwise covers.
/// Separately, no span may name
/// memory outside the target, which is this executor's own invariant and not
/// a hardware one: the RDP would happily scribble past the end of a
/// colour image, but fn64's target is a sized buffer and a write past it is
/// a defect, not content. Intersecting both is therefore strictly narrower
/// than either.
///
/// ## What this REFUSES rather than clips
///
/// An empty result. Once the intersection is taken, an extent with no
/// surviving column or row is not a rectangle that draws nothing quietly --
/// it is either a genuinely off-screen primitive or a reversed/degenerate
/// scissor, and both are worth naming. `ScissoredAway` carries the rect and
/// the scissor so the reader can tell which.
///
/// The old [`TexrectExecutionError::OutsideTarget`] refusal it replaces was
/// wrong for the opposite reason: it refused whenever ANY part of the
/// rectangle overhung, which for a rectangle straddling the framebuffer edge
/// is completely routine content the RDP draws every frame.
pub(super) fn clip_texrect_extent(
    draw: TexrectDraw,
    scissor: RdpScissorRect,
    extent_width: u32,
    extent_height: u32,
    key: ColorTargetKey,
    rectangle: TargetRectangle,
) -> Result<ClippedTexrectExtent, TexrectExecutionError> {
    // Screen-space intersection of three half-open spans: the rectangle's
    // own, the scissor's, and the target's. `saturating_sub` below then
    // rebases the survivor onto the rectangle's origin; it cannot underflow
    // because `first` is already `>= draw.left()`.
    let first_x = draw.left().max(scissor.first_column());
    let limit_x = draw.right().min(scissor.column_limit()).min(extent_width);
    let first_y = draw.top().max(scissor.first_row());
    let limit_y = draw.bottom().min(scissor.row_limit()).min(extent_height);
    if first_x >= limit_x || first_y >= limit_y {
        return Err(TexrectExecutionError::ScissoredAway {
            key,
            rectangle,
            scissor,
        });
    }
    Ok(ClippedTexrectExtent {
        first_column: first_x - draw.left(),
        column_limit: limit_x - draw.left(),
        first_row: first_y - draw.top(),
        row_limit: limit_y - draw.top(),
    })
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
