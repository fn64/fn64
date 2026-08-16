// Alpha compare seam. Characterization-only; not wired into any draw path
// or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `alpha_compare_value`/`copy_alpha_compare_value`
// (`fn64-render-reference/src/raster/blend.rs:105-149`): mode 0=None always
// passes, mode 1=Threshold passes iff `alpha >= threshold_alpha`, mode
// 3=Dither cross-multiplies `alpha*256 > noise_byte*255`. Mode 2 (Reserved)
// is a host-side decode-time rejection (see `require_supported_alpha_compare`
// in the sibling .rs module); this shader never receives it and returns
// false defensively if it somehow does. When `copy_cycle_rgba16 != 0u` and
// mode is Threshold or Dither, the RGBA16 copy-cycle hard-alpha-bit special
// case (`alpha != 0u`) applies instead of the general arithmetic.

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

fn general_compare(mode: u32, alpha: u32, threshold_alpha: u32, noise_byte: u32) -> bool {
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

fn evaluate(input: AlphaCompareInput) -> u32 {
    var passed: bool;
    if (input.copy_cycle_rgba16 != 0u && (input.mode == 1u || input.mode == 3u)) {
        passed = input.alpha != 0u;
    } else {
        passed = general_compare(input.mode, input.alpha, input.threshold_alpha, input.noise_byte);
    }
    if (passed) {
        return 1u;
    }
    return 0u;
}

@compute @workgroup_size(64)
fn alpha_compare_fragment(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
