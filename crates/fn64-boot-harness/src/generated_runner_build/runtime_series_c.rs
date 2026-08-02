#![allow(clippy::module_inception)]
use super::*;

pub fn parse_generated_runner_rsp_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerRspRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("RSP runtime child output is not UTF-8: {source}")))?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner RSP runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner RSP runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no RSP runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner RSP runtime report: {source}"
        ))
    })?;
    validate_generated_runner_rsp_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_rsp_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerRspRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_rsp_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerRspRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("RSP runtime series authority failed self-validation"));
    }
    Ok(series)
}

pub(super) fn run_rsp_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerRspRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(RSP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..RSP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain RSP audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an RSP audit nonce"));
        }
        let launched = launch_rsp_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_rsp_runtime_series(&build.evidence, &observed)?;
    validate_rsp_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn rsp_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Rsp,
    )?;
    Ok(command)
}

pub(super) fn launch_rsp_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerRspRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        rsp_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Rsp,
    )?;
    parse_generated_runner_rsp_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn rsp_semantic_report_sha256(
    report: &GeneratedRunnerRspRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize RSP runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_rsp_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerRspRuntimeReportV1)],
) -> Result<GeneratedRunnerRspRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != RSP_RUNTIME_SERIES_RUNS {
        return Err(error("RSP runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-rsp-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("RSP runtime series repeats a nonce"));
        }
        validate_generated_runner_rsp_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = rsp_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "RSP runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerRspRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1,
        run_count: RSP_RUNTIME_SERIES_RUNS as u8,
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
        writeback_trace_sha256: prerequisite.writeback_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = rsp_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn rsp_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerRspRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-rsp-series.v1\0");
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
        &evidence.writeback_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_rsp_runtime_series_evidence(
    evidence: &GeneratedRunnerRspRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != RSP_RUNTIME_SERIES_RUNS
    {
        return Err(error("RSP runtime series has a noncanonical shape"));
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
        ("writeback_trace_sha256", &evidence.writeback_trace_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if rsp_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("RSP runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_rsp_runtime_report_v1(
    report: &GeneratedRunnerRspRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner RSP runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "RSP runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner RSP report does not bind the selected build identity",
        ));
    }
    validate_rsp_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_rsp_runtime_prerequisite(
    prerequisite: &RspWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::RSP_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "RSP runtime prerequisite does not bind the selected production-AOT build",
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
            "writeback_trace_sha256",
            &prerequisite.writeback_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    let publication_count = prerequisite
        .interpreter_writeback_count
        .checked_add(prerequisite.translated_audio_hle_publication_count)
        .ok_or_else(|| error("RSP runtime prerequisite publication count overflow"))?;
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || publication_count == 0
        || prerequisite.writeback_range_count != prerequisite.interpreter_writeback_count
    {
        return Err(error(
            "RSP runtime prerequisite lacks a fresh typed writeback publication",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "RSP runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.receipt_sha256 != recompute_rsp_runtime_prerequisite_receipt(prerequisite)? {
        return Err(error("RSP runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

pub(super) fn recompute_rsp_runtime_prerequisite_receipt(
    prerequisite: &RspWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:rsp-execution-writeback-runtime-state-receipt:v1");
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
    hasher.update(prerequisite.rsp_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.interpreter_writeback_count.to_be_bytes());
    hasher.update(
        prerequisite
            .translated_audio_hle_publication_count
            .to_be_bytes(),
    );
    hasher.update(prerequisite.writeback_range_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.writeback_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_si_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerSiRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("SI runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner SI runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner SI runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no SI runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner SI runtime report: {source}"
        ))
    })?;
    validate_generated_runner_si_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

/// Consume one verified build in a verifier-owned exact-ten SI audit series.
///
/// Every child receives a distinct OS-random nonce and only the retained
/// staged private inputs. The selected binary and all private inputs are
/// revalidated before and after every launch. Success returns a move-only
/// series capability; it does not complete the writer-channel denominator.
pub fn run_wm2000_generated_runner_si_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerSiRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_si_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerSiRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("SI runtime series authority failed self-validation"));
    }
    Ok(series)
}

pub(super) fn run_si_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerSiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(SI_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..SI_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain SI audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an SI audit nonce"));
        }
        let launched = launch_si_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        let report = launched?;
        observed.push((nonce, report));
    }
    let evidence = validate_si_runtime_series(&build.evidence, &observed)?;
    validate_si_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn si_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Si,
    )?;
    Ok(command)
}

#[derive(Clone, Copy)]
pub(super) enum WriterRuntimeAuditProtocol {
    Bootstrap,
    Cpu,
    HostAbi,
    Pi,
    RdpRenderer,
    Rsp,
    Si,
    Sp,
}

impl WriterRuntimeAuditProtocol {
    const fn argument(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1,
        }
    }

    const fn nonce_environment(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1,
        }
    }

    const fn report_prefix(self) -> &'static str {
        match self {
            Self::Bootstrap => GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
            Self::Cpu => GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
            Self::HostAbi => GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
            Self::Pi => GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
            Self::RdpRenderer => GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
            Self::Rsp => GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
            Self::Si => GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
            Self::Sp => GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Cpu => "CPU",
            Self::HostAbi => "Host ABI",
            Self::Pi => "PI",
            Self::RdpRenderer => "RDP renderer",
            Self::Rsp => "RSP",
            Self::Si => "SI",
            Self::Sp => "SP",
        }
    }
}

