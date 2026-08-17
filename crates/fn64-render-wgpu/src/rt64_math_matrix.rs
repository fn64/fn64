//! Literal port of RT64 `rt64_math.cpp`'s deferred matrix cluster --
//! `extract3x3`, `rotationFrom3x3`, `matrixDifference`, `lerpMatrix`,
//! `lerpMatrix3x3`, `lerpMatrixComponents` -- a literal port of the
//! permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/common/rt64_math.h`/`.cpp`
//! (SHA-256 of the whole files,
//! `d0d768a666555a3099b564fe8b7f62af088e921e7ebc8f9d232e5e3239f406a9` /
//! `d32abc9572001870b4144ffa49e832589858de0830dbb0d008761ad15a76364b`):
//! `rt64_math.rs`'s own Nonclaims section explicitly deferred this cluster
//! ("Does not port ... `extract3x3`, `rotationFrom3x3`, `matrixDifference`,
//! `lerpMatrix`/`lerpMatrix3x3`/`lerpMatrixComponents` ... (deferred --
//! needs new matrix-inverse/quaternion infra)"); this module closes that
//! deferral for the six matrix-shaped functions only. The quaternion
//! `decomposeMatrix`/`recomposeMatrix`/`DecomposedTransform`/
//! `lerpTransforms` cluster in the same source file is a separate,
//! independently-owned port and is NOT ported here (see "Nonclaims").
//!
//! Source line ranges (`src/common/rt64_math.cpp`):
//!
//! ```text
//! // lines 116-122
//! hlslpp::float3x3 extract3x3(const hlslpp::float4x4 &m) {
//!     return hlslpp::float3x3(
//!         m[0][0], m[0][1], m[0][2],
//!         m[1][0], m[1][1], m[1][2],
//!         m[2][0], m[2][1], m[2][2]
//!     );
//! }
//!
//! // lines 124-126
//! hlslpp::float3x3 rotationFrom3x3(const hlslpp::float3x3 &m) {
//!     return hlslpp::float3x3(hlslpp::normalize(m[0]), hlslpp::normalize(m[1]), hlslpp::normalize(m[2]));
//! }
//!
//! // lines 132-139
//! float matrixDifference(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b) {
//!     float difference = 0.0f;
//!     for (int i = 0; i < 4; i++) {
//!         difference += hlslpp::dot(hlslpp::abs(a[i] - b[i]), 1.0f);
//!     }
//!
//!     return difference;
//! }
//!
//! // lines 153-163
//! hlslpp::float4x4 lerpMatrix(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, float t) {
//!     // Copy b into the result.
//!     hlslpp::float4x4 c = b;
//!
//!     // Replace with a component-wise linear interpolation between a and b.
//!     for (int i = 0; i < 4; i++) {
//!         c[i] = hlslpp::lerp(a[i], b[i], t);
//!     }
//!
//!     return c;
//! }
//!
//! // lines 165-175
//! hlslpp::float4x4 lerpMatrix3x3(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, float t) {
//!     // Copy b into the result.
//!     hlslpp::float4x4 c = b;
//!
//!     // Replace the result's top left 3x3 with a component-wise linear interpolation between a and b.
//!     for (int i = 0; i < 3; i++) {
//!         c[i].xyz = hlslpp::lerp(a[i].xyz, b[i].xyz, t);
//!     }
//!
//!     return c;
//! }
//!
//! // lines 177-199
//! hlslpp::float4x4 lerpMatrixComponents(const hlslpp::float4x4 &a, const hlslpp::float4x4 &b, bool linear, bool angular, bool perspective, float t) {
//!     hlslpp::float4x4 ret;
//!     // Start by either component-wise lerping the top left 3x3 if rotation is enabled or directly copying it otherwise.
//!     // This leaves the last row and last column as a copy of b's in either case.
//!     if (angular) {
//!         ret = lerpMatrix3x3(a, b, t);
//!     }
//!     else {
//!         ret = b;
//!     }
//!     // Next, lerp the translation component of the last row if enabled, otherwise leave it intact from the initial copy step.
//!     if (linear) {
//!         ret[3].xyz = lerp(a[3].xyz, b[3].xyz, t);
//!     }
//!     // Finally, do the same for the last column if perspective is enabled.
//!     if (perspective) {
//!         ret[0].w = lerp(a[0].w, b[0].w, t);
//!         ret[1].w = lerp(a[1].w, b[1].w, t);
//!         ret[2].w = lerp(a[2].w, b[2].w, t);
//!         ret[3].w = lerp(a[3].w, b[3].w, t);
//!     }
//!     return ret;
//! }
//! ```
//!
//! **Reuse, not new type.** This module reuses
//! [`fn64_render_ir::{Mat4, Vec4}`](fn64_render_ir) directly, matching
//! `rt64_math.rs`'s established convention exactly: `Mat4` is "a
//! backend-neutral **row-major** 4x4 float matrix, matching HLSL
//! `float4x4`" with `rows[i]` = row `i`, `rows[i].x/y/z/w` = that row's
//! four columns (`rsp_math.rs:78-84`), so an HLSL `m[i][j]` read becomes
//! `m.rows[i].{x,y,z,w}` for `j = 0..3`. `extract3x3` and
//! `rotation_from_3x3` reuse the sibling `rt64_math` module's already-`pub`
//! `Mat3` type (a local row-major 3x3 type built from
//! `fn64_render_ir::Vec3`, `rt64_math.rs:143-145`) rather than inventing a
//! second 3x3 type, since `rt64_math.rs` already established `Mat3` for
//! this exact upper-left-3x3 shape (`trace_from_3x3`) and this module is
//! that same source file's continuation. This is a read-only, same-crate
//! `crate::rt64_math::Mat3` reference -- it does not edit `rt64_math.rs`
//! (excluded from this ticket's edit set) and does not `pub use` it either;
//! `rt64_math_matrix` stays reachable only via `crate::rt64_math_matrix::*`,
//! same as every other unwired characterization module in this crate.
//!
//! ## Admitted domain / unpinned HLSL semantics (bound, not invented)
//!
//! - **`hlslpp::normalize` in `rotation_from_3x3`**: `hlslpp` is an
//!   unpopulated submodule in every checkout available to this program
//!   (`rsp_math.rs:21-23`, `rt64_math.rs:95-101` document this same
//!   constraint for `hlslpp::any`/`isnan`) -- there is no vendored source to
//!   inspect for a possible platform-dependent divergence. This port
//!   assumes the conventional, universal `normalize(v) = v / length(v)`
//!   definition (an unguarded division -- see below), matching every known
//!   HLSL/SIMD-math-library convention, stated here as an assumption rather
//!   than a verified fact.
//! - **`rotation_from_3x3` at a zero-length row** (degenerate/non-invertible
//!   3x3 input): `normalize`'s unguarded `v / length(v)` yields component-wise
//!   `0.0 / 0.0 = NaN` for that row. This port lets plain IEEE-754 division
//!   propagate, matching `rt64_math.rs::barycentric_coordinates`'s and
//!   `depth_strict_less.rs`'s precedent of preserving unguarded upstream
//!   arithmetic rather than inventing a guard the source does not have.
//! - **`hlslpp::dot(hlslpp::abs(a[i]-b[i]), 1.0f)` in `matrix_difference`**:
//!   `dot` of a `float4` with a scalar literal broadcasts the scalar to
//!   `float4(1,1,1,1)` per HLSL's standard scalar-to-vector broadcast rule
//!   (an unpinned but conventional HLSL/hlslpp semantic, same
//!   unpopulated-submodule caveat as above), making `dot(v, 1.0f)` the sum
//!   of `v`'s four components. This port implements that sum directly
//!   (`v.x + v.y + v.z + v.w`) rather than route through a generic `dot`
//!   helper, since no `Vec4::dot` exists in `fn64_render_ir` (only
//!   `Vec3::dot`) and adding one for this single caller would be an
//!   unrequested `fn64-render-ir` API-surface widening outside this
//!   module's exclusive-paths boundary.
//! - **`matrix_difference`'s summation order**: preserved exactly as the
//!   source's row-major, left-to-right element order (`i=0..4`, each row's
//!   `x,y,z,w` in that order) -- float addition is not associative, so this
//!   port does not reassociate, reorder, or use a different reduction
//!   (e.g. pairwise/Kahan summation) than the source's straight linear
//!   accumulation.
//! - **`hlslpp::abs` on `f32`**: assumed to be plain `f32::abs()`
//!   (magnitude, sign-independent; `abs(-0.0) == 0.0`, `abs(NaN)` is
//!   `NaN` with an unspecified sign bit per IEEE-754) -- conventional, not
//!   contradicted by any hlslpp source citation in this repo, but likewise
//!   not a verified read.
//! - **`lerp_matrix`/`lerp_matrix3x3`/`lerp_matrix_components`'s lerp
//!   formula**: `hlslpp::lerp(a, b, t)` and the source's own bare `lerp(a,
//!   b, t)` calls (unqualified in `lerpMatrixComponents`, resolved via
//!   using-directive/ADL to the same `hlslpp::lerp`) are assumed to be the
//!   conventional `a + t * (b - a)` formula, matching HLSL's `lerp`
//!   intrinsic contract and `rsp_math.rs`'s established assumption-with-
//!   citation style for unpopulated-submodule hlslpp calls. This is **not**
//!   the same formula as `a*(1-t) + b*t` -- the two are algebraically equal
//!   in real-number arithmetic but differ in floating-point rounding and in
//!   `NaN`/infinity propagation at extreme `t`, so this port picks the
//!   `a + t*(b-a)` form specifically (HLSL's own documented definition) and
//!   does not use the other. At `t=0` and *finite* `a`/`b`, this yields
//!   exactly `a` (`a + 0*(b-a) = a`, since `0 * finite = 0`) -- but the
//!   formula is not special-cased to `return a` at `t=0`, so two corner
//!   cases genuinely diverge from "just `a`": (1) if `b-a` is itself
//!   infinite (e.g. `a=+inf, b=0.0`), `0 * (b-a) = 0 * -inf = NaN` (`0 *
//!   infinity` is `NaN` per IEEE-754), so the whole sum is `NaN`, not `a`;
//!   (2) if `a` is `-0.0` and `b` is `0.0`, `b-a = 0.0-(-0.0) = +0.0`,
//!   `0*+0.0 = +0.0`, and `-0.0 + +0.0 = +0.0` in round-to-nearest mode --
//!   so the result's sign bit flips from `a`'s `-0.0` to `+0.0`, not
//!   "exactly `a`" in the bit-identical sense. Both are exercised as
//!   characterization tests below, not asserted from this doc comment
//!   alone. At `t=1`, `a + 1*(b-a) = a + b - a`, which is `b` only when
//!   `a`'s cancellation is exact in `f32` -- for most inputs this is
//!   bit-identical to `b`, but is not a `t=1`-special-cased identity in the
//!   source and is not special-cased here either.
//! - **`lerp_matrix`/`lerp_matrix3x3` at out-of-`[0,1]` `t`**: no clamp in
//!   the source (`hlslpp::lerp`/HLSL `lerp` never clamps `t`) -- this port
//!   does not add one; `t<0` or `t>1` extrapolates linearly past `a`/`b` as
//!   plain arithmetic dictates.
//! - **`lerp_matrix_components`'s four independent boolean gates**: ported
//!   exactly as four independent `if`s over the same `ret` accumulator, in
//!   the source's exact order (angular-or-copy first, then linear, then
//!   perspective) -- not restructured into a single expression, since the
//!   gates read from `ret` (already possibly mutated by an earlier gate in
//!   the linear/perspective steps only through `a`/`b`, not `ret`, so gate
//!   order does not change the *result*, but this port preserves the
//!   source's exact statement order rather than relying on that
//!   independence argument to justify reordering).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, camera/view-matrix wiring, or RT64 visual/pixel/silicon
//! parity or performance claim. This module is not called from anywhere
//! yet (dead-code warnings on the unused public surface are expected and
//! correct, matching `rt64_math.rs`'s and every other characterization-
//! first module's precedent). Does not port any other function from
//! `rt64_math.h`/`.cpp` beyond the six named above -- in particular, the
//! quaternion cluster `decomposeMatrix`, `recomposeMatrix`,
//! `DecomposedTransform` (and its constructor), and `lerpTransforms`, plus
//! their `vecCombine`/`vecScale` helpers, are a separate ticket (owned by
//! another executor) and are deliberately NOT ported here: they need a
//! `hlslpp::quaternion`-equivalent type and a 4x4 matrix inverse/determinant
//! that this module does not add. `matrixScale`, `matrixTranslation`,
//! `matrixRotationX/Y/Z`, `matrixDecomposeViewProj`, and `pseudoRandom`
//! remain out of scope exactly as `rt64_math.rs` already stated (unchanged
//! by this module, which does not edit `rt64_math.rs`).

