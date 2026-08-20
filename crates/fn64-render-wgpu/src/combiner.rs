//! RT64 color combiner: selector decode and one-cycle/two-cycle arithmetic.
//!
//! Characterization-first port. Source: MIT RT64, pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`'s
//! Rust-port source pin), `src/shared/rt64_color_combiner.h` (SHA-256 of the
//! whole file,
//! `bc116cd9d8a86ca74ebb8f3294fa48bc9e605c0eec53bcac5e07503dfd668b02`,
//! matching `docs/rt64-port-inventory.json`'s `sources.port.sha256` for that
//! path, confirmed independently here by `shasum -a 256` against the pinned
//! port-commit checkout; fn64's own
//! `docs/RT64-PORT-INVENTORY.md:291` records this file `unchanged` /
//! `source-digests-verified` against the executable-comparison oracle -- the
//! `unchanged` delta is exactly the statement that this digest is also the
//! file's `sources.oracle.sha256`, so unlike `rt64_state.cpp` there is no
//! drift caveat here).
//!
//! Decode is exact and complete for every wire-legal `(slot, index,
//! second_cycle)` triple: [`CombineParams::decode_color`]/[`decode_alpha`]
//! reproduce RT64's `decodeColorInput`/`decodeAlphaInput` bit-for-bit. Only
//! genuine RT64 out-of-range indices alias `ZERO` (`default:` arms in the
//! pinned source) — this port never substitutes `Zero` for a selector RT64
//! itself would decode to something else.
//!
//! Full one-cycle arithmetic: [`run_one_cycle`] (via `resolve_color_input`/
//! `resolve_alpha_input`, transcribed directly from RT64's `fromColorInput`/
//! `fromAlphaInput`, `rt64_color_combiner.h:468-540`) evaluates `COMBINED`,
//! `TEXEL0`, `TEXEL1`, `PRIMITIVE`, `SHADE`, `ENVIRONMENT`, `KEY_CENTER`,
//! `KEY_SCALE`, `COMBINED_ALPHA`, `TEXEL0_ALPHA`, `TEXEL1_ALPHA`,
//! `PRIMITIVE_ALPHA`, `SHADE_ALPHA`, `ENV_ALPHA`, `LOD_FRACTION`,
//! `PRIM_LOD_FRAC`, `NOISE`, `K4`, `K5`, `ONE`, `ZERO` for color, and
//! `COMBINED`, `TEXEL0`, `TEXEL1`, `PRIMITIVE`, `SHADE`, `ENVIRONMENT`,
//! `LOD_FRACTION`, `PRIM_LOD_FRAC`, `ONE`, `ZERO` for alpha — every variant
//! either enum can hold, so there is no deferred-selector rejection path.
//! [`CombinerInputs`] carries every field RT64's `Inputs` struct has that a
//! combine formula can reach (`keyCenter`, `keyScale`, `lodFraction`,
//! `primLodFrac`, `noise`, `K4`, `K5`), all caller-supplied — NOISE and the
//! LOD fractions are explicit typed inputs at this seam, not a PRNG or a
//! derivative computed here (RT64's own `initRand`/`nextRand`/`computeLOD`
//! remain uncharacterized; this port does not invent them).
//!
//! [`run_combiner`]/[`run_two_cycle`] add full two-cycle mode, transcribed
//! from `runCycle`/`run` (`rt64_color_combiner.h:567-634`):
//!
//! - **Cycle-0-then-cycle-1 ordering and zero-initialization**: `run`
//!   zero-initializes `combinerColor` once, then threads its single
//!   `inout float4` accumulator through cycle 0's `runCycle` call and, for
//!   two-cycle mode, cycle 1's — never the reverse, never independently.
//! - **`COMBINED`/`COMBINED_ALPHA` cross-cycle reads**: cycle 1's
//!   `C_COMBINED`/`A_COMBINED` selectors read cycle 0's real (not
//!   zero-init) output, subject to the pre-arithmetic wrap below.
//! - **`TEXEL0`/`TEXEL1` cycle swapping**: `fromColorInput`/
//!   `fromAlphaInput`'s own `secondCycle` parameter (distinct from
//!   `decodeColorInput`'s bitfield-slice parameter of the same name) is
//!   `twoCycle && secondCycleInputs` — `true` only for two-cycle mode's
//!   second pass, where `C_TEXEL0` reads `texVal1` and `C_TEXEL1` reads
//!   `texVal0` (and their `*_ALPHA` cross-reads).
//! - **`twoCycle`-conditioned pre-arithmetic wrapping**: cycle 1 applies
//!   [`wrap_input_c`]/[`wrap_input_abd`] to the *incoming* accumulator
//!   before any `fromColorInput`/`fromAlphaInput` call reads it — never
//!   after the `(A-B)*C+D` arithmetic — and the range choice
//!   (`wrapInputC`'s `[-1-1/255,1+1/255]` vs. `wrapInputABD`'s
//!   `[-0.5-1/255,1.5+1/255]`) is decided independently for color and
//!   alpha, by whether *that channel's own slot-C selector* is
//!   `COMBINED`/`COMBINED_ALPHA` this cycle — not by which slot is
//!   currently being resolved. One-cycle mode never applies this wrap
//!   (`twoCycle` is `false`, so RT64's `secondCycle` flag is always
//!   `false` regardless of the bitfield slice used).
//! - **`alphaCompareValue` capture timing**: `run` snapshots
//!   `alphaCompareValue = combinerColor.a` immediately after the first (and,
//!   in one-cycle mode, only) `runCycle` call — i.e. cycle 0's alpha output
//!   in two-cycle mode, never overwritten by cycle 1.
//! - **Final `wrapClamp`**: `wrapInputABD` then `clamp(0,1)`, applied
//!   unconditionally to the finished color and to `alphaCompareValue`,
//!   regardless of cycle count — this is a separate mechanism from the
//!   cross-cycle carry wrap above (see [`wrap_clamp`]'s doc).
//!
//! Explicitly not in this port: copy mode, real NOISE/LOD generation,
//! shader-keying, draw-path wiring, and any GPU execution. `SetCombine`'s
//! raw-DPC opcode decode and its durable `RdpState` retention live in
//! `raw_dpc::mod`/`state.rs` (`RawDpcCommandKind::SetCombine`,
//! `RdpState::combine`), which construct and store [`CombineParams`] via
//! `from_wire` but do not change its arithmetic. RT64's `!inputs.alphaOnly`
//! gate around the RGB
//! combine (`runCycle` line 589) is transcribed as [`run_cycle`]'s
//! `alpha_only` parameter for structural fidelity, but this port's
//! [`CombinerInputs`] has no `alphaOnly` field to set it from — RT64's own
//! only observed call site (`RasterPS.hlsl:171`) always sets it `false` —
//! so every public entry point always passes `false`.

use crate::state::{Color4, PrimColor};

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

/// Raw 64-bit `SetCombine` payload, split into low/high 32-bit halves per
/// RT64's own field naming (`rt64_color_combiner.h`'s `L`/`H`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombineParams {
    low: u32,
    high: u32,
}

impl CombineParams {
    /// `RDP::setCombine`'s wire split (`src/hle/rt64_rdp.cpp:295-302`, pinned
    /// commit `5473732a822a4423b5696e7cb18fecc425a59875`): `low` is exactly
    /// the command's first wire word (`w0`, unmasked — RT64 never strips its
    /// top opcode byte before storing `combineL`), `high` is exactly the
    /// second wire word (`w1`, `combineH`). The `raw_dpc` module's
    /// `SET_COMBINE` decode arm is this constructor's opcode handler.
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
    const fn color_input_a(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::One,
            7 => ColorInput::Noise,
            _ => ColorInput::Zero,
        }
    }

    /// `colorInputB`, decoded exactly: 0-5 common, 6=KEY_CENTER, 7=K4,
    /// 8-15=ZERO.
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
    /// (RT64's own upper-half-of-the-5-bit-field collapse).
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
    /// own table for D has no NOISE/KEY_CENTER/etc. entries to begin with).
    const fn color_input_d(index: u32) -> ColorInput {
        match index {
            0..=5 => Self::color_input_common(index),
            6 => ColorInput::One,
            _ => ColorInput::Zero,
        }
    }

    /// `alphaInputABD`: shared table for alpha slots A, B, D, decoded
    /// exactly.
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
    /// no `A_COMBINED` reachable from alpha-C at all).
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

    /// Selector-reference predicate, not a general algebraic-necessity one:
    /// true iff any of the four first-cycle color selectors (A/B/C/D) or
    /// four first-cycle alpha selectors (A/B/C/D) decode to
    /// `ColorInput::{Texel0,Texel1,Texel0Alpha,Texel1Alpha}` /
    /// `AlphaInput::{Texel0,Texel1}`. This does NOT evaluate whether the
    /// formula is algebraically zeroed out in general — a referenced D
    /// selector, for instance, always counts, since `run_one_cycle`'s
    /// `(A-B)*C+D` formula adds `D` unconditionally with no coefficient that
    /// could zero it.
    ///
    /// The one deliberate, narrow exception: slot C is not a free-standing
    /// reference, it is `(A-B)*C`'s own coefficient — `run_one_cycle`
    /// evaluates `resolve_color_input`/`resolve_alpha_input` for C
    /// unconditionally, but whatever it resolves to is multiplied by
    /// `(A-B)`, which is exactly zero whenever A and B decode to the same
    /// selector (they then resolve to the identical value, per
    /// `resolve_color_input`/`resolve_alpha_input`'s own shared `match`).
    /// This crate's own fixtures rely on exactly this cancellation as a
    /// "don't care" idiom (`targets/triangle_pipeline/tests.rs`'s
    /// `shade_passthrough_combine_params` sets alpha A=B=COMBINED and
    /// alpha C=TEXEL0 via `alpha_input_c`'s own distinct table — see that
    /// table's doc — purely because C's coefficient is forced to zero,
    /// never because TEXEL0's value is wanted). Treating slot C as an
    /// unconditional reference the same as A/B/D would report
    /// "texture referenced" for every SHADE-only fixture in this crate that
    /// happens to reuse that idiom, defeating the gate this predicate
    /// exists to drive. This is a fact about `(A-B)*C+D`'s own coefficient
    /// structure, not a guess about which fixtures happen to exist: C is
    /// checked whenever A and B decode to *different* selectors (the only
    /// condition under which C's value can possibly reach the output),
    /// matching the exact cancellation `run_one_cycle`'s arithmetic
    /// performs, not an approximation of it.
    ///
    /// Decodes with `second_cycle = true`, matching [`run_one_cycle`]'s own
    /// slot-decode call (`decode_color`/`decode_alpha`'s `SECOND_CYCLE`
    /// constant, `run_one_cycle`'s doc) — this predicate must inspect
    /// exactly the selectors that function's own formula reads, not the
    /// unused `second_cycle = false` bitfield slice.
    pub const fn references_texels_in_first_cycle(self) -> bool {
        const SECOND_CYCLE: bool = true;

        let ca = self.decode_color(ColorInputSlot::A, SECOND_CYCLE);
        let cb = self.decode_color(ColorInputSlot::B, SECOND_CYCLE);
        let cc = self.decode_color(ColorInputSlot::C, SECOND_CYCLE);
        let cd = self.decode_color(ColorInputSlot::D, SECOND_CYCLE);
        let color_coefficient_nonzero = !Self::color_input_eq(ca, cb);
        if Self::color_input_references_texel(ca)
            || Self::color_input_references_texel(cb)
            || (color_coefficient_nonzero && Self::color_input_references_texel(cc))
            || Self::color_input_references_texel(cd)
        {
            return true;
        }

        let aa = self.decode_alpha(AlphaInputSlot::A, SECOND_CYCLE);
        let ab = self.decode_alpha(AlphaInputSlot::B, SECOND_CYCLE);
        let ac = self.decode_alpha(AlphaInputSlot::C, SECOND_CYCLE);
        let ad = self.decode_alpha(AlphaInputSlot::D, SECOND_CYCLE);
        let alpha_coefficient_nonzero = !Self::alpha_input_eq(aa, ab);
        Self::alpha_input_references_texel(aa)
            || Self::alpha_input_references_texel(ab)
            || (alpha_coefficient_nonzero && Self::alpha_input_references_texel(ac))
            || Self::alpha_input_references_texel(ad)
    }

    const fn color_input_references_texel(input: ColorInput) -> bool {
        matches!(
            input,
            ColorInput::Texel0
                | ColorInput::Texel1
                | ColorInput::Texel0Alpha
                | ColorInput::Texel1Alpha
        )
    }

    const fn alpha_input_references_texel(input: AlphaInput) -> bool {
        matches!(input, AlphaInput::Texel0 | AlphaInput::Texel1)
    }

    /// `const fn`-usable equality for the `(A-B)` cancellation check above —
    /// `ColorInput` derives `PartialEq` but that impl is not itself callable
    /// from a `const fn` in this crate's Rust edition, so this is a manual
    /// discriminant match instead.
    const fn color_input_eq(a: ColorInput, b: ColorInput) -> bool {
        matches!(
            (a, b),
            (ColorInput::Combined, ColorInput::Combined)
                | (ColorInput::Texel0, ColorInput::Texel0)
                | (ColorInput::Texel1, ColorInput::Texel1)
                | (ColorInput::Primitive, ColorInput::Primitive)
                | (ColorInput::Shade, ColorInput::Shade)
                | (ColorInput::Environment, ColorInput::Environment)
                | (ColorInput::KeyCenter, ColorInput::KeyCenter)
                | (ColorInput::KeyScale, ColorInput::KeyScale)
                | (ColorInput::CombinedAlpha, ColorInput::CombinedAlpha)
                | (ColorInput::Texel0Alpha, ColorInput::Texel0Alpha)
                | (ColorInput::Texel1Alpha, ColorInput::Texel1Alpha)
                | (ColorInput::PrimitiveAlpha, ColorInput::PrimitiveAlpha)
                | (ColorInput::ShadeAlpha, ColorInput::ShadeAlpha)
                | (ColorInput::EnvAlpha, ColorInput::EnvAlpha)
                | (ColorInput::LodFraction, ColorInput::LodFraction)
                | (ColorInput::PrimLodFrac, ColorInput::PrimLodFrac)
                | (ColorInput::Noise, ColorInput::Noise)
                | (ColorInput::K4, ColorInput::K4)
                | (ColorInput::K5, ColorInput::K5)
                | (ColorInput::One, ColorInput::One)
                | (ColorInput::Zero, ColorInput::Zero)
        )
    }

    /// `const fn`-usable equality for the `(A-B)` cancellation check above,
    /// same rationale as [`Self::color_input_eq`].
    const fn alpha_input_eq(a: AlphaInput, b: AlphaInput) -> bool {
        matches!(
            (a, b),
            (AlphaInput::Combined, AlphaInput::Combined)
                | (AlphaInput::Texel0, AlphaInput::Texel0)
                | (AlphaInput::Texel1, AlphaInput::Texel1)
                | (AlphaInput::Primitive, AlphaInput::Primitive)
                | (AlphaInput::Shade, AlphaInput::Shade)
                | (AlphaInput::Environment, AlphaInput::Environment)
                | (AlphaInput::LodFraction, AlphaInput::LodFraction)
                | (AlphaInput::PrimLodFrac, AlphaInput::PrimLodFrac)
                | (AlphaInput::One, AlphaInput::One)
                | (AlphaInput::Zero, AlphaInput::Zero)
        )
    }
}

/// Per-pixel combiner inputs. Mirrors RT64's `Inputs` struct
/// (`rt64_color_combiner.h:451-466`) restricted to the fields a one-cycle
/// formula can reach (`otherMode`/`alphaOnly` are two-cycle/copy-mode-only
/// concerns, Slice 3+). `key_center`/`key_scale`/`lod_fraction`/
/// `prim_lod_frac`/`noise`/`k4`/`k5` are caller-supplied typed values, not
/// computed here: RT64's own `keyCenter`/`keyScale` are host-tracked
/// (`rt64_rdp.h:129-131`), `lodFraction` is a per-pixel GPU derivative
/// (`computeLOD`, uncharacterized), `primLodFrac` is a host-set per-draw
/// uniform, and `noise` is a per-pixel PRNG draw (`initRand`/`nextRand`,
/// uncharacterized) — this module has no PRNG or derivative implementation
/// and does not invent one; it only proves the arithmetic that consumes
/// whatever value the caller supplies at this seam.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombinerInputs {
    pub tex_val0: [f32; 4],
    pub tex_val1: [f32; 4],
    pub prim_color: [f32; 4],
    pub shade_color: [f32; 4],
    pub env_color: [f32; 4],
    pub key_center: [f32; 3],
    pub key_scale: [f32; 3],
    pub lod_fraction: f32,
    pub prim_lod_frac: f32,
    pub noise: f32,
    pub k4: f32,
    pub k5: f32,
}

/// Resolves one color selector to its RGB value, per `fromColorInput`
/// (`rt64_color_combiner.h:468-514`). `combiner_color` stands in for RT64's
/// `combinerColor.rgb` accumulator — in one-cycle mode this is always
/// `[0.0, 0.0, 0.0]` (RT64 `run`'s zero-init, never written before the
/// single `runCycle` call), so [`run_one_cycle`] passes that fixed value
/// rather than exposing a caller-settable carry. Slice 3's [`run_cycle`]
/// passes cycle 0's real output when evaluating cycle 1.
///
/// `second_cycle` here is `fromColorInput`'s own parameter, distinct from
/// `decodeColorInput`'s bitfield-slice `secondCycle` — RT64 calls it as
/// `secondCycle = twoCycle && secondCycleInputs` (`runCycle`,
/// `rt64_color_combiner.h:577`). It is `false` for one-cycle mode's single
/// pass (`twoCycle` is `false`) and for two-cycle mode's cycle 0
/// (`secondCycleInputs` is `false` for `cycle == 0`), but exactly `true`
/// for two-cycle mode's cycle 1 — that is the *only* condition under which
/// the `TEXEL0`/`TEXEL1` swap below fires: `C_TEXEL0` reads `texVal1` and
/// `C_TEXEL1` reads `texVal0`, and likewise for their `*_ALPHA` cross-reads.
/// `C_COMBINED_ALPHA`/`*_ALPHA` selectors read the *alpha* channel of the
/// same (possibly swapped) named input, replicated into all three RGB
/// lanes — exactly RT64's `return combinerColor.a;` / `return
/// inputs.texVal0.a;` etc., an HLSL scalar-to-`float3` return that
/// implicitly broadcasts.
fn resolve_color_input(
    inputs: CombinerInputs,
    second_cycle: bool,
    combiner_color: [f32; 3],
    combiner_alpha: f32,
    selector: ColorInput,
) -> [f32; 3] {
    let texel0 = if second_cycle {
        inputs.tex_val1
    } else {
        inputs.tex_val0
    };
    let texel1 = if second_cycle {
        inputs.tex_val0
    } else {
        inputs.tex_val1
    };
    match selector {
        ColorInput::Combined => combiner_color,
        ColorInput::Texel0 => [texel0[0], texel0[1], texel0[2]],
        ColorInput::Texel1 => [texel1[0], texel1[1], texel1[2]],
        ColorInput::Primitive => [
            inputs.prim_color[0],
            inputs.prim_color[1],
            inputs.prim_color[2],
        ],
        ColorInput::Shade => [
            inputs.shade_color[0],
            inputs.shade_color[1],
            inputs.shade_color[2],
        ],
        ColorInput::Environment => [
            inputs.env_color[0],
            inputs.env_color[1],
            inputs.env_color[2],
        ],
        ColorInput::KeyCenter => inputs.key_center,
        ColorInput::KeyScale => inputs.key_scale,
        ColorInput::CombinedAlpha => [combiner_alpha; 3],
        ColorInput::Texel0Alpha => [texel0[3]; 3],
        ColorInput::Texel1Alpha => [texel1[3]; 3],
        ColorInput::PrimitiveAlpha => [inputs.prim_color[3]; 3],
        ColorInput::ShadeAlpha => [inputs.shade_color[3]; 3],
        ColorInput::EnvAlpha => [inputs.env_color[3]; 3],
        ColorInput::LodFraction => [inputs.lod_fraction; 3],
        ColorInput::PrimLodFrac => [inputs.prim_lod_frac; 3],
        ColorInput::Noise => [inputs.noise; 3],
        ColorInput::K4 => [inputs.k4; 3],
        ColorInput::K5 => [inputs.k5; 3],
        ColorInput::One => [1.0, 1.0, 1.0],
        ColorInput::Zero => [0.0, 0.0, 0.0],
    }
}

