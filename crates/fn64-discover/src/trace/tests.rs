use super::*;
use std::io::Cursor;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn digest() -> NormalizedRomDigest {
    NormalizedRomDigest::try_from(DIGEST.to_string()).unwrap()
}

fn ingest(lines: &[&str]) -> Result<IngestReport, TraceIngestError> {
    ingest_jsonl(Cursor::new(lines.join("\n")), &digest())
}

fn header() -> &'static str {
    r#"{"event":"header","sequence":0,"schema_version":1,"normalized_rom_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","trace_id":"test-1","producer":"synthetic-test"}"#
}

fn executable_image_json(sha256: &str, words: &str) -> String {
    format!(
        r#"{{"schema":"fn64.executable-image.v1","producer":"public-debugger-test","normalized_rom_sha256":"{DIGEST}","image_id":"general-exception-preamble","lineage":"cpu_produced","generation":0,"capture_pc":2147484032,"first_executed_pc":2147484032,"retired_instructions":7,"va_start":2147484032,"byte_len":16,"sha256":"{sha256}","words":[{words}]}}"#
    )
}

#[test]
fn executable_image_capture_validates_identity_geometry_and_content() {
    let json = executable_image_json(
        "92d005d9f1c311068500142b0129d6160dd193f92baa1e0f84061a169b48b982",
        "1008369667,660236176,54525960,0",
    );
    let capture = parse_executable_image_capture(json.as_bytes(), &digest()).unwrap();
    assert_eq!(capture.lineage, ExecutableImageLineage::CpuProduced);
    assert_eq!(capture.va_start, 0x8000_0180);
    assert_eq!(capture.words, [0x3c1a_8003, 0x275a_6790, 0x0340_0008, 0]);
}

#[test]
fn executable_image_capture_rejects_content_not_bound_by_its_digest() {
    let json = executable_image_json(
        "92d005d9f1c311068500142b0129d6160dd193f92baa1e0f84061a169b48b982",
        "1008369667,660236176,54525960,1",
    );
    assert!(matches!(
        parse_executable_image_capture(json.as_bytes(), &digest()),
        Err(ExecutableImageCaptureError::ContentDigestMismatch { .. })
    ));
}

#[test]
fn reproducible_image_group_binds_identity_and_bytes_not_route_length() {
    let json = executable_image_json(
        "92d005d9f1c311068500142b0129d6160dd193f92baa1e0f84061a169b48b982",
        "1008369667,660236176,54525960,0",
    );
    let later_route = json.replace("\"retired_instructions\":7", "\"retired_instructions\":8");
    let documents = vec![
        json.as_bytes().to_vec(),
        later_route.into_bytes(),
        json.into_bytes(),
    ];
    let capture = parse_reproducible_executable_image_group(&documents, &digest(), 3).unwrap();
    assert_eq!(capture.va_start, 0x8000_0180);

    let mut mismatched = documents;
    mismatched[2] = String::from_utf8(mismatched[2].clone())
        .unwrap()
        .replace("public-debugger-test", "different-producer")
        .into_bytes();
    assert!(matches!(
        parse_reproducible_executable_image_group(&mismatched, &digest(), 3),
        Err(ReproducibleExecutableImageError::ObservationMismatch { index: 2 })
    ));
}

