// Pure coverage arithmetic seam. Characterization-only; not wired into any
// draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `coverage_result` and `apply_coverage_alpha`
// (`fn64-render-reference/src/raster/coverage.rs:61-115`, ported via this
// crate's `coverage.rs`): four `cvg_dst` modes (Clamp=0, Wrap=1, Full=2,
// Save=3) accumulate `pixel`/`memory` coverage into `destination`, `wraps`,
// and `blend_enabled`; `coverage_times_alpha`/`alpha_coverage_select`
// independently adjust the fragment's coverage and alpha channel. No
// framebuffer-read mechanism, draw-call integration, or native GPU
// execution -- this module's inputs/outputs are plain storage-buffer
// records the caller supplies and reads back.

struct CoverageInput {
    pixel_count: u32,
    memory_count: u32,
    image_read_enabled: u32,
    force_blend: u32,
    antialias_enabled: u32,
    coverage_destination: u32,
    coverage_times_alpha: u32,
    alpha_coverage_select: u32,
    fragment_alpha: u32,
}

struct CoverageOutput {
    destination_count: u32,
    wraps: u32,
    blend_enabled: u32,
    adjusted_alpha: u32,
    adjusted_coverage_count: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<CoverageInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<CoverageOutput>;

const CVG_DST_CLAMP: u32 = 0u;
const CVG_DST_WRAP: u32 = 1u;
const CVG_DST_FULL: u32 = 2u;
const CVG_DST_SAVE: u32 = 3u;
const COVERAGE_FULL: u32 = 8u;

fn coverage_alpha(count: u32) -> u32 {
    return (count * 255u + 4u) / 8u;
}

fn coverage_times_alpha_value(count: u32, alpha: u32) -> u32 {
    return (count * alpha + 127u) / 255u;
}

fn evaluate(input: CoverageInput) -> CoverageOutput {
    let image_read = input.image_read_enabled != 0u;
    let force_blend = input.force_blend != 0u;
    let antialias = input.antialias_enabled != 0u;

    var sum: u32 = input.pixel_count;
    if (image_read) {
        sum = input.pixel_count + input.memory_count;
    }
    let wraps = image_read && (sum > COVERAGE_FULL);
    let blend_enabled = force_blend || (antialias && !wraps);

    var destination: u32 = input.pixel_count;
    if (input.coverage_destination == CVG_DST_CLAMP) {
        if (image_read && blend_enabled) {
            destination = min(sum, COVERAGE_FULL);
        } else {
            destination = input.pixel_count;
        }
    } else if (input.coverage_destination == CVG_DST_WRAP) {
        if (image_read) {
            if (wraps) {
                destination = sum - COVERAGE_FULL;
            } else {
                destination = sum;
            }
        } else {
            destination = input.pixel_count;
        }
    } else if (input.coverage_destination == CVG_DST_FULL) {
        destination = COVERAGE_FULL;
    } else {
        // CVG_DST_SAVE
        destination = input.memory_count;
    }

    var adjusted_coverage = destination;
    if (input.coverage_times_alpha != 0u) {
        adjusted_coverage = coverage_times_alpha_value(destination, input.fragment_alpha);
    }
    var adjusted_alpha = input.fragment_alpha;
    if (input.alpha_coverage_select != 0u) {
        adjusted_alpha = coverage_alpha(adjusted_coverage);
    }

    var result: CoverageOutput;
    result.destination_count = destination;
    result.wraps = select(0u, 1u, wraps);
    result.blend_enabled = select(0u, 1u, blend_enabled);
    result.adjusted_alpha = adjusted_alpha;
    result.adjusted_coverage_count = adjusted_coverage;
    return result;
}

@compute @workgroup_size(64)
fn evaluate_coverage(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
