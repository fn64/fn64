// Pure coverage arithmetic seam. Characterization-only; not wired into any
// draw path or bind group layout used elsewhere in this crate.
//
// Storage-buffer compute wrapper around the shared `coverage_alpha`/
// `coverage_times_alpha_value` body (`coverage_body.wgsl`, single-sourced
// with the fragment-callable twin below it -- Task 4.4). See that file's
// own header for the shared arithmetic; the citation for the full
// `evaluate` logic this wrapper's `evaluate` implements is
// `coverage_result`/`apply_coverage_alpha`
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