use crate::rt64_math::Mat3;
use fn64_render_ir::Vec3;

/// `extract3x3`: copies the upper-left 3x3 of a row-major `float4x4`,
/// element-for-element, into the sibling `rt64_math` module's `Mat3` (see
/// module doc "Reuse, not new type").
pub fn extract_3x3(m: fn64_render_ir::Mat4) -> Mat3 {
    Mat3 {
        rows: [
            Vec3::new(m.rows[0].x, m.rows[0].y, m.rows[0].z),
            Vec3::new(m.rows[1].x, m.rows[1].y, m.rows[1].z),
            Vec3::new(m.rows[2].x, m.rows[2].y, m.rows[2].z),
        ],
    }
}

/// `rotationFrom3x3`: normalizes each row of a 3x3 matrix independently.
/// Unguarded division (`normalize(v) = v / length(v)`); a zero-length row
/// yields `NaN` lanes, propagated per IEEE-754 (see module doc "Admitted
/// domain").
pub fn rotation_from_3x3(m: Mat3) -> Mat3 {
    Mat3 {
        rows: [
            normalize_vec3(m.rows[0]),
            normalize_vec3(m.rows[1]),
            normalize_vec3(m.rows[2]),
        ],
    }
}

fn normalize_vec3(v: Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    Vec3::new(v.x / len, v.y / len, v.z / len)
}

