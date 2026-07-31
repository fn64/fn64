//! Receipt-bound admission for discovery-only external-tool claims.
//!
//! This module proves a deliberately small thing: the supplied candidate
//! sidecar is the one named by a completed, seedless queue attempt for one
//! exact snapshot bank.  It does **not** trust the analyzer, its guard, or
//! the toolchain, and it does not make any claim about analyzer coverage.
//! In particular, the returned capability is not native evidence and cannot
//! add a fact, a partition boundary, an owner, or a traversal root.

use crate::snapshot::ProgramSnapshotV1;
use crate::tool_adapter::{
    ingest_tool_jsonl, AdapterLimits, Sha256Digest, ToolAdapterExpectation, ToolIdentity,
    ToolLineageRef, ToolLineageRole, ToolRunRole,
};
use crate::tool_claims::{
    bank_input_identity_v1, freeze_tool_claims_v1, program_snapshot_sha256_v2,
    validate_tool_claim_set_v1, ToolClaimSetV1,
};
use serde::Deserialize;

const MIB: u64 = 1024 * 1024;
const MAX_BANK_BYTES: u64 = 128 * MIB;
const QUEUE_REQUEST_SCHEMA: &str = "fn64.ghidra-snapshot-workspace-request";
const QUEUE_RECEIPT_SCHEMA: &str = "fn64.ghidra-snapshot-workspace-receipt";
const ATTEMPT_SCHEMA: &str = "fn64.ghidra-snapshot-workspace-attempt";
const RUNNER_RECEIPT_SCHEMA: &str = "fn64.ghidra-snapshot-bank-receipt";
const EVIDENCE_SCHEMA: &str = "fn64.snapshot-bank-evidence";
const CONFIG_SCHEMA: &str = "fn64.ghidra-bank-config";

/// The byte artifacts retained by one successful queue attempt.  All inputs
/// are untrusted interchange.  Callers must obtain them from their private
/// retention directory; this module performs no path or filesystem checks.
#[derive(Debug)]
pub struct DiscoveryOnlyReceiptBundle<'a> {
    pub queue_request: &'a [u8],
    pub terminal_queue_receipt: &'a [u8],
    pub bank_attempt_result: &'a [u8],
    pub runner_receipt: &'a [u8],
    pub runner_request: &'a [u8],
    pub evidence: &'a [u8],
    pub unseeded_config: &'a [u8],
    pub unseeded_tool_manifest: &'a [u8],
    pub provider_jsonl: &'a [u8],
    pub tool_claims: &'a [u8],
    pub snapshot: &'a [u8],
    pub bank_index: usize,
}

/// A receipt-only bundle cannot establish whether the analyzer exhausted its
/// search space. This explicit value prevents successful receipt validation
/// from being mistaken for analyzer completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyzerCompleteness {
    Unknown,
}

/// Candidate-only claims whose retained receipt chain was internally
/// consistent at admission time.
///
/// The fields remain private and this type intentionally has no `Clone`,
/// `Deserialize`, or public constructor.  Dropping it is the only way to
/// discard the validated association; no inverse conversion exists.
#[derive(Debug)]
pub struct ValidatedDiscoveryOnlyToolClaims {
    claims: ToolClaimSetV1,
    snapshot: ProgramSnapshotV1,
    bank_index: usize,
    tool_claim_set_sha256: Sha256Digest,
}

impl ValidatedDiscoveryOnlyToolClaims {
    /// Candidate-only semantic claims.  This accessor does not confer native
    /// proof authority; callers must keep using an independently justified
    /// corroboration rule.
    pub fn claims(&self) -> &ToolClaimSetV1 {
        &self.claims
    }

    /// The diagnostic snapshot to which the claims are exactly bound.
    pub fn snapshot(&self) -> &ProgramSnapshotV1 {
        &self.snapshot
    }

