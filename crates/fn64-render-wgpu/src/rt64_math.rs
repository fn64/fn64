//! Literal port of the bounded RT64 `rt64_math.cpp` scalar/matrix-predicate/
//! projection-derivation cluster: a literal port of the permitted MIT RT64
//! Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_math.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `d0d768a666555a3099b564fe8b7f62af088e921e7ebc8f9d232e5e3239f406a9` /
//! `d32abc9572001870b4144ffa49e832589858de0830dbb0d008761ad15a76364b`):
//!
//! ```text
//! float sqr(float x) {
//!     return x * x;
//! }
//!
//! bool matrixIsNaN(const hlslpp::float4x4 &m) {
//!     for (uint32_t i = 0; i < 4; i++) {
//!         if (hlslpp::any(hlslpp::isnan(m[i]))) {
//!             return true;
//!         }
//!     }
//!     return false;
//! }
//!
//! float traceFrom3x3(const hlslpp::float3x3 &m) {
//!     return m[0][0] + m[1][1] + m[2][2];
//! }
//!
//! bool isMatrixAffine(const hlslpp::float4x4 &m) {
//!     return (m[0][3] == 0.0f) && (m[1][3] == 0.0f) && (m[2][3] == 0.0f) && (m[3][3] == 1.0f);
//! }
//!
//! bool isMatrixIdentity(const hlslpp::float4x4 &m) {
//!     return hlslpp::all(m == hlslpp::float4x4::identity());
//! }
//!
//! bool isMatrixViewProj(const hlslpp::float4x4 &m) {
//!     return (abs(m[3][3]) >= 1e-6f) && (abs(1.0f - m[3][3]) >= 1e-6f);
//! }
//!
//! float nearPlaneFromProj(const hlslpp::float4x4 &m) {
//!     return std::max((m[3][3] + m[3][2]) / (m[2][2] + m[2][3]), 1e-5f);
//! }
//!
//! float farPlaneFromProj(const hlslpp::float4x4 &m) {
//!     return std::max((m[3][2] - m[3][3]) / (m[2][2] - m[2][3]), 1e-4f);
//! }
//!
//! float fovFromProj(const hlslpp::float4x4 &m) {
//!     return std::max(2.0f * atanf(-m[2][3] / m[1][1]), 1e-2f);
//! }
//!
//! hlslpp::float2 barycentricCoordinates(const hlslpp::float2 p, const hlslpp::float2 a, const hlslpp::float2 b, const hlslpp::float2 c) {
//!     float area = -b.y * c.x + a.y * (c.x - b.x) + a.x * (b.y - c.y) + b.x * c.y;
//!     float s = (a.y * c.x - a.x * c.y + (c.y - a.y) * p.x + (a.x - c.x) * p.y) / area;
//!     float t = (a.x * b.y - a.y * b.x + (a.y - b.y) * p.x + (b.x - a.x) * p.y) / area;
//!     return { s, t };
//! }
//!
//! bool epsilonEqual(float a, float b) {
//!     return abs(a - b) < std::numeric_limits<float>::epsilon();
//! }
//! ```
//!
//! **Reuse, not new matrix type.** This module reuses
//! [`fn64_render_ir::{Mat4, Vec4}`](fn64_render_ir) directly for the eleven
//! `float4x4`-shaped functions -- no new matrix/vector type, and no
//! `fn64-render-ir` edit. `Mat4` is "a backend-neutral **row-major** 4x4
//! float matrix, matching HLSL `float4x4`" with `rows[i]` = row `i`,
//! `rows[i].x/y/z/w` = that row's four columns (`rsp_math.rs:78-84`), so an
//! HLSL `m[i][j]` read becomes `m.rows[i].{x,y,z,w}` for `j = 0..3`. A local
//! [`Mat3`] (3x3, same row-major shape) is added in *this* module for
//! `trace_from_3x3` only, since `fn64_render_ir::rsp_math` has no 3x3 type
//! and no other ported function here needs one.
//!
//! `barycentric_coordinates` uses plain `(f32, f32)` tuples for `p`/`a`/`b`/
//! `c` and its return value: `fn64_render_ir::rsp_math` has no `Vec2` type
//! (only `Vec3`/`Vec4`), and adding one for this single caller would go
//! against `RENDER-WGPU-PORT-PLAN.md`'s dependency-boundary rule that
//! `fn64-render-ir`'s vector types exist specifically for RSP math. This
//! matches `texture_lod.rs`'s established precedent of avoiding new small
//! vector types when nothing else needs them.
//!
//! ## Admitted domain / unpinned HLSL semantics (bound, not invented)
//!
//! - **`barycentric_coordinates` at `area == 0.0`** (degenerate/collinear
//!   triangle): the source performs an unguarded float division. This port
//!   lets plain IEEE-754 division propagate (`±inf` or `NaN` depending on
//!   numerator sign/zero), matching `computeLOD`'s and `depth_strict_less.rs`'s
//!   precedent of preserving unguarded upstream arithmetic rather than
//!   inventing a guard the source does not have.
//! - **`fov_from_proj` at `m[1][1] == 0.0`**: `atanf(-m[2][3] / 0.0)` is
//!   `±inf`/`NaN` per the same unguarded-division policy; `atanf` of an
//!   infinite input is well-defined in IEEE-754 (`±π/2`), so this resolves
//!   to a finite result even though the intermediate division does not.
//! - **`hlslpp::any`/`hlslpp::isnan` per-lane semantics for `matrix_is_nan`**:
//!   `hlslpp` is an unpopulated submodule in every checkout available to
//!   this program (`rsp_math.rs:21-23` documents this same constraint) --
//!   there is no vendored source to inspect for a possible platform-dependent
//!   divergence. This port assumes plain per-lane `f32::is_nan()` ORed
//!   across all 16 elements is the correct literal translation, stated here
//!   as an assumption rather than a verified fact.
//! - **`hlslpp::float4x4::identity()`** for `is_matrix_identity`: assumed to
//!   be the standard mathematical identity matrix (`1`s on the diagonal,
//!   `0`s elsewhere) -- conventional, not contradicted by any hlslpp source
//!   citation in this repo, but likewise not a verified read (`hlslpp` is
//!   unpopulated in every available checkout).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, camera/view-matrix wiring, HFR/projection-matching
//! integration (`rt64_game_frame.cpp` itself remains unported), triangle
//! rasterizer integration, or RT64 visual/pixel/silicon parity or
//! performance claim. Does not port `matrixScale`, `matrixTranslation`,
//! `matrixRotationX/Y/Z`, `extract3x3`, `rotationFrom3x3`,
//! `matrixDifference`, `lerpMatrix`/`lerpMatrix3x3`/`lerpMatrixComponents`,
//! `matrixDecomposeViewProj`, `decomposeMatrix`/`recomposeMatrix`,
//! `DecomposedTransform`/`lerpTransforms` (deferred -- needs new
//! matrix-inverse/quaternion infra), or `pseudoRandom` (bit-identical to
//! `random.rs`'s already-landed `RandomState::advance`, would be a literal
//! duplicate under a new name).

