//! Verify and stage one caller-materialized snapshot bank for an external tool.

use fn64_discover::owner_proof::OwnerAssessment;
use fn64_discover::snapshot::{ProgramSnapshotV1, PROGRAM_SNAPSHOT_SCHEMA_V5};
use fn64_discover::tool_adapter::{BankInputIdentity, Sha256Digest};
use fn64_discover::tool_claims::{bank_input_identity_v1, program_snapshot_sha256_v2};
use fn64_discover::workspace_artifacts::{publish_new, validate_output_path, validate_workspace};
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 128 * MIB;
const MAX_BANK_BYTES: u64 = 128 * MIB;

#[derive(Serialize)]
struct EvidenceManifest<'a> {
    schema: &'static str,
    schema_version: u32,
    program_snapshot_sha256: Sha256Digest,
    input: &'a BankInputIdentity,
    backing: Backing,
    artifact: Artifact,
    seeds: Seeds,
}

#[derive(Serialize)]
struct Backing {
    rom_space: fn64_discover::RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
}

#[derive(Serialize)]
struct Artifact {
    byte_length: u64,
    sha256: Sha256Digest,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum Seeds {
    DiscoveryOnly { role: &'static str },
    BaseOnly { base_seed: u32 },
    Paired { base_seed: u32, snapshot_seed: u32 },
}

#[derive(Clone, Copy)]
enum InvocationMode {
    DiscoveryOnly,
    BaseOnly,
    Paired,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("stage-snapshot-bank: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let first = args.next().ok_or_else(usage)?;
    let (invocation_mode, snapshot_path) = if first == "--discovery-only" {
        (
            InvocationMode::DiscoveryOnly,
            args.next().map(PathBuf::from).ok_or_else(usage)?,
        )
    } else if first == "--base-only" {
        (
            InvocationMode::BaseOnly,
            args.next().map(PathBuf::from).ok_or_else(usage)?,
        )
    } else {
        let snapshot_path = PathBuf::from(first);
        if snapshot_path
            .as_os_str()
            .to_string_lossy()
            .starts_with("--")
        {
            return Err(usage());
        }
        (InvocationMode::Paired, snapshot_path)
    };
    let bank = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    let source_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let workspace_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let output_bank = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let output_evidence = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let base_seed = match invocation_mode {
        InvocationMode::DiscoveryOnly => None,
        InvocationMode::BaseOnly | InvocationMode::Paired => {
            Some(parse_u32(args.next().ok_or_else(usage)?, "BASE_SEED")?)
        }
    };
    let snapshot_seed = match invocation_mode {
        InvocationMode::DiscoveryOnly | InvocationMode::BaseOnly => None,
        InvocationMode::Paired => Some(parse_u32(args.next().ok_or_else(usage)?, "SNAPSHOT_SEED")?),
    };
    if args.next().is_some() {
        return Err(usage());
    }

    let workspace = validate_workspace(&workspace_path)?;
    validate_output_path(&workspace, &output_bank)?;
    validate_output_path(&workspace, &output_evidence)?;
    if output_bank == output_evidence {
        return Err("bank and evidence outputs must be different paths".into());
    }
    require_new_output(&output_bank)?;
    require_new_output(&output_evidence)?;

    let snapshot_bytes =
        read_bounded_regular(&snapshot_path, "program snapshot", MAX_SNAPSHOT_BYTES, None)?;
    let snapshot: ProgramSnapshotV1 = serde_json::from_slice(&snapshot_bytes).map_err(|error| {
        format!(
            "parsing program snapshot {}: {error}",
            snapshot_path.display()
        )
    })?;
    if snapshot.schema_version != PROGRAM_SNAPSHOT_SCHEMA_V5 {
        return Err(format!(
            "unsupported program snapshot schema {}",
            snapshot.schema_version
        ));
    }
    let matching: Vec<_> = snapshot
        .banks
        .iter()
        .filter(|candidate| candidate.input.bank == bank)
        .collect();
    let bank_snapshot = match matching.as_slice() {
        [] => return Err(format!("snapshot has no bank {bank:?}")),
        [bank_snapshot] => *bank_snapshot,
        _ => return Err(format!("snapshot has duplicate bank {bank:?}")),
    };
    let input = bank_input_identity_v1(&snapshot, &bank)
        .map_err(|error| format!("deriving bank identity: {error}"))?;
    let expected_len = u64::from(
        input
            .va_end
            .checked_sub(input.va_start)
            .ok_or_else(|| "snapshot bank has inverted VA geometry".to_string())?,
    );
    if expected_len > MAX_BANK_BYTES {
        return Err(format!("snapshot bank exceeds {MAX_BANK_BYTES} bytes"));
    }
    let bank_bytes = read_bounded_regular(
        &source_path,
        "materialized bank",
        MAX_BANK_BYTES,
        Some(expected_len),
    )?;
    let measured_bank_sha = Sha256Digest::of(&bank_bytes);
    if measured_bank_sha != input.bank_bytes_sha256 {
        return Err(format!(
            "materialized bank digest does not match snapshot bank {bank:?}"
        ));
    }

    if let Some(base_seed) = base_seed {
        validate_seed(base_seed, &input, "base seed")?;
        let base_is_proven = bank_snapshot
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == base_seed)
            });
        if !base_is_proven {
            return Err("base seed is not a proven owner entry in the snapshot".into());
        }
    }
    if let Some(snapshot_seed) = snapshot_seed {
        validate_seed(snapshot_seed, &input, "snapshot seed")?;
        if base_seed == Some(snapshot_seed) {
            return Err("base seed and snapshot seed must be distinct".into());
        }
        if !bank_snapshot
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| assessment.entry().pc == snapshot_seed)
        {
            return Err("snapshot seed is not an assessed owner entry in the snapshot".into());
        }
    }

    let snapshot_sha = program_snapshot_sha256_v2(&snapshot)
        .map_err(|error| format!("deriving program-snapshot digest: {error}"))?;
    let manifest = EvidenceManifest {
        schema: "fn64.snapshot-bank-evidence",
        schema_version: match invocation_mode {
            InvocationMode::DiscoveryOnly => 3,
            InvocationMode::BaseOnly | InvocationMode::Paired => 2,
        },
        program_snapshot_sha256: snapshot_sha,
        input: &input,
        backing: Backing {
            rom_space: bank_snapshot.input.rom_space,
            rom_start: bank_snapshot.input.rom_start,
            rom_end: bank_snapshot.input.rom_end,
        },
        artifact: Artifact {
            byte_length: bank_bytes.len() as u64,
            sha256: measured_bank_sha,
        },
        seeds: match (base_seed, snapshot_seed) {
            (None, None) => Seeds::DiscoveryOnly {
                role: "candidate_only",
            },
            (Some(base_seed), Some(snapshot_seed)) => Seeds::Paired {
                base_seed,
                snapshot_seed,
            },
            (Some(base_seed), None) => Seeds::BaseOnly { base_seed },
            (None, Some(_)) => return Err("snapshot seed requires a base seed".into()),
        },
    };
    let mut evidence = serde_json::to_vec(&manifest)
        .map_err(|error| format!("serializing bank evidence: {error}"))?;
    evidence.push(b'\n');

    // Both destinations were checked before either publication. The runner
    // supplies a fresh attempt directory; create-new links still arbitrate a
    // concurrent writer without replacing its bytes.
    publish_new(&output_bank, &bank_bytes)?;
    publish_new(&output_evidence, &evidence)?;
    println!(
        "stage-snapshot-bank: snapshot={} bank_sha256={} bytes={} va_start={} va_end={}",
        snapshot_sha.to_hex(),
        measured_bank_sha.to_hex(),
        bank_bytes.len(),
        input.va_start,
        input.va_end
    );
    Ok(())
}

