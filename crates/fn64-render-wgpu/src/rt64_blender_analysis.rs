//! Blender CPU-side shader-**selection** analysis (RT64 port ticket M4.4):
//! `combineCycleCount`, `blendCycleCount`, `usesInput`/`usesCombinerAlpha`,
//! `usesAlphaBlendCycle`/`usesAlphaBlend`, `usesStandardFogCycle`, and
//! `usesVisualizeCoverageCycle`.
//!
//! Characterization-first, literal port of `Blender::combineCycleCount`,
//! `Blender::blendCycleCount`, `Blender::usesInput`,
//! `Blender::usesCombinerAlpha`, `Blender::usesAlphaBlendCycle`,
//! `Blender::usesAlphaBlend`, both `Blender::usesStandardFogCycle` overloads,
//! and both `Blender::usesVisualizeCoverageCycle` overloads from
//! `src/shared/rt64_blender.h:45-175` (pinned commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, per
//! `docs/RT64-PORT-AUTHORITY.md`). Source digest (sha256, verbatim from
//! `docs/rt64-port-inventory.json`'s `files[].sources.port.sha256` for
//! `src/shared/rt64_blender.h`):
//! `0520bbe032eea7f8791833c25a8b61c03b184174a97bfc77e52e45e1ce28d4b6`
//! (independently reproduced with `shasum -a 256` against the pinned
//! checkout).
//!
//! ```text
//! static uint combineCycleCount(const OtherMode otherMode) {
//!     const uint cycleType = otherMode.cycleType();
//!     if (cycleType == G_CYC_2CYCLE) {
//!         return 2;
//!     }
//!     else if (cycleType == G_CYC_1CYCLE) {
//!         return 1;
//!     }
//!     else {
//!         return 0;
//!     }
//! }
//!
//! static uint blendCycleCount(const OtherMode otherMode) {
//!     const uint ccCount = combineCycleCount(otherMode);
//!     if (otherMode.forceBlend()) {
//!         return ccCount;
//!     }
//!     else {
//!         return (ccCount > 0) ? (ccCount - 1) : 0;
//!     }
//! }
//!
//! static InputPM decodeInputP(uint blenderInputs, bool secondCycle) {
//!     return secondCycle ? (InputPM)((blenderInputs >> 12) & 0x3) : (InputPM)((blenderInputs >> 14) & 0x3);
//! }
//!
//! static InputPM decodeInputM(uint blenderInputs, bool secondCycle) {
//!     return secondCycle ? (InputPM)((blenderInputs >> 4) & 0x3) : (InputPM)((blenderInputs >> 6) & 0x3);
//! }
//!
//! static InputA decodeInputA(uint blenderInputs, bool secondCycle) {
//!     return secondCycle ? (InputA)((blenderInputs >> 8) & 0x3) : (InputA)((blenderInputs >> 10) & 0x3);
//! }
//!
//! static InputB decodeInputB(uint blenderInputs, bool secondCycle) {
//!     return secondCycle ? (InputB)((blenderInputs >> 0) & 0x3) : (InputB)((blenderInputs >> 2) & 0x3);
//! }
//!
//! static bool usesInput(const OtherMode otherMode, InputA inputA) {
//!     const uint32_t cycles = blendCycleCount(otherMode);
//!     const uint blenderInputs = otherMode.blenderInputs();
//!     for (uint32_t c = 0; c < cycles; c++) {
//!         if (decodeInputA(blenderInputs, c > 0) == inputA) {
//!             return true;
//!         }
//!     }
//!
//!     return false;
//! }
//!
//! static bool usesCombinerAlpha(const OtherMode otherMode) {
//!     return usesInput(otherMode, Blender::A_CC_ALPHA);
//! }
//!
//! static bool usesAlphaBlendCycle(const OtherMode otherMode, bool secondCycle, bool allInputs) {
//!     const uint blenderInputs = otherMode.blenderInputs();
//!     const InputPM P = decodeInputP(blenderInputs, secondCycle);
//!     if (allInputs) {
//!         const InputA A = decodeInputA(blenderInputs, secondCycle);
//!         if ((P == PM_FRAMEBUFFER_COLOR) && (A != A_ZERO)) {
//!             return true;
//!         }
//!
//!         const InputPM M = decodeInputM(blenderInputs, secondCycle);
//!         const InputB B = decodeInputB(blenderInputs, secondCycle);
//!         if ((M == PM_FRAMEBUFFER_COLOR) && (B != B_ZERO)) {
//!             return true;
//!         }
//!     }
//!     else if (P == PM_FRAMEBUFFER_COLOR) {
//!         return true;
//!     }
//!
//!     return false;
//! }
//!
//! static bool usesAlphaBlend(const OtherMode otherMode) {
//!     const bool forceBlend = otherMode.forceBlend();
//!     const uint ccCount = combineCycleCount(otherMode);
//!     if ((ccCount >= 2) && usesAlphaBlendCycle(otherMode, true, forceBlend)) {
//!         return true;
//!     }
//!
//!     if ((ccCount >= 1) && usesAlphaBlendCycle(otherMode, false, (ccCount >= 2) || forceBlend)) {
//!         return true;
//!     }
//!
//!     return false;
//! }
//!
//! static bool usesStandardFogCycle(uint blenderInputs, uint cycleIndex) {
//!     const bool secondCycle = (cycleIndex > 0);
//!     const InputPM P = decodeInputP(blenderInputs, secondCycle);
//!     const InputPM M = decodeInputM(blenderInputs, secondCycle);
//!     const InputA A = decodeInputA(blenderInputs, secondCycle);
//!     const InputB B = decodeInputB(blenderInputs, secondCycle);
//!     return (P == PM_FOG_COLOR) && (A == A_SHADE_ALPHA) && (M == PM_CC_OR_BLENDER) && (B == B_ONE_MINUS_A);
//! }
//!
//! static bool usesStandardFogCycle(const OtherMode otherMode) {
//!     const uint cycles = blendCycleCount(otherMode);
//!     const uint blenderInputs = otherMode.blenderInputs();
//!     for (uint c = 0; c < cycles; c++) {
//!         if (usesStandardFogCycle(blenderInputs, c)) {
//!             return true;
//!         }
//!     }
//!
//!     return false;
//! }
//!
//! static bool usesVisualizeCoverageCycle(uint blenderInputs, uint cycleIndex) {
//!     const bool secondCycle = (cycleIndex > 0);
//!     const InputPM M = decodeInputM(blenderInputs, secondCycle);
//!     const InputA A = decodeInputA(blenderInputs, secondCycle);
//!     const InputB B = decodeInputB(blenderInputs, secondCycle);
//!     return (A == A_ZERO) && (M == PM_BLEND_COLOR) && (B == B_FRAMEBUFFER_ALPHA);
//! }
//!
//! static bool usesVisualizeCoverageCycle(const OtherMode otherMode) {
//!     const uint cycles = blendCycleCount(otherMode);
//!     const uint blenderInputs = otherMode.blenderInputs();
//!     for (uint c = 0; c < cycles; c++) {
//!         if (usesVisualizeCoverageCycle(blenderInputs, c)) {
//!             return true;
//!         }
//!     }
//!
//!     return false;
//! }
//! ```
//!
//! ## Reuse, not new type
//!
//! `crates/fn64-render-wgpu/src/blend.rs` already ports the *other* half of
//! this same header: the runtime blend **evaluation** path
//! (`Blender::runCycle`/`Blender::run`, header lines 366-504, exposed there as
//! `blend_fragment`/`blend_color`/`blend_a`/`blend_b`) plus the P/M/A/B
//! selector decode those functions need (header lines 68-81, exposed there as
//! `ResolvedBlendCycle::from_wire` and `BlendColorInput`/`BlendAlphaInput`/
//! `BlendBInput`). This module ports the header's **disjoint** other half:
//! the CPU-side analysis RT64 runs *before* any fragment shades, to decide
//! which shader/pipeline variant to select (header lines 45-175: the
//! `*CycleCount`/`uses*` predicate cluster). Neither half calls the other in
//! the header, and this module does not call anything in `blend.rs` that
//! *runs* a blend -- it only imports `blend.rs`'s already-verified selector
//! enums and per-cycle decode (`BlendColorInput`, `BlendAlphaInput`,
//! `BlendBInput`, `ResolvedBlendCycle`) rather than redefining a second copy
//! of `InputPM`/`InputA`/`InputB`. Those types' `from_wire` mappings were
//! independently checked against the header's own enum literals
//! (`PM_CC_OR_BLENDER=0/PM_FRAMEBUFFER_COLOR=1/PM_BLEND_COLOR=2/PM_FOG_COLOR=3`,
//! `A_CC_ALPHA=0/A_FOG_ALPHA=1/A_SHADE_ALPHA=2/A_ZERO=3`,
//! `B_ONE_MINUS_A=0/B_FRAMEBUFFER_ALPHA=1/B_ONE=2/B_ZERO=3`) while writing
//! this module and confirmed identical; no genuine overlap was found beyond
//! that shared, already-landed vocabulary.
//!
//! This module also reuses `crate::state::OtherMode`'s landed
//! `cycle_type()`/`force_blend()`/`blender_cycle_1()`/`blender_cycle_2()`
//! accessors rather than adding a duplicate `cycleType()`/`forceBlend()`
//! decode. `OtherMode` has no raw 16-bit `blenderInputs()` word accessor (see
//! "Admitted domain" below); `blender_cycle_1()`/`blender_cycle_2()` already
//! return exactly `decodeInputP/M/A/B(blenderInputs, secondCycle=false/true)`
//! for both cycles at once (confirmed by shift arithmetic below), so this
//! module calls those two accessors instead of re-deriving the header's
//! four free `decodeInput*` functions from a raw word this crate does not
//! expose. `state.rs` is read-only to this ticket (owned by ticket M4.2,
//! a separate module this ticket does not modify); this module adds no
//! field, accessor, or edit there.
//!
//! ## Admitted domain
//!
//! - **`blender_cycle_1`/`blender_cycle_2` == `decodeInputP/M/A/B` for both
//!   cycles.** RT64's `blenderInputs()` is the other-mode `low` word's high
//!   16 bits (`low >> 16`, per `state.rs`'s own `BlenderCycle` doc). Applying
//!   the header's `decodeInput*` shifts to that 16-bit slice and adding back
//!   the `+16` origin recovers exactly `state.rs`'s absolute-bit shifts:
//!   `decodeInputP(..,false)`'s shift-14 -> absolute bit 30 ==
//!   `blender_cycle_1().color_a`; `decodeInputP(..,true)`'s shift-12 ->
//!   absolute bit 28 == `blender_cycle_2().color_a`; `decodeInputM`'s
//!   shift-6/4 -> absolute bit 22/20 == `blender_cycle_1/2().color_b`;
//!   `decodeInputA`'s shift-10/8 -> absolute bit 26/24 ==
//!   `blender_cycle_1/2().alpha_a`; `decodeInputB`'s shift-2/0 -> absolute
//!   bit 18/16 == `blender_cycle_1/2().alpha_b`. So `P = color_a`, `M =
//!   color_b`, `A = alpha_a`, `B = alpha_b`, and "not second cycle" (`P` in
//!   `decodeInputP(x, false)`) is `blender_cycle_1`, "second cycle" is
//!   `blender_cycle_2` -- exactly this module's [`cycle_selectors`] helper.
//! - **`combineCycleCount`'s three-way `if`/`else if`/`else` collapses
//!   `Copy`/`Fill` into one `else` branch returning `0`.** This is not
//!   normalized into an exhaustive `match` over all four `CycleType`
//!   variants naming `Copy`/`Fill` separately -- the header genuinely never
//!   distinguishes them here, so [`combine_cycle_count`] mirrors that with
//!   its own catch-all arm, and a dedicated test pins both `Copy` and `Fill`
//!   returning `0` via the *same* code path (not merely the same result).
//! - **`blendCycleCount`'s strict `> 0` (not `>= 1`) guard on the
//!   subtraction.** Only matters at `ccCount == 0`: `0 > 0` is false, so the
//!   `else` arm's `(ccCount > 0) ? (ccCount - 1) : 0` yields `0` without ever
//!   evaluating `ccCount - 1` (which would underflow on an unsigned `uint` if
//!   evaluated at `ccCount == 0`). [`blend_cycle_count`] preserves the guard
//!   as a literal `if ccCount > 0` before subtracting, pinned by
//!   `blend_cycle_count_boundary_ccount_0_1_2_force_blend_on_and_off`.
//! - **`usesAlphaBlendCycle`'s `allInputs` branch is asymmetric by
//!   construction, not by oversight.** When `allInputs` is `false`, only `P`
//!   is decoded and checked; `M`/`A`/`B` are never even decoded, and a
//!   `PM_FRAMEBUFFER_COLOR` `M` selector with a non-`ZERO` `B` selector is
//!   *not* detected on that path. [`uses_alpha_blend_cycle`] preserves this
//!   literally: the `M`/`B` check lives strictly inside the `if allInputs`
//!   block, never hoisted out or OR'd into the `else` arm. Both the
//!   `all_inputs=true` two-guard path and the `all_inputs=false`
//!   single-guard path are pinned, including the case where `M`/`B` alone
//!   would trigger `true` under `allInputs=true` but must stay `false` under
//!   `allInputs=false`.
//! - **`usesAlphaBlend`'s two guards are sequential `if`s, not
//!   `if`/`else if`.** Both `ccCount >= 2` and `ccCount >= 1` are evaluated
//!   independently (the first's `return true` only fires on a match; on no
//!   match, execution falls through to evaluate the second guard from
//!   scratch, not skipped because the first guard's condition was true).
//!   [`uses_alpha_blend`] preserves this as two independent
//!   `if cond && call(..) { return true }` checks rather than collapsing them
//!   into one `match` on `ccCount`. Also preserves `usesAlphaBlend` calling
//!   `combineCycleCount`
//!   (not `blendCycleCount`) directly -- a different cycle count than the
//!   `usesInput`/`usesStandardFogCycle`/`usesVisualizeCoverageCycle` family,
//!   which all loop over `blendCycleCount`.
//! - **`usesVisualizeCoverageCycle`'s single-cycle predicate never reads
//!   `P`.** Only `M`/`A`/`B` are decoded and compared; `P`'s value is
//!   irrelevant to this predicate. [`uses_visualize_coverage_cycle`] mirrors
//!   this by never calling `cycle_selectors`' `p` field, pinned by varying
//!   `P` across all four values while holding `M`/`A`/`B` at the matching
//!   triple and asserting the predicate stays `true` throughout.
//! - **The loop bodies (`usesInput`, `usesStandardFogCycle`,
//!   `usesVisualizeCoverageCycle` aggregate overloads) have no `break`
//!   statement; they return `true` immediately on the first match instead**
//!   -- functionally an early-return, not a loop that runs to completion.
//!   Ported literally as `for c in 0..cycles { if pred(..) { return true } }`
//!   (Rust's `for` has no fallthrough to worry about, but the *early return*
//!   shape, not a boolean accumulator ORed across all iterations, is
//!   preserved -- a flag-accumulate rewrite would behave identically for
//!   these particular boolean-OR loops, but is not what the source does, so
//!   it is not what this port does either).
//! - **Enum-ordinal handling: none of these ordinals can go out of range.**
//!   `InputPM`/`InputA`/`InputB` are 2-bit wire fields (`& 0x3` in
//!   `decodeInput*`, and identically in `BlendColorInput`/`BlendAlphaInput`/
//!   `BlendBInput::from_wire`'s own `& 0x3` mask); every 2-bit pattern maps
//!   to a defined variant, so there is no reserved/default encoding to
//!   normalize here (unlike, e.g., `AlphaCompare`'s encoding 2 or
//!   `TextureLutMode`'s encoding 1 elsewhere in `state.rs`). The out-of-range
//!   surface this module does own is `cycle_index: u32` on the two-arg
//!   overloads (`usesStandardFogCycle`/`usesVisualizeCoverageCycle`'s
//!   `(blenderInputs, cycleIndex)` form): the header computes `secondCycle =
//!   (cycleIndex > 0)`, so *every* `cycleIndex >= 1` (not just `1`) selects
//!   the second cycle -- `cycle_index == 5` behaves identically to
//!   `cycle_index == 1`. Pinned by
//!   `standard_fog_cycle_two_arg_treats_any_nonzero_cycle_index_as_second_cycle`
//!   and the analogous visualize-coverage test.
//!
//! ## Nonclaims
//!
//! No GPU execution, resource binding, draw-call, or pipeline-selection
//! integration -- these are pure CPU predicates over [`crate::state::OtherMode`]
//! only, matching the ticket's "unwired CPU predicates; no parity claim".
//! Nothing here is called from production code (`grep`-verified: this
//! module is `mod`-declared in `lib.rs` but never `pub use`'d or referenced
//! outside its own tests). No claim of pixel, timing, or full-ROM parity
//! with RT64; no claim about which real N64 titles exercise which branch.
//! Partial coverage of the header: this module ports only the
//! shader-*selection* analysis cluster (lines 45-175) the ticket names. It
//! does **not** port `EmulationRequirements`/`checkEmulationRequirements`
//! here (lines 178-271, a separate CPU analysis for blend *emulation
//! strategy*, not pipeline *selection* -- out of scope per this module's
//! ticket's named symbol list). Read that as a refusal by *this* module,
//! not as a claim the crate lacks the symbol: `rt64_blender_emulation.rs`
//! (ticket M4.5) ports both, as
//! [`EmulationRequirements`](crate::rt64_blender_emulation::EmulationRequirements)
//! and
//! [`check_emulation_requirements`](crate::rt64_blender_emulation::check_emulation_requirements),
//! which is that module's own declared scope. This module also does not
//! port `inputToString` (lines 273-316,
//! debug-only string formatting, not analysis), or
//! `fromInputPM`/`fromInputA`/`fromInputB`/`runCycle`/`run` (lines
//! 324-505, the runtime evaluation half `blend.rs` already owns).
//! `docs/rt64-port-inventory.json`'s `port_state` for this file may still
//! read `not-started`/`ported_as: []` after this change lands (the
//! inventory's `writable_paths` for this task card names
//! `rt64_blender_h.rs`, not `rt64_blender_analysis.rs` -- see the ticket/
//! brief scope note below); that is expected drift for `lint-docs.py`, not a
//! defect in this module.
//!
//! **Ticket/brief scope note:** the M4.4 ticket text names `usesInput`/
//! `usesCombinerAlpha` and `usesAlphaBlendCycle` in addition to the six
//! functions the assigning brief listed by name
//! (`combineCycleCount`/`blendCycleCount`/`usesAlphaBlend`/
//! `usesStandardFogCycle`/`usesVisualizeCoverageCycle` plus "whatever else
//! the ticket lists"). Per instruction, this module ports the ticket's
//! fuller list, not the brief's shorter one.

