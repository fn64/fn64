use super::*;

// --- Independent CPU oracle -------------------------------------------------
//
// A second, independently-derived re-expression of `VSMain`, written from
// the per-vertex table in `fullscreen_vs`'s doc comment (not by copying
// `fullscreen_vs`'s ternary-and-multiply control flow): a `match` over `id`
// selecting the UV corner directly, and the position computed via an
// explicit two-step "scale, then offset" rather than one fused expression.
// This gives the tests below a genuine second derivation to compare against
// `fullscreen_vs`, rather than comparing an implementation to itself.
fn oracle_fullscreen_vertex(id: u32) -> FullScreenVertex {
    let (u, v) = match id {
        1 => (0.0f32, 2.0f32),
        2 => (2.0f32, 0.0f32),
        _ => (0.0f32, 0.0f32),
    };

    let scaled_x = u * 2.0;
    let scaled_y = v * -2.0;
    let x = scaled_x + -1.0;
    let y = scaled_y + 1.0;

    FullScreenVertex {
        position: [x, y, 1.0, 1.0],
        uv: [u, v],
    }
}

// --- Per-vertex-index characterization (hand-computed) ----------------------

#[test]
fn id_0_uv_is_origin() {
    assert_eq!(fullscreen_vs(0).uv, [0.0, 0.0]);
}

