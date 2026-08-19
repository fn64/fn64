// Alpha compare seam. Characterization-only; not wired into any draw path
// or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `alpha_compare_value`/`copy_alpha_compare_value`
// (`fn64-render-reference/src/raster/blend.rs:105-149`): mode 0=None always
// passes, mode 1=Threshold passes iff `alpha >= threshold_alpha`, mode
// 3=Dither cross-multiplies `alpha*256 > noise_byte*255`. Mode 2 is NOT a
// reserved encoding: other-mode low bits 1:0 are two independent hardware
// bits (angrylion `src/core/n64video/rdp.c:659-660`), and `rdp/blender.c`'s
// `alpha_compare` returns 1 whenever bit 0 (`alpha_compare_en`) is clear.
// Mode 2 is `alpha_compare_en = 0`, so it always PASSES, exactly like mode 0
// (`docs/RT64-GUARD-AUDIT.md` finding A3). When `copy_cycle_rgba16 != 0u` and
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
    // Bit 0 is `alpha_compare_en`: clear (modes 0 and 2) means no compare.
    if ((mode & 1u) == 0u) {
        return true;
    }
    // Bit 1 is `dither_alpha_en`: mode 3 dithers the threshold, mode 1 uses
    // the blend-colour alpha.
    if ((mode & 2u) != 0u) {
        return alpha * 256u > noise_byte * 255u;
    }
    return alpha >= threshold_alpha;
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
