#![allow(clippy::module_inception)]
use super::*;

/// Parse and semantically validate exactly one Bootstrap child report.
///
/// This does not launch a child and does not mint authority. The future series
/// owner must supply a fresh OS-random nonce for each directly owned launch;
/// replaying the same bytes under another challenge fails here.
pub fn parse_generated_runner_bootstrap_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerBootstrapRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes).map_err(|source| {
        error(format!(
            "bootstrap runtime child output is not UTF-8: {source}"
        ))
    })?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner bootstrap runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner bootstrap runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no bootstrap runtime report"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner bootstrap runtime report: {source}"
        ))
    })?;
    validate_generated_runner_bootstrap_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_bootstrap_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_bootstrap_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error(
            "bootstrap runtime series authority failed self-validation",
        ));
    }
    Ok(series)
}

pub(super) fn run_bootstrap_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(BOOTSTRAP_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..BOOTSTRAP_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain bootstrap audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a bootstrap audit nonce"));
        }
        let launched = launch_bootstrap_runtime_child(build, nonce, run_index);
        let post_launch_integrity = build.revalidate_selected_binary();
        post_launch_integrity?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_bootstrap_runtime_series(&build.evidence, &observed)?;
    validate_bootstrap_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn bootstrap_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Bootstrap,
    )?;
    Ok(command)
}

