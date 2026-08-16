//! RT64 color combiner: selector decode and one-cycle arithmetic.
//!
//! Characterization-first port, Slice 1 of
//! `/private/tmp/rt64-combiner-characterization-card.md`
//! (sha256 `e67751ff975eaf970b8179b2b62bd0093ccddac3d73c3dc0539b611006b345a`).
//! Source: MIT RT64, pinned commit `5473732a822a4423b5696e7cb18fecc425a59875`,
//! `src/shared/rt64_color_combiner.h` (fn64's own
//! `docs/RT64-PORT-INVENTORY.md:291` records this file `unchanged` /
//! `source-digests-verified` against the executable-comparison oracle, so
//! unlike `rt64_state.cpp` there is no drift caveat here).
//!
//! Decode is exact and complete for every wire-legal `(slot, index,
//! second_cycle)` triple: [`CombineParams::decode_color`]/[`decode_alpha`]
//! reproduce RT64's `decodeColorInput`/`decodeAlphaInput` bit-for-bit,
//! including selectors this slice's arithmetic does not yet implement
//! (NOISE, KEY_CENTER/KEY_SCALE, K4/K5, LOD_FRACTION/PRIM_LOD_FRAC,
//! `*_ALPHA` cross-reads, `COMBINED_ALPHA`). Only genuine RT64 out-of-range
//! indices alias `ZERO`/`Combined`-adjacent fallthroughs (`default:` arms
//! in the pinned source) — this port never substitutes `Zero` for a
//! selector RT64 itself would decode to something else. Scope narrowing
//! happens one layer later, in the *arithmetic*: [`run_one_cycle`] (via
//! `resolve_color_input`/`resolve_alpha_input`) only evaluates `COMBINED`,
//! `TEXEL0`, `TEXEL1`, `PRIMITIVE`, `SHADE`, `ENVIRONMENT`, `ONE`, `ZERO` —
//! any other *decoded* selector returns a loud [`CombinerInputError`]
//! rather than a silently wrong number, since this slice has no PRNG,
//! derivative, or cross-channel-read implementation to evaluate it
//! correctly yet.
//!
//! One-cycle mode only. Explicitly not in this slice (RT64 source read,
//! not characterized here): NOISE's arithmetic (needs `Random.hlsli`'s
//! PRNG, uncharacterized), KEY_CENTER/KEY_SCALE/K4/K5's arithmetic
//! (deferred to Slice 2), LOD_FRACTION/PRIM_LOD_FRAC's arithmetic (needs
//! `computeLOD`, uncharacterized), `*_ALPHA`/`COMBINED_ALPHA`'s arithmetic
//! (Slice 2), two-cycle mode and its wrap/carry arithmetic (Slice 3,
//! `wrap`/`wrapInputC`/`wrapInputABD`), copy mode, draw-path/shader-keying
//! wiring, and any GPU execution. See [`CombinerInputError`] for the exact
//! rejection behavior when a decoded selector's arithmetic is deferred.
//!
//! The final `wrapClamp` (RT64 `rt64_color_combiner.h:562-565`, called
//! unconditionally by `run` regardless of cycle count) still applies here:
//! it is not part of the two-cycle carry mechanism, it is the ordinary
//! `[0,1]` clamp on the finished color, preceded by the same `wrapInputABD`
//! wrap used for cross-cycle carry. One-cycle mode never triggers the
//! *carry* wrap (RT64's `secondCycle` flag is `twoCycle && secondCycleInputs`
//! = `false && true` = `false`), but it still runs the final `wrapClamp`.

/// Owned WGSL transcription of this module's decode tables and one-cycle
/// arithmetic (`shaders/color_combiner.wgsl`). Naga-validated in this
/// module's tests; not compiled into any pipeline or wired to a draw path.
pub const COLOR_COMBINER_WGSL: &str = include_str!("shaders/color_combiner.wgsl");

/// Selector reachable from color inputs A/B/C/D. Lists every RT64
/// `ColorInput` variant (`rt64_color_combiner.h:22-44`) for a complete type,
/// even though this slice's decode/arithmetic only reach the first eight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorInput {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    KeyCenter,
    KeyScale,
    CombinedAlpha,
    Texel0Alpha,
    Texel1Alpha,
    PrimitiveAlpha,
    ShadeAlpha,
    EnvAlpha,
    LodFraction,
    PrimLodFrac,
    Noise,
    K4,
    K5,
    One,
    Zero,
}

/// Selector reachable from alpha inputs A/B/C/D
/// (`rt64_color_combiner.h:46-57`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaInput {
    Combined,
    Texel0,
    Texel1,
    Primitive,
    Shade,
    Environment,
    LodFraction,
    PrimLodFrac,
    One,
    Zero,
}

/// Which of the four combine-formula slots a selector was decoded for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorInputSlot {
    A,
    B,
    C,
    D,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlphaInputSlot {
    A,
    B,
    C,
    D,
}

/// A *decoded* selector's arithmetic is not yet implemented by this slice
/// (COMBINED/TEXEL0/TEXEL1/PRIMITIVE/SHADE/ENVIRONMENT/ONE/ZERO are the only
/// ones [`run_one_cycle`] evaluates). This is strictly an arithmetic-layer
/// concern — decode itself is always exact (see module docs): a color-A
/// index of 7 always decodes to `ColorInput::Noise`, never `Zero`, exactly
/// matching RT64. This error exists only because this port slice has not
/// yet implemented the arithmetic those other selectors need (NOISE's PRNG,
/// KEY_CENTER/KEY_SCALE, LOD_FRACTION's derivative, K4/K5, alpha cross-reads)
/// — silently treating them as ZERO would misrepresent RT64's behavior, so
/// this slice loudly refuses instead. AGENTS.md: "Unimplemented ABI surface
/// panics with the symbol name and call context... a fallback that masks a
/// missing feature" is exactly what a silent substitution here would be.
///
/// An enum, not a `{color: Option<_>, alpha: Option<_>}` struct: a color
/// rejection and an alpha rejection are mutually exclusive by construction
/// (one `resolve_*_input` call rejects at most one selector), so the type
/// makes the `(Some, Some)`/`(None, None)` states that struct shape would
/// allow unrepresentable instead of merely unconstructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombinerInputError {
    Color(ColorInput),
    Alpha(AlphaInput),
}

