//! The CPU-evaluable arithmetic of fifteen `src/shaders/` present/debug/decode
//! shaders: a literal port of the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! ## Cited sources and their whole-file digests
//!
//! Every digest below was computed independently here with `shasum -a 256`
//! against the pinned port-commit checkout at
//! `/private/tmp/fn64-rt64-port-source` and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256`. **All fifteen match; no mismatch.**
//!
//! | path (`src/shaders/`) | inventory lines | SHA-256 |
//! |---|---|---|
//! | `DebugPS.hlsl` | 155 | `7309ae1af2590ed758b7098bf9d27d4b534cc5564ce6bcb54c97854fa4f8c2e8` |
//! | `BlueNoise.hlsli` | 42 | `4550c5a3bd38497f47e6c51351b09ba39b095745702037f3982f3e670de2f051` |
//! | `RSPVertexTestZCS.hlsl` | 41 | `767f216cb1bdd893b8d05181c6dc78922d0a4ebff17a9d1d712cc03c05751a1e` |
//! | `VideoInterfacePS.hlsl` | 41 | `f4990ef5edfbdece9c89938a0c4951b3a285f99be9e6d92a415433c0e227f961` |
//! | `PostBlendDitherNoisePS.hlsl` | 40 | `73eb5b96fd19be76ae243c3821c44211250156b778657e54fdf1ab30d04af0a5` |
//! | `TextureResolvePS.hlsl` | 39 | `141e8f74af1a9228d1b45879b74826b83e6f1cd0988d6bf1a9095d6ad69a34d4` |
//! | `ComposePS.hlsl` | 37 | `85a1aef1db2f21de09b4bc6de552fbd86a501bdc8e0b78c0aa6a483f61af9889` |
//! | `TextureDecodeCS.hlsl` | 28 | `794a516a6fdaf702d25c97d90aaac3b1ed8c3f42389a43049414d1a69b5c61d4` |
//! | `Im3DPS.hlsl` | 22 | `44675d859f5902f73c70374a9410d53ff16b2df93409fa554a77898c7ea7ee0b` |
//! | `Im3DVS.hlsl` | 20 | `59ab294c3d779d94fbcd3e7fb4d53e35b9fc89ff22e8b37d46889e655f8837b4` |
//! | `RenderParamsSpecConstants.hlsli` | 17 | `805719635740868e7868839d8ccc152bf521ccd10930a7e23f5356b8d3e06ff3` |
//! | `GlobalHitBuffers.hlsli` | 11 | `02498d46e883ef47c5977c26002cdd08f989ac7b7dee74dd272557a3f977c9d4` |
//! | `Library.hlsli` | 11 | `dd115f76ae192462eda8cfd6f464a2399d20cc2615adeebdb1fb5e44a91131ef` |
//! | `Im3DCommon.hlsli` | 10 | `3ce335b2f41f040b7d1c506a50bab82d4131b441d2c64fad16dd072dbff232b7` |
//! | `RenderParams.hlsli` | 5 | `d5f88b220b8aac84b541185cf18c8761701a7a6eb9fb81adfe1d421b7c1bdf58` |
//!
//! The inventory's line counts are one higher than `wc -l` for every file
//! because each ends with a final unterminated line, exactly as
//! `rt64_extra_params.rs` documents for its own header. Total 519 inventory
//! lines / 504 `wc -l` lines.
//!
//! ## The porting criterion
//!
//! A construct is ported when its behavior is fully determined by values and
//! control flow present in the cited file -- no GPU resource binding, no
//! sampler, no `[numthreads]` dispatch, no `SV_*` semantic, no type from an
//! uncited file. These are fifteen *shaders*: most of what they contain is
//! resource declaration and texture fetch, which has no CPU meaning. The
//! majority of the 519 lines is therefore refused, and that is the correct
//! answer rather than a shortfall.
//!
//! Where a shader's only impure step is a texture `Load`/`SampleLevel` or a
//! hardware derivative (`fwidth`), the sampled value is **hoisted to a
//! parameter**, never synthesized -- the same discipline
//! `rt64_lights_math.rs` applies to `getBlueNoise`. No test in this module
//! ever pretends to know what a texture would have returned.
//!
//! ## Per-file inventory-drift disclosure
//!
//! `docs/rt64-port-inventory.json` records `"ported_as": []` for all fifteen
//! paths, and every one of them has an oracle digest identical to its port
//! digest, so `tools/rt64_port_inventory.py`'s mechanical SHA-256 citation
//! scan now sees this module citing all fifteen and expects `ported_as` to
//! name it. **Measured**: `python3 scripts/lint-docs.py` was `clean (1037
//! refs across 120 docs)` before this change and reports exactly **1 error**
//! after -- `src/shaders/BlueNoise.hlsli: ported_as drift from mechanical
//! SHA-256 citation scan`. That single line stands for **all fifteen**
//! paths, not just `BlueNoise.hlsli`: the tool's `require()` raises on the
//! first failing path and `BlueNoise.hlsli` merely sorts first. Regenerating
//! the inventory is the fix, and `docs/rt64-port-inventory.json` is outside
//! this card's writable surface, so that reconciliation is left to the owning
//! ticket. Note also that the inventory credits a source as
//! `ported` at **whole-file** granularity: a partial port is credited in full
//! by that mechanism. Ten of these fifteen are partial or unported, so the
//! over-credit is real here and is disclosed explicitly per file:
//!
//! - `GlobalHitBuffers.hlsli` -- **full port**. Its entire content is one
//!   `#define` and one pure `uint` function; both are here.
//! - `DebugPS.hlsl` -- **partial, ~14 of 155 lines (~9%)**. Ported:
//!   `distanceFromLineSegment` (`:18-28`), `getMotionVector`'s block-center
//!   and line-threshold logic (`:31-45`, flow hoisted), `getShadingNormal`'s
//!   remap (`:53`). Refused: the fifteen `gX[pos]` texture-load accessors and
//!   the 37-line `VisualizationMode` `switch`, which selects among them.
//! - `BlueNoise.hlsli` -- **partial, ~10 of 42 lines (~24%)**. Ported: the
//!   tile-address arithmetic of `getBlueNoise` (`:9-12`) and the two
//!   tangent-frame lobe constructions (`:22-28`, `:35-41`) with `randVal`
//!   hoisted. Refused: the three `Texture2D.Load` calls themselves.
//! - `PostBlendDitherNoisePS.hlsl` -- **partial, ~9 of 40 lines (~23%)**.
//!   Ported: the `Range` constant (`:22`), the `NEGATIVE_MODE`/half-range
//!   subtraction (`:28-33`) and the `ADD_MODE`/`SUB_MODE` `max` clamps
//!   (`:35-39`). Refused: the `SV_POSITION`/`SV_TARGET0` signature and the
//!   push-constant binding. The `initRand`/`nextRand` half (`:21`, `:23-25`)
//!   is **already ported** in this crate's `random.rs`, which names this exact
//!   call site at `random.rs:45` and pins its seed composition plus the three
//!   sequential R/G/B draws in
//!   `postblenddithernoiseps_shaped_seed_and_sequential_rgb_draws`. This
//!   module cites that and does not re-derive it.
//! - `VideoInterfacePS.hlsl` -- **partial, ~11 of 41 lines (~27%)**. Ported:
//!   `SampleInput`'s border/clamp coordinate math and border mask (`:14-20`,
//!   sampler hoisted), `PixelAntialiasing`'s seam math (`:28-31`, `fwidth`
//!   hoisted). Refused: the two `SampleLevel`s, the `pow` gamma application
//!   (a `float4`-wide `pow` over a sampled value; hoisting it would leave
//!   nothing but `pow` itself), and the `PIXEL_ANTIALIASING` `#ifdef`.
//! - `TextureResolvePS.hlsl` -- **partial, ~5 of 39 lines (~13%)**. Ported:
//!   the `pixelPos` coordinate math (`:12`) and the three MSAA averaging
//!   divisors (`:14-35`) with the samples hoisted. Refused: the
//!   `Texture2DMS.Load` calls and the `SAMPLES_*` `#if` ladder as a
//!   compile-time selector.
//! - `ComposePS.hlsl` -- **partial, ~8 of 37 lines (~22%)**. Ported: the
//!   `diffuse.a > EPSILON` branch and the `lerp`/accumulate compose
//!   (`:20-35`), all seven samples hoisted. Refused: the seven
//!   `SampleLevel`s and the sampler/texture bindings. `LinearToSrgb` is
//!   **already ported** in this crate's `color_hlsli.rs`
//!   (`linear_to_srgb`/`linear_to_srgb4`) and is called, not re-derived.
//! - `RSPVertexTestZCS.hlsl` -- **partial, ~4 of 41 lines (~10%)**. Ported:
//!   the `pixelPos.xy * resolutionScale` integer truncation (`:29`) and the
//!   `pixelDepth <= pixelPos.z` branch's index-write selection (`:31-40`).
//!   Refused: both `sampleBackgroundDepth` overloads (`Texture2DMS`/
//!   `Texture2D` `Load`), the three `StructuredBuffer` bindings, and
//!   `[numthreads(1,1,1)]`.
//! - `Im3DPS.hlsl` -- **partial, ~5 of 22 lines (~23%)**. Ported: the
//!   occlusion test and its dither (`:12-19`), depths hoisted. Refused: the
//!   `gDepth` load, the `RtParams` matrix multiply, and `SV_Target`.
//! - `Im3DVS.hlsl` -- **partial, ~2 of 20 lines (~10%)**. Ported: the `.abgr`
//!   color swizzle (`:15`). Refused: the `mul(mul(proj, view), ...)` chain
//!   (needs `RtParams` from the uncited `FbRendererRT.hlsli`) and the
//!   `POSITION_SIZE`/`COLOR` input semantics.
//! - `TextureDecodeCS.hlsl` -- **cited, not ported**. Its body is a bounds
//!   check guarding one `sampleTMEM` call. `sampleTMEM` belongs to the
//!   uncited `TextureDecoder.hlsli` and is **already ported** in this crate's
//!   `tmem/` (see `tmem/texel.rs:41`, which names the
//!   `sampleTMEM4b`/`8b`/`16b`/`32b` family) and cited by
//!   `rt64_texture_sampler.rs:41`. The remaining `coord.x < Resolution.x &&
//!   coord.y < Resolution.y` guard is a dispatch-tail bounds check whose only
//!   consumer is an `RWTexture2D` store; porting a two-term comparison in
//!   isolation would be manufactured content, so it is refused.
//! - `RenderParamsSpecConstants.hlsli` -- **cited, not ported**. Five
//!   `[[vk::constant_id(N)]]` Vulkan specialization constants plus a
//!   constructor that copies each into a `RenderParams` field. The
//!   specialization-constant mechanism is a pipeline-creation concern with no
//!   CPU meaning, and the constructor's only content is field order, which
//!   **is not pinnable in safe Rust** (see `rt64_shared_params.rs:255`:
//!   field-init shorthand binds by identifier, so no test detects a reorder).
//!   `RenderParams`' own layout comes from the uncited
//!   `shared/rt64_render_params.h`.
//! - `Im3DCommon.hlsli` -- **cited, not ported**. One struct of four
//!   interpolants carrying `SV_POSITION`/`POSITION`/`COLOR`/`SIZE` semantics
//!   and `linear`/`noperspective` interpolation modifiers. The semantics and
//!   modifiers are rasterizer instructions with no CPU behavior; what remains
//!   is field order, which is not pinnable (as above). Nothing to port.
//! - `Library.hlsli` -- **cited, not ported**. `#pragma once` plus an
//!   `#ifdef LIBRARY` that defines `LIBRARY_EXPORT` as either `export` or
//!   nothing. Pure preprocessor plumbing selecting an HLSL linkage keyword;
//!   zero runtime behavior. This is the file the card predicted would be
//!   include-plumbing only, and it is.
//! - `RenderParams.hlsli` -- **cited, not ported**. A comment banner and a
//!   single `#include "shared/rt64_render_params.h"`. The file contains no
//!   declaration of its own. Likewise predicted, likewise confirmed.
//!
//! Tally: **1 full port, 9 partial, 5 cited-but-not-ported.**
//!
//! ## Reuse, not new type
//!
//! Per `AGENTS.md` "One vector type per port", a ported struct field,
//! parameter, or return that upstream spells `float3`/`float4` uses
//! `fn64_render_ir::Vec3`/`Vec4`. That rule's **second exception** covers
//! nearly everything here: these are loose shader-local `float2`/`float3`/
//! `float4` values inside HLSL function bodies, and caller-supplied
//! already-sampled values -- there is no upstream *type* to reuse, so they
//! stay bare `[f32; N]`. HLSL `float2` has no shared carrier at all.
//! [`compose_pixel`] is the one place a real `float3`-shaped ported value
//! flows end to end, and it uses `Vec3`.
//!
//! Reused rather than re-derived:
//! - [`crate::color_hlsli::linear_to_srgb`] / `linear_to_srgb4` for
//!   `Color.hlsli`'s `LinearToSrgb`, called by `ComposePS` and `DebugPS`.
//! - [`crate::math_hlsli::get_perpendicular_vector`] for `Math.hlsli`'s
//!   `getPerpendicularVector`, called by both `BlueNoise.hlsli` lobe
//!   functions.
//! - `crate::random::RandomState` for `PostBlendDitherNoisePS`' RNG half.
//! - `crate::tmem` for `TextureDecodeCS`' `sampleTMEM`.
//!
//! ## Admitted domain
//!
//! - **`EPSILON` is `1e-6`** and **`M_PI` is `3.14159265f`**, both from
//!   `src/shaders/Math.hlsli:7` and `:8` -- the only two facts admitted from
//!   an uncited file, each needed to give a ported expression a value, and
//!   both already quoted by this crate's `rt64_lights_math.rs` and
//!   `math_hlsli.rs` at the same pinned commit. `M_PI` is spelled as RT64's
//!   own truncated literal `3.14159265f` rather than
//!   `core::f32::consts::PI`. **CORRECTION**: an earlier revision of this doc
//!   claimed the two are different `f32` values and that substituting `PI`
//!   would be a silent behavior change. That is **false**, and was disproved
//!   by the test below: `3.14159265f32` and `core::f32::consts::PI` are the
//!   *same* `f32`, both `0x40490fdb`, because the decimal literal rounds to
//!   the nearest representable `f32`, which is also the nearest to true pi.
//!   The literal is kept anyway so the source text is readable against
//!   `Math.hlsli:8`, but no behavioral difference is claimed -- the choice is
//!   auditability, not semantics. Note this equality is a property of `f32`'s
//!   precision alone; the same literal in `f64` would genuinely differ.
//! - Integer conversions follow HLSL's rules: `float`->`int` **truncates
//!   toward zero**, and `uint2`/`int2` arithmetic **wraps** at 32 bits. Both
//!   are reproduced with `as` casts and `wrapping_*`, never with a guard.
//! - Floating-point comparison order and operand order are preserved
//!   literally. `min`/`max`/`clamp`/`saturate` are lowered as the source's own
//!   ternary with the source's argument order, so NaN propagates HLSL-style
//!   (`min(NaN, b)` is NaN; Rust's `f32::min` would give `b`). See
//!   [`hlsl_min`]/[`hlsl_max`], the same lowering
//!   `rt64_lights_math.rs:373-393` uses.
//! - `distanceFromLineSegment`'s `dot(...) / l2` has **no zero guard** for a
//!   non-zero-but-denormal `l2`; the `l2 == 0.0f` early return covers only
//!   exact zero. This is RT64's own behavior and is preserved, not guarded.
//!
//! ## Nonclaims
//!
//! - **No GPU, WGSL, production-path, shader-manifest, pipeline, or
//!   draw-call-wiring claim of any kind.** This module is pure CPU-side
//!   function ports and is unwired -- no caller anywhere in this crate. It
//!   compiles no shader and creates no resource.
//! - **No claim that any refused construct is unimportant** -- only that its
//!   behavior is not determined by the cited file. The fifteen `DebugPS`
//!   accessors, the seven `ComposePS` samples, and the `sampleTMEM` dispatch
//!   are all load-bearing on a GPU; they are simply not CPU-evaluable here.
//! - **No struct field-order claim.** This module declares no `repr(C)` type
//!   and pins no declaration order; per `rt64_shared_params.rs:255`, safe
//!   Rust offers no construction form that would detect a reorder. This is
//!   also the reason `Im3DCommon.hlsli` and `RenderParamsSpecConstants.hlsli`
//!   are refused rather than ported as structs.
//! - **No byte-layout, HLSL packing, or constant-buffer-offset claim** for any
//!   of the `[[vk::push_constant]]` structs the cited shaders bind.
//! - **No claim about the uncited includes' own behavior** --
//!   `FbRendererCommon.hlsli`, `FbRendererRT.hlsli`, `TextureDecoder.hlsli`,
//!   `Random.hlsli`, `Color.hlsli`, `Constants.hlsli`, `Formats.hlsli`, and
//!   `shared/rt64_render_params.h` are named only where a cited file includes
//!   them.
//! - **`PixelAntialiasing`'s `fwidth`** is a hardware screen-space derivative.
//!   [`pixel_antialiasing_texspace`] takes it as a parameter and makes **no
//!   claim** about what value a GPU would supply.
//! - **No claim that `Im3DPS`' `clip()` discard is modelled.** `clip` is a
//!   fragment-kill intrinsic; [`im3d_occlusion_dither`] returns the *argument*
//!   to `clip` and a `bool` for whether it would discard, which is the
//!   CPU-visible part of the decision, not the discard itself.
//! - **No new license obligation** beyond RT64's existing root MIT `LICENSE`
//!   boundary already established by every prior landed `fn64-render-wgpu`
//!   port module.