/// `matrixDifference`: sum over all 16 elements of `|a[i][j] - b[i][j]|`,
/// accumulated row-major, left-to-right, in the source's exact order (see
/// module doc "Admitted domain" -- float addition is not associative).
pub fn matrix_difference(a: fn64_render_ir::Mat4, b: fn64_render_ir::Mat4) -> f32 {
    let mut difference = 0.0f32;
    for i in 0..4 {
        let ar = a.rows[i];
        let br = b.rows[i];
        let d = fn64_render_ir::Vec4::new(
            (ar.x - br.x).abs(),
            (ar.y - br.y).abs(),
            (ar.z - br.z).abs(),
            (ar.w - br.w).abs(),
        );
        difference += d.x + d.y + d.z + d.w;
    }
    difference
}

/// `lerpMatrix`: component-wise linear interpolation of every row,
/// `a + t*(b-a)` per element (see module doc "Admitted domain" for the
/// exact formula and its `t=0`/`t=1`/out-of-range behavior).
pub fn lerp_matrix(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut c = b;
    for i in 0..4 {
        c.rows[i] = lerp_vec4(a.rows[i], b.rows[i], t);
    }
    c
}

/// `lerpMatrix3x3`: lerps only the upper-left 3x3 (rows 0-2's `x,y,z`);
/// row 3 and every row's `w` column are copied verbatim from `b`.
pub fn lerp_matrix_3x3(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut c = b;
    for i in 0..3 {
        let av = a.rows[i];
        let bv = b.rows[i];
        c.rows[i] = fn64_render_ir::Vec4::new(
            lerp_f32(av.x, bv.x, t),
            lerp_f32(av.y, bv.y, t),
            lerp_f32(av.z, bv.z, t),
            bv.w,
        );
    }
    c
}

