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
//! `Shade`, `Texel1`, `Combined`, the LOD fractions, noise, the chroma key
//! -- is refused **by name** at [`TexrectShading::try_new`], before any
//! pixel is produced, so a title that needs one gets a loud error rather
//! than pixels combined against a zero this executor invented.
//!
//! ## Nonclaims
//!
//! - **No two-cycle, no Fill cycle.** Both refused by name
//!   ([`TexrectExecutionError::UnsupportedCycleType`]); two-cycle needs the
//!   `Combined` carry and a second texel, neither of which this executor
//!   supplies. Measured absent from the window above.
//! - **No blending, alpha compare, dither, or coverage.** The combiner's
//!   output is written to the destination; the blender is a separate stage
//!   this executor does not run. `run_one_cycle`'s `alphaCompareValue`
//!   out-parameter is deliberately discarded for that reason.
//! - **No Shade.** This executor has no vertex-interpolated color to
//!   supply, so a program reading `Shade` is refused rather than combined
//!   against zero.
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
//! Copy cycle and one-cycle; one rectangle per packet; a non-negative,
//! non-empty, in-target pixel extent; point sampling; texcoords that
//! recover integer S10.5 endpoints; and, in one-cycle, a combiner program
//! reading only `Texel0`/`Primitive`/`Environment`/`One`/`Zero` with every
//! register it reads actually set. Everything outside that is a named
//! [`TexrectExecutionError`], never an approximation.
//!
//! ## Scope status
//!
//! DONE for the composed `fill + LoadBlock + texrect` shape in **both**
//! Copy and one-cycle, proven end to end into guest RDRAM for both measured
//! WM2000 programs. Two-cycle texrects are **deliberately not ported** (a
//! scope boundary this slice chose, not work this module waits on): zero
//! occurrences in the measured window, and they need the cross-cycle
//! `Combined` carry and a second texel.
//!
//! ## Open questions
//!
//! `step_axis`'s truncating division is a preserved convention, not a
//! verified silicon tie-break; public documentation does not establish the
//! RDP's rounding for interpolated texture coordinates. Likewise
//! `TmemFirstRowParity::Even` is passed unconditionally, which is correct
//! for every tile whose first row is even (all this crate's fixtures) and
//! is a frontier for a tile loaded at an odd row parity.

use crate::combiner::{
    combiner_inputs_from_fragment_registers, run_one_cycle, AlphaInput, AlphaInputSlot, ColorInput,
    ColorInputSlot, CombineParams, CombinerInputs,
};
use crate::state::{Color4, PrimColor};
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
    /// Copy cycle blits the texel; one-cycle runs it through the color
    /// combiner. Both are executed. Two-cycle needs the `Combined` carry
    /// and a second texel, and Fill cycle samples no texture at all --
    /// both refused by name rather than approximated. Measured: WM2000's
    /// boot-through-attract window issues 2,520 texrects, all one-cycle,
    /// none two-cycle or Fill (`docs/RT64-WM2000-CYCLE-MODES.md` §1).
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
    /// The program reads a constant color register whose wire command has
    /// not run at this texrect's own stream position. Never defaulted to
    /// black: an unset register is a stream this executor has not seen,
    /// not a register that happens to be zero.
    UnsetConstantRegister {
        register: TexrectConstantRegister,
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
                "execute_texture_rectangle admits Copy cycle (direct blit) and OneCycle \
                 (combiner-evaluated); got {cycle_type:?}"
            ),
            Self::UnsupportedColorInput { slot, input } => write!(
                formatter,
                "execute_texture_rectangle evaluates only TEXEL0/PRIMITIVE/ENVIRONMENT/ONE/ZERO \
                 color inputs; one-cycle slot {slot:?} selects {input:?}"
            ),
            Self::UnsupportedAlphaInput { slot, input } => write!(
                formatter,
                "execute_texture_rectangle evaluates only TEXEL0/PRIMITIVE/ENVIRONMENT/ONE/ZERO \
                 alpha inputs; one-cycle slot {slot:?} selects {input:?}"
            ),
            Self::UnsetConstantRegister { register } => write!(
                formatter,
                "the one-cycle combiner program reads the {register} register, but no {register} \
                 command has run at this texrect's own stream position"
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

/// The one-cycle shading state a texrect's fragments are combined with:
/// the `SetCombine` program current at the texrect's own stream position,
/// plus the two constant color registers the measured programs read.
///
/// Constructed by [`Self::try_new`], which refuses -- by name, before any
/// pixel is written -- every combiner selector this executor does not
/// evaluate. `Primitive` and `Environment` are `Option` because the
/// registers are genuinely unset until their own wire command runs, and a
/// program that reads an unset register is a named refusal rather than a
/// silently-black default.
///
/// Not a `CombinerInputs` itself: that struct is per-pixel (its `tex_val0`
/// changes on every texel), whereas this is the per-rectangle half. The
/// per-pixel half is assembled inside the sampling loop from this plus the
/// sampled texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexrectShading {
    combine: CombineParams,
    env_color: Option<Color4>,
    prim_color: Option<PrimColor>,
}

