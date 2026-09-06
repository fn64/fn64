use super::*;

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
pub(super) const ADMITTED_COLOR_INPUTS: [ColorInput; 9] = [
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
pub(super) const ADMITTED_ALPHA_INPUTS: [AlphaInput; 6] = [
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
    pub(in crate::targets) fn base_inputs(self) -> CombinerInputs {
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
pub(super) struct RankOneCi4Rgba16;

fn rank_one_ci4_rgba16_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        crate::diag_env::diag_env("FN64_TEXRECT_RANK_ONE_SPECIALIZATION")
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

    pub(super) fn admit(
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

    pub(super) fn sample<S: crate::TmemByteSource + ?Sized>(
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
#[path = "tests/rank_one_ci4_rgba16.rs"]
mod rank_one_ci4_rgba16_tests;
