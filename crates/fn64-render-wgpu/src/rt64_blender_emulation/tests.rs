//! Characterization tests for `rt64_blender_emulation`, pinning hand-computed
//! expectations (never values captured from this module's own output) for
//! each per-cycle classification, every `simple_emulation` decision-tree
//! path, both `Approximation` patterns (and their asymmetry), the
//! `Approximation::None`-via-default case, and out-of-range considerations.

use super::*;
use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput};
use crate::state::OtherMode;

/// Build an `OtherMode` selecting `cycle_type_bits` (0=OneCycle, 1=TwoCycle,
/// 2=Copy, 3=Fill) with `force_blend` and both cycles' four selectors set
/// directly by wire value, per `state.rs`'s documented absolute-bit layout:
/// cycle 1 = {color_a:30-31, alpha_a:26-27, color_b:22-23, alpha_b:18-19},
/// cycle 2 = {color_a:28-29, alpha_a:24-25, color_b:20-21, alpha_b:16-17}.
/// Mirrors `rt64_blender_analysis/tests.rs`'s helper of the same shape (a
/// deliberate independent copy, not a shared import, per each module's own
/// test scope).
#[allow(clippy::too_many_arguments)]
fn mode(
    cycle_type_bits: u32,
    force_blend: bool,
    c1_color_a: u32,
    c1_alpha_a: u32,
    c1_color_b: u32,
    c1_alpha_b: u32,
    c2_color_a: u32,
    c2_alpha_a: u32,
    c2_color_b: u32,
    c2_alpha_b: u32,
) -> OtherMode {
    let high = (cycle_type_bits & 0x3) << 20;
    let mut low = 0u32;
    if force_blend {
        low |= 0x4000;
    }
    low |= (c1_color_a & 0x3) << 30;
    low |= (c1_alpha_a & 0x3) << 26;
    low |= (c1_color_b & 0x3) << 22;
    low |= (c1_alpha_b & 0x3) << 18;
    low |= (c2_color_a & 0x3) << 28;
    low |= (c2_alpha_a & 0x3) << 24;
    low |= (c2_color_b & 0x3) << 20;
    low |= (c2_alpha_b & 0x3) << 16;
    OtherMode::from_wire(high, low)
}

// Selector wire values (see header enum literals in the module doc):
// InputPM: Combined=0, Framebuffer=1, Blend=2, Fog=3
// InputA:  Combined=0, Fog=1, Shade=2, Zero=3
// InputB:  OneMinusA=0, FramebufferAlpha=1, One=2, Zero=3

const P_COMBINED: u32 = 0;
const P_FRAMEBUFFER: u32 = 1;
const P_BLEND: u32 = 2;
const A_COMBINED: u32 = 0;
const A_SHADE: u32 = 2;
const A_ZERO: u32 = 3;
const B_ONE_MINUS_A: u32 = 0;
const B_ONE: u32 = 2;
const B_ZERO: u32 = 3;

/// One-cycle mode (`blend_cycle_count == 1`): `cycle_type_bits = 0`
/// (OneCycle -> `combineCycleCount = 1`), `force_blend = true` so
/// `blendCycleCount` keeps it at 1 instead of subtracting to 0.
#[allow(clippy::too_many_arguments)]
fn one_cycle_mode(color_a: u32, alpha_a: u32, color_b: u32, alpha_b: u32) -> OtherMode {
    mode(0, true, color_a, alpha_a, color_b, alpha_b, 0, 0, 0, 0)
}

/// Two-cycle mode (`blend_cycle_count == 2`): `cycle_type_bits = 1`
/// (TwoCycle -> `combineCycleCount = 2`), `force_blend = true` so
/// `blendCycleCount` keeps it at 2 instead of subtracting to 1.
#[allow(clippy::too_many_arguments)]
fn two_cycle_mode(
    c1_color_a: u32,
    c1_alpha_a: u32,
    c1_color_b: u32,
    c1_alpha_b: u32,
    c2_color_a: u32,
    c2_alpha_a: u32,
    c2_color_b: u32,
    c2_alpha_b: u32,
) -> OtherMode {
    mode(
        1, true, c1_color_a, c1_alpha_a, c1_color_b, c1_alpha_b, c2_color_a, c2_alpha_a,
        c2_color_b, c2_alpha_b,
    )
}

/// Zero-cycle mode (`blend_cycle_count == 0`): Copy (`cycle_type_bits = 2`),
/// `force_blend = false` so `blendCycleCount` stays at Copy's
/// `combineCycleCount` of 0 (the `> 0` guard's `false` side, `0 > 0` is
/// false, so no subtraction occurs -- it's already 0).
fn zero_cycle_mode() -> OtherMode {
    mode(2, false, 0, 0, 0, 0, 0, 0, 0, 0)
}

