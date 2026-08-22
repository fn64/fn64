//! N64 RDP depth-memory encoding.
//!
//! Nintendo 64 Programming Manual, Chapter 16, "Z Image Format" defines the
//! stored Z value as a 3-bit exponent plus 11-bit mantissa decoded into the
//! RDP's unsigned 15.3 (18-bit) working value. A 4-bit priority-encoded DeltaZ
//! shares the visible 16-bit word: its upper two bits occupy the word's low
//! bits and its lower two bits occupy the RDRAM hidden bits.

const Z_SHIFT: [u32; 8] = [6, 5, 4, 3, 2, 1, 0, 0];
const Z_ADD: [u32; 8] = [
    0x00000, 0x20000, 0x30000, 0x38000, 0x3c000, 0x3e000, 0x3f000, 0x3f800,
];

/// One encoded RDP Z-memory pixel, including the two RDRAM hidden bits that
/// ordinary CPU halfword accesses cannot observe.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EncodedDepth {
    pub visible: u16,
    pub hidden: u8,
}

impl EncodedDepth {
    /// Expand one 16-bit half of the MI fill-color register into its physical
    /// 18-bit depth-memory pixel. Programming Manual Chapter 12.8, Figure
    /// 12.8.2 shows that fill mode replicates the halfword's least-significant
    /// bit into both hidden positions. Keeping this constructor on the encoded
    /// storage type prevents depth-fill producers from inventing hidden bits
    /// independently of the memory-interface wire rule.
    pub const fn from_fill_halfword(visible: u16) -> Self {
        Self {
            visible,
            hidden: if visible & 1 == 0 { 0 } else { 3 },
        }
    }
}

/// Decode the visible 14-bit exponent/mantissa field to the RDP's unsigned
/// 18-bit 15.3 working value.
pub fn decode_z(encoded_z: u16) -> u32 {
    let exponent = usize::from((encoded_z >> 11) & 7);
    let mantissa = u32::from(encoded_z & 0x07ff);
    (mantissa << Z_SHIFT[exponent]) + Z_ADD[exponent]
}

/// Quantize an unsigned 18-bit working Z value into exponent/mantissa form.
/// Values outside the hardware range saturate to `0x3ffff` before encoding.
pub fn encode_z(z: u32) -> u16 {
    let z = z.min(0x3ffff);
    let exponent = match z {
        0x00000..=0x1ffff => 0,
        0x20000..=0x2ffff => 1,
        0x30000..=0x37fff => 2,
        0x38000..=0x3bfff => 3,
        0x3c000..=0x3dfff => 4,
        0x3e000..=0x3efff => 5,
        0x3f000..=0x3f7ff => 6,
        _ => 7,
    };
    let mantissa = ((z - Z_ADD[exponent]) >> Z_SHIFT[exponent]) as u16;
    ((exponent as u16) << 11) | mantissa
}

/// Priority-encode a 16-bit pixel DeltaZ to the four-bit stored exponent.
/// Programming Manual Chapter 15, Equation 10 defines `log2(DeltaZpix)` and
/// states that the bit number of the most-significant one is stored. Zero and
/// one therefore both encode as zero; larger values use floor(log2), saturated
/// to the four-bit maximum.
pub fn encode_delta_z(delta_z: u16) -> u8 {
    if delta_z == 0 {
        0
    } else {
        (u16::BITS - 1 - delta_z.leading_zeros()).min(15) as u8
    }
}

/// Expand a stored four-bit DeltaZ exponent for the blender's comparisons.
/// Encoding zero represents the minimum comparison delta of one, including
/// the conventional zero-DeltaZ depth clear.
pub fn decode_delta_z(encoded_delta: u8) -> u16 {
    1 << encoded_delta.min(15)
}

