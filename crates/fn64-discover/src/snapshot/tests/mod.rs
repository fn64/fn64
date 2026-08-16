    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, load_image_table_record_subject,
        CandidateDetector, FunctionEntryEvidence, MappingAddressSpace, MaterializationEvaluatorV1,
        MaterializedImageSourceV1, ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::normalize;
    use flate2::{write::DeflateEncoder, Compression};

    const BASE: u32 = 0x8000_0000;
    const ROM_START: u32 = 0x1000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    fn create_thread_fixture(pc: u32) -> [u32; 42] {
        let mut words = [0; 42];
        words[0] = 0x27bd_ffe8;
        words[2] = 0x0080_9821;
        words[7] = 0x2402_0001;
        words[8] = 0x00e0_4825;
        words[10] = 0xae62_0100;
        words[11] = 0xae63_0104;
        words[12] = 0xae62_0118;
        words[13] = 0xae62_0128;
        words[14] = 0xae65_0014;
        words[15] = 0xae60_0000;
        words[16] = 0xae60_0008;
        words[17] = 0xae66_011c;
        words[18] = 0xae68_0038;
        words[19] = 0xae69_003c;
        words[20] = 0xae64_012c;
        words[21] = 0xae60_0018;
        words[22] = 0xa662_0010;
        words[23] = 0xa660_0012;
        words[24] = 0x8fa2_002c;
        words[25] = 0xae62_0004;
        words[32] = 0x8fab_0028;
        words[34] = 0xae6a_00f0;
        words[35] = 0xae6b_00f4;
        words[39] = jal(pc + 0x1000);
        words[41] = JR_RA;
        words
    }

    fn write_create_thread_call(words: &mut [u32], index: usize, callee: u32, entry: u32) {
        words[index] = 0x3c06_0000 | entry >> 16;
        words[index + 1] = 0x24c6_0000 | entry as u16 as u32;
        words[index + 2] = jal(callee);
        words[index + 3] = NOP;
        words[index + 4] = JR_RA;
        words[index + 5] = NOP;
    }

    #[test]
    fn reachable_create_thread_entry_arguments_close_to_a_fixed_point() {
        const CREATE_INDEX: usize = 32;
        const THREAD_A_INDEX: usize = 128;
        const THREAD_B_INDEX: usize = 144;
        const DECOY_INDEX: usize = 160;
        const DECOY_TARGET_INDEX: usize = 176;
        let create = BASE + CREATE_INDEX as u32 * 4;
        let thread_a = BASE + THREAD_A_INDEX as u32 * 4;
        let thread_b = BASE + THREAD_B_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 184];
        write_create_thread_call(&mut words, 0, create, thread_a);
        words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        write_create_thread_call(&mut words, THREAD_A_INDEX, create, thread_b);
        words[THREAD_B_INDEX] = JR_RA;
        words[THREAD_B_INDEX + 1] = NOP;
        write_create_thread_call(&mut words, DECOY_INDEX, create, decoy_target);
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert_eq!(roots, BTreeSet::from([thread_a, thread_b]));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn reachable_argument_to_jalr_contract_authorizes_constant_callers_only() {
        const CALLBACK_CONSUMER_INDEX: usize = 32;
        const TARGET_INDEX: usize = 64;
        const DECOY_CALLER_INDEX: usize = 80;
        const DECOY_TARGET_INDEX: usize = 96;
        let consumer = BASE + CALLBACK_CONSUMER_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 112];
        words[0] = 0x3c04_0000 | target >> 16;
        words[1] = 0x2484_0000 | target as u16 as u32;
        words[2] = jal(consumer);
        words[3] = NOP;
        words[4] = JR_RA;
        words[5] = NOP;
        words[CALLBACK_CONSUMER_INDEX] = 0x0080_a025; // move s4, a0
        words[CALLBACK_CONSUMER_INDEX + 1] = 0x0280_f809; // jalr s4
        words[CALLBACK_CONSUMER_INDEX + 2] = NOP;
        words[CALLBACK_CONSUMER_INDEX + 3] = JR_RA;
        words[CALLBACK_CONSUMER_INDEX + 4] = NOP;
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        words[DECOY_CALLER_INDEX] = 0x3c04_0000 | decoy_target >> 16;
        words[DECOY_CALLER_INDEX + 1] = 0x2484_0000 | decoy_target as u16 as u32;
        words[DECOY_CALLER_INDEX + 2] = jal(consumer);
        words[DECOY_CALLER_INDEX + 3] = NOP;
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert!(roots.contains(&target));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn reachable_registry_dispatch_authorizes_constant_registrar_callers_only() {
        const REGISTRAR_INDEX: usize = 32;
        const DISPATCHER_INDEX: usize = 64;
        const TARGET_INDEX: usize = 100;
        const DECOY_CALLER_INDEX: usize = 120;
        const DECOY_TARGET_INDEX: usize = 140;
        let registrar = BASE + REGISTRAR_INDEX as u32 * 4;
        let dispatcher = BASE + DISPATCHER_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 160];
        words[0] = 0x3c05_0000 | target >> 16;
        words[1] = 0x24a5_0000 | target as u16 as u32;
        words[2] = jal(registrar);
        words[3] = NOP;
        words[4] = jal(dispatcher);
        words[5] = NOP;
        words[6] = JR_RA;
        words[7] = NOP;
        words[REGISTRAR_INDEX..REGISTRAR_INDEX + 11].copy_from_slice(&[
            0x0080_8025, // move s0, a0
            0xae05_0004, // sw   a1, 4(s0): callback
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0240, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d2a_0020, // lw   t2, 0x20(t1): old head
            0xae0a_0000, // sw   t2, 0(s0): link
            0x8d0b_0000, // lw   t3, 0(t0)
            0xad70_0020, // sw   s0, 0x20(t3): publish
            JR_RA,
            NOP,
        ]);
        words[DISPATCHER_INDEX..DISPATCHER_INDEX + 10].copy_from_slice(&[
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0240, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d30_0020, // lw   s0, 0x20(t1): head
            0x8e19_0004, // lw   t9, 4(s0): callback
            0x0320_f809, // jalr t9
            NOP,
            0x8e10_0000, // lw   s0, 0(s0): link
            JR_RA,
            NOP,
        ]);
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        words[DECOY_CALLER_INDEX] = 0x3c05_0000 | decoy_target >> 16;
        words[DECOY_CALLER_INDEX + 1] = 0x24a5_0000 | decoy_target as u16 as u32;
        words[DECOY_CALLER_INDEX + 2] = jal(registrar);
        words[DECOY_CALLER_INDEX + 3] = NOP;
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert!(roots.contains(&target));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn traversal_hint_cannot_bootstrap_create_thread_authority() {
        const CREATE_INDEX: usize = 32;
        const DECOY_INDEX: usize = 96;
        const TARGET_INDEX: usize = 112;
        let create = BASE + CREATE_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 128];
        words[0] = JR_RA;
        words[1] = NOP;
        words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        write_create_thread_call(&mut words, DECOY_INDEX, create, target);
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        // The decoy address could be an ordinary MaterializedBankInput seed,
        // but this authority helper accepts hardware roots only. Starting at
        // the real hardware entry therefore cannot reach or promote it.
        assert!(derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn ambiguous_create_thread_identity_fails_composition_closed() {
        const FIRST_INDEX: usize = 16;
        const SECOND_INDEX: usize = 80;
        let mut words = vec![NOP; 128];
        words[FIRST_INDEX..FIRST_INDEX + 42]
            .copy_from_slice(&create_thread_fixture(BASE + FIRST_INDEX as u32 * 4));
        words[SECOND_INDEX..SECOND_INDEX + 42]
            .copy_from_slice(&create_thread_fixture(BASE + SECOND_INDEX as u32 * 4));
        let bytes = asm(&words);

        assert!(matches!(
            derive_semantic_callable_argument_roots(
                "bank",
                &bytes,
                BASE,
                BASE + bytes.len() as u32,
                &[BASE],
            ),
            Err(SnapshotError::AmbiguousOsCreateThreadBinding { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn invalid_create_thread_entry_operands_remain_non_authoritative() {
        const CREATE_INDEX: usize = 32;
        let create = BASE + CREATE_INDEX as u32 * 4;
        for target in [BASE + 0x201, BASE - 4, BASE + 0x1000] {
            let mut words = vec![NOP; 128];
            write_create_thread_call(&mut words, 0, create, target);
            words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
            let bytes = asm(&words);
            assert!(derive_semantic_callable_argument_roots(
                "bank",
                &bytes,
                BASE,
                BASE + bytes.len() as u32,
                &[BASE],
            )
            .unwrap()
            .is_empty());
        }

        let mut unresolved = vec![NOP; 128];
        unresolved[0] = 0x8c06_0000; // lw a2, 0(zero): memory value is open.
        unresolved[1] = jal(create);
        unresolved[2] = NOP;
        unresolved[3] = JR_RA;
        unresolved[4] = NOP;
        unresolved[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        let bytes = asm(&unresolved);
        assert!(derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap()
        .is_empty());
    }

    fn rom_with_bank(bank: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; ROM_START as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..].copy_from_slice(bank);
        normalize(&bytes).unwrap()
    }

    fn headered_raw_deflate(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut encoded = Vec::with_capacity(6 + compressed.len());
        encoded.extend_from_slice(&[0x11, 0x72]);
        encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&compressed);
        encoded
    }

    fn rom_with_encoded_image(encoded: &[u8]) -> NormalizedRom {
        let len = (ROM_START as usize + encoded.len() + 3) & !3;
        let mut bytes = vec![0u8; len];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..ROM_START as usize + encoded.len()].copy_from_slice(encoded);
        normalize(&bytes).unwrap()
    }

    fn facts_for(byte_len: u32, authoritative_entries: &[u32]) -> FactDb {
        let mut facts = facts_without_executable(byte_len, authoritative_entries);
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + byte_len),
                ProofState::Proven,
                vec![executable],
                "test_executable",
            )
            .unwrap();
        facts
    }

    fn facts_without_executable(byte_len: u32, authoritative_entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + byte_len,
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
        for &entry in authoritative_entries {
            let target = BankAddr::new("bank", entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new("bank", entry + 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "test_entry",
                )
                .unwrap();
        }
        facts
    }

    fn compose<'a>(
        rom: &NormalizedRom,
        facts: &FactDb,
        bytes: &'a [u8],
        roots: &'a [u32],
    ) -> Result<ProgramSnapshotV1, SnapshotError> {
        compose_materialized_bank_v1(
            rom,
            facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes,
                seed_roots: roots,
            },
        )
    }

    #[test]
    fn authoritative_return_function_composes_to_one_exact_owner() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert_eq!(snapshot.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V6);
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 1);
        assert!(snapshot.banks[0].blocker_histogram.is_empty());
        assert_eq!(
            snapshot.banks[0].input.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_START,
                rom_end: ROM_START + bytes.len() as u32,
            }
        );
    }

    #[test]
    fn two_pc_candidate_delay_slot_root_is_suppressed_but_fact_is_retained() {
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let target = BankAddr::new("bank", BASE + 4);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("bank", BASE + 8),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Candidate,
                vec![claim],
                "test_candidate",
            )
            .unwrap();

        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 4]).unwrap();

        assert!(snapshot.banks[0]
            .authority_closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE
                && block.end_va == BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Return)));
        assert!(!snapshot.banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE + 4));
        assert!(snapshot
            .facts
            .candidate_function_entries("bank")
            .contains(&(BASE + 4)));
    }

    #[test]
    fn two_pc_authoritative_plain_delay_slot_root_is_an_alias() {
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE, BASE + 4]);

        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 4]).unwrap();
        let cfg = &snapshot.banks[0].authority_closure.cfg;
        assert!(cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| { alias.entry_va == BASE + 4 && alias.control_pc == BASE }));
        assert!(!cfg.blocks.iter().any(|block| block.start_va == BASE + 4));
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == BASE
                && block.end_va == BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Return)
        }));
    }

    #[test]
    fn authoritative_control_shaped_delay_entry_fails_loud() {
        let bytes = asm(&[JR_RA, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE, BASE + 4]);

        assert!(matches!(
            compose(&rom, &facts, &bytes, &[BASE, BASE + 4]),
            Err(SnapshotError::UnsupportedControlDelayEntry {
                bank,
                entry,
                control_pc,
            }) if bank == "bank" && entry == BASE + 4 && control_pc == BASE
        ));
    }

    #[test]
    fn candidate_call_cannot_promote_authority_reached_interior_owner() {
        let target = BASE + 4;
        let candidate_caller = BASE + 0x10;
        let bytes = asm(&[NOP, NOP, JR_RA, NOP, jal(target), NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, candidate_caller]).unwrap();

        assert!(snapshot.banks[0]
            .block_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                crate::block_proof::BlockAssessment::Proven { block }
                    if block.start_va == target
                        && block.authoritative_roots.as_slice() == [BASE]
            )));
        assert!(snapshot.banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Candidate { frontier }
                    | OwnerAssessment::Ambiguous { frontier }
                    if frontier.entry.pc == target
                        && frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
            )));
    }

    #[test]
    fn proven_nonresident_physical_bank_composes_without_resident_special_case() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "overlay".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + bytes.len() as u32,
            va_start: BASE,
            va_end: BASE + bytes.len() as u32,
        });
        facts
            .conclude(
                "bank:overlay",
                ProofState::Proven,
                vec![mapping],
                "test_overlay_mapping",
            )
            .unwrap();
        let target = BankAddr::new("overlay", BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("overlay", BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "test_overlay_entry",
            )
            .unwrap();

        let snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "overlay",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();

        assert_eq!(snapshot.banks[0].input.bank, "overlay");
        assert_eq!(snapshot.banks[0].owner_proof.bank, "overlay");
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
    }

    #[test]
    fn validated_vrom_composition_emits_v3_and_requires_facts_to_materialize() {
        const VROM_START: u32 = 0x0020_0000;

        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = FactDb::new();
        let file = facts.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x800,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: VROM_START,
            source_end: VROM_START + bytes.len() as u32,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: ROM_START,
            destination_end: ROM_START + bytes.len() as u32,
        });
        facts
            .conclude(
                load_image_table_record_subject("files", 0),
                ProofState::Proven,
                vec![file],
                "test_vrom_file",
            )
            .unwrap();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "overlay".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: VROM_START,
            rom_end: VROM_START + bytes.len() as u32,
            va_start: BASE,
            va_end: BASE + bytes.len() as u32,
        });
        facts
            .conclude(
                "bank:overlay",
                ProofState::Proven,
                vec![mapping, file],
                "test_vrom_mapping",
            )
            .unwrap();
        let target = BankAddr::new("overlay", BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("overlay", BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "test_vrom_entry",
            )
            .unwrap();

        let validated = compose_materialized_bank_validated_v2(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "overlay",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let pack = crate::block_pack::emit_validated_block_pack_v3(&validated, 0, &rom).unwrap();
        assert_eq!(pack.schema_version, crate::block_pack::BLOCK_PACK_SCHEMA_V3);
        assert!(matches!(
            pack.banks[0].blocks[0].backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Virtual,
                ..
            }
        ));
        let diagnostic =
            crate::block_pack::emit_block_pack_v1(&validated.snapshots()[0], &rom).unwrap();
        assert_eq!(
            diagnostic.schema_version,
            crate::block_pack::BLOCK_PACK_SCHEMA_V3
        );
        assert!(matches!(
            crate::block_pack::materialize_block_pack(&pack, &rom),
            Err(crate::block_pack::BlockPackError::VromRequiresFacts {
                bank,
                start_va: BASE,
            }) if bank == "overlay"
        ));
        let materialized =
            crate::block_pack::materialize_block_pack_with_facts(&pack, &rom, Some(&facts))
                .unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![JR_RA, NOP]);
    }

    #[test]
    fn traversal_seed_does_not_become_entry_authority() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert_eq!(snapshot.coverage.function_owners.candidate_owners, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 0);
        assert!(snapshot.banks[0].blocker_histogram.iter().any(|summary| {
            summary.kind == OwnerBlockerKind::EntryNotAuthoritative
                && summary.affected_assessments == 1
        }));
    }

    #[test]
    fn closure_indirect_fact_matches_owner_proof_evidence() {
        // Construct 0x80000020 in t2, jump there, then return.
        let lui_t2 = 0x3c0a_8000;
        let addiu_t2 = 0x254a_0020;
        let jr_t2 = 0x0140_0008;
        let mut bytes = asm(&[lui_t2, addiu_t2, jr_t2, NOP]);
        bytes.resize(0x20, 0);
        bytes.extend_from_slice(&asm(&[JR_RA, NOP]));
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert!(snapshot.facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::IndirectTransferAnalysis {
                site,
                state: IndirectTransferState::Exhaustive,
                targets,
                ..
            } if site.pc == BASE + 8 && targets == &[BASE + 0x20]
        )));
        assert!(!snapshot.banks[0]
            .blocker_histogram
            .iter()
            .any(|summary| { summary.kind == OwnerBlockerKind::ResolvedIndirectEvidenceMismatch }));
    }

    #[test]
    fn authoritative_block_proof_does_not_require_section_boundary_evidence() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + bytes.len() as u32),
                ProofState::Conflict,
                vec![],
                "test_remove_executable_authority",
            )
            .unwrap();
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 0);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_bytes, 8);
    }

    #[test]
    fn reached_closure_derives_executable_evidence_and_admits_owner() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        // No executable evidence was supplied; the reached proven-code block
        // itself became the typed proven range and admitted the owner.
        assert_eq!(
            snapshot.facts.proven_executable_ranges("bank"),
            vec![(BASE, BASE + 8)]
        );
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
        assert!(snapshot.banks[0].blocker_histogram.is_empty());
    }

    #[test]
    fn owner_spanning_unreached_gap_stays_blocked() {
        // `j` over two unreached words to a return block: closure reaches
        // [BASE,BASE+8) and [BASE+0x10,BASE+0x18) but never the gap between.
        let j_over_gap = 0x0800_0000 | (((BASE + 0x10) >> 2) & 0x03ff_ffff);
        let bytes = asm(&[j_over_gap, NOP, NOP, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        // Both reached blocks became proven executable ranges; the gap
        // between them was not smeared over.
        assert_eq!(
            snapshot.facts.proven_executable_ranges("bank"),
            vec![(BASE, BASE + 8), (BASE + 0x10, BASE + 0x18)]
        );
        // The owner's proposed extent spans the gap, so admission stays
        // blocked: derived ranges prove reached bytes, never the extent.
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 0);
        let histogram = &snapshot.banks[0].blocker_histogram;
        assert!(histogram
            .iter()
            .any(|summary| summary.kind == OwnerBlockerKind::NotProvenExecutable));
        assert!(histogram
            .iter()
            .any(|summary| summary.kind == OwnerBlockerKind::OwnerNotContiguous));
    }

    #[test]
    fn block_proof_rejects_shared_decoder_failures() {
        let unknown = 0x7801_2345;
        let bytes = asm(&[unknown]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 0);
        assert!(matches!(
            &snapshot.banks[0].block_proof.assessments[0],
            crate::block_proof::BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&crate::block_proof::BlockProofBlocker::InvalidInstruction {
                    pc: BASE,
                    word: unknown,
                })
        ));
    }

    #[test]
    fn block_pack_round_trip_binds_geometry_without_serializing_words() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let validated = compose_materialized_bank_validated_v2(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let pack = crate::block_pack::emit_validated_block_pack_v2(&validated, 0, &rom).unwrap();
        assert_eq!(pack.schema_version, crate::block_pack::BLOCK_PACK_SCHEMA_V3);
        let diagnostic =
            crate::block_pack::emit_block_pack_v1(&validated.snapshots()[0], &rom).unwrap();
        assert_eq!(
            diagnostic.schema_version,
            crate::block_pack::BLOCK_PACK_SCHEMA_V3
        );
        assert!(matches!(
            crate::block_pack::emit_validated_block_pack_v2(&validated, 1, &rom),
            Err(
                crate::block_pack::BlockPackError::ValidatedSnapshotIndexOutsideComposition {
                    index: 1,
                    count: 1,
                }
            )
        ));
        let mut caller_authored = validated.snapshots()[0].clone();
        caller_authored.schema_version = 1;
        assert!(matches!(
            crate::block_pack::emit_block_pack_v1(&caller_authored, &rom),
            Err(
                crate::block_pack::BlockPackError::UnsupportedSnapshotSchema {
                    expected: PROGRAM_SNAPSHOT_SCHEMA_V6,
                    actual: 1,
                }
            )
        ));
        let json = serde_json::to_string(&pack).unwrap();
        assert!(!json.contains("\"words\""));
        let materialized = crate::block_pack::materialize_block_pack(&pack, &rom).unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![JR_RA, NOP]);
        let runner = crate::block_pack::emit_materialized_bank_runner(
            &materialized[0],
            "run_materialized_bank",
        );
        assert!(runner.contains(&format!("{BASE:#010X} => {{")));
        assert!(runner.contains("Sparse bank-qualified MIPS runner"));
        let code_bank = crate::block_pack::materialized_code_bank(&materialized[0]).unwrap();
        assert_eq!(code_bank.instruction_count(), 2);
        let mut catalog = fn64_cpu_runtime::CodeCatalog::new();
        let bank_id = code_bank.id();
        catalog.register(code_bank).unwrap();
        assert_eq!(
            catalog
                .resolve(fn64_cpu_runtime::ExecutionKey::new(
                    bank_id,
                    fn64_cpu_runtime::GuestPc::new(BASE),
                ))
                .unwrap()
                .word,
            JR_RA
        );

        let mut changed = rom.clone();
        changed.bytes[ROM_START as usize] ^= 1;
        assert!(matches!(
            crate::block_pack::materialize_block_pack(&pack, &changed),
            Err(crate::block_pack::BlockPackError::BlockDigestMismatch { .. })
        ));
    }

    #[test]
    fn materialized_bytes_are_bound_to_the_normalized_rom() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let different = asm(&[NOP, NOP]);
        assert!(matches!(
            compose(&rom, &facts, &different, &[BASE]),
            Err(SnapshotError::MaterializedBytesMismatch { .. })
        ));
    }

    #[test]
    fn evaluated_image_subrange_is_rederived_with_typed_output_offsets() {
        let full_output = asm(&[NOP, NOP, JR_RA, NOP]);
        let selected = &full_output[8..];
        let encoded = headered_raw_deflate(&full_output);
        let rom = rom_with_encoded_image(&encoded);
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + encoded.len() as u32,
            cursor: 0,
        };
        let evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 };
        let receipt = crate::materialized_image::evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source,
            &evaluator,
            MaterializedImageLimitsV1::default(),
        )
        .unwrap()
        .receipt()
        .clone();
        let receipt_sha256 = crate::facts::evaluated_image_receipt_sha256_v1(&receipt);
        let mut facts = FactDb::new();
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "materialized".into(),
            va_start: BASE,
            va_end: BASE + full_output.len() as u32,
            receipt,
        });
        facts
            .conclude(
                "bank:materialized",
                ProofState::Proven,
                vec![image],
                "test evaluated image",
            )
            .unwrap();

        let snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "materialized",
                va_start: BASE + 8,
                bytes: selected,
                seed_roots: &[BASE + 8],
            },
        )
        .unwrap();

        assert_eq!(snapshot.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V6);
        assert_eq!(
            snapshot.banks[0].input.backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256,
                output_start: 8,
                output_end: 16,
            }
        );
        let wire = serde_json::to_string(&snapshot).unwrap();
        assert!(wire.contains("\"kind\":\"materialized\""));
        assert!(!wire.contains("\"bytes\":"));
    }

    #[test]
    fn malformed_geometry_fails_loudly() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        assert!(matches!(
            compose_materialized_bank_v1(
                &rom,
                &facts,
                MaterializedBankInput {
                    bank: "bank",
                    va_start: BASE + 1,
                    bytes: &bytes,
                    seed_roots: &[BASE + 1],
                }
            ),
            Err(SnapshotError::UnalignedBank { .. })
        ));
        assert!(matches!(
            compose(&rom, &facts, &bytes, &[BASE + 2]),
            Err(SnapshotError::RootUnaligned { .. })
        ));
    }

    #[test]
    fn serialization_is_deterministic_and_contains_no_rom_bytes() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let expected =
            serde_json::to_vec(&compose(&rom, &facts, &bytes, &[BASE]).unwrap()).unwrap();
        let text = String::from_utf8(expected.clone()).unwrap();
        assert!(!text.contains("\"bytes\":"));
        assert!(text.contains("\"bytes_sha256\":"));
        for _ in 0..10 {
            let actual =
                serde_json::to_vec(&compose(&rom, &facts, &bytes, &[BASE]).unwrap()).unwrap();
            assert_eq!(actual, expected);
        }
    }

    // ---- Multi-bank cross-bank authority ----
    //
    // Bank X ("caller") at VA X_BASE holds an authoritative returning function
    // that `jal`s into bank Y ("callee") at Y_BASE. Both banks live in the same
    // 256 MB region so a real MIPS `jal` can address across them. Y_BASE names a
    // valid returning function with NO in-bank authority; only X's proven direct
    // call can authorize it.

    const X_BASE: u32 = 0x8000_0000;
    const X_ROM: u32 = 0x1000;
    const Y_BASE: u32 = 0x8000_1000;
    const Y_ROM: u32 = 0x2000;

    fn jal_to(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    /// A ROM holding bank X bytes at `X_ROM` and bank Y bytes at `Y_ROM`.
    fn rom_with_two_banks(x_bytes: &[u8], y_bytes: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; Y_ROM as usize + y_bytes.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        bytes[X_ROM as usize..X_ROM as usize + x_bytes.len()].copy_from_slice(x_bytes);
        bytes[Y_ROM as usize..Y_ROM as usize + y_bytes.len()].copy_from_slice(y_bytes);
        normalize(&bytes).unwrap()
    }

    /// Prove one physical bank mapping and, optionally, an authoritative entry.
    fn prove_bank(
        facts: &mut FactDb,
        bank: &str,
        rom_start: u32,
        va_start: u32,
        byte_len: u32,
        authoritative_entries: &[u32],
    ) {
        let mapping = facts.insert(Fact::RomMapping {
            bank: bank.into(),
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end: rom_start + byte_len,
            va_start,
            va_end: va_start + byte_len,
        });
        facts
            .conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
        for &entry in authoritative_entries {
            let target = BankAddr::new(bank, entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new(bank, entry + 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "test_entry",
                )
                .unwrap();
        }
    }

    fn prove_vrom_bank(
        facts: &mut FactDb,
        bank: &str,
        vrom_start: u32,
        physical_start: u32,
        va_start: u32,
        byte_len: u32,
    ) {
        let record = facts.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x800,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: vrom_start,
            source_end: vrom_start + byte_len,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: physical_start,
            destination_end: physical_start + byte_len,
        });
        facts
            .conclude(
                load_image_table_record_subject("files", 0),
                ProofState::Proven,
                vec![record],
                "test_vrom_file",
            )
            .unwrap();
        let mapping = facts.insert(Fact::RomMapping {
            bank: bank.into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: vrom_start,
            rom_end: vrom_start + byte_len,
            va_start,
            va_end: va_start + byte_len,
        });
        facts
            .conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test_vrom_mapping",
            )
            .unwrap();
    }

    fn two_bank_inputs<'a>(
        x_bytes: &'a [u8],
        y_bytes: &'a [u8],
        x_seeds: &'a [u32],
        y_seeds: &'a [u32],
    ) -> [MaterializedBankInput<'a>; 2] {
        [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: x_bytes,
                seed_roots: x_seeds,
            },
            MaterializedBankInput {
                bank: "callee",
                va_start: Y_BASE,
                bytes: y_bytes,
                seed_roots: y_seeds,
            },
        ]
    }

    fn callee_owner_is_proven(snapshots: &[ProgramSnapshotV1]) -> bool {
        snapshots[1]
            .banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Y_BASE)
            })
    }

    fn callee_entry_not_authoritative(snapshots: &[ProgramSnapshotV1]) -> bool {
        snapshots[1].banks[0].owner_proof.assessments.iter().any(|assessment| {
            assessment.entry().pc == Y_BASE
                && matches!(
                    assessment,
                    OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier }
                        if frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
                )
        })
    }

    #[test]
    fn proven_cross_bank_jal_authorizes_a_callee_owner() {
        // X calls Y with a real `jal`; Y is a bare returning function with no
        // in-bank authority. Cross-bank composition must admit Y's owner.
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            callee_owner_is_proven(&snapshots),
            "a proven cross-bank jal should authorize the callee entry: {:?}",
            snapshots[1].banks[0].owner_proof.assessments
        );
        // The honest cross-bank edge is recorded as a fact on the callee.
        assert!(snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller" && target.bank == "callee" && target.pc == Y_BASE
        )));
    }

    #[test]
    fn unique_cross_bank_call_seeds_semantic_thread_entry_recovery() {
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        let semantic_claim = snapshots[1]
            .facts
            .facts()
            .iter()
            .enumerate()
            .find_map(|(index, fact)| match fact {
                Fact::FunctionEntryClaim {
                    target,
                    detector: CandidateDetector::SemanticCallableArgument,
                    evidence:
                        FunctionEntryEvidence::SemanticCallableArgument {
                            call_site,
                            callee,
                            pointer_register: 6,
                            contract: SemanticCallableContract::OsCreateThread,
                        },
                    proposed_state: ProofState::Proven,
                } if target.bank == "callee"
                    && target.pc == thread
                    && call_site.bank == "callee"
                    && call_site.pc == Y_BASE + 8
                    && callee.bank == "callee"
                    && callee.pc == create =>
                {
                    Some(index)
                }
                _ => None,
            })
            .expect("semantic authority must retain its exact call contract");
        let conclusion = snapshots[1]
            .facts
            .conclusion(&function_entry_subject(&BankAddr::new("callee", thread)))
            .expect("semantic entry must have a conclusion");
        assert_eq!(conclusion.state, ProofState::Proven);
        assert!(conclusion.justified_by.contains(&semantic_claim));

        let wire = serde_json::to_vec(&snapshots[1]).unwrap();
        let round_trip: ProgramSnapshotV1 = serde_json::from_slice(&wire).unwrap();
        assert_eq!(round_trip.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V6);
        assert_eq!(
            serde_json::to_vec(&round_trip.facts).unwrap(),
            serde_json::to_vec(&snapshots[1].facts).unwrap()
        );
    }

    #[test]
    fn unique_vrom_cross_calls_reach_a_semantic_fixed_point() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        const Y_VROM: u32 = 0x5000;
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Z_BASE + CREATE_INDEX as u32 * 4;
        let thread = Z_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[jal_to(Z_BASE), NOP, JR_RA, NOP]);
        let mut z_words = vec![NOP; 112];
        write_create_thread_call(&mut z_words, 0, create, thread);
        z_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        z_words[THREAD_INDEX] = JR_RA;
        z_words[THREAD_INDEX + 1] = NOP;
        let z = asm(&z_words);

        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_vrom_bank(&mut facts, "loaded", Y_VROM, Y_ROM, Y_BASE, y.len() as u32);
        prove_bank(&mut facts, "resident", Z_ROM, Z_BASE, z.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "loaded",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "resident",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
        ];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[2].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        assert!(snapshots[2].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "loaded"
                    && source.pc == Y_BASE
                    && target.bank == "resident"
                    && target.pc == Z_BASE
        )));
    }

    #[test]
    fn resolved_then_direct_cross_bank_chain_reaches_one_fixed_point() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        let x = asm(&[
            0x3c19_0000 | (Y_BASE >> 16),
            0x3739_0000 | (Y_BASE & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let y = asm(&[jal_to(Z_BASE), NOP, JR_RA, NOP]);
        let z = asm(&[JR_RA, NOP]);
        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "leaf", Z_ROM, Z_BASE, z.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[2].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE
            )));
        assert!(snapshots[2].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "middle"
                    && source.pc == Y_BASE
                    && target.bank == "leaf"
                    && target.pc == Z_BASE
        )));

        let reversed = [
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
        ];
        let reversed_snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 2,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        assert!(reversed_snapshots[0].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE
            )));
        assert!(reversed_snapshots[0]
            .facts
            .facts()
            .iter()
            .any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "middle"
                        && source.pc == Y_BASE
                        && target.bank == "leaf"
                        && target.pc == Z_BASE
            )));
        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 1,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: 2,
                limit: 1,
            }
        );
    }

mod composition;
