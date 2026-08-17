//! Literal port of RT64's `RSPModifyCS` (fixed-point vertex-patch write) and
//! `RSPWorldCS` (world-space position/normal/velocity transform) arithmetic,
//! a permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! `src/shaders/RSPModifyCS.hlsl` (SHA-256 of the whole file, 36 lines,
//! `761633642c76b3e5a09f8e9077d646150291bd4d831fc21aa952d8eb6339fb6c`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #define GROUP_SIZE 64
//!
//! struct RSPModifyCB {
//!     uint modifyCount;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<RSPModifyCB> gConstants : register(b0);
//! Buffer<uint> srcModifyPos : register(t1);
//! RWStructuredBuffer<float4> screenPos : register(u2);
//!
//! [numthreads(GROUP_SIZE, 1, 1)]
//! void CSMain(uint modifyIndex : SV_DispatchThreadID) {
//!     if (modifyIndex >= gConstants.modifyCount) {
//!         return;
//!     }
//!
//!     const uint modifyOffset = modifyIndex * 2;
//!     const bool modifyZ = srcModifyPos[modifyOffset] & 0x1;
//!     const uint vertexIndex = srcModifyPos[modifyOffset] >> 1;
//!     const uint modifyValue = srcModifyPos[modifyOffset + 1];
//!     if (modifyZ) {
//!         screenPos[vertexIndex].z = modifyValue / 65536.0f;
//!     }
//!     else {
//!         const uint extX = (modifyValue >> 16) & 0xFFFF;
//!         const uint extY = modifyValue & 0xFFFF;
//!         const int intX = int(extX) << 16 >> 16;
//!         const int intY = int(extY) << 16 >> 16;
//!         screenPos[vertexIndex].x = intX / 4.0f;
//!         screenPos[vertexIndex].y = intY / 4.0f;
//!     }
//! }
//! ```
//!
//! `src/shaders/RSPWorldCS.hlsl` (SHA-256 of the whole file, 45 lines,
//! `e696b0bc2924f31e0636f07cf437bcf805957064ce6a1dcefa643d9f7f71bd54`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #define GROUP_SIZE 64
//!
//! struct RSPWorldCB {
//!     uint vertexStart;
//!     uint vertexCount;
//!     float prevFrameWeight;
//!     float curFrameWeight;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<RSPWorldCB> gConstants : register(b0);
//! Buffer<float> srcPos : register(t1);
//! Buffer<float> srcVel : register(t2);
//! Buffer<int> srcNorm : register(t3);
//! Buffer<uint> srcIndices : register(t4);
//! StructuredBuffer<float4x4> worldMats : register(t5);
//! StructuredBuffer<float4x4> invTWorldMats : register(t6);
//! StructuredBuffer<float4x4> prevWorldMats : register(t7);
//! RWStructuredBuffer<float4> dstPos : register(u8);
//! RWStructuredBuffer<float4> dstNorm : register(u9);
//! RWStructuredBuffer<float4> dstVel : register(u10);
//!
//! [numthreads(GROUP_SIZE, 1, 1)]
//! void CSMain(uint vertexIndex : SV_DispatchThreadID) {
//!     if (vertexIndex >= gConstants.vertexCount) {
//!         return;
//!     }
//!
//!     const uint vertexOffsetIndex = gConstants.vertexStart + vertexIndex;
//!     const uint transformIndex = srcIndices[vertexOffsetIndex];
//!     const uint posIndex = vertexOffsetIndex * 3;
//!     const uint normIndex = vertexOffsetIndex * 4;
//!     const float3 pos = float3(srcPos[posIndex + 0], srcPos[posIndex + 1], srcPos[posIndex + 2]);
//!     const float3 vel = float3(srcVel[posIndex + 0], srcVel[posIndex + 1], srcVel[posIndex + 2]);
//!     const float3 norm = float3(srcNorm[normIndex + 0], srcNorm[normIndex + 1], srcNorm[normIndex + 2]);
//!     const float4 worldPos = mul(worldMats[transformIndex], float4(pos - vel * (1.0f - gConstants.curFrameWeight), 1.0f));
//!     const float4 worldNorm = all(norm == 0.0f) ? float4(0.0f, 0.0f, 0.0f, 1.0f) : float4(normalize(mul(invTWorldMats[transformIndex], float4(norm, 0.0f)).xyz), 1.0f);
//!     const float4 prevWorldPos = mul(prevWorldMats[transformIndex], float4(pos - vel * (1.0f - gConstants.prevFrameWeight), 1.0f));
//!     dstPos[vertexOffsetIndex] = worldPos;
//!     dstNorm[vertexOffsetIndex] = worldNorm;
//!     dstVel[vertexOffsetIndex] = worldPos - prevWorldPos;
//! }
//! ```
//!
//! **Reuse, not new type.** [`crate::rt64_rsp_patch`] (just landed, M5.6)
//! already ports the *CPU-side* half of `RSPModifyCS`'s producer:
//! `RSP::modifyVertex`'s `G_MWO_POINT_XYSCREEN`/`G_MWO_POINT_ZSCREEN` cases
//! compute the exact same `intX/intY = int16_t(extX/extY) ... / 4.0f` and
//! `z = value / 65536.0f` formulas this GPU shader's `CSMain` re-derives
//! from the `modifyPosUints` tag word that CPU code appends to
//! (`decode_modify_vertex`'s `ModifyVertexPatch::XyScreen`/`ZScreen`
//! variants and their doc comment "the tag word... decoded the same way").
//! `RSPModifyCS` is the GPU-side counterpart of that same patch: it reads
//! the packed `(tag, value)` pair back out of `srcModifyPos` and writes the
//! decoded floats into `screenPos`. [`decode_modify_cs`] below is a **second,
//! independent port of the same semantics** -- it is not a call-through to
//! `rt64_rsp_patch::decode_modify_vertex` (that function's `ModifyVertexPatch`
//! carries CPU-only fields this shader never touches, e.g. `Rgba`/`St`
//! variants and the `dstIndex`-bound check, and it takes the *decoded*
//! `(dst_index, dst_attribute, value, global_index)` tuple as input rather
//! than the *packed* `(tag_and_index, value)` word pair `CSMain` actually
//! reads off the wire). This module's tests separately confirm the two
//! ports produce identical results for the shared XY/Z formulas (see
//! "Admitted domain"), demonstrating the semantic overlap without merging
//! the two call shapes.
//!
//! [`fn64_render_ir::rsp_math::Mat4::transform_point`] already implements
//! exactly the `mul(matrix, vector) = M·v` call shape `RSPWorldCS` needs
//! (`crates/fn64-render-ir/src/rsp_math.rs:106-126`, itself the established
//! convention `rt64_rsp_matrix_stack.rs`'s module doc cites by name). This
//! module reuses it directly for all three `mul(...)` calls in
//! [`rsp_world_transform`] rather than re-deriving 4x4-matrix-by-vector
//! arithmetic a fourth time in this crate; [`fn64_render_ir::rsp_math::Vec3`]
//! and [`fn64_render_ir::rsp_math::Vec4`] are reused the same way for
//! `pos`/`vel`/`norm`. `RSPWorldCS` needs no matrix-by-matrix product (every
//! `mul` call here is `mul(float4x4, float4)`), so this module does not need
//! -- and does not add -- a fourth private re-derivation of `mat4_mul`; see
//! "Nonclaims" for the existing `mat4_mul` visibility gap this module
//! deliberately does not touch or worsen.
//!
//! ## Admitted domain
//!
//! **`RSPModifyCS` (fixed-point vertex-patch write):**
//!
//! - **`modifyZ`/`vertexIndex` packed-word decode.** `srcModifyPos[modifyOffset]`
//!   packs a select bit (`& 0x1`) and a vertex index (`>> 1`) into one
//!   `uint`, the exact `(global_index << 1) | tag_bit` encoding
//!   `rt64_rsp_patch::decode_modify_vertex`'s `XyScreen`/`ZScreen` variants'
//!   `tag` field already documents producing (XYSCREEN: `global_index << 1`,
//!   low bit clear; ZSCREEN: `(global_index << 1) | 0x1`, low bit set).
//!   [`ModifyPosDecode`] carries these two fields (`modify_z: bool`,
//!   `vertex_index: u32`) as a plain bitfield decode, `u32 & 1`/`u32 >> 1`,
//!   no signedness or width concern (both fields are well inside `u32`'s
//!   range for any realistic vertex count).
//! - **Z branch: `modifyValue / 65536.0f` is an UNSIGNED widening
//!   conversion, not a signed s16.16 reinterpret.** This is the same fact
//!   `rt64_rsp_patch.rs`'s module doc already establishes for
//!   `G_MWO_POINT_ZSCREEN` ("deliberately **not** routed through
//!   `FixedMatrix::fixed_to_float` despite the shared divisor, since that
//!   helper's `(full_word as i32) as f32` step assumes a *signed* s16.16
//!   value this field never constructs") -- `RSPModifyCS`'s Z branch is
//!   that same value read back on the GPU side, and it uses the identical
//!   unsigned `uint -> float` conversion HLSL performs implicitly for
//!   `modifyValue / 65536.0f` (`modifyValue` is `uint`, `65536.0f` is
//!   `float`, so HLSL promotes `modifyValue` to `float` via an unsigned
//!   widening conversion before dividing -- never a bit-pattern reinterpret
//!   as `int32_t`). Ported as `modify_value as f32 / 65536.0`. This is the
//!   ticket brief's "one an s16.16 signed reinterpret, one a plain unsigned
//!   cast" pair's *unsigned* member (`FixedMatrix::fixed_to_float` in
//!   `rt64_common.rs`, `(full_word as i32) as f32 / 65536.0`, is the
//!   *signed* member; both divide by the same `65536.0` literal but are
//!   different operations on different data, and neither is reused by
//!   name here to avoid conflating them -- [`rsp_modify_z`] is its own
//!   independent one-line function).
//! - **XY branch: `(int(extX) << 16) >> 16` sign-extends each 16-bit half,
//!   then divides by `4.0f` -- signed.** `extX`/`extY` are extracted as
//!   `uint` (`(modifyValue >> 16) & 0xFFFF` / `modifyValue & 0xFFFF`), then
//!   `int(extX)` reinterprets those low 16 bits as a 32-bit signed value
//!   with the top 16 bits still zero (a zero-extending `uint`-to-`int`
//!   *value* conversion, not yet sign-extended), and `<< 16 >> 16` (an
//!   arithmetic right shift on `int` in HLSL) is the idiom that performs
//!   the actual 16-to-32-bit sign extension: shifting the 16-bit payload up
//!   into the top half, then back down with sign-fill. Ported as
//!   `((ext_x as i32) << 16) >> 16` in Rust: `as i32` on a `u32` already
//!   masked to `0..=0xFFFF` is a zero-extending cast (matching HLSL's
//!   `int(uint)` value conversion for in-range values), and Rust's `>>` on
//!   `i32` is already an arithmetic (sign-preserving) shift, matching
//!   HLSL's `int` right-shift exactly -- no additional cast needed at the
//!   shift step. This is the exact same idiom `rt64_rsp_patch.rs`'s
//!   `decode_modify_vertex` XYSCREEN case documents as "extracting `u16`
//!   then `as i16`, an identical bit-preserving cast" (a shorter
//!   `as u16 as i16` route to the same sign-extended value the shift idiom
//!   produces) -- this module's tests confirm both routes agree (see
//!   "negative literals... where a Rust oracle and WGSL most easily
//!   diverge", quoting the ticket's own finding). Divisor is `4.0f`,
//!   matching `rt64_rsp_patch.rs`'s XYSCREEN divisor exactly (both are the
//!   RSP's screen-coordinate fixed-point convention: 2 fractional bits, not
//!   the s16.16 or ST's 5-fractional-bit conventions).
//!
//! **`RSPWorldCS` (world-space transform):**
//!
//! - **`pos - vel * (1.0f - frameWeight)`: float subtraction/multiplication
//!   order preserved exactly, not reassociated.** Evaluated component-wise
//!   as `pos[i] - (vel[i] * (1.0 - frame_weight))` -- the `1.0 - frameWeight`
//!   subtraction happens once and is reused for all three components (both
//!   in the source and in this port), then each component's `vel[i] *
//!   that_value` product is subtracted from `pos[i]`. This exact formula is
//!   evaluated **twice** per vertex with two different weights
//!   (`curFrameWeight` for `worldPos`, `prevFrameWeight` for
//!   `prevWorldPos`) -- [`rsp_world_weighted_pos`] takes `frame_weight` as a
//!   parameter precisely so both call sites share one ported formula rather
//!   than two independently-drifting copies, mirroring the source's own
//!   literal duplication (the HLSL itself does not factor this into a
//!   helper; the *formula* is identical at both call sites, ported once
//!   here as a matter of this module's own economy, not as a source-fidelity
//!   requirement -- both call sites still evaluate the identical
//!   expression).
//! - **Matrix multiply order: `mul(worldMats[transformIndex], float4(...))` is
//!   matrix-on-the-left, `M·v`.** Reused via
//!   [`fn64_render_ir::rsp_math::Mat4::transform_point`], whose own doc
//!   comment states this exact convention (`mul(matrix, vector)`: `vector`
//!   as a column vector, `M·v`) -- see "Reuse, not new type" above. Getting
//!   this backwards (`mul_vec_mat`, the `vᵀ·M` shape) would silently
//!   transpose every world-space vertex; this module never calls
//!   `mul_vec_mat` for `worldPos`/`prevWorldPos`/the normal transform, only
//!   `transform_point`.
//! - **The zero-normal guard is an exact-equality branch, `all(norm ==
//!   0.0f)`, evaluated BEFORE the matrix multiply -- not a post-hoc NaN/inf
//!   guard on the result.** `all(...)` requires **every** component
//!   (`norm.x`, `norm.y`, `norm.z`) to compare bit-exactly equal to `0.0f`;
//!   a partially-zero normal (e.g. `(0,0,0.0001)`) takes the transform
//!   branch, not the guard branch. Ported as
//!   `norm.x == 0.0 && norm.y == 0.0 && norm.z == 0.0` in Rust (IEEE-754
//!   equality: `-0.0 == 0.0` is `true` under this comparison, matching
//!   HLSL/D3D's IEEE-754 `==`, so a normal of exactly `(-0.0, 0.0, 0.0)`
//!   also takes the guard branch -- tested explicitly). The guard branch's
//!   result, `float4(0,0,0,1)`, performs no matrix multiply, no
//!   `normalize`, and involves `invTWorldMats[transformIndex]` not at all
//!   -- this module's [`rsp_world_norm`] oracle mirrors that by returning
//!   early before calling `Mat4::transform_point`.
//! - **`normalize(...)` is plain, unguarded Euclidean normalization: `v /
//!   length(v)`, `length(v) = sqrt(x^2+y^2+z^2)`, ordinary IEEE-754
//!   division with no epsilon guard.** This is a different formula from
//!   `fn64_render_ir::rsp_math::compute_length`'s `sqrt(x^2+y^2+2*z^2)` (the
//!   RSP fog/light-specific doubled-Z-term length used elsewhere in this
//!   crate) -- `RSPWorldCS`'s `normalize` is HLSL's generic intrinsic, an
//!   ordinary Euclidean 3-vector normalize, so `compute_length` is
//!   deliberately **not** reused here despite the superficial "vector
//!   length" similarity; conflating the two would silently double the Z
//!   contribution. Ported as [`rsp_world_norm`]'s own `sqrt(x*x+y*y+z*z)`
//!   plus a plain division per component -- for a transformed normal whose
//!   length underflows to exactly `0.0` (reachable only from extreme
//!   `invTWorldMats` values, since the zero-normal guard above already
//!   excludes a raw zero `norm` input), the resulting division is `0.0/0.0`
//!   producing `NaN` in every component, matching plain, unguarded
//!   IEEE-754 semantics on both the HLSL and Rust sides -- this port adds
//!   no epsilon guard the source does not have.
//! - **`worldPos - prevWorldPos` for `dstVel`: plain four-component
//!   subtraction, including the `w` component (`1.0 - 1.0 = 0.0` for any
//!   two well-formed `float4(..., 1.0)` results, but the subtraction is
//!   unconditional on all four components, not clamped to `xyz`).** Ported
//!   as component-wise `f32` subtraction on all four fields; no special
//!   case for `w`.
//! - **`vertexOffsetIndex`/`posIndex`/`normIndex`/`transformIndex` buffer-index
//!   arithmetic is NOT ported** -- see "Nonclaims": this module's oracle
//!   functions take already-extracted `pos: Vec3`/`vel: Vec3`/`norm: Vec3`
//!   and already-selected `world_mat`/`inv_t_world_mat`/`prev_world_mat: Mat4`
//!   parameters directly, standing in for whatever buffer reads a future
//!   integration would perform.
//!
//! ## Nonclaims
//!
//! No GPU execution: the WGSL sibling files
//! (`crates/fn64-render-wgpu/src/shaders/rsp_modify.wgsl`,
//! `crates/fn64-render-wgpu/src/shaders/rsp_world.wgsl`) are validated only
//! through Naga's WGSL front-end and validator (a plain, non-GPU test), not
//! by dispatching either shader on a real adapter/device -- this host does
//! have a real GPU (`host-gpu-tests`) but no such run was performed for this
//! ticket. No production wiring: no pipeline, `wgpu::ShaderModule`, bind
//! group layout, or `targets/` integration is created here, this module has
//! no `pub use` anywhere else in the crate, and it is not referenced from
//! any dispatch path. No parity or performance claim of any kind against
//! RT64's own renderer.
//!
//! Dispatch scaffolding is not ported, in either language: `[numthreads(64,
//! 1, 1)]`/`SV_DispatchThreadID`, the `modifyIndex >= gConstants.modifyCount`
//! / `vertexIndex >= gConstants.vertexCount` early-return guards,
//! `groupshared` (neither shader uses it), barriers (neither shader uses
//! them), and every resource bind/load/store
//! (`srcModifyPos`/`screenPos`/`srcPos`/`srcVel`/`srcNorm`/`srcIndices`/
//! `worldMats`/`invTWorldMats`/`prevWorldMats`/`dstPos`/`dstNorm`/`dstVel`,
//! all twelve `register(t*)`/`register(u*)` bindings across both shaders,
//! and both `RSPModifyCB`/`RSPWorldCB` push-constant structs) are excluded.
//! This module's oracle functions take and return plain values
//! (`ModifyPosDecode`, `RspWorldOutput`, `Vec3`/`Vec4`/`Mat4`) rather than
//! reading or writing any buffer.
//!
//! `RSPModifyCS`'s `modifyOffset = modifyIndex * 2` stride and
//! `RSPWorldCS`'s `vertexOffsetIndex = gConstants.vertexStart + vertexIndex`,
//! `posIndex = vertexOffsetIndex * 3`, `normIndex = vertexOffsetIndex * 4`
//! are all buffer-layout facts, not arithmetic this module characterizes --
//! see "Admitted domain"'s closing bullet.
//!
//! This is a **second, independent port** of the fixed-point vertex-patch
//! semantics [`crate::rt64_rsp_patch`] (M5.6, just landed) already
//! characterizes from the CPU producer side; see "Reuse, not new type" for
//! why this module does not call into it directly. This module also does
//! not touch, extend, or attempt to make `pub` the private `mat4_mul`
//! (matrix-by-matrix product) helper duplicated in
//! `crate::rt64_math_decompose` and `crate::rt64_rsp_matrix_stack` (a
//! visibility gap that ticket's own module doc already names) -- `RSPWorldCS`
//! has no matrix-by-matrix product to port, so this module does not need
//! that helper and does not create a third private copy of it; the gap is
//! simply not this module's concern to fix.
//!
//! `RSPModifyCS.hlsl` and `RSPWorldCS.hlsl` contain no `lerp` or `mix` call
//! of any kind -- there is no HLSL-`lerp`-vs-WGSL-`mix` divergence to admit
//! in this module (unlike `crate::rt64_resample`, which does have one; see
//! that module's doc for the general pattern this module would have
//! followed had either shader used `lerp`).

