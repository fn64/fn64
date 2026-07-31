//! Production bridge from external-tool JSONL into a snapshot-bound claim sidecar.
//!
//! Unlike `gate_tool_jsonl`, this command derives bank identity and discovery-
//! snapshot lineage from the supplied `ProgramSnapshotV1`. The request may
//! name additional lineage, but it cannot supply or override snapshot lineage.

use fn64_discover::snapshot::ProgramSnapshotV1;
use fn64_discover::tool_adapter::{
    ingest_tool_jsonl, AdapterLimits, Sha256Digest, ToolAdapterExpectation, ToolIdentity,
    ToolLineageRef, ToolLineageRole, ToolRunRole,
};
use fn64_discover::tool_claims::{
    bank_input_identity_v1, discovery_snapshot_lineage_v2, freeze_tool_claims_v1,
    validate_tool_claim_set_v1,
};
use fn64_discover::workspace_artifacts::{publish_new, validate_output_path, validate_workspace};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const REQUEST_SCHEMA: &str = "fn64.tool-ingest-request";
const REQUEST_SCHEMA_VERSION: u32 = 1;
const MIB: u64 = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 128 * MIB;
const MAX_REQUEST_BYTES: u64 = 4 * MIB;
const MAX_MANIFEST_BYTES: u64 = 4 * MIB;
const MAX_RUNS: usize = 256;
const MAX_LINEAGE_ARTIFACTS_PER_RUN: usize = 64;
const MAX_TOOL_ARTIFACTS: usize = 4096;
const MAX_TOOL_ARTIFACT_BYTES: u64 = 2 * 1024 * MIB;
const MAX_AGGREGATE_PROVIDER_BYTES: u64 = 256 * MIB;
const MAX_AGGREGATE_CANDIDATES: usize = 250_000;
const MAX_SIDECAR_BYTES: usize = 128 * MIB as usize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestRequest {
    schema: String,
    schema_version: u32,
    runs: Vec<RunRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRequest {
    bank: String,
    jsonl: PathBuf,
    tool: ToolIdentity,
    tool_artifact_manifest: PathBuf,
    role: ToolRunRole,
    lineage_artifacts: Vec<LineageArtifactRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageArtifactRequest {
    role: ToolLineageRole,
    path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArtifactManifest {
    schema: String,
    schema_version: u32,
    tool_name: String,
    tool_version: String,
    artifacts: Vec<ToolArtifactEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArtifactEntry {
    path: PathBuf,
    byte_length: u64,
    sha256: Sha256Digest,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("ingest-tool-claims: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let snapshot_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let request_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let workspace_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let output_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let workspace = validate_workspace(&workspace_path)?;
    validate_output_path(&workspace, &output_path)?;
    let snapshot: ProgramSnapshotV1 =
        read_json(&snapshot_path, "program snapshot", MAX_SNAPSHOT_BYTES)?;
    let request: IngestRequest =
        read_json(&request_path, "tool-ingest request", MAX_REQUEST_BYTES)?;
    validate_request(&request)?;
    let request_dir = request_path.parent().unwrap_or_else(|| Path::new("."));
    let snapshot_lineage = discovery_snapshot_lineage_v2(&snapshot)
        .map_err(|error| format!("deriving discovery-snapshot lineage: {error}"))?;

    let mut outputs = Vec::with_capacity(request.runs.len());
    let mut aggregate_provider_bytes = 0u64;
    let mut aggregate_candidates = 0usize;
    for requested in request.runs {
        if requested
            .lineage_artifacts
            .iter()
            .any(|item| item.role == ToolLineageRole::DiscoverySnapshot)
        {
            return Err(format!(
                "run for bank {:?} must not supply discovery_snapshot lineage; it is derived from the snapshot",
                requested.bank
            ));
        }
        let tool_manifest_path = resolve(&request_dir, &requested.tool_artifact_manifest);
        let tool_manifest = validate_tool_artifact_manifest(&tool_manifest_path, &requested.tool)?;
        let measured_tool_digest = format!("{:x}", Sha256::digest(&tool_manifest));
        if measured_tool_digest != requested.tool.build_sha256.to_hex() {
            return Err(format!(
                "tool artifact manifest digest mismatch for bank {:?}",
                requested.bank
            ));
        }

        let mut lineage = Vec::with_capacity(requested.lineage_artifacts.len() + 1);
        for artifact in requested.lineage_artifacts {
            let artifact_path = resolve(&request_dir, &artifact.path);
            let bytes = read_bounded(&artifact_path, "lineage artifact", MAX_MANIFEST_BYTES)?;
            lineage.push(ToolLineageRef {
                role: artifact.role,
                source_sha256: Sha256Digest::from_hex(&format!("{:x}", Sha256::digest(&bytes)))
                    .map_err(|error| format!("hashing lineage artifact: {error}"))?,
            });
        }
        lineage.push(snapshot_lineage.clone());
        lineage.sort();
        lineage.dedup();
        let input = bank_input_identity_v1(&snapshot, &requested.bank)
            .map_err(|error| format!("deriving bank {:?} identity: {error}", requested.bank))?;
        let jsonl_path = resolve(&request_dir, &requested.jsonl);
        let limits = AdapterLimits::default();
        let jsonl_bytes =
            read_bounded(&jsonl_path, "provider JSONL", limits.max_total_bytes as u64)?;
        aggregate_provider_bytes = aggregate_provider_bytes
            .checked_add(jsonl_bytes.len() as u64)
            .ok_or_else(|| "aggregate provider byte count overflowed".to_string())?;
        if aggregate_provider_bytes > MAX_AGGREGATE_PROVIDER_BYTES {
            return Err(format!(
                "aggregate provider JSONL exceeds {} bytes",
                MAX_AGGREGATE_PROVIDER_BYTES
            ));
        }
        let jsonl = String::from_utf8(jsonl_bytes)
            .map_err(|_| format!("provider JSONL {} is not UTF-8", jsonl_path.display()))?;
        let output = ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role: requested.role,
                lineage,
                limits,
            },
        )
        .map_err(|error| format!("ingesting provider JSONL {}: {error}", jsonl_path.display()))?;
        if output.source().tool != requested.tool {
            return Err(format!(
                "provider identity mismatch for bank {:?}: expected {:?}, got {:?}",
                requested.bank,
                requested.tool,
                output.source().tool
            ));
        }
        aggregate_candidates = aggregate_candidates
            .checked_add(output.candidates().len())
            .ok_or_else(|| "aggregate candidate count overflowed".to_string())?;
        if aggregate_candidates > MAX_AGGREGATE_CANDIDATES {
            return Err(format!(
                "aggregate candidate count exceeds {}",
                MAX_AGGREGATE_CANDIDATES
            ));
        }
        outputs.push(output);
    }

    let sidecar = freeze_tool_claims_v1(&snapshot, &outputs)
        .map_err(|error| format!("freezing snapshot-bound tool claims: {error}"))?;
    validate_tool_claim_set_v1(&snapshot, &sidecar)
        .map_err(|error| format!("validating frozen tool claims: {error}"))?;
    let mut encoded = serde_json::to_vec_pretty(&sidecar)
        .map_err(|error| format!("serializing tool-claim sidecar: {error}"))?;
    encoded.push(b'\n');
    if encoded.len() > MAX_SIDECAR_BYTES {
        return Err(format!(
            "serialized tool-claim sidecar exceeds {} bytes",
            MAX_SIDECAR_BYTES
        ));
    }
    publish_new(&output_path, &encoded)?;

    let output_sha256 = format!("{:x}", Sha256::digest(&encoded));
    println!(
        "ingest-tool-claims: snapshot={} sources={} claims={} output_sha256={}",
        sidecar.program_snapshot_sha256.to_hex(),
        sidecar.sources.len(),
        sidecar.claims.len(),
        output_sha256
    );
    Ok(())
}

fn validate_request(request: &IngestRequest) -> Result<(), String> {
    if request.schema != REQUEST_SCHEMA || request.schema_version != REQUEST_SCHEMA_VERSION {
        return Err(format!(
            "unsupported request schema {:?} version {}",
            request.schema, request.schema_version
        ));
    }
    if request.runs.is_empty() {
        return Err("tool-ingest request contains no runs".into());
    }
    if request.runs.len() > MAX_RUNS {
        return Err(format!("tool-ingest request exceeds {MAX_RUNS} runs"));
    }
    for run in &request.runs {
        if run.lineage_artifacts.len() > MAX_LINEAGE_ARTIFACTS_PER_RUN {
            return Err(format!(
                "run for bank {:?} exceeds {} lineage artifacts",
                run.bank, MAX_LINEAGE_ARTIFACTS_PER_RUN
            ));
        }
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    label: &str,
    limit: u64,
) -> Result<T, String> {
    let bytes = read_bounded(path, label, limit)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("parsing {label} {}: {error}", path.display()))
}

fn read_bounded(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!(
            "{label} {} exceeds {} bytes",
            path.display(),
            limit
        ));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("reading {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{label} {} grew beyond {} bytes while reading",
            path.display(),
            limit
        ));
    }
    Ok(bytes)
}

