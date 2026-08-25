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
//! ## The one-cycle color combiner
//!
//! Copy cycle blits the sampled texel; **one-cycle routes it through the
//! color combiner** ([`crate::combiner::run_one_cycle`]) as `Texel0`, with
//! `Primitive` and `Environment` supplied from the `G_SETPRIMCOLOR`/
//! `G_SETENVCOLOR` registers current at the texrect's own stream position.
//! That evaluator is not new here and is not a second copy: it is the same
//! public function the triangle pipeline's own WGSL is checked against
//! (`production.rs`'s real-`WgpuBackend` agreement test), reached through
//! [`crate::combiner::combiner_inputs_from_fragment_registers`] so both
//! consumers normalize the constant registers by one assembly.
//!
//! Measured, not assumed: WWF WrestleMania 2000's boot-through-attract
//! window issues 2,520 texrects, **all one-cycle and none Copy**, running
//! exactly two combiner programs between them, reading only `Texel0`,
//! `Primitive`, `Environment`, `Zero` and `One`
//! (`docs/RT64-WM2000-CYCLE-MODES.md` §§1-2). Every other selector --
//! `Shade`, `Texel1`, the LOD fractions, noise, the chroma key -- is
//! refused **by name** at [`TexrectShading::validate_combiner_program`],
//! before any pixel is produced, so a title that needs one gets a loud
//! error rather than pixels combined against a zero this executor invented.
//! `Combined` is admitted in exactly one place, a two-cycle program's
//! second slice, where a first-cycle result exists for it to read.
//!
//! ## Nonclaims
//!
//! - **Two-cycle runs; Fill cycle does not.** Two-cycle evaluates through
//!   [`crate::combiner::run_two_cycle`] -- cycle 0's bitfield slice, then
//!   cycle 1's over the accumulator cycle 0 wrote, with the cross-cycle
//!   carry wrap between them. Its second slice is the one place `Combined`
//!   is an admitted selector, matching the reference lane's rule that
//!   `COMBINED` before a first-cycle result exists is a refusal. `Texel1`
//!   stays refused in both slices: a rectangle binds one tile, so there is
//!   no decoded tile+1 to sample -- the reference refuses it for that same
//!   reason (`backend/validate.rs:479-483`). Fill cycle remains refused
//!   by name ([`TexrectExecutionError::UnsupportedCycleType`]). n64brew
//!   documents what it should do ("In FILL mode this behaves identically to
//!   Fill Rectangle") and the reference lane implements it, so this refusal
//!   is a known lane gap rather than an unknown -- but closing it is not a
//!   widening of the cycle match, because this path's rectangle rule and
//!   the fill path's differ by a pixel on every axis and no `FillColor`
//!   reaches here. [`TexrectExecutionError::UnsupportedCycleType`]'s own
//!   doc carries the measured numbers and the three-part shape of the fix.
//! - **Three of the four post-combiner stages now run**, each through
//!   this crate's already-landed port rather than a second copy:
//!   the **blender** (`crate::blend::blend_fragment`), **alpha compare**
//!   and **alpha dither** (`crate::alpha_compare`), and **coverage**
//!   (`crate::coverage`). [`TexrectFragmentStages::try_new`] and
//!   [`require_blendable_mode`] gate their admitted subsets, refusing
//!   every other mode **by name** before any pixel is produced.
//! - **RGB dither is the one stage this executor does NOT run.** It is
//!   the identity in every mode, exactly as before. Not an oversight: the
//!   workspace's two ports disagree on both the Bayer table (8 of 16
//!   cells, pinned by `rgb_dither.rs`'s own test) and the arithmetic
//!   (RT64 adds-then-truncates, the reference bumps conditionally --
//!   witness: channel 1 at threshold 0 gives 5-bit 0 vs 1). Refusing
//!   instead would decline the power-on default `MagicSquare` that this
//!   crate's own fixtures latch. Named frontier, unchanged behaviour.
//! - **The `Noise` dither modes run at a proven endpoint, not a guessed
//!   sample.** See [`NOISE_DITHER_THRESHOLD`]: over all 256 inputs the
//!   mode's output set is exactly `{floor, floor + 1}` in the five-bit
//!   channel, and the maximum threshold selects `floor`. This crate has
//!   no authority for the RDP's random sequence and does not invent one.
//! - **`alphaCompareValue` is still discarded, and now for a different,
//!   narrower reason.** [`run_one_cycle`]'s second return is the
//!   *combiner's* alpha output; the gate this executor runs takes its
//!   comparand from the post-coverage fragment alpha, which is that value
//!   after [`apply_coverage_alpha`] may have replaced it under
//!   `ALPHA_CVG_SEL` (`raster/draw.rs:243-263` applies coverage-alpha
//!   before comparing). Reading the pre-coverage out-parameter would
//!   bypass that. The value reaches the gate through `combined[3]`
//!   instead, so nothing is dropped -- only the redundant channel is.
//! - **No filtering.** Point sampling only. Three-nearest/bilerp exists in
//!   [`crate::filter_three_nearest_committed_cell`] and is not selected
//!   here.
//! - **None of the three new stages is validated by the WM2000 oracle
//!   comparison**, and the card says so rather than implying otherwise:
//!   all four captured entries latch `G_AC_NONE`, `CVG_X_ALPHA` and
//!   `ALPHA_CVG_SEL` clear, so that differential would not detect a
//!   defect in alpha compare or coverage-alpha at all. Their evidence is
//!   hand-derived characterization instead
//!   (`fragment_stage_tests`).
//! - **No Shade.** This executor has no vertex-interpolated color to
//!   supply, so a program reading `Shade` is refused rather than combined
//!   against zero.
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
//! Copy cycle and one-cycle; one rectangle per packet; a non-negative,
//! non-empty, in-target pixel extent; point sampling; texcoords that
//! recover integer S10.5 endpoints; in one-cycle, a combiner program
//! reading only `Texel0`/`Primitive`/`Environment`/`One`/`Zero` with every
//! register it reads actually set; and a blender mode with `FORCE_BL` set
//! whose cycles select neither `A = Shade` nor `B = FramebufferAlpha`,
//! with every color register those cycles read actually set. Everything
//! outside that is a named [`TexrectExecutionError`], never an
//! approximation.
//!
//! ## Scope status
//!
//! DONE for the composed `fill + LoadBlock + texrect` shape in **both**
//! Copy and one-cycle, proven end to end into guest RDRAM for both measured
//! WM2000 programs, and DONE for three of the four post-combiner stages
//! (blender, alpha compare, alpha dither + coverage). Two-cycle texrects
//! are **deliberately not ported** (a scope boundary this slice chose, not
//! work this module waits on): zero occurrences in the measured window,
//! and they need the cross-cycle `Combined` carry and a second texel.
//!
//! **RGB dither is deliberately not ported**, and unlike two-cycle it is
//! blocked on evidence rather than on scope: the workspace's two ports
//! disagree about both its table and its arithmetic, and nothing in this
//! repo settles which is the RDP's. Landing it needs that authority call,
//! not more code -- both implementations already exist.
//!
//! ## Open questions
//!
//! **Which RGB dither is the RDP's?** `crate::rgb_dither` (RT64) and
//! `fn64-render-reference`'s `apply_rgb_dither` disagree on the Bayer
//! table at 8 of 16 cells and on the arithmetic at every input where
//! `(channel & 7)` straddles the threshold. Settling it needs hardware or
//! an independent third source; until then this executor runs neither.
//!
//! **What is the RDP's noise sequence?** Both the `Noise` dither modes and
//! `G_AC_DITHER` consume it. This module uses a proven endpoint for the
//! former and refuses the latter, because a gate has no bounded-interval
//! argument the way a `+/-1` perturbation does.
//!
//! `step_axis`'s truncating division is a preserved convention, not a
//! verified silicon tie-break; public documentation does not establish the
//! RDP's rounding for interpolated texture coordinates.
//!
//! `TmemFirstRowParity` is **derived per tile, never frozen** -- see
//! `execute_texture_rectangle`'s own derivation from
//! `tile.size().low_t().integer() & 1` and the comment above it. This
//! paragraph previously claimed `Even` was passed unconditionally; that was
//! true before that fix and is no longer, so the odd-row-parity frontier it
//! named is closed rather than open.

use crate::alpha_compare::{alpha_compare_value, apply_alpha_dither, AlphaCompareNoise};
use crate::blend::{
    blend_fragment, BlendFramebufferSample, BlendImageReadError, BlendModeState, BlendedFragment,
};
use crate::combiner::{
    combiner_inputs_from_fragment_registers, run_one_cycle, run_two_cycle, AlphaInput,
    AlphaInputSlot, ColorInput, ColorInputSlot, CombineParams, CombinerInputs,
    PreparedTwoCycleCombiner,
};
use crate::coverage::{apply_coverage_alpha, coverage_result, Coverage, CoverageModeBits};
use crate::state::{AlphaCompare, AlphaDither, Color4, PrimColor, RgbDither};
use crate::targets::oracle::DeviceColorBytes;
use crate::targets::{
    CandidateColorTarget, ColorTargetFormat, ColorTargetKey, CompletedColorTargetWrite,
    TargetError, TargetRectangle,
};
use crate::tmem::{
    sample_point, PhysicalTexelReadError, PointSampleCoordinates, PointSampleError,
    PointSampleRequest, TextureCoordinateS10_5, TileAddressMode, TileCoordinate, TileDescriptor,
    TileSize, TmemFirstRowParity, TmemWordAddress,
};
use crate::{CycleType, ImageFormat, OtherMode, PixelSize, TextureLutMode};

use fn64_render::RectViewportPixels;
use std::borrow::Cow;

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
    flipped_axes: bool,
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
    first_column: u32,
    column_limit: u32,
    first_row: u32,
    row_limit: u32,
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
fn clip_texrect_extent(
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

/// Why an admitted `TextureRectangle` could not be executed against a color
/// target. Every variant is a loud rejection; none mutates the target.
///
/// Not `Eq`: two variants carry the `f32` texcoord that was refused, and an
/// `Eq` impl over `f32` would have to claim a total equality NaN does not
/// satisfy. `PartialEq` is what the error genuinely supports.
#[derive(Clone, Debug, PartialEq)]
pub enum TexrectExecutionError {
    /// Copy cycle blits the texel; one-cycle runs it through the color
    /// combiner; two-cycle runs both passes through
    /// [`crate::combiner::run_two_cycle`], carry and all. All three are
    /// executed. **Only Fill cycle reaches this variant**, and it does so
    /// because Fill samples no texture at all -- the RDP's fill-colour path,
    /// which this executor does not own.
    ///
    /// The previous text here claimed two-cycle "needs the `Combined` carry
    /// and a second texel, neither of which this executor supplies." That
    /// was wrong about its own crate on the first count -- `combiner.rs`'s
    /// `CyclePass::SecondOfTwoCycles` has modeled the carry since the port
    /// landed -- and wrong about the requirement on the second: `Texel1` is
    /// refused for texrects by [`Self::UnsupportedColorInput`] because a
    /// rectangle binds one tile, which is the reference lane's rule too, and
    /// a two-cycle program that reads only `Texel0` needs no second texel.
    ///
    /// The measurement that used to be cited here ("2,520 texrects, all
    /// one-cycle") is a boot-through-attract window
    /// (`docs/RT64-WM2000-CYCLE-MODES.md` §1). It says two-cycle was not
    /// seen there; it never said two-cycle does not occur, and gameplay has
    /// not been reached on either lane. Read the Fill zero in that same
    /// window the same way.
    ///
    /// ## Why Fill is still refused, and what it would cost to admit
    ///
    /// **Not because the hardware behavior is unknown.** n64brew's RDP
    /// command table states it in the Texture Rectangle section itself:
    /// "In FILL mode this behaves identically to Fill Rectangle, the
    /// texturing properties are ignored." `fn64-render-reference` acts on
    /// exactly that, executing the command as
    /// `draw_fill_rectangle(&rectangle.as_fill_cycle_rectangle(), target)`
    /// (`backend/imp.rs:911-919`), and records that refusing it aborted a
    /// real WCW/nWo Revenge frame -- a shipped AKI-engine sibling of
    /// WM2000. This refusal is a lane gap, and it is the one row of
    /// `docs/RT64-LANE-DIVERGENCES.md` this module could not close in the
    /// pass that closed D2.
    ///
    /// It is **not** a widening of the match above. Admitting `Fill` there
    /// would draw the wrong rectangle, silently, which is the exact failure
    /// mode this whole error type exists to prevent. Three things are
    /// missing, and each is somewhere else:
    ///
    /// 1. **The two lanes' rectangles disagree by a pixel on every axis.**
    ///    A texrect reaches this executor as an already-resolved
    ///    [`crate::RectViewportPixels`], which
    ///    `raw_dpc/texture_rectangle.rs` builds by RT64's `FixedRect` rule:
    ///    round `ulx`/`uly` down in fill mode, then `(coord + 3) >> 2` at
    ///    *both* ends, giving a **half-open** rectangle. A fill rectangle's
    ///    rule is `targets/fill.rs`'s `resolve_fill_pixel_rectangle`:
    ///    `coord >> 2` at both ends, **inclusive** (`width = x1 - x0 + 1`).
    ///    On wire `(0, 0, 1276, 956)` the first gives 319x239 and the
    ///    second 320x240; on wire `ulx = 2` the first rounds and the second
    ///    refuses `FractionalEdge`. Executing the viewport this function's
    ///    caller already holds would draw a full-screen fill one pixel
    ///    short on each axis.
    /// 2. **`FillColor` is not on this path.**
    ///    `raw_dpc::triangle_draw_data::RetrievedTriangleDraw` snapshots
    ///    `blend_color`, `env_color`, `prim_color` and `fog_color` at each
    ///    triangle's own stream position. It does not snapshot the fill
    ///    color, because until now no triangle-sourced command read it.
    ///    A Fill-cycle texrect reads nothing else.
    /// 3. **The fill-cycle blender hazard is a property of the cycle, not
    ///    the command.** The reference checks
    ///    `other_mode.validate_fill_cycle_bypass()` on this command for
    ///    that reason (`backend/validate.rs:152-161`: a retained depth
    ///    consumer in fill cycle can hang the RDP), and
    ///    `targets/fill.rs`'s `require_safe_fill_cycle_bypass` is this
    ///    crate's equivalent. It would have to run here too.
    ///
    /// So the real shape of the fix is: carry the raw wire rectangle (or
    /// the resolved fill rectangle) alongside the viewport, snapshot
    /// `FillColor` on the triangle path, and route the command to
    /// `execute_fill_rectangle` rather than through this executor at all --
    /// which is what the reference does. That is three modules, and it is
    /// deliberately not attempted behind a one-line match arm.
    UnsupportedCycleType {
        cycle_type: CycleType,
    },
    /// A one-cycle program selected a color input this executor does not
    /// evaluate. Named rather than substituted: `Shade` combined against a
    /// zero this executor invented would draw plausible-looking wrong
    /// pixels, which is the failure mode this refusal exists to prevent.
    UnsupportedColorInput {
        slot: ColorInputSlot,
        input: ColorInput,
    },
    /// [`Self::UnsupportedColorInput`]'s alpha counterpart.
    UnsupportedAlphaInput {
        slot: AlphaInputSlot,
        input: AlphaInput,
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
    ///
    /// **Reached only from the raw-triangle executor now.** For a texrect
    /// this variant used to fire whenever any part of the rectangle
    /// overhung the target, on the reasoning that "a clamped rectangle
    /// would write pixels the RDP never covers." That reasoning was
    /// backwards: clipping is precisely what the scissor does. Pinned RT64
    /// intersects the scissor and draw rectangles and keeps a non-empty
    /// intersection rather than rejecting an overhanging primitive
    /// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`),
    /// and fn64's own reference renderer clamps the same way
    /// at `fn64-render-reference/src/raster/draw.rs:197-203`. The texrect
    /// path now clips through [`clip_texrect_extent`] and refuses only the
    /// genuinely empty result, as [`Self::ScissoredAway`].
    OutsideTarget {
        key: ColorTargetKey,
        rectangle: TargetRectangle,
    },
    /// Nothing of the rectangle survived the intersection of its own
    /// extent, the latched scissor, and the colour target.
    ///
    /// This is the *clipped-to-nothing* case, not the *overhangs* case: a
    /// rectangle that merely straddles an edge is drawn, narrowed to the
    /// surviving span. An empty result means the primitive is entirely
    /// outside the admitted region, or the scissor is reversed/degenerate,
    /// and both are worth naming rather than silently writing zero pixels
    /// and reporting success. Carries both rects so the reader can tell
    /// which of the two it is.
    ScissoredAway {
        key: ColorTargetKey,
        rectangle: TargetRectangle,
        scissor: RdpScissorRect,
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
    /// The other-mode word selects a stage mode whose value depends on the
    /// RDP's per-pixel random threshold. **This crate has no authority for
    /// that sequence and refuses rather than inventing one.**
    ///
    /// Two generators exist in the workspace and they are different
    /// sequences, neither claiming to be silicon: `crate::random`'s
    /// `initRand`/`nextRand` is RT64's shader PRNG seeded from
    /// `frameCount` and pixel position, and
    /// `fn64-render-reference`'s is a SplitMix64 policy its own source
    /// calls "deliberately not described as the silicon sequence"
    /// (`raster/mod.rs:85-119`). Picking either here would produce pixels
    /// that agree with one implementation by construction and with the
    /// hardware by accident.
    NoiseThresholdUnavailable {
        stage: TexrectNoiseStage,
    },
    /// The other-mode word selects an ordered dither tile
    /// (`MagicSquare`/`Bayer`), whose threshold this crate's port and
    /// `fn64-render-reference`'s **disagree** about for `Bayer` at
    /// documented cells, pinned rather than resolved by `rgb_dither.rs`'s
    /// `bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`.
    /// For the *RGB* stage the two ports' arithmetic also differs -- RT64
    /// adds the threshold then truncates, the reference bumps to the next
    /// bucket conditionally. Refused by name rather than picking a side no
    /// evidence settles.
    ///
    /// **This applies to the alpha-dither stage too, and that is a fact
    /// about the current tree rather than about the RGB stage.**
    /// `docs/RT64-LANE-DIVERGENCES.md` D7 scored this refusal a wgpu defect
    /// on the ground that the disputed table lived only in the RGB path,
    /// while `alpha_compare.rs` carried a *second* Bayer table
    /// byte-identical to the reference's -- so, the argument went, the
    /// stage being refused already agreed with the reference cell-for-cell.
    /// That was true at the audit's pin `4371d57a`
    /// (`alpha_compare.rs:175-176` held the duplicate), and **`51b4e184`
    /// deleted it.** libultra defines `G_AD_PATTERN`'s threshold as *the
    /// currently selected RGB dither matrix* (`gbi.h:674-678`), so carrying
    /// two tables for one hardware quantity was itself the defect; the
    /// alpha path now reads `crate::rgb_dither::ordered_tile_value`, pinned
    /// at every cell by `rgb_dither.rs`'s
    /// `the_alpha_dither_path_reads_this_modules_tables`.
    ///
    /// The consequence is that the alpha stage is now downstream of the
    /// disputed tile by construction, and its rounding
    /// (`(alpha & 7) > threshold`, `alpha_compare.rs`'s
    /// `apply_alpha_dither`) reads the threshold directly, so the eight
    /// disputed cells are observable in its output. **The refusal is
    /// therefore correct as it stands, and D7's verdict is superseded, not
    /// unimplemented.** It is blocked on D19 -- which Bayer arrangement the
    /// RDP uses -- which the audit itself scores UNKNOWN and which nothing
    /// in this repo settles. Pinned by
    /// `the_alpha_dither_refusal_is_downstream_of_the_one_disputed_tile`.
    OrderedDitherAuthorityUnsettled {
        stage: TexrectNoiseStage,
        pattern: RgbDither,
    },
    /// A coverage-consuming mode needs the **destination** coverage count,
    /// which is 3 bits the RDP splits between the RGBA16 halfword's visible
    /// LSB and a 2-bit hidden sidecar. `fn64-render-wgpu` maintains no such
    /// sidecar (the oracle does, as `RdramHiddenBits`), so only 1 of the 3
    /// bits is recoverable. Refused rather than reconstructed from a third
    /// of its bits.
    DestinationCoverageUnavailable {
        consumer: &'static str,
    },
    /// A blender cycle selects `A = Shade`, which this executor cannot
    /// resolve: it has no vertex-interpolated color to supply, exactly as
    /// its combiner refuses a `Shade`-reading program. Named rather than
    /// combined against a zero this executor invented.
    UnsupportedBlendShadeAlpha,
    /// A blender cycle selects `B = FramebufferAlpha`, the destination
    /// *coverage count*. RGBA16 stores one coverage bit, not the three a
    /// count needs, and this executor runs no coverage stage, so the term
    /// cannot be resolved. Refused rather than approximated from the bit.
    UnsupportedBlendFramebufferAlpha,
    /// `FORCE_BL` is clear **and** `AA_EN` is set, so the reference's
    /// `blend_enabled` reduces to `!wraps`, and `wraps` needs the
    /// destination coverage count this executor does not maintain
    /// (`fn64-render-reference/src/raster/coverage.rs:68-69`). Refused
    /// rather than guessed in either direction, since guessing `true` runs
    /// a blender the RDP bypasses and guessing `false` bypasses one it
    /// runs.
    ///
    /// The other two `FORCE_BL`-clear cases are **not** refused, because
    /// the disjunction settles without `wraps`: with `AA_EN` clear it is
    /// `false` outright, and with `FORCE_BL` set it is `true` outright.
    BlendEnabledNotDerivable,
    /// The blender selected a framebuffer term while `IM_RD` is disabled,
    /// so no destination sample legally exists. Propagated by name from
    /// [`crate::blend::blend_fragment`] rather than substituted with a
    /// zero destination, which would draw a plausible-looking wrong pixel
    /// -- exactly the failure this executor's other refusals exist to
    /// prevent.
    Blend {
        column: u32,
        row: u32,
        source: BlendImageReadError,
    },
    /// **The raw-triangle rasterizer's own row list disagrees with the row
    /// count the decoder declared into the journal.**
    ///
    /// The two walks bound themselves differently and must: the decoder has
    /// no target height (`SetColorImage` carries only a width) so it bounds
    /// by installed RDRAM, while the executor bounds by the real extent. A
    /// disagreement means some declared row would never be rasterized --
    /// and `fill_completed_writes` slices and digests the full-extent buffer
    /// for EVERY declared range without checking the raster touched it, so
    /// that row would reach guest RDRAM holding stale bytes under a
    /// perfectly valid digest.
    ///
    /// Refused loudly rather than clipped. This is the single check that
    /// makes the declared-vs-drawn contract enforced rather than assumed.
    TriangleRowCountDisagreesWithJournal {
        declared: usize,
        rasterized: usize,
    },
    /// The rasterizer's row at `position` covers a different byte range than
    /// the journal declared at that position.
    ///
    /// The count check above catches a walk that stopped early; this catches
    /// one that walked the same number of rows over different geometry.
    /// Comparing ranges rather than trusting equal counts is what turns
    /// "the two walks cannot disagree" from an inference into a check.
    TriangleRowRangeDisagreesWithJournal {
        position: usize,
        declared: (u32, u32),
        rasterized: (u32, u32),
    },
    /// A pixel with non-zero subpixel coverage reported no covered subsample
    /// to evaluate its attribute planes at.
    ///
    /// Unreachable while `pixel_coverage` and `attribute_sample` scan the
    /// same subsamples in the same order, which they do. Named rather than
    /// `expect`ed so a future divergence between the two refuses the
    /// triangle instead of aborting the process -- and refuses rather than
    /// falling back to the pixel centre, which would evaluate the shade
    /// plane at a point the triangle does not cover.
    TriangleAttributeSampleMissing {
        column: u32,
        row: u32,
    },
    /// The wire opcode's texture bit and the caller's TMEM binding disagree.
    ///
    /// Both directions are refused, not just one. A TEXTURED triangle with no
    /// binding would have to combine against a fabricated zero texel -- the
    /// silent-wrong-answer shape this crate has already shipped once. An
    /// UNTEXTURED one with a binding has no S/T/W coefficient block to
    /// evaluate, so there is no coordinate to sample at and any texel
    /// produced would be invented.
    TriangleTextureBindingDisagreesWithOpcode {
        opcode_textured: bool,
        binding_present: bool,
    },
    Target(TargetError),
}

impl core::fmt::Display for TexrectExecutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedCycleType { cycle_type } => write!(
                formatter,
                "execute_texture_rectangle admits Copy cycle (direct blit), OneCycle and \
                 TwoCycle (both combiner-evaluated); got {cycle_type:?}"
            ),
            Self::UnsupportedColorInput { slot, input } => write!(
                formatter,
                "execute_texture_rectangle evaluates only TEXEL0/PRIMITIVE/ENVIRONMENT/ONE/ZERO \
                 color inputs (plus COMBINED in a two-cycle program's second cycle); slot \
                 {slot:?} selects {input:?}"
            ),
            Self::UnsupportedAlphaInput { slot, input } => write!(
                formatter,
                "execute_texture_rectangle evaluates only TEXEL0/PRIMITIVE/ENVIRONMENT/ONE/ZERO \
                 alpha inputs (plus COMBINED in a two-cycle program's second cycle); slot \
                 {slot:?} selects {input:?}"
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
            Self::ScissoredAway {
                key,
                rectangle,
                scissor,
            } => write!(
                formatter,
                "texture rectangle {rectangle:?} has no pixel surviving the scissor {scissor:?} \
                 and color target {key:?}"
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
            Self::NoiseThresholdUnavailable { stage } => write!(
                formatter,
                "{stage} selects a noise-thresholded mode, and execute_texture_rectangle has no \
                 authority for the RDP's per-pixel random sequence; the two generators in this \
                 workspace are different sequences and neither claims to be silicon"
            ),
            Self::OrderedDitherAuthorityUnsettled { stage, pattern } => write!(
                formatter,
                "{stage} selects the ordered {pattern:?} dither tile, whose threshold and \
                 arithmetic this crate's RT64 and reference ports disagree about; no evidence \
                 in this repo settles which is the RDP's"
            ),
            Self::DestinationCoverageUnavailable { consumer } => write!(
                formatter,
                "{consumer} needs the destination coverage count, which is 3 bits split between \
                 RGBA16's visible LSB and a 2-bit hidden sidecar fn64-render-wgpu does not \
                 maintain; only 1 of the 3 bits is recoverable"
            ),
            Self::TriangleRowCountDisagreesWithJournal {
                declared,
                rasterized,
            } => write!(
                formatter,
                "the raw triangle's journal declares {declared} scanline write(s) but the \
                 rasterizer covers {rasterized}; a declared row the raster never visits would \
                 be digested from stale resident bytes"
            ),
            Self::TriangleRowRangeDisagreesWithJournal {
                position,
                declared,
                rasterized,
            } => write!(
                formatter,
                "the raw triangle's scanline #{position} is declared at \
                 {declared:?} but rasterizes {rasterized:?} (start, len)"
            ),
            Self::TriangleAttributeSampleMissing { column, row } => write!(
                formatter,
                "raw-triangle pixel ({column}, {row}) has subpixel coverage but no covered \
                 subsample to evaluate its attribute planes at"
            ),
            Self::TriangleTextureBindingDisagreesWithOpcode {
                opcode_textured,
                binding_present,
            } => write!(
                formatter,
                "the raw triangle's wire opcode says textured={opcode_textured} but its TMEM \
                 binding is present={binding_present}; a textured triangle needs a bound tile \
                 and an untextured one has no S/T/W planes to sample with"
            ),
            Self::UnsupportedBlendShadeAlpha => formatter.write_str(
                "the blender cycle selects A = Shade, and execute_texture_rectangle has no \
                 vertex-interpolated shade alpha to resolve it with",
            ),
            Self::UnsupportedBlendFramebufferAlpha => formatter.write_str(
                "the blender cycle selects B = FramebufferAlpha (destination coverage count), \
                 and execute_texture_rectangle maintains no coverage count; RGBA16 stores one \
                 coverage bit, not the three a count needs",
            ),
            Self::BlendEnabledNotDerivable => formatter.write_str(
                "FORCE_BL is clear and AA_EN is set, so blend_enabled reduces to !wraps, which \
                 needs the destination coverage count execute_texture_rectangle does not \
                 maintain",
            ),
            Self::Blend {
                column,
                row,
                source,
            } => write!(
                formatter,
                "texture rectangle blender refused at pixel ({column}, {row}): {source}"
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

/// The one-cycle shading state a texrect's fragments are combined with:
/// the `SetCombine` program current at the texrect's own stream position,
/// plus the two constant color registers the measured programs read.
///
/// Constructed by [`Self::try_new`], which refuses -- by name, before any
/// pixel is written -- every combiner selector this executor does not
/// evaluate.
///
/// `Primitive` and `Environment` are **not** `Option`: they name RDP
/// registers, which always hold a value (zero until the guest writes one).
/// `fn64-render-reference` models the constant color registers as
/// zero-initialized `[u8; 4]` (`gbi/state.rs:227`, `:387`) and RT64's own
/// C++ zero-initializes `primColor`/`envColor` at
/// `src/hle/rt64_state.cpp:126-129`. The refusal this replaced invented an
/// "unset" state the hardware has no way to be in.
///
/// This is a different question from the selector refusals above, which
/// stay: a register's power-on zero is its **real content**, whereas
/// `Shade`/`Noise`/`K4`/`K5` have no register behind them at all and would
/// combine against a value this executor made up.
///
/// Not a `CombinerInputs` itself: that struct is per-pixel (its `tex_val0`
/// changes on every texel), whereas this is the per-rectangle half. The
/// per-pixel half is assembled inside the sampling loop from this plus the
/// sampled texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectShading {
    combine: CombineParams,
    env_color: Color4,
    prim_color: PrimColor,
}

/// Every color selector this executor evaluates.
///
/// The first five were measured against WM2000's entire
/// boot-through-attract window (`docs/RT64-WM2000-CYCLE-MODES.md` §2): its
/// 2,520 texrects run exactly two programs, and between them they read only
/// those. Everything else is refused by name so a future title gets a loud
/// error instead of wrong pixels -- `Shade` in particular, which this
/// executor has no vertex-interpolated color to supply and would otherwise
/// silently combine against zero.
///
/// **The last four are admitted on a different ground, and the distinction
/// is the whole point of this comment.** They are not measured in WM2000's
/// window; they are admitted because each resolves to a component of a
/// value this executor *already sources from a real wire register*, so
/// evaluating them invents nothing:
///
/// - `Texel0Alpha` is `texel0[3]`, the alpha of the very texel the sampling
///   loop decodes for `Texel0` (`combiner.rs`'s `resolve_color_input`).
/// - `PrimitiveAlpha` is `prim_color[3]` and `EnvAlpha` is `env_color[3]`.
///   Both registers are already required to be set -- a program reading
///   them with no wire command staged is
///   [`TexrectExecutionError::UnsetConstantRegister`], not a black default.
/// - `PrimLodFrac` is `PrimColor::lod().lod_frac_normalized()`, wired into
///   `CombinerInputs` by `combiner_inputs_from_fragment_registers` from the
///   same `SetPrimColor` word that supplies `Primitive`.
///
/// `docs/RT64-LANE-DIVERGENCES.md` D4 lists twelve selectors this executor
/// refused while `crate::combiner` implements all of them, and scores the
/// gap reference-correct. Four is the subset that is a *wiring* gap. The
/// rest stay refused, and for a reason the audit's framing does not
/// separate out: `crate::combiner` implementing a selector means it can
/// read the corresponding `CombinerInputs` field, not that this executor
/// can fill it. `LodFraction`, `Noise`, `K4`, `K5`, `KeyCenter` and
/// `KeyScale` all read fields [`TexrectShading::base_inputs`] leaves at
/// zero -- there is no `SetConvert`/`SetKey` plumbing, no LOD stage, and no
/// noise authority (the same one [`TexrectNoiseStage`] refuses by name).
/// Admitting them would combine against an invented zero, which is exactly
/// the failure the `Shade` refusal exists to prevent. `Texel1` and
/// `Texel1Alpha` stay refused because a rectangle binds one tile, which is
/// the reference's own reason (`backend/validate.rs:479-483`).
const ADMITTED_COLOR_INPUTS: [ColorInput; 9] = [
    ColorInput::Texel0,
    ColorInput::Primitive,
    ColorInput::Environment,
    ColorInput::One,
    ColorInput::Zero,
    ColorInput::Texel0Alpha,
    ColorInput::PrimitiveAlpha,
    ColorInput::EnvAlpha,
    ColorInput::PrimLodFrac,
];

/// [`ADMITTED_COLOR_INPUTS`]' alpha counterpart, same measurement and same
/// rationale -- including the register-backed widening, which for the alpha
/// selectors adds only `PrimLodFrac`. The alpha enum has no `*Alpha`
/// variants: an alpha selector already resolves to a scalar, so
/// `AlphaInput::Primitive` *is* `prim_color[3]`.
const ADMITTED_ALPHA_INPUTS: [AlphaInput; 6] = [
    AlphaInput::Texel0,
    AlphaInput::Primitive,
    AlphaInput::Environment,
    AlphaInput::One,
    AlphaInput::Zero,
    AlphaInput::PrimLodFrac,
];

/// Which combiner bitfield slices a program's cycle mode actually
/// evaluates, and therefore which ones must be validated.
///
/// Validating a slice that never runs would refuse programs the RDP
/// executes; skipping one that does run would admit a program and then
/// evaluate selectors nothing checked. The mapping is RT64's own: one-cycle
/// mode reads the *second*-cycle slice (`run`:
/// `runCycle(inputs, twoCycle ? 0 : 1, twoCycle, ...)`), so the first slice
/// of a one-cycle program is dead bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombinerProgramCycles {
    /// One-cycle mode: only the second-cycle slice runs.
    OnlySecondSlice,
    /// Two-cycle mode: the first slice runs, then the second over its
    /// result.
    BothSlices,
}

impl CombinerProgramCycles {
    /// The slices this mode evaluates, in evaluation order.
    fn evaluated_slices(self) -> &'static [CombinerProgramSlice] {
        match self {
            Self::OnlySecondSlice => &[CombinerProgramSlice::OnlyCycleOfOneCycleMode],
            Self::BothSlices => &[
                CombinerProgramSlice::FirstOfTwoCycles,
                CombinerProgramSlice::SecondOfTwoCycles,
            ],
        }
    }
}

/// One evaluated pass, named the same way
/// [`crate::combiner`]'s own private `CyclePass` is -- this is the
/// admission-side mirror of that evaluation-side enum, and the two must
/// agree on which bitfield slice each pass reads or the gate would check a
/// program the combiner never runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CombinerProgramSlice {
    OnlyCycleOfOneCycleMode,
    FirstOfTwoCycles,
    SecondOfTwoCycles,
}

impl CombinerProgramSlice {
    /// `decodeColorInput`/`decodeAlphaInput`'s `secondCycleInputs` selector,
    /// identical to `combiner::CyclePass::bitfield_second_cycle`.
    const fn reads_second_bitfield_slice(self) -> bool {
        !matches!(self, Self::FirstOfTwoCycles)
    }

    /// Whether `Combined`/`CombinedAlpha` names a value this executor can
    /// resolve in this pass.
    ///
    /// **True everywhere except a two-cycle program's FIRST pass.** The
    /// authority is RT64's pinned `5473732a`,
    /// `src/shared/rt64_color_combiner.h`:
    ///
    /// - `fromColorInput`'s `case C_COMBINED: return combinerColor.rgb;`
    ///   (lines 470-471) and `fromAlphaInput`'s `case A_COMBINED: return
    ///   combinerAlpha;` (lines 517-518) are **unconditional**. RT64 has no
    ///   refusal, no cycle guard and no special case for the selector.
    /// - `run()` (line 611) zero-initializes `combinerColor = float4(0, 0,
    ///   0, 0)` (line 612) and then, for a one-cycle program
    ///   (`twoCycle == false`), makes exactly one
    ///   `runCycle(inputs, 1, false, combinerColor)` call (line 620). So a
    ///   one-cycle program's `COMBINED` reads that zero.
    /// - The input wrap that turns the accumulator into a genuine *carry*
    ///   is gated on
    ///   `const bool secondCycle = twoCycle && secondCycleInputs` (line
    ///   577) and runs only in lines 580-601. A one-cycle program skips it,
    ///   leaving the zero untouched.
    ///
    /// So `COMBINED` in one-cycle mode is **defined behaviour reading a
    /// hardware zero**, not undefined behaviour and not a value this
    /// executor invents. [`crate::combiner::run_one_cycle`] has always
    /// evaluated it that way (`combiner.rs`'s `combiner_color_in =
    /// [0.0; 3]` / `combiner_alpha_in = 0.0`, citing the same RT64
    /// zero-init), and `two_cycle_carries_the_accumulator_one_cycle_cannot`
    /// pins the arithmetic. This predicate was the only thing refusing a
    /// program the evaluator behind it already handled correctly.
    ///
    /// Measured on the real WM2000 ROM on the all-Rust stack: a texrect
    /// latches `combine` = `low 0xfc15fea3` / `high 0xf00ff23f` and runs it
    /// in ONE-cycle mode. `parseColorInputB`'s second-cycle field is
    /// `(high >> 24) & 0xF` = `0`, and selector `0` is `C_COMBINED`
    /// (`rt64_color_combiner.h:23`). The run aborted at 1,887 VI swaps on
    /// this predicate.
    ///
    /// **`FirstOfTwoCycles` stays refused**, and that is not conservatism:
    /// this executor's two-cycle arithmetic for a `COMBINED` read in cycle
    /// 0 is covered by no measurement in this repo.
    const fn resolves_the_combined_selector(self) -> bool {
        !matches!(self, Self::FirstOfTwoCycles)
    }

    /// `shade_available` is `true` when the caller can supply the shade the
    /// RDP's edge walker would have produced. That is a SHADED raw triangle,
    /// which interpolates it from the triangle's own shade coefficient
    /// planes -- and **also every texture rectangle**, whose shade the
    /// hardware defines as zero (derived in [`TexrectShading::base_inputs`]).
    /// It is `false` for an UNSHADED raw triangle, where the coefficient
    /// planes carry a real interpolated value this executor does not have,
    /// and reading `base_inputs`' zeroed field would be the silent
    /// substitution this admission exists to prevent.
    fn admits_color(self, input: ColorInput, shade_available: bool, texel_available: bool) -> bool {
        if matches!(input, ColorInput::Combined | ColorInput::CombinedAlpha) {
            return self.resolves_the_combined_selector();
        }
        if shade_available && matches!(input, ColorInput::Shade | ColorInput::ShadeAlpha) {
            return true;
        }
        // An UNTEXTURED raw triangle has no texel, so a program selecting
        // Texel0 would combine against a fabricated zero -- the exact
        // substitution every other refusal here exists to prevent. Texrects
        // always sample a texel and pass `true`, so their admission is
        // unchanged.
        if !texel_available && matches!(input, ColorInput::Texel0 | ColorInput::Texel0Alpha) {
            return false;
        }
        ADMITTED_COLOR_INPUTS
            .iter()
            .any(|admitted| core::mem::discriminant(admitted) == core::mem::discriminant(&input))
    }

    fn admits_alpha(self, input: AlphaInput, shade_available: bool, texel_available: bool) -> bool {
        if matches!(input, AlphaInput::Combined) {
            return self.resolves_the_combined_selector();
        }
        if shade_available && matches!(input, AlphaInput::Shade) {
            return true;
        }
        if !texel_available && matches!(input, AlphaInput::Texel0) {
            return false;
        }
        ADMITTED_ALPHA_INPUTS
            .iter()
            .any(|admitted| core::mem::discriminant(admitted) == core::mem::discriminant(&input))
    }
}

impl TexrectShading {
    /// Validates that `combine`'s one-cycle program reads only selectors
    /// this executor evaluates, and that every constant register it does
    /// read is actually set.
    ///
    /// `second_cycle = true` throughout, matching [`run_one_cycle`]'s own
    /// hardcoded `SECOND_CYCLE` constant: RT64's one-cycle mode reads the
    /// *second-cycle* bitfield slice, so validating the first-cycle slice
    /// would check a program that never runs. This function and
    /// `run_one_cycle` must agree on which slice they read or the gate
    /// would admit one program and evaluate another.
    pub fn new(combine: CombineParams, env_color: Color4, prim_color: PrimColor) -> Self {
        Self {
            combine,
            env_color,
            prim_color,
        }
    }

    /// Validates that `combine`'s one-cycle program reads only selectors
    /// this executor evaluates, and that every constant register it does
    /// read is actually set.
    ///
    /// Thin alias for [`Self::validate_combiner_program`] at
    /// [`CombinerProgramCycles::OnlySecondSlice`], kept because one-cycle is
    /// the mode every existing caller and fixture names.
    pub fn validate_one_cycle(self) -> Result<Self, TexrectExecutionError> {
        self.validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
    }

    /// Validates every bitfield slice `cycles` says will actually be
    /// evaluated, and that every constant register any of them reads is set.
    ///
    /// Called by the executor **only when a combiner runs**. Copy cycle
    /// consults no combiner program on real hardware, so gating a Copy
    /// rectangle on the program that happens to be latched would refuse
    /// rectangles the RDP draws -- measured, not reasoned: the existing
    /// composed Copy fixture latches `SetCombine(0, 0)`, whose slot A
    /// decodes to `COMBINED`, and validating it unconditionally refused a
    /// packet that had executed correctly for the whole life of the Copy
    /// path.
    ///
    /// `Combined`/`CombinedAlpha` is admitted **only in the second slice of
    /// a two-cycle program**, which is the one place a first-cycle result
    /// exists to carry. That is the reference lane's own rule
    /// (`fn64-render-reference/src/backend/validate.rs:476-478`: "selects
    /// COMBINED before a first-cycle result exists"), and it is why the
    /// admitted set is a function of the slice rather than a constant.
    pub fn validate_combiner_program(
        self,
        cycles: CombinerProgramCycles,
    ) -> Result<Self, TexrectExecutionError> {
        // A texture rectangle's shade is **architecturally zero**, not
        // absent, so `Shade`/`ShadeAlpha` is admitted and evaluates against
        // `base_inputs`' `shade_color: [0.0; 4]`. That is a hardware value
        // with a citation, not a substituted placeholder -- see the
        // `shade_color` field's own comment in [`Self::base_inputs`] for the
        // derivation. Texrects always sample a texel, so `Texel0` stays
        // admitted for the same reason it always was.
        self.validate_combiner_program_for(cycles, true, true)
    }

    /// [`Self::validate_combiner_program`] for a raw triangle, told whether
    /// this triangle carries a shade plane and whether it carries a texture.
    pub fn validate_combiner_program_with_shade(
        self,
        cycles: CombinerProgramCycles,
        shade_available: bool,
    ) -> Result<Self, TexrectExecutionError> {
        self.validate_combiner_program_for(cycles, shade_available, false)
    }

    /// [`Self::validate_combiner_program`], but told which per-fragment
    /// inputs the caller can actually supply.
    ///
    /// `shade_available` is `true` only for a SHADED raw triangle, which
    /// interpolates the value from its own shade coefficient planes.
    /// `texel_available` is `true` for every texrect (which always samples
    /// one) and `false` for the untextured raw triangles this backend
    /// currently admits -- so a program reading `Texel0` on one is refused
    /// rather than combined against a fabricated zero.
    ///
    /// Both flags are facts about the primitive, not policy.
    pub fn validate_combiner_program_for(
        self,
        cycles: CombinerProgramCycles,
        shade_available: bool,
        texel_available: bool,
    ) -> Result<Self, TexrectExecutionError> {
        let Self {
            combine,
            env_color,
            prim_color,
        } = self;
        let mut reads_env = false;
        let mut reads_prim = false;
        for slice in cycles.evaluated_slices() {
            let second_cycle = slice.reads_second_bitfield_slice();
            for slot in [
                ColorInputSlot::A,
                ColorInputSlot::B,
                ColorInputSlot::C,
                ColorInputSlot::D,
            ] {
                let input = combine.decode_color(slot, second_cycle);
                if !slice.admits_color(input, shade_available, texel_available) {
                    return Err(TexrectExecutionError::UnsupportedColorInput { slot, input });
                }
                // **Every selector that reads the register, not only the
                // one named after it.** `EnvAlpha` is `env_color[3]`, and
                // `PrimitiveAlpha`/`PrimLodFrac` are `prim_color[3]` and
                // `PrimColor::lod()` -- all three come from the same wire
                // word as the plain variant. Matching only the plain
                // variant would let a program reading `EnvAlpha` with no
                // `SetEnvColor` staged fall through to `base_inputs`'
                // `unwrap_or(Color4::from_wire(0))` and silently combine
                // against a black default, which is the exact substitution
                // `UnsetConstantRegister` exists to prevent.
                reads_env |= matches!(input, ColorInput::Environment | ColorInput::EnvAlpha);
                reads_prim |= matches!(
                    input,
                    ColorInput::Primitive | ColorInput::PrimitiveAlpha | ColorInput::PrimLodFrac
                );
            }
            for slot in [
                AlphaInputSlot::A,
                AlphaInputSlot::B,
                AlphaInputSlot::C,
                AlphaInputSlot::D,
            ] {
                let input = combine.decode_alpha(slot, second_cycle);
                if !slice.admits_alpha(input, shade_available, texel_available) {
                    return Err(TexrectExecutionError::UnsupportedAlphaInput { slot, input });
                }
                reads_env |= matches!(input, AlphaInput::Environment);
                reads_prim |= matches!(input, AlphaInput::Primitive | AlphaInput::PrimLodFrac);
            }
        }
        // **Diagnostic-only census, over exactly the slices just walked.**
        // Placed inside this function, sharing its `evaluated_slices()`
        // walk, so the tally cannot disagree with the admission gate about
        // WHICH bitfield slice runs -- a census of the other slice would
        // report selectors the hardware never consults, which is the exact
        // silent-wrong-answer shape this probe exists to rule out.
        // Only programs that pass admission are counted: a refused one
        // never draws a pixel.
        if crate::combiner::census::enabled() {
            crate::combiner::census::note_wire(combine.low(), combine.high());
            for slice in cycles.evaluated_slices() {
                let second_cycle = slice.reads_second_bitfield_slice();
                crate::combiner::census::note_program(
                    [
                        combine.decode_color(ColorInputSlot::A, second_cycle),
                        combine.decode_color(ColorInputSlot::B, second_cycle),
                        combine.decode_color(ColorInputSlot::C, second_cycle),
                        combine.decode_color(ColorInputSlot::D, second_cycle),
                    ],
                    [
                        combine.decode_alpha(AlphaInputSlot::A, second_cycle),
                        combine.decode_alpha(AlphaInputSlot::B, second_cycle),
                        combine.decode_alpha(AlphaInputSlot::C, second_cycle),
                        combine.decode_alpha(AlphaInputSlot::D, second_cycle),
                    ],
                    texel_available,
                    match slice {
                        CombinerProgramSlice::OnlyCycleOfOneCycleMode => {
                            crate::combiner::census::Pass::OneCycleOnly
                        }
                        CombinerProgramSlice::FirstOfTwoCycles => {
                            crate::combiner::census::Pass::TwoCycleFirst
                        }
                        CombinerProgramSlice::SecondOfTwoCycles => {
                            crate::combiner::census::Pass::TwoCycleSecond
                        }
                    },
                );
            }
        }
        // No refusal for a never-written `SetEnvColor`/`SetPrimColor`: both
        // are RDP registers holding their power-on zero until the guest
        // writes them (see this type's own doc). `reads_env`/`reads_prim`
        // are still computed above because the *selector* admission checks
        // below them depend on the same walk, and because a future consumer
        // that genuinely cannot supply a register needs the same tracking.
        let _ = (reads_env, reads_prim);
        Ok(Self {
            combine,
            env_color,
            prim_color,
        })
    }

    pub const fn combine(self) -> CombineParams {
        self.combine
    }

    /// The per-rectangle half of [`CombinerInputs`], with `tex_val0` still
    /// zeroed -- the sampling loop overwrites it per texel.
    ///
    /// Built through [`combiner_inputs_from_fragment_registers`], the
    /// crate's existing `RasterPS.hlsl` transcription, rather than by
    /// assigning `env_color`/`prim_color` here: routing both the triangle
    /// pipeline and this executor through one assembly is what makes them
    /// incapable of disagreeing about the normalization
    /// ([`Color4::normalized`]'s `/ 255.0`) or about `prim_lod_frac`.
    ///
    /// There is no "unset case" to substitute for any more: both registers
    /// carry their real contents, which are zero until the guest writes
    /// them (see this type's own doc). A program reading `Environment`
    /// before any `SetEnvColor` therefore combines against the register's
    /// actual power-on value rather than aborting.
    ///
    /// # `shade_color` is fn64's zero, not a placeholder
    ///
    /// A `G_TEXRECT` command carries no shade coefficient words. fn64 reads
    /// that wire layout as requiring a zero shade for the synthesized
    /// rectangle primitive, rather than retaining a previous triangle's
    /// shade, so `Shade` and `ShadeAlpha` are admitted and read zero here.
    ///
    /// **Not independently confirmed against an allowed hardware reference.**
    /// Treat the zero-shade rule as fn64's own reading until an allowed source
    /// or differential experiment settles it.
    ///
    /// This is why the refusal that used to stand here was a wiring gap
    /// rather than a guard: the executor already held the right number and
    /// declined to let the combiner read it. Contrast an UNSHADED raw
    /// triangle, where the hardware **does** interpolate a real non-zero
    /// shade this executor cannot reconstruct -- that refusal stays.
    pub(super) fn base_inputs(self) -> CombinerInputs {
        combiner_inputs_from_fragment_registers(
            CombinerInputs {
                tex_val0: [0.0; 4],
                tex_val1: [0.0; 4],
                prim_color: [0.0; 4],
                shade_color: [0.0; 4],
                env_color: [0.0; 4],
                key_center: [0.0; 3],
                key_scale: [0.0; 3],
                lod_fraction: 0.0,
                prim_lod_frac: 0.0,
                noise: 0.0,
                k4: 0.0,
                k5: 0.0,
            },
            self.env_color,
            self.prim_color,
        )
    }
}

/// Which constant color register a [`TexrectExecutionError::UnsetConstantRegister`]
/// names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexrectConstantRegister {
    Primitive,
    Environment,
    /// `SetBlendColor`, read by the blender's `P`/`M = Blend` selector --
    /// never by the combiner.
    Blend,
    /// `SetFogColor`, read by the blender's `P`/`M = Fog` and `A = Fog`
    /// selectors -- never by the combiner.
    Fog,
}

impl core::fmt::Display for TexrectConstantRegister {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Primitive => formatter.write_str("G_SETPRIMCOLOR"),
            Self::Environment => formatter.write_str("G_SETENVCOLOR"),
            Self::Blend => formatter.write_str("G_SETBLENDCOLOR"),
            Self::Fog => formatter.write_str("G_SETFOGCOLOR"),
        }
    }
}