pub(super) fn launch_bootstrap_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerBootstrapRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        bootstrap_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Bootstrap,
    )?;
    parse_generated_runner_bootstrap_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn semantic_bootstrap_report_sha256(
    report: &GeneratedRunnerBootstrapRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize bootstrap runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_bootstrap_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerBootstrapRuntimeReportV1)],
) -> Result<GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != BOOTSTRAP_RUNTIME_SERIES_RUNS {
        return Err(error("bootstrap runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-bootstrap-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("bootstrap runtime series repeats a nonce"));
        }
        validate_generated_runner_bootstrap_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = semantic_bootstrap_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|baseline| baseline != &semantic)
        {
            return Err(error(
                "bootstrap runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1,
        run_count: BOOTSTRAP_RUNTIME_SERIES_RUNS as u8,
        build_authority_sha256: build.authority_sha256.clone(),
        selected_binary_sha256: build.selected_binary_sha256.clone(),
        private_build_inputs_sha256: build.private_build_inputs_sha256.clone(),
        build_identity_sha256: report.build_identity_sha256.clone(),
        program_identity_sha256: report.program_identity_sha256.clone(),
        program_model_sha256: prerequisite.program_model_sha256.clone(),
        bootstrap_receipt_sha256: prerequisite.bootstrap_receipt_sha256.clone(),
        rom_sha256: prerequisite.rom_sha256.clone(),
        resolver_install_sha256: prerequisite.resolver_install_sha256.clone(),
        generation_catalog_sha256: prerequisite.generation_catalog_sha256.clone(),
        journal_root_sha256: prerequisite.journal_entry.journal_root_sha256.clone(),
        final_watched_sha256: prerequisite.final_watched_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = bootstrap_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn bootstrap_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-bootstrap-series.v1\0");
    push_bytes(&mut digest, evidence.schema.as_bytes());
    digest.update([evidence.run_count]);
    for value in [
        &evidence.build_authority_sha256,
        &evidence.selected_binary_sha256,
        &evidence.private_build_inputs_sha256,
        &evidence.build_identity_sha256,
        &evidence.program_identity_sha256,
        &evidence.program_model_sha256,
        &evidence.bootstrap_receipt_sha256,
        &evidence.rom_sha256,
        &evidence.resolver_install_sha256,
        &evidence.generation_catalog_sha256,
        &evidence.journal_root_sha256,
        &evidence.final_watched_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_bootstrap_runtime_series_evidence(
    evidence: &GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != BOOTSTRAP_RUNTIME_SERIES_RUNS
    {
        return Err(error("bootstrap runtime series has a noncanonical shape"));
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
        (
            "bootstrap_receipt_sha256",
            &evidence.bootstrap_receipt_sha256,
        ),
        ("rom_sha256", &evidence.rom_sha256),
        ("resolver_install_sha256", &evidence.resolver_install_sha256),
        (
            "generation_catalog_sha256",
            &evidence.generation_catalog_sha256,
        ),
        ("journal_root_sha256", &evidence.journal_root_sha256),
        ("final_watched_sha256", &evidence.final_watched_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if bootstrap_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("bootstrap runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_bootstrap_runtime_report_v1(
    report: &GeneratedRunnerBootstrapRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1 {
        return Err(error(
            "unsupported generated-runner bootstrap runtime report schema",
        ));
    }
    require_sha256(&report.nonce, "bootstrap runtime report nonce")?;
    if report.nonce != hex(&expected_nonce) {
        return Err(error(
            "generated-runner bootstrap runtime report nonce mismatch",
        ));
    }
    let expected_build_identity_sha256 = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build_identity_sha256
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner bootstrap report does not bind the selected build identity",
        ));
    }
    validate_bootstrap_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_bootstrap_ranges(
    ranges: &[BootstrapWriterWatchedRangeV1],
    field: &str,
    allow_empty: bool,
) -> Result<(), GeneratedRunnerBuildError> {
    if !allow_empty && ranges.is_empty() {
        return Err(error(format!("{field} is empty")));
    }
    let mut previous_end = None;
    for range in ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(format!("{field} is not canonical physical backing")));
        }
        previous_end = Some(range.physical_end);
    }
    Ok(())
}

pub(super) fn range_is_watched(
    range: &BootstrapWriterWatchedRangeV1,
    watched: &[BootstrapWriterWatchedRangeV1],
) -> bool {
    watched.iter().any(|owner| {
        owner.physical_start <= range.physical_start && range.physical_end <= owner.physical_end
    })
}

pub(super) fn validate_bootstrap_runtime_prerequisite(
    prerequisite: &BootstrapWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1 {
        return Err(error("unsupported ABI bootstrap writer receipt schema"));
    }
    for (field, digest) in [
        ("program_model_sha256", &prerequisite.program_model_sha256),
        (
            "bootstrap_receipt_sha256",
            &prerequisite.bootstrap_receipt_sha256,
        ),
        ("rom_sha256", &prerequisite.rom_sha256),
        (
            "resolver_install_sha256",
            &prerequisite.resolver_install_sha256,
        ),
        (
            "generation_catalog_sha256",
            &prerequisite.generation_catalog_sha256,
        ),
        (
            "bootstrap_watched_sha256",
            &prerequisite.bootstrap_watched_sha256,
        ),
        ("before_sha256", &prerequisite.journal_entry.before_sha256),
        ("after_sha256", &prerequisite.journal_entry.after_sha256),
        (
            "journal_root_sha256",
            &prerequisite.journal_entry.journal_root_sha256,
        ),
        ("final_watched_sha256", &prerequisite.final_watched_sha256),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.rom_sha256 != build_identity.normalized_rom_sha256 {
        return Err(error(
            "bootstrap writer receipt does not bind the selected normalized ROM",
        ));
    }
    validate_bootstrap_ranges(
        &prerequisite.watched_ranges,
        "bootstrap watched ranges",
        false,
    )?;
    let declared = prerequisite
        .journal_entry
        .declared_writes
        .iter()
        .map(|write| BootstrapWriterWatchedRangeV1 {
            physical_start: write.physical_start,
            physical_end: write.physical_end,
        })
        .collect::<Vec<_>>();
    if declared.iter().any(|range| {
        range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
    }) {
        return Err(error(
            "bootstrap declared writes contain an invalid physical range",
        ));
    }
    validate_bootstrap_ranges(
        &prerequisite.journal_entry.changed_ranges,
        "bootstrap changed ranges",
        true,
    )?;
    let mut declared_union = declared.clone();
    declared_union.sort_by_key(|range| (range.physical_start, range.physical_end));
    let mut merged_declared: Vec<BootstrapWriterWatchedRangeV1> = Vec::new();
    for range in declared_union {
        if let Some(previous) = merged_declared.last_mut() {
            if range.physical_start <= previous.physical_end {
                previous.physical_end = previous.physical_end.max(range.physical_end);
                continue;
            }
        }
        merged_declared.push(range);
    }
    if declared
        .iter()
        .chain(&prerequisite.journal_entry.changed_ranges)
        .any(|range| !range_is_watched(range, &prerequisite.watched_ranges))
        || prerequisite
            .journal_entry
            .changed_ranges
            .iter()
            .any(|changed| !range_is_watched(changed, &merged_declared))
        || prerequisite.journal_entry.sequence != 0
        || !prerequisite
            .journal_entry
            .invalidated_generations
            .is_empty()
        || prerequisite.journal_entry.after_sha256 != prerequisite.final_watched_sha256
        || prerequisite.bootstrap_watched_sha256 != prerequisite.final_watched_sha256
        || prerequisite
            .initial_generations
            .iter()
            .any(|generation| *generation == 0)
        || prerequisite
            .initial_generations
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(error(
            "bootstrap writer receipt has a noncanonical sequence-zero journal",
        ));
    }
    let canonical_journal_root = recompute_bootstrap_canonical_journal_root(
        &prerequisite.watched_ranges,
        &prerequisite.journal_entry,
    )?;
    if prerequisite.journal_entry.journal_root_sha256 != canonical_journal_root {
        return Err(error(format!(
            "bootstrap canonical journal root mismatch: stored={}, recomputed={canonical_journal_root}",
            prerequisite.journal_entry.journal_root_sha256
        )));
    }
    let recomputed = recompute_bootstrap_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error(format!(
            "bootstrap runtime prerequisite receipt mismatch: stored={}, recomputed={recomputed}",
            prerequisite.receipt_sha256
        )));
    }
    Ok(())
}

pub(super) fn recompute_bootstrap_canonical_journal_root(
    watched_ranges: &[BootstrapWriterWatchedRangeV1],
    entry: &BootstrapMutationBatchV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut initial = Sha256::new();
    initial
        .update(fn64_abi::recompiled::CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1.as_bytes());
    initial.update(decode_sha256(&entry.before_sha256)?);
    for range in watched_ranges {
        initial.update(range.physical_start.to_be_bytes());
        initial.update(range.physical_end.to_be_bytes());
    }

    let mut root = Sha256::new();
    root.update(initial.finalize());
    root.update(entry.sequence.to_be_bytes());
    root.update(decode_sha256(&entry.before_sha256)?);
    root.update(decode_sha256(&entry.after_sha256)?);
    for declaration in &entry.declared_writes {
        root.update([declaration.channel.tag()]);
        root.update(declaration.physical_start.to_be_bytes());
        root.update(declaration.physical_end.to_be_bytes());
    }
    for range in &entry.changed_ranges {
        root.update(range.physical_start.to_be_bytes());
        root.update(range.physical_end.to_be_bytes());
    }
    for generation in &entry.invalidated_generations {
        root.update(generation.to_be_bytes());
    }
    Ok(hex(&root.finalize()))
}

pub(super) fn recompute_bootstrap_runtime_prerequisite_receipt(
    prerequisite: &BootstrapWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:bootstrap-writer-channel-completion-receipt:v1");
    hasher.update((prerequisite.schema.len() as u64).to_be_bytes());
    hasher.update(prerequisite.schema.as_bytes());
    for digest in [
        &prerequisite.program_model_sha256,
        &prerequisite.bootstrap_receipt_sha256,
        &prerequisite.rom_sha256,
        &prerequisite.resolver_install_sha256,
        &prerequisite.generation_catalog_sha256,
    ] {
        hasher.update(decode_sha256(digest)?);
    }
    hasher.update((prerequisite.watched_ranges.len() as u64).to_be_bytes());
    for range in &prerequisite.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(decode_sha256(&prerequisite.bootstrap_watched_sha256)?);
    hasher.update((prerequisite.initial_generations.len() as u64).to_be_bytes());
    for generation in &prerequisite.initial_generations {
        hasher.update(generation.to_be_bytes());
    }
    let entry = &prerequisite.journal_entry;
    hasher.update(entry.sequence.to_be_bytes());
    hasher.update((entry.declared_writes.len() as u64).to_be_bytes());
    for declaration in &entry.declared_writes {
        hasher.update([declaration.channel.tag()]);
        hasher.update(declaration.physical_start.to_be_bytes());
        hasher.update(declaration.physical_end.to_be_bytes());
    }
    hasher.update((entry.changed_ranges.len() as u64).to_be_bytes());
    for range in &entry.changed_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(decode_sha256(&entry.before_sha256)?);
    hasher.update(decode_sha256(&entry.after_sha256)?);
    hasher.update((entry.invalidated_generations.len() as u64).to_be_bytes());
    for generation in &entry.invalidated_generations {
        hasher.update(generation.to_be_bytes());
    }
    hasher.update(decode_sha256(&entry.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    Ok(hex(&hasher.finalize()))
}

pub fn parse_generated_runner_cpu_runtime_report_v1(
    bytes: &[u8],
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<GeneratedRunnerCpuRuntimeReportV1, GeneratedRunnerBuildError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|source| error(format!("CPU runtime child output is not UTF-8: {source}")))?;
    let line = source.strip_suffix('\n').ok_or_else(|| {
        error("generated-runner CPU runtime report is not one LF-terminated line")
    })?;
    if line.contains('\n') || line.contains('\r') {
        return Err(error(
            "generated-runner CPU runtime report contains extra output lines",
        ));
    }
    let json = line
        .strip_prefix(GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1)
        .ok_or_else(|| error("generated-runner child emitted no CPU runtime report envelope"))?;
    let report = serde_json::from_str(json).map_err(|source| {
        error(format!(
            "parse generated-runner CPU runtime report: {source}"
        ))
    })?;
    validate_generated_runner_cpu_runtime_report_v1(&report, expected_nonce, build_identity)?;
    Ok(report)
}

pub fn run_wm2000_generated_runner_cpu_runtime_series_v1(
    build: VerifiedGeneratedRunnerBuildV1,
) -> Result<VerifiedGeneratedRunnerCpuRuntimeSeriesV1, GeneratedRunnerBuildError> {
    let evidence = run_cpu_runtime_series_evidence_v1(&build)?;
    let series = VerifiedGeneratedRunnerCpuRuntimeSeriesV1 {
        evidence,
        _build: build,
    };
    if !series.has_valid_evidence_hash() {
        return Err(error("CPU runtime series authority failed self-validation"));
    }
    Ok(series)
}

pub(super) fn run_cpu_runtime_series_evidence_v1(
    build: &VerifiedGeneratedRunnerBuildV1,
) -> Result<GeneratedRunnerCpuRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    let mut observed = Vec::with_capacity(CPU_RUNTIME_SERIES_RUNS);
    let mut nonces = BTreeSet::new();
    for run_index in 0..CPU_RUNTIME_SERIES_RUNS {
        build.revalidate_selected_binary()?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|source| error(format!("obtain CPU audit nonce: {source}")))?;
        if !nonces.insert(nonce) {
            return Err(error("OS random source repeated a CPU audit nonce"));
        }
        let launched = launch_cpu_runtime_child(build, nonce, run_index);
        build.revalidate_selected_binary()?;
        observed.push((nonce, launched?));
    }
    let evidence = validate_cpu_runtime_series(&build.evidence, &observed)?;
    validate_cpu_runtime_series_evidence(&evidence)?;
    Ok(evidence)
}

