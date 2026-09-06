//! Hand-derived characterization tests for [`super`]'s port of
//! `RSPProcessCS.hlsl`. Every expected value below is derived by hand from
//! the quoted source arithmetic; none is captured from a run.

use super::*;

const EPS: f32 = 1e-5;

fn approx(a: f32, b: f32) {
    assert!((a - b).abs() < EPS, "{a} !~= {b}");
}

fn approx_vec3(a: Vec3, b: Vec3) {
    approx(a.x, b.x);
    approx(a.y, b.y);
    approx(a.z, b.z);
}

fn approx_vec4(a: Vec4, b: Vec4) {
    approx(a.x, b.x);
    approx(a.y, b.y);
    approx(a.z, b.z);
    approx(a.w, b.w);
}

fn identity() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// Rotate +90 degrees about Z (`x' = -y`, `y' = x`), no translation.
fn rot_z90() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(0.0, -1.0, 0.0, 0.0),
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// Translate by `(10, 0, 0)`.
fn translate_x10() -> Mat4 {
    Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 10.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

fn light(pos_dir: Vec3, col: Vec3, kc: u32, kl: u32, kq: u32) -> RspLight {
    RspLight {
        pos_dir,
        col,
        colc: Vec3::default(),
        kc,
        kl,
        kq,
    }
}

fn zero_vertex() -> RspProcessVertex {
    RspProcessVertex {
        pos: Vec3::new(0.0, 0.0, 0.0),
        vel: Vec3::new(0.0, 0.0, 0.0),
        norm: [0, 0, 0],
        col: [0, 0, 0, 0],
        tc: [0.0, 0.0],
        tc_vel: [0.0, 0.0],
        fog_index: 0,
        look_at_index: 0,
    }
}

// =====================================================================
// rsp_process_norm -- lines 57-61
// =====================================================================

#[test]
fn norm_divides_each_component_by_127_not_128() {
    // 127/127 = 1 exactly; 64/127 = 0.503937007874...; 0/127 = 0.
    approx_vec3(
        rsp_process_norm([127, 64, 0]),
        Vec3::new(1.0, 64.0 / 127.0, 0.0),
    );
    // A /128 divisor would have given 127/128 = 0.9921875 for the first
    // component; assert the port is NOT that.
    assert!((rsp_process_norm([127, 0, 0]).x - 0.9921875).abs() > 1e-3);
}

#[test]
fn norm_is_signed_and_most_negative_input_exceeds_unit_magnitude() {
    // srcNorm is Buffer<int>: -128/127 = -1.0078740157480315, magnitude > 1.
    let got = rsp_process_norm([-128, -127, -1]);
    approx(got.x, -1.007_874);
    assert!(got.x < -1.0, "{} should exceed unit magnitude", got.x);
    approx(got.y, -1.0);
    approx(got.z, -1.0 / 127.0);
}

#[test]
fn norm_applies_no_clamp_to_out_of_s8_range_inputs() {
    // 1270/127 = 10 exactly; the port adds no range guard.
    approx_vec3(
        rsp_process_norm([1270, -1270, 254]),
        Vec3::new(10.0, -10.0, 2.0),
    );
}

// =====================================================================
// rsp_process_color_bytes -- lines 77, 115-117
// =====================================================================

#[test]
fn color_bytes_divide_by_255_and_map_full_scale_to_one() {
    approx_vec4(
        rsp_process_color_bytes([255, 0, 128, 255]),
        Vec4::new(1.0, 0.0, 128.0 / 255.0, 1.0),
    );
}

#[test]
fn color_bytes_alpha_is_the_fourth_element_not_the_first() {
    // srcCol[normColIndex + 3] is alpha (line 77); +0/+1/+2 are RGB.
    let got = rsp_process_color_bytes([10, 20, 30, 40]);
    approx(got.x, 10.0 / 255.0);
    approx(got.w, 40.0 / 255.0);
}

#[test]
fn color_divisor_differs_from_the_normal_divisor_on_the_same_byte() {
    // The same wire byte 127 becomes 1.0 as a normal and 127/255 as a color.
    approx(rsp_process_norm([127, 0, 0]).x, 1.0);
    approx(rsp_process_color_bytes([127, 0, 0, 0]).x, 127.0 / 255.0);
}

// =====================================================================
// rsp_process_composed_matrix -- line 66's mul(A, B)
// =====================================================================

#[test]
fn composed_matrix_with_identity_on_either_side_is_a_fixed_point() {
    let a = rot_z90();
    let left = rsp_process_composed_matrix(identity(), a);
    let right = rsp_process_composed_matrix(a, identity());
    for i in 0..4 {
        approx_vec4(left.rows[i], a.rows[i]);
        approx_vec4(right.rows[i], a.rows[i]);
    }
}

#[test]
fn composed_matrix_is_view_proj_times_world_and_is_not_commutative() {
    // A = rot_z90, B = translate_x10.
    // A·B applies B first: point (0,0,0,1) -> translate to (10,0,0) ->
    // rotate to (0,10,0).
    let ab = rsp_process_composed_matrix(rot_z90(), translate_x10());
    approx_vec4(
        ab.transform_point(Vec4::new(0.0, 0.0, 0.0, 1.0)),
        Vec4::new(0.0, 10.0, 0.0, 1.0),
    );
    // B·A applies A first: (0,0,0,1) -> rotate to (0,0,0) -> translate to
    // (10,0,0). Different, so the operand order is observable.
    let ba = rsp_process_composed_matrix(translate_x10(), rot_z90());
    approx_vec4(
        ba.transform_point(Vec4::new(0.0, 0.0, 0.0, 1.0)),
        Vec4::new(10.0, 0.0, 0.0, 1.0),
    );
}

#[test]
fn composed_matrix_matches_the_hand_computed_row_by_column_product() {
    // A = [[1,2,3,4],[5,6,7,8],[9,10,11,12],[13,14,15,16]]
    // B = identity with B[0][1] = 2, i.e. row0 = (1,2,0,0).
    // (A·B)[i][0] = A[i][0]*1 = A[i][0].
    // (A·B)[i][1] = A[i][0]*2 + A[i][1]*1.
    // (A·B)[i][2] = A[i][2]; (A·B)[i][3] = A[i][3].
    let a = Mat4::from_rows([
        Vec4::new(1.0, 2.0, 3.0, 4.0),
        Vec4::new(5.0, 6.0, 7.0, 8.0),
        Vec4::new(9.0, 10.0, 11.0, 12.0),
        Vec4::new(13.0, 14.0, 15.0, 16.0),
    ]);
    let b = Mat4::from_rows([
        Vec4::new(1.0, 2.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ]);
    let got = rsp_process_composed_matrix(a, b);
    approx_vec4(got.rows[0], Vec4::new(1.0, 1.0 * 2.0 + 2.0, 3.0, 4.0));
    approx_vec4(got.rows[1], Vec4::new(5.0, 5.0 * 2.0 + 6.0, 7.0, 8.0));
    approx_vec4(got.rows[2], Vec4::new(9.0, 9.0 * 2.0 + 10.0, 11.0, 12.0));
    approx_vec4(got.rows[3], Vec4::new(13.0, 13.0 * 2.0 + 14.0, 15.0, 16.0));
}

#[test]
fn composed_matrix_zero_left_operand_annihilates() {
    let zeros = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
    let got = rsp_process_composed_matrix(zeros, rot_z90());
    for i in 0..4 {
        approx_vec4(got.rows[i], Vec4::new(0.0, 0.0, 0.0, 0.0));
    }
}

// =====================================================================
// rsp_process_tf_pos -- line 66 in full
// =====================================================================

#[test]
fn tf_pos_at_unit_frame_weight_ignores_velocity_entirely() {
    // 1 - 1 = 0, so pos - vel*0 = pos.
    let got = rsp_process_tf_pos(
        identity(),
        identity(),
        Vec3::new(3.0, 4.0, 5.0),
        Vec3::new(100.0, 200.0, 300.0),
        1.0,
    );
    approx_vec4(got, Vec4::new(3.0, 4.0, 5.0, 1.0));
}

#[test]
fn tf_pos_at_zero_frame_weight_subtracts_the_full_velocity() {
    // 1 - 0 = 1, so pos - vel*1 = (3-1, 4-2, 5-3) = (2, 2, 2).
    let got = rsp_process_tf_pos(
        identity(),
        identity(),
        Vec3::new(3.0, 4.0, 5.0),
        Vec3::new(1.0, 2.0, 3.0),
        0.0,
    );
    approx_vec4(got, Vec4::new(2.0, 2.0, 2.0, 1.0));
}

#[test]
fn tf_pos_applies_world_before_view_proj() {
    // world = translate(10,0,0), viewProj = rot_z90.
    // pos (0,0,0) -> world (10,0,0) -> rot (0,10,0).
    let got = rsp_process_tf_pos(
        rot_z90(),
        translate_x10(),
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.0),
        1.0,
    );
    approx_vec4(got, Vec4::new(0.0, 10.0, 0.0, 1.0));
}

#[test]
fn tf_pos_sets_the_input_w_to_one_before_transforming() {
    // A world matrix whose bottom row is (0,0,0,7) makes w = 7*1 = 7,
    // proving the input w is 1.0 and not, say, 0.0.
    let m = Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 7.0),
    ]);
    let got = rsp_process_tf_pos(
        identity(),
        m,
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(0.0, 0.0, 0.0),
        1.0,
    );
    approx(got.w, 7.0);
}

