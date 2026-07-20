use fn64_boot_harness::{
    parse_unsupported_journal, verify_release_report_series, ParsedUnsupportedJournal,
    ReleaseGateReport,
};
use std::{collections::BTreeSet, env, fs, path::PathBuf, process};

const DETERMINISTIC_MINIMUM: usize = 10;

fn main() {
    if let Err(error) = run() {
        eprintln!("verify-release-series: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let paths: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() || !paths.len().is_multiple_of(2) {
        return Err(
            "usage: verify-release-series REPORT.json JOURNAL.jsonl [REPORT.json JOURNAL.jsonl ...]"
                .to_owned(),
        );
    }

    let mut distinct_paths = BTreeSet::new();
    for path in &paths {
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("resolve {}: {error}", path.display()))?;
        if !distinct_paths.insert(canonical) {
            return Err(format!(
                "report path {} is repeated; retain one output per invocation",
                path.display()
            ));
        }
    }

    let mut evidence: Vec<(ReleaseGateReport, ParsedUnsupportedJournal)> = Vec::new();
    for pair in paths.chunks_exact(2) {
        let report_bytes =
            fs::read(&pair[0]).map_err(|error| format!("read {}: {error}", pair[0].display()))?;
        let report = serde_json::from_slice::<ReleaseGateReport>(&report_bytes)
            .map_err(|error| format!("parse {}: {error}", pair[0].display()))?;
        let journal_bytes =
            fs::read(&pair[1]).map_err(|error| format!("read {}: {error}", pair[1].display()))?;
        let journal = parse_unsupported_journal(&journal_bytes)
            .map_err(|error| format!("parse {}: {error}", pair[1].display()))?;
        evidence.push((report, journal));
    }

    let verified = verify_release_report_series(&evidence, DETERMINISTIC_MINIMUM)
        .map_err(|error| error.to_string())?;
    println!(
        "verified {} consecutive report+journal pairs: cycle={} scenario={} report_sha256={}",
        verified.count, verified.guest_cycle, verified.scenario, verified.report_sha256
    );
    Ok(())
}