use fn64_render_ir::{Mat4, Vec3, Vec4};

// ---------------------------------------------------------------------
// RSPModifyCS
// ---------------------------------------------------------------------

/// Decoded `(modifyZ, vertexIndex)` pair from `srcModifyPos[modifyOffset]`
/// (`RSPModifyCS.hlsl:19-20`). See module doc "Admitted domain".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifyPosDecode {
    /// `true` selects the Z branch, `false` the XY branch.
    pub modify_z: bool,
    /// `srcModifyPos[modifyOffset] >> 1`.
    pub vertex_index: u32,
}

/// Literal port of `RSPModifyCS.hlsl:20-21`'s packed-word decode:
/// `modifyZ = tag_and_index & 0x1`, `vertexIndex = tag_and_index >> 1`.
pub fn decode_modify_pos(tag_and_index: u32) -> ModifyPosDecode {
    ModifyPosDecode {
        modify_z: (tag_and_index & 0x1) != 0,
        vertex_index: tag_and_index >> 1,
    }
}

/// Literal port of `RSPModifyCS.hlsl:23`'s Z branch:
/// `screenPos[vertexIndex].z = modifyValue / 65536.0f`. Unsigned `u32 ->
/// f32` widening conversion, NOT a signed s16.16 reinterpret -- see module
/// doc "Admitted domain".
pub fn rsp_modify_z(modify_value: u32) -> f32 {
    modify_value as f32 / 65536.0
}