#[test]
fn id_0_position_is_top_left_of_ndc_square() {
    assert_eq!(fullscreen_vs(0).position, [-1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn id_1_uv_is_zero_two() {
    assert_eq!(fullscreen_vs(1).uv, [0.0, 2.0]);
}

#[test]
fn id_1_position_is_bottom_overshoot() {
    // uv=(0,2) -> pos.x = 0*2 + -1 = -1; pos.y = 2*-2 + 1 = -3.
    assert_eq!(fullscreen_vs(1).position, [-1.0, -3.0, 1.0, 1.0]);
}

#[test]
fn id_2_uv_is_two_zero() {
    assert_eq!(fullscreen_vs(2).uv, [2.0, 0.0]);
}

#[test]
fn id_2_position_is_right_overshoot() {
    // uv=(2,0) -> pos.x = 2*2 + -1 = 3; pos.y = 0*-2 + 1 = 1.
    assert_eq!(fullscreen_vs(2).position, [3.0, 1.0, 1.0, 1.0]);
}

#[test]
fn id_0_position_z_is_one() {
    assert_eq!(fullscreen_vs(0).position[2], 1.0);
}

#[test]
fn id_1_position_z_is_one() {
    assert_eq!(fullscreen_vs(1).position[2], 1.0);
}

#[test]
fn id_2_position_z_is_one() {
    assert_eq!(fullscreen_vs(2).position[2], 1.0);
}

#[test]
fn id_0_position_w_is_one() {
    assert_eq!(fullscreen_vs(0).position[3], 1.0);
}

#[test]
fn id_1_position_w_is_one() {
    assert_eq!(fullscreen_vs(1).position[3], 1.0);
}

#[test]
fn id_2_position_w_is_one() {
    assert_eq!(fullscreen_vs(2).position[3], 1.0);
}

// --- Out-of-domain indices --------------------------------------------------
//
// RT64 always dispatches this as a 3-vertex draw (module doc "Admitted
// domain"), so ids other than 0/1/2 are out of the shader's admitted domain.
// The HLSL's ternaries are still well-defined for any `uint`, though: both
// comparisons are false, so the result equals the `id == 0` case. This port
// preserves that literal fallthrough behavior rather than asserting a
// narrower domain the source code does not itself enforce.
#[test]
fn id_3_falls_through_to_the_id_0_result() {
    assert_eq!(fullscreen_vs(3), fullscreen_vs(0));
}

#[test]
fn id_u32_max_falls_through_to_the_id_0_result() {
    assert_eq!(fullscreen_vs(u32::MAX), fullscreen_vs(0));
}

// --- Structural facts about the three admitted vertices ---------------------

#[test]
fn the_three_vertices_are_pairwise_distinct() {
    let a = fullscreen_vs(0).position;
    let b = fullscreen_vs(1).position;
    let c = fullscreen_vs(2).position;
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
}

#[test]
fn the_triangle_covers_the_full_ndc_square_with_overshoot() {
    // x in {-1, -1, 3}: spans from the left edge to beyond the right edge.
    // y in {1, -3, 1}: spans from the top edge to beyond the bottom edge.
    let xs: Vec<f32> = [0u32, 1, 2]
        .iter()
        .map(|&id| fullscreen_vs(id).position[0])
        .collect();
    let ys: Vec<f32> = [0u32, 1, 2]
        .iter()
        .map(|&id| fullscreen_vs(id).position[1])
        .collect();
    assert!(xs.iter().cloned().fold(f32::INFINITY, f32::min) <= -1.0);
    assert!(xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max) >= 1.0);
    assert!(ys.iter().cloned().fold(f32::INFINITY, f32::min) <= -1.0);
    assert!(ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max) >= 1.0);
}

#[test]
fn triangle_signed_area_is_positive_counter_clockwise_in_plus_y_up_frame() {
    let p0 = fullscreen_vs(0).position;
    let p1 = fullscreen_vs(1).position;
    let p2 = fullscreen_vs(2).position;
    // Shoelace formula: x0*(y1-y2) + x1*(y2-y0) + x2*(y0-y1).
    let signed_area_x2 =
        p0[0] * (p1[1] - p2[1]) + p1[0] * (p2[1] - p0[1]) + p2[0] * (p0[1] - p1[1]);
    assert_eq!(signed_area_x2, 16.0);
    assert!(signed_area_x2 > 0.0);
}

// --- Differential: independent CPU oracle vs. the port ----------------------

#[test]
fn oracle_agrees_with_port_at_id_0() {
    assert_eq!(oracle_fullscreen_vertex(0), fullscreen_vs(0));
}

#[test]
fn oracle_agrees_with_port_at_id_1() {
    assert_eq!(oracle_fullscreen_vertex(1), fullscreen_vs(1));
}

#[test]
fn oracle_agrees_with_port_at_id_2() {
    assert_eq!(oracle_fullscreen_vertex(2), fullscreen_vs(2));
}

#[test]
fn oracle_agrees_with_port_across_the_full_admitted_and_fallthrough_domain() {
    for id in [0u32, 1, 2, 3, 4, 100, u32::MAX] {
        assert_eq!(
            oracle_fullscreen_vertex(id),
            fullscreen_vs(id),
            "mismatch at id={id}"
        );
    }
}

// --- WGSL structural checks --------------------------------------------------

#[test]
fn wgsl_entry_point_name_matches_constant() {
    assert!(FULLSCREEN_VS_WGSL.contains(&format!("fn {FULLSCREEN_VS_ENTRY_POINT}(")));
}

#[test]
fn wgsl_source_contains_the_exact_selection_and_transform() {
    assert!(FULLSCREEN_VS_WGSL.contains("select(0.0, 2.0, id == 2u)"));
    assert!(FULLSCREEN_VS_WGSL.contains("select(0.0, 2.0, id == 1u)"));
    assert!(FULLSCREEN_VS_WGSL.contains("vec2<f32>(2.0, -2.0)"));
    assert!(FULLSCREEN_VS_WGSL.contains("vec2<f32>(-1.0, 1.0)"));
}

#[test]
fn wgsl_uses_builtin_position_and_vertex_index() {
    assert!(FULLSCREEN_VS_WGSL.contains("@builtin(position)"));
    assert!(FULLSCREEN_VS_WGSL.contains("@builtin(vertex_index)"));
}

#[test]
fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(FULLSCREEN_VS_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}

#[test]
fn malformed_wgsl_fails_to_parse() {
    // Drop the closing brace and everything after it, leaving an unclosed
    // function body -- guaranteed invalid regardless of where in the file
    // the cut lands, unlike a length/2 truncation which can land on a
    // syntactically-complete prefix for a file this short.
    let truncated = FULLSCREEN_VS_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn duplicate_location_index_fails_naga_validation() {
    let duplicate_location = FULLSCREEN_VS_WGSL.replacen(
        "@builtin(position) position: vec4<f32>,",
        "@location(0) position: vec4<f32>,",
        1,
    );
    let module = naga::front::wgsl::parse_str(&duplicate_location).unwrap();
    assert!(naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .is_err());
}

#[test]
fn wgsl_oracle_agrees_with_rust_across_the_full_id_domain() {
    // Differential (structural/textual, not GPU-executed -- matching
    // raster_vs.rs's identically-scoped precedent and this crate's lack of a
    // vertex-shader-dispatch test harness): independently re-evaluate the
    // WGSL's exact textual formula in Rust and confirm it agrees with the
    // `fullscreen_vs` CPU oracle across the full admitted-and-fallthrough
    // `id` domain.
    fn wgsl_formula(id: u32) -> ([f32; 4], [f32; 2]) {
        let u = if id == 2 { 2.0f32 } else { 0.0 };
        let v = if id == 1 { 2.0f32 } else { 0.0 };
        let x = u * 2.0 + (-1.0);
        let y = v * (-2.0) + 1.0;
        ([x, y, 1.0, 1.0], [u, v])
    }

    for id in [0u32, 1, 2, 3, 42, u32::MAX] {
        let expected = fullscreen_vs(id);
        let (position, uv) = wgsl_formula(id);
        assert_eq!(position, expected.position, "position mismatch at id={id}");
        assert_eq!(uv, expected.uv, "uv mismatch at id={id}");
    }
}

// --- Float exactness (no epsilon needed; see module doc) --------------------

#[test]
fn all_output_components_are_exactly_representable_literals() {
    for id in [0u32, 1, 2] {
        let vertex = fullscreen_vs(id);
        for component in vertex.position {
            assert!(
                component == component.trunc()
                    || component == 1.0
                    || component == -3.0
                    || component == 3.0
            );
        }
    }
}

#[test]
fn position_recomputation_is_bit_exact_across_repeated_calls() {
    for id in [0u32, 1, 2] {
        let first = fullscreen_vs(id);
        let second = fullscreen_vs(id);
        assert_eq!(first, second);
        assert_eq!(first.position[0].to_bits(), second.position[0].to_bits());
        assert_eq!(first.position[1].to_bits(), second.position[1].to_bits());
    }
}
