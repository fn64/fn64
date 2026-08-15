#![allow(clippy::module_inception)]
use super::*;

pub fn parse_generated_runner_host_abi_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerHostAbiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "Host ABI runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner Host ABI runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner Host ABI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| {
            error("generated-runner child emitted no Host ABI runtime report envelope")
        })?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner Host ABI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_host_abi_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_generated_runner_host_abi_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_host_abi_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "Host ABI runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

pub(super) fn run_host_abi_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(HOST_ABI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..HOST_ABI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain Host ABI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a Host ABI audit nonce"));
        }
        let launched = launch_host_abi_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_host_abi_runtime_series(&build.evidence, &observed)?;
    validate_host_abi_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn host_abi_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::HostAbi,
    )?;
    Ok(command)
}

pub(super) fn launch_host_abi_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerHostAbiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        host_abi_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::HostAbi,
    )?;
    parse_generated_runner_host_abi_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn host_abi_semantic_report_sha256(
    report: &GeneratedRunnerHostAbiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize Host ABI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_host_abi_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerHostAbiRuntimeReportV1)],
) -> Result<GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != HOST_ABI_RUNTIME_SERIES_RUNS {
        return Err(error("Host ABI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-host-abi-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("Host ABI runtime series repeats a nonce"));
        }
        validate_generated_runner_host_abi_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = host_abi_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "Host ABI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1,
        run_count: HOST_ABI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        lifecycle_sha256: prerequisite.lifecycle_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = host_abi_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn host_abi_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-host-abi-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.lifecycle_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_host_abi_runtime_series_evidence(
    evidence: &GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != HOST_ABI_RUNTIME_SERIES_RUNS
    {
        return Err(error("Host ABI runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("lifecycle_sha256", &evidence.lifecycle_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if host_abi_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("Host ABI runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_host_abi_runtime_report_v1(
    report: &GeneratedRunnerHostAbiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner Host ABI runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "Host ABI runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner Host ABI report does not bind the selected build identity",
        ));
    }
    validate_host_abi_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_host_abi_runtime_prerequisite(
    prerequisite: &HostAbiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "Host ABI runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("lifecycle_sha256", &prerequisite.lifecycle_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.final_journal_entry_count <= prerequisite.initial_journal_entry_count
        || prerequisite.host_abi_journal_entry_count == 0
        || prerequisite.host_abi_journal_declaration_count == 0
        || prerequisite.transactions_started == 0
        || prerequisite.transactions_started != prerequisite.transactions_finished
        || prerequisite.ordering_boundaries == 0
    {
        return Err(error(
            "Host ABI runtime prerequisite lacks a fresh balanced write lifecycle",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "Host ABI runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    let recomputed = recompute_host_abi_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(
            "Host ABI runtime prerequisite receipt digest mismatch",
        ));
    }
    Ok(())
}

pub(super) fn recompute_host_abi_runtime_prerequisite_receipt(
    prerequisite: &HostAbiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:host-abi-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update(prerequisite.initial_journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.final_journal_entry_count.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.host_abi_journal_entry_count.to_be_bytes());
    hasher.update(
        prerequisite
            .host_abi_journal_declaration_count
            .to_be_bytes(),
    );
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.transactions_started.to_be_bytes());
    hasher.update(prerequisite.transactions_finished.to_be_bytes());
    hasher.update(prerequisite.ordering_boundaries.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.lifecycle_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_pi_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerPiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("PI runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner PI runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner PI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no PI runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner PI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_pi_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_generated_runner_pi_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerPiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_pi_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerPiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("PI runtime series authority failed self-validation"));
    }
    Ok(series)
}

pub(super) fn run_pi_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerPiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(PI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..PI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain PI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a PI audit nonce"));
        }
        let launched = launch_pi_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_pi_runtime_series(&build.evidence, &observed)?;
    validate_pi_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn pi_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Pi,
    )?;
    Ok(command)
}

pub(super) fn launch_pi_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerPiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        pi_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Pi,
    )?;
    parse_generated_runner_pi_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn pi_semantic_report_sha256(
    report: &GeneratedRunnerPiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize PI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_pi_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerPiRuntimeReportV1)],
) -> Result<GeneratedRunnerPiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != PI_RUNTIME_SERIES_RUNS {
        return Err(error("PI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-pi-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("PI runtime series repeats a nonce"));
        }
        validate_generated_runner_pi_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = pi_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "PI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerPiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1,
        run_count: PI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        pi_transition_sha256: prerequisite.pi_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = pi_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn pi_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerPiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-pi-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.pi_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_pi_runtime_series_evidence(
    evidence: &GeneratedRunnerPiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != PI_RUNTIME_SERIES_RUNS
    {
        return Err(error("PI runtime series has a noncanonical shape"));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("pi_transition_sha256", &evidence.pi_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if pi_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("PI runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_pi_runtime_report_v1(
    report: &GeneratedRunnerPiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner PI runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "PI runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner PI report does not bind the selected build identity",
        ));
    }
    validate_pi_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_pi_runtime_prerequisite(
    prerequisite: &PiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::PI_WRITER_RUNTIME_STATE_SCHEMA_V2
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "PI runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("pi_transition_sha256", &prerequisite.pi_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
        || prerequisite.pi_started == 0
        || prerequisite.pi_committed == 0
        || prerequisite.pi_busy_cleared == 0
        || prerequisite.pi_notifications == 0
        || prerequisite.pi_to_rdram_committed == 0
    {
        return Err(error(
            "PI runtime prerequisite lacks a fresh completed read-DMA lifecycle",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "PI runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256 != recompute_pi_runtime_prerequisite_receipt(prerequisite)? {
        return Err(error("PI runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

pub(super) fn recompute_pi_runtime_prerequisite_receipt(
    prerequisite: &PiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-state-receipt:v2");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.pi_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    for count in [
        prerequisite.pi_started,
        prerequisite.pi_committed,
        prerequisite.pi_busy_cleared,
        prerequisite.pi_interrupt_raised,
        prerequisite.pi_interrupt_cleared,
        prerequisite.pi_notifications,
        prerequisite.pi_to_rdram_committed,
    ] {
        hasher.update(count.to_be_bytes());
    }
    hasher.update(decode_sha256(&prerequisite.pi_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_rdp_renderer_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerRdpRendererRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "RDP renderer runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner RDP renderer runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner RDP renderer runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| {
            error("generated-runner child emitted no RDP renderer runtime report envelope")
        })?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner RDP renderer runtime report: {source}"
        ))
    })?;
    validate_generated_runner_rdp_renderer_runtime_report_v1(
        &report,
        expected_nonce,
        build_identity,
    )?;
    Ok(report)
}

pub fn run_generated_runner_rdp_renderer_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_rdp_renderer_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "RDP renderer runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

pub(super) fn run_rdp_renderer_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(RDP_RENDERER_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..RDP_RENDERER_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain RDP renderer audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error(
                "OS random source repeated an RDP renderer audit nonce",
            ));
        }
        let launched = launch_rdp_renderer_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_rdp_renderer_runtime_series(&build.evidence, &observed)?;
    validate_rdp_renderer_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn rdp_renderer_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::RdpRenderer,
    )?;
    Ok(command)
}

pub(super) fn launch_rdp_renderer_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerRdpRendererRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        rdp_renderer_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::RdpRenderer,
    )?;
    parse_generated_runner_rdp_renderer_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn rdp_renderer_semantic_report_sha256(
    report: &GeneratedRunnerRdpRendererRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic).map_err(|source| {
        error(format!(
            "serialize RDP renderer runtime semantics: {source}"
        ))
    })?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_rdp_renderer_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerRdpRendererRuntimeReportV1)],
) -> Result<GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != RDP_RENDERER_RUNTIME_SERIES_RUNS {
        return Err(error("RDP renderer runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-rdp-renderer-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("RDP renderer runtime series repeats a nonce"));
        }
        validate_generated_runner_rdp_renderer_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = rdp_renderer_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "RDP renderer runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1,
        run_count: RDP_RENDERER_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        publication_trace_sha256: prerequisite.publication_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = rdp_renderer_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn rdp_renderer_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-rdp-renderer-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.resolver_install_sha256,
        &evidence.abi_host_catalog_receipt_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.publication_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_rdp_renderer_runtime_series_evidence(
    evidence: &GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != RDP_RENDERER_RUNTIME_SERIES_RUNS
    {
        return Err(error(
            "RDP renderer runtime series has a noncanonical shape",
        ));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("build_identity_sha256", &evidence.build_identity_sha256),
        ("program_identity_sha256", &evidence.program_identity_sha256),
        ("program_model_sha256", &evidence.program_model_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "abi_host_catalog_receipt_sha256",
            &evidence.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        (
            "publication_trace_sha256",
            &evidence.publication_trace_sha256,
        ),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if rdp_renderer_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error(
            "RDP renderer runtime series authority digest mismatch",
        ));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_rdp_renderer_runtime_report_v1(
    report: &GeneratedRunnerRdpRendererRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner RDP renderer runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "RDP renderer runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner RDP renderer report does not bind the selected build identity",
        ));
    }
    validate_rdp_renderer_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_rdp_renderer_runtime_prerequisite(
    prerequisite: &RdpRendererWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "RDP renderer runtime prerequisite does not bind the selected production-AOT build",
        ));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "abi_host_catalog_receipt_sha256",
            &prerequisite.abi_host_catalog_receipt_sha256,
        ),
        ("journal_root_sha256", &prerequisite.journal_root_sha256),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        (
            "publication_trace_sha256",
            &prerequisite.publication_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.final_journal_entry_count <= prerequisite.initial_journal_entry_count
        || prerequisite.rdp_renderer_journal_entry_count == 0
        || prerequisite.rdp_renderer_journal_declaration_count == 0
        || prerequisite.renderer_publication_count == 0
        || prerequisite.rdp_renderer_journal_entry_count
            > prerequisite.final_journal_entry_count - prerequisite.initial_journal_entry_count
        || prerequisite.rdp_renderer_journal_declaration_count
            < prerequisite.rdp_renderer_journal_entry_count
        || prerequisite.rdp_renderer_journal_entry_count > prerequisite.renderer_publication_count
    {
        return Err(error(
            "RDP renderer runtime prerequisite lacks a fresh executable-byte publication",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "RDP renderer runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256
        != recompute_rdp_renderer_runtime_prerequisite_receipt(prerequisite)?
    {
        return Err(error(
            "RDP renderer runtime prerequisite receipt digest mismatch",
        ));
    }
    Ok(())
}

pub(super) fn recompute_rdp_renderer_runtime_prerequisite_receipt(
    prerequisite: &RdpRendererWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:rdp-renderer-writer-runtime-state-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.abi_host_catalog_receipt_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update(prerequisite.build_receipt_schema.to_be_bytes());
    hasher.update([
        prerequisite.aot_runtime as u8,
        prerequisite.production_aot as u8,
        prerequisite.dev_interpreter as u8,
    ]);
    hasher.update(prerequisite.trace_epoch_id.to_be_bytes());
    hasher.update(prerequisite.initial_journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.final_journal_entry_count.to_be_bytes());
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.rdp_renderer_journal_entry_count.to_be_bytes());
    hasher.update(
        prerequisite
            .rdp_renderer_journal_declaration_count
            .to_be_bytes(),
    );
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.renderer_publication_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.publication_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}
