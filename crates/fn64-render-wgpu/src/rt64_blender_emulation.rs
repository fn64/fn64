//! Blender CPU-side **emulation-requirements** classifier (RT64 port ticket
//! M4.5): `EmulationRequirements`, `checkEmulationRequirements`, and the
//! `Approximation` enum's two named patterns.
//!
//! Literal port of `Blender::EmulationRequirements` and
//! `Blender::checkEmulationRequirements` from
//! `src/shared/rt64_blender.h:178-271` (pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, per
//! `docs/RT64-PORT-AUTHORITY.md`). Source digest (sha256, verbatim from
//! `docs/rt64-port-inventory.json`'s `files[].sources.port.sha256` for
//! `src/shared/rt64_blender.h`, independently reproduced with
//! `shasum -a 256` against the pinned checkout):
//! `0520bbe032eea7f8791833c25a8b61c03b184174a97bfc77e52e45e1ce28d4b6`
//!
//! ```text
//! struct EmulationRequirements {
//!     struct Cycle {
//!         // If either input is zero, overflow isn't possible. The cycle is merely a passthrough of whatever input isn't zero.
//!         // Even in the case one of the inputs is the framebuffer in the first cycle, it merely replaces whatever is on the second cycle.
//!         bool passthrough;
//!
//!         // Numerator overflow is possible if the cycle doesn't use 1MA in the second input. These require overflow
//!         // and normalization emulation, which is only impossible to emulate natively if at least one of the inputs is the framebuffer color.
//!         bool numeratorOverflow;
//!
//!         // If this is a multi-cycle blender and the framebuffer is used in the first cycle, simple emulation can't be used.
//!         // However, there may be cases where the second cycle doesn't act in a way that makes it impossible to emulate natively.
//!         bool framebufferColor;
//!     };
//!
//!     Cycle cycles[2];
//!     bool simpleEmulation;
//!     Approximation approximateEmulation;
//! };
//!
//! static EmulationRequirements checkEmulationRequirements(const OtherMode otherMode) {
//!     EmulationRequirements reqs = { };
//!
//!     // Check the cycles for the emulation requirements.
//!     const uint blenderInputs = otherMode.blenderInputs();
//!     const uint blenderCycleCount = Blender::blendCycleCount(otherMode);
//!     for (uint c = 0; c < blenderCycleCount; c++) {
//!         const bool secondCycle = (c > 0);
//!         const InputPM P = decodeInputP(blenderInputs, secondCycle);
//!         const InputPM M = decodeInputM(blenderInputs, secondCycle);
//!         const InputA A = decodeInputA(blenderInputs, secondCycle);
//!         const InputB B = decodeInputB(blenderInputs, secondCycle);
//!         const bool anyInputIsZero = (A == A_ZERO) || (B == B_ZERO);
//!         const bool duplicateInput1MA = (P == M) && (B == B_ONE_MINUS_A);
//!         if (anyInputIsZero || duplicateInput1MA) {
//!             reqs.cycles[c].passthrough = true;
//!         }
//!         else if (B != B_ONE_MINUS_A) {
//!             reqs.cycles[c].numeratorOverflow = true;
//!         }
//!
//!         if ((P == PM_FRAMEBUFFER_COLOR) || (M == PM_FRAMEBUFFER_COLOR)) {
//!             reqs.cycles[c].framebufferColor = true;
//!         }
//!     }
//!
//!     // Assume by default simple emulation is possible.
//!     reqs.simpleEmulation = true;
//!
//!     // First cycle relies on numerator overflow and uses the framebuffer color.
//!     if (reqs.cycles[0].numeratorOverflow && reqs.cycles[0].framebufferColor) {
//!         reqs.simpleEmulation = false;
//!     }
//!     // Check for two-cycle cases.
//!     else if (blenderCycleCount == 2) {
//!         // First cycle uses framebuffer color and it's not a simple passthrough.
//!         if (reqs.cycles[0].framebufferColor && !reqs.cycles[0].passthrough) {
//!             reqs.simpleEmulation = false;
//!         }
//!         // Second cycle relies on numerator overflow and uses the framebuffer color.
//!         else if (reqs.cycles[1].numeratorOverflow && reqs.cycles[1].framebufferColor) {
//!             reqs.simpleEmulation = false;
//!         }
//!     }
//!
//!     // Search for approximations if simple emulation isn't capable.
//!     if (!reqs.simpleEmulation) {
//!         if (blenderCycleCount == 2) {
//!             const InputPM P0 = decodeInputP(blenderInputs, false);
//!             const InputPM M0 = decodeInputM(blenderInputs, false);
//!             const InputA A0 = decodeInputA(blenderInputs, false);
//!             const InputB B0 = decodeInputB(blenderInputs, false);
//!             const InputPM P1 = decodeInputP(blenderInputs, true);
//!             const InputPM M1 = decodeInputM(blenderInputs, true);
//!             const InputA A1 = decodeInputA(blenderInputs, true);
//!             const InputB B1 = decodeInputB(blenderInputs, true);
//!             if ((P0 == PM_CC_OR_BLENDER) && (M0 == PM_FRAMEBUFFER_COLOR) && (A0 == A_CC_ALPHA) && (B0 == B_ONE_MINUS_A) &&
//!                 (P1 == PM_CC_OR_BLENDER) && (M1 == PM_FRAMEBUFFER_COLOR) && (A1 == A_CC_ALPHA) && (B1 == B_ONE_MINUS_A))
//!             {
//!                 reqs.approximateEmulation = Approximation::CombinerFramebuffer1MA_SquareMix;
//!             }
//!             else if ((P0 != PM_FRAMEBUFFER_COLOR) && (M0 == PM_FRAMEBUFFER_COLOR) && (B0 == B_ONE_MINUS_A) &&
//!                      (P1 == PM_CC_OR_BLENDER) && (M1 == PM_FRAMEBUFFER_COLOR) && (B1 == B_ONE_MINUS_A))
//!             {
//!                 reqs.approximateEmulation = Approximation::AnyFramebuffer1MA_MultiplyMix;
//!             }
//!             else {
//!                 reqs.approximateEmulation = Approximation::None;
//!             }
//!         }
//!     }
//!
//!     return reqs;
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! `crates/fn64-render-wgpu/src/rt64_blender_analysis.rs` (M4.4) already
//! ports this same header's `combineCycleCount`/`blendCycleCount` and the
//! `decodeInputP/M/A/B` selector decode (there exposed via its
//! `cycle_selectors` helper over `crate::blend::ResolvedBlendCycle`). This
//! module calls that sibling's [`crate::rt64_blender_analysis::blend_cycle_count`]
//! directly for `blenderCycleCount` rather than redefining a second copy, and
//! reuses `crate::blend`'s already-verified `BlendColorInput`/
//! `BlendAlphaInput`/`BlendBInput` enums (the header's `InputPM`/`InputA`/
//! `InputB`) plus `crate::state::OtherMode::blender_cycle_1()`/
//! `blender_cycle_2()` (the header's `decodeInputP/M/A/B(blenderInputs,
//! secondCycle)`, proved equivalent by M4.4's shift-arithmetic derivation) --
//! see M4.4's own doc header for that proof; it is not re-derived here.
//! `crates/fn64-render-wgpu/src/blend.rs` separately ports the runtime blend
//! **evaluation** half of this header (`Blender::runCycle`/`Blender::run`,
//! lines 366-504); neither this module nor `rt64_blender_analysis.rs` calls
//! into it, and it does not call into either of them.
//!
//! This module adds exactly one new type cluster the two siblings do not
//! define: [`Cycle`], [`EmulationRequirements`], and [`Approximation`] --
//! the header's `EmulationRequirements`/`EmulationRequirements::Cycle`/
//! `Blender::Approximation`, none of which either sibling ports.
//!
//! ## Admitted domain
//!
//! - **`EmulationRequirements reqs = { };` is a full C++ aggregate
//!   zero-initialization**, not merely "the two touched cycle slots start
//!   zeroed". Both `cycles[0]` and `cycles[1]` start as
//!   `{ passthrough: false, numeratorOverflow: false, framebufferColor:
//!   false }`; `simpleEmulation` starts `false` (immediately and
//!   unconditionally overwritten to `true` right after the loop, so this
//!   zero only matters if a reader assumes it means "no simple emulation" --
//!   it doesn't survive to the return); `approximateEmulation` starts
//!   `Approximation::None` because a C++ scoped enum's value-initialization
//!   is the zero bit pattern, and `Approximation::None` is declared first
//!   (ordinal 0). [`EmulationRequirements::default`] reproduces this exact
//!   all-zero/`Approximation::None` state, and
//!   [`check_emulation_requirements`] starts from `EmulationRequirements::default()`
//!   rather than constructing each field inline, matching the header's own
//!   `reqs = { }` shape.
//! - **The loop only visits `cycles[0..blenderCycleCount]`; an unvisited
//!   slot keeps its zero-init value, it is not computed as if empty
//!   selectors were decoded.** For `blenderCycleCount == 0` neither slot is
//!   touched; for `blenderCycleCount == 1` only `cycles[0]` is touched and
//!   `cycles[1]` stays all-`false`; only `blenderCycleCount == 2` touches
//!   both. [`check_emulation_requirements`] loops `for c in
//!   0..blender_cycle_count` exactly as the header does (not `0..2`), pinned
//!   by `cycles_1_stays_zero_when_blender_cycle_count_is_0_or_1`.
//! - **Per-cycle `passthrough`/`numeratorOverflow` are `if`/`else if` --
//!   mutually exclusive by construction -- while `framebufferColor` is a
//!   wholly separate, unconditional `if` evaluated every iteration
//!   regardless of which (if either) of the first two fired.** A cycle can
//!   therefore be `framebufferColor: true` together with `passthrough:
//!   true`, together with `numeratorOverflow: true`, or with neither of the
//!   first two set (when `!anyInputIsZero && !duplicateInput1MA && B ==
//!   B_ONE_MINUS_A`, i.e. proper single-cycle 1MA with no zero input -- a
//!   real, reachable third per-cycle state where all three `Cycle` bools are
//!   `false`). [`check_emulation_requirements`] preserves this exact
//!   `if { .. } else if { .. }` then separate `if { .. }` shape rather than
//!   merging into one `match`. Pinned by
//!   `cycle_all_three_bools_false_when_1ma_and_no_zero_input`,
//!   `cycle_passthrough_and_framebuffer_color_can_both_be_true`, and
//!   `cycle_numerator_overflow_and_framebuffer_color_can_both_be_true`.
//! - **`anyInputIsZero` compares `A`/`B` only** (never `P`/`M`); `A ==
//!   A_ZERO` uses the `InputA` zero variant, `B == B_ZERO` the *distinct*
//!   `InputB` zero variant -- these are different enums with independently
//!   assigned ordinal 3, not a shared "zero" tag. `duplicateInput1MA`
//!   requires **both** `P == M` (same `InputPM` value in both slots) **and**
//!   `B == B_ONE_MINUS_A`; `A` is irrelevant to this check. Ported as
//!   `cycle.a == BlendAlphaInput::Zero || cycle.b == BlendBInput::Zero` and
//!   `cycle.p == cycle.m && cycle.b == BlendBInput::OneMinusA` respectively,
//!   preserving which fields each predicate reads.
//! - **`simpleEmulation`'s decision tree is order-sensitive and its first
//!   branch is NOT gated on `blenderCycleCount == 2`.** The cycle-0
//!   `numeratorOverflow && framebufferColor` check runs unconditionally --
//!   including for a one-cycle mode, where it is both reachable and
//!   sufficient to set `simpleEmulation = false` on its own -- and only its
//!   *failure* falls through to the `else if (blenderCycleCount == 2)`
//!   two-cycle-only sub-tree (whose own two branches are themselves
//!   `if`/`else if`, so a cycle-0-framebuffer-non-passthrough match
//!   short-circuits the cycle-1 check rather than both being evaluated).
//!   [`check_emulation_requirements`] preserves this exact three-level
//!   `if` / `else if (blenderCycleCount == 2) { if / else if }` shape --
//!   never restructured into a flat OR of all four conditions, which would
//!   change nothing about which cycle-1 fields get *read* (cycle 1 is never
//!   touched for a one-cycle mode) but would misrepresent the source's
//!   actual branch order. Pinned by
//!   `simple_emulation_false_from_cycle0_alone_even_in_one_cycle_mode`
//!   (the asymmetric-branch hazard: cycle-0's check does NOT require
//!   `blenderCycleCount == 2`) and
//!   `simple_emulation_two_cycle_short_circuits_before_checking_cycle1`.
//! - **The `Approximation` search is gated on `!simpleEmulation` first,
//!   and *inside* that, separately gated on `blenderCycleCount == 2`.**
//!   When `!simpleEmulation` is true but `blenderCycleCount != 2` (i.e. the
//!   one-cycle path reached `simpleEmulation = false` purely via the
//!   cycle-0-alone check above), the header's inner `if (blenderCycleCount
//!   == 2)` body never runs and `reqs.approximateEmulation` is never
//!   assigned -- it keeps its zero-initialized `Approximation::None` from
//!   `reqs = { }`, via the *default*, not via evaluating the two-cycle
//!   pattern match's own `else` arm. [`check_emulation_requirements`]
//!   preserves this by leaving `approximate_emulation` at
//!   `Approximation::default()` when `blender_cycle_count != 2`, matching
//!   the header's silent no-op rather than adding a synthesized `else`.
//!   Pinned by
//!   `approximation_none_via_default_when_not_simple_and_one_cycle`
//!   (one-cycle, `!simpleEmulation`, confirms the value equals the untouched
//!   default) and `approximation_stays_none_when_simple_emulation_is_true`
//!   (two-cycle, `simpleEmulation == true`, the whole approximation block is
//!   skipped).
//! - **The two `Approximation` patterns are exact 8-selector equality (or,
//!   for one field, inequality) patterns over the *raw* `blenderInputs`
//!   word, decoded fresh via `decodeInputP/M/A/B(.., false/true)` --
//!   independently of `reqs.cycles[]`, not derived from the already-computed
//!   `Cycle` bools.** [`check_emulation_requirements`] decodes `p0/m0/a0/b0`
//!   and `p1/m1/a1/b1` via [`crate::rt64_blender_analysis`]'s
//!   `cycle_selectors`-equivalent (`ResolvedBlendCycle::from_wire` over
//!   `blender_cycle_1()`/`blender_cycle_2()`) rather than reading `reqs`.
//! - **The two patterns are asymmetric by construction, not by oversight --
//!   ported literally, not normalized into a mirror-image pair.**
//!   `CombinerFramebuffer1MA_SquareMix` constrains all eight selectors with
//!   `==`, including `A0 == A_CC_ALPHA` and `A1 == A_CC_ALPHA`.
//!   `AnyFramebuffer1MA_MultiplyMix` constrains only six of the eight: `A0`
//!   and `A1` are never read at all (not merely unconstrained-but-checked --
//!   the header's condition never mentions `A0`/`A1`), and its first
//!   selector comparison is the header's only **inequality** in either
//!   pattern (`P0 != PM_FRAMEBUFFER_COLOR`, not `P0 == `something). Ported
//!   as two independent boolean expressions matching this shape exactly:
//!   [`is_combiner_framebuffer_1ma_square_mix`] takes and compares all of
//!   `p0/m0/a0/b0/p1/m1/a1/b1`; [`is_any_framebuffer_1ma_multiply_mix`]
//!   takes only `p0/m0/b0/p1/m1/b1` (no `a0`/`a1` parameters at all) and its
//!   first comparison is `p0 != BlendColorInput::Framebuffer`. Pinned by
//!   `any_framebuffer_pattern_ignores_a0_and_a1_entirely` (varies `A0`/`A1`
//!   across all four `BlendAlphaInput` values while holding the other six
//!   selectors at the matching pattern and asserts the match is unaffected)
//!   and `square_mix_pattern_requires_exact_a0_and_a1_cc_alpha` (the
//!   contrasting case: varying `A0` away from `A_CC_ALPHA` breaks the square
//!   pattern's match, proving the asymmetry is real and not a missed
//!   constraint in the multiply-mix port).
//! - **`if/else if/else` among the two patterns and the explicit `None`
//!   arm.** A two-cycle, non-simple mode that matches neither named pattern
//!   takes the explicit `else { approximateEmulation = Approximation::None;
//!   }` arm -- observably identical to the default in this module's typed
//!   representation, but [`check_emulation_requirements`] still evaluates
//!   both pattern predicates and assigns `Approximation::None` on that arm
//!   explicitly (rather than "falling through" by construction) to keep the
//!   three-way branch structure literal. `CombinerFramebuffer1MA_SquareMix`
//!   is checked before `AnyFramebuffer1MA_MultiplyMix`; a hand-checked input
//!   was found that cannot satisfy both simultaneously (the square pattern
//!   requires `P0 == PM_CC_OR_BLENDER`, the multiply pattern requires `P0 !=
//!   PM_FRAMEBUFFER_COLOR` while leaving `P0` otherwise free -- so both
//!   *could* in principle match the same input if `P0 == PM_CC_OR_BLENDER`
//!   and every other multiply-mix field also holds with `A0/A1 ==
//!   A_CC_ALPHA` by coincidence), so order still matters and is preserved:
//!   `is_combiner_framebuffer_1ma_square_mix` is checked strictly first.
//!   Pinned by `square_mix_pattern_is_checked_before_multiply_mix_pattern`.
//! - **Enum-ordinal handling.** `InputPM`/`InputA`/`InputB` remain the same
//!   totally-defined 2-bit wire fields M4.4 already documented (every 2-bit
//!   pattern maps to a defined variant; no reserved encoding). This module's
//!   own out-of-range surface is [`Approximation`]'s implicit
//!   zero-initialized default: `Approximation` here has no wire decode of
//!   its own (RT64 never serializes it over the wire; it is a host-only
//!   classification result), so there is no out-of-range *input* ordinal to
//!   reject -- the only "ordinal" behavior this module owns is that
//!   `Approximation::default()` is exactly `Approximation::None`
//!   (ordinal 0), matching C++ zero-initialization of a scoped enum whose
//!   first declared member has value 0. Pinned by
//!   `approximation_default_is_none_ordinal_zero`.
//!
//! ## Nonclaims
//!
//! No GPU execution, resource binding, draw-call, or emulation-strategy
//! selection wiring -- these are pure CPU predicates over
//! [`crate::state::OtherMode`] only, matching the ticket's "unwired CPU
//! classifier; no parity claim". Nothing here is called from production
//! code (this module is `mod`-declared in `lib.rs` but never `pub use`'d or
//! referenced outside its own tests). No claim of pixel, timing, or
//! full-ROM parity with RT64; no claim about which real N64 titles exercise
//! which branch or approximation pattern; no claim about which downstream
//! GPU shader or fallback path (if any) a future ticket wires
//! `EmulationRequirements` into.
//!
//! Partial coverage of the header, disjoint from both existing siblings:
//! this module ports only `EmulationRequirements`/
//! `EmulationRequirements::Cycle`/`checkEmulationRequirements`/
//! `Approximation` (lines 178-271, the ticket's named scope). It does
//! **not** port `combineCycleCount`/`blendCycleCount`/`usesInput`/
//! `usesCombinerAlpha`/`usesAlphaBlendCycle`/`usesAlphaBlend`/
//! `usesStandardFogCycle`/`usesVisualizeCoverageCycle` (lines 45-175,
//! `rt64_blender_analysis.rs`/M4.4's scope -- this module calls into that
//! sibling's `blend_cycle_count` rather than redefining it), `inputToString`
//! (lines 273-316, debug-only string formatting, not analysis, not named by
//! this ticket), or `fromInputPM`/`fromInputA`/`fromInputB`/`runCycle`/`run`
//! (lines 324-505, the runtime evaluation half `blend.rs` already owns).
//! `docs/rt64-port-inventory.json`'s `port_state` for
//! `src/shared/rt64_blender.h` may still read a state that does not name
//! this module after this change lands (the inventory's `writable_paths`
//! for the relevant task card, like the sibling M4.4 module before it,
//! predates this file's exact name); that is expected `ported_as` drift for
//! `lint-docs.py`, not a defect in this module or a claim this file is
//! unrelated to that source.