// =====================================================================
// rsp_process_fog_index -- lines 71-72
// =====================================================================

#[test]
fn fog_index_zero_is_the_no_fog_sentinel() {
    assert_eq!(rsp_process_fog_index(0), None);
}

#[test]
fn fog_index_is_one_based_on_the_wire() {
    assert_eq!(rsp_process_fog_index(1), Some(0));
    assert_eq!(rsp_process_fog_index(5), Some(4));
    assert_eq!(rsp_process_fog_index(u32::MAX), Some(u32::MAX - 1));
}

// =====================================================================
// rsp_process_fog_alpha -- lines 73-74
// =====================================================================

fn fog(mul: f32, offset: f32) -> RspFog {
    RspFog { mul, offset }
}

#[test]
fn fog_alpha_midscale_case_is_hand_computable() {
    // z = 1, w = 2 -> max(1,0)/2 = 0.5; *255 = 127.5; +0 = 127.5;
    // /255 = 0.5; clamp -> 0.5.
    approx(rsp_process_fog_alpha(1.0, 2.0, fog(255.0, 0.0)), 0.5);
}

#[test]
fn fog_alpha_floors_negative_z_to_zero_before_the_divide() {
    // max(-100, 0) = 0 -> 0/2 = 0 -> 0*255 + 0 = 0 -> 0/255 = 0.
    approx(rsp_process_fog_alpha(-100.0, 2.0, fog(255.0, 0.0)), 0.0);
    // With a nonzero offset the floor is observable: offset 51/255 = 0.2.
    approx(rsp_process_fog_alpha(-100.0, 2.0, fog(255.0, 51.0)), 0.2);
}

#[test]
fn fog_alpha_clamps_above_one() {
    // z/w = 1 -> 1*1000 + 0 = 1000 -> /255 = 3.92... -> clamp -> 1.0.
    approx(rsp_process_fog_alpha(1.0, 1.0, fog(1000.0, 0.0)), 1.0);
}