/// The census's sustained rank-1 program and its complete CI4/RGBA16
/// sampling identity. Admission is deliberately literal: changing any one
/// field routes the draw through [`sample_point`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RankOneCi4Rgba16;

fn rank_one_ci4_rgba16_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("FN64_TEXRECT_RANK_ONE_SPECIALIZATION")
            .is_none_or(|value| value != "0")
    })
}

#[cfg(test)]
thread_local! {
    static FORCE_GENERIC_RANK_ONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn with_generic_rank_one_for_test<R>(run: impl FnOnce() -> R) -> R {
    FORCE_GENERIC_RANK_ONE.with(|forced| {
        struct Reset<'a> {
            forced: &'a std::cell::Cell<bool>,
            previous: bool,
        }
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.forced.set(self.previous);
            }
        }
        let previous = forced.replace(true);
        let _reset = Reset { forced, previous };
        run()
    })
}

impl RankOneCi4Rgba16 {
    const COMBINE_LOW: u32 = 0xfcff_ffff;
    const COMBINE_HIGH: u32 = 0xfffd_f6fb;
    const OTHER_MODE_HIGH: u32 = 0x0000_acef;
    const OTHER_MODE_LOW: u32 = 0x0050_41c8;

    fn admit(
        combine: CombineParams,
        other_mode: OtherMode,
        target_format: ColorTargetFormat,
        lut_mode: TextureLutMode,
        tile: TexrectTileBinding,
        draw: TexrectDraw,
    ) -> Option<Self> {
        #[cfg(test)]
        if FORCE_GENERIC_RANK_ONE.with(std::cell::Cell::get) {
            return None;
        }
        let descriptor = tile.descriptor();
        let size = tile.size();
        (rank_one_ci4_rgba16_enabled()
            && combine.low() == Self::COMBINE_LOW
            && combine.high() == Self::COMBINE_HIGH
            && other_mode.high() == Self::OTHER_MODE_HIGH
            && other_mode.low() == Self::OTHER_MODE_LOW
            && target_format == ColorTargetFormat::Rgba16
            && lut_mode == TextureLutMode::Rgba16
            && descriptor.format() == ImageFormat::ColorIndex
            && descriptor.size() == PixelSize::Bits4
            && descriptor.line_words() == 1
            && descriptor.tmem().get() == 0
            && descriptor.palette() == 0
            && !descriptor.s_mode().mirror()
            && !descriptor.s_mode().clamp()
            && descriptor.mask_s() == 4
            && descriptor.shift_s() == 0
            && !descriptor.t_mode().mirror()
            && !descriptor.t_mode().clamp()
            && descriptor.mask_t() == 4
            && descriptor.shift_t() == 0
            && size.low_s().raw() == 0
            && size.low_t().raw() == 0
            && size.high_s().raw() == 60
            && size.high_t().raw() == 60
            && !draw.flipped_axes)
            .then_some(Self)
    }

    fn sample<S: crate::TmemByteSource + ?Sized>(
        self,
        tmem: &S,
        s: i16,
        t: i16,
    ) -> Result<[u8; 4], PointSampleError> {
        let column = (i64::from(s).div_euclid(32) & 15) as u16;
        let row = (i64::from(t).div_euclid(32) & 15) as u16;
        let linear = row * 8 + column / 2;
        let source_address = if row & 1 == 0 { linear } else { linear ^ 4 };
        let packed = tmem.valid_byte(source_address).ok_or_else(|| {
            PointSampleError::from(PhysicalTexelReadError::InvalidTexelByte {
                address: source_address,
            })
        })?;
        let index = if column & 1 == 0 {
            packed >> 4
        } else {
            packed & 0x0f
        };
        let palette_address = 0x0800 + u16::from(index) * 8;
        let high = tmem.valid_byte(palette_address).ok_or_else(|| {
            PointSampleError::from(PhysicalTexelReadError::InvalidTexelByte {
                address: palette_address,
            })
        })?;
        let low_address = palette_address + 1;
        let low = tmem.valid_byte(low_address).ok_or_else(|| {
            PointSampleError::from(PhysicalTexelReadError::InvalidTexelByte {
                address: low_address,
            })
        })?;
        let packed = u16::from_be_bytes([high, low]);
        let expand = |five: u16| ((five << 3) | (five >> 2)) as u8;
        Ok([
            expand((packed >> 11) & 0x1f),
            expand((packed >> 6) & 0x1f),
            expand((packed >> 1) & 0x1f),
            if packed & 1 == 0 { 0 } else { 0xff },
        ])
    }
}

#[cfg(test)]
mod rank_one_ci4_rgba16_tests {
    use super::*;
    use fn64_render::{
        NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode, NeutralTileDescriptor,
        NeutralTileSize,
    };
    use fn64_render_ir::PhysicalMemoryLayout;
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    use crate::targets::{ColorTargetExtent, ColorTargetRegistry};

    #[derive(Clone, Copy)]
    struct AdmissionInputs {
        combine: CombineParams,
        other_mode: OtherMode,
        target_format: ColorTargetFormat,
        lut_mode: TextureLutMode,
        descriptor: NeutralTileDescriptor,
        size: NeutralTileSize,
        draw: TexrectDraw,
    }

    impl AdmissionInputs {
        fn tile(self) -> TexrectTileBinding {
            TexrectTileBinding::try_from_neutral(self.descriptor, self.size).unwrap()
        }

        fn admit(self) -> Option<RankOneCi4Rgba16> {
            RankOneCi4Rgba16::admit(
                self.combine,
                self.other_mode,
                self.target_format,
                self.lut_mode,
                self.tile(),
                self.draw,
            )
        }
    }

    fn exact_inputs() -> AdmissionInputs {
        AdmissionInputs {
            combine: CombineParams::from_wire(
                RankOneCi4Rgba16::COMBINE_LOW,
                RankOneCi4Rgba16::COMBINE_HIGH,
            ),
            other_mode: OtherMode::from_wire(
                RankOneCi4Rgba16::OTHER_MODE_HIGH,
                RankOneCi4Rgba16::OTHER_MODE_LOW,
            ),
            target_format: ColorTargetFormat::Rgba16,
            lut_mode: TextureLutMode::Rgba16,
            descriptor: NeutralTileDescriptor {
                format: NeutralImageFormat::ColorIndex,
                size: NeutralPixelSize::Bits4,
                line_words: 1,
                tmem_word_address: 0,
                palette: 0,
                s_mode: NeutralTileAddressMode::default(),
                mask_s: 4,
                shift_s: 0,
                t_mode: NeutralTileAddressMode::default(),
                mask_t: 4,
                shift_t: 0,
            },
            size: NeutralTileSize {
                low_s: 0,
                low_t: 0,
                high_s: 60,
                high_t: 60,
            },
            draw: TexrectDraw {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
                s_start: 0,
                t_start: 0,
                s_end: 2048,
                t_end: 2048,
                flipped_axes: false,
            },
        }
    }

    #[test]
    fn admission_is_closed_over_every_census_field_the_sampler_depends_on() {
        let exact = exact_inputs();
        assert_eq!(exact.admit(), Some(RankOneCi4Rgba16));
        let mut mutations = Vec::new();

        let mut input = exact;
        input.combine = CombineParams::from_wire(0, RankOneCi4Rgba16::COMBINE_HIGH);
        mutations.push(input);
        let mut input = exact;
        input.combine = CombineParams::from_wire(RankOneCi4Rgba16::COMBINE_LOW, 0);
        mutations.push(input);
        let mut input = exact;
        input.other_mode = OtherMode::from_wire(0, RankOneCi4Rgba16::OTHER_MODE_LOW);
        mutations.push(input);
        let mut input = exact;
        input.other_mode = OtherMode::from_wire(RankOneCi4Rgba16::OTHER_MODE_HIGH, 0);
        mutations.push(input);
        let mut input = exact;
        input.target_format = ColorTargetFormat::Rgba32;
        mutations.push(input);
        let mut input = exact;
        input.lut_mode = TextureLutMode::Ia16;
        mutations.push(input);

        let mut input = exact;
        input.descriptor.format = NeutralImageFormat::IntensityAlpha;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.size = NeutralPixelSize::Bits8;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.line_words = 2;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.tmem_word_address = 1;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.palette = 1;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.s_mode.mirror = true;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.s_mode.clamp = true;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.mask_s = 3;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.shift_s = 1;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.t_mode.mirror = true;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.t_mode.clamp = true;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.mask_t = 3;
        mutations.push(input);
        let mut input = exact;
        input.descriptor.shift_t = 1;
        mutations.push(input);

        let mut input = exact;
        input.size.low_s = 4;
        mutations.push(input);
        let mut input = exact;
        input.size.low_t = 4;
        mutations.push(input);
        let mut input = exact;
        input.size.high_s = 56;
        mutations.push(input);
        let mut input = exact;
        input.size.high_t = 56;
        mutations.push(input);
        let mut input = exact;
        input.draw = input.draw.with_flipped_axes();
        mutations.push(input);

        assert_eq!(mutations.len(), 24);
        for (index, mutation) in mutations.into_iter().enumerate() {
            assert_eq!(mutation.admit(), None, "mutation {index} escaped admission");
        }
    }

    struct CorpusTmem {
        bytes: [u8; 4096],
        valid: [bool; 4096],
        snapshot: crate::TmemSnapshotIdentity,
    }

    impl CorpusTmem {
        fn complete() -> Self {
            let mut bytes = [0; 4096];
            for (address, byte) in bytes.iter_mut().enumerate() {
                *byte = (address as u8)
                    .wrapping_mul(73)
                    .wrapping_add((address >> 4) as u8)
                    .wrapping_add(19);
            }
            let physical = crate::PhysicalTmemState::try_new().unwrap();
            Self {
                bytes,
                valid: [true; 4096],
                snapshot: crate::TmemByteSource::snapshot(&physical),
            }
        }
    }

    impl crate::TmemByteSource for CorpusTmem {
        fn snapshot(&self) -> crate::TmemSnapshotIdentity {
            self.snapshot
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            let index = usize::from(address);
            self.valid[index].then_some(self.bytes[index])
        }
    }

    fn generic_sample(
        tmem: &CorpusTmem,
        tile: TexrectTileBinding,
        s: i16,
        t: i16,
    ) -> Result<[u8; 4], PointSampleError> {
        sample_point(
            tmem,
            tile.descriptor(),
            tile.size(),
            PointSampleRequest::new(
                PointSampleCoordinates::new(
                    TextureCoordinateS10_5::from_raw(s),
                    TextureCoordinateS10_5::from_raw(t),
                ),
                TmemFirstRowParity::Even,
            ),
            TextureLutMode::Rgba16,
        )
        .map(|sample| sample.texel().rgba8888())
    }

    #[test]
    fn specialization_matches_the_generic_oracle_at_boundaries_and_mutations() {
        let mut tmem = CorpusTmem::complete();
        let tile = exact_inputs().tile();
        let specialized = RankOneCi4Rgba16;
        let boundaries = [
            i16::MIN,
            -1025,
            -513,
            -512,
            -511,
            -33,
            -32,
            -31,
            -1,
            0,
            1,
            31,
            32,
            33,
            479,
            480,
            481,
            511,
            512,
            513,
            i16::MAX,
        ];
        for &s in &boundaries {
            for &t in &boundaries {
                assert_eq!(specialized.sample(&tmem, s, t), generic_sample(&tmem, tile, s, t));
            }
        }

        let mut state = 0x9e37_79b9u32;
        for _ in 0..50_000 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let s = state as i16;
            state = state.rotate_left(11).wrapping_add(0x7f4a_7c15);
            let t = state as i16;
            assert_eq!(specialized.sample(&tmem, s, t), generic_sample(&tmem, tile, s, t));
        }

        for address in [0u16, 4, 127, 0x0800, 0x0878, 0x0879] {
            tmem.valid[usize::from(address)] = false;
            for &s in &boundaries {
                for &t in &boundaries {
                    assert_eq!(
                        specialized.sample(&tmem, s, t),
                        generic_sample(&tmem, tile, s, t)
                    );
                }
            }
            tmem.valid[usize::from(address)] = true;
        }
    }

    fn time_samples(
        generic: bool,
        tmem: &CorpusTmem,
        tile: TexrectTileBinding,
        coordinates: &[(i16, i16)],
    ) -> Duration {
        let started = Instant::now();
        let mut checksum = 0u64;
        for &(s, t) in coordinates {
            let rgba = if generic {
                generic_sample(tmem, tile, black_box(s), black_box(t)).unwrap()
            } else {
                RankOneCi4Rgba16
                    .sample(tmem, black_box(s), black_box(t))
                    .unwrap()
            };
            checksum = checksum.wrapping_add(u64::from(rgba[0]) + u64::from(rgba[3]));
        }
        black_box(checksum);
        started.elapsed()
    }

    fn full_draw(
        generic: bool,
        candidate: &CandidateColorTarget,
        tmem: &CorpusTmem,
        resident: &[u8],
    ) -> CompletedColorTargetWrite {
        let inputs = exact_inputs();
        let execute = || {
            execute_texture_rectangle(
                candidate,
                inputs.other_mode,
                inputs.draw,
                inputs.tile(),
                tmem,
                inputs.lut_mode,
                TexrectShading::new(
                    inputs.combine,
                    Color4::from_wire(0x2040_80ff),
                    PrimColor::from_wire(0, 0x80ff_40ff),
                ),
                TexrectBlendRegisters::new(
                    Color4::from_wire(0x1020_30ff),
                    Color4::from_wire(0x4050_60ff),
                ),
                RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 256, 256),
                Cow::Borrowed(resident),
                None,
            )
            .unwrap()
        };
        if generic {
            with_generic_rank_one_for_test(execute)
        } else {
            execute()
        }
    }

    fn full_draw_fixture() -> (CandidateColorTarget, CorpusTmem, Vec<u8>) {
        let layout = PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap();
        let key = ColorTargetKey::try_new(
            layout.address(0x400).unwrap(),
            ColorTargetExtent::try_new(64, 64).unwrap(),
            ColorTargetFormat::Rgba16,
        )
        .unwrap();
        let registry = ColorTargetRegistry::try_new(layout, 1).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let resident = vec![0x5a; key.extent().pixels() as usize * 2];
        (candidate, CorpusTmem::complete(), resident)
    }

    #[test]
    fn full_draw_device_bytes_match_the_forced_generic_oracle() {
        let (candidate, tmem, resident) = full_draw_fixture();
        let generic = full_draw(true, &candidate, &tmem, &resident);
        let specialized = full_draw(false, &candidate, &tmem, &resident);
        assert_eq!(
            specialized.device_bytes().device_bytes(),
            generic.device_bytes().device_bytes()
        );
        assert_eq!(specialized.rectangle(), generic.rectangle());
    }

    fn time_full_draws(
        generic: bool,
        count: usize,
        candidate: &CandidateColorTarget,
        tmem: &CorpusTmem,
        resident: &[u8],
    ) -> Duration {
        let started = Instant::now();
        let mut checksum = 0u64;
        for iteration in 0..count {
            let completed = full_draw(generic, candidate, tmem, black_box(resident));
            let bytes = completed.device_bytes().device_bytes();
            checksum = checksum.wrapping_add(u64::from(bytes[iteration % bytes.len()]));
        }
        black_box(checksum);
        started.elapsed()
    }

    #[test]
    #[ignore = "release-only alternating microbenchmark"]
    fn release_microbenchmark_is_a_meaningful_win() {
        assert!(!cfg!(debug_assertions), "run this benchmark with --release");
        let tmem = CorpusTmem::complete();
        let tile = exact_inputs().tile();
        let coordinates = (0..250_000u32)
            .map(|index| {
                let s = index.wrapping_mul(73).wrapping_add(index >> 3) as i16;
                let t = index.wrapping_mul(151).wrapping_add(index >> 5) as i16;
                (s, t)
            })
            .collect::<Vec<_>>();
        let mut generic = Duration::ZERO;
        let mut specialized = Duration::ZERO;
        for round in 0..10 {
            if round & 1 == 0 {
                generic += time_samples(true, &tmem, tile, &coordinates);
                specialized += time_samples(false, &tmem, tile, &coordinates);
            } else {
                specialized += time_samples(false, &tmem, tile, &coordinates);
                generic += time_samples(true, &tmem, tile, &coordinates);
            }
        }
        eprintln!(
            "rank-one-ci4-rgba16 generic_ns={} specialized_ns={} speedup={:.2}x",
            generic.as_nanos(),
            specialized.as_nanos(),
            generic.as_secs_f64() / specialized.as_secs_f64()
        );
        assert!(
            specialized.as_nanos() * 10 < generic.as_nanos() * 9,
            "the specialization must save at least 10%: generic={generic:?}, specialized={specialized:?}"
        );
    }

    #[test]
    #[ignore = "release-only alternating full-draw microbenchmark"]
    fn release_full_draw_microbenchmark_is_a_meaningful_net_win() {
        assert!(!cfg!(debug_assertions), "run this benchmark with --release");
        let (candidate, tmem, resident) = full_draw_fixture();
        let mut generic = Duration::ZERO;
        let mut specialized = Duration::ZERO;
        for round in 0..10 {
            if round & 1 == 0 {
                generic += time_full_draws(true, 20, &candidate, &tmem, &resident);
                specialized += time_full_draws(false, 20, &candidate, &tmem, &resident);
            } else {
                specialized += time_full_draws(false, 20, &candidate, &tmem, &resident);
                generic += time_full_draws(true, 20, &candidate, &tmem, &resident);
            }
        }
        eprintln!(
            "rank-one-full-draw generic_ns={} specialized_ns={} speedup={:.2}x",
            generic.as_nanos(),
            specialized.as_nanos(),
            generic.as_secs_f64() / specialized.as_secs_f64()
        );
        assert!(
            specialized.as_nanos() * 100 < generic.as_nanos() * 95,
            "the full draw must save at least 5%: generic={generic:?}, specialized={specialized:?}"
        );
    }
}

