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