// ---------------------------------------------------------------------
// blend_cycle_count sanity for the mode builders above (guards the rest of
// this file's hand-computed expectations against a wrong fixture).
// ---------------------------------------------------------------------

#[test]
fn fixture_blend_cycle_counts_are_0_1_2_as_intended() {
    assert_eq!(blend_cycle_count(zero_cycle_mode()), 0);
    assert_eq!(blend_cycle_count(one_cycle_mode(0, 0, 0, 0)), 1);
    assert_eq!(blend_cycle_count(two_cycle_mode(0, 0, 0, 0, 0, 0, 0, 0)), 2);
}

// ---------------------------------------------------------------------
// Approximation::default()
// ---------------------------------------------------------------------

#[test]
fn approximation_default_is_none_ordinal_zero() {
    // C++ scoped-enum value-init is the zero bit pattern; Approximation::None
    // is declared first (ordinal 0). Rust's #[default] must name the same
    // variant, and there is exactly one way to construct a default value.
    assert_eq!(Approximation::default(), Approximation::None);
}

#[test]
fn cycle_default_is_all_false() {
    let c = Cycle::default();
    assert!(!c.passthrough);
    assert!(!c.numerator_overflow);
    assert!(!c.framebuffer_color);
}

#[test]
fn emulation_requirements_default_is_full_zero_init() {
    let reqs = EmulationRequirements::default();
    assert_eq!(reqs.cycles[0], Cycle::default());
    assert_eq!(reqs.cycles[1], Cycle::default());
    assert!(!reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
}

// ---------------------------------------------------------------------
// Per-cycle classification: passthrough / numeratorOverflow / framebufferColor
// ---------------------------------------------------------------------

#[test]
fn cycle_passthrough_when_alpha_a_is_zero() {
    // anyInputIsZero via A == A_ZERO. P/M/B otherwise generic (Combined/
    // Combined/One) so duplicateInput1MA and numeratorOverflow are not
    // independently satisfied -- isolates the A_ZERO path.
    let reqs = check_emulation_requirements(one_cycle_mode(P_COMBINED, A_ZERO, P_COMBINED, B_ONE));
    assert!(reqs.cycles[0].passthrough);
    assert!(!reqs.cycles[0].numerator_overflow);
}

#[test]
fn cycle_passthrough_when_alpha_b_is_zero() {
    // anyInputIsZero via B == B_ZERO, A left non-zero (Shade).
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_COMBINED, A_SHADE, P_COMBINED, B_ZERO));
    assert!(reqs.cycles[0].passthrough);
}

#[test]
fn cycle_passthrough_when_p_equals_m_and_b_is_one_minus_a() {
    // duplicateInput1MA: P == M (both Blend) and B == B_ONE_MINUS_A, with A
    // non-zero and B non-zero so anyInputIsZero is false -- isolates the
    // duplicateInput1MA disjunct.
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_BLEND, A_SHADE, P_BLEND, B_ONE_MINUS_A));
    assert!(reqs.cycles[0].passthrough);
    assert!(!reqs.cycles[0].numerator_overflow);
}

#[test]
fn cycle_duplicate_1ma_requires_b_one_minus_a_not_just_p_equals_m() {
    // P == M (both Blend) but B == B_ONE (not OneMinusA) and A non-zero, B
    // non-zero: duplicateInput1MA's B_ONE_MINUS_A conjunct fails, and neither
    // disjunct of anyInputIsZero holds either -- passthrough must be false,
    // and B != B_ONE_MINUS_A makes numeratorOverflow true instead.
    let reqs = check_emulation_requirements(one_cycle_mode(P_BLEND, A_SHADE, P_BLEND, B_ONE));
    assert!(!reqs.cycles[0].passthrough);
    assert!(reqs.cycles[0].numerator_overflow);
}

#[test]
fn cycle_numerator_overflow_when_b_is_not_one_minus_a_and_not_passthrough() {
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_COMBINED, A_SHADE, P_FRAMEBUFFER, B_ONE));
    assert!(!reqs.cycles[0].passthrough);
    assert!(reqs.cycles[0].numerator_overflow);
}

#[test]
fn cycle_all_three_bools_false_when_1ma_and_no_zero_input() {
    // Proper single-cycle 1MA (B == B_ONE_MINUS_A) with P != M (Combined vs
    // Framebuffer, so duplicateInput1MA's P==M conjunct is false) and no
    // zero input (A=Shade, B=OneMinusA != Zero): neither passthrough
    // disjunct holds, and B == B_ONE_MINUS_A means the numeratorOverflow
    // `else if` guard (`B != B_ONE_MINUS_A`) is false too. All three Cycle
    // bools stay false -- a real reachable third state, not merely "unset".
    // (M == Framebuffer here also makes framebufferColor true, independently
    // -- that's the separate check covered elsewhere; this test asserts it
    // too, to show the "all three false" claim is about passthrough and
    // numerator_overflow specifically, not that framebufferColor is also
    // false in this exact fixture.)
    let reqs = check_emulation_requirements(one_cycle_mode(
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    ));
    assert!(!reqs.cycles[0].passthrough);
    assert!(!reqs.cycles[0].numerator_overflow);
    assert!(reqs.cycles[0].framebuffer_color);
}

