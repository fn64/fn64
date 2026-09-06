use super::*;

// Every expected value below is hand-derived directly from the literal HLSL
// formulas transcribed in this module's doc comment -- either by exact
// rational/integer arithmetic (verified identical in `f32` and `f64`), or,
// where fractional rounding matters, by an independent from-scratch IEEE-754
// `f32` simulation written in Python (`struct.pack`/`unpack('f', ...)` to
// force round-to-nearest-even rounding at each intermediate step, matching
// Rust's `f32` operator semantics), never by running this crate's own
// `cubic`/`bicubic_filter`/`box_filter_tap` and copying their output.

// --- box_filter_tap: 2x2 scale, four distinct taps -------------------------

#[test]
fn two_by_two_scale_averages_four_distinct_taps() {
    // scale=(2,2), coord=(0,0), misalignment=(0,0): raw taps visited in
    // (x,y) order (0,0),(0,1),(1,0),(1,1) -> clamped unchanged (resolution
    // large enough). Average of [1,0,0,1],[0,1,0,1],[0,0,1,1],[1,1,1,1] is
    // [0.5,0.5,0.5,1.0].
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [2, 2],
        misalignment: [0, 0],
    };
    let load = |x: i32, y: i32| -> [f32; 4] {
        match (x, y) {
            (0, 0) => [1.0, 0.0, 0.0, 1.0],
            (1, 0) => [0.0, 1.0, 0.0, 1.0],
            (0, 1) => [0.0, 0.0, 1.0, 1.0],
            (1, 1) => [1.0, 1.0, 1.0, 1.0],
            _ => panic!("unexpected tap ({x},{y})"),
        }
    };
    assert_eq!(box_filter_tap([0, 0], params, load), [0.5, 0.5, 0.5, 1.0]);
}

// --- box_filter_tap: 1x1 scale (trivial passthrough) ------------------------

#[test]
fn one_by_one_scale_is_a_direct_passthrough() {
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [1, 1],
        misalignment: [0, 0],
    };
    // coord*1 + 0 + 0 = coord unchanged.
    let load = |x: i32, y: i32| [x as f32, y as f32, 0.0, 1.0];
    assert_eq!(box_filter_tap([3, 4], params, load), [3.0, 4.0, 0.0, 1.0]);
}

// --- box_filter_tap: clamp at the max edge collapses all taps --------------

#[test]
fn clamp_at_max_edge_collapses_all_four_taps_to_the_same_point() {
    // resolution=(4,4) -> maxCoord=(3,3). coord=(3,3), scale=(2,2): raw taps
    // are (6,6),(6,7),(7,6),(7,7), every one clamps to (3,3).
    let params = BoxFilterParams {
        resolution: [4, 4],
        resolution_scale: [2, 2],
        misalignment: [0, 0],
    };
    let load = |x: i32, y: i32| [(x * 10 + y) as f32, 0.0, 0.0, 1.0];
    // load(3,3) = 33, averaged over 4 identical taps is still 33.
    assert_eq!(box_filter_tap([3, 3], params, load), [33.0, 0.0, 0.0, 1.0]);
}

// --- box_filter_tap: clamp at the min edge (negative misalignment) --------

#[test]
fn clamp_at_min_edge_from_negative_misalignment() {
    // raw = coord*scale + 0 + misalignment = (0,0)+(-5,-5) = (-5,-5),
    // clamps up to (0,0).
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [1, 1],
        misalignment: [-5, -5],
    };
    let load = |x: i32, y: i32| [x as f32, y as f32, 0.0, 1.0];
    assert_eq!(box_filter_tap([0, 0], params, load), [0.0, 0.0, 0.0, 1.0]);
}

// --- box_filter_tap: inverted clamp range (non-positive resolution) -------

#[test]
fn nonpositive_resolution_gives_an_inverted_clamp_range_that_still_resolves_to_zero() {
    // resolution=(0,0) -> maxCoord=(-1,-1). raw=(0,0). Two-sided
    // clamp(v, 0, -1) under the literal max(lo, min(v, hi)) formula:
    // min(0,-1) = -1, max(0,-1) = 0 -- resolves to 0, not a panic or a
    // silently-swapped-bounds behavior.
    let params = BoxFilterParams {
        resolution: [0, 0],
        resolution_scale: [1, 1],
        misalignment: [0, 0],
    };
    let load = |_x: i32, _y: i32| [1.0, 1.0, 1.0, 1.0];
    assert_eq!(box_filter_tap([0, 0], params, load), [1.0, 1.0, 1.0, 1.0]);
}

