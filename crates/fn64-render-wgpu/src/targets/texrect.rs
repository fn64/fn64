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

mod combine;
mod errors;
mod execute;
mod fragment;
mod geometry;
mod shading;

pub use errors::TexrectExecutionError;
pub use execute::execute_texture_rectangle;
pub use geometry::{RdpScissorRect, TexrectAxis, TexrectDraw};
pub use shading::{
    CombinerProgramCycles, TexrectConstantRegister, TexrectShading, TexrectTileBinding,
};
pub use combine::TexrectBlendRegisters;

pub(in crate::targets) use combine::{
    admitted_cycle_evaluation, combine_one_texel, combine_one_texel_prepared_two_cycle,
    TexrectCombinerEvaluation,
};
pub(in crate::targets) use fragment::{
    blend_and_write_pixel, blend_and_write_pixel_with_coverage, require_blendable_mode,
    TexrectFragmentStages,
};

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
