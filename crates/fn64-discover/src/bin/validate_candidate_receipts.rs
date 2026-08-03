//! Validate one retained discovery-only candidate receipt bundle.

use fn64_discover::candidate_corroboration::{
    validate_discovery_only_tool_claims_v1, DiscoveryOnlyReceiptBundle,
};
use fn64_discover::candidate_relation_report::{
    probe_baseline_unreached_candidates_v1, report_candidate_native_relations_v1,
    MAX_UNREACHED_PROBE_BANK_BYTES,
};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MAX_QUEUE_REQUEST_BYTES: u64 = MIB;
const MAX_QUEUE_RECEIPT_BYTES: u64 = MIB;
const MAX_ATTEMPT_RESULT_BYTES: u64 = MIB;
const MAX_RUNNER_RECEIPT_BYTES: u64 = MIB;
const MAX_RUNNER_REQUEST_BYTES: u64 = MIB;
const MAX_EVIDENCE_BYTES: u64 = 4 * MIB;
const MAX_CONFIG_BYTES: u64 = MIB;
const MAX_TOOL_MANIFEST_BYTES: u64 = MIB;
const MAX_PROVIDER_JSONL_BYTES: u64 = 16 * MIB;
const MAX_TOOL_CLAIMS_BYTES: u64 = 16 * MIB;
const MAX_SNAPSHOT_BYTES: u64 = 32 * MIB;

const REQUIRED: [&str; 12] = [
    "--queue-request",
    "--queue-receipt",
    "--attempt-result",
    "--runner-receipt",
    "--runner-request",
    "--evidence",
    "--config",
    "--tool-manifest",
    "--provider-jsonl",
    "--tool-claims",
    "--snapshot",
    "--bank-bytes",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("validate_candidate_receipts: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut paths = BTreeMap::new();
    let mut bank_index = None;
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {option}\n{}", usage()))?;
        if option == "--bank-index" {
            if bank_index.is_some() {
                return Err("duplicate --bank-index".into());
            }
            bank_index = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "--bank-index must be a nonnegative integer")?,
            );
        } else if REQUIRED.contains(&option.as_str()) {
            if paths.insert(option.clone(), PathBuf::from(value)).is_some() {
                return Err(format!("duplicate {option}"));
            }
        } else {
            return Err(format!("unknown option {option}\n{}", usage()));
        }
    }
    for option in REQUIRED {
        if !paths.contains_key(option) {
            return Err(format!("missing {option}\n{}", usage()));
        }
    }
    let bank_index = bank_index.ok_or_else(|| format!("missing --bank-index\n{}", usage()))?;

    let read = |option: &str, label: &str, limit: u64| -> Result<Vec<u8>, String> {
        let path = paths.get(option).expect("required option was checked");
        read_bounded_stable_regular(path, label, limit)
    };
    let queue_request = read("--queue-request", "queue request", MAX_QUEUE_REQUEST_BYTES)?;
    let terminal_queue_receipt = read(
        "--queue-receipt",
        "terminal queue receipt",
        MAX_QUEUE_RECEIPT_BYTES,
    )?;
    let bank_attempt_result = read(
        "--attempt-result",
        "bank attempt result",
        MAX_ATTEMPT_RESULT_BYTES,
    )?;
    let runner_receipt = read(
        "--runner-receipt",
        "runner receipt",
        MAX_RUNNER_RECEIPT_BYTES,
    )?;
    let runner_request = read(
        "--runner-request",
        "runner request",
        MAX_RUNNER_REQUEST_BYTES,
    )?;
    let evidence = read("--evidence", "runner evidence", MAX_EVIDENCE_BYTES)?;
    let unseeded_config = read("--config", "unseeded configuration", MAX_CONFIG_BYTES)?;
    let unseeded_tool_manifest = read(
        "--tool-manifest",
        "unseeded tool manifest",
        MAX_TOOL_MANIFEST_BYTES,
    )?;
    let provider_jsonl = read(
        "--provider-jsonl",
        "provider JSONL",
        MAX_PROVIDER_JSONL_BYTES,
    )?;
    let tool_claims = read("--tool-claims", "tool claim set", MAX_TOOL_CLAIMS_BYTES)?;
    let snapshot = read("--snapshot", "program snapshot", MAX_SNAPSHOT_BYTES)?;
    let bank_bytes = read(
        "--bank-bytes",
        "materialized bank",
        MAX_UNREACHED_PROBE_BANK_BYTES as u64,
    )?;

    let validated = validate_discovery_only_tool_claims_v1(DiscoveryOnlyReceiptBundle {
        queue_request: &queue_request,
        terminal_queue_receipt: &terminal_queue_receipt,
        bank_attempt_result: &bank_attempt_result,
        runner_receipt: &runner_receipt,
        runner_request: &runner_request,
        evidence: &evidence,
        unseeded_config: &unseeded_config,
        unseeded_tool_manifest: &unseeded_tool_manifest,
        provider_jsonl: &provider_jsonl,
        tool_claims: &tool_claims,
        snapshot: &snapshot,
        bank_index,
    })
    .map_err(|error| error.to_string())?;

    println!(
        "validated candidate-only receipt bundle: bank_index={} claims={} analyzer_completeness={:?} tool_claims_sha256={}",
        validated.bank_index(),
        validated.claims().claims.len(),
        validated.analyzer_completeness(),
        validated.tool_claim_set_sha256().to_hex(),
    );
    let relations = report_candidate_native_relations_v1(&validated)
        .map_err(|error| format!("snapshot relation report: {error}"))?;
    println!(
        "{}",
        serde_json::to_string(&relations)
            .map_err(|error| format!("serialize native relation report: {error}"))?
    );
    let probe = probe_baseline_unreached_candidates_v1(&validated, &bank_bytes)
        .map_err(|error| format!("unreached candidate probe: {error}"))?;
    println!(
        "{}",
        serde_json::to_string(&probe)
            .map_err(|error| format!("serialize unreached candidate probe: {error}"))?
    );
    Ok(())
}