/// Literal port of `RSPModifyCS.hlsl:26-30`'s XY branch: extract each
/// 16-bit half, sign-extend via the `int(ext) << 16 >> 16` idiom, divide by
/// `4.0f`. Returns `(x, y)`. See module doc "Admitted domain".
pub fn rsp_modify_xy(modify_value: u32) -> (f32, f32) {
    let ext_x = (modify_value >> 16) & 0xFFFF;
    let ext_y = modify_value & 0xFFFF;
    let int_x = ((ext_x as i32) << 16) >> 16;
    let int_y = ((ext_y as i32) << 16) >> 16;
    (int_x as f32 / 4.0, int_y as f32 / 4.0)
}

// ---------------------------------------------------------------------
// RSPWorldCS
// ---------------------------------------------------------------------

/// `RSPWorldCS`'s three `dstPos`/`dstNorm`/`dstVel` outputs for one vertex
/// (`RSPWorldCS.hlsl:39-42`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspWorldOutput {
    pub world_pos: Vec4,
    pub world_norm: Vec4,
    pub world_vel: Vec4,
}

/// Literal port of `RSPWorldCS.hlsl:35,37`'s shared weighted-position
/// formula: `pos - vel * (1.0f - frameWeight)`, evaluated component-wise,
/// used for both `worldPos` (`curFrameWeight`) and `prevWorldPos`
/// (`prevFrameWeight`). See module doc "Admitted domain".
pub fn rsp_world_weighted_pos(pos: Vec3, vel: Vec3, frame_weight: f32) -> Vec3 {
    let one_minus_weight = 1.0 - frame_weight;
    Vec3::new(
        pos.x - vel.x * one_minus_weight,
        pos.y - vel.y * one_minus_weight,
        pos.z - vel.z * one_minus_weight,
    )
}

