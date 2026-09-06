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
//! (`docs/rt64/RT64-WM2000-CYCLE-MODES.md` §§1-2). Every other selector --
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
//! - **Filtering follows OtherMode.** Point, the RDP's three-nearest bilerp,
//!   and four-corner average all share [`crate::sample_texture`]. Reserved
//!   filter encoding refuses by name. Copy cycle retains its documented
//!   point/copy sampling behavior rather than consulting the filter bits.
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
//! non-empty, in-target pixel extent; point, three-nearest, or average
//! sampling (copy cycle is always point); texcoords that
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
    PhysicalTexelReadError, PointSampleCoordinates, PointSampleError, PointSampleRequest,
    PreparedTextureSampler, TextureCoordinateS10_5, TextureSampleError, TileAddressMode,
    TileCoordinate, TileDescriptor, TileSize, TmemFirstRowParity, TmemWordAddress,
};
use crate::{CycleType, ImageFormat, OtherMode, PixelSize, TextureFilter, TextureLutMode};

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
    /// (`docs/rt64/RT64-WM2000-CYCLE-MODES.md` §1). It says two-cycle was not
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
    /// `docs/rt64/RT64-LANE-DIVERGENCES.md` this module could not close in the
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
        source: TextureSampleError,
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
    /// `docs/rt64/RT64-LANE-DIVERGENCES.md` D7 scored this refusal a wgpu defect
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
/// boot-through-attract window (`docs/rt64/RT64-WM2000-CYCLE-MODES.md` §2): its
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
/// `docs/rt64/RT64-LANE-DIVERGENCES.md` D4 lists twelve selectors this executor
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
        crate::diag_env::diag_env("FN64_TEXRECT_RANK_ONE_SPECIALIZATION").is_none_or(|value| value != "0")
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
#[path = "texrect/tests/rank_one_ci4_rgba16.rs"]
mod rank_one_ci4_rgba16_tests;

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
    let texture_filter = match evaluation {
        TexrectCombinerEvaluation::BlitsTheTexel => TextureFilter::Point,
        TexrectCombinerEvaluation::OneCycle | TexrectCombinerEvaluation::TwoCycle => {
            other_mode.texture_filter()
        }
    };
    let rank_one = (texture_filter == TextureFilter::Point)
        .then(|| {
            RankOneCi4Rgba16::admit(shading.combine(), other_mode, format, lut_mode, tile, draw)
        })
        .flatten();
    let mut prepared_sampler = rank_one
        .is_none()
        .then(|| {
            PreparedTextureSampler::try_new(
                tile.descriptor(),
                tile.size(),
                lut_mode,
                texture_filter,
            )
        })
        .transpose()
        .map_err(|source| TexrectExecutionError::Sample {
            column: 0,
            row: 0,
            source,
        })?
        .map(|sampler| sampler.bind(tmem));
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
                Some(specialized) => specialized.sample(tmem, s, t).map_err(Into::into),
                None => prepared_sampler
                    .as_mut()
                    .expect("the generic texrect path prepared one sampler")
                    .sample(request),
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
                Some(base) => combine_one_texel(shading.combine(), base, sampled_rgba, evaluation),
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

    /// Takes `Option<&str>` since task 2.2b (was `Option<&OsStr>`): the
    /// crate's single permitted read site returns `Option<String>`. "Set to
    /// anything but `0`" is the same predicate either way.
    fn env_value_enables(value: Option<&str>) -> bool {
        value.is_some_and(|value| value != "0")
    }

    fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            env_value_enables(crate::diag_env::diag_env("FN64_TEXRECT_TIMING_CENSUS").as_deref())
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
            assert!(!env_value_enables(Some("0")));
            assert!(env_value_enables(Some("")));
            assert!(env_value_enables(Some("1")));
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
            // See `docs/rt64/RT64-GUARD-AUDIT.md` finding A3.
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
            // `docs/rt64/RT64-LANE-DIVERGENCES.md` D7's "the alpha stage already
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

