use fn64_boot_harness::{
    load_private_release_run_contract, parse_unsupported_journal, run_rt64_platform_case_series,
    verify_private_release_series_with_runner, verify_release_matrix,
    verify_release_matrix_with_platform_series,
    verify_release_matrix_with_private_and_platform_series,
    verify_release_matrix_with_private_series, ParsedUnsupportedJournal,
    PrivateReleaseSeriesReceipt, ReleaseGateReport, ReleaseMatrixManifest,
    ReleaseMatrixVerification, Rt64PlatformCase, Rt64PlatformTarget, VerifiedPrivateReleaseSeries,
    VerifiedReleaseMatrix,
};
use std::{
    collections::BTreeSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("verify-release-matrix: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let first = args.next().ok_or_else(usage)?;
    if first == OsStr::new("--print-declaration-digests") {
        let manifest_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
        if args.next().is_some() {
            return Err(usage());
        }
        let manifest = read_manifest(&manifest_path)?;
        for scenario in manifest.scenarios {
            println!(
                "{}={}",
                scenario.id,
                scenario.recompute_declaration_sha256()
            );
        }
        return Ok(());
    }
    if first == OsStr::new("--verify-json") {
        let report_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
        if args.next().is_some() {
            return Err(usage());
        }
        let bytes = fs::read(&report_path)
            .map_err(|error| format!("read {}: {error}", report_path.display()))?;
        let report = verify_retained_json(&bytes)
            .map_err(|error| format!("verify {}: {error}", report_path.display()))?;
        println!(
            "verified retained release matrix: scenarios={} reports={} verification_sha256={}",
            report.scenarios.len(),
            report.total_reports,
            report.verification_sha256
        );
        return Ok(());
    }
    let NormalVerificationArguments {
        json,
        manifest_path,
        private_series,
        platform_cases,
        assignments,
    } = parse_normal_verification_arguments(first, args)?;

    let manifest = read_manifest(&manifest_path)?;
    let mut distinct_platform_cases = BTreeSet::new();
    for request in &platform_cases {
        if !distinct_platform_cases.insert((request.target, request.case)) {
            return Err(format!(
                "RT64 platform case {}:{} is requested more than once",
                request.target.id(),
                request.case.id()
            ));
        }
    }
    let verified_private_series = private_series
        .iter()
        .map(load_and_verify_private_series)
        .collect::<Result<Vec<VerifiedPrivateReleaseSeries>, String>>()?;

    let mut distinct_reports = BTreeSet::new();
    let mut distinct_journals = BTreeSet::new();
    let mut evidence = Vec::<(ReleaseGateReport, ParsedUnsupportedJournal)>::new();
    for assignment in assignments {
        let assignment = assignment
            .into_string()
            .map_err(|_| "report/journal assignment is not UTF-8".to_owned())?;
        let (raw_report, raw_journal) = assignment.split_once(',').ok_or_else(usage)?;
        if raw_report.is_empty() || raw_journal.is_empty() {
            return Err(usage());
        }
        let report_path = PathBuf::from(raw_report);
        let journal_path = PathBuf::from(raw_journal);
        let canonical_report = fs::canonicalize(&report_path)
            .map_err(|error| format!("resolve {}: {error}", report_path.display()))?;
        let canonical_journal = fs::canonicalize(&journal_path)
            .map_err(|error| format!("resolve {}: {error}", journal_path.display()))?;
        if canonical_report == canonical_journal {
            return Err(format!(
                "{} cannot be both a report and its unsupported journal",
                report_path.display()
            ));
        }
        if !distinct_reports.insert(canonical_report) {
            return Err(format!(
                "report path {} is repeated; retain one output per invocation",
                report_path.display()
            ));
        }
        if !distinct_journals.insert(canonical_journal) {
            return Err(format!(
                "journal path {} is repeated; retain one output per invocation",
                journal_path.display()
            ));
        }
        let report_bytes = fs::read(&report_path)
            .map_err(|error| format!("read {}: {error}", report_path.display()))?;
        let report = serde_json::from_slice::<ReleaseGateReport>(&report_bytes)
            .map_err(|error| format!("parse {}: {error}", report_path.display()))?;
        let journal_bytes = fs::read(&journal_path)
            .map_err(|error| format!("read {}: {error}", journal_path.display()))?;
        let journal = parse_unsupported_journal(&journal_bytes)
            .map_err(|error| format!("parse {}: {error}", journal_path.display()))?;
        evidence.push((report, journal));
    }

    let private_series_refs = verified_private_series.iter().collect::<Vec<_>>();
    if !platform_cases.is_empty() {
        verify_matrix_without_platform_authority(&manifest, &evidence, &private_series_refs)
            .map_err(|error| {
                format!("preflight release matrix before RT64 platform cases: {error}")
            })?;
    }
    let verified_platform_series = run_preflighted_platform_cases(
        &platform_cases,
        |request| {
            let bound = request
                .private_series_ordinal
                .checked_sub(1)
                .and_then(|index| verified_private_series.get(index))
                .ok_or_else(|| {
                    missing_private_series_error(request, verified_private_series.len())
                })?;
            request
                .case
                .preflight_series_binding(
                    request.target,
                    &request.rt64_source_directory,
                    bound,
                    &evidence,
                )
                .map_err(|error| {
                    format!(
                        "preflight RT64 platform case {}:{}: {error}",
                        request.target.id(),
                        request.case.id()
                    )
                })
        },
        |request| {
            let bound = request
                .private_series_ordinal
                .checked_sub(1)
                .and_then(|index| verified_private_series.get(index))
                .ok_or_else(|| {
                    missing_private_series_error(request, verified_private_series.len())
                })?;
            run_rt64_platform_case_series(
                request.target,
                request.case,
                &request.rt64_source_directory,
                bound,
            )
            .map_err(|error| {
                format!(
                    "run RT64 platform case {}:{}: {error}",
                    request.target.id(),
                    request.case.id()
                )
            })
        },
    )?;

    let platform_series_refs = verified_platform_series.iter().collect::<Vec<_>>();
    let outcome = match (
        private_series_refs.is_empty(),
        platform_series_refs.is_empty(),
    ) {
        (true, true) => verify_release_matrix(&manifest, &evidence),
        (false, true) => {
            verify_release_matrix_with_private_series(&manifest, &evidence, &private_series_refs)
        }
        (true, false) => {
            verify_release_matrix_with_platform_series(&manifest, &evidence, &platform_series_refs)
        }
        (false, false) => verify_release_matrix_with_private_and_platform_series(
            &manifest,
            &evidence,
            &private_series_refs,
            &platform_series_refs,
        ),
    }
    .map_err(|error| error.to_string())?;
    let verified = match outcome {
        ReleaseMatrixVerification::Complete(verified) => verified,
        ReleaseMatrixVerification::Incomplete(incomplete) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ReleaseMatrixVerification::Incomplete(
                        incomplete.clone()
                    ))
                    .map_err(|error| format!("serialize incomplete matrix: {error}"))?
                );
            }
            let preview = incomplete
                .missing
                .iter()
                .take(8)
                .map(|requirement| format!("{}:{}", requirement.class().as_str(), requirement.id()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "certification incomplete: {} of {} profile requirements missing{}{}",
                incomplete.missing.len(),
                incomplete.missing.len() + incomplete.satisfied.len(),
                if preview.is_empty() { "" } else { "; first: " },
                preview
            ));
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&verified)
                .map_err(|error| format!("serialize verified matrix: {error}"))?
        );
        return Ok(());
    }
    println!(
        "verified release matrix: scenarios={} reports={}",
        verified.scenarios.len(),
        verified
            .scenarios
            .iter()
            .map(|scenario| scenario.count)
            .sum::<usize>()
    );
    for scenario in verified.scenarios {
        println!(
            "{}: cycle={} reports={} report_scenario={} report_sha256={}",
            scenario.id,
            scenario.guest_cycle,
            scenario.count,
            scenario.report_scenario,
            scenario.report_sha256
        );
    }
    Ok(())
}

