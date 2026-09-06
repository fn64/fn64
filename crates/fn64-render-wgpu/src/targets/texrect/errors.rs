use super::fragment::TexrectNoiseStage;
use super::*;

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