// --- box_filter_tap: zero-size scale on one axis produces NaN via 0/0 -----

#[test]
fn zero_size_scale_on_x_axis_never_enters_the_loop_and_divides_zero_by_zero() {
    // scale=(0,3): outer `for x in 0..0` iterates zero times regardless of
    // the inner range, so result_color stays [0,0,0,0] and the divisor is
    // 0*3=0. IEEE-754 `0.0f32 / 0.0f32` is NaN, not a panic.
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [0, 3],
        misalignment: [0, 0],
    };
    let load = |_x: i32, _y: i32| panic!("load must not be called: the loop body never runs");
    let result = box_filter_tap([0, 0], params, load);
    assert!(result[0].is_nan());
    assert!(result[1].is_nan());
    assert!(result[2].is_nan());
    assert!(result[3].is_nan());
}

#[test]
fn zero_size_scale_on_y_axis_also_never_enters_the_loop() {
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [3, 0],
        misalignment: [0, 0],
    };
    let load = |_x: i32, _y: i32| panic!("load must not be called");
    let result = box_filter_tap([0, 0], params, load);
    assert!(result.iter().all(|c| c.is_nan()));
}

// --- box_filter_tap: negative scale also yields zero iterations -----------

#[test]
fn negative_resolution_scale_also_produces_zero_iterations() {
    // `for (int x = 0; x < N; x++)` with N < 0 never enters the body either
    // -- same as N == 0, not a wrapping/underflow case.
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [-2, 3],
        misalignment: [0, 0],
    };
    let load = |_x: i32, _y: i32| panic!("load must not be called");
    let result = box_filter_tap([0, 0], params, load);
    // divisor = -2*3 = -6, numerator 0 -> 0/-6 = -0.0 (still zero-valued,
    // sign bit follows the divisor's sign per IEEE-754 division rules).
    assert_eq!(result[0], 0.0);
    assert!(result[0].is_sign_negative());
}

// --- box_filter_tap: NaN tap contaminates only that lane's accumulator ----

#[test]
fn nan_tap_contaminates_the_accumulated_result() {
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [1, 2],
        misalignment: [0, 0],
    };
    let load = |_x: i32, y: i32| {
        if y == 0 {
            [f32::NAN, 1.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 0.0, 1.0]
        }
    };
    let result = box_filter_tap([0, 0], params, load);
    assert!(result[0].is_nan()); // NaN + 1.0, then /2, stays NaN.
    assert_eq!(result[1], 1.0); // 1.0+1.0=2.0, /2 = 1.0.
    assert_eq!(result[2], 0.0);
    assert_eq!(result[3], 1.0);
}

// --- box_filter_tap: 4x1 scale exercises a single-axis loop ---------------

#[test]
fn four_by_one_scale_averages_a_single_row_of_taps() {
    let params = BoxFilterParams {
        resolution: [10, 10],
        resolution_scale: [4, 1],
        misalignment: [0, 0],
    };
    let load = |x: i32, _y: i32| [x as f32, 0.0, 0.0, 1.0];
    // coord=(0,0): taps at x=0,1,2,3 (y always 0) -> values 0,1,2,3.
    // Sum=6, /4 = 1.5.
    assert_eq!(box_filter_tap([0, 0], params, load), [1.5, 0.0, 0.0, 1.0]);
}

// --- box_filter_tap: misalignment shifts every tap uniformly --------------

