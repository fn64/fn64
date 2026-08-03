use super::*;

#[test]
fn identity_validator_recomputes_runner_binding_and_production_features() {
    let valid = identity();
    validate_identity(&valid, &valid.manifest_sha256, &valid.lock_sha256).unwrap();

    let mut wrong_role = valid.clone();
    wrong_role.runners[0].adapter_role = GeneratedRunnerAdapterRoleV1::EntryContextGate;
    assert!(validate_identity(
        &wrong_role,
        &wrong_role.manifest_sha256,
        &wrong_role.lock_sha256
    )
    .is_err());

    let mut interpreter = valid.clone();
    interpreter.production_aot = false;
    interpreter.dev_interpreter = true;
    assert!(validate_identity(
        &interpreter,
        &interpreter.manifest_sha256,
        &interpreter.lock_sha256
    )
    .is_err());
}

#[test]
fn prepared_tree_measurement_binds_content_separately_from_descriptors() {
    let (_first_scratch, first_root, claims, rom) = synthetic_prepared_tree(None);
    let first = measure_prepared_tree_v3(&first_root, &rom, &claims).unwrap();
    let (_second_scratch, second_root, _, _) = synthetic_prepared_tree(None);
    let second = measure_prepared_tree_v3(&second_root, &rom, &claims).unwrap();
    assert_eq!(first.tree_sha256, second.tree_sha256);
    assert_ne!(first.descriptor_sha256, second.descriptor_sha256);

    let (_changed_scratch, changed_root, _, _) =
        synthetic_prepared_tree(Some(PREPARED_PACKAGES[24]));
    let changed = measure_prepared_tree_v3(&changed_root, &rom, &claims).unwrap();
    assert_ne!(first.tree_sha256, changed.tree_sha256);
}

#[test]
fn prepared_tree_measurement_rejects_extra_marker_and_digest_drift() {
    let (_scratch, root, claims, rom) = synthetic_prepared_tree(None);
    let extra = root.join("extra");
    fs::write(&extra, b"extra").unwrap();
    assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
    fs::remove_file(extra).unwrap();

    let marker = root.join(PREPARED_UPDATE_MARKER_NAME);
    fs::write(&marker, b"update").unwrap();
    assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
    fs::remove_file(marker).unwrap();

    fs::write(root.join(PREPARED_PACKAGES[0]).join("runner.rs"), b"drift").unwrap();
    assert!(measure_prepared_tree_v3(&root, &rom, &claims).is_err());
}

#[test]
fn wm_shard_source_graph_uses_hardened_sibling_paths() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root
        .parent()
        .and_then(Path::parent)
        .expect("boot-harness crate is under the workspace crates directory");
    let package_root = repo_root.join("examples/wm2000-block-boot");
    let shard_root = wm_shard_root(&package_root).expect("derive shard sibling");
    assert!(
        !shard_root
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir)),
        "hardened source readers reject lexical parent traversal"
    );

    let mode = wm_prepared_source_mode_v3(&package_root).expect("classify shard source mode");
    let digest = wm_shard_cargo_source_sha256(&package_root, mode)
        .expect("hash the exact shard source graph through hardened reads");
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn bounded_diagnostic_retains_the_command_failure_tail() {
    let diagnostic =
        bounded_diagnostic(format!("{}\nactual error", "progress\n".repeat(600)).as_bytes());
    assert!(diagnostic.starts_with("<earlier output truncated>\n"));
    assert!(diagnostic.ends_with("actual error"));
}

#[cfg(unix)]
#[test]
fn nonzero_writer_child_error_retains_bounded_stderr_tail() {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "printf 'private diagnostic tail' >&2; exit 17"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let error =
        launch_writer_runtime_child_output(command, 3, WriterRuntimeAuditProtocol::Bootstrap)
            .unwrap_err()
            .to_string();
    assert!(error.contains("bootstrap audit child run 3 exited"));
    assert!(error.contains("stderr_bytes=23"));
    assert!(error.ends_with("stderr: private diagnostic tail"));
}

#[test]
fn writer_runtime_transport_extracts_one_strict_report_amid_diagnostics() {
    let identity = identity();
    let nonce = [0x21; 32];
    let report = bootstrap_report(nonce, &identity);
    let report_wire = bootstrap_report_output(&report);
    let mut stdout = b"ordinary runtime diagnostic\n".to_vec();
    stdout.extend_from_slice(&report_wire);
    stdout.extend_from_slice(b"later ordinary diagnostic\n");

    let envelope =
        extract_writer_runtime_report_envelope(&stdout, WriterRuntimeAuditProtocol::Bootstrap)
            .unwrap();
    assert_eq!(envelope, report_wire);
    assert_eq!(
        parse_generated_runner_bootstrap_runtime_report_v1(&envelope, nonce, &identity)
            .unwrap(),
        report
    );
}

#[test]
fn writer_runtime_transport_rejects_zero_multiple_malformed_and_over_limit_reports() {
    let protocol = WriterRuntimeAuditProtocol::Bootstrap;
    assert!(extract_writer_runtime_report_envelope(b"diagnostic only\n", protocol).is_err());

    let minimal = format!("{}{{}}\n", protocol.report_prefix());
    let duplicate = format!("{minimal}{minimal}");
    assert!(extract_writer_runtime_report_envelope(duplicate.as_bytes(), protocol).is_err());

    let malformed = extract_writer_runtime_report_envelope(minimal.as_bytes(), protocol)
        .expect("transport accepts one prefixed envelope for semantic validation");
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &malformed,
        [0x21; 32],
        &identity(),
    )
    .is_err());

    assert!(writer_runtime_outputs_within_limit(
        WRITER_RUNTIME_OUTPUT_LIMIT as u64,
        0,
    ));
    assert!(!writer_runtime_outputs_within_limit(
        WRITER_RUNTIME_OUTPUT_LIMIT as u64 + 1,
        0,
    ));

    let mut oversized = protocol.report_prefix().as_bytes().to_vec();
    oversized.resize(WRITER_RUNTIME_REPORT_LIMIT + 1, b'x');
    oversized.push(b'\n');
    assert!(extract_writer_runtime_report_envelope(&oversized, protocol).is_err());
}

