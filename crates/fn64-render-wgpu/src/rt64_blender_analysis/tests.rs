//! Characterization tests for `rt64_blender_analysis`, pinning hand-computed
//! expectations (never values captured from this module's own output) for
//! every cycle-count input, every predicate's true/false sides, every
//! comparison's strictness boundary, out-of-range cycle-index ordinals, and
//! every asymmetric branch documented in the module's "Admitted domain".

use super::*;
use crate::blend::{BlendAlphaInput, BlendBInput, BlendColorInput};
use crate::state::OtherMode;

/// Build an `OtherMode` selecting `cycle_type_bits` (0=OneCycle, 1=TwoCycle,
/// 2=Copy, 3=Fill) with `force_blend` and both cycles' four selectors set
/// directly by wire value, per `state.rs`'s documented absolute-bit layout:
/// cycle 1 = {color_a:30-31, alpha_a:26-27, color_b:22-23, alpha_b:18-19},
/// cycle 2 = {color_a:28-29, alpha_a:24-25, color_b:20-21, alpha_b:16-17}.
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

/// All selectors zeroed (P=M=Combined/CC_OR_BLENDER, A=Combined/CC_ALPHA,
/// B=OneMinusA) for both cycles -- a convenience base for tests that only
/// care about cycle type / force-blend.
fn mode_simple(cycle_type_bits: u32, force_blend: bool) -> OtherMode {
    mode(cycle_type_bits, force_blend, 0, 0, 0, 0, 0, 0, 0, 0)
}

// ---------------------------------------------------------------------
// combine_cycle_count
// ---------------------------------------------------------------------

#[test]
fn combine_cycle_count_two_cycle_is_2() {
    assert_eq!(combine_cycle_count(mode_simple(1, false)), 2);
}

#[test]
fn combine_cycle_count_one_cycle_is_1() {
    assert_eq!(combine_cycle_count(mode_simple(0, false)), 1);
}

#[test]
fn combine_cycle_count_copy_is_0_via_else_catchall() {
    assert_eq!(combine_cycle_count(mode_simple(2, false)), 0);
}

#[test]
fn combine_cycle_count_fill_is_0_via_same_else_catchall_as_copy() {
    // Copy and Fill are NOT distinguished by the header's if/else if/else --
    // both fall into the same `else` branch returning 0. Pin both to the
    // same code path's output, not merely the same numeric result.
    assert_eq!(combine_cycle_count(mode_simple(3, false)), 0);
    assert_eq!(
        combine_cycle_count(mode_simple(2, false)),
        combine_cycle_count(mode_simple(3, false))
    );
}

#[test]
fn combine_cycle_count_ignores_force_blend() {
    assert_eq!(combine_cycle_count(mode_simple(1, true)), 2);
    assert_eq!(combine_cycle_count(mode_simple(0, true)), 1);
}

// ---------------------------------------------------------------------
// blend_cycle_count
// ---------------------------------------------------------------------

#[test]
fn blend_cycle_count_force_blend_returns_cc_count_unchanged_at_0() {
    // ccCount == 0 (Copy), forceBlend true -> returns ccCount (0), not the
    // subtract-one branch.
    assert_eq!(blend_cycle_count(mode_simple(2, true)), 0);
}

#[test]
fn blend_cycle_count_force_blend_returns_cc_count_unchanged_at_1() {
    assert_eq!(blend_cycle_count(mode_simple(0, true)), 1);
}

#[test]
fn blend_cycle_count_force_blend_returns_cc_count_unchanged_at_2() {
    assert_eq!(blend_cycle_count(mode_simple(1, true)), 2);
}

#[test]
fn blend_cycle_count_no_force_blend_at_ccount_0_stays_0_no_underflow() {
    // Strict `> 0` guard: ccCount == 0, `0 > 0` false, else-arm literal 0.
    assert_eq!(blend_cycle_count(mode_simple(2, false)), 0);
    assert_eq!(blend_cycle_count(mode_simple(3, false)), 0);
}