fn verify_matrix_without_platform_authority(
    manifest: &ReleaseMatrixManifest,
    evidence: &[(ReleaseGateReport, ParsedUnsupportedJournal)],
    private_series: &[&VerifiedPrivateReleaseSeries],
) -> Result<ReleaseMatrixVerification, fn64_boot_harness::ReleaseMatrixError> {
    if private_series.is_empty() {
        verify_release_matrix(manifest, evidence)
    } else {
        verify_release_matrix_with_private_series(manifest, evidence, private_series)
    }
}

fn run_preflighted_platform_cases<T>(
    requests: &[PlatformCaseRunRequest],
    mut preflight: impl FnMut(&PlatformCaseRunRequest) -> Result<(), String>,
    mut runner: impl FnMut(&PlatformCaseRunRequest) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    // Keep this as two loops: a later unbindable request must not be
    // discovered only after an earlier request has spent its native series.
    for request in requests {
        preflight(request)?;
    }
    requests.iter().map(&mut runner).collect()
}

fn missing_private_series_error(request: &PlatformCaseRunRequest, supplied: usize) -> String {
    format!(
        "RT64 platform case {}:{} binds private-series ordinal {}, but only {supplied} private series were supplied",
        request.target.id(),
        request.case.id(),
        request.private_series_ordinal,
    )
}

