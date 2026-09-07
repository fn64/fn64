// Alpha compare seam. Characterization-only; not wired into any draw path
// or bind group layout used elsewhere in this crate.
//
// Storage-buffer compute wrapper around the shared `alpha_compare_general`
// body (`alpha_compare_body.wgsl`, single-sourced with the fragment-callable
// twin below it -- see that file's own header for the shared arithmetic's
// scope/citations). This file supplies only the compute-dispatchable shell:
// the input/output buffer layout, the copy-cycle RGBA16 special case, and
// the `@compute` entry point.

struct AlphaCompareInput {
    mode: u32,
    alpha: u32,
    threshold_alpha: u32,
    noise_byte: u32,
    copy_cycle_rgba16: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<AlphaCompareInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<u32>;