fn validate_tool_artifact_manifest(
    path: &Path,
    expected_tool: &ToolIdentity,
) -> Result<Vec<u8>, String> {
    let bytes = read_bounded(path, "tool artifact manifest", MAX_MANIFEST_BYTES)?;
    let manifest: ToolArtifactManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parsing tool artifact manifest {}: {error}", path.display()))?;
    if manifest.schema != "fn64.tool-artifact-manifest" || manifest.schema_version != 1 {
        return Err("unsupported tool artifact manifest schema".into());
    }
    if manifest.tool_name != expected_tool.name || manifest.tool_version != expected_tool.version {
        return Err("tool artifact manifest identity does not match request".into());
    }
    if manifest.artifacts.is_empty() || manifest.artifacts.len() > MAX_TOOL_ARTIFACTS {
        return Err(format!(
            "tool artifact manifest must contain 1..={MAX_TOOL_ARTIFACTS} artifacts"
        ));
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut previous: Option<&Path> = None;
    let mut aggregate = 0u64;
    for artifact in &manifest.artifacts {
        if artifact.path.as_os_str().is_empty()
            || artifact.path.is_absolute()
            || artifact
                .path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "tool artifact path must be a nonempty portable relative path: {}",
                artifact.path.display()
            ));
        }
        if previous.is_some_and(|prior| prior >= artifact.path.as_path()) {
            return Err("tool artifact paths must be strictly sorted and unique".into());
        }
        previous = Some(&artifact.path);
        if artifact.byte_length > MAX_TOOL_ARTIFACT_BYTES {
            return Err(format!(
                "tool artifact {} exceeds {} bytes",
                artifact.path.display(),
                MAX_TOOL_ARTIFACT_BYTES
            ));
        }
        aggregate = aggregate
            .checked_add(artifact.byte_length)
            .ok_or_else(|| "tool artifact aggregate byte count overflowed".to_string())?;
        if aggregate > MAX_TOOL_ARTIFACT_BYTES {
            return Err(format!(
                "tool artifact manifest exceeds {} aggregate bytes",
                MAX_TOOL_ARTIFACT_BYTES
            ));
        }
        validate_tool_artifact(&base.join(&artifact.path), artifact)?;
    }
    Ok(bytes)
}

