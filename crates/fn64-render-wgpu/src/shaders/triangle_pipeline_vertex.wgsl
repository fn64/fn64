// Real @vertex entry point for the triangle-pipeline slice (port card
// §3 step 2). `crate::raster_vs`'s existing `RASTER_VS_WGSL`
// (`shaders/raster_vs.wgsl`) is a `@compute` seam (storage-buffer in/out,
// entry `raster_vs`) written as a characterization oracle, not a `@vertex`
// stage a `wgpu::RenderPipelineDescriptor` can bind directly -- wgpu vertex
// stages take `@location` input attributes and return `@builtin(position)`,
// a structurally different entry shape. This file re-expresses the exact
// same transform arithmetic (`raster_vs.wgsl:45-69`, itself a literal port
// of RT64's `RasterVS.hlsl:15-33`, pinned commit
// `5473732a822a4423b5696e7cb18fecc425a59875`) as a real vertex stage, plus
// the `oUV`/`oSmoothColor` passthrough `raster_vs.rs`'s module doc states is
// out of scope for that module ("there is no arithmetic to port").
//
// This slice fixes `is_rect = false` (confirmed for every triangle-sourced
// draw by the landed `rt64-triangle-composition-precursor` commit, port card
// §4) and `z_override = false` (the fixed fixture's `OtherMode` is
// `from_wire(0, 0)`, whose `primitive_depth_source()` is false) -- so the
// rect-skip and Z-override branches `raster_vs.wgsl` guards with dynamic
// per-vertex flags are not reachable here and are omitted, not silently
// dropped. `resolution`/`screen_scale`/`screen_offset` remain real per-draw
// uniform inputs.

struct RasterParams {
    resolution_x: f32,
    resolution_y: f32,
    screen_scale_x: f32,
    screen_scale_y: f32,
    screen_offset_x: f32,
    screen_offset_y: f32,
    reserved_0: f32,
    reserved_1: f32,
}

@group(0) @binding(0)
var<uniform> raster_params: RasterParams;

struct VertexInput {
    @location(0) position: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var x = input.position.x;
    var y = input.position.y;
    var z = input.position.z;
    let w = input.position.w;

    // renderFlagRect skip (raster_vs.wgsl:51-59): always taken for this
    // slice's triangle-sourced vertices (is_rect = false, see module doc).
    x -= raster_params.resolution_x / 2.0;
    y -= raster_params.resolution_y / 2.0;
    x /= raster_params.resolution_x / 2.0;
    y /= raster_params.resolution_y / -2.0;
    x *= w;
    y *= w;
    z *= w;

    x = (x * raster_params.screen_scale_x) + raster_params.screen_offset_x * w;
    y = (y * raster_params.screen_scale_y) + raster_params.screen_offset_y * w;

    // z_override branch (raster_vs.wgsl:64-66) omitted: this slice's fixed
    // OtherMode(0, 0) has primitive_depth_source() == false, so RT64 itself
    // never takes this branch for the fixture this slice submits.

    var output: VertexOutput;
    output.position = vec4<f32>(x, y, z, w);
    output.uv = input.uv;
    output.color = input.color;
    return output;
}