#[test]
fn independent_emitter_source_measurement_matches_the_linked_receipt() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root
        .parent()
        .and_then(Path::parent)
        .expect("boot-harness crate is under the workspace crates directory");
    let measured = wm_emitter_source_sha256(repo_root).expect("measure emitter source");
    let linked = hex(
        &fn64_recomp_rs_codegen::generated_runner_emitter_source_receipt_v2().source_sha256(),
    );
    assert_eq!(measured, linked);
}

#[test]
fn identity_output_requires_exactly_one_prefixed_envelope() {
    let wire = serde_json::to_string(&identity()).unwrap();
    let output = format!("diagnostic\n{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n");
    assert_eq!(
        parse_identity_output(output.as_bytes()).unwrap(),
        identity()
    );
    assert!(parse_identity_output(b"diagnostic only\n").is_err());
    let repeated = format!(
        "{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n{GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1}{wire}\n"
    );
    assert!(parse_identity_output(repeated.as_bytes()).is_err());
}

#[test]
fn identity_wire_denies_unknown_fields_and_requires_bank_order() {
    let mut unknown = serde_json::to_value(identity()).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<GeneratedRunnerBuildIdentityV1>(unknown).is_err());

    let mut unsorted = identity();
    let mut earlier = unsorted.runners[0].clone();
    earlier.bank -= 1;
    unsorted.runners.push(earlier);
    unsorted.binding_sha256 = recompute_binding_sha256(&unsorted).unwrap();
    assert!(
        validate_identity(&unsorted, &unsorted.manifest_sha256, &unsorted.lock_sha256,)
            .is_err()
    );
}