pub(super) fn cpu_runtime_command(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
) -> Result<Command, GeneratedRunnerBuildError> {
    let mut command = Command::new(&build.selected_binary);
    configure_writer_runtime_command(
        &mut command,
        &build.private_inputs,
        nonce,
        WriterRuntimeAuditProtocol::Cpu,
    )?;
    Ok(command)
}

pub(super) fn launch_cpu_runtime_child(
    build: &VerifiedGeneratedRunnerBuildV1,
    nonce: [u8; 32],
    run_index: usize,
) -> Result<GeneratedRunnerCpuRuntimeReportV1, GeneratedRunnerBuildError> {
    let stdout = launch_writer_runtime_child_output(
        cpu_runtime_command(build, nonce)?,
        run_index,
        WriterRuntimeAuditProtocol::Cpu,
    )?;
    parse_generated_runner_cpu_runtime_report_v1(&stdout, nonce, &build.evidence.identity)
}

pub(super) fn cpu_semantic_report_sha256(
    report: &GeneratedRunnerCpuRuntimeReportV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut semantic = report.clone();
    semantic.nonce.clear();
    let bytes = serde_json::to_vec(&semantic)
        .map_err(|source| error(format!("serialize CPU runtime semantics: {source}")))?;
    Ok(hex(&Sha256::digest(bytes)))
}

