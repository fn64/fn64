//! Literal port of RT64's `PostProcessPS.hlsl` tonemap arithmetic and
//! `Upscaler::getQualityAuto`'s resolution-ratio quality ladder -- permitted
//! MIT RT64 Rust-port sources pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! `src/shaders/PostProcessPS.hlsl` (SHA-256 of the whole file, 61 lines,
//! `a3560cb7336fcb8f4963359944818cc0fedd6540cae58802b94b45eb8d6f0848`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #include "Color.hlsli"
//! #include "Constants.hlsli"
//! #include "Math.hlsli"
//!
//! #include "shared/rt64_raytracing_params.h"
//!
//! #define ENABLE_EXPOSURE_ADJUSTMENT
//!
//! ConstantBuffer<RaytracingParams> RtParams : register(b0);
//! Texture2D<float4> gInput : register(t1);
//! Texture2D<float4> gFlow : register(t2);
//! Texture2D<float> gLumaAvg : register(t3);
//! SamplerState gSampler : register(s4);
//!
//! float4 ColorMotionBlurred(float2 uv) {
//!     if ((RtParams.motionBlurStrength > 0.0f) && (RtParams.motionBlurSamples > 0)) {
//!         float2 flow = gFlow.SampleLevel(gSampler, uv, 0).xy / RtParams.resolution.xy;
//!         float flowLength = length(flow);
//!         if (flowLength > EPSILON) {
//!             const float SampleStep = RtParams.motionBlurStrength / RtParams.motionBlurSamples;
//!             float3 sumColor = float3(0.0f, 0.0f, 0.0f);
//!             float sumWeight = 0.0f;
//!             float2 startUV = uv - (flow * RtParams.motionBlurStrength / 2.0f);
//!             for (uint s = 0; s < RtParams.motionBlurSamples; s++) {
//!                 float2 sampleUV = clamp(startUV + flow * s * SampleStep, float2(0.0f, 0.0f), float2(1.0f, 1.0f));
//!                 float sampleWeight = 1.0f;
//!                 float4 outputColor = gInput.SampleLevel(gSampler, sampleUV, 0);
//!                 sumColor += outputColor.rgb * sampleWeight;
//!                 sumWeight += sampleWeight;
//!             }
//!
//!             return float4(sumColor / sumWeight, 1.0f);
//!         }
//!     }
//!
//!     float4 outputColor = gInput.SampleLevel(gSampler, uv, 0);
//!     return float4(outputColor.rgb, 1.0f);
//! }
//!
//! float3 WhiteBlackPoint(float3 bl, float3 wp, float3 color) {
//!     return (color - bl) / (wp - bl);
//! }
//!
//! float4 PSMain(in float4 pos : SV_Position, in float2 uv : TEXCOORD0) : SV_TARGET {
//!     // For the reasons stated in ComposePS, the exposure adjustment is performed in sRGB space and to
//!     // make the histogram have a better distribution since it doesn't use a logarithmic scale.
//!     float4 color = max(ColorMotionBlurred(uv), 0.0f);
//!
//! #ifdef ENABLE_EXPOSURE_ADJUSTMENT
//!     float avgLuma = gLumaAvg[uint2(0, 0)];
//!     float exposure = RtParams.tonemapExposure / avgLuma;
//!     color.rgb *= exposure;
//!     color.rgb = WhiteBlackPoint(RtParams.tonemapBlack, RtParams.tonemapWhite, color.rgb);
//! #endif
//!
//!     return clamp(color, 0.0f, 1.0f);
//! }
//! ```
//!
//! `PostProcessPS.hlsl` lines 5-9 `#include` `Color.hlsli`, `Constants.hlsli`,
//! `Math.hlsli`, and `shared/rt64_raytracing_params.h`. Matching the
//! `rt64_fullscreen_vs.rs`/`rt64_resample.rs` precedent of citing but not
//! digesting unused headers: no symbol from `Color.hlsli` or
//! `Constants.hlsli` is referenced anywhere in this file. `Math.hlsli`
//! contributes exactly one symbol this port depends on --
//! `#define EPSILON 1e-6` (`Math.hlsli:7`, cited literally, not digested as a
//! whole file since only that one macro is admitted) -- used in
//! `ColorMotionBlurred`'s `flowLength > EPSILON` gate. `shared/
//! rt64_raytracing_params.h` supplies `RaytracingParams`' field *types* used
//! by this file (`float motionBlurStrength`, `uint motionBlurSamples`,
//! `float tonemapExposure`, `float tonemapWhite`, `float tonemapBlack`); the
//! struct itself, its constructor defaults, and every other field are
//! resource-binding scaffolding, out of this ticket's scope (see "Ported vs.
//! skipped" below), so that header is cited for the field-type facts it
//! establishes but not digested or reproduced as a struct.
//!
//! `src/render/rt64_upscaler.cpp` (SHA-256 of the whole file, 36 lines,
//! `476ef2b549db6e9ff5acafdb72950850cc0535992c4052e735f5022c59c6bcb5`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #include <cassert>
//!
//! #include "rt64_upscaler.h"
//!
//! // Upscaler
//!
//! RT64::Upscaler::QualityMode RT64::Upscaler::getQualityAuto(int displayWidth, int displayHeight) {
//!     assert(displayWidth > 0);
//!     assert(displayHeight > 0);
//!
//!     // Get the most appropriate quality level for the target resolution.
//!     const uint64_t PixelsDisplay = displayWidth * displayHeight;
//!     const uint64_t Pixels720p = 1280 * 720;
//!     const uint64_t Pixels1080p = 1920 * 1080;
//!     const uint64_t Pixels1440p = 2560 * 1440;
//!     const uint64_t Pixels4K = 3840 * 2160;
//!     if (PixelsDisplay <= Pixels720p) {
//!         return QualityMode::UltraQuality;
//!     }
//!     else if (PixelsDisplay <= Pixels1080p) {
//!         return QualityMode::Quality;
//!     }
//!     else if (PixelsDisplay <= Pixels1440p) {
//!         return QualityMode::Balanced;
//!     }
//!     else if (PixelsDisplay <= Pixels4K) {
//!         return QualityMode::Performance;
//!     }
//!     else {
//!         return QualityMode::UltraPerformance;
//!     }
//! }
//! ```
//!
//! `rt64_upscaler.cpp` line 7 `#include`s `rt64_upscaler.h` for the
//! `QualityMode` enum declaration (`UltraPerformance = 0, Performance,
//! Balanced, Quality, UltraQuality, Native, Auto, MAX`) and the
//! `getQualityAuto` static-method declaration this file defines; that header
//! is a pure-virtual interface plus a POD resource-pointer bag with no
//! computation of its own (ticket non-goal: `rt64_upscaler.h` is out of
//! scope for this card), so only the five enum *values* this port actually
//! returns are cited, not the whole header or its digest.
//!
//! **Reuse, not new type.** [`QualityMode`] is the one owned representation
//! of `RT64::Upscaler::QualityMode`'s five resolution-ladder variants (this
//! port only ever returns `UltraPerformance`/`Performance`/`Balanced`/
//! `Quality`/`UltraQuality` from [`get_quality_auto`] -- `Native`/`Auto`/`MAX`
//! are declared in the header for other call sites this ticket does not
//! touch, so this port's enum omits them rather than modeling unreachable
//! variants). [`white_black_point`] and [`tonemap_exposure`] are the two
//! owned tonemap-arithmetic functions; [`motion_blur_sample_offset`] is the
//! owned motion-blur offset-sequence function. No WGSL sibling is emitted --
//! see "Ported vs. skipped" for why.
//!
//! **Halton/texture-gen finding**: neither `PostProcessPS.hlsl` nor
//! `rt64_upscaler.cpp` references Halton jitter or texture-coordinate
//! generation in any form -- no symbol resembling `halton`, `Halton`,
//! `computeTextureGen`, or `TextureGen` appears in either pinned file (both
//! read in full above). The already-landed `halton_sequence`/`halton_jitter`
//! (`crate::rt64_common`) and `compute_texture_gen`
//! (`crate::texture_gen::compute_texture_gen`) are therefore inapplicable to
//! this ticket's two sources; this module neither reimplements nor
//! references them.
//!
//! ## Ported vs. skipped
//!
//! Ported (the tonemap arithmetic and the quality-ratio ladder):
//! - `PostProcessPS.hlsl:44-46` (`WhiteBlackPoint`): the full per-component
//!   `(color - bl) / (wp - bl)` remap, ported as [`white_black_point`].
//! - `PostProcessPS.hlsl:54-56` (the `#ifdef ENABLE_EXPOSURE_ADJUSTMENT`
//!   block's exposure computation and multiply): `exposure = tonemapExposure
//!   / avgLuma` then `color.rgb *= exposure`, ported as [`tonemap_exposure`]
//!   (the divide) and folded into [`post_process_tonemap`] (the multiply) --
//!   `avgLuma` is taken as a plain `f32` argument, per the ticket's finding,
//!   cleanly breaking the dependency on the 1x1 GPU luminance texture
//!   `gLumaAvg` this file reads from (`Texture2D<float> gLumaAvg :
//!   register(t3)`, `PostProcessPS.hlsl:16`) -- that texture read
//!   (`gLumaAvg[uint2(0,0)]`, line 54) is resource-bind scaffolding, not
//!   ported; only the value it *would* yield, as an opaque `f32`, is
//!   admitted.
//! - `PostProcessPS.hlsl:51` (`max(ColorMotionBlurred(uv), 0.0f)`) and
//!   `PostProcessPS.hlsl:60` (`clamp(color, 0.0f, 1.0f)`): both ported as
//!   part of [`post_process_tonemap`]'s overall arithmetic chain -- the
//!   floor-at-zero and the final unit-range clamp bracket the tonemap
//!   arithmetic in the source and are reproduced in that same position
//!   (before `ENABLE_EXPOSURE_ADJUSTMENT`'s block for the `max`, after it for
//!   the `clamp`), not moved or elided.
//! - `PostProcessPS.hlsl:27-34` (the motion-blur sample-offset sequence
//!   inside `ColorMotionBlurred`'s inner `if (flowLength > EPSILON)` branch):
//!   `SampleStep = motionBlurStrength / motionBlurSamples`, `startUV = uv -
//!   flow * motionBlurStrength / 2.0f`, and the per-sample `sampleUV =
//!   clamp(startUV + flow * s * SampleStep, (0,0), (1,1))` -- a pure function
//!   of `(uv, flow, strength, samples, s)` with no texture fetch -- ported as
//!   [`motion_blur_sample_offset`], per the ticket's finding that this
//!   sequence is portable while the texture fetches (`gFlow.SampleLevel`,
//!   `gInput.SampleLevel`) are not.
//! - `PostProcessPS.hlsl:20-23`'s two-part motion-blur gate
//!   (`motionBlurStrength > 0.0f && motionBlurSamples > 0`, then, inside
//!   that, `flowLength > EPSILON`) and `PostProcessPS.hlsl:28`'s `for (uint s
//!   = 0; s < motionBlurSamples; s++)` sample count: ported as
//!   [`motion_blur_is_active`] and [`motion_blur_sample_count`] respectively,
//!   as plain predicates/counts a caller can evaluate before invoking
//!   [`motion_blur_sample_offset`] per sample -- reproduced as documented
//!   upstream facts, per the ticket's finding, not invented behavior.
//! - `rt64_upscaler.cpp:11-35` (`Upscaler::getQualityAuto`) in full: the five
//!   `uint64_t` pixel-count thresholds and the five-way `<=`-chained ladder,
//!   ported as [`get_quality_auto`]. See "Admitted domain" below for the
//!   exact thresholds, comparison direction, and the `int*int`-then-widen
//!   evaluation order.
//!
//! Skipped (dispatch/pixel-shader scaffolding, resource binds, texture
//! fetches, and the ticket's explicit non-goals):
//! - `ConstantBuffer<RaytracingParams> RtParams : register(b0)`,
//!   `Texture2D<float4> gInput : register(t1)`, `Texture2D<float4> gFlow :
//!   register(t2)`, `Texture2D<float> gLumaAvg : register(t3)`,
//!   `SamplerState gSampler : register(s4)`: resource binds. No `register`,
//!   no `ConstantBuffer`, no `Texture2D`/`SamplerState` type appears in this
//!   module.
//! - `gFlow.SampleLevel`, `gInput.SampleLevel` (three call sites:
//!   `ColorMotionBlurred`'s per-sample fetch inside the loop, and its two
//!   non-blurred/pass-through fetches at lines 21 and 40), `gLumaAvg[uint2(0,
//!   0)]` (line 54): actual texture fetches. None is ported; `avgLuma` and
//!   each per-sample fetched color are admitted only as opaque caller-
//!   supplied `f32`/`[f32; 3]` values, matching this crate's established
//!   `tmem`/`combiner`/`rt64_resample` precedent of taking upstream texel
//!   values as typed inputs rather than re-deriving GPU texture-unit
//!   behavior.
//! - `ColorMotionBlurred`'s non-blurred fallthrough paths
//!   (`PostProcessPS.hlsl:40-41`, and the `motionBlurStrength <= 0.0f ||
//!   motionBlurSamples <= 0 || flowLength <= EPSILON` cases that reach them):
//!   these are a single unweighted texture fetch with no arithmetic beyond
//!   "return the sampled color with alpha forced to `1.0f`" -- there is
//!   nothing to port beyond the `1.0f`-alpha fact itself, which
//!   [`motion_blur_is_active`]'s doc records as an upstream fact per the
//!   ticket's finding, without a corresponding function (a caller who finds
//!   the gate false already has the fetched `outputColor.rgb` in hand and
//!   only needs to pair it with alpha `1.0`, which is not meaningfully
//!   "arithmetic" to port).
//! - `in float4 pos : SV_Position`, `in float2 uv : TEXCOORD0`, `: SV_TARGET`
//!   (`PSMain`'s signature) and `float4 ColorMotionBlurred(float2 uv)`'s own
//!   signature/dispatch role as a pixel-shader entry point: pixel-shader
//!   scaffolding, not arithmetic.
//! - `#define ENABLE_EXPOSURE_ADJUSTMENT` / `#ifdef` itself: a compile-time
//!   toggle. As pinned, it is unconditionally defined with no observed
//!   `#undef` (mirroring `color_hlsli.rs`'s `#ifdef STRONG_GREEN_LUMINANCE`
//!   precedent), so this port hard-codes only the `#ifdef`-active branch as
//!   literal, unconditional Rust -- no `#[cfg]`, no runtime flag, no dead
//!   alternate-branch code path for "exposure adjustment disabled".
//! - `rt64_upscaler.cpp:12-13`'s `assert(displayWidth > 0); assert(
//!   displayHeight > 0);`: a debug-only precondition check, not arithmetic;
//!   [`get_quality_auto`] takes plain `i32` and does not assert or panic on
//!   non-positive input (see "Admitted domain" for what it does instead).
//! - `rt64_upscaler.h` in its entirety (56 lines; the ticket's explicit
//!   non-goal): a pure-virtual interface (`set`, `getQualityInformation`,
//!   `getJitterPhaseCount`, `upscale`, `isInitialized`,
//!   `requiresNonShaderResourceInputs`) plus `UpscaleParameters`, a POD bag
//!   of nine `RenderTexture*` pointers, with zero computation of its own; the
//!   FSR/DLSS/XeSS backends it abstracts are absent from the pinned
//!   checkout. Only the `QualityMode` enum's five reachable values are cited
//!   above (not digested, not reproduced as a struct/class).
//!
//! ## Admitted domain
//!
//! **Tonemap curve, exact constants and evaluation order** (all in
//! [`post_process_tonemap`], which composes the pieces below in the source's
//! own order):
//! 1. `color = max(colorMotionBlurred, 0.0f)` -- a floor at zero, per-
//!    component (this port takes `colorMotionBlurred` as a caller-supplied
//!    `[f32; 3]`, the already-computed `ColorMotionBlurred(uv).rgb`, since
//!    that function's own body is a texture-fetch-driven box mean or a
//!    single fetch -- see [`motion_blur_sample_offset`] and "Ported vs.
//!    skipped" above -- not part of the *tonemap* arithmetic this ticket
//!    scopes).
//! 2. `exposure = tonemapExposure / avgLuma` (plain `f32` division, no
//!    guard -- see "Unguarded division" below).
//! 3. `color.rgb *= exposure` -- each of the three components independently
//!    multiplied by the same scalar `exposure`.
//! 4. `color.rgb = WhiteBlackPoint(tonemapBlack, tonemapWhite, color.rgb)`
//!    -- per-component `(color[i] - black[i]) / (white[i] - black[i])`, with
//!    the subtraction on both sides evaluated before the division (matching
//!    HLSL operator precedence and the literal parenthesization in
//!    `PostProcessPS.hlsl:45`).
//! 5. `return clamp(color, 0.0f, 1.0f)` -- component-wise clamp to `[0,1]`,
//!    applied to all four components including the alpha carried in from
//!    `ColorMotionBlurred` (this port's [`post_process_tonemap`] clamps only
//!    the three RGB components it owns; a caller composing the full `PSMain`
//!    return value is responsible for clamping alpha the same way, since
//!    alpha here is a texture-fetch-derived value outside this ticket's
//!    tonemap-arithmetic scope).
//!
//! Steps 2-4 are gated by `ENABLE_EXPOSURE_ADJUSTMENT`, which -- as pinned --
//! is unconditionally active (see "Ported vs. skipped"); [`post_process_tonemap`]
//! therefore always performs them, matching the pinned source's only reachable
//! behavior.
//!
//! **`saturate`/`clamp` semantics (this port's assumption).** HLSL's
//! `clamp(x, min, max)` is documented as `max(min_bound, min(x, max_bound))`
//! (the same formula this crate's `rt64_resample.rs` precedent cites for its
//! own integer `clamp`). For the two literal-bound clamps in this file
//! (`PostProcessPS.hlsl:29`'s `clamp(..., (0,0), (1,1))` and line 60's
//! `clamp(color, 0.0f, 1.0f)`), this port follows this crate's *established*
//! convention from `color_hlsli.rs`/`rt64_luminance_histogram.rs` instead:
//! Rust's `x.clamp(0.0, 1.0)` with literal bounds (never computed, so
//! `f32::clamp`'s `min > max`/NaN-bound panic conditions can never trigger).
//! This is a documented, explicit choice, not a proven fact: `f32::clamp`
//! propagates a NaN `x` unchanged (both bound comparisons are IEEE-754-
//! unordered-false), whereas the `max(min_bound, min(x, max_bound))`
//! decomposition would instead *suppress* a NaN `x` to one of the literal
//! bounds (HLSL's `min`/`max` intrinsics are IEEE `fmin`/`fmax`-style,
//! NaN-non-propagating when the other operand is a number -- this crate's
//! own `rt64_luminance_histogram.rs` precedent documents that same `max`
//! behavior for `HDRToHistogramBin`'s `max(pixelCount - countForThisBin,
//! 1.0)`). These two decompositions of "the same" `clamp` genuinely disagree
//! on NaN input, and this port cannot resolve that disagreement without an
//! actual compiled-HLSL oracle (compiler/hardware `clamp` intrinsic lowering
//! is not fully specified for NaN by the HLSL reference). This port makes
//! the same literal choice `color_hlsli.rs`/`rt64_luminance_histogram.rs`
//! already made -- `f32::clamp`, NaN-propagating -- for consistency with
//! that established precedent, and states it here as an open assumption
//! rather than a verified fact. [`post_process_tonemap`]'s and
//! [`motion_blur_sample_offset`]'s doc comments and this module's
//! characterization tests both exercise and assert this exact choice.
//!
//! **`max(x, 0.0f)` (`PostProcessPS.hlsl:51`)**: unlike the two-literal-bound
//! `clamp` above, this is HLSL's plain two-argument `max`, ported via Rust's
//! `f32::max` -- IEEE `fmax`-style, NaN-suppressing when the other operand is
//! a number (this crate's `rt64_luminance_histogram.rs` precedent documents
//! the identical choice for its own `max(..., 1.0)` floor). A NaN
//! `colorMotionBlurred` component therefore becomes `0.0` after this step,
//! not NaN -- the opposite propagation behavior from the `clamp` calls above,
//! because `f32::max`/`f32::min` (not `f32::clamp`) are used here, matching
//! the source's own choice of a different intrinsic (`max`, not `clamp`) at
//! this call site.
//!
//! **`length()` (`PostProcessPS.hlsl:22`, inside `ColorMotionBlurred`, not
//! itself part of the ported tonemap/motion-blur-offset arithmetic)**: not
//! ported. [`motion_blur_is_active`] takes the caller-computed `flow_length`
//! (`length(flow)`) as a plain `f32` argument rather than re-deriving it from
//! a `flow: [f32;2]`, since `flow` itself comes from `gFlow.SampleLevel`, an
//! unported texture fetch.
//!
//! **Unguarded division.** Every division in this module --
//! [`white_black_point`]'s `(color-bl)/(wp-bl)`, [`tonemap_exposure`]'s
//! `tonemapExposure/avgLuma`, [`motion_blur_sample_offset`]'s
//! `motionBlurStrength/motionBlurSamples` -- is a plain, unguarded `f32`
//! division, exactly as HLSL writes it with no zero-check. This port adds no
//! guard: a zero or NaN denominator produces IEEE-754 `inf`/`-inf`/`NaN`
//! exactly as the hardware division HLSL compiles to would, and the
//! characterization tests below assert those non-finite outcomes directly
//! rather than treating them as errors.
//!
//! **`pow`/`exp2`/`rcp`**: none of `pow`, `exp2`, or `rcp` appears anywhere
//! in either pinned source file (both read in full above); this port neither
//! introduces nor needs to reason about their HLSL/WGSL/Rust semantic
//! differences.
//!
//! **`getQualityAuto`'s exact thresholds and comparison direction.** All
//! five comparisons are `<=` (inclusive upper bound: the threshold pixel
//! count itself belongs to the *lower*/higher-quality tier, not the next
//! one), evaluated as a strict top-to-bottom else-if chain (first match
//! wins, matching [`get_quality_auto`]'s `if`/`else if` chain below):
//!
//! | condition (`pixels_display <= N`) | N (pixel count) | tier |
//! |---|---|---|
//! | `<= 1280*720`   | `921_600`   | [`QualityMode::UltraQuality`] |
//! | `<= 1920*1080`  | `2_073_600` | [`QualityMode::Quality`] |
//! | `<= 2560*1440`  | `3_686_400` | [`QualityMode::Balanced`] |
//! | `<= 3840*2160`  | `8_294_400` | [`QualityMode::Performance`] |
//! | (else, i.e. `> 8_294_400`) | -- | [`QualityMode::UltraPerformance`] |
//!
//! So e.g. exactly `921_600` pixels (1280x720, or any other `w*h` product
//! equal to it) is `UltraQuality`, and `921_601` is `Quality` -- the boundary
//! is on the `<=`/`>` side, not `<`/`>=`. Verified independently by a
//! from-scratch Python integer simulation (no shared code with
//! [`get_quality_auto`]) at every threshold and at one pixel above/below
//! each; see this module's `tests` submodule.
//!
//! **`int * int` then widen to `uint64_t` (not widen-then-multiply).**
//! `rt64_upscaler.cpp:16`: `const uint64_t PixelsDisplay = displayWidth *
//! displayHeight;` multiplies the two `int displayWidth, displayHeight`
//! parameters as plain 32-bit signed `int` arithmetic first, and only the
//! *result* is implicitly converted to `uint64_t` by the `const uint64_t`
//! initialization -- there is no earlier widening of either operand. This
//! port's [`get_quality_auto`] reproduces that exact order: it multiplies
//! `display_width: i32` and `display_height: i32` as `i32` (via
//! `i64::from(display_width) * i64::from(display_height)`, using `i64` --
//! wider than the source's `i32`, deliberately, to sidestep Rust's overflow
//! panic in debug builds / silent 32-bit wraparound in release builds, since
//! plain `i32 * i32` for large but plausible display dimensions, e.g. an
//! 8K-plus multi-monitor width, would overflow 32-bit signed range exactly
//! where C++'s own `int * int` would already be signed-overflow UB --
//! rather than the C++ literal `int32 * int32`-with-wraparound this port
//! cannot faithfully reproduce without also reproducing C++ signed-overflow
//! UB) then widens to match the threshold constants' `u64` width; the
//! *widen-after-multiply* ordering itself (not the wraparound behavior on
//! overflow) is the fact this port preserves. `rt64_upscaler.cpp:12-13`'s
//! `assert(displayWidth > 0); assert(displayHeight > 0);` are debug-only
//! preconditions, not ported (see "Ported vs. skipped") -- [`get_quality_auto`]
//! accepts non-positive `i32` inputs without panicking; a non-positive
//! product simply participates in the same `<=` ladder as any other `i64`
//! value (see this module's tests for a `(0, 0)` and negative-dimension
//! case, both landing in [`QualityMode::UltraQuality`] since their product is
//! `<= 921_600`).
//!
//! ## Nonclaims
//!
//! This module performs no GPU execution: no shader is compiled, validated,
//! or dispatched here, and -- unlike `rt64_fullscreen_vs.rs`/
//! `rt64_resample.rs` -- no WGSL sibling is emitted at all, because nothing
//! in this ticket's admitted scope (a pure per-component tonemap remap, a
//! pure motion-blur offset sequence, and a pure CPU resolution-ratio ladder)
//! is itself a shader entry point or compute kernel; `PSMain`'s own
//! signature/dispatch role and every texture fetch it depends on are
//! explicitly skipped (see "Ported vs. skipped"), leaving no GPU-shaped
//! surface here to re-express in WGSL. It makes no production-wiring claim:
//! no pipeline, bind group, `wgpu::ShaderModule`, render target, or
//! `targets/`/draw-path integration is created here, and this module is not
//! referenced (`mod` only, no `pub use`) from any other module in this
//! crate. It makes no parity or performance claim against RT64's own
//! renderer, and no claim about `rt64_upscaler.h`'s own interface behavior
//! (out of scope, see "Ported vs. skipped").