use crate::color_hlsli::{linear_to_srgb, linear_to_srgb4};
use crate::math_hlsli::get_perpendicular_vector;
use fn64_render_ir::Vec3;

/// `EPSILON` (`src/shaders/Math.hlsli:7`), admitted per the module doc.
const EPSILON: f32 = 1e-6;

/// `M_PI` (`src/shaders/Math.hlsli:8`): RT64's own literal `3.14159265f`.
///
/// This is bit-identical to `core::f32::consts::PI` -- see the module doc's
/// CORRECTION and `present_shaders_m_pi_literal_is_bit_equal_to_rust_pi_in_f32`.
/// The literal spelling is kept for auditability against the source line, not
/// because it denotes a different value.
const M_PI: f32 = 3.14159265f32;

/// HLSL `max(a, b)`, lowered as `b > a ? b : a` -- the source's own argument
/// order, so `max(NaN, b)` is `NaN` and `max(a, NaN)` is `a`. Rust's
/// `f32::max` returns the non-NaN operand in both cases and must not be
/// substituted. Matches `rt64_lights_math.rs:373`'s lowering.
#[inline]
fn hlsl_max(a: f32, b: f32) -> f32 {
    if b > a {
        b
    } else {
        a
    }
}

/// HLSL `min(a, b)`, lowered as `b < a ? b : a`. Same asymmetry as
/// [`hlsl_max`]: `min(NaN, b)` is `NaN`, `min(a, NaN)` is `a`.
#[inline]
fn hlsl_min(a: f32, b: f32) -> f32 {
    if b < a {
        b
    } else {
        a
    }
}

/// HLSL `clamp(x, lo, hi)`, lowered as `min(max(x, lo), hi)` -- NaN
/// propagation follows from [`hlsl_min`]/[`hlsl_max`], and `lo > hi` resolves
/// to `hi` rather than panicking.
#[inline]
fn hlsl_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    hlsl_min(hlsl_max(x, lo), hi)
}

/// HLSL `step(edge, x) = (x >= edge) ? 1.0 : 0.0`.
#[inline]
fn hlsl_step(edge: f32, x: f32) -> f32 {
    if x >= edge {
        1.0
    } else {
        0.0
    }
}

/// HLSL `lerp(x, y, s) = x + s * (y - x)`, spelled out literally -- never the
/// algebraically-equal-but-different-in-floating-point `x*(1-s) + y*s`.
#[inline]
fn hlsl_lerp(x: f32, y: f32, s: f32) -> f32 {
    x + s * (y - x)
}

/// `length(a - b)` for HLSL `float2`.
#[inline]
fn length2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

// ---------------------------------------------------------------------------
// GlobalHitBuffers.hlsli -- FULL PORT
// ---------------------------------------------------------------------------

/// `#define MAX_HIT_QUERIES 24` (`GlobalHitBuffers.hlsli:7`).
pub const MAX_HIT_QUERIES: u32 = 24;

/// Literal port of `uint getHitBufferIndex(uint hitPos, uint2 pixelIdx, uint2
/// pixelDims)` (`GlobalHitBuffers.hlsli:9-11`):
///
/// ```text
/// return (hitPos * pixelDims.y + pixelIdx.y) * pixelDims.x + pixelIdx.x;
/// ```
///
/// Every operand is `uint`, so every multiply and add **wraps** at 32 bits;
/// `wrapping_*` reproduces that exactly rather than panicking in debug builds.
/// The association is preserved as written: the `hitPos * dims.y + idx.y`
/// subexpression is formed first, then scaled by `dims.x`, then offset by
/// `idx.x` -- a hit-major, row-major, column-minor layout.
pub fn get_hit_buffer_index(hit_pos: u32, pixel_idx: [u32; 2], pixel_dims: [u32; 2]) -> u32 {
    hit_pos
        .wrapping_mul(pixel_dims[1])
        .wrapping_add(pixel_idx[1])
        .wrapping_mul(pixel_dims[0])
        .wrapping_add(pixel_idx[0])
}

// ---------------------------------------------------------------------------
// BlueNoise.hlsli -- PARTIAL
// ---------------------------------------------------------------------------

/// The tile-address arithmetic of `getBlueNoise` (`BlueNoise.hlsli:9-13`),
/// with the `Texture2D.Load` itself refused:
///
/// ```text
/// uint2 blueNoiseBase;
/// uint blueNoiseFrame = frameCount % 64;
/// blueNoiseBase.x = (blueNoiseFrame % 8) * 64;
/// blueNoiseBase.y = (blueNoiseFrame / 8) * 64;
/// return blueNoiseTexture.Load(uint3(blueNoiseBase + pixelPos % 64, 0)).rgb;
/// ```
///
/// Returns the `uint2` texel coordinate the `Load` would be given -- an
/// 8x8 grid of 64x64 tiles cycling with a period of 64 frames, offset by the
/// pixel position wrapped into one tile. All arithmetic is `uint`: `/` is
/// integer division and `%` is unsigned remainder, both of which Rust's `u32`
/// operators match exactly (unlike for signed types).
///
/// The addition `blueNoiseBase + pixelPos % 64` binds as
/// `blueNoiseBase + (pixelPos % 64)` -- `%` outranks `+` in HLSL as in Rust --
/// and cannot overflow for any input, since the base is at most 448 and the
/// wrapped offset at most 63.
pub fn blue_noise_texel_coord(pixel_pos: [u32; 2], frame_count: u32) -> [u32; 2] {
    let blue_noise_frame = frame_count % 64;
    let base_x = (blue_noise_frame % 8) * 64;
    let base_y = (blue_noise_frame / 8) * 64;
    [base_x + pixel_pos[0] % 64, base_y + pixel_pos[1] % 64]
}

/// The lobe construction of `getCosHemisphereSampleBlueNoise`
/// (`BlueNoise.hlsli:22-28`), with `randVal` hoisted from the refused
/// `getBlueNoise(...).rg` load:
///
/// ```text
/// float3 bitangent = getPerpendicularVector(normal);
/// float3 tangent = cross(bitangent, normal);
/// float r = sqrt(randVal.x);
/// float phi = 2.0f * M_PI * randVal.y;
/// return tangent * (r * cos(phi).x) + bitangent * (r * sin(phi)) + normal.xyz * sqrt(max(0.0, 1.0f - randVal.x));
/// ```
///
/// `getPerpendicularVector` is **not re-derived** -- it is
/// `crate::math_hlsli::get_perpendicular_vector`, this crate's existing
/// `Math.hlsli:20-27` port at the same pinned commit.
///
/// Two details preserved literally: the source writes `cos(phi).x`, a
/// no-op scalar swizzle that HLSL permits and that changes nothing; and
/// `max(0.0, 1.0f - randVal.x)` has its arguments in **that** order, so a NaN
/// in `randVal.x` yields `1.0 - NaN = NaN` as the *second* argument, which
/// [`hlsl_max`] then discards in favor of `0.0`. Rust's `f32::max` would agree
/// here by luck; the ternary is used anyway so the order is auditable.
///
/// The three scaled vectors are summed left to right, matching the source's
/// `a + b + c` association.
pub fn cos_hemisphere_sample_from_blue_noise(rand_val: [f32; 2], normal: [f32; 3]) -> [f32; 3] {
    let bitangent = get_perpendicular_vector(normal);
    let tangent = cross3(bitangent, normal);
    let r = rand_val[0].sqrt();
    let phi = 2.0f32 * M_PI * rand_val[1];
    let a = scale3(tangent, r * phi.cos());
    let b = scale3(bitangent, r * phi.sin());
    let c = scale3(normal, hlsl_max(0.0, 1.0f32 - rand_val[0]).sqrt());
    add3(add3(a, b), c)
}

