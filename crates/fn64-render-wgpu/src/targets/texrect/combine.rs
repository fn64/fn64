use super::*;

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
pub(in crate::targets) fn admitted_cycle_evaluation(
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
pub(in crate::targets) enum TexrectCombinerEvaluation {
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
    pub(in crate::targets) const fn validated_cycles(self) -> Option<CombinerProgramCycles> {
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
pub(in crate::targets) fn combine_one_texel(
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

pub(in crate::targets) fn combine_one_texel_prepared_two_cycle(
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
    pub(in crate::targets) blend_color: Color4,
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
    pub(in crate::targets) const fn blend_color(self) -> Color4 {
        self.blend_color
    }

    pub(in crate::targets) fn mode_state(self, other_mode: OtherMode) -> BlendModeState {
        BlendModeState {
            other_mode,
            blend_color_register: self.blend_color.rgba8(),
            fog_color: self.fog_color.rgba8(),
        }
    }
}