use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput};
use crate::rt64_blender_analysis::blend_cycle_count;
use crate::state::OtherMode;

/// Literal port of `Blender::Approximation` (header lines 39-43). `None` is
/// declared first (ordinal 0) and is this type's [`Default`], matching C++
/// zero-initialization of `reqs.approximateEmulation` inside `reqs = { }`.
/// Variant names are kept byte-identical to the header's own
/// `CombinerFramebuffer1MA_SquareMix`/`AnyFramebuffer1MA_MultiplyMix`
/// (mixed-case with embedded underscores) rather than renamed to idiomatic
/// Rust `UpperCamelCase`, so a reader diffing this type against the header
/// sees the same two names; `non_camel_case_types` is silenced accordingly.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Approximation {
    #[default]
    None,
    CombinerFramebuffer1MA_SquareMix,
    AnyFramebuffer1MA_MultiplyMix,
}

/// Literal port of `EmulationRequirements::Cycle` (header lines 179-191).
/// All three fields are independent bools; see the module doc's "Admitted
/// domain" for exactly which pair is mutually exclusive
/// (`passthrough`/`numeratorOverflow`) and which is not
/// (`framebufferColor`, set by its own separate `if`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cycle {
    pub passthrough: bool,
    pub numerator_overflow: bool,
    pub framebuffer_color: bool,
}