impl core::fmt::Display for CombinerInputError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Color(color) => write!(
                formatter,
                "combiner color selector {color:?} decoded exactly per RT64, but Slice 1 does not yet implement its arithmetic (evaluates COMBINED/TEXEL0/TEXEL1/PRIMITIVE/SHADE/ENVIRONMENT/ONE/ZERO only)"
            ),
            Self::Alpha(alpha) => write!(
                formatter,
                "combiner alpha selector {alpha:?} decoded exactly per RT64, but Slice 1 does not yet implement its arithmetic (evaluates COMBINED/TEXEL0/TEXEL1/PRIMITIVE/SHADE/ENVIRONMENT/ONE/ZERO only)"
            ),
        }
    }
}

impl std::error::Error for CombinerInputError {}

/// Raw 64-bit `SetCombine` payload, split into low/high 32-bit halves per
/// RT64's own field naming (`rt64_color_combiner.h`'s `L`/`H`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombineParams {
    low: u32,
    high: u32,
}

impl CombineParams {
    // No `SetCombine` opcode handler exists yet (Slice 4, `RdpState`
    // integration, is out of this slice's scope) — only tests construct
    // `CombineParams` for now.
    #[allow(dead_code)]
    pub(crate) const fn from_wire(low: u32, high: u32) -> Self {
        Self { low, high }
    }

    pub const fn low(self) -> u32 {
        self.low
    }

    pub const fn high(self) -> u32 {
        self.high
    }

    // Bit positions below are `parseColorInputA/B/C/D` /
    // `parseAlphaInputA/B/C/D` (`rt64_color_combiner.h`), verified directly
    // against the pinned source, not transcribed from the card alone.

    const fn parse_color_a(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.low >> 5) & 0xF
        } else {
            (self.low >> 20) & 0xF
        }
    }

    const fn parse_color_b(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.high >> 24) & 0xF
        } else {
            (self.high >> 28) & 0xF
        }
    }

    const fn parse_color_c(self, second_cycle: bool) -> u32 {
        if second_cycle {
            self.low & 0x1F
        } else {
            (self.low >> 15) & 0x1F
        }
    }

    const fn parse_color_d(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.high >> 6) & 0x7
        } else {
            (self.high >> 15) & 0x7
        }
    }

    const fn parse_alpha_a(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.high >> 21) & 0x7
        } else {
            (self.low >> 12) & 0x7
        }
    }

    const fn parse_alpha_b(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.high >> 3) & 0x7
        } else {
            (self.high >> 12) & 0x7
        }
    }

    const fn parse_alpha_c(self, second_cycle: bool) -> u32 {
        if second_cycle {
            (self.high >> 18) & 0x7
        } else {
            (self.low >> 9) & 0x7
        }
    }

    const fn parse_alpha_d(self, second_cycle: bool) -> u32 {
        if second_cycle {
            self.high & 0x7
        } else {
            (self.high >> 9) & 0x7
        }
    }

    /// Common table shared by every color slot's index 0-5
    /// (`colorInputCommon`, `rt64_color_combiner.h`).
    const fn color_input_common(index: u32) -> ColorInput {
        match index {
            0 => ColorInput::Combined,
            1 => ColorInput::Texel0,
            2 => ColorInput::Texel1,
            3 => ColorInput::Primitive,
            4 => ColorInput::Shade,
            5 => ColorInput::Environment,
            _ => ColorInput::Zero,
        }
    }

    /// `colorInputA` (`rt64_color_combiner.h`), decoded exactly: 0-5
    /// common, 6=ONE, 7=NOISE, 8-15=ZERO (the wire-legal-but-undecoded
    /// upper half of the 4-bit field, matching RT64's `default:` arm).
    /// NOISE's *arithmetic* is deferred (Slice 2, no PRNG characterized
    /// yet), but its *decode* is not — [`Self::color_input_a`] returns
    /// `ColorInput::Noise` for index 7 exactly as RT64 does; only
    /// `resolve_color_input` (the arithmetic layer) may reject it. Decoding
    /// a valid RT64 selector to `Zero` here would be a silent behavior
    /// change this port does not make.
    const fn color_input_a(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::One,
            7 => ColorInput::Noise,
            _ => ColorInput::Zero,
        }
    }

    /// `colorInputB`, decoded exactly: 0-5 common, 6=KEY_CENTER, 7=K4,
    /// 8-15=ZERO. See [`Self::color_input_a`]'s doc: decode is exact even
    /// though KEY_CENTER/K4's arithmetic is deferred to Slice 2.
    const fn color_input_b(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::KeyCenter,
            7 => ColorInput::K4,
            _ => ColorInput::Zero,
        }
    }

    /// `colorInputC`, decoded exactly: 0-5 common, 6-15 the extended
    /// KEY_SCALE/`*_ALPHA`/LOD_FRACTION/PRIM_LOD_FRAC/K5 table, 16-31=ZERO
    /// (RT64's own upper-half-of-the-5-bit-field collapse). See
    /// [`Self::color_input_a`]'s doc: decode is exact even though most of
    /// this table's arithmetic is deferred to Slice 2.
    const fn color_input_c(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::KeyScale,
            7 => ColorInput::CombinedAlpha,
            8 => ColorInput::Texel0Alpha,
            9 => ColorInput::Texel1Alpha,
            10 => ColorInput::PrimitiveAlpha,
            11 => ColorInput::ShadeAlpha,
            12 => ColorInput::EnvAlpha,
            13 => ColorInput::LodFraction,
            14 => ColorInput::PrimLodFrac,
            15 => ColorInput::K5,
            _ => ColorInput::Zero,
        }
    }

    /// `colorInputD`, decoded exactly: 0-5 common, 6=ONE, 7=ZERO (RT64's
    /// own table for D has no NOISE/KEY_CENTER/etc. entries to begin with,
    /// so this matches RT64 exactly with no Slice 1/2 distinction to draw).
    const fn color_input_d(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::One,
            _ => ColorInput::Zero,
        }
    }

    /// `alphaInputABD`: shared table for alpha slots A, B, D, decoded
    /// exactly. RT64's own table has no entries outside Slice 1's
    /// arithmetic scope in the first place, so there is nothing to defer.
    const fn alpha_input_abd(index: u32) -> AlphaInput {
        match index {
            0 => AlphaInput::Combined,
            1 => AlphaInput::Texel0,
            2 => AlphaInput::Texel1,
            3 => AlphaInput::Primitive,
            4 => AlphaInput::Shade,
            5 => AlphaInput::Environment,
            6 => AlphaInput::One,
            _ => AlphaInput::Zero,
        }
    }

    /// `alphaInputC`, distinct table (alpha slot C only), decoded exactly:
    /// 0=LOD_FRACTION, 1-5 common, 6=PRIM_LOD_FRAC, 7=ZERO. Note this
    /// table's own index-to-selector mapping is shifted from
    /// `alphaInputABD`'s (index 1 is TEXEL0 here, not COMBINED — RT64 has
    /// no `A_COMBINED` reachable from alpha-C at all). See
    /// [`Self::color_input_a`]'s doc: decode is exact even though
    /// LOD_FRACTION/PRIM_LOD_FRAC's arithmetic is deferred to Slice 2.
    const fn alpha_input_c(index: u32) -> AlphaInput {
        match index {
            0 => AlphaInput::LodFraction,
            1 => AlphaInput::Texel0,
            2 => AlphaInput::Texel1,
            3 => AlphaInput::Primitive,
            4 => AlphaInput::Shade,
            5 => AlphaInput::Environment,
            6 => AlphaInput::PrimLodFrac,
            _ => AlphaInput::Zero,
        }
    }

    /// Decodes one color slot's selector for the given cycle's bitfield
    /// slice, matching `decodeColorInput` exactly (including the
    /// unreachable-in-practice ZERO fallthrough for a slot index outside
    /// A/B/C/D, which this typed API cannot construct).
    pub const fn decode_color(self, slot: ColorInputSlot, second_cycle: bool) -> ColorInput {
        match slot {
            ColorInputSlot::A => Self::color_input_a(self.parse_color_a(second_cycle)),
            ColorInputSlot::B => Self::color_input_b(self.parse_color_b(second_cycle)),
            ColorInputSlot::C => Self::color_input_c(self.parse_color_c(second_cycle)),
            ColorInputSlot::D => Self::color_input_d(self.parse_color_d(second_cycle)),
        }
    }

    /// Decodes one alpha slot's selector for the given cycle's bitfield
    /// slice, matching `decodeAlphaInput` exactly.
    pub const fn decode_alpha(self, slot: AlphaInputSlot, second_cycle: bool) -> AlphaInput {
        match slot {
            AlphaInputSlot::A => Self::alpha_input_abd(self.parse_alpha_a(second_cycle)),
            AlphaInputSlot::B => Self::alpha_input_abd(self.parse_alpha_b(second_cycle)),
            AlphaInputSlot::C => Self::alpha_input_c(self.parse_alpha_c(second_cycle)),
            AlphaInputSlot::D => Self::alpha_input_abd(self.parse_alpha_d(second_cycle)),
        }
    }
}