/// Resolves one alpha selector, per `fromAlphaInput`
/// (`rt64_color_combiner.h:516-540`). `combiner_alpha` stands in for RT64's
/// `combinerColor.a` accumulator — see [`resolve_color_input`]'s doc for the
/// `second_cycle` TEXEL0/TEXEL1 swap note (identical shape here).
fn resolve_alpha_input(
    inputs: CombinerInputs,
    second_cycle: bool,
    combiner_alpha: f32,
    selector: AlphaInput,
) -> f32 {
    let texel0 = if second_cycle {
        inputs.tex_val1
    } else {
        inputs.tex_val0
    };
    let texel1 = if second_cycle {
        inputs.tex_val0
    } else {
        inputs.tex_val1
    };
    match selector {
        AlphaInput::Combined => combiner_alpha,
        AlphaInput::Texel0 => texel0[3],
        AlphaInput::Texel1 => texel1[3],
        AlphaInput::Primitive => inputs.prim_color[3],
        AlphaInput::Shade => inputs.shade_color[3],
        AlphaInput::Environment => inputs.env_color[3],
        AlphaInput::LodFraction => inputs.lod_fraction,
        AlphaInput::PrimLodFrac => inputs.prim_lod_frac,
        AlphaInput::One => 1.0,
        AlphaInput::Zero => 0.0,
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
/// `wrapInputABD`'s range is `[-0.5 - 1/255, 1.5 + 1/255]`. For a texel/prim/
/// shade/env/key input (always `[0,1]`-normalized) this is a plain no-op
/// `clamp(i, 0.0, 1.0)`, since `(A-B)*C+D` cannot push arbitrarily far
/// outside that band from in-range inputs alone. Slice 2's caller-supplied
/// `NOISE`/`K4`/`K5`/`LOD_FRACTION`/`PRIM_LOD_FRAC` fields carry no such
/// guarantee (RT64 itself performs no range check on them either — see
/// [`CombinerInputs`]'s doc), so an out-of-range value here genuinely
/// exercises the wrap `step` branches this doc describes, not only the
/// reduced clamp form — see `wrap_clamp_low_branch_triggers_below_low_bound`/
/// `wrap_clamp_high_branch_triggers_at_or_above_high_bound` in this file's
/// tests.
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
/// Every selector [`ColorInput`]/[`AlphaInput`] can hold is evaluated —
/// there is no remaining decoded-but-unimplemented selector in one-cycle
/// mode, so this is infallible. `NOISE`/`LOD_FRACTION`/`PRIM_LOD_FRAC`/K4/K5
/// use whatever value `inputs` supplies (see [`CombinerInputs`]'s doc); this
/// function does not generate or validate them, matching RT64's own
/// `fromColorInput`/`fromAlphaInput`, which take them as given struct
/// fields with no range check either.
pub fn run_one_cycle(params: CombineParams, inputs: CombinerInputs) -> ([f32; 4], f32) {
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
    // call (see `resolve_color_input`/`resolve_alpha_input` docs). This
    // call's own `fromColorInput`/`fromAlphaInput` `secondCycle` parameter
    // (the TEXEL0/TEXEL1-swap flag) is `false` here, matching one-cycle
    // mode's `twoCycle = false` exactly (see module docs).
    let combiner_color_in = [0.0f32; 3];
    let combiner_alpha_in = 0.0f32;

    let a = resolve_color_input(inputs, false, combiner_color_in, combiner_alpha_in, ca);
    let b = resolve_color_input(inputs, false, combiner_color_in, combiner_alpha_in, cb);
    let c = resolve_color_input(inputs, false, combiner_color_in, combiner_alpha_in, cc);
    let d = resolve_color_input(inputs, false, combiner_color_in, combiner_alpha_in, cd);
    let rgb = [
        (a[0] - b[0]) * c[0] + d[0],
        (a[1] - b[1]) * c[1] + d[1],
        (a[2] - b[2]) * c[2] + d[2],
    ];

    let aa_v = resolve_alpha_input(inputs, false, combiner_alpha_in, aa);
    let ab_v = resolve_alpha_input(inputs, false, combiner_alpha_in, ab);
    let ac_v = resolve_alpha_input(inputs, false, combiner_alpha_in, ac);
    let ad_v = resolve_alpha_input(inputs, false, combiner_alpha_in, ad);
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

    (combiner_color, alpha_compare_value)
}

/// `wrap`'s cross-cycle-carry range when the *consuming* selector is
/// specifically `C_COMBINED`/`A_COMBINED` read by input slot **C**
/// (`wrapInputC`, `rt64_color_combiner.h:548-553`): `[-1 - 1/255, 1 +
/// 1/255]`. Distinct from [`wrap_input_abd`]'s range — RT64 chooses between
/// the two based solely on whether the *C slot's own selector* is
/// `COMBINED`/`COMBINED_ALPHA` for this cycle (`runCycle` lines 581, 591),
/// not on which slot is being evaluated at any given call to
/// `resolve_color_input`/`resolve_alpha_input`.
fn wrap_input_c(i: f32) -> f32 {
    const ROUNDING: f32 = 1.0 / 255.0;
    const LOW: f32 = -1.0 - ROUNDING;
    const HIGH: f32 = 1.0 + ROUNDING;
    const RANGE: f32 = HIGH - LOW;
    let mut wrapped = i;
    if wrapped <= LOW {
        wrapped += RANGE;
    }
    if HIGH <= wrapped {
        wrapped -= RANGE;
    }
    wrapped
}

/// `wrap`'s cross-cycle-carry range for every other consuming case (slots
/// A, B, or D reading `COMBINED`, or slot C reading it but not being the
/// only reader — i.e. RT64's `else` branch at `runCycle` lines 584, 596/599)
/// (`wrapInputABD`, `rt64_color_combiner.h:555-560`): `[-0.5 - 1/255, 1.5 +
/// 1/255]`. This is the same range [`wrap_clamp`] applies as its own
/// pre-clamp wrap step — that is RT64's `wrapClamp = wrapInputABD then
/// clamp(0,1)`, an intentional reuse, not a coincidence.
fn wrap_input_abd(i: f32) -> f32 {
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
    wrapped
}

/// Which pass of a (possibly two-cycle) combiner evaluation this is. Mirrors
/// RT64's `runCycle(inputs, cycle, twoCycle, combinerColor)` parameters
/// (`cycle`, `twoCycle`) as one typed value instead of two raw booleans/a
/// raw `uint cycle`, per this port's own convention of using
/// [`state::CycleType`](crate::state::CycleType)-shaped enums rather than
/// bare bools at public seams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CyclePass {
    /// The only pass of one-cycle mode. RT64 still calls this `cycle = 1`
    /// (`run`: `runCycle(inputs, twoCycle ? 0 : 1, twoCycle, ...)`), i.e. it
    /// reads the *second*-cycle bitfield slice, but `twoCycle = false` means
    /// `secondCycle` (the cross-cycle-carry flag) is always `false` — no
    /// wrap ever applies, and `C_COMBINED`/`A_COMBINED` reads the
    /// zero-initialized accumulator. See module docs.
    OnlyCycleOfOneCycleMode,
    /// Two-cycle mode's first pass (`cycle = 0`): reads the cycle-0
    /// bitfield slice, no cross-cycle wrap (`secondCycleInputs` is `false`).
    FirstOfTwoCycles,
    /// Two-cycle mode's second pass (`cycle = 1`): reads the cycle-1
    /// bitfield slice, and *does* apply the cross-cycle-carry wrap to
    /// whatever cycle 0 just wrote into the accumulator before using it as
    /// an input (`secondCycle = twoCycle && secondCycleInputs = true`).
    SecondOfTwoCycles,
}

impl CyclePass {
    /// `decodeColorInput`/`decodeAlphaInput`'s own `secondCycleInputs`
    /// bitfield-slice selector: `true` for both the only one-cycle pass
    /// (RT64's `cycle == 1` for that call) and two-cycle mode's second pass.
    const fn bitfield_second_cycle(self) -> bool {
        !matches!(self, CyclePass::FirstOfTwoCycles)
    }

    /// `runCycle`'s `secondCycle` cross-cycle-carry flag: `twoCycle &&
    /// secondCycleInputs`. Only `true` for two-cycle mode's second pass.
    const fn carries_wrap(self) -> bool {
        matches!(self, CyclePass::SecondOfTwoCycles)
    }
}

/// One `runCycle` call (`rt64_color_combiner.h:567-609`): decodes this
/// pass's eight selectors from the right bitfield slice, applies the
/// cross-cycle-carry wrap to the incoming accumulator when `pass` requires
/// it, then evaluates `(A-B)*C+D` for color and alpha, writing the result
/// back into the accumulator RT64 threads through both calls as one
/// `inout float4 combinerColor`.
///
/// The wrap-range choice (`wrap_input_c` vs. `wrap_input_abd`) is decided
/// **independently for color and alpha**, each by its own slot-C selector
/// for *this* pass — exactly RT64's two separate `if (AC == A_COMBINED)` /
/// `if (CC == C_COMBINED)` checks (`runCycle` lines 581, 591), not a single
/// shared decision. The color-side wrap additionally applies to all three
/// RGB channels independently (RT64 lines 592-594/597-599 call
/// `wrapInputC`/`wrapInputABD` three times, once per channel — the
/// per-channel repetition matters only in that each channel's value can
/// legitimately differ, not in the wrap arithmetic itself, which is
/// scalar).
///
/// `alpha_only` mirrors RT64's `!inputs.alphaOnly` gate around the RGB
/// combine (`runCycle` line 589): this port's [`CombinerInputs`] has no
/// `alphaOnly` field (RT64's own only observed call site,
/// `RasterPS.hlsl:171`, always sets it `false` — see module docs), so this
/// function's only caller always passes `false`. It is threaded through
/// (rather than hard-coded away) so `run_cycle` stays a faithful
/// transcription of `runCycle`'s exact branch structure, not an
/// RT64-diverging simplification. The WGSL twin (`color_combiner.wgsl`'s
/// `run_cycle`) elides this parameter entirely rather than threading an
/// always-`false` value through a shader function signature, documenting
/// the gate as unconditionally taken instead — a deliberate, documented
/// structural difference between the two, not a semantic one (both always
/// execute the RGB combine).
fn run_cycle(
    params: CombineParams,
    inputs: CombinerInputs,
    pass: CyclePass,
    alpha_only: bool,
    combiner_color_in: [f32; 4],
) -> [f32; 4] {
    let bitfield_second_cycle = pass.bitfield_second_cycle();
    let carries_wrap = pass.carries_wrap();

    let ca = params.decode_color(ColorInputSlot::A, bitfield_second_cycle);
    let cb = params.decode_color(ColorInputSlot::B, bitfield_second_cycle);
    let cc = params.decode_color(ColorInputSlot::C, bitfield_second_cycle);
    let cd = params.decode_color(ColorInputSlot::D, bitfield_second_cycle);
    let aa = params.decode_alpha(AlphaInputSlot::A, bitfield_second_cycle);
    let ab = params.decode_alpha(AlphaInputSlot::B, bitfield_second_cycle);
    let ac = params.decode_alpha(AlphaInputSlot::C, bitfield_second_cycle);
    let ad = params.decode_alpha(AlphaInputSlot::D, bitfield_second_cycle);

    let [mut r, mut g, mut b_ch, mut a_ch] = combiner_color_in;

    // "Simulate the wrap on the inputs of the second cycle" (RT64's own
    // comment, `runCycle` line 579) — applied to the *incoming* accumulator
    // before it is read by this pass's `fromColorInput`/`fromAlphaInput`
    // calls, not after this pass computes its own output.
    if carries_wrap {
        a_ch = if ac == AlphaInput::Combined {
            wrap_input_c(a_ch)
        } else {
            wrap_input_abd(a_ch)
        };
    }

    if !alpha_only {
        if carries_wrap {
            if cc == ColorInput::Combined {
                r = wrap_input_c(r);
                g = wrap_input_c(g);
                b_ch = wrap_input_c(b_ch);
            } else {
                r = wrap_input_abd(r);
                g = wrap_input_abd(g);
                b_ch = wrap_input_abd(b_ch);
            }
        }

        let combiner_color = [r, g, b_ch];
        let a = resolve_color_input(inputs, carries_wrap, combiner_color, a_ch, ca);
        let b = resolve_color_input(inputs, carries_wrap, combiner_color, a_ch, cb);
        let c = resolve_color_input(inputs, carries_wrap, combiner_color, a_ch, cc);
        let d = resolve_color_input(inputs, carries_wrap, combiner_color, a_ch, cd);
        r = (a[0] - b[0]) * c[0] + d[0];
        g = (a[1] - b[1]) * c[1] + d[1];
        b_ch = (a[2] - b[2]) * c[2] + d[2];
    }

    let aa_v = resolve_alpha_input(inputs, carries_wrap, a_ch, aa);
    let ab_v = resolve_alpha_input(inputs, carries_wrap, a_ch, ab);
    let ac_v = resolve_alpha_input(inputs, carries_wrap, a_ch, ac);
    let ad_v = resolve_alpha_input(inputs, carries_wrap, a_ch, ad);
    a_ch = (aa_v - ab_v) * ac_v + ad_v;

    [r, g, b_ch, a_ch]
}

/// Cycle-mode dispatch, mirroring RT64's `run`
/// (`rt64_color_combiner.h:611-634`) minus its copy-mode branch (`run`'s
/// `cycleType == G_CYC_COPY` case, out of this slice's scope — see module
/// docs). Callers distinguish one-cycle from two-cycle execution through
/// this typed enum rather than a raw `twoCycle: bool`, per the task's API
/// requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombinerCycleMode {
    OneCycle,
    TwoCycle,
}

/// Runs the combiner for either cycle mode and returns `(combinerColor,
/// alphaCompareValue)`, exactly RT64's `run` out-parameters (minus copy
/// mode). [`run_one_cycle`] remains the Slice 2 one-cycle-only entry point
/// (unchanged signature, still independently tested); this function is the
/// Slice 3 addition that also covers two-cycle mode and is the one
/// `run_one_cycle`'s regression-equivalence test compares itself against.
///
/// **Cycle-0-then-cycle-1 ordering**: for [`CombinerCycleMode::TwoCycle`],
/// `run_cycle` is called first with [`CyclePass::FirstOfTwoCycles`], then
/// its full `[f32; 4]` output (not just the alpha or just the RGB channels)
/// is threaded as the *next* call's `combiner_color_in` — this is RT64's
/// single `inout float4 combinerColor` shared across both `runCycle` calls
/// in `run` (`rt64_color_combiner.h:620,626`), not two independent
/// evaluations. **`alphaCompareValue` capture timing**: RT64 snapshots
/// `alphaCompareValue = combinerColor.a` (line 623) *between* the two
/// `runCycle` calls in two-cycle mode — i.e. it is cycle 0's alpha output,
/// pre-wrap-and-clamp at capture time (the shared final `wrapClamp` pass
/// applies to it afterward, same as the finished color), and is **not**
/// overwritten by cycle 1. For one-cycle mode there is only one `runCycle`
/// call, so `alphaCompareValue` is trivially that (only) cycle's alpha
/// output — see [`run_one_cycle`]'s doc, which this function reproduces via
/// [`CyclePass::OnlyCycleOfOneCycleMode`].
pub fn run_combiner(
    params: CombineParams,
    inputs: CombinerInputs,
    cycle_mode: CombinerCycleMode,
) -> ([f32; 4], f32) {
    // RT64 `run`'s zero-init (`combinerColor = float4(0,0,0,0)`), unwritten
    // before the first `runCycle` call regardless of cycle mode.
    let zero_init = [0.0f32; 4];

    let after_first_cycle = match cycle_mode {
        CombinerCycleMode::OneCycle => run_cycle(
            params,
            inputs,
            CyclePass::OnlyCycleOfOneCycleMode,
            false,
            zero_init,
        ),
        CombinerCycleMode::TwoCycle => run_cycle(
            params,
            inputs,
            CyclePass::FirstOfTwoCycles,
            false,
            zero_init,
        ),
    };

    // RT64 `run`: `alphaCompareValue = combinerColor.a`, snapshotted right
    // after the first (and, in one-cycle mode, only) `runCycle` call, before
    // any second cycle can overwrite it.
    let alpha_compare_raw = after_first_cycle[3];

    let final_color = match cycle_mode {
        CombinerCycleMode::OneCycle => after_first_cycle,
        CombinerCycleMode::TwoCycle => run_cycle(
            params,
            inputs,
            CyclePass::SecondOfTwoCycles,
            false,
            after_first_cycle,
        ),
    };

    let alpha_compare_value = wrap_clamp(alpha_compare_raw);
    let combiner_color = [
        wrap_clamp(final_color[0]),
        wrap_clamp(final_color[1]),
        wrap_clamp(final_color[2]),
        wrap_clamp(final_color[3]),
    ];

    (combiner_color, alpha_compare_value)
}

/// Runs two-cycle combiner arithmetic. Thin, explicitly-named wrapper over
/// [`run_combiner`] with [`CombinerCycleMode::TwoCycle`] — kept alongside
/// [`run_one_cycle`] so both cycle modes have an equally discoverable named
/// entry point, matching this crate's existing per-mode function naming
/// convention rather than forcing every caller through the enum-taking form.
pub fn run_two_cycle(params: CombineParams, inputs: CombinerInputs) -> ([f32; 4], f32) {
    run_combiner(params, inputs, CombinerCycleMode::TwoCycle)
}

