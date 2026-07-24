//! Phase-neutral owned effects shared by speculative audio execution lanes.
//!
//! IMEM replacement images affect future guest execution, while fn64's
//! monotonic generation and ordered journal are internal ownership/evidence
//! metadata rather than N64 registers. Both rspboot and ucode execution retain the same value shape so a
//! later whole-task adapter can concatenate their already-ordered effects
//! without reinterpreting either phase.

use crate::hle_outcome::{Sha256Digest, RSP_BANK_BYTES};

/// One complete IMEM image installed by an RSP DMA, in installation order.
///
/// Construction is crate-private so the stored digest cannot disagree with
/// the image. The value carries no commit authority and is intentionally
/// separate from [`crate::rsp::runtime::RspMachineState`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioImemReplacement {
    generation: u64,
    identity: Sha256Digest,
    image: [u8; RSP_BANK_BYTES],
}

impl AudioImemReplacement {
    pub(crate) fn from_image(generation: u64, image: [u8; RSP_BANK_BYTES]) -> Self {
        Self {
            generation,
            identity: Sha256Digest::hash(&image),
            image,
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn identity(&self) -> Sha256Digest {
        self.identity
    }

    pub const fn image(&self) -> &[u8; RSP_BANK_BYTES] {
        &self.image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_bound_to_the_complete_image() {
        let mut image = [0u8; RSP_BANK_BYTES];
        image[0] = 0xa5;
        image[RSP_BANK_BYTES - 1] = 0x5a;
        let replacement = AudioImemReplacement::from_image(9, image);

        assert_eq!(replacement.generation(), 9);
        assert_eq!(replacement.identity(), Sha256Digest::hash(&image));
        assert_eq!(replacement.image(), &image);
    }
}