/// `RT64::Upscaler::QualityMode`'s five reachable resolution-ladder variants
/// (`rt64_upscaler.h`'s enum has eight total -- `Native`, `Auto`, `MAX` are
/// declared for other call sites this ticket does not touch, so they are
/// omitted here rather than modeled as unreachable). Explicit discriminants
/// match the pinned header's own `enum class QualityMode : int` values
/// exactly (`UltraPerformance = 0, Performance, Balanced, Quality,
/// UltraQuality`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityMode {
    UltraPerformance = 0,
    Performance = 1,
    Balanced = 2,
    Quality = 3,
    UltraQuality = 4,
}

/// Literal port of `RT64::Upscaler::getQualityAuto(int displayWidth, int
/// displayHeight)` (`rt64_upscaler.cpp:11-35`), minus the two debug-only
/// `assert`s (see module doc "Ported vs. skipped"). See module doc
/// "`getQualityAuto`'s exact thresholds and comparison direction" and
/// "`int * int` then widen to `uint64_t`" for the exact evaluation order this
/// reproduces.
pub fn get_quality_auto(display_width: i32, display_height: i32) -> QualityMode {
    let pixels_display = i64::from(display_width) * i64::from(display_height);
    let pixels_720p: i64 = 1280 * 720;
    let pixels_1080p: i64 = 1920 * 1080;
    let pixels_1440p: i64 = 2560 * 1440;
    let pixels_4k: i64 = 3840 * 2160;
    if pixels_display <= pixels_720p {
        QualityMode::UltraQuality
    } else if pixels_display <= pixels_1080p {
        QualityMode::Quality
    } else if pixels_display <= pixels_1440p {
        QualityMode::Balanced
    } else if pixels_display <= pixels_4k {
        QualityMode::Performance
    } else {
        QualityMode::UltraPerformance
    }
}