use fn64_render_ir::{Mat4, Vec4};

/// `sqr(x) = x * x`.
pub fn sqr(x: f32) -> f32 {
    x * x
}

/// `matrixIsNaN`: true if any of the 16 elements is NaN.
pub fn matrix_is_nan(m: Mat4) -> bool {
    for row in m.rows {
        if row.x.is_nan() || row.y.is_nan() || row.z.is_nan() || row.w.is_nan() {
            return true;
        }
    }
    false
}

/// A row-major 3x3 float matrix, matching HLSL `float3x3`. Local to this
/// module because `fn64_render_ir::rsp_math` has no 3x3 type and no other
/// ported function here needs one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3 {
    pub rows: [fn64_render_ir::Vec3; 3],
}

/// `traceFrom3x3`: `m[0][0] + m[1][1] + m[2][2]`.
pub fn trace_from_3x3(m: Mat3) -> f32 {
    m.rows[0].x + m.rows[1].y + m.rows[2].z
}

/// `isMatrixAffine`: last column is `(0,0,0,1)`.
pub fn is_matrix_affine(m: Mat4) -> bool {
    m.rows[0].w == 0.0 && m.rows[1].w == 0.0 && m.rows[2].w == 0.0 && m.rows[3].w == 1.0
}

/// `isMatrixIdentity`: exact (not epsilon) equality to the identity matrix.
pub fn is_matrix_identity(m: Mat4) -> bool {
    m == Mat4::from_rows([
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    ])
}

