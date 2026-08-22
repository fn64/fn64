//! The CPU-determined light arithmetic of `src/shaders/Lights.hlsli`: the two
//! `Calculate*IntensitySimple` attenuation/falloff functions, the shared
//! perpendicular-basis construction, the per-sample spot/attenuation weight,
//! the `MAX_LIGHTS`-bounded candidate-light scan, and the intensity-weighted
//! roulette selection -- a literal port of the permitted MIT RT64 source
//! pinned at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/shaders/Lights.hlsli` (whole-file
//! SHA-256 `6d1fe05edada003b2ad0b081774a6009fc16aca597a0d9044fb6d675332b2177`,
//! 280 lines as the inventory counts them / 279 newline-terminated lines as
//! `wc -l` counts them, the file having no trailing newline). That digest was
//! computed independently here with `shasum -a 256` against the pinned
//! checkout at `src/shaders/Lights.hlsli` and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path="src/shaders/Lights.hlsli"].sources.port.sha256`, which records
//! the identical digest.
//!
//! ## Inventory drift: this file is a *fraction* port, digested whole
//!
//! The whole-file digest above is the only granularity
//! `docs/rt64-port-inventory.json` records, so once that inventory's
//! `ported_as` names this module, `src/shaders/Lights.hlsli` will read as
//! `ported` at *file* granularity. That would over-credit this card
//! substantially. **This module ports roughly 45 of the file's 279 lines --
//! about 16%.** The remaining ~84% is refused as GPU-only (see "Refused
//! surface" below), and no part of this module should be read as a claim that
//! `Lights.hlsli`'s shading behavior as a whole has been ported. The inventory
//! currently lists `"ported_as": []` and `"port_state": "not-started"` for this
//! path; `scripts/lint-docs.py`'s inventory scanner is expected to report an
//! `ported_as drift` line until a follow-up regenerates the inventory. This
//! module's writable surface does not include
//! `docs/rt64-port-inventory.json`, so that reconciliation is deliberately
//! left to the owning ticket rather than done here.
//!
//! ## Porting criterion
//!
//! A construct is ported when its behavior is **fully determined by values and
//! control flow present in the cited file** -- no acceleration structure, no
//! sampler, no dispatch context. Concretely: a construct is admitted when it
//! can be evaluated from its scalar/vector operands alone. It is refused when
//! evaluating it requires `TraceRay`, a `RaytracingAccelerationStructure`, a
//! `Texture2D` `Load`, a `StructuredBuffer` binding, or a `uint2 launchIndex`
//! that only a dispatch supplies.
//!
//! Two constructs sit exactly on that line and are admitted with the
//! sampler-derived value **hoisted to a parameter**, never synthesized:
//!
//! - [`sample_weights`] takes `sample_position` rather than deriving it from
//!   `getBlueNoise(...)`. The blue-noise `Load` decides *where* a sample lands;
//!   everything downstream of that position (`Lights.hlsli:99-106`) is pure.
//! - [`roulette_select`] takes the scalar `r` rather than computing
//!   `getBlueNoise(...).r * randomRange`. The `Load` decides *which* scalar;
//!   the walk over `sLightIntensities` (`Lights.hlsli:203-215`) is pure.
//!
//! In both cases the hoisted value is an explicit input, so no test in this
//! module ever pretends to know what the blue-noise texture would have
//! returned.
//!
//! ## Ported surface, verbatim
//!
//! ```text
//! // Lights.hlsli:13
//! #define MAX_LIGHTS 24
//!
//! // Lights.hlsli:42-53
//! float CalculateLightIntensitySimple(PointLight pointLight, float3 position, float3 normal, float ignoreNormalFactor) {
//!     float3 lightPosition = pointLight.position;
//!     float lightRadius = pointLight.attenuationRadius;
//!     float lightAttenuation = pointLight.attenuationExponent;
//!     float lightDistance = length(position - lightPosition);
//!     float3 lightDirection = normalize(lightPosition - position);
//!     float NdotL = dot(normal, lightDirection);
//!     const float surfaceBiasDotOffset = 0.707106f;
//!     float surfaceBias = max(lerp(NdotL, 1.0f, ignoreNormalFactor) + surfaceBiasDotOffset, 0.0f);
//!     float sampleIntensityFactor = pow(max(1.0f - (lightDistance / lightRadius), 0.0f), lightAttenuation) * surfaceBias;
//!     return sampleIntensityFactor * dot(pointLight.diffuseColor, float3(1.0f, 1.0f, 1.0f));
//! }
//!
//! // Lights.hlsli:58-64
//! float CalculateShadowIntensitySimple(PointLight pointLight, float3 position) {
//!     float3 lightPosition = pointLight.position;
//!     float lightRadius = pointLight.attenuationRadius;
//!     float lightAttenuation = pointLight.attenuationExponent;
//!     float lightDistance = length(position - lightPosition);
//!     return pow(max(1.0f - (lightDistance / lightRadius), 0.0f), lightAttenuation);
//! }
//!
//! // Lights.hlsli:81-86 (identical at :139-144)
//!     float3 perpX = cross(-lightDirection, float3(0.f, 1.0f, 0.f));
//!     if (all(perpX == 0.0f)) {
//!         perpX.x = 1.0;
//!     }
//!
//!     float3 perpY = cross(perpX, -lightDirection);
//!
//! // Lights.hlsli:97-106 (attenuation/spot half identical at :153-159)
//!         float3 samplePosition = lightPosition + perpX * sampleCoordinate.x * lightPointRadius + perpY * sampleCoordinate.y * lightPointRadius;
//!         float3 sampleDirection = normalize(samplePosition - position);
//!         float lightSpotDot = dot(sampleDirection, lightSpotDirection);
//!         if (lightSpotDot <= lightSpotMaxCosine) {
//!             float spotIntensity = 1.0f - clamp((lightSpotDot - lightSpotFalloffCosine) / (lightSpotMaxCosine - lightSpotFalloffCosine), 0.0f, 1.0f);
//!             float sampleDistance = length(position - samplePosition);
//!             float sampleIntensityFactor = pow(max(1.0f - (sampleDistance / lightRadius), 0.0f), lightAttenuation) * spotIntensity;
//!             float3 reflectedLight = reflect(-sampleDirection, normal);
//!             float NdotL = max(dot(normal, sampleDirection), 0.0f);
//!             float sampleLambertFactor = lerp(NdotL, 1.0f, ignoreNormalFactor) * sampleIntensityFactor;
//!
//! // Lights.hlsli:182-192 (identical shape at :239-249, differing only in the
//! // intensity function called)
//!         for (uint l = 0; (l < pointLightsCount) && (sLightCount < MAX_LIGHTS); l++) {
//!             if (lightGroupMaskBits & pointLights[l].groupBits) {
//!                 float lightIntensity = CalculateLightIntensitySimple(pointLights[l], position, normal, ignoreNormalFactor);
//!                 if (lightIntensity > EPSILON) {
//!                     sLightIntensities[sLightCount] = lightIntensity;
//!                     sLightIndices[sLightCount] = l;
//!                     totalLightIntensity += lightIntensity;
//!                     sLightCount++;
//!                 }
//!             }
//!         }
//!
//! // Lights.hlsli:200-215 (identical at :257-272)
//!     bool useProbability = lLightCount == 1;
//!     for (uint s = 0; s < lLightCount; s++) {
//!         float r = getBlueNoise(blueNoiseTexture, launchIndex, frameCount + s).r * randomRange;
//!         uint chosen = 0;
//!         float rLightIntensity = sLightIntensities[chosen];
//!         while ((chosen < (sLightCount - 1)) && (r >= rLightIntensity)) {
//!             chosen++;
//!             rLightIntensity += sLightIntensities[chosen];
//!         }
//!
//!         // Store and clear the light intensity from the array.
//!         float cLightIntensity = sLightIntensities[chosen];
//!         uint cLightIndex = sLightIndices[chosen];
//!         float invProbability = useProbability ? (randomRange / cLightIntensity) : 1.0f;
//!         sLightIntensities[chosen] = 0.0f;
//!         randomRange -= cLightIntensity;
//! ```
//!
//! `EPSILON` is `1e-6`, from `src/shaders/Math.hlsli:7` (already quoted in
//! this crate's `math_hlsli.rs` module doc, same pinned commit).
//!
//! ## Refused surface (~234 of 279 lines, ~84%)
//!
//! Refused because evaluating them requires state this file does not carry:
//!
//! - `TraceShadow` (`:15-40`, 26 lines) -- takes a
//!   `RaytracingAccelerationStructure`, builds a `RayDesc`, sets
//!   `RAY_FLAG_*` bits, and calls the `TraceRay` intrinsic. Its return value
//!   is decided entirely by scene geometry. Nothing CPU-side.
//! - `ComputeLight`'s and `ComputeShadow`'s bodies as whole functions
//!   (`:66-122`, `:127-168`) -- each opens with a `Texture2D<float4>`
//!   `getBlueNoise` load per loop iteration and (in `ComputeLight`) an inner
//!   `TraceShadow`. Only the pure arithmetic *between* those two calls is
//!   ported, via [`perpendicular_basis`] and [`sample_weights`]; the loop that
//!   drives them and the `1.0f / maxSamples` accumulation are not, because the
//!   per-iteration input is a texture load.
//! - `reflect(-sampleDirection, normal)` and the specular term at `:104`,
//!   `:112` -- `reflect` is pure and could be ported, but its only consumer is
//!   the specular factor, which is multiplied by `specular`, a value that in
//!   every RT64 call site comes from a G-buffer/texture read. It is refused to
//!   avoid a port whose only exercised path is a synthesized input. See
//!   "Nonclaims".
//! - `ComputeLightsRandom` / `ComputeShadowsRandom` as whole functions
//!   (`:170-223`, `:228-280`) -- both take a `StructuredBuffer<PointLight>`, a
//!   `RaytracingAccelerationStructure`, a `uint2 launchIndex`, and a
//!   `Texture2D<float4>`. Their *scan* and *roulette* halves are ported
//!   separately (see above); the `resultLight += ComputeLight(...)` /
//!   `resultShadow += ComputeShadow(...)` accumulation is not, because each
//!   addend requires a trace.
//! - `ExtraParams` and its `applyExtraAttributes` -- from
//!   `src/shared/rt64_extra_params.h`, a different inventory path, not this
//!   card's source. The four `ExtraParams` fields this file reads
//!   (`ignoreNormalFactor`, `specularExponent`, `shadowRayBias`,
//!   `lightGroupMaskBits`) are taken as plain scalar parameters here rather
//!   than by porting that struct.
//!
//! ## Reuse, not new type
//!
//! - [`PointLight`] is re-exported from this crate's
//!   [`crate::rt64_light_estimation`], which already ports
//!   `src/shared/rt64_point_light.h`'s `interop::PointLight` field-for-field.
//!   No second light struct is defined here.
//! - `float3` is [`Vec3`], re-exported from `fn64_render_ir`'s crate root and
//!   defined at `crates/fn64-render-ir/src/rsp_math.rs:42`, the workspace's HLSL
//!   `float3` equivalent, already used by `rt64_light_estimation`,
//!   `rt64_math_matrix`, and `rt64_math_decompose`. `Vec3::sub`, `::scale`,
//!   and `::dot` are used where the source writes `-`, `*`, and `dot`.
//!   `Vec3::dot` is `x*rhs.x + y*rhs.y + z*rhs.z`, matching HLSL `dot`'s
//!   left-to-right summation order.
//! - `float2` appears only as `sampleCoordinate`, whose two components are
//!   consumed independently at `:97`; it is passed as two `f32` scalars rather
//!   than introducing a `Vec2`.
//! - `EPSILON` is re-stated as a local constant rather than exported from
//!   `math_hlsli`, which quotes it in a doc comment but does not define it as
//!   an item.
//!
//! ## HLSL scalar-intrinsic semantics used here
//!
//! Every intrinsic below is written as a literal expression, in the source's
//! operand order, never as the Rust standard-library method of the same name.
//! `f32::max`/`f32::min` are NaN-*absorbing* (they return the non-NaN operand);
//! HLSL's are NaN-*propagating* in one argument position and not the other.
//! Using the std methods would silently change every NaN result in this file.
//!
//! - `max(a, b)` is `if b > a { b } else { a }`. So `max(NaN, 0.0)` is `NaN`
//!   (`0.0 > NaN` is false, yielding `a`), while `max(0.0, NaN)` is `0.0`
//!   (`NaN > 0.0` is false, yielding `a`). Rust's `f32::max` returns `0.0` for
//!   both. Signed zero: `max(-0.0, 0.0)` is `-0.0`, since `0.0 > -0.0` is
//!   false. Every `max` in the ported source is `max(x, 0.0f)` -- the
//!   NaN-propagating position -- so a NaN distance or dot product survives to
//!   the result rather than being scrubbed to zero.
//! - `min(a, b)` is `if b < a { b } else { a }`, used once, at
//!   `min(sLightCount, maxLightCount)` on `uint`, where NaN cannot arise.
//! - `clamp(x, lo, hi)` is `min(max(x, lo), hi)`, i.e.
//!   `max` then `min` with the semantics above. `clamp(NaN, 0.0, 1.0)` is
//!   therefore `NaN`: `max(NaN, 0.0)` is `NaN`, then `min(NaN, 1.0)` is `NaN`.
//! - `saturate(x)` is `clamp(x, 0, 1)`, so it likewise propagates NaN under
//!   this lowering. `saturate` does not appear in the ported subset (it is used
//!   only on the blue-noise coordinate at `:95`/`:151` and on the refused
//!   specular term at `:112`), so this is recorded for completeness, not
//!   relied upon.
//! - `lerp(a, b, t)` is `a + t * (b - a)`. It is **not** written as the
//!   precise form `a * (1 - t) + b * t`; the two are algebraically equal but
//!   round differently, differing by one ulp at (for instance) `a =
//!   0.16333017`, `t = 0.58738482`, `b = 1.0`. That 1-ulp gap is *sometimes*
//!   absorbed downstream by the `+0.707106` bias and the trailing multiplies
//!   and sometimes survives to the returned intensity; both cases are tested,
//!   so the form choice is pinned as observable rather than asserted to matter
//!   everywhere. Also note `lerp(a, b, 0.0)` returns exactly `a` under this
//!   form, while `lerp(a, b, 1.0)` returns `a + (b - a)`, which is not always
//!   exactly `b`. Both ported `lerp` calls have `b == 1.0f`.
//! - `length(v)` is `sqrt(dot(v, v))`, and `normalize(v)` is `v / length(v)`,
//!   both unguarded against a zero-length input. Note the source computes
//!   `sampleDistance` as `length(position - samplePosition)` (`:102`) but
//!   `sampleDirection` as `normalize(samplePosition - position)` (`:98`) --
//!   opposite operand orders. The `length` one is *order-independent*: each
//!   component is squared, and `(-x) * (-x)` is bit-identical to `x * x` in
//!   IEEE-754 for every finite, denormal, zero, and infinite `x`, so writing
//!   it either way gives the same f32. No test can distinguish the two, and
//!   this module makes no claim that it can; the source's order is kept
//!   because it is the source's order, not because it is observable. The
//!   `normalize` one *is* order-dependent (it flips the direction's sign) and
//!   is tested.
//! - `pow(x, y)` is IEEE `powf`. `pow(0.0, 0.0)` is `1.0`.
//! - `cross(a, b)` is the standard right-handed cross product with the
//!   component order `(a.y*b.z - a.z*b.y, a.z*b.x - a.x*b.z, a.x*b.y -
//!   a.y*b.x)`.
//!
//! ## Admitted domain
//!
//! Ten upstream behaviors are **pinned, not fixed**. Each is a real property of
//! the pinned source, reproduced exactly, and each has at least one test:
//!
//! 1. **`attenuationRadius == 0.0` divides by zero** (`:51`, `:63`, `:103`).
//!    `lightDistance / 0.0` is `+Inf` for a positive distance, so
//!    `1.0 - Inf` is `-Inf`, `max(-Inf, 0.0)` is `0.0`, and `pow(0.0, e)` is
//!    `0.0` for `e > 0` but `1.0` for `e == 0.0`. When the distance is also
//!    `0.0` the division is `0.0/0.0 == NaN`, which `max`'s NaN-propagating
//!    position carries all the way out. No guard is added.
//! 2. **`surfaceBias`'s `+0.707106f` admits back-facing light** (`:49-50`).
//!    With `ignoreNormalFactor == 0.0`, a surface whose normal points *away*
//!    from the light (`NdotL` in `(-0.707106, 0.0)`) still yields a strictly
//!    positive `surfaceBias` and therefore a strictly positive intensity. The
//!    constant is `0.707106f`, a truncated `1/sqrt(2)` (`0.70710678...`), not
//!    the rounded `0.7071068f`; the truncation is preserved literally.
//! 3. **The spot test is `<=`, not `>=`** (`:100`, `:156`). A sample
//!    contributes when `lightSpotDot <= lightSpotMaxCosine`, i.e. when the
//!    sample direction is *less* aligned with the spot direction than the cone
//!    limit. Read as a cone test this is inverted; it is reproduced as written.
//! 4. **`spotMaxCosine == spotFalloffCosine` divides by zero** (`:101`,
//!    `:157`). The numerator `lightSpotDot - lightSpotFalloffCosine` is `<= 0`
//!    inside the guarded branch, so the quotient is `-Inf`, `0.0/0.0 == NaN`,
//!    or `-0.0`; `clamp` then yields `0.0`, `NaN`, or `-0.0` respectively, and
//!    `spotIntensity` is `1.0`, `NaN`, or `1.0`. No guard is added.
//! 5. **`all(perpX == 0.0f)` sets only `.x`** (`:82-83`). The fixup writes
//!    `perpX.x = 1.0` and leaves `.y`/`.z` at whatever they were -- which is
//!    `0.0` only because the branch requires all three to be zero. A
//!    *partially* zero cross product (e.g. `lightDirection` exactly `+Y`, which
//!    gives `perpX == (0,0,0)`, versus one merely near `+Y`) is not fixed up.
//!    Note `all(perpX == 0.0f)` is true for `-0.0` components as well, since
//!    `-0.0 == 0.0`.
//! 6. **`dot(diffuseColor, float3(1,1,1))` sums signed channels** (`:52`). A
//!    light with negative channels can cancel to zero or negative total
//!    intensity, which then fails the `> EPSILON` scan test.
//! 7. **The scan bound is `MAX_LIGHTS`, the array size is `MAX_LIGHTS + 1`**
//!    (`:178-179`, `:182`). The loop stops at `sLightCount < MAX_LIGHTS`, so
//!    index 24 of the 25-element arrays is never written. The extra slot is
//!    dead. The bound is on *accepted* lights, not on lights examined: the scan
//!    keeps walking `pointLights` while rejecting them, and stops early only
//!    once 24 have been accepted.
//! 8. **`sLightCount - 1` on a `uint`** (`:205`, `:262`). If `sLightCount` were
//!    `0` this would wrap to `0xFFFFFFFF`. It cannot be reached: the enclosing
//!    `for (uint s = 0; s < lLightCount; ...)` requires `lLightCount > 0`, and
//!    `lLightCount = min(sLightCount, maxLightCount)`, so `sLightCount >= 1`.
//!    [`roulette_select`] therefore takes a non-empty slice and this module
//!    documents the precondition rather than reproducing a wrap. This is the
//!    one place the port is *narrower* than the source rather than equal, and
//!    it is narrower only over an unreachable input.
//! 9. **`invProbability` divides by the chosen intensity, unguarded** (`:213`,
//!    `:270`). `cLightIntensity` is `> EPSILON` when first stored, but the
//!    roulette *zeroes* the chosen slot (`:214`) and a later iteration can
//!    re-select that same index once every candidate has been exhausted --
//!    at which point `cLightIntensity` is `0.0` and `invProbability` would be
//!    `randomRange / 0.0`. `useProbability` is only true when
//!    `lLightCount == 1`, i.e. when exactly one iteration runs, so the divide
//!    is unreachable in the source. [`roulette_select`] returns the raw chosen
//!    intensity and leaves the `invProbability` decision to the caller, so
//!    nothing here divides.
//! 10. **`ComputeShadowsRandom` clamps its result, `ComputeLightsRandom` does
//!     not** (`:279` vs `:222`). Both accumulate the same way. Only the shadow
//!     path's final `clamp(resultShadow, 0.0f, 1.0f)` is present. Neither final
//!     accumulation is ported (both require a trace per addend), so this
//!     asymmetry is recorded but not exercised.
//!
//! ## Nonclaims
//!
//! - **This is not a port of `Lights.hlsli`.** It is a port of ~16% of it. It
//!   computes no lighting result. `ComputeLight`, `ComputeShadow`,
//!   `ComputeLightsRandom`, `ComputeShadowsRandom`, and `TraceShadow` return
//!   values that this module cannot produce and does not approximate.
//! - No claim of GPU/CPU bit-parity. Nothing here has been differentially
//!   tested against a compiled DXIL/SPIR-V build of this shader. HLSL permits
//!   an implementation to contract `a * b + c` into an FMA and to reassociate
//!   under fast-math; every expected value in this module's tests is
//!   hand-derived against the *unfused, source-order* IEEE-754 single-precision
//!   evaluation written here, which is one admissible lowering, not the only
//!   one. A real GPU may differ in the last ulp on the multiply-add chains in
//!   [`sample_weights`] and [`calculate_light_intensity_simple`].
//! - No claim about `saturate`'s NaN behavior on hardware. The lowering
//!   documented above (`min(max(x,0),1)`, NaN-propagating) is the one this
//!   module states; DXC has historically emitted a `saturate` modifier whose
//!   NaN result is target-dependent. `saturate` is not used in the ported
//!   subset, so nothing here depends on the answer.
//! - `reflect` and the specular term are refused, not ported-and-untested. No
//!   specular value appears anywhere in this module.
//! - The blue-noise sequence is never modelled. [`sample_weights`] and
//!   [`roulette_select`] take the sampler-derived value as a parameter, and no
//!   test asserts what that parameter *would* be for any pixel or frame.
//! - `MAX_LIGHTS` is pinned as a constant and used as this module's scan bound,
//!   but no claim is made that 24 is a hardware or ABI limit; it is a
//!   `#define` local to this shader file.

