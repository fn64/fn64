use fn64_cpu_runtime::{AotMiss, BankId, GuestPc, Rdram};
use sha2::{Digest, Sha256};

/// Verify a short immutable executable image without hashing on the matching
/// hot path.
///
/// The expected digest remains the admitted image identity reported by an
/// [`AotMiss`]. Direct word comparison is equivalent for the normal case;
/// only a mismatch pays to materialize and hash the live bytes. This is
/// intended for tiny, frequently entered vector images where SHA-256 setup
/// dominates the actual comparison.
pub fn verify_precompiled_words(
    expected_bank: BankId,
    va_start: GuestPc,
    expected_words: &[u32],
    expected_sha256: [u8; 32],
    mem: &Rdram<'_>,
) -> Result<(), AotMiss> {
    assert!(va_start.is_instruction_aligned());
    let byte_len = u32::try_from(
        expected_words
            .len()
            .checked_mul(4)
            .expect("precompiled word image length overflow"),
    )
    .expect("precompiled word image exceeds u32 length");
    for (index, expected_word) in expected_words.iter().copied().enumerate() {
        let offset = u32::try_from(index)
            .expect("precompiled word index exceeds u32")
            .checked_mul(4)
            .expect("precompiled word offset overflow");
        let pc = va_start
            .get()
            .checked_add(offset)
            .expect("precompiled word image range overflow");
        let address = 0xffff_ffff_0000_0000u64 | u64::from(pc);
        if mem.load_w(address) as u32 != expected_word {
            let bytes = (0..byte_len)
                .map(|byte_offset| {
                    mem.load_bu(
                        0xffff_ffff_0000_0000u64
                            | u64::from(
                                va_start
                                    .get()
                                    .checked_add(byte_offset)
                                    .expect("precompiled word image range overflow"),
                            ),
                    )
                })
                .collect::<Vec<_>>();
            return Err(AotMiss {
                expected_bank,
                va_start,
                byte_len,
                expected_sha256,
                actual_sha256: Sha256::digest(bytes).into(),
                // This seam walks the EXPECTED words, so unlike the digest-only
                // callers it knows exactly which word first diverged. `offset`
                // is that word's byte offset from `va_start`.
                first_diff_offset: Some(offset),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_cpu_runtime::verify_precompiled_image;

    #[test]
    fn word_admission_matches_full_image_miss_evidence() {
        let words = [0x3c1a_8003u32, 0x275a_6790, 0x0340_0008, 0];
        let expected: [u8; 32] = Sha256::digest(
            words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect::<Vec<_>>(),
        )
        .into();
        let mut storage = vec![0u8; 0x200];
        let mut mem = Rdram::new(&mut storage);
        for (index, word) in words.iter().copied().enumerate() {
            mem.store_w(0xffff_ffff_8000_0180 + index as u64 * 4, word);
        }
        let bank = BankId::new(0x4321);
        let start = GuestPc::new(0x8000_0180);
        assert_eq!(
            verify_precompiled_words(bank, start, &words, expected, &mem),
            Ok(())
        );

        mem.store_w(0xffff_ffff_8000_0188, 0x0340_0009);
        let word_miss = verify_precompiled_words(bank, start, &words, expected, &mem)
            .expect_err("changed vector word must fail closed");
        let image_miss = verify_precompiled_image(bank, start, 16, expected, &mem)
            .expect_err("the full-image gate must see the same changed bytes");
        assert_eq!(word_miss, image_miss);
    }
}
