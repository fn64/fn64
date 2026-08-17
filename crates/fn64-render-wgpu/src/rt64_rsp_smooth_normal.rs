//! Literal port of RT64's `RSPSmoothNormalCS` (welded-vertex normal
//! computation) arithmetic, a permitted MIT RT64 Rust-port source pinned at
//! commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! `src/shaders/RSPSmoothNormalCS.hlsl` (SHA-256 of the whole file, 64
//! lines, `d2ebad1d920cbab555171f9c95e25780777900c1c703e604730fbc28dfb3869e`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//! // Computes the smooth normal of the vertices by welding the vertices with similar position and color.
//! //
//!
//! #define GROUP_SIZE 64
//!
//! struct RSPSmoothNormalCB {
//!     uint indexStart;
//!     uint indexCount;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<RSPSmoothNormalCB> gConstants : register(b0);
//! StructuredBuffer<float4> srcWorldPos : register(t1);
//! StructuredBuffer<uint> srcCol : register(t2);
//! StructuredBuffer<uint> srcFaceIndices : register(t3);
//! RWStructuredBuffer<float4> dstWorldNorm : register(u4);
//!
//! float3 computeSmoothNormal(uint vertexIndex) {
//!     const float PosDistSqr = 1.0f;
//!     const float3 vertexPos = srcWorldPos[vertexIndex].xyz;
//!     const uint vertexCol = srcCol[vertexIndex];
//!     float3 vertexNorm = float3(0.0f, 0.0f, 0.0f);
//!     const uint triangleCount = gConstants.indexCount / 3;
//!     for (uint t = 0; t < triangleCount; t++) {
//!         for (uint j = 0; j < 3; j++) {
//!             const uint cmpIndex = srcFaceIndices[gConstants.indexStart + t * 3 + j];
//!             const float3 cmpPos = srcWorldPos[cmpIndex].xyz;
//!             const uint cmpCol = srcCol[cmpIndex];
//!             const float3 posDelta = cmpPos - vertexPos;
//!             if ((dot(posDelta, posDelta) <= PosDistSqr) && (cmpCol == vertexCol)) {
//!                 const uint indexA = srcFaceIndices[gConstants.indexStart + t * 3 + 0];
//!                 const uint indexB = srcFaceIndices[gConstants.indexStart + t * 3 + 1];
//!                 const uint indexC = srcFaceIndices[gConstants.indexStart + t * 3 + 2];
//!                 const float3 triA = srcWorldPos[indexA].xyz;
//!                 const float3 triB = srcWorldPos[indexB].xyz;
//!                 const float3 triC = srcWorldPos[indexC].xyz;
//!                 vertexNorm += normalize(cross(triB - triA, triC - triA));
//!             }
//!         }
//!     }
//!
//!     return normalize(vertexNorm);
//! }
//!
//! [numthreads(GROUP_SIZE, 1, 1)]
//! void CSMain(uint triangleIndex : SV_DispatchThreadID) {
//!     if ((triangleIndex * 3) >= gConstants.indexCount) {
//!         return;
//!     }
//!
//!     const uint baseIndex = gConstants.indexStart + triangleIndex * 3;
//!     const uint v0 = srcFaceIndices[baseIndex + 0];
//!     const uint v1 = srcFaceIndices[baseIndex + 1];
//!     const uint v2 = srcFaceIndices[baseIndex + 2];
//!     const float3 n0 = computeSmoothNormal(v0);
//!     const float3 n1 = computeSmoothNormal(v1);
//!     const float3 n2 = computeSmoothNormal(v2);
//!     AllMemoryBarrierWithGroupSync();
//!     dstWorldNorm[v0] = float4(n0, 1.0f);
//!     dstWorldNorm[v1] = float4(n1, 1.0f);
//!     dstWorldNorm[v2] = float4(n2, 1.0f);
//! }
//! ```
//!
//! **Reuse, not new type.** [`fn64_render_ir::Vec3`] provides
//! `sub`/`scale`/`dot`, all plain enough (no accumulation-order or
//! signedness question) that this module reuses them directly for
//! `posDelta`/`triB - triA`/`triC - triA`/`dot(posDelta, posDelta)`.
//! `rsp_math::Vec3` has no `cross`, `add`, or `normalize` method (checked:
//! `crates/fn64-render-ir/src/rsp_math.rs` defines only `new`, `splat`,
//! `sub`, `scale`, `dot` on `Vec3`) -- this module adds its own
//! [`cross`], [`add`], and [`normalize`] free functions rather than
//! reaching into any sibling's private helpers or widening `rsp_math`'s
//! public surface; this is a plain gap, not a visibility block (nothing
//! private was found blocking reuse), so it is noted here rather than
//! reported as a hazard-4 gap. `crate::rt64_rsp_world_modify` (the M5.7
//! sibling landed just before this ticket) is the closest precedent for
//! this module's CPU-oracle + owned-WGSL + Naga-validation shape and is
//! followed structurally throughout.
//!
//! ## Admitted domain
//!
//! - **Weld predicate: `dot(posDelta, posDelta) <= 1.0f && cmpCol ==
//!   vertexCol`, both conditions required (logical AND), threshold
//!   INCLUSIVE (`<=`).** `PosDistSqr` is a squared-distance threshold of
//!   exactly `1.0f` (i.e. weld radius `1.0` world unit, compared against the
//!   squared length so no `sqrt` is needed) -- a candidate vertex at exactly
//!   distance `1.0` welds (`<=`, not `<`); one unit past the boundary does
//!   not. Ported as [`weld_predicate`]: `pos_delta.dot(pos_delta) <=
//!   1.0 && cmp_col == vertex_col`, evaluated with plain `f32`/`u32`
//!   comparisons, no epsilon slop added beyond what the source itself has
//!   (none). Color compares as a raw packed `u32` (`cmpCol == vertexCol`),
//!   not per-channel or with any tolerance.
//! - **Per-candidate-triangle accumulation: `vertexNorm += normalize(cross(triB
//!   - triA, triC - triA))`, summed once per (t, j) iteration that passes
//!   the weld predicate -- in `(t, j)` loop order, `t` outer (triangle),
//!   `j` inner (0..3, one weld test per triangle corner).** Because the
//!   *same* triangle's face normal is added up to three times if more than
//!   one of its three corners individually welds to `vertexIndex` (the
//!   `j` loop re-tests and re-adds `normalize(cross(...))` for the *same*
//!   `triA/triB/triC` on each passing `j`, not once per triangle) --
//!   this is a literal reading of the source's nested-loop structure, not
//!   an optimization or dedup this port performs. Float addition is NOT
//!   associative; the accumulation order here is triangle-index-then-corner
//!   order exactly as the HLSL loop visits them, and this port's
//!   [`compute_smooth_normal`] preserves that same visitation order over
//!   its `faces: &[[VertexSample; 3]]` slice (iterate `faces` in slice
//!   order, then `j` in `0..3` within each), never reassociated,
//!   reordered, or pre-summed via any commutative-reduction shortcut.
//! - **Per-triangle face normal: `normalize(cross(triB - triA, triC -
//!   triA))`, using the triangle's OWN three corner positions (indices
//!   `indexA`/`indexB`/`indexC` of triangle `t`), not the candidate vertex
//!   `cmpIndex`/`vertexIndex` positions.** `triA`/`triB`/`triC` are always
//!   `srcWorldPos[indexA/B/C]` -- `cmpIndex`/`cmpPos` from the weld test are
//!   used ONLY to decide whether to accumulate, never as an operand of the
//!   `cross`/`normalize` itself. Ported as [`face_normal`]: `normalize(cross(tri_b.sub(tri_a),
//!   tri_c.sub(tri_a)))`, in that exact `(triB - triA, triC - triA)`
//!   argument order (swapping the two cross operands negates the result,
//!   and `cross` is anticommutative, so operand order is load-bearing).
//! - **`cross(a, b)` component formula and evaluation order:
//!   `(a.y*b.z - a.z*b.y, a.z*b.x - a.x*b.z, a.x*b.y - a.y*b.x)`**, each
//!   component a single two-term difference of two products, no
//!   reassociation. Ported as [`cross`] with that exact per-component
//!   formula.
//! - **`normalize(v)` is plain, unguarded Euclidean normalization: `v /
//!   length(v)`, `length(v) = sqrt(x^2+y^2+z^2)`, ordinary IEEE-754
//!   division with NO epsilon guard, matching the same convention
//!   `crate::rt64_rsp_world_modify`'s `rsp_world_norm` already documents
//!   for `RSPWorldCS`'s `normalize`.** This module's [`normalize`] free
//!   function is a plain `sqrt(x*x+y*y+z*z)` length plus a per-component
//!   division, unguarded, called at TWO distinct sites: (1) inside the
//!   weld-accumulation loop on each individual `cross(...)` face normal
//!   before adding it to `vertexNorm`, and (2) once more on the final
//!   `vertexNorm` sum before returning. Both call sites use the identical
//!   [`normalize`] function; there is no difference in formula between the
//!   per-face and the final normalize.
//! - **Zero-length normalization yields NaN in every component, with no
//!   guard added by this port.** A degenerate triangle (`triB == triA` or
//!   `triC == triA`, or a `cross` product that underflows to exactly
//!   `(0,0,0)`) makes `length(v) == 0.0`, so `v / length(v)` is `0.0 /
//!   0.0` in every component, IEEE-754 `NaN`. This is exactly what welding
//!   is expected to produce for a vertex whose accumulated normal sum
//!   cancels to exactly `(0,0,0)` (e.g. two triangles with opposing face
//!   normals of equal magnitude both welding to the same vertex): the
//!   FINAL `normalize(vertexNorm)` call then divides `0.0/0.0`, propagating
//!   `NaN` into the vertex's output normal, matching the source's plain
//!   IEEE-754 semantics exactly -- this port adds no epsilon guard the
//!   source does not have, at either normalize call site.
//! - **`RSPSmoothNormalCB { indexStart, indexCount }` / `triangleCount =
//!   indexCount / 3` (integer division, truncating) is buffer-range
//!   framing, not welding arithmetic** -- this port's [`compute_smooth_normal`]
//!   takes an already-sliced `faces: &[[VertexSample; 3]]` (one entry per
//!   triangle in `[indexStart, indexStart + indexCount)`, each already
//!   resolved to its three `(pos, col)` vertex samples) rather than a flat
//!   index buffer plus `indexStart`/`indexCount`/`triangleCount`; see
//!   "Nonclaims".
//! - **No fixed-point conversion of any kind appears in this shader** --
//!   `srcWorldPos` is `float4`, `srcCol`/`srcFaceIndices` are plain `uint`
//!   compared/indexed as `uint`, never divided by `65536.0f` or any other
//!   fixed-point scale. Hazard 1 (`65536.0` unsigned-widening vs. signed
//!   s16.16-reinterpret ambiguity) does not apply to this file; there is
//!   nothing to pin here.
//! - **No `lerp` or `mix` call of any kind appears in this shader** -- this
//!   module has no HLSL-`lerp`-vs-WGSL-`mix` divergence to admit (checked:
//!   `grep -n 'lerp\|mix(' RSPSmoothNormalCS.hlsl` finds nothing); see the
//!   test asserting this module's own oracle and WGSL text likewise contain
//!   neither spelling.
//!
//! ## Nonclaims
//!
//! No GPU execution: the WGSL sibling file
//! (`crates/fn64-render-wgpu/src/shaders/rsp_smooth_normal.wgsl`) is
//! validated only through Naga's WGSL front-end and validator (a plain,
//! non-GPU test), not by dispatching the shader on a real adapter/device --
//! this host does have a real GPU (`host-gpu-tests`) but no such run was
//! performed for this ticket. No production wiring: no pipeline,
//! `wgpu::ShaderModule`, bind group layout, or `targets/` integration is
//! created here, this module has no `pub use` anywhere else in the crate,
//! and it is not referenced from any dispatch path. No parity or
//! performance claim of any kind against RT64's own renderer.
//!
//! Dispatch scaffolding is not ported, in either language: `#define
//! GROUP_SIZE 64` / `[numthreads(GROUP_SIZE, 1, 1)]` / `SV_DispatchThreadID`,
//! the `CSMain` entry point's `(triangleIndex * 3) >= gConstants.indexCount`
//! early-return guard, `AllMemoryBarrierWithGroupSync()` (no `groupshared`
//! state appears in this shader; the barrier here only orders the three
//! `dstWorldNorm` writes against the three `computeSmoothNormal` calls
//! within one invocation), and every resource bind/load/store
//! (`gConstants`/`srcWorldPos`/`srcCol`/`srcFaceIndices`/`dstWorldNorm`, all
//! five `register(b0)`/`register(t*)`/`register(u4)` bindings, and the
//! `RSPSmoothNormalCB` push-constant struct) are excluded. `baseIndex =
//! gConstants.indexStart + triangleIndex * 3` and `gConstants.indexStart +
//! t * 3 + j` / `+ 0`/`+ 1`/`+ 2` are buffer-layout index arithmetic, also
//! excluded -- see "Admitted domain"'s closing bullets. This module's
//! [`compute_smooth_normal`] takes and returns plain values
//! ([`VertexSample`], `fn64_render_ir::rsp_math::Vec3`) rather than reading
//! or writing any buffer, and takes an already-resolved `faces` slice
//! rather than a flat index buffer -- the `CSMain` entry point's own
//! three-vertices-per-thread fan-out (`v0`/`v1`/`v2` each independently
//! calling `computeSmoothNormal`, then writing three `dstWorldNorm` slots)
//! is also not reproduced; this module ports only the single-vertex
//! `computeSmoothNormal` body.

