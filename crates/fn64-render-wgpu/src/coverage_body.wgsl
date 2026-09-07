const CVG_DST_CLAMP: u32 = 0u;
const CVG_DST_WRAP: u32 = 1u;
const CVG_DST_FULL: u32 = 2u;
const CVG_DST_SAVE: u32 = 3u;
const COVERAGE_FULL: u32 = 8u;

fn coverage_alpha(count: u32) -> u32 {
    return (count * 255u + 4u) / 8u;
}

fn coverage_times_alpha_value(count: u32, alpha: u32) -> u32 {
    return (count * alpha + 127u) / 255u;
}
