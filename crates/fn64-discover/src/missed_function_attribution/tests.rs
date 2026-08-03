    use super::*;
    use crate::facts::{FactDb, FunctionEntryEvidence};
    use crate::grade_candidates::DetectorCandidateIdentitiesV2;
    use crate::snapshot::PROGRAM_SNAPSHOT_SCHEMA_V6;

    fn validation_envelope() -> AttributionEnvelopeV2 {
        let section = AnswerSectionV1 {
            raw_ordinal: 0,
            name: "section".into(),
            execution_domain: ExecutionDomain::Unknown,
            rom_start: 0x100,
            vram_start: 0x8000_0000,
            size: 4,
        };
        let function = AnswerFunctionV1 {
            raw_ordinal: 0,
            section_raw_ordinal: 0,
            name: "marker".into(),
            vram: section.vram_start,
            size: 0,
            kind: AnswerRowKind::ZeroSizeMarker,
        };
        let observations = AttributionObservationsV1 {
            mappings: Vec::new(),
            claims: Vec::new(),
            conclusion_states: Vec::new(),
            word_classes: Vec::new(),
            owners: Vec::new(),
            incoming_relations: Vec::new(),
            candidate_detectors: Vec::new(),
        };
        let status = AnswerAttributionStatusV1::NotDiscoverableMarker;
        let mechanism_cluster_key = mechanism_key(ExecutionDomain::Unknown, &status);
        let row = AnswerAttributionV1 {
            function,
            execution_domain: ExecutionDomain::Unknown,
            raw_rom: section.rom_start,
            status,
            instance_cluster_key: instance_key(&mechanism_cluster_key, &observations).unwrap(),
            mechanism_cluster_key,
            observations,
        };
        let totals = AttributionTotalsV1 {
            raw_rows: 1,
            marker_rows: 1,
            ..AttributionTotalsV1::default()
        };
        let mut report = AttributionReportV1 {
            schema_version: MISSED_FUNCTION_ATTRIBUTION_SCHEMA_V1,
            sections: vec![section],
            rows: vec![row],
            candidate_statuses: Vec::new(),
            candidate_totals: CandidateAccountingTotalsV1::default(),
            totals: totals.clone(),
            per_domain: vec![DomainTotalsV1 {
                execution_domain: ExecutionDomain::Unknown,
                totals,
            }],
            canonical_sha256: String::new(),
        };
        report.canonical_sha256 = canonical_attribution_report_digest(&report).unwrap();
        AttributionEnvelopeV2 {
            schema_version: KNOWN_FUNCTION_ATTRIBUTION_ENVELOPE_SCHEMA_V2,
            algorithm: KNOWN_FUNCTION_ATTRIBUTION_ALGORITHM_V2.into(),
            normalized_rom_sha256: "1".repeat(64),
            cold_workspace_manifest_sha256: "2".repeat(64),
            cold_candidate_identities_v3_sha256: "3".repeat(64),
            answer_key_sha256: "4".repeat(64),
            answer_key_execution_domain: ExecutionDomain::Unknown,
            report,
        }
    }

    fn validation_bindings() -> AttributionEnvelopeBindingsV2<'static> {
        AttributionEnvelopeBindingsV2 {
            normalized_rom_sha256:
                "1111111111111111111111111111111111111111111111111111111111111111",
            cold_workspace_manifest_sha256:
                "2222222222222222222222222222222222222222222222222222222222222222",
            cold_candidate_identities_v3_sha256:
                "3333333333333333333333333333333333333333333333333333333333333333",
            answer_key_sha256: "4444444444444444444444444444444444444444444444444444444444444444",
        }
    }

    fn encode_validation_envelope(envelope: &mut AttributionEnvelopeV2) -> Vec<u8> {
        envelope.report.canonical_sha256 =
            canonical_attribution_report_digest(&envelope.report).unwrap();
        serde_json::to_vec(envelope).unwrap()
    }

    #[test]
    fn strict_report_validator_rejects_recomputed_digest_mutations() {
        let mut valid = validation_envelope();
        let bytes = encode_validation_envelope(&mut valid);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_ok());

        let mut status = valid.clone();
        status.report.rows[0].status = AnswerAttributionStatusV1::Missed {
            primary_reason: MissReasonV1::NoRelation,
        };
        status.report.rows[0].mechanism_cluster_key =
            mechanism_key(ExecutionDomain::Unknown, &status.report.rows[0].status);
        status.report.rows[0].instance_cluster_key = instance_key(
            &status.report.rows[0].mechanism_cluster_key,
            &status.report.rows[0].observations,
        )
        .unwrap();
        let bytes = encode_validation_envelope(&mut status);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());

        let mut totals = valid.clone();
        totals.report.totals.raw_rows = 2;
        totals.report.per_domain[0].totals.raw_rows = 2;
        let bytes = encode_validation_envelope(&mut totals);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());

        let mut candidate = valid.clone();
        candidate
            .report
            .candidate_statuses
            .push(CandidateAttributionV1 {
                identity: CandidateAccountingIdentityV1::Ungradable {
                    address: BankAddr::new("bank", 0x8000_0000),
                },
                combined: true,
                detectors: vec![CandidateDetector::ProloguePattern],
                detector_sources: Vec::new(),
                status: CandidateStatusV1::Outside,
            });
        candidate.report.candidate_totals = CandidateAccountingTotalsV1 {
            denominator: 1,
            gradable: 1,
            combined: 1,
            outside: 1,
            ..CandidateAccountingTotalsV1::default()
        };
        let bytes = encode_validation_envelope(&mut candidate);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());

        let mut unknown = serde_json::to_value(valid).unwrap();
        unknown["report"]["rows"][0]["unknown"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&unknown).unwrap();
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());
    }

    #[test]
    fn strict_report_validator_rejects_binding_and_order_mutations() {
        let mut envelope = validation_envelope();
        envelope.normalized_rom_sha256 = "5".repeat(64);
        let bytes = encode_validation_envelope(&mut envelope);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());

        let mut order = validation_envelope();
        let mut duplicate = order.report.sections[0].clone();
        duplicate.name = "duplicate".into();
        order.report.sections.push(duplicate);
        let bytes = encode_validation_envelope(&mut order);
        assert!(validate_attribution_envelope_json_v2(&bytes, validation_bindings()).is_err());
    }

    const VA: u32 = 0x8000_0000;
    const ROM: u32 = 0x100;

    fn mapping(
        bank: u32,
        rom_space: RomAddressSpace,
        rom_start: u32,
        va_start: u32,
        size: u32,
    ) -> CompactMapping {
        CompactMapping {
            bank,
            rom_space,
            rom_start,
            rom_end: rom_start + size,
            va_start,
            va_end: va_start + size,
        }
    }

    fn base_index() -> ColdAttributionIndex {
        ColdAttributionIndex {
            banks: vec!["code".to_owned()],
            mappings: vec![mapping(0, RomAddressSpace::Physical, ROM, VA, 0x100)],
            mapping_prefix_max_end: vec![ROM + 0x100],
            claims: BTreeMap::new(),
            conclusions: BTreeMap::new(),
            words: BTreeMap::new(),
            owners: BTreeMap::new(),
            relations: BTreeMap::new(),
        }
    }

    fn addressed(rom_space: RomAddressSpace, offset: u32) -> AddressedPhysicalEntryV2 {
        AddressedPhysicalEntryV2 {
            rom_space,
            rom: ROM + offset,
            vram: VA + offset,
        }
    }

    fn identities(
        combined: Vec<AddressedPhysicalEntryV2>,
        per_detector: Vec<(CandidateDetector, Vec<AddressedPhysicalEntryV2>)>,
    ) -> ScopedCandidateIdentitiesV3 {
        ScopedCandidateIdentitiesV3 {
            schema_version: crate::grade_candidates::SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3,
            per_detector: per_detector
                .into_iter()
                .map(|(detector, candidates)| DetectorCandidateIdentitiesV2 {
                    detector,
                    candidates,
                    provenance: Vec::new(),
                    ungradable: Vec::new(),
                })
                .collect(),
            combined_candidates: combined,
            combined_ungradable: Vec::new(),
        }
    }

    fn section(raw_ordinal: u64, execution_domain: ExecutionDomain) -> AnswerSectionV1 {
        AnswerSectionV1 {
            raw_ordinal,
            name: format!("section_{raw_ordinal}"),
            execution_domain,
            rom_start: ROM,
            vram_start: VA,
            size: 0x100,
        }
    }

    fn function(
        raw_ordinal: u64,
        section_raw_ordinal: u64,
        offset: u32,
        size: u32,
        kind: AnswerRowKind,
    ) -> AnswerFunctionV1 {
        AnswerFunctionV1 {
            raw_ordinal,
            section_raw_ordinal,
            name: format!("function_{raw_ordinal}"),
            vram: VA + offset,
            size,
            kind,
        }
    }

    fn reason(row: &AnswerAttributionV1) -> Option<MissReasonV1> {
        match row.status {
            AnswerAttributionStatusV1::Missed { primary_reason } => Some(primary_reason),
            _ => None,
        }
    }

    #[test]
    fn builder_streams_and_deduplicates_cold_facts() {
        let mut facts = FactDb::new();
        let map = facts.insert(Fact::RomMapping {
            bank: "code".to_owned(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM,
            rom_end: ROM + 0x100,
            va_start: VA,
            va_end: VA + 0x100,
        });
        facts
            .conclude("bank:code", ProofState::Proven, vec![map], "test")
            .unwrap();
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: BankAddr::new("code", VA + 4),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("code", VA),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&BankAddr::new("code", VA + 4)),
                ProofState::Supported,
                vec![claim],
                "test",
            )
            .unwrap();
        facts.insert(Fact::DirectCall {
            source: BankAddr::new("code", VA),
            target: BankAddr::new("code", VA + 4),
        });
        let snapshot = ProgramSnapshotV1 {
            schema_version: PROGRAM_SNAPSHOT_SCHEMA_V6,
            normalized_rom_sha256: "00".repeat(32),
            coverage: crate::coverage::report(0, &facts),
            facts,
            banks: Vec::new(),
        };
        let mut builder = ColdAttributionIndexBuilder::new();
        builder.ingest_snapshot(&snapshot).unwrap();
        builder.ingest_snapshot(&snapshot).unwrap();
        let index = builder.finalize().unwrap();
        assert_eq!(index.mappings.len(), 1);
        assert_eq!(index.claims.len(), 1);
        assert_eq!(index.resolve(ROM + 4, VA + 4).len(), 1);
        assert_eq!(
            index
                .relations
                .get(&CompactAddr {
                    bank: 0,
                    pc: VA + 4
                })
                .unwrap(),
            &BTreeSet::from([RelationKindV1::Direct])
        );
    }

    #[test]
    fn aliases_and_zero_markers_remain_raw_but_not_misses() {
        let index = base_index();
        let functions = vec![
            function(0, 0, 0, 0x10, AnswerRowKind::Function),
            // Raw dumps can disagree on alias extents. Identity and the
            // denominator remain keyed by (raw ROM, VA), never size.
            function(1, 0, 0, 8, AnswerRowKind::Alias),
            function(2, 0, 0x10, 0, AnswerRowKind::ZeroSizeMarker),
        ];
        let report = attribute_known_functions(
            &index,
            &identities(vec![addressed(RomAddressSpace::Physical, 0)], vec![]),
            &[section(0, ExecutionDomain::Vr4300)],
            &functions,
        )
        .unwrap();
        assert_eq!(report.totals.raw_rows, 3);
        assert_eq!(report.totals.nonzero_rows, 2);
        assert_eq!(report.totals.distinct_bodies, 1);
        assert_eq!(report.totals.alias_rows, 1);
        assert_eq!(report.totals.marker_rows, 1);
        assert_eq!(report.totals.candidate_matched_rows, 2);
        assert_eq!(report.totals.missed_rows, 0);
        assert!(matches!(
            report.rows[2].status,
            AnswerAttributionStatusV1::NotDiscoverableMarker
        ));
    }

    #[test]
    fn every_primary_reason_has_fixed_precedence() {
        let mut index = base_index();
        index
            .mappings
            .push(mapping(0, RomAddressSpace::Virtual, ROM + 4, VA + 4, 4));
        index.mapping_prefix_max_end.push(ROM + 0x100);
        index.claims.insert(
            CompactAddr {
                bank: 0,
                pc: VA + 8,
            },
            BTreeMap::from([(
                CandidateDetector::ProloguePattern,
                BTreeSet::from([ProofState::Candidate]),
            )]),
        );
        index.words.insert(
            CompactAddr {
                bank: 0,
                pc: VA + 12,
            },
            BTreeSet::from([WordClass::ProvenCode]),
        );
        index.words.insert(
            CompactAddr {
                bank: 0,
                pc: VA + 16,
            },
            BTreeSet::from([WordClass::CandidateCode]),
        );
        index.relations.insert(
            CompactAddr {
                bank: 0,
                pc: VA + 20,
            },
            BTreeSet::from([RelationKindV1::Table]),
        );
        let candidates = identities(
            vec![addressed(RomAddressSpace::Physical, 28)],
            vec![(
                CandidateDetector::ProloguePattern,
                vec![addressed(RomAddressSpace::Physical, 8)],
            )],
        );
        let mut unmapped_section = section(1, ExecutionDomain::Unknown);
        unmapped_section.rom_start = 0x9000;
        unmapped_section.vram_start = 0x9000_0000;
        unmapped_section.size = 4;
        let rows = vec![
            AnswerFunctionV1 {
                raw_ordinal: 0,
                section_raw_ordinal: 1,
                name: "no_mapping".to_owned(),
                vram: 0x9000_0000,
                size: 4,
                kind: AnswerRowKind::Function,
            },
            function(1, 0, 4, 4, AnswerRowKind::Function),
            function(2, 0, 8, 4, AnswerRowKind::Function),
            function(3, 0, 12, 4, AnswerRowKind::Function),
            function(4, 0, 16, 4, AnswerRowKind::Function),
            function(5, 0, 20, 4, AnswerRowKind::Function),
            function(6, 0, 24, 4, AnswerRowKind::Function),
            function(7, 0, 28, 4, AnswerRowKind::Function),
        ];
        let report = attribute_known_functions(
            &index,
            &candidates,
            &[section(0, ExecutionDomain::Vr4300), unmapped_section],
            &rows,
        )
        .unwrap();
        assert_eq!(reason(&report.rows[0]), Some(MissReasonV1::NoMapping));
        assert_eq!(
            reason(&report.rows[1]),
            Some(MissReasonV1::AmbiguousMapping)
        );
        assert_eq!(
            reason(&report.rows[2]),
            Some(MissReasonV1::ExactCandidateNotPromoted)
        );
        assert_eq!(
            reason(&report.rows[3]),
            Some(MissReasonV1::ProvenCodeNoEntry)
        );
        assert_eq!(
            reason(&report.rows[4]),
            Some(MissReasonV1::CandidateCodeNoEntry)
        );
        assert_eq!(reason(&report.rows[5]), Some(MissReasonV1::MappedUnreached));
        assert_eq!(reason(&report.rows[6]), Some(MissReasonV1::NoRelation));
        assert!(matches!(
            report.rows[7].status,
            AnswerAttributionStatusV1::CandidateMatched
        ));
    }

    #[test]
    fn candidate_false_taxonomy_is_rom_address_space_safe() {
        let index = base_index();
        let candidates = vec![
            addressed(RomAddressSpace::Physical, 0),
            addressed(RomAddressSpace::Physical, 4),
            addressed(RomAddressSpace::Physical, 0x20),
            addressed(RomAddressSpace::Physical, 0x200),
            addressed(RomAddressSpace::Virtual, 0),
        ];
        let report = attribute_known_functions(
            &index,
            &identities(candidates, vec![]),
            &[section(0, ExecutionDomain::Vr4300)],
            &[function(0, 0, 0, 0x10, AnswerRowKind::Function)],
        )
        .unwrap();
        assert_eq!(
            report
                .candidate_statuses
                .iter()
                .map(|row| row.status)
                .collect::<Vec<_>>(),
            vec![
                CandidateStatusV1::CandidateMatched,
                CandidateStatusV1::Interior,
                CandidateStatusV1::Gap,
                CandidateStatusV1::Outside,
                CandidateStatusV1::Outside,
            ]
        );
    }

    #[test]
    fn strict_report_validator_rejects_rehashed_cold_taxonomy_mutations() {
        let index = base_index();
        let identities = identities(
            vec![
                addressed(RomAddressSpace::Physical, 0),
                addressed(RomAddressSpace::Physical, 4),
                addressed(RomAddressSpace::Physical, 0x20),
                addressed(RomAddressSpace::Physical, 0x200),
            ],
            vec![],
        );
        let sections = [section(0, ExecutionDomain::Vr4300)];
        let functions = [function(0, 0, 0, 0x10, AnswerRowKind::Function)];
        let report = attribute_known_functions(&index, &identities, &sections, &functions).unwrap();
        assert!(validate_attribution_report_against_cold_v1(
            &report,
            &index,
            &identities,
            &sections,
            &functions,
        )
        .is_ok());

        let mut interior_as_outside = report.clone();
        interior_as_outside.candidate_statuses[1].status = CandidateStatusV1::Outside;
        interior_as_outside.candidate_totals.interior -= 1;
        interior_as_outside.candidate_totals.outside += 1;
        interior_as_outside.canonical_sha256 =
            canonical_attribution_report_digest(&interior_as_outside).unwrap();
        assert!(validate_attribution_report_v1(&interior_as_outside).is_ok());
        assert!(validate_attribution_report_against_cold_v1(
            &interior_as_outside,
            &index,
            &identities,
            &sections,
            &functions,
        )
        .is_err());

        let mut gap_as_interior = report.clone();
        gap_as_interior.candidate_statuses[2].status = CandidateStatusV1::Interior;
        gap_as_interior.candidate_totals.gap -= 1;
        gap_as_interior.candidate_totals.interior += 1;
        gap_as_interior.canonical_sha256 =
            canonical_attribution_report_digest(&gap_as_interior).unwrap();
        assert!(validate_attribution_report_v1(&gap_as_interior).is_ok());
        assert!(validate_attribution_report_against_cold_v1(
            &gap_as_interior,
            &index,
            &identities,
            &sections,
            &functions,
        )
        .is_err());
    }

    #[test]
    fn candidate_union_retains_detector_only_ungradable_and_sources() {
        let index = base_index();
        let combined_ungradable = BankAddr::new("combined_missing", VA);
        let detector_ungradable = BankAddr::new("detector_missing", VA + 4);
        let source = addressed(RomAddressSpace::Physical, 0x40);
        let detector_only = addressed(RomAddressSpace::Physical, 4);
        let identities = ScopedCandidateIdentitiesV3 {
            schema_version: crate::grade_candidates::SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3,
            per_detector: vec![DetectorCandidateIdentitiesV2 {
                detector: CandidateDetector::JalTarget,
                candidates: vec![addressed(RomAddressSpace::Physical, 0), detector_only],
                provenance: vec![crate::grade_candidates::CandidatePhysicalProvenanceV2 {
                    candidate: detector_only,
                    sources: vec![source],
                }],
                ungradable: vec![detector_ungradable.clone()],
            }],
            combined_candidates: vec![addressed(RomAddressSpace::Physical, 0)],
            combined_ungradable: vec![combined_ungradable.clone()],
        };
        let report = attribute_known_functions(
            &index,
            &identities,
            &[section(0, ExecutionDomain::Vr4300)],
            &[function(0, 0, 0, 0x10, AnswerRowKind::Function)],
        )
        .unwrap();
        assert_eq!(report.candidate_totals.denominator, 4);
        assert_eq!(report.candidate_totals.gradable, 2);
        assert_eq!(report.candidate_totals.ungradable, 2);
        assert_eq!(report.candidate_totals.combined, 2);
        assert_eq!(report.candidate_totals.per_detector_only, 2);
        let detector_row = report
            .candidate_statuses
            .iter()
            .find(|row| {
                row.identity
                    == CandidateAccountingIdentityV1::Addressed {
                        entry: detector_only,
                    }
            })
            .unwrap();
        assert!(!detector_row.combined);
        assert_eq!(detector_row.detectors, vec![CandidateDetector::JalTarget]);
        assert_eq!(detector_row.detector_sources[0].sources, vec![source]);
        assert!(report.candidate_statuses.iter().any(|row| {
            row.identity
                == CandidateAccountingIdentityV1::Ungradable {
                    address: combined_ungradable.clone(),
                }
                && row.combined
                && row.status == CandidateStatusV1::Ungradable
        }));
        assert!(report.candidate_statuses.iter().any(|row| {
            row.identity
                == CandidateAccountingIdentityV1::Ungradable {
                    address: detector_ungradable.clone(),
                }
                && !row.combined
                && row.status == CandidateStatusV1::Ungradable
        }));
    }

    #[test]
    fn ambiguous_answer_mapping_never_mints_candidate_match() {
        let mut index = base_index();
        index
            .mappings
            .push(mapping(0, RomAddressSpace::Virtual, ROM, VA, 0x100));
        index.mapping_prefix_max_end.push(ROM + 0x100);
        let candidates = identities(
            vec![
                addressed(RomAddressSpace::Physical, 0),
                addressed(RomAddressSpace::Virtual, 0),
            ],
            vec![],
        );
        let report = attribute_known_functions(
            &index,
            &candidates,
            &[section(0, ExecutionDomain::Vr4300)],
            &[function(0, 0, 0, 4, AnswerRowKind::Function)],
        )
        .unwrap();
        assert_eq!(
            reason(&report.rows[0]),
            Some(MissReasonV1::AmbiguousMapping)
        );
        assert_eq!(report.candidate_totals.candidate_matched, 0);
        assert_eq!(report.candidate_totals.ambiguous_answer_mapping, 2);
        assert!(report
            .candidate_statuses
            .iter()
            .all(|row| row.status == CandidateStatusV1::AmbiguousAnswerMapping));
    }

    #[test]
    fn gap_taxonomy_stops_at_actual_cold_mapping_end() {
        let mut index = base_index();
        index.mappings = vec![mapping(0, RomAddressSpace::Physical, ROM, VA, 4)];
        index.mapping_prefix_max_end = vec![ROM + 4];
        let report = attribute_known_functions(
            &index,
            &identities(
                vec![
                    addressed(RomAddressSpace::Physical, 0),
                    addressed(RomAddressSpace::Physical, 0x20),
                ],
                vec![],
            ),
            &[section(0, ExecutionDomain::Vr4300)],
            &[function(0, 0, 0, 4, AnswerRowKind::Function)],
        )
        .unwrap();
        assert_eq!(
            report.candidate_statuses[0].status,
            CandidateStatusV1::CandidateMatched
        );
        assert_eq!(
            report.candidate_statuses[1].status,
            CandidateStatusV1::Outside
        );
    }

    #[test]
    fn mapping_lookup_prefix_keeps_long_overlap_visible() {
        let mut index = base_index();
        index
            .mappings
            .push(mapping(0, RomAddressSpace::Virtual, ROM + 4, VA + 4, 4));
        index.mapping_prefix_max_end = vec![ROM + 0x100, ROM + 0x100];
        let resolved = index.resolve(ROM + 0x80, VA + 0x80);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].1.rom_space, RomAddressSpace::Physical);
    }

    #[test]
    fn report_is_deterministic_under_input_order_changes() {
        let index = base_index();
        let candidates = identities(
            vec![
                addressed(RomAddressSpace::Physical, 0x20),
                addressed(RomAddressSpace::Physical, 0),
            ],
            vec![],
        );
        let functions = vec![
            function(8, 0, 0x20, 4, AnswerRowKind::Function),
            function(2, 0, 0, 4, AnswerRowKind::Function),
        ];
        let mut unused = section(1, ExecutionDomain::Unknown);
        unused.rom_start = 0x4000;
        unused.vram_start = 0x9000_0000;
        let sections = vec![section(0, ExecutionDomain::Rsp), unused];
        let first = attribute_known_functions(&index, &candidates, &sections, &functions).unwrap();
        let mut reversed = functions;
        reversed.reverse();
        let mut reversed_sections = sections;
        reversed_sections.reverse();
        let second =
            attribute_known_functions(&index, &candidates, &reversed_sections, &reversed).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.canonical_sha256.len(), 64);
    }

    #[test]
    fn finalization_canonicalizes_bank_interning_order() {
        fn built(order: [&str; 2]) -> ColdAttributionIndex {
            let mut builder = ColdAttributionIndexBuilder::new();
            for (index, bank) in order.into_iter().enumerate() {
                let id = builder.intern_bank(bank).unwrap();
                let offset = u32::try_from(index).unwrap() * 4;
                builder.mappings.insert(mapping(
                    id,
                    RomAddressSpace::Physical,
                    ROM + offset,
                    VA + offset,
                    4,
                ));
            }
            builder.finalize().unwrap()
        }
        let first = built(["z_bank", "a_bank"]);
        let second = built(["a_bank", "z_bank"]);
        assert_eq!(first.banks, second.banks);
        assert_eq!(first.banks, vec!["a_bank", "z_bank"]);
    }

    #[test]
    fn checked_extents_fail_loudly() {
        let index = base_index();
        let mut overflowing = section(0, ExecutionDomain::Cic);
        overflowing.rom_start = u32::MAX;
        overflowing.size = 2;
        assert!(matches!(
            attribute_known_functions(&index, &identities(vec![], vec![]), &[overflowing], &[]),
            Err(AttributionError::ArithmeticOverflow {
                context: "section ROM extent",
                ..
            })
        ));
    }
