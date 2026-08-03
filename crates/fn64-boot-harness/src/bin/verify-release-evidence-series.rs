use fn64_boot_harness::{
    parse_unsupported_journal, verify_release_evidence_series, ExecutionDestinationSource,
    ParsedUnsupportedJournal, ReleaseGateReport,
};
use std::{collections::BTreeSet, env, ffi::OsString, fs, path::PathBuf, process};

const DETERMINISTIC_MINIMUM: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedProgramLane {
    NoProgramFixture,
    IdentifiedNativeArchive,
    TypedObservedFunction,
    TypedBlockProgram,
}

impl ExpectedProgramLane {
    fn parse(value: OsString) -> Result<Self, String> {
        let value = value
            .into_string()
            .map_err(|_| "--program-lane must be UTF-8".to_owned())?;
        match value.as_str() {
            "no_program_fixture" => Ok(Self::NoProgramFixture),
            "identified_native_archive" => Ok(Self::IdentifiedNativeArchive),
            "typed_observed_function" => Ok(Self::TypedObservedFunction),
            "typed_block_program" => Ok(Self::TypedBlockProgram),
            "typed_function" => Err(
                "--program-lane typed_function is stale and not release-admissible: install the generated observation schema and select typed_observed_function"
                    .to_owned(),
            ),
            "unidentified_native" => Err(
                "--program-lane unidentified_native is not release-admissible: bind the exact linked archive identity and select identified_native_archive"
                    .to_owned(),
            ),
            _ => Err(format!(
                "unsupported --program-lane {value:?}; expected no_program_fixture, identified_native_archive, typed_observed_function, or typed_block_program"
            )),
        }
    }

