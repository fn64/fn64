
fn evaluate(input: AlphaCompareInput) -> u32 {
    var passed: bool;
    if (input.copy_cycle_rgba16 != 0u && (input.mode == 1u || input.mode == 3u)) {
        passed = input.alpha != 0u;
    } else {
        passed = alpha_compare_general(input.mode, input.alpha, input.threshold_alpha, input.noise_byte);
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
