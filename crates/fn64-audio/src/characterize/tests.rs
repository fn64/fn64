    use super::*;

    const BREAK: u32 = 0x0000_000d;

    fn mtc0(rt: u32, rd: u32) -> u32 {
        (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11)
    }

    fn addiu(rt: u32, rs: u32, immediate: u16) -> u32 {
        (0x09 << 26) | (rs << 21) | (rt << 16) | u32::from(immediate)
    }

    fn lui(rt: u32, immediate: u16) -> u32 {
        (0x0f << 26) | (rt << 16) | u32::from(immediate)
    }

    fn sw(rt: u32, base: u32, immediate: u16) -> u32 {
        (0x2b << 26) | (base << 21) | (rt << 16) | u32::from(immediate)
    }

    fn sh(rt: u32, base: u32, immediate: u16) -> u32 {
        (0x29 << 26) | (base << 21) | (rt << 16) | u32::from(immediate)
    }

    fn public_fixture() -> (CharacterizationRequest, LoadedInputs) {
        public_fixture_with_text(&[0x2405_5678, BREAK])
    }

    fn public_fixture_with_text(text_words: &[u32]) -> (CharacterizationRequest, LoadedInputs) {
        let layout = CharacterizationLayout {
            task_address: 0x40,
            rspboot_address: 0x100,
            text_address: 0x200,
            data_address: 0x300,
            command_address: 0x400,
        };
        let text_bytes = text_words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        assert!(!text_bytes.is_empty() && text_bytes.len().is_multiple_of(8));
        let dma_descriptor = u16::try_from(text_bytes.len() - 1).unwrap();
        let boot_words = [
            0x2402_0000 | layout.text_address,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            addiu(4, 0, dma_descriptor),
            mtc0(4, 2),
            0x0800_0020,
            0x2407_7777,
        ];
        let loaded = LoadedInputs {
            rspboot: boot_words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect(),
            text: text_bytes,
            data: vec![1, 2, 3, 4],
        };
        let request = CharacterizationRequest {
            schema: REQUEST_SCHEMA.into(),
            fixture_revision: FIXTURE_REVISION,
            microcode: PrivateMicrocodePaths {
                rspboot_path: PathBuf::new(),
                rspboot_sha256: String::new(),
                text_path: PathBuf::new(),
                text_sha256: String::new(),
                data_path: PathBuf::new(),
                data_sha256: String::new(),
            },
            layout,
            cases: vec![CharacterizationCase {
                id: "public-smoke".into(),
                parameters: ExperimentParameters::Count {
                    opcode: 2,
                    count: 8,
                },
                sentinels: vec![SentinelRange {
                    start: 0x500,
                    byte_len: 16,
                    pattern_seed: 0x40,
                }],
                trials: vec![
                    CharacterizationTrial {
                        phases: vec![CharacterizationPhase {
                            packets: vec![PublicCommandPacket {
                                word0: 0xdead_beef,
                                word1: 0xcafe_babe,
                            }],
                        }],
                    },
                    CharacterizationTrial {
                        phases: vec![CharacterizationPhase {
                            packets: vec![PublicCommandPacket {
                                word0: 0xdead_beef,
                                word1: 0xcafe_babf,
                            }],
                        }],
                    },
                ],
            }],
        };
        (request, loaded)
    }

    fn public_compact_verification_fixture() -> (CharacterizationRequest, LoadedInputs) {
        let program = [
            lui(2, 0x0e00),
            sw(2, 0, 0x02b0),
            addiu(3, 0, 0x1234),
            sw(3, 0, 0x02b4),
            sh(3, 0, 0x0fea),
            BREAK,
            0,
            0,
        ];
        let (mut request, loaded) = public_fixture_with_text(&program);
        for trial in &mut request.cases[0].trials {
            trial.phases[0].packets = vec![PublicCommandPacket {
                word0: 0x0e00_0000,
                word1: 0x0000_1234,
            }];
        }
        (request, loaded)
    }

    #[test]
    fn compact_verifier_matches_public_synthetic_hle_and_lle() {
        let (request, loaded) = public_compact_verification_fixture();
        let first = verify_compact_loaded(request.clone(), loaded.clone()).unwrap();
        let second = verify_compact_loaded(request, loaded).unwrap();
        let first_json = serde_json::to_string(&first).unwrap();
        assert_eq!(first_json, serde_json::to_string(&second).unwrap());
        assert!(first_json.starts_with("{\"schema\":\"fn64.audio-compact-verification-report.v1\""));
        assert!(!first_json.contains("public-smoke"));
        assert!(!first_json.contains("234881024"));
        assert!(!first_json.contains("4660"));
        assert_eq!(first.phases.len(), 2);
        for phase in &first.phases {
            assert_eq!(phase.command_count, 1);
            assert_eq!(phase.decoded_commands, 1);
            assert!(phase.dmem_equivalent);
            assert!(phase.first_dmem_difference.is_none());
            assert!(phase.dmem_differences.is_empty());
            assert!(phase.rdram_patches_equivalent);
        }
    }

    #[test]
    fn public_fixture_is_deterministic_and_content_safe() {
        let (request, loaded) = public_fixture();
        let private_needles = loaded
            .rspboot
            .iter()
            .chain(&loaded.text)
            .chain(&loaded.data)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>();
        let first =
            canonical_report_json(&characterize_loaded(request.clone(), loaded.clone()).unwrap())
                .unwrap();
        let second = canonical_report_json(&characterize_loaded(request, loaded).unwrap()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("{\"schema\":\"fn64.audio-abi-characterization-report.v2\""));
        assert!(!first.contains("rspboot_path"));
        assert!(!first.contains("text_path"));
        assert!(!first.contains("public-smoke"));
        assert!(!first.contains("3735928559"));
        assert!(!first.contains("3405691582"));
        assert!(!first.contains("\"id\""));
        assert!(!first.contains("\"packets\""));
        assert!(!first.contains("\"parameters\""));
        assert!(!first.contains("\"layout\""));
        for needle in private_needles {
            assert!(
                !first.contains(&format!("\"{needle}\"")),
                "report serialized a raw one-byte string"
            );
        }
    }

    #[test]
    fn dma_journal_is_ordered_and_reports_raw_descriptor() {
        let (request, loaded) = public_fixture();
        let report = characterize_loaded(request, loaded).unwrap();
        let phase = &report.cases[0].trials[0].phases[0];
        assert_eq!(phase.rspboot_dma.len(), 1);
        assert_eq!(phase.rspboot_dma[0].direction, "read");
        assert_eq!(phase.rspboot_dma[0].effective_dram_address, 0x200);
        assert_eq!(phase.rspboot_dma[0].sp_mem_address, 0x1080);
        assert_eq!(phase.rspboot_dma[0].raw_length_descriptor, 7);
        assert!(phase.ucode_dma.is_empty());
    }

    #[test]
    fn trials_share_one_content_bound_baseline_and_compare_canonically() {
        let (request, loaded) = public_fixture();
        let report = characterize_loaded(request, loaded).unwrap();
        let case = &report.cases[0];
        assert_eq!(case.common_baseline_sha256.len(), 64);
        assert_eq!(case.trials.len(), 2);
        assert_eq!(
            case.trials[0].phases[0].entry_snapshot_sha256,
            case.trials[1].phases[0].entry_snapshot_sha256
        );
        assert_eq!(case.comparisons.len(), 1);
        assert_eq!(case.comparisons[0].reference_trial, 0);
        assert_eq!(case.comparisons[0].candidate_trial, 1);
        assert!(case.comparisons[0].phases[0].equivalent);
        assert!(case.comparisons[0].phases[0].first_divergence.is_none());
    }

    #[test]
    fn comparison_reports_exact_first_dmem_byte_without_serializing_values() {
        let (request, loaded) = public_fixture();
        let case = &request.cases[0];
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        let reference = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        let mut candidate = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        candidate.phases[0].dmem[0x321] ^= 0xa5;

        let comparison = compare_phase_execution(0, &reference.phases[0], &candidate.phases[0]);
        assert!(!comparison.equivalent);
        assert_eq!(
            comparison.first_divergence,
            Some(FirstDivergence {
                domain: "dmem_byte",
                index: None,
                address: Some(0x321),
            })
        );
        assert_eq!(comparison.dmem_differences.len(), 1);
        assert_eq!(comparison.dmem_differences[0].start, 0x321);
        assert_eq!(comparison.dmem_differences[0].byte_len, 1);
        let json = serde_json::to_string(&comparison).unwrap();
        assert!(!json.contains("reference_byte"));
        assert!(!json.contains("candidate_byte"));
    }

    #[test]
    fn comparison_uses_written_rdram_union_and_logical_guest_addresses() {
        let (request, loaded) = public_fixture();
        let case = &request.cases[0];
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        let mut reference = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        let mut candidate = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        let reference_bytes = [1, 2, 3, 4];
        let candidate_bytes = [1, 9, 3, 8];
        reference.phases[0].rdram_patches = crate::hle_outcome::CanonicalRdramPatches::new(vec![
            crate::hle_outcome::RdramPatch::new(0x600, reference_bytes.to_vec()).unwrap(),
        ])
        .unwrap();
        candidate.phases[0].rdram_patches = crate::hle_outcome::CanonicalRdramPatches::new(vec![
            crate::hle_outcome::RdramPatch::new(0x600, candidate_bytes.to_vec()).unwrap(),
        ])
        .unwrap();
        install_logical(
            &mut reference.phases[0].rdram_storage,
            0x600,
            &reference_bytes,
        );
        install_logical(
            &mut candidate.phases[0].rdram_storage,
            0x600,
            &candidate_bytes,
        );

        let comparison = compare_phase_execution(0, &reference.phases[0], &candidate.phases[0]);
        assert_eq!(
            comparison.first_divergence,
            Some(FirstDivergence {
                domain: "rdram_patch_byte",
                index: Some(0),
                address: Some(0x601),
            })
        );
        assert_eq!(
            comparison
                .rdram_differences
                .iter()
                .map(|range| (range.start, range.byte_len))
                .collect::<Vec<_>>(),
            vec![(0x601, 1), (0x603, 1)]
        );
    }

    #[test]
    fn comparison_retains_boot_effects_and_entry_provenance_independently() {
        let (request, loaded) = public_fixture();
        let case = &request.cases[0];
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        let reference = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        let mut candidate = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();

        candidate.phases[0].rspboot_rdram_patches =
            crate::hle_outcome::CanonicalRdramPatches::new(vec![
                crate::hle_outcome::RdramPatch::new(0x700, vec![0]).unwrap(),
            ])
            .unwrap();
        assert_eq!(
            first_phase_divergence(&reference.phases[0], &candidate.phases[0]),
            Some(FirstDivergence {
                domain: "rspboot_rdram_patch_range",
                index: Some(0),
                address: None,
            })
        );

        candidate.phases[0].rspboot_rdram_patches =
            crate::hle_outcome::CanonicalRdramPatches::new(vec![
                crate::hle_outcome::RdramPatch::new(0x700, vec![0xa5]).unwrap(),
            ])
            .unwrap();
        install_logical(&mut candidate.phases[0].rdram_storage, 0x700, &[0xa5]);
        let comparison = compare_phase_execution(0, &reference.phases[0], &candidate.phases[0]);
        assert_eq!(comparison.rdram_differences[0].start, 0x700);
        assert_eq!(comparison.rdram_differences[0].byte_len, 1);
        let mut reference_byte = [0];
        RdramView::from_storage(&reference.phases[0].rdram_storage)
            .copy_logical_bytes(RdramAddr::from_offset(0x700), &mut reference_byte);
        install_logical(
            &mut candidate.phases[0].rdram_storage,
            0x700,
            &reference_byte,
        );

        candidate.phases[0].rspboot_rdram_patches =
            reference.phases[0].rspboot_rdram_patches.clone();
        candidate.phases[0]
            .rspboot_dma_journal
            .push(RspDmaJournalEntry {
                direction: RspDmaDirection::Write,
                effective_dram_address: 0x700,
                sp_mem_address: 0,
                raw_length_descriptor: 0,
            });
        assert_eq!(
            first_phase_divergence(&reference.phases[0], &candidate.phases[0]),
            Some(FirstDivergence {
                domain: "rspboot_dma_journal",
                index: Some(reference.phases[0].rspboot_dma_journal.len()),
                address: None,
            })
        );

        candidate.phases[0].rspboot_dma_journal = reference.phases[0].rspboot_dma_journal.clone();
        candidate.phases[0].rspboot_imem_replacements.push(
            crate::hle_effects::AudioImemReplacement::from_image(
                99,
                [0; crate::hle_outcome::RSP_BANK_BYTES],
            ),
        );
        assert_eq!(
            first_phase_divergence(&reference.phases[0], &candidate.phases[0]),
            Some(FirstDivergence {
                domain: "rspboot_imem_replacement",
                index: Some(reference.phases[0].rspboot_imem_replacements.len()),
                address: None,
            })
        );

        candidate.phases[0].rspboot_imem_replacements =
            reference.phases[0].rspboot_imem_replacements.clone();
        candidate.phases[0].entry_snapshot = Sha256Digest::new([0xa5; 32]);
        assert_eq!(
            first_phase_divergence(&reference.phases[0], &candidate.phases[0]),
            Some(FirstDivergence {
                domain: "entry_snapshot",
                index: None,
                address: None,
            })
        );
    }

    #[test]
    fn comparison_retains_deferred_dpc_after_machine_snapshot_drain() {
        let (request, loaded) = public_fixture();
        let case = &request.cases[0];
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        let reference = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        let mut candidate = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        candidate.phases[0].deferred_dpc_submissions.push(
            DeferredDpcSubmission::from_rdram_words(0, 8, vec![0x0102_0304, 0x0506_0708]).unwrap(),
        );

        let comparison = compare_phase_execution(0, &reference.phases[0], &candidate.phases[0]);
        assert_eq!(
            comparison.first_divergence,
            Some(FirstDivergence {
                domain: "deferred_dpc_submission",
                index: Some(0),
                address: None,
            })
        );
        assert_ne!(
            comparison.reference_deferred_dpc_sha256,
            comparison.candidate_deferred_dpc_sha256
        );
        let observations = deferred_dpc_observations(&candidate.phases[0].deferred_dpc_submissions);
        let json = serde_json::to_string(&observations).unwrap();
        assert!(!json.contains("command_words"));
        assert!(!json.contains("payload"));
    }

    #[test]
    fn public_dpc_program_reaches_capture_report_and_comparison_through_lle() {
        let program = [
            addiu(2, 0, 2),
            mtc0(2, 11),
            addiu(3, 0, 0x100),
            mtc0(3, 8),
            addiu(4, 0, 0x108),
            mtc0(4, 9),
            BREAK,
            0,
        ];
        let (request, loaded) = public_fixture_with_text(&program);
        let case = &request.cases[0];
        let baseline = build_baseline_rdram(request.layout, &loaded, &case.sentinels);
        let execution = run_trial(
            request.layout,
            &loaded,
            &case.sentinels,
            &baseline,
            &case.trials[0],
        )
        .unwrap();
        assert_eq!(execution.phases[0].deferred_dpc_submissions.len(), 1);
        assert_eq!(
            execution.phases[0].deferred_dpc_submissions[0].source(),
            DpcSubmissionSource::Dmem
        );
        assert_eq!(
            execution.phases[0].deferred_dpc_submissions[0].start(),
            0x100
        );
        assert_eq!(execution.phases[0].deferred_dpc_submissions[0].end(), 0x108);
        assert!(
            execution.phases[0]
                .machine_state
                .architectural_state()
                .dp_submissions()
                .is_empty(),
            "the capture must retain the drained submission outside machine state"
        );

        let report = characterize_loaded(request, loaded).unwrap();
        let phase = &report.cases[0].trials[0].phases[0];
        assert_eq!(phase.deferred_dpc_submissions.len(), 1);
        assert_eq!(phase.deferred_dpc_submissions[0].source, "dmem");
        assert_eq!(phase.deferred_dpc_submissions[0].start, 0x100);
        assert_eq!(phase.deferred_dpc_submissions[0].end, 0x108);
        let comparison = &report.cases[0].comparisons[0].phases[0];
        assert!(comparison.equivalent);
        assert_eq!(
            comparison.reference_deferred_dpc_sha256,
            comparison.candidate_deferred_dpc_sha256
        );
    }

    #[test]
    fn speculative_lle_error_text_never_formats_private_context() {
        let text =
            content_safe_speculative_lle_error(SpeculativeAudioLleError::XbusCommandWordMismatch {
                index: 12345,
                payload_word: 0xdead_beef,
                raw_word: 0xcafe_babe,
            });
        assert_eq!(text, "speculative LLE: XBUS command word content mismatch");
        for forbidden in ["12345", "dead", "beef", "cafe", "babe"] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn rspboot_and_snapshot_errors_never_format_private_context() {
        let messages = [
            content_safe_rspboot_error(
                "rspboot input",
                AudioRspbootError::InitialExecutionContinuation {
                    jump_target: 0xdead_beef,
                    resume_address: 0xcafe_babe,
                    resume_delay: true,
                },
            ),
            content_safe_rspboot_error(
                "rspboot execution",
                AudioRspbootError::StaticAliasNotAllowed {
                    field: crate::hle_rspboot::RspbootHeaderRange::CommandList,
                    address: 0x7654_3210,
                    byte_len: 0x1234,
                },
            ),
            content_safe_rspboot_error(
                "rspboot execution",
                AudioRspbootError::EntrySnapshot(
                    AudioHleSnapshotError::MicrocodeIdentityMismatch {
                        component: crate::hle_snapshot::MicrocodeIdentityMismatch::ImemDigest {
                            selected: Sha256Digest::new([0xde; 32]),
                            captured: Sha256Digest::new([0xad; 32]),
                        },
                    },
                ),
            ),
            content_safe_rspboot_error(
                "rspboot execution",
                AudioRspbootError::EntrySnapshot(AudioHleSnapshotError::EntryPcResumeMismatch {
                    entry_pc_low12: 0xabc,
                    resume_pc_low12: 0xdef,
                }),
            ),
            content_safe_rsp_memory_error(
                "install task DMEM",
                RspMemoryError::CrossesBank {
                    addr: RspMemAddr::from_register(0x1fed),
                    len: 0x12345,
                },
            ),
        ];
        assert_eq!(messages[0], "rspboot input: initial execution continuation");
        assert_eq!(
            messages[2],
            "rspboot execution: entry snapshot IMEM identity mismatch"
        );
        for message in messages {
            for forbidden in [
                "dead", "beef", "cafe", "babe", "7654", "3210", "1234", "222", "173", "2748",
                "3567", "8173", "74565",
            ] {
                assert!(
                    !message.contains(forbidden),
                    "leaked {forbidden}: {message}"
                );
            }
        }
    }

    #[test]
    fn same_baseline_matrix_rejects_single_or_ragged_trials() {
        let (mut request, _) = public_fixture();
        request.cases[0].trials.pop();
        assert!(validate_request_header(&request).is_err());

        let (mut request, _) = public_fixture();
        request.cases[0].trials[1]
            .phases
            .push(CharacterizationPhase {
                packets: vec![PublicCommandPacket { word0: 0, word1: 0 }],
            });
        assert!(validate_request_header(&request).is_err());

        let (mut request, _) = public_fixture();
        request.cases[0].trials[1].phases[0]
            .packets
            .push(PublicCommandPacket { word0: 0, word1: 0 });
        assert!(validate_request_header(&request).is_err());
    }

    #[test]
    fn persistence_requires_multiple_matching_phases() {
        let (mut request, _) = public_fixture();
        request.cases[0].parameters = ExperimentParameters::Persistence {
            state: PersistenceState::Segment,
            task_count: 2,
        };
        assert!(validate_request_header(&request).is_err());
    }

    #[test]
    fn only_persistence_cases_can_carry_state_across_phases() {
        let (mut request, _) = public_fixture();
        let second = request.cases[0].trials[0].phases[0].clone();
        for trial in &mut request.cases[0].trials {
            trial.phases.push(second.clone());
        }
        assert!(validate_request_header(&request).is_err());
    }

    #[test]
    fn opaque_digests_bind_hidden_case_ids_and_packet_words() {
        let (request, loaded) = public_fixture();
        let original = characterize_loaded(request.clone(), loaded.clone()).unwrap();
        let mut changed = request;
        changed.cases[0].id = "different-private-label".into();
        changed.cases[0].trials[0].phases[0].packets[0].word1 ^= 1;
        let changed = characterize_loaded(changed, loaded).unwrap();
        assert_ne!(original.request_sha256, changed.request_sha256);
        assert_ne!(original.cases[0].case_sha256, changed.cases[0].case_sha256);
        assert_ne!(
            original.cases[0].trials[0].phases[0].phase_sha256,
            changed.cases[0].trials[0].phases[0].phase_sha256
        );
    }

    #[test]
    fn exact_digest_parser_rejects_non_sha256_shapes() {
        assert!(parse_digest("00").is_err());
        assert_eq!(
            parse_digest(&"ab".repeat(32)).unwrap(),
            Sha256Digest::new([0xab; 32])
        );
    }

    #[test]
    fn request_schema_names_every_predeclared_experiment_axis() {
        let json = r#"[
            {"kind":"address","opcode":1,"selector":2,"address":3,"alignment":4},
            {"kind":"selector","opcode":5,"selector":6},
            {"kind":"count","opcode":7,"count":8},
            {"kind":"dmem_move","input_dmem":9,"output_dmem":10,"count":11,"overlap":"forward"},
            {"kind":"aux","flags":8,"input_dmem":12,"output_dmem":13,"aux_a":14,"aux_c":15,"aux_e":16},
            {"kind":"reserved","opcode":17,"word0_reserved_mask":18,"word1_reserved_mask":19},
            {"kind":"persistence","state":"codebook","task_count":2}
        ]"#;
        let parsed: Vec<ExperimentParameters> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.len(), 7);
    }
