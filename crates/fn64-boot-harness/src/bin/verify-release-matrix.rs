use fn64_boot_harness::{
    parse_unsupported_journal, verify_release_matrix, ParsedUnsupportedJournal, ReleaseGateReport,
    ReleaseMatrixManifest, ReleaseMatrixVerification, VerifiedReleaseMatrix,
};
use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
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
    let (json, manifest_path) = if first == OsStr::new("--json") {
        (true, args.next().map(PathBuf::from).ok_or_else(usage)?)
    } else {
        (false, PathBuf::from(first))
    };
    let assignments: Vec<_> = args.collect();
    if assignments.is_empty() {
        return Err(usage());
    }

    let manifest = read_manifest(&manifest_path)?;

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

    let outcome = verify_release_matrix(&manifest, &evidence).map_err(|error| error.to_string())?;
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
    "usage: verify-release-matrix [--json] MANIFEST.json REPORT.json,JOURNAL.jsonl ...\n       verify-release-matrix --print-declaration-digests MANIFEST.json\n       verify-release-matrix --verify-json VERIFIED.json".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