#[test]
fn fog_alpha_clamps_below_zero_via_a_negative_offset() {
    // z/w = 0 -> 0*1 + (-500) = -500 -> /255 = -1.96... -> clamp -> 0.0.
    approx(rsp_process_fog_alpha(0.0, 1.0, fog(1.0, -500.0)), 0.0);
}

#[test]
fn fog_alpha_offset_is_added_after_the_multiply_not_before() {
    // If offset were added before the mul: (0.5 + 51) * 255 / 255 = 51.5,
    // clamped to 1.0. Written order: 0.5*255 + 51 = 178.5, /255 = 0.7.
    approx(rsp_process_fog_alpha(1.0, 2.0, fog(255.0, 51.0)), 0.7);
}

#[test]
fn fog_alpha_uses_hlsl_max_semantics_and_propagates_a_nan_z() {
    // HLSL max(a,b) == a < b ? b : a; NaN < 0 is false, so the result is
    // the NaN first argument. Rust's f32::max would have returned 0.0.
    let got = rsp_process_fog_alpha(f32::NAN, 2.0, fog(255.0, 0.0));
    assert!(got.is_nan(), "expected NaN, got {got}");
}

#[test]
fn fog_alpha_negative_zero_z_takes_the_max_first_argument_branch() {
    // -0.0 < 0.0 is false, so max returns the first argument, -0.0.
    // -0.0 / 2.0 = -0.0; *255 = -0.0; +0.0 = +0.0 (IEEE: -0 + +0 = +0).
    let got = rsp_process_fog_alpha(-0.0, 2.0, fog(255.0, 0.0));
    approx(got, 0.0);
    assert!(
        !got.is_sign_negative(),
        "-0.0 + 0.0 should be +0.0, got a negative zero"
    );
}

#[test]
fn fog_alpha_divides_by_a_zero_w_with_no_guard_yielding_infinity() {
    // The near-clip HACK runs LATER (line 123), so the fog block sees w = 0.
    // max(1,0)/0 = +Inf -> *1 + 0 = +Inf -> /255 = +Inf -> clamp -> 1.0.
    approx(rsp_process_fog_alpha(1.0, 0.0, fog(1.0, 0.0)), 1.0);
    // With a negative mul: +Inf * -1 = -Inf -> clamp -> 0.0.
    approx(rsp_process_fog_alpha(1.0, 0.0, fog(-1.0, 0.0)), 0.0);
}

#[test]
fn fog_alpha_zero_over_zero_is_nan_and_survives_the_clamp() {
    // z <= 0 floors to +0.0, then +0.0 / 0.0 = NaN.
    // HLSL clamp = min(max(NaN,0),1): max(NaN,0) = NaN (false comparison
    // returns first arg), min(NaN,1) = NaN. So NaN survives.
    let got = rsp_process_fog_alpha(0.0, 0.0, fog(1.0, 0.0));
    assert!(got.is_nan(), "expected NaN, got {got}");
    let got_neg = rsp_process_fog_alpha(-5.0, 0.0, fog(1.0, 0.0));
    assert!(got_neg.is_nan(), "expected NaN, got {got_neg}");
}

#[test]
fn fog_alpha_negative_w_flips_the_ratio_sign() {
    // max(1,0) / -2 = -0.5; *255 = -127.5; /255 = -0.5; clamp -> 0.0.
    approx(rsp_process_fog_alpha(1.0, -2.0, fog(255.0, 0.0)), 0.0);
    // Offset it back into range: -127.5 + 191.25 = 63.75; /255 = 0.25.
    approx(rsp_process_fog_alpha(1.0, -2.0, fog(255.0, 191.25)), 0.25);
}

// =====================================================================
// rsp_process_look_at_index -- lines 84-86
// =====================================================================

#[test]
fn look_at_index_zero_is_disabled_with_a_zero_index() {
    let d = rsp_process_look_at_index(0);
    assert_eq!(
        d,
        LookAtIndexDecode {
            enabled: false,
            linear: false,
            extracted_index: 0,
        }
    );
}

#[test]
fn look_at_index_enabled_bit_is_the_low_bit() {
    assert!(rsp_process_look_at_index(0x1).enabled);
    assert!(!rsp_process_look_at_index(0x2).enabled);
}

#[test]
fn look_at_index_linear_bit_is_bit_one() {
    assert!(rsp_process_look_at_index(0x2).linear);
    assert!(!rsp_process_look_at_index(0x1).linear);
    assert!(rsp_process_look_at_index(0x3).linear);
}

#[test]
fn look_at_index_shift_of_two_discards_both_flag_bits() {
    // 0b1101_11 = 55: index bits are 0b1101 = 13, flags are 0b11.
    let d = rsp_process_look_at_index(55);
    assert!(d.enabled);
    assert!(d.linear);
    assert_eq!(d.extracted_index, 13);
    // The same index with both flags clear extracts identically.
    assert_eq!(rsp_process_look_at_index(52).extracted_index, 13);
}

#[test]
fn look_at_index_shift_is_logical_not_arithmetic_on_the_high_bit() {
    // u32 >> 2 on 0x8000_0000 fills with zeros: 0x2000_0000.
    assert_eq!(
        rsp_process_look_at_index(0x8000_0000).extracted_index,
        0x2000_0000
    );
}

#[test]
fn look_at_index_decode_reuses_the_shared_constants() {
    // Pin the constants this decode depends on, sourced from fn64-render-ir
    // rather than re-declared here.
    assert_eq!(RSP_LOOKAT_INDEX_ENABLED, 0x1);
    assert_eq!(RSP_LOOKAT_INDEX_LINEAR, 0x2);
    assert_eq!(RSP_LOOKAT_INDEX_SHIFT, 2);
}

