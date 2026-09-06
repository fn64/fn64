//! **Property tests for the S10.5 texture-coordinate conversion.**
//!
//! The kernel under test is
//! [`triangle_span::texture_coordinates_s10_5`][super::triangle_span] at
//! `crates/fn64-render-wgpu/src/raw_dpc/triangle_span.rs:803`, together with
//! its narrowing helper `saturate_s10_5` at `:828`. It turns a triangle's
//! interpolated `[S, T, W]` plane values -- Q16.16 fixed point carried in
//! `i64` -- into the pair of `i16` S10.5 texture coordinates the sampler
//! consumes.
//!
//! ```text
//! perspective: (S/W) * 2^15,  then saturate into i16
//! affine:      S / 2^16,      then saturate into i16
//! ```
//!
//! # The oracles, and why they are independent
//!
//! Two of the three properties do NOT re-run the kernel's own arithmetic:
//!
//! - **Monotonicity** (`the_affine_conversion_is_monotone_in_its_plane_value`,
//!   `the_perspective_conversion_is_monotone_in_its_numerator`). The oracle is
//!   an ORDER LAW, not a formula: the conversion is a positive scale followed
//!   by a clamp, and both are monotone non-decreasing, so `a <= b` must imply
//!   `f(a) <= f(b)`. Nothing in the property computes `2^15`, `2^16`, or a
//!   clamp bound, so it cannot agree with the kernel by sharing its constants.
//!   A sign flip, a wrong-direction clamp, or an `as i16` wrap (which folds
//!   the top of the range back to the bottom) all break the order.
//!
//! - **Roundtrip** (`an_in_range_affine_coordinate_survives_the_roundtrip`).
//!   The oracle is INVERSION: for a plane value chosen so the result is inside
//!   S10.5's representable range, scaling the recovered coordinate back by the
//!   documented plane unit must return to the same Q16.16 neighbourhood. This
//!   is the one property that names the affine scale, and it names it as the
//!   INVERSE, so a change to the forward constant is not silently mirrored.
//!
//! - **Saturation** (`a_plane_value_past_the_range_saturates_to_exactly_that_bound`).
//!   The oracle is the BOUND ITSELF: a plane value past the representable
//!   range must convert to exactly `i16::MAX` / `i16::MIN`, and one inside
//!   must not be clamped at all. This owns both clamp bounds, so a one-ULP
//!   move of either is a property failure.
//!
//!   An earlier version asserted `(i16::MIN..=i16::MAX).contains(&v)`, which
//!   is a **tautology** on an `i16` -- a test that could not fail, in breach
//!   of rule 12. It showed: an off-by-one in either bound left every property
//!   green and was caught only by the coverage counter. Replaced, and the
//!   mutation table below now records both bounds killed by a property.
//!
//! - **Degenerate W** (`degenerate_w_maps_to_the_documented_value`) pins the
//!   NaN and infinity arms to concrete values -- zero for `0/0`, the
//!   sign-matching edge for a non-zero numerator over zero -- which a naive
//!   `as i16` would resolve differently.
//!
//! # The vacuity trap this file was written against
//!
//! Task 5.1's `stepping_differential` passed 256 cases while writing zero
//! bytes. The analogue here is a generator that only ever produces values
//! which SATURATE: every case would then return `i16::MAX` or `i16::MIN`,
//! monotonicity would hold trivially, and the scale constants would be
//! entirely untested. `the_generator_reaches_all_three_saturation_regimes`
//! is the guard: it counts how many generated cases land below the range, in
//! the range, and above it, and fails unless all three are populated. The
//! in-range count is the one that matters -- that is the only regime in which
//! the arithmetic is observable at all.
//!
//! # Mutation results (see the task report for the full table)
//!
//! | mutation | killed by |
//! |---|---|
//! | `PERSPECTIVE_TEXEL_SCALE` `32768.0` -> `-32768.0` | monotonicity |
//! | `saturate_s10_5` clamp high `i16::MAX` -> `i16::MAX - 1` | saturation bounds |
//! | `saturate_s10_5` clamp low `i16::MIN` -> `i16::MIN + 1` | saturation bounds |
//!
//! # Blast radius
//!
//! These properties see the SCALE, the SIGN, the CLAMP and the NaN policy.
//! They do NOT see the choice between the perspective and affine arms (the
//! `perspective` flag is an input, not a derived value), nor the upstream
//! plane interpolation that produces `stw`. A wrong `perspective` flag at the
//! call site is invisible here.

use proptest::prelude::*;

use super::triangle_span::texture_coordinates_s10_5;

/// The affine arm's documented plane unit: `2^16`, one Q16.16 fractional
/// unit. Written here as the INVERSE direction (multiply to go back), so this
/// file does not simply restate the kernel's own divisor.
const PLANE_UNITS_PER_S10_5: f64 = 65536.0;