pub(super) fn configure_writer_runtime_command(
    command: &mut Command,
    inputs: &Wm2000GeneratedRunnerBuildInputsV1,
    nonce: [u8; 32],
    protocol: WriterRuntimeAuditProtocol,
) -> Result<(), GeneratedRunnerBuildError> {
    command
        .arg(protocol.argument())
        .env_clear()
        .env("ROM", &inputs.rom)
        .env("FN64_BOOT_CONTEXT", &inputs.boot_context)
        .env(protocol.nonce_environment(), hex(&nonce))
        .env(
            "FN64_EXECUTABLE_IMAGE_GROUPS",
            inputs
                .executable_image_groups
                .iter()
                .map(|group| group.environment_name.as_str())
                .collect::<Vec<_>>()
                .join(","),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for group in &inputs.executable_image_groups {
        command.env(
            &group.environment_name,
            std::env::join_paths(&group.captures).map_err(|source| {
                error(format!(
                    "join retained staged capture group for {} audit: {source}",
                    protocol.label()
                ))
            })?,
        );
    }
    Ok(())
}

pub(super) fn launch_si_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerSiRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        si_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Si,
    )?;
    parse_generated_runner_si_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn launch_writer_runtime_child_output(
    mut command: Command,
    run_index: usize,
    protocol: WriterRuntimeAuditProtocol,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    let label = protocol.label();
    let mut process = command.spawn().map_err(|source| {
        error(format!(
            "launch generated-runner {label} audit child: {source}"
        ))
    })?;
    let stdout = process
        .stdout
        .take()
        .expect("writer audit command configured piped stdout");
    let stderr = process
        .stderr
        .take()
        .expect("writer audit command configured piped stderr");
    let stdout_reader = thread::spawn(move || read_bounded_output(stdout));
    let stderr_reader = thread::spawn(move || read_bounded_output(stderr));
    let wait = wait_with_watchdog(
        &mut process,
        WRITER_RUNTIME_WATCHDOG,
        "generated-runner writer audit child",
    );
    let status = process
        .try_wait()
        .map_err(|source| error(format!("read {label} audit child status: {source}")))?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| error(format!("{label} audit stdout reader panicked")))?
        .map_err(error)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| error(format!("{label} audit stderr reader panicked")))?
        .map_err(error)?;
    if let Err(wait_error) = wait {
        return Err(error(format!(
            "{label} audit child run {run_index} failed its watchdog: {wait_error}; stdout_bytes={} stdout_sha256={} stdout_tail={}; stderr_bytes={} stderr_sha256={} stderr_tail={}",
            stdout.total_bytes,
            stdout.sha256_hex(),
            stdout.diagnostic_tail(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            stderr.diagnostic_tail(),
        )));
    }
    let status = status.expect("watchdog returned only after child exit");
    if !writer_runtime_outputs_within_limit(stdout.total_bytes, stderr.total_bytes) {
        return Err(error(format!(
            "{label} audit child run {run_index} exceeded the {}-byte output limit: stdout_bytes={} stdout_sha256={} stdout_tail={}; stderr_bytes={} stderr_sha256={} stderr_tail={}",
            WRITER_RUNTIME_OUTPUT_LIMIT,
            stdout.total_bytes,
            stdout.sha256_hex(),
            stdout.diagnostic_tail(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            stderr.diagnostic_tail(),
        )));
    }
    if !status.success() {
        return Err(error(format!(
            "{label} audit child run {run_index} exited {status}: stdout_bytes={} stdout_sha256={} stderr_bytes={} stderr_sha256={}; stderr: {}",
            stdout.total_bytes,
            stdout.sha256_hex(),
            stderr.total_bytes,
            stderr.sha256_hex(),
            bounded_diagnostic(&stderr.bytes),
        )));
    }
    if stderr.total_bytes != 0 {
        return Err(error(format!(
            "{label} audit child run {run_index} emitted stderr: bytes={} sha256={}",
            stderr.total_bytes,
            stderr.sha256_hex(),
        )));
    }
    extract_writer_runtime_report_envelope(&stdout.bytes, protocol)
}

pub(super) fn writer_runtime_outputs_within_limit(stdout_bytes: u64, stderr_bytes: u64) -> bool {
    stdout_bytes <= WRITER_RUNTIME_OUTPUT_LIMIT as u64
        && stderr_bytes <= WRITER_RUNTIME_OUTPUT_LIMIT as u64
}

pub(super) struct BoundedOutput {
    bytes: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    sha256: [u8; 32],
}

impl BoundedOutput {
    fn sha256_hex(&self) -> String {
        hex(&self.sha256)
    }

    fn diagnostic_tail(&self) -> String {
        if self.total_bytes <= self.bytes.len() as u64 {
            bounded_diagnostic(&self.bytes)
        } else {
            bounded_diagnostic(&self.tail)
        }
    }
}

pub(super) fn read_bounded_output(mut input: impl Read) -> Result<BoundedOutput, String> {
    let mut bytes = Vec::new();
    let mut tail = Vec::new();
    let mut total_bytes = 0u64;
    let mut sha256 = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|source| format!("read bounded child output: {source}"))?;
        if count == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(count as u64)
            .ok_or_else(|| "child output byte count overflow".to_owned())?;
        sha256.update(&buffer[..count]);
        tail.extend_from_slice(&buffer[..count]);
        if tail.len() > WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT {
            let excess = tail.len() - WRITER_RUNTIME_DIAGNOSTIC_TAIL_LIMIT;
            tail.drain(..excess);
        }
        if bytes.len() < WRITER_RUNTIME_OUTPUT_LIMIT {
            let retain = count.min(WRITER_RUNTIME_OUTPUT_LIMIT - bytes.len());
            bytes.extend_from_slice(&buffer[..retain]);
        }
    }
    Ok(BoundedOutput {
        bytes,
        tail,
        total_bytes,
        sha256: sha256.finalize().into(),
    })
}

