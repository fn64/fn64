use fn64_discover::closure::UnsupportedDestinationAuditV1;
use fn64_discover::cold_sweep::{
    measure_cold_rom, ColdClosureMeasurementV2, COLD_ROM_MAX_INPUT_BYTES,
};
use fn64_discover::DiscoveryStrategy;
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
struct ColdUnsupportedDiagnosticV1 {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: String,
    selected_strategy: DiscoveryStrategy,
    proven_bank_count: usize,
    closure: ColdClosureMeasurementV2,
    unsupported_destinations: Vec<UnsupportedDestinationAuditV1>,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("diagnose-cold-unsupported: {error}");
        std::process::exit(1);
    }
}

fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
    if paths.is_empty() {
        return Err("usage: diagnose_cold_unsupported ROM [ROM ...]".into());
    }
    let mut failures = 0u64;
    for path in paths {
        match diagnose(&path) {
            Ok(report) => println!(
                "{}",
                serde_json::to_string(&report).map_err(|error| error.to_string())?
            ),
            Err(error) => {
                failures += 1;
                eprintln!("diagnose-cold-unsupported: {}: {error}", path.display());
            }
        }
    }
    if failures != 0 {
        return Err(format!("{failures} ROM input(s) failed"));
    }
    Ok(())
}

fn diagnose(path: &Path) -> Result<ColdUnsupportedDiagnosticV1, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("reading ROM metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("ROM input is not a regular file".into());
    }
    if metadata.len() > COLD_ROM_MAX_INPUT_BYTES as u64 {
        return Err(format!(
            "ROM input is {} bytes, exceeding the {}-byte limit",
            metadata.len(),
            COLD_ROM_MAX_INPUT_BYTES
        ));
    }
    let rom_bytes = fs::read(path).map_err(|error| format!("reading ROM: {error}"))?;
    let run = measure_cold_rom(&rom_bytes).map_err(|error| error.to_string())?;
    Ok(ColdUnsupportedDiagnosticV1 {
        schema: "fn64.cold-unsupported-diagnostic.v1",
        schema_version: 1,
        normalized_rom_sha256: run.receipt.measurement.normalized_rom_sha256,
        selected_strategy: run.receipt.measurement.selected_strategy,
        proven_bank_count: run.receipt.measurement.proven_bank_count,
        closure: run.receipt.measurement.closure,
        unsupported_destinations: run.unsupported_destinations,
    })
}