#[test]
fn bootstrap_runtime_report_is_one_nonce_bound_deny_unknown_sequence_zero_envelope() {
    let identity = identity();
    let nonce = [0x21; 32];
    let report = bootstrap_report(nonce, &identity);
    let output = bootstrap_report_output(&report);
    assert_eq!(
        parse_generated_runner_bootstrap_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_bootstrap_runtime_report_v1(&output, [0x22; 32], &identity)
            .is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(
        parse_generated_runner_bootstrap_runtime_report_v1(&duplicate, nonce, &identity)
            .is_err()
    );
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &output[..output.len() - 1],
        nonce,
        &identity
    )
    .is_err());

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        unknown.as_bytes(),
        nonce,
        &identity
    )
    .is_err());

    let mut later_entry = report.clone();
    later_entry.prerequisite.journal_entry.sequence = 1;
    later_entry.prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&later_entry.prerequisite).unwrap();
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &bootstrap_report_output(&later_entry),
        nonce,
        &identity
    )
    .is_err());

    let mut wrong_rom = report.clone();
    wrong_rom.prerequisite.rom_sha256 = "ff".repeat(32);
    wrong_rom.prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&wrong_rom.prerequisite).unwrap();
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &bootstrap_report_output(&wrong_rom),
        nonce,
        &identity
    )
    .is_err());

    let mut zero_generation = report.clone();
    zero_generation.prerequisite.initial_generations[0] = 0;
    zero_generation.prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&zero_generation.prerequisite)
            .unwrap();
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &bootstrap_report_output(&zero_generation),
        nonce,
        &identity
    )
    .is_err());

    let mut forged_journal_root = report.clone();
    forged_journal_root
        .prerequisite
        .journal_entry
        .journal_root_sha256 = "fd".repeat(32);
    forged_journal_root.prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&forged_journal_root.prerequisite)
            .unwrap();
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &bootstrap_report_output(&forged_journal_root),
        nonce,
        &identity
    )
    .is_err());

    let mut bad_receipt = report;
    bad_receipt.prerequisite.receipt_sha256 = "fe".repeat(32);
    assert!(parse_generated_runner_bootstrap_runtime_report_v1(
        &bootstrap_report_output(&bad_receipt),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn cpu_runtime_report_requires_one_nonce_bound_deny_unknown_envelope() {
    let identity = identity();
    let nonce = [0x31; 32];
    let report = cpu_report(nonce, &identity);
    let output = cpu_report_output(&report);
    assert_eq!(
        parse_generated_runner_cpu_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_cpu_runtime_report_v1(&output, [0x32; 32], &identity).is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(
        parse_generated_runner_cpu_runtime_report_v1(&duplicate, nonce, &identity).is_err()
    );

    let mut value = serde_json::to_value(&report).unwrap();
    value["unexpected"] = serde_json::json!(true);
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&value).unwrap()
    );
    assert!(
        parse_generated_runner_cpu_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
            .is_err()
    );
}

#[test]
fn cpu_runtime_report_recomputes_receipt_and_requires_fresh_store_evidence() {
    let identity = identity();
    let nonce = [0x33; 32];
    let mut report = cpu_report(nonce, &identity);
    report.prerequisite.cpu_store_count = 0;
    report.prerequisite.receipt_sha256 =
        recompute_cpu_runtime_prerequisite_receipt(&report.prerequisite).unwrap();
    assert!(parse_generated_runner_cpu_runtime_report_v1(
        &cpu_report_output(&report),
        nonce,
        &identity
    )
    .is_err());

    let mut report = cpu_report(nonce, &identity);
    report.prerequisite.cpu_store_trace_sha256 = "fe".repeat(32);
    assert!(parse_generated_runner_cpu_runtime_report_v1(
        &cpu_report_output(&report),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn cpu_runtime_series_requires_ten_distinct_semantically_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, cpu_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_cpu_runtime_series(&build, &observed).unwrap();
    validate_cpu_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9].0 = repeated[0].0;
    repeated[9].1 = cpu_report(repeated[0].0, &build.identity);
    assert!(validate_cpu_runtime_series(&build, &repeated).is_err());

    let mut changed = observed;
    changed[9].1.prerequisite.cpu_store_count += 1;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_cpu_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_cpu_runtime_series(&build, &changed).is_err());

    let mut tampered = evidence;
    tampered.cpu_store_trace_sha256 = "ff".repeat(32);
    assert!(validate_cpu_runtime_series_evidence(&tampered).is_err());
}

#[test]
fn host_abi_runtime_report_is_strict_nonce_bound_and_recomputes_receipt() {
    let identity = identity();
    let nonce = [0x43; 32];
    let report = host_abi_report(nonce, &identity);
    let output = host_abi_report_output(&report);
    assert_eq!(
        parse_generated_runner_host_abi_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_host_abi_runtime_report_v1(&output, [0x44; 32], &identity)
            .is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(
        parse_generated_runner_host_abi_runtime_report_v1(&duplicate, nonce, &identity)
            .is_err()
    );

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert("raw_pointer_catalog".to_owned(), serde_json::json!(true));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(parse_generated_runner_host_abi_runtime_report_v1(
        unknown.as_bytes(),
        nonce,
        &identity
    )
    .is_err());

    let mut no_write = host_abi_report(nonce, &identity);
    no_write.prerequisite.host_abi_journal_declaration_count = 0;
    no_write.prerequisite.receipt_sha256 =
        recompute_host_abi_runtime_prerequisite_receipt(&no_write.prerequisite).unwrap();
    assert!(parse_generated_runner_host_abi_runtime_report_v1(
        &host_abi_report_output(&no_write),
        nonce,
        &identity
    )
    .is_err());

    let mut tampered = host_abi_report(nonce, &identity);
    tampered.prerequisite.lifecycle_sha256 = "fe".repeat(32);
    assert!(parse_generated_runner_host_abi_runtime_report_v1(
        &host_abi_report_output(&tampered),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn host_abi_runtime_series_requires_exact_ten_identical_canonical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, host_abi_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_host_abi_runtime_series(&build, &observed).unwrap();
    validate_host_abi_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9].0 = repeated[0].0;
    repeated[9].1 = host_abi_report(repeated[0].0, &build.identity);
    assert!(validate_host_abi_runtime_series(&build, &repeated).is_err());

    let mut changed = observed;
    changed[9].1.prerequisite.transactions_started += 1;
    changed[9].1.prerequisite.transactions_finished += 1;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_host_abi_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_host_abi_runtime_series(&build, &changed).is_err());

    let mut tampered = evidence;
    tampered.lifecycle_sha256 = "ff".repeat(32);
    assert!(validate_host_abi_runtime_series_evidence(&tampered).is_err());
}

#[test]
fn rdp_renderer_report_requires_one_nonce_bound_deny_unknown_envelope() {
    let identity = identity();
    let nonce = [0x61; 32];
    let report = rdp_renderer_report(nonce, &identity);
    let output = rdp_renderer_report_output(&report);
    assert_eq!(
        parse_generated_runner_rdp_renderer_runtime_report_v1(&output, nonce, &identity)
            .unwrap(),
        report
    );
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        &output, [0x62; 32], &identity
    )
    .is_err());
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        &duplicate, nonce, &identity
    )
    .is_err());

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert("needs_lle_count".to_owned(), serde_json::json!(1));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        unknown.as_bytes(),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn rdp_renderer_report_requires_actual_executable_publication_and_recomputed_receipt() {
    let identity = identity();
    let nonce = [0x63; 32];

    let mut needs_lle_only = rdp_renderer_report(nonce, &identity);
    needs_lle_only.prerequisite.final_journal_entry_count =
        needs_lle_only.prerequisite.initial_journal_entry_count;
    needs_lle_only.prerequisite.rdp_renderer_journal_entry_count = 0;
    needs_lle_only
        .prerequisite
        .rdp_renderer_journal_declaration_count = 0;
    needs_lle_only.prerequisite.renderer_publication_count = 0;
    needs_lle_only.prerequisite.receipt_sha256 =
        recompute_rdp_renderer_runtime_prerequisite_receipt(&needs_lle_only.prerequisite)
            .unwrap();
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        &rdp_renderer_report_output(&needs_lle_only),
        nonce,
        &identity
    )
    .is_err());

    let mut framebuffer_only = rdp_renderer_report(nonce, &identity);
    framebuffer_only.prerequisite.final_journal_entry_count =
        framebuffer_only.prerequisite.initial_journal_entry_count;
    framebuffer_only
        .prerequisite
        .rdp_renderer_journal_entry_count = 0;
    framebuffer_only
        .prerequisite
        .rdp_renderer_journal_declaration_count = 0;
    framebuffer_only.prerequisite.receipt_sha256 =
        recompute_rdp_renderer_runtime_prerequisite_receipt(&framebuffer_only.prerequisite)
            .unwrap();
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        &rdp_renderer_report_output(&framebuffer_only),
        nonce,
        &identity
    )
    .is_err());

    let mut tampered = rdp_renderer_report(nonce, &identity);
    tampered.prerequisite.publication_trace_sha256 = "ee".repeat(32);
    assert!(parse_generated_runner_rdp_renderer_runtime_report_v1(
        &rdp_renderer_report_output(&tampered),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn rdp_renderer_runtime_series_requires_exact_ten_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, rdp_renderer_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_rdp_renderer_runtime_series(&build, &observed).unwrap();
    validate_rdp_renderer_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9].0 = repeated[0].0;
    repeated[9].1 = rdp_renderer_report(repeated[0].0, &build.identity);
    assert!(validate_rdp_renderer_runtime_series(&build, &repeated).is_err());

    let mut changed = observed;
    changed[9].1.prerequisite.renderer_publication_count += 1;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_rdp_renderer_runtime_prerequisite_receipt(&changed[9].1.prerequisite)
            .unwrap();
    assert!(validate_rdp_renderer_runtime_series(&build, &changed).is_err());

    let mut tampered = evidence;
    tampered.publication_trace_sha256 = "ff".repeat(32);
    assert!(validate_rdp_renderer_runtime_series_evidence(&tampered).is_err());
}