pub(super) fn validate_cpu_runtime_series(
    build: &GeneratedRunnerBuildEvidenceV1,
    observed: &[([u8; 32], GeneratedRunnerCpuRuntimeReportV1)],
) -> Result<GeneratedRunnerCpuRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError> {
    build.verify_integrity()?;
    if observed.len() != CPU_RUNTIME_SERIES_RUNS {
        return Err(error("CPU runtime series is not exactly ten runs"));
    }
    let mut nonce_set = BTreeSet::new();
    let mut nonce_digest = Sha256::new();
    nonce_digest.update(b"fn64.generated-runner-cpu-runtime-nonces.v1\0");
    let mut baseline_semantic = None;
    for (nonce, report) in observed {
        if !nonce_set.insert(*nonce) {
            return Err(error("CPU runtime series repeats a nonce"));
        }
        validate_generated_runner_cpu_runtime_report_v1(report, *nonce, &build.identity)?;
        let semantic = cpu_semantic_report_sha256(report)?;
        if baseline_semantic
            .as_ref()
            .is_some_and(|value| value != &semantic)
        {
            return Err(error(
                "CPU runtime series reports are not semantically identical",
            ));
        }
        baseline_semantic.get_or_insert(semantic);
    }
    for nonce in nonce_set {
        nonce_digest.update(nonce);
    }
    let report = &observed[0].1;
    let prerequisite = &report.prerequisite;
    let mut evidence = GeneratedRunnerCpuRuntimeSeriesEvidenceV1 {
        schema: VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1,
        run_count: CPU_RUNTIME_SERIES_RUNS as u8,
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
        cpu_store_trace_sha256: prerequisite.cpu_store_trace_sha256.clone(),
        runtime_receipt_sha256: prerequisite.receipt_sha256.clone(),
        semantic_report_sha256: baseline_semantic.expect("exact-ten series has a baseline"),
        nonce_set_sha256: hex(&nonce_digest.finalize()),
        authority_sha256: String::new(),
    };
    evidence.authority_sha256 = cpu_runtime_series_authority_sha256(&evidence)?;
    Ok(evidence)
}