#[test]
fn cycle_framebuffer_color_true_when_p_is_framebuffer() {
    let reqs = check_emulation_requirements(one_cycle_mode(
        P_FRAMEBUFFER,
        A_ZERO,
        P_COMBINED,
        B_ONE_MINUS_A,
    ));
    assert!(reqs.cycles[0].framebuffer_color);
}

#[test]
fn cycle_framebuffer_color_true_when_m_is_framebuffer() {
    let reqs = check_emulation_requirements(one_cycle_mode(
        P_COMBINED,
        A_ZERO,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    ));
    assert!(reqs.cycles[0].framebuffer_color);
}

#[test]
fn cycle_framebuffer_color_false_when_neither_p_nor_m_is_framebuffer() {
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_COMBINED, A_ZERO, P_BLEND, B_ONE_MINUS_A));
    assert!(!reqs.cycles[0].framebuffer_color);
}

#[test]
fn cycle_passthrough_and_framebuffer_color_can_both_be_true() {
    // P == Framebuffer, A == Zero (anyInputIsZero -> passthrough), M !=
    // Framebuffer so framebufferColor comes from P alone. Proves these two
    // fields are independent, not mutually exclusive.
    let reqs = check_emulation_requirements(one_cycle_mode(
        P_FRAMEBUFFER,
        A_ZERO,
        P_COMBINED,
        B_ONE_MINUS_A,
    ));
    assert!(reqs.cycles[0].passthrough);
    assert!(reqs.cycles[0].framebuffer_color);
    assert!(!reqs.cycles[0].numerator_overflow);
}

#[test]
fn cycle_numerator_overflow_and_framebuffer_color_can_both_be_true() {
    // P == Framebuffer (framebufferColor), A/B both non-zero and P != M and
    // B != OneMinusA (numeratorOverflow), all in the same cycle.
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_FRAMEBUFFER, A_SHADE, P_COMBINED, B_ONE));
    assert!(!reqs.cycles[0].passthrough);
    assert!(reqs.cycles[0].numerator_overflow);
    assert!(reqs.cycles[0].framebuffer_color);
}

// ---------------------------------------------------------------------
// Loop bound: cycles[1] stays zero-init unless blender_cycle_count == 2.
// ---------------------------------------------------------------------

#[test]
fn cycles_1_stays_zero_when_blender_cycle_count_is_0() {
    let reqs = check_emulation_requirements(zero_cycle_mode());
    assert_eq!(reqs.cycles[1], Cycle::default());
}

#[test]
fn cycles_1_stays_zero_when_blender_cycle_count_is_1() {
    // cycle 1 (index 0) set to something that would classify strongly if it
    // were read as cycle 2, but blend_cycle_count == 1 means the loop only
    // runs c=0 -- cycles[1] must remain fully default regardless.
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_FRAMEBUFFER, A_SHADE, P_COMBINED, B_ONE));
    assert_eq!(reqs.cycles[1], Cycle::default());
}

#[test]
fn cycles_1_is_populated_when_blender_cycle_count_is_2() {
    let reqs = check_emulation_requirements(two_cycle_mode(
        P_COMBINED,
        A_ZERO,
        P_COMBINED,
        B_ONE_MINUS_A,
        P_FRAMEBUFFER,
        A_SHADE,
        P_COMBINED,
        B_ONE,
    ));
    // Cycle 2 (index 1): P=Framebuffer, A=Shade, M=Combined, B=One ->
    // framebufferColor true (P==Framebuffer), numeratorOverflow true
    // (B != OneMinusA, and neither passthrough disjunct holds: A != Zero,
    // B != Zero, and P != M so duplicateInput1MA is false too).
    assert!(reqs.cycles[1].framebuffer_color);
    assert!(reqs.cycles[1].numerator_overflow);
    assert!(!reqs.cycles[1].passthrough);
}

// ---------------------------------------------------------------------
// simple_emulation decision tree
// ---------------------------------------------------------------------

#[test]
fn simple_emulation_true_by_default_when_no_branch_fires() {
    // One cycle, all-zero selectors (P=M=Combined, A=Combined, B=OneMinusA):
    // duplicateInput1MA fires (P==M, B==OneMinusA) -> passthrough true,
    // numeratorOverflow false; framebufferColor false (neither P nor M is
    // Framebuffer). The cycle-0 numeratorOverflow&&framebufferColor guard is
    // false (numeratorOverflow is false), and blender_cycle_count != 2, so
    // the whole tree leaves simple_emulation at its post-loop `true`.
    let reqs = check_emulation_requirements(one_cycle_mode(0, 0, 0, 0));
    assert!(reqs.simple_emulation);
}