#[test]
fn rsp_runtime_report_is_nonce_bound_deny_unknown_and_recomputes_receipt() {
    let identity = identity();
    let nonce = [0x67; 32];
    let report = rsp_report(nonce, &identity);
    let output = rsp_report_output(&report);
    assert_eq!(
        parse_generated_runner_rsp_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_rsp_runtime_report_v1(&output, [0x68; 32], &identity).is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(
        parse_generated_runner_rsp_runtime_report_v1(&duplicate, nonce, &identity).is_err()
    );

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert("self_asserted_complete".to_owned(), serde_json::json!(true));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(
        parse_generated_runner_rsp_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
            .is_err()
    );

    let mut no_publication = rsp_report(nonce, &identity);
    no_publication.prerequisite.interpreter_writeback_count = 0;
    no_publication.prerequisite.writeback_range_count = 0;
    no_publication.prerequisite.receipt_sha256 =
        recompute_rsp_runtime_prerequisite_receipt(&no_publication.prerequisite).unwrap();
    assert!(parse_generated_runner_rsp_runtime_report_v1(
        &rsp_report_output(&no_publication),
        nonce,
        &identity
    )
    .is_err());

    let mut tampered = rsp_report(nonce, &identity);
    tampered.prerequisite.writeback_trace_sha256 = "ee".repeat(32);
    assert!(parse_generated_runner_rsp_runtime_report_v1(
        &rsp_report_output(&tampered),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn rsp_runtime_series_requires_exact_ten_distinct_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, rsp_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_rsp_runtime_series(&build, &observed).unwrap();
    validate_rsp_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9].0 = repeated[0].0;
    repeated[9].1 = rsp_report(repeated[0].0, &build.identity);
    assert!(validate_rsp_runtime_series(&build, &repeated).is_err());

    let mut changed = observed;
    changed[9].1.prerequisite.interpreter_writeback_count += 1;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_rsp_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_rsp_runtime_series(&build, &changed).is_err());

    let mut tampered = evidence;
    tampered.writeback_trace_sha256 = "ff".repeat(32);
    assert!(validate_rsp_runtime_series_evidence(&tampered).is_err());
}

#[test]
fn pi_runtime_report_requires_one_nonce_bound_deny_unknown_envelope() {
    let identity = identity();
    let nonce = [0x51; 32];
    let report = pi_report(nonce, &identity);
    let output = pi_report_output(&report);
    assert_eq!(
        parse_generated_runner_pi_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_pi_runtime_report_v1(&output, [0x52; 32], &identity).is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(parse_generated_runner_pi_runtime_report_v1(&duplicate, nonce, &identity).is_err());

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert("self_asserted_complete".to_owned(), serde_json::json!(true));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(
        parse_generated_runner_pi_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
            .is_err()
    );
}

#[test]
fn pi_runtime_report_recomputes_receipt_and_requires_completed_read_dma() {
    let identity = identity();
    let nonce = [0x53; 32];

    let mut stale_epoch = pi_report(nonce, &identity);
    stale_epoch.prerequisite.trace_epoch_id = 0;
    stale_epoch.prerequisite.receipt_sha256 =
        recompute_pi_runtime_prerequisite_receipt(&stale_epoch.prerequisite).unwrap();
    assert!(parse_generated_runner_pi_runtime_report_v1(
        &pi_report_output(&stale_epoch),
        nonce,
        &identity
    )
    .is_err());

    let mut no_read_dma = pi_report(nonce, &identity);
    no_read_dma.prerequisite.pi_to_rdram_committed = 0;
    no_read_dma.prerequisite.receipt_sha256 =
        recompute_pi_runtime_prerequisite_receipt(&no_read_dma.prerequisite).unwrap();
    assert!(parse_generated_runner_pi_runtime_report_v1(
        &pi_report_output(&no_read_dma),
        nonce,
        &identity
    )
    .is_err());

    let mut tampered = pi_report(nonce, &identity);
    tampered.prerequisite.pi_transition_sha256 = "fe".repeat(32);
    assert!(parse_generated_runner_pi_runtime_report_v1(
        &pi_report_output(&tampered),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn pi_runtime_series_requires_ten_distinct_semantically_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, pi_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_pi_runtime_series(&build, &observed).unwrap();
    validate_pi_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9].0 = repeated[0].0;
    repeated[9].1 = pi_report(repeated[0].0, &build.identity);
    assert!(validate_pi_runtime_series(&build, &repeated).is_err());

    let mut changed = observed;
    changed[9].1.prerequisite.pi_notifications += 1;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_pi_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_pi_runtime_series(&build, &changed).is_err());

    let mut tampered = evidence;
    tampered.pi_transition_sha256 = "ff".repeat(32);
    assert!(validate_pi_runtime_series_evidence(&tampered).is_err());
}

#[test]
fn si_runtime_report_requires_one_deny_unknown_envelope() {
    let identity = identity();
    let nonce = [0x31; 32];
    let report = si_report(nonce, &identity);
    let output = si_report_output(&report);
    assert_eq!(
        parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(parse_generated_runner_si_runtime_report_v1(
        b"diagnostic only\n",
        nonce,
        &identity
    )
    .is_err());
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(parse_generated_runner_si_runtime_report_v1(&duplicate, nonce, &identity).is_err());
    assert!(parse_generated_runner_si_runtime_report_v1(
        &output[..output.len() - 1],
        nonce,
        &identity
    )
    .is_err());
    let mut prefixed_noise = b"unexpected\n".to_vec();
    prefixed_noise.extend_from_slice(&output);
    assert!(
        parse_generated_runner_si_runtime_report_v1(&prefixed_noise, nonce, &identity).is_err()
    );
    let mut blank = output.clone();
    blank.push(b'\n');
    assert!(parse_generated_runner_si_runtime_report_v1(&blank, nonce, &identity).is_err());

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("caller_claim".to_owned(), serde_json::Value::Bool(true));
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(
        parse_generated_runner_si_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
            .is_err()
    );
    let mut nested_unknown = serde_json::to_value(&report).unwrap();
    nested_unknown["prerequisite"]
        .as_object_mut()
        .unwrap()
        .insert(
            "self_asserted_complete".to_owned(),
            serde_json::Value::Bool(true),
        );
    let nested_unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&nested_unknown).unwrap()
    );
    assert!(parse_generated_runner_si_runtime_report_v1(
        nested_unknown.as_bytes(),
        nonce,
        &identity
    )
    .is_err());
}

#[test]
fn si_runtime_report_binds_nonce_and_rejects_replay_under_another_challenge() {
    let identity = identity();
    let nonce = [0x41; 32];
    let output = si_report_output(&si_report(nonce, &identity));
    parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).unwrap();
    assert!(
        parse_generated_runner_si_runtime_report_v1(&output, [0x42; 32], &identity).is_err()
    );
}

