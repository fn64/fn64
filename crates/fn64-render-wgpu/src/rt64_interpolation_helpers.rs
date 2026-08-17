//! Literal port of two small pure-computation fragments carved out of
//! RT64's `LookAtProcessor`/`ProjectionProcessor`: the RSPLookAt x/y lerp
//! formula and `adjustProjectionMatrix`'s aspect-ratio column scale, a
//! literal port of the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/render/rt64_look_at_processor.cpp` (whole-file SHA-256,
//!   `478bc5254640b426884d8c634399dbbd6efd6fd92bd4a215e89f057f61a55ea5`, 66
//!   lines -- matching `docs/rt64-port-inventory.json`'s `sources.port.sha256`
//!   for that path, confirmed independently here by `shasum -a 256` against
//!   the pinned port-commit checkout). The lerp is lines 40-41.
//! - `src/render/rt64_projection_processor.cpp` (whole-file SHA-256,
//!   `cf04cb4da1bc39ab60cb24c92c0668385632ab7c6894157e6857c93252a39de9`, 163
//!   lines -- matching the same inventory field, confirmed the same way).
//!   `adjustProjectionMatrix` is lines 11-16.
//!
//! `docs/rt64-port-inventory.json` does not yet record either path's
//! `ported_as` as pointing at this module (both currently list `"ported_as":
//! []`) -- `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! drift for that until a follow-up regenerates the inventory to add this
//! module; this module's own writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here.
//!
//! ```text
//! // rt64_look_at_processor.cpp:38-42 (inside LookAtProcessor::process's inner loop)
//! const interop::RSPLookAt &curLookAt = curLookAts[l];
//! interop::RSPLookAt &lerpLookAt = lerpRspLookAts[l];
//! lerpLookAt.x = curLookAt.x - lookAtMap.deltaX * (1.0f - p.curFrameWeight);
//! lerpLookAt.y = curLookAt.y - lookAtMap.deltaY * (1.0f - p.curFrameWeight);
//!
//! // rt64_projection_processor.cpp:11-16
//! inline void adjustProjectionMatrix(interop::float4x4 &matrix, const float aspectRatioScale) {
//!     matrix[0][0] *= aspectRatioScale;
//!     matrix[1][0] *= aspectRatioScale;
//!     matrix[2][0] *= aspectRatioScale;
//!     matrix[3][0] *= aspectRatioScale;
//! }
//! ```
//!
//! **Reuse, not new type.** `RSPLookAt::x`/`y` are `hlslpp::float3`
//! (`src/shared/rt64_rsp_lookat.h:16-19`, already ported as
//! `fn64_render_ir::Vec3` in `rt64_common.rs`'s sibling `rsp_math.rs`), and
//! `adjustProjectionMatrix`'s `interop::float4x4` is the same `float4x4`
//! `rt64_math_matrix.rs` already represents as `fn64_render_ir::Mat4` (row-
//! major, `rows[i].x` = HLSL `m[i][0]`, per that module's established "`m[i][j]`
//! read becomes `m.rows[i].{x,y,z,w}`" convention). This module reuses both
//! types directly -- no new vector or matrix type, and no edit to
//! `fn64-render-ir` or `rt64_math_matrix.rs`.
//!
//! ## Admitted domain
//!
//! - **The LookAt lerp is `cur - delta * (1 - weight)`, literally -- NOT the
//!   canonical HLSL `lerp(x,y,s) = x + s*(y-x)` form, and NOT rewritten to
//!   one.** This hazard applies to `hlslpp::lerp` call sites in this port
//!   program generally, but this particular fragment is a hand-written
//!   expression in the source, not a call to `hlslpp::lerp` at all --
//!   confirmed by reading the exact line (`lerpLookAt.x = curLookAt.x -
//!   lookAtMap.deltaX * (1.0f - p.curFrameWeight)`), so there is no `lerp`
//!   intrinsic here to convert. [`look_at_lerp_component`] below preserves
//!   this exact operand order and subtraction/multiplication shape:
//!   `cur - delta * (1.0 - weight)`, never reassociated into `cur -
//!   delta + delta * weight` or any other algebraically-equal-looking
//!   rewrite (those are not bit-identical in `f32`). At `weight = 1.0`,
//!   `1.0 - 1.0 = 0.0` exactly (no cancellation error, since `1.0 - 1.0` is
//!   always exactly representable), so the result is exactly `cur` --
//!   pinned by [`look_at_lerp_component_weight_one_returns_cur_unchanged`]
//!   below with a hand-computed value, not a captured one. At `weight =
//!   0.0`, the result is `cur - delta` exactly, pinned by
//!   [`look_at_lerp_component_weight_zero_returns_cur_minus_delta`].
//! - **The lerp is applied identically and independently to `x` and `y`
//!   (`RSPLookAt`'s two `float3` fields), and each `float3` op is itself
//!   three independent per-component scalar operations (hlslpp `float3 -
//!   float3 * float` is elementwise, no cross-component interaction).**
//!   [`look_at_lerp_vec3`] below applies [`look_at_lerp_component`]
//!   component-wise to `Vec3`, matching that elementwise contract exactly
//!   -- it does not, for instance, compute a shared scalar delta magnitude
//!   or otherwise couple the three components.
//! - **`adjustProjectionMatrix` scales column 0 of every row (`matrix[r][0]`
//!   for `r` in `0..4`), i.e. `rows[r].x` in this crate's row-major `Mat4`
//!   convention -- not row 0 of every column, and not the whole matrix.**
//!   This is an easy transpose mistake since "row 0" and "column 0" sound
//!   similar; [`adjust_projection_matrix`] below scales exactly
//!   `rows[0].x, rows[1].x, rows[2].x, rows[3].x` and leaves every other
//!   component of all four rows untouched, pinned by
//!   [`adjust_projection_matrix_scales_only_column_zero_of_every_row`]
//!   below against a hand-built non-identity matrix where every element is
//!   distinct, so a transposed or partial application would be caught.
//! - **No divide-by-zero frontier in either fragment.** The LookAt lerp has
//!   no division at all. `adjustProjectionMatrix` has no division either
//!   (pure multiplication by `aspectRatioScale`); the division that
//!   produces `aspectRatioScale` itself
//!   (`projRatioScale = adjustAspectRatio ? (1.0f / p.aspectRatioScale) :
//!   1.0f`, `rt64_projection_processor.cpp:101`) is the caller's
//!   responsibility, not `adjustProjectionMatrix`'s, and is
//!   `ProjectionProcessor::processScene` RHI/state plumbing this ticket
//!   excludes (see "Nonclaims") -- so it is out of this module's scope, not
//!   silently dropped.
//! - **No private-helper visibility gap was hit.** Both fragments only need
//!   `fn64_render_ir::{Vec3, Mat4}`, both already `pub` on that crate's
//!   surface and already the established reuse target for this exact
//!   `float3`/`float4x4` shape elsewhere in this crate (`rt64_math_matrix.rs`).
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet; dead-code warnings on the unused public surface are
//! expected and correct), and no RT64 visual/pixel/silicon parity or
//! performance claim. Both source files are majority `BufferUploader`/RHI
//! upload plumbing over `WorkloadQueue`/`GameFrame`; deliberately not
//! ported from this cluster:
//!
//! - `LookAtProcessor`'s constructor/destructor, `setup` (allocates a
//!   `BufferUploader`), and `upload` (submits `BufferUploader::Upload`
//!   records) -- RHI plumbing.
//! - `LookAtProcessor::process`'s outer loop structure itself (iterating
//!   `p.curFrame->workloads`, indexing `WorkloadQueue`/`DrawData`,
//!   `GameFrameMap::LookAtMap::mapped`-gated skip, and the
//!   `lerpRspLookAts = curLookAts` bulk-copy that precedes the lerp) --
//!   this module ports only the two-line lerp body itself as a pure
//!   function of already-extracted scalars/vectors, not the surrounding
//!   iteration/indexing over engine state.
//! - `ProjectionProcessor`'s constructor/destructor, `setup`, `upload` --
//!   RHI plumbing, same as `LookAtProcessor`.
//! - `ProjectionProcessor::process`/`processScene` in their entirety: the
//!   per-workload copy of `viewTransforms`/`projTransforms`/
//!   `viewProjTransforms` into `mod*`/`prev*` fields, the aspect-mode
//!   branch (`G_EX_ASPECT_ADJUST`/`G_EX_ASPECT_AUTO`, `FixedRect`
//!   intersection against `RSPViewport::rect`, `coversWholeWidth`/
//!   `horizontalRatio` derivation), the debugger-camera override, the
//!   `1.0f / p.aspectRatioScale` division that produces `projRatioScale`,
//!   `RigidBody::lerp` calls (a distinct, separately-owned rigid-body lerp,
//!   not this ticket's LookAt lerp), the `lerpMatrix` calls (already ported
//!   in `rt64_math_matrix.rs`, reused there, not re-ported here), and the
//!   `hlslpp::mul(viewMatrix, projMatrix)` view-projection composition --
//!   all excluded as either RHI/state plumbing or already-ported-elsewhere
//!   functionality this ticket's named scope does not include.
//! - `RSPLookAt`'s and `interop::float4x4`'s full field/method surface
//!   beyond the two fields (`x`, `y`) and the one indexing shape
//!   (`matrix[r][0]`) each ported function actually touches.

