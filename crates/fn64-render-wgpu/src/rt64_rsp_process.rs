//! The per-vertex arithmetic of RT64's `RSPProcessCS` compute shader: a
//! literal, characterization-first port of the permitted MIT RT64 source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/RSPProcessCS.hlsl`.
//!
//! SHA-256 of the whole file,
//! `4455dad690da65c0e5d5ddc21fc3df04c4e0bcdad6137ee22e6a9eaa0a3816ec`
//! (133 newline-terminated lines plus a final unterminated line -- the
//! closing `}` -- which the inventory records as 134). That digest was
//! computed independently here with `shasum -a 256` against the pinned
//! checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/shaders/RSPProcessCS.hlsl"].sources.port.sha256`, which
//! records the identical digest -- **no mismatch**. (The inventory's
//! `sources.oracle.sha256` for this path records the same digest and its
//! `port_delta` is `"unchanged"`, so the oracle tree at `f0728a25` and the
//! port tree at `5473732a` agree on this file byte for byte; no delta to
//! detect.)
//!
//! ## Inventory drift, and the ported fraction stated plainly
//!
//! **This is a partial port: 31 of the file's 134 lines carry arithmetic or
//! control flow this module characterizes; 103 lines are refused.** That is
//! roughly 23% of the file. The inventory's whole-file digest marks a source
//! `ported` at *file* granularity, so once
//! `docs/rt64-port-inventory.json` records this module in `ported_as` the
//! burndown will credit all 134 lines. It should not: 77% of this file is
//! GPU dispatch scaffolding, resource bindings, and buffer-index arithmetic
//! with no CPU meaning, and a further slice of the arithmetic that *is*
//! present is delegated to already-landed ports rather than re-derived here
//! (see "Reuse, not new type"). The over-credit is disclosed here because
//! the burndown mechanism is known to over-credit for exactly this reason.
//!
//! This card's writable surface does not include
//! `docs/rt64-port-inventory.json`, whose entry for this path currently
//! reads `"port_state": "not-started"` and `"ported_as": []`.
//! `scripts/lint-docs.py`'s inventory scanner is expected to report a
//! `ported_as` drift until a follow-up regenerates the inventory; that
//! reconciliation is deliberately left to the owning ticket. Note also that
//! the inventory's `task_card.writable_paths` for this path names
//! `crates/fn64-render-wgpu/src/rspprocesscs_hlsl.wgsl`, a WGSL
//! transcription; this card's writable surface is the Rust module instead
//! and no WGSL sibling is written (see "Nonclaims").
//!
//! ## Ported / refused boundary, and the criterion
//!
//! **Criterion**: a construct is ported when (a) its behavior is fully
//! determined by float/integer values and control flow present in the cited
//! file -- no GPU, no dispatch context, no buffer contents -- **and** (b) it
//! is not already characterized by a landed port in this workspace. Clause
//! (b) is what keeps this module from becoming a parallel second copy of
//! four already-ported bodies of RSP math; see "Reuse, not new type".
//!
//! **Ported** (31 lines):
//! - lines 57-61: the `srcNorm[...] / 127.0f` signed-normal decode.
//! - line 66: the *composition* `mul(mul(viewProjMat, worldMat), ...)` --
//!   specifically the matrix-by-matrix product and the order in which the
//!   two matrices combine before the point transform. The `pos - vel *
//!   (1 - curFrameWeight)` operand itself is **reused**, not re-derived.
//! - lines 71-78: the fog branch (`fogIndex > 0`), the fog-alpha formula
//!   `((max(tfPos.z, 0) / tfPos.w) * mul + offset)`, the `/255` clamp, and
//!   the non-fog `srcCol[...] / 255.0f` alpha fallback.
//! - lines 84-93: the `lookAtIndex` bit-field dispatch
//!   (`& ENABLED`, `& LINEAR`, `>> SHIFT`) and the *else* arm's
//!   `tc -= tcVel * (1.0f - curFrameWeight)` texture-coordinate
//!   de-velocity step. `computeTextureGen` itself is **reused**.
//! - lines 98-119: the lighting branch -- the `ambientIndex = lightIndex +
//!   lightCount - 1` derivation, the ambient-seeded accumulation loop, the
//!   `light.kc > 0` positional/directional selector, the `min(resultColor,
//!   1.0f)` saturation, and the unlit `srcCol[...] / 255.0f` fallback.
//!   `computePosLight`/`computeDirLight` themselves are **reused**.
//! - lines 123-125: the `tfPos.w == 0.0f -> 1e-6f` near-clip HACK.
//! - lines 128-130: the NDC divide by `float3(w, -w, w)` and the
//!   `ndcPos * viewport.scale + viewport.translate` screen mapping.
//!
//! **Refused / not modelled** (103 lines), each named:
//! - lines 1-3, 63, 68, 80, 95, 121-122, 127: comments (the HACK comment's
//!   *content* is preserved in "Admitted domain" below, but a comment is
//!   not behavior).
//! - lines 5-10: `#include` directives. Their contents are separately
//!   ported -- see "Reuse, not new type".
//! - line 12: `#define GROUP_SIZE 64`. A dispatch tile width; it selects how
//!   many threads a workgroup runs, not what any one of them computes.
//! - lines 14-19: `struct RSPProcessCB`. A push-constant *layout*. Its two
//!   `float` members are consumed as plain parameters by the ported
//!   functions below; its two `uint` members (`vertexStart`, `vertexCount`)
//!   exist only to index and bound the dispatch and are refused with it.
//!   No `repr(C)` mirror of this struct is created: claiming its byte
//!   offsets would need HLSL/Vulkan push-constant packing rules verified
//!   against a real compile, a GPU concern this card refuses.
//! - line 21: `[[vk::push_constant]] ConstantBuffer<...> : register(b0)`.
//! - lines 22-42: all 21 resource bindings -- `srcPos`, `srcVel`, `srcTc`,
//!   `srcTcVel`, `srcCol`, `srcNorm`, `srcViewProjIndices`,
//!   `srcWorldIndices`, `srcFogIndices`, `srcLightIndices`,
//!   `srcLightCounts`, `srcLookAtIndices`, `rspViewportVector`,
//!   `rspFogVector`, `rspLightVector`, `rspLookAtVector`,
//!   `viewProjTransforms`, `worldTransforms`, `dstPos`, `dstTc`, `dstCol`
//!   (`Buffer<T>`/`StructuredBuffer<T>`/`RWStructuredBuffer<T>` at
//!   `register(t1..t18)`/`register(u19..u21)`). Every ported function below
//!   takes already-loaded plain values in place of these reads.
//! - line 44: `[numthreads(GROUP_SIZE, 1, 1)]`.
//! - line 45: `void CSMain(uint vertexIndex : SV_DispatchThreadID)` -- the
//!   entry point and its dispatch-context system value.
//! - lines 46-48: `if (vertexIndex >= gConstants.vertexCount) return;`, the
//!   out-of-range thread guard. This is dispatch bookkeeping (a dispatch
//!   rounds up to whole workgroups, so trailing threads must self-retire);
//!   it computes nothing.
//! - lines 50-56: `vertexOffsetIndex = vertexStart + vertexIndex`,
//!   `viewProjIndex`/`transformIndex` index loads, `viewProjMat`/`worldMat`
//!   table lookups, `posIndex = vertexOffsetIndex * 3`, `normColIndex =
//!   vertexOffsetIndex * 4`. Buffer-layout strides and indirections, not
//!   arithmetic with observable float semantics.
//! - lines 64-65, 69, 81-83 (loads), 87, 96-97, 100, 102, 128 (load): the
//!   individual buffer *reads* that materialize `pos`, `vel`, `fogIndex`,
//!   `tc`, `tcIndex`, `lookAt`, `lightIndex`, `lightCount`, the light
//!   records and `rspViewport`. The values are parameters here; the reads
//!   are not.
//! - lines 131-133: the three `dstPos`/`dstTc`/`dstCol` stores. This module
//!   returns an [`RspProcessOutput`] value instead.
//! - line 134 and the closing braces: syntax.
//!
//! ## Reuse, not new type
//!
//! Four bodies of arithmetic this shader invokes are **already ported** in
//! this workspace, and this module calls them rather than growing a second,
//! independently-drifting copy of each. This is the single largest reason
//! the ported fraction is 23% rather than higher, and it is deliberate:
//!
//! 1. **`pos - vel * (1.0f - gConstants.curFrameWeight)` (line 66's inner
//!    operand)** is character-for-character the formula
//!    [`crate::rt64_rsp_world_modify::rsp_world_weighted_pos`] already ports
//!    from `RSPWorldCS.hlsl:35,37` -- same subtraction, same
//!    `1.0 - frame_weight` computed once and reused across all three
//!    components, same `pos[i] - (vel[i] * that)` association. This module
//!    calls it. (`RSPProcessCS` evaluates it once with `curFrameWeight`;
//!    `RSPWorldCS` evaluates it twice, with `cur` and `prev`. The
//!    parameterized helper covers both.)
//! 2. **`computeTextureGen` / `normalizeSafe`** (`TextureGen.hlsli:9-34`,
//!    reached through line 88) are ported in
//!    [`crate::texture_gen::compute_texture_gen`] (inventory: `"port_state":
//!    "ported"`, `ported_as` `crates/fn64-render-wgpu/src/texture_gen.rs`,
//!    M4). This module calls it, converting `Vec3`/`Mat4` to that module's
//!    `[f32; 3]`/[`crate::texture_gen::WorldMatrix`] row-major shapes at the
//!    boundary -- a pure repacking, no arithmetic.
//! 3. **`computePosLight` / `computeDirLight` / `computeAttenuation` /
//!    `computeNDotL` / `computeLength`** (`rt64_rsp_light.h:22-63`, reached
//!    through lines 104 and 107) are ported in
//!    `crates/fn64-render-ir/src/rsp_math.rs` (inventory: `"port_state":
//!    "ported"`, M5) and re-exported as
//!    [`fn64_render_ir::compute_pos_light`] /
//!    [`fn64_render_ir::compute_dir_light`]. This module calls them. It
//!    notably does **not** re-derive `computeLength`'s doubled-Z-term
//!    formula `sqrt(x^2+y^2+2z^2)`, which is easy to confuse with an
//!    ordinary Euclidean length (the same trap
//!    `rt64_rsp_world_modify.rs`'s doc already names).
//! 4. **`struct RSPFog` / `RSPLight` / `RSPLookAt` / `RSPViewport` and the
//!    `RSP_LOOKAT_INDEX_*` constants** (lines 5-8's headers) are ported as
//!    [`fn64_render_ir::RspFog`], [`fn64_render_ir::RspLight`],
//!    [`fn64_render_ir::RspLookAt`], [`fn64_render_ir::RspViewport`],
//!    [`fn64_render_ir::RSP_LOOKAT_INDEX_ENABLED`],
//!    [`fn64_render_ir::RSP_LOOKAT_INDEX_LINEAR`] and
//!    [`fn64_render_ir::RSP_LOOKAT_INDEX_SHIFT`]. This module defines **no**
//!    new struct for any of them and hard-codes none of the three constants.
//!
//! [`fn64_render_ir::Vec3`] / [`fn64_render_ir::Vec4`] / [`fn64_render_ir::Mat4`]
//! are reused for HLSL `float3`/`float4`/`float4x4`, and
//! [`fn64_render_ir::Mat4::transform_point`] implements exactly the
//! `mul(matrix, vector)` = `M·v` call shape line 66 needs. (HLSL `float2`
//! has no established mirror in this workspace and only two values in this
//! shader are `float2` -- `tc` and `tcVel` -- so those are carried as plain
//! `[f32; 2]`, matching `texture_gen.rs`'s own choice for the same values,
//! rather than introducing a `Vec2` type this port does not need.)
//!
//! The one matrix-by-matrix product line 66 does need is **not** available
//! for reuse: `mat4_mul` is a *private* helper duplicated in
//! `crate::rt64_math_decompose` and `crate::rt64_rsp_matrix_stack` (a
//! visibility gap `rt64_rsp_world_modify.rs`'s doc already names and
//! deliberately does not fix). This module does not widen either one's
//! visibility and does not add a third private copy either; instead
//! [`rsp_process_composed_matrix`] performs the product inline as the
//! literal four `transform_point`-shaped row-times-column dot products the
//! HLSL `mul(float4x4, float4x4)` intrinsic specifies, expressed as four
//! calls to the already-reused [`fn64_render_ir::Mat4::transform_point`] on
//! the *columns* of the right operand. See "Admitted domain" for why that
//! composition is exactly `A·B` and not `B·A`.
//!
//! ## Verbatim key logic
//!
//! ```text
//! // RSPProcessCS.hlsl lines 57-66
//!     const float3 norm = float3(
//!         srcNorm[normColIndex + 0] / 127.0f,
//!         srcNorm[normColIndex + 1] / 127.0f,
//!         srcNorm[normColIndex + 2] / 127.0f
//!     );
//!
//!     // NDC Position.
//!     const float3 pos = float3(srcPos[posIndex + 0], srcPos[posIndex + 1], srcPos[posIndex + 2]);
//!     const float3 vel = float3(srcVel[posIndex + 0], srcVel[posIndex + 1], srcVel[posIndex + 2]);
//!     float4 tfPos = mul(mul(viewProjMat, worldMat), float4(pos - vel * (1.0f - gConstants.curFrameWeight), 1.0f));
//!
//!     // Fog.
//!     const uint fogIndex = srcFogIndices[vertexOffsetIndex];
//!     float4 vertexColor;
//!     if (fogIndex > 0) {
//!         const RSPFog rspFog = rspFogVector[fogIndex - 1];
//!         const float fogAlpha = ((max(tfPos.z, 0) / tfPos.w) * rspFog.mul + rspFog.offset);
//!         vertexColor.a = clamp(fogAlpha / 255.0f, 0.0f, 1.0f);
//!     }
//!     else {
//!         vertexColor.a = srcCol[normColIndex + 3] / 255.0f;
//!     }
//!
//!     // Texgen.
//!     const uint tcIndex = vertexOffsetIndex * 2;
//!     const uint lookAtIndex = srcLookAtIndices[vertexOffsetIndex];
//!     float2 tc = float2(srcTc[tcIndex + 0], srcTc[tcIndex + 1]);
//!     if (lookAtIndex & RSP_LOOKAT_INDEX_ENABLED) {
//!         const bool textureGenLinear = lookAtIndex & RSP_LOOKAT_INDEX_LINEAR;
//!         const uint extractedIndex = (lookAtIndex >> RSP_LOOKAT_INDEX_SHIFT);
//!         const RSPLookAt lookAt = rspLookAtVector[extractedIndex];
//!         tc = computeTextureGen(tc, norm, lookAt, textureGenLinear, worldMat);
//!     }
//!     else {
//!         float2 tcVel = float2(srcTcVel[tcIndex + 0], srcTcVel[tcIndex + 1]);
//!         tc -= tcVel * (1.0f - gConstants.curFrameWeight);
//!     }
//!
//!     // Lighting.
//!     const uint lightIndex = srcLightIndices[vertexOffsetIndex];
//!     const uint lightCount = srcLightCounts[vertexOffsetIndex];
//!     if (lightCount > 0) {
//!         const uint ambientIndex = lightIndex + lightCount - 1;
//!         float3 resultColor = rspLightVector[ambientIndex].col;
//!         for (uint i = lightIndex; i < ambientIndex; i++) {
//!             const RSPLight light = rspLightVector[i];
//!             if (light.kc > 0) {
//!                 resultColor += computePosLight(pos, norm, light, worldMat);
//!             }
//!             else {
//!                 resultColor += computeDirLight(norm, light, worldMat);
//!             }
//!         }
//!
//!         vertexColor.rgb = min(resultColor, 1.0f);
//!     }
//!     else {
//!         vertexColor.rgb = float3(
//!             srcCol[normColIndex + 0] / 255.0f,
//!             srcCol[normColIndex + 1] / 255.0f,
//!             srcCol[normColIndex + 2] / 255.0f
//!         );
//!     }
//!
//!     // HACK: For handling geometry exactly at the near clip plane of the viewport. This hack can
//!     // probably be removed once the behavior of the RSP has been reviewed in this case.
//!     if (tfPos.w == 0.0f) {
//!         tfPos.w = 1e-6f;
//!     }
//!
//!     // Convert to N64 screen position.
//!     const RSPViewport rspViewport = rspViewportVector[viewProjIndex];
//!     const float3 ndcPos = tfPos.xyz / float3(tfPos.w, -tfPos.w, tfPos.w);
//!     const float4 screenPos = float4(ndcPos * rspViewport.scale + rspViewport.translate, tfPos.w);
//! ```
//!
//! ## Admitted domain
//!
//! - **Normal decode divides by `127.0f`, not `128.0f`, and the source
//!   buffer is `Buffer<int>` -- signed.** `srcNorm` is declared
//!   `Buffer<int> : register(t6)` (line 27), so each component is a 32-bit
//!   *signed* integer; `srcNorm[i] / 127.0f` is therefore a signed
//!   int-to-float conversion followed by a float divide, and the natural
//!   `-128..=127` s8 input range maps to `-1.0079…..=1.0`, i.e. the
//!   most-negative value produces a magnitude slightly **greater than 1**.
//!   This port does not clamp it: [`rsp_process_norm`] takes `[i32; 3]` and
//!   emits `c as f32 / 127.0` per component with no range guard, exactly as
//!   written. (A `/128.0` divisor would have kept the result inside the unit
//!   interval; the source does not use one, and this is a genuine
//!   upstream oddity pinned rather than fixed. `computeNDotL` downstream
//!   clamps the *dot product*, not the normal, so the >1 magnitude does
//!   survive into `computeTextureGen`'s dot products unclamped -- and
//!   `computeTextureGen`'s own `[-1,1]` clamp is what bounds it there.)
//! - **Line 66's `mul(mul(viewProjMat, worldMat), v)` composes
//!   view-projection on the LEFT: the effective transform is
//!   `(VP · W) · v`, i.e. world applies first, then view-projection.** HLSL
//!   `mul(A, B)` for two `float4x4` operands is the ordinary matrix product
//!   `A·B` (`(A·B)[i][j] = sum_k A[i][k] * B[k][j]`), and `mul(M, v)` for a
//!   `float4` is `M·v` with `v` a column vector. So the composite acts on
//!   `v` as `VP·(W·v)`. Getting the two matrices' order backwards would
//!   silently apply the camera before the model transform on every vertex.
//!   [`rsp_process_composed_matrix`] takes `(view_proj_mat, world_mat)` in
//!   that argument order and returns `VP·W`; its tests pin the
//!   non-commutativity explicitly with a rotate-then-translate pair whose
//!   two orders differ.
//! - **`max(tfPos.z, 0)` is HLSL's `max`, which returns its FIRST argument
//!   when the comparison is false -- so `max(NaN, 0)` is `0`, where Rust's
//!   `f32::max(NaN, 0.0)` is `0.0` too, but the two disagree elsewhere.**
//!   HLSL `max(a, b)` is specified as `a < b ? b : a`. With `a = NaN` the
//!   comparison `NaN < 0` is false, so the result is `a` = `NaN`. (Rust's
//!   `f32::max` is NaN-*suppressing*: `f32::NAN.max(0.0)` returns `0.0`.)
//!   The two therefore **disagree** on a NaN `tfPos.z`. This port writes the
//!   literal ternary `if z < 0.0 { 0.0 } else { z }` in
//!   [`rsp_process_fog_alpha`], preserving the HLSL argument order exactly,
//!   so a NaN `z` propagates as NaN rather than being silently replaced by
//!   `0.0`. Tested directly.
//! - **`min(resultColor, 1.0f)` likewise returns its FIRST argument when the
//!   comparison is false, so a NaN accumulated color stays NaN.** HLSL
//!   `min(a, b)` is `b < a ? b : a`; with `a = NaN` the comparison `1.0 <
//!   NaN` is false, so the result is `NaN`. Rust's `f32::min(NaN, 1.0)`
//!   returns `1.0`. [`rsp_process_saturate_color`] writes the literal
//!   ternary `if 1.0 < c { 1.0 } else { c }` per component -- same argument
//!   order as the source -- and is tested against a NaN component. Note the
//!   source's `min` has **no lower bound**: a light accumulation that goes
//!   negative (reachable only through negative `light.col` values, since
//!   both `computePosLight` and `computeDirLight` clamp their scalar weight
//!   to `>= 0`) is *not* clamped up to `0`. This is `min`, not `saturate`;
//!   the function name below says `saturate_color` for the role it plays,
//!   but its body is the one-sided `min` the source actually writes, and a
//!   test pins that a negative component passes through unchanged.
//! - **`clamp(fogAlpha / 255.0f, 0.0f, 1.0f)` expands to `min(max(x, 0), 1)`
//!   and is therefore NaN-COLLAPSING, unlike the two bare `min`/`max` calls
//!   above.** HLSL's `clamp(x, lo, hi)` is defined as `min(max(x, lo), hi)`.
//!   Substituting the ternaries: `max(NaN, 0)` = `NaN < 0 ? 0 : NaN` = `NaN`
//!   (false comparison returns the first argument, `NaN`); then `min(NaN, 1)`
//!   = `1 < NaN ? 1 : NaN` = `NaN`. So HLSL's `clamp` **also** propagates
//!   NaN here. Rust's `f32::clamp` propagates NaN as well (`NaN.clamp(0.0,
//!   1.0)` is `NaN`), so the two agree -- but this port still writes the
//!   nested literal ternaries rather than calling `f32::clamp`, because the
//!   agreement is a coincidence of this particular bound ordering and not a
//!   property either language guarantees for the composition. Tested with a
//!   NaN input.
//! - **The fog divide `tfPos.z / tfPos.w` has NO zero guard, and the near-clip
//!   HACK that would supply one runs LATER (line 123, after the fog block at
//!   line 73).** So a vertex with `tfPos.w == 0.0` computes its fog alpha
//!   against a `w` of exactly zero: `max(z,0) / 0.0` is `+Inf` for positive
//!   `z`, and `0.0 / 0.0` = `NaN` when `z <= 0` (because `max(z,0)` has
//!   already floored a non-positive `z` to `+0.0`). The `+Inf` case then
//!   flows through `* mul + offset` and the clamp to `1.0` (or to `-Inf` and
//!   thence `0.0` for a negative `rspFog.mul`); the `NaN` case survives the
//!   clamp as `NaN` per the bullet above. Only *after* the fog block does
//!   line 123 rewrite `w` to `1e-6f` for the screen-position math. This
//!   ordering is preserved exactly: [`rsp_process_vertex`] calls
//!   [`rsp_process_fog_alpha`] with the **un-patched** `tf_pos.w` and applies
//!   [`rsp_process_near_clip_w`] afterwards. Both the `+Inf` and the `NaN`
//!   outcomes are pinned by tests rather than guarded away.
//! - **The near-clip HACK tests `w == 0.0f` by exact float equality, so it
//!   fires for `-0.0` as well as `+0.0` and replaces both with a
//!   POSITIVE `1e-6f`.** IEEE-754 `-0.0 == 0.0` is `true`, so a vertex whose
//!   `w` underflowed to negative zero is rewritten to `+1e-6`, flipping the
//!   sign of the subsequent perspective divide relative to a `w` of, say,
//!   `-1e-30` (which is *not* equal to zero and is left alone). This
//!   signed-zero asymmetry is upstream behavior, pinned by test, not fixed.
//!   The guard also does nothing for a subnormal-but-nonzero `w`, and
//!   nothing for a NaN `w` (`NaN == 0.0` is false), so a NaN `w` reaches the
//!   perspective divide unmodified and produces NaN in all three NDC
//!   components -- also tested.
//! - **The NDC divide negates only Y, and does so by negating the DIVISOR,
//!   not the dividend: `tfPos.xyz / float3(tfPos.w, -tfPos.w, tfPos.w)`.**
//!   For finite nonzero `w` this is numerically identical to negating the
//!   quotient, but it is *not* identical for signed zero: with `y = +0.0`
//!   and `w = +1.0`, `y / -w` is `-0.0`, whereas `-(y / w)` is also `-0.0`
//!   -- these agree -- but with `y = +0.0` and the divisor path taken at
//!   `w = 1e-6` (post-HACK), the sign of the resulting zero follows the
//!   divisor's sign in both formulations. The port writes the source's
//!   divisor-negation form literally (`tf_pos.y / -w`) rather than
//!   `-(tf_pos.y / w)`, so no reasoning about their equivalence is required.
//!   Signed-zero output is pinned by test.
//! - **The screen mapping is fused-free `ndc * scale + translate`,
//!   component-wise, and the output's `w` is the (possibly HACK-patched)
//!   `tfPos.w`, NOT `1.0`.** Line 130's fourth component is `tfPos.w`, so
//!   the emitted `screenPos` carries the perspective `w` forward for a later
//!   stage's use. Written as `ndc.x * scale.x + translate.x` per component
//!   with no `mul_add`/FMA: Rust's `f32::mul_add` would contract the two
//!   operations into a single rounding step, which HLSL's `*` followed by
//!   `+` does not guarantee, so this port uses two separately-rounded
//!   operations. (HLSL compilers *may* contract; the port pins the
//!   unfused reading because that is what the two written operators mean in
//!   isolation. See "Nonclaims".)
//! - **`fogIndex > 0` selects fog, and the table index is `fogIndex - 1`:
//!   index `0` is a reserved "no fog" sentinel, so the fog table is
//!   one-based on the wire.** [`rsp_process_vertex`] takes an
//!   `Option<RspFog>` rather than a raw index plus table, moving the
//!   sentinel decode to [`rsp_process_fog_index`], a one-line function that
//!   pins `0 -> None` and `n -> Some(n - 1)`. The `- 1` on a `u32` is
//!   unreachable for `n == 0` because that case returns first, so no
//!   underflow is possible; this is checked by construction rather than by a
//!   `saturating_sub` the source does not have.
//! - **`const bool textureGenLinear = lookAtIndex & RSP_LOOKAT_INDEX_LINEAR;`
//!   is an implicit uint-to-bool conversion, i.e. `!= 0`.** HLSL converts a
//!   nonzero integer to `true`. Since the mask isolates a single bit
//!   (`0x2`), the result is that bit's value. Ported as `(look_at_index &
//!   RSP_LOOKAT_INDEX_LINEAR) != 0`. Likewise the `if (lookAtIndex &
//!   RSP_LOOKAT_INDEX_ENABLED)` condition on line 84.
//! - **`extractedIndex = lookAtIndex >> RSP_LOOKAT_INDEX_SHIFT` shifts by 2,
//!   discarding BOTH flag bits -- the LINEAR flag is not part of the index.**
//!   With `RSP_LOOKAT_INDEX_SHIFT == 2` the two low bits (`ENABLED = 0x1`,
//!   `LINEAR = 0x2`) are exactly the discarded ones. Ported as a plain
//!   logical `>>` on `u32`. Note the shift is applied to the *whole* word
//!   with no prior masking, so the extracted index is
//!   `lookAtIndex / 4` for any input.
//! - **`ambientIndex = lightIndex + lightCount - 1`, and the loop runs
//!   `i = lightIndex; i < ambientIndex` -- so the LAST light in the range is
//!   the ambient term and is NOT iterated.** The loop body executes
//!   `lightCount - 1` times, and the ambient light's `col` seeds
//!   `resultColor` before the loop. For `lightCount == 1` the loop body
//!   never executes and the result is the ambient color alone; that
//!   degenerate case is tested. The `- 1` is safe because the whole block is
//!   guarded by `lightCount > 0`. [`rsp_process_lighting`] takes the light
//!   slice already sliced to `[lightIndex ..= ambientIndex]` and treats its
//!   **last** element as ambient and the rest as the loop range, which is
//!   the same partition; the raw `lightIndex`/`lightCount` index arithmetic
//!   that produces that slice is refused with the other buffer indexing, but
//!   the *partition rule* it encodes is ported and tested (an empty slice is
//!   the `lightCount == 0` case and is represented as `None`, not as an
//!   empty slice, so [`rsp_process_lighting`] never indexes out of bounds).
//! - **`light.kc > 0` selects the POSITIONAL path, and `kc` is a `uint`, so
//!   the comparison is an unsigned `!= 0`.** `RSPLight::kc` is `uint`
//!   (`rt64_rsp_light.h:16`), mirrored as `u32` in
//!   [`fn64_render_ir::RspLight`], so `> 0` and `!= 0` coincide -- there is
//!   no negative `kc` to worry about. Ported as `light.kc > 0` verbatim.
//! - **The light accumulation is a running `+=` in ascending index order,
//!   seeded by the ambient color.** Float addition is not associative, so
//!   the accumulation order is observable; this port iterates the slice
//!   front-to-back and folds with `+` in that order, matching the source's
//!   `for (uint i = lightIndex; i < ambientIndex; i++)`. A test with three
//!   lights whose magnitudes differ by more than 2^24 pins that the order is
//!   the source's and not, say, a sorted or reversed one.
//! - **The unlit and non-fog color fallbacks read the SAME `srcCol` element
//!   base (`normColIndex`) that the normal decode uses, but divide by
//!   `255.0f` where the normal divides by `127.0f`, and `srcCol` is
//!   `Buffer<uint>` -- unsigned -- where `srcNorm` is `Buffer<int>`.** So
//!   the color path is an unsigned widening conversion over `0..=255`
//!   yielding `0.0..=1.0`, while the normal path is a signed conversion over
//!   `-128..=127` yielding `-1.0079…..=1.0`. Two different divisors and two
//!   different signednesses on adjacent lines; [`rsp_process_color_bytes`]
//!   takes `[u32; 4]` and [`rsp_process_norm`] takes `[i32; 3]` so the type
//!   system keeps them apart. Alpha is element `+3` of the same quad; RGB
//!   are `+0`, `+1`, `+2`.
//!
//! ## Nonclaims
//!
//! No GPU execution and no WGSL sibling. Unlike
//! `crate::rt64_rsp_world_modify` and `crate::texture_gen`, this module
//! writes **no** `.wgsl` transcription and therefore performs no Naga
//! validation -- there is no WGSL artifact here to validate, and none is
//! claimed. No pipeline, `wgpu::ShaderModule`, bind group layout, dispatch,
//! or `targets/` integration is created; this module has no `pub use`
//! anywhere else in the crate and is not referenced from any draw path. No
//! parity, pixel, silicon, or performance claim of any kind against RT64's
//! own renderer or against N64 hardware.
//!
//! **This is a ~23% port of the cited file** and no claim is made about the
//! 103 refused lines -- see "Ported / refused boundary" for the itemized
//! list. In particular, nothing here characterizes the shader's dispatch
//! geometry, its resource binding model, its buffer layouts and strides, or
//! the `RSPProcessCB` push-constant packing.
//!
//! **Floating-point contraction is not claimed either way.** The port
//! expresses `a * b + c` as two separately-rounded operations, never
//! `f32::mul_add`. A real HLSL compiler is permitted (and on most targets
//! likely) to contract those into an FMA, which rounds once and can differ
//! in the last bit. Any future bit-exact comparison against a compiled
//! `RSPProcessCS` must account for that; this port pins the unfused
//! reading only.
//!
//! **Transcendental bit-exactness is not claimed.** The texgen linear path
//! reached through [`crate::texture_gen::compute_texture_gen`] calls
//! `acos`, whose last-bit result is implementation-defined in both HLSL and
//! Rust. That is `texture_gen.rs`'s admitted domain, not re-litigated here;
//! this module's own tests avoid asserting on `acos` outputs to more
//! precision than the shared epsilon.
//!
//! **The `f32` vs. HLSL `float` precision equivalence is assumed, not
//! proven.** HLSL `float` is IEEE-754 binary32 under D3D's rules, matching
//! Rust `f32`, but D3D also permits relaxed precision for some intrinsics
//! (notably `rcp`-style reciprocal substitution for division). Where the
//! source writes `/`, this port writes `/`.
//!
//! **No UB is reproduced and none was found.** Every branch in the ported
//! region is total over its input domain: the two `- 1` subtractions are
//! both guarded by a preceding `> 0` test, no array is indexed outside a
//! bound this module controls, and the unguarded divisions produce
//! IEEE-754 `Inf`/`NaN` rather than trapping. No deviation from the source
//! was necessary, so no test in this module pins a deviation.

