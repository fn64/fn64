use fn64_discover::evidence::EvidenceManifest;
use fn64_discover::{run_discovery, run_discovery_with_manifest, FactDb, NormalizedRom};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::BufReader;
use std::path::PathBuf;

#[derive(Serialize)]
struct DiscoveryArtifact<'a> {
    schema_version: u32,
    rom: &'a NormalizedRom,
    facts: &'a FactDb,
    coverage: fn64_discover::coverage::CoverageReport,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    traces: Vec<fn64_discover::trace::IngestReport>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("fn64-discover: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let rom_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let mut evidence_path = None;
    let mut output_path = None;
    let mut trace_paths = Vec::new();
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--evidence") => {
                evidence_path =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        "--evidence requires a TOML path".to_string()
                    })?));
            }
            Some("--out") => {
                output_path = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--out requires a JSON path".to_string())?,
                ));
            }
            Some("--trace") => {
                trace_paths.push(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--trace requires a JSONL path".to_string())?,
                ));
            }
            Some(other) => return Err(format!("unknown argument {other:?}\n{}", usage())),
            None => return Err("arguments must be valid UTF-8".to_string()),
        }
    }

    let rom_bytes = std::fs::read(&rom_path)
        .map_err(|error| format!("reading ROM {}: {error}", rom_path.display()))?;
    let (rom, facts) = if let Some(path) = evidence_path {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading evidence {}: {error}", path.display()))?;
        let manifest = EvidenceManifest::from_toml(&text).map_err(|error| error.to_string())?;
        run_discovery_with_manifest(&rom_bytes, &manifest).map_err(|error| error.to_string())?
    } else {
        run_discovery(&rom_bytes, None).map_err(|error| error.to_string())?
    };
    trace_paths.sort();
    trace_paths.dedup();
    let expected_digest = fn64_discover::trace::NormalizedRomDigest::try_from(rom.sha256.clone())
        .map_err(str::to_string)?;
    let mut traces = Vec::new();
    let mut trace_ids = BTreeSet::new();
    for path in trace_paths {
        let file = std::fs::File::open(&path)
            .map_err(|error| format!("opening trace {}: {error}", path.display()))?;
        let report = fn64_discover::trace::ingest_jsonl(BufReader::new(file), &expected_digest)
            .map_err(|error| format!("ingesting trace {}: {error}", path.display()))?;
        if !trace_ids.insert(report.header.trace_id.clone()) {
            return Err(format!(
                "duplicate trace_id {:?} in trace inputs",
                report.header.trace_id
            ));
        }
        traces.push(report);
    }
    let artifact = DiscoveryArtifact {
        schema_version: 1,
        rom: &rom,
        facts: &facts,
        coverage: fn64_discover::coverage::report(rom.len(), &facts),
        traces,
    };
    let json = serde_json::to_string_pretty(&artifact)
        .map_err(|error| format!("serializing discovery artifact: {error}"))?;
    if let Some(path) = output_path {
        std::fs::write(&path, format!("{json}\n"))
            .map_err(|error| format!("writing {}: {error}", path.display()))?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn usage() -> String {
    "usage: fn64-discover <rom> [--evidence manifest.toml] [--trace events.jsonl]... [--out facts.json]".to_string()
}
