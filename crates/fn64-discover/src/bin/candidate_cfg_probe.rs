//! Path-free stdout bridge for bounded candidate CFG diagnostics.

use fn64_discover::candidate_cfg_probe::run_candidate_cfg_probe_v1;
use fn64_discover::snapshot::ProgramSnapshotV1;
use fn64_discover::tool_claims::{validate_tool_claim_set_v1, ToolClaimSetV1};
use serde::de::DeserializeOwned;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 128 * MIB;
const MAX_TOOL_CLAIMS_BYTES: u64 = 128 * MIB;
const MAX_BANK_BYTES: u64 = 128 * MIB;

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("candidate-cfg-probe: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let snapshot_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let claims_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let bank = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    let bank_path = args.next().map(PathBuf::from).ok_or_else(usage)?;

    let snapshot: ProgramSnapshotV1 =
        read_json(&snapshot_path, "program snapshot", MAX_SNAPSHOT_BYTES)?;
    let mut claims: ToolClaimSetV1 =
        read_json(&claims_path, "tool claim set", MAX_TOOL_CLAIMS_BYTES)?;
    for (index, path) in args.enumerate() {
        let extra: ToolClaimSetV1 = read_json(
            &PathBuf::from(path),
            &format!("additional tool claim set {index}"),
            MAX_TOOL_CLAIMS_BYTES,
        )?;
        if extra.program_snapshot_sha256 != claims.program_snapshot_sha256 {
            return Err("additional tool claim set has a different snapshot".into());
        }
        claims.sources.extend(extra.sources);
        claims.claims.extend(extra.claims);
    }
    claims.sources.sort_by_key(|source| source.source_sha256);
    claims.claims.sort_by_key(|claim| claim.claim_id);
    let bank_bytes = read_bounded_regular(&bank_path, "materialized bank", MAX_BANK_BYTES)?;
    validate_tool_claim_set_v1(&snapshot, &claims)
        .map_err(|error| format!("merged tool claims rejected: {error}"))?;
    let report = run_candidate_cfg_probe_v1(&snapshot, &claims, &bank, &bank_bytes)
        .map_err(|error| format!("candidate analysis rejected: {error}"))?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)
        .map_err(|error| format!("serializing diagnostics: {error}"))?;
    println!();
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path, label: &str, limit: u64) -> Result<T, String> {
    let bytes = read_bounded_regular(path, label, limit)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parsing {label}: {error}"))
}

fn read_bounded_regular(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
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
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspecting opened {label}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{label} is not a regular file"));
    }
    ensure_same_metadata(&initial, &metadata, label)?;
    if metadata.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} grew beyond {limit} bytes"));
    }
    if bytes.len() as u64 != metadata.len() {
        return Err(format!("{label} changed length while reading"));
    }
    let after_open = file
        .metadata()
        .map_err(|error| format!("reinspecting opened {label}: {error}"))?;
    let after_path = std::fs::symlink_metadata(path)
        .map_err(|error| format!("reinspecting {label} path: {error}"))?;
    ensure_same_metadata(&metadata, &after_open, label)?;
    ensure_same_metadata(&metadata, &after_path, label)?;
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

fn usage() -> String {
    "usage: candidate_cfg_probe PROGRAM_SNAPSHOT.json TOOL_CLAIMS.json BANK MATERIALIZED_BANK.bin [ADDITIONAL_TOOL_CLAIMS.json ...]"
        .into()
}