use fn64_render_ir::{
    compute_dir_light, compute_pos_light, Mat4, RspFog, RspLight, RspLookAt, RspViewport, Vec3,
    Vec4, RSP_LOOKAT_INDEX_ENABLED, RSP_LOOKAT_INDEX_LINEAR, RSP_LOOKAT_INDEX_SHIFT,
};

use crate::rt64_rsp_world_modify::rsp_world_weighted_pos;
use crate::texture_gen::{compute_texture_gen, RspLookAt as TexGenLookAt, WorldMatrix};

// ---------------------------------------------------------------------
// Leaf arithmetic (lines 57-61, 66, 71-78, 84-93, 98-119, 123-125, 128-130)
// ---------------------------------------------------------------------

/// Literal port of `RSPProcessCS.hlsl:57-61`'s normal decode:
/// `srcNorm[i] / 127.0f` per component, over a `Buffer<int>` (signed).
///
/// Deliberately unclamped: an input of `-128` yields `-1.0078740…`, a
/// magnitude greater than one. See module doc "Admitted domain".
pub fn rsp_process_norm(src_norm: [i32; 3]) -> Vec3 {
    Vec3::new(
        src_norm[0] as f32 / 127.0,
        src_norm[1] as f32 / 127.0,
        src_norm[2] as f32 / 127.0,
    )
}