#[test]
fn ingests_all_event_classes_without_inventing_unknown_banks() {
    let report = ingest(&[
        header(),
        r#"{"event":"pi_dma","sequence":1,"direction":"cart_to_rdram","cart_address":268439552,"dram_address":1024,"byte_len":64,"active_bank":{"status":"unknown"}}"#,
        r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}}}"#,
        r#"{"event":"indirect_transfer","sequence":3,"kind":"call","site":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}},"target":{"address":2148532224,"bank":{"status":"unknown"}}}"#,
        r#"{"event":"watched_table_write","sequence":4,"watch_id":"overlay-slots","address":2149580800,"width":"u32","value":2148532224,"active_bank":{"status":"known","bank":"loader","activation":7}}"#,
        r#"{"event":"end","sequence":5,"completion":"completed","exhaustiveness":[{"domain":"pi_dma","first_sequence":1,"last_sequence":4},{"domain":"indirect_transfer","first_sequence":1,"last_sequence":4}]}"#,
    ])
    .unwrap();

    assert_eq!(report.final_sequence, 5);
    assert_eq!(report.facts.len(), 4);
    assert_eq!(report.counts.pi_dma, 1);
    assert_eq!(report.counts.executed_pc, 1);
    assert_eq!(report.counts.indirect_transfer, 1);
    assert_eq!(report.counts.watched_table_write, 1);
    assert_eq!(report.observations_with_unknown_bank, 2);
    assert!(matches!(
        &report.facts[0],
        ObservedTraceFact::PiDma {
            active_bank: BankContext::Unknown,
            ..
        }
    ));
}

#[test]
fn rejects_digest_mismatch_before_events() {
    let other = NormalizedRomDigest::try_from("f".repeat(64)).unwrap();
    let error = ingest_jsonl(Cursor::new(header()), &other).unwrap_err();
    assert!(error.message.contains("digest does not match"));
}

#[test]
fn rejects_sequence_gaps_and_records_after_end() {
    let gap = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
    ])
    .unwrap_err();
    assert!(gap.message.contains("expected sequence 1"));

    let after_end = ingest(&[
        header(),
        r#"{"event":"end","sequence":1,"completion":"completed","exhaustiveness":[]}"#,
        r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
    ])
    .unwrap_err();
    assert!(after_end.message.contains("after end"));
}

#[test]
fn validates_instruction_dma_and_table_write_ranges() {
    let unaligned_pc = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484673,"bank":{"status":"unknown"}}}"#,
    ])
    .unwrap_err();
    assert!(unaligned_pc.message.contains("four-byte aligned"));

    let dma_overrun = ingest(&[
        header(),
        r#"{"event":"pi_dma","sequence":1,"direction":"cart_to_rdram","cart_address":268435456,"dram_address":16777200,"byte_len":32,"active_bank":{"status":"unknown"}}"#,
    ])
    .unwrap_err();
    assert!(dma_overrun.message.contains("24-bit PI DRAM"));

    let unaligned_write = ingest(&[
        header(),
        r#"{"event":"watched_table_write","sequence":1,"watch_id":"table","address":4098,"width":"u32","value":1,"active_bank":{"status":"unknown"}}"#,
    ])
    .unwrap_err();
    assert!(unaligned_write.message.contains("not 4-byte aligned"));

    let wide_value = ingest(&[
        header(),
        r#"{"event":"watched_table_write","sequence":1,"watch_id":"table","address":4096,"width":"u8","value":256,"active_bank":{"status":"unknown"}}"#,
    ])
    .unwrap_err();
    assert!(wide_value.message.contains("does not fit"));
}

#[test]
fn exhaustive_claims_require_completed_bounded_intervals() {
    let aborted = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
        r#"{"event":"end","sequence":2,"completion":"aborted","exhaustiveness":[{"domain":"executed_pc","first_sequence":1,"last_sequence":1}]}"#,
    ])
    .unwrap_err();
    assert!(aborted.message.contains("aborted trace"));

    let outside = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
        r#"{"event":"end","sequence":2,"completion":"completed","exhaustiveness":[{"domain":"executed_pc","first_sequence":1,"last_sequence":2}]}"#,
    ])
    .unwrap_err();
    assert!(outside.message.contains("invalid exhaustiveness interval"));
}

#[test]
fn requires_footer_and_valid_nonempty_known_bank() {
    let missing_end = ingest(&[header()]).unwrap_err();
    assert!(missing_end.message.contains("missing end"));

    let empty_bank = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"known","bank":" ","activation":1}}}"#,
    ])
    .unwrap_err();
    assert!(empty_bank.message.contains("bank must not be empty"));
}