/// Literal port of `Blender::EmulationRequirements` (header lines 178-196).
/// `cycles[1]` is left at its `Cycle::default()` value whenever
/// `blender_cycle_count < 2` -- see [`check_emulation_requirements`]'s
/// "Admitted domain" note; this struct's own [`Default`] reproduces the
/// header's `reqs = { }` zero-init exactly (`simpleEmulation: false`, though
/// [`check_emulation_requirements`] always overwrites that before
/// returning).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmulationRequirements {
    pub cycles: [Cycle; 2],
    pub simple_emulation: bool,
    pub approximate_emulation: Approximation,
}

/// Literal port of the header's `CombinerFramebuffer1MA_SquareMix` pattern
/// (lines 254-258): all eight selectors from both cycles, exact equality,
/// including `A0 == A_CC_ALPHA` and `A1 == A_CC_ALPHA`.
#[allow(clippy::too_many_arguments)]
const fn is_combiner_framebuffer_1ma_square_mix(
    p0: BlendColorInput,
    m0: BlendColorInput,
    a0: BlendAlphaInput,
    b0: BlendBInput,
    p1: BlendColorInput,
    m1: BlendColorInput,
    a1: BlendAlphaInput,
    b1: BlendBInput,
) -> bool {
    matches!(p0, BlendColorInput::Combined)
        && matches!(m0, BlendColorInput::Framebuffer)
        && matches!(a0, BlendAlphaInput::Combined)
        && matches!(b0, BlendBInput::OneMinusA)
        && matches!(p1, BlendColorInput::Combined)
        && matches!(m1, BlendColorInput::Framebuffer)
        && matches!(a1, BlendAlphaInput::Combined)
        && matches!(b1, BlendBInput::OneMinusA)
}