pub(super) fn extract_writer_runtime_report_envelope(
    stdout: &[u8],
    protocol: WriterRuntimeAuditProtocol,
) -> Result<Vec<u8>, GeneratedRunnerBuildError> {
    const PREFIXES: [&str; 8] = [
        GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1,
        GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1,
    ];

    let expected = protocol.report_prefix().as_bytes();
    let mut report = None;
    for line in stdout.split_inclusive(|byte| *byte == b'\n') {
        let Some(prefix) = PREFIXES
            .iter()
            .find(|prefix| line.starts_with(prefix.as_bytes()))
        else {
            continue;
        };
        if prefix.as_bytes() != expected {
            return Err(error(format!(
                "{} audit child emitted a report for another protocol",
                protocol.label()
            )));
        }
        if report.replace(line).is_some() {
            return Err(error(format!(
                "{} audit child emitted multiple runtime reports",
                protocol.label()
            )));
        }
    }
    let report = report.ok_or_else(|| {
        error(format!(
            "{} audit child emitted no runtime report",
            protocol.label()
        ))
    })?;
    if report.len() > WRITER_RUNTIME_REPORT_LIMIT {
        return Err(error(format!(
            "{} audit child report exceeds the {}-byte envelope limit",
            protocol.label(),
            WRITER_RUNTIME_REPORT_LIMIT
        )));
    }
    Ok(report.to_vec())
}

