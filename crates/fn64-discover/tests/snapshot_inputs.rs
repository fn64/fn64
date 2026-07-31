use fn64_discover::facts::{
    function_entry_subject, load_image_table_record_subject, BankAddr, CandidateDetector,
    FunctionEntryEvidence, MappingAddressSpace, ProofState, RomAddressSpace,
};
use fn64_discover::snapshot_inputs::{
    prepare_snapshot_banks, prepare_snapshot_banks_with_limits, PrepareSnapshotBanksError,
    PrepareSnapshotBanksLimits,
};
use fn64_discover::{normalize, Fact, FactDb, NormalizedRom};

fn synthetic_rom(writes: &[(usize, &[u8])]) -> NormalizedRom {
    let mut bytes = vec![0u8; 0x400];
    bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
    for (offset, value) in writes {
        bytes[*offset..*offset + value.len()].copy_from_slice(value);
    }
    normalize(&bytes).unwrap()
}

fn mapping(
    facts: &mut FactDb,
    bank: &str,
    state: ProofState,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
) -> usize {
    let index = facts.insert(Fact::RomMapping {
        bank: bank.to_string(),
        rom_space,
        rom_start,
        rom_end,
        va_start,
        va_end,
    });
    facts
        .conclude(format!("bank:{bank}"), state, vec![index], "test mapping")
        .unwrap();
    index
}

fn entry_claim(
    facts: &mut FactDb,
    bank: &str,
    pc: u32,
    evidence: FunctionEntryEvidence,
    proposed_state: ProofState,
    conclusion: Option<ProofState>,
) {
    let target = BankAddr::new(bank, pc);
    let index = facts.insert(Fact::FunctionEntryClaim {
        target: target.clone(),
        detector: match &evidence {
            FunctionEntryEvidence::Prologue { .. } => CandidateDetector::ProloguePattern,
            _ => CandidateDetector::JalTarget,
        },
        evidence,
        proposed_state,
    });
    if let Some(state) = conclusion {
        facts
            .conclude(
                function_entry_subject(&target),
                state,
                vec![index],
                "test entry",
            )
            .unwrap();
    }
}

#[test]
fn physical_bank_excludes_bss_and_keeps_seeds_distinct_from_authority() {
    let payload = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let rom = synthetic_rom(&[(0x40, &payload)]);
    let mut facts = FactDb::new();
    mapping(
        &mut facts,
        "overlay",
        ProofState::Proven,
        RomAddressSpace::Physical,
        0x40,
        0x48,
        0x8000_1000,
        0x8000_1010,
    );
    entry_claim(
        &mut facts,
        "overlay",
        0x8000_1004,
        FunctionEntryEvidence::DirectJal {
            call_site: BankAddr::new("overlay", 0x8000_1000),
        },
        ProofState::Candidate,
        None,
    );
    entry_claim(
        &mut facts,
        "overlay",
        0x8000_1000,
        FunctionEntryEvidence::Prologue {
            stack_adjust: BankAddr::new("overlay", 0x8000_1000),
            frame_size: 0x10,
            pattern: fn64_discover::facts::ProloguePattern::SavesReturnAddress,
            corroborating_site: BankAddr::new("overlay", 0x8000_1004),
        },
        ProofState::Proven,
        Some(ProofState::Proven),
    );
    entry_claim(
        &mut facts,
        "overlay",
        0x8000_1006,
        FunctionEntryEvidence::Prologue {
            stack_adjust: BankAddr::new("overlay", 0x8000_1006),
            frame_size: 0x10,
            pattern: fn64_discover::facts::ProloguePattern::SavesReturnAddress,
            corroborating_site: BankAddr::new("overlay", 0x8000_1004),
        },
        ProofState::Candidate,
        None,
    );
    entry_claim(
        &mut facts,
        "overlay",
        0x8000_100c,
        FunctionEntryEvidence::DirectJal {
            call_site: BankAddr::new("overlay", 0x8000_1004),
        },
        ProofState::Candidate,
        None,
    );

    let prepared = prepare_snapshot_banks(&rom, &facts).unwrap();
    let bank = &prepared.banks()[0];
    assert_eq!(bank.bytes, payload);
    assert_eq!(bank.va_end, 0x8000_1008, "load-time .bss must be excluded");
    assert_eq!(bank.traversal_seeds, [0x8000_1000, 0x8000_1004]);
    assert!(
        facts.conclusion("fn:overlay:0x80001004").is_none(),
        "a traversal seed must not acquire callable-entry authority"
    );
}