#[test]
fn blend_cycle_count_no_force_blend_at_ccount_1_subtracts_to_0() {
    assert_eq!(blend_cycle_count(mode_simple(0, false)), 0);
}

#[test]
fn blend_cycle_count_no_force_blend_at_ccount_2_subtracts_to_1() {
    assert_eq!(blend_cycle_count(mode_simple(1, false)), 1);
}

// ---------------------------------------------------------------------
// cycle_selectors decode correctness (via the public predicates), pinning
// the P=color_a / M=color_b / A=alpha_a / B=alpha_b mapping and the
// cycle_1==first / cycle_2==second correspondence.
// ---------------------------------------------------------------------

#[test]
fn uses_standard_fog_cycle_at_reads_cycle_1_for_index_0() {
    // Cycle 1: P=Fog(3), A=Shade(2), M=Combined(0), B=OneMinusA(0) -- exact
    // standard-fog pattern on cycle 1 only; cycle 2 left mismatched (all 0,
    // which fails the P==Fog check).
    let m = mode(0, false, 3, 2, 0, 0, 0, 0, 0, 0);
    assert!(uses_standard_fog_cycle_at(m, 0));
}

#[test]
fn uses_standard_fog_cycle_at_reads_cycle_2_for_index_1() {
    // Standard-fog pattern placed on cycle 2 only; cycle 1 mismatched.
    let m = mode(1, false, 0, 0, 0, 0, 3, 2, 0, 0);
    assert!(uses_standard_fog_cycle_at(m, 1));
    // Cycle 1 (index 0) must NOT match -- proves index 0 reads cycle 1, not
    // cycle 2.
    assert!(!uses_standard_fog_cycle_at(m, 0));
}

// ---------------------------------------------------------------------
// uses_input / uses_combiner_alpha
// ---------------------------------------------------------------------

#[test]
fn uses_input_false_when_blend_cycle_count_is_0() {
    // Copy mode: blendCycleCount == 0 regardless of force_blend, loop body
    // never runs.
    let m = mode(2, true, 0, 0, 0, 0, 0, 0, 0, 0);
    assert!(!uses_input(m, BlendAlphaInput::Combined));
}

#[test]
fn uses_combiner_alpha_true_when_cycle_1_alpha_a_is_cc_alpha() {
    // TwoCycle, no force_blend -> blendCycleCount == 2-1 == 1, loop checks
    // c=0 (first cycle, "c > 0" == false). alpha_a = 0 (A_CC_ALPHA).
    let m = mode(1, false, 0, 0, 0, 0, 0, 0, 0, 0);
    assert!(uses_combiner_alpha(m));
}

#[test]
fn uses_combiner_alpha_false_when_cycle_1_alpha_a_is_not_cc_alpha() {
    let m = mode(1, false, 0, /* alpha_a = */ 3, 0, 0, 0, 0, 0, 0);
    assert!(!uses_combiner_alpha(m));
}

#[test]
fn uses_combiner_alpha_checks_second_cycle_when_two_cycle() {
    // TwoCycle, no force_blend -> blendCycleCount == 1 (2-1), loop runs
    // c=0 only -> checks cycle 1 (c>0 is false), NOT cycle 2. Put CC_ALPHA
    // only on cycle 2's alpha_a to prove cycle 2 is not consulted for this
    // one-iteration case.
    let m = mode(1, false, 0, 3, 0, 0, 0, 0, 0, 0);
    assert!(!uses_combiner_alpha(m));
}

#[test]
fn uses_combiner_alpha_checks_both_cycles_when_force_blend_two_cycle() {
    // TwoCycle + force_blend -> blendCycleCount == 2, loop runs c=0 (cycle
    // 1, mismatched alpha_a=3) then c=1 (cycle 2, alpha_a=0 == CC_ALPHA) ->
    // true, proving the loop reaches the second iteration and cycle 2 is
    // consulted via `c > 0`.
    let m = mode(1, true, 0, 3, 0, 0, 0, 0, 0, 0);
    assert!(uses_combiner_alpha(m));
}