pub(super) fn semantic_report_sha256(
    report: &GeneratedRunnerSiRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize SI runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_si_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerSiRuntimeReportV1)],
) -> Result<GeneratedRunnerSiRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != SI_RUNTIME_SERIES_RUNS {
        return Err(error("SI runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-si-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("SI runtime series repeats a nonce"));
        }
        validate_generated_runner_si_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "SI runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(&build.identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    let mut evidence = GeneratedRunnerSiRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1,
        run_count: SI_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256,
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        si_transition_sha256: prerequisite.si_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = si_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn si_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerSiRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-si-series.v1\0");
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
        &evidence.si_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_si_runtime_series_evidence(
    evidence: &GeneratedRunnerSiRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != SI_RUNTIME_SERIES_RUNS
    {
        return Err(error("SI runtime series has a noncanonical shape"));
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
        ("si_transition_sha256", &evidence.si_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if si_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("SI runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_si_runtime_report_v1(
    report: &GeneratedRunnerSiRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner SI runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "SI runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error("generated-runner SI runtime report nonce mismatch"));
    }
    let identity_bytes = serde_json::to_vec(build_identity)
        .expect("generated-runner build identity serialization is infallible");
    let expected_build_identity_sha256 = hex(&Sha256::digest(identity_bytes));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner SI runtime report does not bind the selected build identity",
        ));
    }
    require_sha256(
        &report.build_identity_sha256,
        "SI runtime report build_identity_sha256",
    )?;
    require_sha256(
        &report.program_identity_sha256,
        "SI runtime report program_identity_sha256",
    )?;
    validate_si_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_si_runtime_prerequisite(
    prerequisite: &SiWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::SI_WRITER_RUNTIME_STATE_SCHEMA_V1 {
        return Err(error(
            "unsupported ABI SI runtime-state prerequisite schema",
        ));
    }
    if prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "SI runtime prerequisite does not bind the selected production-AOT build receipt",
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
        ("si_transition_sha256", &prerequisite.si_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.watched_ranges.is_empty() || prerequisite.journal_entry_count == 0 {
        return Err(error(
            "SI runtime prerequisite lacks validated executable-journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "SI runtime prerequisite watched ranges are not canonical executable backing",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.si_started == 0
        || prerequisite.si_started != prerequisite.si_committed
        || prerequisite.si_pif_to_dram_committed == 0
        || prerequisite.si_pif_to_dram_committed > prerequisite.si_committed
    {
        return Err(error(
            "SI runtime prerequisite contains inconsistent transition counts",
        ));
    }
    let recomputed = recompute_si_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "SI runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

pub(super) fn recompute_si_runtime_prerequisite_receipt(
    prerequisite: &SiWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:si-writer-runtime-state-receipt:v1");
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
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(prerequisite.journal_entry_count.to_be_bytes());
    hasher.update(prerequisite.si_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.si_started.to_be_bytes());
    hasher.update(prerequisite.si_committed.to_be_bytes());
    hasher.update(prerequisite.si_pif_to_dram_committed.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.si_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_sp_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerSpRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("SP runtime child output is not UTF-8: {source}")))?;
    let line = source
        .strip_suffix('\n')
        .ok_or_else(|| error("generated-runner SP runtime report is not one LF-terminated line"))?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner SP runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no SP runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner SP runtime report: {source}"
        ))
    })?;
    validate_generated_runner_sp_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

/// Consume one verified build in a verifier-owned exact-ten SP audit series.
/// Every child receives one fresh OS-random nonce and only retained staged
/// inputs. Pre/post launch revalidation closes replacement of the executable
/// or any private input while the bounded child is running.
pub fn run_wm2000_generated_runner_sp_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerSpRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_sp_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerSpRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("SP runtime series authority failed self-validation"));
    }
    Ok(series)
}