use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput, ResolvedBlendCycle};
use crate::state::OtherMode;

/// Literal port of `Blender::combineCycleCount` (header lines 45-56). `2CYCLE`
/// -> `2`, `1CYCLE` -> `1`, everything else (`Copy` and `Fill` both) -> `0`
/// via the header's own single `else` catch-all -- not an exhaustive match
/// naming `Copy`/`Fill` separately.
pub const fn combine_cycle_count(other_mode: OtherMode) -> u32 {
    match other_mode.cycle_type() {
        crate::state::CycleType::TwoCycle => 2,
        crate::state::CycleType::OneCycle => 1,
        crate::state::CycleType::Copy | crate::state::CycleType::Fill => 0,
    }
}

/// Literal port of `Blender::blendCycleCount` (header lines 58-66). With
/// `forceBlend` set, returns `combineCycleCount` unchanged; otherwise
/// subtracts one, guarded by a strict `> 0` check so `ccCount == 0` never
/// underflows.
pub const fn blend_cycle_count(other_mode: OtherMode) -> u32 {
    let cc_count = combine_cycle_count(other_mode);
    if other_mode.force_blend() {
        cc_count
    } else if cc_count > 0 {
        cc_count - 1
    } else {
        0
    }
}

/// Resolve one blend cycle's `P`/`M`/`A`/`B` selectors, reusing
/// `crate::blend::ResolvedBlendCycle::from_wire` over
/// `OtherMode::blender_cycle_1()`/`blender_cycle_2()`. `second_cycle == false`
/// is the header's `decodeInput*(.., secondCycle=false)` (cycle 1); `true` is
/// `secondCycle=true` (cycle 2) -- see "Admitted domain" for the shift-level
/// proof these are the same decode as the header's free `decodeInputP/M/A/B`
/// functions.
const fn cycle_selectors(other_mode: OtherMode, second_cycle: bool) -> ResolvedBlendCycle {
    if second_cycle {
        ResolvedBlendCycle::from_wire(other_mode.blender_cycle_2())
    } else {
        ResolvedBlendCycle::from_wire(other_mode.blender_cycle_1())
    }
}

