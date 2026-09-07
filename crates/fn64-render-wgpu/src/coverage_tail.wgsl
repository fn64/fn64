
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