#[test]
fn supported_mapping_is_not_admitted() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    mapping(
        &mut facts,
        "candidate",
        ProofState::Supported,
        RomAddressSpace::Physical,
        0x40,
        0x44,
        0x8000_0000,
        0x8000_0004,
    );
    assert_eq!(
        prepare_snapshot_banks(&rom, &facts).unwrap_err(),
        PrepareSnapshotBanksError::NoProvenMappings
    );
}

#[test]
fn distinct_proven_geometries_are_ambiguous_but_exact_duplicates_collapse() {
    let rom = synthetic_rom(&[]);
    let mut duplicates = FactDb::new();
    for _ in 0..2 {
        mapping(
            &mut duplicates,
            "same",
            ProofState::Proven,
            RomAddressSpace::Physical,
            0x40,
            0x44,
            0x8000_0000,
            0x8000_0004,
        );
    }
    assert_eq!(
        prepare_snapshot_banks(&rom, &duplicates)
            .unwrap()
            .banks()
            .len(),
        1
    );

    mapping(
        &mut duplicates,
        "same",
        ProofState::Proven,
        RomAddressSpace::Physical,
        0x44,
        0x48,
        0x8000_0000,
        0x8000_0004,
    );
    assert_eq!(
        prepare_snapshot_banks(&rom, &duplicates).unwrap_err(),
        PrepareSnapshotBanksError::AmbiguousMapping {
            bank: "same".to_string(),
            distinct_geometries: 2,
        }
    );
}

#[test]
fn invalid_mapping_geometry_is_rejected_before_materialization() {
    let rom = synthetic_rom(&[]);
    let cases = [
        (
            (0x48, 0x40, 0x8000_0000, 0x8000_0008),
            PrepareSnapshotBanksError::InvertedInterval {
                bank: "bad".to_string(),
            },
        ),
        (
            (0x40, 0x40, 0x8000_0000, 0x8000_0000),
            PrepareSnapshotBanksError::EmptyRomInterval {
                bank: "bad".to_string(),
            },
        ),
        (
            (0x40, 0x48, 0x8000_0000, 0x8000_0004),
            PrepareSnapshotBanksError::RomExceedsVa {
                bank: "bad".to_string(),
                rom_extent: 8,
                va_extent: 4,
            },
        ),
        (
            (0x40, 0x48, 0xffff_fffc, 0xffff_ffff),
            PrepareSnapshotBanksError::VaPrefixOverflow {
                bank: "bad".to_string(),
            },
        ),
    ];
    for ((rom_start, rom_end, va_start, va_end), expected) in cases {
        let mut facts = FactDb::new();
        mapping(
            &mut facts,
            "bad",
            ProofState::Proven,
            RomAddressSpace::Physical,
            rom_start,
            rom_end,
            va_start,
            va_end,
        );
        assert_eq!(prepare_snapshot_banks(&rom, &facts).unwrap_err(), expected);
    }
}