#[test]
fn sp_runtime_report_requires_one_deny_unknown_nonce_bound_envelope() {
    let identity = identity();
    let nonce = [0x61; 32];
    let report = sp_report(nonce, &identity);
    let output = sp_report_output(&report);
    assert_eq!(
        parse_generated_runner_sp_runtime_report_v1(&output, nonce, &identity).unwrap(),
        report
    );
    assert!(
        parse_generated_runner_sp_runtime_report_v1(&output, [0x62; 32], &identity).is_err()
    );
    let mut duplicate = output.clone();
    duplicate.extend_from_slice(&output);
    assert!(parse_generated_runner_sp_runtime_report_v1(&duplicate, nonce, &identity).is_err());
    assert!(parse_generated_runner_sp_runtime_report_v1(
        &output[..output.len() - 1],
        nonce,
        &identity
    )
    .is_err());
    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["prerequisite"].as_object_mut().unwrap().insert(
        "self_asserted_complete".to_owned(),
        serde_json::Value::Bool(true),
    );
    let unknown = format!(
        "{}{}\n",
        GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
        serde_json::to_string(&unknown).unwrap()
    );
    assert!(
        parse_generated_runner_sp_runtime_report_v1(unknown.as_bytes(), nonce, &identity)
            .is_err()
    );

    let mut stale_epoch = report.clone();
    stale_epoch.prerequisite.trace_epoch_id = 0;
    stale_epoch.prerequisite.receipt_sha256 =
        recompute_sp_runtime_prerequisite_receipt(&stale_epoch.prerequisite).unwrap();
    assert!(parse_generated_runner_sp_runtime_report_v1(
        &sp_report_output(&stale_epoch),
        nonce,
        &identity,
    )
    .is_err());

    let mut no_writeback = report.clone();
    no_writeback.prerequisite.sp_rsp_to_rdram_committed = 0;
    no_writeback.prerequisite.receipt_sha256 =
        recompute_sp_runtime_prerequisite_receipt(&no_writeback.prerequisite).unwrap();
    assert!(parse_generated_runner_sp_runtime_report_v1(
        &sp_report_output(&no_writeback),
        nonce,
        &identity,
    )
    .is_err());

    let mut bad_receipt = report;
    bad_receipt.prerequisite.receipt_sha256 = "ff".repeat(32);
    assert!(parse_generated_runner_sp_runtime_report_v1(
        &sp_report_output(&bad_receipt),
        nonce,
        &identity,
    )
    .is_err());
}

#[test]
fn si_runtime_report_rejects_nonproduction_identity_and_inconsistent_prerequisite() {
    let identity = identity();
    let nonce = [0x51; 32];
    let mut nonproduction = identity.clone();
    nonproduction.production_aot = false;
    nonproduction.dev_interpreter = true;
    let output = si_report_output(&si_report(nonce, &nonproduction));
    assert!(
        parse_generated_runner_si_runtime_report_v1(&output, nonce, &nonproduction).is_err()
    );

    let mut inconsistent = si_report(nonce, &identity);
    inconsistent.prerequisite.si_committed = 0;
    let output = si_report_output(&inconsistent);
    assert!(parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).is_err());

    let mut wrong_model_receipt = si_report(nonce, &identity);
    wrong_model_receipt.prerequisite.program_model_sha256 = "ff".repeat(32);
    let output = si_report_output(&wrong_model_receipt);
    assert!(parse_generated_runner_si_runtime_report_v1(&output, nonce, &identity).is_err());
}

#[test]
fn authority_integrity_binds_selected_binary_graph_sources_and_child_identity() {
    assert_eq!(BUILD_MAX_RSS_MIB, 4096);
    assert_eq!(BUILD_MIN_FREE_PERCENT, 40);
    assert_eq!(SELECTED_BUILD_CARGO_JOBS_V5, 2);
    let evidence = build_evidence();
    evidence.verify_integrity().unwrap();

    let mut wrong_jobs = evidence.clone();
    wrong_jobs.selected_build_cargo_jobs = 1;
    wrong_jobs.authority_sha256 = wrong_jobs.recompute_authority_sha256();
    assert!(wrong_jobs
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("requires exactly 2 selected-build Cargo jobs"));

    let mut downgraded_schema = evidence.clone();
    downgraded_schema.schema = VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V4;
    downgraded_schema.authority_sha256 = downgraded_schema.recompute_authority_sha256();
    assert!(downgraded_schema
        .verify_integrity()
        .unwrap_err()
        .to_string()
        .contains("unsupported verified generated-runner build schema"));

    for mutate in [
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.selected_binary_sha256 = "f1".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.private_build_inputs_sha256 = "f2".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.prepared_tree_descriptor_sha256 = "f3".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.prepared_tree_sha256 = "f4".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.producer_binary_sha256 = "f5".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.build_environment_sha256 = "f6".repeat(32)
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| value.build_max_rss_mib = 2048,
        |value: &mut GeneratedRunnerBuildEvidenceV1| value.build_min_free_percent = 39,
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.prepared_source_mode = PREPARED_SOURCE_MODE_CONSUMED_V1.to_owned()
        },
        |value: &mut GeneratedRunnerBuildEvidenceV1| {
            value.identity.prepared_materializer_source_sha256 = "f7".repeat(32)
        },
    ] {
        let mut changed = evidence.clone();
        mutate(&mut changed);
        assert!(changed.verify_integrity().is_err());
    }
}