fn validate_seed(seed: u32, input: &BankInputIdentity, label: &str) -> Result<(), String> {
    if !seed.is_multiple_of(4) || seed < input.va_start || seed >= input.va_end {
        return Err(format!(
            "{label} 0x{seed:08x} is unaligned or outside [0x{:08x},0x{:08x})",
            input.va_start, input.va_end
        ));
    }
    Ok(())
}

fn parse_u32(value: OsString, label: &str) -> Result<u32, String> {
    let value = value
        .into_string()
        .map_err(|_| format!("{label} must be valid UTF-8"))?;
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        value.parse::<u32>()
    };
    parsed.map_err(|_| format!("{label} must be a decimal u32 or 0x-prefixed hexadecimal u32"))
}

fn read_bounded_regular(
    path: &Path,
    label: &str,
    limit: u64,
    exact_length: Option<u64>,
) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    if let Some(expected) = exact_length {
        if metadata.len() != expected {
            return Err(format!(
                "{label} length {} does not match snapshot length {expected}",
                metadata.len()
            ));
        }
    }
    if metadata.len() > limit {
        return Err(format!("{label} {} exceeds {limit} bytes", path.display()));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("reading {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{label} {} grew beyond {limit} bytes",
            path.display()
        ));
    }
    if let Some(expected) = exact_length {
        if bytes.len() as u64 != expected {
            return Err(format!("{label} changed length while reading"));
        }
    }
    Ok(bytes)
}

fn require_new_output(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!("refusing to overwrite {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("inspecting output {}: {error}", path.display())),
    }
}

fn usage() -> String {
    "usage: stage_snapshot_bank PROGRAM_SNAPSHOT.json BANK MATERIALIZED_BANK.bin WORKSPACE OUT_BANK.bin OUT_EVIDENCE.json BASE_SEED SNAPSHOT_SEED\n       stage_snapshot_bank --base-only PROGRAM_SNAPSHOT.json BANK MATERIALIZED_BANK.bin WORKSPACE OUT_BANK.bin OUT_EVIDENCE.json BASE_SEED\n       stage_snapshot_bank --discovery-only PROGRAM_SNAPSHOT.json BANK MATERIALIZED_BANK.bin WORKSPACE OUT_BANK.bin OUT_EVIDENCE.json".into()
}
