// RSPWorldCS arithmetic. Characterization-only; not wired into any draw
// path, dispatch, or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `rsp_world_transform` (`crate::rt64_rsp_world_modify`,
// mirroring RT64's `src/shaders/RSPWorldCS.hlsl`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875, SHA-256
// e696b0bc2924f31e0636f07cf437bcf805957064ce6a1dcefa643d9f7f71bd54). Reproduces
// exactly:
// - `worldPos = mul(worldMat, float4(pos - vel * (1 - curFrameWeight), 1))`
// - `prevWorldPos` computed the same way with `prevFrameWeight`/`prevWorldMat`
// - the `all(norm == 0.0f)` zero-normal guard, else
//   `normalize(mul(invTWorldMat, float4(norm, 0)).xyz)` extended to `(.., 1)`
// - `velOut = worldPos - prevWorldPos`
//
// A `float4x4` here is passed as four `vec4<f32>` ROWS (`row0`..`row3`), not
// WGSL's built-in `mat4x4<f32>` (which is column-major and whose `*`
// operator does not by itself guarantee the same row/column dot-product
// evaluation order this port needs to stay bit-exact with the row-major
// `Mat4::transform_point` oracle in `fn64_render_ir::rsp_math` -- see
// `crate::rt64_rsp_world_modify` module doc "Admitted domain", "Matrix
// multiply order"). `rsp_world_mul` below reproduces
// `Mat4::transform_point`'s exact four-term-sum-per-component form.
//
// `[numthreads(64,1,1)]`, `SV_DispatchThreadID`, the `vertexIndex >=
// gConstants.vertexCount` dispatch guard, `vertexOffsetIndex`/`posIndex`/
// `normIndex` buffer-layout index arithmetic, and every `srcPos`/`srcVel`/
// `srcNorm`/`srcIndices`/`worldMats`/`invTWorldMats`/`prevWorldMats`/
// `dstPos`/`dstNorm`/`dstVel` buffer bind/load/store are all deliberately
// NOT ported -- this file has no `@compute` entry point, only the plain
// arithmetic functions, per this ticket's scope statement (dispatch
// scaffolding excluded). No `lerp`/`mix` appears in `RSPWorldCS.hlsl`, so
// there is no HLSL-`lerp`-vs-WGSL-`mix` concern in this file.

// `mul(matrix, vector)` (`float4x4` on the left, column vector on the right):
// M*v, reproducing `fn64_render_ir::rsp_math::Mat4::transform_point`'s exact
// four-term-sum-per-row evaluation order (never reassociated).
fn rsp_world_mul(row0: vec4<f32>, row1: vec4<f32>, row2: vec4<f32>, row3: vec4<f32>, v: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        row0.x * v.x + row0.y * v.y + row0.z * v.z + row0.w * v.w,
        row1.x * v.x + row1.y * v.y + row1.z * v.z + row1.w * v.w,
        row2.x * v.x + row2.y * v.y + row2.z * v.z + row2.w * v.w,
        row3.x * v.x + row3.y * v.y + row3.z * v.z + row3.w * v.w,
    );
}

// `pos - vel * (1.0f - frameWeight)` (`RSPWorldCS.hlsl:35,37`), shared by
// both the current-frame and previous-frame position computation with a
// different `frameWeight` argument (`curFrameWeight` / `prevFrameWeight`).
fn rsp_world_weighted_pos(pos: vec3<f32>, vel: vec3<f32>, frame_weight: f32) -> vec3<f32> {
    return pos - vel * (1.0 - frame_weight);
}

// The `all(norm == 0.0f) ? float4(0,0,0,1) : float4(normalize(mul(invTWorldMat, float4(norm,0)).xyz), 1)`
// ternary (`RSPWorldCS.hlsl:36`). `normalize` here is plain Euclidean
// normalization (`v / length(v)`, unguarded IEEE-754 division -- a
// zero-length *non-zero-component* input, impossible for f32 unless every
// component underflows to exactly 0.0, is not specially guarded beyond the
// exact-zero check the source itself performs).
fn rsp_world_norm(
    inv_t_row0: vec4<f32>,
    inv_t_row1: vec4<f32>,
    inv_t_row2: vec4<f32>,
    inv_t_row3: vec4<f32>,
    norm: vec3<f32>,
) -> vec4<f32> {
    if (norm.x == 0.0 && norm.y == 0.0 && norm.z == 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let transformed = rsp_world_mul(inv_t_row0, inv_t_row1, inv_t_row2, inv_t_row3, vec4<f32>(norm, 0.0));
    let n = transformed.xyz;
    let len = sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
    return vec4<f32>(n / len, 1.0);
}