#[test]
fn simple_emulation_false_from_cycle0_alone_even_in_one_cycle_mode() {
    // Hazard case: the cycle-0 numeratorOverflow&&framebufferColor check is
    // NOT gated on blender_cycle_count == 2. One-cycle mode, cycle 0: P =
    // Framebuffer (framebufferColor), A/B non-zero, P != M, B != OneMinusA
    // (numeratorOverflow). Must set simple_emulation = false even though
    // blender_cycle_count == 1, never reaching the `else if` at all.
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_FRAMEBUFFER, A_SHADE, P_COMBINED, B_ONE));
    assert!(!reqs.simple_emulation);
}

#[test]
fn simple_emulation_true_in_one_cycle_mode_when_cycle0_is_not_numerator_overflow_framebuffer() {
    // One-cycle mode, cycle 0 has framebufferColor true but NOT
    // numeratorOverflow (it's a passthrough instead, via A_ZERO): the
    // cycle-0 guard requires BOTH flags, so it must not fire, and since
    // blender_cycle_count != 2 the `else if` sub-tree is skipped entirely.
    let reqs = check_emulation_requirements(one_cycle_mode(
        P_FRAMEBUFFER,
        A_ZERO,
        P_COMBINED,
        B_ONE_MINUS_A,
    ));
    assert!(reqs.cycles[0].framebuffer_color);
    assert!(!reqs.cycles[0].numerator_overflow);
    assert!(reqs.simple_emulation);
}

#[test]
fn simple_emulation_two_cycle_short_circuits_before_checking_cycle1() {
    // Cycle 0 alone satisfies numeratorOverflow && framebufferColor even
    // though blender_cycle_count == 2; cycle 1 is left fully default
    // (matching, would NOT independently trigger the two-cycle sub-tree).
    // simple_emulation must still be false via the first branch, not the
    // `else if`.
    let reqs = check_emulation_requirements(two_cycle_mode(
        P_FRAMEBUFFER,
        A_SHADE,
        P_COMBINED,
        B_ONE,
        // cycle 2: all zero -> duplicateInput1MA true -> passthrough, not
        // numeratorOverflow; framebufferColor false. Would not itself
        // trigger simple_emulation = false.
        0,
        0,
        0,
        0,
    ));
    assert!(!reqs.simple_emulation);
}

#[test]
fn simple_emulation_two_cycle_first_branch_via_cycle0_framebuffer_non_passthrough() {
    // Cycle 0 does NOT satisfy numeratorOverflow && framebufferColor (it's
    // framebufferColor via P==Framebuffer, but passthrough via A==Zero, so
    // numeratorOverflow is false) -- so the first `if` fails and control
    // reaches `else if (blenderCycleCount == 2)`. There, cycle 0's
    // `framebufferColor && !passthrough` must ALSO be false (passthrough is
    // true here), so this sub-branch does not itself fire this test's
    // result; use a case where passthrough is false instead, isolating the
    // `framebufferColor && !passthrough` branch cleanly.
    let reqs = check_emulation_requirements(two_cycle_mode(
        // cycle 1: P=Framebuffer, framebufferColor true; not passthrough
        // (A=Shade non-zero, B=One non-zero, P!=M) and not
        // numeratorOverflow-qualifying for the FIRST guard because that
        // guard also requires numeratorOverflow, which IS true here (B !=
        // OneMinusA) -- so to isolate the second branch we need
        // numeratorOverflow false on cycle 0. Use B == OneMinusA on cycle 0
        // instead so the first guard's numeratorOverflow conjunct is false.
        P_FRAMEBUFFER,
        A_SHADE,
        P_COMBINED,
        B_ONE_MINUS_A,
        0,
        0,
        0,
        0,
    ));
    // Cycle 0: P=Framebuffer, M=Combined, A=Shade, B=OneMinusA.
    // anyInputIsZero: A!=Zero, B!=Zero -> false. duplicateInput1MA: P!=M ->
    // false. So passthrough=false. numeratorOverflow: B==OneMinusA -> `B !=
    // B_ONE_MINUS_A` is false -> numeratorOverflow=false. framebufferColor:
    // P==Framebuffer -> true.
    assert!(!reqs.cycles[0].passthrough);
    assert!(!reqs.cycles[0].numerator_overflow);
    assert!(reqs.cycles[0].framebuffer_color);
    // First guard (numeratorOverflow && framebufferColor) is false
    // (numeratorOverflow false) -> falls to `else if (count==2)` ->
    // `framebufferColor && !passthrough` == `true && !false` == true ->
    // simple_emulation = false.
    assert!(!reqs.simple_emulation);
}

