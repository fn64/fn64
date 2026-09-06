use super::*;

// Every expected value below is hand-derived directly from the literal HLSL
// formulas transcribed in this module's doc comment, cross-checked with an
// independent from-scratch IEEE-754 `f32` simulation written in Python
// (`numpy.float32`, which follows real IEEE-754 hardware division/inf/NaN
// semantics rather than Python's own float, and a hand-written `clamp01`
// mirroring Rust's `f32::clamp` NaN-propagating behavior) -- never by running
// this crate's own `white_black_point`/`tonemap_exposure`/
// `post_process_tonemap`/`motion_blur_sample_offset`/`get_quality_auto` and
// copying their output.

// --- white_black_point: black/white/mid reference points -------------------

#[test]
fn white_black_point_color_at_black_point_is_zero() {
    // (0-0)/(1-0) = 0
    assert_eq!(white_black_point([0.0; 3], [1.0; 3], [0.0; 3]), [0.0; 3]);
}

#[test]
fn white_black_point_color_at_white_point_is_one() {
    // (1-0)/(1-0) = 1
    assert_eq!(white_black_point([0.0; 3], [1.0; 3], [1.0; 3]), [1.0; 3]);
}

#[test]
fn white_black_point_mid_grey_with_unit_range_is_unchanged() {
    // (0.5-0)/(1-0) = 0.5
    assert_eq!(white_black_point([0.0; 3], [1.0; 3], [0.5; 3]), [0.5; 3]);
}

#[test]
fn white_black_point_nontrivial_bounds_per_component() {
    // Component 0: (0.5-0.1)/(0.9-0.1) = 0.4/0.8, which f32 rounds to
    // 0.50000006 (not exactly 0.5) -- verified by the Python f32 oracle.
    // Components 1/2: (0.5-0.2)/(0.8-0.2) = 0.3/0.6 = 0.5 exactly;
    // (0.5-0.3)/(0.7-0.3) = 0.2/0.4 = 0.5 exactly.
    let result = white_black_point([0.1, 0.2, 0.3], [0.9, 0.8, 0.7], [0.5, 0.5, 0.5]);
    assert_eq!(result[0], 0.500_000_06_f32);
    assert_eq!(result[1], 0.5);
    assert_eq!(result[2], 0.5);
}

// --- white_black_point: out-of-range (negative and >1) inputs --------------

#[test]
fn white_black_point_negative_color_extrapolates_below_zero() {
    // (-0.5-0)/(1-0) = -0.5, no clamp inside white_black_point itself.
    assert_eq!(white_black_point([0.0; 3], [1.0; 3], [-0.5; 3]), [-0.5; 3]);
}

#[test]
fn white_black_point_color_beyond_white_extrapolates_above_one() {
    // (2-0)/(1-0) = 2, no clamp inside white_black_point itself.
    assert_eq!(white_black_point([0.0; 3], [1.0; 3], [2.0; 3]), [2.0; 3]);
}

// --- white_black_point: degenerate wp == bl (unguarded division) -----------

#[test]
fn white_black_point_degenerate_range_color_above_black_is_positive_infinity() {
    // wp == bl == 0.5, color == 0.6: (0.6-0.5)/(0.5-0.5) = 0.1/0.0 = +inf.
    let result = white_black_point([0.5; 3], [0.5; 3], [0.6; 3]);
    assert!(result.iter().all(|v| *v == f32::INFINITY));
}

#[test]
fn white_black_point_degenerate_range_color_at_black_is_nan() {
    // wp == bl == color == 0.5: (0.5-0.5)/(0.5-0.5) = 0.0/0.0 = NaN.
    let result = white_black_point([0.5; 3], [0.5; 3], [0.5; 3]);
    assert!(result.iter().all(|v| v.is_nan()));
}

#[test]
fn white_black_point_degenerate_range_color_below_black_is_negative_infinity() {
    // wp == bl == 0.5, color == 0.4: (0.4-0.5)/(0.5-0.5) = -0.1/0.0 = -inf.
    let result = white_black_point([0.5; 3], [0.5; 3], [0.4; 3]);
    assert!(result.iter().all(|v| *v == f32::NEG_INFINITY));
}

