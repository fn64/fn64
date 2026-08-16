use super::*;
use crate::recompiled::live_program::resident_backing_intersects_catalog;

const RESIDENT_ENTRY: GuestPc = GuestPc::new(0x8000_1000);
const RESIDENT_PHYSICAL: u32 = 0x1000;
/// Backed by a generation that is registered but never activated, so nothing
/// is resident over it.
const DORMANT_ENTRY: GuestPc = GuestPc::new(0x8000_2000);
const DORMANT_PHYSICAL: u32 = 0x2000;

fn shard_generation(id: u64, bank: u64, entry: GuestPc, image: &[u8]) -> PrecompiledGeneration {
    let end = GuestPc::new(entry.get() + image.len() as u32);
    PrecompiledGeneration::new(
        GenerationId::new(id),
        entry,
        end,
        entry,
        end,
        sha2::Sha256::digest(image).into(),
        vec![PrecompiledShard::new(BankId::new(bank), entry, end).unwrap()],
    )
    .unwrap()
}

fn write_physical(storage: &mut [u8], physical_start: u32, bytes: &[u8]) {
    for (index, byte) in bytes.iter().copied().enumerate() {
        storage[(physical_start as usize + index) ^ 3] = byte;
    }
}

/// Two generations, one activated and one dormant, with disjoint physical
/// backings.
fn two_generation_catalog(
    resident_image: &[u8],
    dormant_image: &[u8],
) -> BackedPrecompiledGenerationCatalogV1 {
    let mut catalog = PrecompiledGenerationCatalog::new();
    catalog
        .register(shard_generation(1, 11, RESIDENT_ENTRY, resident_image))
        .unwrap();
    catalog
        .register(shard_generation(2, 12, DORMANT_ENTRY, dormant_image))
        .unwrap();
    BackedPrecompiledGenerationCatalogV1::new(
        catalog,
        vec![
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(1),
                vec![BackedExecutableSpanV1::new(
                    RESIDENT_ENTRY,
                    RESIDENT_PHYSICAL,
                    resident_image.len() as u32,
                )
                .unwrap()],
            )
            .unwrap(),
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(2),
                vec![BackedExecutableSpanV1::new(
                    DORMANT_ENTRY,
                    DORMANT_PHYSICAL,
                    dormant_image.len() as u32,
                )
                .unwrap()],
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

/// The boundary predicate breaks the block for a write backing a RESIDENT
/// generation and chains through one backing only a DORMANT generation --
/// even though both lie inside the watched executable region.
#[test]
fn only_resident_backing_forces_a_block_boundary() {
    let resident_image = 0x2402_0001u32.to_be_bytes();
    let dormant_image = 0x2402_0002u32.to_be_bytes();
    let mut storage = vec![0u8; 0x8000];
    write_physical(&mut storage, RESIDENT_PHYSICAL, &resident_image);
    write_physical(&mut storage, DORMANT_PHYSICAL, &dormant_image);

    let mut generations = two_generation_catalog(&resident_image, &dormant_image);
    {
        let mem = Rdram::new(&mut storage);
        generations
            .activate_for_fetch(RESIDENT_ENTRY, &mem)
            .unwrap();
    }
    // Exactly one generation is resident, and it is the one at
    // RESIDENT_PHYSICAL. The dormant generation is registered and backed but
    // was never activated.
    assert_eq!(
        generations.active_generations(),
        vec![GenerationId::new(1)],
        "only the activated generation should be resident"
    );

    // Both spans are inside the watched executable region, so the OLD
    // predicate -- intersection with EXECUTABLE_WRITE_RANGES alone -- would
    // answer ExecutableChanged for both.
    assert!(
        resident_backing_intersects_catalog(&generations, RESIDENT_PHYSICAL, RESIDENT_PHYSICAL + 4),
        "a write over a resident generation's backing must be a boundary"
    );
    assert!(
        !resident_backing_intersects_catalog(&generations, DORMANT_PHYSICAL, DORMANT_PHYSICAL + 4),
        "a write over a backing with no resident generation must not be a boundary"
    );
}

/// THE SAFETY PROPERTY. A write to bytes with no resident generation does not
/// break the block -- and a LATER activation over exactly those bytes is
/// rejected as `AotMiss` rather than executing stale translated code.
///
/// This is the half that makes skipping the boundary sound.
/// `activate_for_fetch_with_digest` (`fn64-cpu-runtime` `generation/mod.rs:771`)
/// digests LIVE memory for every containing candidate before it consults
/// `self.active`, so the changed bytes are seen at activation time even though
/// no boundary was raised when they were written.
#[test]
fn a_write_with_nothing_resident_still_blocks_a_later_stale_activation() {
    let resident_image = 0x2402_0001u32.to_be_bytes();
    let dormant_image = 0x2402_0002u32.to_be_bytes();
    let mut storage = vec![0u8; 0x8000];
    write_physical(&mut storage, RESIDENT_PHYSICAL, &resident_image);
    write_physical(&mut storage, DORMANT_PHYSICAL, &dormant_image);

    let mut generations = two_generation_catalog(&resident_image, &dormant_image);
    {
        let mem = Rdram::new(&mut storage);
        generations
            .activate_for_fetch(RESIDENT_ENTRY, &mem)
            .unwrap();
    }

    // Nothing is resident over the dormant span, so the predicate says
    // "no boundary" -- the guest keeps executing without a scheduler
    // round trip.
    assert!(!resident_backing_intersects_catalog(
        &generations,
        DORMANT_PHYSICAL,
        DORMANT_PHYSICAL + 4
    ));

    // The guest now corrupts those un-resident bytes. No boundary was raised.
    let corrupted = 0xdead_beefu32.to_be_bytes();
    write_physical(&mut storage, DORMANT_PHYSICAL, &corrupted);

    // Activating over the corrupted bytes must NOT execute the dormant
    // generation's translated code. The digest is recomputed from live memory
    // and does not match, so activation fails.
    let mem = Rdram::new(&mut storage);
    let attempted = generations.activate_for_fetch(DORMANT_ENTRY, &mem);
    assert!(
        matches!(
            attempted,
            Err(GenerationLookupError::AotMiss(_))
                | Err(GenerationLookupError::NoGenerationMatched { .. })
        ),
        "activating over bytes changed while nothing was resident must be \
         rejected, not silently run stale code; got {attempted:?}"
    );
    // And nothing became resident over the corrupted span.
    assert_eq!(
        generations.active_generations(),
        vec![GenerationId::new(1)],
        "a failed activation must not publish the stale generation"
    );

    // Restoring the exact expected bytes lets the same activation succeed,
    // which proves the rejection above was the digest and not a broken setup.
    write_physical(&mut storage, DORMANT_PHYSICAL, &dormant_image);
    let mem = Rdram::new(&mut storage);
    let recovered = generations
        .activate_for_fetch(DORMANT_ENTRY, &mem)
        .expect("the unmodified image activates");
    assert_eq!(recovered.entry.bank, BankId::new(12));
}