pub(super) fn cpu_runtime_series_authority_sha256(
    evidence: &GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut digest = Sha256::new();
    digest.update(b"fn64.verified-generated-runner-cpu-series.v1\0");
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
        &evidence.cpu_store_trace_sha256,
        &evidence.runtime_receipt_sha256,
        &evidence.semantic_report_sha256,
        &evidence.nonce_set_sha256,
    ] {
        digest.update(decode_sha256(value)?);
    }
    Ok(hex(&digest.finalize()))
}

pub(super) fn validate_cpu_runtime_series_evidence(
    evidence: &GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if evidence.schema != VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1
        || usize::from(evidence.run_count) != CPU_RUNTIME_SERIES_RUNS
    {
        return Err(error("CPU runtime series has a noncanonical shape"));
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
        ("cpu_store_trace_sha256", &evidence.cpu_store_trace_sha256),
        ("runtime_receipt_sha256", &evidence.runtime_receipt_sha256),
        ("semantic_report_sha256", &evidence.semantic_report_sha256),
        ("nonce_set_sha256", &evidence.nonce_set_sha256),
        ("authority_sha256", &evidence.authority_sha256),
    ] {
        require_sha256(value, field)?;
    }
    if cpu_runtime_series_authority_sha256(evidence)? != evidence.authority_sha256 {
        return Err(error("CPU runtime series authority digest mismatch"));
    }
    Ok(())
}