#[test]
fn uses_combiner_alpha_returns_on_first_match_cycle_1_true() {
    let m = mode(1, true, 0, 0, 0, 0, 0, 3, 0, 0);
    assert!(uses_combiner_alpha(m));
}

// ---------------------------------------------------------------------
// uses_alpha_blend_cycle -- the asymmetric all_inputs branch
// ---------------------------------------------------------------------

#[test]
fn uses_alpha_blend_cycle_all_inputs_true_via_p_framebuffer_and_a_nonzero() {
    // P=Framebuffer(1), A=Fog(1, != Zero) on cycle 1.
    let m = mode(0, false, 1, 1, 0, 0, 0, 0, 0, 0);
    assert!(uses_alpha_blend_cycle(m, false, true));
}

#[test]
fn uses_alpha_blend_cycle_all_inputs_false_when_p_framebuffer_but_a_zero() {
    // P=Framebuffer(1), A=Zero(3) on cycle 1 -- P branch requires A != Zero.
    // M/B left at 0/0 (M=Combined != Framebuffer), so no match via the M/B
    // check either.
    let m = mode(0, false, 1, 3, 0, 0, 0, 0, 0, 0);
    assert!(!uses_alpha_blend_cycle(m, false, true));
}

#[test]
fn uses_alpha_blend_cycle_all_inputs_true_via_m_framebuffer_and_b_nonzero() {
    // P=Combined(0, not Framebuffer, so first check false), M=Framebuffer(1),
    // B=One(2, != Zero) on cycle 1.
    let m = mode(0, false, 0, 0, 1, 2, 0, 0, 0, 0);
    assert!(uses_alpha_blend_cycle(m, false, true));
}

#[test]
fn uses_alpha_blend_cycle_all_inputs_false_when_m_framebuffer_but_b_zero() {
    let m = mode(0, false, 0, 0, 1, 3, 0, 0, 0, 0);
    assert!(!uses_alpha_blend_cycle(m, false, true));
}

#[test]
fn uses_alpha_blend_cycle_not_all_inputs_true_via_p_framebuffer_alone() {
    let m = mode(0, false, 1, 0, 0, 0, 0, 0, 0, 0);
    assert!(uses_alpha_blend_cycle(m, false, false));
}

#[test]
fn uses_alpha_blend_cycle_not_all_inputs_ignores_m_framebuffer_even_with_b_nonzero() {
    // The exact asymmetry: M=Framebuffer + B=One would trigger `true` under
    // all_inputs=true (proven above), but under all_inputs=false only P is
    // even decoded, so this must stay false. P=Combined(0).
    let m = mode(0, false, 0, 0, 1, 2, 0, 0, 0, 0);
    assert!(!uses_alpha_blend_cycle(m, false, false));
}

#[test]
fn uses_alpha_blend_cycle_second_cycle_reads_cycle_2() {
    // P=Framebuffer on cycle 2 only; cycle 1 left at Combined.
    let m = mode(1, false, 0, 0, 0, 0, 1, 0, 0, 0);
    assert!(uses_alpha_blend_cycle(m, true, false));
    assert!(!uses_alpha_blend_cycle(m, false, false));
}

// ---------------------------------------------------------------------
// uses_alpha_blend -- sequential (not else-if) guards, uses
// combine_cycle_count not blend_cycle_count
// ---------------------------------------------------------------------