/// Executes one admitted `TextureRectangle` against `candidate`, sampling
/// every texel from `tmem` -- any [`TmemByteSource`], which in practice is
/// one of exactly two images the caller chooses between by a rule this
/// function does not apply:
///
/// - a [`PendingTmemImage`](crate::tmem::PendingTmemImage), the sealed
///   post-image of the **same packet's**
///   own TMEM loads, for a packet that carries at least one load; or
/// - the durable [`PhysicalTmemState`] the coordinator holds, for a packet
///   that carries **no** load at all and therefore samples what an earlier
///   packet already published.
///
/// Generic rather than two overloads for the same reason
/// [`sample_point`] is: one addressing/validity/XOR4/TLUT path, so the two
/// images cannot disagree about a texel. The distinction survives in the
/// data, not the signature -- a sampled texel's `snapshot()` answers
/// `Proposed` for the post-image and `Committed` for durable state, and the
/// caller checks that crossing rather than trusting it.
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
pub fn execute_texture_rectangle<'a, S: crate::TmemByteSource + ?Sized>(
    candidate: &CandidateColorTarget,
    other_mode: OtherMode,
    draw: TexrectDraw,
    tile: TexrectTileBinding,
    tmem: &S,
    lut_mode: TextureLutMode,
    shading: TexrectShading,
    blend_registers: TexrectBlendRegisters,
    scissor: RdpScissorRect,
    resident_bytes: impl Into<Cow<'a, [u8]>>,
    already_initialized: Option<TargetRectangle>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    let timing = texrect_timing_census::StartedDraw::if_enabled(
        shading.combine(),
        other_mode,
        candidate.key().format(),
        lut_mode,
        tile,
        draw,
        u64::from(draw.width()) * u64::from(draw.height()),
    );
    let mut bytes = resident_bytes.into().into_owned();
    // Copy cycle blits the texel to the destination with no combiner, which
    // is what the RDP itself does in that mode. One-cycle runs the texel
    // through the color combiner once per fragment; two-cycle runs it twice
    // with the cross-cycle carry, both through `crate::combiner`'s own
    // evaluators. Fill cycle samples no texture at all and is refused by
    // name rather than drawn as an approximation.
    let evaluation = admitted_cycle_evaluation(other_mode.cycle_type())?;
    // Selector admission runs before any pixel is produced, so an
    // unevaluatable program refuses with an untouched target rather than a
    // half-drawn one. Skipped in Copy cycle, where the RDP consults no
    // combiner program at all and gating on one would refuse a rectangle
    // the hardware draws.
    let base_inputs = match evaluation.validated_cycles() {
        Some(cycles) => Some(shading.validate_combiner_program(cycles)?.base_inputs()),
        None => None,
    };
    // The blender's own admission, run at the same point and for the same
    // reason as the combiner's: before any pixel is produced, so a mode
    // this executor cannot evaluate exactly refuses with an untouched
    // target rather than a half-drawn one. Copy cycle passes through with
    // `cycle_count() == 0`, which is the RDP's own blender bypass.
    let blend_state = blend_registers.mode_state(other_mode);
    require_blendable_mode(blend_state)?;
    // The other three post-combiner stages, admitted at the same point and
    // for the same reason: a mode this executor cannot evaluate exactly
    // refuses with an untouched target rather than a half-drawn one.
    let stages = TexrectFragmentStages::try_new(other_mode, blend_registers.blend_color)?;

    let key = candidate.key();
    let format = key.format();
    let rank_one = RankOneCi4Rgba16::admit(
        shading.combine(),
        other_mode,
        format,
        lut_mode,
        tile,
        draw,
    );
    let extent = key.extent();
    let rectangle = TargetRectangle::try_new(draw.left(), draw.top(), draw.width(), draw.height())?;
    // **Clipped, not refused.** Pinned RT64 intersects the scissor and draw
    // rectangles and keeps a non-empty intersection rather than rejecting
    // an overhanging primitive
    // (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`).
    // A rectangle that
    // overhangs the framebuffer is routine content, and the previous
    // `OutsideTarget` refusal here dropped all of it. See
    // [`clip_texrect_extent`] for the precedence between the scissor and
    // the target extent, and for what still refuses.
    let clipped = clip_texrect_extent(
        draw,
        scissor,
        extent.width(),
        extent.height(),
        key,
        rectangle,
    )?;
    // The rectangle actually written, after clipping -- narrower than
    // `rectangle` whenever the scissor or the target bit into it. This is
    // what the journal is told about, because it is what the pixel loop
    // below touches; claiming the unclipped rectangle would declare rows
    // this call never writes.
    let drawn = TargetRectangle::try_new(
        draw.left() + clipped.first_column,
        draw.top() + clipped.first_row,
        clipped.column_limit - clipped.first_column,
        clipped.row_limit - clipped.first_row,
    )?;
    // Planned, not just bounds-checked: `plan_rows` is the target's own
    // row-planning authority and rejects the same out-of-bounds cases with
    // its own named error. Calling it keeps this executor and the fill
    // executor on one row planner. Handed the CLIPPED rectangle, which is
    // the one whose rows are written.
    let _plan = candidate.plan_rows(drawn)?;

    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let full_len = (extent.pixels() as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or(TargetError::PixelBufferLengthOverflow {
            pixels: extent.pixels() as usize,
            bytes_per_pixel: format.bytes_per_pixel(),
        })?;
    if bytes.len() != full_len {
        return Err(TargetError::CompletedByteLengthMismatch {
            key,
            generation: candidate.generation(),
            expected: full_len,
            actual: bytes.len(),
        }
        .into());
    }
    // **First-row parity comes from the tile's own T origin, not a
    // constant.** [`crate::TmemFirstRowParity`] is explicit caller input by
    // design -- the reader never infers it -- so this executor owes the
    // reader the same parity the *writer* used, or the two disagree about
    // which TMEM rows carry the XOR4 bank exchange.
    //
    // The writer's rule is `tmem/types.rs`'s `project_tmem_transfer_word`,
    // `TmemLoadKind::Tile` arm: `odd_row_exchange = (bounds.low_t().integer()
    // + row) & 1`, applied to the physical lanes by
    // `tmem/execute/load_tile.rs`'s `map_physical_lanes`. The reader's rule
    // is `tmem/read.rs`'s `odd_row_exchange`: `first_is_odd ^ (row & 1)`.
    // The two agree exactly when `first_is_odd == low_t.integer() & 1`, and
    // this line is that equality.
    //
    // A frozen `Even` was previously passed here. That is correct only for
    // a tile whose T origin is even -- and it is invisible for `LoadBlock`,
    // whose `transfer_shape` `Block` arm always reports `row_count = 1` so
    // its own `odd_row_exchange` never fires on the write side. Measured on
    // the real ROM, WM2000's sprite-strip tile has `low_t.integer() == 47`,
    // an ODD origin, so the frozen constant inverted the exchange for every
    // row and each rectangle row's last pixel read a byte the load never
    // wrote (`tmem::read::tests`'s two `wm2000_texrect_*` tests pin exactly
    // that, including the production abort's own byte `0x04c`).
    let first_row_parity = if tile.size().low_t().integer() & 1 == 1 {
        TmemFirstRowParity::Odd
    } else {
        TmemFirstRowParity::Even
    };

    // The loop walks the CLIPPED offsets, but `t_at`/`s_at` are still
    // indexed by the offset from the rectangle's own unclipped origin --
    // that is the whole reason `clip_texrect_extent` returns offsets rather
    // than a narrowed `TexrectDraw`. `rdp_tex_rect` loads the S/T origin and
    // steps once from the unclipped command (`rasterizer.c:2657-2677`) and
    // the edgewalker's clip touches only `majorx`/`minorx` (`:2349-2363`),
    // so a clipped rectangle samples the same texel at a given screen pixel
    // that an unclipped one would. Rebasing the ramp onto the clipped left
    // edge would slide the texture sideways by the clipped amount.
    for row in clipped.rows() {
        for column in clipped.columns() {
            let (s, t) = draw.coordinates_at(column, row);
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
                first_row_parity,
            );
            let sampled_rgba = match rank_one {
                Some(specialized) => specialized.sample(tmem, s, t),
                None => sample_point(tmem, tile.descriptor(), tile.size(), request, lut_mode)
                    .map(|decoded| decoded.texel().rgba8888()),
            }
            .map_err(|source| TexrectExecutionError::Sample {
                column,
                row,
                source,
            })?;
            let rgba = match base_inputs {
                // Copy cycle: the sampled texel's own RGBA8888, unchanged.
                None => sampled_rgba,
                // One cycle: `(A-B)*C+D` for color and alpha independently,
                // then RT64's final `wrapClamp` -- all inside
                // `run_one_cycle`, which is the triangle pipeline's own
                // evaluator, not a second copy of the arithmetic. The
                // texel enters as `tex_val0` normalized by `/ 255.0`,
                // matching `RasterPS.hlsl`'s already-normalized sample, and
                // the `[0.0, 1.0]` result is returned to bytes by
                // `* 255.0` then round-half-away-from-zero (`f32::round`),
                // the same quantization `production.rs`'s existing
                // WGSL-agreement test uses. Rounding happens strictly
                // after `wrap_clamp`: clamping a rounded value and
                // rounding a clamped one differ at the boundary, and RT64
                // clamps in float before any quantization.
                Some(base) => combine_one_texel(
                    shading.combine(),
                    base,
                    sampled_rgba,
                    evaluation,
                ),
            };
            let pixel_x = draw.left() + column;
            let pixel_y = draw.top() + row;
            let offset =
                (pixel_y as usize * extent.width() as usize + pixel_x as usize) * bytes_per_pixel;
            // **The blender, the stage this executor previously declared it
            // did not run**, composed with the write in one named function
            // so a mutation that drops it is reachable from a unit test --
            // the same reason `combine_one_texel` is a function rather than
            // an inline block (see its own doc: while that arithmetic was
            // inline, replacing `round()` with a truncating cast left the
            // entire suite green).
            blend_and_write_pixel(
                format,
                &mut bytes[offset..offset + bytes_per_pixel],
                rgba,
                blend_state,
                stages,
                column,
                row,
            )?;
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
    //
    // `drawn`, not `rectangle`: the claim must be what this call actually
    // wrote. A clipped texrect covers less than its command asked for, and
    // claiming the unclipped rect would assert proof over pixels the
    // scissor kept it away from.
    let claimed = union_rectangle(drawn, already_initialized);
    let completed = CompletedColorTargetWrite::new_for_fill(
        key,
        candidate.generation(),
        key.range(),
        claimed,
        device_bytes,
    );
    if let Some(timing) = timing {
        timing.finish(u64::from(drawn.width()) * u64::from(drawn.height()));
    }
    Ok(completed)
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

/// Emits the current exact-key texrect timing snapshot when the census is
/// enabled. Backend teardown may call this explicitly; normal thread exit
/// also flushes the final partial reporting interval.
pub(crate) fn flush_texrect_timing_census() {
    texrect_timing_census::flush("explicit");
}

/// Default-off timing for successful CPU texrect execution, keyed by every
/// state field needed to choose a closed exact specialization.
///
/// There is deliberately no task identifier here: the target executor owns
/// no production scheduling context. Rows are cumulative rankings. Joining a
/// row to one drawn-frame tail requires the production caller to supply its
/// task/member identity at the `execute_scheduled_texrect` seam rather than
/// introducing an ambient thread-local identity in this reusable executor.
mod texrect_timing_census {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    const REPORT_EVERY_CALLS: u64 = 5_000;

    const fn target_format_code(format: ColorTargetFormat) -> u8 {
        match format {
            ColorTargetFormat::Rgba16 => 16,
            ColorTargetFormat::Rgba32 => 32,
        }
    }

    /// Public RDP `G_TT_*` encodings: disabled=0, RGBA16=2, IA16=3.
    const fn lut_mode_code(mode: TextureLutMode) -> u8 {
        match mode {
            TextureLutMode::Disabled => 0,
            TextureLutMode::Rgba16 => 2,
            TextureLutMode::Ia16 => 3,
        }
    }

    /// Public RDP image-format encodings (`G_IM_FMT_*`).
    const fn tile_format_code(format: ImageFormat) -> u8 {
        match format {
            ImageFormat::Rgba => 0,
            ImageFormat::Yuv => 1,
            ImageFormat::ColorIndex => 2,
            ImageFormat::IntensityAlpha => 3,
            ImageFormat::Intensity => 4,
        }
    }

    const fn tile_size_bits(size: PixelSize) -> u8 {
        match size {
            PixelSize::Bits4 => 4,
            PixelSize::Bits8 => 8,
            PixelSize::Bits16 => 16,
            PixelSize::Bits32 => 32,
        }
    }

    const fn address_mode_bits(mode: TileAddressMode) -> u8 {
        let mirror = if mode.mirror() { 1 } else { 0 };
        let clamp = if mode.clamp() { 1 } else { 0 };
        mirror | (clamp << 1)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Key {
        combine_low: u32,
        combine_high: u32,
        other_mode_high: u32,
        other_mode_low: u32,
        target_format: u8,
        lut_mode: u8,
        tile_format: u8,
        tile_size: u8,
        tile_line_words: u16,
        tile_tmem_word: u16,
        tile_palette: u8,
        tile_s_mode: u8,
        tile_mask_s: u8,
        tile_shift_s: u8,
        tile_t_mode: u8,
        tile_mask_t: u8,
        tile_shift_t: u8,
        tile_low_s: u16,
        tile_low_t: u16,
        tile_high_s: u16,
        tile_high_t: u16,
        tile_low_t_parity: u8,
        flipped_axes: bool,
    }

    impl Key {
        fn new(
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            tile: TexrectTileBinding,
            draw: TexrectDraw,
        ) -> Self {
            let descriptor = tile.descriptor();
            let size = tile.size();
            Self {
                combine_low: combine.low(),
                combine_high: combine.high(),
                other_mode_high: other_mode.high(),
                other_mode_low: other_mode.low(),
                target_format: target_format_code(target_format),
                lut_mode: lut_mode_code(lut_mode),
                tile_format: tile_format_code(descriptor.format()),
                tile_size: tile_size_bits(descriptor.size()),
                tile_line_words: descriptor.line_words(),
                tile_tmem_word: descriptor.tmem().get(),
                tile_palette: descriptor.palette(),
                tile_s_mode: address_mode_bits(descriptor.s_mode()),
                tile_mask_s: descriptor.mask_s(),
                tile_shift_s: descriptor.shift_s(),
                tile_t_mode: address_mode_bits(descriptor.t_mode()),
                tile_mask_t: descriptor.mask_t(),
                tile_shift_t: descriptor.shift_t(),
                tile_low_s: size.low_s().raw(),
                tile_low_t: size.low_t().raw(),
                tile_high_s: size.high_s().raw(),
                tile_high_t: size.high_t().raw(),
                tile_low_t_parity: (size.low_t().integer() & 1) as u8,
                flipped_axes: draw.flipped_axes,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct Stats {
        calls: u64,
        requested_pixels: u64,
        clipped_pixels: u64,
        elapsed_ns: u128,
        max_call_ns: u128,
    }

    impl Stats {
        fn note(&mut self, requested_pixels: u64, clipped_pixels: u64, elapsed: Duration) {
            self.calls += 1;
            self.requested_pixels += requested_pixels;
            self.clipped_pixels += clipped_pixels;
            self.elapsed_ns += elapsed.as_nanos();
            self.max_call_ns = self.max_call_ns.max(elapsed.as_nanos());
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct Census {
        calls: u64,
        keys: BTreeMap<Key, Stats>,
    }

    static CENSUS: Mutex<Option<Census>> = Mutex::new(None);
    static LAST_EMITTED_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct ThreadExitReporter;

    impl Drop for ThreadExitReporter {
        fn drop(&mut self) {
            flush("final");
        }
    }

    thread_local! {
        static THREAD_EXIT_REPORTER: ThreadExitReporter = const { ThreadExitReporter };
    }

    fn env_value_enables(value: Option<&OsStr>) -> bool {
        value.is_some_and(|value| value != OsStr::new("0"))
    }

    fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            env_value_enables(std::env::var_os("FN64_TEXRECT_TIMING_CENSUS").as_deref())
        });
        if enabled {
            THREAD_EXIT_REPORTER.with(|_| {});
        }
        enabled
    }

    pub(super) struct StartedDraw {
        key: Key,
        requested_pixels: u64,
        started: Instant,
    }

    impl StartedDraw {
        pub(super) fn if_enabled(
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            tile: TexrectTileBinding,
            draw: TexrectDraw,
            requested_pixels: u64,
        ) -> Option<Self> {
            enabled().then(|| Self {
                key: Key::new(combine, other_mode, target_format, lut_mode, tile, draw),
                requested_pixels,
                started: Instant::now(),
            })
        }

        pub(super) fn finish(self, clipped_pixels: u64) {
            note(
                self.key,
                self.requested_pixels,
                clipped_pixels,
                self.started.elapsed(),
            );
        }
    }

    fn note(key: Key, requested_pixels: u64, clipped_pixels: u64, elapsed: Duration) {
        let snapshot = {
            let mut guard = CENSUS.lock().expect("texrect timing census mutex poisoned");
            let census = guard.get_or_insert_with(Census::default);
            census.calls += 1;
            census
                .keys
                .entry(key)
                .or_default()
                .note(requested_pixels, clipped_pixels, elapsed);
            (census.calls % REPORT_EVERY_CALLS == 0).then(|| census.clone())
        };
        if let Some(snapshot) = snapshot {
            emit_snapshot("periodic", &snapshot);
        }
    }

    pub(super) fn flush(tag: &str) {
        let snapshot = CENSUS
            .lock()
            .expect("texrect timing census mutex poisoned")
            .as_ref()
            .cloned();
        if let Some(snapshot) = snapshot {
            emit_snapshot(tag, &snapshot);
        }
    }

    fn emit_snapshot(tag: &str, census: &Census) {
        use std::sync::atomic::Ordering;

        let prior = LAST_EMITTED_CALLS.fetch_max(census.calls, Ordering::Relaxed);
        if census.calls <= prior {
            return;
        }
        for row in format_report(tag, census) {
            eprintln!("{row}");
        }
    }

    fn format_report(tag: &str, census: &Census) -> Vec<String> {
        let mut ranked = census.keys.iter().collect::<Vec<_>>();
        ranked.sort_by(|(key_a, stats_a), (key_b, stats_b)| {
            stats_b
                .elapsed_ns
                .cmp(&stats_a.elapsed_ns)
                .then_with(|| key_a.cmp(key_b))
        });
        let total_requested = census
            .keys
            .values()
            .map(|stats| stats.requested_pixels)
            .sum::<u64>();
        let total_clipped = census
            .keys
            .values()
            .map(|stats| stats.clipped_pixels)
            .sum::<u64>();
        let total_ns = census
            .keys
            .values()
            .map(|stats| stats.elapsed_ns)
            .sum::<u128>();
        let mut rows = vec![format!(
            "[fn64-texrect-census] snapshot={tag} calls={} keys={} requested_pixels={} clipped_pixels={} elapsed_ns={} elapsed_ms={:.3}",
            census.calls,
            census.keys.len(),
            total_requested,
            total_clipped,
            total_ns,
            total_ns as f64 / 1_000_000.0,
        )];
        for (rank, (key, stats)) in ranked.into_iter().take(16).enumerate() {
            rows.push(format_row(tag, rank + 1, key, stats));
        }
        rows
    }

    fn format_row(tag: &str, rank: usize, key: &Key, stats: &Stats) -> String {
        let ns_per_clipped_pixel = if stats.clipped_pixels == 0 {
            0.0
        } else {
            stats.elapsed_ns as f64 / stats.clipped_pixels as f64
        };
        format!(
            "[fn64-texrect-census] snapshot={tag} rank={rank} combine={:#010x}/{:#010x} other={:#010x}/{:#010x} target_fmt={} lut={} tile_fmt={} tile_size={} line_words={} tmem_word={} palette={} s_mode={} mask_s={} shift_s={} t_mode={} mask_t={} shift_t={} low_s={} low_t={} high_s={} high_t={} low_t_parity={} flipped={} calls={} requested_pixels={} clipped_pixels={} elapsed_ns={} max_call_ns={} elapsed_ms={:.3} max_call_ms={:.3} ns_per_clipped_pixel={:.2}",
            key.combine_low,
            key.combine_high,
            key.other_mode_high,
            key.other_mode_low,
            key.target_format,
            key.lut_mode,
            key.tile_format,
            key.tile_size,
            key.tile_line_words,
            key.tile_tmem_word,
            key.tile_palette,
            key.tile_s_mode,
            key.tile_mask_s,
            key.tile_shift_s,
            key.tile_t_mode,
            key.tile_mask_t,
            key.tile_shift_t,
            key.tile_low_s,
            key.tile_low_t,
            key.tile_high_s,
            key.tile_high_t,
            key.tile_low_t_parity,
            u8::from(key.flipped_axes),
            stats.calls,
            stats.requested_pixels,
            stats.clipped_pixels,
            stats.elapsed_ns,
            stats.max_call_ns,
            stats.elapsed_ns as f64 / 1_000_000.0,
            stats.max_call_ns as f64 / 1_000_000.0,
            ns_per_clipped_pixel,
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use fn64_render::{
            NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode, NeutralTileDescriptor,
            NeutralTileSize,
        };

        #[derive(Clone, Copy)]
        struct Inputs {
            combine: CombineParams,
            other_mode: OtherMode,
            target_format: ColorTargetFormat,
            lut_mode: TextureLutMode,
            descriptor: NeutralTileDescriptor,
            size: NeutralTileSize,
            draw: TexrectDraw,
        }

        impl Inputs {
            fn key(self) -> Key {
                Key::new(
                    self.combine,
                    self.other_mode,
                    self.target_format,
                    self.lut_mode,
                    TexrectTileBinding::try_from_neutral(self.descriptor, self.size).unwrap(),
                    self.draw,
                )
            }
        }

        fn base_inputs() -> Inputs {
            Inputs {
                combine: CombineParams::from_wire(1, 2),
                other_mode: OtherMode::from_wire(3, 4),
                target_format: ColorTargetFormat::Rgba16,
                lut_mode: TextureLutMode::Disabled,
                descriptor: NeutralTileDescriptor {
                    format: NeutralImageFormat::Rgba,
                    size: NeutralPixelSize::Bits4,
                    line_words: 4,
                    tmem_word_address: 7,
                    palette: 3,
                    s_mode: NeutralTileAddressMode::default(),
                    mask_s: 2,
                    shift_s: 1,
                    t_mode: NeutralTileAddressMode::default(),
                    mask_t: 3,
                    shift_t: 2,
                },
                size: NeutralTileSize {
                    low_s: 0,
                    low_t: 0,
                    high_s: 60,
                    high_t: 56,
                },
                draw: TexrectDraw {
                    left: 0,
                    top: 0,
                    right: 8,
                    bottom: 8,
                    s_start: 0,
                    t_start: 0,
                    s_end: 256,
                    t_end: 256,
                    flipped_axes: false,
                },
            }
        }

        fn base_key() -> Key {
            base_inputs().key()
        }

        #[test]
        fn default_off_requires_the_environment_variable_to_exist() {
            assert!(!env_value_enables(None));
            assert!(!env_value_enables(Some(OsStr::new("0"))));
            assert!(env_value_enables(Some(OsStr::new(""))));
            assert!(env_value_enables(Some(OsStr::new("1"))));
        }

        #[test]
        fn exact_key_denominator_contains_every_requested_field() {
            let base = base_inputs();
            let mut variants = Vec::new();

            let mut input = base;
            input.combine = CombineParams::from_wire(11, 2);
            variants.push(input.key());
            let mut input = base;
            input.combine = CombineParams::from_wire(1, 12);
            variants.push(input.key());
            let mut input = base;
            input.other_mode = OtherMode::from_wire(13, 4);
            variants.push(input.key());
            let mut input = base;
            input.other_mode = OtherMode::from_wire(3, 14);
            variants.push(input.key());
            let mut input = base;
            input.target_format = ColorTargetFormat::Rgba32;
            variants.push(input.key());
            let mut input = base;
            input.lut_mode = TextureLutMode::Rgba16;
            variants.push(input.key());

            let mut input = base;
            input.descriptor.format = NeutralImageFormat::ColorIndex;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.size = NeutralPixelSize::Bits16;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.line_words = 5;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.tmem_word_address = 8;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.palette = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.s_mode.mirror = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.s_mode.clamp = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.mask_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.shift_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.t_mode.mirror = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.t_mode.clamp = true;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.mask_t = 5;
            variants.push(input.key());
            let mut input = base;
            input.descriptor.shift_t = 5;
            variants.push(input.key());

            let mut input = base;
            input.size.low_s = 4;
            variants.push(input.key());
            let mut input = base;
            input.size.low_t = 4;
            let odd_low_t = input.key();
            assert_eq!(odd_low_t.tile_low_t_parity, 1);
            variants.push(odd_low_t);
            let mut input = base;
            input.size.high_s = 64;
            variants.push(input.key());
            let mut input = base;
            input.size.high_t = 64;
            variants.push(input.key());
            let mut input = base;
            input.draw = input.draw.with_flipped_axes();
            variants.push(input.key());

            let key = base.key();
            assert_eq!(key.tile_line_words, 4);
            assert_eq!(key.tile_tmem_word, 7);
            assert_eq!(key.tile_palette, 3);
            assert_eq!(key.tile_low_s, 0);
            assert_eq!(key.tile_low_t, 0);
            assert_eq!(key.tile_high_s, 60);
            assert_eq!(key.tile_high_t, 56);
            assert_eq!(key.tile_low_t_parity, 0);
            assert!(!key.flipped_axes);

            let mut denominator = std::collections::BTreeSet::from([key]);
            denominator.extend(variants);
            assert_eq!(denominator.len(), 25);
        }

        #[test]
        fn aggregation_tracks_calls_both_pixel_denominators_total_and_maximum() {
            let mut stats = Stats::default();
            stats.note(80, 60, Duration::from_micros(10));
            stats.note(120, 40, Duration::from_micros(40));
            assert_eq!(
                stats,
                Stats {
                    calls: 2,
                    requested_pixels: 200,
                    clipped_pixels: 100,
                    elapsed_ns: 50_000,
                    max_call_ns: 40_000,
                }
            );
        }

        #[test]
        fn final_snapshot_reports_a_partial_interval_with_a_checkable_denominator() {
            let mut census = Census {
                calls: 1,
                keys: BTreeMap::new(),
            };
            census.keys.insert(
                base_key(),
                Stats {
                    calls: 2,
                    requested_pixels: 200,
                    clipped_pixels: 100,
                    elapsed_ns: 50_000,
                    max_call_ns: 40_000,
                },
            );
            let rows = format_report("final", &census);
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[0],
                "[fn64-texrect-census] snapshot=final calls=1 keys=1 requested_pixels=200 clipped_pixels=100 elapsed_ns=50000 elapsed_ms=0.050"
            );
            for field in [
                "snapshot=final",
                "rank=1",
                "combine=0x00000001/0x00000002",
                "other=0x00000003/0x00000004",
                "target_fmt=16",
                "lut=0",
                "tile_fmt=0",
                "tile_size=4",
                "line_words=4",
                "tmem_word=7",
                "palette=3",
                "s_mode=0",
                "mask_s=2",
                "shift_s=1",
                "t_mode=0",
                "mask_t=3",
                "shift_t=2",
                "low_s=0",
                "low_t=0",
                "high_s=60",
                "high_t=56",
                "low_t_parity=0",
                "flipped=0",
                "calls=2",
                "requested_pixels=200",
                "clipped_pixels=100",
                "elapsed_ns=50000",
                "max_call_ns=40000",
                "elapsed_ms=0.050",
                "max_call_ms=0.040",
                "ns_per_clipped_pixel=500.00",
            ] {
                assert!(rows[1].split_ascii_whitespace().any(|token| token == field));
            }
        }
    }
}

/// How this executor evaluates a texel for a given cycle type, or a named
/// refusal.
///
/// Copy cycle blits, which is the RDP's own behavior in that mode.
/// One-cycle runs `(A-B)*C+D` once, over the *second*-cycle bitfield slice
/// (RT64's `runCycle(inputs, twoCycle ? 0 : 1, ...)`). Two-cycle runs it
/// twice, cycle 0's slice then cycle 1's, threading the accumulator between
/// them with the cross-cycle-carry wrap -- exactly
/// [`crate::combiner::run_two_cycle`], which this crate has always had.
///
/// Fill cycle samples no texture at all and is still refused here.
///
/// A named function rather than an inline match so the decision is
/// reachable from a unit test -- reaching `execute_texture_rectangle`
/// itself requires a live pending TMEM transaction. Measured, not
/// stylistic: while this match was inline, widening it to admit two-cycle
/// left the entire suite green, because nothing observed the *arithmetic*
/// the widened arm selects. [`two_cycle_carries_the_accumulator_one_cycle_cannot`]
/// is the observation that closes that gap.
pub(super) fn admitted_cycle_evaluation(
    cycle_type: CycleType,
) -> Result<TexrectCombinerEvaluation, TexrectExecutionError> {
    match cycle_type {
        CycleType::Copy => Ok(TexrectCombinerEvaluation::BlitsTheTexel),
        CycleType::OneCycle => Ok(TexrectCombinerEvaluation::OneCycle),
        CycleType::TwoCycle => Ok(TexrectCombinerEvaluation::TwoCycle),
        CycleType::Fill => Err(TexrectExecutionError::UnsupportedCycleType { cycle_type }),
    }
}

/// [`admitted_cycle_evaluation`]'s three outcomes, as one typed value rather
/// than the `bool` this decision used to be: a `bool` could distinguish
/// "combines" from "blits" but had nowhere to put "combines *twice*".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TexrectCombinerEvaluation {
    /// Copy cycle. The sampled texel's own RGBA8888, unchanged; the RDP
    /// consults no combiner program in this mode.
    BlitsTheTexel,
    /// One pass of `(A-B)*C+D` over the second-cycle bitfield slice.
    OneCycle,
    /// Two passes: cycle 0's slice, then cycle 1's over the accumulator
    /// cycle 0 wrote, with `wrapInputC`/`wrapInputABD` applied to the carry.
    TwoCycle,
}

impl TexrectCombinerEvaluation {
    /// The combiner-program validation this evaluation requires, or `None`
    /// in Copy cycle where no program is consulted.
    pub(super) const fn validated_cycles(self) -> Option<CombinerProgramCycles> {
        match self {
            Self::BlitsTheTexel => None,
            Self::OneCycle => Some(CombinerProgramCycles::OnlySecondSlice),
            Self::TwoCycle => Some(CombinerProgramCycles::BothSlices),
        }
    }
}

/// Combines one sampled texel through the one-cycle color combiner.
///
/// The texel enters as `Texel0` normalized by `/ 255.0`, matching
/// `RasterPS.hlsl`'s already-normalized sample, and the `[0.0, 1.0]` result
/// returns to bytes by `* 255.0` then [`f32::round`]
/// (round-half-away-from-zero).
///
/// **Order is load bearing and is not an implementation detail.** RT64's
/// `wrapClamp` runs in float inside [`run_one_cycle`], strictly before any
/// quantization here; clamping a rounded value and rounding a clamped one
/// differ at the boundary. Likewise rounding rather than truncating is a
/// real choice with an observable witness -- see
/// `the_quantization_rounds_rather_than_truncating`, which records the
/// mutation that survived until it existed.
///
/// A named function rather than an inline block inside the pixel loop so a
/// mutation to this arithmetic is reachable from a unit test. Measured, not
/// stylistic: while the arithmetic was inline, replacing `round()` with a
/// truncating cast left the entire suite green, because every unit test
/// reached the arithmetic through the test module's own copy of it.
///
/// `alphaCompareValue`, the combiner's second return, is deliberately
/// discarded: alpha compare is a separate stage this executor does not run
/// (see this module's Nonclaims).
///
/// `evaluation` selects [`run_one_cycle`] or [`run_two_cycle`]. Both are
/// `crate::combiner`'s own public entry points -- the triangle pipeline's
/// evaluators, not a second copy of the arithmetic. Two-cycle is **not**
/// one-cycle run twice: `run_two_cycle` reads the cycle-0 bitfield slice on
/// its first pass and applies `wrapInputC`/`wrapInputABD` to the
/// accumulator before the second pass reads it as `COMBINED`, neither of
/// which one-cycle mode does. `two_cycle_carries_the_accumulator_one_cycle_cannot`
/// pins a program where the two answers differ.
///
/// [`TexrectCombinerEvaluation::BlitsTheTexel`] never reaches here: Copy
/// cycle short-circuits at the call site with the texel's own bytes, and
/// admitting it to a combiner call would evaluate a latched program the RDP
/// ignores in that mode.
pub(super) fn combine_one_texel(
    combine: CombineParams,
    base: CombinerInputs,
    texel: [u8; 4],
    evaluation: TexrectCombinerEvaluation,
) -> [u8; 4] {
    let inputs = inputs_with_texel(base, texel);
    let (combined_color, _alpha_compare) = match evaluation {
        TexrectCombinerEvaluation::OneCycle | TexrectCombinerEvaluation::BlitsTheTexel => {
            run_one_cycle(combine, inputs)
        }
        TexrectCombinerEvaluation::TwoCycle => run_two_cycle(combine, inputs),
    };
    quantize_combined_color(combined_color)
}

pub(super) fn combine_one_texel_prepared_two_cycle(
    combine: PreparedTwoCycleCombiner,
    base: CombinerInputs,
    texel: [u8; 4],
) -> [u8; 4] {
    let (combined_color, _alpha_compare) = combine.run(inputs_with_texel(base, texel));
    quantize_combined_color(combined_color)
}

fn inputs_with_texel(base: CombinerInputs, texel: [u8; 4]) -> CombinerInputs {
    let [red, green, blue, alpha] = texel;
    CombinerInputs {
        tex_val0: [
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0,
            f32::from(alpha) / 255.0,
        ],
        ..base
    }
}

fn quantize_combined_color(combined_color: [f32; 4]) -> [u8; 4] {
    combined_color.map(|channel| (channel * 255.0).round() as u8)
}

/// The two blender-only color registers, snapshotted at the texrect's own
/// stream position exactly as [`TexrectShading`]'s combiner registers are.
///
/// Separate from [`TexrectShading`] because these feed a different stage:
/// the combiner never reads `SetBlendColor`, and the blender never reads
/// `SetPrimColor`.
///
/// Neither is an `Option`. `SetBlendColor` and `SetFogColor` name RDP
/// registers, and a register always holds a value -- zero until the guest
/// writes one. `fn64-render-reference` models both as zero-initialized
/// `[u8; 4]` (`gbi/state.rs:227-228`, `:387-388`) and RT64's own C++
/// zero-initializes `fogColor`/`blendColor` at
/// `src/hle/rt64_state.cpp:130-131`. Treating "never written" as a refusal
/// invented a state the hardware has no way to be in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TexrectBlendRegisters {
    blend_color: Color4,
    fog_color: Color4,
}

impl TexrectBlendRegisters {
    pub const fn new(blend_color: Color4, fog_color: Color4) -> Self {
        Self {
            blend_color,
            fog_color,
        }
    }

    /// Assembles the [`BlendModeState`] [`blend_fragment`] consumes.
    ///
    /// No refusal for a never-written register: both registers always hold
    /// a value (see this type's own doc), so a cycle selecting `Blend` or
    /// `Fog` before any `SetBlendColor`/`SetFogColor` reads the power-on
    /// zero, which is what both other lanes do. The bytes here are the
    /// register's real contents, not a substitution.
    /// The `SetBlendColor` register this texrect/triangle observes.
    ///
    /// An accessor rather than a public field so the two executors read the
    /// register through one name, and so a future refusal for an unset
    /// register has one place to live.
    pub(super) const fn blend_color(self) -> Color4 {
        self.blend_color
    }

    pub(super) fn mode_state(self, other_mode: OtherMode) -> BlendModeState {
        BlendModeState {
            other_mode,
            blend_color_register: self.blend_color.rgba8(),
            fog_color: self.fog_color.rgba8(),
        }
    }
}

/// The dither threshold this executor uses for the RDP's `Noise` dither
/// modes, and the reason it is a constant rather than a sequence.
///
/// **Every `*_dither` port in this workspace bumps a value iff
/// `(value & 7) > threshold`.** The threshold is three bits, so at its
/// maximum of 7 the comparison is false for every input and the stage is
/// the identity. That makes this value not an invented noise sample but
/// **one endpoint of the mode's own output range**: for any input, the
/// dithered result over all eight thresholds takes exactly two values,
/// `floor` and `floor + 1` in the five-bit channel, and 7 selects `floor`.
/// Asserted exhaustively over all 256 inputs and all 8 thresholds in
/// `the_noise_dither_threshold_is_an_endpoint_not_an_invention`.
///
/// **Why an endpoint rather than a refusal.** This crate has no authority
/// for the RDP's random sequence -- the two generators in the workspace are
/// different sequences and neither claims to be silicon
/// (`crate::random`'s RT64 shader PRNG;
/// `fn64-render-reference/src/raster/mod.rs:85-119`'s SplitMix64, whose own
/// source calls it "deliberately not described as the silicon sequence").
/// Emitting either would agree with one implementation by construction and
/// with hardware by accident. Refusing outright would instead decline to
/// draw a frame the RDP does draw, over a stage that provably cannot move
/// a channel by more than one five-bit step. Producing a *named, proven*
/// endpoint of the true range is the honest third option, and it is
/// disclosed in this module's Nonclaims rather than presented as parity.
///
/// The ordered `MagicSquare`/`Bayer` tiles are **not** admitted this way
/// and remain refused
/// ([`TexrectExecutionError::OrderedDitherAuthorityUnsettled`]): their
/// threshold is a screen-registered function this crate's two ports
/// disagree about, so no endpoint argument applies -- picking one would be
/// picking a side.
const NOISE_DITHER_THRESHOLD: AlphaCompareNoise = AlphaCompareNoise(7);

/// Which stage a noise/ordered-dither refusal came from, so the error names
/// the mode that could not be evaluated rather than "dither" in general.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexrectNoiseStage {
    /// `G_AC_DITHER` -- alpha compare against the per-pixel random value.
    AlphaCompareDither,
    /// `G_MDSFT_ALPHADITHER` -- the pre-blend alpha perturbation.
    AlphaDither,
    /// `G_MDSFT_RGBDITHER` -- the memory-interface RGB perturbation.
    RgbDither,
}

impl core::fmt::Display for TexrectNoiseStage {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlphaCompareDither => formatter.write_str("G_AC_DITHER alpha compare"),
            Self::AlphaDither => formatter.write_str("G_MDSFT_ALPHADITHER"),
            Self::RgbDither => formatter.write_str("G_MDSFT_RGBDITHER"),
        }
    }
}

/// The four pipeline stages between the combiner and the framebuffer write,
/// resolved once per rectangle from the latched other-mode word.
///
/// Assembled by [`Self::try_new`], which refuses every mode this executor
/// cannot evaluate **exactly** before any pixel is produced. The refusals
/// are not "not implemented yet": each names a quantity this crate has no
/// authority for (an unpublished noise sequence, a dither tile its two
/// ports disagree about) or does not store (the destination coverage
/// count's two hidden bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectFragmentStages {
    alpha_compare: AlphaCompare,
    /// `G_SETBLENDCOLOR.a`, the `G_AC_THRESHOLD` comparand. Always a real
    /// byte: the register holds zero until the guest writes it, so a
    /// `Threshold` compare with no `SetBlendColor` tests `alpha >= 0`,
    /// which passes -- the reference lane's own behaviour
    /// (`raster/blend.rs:113` against the zero-initialized
    /// `other_mode.blend_color_alpha`).
    threshold_alpha: u8,
    alpha_dither: AlphaDither,
    rgb_dither: RgbDither,
    coverage_times_alpha: bool,
    alpha_coverage_select: bool,
    coverage_mode: CoverageModeBits,
}

impl TexrectFragmentStages {
    /// Resolve the four stages, refusing by name every mode outside the
    /// admitted set.
    ///
    /// **Admitted, and why each is exact:**
    ///
    /// - Alpha compare `None` (no gate) and `Threshold` (`alpha >=
    ///   G_SETBLENDCOLOR.a`, pure integer comparison).
    /// - Alpha dither `Disabled` and RGB dither `Disabled` -- both are the
    ///   identity on the working color, and `Disabled` RGB dither is
    ///   exactly the `>> 3` truncation [`write_pixel`] already performs
    ///   (`fn64-render-reference/src/backend/framebuffer_io.rs:117-122`:
    ///   "disabled dither remains exact `>> 3` truncation").
    /// - Coverage: `CVG_X_ALPHA`/`ALPHA_CVG_SEL` in every combination, and
    ///   `cvg_dst` in the subset that does not consult the destination
    ///   count.
    ///
    /// **Refused, each for a missing authority rather than missing work:**
    /// `G_AC_DITHER`, alpha dither `Noise`, RGB dither `Noise`
    /// ([`TexrectExecutionError::NoiseThresholdUnavailable`]); the ordered
    /// `Pattern`/`InversePattern`/`MagicSquare`/`Bayer` tiles
    /// ([`TexrectExecutionError::OrderedDitherAuthorityUnsettled`]); and every mode
    /// reading the destination coverage count
    /// ([`TexrectExecutionError::DestinationCoverageUnavailable`]).
    pub fn try_new(
        other_mode: OtherMode,
        blend_color: Color4,
    ) -> Result<Self, TexrectExecutionError> {
        let alpha_compare = other_mode.alpha_compare();
        match alpha_compare {
            // Pinned RT64 implements alpha compare only for `G_AC_DITHER`
            // and `G_AC_THRESHOLD`; encoding 2 falls through without a
            // compare
            // (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/RasterPS.hlsl:203-213`).
            // It therefore arrives here already decoded as `None`.
            // See `docs/RT64-GUARD-AUDIT.md` finding A3.
            AlphaCompare::None | AlphaCompare::Threshold => {}
            AlphaCompare::Dither => {
                return Err(TexrectExecutionError::NoiseThresholdUnavailable {
                    stage: TexrectNoiseStage::AlphaCompareDither,
                })
            }
        }
        // No refusal for a never-written `SetBlendColor` under
        // `Threshold`: the register holds its power-on zero, so the
        // comparand is 0 and every fragment passes. That is what the
        // reference lane computes and what RT64's zero-initialized
        // `blendColor` produces.

        let alpha_dither = other_mode.alpha_dither();
        let rgb_dither = other_mode.rgb_dither();
        match alpha_dither {
            AlphaDither::Disabled => {}
            // **Admitted at a named, bounded endpoint -- not refused, and
            // not an invented sequence.** See
            // [`NOISE_DITHER_THRESHOLD`]'s own doc for the proof that this
            // is one of the two values the mode can produce rather than a
            // third value between them.
            AlphaDither::Noise => {}
            // `Pattern`/`InversePattern` resolve to an ordered tile,
            // substituting `Bayer` when RGB dither is `Disabled` and
            // `MagicSquare` when it is `Noise` (`apply_alpha_dither`'s own
            // rule, ported at `alpha_compare.rs:203-231`).
            //
            // **MagicSquare is admitted; Bayer is refused.** The two are
            // not in the same evidential position, and the crate already
            // measured the difference: `rgb_dither.rs`'s
            // `magic_square_matches_reference_oracle_at_every_cell` proves
            // this crate's RT64 table and the reference's agree at all 16
            // cells, while
            // `bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`
            // pins 8 cells where they do not. Refusing an agreed table
            // would decline work no evidence disputes; admitting a
            // disputed one would pick a side.
            //
            // The tile this reads is `crate::rgb_dither`'s -- since
            // `51b4e184` there is exactly one, and `alpha_compare.rs`'s
            // former duplicate is gone. That is what makes the split by
            // table (rather than by stage) the right axis, and it is why
            // `docs/RT64-LANE-DIVERGENCES.md` D7's "the alpha stage already
            // agrees with the reference" argument no longer holds. See
            // `TexrectExecutionError::OrderedDitherAuthorityUnsettled`.
            AlphaDither::Pattern | AlphaDither::InversePattern => {
                let pattern = match rgb_dither {
                    RgbDither::MagicSquare | RgbDither::Bayer => rgb_dither,
                    RgbDither::Noise => RgbDither::MagicSquare,
                    RgbDither::Disabled => RgbDither::Bayer,
                };
                if matches!(pattern, RgbDither::Bayer) {
                    return Err(TexrectExecutionError::OrderedDitherAuthorityUnsettled {
                        stage: TexrectNoiseStage::AlphaDither,
                        pattern,
                    });
                }
            }
        }
        match rgb_dither {
            RgbDither::Disabled => {}
            // Same bounded endpoint as alpha dither's `Noise` arm above:
            // the reference's `apply_rgb_dither` bumps a channel iff
            // `(channel & 7) > threshold`, which at threshold 7 is never,
            // leaving the channel exactly as `Disabled` does.
            RgbDither::Noise => {}
            // **Both ordered RGB tiles are admitted only as a
            // pass-through, and the stage is declared NOT ported.** This
            // is the one of the four stages this card did not land, and
            // the boundary is stated rather than blurred.
            //
            // RGB dither has two ports in this workspace whose
            // *arithmetic* differs, not merely their tables: RT64's
            // `quantize_channel` computes `min(channel + threshold, 255)
            // >> 3` (`rgb_dither.rs`), while the reference's
            // `apply_rgb_dither` computes
            // `if (channel & 7) > threshold { (channel & !7) + 8 }`
            // (`raster/blend.rs:51-67`). Witness: channel 1 at threshold 0
            // gives 5-bit 0 under RT64 and 1 under the reference. Their
            // Bayer *tables* also disagree at 8 of 16 cells, already
            // pinned by `rgb_dither.rs`'s own
            // `bayer_matrix_disagrees_with_reference_oracle_at_documented_cells`.
            //
            // Refusing outright was measured and rejected: encoding 0 is
            // `MagicSquare`, the power-on default that this crate's own
            // composed fixtures latch, so a refusal declines packets that
            // execute correctly today. Applying either port's arithmetic
            // would pick a side no evidence settles. The stage therefore
            // runs as the identity, exactly as it did before this card,
            // and says so in this module's Nonclaims -- an unchanged
            // behaviour with a named frontier, not a silent approximation.
            RgbDither::MagicSquare | RgbDither::Bayer => {}
        }

        let coverage_mode = CoverageModeBits {
            image_read_enabled: other_mode.image_read_enabled(),
            force_blend: other_mode.force_blend(),
            antialias_enabled: other_mode.antialias_enabled(),
            coverage_destination: other_mode.coverage_destination(),
        };
        Ok(Self {
            alpha_compare,
            threshold_alpha: blend_color.rgba8()[3],
            alpha_dither,
            rgb_dither,
            coverage_times_alpha: other_mode.coverage_times_alpha(),
            alpha_coverage_select: other_mode.alpha_coverage_select(),
            coverage_mode,
        })
    }

