use fn64_boot_harness::{
    load_private_release_run_contract, run_private_release_series, verify_private_release_series,
    PrivateReleaseRunContract, PrivateReleaseSeriesReceipt,
};
use std::{env, ffi::OsStr, fs, path::PathBuf, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("run-private-release-series: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let mode = arguments.next().ok_or_else(usage)?;
    if mode == OsStr::new("--print-contract-sha256") {
        let path = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        if arguments.next().is_some() {
            return Err(usage());
        }
        let bytes = fs::read(&path)
            .map_err(|source| format!("read contract {}: {source}", path.display()))?;
        let contract: PrivateReleaseRunContract = serde_json::from_slice(&bytes)
            .map_err(|source| format!("parse contract {}: {source}", path.display()))?;
        println!(
            "{}",
            contract
                .recompute_contract_sha256()
                .map_err(|source| source.to_string())?
        );
        return Ok(());
    }
    if mode == OsStr::new("run") {
        let contract_path = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        let output_directory = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        if arguments.next().is_some() {
            return Err(usage());
        }
        let contract = load_private_release_run_contract(&contract_path)
            .map_err(|source| source.to_string())?;
        let receipt = run_private_release_series(&contract, &output_directory)
            .map_err(|source| source.to_string())?;
        println!(
            "verified {} fresh child processes: cycle={} scenario={} report_sha256={} receipt_sha256={} receipt={}",
            receipt.count,
            receipt.guest_cycle,
            receipt.report_scenario,
            receipt.semantic_report_sha256,
            receipt.receipt_sha256,
            output_directory.join("receipt.json").display()
        );
        return Ok(());
    }
    if mode == OsStr::new("verify") {
        let contract_path = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        let output_directory = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        let receipt_path = arguments.next().map(PathBuf::from).ok_or_else(usage)?;
        if arguments.next().is_some() {
            return Err(usage());
        }
        let contract = load_private_release_run_contract(&contract_path)
            .map_err(|source| source.to_string())?;
        let bytes = fs::read(&receipt_path)
            .map_err(|source| format!("read receipt {}: {source}", receipt_path.display()))?;
        let receipt: PrivateReleaseSeriesReceipt = serde_json::from_slice(&bytes)
            .map_err(|source| format!("parse receipt {}: {source}", receipt_path.display()))?;
        verify_private_release_series(&contract, &output_directory, &receipt)
            .map_err(|source| source.to_string())?;
        println!(
            "reverified {} fresh-process pairs: cycle={} scenario={} report_sha256={} receipt_sha256={}",
            receipt.count,
            receipt.guest_cycle,
            receipt.report_scenario,
            receipt.semantic_report_sha256,
            receipt.receipt_sha256
        );
        return Ok(());
    }
    Err(usage())
}

fn usage() -> String {
    "usage: run-private-release-series --print-contract-sha256 CONTRACT.json\n       run-private-release-series run CONTRACT.json NEW_OUTPUT_DIRECTORY\n       run-private-release-series verify CONTRACT.json OUTPUT_DIRECTORY RECEIPT.json"
        .to_owned()
}