    fn verify(self, source: &ExecutionDestinationSource) -> Result<(), String> {
        let matched = matches!(
            (self, source),
            (
                Self::NoProgramFixture,
                ExecutionDestinationSource::NoProgram
            ) | (
                Self::IdentifiedNativeArchive,
                ExecutionDestinationSource::NativeArchive { .. }
            ) | (
                Self::TypedObservedFunction,
                ExecutionDestinationSource::TypedObservedFunctionProgram { .. }
            ) | (
                Self::TypedBlockProgram,
                ExecutionDestinationSource::TypedBlockProgram { .. }
            )
        );
        if matched {
            return Ok(());
        }
        Err(format!(
            "report program evidence {:?} does not match selected authoritative lane {self:?}; rerun with the installed lane declared by private-input readiness, never relabel an existing report",
            source
        ))
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("verify-release-evidence-series: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut raw_arguments = env::args_os().skip(1);
    if raw_arguments.next().as_deref() != Some(std::ffi::OsStr::new("--program-lane")) {
        return Err(usage());
    }
    let expected_program_lane =
        ExpectedProgramLane::parse(raw_arguments.next().ok_or_else(usage)?)?;
    let arguments: Vec<_> = raw_arguments.collect();
    if arguments.is_empty() {
        return Err(usage());
    }
    if arguments.len() % 2 != 0 {
        return Err("each release report must be followed by its unsupported journal".to_owned());
    }

    let paths: Vec<(PathBuf, PathBuf)> = arguments
        .chunks_exact(2)
        .map(|pair| (PathBuf::from(&pair[0]), PathBuf::from(&pair[1])))
        .collect();
    validate_distinct_paths(&paths)?;

    let mut evidence: Vec<(ReleaseGateReport, ParsedUnsupportedJournal)> = Vec::new();
    for (report_path, journal_path) in &paths {
        let report_bytes = fs::read(report_path)
            .map_err(|error| format!("read {}: {error}", report_path.display()))?;
        let report = serde_json::from_slice::<ReleaseGateReport>(&report_bytes)
            .map_err(|error| format!("parse {}: {error}", report_path.display()))?;
        expected_program_lane
            .verify(&report.execution_destinations.source)
            .map_err(|error| format!("{}: {error}", report_path.display()))?;
        let journal_bytes = fs::read(journal_path)
            .map_err(|error| format!("read {}: {error}", journal_path.display()))?;
        let journal = parse_unsupported_journal(&journal_bytes)
            .map_err(|error| format!("parse {}: {error}", journal_path.display()))?;
        evidence.push((report, journal));
    }

    let verified = verify_release_evidence_series(&evidence, DETERMINISTIC_MINIMUM)
        .map_err(|error| error.to_string())?;
    println!(
        "verified {} consecutive report+journal pairs: cycle={} scenario={} report_sha256={}",
        verified.count, verified.guest_cycle, verified.scenario, verified.report_sha256
    );
    Ok(())
}

fn usage() -> String {
    "usage: verify-release-evidence-series --program-lane {no_program_fixture|identified_native_archive|typed_observed_function|typed_block_program} REPORT.json JOURNAL.jsonl ...".to_owned()
}

fn validate_distinct_paths(paths: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut distinct_reports = BTreeSet::new();
    let mut distinct_journals = BTreeSet::new();
    for (report_path, journal_path) in paths {
        let canonical_report = require_distinct(report_path, &mut distinct_reports, "report")?;
        let canonical_journal = require_distinct(journal_path, &mut distinct_journals, "journal")?;
        if canonical_report == canonical_journal {
            return Err(format!(
                "{} cannot be both a report and its unsupported journal",
                report_path.display()
            ));
        }
    }
    Ok(())
}

fn require_distinct(
    path: &PathBuf,
    seen: &mut BTreeSet<PathBuf>,
    kind: &str,
) -> Result<PathBuf, String> {
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("resolve {}: {error}", path.display()))?;
    if !seen.insert(canonical.clone()) {
        return Err(format!(
            "{kind} path {} is repeated; retain one output per invocation",
            path.display()
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn existing(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name)
    }

    #[test]
    fn rejects_duplicate_report_duplicate_journal_and_same_pair_path() {
        let manifest = existing("Cargo.toml");
        let library = existing("src/lib.rs");
        let gate = existing("src/release_gate/mod.rs");
        assert!(validate_distinct_paths(&[
            (library.clone(), manifest.clone()),
            (library.clone(), gate.clone()),
        ])
        .unwrap_err()
        .contains("report path"));
        assert!(validate_distinct_paths(&[
            (library.clone(), manifest.clone()),
            (gate.clone(), manifest.clone()),
        ])
        .unwrap_err()
        .contains("journal path"));
        assert!(validate_distinct_paths(&[(library.clone(), library)])
            .unwrap_err()
            .contains("both a report"));
    }

    #[test]
    fn program_lane_parser_rejects_nonauthoritative_preflight_choices() {
        let typed = ExpectedProgramLane::parse(OsString::from("typed_function")).unwrap_err();
        assert!(typed.contains("select typed_observed_function"));
        let native = ExpectedProgramLane::parse(OsString::from("unidentified_native")).unwrap_err();
        assert!(native.contains("bind the exact linked archive identity"));
    }

    #[test]
    fn selected_program_lane_must_match_the_v16_report_source() {
        let native = ExecutionDestinationSource::NativeArchive {
            artifact_sha256: "11".repeat(32),
        };
        let block = ExecutionDestinationSource::TypedBlockProgram {
            program_sha256: "22".repeat(32),
            dispatch_artifact_sha256: "33".repeat(32),
        };
        let function = ExecutionDestinationSource::TypedObservedFunctionProgram {
            artifact_sha256: "44".repeat(32),
        };
        ExpectedProgramLane::IdentifiedNativeArchive
            .verify(&native)
            .unwrap();
        ExpectedProgramLane::TypedBlockProgram
            .verify(&block)
            .unwrap();
        ExpectedProgramLane::TypedObservedFunction
            .verify(&function)
            .unwrap();
        let error = ExpectedProgramLane::TypedBlockProgram
            .verify(&native)
            .unwrap_err();
        assert!(error.contains("never relabel an existing report"));
    }
}