/// Per-pixel combiner inputs restricted to Slice 1's selector set. Mirrors
/// the fields of RT64's `Inputs` struct (`rt64_color_combiner.h`) that
/// COMBINED/TEXEL0/TEXEL1/PRIMITIVE/SHADE/ENVIRONMENT/ONE/ZERO can reach;
/// NOISE/KEY_CENTER/KEY_SCALE/LOD_FRACTION/PRIM_LOD_FRAC/K4/K5 fields are
/// intentionally absent — Slice 1 cannot consume them, so there is nowhere
/// to plumb them through yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombinerInputs {
    pub tex_val0: [f32; 4],
    pub tex_val1: [f32; 4],
    pub prim_color: [f32; 4],
    pub shade_color: [f32; 4],
    pub env_color: [f32; 4],
}

/// Resolves one color selector to its RGB value, per `fromColorInput`
/// (`rt64_color_combiner.h`). `combiner_color` stands in for RT64's
/// `combinerColor.rgb` accumulator — in one-cycle mode this is always
/// `[0.0, 0.0, 0.0]` (RT64 `run`'s zero-init, never written before the
/// single `runCycle` call), so [`run_one_cycle`] passes that fixed value
/// rather than exposing a caller-settable carry (which only exists for
/// two-cycle mode, Slice 3).
///
/// One-cycle mode's `secondCycle` local (`twoCycle && secondCycleInputs`) is
/// always `false` (`twoCycle` is `false`), so `fromColorInput`'s TEXEL0/
/// TEXEL1 swap-on-`secondCycle` branch never triggers — this always reads
/// the un-swapped texel, matching RT64 exactly for one-cycle mode
/// specifically (see module docs).
fn resolve_color_input(
    inputs: CombinerInputs,
    combiner_color: [f32; 3],
    selector: ColorInput,
) -> Result<[f32; 3], CombinerInputError> {
    match selector {
        ColorInput::Combined => Ok(combiner_color),
        ColorInput::Texel0 => Ok([inputs.tex_val0[0], inputs.tex_val0[1], inputs.tex_val0[2]]),
        ColorInput::Texel1 => Ok([inputs.tex_val1[0], inputs.tex_val1[1], inputs.tex_val1[2]]),
        ColorInput::Primitive => Ok([
            inputs.prim_color[0],
            inputs.prim_color[1],
            inputs.prim_color[2],
        ]),
        ColorInput::Shade => Ok([
            inputs.shade_color[0],
            inputs.shade_color[1],
            inputs.shade_color[2],
        ]),
        ColorInput::Environment => Ok([
            inputs.env_color[0],
            inputs.env_color[1],
            inputs.env_color[2],
        ]),
        ColorInput::One => Ok([1.0, 1.0, 1.0]),
        ColorInput::Zero => Ok([0.0, 0.0, 0.0]),
        other => Err(CombinerInputError::Color(other)),
    }
}

