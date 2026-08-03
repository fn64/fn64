    use super::*;

    const MODEL_SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[cfg(feature = "writer-runtime-authority")]
    fn si_authority(
        program_model_sha256: &str,
        evidence_valid: bool,
    ) -> SiDmaCompletionAuthorityV2 {
        SiDmaCompletionAuthorityV2 {
            evidence_valid,
            validator_schema: fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
                .to_owned(),
            program_model_sha256: program_model_sha256.to_owned(),
            series_authority_sha256:
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned(),
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn sp_authority(
        program_model_sha256: &str,
        evidence_valid: bool,
    ) -> SpDmaCompletionAuthorityV2 {
        SpDmaCompletionAuthorityV2 {
            evidence_valid,
            validator_schema: fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
                .to_owned(),
            program_model_sha256: program_model_sha256.to_owned(),
            series_authority_sha256:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    fn writer_audit_bundle_authority(
        completed_channels: u8,
        program_model_sha256: &str,
    ) -> WriterAuditBundleCompletionAuthorityV2 {
        let completions = [
            (
                fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
                WriterChannelV2::BootstrapOrImport,
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
                WriterChannelV2::CpuInstructionStore,
                "34567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                WriterChannelV2::HostAbi,
                "567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                WriterChannelV2::PiDma,
                "4567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef123",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
                WriterChannelV2::RdpRenderer,
                "67890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12345",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
                WriterChannelV2::RspExecutionOrHleWriteback,
                "7890abcdef1234567890abcdef1234567890abcdef1234567890abcdef123456",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                WriterChannelV2::SiDma,
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
            (
                fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1,
                WriterChannelV2::SpDma,
                "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            ),
        ]
        .into_iter()
        .filter_map(|(bit, channel, series_authority_sha256)| {
            (completed_channels & bit != 0).then(|| WriterAuditBundleRowCompletionV2 {
                channel,
                program_model_sha256: program_model_sha256.to_owned(),
                series_authority_sha256: series_authority_sha256.to_owned(),
            })
        })
        .collect();
        WriterAuditBundleCompletionAuthorityV2 {
            evidence_valid: true,
            validator_schema:
                fn64_boot_harness::VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
                    .to_owned(),
            bundle_authority_sha256:
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_owned(),
            completed_channels,
            completions,
        }
    }

    fn input() -> WriterFrontierMatrixInputV2 {
        WriterFrontierMatrixInputV2 {
            producer: "fn64-test".to_string(),
            program_model_sha256: MODEL_SHA.to_string(),
            classes: WRITER_CLASSES_V2
                .into_iter()
                .rev()
                .map(|class| OpenWriterClassInputV2 {
                    class,
                    blockers: vec![WriterClassBlockerV2 {
                        code: WriterClassBlockerCodeV2::ValidatorUnavailable,
                        evidence: format!("validator for {class:?} is not implemented"),
                    }],
                })
                .collect(),
        }
    }

    fn channel_input() -> WriterChannelDenominatorInputV2 {
        WriterChannelDenominatorInputV2 {
            producer: "fn64-test".to_string(),
            program_model_sha256: MODEL_SHA.to_string(),
            channels: WRITER_CHANNELS_V2
                .into_iter()
                .rev()
                .map(|channel| OpenWriterChannelInputV2 {
                    channel,
                    blockers: vec![WriterChannelBlockerV2 {
                        code: WriterChannelBlockerCodeV2::MutableApiEscape,
                        evidence: format!("mutation channel {channel:?} is not sealed"),
                    }],
                })
                .collect(),
        }
    }

    #[test]
    fn production_constructor_requires_and_derives_all_fourteen_open_classes() {
        let receipt = WriterFrontierMatrixV2::new_open(input()).unwrap();
        assert_eq!(receipt.open_classes(), WRITER_CLASSES_V2);
        assert!(!receipt.is_complete());

        let json: serde_json::Value =
            serde_json::from_slice(&receipt.canonical_json_bytes().unwrap()).unwrap();
        assert_eq!(json["schema"], EXECUTABLE_WRITER_FRONTIER_MATRIX_SCHEMA_V2);
        assert_eq!(json["classes"].as_array().unwrap().len(), 14);
        assert!(json["classes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "open"));
    }

    #[test]
    fn missing_and_duplicate_classes_fail_closed() {
        let mut missing = input();
        let absent = missing.classes.pop().unwrap().class;
        assert_eq!(
            WriterFrontierMatrixV2::new_open(missing),
            Err(WriterFrontierMatrixErrorV2::MissingClass { class: absent })
        );

        let mut duplicate = input();
        let class = duplicate.classes[0].class;
        duplicate.classes.push(duplicate.classes[0].clone());
        assert_eq!(
            WriterFrontierMatrixV2::new_open(duplicate),
            Err(WriterFrontierMatrixErrorV2::DuplicateClass { class })
        );
    }

    #[test]
    fn open_rows_require_named_evidence() {
        let mut empty = input();
        let class = empty.classes[0].class;
        empty.classes[0].blockers.clear();
        assert_eq!(
            WriterFrontierMatrixV2::new_open(empty),
            Err(WriterFrontierMatrixErrorV2::EmptyBlockers { class })
        );

        let mut unnamed = input();
        let class = unnamed.classes[0].class;
        unnamed.classes[0].blockers[0].evidence = "  ".to_string();
        assert_eq!(
            WriterFrontierMatrixV2::new_open(unnamed),
            Err(WriterFrontierMatrixErrorV2::EmptyBlockerEvidence { class })
        );
    }

    #[test]
    fn canonical_form_ignores_input_and_blocker_order() {
        let mut reordered = input();
        reordered.classes.reverse();
        for row in &mut reordered.classes {
            row.blockers.push(WriterClassBlockerV2 {
                code: WriterClassBlockerCodeV2::CoverageOpen,
                evidence: "coverage denominator remains open".to_string(),
            });
            row.blockers.reverse();
        }
        let mut equivalent = reordered.clone();
        equivalent.classes.reverse();
        for row in &mut equivalent.classes {
            row.blockers.reverse();
        }

        let first = WriterFrontierMatrixV2::new_open(reordered).unwrap();
        let second = WriterFrontierMatrixV2::new_open(equivalent).unwrap();
        assert_eq!(
            first.canonical_json_bytes().unwrap(),
            second.canonical_json_bytes().unwrap()
        );
        assert_eq!(
            first.canonical_sha256().unwrap(),
            second.canonical_sha256().unwrap()
        );
    }

    #[test]
    fn frontier_completion_exists_only_behind_the_private_validator_seam() {
        let receipt = WriterFrontierMatrixV2::complete_for_test("fn64-test", MODEL_SHA);
        assert!(receipt.is_complete());
        assert!(receipt.open_classes().is_empty());
        let json: serde_json::Value =
            serde_json::from_slice(&receipt.canonical_json_bytes().unwrap()).unwrap();
        assert!(json["classes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "complete"));
        let cpu_copy = json["classes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["class"] == "cpu_copy_store_or_decompression")
            .unwrap();
        let split = &cpu_copy["receipt"]["receipt"];
        assert!(split.get("effect_closure_certificate").is_some());
        assert!(split.get("evaluated_image_block_harvest").is_some());
        assert!(split.get("class_completeness_aggregation").is_some());
    }

    #[test]
    fn semantic_channels_are_a_distinct_exact_denominator() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(denominator.open_channels(), WRITER_CHANNELS_V2);
        assert!(!denominator.is_complete());
        assert_eq!(WRITER_CHANNELS_V2.len(), 8);
        assert_eq!(WRITER_CLASSES_V2.len(), 14);

        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        assert_eq!(
            json["schema"],
            EXECUTABLE_WRITER_CHANNEL_DENOMINATOR_SCHEMA_V2
        );
        assert_eq!(json["channels"].as_array().unwrap().len(), 8);
        assert!(json.get("classes").is_none());
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_api_requires_the_move_only_bundle_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerWriterAuditBundleV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_writer_audit_bundle;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_closes_exactly_its_represented_rows() {
        let completed = fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SP_COMPLETED_V1;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                completed, MODEL_SHA,
            ))
            .unwrap();
        assert!(denominator.open_channels().is_empty());
        assert!(denominator.is_complete());
        for channel in [
            WriterChannelV2::BootstrapOrImport,
            WriterChannelV2::CpuInstructionStore,
            WriterChannelV2::HostAbi,
            WriterChannelV2::PiDma,
            WriterChannelV2::RdpRenderer,
            WriterChannelV2::RspExecutionOrHleWriteback,
            WriterChannelV2::SiDma,
            WriterChannelV2::SpDma,
        ] {
            assert!(!denominator.open_channels().contains(&channel));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        for channel in [
            "bootstrap_or_import",
            "cpu_instruction_store",
            "host_abi",
            "pi_dma",
            "rdp_renderer",
            "rsp_execution_or_hle_writeback",
            "si_dma",
            "sp_dma",
        ] {
            let row = json["channels"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["channel"] == channel)
                .unwrap();
            assert_eq!(row["receipt"]["validator"], "writer_audit_bundle");
            assert_eq!(
                row["receipt"]["receipt"]["bundle_validator_schema"],
                fn64_boot_harness::VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
            );
            let receipt = row["receipt"]["receipt"].as_object().unwrap();
            assert_eq!(receipt.len(), 3);
            assert!(receipt.contains_key("bundle_authority_sha256"));
            assert!(receipt.contains_key("channel_series_authority_sha256"));
        }
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_can_close_a_strict_subset() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::BootstrapOrImport));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::SpDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::CpuInstructionStore));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_preflights_models_and_rows_before_any_completion() {
        let completed = fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
            | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    completed,
                    &"30".repeat(32),
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::CpuInstructionStore,
                expected: MODEL_SHA.to_owned(),
                actual: "30".repeat(32),
            }
        );

        let mut pi_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        pi_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::PiDma)
            .unwrap()
            .program_model_sha256 = "31".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(pi_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::PiDma,
                expected: MODEL_SHA.to_owned(),
                actual: "31".repeat(32),
            }
        );

        let mut host_abi_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        host_abi_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::HostAbi)
            .unwrap()
            .program_model_sha256 = "32".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(host_abi_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::HostAbi,
                expected: MODEL_SHA.to_owned(),
                actual: "32".repeat(32),
            }
        );

        let mut rdp_renderer_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        rdp_renderer_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::RdpRenderer)
            .unwrap()
            .program_model_sha256 = "33".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rdp_renderer_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::RdpRenderer,
                expected: MODEL_SHA.to_owned(),
                actual: "33".repeat(32),
            }
        );

        let mut rsp_model_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        rsp_model_mismatch
            .completions
            .iter_mut()
            .find(|completion| completion.channel == WriterChannelV2::RspExecutionOrHleWriteback)
            .unwrap()
            .program_model_sha256 = "34".repeat(32);
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rsp_model_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleProgramModelMismatch {
                channel: WriterChannelV2::RspExecutionOrHleWriteback,
                expected: MODEL_SHA.to_owned(),
                actual: "34".repeat(32),
            }
        );

        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    completed, MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::SiDma,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_malformed_shape_before_row_mutation() {
        let mut malformed = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
            MODEL_SHA,
        );
        malformed.completed_channels |= 0x80;
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(malformed)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut invalid_evidence = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        invalid_evidence.evidence_valid = false;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(invalid_evidence)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_cpu = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_cpu.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_cpu)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_pi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_pi.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_pi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_host_abi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_host_abi.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_host_abi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_rdp_renderer = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_rdp_renderer.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_rdp_renderer)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut missing_rsp = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        missing_rsp.completions.clear();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(missing_rsp)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut pi_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        pi_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(pi_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut host_abi_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        host_abi_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(host_abi_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut rdp_renderer_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        rdp_renderer_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rdp_renderer_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut rsp_bitmap_mismatch = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        rsp_bitmap_mismatch.completions[0].channel = WriterChannelV2::CpuInstructionStore;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(rsp_bitmap_mismatch)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_cpu = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_cpu.completions[1] = duplicate_cpu.completions[0].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_cpu)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_pi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_pi.completions[0] = duplicate_pi.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_pi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_host_abi = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_host_abi.completions[0] = duplicate_host_abi.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_host_abi)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_rdp_renderer = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_rdp_renderer.completions[0] = duplicate_rdp_renderer.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_rdp_renderer)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );

        let mut duplicate_rsp = writer_audit_bundle_authority(
            fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                | fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
            MODEL_SHA,
        );
        duplicate_rsp.completions[0] = duplicate_rsp.completions[1].clone();
        assert_eq!(
            WriterChannelDenominatorV2::new_open(channel_input())
                .unwrap()
                .complete_writer_audit_bundle_authority(duplicate_rsp)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidWriterAuditBundle
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_an_already_complete_cpu_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_CPU_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::CpuInstructionStore,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_pi_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_PI_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::PiDma,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_host_abi_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_HOST_ABI_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::HostAbi,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_rdp_renderer_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::RdpRenderer,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn writer_audit_bundle_rejects_rsp_replay() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1,
                MODEL_SHA,
            ))
            .unwrap();
        assert_eq!(
            denominator
                .complete_writer_audit_bundle_authority(writer_audit_bundle_authority(
                    fn64_boot_harness::WRITER_AUDIT_RSP_COMPLETED_V1
                        | fn64_boot_harness::WRITER_AUDIT_SI_COMPLETED_V1,
                    MODEL_SHA,
                ))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::WriterAuditBundleRowAlreadyComplete {
                channel: WriterChannelV2::RspExecutionOrHleWriteback,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_api_requires_the_move_only_series_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerSiRuntimeSeriesV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_si;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_validated_authority_completes_only_its_exact_channel() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();

        assert_eq!(denominator.open_channels().len(), 7);
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(!denominator.is_complete());
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        let si = json["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["channel"] == "si_dma")
            .unwrap();
        assert_eq!(si["state"], "complete");
        assert_eq!(si["receipt"]["validator"], "si_dma");
        assert_eq!(
            si["receipt"]["receipt"]["validator_schema"],
            fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
        );
        assert_eq!(
            si["receipt"]["receipt"]["series_authority_sha256"],
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
        let serialized = String::from_utf8(denominator.canonical_json_bytes().unwrap()).unwrap();
        assert!(!serialized.contains("private_build_inputs"));
        assert!(!serialized.contains("selected_binary"));
        assert!(!serialized.contains("nonce_set"));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_invalid_capability_evidence() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(MODEL_SHA, false))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSiAuthority
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_a_different_program_model() {
        let actual = "10".repeat(32);
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(&actual, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SiProgramModelMismatch {
                expected: MODEL_SHA.to_owned(),
                actual,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn si_completion_rejects_an_already_complete_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_si_authority(si_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_si_authority(si_authority(MODEL_SHA, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SiRowAlreadyComplete
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_api_requires_the_move_only_series_capability() {
        let _: fn(
            WriterChannelDenominatorV2,
            fn64_boot_harness::VerifiedGeneratedRunnerSpRuntimeSeriesV1,
        ) -> Result<WriterChannelDenominatorV2, WriterChannelDenominatorErrorV2> =
            WriterChannelDenominatorV2::complete_sp;
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_validated_authority_completes_only_its_exact_channel() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_sp_authority(sp_authority(MODEL_SHA, true))
            .unwrap();

        assert_eq!(denominator.open_channels().len(), 7);
        assert!(!denominator
            .open_channels()
            .contains(&WriterChannelV2::SpDma));
        assert!(denominator
            .open_channels()
            .contains(&WriterChannelV2::SiDma));
        assert!(!denominator.is_complete());
        let json: serde_json::Value =
            serde_json::from_slice(&denominator.canonical_json_bytes().unwrap()).unwrap();
        let sp = json["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["channel"] == "sp_dma")
            .unwrap();
        assert_eq!(sp["state"], "complete");
        assert_eq!(sp["receipt"]["validator"], "sp_dma");
        assert_eq!(
            sp["receipt"]["receipt"]["validator_schema"],
            fn64_boot_harness::VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
        );
        assert_eq!(
            sp["receipt"]["receipt"]["series_authority_sha256"],
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        let serialized = String::from_utf8(denominator.canonical_json_bytes().unwrap()).unwrap();
        assert!(!serialized.contains("private_build_inputs"));
        assert!(!serialized.contains("selected_binary"));
        assert!(!serialized.contains("nonce_set"));
        assert!(!serialized.contains("sp_transition"));
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_invalid_capability_evidence() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(MODEL_SHA, false))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );

        let mut wrong_schema = sp_authority(MODEL_SHA, true);
        wrong_schema.validator_schema = "fn64.synthetic-sp-series.v1".to_owned();
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator.complete_sp_authority(wrong_schema).unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );

        let mut malformed_digest = sp_authority(MODEL_SHA, true);
        malformed_digest.series_authority_sha256 = "not-a-sha256".to_owned();
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(malformed_digest)
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::InvalidSpAuthority
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_a_different_program_model() {
        let actual = "20".repeat(32);
        let denominator = WriterChannelDenominatorV2::new_open(channel_input()).unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(&actual, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SpProgramModelMismatch {
                expected: MODEL_SHA.to_owned(),
                actual,
            }
        );
    }

    #[cfg(feature = "writer-runtime-authority")]
    #[test]
    fn sp_completion_rejects_an_already_complete_row() {
        let denominator = WriterChannelDenominatorV2::new_open(channel_input())
            .unwrap()
            .complete_sp_authority(sp_authority(MODEL_SHA, true))
            .unwrap();
        assert_eq!(
            denominator
                .complete_sp_authority(sp_authority(MODEL_SHA, true))
                .unwrap_err(),
            WriterChannelDenominatorErrorV2::SpRowAlreadyComplete
        );
    }

    #[test]
    fn missing_and_duplicate_semantic_channels_fail_closed() {
        let mut missing = channel_input();
        let absent = missing.channels.pop().unwrap().channel;
        assert_eq!(
            WriterChannelDenominatorV2::new_open(missing),
            Err(WriterChannelDenominatorErrorV2::MissingChannel { channel: absent })
        );

        let mut duplicate = channel_input();
        let channel = duplicate.channels[0].channel;
        duplicate.channels.push(duplicate.channels[0].clone());
        assert_eq!(
            WriterChannelDenominatorV2::new_open(duplicate),
            Err(WriterChannelDenominatorErrorV2::DuplicateChannel { channel })
        );
    }
