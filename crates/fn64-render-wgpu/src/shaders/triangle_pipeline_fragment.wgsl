// Real @fragment entry point for the triangle-pipeline slice (port card §3
// step 3, restriction set). Thin wrapper over `crate::combiner`'s existing
// WGSL transcription (`shaders/color_combiner.wgsl`, `run_one_cycle`) --
// that file defines only library functions (no `@group`/`@binding`, no
// entry point), so this wrapper is concatenated after it at the Rust build
// seam (`TRIANGLE_PIPELINE_FRAGMENT_WGSL`) into one shader module, not
// included via a WGSL-native import mechanism (none exists in this wgpu
// version). `color_combiner.wgsl`'s own source is reused byte-for-byte,
// unmodified. `tmem_sample.wgsl` (published committed-TMEM textured-draw
// card, Option B) is concatenated the same way, ahead of this file, so
// `fs_main` can call its `sample_committed_rgba16_three_nearest_bound` entry
// directly.
//
// Implements exactly RasterPS.hlsl lines 165-184 (port card §1c/§3 step 3)
// plus this slice's own texture-sample wiring: build `ColorCombiner::Inputs`
// from the vertex's interpolated color, the real per-fragment
// `tmem_sample.wgsl` texel sample, and a caller-supplied fixed
// `CombineParams`, call the one-cycle formula. Every other RasterPS block
// (alpha compare, blend, coverage, decal, depth clip) is out of scope per
// the restriction set in §3 -- this slice is opaque-only, no-blend
// (`blend: None` in the pipeline's `ColorTargetState`, not fragment-shader
// logic), smooth-shaded only (`oSmoothColor`, matching `raster_vs.wgsl`'s
// existing output naming).
//
// Observable shader failure status (card audit repair: "observable shader
// failure status"). `fs_main` writes a second color attachment
// (`@location(1)`, `R32Uint`) carrying `tmem_sample.wgsl`'s own
// `TMEM_SAMPLE_STATUS_*` code for this fragment, alongside the ordinary
// RGBA8 color attachment -- the only channel a fragment shader has to report
// a per-pixel typed failure back to the CPU. `production.rs`'s draw path
// reads this second attachment back and turns any non-`TMEM_SAMPLE_STATUS_OK`
// texel into a named `WgpuRawDpcExecutionError` variant; it is never
// silently ignored.
//
// Conditional TMEM sample call (SHADE-only-triangle repair): `fs_main` only
// calls `sample_committed_rgba16_three_nearest_bound` when
// `texture_referenced != 0` -- host-serialized from
// `CombineParams::references_texels_in_first_cycle()`, a selector-reference
// predicate over this triangle's own real `SetCombine` value (see that
// function's doc for its one narrow exception: slot C is `(A-B)*C`'s own
// coefficient, so it only counts as a reference when A and B decode to
// different selectors -- otherwise `(A-B)` is exactly zero and C's value
// can never reach the output regardless of what it decodes to). A triangle
// whose combine formula never reads TEXEL0/TEXEL1/TEXEL0_ALPHA/TEXEL1_ALPHA
// with a nonzero coefficient (e.g. a pure SHADE passthrough) legitimately
// carries `TileBindingParams::unbound()` -- calling the sampler
// unconditionally for such a triangle would report
// `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` and abort the draw even though the
// combiner output never depends on the texel value. When the gate is
// closed, `tex_val0`/`tex_val1` are the fixed zero color and the reported
// status is `TMEM_SAMPLE_STATUS_OK` -- unbound must not read as success
// when the combiner *does* reference a texel, which is exactly what the
// `texture_referenced != 0` branch below still guards against: that path
// always calls the real sampler and propagates whatever status it returns,
// loud failures included.

struct FragmentCombineParams {
    low: u32,
    high: u32,
    texture_referenced: u32,
    reserved_zero: u32,
}

@group(0) @binding(1)
var<uniform> fragment_combine_params: FragmentCombineParams;

// Alpha-compare production card §3b: the real post-combiner discard gate's
// mode/threshold uniform. `mode` is `OtherMode::alpha_compare()`'s wire
// encoding, guaranteed 0 (None) or 1 (Threshold) by CPU-side retrieval-time
// rejection of Reserved/Dither (see `raw_dpc::triangle_draw_data`'s and
// `production.rs`'s `PlanCollector`'s own retrieval-time panics) --
// `alpha_compare_fragment_fn` is never called with `mode == 3u` (Dither) in
// this slice. `threshold_alpha` is `G_SETBLENDCOLOR.a`, zero-extended.
struct FragmentAlphaCompareParams {
    mode: u32,
    threshold_alpha: u32,
    _reserved_0: u32,
    _reserved_1: u32,
}

@group(0) @binding(5)
var<uniform> fragment_alpha_compare_params: FragmentAlphaCompareParams;

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) tmem_sample_status: u32,
}

@fragment
fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
) -> FragmentOutput {
    // `uv` carries raw S10.5 texture-coordinate units (matching
    // `tmem/sample.rs`'s `TextureCoordinateS10_5`), linearly interpolated by
    // the rasterizer across the triangle -- three genuinely distinct
    // per-vertex UVs (card requirement) produce a genuinely varying `uv`
    // here, not a per-triangle constant. Interpolation can land on a
    // negative raw coordinate (raw S10.5 units are signed), so this must
    // floor toward negative infinity before truncating to `i32` -- plain
    // `i32(uv.x)` truncates toward zero and disagrees with `tmem/sample.rs`'s
    // integer-only addressing (and its own `relative_axis_coordinate` port
    // in `tmem_sample.wgsl`, which already assumes an exact integer input)
    // for any negative interpolated value.
    var sample: TmemSampleResult;
    if fragment_combine_params.texture_referenced != 0u {
        sample = sample_committed_rgba16_three_nearest_bound(i32(floor(uv.x)), i32(floor(uv.y)));
    } else {
        sample.status = TMEM_SAMPLE_STATUS_OK;
        sample.color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    var inputs: CombinerInputs;
    inputs.tex_val0 = sample.color;
    inputs.tex_val1 = sample.color;
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

    // Alpha-compare production card §3c: real post-combiner, pre-output
    // discard gate. Operates on the combiner's own output alpha (its `.a`
    // channel), matching `alpha_compare.rs`'s doc framing of alpha compare
    // as a post-combine gate -- not the raw vertex/texel alpha. `noise_byte`
    // is always `0u` (mode is guaranteed 0 or 1 by CPU-side rejection, see
    // struct doc above); `copy_cycle_rgba16` is always `0u` (no copy-cycle
    // triangle path exists in this pipeline yet).
    let alpha_u32 = u32(clamp(result.combiner_color.a, 0.0, 1.0) * 255.0 + 0.5);
    let alpha_compare_passed = alpha_compare_fragment_fn(
        fragment_alpha_compare_params.mode,
        alpha_u32,
        fragment_alpha_compare_params.threshold_alpha,
        0u,
        0u,
    );
    if (!alpha_compare_passed) {
        discard;
    }

    var output: FragmentOutput;
    output.color = result.combiner_color;
    output.tmem_sample_status = sample.status;
    return output;
}