#[test]
fn simple_emulation_two_cycle_reaches_cycle1_branch_when_cycle0_is_clean() {
    // Cycle 0: fully clean passthrough (all zero -> duplicateInput1MA,
    // framebufferColor false) so both prior branches are skipped. Cycle 1:
    // numeratorOverflow && framebufferColor both true -> the innermost
    // `else if` must fire.
    let reqs = check_emulation_requirements(two_cycle_mode(
        0,
        0,
        0,
        0,
        P_FRAMEBUFFER,
        A_SHADE,
        P_COMBINED,
        B_ONE,
    ));
    assert!(!reqs.cycles[0].framebuffer_color);
    assert!(reqs.cycles[1].numerator_overflow);
    assert!(reqs.cycles[1].framebuffer_color);
    assert!(!reqs.simple_emulation);
}

#[test]
fn simple_emulation_two_cycle_stays_true_when_neither_sub_branch_fires() {
    // Cycle 0: clean passthrough (framebufferColor false). Cycle 1: clean
    // passthrough too (framebufferColor false, so its guard can't fire
    // either). simple_emulation must stay true.
    let reqs = check_emulation_requirements(two_cycle_mode(0, 0, 0, 0, 0, 0, 0, 0));
    assert!(reqs.simple_emulation);
}

// ---------------------------------------------------------------------
// Approximation search gating: !simple_emulation, then blender_cycle_count == 2
// ---------------------------------------------------------------------

