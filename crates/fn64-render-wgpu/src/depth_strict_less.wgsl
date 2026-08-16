// Strict-less depth compare/update seam. Characterization-only; not wired
// into any draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `Framebuffer::set_depth_tested`
// (`fn64-render-reference/src/raster/draw.rs:632-653`): `z < memory_z`
// passes and commits fragment_z/fragment_rgba; anything else (equal or
// farther) rejects and echoes memory_z/memory_rgba back unchanged. No
// blend, coverage, alpha compare, DepthMode dispatch, or framebuffer-read
// beyond the single memory sample this struct already carries.

struct StrictLessDepthInput {
    fragment_z: f32,
    memory_z: f32,
    fragment_rgba: u32,
    memory_rgba: u32,
}

struct StrictLessDepthOutput {
    passed: u32,
    committed_depth: f32,
    committed_rgba: u32,
    reserved_zero: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<StrictLessDepthInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<StrictLessDepthOutput>;

fn depth_passes(fragment_z: f32, memory_z: f32) -> bool {
    return fragment_z < memory_z;
}

fn evaluate(input: StrictLessDepthInput) -> StrictLessDepthOutput {
    if (depth_passes(input.fragment_z, input.memory_z)) {
        return StrictLessDepthOutput(1u, input.fragment_z, input.fragment_rgba, 0u);
    }
    return StrictLessDepthOutput(0u, input.memory_z, input.memory_rgba, 0u);
}

@compute @workgroup_size(64)
fn strict_less_depth_test(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
