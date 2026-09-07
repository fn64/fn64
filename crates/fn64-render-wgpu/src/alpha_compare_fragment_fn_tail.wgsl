
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
