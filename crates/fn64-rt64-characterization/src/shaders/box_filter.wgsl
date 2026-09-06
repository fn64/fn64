// BoxFilterCS seam. Characterization-only; not wired into any pipeline,
// bind group layout, or draw path used elsewhere in this crate.
//
// Literal WGSL re-expression of `box_filter_tap`
// (`crate::rt64_resample`, mirroring RT64's
// `src/shaders/BoxFilterCS.hlsl:19-28`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). Reproduces exactly:
// - the `maxCoord = resolution - (1,1)` bound
// - the nested tap-loop's `clamp(coord*scale + (x,y) + misalignment, 0,
//   maxCoord)` addressing
// - the `result_color += tap` accumulation
// - the final `result_color / (scale.x * scale.y)` average
//
// `gInput`/`gOutput` binds and `[numthreads]`/`SV_DispatchThreadID`
// dispatch scaffolding are declared below only so this file is a complete,
// bindable WGSL compute module for Naga's validator -- no pipeline is ever
// created from it and it is never dispatched on a real adapter/device (see
// `crate::rt64_resample`'s doc comment "Ported vs. skipped"/"Nonclaims").

struct BoxFilterConstants {
    resolution: vec2<i32>,
    resolution_scale: vec2<i32>,
    misalignment: vec2<i32>,
}

@group(0) @binding(0) var<uniform> gConstants: BoxFilterConstants;
@group(0) @binding(1) var gInput: texture_2d<f32>;
@group(0) @binding(2) var gOutput: texture_storage_2d<rgba8unorm, write>;

fn box_filter_tap(coord: vec2<i32>) -> vec4<f32> {
    var result_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    let max_coord = gConstants.resolution - vec2<i32>(1, 1);

    for (var x: i32 = 0; x < gConstants.resolution_scale.x; x = x + 1) {
        for (var y: i32 = 0; y < gConstants.resolution_scale.y; y = y + 1) {
            let raw = coord * gConstants.resolution_scale + vec2<i32>(x, y) + gConstants.misalignment;
            let clamped = clamp(raw, vec2<i32>(0, 0), max_coord);
            let tap = textureLoad(gInput, clamped, 0);
            result_color = result_color + tap;
        }
    }

    let divisor = f32(gConstants.resolution_scale.x * gConstants.resolution_scale.y);
    return result_color / divisor;
}

@compute @workgroup_size(8, 8, 1)
fn box_filter_entry(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);
    let result = box_filter_tap(coord);
    textureStore(gOutput, coord, result);
}