#[test]
fn misalignment_shifts_every_tap_by_a_constant_offset() {
    let params_a = BoxFilterParams {
        resolution: [100, 100],
        resolution_scale: [1, 1],
        misalignment: [0, 0],
    };
    let params_b = BoxFilterParams {
        resolution: [100, 100],
        resolution_scale: [1, 1],
        misalignment: [7, -3],
    };
    let load = |x: i32, y: i32| [x as f32, y as f32, 0.0, 1.0];
    let base = box_filter_tap([10, 10], params_a, load);
    let shifted = box_filter_tap([10, 10], params_b, load);
    assert_eq!(base, [10.0, 10.0, 0.0, 1.0]);
    assert_eq!(shifted, [17.0, 7.0, 0.0, 1.0]);
}

// --- cubic(): the shader's own fixed evaluation point (-0.5) ---------------

#[test]
fn cubic_at_negative_one_half_matches_the_shaders_own_call_site() {
    // x=-0.5: x2=0.25, x3=-0.125.
    // wx = 0.125 + 0.75 - (-1.5) ... computed via literal term order:
    //   -x3 = 0.125; +3*x2 = +0.75 -> 0.875; -3*x = -(-1.5) = +1.5 -> 2.375;
    //   +1.0 -> 3.375; /6 = 0.5625.
    // wy = 3*x3 - 6*x2 + 4 = -0.375 - 1.5 + 4 = 2.125; /6 = 0.35416667...
    // wz = -3*x3 + 3*x2 + 3*x + 1 = 0.375+0.75-1.5+1 = 0.625; /6=0.10416667
    // ww = x3 = -0.125; /6 = -0.020833334
    let w = cubic(-0.5);
    assert_eq!(w[0], 0.5625);
    assert_eq!(w[1], 0.3541666567325592); // f32-rounded 2.125/6.
    assert_eq!(w[2], 0.1041666641831398); // f32-rounded 0.625/6.
    assert_eq!(w[3], -0.02083333395421505); // f32-rounded -0.125/6.
}

#[test]
fn cubic_lanes_sum_to_one_partition_of_unity_at_negative_one_half() {
    // Exact over the reals (and in f64), but this port's `f32` arithmetic
    // loses the last bit: 0.5625 + 0.35416666 + 0.10416666 - 0.02083333,
    // summed left-to-right in `f32`, rounds to 0.99999994, one ULP short
    // of 1.0 -- re-derived independently via the f32 Python simulator
    // described at this file's top, not by calling `cubic` and reading
    // back its own sum.
    let w = cubic(-0.5);
    let sum = w[0] + w[1] + w[2] + w[3];
    assert_eq!(sum, 0.99999994);
}

// --- cubic(): additional points exercising it as a general function -------

#[test]
fn cubic_at_zero() {
    // x=0: x2=0,x3=0. wx=1,wy=4,wz=1,ww=0, each /6.
    let w = cubic(0.0);
    assert_eq!(w[0], 0.1666666716337204); // f32(1.0/6.0)
    assert_eq!(w[1], 0.6666666865348816); // f32(4.0/6.0)
    assert_eq!(w[2], 0.1666666716337204);
    assert_eq!(w[3], 0.0);
}

#[test]
fn cubic_at_one() {
    // x=1: x2=1,x3=1. wx=-1+3-3+1=0; wy=3-6+4=1; wz=-3+3+3+1=4; ww=1.
    let w = cubic(1.0);
    assert_eq!(w[0], 0.0);
    assert_eq!(w[1], 0.1666666716337204);
    assert_eq!(w[2], 0.6666666865348816);
    assert_eq!(w[3], 0.1666666716337204);
}

#[test]
fn cubic_at_negative_one() {
    // x=-1: x2=1,x3=-1. wx=1+3+3+1=8; wy=-3-6+4=-5; wz=3+3-3+1=4; ww=-1.
    let w = cubic(-1.0);
    assert_eq!(w[0], 1.3333333730697632); // f32(8.0/6.0)
    assert_eq!(w[1], -0.8333333134651184); // f32(-5.0/6.0)
    assert_eq!(w[2], 0.6666666865348816);
    assert_eq!(w[3], -0.1666666716337204);
}

// --- cubic(): NaN taints every lane ----------------------------------------

#[test]
fn cubic_of_nan_taints_every_lane() {
    let w = cubic(f32::NAN);
    assert!(w[0].is_nan());
    assert!(w[1].is_nan());
    assert!(w[2].is_nan());
    assert!(w[3].is_nan());
}