/// Literal port of the header's `AnyFramebuffer1MA_MultiplyMix` pattern
/// (lines 259-263). **Deliberately does not read `A0`/`A1` at all** -- the
/// header's condition never mentions them, so this port takes no `a0`/`a1`
/// parameters, not merely "parameters that are always true". Its first
/// comparison is the header's only inequality in either pattern (`P0 !=
/// PM_FRAMEBUFFER_COLOR`).
const fn is_any_framebuffer_1ma_multiply_mix(
    p0: BlendColorInput,
    m0: BlendColorInput,
    b0: BlendBInput,
    p1: BlendColorInput,
    m1: BlendColorInput,
    b1: BlendBInput,
) -> bool {
    !matches!(p0, BlendColorInput::Framebuffer)
        && matches!(m0, BlendColorInput::Framebuffer)
        && matches!(b0, BlendBInput::OneMinusA)
        && matches!(p1, BlendColorInput::Combined)
        && matches!(m1, BlendColorInput::Framebuffer)
        && matches!(b1, BlendBInput::OneMinusA)
}

/// Literal port of `Blender::checkEmulationRequirements` (header lines
/// 198-271). See the module doc's "Admitted domain" for the full
/// justification of every branch and comparison preserved here; this
/// function's structure mirrors the header's four sequential blocks in
/// order: (1) the per-cycle classification loop bounded by
/// `blender_cycle_count`, (2) the `simple_emulation` decision tree, (3) the
/// gated `Approximation` search, (4) return.
pub fn check_emulation_requirements(other_mode: OtherMode) -> EmulationRequirements {
    let mut reqs = EmulationRequirements::default();

    // (1) Check the cycles for the emulation requirements.
    let blender_cycle_count = blend_cycle_count(other_mode);
    for c in 0..blender_cycle_count {
        let second_cycle = c > 0;
        let cycle = if second_cycle {
            crate::blend::ResolvedBlendCycle::from_wire(other_mode.blender_cycle_2())
        } else {
            crate::blend::ResolvedBlendCycle::from_wire(other_mode.blender_cycle_1())
        };
        let p = cycle.p;
        let m = cycle.m;
        let a = cycle.a;
        let b = cycle.b;
        let any_input_is_zero = a == BlendAlphaInput::Zero || b == BlendBInput::Zero;
        let duplicate_input_1ma = p == m && b == BlendBInput::OneMinusA;
        let slot = &mut reqs.cycles[c as usize];
        if any_input_is_zero || duplicate_input_1ma {
            slot.passthrough = true;
        } else if b != BlendBInput::OneMinusA {
            slot.numerator_overflow = true;
        }

        if p == BlendColorInput::Framebuffer || m == BlendColorInput::Framebuffer {
            slot.framebuffer_color = true;
        }
    }

    // (2) Assume by default simple emulation is possible.
    reqs.simple_emulation = true;

    // First cycle relies on numerator overflow and uses the framebuffer color.
    if reqs.cycles[0].numerator_overflow && reqs.cycles[0].framebuffer_color {
        reqs.simple_emulation = false;
    }
    // Check for two-cycle cases.
    else if blender_cycle_count == 2 {
        // First cycle uses framebuffer color and it's not a simple passthrough.
        if reqs.cycles[0].framebuffer_color && !reqs.cycles[0].passthrough {
            reqs.simple_emulation = false;
        }
        // Second cycle relies on numerator overflow and uses the framebuffer color.
        else if reqs.cycles[1].numerator_overflow && reqs.cycles[1].framebuffer_color {
            reqs.simple_emulation = false;
        }
    }

    // (3) Search for approximations if simple emulation isn't capable.
    if !reqs.simple_emulation && blender_cycle_count == 2 {
        let c0 = crate::blend::ResolvedBlendCycle::from_wire(other_mode.blender_cycle_1());
        let c1 = crate::blend::ResolvedBlendCycle::from_wire(other_mode.blender_cycle_2());
        if is_combiner_framebuffer_1ma_square_mix(c0.p, c0.m, c0.a, c0.b, c1.p, c1.m, c1.a, c1.b) {
            reqs.approximate_emulation = Approximation::CombinerFramebuffer1MA_SquareMix;
        } else if is_any_framebuffer_1ma_multiply_mix(c0.p, c0.m, c0.b, c1.p, c1.m, c1.b) {
            reqs.approximate_emulation = Approximation::AnyFramebuffer1MA_MultiplyMix;
        } else {
            reqs.approximate_emulation = Approximation::None;
        }
    }

    // (4)
    reqs
}

#[cfg(test)]
mod tests;