fn load_and_verify_private_series(
    paths: &PrivateSeriesPaths,
) -> Result<VerifiedPrivateReleaseSeries, String> {
    let contract = load_private_release_run_contract(&paths.contract).map_err(|error| {
        format!(
            "load private contract {}: {error}",
            paths.contract.display()
        )
    })?;
    let receipt_bytes = fs::read(&paths.receipt)
        .map_err(|error| format!("read receipt {}: {error}", paths.receipt.display()))?;
    let receipt = serde_json::from_slice::<PrivateReleaseSeriesReceipt>(&receipt_bytes)
        .map_err(|error| format!("parse receipt {}: {error}", paths.receipt.display()))?;
    verify_private_release_series_with_runner(
        &contract,
        &paths.output_directory,
        &receipt,
        &paths.runner_executable,
    )
    .map_err(|error| {
        format!(
            "verify private series contract={} output={} receipt={} runner={}: {error}",
            paths.contract.display(),
            paths.output_directory.display(),
            paths.receipt.display(),
            paths.runner_executable.display()
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
struct NormalVerificationArguments {
    json: bool,
    manifest_path: PathBuf,
    private_series: Vec<PrivateSeriesPaths>,
    platform_cases: Vec<PlatformCaseRunRequest>,
    assignments: Vec<OsString>,
}

#[derive(Debug, PartialEq, Eq)]
struct PrivateSeriesPaths {
    contract: PathBuf,
    output_directory: PathBuf,
    receipt: PathBuf,
    runner_executable: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct PlatformCaseRunRequest {
    target: Rt64PlatformTarget,
    case: Rt64PlatformCase,
    rt64_source_directory: PathBuf,
    private_series_ordinal: usize,
}

fn parse_normal_verification_arguments(
    first: OsString,
    rest: impl IntoIterator<Item = OsString>,
) -> Result<NormalVerificationArguments, String> {
    let mut arguments = std::iter::once(first).chain(rest).peekable();
    let mut json = false;
    let mut private_series = Vec::new();
    let mut platform_cases = Vec::new();
    let manifest_path = loop {
        let argument = arguments.next().ok_or_else(usage)?;
        if argument == OsStr::new("--json") {
            if json {
                return Err("--json may be specified only once".to_owned());
            }
            json = true;
            continue;
        }
        if argument == OsStr::new("--private-series") {
            let contract = arguments.next().ok_or_else(usage)?;
            let output_directory = arguments.next().ok_or_else(usage)?;
            let receipt = arguments.next().ok_or_else(usage)?;
            let runner_executable = arguments.next().ok_or_else(usage)?;
            if [&contract, &output_directory, &receipt, &runner_executable]
                .into_iter()
                .any(|path| is_normal_option(path))
            {
                return Err(
                    "--private-series requires CONTRACT.json OUTPUT_DIRECTORY RECEIPT.json RUNNER_EXECUTABLE"
                        .to_owned(),
                );
            }
            private_series.push(PrivateSeriesPaths {
                contract: PathBuf::from(contract),
                output_directory: PathBuf::from(output_directory),
                receipt: PathBuf::from(receipt),
                runner_executable: PathBuf::from(runner_executable),
            });
            continue;
        }
        if argument == OsStr::new("--private-contract") {
            return Err(
                "--private-contract cannot authorize ROM-class credit; use --private-series CONTRACT.json OUTPUT_DIRECTORY RECEIPT.json RUNNER_EXECUTABLE"
                    .to_owned(),
            );
        }
        if argument == OsStr::new("--rt64-platform-case") {
            let selection = arguments.next().ok_or_else(usage)?;
            let rt64_source_directory = arguments.next().ok_or_else(usage)?;
            let private_series_ordinal = arguments.next().ok_or_else(usage)?;
            if [&selection, &rt64_source_directory, &private_series_ordinal]
                .into_iter()
                .any(|value| is_normal_option(value))
            {
                return Err(
                    "--rt64-platform-case requires TARGET:CASE RT64_DIR PRIVATE_SERIES_ORDINAL"
                        .to_owned(),
                );
            }
            let selection = selection
                .into_string()
                .map_err(|_| "RT64 platform target/case is not UTF-8".to_owned())?;
            let (target, case) = selection.split_once(':').ok_or_else(usage)?;
            let target = Rt64PlatformTarget::from_id(target)
                .ok_or_else(|| format!("unknown RT64 platform target {target:?}"))?;
            let case = Rt64PlatformCase::from_id(case)
                .ok_or_else(|| format!("unknown RT64 platform case {case:?}"))?;
            let private_series_ordinal = private_series_ordinal
                .into_string()
                .map_err(|_| "private-series ordinal is not UTF-8".to_owned())?
                .parse::<usize>()
                .map_err(|_| "private-series ordinal must be a positive integer".to_owned())?;
            if private_series_ordinal == 0 {
                return Err("private-series ordinal must be one-based and positive".to_owned());
            }
            platform_cases.push(PlatformCaseRunRequest {
                target,
                case,
                rt64_source_directory: PathBuf::from(rt64_source_directory),
                private_series_ordinal,
            });
            continue;
        }
        if is_mode_option(&argument) {
            return Err(format!(
                "{} is a standalone mode and cannot be combined with normal matrix verification",
                argument.to_string_lossy()
            ));
        }
        break PathBuf::from(argument);
    };

    let assignments = arguments.collect::<Vec<_>>();
    if assignments.is_empty() {
        return Err(usage());
    }
    if let Some(option) = assignments
        .iter()
        .find(|argument| is_normal_option(argument))
    {
        return Err(format!(
            "option {} must precede MANIFEST.json",
            option.to_string_lossy()
        ));
    }

    Ok(NormalVerificationArguments {
        json,
        manifest_path,
        private_series,
        platform_cases,
        assignments,
    })
}

fn is_normal_option(argument: &OsStr) -> bool {
    argument == OsStr::new("--json")
        || argument == OsStr::new("--private-series")
        || argument == OsStr::new("--private-contract")
        || argument == OsStr::new("--rt64-platform-case")
        || is_mode_option(argument)
}

fn is_mode_option(argument: &OsStr) -> bool {
    argument == OsStr::new("--print-declaration-digests") || argument == OsStr::new("--verify-json")
}

fn verify_retained_json(bytes: &[u8]) -> Result<VerifiedReleaseMatrix, String> {
    #[derive(serde::Deserialize)]
    struct RetainedSchema {
        schema: String,
    }

    let schema = serde_json::from_slice::<RetainedSchema>(bytes)
        .map_err(|error| format!("parse retained release-matrix schema: {error}"))?;
    if schema.schema != fn64_boot_harness::VERIFIED_RELEASE_MATRIX_SCHEMA {
        return Err(format!(
            "unsupported verified release-matrix schema {:?}",
            schema.schema
        ));
    }
    let report = serde_json::from_slice::<VerifiedReleaseMatrix>(bytes)
        .map_err(|error| format!("parse retained release matrix: {error}"))?;
    report
        .verify_integrity()
        .map_err(|error| error.to_string())?;
    Ok(report)
}

fn read_manifest(path: &Path) -> Result<ReleaseMatrixManifest, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice::<ReleaseMatrixManifest>(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn usage() -> String {
    "usage: verify-release-matrix [--json] [--private-series CONTRACT.json OUTPUT_DIRECTORY RECEIPT.json RUNNER_EXECUTABLE]... [--rt64-platform-case TARGET:CASE RT64_DIR PRIVATE_SERIES_ORDINAL]... MANIFEST.json REPORT.json,JOURNAL.jsonl ...\n       verify-release-matrix --print-declaration-digests MANIFEST.json\n       verify-release-matrix --verify-json VERIFIED.json".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn parse_normal(arguments: &[&str]) -> Result<NormalVerificationArguments, String> {
        let mut arguments = arguments.iter().map(OsString::from);
        let first = arguments.next().ok_or_else(usage)?;
        parse_normal_verification_arguments(first, arguments)
    }

    fn empty_retained(schema: &str, verification_sha256: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": schema,
            "manifest_sha256": "11".repeat(32),
            "profile": {
                "schema": fn64_boot_harness::FULL_PARITY_V1_SCHEMA,
                "definition_sha256": fn64_boot_harness::FULL_PARITY_V1_DEFINITION_SHA256
            },
            "total_reports": 0,
            "scenarios": [],
            "assignments": [],
            "platform_case_authorities": [],
            "verification_sha256": verification_sha256
        }))
        .unwrap()
    }

    #[test]
    fn retained_cli_rejects_historical_schemas_before_deserializing_their_wire_shape() {
        for schema in [
            "fn64.verified-release-matrix.v2",
            "fn64.verified-release-matrix.v3",
            "fn64.verified-release-matrix.v4",
            "fn64.verified-release-matrix.v5",
            "fn64.verified-release-matrix.v6",
            "fn64.verified-release-matrix.v7",
            "fn64.verified-release-matrix.v8",
            "fn64.verified-release-matrix.v9",
            "fn64.verified-release-matrix.v10",
            "fn64.verified-release-matrix.v11",
            "fn64.verified-release-matrix.v12",
            "fn64.verified-release-matrix.v13",
            "fn64.verified-release-matrix.v14",
            "fn64.verified-release-matrix.v15",
            "fn64.verified-release-matrix.v16",
            "fn64.verified-release-matrix.v17",
        ] {
            let error =
                verify_retained_json(&empty_retained(schema, &"00".repeat(32))).unwrap_err();
            assert!(error.contains("unsupported verified release-matrix schema"));
        }
    }

    #[test]
    fn retained_cli_rejects_empty_current_schema_before_digest() {
        let error = verify_retained_json(&empty_retained(
            fn64_boot_harness::VERIFIED_RELEASE_MATRIX_SCHEMA,
            &"00".repeat(32),
        ))
        .unwrap_err();
        assert!(error.contains("release matrix has 0 scenarios"));
    }

    #[test]
    fn normal_parser_preserves_the_report_only_cli_shape() {
        let parsed = parse_normal(&["--json", "matrix.json", "report.json,journal.jsonl"])
            .expect("legacy report-only invocation parses");
        assert!(parsed.json);
        assert_eq!(parsed.manifest_path, PathBuf::from("matrix.json"));
        assert!(parsed.private_series.is_empty());
        assert!(parsed.platform_cases.is_empty());
        assert_eq!(
            parsed.assignments,
            vec![OsString::from("report.json,journal.jsonl")]
        );
    }

    #[test]
    fn normal_parser_retains_repeated_private_series_in_argument_order() {
        let parsed = parse_normal(&[
            "--private-series",
            "retail.json",
            "retail-series",
            "retail-receipt.json",
            "retail-runner",
            "--json",
            "--private-series",
            "homebrew.json",
            "homebrew-series",
            "homebrew-receipt.json",
            "homebrew-runner",
            "matrix.json",
            "a.json,a.jsonl",
            "b.json,b.jsonl",
        ])
        .expect("verified-series invocation parses");
        assert!(parsed.json);
        assert_eq!(parsed.manifest_path, PathBuf::from("matrix.json"));
        assert_eq!(
            parsed.private_series,
            vec![
                PrivateSeriesPaths {
                    contract: PathBuf::from("retail.json"),
                    output_directory: PathBuf::from("retail-series"),
                    receipt: PathBuf::from("retail-receipt.json"),
                    runner_executable: PathBuf::from("retail-runner"),
                },
                PrivateSeriesPaths {
                    contract: PathBuf::from("homebrew.json"),
                    output_directory: PathBuf::from("homebrew-series"),
                    receipt: PathBuf::from("homebrew-receipt.json"),
                    runner_executable: PathBuf::from("homebrew-runner"),
                }
            ]
        );
        assert_eq!(parsed.assignments.len(), 2);
        assert!(parsed.platform_cases.is_empty());
    }

    #[test]
    fn normal_parser_binds_platform_case_to_one_based_private_series() {
        let parsed = parse_normal(&[
            "--private-series",
            "rt64.json",
            "rt64-series",
            "rt64-receipt.json",
            "rt64-runner",
            "--rt64-platform-case",
            "macos-metal:resolution-downsample",
            "/tmp/pinned-rt64",
            "1",
            "matrix.json",
            "report.json,journal.jsonl",
        ])
        .expect("platform case invocation parses");
        assert_eq!(
            parsed.platform_cases,
            vec![PlatformCaseRunRequest {
                target: Rt64PlatformTarget::MacosMetal,
                case: Rt64PlatformCase::ResolutionDownsample,
                rt64_source_directory: PathBuf::from("/tmp/pinned-rt64"),
                private_series_ordinal: 1,
            }]
        );
        assert_eq!(parsed.private_series.len(), 1);
    }

    #[test]
    fn normal_parser_rejects_unknown_or_zero_platform_case_binding() {
        for selection in ["unknown:resolution-downsample", "macos-metal:unknown"] {
            assert!(parse_normal(&[
                "--rt64-platform-case",
                selection,
                "/tmp/pinned-rt64",
                "1",
                "matrix.json",
                "report.json,journal.jsonl",
            ])
            .is_err());
        }
        let zero = parse_normal(&[
            "--rt64-platform-case",
            "macos-metal:resolution-downsample",
            "/tmp/pinned-rt64",
            "0",
            "matrix.json",
            "report.json,journal.jsonl",
        ])
        .unwrap_err();
        assert!(zero.contains("one-based"), "{zero}");
    }

    #[test]
    fn normal_parser_rejects_standalone_modes_after_private_series_options() {
        for mode in ["--verify-json", "--print-declaration-digests"] {
            let error = parse_normal(&[
                "--private-series",
                "authority.json",
                "series",
                "receipt.json",
                "runner",
                mode,
                "artifact.json",
            ])
            .unwrap_err();
            assert!(error.contains("standalone mode"), "{error}");
        }
    }

    #[test]
    fn normal_parser_rejects_options_after_the_manifest() {
        for option in [
            "--json",
            "--private-series",
            "--private-contract",
            "--rt64-platform-case",
            "--verify-json",
        ] {
            let error =
                parse_normal(&["matrix.json", "report.json,journal.jsonl", option]).unwrap_err();
            assert!(error.contains("must precede MANIFEST.json"), "{error}");
        }
    }

    #[test]
    fn normal_parser_rejects_bare_contract_authority() {
        let error = parse_normal(&[
            "--private-contract",
            "contract.json",
            "matrix.json",
            "report.json,journal.jsonl",
        ])
        .unwrap_err();
        assert!(
            error.contains("cannot authorize ROM-class credit"),
            "{error}"
        );
    }

    #[test]
    fn normal_parser_rejects_incomplete_private_series_tuple() {
        let error = parse_normal(&[
            "--private-series",
            "contract.json",
            "output-directory",
            "receipt.json",
        ])
        .unwrap_err();
        assert!(error.contains("usage:"), "{error}");

        let error = parse_normal(&[
            "--private-series",
            "contract.json",
            "--json",
            "receipt.json",
            "runner",
            "matrix.json",
            "report.json,journal.jsonl",
        ])
        .unwrap_err();
        assert!(error.contains("requires CONTRACT.json"), "{error}");
    }

    #[test]
    fn unbindable_platform_request_invokes_no_native_runner() {
        let requests = vec![
            PlatformCaseRunRequest {
                target: Rt64PlatformTarget::MacosMetal,
                case: Rt64PlatformCase::BackendLifecycle,
                rt64_source_directory: PathBuf::from("/private/pinned-rt64"),
                private_series_ordinal: 1,
            },
            PlatformCaseRunRequest {
                target: Rt64PlatformTarget::MacosMetal,
                case: Rt64PlatformCase::ResolutionDownsample,
                rt64_source_directory: PathBuf::from("/private/pinned-rt64"),
                private_series_ordinal: 1,
            },
        ];
        let preflights = Cell::new(0);
        let native_runs = Cell::new(0);
        let error = run_preflighted_platform_cases(
            &requests,
            |request| {
                preflights.set(preflights.get() + 1);
                if request.case == Rt64PlatformCase::ResolutionDownsample {
                    Err("stale RT64 adapter source identity".to_owned())
                } else {
                    Ok(())
                }
            },
            |_| {
                native_runs.set(native_runs.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.contains("stale RT64 adapter"));
        assert_eq!(preflights.get(), 2);
        assert_eq!(native_runs.get(), 0);
    }
}