    /// The coverage accumulation for one fragment, and whether the write
    /// proceeds at all.
    ///
    /// **The destination coverage count is only partly recoverable, and
    /// this function refuses exactly the cases where the missing part is
    /// observable.** RGBA16 stores the count as 3 bits, 2 of them in a
    /// hidden sidecar `fn64-render-wgpu` does not maintain (the oracle does,
    /// as `RdramHiddenBits`). What saves the common case is the pixel
    /// coverage: a texrect covers whole pixels, so `pixel.count() == 8`.
    ///
    /// Derived, not assumed. `Coverage::from_stored` is `(stored & 7) + 1`,
    /// so a stored destination count is always in `1..=8` -- never zero.
    /// With `pixel == 8` and image read enabled, `sum = 8 + memory >= 9`,
    /// which is `> 8` for **every** value the missing bits could hold.
    /// So `wraps` is `true` regardless, and every consumer of `wraps` --
    /// `blend_enabled`'s `!wraps` term and `CLR_ON_CVG`'s write gate -- is
    /// determined without them. Asserted over all eight possible memory
    /// counts in `wraps_is_determined_for_a_full_coverage_fragment`.
    ///
    /// The complete destination count remains unknown. For a full fragment,
    /// Wrap's visible stored bit is exact for either endpoint supplied by the
    /// RGBA16 decoder, while Clamp and Full store full coverage. Save is
    /// different: it preserves the genuine three-bit memory coverage, so an
    /// image-read Save cannot consume the endpoint substituted for RGBA16's
    /// two unavailable hidden bits and is refused by name. A partial fragment
    /// with image read could likewise make those bits affect wrap and is
    /// refused; texrects normally cannot produce one unless CVG_X_ALPHA
    /// reduces their full primitive coverage.
    fn coverage_for(
        self,
        pixel_coverage: Coverage,
        memory_coverage: Coverage,
    ) -> Result<crate::coverage::CoverageResult, TexrectExecutionError> {
        if self.coverage_mode.image_read_enabled {
            if matches!(
                self.coverage_mode.coverage_destination,
                crate::state::CoverageDestination::Save
            ) {
                return Err(TexrectExecutionError::DestinationCoverageUnavailable {
                    consumer: "cvg_dst = Save",
                });
            }
            if pixel_coverage.count() != Coverage::FULL.count() {
                return Err(TexrectExecutionError::DestinationCoverageUnavailable {
                    consumer: "a partial-coverage fragment's cvg_dst accumulation",
                });
            }
        }
        Ok(coverage_result(
            pixel_coverage,
            memory_coverage,
            self.coverage_mode,
        ))
    }
}

/// Admits exactly the blender states this executor can evaluate exactly,
/// refusing every other by name before any pixel is produced.
///
/// Three refusals, each for a term this executor genuinely cannot supply
/// rather than for one it has merely not written yet:
///
/// - **`FORCE_BL` clear with `AA_EN` set and `IM_RD` set.** The reference
///   derives `blend_enabled` as
///   `force_blend() || (antialias_enabled() && !wraps)`
///   (`fn64-render-reference/src/raster/coverage.rs:68-69`). `wraps` is
///   `image_read && pixel_coverage + memory_coverage > 8`, and this
///   executor maintains no coverage count on either side. Three of the four
///   `FORCE_BL`-clear cases settle exactly without any coverage count:
///   `AA_EN` clear makes the conjunction `false` outright, and — the second
///   narrowing — a clear `IM_RD` makes `wraps` `false` **by the reference's
///   own definition**, since `wraps` is a conjunction whose first term is
///   `image_read`. With `wraps` pinned `false`, `blend_enabled` reduces to
///   `antialias_enabled()`, which is `true`, and no coverage count is
///   consulted. Only `FORCE_BL` clear **and** `AA_EN` set **and** `IM_RD`
///   set leaves the value resting on the sum, and that one is refused
///   rather than blended under a guess.
/// - **`A = Shade`.** No vertex-interpolated color exists here, matching
///   the combiner's own `Shade` refusal.
/// - **`B = FramebufferAlpha`.** The destination coverage *count*, which
///   RGBA16's single stored bit cannot express.
///
/// Copy cycle is admitted unconditionally: [`BlendModeState::cycle_count`]
/// returns 0 for it and [`blend_fragment`] then returns the source
/// unchanged, which is the RDP's own blender bypass in that mode, not an
/// approximation.
pub(super) fn require_blendable_mode(state: BlendModeState) -> Result<(), TexrectExecutionError> {
    let cycle_count = state.cycle_count();
    if cycle_count == 0 {
        return Ok(());
    }
    // `blend_enabled = force_blend() || (antialias_enabled() && !wraps)`
    // with `wraps = image_read_enabled() && sum > 8`
    // (`fn64-render-reference/src/raster/coverage.rs:68-69`). Only one case
    // consults the sum: `force_blend()` short-circuits the disjunction to
    // `true`; a clear `AA_EN` short-circuits the conjunction to `false`;
    // and a clear `IM_RD` short-circuits `wraps` itself to `false`, leaving
    // `blend_enabled = antialias_enabled() = true` with no coverage count
    // read. Refusing every `FORCE_BL`-clear mode instead of only the one
    // that rests on the sum was measured wrong -- three composed one-cycle
    // fixtures latch other-mode low `0`, where `AA_EN` is clear and
    // `blend_enabled` is exactly `false` with no coverage count needed.
    if !state.other_mode.force_blend()
        && state.other_mode.antialias_enabled()
        && state.other_mode.image_read_enabled()
    {
        return Err(TexrectExecutionError::BlendEnabledNotDerivable);
    }
    for cycle_index in 0..cycle_count {
        let cycle = state.cycle(cycle_index);
        if matches!(cycle.a, crate::blend::BlendAlphaInput::Shade) {
            return Err(TexrectExecutionError::UnsupportedBlendShadeAlpha);
        }
        if cycle.requires_framebuffer_alpha() {
            return Err(TexrectExecutionError::UnsupportedBlendFramebufferAlpha);
        }
    }
    Ok(())
}

/// Reads one already-stored destination pixel back out of the target's own
/// pixel format into the RGBA8888 domain the blender consumes.
///
/// The exact inverse of [`write_pixel`], and deliberately written as its
/// mirror: RGBA16's three 5-bit channels expand through
/// [`crate::targets::fill::expand_five`]'s `(v << 3) | (v >> 2)` -- the same
/// expansion `decode_fill_cycle_pixel` already applies to a fill color, so a
/// pixel this executor wrote and a pixel the fill executor wrote decode
/// identically. RGBA16's single stored bit is **coverage**, not retained
/// pixel alpha (`fn64-render-reference/src/backend/framebuffer_io.rs:120-122`
/// states the same for the oracle's own packing), so it expands to a full or
/// empty coverage count rather than to an alpha byte.
///
/// The returned [`BlendFramebufferSample::rgba`]'s alpha channel is the
/// coverage-derived value the same bit encodes; this executor's admitted
/// modes never select [`crate::blend::BlendBInput::FramebufferAlpha`] (see
/// [`blend_texrect_fragment`]'s own refusal), so no admitted program reads
/// it as a blend term.
fn read_pixel(format: ColorTargetFormat, source: &[u8]) -> BlendFramebufferSample {
    match format {
        ColorTargetFormat::Rgba16 => {
            let packed = u16::from_be_bytes([source[0], source[1]]);
            let expand = |value: u16| -> u8 {
                let value = value as u8;
                (value << 3) | (value >> 2)
            };
            let coverage_bit = (packed & 1) as u8;
            BlendFramebufferSample {
                rgba: [
                    expand((packed >> 11) & 0x1f),
                    expand((packed >> 6) & 0x1f),
                    expand((packed >> 1) & 0x1f),
                    if coverage_bit != 0 { 255 } else { 0 },
                ],
                coverage_count: if coverage_bit != 0 { 8 } else { 0 },
            }
        }
        ColorTargetFormat::Rgba32 => BlendFramebufferSample {
            rgba: [source[0], source[1], source[2], source[3]],
            coverage_count: source[3] >> 5,
        },
    }
}

/// Runs the RDP blender over one combined fragment against the destination
/// pixel already stored in the target.
///
/// This is the stage this executor's header previously declared it did not
/// run. It is not a second blender: [`blend_fragment`] is `crate::blend`'s
/// already-landed literal port of the reference rasterizer's
/// `blend_fragment` (`crates/fn64-render-reference/src/raster/blend.rs:157-240`),
/// reached here rather than reimplemented, so a disagreement between this
/// executor and that port is unrepresentable rather than merely tested for.
///
/// **`blend_enabled` is the reference's own formula with `wraps` pinned
/// `false`, which is exact on every admitted mode.** The reference computes
/// it as
///
/// ```text
/// wraps         = image_read_enabled && pixel_coverage + memory_coverage > 8
/// blend_enabled = force_blend || (antialias_enabled && !wraps)
/// ```
///
/// (`raster/coverage.rs:68-69`), where the sum needs the destination
/// coverage *count* this executor does not maintain -- RGBA16 stores one
/// coverage bit, not the three the count needs, and the executor declares no
/// coverage stage. [`require_blendable_mode`] admits only the modes on which
/// the disjunction settles **without evaluating that sum**, and on each of
/// them `wraps` is provably `false`:
///
/// - `FORCE_BL` set: the disjunction is `true` regardless, so `wraps` is
///   never read.
/// - `AA_EN` clear: the conjunction is `false` regardless, so `wraps` is
///   never read.
/// - `FORCE_BL` clear, `AA_EN` set, `IM_RD` clear: `wraps` is a conjunction
///   whose first term is `image_read`, so it is `false` outright and
///   `blend_enabled` is `antialias_enabled()`, i.e. `true`.
///
/// Substituting `wraps = false` therefore reproduces the reference exactly
/// on the admitted set rather than approximating it, and the one mode where
/// that substitution would be a guess -- `FORCE_BL` clear, `AA_EN` set,
/// `IM_RD` set -- is refused as
/// [`TexrectExecutionError::BlendEnabledNotDerivable`] before any pixel is
/// produced. So no admitted fragment reaches the blender with a
/// `blend_enabled` this executor guessed.
///
/// Writing `force_blend()` alone here was correct only while the admitted
/// set was the first two bullets; the third makes `false || (true && !false)`
/// a case where the two expressions disagree, and the blender would have
/// been bypassed on a mode the RDP runs it for.
///
/// # Errors
/// [`TexrectExecutionError::Blend`] when the blender selects a framebuffer
/// term with `IM_RD` disabled -- propagated, never substituted.
fn blend_texrect_fragment(
    combined: [u8; 4],
    destination: BlendFramebufferSample,
    state: BlendModeState,
    column: u32,
    row: u32,
) -> Result<[u8; 4], TexrectExecutionError> {
    let memory = state.other_mode.image_read_enabled().then_some(destination);
    // `shade_alpha` is zero, and it is never read: a blender cycle whose `A`
    // selects `Shade` is refused by name before any pixel is produced (see
    // `require_blendable_mode`/`UnsupportedBlendShadeAlpha`).
    //
    // The refusal, not the absence of a value, is what makes the zero safe.
    // This mattered when the shaded raw-triangle executor landed: that
    // executor DOES interpolate a per-pixel shade colour, so the old reason
    // given here ("this executor has no vertex-interpolated color") stopped
    // being true for one of its two callers while the conclusion stayed
    // correct. Threading the triangle's real shade alpha through is the
    // right widening when a blender program that reads it is admitted; until
    // then the refusal above is the one that holds.
    // Exact on the admitted set, not a stand-in: see this function's own
    // doc. A hardcoded `true` would be wrong for the `AA_EN`-clear modes
    // `require_blendable_mode` admits, where the RDP bypasses the last
    // cycle; a hardcoded `force_blend()` would be wrong for the
    // `IM_RD`-clear ones, where the RDP runs it. This is the reference's
    // disjunction with `wraps` pinned `false` -- provably its exact value on
    // every mode `require_blendable_mode` lets through.
    let blend_enabled = state.other_mode.force_blend() || state.other_mode.antialias_enabled();
    let BlendedFragment { rgba } = blend_fragment(combined, memory, 0, state, blend_enabled)
        .map_err(|source| TexrectExecutionError::Blend {
            column,
            row,
            source,
        })?;
    Ok(rgba)
}

/// Blends one combined fragment against the destination pixel already
/// stored at `dest`, then writes the result back over it.
///
/// **The destination is read back out of the buffer being written**, not
/// out of the caller's `resident_bytes`, so a later pixel in the same
/// rectangle blends against an earlier one's result exactly as the RDP's
/// serial per-pixel pipeline does. Reading `resident_bytes` instead would
/// make an overlapping rectangle's self-composition invisible.
///
/// A named function rather than an inline block inside the pixel loop, for
/// the reason [`combine_one_texel`]'s own doc records: an inline stage is
/// unreachable from a unit test, so a mutation that drops it survives.
///
/// # Errors
/// [`TexrectExecutionError::Blend`], propagated from
/// [`blend_texrect_fragment`].
pub(super) fn blend_and_write_pixel(
    format: ColorTargetFormat,
    dest: &mut [u8],
    combined: [u8; 4],
    state: BlendModeState,
    stages: TexrectFragmentStages,
    column: u32,
    row: u32,
) -> Result<(), TexrectExecutionError> {
    // **Stage order is the RDP's, and it is observable.** Coverage-to-alpha
    // runs before alpha compare (so `ALPHA_CVG_SEL` can supply the value
    // the comparator tests), alpha compare gates before the alpha dither
    // that feeds the blender, and RGB dither is a memory-interface
    // perturbation applied after the blend. This mirrors the reference's
    // own `set_blended` (`fn64-render-reference/src/raster/draw.rs:596-630`)
    // and its `draw_combined_fill_rectangle` caller (`:243-263`), which
    // applies coverage-alpha and alpha compare before calling it.
    //
    // A texrect's fragment coverage is `Coverage::FULL`: a rectangle covers
    // whole pixels, so no edge produces a partial mask. This executor
    // rasterizes no edges and computes no subpixel mask, which is why that
    // is a fact about the primitive rather than an assumption.
    let (combined, pixel_coverage) = apply_coverage_alpha(
        stages.coverage_times_alpha,
        stages.alpha_coverage_select,
        combined,
        Coverage::FULL,
    );

    // Zero coverage writes nothing, and `CLR_ON_CVG` without a wrap writes
    // nothing -- both are the reference's own early returns, not this
    // executor declining to draw.
    if pixel_coverage.count() == 0 {
        return Ok(());
    }
    let memory_coverage = match format {
        // RGBA16 exposes only stored coverage bit 2. The missing low bits
        // cannot affect the admitted full-fragment Clamp or Wrap result. Use
        // an endpoint with the same visible bit; image-read Save and partial
        // fragments with IM_RD are refused below.
        ColorTargetFormat::Rgba16 => {
            if dest[1] & 1 == 0 {
                Coverage::new(1)
            } else {
                Coverage::FULL
            }
        }
        // RGBA32 keeps the complete three-bit stored coverage value in the
        // high bits of byte three.
        ColorTargetFormat::Rgba32 => Coverage::from_stored(dest[3] >> 5),
    };
    let coverage = stages.coverage_for(pixel_coverage, memory_coverage)?;
    if !alpha_compare_texrect_fragment(stages, combined[3])? {
        return Ok(());
    }
    // `CLR_ON_CVG` (color-on-coverage) gates the color write on the coverage
    // *carry-out*, not on `coverage.wraps`. The two differ only when
    // `IM_RD` is clear. With `IM_RD` set, no memory coverage is read and
    // angrylion's `prewrap = (curpixel_memcvg + curpixel_cvg) & 8`
    // (`angrylion-rdp-plus src/core/n64video/rdp/zbuffer.c` `z_compare`)
    // carries out exactly when `coverage.wraps`
    // (`image_read_enabled && pixel + memory > 8`) does for the full-pixel
    // fragments this executor produces -- so that path is preserved
    // verbatim, including the `gen-coverage-*-combined`/`force-blend`
    // shared-ported-bug rows that must stay as-is.
    //
    // With `IM_RD` clear, angrylion's no-read `fbread` returns
    // `memcvg = 0`, so `prewrap = curpixel_cvg & 8`: a FULL-coverage
    // fragment (`pixel = 8`) carries out and IS written under `CLR_ON_CVG`.
    // The reference's `wraps` short-circuits to `false` here (its
    // `image_read_enabled &&` guard), which wrongly DROPPED the write --
    // the `gen-coverage-color-on-cvg-one-cycle` defect (12/12 pixels left
    // STALE, vs angrylion + RT64 both writing). `color_on_cvg` never gates
    // the color write itself in `blender_1cycle`; it only selects the
    // color source (`!color_on_cvg || prewrap`), and the write is gated by
    // the coverage bit alone, which a full-coverage fragment always sets.
    let coverage_carry = if state.other_mode.image_read_enabled() {
        coverage.wraps
    } else {
        pixel_coverage.count() & 8 != 0
    };
    if state.other_mode.clear_on_coverage() && !coverage_carry {
        return Ok(());
    }

    // Alpha dither, admitted only as `Disabled` -- the identity. Routed
    // through `apply_alpha_dither` rather than skipped, so the stage is
    // present and a future admission widens the match arm instead of
    // adding a call.
    let mut combined = combined;
    combined[3] = apply_alpha_dither(
        combined[3],
        stages.alpha_dither,
        stages.rgb_dither,
        column as i32,
        row as i32,
        // Read only by the `Noise` arm, and then as the proven endpoint --
        // see [`NOISE_DITHER_THRESHOLD`]. `Disabled` returns early without
        // consulting it; the ordered arms were refused in `try_new`.
        NOISE_DITHER_THRESHOLD,
    );

    let destination = read_pixel(format, dest);
    let blended = blend_texrect_fragment(combined, destination, state, column, row)?;
    // RGB dither, admitted only as `Disabled`. The reference's
    // `apply_rgb_dither` returns its input unchanged in that mode
    // (`raster/blend.rs:59`), and the `>> 3` truncation that follows is
    // `write_pixel`'s own packing -- so this is the whole of the stage on
    // the admitted set, not a partial one.
    // No call: RGB dither is the one stage this card did NOT port, and it
    // runs as the identity in every mode (see `try_new`'s own arm). The
    // field is retained on `TexrectFragmentStages` because
    // `apply_alpha_dither` reads it for the `Pattern` substitution rule
    // above -- it is a real input to a stage that IS ported, not a
    // placeholder for this one.
    let _ = stages.rgb_dither;
    write_pixel(format, dest, blended, coverage.destination);
    Ok(())
}

/// The alpha-compare gate for one fragment: `true` writes, `false` is a
/// silent-by-design non-write (the RDP's own behaviour, not a refusal).
///
/// Reached only for modes [`TexrectFragmentStages::try_new`] admitted, so
/// the noise byte is never consulted.
///
/// # Errors
/// [`TexrectExecutionError::NoiseThresholdUnavailable`] for `Dither`, which
/// [`TexrectFragmentStages::try_new`] already refused; kept here so the invariant holds at the point it is relied on
/// rather than only where it was checked. `Threshold` needs no error of its
/// own -- `G_SETBLENDCOLOR.a` is always a real byte.
fn alpha_compare_texrect_fragment(
    stages: TexrectFragmentStages,
    alpha: u8,
) -> Result<bool, TexrectExecutionError> {
    let threshold_alpha = match stages.alpha_compare {
        AlphaCompare::None => 0,
        AlphaCompare::Threshold => stages.threshold_alpha,
        AlphaCompare::Dither => {
            return Err(TexrectExecutionError::NoiseThresholdUnavailable {
                stage: TexrectNoiseStage::AlphaCompareDither,
            })
        }
    };
    Ok(alpha_compare_value(
        stages.alpha_compare,
        alpha,
        threshold_alpha,
        // Unreachable: only `None`/`Threshold` reach here, neither of which
        // reads the noise byte.
        AlphaCompareNoise(0),
    ))
}

/// Packs one decoded RGBA8888 texel into the target's own pixel format.
///
/// Programming Manual §§15.5.3, 15.5.6, and 15.7 define RGBA16 bit 0 as
/// stored coverage bit 2, not primitive alpha. `coverage` is the post-
/// `CVG_DST_CLAMP/WRAP/FULL/SAVE` destination count; `Coverage::stored()`
/// converts it to the documented three-bit `count - 1` representation.
/// RT64 independently uses the same encoding in `Float4ToRGBA16`.
fn write_pixel(
    format: ColorTargetFormat,
    dest: &mut [u8],
    rgba: [u8; 4],
    coverage: Coverage,
) {
    let [red, green, blue, alpha] = rgba;
    match format {
        ColorTargetFormat::Rgba16 => {
            let packed = (u16::from(red >> 3) << 11)
                | (u16::from(green >> 3) << 6)
                | (u16::from(blue >> 3) << 1)
                | u16::from((coverage.stored() >> 2) & 1);
            dest.copy_from_slice(&packed.to_be_bytes());
        }
        ColorTargetFormat::Rgba32 => {
            dest.copy_from_slice(&[red, green, blue, alpha]);
        }
    }
}

#[cfg(test)]
mod one_cycle_tests {
    use super::*;

    /// The two combiner programs `docs/RT64-WM2000-CYCLE-MODES.md` §2
    /// measured across all 2,520 of WM2000's texrects, packed into their
    /// `SetCombine` wire words.
    ///
    /// The packing is hand-derived from `CombineParams`' own
    /// `parse_color_*`/`parse_alpha_*` **second-cycle** bit positions
    /// (`combiner.rs:189-250`), which is the slice one-cycle mode reads:
    /// color A `low >> 5 & 0xF`, B `high >> 24 & 0xF`, C `low & 0x1F`,
    /// D `high >> 6 & 0x7`; alpha A `high >> 21 & 0x7`, B `high >> 3 & 0x7`,
    /// C `high >> 18 & 0x7`, D `high & 0x7`. Every field occupies disjoint
    /// bits in its word, which `wire_programs_decode_to_the_measured_selectors`
    /// below asserts by decoding rather than by inspection.
    fn pack_second_cycle(color: [u32; 4], alpha: [u32; 4]) -> CombineParams {
        let [ca, cb, cc, cd] = color;
        let [aa, ab, ac, ad] = alpha;
        let low = (ca << 5) | cc;
        let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
        CombineParams::from_wire(low, high)
    }

    /// Program 1 (2,100 of 2,520 texrects): RGB
    /// `(Environment - Texel0) * Primitive + Texel0`, Alpha
    /// `(Texel0 - Zero) * Primitive + Zero`.
    ///
    /// Indices: `Environment = 5` and `Texel0 = 1` from the shared
    /// `colorInputCommon` table, `Primitive = 3` likewise; alpha `Zero` is
    /// index 7 in both `alphaInputABD` and `alphaInputC`.
    fn env_lerp_program() -> CombineParams {
        pack_second_cycle([5, 1, 3, 1], [1, 7, 3, 7])
    }

    /// [`pack_second_cycle`]'s first-cycle twin, hand-derived from the same
    /// `parse_color_*`/`parse_alpha_*` functions at `second_cycle = false`
    /// (`combiner.rs:189-250`): color A `low >> 20 & 0xF`,
    /// B `high >> 28 & 0xF`, C `low >> 15 & 0x1F`, D `high >> 15 & 0x7`;
    /// alpha A `low >> 12 & 0x7`, B `high >> 12 & 0x7`, C `low >> 9 & 0x7`,
    /// D `high >> 9 & 0x7`.
    ///
    /// Every one of those fields is disjoint from every field
    /// [`pack_second_cycle`] writes, which is what lets the two be OR'd into
    /// one `CombineParams` to build a genuine two-cycle program.
    /// `two_cycle_wire_program_decodes_to_both_slices` asserts that by
    /// decoding rather than by inspection.
    fn pack_first_cycle(color: [u32; 4], alpha: [u32; 4]) -> CombineParams {
        let [ca, cb, cc, cd] = color;
        let [aa, ab, ac, ad] = alpha;
        let low = (ca << 20) | (cc << 15) | (aa << 12) | (ac << 9);
        let high = (cb << 28) | (cd << 15) | (ab << 12) | (ad << 9);
        CombineParams::from_wire(low, high)
    }

    /// Merges a first-cycle and a second-cycle packing into one program.
    fn merge_cycles(first: CombineParams, second: CombineParams) -> CombineParams {
        CombineParams::from_wire(first.low() | second.low(), first.high() | second.high())
    }

    /// A two-cycle program whose two cycles cannot be collapsed into one.
    ///
    /// **Cycle 0** (RGB and alpha alike): `(Zero - Zero) * Zero + Primitive`
    /// -- the accumulator becomes the primitive colour. Slot indices are
    /// each slot's own out-of-table `Zero` (`A = 8`, `B = 8`, `C = 16`,
    /// alpha `= 7`) with `D = 3` (`Primitive` in `colorInputD` and
    /// `alphaInputABD` alike), exactly as [`flat_primitive_program`] does
    /// for the second slice.
    ///
    /// **Cycle 1** (RGB and alpha alike): `(Zero - Zero) * Zero + Combined`
    /// -- `D = 0`, which is `Combined` in `colorInputD` and
    /// `alphaInputABD`. So cycle 1 emits, verbatim, whatever cycle 0 put in
    /// the accumulator.
    ///
    /// Under two-cycle evaluation the result is the primitive colour.
    /// Under one-cycle evaluation **only the second slice runs**, against
    /// the zero-initialized accumulator, so `D = Combined` resolves to zero
    /// and the result is transparent black. The two answers differ in all
    /// four channels, which is the point: no reading of the second slice
    /// alone can produce the two-cycle answer.
    ///
    /// Deliberately `Texel0`-free in both slices. This executor binds one
    /// tile, so `Texel1` is refused (the reference lane refuses it for the
    /// same reason), and a program needing a second texel would prove
    /// nothing about the carry.
    fn carry_program() -> CombineParams {
        merge_cycles(
            pack_first_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
            pack_second_cycle([8, 8, 16, 0], [7, 7, 7, 0]),
        )
    }

    /// Program 2 (420 of 2,520): RGB and Alpha both
    /// `(Zero - Zero) * Zero + Primitive`.
    ///
    /// Each slot's `Zero` index is that slot's OWN out-of-table value, not
    /// a shared constant -- `IDX_COLOR_ZERO_A = 8`, `_B = 8`, `_C = 16`
    /// (its field is 5 bits wide), alpha `Zero = 7`. Using one index for
    /// all four would decode to `NOISE`/`K4`/`K5` in the slots whose
    /// tables define index 7.
    fn flat_primitive_program() -> CombineParams {
        pack_second_cycle([8, 8, 16, 3], [7, 7, 7, 3])
    }

    const ENV_WIRE: u32 = 0xFF00_80FF;
    const PRIM_WIRE: u32 = 0x80FF_4080;
    /// `SetPrimColor`'s `w0`: `lod_frac` in bits 0:7, `lod_min` in 8:12.
    /// Neither program reads either, so the value is deliberately non-zero
    /// -- if `prim_lod_frac` ever leaked into a channel, this catches it.
    const PRIM_LOD_W0: u32 = 0x0540;

    fn measured_shading(combine: CombineParams) -> TexrectShading {
        TexrectShading::new(
            combine,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_one_cycle()
        .expect("both measured programs read only admitted selectors")
    }

    /// Calls [`combine_one_texel`] -- **the executor's own** per-texel
    /// function, not a copy of it.
    ///
    /// This was a duplicate in the first draft, on the reasoning that a
    /// shared helper makes agreement structural rather than tested. That
    /// reasoning was wrong in the direction that matters: the duplicate put
    /// the executor's real arithmetic out of every unit test's reach, and a
    /// truncation mutant inside it survived the whole suite. Sharing the
    /// function is what makes these tests able to kill it. What proves the
    /// executor actually *calls* it is the composed and end-to-end tests,
    /// which is the right place for that claim.
    fn combine_texel(shading: TexrectShading, texel: [u8; 4]) -> [u8; 4] {
        combine_one_texel(
            shading.combine(),
            shading.base_inputs(),
            texel,
            TexrectCombinerEvaluation::OneCycle,
        )
    }

    /// **Positive control for every assertion below**: the two wire words
    /// really do decode to the measured programs.
    ///
    /// Without this, a packing slip would silently substitute a different
    /// program and every hand-derived expectation below would be checking
    /// arithmetic nobody measured. Asserted through
    /// `CombineParams::decode_color`/`decode_alpha` at `second_cycle =
    /// true`, the exact call `TexrectShading::try_new` and `run_one_cycle`
    /// both make.
    #[test]
    fn wire_programs_decode_to_the_measured_selectors() {
        let lerp = env_lerp_program();
        assert_eq!(
            [
                lerp.decode_color(ColorInputSlot::A, true),
                lerp.decode_color(ColorInputSlot::B, true),
                lerp.decode_color(ColorInputSlot::C, true),
                lerp.decode_color(ColorInputSlot::D, true),
            ],
            [
                ColorInput::Environment,
                ColorInput::Texel0,
                ColorInput::Primitive,
                ColorInput::Texel0,
            ],
            "program 1's RGB must be (Environment - Texel0) * Primitive + Texel0"
        );
        assert_eq!(
            [
                lerp.decode_alpha(AlphaInputSlot::A, true),
                lerp.decode_alpha(AlphaInputSlot::B, true),
                lerp.decode_alpha(AlphaInputSlot::C, true),
                lerp.decode_alpha(AlphaInputSlot::D, true),
            ],
            [
                AlphaInput::Texel0,
                AlphaInput::Zero,
                AlphaInput::Primitive,
                AlphaInput::Zero,
            ],
            "program 1's alpha must be (Texel0 - Zero) * Primitive + Zero"
        );

        let flat = flat_primitive_program();
        assert_eq!(
            [
                flat.decode_color(ColorInputSlot::A, true),
                flat.decode_color(ColorInputSlot::B, true),
                flat.decode_color(ColorInputSlot::C, true),
                flat.decode_color(ColorInputSlot::D, true),
            ],
            [
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Primitive,
            ],
            "program 2's RGB must be (Zero - Zero) * Zero + Primitive"
        );
        assert_eq!(
            [
                flat.decode_alpha(AlphaInputSlot::A, true),
                flat.decode_alpha(AlphaInputSlot::B, true),
                flat.decode_alpha(AlphaInputSlot::C, true),
                flat.decode_alpha(AlphaInputSlot::D, true),
            ],
            [
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Primitive,
            ],
            "program 2's alpha must be (Zero - Zero) * Zero + Primitive"
        );
        // The two programs must not be the same wire value, or every
        // "different program, different pixel" assertion is vacuous.
        assert_ne!(
            (lerp.low(), lerp.high()),
            (flat.low(), flat.high()),
            "the two measured programs must be distinct wire words"
        );
    }

    /// **Program 1's arithmetic, hand-derived per channel and reconciled
    /// against a second derivation of the same value.**
    ///
    /// Inputs: texel `(0x18, 0x40, 0xC8, 0xFF)`, env `0xFF0080FF` ->
    /// `(255, 0, 128, 255)`, prim `0x80FF4080` -> `(128, 255, 64, 128)`.
    ///
    /// Derivation 1, per channel, in the `(A - B) * C + D` order RT64
    /// evaluates (`run_one_cycle`'s own expression, not an algebraically
    /// rearranged one -- `A*C - B*C + D` is equal in exact arithmetic and
    /// NOT bit-identical in f32):
    ///
    /// ```text
    /// R: (255/255 - 24/255) * (128/255) + 24/255
    /// G: (  0/255 - 64/255) * (255/255) + 64/255
    /// B: (128/255 - 200/255) * ( 64/255) + 200/255
    /// A: (255/255 -       0) * (128/255) +       0
    /// ```
    ///
    /// Derivation 2, independent of the first: G's `C` is exactly `1.0`, so
    /// G reduces algebraically to `A - B + B = A = 0`. B's operand
    /// `(128 - 200)/255` is negative, so B must fall BELOW its `D` addend
    /// of `200/255 ~ 0.784` -- `0.713` does. A's `B` and `D` are both
    /// `Zero`, so alpha reduces to `texel.a * prim.a = 1.0 * 128/255 =
    /// 0.502`, and `0.502 * 255` rounds to exactly `128` -- the primitive
    /// alpha byte returned unchanged, which is the sharpest possible check
    /// that the `* 255.0` quantization is not off by one.
    ///
    /// The green channel is the load-bearing one: it is `0` only because
    /// the `+ Texel0` addend cancels the `- Texel0` subtrahend at `C = 1`.
    /// Dropping the `D` addend gives `-64/255`, which `wrap_clamp` pins to
    /// `0` -- so green ALONE cannot catch a dropped addend, and red and
    /// blue are what do.
    #[test]
    fn program_one_env_lerp_produces_hand_derived_bytes() {
        let shading = measured_shading(env_lerp_program());
        let observed = combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]);
        assert_eq!(
            observed,
            [140, 0, 182, 128],
            "program 1 must produce the hand-derived RGBA8888"
        );

