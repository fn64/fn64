fn alpha_compare_general(mode: u32, alpha: u32, threshold_alpha: u32, noise_byte: u32) -> bool {
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