#[test]
fn white_black_point_inverted_range_wp_less_than_bl_still_the_same_literal_formula() {
    // wp=(0,0,0) < bl=(1,1,1): the literal (color-bl)/(wp-bl) formula
    // applies unconditionally, no special-casing for an inverted range.
    // (0.5-1)/(0-1) = -0.5/-1 = 0.5.
    assert_eq!(white_black_point([1.0; 3], [0.0; 3], [0.5; 3]), [0.5; 3]);
}

// --- white_black_point: NaN/inf inputs --------------------------------------

#[test]
fn white_black_point_nan_color_component_propagates_only_in_its_own_lane() {
    let result = white_black_point([0.0; 3], [1.0; 3], [f32::NAN, 0.5, 0.5]);
    assert!(result[0].is_nan());
    assert_eq!(result[1], 0.5);
    assert_eq!(result[2], 0.5);
}

#[test]
fn white_black_point_infinite_color_components_propagate_with_sign() {
    let result = white_black_point([0.0; 3], [1.0; 3], [f32::INFINITY, f32::NEG_INFINITY, 0.5]);
    assert_eq!(result[0], f32::INFINITY);
    assert_eq!(result[1], f32::NEG_INFINITY);
    assert_eq!(result[2], 0.5);
}

// --- tonemap_exposure: normal division and unguarded edge cases ------------

#[test]
fn tonemap_exposure_normal_division() {
    // 0.6 / 0.5 = 1.2
    assert_eq!(tonemap_exposure(0.6, 0.5), 1.2_f32);
}

#[test]
fn tonemap_exposure_zero_avg_luma_with_positive_numerator_is_positive_infinity() {
    assert_eq!(tonemap_exposure(0.6, 0.0), f32::INFINITY);
}

#[test]
fn tonemap_exposure_zero_over_zero_is_nan() {
    let result = tonemap_exposure(0.0, 0.0);
    assert!(result.is_nan());
}

#[test]
fn tonemap_exposure_negative_avg_luma_flips_sign() {
    // 0.6 / -0.5 = -1.2
    assert_eq!(tonemap_exposure(0.6, -0.5), -1.2_f32);
}

#[test]
fn tonemap_exposure_nan_avg_luma_propagates() {
    assert!(tonemap_exposure(0.6, f32::NAN).is_nan());
}

// --- post_process_tonemap: black, white, mid-grey ---------------------------

#[test]
fn post_process_tonemap_black_input_with_identity_params_stays_black() {
    // max(0,0)=0; exposure=0.6/1.0=0.6; 0*0.6=0; (0-0)/(1-0)=0; clamp(0,0,1)=0.
    let result = post_process_tonemap([0.0; 3], 0.6, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [0.0; 3]);
}

#[test]
fn post_process_tonemap_white_input_overexposed_clamps_to_one() {
    // max(1,0)=1; exposure=2.0/1.0=2.0; 1*2=2; (2-0)/(1-0)=2; clamp(2,0,1)=1.
    let result = post_process_tonemap([1.0; 3], 2.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [1.0; 3]);
}

#[test]
fn post_process_tonemap_mid_grey_with_unit_exposure_and_identity_range_is_unchanged() {
    // max(0.5,0)=0.5; exposure=1.0/1.0=1.0; 0.5*1=0.5; (0.5-0)/(1-0)=0.5;
    // clamp(0.5,0,1)=0.5.
    let result = post_process_tonemap([0.5; 3], 1.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [0.5; 3]);
}

#[test]
fn post_process_tonemap_rt64_default_like_parameters() {
    // exposure=0.6/0.6667 (RT64's own defaults: tonemapExposure=0.6f,
    // representative non-degenerate avgLuma); color=0.4 pre-exposure.
    // Verified via the Python f32 oracle to 0.3599820137023926.
    let result = post_process_tonemap([0.4; 3], 0.6, 0.6667, [0.0; 3], [1.0; 3]);
    for v in result {
        assert_eq!(v, 0.359_982_01_f32);
    }
}

// --- post_process_tonemap: out-of-range (negative and >1) inputs -----------

#[test]
fn post_process_tonemap_negative_input_color_is_floored_to_zero_by_max() {
    // max(-0.5,0)=0 for every component, then the rest of the chain stays 0.
    let result = post_process_tonemap([-0.5; 3], 1.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [0.0; 3]);
}