/// Plane values spanning both the in-range and the saturating regimes.
///
/// `i16::MAX * 2^16` is the largest affine plane value that still lands
/// inside S10.5, so a range a few times wider than that guarantees the
/// generator straddles the clamp in both directions instead of sitting
/// entirely inside it or entirely outside it.
const AFFINE_SPAN: i64 = (i16::MAX as i64) * 65536 * 4;

prop_compose! {
    // An affine `[S, T, W]` triple. `W` is unused by the affine arm and is
    // held at one plane unit so a future reader does not mistake it for a
    // participating value.
    fn affine_plane()(s in -AFFINE_SPAN..=AFFINE_SPAN, t in -AFFINE_SPAN..=AFFINE_SPAN)
        -> [i64; 3]
    {
        [s, t, 65536]
    }
}

proptest! {
    /// **Order law.** The affine arm is `value / 2^16` clamped into `i16`:
    /// a positive scale composed with a clamp, both monotone non-decreasing.
    /// So a larger plane value can never convert to a smaller coordinate.
    ///
    /// The oracle is the ordering itself. This test never computes `2^16`.
    #[test]
    fn the_affine_conversion_is_monotone_in_its_plane_value(
        a in -AFFINE_SPAN..=AFFINE_SPAN,
        b in -AFFINE_SPAN..=AFFINE_SPAN,
    ) {
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        let (low_s, low_t) = texture_coordinates_s10_5([low, low, 65536], false);
        let (high_s, high_t) = texture_coordinates_s10_5([high, high, 65536], false);
        prop_assert!(
            low_s <= high_s,
            "affine S not monotone: plane {low} -> {low_s}, plane {high} -> {high_s}"
        );
        prop_assert!(
            low_t <= high_t,
            "affine T not monotone: plane {low} -> {low_t}, plane {high} -> {high_t}"
        );
    }

    /// **Order law, perspective arm.** With `W` held strictly positive, the
    /// perspective arm is `(S/W) * 2^15` clamped -- again a positive scale
    /// and a clamp. A negative scale constant, or a clamp that folds instead
    /// of saturating, inverts some pair and fails here.
    #[test]
    fn the_perspective_conversion_is_monotone_in_its_numerator(
        a in -AFFINE_SPAN..=AFFINE_SPAN,
        b in -AFFINE_SPAN..=AFFINE_SPAN,
        w in 1i64..=AFFINE_SPAN,
    ) {
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        let (low_s, _) = texture_coordinates_s10_5([low, 0, w], true);
        let (high_s, _) = texture_coordinates_s10_5([high, 0, w], true);
        prop_assert!(
            low_s <= high_s,
            "perspective S not monotone at W={w}: {low} -> {low_s}, {high} -> {high_s}"
        );
    }

    /// **Inversion.** For a plane value whose affine result is strictly
    /// inside S10.5's range, scaling the coordinate back by the plane unit
    /// must land within one plane unit of where it started -- the only loss
    /// is the conversion's own truncation toward zero.
    ///
    /// Restricting the generator to the in-range band is what makes this an
    /// inversion rather than a clamp test; `the_generator_reaches_...` below
    /// proves the band is genuinely reached.
    #[test]
    fn an_in_range_affine_coordinate_survives_the_roundtrip(
        plane in -((i16::MAX as i64) * 65536)..=((i16::MAX as i64) * 65536),
    ) {
        let (s, _) = texture_coordinates_s10_5([plane, 0, 65536], false);
        let recovered = f64::from(s) * PLANE_UNITS_PER_S10_5;
        let error = (recovered - plane as f64).abs();
        prop_assert!(
            error < PLANE_UNITS_PER_S10_5,
            "roundtrip lost more than one plane unit: plane {plane} -> S10.5 {s} \
             -> {recovered} (error {error})"
        );
    }

    /// **The clamp bounds, owned.** A plane value beyond the representable
    /// range on either side must convert to EXACTLY that bound, and a value
    /// inside must not be clamped at all.
    ///
    /// This replaces an earlier assertion, `(i16::MIN..=i16::MAX).contains(&v)`,
    /// which was a tautology: `v` is already an `i16`, so the compiler can
    /// prove the range check universally true. Under rule 12 that was a test
    /// which could not fail, and it showed -- an off-by-one in EITHER
    /// saturation bound left every property green and was caught only by the
    /// coverage counter below. The claims here can fail, and they name the
    /// exact bound on each side, so a one-ULP change to either is a property
    /// failure.
    #[test]
    fn a_plane_value_past_the_range_saturates_to_exactly_that_bound(
        excess in 1i64..=(AFFINE_SPAN * 4),
        inside in -((i16::MAX as i64 - 1) * 65536)..=((i16::MAX as i64 - 1) * 65536),
    ) {
        // Beyond the top: `i16::MAX * 2^16` is the largest plane value that
        // still represents, so anything strictly above it must pin to MAX.
        let above = (i16::MAX as i64) * 65536 + excess;
        let (s, _) = texture_coordinates_s10_5([above, 0, 65536], false);
        prop_assert_eq!(
            s,
            i16::MAX,
            "plane {} is past the top of S10.5 but did not saturate to i16::MAX",
            above
        );

        // Beyond the bottom, symmetrically.
        let below = (i16::MIN as i64) * 65536 - excess;
        let (s, _) = texture_coordinates_s10_5([below, 0, 65536], false);
        prop_assert_eq!(
            s,
            i16::MIN,
            "plane {} is past the bottom of S10.5 but did not saturate to i16::MIN",
            below
        );

        // And a value comfortably inside must NOT be clamped: if either bound
        // moved inward, some in-range value would start pinning to it.
        let (s, _) = texture_coordinates_s10_5([inside, 0, 65536], false);
        prop_assert!(
            s != i16::MAX && s != i16::MIN,
            "in-range plane {} was clamped to {} -- a saturation bound moved inward",
            inside,
            s
        );
    }

    /// **Total postcondition.** Every input, including the degenerate `W`
    /// values that make the perspective divide produce an infinity or a NaN,
    /// must produce a defined coordinate.
    ///
    /// The NaN arm maps to exactly zero per the kernel's documented policy,
    /// and an infinity must saturate to the edge matching its sign rather
    /// than wrapping -- both asserted as concrete values, since the range
    /// itself is not assertable on an `i16` (see the property above).
    #[test]
    fn degenerate_w_maps_to_the_documented_value(
        s in any::<i64>(),
        t in any::<i64>(),
        w in any::<i64>(),
    ) {
        let (ps, _pt) = texture_coordinates_s10_5([s, t, w], true);

        if w == 0 {
            if s == 0 {
                // 0/0 is NaN: the documented policy maps it to zero rather
                // than letting a cast choose.
                prop_assert_eq!(ps, 0, "NaN did not map to zero for S");
            } else {
                // Non-zero over zero is an infinity, which must saturate to
                // the edge matching its sign, never wrap to the other one.
                let expected = if s > 0 { i16::MAX } else { i16::MIN };
                prop_assert_eq!(
                    ps,
                    expected,
                    "infinite S/W (s={}) did not saturate to the matching edge",
                    s
                );
            }
        }
    }
}