/// The four public Z comparison signals from Programming Manual Chapter 15,
/// Equations 5-9. Coverage-wrap refinements consume these signals later; this
/// type keeps their boundary semantics independently testable.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DepthRelations {
    pub memory_is_max: bool,
    pub farther: bool,
    pub nearer: bool,
    pub in_front: bool,
}

pub fn relations(
    pixel_z: u32,
    pixel_delta_z: u16,
    memory_z: u32,
    memory_encoded_delta_z: u8,
) -> DepthRelations {
    let delta_z_max = u32::from(pixel_delta_z.max(decode_delta_z(memory_encoded_delta_z)));
    DepthRelations {
        memory_is_max: memory_z >= 0x3ffff,
        farther: pixel_z.saturating_add(delta_z_max) >= memory_z,
        nearer: pixel_z.saturating_sub(delta_z_max) <= memory_z,
        in_front: pixel_z < memory_z,
    }
}

/// Mode-dependent color-write admission without the coverage-wrap override.
/// Chapter 15 §15.7 defines opaque/interpenetrating surfaces as accepting
/// clearly-front or correlated fragments, translucent surfaces as strict
/// in-front compares, and decals as correlated-only with a non-clear memory
/// sample. Exact coverage-wrap selection remains in the coverage unit.
pub fn mode_passes(mode: crate::gbi::DepthMode, relations: DepthRelations) -> bool {
    use crate::gbi::DepthMode;
    match mode {
        DepthMode::Opaque | DepthMode::Interpenetrating => relations.nearer,
        DepthMode::Translucent => relations.in_front,
        DepthMode::Decal => relations.farther && relations.nearer && !relations.memory_is_max,
    }
}

/// Pack exponent/mantissa and four-bit DeltaZ into the CPU-visible halfword
/// plus the two hidden RDRAM bits.
pub fn pack(z: u32, delta_z: u16) -> EncodedDepth {
    let encoded_z = encode_z(z);
    let encoded_delta = encode_delta_z(delta_z);
    EncodedDepth {
        visible: (encoded_z << 2) | u16::from(encoded_delta >> 2),
        hidden: encoded_delta & 3,
    }
}