#[test]
fn report_serialization_is_deterministic() {
    let input = [
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"unknown"}}}"#,
        r#"{"event":"end","sequence":2,"completion":"completed","exhaustiveness":[]}"#,
    ];
    let first = serde_json::to_string(&ingest(&input).unwrap()).unwrap();
    let second = serde_json::to_string(&ingest(&input).unwrap()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn execution_roots_preserve_exact_bank_generation() {
    let report = ingest(&[
        header(),
        r#"{"event":"executed_pc","sequence":1,"pc":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}}}"#,
        r#"{"event":"executed_pc","sequence":2,"pc":{"address":2147484672,"bank":{"status":"known","bank":"boot","activation":0}}}"#,
        r#"{"event":"executed_pc","sequence":3,"pc":{"address":2147484676,"bank":{"status":"known","bank":"boot","activation":1}}}"#,
        r#"{"event":"executed_pc","sequence":4,"pc":{"address":2147484680,"bank":{"status":"known","bank":"overlay","activation":0}}}"#,
        r#"{"event":"executed_pc","sequence":5,"pc":{"address":2147484684,"bank":{"status":"unknown"}}}"#,
        r#"{"event":"end","sequence":6,"completion":"completed","exhaustiveness":[]}"#,
    ])
    .unwrap();

    assert_eq!(
        observed_execution_roots(&report, "boot", 0),
        BTreeSet::from([0x8000_0400])
    );
    assert_eq!(
        observed_execution_roots(&report, "boot", 1),
        BTreeSet::from([0x8000_0404])
    );
}

#[test]
fn digest_rejects_ambiguous_encodings() {
    assert!(NormalizedRomDigest::try_from("0".repeat(63)).is_err());
    assert!(NormalizedRomDigest::try_from("A".repeat(64)).is_err());
    assert!(NormalizedRomDigest::try_from("g".repeat(64)).is_err());
}

mod fold_into_fact_db {
    use super::*;
    use crate::cfg::WordClass;
    use crate::facts::{observed_executed_code_subject, BankAddr, Fact, FactDb, ProofState};

    fn known_pc(bank: &str, address: u32, activation: u64) -> ObservedAddress {
        ObservedAddress {
            address,
            bank: BankContext::Known {
                bank: bank.to_string(),
                activation,
            },
        }
    }

    fn unknown_pc(address: u32) -> ObservedAddress {
        ObservedAddress {
            address,
            bank: BankContext::Unknown,
        }
    }

    fn no_static_class(_bank: &str, _va: u32) -> Option<WordClass> {
        None
    }

    #[test]
    fn known_bank_observation_adds_a_code_existence_fact() {
        let mut db = FactDb::new();
        let facts = vec![ObservedTraceFact::ExecutedPc {
            sequence: 1,
            pc: known_pc("boot", 0x8000_0400, 0),
        }];

        let report =
            fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

        assert_eq!(report.facts_added, 1);
        let site = BankAddr::new("boot", 0x8000_0400);
        assert_eq!(report.new_code_existence, [site.clone()].into());
        assert!(report.corroborated.is_empty());
        assert!(report.conflicts.is_empty());
        assert_eq!(report.unknown_bank_skipped, 0);

        assert_eq!(db.facts().len(), 1);
        assert!(matches!(
            &db.facts()[0],
            Fact::ObservedExecutedCode { site: s, trace, sequence: 1 }
                if *s == site && trace == "trace-a"
        ));
        let conclusion = db
            .conclusion(&observed_executed_code_subject("boot", 0x8000_0400))
            .unwrap();
        assert_eq!(conclusion.state, ProofState::Supported);
        assert_eq!(conclusion.justified_by, vec![0]);
    }

    #[test]
    fn repeated_observation_of_the_same_pc_adds_one_conclusion_two_provenance_records() {
        let mut db = FactDb::new();
        let facts = vec![
            ObservedTraceFact::ExecutedPc {
                sequence: 1,
                pc: known_pc("boot", 0x8000_0400, 0),
            },
            ObservedTraceFact::ExecutedPc {
                sequence: 2,
                pc: known_pc("boot", 0x8000_0400, 0),
            },
        ];

        let report =
            fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

        assert_eq!(report.facts_added, 2);
        let site = BankAddr::new("boot", 0x8000_0400);
        // First sighting is new evidence; the second is a corroboration
        // of that same word, not a second new-evidence word.
        assert_eq!(report.new_code_existence, [site.clone()].into());
        assert_eq!(report.corroborated, [site.clone()].into());

        // Two distinct Fact::ObservedExecutedCode records (provenance)...
        assert_eq!(db.facts().len(), 2);
        let sequences: Vec<u64> = db
            .facts()
            .iter()
            .map(|f| match f {
                Fact::ObservedExecutedCode { sequence, .. } => *sequence,
                _ => panic!("unexpected fact variant"),
            })
            .collect();
        assert_eq!(sequences, vec![1, 2]);

        // ...but exactly one conclusion for the (bank, pc) subject, whose
        // justified_by names both provenance facts.
        assert_eq!(db.conclusions().count(), 1);
        let conclusion = db
            .conclusion(&observed_executed_code_subject("boot", 0x8000_0400))
            .unwrap();
        assert_eq!(conclusion.justified_by, vec![0, 1]);
    }

    #[test]
    fn observed_executed_word_that_is_statically_proven_data_raises_a_conflict() {
        let mut db = FactDb::new();
        let facts = vec![ObservedTraceFact::ExecutedPc {
            sequence: 7,
            pc: known_pc("boot", 0x8000_0800, 0),
        }];
        let static_class = |bank: &str, va: u32| -> Option<WordClass> {
            (bank == "boot" && va == 0x8000_0800).then_some(WordClass::ProvenData)
        };

        let report = fold_executed_pcs_into_fact_db(&mut db, "trace-b", &facts, static_class);

        assert_eq!(report.facts_added, 1);
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].site, BankAddr::new("boot", 0x8000_0800));
        assert_eq!(report.conflicts[0].trace_id, "trace-b");
        assert_eq!(report.conflicts[0].sequence, 7);
        // The conflict is surfaced, not resolved: the observation still
        // becomes Supported evidence rather than being dropped, and
        // nothing here mutates or removes the static ProvenData claim
        // (which this test's closure stands in for -- it is never
        // touched by the adapter at all).
        let conclusion = db
            .conclusion(&observed_executed_code_subject("boot", 0x8000_0800))
            .unwrap();
        assert_eq!(conclusion.state, ProofState::Supported);
    }

    #[test]
    fn unknown_bank_observation_adds_nothing() {
        let mut db = FactDb::new();
        let facts = vec![ObservedTraceFact::ExecutedPc {
            sequence: 1,
            pc: unknown_pc(0x8000_0400),
        }];

        let report =
            fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

        assert_eq!(report.facts_added, 0);
        assert_eq!(report.unknown_bank_skipped, 1);
        assert!(report.new_code_existence.is_empty());
        assert!(report.corroborated.is_empty());
        assert!(report.conflicts.is_empty());
        assert!(db.facts().is_empty());
        assert_eq!(db.conclusions().count(), 0);
    }

    #[test]
    fn non_executed_pc_facts_are_left_untouched() {
        let mut db = FactDb::new();
        let facts = vec![ObservedTraceFact::PiDma {
            sequence: 1,
            direction: PiDmaDirection::CartToRdram,
            cart_address: 0x1000_0000,
            dram_address: 0x400,
            byte_len: 64,
            active_bank: BankContext::Unknown,
        }];

        let report =
            fold_executed_pcs_into_fact_db(&mut db, "trace-a", &facts, no_static_class);

        assert_eq!(report.facts_added, 0);
        assert_eq!(report.unknown_bank_skipped, 0);
        assert!(db.facts().is_empty());
    }
}