#[test]
fn si_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, si_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_si_runtime_series(&build, &observed).unwrap();
    validate_si_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9] = repeated[0].clone();
    assert!(validate_si_runtime_series(&build, &repeated).is_err());

    let mut changed = observed.clone();
    changed[9].1.prerequisite.si_started = 2;
    changed[9].1.prerequisite.si_committed = 2;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_si_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_si_runtime_series(&build, &changed).is_err());

    for mutate in [
        |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| value.run_count = 9,
        |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
            value.selected_binary_sha256 = "ff".repeat(32)
        },
        |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
            value.private_build_inputs_sha256 = "fe".repeat(32)
        },
        |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
            value.program_model_sha256 = "fd".repeat(32)
        },
        |value: &mut GeneratedRunnerSiRuntimeSeriesEvidenceV1| {
            value.si_transition_sha256 = "fc".repeat(32)
        },
    ] {
        let mut changed = evidence.clone();
        mutate(&mut changed);
        assert!(validate_si_runtime_series_evidence(&changed).is_err());
    }
}

#[test]
fn bootstrap_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, bootstrap_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_bootstrap_runtime_series(&build, &observed).unwrap();
    validate_bootstrap_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9] = repeated[0].clone();
    assert!(validate_bootstrap_runtime_series(&build, &repeated).is_err());

    let mut changed = observed.clone();
    changed[9].1.prerequisite.journal_entry.before_sha256 = "cc".repeat(32);
    changed[9].1.prerequisite.journal_entry.journal_root_sha256 =
        recompute_bootstrap_canonical_journal_root(
            &changed[9].1.prerequisite.watched_ranges,
            &changed[9].1.prerequisite.journal_entry,
        )
        .unwrap();
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_bootstrap_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_bootstrap_runtime_series(&build, &changed).is_err());

    for mutate in [
        |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| value.run_count = 9,
        |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
            value.selected_binary_sha256 = "ff".repeat(32)
        },
        |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
            value.private_build_inputs_sha256 = "fe".repeat(32)
        },
        |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
            value.program_model_sha256 = "fd".repeat(32)
        },
        |value: &mut GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1| {
            value.runtime_receipt_sha256 = "fc".repeat(32)
        },
    ] {
        let mut changed = evidence.clone();
        mutate(&mut changed);
        assert!(validate_bootstrap_runtime_series_evidence(&changed).is_err());
    }
}

#[test]
fn sp_runtime_series_requires_ten_distinct_nonce_bound_identical_reports() {
    let build = build_evidence();
    let observed = (0u8..10)
        .map(|index| {
            let nonce = [index; 32];
            (nonce, sp_report(nonce, &build.identity))
        })
        .collect::<Vec<_>>();
    let evidence = validate_sp_runtime_series(&build, &observed).unwrap();
    validate_sp_runtime_series_evidence(&evidence).unwrap();

    let mut repeated = observed.clone();
    repeated[9] = repeated[0].clone();
    assert!(validate_sp_runtime_series(&build, &repeated).is_err());

    let mut changed = observed.clone();
    changed[9].1.prerequisite.sp_started = 3;
    changed[9].1.prerequisite.sp_committed = 3;
    changed[9].1.prerequisite.receipt_sha256 =
        recompute_sp_runtime_prerequisite_receipt(&changed[9].1.prerequisite).unwrap();
    assert!(validate_sp_runtime_series(&build, &changed).is_err());

    for mutate in [
        |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| value.run_count = 9,
        |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
            value.selected_binary_sha256 = "ff".repeat(32)
        },
        |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
            value.private_build_inputs_sha256 = "fe".repeat(32)
        },
        |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
            value.program_model_sha256 = "fd".repeat(32)
        },
        |value: &mut GeneratedRunnerSpRuntimeSeriesEvidenceV1| {
            value.sp_transition_sha256 = "fc".repeat(32)
        },
    ] {
        let mut changed = evidence.clone();
        mutate(&mut changed);
        assert!(validate_sp_runtime_series_evidence(&changed).is_err());
    }
}