/// `RSPLookAt::x`/`y`'s per-component lerp
/// (`rt64_look_at_processor.cpp:40-41`): `cur - delta * (1.0 - weight)`,
/// literally -- NOT the canonical HLSL `lerp(x,y,s) = x + s*(y-x)` form (see
/// module doc "Admitted domain": this is a hand-written expression, not an
/// `hlslpp::lerp` call).
pub fn look_at_lerp_component(cur: f32, delta: f32, weight: f32) -> f32 {
    cur - delta * (1.0 - weight)
}

/// Applies [`look_at_lerp_component`] independently to each of `x`/`y`/`z`,
/// matching hlslpp's elementwise `float3 - float3 * float` semantics (see
/// module doc "Admitted domain").
pub fn look_at_lerp_vec3(
    cur: fn64_render_ir::Vec3,
    delta: fn64_render_ir::Vec3,
    weight: f32,
) -> fn64_render_ir::Vec3 {
    fn64_render_ir::Vec3::new(
        look_at_lerp_component(cur.x, delta.x, weight),
        look_at_lerp_component(cur.y, delta.y, weight),
        look_at_lerp_component(cur.z, delta.z, weight),
    )
}

/// `adjustProjectionMatrix` (`rt64_projection_processor.cpp:11-16`): scales
/// column 0 of every row (`matrix[r][0] *= aspectRatioScale` for `r` in
/// `0..4`), i.e. `rows[r].x` in this crate's row-major `Mat4` convention.
/// Every other component of all four rows is left untouched (see module doc
/// "Admitted domain" for the transpose hazard this guards against).
pub fn adjust_projection_matrix(
    matrix: fn64_render_ir::Mat4,
    aspect_ratio_scale: f32,
) -> fn64_render_ir::Mat4 {
    let mut out = matrix;
    out.rows[0].x *= aspect_ratio_scale;
    out.rows[1].x *= aspect_ratio_scale;
    out.rows[2].x *= aspect_ratio_scale;
    out.rows[3].x *= aspect_ratio_scale;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{Mat4, Vec3, Vec4};

    // --- look_at_lerp_component: weight 0, 1, midpoint ---

    #[test]
    fn look_at_lerp_component_weight_zero_returns_cur_minus_delta() {
        // weight=0.0 -> cur - delta*(1.0-0.0) = cur - delta.
        assert_eq!(look_at_lerp_component(10.0, 3.0, 0.0), 7.0);
    }

    #[test]
    fn look_at_lerp_component_weight_one_returns_cur_unchanged() {
        // weight=1.0 -> cur - delta*(1.0-1.0) = cur - delta*0.0 = cur.
        assert_eq!(look_at_lerp_component(10.0, 3.0, 1.0), 10.0);
    }

    #[test]
    fn look_at_lerp_component_weight_midpoint() {
        // weight=0.5 -> 10.0 - 3.0*(1.0-0.5) = 10.0 - 3.0*0.5 = 10.0-1.5=8.5.
        assert_eq!(look_at_lerp_component(10.0, 3.0, 0.5), 8.5);
    }

    #[test]
    fn look_at_lerp_component_zero_delta_returns_cur_regardless_of_weight() {
        assert_eq!(look_at_lerp_component(5.0, 0.0, 0.25), 5.0);
        assert_eq!(look_at_lerp_component(5.0, 0.0, 0.75), 5.0);
    }

    #[test]
    fn look_at_lerp_component_negative_cur_and_delta() {
        // -10.0 - (-3.0)*(1.0-0.25) = -10.0 - (-2.25) = -10.0+2.25 = -7.75.
        assert_eq!(look_at_lerp_component(-10.0, -3.0, 0.25), -7.75);
    }

    #[test]
    fn look_at_lerp_component_out_of_range_weight_extrapolates() {
        // No clamp in the source: weight=2.0 -> cur - delta*(1.0-2.0) = cur - delta*(-1.0) = cur+delta.
        assert_eq!(look_at_lerp_component(10.0, 3.0, 2.0), 13.0);
        // weight=-1.0 -> cur - delta*(1.0-(-1.0)) = cur - delta*2.0.
        assert_eq!(look_at_lerp_component(10.0, 3.0, -1.0), 4.0);
    }

    #[test]
    fn look_at_lerp_component_zero_cur_and_delta() {
        assert_eq!(look_at_lerp_component(0.0, 0.0, 0.5), 0.0);
    }

    #[test]
    fn look_at_lerp_component_weight_quarter() {
        // 10.0 - 4.0*(1.0-0.25) = 10.0 - 4.0*0.75 = 10.0-3.0=7.0.
        assert_eq!(look_at_lerp_component(10.0, 4.0, 0.25), 7.0);
    }

    #[test]
    fn look_at_lerp_component_weight_three_quarters() {
        // 10.0 - 4.0*(1.0-0.75) = 10.0 - 4.0*0.25 = 10.0-1.0=9.0.
        assert_eq!(look_at_lerp_component(10.0, 4.0, 0.75), 9.0);
    }

    #[test]
    fn look_at_lerp_component_large_delta_small_weight() {
        // 0.0 - 1000.0*(1.0-0.01) = -1000.0*0.99 = -990.0.
        assert_eq!(look_at_lerp_component(0.0, 1000.0, 0.01), -990.0);
    }

    #[test]
    fn look_at_lerp_component_positive_cur_negative_delta_weight_zero() {
        // 5.0 - (-2.0)*(1.0-0.0) = 5.0 - (-2.0) = 7.0.
        assert_eq!(look_at_lerp_component(5.0, -2.0, 0.0), 7.0);
    }

    // --- look_at_lerp_component: NaN / inf ---

    #[test]
    fn look_at_lerp_component_nan_delta_propagates() {
        let result = look_at_lerp_component(10.0, f32::NAN, 0.5);
        assert!(result.is_nan());
    }

    #[test]
    fn look_at_lerp_component_infinite_cur_propagates() {
        let result = look_at_lerp_component(f32::INFINITY, 3.0, 0.5);
        assert!(result.is_infinite() && result.is_sign_positive());
    }

    #[test]
    fn look_at_lerp_component_weight_one_with_infinite_delta_still_returns_cur() {
        // At weight=1.0, delta*(1.0-1.0) = inf*0.0 = NaN in IEEE 754 (not 0.0!)
        // -- this is a real float-arithmetic frontier: the "weight=1 always
        // returns cur" property does NOT hold when delta is infinite, since
        // inf*0.0 is NaN, and cur - NaN is NaN, not cur. Preserved exactly,
        // not special-cased.
        let result = look_at_lerp_component(10.0, f32::INFINITY, 1.0);
        assert!(result.is_nan());
    }

    // --- look_at_lerp_vec3: componentwise, no cross-component coupling ---

    #[test]
    fn look_at_lerp_vec3_applies_componentwise() {
        let cur = Vec3::new(10.0, 20.0, 30.0);
        let delta = Vec3::new(1.0, 2.0, 3.0);
        let result = look_at_lerp_vec3(cur, delta, 0.0);
        // Each component independently: cur - delta (weight=0).
        assert_eq!(result, Vec3::new(9.0, 18.0, 27.0));
    }

    #[test]
    fn look_at_lerp_vec3_weight_one_returns_cur() {
        let cur = Vec3::new(1.0, 2.0, 3.0);
        let delta = Vec3::new(100.0, 200.0, 300.0);
        let result = look_at_lerp_vec3(cur, delta, 1.0);
        assert_eq!(result, cur);
    }

    #[test]
    fn look_at_lerp_vec3_distinct_per_component_deltas_do_not_cross_couple() {
        // If components were accidentally coupled (e.g. summed), changing
        // only delta.z would affect x/y too. Confirm it does not.
        let cur = Vec3::new(5.0, 5.0, 5.0);
        let delta_a = Vec3::new(1.0, 1.0, 1.0);
        let delta_b = Vec3::new(1.0, 1.0, 999.0);
        let result_a = look_at_lerp_vec3(cur, delta_a, 0.5);
        let result_b = look_at_lerp_vec3(cur, delta_b, 0.5);
        assert_eq!(result_a.x, result_b.x);
        assert_eq!(result_a.y, result_b.y);
        assert_ne!(result_a.z, result_b.z);
    }

    // --- adjust_projection_matrix: identity ---

    #[test]
    fn adjust_projection_matrix_identity_scaled_by_one_is_unchanged() {
        let identity = Mat4 {
            rows: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        };
        let result = adjust_projection_matrix(identity, 1.0);
        assert_eq!(result, identity);
    }

    #[test]
    fn adjust_projection_matrix_identity_scaled_by_two() {
        let identity = Mat4 {
            rows: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        };
        let result = adjust_projection_matrix(identity, 2.0);
        // Only rows[0].x changes for the identity (the only nonzero column-0
        // entry is row 0); rows[1..4].x are 0.0*2.0=0.0, unchanged in value.
        assert_eq!(result.rows[0], Vec4::new(2.0, 0.0, 0.0, 0.0));
        assert_eq!(result.rows[1], Vec4::new(0.0, 1.0, 0.0, 0.0));
        assert_eq!(result.rows[2], Vec4::new(0.0, 0.0, 1.0, 0.0));
        assert_eq!(result.rows[3], Vec4::new(0.0, 0.0, 0.0, 1.0));
    }

    // --- adjust_projection_matrix: scales only column 0 of every row ---

    #[test]
    fn adjust_projection_matrix_scales_only_column_zero_of_every_row() {
        // Every element distinct so a transpose or partial application is caught.
        let m = Mat4 {
            rows: [
                Vec4::new(1.0, 2.0, 3.0, 4.0),
                Vec4::new(5.0, 6.0, 7.0, 8.0),
                Vec4::new(9.0, 10.0, 11.0, 12.0),
                Vec4::new(13.0, 14.0, 15.0, 16.0),
            ],
        };
        let result = adjust_projection_matrix(m, 0.5);
        // Column 0 (x of every row) halved:
        assert_eq!(result.rows[0], Vec4::new(0.5, 2.0, 3.0, 4.0));
        assert_eq!(result.rows[1], Vec4::new(2.5, 6.0, 7.0, 8.0));
        assert_eq!(result.rows[2], Vec4::new(4.5, 10.0, 11.0, 12.0));
        assert_eq!(result.rows[3], Vec4::new(6.5, 14.0, 15.0, 16.0));
    }

    #[test]
    fn adjust_projection_matrix_does_not_scale_row_zero_of_every_column() {
        // Guards specifically against the row/column transpose mistake: if
        // the implementation scaled row 0 entirely (matrix[0][c] for all c)
        // instead of column 0 (matrix[r][0] for all r), rows[0].y/.z/.w would
        // also change here. They must not.
        let m = Mat4 {
            rows: [
                Vec4::new(1.0, 2.0, 3.0, 4.0),
                Vec4::new(5.0, 6.0, 7.0, 8.0),
                Vec4::new(9.0, 10.0, 11.0, 12.0),
                Vec4::new(13.0, 14.0, 15.0, 16.0),
            ],
        };
        let result = adjust_projection_matrix(m, 3.0);
        assert_eq!(result.rows[0].y, 2.0);
        assert_eq!(result.rows[0].z, 3.0);
        assert_eq!(result.rows[0].w, 4.0);
    }

    #[test]
    fn adjust_projection_matrix_scale_zero_zeroes_column_zero_only() {
        let m = Mat4 {
            rows: [
                Vec4::new(1.0, 2.0, 3.0, 4.0),
                Vec4::new(5.0, 6.0, 7.0, 8.0),
                Vec4::new(9.0, 10.0, 11.0, 12.0),
                Vec4::new(13.0, 14.0, 15.0, 16.0),
            ],
        };
        let result = adjust_projection_matrix(m, 0.0);
        assert_eq!(result.rows[0].x, 0.0);
        assert_eq!(result.rows[1].x, 0.0);
        assert_eq!(result.rows[2].x, 0.0);
        assert_eq!(result.rows[3].x, 0.0);
        // Non-column-0 entries untouched.
        assert_eq!(result.rows[2].w, 12.0);
    }

    #[test]
    fn adjust_projection_matrix_negative_scale() {
        let m = Mat4 {
            rows: [
                Vec4::new(2.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        };
        let result = adjust_projection_matrix(m, -1.0);
        assert_eq!(result.rows[0].x, -2.0);
    }

    // --- adjust_projection_matrix: NaN / inf ---

    #[test]
    fn adjust_projection_matrix_nan_scale_propagates_to_column_zero_only() {
        let m = Mat4 {
            rows: [
                Vec4::new(1.0, 2.0, 3.0, 4.0),
                Vec4::new(5.0, 6.0, 7.0, 8.0),
                Vec4::new(9.0, 10.0, 11.0, 12.0),
                Vec4::new(13.0, 14.0, 15.0, 16.0),
            ],
        };
        let result = adjust_projection_matrix(m, f32::NAN);
        assert!(result.rows[0].x.is_nan());
        assert!(result.rows[1].x.is_nan());
        assert!(result.rows[2].x.is_nan());
        assert!(result.rows[3].x.is_nan());
        // Non-column-0 entries untouched, not NaN-contaminated.
        assert_eq!(result.rows[0].y, 2.0);
        assert_eq!(result.rows[3].w, 16.0);
    }

    #[test]
    fn adjust_projection_matrix_infinite_scale_on_zero_column_yields_nan() {
        // 0.0 * inf = NaN (IEEE 754), a real frontier when a row's column-0
        // entry is exactly 0.0 and the scale is infinite.
        let m = Mat4 {
            rows: [
                Vec4::new(0.0, 2.0, 3.0, 4.0),
                Vec4::new(5.0, 6.0, 7.0, 8.0),
                Vec4::new(9.0, 10.0, 11.0, 12.0),
                Vec4::new(13.0, 14.0, 15.0, 16.0),
            ],
        };
        let result = adjust_projection_matrix(m, f32::INFINITY);
        assert!(result.rows[0].x.is_nan());
        assert!(result.rows[1].x.is_infinite());
    }
}