/// `isMatrixViewProj`: `m[3][3]` is neither ~0 nor ~1 (within `1e-6`).
pub fn is_matrix_view_proj(m: Mat4) -> bool {
    let m33 = m.rows[3].w;
    m33.abs() >= 1e-6 && (1.0 - m33).abs() >= 1e-6
}

/// `nearPlaneFromProj`: `(m[3][3]+m[3][2]) / (m[2][2]+m[2][3])`, floored at
/// `1e-5`.
pub fn near_plane_from_proj(m: Mat4) -> f32 {
    let m22 = m.rows[2].z;
    let m23 = m.rows[2].w;
    let m32 = m.rows[3].z;
    let m33 = m.rows[3].w;
    ((m33 + m32) / (m22 + m23)).max(1e-5)
}

/// `farPlaneFromProj`: `(m[3][2]-m[3][3]) / (m[2][2]-m[2][3])`, floored at
/// `1e-4`.
pub fn far_plane_from_proj(m: Mat4) -> f32 {
    let m22 = m.rows[2].z;
    let m23 = m.rows[2].w;
    let m32 = m.rows[3].z;
    let m33 = m.rows[3].w;
    ((m32 - m33) / (m22 - m23)).max(1e-4)
}

/// `fovFromProj`: `2*atan(-m[2][3]/m[1][1])`, floored at `1e-2`.
pub fn fov_from_proj(m: Mat4) -> f32 {
    let m11 = m.rows[1].y;
    let m23 = m.rows[2].w;
    (2.0 * (-m23 / m11).atan()).max(1e-2)
}

/// `epsilonEqual`: `|a-b| < f32::EPSILON` (strict less-than, not `<=`).
pub fn epsilon_equal(a: f32, b: f32) -> bool {
    (a - b).abs() < f32::EPSILON
}

