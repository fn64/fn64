// BicubicScalingCS seam. Characterization-only; not wired into any
// pipeline, bind group layout, or draw path used elsewhere in this crate.
//
// Literal WGSL re-expression of `cubic`/`bicubic_filter`
// (`crate::rt64_resample`, mirroring RT64's
// `src/shaders/BicubicScalingCS.hlsl:21-51`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). Reproduces exactly:
// - `cubic`'s four-lane polynomial and its exact left-to-right term order
//   per lane (float addition is not associative -- not reassociated)
// - the fixed `fx = fy = -0.5` evaluation
// - the `c`/`s`/`offset` derivation (divide-then-add order preserved)
// - the four sample-UV computations
// - the `lerp(lerp(sample3, sample2, sx), lerp(sample1, sample0, sx), sy)`
//   nested blend, using the literal `x + s*(y-x)` HLSL `lerp` formula (NOT
//   WGSL's built-in `mix`, which is a different floating-point expression --
//   see `crate::rt64_resample`'s doc comment "HLSL `lerp` vs. WGSL `mix`")
//
// `gInput`/`gSampler`/`gOutput` binds and `[numthreads]`/
// `SV_DispatchThreadID`/the dispatch-overhang bounds guard are declared or
// omitted below only so this file is a complete, bindable WGSL compute
// module for Naga's validator -- no pipeline is ever created from it and it
// is never dispatched on a real adapter/device (see
// `crate::rt64_resample`'s doc comment "Ported vs. skipped"/"Nonclaims").

struct BicubicConstants {
    input_resolution: vec2<u32>,
    output_resolution: vec2<u32>,
}

@group(0) @binding(0) var<uniform> gConstants: BicubicConstants;
@group(0) @binding(1) var gInput: texture_2d<f32>;
@group(0) @binding(2) var gOutput: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var gSampler: sampler;

// Literal port of HLSL `lerp(x, y, s) = x + s*(y-x)`. Deliberately not
// WGSL's `mix` -- see module doc above.
fn lerp(x: vec4<f32>, y: vec4<f32>, s: f32) -> vec4<f32> {
    return x + s * (y - x);
}

fn cubic(x: f32) -> vec4<f32> {
    let x2 = x * x;
    let x3 = x2 * x;
    var w: vec4<f32>;
    w.x = -x3 + 3.0 * x2 - 3.0 * x + 1.0;
    w.y = 3.0 * x3 - 6.0 * x2 + 4.0;
    w.z = -3.0 * x3 + 3.0 * x2 + 3.0 * x + 1.0;
    w.w = x3;
    return w / 6.0;
}

fn bicubic_filter(uv: vec2<f32>, output_resolution: vec2<f32>) -> vec4<f32> {
    let fx = -0.5;
    let fy = -0.5;
    let xcubic = cubic(fx);
    let ycubic = cubic(fy);
    let coord = uv * output_resolution;

    let c = vec4<f32>(coord.x - 0.5, coord.x + 1.5, coord.y - 0.5, coord.y + 1.5);
    let s = vec4<f32>(
        xcubic.x + xcubic.y,
        xcubic.z + xcubic.w,
        ycubic.x + ycubic.y,
        ycubic.z + ycubic.w,
    );
    let offset = c + vec4<f32>(xcubic.y, xcubic.w, ycubic.y, ycubic.w) / s;

    let sample0 = textureSampleLevel(gInput, gSampler, vec2<f32>(offset.x, offset.z) / output_resolution, 0.0);
    let sample1 = textureSampleLevel(gInput, gSampler, vec2<f32>(offset.y, offset.z) / output_resolution, 0.0);
    let sample2 = textureSampleLevel(gInput, gSampler, vec2<f32>(offset.x, offset.w) / output_resolution, 0.0);
    let sample3 = textureSampleLevel(gInput, gSampler, vec2<f32>(offset.y, offset.w) / output_resolution, 0.0);

    let sx = s.x / (s.x + s.y);
    let sy = s.z / (s.z + s.w);
    return lerp(lerp(sample3, sample2, sx), lerp(sample1, sample0, sx), sy);
}

@compute @workgroup_size(8, 8, 1)
fn bicubic_scaling_entry(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = global_id.xy;
    if (coord.x < gConstants.output_resolution.x && coord.y < gConstants.output_resolution.y) {
        let uv = vec2<f32>(coord) / vec2<f32>(gConstants.output_resolution);
        let result = bicubic_filter(uv, vec2<f32>(gConstants.output_resolution));
        textureStore(gOutput, coord, result);
    }
}