use fn64_render_ir::Vec3;

pub use crate::rt64_light_estimation::PointLight;

/// `#define MAX_LIGHTS 24` (`Lights.hlsli:13`): the maximum number of
/// candidate lights [`scan_candidate_lights`] will accept.
pub const MAX_LIGHTS: u32 = 24;

/// The `sLightIndices`/`sLightIntensities` array length, `MAX_LIGHTS + 1`
/// (`Lights.hlsli:178-179`). See "Admitted domain" item 7: the last slot is
/// dead, because the scan loop stops at `sLightCount < MAX_LIGHTS`.
pub const LIGHT_ARRAY_LEN: usize = MAX_LIGHTS as usize + 1;

/// `#define EPSILON 1e-6` (`src/shaders/Math.hlsli:7`), the scan's
/// strictly-greater acceptance threshold at `Lights.hlsli:185` / `:242`.
pub const EPSILON: f32 = 1e-6;

/// `const float surfaceBiasDotOffset = 0.707106f` (`Lights.hlsli:49`).
///
/// Truncated, not rounded: `1/sqrt(2)` is `0.70710678...`, so the rounded
/// 7-digit form would be `0.7071068`. The source's truncation is preserved.
pub const SURFACE_BIAS_DOT_OFFSET: f32 = 0.707106;