#[test]
fn writer_audit_bundle_binds_bitmap_build_channels_and_nested_authorities() {
    let evidence = writer_audit_bundle_evidence();
    validate_writer_audit_bundle_evidence(&evidence).unwrap();

    let mut partial = evidence.clone();
    partial.completed_channels = WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1;
    partial.cpu = None;
    partial.host_abi = None;
    partial.pi = None;
    partial.rdp_renderer = None;
    partial.rsp = None;
    partial.si = None;
    partial.sp = None;
    partial.authority_sha256 = writer_audit_bundle_authority_sha256(&partial).unwrap();
    validate_writer_audit_bundle_evidence(&partial).unwrap();

    let mut bitmap_mismatch = evidence.clone();
    bitmap_mismatch.completed_channels &= !WRITER_AUDIT_PI_COMPLETED_V1;
    bitmap_mismatch.authority_sha256 =
        writer_audit_bundle_authority_sha256(&bitmap_mismatch).unwrap();
    assert!(validate_writer_audit_bundle_evidence(&bitmap_mismatch).is_err());

    let mut nested_tamper = evidence.clone();
    nested_tamper
        .bootstrap
        .as_mut()
        .unwrap()
        .runtime_receipt_sha256 = "ee".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&nested_tamper).is_err());

    let mut nested_pi_tamper = evidence.clone();
    nested_pi_tamper.pi.as_mut().unwrap().runtime_receipt_sha256 = "ef".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&nested_pi_tamper).is_err());

    let mut nested_host_abi_tamper = evidence.clone();
    nested_host_abi_tamper
        .host_abi
        .as_mut()
        .unwrap()
        .runtime_receipt_sha256 = "e0".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&nested_host_abi_tamper).is_err());

    let mut nested_rdp_renderer_tamper = evidence.clone();
    nested_rdp_renderer_tamper
        .rdp_renderer
        .as_mut()
        .unwrap()
        .runtime_receipt_sha256 = "e1".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&nested_rdp_renderer_tamper).is_err());

    let mut nested_rsp_tamper = evidence.clone();
    nested_rsp_tamper
        .rsp
        .as_mut()
        .unwrap()
        .runtime_receipt_sha256 = "e2".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&nested_rsp_tamper).is_err());

    let mut cross_build = evidence.clone();
    let si = cross_build.si.as_mut().unwrap();
    si.build_authority_sha256 = "ed".repeat(32);
    si.authority_sha256 = si_runtime_series_authority_sha256(si).unwrap();
    assert!(validate_writer_audit_bundle_evidence(&cross_build).is_err());

    let mut cross_program_model = evidence.clone();
    let sp = cross_program_model.sp.as_mut().unwrap();
    sp.program_model_sha256 = "ec".repeat(32);
    sp.authority_sha256 = sp_runtime_series_authority_sha256(sp).unwrap();
    assert!(validate_writer_audit_bundle_evidence(&cross_program_model).is_err());

    let mut authority_tamper = evidence;
    authority_tamper.authority_sha256 = "eb".repeat(32);
    assert!(validate_writer_audit_bundle_evidence(&authority_tamper).is_err());
}

#[test]
fn compiler_artifact_selector_rejects_absent_and_duplicate_roots() {
    assert!(select_compiler_artifact(b"{}").is_err());
    let line = serde_json::json!({
        "reason": "compiler-artifact",
        "target": { "name": PACKAGE, "kind": ["bin"] },
        "executable": "/does/not/matter"
    })
    .to_string();
    let duplicate = format!("{line}\n{line}\n");
    assert!(select_compiler_artifact(duplicate.as_bytes())
        .unwrap_err()
        .to_string()
        .contains("multiple"));
}

#[test]
fn cargo_progress_counts_completed_shard_libraries_without_content() {
    let shard = PREPARED_PACKAGES[0].replace('-', "_");
    let build_script = serde_json::json!({
        "reason": "compiler-artifact",
        "target": { "name": "build_script_build", "kind": ["custom-build"] },
    });
    let shard_library = serde_json::json!({
        "reason": "compiler-artifact",
        "target": { "name": shard, "kind": ["lib"] },
    });
    let root_binary = serde_json::json!({
        "reason": "compiler-artifact",
        "target": { "name": PACKAGE, "kind": ["bin"] },
    });
    let stream = format!("{build_script}\n{shard_library}\n{root_binary}\n");
    assert_eq!(
        cargo_build_progress(stream.as_bytes()),
        format!(
            "compiler_artifacts=3 completed_shards=1/{} root_binary=1",
            PREPARED_PACKAGES.len()
        )
    );
    assert_eq!(
        cargo_build_progress(b"not-json\n"),
        format!(
            "compiler_artifacts=0 completed_shards=0/{} root_binary=0",
            PREPARED_PACKAGES.len()
        )
    );
}

#[test]
fn selected_build_command_binds_two_jobs_and_the_exact_process_group_guard() {
    let workspace = repository_workspace().unwrap();
    let guard = workspace.join("scripts/memory-guard.zsh");
    let manifest = workspace.join("examples/wm2000-block-boot/Cargo.toml");
    let staged_boot_context =
        PathBuf::from("/private/tmp/fn64-command-policy/private-inputs/boot-context.json");
    let inputs = Wm2000GeneratedRunnerBuildInputsV1 {
        rom: PathBuf::from("/private/tmp/fn64-command-policy.rom"),
        boot_context: staged_boot_context.clone(),
        executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
            environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
            captures: vec![
                PathBuf::from("/private/tmp/capture-a"),
                PathBuf::from("/private/tmp/capture-b"),
                PathBuf::from("/private/tmp/capture-c"),
            ],
        }],
        max_build_seconds: 60 * 60,
    };
    let prepared = PreparedTreeMeasurementV3 {
        root: PathBuf::from("/private/tmp/fn64-command-policy/prepared"),
        normalized_rom_sha256: "11".repeat(32),
        manifest_sha256: "12".repeat(32),
        tree_sha256: "13".repeat(32),
        descriptor_sha256: "14".repeat(32),
        claims: synthetic_claims(),
    };
    let producer = ProducerBuildMeasurementV3 {
        manifest_sha256: "21".repeat(32),
        lock_sha256: "22".repeat(32),
        cargo_graph_sha256: "23".repeat(32),
        cargo_source_sha256: "24".repeat(32),
        binary_sha256: "25".repeat(32),
        binary: PathBuf::from("/private/tmp/fn64-command-policy/producer"),
    };
    let environment = BuildEnvironmentV3 {
        path: "/usr/bin:/bin".into(),
        home: PathBuf::from("/private/tmp/fn64-command-policy/home"),
        cargo_home: PathBuf::from("/private/tmp/fn64-command-policy/cargo-home"),
        temp: PathBuf::from("/private/tmp/fn64-command-policy/temp"),
        rustc: PathBuf::from("/absolute/verifier-owned/rustc"),
        identity_sha256: "31".repeat(32),
        rustc_sha256: "32".repeat(32),
        cargo_config_sha256: "33".repeat(32),
    };
    let command = guarded_build_command(
        &guard,
        Path::new("/absolute/verifier-owned/cargo"),
        &manifest,
        &inputs,
        &prepared,
        &producer,
        PREPARED_SOURCE_MODE_INACTIVE_V1,
        &environment,
        Path::new("/private/tmp/fn64-command-policy"),
    )
    .unwrap();
    assert_eq!(command.get_program(), guard.as_os_str());
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        &args[..4],
        &["/absolute/verifier-owned/cargo", "build", "-j2", "--frozen"]
    );
    let environments = command
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(environments["CARGO_BUILD_JOBS"].as_deref(), Some("2"));
    assert_eq!(
        environments["FN64_BOOT_CONTEXT"].as_deref(),
        staged_boot_context.to_str()
    );
    assert_eq!(
        environments["ROM"].as_deref(),
        Some("/private/tmp/fn64-command-policy.rom")
    );
    assert_eq!(
        environments["FN64_EXECUTABLE_IMAGE_TEST"].as_deref(),
        std::env::join_paths(&inputs.executable_image_groups[0].captures)
            .unwrap()
            .to_str()
    );
    assert_eq!(
        environments["FN64_GUARD_MAX_RSS_MIB"].as_deref(),
        Some("4096")
    );
    assert_eq!(
        environments["FN64_GUARD_MIN_FREE_PERCENT"].as_deref(),
        Some("40")
    );
    assert_eq!(
        environments["FN64_GUARD_MAX_SECONDS"].as_deref(),
        Some("3600")
    );
}