        // Derivation 2, recomputed here in the target precision (f32, not
        // f64) so a Python-style f64 model cannot hide a rounding lane.
        let n = |byte: u8| f32::from(byte) / 255.0;
        let red = (n(0xFF) - n(0x18)) * n(0x80) + n(0x18);
        let green = (n(0x00) - n(0x40)) * n(0xFF) + n(0x40);
        let blue = (n(0x80) - n(0xC8)) * n(0x40) + n(0xC8);
        let alpha = (n(0xFF) - 0.0) * n(0x80) + 0.0;
        assert_eq!(
            [red, green, blue, alpha].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
            observed,
            "the second, independently written derivation must reconcile with the first"
        );
        // Green really is exactly zero, not merely small -- the `C = 1`
        // cancellation, pinned.
        assert_eq!(
            green, 0.0,
            "green's C is exactly ONE, so A - B + B cancels to A = 0"
        );
        // And alpha really is the primitive alpha byte round-tripped.
        assert_eq!(
            observed[3], 0x80,
            "alpha is texel.a * prim.a with texel.a = 1.0"
        );
    }

    /// **Program 2's arithmetic: `(Zero - Zero) * Zero + Primitive` is
    /// exactly the primitive color, every channel, texel-independent.**
    ///
    /// Hand-derived twice. Derivation 1: `(0 - 0) * 0 = 0`, so the result
    /// is the `D` addend, which is `Primitive` in all four channels ->
    /// `0x80FF4080` -> `(128, 255, 64, 128)`. Derivation 2, independent:
    /// the byte values must be `Color4::normalized`'s `/ 255.0` followed by
    /// `* 255.0` and `round`, which is the identity on every byte because
    /// `f32` represents `b / 255.0 * 255.0` exactly enough that no byte
    /// moves -- asserted for the full `0..=255` sweep below rather than for
    /// these four values alone.
    ///
    /// The texel-independence is asserted, not assumed: the same program
    /// against three unrelated texels must give one answer. That is what
    /// distinguishes "the combiner ran program 2" from "the combiner was
    /// bypassed and wrote the texel", which is mutant (a).
    #[test]
    fn program_two_flat_primitive_ignores_the_texel_entirely() {
        let shading = measured_shading(flat_primitive_program());
        let expected = [0x80, 0xFF, 0x40, 0x80];
        for texel in [
            [0x00, 0x00, 0x00, 0x00],
            [0x18, 0x40, 0xC8, 0xFF],
            [0xFF, 0xFF, 0xFF, 0xFF],
        ] {
            assert_eq!(
                combine_texel(shading, texel),
                expected,
                "program 2 must be the primitive color regardless of texel {texel:?}"
            );
        }
        // Derivation 2's round-trip claim, swept exhaustively rather than
        // spot-checked: `b / 255.0 * 255.0` rounds back to `b` for every
        // byte, so "the primitive color unchanged" is a real claim about
        // the quantization and not an accident of these four values.
        for byte in 0u8..=255 {
            let round_tripped = ((f32::from(byte) / 255.0) * 255.0).round() as u8;
            assert_eq!(
                round_tripped, byte,
                "byte {byte} must survive the normalize/quantize pair"
            );
        }
    }

    /// **The two programs disagree on the same texel** -- so a test that
    /// applied the wrong program to the wrong entries (mutant (e)) cannot
    /// pass, and neither can one that ignores the program entirely.
    #[test]
    fn the_two_measured_programs_disagree_on_one_texel() {
        let texel = [0x18, 0x40, 0xC8, 0xFF];
        assert_ne!(
            combine_texel(measured_shading(env_lerp_program()), texel),
            combine_texel(measured_shading(flat_primitive_program()), texel),
            "the env-lerp and flat-primitive programs must produce different pixels for the \
             same texel, or applying one where the other belongs is undetectable"
        );
        // And neither equals the raw texel, so bypassing the combiner
        // (mutant (a)) is detectable by either program.
        for (label, shading) in [
            ("env-lerp", measured_shading(env_lerp_program())),
            ("flat-primitive", measured_shading(flat_primitive_program())),
        ] {
            assert_ne!(
                combine_texel(shading, texel),
                texel,
                "{label} must not reproduce the raw texel, or bypassing the combiner is \
                 indistinguishable from running it"
            );
        }
    }

    /// **Primitive and Environment are not interchangeable** -- mutant (b).
    ///
    /// Swapping the two registers must change program 1's output. Asserted
    /// by evaluating the same program with the two wire words exchanged,
    /// which is exactly what a swapped plumbing would do at the call site.
    #[test]
    fn swapping_primitive_and_environment_changes_the_pixel() {
        let texel = [0x18, 0x40, 0xC8, 0xFF];
        let straight = combine_texel(measured_shading(env_lerp_program()), texel);
        let swapped = TexrectShading::new(
            env_lerp_program(),
            Color4::from_wire(PRIM_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, ENV_WIRE),
        )
        .validate_one_cycle()
        .expect("the swapped registers are still admitted selectors");
        assert_ne!(
            combine_texel(swapped, texel),
            straight,
            "exchanging the Primitive and Environment wire words must change program 1's \
             output, or the two are plumbed interchangeably"
        );
    }

    /// **Dropping the `+ Texel0` addend changes the pixel** -- mutant (c),
    /// expressed as the program that differs by exactly that term.
    ///
    /// `(Environment - Texel0) * Primitive + Zero` is program 1 with `D`
    /// changed from `Texel0` to `Zero`; its output must differ, and on the
    /// red and blue channels specifically (green's `C = 1` makes the
    /// clamped result `0` either way -- documented in
    /// `program_one_env_lerp_produces_hand_derived_bytes`).
    #[test]
    fn dropping_the_texel_addend_changes_the_pixel() {
        let texel = [0x18, 0x40, 0xC8, 0xFF];
        let with_addend = combine_texel(measured_shading(env_lerp_program()), texel);
        // `colorInputD`'s ZERO index is 7 -- its 3-bit table's only
        // out-of-range value.
        let without = pack_second_cycle([5, 1, 3, 7], [1, 7, 3, 7]);
        let observed = combine_texel(measured_shading(without), texel);
        assert_ne!(
            observed, with_addend,
            "the `+ Texel0` addend must be load bearing"
        );
        assert_ne!(
            observed[0], with_addend[0],
            "red must differ without the addend"
        );
        assert_ne!(
            observed[2], with_addend[2],
            "blue must differ without the addend"
        );
    }

    /// **Clamping happens in float, before quantization, and the wrap step
    /// runs before the clamp** -- mutant (d).
    ///
    /// Color slot C's table has no `ONE` entry at all (`colorInputC` maps
    /// index 6 to `KEY_SCALE`), so an over-range color result is reached
    /// through a `PRIMITIVE` register set to `0xFFFFFFFF` instead -- a real
    /// register at exactly `1.0`, not a synthetic constant.
    ///
    /// **The over-range case.** `(One - Zero) * Primitive(1.0) + One`
    /// evaluates to `2.0`. `wrap_clamp` sees `2.0 >= 1.5 + 1/255`, subtracts
    /// the `2.0 + 2/255` range to get `~-0.008`, and the final
    /// `clamp(0, 1)` pins that to **`0.0` -> byte 0**. A naive
    /// clamp-without-wrap would give `1.0` -> byte 255, and a
    /// quantize-then-clamp order would compute `2.0 * 255 = 510` and
    /// saturate to 255 as well. So byte `0` separates RT64's actual
    /// wrap-then-clamp-then-quantize order from BOTH of the plausible
    /// wrong orders, by the full channel range.
    ///
    /// Hand-derived twice: (1) `2.0 - (1.5 + 1/255 - (-0.5 - 1/255)) =
    /// 2.0 - 2.00784 = -0.00784`, clamped to `0.0`; (2) the wrap range is
    /// exactly `2 + 2/255`, and `2.0` is `2/255` below it, so the wrapped
    /// value is `-2/255 ~ -0.00784`. Same. Both are computed in `f32`
    /// below, not in `f64`.
    ///
    /// **The negative case.** `(Zero - One) * Primitive(1.0) + Zero` is
    /// `-1.0`; the wrap step fires (`-1.0 <= -0.5 - 1/255`), adding the
    /// range to give `~1.008`, clamped to `1.0` -> byte **255**. A
    /// quantize-first order would saturate `-255.0` to byte `0`. Again the
    /// two orders disagree by the full range.
    #[test]
    fn wrap_clamp_runs_before_quantization() {
        let one_register = PrimColor::from_wire(0, 0xFFFF_FFFF);
        // Color: A = ONE (index 6 in `colorInputA`), B = ZERO (8),
        // C = PRIMITIVE (3), D = ONE (6 in `colorInputD`).
        // Alpha: A = ONE (6), B = ZERO (7), C = PRIMITIVE (3), D = ONE (6).
        let over = pack_second_cycle([6, 8, 3, 6], [6, 7, 3, 6]);
        let shading = TexrectShading::new(over, Color4::from_wire(0), one_register)
            .validate_one_cycle()
            .expect("ONE/ZERO/PRIMITIVE are all admitted selectors");
        assert_eq!(
            combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]),
            [0, 0, 0, 0],
            "(One - Zero) * Primitive(1.0) + One is 2.0; wrap_clamp wraps it to ~-0.008 and              clamps to 0.0 -> byte 0. A clamp-only or a quantize-first order gives 255."
        );

        // Derivation 2, in f32: the wrap range and the wrapped value.
        let rounding = 1.0f32 / 255.0;
        let low = -0.5f32 - rounding;
        let high = 1.5f32 + rounding;
        let wrapped = 2.0f32 - (high - low);
        assert!(
            wrapped < 0.0,
            "2.0 must wrap BELOW zero, which is what makes the clamped answer 0 and not 1:              got {wrapped}"
        );
        assert_eq!(
            (wrapped.clamp(0.0, 1.0) * 255.0).round() as u8,
            0,
            "the independently computed wrap must reconcile with the observed byte"
        );

        // The negative case, the other direction.
        // Color: A = ZERO (8), B = ONE (6 in `colorInputB`? no -- B's 6 is
        // KEY_CENTER), so the subtrahend ONE comes from B index... none.
        // Reached instead through B = PRIMITIVE (3) at 1.0.
        let negative = pack_second_cycle([8, 3, 3, 7], [7, 3, 3, 7]);
        let shading = TexrectShading::new(negative, Color4::from_wire(0), one_register)
            .validate_one_cycle()
            .expect("ZERO/PRIMITIVE are admitted selectors");
        assert_eq!(
            combine_texel(shading, [0x18, 0x40, 0xC8, 0xFF]),
            [255, 255, 255, 255],
            "(Zero - Primitive(1.0)) * Primitive(1.0) + Zero is -1.0; wrap_clamp wraps it to              ~1.008 and clamps to 1.0 -> byte 255. A quantize-first order saturates to 0."
        );
        let wrapped_negative = -1.0f32 + (high - low);
        assert!(
            wrapped_negative > 1.0,
            "-1.0 must wrap ABOVE one, which is what makes the clamped answer 255 and not 0:              got {wrapped_negative}"
        );
    }

    /// **The executor evaluates the LATCHED program, not a fixed one** --
    /// mutant (e), and this test exists because that mutant SURVIVED its
    /// first draft.
    ///
    /// Replacing `shading.combine()` inside the pixel loop with a hardcoded
    /// flat-primitive program left the whole suite green. The reason is a
    /// reach gap, not an equivalence: the only *executed* one-cycle fixture
    /// runs the flat-primitive program itself (the env-lerp one is blocked
    /// by the GPU-path defect
    /// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_tmem_projection`
    /// pins), so substituting that exact program for the latched one is a
    /// no-op there, and every other assertion reached the arithmetic
    /// through the test helper.
    ///
    /// This test closes it at the executor's own function: the same texel
    /// and the same registers, through [`combine_one_texel`], must give
    /// different bytes for the two measured programs. A hardcoded program
    /// makes them equal.
    #[test]
    fn combine_one_texel_consults_the_program_it_is_given() {
        let texel = [0x18, 0x40, 0xC8, 0xFF];
        let base = measured_shading(env_lerp_program()).base_inputs();
        let lerp = combine_one_texel(
            env_lerp_program(),
            base,
            texel,
            TexrectCombinerEvaluation::OneCycle,
        );
        let flat = combine_one_texel(
            flat_primitive_program(),
            base,
            texel,
            TexrectCombinerEvaluation::OneCycle,
        );
        assert_ne!(
            lerp, flat,
            "the same texel and the same registers must combine differently under the two \
             measured programs, or the executor is not consulting the program it is handed"
        );
        // And each is the value its own program's hand derivation gives, so
        // "they differ" is not satisfied by two equally wrong answers.
        assert_eq!(
            lerp,
            [140, 0, 182, 128],
            "the env-lerp program's hand-derived bytes"
        );
        assert_eq!(
            flat,
            [0x80, 0xFF, 0x40, 0x80],
            "the flat program's primitive colour"
        );
    }

    /// **The quantization is round-half-away-from-zero, not truncation** --
    /// mutant (d), and this test exists because the first draft's mutant
    /// SURVIVED.
    ///
    /// # Why it survived, and what that revealed
    ///
    /// Replacing `(channel * 255.0).round() as u8` with a truncating
    /// `(channel * 255.0) as u8` left the whole suite green. Two reasons,
    /// both real:
    ///
    /// 1. Every other assertion in this module reached the arithmetic
    ///    through this module's own `combine_texel` helper, which duplicates
    ///    the quantization rather than calling the executor -- so a mutation
    ///    inside the executor's pixel loop was out of the helper's reach.
    /// 2. The executed fixtures write an **RGBA16** target, whose
    ///    `write_pixel` truncates each colour channel by `>> 3`. That
    ///    absorbs a one-count difference in the 8-bit intermediate unless
    ///    the two values straddle a multiple of 8. For the env-lerp
    ///    program's own bytes they do not: `139.95` truncates to `139` and
    ///    rounds to `140`, and `139 >> 3 == 140 >> 3 == 17`.
    ///
    /// # The witness, found by search rather than guessed
    ///
    /// `(Environment(0) - Texel0(16)) * Primitive(128) + Texel0(16)`
    /// evaluates to `7.96862745...` in f32 after `* 255.0`. Truncation
    /// gives `7`; round-half-away-from-zero gives `8`. `7 >> 3 == 0` and
    /// `8 >> 3 == 1`, so the two **do** straddle a multiple of eight and
    /// the difference survives the RGBA16 pack.
    ///
    /// A spot-check on the env-lerp program's own bytes would have
    /// supported the truncating form. This is the same lesson
    /// `RT64-PORT-CARD-BRIEF.md` §3.4 records: the witness had to be
    /// searched for, not assumed.
    ///
    /// Hand-derived twice: (1) `(0 - 16/255) * (128/255) + 16/255 =
    /// (16/255)(1 - 128/255) = (16 * 127) / 255^2 = 2032/65025 =
    /// 0.031249...`, times 255 is `7.9686...`; (2) computed in `f32` below
    /// and asserted to land strictly between 7 and 8, which is what makes
    /// the two roundings differ at all.
    #[test]
    fn the_quantization_rounds_rather_than_truncating() {
        let program = pack_second_cycle([5, 1, 3, 1], [1, 7, 3, 7]);
        let shading = TexrectShading::new(
            program,
            Color4::from_wire(0x0000_0000),
            PrimColor::from_wire(0, 0x8080_8080),
        )
        .validate_one_cycle()
        .expect("the env-lerp program reads only admitted selectors");
        let combined = combine_texel(shading, [0x10, 0x10, 0x10, 0x10]);
        assert_eq!(
            combined[0], 8,
            "7.9686 must round to 8, not truncate to 7 -- RT64 clamps in float and the byte is \
             the rounded value"
        );

        // Derivation 2, in the target precision: the pre-quantization value
        // really does lie strictly between 7 and 8, which is the only
        // condition under which the two roundings can disagree.
        let n = |byte: u8| f32::from(byte) / 255.0;
        let raw = (n(0x00) - n(0x10)) * n(0x80) + n(0x10);
        let scaled = raw * 255.0;
        assert!(
            scaled > 7.0 && scaled < 8.0,
            "the witness must straddle the two roundings: got {scaled}"
        );
        assert_eq!(scaled.round() as u8, 8);
        assert_eq!(
            scaled as u8, 7,
            "truncation gives 7, which is the mutant's answer"
        );

        // **And the difference survives the RGBA16 pack**, which is what
        // makes it observable in a composed image rather than only in the
        // 8-bit intermediate. This is the half the first draft missed.
        assert_ne!(
            8u16 >> 3,
            7u16 >> 3,
            "the two roundings must straddle a multiple of eight, or the RGBA16 target's `>> 3` \
             absorbs the difference and no composed test can ever see it"
        );
    }

    /// **A texrect's `Shade`/`ShadeAlpha` is admitted and evaluates to
    /// fn64's zero**, rather than being refused by name.
    ///
    /// A `G_TEXRECT` command carries no shade coefficient words; fn64 reads
    /// that wire layout as making `shade_color` `[0, 0, 0, 0]` for every
    /// pixel of every rectangle. This rule is not independently confirmed
    /// against an allowed hardware reference. See
    /// [`TexrectShading::base_inputs`].
    #[test]
    fn a_texrect_shade_reading_program_is_admitted_and_reads_zero() {
        // Color A index 4 is SHADE in the shared common table.
        let shade_in_color = pack_second_cycle([4, 8, 16, 7], [7, 7, 7, 7]);
        assert!(
            TexrectShading::new(
                shade_in_color,
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0)
            )
            .validate_one_cycle()
            .is_ok(),
            "a texrect program reading SHADE in a color slot must be admitted: its shade is the \
             hardware's zero, not an absent value"
        );
        // Color C index 11 is SHADE_ALPHA -- the exact selector WM2000
        // stages, and the one this executor used to abort on.
        let shade_alpha_in_c = pack_second_cycle([1, 8, 11, 7], [7, 7, 7, 7]);
        assert!(
            TexrectShading::new(
                shade_alpha_in_c,
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0)
            )
            .validate_one_cycle()
            .is_ok(),
            "slot C selecting SHADE_ALPHA must be admitted"
        );
        // And the alpha side, which has its own table.
        let shade_in_alpha = pack_second_cycle([8, 8, 16, 7], [4, 7, 7, 7]);
        assert!(
            TexrectShading::new(
                shade_in_alpha,
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0)
            )
            .validate_one_cycle()
            .is_ok(),
            "a program reading SHADE in an alpha slot must be admitted too"
        );
    }

    /// **The value a texrect's admitted `Shade` reads is zero, and the test
    /// can tell zero from every other candidate.**
    ///
    /// This is the mutation-resistant half. The trap this guards is a
    /// fixture where the correct answer and a wrong one coincide: if the
    /// executor were changed to feed `Shade` the primitive colour, the
    /// environment colour, or one, a program that merely *succeeds* would
    /// not notice. So both constant registers are staged to distinctive
    /// NON-ZERO values and the program is `(SHADE - ZERO) * ONE + ZERO`,
    /// whose output is the shade itself. Expected `[0, 0, 0, 0]` follows
    /// fn64's reading of the `G_TEXRECT` wire layout, which has no shade
    /// coefficient words. The zero-shade rule is not independently confirmed
    /// against an allowed hardware reference.
    #[test]
    fn a_texrects_shade_evaluates_to_the_hardwares_zero_not_a_neighbouring_register() {
        // Distinctive non-zero registers: any executor that substituted one
        // of these for the shade fails this test rather than passing it.
        let env = Color4::from_wire(0x1122_3344);
        let prim = PrimColor::from_wire(0, 0x5566_7788);
        // Derived by hand from the four per-slot decode tables, which are
        // NOT one shared table: slot A's index 6 is ONE, but slot C's is
        // KEY_SCALE and slot C has no ONE entry at all. So the passthrough
        // is built on the multiply-by-zero form instead of multiply-by-one.
        //
        // Color slots: A = SHADE(4), B = ZERO(8), C = ZERO(16), D = SHADE(4)
        // => (shade - 0) * 0 + shade = shade.
        // Alpha slots: A = SHADE(4), B = ZERO(7), C = ZERO(7), D = SHADE(4)
        // (alpha A/B/D share `alpha_input_abd`, where 4 is SHADE).
        let shade_passthrough = pack_second_cycle([4, 8, 16, 4], [4, 7, 7, 4]);
        let shading = TexrectShading::new(shade_passthrough, env, prim)
            .validate_one_cycle()
            .expect("a texrect reading SHADE is admitted");
        let inputs = shading.base_inputs();
        assert_eq!(
            inputs.shade_color, [0.0; 4],
            "a texture rectangle's shade is zero on hardware: the command carries no shade \
             coefficient words and the rasterizer zeroes the block"
        );
        // The registers really are distinct from the shade, so the equality
        // above is a measurement rather than a coincidence.
        assert_ne!(
            inputs.env_color, inputs.shade_color,
            "the fixture must stage an environment colour that differs from the shade, or a \
             mutant substituting ENV for SHADE survives"
        );
        assert_ne!(
            inputs.prim_color, inputs.shade_color,
            "the fixture must stage a primitive colour that differs from the shade, or a mutant \
             substituting PRIM for SHADE survives"
        );
        // And zero is distinguishable from the other obvious wrong answer.
        assert_ne!(
            inputs.shade_color, [1.0; 4],
            "zero must be distinguishable from ONE, or a mutant feeding a full-scale shade \
             survives"
        );
    }

    /// **An UNSHADED raw triangle keeps its `Shade` refusal.** The texrect
    /// admission above must not leak into the triangle path, where the
    /// hardware interpolates a real non-zero shade this executor cannot
    /// reconstruct. Kills the mutant that widens `shade_available` to `true`
    /// everywhere instead of only for rectangles.
    #[test]
    fn an_unshaded_raw_triangle_still_refuses_shade() {
        let shade_in_color = pack_second_cycle([4, 8, 16, 7], [7, 7, 7, 7]);
        assert_eq!(
            TexrectShading::new(
                shade_in_color,
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0)
            )
            .validate_combiner_program_with_shade(CombinerProgramCycles::OnlySecondSlice, false,),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot: ColorInputSlot::A,
                input: ColorInput::Shade,
            }),
            "an unshaded triangle has a real interpolated shade this executor cannot supply, so \
             it must still refuse"
        );
        // The message still names the selector, so a future title's log says
        // what is missing rather than only that something is.
        let message = TexrectShading::new(
            shade_in_color,
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
        )
        .validate_combiner_program_with_shade(CombinerProgramCycles::OnlySecondSlice, false)
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("Shade"),
            "the refusal must name the selector: {message}"
        );
    }

    /// **The admitted-selector tables are pinned by value, not merely
    /// consulted.**
    ///
    /// [`every_unmeasured_selector_is_refused`] derives its expectation
    /// FROM [`ADMITTED_COLOR_INPUTS`]/[`ADMITTED_ALPHA_INPUTS`], so it
    /// cannot notice a selector being added to those tables -- the
    /// expectation moves with the mutation. Measured: a mutant inserting
    /// `ColorInput::Shade` into `ADMITTED_COLOR_INPUTS` SURVIVED that sweep
    /// once the sweep was taught the second (slice-scoped) admission rule.
    ///
    /// This test is the fixed point. The contents are the measured WM2000
    /// window's selector set plus the register-backed widening, transcribed
    /// here by hand so that widening the executor's admitted set requires
    /// editing an explicit list in a test that says why.
    #[test]
    fn the_admitted_selector_tables_are_exactly_these() {
        assert_eq!(
            ADMITTED_COLOR_INPUTS,
            [
                ColorInput::Texel0,
                ColorInput::Primitive,
                ColorInput::Environment,
                ColorInput::One,
                ColorInput::Zero,
                ColorInput::Texel0Alpha,
                ColorInput::PrimitiveAlpha,
                ColorInput::EnvAlpha,
                ColorInput::PrimLodFrac,
            ],
            "ADMITTED_COLOR_INPUTS changed; a selector added here is a claim this executor \
             evaluates it, which needs its own measurement and citation"
        );
        assert_eq!(
            ADMITTED_ALPHA_INPUTS,
            [
                AlphaInput::Texel0,
                AlphaInput::Primitive,
                AlphaInput::Environment,
                AlphaInput::One,
                AlphaInput::Zero,
                AlphaInput::PrimLodFrac,
            ],
            "ADMITTED_ALPHA_INPUTS changed; same rule as the color table"
        );
        // `Combined`/`CombinedAlpha` are deliberately NOT in either table:
        // their admissibility is slice-scoped, not selector-scoped, and
        // lives in `resolves_the_combined_selector`.
        assert!(
            !ADMITTED_COLOR_INPUTS.contains(&ColorInput::Combined)
                && !ADMITTED_COLOR_INPUTS.contains(&ColorInput::CombinedAlpha)
                && !ADMITTED_ALPHA_INPUTS.contains(&AlphaInput::Combined),
            "the COMBINED selectors must stay out of the flat tables, or the slice rule is \
             bypassed for a two-cycle program's first cycle"
        );
    }

    /// The other unmeasured selectors are refused too, each by name -- not
    /// only `Shade`. Swept over every selector the wire can express in
    /// color slot A and alpha slot A, so a selector added to `ColorInput`
    /// later cannot be silently admitted.
    ///
    /// **Admission has two independent rules, and the sweep must model
    /// both.** [`ADMITTED_COLOR_INPUTS`]/[`ADMITTED_ALPHA_INPUTS`] are the
    /// register-and-texel table, and
    /// [`CombinerProgramSlice::resolves_the_combined_selector`] is a
    /// separate slice-scoped rule for `Combined`/`CombinedAlpha`, which are
    /// deliberately absent from those tables because their admissibility
    /// depends on the cycle mode rather than on the selector alone. This
    /// sweep runs `validate_one_cycle`, so for it the second rule says
    /// `Combined` IS admitted -- it reads RT64's zero-initialized
    /// accumulator (`rt64_color_combiner.h:470-471`, `611-620`). Deriving
    /// the expectation from the table alone would assert the opposite and
    /// contradict the gate this crate actually ships.
    ///
    /// There is a **third** rule, also deliberately outside the tables:
    /// `Shade`/`ShadeAlpha` is primitive-scoped. `validate_one_cycle` is
    /// the texrect entry point, and a rectangle's shade is the hardware's
    /// zero (derived in [`TexrectShading::base_inputs`]), so this sweep
    /// expects it admitted. The same selector on an UNSHADED raw triangle
    /// is still refused -- see
    /// [`an_unshaded_raw_triangle_still_refuses_shade`].
    #[test]
    fn every_unmeasured_selector_is_refused() {
        for index in 0u32..16 {
            let params = pack_second_cycle([index, 8, 16, 7], [7, 7, 7, 7]);
            let input = params.decode_color(ColorInputSlot::A, true);
            // All three admission rules, exactly as `admits_color`
            // composes them -- see this test's own doc.
            let admitted = matches!(input, ColorInput::Combined | ColorInput::CombinedAlpha)
                || matches!(input, ColorInput::Shade | ColorInput::ShadeAlpha)
                || ADMITTED_COLOR_INPUTS
                    .iter()
                    .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
            let result = TexrectShading::new(
                params,
                Color4::from_wire(ENV_WIRE),
                PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
            )
            .validate_one_cycle();
            if admitted {
                assert!(
                    result.is_ok(),
                    "color index {index} ({input:?}) must be admitted"
                );
            } else {
                assert_eq!(
                    result,
                    Err(TexrectExecutionError::UnsupportedColorInput {
                        slot: ColorInputSlot::A,
                        input,
                    }),
                    "color index {index} decodes to {input:?}, which must be refused by name"
                );
            }
        }
        for index in 0u32..8 {
            let params = pack_second_cycle([8, 8, 16, 7], [index, 7, 7, 7]);
            let input = params.decode_alpha(AlphaInputSlot::A, true);
            let admitted = matches!(input, AlphaInput::Combined)
                || matches!(input, AlphaInput::Shade)
                || ADMITTED_ALPHA_INPUTS
                    .iter()
                    .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
            let result = TexrectShading::new(
                params,
                Color4::from_wire(ENV_WIRE),
                PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
            )
            .validate_one_cycle();
            if admitted {
                assert!(
                    result.is_ok(),
                    "alpha index {index} ({input:?}) must be admitted"
                );
            } else {
                assert_eq!(
                    result,
                    Err(TexrectExecutionError::UnsupportedAlphaInput {
                        slot: AlphaInputSlot::A,
                        input,
                    }),
                    "alpha index {index} decodes to {input:?}, which must be refused by name"
                );
            }
        }
    }

    /// **D4, the register-backed widening.** Four color selectors and one
    /// alpha selector this executor refused resolve to components of values
    /// it already sources from real wire registers, so evaluating them
    /// invents nothing.
    ///
    /// `docs/RT64-LANE-DIVERGENCES.md` D4 scores twelve refused selectors
    /// reference-correct on the ground that `crate::combiner` implements
    /// every one of them. That is true and it is not sufficient: the
    /// combiner implementing a selector means it can *read* a
    /// `CombinerInputs` field, not that this executor can *fill* it. This
    /// test separates the two, admitting exactly the register-backed subset
    /// and pinning the rest as still-refused.
    ///
    /// Expectations are hand-derived from `combiner.rs`'s
    /// `resolve_color_input`/`resolve_alpha_input` and from
    /// `combiner_inputs_from_fragment_registers`, and the wire indices are
    /// found by asking the decoder rather than transcribed -- so a decode
    /// table change moves the probe instead of silently testing the wrong
    /// selector. Deliberately does NOT consult `ADMITTED_COLOR_INPUTS`, the
    /// way the exhaustive sweep above does: a test that derives its
    /// expectation from the constant under test cannot fail when that
    /// constant changes, which is why the sweep stayed green across this
    /// widening.
    #[test]
    fn register_backed_selectors_are_admitted_and_invented_ones_are_not() {
        // Find a (slot, wire index) pair decoding to `target`. Slot C is
        // the five-bit slot reaching most extended selectors, but not all:
        // `Noise` is slot-A only, and slot A is four bits. Ask the decoder
        // which slot can express the selector rather than assuming one.
        let color_probe_for = |target: ColorInput| -> (ColorInputSlot, CombineParams) {
            for index in 0u32..32 {
                let params = pack_second_cycle([1, 1, index, 1], [7, 7, 7, 7]);
                if core::mem::discriminant(&params.decode_color(ColorInputSlot::C, true))
                    == core::mem::discriminant(&target)
                {
                    return (ColorInputSlot::C, params);
                }
            }
            // Slots A, B and D are four-bit and reach different subsets:
            // `Noise` is slot-A only and `K4` is slot-B only. Try each.
            for index in 0u32..16 {
                for (slot, params) in [
                    (
                        ColorInputSlot::A,
                        pack_second_cycle([index, 1, 16, 1], [7, 7, 7, 7]),
                    ),
                    (
                        ColorInputSlot::B,
                        pack_second_cycle([1, index, 16, 1], [7, 7, 7, 7]),
                    ),
                    (
                        ColorInputSlot::D,
                        pack_second_cycle([1, 1, 16, index], [7, 7, 7, 7]),
                    ),
                ] {
                    if core::mem::discriminant(&params.decode_color(slot, true))
                        == core::mem::discriminant(&target)
                    {
                        return (slot, params);
                    }
                }
            }
            panic!("no color wire index decodes to {target:?}")
        };
        let alpha_index_for = |target: AlphaInput| -> u32 {
            (0u32..8)
                .find(|index| {
                    let params = pack_second_cycle([1, 1, 16, 7], [7, 7, *index, 7]);
                    core::mem::discriminant(&params.decode_alpha(AlphaInputSlot::C, true))
                        == core::mem::discriminant(&target)
                })
                .unwrap_or_else(|| panic!("no slot-C alpha wire index decodes to {target:?}"))
        };

        let shading = |params| {
            TexrectShading::new(
                params,
                Color4::from_wire(ENV_WIRE),
                PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
            )
            .validate_one_cycle()
        };

        // ADMITTED: each reads a component of a register this executor
        // already sources. Before this widening every one of these was
        // `UnsupportedColorInput`.
        for target in [
            ColorInput::Texel0Alpha,
            ColorInput::PrimitiveAlpha,
            ColorInput::EnvAlpha,
            ColorInput::PrimLodFrac,
        ] {
            let (slot, params) = color_probe_for(target);
            assert!(
                shading(params).is_ok(),
                "{target:?} (via {slot:?}) reads a register this executor \
                 already supplies and must be admitted"
            );
        }
        let index = alpha_index_for(AlphaInput::PrimLodFrac);
        let params = pack_second_cycle([1, 1, 16, 7], [7, 7, index, 7]);
        assert!(
            shading(params).is_ok(),
            "AlphaInput::PrimLodFrac (slot-C index {index}) comes from the \
             same SetPrimColor word as Primitive and must be admitted"
        );

        // STILL REFUSED: each reads a `base_inputs` field this executor
        // leaves at zero *with no authority saying zero is what the
        // hardware produces*. There is no `SetConvert`/`SetKey` plumbing,
        // no LOD stage, no noise seed, and no decoded tile+1 -- so
        // admitting one would combine against an invented value.
        //
        // `Shade`/`ShadeAlpha` is deliberately NOT in this list any more,
        // and the distinction is the whole point of the list. Its zero is
        // not an accidental unset field: a texture rectangle carries no
        // shade words, and fn64 reads that wire layout as requiring zero for
        // the synthesized primitive. This zero-shade rule is fn64's own
        // reading and is not independently confirmed against an allowed
        // hardware reference. See
        // [`a_texrects_shade_evaluates_to_the_hardwares_zero_not_a_neighbouring_register`]
        // and [`TexrectShading::base_inputs`]. The unshaded-*triangle*
        // refusal, where the hardware really does interpolate a value this
        // executor cannot reconstruct, is pinned by
        // [`an_unshaded_raw_triangle_still_refuses_shade`].
        for target in [
            ColorInput::LodFraction,
            ColorInput::Noise,
            ColorInput::K4,
            ColorInput::K5,
            ColorInput::KeyCenter,
            ColorInput::KeyScale,
            ColorInput::Texel1,
            ColorInput::Texel1Alpha,
        ] {
            let (slot, params) = color_probe_for(target);
            assert_eq!(
                shading(params),
                Err(TexrectExecutionError::UnsupportedColorInput {
                    slot,
                    input: target,
                }),
                "{target:?} (via {slot:?}) reads a zeroed field and must \
                 stay refused by name"
            );
        }

        // **The register-backed selectors are admitted even against a
        // never-written register.** `SetEnvColor`/`SetPrimColor` name RDP
        // registers, which hold their power-on zero until the guest writes
        // them, so reading one before any wire command is a legal read of a
        // real value -- not a substitution. This is the opposite assertion
        // to the loop above, and the difference is the point: `LodFraction`
        // has no authority behind its zero at all, while `EnvAlpha` reads a
        // real RDP register and `Shade` reads a value the rasterizer
        // demonstrably clears.
        for target in [
            ColorInput::EnvAlpha,
            ColorInput::PrimitiveAlpha,
            ColorInput::PrimLodFrac,
        ] {
            let (_, params) = color_probe_for(target);
            assert!(
                TexrectShading::new(params, Color4::from_wire(0), PrimColor::from_wire(0, 0))
                    .validate_one_cycle()
                    .is_ok(),
                "{target:?} reads a register that always holds a value, so a never-written \
                 register must not refuse the rectangle"
            );
        }
        let index = alpha_index_for(AlphaInput::PrimLodFrac);
        let params = pack_second_cycle([1, 1, 16, 7], [7, 7, index, 7]);
        assert!(
            TexrectShading::new(params, Color4::from_wire(0), PrimColor::from_wire(0, 0))
                .validate_one_cycle()
                .is_ok()
        );
    }

    /// **A never-written constant register reads as its power-on zero
    /// rather than refusing the rectangle**, and the value it supplies is
    /// really the register's -- a written register still wins.
    ///
    /// This replaces a test that asserted the opposite. The refusal it
    /// pinned invented an "unset" state the RDP has no way to be in:
    /// `fn64-render-reference` models the constant color registers as
    /// zero-initialized `[u8; 4]` (`gbi/state.rs:227`, `:387`) and RT64's
    /// C++ zero-initializes `primColor`/`envColor` at
    /// `src/hle/rt64_state.cpp:126-129`.
    #[test]
    fn a_never_written_constant_register_reads_as_zero_instead_of_refusing() {
        let unwritten = TexrectShading::new(
            env_lerp_program(),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
        )
        .validate_one_cycle()
        .expect("a program reading ENVIRONMENT/PRIMITIVE before any wire command is legal");
        // Derived by hand: an RDP color register powers up holding four
        // zero bytes, and `Color4::normalized` is `byte / 255.0`, so every
        // channel is exactly 0.0.
        let inputs = unwritten.base_inputs();
        assert_eq!(inputs.env_color, [0.0; 4]);
        assert_eq!(inputs.prim_color, [0.0; 4]);
        assert_eq!(inputs.prim_lod_frac, 0.0);

        // A written register must actually reach the combiner inputs, or
        // the assertions above could pass against a hardcoded zero that
        // ignores every SetEnvColor/SetPrimColor.
        let written = TexrectShading::new(
            env_lerp_program(),
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_one_cycle()
        .expect("a program reading written registers is legal");
        let written_inputs = written.base_inputs();
        assert_ne!(
            written_inputs.env_color, inputs.env_color,
            "a written SetEnvColor must differ from the power-on zero, or this test cannot \
             distinguish a real register read from a hardcoded zero"
        );
        assert_ne!(
            written_inputs.prim_color, inputs.prim_color,
            "a written SetPrimColor must differ from the power-on zero"
        );

        // A ZERO-only program reads neither register and is legal either
        // way -- unchanged by this fix.
        let neither = pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]);
        assert!(
            TexrectShading::new(neither, Color4::from_wire(0), PrimColor::from_wire(0, 0))
                .validate_one_cycle()
                .is_ok(),
            "a program reading neither constant register stays admitted"
        );
    }

    /// **Which evaluation each cycle type selects at the EXECUTOR, not only
    /// in the error type's prose** -- mutant (f), and this test exists
    /// because that mutant SURVIVED its first draft.
    ///
    /// Reaching `execute_texture_rectangle` needs a live pending TMEM
    /// transaction, which no unit test can build. What this test pins
    /// instead is the decision the executor makes, extracted as
    /// [`admitted_cycle_evaluation`] -- the same match, called by the
    /// executor. Exhaustive over all four `CycleType` variants, so a fifth
    /// added later cannot be silently admitted either.
    ///
    /// Note what this test does **not** prove and never did: that the
    /// evaluation it names is the arithmetic that runs. Naming
    /// `TexrectCombinerEvaluation::TwoCycle` here would still pass if
    /// `combine_one_texel` ignored it and called `run_one_cycle` anyway.
    /// [`two_cycle_carries_the_accumulator_one_cycle_cannot`] is the test
    /// that closes that, and it is the reason the widening was allowed to
    /// land at all.
    #[test]
    fn the_executor_admits_copy_one_cycle_and_two_cycle() {
        assert_eq!(
            admitted_cycle_evaluation(CycleType::Copy),
            Ok(TexrectCombinerEvaluation::BlitsTheTexel),
            "Copy is admitted and evaluates NO combiner"
        );
        assert_eq!(
            admitted_cycle_evaluation(CycleType::OneCycle),
            Ok(TexrectCombinerEvaluation::OneCycle),
            "OneCycle is admitted and evaluates ONE combiner pass"
        );
        assert_eq!(
            admitted_cycle_evaluation(CycleType::TwoCycle),
            Ok(TexrectCombinerEvaluation::TwoCycle),
            "TwoCycle is admitted and evaluates BOTH combiner passes"
        );
        assert_eq!(
            admitted_cycle_evaluation(CycleType::Fill),
            Err(TexrectExecutionError::UnsupportedCycleType {
                cycle_type: CycleType::Fill
            }),
            "Fill samples no texture and must still be refused by name"
        );
    }

    /// **Positive control for [`carry_program`]**: the merged wire words
    /// really do decode to two *different* programs in the two slices.
    ///
    /// Without this, a packing slip could put the same selectors in both
    /// slices, and the witness test below would compare one-cycle against a
    /// two-cycle run of the same formula -- which could pass for the wrong
    /// reason.
    #[test]
    fn two_cycle_wire_program_decodes_to_both_slices() {
        let program = carry_program();
        assert_eq!(
            [
                program.decode_color(ColorInputSlot::A, false),
                program.decode_color(ColorInputSlot::B, false),
                program.decode_color(ColorInputSlot::C, false),
                program.decode_color(ColorInputSlot::D, false),
            ],
            [
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Primitive,
            ],
            "cycle 0's RGB must be (Zero - Zero) * Zero + Primitive"
        );
        assert_eq!(
            [
                program.decode_color(ColorInputSlot::A, true),
                program.decode_color(ColorInputSlot::B, true),
                program.decode_color(ColorInputSlot::C, true),
                program.decode_color(ColorInputSlot::D, true),
            ],
            [
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Zero,
                ColorInput::Combined,
            ],
            "cycle 1's RGB must be (Zero - Zero) * Zero + Combined"
        );
        assert_eq!(
            [
                program.decode_alpha(AlphaInputSlot::A, false),
                program.decode_alpha(AlphaInputSlot::B, false),
                program.decode_alpha(AlphaInputSlot::C, false),
                program.decode_alpha(AlphaInputSlot::D, false),
            ],
            [
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Primitive,
            ],
            "cycle 0's alpha must be (Zero - Zero) * Zero + Primitive"
        );
        assert_eq!(
            [
                program.decode_alpha(AlphaInputSlot::A, true),
                program.decode_alpha(AlphaInputSlot::B, true),
                program.decode_alpha(AlphaInputSlot::C, true),
                program.decode_alpha(AlphaInputSlot::D, true),
            ],
            [
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Zero,
                AlphaInput::Combined,
            ],
            "cycle 1's alpha must be (Zero - Zero) * Zero + Combined"
        );
    }

    /// **Two-cycle evaluation carries cycle 0's result into cycle 1, and
    /// one-cycle evaluation of the same program cannot** -- the observation
    /// the previous draft of this module was missing.
    ///
    /// The refusal that used to sit at [`admitted_cycle_evaluation`] recorded
    /// its own blind spot: "while this match was inline, widening it to
    /// admit two-cycle left the entire suite green." A green suite proved
    /// nothing was broken, not that anything was evaluated. Nothing in the
    /// suite ever ran two-cycle *arithmetic*, so the widened arm was
    /// unobserved either way.
    ///
    /// This test observes it. [`carry_program`]'s two slices are different
    /// formulas by construction (asserted above), and the hand derivation
    /// is:
    ///
    /// - cycle 0: `(0 - 0) * 0 + Primitive` = the primitive colour,
    ///   `0x80/0xFF/0x40/0x80` normalized, written into the accumulator;
    /// - cycle 1: `(0 - 0) * 0 + Combined` = that accumulator verbatim.
    ///
    /// So two-cycle must give back the primitive colour's own bytes. The
    /// same program run as one-cycle evaluates **only** the second slice
    /// against the zero-initialized accumulator, where `Combined` is `0.0`,
    /// so it must give transparent black. Both are asserted, and asserted to
    /// differ -- the inequality alone would be satisfied by two equally
    /// wrong answers.
    ///
    /// `wrap_clamp` is the identity on both: every channel of both answers
    /// is already inside `[0, 1]`.
    #[test]
    fn two_cycle_carries_the_accumulator_one_cycle_cannot() {
        let program = carry_program();
        let base = TexrectShading::new(
            program,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .base_inputs();
        // The texel is deliberately non-zero and unlike the primitive
        // colour: neither slice reads TEXEL0, so a two-cycle answer that
        // leaked the texel would be caught here rather than mistaken for
        // the carry.
        let texel = [0x18, 0x40, 0xC8, 0xFF];

        let two_cycle =
            combine_one_texel(program, base, texel, TexrectCombinerEvaluation::TwoCycle);
        let one_cycle =
            combine_one_texel(program, base, texel, TexrectCombinerEvaluation::OneCycle);

        assert_eq!(
            two_cycle,
            [0x80, 0xFF, 0x40, 0x80],
            "cycle 0 writes the primitive colour into the accumulator and cycle 1 emits it \
             verbatim through D = COMBINED"
        );
        assert_eq!(
            one_cycle,
            [0x00, 0x00, 0x00, 0x00],
            "one-cycle mode runs ONLY the second slice, whose D = COMBINED reads the \
             zero-initialized accumulator"
        );
        assert_ne!(
            two_cycle, one_cycle,
            "if these agree, the two-cycle arm is not running two cycles"
        );
    }

    /// **`Combined` is admitted everywhere except a two-cycle program's
    /// FIRST slice**, and in one-cycle mode it resolves to RT64's
    /// zero-initialized accumulator rather than to a value this executor
    /// invents.
    ///
    /// See [`CombinerProgramSlice::resolves_the_combined_selector`] for the
    /// RT64 citation (`rt64_color_combiner.h:470-471`, `611-620`, `577`)
    /// and the ROM measurement.
    ///
    /// This asserts the ADMISSION rule and the ARITHMETIC together, because
    /// admitting the selector is only correct if the value behind it is the
    /// hardware's. Hand-derived from [`carry_program`]'s second slice,
    /// `(Zero - Zero) * Zero + Combined` over a zero accumulator, which is
    /// transparent black.
    #[test]
    fn combined_is_admitted_outside_the_first_slice_of_two_cycles() {
        let program = carry_program();
        let shading = TexrectShading::new(
            program,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        );
        shading
            .validate_combiner_program(CombinerProgramCycles::BothSlices)
            .expect("cycle 1 has a first-cycle result for COMBINED to read");
        shading
            .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
            .expect(
                "one-cycle COMBINED reads RT64's zero-initialized accumulator, which is \
                 defined behaviour",
            );
        assert_eq!(
            shading.validate_one_cycle(),
            shading.validate_combiner_program(CombinerProgramCycles::OnlySecondSlice),
            "validate_one_cycle must stay an exact alias for the one-slice admission"
        );

        // The admitted program must also EVALUATE to the hardware's answer,
        // not merely get past the gate.
        let base = shading.base_inputs();
        assert_eq!(
            combine_one_texel(
                program,
                base,
                [0x18, 0x40, 0xC8, 0xFF],
                TexrectCombinerEvaluation::OneCycle
            ),
            [0x00, 0x00, 0x00, 0x00],
            "one-cycle D = COMBINED reads the zero-initialized accumulator"
        );
    }

    /// **The alpha `Combined` and the colour `CombinedAlpha` selectors go
    /// through the same slice gate as the plain colour `Combined`.**
    ///
    /// Both are distinct decode paths --
    /// `alphaInputABD` index `0` is `AlphaInput::Combined`
    /// (`combiner.rs`'s `alpha_input_abd`), and `colorInputC` index `7` is
    /// `ColorInput::CombinedAlpha` (`color_input_c`) -- and RT64 resolves
    /// both from the same accumulator (`rt64_color_combiner.h:486-487`
    /// `C_COMBINED_ALPHA -> combinerColor.a`, `517-518` `A_COMBINED ->
    /// combinerAlpha`). A gate that admitted the plain colour selector but
    /// left either of these unguarded would let a two-cycle cycle-0
    /// program through the one door this repair deliberately keeps shut.
    ///
    /// Written because mutants that bypassed `admits_alpha`'s gate and that
    /// dropped `CombinedAlpha` from `admits_color`'s guarded set both
    /// SURVIVED the admission tests above.
    #[test]
    fn the_alpha_and_combined_alpha_selectors_share_the_slice_gate() {
        // Alpha slot A = COMBINED (index 0) in cycle 0 of a two-cycle
        // program; every colour slot and the rest of alpha are Zero.
        let alpha_combined_first = merge_cycles(
            pack_first_cycle([8, 8, 16, 7], [0, 7, 7, 7]),
            pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]),
        );
        let shading = TexrectShading::new(
            alpha_combined_first,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        );
        assert_eq!(
            shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
            Err(TexrectExecutionError::UnsupportedAlphaInput {
                slot: AlphaInputSlot::A,
                input: AlphaInput::Combined,
            }),
            "alpha COMBINED in cycle 0 of a two-cycle program must be refused"
        );
        shading
            .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
            .expect("the same word read as one-cycle names no COMBINED at all");

        // Colour slot C = COMBINED_ALPHA (index 7) in cycle 0.
        let combined_alpha_first = merge_cycles(
            pack_first_cycle([8, 8, 7, 7], [7, 7, 7, 7]),
            pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]),
        );
        let shading = TexrectShading::new(
            combined_alpha_first,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        );
        assert_eq!(
            shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot: ColorInputSlot::C,
                input: ColorInput::CombinedAlpha,
            }),
            "COMBINED_ALPHA in cycle 0 of a two-cycle program must be refused"
        );

        // ...and both are ADMITTED in one-cycle mode, where the zero
        // accumulator is the hardware's own answer.
        let alpha_combined_one_cycle = pack_second_cycle([8, 8, 16, 7], [0, 7, 7, 7]);
        TexrectShading::new(
            alpha_combined_one_cycle,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
        .expect("one-cycle alpha COMBINED reads the zero-initialized accumulator");

        let combined_alpha_one_cycle = pack_second_cycle([8, 8, 7, 7], [7, 7, 7, 7]);
        TexrectShading::new(
            combined_alpha_one_cycle,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        )
        .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
        .expect("one-cycle COMBINED_ALPHA reads the zero-initialized accumulator");
    }

    /// **A two-cycle program's FIRST slice still refuses `Combined`.**
    ///
    /// The widening above is not "admit COMBINED everywhere". This pins the
    /// arm the repair KEEPS: no measurement in this repo covers a
    /// `COMBINED` read in cycle 0 of a two-cycle program.
    #[test]
    fn combined_in_the_first_slice_of_two_cycles_is_still_refused() {
        // Cycle 0 slot A = COMBINED (index 0); cycle 1 is an
        // all-Zero/Primitive program that admits cleanly on its own.
        let program = merge_cycles(
            pack_first_cycle([0, 8, 16, 3], [7, 7, 7, 3]),
            pack_second_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
        );
        let shading = TexrectShading::new(
            program,
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        );
        assert_eq!(
            shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot: ColorInputSlot::A,
                input: ColorInput::Combined,
            }),
            "cycle 0 of a two-cycle program has no first-cycle result behind COMBINED"
        );
        shading
            .validate_combiner_program(CombinerProgramCycles::OnlySecondSlice)
            .expect("the same wire word read as one-cycle names no COMBINED at all");
    }

    /// **`Texel1` stays refused in BOTH slices of a two-cycle program.**
    ///
    /// A rectangle binds one tile ([`TexrectTileBinding`] carries a single
    /// descriptor), so there is no decoded tile+1 to sample. The reference
    /// lane refuses `Texel1` for a rectangle for exactly that reason
    /// (`fn64-render-reference/src/backend/validate.rs:479-483`). Widening
    /// the cycle admission must not widen this one.
    #[test]
    fn texel1_is_refused_in_both_slices_of_a_two_cycle_program() {
        // Color slot A index 2 is TEXEL0's neighbour TEXEL1 in
        // `colorInputCommon`; placed in cycle 0 first, then in cycle 1.
        let in_first = merge_cycles(
            pack_first_cycle([2, 8, 16, 3], [7, 7, 7, 3]),
            pack_second_cycle([8, 8, 16, 0], [7, 7, 7, 0]),
        );
        let in_second = merge_cycles(
            pack_first_cycle([8, 8, 16, 3], [7, 7, 7, 3]),
            pack_second_cycle([2, 8, 16, 0], [7, 7, 7, 0]),
        );
        for (program, slice) in [(in_first, "cycle 0"), (in_second, "cycle 1")] {
            let shading = TexrectShading::new(
                program,
                Color4::from_wire(ENV_WIRE),
                PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
            );
            assert_eq!(
                shading.validate_combiner_program(CombinerProgramCycles::BothSlices),
                Err(TexrectExecutionError::UnsupportedColorInput {
                    slot: ColorInputSlot::A,
                    input: ColorInput::Texel1,
                }),
                "TEXEL1 in {slice} has no decoded tile+1 behind it and must be refused"
            );
        }
    }

    /// **The reason Fill cannot be admitted by widening the match above,
    /// as an executable fact rather than a comment.**
    ///
    /// A texrect reaches [`execute_texture_rectangle`] as an
    /// already-resolved [`RectViewportPixels`], built by
    /// `raw_dpc/texture_rectangle.rs`'s port of RT64's `FixedRect`:
    /// `(coord + 3) >> 2` at both ends, half-open. A fill rectangle's rule
    /// is `targets/fill.rs`'s [`super::resolve_fill_pixel_rectangle`]:
    /// `coord >> 2` at both ends, inclusive.
    ///
    /// This asserts the two disagree on wire coordinates that are legal for
    /// both -- every edge a multiple of four, so the fill rule's
    /// `FractionalEdge` refusal does not fire and the disagreement is
    /// purely the inclusive/half-open split. If a future change made them
    /// agree, admitting Fill above would become a one-line fix and this
    /// test would say so by failing.
    ///
    /// Both rules are re-derived in this test body from their published
    /// expressions on the texrect side, so this pins the disagreement
    /// itself rather than one implementation against the other. The fill
    /// side calls the real `resolve_fill_pixel_rectangle`, since that is
    /// the function a fix would have to route to.
    #[test]
    fn the_texrect_and_fill_rectangle_rules_disagree_by_a_pixel_on_every_axis() {
        // Wire 2.2 fixed-point coordinates, every edge a whole pixel.
        for (ulx, uly, lrx, lry) in [
            (0u16, 0u16, 16u16, 16u16),
            (8, 8, 40, 24),
            (0, 0, 1276, 956),
        ] {
            // The texrect side, re-derived: fill mode rounds the upper-left
            // down (`ulx &= !3`, a no-op on these whole-pixel edges), then
            // `FixedRect::left/top/right/bottom` with `ceil = true`.
            let left = ((i32::from(ulx) & !3) + 3) >> 2;
            let top = ((i32::from(uly) & !3) + 3) >> 2;
            let right = (i32::from(lrx) + 3) >> 2;
            let bottom = (i32::from(lry) + 3) >> 2;
            let texrect_extent = (right - left, bottom - top);

            // The fill side, through the executor a fix would route to.
            let fill = super::super::resolve_fill_pixel_rectangle(ulx, uly, lrx, lry)
                .expect("every edge here is a whole pixel");
            let fill_extent = (fill.width() as i32, fill.height() as i32);

            assert_eq!(
                fill_extent,
                (texrect_extent.0 + 1, texrect_extent.1 + 1),
                "wire ({ulx}, {uly}, {lrx}, {lry}): the fill rule is inclusive and the texrect \
                 rule is half-open, so the fill rectangle is exactly one pixel larger on each \
                 axis"
            );
            assert_ne!(
                fill_extent, texrect_extent,
                "if these ever agree, admitting Fill at admitted_cycle_evaluation becomes a \
                 one-line fix and this test must be re-justified"
            );
        }
    }

    /// **The fill rule refuses a fractional edge the texrect rule silently
    /// rounds** -- the second half of why the two are not interchangeable.
    #[test]
    fn the_fill_rule_refuses_a_fractional_edge_the_texrect_rule_rounds() {
        // `ulx = 2` is half a pixel.
        let texrect_left = ((2i32 & !3) + 3) >> 2;
        assert_eq!(
            texrect_left, 0,
            "the texrect rule rounds a half-pixel upper-left down to pixel 0"
        );
        assert!(
            super::super::resolve_fill_pixel_rectangle(2, 2, 18, 18).is_err(),
            "the fill rule refuses a fractional edge by name rather than rounding it"
        );
    }

    /// **Fill remains refused by name** -- the admission widened by exactly
    /// one mode, not into a blanket acceptance.
    ///
    /// Checked at the enum rather than through the executor because
    /// reaching the executor needs a live pending TMEM transaction, which
    /// the end-to-end tests supply; what is pinned here is that the mode
    /// set this module claims is `{Copy, OneCycle, TwoCycle}` and its
    /// complement is named.
    #[test]
    fn the_admitted_cycle_set_is_copy_one_cycle_and_two_cycle() {
        let cycle_type = CycleType::Fill;
        let error = TexrectExecutionError::UnsupportedCycleType { cycle_type };
        let message = error.to_string();
        assert!(
            message.contains(&format!("{cycle_type:?}")),
            "the refusal must name the mode it refused: {message}"
        );
        assert!(
            message.contains("Copy")
                && message.contains("OneCycle")
                && message.contains("TwoCycle"),
            "the refusal must state which modes ARE admitted: {message}"
        );
    }
}

