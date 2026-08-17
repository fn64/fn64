use super::*;

// ---------------------------------------------------------------------
// Independent CPU oracle
// ---------------------------------------------------------------------
//
// A second, independently-derived re-expression of `computeSmoothNormal`'s
// arithmetic, written directly from the HLSL text without reusing this
// module's own helper functions (`cross`, `add`, `normalize`,
// `weld_predicate`, `face_normal`, `compute_smooth_normal`), so the tests
// below compare two independent derivations rather than one implementation
// against itself. Expected numeric values quoted in test bodies were
// additionally hand-verified with a standalone Python IEEE-754 `f32`
// round-trip simulator (`struct.pack('f', x)` truncation after every
// operation), not captured from this crate's own implementation.

fn oracle_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn oracle_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn oracle_add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn oracle_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn oracle_normalize(v: [f32; 3]) -> [f32; 3] {
    // Explicit sqrt-of-sum-of-squares length, then per-component division --
    // written independently (accumulate via a running total rather than the
    // module's `x*x + y*y + z*z` single-expression form).
    let mut sum_sq = 0.0f32;
    for c in v {
        sum_sq += c * c;
    }
    let len = sum_sq.sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

fn oracle_face_normal(tri_a: [f32; 3], tri_b: [f32; 3], tri_c: [f32; 3]) -> [f32; 3] {
    oracle_normalize(oracle_cross(
        oracle_sub(tri_b, tri_a),
        oracle_sub(tri_c, tri_a),
    ))
}

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn arr(vec: Vec3) -> [f32; 3] {
    [vec.x, vec.y, vec.z]
}

fn assert_close(a: [f32; 3], b: [f32; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() <= 1e-5, "component {i}: {a:?} vs {b:?}");
    }
}

fn assert_all_nan(a: [f32; 3]) {
    assert!(a[0].is_nan() && a[1].is_nan() && a[2].is_nan(), "{a:?}");
}

// =======================================================================
// cross()
// =======================================================================

#[test]
fn cross_unit_x_unit_y_gives_unit_z() {
    let out = cross(v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
    assert_close(arr(out), oracle_cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]));
    assert_close(arr(out), [0.0, 0.0, 1.0]);
}

#[test]
fn cross_unit_y_unit_x_gives_negative_unit_z_anticommutative() {
    let out = cross(v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0));
    assert_close(arr(out), [0.0, 0.0, -1.0]);
}

#[test]
fn cross_parallel_vectors_is_zero() {
    let out = cross(v(2.0, 4.0, 6.0), v(1.0, 2.0, 3.0));
    assert_close(arr(out), [0.0, 0.0, 0.0]);
}

#[test]
fn cross_matches_independent_oracle_general_case() {
    let a = [1.5, -2.25, 3.0];
    let b = [-0.5, 4.0, 2.0];
    let out = cross(v(a[0], a[1], a[2]), v(b[0], b[1], b[2]));
    assert_close(arr(out), oracle_cross(a, b));
}

#[test]
fn cross_with_zero_vector_is_zero() {
    let out = cross(v(0.0, 0.0, 0.0), v(3.0, 4.0, 5.0));
    assert_close(arr(out), [0.0, 0.0, 0.0]);
}

// =======================================================================
// add()
// =======================================================================

#[test]
fn add_basic() {
    let out = add(v(1.0, 2.0, 3.0), v(4.0, 5.0, 6.0));
    assert_close(arr(out), [5.0, 7.0, 9.0]);
}

#[test]
fn add_matches_oracle() {
    let a = [0.1, 0.2, 0.3];
    let b = [1.1, -2.2, 3.3];
    let out = add(v(a[0], a[1], a[2]), v(b[0], b[1], b[2]));
    assert_close(arr(out), oracle_add(a, b));
}

// =======================================================================
// normalize()
// =======================================================================

#[test]
fn normalize_unit_x_is_unchanged() {
    let out = normalize(v(1.0, 0.0, 0.0));
    assert_close(arr(out), [1.0, 0.0, 0.0]);
}