#[test]
fn uses_alpha_blend_false_at_ccount_0() {
    let m = mode(2, true, 1, 1, 0, 0, 1, 1, 0, 0);
    assert!(!uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_1_checks_cycle_1_with_all_inputs_from_force_blend_only() {
    // OneCycle (ccCount=1): the ccCount>=2 guard never fires. The ccCount>=1
    // guard calls uses_alpha_blend_cycle(secondCycle=false, allInputs =
    // (ccCount>=2)||forceBlend = false||forceBlend). With force_blend=false,
    // allInputs=false, so only P is checked. Put P=Framebuffer on cycle 1.
    let m = mode(0, false, 1, 0, 0, 0, 0, 0, 0, 0);
    assert!(uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_1_with_force_blend_uses_all_inputs_on_cycle_1() {
    // force_blend=true makes allInputs=true for the ccCount>=1 check. Trigger
    // via the M/B all-inputs path only (P not Framebuffer), which requires
    // all_inputs=true to be reachable at all.
    let m = mode(0, true, 0, 0, 1, 2, 0, 0, 0, 0);
    assert!(uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_2_first_guard_matches_cycle_2_returns_true() {
    // TwoCycle (ccCount=2), force_blend=false. First guard: ccCount>=2 true,
    // checks cycle 2 (secondCycle=true) with all_inputs=forceBlend=false ->
    // only P on cycle 2 matters. Cycle 1 deliberately has no match so the
    // second guard alone would return false, isolating that the FIRST guard
    // is what returns true here.
    let m = mode(1, false, 0, 0, 0, 0, 1, 0, 0, 0);
    assert!(uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_2_first_guard_false_falls_through_to_second_guard() {
    // TwoCycle, force_blend=false. Cycle 2 has no Framebuffer P (first guard
    // false). Cycle 1 has P=Framebuffer; second guard's all_inputs =
    // (ccCount>=2)||forceBlend = true||false = true, so cycle 1 is checked
    // with all_inputs=true. P=Framebuffer alone (via all_inputs=true's first
    // check, A != Zero) satisfies it. This proves execution falls through
    // from a false first guard to evaluate the second guard, not short-
    // circuited by the first guard's condition being true.
    let m = mode(1, false, 1, 1, 0, 0, 0, 0, 0, 0);
    assert!(uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_2_both_guards_false_is_false() {
    let m = mode(1, false, 0, 0, 0, 0, 0, 0, 0, 0);
    assert!(!uses_alpha_blend(m));
}

#[test]
fn uses_alpha_blend_ccount_2_second_guard_all_inputs_forced_true_even_without_force_blend() {
    // Confirms `(ccCount >= 2) || forceBlend` -- the second guard's
    // all_inputs is true purely from ccCount>=2, independent of forceBlend,
    // when the first guard's cycle-2 check does not itself match. Reuses the
    // M/B all-inputs path (which requires all_inputs=true) on cycle 1 to
    // prove all_inputs really is true here despite forceBlend=false.
    let m = mode(1, false, 0, 0, 1, 2, 0, 0, 0, 0);
    assert!(uses_alpha_blend(m));
}

// ---------------------------------------------------------------------
// uses_standard_fog_cycle (aggregate) -- boundary on cycle count, early
// return on first match
// ---------------------------------------------------------------------

#[test]
fn uses_standard_fog_cycle_false_when_blend_cycle_count_is_0() {
    // Copy mode: blendCycleCount == 0, loop never runs even though cycle 1
    // is set to the exact standard-fog pattern.
    let m = mode(2, false, 3, 2, 0, 0, 0, 0, 0, 0);
    assert!(!uses_standard_fog_cycle(m));
}

#[test]
fn uses_standard_fog_cycle_true_via_cycle_1_only_iteration() {
    // TwoCycle, no force_blend -> blendCycleCount == 2-1 == 1, loop reaches
    // only c=0 (cycle 1).
    let m = mode(1, false, 3, 2, 0, 0, 0, 0, 0, 0);
    assert!(uses_standard_fog_cycle(m));
}

#[test]
fn uses_standard_fog_cycle_requires_all_four_selectors_exact() {
    // P correct, A correct, M correct, B wrong (One=2 instead of
    // OneMinusA=0).
    let m = mode(1, false, 3, 2, 0, 2, 0, 0, 0, 0);
    assert!(!uses_standard_fog_cycle(m));
}

#[test]
fn uses_standard_fog_cycle_true_via_cycle_2_when_two_cycle_force_blend() {
    // TwoCycle + force_blend -> blendCycleCount == 2, loop reaches c=1
    // (cycle 2). Cycle 1 deliberately mismatched.
    let m = mode(1, true, 0, 0, 0, 0, 3, 2, 0, 0);
    assert!(uses_standard_fog_cycle(m));
}

#[test]
fn uses_standard_fog_cycle_cycle_2_pattern_not_seen_when_loop_stops_after_one_iteration() {
    // TwoCycle, no force_blend -> blendCycleCount == 1, loop only reaches
    // c=0 (cycle 1). The fog pattern is placed on cycle 2 only, so it must
    // NOT be detected.
    let m = mode(1, false, 0, 0, 0, 0, 3, 2, 0, 0);
    assert!(!uses_standard_fog_cycle(m));
}

// ---------------------------------------------------------------------
// uses_visualize_coverage_cycle (aggregate) -- P is never checked
// ---------------------------------------------------------------------

#[test]
fn uses_visualize_coverage_cycle_false_when_blend_cycle_count_is_0() {
    let m = mode(3, false, 0, 3, 2, 1, 0, 0, 0, 0);
    assert!(!uses_visualize_coverage_cycle(m));
}

#[test]
fn uses_visualize_coverage_cycle_true_via_cycle_1() {
    // TwoCycle, no force_blend -> blendCycleCount == 2-1 == 1, loop reaches
    // only c=0 (cycle 1). A=Zero(3), M=Blend(2), B=FramebufferAlpha(1).
    let m = mode(1, false, 0, 3, 2, 1, 0, 0, 0, 0);
    assert!(uses_visualize_coverage_cycle(m));
}

#[test]
fn uses_visualize_coverage_cycle_ignores_p_value_entirely() {
    // Same A/M/B pattern as above, but sweep P across all four values --
    // predicate must stay true regardless, since P is never read by this
    // predicate.
    for p in 0..4u32 {
        let m = mode(0, false, p, 3, 2, 1, 0, 0, 0, 0);
        assert!(
            uses_visualize_coverage_cycle_at(m, 0),
            "P={p} must not affect the visualize-coverage predicate"
        );
    }
}

#[test]
fn uses_visualize_coverage_cycle_requires_all_three_checked_selectors_exact() {
    // A correct, M correct, B wrong (Zero=3 instead of FramebufferAlpha=1).
    let m = mode(0, false, 0, 3, 2, 3, 0, 0, 0, 0);
    assert!(!uses_visualize_coverage_cycle(m));
}

#[test]
fn uses_visualize_coverage_cycle_true_via_cycle_2_when_two_cycle_force_blend() {
    let m = mode(1, true, 0, 0, 0, 0, 0, 3, 2, 1);
    assert!(uses_visualize_coverage_cycle(m));
}

#[test]
fn uses_visualize_coverage_cycle_cycle_2_pattern_unreached_when_loop_stops_after_one_iteration() {
    let m = mode(1, false, 0, 0, 0, 0, 0, 3, 2, 1);
    assert!(!uses_visualize_coverage_cycle(m));
}

// ---------------------------------------------------------------------
// Two-arg overload: out-of-range cycle_index ordinal handling
// ---------------------------------------------------------------------

#[test]
fn uses_standard_fog_cycle_two_arg_treats_any_nonzero_cycle_index_as_second_cycle() {
    // Standard-fog pattern on cycle 2 only. cycle_index = 0 must read cycle
    // 1 (no match); cycle_index = 1, 2, and 5 (out-of-range for a
    // "which of two cycles" index) must all read cycle 2 identically, since
    // the header's guard is `cycleIndex > 0`, not `cycleIndex == 1`.
    let m = mode(1, false, 0, 0, 0, 0, 3, 2, 0, 0);
    assert!(!uses_standard_fog_cycle_at(m, 0));
    assert!(uses_standard_fog_cycle_at(m, 1));
    assert!(uses_standard_fog_cycle_at(m, 2));
    assert!(uses_standard_fog_cycle_at(m, 5));
}

#[test]
fn uses_visualize_coverage_cycle_two_arg_treats_any_nonzero_cycle_index_as_second_cycle() {
    let m = mode(1, false, 0, 0, 0, 0, 0, 3, 2, 1);
    assert!(!uses_visualize_coverage_cycle_at(m, 0));
    assert!(uses_visualize_coverage_cycle_at(m, 1));
    assert!(uses_visualize_coverage_cycle_at(m, 2));
    assert!(uses_visualize_coverage_cycle_at(m, 9));
}

// ---------------------------------------------------------------------
// cycle_selectors <-> BlendColorInput/BlendAlphaInput/BlendBInput enum
// coverage: every wire value maps to the expected variant (no reserved
// encodings in this 2-bit space, unlike AlphaCompare/TextureLutMode
// elsewhere in state.rs).
// ---------------------------------------------------------------------

#[test]
fn blend_color_input_from_wire_covers_all_four_pm_encodings() {
    assert_eq!(BlendColorInput::from_wire(0), BlendColorInput::Combined);
    assert_eq!(BlendColorInput::from_wire(1), BlendColorInput::Framebuffer);
    assert_eq!(BlendColorInput::from_wire(2), BlendColorInput::Blend);
    assert_eq!(BlendColorInput::from_wire(3), BlendColorInput::Fog);
}

#[test]
fn blend_alpha_input_from_wire_covers_all_four_a_encodings() {
    assert_eq!(BlendAlphaInput::from_wire(0), BlendAlphaInput::Combined);
    assert_eq!(BlendAlphaInput::from_wire(1), BlendAlphaInput::Fog);
    assert_eq!(BlendAlphaInput::from_wire(2), BlendAlphaInput::Shade);
    assert_eq!(BlendAlphaInput::from_wire(3), BlendAlphaInput::Zero);
}

#[test]
fn blend_b_input_from_wire_covers_all_four_b_encodings() {
    assert_eq!(BlendBInput::from_wire(0), BlendBInput::OneMinusA);
    assert_eq!(BlendBInput::from_wire(1), BlendBInput::FramebufferAlpha);
    assert_eq!(BlendBInput::from_wire(2), BlendBInput::One);
    assert_eq!(BlendBInput::from_wire(3), BlendBInput::Zero);
}

// ---------------------------------------------------------------------
// cycle_selectors reads the documented absolute bit ranges (cross-check
// against OtherMode::blender_cycle_1/2 directly), proving cycle 1 == first
// cycle / cycle 2 == second cycle and P=color_a / M=color_b / A=alpha_a /
// B=alpha_b as documented in "Admitted domain".
// ---------------------------------------------------------------------

#[test]
fn cycle_selectors_first_cycle_matches_blender_cycle_1_mapping() {
    let m = mode(0, false, 1, 2, 3, 0, /* cycle 2 (unused) */ 0, 0, 0, 0);
    let resolved = ResolvedBlendCycle::from_wire(m.blender_cycle_1());
    assert_eq!(resolved.p, BlendColorInput::from_wire(1));
    assert_eq!(resolved.a, BlendAlphaInput::from_wire(2));
    assert_eq!(resolved.m, BlendColorInput::from_wire(3));
    assert_eq!(resolved.b, BlendBInput::from_wire(0));
}

#[test]
fn cycle_selectors_second_cycle_matches_blender_cycle_2_mapping() {
    let m = mode(0, false, 0, 0, 0, 0, 1, 2, 3, 0);
    let resolved = ResolvedBlendCycle::from_wire(m.blender_cycle_2());
    assert_eq!(resolved.p, BlendColorInput::from_wire(1));
    assert_eq!(resolved.a, BlendAlphaInput::from_wire(2));
    assert_eq!(resolved.m, BlendColorInput::from_wire(3));
    assert_eq!(resolved.b, BlendBInput::from_wire(0));
}
