//! Literal port of RT64's `BoxFilterCS.hlsl` and `BicubicScalingCS.hlsl`
//! resampling arithmetic -- the permitted MIT RT64 Rust-port source pinned
//! at commit `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`).
//!
//! `src/shaders/BoxFilterCS.hlsl` (SHA-256 of the whole file, 29 lines,
//! `6983fdb48c3331d19041dd309d5f0e870b36c2e3c62eab72a305f8604b04f560`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//!
//! #define BLOCK_SIZE 8
//!
//! struct BoxFilterCB {
//!     int2 Resolution;
//!     int2 ResolutionScale;
//!     int2 Misalignment;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<BoxFilterCB> gConstants : register(b0);
//! Texture2D<float4> gInput : register(t1);
//! RWTexture2D<float4> gOutput : register(u2);
//!
//! [numthreads(BLOCK_SIZE, BLOCK_SIZE, 1)]
//! void CSMain(uint2 coord : SV_DispatchThreadID) {
//!     float4 resultColor = 0.0f;
//!     int2 maxCoord = gConstants.Resolution - int2(1, 1);
//!     for (int x = 0; x < gConstants.ResolutionScale.x; x++) {
//!         for (int y = 0; y < gConstants.ResolutionScale.y; y++) {
//!             int2 clampedCoord = clamp(coord * gConstants.ResolutionScale + int2(x, y) + gConstants.Misalignment, int2(0, 0), maxCoord);
//!             resultColor += gInput.Load(int3(clampedCoord, 0));
//!         }
//!     }
//!
//!     gOutput[coord] = resultColor / (gConstants.ResolutionScale.x * gConstants.ResolutionScale.y);
//! }
//! ```
//!
//! `src/shaders/BicubicScalingCS.hlsl` (SHA-256 of the whole file, 58 lines,
//! `58f35aa68c30042b31e530c6770b69309ad3e297e142d9cee0d13762b50a4ce4`,
//! cross-checked against `docs/rt64-port-inventory.json`'s
//! `sources.port.sha256` for that path):
//!
//! ```text
//! //
//! // RT64
//! //
//! // Bicubic upscaling, adapted from https://www.shadertoy.com/view/XtKfRV
//! //
//!
//! #include "Color.hlsli"
//!
//! #define BLOCK_SIZE 8
//!
//! struct BicubicCB {
//!     uint2 InputResolution;
//!     uint2 OutputResolution;
//! };
//!
//! [[vk::push_constant]] ConstantBuffer<BicubicCB> gConstants : register(b0);
//! Texture2D<float4> gInput : register(t1);
//! RWTexture2D<float4> gOutput : register(u2);
//! SamplerState gSampler : register(s3);
//!
//! float4 cubic(float x) {
//!     float x2 = x * x;
//!     float x3 = x2 * x;
//!     float4 w;
//!     w.x = -x3 + 3.0 * x2 - 3.0 * x + 1.0;
//!     w.y = 3.0 * x3 - 6.0 * x2 + 4.0;
//!     w.z = -3.0 * x3 + 3.0 * x2 + 3.0 * x + 1.0;
//!     w.w = x3;
//!     return w / 6.0;
//! }
//!
//! float4 BicubicFilter(Texture2D<float4> gInput, SamplerState gSampler, float2 uv, uint2 outputResolution) {
//!     float fx = -0.5;
//!     float fy = -0.5;
//!     float4 xcubic = cubic(fx);
//!     float4 ycubic = cubic(fy);
//!     float2 coord = uv * outputResolution;
//!
//!     float4 c = float4(coord.x - 0.5, coord.x + 1.5, coord.y - 0.5, coord.y + 1.5);
//!     float4 s = float4(xcubic.x + xcubic.y, xcubic.z + xcubic.w, ycubic.x + ycubic.y, ycubic.z + ycubic.w);
//!     float4 offset = c + float4(xcubic.y, xcubic.w, ycubic.y, ycubic.w) / s;
//!
//!     float4 sample0 = gInput.SampleLevel(gSampler, float2(offset.x, offset.z) / outputResolution, 0);
//!     float4 sample1 = gInput.SampleLevel(gSampler, float2(offset.y, offset.z) / outputResolution, 0);
//!     float4 sample2 = gInput.SampleLevel(gSampler, float2(offset.x, offset.w) / outputResolution, 0);
//!     float4 sample3 = gInput.SampleLevel(gSampler, float2(offset.y, offset.w) / outputResolution, 0);
//!
//!     float sx = s.x / (s.x + s.y);
//!     float sy = s.z / (s.z + s.w);
//!     return lerp(lerp(sample3, sample2, sx), lerp(sample1, sample0, sx), sy);
//! }
//!
//! [numthreads(BLOCK_SIZE, BLOCK_SIZE, 1)]
//! void CSMain(uint2 coord : SV_DispatchThreadID) {
//!     if (coord.x < gConstants.OutputResolution.x && coord.y < gConstants.OutputResolution.y) {
//!         gOutput[coord] = BicubicFilter(gInput, gSampler, float2(coord) / float2(gConstants.OutputResolution), gConstants.OutputResolution);
//!     }
//! }
//! ```
//!
//! `BicubicScalingCS.hlsl` line 7 `#include`s `Color.hlsli`, cited here per
//! the ticket's pinning requirement, but -- matching
//! `rt64_fullscreen_vs.rs`'s `Constants.hlsli` precedent -- no symbol from
//! `Color.hlsli` (`HUEtoRGB`, `RGBtoHCV`, `HSLtoRGB`, `RGBtoHSL`,
//! `ModRGBWithHSL`, `RGBtoLuminance`, `LinearToSrgb`, ...) is referenced by
//! `cubic`, `BicubicFilter`, or `CSMain`; none of it is admitted, re-expressed,
//! or given a digest below.
//!
//! **Reuse, not new type.** [`BoxFilterParams`]/[`BicubicFilterParams`] are
//! the owned parameter carriers for each shader's constant-buffer inputs;
//! [`box_filter_tap`]/[`cubic`]/[`bicubic_filter`] and the WGSL siblings
//! ([`BOX_FILTER_WGSL`]/[`BICUBIC_SCALING_WGSL`]) both derive from the same
//! ported formulas, and the characterization tests below compare the Rust
//! port against an independently-written second oracle rather than one
//! implementation against itself.
//!
//! ## Ported vs. skipped
//!
//! Ported (the resampling arithmetic):
//! - `BoxFilterCS.hlsl:19-28`: the `resultColor` zero-init, the
//!   `maxCoord = Resolution - (1,1)` bound, the nested `x`/`y` tap loop's
//!   `clampedCoord = clamp(coord*ResolutionScale + (x,y) + Misalignment, 0,
//!   maxCoord)` addressing, the `resultColor += <tap>` accumulation, and the
//!   final `resultColor / (ResolutionScale.x * ResolutionScale.y)` average.
//! - `BicubicScalingCS.hlsl:21-30` (`cubic`): the full `x2`/`x3`/`w`
//!   polynomial and the `/6.0` normalization.
//! - `BicubicScalingCS.hlsl:32-51` (`BicubicFilter`): the fixed `fx=fy=-0.5`
//!   evaluation of `cubic`, the `coord = uv * outputResolution` scale, the
//!   `c`/`s`/`offset` derivation, the four sample-UV computations (the
//!   `/ outputResolution` texel-to-UV re-normalization; the actual texture
//!   fetch itself is not ported -- see below), `sx`/`sy`, and the nested
//!   `lerp(lerp(...), lerp(...), sy)` blend.
//! - `BicubicScalingCS.hlsl:56`'s `float2(coord) / float2(gConstants.
//!   OutputResolution)` UV construction (ported as
//!   [`bicubic_filter_at_coord`]), evaluated unconditionally -- i.e. with
//!   `CSMain`'s own dispatch guard around it removed, not reproduced.
//!
//! Skipped (compute-dispatch scaffolding and GPU resource binds, per the
//! ticket's scope):
//! - `[numthreads(BLOCK_SIZE, BLOCK_SIZE, 1)]`, `SV_DispatchThreadID`.
//! - `ConstantBuffer<...>`/`[[vk::push_constant]]`/`register(...)` binds for
//!   `gConstants`, `gInput`, `gOutput`, `gSampler`.
//! - `gInput.Load`/`gInput.SampleLevel` themselves (the actual texture
//!   fetch/hardware bilinear filter): both are admitted here only as an
//!   opaque caller-supplied sampling function ([`box_filter_tap`]'s
//!   `load: impl Fn(i32, i32) -> [f32; 4]`, [`bicubic_filter`]'s
//!   `sample_level: impl Fn(f32, f32) -> [f32; 4]`), matching this crate's
//!   `tmem`/`combiner` precedent of admitting upstream texel/color values as
//!   typed inputs rather than re-deriving GPU texture-unit behavior.
//! - `RWTexture2D<float4> gOutput`'s indexed store (`gOutput[coord] = ...`):
//!   the arithmetic that produces the stored value is ported; the store
//!   itself is not (no GPU resource exists here to store into).
//! - `BicubicScalingCS.hlsl`'s `CSMain` in-bounds dispatch guard
//!   (`coord.x < OutputResolution.x && coord.y < OutputResolution.y`): a
//!   dispatch-overhang guard for GPU thread grids wider than the output,
//!   not resampling arithmetic; [`bicubic_filter`] is called directly by a
//!   caller that already knows its own coordinate is in range.
//!
//! ## Admitted domain
//!
//! - **Box filter integer/float mix**: `BoxFilterCB`'s `Resolution`,
//!   `ResolutionScale`, and `Misalignment` are HLSL `int2`, ported as
//!   `[i32; 2]`. `clampedCoord` addressing and the tap-count divisor
//!   (`ResolutionScale.x * ResolutionScale.y`) are computed in `i32`
//!   exactly as the HLSL does, then the divisor is converted to `f32` only
//!   at the final division (an implicit HLSL `int`-to-`float` conversion),
//!   matching the source's own operator-promotion point rather than
//!   promoting earlier.
//! - **Box filter zero-size/degenerate scale**: `ResolutionScale.x <= 0` or
//!   `.y <= 0` makes the corresponding `for` loop execute zero iterations
//!   (HLSL `for (int x = 0; x < N; x++)` with `N <= 0` never enters the
//!   body, identical to Rust's `for x in 0..n` for `n <= 0` after the port's
//!   plain-range re-expression -- see [`box_filter_tap`]'s doc),
//!   leaving `resultColor` at its `0.0f` initializer and then dividing by
//!   `0 * anything = 0` or `anything * 0 = 0`: `0.0 / 0.0`, which HLSL (and
//!   this port's IEEE-754 `f32` division) evaluates to `NaN`, not a
//!   division-by-zero trap. This port reproduces that `NaN` result rather
//!   than special-casing it.
//! - **Box filter clamp with `maxCoord < 0`** (a `Resolution` of `(0,0)` or
//!   less): `clamp(v, 0, maxCoord)` with `maxCoord < min` is HLSL's (and
//!   this port's) literal two-sided `clamp` behavior for an inverted range;
//!   this port uses the same unconditional `max(min_bound,
//!   min(v, max_bound))` formula HLSL's `clamp` intrinsic documents, not a
//!   defensive reordering, so an inverted range's result follows from that
//!   literal formula rather than being special-cased.
//! - **Bicubic `cubic()` coefficients and accumulation order**: for
//!   `w = cubic(x)`, with `x2 = x*x`, `x3 = x2*x`:
//!   - `w.x = -x3 + 3.0*x2 - 3.0*x + 1.0` (left-to-right: negate `x3`, add
//!     `3.0*x2`, subtract `3.0*x`, add `1.0` -- four terms, three
//!     left-associative `+`/`-` folds in that exact order).
//!   - `w.y = 3.0*x3 - 6.0*x2 + 4.0` (three terms, two folds).
//!   - `w.z = -3.0*x3 + 3.0*x2 + 3.0*x + 1.0` (four terms, three folds).
//!   - `w.w = x3` (no accumulation).
//!   - Each component is then divided by `6.0` independently (`w / 6.0`
//!     broadcasts across all four lanes) -- not divided once collectively
//!     or folded into the polynomial's own coefficients. Floating-point
//!     addition is not associative, so [`cubic`] below reproduces this
//!     exact left-to-right term order per component; it does not
//!     reassociate (e.g. it does not compute `3.0*(x2 - x)` in place of
//!     `3.0*x2 - 3.0*x`, even though those are algebraically identical over
//!     the reals).
//!   - `BicubicFilter` calls `cubic` at the two literal constants
//!     `fx = fy = -0.5` only -- `cubic` itself stays a function of `x` (as
//!     RT64 wrote it) so [`cubic`] is ported generally, not hardcoded to
//!     `-0.5`, but every characterization fixture below that feeds
//!     `bicubic_filter` exercises it only at that one fixed evaluation
//!     point, matching the shader's own only call site.
//! - **Bicubic weighted-sum accumulation order (`s`, `offset`, final
//!   blend)**: `s = (xcubic.x+xcubic.y, xcubic.z+xcubic.w, ycubic.x+ycubic.y,
//!   ycubic.z+ycubic.w)` -- each lane is exactly one pairwise `+`, no
//!   three-or-more-term fold to reassociate. `offset = c +
//!   (xcubic.y,xcubic.w,ycubic.y,ycubic.w)/s`: the division happens first
//!   (higher HLSL operator precedence than `+`), then the add -- reproduced
//!   in that exact order (divide-then-add, not add-then-divide). The final
//!   blend is `lerp(lerp(sample3, sample2, sx), lerp(sample1, sample0, sx),
//!   sy)`: two inner lerps (`sample3`->`sample2` blended by `sx`, and
//!   `sample1`->`sample0` blended by `sx`) whose *results* are then blended
//!   by a third `lerp` weighted by `sy` -- a specific nesting order (not a
//!   4-tap weighted average computed as one flat sum), reproduced exactly by
//!   [`bicubic_filter`] and by the WGSL sibling's matching three nested
//!   `lerp` calls, in that same sample-index-to-lerp-argument mapping
//!   (`sample3` is `lerp`'s `x`/first-argument, `sample2` its `y`, etc. --
//!   not a relabeled but algebraically-equivalent pairing).
//! - **HLSL `lerp` vs. WGSL `mix`**: HLSL's `lerp(x, y, s)` is documented as
//!   `x + s*(y - x)`. WGSL's built-in `mix(e1, e2, e3)` is specified as
//!   `e1*(1-e3) + e2*e3` -- algebraically equal over the reals, but a
//!   *different* floating-point expression (different rounding at each
//!   intermediate step), so substituting `mix` for `lerp` would not
//!   generally be bit-exact. This port does not use WGSL's `mix`: both
//!   [`lerp`] (the Rust oracle-adjacent helper) and `BICUBIC_SCALING_WGSL`
//!   spell out `x + s * (y - x)` literally, so the two independent
//!   derivations (Rust vs. WGSL text) use the identical expression form.
//! - **Texel-center offset convention**: `BicubicFilter` never applies an
//!   explicit `+0.5`/`-0.5` texel-center bias to `coord` itself (unlike,
//!   e.g., a nearest/bilinear sampler that centers on `texel + 0.5`);
//!   instead its four sample offsets are `coord.x - 0.5`, `coord.x + 1.5`
//!   (and the `y` analogues) *before* the per-cubic-lobe correction
//!   (`+ xcubic.y/s.x` etc.) is added -- i.e., the `-0.5`/`+1.5` literals
//!   are RT64's own bicubic-lobe placement (four taps spanning two texels on
//!   each side of `coord`), computed directly from `coord = uv *
//!   outputResolution` with no separate texel-center pass applied before or
//!   after scaling. This port makes no independent texel-center claim
//!   beyond reproducing those two literals unchanged.
//! - **Clamp/wrap at texture edges**: `BicubicFilter`'s
//!   `gInput.SampleLevel` relies entirely on `gSampler`'s own configured
//!   HLSL `SamplerState` address mode (clamp, wrap, mirror, or border --
//!   never specified in this file) for out-of-`[0,1]`-range UV coordinates;
//!   no clamp/wrap/mirror logic appears in `BicubicFilter`'s own text. This
//!   port makes no address-mode claim: [`bicubic_filter`]'s
//!   `sample_level` parameter is an opaque caller-supplied function that may
//!   receive out-of-`[0,1]` UV coordinates unchanged, and this module does
//!   not constrain, clamp, or wrap them itself. `BoxFilterCS.hlsl`, by
//!   contrast, *does* clamp explicitly in-shader (`clamp(..., int2(0,0),
//!   maxCoord)`, ported verbatim above) -- the two shaders use genuinely
//!   different edge policies, not a single shared convention.
//! - **`saturate`**: neither shader calls HLSL's `saturate` anywhere -- not
//!   on weights, not on the final color. No clamping to `[0,1]` is ported
//!   or introduced; out-of-range weights or colors (e.g. from a `cubic`
//!   evaluation at an `x` outside `[-1,0]`, though the only real call site
//!   uses the fixed `-0.5`) pass through unclamped, exactly as the HLSL
//!   does.
//! - **Other intrinsics**: no `mad`, `rcp`, or `frac` appears in either
//!   ported shader body; `clamp`, `+`/`-`/`*`//` operators, and `lerp`
//!   (accounted for above) are the only arithmetic intrinsics this port
//!   admits. `float`/`int`/`uint` literal precision: every literal in both
//!   shaders (`0.0f`, `1.0`, `3.0`, `6.0`, `-0.5`, `1.5`, `4.0`, ...) is
//!   exactly representable in IEEE-754 single precision, so no literal
//!   itself is a precision concern; the accumulation-order notes above are
//!   the only source of potential non-bit-exactness against a naively
//!   reassociated re-expression, which this port avoids.
//!
//! ## Nonclaims
//!
//! This module makes no GPU execution claim: the WGSL validation test below
//! runs Naga's WGSL front-end and validator only (a plain, non-GPU test),
//! not a real adapter/device dispatch -- this host has a real GPU reachable
//! via the `host-gpu-tests` feature, but no test here uses it. It makes no
//! production-wiring claim: no pipeline, `wgpu::ShaderModule`, bind group
//! layout, push-constant layout, or `targets/`/draw-path integration is
//! created here, and this module is not referenced from any other module in
//! this crate. It makes no parity or performance claim against RT64's own
//! renderer, and no claim about `Color.hlsli`'s own behavior (unused by
//! this port, see above). Compute-dispatch scaffolding (`[numthreads]`,
//! `SV_DispatchThreadID`, `groupshared`, barriers, texture/sampler binds,
//! the `RWTexture2D` store, `BicubicScalingCS.hlsl`'s dispatch-overhang
//! guard) is not ported -- see "Ported vs. skipped" above.