// =====================================================================
// rsp_process_tc_velocity -- lines 91-92
// =====================================================================

#[test]
fn tc_velocity_at_unit_frame_weight_leaves_tc_unchanged() {
    let got = rsp_process_tc_velocity([5.0, -7.0], [100.0, 200.0], 1.0);
    approx(got[0], 5.0);
    approx(got[1], -7.0);
}

#[test]
fn tc_velocity_at_zero_frame_weight_subtracts_the_full_velocity() {
    let got = rsp_process_tc_velocity([5.0, -7.0], [2.0, 3.0], 0.0);
    approx(got[0], 3.0);
    approx(got[1], -10.0);
}

#[test]
fn tc_velocity_at_half_frame_weight_subtracts_half() {
    // 1 - 0.5 = 0.5; 5 - 4*0.5 = 3; -7 - 8*0.5 = -11.
    let got = rsp_process_tc_velocity([5.0, -7.0], [4.0, 8.0], 0.5);
    approx(got[0], 3.0);
    approx(got[1], -11.0);
}

#[test]
fn tc_velocity_subtracts_rather_than_adds() {
    // A negative velocity increases tc, confirming the sign of the operator.
    let got = rsp_process_tc_velocity([0.0, 0.0], [-3.0, -4.0], 0.0);
    approx(got[0], 3.0);
    approx(got[1], 4.0);
}

// =====================================================================
// rsp_process_saturate_color -- line 111
// =====================================================================

#[test]
fn saturate_color_caps_components_above_one() {
    approx_vec3(
        rsp_process_saturate_color(Vec3::new(2.0, 1.0, 0.25)),
        Vec3::new(1.0, 1.0, 0.25),
    );
}

#[test]
fn saturate_color_has_no_lower_bound_so_negatives_pass_through() {
    // The source writes `min(resultColor, 1.0f)`, not `saturate(...)`.
    approx_vec3(
        rsp_process_saturate_color(Vec3::new(-5.0, -0.001, 0.5)),
        Vec3::new(-5.0, -0.001, 0.5),
    );
}

#[test]
fn saturate_color_uses_hlsl_min_semantics_and_propagates_nan() {
    // HLSL min(a,b) == b < a ? b : a; 1.0 < NaN is false, so the NaN first
    // argument survives. Rust's f32::min would have returned 1.0.
    let got = rsp_process_saturate_color(Vec3::new(f32::NAN, 0.5, 2.0));
    assert!(got.x.is_nan(), "expected NaN, got {}", got.x);
    approx(got.y, 0.5);
    approx(got.z, 1.0);
}

#[test]
fn saturate_color_leaves_exactly_one_unchanged() {
    // 1.0 < 1.0 is false, so the component (a) is returned, not the bound.
    approx_vec3(
        rsp_process_saturate_color(Vec3::new(1.0, 1.0, 1.0)),
        Vec3::new(1.0, 1.0, 1.0),
    );
}

#[test]
fn saturate_color_caps_positive_infinity_but_keeps_negative_infinity() {
    let got = rsp_process_saturate_color(Vec3::new(f32::INFINITY, f32::NEG_INFINITY, 0.0));
    approx(got.x, 1.0);
    assert_eq!(got.y, f32::NEG_INFINITY);
}

// =====================================================================
// rsp_process_lighting -- lines 98-119
// =====================================================================

#[test]
fn lighting_with_a_single_light_returns_the_ambient_color_alone() {
    // lightCount == 1 -> ambientIndex == lightIndex -> loop body never runs.
    let lights = [light(
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.25, 0.5, 0.75),
        0,
        0,
        0,
    )];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(1.0, 0.0, 0.0),
            identity(),
        ),
        Vec3::new(0.25, 0.5, 0.75),
    );
}

#[test]
fn lighting_treats_the_last_slice_element_as_ambient_not_the_first() {
    // Two lights: [directional(col=0), ambient(col=(0.1,0.2,0.3))].
    // The directional light has a zero color, so it contributes nothing and
    // the result is exactly the LAST element's colour.
    let lights = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 0.0), 0, 0, 0),
        light(Vec3::default(), Vec3::new(0.1, 0.2, 0.3), 0, 0, 0),
    ];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(0.0, 0.0, 1.0),
            identity(),
        ),
        Vec3::new(0.1, 0.2, 0.3),
    );
}

#[test]
fn lighting_adds_a_directional_contribution_to_the_ambient_seed() {
    // Directional light: posDir = (0,0,1), col = (0.5,0,0), kc = 0.
    // mul_vec_mat with identity gives localLightDir = (0,0,1), length 1.
    // norm = (0,0,1) -> dot = 1 -> max(1,0) = 1 -> contribution (0.5,0,0).
    // Ambient col = (0.25,0.25,0.25). Sum = (0.75,0.25,0.25), under 1.
    let lights = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.5, 0.0, 0.0), 0, 0, 0),
        light(Vec3::default(), Vec3::splat(0.25), 0, 0, 0),
    ];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(0.0, 0.0, 1.0),
            identity(),
        ),
        Vec3::new(0.75, 0.25, 0.25),
    );
}

#[test]
fn lighting_selects_the_directional_path_when_kc_is_zero() {
    // A back-facing normal makes the directional path's max(dot, 0) clamp to
    // zero, so the ambient seed is returned unchanged. The positional path
    // would have produced a different (attenuated) value, so this pins the
    // selector.
    let lights = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), 0, 0, 0),
        light(Vec3::default(), Vec3::new(0.1, 0.1, 0.1), 0, 0, 0),
    ];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(0.0, 0.0, -1.0),
            identity(),
        ),
        Vec3::new(0.1, 0.1, 0.1),
    );
}