fn read_bounded_stable_regular(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    let initial =
        std::fs::symlink_metadata(path).map_err(|error| format!("inspecting {label}: {error}"))?;
    if !initial.file_type().is_file() {
        return Err(format!("{label} is not a regular file"));
    }
    if initial.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| format!("opening {label}: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("inspecting opened {label}: {error}"))?;
    if !opened.is_file() {
        return Err(format!("opened {label} is not a regular file"));
    }
    ensure_same_metadata(&initial, &opened, label)?;
    if opened.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }

    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} grew beyond {limit} bytes"));
    }
    if bytes.len() as u64 != opened.len() {
        return Err(format!("{label} changed length while reading"));
    }

    let after_open = file
        .metadata()
        .map_err(|error| format!("reinspecting opened {label}: {error}"))?;
    let after_path = std::fs::symlink_metadata(path)
        .map_err(|error| format!("reinspecting {label} path: {error}"))?;
    ensure_same_metadata(&opened, &after_open, label)?;
    ensure_same_metadata(&opened, &after_path, label)?;
    Ok(bytes)
}

fn ensure_same_metadata(
    expected: &std::fs::Metadata,
    actual: &std::fs::Metadata,
    label: &str,
) -> Result<(), String> {
    let changed = expected.len() != actual.len();
    #[cfg(unix)]
    let changed = changed
        || expected.dev() != actual.dev()
        || expected.ino() != actual.ino()
        || expected.mtime() != actual.mtime()
        || expected.mtime_nsec() != actual.mtime_nsec()
        || expected.ctime() != actual.ctime()
        || expected.ctime_nsec() != actual.ctime_nsec();
    if changed {
        return Err(format!(
            "{label} identity or metadata changed while reading"
        ));
    }
    Ok(())
}

fn usage() -> &'static str {
    "usage: validate_candidate_receipts --bank-index N --queue-request PATH --queue-receipt PATH --attempt-result PATH --runner-receipt PATH --runner-request PATH --evidence PATH --config PATH --tool-manifest PATH --provider-jsonl PATH --tool-claims PATH --snapshot PATH --bank-bytes PATH"
}