/// `lerpMatrixComponents`: gated composition of `lerp_matrix_3x3` (or a
/// plain copy of `b`) for the rotation/scale block, an independent lerp of
/// row 3's `xyz` translation, and an independent lerp of every row's `w`
/// perspective column -- each gate applied only if its flag is set,
/// otherwise leaving that part as `b`'s value from the initial copy step
/// (see module doc "Admitted domain" for the exact gate order preserved).
#[allow(clippy::too_many_arguments)]
pub fn lerp_matrix_components(
    a: fn64_render_ir::Mat4,
    b: fn64_render_ir::Mat4,
    linear: bool,
    angular: bool,
    perspective: bool,
    t: f32,
) -> fn64_render_ir::Mat4 {
    let mut ret = if angular { lerp_matrix_3x3(a, b, t) } else { b };

    if linear {
        let av = a.rows[3];
        let bv = b.rows[3];
        ret.rows[3] = fn64_render_ir::Vec4::new(
            lerp_f32(av.x, bv.x, t),
            lerp_f32(av.y, bv.y, t),
            lerp_f32(av.z, bv.z, t),
            ret.rows[3].w,
        );
    }

    if perspective {
        for i in 0..4 {
            let aw = a.rows[i].w;
            let bw = b.rows[i].w;
            ret.rows[i].w = lerp_f32(aw, bw, t);
        }
    }

    ret
}

