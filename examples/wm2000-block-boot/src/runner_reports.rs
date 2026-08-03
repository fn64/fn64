use crate::*;

fn generated_runner_build_identity_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1,
                )
    )
}

fn generated_runner_bootstrap_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_si_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_cpu_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_pi_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_rdp_renderer_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_rsp_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_host_abi_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn generated_runner_sp_audit_mode() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments
        .next()
        .expect("WM generated-runner process has argv[0]");
    matches!(
        (arguments.next(), arguments.next()),
        (Some(argument), None)
            if argument
                == std::ffi::OsStr::new(
                    fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1,
                )
    )
}

fn bootstrap_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed bootstrap audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed bootstrap audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn si_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed SI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed SI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn cpu_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1)
        .expect("fixed CPU audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed CPU audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn pi_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed PI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed PI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn rdp_renderer_audit_nonce() -> String {
    let nonce =
        std::env::var(fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1)
            .expect("fixed RDP renderer audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed RDP renderer audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn rsp_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed RSP audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed RSP audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn host_abi_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1)
        .expect("fixed Host ABI audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed Host ABI audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn sp_audit_nonce() -> String {
    let nonce = std::env::var(fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1)
        .expect("fixed SP audit mode requires its verifier-owned nonce");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "fixed SP audit nonce must be canonical lowercase SHA-256"
    );
    nonce
}

fn protocol_adapter_role(
    role: GeneratedAdapterRole,
) -> fn64_boot_harness::GeneratedRunnerAdapterRoleV1 {
    match role {
        GeneratedAdapterRole::DirectGenerated => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::DirectGenerated
        }
        GeneratedAdapterRole::EntryContextGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::EntryContextGate
        }
        GeneratedAdapterRole::DenseInstrumentationGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::DenseInstrumentationGate
        }
        GeneratedAdapterRole::OverlayGenerationGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::OverlayGenerationGate
        }
        GeneratedAdapterRole::ExternalDigestGate => {
            fn64_boot_harness::GeneratedRunnerAdapterRoleV1::ExternalDigestGate
        }
    }
}

fn generated_runner_build_identity(
    program: &CatalogBlockProgramV1,
    bindings: &[CargoGeneratedRunnerSourceBindingV1],
) -> fn64_boot_harness::GeneratedRunnerBuildIdentityV1 {
    let attestation = program
        .generated_runner_source_attestation()
        .expect("identity mode requires the exact Cargo source attestation");
    let build_receipt = attestation.build_receipt();
    let mut bindings = bindings.to_vec();
    bindings.sort_unstable_by_key(|binding| binding.bank);
    let runners = bindings
        .into_iter()
        .map(
            |binding| fn64_boot_harness::GeneratedRunnerLinkedIdentityV1 {
                bank: binding.bank.get(),
                generated_runner_source_sha256: sha256_hex(binding.generated_runner_source_sha256),
                code_words_sha256: sha256_hex(binding.code_words_sha256),
                vram_start: binding.vram_start.get(),
                vram_end: binding.vram_end.get(),
                composite_subrunner_count: binding.composite_subrunner_count,
                adapter_role: protocol_adapter_role(binding.adapter_role),
            },
        )
        .collect();
    fn64_boot_harness::GeneratedRunnerBuildIdentityV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3.to_owned(),
        package: env!("CARGO_PKG_NAME").to_owned(),
        manifest_sha256: sha256_hex(pack::MANIFEST_SHA256),
        lock_sha256: sha256_hex(pack::LOCK_SHA256),
        source_attestation_schema: attestation.schema().to_owned(),
        cargo_source_fields_validated: attestation.cargo_source_fields_validated(),
        program_identity_sha256: sha256_hex(attestation.program_identity().bytes()),
        root_adapter_source_sha256: sha256_hex(attestation.root_adapter_source_sha256()),
        shard_cargo_source_tree_sha256: sha256_hex(attestation.shard_cargo_source_tree_sha256()),
        emitter_source_sha256: sha256_hex(attestation.emitter_source_sha256()),
        runtime_source_sha256: sha256_hex(attestation.runtime_source_sha256()),
        prepared_source_mode: pack::PREPARED_SOURCE_MODE.to_owned(),
        normalized_rom_sha256: sha256_hex(pack::NORMALIZED_ROM_SHA256),
        prepared_manifest_sha256: sha256_hex(pack::PREPARED_MANIFEST_SHA256),
        prepared_tree_sha256: sha256_hex(pack::PREPARED_TREE_SHA256),
        prepared_generator_source_sha256: sha256_hex(pack::PREPARED_GENERATOR_SOURCE_SHA256),
        prepared_discovery_source_sha256: sha256_hex(pack::PREPARED_DISCOVERY_SOURCE_SHA256),
        prepared_emitter_source_sha256: sha256_hex(pack::PREPARED_EMITTER_SOURCE_SHA256),
        prepared_runtime_source_sha256: sha256_hex(pack::PREPARED_RUNTIME_SOURCE_SHA256),
        prepared_materializer_source_sha256: sha256_hex(pack::PREPARED_MATERIALIZER_SOURCE_SHA256),
        producer_manifest_sha256: sha256_hex(pack::PREPARED_PRODUCER_MANIFEST_SHA256),
        producer_lock_sha256: sha256_hex(pack::PREPARED_PRODUCER_LOCK_SHA256),
        producer_cargo_graph_sha256: sha256_hex(pack::PREPARED_PRODUCER_CARGO_GRAPH_SHA256),
        producer_cargo_source_sha256: sha256_hex(pack::PREPARED_PRODUCER_CARGO_SOURCE_SHA256),
        producer_binary_sha256: sha256_hex(pack::PREPARED_PRODUCER_BINARY_SHA256),
        binding_sha256: sha256_hex(attestation.binding_sha256()),
        build_receipt_schema: build_receipt.schema,
        aot_runtime: build_receipt.aot_runtime,
        production_aot: build_receipt.production_aot,
        dev_interpreter: build_receipt.dev_interpreter,
        runners,
    }
}