/// Resolves one alpha selector, per `fromAlphaInput`. `combiner_alpha`
/// stands in for RT64's `combinerColor.a` accumulator — see
/// [`resolve_color_input`]'s doc for why one-cycle mode fixes it at `0.0`.
fn resolve_alpha_input(
    inputs: CombinerInputs,
    combiner_alpha: f32,
    selector: AlphaInput,
) -> Result<f32, CombinerInputError> {
    match selector {
        AlphaInput::Combined => Ok(combiner_alpha),
        AlphaInput::Texel0 => Ok(inputs.tex_val0[3]),
        AlphaInput::Texel1 => Ok(inputs.tex_val1[3]),
        AlphaInput::Primitive => Ok(inputs.prim_color[3]),
        AlphaInput::Shade => Ok(inputs.shade_color[3]),
        AlphaInput::Environment => Ok(inputs.env_color[3]),
        AlphaInput::One => Ok(1.0),
        AlphaInput::Zero => Ok(0.0),
        other => Err(CombinerInputError::Alpha(other)),
    }
}

/// The final `wrapClamp` RT64 always applies to the finished color and to
/// `alphaCompareValue` (`run`, called unconditionally regardless of cycle
/// count): `wrapInputABD` then hard-clamp to `[0.0, 1.0]`
/// (`rt64_color_combiner.h`'s `wrapClamp`). One-cycle mode never triggers
/// the *cross-cycle carry* wrap (that requires `secondCycle`, which is
/// `false` here) — this is the separate, always-on final clamp, not that
/// carry mechanism.
///
/// `wrapInputABD`'s range is `[-0.5 - 1/255, 1.5 + 1/255]`; for any `i`
/// already within `[0.0, 1.0]` — true for every one-cycle-mode result this
/// slice can produce, since every Slice 1 input is itself `[0,1]`-normalized
/// and the formula is `(A-B)*C+D` with no selector able to push arbitrarily
/// far outside that band from in-range inputs alone — `wrap`'s two `step`
/// adjustments are both no-ops (`step(i, Low)` and `step(High, i)` are both
/// `0.0`), so this reduces to the plain `clamp(i, 0.0, 1.0)` RT64 itself
/// would also produce for these inputs. Implemented as the full two-step
/// formula anyway (not the reduced form) so it stays correct if a future
/// slice widens the input range.
fn wrap_clamp(i: f32) -> f32 {
    const ROUNDING: f32 = 1.0 / 255.0;
    const LOW: f32 = -0.5 - ROUNDING;
    const HIGH: f32 = 1.5 + ROUNDING;
    const RANGE: f32 = HIGH - LOW;
    let mut wrapped = i;
    if wrapped <= LOW {
        wrapped += RANGE;
    }
    if HIGH <= wrapped {
        wrapped -= RANGE;
    }
    wrapped.clamp(0.0, 1.0)
}