/// Recover the 18-bit working Z and four-bit stored DeltaZ exponent.
pub fn unpack(encoded: EncodedDepth) -> (u32, u8) {
    let encoded_z = encoded.visible >> 2;
    let encoded_delta = (((encoded.visible & 3) as u8) << 2) | (encoded.hidden & 3);
    (decode_z(encoded_z), encoded_delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponent_boundaries_match_the_manual_decode_table() {
        let boundaries = [
            (0x00000, 0),
            (0x20000, 1),
            (0x30000, 2),
            (0x38000, 3),
            (0x3c000, 4),
            (0x3e000, 5),
            (0x3f000, 6),
            (0x3f800, 7),
        ];
        for (z, exponent) in boundaries {
            let encoded = encode_z(z);
            assert_eq!(encoded >> 11, exponent);
            assert_eq!(encoded & 0x07ff, 0);
            assert_eq!(decode_z(encoded), z);
        }
    }

    #[test]
    fn every_stored_z_code_is_canonical_after_decode_encode() {
        for encoded in 0..=0x3fff {
            assert_eq!(encode_z(decode_z(encoded)), encoded);
        }
    }

    #[test]
    fn quantization_never_moves_depth_farther_than_one_mantissa_step() {
        for z in 0..=0x3ffff {
            let encoded = encode_z(z);
            let decoded = decode_z(encoded);
            let shift = Z_SHIFT[usize::from(encoded >> 11)];
            assert!(decoded <= z);
            assert!(z - decoded < (1 << shift));
        }
    }

    #[test]
    fn delta_priority_encoding_uses_most_significant_bit_index() {
        assert_eq!(encode_delta_z(0), 0);
        assert_eq!(encode_delta_z(1), 0);
        assert_eq!(encode_delta_z(2), 1);
        assert_eq!(encode_delta_z(3), 1);
        assert_eq!(encode_delta_z(4), 2);
        assert_eq!(encode_delta_z(0x4000), 14);
        assert_eq!(encode_delta_z(0xffff), 15);
    }

    #[test]
    fn delta_exponents_expand_to_their_power_of_two_floor() {
        for exponent in 0..=15 {
            assert_eq!(decode_delta_z(exponent), 1u16 << exponent);
        }
        assert_eq!(decode_delta_z(0xff), 0x8000);
    }

    #[test]
    fn every_nonzero_delta_quantizes_to_its_power_of_two_floor() {
        for delta_z in 1..=u16::MAX {
            let decoded = decode_delta_z(encode_delta_z(delta_z));
            assert!(decoded <= delta_z);
            assert!(u32::from(delta_z) < u32::from(decoded) * 2);
        }
    }

    #[test]
    fn public_z_relations_preserve_inclusive_delta_boundaries() {
        let front = relations(91, 4, 100, 3);
        assert_eq!(
            front,
            DepthRelations {
                memory_is_max: false,
                farther: false,
                nearer: true,
                in_front: true,
            }
        );
        let near_boundary = relations(92, 4, 100, 3);
        assert!(near_boundary.farther && near_boundary.nearer);
        let far_boundary = relations(108, 4, 100, 3);
        assert!(far_boundary.farther && far_boundary.nearer);
        let behind = relations(109, 4, 100, 3);
        assert!(behind.farther && !behind.nearer && !behind.in_front);
    }

    #[test]
    fn z_modes_select_front_or_correlated_fragments() {
        use crate::gbi::DepthMode;
        let clear = relations(0x3f000, 1, 0x3ffff, 0);
        assert!(mode_passes(DepthMode::Opaque, clear));
        assert!(mode_passes(DepthMode::Interpenetrating, clear));
        assert!(mode_passes(DepthMode::Translucent, clear));
        assert!(!mode_passes(DepthMode::Decal, clear));

        let clearly_front = relations(91, 4, 100, 3);
        assert!(mode_passes(DepthMode::Opaque, clearly_front));
        assert!(mode_passes(DepthMode::Interpenetrating, clearly_front));
        assert!(mode_passes(DepthMode::Translucent, clearly_front));
        assert!(!mode_passes(DepthMode::Decal, clearly_front));

        let correlated = relations(108, 4, 100, 3);
        assert!(mode_passes(DepthMode::Opaque, correlated));
        assert!(mode_passes(DepthMode::Interpenetrating, correlated));
        assert!(!mode_passes(DepthMode::Translucent, correlated));
        assert!(mode_passes(DepthMode::Decal, correlated));

        let behind = relations(109, 4, 100, 3);
        for mode in [
            DepthMode::Opaque,
            DepthMode::Interpenetrating,
            DepthMode::Translucent,
            DepthMode::Decal,
        ] {
            assert!(!mode_passes(mode, behind));
        }
    }

    #[test]
    fn visible_and_hidden_delta_bits_round_trip() {
        for encoded_z in 0..=0x3fff {
            for encoded_delta in 0..=15u8 {
                let encoded = EncodedDepth {
                    visible: (encoded_z << 2) | u16::from(encoded_delta >> 2),
                    hidden: encoded_delta & 3,
                };
                assert_eq!(unpack(encoded), (decode_z(encoded_z), encoded_delta));
            }
        }
    }

    #[test]
    fn every_fill_halfword_replicates_its_lsb_into_both_hidden_bits() {
        for visible in 0..=u16::MAX {
            let encoded = EncodedDepth::from_fill_halfword(visible);
            let expected_hidden = if visible & 1 == 0 { 0 } else { 3 };
            assert_eq!(encoded.visible, visible);
            assert_eq!(encoded.hidden, expected_hidden);
            assert_eq!(
                unpack(encoded).1,
                (((visible & 3) as u8) << 2) | expected_hidden
            );
        }
    }
}