/// `BoxFilterCB`'s three `int2` fields (`BoxFilterCS.hlsl:7-11`), each
/// `[x, y]`. Reused directly by [`box_filter_tap`] rather than a new
/// per-field type -- this module has no other consumer of `int2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxFilterParams {
    /// `Resolution`: the source texture's full pixel size.
    pub resolution: [i32; 2],
    /// `ResolutionScale`: how many source taps per output texel, per axis.
    pub resolution_scale: [i32; 2],
    /// `Misalignment`: a fixed per-axis tap-origin offset.
    pub misalignment: [i32; 2],
}

/// Literal port of `BoxFilterCS.hlsl:19-28`'s resampling arithmetic (the
/// `CSMain` body minus `SV_DispatchThreadID`/`[numthreads]` dispatch
/// scaffolding and the `RWTexture2D` store -- see module doc "Ported vs.
/// skipped"). `coord` is the ported `SV_DispatchThreadID` value (an output
/// texel coordinate); `load` stands in for `gInput.Load` (an opaque
/// caller-supplied point sampler over the source texture, taking clamped
/// integer `(x, y)` and returning an RGBA `[f32; 4]`).
///
/// The nested `for (x) for (y)` loop is re-expressed as `0..scale` ranges,
/// which for `scale <= 0` iterate zero times -- identical to HLSL's
/// `for (int x = 0; x < N; x++)` never entering its body when `N <= 0`
/// (see module doc "Box filter zero-size/degenerate scale").
pub fn box_filter_tap(
    coord: [i32; 2],
    params: BoxFilterParams,
    load: impl Fn(i32, i32) -> [f32; 4],
) -> [f32; 4] {
    let mut result_color = [0.0f32; 4];
    let max_coord = [params.resolution[0] - 1, params.resolution[1] - 1];

    for x in 0..params.resolution_scale[0] {
        for y in 0..params.resolution_scale[1] {
            let raw = [
                coord[0] * params.resolution_scale[0] + x + params.misalignment[0],
                coord[1] * params.resolution_scale[1] + y + params.misalignment[1],
            ];
            let clamped = [
                raw[0].max(0).min(max_coord[0]),
                raw[1].max(0).min(max_coord[1]),
            ];
            let tap = load(clamped[0], clamped[1]);
            result_color[0] += tap[0];
            result_color[1] += tap[1];
            result_color[2] += tap[2];
            result_color[3] += tap[3];
        }
    }

    let divisor = (params.resolution_scale[0] * params.resolution_scale[1]) as f32;
    [
        result_color[0] / divisor,
        result_color[1] / divisor,
        result_color[2] / divisor,
        result_color[3] / divisor,
    ]
}