/// HLSL `max(a, b)`, lowered as `b > a ? b : a`.
///
/// Returns `a` whenever the comparison is false, which makes `max(NaN, b)`
/// return `NaN` and `max(a, NaN)` return `a`. Rust's `f32::max` returns the
/// non-NaN operand in both cases and must not be substituted.
#[inline]
fn hlsl_max(a: f32, b: f32) -> f32 {
    if b > a {
        b
    } else {
        a
    }
}

/// HLSL `min(a, b)`, lowered as `b < a ? b : a`.
///
/// Same asymmetry as [`hlsl_max`]: `min(NaN, b)` is `NaN`, `min(a, NaN)` is
/// `a`.
#[inline]
fn hlsl_min(a: f32, b: f32) -> f32 {
    if b < a {
        b
    } else {
        a
    }
}

/// HLSL `clamp(x, lo, hi)`, lowered as `min(max(x, lo), hi)`.
#[inline]
fn hlsl_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    hlsl_min(hlsl_max(x, lo), hi)
}

/// HLSL `lerp(a, b, t)`, lowered as `a + t * (b - a)`.
///
/// Not the `a*(1-t) + b*t` form; see the module doc's intrinsic-semantics
/// section.
#[inline]
fn hlsl_lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// HLSL `length(v)`, lowered as `sqrt(dot(v, v))` with `dot`'s left-to-right
/// summation order.
#[inline]
fn hlsl_length(v: Vec3) -> f32 {
    v.dot(v).sqrt()
}

/// HLSL `normalize(v)`, lowered as `v / length(v)`, unguarded against a
/// zero-length input.
#[inline]
fn hlsl_normalize(v: Vec3) -> Vec3 {
    let len = hlsl_length(v);
    Vec3::new(v.x / len, v.y / len, v.z / len)
}

/// HLSL `cross(a, b)`.
#[inline]
fn hlsl_cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// HLSL unary `-v` on a `float3`, negating each component (so `-0.0` becomes
/// `+0.0` and vice versa; this matters at `Lights.hlsli:81`, where the negated
/// direction feeds a `cross` whose all-zero result is then tested with `==`).
#[inline]
fn hlsl_neg(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, -v.y, -v.z)
}

/// `CalculateLightIntensitySimple` (`Lights.hlsli:42-53`): the scalar
/// importance weight the candidate scan uses to rank lights, combining an
/// exponential distance falloff with a normal-facing bias.
///
/// Preserves, per "Admitted domain": the unguarded
/// `lightDistance / attenuation_radius` (items 1), the `+0.707106f` bias that
/// admits back-facing surfaces (item 2), and the signed-channel
/// `dot(diffuseColor, (1,1,1))` scale (item 6). `normalize` on a
/// zero-length `lightPosition - position` yields NaN components, which the
/// NaN-propagating `max(..., 0.0f)` carries out.
pub fn calculate_light_intensity_simple(
    point_light: &PointLight,
    position: Vec3,
    normal: Vec3,
    ignore_normal_factor: f32,
) -> f32 {
    let light_position = point_light.position;
    let light_radius = point_light.attenuation_radius;
    let light_attenuation = point_light.attenuation_exponent;
    let light_distance = hlsl_length(position.sub(light_position));
    let light_direction = hlsl_normalize(light_position.sub(position));
    let n_dot_l = normal.dot(light_direction);
    let surface_bias = hlsl_max(
        hlsl_lerp(n_dot_l, 1.0, ignore_normal_factor) + SURFACE_BIAS_DOT_OFFSET,
        0.0,
    );
    let sample_intensity_factor =
        hlsl_max(1.0 - (light_distance / light_radius), 0.0).powf(light_attenuation) * surface_bias;
    sample_intensity_factor * point_light.diffuse_color.dot(Vec3::new(1.0, 1.0, 1.0))
}

/// `CalculateShadowIntensitySimple` (`Lights.hlsli:58-64`): the same distance
/// falloff as [`calculate_light_intensity_simple`] with **no** normal bias and
/// **no** diffuse-color scale.
///
/// The upstream `TODO` at `:55-56` notes this is a copy of the other function;
/// the duplication is preserved rather than factored, because the two differ
/// in exactly which trailing factors they apply and merging them would require
/// choosing an evaluation order the source does not specify.
pub fn calculate_shadow_intensity_simple(point_light: &PointLight, position: Vec3) -> f32 {
    let light_position = point_light.position;
    let light_radius = point_light.attenuation_radius;
    let light_attenuation = point_light.attenuation_exponent;
    let light_distance = hlsl_length(position.sub(light_position));
    hlsl_max(1.0 - (light_distance / light_radius), 0.0).powf(light_attenuation)
}

/// The `perpX`/`perpY` disc basis built at `Lights.hlsli:81-86` (byte-identical
/// at `:139-144`), used to place area-light samples on a disc facing the
/// shaded point.
///
/// `light_direction` is the already-normalized `normalize(lightPosition -
/// position)` of `:74` / `:132`; this function does not renormalize it.
///
/// Preserves "Admitted domain" item 5: the degenerate fixup fires only when
/// `all(perpX == 0.0f)` and writes only `.x`. `perpY` is then computed from the
/// *fixed-up* `perpX`, so the degenerate case yields
/// `perpY = cross((1,0,0), -lightDirection)`.
pub fn perpendicular_basis(light_direction: Vec3) -> (Vec3, Vec3) {
    let neg_dir = hlsl_neg(light_direction);
    let mut perp_x = hlsl_cross(neg_dir, Vec3::new(0.0, 1.0, 0.0));
    if perp_x.x == 0.0 && perp_x.y == 0.0 && perp_x.z == 0.0 {
        perp_x.x = 1.0;
    }

    let perp_y = hlsl_cross(perp_x, neg_dir);
    (perp_x, perp_y)
}

/// `samplePosition` (`Lights.hlsli:97`, identical at `:153`): the disc-offset
/// sample point, given a blue-noise-derived unit-disc coordinate.
///
/// `sample_coordinate_x`/`_y` are the two components of the `float2` the
/// source derives from `getBlueNoise(...)` at `:94-95`; they are inputs here
/// rather than being sampled, per the module doc's porting criterion. The
/// expression order is preserved exactly:
/// `lightPosition + perpX*c.x*r + perpY*c.y*r`, with each scale applied
/// left-to-right before the two additions.
pub fn sample_position(
    light_position: Vec3,
    perp_x: Vec3,
    perp_y: Vec3,
    sample_coordinate_x: f32,
    sample_coordinate_y: f32,
    light_point_radius: f32,
) -> Vec3 {
    let a = perp_x.scale(sample_coordinate_x).scale(light_point_radius);
    let b = perp_y.scale(sample_coordinate_y).scale(light_point_radius);
    Vec3::new(
        light_position.x + a.x + b.x,
        light_position.y + a.y + b.y,
        light_position.z + a.z + b.z,
    )
}

/// `lightPointRadius = (diSamples > 0) ? pointLight.pointRadius : 0.0f`
/// (`Lights.hlsli:80`, identical at `:138`): the disc radius collapses to a
/// point light when direct-illumination sampling is disabled.
pub fn light_point_radius(di_samples: u32, point_radius: f32) -> f32 {
    if di_samples > 0 {
        point_radius
    } else {
        0.0
    }
}

/// `const uint maxSamples = max(diSamples, 1)` (`Lights.hlsli:88`, identical at
/// `:146`): the sample loop always runs at least once, so `diSamples == 0`
/// still produces one sample -- with [`light_point_radius`] having collapsed
/// the disc to a point.
pub fn max_samples(di_samples: u32) -> u32 {
    if 1 > di_samples {
        1
    } else {
        di_samples
    }
}

/// The per-sample weights computed inside the guarded branch at
/// `Lights.hlsli:99-106`, returned only when the spot test admits the sample.
///
/// Returns `None` exactly when `lightSpotDot <= lightSpotMaxCosine` is false --
/// i.e. when the source's `if` at `:100` / `:156` does not fire and the sample
/// contributes nothing to any accumulator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleWeights {
    /// `sampleDirection` (`:98`), the normalized direction from the shaded
    /// point to the disc sample.
    pub sample_direction: Vec3,
    /// `lightSpotDot` (`:99`).
    pub light_spot_dot: f32,
    /// `spotIntensity` (`:101` / `:157`).
    pub spot_intensity: f32,
    /// `sampleDistance` (`:102` / `:158`).
    pub sample_distance: f32,
    /// `sampleIntensityFactor` (`:103` / `:159`), the attenuated,
    /// spot-modulated weight shared by the light and shadow paths.
    pub sample_intensity_factor: f32,
    /// `sampleLambertFactor` (`:106`). Present only on the light path;
    /// `ComputeShadow` computes no Lambert term.
    pub sample_lambert_factor: f32,
}

/// `Lights.hlsli:98-106`: everything the sample loop computes between the
/// blue-noise load and the `TraceShadow` call.
///
/// `light_spot_direction` is `normalize(pointLight.direction)` from `:75` /
/// `:133`; it is taken pre-normalized. `normal` and `ignore_normal_factor` feed
/// only the Lambert term, which `ComputeShadow` (`:156-161`) omits -- the
/// shadow path uses the same [`SampleWeights::sample_intensity_factor`] and
/// ignores [`SampleWeights::sample_lambert_factor`].
///
/// Preserves, per "Admitted domain": the `<=` spot test (item 3), the
/// unguarded `spotMaxCosine - spotFalloffCosine` denominator (item 4), and the
/// unguarded `sampleDistance / lightRadius` (item 1). The `NdotL` here is
/// `max(dot(normal, sampleDirection), 0.0f)`, clamped -- unlike the *unclamped*
/// `NdotL` in [`calculate_light_intensity_simple`], which is a real difference
/// between the two functions and not a transcription slip.
#[allow(clippy::too_many_arguments)]
pub fn sample_weights(
    position: Vec3,
    sample_position: Vec3,
    normal: Vec3,
    light_spot_direction: Vec3,
    light_spot_falloff_cosine: f32,
    light_spot_max_cosine: f32,
    light_radius: f32,
    light_attenuation: f32,
    ignore_normal_factor: f32,
) -> Option<SampleWeights> {
    let sample_direction = hlsl_normalize(sample_position.sub(position));
    let light_spot_dot = sample_direction.dot(light_spot_direction);
    if !(light_spot_dot <= light_spot_max_cosine) {
        return None;
    }

    let spot_intensity = 1.0
        - hlsl_clamp(
            (light_spot_dot - light_spot_falloff_cosine)
                / (light_spot_max_cosine - light_spot_falloff_cosine),
            0.0,
            1.0,
        );
    let sample_distance = hlsl_length(position.sub(sample_position));
    let sample_intensity_factor = hlsl_max(1.0 - (sample_distance / light_radius), 0.0)
        .powf(light_attenuation)
        * spot_intensity;
    let n_dot_l = hlsl_max(normal.dot(sample_direction), 0.0);
    let sample_lambert_factor =
        hlsl_lerp(n_dot_l, 1.0, ignore_normal_factor) * sample_intensity_factor;

    Some(SampleWeights {
        sample_direction,
        light_spot_dot,
        spot_intensity,
        sample_distance,
        sample_intensity_factor,
        sample_lambert_factor,
    })
}

/// The candidate-light scan's accumulated result (`Lights.hlsli:177-192`,
/// identical shape at `:235-249`).
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateLights {
    /// `sLightIndices[0..sLightCount]`: the indices into the caller's light
    /// slice, in scan order.
    pub indices: Vec<u32>,
    /// `sLightIntensities[0..sLightCount]`, parallel to `indices`.
    pub intensities: Vec<f32>,
    /// `totalLightIntensity`, accumulated in scan order (`+=` left to right,
    /// so the summation order is load-bearing for float rounding).
    pub total_intensity: f32,
}