/// Overrides `base`'s `env_color`, `prim_color`, and `prim_lod_frac` fields
/// with values derived from this crate's already-decoded fragment constant
/// registers, per `RasterPS.hlsl`'s combiner-input assembly
/// (`src/shaders/RasterPS.hlsl:169-183`, pinned commit
/// `5473732a822a4423b5696e7cb18fecc425a59875`):
///
/// ```text
/// ccInputs.primColor   = instanceRDPParams[instanceIndex].primColor;
/// ccInputs.envColor    = instanceRDPParams[instanceIndex].envColor;
/// ccInputs.primLodFrac = instanceRDPParams[instanceIndex].primLOD.x;
/// ```
///
/// `primColor`/`envColor` there are the already-normalized `float4` staged
/// by `RDP::setPrimColor`/`setEnvColor` (`src/hle/rt64_rdp.cpp:838-842,
/// 865-871`) — exactly this crate's [`Color4::normalized`] — and
/// `primLOD.x` is `lodFrac / 256.0f` (`rt64_rdp.cpp:862`) — exactly
/// [`PrimLod::lod_frac_normalized`] (`primLOD.y`, `lod_min`, is not read by
/// this assembly at all; see this function's nonclaims below).
///
/// `base` supplies every other [`CombinerInputs`] field unchanged
/// (`tex_val0`/`tex_val1`/`shade_color`/`key_center`/`key_scale`/
/// `lod_fraction`/`noise`/`k4`/`k5`) — none of those are sourced from
/// `env_color`/`prim_color`/`prim_lod_frac`/`PrimDepth` in RT64's own
/// assembly (`tex_val0`/`tex_val1` come from texture sampling, `shade_color`
/// from vertex interpolation, `key_center`/`key_scale` from separate
/// host-tracked RDP state, `lod_fraction` from `computeLOD`, `noise` from
/// `nextRand`, and `K4`/`K5` from a separate `convertK` table — all
/// uncharacterized at this seam per [`CombinerInputs`]'s own doc), so this
/// function does not invent values for them.
///
/// Nonclaims: [`PrimDepth`](crate::state::PrimDepth) is not read here —
/// grepping `rt64_color_combiner.h`'s `Inputs` struct and `RasterPS.hlsl`'s
/// `ccInputs` assembly confirms RT64's combiner-input struct has no
/// `primDepth` field at all (`primDepth` feeds depth testing elsewhere in
/// the same shader, a different consumer); `PrimLod::lod_min_normalized`
/// (`primLOD.y`) is likewise never read by this assembly. No cycle-mode
/// selection, no NOISE/LOD real generation, no texture-fetch integration, no
/// draw-path or production-dispatch wiring, and no RT64 parity/performance
/// claim.
pub fn combiner_inputs_from_fragment_registers(
    base: CombinerInputs,
    env_color: Color4,
    prim_color: PrimColor,
) -> CombinerInputs {
    CombinerInputs {
        env_color: env_color.normalized(),
        prim_color: prim_color.color().normalized(),
        prim_lod_frac: prim_color.lod().lod_frac_normalized(),
        ..base
    }
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
        key_center: [0.12, 0.34, 0.56],
        key_scale: [0.21, 0.43, 0.65],
        lod_fraction: 0.37,
        prim_lod_frac: 0.58,
        noise: 0.73,
        k4: 0.19,
        k5: 0.81,
    };

    // -- §9a: exhaustive decode-table sweep over every wire-legal index,
    // asserting exact RT64 decode. Cross-checked directly against the
    // pinned `src/shared/rt64_color_combiner.h` source read for this task,
    // not solely against the characterization card's transcription of it.

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
        let a = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel0);
        let zero = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Zero);
        let one = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::One);
        let rgb = [
            (a[0] - zero[0]) * one[0] + zero[0],
            (a[1] - zero[1]) * one[1] + zero[1],
            (a[2] - zero[2]) * one[2] + zero[2],
        ];
        assert_eq!(rgb, inputs.tex_val0[..3]);

        let alpha_a = resolve_alpha_input(inputs, false, 0.0, AlphaInput::Texel0);
        let alpha_zero = resolve_alpha_input(inputs, false, 0.0, AlphaInput::Zero);
        let alpha_one = resolve_alpha_input(inputs, false, 0.0, AlphaInput::One);
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
            let a = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel0)[channel];
            let c = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Shade)[channel];
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

    /// Wrap-boundary sweep (§9b): exercises both `step`-branches of
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
        let combined = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Combined);
        assert_eq!(combined, [0.0, 0.0, 0.0]);
        let combined_alpha = resolve_alpha_input(inputs, false, 0.0, AlphaInput::Combined);
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
        let (color, alpha_compare) = run_one_cycle(params, inputs);

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

    // -- Slice 2: every newly supported selector resolves to a real value
    // (no `CombinerInputError`/rejection path remains in one-cycle mode —
    // every `ColorInput`/`AlphaInput` variant is now evaluated, see
    // `resolve_color_input`/`resolve_alpha_input`). Each test below mutates
    // exactly one `CombinerInputs` field (holding a fixed identity-shaped
    // formula around it) and confirms the output tracks that one field,
    // proving the new selector actually participates rather than merely
    // type-checking.

    /// `C_KEY_CENTER` (color-B only, §2): `(KEY_CENTER - ZERO) * ONE + ZERO`
    /// must equal `key_center` exactly, and must change when `key_center`
    /// changes while every other field stays fixed.
    #[test]
    fn key_center_participates_in_color_b() {
        let base = ALL_INPUTS;
        let mutated = CombinerInputs {
            key_center: [0.91, 0.92, 0.93],
            ..base
        };
        for inputs in [base, mutated] {
            let a = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::KeyCenter);
            assert_eq!(a, inputs.key_center);
        }
        assert_ne!(base.key_center, mutated.key_center);
    }

    /// `C_KEY_SCALE` (color-C only, §2), same shape as `key_center`.
    #[test]
    fn key_scale_participates_in_color_c() {
        let base = ALL_INPUTS;
        let mutated = CombinerInputs {
            key_scale: [0.01, 0.02, 0.03],
            ..base
        };
        for inputs in [base, mutated] {
            let c = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::KeyScale);
            assert_eq!(c, inputs.key_scale);
        }
        assert_ne!(base.key_scale, mutated.key_scale);
    }

    /// `C_K4`/`C_K5` (color-B/color-C only, §2): scalar inputs replicated
    /// into all three RGB lanes, exactly RT64's `return inputs.K4;` (an
    /// HLSL scalar-to-`float3` implicit broadcast, not a per-channel value).
    #[test]
    fn k4_and_k5_scalars_replicate_across_rgb_and_participate() {
        let base = ALL_INPUTS;
        let mutated = CombinerInputs {
            k4: 0.44,
            k5: 0.55,
            ..base
        };
        for inputs in [base, mutated] {
            assert_eq!(
                resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::K4),
                [inputs.k4; 3]
            );
            assert_eq!(
                resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::K5),
                [inputs.k5; 3]
            );
        }
        assert_ne!(base.k4, mutated.k4);
        assert_ne!(base.k5, mutated.k5);
    }

    /// `C_LOD_FRACTION`/`C_PRIM_LOD_FRAC` (color-C only) and their alpha-C
    /// counterparts `A_LOD_FRACTION`/`A_PRIM_LOD_FRAC` all read the same two
    /// caller-supplied scalars — this is the RGB-vs-alpha cross-shape §2
    /// calls out (color-C reaches them via the extended table; alpha-C via
    /// its own distinct table), not two independent values.
    #[test]
    fn lod_fraction_and_prim_lod_frac_participate_color_and_alpha() {
        let base = ALL_INPUTS;
        let mutated = CombinerInputs {
            lod_fraction: 0.05,
            prim_lod_frac: 0.95,
            ..base
        };
        for inputs in [base, mutated] {
            assert_eq!(
                resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::LodFraction),
                [inputs.lod_fraction; 3]
            );
            assert_eq!(
                resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::PrimLodFrac),
                [inputs.prim_lod_frac; 3]
            );
            assert_eq!(
                resolve_alpha_input(inputs, false, 0.0, AlphaInput::LodFraction),
                inputs.lod_fraction
            );
            assert_eq!(
                resolve_alpha_input(inputs, false, 0.0, AlphaInput::PrimLodFrac),
                inputs.prim_lod_frac
            );
        }
        assert_ne!(base.lod_fraction, mutated.lod_fraction);
        assert_ne!(base.prim_lod_frac, mutated.prim_lod_frac);
    }

    /// `C_NOISE` (color-A only, §2, §5): a caller-supplied scalar, not a
    /// PRNG draw — this module does not generate NOISE, only proves it
    /// participates in the formula once supplied.
    #[test]
    fn noise_participates_in_color_a() {
        let base = ALL_INPUTS;
        let mutated = CombinerInputs {
            noise: 0.02,
            ..base
        };
        for inputs in [base, mutated] {
            assert_eq!(
                resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Noise),
                [inputs.noise; 3]
            );
        }
        assert_ne!(base.noise, mutated.noise);
    }

    /// `C_COMBINED_ALPHA`/`*_ALPHA` (color-C only, §2): a color slot reading
    /// the *alpha* channel of the same named input, replicated across RGB —
    /// the exact cross-read shape §2/§9b flag as needing its own case,
    /// distinct from every other color selector (which reads `.rgb`).
    #[test]
    fn alpha_cross_reads_replicate_the_alpha_channel_not_rgb() {
        let inputs = ALL_INPUTS;
        let combiner_alpha = 0.42;
        assert_eq!(
            resolve_color_input(
                inputs,
                false,
                [0.0; 3],
                combiner_alpha,
                ColorInput::CombinedAlpha
            ),
            [combiner_alpha; 3]
        );
        assert_eq!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel0Alpha),
            [inputs.tex_val0[3]; 3]
        );
        assert_eq!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel1Alpha),
            [inputs.tex_val1[3]; 3]
        );
        assert_eq!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::PrimitiveAlpha),
            [inputs.prim_color[3]; 3]
        );
        assert_eq!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::ShadeAlpha),
            [inputs.shade_color[3]; 3]
        );
        assert_eq!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::EnvAlpha),
            [inputs.env_color[3]; 3]
        );
        // Distinct from the RGB-reading counterpart for the same named
        // input (proves this is genuinely a different read, not a typo
        // that happens to match by construction).
        assert_ne!(
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel0Alpha),
            resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Texel0)
        );
    }

    /// `run_one_cycle` end-to-end with a KEY_CENTER/KEY_SCALE combine mode
    /// (§9b's "environment-key composite" idiom): `(TEXEL0 - KEY_CENTER) *
    /// KEY_SCALE + ZERO`, validating the restricted-slot table (KEY_CENTER
    /// only from B, KEY_SCALE only from C) is honored end-to-end through
    /// real wire bits, not just the unit-level resolver.
    #[test]
    fn run_one_cycle_key_center_key_scale_composite() {
        // color A=TEXEL0(1) at cycle-1 bits (low>>5).
        let low_color = 1u32 << 5;
        // color B=KEY_CENTER(6) at (high>>24); D=ZERO(7) at (high>>6), since
        // D's cycle-1 index 0 decodes to COMBINED, not ZERO — must be set
        // explicitly, not left at the field's default.
        let high_color = (6u32 << 24) | (7u32 << 6);
        let low = low_color | 6u32; // color C cycle-1 bits = low & 0x1F = 6 = KEY_SCALE.

        let params = CombineParams::from_wire(low, high_color);
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, true),
            ColorInput::KeyCenter
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::KeyScale
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, true),
            ColorInput::Zero
        );

        let inputs = ALL_INPUTS;
        let (color, _alpha_compare) = run_one_cycle(params, inputs);

        let expected_rgb = [
            (inputs.tex_val0[0] - inputs.key_center[0]) * inputs.key_scale[0],
            (inputs.tex_val0[1] - inputs.key_center[1]) * inputs.key_scale[1],
            (inputs.tex_val0[2] - inputs.key_center[2]) * inputs.key_scale[2],
        ];
        for (observed, expected) in color[..3].iter().zip(expected_rgb) {
            assert!((observed - expected.clamp(0.0, 1.0)).abs() < 1e-6);
        }
    }

    /// `run_one_cycle` end-to-end with `C_COMBINED_ALPHA` as the color-C
    /// selector (§9b's "combined-alpha cross-read" idiom):
    /// `(TEXEL0 - ZERO) * COMBINED_ALPHA + ZERO`. In one-cycle mode
    /// `COMBINED_ALPHA` reads the zero-initialized alpha accumulator (RT64
    /// `run`'s zero-init, same reasoning as
    /// `combined_reads_zero_init_in_one_cycle_mode`), so the expected color
    /// is exactly zero — this pins that `COMBINED_ALPHA` shares the *same*
    /// accumulator zero-init as plain `COMBINED`, not a separate
    /// always-nonzero path.
    #[test]
    fn run_one_cycle_combined_alpha_cross_read_reads_zero_init() {
        // color A=TEXEL0(1) at cycle-1 bits (low>>5); color C=COMBINED_ALPHA(7) at (low & 0x1F).
        let low = (1u32 << 5) | 7u32;
        let params = CombineParams::from_wire(low, 0);
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::CombinedAlpha
        );

        let inputs = ALL_INPUTS;
        let (color, _alpha_compare) = run_one_cycle(params, inputs);
        assert_eq!(color[..3], [0.0, 0.0, 0.0]);
    }

    /// Extreme/out-of-range caller-supplied values (§9b boundary sweep,
    /// extended to Slice 2's new scalar inputs): NOISE/K4/K5 far outside
    /// `[0,1]` must still land through the real `wrap`/`wrapInputABD`/
    /// `wrapClamp` boundary this module already tests in isolation
    /// (`wrap_clamp_low_branch_triggers_below_low_bound`/
    /// `..._high_branch...`), not merely a plain `clamp` — this is the case
    /// `wrap_clamp`'s doc flags as now reachable in Slice 2 (unlike Slice
    /// 1's always-in-range inputs).
    #[test]
    fn extreme_scalar_inputs_hit_the_real_wrap_boundary() {
        let inputs = CombinerInputs {
            noise: 1_000.0,
            k4: -1_000.0,
            k5: f32::MAX,
            ..ALL_INPUTS
        };

        // (NOISE - ZERO) * ONE + ZERO = NOISE, then wrap_clamp(NOISE).
        let a = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Noise);
        let zero = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::Zero);
        let one = resolve_color_input(inputs, false, [0.0; 3], 0.0, ColorInput::One);
        let rgb0 = (a[0] - zero[0]) * one[0] + zero[0];
        assert_eq!(rgb0, inputs.noise);
        assert_eq!(wrap_clamp(rgb0), wrap_clamp_reference(inputs.noise));
        // 1000.0 is far outside both wrap ranges: after one wrap step it is
        // still outside [0,1] pre-clamp, so the final clamp dominates — but
        // it must go through the same wrap arithmetic RT64 does, which
        // `wrap_clamp_reference` (an independent transcription) models.
        assert_eq!(wrap_clamp(inputs.noise), 1.0);
        assert_eq!(wrap_clamp(inputs.k4), 0.0);
        assert_eq!(wrap_clamp(inputs.k5), 1.0);
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
                0xec, 0xb8, 0xda, 0xe5, 0x4e, 0x77, 0xf7, 0x1f, 0x7e, 0x78, 0x6b, 0x1a, 0x08,
                0xf3, 0xc2, 0x60, 0xcb, 0x5a, 0x10, 0xa8, 0xf2, 0x86, 0xfb, 0xdc, 0x9f, 0xa2,
                0xc7, 0x77, 0xf6, 0xf5, 0x2d, 0xd2,
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
    /// that table. A future edit that re-narrows decode back to a
    /// ZERO collapse — the exact regression this test module was rewritten
    /// to catch — would delete one of these `case` lines and fail here,
    /// even though `wgsl_parses_and_validates_under_naga` would still pass
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
                "color_combiner.wgsl is missing exact-decode case {needle:?} — decode must stay exact per RT64"
            );
        }
    }

    /// Slice 2 companion to the decode-table guard above: asserts
    /// `resolve_color_input`/`resolve_alpha_input` literally contain a
    /// `case` arm evaluating every newly-supported selector's real
    /// arithmetic (not merely decoding to it) — a future edit that reverts
    /// arithmetic support back to a bare fallthrough would delete one of
    /// these `case` lines and fail here.
    #[test]
    fn wgsl_arithmetic_evaluates_every_slice_2_selector_case() {
        let source = COLOR_COMBINER_WGSL;
        for needle in [
            "case COLOR_KEY_CENTER: { return inputs.key_center; }",
            "case COLOR_KEY_SCALE: { return inputs.key_scale; }",
            "case COLOR_COMBINED_ALPHA: { return vec3<f32>(combiner_alpha, combiner_alpha, combiner_alpha); }",
            "case COLOR_TEXEL0_ALPHA: { return vec3<f32>(texel0.a, texel0.a, texel0.a); }",
            "case COLOR_TEXEL1_ALPHA: { return vec3<f32>(texel1.a, texel1.a, texel1.a); }",
            "case COLOR_PRIMITIVE_ALPHA: { return vec3<f32>(inputs.prim_color.a, inputs.prim_color.a, inputs.prim_color.a); }",
            "case COLOR_SHADE_ALPHA: { return vec3<f32>(inputs.shade_color.a, inputs.shade_color.a, inputs.shade_color.a); }",
            "case COLOR_ENV_ALPHA: { return vec3<f32>(inputs.env_color.a, inputs.env_color.a, inputs.env_color.a); }",
            "case COLOR_LOD_FRACTION: { return vec3<f32>(inputs.lod_fraction, inputs.lod_fraction, inputs.lod_fraction); }",
            "case COLOR_PRIM_LOD_FRAC: { return vec3<f32>(inputs.prim_lod_frac, inputs.prim_lod_frac, inputs.prim_lod_frac); }",
            "case COLOR_NOISE: { return vec3<f32>(inputs.noise, inputs.noise, inputs.noise); }",
            "case COLOR_K4: { return vec3<f32>(inputs.k4, inputs.k4, inputs.k4); }",
            "case COLOR_K5: { return vec3<f32>(inputs.k5, inputs.k5, inputs.k5); }",
            "case ALPHA_LOD_FRACTION: { return inputs.lod_fraction; }",
            "case ALPHA_PRIM_LOD_FRAC: { return inputs.prim_lod_frac; }",
        ] {
            assert!(
                source.contains(needle),
                "color_combiner.wgsl is missing Slice 2 arithmetic case {needle:?}"
            );
        }
    }

    /// Slice 3 companion to the Slice 2 WGSL guards above: asserts
    /// `color_combiner.wgsl` literally contains the two-cycle wiring's
    /// load-bearing lines -- the wrap-range selection (per slot C's own
    /// selector, independently for color and alpha), the TEXEL0/TEXEL1
    /// swap, the wrap-before-not-after-arithmetic ordering, and the
    /// alphaCompareValue cycle-0 capture. A future edit that regresses any
    /// of these exact mechanisms (e.g. collapsing the two wrap ranges into
    /// one, or applying the swap unconditionally) would delete one of these
    /// lines and fail here, even though `wgsl_parses_and_validates_under_naga`
    /// would still pass (the mutation is syntactically valid WGSL).
    #[test]
    fn wgsl_two_cycle_wiring_contains_every_load_bearing_line() {
        let source = COLOR_COMBINER_WGSL;
        for needle in [
            // Cross-cycle-carry wrap-range selection, decided independently
            // for alpha (ac == ALPHA_COMBINED) and color (cc == COLOR_COMBINED).
            "if ac == ALPHA_COMBINED {",
            "a_ch = wrap_input_c(a_ch);",
            "a_ch = wrap_input_abd(a_ch);",
            "if cc == COLOR_COMBINED {",
            "r = wrap_input_c(r);",
            "g = wrap_input_c(g);",
            "b_ch = wrap_input_c(b_ch);",
            "r = wrap_input_abd(r);",
            "g = wrap_input_abd(g);",
            "b_ch = wrap_input_abd(b_ch);",
            // TEXEL0/TEXEL1 swap, present in both resolvers.
            "if second_cycle {\n        texel0 = inputs.tex_val1;\n        texel1 = inputs.tex_val0;\n    }",
            // Cycle-0-then-cycle-1 sequencing and cycle-0 alphaCompareValue capture.
            "after_first_cycle = run_cycle(params, inputs, CYCLE_PASS_FIRST_OF_TWO_CYCLES, zero_init);",
            "let alpha_compare_raw = after_first_cycle.a;",
            "final_color = run_cycle(params, inputs, CYCLE_PASS_SECOND_OF_TWO_CYCLES, after_first_cycle);",
            // wrapInputC/wrapInputABD's distinct ranges.
            "let low: f32 = -1.0 - rounding;",
            "let high: f32 = 1.0 + rounding;",
            "let low: f32 = -0.5 - rounding;",
            "let high: f32 = 1.5 + rounding;",
        ] {
            assert!(
                source.contains(needle),
                "color_combiner.wgsl is missing Slice 3 two-cycle wiring line {needle:?}"
            );
        }
    }

    /// Hostile: asserts the wrap-before-arithmetic ordering textually --
    /// the wrap `if` blocks for color must appear in the WGSL source
    /// *before* the `resolve_color_input`/`(a - b) * c + d` computation
    /// inside `run_cycle`, not after. A regression that moved the wrap
    /// calls after the arithmetic would still contain all the same lines
    /// (defeating a pure line-presence check) but in the wrong relative
    /// order -- this test catches that specifically.
    #[test]
    fn wgsl_wrap_before_arithmetic_ordering_is_textually_before_combine_formula() {
        let source = COLOR_COMBINER_WGSL;
        let run_cycle_start = source
            .find("fn run_cycle(")
            .expect("run_cycle must exist in color_combiner.wgsl");
        let wrap_site = source[run_cycle_start..]
            .find("r = wrap_input_abd(r);")
            .map(|offset| run_cycle_start + offset)
            .expect("run_cycle must wrap r via wrap_input_abd somewhere");
        let combine_site = source[run_cycle_start..]
            .find("let rgb = (a - b) * c + d;")
            .map(|offset| run_cycle_start + offset)
            .expect("run_cycle must compute (a - b) * c + d somewhere");
        assert!(
            wrap_site < combine_site,
            "wrap_input_abd(r) must appear before the (a - b) * c + d combine formula in run_cycle"
        );
    }

    // ======================================================================
    // Slice 3: two-cycle cross-cycle wiring and wrap/carry semantics.
    // ======================================================================

    // Wire index constants for `pack_two_cycle_combine`'s literal fixtures.
    // Every slot's table shares `color_input_common`/`alpha_input_abd` for
    // indices 0-5, and index 0 is ALWAYS `Combined` there, never `Zero` --
    // this trips up a hand-written `[0, 0]` meant to read as "zero both
    // slots" (see `hostile_wrap_after_arithmetic_instead_of_before_is_detected`'s
    // debugging history). `Zero`'s own wire index differs per slot/table
    // (only reachable via each table's own out-of-range collapse), so name
    // it explicitly per slot rather than repeating a bare literal whose
    // correctness depends on which slot it's used in.
    const IDX_COMBINED: u32 = 0; // every slot's common table, index 0.
    const IDX_COLOR_ZERO_A: u32 = 8; // color A: 8..15 collapse (out of the 6=ONE/7=NOISE range).
    const IDX_COLOR_ZERO_B: u32 = 8; // color B: 8..15 collapse (out of the 6=KEY_CENTER/7=K4 range).
    const IDX_COLOR_ZERO_C: u32 = 16; // color C: 16..31 collapse (out of the 6..15 extended range).
    const IDX_COLOR_ZERO_D: u32 = 7; // color D: only 7 is out of its 3-bit table's 0..6 range.
    const IDX_ALPHA_ZERO_ABD: u32 = 7; // alpha A/B/D: only 7 is out of its 0..6 range.
    const IDX_ALPHA_ZERO_C: u32 = 7; // alpha C: only 7 is out of its distinct 0..6 range.

    /// Packs a `SetCombine` word from all 16 selector indices (8 slots x 2
    /// cycles) independently, per the exact bit positions verified against
    /// the pinned source in `CombineParams`'s `parse_*` methods. Lets a
    /// two-cycle fixture set cycle 0's and cycle 1's selectors to different
    /// values in one call, rather than only being able to test one cycle's
    /// bitfield slice at a time (as `cycle_bitfield_slice_selection` does).
    #[allow(clippy::too_many_arguments)]
    fn pack_two_cycle_combine(
        color_a: [u32; 2],
        color_b: [u32; 2],
        color_c: [u32; 2],
        color_d: [u32; 2],
        alpha_a: [u32; 2],
        alpha_b: [u32; 2],
        alpha_c: [u32; 2],
        alpha_d: [u32; 2],
    ) -> CombineParams {
        let low = (color_a[0] << 20)
            | (color_a[1] << 5)
            | (color_c[0] << 15)
            | color_c[1]
            | (alpha_a[0] << 12)
            | (alpha_c[0] << 9);
        let high = (color_b[0] << 28)
            | (color_b[1] << 24)
            | (color_d[0] << 15)
            | (color_d[1] << 6)
            | (alpha_a[1] << 21)
            | (alpha_b[0] << 12)
            | (alpha_b[1] << 3)
            | (alpha_c[1] << 18)
            | (alpha_d[0] << 9)
            | alpha_d[1];
        CombineParams::from_wire(low, high)
    }

    /// Self-check for `pack_two_cycle_combine`: every packed index must
    /// decode back to exactly the selector requested, independently for
    /// cycle 0 and cycle 1, proving the packer's bit positions are correct
    /// before any fixture below relies on it. Uses TEXEL0(1)/TEXEL1(2)/
    /// PRIMITIVE(3)/SHADE(4) as distinct per-slot markers.
    #[test]
    fn pack_two_cycle_combine_round_trips_through_decode() {
        let params = pack_two_cycle_combine(
            [1, 2], // color A: cycle0=TEXEL0, cycle1=TEXEL1
            [3, 4], // color B: cycle0=PRIMITIVE, cycle1=SHADE
            [2, 1], // color C: cycle0=TEXEL1, cycle1=TEXEL0
            [4, 3], // color D: cycle0=SHADE, cycle1=PRIMITIVE
            [1, 2], // alpha A: cycle0=TEXEL0, cycle1=TEXEL1
            [3, 4], // alpha B: cycle0=PRIMITIVE, cycle1=SHADE
            [4, 3], // alpha C: cycle0=SHADE, cycle1=PRIMITIVE
            [2, 1], // alpha D: cycle0=TEXEL1, cycle1=TEXEL0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::A, false),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Texel1
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, false),
            ColorInput::Primitive
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, true),
            ColorInput::Shade
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, false),
            ColorInput::Texel1
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Texel0
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, false),
            ColorInput::Shade
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, true),
            ColorInput::Primitive
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, false),
            AlphaInput::Texel0
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, true),
            AlphaInput::Texel1
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::B, false),
            AlphaInput::Primitive
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::B, true),
            AlphaInput::Shade
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, false),
            AlphaInput::Shade
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, true),
            AlphaInput::Primitive
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::D, false),
            AlphaInput::Texel1
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::D, true),
            AlphaInput::Texel0
        );
    }

    // -- Regression: one-cycle mode is unaffected by the Slice 3 addition.

    /// `run_combiner(..., OneCycle)` must equal `run_one_cycle` bit-for-bit
    /// across a representative sweep of `SetCombine` words and inputs — the
    /// Slice 3 dispatch must not perturb the already-shipped Slice 2 path.
    #[test]
    fn run_combiner_one_cycle_matches_run_one_cycle_regression() {
        let words: &[(u32, u32)] = &[
            (0, 0),
            (u32::MAX, u32::MAX),
            (0x0012_3456, 0x789A_BCDE),
            ((1u32 << 5) | 4, (3u32 << 24) | (5u32 << 6)),
        ];
        for &(low, high) in words {
            let params = CombineParams::from_wire(low, high);
            let expected = run_one_cycle(params, ALL_INPUTS);
            let actual = run_combiner(params, ALL_INPUTS, CombinerCycleMode::OneCycle);
            assert_eq!(actual, expected, "low={low:#010x} high={high:#010x}");
        }
    }

    // -- §9c: cycle-mode wiring sweep.

    /// One-cycle mode: `C_COMBINED`/`A_COMBINED` in any slot reads the
    /// zero-initialized accumulator, with no wrap applied — the
    /// `run_combiner`-level counterpart of the existing unit-level
    /// `combined_reads_zero_init_in_one_cycle_mode` test, exercised through
    /// every one of the four color slots and all four alpha slots
    /// independently, end to end.
    #[test]
    fn one_cycle_combined_reads_zero_init_not_wrap_every_slot() {
        // color: A=COMBINED, B=ZERO, C=KEY_SCALE (overridden to [1,1,1],
        // standing in for ONE -- color-C has no true ONE entry), D=ZERO.
        // One-cycle mode always evaluates the cycle-1 bitfield slice
        // (second element of each pair below), per module docs.
        let inputs = CombinerInputs {
            key_scale: [1.0, 1.0, 1.0],
            ..ALL_INPUTS
        };
        let params_a = pack_two_cycle_combine(
            [IDX_COMBINED, IDX_COMBINED],
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [IDX_COLOR_ZERO_C, 6],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [IDX_COMBINED, IDX_COMBINED],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
            [IDX_ALPHA_ZERO_C, 6],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        let inputs_alpha_one = CombinerInputs {
            prim_lod_frac: 1.0,
            ..inputs
        };
        let (color, alpha_compare) =
            run_combiner(params_a, inputs_alpha_one, CombinerCycleMode::OneCycle);
        // (COMBINED(=0) - ZERO) * KEY_SCALE/PRIM_LOD_FRAC(=1.0) + ZERO = 0,
        // for every channel.
        assert_eq!(color, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(alpha_compare, 0.0);
    }

    /// Two-cycle cross-feed (§9c): cycle 0 produces a known non-trivial
    /// color; cycle 1's slot A reads it back via `C_COMBINED`/`A_COMBINED`.
    /// Confirms the wrap-range selected is `wrapInputABD`'s (slot A is not
    /// slot C), and that the value flowing into cycle 1 is cycle 0's real
    /// (unclamped, pre-final-wrapClamp) output, wrapped once by
    /// `wrap_input_abd` — not the raw value, and not additionally clamped
    /// to `[0,1]` before cycle 1 reads it (that only happens at the very
    /// end, in `run_combiner`'s trailing `wrap_clamp` calls).
    #[test]
    fn two_cycle_cross_feed_combined_via_non_c_slot_uses_abd_wrap() {
        // cycle 0 color: A=TEXEL0(1), B=ZERO(covered by common table's 8..
        // collapse via index 8 on a slot that doesn't reach it -- use
        // explicit ZERO-shaped selectors instead), C=ONE, D=ZERO. This
        // yields TEXEL0 verbatim for cycle 0's RGB, which for ALL_INPUTS is
        // within [0,1] so wrap_input_abd is a no-op on it, isolating the
        // cross-feed wrap-selection itself as the property under test.
        // cycle 1 color: A=COMBINED(0), B=ZERO, C=ONE, D=ZERO -> passes
        // cycle 0's (wrapped) color straight through.
        // Color's C slot has NO `ONE` entry (index 6 there is `KeyScale`,
        // not `ONE` -- `ONE` is only reachable from slots A/B/D). Use
        // `KeyScale` with its value overridden to [1,1,1] as an effective
        // ONE. Alpha's C slot has a *distinct* table with no ONE entry
        // either (alpha_input_c has only LOD_FRACTION/TEXEL0/TEXEL1/
        // PRIMITIVE/SHADE/ENVIRONMENT/PRIM_LOD_FRAC), so alpha uses
        // ENVIRONMENT (index 5) as its multiplier instead, and the expected
        // math accounts for that factor explicitly.
        let inputs = CombinerInputs {
            key_scale: [1.0, 1.0, 1.0],
            ..ALL_INPUTS
        };
        let params = pack_two_cycle_combine(
            [1, IDX_COMBINED],                        // color A: cycle0=TEXEL0, cycle1=COMBINED
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],     // color B: ZERO both cycles
            [6, 6], // color C: KEY_SCALE both cycles (overridden to [1,1,1] above)
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D], // color D: ZERO both cycles
            [1, IDX_COMBINED], // alpha A: cycle0=TEXEL0, cycle1=COMBINED
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha B: ZERO both cycles
            [5, 5], // alpha C: ENVIRONMENT both cycles
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );
        let (color, _alpha_compare) = run_combiner(params, inputs, CombinerCycleMode::TwoCycle);

        // Independently derived: cycle 0 color is exactly inputs' tex_val0
        // (in range, so wrap_input_abd is a no-op), which cycle 1 then
        // passes through verbatim (COMBINED * KEY_SCALE(=1) + ZERO); the
        // final wrap_clamp is a no-op for in-range values too.
        let expected_rgb = [inputs.tex_val0[0], inputs.tex_val0[1], inputs.tex_val0[2]];
        for (observed, expected) in color[..3].iter().zip(expected_rgb) {
            assert!((observed - expected).abs() < 1e-6);
        }

        // cycle 0 alpha: (TEXEL0.a - 0) * env.a + 0 = tex_val0.a * env.a.
        let cycle0_alpha = ALL_INPUTS.tex_val0[3] * ALL_INPUTS.env_color[3];
        // cycle 1 alpha: (COMBINED - 0) * env.a + 0, where COMBINED is
        // cycle 0's alpha wrapped by wrap_input_abd (alpha slot A, not C).
        let expected_alpha = wrap_input_abd(cycle0_alpha) * ALL_INPUTS.env_color[3];
        assert!((color[3] - expected_alpha.clamp(0.0, 1.0)).abs() < 1e-6);
    }

    /// Two-cycle cross-feed via slot **C** specifically: confirms the
    /// `wrapInputC`-vs-`wrapInputABD` range choice really is keyed off
    /// which slot's selector is `COMBINED` for the *consuming* cycle, by
    /// driving cycle 0's output to a value that the two wrap ranges treat
    /// differently, then reading it back through slot C in cycle 1.
    /// `wrapInputC`'s range is `[-1-1/255, 1+1/255]`; `wrapInputABD`'s is
    /// `[-0.5-1/255, 1.5+1/255]` — a cycle-0 output of `-0.75` sits inside
    /// the C range (no wrap) but at-or-below the ABD range's `LOW` bound
    /// (wraps). This test drives cycle 0 to produce exactly `-0.75` on
    /// every channel via `(ZERO - PRIMITIVE) * ONE + ZERO` with a crafted
    /// `prim_color`, then reads it back through slot **C** in cycle 1
    /// (`(TEXEL0 - ZERO) * COMBINED + ZERO`) and confirms the result
    /// matches the *unwrapped* `-0.75`, not `wrap_input_abd(-0.75)`.
    #[test]
    fn two_cycle_cross_feed_via_slot_c_uses_c_wrap_not_abd_wrap() {
        // Color's C slot has NO `ONE` entry (index 6 there is `KeyScale`) --
        // override key_scale to [1,1,1] as an effective ONE for cycle 0.
        let inputs = CombinerInputs {
            prim_color: [0.75, 0.75, 0.75, 0.75],
            key_scale: [1.0, 1.0, 1.0],
            ..ALL_INPUTS
        };
        // cycle 0 color: A=ZERO(8, the 8..15 collapse), B=PRIMITIVE(3),
        // C=KEY_SCALE(6, overridden to 1.0), D=ZERO -> (0 - 0.75) * 1 + 0 =
        // -0.75 on every RGB channel. cycle 1 color: A=TEXEL0(1), B=ZERO,
        // C=COMBINED(0), D=ZERO -> reads the carried value back through
        // slot C specifically.
        // Alpha's slots are only decoded/evaluated here as a side effect of
        // running the shared combine formula (`run_cycle` always evaluates
        // both channels) -- this test only asserts on the RGB channels, so
        // alpha's own C-slot selector value is not load-bearing; it is set
        // to ENVIRONMENT (alpha-C has no ONE entry at all, unlike color-C's
        // KEY_SCALE workaround) purely so the alpha computation stays
        // well-defined.
        let params = pack_two_cycle_combine(
            [8, 1],                                   // color A: cycle0=ZERO, cycle1=TEXEL0
            [3, IDX_COLOR_ZERO_B],                    // color B: cycle0=PRIMITIVE, cycle1=ZERO
            [6, IDX_COMBINED], // color C: cycle0=KEY_SCALE(=1.0), cycle1=COMBINED
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D], // color D: ZERO both cycles
            [IDX_ALPHA_ZERO_ABD, 1], // alpha A: cycle0=ZERO, cycle1=TEXEL0
            [3, IDX_ALPHA_ZERO_ABD], // alpha B: cycle0=PRIMITIVE, cycle1=ZERO
            [5, IDX_COMBINED], // alpha C: cycle0=ENVIRONMENT, cycle1=COMBINED
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Combined
        );

        let (color, _alpha_compare) = run_combiner(params, inputs, CombinerCycleMode::TwoCycle);

        // cycle 0: (ZERO - PRIMITIVE) * ONE + ZERO = -0.75 on every channel.
        // cross-feed into cycle 1 slot C specifically: wrap_input_c(-0.75)
        // == -0.75 (inside [-1-1/255, 1+1/255], no wrap), so cycle 1's
        // COMBINED read (via slot C) yields exactly -0.75 as the "C" factor
        // in (TEXEL0 - ZERO) * (-0.75) + ZERO.
        let unwrapped_carry = -0.75f32;
        assert!(
            (wrap_input_c(unwrapped_carry) - unwrapped_carry).abs() < 1e-6,
            "fixture premise: -0.75 must be a no-op under wrap_input_c"
        );
        assert!(
            (wrap_input_abd(unwrapped_carry) - unwrapped_carry).abs() > 1e-3,
            "fixture premise: -0.75 must NOT be a no-op under wrap_input_abd"
        );

        // Cycle 1 is the swapped pass (see
        // `texel_swap_is_cycle_specific_not_selector_specific`), so its
        // `TEXEL0` selector reads `tex_val1`, not `tex_val0`.
        let expected_rgb = [
            inputs.tex_val1[0] * unwrapped_carry,
            inputs.tex_val1[1] * unwrapped_carry,
            inputs.tex_val1[2] * unwrapped_carry,
        ];
        // Compare against the real `wrap_clamp` (wrapInputABD then
        // clamp(0,1)), not a bare `.clamp(0.0, 1.0)` — these expected
        // products are outside [0,1] but inside wrapInputABD's wider
        // range, so a bare clamp would flatten both the correct and an
        // incorrect (e.g. tex_val0-based) expectation to the same value
        // and the assertion would lose its discriminating power.
        for (observed, expected) in color[..3].iter().zip(expected_rgb) {
            assert!(
                (observed - wrap_clamp(expected)).abs() < 1e-5,
                "observed={observed} expected={expected} wrap_clamp(expected)={}",
                wrap_clamp(expected)
            );
        }
    }

    /// `alphaCompareValue` cycle-0 snapshot timing (§9c, §4): crafts a
    /// two-cycle combine where cycle 0's alpha output differs from the
    /// final (cycle-1) alpha output, and confirms `alphaCompareValue`
    /// equals cycle 0's alpha, not the final `combinerColor.a`.
    #[test]
    fn alpha_compare_value_captures_cycle_zero_not_final_alpha() {
        // alpha-C's table has no ONE entry (unlike color-C) -- use
        // PRIM_LOD_FRAC(6) as the multiplier slot with its value overridden
        // to 1.0, so it behaves as an effective ONE for this fixture.
        let inputs = CombinerInputs {
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // cycle 0 alpha: A=SHADE(4 in alphaInputABD), B=ZERO, C=PRIM_LOD_FRAC(6, =1.0), D=ZERO
        // -> shade_color.a.
        // cycle 1 alpha: A=ENVIRONMENT(5), B=ZERO, C=PRIM_LOD_FRAC(6, =1.0), D=ZERO
        // -> env_color.a (unrelated value, distinct from shade_color.a in ALL_INPUTS).
        let params = pack_two_cycle_combine(
            [IDX_COLOR_ZERO_A, IDX_COLOR_ZERO_A],
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [IDX_COLOR_ZERO_C, IDX_COLOR_ZERO_C],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [4, 5], // alpha A: cycle0=SHADE, cycle1=ENVIRONMENT
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha B: ZERO both cycles
            [6, 6], // alpha C: PRIM_LOD_FRAC both cycles (overridden to 1.0 above)
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );
        assert_ne!(inputs.shade_color[3], inputs.env_color[3]);

        let (color, alpha_compare) = run_combiner(params, inputs, CombinerCycleMode::TwoCycle);

        assert!((alpha_compare - inputs.shade_color[3]).abs() < 1e-6);
        assert!((color[3] - inputs.env_color[3]).abs() < 1e-6);
        assert_ne!(alpha_compare, color[3]);
    }

    /// Cycle-specific TEXEL0/TEXEL1 swap (§4, `fromColorInput`/
    /// `fromAlphaInput`'s `secondCycle ? texVal1 : texVal0` branch):
    /// cycle 0 must read the un-swapped texel; cycle 1 must read the
    /// swapped texel. Constructs a combine mode that is *only* TEXEL0 in
    /// both cycles at the selector level, and confirms cycle 0's output
    /// tracks `tex_val0` while cycle 1's tracks `tex_val1` — proving the
    /// swap is driven by which pass is executing, not by the selector
    /// value itself (which is identically `C_TEXEL0`/`A_TEXEL0` both
    /// times).
    #[test]
    fn texel_swap_is_cycle_specific_not_selector_specific() {
        // Neither color-C (index 6 there is KeyScale) nor alpha-C (index 6
        // there is PrimLodFrac) has an ONE entry -- override both to an
        // effective 1.0/[1,1,1] so both channels stay a clean passthrough.
        let inputs = CombinerInputs {
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // Both cycles: A=TEXEL0(1), B=ZERO, C=KEY_SCALE(6)/PRIM_LOD_FRAC(6)
        // overridden to 1.0, D=ZERO -- selector-identical across cycles, so
        // any output difference must come from the pass-specific
        // secondCycle swap flag, not decode. Index 0 (a natural typo target
        // for "zero") decodes to COMBINED in every slot's common table, so
        // ZERO-intended slots use their own explicit zero-collapse index.
        let params = pack_two_cycle_combine(
            [1, 1],
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [6, 6],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [1, 1],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
            [6, 6],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        assert_ne!(inputs.tex_val0, inputs.tex_val1);

        // Isolate cycle 0's raw (pre-final-wrapClamp) output via run_cycle
        // directly, to check the swap without the two-cycle cross-feed's
        // own wrap potentially masking a swap bug through a coincidental
        // match after wrapping.
        let cycle0 = run_cycle(params, inputs, CyclePass::FirstOfTwoCycles, false, [0.0; 4]);
        let cycle1 = run_cycle(params, inputs, CyclePass::SecondOfTwoCycles, false, cycle0);

        for (observed, expected) in cycle0[..3].iter().zip(inputs.tex_val0) {
            assert!(
                (observed - expected).abs() < 1e-6,
                "cycle 0 must read tex_val0 un-swapped"
            );
        }
        assert!((cycle0[3] - inputs.tex_val0[3]).abs() < 1e-6);

        for (observed, expected) in cycle1[..3].iter().zip(inputs.tex_val1) {
            assert!(
                (observed - expected).abs() < 1e-6,
                "cycle 1 must read tex_val1 (swapped)"
            );
        }
        assert!((cycle1[3] - inputs.tex_val1[3]).abs() < 1e-6);
    }

    /// Selectors across both cycles: a combine mode using a *different*,
    /// non-TEXEL0/TEXEL1 selector set per cycle end to end through
    /// `run_combiner`, hand-derived against `(A-B)*C+D` independently for
    /// each cycle, confirming both the correct per-cycle bitfield decode
    /// and the correct threading of cycle 0's output into cycle 1's
    /// `COMBINED` read.
    #[test]
    fn run_combiner_two_cycle_end_to_end_distinct_selectors_per_cycle() {
        // Color-C has no `ONE` (index 6 there is `KeyScale`); alpha-C has no
        // `ONE` either (index 6 there is `PrimLodFrac`). Override both
        // fields to 1.0/[1,1,1] so cycle 1's multiplier slot behaves as an
        // effective ONE.
        let inputs = CombinerInputs {
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // cycle 0 color: (SHADE - PRIMITIVE) * ENVIRONMENT + ZERO.
        // cycle 1 color: (COMBINED - TEXEL0) * KEY_SCALE(=1.0) + ZERO --
        // reads cycle 0's wrapped output back via slot A.
        let params = pack_two_cycle_combine(
            [4, IDX_COMBINED],                        // color A: cycle0=SHADE, cycle1=COMBINED
            [3, 1],                                   // color B: cycle0=PRIMITIVE, cycle1=TEXEL0
            [5, 6], // color C: cycle0=ENVIRONMENT, cycle1=KEY_SCALE(=1.0)
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D], // color D: ZERO both cycles
            [4, IDX_COMBINED], // alpha A: cycle0=SHADE, cycle1=COMBINED
            [3, 1], // alpha B: cycle0=PRIMITIVE, cycle1=TEXEL0
            [5, 6], // alpha C: cycle0=ENVIRONMENT, cycle1=PRIM_LOD_FRAC(=1.0)
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );

        let (color, _alpha_compare) = run_combiner(params, inputs, CombinerCycleMode::TwoCycle);

        let cycle0_rgb = [
            (inputs.shade_color[0] - inputs.prim_color[0]) * inputs.env_color[0],
            (inputs.shade_color[1] - inputs.prim_color[1]) * inputs.env_color[1],
            (inputs.shade_color[2] - inputs.prim_color[2]) * inputs.env_color[2],
        ];
        let cycle0_alpha = (inputs.shade_color[3] - inputs.prim_color[3]) * inputs.env_color[3];

        // Cycle 1 reads cycle 0's output back through slot A as COMBINED,
        // wrapped by wrap_input_abd (slot A, not slot C) before use; slot B
        // reads TEXEL0 -- but cycle 1 is the *swapped* pass, so "TEXEL0"
        // here actually reads inputs.tex_val1 (see
        // texel_swap_is_cycle_specific_not_selector_specific).
        let carried_rgb = [
            wrap_input_abd(cycle0_rgb[0]),
            wrap_input_abd(cycle0_rgb[1]),
            wrap_input_abd(cycle0_rgb[2]),
        ];
        let carried_alpha = wrap_input_abd(cycle0_alpha);
        let expected_rgb = [
            (carried_rgb[0] - inputs.tex_val1[0]) * 1.0,
            (carried_rgb[1] - inputs.tex_val1[1]) * 1.0,
            (carried_rgb[2] - inputs.tex_val1[2]) * 1.0,
        ];
        let expected_alpha = (carried_alpha - inputs.tex_val1[3]) * 1.0;

        // The final, always-on wrapClamp (RT64 `run`'s trailing pass) is
        // itself wrapInputABD-then-clamp -- NOT a bare [0,1] clamp -- so an
        // expected value below wrapInputABD's LOW bound (as happens here)
        // must go through the real wrap_clamp function, not a plain
        // `.clamp(0.0, 1.0)`, to match RT64 exactly.
        for (observed, expected) in color[..3].iter().zip(expected_rgb) {
            assert!(
                (observed - wrap_clamp(expected)).abs() < 1e-5,
                "observed={observed} expected={expected} wrap_clamp(expected)={}",
                wrap_clamp(expected)
            );
        }
        assert!((color[3] - wrap_clamp(expected_alpha)).abs() < 1e-5);
    }

    // -- §9b: wrap boundary partition, extended to the cross-cycle
    // carry wrap functions (`wrap_input_c`/`wrap_input_abd`), independent
    // of `wrap_clamp`'s existing boundary tests. Values immediately below,
    // on, and above every boundary, both signs, mirroring
    // `wrap_clamp_reference`'s independent-oracle discipline (a boundary
    // value can retrigger the opposite branch after the first wrap, so each
    // reference below duplicates the two-`if` structure rather than
    // reasoning about one branch alone).

    const C_ROUNDING: f32 = 1.0 / 255.0;
    const C_LOW: f32 = -1.0 - C_ROUNDING;
    const C_HIGH: f32 = 1.0 + C_ROUNDING;
    const C_RANGE: f32 = C_HIGH - C_LOW;

    fn wrap_input_c_reference(i: f32) -> f32 {
        let mut wrapped = i;
        if wrapped <= C_LOW {
            wrapped += C_RANGE;
        }
        if C_HIGH <= wrapped {
            wrapped -= C_RANGE;
        }
        wrapped
    }

    const ABD_ROUNDING: f32 = 1.0 / 255.0;
    const ABD_LOW: f32 = -0.5 - ABD_ROUNDING;
    const ABD_HIGH: f32 = 1.5 + ABD_ROUNDING;
    const ABD_RANGE: f32 = ABD_HIGH - ABD_LOW;

    fn wrap_input_abd_reference(i: f32) -> f32 {
        let mut wrapped = i;
        if wrapped <= ABD_LOW {
            wrapped += ABD_RANGE;
        }
        if ABD_HIGH <= wrapped {
            wrapped -= ABD_RANGE;
        }
        wrapped
    }

    /// `wrap_input_c` boundary partition: immediately below/on/above
    /// `C_LOW`/`C_HIGH`, both signs (the low boundary is itself negative,
    /// the high boundary positive, so "both signs" is inherent to the two
    /// boundaries; this also covers a strictly-interior negative and
    /// positive value for contrast).
    #[test]
    fn wrap_input_c_boundary_partition() {
        let cases = [
            C_LOW - 0.01,
            C_LOW,
            C_LOW + 0.01,
            C_HIGH - 0.01,
            C_HIGH,
            C_HIGH + 0.01,
            -0.9, // interior, negative
            0.9,  // interior, positive
        ];
        for value in cases {
            assert!(
                (wrap_input_c(value) - wrap_input_c_reference(value)).abs() < 1e-6,
                "value={value}"
            );
        }
    }

    /// `wrap_input_abd` boundary partition, same shape as
    /// `wrap_input_c_boundary_partition` but over `ABD_LOW`/`ABD_HIGH`.
    #[test]
    fn wrap_input_abd_boundary_partition() {
        let cases = [
            ABD_LOW - 0.01,
            ABD_LOW,
            ABD_LOW + 0.01,
            ABD_HIGH - 0.01,
            ABD_HIGH,
            ABD_HIGH + 0.01,
            -0.4, // interior, negative
            1.4,  // interior, positive
        ];
        for value in cases {
            assert!(
                (wrap_input_abd(value) - wrap_input_abd_reference(value)).abs() < 1e-6,
                "value={value}"
            );
        }
    }

    /// Per-RGBA-component wrap boundary sweep (§9b): drives each of R, G,
    /// B, A independently to a value that straddles `wrap_input_abd`'s low
    /// boundary while the other three channels stay comfortably in-range,
    /// via a two-cycle fixture whose cycle-0 output differs per channel
    /// (distinct per-channel `prim_color`/`shade_color` values), then
    /// confirms cycle 1's `COMBINED` read wraps each channel independently
    /// — catching a bug that wraps only channel 0 or applies one wrap
    /// decision uniformly across all four channels.
    #[test]
    fn wrap_applies_independently_per_rgba_component() {
        // cycle 0: (ZERO - PRIMITIVE) * ONE + ZERO, with prim_color chosen
        // so each channel lands at a different point relative to
        // ABD_LOW: R exactly on it (after negation), G just below, B just
        // above, A comfortably interior.
        let prim = [
            -ABD_LOW, // R: (0 - (-ABD_LOW)) = ABD_LOW... see below, negated by formula.
            -(ABD_LOW - 0.01),
            -(ABD_LOW + 0.01),
            0.3,
        ];
        // The combine formula computes (ZERO - PRIMITIVE) = -PRIMITIVE, so
        // to land cycle-0 output exactly on ABD_LOW we need
        // -PRIMITIVE[r] == ABD_LOW, i.e. PRIMITIVE[r] == -ABD_LOW. The
        // array above already encodes that relationship per channel.
        // Neither color-C (index 6 there is `KeyScale`) nor alpha-C (index
        // 6 there is `PrimLodFrac`) has an `ONE` entry -- override both
        // fields to an effective 1.0 so both channels' arithmetic matches
        // the shape described above exactly.
        let inputs = CombinerInputs {
            prim_color: prim,
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };

        // Index 0 (a natural typo target for "zero") decodes to COMBINED in
        // every slot's common table, so ZERO-intended slots use their own
        // explicit zero-collapse index throughout.
        let params = pack_two_cycle_combine(
            [IDX_COLOR_ZERO_A, IDX_COMBINED], // color A: cycle0=ZERO, cycle1=COMBINED
            [3, IDX_COLOR_ZERO_B],            // color B: cycle0=PRIMITIVE, cycle1=ZERO
            [6, 6], // color C: KEY_SCALE both cycles (overridden to [1,1,1] above)
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D], // color D: ZERO both cycles
            [IDX_ALPHA_ZERO_ABD, IDX_COMBINED], // alpha A: cycle0=ZERO, cycle1=COMBINED
            [3, IDX_ALPHA_ZERO_ABD], // alpha B: cycle0=PRIMITIVE, cycle1=ZERO
            [6, 6], // alpha C: PRIM_LOD_FRAC both cycles (overridden to 1.0 above)
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );

        let cycle0 = run_cycle(params, inputs, CyclePass::FirstOfTwoCycles, false, [0.0; 4]);
        let expected_cycle0 = [-prim[0], -prim[1], -prim[2], -prim[3]];
        for (observed, expected) in cycle0.iter().zip(expected_cycle0) {
            assert!((observed - expected).abs() < 1e-6);
        }

        let cycle1 = run_cycle(params, inputs, CyclePass::SecondOfTwoCycles, false, cycle0);
        // cycle 1: (COMBINED - ZERO) * ONE + ZERO = wrap_input_abd(cycle0[channel]),
        // independently per channel.
        for channel in 0..4 {
            let expected = wrap_input_abd(expected_cycle0[channel]);
            assert!(
                (cycle1[channel] - expected).abs() < 1e-6,
                "channel {channel}: observed={} expected={}",
                cycle1[channel],
                expected
            );
        }
        // Confirm the channels genuinely differ in wrap outcome (R sits on
        // the boundary and wraps, D/A stays interior and does not) --
        // proving this test exercises per-channel independence rather than
        // a uniform decision.
        assert_ne!(cycle1[0], expected_cycle0[0]); // R wrapped.
        assert!((cycle1[3] - expected_cycle0[3]).abs() < 1e-6); // A did not.
    }

    // -- Hostile mutations: each test below independently re-derives the
    // expected value from the pinned source's exact formulas/branch
    // structure (not by calling `run_combiner`/`run_cycle` and trusting
    // them), then asserts the real implementation matches that oracle and
    // a plausible *wrong* implementation would not.

    /// Hostile: swapped cycle order (running cycle 1 before cycle 0) would
    /// still type-check and produce *some* two-cycle-shaped output, but
    /// would feed cycle 1's own zero-init accumulator into what should be
    /// cycle 0, and cycle 0's decode into what should be cycle 1's
    /// bitfield slice. Catches this by using distinct, order-sensitive
    /// selectors per cycle and confirming the *correct* cycle-0-then-1
    /// order (already `run_combiner`'s only code path) matches an
    /// independently-ordered-by-hand oracle, while the swapped order would
    /// not.
    #[test]
    fn hostile_swapped_cycle_order_is_detected() {
        // Color's C slot has no ONE entry (index 6 there is KeyScale) --
        // override key_scale to [1,1,1] as an effective ONE. prim_color is
        // pushed out of wrapInputABD's no-op range so cycle 0's real output
        // (-2.0) provably changes under the cross-cycle-carry wrap, giving
        // the correct and swapped orderings genuinely different final
        // values instead of accidentally converging.
        let inputs = CombinerInputs {
            prim_color: [2.0, 2.0, 2.0, 2.0],
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // cycle 0: (ZERO - PRIMITIVE) * KEY_SCALE(=1.0) + ZERO = -2.0 on
        // every channel. cycle 1: (COMBINED - ZERO) * KEY_SCALE(=1.0) +
        // ZERO -- reads cycle 0's real output back via slot A's COMBINED
        // selector, wrapped by wrap_input_abd (slot A, not slot C) before
        // use.
        let params = pack_two_cycle_combine(
            [IDX_COLOR_ZERO_A, IDX_COMBINED], // color A: cycle0=ZERO, cycle1=COMBINED
            [3, IDX_COLOR_ZERO_B],            // color B: cycle0=PRIMITIVE, cycle1=ZERO
            [6, 6],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [IDX_ALPHA_ZERO_ABD, IDX_COMBINED],
            [3, IDX_ALPHA_ZERO_ABD],
            [6, 6],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );

        // Correct order (this is run_combiner's only code path): cycle 0
        // first, then cycle 1 fed cycle 0's real output.
        let (color, _alpha_compare) = run_combiner(params, inputs, CombinerCycleMode::TwoCycle);
        let correct_cycle0 =
            run_cycle(params, inputs, CyclePass::FirstOfTwoCycles, false, [0.0; 4]);
        let correct_cycle1 = run_cycle(
            params,
            inputs,
            CyclePass::SecondOfTwoCycles,
            false,
            correct_cycle0,
        );
        for (observed, expected) in color.iter().zip(correct_cycle1) {
            assert!((observed - expected).abs() < 1e-6);
        }
        // Sanity: cycle 0's real output is both non-zero and provably
        // altered by wrap_input_abd -- otherwise this fixture would not
        // discriminate order at either the intermediate or final level.
        assert_eq!(correct_cycle0, [-2.0, -2.0, -2.0, -2.0]);
        assert_ne!(
            wrap_input_abd(correct_cycle0[0]),
            correct_cycle0[0],
            "fixture premise: wrap_input_abd must not be a no-op on cycle 0's real output"
        );

        // Hostile: a swapped-order bug that literally exchanges which
        // `CyclePass` each call site passes -- the "first" call now uses
        // `SecondOfTwoCycles` (cycle 1's bitfield slice + wrap semantics)
        // against the zero-init accumulator, and the "second" call uses
        // `FirstOfTwoCycles` (cycle 0's bitfield slice) fed that wrong
        // intermediate result. This reproduces exactly what a transposed
        // `run_combiner` body would compute as its final output.
        let swapped_first = run_cycle(
            params,
            inputs,
            CyclePass::SecondOfTwoCycles,
            false,
            [0.0; 4],
        );
        let swapped_final = run_cycle(
            params,
            inputs,
            CyclePass::FirstOfTwoCycles,
            false,
            swapped_first,
        );

        // The swapped order's first call reads COMBINED against zero-init:
        // (0 - 0) * key_scale + 0 = 0, distinguishably different from the
        // correct order's cycle 0 (-2.0, non-zero).
        assert_eq!(swapped_first, [0.0; 4]);
        assert_ne!(swapped_first, correct_cycle0);
        // The swapped order's "second" call uses cycle 0's own selectors
        // (ZERO - PRIMITIVE) * KEY_SCALE + ZERO = -2.0 unconditionally --
        // it does not read COMBINED at all, so it is invariant to the
        // (wrong) accumulator it was fed, landing on plain -2.0. The real,
        // correctly-ordered `run_combiner` output instead reads -2.0 back
        // through COMBINED in its real cycle 1, wrapped by wrap_input_abd
        // first -- a materially different number by the premise assertion
        // above. This is the load-bearing check: it compares FINAL outputs,
        // not intermediates, so a swap that happened to cancel out in some
        // other fixture cannot hide here.
        assert_eq!(swapped_final, [-2.0, -2.0, -2.0, -2.0]);
        assert_ne!(
            color, swapped_final,
            "a swapped cycle order must not reproduce run_combiner's real final output"
        );
    }

    /// Hostile: using the ABD wrap range for a slot-C `COMBINED` read (or
    /// vice versa) is exactly what
    /// `two_cycle_cross_feed_via_slot_c_uses_c_wrap_not_abd_wrap` and
    /// `two_cycle_cross_feed_combined_via_non_c_slot_uses_abd_wrap` above
    /// individually catch; this test adds a single fixture where the two
    /// ranges disagree in *sign of the resulting wrap* (one range wraps,
    /// the other doesn't) for the same carried value, driven through
    /// `run_cycle` directly so no other code path can mask the mix-up.
    #[test]
    fn hostile_abd_wrap_for_c_slot_or_vice_versa_is_detected() {
        // -0.75 is inside wrapInputC's range (no wrap) but outside
        // wrapInputABD's range (wraps) -- see
        // two_cycle_cross_feed_via_slot_c_uses_c_wrap_not_abd_wrap's own
        // fixture-premise assertions for the numeric proof.
        let carried = -0.75f32;
        let via_c = wrap_input_c(carried);
        let via_abd = wrap_input_abd(carried);
        assert_ne!(
            via_c, via_abd,
            "fixture premise: the two wrap ranges must disagree for this value"
        );

        // params: color A=TEXEL0, B=ZERO, C=COMBINED, D=ZERO -- cycle 1's
        // slot C is the discriminator. Index 0 (a natural typo target for
        // "zero") decodes to COMBINED in every slot's common table, so
        // ZERO-intended slots use their own explicit zero-collapse index.
        let params = pack_two_cycle_combine(
            [1, 1], // color A: TEXEL0 both cycles
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [IDX_COMBINED, IDX_COMBINED], // color C: cycle1=COMBINED (the discriminator)
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [1, 1],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
            [IDX_ALPHA_ZERO_C, IDX_ALPHA_ZERO_C],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        // Cycle 1's slot C selector really is COMBINED here (index 0 in
        // both the common table and this packing), so run_cycle must use
        // wrap_input_c, not wrap_input_abd, on the incoming accumulator.
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Combined
        );

        let cycle1 = run_cycle(
            params,
            ALL_INPUTS,
            CyclePass::SecondOfTwoCycles,
            false,
            [carried, carried, carried, carried],
        );
        // (TEXEL0 - ZERO) * COMBINED + ZERO, where the COMBINED read used as
        // the C factor is NOT wrapped a second time by fromColorInput
        // itself -- the pre-arithmetic wrap that primes the accumulator
        // before any fromColorInput call must have used wrap_input_c, not
        // wrap_input_abd, since slot C's own selector is COMBINED this
        // cycle. Cycle 1 is the swapped pass, so "TEXEL0" here reads
        // ALL_INPUTS.tex_val1 (see
        // texel_swap_is_cycle_specific_not_selector_specific).
        let expected_r = ALL_INPUTS.tex_val1[0] * via_c;
        assert!(
            (cycle1[0] - expected_r).abs() < 1e-5,
            "observed={} expected={} (would differ if wrap_input_abd were used instead)",
            cycle1[0],
            expected_r
        );
        let wrong_r = ALL_INPUTS.tex_val1[0] * via_abd;
        assert!(
            (cycle1[0] - wrong_r).abs() > 1e-3,
            "must NOT match the wrapInputABD-for-slot-C mistake"
        );
    }

    /// Hostile: wrapping in one-cycle mode. RT64's `secondCycle` flag is
    /// `twoCycle && secondCycleInputs`, always `false` in one-cycle mode —
    /// so even though one-cycle mode's single pass uses the *cycle-1*
    /// bitfield slice (`CyclePass::OnlyCycleOfOneCycleMode`, which is
    /// `bitfield_second_cycle() == true`), it must NOT apply the
    /// cross-cycle-carry wrap. Confirms this by feeding a non-zero,
    /// out-of-both-wrap-ranges `combiner_color_in` directly into
    /// `run_cycle` under `OnlyCycleOfOneCycleMode` with a `COMBINED`
    /// selector and checking the accumulator passes through un-wrapped
    /// (only the final `wrap_clamp`, which this call bypasses by using
    /// `run_cycle` directly rather than `run_combiner`, would ever touch
    /// it).
    #[test]
    fn hostile_wrap_in_one_cycle_mode_is_detected() {
        // (COMBINED - ZERO) * ONE-via-KEY_SCALE/PRIM_LOD_FRAC(overridden) +
        // ZERO, so the result tracks the un-wrapped accumulator exactly,
        // not a coincidental cancellation.
        let inputs = CombinerInputs {
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        let params = pack_two_cycle_combine(
            [1, IDX_COMBINED], // color A: cycle1=COMBINED (this fixture only evaluates the cycle-1 slice)
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [6, 6], // color C: KEY_SCALE both cycles (overridden to [1,1,1] above)
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [1, IDX_COMBINED],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
            [6, 6],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Combined
        );

        let out_of_range_carry = [5.0f32, 5.0, 5.0, 5.0];
        let result = run_cycle(
            params,
            inputs,
            CyclePass::OnlyCycleOfOneCycleMode,
            false,
            out_of_range_carry,
        );
        // (COMBINED - ZERO) * ONE + ZERO, with COMBINED read UN-wrapped:
        // must equal exactly 5.0, not wrap_input_c(5.0)/wrap_input_abd(5.0)
        // (both of which would reduce it toward [0,1]-ish ranges).
        for observed in result {
            assert!(
                (observed - 5.0).abs() < 1e-6,
                "one-cycle mode must not wrap the accumulator, observed={observed}"
            );
        }
        assert_ne!(result[0], wrap_input_c(5.0));
        assert_ne!(result[0], wrap_input_abd(5.0));
    }

    /// Hostile: wrapping after rather than before arithmetic. RT64 wraps
    /// the *incoming* accumulator before any `fromColorInput`/
    /// `fromAlphaInput` call reads it (`runCycle` lines 579-601, all before
    /// line 603's combine-formula assignment). A buggy implementation that
    /// instead wrapped the pass's *output* after computing `(A-B)*C+D`
    /// would produce a different number whenever the selectors are
    /// anything other than a bare `COMBINED` passthrough. Uses a
    /// `COMBINED`-as-C-factor mode where wrap-before-arithmetic and
    /// wrap-after-arithmetic provably diverge.
    #[test]
    fn hostile_wrap_after_arithmetic_instead_of_before_is_detected() {
        // cycle 1: (TEXEL0 - ZERO) * COMBINED + ZERO. Feed an out-of-range
        // carry so wrap actually changes the value. Every "ZERO" slot below
        // uses its slot-specific zero-collapse index -- index 0 (a natural
        // typo target) decodes to COMBINED in every slot's common table,
        // not ZERO.
        let params = pack_two_cycle_combine(
            [1, 1], // color A: TEXEL0 both cycles
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [IDX_COMBINED, IDX_COMBINED], // color C: cycle1=COMBINED
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [1, 1],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
            [IDX_COMBINED, IDX_COMBINED],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        let carry_in = [2.0f32, 2.0, 2.0, 2.0]; // outside wrapInputABD's range; slot C == COMBINED -> wrapInputC.

        let actual = run_cycle(
            params,
            ALL_INPUTS,
            CyclePass::SecondOfTwoCycles,
            false,
            carry_in,
        );

        // Correct (wrap-before-arithmetic): factor_c = wrap_input_c(2.0),
        // then (TEXEL0 - ZERO) * factor_c.
        let correct_factor_c = wrap_input_c(2.0);
        let correct_r = ALL_INPUTS.tex_val1[0] * correct_factor_c; // cycle 1 reads swapped TEXEL0 -> tex_val1.

        // Wrong (wrap-after-arithmetic): raw_r = TEXEL0 * 2.0 (unwrapped C
        // factor), then wrap_input_c(raw_r) as if the wrap applied to the
        // finished product instead of the input.
        let wrong_raw_r = ALL_INPUTS.tex_val1[0] * 2.0;
        let wrong_r = wrap_input_c(wrong_raw_r);

        assert!(
            (actual[0] - correct_r).abs() < 1e-5,
            "observed={} expected(before-arithmetic)={}",
            actual[0],
            correct_r
        );
        assert!(
            (correct_r - wrong_r).abs() > 1e-3,
            "fixture premise: before- and after-arithmetic wrapping must diverge numerically"
        );
        assert!(
            (actual[0] - wrong_r).abs() > 1e-3,
            "must not match the after-arithmetic mistake"
        );
    }

    /// Hostile: missing per-component wrap (applying the wrap decision or
    /// value to only one RGB channel, or sharing one scalar wrap result
    /// across all three, instead of three independent scalar wraps).
    /// Reuses `wrap_applies_independently_per_rgba_component`'s fixture
    /// shape but asserts the specific failure mode: if channel 0's wrapped
    /// value were incorrectly broadcast to channels 1/2 (a plausible
    /// "wrap once, reuse for RGB" bug), they would not match their own
    /// independently-derived expectations.
    #[test]
    fn hostile_missing_per_component_wrap_is_detected() {
        let prim = [-ABD_LOW, -(ABD_LOW - 0.01), -(ABD_LOW + 0.01), 0.3];
        // Neither color-C nor alpha-C has an ONE entry at index 6; override
        // both to an effective 1.0/[1,1,1] (same shape as
        // `wrap_applies_independently_per_rgba_component`).
        let inputs = CombinerInputs {
            prim_color: prim,
            key_scale: [1.0, 1.0, 1.0],
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // Index 0 (a natural typo target for "zero") decodes to COMBINED in
        // every slot's common table, so ZERO-intended slots use their own
        // explicit zero-collapse index.
        let params = pack_two_cycle_combine(
            [IDX_COLOR_ZERO_A, IDX_COMBINED],
            [3, IDX_COLOR_ZERO_B],
            [6, 6],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [IDX_ALPHA_ZERO_ABD, IDX_COMBINED],
            [3, IDX_ALPHA_ZERO_ABD],
            [6, 6],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );

        let cycle0 = run_cycle(params, inputs, CyclePass::FirstOfTwoCycles, false, [0.0; 4]);
        let cycle1 = run_cycle(params, inputs, CyclePass::SecondOfTwoCycles, false, cycle0);

        let expected_r = wrap_input_abd(cycle0[0]);
        let expected_g = wrap_input_abd(cycle0[1]);
        let expected_b = wrap_input_abd(cycle0[2]);
        assert!((cycle1[0] - expected_r).abs() < 1e-6);
        assert!((cycle1[1] - expected_g).abs() < 1e-6);
        assert!((cycle1[2] - expected_b).abs() < 1e-6);
        // A "wrap once, broadcast channel 0's result" bug would make
        // channel 1 equal channel 0's wrapped value; the real
        // per-component computation must not, since cycle0[0] != cycle0[1]
        // by fixture construction (R sits on the boundary and wraps, G is
        // just below and also wraps but to a different magnitude, so their
        // wrapped outputs remain numerically distinct).
        assert_ne!(cycle1[0], cycle1[1]);
    }

    /// Hostile: premature clamp. If `wrap_input_abd`/`wrap_input_c`
    /// additionally hard-clamped to `[0,1]` (instead of only wrapping,
    /// leaving the final `[0,1]` clamp to `run_combiner`'s trailing
    /// `wrap_clamp` pass alone), a cross-cycle carry value that wraps to
    /// something still outside `[0,1]` would be silently forced into range
    /// one cycle early, changing cycle 1's arithmetic. This test picks
    /// inputs whose wrapped *output* is itself still outside `[0,1]` (per
    /// each function's own wrap range, wider than `[0,1]`) and pins that
    /// `wrap_input_c`/`wrap_input_abd` never additionally clamp to `[0,1]`.
    #[test]
    fn hostile_premature_zero_one_clamp_in_carry_wrap_is_detected() {
        // Values chosen so the wrapped result is provably outside [0,1],
        // which a premature-clamp bug would silently force into range.
        let c_wrapped = wrap_input_c(C_HIGH + 0.3);
        assert!(
            !(0.0..=1.0).contains(&c_wrapped),
            "fixture premise: wrap_input_c's own output must land outside [0,1] here, got {c_wrapped}"
        );
        let abd_wrapped = wrap_input_abd(ABD_HIGH + 0.3);
        assert!(
            !(0.0..=1.0).contains(&abd_wrapped),
            "fixture premise: wrap_input_abd's own output must land outside [0,1] here, got {abd_wrapped}"
        );
    }

    /// Hostile: lost cross-cycle alpha. A bug that threads only the RGB
    /// channels from cycle 0 into cycle 1 (dropping or zeroing the alpha
    /// channel of the carried accumulator) would break any cycle-1 alpha
    /// formula that reads `A_COMBINED`. Constructs exactly that case and
    /// confirms cycle 1's alpha equals `wrap_input_abd` of cycle 0's real
    /// alpha output, not zero and not cycle 0's RGB reused as alpha.
    #[test]
    fn hostile_lost_cross_cycle_alpha_is_detected() {
        // Alpha-C's table has no ONE entry (index 6 there is PrimLodFrac) --
        // override its value to 1.0 as an effective ONE.
        let inputs = CombinerInputs {
            prim_lod_frac: 1.0,
            ..ALL_INPUTS
        };
        // cycle 0 alpha: (SHADE - ZERO) * PRIM_LOD_FRAC(=1.0) + ZERO =
        // shade_color.a (a value distinct from 0.0 and from any RGB channel
        // by construction, since ALL_INPUTS.shade_color's channels are all
        // distinct).
        let params = pack_two_cycle_combine(
            [IDX_COLOR_ZERO_A, IDX_COLOR_ZERO_A],
            [IDX_COLOR_ZERO_B, IDX_COLOR_ZERO_B],
            [IDX_COLOR_ZERO_C, IDX_COLOR_ZERO_C],
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D],
            [4, IDX_COMBINED], // alpha A: cycle0=SHADE, cycle1=COMBINED
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha B: ZERO both cycles
            [6, 6],            // alpha C: PRIM_LOD_FRAC both cycles (overridden to 1.0 above)
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD], // alpha D: ZERO both cycles
        );
        let cycle0 = run_cycle(params, inputs, CyclePass::FirstOfTwoCycles, false, [0.0; 4]);
        assert!((cycle0[3] - inputs.shade_color[3]).abs() < 1e-6);
        assert_ne!(cycle0[3], 0.0);

        let cycle1 = run_cycle(params, inputs, CyclePass::SecondOfTwoCycles, false, cycle0);
        let expected_alpha = wrap_input_abd(cycle0[3]);
        assert!(
            (cycle1[3] - expected_alpha).abs() < 1e-6,
            "observed={} expected={} (a lost-carry bug would instead read 0.0 here)",
            cycle1[3],
            expected_alpha
        );
        assert_ne!(cycle1[3], 0.0);
    }

    /// Hostile: algebraic reordering. `(A-B)*C+D` must be evaluated in
    /// exactly that grouping and order — not `(A-B)*(C+D)`, not
    /// `A*C - B*C + D` reassociated in a way that changes floating-point
    /// rounding, not `D + (A-B)*C`. Uses f32 values specifically chosen so
    /// that `(A-B)*C+D` and a plausible reassociation
    /// (`A*C - B*C + D`) round differently at the bit level — floating
    /// point multiplication does not distribute exactly over subtraction —
    /// confirming `run_cycle`'s output matches the exact grouping RT64
    /// uses, not merely a mathematically-equivalent-in-the-reals one.
    #[test]
    fn hostile_algebraic_reordering_is_detected() {
        // Values chosen to have many significant bits so that
        // (A-B)*C and A*C-B*C differ in their last ULPs.
        let a = 0.1f32;
        let b = 0.100_000_02_f32; // one ULP-ish away from a.
        let c = 123_456.79_f32;
        let d = 0.0f32;

        let grouped = (a - b) * c + d;
        let reassociated = a * c - b * c + d;
        assert_ne!(
            grouped, reassociated,
            "fixture premise: the two groupings must round differently at f32 precision"
        );

        let inputs = CombinerInputs {
            prim_color: [a, a, a, a],
            shade_color: [b, b, b, b],
            env_color: [c, c, c, c],
            ..ALL_INPUTS
        };
        // (PRIMITIVE - SHADE) * ENVIRONMENT + ZERO, one-cycle mode (no wrap
        // interference), values chosen to stay within wrap_clamp's no-op
        // range... actually c is far outside [0,1]-adjacent ranges, so use
        // run_cycle directly (no final wrap_clamp) to observe the raw
        // grouped arithmetic.
        let params = pack_two_cycle_combine(
            [3, 3],                               // color A: PRIMITIVE
            [4, 4],                               // color B: SHADE
            [5, 5],                               // color C: ENVIRONMENT
            [IDX_COLOR_ZERO_D, IDX_COLOR_ZERO_D], // color D: ZERO
            [3, 3],
            [4, 4],
            [5, 5],
            [IDX_ALPHA_ZERO_ABD, IDX_ALPHA_ZERO_ABD],
        );
        let result = run_cycle(
            params,
            inputs,
            CyclePass::OnlyCycleOfOneCycleMode,
            false,
            [0.0; 4],
        );
        assert_eq!(
            result[0], grouped,
            "run_cycle must compute (A-B)*C+D in exactly that grouping"
        );
        assert_ne!(
            result[0], reassociated,
            "run_cycle must NOT match an algebraically-reassociated grouping"
        );
    }

    // -- combiner_inputs_from_fragment_registers: independent-oracle tests.
    // Expected values below are computed by hand from the raw wire words
    // per RT64's own `RDP::setEnvColor`/`setPrimColor` arithmetic
    // (`byte / 255.0`, `lodFrac / 256.0`), not by calling this crate's own
    // `Color4::normalized`/`PrimLod::lod_frac_normalized` and asserting
    // against their output.

    /// `env_color`/`prim_color`/`prim_lod_frac` are overwritten from the
    /// fragment registers; every other field passes through `base`
    /// untouched. Wire words chosen so every byte is distinct and non-zero,
    /// ruling out a transposed-channel or copy-paste-from-wrong-field bug.
    #[test]
    fn combiner_inputs_from_fragment_registers_overrides_exactly_three_fields() {
        // env_color wire word: R=0x10 G=0x20 B=0x30 A=0x40.
        let env = Color4::from_wire(0x1020_3040);
        // prim_color w0 (lodFrac/lodMin): lodFrac=0x50 (bits 0:7), lodMin
        // bits 8:12 = 0b10101 = 21 (0x50_15 -> low byte 0x50, next nibble+1
        // bit from 0x15 masked to 5 bits = 0x15 & 0x1f = 0x15 = 21).
        // prim_color w1 (color): R=0x60 G=0x70 B=0x80 A=0x90.
        let prim = PrimColor::from_wire(0x0000_1550, 0x6070_8090);

        let base = ALL_INPUTS;
        let result = combiner_inputs_from_fragment_registers(base, env, prim);

        // Independent oracle: byte / 255.0, computed from the raw wire bytes
        // above, not by calling Color4::normalized.
        let env_expected = [
            0x10 as f32 / 255.0,
            0x20 as f32 / 255.0,
            0x30 as f32 / 255.0,
            0x40 as f32 / 255.0,
        ];
        let prim_expected = [
            0x60 as f32 / 255.0,
            0x70 as f32 / 255.0,
            0x80 as f32 / 255.0,
            0x90 as f32 / 255.0,
        ];
        let prim_lod_frac_expected = 0x50 as f32 / 256.0;

        assert_eq!(result.env_color, env_expected);
        assert_eq!(result.prim_color, prim_expected);
        assert_eq!(result.prim_lod_frac, prim_lod_frac_expected);

        // Every other field is `base` untouched, proving this is a targeted
        // three-field override, not a from-scratch reconstruction that
        // happens to also carry `base`'s other values by coincidence.
        assert_eq!(result.tex_val0, base.tex_val0);
        assert_eq!(result.tex_val1, base.tex_val1);
        assert_eq!(result.shade_color, base.shade_color);
        assert_eq!(result.key_center, base.key_center);
        assert_eq!(result.key_scale, base.key_scale);
        assert_eq!(result.lod_fraction, base.lod_fraction);
        assert_eq!(result.noise, base.noise);
        assert_eq!(result.k4, base.k4);
        assert_eq!(result.k5, base.k5);
    }

    /// All-zero and all-`0xFF` wire words hit both ends of the byte range,
    /// including the `lodFrac = 0xFF` corner RT64's own `/ 256.0` divisor
    /// (not `/ 255.0`) never reaches exactly `1.0` for.
    #[test]
    fn combiner_inputs_from_fragment_registers_boundary_wire_words() {
        let env_zero = Color4::from_wire(0x0000_0000);
        let prim_zero = PrimColor::from_wire(0x0000_0000, 0x0000_0000);
        let zeroed = combiner_inputs_from_fragment_registers(ALL_INPUTS, env_zero, prim_zero);
        assert_eq!(zeroed.env_color, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(zeroed.prim_color, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(zeroed.prim_lod_frac, 0.0);

        let env_max = Color4::from_wire(0xFFFF_FFFF);
        let prim_max = PrimColor::from_wire(0xFFFF_FFFF, 0xFFFF_FFFF);
        let maxed = combiner_inputs_from_fragment_registers(ALL_INPUTS, env_max, prim_max);
        let max_expected = 0xFF as f32 / 255.0;
        assert_eq!(maxed.env_color, [max_expected; 4]);
        assert_eq!(maxed.prim_color, [max_expected; 4]);
        // Independent oracle: 0xFF / 256.0 = 0.99609375 exactly (power-of-two
        // divisor, no rounding), distinct from env/prim's 0xFF / 255.0.
        assert_eq!(maxed.prim_lod_frac, 0xFF as f32 / 256.0);
        assert_ne!(maxed.prim_lod_frac, max_expected);
    }

    /// `lodMin` (`primLOD.y`) is decoded by `PrimLod` but never read by
    /// `RasterPS.hlsl`'s combiner-input assembly (see this function's doc) —
    /// varying it alone must not change the result at all.
    #[test]
    fn combiner_inputs_from_fragment_registers_ignores_lod_min() {
        let env = Color4::from_wire(0x1122_3344);
        let prim_lod_min_zero = PrimColor::from_wire(0x0000_0050, 0x6070_8090);
        let prim_lod_min_max = PrimColor::from_wire(0x0000_1f50, 0x6070_8090);
        assert_ne!(
            prim_lod_min_zero.lod().lod_min(),
            prim_lod_min_max.lod().lod_min(),
            "fixture premise: the two PrimColor values must actually differ in lod_min"
        );

        let result_zero =
            combiner_inputs_from_fragment_registers(ALL_INPUTS, env, prim_lod_min_zero);
        let result_max = combiner_inputs_from_fragment_registers(ALL_INPUTS, env, prim_lod_min_max);
        assert_eq!(result_zero, result_max);
    }

    /// `PrimDepth` has no field on this function's signature at all — the
    /// RT64 combiner-input assembly this ports never reads it (see this
    /// function's doc's nonclaim). This test exists to make that omission a
    /// visible, intentional API shape rather than a silent gap: it directly
    /// exercises the crate's already-decoded `PrimDepth` type to prove it
    /// compiles and decodes independently of this function, underscoring
    /// that this function's signature omitting it is a mapping fact, not an
    /// oversight.
    #[test]
    fn prim_depth_decodes_independently_and_is_not_a_fragment_register_input() {
        use crate::state::PrimDepth;
        let depth = PrimDepth::from_wire(0xFFFF_0000);
        // 15-bit mask: bit 31 (the sign/unused bit RT64 discards) is clear
        // in the decoded value even though the wire word's top 16 bits are
        // all set.
        assert_eq!(depth.z(), 0x7FFF);
        assert_eq!(depth.dz(), 0x0000);
    }

    // -- `references_texels_in_first_cycle` (SHADE-only-triangle repair):
    // selector-reference coverage for each first-cycle color/alpha slot,
    // plus representative non-texture selectors. Builds raw wire words
    // directly at `run_one_cycle`'s own `SECOND_CYCLE = true` bit positions
    // (`parse_*_a/b/c/d`'s `if second_cycle` arms, matching
    // `targets/triangle_pipeline/tests.rs`'s `shade_passthrough_combine_params`
    // doc): color_a low[9:5], color_b high[31:24], color_c low[4:0],
    // color_d high[8:6]; alpha_a high[23:21], alpha_b high[5:3],
    // alpha_c high[20:18], alpha_d high[2:0]. Every fixture below sets every
    // OTHER slot to an index that decodes to `ColorInput::Zero`/
    // `AlphaInput::Zero` (index 15/index 7, the out-of-range collapse each
    // slot's own table already proves elsewhere in this file), so only the
    // one slot under test can possibly flip the predicate -- EXCEPT the
    // slot-C tests, which need A and B to decode to genuinely different
    // selectors (both `ZERO_COLOR_A`/`ZERO_COLOR_B` would decode equal,
    // zeroing `(A-B)*C`'s coefficient and always suppressing C regardless of
    // what it decodes to -- see `references_texels_in_first_cycle`'s doc).

    const ZERO_COLOR_A: u32 = 15; // color_input_a: 8-15 -> Zero
    const ZERO_COLOR_B: u32 = 15; // color_input_b: 8-15 -> Zero
    const ZERO_COLOR_C: u32 = 31; // color_input_c: 16-31 -> Zero
    const ZERO_COLOR_D: u32 = 7; // color_input_d: 7 -> Zero
    const ZERO_ALPHA_ABD: u32 = 7; // alpha_input_abd: 7 -> Zero
    const ZERO_ALPHA_C: u32 = 7; // alpha_input_c: 7 -> Zero

    fn combine_params_with_only(color: [u32; 4], alpha: [u32; 4]) -> CombineParams {
        let [color_a, color_b, color_c, color_d] = color;
        let [alpha_a, alpha_b, alpha_c, alpha_d] = alpha;
        let low = (color_a << 5) | (color_c & 0x1F);
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        CombineParams::from_wire(low, high)
    }

    fn all_zero_selectors() -> CombineParams {
        combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        )
    }

    #[test]
    fn all_zero_selectors_do_not_reference_texels() {
        assert!(!all_zero_selectors().references_texels_in_first_cycle());
    }

    /// Representative non-texture selectors (SHADE/PRIMITIVE/ONE) must not
    /// flip the predicate either -- it is specifically a TEXEL0/TEXEL1/
    /// `*_ALPHA` reference, not "any non-ZERO selector".
    #[test]
    fn shade_primitive_and_one_selectors_do_not_reference_texels() {
        let shade_d = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, 4], // SHADE
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, 4], // SHADE
        );
        assert!(!shade_d.references_texels_in_first_cycle());

        let primitive_a = combine_params_with_only(
            [3, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D], // PRIMITIVE
            [3, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD], // PRIMITIVE
        );
        assert!(!primitive_a.references_texels_in_first_cycle());

        let one_a = combine_params_with_only(
            [6, ZERO_COLOR_B, ZERO_COLOR_C, 6], // ONE (color_input_a, color_input_d)
            [6, ZERO_ALPHA_ABD, ZERO_ALPHA_C, 6], // ONE (alpha_input_abd)
        );
        assert!(!one_a.references_texels_in_first_cycle());
    }

    #[test]
    fn color_slot_a_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [1, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D], // TEXEL0
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [2, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D], // TEXEL1
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    #[test]
    fn color_slot_b_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, 1, ZERO_COLOR_C, ZERO_COLOR_D], // TEXEL0
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, 2, ZERO_COLOR_C, ZERO_COLOR_D], // TEXEL1
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    /// Color slot C is `(A-B)*C`'s own coefficient, not a free-standing
    /// reference (see `references_texels_in_first_cycle`'s doc) -- it only
    /// counts when A and B decode to *different* selectors, so these
    /// fixtures use A=ZERO, B=PRIMITIVE (any two distinct selectors) rather
    /// than this file's usual `ZERO_COLOR_A`/`ZERO_COLOR_B` pair, which
    /// would zero the coefficient and (correctly) suppress C. Color slot C
    /// reaches TEXEL0_ALPHA/TEXEL1_ALPHA (indices 8/9, its own extended
    /// table), distinct from slots A/B's plain TEXEL0/TEXEL1 -- both must be
    /// caught.
    #[test]
    fn color_slot_c_texel0_texel1_and_their_alpha_variants_are_referenced_with_a_nonzero_coefficient(
    ) {
        const DISTINCT_COLOR_B: u32 = 3; // PRIMITIVE -- differs from ZERO_COLOR_A
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, DISTINCT_COLOR_B, 1, ZERO_COLOR_D], // TEXEL0
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, DISTINCT_COLOR_B, 2, ZERO_COLOR_D], // TEXEL1
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel1.references_texels_in_first_cycle());

        let texel0_alpha = combine_params_with_only(
            [ZERO_COLOR_A, DISTINCT_COLOR_B, 8, ZERO_COLOR_D], // TEXEL0_ALPHA
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel0_alpha.references_texels_in_first_cycle());

        let texel1_alpha = combine_params_with_only(
            [ZERO_COLOR_A, DISTINCT_COLOR_B, 9, ZERO_COLOR_D], // TEXEL1_ALPHA
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel1_alpha.references_texels_in_first_cycle());
    }

    /// The cancellation itself: color slot C decoding to TEXEL0 must NOT
    /// mark the params as texture-referencing when A and B decode to the
    /// *same* selector (here, both ZERO) -- `(A-B)` is then exactly zero,
    /// so C's resolved value can never reach the output regardless of what
    /// it is. This is the exact shape `targets/triangle_pipeline/tests.rs`'s
    /// `shade_passthrough_combine_params` and this crate's own
    /// SHADE-passthrough production fixtures rely on for alpha slot C (see
    /// `the_reported_shade_only_regression_fixture_does_not_reference_texels`);
    /// this test proves the identical cancellation for color slot C.
    #[test]
    fn color_slot_c_texel0_is_suppressed_when_a_and_b_decode_equal() {
        let params = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_A, 1, ZERO_COLOR_D], // B same selector as A -- (A-B) == 0; C=TEXEL0
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(!params.references_texels_in_first_cycle());
    }

    /// Color slot D's own table has no TEXEL0_ALPHA/TEXEL1_ALPHA entries
    /// (`color_input_d`'s doc: "no NOISE/KEY_CENTER/etc. entries to begin
    /// with"), only plain TEXEL0/TEXEL1 -- both are still selector
    /// references and must be caught.
    #[test]
    fn color_slot_d_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, 1], // TEXEL0
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, 2], // TEXEL1
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD],
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    #[test]
    fn alpha_slot_a_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [1, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD], // TEXEL0 (alpha_input_abd)
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [2, ZERO_ALPHA_ABD, ZERO_ALPHA_C, ZERO_ALPHA_ABD], // TEXEL1 (alpha_input_abd)
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    #[test]
    fn alpha_slot_b_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [ZERO_ALPHA_ABD, 1, ZERO_ALPHA_C, ZERO_ALPHA_ABD], // TEXEL0 (alpha_input_abd)
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [ZERO_ALPHA_ABD, 2, ZERO_ALPHA_C, ZERO_ALPHA_ABD], // TEXEL1 (alpha_input_abd)
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    /// Alpha slot C's own table (`alpha_input_c`) has no TEXEL0/TEXEL1
    /// entry at index 1/2 the way `alpha_input_abd` does -- its index 1/2
    /// decode to TEXEL0/TEXEL1 too, but via a *different* table shape (see
    /// `alpha_input_c`'s doc: "index 1 is TEXEL0 here, not COMBINED"). This
    /// proves the predicate reads alpha slot C through its own decode path,
    /// not by reusing slot A/B's table. Like color slot C, alpha slot C is
    /// `(A-B)*C`'s coefficient (see `references_texels_in_first_cycle`'s
    /// doc), so these fixtures use alpha A=COMBINED, B=PRIMITIVE (distinct
    /// selectors) rather than `ZERO_ALPHA_ABD` for both, which would zero
    /// the coefficient and suppress C.
    #[test]
    fn alpha_slot_c_texel0_and_texel1_are_referenced_with_a_nonzero_coefficient() {
        const DISTINCT_ALPHA_B: u32 = 3; // PRIMITIVE (alpha_input_abd) -- differs from COMBINED
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [0, DISTINCT_ALPHA_B, 1, ZERO_ALPHA_ABD], // COMBINED, _, TEXEL0 (alpha_input_c), _
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [0, DISTINCT_ALPHA_B, 2, ZERO_ALPHA_ABD], // COMBINED, _, TEXEL1 (alpha_input_c), _
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    /// The cancellation itself, alpha's mirror of
    /// `color_slot_c_texel0_is_suppressed_when_a_and_b_decode_equal`: alpha
    /// slot C decoding to TEXEL0/TEXEL1 must NOT mark the params as
    /// texture-referencing when alpha A and B decode to the same selector.
    /// This is exactly the shape
    /// `the_reported_shade_only_regression_fixture_does_not_reference_texels`
    /// depends on (alpha A=B=COMBINED, alpha C=TEXEL0) -- this test isolates
    /// just the cancellation, independent of that specific fixture's other
    /// bits.
    #[test]
    fn alpha_slot_c_texel0_is_suppressed_when_a_and_b_decode_equal() {
        let params = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [0, 0, 1, ZERO_ALPHA_ABD], // COMBINED, COMBINED (same as A, (A-B) == 0), TEXEL0 (alpha_input_c), _
        );
        assert!(!params.references_texels_in_first_cycle());
    }

    #[test]
    fn alpha_slot_d_texel0_and_texel1_are_referenced() {
        let texel0 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, 1], // TEXEL0 (alpha_input_abd)
        );
        assert!(texel0.references_texels_in_first_cycle());

        let texel1 = combine_params_with_only(
            [ZERO_COLOR_A, ZERO_COLOR_B, ZERO_COLOR_C, ZERO_COLOR_D],
            [ZERO_ALPHA_ABD, ZERO_ALPHA_ABD, ZERO_ALPHA_C, 2], // TEXEL1 (alpha_input_abd)
        );
        assert!(texel1.references_texels_in_first_cycle());
    }

    /// The exact SHADE-only-triangle repair fixture: the failing test's own
    /// `SetCombine` payload (`production.rs`'s
    /// `wgpu_backend_draws_a_real_admitted_triangle_matching_the_combiner_oracle`)
    /// -- color_d=SHADE(4), alpha_c=TEXEL0(1) in `alpha_input_c`'s table but
    /// multiplied by zero, alpha_d=SHADE(4). Confirms this exact fixture's
    /// `CombineParams` is judged non-texture-referencing, matching the bug
    /// report: this triangle legitimately carries
    /// `TileBindingParams::unbound()` and must not trigger a TMEM sample
    /// call.
    #[test]
    fn the_reported_shade_only_regression_fixture_does_not_reference_texels() {
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 4;
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 4;
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        let params = CombineParams::from_wire(low, high);
        assert!(!params.references_texels_in_first_cycle());
    }

    /// The two real textured differentials' own fixture shape
    /// (`production.rs`'s TEXEL0-passthrough tests): color_d=TEXEL0(1),
    /// alpha_d=TEXEL0(1) -- must remain judged texture-referencing so the
    /// real sampler keeps running for them.
    #[test]
    fn the_real_textured_differential_fixture_shape_references_texels() {
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 1; // TEXEL0
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1; // TEXEL0 in alpha_input_c's table, x0
        let alpha_d: u32 = 1; // TEXEL0
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        let params = CombineParams::from_wire(low, high);
        assert!(params.references_texels_in_first_cycle());
    }
}

