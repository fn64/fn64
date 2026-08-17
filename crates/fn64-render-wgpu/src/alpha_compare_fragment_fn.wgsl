// Alpha compare, fragment-callable form. Concatenated by `shader_manifest.rs`
// into the production triangle fragment shader
// (`shaders/triangle_pipeline_fragment.wgsl`) and called from that file's
// `fs_main` after combiner evaluation, gating the fragment with `discard`
// before output construction -- see the sibling `alpha_compare.rs` module
// doc and `alpha_compare.wgsl`'s own header for the exact scope boundary
// this file shares. This wiring covers only the alpha-compare gate itself;
// it makes no claim about Dither, copy-cycle RGBA16, parity, or
// presentation -- see `triangle_pipeline_fragment.wgsl`'s own header and
// this crate's README for those boundaries.
//
// Ordinary WGSL function re-expression of `alpha_compare.wgsl`'s existing
// `general_compare`/`evaluate` logic (whole file), itself a literal
// transcription of `alpha_compare_value`/`copy_alpha_compare_value`
// (`fn64-render-reference/src/raster/blend.rs:105-149`): mode 0=None always
// passes, mode 1=Threshold passes iff `alpha >= threshold_alpha`, mode
// 3=Dither cross-multiplies `alpha*256 > noise_byte*255`. Mode 2 (Reserved)
// is a host-side decode-time rejection (see `require_supported_alpha_compare`
// in `alpha_compare.rs`); this function never receives it and returns
// `false` defensively if it somehow does. When `copy_cycle_rgba16` is
// nonzero and mode is Threshold or Dither, the RGBA16 copy-cycle
// hard-alpha-bit special case applies instead of the general arithmetic.
//
// Unlike `alpha_compare.wgsl`'s `evaluate`, this function takes plain
// scalar `u32` arguments already available in fragment-shader scope instead
// of a single struct parameter read from a storage buffer, and returns
// `bool` instead of a storage-buffer `u32` convention -- the exact
// input/output contract `alpha_compare_value`/`copy_alpha_compare_value`
// already use on the Rust side. This file declares no resource bindings and
// no entry point of its own, so it is an ordinary callable concatenated at
// build time into the fragment entry point, the same mechanism
// `shaders/triangle_pipeline_fragment.wgsl`'s own header already documents
// for `color_combiner.wgsl`. `shader_manifest.rs` performs that
// concatenation and `triangle_pipeline_fragment.wgsl`'s `fs_main` is the one
// caller in this crate (see `alpha_compare.rs`'s module doc and this
// crate's README for the wiring's exact scope).

fn alpha_compare_general(mode: u32, alpha: u32, threshold_alpha: u32, noise_byte: u32) -> bool {
    if (mode == 0u) {
        return true;
    }
    if (mode == 1u) {
        return alpha >= threshold_alpha;
    }
    if (mode == 3u) {
        return alpha * 256u > noise_byte * 255u;
    }
    return false;
}

fn alpha_compare_fragment_fn(
    mode: u32,
    alpha: u32,
    threshold_alpha: u32,
    noise_byte: u32,
    copy_cycle_rgba16: u32,
) -> bool {
    if (copy_cycle_rgba16 != 0u && (mode == 1u || mode == 3u)) {
        return alpha != 0u;
    }
    return alpha_compare_general(mode, alpha, threshold_alpha, noise_byte);
}