#[test]
fn post_process_tonemap_heavy_overexposure_clamps_to_one_not_the_raw_value() {
    // max(1,0)=1; exposure=10.0/1.0=10; 1*10=10; (10-0)/(1-0)=10; clamp->1.
    let result = post_process_tonemap([1.0; 3], 10.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [1.0; 3]);
}

// --- post_process_tonemap: NaN/inf inputs -----------------------------------

#[test]
fn post_process_tonemap_nan_input_color_component_is_suppressed_to_zero_by_max() {
    // f32::max(NaN, 0.0) suppresses to 0.0 (IEEE fmax-style, NaN-non-
    // propagating when the other operand is a number) -- unlike the final
    // clamp, which propagates NaN. So a NaN *input* here becomes 0.0 in its
    // own lane after the floor step, then stays 0.0 through the rest of the
    // chain (0*exposure=0, (0-0)/(1-0)=0, clamp(0,0,1)=0).
    let result = post_process_tonemap([f32::NAN, 0.5, 0.5], 1.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result[0], 0.0);
    assert_eq!(result[1], 0.5);
    assert_eq!(result[2], 0.5);
}

#[test]
fn post_process_tonemap_infinite_input_color_component_clamps_to_one() {
    // max(inf,0)=inf; inf*1=inf; (inf-0)/(1-0)=inf; clamp(inf,0,1)=1.0
    // (f32::clamp on a finite, ordered, out-of-range value clamps normally --
    // infinity is not NaN, so this is the ordinary clamp path, not the
    // NaN-propagation path).
    let result = post_process_tonemap([f32::INFINITY, 0.5, 0.5], 1.0, 1.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result[0], 1.0);
    assert_eq!(result[1], 0.5);
    assert_eq!(result[2], 0.5);
}

#[test]
fn post_process_tonemap_zero_avg_luma_produces_infinite_exposure_then_clamps_to_one() {
    // exposure = 0.6/0.0 = +inf; 0.5*inf = inf; (inf-0)/(1-0) = inf;
    // clamp(inf,0,1) = 1.0.
    let result = post_process_tonemap([0.5; 3], 0.6, 0.0, [0.0; 3], [1.0; 3]);
    assert_eq!(result, [1.0; 3]);
}

#[test]
fn post_process_tonemap_zero_color_times_infinite_exposure_is_nan_and_propagates_through_clamp() {
    // max(0,0)=0; exposure=0.6/0.0=+inf; 0*inf=NaN; (NaN-0)/(1-0)=NaN;
    // f32::clamp(NaN,0,1) propagates NaN unchanged (this module's
    // documented, established `clamp` convention -- see module doc
    // "saturate/clamp semantics").
    let result = post_process_tonemap([0.0; 3], 0.6, 0.0, [0.0; 3], [1.0; 3]);
    assert!(result.iter().all(|v| v.is_nan()));
}

// --- get_quality_auto: every threshold, exact boundary and one epsilon -----
// each side (the ticket's explicit requirement). All five comparisons are
// `<=` (inclusive): the threshold pixel count itself belongs to the
// LOWER/higher-quality tier, and the very next integer pixel count belongs
// to the next (lower-quality) tier.

#[test]
fn get_quality_auto_720p_exact_boundary_is_ultra_quality() {
    // 1280*720 = 921_600, the Pixels720p threshold itself.
    assert_eq!(get_quality_auto(1280, 720), QualityMode::UltraQuality);
}

#[test]
fn get_quality_auto_one_pixel_above_720p_boundary_is_quality() {
    // 921_601 pixels (e.g. 1281x720 = 922_320, safely above by more than the
    // literal +1 but still strictly the very next tier) crosses to Quality.
    assert_eq!(get_quality_auto(1281, 720), QualityMode::Quality);
}

#[test]
fn get_quality_auto_one_pixel_below_720p_boundary_is_still_ultra_quality() {
    // 1279*720 = 920_880 < 921_600: still within UltraQuality, the epsilon
    // just below the threshold from the other direction.
    assert_eq!(get_quality_auto(1279, 720), QualityMode::UltraQuality);
}