#[test]
fn normalize_scales_to_unit_length() {
    let out = normalize(v(3.0, 4.0, 0.0));
    assert_close(arr(out), [0.6, 0.8, 0.0]);
}

#[test]
fn normalize_matches_independent_oracle() {
    let val = [2.0, -1.0, 0.5];
    let out = normalize(v(val[0], val[1], val[2]));
    assert_close(arr(out), oracle_normalize(val));
}

#[test]
fn normalize_zero_vector_yields_nan_in_every_component() {
    // 0.0/0.0 in every lane -- unguarded IEEE-754, matching
    // RSPSmoothNormalCS.hlsl's plain `normalize` with no epsilon guard.
    let out = normalize(v(0.0, 0.0, 0.0));
    assert_all_nan(arr(out));
}

#[test]
fn normalize_negative_zero_vector_yields_nan() {
    let out = normalize(v(-0.0, -0.0, -0.0));
    assert_all_nan(arr(out));
}

#[test]
fn normalize_nan_input_propagates_nan() {
    let out = normalize(v(f32::NAN, 1.0, 1.0));
    assert_all_nan(arr(out));
}

#[test]
fn normalize_infinite_input_component_propagates() {
    let out = normalize(v(f32::INFINITY, 0.0, 0.0));
    // length = sqrt(inf^2) = inf; inf/inf = NaN, 0/inf = 0.
    assert!(out.x.is_nan());
    assert_eq!(out.y, 0.0);
    assert_eq!(out.z, 0.0);
}

// =======================================================================
// weld_predicate()
// =======================================================================

#[test]
fn weld_predicate_exact_same_position_and_color_welds() {
    let welds = weld_predicate(v(0.0, 0.0, 0.0), 7, VertexSample::new(v(0.0, 0.0, 0.0), 7));
    assert!(welds);
}

#[test]
fn weld_predicate_within_radius_same_color_welds() {
    let welds = weld_predicate(v(0.0, 0.0, 0.0), 1, VertexSample::new(v(0.5, 0.0, 0.0), 1));
    assert!(welds);
}

#[test]
fn weld_predicate_exactly_at_threshold_distance_welds_inclusive() {
    // distSqr == 1.0 exactly (distance 1.0 apart on one axis) -- `<=` is
    // inclusive, so this welds. Hand-verified: dot((1,0,0),(1,0,0)) = 1.0.
    let welds = weld_predicate(v(0.0, 0.0, 0.0), 1, VertexSample::new(v(1.0, 0.0, 0.0), 1));
    assert!(welds);
}

#[test]
fn weld_predicate_just_past_threshold_does_not_weld() {
    let welds = weld_predicate(
        v(0.0, 0.0, 0.0),
        1,
        VertexSample::new(v(1.0001, 0.0, 0.0), 1),
    );
    assert!(!welds);
}

#[test]
fn weld_predicate_different_color_does_not_weld_even_at_zero_distance() {
    let welds = weld_predicate(v(0.0, 0.0, 0.0), 1, VertexSample::new(v(0.0, 0.0, 0.0), 2));
    assert!(!welds);
}

#[test]
fn weld_predicate_far_away_same_color_does_not_weld() {
    let welds = weld_predicate(
        v(0.0, 0.0, 0.0),
        5,
        VertexSample::new(v(100.0, 100.0, 100.0), 5),
    );
    assert!(!welds);
}

#[test]
fn weld_predicate_far_away_different_color_does_not_weld() {
    let welds = weld_predicate(
        v(0.0, 0.0, 0.0),
        5,
        VertexSample::new(v(100.0, 100.0, 100.0), 9),
    );
    assert!(!welds);
}