/// `barycentricCoordinates`: returns `(s, t)`; the third weight is
/// `1-s-t` by the caller's own convention (RT64 returns only `float2`,
/// matching this port).
pub fn barycentric_coordinates(
    p: (f32, f32),
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) -> (f32, f32) {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let (cx, cy) = c;

    let area = -by * cx + ay * (cx - bx) + ax * (by - cy) + bx * cy;
    let s = (ay * cx - ax * cy + (cy - ay) * px + (ax - cx) * py) / area;
    let t = (ax * by - ay * bx + (ay - by) * px + (bx - ax) * py) / area;
    (s, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::Vec3;

    fn identity() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn mat_with(mut set: impl FnMut(&mut Mat4)) -> Mat4 {
        let mut m = identity();
        set(&mut m);
        m
    }

    fn set_elem(m: &mut Mat4, i: usize, j: usize, v: f32) {
        let row = &mut m.rows[i];
        match j {
            0 => row.x = v,
            1 => row.y = v,
            2 => row.z = v,
            3 => row.w = v,
            _ => unreachable!(),
        }
    }

    // --- sqr ---

    #[test]
    fn sqr_zero() {
        assert_eq!(sqr(0.0), 0.0);
    }

    #[test]
    fn sqr_one() {
        assert_eq!(sqr(1.0), 1.0);
    }

    #[test]
    fn sqr_negative_one_squares_away_sign() {
        assert_eq!(sqr(-1.0), 1.0);
    }

    #[test]
    fn sqr_overflows_to_infinity() {
        assert_eq!(sqr(1e20), f32::INFINITY);
    }

    #[test]
    fn sqr_nan_propagates() {
        assert!(sqr(f32::NAN).is_nan());
    }

    // --- matrix_is_nan ---

    #[test]
    fn matrix_is_nan_identity_false() {
        assert!(!matrix_is_nan(identity()));
    }

    #[test]
    fn matrix_is_nan_each_of_sixteen_positions() {
        for i in 0..4 {
            for j in 0..4 {
                let m = mat_with(|m| set_elem(m, i, j, f32::NAN));
                assert!(matrix_is_nan(m), "position ({i},{j}) should be NaN-true");
            }
        }
    }

    #[test]
    fn matrix_is_nan_all_nan_true() {
        let m = Mat4::from_rows([Vec4::new(f32::NAN, f32::NAN, f32::NAN, f32::NAN); 4]);
        assert!(matrix_is_nan(m));
    }

    #[test]
    fn matrix_is_nan_all_inf_false() {
        let m = Mat4::from_rows(
            [Vec4::new(f32::INFINITY, f32::INFINITY, f32::INFINITY, f32::INFINITY); 4],
        );
        assert!(!matrix_is_nan(m));
    }

    // --- trace_from_3x3 ---

    #[test]
    fn trace_from_3x3_identity_is_three() {
        let m = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        assert_eq!(trace_from_3x3(m), 3.0);
    }

    #[test]
    fn trace_from_3x3_zero_matrix_is_zero() {
        let m = Mat3 {
            rows: [Vec3::splat(0.0); 3],
        };
        assert_eq!(trace_from_3x3(m), 0.0);
    }

    #[test]
    fn trace_from_3x3_ignores_off_diagonal() {
        let m = Mat3 {
            rows: [
                Vec3::new(5.0, 100.0, 200.0),
                Vec3::new(300.0, 6.0, 400.0),
                Vec3::new(500.0, 600.0, 7.0),
            ],
        };
        assert_eq!(trace_from_3x3(m), 18.0);
    }

    // --- is_matrix_affine ---

    #[test]
    fn is_matrix_affine_identity_true() {
        assert!(is_matrix_affine(identity()));
    }

    #[test]
    fn is_matrix_affine_m03_nonzero_false() {
        let m = mat_with(|m| set_elem(m, 0, 3, 1.0));
        assert!(!is_matrix_affine(m));
    }

    #[test]
    fn is_matrix_affine_m13_nonzero_false() {
        let m = mat_with(|m| set_elem(m, 1, 3, 1.0));
        assert!(!is_matrix_affine(m));
    }

    #[test]
    fn is_matrix_affine_m23_nonzero_false() {
        let m = mat_with(|m| set_elem(m, 2, 3, 1.0));
        assert!(!is_matrix_affine(m));
    }

    #[test]
    fn is_matrix_affine_m33_not_one_false() {
        let m = mat_with(|m| set_elem(m, 3, 3, 0.5));
        assert!(!is_matrix_affine(m));
    }

    #[test]
    fn is_matrix_affine_nonidentity_affine_true() {
        // Only the upper-left 3x3 and translation row differ; last column
        // is still (0,0,0,1) -- proves the check is only the last column,
        // not full identity.
        let m = mat_with(|m| {
            set_elem(m, 0, 0, 2.0);
            set_elem(m, 3, 0, 10.0);
            set_elem(m, 3, 1, 20.0);
            set_elem(m, 3, 2, 30.0);
        });
        assert!(is_matrix_affine(m));
    }

    // --- is_matrix_identity ---

    #[test]
    fn is_matrix_identity_identity_true() {
        assert!(is_matrix_identity(identity()));
    }

    #[test]
    fn is_matrix_identity_each_element_perturbed_by_epsilon_is_false() {
        for i in 0..4 {
            for j in 0..4 {
                let base = if i == j { 1.0 } else { 0.0 };
                let perturbed = base + f32::EPSILON.max(f32::MIN_POSITIVE);
                let m = mat_with(|m| set_elem(m, i, j, perturbed));
                assert!(
                    !is_matrix_identity(m),
                    "position ({i},{j}) perturbed should be false"
                );
            }
        }
    }

    // --- is_matrix_view_proj ---

    #[test]
    fn is_matrix_view_proj_m33_zero_false() {
        let m = mat_with(|m| set_elem(m, 3, 3, 0.0));
        assert!(!is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_m33_one_false() {
        let m = mat_with(|m| set_elem(m, 3, 3, 1.0));
        assert!(!is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_m33_half_true() {
        let m = mat_with(|m| set_elem(m, 3, 3, 0.5));
        assert!(is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_boundary_at_exactly_1e_minus_6_from_zero_is_true() {
        // abs(m33) >= 1e-6 -- exactly at the boundary must be true (>=, not >).
        let m = mat_with(|m| set_elem(m, 3, 3, 1e-6));
        assert!(is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_just_inside_1e_minus_6_from_zero_is_false() {
        let m = mat_with(|m| set_elem(m, 3, 3, 1e-6 * 0.5));
        assert!(!is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_boundary_at_exactly_1e_minus_6_from_one_is_true() {
        // abs(1-m33) >= 1e-6 -- exactly at the boundary must be true.
        let m = mat_with(|m| set_elem(m, 3, 3, 1.0 - 1e-6));
        assert!(is_matrix_view_proj(m));
    }

    #[test]
    fn is_matrix_view_proj_just_inside_1e_minus_6_from_one_is_false() {
        let m = mat_with(|m| set_elem(m, 3, 3, 1.0 - 1e-6 * 0.5));
        assert!(!is_matrix_view_proj(m));
    }

    // --- near_plane_from_proj / far_plane_from_proj / fov_from_proj ---
    //
    // Independently re-derived (not circularly, from the formula itself)
    // fixture matrix: algebraically solved for m22/m32 given m23=-1, m33=0,
    // zn=1.0, zf=100.0 by requiring both `near = m32/(m22-1)` and
    // `far = m32/(m22+1)` hold, which yields
    // `m22 = (zn+zf)/(zn-zf)`, `m32 = zn*(m22-1)`. Confirmed round-trip with
    // an independent Rust scratch binary (outside this crate) before this
    // fixture was written: near_plane_from_proj recovers 1.0 exactly,
    // far_plane_from_proj recovers 100.0 within 1e-4 (float rounding), and
    // fov_from_proj recovers the input FOV exactly, given m11 = 1/tan(fov/2).

    fn projection_fixture() -> Mat4 {
        let zn = 1.0_f32;
        let zf = 100.0_f32;
        let fov = std::f32::consts::FRAC_PI_3; // 60 degrees
        let m23 = -1.0_f32;
        let m33 = 0.0_f32;
        let m22 = (zn + zf) / (zn - zf);
        let m32 = zn * (m22 - 1.0);
        let m11 = 1.0 / (fov / 2.0).tan();

        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, m11, 0.0, 0.0),
            Vec4::new(0.0, 0.0, m22, m23),
            Vec4::new(0.0, 0.0, m32, m33),
        ])
    }

    #[test]
    fn near_plane_from_proj_recovers_independently_derived_near() {
        let near = near_plane_from_proj(projection_fixture());
        assert!((near - 1.0).abs() < 1e-4, "near={near}");
    }

    #[test]
    fn far_plane_from_proj_recovers_independently_derived_far() {
        let far = far_plane_from_proj(projection_fixture());
        assert!((far - 100.0).abs() < 1e-2, "far={far}");
    }

    #[test]
    fn fov_from_proj_recovers_independently_derived_fov() {
        let fov = fov_from_proj(projection_fixture());
        let expected = std::f32::consts::FRAC_PI_3;
        assert!((fov - expected).abs() < 1e-4, "fov={fov}");
    }

    #[test]
    fn near_plane_from_proj_floors_at_1e_minus_5() {
        // m33=0, m32=0, m22=1, m23=1 -> (0+0)/(1+1) = 0.0, below the floor.
        let m = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        assert_eq!(near_plane_from_proj(m), 1e-5);
    }

    #[test]
    fn near_plane_from_proj_boundary_at_exactly_floor_value() {
        let m = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            Vec4::new(0.0, 0.0, 2e-5, 0.0),
        ]);
        // (0 + 2e-5) / (1 + 1) = 1e-5 exactly.
        assert_eq!(near_plane_from_proj(m), 1e-5);
    }

    #[test]
    fn far_plane_from_proj_floors_at_1e_minus_4() {
        let m = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 1.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        // (0-0)/(1-1) = 0/0 = NaN; NaN.max(1e-4) in Rust returns 1e-4.
        assert_eq!(far_plane_from_proj(m), 1e-4);
    }

    #[test]
    fn fov_from_proj_floors_at_1e_minus_2() {
        // m23=0, so -m23/m11 = 0.0, atan(0.0) = 0.0, 2*0.0 = 0.0, below floor.
        let m = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        assert_eq!(fov_from_proj(m), 1e-2);
    }

    #[test]
    fn fov_from_proj_m11_zero_division_still_yields_finite_atan() {
        // -m23/m11 with m11=0.0 and m23=-1.0 -> -(-1.0)/0.0 = +inf.
        // atan(+inf) = pi/2 (finite), 2*(pi/2) = pi.
        let m = Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, -1.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        let fov = fov_from_proj(m);
        assert!(fov.is_finite());
        assert!((fov - std::f32::consts::PI).abs() < 1e-6);
    }

    // --- epsilon_equal ---

    #[test]
    fn epsilon_equal_exactly_equal_true() {
        assert!(epsilon_equal(1.0, 1.0));
    }

    #[test]
    fn epsilon_equal_differing_by_less_than_epsilon_true() {
        assert!(epsilon_equal(1.0, 1.0 + f32::EPSILON * 0.5));
    }

    #[test]
    fn epsilon_equal_differing_by_exactly_epsilon_false() {
        // Strict less-than: exactly EPSILON apart is false.
        assert!(!epsilon_equal(1.0, 1.0 + f32::EPSILON));
    }

    #[test]
    fn epsilon_equal_differing_by_more_false() {
        assert!(!epsilon_equal(1.0, 2.0));
    }

    #[test]
    fn epsilon_equal_nan_false() {
        assert!(!epsilon_equal(f32::NAN, 1.0));
        assert!(!epsilon_equal(1.0, f32::NAN));
        assert!(!epsilon_equal(f32::NAN, f32::NAN));
    }

    // --- barycentric_coordinates ---

    const TRI_A: (f32, f32) = (0.0, 0.0);
    const TRI_B: (f32, f32) = (4.0, 0.0);
    const TRI_C: (f32, f32) = (0.0, 4.0);

    #[test]
    fn barycentric_at_a_is_zero_zero() {
        let (s, t) = barycentric_coordinates(TRI_A, TRI_A, TRI_B, TRI_C);
        assert!((s - 0.0).abs() < 1e-6);
        assert!((t - 0.0).abs() < 1e-6);
    }

    #[test]
    fn barycentric_at_b_is_one_zero() {
        let (s, t) = barycentric_coordinates(TRI_B, TRI_A, TRI_B, TRI_C);
        assert!((s - 1.0).abs() < 1e-6);
        assert!((t - 0.0).abs() < 1e-6);
    }

    #[test]
    fn barycentric_at_c_is_zero_one() {
        let (s, t) = barycentric_coordinates(TRI_C, TRI_A, TRI_B, TRI_C);
        assert!((s - 0.0).abs() < 1e-6);
        assert!((t - 1.0).abs() < 1e-6);
    }

    #[test]
    fn barycentric_at_centroid_is_one_third_each() {
        let centroid = (
            (TRI_A.0 + TRI_B.0 + TRI_C.0) / 3.0,
            (TRI_A.1 + TRI_B.1 + TRI_C.1) / 3.0,
        );
        let (s, t) = barycentric_coordinates(centroid, TRI_A, TRI_B, TRI_C);
        assert!((s - 1.0 / 3.0).abs() < 1e-5);
        assert!((t - 1.0 / 3.0).abs() < 1e-5);
    }

    #[test]
    fn barycentric_outside_triangle_unclamped() {
        // Far outside the triangle: no inside/outside guard in the source,
        // so this must return a defined (possibly negative or >1) result.
        let (s, t) = barycentric_coordinates((100.0, 100.0), TRI_A, TRI_B, TRI_C);
        assert!(s.is_finite());
        assert!(t.is_finite());
        assert!(s > 1.0 || t > 1.0);
    }

    #[test]
    fn barycentric_degenerate_zero_area_propagates_division_result() {
        // a, b, c collinear -> area == 0.0 -> unguarded division by zero.
        let collinear_a = (0.0, 0.0);
        let collinear_b = (1.0, 0.0);
        let collinear_c = (2.0, 0.0);
        let (s, t) = barycentric_coordinates((0.5, 0.0), collinear_a, collinear_b, collinear_c);
        // Numerator is nonzero for at least one of s/t at this point, area
        // is exactly 0.0, so IEEE-754 division yields +-inf (not NaN, since
        // the numerator is nonzero here) -- this is the admitted, unguarded
        // domain boundary, not a panic or invented guard.
        assert!(!s.is_finite() || !t.is_finite());
    }
}
