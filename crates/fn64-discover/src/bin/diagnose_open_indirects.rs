use fn64_discover::indirect_frontier::{classify_open_indirects_v1, OpenIndirectFrontierV1};
use fn64_discover::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MultiBankCompositionLimits,
};
use fn64_discover::snapshot_inputs::{
    prepare_snapshot_banks_with_limits, PrepareSnapshotBanksLimits,
};
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Serialize)]
struct OpenIndirectDiagnosticV1 {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: String,
    internal_name: String,
    banks: u64,
    elapsed_ms: u128,
    frontier: OpenIndirectFrontierV1,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("diagnose-open-indirects: {error}");
        std::process::exit(1);
    }
}

fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if paths.is_empty() {
        return Err("usage: diagnose_open_indirects ROM [ROM ...]".into());
    }
    for path in paths {
        let report = diagnose(&path)?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
    }
    Ok(())
}

fn diagnose(path: &Path) -> Result<OpenIndirectDiagnosticV1, String> {
    let started = Instant::now();
    let metadata = fs::metadata(path).map_err(|error| format!("reading ROM metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("ROM input is not a regular file".into());
    }
    if metadata.len() > MAX_ROM_BYTES {
        return Err(format!(
            "ROM input is {} bytes, exceeding the {MAX_ROM_BYTES}-byte limit",
            metadata.len()
        ));
    }
    let rom_bytes = fs::read(path).map_err(|error| format!("reading ROM: {error}"))?;
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .map_err(|error| format!("automatic discovery rejected the ROM: {error:?}"))?;
    let prepared = prepare_snapshot_banks_with_limits(
        &discovery.rom,
        &discovery.facts,
        PrepareSnapshotBanksLimits::default(),
    )
    .map_err(|error| format!("preparing snapshot banks: {error}"))?;
    let inputs = prepared.materialized_inputs();
    let composed = compose_materialized_banks_validated_v2_with_limits(
        &discovery.rom,
        &discovery.facts,
        &inputs,
        MultiBankCompositionLimits::default(),
    )
    .map_err(|error| format!("composing snapshot banks: {error}"))?;

    let mut frontier = OpenIndirectFrontierV1 {
        open_sites: 0,
        shapes: vec![],
    };
    for snapshot in composed.snapshots() {
        let [bank_snapshot] = snapshot.banks.as_slice() else {
            return Err("a composed snapshot did not contain exactly one bank".into());
        };
        let matching: Vec<_> = prepared
            .banks()
            .iter()
            .filter(|bank| {
                bank.bank == bank_snapshot.input.bank
                    && bank.va_start == bank_snapshot.input.va_start
                    && bank.va_end == bank_snapshot.input.va_end
            })
            .collect();
        let [bank] = matching.as_slice() else {
            return Err("a composed snapshot did not match exactly one prepared bank".into());
        };
        let bank_frontier =
            classify_open_indirects_v1(&bank_snapshot.closure, &bank.bytes, bank.va_start)
                .map_err(|error| format!("classifying an open-indirect frontier: {error}"))?;
        frontier.merge(&bank_frontier);
    }

    Ok(OpenIndirectDiagnosticV1 {
        schema: "fn64.open-indirect-frontier.v1",
        schema_version: 1,
        normalized_rom_sha256: discovery.rom.sha256,
        internal_name: discovery.rom.header.name,
        banks: composed.snapshots().len() as u64,
        elapsed_ms: started.elapsed().as_millis(),
        frontier,
    })
}
