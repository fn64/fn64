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
//
// Task 4.4 (single-source WGSL bodies): `coverage_alpha`/
// `coverage_times_alpha_value` below used to be `coverage_alpha_fn`/
// `coverage_times_alpha_value_fn` -- duplicate arithmetic under different
// names because this file's assembled text is hashed by
// `shader_manifest.rs`'s `TRIANGLE_PIPELINE_FRAGMENT_SOURCE_SHA256` and a
// straight byte-for-byte body share would have required keeping the old
// `_fn` names forever. Renaming to match `coverage.wgsl`'s own names lets
// both wrappers `include_str!` one shared `coverage_body.wgsl` verbatim,
// so drift between the two copies is now impossible by construction; the
// manifest hash was rebaselined in the same commit that made this change
// (see `coverage.rs`'s doc comment and the task report for the old/new
// digest pair).

struct CoverageFragmentResult {
    destination_count: u32,
    wraps: u32,
    blend_enabled: u32,
    adjusted_alpha: u32,
    adjusted_coverage_count: u32,
}