#[test]
fn lighting_selects_the_positional_path_when_kc_is_nonzero() {
    // kc = 1 routes to computePosLight, whose attenuation term makes the
    // result differ from the identical light run through the directional
    // path. Both use posDir = (0,0,1) and a norm of (0,0,1).
    let ambient = light(Vec3::default(), Vec3::default(), 0, 0, 0);
    let dir = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), 0, 0, 0),
        ambient,
    ];
    let pos = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), 1, 0, 0),
        ambient,
    ];
    let n = Vec3::new(0.0, 0.0, 1.0);
    let d = rsp_process_lighting(&dir, Vec3::default(), n, identity());
    let p = rsp_process_lighting(&pos, Vec3::default(), n, identity());
    assert!(
        (d.x - p.x).abs() > 1e-3,
        "kc selector should change the result: {} vs {}",
        d.x,
        p.x
    );
}

#[test]
fn lighting_saturates_the_accumulated_sum_at_one() {
    // Three unit-white contributors plus a unit-white ambient sum well past
    // 1.0 and are capped.
    let lights = [
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::splat(1.0), 0, 0, 0),
        light(Vec3::new(0.0, 0.0, 1.0), Vec3::splat(1.0), 0, 0, 0),
        light(Vec3::default(), Vec3::splat(1.0), 0, 0, 0),
    ];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(0.0, 0.0, 1.0),
            identity(),
        ),
        Vec3::splat(1.0),
    );
}

#[test]
fn lighting_accumulates_front_to_back_in_slice_order() {
    // Float addition is not associative. Ambient = 0.5, whose ulp is 2^-24,
    // then two contributions of 2^-25 each. Front-to-back:
    // 0.5 + 2^-25 is an exact tie between 0.5 and 0.5 + 2^-24, so
    // ties-to-even rounds it back to 0.5; the second add ties the same way,
    // so the running total is exactly 0.5.
    // A different order -- pre-summing the two contributions (2^-25 + 2^-25
    // = 2^-24, exact) and adding that to the ambient -- would give
    // 0.5 + 2^-24 = 0.500000059604644775390625. Both are below the
    // min(.., 1.0) cap, so the cap cannot mask the difference.
    let eps = f32::from_bits(0x3300_0000); // 2^-25
    assert_eq!(eps, 2.0f32.powi(-25));
    let contributor = light(Vec3::new(0.0, 0.0, 1.0), Vec3::new(eps, 0.0, 0.0), 0, 0, 0);
    let lights = [
        contributor,
        contributor,
        light(Vec3::default(), Vec3::new(0.5, 0.0, 0.0), 0, 0, 0),
    ];
    let got = rsp_process_lighting(
        &lights,
        Vec3::default(),
        Vec3::new(0.0, 0.0, 1.0),
        identity(),
    );
    assert_eq!(
        got.x, 0.5,
        "front-to-back accumulation should round both adds away"
    );
    assert_ne!(
        got.x,
        0.5 + 2.0f32.powi(-24),
        "contributions must not be pre-summed"
    );
}

#[test]
fn lighting_negative_ambient_is_not_clamped_up_to_zero() {
    let lights = [light(Vec3::default(), Vec3::new(-0.5, -1.0, 0.0), 0, 0, 0)];
    approx_vec3(
        rsp_process_lighting(
            &lights,
            Vec3::default(),
            Vec3::new(0.0, 0.0, 1.0),
            identity(),
        ),
        Vec3::new(-0.5, -1.0, 0.0),
    );
}

#[test]
#[should_panic(expected = "lightCount > 0")]
fn lighting_rejects_an_empty_slice() {
    let _ = rsp_process_lighting(&[], Vec3::default(), Vec3::default(), identity());
}

// =====================================================================
// rsp_process_near_clip_w -- lines 123-125
// =====================================================================

#[test]
fn near_clip_replaces_positive_zero_w_with_one_micro() {
    assert_eq!(rsp_process_near_clip_w(0.0), 1e-6);
}

#[test]
fn near_clip_also_fires_for_negative_zero_and_yields_a_positive_result() {
    // IEEE-754: -0.0 == 0.0 is true, so the guard fires and the sign flips.
    let got = rsp_process_near_clip_w(-0.0);
    assert_eq!(got, 1e-6);
    assert!(got.is_sign_positive(), "expected +1e-6, got {got}");
}

#[test]
fn near_clip_leaves_a_tiny_but_nonzero_w_alone() {
    // 1e-30 != 0.0, so no substitution -- the guard is exact equality, not a
    // magnitude threshold.
    assert_eq!(rsp_process_near_clip_w(1e-30), 1e-30);
    assert_eq!(rsp_process_near_clip_w(-1e-30), -1e-30);
}

#[test]
fn near_clip_does_not_fire_for_nan_w() {
    // NaN == 0.0 is false.
    assert!(rsp_process_near_clip_w(f32::NAN).is_nan());
}

#[test]
fn near_clip_leaves_ordinary_w_untouched() {
    assert_eq!(rsp_process_near_clip_w(1.0), 1.0);
    assert_eq!(rsp_process_near_clip_w(-3.5), -3.5);
}

// =====================================================================
// rsp_process_ndc -- line 129
// =====================================================================

#[test]
fn ndc_divides_x_and_z_by_w_and_y_by_negative_w() {
    let got = rsp_process_ndc(Vec4::new(2.0, 4.0, 6.0, 2.0), 2.0);
    approx_vec3(got, Vec3::new(1.0, -2.0, 3.0));
}