/// **The anti-vacuity guard.** Counts which saturation regime each generated
/// affine case lands in and fails unless all three are populated.
///
/// Without this, a generator whose range sat entirely above `i16::MAX * 2^16`
/// would make every case return `i16::MAX`, monotonicity would pass for free,
/// and the scale constant would be untested. The in-range count is the load
/// bearing one: it is the only regime in which the arithmetic is observable.
#[test]
fn the_generator_reaches_all_three_saturation_regimes() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = affine_plane();

    let (mut below, mut inside, mut above) = (0usize, 0usize, 0usize);
    const CASES: usize = 512;
    for _ in 0..CASES {
        let stw = strategy
            .new_tree(&mut runner)
            .expect("affine plane strategy produces a value")
            .current();
        let (s, _) = texture_coordinates_s10_5(stw, false);
        if s == i16::MIN {
            below += 1;
        } else if s == i16::MAX {
            above += 1;
        } else {
            inside += 1;
        }
    }

    assert!(
        below > 0 && inside > 0 && above > 0,
        "generator does not straddle the S10.5 clamp over {CASES} cases: \
         below={below} inside={inside} above={above}"
    );
    // The in-range band is a quarter of the generated span by construction;
    // requiring a substantial share stops a drifting strategy from reducing
    // the observable regime to a handful of cases.
    assert!(
        inside * 8 > CASES,
        "too few in-range cases to exercise the scale: inside={inside} of {CASES}"
    );
}

/// **The perspective generator reaches its own in-range band.** The
/// perspective monotonicity property divides by a generated `W`, and a `W`
/// range that made every quotient saturate would make that property vacuous
/// in exactly the same way. Counted separately because it uses a different
/// strategy.
#[test]
fn the_perspective_generator_reaches_the_in_range_band() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = (-AFFINE_SPAN..=AFFINE_SPAN, 1i64..=AFFINE_SPAN);

    let mut inside = 0usize;
    const CASES: usize = 512;
    for _ in 0..CASES {
        let (s, w) = strategy
            .new_tree(&mut runner)
            .expect("perspective strategy produces a value")
            .current();
        let (coordinate, _) = texture_coordinates_s10_5([s, 0, w], true);
        if coordinate != i16::MIN && coordinate != i16::MAX {
            inside += 1;
        }
    }

    assert!(
        inside > 0,
        "perspective generator never produced an unsaturated coordinate over {CASES} cases"
    );
}
