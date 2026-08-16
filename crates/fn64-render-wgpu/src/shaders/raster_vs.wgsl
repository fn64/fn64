// RasterVS seam. Characterization-only; not wired into any draw path or bind
// group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `raster_vs` (`crate::raster_vs`, mirroring
// RT64's `src/shaders/RasterVS.hlsl:15-33`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). Reproduces exactly:
// - the renderFlagRect-gated RDP-screen-to-NDC conversion
// - the unconditional screenScale/screenOffset apply
// - the (cycleType != G_CYC_COPY) && zSource==G_ZS_PRIM Z override
//
// oUV/oSmoothColor/oFlatColor passthrough is not represented (no arithmetic
// to port, matching crate::raster_vs's module doc "Scope").

struct RasterVsInput {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    resolution_x: f32,
    resolution_y: f32,
    screen_scale_x: f32,
    screen_scale_y: f32,
    screen_offset_x: f32,
    screen_offset_y: f32,
    is_rect: u32,
    z_override: u32,
    prim_depth_z_normalized: f32,
    // Pads the struct to a 16-byte multiple for storage-buffer alignment.
    reserved_zero: u32,
}

struct RasterVsOutput {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<RasterVsInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<RasterVsOutput>;

fn evaluate(input: RasterVsInput) -> RasterVsOutput {
    var x = input.x;
    var y = input.y;
    var z = input.z;
    let w = input.w;

    if (input.is_rect == 0u) {
        x -= input.resolution_x / 2.0;
        y -= input.resolution_y / 2.0;
        x /= input.resolution_x / 2.0;
        y /= input.resolution_y / -2.0;
        x *= w;
        y *= w;
        z *= w;
    }

    x = (x * input.screen_scale_x) + input.screen_offset_x * w;
    y = (y * input.screen_scale_y) + input.screen_offset_y * w;

    if (input.z_override != 0u) {
        z = input.prim_depth_z_normalized * w;
    }

    return RasterVsOutput(x, y, z, w);
}

@compute @workgroup_size(64)
fn raster_vs(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