use fn64_render_ir::Vec3;

/// One candidate/triangle-corner vertex sample: `srcWorldPos[i].xyz` plus
/// `srcCol[i]`, the two fields `computeSmoothNormal`'s weld predicate reads
/// (`RSPSmoothNormalCS.hlsl:22-23,29-30`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexSample {
    pub pos: Vec3,
    pub col: u32,
}

impl VertexSample {
    pub const fn new(pos: Vec3, col: u32) -> Self {
        Self { pos, col }
    }
}

/// Literal port of `cross(a, b)`
/// (`RSPSmoothNormalCS.hlsl:39`'s `cross(triB - triA, triC - triA)` operand):
/// `(a.y*b.z - a.z*b.y, a.z*b.x - a.x*b.z, a.x*b.y - a.y*b.x)`. See module
/// doc "Admitted domain".
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// Component-wise vector addition, `vertexNorm += normalize(...)`'s `+`
/// half (`RSPSmoothNormalCS.hlsl:39`). `rsp_math::Vec3` has no `add` method;
/// see module doc "Reuse, not new type".
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

/// Literal port of `normalize(v)`: plain, unguarded Euclidean
/// normalization, `v / length(v)`, `length(v) = sqrt(x^2+y^2+z^2)`. A
/// zero-length `v` divides `0.0/0.0`, yielding `NaN` in every component --
/// no epsilon guard is added. See module doc "Admitted domain".
pub fn normalize(v: Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    Vec3::new(v.x / len, v.y / len, v.z / len)
}