pub const BOX_FILTER_WGSL: &str = include_str!("shaders/box_filter.wgsl");
/// The WGSL's `@compute` dispatch entry point (`box_filter_entry`), not to
/// be confused with the WGSL's `box_filter_tap` function -- the latter is
/// the ported arithmetic itself (mirroring this module's own
/// [`box_filter_tap`]), named identically on both sides deliberately.
pub const BOX_FILTER_ENTRY_POINT: &str = "box_filter_entry";

/// `BicubicCB`'s two `uint2` fields (`BicubicScalingCS.hlsl:11-14`), each
/// `[x, y]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BicubicFilterParams {
    /// `InputResolution`. Unused by `BicubicFilter` itself (only
    /// `CSMain`'s skipped dispatch guard would read `OutputResolution`
    /// twice and never touch `InputResolution` either) -- carried here only
    /// because it is part of the source `BicubicCB` struct this port cites;
    /// [`bicubic_filter`] does not read it.
    pub input_resolution: [u32; 2],
    /// `OutputResolution`.
    pub output_resolution: [u32; 2],
}

/// Literal port of HLSL `lerp(x, y, s) = x + s*(y-x)`
/// (`BicubicScalingCS.hlsl:50`'s three call sites). Not WGSL's `mix`
/// (`e1*(1-e3)+e2*e3` -- see module doc "HLSL `lerp` vs. WGSL `mix`").
pub fn lerp(x: f32, y: f32, s: f32) -> f32 {
    x + s * (y - x)
}

