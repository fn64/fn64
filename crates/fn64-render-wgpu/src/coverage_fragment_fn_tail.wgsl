
fn coverage_fragment_fn(
    pixel_count: u32,
    memory_count: u32,
    image_read_enabled: u32,
    force_blend: u32,
    antialias_enabled: u32,
    coverage_destination: u32,
    coverage_times_alpha: u32,
    alpha_coverage_select: u32,
    fragment_alpha: u32,
) -> CoverageFragmentResult {
    let image_read = image_read_enabled != 0u;
    let force_blend_on = force_blend != 0u;
    let antialias = antialias_enabled != 0u;

    var sum: u32 = pixel_count;
    if (image_read) {
        sum = pixel_count + memory_count;
    }
    let wraps = image_read && (sum > COVERAGE_FULL);
    let blend_enabled = force_blend_on || (antialias && !wraps);

    var destination: u32 = pixel_count;
    if (coverage_destination == CVG_DST_CLAMP) {
        if (image_read && blend_enabled) {
            destination = min(sum, COVERAGE_FULL);
        } else {
            destination = pixel_count;
        }
    } else if (coverage_destination == CVG_DST_WRAP) {
        if (image_read) {
            if (wraps) {
                destination = sum - COVERAGE_FULL;
            } else {
                destination = sum;
            }
        } else {
            destination = pixel_count;
        }
    } else if (coverage_destination == CVG_DST_FULL) {
        destination = COVERAGE_FULL;
    } else {
        // CVG_DST_SAVE
        destination = memory_count;
    }

    var adjusted_coverage = destination;
    if (coverage_times_alpha != 0u) {
        adjusted_coverage = coverage_times_alpha_value(destination, fragment_alpha);
    }
    var adjusted_alpha = fragment_alpha;
    if (alpha_coverage_select != 0u) {
        adjusted_alpha = coverage_alpha(adjusted_coverage);
    }

    var result: CoverageFragmentResult;
    result.destination_count = destination;
    result.wraps = select(0u, 1u, wraps);
    result.blend_enabled = select(0u, 1u, blend_enabled);
    result.adjusted_alpha = adjusted_alpha;
    result.adjusted_coverage_count = adjusted_coverage;
    return result;
}