/// Every color selector this executor evaluates. Measured against WM2000's
/// entire boot-through-attract window (`docs/RT64-WM2000-CYCLE-MODES.md`
/// §2): its 2,520 texrects run exactly two programs, and between them they
/// read only these five. Everything else is refused by name so a future
/// title gets a loud error instead of wrong pixels -- `Shade` in
/// particular, which this executor has no vertex-interpolated color to
/// supply and would otherwise silently combine against zero.
const ADMITTED_COLOR_INPUTS: [ColorInput; 5] = [
    ColorInput::Texel0,
    ColorInput::Primitive,
    ColorInput::Environment,
    ColorInput::One,
    ColorInput::Zero,
];

/// [`ADMITTED_COLOR_INPUTS`]' alpha counterpart, same measurement and same
/// rationale.
const ADMITTED_ALPHA_INPUTS: [AlphaInput; 5] = [
    AlphaInput::Texel0,
    AlphaInput::Primitive,
    AlphaInput::Environment,
    AlphaInput::One,
    AlphaInput::Zero,
];

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
    pub fn new(
        combine: CombineParams,
        env_color: Option<Color4>,
        prim_color: Option<PrimColor>,
    ) -> Self {
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
    /// Called by the executor **only in one-cycle**. Copy cycle consults no
    /// combiner program on real hardware, so gating a Copy rectangle on the
    /// program that happens to be latched would refuse rectangles the RDP
    /// draws -- measured, not reasoned: the existing composed Copy fixture
    /// latches `SetCombine(0, 0)`, whose slot A decodes to `COMBINED`, and
    /// validating it unconditionally refused a packet that had executed
    /// correctly for the whole life of the Copy path.
    pub fn validate_one_cycle(self) -> Result<Self, TexrectExecutionError> {
        let Self {
            combine,
            env_color,
            prim_color,
        } = self;
        const SECOND_CYCLE: bool = true;
        let mut reads_env = false;
        let mut reads_prim = false;
        for slot in [
            ColorInputSlot::A,
            ColorInputSlot::B,
            ColorInputSlot::C,
            ColorInputSlot::D,
        ] {
            let input = combine.decode_color(slot, SECOND_CYCLE);
            if !ADMITTED_COLOR_INPUTS.iter().any(|admitted| {
                core::mem::discriminant(admitted) == core::mem::discriminant(&input)
            }) {
                return Err(TexrectExecutionError::UnsupportedColorInput { slot, input });
            }
            reads_env |= matches!(input, ColorInput::Environment);
            reads_prim |= matches!(input, ColorInput::Primitive);
        }
        for slot in [
            AlphaInputSlot::A,
            AlphaInputSlot::B,
            AlphaInputSlot::C,
            AlphaInputSlot::D,
        ] {
            let input = combine.decode_alpha(slot, SECOND_CYCLE);
            if !ADMITTED_ALPHA_INPUTS.iter().any(|admitted| {
                core::mem::discriminant(admitted) == core::mem::discriminant(&input)
            }) {
                return Err(TexrectExecutionError::UnsupportedAlphaInput { slot, input });
            }
            reads_env |= matches!(input, AlphaInput::Environment);
            reads_prim |= matches!(input, AlphaInput::Primitive);
        }
        if reads_env && env_color.is_none() {
            return Err(TexrectExecutionError::UnsetConstantRegister {
                register: TexrectConstantRegister::Environment,
            });
        }
        if reads_prim && prim_color.is_none() {
            return Err(TexrectExecutionError::UnsetConstantRegister {
                register: TexrectConstantRegister::Primitive,
            });
        }
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
    /// The unset case substitutes `Color4::from_wire(0)`, which is
    /// unreachable for any register the program actually reads --
    /// [`Self::try_new`] refused that combination already. It is a total
    /// function's answer for a register nothing consults, not a default
    /// that could reach a pixel.
    fn base_inputs(self) -> CombinerInputs {
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
            self.env_color.unwrap_or(Color4::from_wire(0)),
            self.prim_color.unwrap_or(PrimColor::from_wire(0, 0)),
        )
    }
}