pub(super) fn run_sp_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerSpRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(SP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..SP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain SP audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated an SP audit nonce"));
        }
        let launched = launch_sp_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_sp_runtime_series(&build.evidence, &observed)?;
    validate_sp_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn sp_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Sp,
    )?;
    Ok(command)
}

pub(super) fn launch_sp_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerSpRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        sp_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Sp,
    )?;
    parse_generated_runner_sp_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn semantic_sp_report_sha256(
    report: &GeneratedRunnerSpRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize SP runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_sp_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerSpRuntimeReportV1)],
) -> Result<GeneratedRunnerSpRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != SP_RUNTIME_SERIES_RUNS {
        return Err(error("SP runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-sp-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("SP runtime series repeats a nonce"));
        }
        validate_generated_runner_sp_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_sp_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "SP runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(&build.identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    let mut evidence = GeneratedRunnerSpRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1,
        run_count: SP_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256,
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        abi_host_catalog_receipt_sha256: prerequisite.abi_host_catalog_receipt_sha256.clone(),
        journal_root_sha256: prerequisite.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        sp_transition_sha256: prerequisite.sp_transition_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = sp_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn sp_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerSpRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-sp-series.v1\0");
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
        &evidence.sp_transition_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_sp_runtime_series_evidence(
    evidence: &GeneratedRunnerSpRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != SP_RUNTIME_SERIES_RUNS
    {
        return Err(error("SP runtime series has a noncanonical shape"));
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
        ("sp_transition_sha256", &evidence.sp_transition_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if sp_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("SP runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_sp_runtime_report_v1(
    report: &GeneratedRunnerSpRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner SP runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "SP runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error("generated-runner SP runtime report nonce mismatch"));
    }
    let expected_build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner SP runtime report does not bind the selected build identity",
        ));
    }
    require_sha256(
        &report.build_identity_sha256,
        "SP runtime report build_identity_sha256",
    )?;
    require_sha256(
        &report.program_identity_sha256,
        "SP runtime report program_identity_sha256",
    )?;
    validate_sp_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_sp_runtime_prerequisite(
    prerequisite: &SpWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::SP_WRITER_RUNTIME_STATE_SCHEMA_V1 {
        return Err(error(
            "unsupported ABI SP runtime-state prerequisite schema",
        ));
    }
    if prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "SP runtime prerequisite does not bind the selected production-AOT build receipt",
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
        ("sp_transition_sha256", &prerequisite.sp_transition_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
    {
        return Err(error(
            "SP runtime prerequisite lacks a fresh epoch or validated journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "SP runtime prerequisite watched ranges are not canonical executable backing",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    if prerequisite.sp_started == 0
        || prerequisite.sp_started != prerequisite.sp_committed
        || prerequisite.sp_busy_cleared == 0
        || prerequisite.sp_busy_cleared > prerequisite.sp_committed
        || prerequisite.sp_queued > prerequisite.sp_started
        || prerequisite.sp_rsp_to_rdram_committed == 0
        || prerequisite.sp_rsp_to_rdram_committed > prerequisite.sp_committed
    {
        return Err(error(
            "SP runtime prerequisite contains inconsistent transition counts",
        ));
    }
    let recomputed = recompute_sp_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "SP runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

pub(super) fn recompute_sp_runtime_prerequisite_receipt(
    prerequisite: &SpWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:sp-writer-runtime-state-receipt:v1");
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
    hasher.update(prerequisite.sp_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.sp_started.to_be_bytes());
    hasher.update(prerequisite.sp_queued.to_be_bytes());
    hasher.update(prerequisite.sp_committed.to_be_bytes());
    hasher.update(prerequisite.sp_busy_cleared.to_be_bytes());
    hasher.update(prerequisite.sp_rsp_to_rdram_committed.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.sp_transition_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub(super) fn writer_audit_bundle_authority_sha256(
    evidence: &GeneratedRunnerWriterAuditBundleEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-writer-audit-bundle.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.completed_channels]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    for (tag, authority) in [
        (
            WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
            evidence
                .bootstrap
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_CPU_COMPLETED_V1,
            evidence.cpu.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
            evidence
                .host_abi
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_PI_COMPLETED_V1,
            evidence.pi.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
            evidence
                .rdp_renderer
                .as_ref()
                .map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_RSP_COMPLETED_V1,
            evidence.rsp.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_SI_COMPLETED_V1,
            evidence.si.as_ref().map(|series| &series.authority_sha256),
        ),
        (
            WRITER_AUDIT_SP_COMPLETED_V1,
            evidence.sp.as_ref().map(|series| &series.authority_sha256),
        ),
    ] {
        digest.update([tag]);
        match authority {
            Some(authority) => {
                digest.update([1]);
                digest.update(decode_sha256(authority)?);
            }
            None => digest.update([0]),
        }
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_writer_audit_bundle_evidence(
    evidence: &GeneratedRunnerWriterAuditBundleEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1
        || evidence.completed_channels == 0
        || evidence.completed_channels & !WRITER_AUDIT_COMPLETED_MASK_V1 != 0
    {
        return Err(error("writer audit bundle has a noncanonical shape"));
    }
    let expected_bits = u8::from(evidence.bootstrap.is_some())
        * WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1
        | u8::from(evidence.cpu.is_some()) * WRITER_AUDIT_CPU_COMPLETED_V1
        | u8::from(evidence.host_abi.is_some()) * WRITER_AUDIT_HOST_ABI_COMPLETED_V1
        | u8::from(evidence.pi.is_some()) * WRITER_AUDIT_PI_COMPLETED_V1
        | u8::from(evidence.rdp_renderer.is_some()) * WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1
        | u8::from(evidence.rsp.is_some()) * WRITER_AUDIT_RSP_COMPLETED_V1
        | u8::from(evidence.si.is_some()) * WRITER_AUDIT_SI_COMPLETED_V1
        | u8::from(evidence.sp.is_some()) * WRITER_AUDIT_SP_COMPLETED_V1;
    if evidence.completed_channels != expected_bits {
        return Err(error(
            "writer audit bundle bitmap does not match its series evidence",
        ));
    }
    for (field, value) in [
        ("build_authority_sha256", &evidence.build_authority_sha256),
        ("selected_binary_sha256", &evidence.selected_binary_sha256),
        (
            "private_build_inputs_sha256",
            &evidence.private_build_inputs_sha256,
        ),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if let Some(series) = &evidence.bootstrap {
        validate_bootstrap_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.cpu {
        validate_cpu_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.host_abi {
        validate_host_abi_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.pi {
        validate_pi_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.rdp_renderer {
        validate_rdp_renderer_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.rsp {
        validate_rsp_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.si {
        validate_si_runtime_series_evidence(series)?;
    }
    if let Some(series) = &evidence.sp {
        validate_sp_runtime_series_evidence(series)?;
    }
    let mut common = None;
    for series in [
        evidence.bootstrap.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.cpu.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.host_abi.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.pi.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.rdp_renderer.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.rsp.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.si.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
        evidence.sp.as_ref().map(|series| {
            (
                &series.build_authority_sha256,
                &series.selected_binary_sha256,
                &series.private_build_inputs_sha256,
                &series.build_identity_sha256,
                &series.program_identity_sha256,
                &series.program_model_sha256,
            )
        }),
    ]
    .into_iter()
    .flatten()
    {
        if series.0 != &evidence.build_authority_sha256
            || series.1 != &evidence.selected_binary_sha256
            || series.2 != &evidence.private_build_inputs_sha256
        {
            return Err(error(
                "writer audit bundle contains evidence from another verified build",
            ));
        }
        let identity = (series.3, series.4, series.5);
        if common.is_some_and(|expected| expected != identity) {
            return Err(error(
                "writer audit bundle contains cross-channel identity or program-model mismatch",
            ));
        }
        common.get_or_insert(identity);
    }
    if writer_audit_bundle_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("writer audit bundle authority digest mismatch"));
    }
    Ok(())
}