fn emit_generated_runner_build_identity(
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
) {
    let wire = serde_json::to_string(&identity)
        .expect("generated-runner build identity serialization is infallible");
    std::println!(
        "{}{wire}",
        fn64_boot_harness::GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1
    );
}

fn bootstrap_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedBootstrapWriterChannelReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerBootstrapRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    let journal_entry = &evidence.journal_entry;
    fn64_boot_harness::GeneratedRunnerBootstrapRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::BootstrapWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            bootstrap_receipt_sha256: sha256_hex(evidence.bootstrap_receipt_sha256),
            rom_sha256: sha256_hex(evidence.rom_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            generation_catalog_sha256: sha256_hex(evidence.generation_catalog_sha256),
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::BootstrapWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            bootstrap_watched_sha256: sha256_hex(evidence.bootstrap_watched_sha256),
            initial_generations: evidence
                .initial_generations
                .iter()
                .map(|generation| generation.get())
                .collect(),
            journal_entry: fn64_boot_harness::BootstrapMutationBatchV1 {
                sequence: journal_entry.sequence,
                declared_writes: journal_entry
                    .declared_writes
                    .iter()
                    .map(|write| {
                        assert_eq!(
                            write.channel,
                            fn64_recomp_rs::WriterChannel::BootstrapOrImport,
                            "bootstrap receipt contains another writer channel"
                        );
                        fn64_boot_harness::BootstrapAttributedWriteV1 {
                            channel: fn64_boot_harness::BootstrapWriterChannelV1::BootstrapOrImport,
                            physical_start: write.physical_start,
                            physical_end: write.physical_end,
                        }
                    })
                    .collect(),
                changed_ranges: journal_entry
                    .changed_ranges
                    .iter()
                    .map(|range| fn64_boot_harness::BootstrapWriterWatchedRangeV1 {
                        physical_start: range.physical_start,
                        physical_end: range.physical_end,
                    })
                    .collect(),
                before_sha256: sha256_hex(journal_entry.before_sha256),
                after_sha256: sha256_hex(journal_entry.after_sha256),
                invalidated_generations: journal_entry
                    .invalidated_generations
                    .iter()
                    .map(|generation| generation.get())
                    .collect(),
                journal_root_sha256: sha256_hex(journal_entry.journal_root_sha256),
            },
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn si_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedSiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerSiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerSiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::SiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::SiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            si_journal_declaration_count: evidence.si_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            si_started: evidence.si_started,
            si_committed: evidence.si_committed,
            si_pif_to_dram_committed: evidence.si_pif_to_dram_committed,
            si_transition_sha256: sha256_hex(evidence.si_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn cpu_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedCpuWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerCpuRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerCpuRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::CpuWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::CpuWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            cpu_journal_declaration_count: evidence.cpu_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            cpu_store_count: evidence.cpu_store_count,
            cpu_store_trace_sha256: sha256_hex(evidence.cpu_store_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn host_abi_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedHostAbiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerHostAbiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerHostAbiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::HostAbiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            initial_journal_entry_count: evidence.initial_journal_entry_count,
            final_journal_entry_count: evidence.final_journal_entry_count,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::HostAbiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            host_abi_journal_entry_count: evidence.host_abi_journal_entry_count,
            host_abi_journal_declaration_count: evidence.host_abi_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            transactions_started: evidence.transactions_started,
            transactions_finished: evidence.transactions_finished,
            ordering_boundaries: evidence.ordering_boundaries,
            lifecycle_sha256: sha256_hex(evidence.lifecycle_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn pi_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedPiWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerPiRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerPiRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::PiWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::PiWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            pi_journal_declaration_count: evidence.pi_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            pi_started: evidence.pi_started,
            pi_committed: evidence.pi_committed,
            pi_busy_cleared: evidence.pi_busy_cleared,
            pi_interrupt_raised: evidence.pi_interrupt_raised,
            pi_interrupt_cleared: evidence.pi_interrupt_cleared,
            pi_notifications: evidence.pi_notifications,
            pi_to_rdram_committed: evidence.pi_to_rdram_committed,
            pi_transition_sha256: sha256_hex(evidence.pi_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn rdp_renderer_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedRdpRendererWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerRdpRendererRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    assert!(
        evidence.renderer_publication_count != 0
            && evidence.rdp_renderer_journal_entry_count != 0
            && evidence.rdp_renderer_journal_declaration_count != 0
            && evidence.final_journal_entry_count > evidence.initial_journal_entry_count,
        "fixed RDP renderer audit requires a committed executable-byte publication"
    );
    fn64_boot_harness::GeneratedRunnerRdpRendererRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1
            .to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::RdpRendererWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            initial_journal_entry_count: evidence.initial_journal_entry_count,
            final_journal_entry_count: evidence.final_journal_entry_count,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::RdpRendererWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            rdp_renderer_journal_entry_count: evidence.rdp_renderer_journal_entry_count,
            rdp_renderer_journal_declaration_count: evidence.rdp_renderer_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            renderer_publication_count: evidence.renderer_publication_count,
            publication_trace_sha256: sha256_hex(evidence.publication_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn rsp_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedRspWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerRspRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    assert!(
        evidence.interpreter_writeback_count != 0
            || evidence.translated_audio_hle_publication_count != 0,
        "fixed RSP audit requires a committed typed writeback publication"
    );
    fn64_boot_harness::GeneratedRunnerRspRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::RspWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::RspWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            rsp_journal_declaration_count: evidence.rsp_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            interpreter_writeback_count: evidence.interpreter_writeback_count,
            translated_audio_hle_publication_count: evidence.translated_audio_hle_publication_count,
            writeback_range_count: evidence.writeback_range_count,
            writeback_trace_sha256: sha256_hex(evidence.writeback_trace_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn sp_runtime_report(
    nonce: String,
    identity: &fn64_boot_harness::GeneratedRunnerBuildIdentityV1,
    receipt: fn64_abi::recompiled::ValidatedSpWriterRuntimeStateReceiptV1,
) -> fn64_boot_harness::GeneratedRunnerSpRuntimeReportV1 {
    let evidence = receipt.evidence();
    assert!(receipt.has_valid_evidence_hash());
    fn64_boot_harness::GeneratedRunnerSpRuntimeReportV1 {
        schema: fn64_boot_harness::GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1.to_owned(),
        nonce,
        build_identity_sha256: sha256_hex(
            Sha256::digest(
                serde_json::to_vec(identity)
                    .expect("generated-runner build identity serialization is infallible"),
            )
            .into(),
        ),
        program_identity_sha256: identity.program_identity_sha256.clone(),
        prerequisite: fn64_boot_harness::SpWriterRuntimePrerequisiteV1 {
            schema: evidence.schema.clone(),
            program_model_sha256: sha256_hex(evidence.program_model_sha256),
            resolver_install_sha256: sha256_hex(evidence.resolver_install_sha256),
            abi_host_catalog_receipt_sha256: sha256_hex(evidence.abi_host_catalog_receipt_sha256),
            build_receipt_schema: evidence.build_receipt.schema,
            aot_runtime: evidence.build_receipt.aot_runtime,
            production_aot: evidence.build_receipt.production_aot,
            dev_interpreter: evidence.build_receipt.dev_interpreter,
            trace_epoch_id: evidence.trace_epoch_id,
            watched_ranges: evidence
                .watched_ranges
                .iter()
                .map(|range| fn64_boot_harness::SpWriterWatchedRangeV1 {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            journal_entry_count: evidence.journal_entry_count,
            sp_journal_declaration_count: evidence.sp_journal_declaration_count,
            journal_root_sha256: sha256_hex(evidence.journal_root_sha256),
            final_watched_sha256: sha256_hex(evidence.final_watched_sha256),
            sp_started: evidence.sp_started,
            sp_queued: evidence.sp_queued,
            sp_committed: evidence.sp_committed,
            sp_busy_cleared: evidence.sp_busy_cleared,
            sp_rsp_to_rdram_committed: evidence.sp_rsp_to_rdram_committed,
            sp_transition_sha256: sha256_hex(evidence.sp_transition_sha256),
            receipt_sha256: sha256_hex(evidence.receipt_sha256),
        },
    }
}

fn take_completed_si_audit_receipt(
) -> Option<fn64_abi::recompiled::ValidatedSiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::SiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_si_writer_runtime_state_receipt_v1() {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed SI audit mode has no canonical runtime owner"),
        Err(
            Error::PendingDeviceSi
            | Error::PendingAbiSi
            | Error::NoSiTransitions
            | Error::NoPifToDramCommit,
        ) => None,
        Err(error) => panic!("fixed SI audit invariant failed: {error}"),
    }
}

fn take_completed_cpu_audit_receipt(
    epoch: &fn64_abi::recompiled::CpuWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedCpuWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::CpuWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_cpu_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed CPU audit mode has no unconsumed canonical runtime owner"),
        Err(Error::NoCpuStores) => None,
        Err(error) => panic!("fixed CPU audit invariant failed: {error}"),
    }
}

fn take_completed_pi_audit_receipt(
    epoch: &fn64_abi::recompiled::PiWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedPiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::PiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_pi_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed PI audit mode has no unconsumed canonical runtime owner"),
        Err(
            Error::PendingDevicePi
            | Error::PendingAbiPi
            | Error::NoPiTransitions
            | Error::NoToRdramCommit,
        ) => None,
        Err(error) => panic!("fixed PI audit invariant failed: {error}"),
    }
}

fn take_completed_rdp_renderer_audit_receipt(
    epoch: &fn64_abi::recompiled::RdpRendererWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedRdpRendererWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::RdpRendererWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_rdp_renderer_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed RDP renderer audit mode has no unconsumed canonical owner"),
        Err(
            Error::PendingDeviceRspTask
            | Error::PendingDeviceDpcTransaction
            | Error::PendingDeviceDpCompletion
            | Error::PendingAbiRendererWork
            | Error::NoRendererPublications,
        ) => None,
        Err(error) => panic!("fixed RDP renderer audit invariant failed: {error}"),
    }
}

fn take_completed_rsp_audit_receipt(
    epoch: &fn64_abi::recompiled::RspWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedRspWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::RspWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_rsp_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed RSP audit mode has no unconsumed canonical owner"),
        Err(Error::PendingDeviceRspTask | Error::PendingAbiRspWork | Error::NoRspWritebacks) => {
            None
        }
        Err(error) => panic!("fixed RSP audit invariant failed: {error}"),
    }
}

fn take_completed_host_abi_audit_receipt(
    epoch: &fn64_abi::recompiled::HostAbiWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedHostAbiWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::HostAbiWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_host_abi_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed Host ABI audit mode has no unconsumed canonical runtime owner"),
        Err(Error::NoHostAbiTransactions | Error::NoHostAbiWriteCommit) => None,
        Err(error) => panic!("fixed Host ABI audit invariant failed: {error}"),
    }
}

fn take_completed_sp_audit_receipt(
    epoch: &fn64_abi::recompiled::SpWriterRuntimeTraceEpochV1,
) -> Option<fn64_abi::recompiled::ValidatedSpWriterRuntimeStateReceiptV1> {
    use fn64_abi::recompiled::SpWriterRuntimeStateErrorV1 as Error;
    match fn64_abi::recompiled::take_validated_sp_writer_runtime_state_receipt_v1(epoch) {
        Ok(Some(receipt)) => Some(receipt),
        Ok(None) => panic!("fixed SP audit mode has no unconsumed canonical runtime owner"),
        Err(
            Error::PendingDeviceSpDma
            | Error::PendingDeviceSpTask
            | Error::PendingAbiSpWork
            | Error::NoSpTransitions
            | Error::NoRspToRdramCommit,
        ) => None,
        Err(error) => panic!("fixed SP audit invariant failed: {error}"),
    }
}