/// Literal port of the `srcCol[normColIndex + n] / 255.0f` conversions on
/// `RSPProcessCS.hlsl:77` (alpha) and `:115-117` (RGB), over a
/// `Buffer<uint>` (unsigned). Returns `[r, g, b, a]`.
///
/// Note the divisor is `255.0f` here and `127.0f` in [`rsp_process_norm`],
/// and the source buffer's signedness differs too. See module doc
/// "Admitted domain".
pub fn rsp_process_color_bytes(src_col: [u32; 4]) -> Vec4 {
    Vec4::new(
        src_col[0] as f32 / 255.0,
        src_col[1] as f32 / 255.0,
        src_col[2] as f32 / 255.0,
        src_col[3] as f32 / 255.0,
    )
}

/// The matrix-by-matrix half of `RSPProcessCS.hlsl:66`'s
/// `mul(mul(viewProjMat, worldMat), ...)`: the HLSL `mul(A, B)` product
/// `A·B`, with `view_proj_mat` as `A` and `world_mat` as `B`.
///
/// `(A·B)[i][j] = sum_k A[i][k] * B[k][j]`, summed in ascending `k` -- the
/// same accumulation order [`Mat4::transform_point`] uses, which is why each
/// output row is expressible as `A.transform_point(column_j_of_B)`
/// re-gathered by column. See module doc "Admitted domain" for why the
/// operand order matters and "Reuse, not new type" for why `mat4_mul` is not
/// called.
pub fn rsp_process_composed_matrix(view_proj_mat: Mat4, world_mat: Mat4) -> Mat4 {
    let b = world_mat;
    // Columns of B, gathered so each `transform_point` computes one column
    // of the product: `A · B[:, j]`.
    let col0 = Vec4::new(b.rows[0].x, b.rows[1].x, b.rows[2].x, b.rows[3].x);
    let col1 = Vec4::new(b.rows[0].y, b.rows[1].y, b.rows[2].y, b.rows[3].y);
    let col2 = Vec4::new(b.rows[0].z, b.rows[1].z, b.rows[2].z, b.rows[3].z);
    let col3 = Vec4::new(b.rows[0].w, b.rows[1].w, b.rows[2].w, b.rows[3].w);

    let p0 = view_proj_mat.transform_point(col0);
    let p1 = view_proj_mat.transform_point(col1);
    let p2 = view_proj_mat.transform_point(col2);
    let p3 = view_proj_mat.transform_point(col3);

    Mat4::from_rows([
        Vec4::new(p0.x, p1.x, p2.x, p3.x),
        Vec4::new(p0.y, p1.y, p2.y, p3.y),
        Vec4::new(p0.z, p1.z, p2.z, p3.z),
        Vec4::new(p0.w, p1.w, p2.w, p3.w),
    ])
}