#[test]
fn approximation_stays_none_when_simple_emulation_is_true() {
    // Two-cycle, both cycles clean passthrough -> simple_emulation true ->
    // the whole approximation block is skipped -> approximate_emulation
    // stays at its untouched default (None).
    let reqs = check_emulation_requirements(two_cycle_mode(0, 0, 0, 0, 0, 0, 0, 0));
    assert!(reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
}

#[test]
fn approximation_none_via_default_when_not_simple_and_one_cycle() {
    // One-cycle mode reaching simple_emulation = false purely via the
    // cycle-0-alone hazard path (blender_cycle_count == 1, so the
    // `if (blenderCycleCount == 2)` approximation gate's body never runs).
    // approximate_emulation must equal the untouched EmulationRequirements
    // default, not a value computed by evaluating either named pattern.
    let reqs =
        check_emulation_requirements(one_cycle_mode(P_FRAMEBUFFER, A_SHADE, P_COMBINED, B_ONE));
    assert!(!reqs.simple_emulation);
    assert_eq!(blend_cycle_count(one_cycle_mode(0, 0, 0, 0)), 1);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
    assert_eq!(
        reqs.approximate_emulation,
        EmulationRequirements::default().approximate_emulation
    );
}

#[test]
fn approximation_zero_cycle_mode_stays_simple_and_none() {
    // blender_cycle_count == 0: the per-cycle loop never runs, so both
    // cycles are fully default (framebufferColor false everywhere) --
    // simple_emulation's first guard cannot fire (numerator_overflow is
    // false), and blender_cycle_count != 2 so the `else if` sub-tree is
    // skipped too. simple_emulation stays true, so the approximation block
    // is skipped by its own `!simple_emulation` gate.
    let reqs = check_emulation_requirements(zero_cycle_mode());
    assert!(reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
}

// ---------------------------------------------------------------------
// Approximation::CombinerFramebuffer1MA_SquareMix (exact 8-selector pattern)
// ---------------------------------------------------------------------

/// Both cycles: P=Combined, M=Framebuffer, A=Combined(CC_ALPHA), B=OneMinusA.
/// This is a two-cycle mode where cycle 0 is `framebufferColor && !passthrough`
/// (M==Framebuffer -> framebufferColor; P!=M and A!=Zero,B!=Zero ->
/// !passthrough), so simple_emulation is guaranteed false via the two-cycle
/// `else if` branch, making the approximation search reachable.
fn square_mix_pattern_mode() -> OtherMode {
    two_cycle_mode(
        P_COMBINED,
        A_COMBINED,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_COMBINED,
        A_COMBINED,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    )
}

#[test]
fn square_mix_pattern_mode_is_not_simple_emulation() {
    let reqs = check_emulation_requirements(square_mix_pattern_mode());
    assert!(!reqs.simple_emulation);
}

#[test]
fn square_mix_pattern_matches_exact_selector_tuple() {
    let reqs = check_emulation_requirements(square_mix_pattern_mode());
    assert_eq!(
        reqs.approximate_emulation,
        Approximation::CombinerFramebuffer1MA_SquareMix
    );
}

#[test]
fn square_mix_pattern_requires_exact_a0_and_a1_cc_alpha() {
    // Move A0 away from A_CC_ALPHA (Combined) to A_SHADE; every other
    // selector unchanged from the matching pattern. The square-mix pattern
    // must stop matching (proving A0's constraint is real, not vestigial),
    // and since the mode stays framebufferColor && !passthrough on cycle 0
    // (M still Framebuffer, A now Shade != Zero, B still OneMinusA -> P==M?
    // P=Combined, M=Framebuffer -> P!=M, so duplicateInput1MA false; A!=
    // Zero, B!=Zero -> anyInputIsZero false -> passthrough false), it still
    // reaches the approximation search.
    let m = two_cycle_mode(
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_COMBINED,
        A_COMBINED,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    );
    let reqs = check_emulation_requirements(m);
    assert!(!reqs.simple_emulation);
    assert_ne!(
        reqs.approximate_emulation,
        Approximation::CombinerFramebuffer1MA_SquareMix
    );
}

#[test]
fn square_mix_pattern_breaks_when_a1_is_not_cc_alpha() {
    let m = two_cycle_mode(
        P_COMBINED,
        A_COMBINED,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    );
    let reqs = check_emulation_requirements(m);
    assert_ne!(
        reqs.approximate_emulation,
        Approximation::CombinerFramebuffer1MA_SquareMix
    );
}

// ---------------------------------------------------------------------
// Approximation::AnyFramebuffer1MA_MultiplyMix (asymmetric: ignores A0/A1;
// first comparison is an inequality)
// ---------------------------------------------------------------------

/// Cycle 0: P=Blend (!= Framebuffer, satisfying the inequality), M=Framebuffer,
/// B=OneMinusA. Cycle 1: P=Combined, M=Framebuffer, B=OneMinusA. A0/A1 left
/// at A_SHADE (deliberately NOT A_CC_ALPHA) to prove the pattern doesn't
/// depend on them. Cycle 0 (P=Blend, M=Framebuffer, A=Shade, B=OneMinusA):
/// P!=M -> duplicateInput1MA false; A!=Zero, B!=Zero -> anyInputIsZero
/// false -> passthrough false; M==Framebuffer -> framebufferColor true, so
/// this mode is not-simple via the two-cycle `framebufferColor &&
/// !passthrough` branch.
fn multiply_mix_pattern_mode_with_a(a0: u32, a1: u32) -> OtherMode {
    two_cycle_mode(
        P_BLEND,
        a0,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_COMBINED,
        a1,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    )
}

#[test]
fn multiply_mix_pattern_mode_is_not_simple_emulation() {
    let reqs = check_emulation_requirements(multiply_mix_pattern_mode_with_a(A_SHADE, A_SHADE));
    assert!(!reqs.simple_emulation);
}

#[test]
fn multiply_mix_pattern_matches_exact_selector_tuple() {
    let reqs = check_emulation_requirements(multiply_mix_pattern_mode_with_a(A_SHADE, A_SHADE));
    assert_eq!(
        reqs.approximate_emulation,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );
}

#[test]
fn any_framebuffer_pattern_ignores_a0_and_a1_entirely() {
    // Vary A0/A1 across the three BlendAlphaInput wire values that do NOT
    // also trip `anyInputIsZero` on cycle 0 (A_ZERO is excluded here -- see
    // `a0_equal_zero_breaks_the_case_via_cycle0_passthrough_not_via_the_pattern`
    // below for that boundary handled separately), holding every other
    // selector at the matching pattern. The match must be unaffected in
    // every one of these combinations, proving the header's
    // `AnyFramebuffer1MA_MultiplyMix` condition truly never reads A0/A1 --
    // confirmed independently and unconditionally by
    // `is_any_framebuffer_1ma_multiply_mix_direct`, which calls the
    // predicate with no `a0`/`a1` parameters at all.
    for a0 in [A_COMBINED, 1 /* Fog */, A_SHADE] {
        for a1 in [A_COMBINED, 1 /* Fog */, A_SHADE] {
            let reqs = check_emulation_requirements(multiply_mix_pattern_mode_with_a(a0, a1));
            assert_eq!(
                reqs.approximate_emulation,
                Approximation::AnyFramebuffer1MA_MultiplyMix,
                "a0={a0} a1={a1} should not affect the multiply-mix match"
            );
        }
    }
}

#[test]
fn a0_equal_zero_breaks_the_case_via_cycle0_passthrough_not_via_the_pattern() {
    // At a0 == A_ZERO, cycle 0's `anyInputIsZero` becomes true (A == Zero),
    // making cycle 0 a passthrough. That flips `simple_emulation`'s
    // two-cycle sub-branch condition (`framebufferColor && !passthrough`)
    // to false, so `simple_emulation` stays true and the whole
    // approximation search is skipped -- `approximate_emulation` reports
    // `None` via its untouched default, exactly like
    // `approximation_stays_none_when_simple_emulation_is_true`. This is a
    // real, correct behavior difference at a0 == A_ZERO, but it flows
    // through `simple_emulation`'s gate, not through the
    // `AnyFramebuffer1MA_MultiplyMix` predicate reading A0 (which it does
    // not -- see `is_any_framebuffer_1ma_multiply_mix_direct`). Distinguishing
    // these two causes is exactly why this module's test suite checks the
    // pattern predicate directly as well as through the full function.
    let reqs = check_emulation_requirements(multiply_mix_pattern_mode_with_a(A_ZERO, A_SHADE));
    assert!(reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
}

#[test]
fn multiply_mix_pattern_p0_inequality_rejects_framebuffer_but_allows_everything_else() {
    // P0 == Framebuffer must break the pattern (the header's condition is
    // `P0 != PM_FRAMEBUFFER_COLOR`).
    let m = two_cycle_mode(
        P_FRAMEBUFFER,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    );
    let reqs = check_emulation_requirements(m);
    assert_ne!(
        reqs.approximate_emulation,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );

    // P0 == Blend (not Combined, not Framebuffer) still satisfies the
    // inequality and must still match -- proving P0's constraint really is
    // "not Framebuffer", not "must equal a specific other value".
    let reqs_blend =
        check_emulation_requirements(multiply_mix_pattern_mode_with_a(A_SHADE, A_SHADE));
    assert_eq!(
        reqs_blend.approximate_emulation,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );
}

#[test]
fn multiply_mix_pattern_p1_must_be_combined_exactly() {
    let m = two_cycle_mode(
        P_BLEND,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
        P_BLEND, // P1 should be Combined, not Blend
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    );
    let reqs = check_emulation_requirements(m);
    assert_ne!(
        reqs.approximate_emulation,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );
}

// ---------------------------------------------------------------------
// Pattern precedence: square-mix checked strictly before multiply-mix.
// ---------------------------------------------------------------------

#[test]
fn square_mix_pattern_is_checked_before_multiply_mix_pattern() {
    // Construct an input satisfying BOTH named patterns simultaneously:
    // P0=Combined (satisfies square's P0==Combined AND multiply's P0!=
    // Framebuffer), M0=Framebuffer, A0=Combined(CC_ALPHA), B0=OneMinusA;
    // P1=Combined, M1=Framebuffer, A1=Combined(CC_ALPHA), B1=OneMinusA.
    // This is exactly `square_mix_pattern_mode()`. If precedence were
    // reversed, this would report AnyFramebuffer1MA_MultiplyMix instead.
    let reqs = check_emulation_requirements(square_mix_pattern_mode());
    assert_eq!(
        reqs.approximate_emulation,
        Approximation::CombinerFramebuffer1MA_SquareMix
    );
}

#[test]
fn neither_pattern_matches_falls_through_to_explicit_none() {
    // Two-cycle, not-simple (cycle 0 framebufferColor && !passthrough), but
    // selectors matching neither named pattern (e.g. B0 != OneMinusA breaks
    // both patterns' B0 requirement).
    let m = two_cycle_mode(
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE, // B0 != OneMinusA breaks both patterns
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A,
    );
    let reqs = check_emulation_requirements(m);
    assert!(!reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
}

// ---------------------------------------------------------------------
// Direct pattern-predicate unit tests (bypassing check_emulation_requirements)
// ---------------------------------------------------------------------

#[test]
fn is_combiner_framebuffer_1ma_square_mix_direct() {
    assert!(is_combiner_framebuffer_1ma_square_mix(
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendAlphaInput::Combined,
        BlendBInput::OneMinusA,
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendAlphaInput::Combined,
        BlendBInput::OneMinusA,
    ));
    assert!(!is_combiner_framebuffer_1ma_square_mix(
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendAlphaInput::Shade, // A0 wrong
        BlendBInput::OneMinusA,
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendAlphaInput::Combined,
        BlendBInput::OneMinusA,
    ));
}

#[test]
fn is_any_framebuffer_1ma_multiply_mix_direct() {
    assert!(is_any_framebuffer_1ma_multiply_mix(
        BlendColorInput::Blend, // != Framebuffer
        BlendColorInput::Framebuffer,
        BlendBInput::OneMinusA,
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendBInput::OneMinusA,
    ));
    assert!(!is_any_framebuffer_1ma_multiply_mix(
        BlendColorInput::Framebuffer, // P0 == Framebuffer breaks it
        BlendColorInput::Framebuffer,
        BlendBInput::OneMinusA,
        BlendColorInput::Combined,
        BlendColorInput::Framebuffer,
        BlendBInput::OneMinusA,
    ));
}

// ---------------------------------------------------------------------
// Out-of-range / ordinal considerations
// ---------------------------------------------------------------------

#[test]
fn all_2bit_wire_selector_encodings_are_defined_no_reserved_value() {
    // InputPM/InputA/InputB remain the totally-defined 2-bit wire fields
    // M4.4 already documented; this module reuses their from_wire and adds
    // no new wire decode. Confirm every 2-bit pattern still maps to a
    // defined variant via the shared enums this module imports.
    for v in 0..4u8 {
        let _ = BlendColorInput::from_wire(v);
        let _ = BlendAlphaInput::from_wire(v);
        let _ = BlendBInput::from_wire(v);
    }
}

#[test]
fn cycle_type_reserved_ordinals_copy_and_fill_both_yield_blend_cycle_count_0() {
    // combine_cycle_count's else-catchall (owned by rt64_blender_analysis,
    // M4.4) means blend_cycle_count is 0 for both Copy and Fill regardless
    // of force_blend's subtraction guard -- check_emulation_requirements's
    // loop then never runs, matching the zero_cycle_mode() cases above.
    let copy_reqs = check_emulation_requirements(mode(2, false, 0, 0, 0, 0, 0, 0, 0, 0));
    let fill_reqs = check_emulation_requirements(mode(3, false, 0, 0, 0, 0, 0, 0, 0, 0));
    assert_eq!(copy_reqs, fill_reqs);
    assert!(copy_reqs.simple_emulation);
    assert_eq!(copy_reqs.approximate_emulation, Approximation::None);
}

// ---------------------------------------------------------------------
// Additional per-cycle-2 classification checks (mirroring the cycle-0
// coverage above, using two_cycle_mode with a clean cycle 0 so cycle 1's
// classification is isolated).
// ---------------------------------------------------------------------

#[test]
fn cycle1_passthrough_when_alpha_a_is_zero() {
    let reqs = check_emulation_requirements(two_cycle_mode(
        0, 0, 0, 0, P_COMBINED, A_ZERO, P_COMBINED, B_ONE,
    ));
    assert!(reqs.cycles[1].passthrough);
    assert!(!reqs.cycles[1].numerator_overflow);
}

#[test]
fn cycle1_numerator_overflow_when_b_is_not_one_minus_a_and_not_passthrough() {
    let reqs = check_emulation_requirements(two_cycle_mode(
        0,
        0,
        0,
        0,
        P_COMBINED,
        A_SHADE,
        P_FRAMEBUFFER,
        B_ONE,
    ));
    assert!(!reqs.cycles[1].passthrough);
    assert!(reqs.cycles[1].numerator_overflow);
}

#[test]
fn cycle1_duplicate_1ma_when_p_equals_m_and_b_is_one_minus_a() {
    let reqs = check_emulation_requirements(two_cycle_mode(
        0,
        0,
        0,
        0,
        P_BLEND,
        A_SHADE,
        P_BLEND,
        B_ONE_MINUS_A,
    ));
    assert!(reqs.cycles[1].passthrough);
    assert!(!reqs.cycles[1].numerator_overflow);
}

// ---------------------------------------------------------------------
// EmulationRequirements/Cycle derive sanity (equality is field-wise, not a
// pointer/identity comparison -- matters because tests above rely on
// assert_eq!/assert_ne! for Approximation and Cycle values).
// ---------------------------------------------------------------------

#[test]
fn emulation_requirements_equality_is_field_wise() {
    let a = check_emulation_requirements(one_cycle_mode(0, 0, 0, 0));
    let b = check_emulation_requirements(one_cycle_mode(0, 0, 0, 0));
    let c = check_emulation_requirements(one_cycle_mode(P_FRAMEBUFFER, A_SHADE, P_COMBINED, B_ONE));
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn approximation_variants_are_pairwise_distinct() {
    assert_ne!(
        Approximation::None,
        Approximation::CombinerFramebuffer1MA_SquareMix
    );
    assert_ne!(
        Approximation::None,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );
    assert_ne!(
        Approximation::CombinerFramebuffer1MA_SquareMix,
        Approximation::AnyFramebuffer1MA_MultiplyMix
    );
}

// ---------------------------------------------------------------------
// One-cycle mode can never reach the Approximation search, even when
// cycle-1-shaped selectors (unreachable, since blender_cycle_count == 1)
// happen to encode a matching pattern -- guards against a loop-bound
// regression that would read cycles[1] despite blender_cycle_count == 1.
// ---------------------------------------------------------------------

#[test]
fn one_cycle_mode_never_reaches_approximation_search_regardless_of_unreachable_cycle1_bits() {
    // Build directly via `mode()` (not `one_cycle_mode`) so cycle 2's wire
    // bits are set to the square-mix pattern's cycle-2 half, even though
    // blend_cycle_count == 1 means cycle 2 is never decoded.
    let m = mode(
        0,
        true, // OneCycle, force_blend -> blend_cycle_count == 1
        P_FRAMEBUFFER,
        A_SHADE,
        P_COMBINED,
        B_ONE, // cycle 1: not-simple trigger
        P_COMBINED,
        A_COMBINED,
        P_FRAMEBUFFER,
        B_ONE_MINUS_A, // cycle 2: unreachable
    );
    assert_eq!(blend_cycle_count(m), 1);
    let reqs = check_emulation_requirements(m);
    assert!(!reqs.simple_emulation);
    assert_eq!(reqs.approximate_emulation, Approximation::None);
    assert_eq!(reqs.cycles[1], Cycle::default());
}