#[test]
fn bank_and_seed_order_is_deterministic() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    for (bank, rom_start, rom_end, va) in [
        ("z", 0x50, 0x54, 0x8000_1000),
        ("a", 0x40, 0x4c, 0x8000_0000),
    ] {
        mapping(
            &mut facts,
            bank,
            ProofState::Proven,
            RomAddressSpace::Physical,
            rom_start,
            rom_end,
            va,
            va + (rom_end - rom_start),
        );
    }
    for pc in [0x8000_0008, 0x8000_0000, 0x8000_0004] {
        entry_claim(
            &mut facts,
            "a",
            pc,
            FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("a", 0x8000_0000),
            },
            ProofState::Candidate,
            None,
        );
    }

    let first = prepare_snapshot_banks(&rom, &facts).unwrap();
    let second = prepare_snapshot_banks(&rom, &facts).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first
            .banks()
            .iter()
            .map(|bank| bank.bank.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert_eq!(
        first.banks()[0].traversal_seeds,
        [0x8000_0000, 0x8000_0004, 0x8000_0008]
    );
}

#[test]
fn unaligned_traversal_claim_is_rejected_before_composition() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    mapping(
        &mut facts,
        "bad_seed",
        ProofState::Proven,
        RomAddressSpace::Physical,
        0x40,
        0x44,
        0x8000_0000,
        0x8000_0004,
    );
    entry_claim(
        &mut facts,
        "bad_seed",
        0x8000_0002,
        FunctionEntryEvidence::DirectJal {
            call_site: BankAddr::new("bad_seed", 0x8000_0000),
        },
        ProofState::Candidate,
        None,
    );

    assert_eq!(
        prepare_snapshot_banks(&rom, &facts).unwrap_err(),
        PrepareSnapshotBanksError::UnalignedTraversalSeed {
            bank: "bad_seed".to_string(),
            pc: 0x8000_0002,
        }
    );
}

#[test]
fn unaligned_bank_geometry_is_rejected_before_composition() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    mapping(
        &mut facts,
        "bad_bank",
        ProofState::Proven,
        RomAddressSpace::Physical,
        0x40,
        0x46,
        0x8000_0000,
        0x8000_0006,
    );
    assert_eq!(
        prepare_snapshot_banks(&rom, &facts).unwrap_err(),
        PrepareSnapshotBanksError::UnalignedBank {
            bank: "bad_bank".to_string(),
            va_start: 0x8000_0000,
            rom_extent: 6,
        }
    );
}

#[test]
fn vrom_yaz0_bank_materializes_through_one_proven_file_record() {
    let decoded = [0xde, 0xad, 0xbe, 0xef, 0x12, 0x34, 0x56, 0x78];
    let mut yaz0 = Vec::new();
    yaz0.extend_from_slice(b"Yaz0");
    yaz0.extend_from_slice(&(decoded.len() as u32).to_be_bytes());
    yaz0.extend_from_slice(&[0; 8]);
    yaz0.push(0xff);
    yaz0.extend_from_slice(&decoded);
    let rom = synthetic_rom(&[(0x100, &yaz0)]);
    let mut facts = FactDb::new();
    let backing = facts.insert(Fact::LoadImageTableRecord {
        table: "files".to_string(),
        bank: None,
        table_space: RomAddressSpace::Physical,
        table_offset: 0x80,
        index: 0,
        source_space: MappingAddressSpace::VirtualRom,
        source_start: 0x1000,
        source_end: 0x1008,
        destination_space: MappingAddressSpace::PhysicalRom,
        destination_start: 0x100,
        destination_end: 0x100 + yaz0.len() as u32,
    });
    facts
        .conclude(
            load_image_table_record_subject("files", 0),
            ProofState::Proven,
            vec![backing],
            "test file record",
        )
        .unwrap();
    mapping(
        &mut facts,
        "compressed",
        ProofState::Proven,
        RomAddressSpace::Virtual,
        0x1000,
        0x1008,
        0x8080_0000,
        0x8080_0008,
    );

    let prepared = prepare_snapshot_banks(&rom, &facts).unwrap();
    assert_eq!(prepared.banks()[0].bytes, decoded);
    assert_eq!(prepared.banks()[0].backing_evidence, [backing]);
}