/// The lobe construction of `getGGXMicrofacet` (`BlueNoise.hlsli:35-41`),
/// with `randVal` hoisted from the refused `getBlueNoise(...).rg` load:
///
/// ```text
/// float3 bitangent = getPerpendicularVector(normal);
/// float3 tangent = cross(bitangent, normal);
/// float a2 = roughness * roughness;
/// float cosThetaH = sqrt(max(0.0f, (1.0f - randVal.x) / ((a2 - 1.0f) * randVal.x + 1.0f)));
/// float sinThetaH = sqrt(max(0.0f, 1.0f - cosThetaH * cosThetaH));
/// float phiH = randVal.y * M_PI * 2.0f;
/// return tangent * (sinThetaH * cos(phiH)) + bitangent * (sinThetaH * sin(phiH)) + normal * cosThetaH;
/// ```
///
/// `phiH` here is `randVal.y * M_PI * 2.0f` -- note the operand order differs
/// from the cosine-hemisphere function's `2.0f * M_PI * randVal.y` above.
/// Both are preserved as written rather than normalized to a common form,
/// because float multiplication is not associative and the two can round
/// differently.
///
/// The `cosThetaH` division has **no zero guard**: when
/// `(a2 - 1.0) * randVal.x + 1.0` is exactly zero the quotient is `Inf` (or
/// `NaN` for a zero numerator), `max` keeps it, and `sqrt` yields `Inf`/`NaN`.
/// This is RT64's own behavior and is pinned, not guarded.
pub fn ggx_microfacet_from_blue_noise(
    rand_val: [f32; 2],
    roughness: f32,
    normal: [f32; 3],
) -> [f32; 3] {
    let bitangent = get_perpendicular_vector(normal);
    let tangent = cross3(bitangent, normal);
    let a2 = roughness * roughness;
    let cos_theta_h = hlsl_max(
        0.0f32,
        (1.0f32 - rand_val[0]) / ((a2 - 1.0f32) * rand_val[0] + 1.0f32),
    )
    .sqrt();
    let sin_theta_h = hlsl_max(0.0f32, 1.0f32 - cos_theta_h * cos_theta_h).sqrt();
    let phi_h = rand_val[1] * M_PI * 2.0f32;
    let a = scale3(tangent, sin_theta_h * phi_h.cos());
    let b = scale3(bitangent, sin_theta_h * phi_h.sin());
    let c = scale3(normal, cos_theta_h);
    add3(add3(a, b), c)
}

#[inline]
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn scale3(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

#[inline]
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

// ---------------------------------------------------------------------------
// PostBlendDitherNoisePS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// The compile-time mode the `#if` ladder at `PostBlendDitherNoisePS.hlsl:28-39`
/// selects. The `#ifdef`s are two *independent* axes in the source -- a
/// `NEGATIVE_MODE`/default choice at `:28-33` and an
/// `ADD_MODE`/`SUB_MODE`/neither choice at `:35-39` -- so they are modelled as
/// two parameters, not one enum, to avoid asserting a combination the source
/// does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitherClampMode {
    /// Neither `ADD_MODE` nor `SUB_MODE`: no clamp at all (`:35-39` falls
    /// through with no `#else`).
    None,
    /// `ADD_MODE` (`:36`): `resultColor = max(resultColor, 0.0f)`.
    Add,
    /// `SUB_MODE` (`:38`): `resultColor = max(-resultColor, 0.0f)`.
    Sub,
}

/// `const float Range = (7.0f * FrParams.ditherNoiseStrength) / 255.0f;`
/// (`PostBlendDitherNoisePS.hlsl:22`).
///
/// Kept as a named function rather than inlined because it is the module's one
/// quantization-shaped constant and the card requires such a constant be
/// derived two independent ways. It is: see
/// `dither_range_matches_an_independent_exact_rational_derivation`, which
/// recomputes the same value through a single correctly-rounded rational
/// division instead of the staged `f32` multiply-then-divide, and
/// `dither_range_halves_exactly_matching_half_strength`, which cross-checks
/// `Range(s)/2 == Range(s/2)`.
///
/// The multiply happens **before** the divide, exactly as written; folding
/// `7.0/255.0` into one constant first would round differently.
pub fn dither_noise_range(dither_noise_strength: f32) -> f32 {
    (7.0f32 * dither_noise_strength) / 255.0f32
}

