//! Candidate-only differential between Ghidra computed flows and fn64 closure.
//!
//! This command never mutates native CFG/facts. It derives the provider's
//! bank identity and discovery-snapshot lineage from the supplied snapshot,
//! runs the strict schema-v3 adapter, freezes/revalidates the sidecar, then
//! reports agreement and disagreement against the snapshot's native indirect
//! frontier. Ghidra targets at native-open sites remain candidates.

use fn64_discover::resolve::IndirectProofState;
use fn64_discover::snapshot::ProgramSnapshotV1;
use fn64_discover::tool_adapter::{
    ingest_tool_jsonl, AdapterLimits, BankInputIdentity, ComputedFlowCompleteness, Sha256Digest,
    ToolAdapterExpectation, ToolCandidateKind, ToolIdentity, ToolLineageRef, ToolRunRole,
    TOOL_ADAPTER_SCHEMA, TOOL_ADAPTER_SCHEMA_VERSION_V3,
};
use fn64_discover::tool_claims::{
    bank_input_identity_v1, freeze_tool_claims_v1, validate_tool_claim_set_v1,
};
use fn64_discover::workspace_artifacts::{publish_new, validate_output_path, validate_workspace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 128 * MIB;
const MAX_BANK_BYTES: u64 = 128 * MIB;
const MAX_PROVIDER_BYTES: u64 = 64 * MIB;
const MAX_OUTPUT_BYTES: usize = 64 * MIB as usize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    record: String,
    schema: String,
    schema_version: u32,
    tool: ToolIdentity,
    role: ToolRunRole,
    input: BankInputIdentity,
    lineage: Vec<ToolLineageRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodedComputedSite {
    via_call: bool,
    ordinary_return: bool,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateFlowV1 {
    site: u32,
    ghidra_via_call: bool,
    isa_via_call: bool,
    targets: Vec<u32>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeComparisonV1 {
    site: u32,
    native_state: &'static str,
    native_via_call: bool,
    isa_via_call: bool,
    native_targets: Vec<u32>,
    ghidra_via_call: Option<bool>,
    ghidra_targets: Option<Vec<u32>>,
    missing_from_ghidra: Vec<u32>,
    extra_from_ghidra: Vec<u32>,
    exact_exhaustive_match: bool,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SummaryV1 {
    native_sites: usize,
    native_exhaustive_sites: usize,
    native_exhaustive_exact_matches: usize,
    native_exhaustive_disagreements: usize,
    native_open_or_bounded_sites: usize,
    native_open_or_bounded_with_ghidra_targets: usize,
    native_sites_missing_from_ghidra: usize,
    ghidra_sites: usize,
    ghidra_only_sites: usize,
    ghidra_isa_call_classification_disagreements: usize,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportBodyV1 {
    candidate_only: bool,
    production_ingest_authority: bool,
    bank: String,
    normalized_rom_sha256: Sha256Digest,
    bank_bytes_sha256: Sha256Digest,
    mapping_sha256: Sha256Digest,
    program_snapshot_sha256: Sha256Digest,
    provider_jsonl_sha256: Sha256Digest,
    provider_source_sha256: Sha256Digest,
    provider_tool: ToolIdentity,
    provider_artifact_identity_independently_verified: bool,
    ghidra_candidates: Vec<CandidateFlowV1>,
    native_comparisons: Vec<NativeComparisonV1>,
    native_sites_missing_from_ghidra: Vec<u32>,
    ghidra_only_sites: Vec<u32>,
    ghidra_isa_call_classification_disagreements: Vec<u32>,
    summary: SummaryV1,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportV1 {
    schema: &'static str,
    schema_version: u32,
    semantic_sha256: Sha256Digest,
    body: ReportBodyV1,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("compare-computed-flows: {error}");
        std::process::exit(1);
    }
}

fn usage() -> String {
    "usage: compare_computed_flows SNAPSHOT BANK_BYTES BANK PROVIDER_JSONL WORKSPACE OUT".into()
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let snapshot_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let bank_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let bank_name = args
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(usage)?;
    let provider_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let workspace_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    let output_path = args.next().map(PathBuf::from).ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    let workspace = validate_workspace(&workspace_path)?;
    validate_output_path(&workspace, &output_path)?;

    let snapshot_bytes = read_bounded(&snapshot_path, "snapshot", MAX_SNAPSHOT_BYTES)?;
    let snapshot: ProgramSnapshotV1 = serde_json::from_slice(&snapshot_bytes)
        .map_err(|error| format!("parsing snapshot: {error}"))?;
    let bank_bytes = read_bounded(&bank_path, "bank bytes", MAX_BANK_BYTES)?;
    let provider_bytes = read_bounded(&provider_path, "provider JSONL", MAX_PROVIDER_BYTES)?;
    let provider = std::str::from_utf8(&provider_bytes)
        .map_err(|_| "provider JSONL is not UTF-8".to_string())?;
    let first = provider
        .lines()
        .next()
        .ok_or_else(|| "provider JSONL is empty".to_string())?;
    let header: Header =
        serde_json::from_str(first).map_err(|error| format!("parsing provider header: {error}"))?;
    if header.record != "header"
        || header.schema != TOOL_ADAPTER_SCHEMA
        || header.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
        || header.role != ToolRunRole::ControlFlowCandidates
    {
        return Err("provider is not a control-flow schema-v3 stream".into());
    }

    let expected_input = bank_input_identity_v1(&snapshot, &bank_name)
        .map_err(|error| format!("deriving snapshot bank identity: {error}"))?;
    if header.input != expected_input {
        return Err("provider bank identity does not match the snapshot".into());
    }
    let actual_bank_sha = Sha256Digest::of(&bank_bytes);
    if actual_bank_sha != expected_input.bank_bytes_sha256 {
        return Err("materialized bank digest does not match the snapshot".into());
    }
    if bank_bytes.len() as u64 != u64::from(expected_input.va_end - expected_input.va_start) {
        return Err("materialized bank length does not match the snapshot".into());
    }

    let output = ingest_tool_jsonl(
        provider,
        &ToolAdapterExpectation {
            input: expected_input.clone(),
            role: ToolRunRole::ControlFlowCandidates,
            lineage: header.lineage,
            limits: AdapterLimits::default(),
        },
    )
    .map_err(|error| format!("strict provider ingest failed: {error}"))?;
    if output.source().tool != header.tool {
        return Err("provider tool identity changed during ingest".into());
    }
    let frozen = freeze_tool_claims_v1(&snapshot, [&output])
        .map_err(|error| format!("freezing snapshot-bound candidates: {error}"))?;
    validate_tool_claim_set_v1(&snapshot, &frozen)
        .map_err(|error| format!("revalidating snapshot-bound candidates: {error}"))?;

    let bank = snapshot
        .banks
        .iter()
        .filter(|bank| bank.input.bank == bank_name)
        .collect::<Vec<_>>();
    let bank = match bank.as_slice() {
        [bank] => *bank,
        [] => return Err("snapshot does not contain the requested bank".into()),
        _ => return Err("snapshot contains the requested bank more than once".into()),
    };

    let mut candidates = BTreeMap::<u32, CandidateFlowV1>::new();
    for candidate in output.candidates() {
        let ToolCandidateKind::ComputedControlFlow {
            site,
            via_call,
            targets,
            completeness: ComputedFlowCompleteness::Unknown,
        } = &candidate.kind
        else {
            return Err("control-flow provider emitted a non-flow candidate".into());
        };
        let decoded = decode_computed_site(&bank_bytes, expected_input.va_start, site.pc)?;
        if decoded.ordinary_return {
            return Err(format!(
                "provider exported ordinary jr $ra return at 0x{:08x}",
                site.pc
            ));
        }
        let flow = CandidateFlowV1 {
            site: site.pc,
            ghidra_via_call: *via_call,
            isa_via_call: decoded.via_call,
            targets: targets.iter().map(|target| target.pc).collect(),
        };
        if candidates.insert(site.pc, flow).is_some() {
            return Err(format!("provider emitted duplicate site 0x{:08x}", site.pc));
        }
    }

    let mut native_sites = BTreeMap::new();
    for resolution in &bank.closure.indirect {
        let decoded =
            decode_computed_site(&bank_bytes, expected_input.va_start, resolution.site_pc)?;
        if decoded.ordinary_return || decoded.via_call != resolution.via_call {
            return Err(format!(
                "snapshot indirect classification disagrees with ISA at 0x{:08x}",
                resolution.site_pc
            ));
        }
        if native_sites
            .insert(resolution.site_pc, resolution)
            .is_some()
        {
            return Err(format!(
                "snapshot contains duplicate indirect site 0x{:08x}",
                resolution.site_pc
            ));
        }
    }

    let mut comparisons = Vec::new();
    let mut native_missing = Vec::new();
    let mut exhaustive_sites = 0usize;
    let mut exhaustive_exact = 0usize;
    let mut open_or_bounded = 0usize;
    let mut open_or_bounded_with_targets = 0usize;
    for (&site, resolution) in &native_sites {
        let candidate = candidates.get(&site);
        let native_targets: BTreeSet<_> = resolution.targets.iter().copied().collect();
        let ghidra_targets: BTreeSet<_> = candidate
            .map(|candidate| candidate.targets.iter().copied().collect())
            .unwrap_or_default();
        let missing: Vec<_> = native_targets
            .difference(&ghidra_targets)
            .copied()
            .collect();
        let extra: Vec<_> = ghidra_targets
            .difference(&native_targets)
            .copied()
            .collect();
        let is_exhaustive = resolution.state == IndirectProofState::Exhaustive;
        let exact = is_exhaustive && candidate.is_some() && missing.is_empty() && extra.is_empty();
        if is_exhaustive {
            exhaustive_sites += 1;
            exhaustive_exact += usize::from(exact);
        } else {
            open_or_bounded += 1;
            open_or_bounded_with_targets +=
                usize::from(candidate.is_some_and(|candidate| !candidate.targets.is_empty()));
        }
        if candidate.is_none() {
            native_missing.push(site);
        }
        comparisons.push(NativeComparisonV1 {
            site,
            native_state: proof_state_name(resolution.state),
            native_via_call: resolution.via_call,
            isa_via_call: resolution.via_call,
            native_targets: resolution.targets.clone(),
            ghidra_via_call: candidate.map(|candidate| candidate.ghidra_via_call),
            ghidra_targets: candidate.map(|candidate| candidate.targets.clone()),
            missing_from_ghidra: missing,
            extra_from_ghidra: extra,
            exact_exhaustive_match: exact,
        });
    }
    let native_keys: BTreeSet<_> = native_sites.keys().copied().collect();
    let ghidra_only: Vec<_> = candidates
        .keys()
        .copied()
        .filter(|site| !native_keys.contains(site))
        .collect();
    let classification_disagreements: Vec<_> = candidates
        .values()
        .filter(|candidate| candidate.ghidra_via_call != candidate.isa_via_call)
        .map(|candidate| candidate.site)
        .collect();

    let body = ReportBodyV1 {
        candidate_only: true,
        production_ingest_authority: false,
        bank: bank_name,
        normalized_rom_sha256: expected_input.normalized_rom_sha256,
        bank_bytes_sha256: expected_input.bank_bytes_sha256,
        mapping_sha256: expected_input.mapping_sha256,
        program_snapshot_sha256: frozen.program_snapshot_sha256,
        provider_jsonl_sha256: Sha256Digest::of(&provider_bytes),
        provider_source_sha256: output.source().source_sha256,
        provider_tool: output.source().tool.clone(),
        provider_artifact_identity_independently_verified: false,
        ghidra_candidates: candidates.into_values().collect(),
        native_comparisons: comparisons,
        native_sites_missing_from_ghidra: native_missing.clone(),
        ghidra_only_sites: ghidra_only.clone(),
        ghidra_isa_call_classification_disagreements: classification_disagreements.clone(),
        summary: SummaryV1 {
            native_sites: native_sites.len(),
            native_exhaustive_sites: exhaustive_sites,
            native_exhaustive_exact_matches: exhaustive_exact,
            native_exhaustive_disagreements: exhaustive_sites - exhaustive_exact,
            native_open_or_bounded_sites: open_or_bounded,
            native_open_or_bounded_with_ghidra_targets: open_or_bounded_with_targets,
            native_sites_missing_from_ghidra: native_missing.len(),
            ghidra_sites: native_keys.len() + ghidra_only.len() - native_missing.len(),
            ghidra_only_sites: ghidra_only.len(),
            ghidra_isa_call_classification_disagreements: classification_disagreements.len(),
        },
    };
    let body_bytes = serde_json::to_vec(&body)
        .map_err(|error| format!("serializing report semantics: {error}"))?;
    let mut semantic = Sha256::new();
    semantic.update(b"fn64.computed-flow-differential.v1\0");
    semantic.update(body_bytes);
    let report = ReportV1 {
        schema: "fn64.computed-flow-differential",
        schema_version: 1,
        semantic_sha256: Sha256Digest(semantic.finalize().into()),
        body,
    };
    let mut encoded = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("serializing report: {error}"))?;
    encoded.push(b'\n');
    if encoded.len() > MAX_OUTPUT_BYTES {
        return Err("computed-flow differential report exceeds output limit".into());
    }
    publish_new(&output_path, &encoded)?;
    println!(
        "compare-computed-flows: bank={} native={} ghidra={} exhaustive_exact={}/{} output_sha256={:x}",
        report.body.bank,
        report.body.summary.native_sites,
        report.body.summary.ghidra_sites,
        report.body.summary.native_exhaustive_exact_matches,
        report.body.summary.native_exhaustive_sites,
        Sha256::digest(&encoded),
    );
    Ok(())
}

fn proof_state_name(state: IndirectProofState) -> &'static str {
    match state {
        IndirectProofState::Exhaustive => "exhaustive",
        IndirectProofState::Bounded => "bounded",
        IndirectProofState::Open => "open",
    }
}

fn decode_computed_site(
    bank: &[u8],
    va_start: u32,
    site: u32,
) -> Result<DecodedComputedSite, String> {
    let offset =
        site.checked_sub(va_start)
            .ok_or_else(|| format!("computed site 0x{site:08x} precedes bank"))? as usize;
    let word_bytes = bank
        .get(offset..offset + 4)
        .ok_or_else(|| format!("computed site 0x{site:08x} is outside bank"))?;
    let word = u32::from_be_bytes(word_bytes.try_into().unwrap());
    if word >> 26 != 0 {
        return Err(format!("computed site 0x{site:08x} is not SPECIAL jr/jalr"));
    }
    match word & 0x3f {
        8 => Ok(DecodedComputedSite {
            via_call: false,
            ordinary_return: ((word >> 21) & 0x1f) == 31,
        }),
        9 => Ok(DecodedComputedSite {
            via_call: ((word >> 11) & 0x1f) != 0,
            ordinary_return: false,
        }),
        _ => Err(format!("computed site 0x{site:08x} is not jr/jalr")),
    }
}

fn read_bounded(path: &Path, label: &str, limit: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspecting {label} {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{label} must be a regular non-symlink file"));
    }
    if metadata.len() > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("reading {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} grew beyond {limit} bytes while reading"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isa_decoder_distinguishes_jump_call_and_return() {
        let words = [
            0x0100_0008u32, // jr $t0
            0x03e0_0008u32, // jr $ra
            0x0320_f809u32, // jalr $ra,$t9
            0x0320_0009u32, // jalr $zero,$t9
        ];
        let bytes: Vec<_> = words.into_iter().flat_map(u32::to_be_bytes).collect();
        assert_eq!(
            decode_computed_site(&bytes, 0x8000_0000, 0x8000_0000).unwrap(),
            DecodedComputedSite {
                via_call: false,
                ordinary_return: false
            }
        );
        assert!(
            decode_computed_site(&bytes, 0x8000_0000, 0x8000_0004)
                .unwrap()
                .ordinary_return
        );
        assert!(
            decode_computed_site(&bytes, 0x8000_0000, 0x8000_0008)
                .unwrap()
                .via_call
        );
        assert!(
            !decode_computed_site(&bytes, 0x8000_0000, 0x8000_000c)
                .unwrap()
                .via_call
        );
    }

    #[test]
    fn isa_decoder_rejects_noncomputed_and_out_of_bank_sites() {
        let bytes = 0x0c00_0000u32.to_be_bytes();
        assert!(decode_computed_site(&bytes, 0x8000_0000, 0x8000_0000).is_err());
        assert!(decode_computed_site(&bytes, 0x8000_0000, 0x8000_0004).is_err());
        assert!(decode_computed_site(&bytes, 0x8000_0000, 0x7fff_fffc).is_err());
    }
}