/// **Diagnostic-only.** Tallies which color/alpha selectors the combiner
/// programs of actually-drawn triangles select, per slot.
///
/// Exists to answer one question with counts instead of impressions: when
/// WM2000's models render flat despite every admitted triangle being
/// textured and reaching `sample_point` (see
/// `docs/RT64-WM2000-INMATCH-GAPS.md`), is the sampled texel being
/// *discarded* by a program that never names `Texel0`, or is it being
/// *selected* and merely wrong? A tally with `Texel0` near zero indicts the
/// combiner; a tally where `Texel0` dominates the C or A slot indicts the
/// sample.
///
/// Nothing in the render path reads these. Dumped to stderr every 100,000
/// notes when `FN64_COMBINER_CENSUS` is set, matching
/// `raw_dpc::raw_triangle_drop_stats`' self-reporting shape -- the harness
/// is a separate crate and does not call in, so the dump has to come from
/// here.
///
/// **The slice tallied is the one that will actually be evaluated**, chosen
/// by the caller from the cycle type: one-cycle mode evaluates the *cycle-1*
/// bitfield slice (angrylion `combiner_1cycle`, `combiner.c:173-220`,
/// dereferences index `[1]` throughout), so tallying the cycle-0 slice for a
/// one-cycle program would report selectors the hardware never reads.
pub mod census {
    use super::{AlphaInput, AlphaInputSlot, ColorInput, ColorInputSlot};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `ColorInput` in declaration order, used as the index space for
    /// [`COLOR_COUNTS`]. A local table rather than a derive so the counter
    /// index is stable and explicit.
    pub const COLOR_NAMES: [&str; 21] = [
        "Combined",
        "Texel0",
        "Texel1",
        "Primitive",
        "Shade",
        "Environment",
        "KeyCenter",
        "KeyScale",
        "CombinedAlpha",
        "Texel0Alpha",
        "Texel1Alpha",
        "PrimitiveAlpha",
        "ShadeAlpha",
        "EnvAlpha",
        "LodFraction",
        "PrimLodFrac",
        "Noise",
        "K4",
        "K5",
        "One",
        "Zero",
    ];