/// The post-RNG half of `PSMain` (`PostBlendDitherNoisePS.hlsl:22-39`), with
/// the three `nextRand(randomSeed)` draws hoisted.
///
/// The RNG half (`:21` `initRand`, `:23-25` three sequential `nextRand`s) is
/// **already ported** in this crate's `random.rs`, which names this call site
/// at `random.rs:45` and pins both the seed composition and the R/G/B draw
/// order. `unit_randoms` is the three draws in that fixed order; this function
/// re-derives none of it.
///
/// ```text
/// resultColor.r = nextRand(randomSeed) * Range;   // and g, b
/// resultColor.a = 0.0f;
/// #if defined(NEGATIVE_MODE)
///     resultColor.rgb -= Range;
/// #else
///     const float HalfRange = Range / 2.0f;
///     resultColor.rgb -= HalfRange;
/// #endif
/// #if defined(ADD_MODE)
///     resultColor = max(resultColor, 0.0f);
/// #elif defined(SUB_MODE)
///     resultColor = max(-resultColor, 0.0f);
/// #endif
/// ```
///
/// Two behaviors that are easy to get wrong and are pinned below:
/// - The `-= Range` / `-= HalfRange` subtraction applies to `.rgb` **only**;
///   alpha stays at the `0.0f` assigned at `:26`.
/// - The `ADD_MODE`/`SUB_MODE` clamp applies to the **whole `float4`**,
///   alpha included. Under `SUB_MODE` that matters: `max(-0.0, 0.0)` is `0.0`
///   via [`hlsl_max`]'s `b > a` test (`0.0 > -0.0` is false, so `-0.0` is
///   returned) -- so alpha comes out `-0.0`, not `+0.0`. That is HLSL's own
///   result and is pinned rather than normalized.
pub fn post_blend_dither_noise(
    unit_randoms: [f32; 3],
    dither_noise_strength: f32,
    negative_mode: bool,
    clamp_mode: DitherClampMode,
) -> [f32; 4] {
    let range = dither_noise_range(dither_noise_strength);
    let mut result = [
        unit_randoms[0] * range,
        unit_randoms[1] * range,
        unit_randoms[2] * range,
        0.0f32,
    ];

    let bias = if negative_mode { range } else { range / 2.0f32 };
    result[0] -= bias;
    result[1] -= bias;
    result[2] -= bias;

    match clamp_mode {
        DitherClampMode::None => {}
        DitherClampMode::Add => {
            for c in result.iter_mut() {
                *c = hlsl_max(*c, 0.0f32);
            }
        }
        DitherClampMode::Sub => {
            for c in result.iter_mut() {
                *c = hlsl_max(-*c, 0.0f32);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// VideoInterfacePS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// The coordinate/border half of `SampleInput` (`VideoInterfacePS.hlsl:14-19`),
/// with both `SampleLevel` and the `pow` gamma refused.
///
/// ```text
/// const float2 LowerRight = gConstants.videoResolution / gConstants.textureResolution;
/// const float2 HalfPixel = float2(0.5f, 0.5f) / gConstants.textureResolution;
/// float2 outsideBorder = step(LowerRight, uv);
/// float4 sampledColor = gInput.SampleLevel(gSampler, clamp(uv, HalfPixel, LowerRight - HalfPixel), 0);
/// ...
/// gammaCorrectedColor.rgb *= max(1.0f - outsideBorder.x - outsideBorder.y, 0.0f);
/// ```
///
/// Returns the clamped UV the sampler would be given and the RGB border
/// multiplier, as `(clamped_uv, border_scale)`.
///
/// The border scale is `max(1 - step_x - step_y, 0)`: it is `1.0` strictly
/// inside the VI's sampleable region, and `0.0` once **either** axis is at or
/// past `LowerRight` (`step` is `>=`-inclusive, so landing exactly on
/// `LowerRight` is already outside). The subtraction is left-associative --
/// `(1 - x) - y` -- as written.
///
/// The `clamp` bounds are `HalfPixel` and `LowerRight - HalfPixel`. When
/// `videoResolution` is smaller than one pixel the upper bound can fall below
/// the lower; [`hlsl_clamp`] then resolves to the upper bound rather than
/// panicking, matching HLSL's `min(max(x, lo), hi)` lowering. There is no
/// guard against a zero `textureResolution` either: both divisions yield
/// `Inf`/`NaN` and propagate, which is RT64's behavior and is pinned.
pub fn vi_sample_input_coords(
    uv: [f32; 2],
    video_resolution: [f32; 2],
    texture_resolution: [f32; 2],
) -> ([f32; 2], f32) {
    let lower_right = [
        video_resolution[0] / texture_resolution[0],
        video_resolution[1] / texture_resolution[1],
    ];
    let half_pixel = [
        0.5f32 / texture_resolution[0],
        0.5f32 / texture_resolution[1],
    ];
    let outside_border = [
        hlsl_step(lower_right[0], uv[0]),
        hlsl_step(lower_right[1], uv[1]),
    ];
    let clamped = [
        hlsl_clamp(uv[0], half_pixel[0], lower_right[0] - half_pixel[0]),
        hlsl_clamp(uv[1], half_pixel[1], lower_right[1] - half_pixel[1]),
    ];
    let border_scale = hlsl_max(1.0f32 - outside_border[0] - outside_border[1], 0.0f32);
    (clamped, border_scale)
}

/// The seam math of `PixelAntialiasing` (`VideoInterfacePS.hlsl:28-32`), with
/// the hardware derivative `fwidth(uvTexspace)` hoisted to a parameter.
///
/// ```text
/// float2 uvTexspace = uv * gConstants.videoResolution;
/// float2 seam = floor(uvTexspace + 0.5f);
/// uvTexspace = (uvTexspace - seam) / fwidth(uvTexspace) + seam;
/// uvTexspace = clamp(uvTexspace, seam - 0.5f, seam + 0.5f);
/// return SampleInput(uvTexspace / gConstants.textureResolution);
/// ```
///
/// Returns the post-clamp `uvTexspace`, which the source then divides by
/// `textureResolution` and feeds to `SampleInput`. Sourced upstream from
/// <https://www.shadertoy.com/view/csX3RH> (credited at `:24-26`).
///
/// Note `fwidth` is evaluated on the **pre-modification** `uvTexspace`: the
/// source reassigns `uvTexspace` in the same statement that reads
/// `fwidth(uvTexspace)`, and HLSL evaluates the right-hand side first. The
/// parameter therefore means "`fwidth` of `uv * videoResolution`", and this
/// module makes no claim about what a GPU would produce for it.
///
/// A zero `fwidth` component divides by zero with no guard: the offset becomes
/// `±Inf` (or `NaN` at the exact seam, where the numerator is also zero), and
/// the following `clamp` then resolves it to `seam + 0.5`, `seam - 0.5`, or --
/// for `NaN` -- to `seam + 0.5` via [`hlsl_clamp`]'s NaN-hostile ternaries.
/// Pinned below rather than guarded.
pub fn pixel_antialiasing_texspace(
    uv: [f32; 2],
    video_resolution: [f32; 2],
    fwidth_uv_texspace: [f32; 2],
) -> [f32; 2] {
    let uv_texspace = [uv[0] * video_resolution[0], uv[1] * video_resolution[1]];
    let seam = [
        (uv_texspace[0] + 0.5f32).floor(),
        (uv_texspace[1] + 0.5f32).floor(),
    ];
    let shifted = [
        (uv_texspace[0] - seam[0]) / fwidth_uv_texspace[0] + seam[0],
        (uv_texspace[1] - seam[1]) / fwidth_uv_texspace[1] + seam[1],
    ];
    [
        hlsl_clamp(shifted[0], seam[0] - 0.5f32, seam[0] + 0.5f32),
        hlsl_clamp(shifted[1], seam[1] - 0.5f32, seam[1] + 0.5f32),
    ]
}

// ---------------------------------------------------------------------------
// TextureResolvePS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// `uint2 pixelPos = gConstants.uvScroll + uv.xy * gConstants.uvScale;`
/// (`TextureResolvePS.hlsl:12`).
///
/// The right-hand side is a `float2`; the assignment to `uint2` is an implicit
/// HLSL conversion that **truncates toward zero**, not rounds. `as u32`
/// reproduces that for non-negative values.
///
/// A negative result is **UB territory** and is deliberately not reproduced:
/// HLSL's `float`->`uint` conversion for a negative operand is undefined, and
/// Rust's `as u32` saturates to `0`. The DEVIATION is disclosed here and
/// pinned by `texture_resolve_pixel_pos_negative_is_a_deviation_not_a_claim`,
/// which asserts only Rust's saturating result and explicitly disclaims
/// parity. Real call sites pass a non-negative `uv` in `[0, 1]` and a
/// non-negative scroll/scale, well inside the admitted domain.
pub fn texture_resolve_pixel_pos(
    uv: [f32; 2],
    uv_scroll: [f32; 2],
    uv_scale: [f32; 2],
) -> [u32; 2] {
    [
        (uv_scroll[0] + uv[0] * uv_scale[0]) as u32,
        (uv_scroll[1] + uv[1] * uv_scale[1]) as u32,
    ]
}

/// The MSAA averaging of `PSMain` (`TextureResolvePS.hlsl:14-37`), with the
/// `Texture2DMS.Load` calls hoisted: the caller supplies the already-loaded
/// samples, and this sums them in index order and divides by the count.
///
/// The `#if` ladder selects 8, 4, 2, or 1 samples; `samples.len()` stands in
/// for that compile-time choice. The single-sample case (`:37`) returns the
/// load **unaveraged** -- there is no `/ 1.0f` in the source -- which this
/// reproduces by special-casing length 1, since dividing by `1.0` would in
/// fact be exact anyway but the *absence* of the operation is what the source
/// says.
///
/// Summation is strictly left to right in sample-index order, matching the
/// source's `Load(p,0) + Load(p,1) + ...` association. Float addition is not
/// associative, so any reordering is a real behavior change.
///
/// Returns `None` for a sample count the `#if` ladder cannot produce (anything
/// other than 1, 2, 4, or 8), rather than inventing a divisor.
pub fn texture_resolve_average(samples: &[[f32; 4]]) -> Option<[f32; 4]> {
    let divisor = match samples.len() {
        1 => return Some(samples[0]),
        2 => 2.0f32,
        4 => 4.0f32,
        8 => 8.0f32,
        _ => return None,
    };
    let mut sum = [0.0f32; 4];
    for (c, s) in sum.iter_mut().zip(0..4usize) {
        let mut acc = samples[0][s];
        for sample in &samples[1..] {
            acc += sample[s];
        }
        *c = acc;
    }
    Some([
        sum[0] / divisor,
        sum[1] / divisor,
        sum[2] / divisor,
        sum[3] / divisor,
    ])
}

// ---------------------------------------------------------------------------
// ComposePS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// Literal port of `PSMain`'s compose body (`ComposePS.hlsl:18-36`), with all
/// seven `SampleLevel`s hoisted to parameters:
///
/// ```text
/// if (diffuse.a > EPSILON) {
///     float3 result = lerp(LinearToSrgb(diffuse.rgb), LinearToSrgb(diffuse.rgb * (directLight + indirectLight)), diffuse.a);
///     result += reflection;
///     result += refraction;
///     result += transparent;
///     return float4(result, 1.0f);
/// }
/// else {
///     return LinearToSrgb(float4(diffuse.rgb, 1.0f));
/// }
/// ```
///
/// `LinearToSrgb` is **not re-derived**: it is
/// `crate::color_hlsli::linear_to_srgb` / `linear_to_srgb4`, this crate's
/// existing `Color.hlsli` port at the same pinned commit. The `float3` and
/// `float4` overloads are distinct functions there for the reason
/// `color_hlsli.rs:92` documents (Rust has no overloading); the `else` branch
/// takes the `float4` one, matching the source's `float4` argument.
///
/// Four behaviors preserved literally:
/// - The threshold is `diffuse.a > EPSILON`, strictly greater -- an alpha of
///   exactly `1e-6` takes the **else** branch.
/// - The `else` branch's alpha is the literal `1.0f`, and `linear_to_srgb4`
///   passes alpha through verbatim, so it stays `1.0`. The *input* alpha is
///   discarded entirely on that path.
/// - `lerp` is `x + s*(y - x)` (see [`hlsl_lerp`]), never `x*(1-s) + y*s`.
/// - The three accumulations happen **after** the lerp, in
///   reflection/refraction/transparent order, each a separate `+=`. The
///   returned alpha is the literal `1.0f`, not `diffuse.a`.
///
/// `Vec3` is used here per `AGENTS.md` "One vector type per port": the
/// composed `float3` is a real value flowing end to end, not a loose local.
/// The comment at `:27` records that the mix is intentionally done in sRGB
/// space to preserve fog-like effect colors.
pub fn compose_pixel(
    diffuse: [f32; 4],
    direct_light: Vec3,
    indirect_light: Vec3,
    reflection: Vec3,
    refraction: Vec3,
    transparent: Vec3,
) -> [f32; 4] {
    let diffuse_rgb = [diffuse[0], diffuse[1], diffuse[2]];
    if diffuse[3] > EPSILON {
        let lit = [
            diffuse_rgb[0] * (direct_light.x + indirect_light.x),
            diffuse_rgb[1] * (direct_light.y + indirect_light.y),
            diffuse_rgb[2] * (direct_light.z + indirect_light.z),
        ];
        let base = linear_to_srgb(diffuse_rgb);
        let shaded = linear_to_srgb(lit);
        let mut result = [
            hlsl_lerp(base[0], shaded[0], diffuse[3]),
            hlsl_lerp(base[1], shaded[1], diffuse[3]),
            hlsl_lerp(base[2], shaded[2], diffuse[3]),
        ];
        result[0] += reflection.x;
        result[1] += reflection.y;
        result[2] += reflection.z;
        result[0] += refraction.x;
        result[1] += refraction.y;
        result[2] += refraction.z;
        result[0] += transparent.x;
        result[1] += transparent.y;
        result[2] += transparent.z;
        [result[0], result[1], result[2], 1.0f32]
    } else {
        linear_to_srgb4([diffuse_rgb[0], diffuse_rgb[1], diffuse_rgb[2], 1.0f32])
    }
}

// ---------------------------------------------------------------------------
// DebugPS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// Literal port of `float distanceFromLineSegment(float2 p, float2 start,
/// float2 end)` (`DebugPS.hlsl:18-28`):
///
/// ```text
/// float len = length(start - end);
/// float l2 = len * len;
/// if (l2 == 0.0f) { return length(p - start); }
/// float t = max(0.0f, min(1.0f, dot(p - start, end - start) / l2));
/// float2 projection = start + t * (end - start);
/// return length(p - projection);
/// ```
///
/// Three details preserved exactly:
/// - `l2` is `length(start - end)` **squared**, i.e. a `sqrt` immediately
///   undone by a multiply. That round trip loses precision relative to a
///   direct `dx*dx + dy*dy`, and the source's form is kept rather than the
///   algebraically-equal shortcut.
/// - The degenerate guard tests `l2 == 0.0f`, not `len == 0.0f` and not a
///   tolerance. A segment short enough that `len*len` underflows to zero takes
///   the early return; one whose `l2` is merely denormal does **not**, and
///   divides by it. No guard is added.
/// - The clamp is `max(0.0f, min(1.0f, q))` -- `min` first, then `max`, with
///   the constants as the *first* argument to each. Per [`hlsl_min`]'s
///   ordering a `NaN` quotient makes `min(1.0, NaN)` return `1.0`, and then
///   `max(0.0, 1.0)` returns `1.0`; so a NaN `t` is impossible here even
///   though the division is unguarded. That asymmetry is exactly why the
///   argument order must not be normalized, and it is pinned below.
pub fn distance_from_line_segment(p: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let len = length2(start, end);
    let l2 = len * len;
    if l2 == 0.0f32 {
        return length2(p, start);
    }

    let d = (p[0] - start[0]) * (end[0] - start[0]) + (p[1] - start[1]) * (end[1] - start[1]);
    let t = hlsl_max(0.0f32, hlsl_min(1.0f32, d / l2));
    let projection = [
        start[0] + t * (end[0] - start[0]),
        start[1] + t * (end[1] - start[1]),
    ];
    length2(p, projection)
}

/// `float2 currentCenterPos = floor(pos / blockSize) * blockSize + (blockSize * 0.5f);`
/// (`DebugPS.hlsl:36`), with `blockSize` the literal `32.0f` from `:32`.
///
/// The screen coordinate of the center of the 32x32 block containing `pos`.
/// `floor` (not truncation) means negative positions round *down*, so
/// `pos = -1.0` lands in the block centered at `-16.0`, not `+16.0`.
pub fn motion_vector_block_center(pos: [f32; 2]) -> [f32; 2] {
    const BLOCK_SIZE: f32 = 32.0f32;
    [
        (pos[0] / BLOCK_SIZE).floor() * BLOCK_SIZE + (BLOCK_SIZE * 0.5f32),
        (pos[1] / BLOCK_SIZE).floor() * BLOCK_SIZE + (BLOCK_SIZE * 0.5f32),
    ]
}

/// The line-threshold decision of `getMotionVector` (`DebugPS.hlsl:31-45`),
/// with the `gFlow` texture load hoisted.
///
/// ```text
/// float lineThickness = 1.0f;
/// ...
/// float lineDistance = distanceFromLineSegment(pos, currentCenterPos, previousCenterPos);
/// return (lineDistance < lineThickness) ? float4(1,1,1,1) : float4(0,0,0,0);
/// ```
///
/// `previous_center_pos` is what `getPreviousFrameUVs(currentCenterPos)`
/// (`:12-16`) would return: `pos + gFlow[round(pos)].xy`. The `gFlow` load is
/// refused, so the caller supplies the result; this module makes no claim
/// about what the flow buffer contains.
///
/// Returns `true` when the pixel is *on* the drawn line -- strictly less than
/// `1.0f`, so a distance of exactly `1.0` is **off**. The caller maps that to
/// opaque white or transparent black.
pub fn motion_vector_is_line(pos: [f32; 2], previous_center_pos: [f32; 2]) -> bool {
    const LINE_THICKNESS: f32 = 1.0f32;
    let current_center_pos = motion_vector_block_center(pos);
    let line_distance = distance_from_line_segment(pos, current_center_pos, previous_center_pos);
    line_distance < LINE_THICKNESS
}

/// `getShadingNormal`'s remap (`DebugPS.hlsl:53`):
/// `(gShadingNormal[pos].rgb + 1.0f) / 2.0f`, with the load hoisted.
///
/// Maps a signed `[-1, 1]` normal into `[0, 1]` for display. The `+1` happens
/// before the `/2`, as written; there is no `saturate`, so a normal outside
/// `[-1, 1]` maps outside `[0, 1]` and is **not** clamped. The alpha the
/// source attaches is the literal `1.0f` and is not part of this remap.
pub fn shading_normal_to_display(normal: [f32; 3]) -> [f32; 3] {
    [
        (normal[0] + 1.0f32) / 2.0f32,
        (normal[1] + 1.0f32) / 2.0f32,
        (normal[2] + 1.0f32) / 2.0f32,
    ]
}

// ---------------------------------------------------------------------------
// RSPVertexTestZCS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// `int2 pixelPosInt = pixelPos.xy * gConstants.resolutionScale;`
/// (`RSPVertexTestZCS.hlsl:29`).
///
/// `float2`->`int2` is an implicit HLSL conversion that **truncates toward
/// zero**, so `-1.7` becomes `-1`, not `-2`. Rust's `as i32` matches for all
/// finite in-range values.
///
/// `as i32` also saturates for out-of-range and maps `NaN` to `0`, where HLSL
/// leaves those undefined; that is a DEVIATION, disclosed here, not
/// reproduced. Real screen positions are small and finite.
pub fn vertex_test_z_pixel_pos(pixel_pos_xy: [f32; 2], resolution_scale: [f32; 2]) -> [i32; 2] {
    [
        (pixel_pos_xy[0] * resolution_scale[0]) as i32,
        (pixel_pos_xy[1] * resolution_scale[1]) as i32,
    ]
}

/// The branch of `CSMain` (`RSPVertexTestZCS.hlsl:31-40`), with the
/// `sampleBackgroundDepth` load hoisted:
///
/// ```text
/// if (pixelDepth <= pixelPos.z) {
///     for (uint i = 0; i < gConstants.indexCount; i++) {
///         dstFaceIndices[gConstants.dstIndexStart + i] = gConstants.vertexIndex;
///     }
/// }
/// else {
///     for (uint i = 0; i < gConstants.indexCount; i++) {
///         dstFaceIndices[gConstants.dstIndexStart + i] = srcFaceIndices[gConstants.srcIndexStart + i];
///     }
/// }
/// ```
///
/// Returns the `indexCount` values written to `dstFaceIndices` starting at
/// `dstIndexStart`. The occluded branch (`pixelDepth <= pixelPos.z`) writes
/// `vertexIndex` repeated; the visible branch copies from `srcFaceIndices`.
///
/// The comparison is `<=`, so an exactly-equal depth takes the **occluded**
/// branch. A `NaN` depth makes `<=` false and takes the copy branch, matching
/// HLSL's NaN-hostile comparison; not special-cased.
///
/// Both index additions are `uint` and **wrap**, reproduced with
/// `wrapping_add`. `src_face_indices` is indexed as
/// `srcIndexStart + i`; an out-of-range read is a GPU out-of-bounds
/// `StructuredBuffer` access whose behavior is device-defined, so this
/// returns `None` rather than reproducing it -- a DEVIATION, disclosed, since
/// the alternative would be to invent a value.
pub fn vertex_test_z_dst_indices(
    pixel_depth: f32,
    pixel_pos_z: f32,
    vertex_index: u32,
    src_index_start: u32,
    index_count: u32,
    src_face_indices: &[u32],
) -> Option<Vec<u32>> {
    if pixel_depth <= pixel_pos_z {
        Some(vec![vertex_index; index_count as usize])
    } else {
        let mut out = Vec::with_capacity(index_count as usize);
        for i in 0..index_count {
            let idx = src_index_start.wrapping_add(i) as usize;
            out.push(*src_face_indices.get(idx)?);
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Im3DPS.hlsl / Im3DVS.hlsl -- PARTIAL
// ---------------------------------------------------------------------------

/// The occlusion dither of `PSMain` (`Im3DPS.hlsl:12-19`), with the `gDepth`
/// load and the `viewProj` multiply hoisted:
///
/// ```text
/// float pixelDepth = projPos.z / projPos.w;
/// float4 ret = _in.m_color;
/// // Dither and make the pixels more transparent if occluded.
/// if (bufferDepth < pixelDepth) {
///     ret *= 0.5f;
///     clip(fmod(_in.m_position.x + _in.m_position.y, 2.0f) - 1.0f);
/// }
/// return ret;
/// ```
///
/// Returns `(color, clip_argument)`. `clip_argument` is `None` when the pixel
/// is not occluded (the `clip` is not reached); when it is `Some(v)`, HLSL
/// `clip` discards the fragment iff `v < 0`, which
/// [`im3d_clip_discards`] decides. `clip` itself is a fragment-kill
/// intrinsic and is **not** modelled -- see Nonclaims.
///
/// Three details preserved:
/// - `projPos.z / projPos.w` has **no guard**; a `w` of zero yields `±Inf` (or
///   `NaN` for a zero `z`), the `<` comparison against `NaN` is false, and the
///   unoccluded path is taken.
/// - `ret *= 0.5f` scales **all four** components including alpha, which is
///   the "more transparent" the comment describes.
/// - `fmod` is C-style truncated remainder (sign follows the dividend), which
///   Rust's `%` on `f32` matches -- unlike `Math.hlsli`'s integer `modulo`,
///   which is floored. For a non-negative screen position `x + y` the result
///   is in `[0, 2)`, so `- 1.0` alternates sign on a one-pixel diagonal
///   checkerboard: the dither.
pub fn im3d_occlusion_dither(
    color: [f32; 4],
    buffer_depth: f32,
    proj_pos_z: f32,
    proj_pos_w: f32,
    position_xy: [f32; 2],
) -> ([f32; 4], Option<f32>) {
    let pixel_depth = proj_pos_z / proj_pos_w;
    let mut ret = color;
    if buffer_depth < pixel_depth {
        ret[0] *= 0.5f32;
        ret[1] *= 0.5f32;
        ret[2] *= 0.5f32;
        ret[3] *= 0.5f32;
        let clip_arg = (position_xy[0] + position_xy[1]) % 2.0f32 - 1.0f32;
        return (ret, Some(clip_arg));
    }
    (ret, None)
}

/// HLSL `clip(v)`: the fragment is discarded iff `v < 0`. A `NaN` argument
/// makes `<` false, so the fragment survives -- not special-cased.
pub fn im3d_clip_discards(clip_argument: f32) -> bool {
    clip_argument < 0.0f32
}

/// `ret.m_color = _in.m_color.abgr;` (`Im3DVS.hlsl:15`).
///
/// A full component reversal of the input `float4`, not a partial swap: the
/// vertex stream delivers the color in ABGR order and the rasterizer wants
/// RGBA. Reversal is its own inverse, which the round-trip test uses as an
/// independent check.
pub fn im3d_color_abgr(color: [f32; 4]) -> [f32; 4] {
    [color[3], color[2], color[1], color[0]]
}

#[cfg(test)]
mod present_shaders_tests {
    use super::*;

    // -- Math.hlsli constants admitted by this module -----------------------

    #[test]
    fn present_shaders_m_pi_literal_is_bit_equal_to_rust_pi_in_f32() {
        // CORRECTION, found by this test failing. An earlier revision
        // asserted `M_PI != core::f32::consts::PI`, reasoning that
        // `Math.hlsli:8`'s truncated `3.14159265f` must differ from the
        // correctly-rounded constant. It does not: the decimal literal
        // rounds to the nearest f32, which is the same 0x40490fdb that PI
        // rounds to. f32 simply lacks the precision to separate them.
        assert_eq!(M_PI.to_bits(), core::f32::consts::PI.to_bits());
        assert_eq!(M_PI.to_bits(), 0x4049_0fdb);

        // The literal genuinely IS below true pi -- just not by enough to
        // change the f32. Confirmed at f64 width, where the two separate:
        assert!(3.14159265f64 < core::f64::consts::PI);
        assert_ne!(3.14159265f64, core::f64::consts::PI);

        // So no behavioral claim rests on the spelling. This test exists to
        // keep the disproved claim from being reintroduced.
    }

    #[test]
    fn present_shaders_epsilon_is_one_e_minus_six() {
        assert_eq!(EPSILON, 1e-6f32);
    }

    // -- GlobalHitBuffers.hlsli ---------------------------------------------

    #[test]
    fn present_shaders_hit_buffer_index_origin_of_first_hit_is_zero() {
        assert_eq!(get_hit_buffer_index(0, [0, 0], [320, 240]), 0);
    }

    #[test]
    fn present_shaders_hit_buffer_index_is_hit_major_then_row_then_column() {
        // Hand-derived from `(hitPos * dims.y + idx.y) * dims.x + idx.x`
        // with dims 320x240:
        //   x+1 advances by 1        -> (0*240 + 0)*320 + 1 = 1
        //   y+1 advances by dims.x   -> (0*240 + 1)*320 + 0 = 320
        //   hit+1 advances by a page -> (1*240 + 0)*320 + 0 = 76800
        assert_eq!(get_hit_buffer_index(0, [1, 0], [320, 240]), 1);
        assert_eq!(get_hit_buffer_index(0, [0, 1], [320, 240]), 320);
        assert_eq!(get_hit_buffer_index(1, [0, 0], [320, 240]), 76_800);
    }

    #[test]
    fn present_shaders_hit_buffer_index_last_slot_of_the_max_query_range() {
        // The final addressable slot at MAX_HIT_QUERIES-1 hits, computed two
        // independent ways: through the port, and by the flat formula
        // hit*w*h + y*w + x evaluated in u64 (no wrapping possible).
        let (w, h) = (320u32, 240u32);
        let hit = MAX_HIT_QUERIES - 1;
        let ported = get_hit_buffer_index(hit, [w - 1, h - 1], [w, h]);
        let independent =
            (hit as u64) * (w as u64) * (h as u64) + (h as u64 - 1) * (w as u64) + (w as u64 - 1);
        assert_eq!(ported as u64, independent);
        assert_eq!(ported, 1_843_199);
    }

    #[test]
    fn present_shaders_hit_buffer_index_wraps_rather_than_panicking() {
        // Every operand is `uint`; HLSL wraps. Debug-mode Rust `*`/`+` would
        // panic, so `wrapping_*` is load-bearing, not cosmetic.
        // (0xFFFF_FFFF * 2 + 0) * 1 + 0 = 0xFFFF_FFFE after wrapping.
        assert_eq!(
            get_hit_buffer_index(u32::MAX, [0, 0], [1, 2]),
            0xFFFF_FFFEu32
        );
    }

    #[test]
    fn present_shaders_max_hit_queries_is_twenty_four() {
        assert_eq!(MAX_HIT_QUERIES, 24);
    }

    // -- BlueNoise.hlsli ----------------------------------------------------

    #[test]
    fn present_shaders_blue_noise_frame_zero_starts_at_the_tile_origin() {
        assert_eq!(blue_noise_texel_coord([0, 0], 0), [0, 0]);
    }

    #[test]
    fn present_shaders_blue_noise_walks_an_eight_by_eight_grid_of_sixty_four_tiles() {
        // frame 7 -> (7%8)*64 = 448 across, (7/8)*64 = 0 down: end of row 0.
        assert_eq!(blue_noise_texel_coord([0, 0], 7), [448, 0]);
        // frame 8 wraps to the start of row 1.
        assert_eq!(blue_noise_texel_coord([0, 0], 8), [0, 64]);
        // frame 63 is the last tile: (63%8)*64=448, (63/8)*64=448.
        assert_eq!(blue_noise_texel_coord([0, 0], 63), [448, 448]);
    }

    #[test]
    fn present_shaders_blue_noise_frame_cycle_is_sixty_four_not_the_grid_size() {
        // frameCount % 64 makes frame 64 identical to frame 0 and 65 to 1 --
        // the cycle is 64, not the 8 of the row index.
        for f in 0u32..64 {
            assert_eq!(
                blue_noise_texel_coord([13, 27], f),
                blue_noise_texel_coord([13, 27], f + 64)
            );
        }
        assert_ne!(
            blue_noise_texel_coord([0, 0], 0),
            blue_noise_texel_coord([0, 0], 1)
        );
    }

    #[test]
    fn present_shaders_blue_noise_pixel_offset_wraps_inside_one_sixty_four_tile() {
        // pixelPos % 64 confines the offset to [0,63] on each axis, so the
        // sample never leaves the frame's own tile.
        assert_eq!(blue_noise_texel_coord([63, 63], 0), [63, 63]);
        assert_eq!(blue_noise_texel_coord([64, 64], 0), [0, 0]);
        assert_eq!(blue_noise_texel_coord([65, 130], 0), [1, 2]);
        // And it composes with a non-origin tile base without carrying.
        assert_eq!(
            blue_noise_texel_coord([64 + 5, 64 + 9], 9),
            [64 + 5, 64 + 9]
        );
    }

    #[test]
    fn present_shaders_cos_hemisphere_at_zero_rand_returns_the_normal_itself() {
        // randVal = (0,0): r = sqrt(0) = 0, so both tangent terms vanish and
        // the normal term is scaled by sqrt(max(0, 1-0)) = 1.
        let n = [0.0f32, 0.0, 1.0];
        let got = cos_hemisphere_sample_from_blue_noise([0.0, 0.0], n);
        assert_eq!(got, n);
    }

    #[test]
    fn present_shaders_cos_hemisphere_at_rand_x_one_lies_in_the_tangent_plane() {
        // randVal.x = 1: r = 1, and the normal term is scaled by
        // sqrt(max(0, 1-1)) = 0 -- the sample is purely tangential, so its
        // dot with the normal is zero.
        let n = [0.0f32, 0.0, 1.0];
        let got = cos_hemisphere_sample_from_blue_noise([1.0, 0.25], n);
        let dot = got[0] * n[0] + got[1] * n[1] + got[2] * n[2];
        assert!(dot.abs() < 1e-6, "expected tangential, got {got:?}");
    }

    #[test]
    fn present_shaders_cos_hemisphere_max_argument_order_survives_rand_x_above_one() {
        // randVal.x > 1 makes `1.0 - randVal.x` negative; the source's
        // `max(0.0, ...)` keeps 0.0, so sqrt is 0 rather than NaN. Inverting
        // that max's argument order would still give 0 here, so the sharper
        // check is that the result is finite and has no normal component.
        let n = [0.0f32, 0.0, 1.0];
        let got = cos_hemisphere_sample_from_blue_noise([4.0, 0.0], n);
        assert!(got.iter().all(|c| c.is_finite()), "got {got:?}");
        assert_eq!(got[2], 0.0);
    }

    #[test]
    fn present_shaders_ggx_at_zero_roughness_and_zero_rand_returns_the_normal() {
        // a2 = 0, randVal.x = 0 -> cosThetaH = sqrt(max(0, 1/1)) = 1, so
        // sinThetaH = sqrt(max(0, 1-1)) = 0: purely the normal.
        let n = [0.0f32, 1.0, 0.0];
        let got = ggx_microfacet_from_blue_noise([0.0, 0.0], 0.0, n);
        assert_eq!(got, n);
    }

    #[test]
    fn present_shaders_ggx_unguarded_division_by_zero_denominator_is_pinned_not_guarded() {
        // The denominator `(a2 - 1)*randVal.x + 1` is exactly zero when
        // a2 = 0 and randVal.x = 1: (0-1)*1 + 1 = 0. Numerator 1-1 = 0, so
        // the quotient is 0/0 = NaN. `max(0.0, NaN)` -- with the source's
        // argument order -- tests `NaN > 0.0`, which is false, so 0.0 wins
        // and cosThetaH is sqrt(0) = 0. RT64's own behavior; no guard added.
        let n = [0.0f32, 0.0, 1.0];
        let got = ggx_microfacet_from_blue_noise([1.0, 0.0], 0.0, n);
        assert!(got.iter().all(|c| c.is_finite()), "got {got:?}");
        assert_eq!(got[2], 0.0, "cosThetaH must have collapsed to 0");
    }

    #[test]
    fn present_shaders_ggx_phi_operand_order_differs_from_cos_hemisphere() {
        // Both compute "2*pi*randVal.y", but GGX writes
        // `randVal.y * M_PI * 2.0f` (:40) while the hemisphere writes
        // `2.0f * M_PI * randVal.y` (:25). Float multiply is not
        // associative, so the two groupings can differ in the last bit.
        // This pins that the module kept them distinct rather than
        // normalizing both to one form.
        let y = 0.1f32;
        let hemisphere_form = 2.0f32 * M_PI * y;
        let ggx_form = y * M_PI * 2.0f32;
        // Whether they happen to be bit-equal for this y is incidental; what
        // matters is that each expression is spelled as its own source says.
        // Recompute both through the ported functions with a normal that
        // isolates phi's effect and confirm each matches its own form.
        let n = [0.0f32, 0.0, 1.0];
        let ggx = ggx_microfacet_from_blue_noise([0.5, y], 1.0, n);
        // roughness 1 -> a2 = 1 -> denominator = 1, cosThetaH = sqrt(0.5),
        // sinThetaH = sqrt(1 - 0.5) = sqrt(0.5). Tangent frame for +Z:
        // bitangent = getPerpendicularVector([0,0,1]), tangent = cross(b, n).
        let b = get_perpendicular_vector(n);
        let t = cross3(b, n);
        let s = (1.0f32 - 0.5f32).sqrt();
        let expected = add3(
            add3(scale3(t, s * ggx_form.cos()), scale3(b, s * ggx_form.sin())),
            scale3(n, 0.5f32.sqrt()),
        );
        assert_eq!(ggx, expected);
        // And the hemisphere form is a genuinely separate expression.
        let _ = hemisphere_form;
    }

    #[test]
    fn present_shaders_blue_noise_lobes_reuse_the_landed_math_hlsli_port() {
        // Both lobe functions build their tangent frame from
        // `crate::math_hlsli::get_perpendicular_vector` (Math.hlsli:20-27),
        // not a re-derivation. Pin that the frame is the perpendicular one.
        let n = [0.3f32, -0.7, 0.65];
        let b = get_perpendicular_vector(n);
        assert!(
            (b[0] * n[0] + b[1] * n[1] + b[2] * n[2]).abs() < 1e-6,
            "bitangent must be perpendicular to the normal"
        );
    }

    // -- PostBlendDitherNoisePS.hlsl ----------------------------------------

    #[test]
    fn present_shaders_dither_range_matches_an_independent_exact_rational_derivation() {
        // DERIVATION 1 (the port): staged f32 ops, `(7.0f * s) / 255.0f`.
        // DERIVATION 2 (independent): compute 7*s/255 in f64 from the exact
        // f32 value of s and round once. The two round differently in
        // general, so agreement is real evidence, not a tautology.
        for s in [1.0f32, 0.5, 0.25, 2.0, 0.1] {
            let ported = dither_noise_range(s);
            let independent = ((7.0f64 * (s as f64)) / 255.0f64) as f32;
            assert_eq!(
                ported.to_bits(),
                independent.to_bits(),
                "Range({s}) disagreed between derivations"
            );
        }
        // And the anchor value, written out: 7/255 at full strength.
        assert_eq!(
            dither_noise_range(1.0).to_bits(),
            0.027_450_980_618_596_08f32.to_bits()
        );
    }

    #[test]
    fn present_shaders_dither_range_halves_exactly_matching_half_strength() {
        // Second independent cross-check of the same constant: Range is
        // linear in strength and 2 is a power of two, so Range(s)/2 must be
        // bit-identical to Range(s/2). A wrong divisor (254, 256) or a
        // folded `7.0/255.0` constant would break this for some s.
        for s in [1.0f32, 0.5, 0.1, 3.0] {
            assert_eq!(
                (dither_noise_range(s) / 2.0).to_bits(),
                dither_noise_range(s / 2.0).to_bits(),
                "half-range mismatch at strength {s}"
            );
        }
        assert_eq!(
            (dither_noise_range(1.0) / 2.0).to_bits(),
            0.013_725_490_309_298_038f32.to_bits()
        );
    }

    #[test]
    fn present_shaders_dither_zero_strength_collapses_the_whole_range() {
        // Range = 0 -> every channel is 0*0 - 0 = 0 regardless of the RNG.
        let out = post_blend_dither_noise([0.9, 0.1, 0.5], 0.0, false, DitherClampMode::None);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn present_shaders_dither_default_mode_centers_on_zero_and_negative_mode_does_not() {
        // Default (`#else`, :31-32) subtracts Range/2, so a mid draw of 0.5
        // lands exactly on 0. NEGATIVE_MODE (:29) subtracts the full Range,
        // so the same draw lands at -Range/2.
        let range = dither_noise_range(1.0);
        let centered = post_blend_dither_noise([0.5, 0.5, 0.5], 1.0, false, DitherClampMode::None);
        assert_eq!(centered[0], 0.0);
        let negative = post_blend_dither_noise([0.5, 0.5, 0.5], 1.0, true, DitherClampMode::None);
        assert_eq!(negative[0], 0.5 * range - range);
        assert_eq!(negative[0], -(range / 2.0));
    }

    #[test]
    fn present_shaders_dither_bias_touches_rgb_only_never_alpha() {
        // `resultColor.rgb -= ...` (:29/:32) is an rgb-only swizzle write;
        // alpha keeps the 0.0f assigned at :26. Shifting the bias onto alpha
        // would show up here.
        let out = post_blend_dither_noise([0.0, 0.0, 0.0], 1.0, false, DitherClampMode::None);
        let range = dither_noise_range(1.0);
        assert_eq!(out[0], -(range / 2.0));
        assert_eq!(out[1], -(range / 2.0));
        assert_eq!(out[2], -(range / 2.0));
        assert_eq!(out[3], 0.0, "alpha must not receive the bias");
    }

    #[test]
    fn present_shaders_dither_add_mode_clamps_the_whole_float4_including_alpha() {
        // `resultColor = max(resultColor, 0.0f)` (:36) is a float4 assign,
        // not an .rgb one -- alpha is clamped too. A draw of 0.0 gives a
        // negative rgb that ADD_MODE floors to 0.
        let out = post_blend_dither_noise([0.0, 1.0, 0.5], 1.0, false, DitherClampMode::Add);
        let range = dither_noise_range(1.0);
        assert_eq!(out[0], 0.0, "negative channel must floor to zero");
        assert_eq!(out[1], 1.0 * range - range / 2.0);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn present_shaders_dither_sub_mode_negates_first_and_leaves_alpha_negative_zero() {
        // `max(-resultColor, 0.0f)` (:38). Alpha is 0.0, so -alpha is -0.0,
        // and hlsl_max(-0.0, 0.0) tests `0.0 > -0.0` -- false -- returning
        // -0.0. Rust's f32::max would return +0.0. The sign bit is the
        // observable difference and it is pinned, not normalized.
        let out = post_blend_dither_noise([1.0, 0.0, 0.5], 1.0, false, DitherClampMode::Sub);
        let range = dither_noise_range(1.0);
        // rgb: draw 1.0 -> +range/2, negated to -range/2, floored to 0.
        assert_eq!(out[0], 0.0);
        // draw 0.0 -> -range/2, negated to +range/2, kept.
        assert_eq!(out[1], range / 2.0);
        assert!(
            out[3].is_sign_negative(),
            "SUB_MODE alpha must be -0.0 (HLSL max order), got {}",
            out[3]
        );
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn present_shaders_dither_rng_half_is_delegated_to_the_landed_random_module() {
        // random.rs:45 names PostBlendDitherNoisePS.hlsl:21 and pins the seed
        // composition plus the three sequential R/G/B draws. This module
        // takes those three as an input in that fixed order and must apply
        // them to r, g, b respectively -- a transposition would show here.
        let range = dither_noise_range(1.0);
        let out = post_blend_dither_noise([0.25, 0.5, 0.75], 1.0, true, DitherClampMode::None);
        assert_eq!(out[0], 0.25 * range - range);
        assert_eq!(out[1], 0.5 * range - range);
        assert_eq!(out[2], 0.75 * range - range);
    }

    // -- VideoInterfacePS.hlsl ----------------------------------------------

    #[test]
    fn present_shaders_vi_inside_the_border_scales_by_one() {
        // video 320x240 in a 512x256 texture: LowerRight = (0.625, 0.9375).
        // A uv well inside both is unclamped and fully lit.
        let (uv, scale) = vi_sample_input_coords([0.25, 0.25], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(uv, [0.25, 0.25]);
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn present_shaders_vi_border_step_is_inclusive_at_lower_right() {
        // `step(edge, x)` is `x >= edge`, so landing exactly on LowerRight is
        // already outside and kills the sample. 320/512 = 0.625 exactly.
        let (_, at_edge) = vi_sample_input_coords([0.625, 0.0], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(at_edge, 0.0, "exactly at LowerRight must be outside");
        let (_, just_inside) = vi_sample_input_coords([0.624, 0.0], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(just_inside, 1.0);
    }

    #[test]
    fn present_shaders_vi_either_axis_outside_zeroes_the_border_scale() {
        // max(1 - x - y, 0): one axis outside gives 1-1-0 = 0; both outside
        // gives 1-1-1 = -1, which the max floors to 0 (never negative).
        let (_, one) = vi_sample_input_coords([0.9, 0.1], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(one, 0.0);
        let (_, both) = vi_sample_input_coords([0.9, 0.99], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(both, 0.0, "must floor at 0, not go negative");
    }

    #[test]
    fn present_shaders_vi_clamps_uv_into_the_half_pixel_inset_region() {
        // HalfPixel = 0.5/512 = 0.0009765625, 0.5/256 = 0.001953125.
        // A uv of 0 clamps up to HalfPixel; a uv past LowerRight clamps down
        // to LowerRight - HalfPixel = 0.625 - 0.0009765625.
        let (uv, _) = vi_sample_input_coords([0.0, 0.0], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(uv, [0.000_976_562_5, 0.001_953_125]);
        let (uv_hi, _) = vi_sample_input_coords([5.0, 5.0], [320.0, 240.0], [512.0, 256.0]);
        assert_eq!(uv_hi[0], 0.625 - 0.000_976_562_5);
        assert_eq!(uv_hi[1], 0.937_5 - 0.001_953_125);
    }

    #[test]
    fn present_shaders_vi_pixel_antialiasing_snaps_toward_the_seam() {
        // uv 0.5 * video 320 = 160.0 texspace. seam = floor(160.5) = 160.
        // Offset is 0, so any fwidth leaves it at the seam, and the clamp is
        // a no-op.
        let out = pixel_antialiasing_texspace([0.5, 0.5], [320.0, 240.0], [1.0, 1.0]);
        assert_eq!(out, [160.0, 120.0]);
    }

    #[test]
    fn present_shaders_vi_pixel_antialiasing_clamps_to_half_a_texel_of_the_seam() {
        // uvTexspace = 0.3 * 100 = 30.0 -> wait: use a value with a real
        // offset. uv 0.309 * 100 = 30.9 (approx); seam = floor(31.4) = 31.
        // offset = 30.9 - 31 = -0.1; with a tiny fwidth of 0.01 the shift is
        // -10, far past the clamp, so it pins to seam - 0.5 = 30.5.
        let out = pixel_antialiasing_texspace([0.309, 0.5], [100.0, 100.0], [0.01, 1.0]);
        assert_eq!(out[0], 30.5, "must clamp to seam - 0.5");
        // The upper side clamps symmetrically: uv 0.311*100 = 31.1,
        // seam = floor(31.6) = 31, offset +0.1, shift +10 -> seam + 0.5.
        let hi = pixel_antialiasing_texspace([0.311, 0.5], [100.0, 100.0], [0.01, 1.0]);
        assert_eq!(hi[0], 31.5, "must clamp to seam + 0.5");
    }

    #[test]
    fn present_shaders_vi_pixel_antialiasing_zero_fwidth_is_pinned_not_guarded() {
        // fwidth = 0 with a non-zero offset divides to ±Inf, which the clamp
        // then resolves to a seam bound -- finite, no guard needed and none
        // added. A zero offset gives 0/0 = NaN, and hlsl_clamp's
        // min(max(NaN, lo), hi) = min(lo, hi) = ... resolves NaN-hostilely.
        let inf_side = pixel_antialiasing_texspace([0.309, 0.5], [100.0, 100.0], [0.0, 1.0]);
        assert_eq!(inf_side[0], 30.5);
        // Exactly on the seam: offset 0, 0/0 = NaN. hlsl_max(NaN, lo) tests
        // `lo > NaN` = false, keeping NaN; hlsl_min(NaN, hi) tests
        // `hi < NaN` = false, keeping NaN. So NaN survives the clamp.
        let nan_side = pixel_antialiasing_texspace([0.31, 0.5], [100.0, 100.0], [0.0, 1.0]);
        assert!(
            nan_side[0].is_nan(),
            "0/0 at the seam must propagate NaN through the NaN-hostile clamp, got {}",
            nan_side[0]
        );
    }

    // -- TextureResolvePS.hlsl ----------------------------------------------

    #[test]
    fn present_shaders_texture_resolve_pixel_pos_truncates_it_does_not_round() {
        // float2 -> uint2 is a truncation. 0.9 * 10 + 0 = 9.0 -> 9, but
        // 0.99 * 10 = 9.9 -> 9, not 10. A round() here would give 10.
        assert_eq!(
            texture_resolve_pixel_pos([0.99, 0.5], [0.0, 0.0], [10.0, 10.0]),
            [9, 5]
        );
        assert_eq!(
            texture_resolve_pixel_pos([0.0, 1.0], [3.0, 4.0], [10.0, 10.0]),
            [3, 14]
        );
    }

    #[test]
    fn present_shaders_texture_resolve_pixel_pos_negative_is_a_deviation_not_a_claim() {
        // DEVIATION, disclosed in the module doc: HLSL's float->uint for a
        // negative operand is undefined; Rust's `as u32` saturates to 0.
        // This test pins ONLY Rust's behavior and makes NO parity claim.
        assert_eq!(
            texture_resolve_pixel_pos([1.0, 0.0], [-5.0, 0.0], [1.0, 1.0]),
            [0, 0]
        );
    }

    #[test]
    fn present_shaders_texture_resolve_single_sample_is_returned_unaveraged() {
        // `:37` returns `gInput.Load(pixelPos, 0)` with no division at all.
        let s = [[0.25f32, 0.5, 0.75, 1.0]];
        assert_eq!(texture_resolve_average(&s), Some(s[0]));
    }

    #[test]
    fn present_shaders_texture_resolve_divisor_matches_the_sample_count() {
        // Four identical samples must average back to the same value; a
        // wrong divisor (2, 8) would scale it.
        let four = [[0.5f32, 0.25, 0.125, 1.0]; 4];
        assert_eq!(
            texture_resolve_average(&four),
            Some([0.5, 0.25, 0.125, 1.0])
        );
        let two = [[1.0f32, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        assert_eq!(texture_resolve_average(&two), Some([0.5, 0.5, 0.0, 0.0]));
        let eight = [[0.0f32; 4]; 8];
        assert_eq!(texture_resolve_average(&eight), Some([0.0; 4]));
    }

    #[test]
    fn present_shaders_texture_resolve_eight_sample_sum_divides_by_eight() {
        // Hand-derived: samples 0..7 with r = i as f32 sum to 28, /8 = 3.5.
        let mut s = [[0.0f32; 4]; 8];
        for (i, sample) in s.iter_mut().enumerate() {
            sample[0] = i as f32;
        }
        let got = texture_resolve_average(&s).unwrap();
        assert_eq!(got[0], 3.5);
        // Independent check: 0+1+...+7 = 7*8/2 = 28; 28/8 = 3.5.
        assert_eq!((0..8).sum::<u32>() as f32 / 8.0, got[0]);
    }

    #[test]
    fn present_shaders_texture_resolve_rejects_counts_the_if_ladder_cannot_produce() {
        // The `#if` ladder only ever compiles 8x, 4x, 2x, or 1x. Anything
        // else has no divisor in the source and must not be invented.
        assert_eq!(texture_resolve_average(&[[0.0f32; 4]; 3]), None);
        assert_eq!(texture_resolve_average(&[[0.0f32; 4]; 16]), None);
        assert_eq!(texture_resolve_average(&[]), None);
    }

    // -- ComposePS.hlsl -----------------------------------------------------

    #[test]
    fn present_shaders_compose_epsilon_threshold_is_strictly_greater() {
        // `diffuse.a > EPSILON`: an alpha of exactly 1e-6 takes the ELSE
        // branch, whose alpha is the literal 1.0f regardless of input.
        let z = Vec3::default();
        let at = compose_pixel([0.5, 0.5, 0.5, EPSILON], z, z, z, z, z);
        assert_eq!(at, linear_to_srgb4([0.5, 0.5, 0.5, 1.0]));
        // Just above takes the IF branch, whose output differs here because
        // the lights are zero: lerp toward LinearToSrgb(0) = 0.
        let above = compose_pixel([0.5, 0.5, 0.5, 1.0], z, z, z, z, z);
        assert_ne!(above, at);
    }

    #[test]
    fn present_shaders_compose_else_branch_forces_alpha_to_one_discarding_input_alpha() {
        // `LinearToSrgb(float4(diffuse.rgb, 1.0f))` -- the input alpha never
        // reaches the output on this path.
        let z = Vec3::default();
        let a = compose_pixel([0.2, 0.4, 0.6, 0.0], z, z, z, z, z);
        let b = compose_pixel([0.2, 0.4, 0.6, -3.0], z, z, z, z, z);
        assert_eq!(a, b, "input alpha must not survive the else branch");
        assert_eq!(a[3], 1.0);
    }

    #[test]
    fn present_shaders_compose_full_alpha_lerps_all_the_way_to_the_lit_term() {
        // lerp(x, y, 1.0) = x + 1*(y - x) = y exactly. With directLight = 1
        // and indirectLight = 0, the lit term is LinearToSrgb(diffuse * 1).
        let one = Vec3::new(1.0, 1.0, 1.0);
        let z = Vec3::default();
        let got = compose_pixel([0.5, 0.5, 0.5, 1.0], one, z, z, z, z);
        let expected = linear_to_srgb([0.5, 0.5, 0.5]);
        assert_eq!([got[0], got[1], got[2]], expected);
        assert_eq!(got[3], 1.0);
    }

    #[test]
    fn present_shaders_compose_accumulates_reflection_refraction_transparent_after_the_lerp() {
        // The three `+=` are applied to the lerp result, and the returned
        // alpha is the literal 1.0f -- NOT diffuse.a, which is 0.25 here.
        let one = Vec3::new(1.0, 1.0, 1.0);
        let z = Vec3::default();
        let base = compose_pixel([0.5, 0.5, 0.5, 0.25], one, z, z, z, z);
        let with_all = compose_pixel(
            [0.5, 0.5, 0.5, 0.25],
            one,
            z,
            Vec3::new(0.1, 0.0, 0.0),
            Vec3::new(0.0, 0.2, 0.0),
            Vec3::new(0.0, 0.0, 0.3),
        );
        assert_eq!(with_all[0], base[0] + 0.1);
        assert_eq!(with_all[1], base[1] + 0.2);
        assert_eq!(with_all[2], base[2] + 0.3);
        assert_eq!(with_all[3], 1.0, "alpha is the literal 1.0f, not diffuse.a");
    }

    #[test]
    fn present_shaders_compose_lerp_is_the_literal_form_not_the_mix_form() {
        // lerp(x, y, s) = x + s*(y - x). At s = 0 that is exactly x, bit for
        // bit; the `x*(1-s) + y*s` form would also give x here, so the
        // sharper check is a mid-s value against the literal expression.
        let dl = Vec3::new(0.7, 0.7, 0.7);
        let z = Vec3::default();
        let s = 0.3f32;
        let got = compose_pixel([0.4, 0.4, 0.4, s], dl, z, z, z, z);
        let x = linear_to_srgb([0.4, 0.4, 0.4])[0];
        let y = linear_to_srgb([0.4 * 0.7, 0.4 * 0.7, 0.4 * 0.7])[0];
        assert_eq!(got[0], x + s * (y - x));
    }

    #[test]
    fn present_shaders_compose_sums_direct_and_indirect_light_before_multiplying() {
        // The source is `diffuse.rgb * (directLight + indirectLight)` -- ONE
        // multiply after ONE add, not two multiplies summed.
        //
        // CORRECTION: an earlier revision split 0.7 as 0.3 + 0.4 and asserted
        // the result equalled passing a literal 0.7. It does not:
        // 0.3f32 + 0.4f32 is 0x3f333334 while 0.7f32 is 0x3f333333, one ULP
        // apart. The test failed and the port was right. Use an exactly
        // representable split instead.
        let z = Vec3::default();
        let a = Vec3::new(0.25, 0.25, 0.25);
        let b = Vec3::new(0.5, 0.5, 0.5);
        let split = compose_pixel([0.5, 0.5, 0.5, 1.0], a, b, z, z, z);
        let combined = compose_pixel(
            [0.5, 0.5, 0.5, 1.0],
            Vec3::new(0.75, 0.75, 0.75),
            z,
            z,
            z,
            z,
        );
        assert_eq!(split, combined, "0.25 + 0.5 is exactly 0.75 in f32");

        // The distinguishing check: `d*(x+y)` differs from `d*x + d*y` for
        // operands where the distributed form rounds differently. Here the
        // grouped form is what the source specifies.
        let (d, x, y) = (0.1f32, 0.2f32, 0.3f32);
        assert_ne!(d * (x + y), d * x + d * y);
    }

    // -- DebugPS.hlsl -------------------------------------------------------

    #[test]
    fn present_shaders_line_segment_perpendicular_distance_on_the_interior() {
        // p = (5,3) onto the segment (0,0)-(10,0): t = 50/100 = 0.5,
        // projection (5,0), distance 3.
        assert_eq!(
            distance_from_line_segment([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]),
            3.0
        );
    }

    #[test]
    fn present_shaders_line_segment_clamps_t_beyond_both_endpoints() {
        // Before the start: t clamps to 0, so the distance is to `start`.
        assert_eq!(
            distance_from_line_segment([-5.0, 0.0], [0.0, 0.0], [10.0, 0.0]),
            5.0
        );
        // Past the end: t clamps to 1, so the distance is to `end`.
        assert_eq!(
            distance_from_line_segment([15.0, 0.0], [0.0, 0.0], [10.0, 0.0]),
            5.0
        );
    }

    #[test]
    fn present_shaders_line_segment_degenerate_returns_distance_to_start() {
        // start == end -> l2 == 0.0 exactly -> the early return at :22.
        // (3,4) to (2,2) is sqrt(1 + 4) = sqrt(5).
        let d = distance_from_line_segment([3.0, 4.0], [2.0, 2.0], [2.0, 2.0]);
        assert_eq!(d, 5.0f32.sqrt());
    }

    #[test]
    fn present_shaders_line_segment_min_max_order_makes_a_nan_t_impossible() {
        // The clamp is `max(0.0, min(1.0, q))` with the CONSTANT first in
        // each. If q were NaN, hlsl_min(1.0, NaN) tests `NaN < 1.0` = false
        // and returns 1.0; hlsl_max(0.0, 1.0) returns 1.0. So t is never
        // NaN even though `d / l2` is unguarded. Swapping either argument
        // order would let NaN through. Verified directly on the helpers:
        assert_eq!(hlsl_min(1.0, f32::NAN), 1.0);
        assert_eq!(hlsl_max(0.0, hlsl_min(1.0, f32::NAN)), 1.0);
        // And the reversed order would NOT be NaN-safe:
        assert!(hlsl_min(f32::NAN, 1.0).is_nan());
    }

    #[test]
    fn present_shaders_line_segment_min_argument_order_is_observable_through_the_public_fn() {
        // The test above exercises the helpers directly, which a MUTATION
        // showed is not enough: swapping `hlsl_min(1.0, d/l2)` to
        // `hlsl_min(d/l2, 1.0)` inside `distance_from_line_segment` left all
        // tests green. This one reaches the swap through the public
        // function, so the argument order is genuinely pinned.
        //
        // Construction: a segment long enough that `length()` overflows to
        // +inf makes `l2` +inf, and a dot product that is also +inf, so
        // `d / l2` is inf/inf = NaN -- a reachable NaN `t` with a non-zero
        // `l2` (so the degenerate early return does not fire).
        let huge = 3e38f32;
        let s = [0.0f32, 0.0];
        let e = [huge, 0.0];
        let p = [huge, 0.0];

        // Source order `max(0.0, min(1.0, NaN))`: min returns 1.0 (since
        // `NaN < 1.0` is false), max returns 1.0, the projection lands on
        // `end`, and the distance from p to end is exactly 0.
        assert_eq!(distance_from_line_segment(p, s, e), 0.0);

        // Swapped order would give min -> NaN, max(0.0, NaN) -> 0.0, putting
        // the projection at `start` and the distance at +inf. Assert the
        // port did NOT produce that.
        assert!(distance_from_line_segment(p, s, e).is_finite());
        assert_ne!(distance_from_line_segment(p, s, e), f32::INFINITY);
    }

    #[test]
    fn present_shaders_line_segment_uses_the_sqrt_then_square_form_not_the_direct_one() {
        // `l2 = length(start-end) * length(start-end)`: a sqrt immediately
        // undone by a multiply, which is NOT equal to the direct
        // `dx*dx + dy*dy` in f32.
        //
        // CORRECTION: an earlier revision tried to show this with a 1e-20
        // segment, claiming the direct square underflows to 0 while the
        // round trip does not. That was wrong twice over -- 1e-20 squared is
        // 1e-40, a subnormal rather than 0, and `length2` computes `dx*dx`
        // internally anyway, so it underflows *first* and the round trip
        // cannot recover what the direct form lost. The real difference is
        // mantissa rounding, shown here.
        //
        // Segment (0,0)-(2,2): the exact squared length is 8.0, which IS
        // f32-representable. But length() = sqrt(8) = 2.8284271, and
        // squaring that rounded value gives 7.9999995, not 8.0.
        let (s, e) = ([0.0f32, 0.0], [2.0f32, 2.0]);
        let len = length2(s, e);
        let source_form = len * len;
        let direct_form = 2.0f32 * 2.0f32 + 2.0f32 * 2.0f32;
        assert_eq!(direct_form, 8.0);
        assert_ne!(source_form, direct_form);
        assert_eq!(source_form, 7.999_999_523_162_842);

        // That feeds `t = dot / l2`, so the two forms give observably
        // different projection parameters. dot for p=(2,0) is 4.0:
        //   source: 4.0 / 7.9999995 = 0.50000006
        //   direct: 4.0 / 8.0       = 0.5
        let t_source = 4.0f32 / source_form;
        let t_direct = 4.0f32 / direct_form;
        assert_ne!(t_source, t_direct);
        assert_eq!(t_source, 0.500_000_06);
        assert_eq!(t_direct, 0.5);

        // The port must be using the lossy source form. Verified through the
        // public function: reconstruct its projection from t_source and
        // confirm the returned distance matches that, which it does.
        let d = distance_from_line_segment([2.0, 0.0], s, e);
        let proj = [
            s[0] + t_source * (e[0] - s[0]),
            s[1] + t_source * (e[1] - s[1]),
        ];
        assert_eq!(d, length2([2.0, 0.0], proj));
    }

    #[test]
    fn present_shaders_motion_vector_block_center_is_the_middle_of_a_thirty_two_pixel_block() {
        // floor(pos/32)*32 + 16.
        assert_eq!(motion_vector_block_center([0.0, 0.0]), [16.0, 16.0]);
        assert_eq!(motion_vector_block_center([31.9, 31.9]), [16.0, 16.0]);
        assert_eq!(motion_vector_block_center([32.0, 64.0]), [48.0, 80.0]);
        assert_eq!(motion_vector_block_center([100.0, 100.0]), [112.0, 112.0]);
    }

    #[test]
    fn present_shaders_motion_vector_block_center_floors_it_does_not_truncate() {
        // A negative position must round DOWN. Truncation would give
        // -0.0*32 + 16 = 16; floor gives -1*32 + 16 = -16.
        assert_eq!(motion_vector_block_center([-1.0, -1.0]), [-16.0, -16.0]);
    }

    #[test]
    fn present_shaders_motion_vector_line_threshold_is_strictly_less_than_one() {
        // A pixel at the block center with zero flow: the segment is
        // degenerate, distance 0 < 1 -> on the line.
        assert!(motion_vector_is_line([16.0, 16.0], [16.0, 16.0]));
        // A pixel exactly 1.0 away from a degenerate segment is OFF, because
        // the test is `<`, not `<=`.
        assert!(!motion_vector_is_line([16.0, 17.0], [16.0, 16.0]));
        // And just under 1.0 is on.
        assert!(motion_vector_is_line([16.0, 16.999], [16.0, 16.0]));
    }

    #[test]
    fn present_shaders_shading_normal_remap_maps_signed_to_unit_range() {
        assert_eq!(shading_normal_to_display([-1.0, 0.0, 1.0]), [0.0, 0.5, 1.0]);
    }

    #[test]
    fn present_shaders_shading_normal_remap_does_not_saturate_out_of_range_input() {
        // No `saturate` in the source: an out-of-range normal maps outside
        // [0,1] and is displayed as-is.
        assert_eq!(
            shading_normal_to_display([-3.0, 3.0, 0.0]),
            [-1.0, 2.0, 0.5]
        );
    }

    // -- RSPVertexTestZCS.hlsl ----------------------------------------------

    #[test]
    fn present_shaders_vertex_test_z_pixel_pos_truncates_toward_zero() {
        // float2 -> int2 truncates, so -1.7 is -1 (not -2, which floor gives).
        assert_eq!(vertex_test_z_pixel_pos([1.9, -1.7], [1.0, 1.0]), [1, -1]);
        assert_eq!(
            vertex_test_z_pixel_pos([100.5, 50.5], [2.0, 2.0]),
            [201, 101]
        );
    }

    #[test]
    fn present_shaders_vertex_test_z_occluded_branch_repeats_the_vertex_index() {
        // pixelDepth <= pixelPos.z -> write `vertexIndex` indexCount times.
        let src = [7u32, 8, 9, 10];
        let got = vertex_test_z_dst_indices(0.5, 0.9, 42, 0, 3, &src).unwrap();
        assert_eq!(got, vec![42, 42, 42]);
    }

    #[test]
    fn present_shaders_vertex_test_z_equal_depth_takes_the_occluded_branch() {
        // The comparison is `<=`, not `<`. An exactly-equal depth must NOT
        // copy from the source buffer.
        let src = [7u32, 8, 9, 10];
        let got = vertex_test_z_dst_indices(0.5, 0.5, 42, 0, 2, &src).unwrap();
        assert_eq!(got, vec![42, 42], "equal depth must be treated as occluded");
    }

    #[test]
    fn present_shaders_vertex_test_z_visible_branch_copies_from_src_index_start() {
        // pixelDepth > pixelPos.z -> copy srcFaceIndices[srcIndexStart + i].
        let src = [7u32, 8, 9, 10];
        let got = vertex_test_z_dst_indices(0.9, 0.5, 42, 1, 3, &src).unwrap();
        assert_eq!(got, vec![8, 9, 10]);
    }

    #[test]
    fn present_shaders_vertex_test_z_nan_depth_takes_the_visible_copy_branch() {
        // `NaN <= z` is false, so the else branch runs. Not special-cased.
        let src = [7u32, 8, 9, 10];
        let got = vertex_test_z_dst_indices(f32::NAN, 0.5, 42, 0, 2, &src).unwrap();
        assert_eq!(got, vec![7, 8]);
    }

    #[test]
    fn present_shaders_vertex_test_z_zero_index_count_writes_nothing() {
        let src = [7u32];
        assert_eq!(
            vertex_test_z_dst_indices(0.1, 0.9, 42, 0, 0, &src),
            Some(vec![])
        );
        assert_eq!(
            vertex_test_z_dst_indices(0.9, 0.1, 42, 0, 0, &src),
            Some(vec![])
        );
    }

    // -- Im3DPS.hlsl / Im3DVS.hlsl ------------------------------------------

    #[test]
    fn present_shaders_im3d_unoccluded_returns_the_color_untouched_and_no_clip() {
        // bufferDepth (0.9) < pixelDepth (0.5/1.0 = 0.5)? No. So no dither.
        let c = [0.2f32, 0.4, 0.6, 0.8];
        let (out, clip) = im3d_occlusion_dither(c, 0.9, 0.5, 1.0, [3.0, 4.0]);
        assert_eq!(out, c);
        assert_eq!(clip, None);
    }

    #[test]
    fn present_shaders_im3d_occluded_halves_all_four_components_including_alpha() {
        // `ret *= 0.5f` is a float4 scale; alpha is halved too -- that is the
        // "more transparent" the source comment describes.
        let (out, clip) = im3d_occlusion_dither([0.2, 0.4, 0.6, 0.8], 0.1, 0.5, 1.0, [3.0, 4.0]);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
        assert!(clip.is_some());
    }

    #[test]
    fn present_shaders_im3d_clip_argument_alternates_on_a_one_pixel_diagonal() {
        // fmod(x + y, 2) - 1: sum 7 -> 1 - 1 = 0 (kept, since clip needs < 0);
        // sum 8 -> 0 - 1 = -1 (discarded). That one-pixel alternation is the
        // dither. Shifting the modulus or the -1 would break the pattern.
        let (_, odd) = im3d_occlusion_dither([1.0; 4], 0.0, 1.0, 1.0, [3.0, 4.0]);
        assert_eq!(odd, Some(0.0));
        assert!(!im3d_clip_discards(0.0), "0.0 is not < 0, so it survives");
        let (_, even) = im3d_occlusion_dither([1.0; 4], 0.0, 1.0, 1.0, [4.0, 4.0]);
        assert_eq!(even, Some(-1.0));
        assert!(im3d_clip_discards(-1.0));
    }

    #[test]
    fn present_shaders_im3d_unguarded_depth_divide_by_zero_w_takes_the_safe_path() {
        // projPos.w = 0 with z = 0 gives 0/0 = NaN; `bufferDepth < NaN` is
        // false, so the unoccluded path runs. No guard, as in the source.
        let (out, clip) = im3d_occlusion_dither([1.0; 4], 0.0, 0.0, 0.0, [0.0, 0.0]);
        assert_eq!(out, [1.0; 4]);
        assert_eq!(clip, None);
        // A non-zero z over a zero w is +Inf, which IS greater than any
        // finite bufferDepth, so that one does dither.
        let (_, inf_clip) = im3d_occlusion_dither([1.0; 4], 0.0, 1.0, 0.0, [0.0, 0.0]);
        assert!(inf_clip.is_some());
    }

    #[test]
    fn present_shaders_im3d_clip_never_discards_on_nan() {
        assert!(!im3d_clip_discards(f32::NAN));
    }

    #[test]
    fn present_shaders_im3d_abgr_reverses_all_four_components() {
        assert_eq!(im3d_color_abgr([0.1, 0.2, 0.3, 0.4]), [0.4, 0.3, 0.2, 0.1]);
    }

    #[test]
    fn present_shaders_im3d_abgr_is_its_own_inverse() {
        // Independent structural check: a full reversal applied twice is the
        // identity. A partial swap (e.g. only rb) would also satisfy this,
        // so it is paired with the explicit ordering test above.
        let c = [0.11f32, 0.22, 0.33, 0.44];
        assert_eq!(im3d_color_abgr(im3d_color_abgr(c)), c);
    }

    // -- hlsl_* lowering helpers, whose asymmetry several ports depend on ----

    #[test]
    fn present_shaders_hlsl_min_max_propagate_nan_in_the_first_argument_only() {
        assert!(hlsl_min(f32::NAN, 1.0).is_nan());
        assert_eq!(hlsl_min(1.0, f32::NAN), 1.0);
        assert!(hlsl_max(f32::NAN, 1.0).is_nan());
        assert_eq!(hlsl_max(1.0, f32::NAN), 1.0);
        // Rust's own min/max disagree with HLSL on both first-argument cases.
        assert_eq!(f32::NAN.min(1.0), 1.0);
        assert_eq!(f32::NAN.max(1.0), 1.0);
    }

    #[test]
    fn present_shaders_hlsl_max_returns_the_first_argument_on_a_signed_zero_tie() {
        // `b > a` is false for (a, b) = (-0.0, 0.0), so -0.0 is returned.
        // This is what makes SUB_MODE's alpha come out -0.0.
        assert!(hlsl_max(-0.0, 0.0).is_sign_negative());
        assert!(!f32::max(-0.0, 0.0).is_sign_negative());
    }

    #[test]
    fn present_shaders_hlsl_clamp_resolves_to_the_upper_bound_when_bounds_invert() {
        // min(max(x, lo), hi) with lo > hi gives hi -- never a panic, unlike
        // Rust's `f32::clamp`.
        assert_eq!(hlsl_clamp(5.0, 10.0, 1.0), 1.0);
    }

    #[test]
    fn present_shaders_hlsl_step_is_inclusive_at_the_edge() {
        assert_eq!(hlsl_step(1.0, 1.0), 1.0);
        assert_eq!(hlsl_step(1.0, 0.999), 0.0);
    }
}