/// Literal port of `RSPSmoothNormalCS.hlsl:32`'s weld predicate:
/// `dot(posDelta, posDelta) <= PosDistSqr && cmpCol == vertexCol`,
/// `PosDistSqr = 1.0f`, threshold inclusive. See module doc "Admitted
/// domain".
pub fn weld_predicate(vertex_pos: Vec3, vertex_col: u32, cmp: VertexSample) -> bool {
    const POS_DIST_SQR: f32 = 1.0;
    let pos_delta = cmp.pos.sub(vertex_pos);
    pos_delta.dot(pos_delta) <= POS_DIST_SQR && cmp.col == vertex_col
}

/// Literal port of `RSPSmoothNormalCS.hlsl:39`'s per-triangle face normal:
/// `normalize(cross(triB - triA, triC - triA))`. Operand order is
/// load-bearing (`cross` is anticommutative). See module doc "Admitted
/// domain".
pub fn face_normal(tri_a: Vec3, tri_b: Vec3, tri_c: Vec3) -> Vec3 {
    normalize(cross(tri_b.sub(tri_a), tri_c.sub(tri_a)))
}

/// Literal port of `computeSmoothNormal`'s full body
/// (`RSPSmoothNormalCS.hlsl:20-45`), buffer reads/writes and
/// `indexStart`/`indexCount`/`triangleCount` index arithmetic excluded --
/// see module doc "Nonclaims". `vertex` stands in for the already-read
/// `srcWorldPos[vertexIndex].xyz`/`srcCol[vertexIndex]` pair. `faces` stands
/// in for the already-resolved triangle list in `[indexStart, indexStart +
/// indexCount)`: `faces[t]` is triangle `t`'s three `(pos, col)` corner
/// samples in `indexA, indexB, indexC` order (`faces[t][0]` = `triA`'s
/// sample, etc.), one entry per triangle, in the same order
/// `srcFaceIndices[gConstants.indexStart + t*3 + j]` visits them for
/// `t in 0..triangleCount`.
///
/// Loop order and accumulation order are preserved exactly: outer over
/// `faces` (triangle `t`), inner over the triangle's three corners (`j` in
/// `0..3`); each passing corner re-adds that SAME triangle's `face_normal`
/// to the running sum (float addition is not associative -- see module doc
/// "Admitted domain").
pub fn compute_smooth_normal(vertex: VertexSample, faces: &[[VertexSample; 3]]) -> Vec3 {
    let mut vertex_norm = Vec3::new(0.0, 0.0, 0.0);
    for corners in faces {
        let tri_a = corners[0].pos;
        let tri_b = corners[1].pos;
        let tri_c = corners[2].pos;
        let fn_ = face_normal(tri_a, tri_b, tri_c);
        for cmp in corners {
            if weld_predicate(vertex.pos, vertex.col, *cmp) {
                vertex_norm = add(vertex_norm, fn_);
            }
        }
    }
    normalize(vertex_norm)
}

pub const RSP_SMOOTH_NORMAL_WGSL: &str = include_str!("shaders/rsp_smooth_normal.wgsl");

#[cfg(test)]
mod tests;
