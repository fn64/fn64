
use super::*;

fn synthetic_claims() -> PreparedSourceClaimsV3 {
    PreparedSourceClaimsV3 {
        generator_source_sha256: "a1".repeat(32),
        discovery_source_sha256: "a2".repeat(32),
        emitter_source_sha256: "a3".repeat(32),
        runtime_source_sha256: "a4".repeat(32),
        materializer_source_sha256: "a5".repeat(32),
    }
}

fn synthetic_prepared_tree(
    changed_package: Option<&str>,
) -> (ScratchDirectory, PathBuf, PreparedSourceClaimsV3, String) {
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).unwrap();
    let scratch = ScratchDirectory::create(&nonce).unwrap();
    let root = scratch.path().join("prepared-test");
    fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let claims = synthetic_claims();
    let rom = "b1".repeat(32);
    let mut manifest = format!(
        concat!(
            "schema fn64.wm-prepared-shard-tree.v2\n",
            "normalized_rom_sha256 {}\n",
            "generator_source_sha256 {}\n",
            "discovery_source_sha256 {}\n",
            "emitter_source_sha256 {}\n",
            "runtime_source_sha256 {}\n",
            "artifact_count {}\n"
        ),
        rom,
        claims.generator_source_sha256,
        claims.discovery_source_sha256,
        claims.emitter_source_sha256,
        claims.runtime_source_sha256,
        PREPARED_PACKAGES.len(),
    );
    for package in PREPARED_PACKAGES {
        let package_root = root.join(package);
        fs::create_dir(&package_root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&package_root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut runner = format!("// runner {package}\n").into_bytes();
        if changed_package == Some(package) {
            runner.extend_from_slice(b"// changed\n");
        }
        let metadata = format!("// metadata {package}\n").into_bytes();
        let runner_sha = hex(&Sha256::digest(&runner));
        let metadata_sha = hex(&Sha256::digest(&metadata));
        let identity = format!(
            "schema fn64.wm-prepared-shard-artifact.v1\npackage {package}\nrunner_sha256 {runner_sha}\nmetadata_sha256 {metadata_sha}\n"
        )
        .into_bytes();
        for (name, bytes) in [
            ("identity.v1", identity.as_slice()),
            ("runner.rs", runner.as_slice()),
            ("metadata.rs", metadata.as_slice()),
        ] {
            let path = package_root.join(name);
            fs::write(&path, bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        manifest.push_str(&format!(
            "artifact {package} {} {runner_sha} {metadata_sha}\n",
            hex(&Sha256::digest(&identity)),
        ));
    }
    let manifest_path = root.join(PREPARED_MANIFEST_NAME);
    fs::write(&manifest_path, manifest).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(manifest_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    (scratch, root, claims, rom)
}

fn identity() -> GeneratedRunnerBuildIdentityV1 {
    let mut identity = GeneratedRunnerBuildIdentityV1 {
        schema: GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3.to_owned(),
        package: PACKAGE.to_owned(),
        manifest_sha256: "11".repeat(32),
        lock_sha256: "22".repeat(32),
        source_attestation_schema:
            fn64_recomp_rs::GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2.to_owned(),
        cargo_source_fields_validated: true,
        program_identity_sha256: "33".repeat(32),
        root_adapter_source_sha256: "44".repeat(32),
        shard_cargo_source_tree_sha256: "55".repeat(32),
        emitter_source_sha256: "66".repeat(32),
        runtime_source_sha256: "67".repeat(32),
        prepared_source_mode: PREPARED_SOURCE_MODE_INACTIVE_V1.to_owned(),
        normalized_rom_sha256: "68".repeat(32),
        prepared_manifest_sha256: "69".repeat(32),
        prepared_tree_sha256: "6a".repeat(32),
        prepared_generator_source_sha256: "6b".repeat(32),
        prepared_discovery_source_sha256: "6c".repeat(32),
        prepared_emitter_source_sha256: "6d".repeat(32),
        prepared_runtime_source_sha256: "6e".repeat(32),
        prepared_materializer_source_sha256: "6f".repeat(32),
        producer_manifest_sha256: "70".repeat(32),
        producer_lock_sha256: "71".repeat(32),
        producer_cargo_graph_sha256: "72".repeat(32),
        producer_cargo_source_sha256: "73".repeat(32),
        producer_binary_sha256: "74".repeat(32),
        binding_sha256: String::new(),
        build_receipt_schema: 1,
        aot_runtime: true,
        production_aot: true,
        dev_interpreter: false,
        runners: vec![GeneratedRunnerLinkedIdentityV1 {
            bank: 7,
            generated_runner_source_sha256: "77".repeat(32),
            code_words_sha256: "88".repeat(32),
            vram_start: 0x8000_0400,
            vram_end: 0x8000_0800,
            composite_subrunner_count: 1,
            adapter_role: GeneratedRunnerAdapterRoleV1::DirectGenerated,
        }],
    };
    identity.binding_sha256 = recompute_binding_sha256(&identity).unwrap();
    identity
}

fn bootstrap_prerequisite(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> BootstrapWriterRuntimePrerequisiteV1 {
    let mut prerequisite = BootstrapWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        bootstrap_receipt_sha256: "c2".repeat(32),
        rom_sha256: identity.normalized_rom_sha256.clone(),
        resolver_install_sha256: "c3".repeat(32),
        generation_catalog_sha256: "c4".repeat(32),
        watched_ranges: vec![BootstrapWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        bootstrap_watched_sha256: "c5".repeat(32),
        initial_generations: vec![1, 2],
        journal_entry: BootstrapMutationBatchV1 {
            sequence: 0,
            declared_writes: vec![BootstrapAttributedWriteV1 {
                channel: BootstrapWriterChannelV1::BootstrapOrImport,
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            changed_ranges: vec![BootstrapWriterWatchedRangeV1 {
                physical_start: 0x400,
                physical_end: 0x800,
            }],
            before_sha256: "c6".repeat(32),
            after_sha256: "c5".repeat(32),
            invalidated_generations: Vec::new(),
            journal_root_sha256: "c7".repeat(32),
        },
        final_watched_sha256: "c5".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.journal_entry.journal_root_sha256 =
        recompute_bootstrap_canonical_journal_root(
            &prerequisite.watched_ranges,
            &prerequisite.journal_entry,
        )
        .unwrap();
    prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn bootstrap_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerBootstrapRuntimeReportV1 {
    GeneratedRunnerBootstrapRuntimeReportV1 {
        schema: GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: bootstrap_prerequisite(identity),
    }
}

fn bootstrap_report_output(report: &GeneratedRunnerBootstrapRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn cpu_prerequisite(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> CpuWriterRuntimePrerequisiteV1 {
    let mut prerequisite = CpuWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::CPU_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "d2".repeat(32),
        abi_host_catalog_receipt_sha256: "d3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        watched_ranges: vec![CpuWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        journal_entry_count: 1,
        cpu_journal_declaration_count: 0,
        journal_root_sha256: "d4".repeat(32),
        final_watched_sha256: "d5".repeat(32),
        cpu_store_count: 3,
        cpu_store_trace_sha256: "d6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_cpu_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn cpu_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerCpuRuntimeReportV1 {
    GeneratedRunnerCpuRuntimeReportV1 {
        schema: GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: cpu_prerequisite(identity),
    }
}

fn cpu_report_output(report: &GeneratedRunnerCpuRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn host_abi_prerequisite(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> HostAbiWriterRuntimePrerequisiteV1 {
    let mut prerequisite = HostAbiWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "c2".repeat(32),
        abi_host_catalog_receipt_sha256: "c3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        initial_journal_entry_count: 1,
        final_journal_entry_count: 2,
        watched_ranges: vec![HostAbiWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        host_abi_journal_entry_count: 1,
        host_abi_journal_declaration_count: 1,
        journal_root_sha256: "c4".repeat(32),
        final_watched_sha256: "c5".repeat(32),
        transactions_started: 1,
        transactions_finished: 1,
        ordering_boundaries: 1,
        lifecycle_sha256: "c6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_host_abi_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn host_abi_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerHostAbiRuntimeReportV1 {
    GeneratedRunnerHostAbiRuntimeReportV1 {
        schema: GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: host_abi_prerequisite(identity),
    }
}

fn host_abi_report_output(report: &GeneratedRunnerHostAbiRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn pi_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> PiWriterRuntimePrerequisiteV1 {
    let mut prerequisite = PiWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::PI_WRITER_RUNTIME_STATE_SCHEMA_V2.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "e2".repeat(32),
        abi_host_catalog_receipt_sha256: "e3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        watched_ranges: vec![PiWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        journal_entry_count: 1,
        pi_journal_declaration_count: 0,
        journal_root_sha256: "e4".repeat(32),
        final_watched_sha256: "e5".repeat(32),
        pi_started: 1,
        pi_committed: 1,
        pi_busy_cleared: 1,
        pi_interrupt_raised: 1,
        pi_interrupt_cleared: 1,
        pi_notifications: 1,
        pi_to_rdram_committed: 1,
        pi_transition_sha256: "e6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_pi_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn pi_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerPiRuntimeReportV1 {
    GeneratedRunnerPiRuntimeReportV1 {
        schema: GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: pi_prerequisite(identity),
    }
}

fn pi_report_output(report: &GeneratedRunnerPiRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn rdp_renderer_prerequisite(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> RdpRendererWriterRuntimePrerequisiteV1 {
    let mut prerequisite = RdpRendererWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "f2".repeat(32),
        abi_host_catalog_receipt_sha256: "f3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        initial_journal_entry_count: 1,
        final_journal_entry_count: 2,
        watched_ranges: vec![RdpRendererWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        rdp_renderer_journal_entry_count: 1,
        rdp_renderer_journal_declaration_count: 1,
        journal_root_sha256: "f4".repeat(32),
        final_watched_sha256: "f5".repeat(32),
        renderer_publication_count: 1,
        publication_trace_sha256: "f6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_rdp_renderer_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn rdp_renderer_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerRdpRendererRuntimeReportV1 {
    GeneratedRunnerRdpRendererRuntimeReportV1 {
        schema: GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: rdp_renderer_prerequisite(identity),
    }
}

fn rdp_renderer_report_output(report: &GeneratedRunnerRdpRendererRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn rsp_prerequisite(
    identity: &GeneratedRunnerBuildIdentityV1,
) -> RspWriterRuntimePrerequisiteV1 {
    let mut prerequisite = RspWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::RSP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "d2".repeat(32),
        abi_host_catalog_receipt_sha256: "d3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        watched_ranges: vec![RspWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        journal_entry_count: 1,
        rsp_journal_declaration_count: 1,
        journal_root_sha256: "d4".repeat(32),
        final_watched_sha256: "d5".repeat(32),
        interpreter_writeback_count: 1,
        translated_audio_hle_publication_count: 0,
        writeback_range_count: 1,
        writeback_trace_sha256: "d6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_rsp_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn rsp_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerRspRuntimeReportV1 {
    GeneratedRunnerRspRuntimeReportV1 {
        schema: GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: rsp_prerequisite(identity),
    }
}

fn rsp_report_output(report: &GeneratedRunnerRspRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn si_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> SiWriterRuntimePrerequisiteV1 {
    let mut prerequisite = SiWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::SI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "a2".repeat(32),
        abi_host_catalog_receipt_sha256: "a3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        watched_ranges: vec![SiWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        journal_entry_count: 2,
        si_journal_declaration_count: 0,
        journal_root_sha256: "a4".repeat(32),
        final_watched_sha256: "a5".repeat(32),
        si_started: 1,
        si_committed: 1,
        si_pif_to_dram_committed: 1,
        si_transition_sha256: "a6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_si_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn si_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerSiRuntimeReportV1 {
    let identity_bytes = serde_json::to_vec(identity).unwrap();
    GeneratedRunnerSiRuntimeReportV1 {
        schema: GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(identity_bytes)),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: si_prerequisite(identity),
    }
}

fn si_report_output(report: &GeneratedRunnerSiRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn sp_prerequisite(identity: &GeneratedRunnerBuildIdentityV1) -> SpWriterRuntimePrerequisiteV1 {
    let mut prerequisite = SpWriterRuntimePrerequisiteV1 {
        schema: fn64_abi::recompiled::SP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_owned(),
        program_model_sha256: "a1".repeat(32),
        resolver_install_sha256: "b2".repeat(32),
        abi_host_catalog_receipt_sha256: "b3".repeat(32),
        build_receipt_schema: identity.build_receipt_schema,
        aot_runtime: identity.aot_runtime,
        production_aot: identity.production_aot,
        dev_interpreter: identity.dev_interpreter,
        trace_epoch_id: 1,
        watched_ranges: vec![SpWriterWatchedRangeV1 {
            physical_start: 0x400,
            physical_end: 0x800,
        }],
        journal_entry_count: 1,
        sp_journal_declaration_count: 0,
        journal_root_sha256: "b4".repeat(32),
        final_watched_sha256: "b5".repeat(32),
        sp_started: 2,
        sp_queued: 0,
        sp_committed: 2,
        sp_busy_cleared: 2,
        sp_rsp_to_rdram_committed: 1,
        sp_transition_sha256: "b6".repeat(32),
        receipt_sha256: String::new(),
    };
    prerequisite.receipt_sha256 =
        recompute_sp_runtime_prerequisite_receipt(&prerequisite).unwrap();
    prerequisite
}

fn sp_report(
    nonce: [u8; 32],
    identity: &GeneratedRunnerBuildIdentityV1,
) -> GeneratedRunnerSpRuntimeReportV1 {
    GeneratedRunnerSpRuntimeReportV1 {
        schema: GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce: hex(&nonce),
        build_identity_sha256: hex(&Sha256::digest(serde_json::to_vec(identity).unwrap())),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: sp_prerequisite(identity),
    }
}

fn sp_report_output(report: &GeneratedRunnerSpRuntimeReportV1) -> Vec<u8> {
    format!(
        "{}{report}\n",
        GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
        report = serde_json::to_string(report).unwrap()
    )
    .into_bytes()
}

fn build_evidence() -> GeneratedRunnerBuildEvidenceV1 {
    let mut evidence = GeneratedRunnerBuildEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5,
        builder_cargo_sha256: "91".repeat(32),
        cargo_graph_sha256: "92".repeat(32),
        cargo_source_sha256: "93".repeat(32),
        build_environment_sha256: "98".repeat(32),
        builder_rustc_sha256: "99".repeat(32),
        cargo_config_sha256: "9a".repeat(32),
        memory_guard_sha256: "97".repeat(32),
        selected_build_cargo_jobs: SELECTED_BUILD_CARGO_JOBS_V5,
        build_max_rss_mib: BUILD_MAX_RSS_MIB,
        build_min_free_percent: BUILD_MIN_FREE_PERCENT,
        max_build_seconds: 60 * 60,
        selected_binary_sha256: "94".repeat(32),
        private_build_inputs_sha256: "95".repeat(32),
        prepared_tree_descriptor_sha256: "96".repeat(32),
        prepared_tree_sha256: "6a".repeat(32),
        prepared_source_mode: PREPARED_SOURCE_MODE_INACTIVE_V1.to_owned(),
        producer_manifest_sha256: "70".repeat(32),
        producer_lock_sha256: "71".repeat(32),
        producer_cargo_graph_sha256: "72".repeat(32),
        producer_cargo_source_sha256: "73".repeat(32),
        producer_binary_sha256: "74".repeat(32),
        identity: identity(),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = evidence.recompute_authority_sha256();
    evidence
}

fn writer_audit_bundle_evidence() -> GeneratedRunnerWriterAuditBundleEvidenceV1 {
    let build = build_evidence();
    let bootstrap_observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, bootstrap_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let si_observed = (10u8..20)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, si_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let cpu_observed = (30u8..40)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, cpu_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let sp_observed = (20u8..30)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, sp_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let pi_observed = (40u8..50)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, pi_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let host_abi_observed = (50u8..60)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, host_abi_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let rdp_renderer_observed = (60u8..70)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, rdp_renderer_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let rsp_observed = (70u8..80)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, rsp_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let mut evidence = GeneratedRunnerWriterAuditBundleEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1,
        completed_channels: WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
            | WRITER_AUDIT_CPU_COMPLETED_V1
            | WRITER_AUDIT_HOST_ABI_COMPLETED_V1
            | WRITER_AUDIT_PI_COMPLETED_V1
            | WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
            | WRITER_AUDIT_RSP_COMPLETED_V1
            | WRITER_AUDIT_SI_COMPLETED_V1
            | WRITER_AUDIT_SP_COMPLETED_V1,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        bootstrap: Some(
            validate_bootstrap_runtime_series(&build, &bootstrap_observed).unwrap(),
        ),
        cpu: Some(validate_cpu_runtime_series(&build, &cpu_observed).unwrap()),
        host_abi: Some(validate_host_abi_runtime_series(&build, &host_abi_observed).unwrap()),
        pi: Some(validate_pi_runtime_series(&build, &pi_observed).unwrap()),
        rdp_renderer: Some(
            validate_rdp_renderer_runtime_series(&build, &rdp_renderer_observed).unwrap(),
        ),
        rsp: Some(validate_rsp_runtime_series(&build, &rsp_observed).unwrap()),
        si: Some(validate_si_runtime_series(&build, &si_observed).unwrap()),
        sp: Some(validate_sp_runtime_series(&build, &sp_observed).unwrap()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = writer_audit_bundle_authority_sha256(&evidence).unwrap();
    evidence
}

mod part1;
mod shard_selector;