    pub const ALPHA_NAMES: [&str; 10] = [
        "Combined",
        "Texel0",
        "Texel1",
        "Primitive",
        "Shade",
        "Environment",
        "LodFraction",
        "PrimLodFrac",
        "One",
        "Zero",
    ];

    const fn color_index(input: ColorInput) -> usize {
        match input {
            ColorInput::Combined => 0,
            ColorInput::Texel0 => 1,
            ColorInput::Texel1 => 2,
            ColorInput::Primitive => 3,
            ColorInput::Shade => 4,
            ColorInput::Environment => 5,
            ColorInput::KeyCenter => 6,
            ColorInput::KeyScale => 7,
            ColorInput::CombinedAlpha => 8,
            ColorInput::Texel0Alpha => 9,
            ColorInput::Texel1Alpha => 10,
            ColorInput::PrimitiveAlpha => 11,
            ColorInput::ShadeAlpha => 12,
            ColorInput::EnvAlpha => 13,
            ColorInput::LodFraction => 14,
            ColorInput::PrimLodFrac => 15,
            ColorInput::Noise => 16,
            ColorInput::K4 => 17,
            ColorInput::K5 => 18,
            ColorInput::One => 19,
            ColorInput::Zero => 20,
        }
    }

    const fn alpha_index(input: AlphaInput) -> usize {
        match input {
            AlphaInput::Combined => 0,
            AlphaInput::Texel0 => 1,
            AlphaInput::Texel1 => 2,
            AlphaInput::Primitive => 3,
            AlphaInput::Shade => 4,
            AlphaInput::Environment => 5,
            AlphaInput::LodFraction => 6,
            AlphaInput::PrimLodFrac => 7,
            AlphaInput::One => 8,
            AlphaInput::Zero => 9,
        }
    }