/// Literal port of `float3 WhiteBlackPoint(float3 bl, float3 wp, float3
/// color)` (`PostProcessPS.hlsl:44-46`): `(color - bl) / (wp - bl)`,
/// component-wise, subtraction on both sides before the division (matching
/// the HLSL's literal parenthesization and operator precedence). No guard
/// against `wp == bl` (see module doc "Unguarded division").
pub fn white_black_point(black: [f32; 3], white: [f32; 3], color: [f32; 3]) -> [f32; 3] {
    [
        (color[0] - black[0]) / (white[0] - black[0]),
        (color[1] - black[1]) / (white[1] - black[1]),
        (color[2] - black[2]) / (white[2] - black[2]),
    ]
}

/// Literal port of `PostProcessPS.hlsl:55`'s `float exposure = RtParams.
/// tonemapExposure / avgLuma;` -- a plain, unguarded division (see module doc
/// "Unguarded division"). `avg_luma` stands in for `gLumaAvg[uint2(0,0)]`
/// (an unported 1x1-texture read; see module doc "Ported vs. skipped").
pub fn tonemap_exposure(tonemap_exposure: f32, avg_luma: f32) -> f32 {
    tonemap_exposure / avg_luma
}

/// Literal port of `PostProcessPS.hlsl:48-60`'s `PSMain` tonemap arithmetic
/// (steps 1-5 in module doc "Tonemap curve, exact constants and evaluation
/// order"), minus the pixel-shader signature itself, `ColorMotionBlurred`'s
/// own texture-fetch-driven body (its already-computed `.rgb` result is
/// `color_motion_blurred`), and alpha (this function returns only the three
/// RGB components it owns -- see module doc step 5).
///
/// `color_motion_blurred` is `ColorMotionBlurred(uv).rgb` (caller-supplied,
/// per module doc "Ported vs. skipped"); `tonemap_exposure_value`,
/// `tonemap_black`, `tonemap_white` are `RtParams.tonemapExposure`/
/// `tonemapBlack`/`tonemapWhite`; `avg_luma` is `gLumaAvg[uint2(0,0)]`
/// (caller-supplied, per [`tonemap_exposure`]'s doc).
///
/// `ENABLE_EXPOSURE_ADJUSTMENT` is unconditionally active as pinned (module
/// doc "Ported vs. skipped"), so this function always performs the
/// exposure/white-black-point steps -- there is no "adjustment disabled"
/// code path to select between.
pub fn post_process_tonemap(
    color_motion_blurred: [f32; 3],
    tonemap_exposure_value: f32,
    avg_luma: f32,
    tonemap_black: [f32; 3],
    tonemap_white: [f32; 3],
) -> [f32; 3] {
    let floored = [
        color_motion_blurred[0].max(0.0),
        color_motion_blurred[1].max(0.0),
        color_motion_blurred[2].max(0.0),
    ];

    let exposure = tonemap_exposure(tonemap_exposure_value, avg_luma);
    let exposed = [
        floored[0] * exposure,
        floored[1] * exposure,
        floored[2] * exposure,
    ];

    let remapped = white_black_point(tonemap_black, tonemap_white, exposed);

    [
        remapped[0].clamp(0.0, 1.0),
        remapped[1].clamp(0.0, 1.0),
        remapped[2].clamp(0.0, 1.0),
    ]
}