/// The blender stage this executor runs, checked against hand-derived
/// arithmetic rather than against either implementation's output.
///
/// Every expectation here is derived from WM2000's frame-0 packet's own
/// latched words and the public `GBL_c1` formula, computed independently in
/// the test body from those words -- never captured from a run. The
/// measured whole-image consequence lives in `fn64-abi`'s
/// `wm2000_frame_zero_*` comparison against `fn64-render-reference`; these
/// pin the arithmetic that produces it.
#[cfg(test)]
mod blend_stage_tests {
    use super::*;
    use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput};

    /// WM2000 frame 0's latched other-mode words, read off the captured
    /// packet (`docs/RT64-WM2000-VALIDATION.md` §3): high `0x0000acef`
    /// (one-cycle, RGB dither Disabled, alpha dither Noise), low
    /// `0x005041c8` (`AA_EN`, `IM_RD`, `CLR_ON_CVG`, `cvg_dst = Wrap`,
    /// `FORCE_BL`, `CVG_X_ALPHA` and `ALPHA_CVG_SEL` clear).
    const WM2000_OTHER_MODE_HIGH: u32 = 0x0000_acef;
    const WM2000_OTHER_MODE_LOW: u32 = 0x0050_41c8;

    fn wm2000_other_mode() -> OtherMode {
        OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW)
    }

    fn wm2000_blend_state() -> BlendModeState {
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(wm2000_other_mode())
    }

    fn wm2000_stages() -> TexrectFragmentStages {
        TexrectFragmentStages::try_new(wm2000_other_mode(), Color4::from_wire(0))
            .expect("WM2000's frame-0 mode is admitted by every stage")
    }

    /// **Positive control for every expectation below.** The two wire words
    /// really do decode to the mode the derivation assumes.
    ///
    /// Each field is asserted from the accessor AND reconciled against an
    /// independent bit derivation of the same literal, so an off-by-one in
    /// either the mask or the transcription contradicts itself rather than
    /// agreeing by construction.
    #[test]
    fn wm2000_frame_zero_other_mode_decodes_to_the_derived_blender_state() {
        let mode = wm2000_other_mode();
        assert_eq!(mode.cycle_type(), CycleType::OneCycle);
        assert_eq!(
            (WM2000_OTHER_MODE_HIGH >> 20) & 0x3,
            0,
            "one-cycle, derived from the literal independently of the accessor"
        );
        assert!(mode.force_blend());
        assert_eq!(WM2000_OTHER_MODE_LOW & 0x4000, 0x4000, "FORCE_BL is bit 14");
        assert!(mode.image_read_enabled());
        assert_eq!(WM2000_OTHER_MODE_LOW & 0x0040, 0x0040, "IM_RD is bit 6");

        let cycle = ResolvedBlendCycleUnderTest::of(mode);
        assert_eq!(cycle.p, BlendColorInput::Combined);
        assert_eq!(cycle.a, BlendAlphaInput::Combined);
        assert_eq!(cycle.m, BlendColorInput::Framebuffer);
        assert_eq!(cycle.b, BlendBInput::OneMinusA);
        // The same four selectors, re-derived from the literal's own bit
        // fields rather than from `blender_cycle_1`.
        assert_eq!((WM2000_OTHER_MODE_LOW >> 30) & 0x3, 0, "P = Combined");
        assert_eq!((WM2000_OTHER_MODE_LOW >> 26) & 0x3, 0, "A = Combined");
        assert_eq!((WM2000_OTHER_MODE_LOW >> 22) & 0x3, 1, "M = Framebuffer");
        assert_eq!((WM2000_OTHER_MODE_LOW >> 18) & 0x3, 0, "B = OneMinusA");
    }

    struct ResolvedBlendCycleUnderTest;
    impl ResolvedBlendCycleUnderTest {
        fn of(mode: OtherMode) -> crate::blend::ResolvedBlendCycle {
            crate::blend::ResolvedBlendCycle::from_wire(mode.blender_cycle_1())
        }
    }

    /// **The hand-derivation the whole card rests on.**
    ///
    /// WM2000's texrect combiner is `(Zero - Zero) * Zero + Primitive` with
    /// `SetPrimColor 0xffffffdf`, so the combined fragment is RGB 255 at
    /// alpha 223. The cycle above is `P = Combined, A = Combined,
    /// M = Framebuffer, B = 1 - A`, and `blend_fragment`'s
    /// `M == Framebuffer` arm makes the composite
    /// `combined * (223/255) + destination * (1 - 223/255)`.
    ///
    /// Derived here in the test, twice and by different routes: once as the
    /// closed form, once by stepping the selector arms the same way the
    /// blender does. The two must agree, so a transcription slip in either
    /// contradicts itself rather than being confirmed by the implementation
    /// it is supposed to check.
    #[test]
    fn the_wm2000_composite_is_hand_derived_over_a_zero_destination() {
        const COMBINED: [u8; 4] = [255, 255, 255, 223];
        let destination = BlendFramebufferSample {
            rgba: [0, 0, 0, 255],
            coverage_count: 8,
        };

        let a = f32::from(COMBINED[3]) / 255.0;
        let closed_form = (f32::from(COMBINED[0]) * a + 0.0 * (1.0 - a)).round() as u8;

        // The selector walk: P resolves to the combined color (cycle 0's
        // `Combined`), M to the framebuffer; the `M == Framebuffer` arm
        // keeps P as the blender color and makes A the composite factor.
        let p = f32::from(COMBINED[0]);
        let final_alpha = a;
        let stepped = (p * final_alpha + 0.0 * (1.0 - final_alpha)).round() as u8;
        assert_eq!(
            closed_form, stepped,
            "the two derivations of the same composite must agree"
        );
        assert_eq!(closed_form, 223, "255 * 223/255 over a zero destination");

        let blended = blend_texrect_fragment(COMBINED, destination, wm2000_blend_state(), 0, 0)
            .expect("WM2000's mode is admitted");
        assert_eq!(blended[0..3], [223, 223, 223]);
        // **A corrected derivation, kept as a correction.** The first draft
        // expected 223 here by assuming the alpha channel composites the
        // same way RGB does. It does not: `blend_fragment` composites alpha
        // as `255 * final_alpha + memory_alpha * (1 - final_alpha)`
        // (`crates/fn64-render-reference/src/raster/blend.rs:232-236`) --
        // the *source* alpha term is the constant 255, not the fragment's
        // own 223. With the destination's alpha byte also 255 the result is
        // 255 for every `final_alpha`. The test caught the wrong
        // expectation; the implementation was right.
        let memory_alpha = f32::from(destination.rgba[3]);
        let derived_alpha = (255.0 * a + memory_alpha * (1.0 - a)).round() as u8;
        assert_eq!(derived_alpha, 255);
        assert_eq!(blended[3], derived_alpha);

        // Full destination coverage supplies RGBA16 bit 0 independently of
        // the blender's alpha result.
        let mut packed = [0u8; 2];
        write_pixel(ColorTargetFormat::Rgba16, &mut packed, blended, Coverage::FULL);
        let five = 223u16 >> 3;
        let expected = (five << 11) | (five << 6) | (five << 1) | 1;
        assert_eq!(u16::from_be_bytes(packed), expected);
        assert_eq!(
            expected, 0xdef7,
            "27 in all three channels, coverage bit set"
        );
        // Changing blended alpha cannot move bit 0; only destination coverage
        // can do that.
    }

    /// The unblended value the executor produced before this stage existed,
    /// asserted as a **contrast**, so a regression that silently drops the
    /// blender is a failing assertion rather than a quiet return to the
    /// old output.
    ///
    /// Asserted through [`blend_and_write_pixel`] -- **the executor's own
    /// per-pixel composition**, the exact function the sampling loop calls
    /// -- not through `blend_texrect_fragment` alone. Measured, not
    /// stylistic: while this test went through the lower helper, deleting
    /// the blender call from the pixel loop left this crate's entire suite
    /// green and was caught only by `fn64-abi`'s whole-image comparison.
    /// A mutant that survives is first a question about the test's reach.
    #[test]
    fn skipping_the_blender_would_produce_a_different_pixel() {
        const COMBINED: [u8; 4] = [255, 255, 255, 223];
        let mut unblended = [0u8; 2];
        write_pixel(
            ColorTargetFormat::Rgba16,
            &mut unblended,
            COMBINED,
            Coverage::FULL,
        );
        assert_eq!(
            u16::from_be_bytes(unblended),
            0xffff,
            "the pre-blend combiner output, which the port used to publish"
        );

        // A zero destination, which is what WM2000's Fill-cycle
        // `0x00010001` decodes to: RGB 0 with the coverage bit set.
        let mut stored = 0x0001u16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            COMBINED,
            wm2000_blend_state(),
            wm2000_stages(),
            0,
            0,
        )
        .expect("WM2000's mode is admitted");
        assert_ne!(
            u16::from_be_bytes(stored),
            u16::from_be_bytes(unblended),
            "running the blender must change this pixel; if it does not, the stage is not running"
        );
        assert_eq!(
            u16::from_be_bytes(stored),
            0xdef7,
            "the blended value derived in \
             `the_wm2000_composite_is_hand_derived_over_a_zero_destination`"
        );
    }

    /// The destination a pixel blends against is the **buffer being
    /// written**, not the caller's incoming resident bytes, so two writes
    /// to the same pixel compose serially the way the RDP's per-pixel
    /// pipeline does.
    ///
    /// Without this, reading `resident_bytes` instead would pass every
    /// other test in this module -- every one of them writes each pixel
    /// once.
    #[test]
    fn a_second_write_to_one_pixel_blends_against_the_first() {
        const COMBINED: [u8; 4] = [255, 255, 255, 223];
        let state = wm2000_blend_state();
        let mut stored = 0x0001u16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            COMBINED,
            state,
            wm2000_stages(),
            0,
            0,
        )
        .unwrap();
        let after_first = u16::from_be_bytes(stored);
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            COMBINED,
            state,
            wm2000_stages(),
            0,
            0,
        )
        .unwrap();
        let after_second = u16::from_be_bytes(stored);
        assert_ne!(
            after_first, after_second,
            "the second write must see the first's result as its destination"
        );

        // Hand-derived: the first write leaves 5-bit 27, which `read_pixel`
        // expands to `(27 << 3) | (27 >> 2)` = 222. Blending white at
        // 223/255 over 222 gives 222 + 33/255*... -- computed here rather
        // than quoted.
        let a = 223.0f32 / 255.0;
        let first_channel = ((27u8 << 3) | (27u8 >> 2)) as f32;
        let second = (255.0 * a + first_channel * (1.0 - a)).round() as u16;
        let five = second >> 3;
        assert_eq!(after_second >> 11, five);
    }

    /// Source and destination are **not** interchangeable in this
    /// composite. Swapping them is a standard mutation, and without this
    /// it survives on WM2000's own fixture only because its destination is
    /// zero -- so the witness deliberately uses a non-zero destination.
    ///
    /// **The first witness was wrong and is recorded as a correction.**
    /// Alpha 128 was chosen by hand; at `128/255 = 0.50196` the composite is
    /// symmetric to within a byte and the "swapped" result was *identical*
    /// (`[108, 150, 145]` both ways), so the mutation it was written to
    /// catch survived. Alpha 64 separates them (`[62, 175, 192]` vs
    /// `[154, 125, 98]`). A hand-picked witness near `a = 1/2` proves
    /// nothing here.
    #[test]
    fn the_blend_source_and_destination_are_not_interchangeable() {
        const COMBINED: [u8; 4] = [200, 100, 50, 64];
        let destination = BlendFramebufferSample {
            rgba: [16, 200, 240, 255],
            coverage_count: 8,
        };
        let state = wm2000_blend_state();
        let forward = blend_texrect_fragment(COMBINED, destination, state, 0, 0).unwrap();

        let swapped_combined = [
            destination.rgba[0],
            destination.rgba[1],
            destination.rgba[2],
            COMBINED[3],
        ];
        let swapped_destination = BlendFramebufferSample {
            rgba: [COMBINED[0], COMBINED[1], COMBINED[2], destination.rgba[3]],
            coverage_count: destination.coverage_count,
        };
        let reversed =
            blend_texrect_fragment(swapped_combined, swapped_destination, state, 0, 0).unwrap();
        assert_ne!(
            forward[0..3],
            reversed[0..3],
            "P and M are asymmetric: A weights the source and (1 - A) the destination"
        );

        // Hand-derived, both directions, at alpha 64/255.
        let a = f32::from(COMBINED[3]) / 255.0;
        let expect =
            |src: u8, dst: u8| (f32::from(src) * a + f32::from(dst) * (1.0 - a)).round() as u8;
        assert_eq!(forward[0], expect(200, 16));
        assert_eq!(forward[1], expect(100, 200));
        assert_eq!(forward[2], expect(50, 240));
    }

    /// Rounding, not truncation, and it is observable. The witness is
    /// chosen by exhaustive search below rather than guessed -- most
    /// (source, destination, alpha) triples round and truncate to the same
    /// byte, which is exactly why a spot check would have let the mutation
    /// live.
    #[test]
    fn the_blend_composite_rounds_rather_than_truncating() {
        let state = wm2000_blend_state();
        let mut witnesses = 0usize;
        for alpha in [1u8, 64, 128, 200, 223, 254] {
            for source in [0u8, 1, 7, 100, 200, 255] {
                for destination in [0u8, 3, 9, 128, 255] {
                    let a = f32::from(alpha) / 255.0;
                    let exact = f32::from(source) * a + f32::from(destination) * (1.0 - a);
                    let rounded = exact.round() as u8;
                    let truncated = exact as u8;
                    if rounded == truncated {
                        continue;
                    }
                    witnesses += 1;
                    let blended = blend_texrect_fragment(
                        [source, source, source, alpha],
                        BlendFramebufferSample {
                            rgba: [destination, destination, destination, 255],
                            coverage_count: 8,
                        },
                        state,
                        0,
                        0,
                    )
                    .unwrap();
                    assert_eq!(
                        blended[0], rounded,
                        "source {source} over destination {destination} at alpha {alpha}: \
                         the composite must round ({rounded}), not truncate ({truncated})"
                    );
                }
            }
        }
        assert!(
            witnesses > 0,
            "the sweep must contain at least one triple where rounding and truncation differ, \
             or it proves nothing about which one runs"
        );
    }

    /// `read_pixel`'s 5-bit expansion is the crate's existing one, asserted
    /// against **the fill executor's own decode** rather than against a
    /// literal.
    ///
    /// The round-trip test below cannot catch this on its own, and that is
    /// measured, not assumed: dropping the `>> 2` low-bit replication
    /// leaves `write_pixel`'s `>> 3` recovering the same five bits, so the
    /// round trip is preserved while every non-zero destination changes
    /// (5-bit 27 expands to 222 with the replication and 216 without). The
    /// mutant survived until this test existed.
    ///
    /// The authority is [`crate::targets::decode_fill_cycle_pixel`], which
    /// applies the identical `(value << 3) | (value >> 2)` to a fill colour
    /// (`targets/fill.rs`'s `expand_five`) and is itself the port of the
    /// oracle's `decode_16` (`fn64-render-reference/src/raster/draw.rs:130-142`).
    /// A destination the fill executor wrote must decode back to the colour
    /// the fill executor meant, or the two halves of a composed image
    /// disagree about their shared format.
    #[test]
    fn read_pixel_expands_five_bit_channels_the_way_the_fill_decode_does() {
        use crate::state::FillColor;
        use crate::targets::decode_fill_cycle_pixel;
        for five in 0u16..32 {
            // A fill colour whose even-column halfword carries `five` in
            // all three channels with the coverage bit set.
            let halfword = (five << 11) | (five << 6) | (five << 1) | 1;
            let fill_word = (u32::from(halfword) << 16) | u32::from(halfword);
            let from_fill = decode_fill_cycle_pixel(
                FillColor::from_wire(fill_word),
                ColorTargetFormat::Rgba16,
                0,
            );
            let from_read = read_pixel(ColorTargetFormat::Rgba16, &halfword.to_be_bytes());
            assert_eq!(
                [from_read.rgba[0], from_read.rgba[1], from_read.rgba[2]],
                [from_fill.red, from_fill.green, from_fill.blue],
                "read_pixel and the fill decode must expand 5-bit {five} identically"
            );
            // And reconciled against an independent derivation of the same
            // expansion, so a shared slip in both would still contradict.
            let value = five as u8;
            assert_eq!(from_read.rgba[0], (value << 3) | (value >> 2));
        }
        // The witness that separates the two expansions, named so a
        // "simplification" to `<< 3` fails loudly rather than silently.
        assert_eq!(
            read_pixel(ColorTargetFormat::Rgba16, &(27u16 << 11).to_be_bytes()).rgba[0],
            222,
            "5-bit 27 expands to 222, not 216: the low-bit replication is load bearing"
        );
    }

    /// `read_pixel` is the exact inverse of `write_pixel` on every value
    /// RGBA16 can hold, so a destination the executor wrote decodes back to
    /// the color it meant. Exhaustive over all 65,536 halfwords, not
    /// sampled.
    ///
    /// **Necessary but not sufficient** -- see
    /// `read_pixel_expands_five_bit_channels_the_way_the_fill_decode_does`
    /// for the expansion this round trip cannot see.
    #[test]
    fn read_pixel_inverts_write_pixel_over_every_rgba16_halfword() {
        for raw in 0u16..=u16::MAX {
            let stored = raw.to_be_bytes();
            let sample = read_pixel(ColorTargetFormat::Rgba16, &stored);
            let mut round_tripped = [0u8; 2];
            let coverage = if raw & 1 == 0 {
                Coverage::new(1)
            } else {
                Coverage::FULL
            };
            write_pixel(
                ColorTargetFormat::Rgba16,
                &mut round_tripped,
                sample.rgba,
                coverage,
            );
            assert_eq!(
                u16::from_be_bytes(round_tripped),
                raw,
                "read_pixel then write_pixel must be the identity on {raw:#06x}"
            );
        }
    }

    /// Copy cycle runs no blender at all -- `cycle_count() == 0` is the
    /// RDP's own bypass, not this executor declining to implement one. A
    /// mutation that blends in Copy cycle changes the pixel; this catches
    /// it.
    #[test]
    fn copy_cycle_passes_the_fragment_through_unblended() {
        let copy_mode = OtherMode::from_wire(2 << 20, WM2000_OTHER_MODE_LOW);
        assert_eq!(copy_mode.cycle_type(), CycleType::Copy);
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(copy_mode);
        assert_eq!(state.cycle_count(), 0);
        const TEXEL: [u8; 4] = [200, 100, 50, 128];
        let blended = blend_texrect_fragment(
            TEXEL,
            BlendFramebufferSample {
                rgba: [16, 200, 240, 255],
                coverage_count: 8,
            },
            state,
            0,
            0,
        )
        .unwrap();
        assert_eq!(blended, TEXEL, "Copy cycle blits the texel unchanged");
    }

    /// Each of the three admission refusals fires by name, and none of
    /// them fires on WM2000's own mode.
    #[test]
    fn every_unevaluatable_blender_mode_is_refused_by_name() {
        assert_eq!(require_blendable_mode(wm2000_blend_state()), Ok(()));

        // FORCE_BL clear (bit 14) with AA_EN set (bit 3) AND IM_RD set
        // (bit 6): the one case where `blend_enabled` rests on the coverage
        // count. All three conjuncts are required; see
        // `a_clear_image_read_settles_blend_enabled_without_any_coverage_count`
        // for the IM_RD-clear case this refusal must NOT claim.
        let no_force =
            OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW & !0x4000);
        assert!(!no_force.force_blend());
        assert!(no_force.antialias_enabled(), "WM2000's mode sets AA_EN");
        assert!(no_force.image_read_enabled(), "WM2000's mode sets IM_RD");
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(no_force);
        assert_eq!(
            require_blendable_mode(state),
            Err(TexrectExecutionError::BlendEnabledNotDerivable)
        );

        // **The narrowing, pinned.** FORCE_BL clear AND AA_EN clear is
        // admitted, because `force_blend() || (antialias_enabled() &&
        // !wraps)` is then `false` outright with no `wraps` consulted.
        // Refusing this case too was measured wrong: three composed
        // one-cycle fixtures in `production.rs` latch other-mode low `0`
        // and had executed correctly for the life of the texrect path.
        let no_force_no_aa = OtherMode::from_wire(
            WM2000_OTHER_MODE_HIGH,
            WM2000_OTHER_MODE_LOW & !0x4000 & !0x0008,
        );
        assert!(!no_force_no_aa.force_blend());
        assert!(!no_force_no_aa.antialias_enabled());
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(no_force_no_aa);
        assert_eq!(require_blendable_mode(state), Ok(()));
        // And that admitted mode bypasses the blender: `is_last &&
        // !blend_enabled` selects P, which is `Combined`, leaving the
        // fragment unchanged. Derived from the selector, not from a run.
        const FRAGMENT: [u8; 4] = [200, 100, 50, 64];
        assert_eq!(
            crate::blend::ResolvedBlendCycle::from_wire(no_force_no_aa.blender_cycle_1()).p,
            BlendColorInput::Combined
        );
        assert_eq!(
            blend_texrect_fragment(
                FRAGMENT,
                BlendFramebufferSample {
                    rgba: [16, 200, 240, 255],
                    coverage_count: 8,
                },
                state,
                0,
                0,
            )
            .unwrap()[0..3],
            FRAGMENT[0..3],
            "the no-FORCE_BL last-cycle bypass selects P = Combined unchanged"
        );

        // A = Shade: cycle 1's alpha_a is bits 26:27, encoding 2.
        let shade_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 26)) | (0x2 << 26);
        let shade = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, shade_low);
        assert_eq!(
            crate::blend::ResolvedBlendCycle::from_wire(shade.blender_cycle_1()).a,
            BlendAlphaInput::Shade
        );
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(shade);
        assert_eq!(
            require_blendable_mode(state),
            Err(TexrectExecutionError::UnsupportedBlendShadeAlpha)
        );

        // B = FramebufferAlpha: cycle 1's alpha_b is bits 18:19, encoding 1.
        let fba_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 18)) | (0x1 << 18);
        let fba = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, fba_low);
        assert_eq!(
            crate::blend::ResolvedBlendCycle::from_wire(fba.blender_cycle_1()).b,
            BlendBInput::FramebufferAlpha
        );
        let state =
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(fba);
        assert_eq!(
            require_blendable_mode(state),
            Err(TexrectExecutionError::UnsupportedBlendFramebufferAlpha)
        );
    }

    /// **D5, the second narrowing.** `BlendEnabledNotDerivable` claimed that
    /// `FORCE_BL` clear with `AA_EN` set always rests on a coverage count.
    /// It does not. The reference's own definition
    /// (`fn64-render-reference/src/raster/coverage.rs:68-69`) is
    ///
    /// ```text
    /// wraps         = image_read_enabled && sum > 8
    /// blend_enabled = force_blend || (antialias_enabled && !wraps)
    /// ```
    ///
    /// and `wraps` is a **conjunction whose first term is `image_read`**. A
    /// clear `IM_RD` therefore pins `wraps` to `false` without the sum being
    /// evaluated at all, and `blend_enabled` collapses to
    /// `antialias_enabled()`, which this branch already knows is `true`. No
    /// coverage count on either side is read, so the stated reason for the
    /// refusal — "needs the destination coverage count this executor does
    /// not maintain" — is simply not true of this case.
    ///
    /// Hand-derived, not captured: the expectation below is read off the
    /// two-line formula above, and the blended pixel off the resolved
    /// selectors, never off a recorded run.
    #[test]
    fn a_clear_image_read_settles_blend_enabled_without_any_coverage_count() {
        // FORCE_BL clear (bit 14), AA_EN set (bit 3), IM_RD clear (bit 6).
        let no_force_no_read = OtherMode::from_wire(
            WM2000_OTHER_MODE_HIGH,
            (WM2000_OTHER_MODE_LOW & !0x4000) & !0x0040,
        );
        assert!(!no_force_no_read.force_blend());
        assert!(no_force_no_read.antialias_enabled());
        assert!(!no_force_no_read.image_read_enabled());
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(no_force_no_read);

        // Before this narrowing this returned `BlendEnabledNotDerivable`.
        assert_eq!(
            require_blendable_mode(state),
            Ok(()),
            "IM_RD clear pins `wraps` false, so `blend_enabled` is exactly \
             `antialias_enabled()` with no coverage count consulted"
        );

        // **The KEPT arm, pinned in the same test.** Setting IM_RD back —
        // and changing nothing else — must still refuse, because then and
        // only then does `wraps` depend on `pixel + memory > 8`. Without
        // this assertion, deleting the whole condition would pass.
        let read_enabled = OtherMode::from_wire(
            WM2000_OTHER_MODE_HIGH,
            (WM2000_OTHER_MODE_LOW & !0x4000) | 0x0040,
        );
        assert!(read_enabled.image_read_enabled());
        let refused = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(read_enabled);
        assert_eq!(
            require_blendable_mode(refused),
            Err(TexrectExecutionError::BlendEnabledNotDerivable),
            "with IM_RD set, `wraps` rests on a sum this executor cannot form"
        );

        // **`IM_RD` clear also removes the destination itself.** WM2000's
        // own cycle 1 selects `M = Framebuffer`, and with no image read
        // there is legally no destination sample, so `blend_fragment`
        // refuses by name rather than substituting one. That is the RDP's
        // rule, not a gap: the widening admits the *mode*, and this
        // orthogonal refusal still fires on the *program*.
        let cycle = crate::blend::ResolvedBlendCycle::from_wire(no_force_no_read.blender_cycle_1());
        assert_eq!(cycle.m, BlendColorInput::Framebuffer);
        const FRAGMENT: [u8; 4] = [200, 100, 50, 64];
        const DESTINATION: [u8; 4] = [16, 200, 240, 255];
        let sample = BlendFramebufferSample {
            rgba: DESTINATION,
            coverage_count: 8,
        };
        assert!(
            matches!(
                blend_texrect_fragment(FRAGMENT, sample, state, 0, 0),
                Err(TexrectExecutionError::Blend { .. })
            ),
            "a framebuffer term with IM_RD clear is refused by the blender, \
             not answered with an invented destination"
        );

        // **Positive control: the widened mode actually runs the mux.**
        // Admitting it would be worthless if every admitted fragment then
        // bypassed the blender. Swap `M` (cycle 1's color_b, bits 22:23) to
        // `Blend` (encoding 2) so the second term is a register rather than
        // the absent framebuffer, and supply that register.
        let mixing_low = ((WM2000_OTHER_MODE_LOW & !0x4000) & !0x0040 & !(0x3 << 22)) | (0x2 << 22);
        let mixing = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, mixing_low);
        assert!(!mixing.force_blend());
        assert!(mixing.antialias_enabled());
        assert!(!mixing.image_read_enabled());
        let resolved = crate::blend::ResolvedBlendCycle::from_wire(mixing.blender_cycle_1());
        assert_eq!(resolved.p, BlendColorInput::Combined);
        assert_eq!(resolved.m, BlendColorInput::Blend);
        const BLEND_REGISTER: [u8; 4] = [16, 200, 240, 255];
        let blend_color = Color4::from_wire(u32::from_be_bytes(BLEND_REGISTER));
        assert_eq!(blend_color.rgba8(), BLEND_REGISTER);
        let mixing_state =
            TexrectBlendRegisters::new(blend_color, Color4::from_wire(0)).mode_state(mixing);
        assert_eq!(require_blendable_mode(mixing_state), Ok(()));

        let blended = blend_texrect_fragment(FRAGMENT, sample, mixing_state, 0, 0)
            .expect("the widened mode evaluates");
        assert_ne!(
            blended[0..3],
            FRAGMENT[0..3],
            "an admitted-but-inert mode would prove nothing; with \
             `blend_enabled` true the last cycle must NOT take the \
             `is_last && !blend_enabled` P-passthrough"
        );
        // And it mixed the blend register in, not an invented constant:
        // every channel lands between the two operands.
        for channel in 0..3 {
            let low = FRAGMENT[channel].min(BLEND_REGISTER[channel]);
            let high = FRAGMENT[channel].max(BLEND_REGISTER[channel]);
            assert!(
                (low..=high).contains(&blended[channel]),
                "channel {channel}: {} is outside [{low}, {high}]",
                blended[channel]
            );
        }

        // **The kept arm's other half, pinned.** Reverting `blend_enabled`
        // to the old `force_blend()` alone would make this same program
        // bypass the mux and return the fragment unchanged. Deriving the
        // expected P-passthrough from the selector proves the two answers
        // are actually distinguishable, so the assertion above is not
        // vacuous.
        assert_eq!(
            crate::blend::ResolvedBlendCycle::from_wire(mixing.blender_cycle_1()).p,
            BlendColorInput::Combined,
            "the bypass this fix avoids would have returned P = Combined, \
             i.e. the fragment unchanged"
        );
    }

    /// A blender cycle reading a never-written `SetBlendColor`/
    /// `SetFogColor` gets the register's power-on zero, not a refusal.
    ///
    /// This replaces a test that asserted the refusal. The registers always
    /// hold a value: `fn64-render-reference` zero-initializes both
    /// (`gbi/state.rs:227-228`, `:387-388`) and RT64's C++ does the same at
    /// `src/hle/rt64_state.cpp:130-131`.
    #[test]
    fn a_never_written_blender_register_reads_as_zero_instead_of_refusing() {
        // P = Blend is cycle 1's color_a (bits 30:31) encoding 2.
        let blend_low = (WM2000_OTHER_MODE_LOW & !(0x3u32 << 30)) | (0x2u32 << 30);
        let mode = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, blend_low);
        // Derived by hand: an unwritten register holds four zero bytes.
        let unwritten =
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(mode);
        assert_eq!(unwritten.blend_color_register, [0, 0, 0, 0]);

        // A written register must reach `BlendModeState` unchanged, or the
        // assertion above could hold against a hardcoded zero. `rgba8`
        // unpacks the wire word big-endian, so 0x1122_3344 is [0x11, 0x22,
        // 0x33, 0x44] -- derived from the wire layout, not from the code
        // under test.
        let written =
            TexrectBlendRegisters::new(Color4::from_wire(0x1122_3344), Color4::from_wire(0))
                .mode_state(mode);
        assert_eq!(written.blend_color_register, [0x11, 0x22, 0x33, 0x44]);

        // A = Fog is cycle 1's alpha_a (bits 26:27) encoding 1.
        let fog_low = (WM2000_OTHER_MODE_LOW & !(0x3 << 26)) | (0x1 << 26);
        let mode = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, fog_low);
        let unwritten_fog =
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(mode);
        assert_eq!(unwritten_fog.fog_color, [0, 0, 0, 0]);
        let written_fog =
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0x5566_7788))
                .mode_state(mode);
        assert_eq!(written_fog.fog_color, [0x55, 0x66, 0x77, 0x88]);

        // WM2000's own cycle reads neither register; both still carry their
        // real (zero) contents.
        let wm2000 = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(wm2000_other_mode());
        assert_eq!(wm2000.blend_color_register, [0, 0, 0, 0]);
        assert_eq!(wm2000.fog_color, [0, 0, 0, 0]);
    }

    /// `IM_RD` disabled with a `Framebuffer` selector is propagated as a
    /// named error, never substituted with a zero destination.
    #[test]
    fn a_framebuffer_selector_without_image_read_is_refused_by_name() {
        let no_read = OtherMode::from_wire(WM2000_OTHER_MODE_HIGH, WM2000_OTHER_MODE_LOW & !0x0040);
        assert!(!no_read.image_read_enabled());
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(no_read);
        let error = blend_texrect_fragment(
            [255, 255, 255, 223],
            BlendFramebufferSample {
                rgba: [0, 0, 0, 255],
                coverage_count: 8,
            },
            state,
            7,
            9,
        )
        .expect_err("a Framebuffer selector with IM_RD clear has no legal destination");
        let TexrectExecutionError::Blend {
            column,
            row,
            source,
        } = error
        else {
            panic!("expected the blender's own refusal, got {error:?}");
        };
        assert_eq!((column, row), (7, 9), "the refusal names the pixel");
        assert_eq!(source.selector, "framebuffer color");
    }
}