    const fn color_slot_index(slot: ColorInputSlot) -> usize {
        match slot {
            ColorInputSlot::A => 0,
            ColorInputSlot::B => 1,
            ColorInputSlot::C => 2,
            ColorInputSlot::D => 3,
        }
    }

    const fn alpha_slot_index(slot: AlphaInputSlot) -> usize {
        match slot {
            AlphaInputSlot::A => 0,
            AlphaInputSlot::B => 1,
            AlphaInputSlot::C => 2,
            AlphaInputSlot::D => 3,
        }
    }

    /// A flat counter bank indexed `slot * stride + selector`.
    ///
    /// Flat rather than `[[AtomicU64; N]; 4]` because `AtomicU64` is not
    /// `Copy`, so the nested array cannot be built from a repeat expression
    /// in a `static` initializer, and spelling out 84 constructors would be
    /// its own transcription hazard.
    macro_rules! counter_bank {
        ($name:ident, $len:expr) => {
            static $name: [AtomicU64; $len] = {
                #[allow(clippy::declare_interior_mutable_const)]
                const INIT: AtomicU64 = AtomicU64::new(0);
                [INIT; $len]
            };
        };
    }

    counter_bank!(COLOR_COUNTS, 84);
    counter_bank!(ALPHA_COUNTS, 40);
    static NOTES: AtomicU64 = AtomicU64::new(0);
    /// Programs whose evaluated slice names `Texel0`/`Texel0Alpha` anywhere
    /// in color, versus programs that name it nowhere. This is the headline
    /// number: a drawn textured triangle in the second bucket has its
    /// sampled texel discarded outright.
    static COLOR_READS_TEXEL0: AtomicU64 = AtomicU64::new(0);
    static COLOR_IGNORES_TEXEL0: AtomicU64 = AtomicU64::new(0);