#[test]
fn get_quality_auto_1080p_exact_boundary_is_quality() {
    // 1920*1080 = 2_073_600, the Pixels1080p threshold itself.
    assert_eq!(get_quality_auto(1920, 1080), QualityMode::Quality);
}

#[test]
fn get_quality_auto_one_pixel_above_1080p_boundary_is_balanced() {
    assert_eq!(get_quality_auto(1921, 1080), QualityMode::Balanced);
}

#[test]
fn get_quality_auto_one_pixel_below_1080p_boundary_is_still_quality() {
    assert_eq!(get_quality_auto(1919, 1080), QualityMode::Quality);
}

#[test]
fn get_quality_auto_1440p_exact_boundary_is_balanced() {
    // 2560*1440 = 3_686_400, the Pixels1440p threshold itself.
    assert_eq!(get_quality_auto(2560, 1440), QualityMode::Balanced);
}

#[test]
fn get_quality_auto_one_pixel_above_1440p_boundary_is_performance() {
    assert_eq!(get_quality_auto(2561, 1440), QualityMode::Performance);
}

#[test]
fn get_quality_auto_one_pixel_below_1440p_boundary_is_still_balanced() {
    assert_eq!(get_quality_auto(2559, 1440), QualityMode::Balanced);
}

#[test]
fn get_quality_auto_4k_exact_boundary_is_performance() {
    // 3840*2160 = 8_294_400, the Pixels4K threshold itself.
    assert_eq!(get_quality_auto(3840, 2160), QualityMode::Performance);
}

#[test]
fn get_quality_auto_one_pixel_above_4k_boundary_is_ultra_performance() {
    assert_eq!(get_quality_auto(3841, 2160), QualityMode::UltraPerformance);
}

#[test]
fn get_quality_auto_one_pixel_below_4k_boundary_is_still_performance() {
    assert_eq!(get_quality_auto(3839, 2160), QualityMode::Performance);
}

#[test]
fn get_quality_auto_far_above_4k_is_ultra_performance() {
    // 7680x4320 (8K): 33_177_600, far past Pixels4K.
    assert_eq!(get_quality_auto(7680, 4320), QualityMode::UltraPerformance);
}

// --- get_quality_auto: degenerate/edge display dimensions ------------------

#[test]
fn get_quality_auto_one_by_one_pixel_is_ultra_quality() {
    assert_eq!(get_quality_auto(1, 1), QualityMode::UltraQuality);
}

#[test]
fn get_quality_auto_zero_by_zero_does_not_panic_and_is_ultra_quality() {
    // The source's assert(displayWidth > 0) is a debug-only precondition,
    // not ported (see module doc "Ported vs. skipped") -- this function
    // must not panic on non-positive input. Product is 0, <= every
    // threshold, so it lands in the first (UltraQuality) tier.
    assert_eq!(get_quality_auto(0, 0), QualityMode::UltraQuality);
}

#[test]
fn get_quality_auto_negative_dimensions_do_not_panic_and_land_in_ultra_quality() {
    // A negative product (e.g. one negative, one positive dimension) is
    // still <= 921_600, landing in UltraQuality via the same `<=` ladder --
    // no special-casing for a negative product.
    assert_eq!(get_quality_auto(-100, 100), QualityMode::UltraQuality);
    assert_eq!(get_quality_auto(-100, -100), QualityMode::UltraQuality);
}

#[test]
fn get_quality_auto_large_dimensions_use_i64_widening_without_overflow_panic() {
    // 65536 * 65536 = 4_294_967_296, which overflows i32 (and would panic
    // Rust's debug-mode i32 multiply) but fits comfortably in i64 -- this
    // exercises the "widen before multiplying beyond i32 range" choice this
    // port makes to avoid reproducing C++ signed-overflow UB (see module doc
    // "int * int then widen to uint64_t").
    assert_eq!(
        get_quality_auto(65536, 65536),
        QualityMode::UltraPerformance
    );
}

// --- get_quality_auto: comparison direction sanity (all five tiers reachable)

