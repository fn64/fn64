//! Strict admission for sealed cold-function-training workspaces.
//!
//! This loader accepts only the current training schema emitted by
//! `produce_snapshot_workspace`.  It deliberately does not accept the legacy
//! diagnostic/Ghidra manifests: changing an `intended_use` string is not a
//! provenance conversion.  Validation is completed before this module returns
//! a capability.  Snapshot and bank bytes are then revalidated and borrowed to
//! a visitor one bank at a time, keeping full-ROM workspaces out of memory.

use crate::facts::{BankAddr, CandidateDetector, RomAddressSpace};
use crate::grade_candidates::{
    AddressedPhysicalEntryV2, CandidatePhysicalProvenanceV2, DetectorCandidateIdentitiesV2,
    ScopedCandidateIdentitiesV3, SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3,
};
use crate::snapshot::{ProgramSnapshotV1, PROGRAM_SNAPSHOT_SCHEMA_V5};
use crate::tool_adapter::Sha256Digest;
use crate::tool_claims::program_snapshot_sha256_v2;
use crate::workspace_artifacts::validate_workspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const MANIFEST_NAME: &str = "snapshot-workspace.json";
const CANDIDATE_NAME: &str = "cold-candidates.json";
const MANIFEST_LIMIT: u64 = 16 * MIB;
const CANDIDATE_LIMIT: u64 = 64 * MIB;
const MAX_BANKS: usize = 4096;
const MAX_SNAPSHOT_ARTIFACT_BYTES: u64 = 128 * MIB;
const MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES: u64 = 1024 * MIB;
const MAX_AGGREGATE_BANK_BYTES: u64 = 256 * MIB;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWorkspaceIdentity {
    pub manifest_sha256: Sha256Digest,
    pub normalized_rom_sha256: Sha256Digest,
    pub scoped_candidate_identities_v3_sha256: Sha256Digest,
    pub state: ValidatedWorkspaceState,
    pub bank_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatedWorkspaceState {
    Composed,
    OpenNoProvenBanks,
}

/// A fully checked workspace capability.
///
/// The path and receipts stay private so consumers cannot skip revalidation.
/// `visit_banks` reopens and checks each artifact immediately before exposing
/// borrowed bytes, then drops it before advancing to the next bank.
pub struct ValidatedSnapshotWorkspace {
    root: PathBuf,
    manifest: WorkspaceManifest,
    identity: SnapshotWorkspaceIdentity,
}

pub struct ValidatedSnapshotBank<'a> {
    pub index: usize,
    pub bank: &'a str,
    pub bytes: &'a [u8],
    pub snapshot: &'a ProgramSnapshotV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWorkspaceError(String);

impl std::fmt::Display for SnapshotWorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SnapshotWorkspaceError {}

impl SnapshotWorkspaceError {
    pub fn visitor(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<String> for SnapshotWorkspaceError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl ValidatedSnapshotWorkspace {
    pub fn identity(&self) -> &SnapshotWorkspaceIdentity {
        &self.identity
    }

    /// Revalidate and expose the answer-key-free candidate identity receipt.
    pub fn visit_candidate_identities<R>(
        &self,
        visitor: impl FnOnce(&ScopedCandidateIdentitiesV3) -> R,
    ) -> Result<R, SnapshotWorkspaceError> {
        verify_namespace(&self.root, &self.manifest)?;
        let candidates = load_candidates(&self.root, &self.manifest.cold_training)?;
        Ok(visitor(&candidates))
    }

    /// Revalidate and visit banks in manifest order with O(one-bank) memory.
    pub fn visit_banks(
        &self,
        mut visitor: impl FnMut(ValidatedSnapshotBank<'_>) -> Result<(), SnapshotWorkspaceError>,
    ) -> Result<(), SnapshotWorkspaceError> {
        verify_namespace(&self.root, &self.manifest)?;
        for receipt in &self.manifest.banks {
            let bytes = load_bound_artifact(
                &self.root.join(&receipt.bank_artifact),
                receipt.byte_length,
                receipt.bank_sha256,
                MAX_AGGREGATE_BANK_BYTES,
                "bank artifact",
            )?;
            let snapshot_bytes = load_bound_artifact(
                &self.root.join(&receipt.snapshot_artifact),
                receipt.snapshot_artifact_byte_length,
                receipt.snapshot_artifact_sha256,
                MAX_SNAPSHOT_ARTIFACT_BYTES,
                "snapshot artifact",
            )?;
            let snapshot =
                parse_and_verify_snapshot(receipt, &self.manifest, &bytes, &snapshot_bytes)?;
            visitor(ValidatedSnapshotBank {
                index: receipt.index,
                bank: &receipt.bank,
                bytes: &bytes,
                snapshot: &snapshot,
            })?;
        }
        verify_namespace(&self.root, &self.manifest)?;
        Ok(())
    }
}

/// Validate a sealed cold-training workspace without accepting a ROM or an
/// answer key.  No game-derived bytes escape in the returned capability.
pub fn validate_snapshot_workspace(
    path: &Path,
) -> Result<ValidatedSnapshotWorkspace, SnapshotWorkspaceError> {
    let root = validate_workspace(path).map_err(SnapshotWorkspaceError)?;
    let manifest_bytes = read_private_regular_bounded(
        &root.join(MANIFEST_NAME),
        MANIFEST_LIMIT,
        "workspace manifest",
    )?;
    let manifest_sha256 = Sha256Digest::of(&manifest_bytes);
    let manifest: WorkspaceManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| SnapshotWorkspaceError(format!("parsing workspace manifest: {error}")))?;
    validate_manifest(&manifest)?;
    verify_namespace(&root, &manifest)?;
    let candidates = load_candidates(&root, &manifest.cold_training)?;
    let candidate_digest = parse_digest(
        &manifest.cold_training.scoped_candidate_identities_v3_sha256,
        "scoped candidate identity digest",
    )?;
    if candidates.digest_sha256() != candidate_digest.to_hex() {
        return Err(SnapshotWorkspaceError(
            "cold candidate semantic digest changed after strict decoding".into(),
        ));
    }

    // Complete the expensive validation before returning a capability.  The
    // visitor API rechecks on consumption so a post-admission replacement
    // cannot inherit this result.
    for receipt in &manifest.banks {
        let bank_bytes = load_bound_artifact(
            &root.join(&receipt.bank_artifact),
            receipt.byte_length,
            receipt.bank_sha256,
            MAX_AGGREGATE_BANK_BYTES,
            "bank artifact",
        )?;
        let snapshot_bytes = load_bound_artifact(
            &root.join(&receipt.snapshot_artifact),
            receipt.snapshot_artifact_byte_length,
            receipt.snapshot_artifact_sha256,
            MAX_SNAPSHOT_ARTIFACT_BYTES,
            "snapshot artifact",
        )?;
        parse_and_verify_snapshot(receipt, &manifest, &bank_bytes, &snapshot_bytes)?;
    }
    verify_namespace(&root, &manifest)?;

    let normalized_rom_sha256 =
        parse_digest(&manifest.normalized_rom_sha256, "normalized ROM digest")?;
    let state = match manifest.state {
        WorkspaceState::Composed => ValidatedWorkspaceState::Composed,
        WorkspaceState::Open => ValidatedWorkspaceState::OpenNoProvenBanks,
    };
    let identity = SnapshotWorkspaceIdentity {
        manifest_sha256,
        normalized_rom_sha256,
        scoped_candidate_identities_v3_sha256: candidate_digest,
        state,
        bank_count: manifest.banks.len(),
    };
    Ok(ValidatedSnapshotWorkspace {
        root,
        manifest,
        identity,
    })
}

/// Validate and consume a workspace with exactly one read of each large bank
/// and snapshot artifact.
///
/// Callers may build a compact index in the visitors, but must not consult an
/// answer key until this function returns `Ok`: a late artifact or namespace
/// failure deliberately withholds the validated identity.
pub fn validate_snapshot_workspace_streaming(
    path: &Path,
    candidate_visitor: impl FnOnce(&ScopedCandidateIdentitiesV3) -> Result<(), SnapshotWorkspaceError>,
    mut bank_visitor: impl FnMut(ValidatedSnapshotBank<'_>) -> Result<(), SnapshotWorkspaceError>,
) -> Result<SnapshotWorkspaceIdentity, SnapshotWorkspaceError> {
    let root = validate_workspace(path).map_err(SnapshotWorkspaceError)?;
    let manifest_bytes = read_private_regular_bounded(
        &root.join(MANIFEST_NAME),
        MANIFEST_LIMIT,
        "workspace manifest",
    )?;
    let manifest_sha256 = Sha256Digest::of(&manifest_bytes);
    let manifest: WorkspaceManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| SnapshotWorkspaceError(format!("parsing workspace manifest: {error}")))?;
    validate_manifest(&manifest)?;
    verify_namespace(&root, &manifest)?;

    let candidates = load_candidates(&root, &manifest.cold_training)?;
    let candidate_digest = parse_digest(
        &manifest.cold_training.scoped_candidate_identities_v3_sha256,
        "scoped candidate identity v3 digest",
    )?;
    if candidates.digest_sha256() != candidate_digest.to_hex() {
        return Err(SnapshotWorkspaceError(
            "cold candidate semantic digest changed after strict decoding".into(),
        ));
    }
    candidate_visitor(&candidates)?;

    for receipt in &manifest.banks {
        let bytes = load_bound_artifact(
            &root.join(&receipt.bank_artifact),
            receipt.byte_length,
            receipt.bank_sha256,
            MAX_AGGREGATE_BANK_BYTES,
            "bank artifact",
        )?;
        let snapshot_bytes = load_bound_artifact(
            &root.join(&receipt.snapshot_artifact),
            receipt.snapshot_artifact_byte_length,
            receipt.snapshot_artifact_sha256,
            MAX_SNAPSHOT_ARTIFACT_BYTES,
            "snapshot artifact",
        )?;
        let snapshot = parse_and_verify_snapshot(receipt, &manifest, &bytes, &snapshot_bytes)?;
        bank_visitor(ValidatedSnapshotBank {
            index: receipt.index,
            bank: &receipt.bank,
            bytes: &bytes,
            snapshot: &snapshot,
        })?;
    }
    verify_namespace(&root, &manifest)?;

    Ok(SnapshotWorkspaceIdentity {
        manifest_sha256,
        normalized_rom_sha256: parse_digest(
            &manifest.normalized_rom_sha256,
            "normalized ROM digest",
        )?,
        scoped_candidate_identities_v3_sha256: candidate_digest,
        state: match manifest.state {
            WorkspaceState::Composed => ValidatedWorkspaceState::Composed,
            WorkspaceState::Open => ValidatedWorkspaceState::OpenNoProvenBanks,
        },
        bank_count: manifest.banks.len(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceManifest {
    schema: String,
    schema_version: u32,
    state: WorkspaceState,
    open_reason: Option<String>,
    normalized_rom_sha256: String,
    discovery: DiscoveryReceipt,
    limits: LimitsReceipt,
    snapshot_wire: SnapshotWireReceipt,
    aggregate_snapshot_artifact_bytes: u64,
    rom_recompilation_complete: bool,
    remaining_recompilation_frontier: String,
    intended_use: String,
    cold_training: ColdTrainingReceipt,
    #[serde(default)]
    selection: SelectionPresence,
    banks: Vec<BankReceipt>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceState {
    Composed,
    Open,
}

#[derive(Debug, Default)]
enum SelectionPresence {
    #[default]
    Absent,
    Present,
}

impl<'de> Deserialize<'de> for SelectionPresence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _ = serde_json::Value::deserialize(deserializer)?;
        Ok(Self::Present)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColdTrainingReceipt {
    schema_version: u32,
    algorithm: String,
    answer_key_present: bool,
    candidate_artifact: String,
    candidate_artifact_byte_length: u64,
    candidate_artifact_sha256: Sha256Digest,
    scoped_candidate_identities_v3_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryReceipt {
    selected: DiscoveryStrategyWire,
    outcomes: Vec<StrategyOutcomeWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiscoveryStrategyWire {
    BootBankOpen,
    BootBankOnly,
    RecoveredVrom,
    RecoveredOverlays,
    UntabledDeltaVote,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyOutcomeWire {
    strategy: DiscoveryStrategyWire,
    candidate_tables: usize,
    admitted_tables: usize,
    admitted_intervals: usize,
    decoded_file_limit_hits: usize,
    proven_mappings: usize,
    supported_mappings: usize,
    request_dma_open_rows: usize,
    request_dma_incomplete: bool,
    request_dma_input_limit_hit: bool,
    physical_wrapper_candidates_examined: usize,
    wrapper_semantic_proof_unavailable: usize,
    physical_wrapper_candidate_limit_hit: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsReceipt {
    max_rom_bytes: u64,
    max_banks: usize,
    max_snapshot_artifact_bytes: usize,
    max_aggregate_snapshot_artifact_bytes: u64,
    max_discovery_decoded_vrom_file_bytes: usize,
    max_preparation_decoded_vrom_file_bytes: usize,
    max_projected_fact_rows: u64,
    max_projected_fact_bytes: u64,
    max_aggregate_materialized_bytes: u64,
    max_cross_bank_authority_records: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotWireReceipt {
    schema_version: u32,
    authority: String,
    duplicates_fact_db_per_bank: bool,
    remaining_large_rom_frontier: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BankReceipt {
    index: usize,
    bank: String,
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
    byte_length: u64,
    backing_evidence_fact_indices: Vec<usize>,
    bank_sha256: Sha256Digest,
    bank_artifact: String,
    snapshot_artifact: String,
    snapshot_artifact_byte_length: u64,
    snapshot_artifact_sha256: Sha256Digest,
    program_snapshot_sha256: Sha256Digest,
    ghidra_seeds: GhidraSeedsWire,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum GhidraSeedsWire {
    DiscoveryOnly {
        role: String,
    },
    BaseOnly {
        base_seed: u32,
        base_seed_role: String,
    },
    Paired {
        base_seed: u32,
        base_seed_role: String,
        snapshot_seed: u32,
        snapshot_seed_role: String,
        snapshot_seed_assessment: String,
    },
}

fn validate_manifest(manifest: &WorkspaceManifest) -> Result<(), SnapshotWorkspaceError> {
    if manifest.schema != "fn64.snapshot-workspace"
        || manifest.schema_version != 3
        || manifest.intended_use != "sealed_cold_function_training_input"
    {
        return Err(SnapshotWorkspaceError(
            "workspace is not the current sealed cold-function-training schema".into(),
        ));
    }
    parse_digest(&manifest.normalized_rom_sha256, "normalized ROM digest")?;
    if manifest.rom_recompilation_complete
        || manifest.remaining_recompilation_frontier != "proven_bank_and_callable_owner_closure"
        || !matches!(manifest.selection, SelectionPresence::Absent)
    {
        return Err(SnapshotWorkspaceError(
            "training workspace carries selected-bank or completion semantics".into(),
        ));
    }
    match manifest.state {
        WorkspaceState::Composed
            if manifest.open_reason.is_none() && !manifest.banks.is_empty() => {}
        WorkspaceState::Open
            if manifest.open_reason.as_deref() == Some("no_proven_banks")
                && manifest.banks.is_empty()
                && manifest.aggregate_snapshot_artifact_bytes == 0
                && manifest
                    .discovery
                    .outcomes
                    .iter()
                    .all(|outcome| outcome.proven_mappings == 0) => {}
        _ => {
            return Err(SnapshotWorkspaceError(
                "training state/open_reason/bank shape is inconsistent".into(),
            ))
        }
    }
    if manifest.cold_training.schema_version != 3
        || manifest.cold_training.algorithm != "fn64.cold-function-training.v3"
        || manifest.cold_training.answer_key_present
        || manifest.cold_training.candidate_artifact != CANDIDATE_NAME
        || manifest.cold_training.candidate_artifact_byte_length > CANDIDATE_LIMIT
    {
        return Err(SnapshotWorkspaceError(
            "cold-training receipt is not the current answer-key-free form".into(),
        ));
    }
    parse_digest(
        &manifest.cold_training.scoped_candidate_identities_v3_sha256,
        "scoped candidate identity digest",
    )?;
    validate_limits(&manifest.limits)?;
    if manifest.snapshot_wire.schema_version != PROGRAM_SNAPSHOT_SCHEMA_V5
        || manifest.snapshot_wire.authority != "diagnostic_only"
        || manifest.snapshot_wire.duplicates_fact_db_per_bank
        || manifest.snapshot_wire.remaining_large_rom_frontier != "streaming_v5"
    {
        return Err(SnapshotWorkspaceError(
            "snapshot wire receipt does not describe current projected schema v5".into(),
        ));
    }
    validate_discovery(&manifest.discovery)?;
    if manifest.banks.len() > MAX_BANKS {
        return Err(SnapshotWorkspaceError(
            "bank count exceeds loader bound".into(),
        ));
    }
    let mut aggregate_snapshots = 0u64;
    let mut aggregate_banks = 0u64;
    let mut prior_name: Option<&str> = None;
    for (index, bank) in manifest.banks.iter().enumerate() {
        if bank.index != index
            || bank.bank_artifact != bank_artifact_name(index)
            || bank.snapshot_artifact != snapshot_artifact_name(index)
        {
            return Err(SnapshotWorkspaceError(
                "bank receipts do not use fixed consecutive artifact names".into(),
            ));
        }
        validate_bank_token(&bank.bank)?;
        if prior_name.is_some_and(|prior| prior >= bank.bank.as_str()) {
            return Err(SnapshotWorkspaceError(
                "bank receipts are not in unique canonical name order".into(),
            ));
        }
        prior_name = Some(&bank.bank);
        let rom_span = bank.rom_end.checked_sub(bank.rom_start).map(u64::from);
        let va_span = bank.va_end.checked_sub(bank.va_start).map(u64::from);
        if bank.byte_length == 0
            || rom_span != Some(bank.byte_length)
            || va_span != Some(bank.byte_length)
        {
            return Err(SnapshotWorkspaceError(format!(
                "bank {index} has inconsistent geometry"
            )));
        }
        match bank.rom_space {
            RomAddressSpace::Physical if !bank.backing_evidence_fact_indices.is_empty() => {
                return Err(SnapshotWorkspaceError(format!(
                    "physical bank {index} carries VROM backing evidence"
                )))
            }
            RomAddressSpace::Virtual if bank.backing_evidence_fact_indices.len() != 1 => {
                return Err(SnapshotWorkspaceError(format!(
                    "virtual bank {index} lacks one backing evidence index"
                )))
            }
            _ => {}
        }
        validate_seeds(bank)?;
        aggregate_banks = checked_add(aggregate_banks, bank.byte_length, "bank bytes")?;
        aggregate_snapshots = checked_add(
            aggregate_snapshots,
            bank.snapshot_artifact_byte_length,
            "snapshot bytes",
        )?;
        if bank.snapshot_artifact_byte_length > MAX_SNAPSHOT_ARTIFACT_BYTES {
            return Err(SnapshotWorkspaceError(format!(
                "snapshot {index} exceeds per-artifact bound"
            )));
        }
    }
    if aggregate_banks > MAX_AGGREGATE_BANK_BYTES
        || aggregate_snapshots > MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES
        || aggregate_snapshots != manifest.aggregate_snapshot_artifact_bytes
    {
        return Err(SnapshotWorkspaceError(
            "aggregate artifact lengths exceed or disagree with manifest bounds".into(),
        ));
    }
    Ok(())
}

fn validate_limits(limits: &LimitsReceipt) -> Result<(), SnapshotWorkspaceError> {
    if limits.max_rom_bytes != 64 * MIB
        || limits.max_banks != MAX_BANKS
        || limits.max_snapshot_artifact_bytes as u64 != MAX_SNAPSHOT_ARTIFACT_BYTES
        || limits.max_aggregate_snapshot_artifact_bytes != MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES
        || limits.max_discovery_decoded_vrom_file_bytes as u64 != 64 * MIB
        || limits.max_preparation_decoded_vrom_file_bytes as u64 != 64 * MIB
        || limits.max_projected_fact_rows != 4_000_000
        || limits.max_projected_fact_bytes != 256 * MIB
        || limits.max_aggregate_materialized_bytes != MAX_AGGREGATE_BANK_BYTES
        || limits.max_cross_bank_authority_records != 1_048_576
    {
        return Err(SnapshotWorkspaceError(
            "workspace resource limits differ from the current producer envelope".into(),
        ));
    }
    Ok(())
}

fn validate_discovery(discovery: &DiscoveryReceipt) -> Result<(), SnapshotWorkspaceError> {
    let strategies: Vec<_> = discovery
        .outcomes
        .iter()
        .map(|item| item.strategy)
        .collect();
    let baseline_ok = matches!(
        strategies.first(),
        Some(DiscoveryStrategyWire::BootBankOpen | DiscoveryStrategyWire::BootBankOnly)
    );
    let expected_prefix = [
        DiscoveryStrategyWire::RecoveredVrom,
        DiscoveryStrategyWire::RecoveredOverlays,
    ];
    if !baseline_ok
        || strategies.get(1..3) != Some(expected_prefix.as_slice())
        || !matches!(strategies.len(), 3 | 4)
        || (strategies.len() == 4 && strategies[3] != DiscoveryStrategyWire::UntabledDeltaVote)
        || !strategies.contains(&discovery.selected)
    {
        return Err(SnapshotWorkspaceError(
            "discovery outcomes are incomplete, reordered, or inconsistent".into(),
        ));
    }
    // Touch every field here: serde's exact shape plus these arithmetic
    // invariants prevent a receipt from becoming an unbounded side channel.
    for outcome in &discovery.outcomes {
        if outcome.admitted_tables > outcome.candidate_tables
            || outcome.admitted_intervals > MAX_BANKS
            || outcome.proven_mappings > MAX_BANKS
            || outcome.supported_mappings > MAX_BANKS
            || outcome.decoded_file_limit_hits > MAX_BANKS
            || outcome.request_dma_open_rows > MAX_BANKS
            || outcome.physical_wrapper_candidates_examined > MAX_BANKS
            || outcome.wrapper_semantic_proof_unavailable
                > outcome.physical_wrapper_candidates_examined
            || outcome.request_dma_incomplete != (outcome.request_dma_open_rows != 0)
            || outcome.request_dma_input_limit_hit && !outcome.request_dma_incomplete
            || outcome.physical_wrapper_candidate_limit_hit && !outcome.request_dma_incomplete
            || outcome.wrapper_semantic_proof_unavailable != 0 && !outcome.request_dma_incomplete
        {
            return Err(SnapshotWorkspaceError(
                "discovery outcome exceeds structural bounds".into(),
            ));
        }
    }
    Ok(())
}

fn validate_seeds(bank: &BankReceipt) -> Result<(), SnapshotWorkspaceError> {
    let in_bank = |pc: u32| pc.is_multiple_of(4) && pc >= bank.va_start && pc < bank.va_end;
    match &bank.ghidra_seeds {
        GhidraSeedsWire::DiscoveryOnly { role } if role == "candidate_only" => Ok(()),
        GhidraSeedsWire::BaseOnly {
            base_seed,
            base_seed_role,
        } if base_seed_role == "proven_owner" && in_bank(*base_seed) => Ok(()),
        GhidraSeedsWire::Paired {
            base_seed,
            base_seed_role,
            snapshot_seed,
            snapshot_seed_role,
            snapshot_seed_assessment,
        } if base_seed_role == "proven_owner"
            && snapshot_seed_role == "assessed_owner"
            && matches!(
                snapshot_seed_assessment.as_str(),
                "proven" | "candidate" | "ambiguous"
            )
            && base_seed != snapshot_seed
            && in_bank(*base_seed)
            && in_bank(*snapshot_seed) =>
        {
            Ok(())
        }
        _ => Err(SnapshotWorkspaceError(
            "Ghidra seed receipt is not canonical for its bank".into(),
        )),
    }
}

fn parse_and_verify_snapshot(
    receipt: &BankReceipt,
    manifest: &WorkspaceManifest,
    bank_bytes: &[u8],
    artifact: &[u8],
) -> Result<ProgramSnapshotV1, SnapshotWorkspaceError> {
    let snapshot: ProgramSnapshotV1 = serde_json::from_slice(artifact).map_err(|error| {
        SnapshotWorkspaceError(format!("parsing snapshot {}: {error}", receipt.index))
    })?;
    let mut canonical = serde_json::to_vec(&snapshot).map_err(|error| {
        SnapshotWorkspaceError(format!("serializing snapshot {}: {error}", receipt.index))
    })?;
    canonical.push(b'\n');
    if canonical != artifact {
        return Err(SnapshotWorkspaceError(format!(
            "snapshot {} is not the canonical current wire",
            receipt.index
        )));
    }
    if snapshot.schema_version != PROGRAM_SNAPSHOT_SCHEMA_V5
        || snapshot.normalized_rom_sha256 != manifest.normalized_rom_sha256
        || snapshot.banks.len() != 1
    {
        return Err(SnapshotWorkspaceError(format!(
            "snapshot {} has wrong schema, ROM identity, or bank cardinality",
            receipt.index
        )));
    }
    let input = &snapshot.banks[0].input;
    if input.bank != receipt.bank
        || input.va_start != receipt.va_start
        || input.va_end != receipt.va_end
        || input.rom_space != receipt.rom_space
        || input.rom_start != receipt.rom_start
        || input.rom_end != receipt.rom_end
        || input.bytes_sha256 != receipt.bank_sha256.to_hex()
        || Sha256Digest::of(bank_bytes) != receipt.bank_sha256
    {
        return Err(SnapshotWorkspaceError(format!(
            "snapshot {} bank identity/geometry does not match its receipt",
            receipt.index
        )));
    }
    let semantic = program_snapshot_sha256_v2(&snapshot).map_err(|error| {
        SnapshotWorkspaceError(format!(
            "hashing semantic snapshot {}: {error}",
            receipt.index
        ))
    })?;
    if semantic != receipt.program_snapshot_sha256 {
        return Err(SnapshotWorkspaceError(format!(
            "snapshot {} semantic digest mismatch",
            receipt.index
        )));
    }
    Ok(snapshot)
}

fn load_candidates(
    root: &Path,
    receipt: &ColdTrainingReceipt,
) -> Result<ScopedCandidateIdentitiesV3, SnapshotWorkspaceError> {
    let bytes = load_bound_artifact(
        &root.join(CANDIDATE_NAME),
        receipt.candidate_artifact_byte_length,
        receipt.candidate_artifact_sha256,
        CANDIDATE_LIMIT,
        "cold candidate artifact",
    )?;
    let strict: StrictScopedCandidates = serde_json::from_slice(&bytes).map_err(|error| {
        SnapshotWorkspaceError(format!("parsing cold candidate artifact: {error}"))
    })?;
    let candidates = strict.into_public()?;
    let mut canonical = serde_json::to_vec(&candidates).map_err(|error| {
        SnapshotWorkspaceError(format!("serializing cold candidate artifact: {error}"))
    })?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err(SnapshotWorkspaceError(
            "cold candidate artifact is not canonical".into(),
        ));
    }
    validate_candidate_order(&candidates)?;
    Ok(candidates)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictScopedCandidates {
    schema_version: u32,
    per_detector: Vec<StrictDetectorCandidates>,
    combined_candidates: Vec<StrictPhysicalEntry>,
    combined_ungradable: Vec<StrictBankAddr>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictDetectorCandidates {
    detector: CandidateDetector,
    candidates: Vec<StrictPhysicalEntry>,
    provenance: Vec<StrictCandidateProvenance>,
    ungradable: Vec<StrictBankAddr>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictCandidateProvenance {
    candidate: StrictPhysicalEntry,
    sources: Vec<StrictPhysicalEntry>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictPhysicalEntry {
    rom_space: RomAddressSpace,
    rom: u32,
    vram: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictBankAddr {
    bank: String,
    pc: u32,
}

impl StrictScopedCandidates {
    fn into_public(self) -> Result<ScopedCandidateIdentitiesV3, SnapshotWorkspaceError> {
        if self.schema_version != SCOPED_CANDIDATE_IDENTITY_SCHEMA_V3 {
            return Err(SnapshotWorkspaceError(
                "unsupported cold candidate identity schema".into(),
            ));
        }
        Ok(ScopedCandidateIdentitiesV3 {
            schema_version: self.schema_version,
            per_detector: self
                .per_detector
                .into_iter()
                .map(|item| DetectorCandidateIdentitiesV2 {
                    detector: item.detector,
                    candidates: item.candidates.into_iter().map(Into::into).collect(),
                    provenance: item
                        .provenance
                        .into_iter()
                        .map(|entry| CandidatePhysicalProvenanceV2 {
                            candidate: entry.candidate.into(),
                            sources: entry.sources.into_iter().map(Into::into).collect(),
                        })
                        .collect(),
                    ungradable: item.ungradable.into_iter().map(Into::into).collect(),
                })
                .collect(),
            combined_candidates: self
                .combined_candidates
                .into_iter()
                .map(Into::into)
                .collect(),
            combined_ungradable: self
                .combined_ungradable
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }
}

impl From<StrictPhysicalEntry> for AddressedPhysicalEntryV2 {
    fn from(value: StrictPhysicalEntry) -> Self {
        Self {
            rom_space: value.rom_space,
            rom: value.rom,
            vram: value.vram,
        }
    }
}

impl From<StrictBankAddr> for BankAddr {
    fn from(value: StrictBankAddr) -> Self {
        Self {
            bank: value.bank,
            pc: value.pc,
        }
    }
}

fn validate_candidate_order(
    candidates: &ScopedCandidateIdentitiesV3,
) -> Result<(), SnapshotWorkspaceError> {
    let detectors: Vec<_> = candidates
        .per_detector
        .iter()
        .map(|entry| entry.detector)
        .collect();
    if detectors
        != [
            CandidateDetector::HardwareEntrypoint,
            CandidateDetector::JalTarget,
            CandidateDetector::IndirectCallTarget,
            CandidateDetector::SemanticCallableArgument,
            CandidateDetector::ProloguePattern,
            CandidateDetector::ArgumentHomeSpillLeaf,
            CandidateDetector::TableDerived,
        ]
    {
        return Err(SnapshotWorkspaceError(
            "candidate receipt does not contain the canonical detector denominator".into(),
        ));
    }
    strictly_sorted_by(
        &candidates.per_detector,
        |entry| entry.detector,
        "detector list",
    )?;
    strictly_sorted(&candidates.combined_candidates, "combined candidates")?;
    strictly_sorted(
        &candidates.combined_ungradable,
        "combined ungradable candidates",
    )?;
    let mut detector_candidates = BTreeSet::new();
    let mut detector_ungradable = BTreeSet::new();
    for detector in &candidates.per_detector {
        strictly_sorted(&detector.candidates, "detector candidates")?;
        strictly_sorted_by(
            &detector.provenance,
            |entry| entry.candidate,
            "candidate provenance",
        )?;
        strictly_sorted(&detector.ungradable, "detector ungradable candidates")?;
        for candidate in &detector.candidates {
            validate_candidate_pc(candidate, "detector candidate")?;
            detector_candidates.insert(*candidate);
        }
        for ungradable in &detector.ungradable {
            validate_ungradable_pc(ungradable, "detector ungradable candidate")?;
            detector_ungradable.insert(ungradable.clone());
        }
        for provenance in &detector.provenance {
            strictly_sorted(&provenance.sources, "candidate provenance sources")?;
            validate_candidate_pc(&provenance.candidate, "candidate provenance target")?;
            if detector
                .candidates
                .binary_search(&provenance.candidate)
                .is_err()
            {
                return Err(SnapshotWorkspaceError(
                    "candidate provenance target is absent from its detector population".into(),
                ));
            }
            for source in &provenance.sources {
                validate_candidate_pc(source, "candidate provenance source")?;
            }
        }
    }

    // Combined entries are the positive merged conclusions. A conflict or
    // rejection can remove a raw detector claim, so these are canonical
    // subsets—not necessarily the union—of the per-detector populations.
    for candidate in &candidates.combined_candidates {
        validate_candidate_pc(candidate, "combined candidate")?;
        if !detector_candidates.contains(candidate) {
            return Err(SnapshotWorkspaceError(
                "combined candidate is absent from every detector population".into(),
            ));
        }
    }
    for ungradable in &candidates.combined_ungradable {
        validate_ungradable_pc(ungradable, "combined ungradable candidate")?;
        if !detector_ungradable.contains(ungradable) {
            return Err(SnapshotWorkspaceError(
                "combined ungradable candidate is absent from every detector population".into(),
            ));
        }
    }
    Ok(())
}

fn validate_candidate_pc(
    candidate: &AddressedPhysicalEntryV2,
    label: &str,
) -> Result<(), SnapshotWorkspaceError> {
    if !candidate.vram.is_multiple_of(4) {
        return Err(SnapshotWorkspaceError(format!(
            "{label} PC is not instruction-aligned"
        )));
    }
    Ok(())
}

fn validate_ungradable_pc(candidate: &BankAddr, label: &str) -> Result<(), SnapshotWorkspaceError> {
    validate_bank_token(&candidate.bank)?;
    if !candidate.pc.is_multiple_of(4) {
        return Err(SnapshotWorkspaceError(format!(
            "{label} PC is not instruction-aligned"
        )));
    }
    Ok(())
}

fn strictly_sorted<T: Ord>(items: &[T], label: &str) -> Result<(), SnapshotWorkspaceError> {
    if items.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(SnapshotWorkspaceError(format!("{label} is not canonical")));
    }
    Ok(())
}

fn strictly_sorted_by<T, K: Ord + Copy>(
    items: &[T],
    key: impl Fn(&T) -> K,
    label: &str,
) -> Result<(), SnapshotWorkspaceError> {
    if items.windows(2).any(|pair| key(&pair[0]) >= key(&pair[1])) {
        return Err(SnapshotWorkspaceError(format!("{label} is not canonical")));
    }
    Ok(())
}

fn verify_namespace(
    root: &Path,
    manifest: &WorkspaceManifest,
) -> Result<(), SnapshotWorkspaceError> {
    let mut expected = BTreeSet::from([MANIFEST_NAME.to_string(), CANDIDATE_NAME.to_string()]);
    for bank in &manifest.banks {
        expected.insert(bank.bank_artifact.clone());
        expected.insert(bank.snapshot_artifact.clone());
    }
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(root)
        .map_err(|error| SnapshotWorkspaceError(format!("reading workspace namespace: {error}")))?
    {
        let entry = entry.map_err(|error| {
            SnapshotWorkspaceError(format!("reading workspace namespace entry: {error}"))
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            SnapshotWorkspaceError("workspace contains a non-UTF-8 filename".into())
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            SnapshotWorkspaceError(format!("inspecting workspace entry {name}: {error}"))
        })?;
        if !metadata.file_type().is_file() {
            return Err(SnapshotWorkspaceError(format!(
                "workspace entry {name:?} is not a regular file"
            )));
        }
        actual.insert(name);
    }
    if actual != expected {
        return Err(SnapshotWorkspaceError(
            "workspace contains missing or unmanifested files".into(),
        ));
    }
    Ok(())
}

fn load_bound_artifact(
    path: &Path,
    expected_length: u64,
    expected_sha256: Sha256Digest,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, SnapshotWorkspaceError> {
    if expected_length > limit {
        return Err(SnapshotWorkspaceError(format!("{label} exceeds its bound")));
    }
    let bytes = read_private_regular_bounded(path, limit, label)?;
    if bytes.len() as u64 != expected_length || Sha256Digest::of(&bytes) != expected_sha256 {
        return Err(SnapshotWorkspaceError(format!(
            "{label} length or SHA-256 does not match its receipt"
        )));
    }
    Ok(bytes)
}

fn read_private_regular_bounded(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, SnapshotWorkspaceError> {
    let inspected = fs::symlink_metadata(path)
        .map_err(|error| SnapshotWorkspaceError(format!("inspecting {label}: {error}")))?;
    if !inspected.file_type().is_file() {
        return Err(SnapshotWorkspaceError(format!(
            "{label} is not a regular file"
        )));
    }
    #[cfg(unix)]
    if inspected.mode() & 0o777 != 0o600 || inspected.nlink() != 1 {
        return Err(SnapshotWorkspaceError(format!(
            "{label} must be private mode 0600 with one link"
        )));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| SnapshotWorkspaceError(format!("opening {label}: {error}")))?;
    let opened = file
        .metadata()
        .map_err(|error| SnapshotWorkspaceError(format!("inspecting open {label}: {error}")))?;
    if !opened.is_file() || opened.len() > limit {
        return Err(SnapshotWorkspaceError(format!("{label} exceeds its bound")));
    }
    #[cfg(unix)]
    if opened.dev() != inspected.dev()
        || opened.ino() != inspected.ino()
        || opened.mode() & 0o777 != 0o600
        || opened.nlink() != 1
    {
        return Err(SnapshotWorkspaceError(format!(
            "{label} identity or privacy changed during open"
        )));
    }
    let allocation = usize::try_from(opened.len().min(limit))
        .map_err(|_| SnapshotWorkspaceError(format!("{label} length does not fit memory")))?;
    let mut bytes = Vec::with_capacity(allocation);
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| SnapshotWorkspaceError(format!("reading {label}: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(SnapshotWorkspaceError(format!("{label} exceeds its bound")));
    }
    let after = file
        .metadata()
        .map_err(|error| SnapshotWorkspaceError(format!("rechecking {label}: {error}")))?;
    #[cfg(unix)]
    if after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || after.len() != opened.len()
        || after.mtime() != opened.mtime()
        || after.mtime_nsec() != opened.mtime_nsec()
        || after.mode() & 0o777 != 0o600
        || after.nlink() != 1
    {
        return Err(SnapshotWorkspaceError(format!(
            "{label} changed while it was read"
        )));
    }
    Ok(bytes)
}

fn parse_digest(value: &str, label: &str) -> Result<Sha256Digest, SnapshotWorkspaceError> {
    Sha256Digest::from_hex(value)
        .map_err(|error| SnapshotWorkspaceError(format!("invalid {label}: {error}")))
}

fn checked_add(left: u64, right: u64, label: &str) -> Result<u64, SnapshotWorkspaceError> {
    left.checked_add(right)
        .ok_or_else(|| SnapshotWorkspaceError(format!("{label} length overflow")))
}

fn validate_bank_token(bank: &str) -> Result<(), SnapshotWorkspaceError> {
    if bank.is_empty()
        || bank.len() > 256
        || bank == "."
        || bank == ".."
        || bank
            .bytes()
            .any(|byte| byte == 0 || byte == b'/' || byte == b'\\')
    {
        return Err(SnapshotWorkspaceError(
            "invalid bank name in receipt".into(),
        ));
    }
    Ok(())
}

fn bank_artifact_name(index: usize) -> String {
    format!("bank-{index:06}.bin")
}

fn snapshot_artifact_name(index: usize) -> String {
    format!("bank-{index:06}.snapshot.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, Fact, FactDb, FunctionEntryEvidence,
        ProloguePattern, ProofState,
    };
    use crate::normalize;
    use crate::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    const BASE: u32 = 0x8000_0400;

    struct Fixture {
        root: PathBuf,
        workspace: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn fixture_snapshot() -> (ProgramSnapshotV1, Vec<u8>) {
        let mut rom_bytes = vec![0u8; 0x1020];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        rom_bytes[0x1000..0x1004].copy_from_slice(&0x03e0_0008u32.to_be_bytes());
        let rom = normalize(&rom_bytes).unwrap();
        let bank_bytes = rom.bytes[0x1000..].to_vec();
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank_a".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: BASE,
            va_end: BASE + 0x20,
        });
        facts
            .conclude("bank:bank_a", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank_a".into(),
            va_start: BASE,
            va_end: BASE + 0x20,
        });
        facts
            .conclude(
                executable_range_subject("bank_a", BASE, BASE + 0x20),
                ProofState::Proven,
                vec![executable],
                "test",
            )
            .unwrap();
        let target = BankAddr::new("bank_a", BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("bank_a", BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "test",
            )
            .unwrap();
        let snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank_a",
                va_start: BASE,
                bytes: &bank_bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        (snapshot, bank_bytes)
    }

    fn empty_candidates() -> ScopedCandidateIdentitiesV3 {
        ScopedCandidateIdentitiesV3 {
            schema_version: 3,
            per_detector: [
                CandidateDetector::HardwareEntrypoint,
                CandidateDetector::JalTarget,
                CandidateDetector::IndirectCallTarget,
                CandidateDetector::SemanticCallableArgument,
                CandidateDetector::ProloguePattern,
                CandidateDetector::ArgumentHomeSpillLeaf,
                CandidateDetector::TableDerived,
            ]
            .into_iter()
            .map(|detector| DetectorCandidateIdentitiesV2 {
                detector,
                candidates: Vec::new(),
                provenance: Vec::new(),
                ungradable: Vec::new(),
            })
            .collect(),
            combined_candidates: Vec::new(),
            combined_ungradable: Vec::new(),
        }
    }

    fn physical_entry(rom: u32, vram: u32) -> AddressedPhysicalEntryV2 {
        AddressedPhysicalEntryV2 {
            rom_space: RomAddressSpace::Physical,
            rom,
            vram,
        }
    }

    fn bank_addr(pc: u32) -> BankAddr {
        BankAddr::new("bank_a", pc)
    }

    fn rewrite_manifest(fixture: &Fixture, mutate: impl FnOnce(&mut serde_json::Value)) {
        let path = fixture.workspace.join(MANIFEST_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutate(&mut manifest);
        let mut bytes = serde_json::to_vec(&manifest).unwrap();
        bytes.push(b'\n');
        write_private(&path, &bytes);
    }

    fn valid_fixture() -> Fixture {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = (0..128u32)
            .find_map(|attempt| {
                let candidate = std::env::temp_dir().join(format!(
                    "fn64-snapshot-workspace-validator-{}-{nonce}-{attempt}",
                    std::process::id()
                ));
                match fs::create_dir(&candidate) {
                    Ok(()) => Some(candidate),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("creating test fixture directory: {error}"),
                }
            })
            .expect("could not allocate a unique test fixture directory");
        let workspace = root.join("workspace");
        fs::create_dir(&workspace).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();

        let (snapshot, bank_bytes) = fixture_snapshot();
        let mut snapshot_bytes = serde_json::to_vec(&snapshot).unwrap();
        snapshot_bytes.push(b'\n');
        write_private(&workspace.join("bank-000000.bin"), &bank_bytes);
        write_private(
            &workspace.join("bank-000000.snapshot.json"),
            &snapshot_bytes,
        );
        let candidates = empty_candidates();
        let mut candidate_bytes = serde_json::to_vec(&candidates).unwrap();
        candidate_bytes.push(b'\n');
        write_private(&workspace.join(CANDIDATE_NAME), &candidate_bytes);

        let input = &snapshot.banks[0].input;
        let manifest = serde_json::json!({
            "schema": "fn64.snapshot-workspace",
            "schema_version": 3,
            "state": "composed",
            "open_reason": null,
            "normalized_rom_sha256": snapshot.normalized_rom_sha256,
            "discovery": {
                "selected": "boot_bank_only",
                "outcomes": [
                    {"strategy":"boot_bank_only","candidate_tables":0,"admitted_tables":0,"admitted_intervals":0,"decoded_file_limit_hits":0,"proven_mappings":1,"supported_mappings":0,"request_dma_open_rows":0,"request_dma_incomplete":false,"request_dma_input_limit_hit":false,"physical_wrapper_candidates_examined":0,"wrapper_semantic_proof_unavailable":0,"physical_wrapper_candidate_limit_hit":false},
                    {"strategy":"recovered_vrom","candidate_tables":0,"admitted_tables":0,"admitted_intervals":0,"decoded_file_limit_hits":0,"proven_mappings":1,"supported_mappings":0,"request_dma_open_rows":0,"request_dma_incomplete":false,"request_dma_input_limit_hit":false,"physical_wrapper_candidates_examined":0,"wrapper_semantic_proof_unavailable":0,"physical_wrapper_candidate_limit_hit":false},
                    {"strategy":"recovered_overlays","candidate_tables":0,"admitted_tables":0,"admitted_intervals":0,"decoded_file_limit_hits":0,"proven_mappings":1,"supported_mappings":0,"request_dma_open_rows":0,"request_dma_incomplete":false,"request_dma_input_limit_hit":false,"physical_wrapper_candidates_examined":0,"wrapper_semantic_proof_unavailable":0,"physical_wrapper_candidate_limit_hit":false},
                    {"strategy":"untabled_delta_vote","candidate_tables":0,"admitted_tables":0,"admitted_intervals":0,"decoded_file_limit_hits":0,"proven_mappings":1,"supported_mappings":0,"request_dma_open_rows":0,"request_dma_incomplete":false,"request_dma_input_limit_hit":false,"physical_wrapper_candidates_examined":0,"wrapper_semantic_proof_unavailable":0,"physical_wrapper_candidate_limit_hit":false}
                ]
            },
            "limits": {
                "max_rom_bytes":64*MIB,"max_banks":MAX_BANKS,
                "max_snapshot_artifact_bytes":MAX_SNAPSHOT_ARTIFACT_BYTES,
                "max_aggregate_snapshot_artifact_bytes":MAX_AGGREGATE_SNAPSHOT_ARTIFACT_BYTES,
                "max_discovery_decoded_vrom_file_bytes":64*MIB,
                "max_preparation_decoded_vrom_file_bytes":64*MIB,
                "max_projected_fact_rows":4_000_000,
                "max_projected_fact_bytes":256*MIB,
                "max_aggregate_materialized_bytes":MAX_AGGREGATE_BANK_BYTES,
                "max_cross_bank_authority_records":1_048_576
            },
            "snapshot_wire":{"schema_version":5,"authority":"diagnostic_only","duplicates_fact_db_per_bank":false,"remaining_large_rom_frontier":"streaming_v5"},
            "aggregate_snapshot_artifact_bytes":snapshot_bytes.len(),
            "rom_recompilation_complete":false,
            "remaining_recompilation_frontier":"proven_bank_and_callable_owner_closure",
            "intended_use":"sealed_cold_function_training_input",
            "cold_training":{
                "schema_version":3,"algorithm":"fn64.cold-function-training.v3","answer_key_present":false,
                "candidate_artifact":CANDIDATE_NAME,
                "candidate_artifact_byte_length":candidate_bytes.len(),
                "candidate_artifact_sha256":Sha256Digest::of(&candidate_bytes),
                "scoped_candidate_identities_v3_sha256":candidates.digest_sha256()
            },
            "banks":[{
                "index":0,"bank":"bank_a","rom_space":"Physical",
                "rom_start":input.rom_start,"rom_end":input.rom_end,
                "va_start":input.va_start,"va_end":input.va_end,
                "byte_length":bank_bytes.len(),"backing_evidence_fact_indices":[],
                "bank_sha256":Sha256Digest::of(&bank_bytes),
                "bank_artifact":"bank-000000.bin","snapshot_artifact":"bank-000000.snapshot.json",
                "snapshot_artifact_byte_length":snapshot_bytes.len(),
                "snapshot_artifact_sha256":Sha256Digest::of(&snapshot_bytes),
                "program_snapshot_sha256":program_snapshot_sha256_v2(&snapshot).unwrap(),
                "ghidra_seeds":{"mode":"discovery_only","role":"candidate_only"}
            }]
        });
        let mut manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        write_private(&workspace.join(MANIFEST_NAME), &manifest_bytes);
        Fixture { root, workspace }
    }

    fn open_fixture() -> Fixture {
        let fixture = valid_fixture();
        fs::remove_file(fixture.workspace.join("bank-000000.bin")).unwrap();
        fs::remove_file(fixture.workspace.join("bank-000000.snapshot.json")).unwrap();
        let manifest_path = fixture.workspace.join(MANIFEST_NAME);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["state"] = serde_json::json!("open");
        manifest["open_reason"] = serde_json::json!("no_proven_banks");
        manifest["aggregate_snapshot_artifact_bytes"] = serde_json::json!(0);
        manifest["banks"] = serde_json::json!([]);
        manifest["discovery"]["selected"] = serde_json::json!("boot_bank_open");
        manifest["discovery"]["outcomes"][0]["strategy"] = serde_json::json!("boot_bank_open");
        for outcome in manifest["discovery"]["outcomes"].as_array_mut().unwrap() {
            outcome["proven_mappings"] = serde_json::json!(0);
        }
        let mut bytes = serde_json::to_vec(&manifest).unwrap();
        bytes.push(b'\n');
        write_private(&manifest_path, &bytes);
        fixture
    }

    #[test]
    fn legacy_and_selected_manifests_are_not_training_inputs() {
        let legacy = serde_json::json!({
            "schema": "fn64.snapshot-workspace",
            "schema_version": 1,
            "state": "composed",
            "open_reason": null,
            "normalized_rom_sha256": "00".repeat(32),
            "discovery": {"selected":"boot_bank_only","outcomes":[]},
            "limits": {}, "snapshot_wire": {},
            "aggregate_snapshot_artifact_bytes": 0,
            "rom_recompilation_complete": false,
            "remaining_recompilation_frontier": "proven_bank_and_callable_owner_closure",
            "intended_use": "candidate_ghidra_only", "banks": []
        });
        assert!(serde_json::from_value::<WorkspaceManifest>(legacy).is_err());
    }

    #[test]
    fn request_dma_open_frontier_round_trips_and_must_be_self_consistent() {
        let fixture = valid_fixture();
        rewrite_manifest(&fixture, |manifest| {
            let outcome = &mut manifest["discovery"]["outcomes"][1];
            outcome["request_dma_open_rows"] = serde_json::json!(1);
            outcome["request_dma_incomplete"] = serde_json::json!(true);
            outcome["physical_wrapper_candidates_examined"] = serde_json::json!(1);
            outcome["wrapper_semantic_proof_unavailable"] = serde_json::json!(1);
        });
        validate_snapshot_workspace(&fixture.workspace)
            .expect("candidate-only wrapper frontier remains explicit in the sealed manifest");

        rewrite_manifest(&fixture, |manifest| {
            manifest["discovery"]["outcomes"][1]["request_dma_incomplete"] =
                serde_json::json!(false);
        });
        let error = match validate_snapshot_workspace(&fixture.workspace) {
            Ok(_) => panic!("an open request-DMA row cannot be serialized as complete"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("discovery outcome exceeds structural bounds"));
    }

    #[test]
    fn candidate_receipt_rejects_unknown_nested_fields_and_noncanonical_order() {
        let unknown = br#"{"schema_version":3,"per_detector":[],"combined_candidates":[{"rom_space":"Physical","rom":1,"vram":2,"extra":3}],"combined_ungradable":[]}"#;
        assert!(serde_json::from_slice::<StrictScopedCandidates>(unknown).is_err());

        let mut receipt = empty_candidates();
        receipt.combined_candidates = vec![
            AddressedPhysicalEntryV2 {
                rom_space: RomAddressSpace::Physical,
                rom: 2,
                vram: 2,
            },
            AddressedPhysicalEntryV2 {
                rom_space: RomAddressSpace::Physical,
                rom: 1,
                vram: 1,
            },
        ];
        assert!(validate_candidate_order(&receipt).is_err());
    }

    #[test]
    fn manifest_requires_equal_checked_rom_and_va_spans() {
        for (rom_start, rom_end) in [(0x1000, 0x101c), (0x1020, 0x1000)] {
            let fixture = valid_fixture();
            rewrite_manifest(&fixture, |manifest| {
                manifest["banks"][0]["rom_start"] = serde_json::json!(rom_start);
                manifest["banks"][0]["rom_end"] = serde_json::json!(rom_end);
            });
            let error = validate_snapshot_workspace(&fixture.workspace)
                .err()
                .unwrap();
            assert!(error.to_string().contains("inconsistent geometry"));
        }
    }

    #[test]
    fn candidate_receipt_requires_provenance_target_in_its_detector() {
        let mut receipt = empty_candidates();
        receipt.per_detector[3].provenance = vec![CandidatePhysicalProvenanceV2 {
            candidate: physical_entry(0x1000, BASE),
            sources: vec![physical_entry(0x1004, BASE + 4)],
        }];
        let error = validate_candidate_order(&receipt).unwrap_err();
        assert!(error.to_string().contains("absent from its detector"));
    }

    #[test]
    fn combined_populations_must_come_from_a_detector_population() {
        let mut translated = empty_candidates();
        translated.combined_candidates = vec![physical_entry(0x1000, BASE)];
        let error = validate_candidate_order(&translated).unwrap_err();
        assert!(error.to_string().contains("absent from every detector"));

        let mut ungradable = empty_candidates();
        ungradable.combined_ungradable = vec![bank_addr(BASE)];
        let error = validate_candidate_order(&ungradable).unwrap_err();
        assert!(error.to_string().contains("absent from every detector"));
    }

    #[test]
    fn candidate_receipt_requires_instruction_aligned_pcs() {
        let mut detector_target = empty_candidates();
        detector_target.per_detector[3].candidates = vec![physical_entry(0x1000, BASE + 2)];
        assert!(validate_candidate_order(&detector_target)
            .unwrap_err()
            .to_string()
            .contains("instruction-aligned"));

        let mut provenance_source = empty_candidates();
        let candidate = physical_entry(0x1000, BASE);
        provenance_source.per_detector[3].candidates = vec![candidate];
        provenance_source.per_detector[3].provenance = vec![CandidatePhysicalProvenanceV2 {
            candidate,
            sources: vec![physical_entry(0x1004, BASE + 2)],
        }];
        assert!(validate_candidate_order(&provenance_source)
            .unwrap_err()
            .to_string()
            .contains("instruction-aligned"));

        let mut ungradable = empty_candidates();
        ungradable.per_detector[3].ungradable = vec![bank_addr(BASE + 2)];
        assert!(validate_candidate_order(&ungradable)
            .unwrap_err()
            .to_string()
            .contains("instruction-aligned"));
    }

    #[test]
    fn merged_candidate_populations_may_be_canonical_subsets() {
        let mut receipt = empty_candidates();
        let accepted = physical_entry(0x1000, BASE);
        let rejected = physical_entry(0x1004, BASE + 4);
        receipt.per_detector[3].candidates = vec![accepted, rejected];
        receipt.combined_candidates = vec![accepted];
        receipt.per_detector[3].ungradable = vec![bank_addr(BASE + 8), bank_addr(BASE + 12)];
        receipt.combined_ungradable = vec![bank_addr(BASE + 8)];
        validate_candidate_order(&receipt).unwrap();
    }

    #[test]
    fn full_validation_precedes_streaming_visitors_and_preserves_order() {
        let fixture = valid_fixture();
        let mut candidate_visits = 0;
        let mut streamed_order = Vec::new();
        let streamed_identity = validate_snapshot_workspace_streaming(
            &fixture.workspace,
            |receipt| {
                candidate_visits += 1;
                assert_eq!(receipt.per_detector.len(), 7);
                Ok(())
            },
            |bank| {
                streamed_order.push((bank.index, bank.bank.to_string(), bank.bytes.len()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(candidate_visits, 1);
        assert_eq!(streamed_order, [(0, "bank_a".to_string(), 0x20)]);
        assert_eq!(streamed_identity.bank_count, 1);

        let validated = validate_snapshot_workspace(&fixture.workspace).unwrap();
        assert_eq!(validated.identity().bank_count, 1);
        assert_eq!(
            validated.identity().state,
            ValidatedWorkspaceState::Composed
        );
        let candidate_count = validated
            .visit_candidate_identities(|receipt| receipt.per_detector.len())
            .unwrap();
        assert_eq!(candidate_count, 7);
        let mut order = Vec::new();
        validated
            .visit_banks(|bank| {
                order.push((bank.index, bank.bank.to_string(), bank.bytes.len()));
                assert_eq!(bank.snapshot.banks.len(), 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(order, [(0, "bank_a".to_string(), 0x20)]);
    }

    #[test]
    fn post_validation_artifact_change_is_rejected_before_visit() {
        let fixture = valid_fixture();
        let validated = validate_snapshot_workspace(&fixture.workspace).unwrap();
        let bank = fixture.workspace.join("bank-000000.bin");
        write_private(&bank, b"tampered");
        let mut called = false;
        assert!(validated
            .visit_banks(|_| {
                called = true;
                Ok(())
            })
            .is_err());
        assert!(!called);
    }

    #[test]
    fn exact_training_open_workspace_has_zero_bank_visits() {
        let fixture = open_fixture();
        let mut candidate_visits = 0;
        let mut bank_visits = 0;
        let identity = validate_snapshot_workspace_streaming(
            &fixture.workspace,
            |_| {
                candidate_visits += 1;
                Ok(())
            },
            |_| {
                bank_visits += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(candidate_visits, 1);
        assert_eq!(bank_visits, 0);
        assert_eq!(identity.state, ValidatedWorkspaceState::OpenNoProvenBanks);
        assert_eq!(identity.bank_count, 0);
    }

    #[test]
    fn unmanifested_file_is_rejected_before_any_visitor() {
        let fixture = valid_fixture();
        write_private(&fixture.workspace.join("answer-key.toml"), b"forbidden");
        let mut called = false;
        assert!(validate_snapshot_workspace_streaming(
            &fixture.workspace,
            |_| {
                called = true;
                Ok(())
            },
            |_| Ok(())
        )
        .is_err());
        assert!(!called);
    }

    #[test]
    fn fixed_artifact_names_are_consecutive_and_unambiguous() {
        assert_eq!(bank_artifact_name(7), "bank-000007.bin");
        assert_eq!(snapshot_artifact_name(7), "bank-000007.snapshot.json");
        assert!(validate_bank_token("overlay_7").is_ok());
        assert!(validate_bank_token("../overlay").is_err());
    }
}