/// Literal port of `Blender::usesInput` (header lines 84-94), specialized to
/// `InputA` (the header's only instantiation). Loops `c` in `0..cycles`
/// (`cycles = blendCycleCount`), decoding cycle `c > 0` each iteration
/// (`c == 0` -> first cycle, `c >= 1` -> second cycle), returning `true` on
/// the first match rather than accumulating across the whole range.
pub fn uses_input(other_mode: OtherMode, input_a: BlendAlphaInput) -> bool {
    let cycles = blend_cycle_count(other_mode);
    for c in 0..cycles {
        if cycle_selectors(other_mode, c > 0).a == input_a {
            return true;
        }
    }
    false
}

/// Literal port of `Blender::usesCombinerAlpha` (header lines 96-98):
/// `usesInput(otherMode, A_CC_ALPHA)`.
pub fn uses_combiner_alpha(other_mode: OtherMode) -> bool {
    uses_input(other_mode, BlendAlphaInput::Combined)
}

/// Literal port of `Blender::usesAlphaBlendCycle` (header lines 100-120).
/// **Asymmetric by construction**: when `all_inputs` is `false`, only `P` is
/// decoded and checked (`M`/`A`/`B` are never even read); the `M`/`B` check
/// exists strictly inside the `all_inputs == true` branch. Do not hoist the
/// `M`/`B` check out of that branch or OR it into the `else` arm.
pub fn uses_alpha_blend_cycle(other_mode: OtherMode, second_cycle: bool, all_inputs: bool) -> bool {
    let cycle = cycle_selectors(other_mode, second_cycle);
    if all_inputs {
        if cycle.p == BlendColorInput::Framebuffer && cycle.a != BlendAlphaInput::Zero {
            return true;
        }
        if cycle.m == BlendColorInput::Framebuffer && cycle.b != BlendBInput::Zero {
            return true;
        }
        false
    } else {
        cycle.p == BlendColorInput::Framebuffer
    }
}