    /// Index of the exact bank selected from `snapshot()`.
    pub fn bank_index(&self) -> usize {
        self.bank_index
    }

    /// Raw sidecar digest named by the retained runner and queue receipts.
    pub fn tool_claim_set_sha256(&self) -> Sha256Digest {
        self.tool_claim_set_sha256
    }

    pub fn analyzer_completeness(&self) -> AnalyzerCompleteness {
        AnalyzerCompleteness::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateCorroborationError {
    Json {
        artifact: &'static str,
        detail: String,
    },
    InvalidSnapshotWire,
    UnsupportedSnapshotSchema,
    BankIndexOutOfRange,
    InvalidBankGeometry,
    QueueRequest,
    QueueReceipt,
    Attempt,
    RunnerReceipt,
    Evidence,
    Configuration,
    ToolClaims(String),
}

impl std::fmt::Display for CandidateCorroborationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CandidateCorroborationError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueRequestV1 {
    schema: String,
    schema_version: u32,
    source_manifest_sha256: Sha256Digest,
    normalized_rom_sha256: Sha256Digest,
    execution_mode: String,
    tools: QueueToolsV1,
    caps: QueueCapsV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueToolsV1 {
    queue: FileIdentityV1,
    runner: FileIdentityV1,
    stage: FileIdentityV1,
    ingest: FileIdentityV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueCapsV1 {
    max_launches: u64,
    max_wall_seconds: u64,
    max_attempts_per_bank: u64,
    max_ordinary_failures: u64,
    max_log_bytes: u64,
    max_attempt_bytes: u64,
    min_free_disk_bytes: u64,
    termination_grace_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIdentityV1 {
    byte_length: u64,
    sha256: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueReceiptV1 {
    schema: String,
    schema_version: u32,
    state: String,
    execution_mode: String,
    queue_request_sha256: Sha256Digest,
    source_manifest_sha256: Sha256Digest,
    normalized_rom_sha256: Sha256Digest,
    cohort: QueueCohortV1,
    banks: Vec<QueueReceiptBankV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueCohortV1 {
    common_sha256: Option<Sha256Digest>,
    ghidra_distribution_manifest_sha256: Option<Sha256Digest>,
    ghidra_distribution_file_count: Option<u64>,
    tool_artifact_scope: Option<String>,
    mode_tool_manifest_sha256: ModeToolManifestV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeToolManifestV1 {
    discovery_only: Option<Sha256Digest>,
    base_only: Option<Sha256Digest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueReceiptBankV1 {
    index: usize,
    state: String,
    receipt_sha256: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptResultV1 {
    schema: String,
    schema_version: u32,
    state: String,
    failure_class: Option<String>,
    runner_exit_status: Option<i64>,
    runner_attempt: Option<String>,
    runner_receipt_sha256: Option<Sha256Digest>,
    tool_claims_sha256: Option<Sha256Digest>,
    ghidra_distribution_manifest_sha256: Option<Sha256Digest>,
    unseeded_tool_manifest_sha256: Option<Sha256Digest>,
    common_cohort_sha256: Option<Sha256Digest>,
    stop_scheduling: bool,
    stdout: Option<FileIdentityV1>,
    stderr: Option<FileIdentityV1>,
    queue_request_sha256: Sha256Digest,
    source_manifest_sha256: Sha256Digest,
    attempt: u64,
    bank: AttemptBankV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptBankV1 {
    index: usize,
    name: String,
    bank_sha256: Sha256Digest,
    snapshot_artifact_sha256: Sha256Digest,
    program_snapshot_sha256: Sha256Digest,
    base_seed: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerReceiptV1 {
    schema: String,
    schema_version: u32,
    execution_mode: String,
    paired_comparison_complete: bool,
    completed_modes: Vec<String>,
    program_snapshot_sha256: Sha256Digest,
    bank: String,
    seeds: DiscoveryOnlySeedsV1,
    evidence_sha256: Sha256Digest,
    request_sha256: Sha256Digest,
    unseeded_tool_manifest_sha256: Sha256Digest,
    tool_claims_sha256: Sha256Digest,
    ghidra_distribution_manifest_complete: bool,
    ghidra_distribution_manifest_sha256: Sha256Digest,
    ghidra_distribution_file_count: u64,
    tool_artifact_scope: String,
    configuration_sha256: UnseededDigestV1,
    provider_jsonl_sha256: UnseededDigestV1,
    resource_evidence_sha256: RunnerResourceDigestsV1,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum DiscoveryOnlySeedsV1 {
    DiscoveryOnly { role: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnseededDigestV1 {
    unseeded: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerResourceDigestsV1 {
    ghidra_distribution_scan_log: Sha256Digest,
    ghidra_distribution_scan: Sha256Digest,
    ghidra_distribution_verify_log: Sha256Digest,
    ghidra_distribution_verify: Sha256Digest,
    stage: Sha256Digest,
    unseeded: Sha256Digest,
    ingest: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceV3 {
    schema: String,
    schema_version: u32,
    program_snapshot_sha256: Sha256Digest,
    input: crate::tool_adapter::BankInputIdentity,
    backing: EvidenceBackingV1,
    artifact: FileIdentityV1,
    seeds: DiscoveryOnlySeedsV1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceBackingV1 {
    rom_space: crate::facts::RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnseededConfigV1 {
    schema: String,
    schema_version: u32,
    mode: String,
    bank: String,
    va_start: u32,
    va_end: u32,
    base_seed: Option<u32>,
    snapshot_seed: Option<u32>,
    loader: String,
    processor: String,
    cspec: String,
    ghidra_version: String,
    analysis_timeout_seconds: u64,
    max_cpu: u64,
    heap_mib: u64,
    rss_mib: u64,
    min_free_percent: u64,
    wall_seconds: u64,
    tool_manifest_sha256: Sha256Digest,
    role: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArtifactManifestV1 {
    schema: String,
    schema_version: u32,
    tool_name: String,
    tool_version: String,
    artifacts: Vec<ToolArtifactV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArtifactV1 {
    path: String,
    byte_length: u64,
    sha256: Sha256Digest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIngestRequestV1 {
    schema: String,
    schema_version: u32,
    runs: Vec<ToolIngestRunV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIngestRunV1 {
    bank: String,
    jsonl: String,
    lineage_artifacts: Vec<ToolIngestLineageArtifactV1>,
    role: ToolRunRole,
    tool: ToolIdentity,
    tool_artifact_manifest: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ToolIngestLineageArtifactV1 {
    path: String,
    role: ToolLineageRole,
}

/// Validate internal receipt integrity for one discovery-only bank.
///
/// This is intentionally a *receipt* validator.  A successful result means
/// the bundle is internally self-consistent and seedless, and that replaying
/// its retained provider stream produces the supplied canonical sidecar. It
/// does not mean that Ghidra reached all code or that any candidate is an
/// owner.
pub fn validate_discovery_only_tool_claims_v1(
    bundle: DiscoveryOnlyReceiptBundle<'_>,
) -> Result<ValidatedDiscoveryOnlyToolClaims, CandidateCorroborationError> {
    let snapshot: ProgramSnapshotV1 = parse("snapshot", bundle.snapshot)?;
    let canonical =
        serde_json::to_vec(&snapshot).map_err(|error| CandidateCorroborationError::Json {
            artifact: "snapshot",
            detail: error.to_string(),
        })?;
    if bundle.snapshot != canonical.as_slice()
        && bundle.snapshot != [canonical.as_slice(), b"\n"].concat().as_slice()
    {
        return Err(CandidateCorroborationError::InvalidSnapshotWire);
    }
    let snapshot_sha = program_snapshot_sha256_v2(&snapshot)
        .map_err(|_| CandidateCorroborationError::UnsupportedSnapshotSchema)?;
    let bank = snapshot
        .banks
        .get(bundle.bank_index)
        .ok_or(CandidateCorroborationError::BankIndexOutOfRange)?;
    let bank_byte_length = validate_bank_geometry(bank.input.va_start, bank.input.va_end)?;
    let bank_name = bank.input.bank.clone();
    let bank_identity = bank_input_identity_v1(&snapshot, &bank_name)
        .map_err(|_| CandidateCorroborationError::Evidence)?;

    let request: QueueRequestV1 = parse("queue request", bundle.queue_request)?;
    if request.schema != QUEUE_REQUEST_SCHEMA
        || request.schema_version != 1
        || request.execution_mode != "candidate-only-sequential"
        || request.normalized_rom_sha256 != bank_identity.normalized_rom_sha256
        || !nonempty_file_identities(&request.tools)
        || !sane_caps(&request.caps)
    {
        return Err(CandidateCorroborationError::QueueRequest);
    }
    let request_sha = Sha256Digest::of(bundle.queue_request);

    let attempt: AttemptResultV1 = parse("bank attempt result", bundle.bank_attempt_result)?;
    if attempt.schema != ATTEMPT_SCHEMA
        || attempt.schema_version != 1
        || attempt.state != "success"
        || attempt.failure_class.is_some()
        || attempt.runner_exit_status != Some(0)
        || attempt.stop_scheduling
        || attempt.queue_request_sha256 != request_sha
        || attempt.source_manifest_sha256 != request.source_manifest_sha256
        || attempt.bank.index != bundle.bank_index
        || attempt.bank.name != bank_name
        || attempt.bank.base_seed.is_some()
        || attempt.bank.bank_sha256 != bank_identity.bank_bytes_sha256
        || attempt.bank.snapshot_artifact_sha256 != Sha256Digest::of(bundle.snapshot)
        || attempt.bank.program_snapshot_sha256 != snapshot_sha
        || attempt.attempt == 0
        || attempt
            .runner_attempt
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none()
    {
        return Err(CandidateCorroborationError::Attempt);
    }
    let Some(attempt_runner_sha) = attempt.runner_receipt_sha256 else {
        return Err(CandidateCorroborationError::Attempt);
    };
    let Some(attempt_claims_sha) = attempt.tool_claims_sha256 else {
        return Err(CandidateCorroborationError::Attempt);
    };
    let Some(attempt_distribution_sha) = attempt.ghidra_distribution_manifest_sha256 else {
        return Err(CandidateCorroborationError::Attempt);
    };
    let Some(attempt_tool_sha) = attempt.unseeded_tool_manifest_sha256 else {
        return Err(CandidateCorroborationError::Attempt);
    };
    let Some(attempt_cohort_sha) = attempt.common_cohort_sha256 else {
        return Err(CandidateCorroborationError::Attempt);
    };
    if attempt.stdout.is_none() || attempt.stderr.is_none() {
        return Err(CandidateCorroborationError::Attempt);
    }

    let terminal: QueueReceiptV1 = parse("terminal queue receipt", bundle.terminal_queue_receipt)?;
    let mut terminal_indices: Vec<_> = terminal.banks.iter().map(|entry| entry.index).collect();
    terminal_indices.sort_unstable();
    if terminal_indices.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CandidateCorroborationError::QueueReceipt);
    }
    let matching_terminal: Vec<_> = terminal
        .banks
        .iter()
        .filter(|entry| entry.index == bundle.bank_index)
        .collect();
    if terminal.schema != QUEUE_RECEIPT_SCHEMA
        || terminal.schema_version != 1
        || terminal.state != "candidate_queue_complete"
        || terminal.execution_mode != "candidate-only-sequential"
        || terminal.queue_request_sha256 != request_sha
        || terminal.source_manifest_sha256 != request.source_manifest_sha256
        || terminal.normalized_rom_sha256 != bank_identity.normalized_rom_sha256
        || matching_terminal.len() != 1
        || matching_terminal[0].state != "success"
        || matching_terminal[0].receipt_sha256 != Sha256Digest::of(bundle.bank_attempt_result)
        || terminal.cohort.common_sha256 != Some(attempt_cohort_sha)
        || terminal.cohort.ghidra_distribution_manifest_sha256 != Some(attempt_distribution_sha)
        || terminal.cohort.mode_tool_manifest_sha256.discovery_only != Some(attempt_tool_sha)
        || terminal.cohort.mode_tool_manifest_sha256.base_only.is_some()
        || terminal.cohort.ghidra_distribution_file_count == Some(0)
        || terminal.cohort.ghidra_distribution_file_count.is_none()
        || terminal.cohort.tool_artifact_scope.as_deref()
            != Some("all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers")
    {
        return Err(CandidateCorroborationError::QueueReceipt);
    }

    let runner: RunnerReceiptV1 = parse("runner receipt", bundle.runner_receipt)?;
    if runner.schema != RUNNER_RECEIPT_SCHEMA
        || runner.schema_version != 1
        || runner.execution_mode != "discovery-only"
        || runner.paired_comparison_complete
        || runner.completed_modes != ["unseeded"]
        || runner.program_snapshot_sha256 != snapshot_sha
        || runner.bank != bank_name
        || !is_discovery_only(&runner.seeds)
        || runner.evidence_sha256 != Sha256Digest::of(bundle.evidence)
        || attempt_runner_sha != Sha256Digest::of(bundle.runner_receipt)
        || runner.request_sha256 != Sha256Digest::of(bundle.runner_request)
        || runner.unseeded_tool_manifest_sha256 != attempt_tool_sha
        || runner.tool_claims_sha256 != attempt_claims_sha
        || !runner.ghidra_distribution_manifest_complete
        || runner.ghidra_distribution_manifest_sha256 != attempt_distribution_sha
        || runner.tool_artifact_scope
            != "all-ghidra-install-regular-files,jdk-java,fn64-analysis-scripts,and-bound-orchestration-helpers"
        || runner.configuration_sha256.unseeded != Sha256Digest::of(bundle.unseeded_config)
        || runner.provider_jsonl_sha256.unseeded != Sha256Digest::of(bundle.provider_jsonl)
        || !resource_digests_present(&runner.resource_evidence_sha256)
        || runner.ghidra_distribution_file_count
            != terminal.cohort.ghidra_distribution_file_count.unwrap_or_default()
    {
        return Err(CandidateCorroborationError::RunnerReceipt);
    }

    let evidence: EvidenceV3 = parse("evidence", bundle.evidence)?;
    if evidence.schema != EVIDENCE_SCHEMA
        || evidence.schema_version != 3
        || evidence.program_snapshot_sha256 != snapshot_sha
        || evidence.input != bank_identity
        || evidence.backing.rom_space != bank.input.rom_space
        || evidence.backing.rom_start != bank.input.rom_start
        || evidence.backing.rom_end != bank.input.rom_end
        || evidence.artifact.byte_length != bank_byte_length
        || evidence.artifact.sha256 != bank_identity.bank_bytes_sha256
        || !is_discovery_only(&evidence.seeds)
    {
        return Err(CandidateCorroborationError::Evidence);
    }

    let config: UnseededConfigV1 = parse("unseeded config", bundle.unseeded_config)?;
    if config.schema != CONFIG_SCHEMA
        || config.schema_version != 1
        || config.mode != "unseeded"
        || config.bank != bank_name
        || config.va_start != bank_identity.va_start
        || config.va_end != bank_identity.va_end
        || config.base_seed.is_some()
        || config.snapshot_seed.is_some()
        || config.role != "candidate_only"
        || config.tool_manifest_sha256 != attempt_tool_sha
        || config.loader != "BinaryLoader"
        || config.processor != "MIPS:BE:64:64-32addr"
        || config.cspec != "o32"
        || config.ghidra_version.is_empty()
        || config.analysis_timeout_seconds == 0
        || config.max_cpu == 0
        || config.heap_mib == 0
        || config.rss_mib == 0
        || config.min_free_percent > 100
        || config.wall_seconds == 0
    {
        return Err(CandidateCorroborationError::Configuration);
    }

    let tool_manifest: ToolArtifactManifestV1 =
        parse("unseeded tool manifest", bundle.unseeded_tool_manifest)?;
    if Sha256Digest::of(bundle.unseeded_tool_manifest) != attempt_tool_sha
        || tool_manifest.schema != "fn64.tool-artifact-manifest"
        || tool_manifest.schema_version != 1
        || tool_manifest.tool_name != "ghidra-headless-unseeded"
        || tool_manifest.tool_version != config.ghidra_version
        || !exact_unseeded_tool_artifacts(&tool_manifest.artifacts, attempt_distribution_sha)
    {
        return Err(CandidateCorroborationError::Configuration);
    }

    let ingest_request: ToolIngestRequestV1 = parse("runner request", bundle.runner_request)?;
    let expected_lineage = [
        ToolIngestLineageArtifactV1 {
            path: "config/unseeded.json".into(),
            role: ToolLineageRole::ToolConfiguration,
        },
        ToolIngestLineageArtifactV1 {
            path: "raw/evidence.json".into(),
            role: ToolLineageRole::EvidenceManifest,
        },
    ];
    if ingest_request.schema != "fn64.tool-ingest-request"
        || ingest_request.schema_version != 1
        || ingest_request.runs.len() != 1
        || !ingest_request.runs.iter().all(|run| {
            run.bank == bank_name
                && run.jsonl == "modes/unseeded/out/provider.jsonl"
                && run.lineage_artifacts == expected_lineage
                && run.role == ToolRunRole::FunctionBoundaryCandidates
                && run.tool.name == tool_manifest.tool_name
                && run.tool.version == tool_manifest.tool_version
                && run.tool.build_sha256 == attempt_tool_sha
                && run.tool_artifact_manifest == "tool-unseeded.json"
        })
    {
        return Err(CandidateCorroborationError::Configuration);
    }

    let claims: ToolClaimSetV1 = parse("tool claims", bundle.tool_claims)?;
    if Sha256Digest::of(bundle.tool_claims) != attempt_claims_sha
        || validate_tool_claim_set_v1(&snapshot, &claims).is_err()
        || claims.sources.len() != 1
        || claims.claims.is_empty()
        || !claims.sources.iter().all(|source| {
            source.schema_version == 2
                && source.tool.name == tool_manifest.tool_name
                && source.tool.version == tool_manifest.tool_version
                && source.tool.build_sha256 == attempt_tool_sha
                && source.role == ToolRunRole::FunctionBoundaryCandidates
                && source.input == bank_identity
                && source.lineage
                    == [
                        crate::tool_adapter::ToolLineageRef {
                            role: ToolLineageRole::DiscoverySnapshot,
                            source_sha256: snapshot_sha,
                        },
                        crate::tool_adapter::ToolLineageRef {
                            role: ToolLineageRole::EvidenceManifest,
                            source_sha256: runner.evidence_sha256,
                        },
                        crate::tool_adapter::ToolLineageRef {
                            role: ToolLineageRole::ToolConfiguration,
                            source_sha256: runner.configuration_sha256.unseeded,
                        },
                    ]
        })
    {
        return Err(CandidateCorroborationError::ToolClaims(
            "claim sidecar is not exact, semantic candidate-only evidence for this bank".into(),
        ));
    }

    let provider_jsonl = std::str::from_utf8(bundle.provider_jsonl).map_err(|_| {
        CandidateCorroborationError::ToolClaims("provider JSONL is not UTF-8".into())
    })?;
    let replay_lineage = vec![
        ToolLineageRef {
            role: ToolLineageRole::DiscoverySnapshot,
            source_sha256: snapshot_sha,
        },
        ToolLineageRef {
            role: ToolLineageRole::EvidenceManifest,
            source_sha256: runner.evidence_sha256,
        },
        ToolLineageRef {
            role: ToolLineageRole::ToolConfiguration,
            source_sha256: runner.configuration_sha256.unseeded,
        },
    ];
    let replayed_run = ingest_tool_jsonl(
        provider_jsonl,
        &ToolAdapterExpectation {
            input: bank_identity,
            role: ToolRunRole::FunctionBoundaryCandidates,
            lineage: replay_lineage,
            limits: AdapterLimits::default(),
        },
    )
    .map_err(|error| {
        CandidateCorroborationError::ToolClaims(format!(
            "retained provider JSONL failed semantic replay: {error}"
        ))
    })?;
    let replayed_claims = freeze_tool_claims_v1(&snapshot, [&replayed_run]).map_err(|error| {
        CandidateCorroborationError::ToolClaims(format!(
            "retained provider JSONL could not be frozen: {error}"
        ))
    })?;
    if replayed_claims != claims {
        return Err(CandidateCorroborationError::ToolClaims(
            "claim sidecar does not equal the retained provider JSONL replay".into(),
        ));
    }

    Ok(ValidatedDiscoveryOnlyToolClaims {
        claims,
        snapshot,
        bank_index: bundle.bank_index,
        tool_claim_set_sha256: attempt_claims_sha,
    })
}

fn validate_bank_geometry(va_start: u32, va_end: u32) -> Result<u64, CandidateCorroborationError> {
    let byte_length = va_end
        .checked_sub(va_start)
        .filter(|length| *length != 0)
        .ok_or(CandidateCorroborationError::InvalidBankGeometry)?;
    if !va_start.is_multiple_of(4)
        || !va_end.is_multiple_of(4)
        || u64::from(byte_length) > MAX_BANK_BYTES
    {
        return Err(CandidateCorroborationError::InvalidBankGeometry);
    }
    Ok(u64::from(byte_length))
}

fn parse<T: serde::de::DeserializeOwned>(
    artifact: &'static str,
    bytes: &[u8],
) -> Result<T, CandidateCorroborationError> {
    serde_json::from_slice(bytes).map_err(|error| CandidateCorroborationError::Json {
        artifact,
        detail: error.to_string(),
    })
}

fn is_discovery_only(value: &DiscoveryOnlySeedsV1) -> bool {
    matches!(value, DiscoveryOnlySeedsV1::DiscoveryOnly { role } if role == "candidate_only")
}

fn nonempty_file_identities(value: &QueueToolsV1) -> bool {
    [
        value.queue.byte_length,
        value.runner.byte_length,
        value.stage.byte_length,
        value.ingest.byte_length,
    ]
    .iter()
    .all(|length| *length > 0)
}

fn sane_caps(value: &QueueCapsV1) -> bool {
    value.max_launches > 0
        && value.max_wall_seconds > 0
        && value.max_attempts_per_bank > 0
        && value.max_ordinary_failures > 0
        && value.max_log_bytes > 0
        && value.max_attempt_bytes >= value.max_log_bytes
        && value.min_free_disk_bytes > 0
        && value.termination_grace_seconds > 0
}

fn resource_digests_present(value: &RunnerResourceDigestsV1) -> bool {
    let zero = Sha256Digest::of(&[]);
    [
        value.ghidra_distribution_scan_log,
        value.ghidra_distribution_scan,
        value.ghidra_distribution_verify_log,
        value.ghidra_distribution_verify,
        value.stage,
        value.unseeded,
        value.ingest,
    ]
    .iter()
    .all(|digest| *digest != zero)
}

fn exact_unseeded_tool_artifacts(
    artifacts: &[ToolArtifactV1],
    distribution_sha256: Sha256Digest,
) -> bool {
    const EXPECTED: [&str; 6] = [
        "tool-artifacts/Fn64ExportCandidates.java",
        "tool-artifacts/analyzeHeadless",
        "tool-artifacts/application.properties",
        "tool-artifacts/ghidra-distribution.json",
        "tool-artifacts/java",
        "tool-artifacts/orchestration.json",
    ];
    artifacts.len() == EXPECTED.len()
        && artifacts.iter().zip(EXPECTED).all(|(artifact, path)| {
            let digest_matches = if path == "tool-artifacts/ghidra-distribution.json" {
                artifact.sha256 == distribution_sha256
            } else {
                artifact.sha256 != Sha256Digest::of(&[])
            };
            artifact.path == path && artifact.byte_length > 0 && digest_matches
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_receipt_structs_reject_unknown_fields() {
        let mut value: serde_json::Value = serde_json::from_str(
            r#"{"schema":"fn64.ghidra-snapshot-bank-receipt","schema_version":1,"execution_mode":"discovery-only","paired_comparison_complete":false,"completed_modes":["unseeded"],"program_snapshot_sha256":"0000000000000000000000000000000000000000000000000000000000000000","bank":"boot","seeds":{"mode":"discovery_only","role":"candidate_only"},"evidence_sha256":"0000000000000000000000000000000000000000000000000000000000000000","request_sha256":"0000000000000000000000000000000000000000000000000000000000000000","unseeded_tool_manifest_sha256":"0000000000000000000000000000000000000000000000000000000000000000","tool_claims_sha256":"0000000000000000000000000000000000000000000000000000000000000000","ghidra_distribution_manifest_complete":true,"ghidra_distribution_manifest_sha256":"0000000000000000000000000000000000000000000000000000000000000000","ghidra_distribution_file_count":1,"tool_artifact_scope":"scope","configuration_sha256":{"unseeded":"0000000000000000000000000000000000000000000000000000000000000000"},"provider_jsonl_sha256":{"unseeded":"0000000000000000000000000000000000000000000000000000000000000000"},"resource_evidence_sha256":{"ghidra_distribution_scan_log":"0000000000000000000000000000000000000000000000000000000000000000","ghidra_distribution_scan":"0000000000000000000000000000000000000000000000000000000000000000","ghidra_distribution_verify_log":"0000000000000000000000000000000000000000000000000000000000000000","ghidra_distribution_verify":"0000000000000000000000000000000000000000000000000000000000000000","stage":"0000000000000000000000000000000000000000000000000000000000000000","unseeded":"0000000000000000000000000000000000000000000000000000000000000000","ingest":"0000000000000000000000000000000000000000000000000000000000000000"}}"#,
        )
        .unwrap();
        assert!(serde_json::from_value::<RunnerReceiptV1>(value.clone()).is_ok());
        value["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<RunnerReceiptV1>(value).is_err());
    }

    #[test]
    fn non_discovery_seed_variant_cannot_deserialize() {
        assert!(serde_json::from_slice::<DiscoveryOnlySeedsV1>(
            br#"{"mode":"base_only","base_seed":2147483648}"#
        )
        .is_err());
    }

    #[test]
    fn bank_geometry_is_checked_aligned_and_capped() {
        assert_eq!(validate_bank_geometry(0x8000_0000, 0x8000_0040), Ok(64));
        for (start, end) in [
            (0x8000_0000, 0x8000_0000),
            (0x8000_0004, 0x8000_0000),
            (0x8000_0001, 0x8000_0040),
            (0x8000_0000, 0x8000_0041),
            (0x7000_0000, 0x8000_0004),
        ] {
            assert_eq!(
                validate_bank_geometry(start, end),
                Err(CandidateCorroborationError::InvalidBankGeometry)
            );
        }
    }
}
