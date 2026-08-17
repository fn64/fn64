use super::*;

// --- Independent CPU oracle -------------------------------------------------
//
// A second, independently-derived re-expression of the region-weight
// selection and combine, written directly from the HLSL region table (not by
// calling `region_weights`/`combine_channel`): a `match` over an explicit
// `(is_left, is_right, is_top, is_bottom)` edge tuple rather than the
// source's early-return `if`/`else if` chain, and a `fold`-based dot product
// rather than the four-term expression. This gives the tests below a genuine
// second derivation to compare against the port, not an implementation
// compared to itself.
fn oracle_region_weights(x: u32, y: u32, width: u32, height: u32) -> [f32; 4] {
    const A: f32 = 0.077847;
    const B: f32 = 0.123317;
    const C: f32 = 0.195346;
    const DIV_CORNER: f32 = 0.519827;
    const DIV_BORDER: f32 = 0.720991;

    let is_left = x == 0;
    let is_right = x == width - 1;
    let is_top = y == 0;
    let is_bottom = y == height - 1;
    let is_border = is_left || is_right || is_top || is_bottom;

    if !is_border {
        return [A + B + B + C, A + B, A + B, A];
    }
    match (is_left, is_right, is_top, is_bottom) {
        (true, false, true, false) => [
            C / DIV_CORNER,
            B / DIV_CORNER,
            B / DIV_CORNER,
            A / DIV_CORNER,
        ],
        (false, true, true, false) => [(B + C) / DIV_CORNER, 0.0, (A + B) / DIV_CORNER, 0.0],
        (true, false, false, true) => [(B + C) / DIV_CORNER, (A + B) / DIV_CORNER, 0.0, 0.0],
        (false, true, false, true) => [(A + B + B + C) / DIV_CORNER, 0.0, 0.0, 0.0],
        (true, false, false, false) => [
            (B + C) / DIV_BORDER,
            (A + B) / DIV_BORDER,
            B / DIV_BORDER,
            A / DIV_BORDER,
        ],
        (false, true, false, false) => {
            [(A + B + B + C) / DIV_BORDER, 0.0, (A + B) / DIV_BORDER, 0.0]
        }
        (false, false, true, false) => [
            (B + C) / DIV_BORDER,
            B / DIV_BORDER,
            (A + B) / DIV_BORDER,
            A / DIV_BORDER,
        ],
        (false, false, false, true) => {
            [(A + B + B + C) / DIV_BORDER, (A + B) / DIV_BORDER, 0.0, 0.0]
        }
        _ => unreachable!("width/height >= 2 guarantee at most one of each axis pair"),
    }
}

fn oracle_combine(samples: [f32; 4], weights: [f32; 4]) -> f32 {
    samples
        .iter()
        .zip(weights.iter())
        .fold(0.0f32, |acc, (s, w)| acc + s * w)
}

// --- Region weight table: hand-computed literal values -----------------------
//
// All values below were computed by hand (see module doc "Weight constants
// and their sums") from the literals a=0.077847, b=0.123317, c=0.195346 and
// divisors 0.519827 (corner), 0.720991 (border), rounded through f32 at each
// step exactly as `region_weights` does. A 10x10 texture (width=height=10,
// so width-1=height-1=9) is used as the canonical non-degenerate case
// throughout.

#[test]
fn interior_weights_match_hand_computed_literals() {
    let w = region_weights(5, 5, 10, 10).as_array();
    assert_eq!(w, [0.519_827_03, 0.201_164, 0.201_164, 0.077_847]);
}

#[test]
fn interior_weights_do_not_sum_to_exactly_one_in_f32() {
    // See module doc "Weight constants and their sums": only the interior
    // region skips renormalization, leaving a real (if tiny) DC gain.
    let w = region_weights(5, 5, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.000_002);
    assert_ne!(sum, 1.0);
    assert!(sum > 1.0);
}