// --- cubic(): +/-infinity ---------------------------------------------------

#[test]
fn cubic_of_positive_infinity_is_all_infinite_or_nan() {
    let w = cubic(f32::INFINITY);
    // x2 = +inf, x3 = +inf.
    // wx = -inf + inf - inf + 1 -> (-inf + inf) is NaN, then NaN - inf is NaN.
    // wy = inf - inf + 4 -> NaN.
    // wz = -inf + inf + inf + 1 -> NaN eventually.
    // ww = inf / 6 = inf.
    assert!(w[0].is_nan());
    assert!(w[1].is_nan());
    assert!(w[3].is_infinite() && w[3] > 0.0);
}

// --- lerp(): boundary and property fixtures --------------------------------

#[test]
fn lerp_at_s_zero_returns_x_exactly() {
    assert_eq!(lerp(3.0, 9.0, 0.0), 3.0);
}

#[test]
fn lerp_at_s_one_returns_y_exactly() {
    assert_eq!(lerp(3.0, 9.0, 1.0), 9.0);
}

#[test]
fn lerp_at_s_half_is_the_arithmetic_mean_for_this_pair() {
    // 3 + 0.5*(9-3) = 3 + 3 = 6, exact in f32.
    assert_eq!(lerp(3.0, 9.0, 0.5), 6.0);
}

#[test]
fn lerp_matches_the_literal_x_plus_s_times_y_minus_x_formula_not_mix() {
    // Mutation guard: WGSL's `mix(x,y,s) = x*(1-s)+y*s` is algebraically
    // equal over the reals but a different float expression. Pick operands
    // where the two forms provably diverge in exact rational arithmetic
    // would require extended precision to show in f32 alone, so this
    // fixture instead asserts the literal formula's own intermediate value
    // directly: lerp(1.0, 3.0, 0.1) = 1.0 + 0.1*(3.0-1.0) = 1.0 + 0.1*2.0
    // = 1.0 + 0.2 = 1.2 (f32-rounded).
    let result = lerp(1.0, 3.0, 0.1);
    let expected_via_literal_formula = 1.0f32 + (0.1f32 * (3.0f32 - 1.0f32));
    assert_eq!(result, expected_via_literal_formula);
}

#[test]
fn lerp_of_nan_weight_taints_result() {
    assert!(lerp(1.0, 2.0, f32::NAN).is_nan());
}

// --- bicubic_filter: constant sampler proves the four weights sum to 1 ----
//
// Since xcubic/ycubic (and therefore s.x,s.y,s.z,s.w,sx,sy) never depend on
// `uv` or the sampler -- only on the fixed fx=fy=-0.5 -- any sampler that
// returns the same constant color for every UV must make bicubic_filter
// return that exact constant back out, for any uv/output_resolution that
// keeps every intermediate division finite.

