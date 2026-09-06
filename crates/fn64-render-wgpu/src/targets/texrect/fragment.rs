use super::*;

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
pub(super) const NOISE_DITHER_THRESHOLD: AlphaCompareNoise = AlphaCompareNoise(7);

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
    pub(super) alpha_dither: AlphaDither,
    pub(super) rgb_dither: RgbDither,
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
    pub(super) fn coverage_for(
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
pub(in crate::targets) fn require_blendable_mode(
    state: BlendModeState,
) -> Result<(), TexrectExecutionError> {
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
pub(super) fn read_pixel(format: ColorTargetFormat, source: &[u8]) -> BlendFramebufferSample {
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
pub(super) fn blend_texrect_fragment(
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
pub(in crate::targets) fn blend_and_write_pixel(
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
pub(in crate::targets) fn blend_and_write_pixel_with_coverage(
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
pub(super) fn alpha_compare_texrect_fragment(
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
pub(super) fn write_pixel(
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