/// `hlslpp::lerp(a, b, t)` for a single scalar component: `a + t*(b-a)`
/// (see module doc "Admitted domain" for why this exact formula, not
/// `a*(1-t)+b*t`, was chosen).
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// `hlslpp::lerp(a, b, t)` applied component-wise to a `float4`.
fn lerp_vec4(a: fn64_render_ir::Vec4, b: fn64_render_ir::Vec4, t: f32) -> fn64_render_ir::Vec4 {
    fn64_render_ir::Vec4::new(
        lerp_f32(a.x, b.x, t),
        lerp_f32(a.y, b.y, t),
        lerp_f32(a.z, b.z, t),
        lerp_f32(a.w, b.w, t),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{Mat4, Vec4};

    fn identity() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 1.0),
        ])
    }

    fn zeros() -> Mat4 {
        Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4])
    }

    fn arbitrary_a() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(1.0, 2.0, 3.0, 4.0),
            Vec4::new(5.0, 6.0, 7.0, 8.0),
            Vec4::new(9.0, 10.0, 11.0, 12.0),
            Vec4::new(13.0, 14.0, 15.0, 16.0),
        ])
    }

    fn arbitrary_b() -> Mat4 {
        Mat4::from_rows([
            Vec4::new(-1.0, 0.0, 2.0, 4.0),
            Vec4::new(10.0, -6.0, 7.5, 0.0),
            Vec4::new(9.0, 20.0, -11.0, 12.5),
            Vec4::new(-13.0, 14.0, 0.0, 100.0),
        ])
    }

    // --- extract_3x3 ---

    #[test]
    fn extract_3x3_identity_is_3x3_identity() {
        let e = extract_3x3(identity());
        assert_eq!(e.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(e.rows[1], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(e.rows[2], Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn extract_3x3_drops_last_row_and_column() {
        let m = arbitrary_a();
        let e = extract_3x3(m);
        assert_eq!(e.rows[0], Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(e.rows[1], Vec3::new(5.0, 6.0, 7.0));
        assert_eq!(e.rows[2], Vec3::new(9.0, 10.0, 11.0));
        // Row 3 and column 3 (the 4.0/8.0/12.0/13..16 values) do not appear.
    }

    #[test]
    fn extract_3x3_zero_matrix_is_zero() {
        let e = extract_3x3(zeros());
        for row in e.rows {
            assert_eq!(row, Vec3::new(0.0, 0.0, 0.0));
        }
    }

    #[test]
    fn extract_3x3_passes_through_nan_and_infinity_unmodified() {
        let m = Mat4::from_rows([
            Vec4::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 999.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, 0.0),
        ]);
        let e = extract_3x3(m);
        assert!(e.rows[0].x.is_nan());
        assert_eq!(e.rows[0].y, f32::INFINITY);
        assert_eq!(e.rows[0].z, f32::NEG_INFINITY);
    }

    // --- rotation_from_3x3 ---

    #[test]
    fn rotation_from_3x3_identity_rows_are_already_unit_length() {
        let m = Mat3 {
            rows: [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert_eq!(r.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[1], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(r.rows[2], Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn rotation_from_3x3_scales_each_row_to_unit_length() {
        let m = Mat3 {
            rows: [
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::new(3.0, 4.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert_eq!(r.rows[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[1], Vec3::new(0.0, 0.0, 1.0));
        assert!((r.rows[2].x - 0.6).abs() < 1e-6);
        assert!((r.rows[2].y - 0.8).abs() < 1e-6);
        assert_eq!(r.rows[2].z, 0.0);
    }

    #[test]
    fn rotation_from_3x3_negative_components_preserve_sign_after_normalize() {
        let m = Mat3 {
            rows: [
                Vec3::new(-3.0, 4.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!((r.rows[0].x - (-0.6)).abs() < 1e-6);
        assert!((r.rows[0].y - 0.8).abs() < 1e-6);
    }

    #[test]
    fn rotation_from_3x3_zero_row_yields_nan_unguarded() {
        let m = Mat3 {
            rows: [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!(r.rows[0].x.is_nan());
        assert!(r.rows[0].y.is_nan());
        assert!(r.rows[0].z.is_nan());
        // Other rows are unaffected.
        assert_eq!(r.rows[1], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r.rows[2], Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn rotation_from_3x3_infinite_component_yields_nan_via_inf_over_inf() {
        // length(inf,0,0) = sqrt(inf^2) = inf; inf/inf = NaN for the x
        // lane, 0/inf = 0.0 for the others -- unguarded, no special case.
        let m = Mat3 {
            rows: [
                Vec3::new(f32::INFINITY, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
        };
        let r = rotation_from_3x3(m);
        assert!(r.rows[0].x.is_nan());
        assert_eq!(r.rows[0].y, 0.0);
        assert_eq!(r.rows[0].z, 0.0);
    }

    // --- matrix_difference ---

    #[test]
    fn matrix_difference_identical_matrices_is_zero() {
        let m = arbitrary_a();
        assert_eq!(matrix_difference(m, m), 0.0);
    }

    #[test]
    fn matrix_difference_zero_vs_identity_sums_the_four_diagonal_ones() {
        assert_eq!(matrix_difference(zeros(), identity()), 4.0);
    }

    #[test]
    fn matrix_difference_is_symmetric_under_swap() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        assert_eq!(matrix_difference(a, b), matrix_difference(b, a));
    }

    #[test]
    fn matrix_difference_exact_value_hand_computed() {
        // Row-by-row |a-b| sums, hand-computed independently of the port:
        // row0: |1-(-1)|+|2-0|+|3-2|+|4-4| = 2+2+1+0 = 5
        // row1: |5-10|+|6-(-6)|+|7-7.5|+|8-0| = 5+12+0.5+8 = 25.5
        // row2: |9-9|+|10-20|+|11-(-11)|+|12-12.5| = 0+10+22+0.5 = 32.5
        // row3: |13-(-13)|+|14-14|+|15-0|+|16-100| = 26+0+15+84 = 125
        // total = 5 + 25.5 + 32.5 + 125 = 188.0
        let d = matrix_difference(arbitrary_a(), arbitrary_b());
        assert!((d - 188.0).abs() < 1e-3, "d={d}");
    }

    #[test]
    fn matrix_difference_negative_zero_abs_is_zero() {
        let a = Mat4::from_rows([Vec4::new(-0.0, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        assert_eq!(matrix_difference(a, b), 0.0);
    }

    #[test]
    fn matrix_difference_nan_propagates() {
        let mut a = zeros();
        a.rows[0].x = f32::NAN;
        assert!(matrix_difference(a, zeros()).is_nan());
    }

    #[test]
    fn matrix_difference_infinite_element_yields_infinite_sum() {
        let mut a = zeros();
        a.rows[0].x = f32::INFINITY;
        assert_eq!(matrix_difference(a, zeros()), f32::INFINITY);
    }

    #[test]
    fn matrix_difference_opposite_infinities_still_yields_infinite_abs_diff() {
        // |(+inf) - (-inf)| = |+inf| = +inf, not NaN: unlike same-sign
        // inf-inf, opposite-sign infinities subtract to a well-defined
        // infinity here.
        let mut a = zeros();
        a.rows[0].x = f32::INFINITY;
        let mut b = zeros();
        b.rows[0].x = f32::NEG_INFINITY;
        assert_eq!(matrix_difference(a, b), f32::INFINITY);
    }

    // --- lerp_matrix ---

    #[test]
    fn lerp_matrix_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        assert_eq!(lerp_matrix(a, b, 0.0), a);
    }

    #[test]
    fn lerp_matrix_at_t_one_is_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix(a, b, 1.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let br = b.rows[i];
            assert!((rr.x - br.x).abs() < 1e-4);
            assert!((rr.y - br.y).abs() < 1e-4);
            assert!((rr.z - br.z).abs() < 1e-4);
            assert!((rr.w - br.w).abs() < 1e-4);
        }
    }

    #[test]
    fn lerp_matrix_at_t_half_is_the_midpoint() {
        let a = identity();
        let b = zeros();
        let r = lerp_matrix(a, b, 0.5);
        // identity[0][0]=1 -> lerp(1,0,0.5) = 0.5.
        assert_eq!(r.rows[0].x, 0.5);
        assert_eq!(r.rows[1].y, 0.5);
        assert_eq!(r.rows[2].z, 0.5);
        assert_eq!(r.rows[3].w, 0.5);
        // Off-diagonal: lerp(0,0,0.5) = 0.
        assert_eq!(r.rows[0].y, 0.0);
    }

    #[test]
    fn lerp_matrix_t_outside_unit_interval_extrapolates() {
        let a = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(1.0, 0.0, 0.0, 0.0); 4]);
        // lerp(0, 1, 2.0) = 0 + 2*(1-0) = 2.0 -- past b.
        let r = lerp_matrix(a, b, 2.0);
        assert_eq!(r.rows[0].x, 2.0);
        // lerp(0, 1, -1.0) = 0 + (-1)*(1-0) = -1.0 -- before a.
        let r2 = lerp_matrix(a, b, -1.0);
        assert_eq!(r2.rows[0].x, -1.0);
    }

    #[test]
    fn lerp_matrix_exact_value_hand_computed_at_quarter() {
        // a=2.0, b=10.0, t=0.25: 2.0 + 0.25*(10.0-2.0) = 2.0 + 2.0 = 4.0.
        let a = Mat4::from_rows([Vec4::new(2.0, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(10.0, 0.0, 0.0, 0.0); 4]);
        let r = lerp_matrix(a, b, 0.25);
        assert_eq!(r.rows[0].x, 4.0);
    }

    #[test]
    fn lerp_matrix_nan_in_a_propagates_even_at_t_zero() {
        // a + t*(b-a) with t=0 still multiplies 0 * (b-a); if a itself is
        // NaN the whole expression is NaN even though "conceptually" t=0
        // should just return a. This is the literal formula's behavior,
        // not a bug (see module doc).
        let mut a = zeros();
        a.rows[0].x = f32::NAN;
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_nan_t_propagates_to_every_lerped_element() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix(a, b, f32::NAN);
        for row in r.rows {
            assert!(row.x.is_nan());
            assert!(row.y.is_nan());
            assert!(row.z.is_nan());
            assert!(row.w.is_nan());
        }
    }

    #[test]
    fn lerp_matrix_infinite_a_at_t_zero_yields_nan_not_a_shortcut() {
        // a=+inf, b=0.0, t=0.0: a + t*(b-a) = inf + 0*(0-inf) =
        // inf + 0*(-inf) = inf + NaN = NaN (0 * infinity is NaN per
        // IEEE-754). The literal `a + t*(b-a)` formula does NOT
        // special-case t=0 to just return `a` -- this is the formula's
        // genuine behavior at this input, not a bug (see module doc).
        let a = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_infinity_minus_infinity_yields_nan() {
        // a=+inf, b=+inf, t=0.5: a + 0.5*(b-a) = inf + 0.5*(inf-inf) =
        // inf + 0.5*NaN = NaN. Not a special-cased "same value" shortcut.
        let a = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let b = Mat4::from_rows([Vec4::new(f32::INFINITY, 0.0, 0.0, 0.0); 4]);
        let r = lerp_matrix(a, b, 0.5);
        assert!(r.rows[0].x.is_nan());
    }

    #[test]
    fn lerp_matrix_negative_zero_at_t_zero_flips_to_positive_zero() {
        // a=-0.0, b=0.0, t=0.0: b-a = 0.0-(-0.0) = +0.0 (IEEE-754: this
        // subtraction is exact and positive), t*(b-a) = 0*+0.0 = +0.0,
        // a+0.0 = -0.0+0.0 = +0.0 (IEEE-754 default rounding: x+0.0 with
        // x=-0.0 and the addend +0.0 produces +0.0, not -0.0 -- the sign
        // of a zero result from adding two zeros of opposite sign is
        // always +0.0 in round-to-nearest mode). So the formula does NOT
        // preserve a's negative-zero sign at t=0 -- another instance of
        // `a + t*(b-a)` not degenerating to a bare copy of `a` (see module
        // doc).
        let a = Mat4::from_rows([Vec4::new(-0.0, 0.0, 0.0, 0.0); 4]);
        let b = zeros();
        let r = lerp_matrix(a, b, 0.0);
        assert_eq!(r.rows[0].x, 0.0);
        assert!(r.rows[0].x.is_sign_positive());
    }

    // --- lerp_matrix_3x3 ---

    #[test]
    fn lerp_matrix_3x3_row_3_always_copies_b_regardless_of_t() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[3], b.rows[3]);
        let r2 = lerp_matrix_3x3(a, b, 1.0);
        assert_eq!(r2.rows[3], b.rows[3]);
    }

    #[test]
    fn lerp_matrix_3x3_w_column_always_copies_b_regardless_of_t() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[0].w, b.rows[0].w);
        assert_eq!(r.rows[1].w, b.rows[1].w);
        assert_eq!(r.rows[2].w, b.rows[2].w);
    }

    #[test]
    fn lerp_matrix_3x3_upper_left_lerps_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.0);
        assert_eq!(r.rows[0].x, a.rows[0].x);
        assert_eq!(r.rows[0].y, a.rows[0].y);
        assert_eq!(r.rows[0].z, a.rows[0].z);
        assert_eq!(r.rows[2].z, a.rows[2].z);
    }

    #[test]
    fn lerp_matrix_3x3_upper_left_lerps_at_t_half() {
        let a = Mat4::from_rows([Vec4::new(0.0, 0.0, 0.0, 99.0); 4]);
        let b = Mat4::from_rows([Vec4::new(10.0, 10.0, 10.0, 5.0); 4]);
        let r = lerp_matrix_3x3(a, b, 0.5);
        assert_eq!(r.rows[0].x, 5.0);
        assert_eq!(r.rows[0].y, 5.0);
        assert_eq!(r.rows[0].z, 5.0);
        // w column is b's regardless: 5.0.
        assert_eq!(r.rows[0].w, 5.0);
        // Row 3 is fully b's: (10,10,10,5).
        assert_eq!(r.rows[3], Vec4::new(10.0, 10.0, 10.0, 5.0));
    }

    #[test]
    fn lerp_matrix_3x3_nan_in_upper_left_does_not_contaminate_row_3_or_w() {
        let mut a = arbitrary_a();
        a.rows[0].x = f32::NAN;
        let b = arbitrary_b();
        let r = lerp_matrix_3x3(a, b, 0.5);
        assert!(r.rows[0].x.is_nan());
        // row 3 and the w column are plain copies of b, untouched by a's NaN.
        assert_eq!(r.rows[3], b.rows[3]);
        assert_eq!(r.rows[1].w, b.rows[1].w);
    }

    // --- lerp_matrix_components ---

    #[test]
    fn lerp_matrix_components_all_flags_false_is_exactly_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, false, false, 0.5);
        assert_eq!(r, b);
    }

    #[test]
    fn lerp_matrix_components_angular_only_matches_lerp_matrix_3x3() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, false, 0.5);
        let expected = lerp_matrix_3x3(a, b, 0.5);
        assert_eq!(r, expected);
    }

    #[test]
    fn lerp_matrix_components_linear_only_lerps_row_3_xyz_leaves_rest_as_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, false, false, 0.5);
        // rows 0-2 untouched (angular=false -> ret starts as b).
        assert_eq!(r.rows[0], b.rows[0]);
        assert_eq!(r.rows[1], b.rows[1]);
        assert_eq!(r.rows[2], b.rows[2]);
        // row 3's xyz is lerped; w stays b's (only linear touches xyz).
        let expected_x = a.rows[3].x + 0.5 * (b.rows[3].x - a.rows[3].x);
        assert!((r.rows[3].x - expected_x).abs() < 1e-4);
        assert_eq!(r.rows[3].w, b.rows[3].w);
    }

    #[test]
    fn lerp_matrix_components_perspective_only_lerps_w_column_leaves_rest_as_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, false, true, 0.5);
        // xyz of every row untouched (still b's, since angular=false and
        // linear=false leave rows 0-2 and row 3's xyz as b's).
        assert_eq!(r.rows[0].x, b.rows[0].x);
        assert_eq!(r.rows[3].x, b.rows[3].x);
        // w column of every row is lerped.
        for i in 0..4 {
            let expected_w = a.rows[i].w + 0.5 * (b.rows[i].w - a.rows[i].w);
            assert!((r.rows[i].w - expected_w).abs() < 1e-4, "row {i}");
        }
    }

    #[test]
    fn lerp_matrix_components_all_flags_true_at_t_zero_is_a() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, true, true, 0.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let ar = a.rows[i];
            assert!((rr.x - ar.x).abs() < 1e-4, "row {i} x");
            assert!((rr.y - ar.y).abs() < 1e-4, "row {i} y");
            assert!((rr.z - ar.z).abs() < 1e-4, "row {i} z");
            assert!((rr.w - ar.w).abs() < 1e-4, "row {i} w");
        }
    }

    #[test]
    fn lerp_matrix_components_all_flags_true_at_t_one_is_b() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, true, true, 1.0);
        for i in 0..4 {
            let rr = r.rows[i];
            let br = b.rows[i];
            assert!((rr.x - br.x).abs() < 1e-4, "row {i} x");
            assert!((rr.y - br.y).abs() < 1e-4, "row {i} y");
            assert!((rr.z - br.z).abs() < 1e-4, "row {i} z");
            assert!((rr.w - br.w).abs() < 1e-4, "row {i} w");
        }
    }

    #[test]
    fn lerp_matrix_components_angular_and_perspective_together_leave_row3_xyz_as_b() {
        // angular lerps rows 0-2's xyz; perspective lerps every row's w;
        // row 3's xyz is untouched by either (only `linear` touches it),
        // so it must remain exactly b's.
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, true, 0.5);
        assert_eq!(r.rows[3].x, b.rows[3].x);
        assert_eq!(r.rows[3].y, b.rows[3].y);
        assert_eq!(r.rows[3].z, b.rows[3].z);
    }

    #[test]
    fn lerp_matrix_components_linear_and_perspective_together_leave_rows_0_2_xyz_as_b() {
        // linear lerps row 3's xyz; perspective lerps every row's w;
        // rows 0-2's xyz are untouched by either (only `angular` touches
        // them), so they must remain exactly b's.
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, true, false, true, 0.5);
        assert_eq!(r.rows[0].x, b.rows[0].x);
        assert_eq!(r.rows[1].y, b.rows[1].y);
        assert_eq!(r.rows[2].z, b.rows[2].z);
        // But row 3's xyz and every row's w ARE lerped.
        let expected_row3_x = a.rows[3].x + 0.5 * (b.rows[3].x - a.rows[3].x);
        assert!((r.rows[3].x - expected_row3_x).abs() < 1e-4);
        let expected_w0 = a.rows[0].w + 0.5 * (b.rows[0].w - a.rows[0].w);
        assert!((r.rows[0].w - expected_w0).abs() < 1e-4);
    }

    #[test]
    fn lerp_matrix_components_nan_t_propagates_only_through_active_gates() {
        let a = arbitrary_a();
        let b = arbitrary_b();
        let r = lerp_matrix_components(a, b, false, true, false, f32::NAN);
        // angular gate lerped -> NaN.
        assert!(r.rows[0].x.is_nan());
        // linear/perspective gates inactive -> untouched b values, not NaN.
        assert_eq!(r.rows[3].x, b.rows[3].x);
        assert_eq!(r.rows[0].w, b.rows[0].w);
    }
}