/// Literal port of `RSPProcessCS.hlsl:66` in full: compose the two matrices,
/// then transform `float4(pos - vel * (1 - curFrameWeight), 1.0f)`.
///
/// The weighted-position operand is **reused** from
/// [`rsp_world_weighted_pos`] rather than re-derived; see module doc
/// "Reuse, not new type".
pub fn rsp_process_tf_pos(
    view_proj_mat: Mat4,
    world_mat: Mat4,
    pos: Vec3,
    vel: Vec3,
    cur_frame_weight: f32,
) -> Vec4 {
    let composed = rsp_process_composed_matrix(view_proj_mat, world_mat);
    let weighted = rsp_world_weighted_pos(pos, vel, cur_frame_weight);
    composed.transform_point(Vec4::from_vec3(weighted, 1.0))
}

/// Literal port of `RSPProcessCS.hlsl:71-72`'s one-based fog-table sentinel:
/// `fogIndex > 0` selects fog and reads `rspFogVector[fogIndex - 1]`.
///
/// Returns the zero-based table index, or `None` for the `0` sentinel. The
/// `- 1` cannot underflow because `0` returns first.
pub fn rsp_process_fog_index(fog_index: u32) -> Option<u32> {
    if fog_index > 0 {
        Some(fog_index - 1)
    } else {
        None
    }
}