/// Literal port of `PostProcessPS.hlsl:20-21`'s outer motion-blur gate:
/// `RtParams.motionBlurStrength > 0.0f && RtParams.motionBlurSamples > 0`.
/// `motion_blur_samples` is `uint` in the source (`RaytracingParams::
/// motionBlurSamples`); ported as `u32` (matching that unsigned width, so
/// `> 0` is simply `!= 0` -- there is no negative `uint` to compare against).
pub fn motion_blur_is_active(motion_blur_strength: f32, motion_blur_samples: u32) -> bool {
    motion_blur_strength > 0.0 && motion_blur_samples > 0
}

/// Literal port of `PostProcessPS.hlsl:28`'s loop bound: `RtParams.
/// motionBlurSamples` itself is the sample count a caller should iterate
/// `0..motion_blur_samples` over (HLSL `for (uint s = 0; s <
/// motionBlurSamples; s++)`). Provided as a named function (rather than
/// having callers read the field directly) only because this module has no
/// `RaytracingParams` type to read it from -- see module doc "Ported vs.
/// skipped".
pub fn motion_blur_sample_count(motion_blur_samples: u32) -> u32 {
    motion_blur_samples
}

/// Literal port of `PostProcessPS.hlsl:24,27,29`'s per-sample motion-blur
/// offset sequence -- the pure `(uv, flow, strength, samples, s)` ->
/// `sampleUV` computation inside `ColorMotionBlurred`'s `if (flowLength >
/// EPSILON)` branch, with the texture fetches removed (see module doc
/// "Ported vs. skipped"):
///
/// ```text
/// const float SampleStep = motionBlurStrength / motionBlurSamples;
/// float2 startUV = uv - (flow * motionBlurStrength / 2.0f);
/// float2 sampleUV = clamp(startUV + flow * s * SampleStep, (0,0), (1,1));
/// ```
///
/// `motion_blur_samples` widens `u32` -> `f32` for the `SampleStep` division
/// (HLSL's own implicit `uint`-to-`float` promotion at that division site);
/// `sample_index` (`s`) does the same. The two-literal-bound `clamp` uses
/// Rust's `f32::clamp` (see module doc "`saturate`/`clamp` semantics").
///
/// This function does not itself check `motionBlurStrength > 0.0f`,
/// `motionBlurSamples > 0`, or `flowLength > EPSILON` -- those are
/// [`motion_blur_is_active`] and the caller-computed `flowLength` gate (see
/// module doc "`length()`"), evaluated before a caller loops
/// `0..motion_blur_sample_count(...)` calling this function per `s`.
pub fn motion_blur_sample_offset(
    uv: [f32; 2],
    flow: [f32; 2],
    motion_blur_strength: f32,
    motion_blur_samples: u32,
    sample_index: u32,
) -> [f32; 2] {
    let sample_step = motion_blur_strength / (motion_blur_samples as f32);
    let start_uv = [
        uv[0] - (flow[0] * motion_blur_strength / 2.0),
        uv[1] - (flow[1] * motion_blur_strength / 2.0),
    ];
    let raw = [
        start_uv[0] + flow[0] * (sample_index as f32) * sample_step,
        start_uv[1] + flow[1] * (sample_index as f32) * sample_step,
    ];
    [raw[0].clamp(0.0, 1.0), raw[1].clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests;