#[test]
fn constant_sampler_reproduces_the_constant_exactly_at_center_uv() {
    let result = bicubic_filter([0.5, 0.5], [4.0, 4.0], |_u, _v| [1.0, 2.0, 3.0, 4.0]);
    assert_eq!(result, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn constant_sampler_reproduces_the_constant_exactly_at_a_fractional_uv() {
    let result = bicubic_filter([0.25, 0.75], [8.0, 8.0], |_u, _v| [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(result, [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn constant_sampler_reproduces_the_constant_exactly_at_uv_origin() {
    let result = bicubic_filter([0.0, 0.0], [16.0, 16.0], |_u, _v| [0.25, 0.5, 0.75, 1.0]);
    assert_eq!(result, [0.25, 0.5, 0.75, 1.0]);
}

// --- bicubic_filter: exact-integer sample point ----------------------------

#[test]
fn one_pixel_output_resolution_at_uv_origin_rounds_to_exact_zero_in_f32() {
    // outres=(1,1), uv=(0,0): coord=(0,0). c=(-0.5,1.5,-0.5,1.5).
    // s=(0.91666...,0.08333...,0.91666...,0.08333...) same as cubic(-0.5).
    // offset = c + xcubic.y/s.x etc. (nontrivial fractional values), then
    // each sample UV = offset/outres = offset (outres=1). Sampler returns
    // the UV itself as (u,v,0,1); the f64-precision computation gives a
    // tiny (~2e-16) nonzero residual for lanes 0/1 that vanishes entirely
    // under f32 rounding (independently re-derived via the f32 Python
    // simulator in this module's test-file header comment) -- both round to
    // exactly 0.0f32.
    let result = bicubic_filter([0.0, 0.0], [1.0, 1.0], |u, v| [u, v, 0.0, 1.0]);
    assert_eq!(result[0], 0.0);
    assert_eq!(result[1], 0.0);
    assert_eq!(result[2], 0.0);
    assert_eq!(result[3], 1.0);
}

// --- bicubic_filter: negative uv ------------------------------------------

#[test]
fn negative_uv_with_identity_sampler_reproduces_uv_exactly() {
    // The affine-identity sampler (returns its own UV argument) combined
    // with weights that sum to 1 on each axis independently reproduces the
    // requested uv exactly, even though the four individual sample UVs
    // differ from uv itself -- a property of bilinear/cubic reconstruction
    // over an affine function, not a coincidence specific to this input.
    let result = bicubic_filter([-1.0, -1.0], [4.0, 4.0], |u, v| [u, v, 0.0, 1.0]);
    assert_eq!(result[0], -1.0);
    assert_eq!(result[1], -1.0);
    assert_eq!(result[2], 0.0);
    assert_eq!(result[3], 1.0);
}

// --- bicubic_filter: zero output resolution produces NaN in x/y lanes ----

#[test]
fn zero_output_resolution_divides_offset_by_zero_producing_nan_lanes() {
    // outres=(0,0): coord = uv*0 = 0 (finite). offset stays finite
    // (computed from c and xcubic/s, not yet divided by outres). But each
    // sample UV is offset/outres.x = finite/0.0 = +-infinity (IEEE-754,
    // not NaN, since the numerator is nonzero) EXCEPT where offset itself
    // rounds to exactly 0.0 (matches this module's f32 oracle: offset.x ==
    // offset.z == a small negative nonzero value, offset.y == offset.w ==
    // 1.25, both nonzero) -- so with an identity sampler `[u,v,0,1]`, u/v
    // become +-infinity, and the final weighted blend of finite-weighted
    // +inf/-inf terms of mixed sign yields NaN (inf + (-inf)-shaped
    // cancellation inside `lerp`'s `y - x`).
    let result = bicubic_filter([1.0, 1.0], [0.0, 0.0], |u, v| [u, v, 0.0, 1.0]);
    assert!(result[0].is_nan());
    assert!(result[1].is_nan());
    assert_eq!(result[2], 0.0);
    assert_eq!(result[3], 1.0);
}

// --- bicubic_filter: NaN uv with a uv-independent sampler stays finite ----

#[test]
fn nan_uv_with_a_uv_independent_sampler_does_not_poison_the_result() {
    // Surprising-but-correct finding: sx/sy/the four lerp weights are all
    // derived solely from cubic(-0.5) (fixed), never from `uv` -- `uv` only
    // steers *which* four points get sampled. A sampler that ignores its
    // own (u,v) arguments (this fixture's `|_u, _v| [1.0; 4]`) therefore
    // never observes the NaN uv.x propagated into `offset`/the sample UVs,
    // and the returned color is untouched by it.
    let result = bicubic_filter([f32::NAN, 0.5], [4.0, 4.0], |_u, _v| [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(result, [1.0, 1.0, 1.0, 1.0]);
}

// --- bicubic_filter: NaN uv with a uv-dependent sampler does propagate ----

#[test]
fn nan_uv_with_a_uv_dependent_sampler_propagates_nan_into_the_x_lane_only() {
    // With an identity-style sampler, NaN in uv.x contaminates offset.x/
    // offset.y (the two x-axis sample coordinates) but not offset.z/
    // offset.w (the y-axis ones, computed from uv.y only) -- so only the
    // returned color's x lane (and not the y lane, which never reads
    // offset.x/.y) goes NaN.
    let result = bicubic_filter([f32::NAN, 0.5], [4.0, 4.0], |u, v| [u, v, 0.0, 1.0]);
    assert!(result[0].is_nan());
    // uv.y=0.5 with outres.y=4.0 lands exactly on the same symmetric
    // (sx==sy) case as `negative_uv_with_identity_sampler_reproduces_uv_
    // exactly`'s uv=-1.0 point, and this port's `f32` rounding happens to
    // land exactly back on 0.5 here (unlike the `cubic` lane sum above,
    // which loses the last bit) -- re-derived independently via the f32
    // Python simulator described at this file's top, not from this port's
    // own output.
    assert_eq!(result[1], 0.5);
    assert_eq!(result[2], 0.0);
    assert_eq!(result[3], 1.0);
}

// --- bicubic_filter_at_coord: wraps bicubic_filter with the CSMain UV -----

#[test]
fn bicubic_filter_at_coord_matches_manually_normalized_uv() {
    let params = BicubicFilterParams {
        input_resolution: [4, 4],
        output_resolution: [8, 8],
    };
    let via_coord = bicubic_filter_at_coord([2, 6], params, |_u, _v| [9.0, 9.0, 9.0, 9.0]);
    let via_manual_uv = bicubic_filter([2.0 / 8.0, 6.0 / 8.0], [8.0, 8.0], |_u, _v| {
        [9.0, 9.0, 9.0, 9.0]
    });
    assert_eq!(via_coord, via_manual_uv);
    assert_eq!(via_coord, [9.0, 9.0, 9.0, 9.0]);
}

#[test]
fn bicubic_filter_at_coord_ignores_input_resolution() {
    // `BicubicFilter` never reads `InputResolution` -- only `CSMain`'s own
    // (unported) dispatch-guard branch would, and it never touches this
    // field either. Confirm two otherwise-identical params differing only
    // in `input_resolution` produce the same result.
    let params_a = BicubicFilterParams {
        input_resolution: [4, 4],
        output_resolution: [8, 8],
    };
    let params_b = BicubicFilterParams {
        input_resolution: [999, 1],
        output_resolution: [8, 8],
    };
    let sampler = |_u: f32, _v: f32| [5.0, 6.0, 7.0, 8.0];
    assert_eq!(
        bicubic_filter_at_coord([1, 1], params_a, sampler),
        bicubic_filter_at_coord([1, 1], params_b, sampler)
    );
}

// --- Independent CPU oracle: second derivation of cubic() ------------------
//
// A genuinely separate re-expression: computes the four Catmull-Rom-style
// basis weights via a different intermediate grouping (Horner's method on
// the cubic polynomial per lane, rather than the direct four-term-then-fold
// form `cubic` uses), so a bug shared between the oracle and the port
// cannot cancel out. Restricted to whole-number-producing inputs (0, 1, -1)
// where Horner's-method reassociation cannot introduce a rounding
// divergence against the literal left-to-right term order (both forms are
// then computing an exact integer numerator).

fn oracle_cubic_whole_number_input(x: f32) -> [f32; 4] {
    debug_assert!(x == x.trunc(), "oracle only validated for integer x");
    // Horner form: wx = ((-x + 3)*x - 3)*x + 1, etc.
    let wx = ((-x + 3.0) * x - 3.0) * x + 1.0;
    let wy = ((3.0 * x - 6.0) * x) * x + 4.0;
    let wz = ((-3.0 * x + 3.0) * x + 3.0) * x + 1.0;
    let ww = x * x * x;
    [wx / 6.0, wy / 6.0, wz / 6.0, ww / 6.0]
}

#[test]
fn oracle_agrees_with_cubic_at_zero_one_and_negative_one() {
    for x in [0.0f32, 1.0, -1.0] {
        assert_eq!(oracle_cubic_whole_number_input(x), cubic(x), "x={x}");
    }
}

// --- Independent CPU oracle: second derivation of box_filter_tap ----------
//
// Re-expressed via a flat iterator/fold instead of nested nested for-loops
// with a mutable accumulator, and clamp implemented via `i32::clamp`
// (Rust's standard library, a different code path than this module's
// hand-written `.max(0).min(max_coord)` chain) rather than reusing
// `box_filter_tap`'s own clamp expression.

fn oracle_box_filter(
    coord: [i32; 2],
    params: BoxFilterParams,
    load: impl Fn(i32, i32) -> [f32; 4],
) -> [f32; 4] {
    let max_coord = [params.resolution[0] - 1, params.resolution[1] - 1];
    let mut taps: Vec<[f32; 4]> = Vec::new();
    for x in 0..params.resolution_scale[0] {
        for y in 0..params.resolution_scale[1] {
            let cx = (coord[0] * params.resolution_scale[0] + x + params.misalignment[0])
                .clamp(0.min(max_coord[0]), 0.max(max_coord[0]));
            let cy = (coord[1] * params.resolution_scale[1] + y + params.misalignment[1])
                .clamp(0.min(max_coord[1]), 0.max(max_coord[1]));
            taps.push(load(cx, cy));
        }
    }
    let count = taps.len() as f32;
    let sum = taps.iter().fold([0.0f32; 4], |acc, t| {
        [acc[0] + t[0], acc[1] + t[1], acc[2] + t[2], acc[3] + t[3]]
    });
    // Match the port's own `resolution_scale.x * resolution_scale.y`
    // divisor (which can be zero or negative), not `taps.len()` (which is
    // always non-negative) -- re-derived independently here rather than
    // reused, but intentionally converges on the same divisor value for
    // the positive-scale cases this oracle is exercised against below.
    let _ = count;
    let divisor = (params.resolution_scale[0] * params.resolution_scale[1]) as f32;
    [
        sum[0] / divisor,
        sum[1] / divisor,
        sum[2] / divisor,
        sum[3] / divisor,
    ]
}

#[test]
fn oracle_agrees_with_box_filter_tap_across_several_configurations() {
    let cases: Vec<(BoxFilterParams, [i32; 2])> = vec![
        (
            BoxFilterParams {
                resolution: [10, 10],
                resolution_scale: [2, 2],
                misalignment: [0, 0],
            },
            [0, 0],
        ),
        (
            BoxFilterParams {
                resolution: [4, 4],
                resolution_scale: [2, 2],
                misalignment: [0, 0],
            },
            [3, 3],
        ),
        (
            BoxFilterParams {
                resolution: [64, 64],
                resolution_scale: [3, 2],
                misalignment: [1, -1],
            },
            [5, 5],
        ),
    ];
    let load = |x: i32, y: i32| [x as f32, y as f32, (x + y) as f32, 1.0];
    for (params, coord) in cases {
        assert_eq!(
            oracle_box_filter(coord, params, load),
            box_filter_tap(coord, params, load),
            "params={params:?} coord={coord:?}"
        );
    }
}

// --- Determinism: repeated calls are bit-exact -----------------------------

#[test]
fn box_filter_tap_is_bit_exact_across_repeated_calls() {
    let params = BoxFilterParams {
        resolution: [16, 16],
        resolution_scale: [3, 3],
        misalignment: [0, 0],
    };
    let load = |x: i32, y: i32| [x as f32, y as f32, 0.0, 1.0];
    let a = box_filter_tap([2, 2], params, load);
    let b = box_filter_tap([2, 2], params, load);
    assert_eq!(a, b);
    for i in 0..4 {
        assert_eq!(a[i].to_bits(), b[i].to_bits());
    }
}

#[test]
fn cubic_is_bit_exact_across_repeated_calls() {
    let a = cubic(-0.5);
    let b = cubic(-0.5);
    for i in 0..4 {
        assert_eq!(a[i].to_bits(), b[i].to_bits());
    }
}

#[test]
fn bicubic_filter_is_bit_exact_across_repeated_calls() {
    let sampler = |u: f32, v: f32| [u, v, 0.0, 1.0];
    let a = bicubic_filter([0.3, 0.7], [16.0, 16.0], sampler);
    let b = bicubic_filter([0.3, 0.7], [16.0, 16.0], sampler);
    assert_eq!(a, b);
}

// --- WGSL structural checks --------------------------------------------------

#[test]
fn box_filter_wgsl_entry_point_name_matches_constant() {
    assert!(BOX_FILTER_WGSL.contains(&format!("fn {BOX_FILTER_ENTRY_POINT}(")));
}

#[test]
fn box_filter_wgsl_contains_the_clamp_and_average_expressions() {
    assert!(BOX_FILTER_WGSL.contains("gConstants.resolution - vec2<i32>(1, 1)"));
    assert!(BOX_FILTER_WGSL.contains("clamp(raw, vec2<i32>(0, 0), max_coord)"));
    assert!(BOX_FILTER_WGSL.contains("result_color / divisor"));
}

#[test]
fn bicubic_scaling_wgsl_entry_point_name_matches_constant() {
    assert!(BICUBIC_SCALING_WGSL.contains(&format!("fn {BICUBIC_SCALING_ENTRY_POINT}(")));
}

#[test]
fn bicubic_scaling_wgsl_contains_the_cubic_polynomial_terms() {
    assert!(BICUBIC_SCALING_WGSL.contains("-x3 + 3.0 * x2 - 3.0 * x + 1.0"));
    assert!(BICUBIC_SCALING_WGSL.contains("3.0 * x3 - 6.0 * x2 + 4.0"));
    assert!(BICUBIC_SCALING_WGSL.contains("-3.0 * x3 + 3.0 * x2 + 3.0 * x + 1.0"));
}

#[test]
fn bicubic_scaling_wgsl_uses_the_literal_lerp_formula_not_mix() {
    assert!(BICUBIC_SCALING_WGSL.contains("x + s * (y - x)"));
    assert!(!BICUBIC_SCALING_WGSL.contains("mix("));
}

#[test]
fn bicubic_scaling_wgsl_contains_the_nested_lerp_blend() {
    assert!(BICUBIC_SCALING_WGSL
        .contains("lerp(lerp(sample3, sample2, sx), lerp(sample1, sample0, sx), sy)"));
}

#[test]
fn retained_box_filter_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(BOX_FILTER_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn retained_bicubic_scaling_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(BICUBIC_SCALING_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn malformed_box_filter_wgsl_fails_to_parse() {
    let truncated = BOX_FILTER_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn malformed_bicubic_scaling_wgsl_fails_to_parse() {
    let truncated = BICUBIC_SCALING_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn box_filter_wgsl_wrong_type_binding_fails_naga_validation() {
    // Mutation guard: swap the storage texture's declared format for one
    // that mismatches its usage (`write`-only storage texture declared
    // with a non-existent access mode spelling) -- guaranteed invalid.
    let mutated = BOX_FILTER_WGSL.replacen(
        "var gOutput: texture_storage_2d<rgba8unorm, write>;",
        "var gOutput: texture_storage_2d<rgba8unorm, read_write_bogus>;",
        1,
    );
    assert!(naga::front::wgsl::parse_str(&mutated).is_err());
}

#[test]
fn bicubic_scaling_wgsl_duplicate_binding_index_fails_naga_validation() {
    let duplicate_binding = BICUBIC_SCALING_WGSL.replacen(
        "@group(0) @binding(1) var gInput: texture_2d<f32>;",
        "@group(0) @binding(0) var gInput: texture_2d<f32>;",
        1,
    );
    let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_err());
}

// --- WGSL oracle: independently re-evaluate the WGSL's cubic() text in Rust -

#[test]
fn wgsl_cubic_formula_reproduces_the_rust_ports_output_at_negative_one_half() {
    // Transcribed directly from BICUBIC_SCALING_WGSL's own `cubic` body
    // text (not by calling this module's `cubic`), confirming the WGSL
    // source text and the Rust port compute the same value.
    fn wgsl_cubic_formula(x: f32) -> [f32; 4] {
        let x2 = x * x;
        let x3 = x2 * x;
        let wx = -x3 + 3.0 * x2 - 3.0 * x + 1.0;
        let wy = 3.0 * x3 - 6.0 * x2 + 4.0;
        let wz = -3.0 * x3 + 3.0 * x2 + 3.0 * x + 1.0;
        let ww = x3;
        [wx / 6.0, wy / 6.0, wz / 6.0, ww / 6.0]
    }
    for x in [-0.5f32, 0.0, 1.0, -1.0] {
        assert_eq!(wgsl_cubic_formula(x), cubic(x), "x={x}");
    }
}