/// Literal port of `float4 cubic(float x)` (`BicubicScalingCS.hlsl:21-30`).
/// Returns `[w.x, w.y, w.z, w.w]` after the `/6.0` broadcast. See module doc
/// "Bicubic `cubic()` coefficients and accumulation order" for the exact
/// per-lane term order this reproduces.
pub fn cubic(x: f32) -> [f32; 4] {
    let x2 = x * x;
    let x3 = x2 * x;
    let wx = -x3 + 3.0 * x2 - 3.0 * x + 1.0;
    let wy = 3.0 * x3 - 6.0 * x2 + 4.0;
    let wz = -3.0 * x3 + 3.0 * x2 + 3.0 * x + 1.0;
    let ww = x3;
    [wx / 6.0, wy / 6.0, wz / 6.0, ww / 6.0]
}

/// Literal port of `float4 BicubicFilter(...)`
/// (`BicubicScalingCS.hlsl:32-51`), minus the `Texture2D`/`SamplerState`
/// parameters themselves (replaced by the opaque `sample_level` callback --
/// see module doc "Ported vs. skipped"). `uv` is the ported `uv` parameter
/// (already `coord / OutputResolution`-normalized by the caller, matching
/// `CSMain`'s own call site); `output_resolution` is `outputResolution`
/// widened to `f32` (the HLSL `uint2` promotes implicitly in `uv *
/// outputResolution` and in the final `/ outputResolution` re-normalization
/// -- both ported as plain `f32` arithmetic here, matching that implicit
/// promotion point rather than promoting earlier or later).
pub fn bicubic_filter(
    uv: [f32; 2],
    output_resolution: [f32; 2],
    sample_level: impl Fn(f32, f32) -> [f32; 4],
) -> [f32; 4] {
    let fx = -0.5f32;
    let fy = -0.5f32;
    let xcubic = cubic(fx);
    let ycubic = cubic(fy);
    let coord = [uv[0] * output_resolution[0], uv[1] * output_resolution[1]];

    let c = [
        coord[0] - 0.5,
        coord[0] + 1.5,
        coord[1] - 0.5,
        coord[1] + 1.5,
    ];
    let s = [
        xcubic[0] + xcubic[1],
        xcubic[2] + xcubic[3],
        ycubic[0] + ycubic[1],
        ycubic[2] + ycubic[3],
    ];
    let offset = [
        c[0] + xcubic[1] / s[0],
        c[1] + xcubic[3] / s[1],
        c[2] + ycubic[1] / s[2],
        c[3] + ycubic[3] / s[3],
    ];

    let sample0 = sample_level(
        offset[0] / output_resolution[0],
        offset[2] / output_resolution[1],
    );
    let sample1 = sample_level(
        offset[1] / output_resolution[0],
        offset[2] / output_resolution[1],
    );
    let sample2 = sample_level(
        offset[0] / output_resolution[0],
        offset[3] / output_resolution[1],
    );
    let sample3 = sample_level(
        offset[1] / output_resolution[0],
        offset[3] / output_resolution[1],
    );

    let sx = s[0] / (s[0] + s[1]);
    let sy = s[2] / (s[2] + s[3]);

    let top = [
        lerp(sample3[0], sample2[0], sx),
        lerp(sample3[1], sample2[1], sx),
        lerp(sample3[2], sample2[2], sx),
        lerp(sample3[3], sample2[3], sx),
    ];
    let bottom = [
        lerp(sample1[0], sample0[0], sx),
        lerp(sample1[1], sample0[1], sx),
        lerp(sample1[2], sample0[2], sx),
        lerp(sample1[3], sample0[3], sx),
    ];

    [
        lerp(top[0], bottom[0], sy),
        lerp(top[1], bottom[1], sy),
        lerp(top[2], bottom[2], sy),
        lerp(top[3], bottom[3], sy),
    ]
}