/// Literal port of `Blender::usesAlphaBlend` (header lines 122-134). Two
/// **sequential, independent** `if` guards (not `if`/`else if`): the
/// `ccCount >= 2` guard's non-match falls through to evaluate the
/// `ccCount >= 1` guard from scratch, rather than being skipped. Uses
/// `combineCycleCount` directly (not `blendCycleCount`), unlike every other
/// predicate in this module.
pub fn uses_alpha_blend(other_mode: OtherMode) -> bool {
    let force_blend = other_mode.force_blend();
    let cc_count = combine_cycle_count(other_mode);
    if cc_count >= 2 && uses_alpha_blend_cycle(other_mode, true, force_blend) {
        return true;
    }
    if cc_count >= 1 && uses_alpha_blend_cycle(other_mode, false, cc_count >= 2 || force_blend) {
        return true;
    }
    false
}

/// Literal port of the two-argument `Blender::usesStandardFogCycle` overload
/// (header lines 136-143). `cycle_index > 0` selects the second cycle --
/// **any** nonzero index, not only `1` (`cycle_index == 5` behaves exactly
/// like `cycle_index == 1`), matching the header's `(cycleIndex > 0)` cast to
/// `bool`.
pub fn uses_standard_fog_cycle_at(other_mode: OtherMode, cycle_index: u32) -> bool {
    let cycle = cycle_selectors(other_mode, cycle_index > 0);
    cycle.p == BlendColorInput::Fog
        && cycle.a == BlendAlphaInput::Shade
        && cycle.m == BlendColorInput::Combined
        && cycle.b == BlendBInput::OneMinusA
}