#[test]
fn writer_runtime_commands_have_only_exact_retained_private_inputs() {
    let inputs = Wm2000GeneratedRunnerBuildInputsV1 {
        rom: PathBuf::from("/private/tmp/staged/rom"),
        boot_context: PathBuf::from("/private/tmp/staged/boot-context"),
        executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
            environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
            captures: vec![
                PathBuf::from("/private/tmp/staged/capture-a"),
                PathBuf::from("/private/tmp/staged/capture-b"),
                PathBuf::from("/private/tmp/staged/capture-c"),
            ],
        }],
        max_build_seconds: 60 * 60,
    };
    let nonce = [0x5a; 32];
    let nonce_hex = hex(&nonce);
    for protocol in [
        WriterRuntimeAuditProtocol::Bootstrap,
        WriterRuntimeAuditProtocol::Cpu,
        WriterRuntimeAuditProtocol::HostAbi,
        WriterRuntimeAuditProtocol::Pi,
        WriterRuntimeAuditProtocol::RdpRenderer,
        WriterRuntimeAuditProtocol::Rsp,
        WriterRuntimeAuditProtocol::Si,
        WriterRuntimeAuditProtocol::Sp,
    ] {
        let mut command = Command::new("/private/tmp/staged/selected-runner");
        configure_writer_runtime_command(&mut command, &inputs, nonce, protocol).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new(protocol.argument())]
        );
        let environments = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(environments.len(), 5);
        assert_eq!(environments["ROM"].as_deref(), inputs.rom.to_str());
        assert_eq!(
            environments["FN64_BOOT_CONTEXT"].as_deref(),
            inputs.boot_context.to_str()
        );
        assert_eq!(
            environments[protocol.nonce_environment()].as_deref(),
            Some(nonce_hex.as_str())
        );
        for nonce_environment in [
            GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1,
            GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1,
        ] {
            assert_eq!(
                environments.contains_key(nonce_environment),
                nonce_environment == protocol.nonce_environment()
            );
        }
        assert_eq!(
            environments["FN64_EXECUTABLE_IMAGE_GROUPS"].as_deref(),
            Some("FN64_EXECUTABLE_IMAGE_TEST")
        );
        assert_eq!(
            environments["FN64_EXECUTABLE_IMAGE_TEST"].as_deref(),
            std::env::join_paths(&inputs.executable_image_groups[0].captures)
                .unwrap()
                .to_str()
        );
    }
}

#[test]
fn private_input_binding_retains_exact_boot_context_path_and_bytes() {
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).unwrap();
    let scratch = ScratchDirectory::create(&nonce).unwrap();
    let rom = scratch.path().join("game.rom");
    let boot_context = scratch.path().join("boot-context.json");
    let alternate_boot_context = scratch.path().join("alternate-boot-context.json");
    fs::write(&rom, b"synthetic-rom").unwrap();
    fs::write(&boot_context, b"synthetic-boot-context").unwrap();
    fs::write(&alternate_boot_context, b"synthetic-boot-context").unwrap();
    let captures = (0..3)
        .map(|index| {
            let path = scratch.path().join(format!("capture-{index}"));
            fs::write(&path, [u8::try_from(index).unwrap()]).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let mut inputs = Wm2000GeneratedRunnerBuildInputsV1 {
        rom,
        boot_context,
        executable_image_groups: vec![Wm2000ExecutableImageGroupV1 {
            environment_name: "FN64_EXECUTABLE_IMAGE_TEST".to_owned(),
            captures,
        }],
        max_build_seconds: 60 * 60,
    };
    validate_inputs(&inputs).unwrap();
    let original = private_inputs_sha256(&inputs).unwrap();
    inputs.boot_context = alternate_boot_context;
    assert_ne!(private_inputs_sha256(&inputs).unwrap(), original);
    let staged = stage_private_inputs(&inputs, scratch.path()).unwrap();
    let staged_digest = private_inputs_sha256(&staged).unwrap();
    assert!(staged
        .rom
        .starts_with(scratch.path().join("private-inputs")));
    assert!(staged
        .boot_context
        .starts_with(scratch.path().join("private-inputs")));
    fs::write(&inputs.boot_context, b"changed-boot-context").unwrap();
    assert_eq!(private_inputs_sha256(&staged).unwrap(), staged_digest);
}

#[test]
fn memory_guard_policy_requires_process_group_launch_and_termination() {
    validate_memory_guard_source(MEMORY_GUARD_SOURCE).unwrap();
    let source = std::str::from_utf8(MEMORY_GUARD_SOURCE).unwrap();
    for required in ["setsid", "terminate_group"] {
        let missing = source.replace(required, &"_".repeat(required.len()));
        assert!(validate_memory_guard_source(missing.as_bytes()).is_err());
    }
}