#[test]
fn get_quality_auto_all_five_tiers_are_reachable_and_ordered() {
    let tiers = [
        get_quality_auto(1280, 720),
        get_quality_auto(1920, 1080),
        get_quality_auto(2560, 1440),
        get_quality_auto(3840, 2160),
        get_quality_auto(7680, 4320),
    ];
    assert_eq!(
        tiers,
        [
            QualityMode::UltraQuality,
            QualityMode::Quality,
            QualityMode::Balanced,
            QualityMode::Performance,
            QualityMode::UltraPerformance,
        ]
    );
}

// --- motion_blur_is_active / motion_blur_sample_count -----------------------

#[test]
fn motion_blur_is_active_requires_both_positive_strength_and_nonzero_samples() {
    assert!(motion_blur_is_active(0.5, 4));
    assert!(!motion_blur_is_active(0.0, 4));
    assert!(!motion_blur_is_active(-0.5, 4));
    assert!(!motion_blur_is_active(0.5, 0));
}

#[test]
fn motion_blur_sample_count_returns_the_field_unchanged() {
    assert_eq!(motion_blur_sample_count(32), 32);
    assert_eq!(motion_blur_sample_count(0), 0);
}

// --- motion_blur_sample_offset: representative sample sequence -------------

#[test]
fn motion_blur_sample_offset_first_sample_matches_start_uv() {
    // uv=(0.5,0.5), flow=(0.1,-0.2), strength=1.0, samples=4:
    // startUV = uv - flow*strength/2 = (0.5-0.05, 0.5-(-0.1)) = (0.45, 0.6).
    // s=0: sampleUV = clamp(startUV + flow*0*step, 0, 1) = startUV.
    let result = motion_blur_sample_offset([0.5, 0.5], [0.1, -0.2], 1.0, 4, 0);
    assert_eq!(result[0], 0.45_f32);
    assert_eq!(result[1], 0.6_f32);
}

#[test]
fn motion_blur_sample_offset_last_sample_of_four() {
    // step = 1.0/4 = 0.25. s=3: raw = (0.45 + 0.1*3*0.25, 0.6 + (-0.2)*3*0.25)
    // = (0.45+0.075, 0.6-0.15) = (0.525, 0.45000002) per the f32 oracle.
    let result = motion_blur_sample_offset([0.5, 0.5], [0.1, -0.2], 1.0, 4, 3);
    assert_eq!(result[0], 0.525_f32);
    assert_eq!(result[1], 0.450_000_02_f32);
}

#[test]
fn motion_blur_sample_offset_zero_flow_is_a_fixed_point_at_every_sample() {
    // flow=(0,0): startUV = uv unchanged, and the s-dependent term is always
    // 0, so every sample lands exactly on uv (already within [0,1]).
    let result = motion_blur_sample_offset([0.3, 0.7], [0.0, 0.0], 1.0, 4, 0);
    assert_eq!(result, [0.3, 0.7]);
    let result3 = motion_blur_sample_offset([0.3, 0.7], [0.0, 0.0], 1.0, 4, 3);
    assert_eq!(result3, [0.3, 0.7]);
}

#[test]
fn motion_blur_sample_offset_single_sample_step_equals_strength() {
    // samples=1: SampleStep = strength/1 = strength. s=0: raw = startUV.
    // uv=(0.5,0.5), flow=(0.2,0.2), strength=2.0:
    // startUV = (0.5 - 0.2*2/2, 0.5 - 0.2*2/2) = (0.3, 0.3).
    let result = motion_blur_sample_offset([0.5, 0.5], [0.2, 0.2], 2.0, 1, 0);
    assert_eq!(result, [0.3, 0.3]);
}

// --- motion_blur_sample_offset: out-of-range (negative and >1) inputs, ------
// exercising the clamp

#[test]
fn motion_blur_sample_offset_uv_already_above_one_clamps_down_to_one() {
    // flow=(0,0), so raw == uv == (1.5,1.5) unchanged by the offset terms,
    // then clamp(1.5,0,1) = 1.0 in both lanes.
    let result = motion_blur_sample_offset([1.5, 1.5], [0.0, 0.0], 1.0, 4, 0);
    assert_eq!(result, [1.0, 1.0]);
}

#[test]
fn motion_blur_sample_offset_uv_already_below_zero_clamps_up_to_zero() {
    let result = motion_blur_sample_offset([-0.5, -0.5], [0.0, 0.0], 1.0, 4, 0);
    assert_eq!(result, [0.0, 0.0]);
}