/// Literal port of the zero-argument `Blender::usesStandardFogCycle` overload
/// (header lines 145-155): loops `c` in `0..blendCycleCount`, returning `true`
/// on the first cycle whose [`uses_standard_fog_cycle_at`] matches.
pub fn uses_standard_fog_cycle(other_mode: OtherMode) -> bool {
    let cycles = blend_cycle_count(other_mode);
    for c in 0..cycles {
        if uses_standard_fog_cycle_at(other_mode, c) {
            return true;
        }
    }
    false
}

/// Literal port of the two-argument `Blender::usesVisualizeCoverageCycle`
/// overload (header lines 157-163). **Never reads `P`** -- only `M`/`A`/`B`
/// are decoded and compared; `P`'s value is irrelevant to this predicate.
/// `cycle_index > 0` selects the second cycle, matching
/// [`uses_standard_fog_cycle_at`]'s any-nonzero-index handling.
pub fn uses_visualize_coverage_cycle_at(other_mode: OtherMode, cycle_index: u32) -> bool {
    let cycle = cycle_selectors(other_mode, cycle_index > 0);
    cycle.a == BlendAlphaInput::Zero
        && cycle.m == BlendColorInput::Blend
        && cycle.b == BlendBInput::FramebufferAlpha
}

/// Literal port of the zero-argument `Blender::usesVisualizeCoverageCycle`
/// overload (header lines 165-175): loops `c` in `0..blendCycleCount`,
/// returning `true` on the first cycle whose
/// [`uses_visualize_coverage_cycle_at`] matches.
pub fn uses_visualize_coverage_cycle(other_mode: OtherMode) -> bool {
    let cycles = blend_cycle_count(other_mode);
    for c in 0..cycles {
        if uses_visualize_coverage_cycle_at(other_mode, c) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;