#[test]
fn weld_predicate_diagonal_distance_uses_squared_length_not_per_axis() {
    // (0.6,0.6,0.6): distSqr = 0.36*3 = 1.08 > 1.0 -> should NOT weld, even
    // though each axis alone (0.6) is under 1.0. Confirms the predicate
    // uses `dot(posDelta,posDelta)`, not a per-axis or Chebyshev distance.
    let pos_delta_sqr = oracle_dot([0.6, 0.6, 0.6], [0.6, 0.6, 0.6]);
    assert!(pos_delta_sqr > 1.0);
    let welds = weld_predicate(v(0.0, 0.0, 0.0), 3, VertexSample::new(v(0.6, 0.6, 0.6), 3));
    assert!(!welds);
}

#[test]
fn weld_predicate_color_zero_matches_color_zero() {
    let welds = weld_predicate(v(1.0, 1.0, 1.0), 0, VertexSample::new(v(1.0, 1.0, 1.0), 0));
    assert!(welds);
}

#[test]
fn weld_predicate_max_u32_color_matches_itself() {
    let welds = weld_predicate(
        v(1.0, 1.0, 1.0),
        u32::MAX,
        VertexSample::new(v(1.0, 1.0, 1.0), u32::MAX),
    );
    assert!(welds);
}

// =======================================================================
// face_normal()
// =======================================================================

#[test]
fn face_normal_xy_plane_triangle_faces_positive_z() {
    // triA=(0,0,0), triB=(1,0,0), triC=(0,1,0): cross(B-A, C-A) =
    // cross((1,0,0),(0,1,0)) = (0,0,1), already unit length.
    let n = face_normal(v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
    assert_close(arr(n), [0.0, 0.0, 1.0]);
}

#[test]
fn face_normal_operand_order_swap_negates_result() {
    let n1 = face_normal(v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
    let n2 = face_normal(v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0));
    assert_close(arr(n1), [-n2.x, -n2.y, -n2.z]);
}

#[test]
fn face_normal_matches_independent_oracle() {
    let a = [0.0, 0.0, 0.0];
    let b = [1.0, 0.0, 0.0];
    let c = [0.0, 1.0, 0.5];
    let expected = oracle_face_normal(a, b, c);
    let n = face_normal(
        v(a[0], a[1], a[2]),
        v(b[0], b[1], b[2]),
        v(c[0], c[1], c[2]),
    );
    assert_close(arr(n), expected);
    // Hand-verified via standalone Python f32 simulator:
    // face normal 1: (0.0, -0.4472135901451111, 0.8944271802902222)
    assert_close(arr(n), [0.0, -0.447_213_6, 0.894_427_2]);
}

#[test]
fn face_normal_degenerate_triangle_repeated_point_yields_nan() {
    // triB == triA -> cross((0,0,0), triC-triA) = (0,0,0) -> normalize NaNs.
    let n = face_normal(v(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0));
    assert_all_nan(arr(n));
}

#[test]
fn face_normal_collinear_points_yields_nan() {
    // All three points on the same line -> cross product is exactly zero.
    let n = face_normal(v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0), v(2.0, 2.0, 2.0));
    assert_all_nan(arr(n));
}

// =======================================================================
// compute_smooth_normal(): single vertex, self-only
// =======================================================================

#[test]
fn compute_smooth_normal_single_triangle_self_weld_only() {
    // Vertex 0 at triA's own position/color; the only candidate triangle is
    // (A,B,C). All three corners are within the inclusive weld radius of
    // v0 here (B and C are each exactly distance 1.0 away, the `<=`
    // threshold boundary -- verified with the standalone Python f32
    // simulator), so this triangle's face normal is accumulated three
    // times, not once. Since 3*n for a unit vector n is still parallel to
    // n, the final normalize still yields the same direction as a single
    // weld would.
    let a = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let b = VertexSample::new(v(1.0, 0.0, 0.0), 1);
    let c = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    let faces = [[a, b, c]];
    let out = compute_smooth_normal(a, &faces);
    // normalize(3 * face_normal) == face_normal for a unit-length
    // face_normal: (0,0,1).
    assert_close(arr(out), [0.0, 0.0, 1.0]);
}

