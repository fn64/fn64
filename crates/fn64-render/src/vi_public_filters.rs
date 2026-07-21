//! Backend-neutral deterministic realizations of public VI filter mechanisms.
//!
//! Public documentation specifies fresh random low-bit noise before final
//! seven-bit gamma-dither quantization, but does not publish the silicon
//! generator, seed, or advancement. The policy below preserves fn64's
//! existing reproducible coordinate/channel stream and names that boundary
//! explicitly; it is an executable cross-backend contract, not a hardware-RNG
//! claim.

/// Stable identity for the deterministic policy implemented by this module.
pub const VI_PUBLIC_FILTER_POLICY_ID: &str = "fn64.vi-public-filters.bounded-v1";

/// Restore one five-bit RGBA16 component to the VI's eight-bit digital output
/// using the signed comparisons against every available 3x3 neighbor.
///
/// The comparison topology follows US 5,699,079. Valid five-bit inputs and at
/// most eight neighbors keep the exact result in `0..=248`.
pub fn restore_rgba16_component_bounded_v1(center: u8, neighbors: &[u8]) -> u8 {
    assert!(center < 32, "RGBA16 center component exceeds five bits");
    assert!(
        neighbors.len() <= 8,
        "RGBA16 restoration has more than eight neighbors"
    );
    let mut restored = i16::from(center) << 3;
    for &neighbor in neighbors {
        assert!(neighbor < 32, "RGBA16 neighbor component exceeds five bits");
        restored += match neighbor.cmp(&center) {
            core::cmp::Ordering::Less => -1,
            core::cmp::Ordering::Equal => 0,
            core::cmp::Ordering::Greater => 1,
        };
    }
    u8::try_from(restored).expect("valid RGBA16 restoration result fits eight bits")
}

/// One checked random bit consumed by the public gamma-dither quantizer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViRandomBit(u8);

impl ViRandomBit {
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 1 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Stochastically round one eight-bit channel to seven bits, then expand that
/// result back to eight-bit host storage by replicating its high bit.
pub const fn gamma_dither_quantize_bounded_v1(channel: u8, random: ViRandomBit) -> u8 {
    let quantized = channel.saturating_add(random.value()) >> 1;
    (quantized << 1) | (quantized >> 6)
}

/// Deterministic SplitMix64-derived bit keyed by retrace cycle, output pixel,
/// and RGB channel. This is fn64 policy, not silicon random-stream identity.
pub const fn reference_noise_bit_v1(
    retrace_cycle: u64,
    output_pixel: u64,
    channel: u8,
) -> ViRandomBit {
    let key = retrace_cycle
        ^ output_pixel.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (channel as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mut mixed = key.wrapping_add(0x9e37_79b9_7f4a_7c15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ViRandomBit((mixed ^ (mixed >> 31)) as u8 & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bit_is_checked() {
        assert_eq!(ViRandomBit::new(0).unwrap().value(), 0);
        assert_eq!(ViRandomBit::new(1).unwrap().value(), 1);
        assert_eq!(ViRandomBit::new(2), None);
    }

    #[test]
    fn rgba16_restoration_preserves_exact_neighbor_and_border_vectors() {
        assert_eq!(restore_rgba16_component_bounded_v1(10, &[11; 8]), 88);
        assert_eq!(restore_rgba16_component_bounded_v1(10, &[9; 8]), 72);
        assert_eq!(restore_rgba16_component_bounded_v1(10, &[11; 3]), 83);
        assert_eq!(restore_rgba16_component_bounded_v1(10, &[9; 5]), 75);
        assert_eq!(
            restore_rgba16_component_bounded_v1(10, &[9, 10, 11, 9, 10, 11, 9, 11]),
            80
        );
    }

    #[test]
    fn rgba16_restoration_exhausts_every_comparison_partition() {
        for center in 0u8..32 {
            for total in 0usize..=8 {
                for less in 0usize..=total {
                    for equal in 0usize..=total - less {
                        let greater = total - less - equal;
                        if (less > 0 && center == 0) || (greater > 0 && center == 31) {
                            continue;
                        }
                        let mut neighbors = Vec::with_capacity(total);
                        neighbors.extend(std::iter::repeat_n(center.saturating_sub(1), less));
                        neighbors.extend(std::iter::repeat_n(center, equal));
                        neighbors
                            .extend(std::iter::repeat_n(center + u8::from(greater > 0), greater));
                        let expected = (i16::from(center) << 3) - less as i16 + greater as i16;
                        assert_eq!(
                            restore_rgba16_component_bounded_v1(center, &neighbors),
                            u8::try_from(expected)
                                .expect("valid RGBA16 restoration result fits eight bits")
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gamma_dither_quantizer_preserves_exact_bounded_vectors() {
        let zero = ViRandomBit::new(0).unwrap();
        let one = ViRandomBit::new(1).unwrap();
        assert_eq!(gamma_dither_quantize_bounded_v1(0, zero), 0);
        assert_eq!(gamma_dither_quantize_bounded_v1(0, one), 0);
        assert_eq!(gamma_dither_quantize_bounded_v1(100, zero), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(100, one), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(101, zero), 100);
        assert_eq!(gamma_dither_quantize_bounded_v1(101, one), 102);
        assert_eq!(gamma_dither_quantize_bounded_v1(128, zero), 129);
        assert_eq!(gamma_dither_quantize_bounded_v1(255, one), 255);
    }

    #[test]
    fn seeded_stream_preserves_the_reference_vector() {
        let input = [0, 1, 2, 63, 64, 65, 100, 101, 127, 128, 129, 254, 255];
        let actual = input.map(|channel| {
            let pixel = input
                .iter()
                .position(|candidate| candidate == &channel)
                .unwrap() as u64;
            gamma_dither_quantize_bounded_v1(
                channel,
                reference_noise_bit_v1(0x0123_4567_89ab_cdef, pixel, 0),
            )
        });
        assert_eq!(
            actual,
            [0, 0, 2, 64, 64, 64, 100, 100, 126, 129, 131, 255, 255]
        );
    }
}
