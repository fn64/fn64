// Coverage, fragment-callable form. Characterization-only; not wired into
// any draw path, bind group layout, or entry point used elsewhere in this
// crate -- see the sibling `coverage.rs` module doc and `coverage.wgsl`'s
// own header for the exact scope boundary this file shares.
//
// Ordinary WGSL function re-expression of `coverage.wgsl`'s existing
// `evaluate` logic (whole file, itself composed of `coverage_alpha`/
// `coverage_times_alpha_value`), a literal transcription of
// `coverage_result`/`apply_coverage_alpha`
// (`fn64-render-reference/src/raster/coverage.rs:61-115`, ported via this
// crate's `coverage.rs`): four `cvg_dst` modes (Clamp=0, Wrap=1, Full=2,
// Save=3) accumulate `pixel`/`memory` coverage into `destination`, `wraps`,
// and `blend_enabled`; `coverage_times_alpha`/`alpha_coverage_select`
// independently adjust the fragment's coverage and alpha channel, with
// `alpha_coverage_select` reading the *already* times-alpha-adjusted
// coverage, not the raw `destination` (matches `apply_coverage_alpha`'s own
// sequencing exactly).
//
// Unlike `coverage.wgsl`'s `evaluate`, this function takes plain scalar
// `u32` arguments already available in fragment-shader scope instead of a
// single struct parameter read from a storage buffer, and returns a plain
// struct instead of a storage-buffer write -- the exact input/output
// contract `coverage_result`/`apply_coverage_alpha` already use on the Rust
// side (`pixel`/`memory`/mode bits in, a result struct out). This file
// declares no resource bindings and no entry point of its own, so it is an
// ordinary callable concatenated at build time into a future fragment
// entry point, the same mechanism `shaders/triangle_pipeline_fragment.wgsl`'s
// own header already documents for `color_combiner.wgsl`. No caller in this
// crate invokes it yet; the bind-group plumbing and the `fs_main` call site
// are explicitly deferred to a future slice (see `coverage.rs`'s module doc
// and this crate's README).
//
// The full four-way `cvg_dst` match is ported (there is no honest partial
// port of `evaluate` -- see `coverage.rs`'s module doc), but only the
// `Full`/`Save` branches (plus the coverage-times-alpha/alpha-coverage-
// select composition on top of `Full`) are independently GPU-differentially
// validated by this crate's test suite: both modes need no real
// framebuffer-sourced `memory` value to exercise honestly. `Clamp`/`Wrap`
// are transcribed here but not GPU-validated -- see this crate's README for
// the exact boundary.

struct CoverageFragmentResult {
    destination_count: u32,
    wraps: u32,
    blend_enabled: u32,
    adjusted_alpha: u32,
    adjusted_coverage_count: u32,
}

const CVG_DST_CLAMP: u32 = 0u;
const CVG_DST_WRAP: u32 = 1u;
const CVG_DST_FULL: u32 = 2u;
const CVG_DST_SAVE: u32 = 3u;
const COVERAGE_FULL: u32 = 8u;

fn coverage_alpha_fn(count: u32) -> u32 {
    return (count * 255u + 4u) / 8u;
}

fn coverage_times_alpha_value_fn(count: u32, alpha: u32) -> u32 {
    return (count * alpha + 127u) / 255u;
}

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
        adjusted_coverage = coverage_times_alpha_value_fn(destination, fragment_alpha);
    }
    var adjusted_alpha = fragment_alpha;
    if (alpha_coverage_select != 0u) {
        adjusted_alpha = coverage_alpha_fn(adjusted_coverage);
    }

    var result: CoverageFragmentResult;
    result.destination_count = destination;
    result.wraps = select(0u, 1u, wraps);
    result.blend_enabled = select(0u, 1u, blend_enabled);
    result.adjusted_alpha = adjusted_alpha;
    result.adjusted_coverage_count = adjusted_coverage;
    return result;
}