#[test]
fn motion_blur_sample_offset_large_positive_flow_drives_raw_past_one_then_clamps() {
    // uv=(0.5,0.5), flow=(10,10), strength=2.0, samples=4, s=3:
    // startUV = 0.5 - 10*2/2 = 0.5-10 = -9.5. step=2/4=0.5.
    // raw = -9.5 + 10*3*0.5 = -9.5+15 = 5.5 (per the f32 oracle), then
    // clamp(5.5,0,1) = 1.0.
    let result = motion_blur_sample_offset([0.5, 0.5], [10.0, 10.0], 2.0, 4, 3);
    assert_eq!(result, [1.0, 1.0]);
}

#[test]
fn motion_blur_sample_offset_large_negative_flow_drives_raw_below_zero_then_clamps() {
    // Mirror of the positive case: raw = -4.5 (per the f32 oracle), clamped
    // to 0.0.
    let result = motion_blur_sample_offset([0.5, 0.5], [-10.0, -10.0], 2.0, 4, 3);
    assert_eq!(result, [0.0, 0.0]);
}

// --- motion_blur_sample_offset: NaN/inf inputs ------------------------------

#[test]
fn motion_blur_sample_offset_nan_uv_propagates_through_clamp() {
    // f32::clamp(NaN, 0, 1) propagates NaN unchanged (this module's
    // documented `clamp` convention).
    let result = motion_blur_sample_offset([f32::NAN, 0.5], [0.0, 0.0], 1.0, 4, 0);
    assert!(result[0].is_nan());
    assert_eq!(result[1], 0.5);
}

#[test]
fn motion_blur_sample_offset_infinite_flow_produces_nan_in_that_lane_at_every_sample() {
    // flow.x = +inf: startUV.x = uv.x - inf*strength/2 = 0.5 - inf = -inf
    // (finite minus infinity is well-defined). The per-sample term is
    // flow.x * s * SampleStep = inf * s * step -- and IEEE-754 `inf * 0.0`
    // is NaN (not 0.0), so this is NaN even at s=0 (int 0 widened to f32
    // 0.0), before SampleStep's own multiply even runs. raw.x = -inf + NaN
    // = NaN at every sample index, which f32::clamp propagates unchanged
    // (this module's documented NaN-propagating `clamp` convention) --
    // verified independently by the Python f32 oracle at both s=0 and s=1.
    let s0 = motion_blur_sample_offset([0.5, 0.5], [f32::INFINITY, 0.0], 1.0, 4, 0);
    assert!(s0[0].is_nan());
    assert_eq!(s0[1], 0.5);
    let s1 = motion_blur_sample_offset([0.5, 0.5], [f32::INFINITY, 0.0], 1.0, 4, 1);
    assert!(s1[0].is_nan());
    assert_eq!(s1[1], 0.5);
}

// --- QualityMode: explicit discriminant values match the pinned header -----

#[test]
fn quality_mode_discriminants_match_the_pinned_header_enum_values() {
    assert_eq!(QualityMode::UltraPerformance as i32, 0);
    assert_eq!(QualityMode::Performance as i32, 1);
    assert_eq!(QualityMode::Balanced as i32, 2);
    assert_eq!(QualityMode::Quality as i32, 3);
    assert_eq!(QualityMode::UltraQuality as i32, 4);
}

// --- White/black point + exposure composition sanity ------------------------

#[test]
fn post_process_tonemap_nontrivial_black_white_bounds_compose_with_exposure() {
    // black=(0.1,0.1,0.1), white=(0.9,0.9,0.9), input color=(0.5,0.5,0.5),
    // exposure=1.0, avgLuma=1.0: max(0.5,0)=0.5; 0.5*1=0.5;
    // (0.5-0.1)/(0.9-0.1) = 0.4/0.8 = 0.50000006 (f32 rounding, matching the
    // white_black_point nontrivial-bounds case above); clamp(0.50000006,0,1)
    // is a no-op (already in range).
    let result = post_process_tonemap([0.5; 3], 1.0, 1.0, [0.1; 3], [0.9; 3]);
    for v in result {
        assert_eq!(v, 0.500_000_06_f32);
    }
}