/// Literal port of `RSPWorldCS.hlsl:36`'s normal-transform ternary:
/// `all(norm == 0.0f) ? float4(0,0,0,1) : float4(normalize(mul(invTWorldMat,
/// float4(norm,0)).xyz), 1)`. See module doc "Admitted domain" for the
/// exact-equality guard semantics and the plain-Euclidean-`normalize`
/// distinction from `fn64_render_ir::rsp_math::compute_length`.
pub fn rsp_world_norm(inv_t_world_mat: Mat4, norm: Vec3) -> Vec4 {
    if norm.x == 0.0 && norm.y == 0.0 && norm.z == 0.0 {
        return Vec4::new(0.0, 0.0, 0.0, 1.0);
    }
    let transformed = inv_t_world_mat.transform_point(Vec4::from_vec3(norm, 0.0));
    let n = transformed.xyz();
    let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
    Vec4::new(n.x / len, n.y / len, n.z / len, 1.0)
}

/// Literal port of `RSPWorldCS.hlsl:29-42`'s full per-vertex body (buffer
/// reads/writes and index arithmetic excluded -- see module doc
/// "Nonclaims"). `pos`/`vel`/`norm` stand in for the already-read
/// `srcPos`/`srcVel`/`srcNorm` triples; `world_mat`/`inv_t_world_mat`/
/// `prev_world_mat` stand in for the already-indexed
/// `worldMats[transformIndex]`/`invTWorldMats[transformIndex]`/
/// `prevWorldMats[transformIndex]` reads.
pub fn rsp_world_transform(
    pos: Vec3,
    vel: Vec3,
    norm: Vec3,
    world_mat: Mat4,
    inv_t_world_mat: Mat4,
    prev_world_mat: Mat4,
    prev_frame_weight: f32,
    cur_frame_weight: f32,
) -> RspWorldOutput {
    let world_pos = world_mat.transform_point(Vec4::from_vec3(
        rsp_world_weighted_pos(pos, vel, cur_frame_weight),
        1.0,
    ));
    let world_norm = rsp_world_norm(inv_t_world_mat, norm);
    let prev_world_pos = prev_world_mat.transform_point(Vec4::from_vec3(
        rsp_world_weighted_pos(pos, vel, prev_frame_weight),
        1.0,
    ));
    let world_vel = Vec4::new(
        world_pos.x - prev_world_pos.x,
        world_pos.y - prev_world_pos.y,
        world_pos.z - prev_world_pos.z,
        world_pos.w - prev_world_pos.w,
    );

    RspWorldOutput {
        world_pos,
        world_norm,
        world_vel,
    }
}

pub const RSP_MODIFY_WGSL: &str = include_str!("shaders/rsp_modify.wgsl");
pub const RSP_WORLD_WGSL: &str = include_str!("shaders/rsp_world.wgsl");

#[cfg(test)]
mod tests;