#[test]
fn conflicted_strong_claim_is_not_a_traversal_seed() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    mapping(
        &mut facts,
        "bank",
        ProofState::Proven,
        RomAddressSpace::Physical,
        0x40,
        0x48,
        0x8000_0000,
        0x8000_0008,
    );
    let target = BankAddr::new("bank", 0x8000_0004);
    let claim = facts.insert(Fact::FunctionEntryClaim {
        target: target.clone(),
        detector: CandidateDetector::JalTarget,
        evidence: FunctionEntryEvidence::DirectJal {
            call_site: BankAddr::new("bank", 0x8000_0000),
        },
        proposed_state: ProofState::Candidate,
    });
    facts
        .conclude(
            function_entry_subject(&target),
            ProofState::Conflict,
            vec![claim],
            "test conflict",
        )
        .unwrap();
    assert!(prepare_snapshot_banks(&rom, &facts).unwrap().banks()[0]
        .traversal_seeds
        .is_empty());
}

#[test]
fn oversized_declared_yaz0_is_rejected_before_decode_allocation() {
    let decoded_len = 16 * 1024 * 1024u32;
    let mut yaz0 = Vec::new();
    yaz0.extend_from_slice(b"Yaz0");
    yaz0.extend_from_slice(&decoded_len.to_be_bytes());
    yaz0.extend_from_slice(&[0; 8]);
    let rom = synthetic_rom(&[(0x100, &yaz0)]);
    let mut facts = FactDb::new();
    let backing = facts.insert(Fact::LoadImageTableRecord {
        table: "large-file".to_string(),
        bank: None,
        table_space: RomAddressSpace::Physical,
        table_offset: 0x80,
        index: 0,
        source_space: MappingAddressSpace::VirtualRom,
        source_start: 0x1000,
        source_end: 0x1000 + decoded_len,
        destination_space: MappingAddressSpace::PhysicalRom,
        destination_start: 0x100,
        destination_end: 0x110,
    });
    facts
        .conclude(
            load_image_table_record_subject("large-file", 0),
            ProofState::Proven,
            vec![backing],
            "test file record",
        )
        .unwrap();
    mapping(
        &mut facts,
        "slice",
        ProofState::Proven,
        RomAddressSpace::Virtual,
        0x1000,
        0x1004,
        0x8080_0000,
        0x8080_0004,
    );
    let error = prepare_snapshot_banks_with_limits(
        &rom,
        &facts,
        PrepareSnapshotBanksLimits {
            max_banks: 1,
            max_aggregate_rom_bytes: 4,
            max_decoded_vrom_file_bytes: 1024,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        PrepareSnapshotBanksError::Materialization { reason, .. }
            if reason.contains("decoded VROM file length") && reason.contains("transient limit")
    ));
}

#[test]
fn preparation_bank_and_aggregate_limits_fail_before_materialization() {
    let rom = synthetic_rom(&[]);
    let mut facts = FactDb::new();
    for (bank, start, va) in [("a", 0x40, 0x8000_0000), ("b", 0x48, 0x8000_1000)] {
        mapping(
            &mut facts,
            bank,
            ProofState::Proven,
            RomAddressSpace::Physical,
            start,
            start + 8,
            va,
            va + 8,
        );
    }
    assert!(matches!(
        prepare_snapshot_banks_with_limits(
            &rom,
            &facts,
            PrepareSnapshotBanksLimits {
                max_banks: 1,
                max_aggregate_rom_bytes: u64::MAX,
                max_decoded_vrom_file_bytes: usize::MAX,
            }
        ),
        Err(PrepareSnapshotBanksError::BankLimitExceeded { banks: 2, limit: 1 })
    ));
    assert!(matches!(
        prepare_snapshot_banks_with_limits(
            &rom,
            &facts,
            PrepareSnapshotBanksLimits {
                max_banks: 2,
                max_aggregate_rom_bytes: 12,
                max_decoded_vrom_file_bytes: usize::MAX,
            }
        ),
        Err(PrepareSnapshotBanksError::AggregateRomBytesLimitExceeded {
            bytes: 16,
            limit: 12
        })
    ));
}