/// `Lights.hlsli:182-192`: walk `pointLights`, keeping those whose `groupBits`
/// intersect `lightGroupMaskBits` and whose intensity exceeds `EPSILON`,
/// stopping once `MAX_LIGHTS` have been accepted.
///
/// `intensity_of` is the per-light weight function -- pass
/// [`calculate_light_intensity_simple`]'s result for the `ComputeLightsRandom`
/// path (`:184`) and [`calculate_shadow_intensity_simple`]'s for the
/// `ComputeShadowsRandom` path (`:241`). Those two call sites are the *only*
/// difference between the source's two otherwise byte-identical scans, which
/// is why one function serves both.
///
/// The `if (lightGroupMaskBits > 0)` guard at `:176` / `:234` is the caller's:
/// with a zero mask the source skips the scan entirely and returns the zero
/// result, and a zero mask here would in any case accept nothing, since
/// `0 & anything` is `0`.
///
/// Preserves "Admitted domain" item 7: the bound is on accepted lights, so a
/// slice of 1000 lights of which 24 pass is scanned only until the 24th is
/// accepted, but a slice of 1000 lights of which none pass is scanned in full.
pub fn scan_candidate_lights<F>(
    light_count: u32,
    light_group_mask_bits: u32,
    group_bits_of: impl Fn(u32) -> u32,
    intensity_of: F,
) -> CandidateLights
where
    F: Fn(u32) -> f32,
{
    let mut indices: Vec<u32> = Vec::new();
    let mut intensities: Vec<f32> = Vec::new();
    let mut total_intensity = 0.0f32;

    let mut l = 0u32;
    while l < light_count && (indices.len() as u32) < MAX_LIGHTS {
        if light_group_mask_bits & group_bits_of(l) != 0 {
            let light_intensity = intensity_of(l);
            if light_intensity > EPSILON {
                intensities.push(light_intensity);
                indices.push(l);
                total_intensity += light_intensity;
            }
        }
        l += 1;
    }

    CandidateLights {
        indices,
        intensities,
        total_intensity,
    }
}

/// `uint lLightCount = min(sLightCount, maxLightCount)` (`Lights.hlsli:195`,
/// identical at `:252`): how many roulette draws the source performs.
pub fn selected_light_count(candidate_count: u32, max_light_count: u32) -> u32 {
    if max_light_count < candidate_count {
        max_light_count
    } else {
        candidate_count
    }
}

/// `bool useProbability = lLightCount == 1` (`Lights.hlsli:200`, identical at
/// `:257`).
///
/// The upstream comment at `:197-199` explains why: the probability of a
/// dependent draw without replacement is not trivially computable, so the
/// inverse-probability weighting is applied only in the single-draw case. See
/// "Admitted domain" item 9 for why this also makes the unguarded
/// `randomRange / cLightIntensity` divide unreachable.
pub fn use_probability(selected_light_count: u32) -> bool {
    selected_light_count == 1
}

/// One roulette draw's outcome (`Lights.hlsli:203-215`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouletteDraw {
    /// `chosen`: the slot in `sLightIntensities`/`sLightIndices`.
    pub chosen: usize,
    /// `cLightIntensity`: the intensity read *before* the slot is zeroed.
    pub chosen_intensity: f32,
    /// `cLightIndex`: `sLightIndices[chosen]`, the caller's light index.
    pub chosen_index: u32,
    /// `randomRange` after `randomRange -= cLightIntensity` (`:215`).
    pub remaining_range: f32,
}

/// `Lights.hlsli:203-215`: the intensity-weighted walk that picks a light, then
/// zeroes its slot and shrinks the range so the next draw samples without
/// replacement.
///
/// `r` is the sampler-derived scalar the source computes as
/// `getBlueNoise(...).r * randomRange` (`:202` / `:259`). It is a parameter
/// here rather than being sampled, per the module doc's porting criterion.
/// `intensities` is `sLightIntensities` and is mutated in place, matching the
/// source's `sLightIntensities[chosen] = 0.0f`.
///
/// The walk condition is `(chosen < (sLightCount - 1)) && (r >= rLightIntensity)`
/// -- a **conjunction**, so it stops at the last slot even when `r` still
/// exceeds the running sum, which is what makes a too-large `r` (or an
/// all-zeroed array) select the final slot rather than running off the end.
/// The comparison is `>=`, not `>`: an `r` exactly equal to the running prefix
/// sum advances past that slot.
///
/// # Panics
///
/// Panics if `intensities` is empty. See "Admitted domain" item 8: the source
/// computes `sLightCount - 1` on a `uint`, which would wrap for an empty array,
/// but the enclosing loop makes that unreachable. This port refuses the input
/// rather than reproducing the wrap.
pub fn roulette_select(
    intensities: &mut [f32],
    indices: &[u32],
    r: f32,
    random_range: f32,
) -> RouletteDraw {
    assert!(
        !intensities.is_empty(),
        "roulette_select requires a non-empty candidate set; \
         Lights.hlsli:205's `sLightCount - 1` would wrap on a uint"
    );
    let light_count = intensities.len();

    let mut chosen = 0usize;
    let mut r_light_intensity = intensities[chosen];
    while (chosen < (light_count - 1)) && (r >= r_light_intensity) {
        chosen += 1;
        r_light_intensity += intensities[chosen];
    }

    let chosen_intensity = intensities[chosen];
    let chosen_index = indices[chosen];
    intensities[chosen] = 0.0;
    let remaining_range = random_range - chosen_intensity;

    RouletteDraw {
        chosen,
        chosen_intensity,
        chosen_index,
        remaining_range,
    }
}

/// `float invProbability = useProbability ? (randomRange / cLightIntensity) : 1.0f`
/// (`Lights.hlsli:213`, identical at `:270`).
///
/// `random_range` is the value *before* `:215`'s subtraction, matching the
/// source's ordering: `:213` reads `randomRange`, `:215` decrements it.
///
/// Preserves "Admitted domain" item 9: the division is unguarded. Callers that
/// pass `use_probability == true` with a zero `chosen_intensity` get `Inf` or
/// `NaN`, exactly as the source would.
pub fn inv_probability(use_probability: bool, random_range: f32, chosen_intensity: f32) -> f32 {
    if use_probability {
        random_range / chosen_intensity
    } else {
        1.0
    }
}