/// Literal port of `RSPProcessCS.hlsl:73-74`'s fog alpha:
/// `clamp(((max(tfPos.z, 0) / tfPos.w) * rspFog.mul + rspFog.offset) / 255.0f, 0.0f, 1.0f)`.
///
/// `max` and `clamp` are written as the literal HLSL ternaries with the
/// source's argument order preserved, **not** as `f32::max`/`f32::clamp`;
/// `tf_pos_w` is the **un-patched** `w` (the near-clip HACK on line 123 runs
/// later) and the divide has no zero guard. See module doc "Admitted
/// domain".
pub fn rsp_process_fog_alpha(tf_pos_z: f32, tf_pos_w: f32, fog: RspFog) -> f32 {
    // HLSL `max(a, b)` == `a < b ? b : a`; here `a = tfPos.z`, `b = 0`.
    let clamped_z = if tf_pos_z < 0.0 { 0.0 } else { tf_pos_z };
    let fog_alpha = (clamped_z / tf_pos_w) * fog.mul + fog.offset;
    let x = fog_alpha / 255.0;
    // HLSL `clamp(x, lo, hi)` == `min(max(x, lo), hi)`, expanded literally:
    // `max(x, 0)` == `x < 0 ? 0 : x`, then `min(.., 1)` == `1 < .. ? 1 : ..`.
    let lower = if x < 0.0 { 0.0 } else { x };
    if 1.0 < lower {
        1.0
    } else {
        lower
    }
}