fn validate_tool_artifact(path: &Path, expected: &ToolArtifactEntry) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting tool artifact {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() != expected.byte_length {
        return Err(format!(
            "tool artifact length/type mismatch: {}",
            path.display()
        ));
    }
    let mut file = fs::File::open(path)
        .map_err(|error| format!("opening tool artifact {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut measured = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("reading tool artifact {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        measured = measured
            .checked_add(count as u64)
            .ok_or_else(|| "tool artifact byte count overflowed".to_string())?;
        if measured > expected.byte_length {
            return Err(format!(
                "tool artifact grew while reading: {}",
                path.display()
            ));
        }
        hasher.update(&buffer[..count]);
    }
    let digest = Sha256Digest(hasher.finalize().into());
    if measured != expected.byte_length || digest != expected.sha256 {
        return Err(format!("tool artifact digest mismatch: {}", path.display()));
    }
    Ok(())
}

fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn usage() -> String {
    "usage: ingest_tool_claims PROGRAM_SNAPSHOT.json REQUEST.json WORKSPACE OUT.tool-claims.json"
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fn64-ingest-tool-claims-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    #[test]
    fn bounded_read_rejects_sparse_file_before_allocation() {
        let directory = temporary_directory("bounded-read");
        let path = directory.join("oversized.jsonl");
        let file = File::create(&path).unwrap();
        file.set_len(65 * MIB).unwrap();
        drop(file);

        let error = read_bounded(&path, "provider JSONL", 64 * MIB).unwrap_err();
        assert!(error.contains("exceeds 67108864 bytes"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tool_manifest_binds_identity_and_actual_artifact_bytes() {
        let directory = temporary_directory("tool-manifest");
        let artifact = directory.join("tool.bin");
        fs::write(&artifact, b"exact tool bytes").unwrap();
        let artifact_sha = format!("{:x}", Sha256::digest(b"exact tool bytes"));
        let manifest = format!(
            "{{\"schema\":\"fn64.tool-artifact-manifest\",\"schema_version\":1,\
             \"tool_name\":\"test-tool\",\"tool_version\":\"1\",\"artifacts\":[{{\
             \"path\":\"tool.bin\",\"byte_length\":16,\"sha256\":\"{artifact_sha}\"}}]}}"
        );
        let manifest_path = directory.join("tool-manifest.json");
        fs::write(&manifest_path, &manifest).unwrap();
        let tool = ToolIdentity {
            name: "test-tool".into(),
            version: "1".into(),
            build_sha256: Sha256Digest(Sha256::digest(manifest.as_bytes()).into()),
        };
        validate_tool_artifact_manifest(&manifest_path, &tool).unwrap();

        fs::write(&artifact, b"mutated tool byt").unwrap();
        let error = validate_tool_artifact_manifest(&manifest_path, &tool).unwrap_err();
        assert!(error.contains("digest mismatch"), "{error}");
        fs::remove_dir_all(directory).unwrap();
    }
}
