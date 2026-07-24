use std::fs;
use std::path::PathBuf;

use fn64_audio::characterize::{
    canonical_report_json, characterize_request, CharacterizationRequest,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("audio ABI characterization failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    let request_path = PathBuf::from(
        args.next()
            .ok_or_else(|| format!("usage: {} REQUEST.json", PathBuf::from(program).display()))?,
    );
    if args.next().is_some() {
        return Err("expected exactly one request path".into());
    }
    let request_bytes = fs::read(&request_path)
        .map_err(|error| format!("read characterization request: {error}"))?;
    let request: CharacterizationRequest = serde_json::from_slice(&request_bytes)
        .map_err(|error| format!("parse characterization request: {error}"))?;
    let report = characterize_request(request)?;
    println!("{}", canonical_report_json(&report)?);
    Ok(())
}