/// Executes one fragment with exact primitive and destination coverage.
#[allow(clippy::too_many_arguments)]
pub(super) fn blend_and_write_pixel_with_coverage(
    format: ColorTargetFormat,
    dest: &mut [u8],
    combined: [u8; 4],
    state: BlendModeState,
    stages: TexrectFragmentStages,
    column: u32,
    row: u32,
    primitive_coverage: Coverage,
    memory_coverage: Coverage,
    exact_memory_coverage: bool,
) -> Result<Coverage, TexrectExecutionError> {
    let (combined, pixel_coverage) = apply_coverage_alpha(
        stages.coverage_times_alpha,
        stages.alpha_coverage_select,
        combined,
        primitive_coverage,
    );
    if pixel_coverage.count() == 0 {
        return Ok(memory_coverage);
    }
    let visible_memory_coverage = match format {
        ColorTargetFormat::Rgba16 => {
            if dest[1] & 1 == 0 {
                Coverage::new(1)
            } else {
                Coverage::FULL
            }
        }
        ColorTargetFormat::Rgba32 => Coverage::from_stored(dest[3] >> 5),
    };
    debug_assert_eq!(
        (visible_memory_coverage.stored() >> 2) & 1,
        (memory_coverage.stored() >> 2) & 1
    );
    let coverage = if exact_memory_coverage {
        coverage_result(pixel_coverage, memory_coverage, stages.coverage_mode)
    } else {
        stages.coverage_for(pixel_coverage, visible_memory_coverage)?
    };
    if !alpha_compare_texrect_fragment(stages, combined[3])? {
        return Ok(memory_coverage);
    }
    let coverage_carry = if state.other_mode.image_read_enabled() {
        coverage.wraps
    } else {
        pixel_coverage.count() & 8 != 0
    };
    if state.other_mode.clear_on_coverage() && !coverage_carry {
        write_coverage_only(format, dest, coverage.destination);
        return Ok(coverage.destination);
    }

    let mut combined = combined;
    combined[3] = apply_alpha_dither(
        combined[3],
        stages.alpha_dither,
        stages.rgb_dither,
        column as i32,
        row as i32,
        NOISE_DITHER_THRESHOLD,
    );
    let destination = read_pixel(format, dest);
    let blended = blend_texrect_fragment(combined, destination, state, column, row)?;
    let _ = stages.rgb_dither;
    write_pixel(format, dest, blended, coverage.destination);
    Ok(coverage.destination)
}

fn write_coverage_only(format: ColorTargetFormat, dest: &mut [u8], coverage: Coverage) {
    match format {
        ColorTargetFormat::Rgba16 => {
            dest[1] = (dest[1] & !1) | ((coverage.stored() >> 2) & 1);
        }
        ColorTargetFormat::Rgba32 => {
            dest[3] = (dest[3] & 0x1f) | (coverage.stored() << 5);
        }
    }
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
fn write_pixel(format: ColorTargetFormat, dest: &mut [u8], rgba: [u8; 4], coverage: Coverage) {
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
#[path = "texrect/tests/one_cycle.rs"]
mod one_cycle_tests;

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
#[path = "texrect/tests/blend_stage.rs"]
mod blend_stage_tests;

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
#[path = "texrect/tests/fragment_stage.rs"]
mod fragment_stage_tests;

/// Coverage for [`TexrectDraw::try_from_viewport_and_texcoords`]'s four
/// construction refusals.
///
/// Why this module exists as its own block rather than as four cases inside
/// `one_cycle_tests`: `docs/rt64/RT64-COVERAGE-AUDIT.md` found all four guards
/// untested by mutation -- deleting each pair left the entire workspace
/// green, and the `NonIntegralTexcoord`/`TexcoordOutOfRange` pair's deletion
/// additionally left a silent `as i16` truncation, which is the "no silent
/// shrugs" ban in `AGENTS.md`'s behavior rules. Every test below is written
/// against the *named* error variant, not merely against `is_err()`, so a
/// guard that is deleted and replaced by a different refusal still fails.
#[cfg(test)]
#[path = "texrect/tests/construction_guard.rs"]
mod construction_guard_tests;

#[cfg(test)]
#[path = "texrect/tests/scissor_clip.rs"]
mod scissor_clip_tests;

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
#[path = "texrect/tests/scissor_execution.rs"]
mod scissor_execution_tests;
