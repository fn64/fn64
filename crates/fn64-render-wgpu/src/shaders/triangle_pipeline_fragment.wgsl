// Real @fragment entry point for the triangle-pipeline slice (port card §3
// step 3, restriction set). Thin wrapper over `crate::combiner`'s existing
// WGSL transcription (`shaders/color_combiner.wgsl`, `run_one_cycle`) --
// that file defines only library functions (no `@group`/`@binding`, no
// entry point), so this wrapper is concatenated after it at the Rust build
// seam (`TRIANGLE_PIPELINE_FRAGMENT_WGSL`) into one shader module, not
// included via a WGSL-native import mechanism (none exists in this wgpu
// version). `color_combiner.wgsl`'s own source is reused byte-for-byte,
// unmodified.
//
// Implements exactly RasterPS.hlsl lines 165-184 (port card §1c/§3 step 3):
// build `ColorCombiner::Inputs` from the vertex's interpolated color plus a
// caller-supplied fixed `CombineParams`, call the one-cycle formula. Every
// other RasterPS block (texture sample, alpha compare, blend, coverage,
// decal, depth clip) is out of scope per the restriction set in §3 -- this
// slice is textureless, opaque-only, no-blend (`blend: None` in the
// pipeline's `ColorTargetState`, not fragment-shader logic), smooth-shaded
// only (`oSmoothColor`, matching `raster_vs.wgsl`'s existing output naming).

struct FragmentCombineParams {
    low: u32,
    high: u32,
}

@group(0) @binding(1)
var<uniform> fragment_combine_params: FragmentCombineParams;

@fragment
fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> @location(0) vec4<f32> {
    var inputs: CombinerInputs;
    inputs.tex_val0 = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    inputs.tex_val1 = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    inputs.prim_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    inputs.shade_color = color;
    inputs.env_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    inputs.key_center = vec3<f32>(0.0, 0.0, 0.0);
    inputs.key_scale = vec3<f32>(0.0, 0.0, 0.0);
    inputs.lod_fraction = 0.0;
    inputs.prim_lod_frac = 0.0;
    inputs.noise = 0.0;
    inputs.k4 = 0.0;
    inputs.k5 = 0.0;

    let params = CombineParams(fragment_combine_params.low, fragment_combine_params.high);
    let result = run_one_cycle(params, inputs);
    return result.combiner_color;
}