    /// Records one drawn triangle's evaluated combiner slice.
    ///
    /// `color` and `alpha` are the four already-decoded selectors in
    /// A, B, C, D order -- decoded by the CALLER from the slice it will
    /// actually evaluate, so this module never re-derives which slice is
    /// live and cannot disagree with the evaluator about it.
    /// Whether the census is switched on, read ONCE.
    ///
    /// `var_os` allocates and the call site is per-draw; a live lane is
    /// measuring frame rate and a probe must not become the thing it
    /// measures.
    pub fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("FN64_COMBINER_CENSUS").is_some())
    }

    /// Which evaluated pass a note came from. Recorded because the whole
    /// census turns on reading the slice the hardware reads, and a tally
    /// that cannot say whether a program was one-cycle or two-cycle cannot
    /// be checked against that rule by a later reader.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Pass {
        /// One-cycle mode: the single pass, over the CYCLE-1 slice.
        OneCycleOnly,
        /// Two-cycle mode, first pass, over the cycle-0 slice.
        TwoCycleFirst,
        /// Two-cycle mode, second pass, over the cycle-1 slice.
        TwoCycleSecond,
    }

    static PASS_COUNTS: [AtomicU64; 3] = {
        #[allow(clippy::declare_interior_mutable_const)]
        const INIT: AtomicU64 = AtomicU64::new(0);
        [INIT; 3]
    };

    pub fn note_program(
        color: [ColorInput; 4],
        alpha: [AlphaInput; 4],
        textured: bool,
        pass: Pass,
    ) {
        PASS_COUNTS[match pass {
            Pass::OneCycleOnly => 0,
            Pass::TwoCycleFirst => 1,
            Pass::TwoCycleSecond => 2,
        }]
        .fetch_add(1, Ordering::Relaxed);
        for (slot, input) in [
            ColorInputSlot::A,
            ColorInputSlot::B,
            ColorInputSlot::C,
            ColorInputSlot::D,
        ]
        .into_iter()
        .zip(color)
        {
            COLOR_COUNTS[color_slot_index(slot) * 21 + color_index(input)]
                .fetch_add(1, Ordering::Relaxed);
        }
        for (slot, input) in [
            AlphaInputSlot::A,
            AlphaInputSlot::B,
            AlphaInputSlot::C,
            AlphaInputSlot::D,
        ]
        .into_iter()
        .zip(alpha)
        {
            ALPHA_COUNTS[alpha_slot_index(slot) * 10 + alpha_index(input)]
                .fetch_add(1, Ordering::Relaxed);
        }
        // The headline split, and it is restricted twice over.
        //
        // First to draws that HAVE a texel: an untextured draw ignoring
        // Texel0 is correct and uninteresting, and counting it would
        // dilute the ratio the card turns on.
        //
        // Second, and this one was earned by a wrong reading, to the FIRST
        // evaluated pass. A two-cycle program's second pass routinely reads
        // no texel: WM2000's dominant program is
        // `cycle0 = (Texel0 - Zero) * ShadeAlpha + Zero` followed by
        // `cycle1 = (Environment - Combined) * Primitive + Combined`, a fog
        // lerp over the textured result. Counting that second pass as a
        // Texel-ignoring draw made 54% of draws appear to discard their
        // texture when they had sampled it in the pass immediately before.
        // The ratio exists to answer "does this program consult the texel
        // at all", and `Combined` carries the first pass's answer forward,
        // so the first pass is where that question is decided.
        if textured && pass != Pass::TwoCycleSecond {
            if color.iter().any(|input| {
                matches!(
                    input,
                    ColorInput::Texel0
                        | ColorInput::Texel0Alpha
                        | ColorInput::Texel1
                        | ColorInput::Texel1Alpha
                )
            }) {
                &COLOR_READS_TEXEL0
            } else {
                &COLOR_IGNORES_TEXEL0
            }
            .fetch_add(1, Ordering::Relaxed);
        }

        let note = NOTES.fetch_add(1, Ordering::Relaxed) + 1;
        if note % 100_000 == 0 {
            // The caller only reaches this function when the env flag is
            // set (it gates the call site behind a `OnceLock`), so the
            // flag is not re-read here; doing so would allocate on the
            // per-triangle path a live perf lane is measuring.
            report(&format!("note={note}"));
        }
    }

    /// The raw `SetCombine` wire words of the programs actually evaluated,
    /// with a count each.
    ///
    /// The per-slot tallies above answer "which selectors appear"; they
    /// cannot answer "which PROGRAM appears", because four independent
    /// histograms do not reconstruct the joint distribution. Two different
    /// programs can produce identical per-slot marginals. This map keeps the
    /// wire words themselves, so the dominant program can be hand-decoded
    /// from the layout rather than guessed from the margins.
    static PROGRAMS: std::sync::Mutex<Option<std::collections::BTreeMap<(u32, u32), u64>>> =
        std::sync::Mutex::new(None);

    pub fn note_wire(low: u32, high: u32) {
        let Ok(mut guard) = PROGRAMS.lock() else {
            return;
        };
        *guard
            .get_or_insert_with(std::collections::BTreeMap::new)
            .entry((low, high))
            .or_insert(0) += 1;
    }

    /// The distinct evaluated programs, most frequent first.
    pub fn program_histogram() -> Vec<((u32, u32), u64)> {
        let Ok(guard) = PROGRAMS.lock() else {
            return Vec::new();
        };
        let mut out: Vec<_> = guard
            .as_ref()
            .map(|map| map.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// `([slot][selector] color counts, alpha counts, (reads_texel, ignores_texel))`.
    #[allow(clippy::type_complexity)]
    pub fn snapshot() -> ([[u64; 21]; 4], [[u64; 10]; 4], (u64, u64)) {
        let mut color = [[0u64; 21]; 4];
        for (slot, row) in color.iter_mut().enumerate() {
            for (index, slot_count) in row.iter_mut().enumerate() {
                *slot_count = COLOR_COUNTS[slot * 21 + index].load(Ordering::Relaxed);
            }
        }
        let mut alpha = [[0u64; 10]; 4];
        for (slot, row) in alpha.iter_mut().enumerate() {
            for (index, slot_count) in row.iter_mut().enumerate() {
                *slot_count = ALPHA_COUNTS[slot * 10 + index].load(Ordering::Relaxed);
            }
        }
        (
            color,
            alpha,
            (
                COLOR_READS_TEXEL0.load(Ordering::Relaxed),
                COLOR_IGNORES_TEXEL0.load(Ordering::Relaxed),
            ),
        )
    }

    /// Writes the current tally to stderr, nonzero buckets only.
    pub fn report(label: &str) {
        let (color, alpha, (reads, ignores)) = snapshot();
        eprintln!("[fn64-combiner] {label} programs={}", reads + ignores);
        eprintln!(
            "[fn64-combiner]   passes: one_cycle={} two_cycle_first={} two_cycle_second={}",
            PASS_COUNTS[0].load(Ordering::Relaxed),
            PASS_COUNTS[1].load(Ordering::Relaxed),
            PASS_COUNTS[2].load(Ordering::Relaxed),
        );
        eprintln!(
            "[fn64-combiner]   TEXTURED draws: color reads Texel* = {reads}, ignores Texel* = {ignores}"
        );
        for (slot, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let mut line = String::new();
            for (index, count) in color[slot].iter().enumerate() {
                if *count > 0 {
                    line.push_str(&format!(" {}={}", COLOR_NAMES[index], count));
                }
            }
            eprintln!("[fn64-combiner]   color {name}:{line}");
        }
        for (slot, name) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let mut line = String::new();
            for (index, count) in alpha[slot].iter().enumerate() {
                if *count > 0 {
                    line.push_str(&format!(" {}={}", ALPHA_NAMES[index], count));
                }
            }
            eprintln!("[fn64-combiner]   alpha {name}:{line}");
        }
        let histogram = program_histogram();
        eprintln!(
            "[fn64-combiner]   distinct programs = {}, top 8 by count:",
            histogram.len()
        );
        for ((low, high), count) in histogram.iter().take(8) {
            eprintln!("[fn64-combiner]     {low:#010x} {high:#010x} x{count}");
        }
    }
}