#[test]
fn ndc_negates_only_y_not_x_or_z() {
    let got = rsp_process_ndc(Vec4::new(1.0, 1.0, 1.0, 1.0), 1.0);
    approx(got.x, 1.0);
    approx(got.y, -1.0);
    approx(got.z, 1.0);
}

#[test]
fn ndc_y_negation_is_applied_to_the_divisor_producing_a_signed_zero() {
    // y = +0.0, w = +1.0: +0.0 / -1.0 = -0.0.
    let got = rsp_process_ndc(Vec4::new(0.0, 0.0, 0.0, 1.0), 1.0);
    assert!(
        got.y.is_sign_negative(),
        "y should be -0.0, got {} (sign positive)",
        got.y
    );
    assert!(got.x.is_sign_positive(), "x should stay +0.0");
}

#[test]
fn ndc_with_a_negative_w_flips_x_and_z_and_restores_y() {
    let got = rsp_process_ndc(Vec4::new(2.0, 4.0, 6.0, -2.0), -2.0);
    approx_vec3(got, Vec3::new(-1.0, 2.0, -3.0));
}

#[test]
fn ndc_uses_the_supplied_w_not_the_vectors_own_w_component() {
    // tf_pos.w = 2.0 but the (post-HACK) divisor argument is 4.0; the
    // divisor argument must win.
    let got = rsp_process_ndc(Vec4::new(8.0, 8.0, 8.0, 2.0), 4.0);
    approx(got.x, 2.0);
    approx(got.y, -2.0);
    approx(got.z, 2.0);
}

#[test]
fn ndc_with_a_nan_w_yields_nan_in_all_three_components() {
    let got = rsp_process_ndc(Vec4::new(1.0, 2.0, 3.0, f32::NAN), f32::NAN);
    assert!(got.x.is_nan() && got.y.is_nan() && got.z.is_nan());
}

#[test]
fn ndc_after_the_near_clip_hack_produces_a_large_finite_magnitude() {
    // w = 0 -> HACK -> 1e-6; 1.0 / 1e-6 = 1e6.
    let w = rsp_process_near_clip_w(0.0);
    let got = rsp_process_ndc(Vec4::new(1.0, 1.0, 1.0, 0.0), w);
    assert!((got.x - 1e6).abs() < 1.0, "{}", got.x);
    assert!((got.y + 1e6).abs() < 1.0, "{}", got.y);
}

// =====================================================================
// rsp_process_screen_pos -- line 130
// =====================================================================

fn viewport(scale: Vec3, translate: Vec3) -> RspViewport {
    RspViewport { scale, translate }
}

#[test]
fn screen_pos_scales_then_translates_component_wise() {
    // (2,3,4) * (10,100,1000) + (1,2,3) = (21, 302, 4003).
    let got = rsp_process_screen_pos(
        Vec3::new(2.0, 3.0, 4.0),
        viewport(Vec3::new(10.0, 100.0, 1000.0), Vec3::new(1.0, 2.0, 3.0)),
        7.0,
    );
    approx_vec4(got, Vec4::new(21.0, 302.0, 4003.0, 7.0));
}

#[test]
fn screen_pos_carries_tf_pos_w_into_the_fourth_component_not_one() {
    let got = rsp_process_screen_pos(Vec3::default(), RspViewport::identity(), 42.0);
    approx(got.w, 42.0);
    assert_ne!(got.w, 1.0);
}

#[test]
fn screen_pos_with_the_identity_viewport_is_the_ndc_position() {
    let got = rsp_process_screen_pos(Vec3::new(-0.5, 0.25, 0.75), RspViewport::identity(), 1.0);
    approx_vec4(got, Vec4::new(-0.5, 0.25, 0.75, 1.0));
}

#[test]
fn screen_pos_translate_is_added_not_multiplied() {
    // With a zero scale the result is exactly the translate.
    let got = rsp_process_screen_pos(
        Vec3::new(999.0, 999.0, 999.0),
        viewport(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 6.0, 7.0)),
        1.0,
    );
    approx_vec4(got, Vec4::new(5.0, 6.0, 7.0, 1.0));
}

// =====================================================================
// rsp_process_vertex -- the composed body, lines 57-130
// =====================================================================

#[test]
fn vertex_with_no_fog_and_no_lights_passes_the_byte_colour_straight_through() {
    let mut v = zero_vertex();
    v.col = [255, 128, 0, 64];
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    approx(got.color.x, 1.0);
    approx(got.color.y, 128.0 / 255.0);
    approx(got.color.z, 0.0);
    approx(got.color.w, 64.0 / 255.0);
}

#[test]
fn vertex_with_identity_transforms_maps_position_to_itself() {
    let mut v = zero_vertex();
    v.pos = Vec3::new(1.0, 2.0, 3.0);
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    // w = 1; ndc = (1, -2, 3); identity viewport leaves it alone.
    approx_vec4(got.screen_pos, Vec4::new(1.0, -2.0, 3.0, 1.0));
}

#[test]
fn vertex_without_texgen_applies_the_tc_velocity_branch() {
    let mut v = zero_vertex();
    v.tc = [10.0, 20.0];
    v.tc_vel = [4.0, 8.0];
    v.look_at_index = 0; // ENABLED bit clear.
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        0.5,
    );
    // 1 - 0.5 = 0.5; 10 - 4*0.5 = 8; 20 - 8*0.5 = 16.
    approx(got.tc[0], 8.0);
    approx(got.tc[1], 16.0);
}

