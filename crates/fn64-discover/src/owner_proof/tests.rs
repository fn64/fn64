    use super::*;
    use crate::cfg::{build_cfg, build_cfg_with_indirect};
    use crate::facts::{
        evaluated_image_receipt_sha256_v1, CandidateDetector, EvaluatedImageReceiptV1,
        FunctionEntryEvidence, IndirectTransferKind, MaterializationEvaluatorV1,
        MaterializedImageSourceV1, MaterializedImageSuffixV1, ProloguePattern, RomAddressSpace,
    };
    use crate::partition::partition;

    const BASE: u32 = 0x8000_0000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn facts_for(bytes_len: u32, entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1000 + bytes_len,
            va_start: BASE,
            va_end: BASE + bytes_len,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
        add_executable_and_entries(&mut facts, bytes_len, entries);
        facts
    }

    fn evaluated_receipt(output_len: u32) -> EvaluatedImageReceiptV1 {
        EvaluatedImageReceiptV1 {
            evaluator: MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            source: MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x2000,
                rom_end: 0x2040,
                cursor: 4,
            },
            source_sha256: "11".repeat(32),
            output_len,
            output_sha256: "22".repeat(32),
            streams: Vec::new(),
            trailing_suffix: MaterializedImageSuffixV1 {
                offset: 0,
                len: 0,
                sha256: "33".repeat(32),
            },
        }
    }

    fn materialized_facts_for(image_len: u32, receipt_output_len: u32, entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + image_len,
            receipt: evaluated_receipt(receipt_output_len),
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![image],
                "test_materialized_image",
            )
            .unwrap();
        add_executable_and_entries(&mut facts, image_len, entries);
        facts
    }

    fn add_executable_and_entries(facts: &mut FactDb, bytes_len: u32, entries: &[u32]) {
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + bytes_len,
        });
        facts
            .conclude(
                crate::facts::executable_range_subject("bank", BASE, BASE + bytes_len),
                ProofState::Proven,
                vec![executable],
                "test_executable",
            )
            .unwrap();
        for &entry in entries {
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
    }

    fn frontier(assessment: &OwnerAssessment) -> &OwnerFrontier {
        match assessment {
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                frontier
            }
            OwnerAssessment::Proven { .. } => panic!("expected unresolved frontier"),
        }
    }

    #[test]
    fn owner_proof_authority_is_bank_bound_and_fails_closed_on_mismatch() {
        let site = BASE + 0x20;
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, site]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[]);
        let authority_closure = ClosureResult {
            cfg: Cfg {
                bank: "other".into(),
                word_class: BTreeMap::new(),
                blocks: Vec::new(),
                direct_calls: Vec::new(),
                tail_transfers: Vec::new(),
                indirect_sites: Vec::new(),
                plain_delay_entry_aliases: Vec::new(),
                unsupported_delay_entries: Vec::new(),
                rejected_transfer_targets: Vec::new(),
                proven_roots: Vec::new(),
            },
            indirect: Vec::new(),
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authority_closure,
            &facts,
            &BTreeSet::from([BASE]),
        );
        let report =
            prove_exact_owners_with_authority(&cfg, &partition, &facts, &bytes, BASE, &authority);

        let blockers = &frontier(&report.assessments[0]).blockers;
        assert!(blockers.contains(&OwnerBlocker::EntryNotAuthoritative));
        assert!(blockers.contains(&OwnerBlocker::UnresolvedIndirect {
            site,
            scope: IndirectScope::Bank,
        }));
    }

    #[test]
    fn exact_direct_call_authority_rejects_malformed_or_unproven_delay_geometry() {
        let target = BASE + 0x20;
        let block = BasicBlock {
            start_va: BASE,
            end_va: BASE + 8,
            terminator: BlockTerminator::Call {
                target,
                next: BASE + 8,
            },
        };
        let mut cfg = Cfg {
            bank: "bank".into(),
            word_class: [
                (BASE, WordClass::ProvenCode),
                (BASE + 4, WordClass::ProvenCode),
            ]
            .into_iter()
            .collect(),
            blocks: vec![block.clone()],
            direct_calls: vec![(BASE, target)],
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            rejected_transfer_targets: Vec::new(),
            proven_roots: vec![BASE],
        };
        assert_eq!(
            exact_authority_direct_call(&cfg, &block),
            Some((BASE, target))
        );

        let malformed = BasicBlock {
            end_va: BASE + 4,
            ..block.clone()
        };
        assert_eq!(exact_authority_direct_call(&cfg, &malformed), None);

        cfg.word_class.insert(BASE + 4, WordClass::Unknown);
        assert_eq!(exact_authority_direct_call(&cfg, &block), None);

        cfg.word_class.insert(BASE + 4, WordClass::ProvenCode);
        cfg.direct_calls.clear();
        assert_eq!(exact_authority_direct_call(&cfg, &block), None);
    }

    #[test]
    fn exact_owner_requires_and_carries_typed_affine_backing() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let OwnerAssessment::Proven { owner } = &report.assessments[0] else {
            panic!("expected exact owner: {:?}", report.assessments[0]);
        };
        assert_eq!(owner.entry, BankAddr::new("bank", BASE));
        assert_eq!(owner.va_end, BASE + 12);
        assert_eq!(
            owner.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x1000,
                rom_end: 0x100c,
            }
        );
        assert_eq!(owner.byte_len(), 12);
    }

    #[test]
    fn exact_owner_carries_materialized_output_offsets_without_rom_coordinates() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = materialized_facts_for(bytes.len() as u32, bytes.len() as u32, &[BASE]);
        let expected_receipt = evaluated_receipt(bytes.len() as u32);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let OwnerAssessment::Proven { owner } = &report.assessments[0] else {
            panic!(
                "expected exact materialized owner: {:?}",
                report.assessments[0]
            );
        };
        assert_eq!(
            owner.backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256: evaluated_image_receipt_sha256_v1(&expected_receipt),
                output_start: 0,
                output_end: 12,
            }
        );
    }

    #[test]
    fn a_seeded_root_without_entry_proof_remains_candidate() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::EntryNotAuthoritative));
    }

    #[test]
    fn missing_executable_evidence_withholds_exact_extent() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let subject = crate::facts::executable_range_subject("bank", BASE, BASE + 8);
        facts
            .conclude(subject, ProofState::Conflict, vec![], "test_conflict")
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::NotProvenExecutable));
    }

    #[test]
    fn direct_call_from_proven_code_authorizes_its_target_entry() {
        let target = BASE + 0x20;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .unwrap();
        assert!(matches!(target_assessment, OwnerAssessment::Proven { .. }));
    }

    #[test]
    fn running_off_the_image_never_proves_an_extent() {
        let bytes = asm(&[NOP, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::RanOffEnd { block_start: BASE }));
    }

    #[test]
    fn invalid_instruction_is_a_typed_owner_blocker() {
        let unknown = 0x7801_2345;
        let bytes = asm(&[unknown]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::InvalidInstruction {
                pc: BASE,
                word: unknown,
            }
        ));
    }

    #[test]
    fn missing_delay_slot_is_a_typed_owner_blocker() {
        let bytes = asm(&[JR_RA]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::MissingDelaySlot { control_pc: BASE }));
    }

    #[test]
    fn distinct_bank_backings_are_ambiguous() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let first = facts.conclusion("bank:bank").unwrap().justified_by[0];
        let second = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x2000,
            rom_end: 0x2008,
            va_start: BASE,
            va_end: BASE + 8,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![first, second],
                "test competing proven backing",
            )
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::AmbiguousBankBacking));
    }

    #[test]
    fn invalid_bank_backing_geometry_is_ambiguous() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = materialized_facts_for(bytes.len() as u32, 12, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::InvalidBankBackingGeometry));
    }

    #[test]
    fn proven_external_call_to_interior_is_ambiguous() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::DirectCall {
            source: BankAddr::new("other", 0x9000_0000),
            target: BankAddr::new("bank", BASE + 4),
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::IncomingEdge {
                source: 0x9000_0000,
                target: BASE + 4,
                edge: IncomingEdgeKind::DirectCall,
            }));
    }

    #[test]
    fn unresolved_indirect_anywhere_in_bank_blocks_exact_incoming_closure() {
        let target = BASE + 0x20;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site: target,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn candidate_only_indirect_does_not_enter_authority_owner_proof() {
        let site = BASE + 0x20;
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let broad_cfg = build_cfg("bank", &bytes, BASE, &[BASE, site]);
        let partition = partition(&broad_cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let authority_closure = ClosureResult {
            cfg: build_cfg("bank", &bytes, BASE, &[BASE]),
            indirect: Vec::new(),
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authority_closure,
            &facts,
            &BTreeSet::new(),
        );
        let report = prove_exact_owners_with_authority(
            &broad_cfg, &partition, &facts, &bytes, BASE, &authority,
        );
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(matches!(caller, OwnerAssessment::Proven { .. }));

        let authoritative_site = ClosureResult {
            cfg: broad_cfg.clone(),
            indirect: vec![crate::resolve::IndirectResolution {
                site_pc: site,
                via_call: true,
                state: IndirectProofState::Open,
                kind: None,
                targets: Vec::new(),
                memory_sources: Vec::new(),
            }],
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authoritative_site,
            &facts,
            &BTreeSet::new(),
        );
        let blocked = prove_exact_owners_with_authority(
            &broad_cfg, &partition, &facts, &bytes, BASE, &authority,
        );
        let caller = blocked
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn guard_bounded_domain_disjoint_from_owner_discharges_bank_scoped_site() {
        let site = BASE + 0x20;
        let jal = 0x0c00_0000 | ((site >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![site],
            memory_sources: vec![BASE + 0x18],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        let still_blocked = match caller {
            OwnerAssessment::Proven { .. } => false,
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                frontier
                    .blockers
                    .contains(&OwnerBlocker::UnresolvedIndirect {
                        site,
                        scope: IndirectScope::Bank,
                    })
            }
        };
        assert!(!still_blocked);

        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Open,
            kind: None,
            targets: vec![],
            memory_sources: vec![],
        });
        let conflicted = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = conflicted
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn bounded_domain_that_can_enter_owner_keeps_bank_scoped_blocker() {
        let site = BASE + 0x20;
        let jal = 0x0c00_0000 | ((site >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![BASE + 4],
            memory_sources: vec![BASE + 0x18],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn bounded_domain_never_discharges_owner_scoped_site() {
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let bytes = asm(&[jalr_t9, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![BASE + 0x100],
            memory_sources: vec![BASE + 0x80],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::UnresolvedIndirect {
                site: BASE,
                scope: IndirectScope::Owner,
            }
        ));
    }

    #[test]
    fn resolved_indirect_requires_one_matching_exhaustive_fact() {
        let jr_t9 = (25u32 << 21) | 0x08;
        let bytes = asm(&[jr_t9, NOP, JR_RA, NOP]);
        let mut targets = BTreeMap::new();
        targets.insert(BASE, vec![BASE + 8]);
        let cfg = build_cfg_with_indirect("bank", &bytes, BASE, &[BASE], &targets);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);

        let missing = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&missing.assessments[0])
            .blockers
            .contains(&OwnerBlocker::ResolvedIndirectNotExhaustive { site: BASE }));

        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: false,
            state: IndirectTransferState::Open,
            kind: None,
            targets: vec![],
            memory_sources: vec![],
        });
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: false,
            state: IndirectTransferState::Exhaustive,
            kind: Some(IndirectTransferKind::Constant),
            targets: vec![BASE + 8],
            memory_sources: vec![],
        });

        let closed = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            closed.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn exhaustive_jalr_target_is_an_authoritative_entry() {
        // An exhaustive computed CALL (`jalr $ra, $t9`) whose proven target set
        // is represented in the CFG promotes that target to an authoritative
        // callable root, exactly like a direct `jal`. The target carries no
        // entry fact of its own: its authority comes solely from being a proven
        // reachable computed-call destination.
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jalr_ra_t9, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let target = BASE + 0x20;
        let mut exhaustive = BTreeMap::new();
        exhaustive.insert(BASE, vec![target]);
        let cfg = build_cfg_with_indirect("bank", &bytes, BASE, &[BASE], &exhaustive);
        let partition = partition(&cfg);
        // Only the caller BASE has an entry fact; the target does not.
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: true,
            state: IndirectTransferState::Exhaustive,
            kind: Some(IndirectTransferKind::Constant),
            targets: vec![target],
            memory_sources: vec![],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .expect("the exhaustive-call target is assessed");
        assert!(
            matches!(target_assessment, OwnerAssessment::Proven { .. }),
            "exhaustive computed-call target should be a proven owner: {target_assessment:?}"
        );
    }

    #[test]
    fn non_exhaustive_jalr_target_is_not_authoritative() {
        // Same computed call, but the site is NOT exhaustively resolved: the
        // CFG keeps it an open `Indirect`, so its runtime destination is
        // unproven. A block that merely looks like a function at BASE+0x20 must
        // NOT be admitted as an owner — it lacks any authoritative entry.
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jalr_ra_t9, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let target = BASE + 0x20;
        // No exhaustive map entry: the `jalr` stays an open indirect site.
        // Seed the target as a traversal root so it is still assessed, proving
        // that traversal reach alone never confers entry authority.
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, target]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .expect("the seeded target is assessed");
        assert!(
            frontier(target_assessment)
                .blockers
                .contains(&OwnerBlocker::EntryNotAuthoritative),
            "a non-exhaustive computed-call target must not be authoritative: {target_assessment:?}"
        );
    }

    #[test]
    fn competing_root_closure_is_ambiguous() {
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, BASE + 4]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE, BASE + 4]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(report
            .assessments
            .iter()
            .all(|assessment| matches!(assessment, OwnerAssessment::Ambiguous { .. })));
        assert!(report
            .assessments
            .iter()
            .any(|assessment| frontier(assessment)
                .blockers
                .iter()
                .any(|blocker| matches!(blocker, OwnerBlocker::PartitionAmbiguity { .. }))));
    }

    #[test]
    fn observed_indirect_entry_into_interior_is_ambiguous() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::ObservedIndirectTarget {
            site: BankAddr::new("other", 0x9000_0000),
            target: BankAddr::new("bank", BASE + 4),
            trace: "synthetic".into(),
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::ObservedInteriorEntry {
                site: 0x9000_0000,
                target: BASE + 4,
            }
        ));
    }

    #[test]
    fn delay_slot_must_be_proven_code() {
        let bytes = asm(&[JR_RA, NOP]);
        let mut cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        cfg.word_class.insert(BASE + 4, WordClass::CandidateCode);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::WordNotProvenCode {
                pc: BASE + 4,
                class: Some(WordClass::CandidateCode),
            }
        ));
    }

    #[test]
    fn interior_candidate_entry_claim_withholds_exactness_until_resolved() {
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let interior = BankAddr::new("bank", BASE + 8);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: interior.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: interior.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("bank", BASE + 12),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&interior),
                ProofState::Candidate,
                vec![claim],
                "test_interior_candidate",
            )
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::InteriorCandidateEntry { pc: BASE + 8 }));

        // Rejecting the claim discharges the blocker: the extent is exact.
        facts
            .conclude(
                function_entry_subject(&interior),
                ProofState::Rejected,
                vec![claim],
                "test_refuted",
            )
            .unwrap();
        let resolved = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            resolved.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn trailing_unattributed_code_blocks_the_right_boundary() {
        // The function returns at +4/+8; the unreached word after it is a
        // bare `jr $ra` with no entry claim — plausible code attributed to
        // nothing, so the proposed end stays unproven.
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::TrailingUnattributedCode {
                pc: BASE + 8,
                word: JR_RA,
            }
        ));

        // An entry claim at exactly the boundary attributes the trailing
        // bytes and closes the right edge.
        let neighbor = BankAddr::new("bank", BASE + 8);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: neighbor.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("bank", BASE + 0x100),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&neighbor),
                ProofState::Candidate,
                vec![claim],
                "test_boundary_claim",
            )
            .unwrap();
        let closed = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            closed.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn zero_padding_to_the_image_end_keeps_the_right_boundary_closed() {
        let bytes = asm(&[JR_RA, NOP, 0, 0]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn report_is_byte_deterministic() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let left = serde_json::to_vec(&prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE))
            .unwrap();
        let right = serde_json::to_vec(&prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE))
            .unwrap();
        assert_eq!(left, right);
    }