/// Decoded form of `RSPProcessCS.hlsl:84-87`'s `lookAtIndex` bit field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LookAtIndexDecode {
    /// `lookAtIndex & RSP_LOOKAT_INDEX_ENABLED` (line 84), as a bool.
    pub enabled: bool,
    /// `lookAtIndex & RSP_LOOKAT_INDEX_LINEAR` (line 85), as a bool.
    pub linear: bool,
    /// `lookAtIndex >> RSP_LOOKAT_INDEX_SHIFT` (line 86). Both flag bits are
    /// discarded by the shift.
    pub extracted_index: u32,
}

/// Literal port of `RSPProcessCS.hlsl:84-86`'s bit-field decode. The two
/// mask tests are HLSL implicit uint-to-bool conversions (`!= 0`); the shift
/// is a plain logical `>>` applied to the unmasked word. Constants are
/// reused from [`fn64_render_ir`], not re-declared.
pub fn rsp_process_look_at_index(look_at_index: u32) -> LookAtIndexDecode {
    LookAtIndexDecode {
        enabled: (look_at_index & RSP_LOOKAT_INDEX_ENABLED) != 0,
        linear: (look_at_index & RSP_LOOKAT_INDEX_LINEAR) != 0,
        extracted_index: look_at_index >> RSP_LOOKAT_INDEX_SHIFT,
    }
}

