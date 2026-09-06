// RSPSmoothNormalCS arithmetic. Characterization-only; not wired into any
// draw path, dispatch, or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `compute_smooth_normal`
// (`crate::rt64_rsp_smooth_normal`, mirroring RT64's
// `src/shaders/RSPSmoothNormalCS.hlsl`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875, SHA-256
// d2ebad1d920cbab555171f9c95e25780777900c1c703e604730fbc28dfb3869e).
// Reproduces exactly the single-vertex `computeSmoothNormal` body:
// - weld predicate: `dot(posDelta, posDelta) <= 1.0 && cmpCol == vertexCol`
//   (inclusive threshold, squared distance, logical AND)
// - per-triangle face normal: `normalize(cross(triB - triA, triC - triA))`
// - accumulation: added once per passing (triangle, corner) pair, in
//   triangle-then-corner visitation order, never reassociated
// - final `normalize(vertexNorm)`, plain unguarded Euclidean normalize
//
// `#define GROUP_SIZE 64`, `[numthreads(64,1,1)]`, `SV_DispatchThreadID`,
// the `CSMain` entry point and its dispatch guard,
// `AllMemoryBarrierWithGroupSync()`, and every `gConstants`/`srcWorldPos`/
// `srcCol`/`srcFaceIndices`/`dstWorldNorm` buffer bind/load/store
// (including `indexStart`/`indexCount`/`triangleCount`/`baseIndex` index
// arithmetic) are all deliberately NOT ported -- this file has no
// `@compute` entry point, only the plain arithmetic functions, per this
// ticket's scope statement (dispatch scaffolding excluded). No `lerp`/
// `mix` appears in `RSPSmoothNormalCS.hlsl`, so there is no
// HLSL-`lerp`-vs-WGSL-`mix` concern in this file.

// `cross(a, b)` (`RSPSmoothNormalCS.hlsl:39`'s cross-product operand):
// `(a.y*b.z - a.z*b.y, a.z*b.x - a.x*b.z, a.x*b.y - a.y*b.x)`.
fn rsp_smooth_normal_cross(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    );
}

// Plain, unguarded Euclidean normalization: `v / length(v)`, `length(v) =
// sqrt(x^2+y^2+z^2)`. A zero-length `v` divides `0.0/0.0` in every
// component, yielding `NaN` -- no epsilon guard is added, matching
// `RSPSmoothNormalCS.hlsl`'s plain `normalize(...)` calls exactly.
fn rsp_smooth_normal_normalize(v: vec3<f32>) -> vec3<f32> {
    let len = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    return vec3<f32>(v.x / len, v.y / len, v.z / len);
}

// `RSPSmoothNormalCS.hlsl:32`'s weld predicate: `dot(posDelta, posDelta) <=
// PosDistSqr && cmpCol == vertexCol`, `PosDistSqr = 1.0f`, threshold
// inclusive.
fn rsp_smooth_normal_weld_predicate(
    vertex_pos: vec3<f32>,
    vertex_col: u32,
    cmp_pos: vec3<f32>,
    cmp_col: u32,
) -> bool {
    let pos_delta = cmp_pos - vertex_pos;
    let dist_sqr = dot(pos_delta, pos_delta);
    return (dist_sqr <= 1.0) && (cmp_col == vertex_col);
}

// `RSPSmoothNormalCS.hlsl:39`'s per-triangle face normal: `normalize(cross(triB
// - triA, triC - triA))`. Operand order is load-bearing (`cross` is
// anticommutative).
fn rsp_smooth_normal_face_normal(tri_a: vec3<f32>, tri_b: vec3<f32>, tri_c: vec3<f32>) -> vec3<f32> {
    return rsp_smooth_normal_normalize(rsp_smooth_normal_cross(tri_b - tri_a, tri_c - tri_a));
}