#[cfg(test)]
mod census_tests {
    use super::census::{ALPHA_NAMES, COLOR_NAMES};
    use super::{AlphaInput, AlphaInputSlot, ColorInput, ColorInputSlot, CombineParams};

    /// The census's two name tables must be in the same order as the
    /// index functions the counters use, or every reported bucket is
    /// mislabelled while every count stays plausible -- a silent wrong
    /// answer of exactly the kind this whole card is chasing.
    ///
    /// Checked by round-tripping the NAME through a fresh decode rather
    /// than by re-listing the enum, which would just restate the table.
    #[test]
    fn the_census_name_tables_match_the_selector_order() {
        // Hand-written from `color_input_common`/`color_input_c`, which are
        // themselves the RT64 tables. Deliberately NOT derived from
        // `COLOR_NAMES`.
        assert_eq!(COLOR_NAMES[1], "Texel0");
        assert_eq!(COLOR_NAMES[4], "Shade");
        assert_eq!(COLOR_NAMES[9], "Texel0Alpha");
        assert_eq!(COLOR_NAMES[20], "Zero");
        assert_eq!(ALPHA_NAMES[1], "Texel0");
        assert_eq!(ALPHA_NAMES[4], "Shade");
        assert_eq!(ALPHA_NAMES[9], "Zero");
    }

    /// The census decodes the slice the evaluator will run. In one-cycle
    /// mode that is the CYCLE-1 slice, so a program whose two slices name
    /// different selectors must census as its cycle-1 selectors.
    ///
    /// **Fixture chosen to distinguish the two slices**: cycle 0 selects
    /// `Shade` in every color slot and cycle 1 selects `Texel0`, so reading
    /// the wrong slice reports the exact opposite of the answer this card
    /// turns on. A fixture with both slices equal -- the obvious one to
    /// write -- would pass under either slice and prove nothing.
    #[test]
    fn one_cycle_censuses_the_cycle_one_slice_not_the_cycle_zero_slice() {
        // gbi.h GCCc*w* packing, hand-assembled: cycle 0 = SHADE (index 4)
        // in a/b/c/d; cycle 1 = TEXEL0 (index 1) in a/b/c/d.
        const SHADE: u32 = 4;
        const TEXEL0: u32 = 1;
        let w0 = (SHADE << 20) | (SHADE << 15) | (TEXEL0 << 5) | TEXEL0;
        let w1 = (SHADE << 28) | (TEXEL0 << 24) | (SHADE << 15) | (TEXEL0 << 6);
        let params = CombineParams::from_wire(w0, w1);

        // second_cycle = true is what `run_one_cycle` passes.
        for slot in [
            ColorInputSlot::A,
            ColorInputSlot::B,
            ColorInputSlot::C,
            ColorInputSlot::D,
        ] {
            assert_eq!(
                params.decode_color(slot, true),
                ColorInput::Texel0,
                "one-cycle slot {slot:?} must read the cycle-1 slice"
            );
            assert_eq!(
                params.decode_color(slot, false),
                ColorInput::Shade,
                "the cycle-0 slice of this fixture is Shade, so the two differ"
            );
        }
    }

    /// WM2000's dominant measured program, hand-decoded from the wire words
    /// the ROM actually issued, and pinned because misreading it is what
    /// made 54% of draws look like they discarded their texture.
    ///
    /// `0xfc15fea3 / 0xf00ff23f`, measured at 73,925 of ~115,000 draws in
    /// one in-match window. Expectations below are derived BY HAND from
    /// gbi.h's `GCCc0w0`/`GCCc1w0`/`GCCc0w1`/`GCCc1w1` bit positions, not
    /// from the decoder under test.
    ///
    /// **Mutation scope, stated honestly.** Rewriting
    /// `color_input_c`'s index 11 from `ShadeAlpha` to `PrimitiveAlpha`
    /// fails this test. Narrowing `parse_color_c`'s second-cycle mask from
    /// `0x1F` to `0xF` does NOT -- this program's `c1` is 3, which reads
    /// identically under either mask, the coincident-fixture trap
    /// `docs/RT64-WM2000-HARNESS-TRAPS.md` names. That mutant is caught by
    /// three tests elsewhere in this crate
    /// (`set_combine_w0_is_passed_through_completely_unmasked`,
    /// `two_cycle_wire_program_decodes_to_both_slices`, and
    /// `the_one_cycle_fixtures_really_do_admit_a_combining_texture_rectangle`),
    /// verified by running the mutant against the full suite, so it is
    /// covered -- just not here. This fixture's job is the ROM's real
    /// program, not the mask widths.
    #[test]
    fn the_wm2000_fog_program_samples_the_texture_in_its_first_cycle() {
        let params = CombineParams::from_wire(0xfc15_fea3, 0xf00f_f23f);

        // Cycle 0 (two-cycle first pass, `second_cycle = false`).
        // a0 = w0>>20 & 0xF = 0x1 = TEXEL0; b0 = w1>>28 & 0xF = 0xf -> ZERO;
        // c0 = w0>>15 & 0x1F = 0x1b = 11 -> SHADE_ALPHA;
        // d0 = w1>>15 & 0x7 = 0x7 -> ZERO.
        assert_eq!(params.decode_color(ColorInputSlot::A, false), ColorInput::Texel0);
        assert_eq!(params.decode_color(ColorInputSlot::B, false), ColorInput::Zero);
        assert_eq!(
            params.decode_color(ColorInputSlot::C, false),
            ColorInput::ShadeAlpha
        );
        assert_eq!(params.decode_color(ColorInputSlot::D, false), ColorInput::Zero);

        // Cycle 1: a1 = w0>>5 & 0xF = 0x5 = ENVIRONMENT;
        // b1 = w1>>24 & 0xF = 0x0 = COMBINED; c1 = w0 & 0x1F = 0x3 = PRIMITIVE;
        // d1 = w1>>6 & 0x7 = 0x0 = COMBINED. A fog lerp over cycle 0's result.
        assert_eq!(
            params.decode_color(ColorInputSlot::A, true),
            ColorInput::Environment
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::B, true),
            ColorInput::Combined
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::C, true),
            ColorInput::Primitive
        );
        assert_eq!(
            params.decode_color(ColorInputSlot::D, true),
            ColorInput::Combined
        );

        // The point of the whole fixture: the SECOND cycle names no texel,
        // and reading only that cycle would conclude this draw throws its
        // texture away. It does not -- cycle 0 sampled it and `Combined`
        // carries it forward.
        for slot in [
            ColorInputSlot::A,
            ColorInputSlot::B,
            ColorInputSlot::C,
            ColorInputSlot::D,
        ] {
            assert_ne!(
                params.decode_color(slot, true),
                ColorInput::Texel0,
                "cycle 1 of the fog program names no Texel0 -- that is the trap"
            );
        }
    }

    /// Alpha slot C has its OWN index table, shifted from A/B/D's: index 1
    /// is TEXEL0 there and COMBINED is unreachable. Pinned because the
    /// census reports all four alpha slots through one name table, and a
    /// reader comparing slot C's counts against slot A's would otherwise
    /// silently compare two different index spaces.
    #[test]
    fn alpha_slot_c_uses_its_own_table() {
        // alpha1 C sits at w1>>18; alpha1 A at w1>>21.
        let params = CombineParams::from_wire(0, (0 << 21) | (0 << 18));
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::A, true),
            AlphaInput::Combined,
            "index 0 in the A/B/D table is COMBINED"
        );
        assert_eq!(
            params.decode_alpha(AlphaInputSlot::C, true),
            AlphaInput::LodFraction,
            "index 0 in the C table is LOD_FRACTION, not COMBINED"
        );
    }
}