/// `clamp(resultShadow, 0.0f, 1.0f)` (`Lights.hlsli:279`), the final clamp
/// `ComputeShadowsRandom` applies and `ComputeLightsRandom` (`:222`) does not.
///
/// See "Admitted domain" item 10. The accumulation this clamps is not ported;
/// only the clamp itself is, so that the asymmetry is recorded in code rather
/// than only in prose.
pub fn clamp_shadow_result(result_shadow: f32) -> f32 {
    hlsl_clamp(result_shadow, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A light with every field neutral, so each test perturbs exactly the
    /// fields it is about.
    fn unit_light() -> PointLight {
        PointLight {
            position: Vec3::new(0.0, 0.0, 0.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
            diffuse_color: Vec3::new(1.0, 1.0, 1.0),
            attenuation_radius: 1.0,
            point_radius: 0.0,
            spot_falloff_cosine: 1.0,
            spot_max_cosine: 1.0,
            specular_color: Vec3::new(0.0, 0.0, 0.0),
            shadow_offset: 0.0,
            attenuation_exponent: 1.0,
            flicker_intensity: 0.0,
            group_bits: 1,
        }
    }

    // ---- constants --------------------------------------------------------

    #[test]
    fn max_lights_is_the_shader_define_twenty_four() {
        assert_eq!(MAX_LIGHTS, 24);
    }

    #[test]
    fn light_array_len_is_max_lights_plus_one_with_a_dead_final_slot() {
        // Lights.hlsli:178-179 sizes the arrays MAX_LIGHTS + 1, but :182 stops
        // accepting at MAX_LIGHTS, so index 24 is never written.
        assert_eq!(LIGHT_ARRAY_LEN, 25);
        assert_eq!(LIGHT_ARRAY_LEN, MAX_LIGHTS as usize + 1);
    }

    #[test]
    fn epsilon_is_one_e_minus_six() {
        assert_eq!(EPSILON, 1e-6f32);
    }

    #[test]
    fn surface_bias_offset_is_truncated_not_rounded() {
        // 1/sqrt(2) = 0.70710678...; the source writes the truncated
        // 0.707106f, not the rounded 0.7071068f.
        assert_eq!(SURFACE_BIAS_DOT_OFFSET, 0.707106f32);
        assert_ne!(SURFACE_BIAS_DOT_OFFSET, 0.7071068f32);
    }

    // ---- intrinsic lowering ----------------------------------------------

    #[test]
    fn hlsl_max_propagates_nan_in_the_first_argument_only() {
        // b > a is false when either is NaN, so the result is always `a`.
        assert!(hlsl_max(f32::NAN, 0.0).is_nan());
        assert_eq!(hlsl_max(0.0, f32::NAN), 0.0);
    }

    #[test]
    fn hlsl_max_differs_from_rust_f32_max_on_nan() {
        // The whole reason hlsl_max exists: f32::max scrubs the NaN.
        assert!(hlsl_max(f32::NAN, 0.0).is_nan());
        assert_eq!(f32::NAN.max(0.0), 0.0);
    }

    #[test]
    fn hlsl_max_returns_first_argument_on_signed_zero_tie() {
        // 0.0 > -0.0 is false, so max(-0.0, 0.0) is -0.0.
        assert!(hlsl_max(-0.0, 0.0).is_sign_negative());
        assert!(hlsl_max(0.0, -0.0).is_sign_positive());
    }

    #[test]
    fn hlsl_min_propagates_nan_in_the_first_argument_only() {
        assert!(hlsl_min(f32::NAN, 1.0).is_nan());
        assert_eq!(hlsl_min(1.0, f32::NAN), 1.0);
    }

    #[test]
    fn hlsl_min_differs_from_rust_f32_min_on_nan() {
        assert!(hlsl_min(f32::NAN, 1.0).is_nan());
        assert_eq!(f32::NAN.min(1.0), 1.0);
    }

    #[test]
    fn hlsl_clamp_is_min_of_max_and_propagates_nan() {
        assert_eq!(hlsl_clamp(0.5, 0.0, 1.0), 0.5);
        assert_eq!(hlsl_clamp(-3.0, 0.0, 1.0), 0.0);
        assert_eq!(hlsl_clamp(3.0, 0.0, 1.0), 1.0);
        // max(NaN, 0) = NaN, then min(NaN, 1) = NaN.
        assert!(hlsl_clamp(f32::NAN, 0.0, 1.0).is_nan());
    }

    #[test]
    fn hlsl_clamp_of_negative_zero_keeps_the_sign() {
        // max(-0.0, 0.0) is -0.0 (0.0 > -0.0 false), min(-0.0, 1.0) is -0.0.
        let clamped = hlsl_clamp(-0.0, 0.0, 1.0);
        assert_eq!(clamped, 0.0);
        assert!(clamped.is_sign_negative());
    }

    #[test]
    fn hlsl_lerp_is_the_a_plus_t_times_b_minus_a_form() {
        // 2 + 0.25*(10-2) = 2 + 2 = 4.
        assert_eq!(hlsl_lerp(2.0, 10.0, 0.25), 4.0);
        // t = 0 returns exactly a.
        assert_eq!(hlsl_lerp(-7.5, 1.0, 0.0), -7.5);
        // t = 1 returns a + (b - a).
        assert_eq!(hlsl_lerp(-7.5, 1.0, 1.0), 1.0);
    }

    #[test]
    fn hlsl_lerp_uses_the_source_form_not_the_precise_form_to_one_ulp() {
        // The two algebraically-equal lerp forms round differently. This pins
        // which one the port uses, because both ported call sites feed the
        // result into further arithmetic where a 1-ulp difference persists.
        //
        // a = 0.16333016753196716 (0x3e274006), t = 0.587384819984436
        // (0x3f165eda), b = 1.0.
        //   b - a       = 0.8366698026657104  (0x3f562ffe)
        //   t * (b - a) = 0.4914471507072449  (0x3efb9ef6)
        //   a + that    = 0.6547772884368896  (0x3f279f7c)  <- source form
        //   a*(1-t)+b*t = 0.6547773480415344  (0x3f279f7d)  <- precise form
        // Exactly one ulp apart.
        let a = f32::from_bits(0x3e27_4006);
        let t = f32::from_bits(0x3f16_5eda);
        let source_form = f32::from_bits(0x3f27_9f7c);
        let precise_form = f32::from_bits(0x3f27_9f7d);
        assert_ne!(source_form, precise_form);
        assert_eq!(hlsl_lerp(a, 1.0, t).to_bits(), source_form.to_bits());
        assert_ne!(hlsl_lerp(a, 1.0, t).to_bits(), precise_form.to_bits());
    }

    #[test]
    fn the_lerp_form_difference_reaches_calculate_light_intensity_simple() {
        // The 1-ulp lerp divergence is not always cancelled downstream. At
        // this input it survives the + bias, the * falloff, and the
        // * channel-sum, coming out one ulp apart at 2.0428249835968018
        // (0x4002bda5) vs 2.042825222015381 (0x4002bda6). So the form choice
        // in hlsl_lerp is observable at this call site, not merely internal.
        //
        // It is NOT observable at every input: the +0.707106 bias and the
        // subsequent multiplies absorb the 1-ulp gap for many (a, t) pairs.
        // This test therefore uses a specific pair where it survives, found by
        // exhaustive search over the two forms rather than assumed.
        let n_dot_l = f32::from_bits(0x3e27_4006);
        let t = f32::from_bits(0x3f16_5eda);
        let via_source = f32::from_bits(0x3f27_9f7c);
        let via_precise = f32::from_bits(0x3f27_9f7d);

        let finish = |lerped: f32| {
            let bias = hlsl_max(lerped + SURFACE_BIAS_DOT_OFFSET, 0.0);
            0.5f32 * bias * 3.0f32
        };
        assert_ne!(finish(via_source), finish(via_precise));

        // And the function really does take the source-form branch: build a
        // light whose NdotL is exactly n_dot_l, then compare the two candidate
        // results against what the function actually returns.
        let mut l = unit_light();
        l.attenuation_radius = 1.0;
        let position = Vec3::new(0.0, 0.0, 0.5);
        // lightDirection = normalize((0,0,-0.5)) = (0,0,-1), so a normal of
        // (0,0,-n_dot_l) gives dot == n_dot_l exactly.
        let normal = Vec3::new(0.0, 0.0, -n_dot_l);
        let got = calculate_light_intensity_simple(&l, position, normal, t);
        // Recompute the tail exactly as the function does, from each candidate
        // lerp result: pow(0.5, 1.0) * bias, then * the channel sum of 3.
        let tail = |lerped: f32| {
            let bias = hlsl_max(lerped + SURFACE_BIAS_DOT_OFFSET, 0.0);
            (0.5f32.powf(1.0) * bias) * Vec3::new(1.0, 1.0, 1.0).dot(Vec3::new(1.0, 1.0, 1.0))
        };
        assert_eq!(got.to_bits(), tail(via_source).to_bits());
        assert_ne!(got.to_bits(), tail(via_precise).to_bits());
    }

    #[test]
    fn hlsl_lerp_extrapolates_above_one_and_below_zero() {
        // Neither ported lerp call site clamps t.
        assert_eq!(hlsl_lerp(0.0, 1.0, 2.0), 2.0);
        assert_eq!(hlsl_lerp(0.0, 1.0, -1.0), -1.0);
    }

    #[test]
    fn hlsl_length_and_normalize_on_a_three_four_five_triangle() {
        let v = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(hlsl_length(v), 5.0);
        let n = hlsl_normalize(v);
        assert_eq!(n, Vec3::new(0.6, 0.8, 0.0));
    }

    #[test]
    fn hlsl_normalize_of_the_zero_vector_is_nan_not_zero() {
        // 0/0 in every lane; the source adds no guard.
        let n = hlsl_normalize(Vec3::new(0.0, 0.0, 0.0));
        assert!(n.x.is_nan() && n.y.is_nan() && n.z.is_nan());
    }

    #[test]
    fn hlsl_cross_matches_the_right_handed_component_order() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(hlsl_cross(x, y), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(hlsl_cross(y, x), Vec3::new(0.0, 0.0, -1.0));
    }

    // ---- CalculateLightIntensitySimple ------------------------------------

    #[test]
    fn light_intensity_at_half_radius_facing_the_light() {
        // position (0,0,1), light at origin, radius 1, exponent 1.
        // Use distance 0.5 so the falloff is exactly 0.5.
        let mut l = unit_light();
        l.attenuation_radius = 1.0;
        let position = Vec3::new(0.0, 0.0, 0.5);
        // lightDirection = normalize((0,0,0)-(0,0,0.5)) = (0,0,-1).
        // normal = (0,0,-1) => NdotL = 1.
        let normal = Vec3::new(0.0, 0.0, -1.0);
        // lerp(1, 1, 0) = 1; surfaceBias = max(1 + 0.707106, 0) = 1.707106.
        // pow(max(1 - 0.5/1, 0), 1) = 0.5; factor = 0.5 * 1.707106 = 0.853553.
        // dot(diffuse, (1,1,1)) = 3 => 0.853553 * 3.
        let expected = 0.5f32 * (1.0f32 + 0.707106f32) * 3.0f32;
        assert_eq!(
            calculate_light_intensity_simple(&l, position, normal, 0.0),
            expected
        );
    }

    #[test]
    fn light_intensity_is_zero_at_and_beyond_the_attenuation_radius() {
        let l = unit_light();
        let normal = Vec3::new(0.0, 0.0, -1.0);
        // distance == radius: 1 - 1/1 = 0, pow(0, 1) = 0.
        assert_eq!(
            calculate_light_intensity_simple(&l, Vec3::new(0.0, 0.0, 1.0), normal, 0.0),
            0.0
        );
        // distance > radius: max(negative, 0) = 0.
        assert_eq!(
            calculate_light_intensity_simple(&l, Vec3::new(0.0, 0.0, 4.0), normal, 0.0),
            0.0
        );
    }

    #[test]
    fn light_intensity_admits_a_back_facing_normal_above_minus_the_bias_offset() {
        // Admitted domain item 2: the +0.707106 bias makes NdotL = -0.5
        // (surface facing 120 degrees away from the light) still contribute.
        let l = unit_light();
        let position = Vec3::new(0.0, 0.0, 0.5);
        // lightDirection = (0,0,-1). Choose normal so dot = -0.5.
        let normal = Vec3::new(0.0, 0.0, 0.5);
        let n_dot_l = -0.5f32;
        let surface_bias = n_dot_l + 0.707106f32;
        assert!(surface_bias > 0.0);
        let expected = 0.5f32 * surface_bias * 3.0f32;
        assert_eq!(
            calculate_light_intensity_simple(&l, position, normal, 0.0),
            expected
        );
    }

    #[test]
    fn light_intensity_clamps_to_zero_below_minus_the_bias_offset() {
        // NdotL = -1 => -1 + 0.707106 < 0 => max(..., 0) = 0.
        let l = unit_light();
        let position = Vec3::new(0.0, 0.0, 0.5);
        let normal = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(
            calculate_light_intensity_simple(&l, position, normal, 0.0),
            0.0
        );
    }

    #[test]
    fn ignore_normal_factor_of_one_erases_the_normal_term_entirely() {
        // lerp(NdotL, 1, 1) = NdotL + 1*(1 - NdotL) = 1 for any NdotL.
        let l = unit_light();
        let position = Vec3::new(0.0, 0.0, 0.5);
        let facing = calculate_light_intensity_simple(&l, position, Vec3::new(0.0, 0.0, -1.0), 1.0);
        let away = calculate_light_intensity_simple(&l, position, Vec3::new(0.0, 0.0, 1.0), 1.0);
        let expected = 0.5f32 * (1.0f32 + 0.707106f32) * 3.0f32;
        assert_eq!(facing, expected);
        assert_eq!(away, expected);
    }

    #[test]
    fn light_intensity_scales_by_the_signed_channel_sum_of_diffuse_color() {
        // Admitted domain item 6: dot(diffuse, (1,1,1)) can cancel.
        let mut l = unit_light();
        l.diffuse_color = Vec3::new(2.0, -1.0, -1.0);
        let position = Vec3::new(0.0, 0.0, 0.5);
        let normal = Vec3::new(0.0, 0.0, -1.0);
        // Channel sum is exactly 0.
        assert_eq!(
            calculate_light_intensity_simple(&l, position, normal, 0.0),
            0.0
        );
    }

    #[test]
    fn light_intensity_goes_negative_when_the_channel_sum_is_negative() {
        let mut l = unit_light();
        l.diffuse_color = Vec3::new(-1.0, 0.0, 0.0);
        let position = Vec3::new(0.0, 0.0, 0.5);
        let normal = Vec3::new(0.0, 0.0, -1.0);
        let expected = 0.5f32 * (1.0f32 + 0.707106f32) * -1.0f32;
        let got = calculate_light_intensity_simple(&l, position, normal, 0.0);
        assert_eq!(got, expected);
        assert!(
            got < 0.0,
            "a negative channel sum yields negative intensity"
        );
        // Which then fails the scan's `> EPSILON` test.
        assert!(!(got > EPSILON));
    }

    #[test]
    fn light_intensity_with_zero_radius_and_nonzero_distance_is_zero() {
        // Admitted domain item 1: d/0 = +Inf, 1-Inf = -Inf, max(-Inf,0) = 0,
        // pow(0, 1) = 0.
        let mut l = unit_light();
        l.attenuation_radius = 0.0;
        let got = calculate_light_intensity_simple(
            &l,
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
        );
        assert_eq!(got, 0.0);
    }

    #[test]
    fn light_intensity_with_zero_radius_and_zero_exponent_is_not_zero() {
        // Same 0.0 base, but pow(0.0, 0.0) is 1.0, so the whole term survives.
        let mut l = unit_light();
        l.attenuation_radius = 0.0;
        l.attenuation_exponent = 0.0;
        let got = calculate_light_intensity_simple(
            &l,
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
        );
        // pow(0,0)=1, NdotL = dot((0,0,-1), (0,0,-1)) = 1, bias = 1.707106.
        let expected = 1.0f32 * (1.0f32 + 0.707106f32) * 3.0f32;
        assert_eq!(got, expected);
    }

    #[test]
    fn light_intensity_at_the_light_position_is_nan_from_the_zero_normalize() {
        // position == lightPosition: normalize((0,0,0)) is NaN in every lane,
        // NdotL is NaN, and max(NaN + 0.707106, 0.0) is NaN in HLSL's
        // NaN-propagating argument position.
        let l = unit_light();
        let got = calculate_light_intensity_simple(
            &l,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
        );
        assert!(got.is_nan(), "expected NaN, got {got}");
    }

    #[test]
    fn light_intensity_uses_an_unclamped_n_dot_l_unlike_the_sample_path() {
        // The contrast that makes this a real difference, not a slip:
        // CalculateLightIntensitySimple's NdotL is raw dot (:48), while
        // sample_weights' is max(dot, 0) (:105).
        let l = unit_light();
        let position = Vec3::new(0.0, 0.0, 0.5);
        let a = calculate_light_intensity_simple(&l, position, Vec3::new(0.0, 0.0, -0.25), 0.0);
        let b = calculate_light_intensity_simple(&l, position, Vec3::new(0.0, 0.0, 0.25), 0.0);
        // dot = +0.25 vs -0.25: different biases, so different results.
        assert_ne!(a, b);
        assert_eq!(a, 0.5f32 * (0.25f32 + 0.707106f32) * 3.0f32);
        assert_eq!(b, 0.5f32 * (-0.25f32 + 0.707106f32) * 3.0f32);
    }

    #[test]
    fn light_intensity_exponent_two_squares_the_falloff() {
        let mut l = unit_light();
        l.attenuation_exponent = 2.0;
        let position = Vec3::new(0.0, 0.0, 0.5);
        let normal = Vec3::new(0.0, 0.0, -1.0);
        let expected = 0.25f32 * (1.0f32 + 0.707106f32) * 3.0f32;
        assert_eq!(
            calculate_light_intensity_simple(&l, position, normal, 0.0),
            expected
        );
    }

    // ---- CalculateShadowIntensitySimple -----------------------------------

    #[test]
    fn shadow_intensity_is_the_bare_falloff_with_no_bias_or_color() {
        let l = unit_light();
        // distance 0.25, radius 1, exponent 1 => 0.75.
        assert_eq!(
            calculate_shadow_intensity_simple(&l, Vec3::new(0.0, 0.0, 0.25)),
            0.75
        );
    }

    #[test]
    fn shadow_intensity_ignores_the_normal_and_diffuse_color_the_light_path_uses() {
        // Same geometry as light_intensity_at_half_radius_facing_the_light,
        // which returns 0.5 * 1.707106 * 3; this returns the bare 0.5.
        let l = unit_light();
        assert_eq!(
            calculate_shadow_intensity_simple(&l, Vec3::new(0.0, 0.0, 0.5)),
            0.5
        );
    }

    #[test]
    fn shadow_intensity_at_the_light_position_is_one_not_nan() {
        // Unlike the light path there is no normalize, so distance 0 gives
        // 1 - 0/1 = 1, pow(1, 1) = 1. No NaN.
        let l = unit_light();
        assert_eq!(
            calculate_shadow_intensity_simple(&l, Vec3::new(0.0, 0.0, 0.0)),
            1.0
        );
    }

    #[test]
    fn shadow_intensity_with_zero_radius_at_zero_distance_is_nan() {
        // Admitted domain item 1's other half: 0.0/0.0 is NaN, and
        // max(1 - NaN, 0.0) is NaN in the propagating position.
        let mut l = unit_light();
        l.attenuation_radius = 0.0;
        let got = calculate_shadow_intensity_simple(&l, Vec3::new(0.0, 0.0, 0.0));
        assert!(got.is_nan(), "expected NaN, got {got}");
    }

    #[test]
    fn shadow_intensity_clamps_to_zero_beyond_the_radius() {
        let l = unit_light();
        assert_eq!(
            calculate_shadow_intensity_simple(&l, Vec3::new(0.0, 0.0, 100.0)),
            0.0
        );
    }

    // ---- perpendicular_basis ----------------------------------------------

    #[test]
    fn perpendicular_basis_for_a_direction_along_plus_x() {
        // lightDirection = (1,0,0); neg = (-1,0,0).
        // perpX = cross((-1,0,0),(0,1,0)) = (0*0-0*1, 0*0-(-1)*0, -1*1-0*0)
        //       = (0, 0, -1).
        // perpY = cross((0,0,-1),(-1,0,0))
        //       = (0*0-(-1)*0, (-1)*(-1)-0*0, 0*0-0*(-1)) = (0, 1, 0).
        let (px, py) = perpendicular_basis(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(px, Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(py, Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn perpendicular_basis_degenerates_when_the_direction_is_plus_y() {
        // neg = (0,-1,0); cross((0,-1,0),(0,1,0)) = (0,0,0), so the :83 fixup
        // fires and sets only .x.
        let (px, py) = perpendicular_basis(Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(px, Vec3::new(1.0, 0.0, 0.0));
        // perpY = cross((1,0,0),(0,-1,0)) = (0*0-0*(-1), 0*0-1*0, 1*(-1)-0*0)
        //       = (0, 0, -1).
        assert_eq!(py, Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn perpendicular_basis_degenerates_when_the_direction_is_minus_y() {
        // neg = (0,1,0); cross((0,1,0),(0,1,0)) = (0,0,0). Fixup fires again.
        let (px, py) = perpendicular_basis(Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(px, Vec3::new(1.0, 0.0, 0.0));
        // perpY = cross((1,0,0),(0,1,0)) = (0, 0, 1).
        assert_eq!(py, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn perpendicular_basis_fixup_treats_negative_zero_as_zero() {
        // Admitted domain item 5: `perpX == 0.0f` is true for -0.0 lanes.
        // direction (0, -0.0, 0) => neg = (0, 0.0, 0)  [negating -0.0]
        // cross((0,0,0),(0,1,0)) has lanes that are 0.0 or -0.0; all compare
        // equal to 0.0, so the fixup fires.
        let (px, _py) = perpendicular_basis(Vec3::new(0.0, -0.0, 0.0));
        assert_eq!(px.x, 1.0);
    }

    #[test]
    fn perpendicular_basis_does_not_fire_the_fixup_for_a_near_plus_y_direction() {
        // A direction merely close to +Y still has a nonzero cross product,
        // so no fixup: the degeneracy is exact, not tolerance-based.
        let (px, _py) = perpendicular_basis(Vec3::new(0.001, 1.0, 0.0));
        assert_ne!(px, Vec3::new(1.0, 0.0, 0.0));
        // neg = (-0.001,-1,0); cross with (0,1,0):
        // x = (-1)*0 - 0*1 = 0; y = 0*0 - (-0.001)*0 = 0; z = -0.001*1 - (-1)*0
        //   = -0.001.
        assert_eq!(px, Vec3::new(0.0, 0.0, -0.001));
    }

    #[test]
    fn perpendicular_basis_axes_are_orthogonal_for_a_generic_direction() {
        let d = hlsl_normalize(Vec3::new(1.0, 2.0, 3.0));
        let (px, py) = perpendicular_basis(d);
        // perpY = cross(perpX, -d) is orthogonal to perpX by construction.
        assert!(px.dot(py).abs() < 1e-6, "perpX . perpY = {}", px.dot(py));
        // perpX = cross(-d, +Y) is orthogonal to d by construction.
        assert!(px.dot(d).abs() < 1e-6, "perpX . d = {}", px.dot(d));
    }

    #[test]
    fn perpendicular_basis_perp_y_is_built_from_the_fixed_up_perp_x() {
        // If perpY used the pre-fixup (0,0,0) perpX it would be (0,0,0).
        let (_px, py) = perpendicular_basis(Vec3::new(0.0, 1.0, 0.0));
        assert_ne!(py, Vec3::new(0.0, 0.0, 0.0));
    }

    // ---- sample_position / light_point_radius / max_samples ---------------

    #[test]
    fn sample_position_offsets_along_both_basis_axes() {
        let lp = Vec3::new(10.0, 20.0, 30.0);
        let px = Vec3::new(1.0, 0.0, 0.0);
        let py = Vec3::new(0.0, 0.0, 1.0);
        // 10 + 1*0.5*2 = 11; 20 + 0 + 0 = 20; 30 + 0 + 1*(-0.25)*2 = 29.5.
        assert_eq!(
            sample_position(lp, px, py, 0.5, -0.25, 2.0),
            Vec3::new(11.0, 20.0, 29.5)
        );
    }

    #[test]
    fn sample_position_with_zero_point_radius_is_the_light_position() {
        let lp = Vec3::new(-3.0, 7.0, 0.5);
        let px = Vec3::new(1.0, 1.0, 1.0);
        let py = Vec3::new(2.0, 2.0, 2.0);
        assert_eq!(sample_position(lp, px, py, 0.9, -0.9, 0.0), lp);
    }

    #[test]
    fn light_point_radius_collapses_to_zero_when_di_samples_is_zero() {
        assert_eq!(light_point_radius(0, 5.0), 0.0);
        assert_eq!(light_point_radius(1, 5.0), 5.0);
        assert_eq!(light_point_radius(64, 5.0), 5.0);
    }

    #[test]
    fn max_samples_floors_at_one_so_the_loop_always_runs_once() {
        // Lights.hlsli:88 max(diSamples, 1). Note this pairs with
        // light_point_radius(0, ..) == 0.0: with diSamples == 0 the source
        // still takes one sample, at the light's exact position.
        assert_eq!(max_samples(0), 1);
        assert_eq!(max_samples(1), 1);
        assert_eq!(max_samples(9), 9);
    }

    // ---- sample_weights ---------------------------------------------------

    #[test]
    fn sample_weights_rejects_a_dot_strictly_above_the_max_cosine() {
        // Admitted domain item 3: the test is `<=`, so a dot above the max
        // cosine is rejected.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            // spotDirection = (0,0,-1); sampleDirection = (0,0,-1) => dot = 1.
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
            0.5, // max cosine below the dot of 1.0
            1.0,
            1.0,
            0.0,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn sample_weights_admits_a_dot_exactly_equal_to_the_max_cosine() {
        // The boundary is inclusive (`<=`, not `<`).
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
            1.0, // exactly the dot
            1.0,
            1.0,
            0.0,
        );
        assert!(got.is_some());
        assert_eq!(got.unwrap().light_spot_dot, 1.0);
    }

    #[test]
    fn sample_weights_admits_a_direction_pointing_away_from_the_spot_axis() {
        // Item 3 read the other way: a dot of -1, maximally *misaligned* with
        // the spot direction, passes the test that a perfectly aligned sample
        // fails. This is the inverted-cone behavior, pinned.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            // spotDirection = +Z, sampleDirection = -Z => dot = -1.
            Vec3::new(0.0, 0.0, 1.0),
            0.0,
            0.5,
            1.0,
            1.0,
            0.0,
        );
        assert!(got.is_some());
        assert_eq!(got.unwrap().light_spot_dot, -1.0);
    }

    #[test]
    fn sample_weights_rejects_a_nan_dot_because_the_comparison_is_false() {
        // `NaN <= x` is false, so the guard's negation admits nothing.
        // A NaN sample position gives a NaN direction and a NaN dot.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(f32::NAN, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
            1.0,
            1.0,
            1.0,
            0.0,
        );
        assert_eq!(got, None);
    }

    #[test]
    fn sample_weights_spot_intensity_is_one_at_the_falloff_cosine() {
        // dot == falloffCosine => numerator 0 => clamp(0,...) = 0 =>
        // spotIntensity = 1 - 0 = 1.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            1.0, // falloff == the dot of 1.0
            1.5, // max above it, so the guard passes
            1.0,
            1.0,
            0.0,
        )
        .expect("spot test admits dot 1.0 <= 1.5");
        assert_eq!(got.spot_intensity, 1.0);
    }

    #[test]
    fn sample_weights_spot_intensity_is_zero_at_the_max_cosine() {
        // dot == maxCosine: (max - falloff)/(max - falloff) = 1,
        // clamp(1,0,1) = 1, spotIntensity = 0.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            0.0,
            1.0,
            1.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 1.0");
        assert_eq!(got.spot_intensity, 0.0);
        // And a zero spot intensity zeroes the whole sample weight.
        assert_eq!(got.sample_intensity_factor, 0.0);
    }

    #[test]
    fn sample_weights_spot_intensity_interpolates_at_the_midpoint() {
        // falloff 0, max 1, dot 0.5 => 0.5/1 = 0.5 => intensity 0.5.
        let got = sample_weights(
            // Place the sample so sampleDirection dots the spot axis at 0.5.
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.6, 0.0, 0.8),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            0.0,
            0.9,
            1.0,
            1.0,
            0.0,
        )
        .expect("dot 0.8 <= 0.9");
        // sampleDirection = normalize((0.6,0,0.8)) = (0.6,0,0.8), dot = 0.8.
        assert_eq!(got.light_spot_dot, 0.8);
        // (0.8 - 0)/(0.9 - 0) = 0.8888889; 1 - that.
        let expected = 1.0f32 - (0.8f32 / 0.9f32);
        assert_eq!(got.spot_intensity, expected);
    }

    #[test]
    fn sample_weights_spot_intensity_is_one_when_the_denominator_is_zero_and_the_dot_is_below() {
        // Admitted domain item 4. dot < falloff == max: numerator negative,
        // denominator 0 => -Inf; clamp(-Inf, 0, 1) = 0; 1 - 0 = 1.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            // spot axis -Z, sampleDirection +Z => dot = -1.
            Vec3::new(0.0, 0.0, -1.0),
            0.5,
            0.5,
            1.0,
            1.0,
            0.0,
        )
        .expect("dot -1.0 <= max 0.5");
        assert_eq!(got.spot_intensity, 1.0);
    }

    #[test]
    fn sample_weights_spot_intensity_is_nan_when_the_dot_equals_a_zero_width_cone() {
        // Item 4's NaN branch: dot == falloff == max => 0.0/0.0 => NaN,
        // clamp propagates it, 1 - NaN = NaN.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            1.0,
            1.0,
            1.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 1.0");
        assert!(
            got.spot_intensity.is_nan(),
            "expected NaN, got {}",
            got.spot_intensity
        );
        assert!(got.sample_intensity_factor.is_nan());
    }

    #[test]
    fn sample_weights_attenuation_matches_the_hand_computed_falloff() {
        // position origin, sample at (0,0,2), radius 4, exponent 1.
        // sampleDistance = 2 => 1 - 2/4 = 0.5.
        // spot: axis +Z, dot = 1; falloff 0, max 1 => clamp(1,0,1)=1 =>
        // spotIntensity = 0. Use falloff 2 instead to keep it nonzero:
        // (1 - 2)/(1 - 2) = 1 => clamp 1 => spotIntensity 0. Use max 3:
        // (1 - 2)/(3 - 2) = -1 => clamp 0 => spotIntensity 1.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            4.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.spot_intensity, 1.0);
        assert_eq!(got.sample_distance, 2.0);
        assert_eq!(got.sample_intensity_factor, 0.5);
    }

    #[test]
    fn sample_weights_lambert_clamps_n_dot_l_at_zero() {
        // Item: :105's NdotL is max(dot, 0). A normal facing away gives 0, so
        // with ignoreNormalFactor 0 the Lambert factor is exactly 0 -- unlike
        // CalculateLightIntensitySimple, which would still bias it positive.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            // normal points -Z, sampleDirection is +Z => dot = -1 => max = 0.
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            4.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.sample_lambert_factor, 0.0);
        // The intensity factor is unaffected by the normal.
        assert_eq!(got.sample_intensity_factor, 0.5);
    }

    #[test]
    fn sample_weights_lambert_with_ignore_normal_one_equals_the_intensity_factor() {
        // lerp(NdotL, 1, 1) = 1, so lambert == sampleIntensityFactor even for
        // a fully back-facing normal.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            4.0,
            1.0,
            1.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.sample_lambert_factor, got.sample_intensity_factor);
        assert_eq!(got.sample_lambert_factor, 0.5);
    }

    #[test]
    fn sample_weights_lambert_is_n_dot_l_times_the_intensity_factor() {
        // Facing normal: NdotL = 1, lerp(1,1,0) = 1 => lambert == factor.
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 2.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            4.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.sample_lambert_factor, 0.5);
    }

    #[test]
    fn sample_weights_beyond_the_radius_zeroes_both_factors() {
        let got = sample_weights(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            4.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.sample_intensity_factor, 0.0);
        assert_eq!(got.sample_lambert_factor, 0.0);
    }

    #[test]
    fn sample_weights_reports_the_direction_it_normalized() {
        let got = sample_weights(
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(1.0, 2.0, 8.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            2.0,
            3.0,
            10.0,
            1.0,
            0.0,
        )
        .expect("dot 1.0 <= max 3.0");
        assert_eq!(got.sample_direction, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(got.sample_distance, 5.0);
    }

    // ---- scan_candidate_lights --------------------------------------------

    #[test]
    fn scan_keeps_only_lights_whose_group_bits_intersect_the_mask() {
        let groups = [0b0001u32, 0b0010, 0b0100, 0b0011];
        let got = scan_candidate_lights(4, 0b0001, |i| groups[i as usize], |_| 1.0);
        // Indices 0 and 3 both have bit 0 set.
        assert_eq!(got.indices, vec![0, 3]);
        assert_eq!(got.intensities, vec![1.0, 1.0]);
        assert_eq!(got.total_intensity, 2.0);
    }

    #[test]
    fn scan_rejects_intensity_exactly_at_epsilon_because_the_test_is_strict() {
        let got = scan_candidate_lights(1, 0xFFFF_FFFF, |_| 1, |_| EPSILON);
        assert!(got.indices.is_empty());
        assert_eq!(got.total_intensity, 0.0);
    }

    #[test]
    fn scan_accepts_intensity_just_above_epsilon() {
        let just_above = EPSILON * 1.000001;
        assert!(just_above > EPSILON);
        let got = scan_candidate_lights(1, 0xFFFF_FFFF, |_| 1, move |_| just_above);
        assert_eq!(got.indices, vec![0]);
    }

    #[test]
    fn scan_rejects_nan_intensity_because_the_comparison_is_false() {
        let got = scan_candidate_lights(1, 0xFFFF_FFFF, |_| 1, |_| f32::NAN);
        assert!(got.indices.is_empty());
        assert_eq!(got.total_intensity, 0.0);
    }

    #[test]
    fn scan_rejects_negative_intensity() {
        let got = scan_candidate_lights(1, 0xFFFF_FFFF, |_| 1, |_| -5.0);
        assert!(got.indices.is_empty());
    }

    #[test]
    fn scan_stops_accepting_at_max_lights() {
        // Admitted domain item 7: 30 all-passing lights yield exactly 24.
        let got = scan_candidate_lights(30, 0xFFFF_FFFF, |_| 1, |_| 1.0);
        assert_eq!(got.indices.len(), MAX_LIGHTS as usize);
        assert_eq!(got.indices.last(), Some(&23));
        assert_eq!(got.total_intensity, 24.0);
    }

    #[test]
    fn scan_never_writes_the_dead_final_array_slot() {
        // The arrays are MAX_LIGHTS + 1 long but at most MAX_LIGHTS are used.
        let got = scan_candidate_lights(100, 0xFFFF_FFFF, |_| 1, |_| 1.0);
        assert!(got.indices.len() < LIGHT_ARRAY_LEN);
        assert_eq!(got.indices.len(), LIGHT_ARRAY_LEN - 1);
    }

    #[test]
    fn scan_bound_counts_accepted_lights_not_examined_lights() {
        // Item 7's second half: 100 lights of which every other one passes the
        // mask still accepts 24, drawn from indices 0,2,4,...,46.
        let got = scan_candidate_lights(100, 0b1, |i| if i % 2 == 0 { 1 } else { 0 }, |_| 1.0);
        assert_eq!(got.indices.len(), 24);
        assert_eq!(got.indices[0], 0);
        assert_eq!(got.indices[23], 46);
    }

    #[test]
    fn scan_walks_every_light_when_none_are_accepted() {
        // The early exit is on sLightCount, so a fully-rejecting mask means
        // the loop runs to pointLightsCount. Count the calls to prove it.
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        let got = scan_candidate_lights(
            50,
            0b1,
            |_| {
                calls.set(calls.get() + 1);
                0
            },
            |_| 1.0,
        );
        assert!(got.indices.is_empty());
        assert_eq!(calls.get(), 50);
    }

    #[test]
    fn scan_with_a_zero_mask_accepts_nothing() {
        let got = scan_candidate_lights(10, 0, |_| 0xFFFF_FFFF, |_| 1.0);
        assert!(got.indices.is_empty());
        assert_eq!(got.total_intensity, 0.0);
    }

    #[test]
    fn scan_totals_in_scan_order_so_rounding_is_order_dependent() {
        // 1.0 + 1e-8 rounds back to 1.0 in f32; adding the small term first
        // would give a different (still 1.0 here) prefix, but the point is the
        // accumulator is a running left-to-right sum, not a reassociated one.
        let vals = [1.0f32, 1e-5, 1e-5];
        let got = scan_candidate_lights(3, 0b1, |_| 1, |i| vals[i as usize]);
        let expected = (1.0f32 + 1e-5f32) + 1e-5f32;
        assert_eq!(got.total_intensity, expected);
    }

    #[test]
    fn scan_preserves_the_parallel_index_and_intensity_arrays() {
        let groups = [1u32, 0, 1, 0, 1];
        let vals = [3.0f32, 99.0, 5.0, 99.0, 7.0];
        let got = scan_candidate_lights(5, 1, |i| groups[i as usize], |i| vals[i as usize]);
        assert_eq!(got.indices, vec![0, 2, 4]);
        assert_eq!(got.intensities, vec![3.0, 5.0, 7.0]);
        assert_eq!(got.total_intensity, 15.0);
    }

    #[test]
    fn scan_of_zero_lights_is_empty() {
        let got = scan_candidate_lights(0, 0xFFFF_FFFF, |_| 1, |_| 1.0);
        assert!(got.indices.is_empty());
        assert_eq!(got.total_intensity, 0.0);
    }

    // ---- selected_light_count / use_probability ---------------------------

    #[test]
    fn selected_light_count_is_the_min_of_candidates_and_the_cap() {
        assert_eq!(selected_light_count(5, 2), 2);
        assert_eq!(selected_light_count(2, 5), 2);
        assert_eq!(selected_light_count(3, 3), 3);
        assert_eq!(selected_light_count(0, 4), 0);
    }

    #[test]
    fn use_probability_is_true_only_for_exactly_one_selected_light() {
        assert!(use_probability(1));
        assert!(!use_probability(0));
        assert!(!use_probability(2));
        assert!(!use_probability(24));
    }

    #[test]
    fn use_probability_being_single_draw_only_is_what_makes_the_divide_safe() {
        // Admitted domain item 9: with lLightCount == 1 exactly one draw runs,
        // so the chosen intensity is still its original > EPSILON value and
        // never the zeroed 0.0 a later draw would see.
        let mut intensities = vec![2.0f32, 3.0];
        let indices = vec![0u32, 1];
        let draw = roulette_select(&mut intensities, &indices, 0.0, 5.0);
        assert_eq!(draw.chosen_intensity, 2.0);
        assert_eq!(
            inv_probability(use_probability(1), 5.0, draw.chosen_intensity),
            2.5
        );
    }

    // ---- roulette_select --------------------------------------------------

    #[test]
    fn roulette_r_below_the_first_intensity_selects_slot_zero() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 1.5, 10.0);
        assert_eq!(draw.chosen, 0);
        assert_eq!(draw.chosen_index, 7);
        assert_eq!(draw.chosen_intensity, 2.0);
        assert_eq!(draw.remaining_range, 8.0);
        // The chosen slot is zeroed in place.
        assert_eq!(intensities, vec![0.0, 3.0, 5.0]);
    }

    #[test]
    fn roulette_r_exactly_at_a_prefix_sum_advances_past_that_slot() {
        // The condition is `r >= rLightIntensity`, not `>`.
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 2.0, 10.0);
        assert_eq!(draw.chosen, 1);
        assert_eq!(draw.chosen_index, 8);
        assert_eq!(draw.chosen_intensity, 3.0);
    }

    #[test]
    fn roulette_r_just_below_a_prefix_sum_stays_on_that_slot() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 1.9999999, 10.0);
        assert_eq!(draw.chosen, 0);
    }

    #[test]
    fn roulette_walks_to_the_middle_slot_on_a_mid_range_r() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        // prefix sums: 2, 5, 10. r = 4 => 4 >= 2 advance to 1; 4 >= 5 false.
        let draw = roulette_select(&mut intensities, &indices, 4.0, 10.0);
        assert_eq!(draw.chosen, 1);
        assert_eq!(draw.chosen_intensity, 3.0);
        assert_eq!(draw.remaining_range, 7.0);
    }

    #[test]
    fn roulette_selects_the_last_slot_for_an_r_at_the_total() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        // r = 10: 10 >= 2 -> 1; 10 >= 5 -> 2; chosen == count-1 stops.
        let draw = roulette_select(&mut intensities, &indices, 10.0, 10.0);
        assert_eq!(draw.chosen, 2);
        assert_eq!(draw.chosen_index, 9);
        assert_eq!(draw.remaining_range, 5.0);
    }

    #[test]
    fn roulette_clamps_to_the_last_slot_for_an_r_far_beyond_the_total() {
        // The `chosen < count - 1` conjunct is what prevents an overrun; an
        // enormous r cannot walk past the end.
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 1.0e9, 10.0);
        assert_eq!(draw.chosen, 2);
    }

    #[test]
    fn roulette_with_a_nan_r_stays_on_slot_zero() {
        // `NaN >= x` is false, so the walk never advances.
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, f32::NAN, 10.0);
        assert_eq!(draw.chosen, 0);
    }

    #[test]
    fn roulette_with_a_negative_r_stays_on_slot_zero() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, -1.0, 10.0);
        assert_eq!(draw.chosen, 0);
    }

    #[test]
    fn roulette_on_a_single_candidate_always_picks_it() {
        // count - 1 == 0, so the `chosen < 0` conjunct is false immediately
        // and no comparison against r is ever made.
        let mut intensities = vec![4.0f32];
        let indices = vec![11u32];
        for r in [-1.0f32, 0.0, 3.9, 4.0, 1.0e9, f32::NAN] {
            let mut copy = intensities.clone();
            let draw = roulette_select(&mut copy, &indices, r, 4.0);
            assert_eq!(draw.chosen, 0, "r = {r}");
            assert_eq!(draw.chosen_index, 11);
        }
        // And it zeroes the slot.
        let draw = roulette_select(&mut intensities, &indices, 0.0, 4.0);
        assert_eq!(draw.chosen_intensity, 4.0);
        assert_eq!(intensities, vec![0.0]);
    }

    #[test]
    fn roulette_without_replacement_skips_a_previously_zeroed_slot() {
        // Two successive draws: the first zeroes slot 0, so the second's
        // prefix sum starts at 0 and any r >= 0 advances.
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let first = roulette_select(&mut intensities, &indices, 0.0, 10.0);
        assert_eq!(first.chosen, 0);
        assert_eq!(intensities, vec![0.0, 3.0, 5.0]);
        // Second draw, range now 8. r = 0: 0 >= 0 (the zeroed slot) advances
        // to 1; 0 >= 3 is false, so slot 1.
        let second = roulette_select(&mut intensities, &indices, 0.0, first.remaining_range);
        assert_eq!(second.chosen, 1);
        assert_eq!(second.chosen_intensity, 3.0);
        assert_eq!(second.remaining_range, 5.0);
    }

    #[test]
    fn roulette_on_an_all_zero_array_selects_the_last_slot_with_zero_intensity() {
        // Every candidate exhausted: 0 >= 0 advances at every step until the
        // `chosen < count - 1` conjunct stops it. This is the state that would
        // make an unguarded invProbability divide by zero -- unreachable in
        // the source, reachable here only because this port exposes the step.
        let mut intensities = vec![0.0f32, 0.0, 0.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 0.0, 0.0);
        assert_eq!(draw.chosen, 2);
        assert_eq!(draw.chosen_intensity, 0.0);
    }

    #[test]
    #[should_panic(expected = "non-empty candidate set")]
    fn roulette_refuses_an_empty_candidate_set_rather_than_wrapping() {
        // Admitted domain item 8: the source's `sLightCount - 1` is uint
        // arithmetic that would wrap to 0xFFFFFFFF. This port refuses instead.
        let mut intensities: Vec<f32> = Vec::new();
        let indices: Vec<u32> = Vec::new();
        let _ = roulette_select(&mut intensities, &indices, 0.0, 0.0);
    }

    #[test]
    fn roulette_remaining_range_subtracts_the_chosen_intensity() {
        let mut intensities = vec![2.0f32, 3.0, 5.0];
        let indices = vec![7u32, 8, 9];
        let draw = roulette_select(&mut intensities, &indices, 4.0, 10.0);
        assert_eq!(draw.remaining_range, 10.0 - 3.0);
    }

    #[test]
    fn roulette_accumulates_the_prefix_sum_left_to_right_and_absorbs_tiny_terms() {
        // The walk adds intensities[chosen] one at a time, so a term below
        // half the accumulator's ulp vanishes and the walk keeps advancing.
        // f32's ulp at 1.0 is 2^-23 = 1.1920929e-7, so 1e-8 is far below the
        // 5.96e-8 rounding threshold and 1.0 + 1e-8 == 1.0 exactly.
        assert_eq!(1.0f32 + 1e-8f32, 1.0f32);
        let mut intensities = vec![1.0f32, 1e-8, 1e-8, 100.0];
        let indices = vec![0u32, 1, 2, 3];
        // prefix sums: 1.0, then 1.0, then 1.0, then 101.0.
        // r = 1.0: 1.0 >= 1.0 -> slot 1; 1.0 >= 1.0 -> slot 2; 1.0 >= 1.0 ->
        // slot 3; chosen == count - 1 stops the walk.
        let draw = roulette_select(&mut intensities, &indices, 1.0, 101.0);
        assert_eq!(draw.chosen, 3);
        assert_eq!(draw.chosen_intensity, 100.0);
    }

    #[test]
    fn roulette_prefix_sum_keeps_a_term_at_or_above_half_an_ulp() {
        // The contrast case: 1e-7 is above the 5.96e-8 threshold, so
        // 1.0 + 1e-7 rounds *up* and the walk stops at slot 1 rather than
        // running on. Same shape as the test above, one exponent apart.
        assert_ne!(1.0f32 + 1e-7f32, 1.0f32);
        let mut intensities = vec![1.0f32, 1e-7, 1e-7, 100.0];
        let indices = vec![0u32, 1, 2, 3];
        let draw = roulette_select(&mut intensities, &indices, 1.0, 101.0);
        assert_eq!(draw.chosen, 1);
        assert_eq!(draw.chosen_intensity, 1e-7);
    }

    // ---- inv_probability / clamp_shadow_result ----------------------------

    #[test]
    fn inv_probability_is_one_when_probability_is_disabled() {
        assert_eq!(inv_probability(false, 10.0, 2.0), 1.0);
        // Even a zero intensity cannot divide when disabled.
        assert_eq!(inv_probability(false, 10.0, 0.0), 1.0);
    }

    #[test]
    fn inv_probability_is_the_range_over_the_chosen_intensity() {
        assert_eq!(inv_probability(true, 10.0, 2.0), 5.0);
        assert_eq!(inv_probability(true, 3.0, 3.0), 1.0);
    }

    #[test]
    fn inv_probability_divides_by_zero_unguarded_when_enabled() {
        // Admitted domain item 9: no guard is added.
        assert_eq!(inv_probability(true, 10.0, 0.0), f32::INFINITY);
        assert!(inv_probability(true, 0.0, 0.0).is_nan());
    }

    #[test]
    fn clamp_shadow_result_bounds_to_zero_one() {
        assert_eq!(clamp_shadow_result(-2.0), 0.0);
        assert_eq!(clamp_shadow_result(0.25), 0.25);
        assert_eq!(clamp_shadow_result(7.0), 1.0);
    }

    #[test]
    fn clamp_shadow_result_propagates_nan_under_the_hlsl_lowering() {
        // Admitted domain item 10 / the clamp lowering: max(NaN,0) = NaN,
        // min(NaN,1) = NaN. A NaN shadow accumulation is NOT scrubbed to 0.
        assert!(clamp_shadow_result(f32::NAN).is_nan());
        // Contrast with Rust's clamp, which would panic or scrub.
        assert_eq!(f32::NAN.max(0.0).min(1.0), 0.0);
    }

    // ---- cross-function composition ---------------------------------------

    #[test]
    fn a_scan_then_select_then_draw_round_trip_holds_together() {
        // Wire the ported pieces the way ComputeLightsRandom's :182-215 does,
        // with the sampler-derived r supplied explicitly.
        let lights: Vec<PointLight> = (0..3)
            .map(|i| {
                let mut l = unit_light();
                l.attenuation_radius = 10.0;
                l.position = Vec3::new(0.0, 0.0, i as f32);
                l
            })
            .collect();
        let position = Vec3::new(0.0, 0.0, 5.0);
        let normal = Vec3::new(0.0, 0.0, -1.0);

        let mut candidates = scan_candidate_lights(
            3,
            1,
            |i| lights[i as usize].group_bits,
            |i| calculate_light_intensity_simple(&lights[i as usize], position, normal, 0.0),
        );
        assert_eq!(candidates.indices.len(), 3);

        let selected = selected_light_count(candidates.indices.len() as u32, 1);
        assert_eq!(selected, 1);
        assert!(use_probability(selected));

        let range = candidates.total_intensity;
        let draw = roulette_select(&mut candidates.intensities, &candidates.indices, 0.0, range);
        assert_eq!(draw.chosen, 0);
        let inv = inv_probability(use_probability(selected), range, draw.chosen_intensity);
        assert_eq!(inv, range / draw.chosen_intensity);
        assert!(inv >= 1.0, "inverse probability is never below one here");
    }

    #[test]
    fn a_basis_then_sample_then_weight_round_trip_holds_together() {
        // ComputeLight's :74-106, minus the blue-noise load and the trace.
        let light_position = Vec3::new(0.0, 0.0, 0.0);
        let position = Vec3::new(0.0, 0.0, 4.0);
        let light_direction = hlsl_normalize(light_position.sub(position));
        assert_eq!(light_direction, Vec3::new(0.0, 0.0, -1.0));

        let (px, py) = perpendicular_basis(light_direction);
        // neg = (0,0,1); cross((0,0,1),(0,1,0)) = (0*0-1*1, 1*0-0*0, 0) =
        // (-1, 0, 0). Not all-zero, so no fixup.
        assert_eq!(px, Vec3::new(-1.0, 0.0, 0.0));
        // cross((-1,0,0),(0,0,1)) = (0*1-0*0, 0*0-(-1)*1, 0) = (0, 1, 0).
        assert_eq!(py, Vec3::new(0.0, 1.0, 0.0));

        let radius = light_point_radius(4, 2.0);
        assert_eq!(radius, 2.0);
        // Coordinate (1, 0) puts the sample at (0,0,0) + (-1,0,0)*1*2 =
        // (-2, 0, 0).
        let sp = sample_position(light_position, px, py, 1.0, 0.0, radius);
        assert_eq!(sp, Vec3::new(-2.0, 0.0, 0.0));

        // Spot axis chosen so the guard passes: sampleDirection is
        // normalize((-2,0,-4)); dot with (0,0,1) is negative.
        let w = sample_weights(
            position,
            sp,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            -1.0,
            0.0,
            10.0,
            1.0,
            0.0,
        )
        .expect("a negative dot is <= a max cosine of 0.0");
        // |(-2,0,-4)| = sqrt(20).
        assert_eq!(w.sample_distance, 20.0f32.sqrt());
    }

    #[test]
    fn the_shadow_scan_and_the_light_scan_differ_only_in_the_intensity_function() {
        // :184 vs :241 is the sole difference between the two scans, and it is
        // observable: the shadow intensity omits the surface bias and the
        // diffuse-color scale, so the same light ranks differently.
        let mut l = unit_light();
        l.attenuation_radius = 10.0;
        let position = Vec3::new(0.0, 0.0, 5.0);
        let normal = Vec3::new(0.0, 0.0, -1.0);

        let light_scan = scan_candidate_lights(
            1,
            1,
            |_| 1,
            |_| calculate_light_intensity_simple(&l, position, normal, 0.0),
        );
        let shadow_scan = scan_candidate_lights(
            1,
            1,
            |_| 1,
            |_| calculate_shadow_intensity_simple(&l, position),
        );

        assert_eq!(light_scan.indices, vec![0]);
        assert_eq!(shadow_scan.indices, vec![0]);
        // 1 - 5/10 = 0.5 for both, then the light path multiplies by
        // 1.707106 * 3.
        assert_eq!(shadow_scan.total_intensity, 0.5);
        assert_eq!(
            light_scan.total_intensity,
            0.5f32 * (1.0f32 + 0.707106f32) * 3.0f32
        );
        assert!(light_scan.total_intensity > shadow_scan.total_intensity);
    }
}