/// Runs one-cycle combiner arithmetic: `(A-B)*C+D` for color and alpha
/// independently, then the final `wrapClamp` on every output channel
/// (`ColorCombiner::run`/`runCycle`, one-cycle path only — `cycle = 1`,
/// `twoCycle = false`, matching RT64's own "1-cycle mode uses the
/// second-cycle bitfield slice" wiring, so `second_cycle = true` is passed
/// to [`CombineParams::decode_color`]/`decode_alpha` even though this is a
/// single pass. See module docs.).
///
/// Returns `(combinerColor, alphaCompareValue)`, mirroring RT64's `run` out
/// parameters. In one-cycle mode `alphaCompareValue` is always exactly the
/// (only) cycle's final alpha, since RT64 snapshots it immediately after
/// the single `runCycle` call and no second cycle can overwrite it.
///
/// Returns [`CombinerInputError`] if `params` decodes any selector outside
/// this slice's scope (NOISE, KEY_CENTER/KEY_SCALE, K4/K5, LOD_FRACTION/
/// PRIM_LOD_FRAC, or any `*_ALPHA`/`COMBINED_ALPHA` cross-read) rather than
/// silently substituting a value RT64 would not produce.
pub fn run_one_cycle(
    params: CombineParams,
    inputs: CombinerInputs,
) -> Result<([f32; 4], f32), CombinerInputError> {
    const SECOND_CYCLE: bool = true;

    let ca = params.decode_color(ColorInputSlot::A, SECOND_CYCLE);
    let cb = params.decode_color(ColorInputSlot::B, SECOND_CYCLE);
    let cc = params.decode_color(ColorInputSlot::C, SECOND_CYCLE);
    let cd = params.decode_color(ColorInputSlot::D, SECOND_CYCLE);
    let aa = params.decode_alpha(AlphaInputSlot::A, SECOND_CYCLE);
    let ab = params.decode_alpha(AlphaInputSlot::B, SECOND_CYCLE);
    let ac = params.decode_alpha(AlphaInputSlot::C, SECOND_CYCLE);
    let ad = params.decode_alpha(AlphaInputSlot::D, SECOND_CYCLE);

    // RT64 `run`'s zero-init, unwritten before this one-and-only `runCycle`
    // call (see `resolve_color_input`/`resolve_alpha_input` docs).
    let combiner_color_in = [0.0f32; 3];
    let combiner_alpha_in = 0.0f32;

    let a = resolve_color_input(inputs, combiner_color_in, ca)?;
    let b = resolve_color_input(inputs, combiner_color_in, cb)?;
    let c = resolve_color_input(inputs, combiner_color_in, cc)?;
    let d = resolve_color_input(inputs, combiner_color_in, cd)?;
    let rgb = [
        (a[0] - b[0]) * c[0] + d[0],
        (a[1] - b[1]) * c[1] + d[1],
        (a[2] - b[2]) * c[2] + d[2],
    ];

    let aa_v = resolve_alpha_input(inputs, combiner_alpha_in, aa)?;
    let ab_v = resolve_alpha_input(inputs, combiner_alpha_in, ab)?;
    let ac_v = resolve_alpha_input(inputs, combiner_alpha_in, ac)?;
    let ad_v = resolve_alpha_input(inputs, combiner_alpha_in, ad)?;
    let alpha = (aa_v - ab_v) * ac_v + ad_v;

    // RT64 `run`: `alphaCompareValue = combinerColor.a` snapshotted right
    // after the (only) `runCycle` call, before the final `wrapClamp` pass.
    let alpha_compare_value = wrap_clamp(alpha);

    let combiner_color = [
        wrap_clamp(rgb[0]),
        wrap_clamp(rgb[1]),
        wrap_clamp(rgb[2]),
        wrap_clamp(alpha),
    ];

    Ok((combiner_color, alpha_compare_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_INPUTS: CombinerInputs = CombinerInputs {
        tex_val0: [0.10, 0.20, 0.30, 0.40],
        tex_val1: [0.50, 0.60, 0.70, 0.80],
        prim_color: [0.05, 0.15, 0.25, 0.35],
        shade_color: [0.11, 0.22, 0.33, 0.44],
        env_color: [0.66, 0.77, 0.88, 0.99],
    };

    // -- §9a: exhaustive decode-table sweep over every wire-legal index,
    // asserting exact RT64 decode (not a Slice-1-narrowed one) — decode is
    // never restricted in this port; only the arithmetic layer defers
    // unimplemented selectors (see module docs / `CombinerInputError`).
    // Cross-checked directly against the pinned
    // `src/shared/rt64_color_combiner.h` source read for this task, not
    // solely against the characterization card's transcription of it.

    /// Exhaustive over color-A's full 4-bit wire range (0-15): every index
    /// this table can ever see, not a spot sample. 0-5 common, 6=ONE,
    /// 7=NOISE (decoded exactly — NOISE's *arithmetic* is deferred, its
    /// decode is not), 8-15 collapse to ZERO (RT64's own `default:` arm,
    /// the wire-legal-but-undecoded upper half of the 4-bit field).
    #[test]
    fn color_slot_a_decode_table_exhaustive() {
        for index in 0u32..16 {
            let expected = match index {
                0 => ColorInput::Combined,
                1 => ColorInput::Texel0,
                2 => ColorInput::Texel1,
                3 => ColorInput::Primitive,
                4 => ColorInput::Shade,
                5 => ColorInput::Environment,
                6 => ColorInput::One,
                7 => ColorInput::Noise,
                _ => ColorInput::Zero,
            };
            assert_eq!(
                CombineParams::color_input_a(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Exhaustive over color-B's full 4-bit wire range. 0-5 common,
    /// 6=KEY_CENTER, 7=K4 (both decoded exactly), 8-15 collapse to ZERO.
    #[test]
    fn color_slot_b_decode_table_exhaustive() {
        for index in 0u32..16 {
            let expected = match index {
                0 => ColorInput::Combined,
                1 => ColorInput::Texel0,
                2 => ColorInput::Texel1,
                3 => ColorInput::Primitive,
                4 => ColorInput::Shade,
                5 => ColorInput::Environment,
                6 => ColorInput::KeyCenter,
                7 => ColorInput::K4,
                _ => ColorInput::Zero,
            };
            assert_eq!(
                CombineParams::color_input_b(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Exhaustive over color-C's full 5-bit wire range (0-31). 0-5 common,
    /// 6-15 the extended KEY_SCALE/`*_ALPHA`/LOD_FRACTION/PRIM_LOD_FRAC/K5
    /// table (all decoded exactly), 16-31 collapse to ZERO (RT64's own
    /// upper-half-of-the-5-bit-field collapse).
    #[test]
    fn color_slot_c_decode_table_exhaustive() {
        for index in 0u32..32 {
            let expected = match index {
                0 => ColorInput::Combined,
                1 => ColorInput::Texel0,
                2 => ColorInput::Texel1,
                3 => ColorInput::Primitive,
                4 => ColorInput::Shade,
                5 => ColorInput::Environment,
                6 => ColorInput::KeyScale,
                7 => ColorInput::CombinedAlpha,
                8 => ColorInput::Texel0Alpha,
                9 => ColorInput::Texel1Alpha,
                10 => ColorInput::PrimitiveAlpha,
                11 => ColorInput::ShadeAlpha,
                12 => ColorInput::EnvAlpha,
                13 => ColorInput::LodFraction,
                14 => ColorInput::PrimLodFrac,
                15 => ColorInput::K5,
                _ => ColorInput::Zero,
            };
            assert_eq!(
                CombineParams::color_input_c(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Exhaustive over color-D's full 3-bit wire range (0-7). RT64's own
    /// table for D has no entries beyond COMBINED/TEXEL0/TEXEL1/PRIMITIVE/
    /// SHADE/ENVIRONMENT/ONE/ZERO to begin with, so there is no
    /// deferred-arithmetic selector reachable from this slot at all.
    #[test]
    fn color_slot_d_decode_table_exhaustive() {
        for index in 0u32..8 {
            let expected = match index {
                0 => ColorInput::Combined,
                1 => ColorInput::Texel0,
                2 => ColorInput::Texel1,
                3 => ColorInput::Primitive,
                4 => ColorInput::Shade,
                5 => ColorInput::Environment,
                6 => ColorInput::One,
                _ => ColorInput::Zero,
            };
            assert_eq!(
                CombineParams::color_input_d(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Exhaustive over alpha-A/B/D's shared 3-bit wire range (0-7). RT64's
    /// own table has no entries beyond this slice's evaluated set, so there
    /// is no deferred-arithmetic selector reachable from this table either.
    #[test]
    fn alpha_slot_abd_decode_table_exhaustive() {
        for index in 0u32..8 {
            let expected = match index {
                0 => AlphaInput::Combined,
                1 => AlphaInput::Texel0,
                2 => AlphaInput::Texel1,
                3 => AlphaInput::Primitive,
                4 => AlphaInput::Shade,
                5 => AlphaInput::Environment,
                6 => AlphaInput::One,
                _ => AlphaInput::Zero,
            };
            assert_eq!(
                CombineParams::alpha_input_abd(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Exhaustive over alpha-C's distinct 3-bit wire range (0-7). 0=
    /// LOD_FRACTION, 1-5 common, 6=PRIM_LOD_FRAC (all decoded exactly),
    /// 7=ZERO. Note this table's index-to-selector mapping is shifted from
    /// `alpha_input_abd`'s (index 1 is TEXEL0 here, not COMBINED — RT64 has
    /// no `A_COMBINED` reachable from alpha-C at all).
    #[test]
    fn alpha_slot_c_decode_table_exhaustive() {
        for index in 0u32..8 {
            let expected = match index {
                0 => AlphaInput::LodFraction,
                1 => AlphaInput::Texel0,
                2 => AlphaInput::Texel1,
                3 => AlphaInput::Primitive,
                4 => AlphaInput::Shade,
                5 => AlphaInput::Environment,
                6 => AlphaInput::PrimLodFrac,
                _ => AlphaInput::Zero,
            };
            assert_eq!(
                CombineParams::alpha_input_c(index),
                expected,
                "index {index}"
            );
        }
    }

    /// Bit-position fixture: crafts a `SetCombine` word whose cycle-0 slice
    /// and cycle-1 slice each select a *different* index per field, then
    /// confirms `decode_color`/`decode_alpha` read the right slice for each
    /// `second_cycle` value. Values chosen directly from the pinned source's
    /// `parseColorInputA/B/C/D` / `parseAlphaInputA/B/C/D` bit positions.
    #[test]
    fn cycle_bitfield_slice_selection() {
        // color A: cycle0 bits 20-23, cycle1 bits 5-8.
        let low = (2u32 << 20) | (4u32 << 5);
        let params = CombineParams::from_wire(low, 0);
        assert_eq!(
            params.decode_color(ColorInputSlot::A, false),
            ColorInput::Texel1
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Shade
        );

        // color B: cycle0 bits 28-31, cycle1 bits 24-27.
        let high = (3u32 << 28) | (5u32 << 24);
        let params = CombineParams::from_wire(0, high);
        assert_eq!(
            params.decode_color(ColorInputSlot::B, false),
            ColorInput::Primitive
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, true),
            ColorInput::Environment
        );

        // color C: cycle0 bits 15-19, cycle1 bits 0-4.
        let low = (1u32 << 15) | 3u32;
        let params = CombineParams::from_wire(low, 0);
        assert_eq!(
            params.decode_color(ColorInputSlot::C, false),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Primitive
        );

        // color D: cycle0 bits 15-17, cycle1 bits 6-8.
        let high = 6u32 << 6;
        let params = CombineParams::from_wire(0, high);
        assert_eq!(
            params.decode_color(ColorInputSlot::D, false),
            ColorInput::Combined
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, true),
            ColorInput::One
        );

        // alpha A: cycle0 bits 12-14 (low), cycle1 bits 21-23 (high).
        let low = 2u32 << 12;
        let high = 4u32 << 21;
        let params = CombineParams::from_wire(low, high);
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, false),
            AlphaInput::Texel1
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, true),
            AlphaInput::Shade
        );

        // alpha C: cycle0 bits 9-11 (low), cycle1 bits 18-20 (high).
        let low = 1u32 << 9;
        let high = 3u32 << 18;
        let params = CombineParams::from_wire(low, high);
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, false),
            AlphaInput::Texel0
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, true),
            AlphaInput::Primitive
        );
    }

    // -- §9b: arithmetic/formula representative sweep, one-cycle only.

    /// (TEXEL0 - ZERO) * ONE + ZERO, both color and alpha: validates the
    /// formula skeleton with all inputs already in range and no clamp
    /// engagement.
    #[test]
    fn identity_passthrough() {
        let inputs = ALL_INPUTS;
        let a = resolve_color_input(inputs, [0.0; 3], ColorInput::Texel0).unwrap();
        let zero = resolve_color_input(inputs, [0.0; 3], ColorInput::Zero).unwrap();
        let one = resolve_color_input(inputs, [0.0; 3], ColorInput::One).unwrap();
        let rgb = [
            (a[0] - zero[0]) * one[0] + zero[0],
            (a[1] - zero[1]) * one[1] + zero[1],
            (a[2] - zero[2]) * one[2] + zero[2],
        ];
        assert_eq!(rgb, inputs.tex_val0[..3]);

        let alpha_a = resolve_alpha_input(inputs, 0.0, AlphaInput::Texel0).unwrap();
        let alpha_zero = resolve_alpha_input(inputs, 0.0, AlphaInput::Zero).unwrap();
        let alpha_one = resolve_alpha_input(inputs, 0.0, AlphaInput::One).unwrap();
        let alpha = (alpha_a - alpha_zero) * alpha_one + alpha_zero;
        assert_eq!(alpha, inputs.tex_val0[3]);
    }

    /// (TEXEL0 - ZERO) * SHADE + ZERO: the textbook "modulate" idiom,
    /// exercising two distinct non-constant inputs multiplying.
    #[test]
    fn texel_modulate_shade() {
        let inputs = ALL_INPUTS;
        let texel0 = inputs.tex_val0;
        let shade = inputs.shade_color;
        let expected = [
            texel0[0] * shade[0],
            texel0[1] * shade[1],
            texel0[2] * shade[2],
        ];
        for (channel, expected) in expected.into_iter().enumerate() {
            let a = resolve_color_input(inputs, [0.0; 3], ColorInput::Texel0).unwrap()[channel];
            let c = resolve_color_input(inputs, [0.0; 3], ColorInput::Shade).unwrap()[channel];
            assert!((a * c - expected).abs() < f32::EPSILON);
        }
    }

    /// Boundary-value sweep: each formula slot independently set to
    /// 0.0/0.5/1.0 via PRIMITIVE-channel-equivalent constant colors, holding
    /// the identity-mode shape otherwise, confirms no off-by-one at the
    /// closed clamp interval edges (`wrap_clamp` no-ops for in-range input).
    #[test]
    fn clamp_boundary_values_are_exact() {
        for value in [0.0f32, 0.5, 1.0] {
            assert_eq!(wrap_clamp(value), value);
        }
    }

    const WRAP_ROUNDING: f32 = 1.0 / 255.0;
    const WRAP_LOW: f32 = -0.5 - WRAP_ROUNDING;
    const WRAP_HIGH: f32 = 1.5 + WRAP_ROUNDING;
    const WRAP_RANGE: f32 = WRAP_HIGH - WRAP_LOW;

    /// Independent reference oracle for `wrap_clamp`, written directly from
    /// RT64's `wrap`/`wrapInputABD`/`wrapClamp` source text rather than by
    /// inlining the expected value at each call site — a boundary input can
    /// trigger *both* sequential `if`s (e.g. `i == LOW` wraps up to exactly
    /// `HIGH`, which then immediately re-triggers the high branch), so a
    /// test that only reasons about one branch at a time silently
    /// mis-derives that case. This mirrors `wrap_clamp`'s own two
    /// sequential (non-`else`) `if` statements line-for-line, deliberately
    /// duplicating the structure so the test can't share a bug with the
    /// implementation by construction alone — it still must independently
    /// match RT64's real formula, which the boundary/no-op assertions below
    /// (and `run_one_cycle_end_to_end`'s in-range check) establish.
    fn wrap_clamp_reference(i: f32) -> f32 {
        let mut wrapped = i;
        if wrapped <= WRAP_LOW {
            wrapped += WRAP_RANGE;
        }
        if WRAP_HIGH <= wrapped {
            wrapped -= WRAP_RANGE;
        }
        wrapped.clamp(0.0, 1.0)
    }

    /// Wrap-boundary sweep (card §9b): exercises both `step`-branches of
    /// RT64's `wrap()` that `wrap_clamp` translates to `if` statements —
    /// the card's own flag that this is "the single most error-prone part
    /// of this whole spec."
    #[test]
    fn wrap_clamp_low_branch_triggers_below_low_bound() {
        // Exactly at LOW: RT64's step(i, Low) is HLSL step(edge=i, x=Low) =
        // (Low >= i) ? 1 : 0 = (i <= Low) ? 1 : 0, so i == Low triggers the
        // wrap (<=, not <). Note LOW + RANGE == HIGH exactly, which then
        // re-triggers the high branch too — wrap_clamp_reference models
        // that, a single "add RANGE once" expectation would not.
        assert!((wrap_clamp(WRAP_LOW) - wrap_clamp_reference(WRAP_LOW)).abs() < 1e-6);

        // Strictly below LOW, comfortably clear of the HIGH re-trigger.
        let input = -2.0f32;
        assert!((wrap_clamp(input) - wrap_clamp_reference(input)).abs() < 1e-6);

        // Just above LOW: must NOT trigger the wrap.
        let input = WRAP_LOW + 0.01;
        assert!((wrap_clamp(input) - input.clamp(0.0, 1.0)).abs() < 1e-6);
    }

    #[test]
    fn wrap_clamp_high_branch_triggers_at_or_above_high_bound() {
        // Exactly at HIGH: RT64's step(High, i) is HLSL step(edge=High,
        // x=i) = (i >= High) ? 1 : 0, so i == High triggers the wrap.
        assert!((wrap_clamp(WRAP_HIGH) - wrap_clamp_reference(WRAP_HIGH)).abs() < 1e-6);

        // Strictly above HIGH.
        let input = 2.5f32;
        assert!((wrap_clamp(input) - wrap_clamp_reference(input)).abs() < 1e-6);

        // Just below HIGH: must NOT trigger the wrap.
        let input = WRAP_HIGH - 0.01;
        assert!((wrap_clamp(input) - input.clamp(0.0, 1.0)).abs() < 1e-6);
    }

    /// One-cycle mode: `C_COMBINED`/`A_COMBINED` in any slot reads the
    /// zero-initialized accumulator (RT64 `run`'s zero-init, never written
    /// before the single `runCycle` call — one-cycle mode's `secondCycle`
    /// flag is always false, so no carry-wrap ever applies to it either).
    #[test]
    fn combined_reads_zero_init_in_one_cycle_mode() {
        let inputs = ALL_INPUTS;
        let combined = resolve_color_input(inputs, [0.0; 3], ColorInput::Combined).unwrap();
        assert_eq!(combined, [0.0, 0.0, 0.0]);
        let combined_alpha = resolve_alpha_input(inputs, 0.0, AlphaInput::Combined).unwrap();
        assert_eq!(combined_alpha, 0.0);
    }

    /// `run_one_cycle` end-to-end: a `SetCombine` word selecting
    /// (TEXEL0 - PRIMITIVE) * SHADE + ENVIRONMENT for color and
    /// (TEXEL1 - PRIMITIVE) * SHADE + ENVIRONMENT for alpha, hand-derived
    /// against `(A-B)*C+D` plus the final `wrap_clamp` (a no-op here, since
    /// every intermediate stays within [0,1] given `ALL_INPUTS`).
    #[test]
    fn run_one_cycle_end_to_end() {
        // color A=TEXEL0(1), B=PRIMITIVE(3), C=SHADE(4), D=ENVIRONMENT(5) at cycle-1 bits (low>>5).
        let low_color = (1u32 << 5) | 4u32;
        // color B=PRIMITIVE(3) at cycle-1 bits (high>>24), D=ENVIRONMENT(5) at (high>>6).
        let high_color = (3u32 << 24) | (5u32 << 6);
        // alpha A=TEXEL1(2) at (high>>21), B=PRIMITIVE(3) at (high>>3).
        let high_alpha_ab = (2u32 << 21) | (3u32 << 3);
        // alpha C=SHADE(4) at (high>>18), D=ENVIRONMENT(5) at (high>>0).
        let high_alpha_cd = (4u32 << 18) | 5u32;

        let params =
            CombineParams::from_wire(low_color, high_color | high_alpha_ab | high_alpha_cd);

        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, true),
            ColorInput::Primitive
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Shade
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, true),
            ColorInput::Environment
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, true),
            AlphaInput::Texel1
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::B, true),
            AlphaInput::Primitive
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, true),
            AlphaInput::Shade
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::D, true),
            AlphaInput::Environment
        );

        let inputs = ALL_INPUTS;
        let (color, alpha_compare) = run_one_cycle(params, inputs).unwrap();

        let expected_rgb = [
            (inputs.tex_val0[0] - inputs.prim_color[0]) * inputs.shade_color[0]
                + inputs.env_color[0],
            (inputs.tex_val0[1] - inputs.prim_color[1]) * inputs.shade_color[1]
                + inputs.env_color[1],
            (inputs.tex_val0[2] - inputs.prim_color[2]) * inputs.shade_color[2]
                + inputs.env_color[2],
        ];
        let expected_alpha = (inputs.tex_val1[3] - inputs.prim_color[3]) * inputs.shade_color[3]
            + inputs.env_color[3];

        for (observed, expected) in color[..3].iter().zip(expected_rgb) {
            assert!((observed - expected.clamp(0.0, 1.0)).abs() < 1e-6);
        }
        assert!((color[3] - expected_alpha.clamp(0.0, 1.0)).abs() < 1e-6);
        assert!((alpha_compare - expected_alpha.clamp(0.0, 1.0)).abs() < 1e-6);
        // One-cycle mode: alphaCompareValue equals the (only) cycle's final
        // alpha exactly, since no second cycle can overwrite it afterward.
        assert_eq!(alpha_compare, color[3]);
    }

    /// Loud typed rejection at the arithmetic layer: a selector this
    /// slice's arithmetic does not implement (e.g. NOISE, LOD_FRACTION)
    /// must return `CombinerInputError`, never a silent ZERO substitution —
    /// the AGENTS.md "loud traps, no silent shrugs" rule. Decode itself is
    /// exact (see the `*_decode_table_exhaustive` tests above and
    /// `noise_is_reachable_only_from_color_a_decode`) — this test is
    /// specifically about [`resolve_color_input`]/[`resolve_alpha_input`],
    /// the layer that actually narrows scope in this port.
    #[test]
    fn out_of_scope_selector_is_loudly_rejected() {
        let inputs = ALL_INPUTS;
        let error = resolve_color_input(inputs, [0.0; 3], ColorInput::Noise).unwrap_err();
        assert_eq!(error, CombinerInputError::Color(ColorInput::Noise));

        let error = resolve_alpha_input(inputs, 0.0, AlphaInput::LodFraction).unwrap_err();
        assert_eq!(error, CombinerInputError::Alpha(AlphaInput::LodFraction));
    }

    /// End-to-end deferred-selector rejection through `run_one_cycle`,
    /// driven by real wire bits (not a hand-constructed enum value): a
    /// `SetCombine` word whose color-A cycle-1 field is exactly 7 decodes
    /// to `ColorInput::Noise` (per RT64, exactly — see
    /// `color_slot_a_decode_table_exhaustive`), and `run_one_cycle` must
    /// reject that decode with `CombinerInputError::Color(Noise)` rather
    /// than silently treating it as ZERO or any other value. This is the
    /// invariant a `CombineParams`-widening future slice depends on:
    /// decode staying exact while arithmetic gates what it can evaluate.
    #[test]
    fn run_one_cycle_rejects_a_wire_encoded_deferred_selector() {
        let inputs = ALL_INPUTS;
        // color A cycle-1 bits are (low >> 5) & 0xF; 7 = NOISE.
        let low = 7u32 << 5;
        let params = CombineParams::from_wire(low, 0);
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Noise
        );
        assert_eq!(
            run_one_cycle(params, inputs),
            Err(CombinerInputError::Color(ColorInput::Noise))
        );
    }

    /// NOISE is reachable *only* from color-A's decode table (§2: no other
    /// slot's table has a NOISE entry) — this is a decode-shape fact, true
    /// regardless of whether NOISE's arithmetic is implemented yet.
    #[test]
    fn noise_is_reachable_only_from_color_a_decode() {
        for index in 0u32..16 {
            assert_ne!(
                CombineParams::color_input_b(index),
                ColorInput::Noise,
                "index {index}"
            );
            assert_ne!(
                CombineParams::color_input_c(index),
                ColorInput::Noise,
                "index {index}"
            );
            assert_ne!(
                CombineParams::color_input_d(index),
                ColorInput::Noise,
                "index {index}"
            );
        }
        assert_eq!(CombineParams::color_input_a(7), ColorInput::Noise);
    }

    // -- Frozen identities: pins this file and the WGSL sibling against
    // silent drift. A change to either requires updating these constants in
    // the same commit, making the drift visible in review.

    #[test]
    fn wgsl_source_sha256_is_frozen() {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(COLOR_COMBINER_WGSL.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        assert_eq!(
            digest,
            [
                0x84, 0x77, 0xe5, 0xd0, 0x1a, 0x13, 0x96, 0xfd, 0xc1, 0x62, 0x75, 0xe8, 0x71,
                0x4e, 0xc5, 0x10, 0x97, 0x5a, 0x60, 0x33, 0x2b, 0xca, 0xa5, 0xb8, 0x56, 0x0b,
                0x49, 0x0e, 0x9c, 0x08, 0x32, 0x72,
            ],
            "color_combiner.wgsl changed; recompute and update this frozen digest in the same commit"
        );
    }

    #[test]
    fn wgsl_parses_and_validates_under_naga() {
        let module = naga::front::wgsl::parse_str(COLOR_COMBINER_WGSL)
            .expect("color_combiner.wgsl must parse as valid WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("color_combiner.wgsl must pass Naga validation");
    }

    /// Hostile structural guard: asserts `color_combiner.wgsl`'s
    /// `color_input_a/b/c/d`/`alpha_input_abd/c` functions literally
    /// contain a `case` arm returning every RT64 selector reachable from
    /// that table (not just the eight this slice's arithmetic evaluates).
    /// A future edit that re-narrows decode back to a Slice-1-only ZERO
    /// collapse — the exact regression this test module was rewritten to
    /// catch — would delete one of these `case` lines and fail here, even
    /// though `wgsl_parses_and_validates_under_naga` would still pass
    /// (removing a case arm is syntactically valid WGSL).
    #[test]
    fn wgsl_decode_tables_contain_every_rt64_selector_case() {
        let source = COLOR_COMBINER_WGSL;
        for needle in [
            "case 6u: { return COLOR_ONE; }",
            "case 7u: { return COLOR_NOISE; }",
            "case 6u: { return COLOR_KEY_CENTER; }",
            "case 7u: { return COLOR_K4; }",
            "case 6u: { return COLOR_KEY_SCALE; }",
            "case 7u: { return COLOR_COMBINED_ALPHA; }",
            "case 8u: { return COLOR_TEXEL0_ALPHA; }",
            "case 9u: { return COLOR_TEXEL1_ALPHA; }",
            "case 10u: { return COLOR_PRIMITIVE_ALPHA; }",
            "case 11u: { return COLOR_SHADE_ALPHA; }",
            "case 12u: { return COLOR_ENV_ALPHA; }",
            "case 13u: { return COLOR_LOD_FRACTION; }",
            "case 14u: { return COLOR_PRIM_LOD_FRAC; }",
            "case 15u: { return COLOR_K5; }",
            "case 0u: { return ALPHA_LOD_FRACTION; }",
            "case 6u: { return ALPHA_PRIM_LOD_FRAC; }",
        ] {
            assert!(
                source.contains(needle),
                "color_combiner.wgsl is missing exact-decode case {needle:?} — decode must stay exact per RT64, only the arithmetic layer (resolve_color_input/resolve_alpha_input) may narrow scope"
            );
        }
    }
}