/// Literal port of `RSPProcessCS.hlsl:91-92`'s non-texgen branch:
/// `tc -= tcVel * (1.0f - gConstants.curFrameWeight)`, component-wise on
/// `float2`.
///
/// This is the same shape as [`rsp_world_weighted_pos`] but on two
/// components rather than three; that helper takes a `Vec3` and there is no
/// `Vec2` in this workspace, so the two-component form is written out here
/// rather than padding a dummy `z` through the three-component helper (which
/// would compute and discard a third product). `1.0 - cur_frame_weight` is
/// computed once and shared by both components, exactly as HLSL's
/// scalar-times-vector `tcVel * scalar` does.
pub fn rsp_process_tc_velocity(tc: [f32; 2], tc_vel: [f32; 2], cur_frame_weight: f32) -> [f32; 2] {
    let one_minus_weight = 1.0 - cur_frame_weight;
    [
        tc[0] - tc_vel[0] * one_minus_weight,
        tc[1] - tc_vel[1] * one_minus_weight,
    ]
}

/// Literal port of `RSPProcessCS.hlsl:111`'s `min(resultColor, 1.0f)`.
///
/// HLSL `min(a, b)` == `b < a ? b : a`; here `a` is the component and `b` is
/// `1.0f`, so the ternary is `1.0 < c ? 1.0 : c` and a NaN component
/// survives. There is **no** lower bound -- this is `min`, not `saturate`.
/// See module doc "Admitted domain".
pub fn rsp_process_saturate_color(result_color: Vec3) -> Vec3 {
    let one_sided_min = |c: f32| if 1.0 < c { 1.0 } else { c };
    Vec3::new(
        one_sided_min(result_color.x),
        one_sided_min(result_color.y),
        one_sided_min(result_color.z),
    )
}

/// Literal port of `RSPProcessCS.hlsl:99-111`'s lighting accumulation.
///
/// `lights` is the already-sliced `rspLightVector[lightIndex ..
/// lightIndex + lightCount]` range (the index arithmetic that produces it is
/// refused -- see module doc "Nonclaims"). Its **last** element is the
/// ambient term seeding `resultColor`, and the preceding elements are the
/// loop range `[lightIndex, ambientIndex)` in ascending order. `lights` must
/// be non-empty; the `lightCount == 0` case is the caller's `else` branch.
///
/// # Panics
///
/// Panics if `lights` is empty. The source's `lightCount > 0` guard makes
/// that unreachable; this port surfaces it rather than silently returning a
/// value the source never produces here.
pub fn rsp_process_lighting(lights: &[RspLight], pos: Vec3, norm: Vec3, world_mat: Mat4) -> Vec3 {
    let (ambient, directional_and_positional) = lights
        .split_last()
        .expect("RSPProcessCS lighting requires lightCount > 0");
    let mut result_color = ambient.col;
    for light in directional_and_positional {
        let contribution = if light.kc > 0 {
            compute_pos_light(pos, norm, *light, world_mat)
        } else {
            compute_dir_light(norm, *light, world_mat)
        };
        result_color = Vec3::new(
            result_color.x + contribution.x,
            result_color.y + contribution.y,
            result_color.z + contribution.z,
        );
    }

    rsp_process_saturate_color(result_color)
}

/// Literal port of `RSPProcessCS.hlsl:123-125`'s near-clip HACK:
/// `if (tfPos.w == 0.0f) { tfPos.w = 1e-6f; }`.
///
/// Exact float equality, so `-0.0` also fires and becomes a **positive**
/// `1e-6`; a NaN `w` does not fire. See module doc "Admitted domain".
pub fn rsp_process_near_clip_w(tf_pos_w: f32) -> f32 {
    if tf_pos_w == 0.0 {
        1e-6
    } else {
        tf_pos_w
    }
}

/// Literal port of `RSPProcessCS.hlsl:129`'s perspective divide:
/// `tfPos.xyz / float3(tfPos.w, -tfPos.w, tfPos.w)`.
///
/// The Y negation is applied to the **divisor**, matching the source's
/// written form rather than the algebraically-equivalent-for-finite-values
/// `-(y / w)`. `tf_pos_w` is expected to be the post-HACK value.
pub fn rsp_process_ndc(tf_pos: Vec4, tf_pos_w: f32) -> Vec3 {
    Vec3::new(
        tf_pos.x / tf_pos_w,
        tf_pos.y / -tf_pos_w,
        tf_pos.z / tf_pos_w,
    )
}

/// Literal port of `RSPProcessCS.hlsl:130`'s screen mapping:
/// `float4(ndcPos * rspViewport.scale + rspViewport.translate, tfPos.w)`.
///
/// Component-wise multiply-then-add, two separately-rounded operations (no
/// `mul_add`/FMA -- see module doc "Nonclaims"). The fourth component is
/// `tfPos.w`, not `1.0`.
pub fn rsp_process_screen_pos(ndc_pos: Vec3, viewport: RspViewport, tf_pos_w: f32) -> Vec4 {
    Vec4::new(
        ndc_pos.x * viewport.scale.x + viewport.translate.x,
        ndc_pos.y * viewport.scale.y + viewport.translate.y,
        ndc_pos.z * viewport.scale.z + viewport.translate.z,
        tf_pos_w,
    )
}