/// Literal port of `CSMain`'s in-body UV construction
/// (`BicubicScalingCS.hlsl:56`'s `float2(coord) / float2(gConstants.
/// OutputResolution)` argument expression, evaluated *before* the dispatch
/// guard -- the guard itself is not ported, see module doc "Ported vs.
/// skipped"), then [`bicubic_filter`]. `coord` is an output-texel
/// coordinate (the ported `SV_DispatchThreadID`, widened to `f32` matching
/// HLSL's implicit `uint`-to-`float` conversion in `float2(coord)`);
/// `params.output_resolution` supplies both the UV divisor and
/// [`bicubic_filter`]'s own `output_resolution` argument, matching
/// `CSMain`'s single `gConstants.OutputResolution` read reused for both.
pub fn bicubic_filter_at_coord(
    coord: [u32; 2],
    params: BicubicFilterParams,
    sample_level: impl Fn(f32, f32) -> [f32; 4],
) -> [f32; 4] {
    let output_resolution = [
        params.output_resolution[0] as f32,
        params.output_resolution[1] as f32,
    ];
    let uv = [
        coord[0] as f32 / output_resolution[0],
        coord[1] as f32 / output_resolution[1],
    ];
    bicubic_filter(uv, output_resolution, sample_level)
}

pub const BICUBIC_SCALING_WGSL: &str = include_str!("shaders/bicubic_scaling.wgsl");
/// The WGSL's `@compute` dispatch entry point (`bicubic_scaling_entry`), not
/// to be confused with the WGSL's `bicubic_filter` function -- the latter is
/// the ported arithmetic itself (mirroring this module's own
/// [`bicubic_filter`], named identically on both sides deliberately).
pub const BICUBIC_SCALING_ENTRY_POINT: &str = "bicubic_scaling_entry";

#[cfg(test)]
mod tests;