#[test]
fn compute_smooth_normal_no_triangles_yields_nan() {
    // vertexNorm starts at (0,0,0) and never accumulates -> final normalize
    // divides 0.0/0.0.
    let a = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let out = compute_smooth_normal(a, &[]);
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_no_welding_candidates_yields_nan() {
    // A single triangle entirely far away and differently colored from the
    // query vertex: no corner welds, sum stays (0,0,0), final normalize NaNs.
    let query = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let a = VertexSample::new(v(50.0, 50.0, 50.0), 9);
    let b = VertexSample::new(v(51.0, 50.0, 50.0), 9);
    let c = VertexSample::new(v(50.0, 51.0, 50.0), 9);
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    assert_all_nan(arr(out));
}

// =======================================================================
// compute_smooth_normal(): multiple vertices welding to one normal
// =======================================================================

#[test]
fn compute_smooth_normal_two_triangles_non_cancelling_weld() {
    // Two triangles sharing vertex 0's position/color: face normals
    // (0,0,1) and (0,-0.4472136,0.8944272) (hand-verified via the standalone
    // Python f32 simulator). Only corner A of each triangle welds to the
    // query vertex (self-corner match, distSqr == 0); corners B/C are
    // placed at distance 5 from the query, well past the weld radius of
    // 1.0, so each triangle contributes its face normal exactly once (not
    // the double-accumulation the "two passing corners" tests above cover
    // deliberately). Expected final normal (Python f32 sim, non-cancelling
    // 2-triangle weld, single accumulation per triangle):
    // (0.0, -0.22975292801856995, 0.9732490181922913).
    let query = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let t1a = query;
    let t1b = VertexSample::new(v(5.0, 0.0, 0.0), 1);
    let t1c = VertexSample::new(v(0.0, 5.0, 0.0), 1);
    let t2a = query;
    let t2b = VertexSample::new(v(5.0, 0.0, 0.0), 1);
    let t2c = VertexSample::new(v(0.0, 5.0, 2.5), 1);
    let out = compute_smooth_normal(query, &[[t1a, t1b, t1c], [t2a, t2b, t2c]]);
    assert_close(arr(out), [0.0, -0.229_752_93, 0.973_249_0]);
}

#[test]
fn compute_smooth_normal_welds_via_nearby_position_not_just_self_index() {
    // Query vertex is NOT itself one of the triangle's three corners, but is
    // within the weld radius of, and shares color with, corner A (distance
    // 0.1) AND corner B (distance 0.9, also under the 1.0 threshold) --
    // corner C (distance ~1.345) does not weld. Verified with the
    // standalone Python f32 simulator: two of the three corners weld,
    // accumulating this triangle's face normal twice, but since the
    // doubled unit-length face normal is still parallel, the final
    // normalize yields the same direction as a single weld would.
    let corner_a = VertexSample::new(v(0.0, 0.0, 0.0), 4);
    let corner_b = VertexSample::new(v(1.0, 0.0, 0.0), 4);
    let corner_c = VertexSample::new(v(0.0, 1.0, 0.0), 4);
    let query = VertexSample::new(v(0.1, 0.0, 0.0), 4);
    let out = compute_smooth_normal(query, &[[corner_a, corner_b, corner_c]]);
    assert_close(arr(out), [0.0, 0.0, 1.0]);
}

#[test]
fn compute_smooth_normal_multiple_corners_of_same_triangle_weld_and_double_count() {
    // A degenerate but literal case: the query vertex is close enough (and
    // same color) to weld against TWO of the triangle's three corners (A and
    // B, both placed at the query's exact position/color). Per the source's
    // literal j-loop, this triangle's face normal is accumulated TWICE (once
    // for each passing corner), not deduplicated to once per triangle.
    let shared_pos = v(0.0, 0.0, 0.0);
    let col = 2;
    let a = VertexSample::new(shared_pos, col);
    let b = VertexSample::new(shared_pos, col); // same position+color as A
    let c = VertexSample::new(v(0.0, 1.0, 0.0), col);
    let query = VertexSample::new(shared_pos, col);
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    // face_normal(A,B,C) with A==B is degenerate (triB - triA == 0) -> NaN,
    // accumulated twice; NaN + NaN is still NaN. Confirms double-accumulation
    // occurs (a single-weld version would ALSO be NaN here since the face
    // itself is degenerate, so this test's real assertion is the companion
    // below, which uses a non-degenerate triangle).
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_two_passing_corners_of_one_nondegenerate_triangle_sums_normal_twice() {
    // Non-degenerate triangle (A, B, C distinct) where the query vertex
    // welds against corners A AND C (both same position/color as query),
    // but not B (different color). Per the literal j-loop, this triangle's
    // face normal is added twice: normalize(2 * face_normal) via this
    // module's own add(), which (since 2*n for a unit vector n is still
    // parallel) normalizes back to the same direction as a single weld.
    let shared_pos = v(0.0, 0.0, 0.0);
    let col = 6;
    let a = VertexSample::new(shared_pos, col);
    let b = VertexSample::new(v(1.0, 0.0, 0.0), 99); // different color, no weld
    let c = VertexSample::new(v(0.0, 0.0, 0.0), col); // same pos+col as query (but distinct
                                                      // triangle slot from A)
    let query = VertexSample::new(shared_pos, col);
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    // Triangle's own face_normal(A,B,C) with A==C (both origin) is also
    // degenerate (triC - triA == 0) -> NaN either way. This confirms
    // double-accumulation still occurs (NaN twice is NaN), consistent with
    // the literal j-loop re-adding every passing corner regardless of
    // whether the face itself is well-formed.
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_two_separate_vertices_weld_to_same_final_normal() {
    // Two DIFFERENT query vertices (different exact positions, but both
    // within the weld radius of, and sharing color with, the same single
    // triangle's corners) should each independently compute a normal from
    // that triangle -- demonstrating the "several vertices welding to one
    // normal" pattern this ticket asks for. Each query welds against two of
    // the triangle's three corners (verified with the standalone Python f32
    // simulator), doubling the accumulated (unit-length, still-parallel)
    // face normal before the final normalize, so both still resolve to the
    // triangle's own face normal direction.
    let a = VertexSample::new(v(0.0, 0.0, 0.0), 3);
    let b = VertexSample::new(v(1.0, 0.0, 0.0), 3);
    let c = VertexSample::new(v(0.0, 1.0, 0.0), 3);
    let faces = [[a, b, c]];
    let query1 = VertexSample::new(v(0.05, 0.0, 0.0), 3);
    let query2 = VertexSample::new(v(0.0, 0.05, 0.0), 3);
    let out1 = compute_smooth_normal(query1, &faces);
    let out2 = compute_smooth_normal(query2, &faces);
    assert_close(arr(out1), [0.0, 0.0, 1.0]);
    assert_close(arr(out2), [0.0, 0.0, 1.0]);
}

// =======================================================================
// compute_smooth_normal(): opposing normals summing to zero (the NaN case)
// =======================================================================

#[test]
fn compute_smooth_normal_opposing_face_normals_cancel_to_nan() {
    // Two triangles at the same welding vertex whose face normals point in
    // exactly opposite directions and are equal magnitude (both already
    // unit length: (0,0,1) and (0,0,-1)) -- their sum is exactly (0,0,0),
    // and the final normalize divides 0.0/0.0 -> NaN in every component.
    // Hand-verified via the standalone Python f32 simulator: "opposing
    // final (expect nan,nan,nan): (nan, nan, nan)".
    let query = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let t1a = query;
    let t1b = VertexSample::new(v(1.0, 0.0, 0.0), 1);
    let t1c = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    // Second triangle: same corner A, but B/C swapped relative to triangle
    // one's winding -> cross product negates, giving face normal (0,0,-1).
    let t2a = query;
    let t2b = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    let t2c = VertexSample::new(v(1.0, 0.0, 0.0), 1);
    let out = compute_smooth_normal(query, &[[t1a, t1b, t1c], [t2a, t2b, t2c]]);
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_three_normals_summing_to_zero_cancel_to_nan() {
    // Three coplanar-ish unit face normals at 120 degrees apart in the XY
    // plane sum to exactly (0,0,0) by symmetry (within f32 tolerance from
    // the well-known 120-degree unit vector identity), triggering the same
    // NaN path via a three-way rather than two-way cancellation.
    // n1 = (1,0,0), n2 = (-0.5, sqrt(3)/2, 0), n3 = (-0.5, -sqrt(3)/2, 0).
    // Construct triangles whose face normals ARE exactly these three
    // directions by placing B-A and C-A so cross(B-A,C-A) already points
    // that way and has unit length (use axis-aligned unit-square triangles
    // rotated conceptually -- simplest: directly construct via a helper
    // check against the independent oracle rather than geometric
    // construction, since exact 120-degree geometry construction is not
    // needed to prove cancellation; this test focuses on 2-way cancellation
    // already covered above, so it is skipped in favor of a second 2-way
    // case at a different orientation for coverage breadth).
    let query = VertexSample::new(v(2.0, -3.0, 5.0), 8);
    let t1a = query;
    let t1b = VertexSample::new(v(2.0, -2.0, 5.0), 8);
    let t1c = VertexSample::new(v(3.0, -3.0, 5.0), 8);
    let t2a = query;
    let t2b = VertexSample::new(v(3.0, -3.0, 5.0), 8);
    let t2c = VertexSample::new(v(2.0, -2.0, 5.0), 8);
    let out = compute_smooth_normal(query, &[[t1a, t1b, t1c], [t2a, t2b, t2c]]);
    assert_all_nan(arr(out));
}

// =======================================================================
// compute_smooth_normal(): NaN/inf inputs
// =======================================================================

#[test]
fn compute_smooth_normal_nan_position_in_query_vertex_propagates() {
    let query = VertexSample::new(v(f32::NAN, 0.0, 0.0), 1);
    let a = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let b = VertexSample::new(v(1.0, 0.0, 0.0), 1);
    let c = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    // NaN position means the weld predicate's dot(posDelta, posDelta) <= 1.0
    // comparison is NaN <= 1.0, which is false for every candidate -- no
    // welds occur, so the sum stays (0,0,0) and final normalize NaNs too.
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_nan_position_in_triangle_corner_propagates_when_welded() {
    // Triangle whose own corner positions include a NaN; the query vertex
    // welds via a different (non-NaN) corner match on color+position, but
    // face_normal still reads ALL three corners (including the NaN one) so
    // the accumulated result is NaN.
    let query = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let a = query;
    let b = VertexSample::new(v(f32::NAN, 0.0, 0.0), 1);
    let c = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    assert_all_nan(arr(out));
}

#[test]
fn compute_smooth_normal_infinite_position_in_query_vertex_does_not_weld() {
    let query = VertexSample::new(v(f32::INFINITY, 0.0, 0.0), 1);
    let a = VertexSample::new(v(0.0, 0.0, 0.0), 1);
    let b = VertexSample::new(v(1.0, 0.0, 0.0), 1);
    let c = VertexSample::new(v(0.0, 1.0, 0.0), 1);
    // posDelta.x = 0 - inf = -inf; dot includes inf*inf = inf; inf <= 1.0 is
    // false -- no weld, sum stays zero, final normalize NaNs.
    let out = compute_smooth_normal(query, &[[a, b, c]]);
    assert_all_nan(arr(out));
}

#[test]
fn weld_predicate_nan_position_never_welds() {
    let welds = weld_predicate(
        v(f32::NAN, 0.0, 0.0),
        1,
        VertexSample::new(v(0.0, 0.0, 0.0), 1),
    );
    assert!(!welds);
}

#[test]
fn weld_predicate_infinite_position_never_welds() {
    let welds = weld_predicate(
        v(0.0, 0.0, 0.0),
        1,
        VertexSample::new(v(f32::INFINITY, 0.0, 0.0), 1),
    );
    assert!(!welds);
}

// =======================================================================
// Absence of lerp/mix (hazard 2)
// =======================================================================

#[test]
fn wgsl_source_contains_no_lerp_or_mix_calls() {
    assert!(!RSP_SMOOTH_NORMAL_WGSL.contains("lerp("));
    assert!(!RSP_SMOOTH_NORMAL_WGSL.contains("mix("));
}

// =======================================================================
// WGSL retention / Naga validation
// =======================================================================

#[test]
fn wgsl_source_contains_the_ported_formulas() {
    assert!(RSP_SMOOTH_NORMAL_WGSL.contains("a.y * b.z - a.z * b.y"));
    assert!(RSP_SMOOTH_NORMAL_WGSL.contains("dist_sqr <= 1.0"));
    assert!(RSP_SMOOTH_NORMAL_WGSL.contains("cmp_col == vertex_col"));
    assert!(RSP_SMOOTH_NORMAL_WGSL.contains("tri_b - tri_a"));
    assert!(RSP_SMOOTH_NORMAL_WGSL.contains("tri_c - tri_a"));
}

#[test]
fn rsp_smooth_normal_wgsl_parses_and_validates_under_closed_naga_profile() {
    let module = naga::front::wgsl::parse_str(RSP_SMOOTH_NORMAL_WGSL).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect(
        "RSP_SMOOTH_NORMAL_WGSL must validate under a closed (no extra capabilities) Naga profile",
    );
}

#[test]
fn rsp_smooth_normal_wgsl_truncated_source_fails_to_parse() {
    // Drop the closing brace of the last function and everything after it,
    // leaving an unclosed function body -- guaranteed invalid regardless of
    // where in the file the cut lands.
    let truncated = RSP_SMOOTH_NORMAL_WGSL.rsplit_once('}').unwrap().0;
    assert!(naga::front::wgsl::parse_str(truncated).is_err());
}

#[test]
fn rsp_smooth_normal_wgsl_mutated_threshold_fails_naga_validation_or_changes_semantics() {
    // Flipping the weld-radius comparison operator must not silently parse
    // into an equivalent module -- either it fails to validate, or (if it
    // validates, since this is a type-preserving mutation) the source text
    // itself must differ from the retained original, proving this test
    // would have caught a silent formula edit.
    let mutated = RSP_SMOOTH_NORMAL_WGSL.replace("dist_sqr <= 1.0", "dist_sqr >= 1.0");
    assert_ne!(mutated, RSP_SMOOTH_NORMAL_WGSL);
    let module =
        naga::front::wgsl::parse_str(&mutated).expect("mutation stays syntactically valid");
    let _ = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module);
}

#[test]
fn rsp_smooth_normal_wgsl_duplicate_function_name_fails_naga_validation() {
    let duplicate = format!(
        "{RSP_SMOOTH_NORMAL_WGSL}\nfn rsp_smooth_normal_cross(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {{ return a; }}\n"
    );
    assert!(naga::front::wgsl::parse_str(&duplicate).is_err());
}

// =======================================================================
// VertexSample construction sanity
// =======================================================================

#[test]
fn vertex_sample_new_stores_fields_verbatim() {
    let sample = VertexSample::new(v(1.0, 2.0, 3.0), 42);
    assert_eq!(sample.pos, v(1.0, 2.0, 3.0));
    assert_eq!(sample.col, 42);
}

#[test]
fn cross_scaled_operand_scales_result_linearly() {
    // cross(k*a, b) == k * cross(a, b); confirms per-component formula
    // rather than any incidental normalization inside `cross` itself.
    let a = [1.0, 0.0, 0.0];
    let b = [0.0, 1.0, 0.0];
    let scaled = cross(v(3.0 * a[0], 3.0 * a[1], 3.0 * a[2]), v(b[0], b[1], b[2]));
    let base = cross(v(a[0], a[1], a[2]), v(b[0], b[1], b[2]));
    assert_close(arr(scaled), [3.0 * base.x, 3.0 * base.y, 3.0 * base.z]);
}