// ---------------------------------------------------------------------
// Composed per-vertex body
// ---------------------------------------------------------------------

/// The three per-vertex values `RSPProcessCS.hlsl:131-133` would store into
/// `dstPos`/`dstTc`/`dstCol`. Returned as a value; the stores themselves are
/// refused (see module doc "Nonclaims").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspProcessOutput {
    /// `dstPos[vertexOffsetIndex] = screenPos` (line 131).
    pub screen_pos: Vec4,
    /// `dstTc[vertexOffsetIndex] = tc` (line 132).
    pub tc: [f32; 2],
    /// `dstCol[vertexOffsetIndex] = vertexColor` (line 133).
    pub color: Vec4,
}

/// The already-loaded per-vertex inputs `CSMain` reads out of its bound
/// buffers. Every field stands in for a buffer read whose *indexing* is
/// refused (see module doc "Ported / refused boundary").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RspProcessVertex {
    /// `srcPos[posIndex + 0..3]` (line 64).
    pub pos: Vec3,
    /// `srcVel[posIndex + 0..3]` (line 65).
    pub vel: Vec3,
    /// `srcNorm[normColIndex + 0..3]`, signed (line 57-61).
    pub norm: [i32; 3],
    /// `srcCol[normColIndex + 0..4]`, unsigned (lines 77, 115-117).
    pub col: [u32; 4],
    /// `srcTc[tcIndex + 0..2]` (line 83).
    pub tc: [f32; 2],
    /// `srcTcVel[tcIndex + 0..2]` (line 91).
    pub tc_vel: [f32; 2],
    /// `srcFogIndices[vertexOffsetIndex]`, one-based with `0` = no fog
    /// (line 69).
    pub fog_index: u32,
    /// `srcLookAtIndices[vertexOffsetIndex]`, a packed bit field (line 82).
    pub look_at_index: u32,
}

/// Literal port of `RSPProcessCS.hlsl`'s per-vertex body (lines 57-130),
/// composed in source order.
///
/// The refused constructs are supplied as already-resolved parameters:
/// `view_proj_mat`/`world_mat` stand in for
/// `viewProjTransforms[viewProjIndex]`/`worldTransforms[transformIndex]`;
/// `fog_table` for `rspFogVector`; `look_at_table` for `rspLookAtVector`;
/// `lights` for the already-sliced `rspLightVector[lightIndex ..
/// lightIndex + lightCount]` range, with `None` standing for `lightCount ==
/// 0`; `viewport` for `rspViewportVector[viewProjIndex]`.
///
/// Evaluation order is preserved exactly, and the ordering that matters is
/// the fog block running **before** the near-clip HACK, so the fog divide
/// sees the un-patched `w`. See module doc "Admitted domain".
///
/// # Panics
///
/// Panics if `lights` is `Some` of an empty slice (see
/// [`rsp_process_lighting`]), or if `fog_table`/`look_at_table` is indexed
/// out of range by the vertex's `fog_index`/`look_at_index` -- both are
/// upstream buffer reads with no bound check in the source, surfaced here
/// rather than silently clamped.
pub fn rsp_process_vertex(
    vertex: RspProcessVertex,
    view_proj_mat: Mat4,
    world_mat: Mat4,
    fog_table: &[RspFog],
    look_at_table: &[RspLookAt],
    lights: Option<&[RspLight]>,
    viewport: RspViewport,
    cur_frame_weight: f32,
) -> RspProcessOutput {
    // Lines 57-61.
    let norm = rsp_process_norm(vertex.norm);

    // Line 66.
    let tf_pos = rsp_process_tf_pos(
        view_proj_mat,
        world_mat,
        vertex.pos,
        vertex.vel,
        cur_frame_weight,
    );

    // Lines 69-78. Note this consumes the UN-patched `tf_pos.w`.
    let byte_color = rsp_process_color_bytes(vertex.col);
    let alpha = match rsp_process_fog_index(vertex.fog_index) {
        Some(table_index) => {
            let fog = fog_table[table_index as usize];
            rsp_process_fog_alpha(tf_pos.z, tf_pos.w, fog)
        }
        None => byte_color.w,
    };

    // Lines 81-93.
    let look_at_decode = rsp_process_look_at_index(vertex.look_at_index);
    let tc = if look_at_decode.enabled {
        let look_at = look_at_table[look_at_decode.extracted_index as usize];
        compute_texture_gen(
            vertex.tc,
            [norm.x, norm.y, norm.z],
            TexGenLookAt {
                x: [look_at.x.x, look_at.x.y, look_at.x.z],
                y: [look_at.y.x, look_at.y.y, look_at.y.z],
            },
            look_at_decode.linear,
            world_matrix_from_mat4(world_mat),
        )
    } else {
        rsp_process_tc_velocity(vertex.tc, vertex.tc_vel, cur_frame_weight)
    };

    // Lines 96-119.
    let rgb = match lights {
        Some(slice) => rsp_process_lighting(slice, vertex.pos, norm, world_mat),
        None => Vec3::new(byte_color.x, byte_color.y, byte_color.z),
    };

    // Lines 123-125.
    let patched_w = rsp_process_near_clip_w(tf_pos.w);

    // Lines 128-130.
    let ndc_pos = rsp_process_ndc(tf_pos, patched_w);
    let screen_pos = rsp_process_screen_pos(ndc_pos, viewport, patched_w);

    RspProcessOutput {
        screen_pos,
        tc,
        color: Vec4::new(rgb.x, rgb.y, rgb.z, alpha),
    }
}

/// Repack a [`Mat4`] into [`crate::texture_gen`]'s row-major
/// [`WorldMatrix`]. Pure field movement, no arithmetic -- see module doc
/// "Reuse, not new type".
fn world_matrix_from_mat4(m: Mat4) -> WorldMatrix {
    WorldMatrix {
        rows: [
            [m.rows[0].x, m.rows[0].y, m.rows[0].z, m.rows[0].w],
            [m.rows[1].x, m.rows[1].y, m.rows[1].z, m.rows[1].w],
            [m.rows[2].x, m.rows[2].y, m.rows[2].z, m.rows[2].w],
            [m.rows[3].x, m.rows[3].y, m.rows[3].z, m.rows[3].w],
        ],
    }
}

#[cfg(test)]
mod tests;