#[test]
fn top_left_corner_weights_match_hand_computed_literals() {
    let w = region_weights(0, 0, 10, 10).as_array();
    assert_eq!(w, [0.375_790_4, 0.237_227_01, 0.237_227_01, 0.149_755_58]);
}

#[test]
fn top_left_corner_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(0, 0, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn top_right_corner_weights_match_hand_computed_literals() {
    let w = region_weights(9, 0, 10, 10).as_array();
    assert_eq!(w, [0.613_017_4, 0.0, 0.386_982_6, 0.0]);
}

#[test]
fn top_right_corner_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(9, 0, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn bottom_left_corner_weights_match_hand_computed_literals() {
    let w = region_weights(0, 9, 10, 10).as_array();
    assert_eq!(w, [0.613_017_4, 0.386_982_6, 0.0, 0.0]);
}

#[test]
fn bottom_left_corner_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(0, 9, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn bottom_right_corner_weights_match_hand_computed_literals() {
    let w = region_weights(9, 9, 10, 10).as_array();
    assert_eq!(w, [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn bottom_right_corner_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(9, 9, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn left_border_weights_match_hand_computed_literals() {
    let w = region_weights(0, 5, 10, 10).as_array();
    assert_eq!(w, [0.441_979_17, 0.279_010_42, 0.171_038_2, 0.107_972_21]);
}

#[test]
fn left_border_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(0, 5, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn right_border_weights_match_hand_computed_literals() {
    let w = region_weights(9, 5, 10, 10).as_array();
    assert_eq!(w, [0.720_989_6, 0.0, 0.279_010_42, 0.0]);
}

#[test]
fn right_border_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(9, 5, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn top_border_weights_match_hand_computed_literals() {
    let w = region_weights(5, 0, 10, 10).as_array();
    assert_eq!(w, [0.441_979_17, 0.171_038_2, 0.279_010_42, 0.107_972_21]);
}

#[test]
fn top_border_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(5, 0, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

#[test]
fn bottom_border_weights_match_hand_computed_literals() {
    let w = region_weights(5, 9, 10, 10).as_array();
    assert_eq!(w, [0.720_989_6, 0.279_010_42, 0.0, 0.0]);
}

#[test]
fn bottom_border_weights_sum_to_exactly_one_in_f32() {
    let w = region_weights(5, 9, 10, 10).as_array();
    let sum = ((w[0] + w[1]) + w[2]) + w[3];
    assert_eq!(sum, 1.0);
}

// --- Branch-order / precedence: corners tested before edges on degenerate sizes ---

#[test]
fn single_pixel_texture_resolves_via_top_left_corner_branch() {
    // width=height=1: x==0==width-1 and y==0==height-1 simultaneously. The
    // HLSL's `if`/`else if` chain tests the top-left corner condition first,
    // so this must resolve there, not fall through to bottom-right.
    let w = region_weights(0, 0, 1, 1).as_array();
    assert_eq!(w, [0.375_790_4, 0.237_227_01, 0.237_227_01, 0.149_755_58]);
}

#[test]
fn one_wide_column_top_pixel_resolves_via_top_left_corner() {
    // width=1: x==0 and x==width-1 both hold. y==0 makes this top-left, not
    // left-border or top-border.
    let w = region_weights(0, 0, 1, 5).as_array();
    assert_eq!(w, [0.375_790_4, 0.237_227_01, 0.237_227_01, 0.149_755_58]);
}

#[test]
fn one_wide_column_bottom_pixel_resolves_via_bottom_left_corner() {
    let w = region_weights(0, 4, 1, 5).as_array();
    assert_eq!(w, [0.613_017_4, 0.386_982_6, 0.0, 0.0]);
}

// --- Tap offsets: hand-computed literal values -------------------------------

#[test]
fn tap_offsets_match_hand_computed_literals() {
    let offsets = tap_offsets();
    assert_eq!(offsets[0], [0.113_017_5, 0.113_017_5]);
    assert_eq!(offsets[1], [1.5, 0.113_017_26]);
    assert_eq!(offsets[2], [0.113_017_26, 1.5]);
}

#[test]
fn tap_offsets_are_pixel_position_independent() {
    // offsets[] depends only on the kernel constants, never on x/y/width/
    // height -- calling it twice must be bit-identical.
    assert_eq!(tap_offsets(), tap_offsets());
}

#[test]
fn offset_zero_has_equal_x_and_y_components() {
    let offsets = tap_offsets();
    assert_eq!(offsets[0][0], offsets[0][1]);
}

#[test]
fn offset_one_x_is_exactly_one_point_five() {
    let offsets = tap_offsets();
    assert_eq!(offsets[1][0], 1.5);
}

#[test]
fn offset_two_y_is_exactly_one_point_five() {
    let offsets = tap_offsets();
    assert_eq!(offsets[2][1], 1.5);
}

// --- Per-channel combine: uniform field proves DC gain -----------------------

#[test]
fn uniform_field_interior_reproduces_the_tiny_dc_gain() {
    // A uniform texel value (all four taps equal) run through the interior
    // weights should reproduce exactly the interior weight sum's departure
    // from 1.0 -- proving the DC-gain finding end-to-end through
    // `combine_channel`, not just by summing weights in isolation.
    let weights = region_weights(5, 5, 10, 10);
    let value = 0.5f32;
    let out = combine_channel([value, value, value, value], weights);
    assert_eq!(out, 0.500_001);
    assert!(out > value);
}

#[test]
fn uniform_field_every_border_and_corner_region_preserves_value_exactly() {
    // With renormalized weights (sum == 1.0 exactly), a uniform field must
    // reproduce the input value bit-exactly -- no gain, no loss.
    let value = 0.25f32;
    for (x, y) in [
        (0u32, 0u32),
        (9, 0),
        (0, 9),
        (9, 9),
        (0, 5),
        (9, 5),
        (5, 0),
        (5, 9),
    ] {
        let weights = region_weights(x, y, 10, 10);
        let out = combine_channel([value, value, value, value], weights);
        assert_eq!(out, value, "region at ({x},{y}) did not preserve DC value");
    }
}

// --- Single bright texel: proves the kernel shape tap-by-tap -----------------

#[test]
fn single_bright_texel_at_tap_zero_interior() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([1.0, 0.0, 0.0, 0.0], weights);
    assert_eq!(out, weights.w0);
    assert_eq!(out, 0.519_827_03);
}

#[test]
fn single_bright_texel_at_tap_one_interior() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([0.0, 1.0, 0.0, 0.0], weights);
    assert_eq!(out, weights.w1);
    assert_eq!(out, 0.201_164);
}

#[test]
fn single_bright_texel_at_tap_two_interior() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([0.0, 0.0, 1.0, 0.0], weights);
    assert_eq!(out, weights.w2);
    assert_eq!(out, 0.201_164);
}

#[test]
fn single_bright_texel_at_tap_three_interior() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([0.0, 0.0, 0.0, 1.0], weights);
    assert_eq!(out, weights.w3);
    assert_eq!(out, 0.077_847);
}

#[test]
fn single_bright_texel_at_tap_one_and_two_are_equal_weight_in_interior() {
    // Interior weights w1 == w2 exactly (both a+b) -- taps 1 and 2 are the
    // symmetric off-diagonal samples.
    let weights = region_weights(5, 5, 10, 10);
    assert_eq!(weights.w1, weights.w2);
}

#[test]
fn single_bright_texel_bottom_right_corner_only_tap_zero_is_nonzero() {
    // bottom-right corner weights are (1.0, 0, 0, 0): only the first tap
    // contributes at all, confirming the kernel collapses to a single
    // full-weight fetch at this corner.
    let weights = region_weights(9, 9, 10, 10);
    assert_eq!(combine_channel([1.0, 5.0, 5.0, 5.0], weights), 1.0);
    assert_eq!(combine_channel([0.0, 5.0, 5.0, 5.0], weights), 0.0);
}

#[test]
fn single_bright_texel_top_right_corner_zeroes_taps_one_and_three() {
    let weights = region_weights(9, 0, 10, 10);
    assert_eq!(weights.w1, 0.0);
    assert_eq!(weights.w3, 0.0);
    let out = combine_channel([0.0, 100.0, 0.0, 100.0], weights);
    assert_eq!(out, 0.0);
}

// --- Negative and NaN/inf inputs ---------------------------------------------

#[test]
fn negative_sample_values_combine_linearly() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([-1.0, -1.0, -1.0, -1.0], weights);
    // -(sum of weights) = -1.000002 for the interior region.
    assert_eq!(out, -1.000_002);
}

#[test]
fn nan_sample_at_a_nonzero_weight_tap_poisons_the_result() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([f32::NAN, 0.0, 0.0, 0.0], weights);
    assert!(out.is_nan());
}

#[test]
fn nan_sample_at_a_zero_weight_tap_still_poisons_the_result() {
    // 0.0 * NaN is NaN, not 0.0 -- IEEE-754 defines the product of zero and
    // NaN as NaN, so a NaN tap poisons the sum even where its own region
    // weight is exactly zero. This port does not special-case that away.
    let weights = region_weights(9, 9, 10, 10); // bottom-right: w1=w2=w3=0
    assert_eq!(weights.w1, 0.0);
    let out = combine_channel([1.0, f32::NAN, 0.0, 0.0], weights);
    assert!(out.is_nan());
}

#[test]
fn positive_infinity_sample_propagates() {
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([f32::INFINITY, 0.0, 0.0, 0.0], weights);
    assert!(out.is_infinite());
    assert!(out.is_sign_positive());
}

#[test]
fn infinity_minus_infinity_produces_nan_via_opposite_signed_taps() {
    // weights.w0 (positive) times +inf, plus weights.w1 (also positive)
    // times -inf: pos*pos + pos*neg = +inf + -inf = NaN. Demonstrates the
    // characterization captures this IEEE-754 edge, not just finite inputs.
    let weights = region_weights(5, 5, 10, 10);
    let out = combine_channel([f32::INFINITY, f32::NEG_INFINITY, 0.0, 0.0], weights);
    assert!(out.is_nan());
}

#[test]
fn nan_weight_component_from_a_hypothetical_zero_over_zero_does_not_occur() {
    // Sanity check that this shader never actually divides 0.0/0.0 anywhere
    // in its weight table (which would itself produce NaN): every division
    // is `literal / divisor` with a nonzero literal divisor, and the
    // `0.0 / divisor` cases are `0.0 / nonzero = 0.0`, not `0.0 / 0.0`.
    let weights = region_weights(9, 9, 10, 10);
    assert!(!weights.w1.is_nan());
    assert!(!weights.w2.is_nan());
    assert!(!weights.w3.is_nan());
}

// --- Differential: independent oracle vs. the port ---------------------------

#[test]
fn oracle_agrees_with_port_across_all_nine_regions() {
    let coords = [
        (5u32, 5u32), // interior
        (0, 0),       // top-left
        (9, 0),       // top-right
        (0, 9),       // bottom-left
        (9, 9),       // bottom-right
        (0, 5),       // left border
        (9, 5),       // right border
        (5, 0),       // top border
        (5, 9),       // bottom border
    ];
    for (x, y) in coords {
        let ported = region_weights(x, y, 10, 10).as_array();
        let oracle = oracle_region_weights(x, y, 10, 10);
        assert_eq!(ported, oracle, "mismatch at ({x},{y})");
    }
}

#[test]
fn oracle_combine_agrees_with_port_combine() {
    let weights = region_weights(3, 3, 10, 10);
    let samples = [0.1f32, 0.2, 0.3, 0.4];
    let ported = combine_channel(samples, weights);
    let oracle = oracle_combine(samples, weights.as_array());
    assert_eq!(ported, oracle);
}

#[test]
fn oracle_agrees_with_port_across_a_full_grid_sweep() {
    let width = 6u32;
    let height = 4u32;
    for y in 0..height {
        for x in 0..width {
            let ported = region_weights(x, y, width, height).as_array();
            let oracle = oracle_region_weights(x, y, width, height);
            assert_eq!(ported, oracle, "mismatch at ({x},{y}) of {width}x{height}");
        }
    }
}

// --- combine_rgba / GaussianTaps convenience wrapper --------------------------

#[test]
fn combine_rgba_matches_four_independent_combine_channel_calls() {
    let weights = region_weights(2, 2, 10, 10);
    let taps = GaussianTaps {
        r: [1.0, 0.0, 0.0, 0.0],
        g: [0.0, 1.0, 0.0, 0.0],
        b: [0.0, 0.0, 1.0, 0.0],
        a: [0.0, 0.0, 0.0, 1.0],
    };
    let out = combine_rgba(taps, weights);
    assert_eq!(out[0], combine_channel(taps.r, weights));
    assert_eq!(out[1], combine_channel(taps.g, weights));
    assert_eq!(out[2], combine_channel(taps.b, weights));
    assert_eq!(out[3], combine_channel(taps.a, weights));
}

#[test]
fn combine_rgba_alpha_channel_is_filtered_like_color_channels() {
    // No passthrough special-case: alpha goes through the same
    // `combine_channel` as R/G/B (module doc "Alpha channel is filtered,
    // not passed through").
    let weights = region_weights(9, 9, 10, 10); // bottom-right: only tap 0 counts
    let taps = GaussianTaps {
        r: [10.0, 0.0, 0.0, 0.0],
        g: [20.0, 0.0, 0.0, 0.0],
        b: [30.0, 0.0, 0.0, 0.0],
        a: [40.0, 0.0, 0.0, 0.0],
    };
    let out = combine_rgba(taps, weights);
    assert_eq!(out, [10.0, 20.0, 30.0, 40.0]);
}

// --- WGSL structural checks ---------------------------------------------------

#[test]
fn wgsl_source_contains_the_kernel_constants() {
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("0.077847"));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("0.123317"));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("0.195346"));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("0.519827"));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("0.720991"));
}