pub(super) fn validate_generated_runner_cpu_runtime_report_v1(
    report: &GeneratedRunnerCpuRuntimeReportV1,
    expected_nonce: [u8; 32],
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    validate_identity(
        build_identity,
        &build_identity.manifest_sha256,
        &build_identity.lock_sha256,
    )?;
    if report.schema != GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1
        || report.nonce != hex(&expected_nonce)
    {
        return Err(error(
            "generated-runner CPU runtime report schema or nonce mismatch",
        ));
    }
    require_sha256(&report.nonce, "CPU runtime report nonce")?;
    let expected_build = hex(&Sha256::digest(
        serde_json::to_vec(build_identity)
            .expect("generated-runner build identity serialization is infallible"),
    ));
    if report.build_identity_sha256 != expected_build
        || report.program_identity_sha256 != build_identity.program_identity_sha256
    {
        return Err(error(
            "generated-runner CPU report does not bind the selected build identity",
        ));
    }
    validate_cpu_runtime_prerequisite(&report.prerequisite, build_identity)
}

pub(super) fn validate_cpu_runtime_prerequisite(
    prerequisite: &CpuWriterRuntimePrerequisiteV1,
    build_identity: &GeneratedRunnerBuildIdentityV1,
) -> Result<(), GeneratedRunnerBuildError> {
    if prerequisite.schema != fn64_abi::recompiled::CPU_WRITER_RUNTIME_STATE_SCHEMA_V1
        || prerequisite.build_receipt_schema != build_identity.build_receipt_schema
        || prerequisite.aot_runtime != build_identity.aot_runtime
        || prerequisite.production_aot != build_identity.production_aot
        || prerequisite.dev_interpreter != build_identity.dev_interpreter
        || !prerequisite.aot_runtime
        || !prerequisite.production_aot
        || prerequisite.dev_interpreter
    {
        return Err(error(
            "CPU runtime prerequisite does not bind the selected production-AOT build",
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
            "cpu_store_trace_sha256",
            &prerequisite.cpu_store_trace_sha256,
        ),
        ("receipt_sha256", &prerequisite.receipt_sha256),
    ] {
        require_sha256(digest, field)?;
    }
    if prerequisite.trace_epoch_id == 0
        || prerequisite.watched_ranges.is_empty()
        || prerequisite.journal_entry_count == 0
        || prerequisite.cpu_store_count == 0
    {
        return Err(error(
            "CPU runtime prerequisite lacks a fresh store epoch or journal state",
        ));
    }
    let mut previous_end = None;
    for range in &prerequisite.watched_ranges {
        if range.physical_start >= range.physical_end
            || usize::try_from(range.physical_end).unwrap() > fn64_recomp_rs::RDRAM_LEN
            || previous_end.is_some_and(|end| range.physical_start <= end)
        {
            return Err(error(
                "CPU runtime prerequisite watched ranges are not canonical",
            ));
        }
        previous_end = Some(range.physical_end);
    }
    let recomputed = recompute_cpu_runtime_prerequisite_receipt(prerequisite)?;
    if prerequisite.receipt_sha256 != recomputed {
        return Err(error("CPU runtime prerequisite receipt digest mismatch"));
    }
    Ok(())
}

pub(super) fn recompute_cpu_runtime_prerequisite_receipt(
    prerequisite: &CpuWriterRuntimePrerequisiteV1,
) -> Result<String, GeneratedRunnerBuildError> {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:cpu-instruction-store-runtime-state-receipt:v1");
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
    hasher.update(prerequisite.cpu_journal_declaration_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.journal_root_sha256)?);
    hasher.update(decode_sha256(&prerequisite.final_watched_sha256)?);
    hasher.update(prerequisite.cpu_store_count.to_be_bytes());
    hasher.update(decode_sha256(&prerequisite.cpu_store_trace_sha256)?);
    Ok(hex(&hasher.finalize()))
}