mod pi_dma_fold {
    use super::*;
    use crate::facts::{Fact, FactDb, ProofState, RomAddressSpace};

    /// `rom_offset` is a ROM file offset; the observation carries the
    /// cart-BUS address the hardware saw, so it is offset into PI cart
    /// domain 1. Keeping the test inputs in bus space is the point -- the
    /// translation is what a live capture proved was missing.
    fn dma(sequence: u64, rom_offset: u32, dram: u32, len: u32) -> ObservedTraceFact {
        ObservedTraceFact::PiDma {
            sequence,
            direction: PiDmaDirection::CartToRdram,
            cart_address: PI_CART_DOMAIN1_BASE + rom_offset,
            dram_address: dram,
            byte_len: len,
            active_bank: BankContext::Unknown,
        }
    }

    fn prove_mapping(db: &mut FactDb, bank: &str, rom_start: u32, va_start: u32, len: u32) {
        let mapping = db.insert(Fact::RomMapping {
            bank: bank.to_string(),
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end: rom_start + len,
            va_start,
            va_end: va_start + len,
        });
        db.conclude(
            format!("bank:{bank}"),
            ProofState::Proven,
            vec![mapping],
            "test",
        )
        .unwrap();
    }

    #[test]
    fn a_cart_to_rdram_transfer_becomes_a_supported_mapping() {
        let mut db = FactDb::new();
        let report =
            fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(1, 0x10_0000, 0x40_0000, 0x2000)]);

        assert_eq!(report.facts_added, 1);
        assert_eq!(report.new_mappings.len(), 1);
        let bank = report.new_mappings.iter().next().unwrap();
        let conclusion = db.conclusion(&format!("bank:{bank}")).unwrap();
        // The whole point of the evidence class: observed, never proven.
        assert_eq!(conclusion.state, ProofState::Supported);
        assert!(
            db.proven_rom_mappings().is_empty(),
            "an observation must not appear as a proven mapping"
        );
        let mapping = db
            .facts()
            .iter()
            .find_map(|fact| match fact {
                Fact::RomMapping {
                    va_start,
                    rom_start,
                    ..
                } => Some((*rom_start, *va_start)),
                _ => None,
            })
            .unwrap();
        assert_eq!(mapping, (0x10_0000, 0x8040_0000), "KSEG0 destination");
    }

    #[test]
    fn a_reloaded_overlay_concludes_once_and_counts_the_repeat() {
        // Games reload the same overlay constantly. That is one mapping and
        // N sightings, not N mappings.
        let mut db = FactDb::new();
        let facts = vec![
            dma(1, 0x10_0000, 0x40_0000, 0x2000),
            dma(9, 0x10_0000, 0x40_0000, 0x2000),
            dma(17, 0x10_0000, 0x40_0000, 0x2000),
        ];
        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(report.facts_added, 1);
        assert_eq!(report.repeated, 2);
        assert_eq!(report.new_mappings.len(), 1);
    }

    #[test]
    fn an_observation_matching_a_proven_mapping_corroborates_instead_of_duplicating() {
        // This is the case worth having: an independent producer agreeing
        // with static composition is corroboration static analysis cannot
        // give itself.
        let mut db = FactDb::new();
        prove_mapping(&mut db, "code", 0x10_0000, 0x8040_0000, 0x2000);

        let report =
            fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(1, 0x10_0000, 0x40_0000, 0x2000)]);

        assert_eq!(report.facts_added, 0, "no duplicate mapping was added");
        assert!(report.new_mappings.is_empty());
        assert_eq!(
            report.corroborated.iter().cloned().collect::<Vec<_>>(),
            vec!["code".to_string()]
        );
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn an_observation_contradicting_a_proven_mapping_is_reported_not_resolved() {
        let mut db = FactDb::new();
        prove_mapping(&mut db, "code", 0x10_0000, 0x8040_0000, 0x2000);

        // Same VA, different ROM source: one of the two is wrong.
        let report =
            fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(4, 0x99_0000, 0x40_0000, 0x2000)]);

        assert_eq!(report.conflicts.len(), 1);
        let conflict = &report.conflicts[0];
        assert_eq!(conflict.va_start, 0x8040_0000);
        assert_eq!(conflict.observed_rom_start, 0x99_0000);
        assert_eq!(conflict.proven_rom_start, 0x10_0000);
        assert_eq!(conflict.proven_bank, "code");
        assert_eq!(
            report.facts_added, 0,
            "a conflict must not be silently admitted"
        );
    }

    #[test]
    fn adjacent_chunks_of_one_streamed_load_coalesce_into_one_image() {
        // Measured shape: SM64 streams a file as sequential 4 KiB chunks at
        // rising ROM offsets mapping to rising VAs. One bank per chunk would
        // describe the transport, not the program.
        let mut db = FactDb::new();
        let facts: Vec<_> = (0..4)
            .map(|i| {
                dma(
                    i + 1,
                    0xf5580 + i as u32 * 0x1000,
                    0x378800 + i as u32 * 0x1000,
                    0x1000,
                )
            })
            .collect();
        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(report.facts_added, 1, "four chunks are one load image");
        assert_eq!(report.coalesced_transfers, 3);
        let mapping = db
            .facts()
            .iter()
            .find_map(|fact| match fact {
                Fact::RomMapping {
                    rom_start,
                    rom_end,
                    va_start,
                    va_end,
                    ..
                } => Some((*rom_start, *rom_end, *va_start, *va_end)),
                _ => None,
            })
            .unwrap();
        assert_eq!(mapping, (0xf5580, 0xf9580, 0x8037_8800, 0x8037_c800));
    }

    #[test]
    fn a_destination_written_from_many_sources_is_a_buffer_not_a_load_image() {
        // Measured on WCW vs. nWo World Tour: 4,411 transfers, of which one
        // destination alone was written from 2,598 distinct ROM sources.
        // Concluding a mapping per transfer would assert thousands of
        // mutually contradictory backings for a single address.
        let mut db = FactDb::new();
        let mut facts: Vec<_> = (0..8)
            .map(|i| dma(i + 1, 0x20_0000 + i as u32 * 0x4000, 0x50_0000, 0x800))
            .collect();
        // One genuine load image alongside the streaming, to prove the rule
        // excludes the buffer without discarding real evidence.
        facts.push(dma(99, 0x10_0000, 0x40_0000, 0x2000));

        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(report.facts_added, 1, "only the single-source image");
        assert_eq!(report.reused_destination_skipped, 8);
        assert_eq!(
            report
                .reused_destinations
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0x8050_0000]
        );
    }

    #[test]
    fn chunks_adjacent_in_only_one_space_do_not_merge() {
        // The condition is adjacency in ROM *and* VA. Two files that happen
        // to sit next to each other in ROM but load to unrelated addresses
        // are two images, and merging them would invent a region that was
        // never transferred.
        let mut db = FactDb::new();
        let facts = vec![
            dma(1, 0x10_0000, 0x40_0000, 0x1000),
            // Contiguous in ROM, but a different load address.
            dma(2, 0x10_1000, 0x60_0000, 0x1000),
            // Contiguous in VA with the first, but from elsewhere in ROM.
            dma(3, 0x90_0000, 0x40_1000, 0x1000),
        ];
        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(report.coalesced_transfers, 0);
        assert_eq!(report.facts_added, 3, "three distinct load images");
    }

    #[test]
    fn coalescing_is_order_independent() {
        // Chunks need not be observed in address order; the result must not
        // depend on the order the emulator happened to emit them.
        let ordered: Vec<_> = (0..4)
            .map(|i| {
                dma(
                    i + 1,
                    0x2000 + i as u32 * 0x800,
                    0x1_0000 + i as u32 * 0x800,
                    0x800,
                )
            })
            .collect();
        let mut shuffled = ordered.clone();
        shuffled.swap(0, 3);
        shuffled.swap(1, 2);

        let mut a = FactDb::new();
        let mut b = FactDb::new();
        let ra = fold_pi_dmas_into_fact_db(&mut a, "t", &ordered);
        let rb = fold_pi_dmas_into_fact_db(&mut b, "t", &shuffled);

        assert_eq!(ra.facts_added, 1);
        assert_eq!(ra.new_mappings, rb.new_mappings);
        assert_eq!(ra.coalesced_transfers, rb.coalesced_transfers);
    }

    #[test]
    fn a_transfer_from_another_pi_device_is_counted_not_mistranslated() {
        // 0x0500_0000 is the 64DD, 0x1FC0_0000 is PIF ROM. Neither is this
        // cartridge, so subtracting the domain-1 base would invent a ROM
        // offset for bytes that never came from the ROM.
        let mut db = FactDb::new();
        let facts = vec![
            ObservedTraceFact::PiDma {
                sequence: 1,
                direction: PiDmaDirection::CartToRdram,
                cart_address: 0x0500_0000,
                dram_address: 0x40_0000,
                byte_len: 0x1000,
                active_bank: BankContext::Unknown,
            },
            ObservedTraceFact::PiDma {
                sequence: 2,
                direction: PiDmaDirection::CartToRdram,
                cart_address: 0x1FC0_0000,
                dram_address: 0x40_0000,
                byte_len: 0x1000,
                active_bank: BankContext::Unknown,
            },
        ];
        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(report.off_cartridge_skipped, 2);
        assert_eq!(report.facts_added, 0);
    }

    #[test]
    fn the_cart_bus_address_is_translated_to_a_rom_offset() {
        // The live-capture bug: the IPL3 boot copy reads cart 0x10001000,
        // which is ROM offset 0x1000. Folding the bus address verbatim put
        // every mapping outside the image and made corroboration with the
        // proven boot bank impossible.
        let mut db = FactDb::new();
        prove_mapping(&mut db, "boot", 0x1000, 0x8000_0400, 0x10_0000);

        let report =
            fold_pi_dmas_into_fact_db(&mut db, "t", &[dma(1, 0x1000, 0x400, 0x10_0000)]);

        assert!(
            report.conflicts.is_empty(),
            "the boot copy must corroborate, not conflict: {:?}",
            report.conflicts
        );
        assert_eq!(
            report.corroborated.iter().cloned().collect::<Vec<_>>(),
            vec!["boot".to_string()]
        );
    }

    #[test]
    fn write_back_and_degenerate_transfers_are_skipped_and_counted() {
        let mut db = FactDb::new();
        let facts = vec![
            ObservedTraceFact::PiDma {
                sequence: 1,
                direction: PiDmaDirection::RdramToCart,
                cart_address: 0x10_0000,
                dram_address: 0x40_0000,
                byte_len: 0x2000,
                active_bank: BankContext::Unknown,
            },
            dma(2, 0x10_0000, 0x40_0000, 0),
            // Already at the top of the address space: end overflows u32.
            ObservedTraceFact::PiDma {
                sequence: 3,
                direction: PiDmaDirection::CartToRdram,
                cart_address: 0x1FBF_FFFF,
                dram_address: 0x40_0000,
                byte_len: 0xFFFF_FFFF,
                active_bank: BankContext::Unknown,
            },
        ];
        let report = fold_pi_dmas_into_fact_db(&mut db, "t", &facts);

        assert_eq!(
            report.non_load_skipped, 1,
            "a write-back is not a load image"
        );
        assert_eq!(report.degenerate_skipped, 2);
        assert_eq!(report.facts_added, 0);
    }
}