#[test]
fn wgsl_source_contains_the_three_functions() {
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("fn region_weights("));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("fn tap_offsets("));
    assert!(GAUSSIAN_FILTER_RGB3X3_WGSL.contains("fn combine_channel("));
}

#[test]
fn wgsl_source_has_no_compute_dispatch_scaffolding() {
    // Ticket scope: no [numthreads]/@compute entry point, no bindings.
    assert!(!GAUSSIAN_FILTER_RGB3X3_WGSL.contains("@compute"));
    assert!(!GAUSSIAN_FILTER_RGB3X3_WGSL.contains("@workgroup_size"));
    assert!(!GAUSSIAN_FILTER_RGB3X3_WGSL.contains("@group"));
    assert!(!GAUSSIAN_FILTER_RGB3X3_WGSL.contains("@binding"));
    assert!(!GAUSSIAN_FILTER_RGB3X3_WGSL.contains("textureSample"));
}

#[test]
fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(GAUSSIAN_FILTER_RGB3X3_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn malformed_wgsl_fails_to_parse() {
    // Drop everything from the last closing brace onward, leaving an
    // unclosed function body -- guaranteed invalid regardless of where in
    // the file the cut lands.
    let truncated = GAUSSIAN_FILTER_RGB3X3_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn duplicate_struct_field_fails_naga_validation() {
    let duplicate_field = GAUSSIAN_FILTER_RGB3X3_WGSL.replacen(
        "struct RegionWeights {\n    w0: f32,",
        "struct RegionWeights {\n    w0: f32,\n    w0: f32,",
        1,
    );
    // A duplicate field name is a parse-time error in WGSL (identifiers in
    // a struct scope must be unique), so this is expected to fail to parse
    // rather than parse-then-fail-validation -- either way it must not
    // silently succeed.
    assert!(naga::front::wgsl::parse_str(&duplicate_field).is_err());
}

#[test]
fn wgsl_oracle_agrees_with_rust_across_all_nine_regions() {
    // Differential (structural/textual, not GPU-executed -- matching
    // rt64_fullscreen_vs.rs's identically-scoped precedent): independently
    // re-evaluate the WGSL's exact textual formulas in Rust and confirm
    // they agree with `region_weights` across all nine regions.
    fn wgsl_region_weights(x: u32, y: u32, width: u32, height: u32) -> [f32; 4] {
        const A: f32 = 0.077847;
        const B: f32 = 0.123317;
        const C: f32 = 0.195346;
        const DIV_C: f32 = 0.519827;
        const DIV_B: f32 = 0.720991;
        if x > 0 && y > 0 && x < width - 1 && y < height - 1 {
            [A + B + B + C, A + B, A + B, A]
        } else if x == 0 && y == 0 {
            [C / DIV_C, B / DIV_C, B / DIV_C, A / DIV_C]
        } else if x == width - 1 && y == 0 {
            [(B + C) / DIV_C, 0.0 / DIV_C, (A + B) / DIV_C, 0.0 / DIV_C]
        } else if x == 0 && y == height - 1 {
            [(B + C) / DIV_C, (A + B) / DIV_C, 0.0 / DIV_C, 0.0 / DIV_C]
        } else if x == width - 1 && y == height - 1 {
            [
                (A + B + B + C) / DIV_C,
                0.0 / DIV_C,
                0.0 / DIV_C,
                0.0 / DIV_C,
            ]
        } else if x == 0 {
            [(B + C) / DIV_B, (A + B) / DIV_B, B / DIV_B, A / DIV_B]
        } else if x == width - 1 {
            [
                (A + B + B + C) / DIV_B,
                0.0 / DIV_B,
                (A + B) / DIV_B,
                0.0 / DIV_B,
            ]
        } else if y == 0 {
            [(B + C) / DIV_B, B / DIV_B, (A + B) / DIV_B, A / DIV_B]
        } else {
            [
                (A + B + B + C) / DIV_B,
                (A + B) / DIV_B,
                0.0 / DIV_B,
                0.0 / DIV_B,
            ]
        }
    }

    let coords = [
        (5u32, 5u32),
        (0, 0),
        (9, 0),
        (0, 9),
        (9, 9),
        (0, 5),
        (9, 5),
        (5, 0),
        (5, 9),
    ];
    for (x, y) in coords {
        let expected = region_weights(x, y, 10, 10).as_array();
        let actual = wgsl_region_weights(x, y, 10, 10);
        assert_eq!(expected, actual, "mismatch at ({x},{y})");
    }
}

// --- Determinism / bit-exactness ---------------------------------------------

#[test]
fn region_weights_recomputation_is_bit_exact_across_repeated_calls() {
    let first = region_weights(5, 5, 10, 10);
    let second = region_weights(5, 5, 10, 10);
    assert_eq!(first, second);
    assert_eq!(first.w0.to_bits(), second.w0.to_bits());
}

#[test]
fn tap_offsets_recomputation_is_bit_exact_across_repeated_calls() {
    let first = tap_offsets();
    let second = tap_offsets();
    for i in 0..3 {
        assert_eq!(first[i][0].to_bits(), second[i][0].to_bits());
        assert_eq!(first[i][1].to_bits(), second[i][1].to_bits());
    }
}