/// Which constant color register a [`TexrectExecutionError::UnsetConstantRegister`]
/// names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TexrectConstantRegister {
    Primitive,
    Environment,
}

impl core::fmt::Display for TexrectConstantRegister {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Primitive => formatter.write_str("G_SETPRIMCOLOR"),
            Self::Environment => formatter.write_str("G_SETENVCOLOR"),
        }
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
    shading: TexrectShading,
    resident_bytes: &[u8],
    already_initialized: Option<TargetRectangle>,
) -> Result<CompletedColorTargetWrite, TexrectExecutionError> {
    // Copy cycle blits the texel to the destination with no combiner, which
    // is what the RDP itself does in that mode. One-cycle runs the texel
    // through the color combiner per fragment, which `run_one_cycle`
    // evaluates below. Two-cycle needs the `Combined` carry and a second
    // texel; Fill cycle samples no texture at all. Both refused by name
    // rather than drawing an approximation and calling it a rendered frame.
    let combined = admitted_cycle_evaluates_combiner(other_mode.cycle_type())?;
    // Selector admission runs before any pixel is produced, so an
    // unevaluatable program refuses with an untouched target rather than a
    // half-drawn one. Skipped in Copy cycle, where the RDP consults no
    // combiner program at all and gating on one would refuse a rectangle
    // the hardware draws.
    let base_inputs = if combined {
        Some(shading.validate_one_cycle()?.base_inputs())
    } else {
        None
    };

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
            let rgba = match base_inputs {
                // Copy cycle: the sampled texel's own RGBA8888, unchanged.
                None => decoded.texel().rgba8888(),
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
                Some(base) => {
                    combine_one_texel(shading.combine(), base, decoded.texel().rgba8888())
                }
            };
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

/// This executor's cycle-type admission, as one decision: `Ok(true)` when
/// the mode evaluates the color combiner, `Ok(false)` when it blits the
/// texel unchanged, and a named refusal otherwise.
///
/// Copy cycle blits, which is the RDP's own behavior in that mode.
/// One-cycle runs `(A-B)*C+D` per fragment. Two-cycle needs the `Combined`
/// cross-cycle carry and a second texel, neither of which this executor
/// supplies; Fill cycle samples no texture at all. Measured: WM2000's
/// boot-through-attract window issues 2,520 texrects, **all one-cycle**,
/// none two-cycle and none Fill (`docs/RT64-WM2000-CYCLE-MODES.md` §1).
///
/// A named function rather than an inline match so the decision is
/// reachable from a unit test -- reaching `execute_texture_rectangle`
/// itself requires a live pending TMEM transaction. Measured, not
/// stylistic: while this match was inline, widening it to admit two-cycle
/// left the entire suite green.
fn admitted_cycle_evaluates_combiner(cycle_type: CycleType) -> Result<bool, TexrectExecutionError> {
    match cycle_type {
        CycleType::Copy => Ok(false),
        CycleType::OneCycle => Ok(true),
        CycleType::TwoCycle | CycleType::Fill => {
            Err(TexrectExecutionError::UnsupportedCycleType { cycle_type })
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
/// `alphaCompareValue`, [`run_one_cycle`]'s second return, is deliberately
/// discarded: alpha compare is a separate stage this executor does not run
/// (see this module's Nonclaims).
fn combine_one_texel(combine: CombineParams, base: CombinerInputs, texel: [u8; 4]) -> [u8; 4] {
    let [red, green, blue, alpha] = texel;
    let inputs = CombinerInputs {
        tex_val0: [
            f32::from(red) / 255.0,
            f32::from(green) / 255.0,
            f32::from(blue) / 255.0,
            f32::from(alpha) / 255.0,
        ],
        ..base
    };
    let (combined_color, _alpha_compare) = run_one_cycle(combine, inputs);
    combined_color.map(|channel| (channel * 255.0).round() as u8)
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
            Some(Color4::from_wire(ENV_WIRE)),
            Some(PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE)),
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
        combine_one_texel(shading.combine(), shading.base_inputs(), texel)
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
            Some(Color4::from_wire(PRIM_WIRE)),
            Some(PrimColor::from_wire(PRIM_LOD_W0, ENV_WIRE)),
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
        let shading = TexrectShading::new(over, None, Some(one_register))
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
        let shading = TexrectShading::new(negative, None, Some(one_register))
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
        let lerp = combine_one_texel(env_lerp_program(), base, texel);
        let flat = combine_one_texel(flat_primitive_program(), base, texel);
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
            Some(Color4::from_wire(0x0000_0000)),
            Some(PrimColor::from_wire(0, 0x8080_8080)),
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

    /// **`Shade` is refused by name**, not combined against an invented
    /// zero. Measured absent from all 2,520 WM2000 texrects, and this
    /// executor has no vertex-interpolated color to supply.
    #[test]
    fn a_shade_reading_program_is_refused_by_name() {
        // Color A index 4 is SHADE in the shared common table.
        let shade_in_color = pack_second_cycle([4, 8, 16, 7], [7, 7, 7, 7]);
        assert_eq!(
            TexrectShading::new(shade_in_color, None, None).validate_one_cycle(),
            Err(TexrectExecutionError::UnsupportedColorInput {
                slot: ColorInputSlot::A,
                input: ColorInput::Shade,
            }),
            "a program reading SHADE in a color slot must be refused, naming the slot and the \
             selector"
        );
        // And the alpha side, which has its own table.
        let shade_in_alpha = pack_second_cycle([8, 8, 16, 7], [4, 7, 7, 7]);
        assert_eq!(
            TexrectShading::new(shade_in_alpha, None, None).validate_one_cycle(),
            Err(TexrectExecutionError::UnsupportedAlphaInput {
                slot: AlphaInputSlot::A,
                input: AlphaInput::Shade,
            }),
            "a program reading SHADE in an alpha slot must be refused too"
        );
        // The message names the selector, so a future title's log says what
        // is missing rather than only that something is.
        let message = TexrectShading::new(shade_in_color, None, None)
            .validate_one_cycle()
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("Shade"),
            "the refusal must name the selector: {message}"
        );
    }

    /// The other unmeasured selectors are refused too, each by name -- not
    /// only `Shade`. Swept over every selector the wire can express in
    /// color slot A and alpha slot A, so a selector added to `ColorInput`
    /// later cannot be silently admitted.
    #[test]
    fn every_unmeasured_selector_is_refused() {
        for index in 0u32..16 {
            let params = pack_second_cycle([index, 8, 16, 7], [7, 7, 7, 7]);
            let input = params.decode_color(ColorInputSlot::A, true);
            let admitted = ADMITTED_COLOR_INPUTS
                .iter()
                .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
            let result = TexrectShading::new(
                params,
                Some(Color4::from_wire(ENV_WIRE)),
                Some(PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE)),
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
            let admitted = ADMITTED_ALPHA_INPUTS
                .iter()
                .any(|a| core::mem::discriminant(a) == core::mem::discriminant(&input));
            let result = TexrectShading::new(
                params,
                Some(Color4::from_wire(ENV_WIRE)),
                Some(PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE)),
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

    /// **A program reading an unset constant register is refused**, and
    /// only when it actually reads it.
    #[test]
    fn an_unset_constant_register_is_refused_only_when_the_program_reads_it() {
        assert_eq!(
            TexrectShading::new(
                env_lerp_program(),
                None,
                Some(PrimColor::from_wire(0, PRIM_WIRE))
            )
            .validate_one_cycle(),
            Err(TexrectExecutionError::UnsetConstantRegister {
                register: TexrectConstantRegister::Environment,
            }),
            "program 1 reads ENVIRONMENT, so an unset env register must be refused"
        );
        assert_eq!(
            TexrectShading::new(env_lerp_program(), Some(Color4::from_wire(ENV_WIRE)), None)
                .validate_one_cycle(),
            Err(TexrectExecutionError::UnsetConstantRegister {
                register: TexrectConstantRegister::Primitive,
            }),
            "program 1 reads PRIMITIVE, so an unset prim register must be refused"
        );
        // Program 2 reads PRIMITIVE but never ENVIRONMENT, so an unset env
        // register must NOT refuse it -- the gate is per-register-actually-
        // read, not a blanket requirement.
        assert!(
            TexrectShading::new(
                flat_primitive_program(),
                None,
                Some(PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE))
            )
            .validate_one_cycle()
            .is_ok(),
            "program 2 never reads ENVIRONMENT, so its absence must not refuse the rectangle"
        );
        // A ZERO-only program reads neither.
        let neither = pack_second_cycle([8, 8, 16, 7], [7, 7, 7, 7]);
        assert!(
            TexrectShading::new(neither, None, None)
                .validate_one_cycle()
                .is_ok(),
            "a program reading neither constant register must not require either"
        );
    }

    /// **Two-cycle and Fill are refused at the EXECUTOR, not only in the
    /// error type's prose** -- mutant (f), and this test exists because
    /// that mutant SURVIVED its first draft.
    ///
    /// Widening the executor's cycle match to admit `TwoCycle` alongside
    /// `OneCycle` left the whole suite green: the sibling test below
    /// asserts only the error's *message*, which a widened gate never
    /// constructs, and no fixture executes a two-cycle texrect.
    ///
    /// Reaching `execute_texture_rectangle` needs a live pending TMEM
    /// transaction, which no unit test can build. What this test pins
    /// instead is the decision the executor makes, extracted as
    /// [`admitted_cycle_evaluates_combiner`] -- the same match, called by
    /// the executor, so widening it here is caught. Exhaustive over all
    /// four `CycleType` variants, so a fifth added later cannot be
    /// silently admitted either.
    #[test]
    fn the_executor_admits_exactly_copy_and_one_cycle() {
        assert_eq!(
            admitted_cycle_evaluates_combiner(CycleType::Copy),
            Ok(false),
            "Copy is admitted and evaluates NO combiner"
        );
        assert_eq!(
            admitted_cycle_evaluates_combiner(CycleType::OneCycle),
            Ok(true),
            "OneCycle is admitted and DOES evaluate the combiner"
        );
        for refused in [CycleType::TwoCycle, CycleType::Fill] {
            assert_eq!(
                admitted_cycle_evaluates_combiner(refused),
                Err(TexrectExecutionError::UnsupportedCycleType {
                    cycle_type: refused
                }),
                "{refused:?} must be refused by name at the executor's own gate"
            );
        }
    }

    /// **Two-cycle and Fill remain refused by name** -- the admission
    /// widened by exactly one mode, not into a blanket acceptance.
    ///
    /// Checked at the enum rather than through the executor because
    /// reaching the executor needs a live pending TMEM transaction, which
    /// the end-to-end tests supply; what is pinned here is that the mode
    /// set this module claims is `{Copy, OneCycle}` and its complement is
    /// named.
    #[test]
    fn the_admitted_cycle_set_is_exactly_copy_and_one_cycle() {
        for cycle_type in [CycleType::TwoCycle, CycleType::Fill] {
            let error = TexrectExecutionError::UnsupportedCycleType { cycle_type };
            let message = error.to_string();
            assert!(
                message.contains(&format!("{cycle_type:?}")),
                "the refusal must name the mode it refused: {message}"
            );
            assert!(
                message.contains("Copy") && message.contains("OneCycle"),
                "the refusal must state which modes ARE admitted: {message}"
            );
        }
    }
}