/// The three post-combiner stages this card wired into the executor
/// alongside the blender: coverage, alpha compare and alpha dither.
///
/// **None of these is validated by the WM2000 oracle comparison, and that
/// is stated rather than implied.** All four captured entries latch
/// `alpha_compare = G_AC_NONE`, `CVG_X_ALPHA` and `ALPHA_CVG_SEL` clear,
/// and `alpha_dither = Noise` -- so the comparison would not detect a
/// defect in the alpha-compare gate or in the coverage-alpha interaction
/// at all, and detects the dither stage only as the endpoint it already
/// produced. Every expectation below is therefore hand-derived twice and
/// reconciled, because no differential covers it.
#[cfg(test)]
mod fragment_stage_tests {
    use super::*;
    use crate::state::CoverageDestination;

    const WM2000_HIGH: u32 = 0x0000_acef;
    const WM2000_LOW: u32 = 0x0050_41c8;

    fn mode(high: u32, low: u32) -> OtherMode {
        OtherMode::from_wire(high, low)
    }

    /// **Positive control for the whole module**: WM2000's captured words
    /// decode to the stage modes every expectation below assumes, and each
    /// field is reconciled against an independent derivation from the same
    /// literal.
    #[test]
    fn wm2000_frame_zero_stage_modes_decode_as_derived() {
        let m = mode(WM2000_HIGH, WM2000_LOW);
        assert_eq!(m.alpha_compare(), AlphaCompare::None);
        assert_eq!(WM2000_LOW & 0x3, 0, "G_AC is other-mode low bits 0:1");
        assert_eq!(m.alpha_dither(), AlphaDither::Noise);
        assert_eq!((WM2000_HIGH >> 4) & 0x3, 2, "alpha dither is high bits 4:5");
        assert_eq!(m.rgb_dither(), RgbDither::Disabled);
        assert_eq!((WM2000_HIGH >> 6) & 0x3, 3, "RGB dither is high bits 6:7");
        assert!(!m.coverage_times_alpha());
        assert_eq!(WM2000_LOW & 0x1000, 0, "CVG_X_ALPHA is low bit 12");
        assert!(!m.alpha_coverage_select());
        assert_eq!(WM2000_LOW & 0x2000, 0, "ALPHA_CVG_SEL is low bit 13");
        assert_eq!(m.coverage_destination(), CoverageDestination::Wrap);
        assert_eq!((WM2000_LOW >> 8) & 0x3, 1, "cvg_dst is low bits 8:9");

        TexrectFragmentStages::try_new(m, Color4::from_wire(0))
            .expect("every WM2000 stage mode is admitted");
    }

    /// **The `blend_cycle_count` hazard, settled: the two counts are not
    /// in conflict, they answer different questions.**
    ///
    /// `rt64_blender_analysis::blend_cycle_count` returns
    /// `combine_cycle_count - 1` without `FORCE_BL`, while
    /// `BlendModeState::cycle_count` returns 1/2/0 straight from
    /// `cycle_type()`. They disagree numerically for every
    /// non-`force_blend` mode, which reads like a defect and is not one:
    ///
    /// - `blend_cycle_count` counts the cycles that **actually blend**.
    ///   Its consumers are the `uses_*` predicates, which ask "does any
    ///   blending cycle read this input?" -- and a bypassed last cycle
    ///   reads only `P`, never `A`/`B`, so excluding it is correct.
    /// - `cycle_count` counts the **loop iterations** `blend_fragment`
    ///   runs. That loop handles the bypass internally via
    ///   `is_last && !blend_enabled`, so it must still visit the cycle it
    ///   bypasses in order to resolve `P`.
    ///
    /// Both are faithful ports of differently-purposed upstream
    /// functions. This test pins the numeric disagreement **and** the
    /// reconciliation, so neither can be "fixed" into the other.
    #[test]
    fn the_two_cycle_counts_disagree_by_design_and_the_reason_is_pinned() {
        use crate::rt64_blender_analysis::{blend_cycle_count, combine_cycle_count};

        // FORCE_BL clear: the two disagree, by exactly one.
        let no_force = mode(WM2000_HIGH, WM2000_LOW & !0x4000);
        assert!(!no_force.force_blend());
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(no_force);
        assert_eq!(combine_cycle_count(no_force), 1);
        assert_eq!(blend_cycle_count(no_force), 0, "no cycle actually blends");
        assert_eq!(state.cycle_count(), 1, "one loop iteration still runs");

        // FORCE_BL set -- WM2000's own mode -- and they agree, which is
        // why the disagreement is unreachable for this packet.
        let forced = mode(WM2000_HIGH, WM2000_LOW);
        assert!(forced.force_blend());
        let state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(forced);
        assert_eq!(blend_cycle_count(forced), 1);
        assert_eq!(u32::from(state.cycle_count()), blend_cycle_count(forced));

        // The reconciliation, asserted rather than asserted-about: the
        // single iteration the loop runs under a cleared FORCE_BL is the
        // bypass, and it leaves the fragment's colour at P = Combined.
        let blended = blend_texrect_fragment(
            [200, 100, 50, 64],
            BlendFramebufferSample {
                rgba: [16, 200, 240, 255],
                coverage_count: 8,
            },
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
                .mode_state(mode(WM2000_HIGH, WM2000_LOW & !0x4000 & !0x0008)),
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            blended[0..3],
            [200, 100, 50],
            "the zero blending cycles blend_cycle_count reports is what the bypass produces"
        );
    }

    /// **The endpoint proof `NOISE_DITHER_THRESHOLD` rests on.**
    ///
    /// Exhaustive over all 256 alpha values and all 8 thresholds: the
    /// dithered five-bit alpha takes exactly two values, `floor` and
    /// `floor + 1`, and threshold 7 always selects `floor`. So the
    /// executor's constant is a member of the mode's real output set, not
    /// a third value between the two. Derived here from the arithmetic
    /// rather than read off `apply_alpha_dither`, then reconciled against
    /// it.
    #[test]
    fn the_noise_dither_threshold_is_an_endpoint_not_an_invention() {
        assert_eq!(
            NOISE_DITHER_THRESHOLD.dither(),
            7,
            "the maximum 3-bit threshold"
        );
        for alpha in 0u8..=255 {
            let floor = u16::from(alpha >> 3);
            let mut seen = std::collections::BTreeSet::new();
            for threshold in 0u8..8 {
                // Re-derived from `apply_alpha_dither`'s documented
                // arithmetic, independently of the function itself.
                let rounded = floor + u16::from((alpha & 7) > threshold);
                seen.insert(rounded.min(31));
            }
            assert!(
                seen.len() <= 2 && *seen.iter().next().unwrap() == floor.min(31),
                "alpha {alpha}: the mode's output set must be {{floor, floor+1}}, got {seen:?}"
            );
            assert!(
                seen.iter().all(|&v| v.abs_diff(floor.min(31)) <= 1),
                "alpha {alpha}: dither must never move the channel by more than one step"
            );

            // And the function itself, at threshold 7, must equal the
            // undithered floor re-expanded.
            let five = (floor.min(31)) as u8;
            assert_eq!(
                apply_alpha_dither(
                    alpha,
                    AlphaDither::Noise,
                    RgbDither::Disabled,
                    0,
                    0,
                    NOISE_DITHER_THRESHOLD
                ),
                (five << 3) | (five >> 2),
                "alpha {alpha} at the maximum threshold must be the undithered floor"
            );
        }
    }

    /// `wraps` does not need the two hidden coverage bits **for a
    /// full-coverage fragment**, which is what a texrect always produces.
    ///
    /// Derived two ways and reconciled: once by enumerating every value
    /// the stored count can hold (`Coverage::from_stored` is
    /// `(stored & 7) + 1`, so `1..=8`), and once from the inequality
    /// `8 + memory > 8` being true for all `memory >= 1`.
    #[test]
    fn wraps_is_determined_for_a_full_coverage_fragment() {
        let bits = CoverageModeBits {
            image_read_enabled: true,
            force_blend: true,
            antialias_enabled: true,
            coverage_destination: CoverageDestination::Wrap,
        };
        // Enumeration.
        for stored in 0u8..8 {
            let memory = Coverage::from_stored(stored);
            assert!((1..=8).contains(&memory.count()));
            let result = coverage_result(Coverage::FULL, memory, bits);
            assert!(
                result.wraps,
                "stored {stored} (count {}) must still wrap under a full-coverage fragment",
                memory.count()
            );
            assert!(result.blend_enabled);
        }
        // The inequality, stated independently of the loop.
        assert!(Coverage::FULL.count() + 1 > Coverage::FULL.count());

        // And the executor's own accessor agrees, for the mode WM2000
        // latches.
        let stages =
            TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
                .unwrap();
        let result = stages
            .coverage_for(Coverage::FULL, Coverage::FULL)
            .unwrap();
        assert!(result.wraps);
        assert!(result.blend_enabled);
    }

    /// Image-read Save still exposes the unavailable three-bit destination
    /// coverage. A partial fragment with image read is likewise refused
    /// because its wrap state is ambiguous.
    #[test]
    fn the_modes_that_expose_the_missing_coverage_bits_are_refused_by_name() {
        // cvg_dst = Save is low bits 8:9 == 3.
        let save = mode(WM2000_HIGH, (WM2000_LOW & !(0x3 << 8)) | (0x3 << 8));
        assert_eq!(save.coverage_destination(), CoverageDestination::Save);
        let stages = TexrectFragmentStages::try_new(save, Color4::from_wire(0)).unwrap();
        assert_eq!(
            stages.coverage_for(Coverage::FULL, Coverage::FULL),
            Err(TexrectExecutionError::DestinationCoverageUnavailable {
                consumer: "cvg_dst = Save"
            })
        );

        let stages =
            TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
                .unwrap();
        assert_eq!(
            stages.coverage_for(Coverage::new(4), Coverage::FULL),
            Err(TexrectExecutionError::DestinationCoverageUnavailable {
                consumer: "a partial-coverage fragment's cvg_dst accumulation"
            })
        );

        // With image read disabled the destination count is never read at
        // all, so even `Save` is admitted -- the refusal is about
        // observability, not about the mode's name.
        let no_read = mode(WM2000_HIGH, (WM2000_LOW & !0x40 & !(0x3 << 8)) | (0x3 << 8));
        assert!(!no_read.image_read_enabled());
        let stages = TexrectFragmentStages::try_new(no_read, Color4::from_wire(0)).unwrap();
        assert!(stages.coverage_for(Coverage::new(4), Coverage::FULL).is_ok());
    }

    /// The visible RGBA16 coverage bit follows the post-accumulation coverage
    /// destination, not fragment alpha. Programming Manual §§15.5.3, 15.5.6,
    /// and 15.7 define the stored `count - 1` encoding; RT64's
    /// `Float4ToRGBA16` independently extracts its bit 2.
    #[test]
    fn rgba16_bit_zero_follows_each_coverage_destination_mode() {
        let cases = [
            (CoverageDestination::Clamp, 1u16),
            (CoverageDestination::Wrap, 0),
            (CoverageDestination::Full, 1),
            (CoverageDestination::Save, 1),
        ];
        for (destination, expected_bit) in cases {
            let result = coverage_result(
                Coverage::new(4),
                Coverage::FULL,
                CoverageModeBits {
                    image_read_enabled: true,
                    force_blend: true,
                    antialias_enabled: false,
                    coverage_destination: destination,
                },
            );
            let mut packed = [0u8; 2];
            write_pixel(
                ColorTargetFormat::Rgba16,
                &mut packed,
                [0, 0, 0, 0],
                result.destination,
            );
            assert_eq!(u16::from_be_bytes(packed) & 1, expected_bit);
        }

        let (selected, coverage) =
            apply_coverage_alpha(false, true, [0, 0, 0, 0], Coverage::new(4));
        assert_eq!(selected[3], coverage.alpha());
        assert_eq!(coverage, Coverage::new(4));
        let mut packed = [0u8; 2];
        write_pixel(ColorTargetFormat::Rgba16, &mut packed, selected, coverage);
        assert_eq!(u16::from_be_bytes(packed) & 1, 0);

        let (unselected, coverage) =
            apply_coverage_alpha(false, false, [0, 0, 0, 0], Coverage::FULL);
        assert_eq!(unselected[3], 0);
        let full = coverage_result(
            coverage,
            Coverage::new(1),
            CoverageModeBits {
                image_read_enabled: false,
                force_blend: false,
                antialias_enabled: false,
                coverage_destination: CoverageDestination::Full,
            },
        );
        write_pixel(
            ColorTargetFormat::Rgba16,
            &mut packed,
            unselected,
            full.destination,
        );
        assert_eq!(u16::from_be_bytes(packed) & 1, 1);
    }

    /// The alpha-compare gate, hand-derived at the threshold boundary in
    /// both directions.
    ///
    /// `G_AC_THRESHOLD` passes iff `alpha >= G_SETBLENDCOLOR.a`, so `a-1`
    /// rejects, `a` passes and `a+1` passes. `G_AC_NONE` passes
    /// everything, including alpha 0.
    #[test]
    fn the_alpha_compare_gate_is_hand_derived_at_its_boundary() {
        // G_AC_NONE: WM2000's own mode.
        let stages =
            TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
                .unwrap();
        for alpha in [0u8, 1, 128, 255] {
            assert!(
                alpha_compare_texrect_fragment(stages, alpha).unwrap(),
                "G_AC_NONE must pass alpha {alpha}"
            );
        }

        // G_AC_THRESHOLD (low bits 0:1 == 1) against SetBlendColor alpha.
        const THRESHOLD: u8 = 0x80;
        let threshold_mode = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x1);
        assert_eq!(threshold_mode.alpha_compare(), AlphaCompare::Threshold);
        let blend_color = Color4::from_wire(0x0000_0000 | u32::from(THRESHOLD));
        assert_eq!(
            blend_color.rgba8()[3],
            THRESHOLD,
            "the wire's low byte is alpha"
        );
        let stages = TexrectFragmentStages::try_new(threshold_mode, blend_color).unwrap();
        assert!(!alpha_compare_texrect_fragment(stages, THRESHOLD - 1).unwrap());
        assert!(alpha_compare_texrect_fragment(stages, THRESHOLD).unwrap());
        assert!(alpha_compare_texrect_fragment(stages, THRESHOLD + 1).unwrap());

        // Threshold with no SetBlendColor staged compares against the
        // register's power-on zero. `alpha >= 0` holds for every alpha, so
        // every fragment passes -- derived by hand from the comparison
        // `alpha >= threshold_alpha`, and it is exactly what the reference
        // lane computes (`raster/blend.rs:113` against the zero-initialized
        // `other_mode.blend_color_alpha`).
        let unwritten =
            TexrectFragmentStages::try_new(threshold_mode, Color4::from_wire(0)).unwrap();
        for alpha in [0u8, 1, 0x7f, THRESHOLD - 1, THRESHOLD, 0xff] {
            assert!(
                alpha_compare_texrect_fragment(unwritten, alpha).unwrap(),
                "alpha {alpha:#04x} must pass a Threshold compare against the power-on zero"
            );
        }
        // ...and the written register must still reject below its own
        // threshold, or the sweep above could pass against a comparator
        // that ignores the register entirely.
        assert!(!alpha_compare_texrect_fragment(stages, THRESHOLD - 1).unwrap());
    }

    /// A rejected fragment writes **nothing** -- the destination keeps its
    /// prior value rather than being overwritten with a blended one.
    #[test]
    fn an_alpha_compare_rejection_leaves_the_destination_untouched() {
        const THRESHOLD: u8 = 0xc0;
        let threshold_mode = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x1);
        let stages =
            TexrectFragmentStages::try_new(threshold_mode, Color4::from_wire(u32::from(THRESHOLD)))
                .unwrap();
        let blend_state = TexrectBlendRegisters::new(
            Color4::from_wire(u32::from(THRESHOLD)),
            Color4::from_wire(0),
        )
        .mode_state(threshold_mode);

        let mut stored = 0x0001u16.to_be_bytes();
        // Alpha below the threshold: rejected, nothing written.
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [255, 255, 255, THRESHOLD - 1],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            u16::from_be_bytes(stored),
            0x0001,
            "a rejected fragment must not write"
        );

        // Alpha at the threshold: accepted, and the pixel changes.
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [255, 255, 255, THRESHOLD],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();
        assert_ne!(
            u16::from_be_bytes(stored),
            0x0001,
            "an accepted fragment must write"
        );
    }

    /// `CVG_X_ALPHA` and `ALPHA_CVG_SEL` are independent bits with
    /// independent effects, hand-derived from `Coverage`'s own encodings.
    ///
    /// With full coverage, `ALPHA_CVG_SEL` overwrites the fragment alpha
    /// with `Coverage::FULL.alpha()` = `(8*255 + 4) / 8` = 255, and
    /// `CVG_X_ALPHA` multiplies coverage by the fragment alpha first.
    #[test]
    fn the_two_coverage_alpha_bits_are_independent_and_hand_derived() {
        assert_eq!(Coverage::FULL.alpha(), 255);
        assert_eq!(
            (8u16 * 255 + 4) / 8,
            255,
            "derived independently of Coverage::alpha"
        );
        // Half alpha times full coverage: (8*128 + 127) / 255 = 4.
        assert_eq!(Coverage::FULL.times_alpha(128).count(), 4);
        assert_eq!((8u16 * 128 + 127) / 255, 4, "derived independently");

        let rgba = [10u8, 20, 30, 128];
        // Neither bit: pass-through.
        let (out, cvg) = apply_coverage_alpha(false, false, rgba, Coverage::FULL);
        assert_eq!(out, rgba);
        assert_eq!(cvg, Coverage::FULL);
        // ALPHA_CVG_SEL only: alpha becomes the coverage encoding.
        let (out, cvg) = apply_coverage_alpha(false, true, rgba, Coverage::FULL);
        assert_eq!(out[3], 255);
        assert_eq!(cvg, Coverage::FULL);
        // CVG_X_ALPHA only: coverage shrinks, alpha is untouched.
        let (out, cvg) = apply_coverage_alpha(true, false, rgba, Coverage::FULL);
        assert_eq!(out[3], 128);
        assert_eq!(cvg.count(), 4);
        // Both: coverage shrinks first, then alpha takes the shrunk value.
        let (out, cvg) = apply_coverage_alpha(true, true, rgba, Coverage::FULL);
        assert_eq!(cvg.count(), 4);
        assert_eq!(out[3], Coverage::new(4).alpha());
        assert_eq!(
            out[3],
            ((4u16 * 255 + 4) / 8) as u8,
            "derived independently"
        );
    }

    /// **The coverage-alpha stage runs inside the pixel loop**, asserted
    /// through [`blend_and_write_pixel`] rather than through
    /// `apply_coverage_alpha` alone.
    ///
    /// **The first witness for this was degenerate and is recorded as a
    /// correction.** It used `CVG_X_ALPHA` with a zero fragment alpha,
    /// expecting no write; but a zero alpha makes the blend composite a
    /// pure destination pass-through, so skipping the stage entirely
    /// produced the *same* stored halfword and the mutant survived.
    /// `ALPHA_CVG_SEL` separates them: with full coverage it *raises*
    /// alpha to `Coverage::FULL.alpha()` = 255, so a fragment alpha of 64
    /// blends to 5-bit 31 with the stage and 5-bit 8 without it.
    #[test]
    fn the_coverage_alpha_stage_runs_inside_the_pixel_loop() {
        // ALPHA_CVG_SEL is low bit 13.
        let cvg_sel = mode(WM2000_HIGH, WM2000_LOW | 0x2000);
        assert!(cvg_sel.alpha_coverage_select());
        let stages = TexrectFragmentStages::try_new(cvg_sel, Color4::from_wire(0)).unwrap();
        let blend_state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(cvg_sel);

        let mut stored = 0x0001u16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [255, 255, 255, 64],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();

        // Hand-derived both ways. With the stage: alpha becomes
        // Coverage::FULL.alpha() = 255, so the composite is 255 * 1 + 0 = 255
        // -> 5-bit 31. Without it: 255 * 64/255 + 0 = 64 -> 5-bit 8.
        let with_stage = (255.0f32 * (255.0 / 255.0)).round() as u16 >> 3;
        let without_stage = (255.0f32 * (64.0 / 255.0)).round() as u16 >> 3;
        assert_eq!((with_stage, without_stage), (31, 8), "the two must differ");
        assert_eq!(
            u16::from_be_bytes(stored) >> 11,
            with_stage,
            "ALPHA_CVG_SEL must have raised the fragment alpha before the blend"
        );
    }

    /// Zero coverage writes nothing. Reachable only through
    /// `CVG_X_ALPHA` with a zero fragment alpha, which is why the witness
    /// sets that bit rather than asserting on an unreachable state.
    ///
    /// **Necessary but not sufficient**, and deliberately kept alongside
    /// the test above rather than replaced by it: a zero fragment alpha
    /// makes the blend a destination pass-through, so this cannot on its
    /// own distinguish "did not write" from "wrote the same value".
    #[test]
    fn a_zero_coverage_fragment_writes_nothing() {
        // CVG_X_ALPHA is low bit 12.
        let cvg_x_alpha = mode(WM2000_HIGH, WM2000_LOW | 0x1000);
        assert!(cvg_x_alpha.coverage_times_alpha());
        assert_eq!(Coverage::FULL.times_alpha(0).count(), 0);
        let stages = TexrectFragmentStages::try_new(cvg_x_alpha, Color4::from_wire(0)).unwrap();
        let blend_state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(cvg_x_alpha);
        let mut stored = 0x0001u16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [255, 255, 255, 0],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();
        assert_eq!(u16::from_be_bytes(stored), 0x0001);
    }

    /// `CLR_ON_CVG` + `CVG_DST_WRAP` with `IM_RD`/`AA_EN`/`FORCE_BL` clear:
    /// a FULL-coverage fragment MUST be written, matching angrylion + RT64
    /// (the `gen-coverage-color-on-cvg-one-cycle` parity case). This pins the
    /// write decision against the pre-fix `!coverage.wraps` gate, which
    /// short-circuited `wraps` to `false` on clear `IM_RD` and dropped the
    /// write -- reverting to that gate re-drops this pixel and fails here.
    ///
    /// The hardware rule (angrylion `blender_1cycle`): `color_on_cvg` never
    /// gates the color write itself; the write is gated by the coverage
    /// carry-out (`prewrap = (memcvg + cvg) & 8`, `memcvg = 0` with no
    /// `IM_RD` read), which a full-coverage fragment (`cvg = 8`) always sets.
    #[test]
    fn clr_on_cvg_with_wrap_writes_a_full_coverage_fragment_without_image_read() {
        // Only CLR_ON_CVG (bit 7) + CVG_DST_WRAP (bits 9:8 == 1): no IM_RD,
        // no AA_EN, no FORCE_BL, no alpha compare, no coverage-alpha bits.
        let low = 0x080 | 0x100;
        let m = mode(0, low);
        assert!(m.clear_on_coverage(), "CLR_ON_CVG must be set");
        assert_eq!(m.coverage_destination(), CoverageDestination::Wrap);
        assert!(!m.image_read_enabled(), "IM_RD must be clear for this case");
        assert!(!m.antialias_enabled(), "AA_EN must be clear for this case");
        assert!(!m.force_blend(), "FORCE_BL must be clear for this case");

        let stages = TexrectFragmentStages::try_new(m, Color4::from_wire(0)).unwrap();
        let blend_state =
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)).mode_state(m);

        // Seed STALE (0xffff); a distinct opaque combined color must land.
        let mut stored = 0xffffu16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [0x20, 0x40, 0x60, 0xff],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();
        assert_ne!(
            u16::from_be_bytes(stored),
            0xffff,
            "CLR_ON_CVG + CVG_DST_WRAP must WRITE a full-coverage fragment \
             (angrylion + RT64 both do); the pixel stayed STALE"
        );
    }

    /// Every mode this card refuses, refused by name and distinguishable
    /// from every other refusal.
    #[test]
    fn every_unevaluatable_stage_mode_is_refused_by_name() {
        // G_AC wire encoding 2 is NOT refused: pinned RT64 branches only for
        // `G_AC_DITHER` and `G_AC_THRESHOLD`, so encoding 2 performs no
        // compare
        // (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/shaders/RasterPS.hlsl:203-213`).
        // Retargeted from the assertion that
        // this encoding raised `ReservedAlphaCompare`; see
        // `docs/RT64-GUARD-AUDIT.md` finding A3.
        let dither_bit_without_enable = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x2);
        assert_eq!(
            dither_bit_without_enable.alpha_compare(),
            AlphaCompare::None,
            "wire 2 sets dither_alpha_en but clears alpha_compare_en"
        );
        assert!(
            TexrectFragmentStages::try_new(dither_bit_without_enable, Color4::from_wire(0)).is_ok(),
            "no compare is not a refusal"
        );
        // Distinguishing check: wire 3 (both bits set) IS still refused, so
        // the `is_ok` above cannot be produced by an executor that admits
        // every alpha-compare mode.
        assert!(TexrectFragmentStages::try_new(
            mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x3),
            Color4::from_wire(0)
        )
        .is_err());

        // G_AC_DITHER (encoding 3) needs the per-pixel random value.
        let ac_dither = mode(WM2000_HIGH, (WM2000_LOW & !0x3) | 0x3);
        assert_eq!(ac_dither.alpha_compare(), AlphaCompare::Dither);
        assert_eq!(
            TexrectFragmentStages::try_new(ac_dither, Color4::from_wire(0)),
            Err(TexrectExecutionError::NoiseThresholdUnavailable {
                stage: TexrectNoiseStage::AlphaCompareDither
            })
        );

        // Alpha dither Pattern resolving to Bayer: RGB dither Disabled
        // (encoding 3) substitutes Bayer, whose tables the two ports
        // disagree about.
        let bayer = mode((WM2000_HIGH & !(0x3 << 4)) | (0x0 << 4), WM2000_LOW);
        assert_eq!(bayer.alpha_dither(), AlphaDither::Pattern);
        assert_eq!(bayer.rgb_dither(), RgbDither::Disabled);
        assert_eq!(
            TexrectFragmentStages::try_new(bayer, Color4::from_wire(0)),
            Err(TexrectExecutionError::OrderedDitherAuthorityUnsettled {
                stage: TexrectNoiseStage::AlphaDither,
                pattern: RgbDither::Bayer
            })
        );

        // The same Pattern resolving to MagicSquare instead IS admitted --
        // the two ports agree at all 16 of its cells.
        let magic = mode(
            (WM2000_HIGH & !(0x3 << 4) & !(0x3 << 6)) | (0x0 << 4) | (0x0 << 6),
            WM2000_LOW,
        );
        assert_eq!(magic.alpha_dither(), AlphaDither::Pattern);
        assert_eq!(magic.rgb_dither(), RgbDither::MagicSquare);
        assert!(TexrectFragmentStages::try_new(magic, Color4::from_wire(0)).is_ok());
    }

    /// **D7's premise, re-measured — and the refusal kept.**
    ///
    /// `docs/RT64-LANE-DIVERGENCES.md` D7 scored
    /// [`TexrectExecutionError::OrderedDitherAuthorityUnsettled`] a wgpu
    /// defect on the ground that the Bayer dispute lives in the *RGB*
    /// module while the alpha-dither stage read a separate,
    /// reference-identical table in `alpha_compare.rs` — so the cited
    /// authority conflict did not apply to the stage being refused.
    ///
    /// That premise was true at the audit's pin and is false now.
    /// `51b4e184` deleted the duplicate, because libultra defines
    /// `G_AD_PATTERN`'s threshold as *the currently selected RGB dither
    /// matrix* (`gbi.h:674-678`) and one hardware quantity must have one
    /// table. The alpha path now reads the disputed tile by construction.
    ///
    /// This test pins that, so the refusal cannot be re-litigated from the
    /// stale premise: it asserts (a) the alpha-dither threshold IS
    /// `rgb_dither`'s Bayer value, (b) that value differs from the
    /// reference's at the documented cells, and (c) the difference is
    /// observable in `apply_alpha_dither`'s own output — which is the only
    /// thing that makes the refusal load-bearing rather than fussy.
    ///
    /// Every expectation is hand-derived from the two tables and the
    /// published rounding rule, never captured.
    #[test]
    fn the_alpha_dither_refusal_is_downstream_of_the_one_disputed_tile() {
        // `fn64-render-reference`'s BAYER (`raster/blend.rs:30`), as a
        // literal so this test needs no cross-crate dependency. Same
        // constant `rgb_dither.rs`'s own disagreement test uses.
        const REFERENCE_BAYER: [[u8; 4]; 4] =
            [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];

        let mut disputed_cells = Vec::new();
        for y in 0..4i32 {
            for x in 0..4i32 {
                let ours = crate::alpha_compare::alpha_dither_pattern_threshold_for_tests(
                    RgbDither::Bayer,
                    x,
                    y,
                );
                // (a) The alpha path reads `rgb_dither`'s tile, not a
                // private copy. If the duplicate ever returns, this fails.
                assert_eq!(
                    ours,
                    crate::rgb_dither::ordered_tile_value(RgbDither::Bayer, x, y),
                    "the alpha-dither path must read rgb_dither's tile at ({x}, {y})"
                );
                if ours != REFERENCE_BAYER[y as usize][x as usize] {
                    disputed_cells.push((x, y, ours, REFERENCE_BAYER[y as usize][x as usize]));
                }
            }
        }

        // (b) The dispute is real and reaches the alpha stage's own tile.
        assert!(
            !disputed_cells.is_empty(),
            "D7's refusal presumes a live disagreement; if the Bayer phase \
             has been settled, resolve the refusal rather than this test"
        );

        // (c) It is observable in alpha dither's output. The rounding rule
        // is `(alpha >> 3) + ((alpha & 7) > threshold)`, so for any two
        // thresholds t_ours != t_ref there is an alpha whose low three bits
        // fall strictly between them and which therefore rounds differently
        // under the two tables. Pick it, do not search for it: with
        // low = min(t_ours, t_ref), the alpha with low-three-bits
        // `low + 1` exceeds the smaller threshold and not the larger.
        let (x, y, ours, theirs) = disputed_cells[0];
        let low = ours.min(theirs);
        let alpha = (16u8 << 3) | (low + 1);
        assert!(
            (alpha & 7) > low && (alpha & 7) <= ours.max(theirs),
            "the probe alpha must separate the two thresholds"
        );
        let dithered_ours = crate::alpha_compare::apply_alpha_dither(
            alpha,
            AlphaDither::Pattern,
            RgbDither::Bayer,
            x,
            y,
            crate::alpha_compare::AlphaCompareNoise(0),
        );
        // Hand-derived expectation under each table, from the same rule.
        let expand = |five: u8| (five << 3) | (five >> 2);
        let round = |threshold: u8| expand(16 + u8::from((alpha & 7) > threshold));
        assert_eq!(
            dithered_ours,
            round(ours),
            "alpha dither follows this crate's tile"
        );
        assert_ne!(
            round(ours),
            round(theirs),
            "the two tables give different alpha at ({x}, {y}); refusing \
             Bayer is therefore a real choice, not a formality"
        );
    }

    /// The alpha-dither stage really runs, and its ordered arm really
    /// perturbs -- so a mutation that drops the call is observable.
    ///
    /// Uses the admitted `MagicSquare` tile at a cell whose threshold is
    /// low enough to bump the chosen alpha, hand-picked from the table
    /// rather than searched: `MAGIC_SQUARE[0][0] == 0`, so any alpha whose
    /// low three bits exceed 0 rounds up.
    #[test]
    fn the_alpha_dither_stage_perturbs_where_the_mode_says_it_should() {
        let magic = mode(
            (WM2000_HIGH & !(0x3 << 4) & !(0x3 << 6)) | (0x0 << 4) | (0x0 << 6),
            WM2000_LOW,
        );
        let stages = TexrectFragmentStages::try_new(magic, Color4::from_wire(0)).unwrap();
        assert_eq!(stages.alpha_dither, AlphaDither::Pattern);

        // alpha 223: floor 27, low bits 7 > threshold 0 -> rounds to 28.
        let dithered = apply_alpha_dither(
            223,
            stages.alpha_dither,
            stages.rgb_dither,
            0,
            0,
            NOISE_DITHER_THRESHOLD,
        );
        assert_eq!(dithered, (28u8 << 3) | (28u8 >> 2), "231");
        assert_ne!(dithered, 223, "the ordered tile must actually perturb here");

        // **And it reaches the pixel loop**, not just the helper.
        // Measured, not assumed: while this test went only through
        // `apply_alpha_dither`, replacing the executor's call with an
        // identity function left the whole suite green. The stage is
        // observable here because `MagicSquare` at cell (0,0) has
        // threshold 0, which bumps alpha 223 to 231 and moves the blended
        // channel by a whole five-bit step.
        let blend_state = TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0))
            .mode_state(magic);
        let mut stored = 0x0001u16.to_be_bytes();
        blend_and_write_pixel(
            ColorTargetFormat::Rgba16,
            &mut stored,
            [255, 255, 255, 223],
            blend_state,
            stages,
            0,
            0,
        )
        .unwrap();
        // Hand-derived: dithered alpha 231 over a zero destination gives
        // 255 * 231/255 = 231 -> 5-bit 28. Undithered would be 27.
        let dithered_five = (255.0f32 * (231.0 / 255.0)).round() as u16 >> 3;
        let undithered_five = (255.0f32 * (223.0 / 255.0)).round() as u16 >> 3;
        assert_eq!(
            (dithered_five, undithered_five),
            (28, 27),
            "the two must differ"
        );
        assert_eq!(
            u16::from_be_bytes(stored) >> 11,
            dithered_five,
            "the executor must have applied the ordered dither before blending"
        );

        // And WM2000's own Noise mode at the endpoint does not.
        let wm =
            TexrectFragmentStages::try_new(mode(WM2000_HIGH, WM2000_LOW), Color4::from_wire(0))
                .unwrap();
        assert_eq!(
            apply_alpha_dither(
                223,
                wm.alpha_dither,
                wm.rgb_dither,
                0,
                0,
                NOISE_DITHER_THRESHOLD
            ),
            (27u8 << 3) | (27u8 >> 2),
            "222 -- the endpoint, which is the undithered floor"
        );
    }
}

