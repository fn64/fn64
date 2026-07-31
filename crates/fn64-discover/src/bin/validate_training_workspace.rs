//! Validate one sealed, answer-key-free training workspace in a single pass.

use fn64_discover::snapshot_workspace::{
    validate_snapshot_workspace_streaming, SnapshotWorkspaceError,
};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    if let Err(error) = run() {
        eprintln!("validate-training-workspace: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let workspace = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: validate_training_workspace WORKSPACE".to_string())?;
    if args.next().is_some() {
        return Err("usage: validate_training_workspace WORKSPACE".into());
    }

    let started = Instant::now();
    let mut candidate_count = 0usize;
    let mut ungradable_count = 0usize;
    let mut visited_banks = 0usize;
    let mut bank_bytes = 0u64;
    let mut fact_rows = 0u64;
    let identity = validate_snapshot_workspace_streaming(
        &workspace,
        |candidates| {
            candidate_count = candidates.combined_candidates.len();
            ungradable_count = candidates.combined_ungradable.len();
            Ok(())
        },
        |bank| {
            visited_banks = visited_banks
                .checked_add(1)
                .ok_or_else(|| SnapshotWorkspaceError::visitor("bank count overflow"))?;
            bank_bytes = bank_bytes
                .checked_add(bank.bytes.len() as u64)
                .ok_or_else(|| SnapshotWorkspaceError::visitor("bank byte count overflow"))?;
            fact_rows = fact_rows
                .checked_add(bank.snapshot.facts.facts().len() as u64)
                .ok_or_else(|| SnapshotWorkspaceError::visitor("fact row count overflow"))?;
            Ok(())
        },
    )
    .map_err(|error| error.to_string())?;

    if visited_banks != identity.bank_count {
        return Err(format!(
            "validated identity reports {} banks but visitor saw {visited_banks}",
            identity.bank_count
        ));
    }
    println!(
        "validate-training-workspace: state={:?} banks={} bank_bytes={} fact_rows={} candidates={} ungradable={} elapsed_ms={} rom_sha256={} manifest_sha256={} candidate_identity_v3_sha256={}",
        identity.state,
        identity.bank_count,
        bank_bytes,
        fact_rows,
        candidate_count,
        ungradable_count,
        started.elapsed().as_millis(),
        identity.normalized_rom_sha256.to_hex(),
        identity.manifest_sha256.to_hex(),
        identity.scoped_candidate_identities_v3_sha256.to_hex(),
    );
    Ok(())
}