mod indirect_fold {
    use super::*;
    use crate::cfg::WordClass;
    use crate::facts::{observed_indirect_target_subject, BankAddr, Fact, FactDb, ProofState};

    fn known(bank: &str, address: u32) -> ObservedAddress {
        ObservedAddress {
            address,
            bank: BankContext::Known {
                bank: bank.to_string(),
                activation: 0,
            },
        }
    }

    fn unknown(address: u32) -> ObservedAddress {
        ObservedAddress {
            address,
            bank: BankContext::Unknown,
        }
    }

    fn edge(site: ObservedAddress, target: ObservedAddress) -> ObservedTraceFact {
        ObservedTraceFact::IndirectTransfer {
            sequence: 1,
            kind: IndirectTransferKind::Call,
            site,
            target,
        }
    }

    fn no_class(_bank: &str, _va: u32) -> Option<WordClass> {
        None
    }

    #[test]
    fn known_edge_becomes_a_supported_observed_indirect_target() {
        let mut db = FactDb::new();
        let facts = vec![edge(known("code", 0x8019_3efc), known("code", 0x8019_4000))];
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);

        assert_eq!(report.facts_added, 1);
        let site = BankAddr::new("code", 0x8019_3efc);
        let target = BankAddr::new("code", 0x8019_4000);
        assert_eq!(report.new_edges, [(site.clone(), target.clone())].into());
        assert!(report.corroborated.is_empty());
        assert!(report.target_conflicts.is_empty());
        assert_eq!(report.unknown_bank_skipped, 0);
        assert!(matches!(
            &db.facts()[0],
            Fact::ObservedIndirectTarget { site: s, target: t, trace }
                if *s == site && *t == target && trace == "trace-a"
        ));
        let subject =
            observed_indirect_target_subject("code", 0x8019_3efc, "code", 0x8019_4000);
        assert_eq!(
            db.conclusion(&subject).unwrap().state,
            ProofState::Supported
        );
    }

    #[test]
    fn same_edge_twice_corroborates_one_conclusion_with_two_facts() {
        let mut db = FactDb::new();
        let facts = vec![
            edge(known("code", 0x100), known("code", 0x200)),
            edge(known("code", 0x100), known("code", 0x200)),
        ];
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);

        assert_eq!(report.facts_added, 2);
        assert_eq!(report.new_edges.len(), 1);
        assert_eq!(report.corroborated.len(), 1);
        let subject = observed_indirect_target_subject("code", 0x100, "code", 0x200);
        assert_eq!(db.conclusion(&subject).unwrap().justified_by, vec![0, 1]);
    }

    #[test]
    fn one_site_two_targets_are_two_distinct_edges() {
        let mut db = FactDb::new();
        let facts = vec![
            edge(known("code", 0x100), known("code", 0x200)),
            edge(known("code", 0x100), known("code", 0x300)),
        ];
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
        // Existence, not exhaustiveness: both edges kept, never merged.
        assert_eq!(report.new_edges.len(), 2);
        assert_eq!(report.facts_added, 2);
    }

    #[test]
    fn unknown_target_bank_is_skipped_not_invented() {
        let mut db = FactDb::new();
        let facts = vec![edge(known("code", 0x100), unknown(0x8020_0000))];
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
        assert_eq!(report.facts_added, 0);
        assert_eq!(report.unknown_bank_skipped, 1);
        assert!(db.facts().is_empty());
    }

    #[test]
    fn unknown_site_bank_is_also_skipped() {
        let mut db = FactDb::new();
        let facts = vec![edge(unknown(0x100), known("code", 0x200))];
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, no_class);
        assert_eq!(report.facts_added, 0);
        assert_eq!(report.unknown_bank_skipped, 1);
    }

    #[test]
    fn target_on_proven_data_is_reported_as_conflict() {
        let mut db = FactDb::new();
        let facts = vec![edge(known("code", 0x100), known("code", 0x200))];
        let class = |bank: &str, va: u32| {
            (bank == "code" && va == 0x200).then_some(WordClass::ProvenData)
        };
        let report = fold_indirect_targets_into_fact_db(&mut db, "trace-a", &facts, class);
        // The edge is still recorded (the observation happened); the
        // conflict is surfaced, never silently resolved.
        assert_eq!(report.facts_added, 1);
        assert_eq!(report.target_conflicts.len(), 1);
        assert_eq!(
            report.target_conflicts[0].site,
            BankAddr::new("code", 0x200)
        );
    }
}