/// Coverage for [`TexrectDraw::try_from_viewport_and_texcoords`]'s four
/// construction refusals.
///
/// Why this module exists as its own block rather than as four cases inside
/// `one_cycle_tests`: `docs/RT64-COVERAGE-AUDIT.md` found all four guards
/// untested by mutation -- deleting each pair left the entire workspace
/// green, and the `NonIntegralTexcoord`/`TexcoordOutOfRange` pair's deletion
/// additionally left a silent `as i16` truncation, which is the "no silent
/// shrugs" ban in `AGENTS.md`'s behavior rules. Every test below is written
/// against the *named* error variant, not merely against `is_err()`, so a
/// guard that is deleted and replaced by a different refusal still fails.
#[cfg(test)]
mod construction_guard_tests {
    use super::*;

    /// The shape every case below perturbs by exactly one field: a
    /// non-degenerate viewport whose texcoords recover integer S10.5
    /// endpoints, so a failure names the perturbation and nothing else.
    fn admitted_viewport() -> RectViewportPixels {
        RectViewportPixels {
            left: 4,
            top: 8,
            right: 20,
            bottom: 24,
        }
    }

    /// Positive control: the unperturbed shape is admitted, and the
    /// recovered endpoints are the exact `value * 32.0` inverses. Without
    /// this, a case below could pass because the fixture is broken rather
    /// than because the guard fired.
    #[test]
    fn the_unperturbed_shape_is_admitted_and_recovers_its_endpoints() {
        let draw = TexrectDraw::try_from_viewport_and_texcoords(
            admitted_viewport(),
            [1.0, 2.0],
            [3.5, 4.25],
        )
        .expect("the unperturbed fixture is admitted");
        assert_eq!(
            (draw.s_start, draw.t_start, draw.s_end, draw.t_end),
            (32, 64, 112, 136),
            "endpoints are the integer inverses of the /32.0 RT64 emitted"
        );
        // Reconciled against an independent derivation from the same
        // literals, per the port card's two-independent-ways rule.
        assert_eq!(
            (
                (1.0f32 * 32.0) as i16,
                (2.0f32 * 32.0) as i16,
                (3.5f32 * 32.0) as i16,
                (4.25f32 * 32.0) as i16
            ),
            (32, 64, 112, 136)
        );
        assert_eq!(
            (draw.left, draw.top, draw.right, draw.bottom),
            (4, 8, 20, 24)
        );
    }

    #[test]
    fn flipped_axes_advance_s_by_row_and_t_by_column() {
        let viewport = RectViewportPixels {
            left: 0,
            top: 0,
            right: 4,
            bottom: 2,
        };
        let ordinary = TexrectDraw::try_from_viewport_and_texcoords(
            viewport,
            [0.0, 0.0],
            [2.0, 4.0],
        )
        .unwrap();
        let flipped = ordinary.with_flipped_axes();

        assert_eq!(ordinary.coordinates_at(1, 0), (16, 0));
        assert_eq!(ordinary.coordinates_at(0, 1), (0, 64));
        assert_eq!(flipped.coordinates_at(1, 0), (0, 32));
        assert_eq!(flipped.coordinates_at(0, 1), (32, 0));
    }

    /// Kills the `viewport.left < 0` half of the `NegativeViewportOrigin`
    /// guard. `left` alone is negative; every other field stays admitted,
    /// so the `EmptyViewport` guard below it does **not** also fire and
    /// this test cannot pass by way of the wrong refusal.
    #[test]
    fn a_negative_viewport_left_is_refused_by_name() {
        let viewport = RectViewportPixels {
            left: -1,
            ..admitted_viewport()
        };
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
            Err(TexrectExecutionError::NegativeViewportOrigin { viewport })
        );
        // The successor guard would not have caught this one: the extent
        // stays strictly positive, so deleting `NegativeViewportOrigin`
        // admits the rectangle rather than rejecting it differently.
        assert!(viewport.right > viewport.left && viewport.bottom > viewport.top);
    }

    /// Kills the `viewport.top < 0` half. Held separate from `left` because
    /// a mutant deleting only one disjunct survives a single-axis test.
    #[test]
    fn a_negative_viewport_top_is_refused_by_name() {
        let viewport = RectViewportPixels {
            top: -1,
            ..admitted_viewport()
        };
        assert_eq!(
            TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
            Err(TexrectExecutionError::NegativeViewportOrigin { viewport })
        );
        assert!(viewport.right > viewport.left && viewport.bottom > viewport.top);
    }

    /// Kills the `right <= left` half of `EmptyViewport`, at both the
    /// zero-width (`==`) and reversed (`<`) boundaries -- `<=` mutated to
    /// `<` survives a reversed-only test.
    #[test]
    fn a_zero_width_and_a_reversed_viewport_are_both_refused_by_name() {
        for right in [4, 3] {
            let viewport = RectViewportPixels {
                right,
                ..admitted_viewport()
            };
            assert_eq!(
                TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
                Err(TexrectExecutionError::EmptyViewport { viewport }),
                "right={right} against left={}",
                viewport.left
            );
        }
    }

    /// Kills the `bottom <= top` half, same two boundaries.
    #[test]
    fn a_zero_height_and_a_reversed_viewport_are_both_refused_by_name() {
        for bottom in [8, 7] {
            let viewport = RectViewportPixels {
                bottom,
                ..admitted_viewport()
            };
            assert_eq!(
                TexrectDraw::try_from_viewport_and_texcoords(viewport, [1.0, 2.0], [3.5, 4.25]),
                Err(TexrectExecutionError::EmptyViewport { viewport }),
                "bottom={bottom} against top={}",
                viewport.top
            );
        }
    }

    /// Kills `NonIntegralTexcoord` on all four texcoord slots.
    ///
    /// `1.0 / 64.0` is exactly representable, so `value * 32.0` is exactly
    /// `0.5` -- the refusal is on a genuinely non-integral product, not on
    /// a float-rounding artifact this test invented. Deleting the guard
    /// silently truncates `0.5 as i16` to `0`, which is the truncation
    /// `AGENTS.md` bans; each slot is exercised separately because the
    /// closure is invoked four times and a per-slot mutation survives a
    /// single-slot test.
    #[test]
    fn a_non_integral_texcoord_is_refused_by_name_on_every_slot() {
        let fractional = 1.0f32 / 64.0;
        assert_eq!(
            fractional * 32.0,
            0.5,
            "the product is exactly non-integral"
        );
        assert_eq!(
            (fractional * 32.0) as i16,
            0,
            "and a deleted guard would silently truncate it to zero"
        );
        for (slot, axis) in [
            (0usize, TexrectAxis::S),
            (1, TexrectAxis::T),
            (2, TexrectAxis::S),
            (3, TexrectAxis::T),
        ] {
            let mut coords = [1.0f32, 2.0, 3.5, 4.25];
            coords[slot] = fractional;
            assert_eq!(
                TexrectDraw::try_from_viewport_and_texcoords(
                    admitted_viewport(),
                    [coords[0], coords[1]],
                    [coords[2], coords[3]],
                ),
                Err(TexrectExecutionError::NonIntegralTexcoord {
                    axis,
                    value: fractional
                }),
                "slot {slot}"
            );
        }
    }

    /// Pins the non-finite refusals, which the fractional case above does
    /// not reach.
    ///
    /// **`!scaled.is_finite()` is a proven-equivalent disjunct, not an
    /// untested one, and this test does not claim to kill it.** Deleting it
    /// leaves all nine cases here green, and that survivor is equivalent
    /// rather than a reach failure: an exhaustive sweep of all 2^32 `f32`
    /// bit patterns found **zero** values for which `!is_finite()` holds
    /// and `fract() == 0.0`, because every non-finite `fract()` is NaN and
    /// `NaN != 0.0`. So `fract() != 0.0` alone already refuses every
    /// infinity and every NaN, and the `is_finite` conjunct is dead on
    /// every reachable input -- kept as documentation of intent, and it is
    /// what stops `f32::INFINITY as i16` from silently saturating to
    /// `i16::MAX` if the `fract` term were ever changed.
    #[test]
    fn non_finite_texcoords_are_refused_by_name() {
        for value in [f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                TexrectDraw::try_from_viewport_and_texcoords(
                    admitted_viewport(),
                    [value, 2.0],
                    [3.5, 4.25],
                ),
                Err(TexrectExecutionError::NonIntegralTexcoord {
                    axis: TexrectAxis::S,
                    value
                }),
                "{value}"
            );
        }
        // NaN is refused too, but `PartialEq` over the payload cannot
        // assert it by equality -- match on the variant instead.
        assert!(matches!(
            TexrectDraw::try_from_viewport_and_texcoords(
                admitted_viewport(),
                [f32::NAN, 2.0],
                [3.5, 4.25],
            ),
            Err(TexrectExecutionError::NonIntegralTexcoord {
                axis: TexrectAxis::S,
                ..
            })
        ));
    }

    /// Kills `TexcoordOutOfRange` at both ends, one step outside the S10.5
    /// range and integral at the scale -- so it passes the
    /// `NonIntegralTexcoord` guard above it and can only be caught here.
    ///
    /// The witnesses are derived, not guessed: `i16::MAX + 1 = 32768` and
    /// `i16::MIN - 1 = -32769` scaled back down by 32. Both are exactly
    /// representable in f32 (integers well inside the 24-bit range), so
    /// the product is exactly the out-of-range integer.
    #[test]
    fn an_out_of_range_texcoord_is_refused_by_name_at_both_ends() {
        let above = (f32::from(i16::MAX) + 1.0) / 32.0;
        let below = (f32::from(i16::MIN) - 1.0) / 32.0;
        assert_eq!((above * 32.0, below * 32.0), (32768.0, -32769.0));
        assert_eq!(
            (above * 32.0).fract(),
            0.0,
            "integral, so the guard above does not fire first"
        );
        assert_eq!((below * 32.0).fract(), 0.0);
        for (value, axis, upper_left, lower_right) in [
            (above, TexrectAxis::S, [above, 2.0], [3.5, 4.25]),
            (below, TexrectAxis::T, [1.0, below], [3.5, 4.25]),
        ] {
            assert_eq!(
                TexrectDraw::try_from_viewport_and_texcoords(
                    admitted_viewport(),
                    upper_left,
                    lower_right,
                ),
                Err(TexrectExecutionError::TexcoordOutOfRange { axis, value }),
                "{value}"
            );
        }
        // The endpoints themselves stay admitted -- a mutant tightening
        // `<`/`>` into `<=`/`>=` is killed here, not by the cases above.
        let draw = TexrectDraw::try_from_viewport_and_texcoords(
            admitted_viewport(),
            [f32::from(i16::MIN) / 32.0, f32::from(i16::MAX) / 32.0],
            [3.5, 4.25],
        )
        .expect("the inclusive S10.5 endpoints are admitted");
        assert_eq!((draw.s_start, draw.t_start), (i16::MIN, i16::MAX));
    }

    /// Every refusal renders a non-empty message naming its own axis or
    /// viewport, so a deleted guard cannot be replaced by a silent one.
    #[test]
    fn each_construction_refusal_renders_an_actionable_message() {
        let viewport = admitted_viewport();
        for (error, needle) in [
            (
                TexrectExecutionError::NegativeViewportOrigin { viewport },
                "negative",
            ),
            (TexrectExecutionError::EmptyViewport { viewport }, "empty"),
            (
                TexrectExecutionError::NonIntegralTexcoord {
                    axis: TexrectAxis::S,
                    value: 0.5,
                },
                "S",
            ),
            (
                TexrectExecutionError::TexcoordOutOfRange {
                    axis: TexrectAxis::T,
                    value: 4096.0,
                },
                "T",
            ),
        ] {
            let rendered = error.to_string();
            assert!(
                rendered.contains(needle),
                "{error:?} rendered {rendered:?} without {needle:?}"
            );
        }
    }
}

#[cfg(test)]
mod scissor_clip_tests {
    use fn64_render_ir::PhysicalMemoryLayout;

    use crate::targets::ColorTargetExtent;

    use super::*;

    /// A 64x64 RGBA16 colour target, the extent every case below clips
    /// against as its *second* bound.
    const TARGET_WIDTH: u32 = 64;
    const TARGET_HEIGHT: u32 = 64;

    fn key() -> ColorTargetKey {
        let layout = PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap();
        ColorTargetKey::try_new(
            layout.address(0x400).unwrap(),
            ColorTargetExtent::try_new(TARGET_WIDTH, TARGET_HEIGHT).unwrap(),
            ColorTargetFormat::Rgba16,
        )
        .unwrap()
    }

    /// A texrect covering `[left, right) x [top, bottom)` with a texcoord
    /// ramp whose endpoints are distinct, so a test can tell whether the
    /// ramp moved when the rectangle was clipped.
    ///
    /// The S10.5 endpoints are passed as `f32` pixels because that is the
    /// domain `try_from_viewport_and_texcoords` recovers from; `s / 32.0`
    /// inverts its own `* 32.0`.
    fn draw(left: i32, top: i32, right: i32, bottom: i32, s_end: i16, t_end: i16) -> TexrectDraw {
        TexrectDraw::try_from_viewport_and_texcoords(
            RectViewportPixels {
                left,
                top,
                right,
                bottom,
            },
            [0.0, 0.0],
            [f32::from(s_end) / 32.0, f32::from(t_end) / 32.0],
        )
        .unwrap()
    }

    fn rectangle(draw: TexrectDraw) -> TargetRectangle {
        TargetRectangle::try_new(draw.left(), draw.top(), draw.width(), draw.height()).unwrap()
    }

    fn clip(
        draw: TexrectDraw,
        scissor: RdpScissorRect,
    ) -> Result<ClippedTexrectExtent, TexrectExecutionError> {
        clip_texrect_extent(
            draw,
            scissor,
            TARGET_WIDTH,
            TARGET_HEIGHT,
            key(),
            rectangle(draw),
        )
    }

    /// A scissor genuinely TIGHTER than the colour target on every edge.
    ///
    /// Hand-derived from the wire layout, not from the code under test.
    /// Public libultra's `gDPSetScissor` encodes each coordinate multiplied
    /// by four into one of four twelve-bit fields
    /// (`/Users/jer/Code/aki-recomp/refs/oot-decomp/include/ultra64/gbi.h:4794-4804`),
    /// so a pixel bound is the quarter value divided by four:
    ///
    /// - `ulx = 40` quarter-pixels -> first column `40 / 4 = 10`
    /// - `lrx = 240` quarter-pixels -> column limit `240 / 4 = 60`
    /// - `uly = 20` quarter-pixels -> first row `20 / 4 = 5`
    /// - `lry = 200` quarter-pixels -> row limit `200 / 4 = 50`
    ///
    /// Every bound is strictly inside `0..64`, so a result that matched the
    /// target extent instead of this rect would be visibly wrong -- which is
    /// the whole point of choosing it. A scissor equal to the target would
    /// give the same answer under either precedence and prove nothing.
    fn tight_scissor() -> RdpScissorRect {
        RdpScissorRect::from_wire_quarter_pixels(0, 40, 20, 240, 200)
    }

    /// A scissor genuinely LOOSER than the colour target: 0..512
    /// quarter-pixels is 0..128 pixels, twice the target's 64. The target
    /// extent must win here, and the tight case above must NOT win, so the
    /// pair together pins the precedence rather than one bound happening to
    /// be right.
    fn loose_scissor() -> RdpScissorRect {
        RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 512, 512)
    }

    /// Wide-open, exactly the target: 64 pixels = 256 quarter-pixels.
    fn exact_scissor() -> RdpScissorRect {
        RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 256, 256)
    }

    /// **The replacement for the old "outside the target is refused"
    /// assertion.** A full-target rectangle under a tighter scissor is
    /// CLIPPED to the scissor, not refused and not clipped to the target.
    ///
    /// Pinned RT64 intersects the scissor and draw rectangles and retains a
    /// non-empty intersection rather than rejecting the primitive
    /// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`).
    /// fn64's own reference renderer clamps identically at
    /// `fn64-render-reference/src/raster/draw.rs:197-203`.
    #[test]
    fn a_rectangle_under_a_tighter_scissor_is_clipped_to_the_scissor_not_the_target() {
        let rect = draw(0, 0, 64, 64, 64, 64);
        let clipped = clip(rect, tight_scissor()).expect("a clipped rectangle still draws");
        assert_eq!(
            clipped.columns(),
            10..60,
            "hand-derived from ulx=40, lrx=240"
        );
        assert_eq!(clipped.rows(), 5..50, "hand-derived from uly=20, lry=200");
        // Distinguishes the scissor from the target: had the clip used the
        // 64x64 extent, the answer would have been the full 0..64 span.
        assert_ne!(clipped.columns(), 0..TARGET_WIDTH);
        assert_ne!(clipped.rows(), 0..TARGET_HEIGHT);
    }

    /// The precedence's other half: when the scissor is LOOSER than the
    /// target, the target's extent is what bounds the write. Neither bound
    /// substitutes for the other -- this case and the tight case above
    /// disagree about the answer, so a clip that consulted only one of them
    /// fails one of the two.
    #[test]
    fn a_rectangle_under_a_looser_scissor_is_clipped_to_the_target_extent() {
        // A rectangle overhanging the target on both axes -- exactly the
        // shape the old `OutsideTarget` refusal rejected outright.
        let rect = draw(32, 32, 96, 96, 64, 64);
        let clipped = clip(rect, loose_scissor()).expect("an overhanging rectangle still draws");
        // Offsets are relative to the rectangle's own origin at (32, 32):
        // screen span [32, min(96, 128, 64)) = [32, 64), so offsets 0..32.
        assert_eq!(clipped.columns(), 0..32);
        assert_eq!(clipped.rows(), 0..32);
        // Had the loose scissor won, the span would have run to offset 64.
        assert_ne!(clipped.columns(), 0..64);
    }

    /// A rectangle fully inside both bounds is untouched -- the clip must
    /// not narrow content that nothing asked it to narrow.
    #[test]
    fn a_rectangle_inside_both_bounds_keeps_its_whole_span() {
        let rect = draw(16, 16, 48, 48, 32, 32);
        let clipped = clip(rect, exact_scissor()).expect("an interior rectangle draws whole");
        assert_eq!(clipped.columns(), 0..32);
        assert_eq!(clipped.rows(), 0..32);
    }

    /// The quarter-pixel bounds round UP, not down or to nearest.
    ///
    /// `curover` fires on `>= clipxlshift` (angrylion `:2352`), making the
    /// high edge exclusive, and the low edge is driven out to `clipxhshift`
    /// (`:2351`), so both ends take the ceiling of `quarter / 4`. A scissor
    /// at quarter-pixel 41 therefore first admits pixel 11, not 10: pixel 10
    /// is only three-quarters covered on its right, and the RDP's clamp
    /// pushes the span past it.
    ///
    /// Truncation would give 10 and 60 here, so this case genuinely
    /// distinguishes the two roundings; the exact multiples used elsewhere
    /// would not.
    #[test]
    fn a_fractional_scissor_edge_rounds_up_on_both_ends() {
        let rect = draw(0, 0, 64, 64, 64, 64);
        // ulx = 41q -> ceil(41/4) = 11; lrx = 241q -> ceil(241/4) = 61.
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 41, 41, 241, 241);
        let clipped = clip(rect, scissor).expect("a fractionally-scissored rectangle draws");
        assert_eq!(clipped.columns(), 11..61);
        assert_eq!(clipped.rows(), 11..61);
        // Truncating division would have produced these instead.
        assert_ne!(clipped.columns(), 10..60);
    }

    /// **Still refused, and this is the case kept.** A rectangle with no
    /// pixel surviving the intersection is named rather than silently
    /// written as zero pixels: it is either genuinely off-screen or the
    /// scissor is degenerate, and both are worth surfacing.
    #[test]
    fn a_rectangle_entirely_outside_the_scissor_is_refused_by_name() {
        let rect = draw(0, 0, 8, 8, 8, 8);
        // Scissor starts at pixel 10; the rectangle ends at pixel 8.
        let error = clip(rect, tight_scissor()).expect_err("nothing survives the intersection");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }

    /// **The refusal fires on EITHER axis alone, not only on both.**
    ///
    /// The two degenerate cases above are empty on X *and* Y, so a check
    /// that consulted only one axis would pass them both -- exactly the
    /// coincident-fixture trap. These two cases are empty on one axis while
    /// the other still has a healthy span, so each one fails if its axis is
    /// dropped from the emptiness test.
    #[test]
    fn an_extent_empty_on_only_the_x_axis_is_still_refused() {
        // X: rect 0..8 vs scissor first column 10 -> empty.
        // Y: rect 0..64 vs scissor rows 5..50 -> 45 rows survive.
        let rect = draw(0, 0, 8, 64, 8, 64);
        let error = clip(rect, tight_scissor()).expect_err("an empty X span admits nothing");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }

    #[test]
    fn an_extent_empty_on_only_the_y_axis_is_still_refused() {
        // Y: rect 0..4 vs scissor first row 5 -> empty.
        // X: rect 0..64 vs scissor columns 10..60 -> 50 columns survive.
        let rect = draw(0, 0, 64, 4, 64, 4);
        let error = clip(rect, tight_scissor()).expect_err("an empty Y span admits nothing");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }

    /// A reversed scissor -- `lrx < ulx` -- is likewise refused rather than
    /// producing a backwards span. The RDP latches whatever four values
    /// arrive (`rdp_set_scissor` performs no ordering check at
    /// `rasterizer.c:2779-2784`), so the degeneracy has to be caught at
    /// clip time, which is where this catches it.
    #[test]
    fn a_reversed_scissor_is_refused_rather_than_producing_a_backwards_span() {
        let rect = draw(0, 0, 64, 64, 64, 64);
        let reversed = RdpScissorRect::from_wire_quarter_pixels(0, 200, 200, 40, 40);
        let error = clip(rect, reversed).expect_err("a reversed rect admits nothing");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }

    /// A rectangle entirely past the colour target's end is refused too --
    /// the target bound is a real invariant even though the scissor is not
    /// the thing rejecting it. fn64's target is a sized buffer, and a write
    /// past it is a defect rather than content.
    #[test]
    fn a_rectangle_entirely_past_the_target_extent_is_refused_by_name() {
        let rect = draw(80, 80, 96, 96, 16, 16);
        let error = clip(rect, loose_scissor()).expect_err("nothing survives the target bound");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }

    /// **The texture ramp must NOT slide when the rectangle is clipped.**
    ///
    /// fn64 evaluates S/T from offsets relative to the unclipped rectangle,
    /// so clipping changes only the surviving screen extent and does not
    /// rebase the texture ramp. This is fn64's own reading of the rule and
    /// is not independently confirmed against an allowed hardware reference.
    ///
    /// This is why the clip returns OFFSETS into the rectangle rather than
    /// a narrowed `TexrectDraw`: rebasing the ramp onto the clipped left
    /// edge would slide the texture sideways by the clipped amount. The
    /// case is chosen so the two answers differ -- at clipped offset 10 the
    /// correct S is 10, while a rebased ramp would sample 0 there.
    #[test]
    fn clipping_does_not_move_the_texture_coordinate_ramp() {
        // 64 pixels wide, S running 0..64 in S10.5 raw units: one raw unit
        // per pixel, so `s_at(n) == n` exactly.
        let rect = draw(0, 0, 64, 64, 64, 64);
        let clipped = clip(rect, tight_scissor()).unwrap();
        let first = clipped.columns().start;
        assert_eq!(first, 10, "the tight scissor's own first column");
        // Sampled at the clipped offset, which is the offset from the
        // UNCLIPPED origin -- so the first drawn pixel reads texel 10.
        assert_eq!(rect.s_at(first), 10);
        assert_eq!(rect.t_at(clipped.rows().start), 5);
        // A rebased ramp would have read texel 0 at the first drawn pixel.
        assert_ne!(rect.s_at(first), 0);
    }

    /// The mode field survives the latch and is not consulted by the clip.
    /// Carried so a reader can see it was decoded rather than dropped; the
    /// progressive full-frame path this executor serves draws every
    /// scanline. Ignoring the mode during clipping is fn64's own policy and
    /// is not independently confirmed against an allowed hardware reference.
    #[test]
    fn the_scissor_mode_field_round_trips_and_does_not_change_the_clip() {
        let rect = draw(0, 0, 64, 64, 64, 64);
        let plain = RdpScissorRect::from_wire_quarter_pixels(0, 40, 20, 240, 200);
        let interlaced = RdpScissorRect::from_wire_quarter_pixels(3, 40, 20, 240, 200);
        assert_eq!(plain.mode(), 0);
        assert_eq!(interlaced.mode(), 3);
        assert_eq!(clip(rect, plain).unwrap(), clip(rect, interlaced).unwrap());
    }

    /// Each of the four coordinates reaches its own axis and end. A clip
    /// that transposed X and Y, or swapped the two ends of one axis, would
    /// pass every symmetric fixture above; this one is deliberately
    /// asymmetric in all four values so no pair coincides.
    #[test]
    fn each_scissor_coordinate_drives_its_own_axis_and_end() {
        let rect = draw(0, 0, 64, 64, 64, 64);
        // ulx=4q->1, uly=12q->3, lrx=180q->45, lry=100q->25. All distinct.
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 4, 12, 180, 100);
        let clipped = clip(rect, scissor).expect("an asymmetric scissor still draws");
        assert_eq!(clipped.columns(), 1..45);
        assert_eq!(clipped.rows(), 3..25);
    }
}

/// End-to-end cases driving [`execute_texture_rectangle`] itself, not just
/// [`clip_texrect_extent`].
///
/// The clip unit tests above pin the arithmetic; these pin that the
/// EXECUTOR uses it -- that the pixel loop walks the clipped span and that
/// the claimed rectangle is the clipped one. Both were live mutation
/// survivors while only the unit tests existed: replacing
/// `for row in clipped.rows()` with `for row in 0..draw.height()`, and
/// claiming the unclipped `rectangle`, each left the whole suite green.
#[cfg(test)]
mod scissor_execution_tests {
    use std::collections::BTreeMap;

    use fn64_render::{NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode};
    use fn64_render_ir::PhysicalMemoryLayout;

    use super::*;
    use crate::targets::{
        ColorTargetExtent, ColorTargetRegistry, CompletedColorTargetWrite, Rgba8,
    };

    const TARGET_WIDTH: u32 = 16;
    const TARGET_HEIGHT: u32 = 16;
    const FIXTURE_START: u32 = 0x400;
    /// The RGBA16 halfword every fixture texel decodes to: red, opaque.
    /// Distinct from the target's initialized blue so a written pixel is
    /// unambiguous.
    const TEXEL: u16 = 0xF801;
    /// The colour the target is initialized to before the texrect runs.
    /// Any pixel still holding this afterwards was NOT written.
    const BACKGROUND: Rgba8 = Rgba8::new(0, 0, 255, 255);

    fn layout() -> PhysicalMemoryLayout {
        PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap()
    }

    fn key() -> ColorTargetKey {
        ColorTargetKey::try_new(
            layout().address(FIXTURE_START).unwrap(),
            ColorTargetExtent::try_new(TARGET_WIDTH, TARGET_HEIGHT).unwrap(),
            ColorTargetFormat::Rgba16,
        )
        .unwrap()
    }

    /// A TMEM source holding one 16x16 RGBA16 tile of the single colour
    /// [`TEXEL`], so every sampled texel is the same and a written pixel is
    /// identified by its colour alone rather than by which texel it read.
    struct FlatTmem {
        bytes: BTreeMap<u16, u8>,
    }

    impl FlatTmem {
        fn new() -> Self {
            let mut bytes = BTreeMap::new();
            // 16 rows of 16 RGBA16 texels: 32 bytes per row, 512 total.
            for address in 0..512u16 {
                bytes.insert(
                    address,
                    if address % 2 == 0 {
                        (TEXEL >> 8) as u8
                    } else {
                        (TEXEL & 0xff) as u8
                    },
                );
            }
            Self { bytes }
        }
    }

    impl crate::TmemByteSource for FlatTmem {
        fn snapshot(&self) -> crate::TmemSnapshotIdentity {
            crate::TmemByteSource::snapshot(&crate::PhysicalTmemState::try_new().unwrap())
        }

        fn valid_byte(&self, address: u16) -> Option<u8> {
            self.bytes.get(&address).copied()
        }
    }

    fn tile() -> TexrectTileBinding {
        TexrectTileBinding::try_from_neutral(
            fn64_render::NeutralTileDescriptor {
                format: NeutralImageFormat::Rgba,
                size: NeutralPixelSize::Bits16,
                // 16 texels * 2 bytes = 32 bytes = 4 TMEM words per row.
                line_words: 4,
                tmem_word_address: 0,
                palette: 0,
                s_mode: NeutralTileAddressMode::default(),
                mask_s: 0,
                shift_s: 0,
                t_mode: NeutralTileAddressMode::default(),
                mask_t: 0,
                shift_t: 0,
            },
            fn64_render::NeutralTileSize {
                low_s: 0,
                low_t: 0,
                // 10.2 fixed point: 15 pixels = 60 quarter-pixels.
                high_s: 60,
                high_t: 60,
            },
        )
        .unwrap()
    }

    /// Copy cycle: the sampled texel is blitted with no combiner and no
    /// blender consulted, which is what the RDP does in that mode. Chosen
    /// so this fixture needs no `SetCombine`, keeping the case about the
    /// clip rather than about combiner setup.
    fn copy_cycle_other_mode() -> OtherMode {
        // `cycle_type` is wire bits 20:21 of the high word; `2` is Copy
        // (`state.rs`'s `cycle_type()` decode).
        OtherMode::from_wire(2 << 20, 0)
    }

    /// Runs one texrect over a target pre-filled with [`BACKGROUND`], and
    /// returns the resulting pixels plus the rectangle the write claimed.
    fn run(
        draw: TexrectDraw,
        scissor: RdpScissorRect,
    ) -> Result<(Vec<Rgba8>, TargetRectangle), TexrectExecutionError> {
        run_with_input_ownership(draw, scissor, false)
    }

    fn run_with_input_ownership(
        draw: TexrectDraw,
        scissor: RdpScissorRect,
        owned: bool,
    ) -> Result<(Vec<Rgba8>, TargetRectangle), TexrectExecutionError> {
        let key = key();
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let full = TargetRectangle::try_new(0, 0, TARGET_WIDTH, TARGET_HEIGHT).unwrap();
        let plan = candidate.plan_rows(full).unwrap();
        let background = vec![BACKGROUND; key.extent().pixels() as usize];
        let device = crate::targets::pack_device_pixels(&candidate, &background).unwrap();
        let resident_bytes = device.device_bytes().to_vec();
        let _ = candidate
            .admit_completed_initialization(CompletedColorTargetWrite {
                key,
                generation: plan.generation,
                range: key.range(),
                rectangle: plan.rectangle,
                device_bytes: device,
            })
            .unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let input = if owned {
            Cow::Owned(resident_bytes)
        } else {
            Cow::Borrowed(resident_bytes.as_slice())
        };
        let completed = execute_texture_rectangle(
            &candidate,
            copy_cycle_other_mode(),
            draw,
            tile(),
            &FlatTmem::new(),
            TextureLutMode::Disabled,
            TexrectShading::new(
                CombineParams::from_wire(0, 0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
            ),
            TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)),
            scissor,
            input,
            None,
        )?;
        let rectangle = completed.rectangle;
        let pixels = crate::targets::unpack_device_pixels(
            ColorTargetFormat::Rgba16,
            completed.device_bytes.device_bytes(),
        )
        .expect("the write's own bytes unpack");
        Ok((pixels.into_vec(), rectangle))
    }

    fn full_rect_draw() -> TexrectDraw {
        TexrectDraw::try_from_viewport_and_texcoords(
            RectViewportPixels {
                left: 0,
                top: 0,
                right: TARGET_WIDTH as i32,
                bottom: TARGET_HEIGHT as i32,
            },
            [0.0, 0.0],
            // 16 pixels across a 16-pixel rect: one S10.5 raw unit per
            // pixel, well inside the tile's 0..60 quarter-pixel bounds.
            [16.0 / 32.0, 16.0 / 32.0],
        )
        .unwrap()
    }

    fn pixel(pixels: &[Rgba8], x: u32, y: u32) -> Rgba8 {
        pixels[(y * TARGET_WIDTH + x) as usize]
    }

    #[test]
    fn owned_and_borrowed_resident_inputs_produce_identical_texrect_bytes() {
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
        let borrowed = run_with_input_ownership(full_rect_draw(), scissor, false).unwrap();
        let owned = run_with_input_ownership(full_rect_draw(), scissor, true).unwrap();
        assert_eq!(owned, borrowed);
    }

    /// **The executor writes only the scissored span.**
    ///
    /// A full-target rectangle under a scissor covering pixels 4..12 on
    /// both axes leaves everything outside that box untouched. Derived by
    /// hand from the wire layout: `ulx = uly = 16` quarter-pixels is pixel
    /// 4, `lrx = lry = 48` quarter-pixels is pixel 12 exclusive. Public
    /// libultra's `gDPSetScissor` encodes all four coordinates multiplied by
    /// four into twelve-bit fields
    /// (`/Users/jer/Code/aki-recomp/refs/oot-decomp/include/ultra64/gbi.h:4794-4804`).
    ///
    /// The scissor is strictly inside the 16x16 target on all four edges,
    /// so a clip that consulted the target extent instead would write the
    /// whole surface and fail every corner assertion below.
    #[test]
    fn the_executor_writes_only_the_scissored_span() {
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
        let (pixels, _) = run(full_rect_draw(), scissor).expect("a clipped rectangle draws");
        // Inside the scissor: written.
        for (x, y) in [(4, 4), (11, 11), (8, 8), (4, 11), (11, 4)] {
            assert_ne!(
                pixel(&pixels, x, y),
                BACKGROUND,
                "({x}, {y}) is inside the scissor and must be written"
            );
        }
        // Outside it on each of the four sides, and at a corner: untouched.
        for (x, y) in [(3, 8), (12, 8), (8, 3), (8, 12), (0, 0), (15, 15)] {
            assert_eq!(
                pixel(&pixels, x, y),
                BACKGROUND,
                "({x}, {y}) is outside the scissor and must be untouched"
            );
        }
    }

    /// **The claimed rectangle is the clipped one, not the command's.**
    ///
    /// `admit_completed_initialization` reads this rectangle as proof of
    /// which pixels a write established. Claiming the unclipped rect would
    /// assert proof over the pixels the scissor kept the executor away
    /// from -- pixels that still hold whatever was there before.
    #[test]
    fn the_claimed_rectangle_is_the_clipped_rectangle() {
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
        let (_, rectangle) = run(full_rect_draw(), scissor).expect("a clipped rectangle draws");
        assert_eq!(rectangle.x(), 4);
        assert_eq!(rectangle.y(), 4);
        assert_eq!(rectangle.width(), 8);
        assert_eq!(rectangle.height(), 8);
        // The command's own rectangle was the full 16x16 surface.
        assert_ne!(rectangle.width(), TARGET_WIDTH);
    }

    /// **A rectangle overhanging the target is DRAWN, not refused.**
    ///
    /// This is the case the old `TexrectExecutionError::OutsideTarget`
    /// refusal rejected outright, on the reasoning that "a clamped
    /// rectangle would write pixels the RDP never covers." Pinned RT64
    /// intersects the scissor and draw rectangles and keeps a non-empty
    /// intersection instead of rejecting it
    /// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`),
    /// and fn64's own reference renderer does the
    /// same at `fn64-render-reference/src/raster/draw.rs:197-203`.
    ///
    /// FAILS BEFORE this change with `OutsideTarget`; passes after, with
    /// the surviving half of the rectangle drawn.
    #[test]
    fn a_rectangle_overhanging_the_target_is_drawn_rather_than_refused() {
        // Starts at pixel 8 and runs to 24 -- eight pixels past the
        // 16-pixel target on each axis.
        let draw = TexrectDraw::try_from_viewport_and_texcoords(
            RectViewportPixels {
                left: 8,
                top: 8,
                right: 24,
                bottom: 24,
            },
            [0.0, 0.0],
            [16.0 / 32.0, 16.0 / 32.0],
        )
        .unwrap();
        // Wide open: 16 pixels = 64 quarter-pixels, so the scissor bounds
        // nothing and the TARGET extent is what clips. That makes this case
        // about the overhang specifically.
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 64, 64);
        let (pixels, rectangle) = run(draw, scissor).expect("an overhanging rectangle draws");
        assert_eq!((rectangle.x(), rectangle.y()), (8, 8));
        assert_eq!((rectangle.width(), rectangle.height()), (8, 8));
        assert_ne!(pixel(&pixels, 8, 8), BACKGROUND, "the surviving quarter");
        assert_ne!(pixel(&pixels, 15, 15), BACKGROUND, "up to the last pixel");
        assert_eq!(pixel(&pixels, 7, 7), BACKGROUND, "outside the rectangle");
    }

    /// The kept refusal, reached through the executor rather than the clip
    /// helper: a rectangle with no surviving pixel is named, not silently
    /// reported as a successful zero-pixel write.
    #[test]
    fn a_fully_scissored_rectangle_is_refused_through_the_executor() {
        // Scissor admits pixels 0..2; the rectangle starts at 8.
        let draw = TexrectDraw::try_from_viewport_and_texcoords(
            RectViewportPixels {
                left: 8,
                top: 8,
                right: 16,
                bottom: 16,
            },
            [0.0, 0.0],
            [8.0 / 32.0, 8.0 / 32.0],
        )
        .unwrap();
        let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 8, 8);
        let error = run(draw, scissor).expect_err("nothing survives");
        assert!(
            matches!(error, TexrectExecutionError::ScissoredAway { .. }),
            "expected ScissoredAway, got {error:?}"
        );
    }
}