#[test]
fn vertex_with_texgen_enabled_ignores_tc_velocity_entirely() {
    // ENABLED set, LINEAR clear, extracted index 0.
    let mut v = zero_vertex();
    v.tc = [65536.0, 65536.0];
    v.tc_vel = [1e9, 1e9]; // Would dominate if the else branch ran.
    v.norm = [127, 0, 0]; // norm = (1, 0, 0).
    v.look_at_index = RSP_LOOKAT_INDEX_ENABLED;
    let look_at = RspLookAt {
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::new(0.0, 1.0, 0.0),
    };
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[look_at],
        None,
        RspViewport::identity(),
        0.0,
    );
    // Non-linear texgen: dot(norm=(1,0,0), axisX=(1,0,0)) = 1, clamp = 1,
    // (1+1)*512 = 1024. dot with axisY=(0,1,0) = 0, (0+1)*512 = 512.
    // Then (inputUV / 65536) * texgenUV = 1 * 1024 and 1 * 512.
    approx(got.tc[0], 1024.0);
    approx(got.tc[1], 512.0);
}

#[test]
fn vertex_fog_branch_replaces_only_alpha_and_leaves_rgb_to_the_colour_path() {
    let mut v = zero_vertex();
    v.pos = Vec3::new(0.0, 0.0, 1.0);
    v.col = [255, 255, 255, 0]; // Alpha 0 would win if fog were skipped.
    v.fog_index = 1; // One-based -> table entry 0.
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[RspFog {
            mul: 255.0,
            offset: 0.0,
        }],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    // tfPos = (0,0,1,1); max(z,0)/w = 1; *255 + 0 = 255; /255 = 1.
    approx(got.color.w, 1.0);
    // RGB is still the unlit byte fallback, not touched by fog.
    approx(got.color.x, 1.0);
    approx(got.color.y, 1.0);
}

#[test]
fn vertex_fog_reads_the_un_patched_w_before_the_near_clip_hack_runs() {
    // Construct a vertex whose tfPos.w is exactly 0 by using a world matrix
    // with a zero bottom row, then pick a fog `mul` small enough that the
    // two candidate `w` values land on OPPOSITE sides of the clamp:
    //
    //   un-patched w = 0  -> max(1,0)/0   = +Inf -> *1e-4 = +Inf
    //                     -> /255 = +Inf  -> clamp -> 1.0
    //   patched   w = 1e-6 -> max(1,0)/1e-6 = 1e6 -> *1e-4 = 100
    //                     -> /255 = 0.39215686... -> clamp is the identity
    //
    // So alpha == 1.0 proves the fog block ran BEFORE the HACK, and alpha
    // == 0.392 would prove the opposite. Any `mul` under 255e-6 works; 1e-4
    // leaves comfortable margin on both sides.
    let zero_w = Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
    ]);
    let mut v = zero_vertex();
    v.pos = Vec3::new(0.0, 0.0, 1.0);
    v.fog_index = 1;
    let got = rsp_process_vertex(
        v,
        identity(),
        zero_w,
        &[RspFog {
            mul: 1e-4,
            offset: 0.0,
        }],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    approx(got.color.w, 1.0);
    assert!(
        (got.color.w - 100.0 / 255.0).abs() > 1e-3,
        "alpha {} matches the POST-hack reading; the fog block must run first",
        got.color.w
    );
    // Screen pos: patched w = 1e-6, so the emitted w is 1e-6, NOT 0.0 --
    // the HACK did run, just later.
    approx(got.screen_pos.w, 1e-6);
}

#[test]
fn vertex_fog_index_zero_uses_the_byte_alpha_and_never_indexes_the_table() {
    let mut v = zero_vertex();
    v.col = [0, 0, 0, 51];
    v.fog_index = 0;
    // An empty fog table would panic if the fog branch were taken.
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    approx(got.color.w, 51.0 / 255.0);
}

#[test]
fn vertex_lighting_branch_replaces_rgb_and_leaves_alpha_to_the_colour_path() {
    let mut v = zero_vertex();
    v.norm = [0, 0, 127]; // norm = (0, 0, 1).
    v.col = [255, 255, 255, 128]; // RGB would be white if unlit.
    let lights = [light(Vec3::default(), Vec3::new(0.2, 0.4, 0.6), 0, 0, 0)];
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        Some(&lights),
        RspViewport::identity(),
        1.0,
    );
    approx(got.color.x, 0.2);
    approx(got.color.y, 0.4);
    approx(got.color.z, 0.6);
    // Alpha still the byte fallback.
    approx(got.color.w, 128.0 / 255.0);
}

#[test]
fn vertex_applies_the_viewport_scale_and_translate_to_the_ndc_position() {
    let mut v = zero_vertex();
    v.pos = Vec3::new(0.5, 0.5, 0.5);
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        viewport(
            Vec3::new(160.0, 120.0, 511.0),
            Vec3::new(160.0, 120.0, 511.0),
        ),
        1.0,
    );
    // ndc = (0.5, -0.5, 0.5).
    // x = 0.5*160 + 160 = 240; y = -0.5*120 + 120 = 60; z = 0.5*511 + 511.
    approx(got.screen_pos.x, 240.0);
    approx(got.screen_pos.y, 60.0);
    approx(got.screen_pos.z, 0.5 * 511.0 + 511.0);
    approx(got.screen_pos.w, 1.0);
}

#[test]
fn vertex_uses_the_normal_divided_by_127_in_the_texgen_dot_product() {
    // norm = -128/127 = -1.00787..., whose dot with axisX = (1,0,0) is
    // -1.00787. computeTextureGen clamps that to -1 before the mode split,
    // so non-linear gives (-1 + 1) * 512 = 0.
    let mut v = zero_vertex();
    v.tc = [65536.0, 65536.0];
    v.norm = [-128, 0, 0];
    v.look_at_index = RSP_LOOKAT_INDEX_ENABLED;
    let look_at = RspLookAt {
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::new(0.0, 1.0, 0.0),
    };
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[look_at],
        None,
        RspViewport::identity(),
        0.0,
    );
    approx(got.tc[0], 0.0);
    approx(got.tc[1], 512.0);
}

#[test]
fn vertex_texgen_linear_flag_selects_the_acos_path() {
    // LINEAR set: acos(-clamp(dot)) * 325.94932. dot = 0 -> acos(0) =
    // pi/2 = 1.5707963; * 325.94932 = 512.0000... (325.94932 = 1024/pi).
    let mut v = zero_vertex();
    v.tc = [65536.0, 65536.0];
    v.norm = [0, 0, 127]; // norm = (0,0,1), orthogonal to both look-at axes.
    v.look_at_index = RSP_LOOKAT_INDEX_ENABLED | RSP_LOOKAT_INDEX_LINEAR;
    let look_at = RspLookAt {
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::new(0.0, 1.0, 0.0),
    };
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[look_at],
        None,
        RspViewport::identity(),
        0.0,
    );
    approx(got.tc[0], 512.0);
    approx(got.tc[1], 512.0);
}

#[test]
fn vertex_texgen_extracted_index_selects_the_right_look_at_entry() {
    // look_at_index = (1 << 2) | ENABLED = 5 -> extracted index 1.
    let mut v = zero_vertex();
    v.tc = [65536.0, 65536.0];
    v.norm = [127, 0, 0];
    v.look_at_index = (1 << RSP_LOOKAT_INDEX_SHIFT) | RSP_LOOKAT_INDEX_ENABLED;
    let entry0 = RspLookAt {
        x: Vec3::new(0.0, 1.0, 0.0),
        y: Vec3::new(0.0, 0.0, 1.0),
    };
    let entry1 = RspLookAt {
        x: Vec3::new(1.0, 0.0, 0.0),
        y: Vec3::new(0.0, 1.0, 0.0),
    };
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[entry0, entry1],
        None,
        RspViewport::identity(),
        0.0,
    );
    // Entry 1's x axis is (1,0,0); dot with norm (1,0,0) is 1 -> 1024.
    // Entry 0's x axis is (0,1,0), which would have given (0+1)*512 = 512.
    approx(got.tc[0], 1024.0);
}

#[test]
fn vertex_velocity_subtraction_moves_the_transformed_position() {
    let mut v = zero_vertex();
    v.pos = Vec3::new(10.0, 0.0, 0.0);
    v.vel = Vec3::new(4.0, 0.0, 0.0);
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        0.25,
    );
    // 1 - 0.25 = 0.75; 10 - 4*0.75 = 7.
    approx(got.screen_pos.x, 7.0);
}

#[test]
fn vertex_composes_view_proj_after_world_end_to_end() {
    // world = translate(10,0,0), viewProj = rot_z90; pos (0,0,0).
    // world -> (10,0,0), rot -> (0,10,0); w = 1.
    // ndc = (0, -10, 0) because y divides by -w.
    let got = rsp_process_vertex(
        zero_vertex(),
        rot_z90(),
        translate_x10(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    approx(got.screen_pos.x, 0.0);
    approx(got.screen_pos.y, -10.0);
}

#[test]
fn vertex_output_tc_and_colour_and_position_are_three_independent_fields() {
    // Set all three to distinguishable values in one call.
    let mut v = zero_vertex();
    v.pos = Vec3::new(1.0, 2.0, 3.0);
    v.tc = [7.0, 8.0];
    v.col = [0, 255, 0, 255];
    let got = rsp_process_vertex(
        v,
        identity(),
        identity(),
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    approx_vec4(got.screen_pos, Vec4::new(1.0, -2.0, 3.0, 1.0));
    approx(got.tc[0], 7.0);
    approx(got.tc[1], 8.0);
    approx_vec4(got.color, Vec4::new(0.0, 1.0, 0.0, 1.0));
}

#[test]
fn vertex_lighting_uses_the_pre_transform_pos_not_the_ndc_pos() {
    // computePosLight takes the RAW `pos` (line 104), not tfPos. Give the
    // world matrix a translation and a positional light sited at the raw
    // pos's world image; if the ported call passed tfPos instead, the light
    // distance -- and hence the attenuation -- would differ.
    let mut v = zero_vertex();
    v.pos = Vec3::new(1.0, 0.0, 0.0);
    v.norm = [127, 0, 0];
    let lights = [
        light(Vec3::new(11.0, 0.0, 0.0), Vec3::splat(1.0), 1, 0, 0),
        light(Vec3::default(), Vec3::default(), 0, 0, 0),
    ];
    let got_world_translated = rsp_process_vertex(
        v,
        identity(),
        translate_x10(),
        &[],
        &[],
        Some(&lights),
        RspViewport::identity(),
        1.0,
    );
    // worldVertexPos = (11,0,0); light at (11,0,0) -> dir (0,0,0), dist 0,
    // so the `> 0` guard leaves dir at (0,0,0) and NdotL = clamp(0*4) = 0.
    approx_vec3(
        Vec3::new(
            got_world_translated.color.x,
            got_world_translated.color.y,
            got_world_translated.color.z,
        ),
        Vec3::default(),
    );
}

#[test]
fn vertex_near_clip_hack_and_ndc_compose_for_a_zero_w_vertex() {
    let zero_w = Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
    ]);
    let mut v = zero_vertex();
    v.pos = Vec3::new(1.0, 0.0, 0.0);
    let got = rsp_process_vertex(
        v,
        identity(),
        zero_w,
        &[],
        &[],
        None,
        RspViewport::identity(),
        1.0,
    );
    // w -> 1e-6; x = 1 / 1e-6 = 1e6.
    assert!((got.screen_pos.x - 1e6).abs() < 1.0, "{}", got.screen_pos.x);
    approx(got.screen_pos.w, 1e-6);
}
