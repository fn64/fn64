//! Snapshot-bound, candidate-only external-tool claim sidecar.
//!
//! Native proof remains entirely inside [`crate::snapshot::ProgramSnapshotV1`]
//! and its [`crate::facts::FactDb`]. External tools are frozen beside that
//! artifact, never into it: this avoids both a self-referential snapshot
//! digest and accidental promotion through generic native conclusion keys.

use crate::facts::BankBackingSpanV1;
use crate::snapshot::{BankInputDigestV1, ProgramSnapshotV1, PROGRAM_SNAPSHOT_SCHEMA_V6};
use crate::tool_adapter::{
    recompute_tool_run_source_sha256, BankInputIdentity, CandidateProofCeiling, Sha256Digest,
    ToolAdapterOutput, ToolCandidateKind, ToolLineageRef, ToolLineageRole, ToolRunRole,
    ToolRunSource, TOOL_ADAPTER_SCHEMA_VERSION, TOOL_ADAPTER_SCHEMA_VERSION_V2,
    TOOL_ADAPTER_SCHEMA_VERSION_V3,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const TOOL_CLAIM_SET_SCHEMA: &str = "fn64.tool-claim-set";
pub const TOOL_CLAIM_SET_SCHEMA_VERSION_V1: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolClaimObservationV1 {
    /// Digest of the complete provider record stream. This preserves the
    /// exact provider-ID/sequence pairing that `ToolRunSource` intentionally
    /// excludes from its semantic digest.
    pub claim_records_sha256: Sha256Digest,
    pub provider_claim_ids: Vec<String>,
    pub source_sequences: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalToolClaimV1 {
    pub claim_id: Sha256Digest,
    pub source_sha256: Sha256Digest,
    pub kind: ToolCandidateKind,
    pub proof_ceiling: CandidateProofCeiling,
    pub observations: Vec<ToolClaimObservationV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolClaimSetV1 {
    pub schema: String,
    pub schema_version: u32,
    pub program_snapshot_sha256: Sha256Digest,
    pub sources: Vec<ToolRunSource>,
    pub claims: Vec<CanonicalToolClaimV1>,
}

impl ToolClaimSetV1 {
    /// Disable one source and every claim derived solely from it. Native
    /// snapshot facts are not reachable from this operation by construction.
    pub fn without_source(&self, source: Sha256Digest) -> Self {
        Self {
            schema: self.schema.clone(),
            schema_version: self.schema_version,
            program_snapshot_sha256: self.program_snapshot_sha256,
            sources: self
                .sources
                .iter()
                .filter(|item| item.source_sha256 != source)
                .cloned()
                .collect(),
            claims: self
                .claims
                .iter()
                .filter(|claim| claim.source_sha256 != source)
                .cloned()
                .collect(),
        }
    }

    /// Exact claim lookup over the canonical claim-ID index.
    pub fn claim(&self, id: Sha256Digest) -> Option<&CanonicalToolClaimV1> {
        self.claims
            .binary_search_by_key(&id, |claim| claim.claim_id)
            .ok()
            .map(|index| &self.claims[index])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolClaimIngestError {
    InvalidClaimSetSchema {
        schema: String,
        version: u32,
    },
    UnsupportedSnapshotSchema(u32),
    SnapshotSerialization(String),
    InvalidSnapshotDigest,
    UnknownBank(String),
    DuplicateBank(String),
    InvalidBankDigest {
        bank: String,
    },
    SourceSchemaMismatch {
        source: Sha256Digest,
        version: u32,
    },
    SourceInputMismatch {
        source: Sha256Digest,
    },
    MissingSnapshotLineage {
        source: Sha256Digest,
    },
    AmbiguousSnapshotLineage {
        source: Sha256Digest,
        count: usize,
    },
    StaleSnapshotLineage {
        source: Sha256Digest,
    },
    WrongRole {
        source: Sha256Digest,
        role: ToolRunRole,
    },
    ProofCeilingViolation {
        source: Sha256Digest,
    },
    SourceDigestCollision {
        source: Sha256Digest,
    },
    ClaimDigestCollision {
        claim: Sha256Digest,
    },
    SnapshotBindingMismatch,
    NonCanonicalSources,
    NonCanonicalClaims,
    OrphanClaim {
        claim: Sha256Digest,
    },
    InvalidObservations {
        claim: Sha256Digest,
    },
    InvalidSourceDigest {
        source: Sha256Digest,
    },
    InvalidClaimShape {
        claim: Sha256Digest,
    },
}

impl std::fmt::Display for ToolClaimIngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ToolClaimIngestError {}

pub fn program_snapshot_sha256_v3(
    snapshot: &ProgramSnapshotV1,
) -> Result<Sha256Digest, ToolClaimIngestError> {
    if snapshot.schema_version != PROGRAM_SNAPSHOT_SCHEMA_V6 {
        return Err(ToolClaimIngestError::UnsupportedSnapshotSchema(
            snapshot.schema_version,
        ));
    }
    let bytes = serde_json::to_vec(snapshot)
        .map_err(|error| ToolClaimIngestError::SnapshotSerialization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.program-snapshot.v3\0");
    hasher.update(bytes);
    Ok(Sha256Digest(hasher.finalize().into()))
}

pub fn bank_input_identity_v1(
    snapshot: &ProgramSnapshotV1,
    bank: &str,
) -> Result<BankInputIdentity, ToolClaimIngestError> {
    let matches: Vec<_> = snapshot
        .banks
        .iter()
        .filter(|item| item.input.bank == bank)
        .collect();
    let input = match matches.as_slice() {
        [] => return Err(ToolClaimIngestError::UnknownBank(bank.into())),
        [input] => &input.input,
        _ => return Err(ToolClaimIngestError::DuplicateBank(bank.into())),
    };
    let normalized_rom_sha256 = Sha256Digest::from_hex(&snapshot.normalized_rom_sha256)
        .map_err(|_| ToolClaimIngestError::InvalidSnapshotDigest)?;
    let bank_bytes_sha256 = Sha256Digest::from_hex(&input.bytes_sha256)
        .map_err(|_| ToolClaimIngestError::InvalidBankDigest { bank: bank.into() })?;
    Ok(BankInputIdentity {
        normalized_rom_sha256,
        bank: bank.into(),
        bank_bytes_sha256,
        mapping_sha256: snapshot_bank_mapping_sha256_v2(input),
        va_start: input.va_start,
        va_end: input.va_end,
    })
}

pub fn discovery_snapshot_lineage_v3(
    snapshot: &ProgramSnapshotV1,
) -> Result<ToolLineageRef, ToolClaimIngestError> {
    Ok(ToolLineageRef {
        role: ToolLineageRole::DiscoverySnapshot,
        source_sha256: program_snapshot_sha256_v3(snapshot)?,
    })
}

pub fn freeze_tool_claims_v1<'a>(
    snapshot: &ProgramSnapshotV1,
    runs: impl IntoIterator<Item = &'a ToolAdapterOutput>,
) -> Result<ToolClaimSetV1, ToolClaimIngestError> {
    let snapshot_digest = program_snapshot_sha256_v3(snapshot)?;
    let mut sources = BTreeMap::<Sha256Digest, ToolRunSource>::new();
    let mut claims = BTreeMap::<Sha256Digest, CanonicalToolClaimV1>::new();

    for run in runs {
        let source = run.source();
        let source_digest = source.source_sha256;
        validate_source_binding(snapshot, snapshot_digest, source)?;
        if let Some(existing) = sources.get(&source_digest) {
            if existing != source {
                return Err(ToolClaimIngestError::SourceDigestCollision {
                    source: source_digest,
                });
            }
        } else {
            sources.insert(source_digest, source.clone());
        }

        for candidate in run.candidates() {
            if candidate.proof_ceiling != CandidateProofCeiling::Candidate {
                return Err(ToolClaimIngestError::ProofCeilingViolation {
                    source: source_digest,
                });
            }
            if !role_accepts(&source.role, &candidate.kind) {
                return Err(ToolClaimIngestError::WrongRole {
                    source: source_digest,
                    role: source.role.clone(),
                });
            }
            let claim_id = tool_claim_id_v1(source_digest, &candidate.kind)?;
            let observation = ToolClaimObservationV1 {
                claim_records_sha256: run.summary().claims_sha256,
                provider_claim_ids: canonical_strings(&candidate.provider_claim_ids),
                source_sequences: canonical_u64s(&candidate.source_sequences),
            };
            if let Some(existing) = claims.get_mut(&claim_id) {
                if existing.source_sha256 != source_digest || existing.kind != candidate.kind {
                    return Err(ToolClaimIngestError::ClaimDigestCollision { claim: claim_id });
                }
                existing.observations.push(observation);
                existing.observations.sort();
                existing.observations.dedup();
            } else {
                claims.insert(
                    claim_id,
                    CanonicalToolClaimV1 {
                        claim_id,
                        source_sha256: source_digest,
                        kind: candidate.kind.clone(),
                        proof_ceiling: CandidateProofCeiling::Candidate,
                        observations: vec![observation],
                    },
                );
            }
        }
    }

    let set = ToolClaimSetV1 {
        schema: TOOL_CLAIM_SET_SCHEMA.into(),
        schema_version: TOOL_CLAIM_SET_SCHEMA_VERSION_V1,
        program_snapshot_sha256: snapshot_digest,
        sources: sources.into_values().collect(),
        claims: claims.into_values().collect(),
    };
    validate_tool_claim_set_v1(snapshot, &set)?;
    Ok(set)
}

/// Validate a serialized sidecar before admitting it to snapshot/pack
/// queries. Deserialization alone is deliberately insufficient: every
/// canonical digest and bank-local constraint is recomputed here.
pub fn validate_tool_claim_set_v1(
    snapshot: &ProgramSnapshotV1,
    set: &ToolClaimSetV1,
) -> Result<(), ToolClaimIngestError> {
    if set.schema != TOOL_CLAIM_SET_SCHEMA || set.schema_version != TOOL_CLAIM_SET_SCHEMA_VERSION_V1
    {
        return Err(ToolClaimIngestError::InvalidClaimSetSchema {
            schema: set.schema.clone(),
            version: set.schema_version,
        });
    }
    let snapshot_digest = program_snapshot_sha256_v3(snapshot)?;
    if set.program_snapshot_sha256 != snapshot_digest {
        return Err(ToolClaimIngestError::SnapshotBindingMismatch);
    }
    if !strictly_sorted_unique_by(&set.sources, |source| source.source_sha256) {
        return Err(ToolClaimIngestError::NonCanonicalSources);
    }
    if !strictly_sorted_unique_by(&set.claims, |claim| claim.claim_id) {
        return Err(ToolClaimIngestError::NonCanonicalClaims);
    }

    let sources: BTreeMap<_, _> = set
        .sources
        .iter()
        .map(|source| (source.source_sha256, source))
        .collect();
    for source in &set.sources {
        validate_source_binding(snapshot, snapshot_digest, source)?;
        if !strictly_sorted_unique(&source.lineage) {
            return Err(ToolClaimIngestError::NonCanonicalSources);
        }
    }

    let function_entries: BTreeMap<Sha256Digest, BTreeSet<crate::facts::BankAddr>> = set
        .claims
        .iter()
        .filter_map(|claim| match &claim.kind {
            ToolCandidateKind::FunctionEntry { address } => {
                Some((claim.source_sha256, address.clone()))
            }
            _ => None,
        })
        .fold(BTreeMap::new(), |mut by_source, (source, address)| {
            by_source.entry(source).or_default().insert(address);
            by_source
        });
    let mut source_kinds = BTreeMap::<Sha256Digest, Vec<ToolCandidateKind>>::new();
    for claim in &set.claims {
        let Some(source) = sources.get(&claim.source_sha256).copied() else {
            return Err(ToolClaimIngestError::OrphanClaim {
                claim: claim.claim_id,
            });
        };
        if claim.proof_ceiling != CandidateProofCeiling::Candidate {
            return Err(ToolClaimIngestError::ProofCeilingViolation {
                source: source.source_sha256,
            });
        }
        if !role_accepts(&source.role, &claim.kind)
            || !claim_is_bank_local(&claim.kind, &source.input)
            || matches!(claim.kind, ToolCandidateKind::FunctionBodyRange { .. })
                && source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V2
                && source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
            || matches!(claim.kind, ToolCandidateKind::ComputedControlFlow { .. })
                && source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
        {
            return Err(ToolClaimIngestError::InvalidClaimShape {
                claim: claim.claim_id,
            });
        }
        if let ToolCandidateKind::FunctionBodyRange { entry, .. } = &claim.kind {
            if !function_entries
                .get(&source.source_sha256)
                .is_some_and(|entries| entries.contains(entry))
            {
                return Err(ToolClaimIngestError::InvalidClaimShape {
                    claim: claim.claim_id,
                });
            }
        }
        if tool_claim_id_v1(source.source_sha256, &claim.kind)? != claim.claim_id {
            return Err(ToolClaimIngestError::ClaimDigestCollision {
                claim: claim.claim_id,
            });
        }
        if claim.observations.is_empty()
            || !strictly_sorted_unique(&claim.observations)
            || claim.observations.iter().any(|observation| {
                observation.provider_claim_ids.is_empty()
                    || observation.source_sequences.is_empty()
                    || !strictly_sorted_unique(&observation.provider_claim_ids)
                    || !strictly_sorted_unique(&observation.source_sequences)
            })
        {
            return Err(ToolClaimIngestError::InvalidObservations {
                claim: claim.claim_id,
            });
        }
        source_kinds
            .entry(source.source_sha256)
            .or_default()
            .push(claim.kind.clone());
    }
    for source in &set.sources {
        let kinds = source_kinds
            .get(&source.source_sha256)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if recompute_tool_run_source_sha256(source, kinds) != source.source_sha256 {
            return Err(ToolClaimIngestError::InvalidSourceDigest {
                source: source.source_sha256,
            });
        }
    }
    Ok(())
}

fn validate_source_binding(
    snapshot: &ProgramSnapshotV1,
    snapshot_digest: Sha256Digest,
    source: &ToolRunSource,
) -> Result<(), ToolClaimIngestError> {
    if source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION
        && source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V2
        && source.schema_version != TOOL_ADAPTER_SCHEMA_VERSION_V3
    {
        return Err(ToolClaimIngestError::SourceSchemaMismatch {
            source: source.source_sha256,
            version: source.schema_version,
        });
    }
    if bank_input_identity_v1(snapshot, &source.input.bank)? != source.input {
        return Err(ToolClaimIngestError::SourceInputMismatch {
            source: source.source_sha256,
        });
    }
    let snapshot_lineage: Vec<_> = source
        .lineage
        .iter()
        .filter(|item| item.role == ToolLineageRole::DiscoverySnapshot)
        .collect();
    match snapshot_lineage.as_slice() {
        [] => Err(ToolClaimIngestError::MissingSnapshotLineage {
            source: source.source_sha256,
        }),
        [lineage] if lineage.source_sha256 == snapshot_digest => Ok(()),
        [_] => Err(ToolClaimIngestError::StaleSnapshotLineage {
            source: source.source_sha256,
        }),
        many => Err(ToolClaimIngestError::AmbiguousSnapshotLineage {
            source: source.source_sha256,
            count: many.len(),
        }),
    }
}

fn snapshot_bank_mapping_sha256_v2(input: &BankInputDigestV1) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.snapshot-bank-mapping.v2\0");
    hash_str(&mut hasher, &input.bank);
    match &input.backing {
        BankBackingSpanV1::RomAffine {
            rom_space,
            rom_start,
            rom_end,
        } => {
            hasher.update([1]);
            hasher.update([match rom_space {
                crate::facts::RomAddressSpace::Physical => 1,
                crate::facts::RomAddressSpace::Virtual => 2,
            }]);
            hasher.update(rom_start.to_le_bytes());
            hasher.update(rom_end.to_le_bytes());
        }
        BankBackingSpanV1::Materialized {
            receipt_sha256,
            output_start,
            output_end,
        } => {
            hasher.update([2]);
            hash_str(&mut hasher, receipt_sha256);
            hasher.update(output_start.to_le_bytes());
            hasher.update(output_end.to_le_bytes());
        }
    }
    hasher.update(input.va_start.to_le_bytes());
    hasher.update(input.va_end.to_le_bytes());
    Sha256Digest(hasher.finalize().into())
}

fn tool_claim_id_v1(
    source: Sha256Digest,
    kind: &ToolCandidateKind,
) -> Result<Sha256Digest, ToolClaimIngestError> {
    let encoded = serde_json::to_vec(kind)
        .map_err(|error| ToolClaimIngestError::SnapshotSerialization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.tool-claim.v1\0");
    hasher.update(source.0);
    hasher.update(encoded);
    Ok(Sha256Digest(hasher.finalize().into()))
}

fn role_accepts(role: &ToolRunRole, kind: &ToolCandidateKind) -> bool {
    matches!(
        (role, kind),
        (
            ToolRunRole::FunctionBoundaryCandidates,
            ToolCandidateKind::FunctionEntry { .. }
                | ToolCandidateKind::FunctionExtent { .. }
                | ToolCandidateKind::FunctionBodyRange { .. }
        ) | (
            ToolRunRole::ControlFlowCandidates,
            ToolCandidateKind::ComputedControlFlow { .. }
        ) | (
            ToolRunRole::RegionCandidates,
            ToolCandidateKind::ExecutableRange { .. } | ToolCandidateKind::DataRange { .. }
        ) | (
            ToolRunRole::SymbolCandidates,
            ToolCandidateKind::SymbolAlias { .. }
        )
    )
}

fn claim_is_bank_local(kind: &ToolCandidateKind, input: &BankInputIdentity) -> bool {
    let valid_address = |address: &crate::facts::BankAddr, aligned: bool| {
        address.bank == input.bank
            && address.pc >= input.va_start
            && address.pc < input.va_end
            && (!aligned || address.pc.is_multiple_of(4))
    };
    let valid_range = |range: &crate::tool_adapter::BankRange, aligned: bool| {
        range.bank == input.bank
            && range.va_start < range.va_end
            && range.va_start >= input.va_start
            && range.va_end <= input.va_end
            && (!aligned || (range.va_start.is_multiple_of(4) && range.va_end.is_multiple_of(4)))
    };
    match kind {
        ToolCandidateKind::FunctionEntry { address } => valid_address(address, true),
        ToolCandidateKind::FunctionExtent { range }
        | ToolCandidateKind::ExecutableRange { range } => valid_range(range, true),
        ToolCandidateKind::FunctionBodyRange { entry, range } => {
            valid_address(entry, true) && valid_range(range, true)
        }
        ToolCandidateKind::DataRange { range } => valid_range(range, false),
        ToolCandidateKind::SymbolAlias { address, alias } => {
            valid_address(address, false)
                && !alias.is_empty()
                && alias.len() <= 256
                && !alias.chars().any(char::is_control)
        }
        ToolCandidateKind::ComputedControlFlow {
            site,
            targets,
            completeness: crate::tool_adapter::ComputedFlowCompleteness::Unknown,
            ..
        } => {
            valid_address(site, true)
                && targets.windows(2).all(|pair| pair[0] < pair[1])
                && targets.iter().all(|target| valid_address(target, true))
        }
    }
}

fn strictly_sorted_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn strictly_sorted_unique_by<T, K: Ord + Copy>(values: &[T], key: impl Fn(&T) -> K) -> bool {
    values.windows(2).all(|pair| key(&pair[0]) < key(&pair[1]))
}

fn canonical_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn canonical_u64s(values: &[u64]) -> Vec<u64> {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.dedup();
    values
}

fn hash_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, BankAddr, CandidateDetector, Fact,
        FactDb, FunctionEntryEvidence, ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::normalize;
    use crate::spimdisasm_adapter::{
        export_function_info_csv, SpimdisasmExportRequest, SpimdisasmRunDiagnostics,
    };
    use crate::tool_adapter::{
        export_complete_tool_jsonl, export_complete_tool_jsonl_v2, ingest_tool_jsonl,
        AdapterLimits, CompleteToolRun, ToolAdapterExpectation, ToolClaimRecord, ToolIdentity,
        ToolResourceDiagnostics,
    };

    const BASE: u32 = 0x8000_0400;

    fn snapshot() -> ProgramSnapshotV1 {
        let mut bytes = vec![0u8; 0x1020];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[0x1000..0x1004].copy_from_slice(&0x03e0_0008u32.to_be_bytes());
        let rom = normalize(&bytes).unwrap();
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: crate::banks::BOOT_BANK.into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1020,
            va_start: BASE,
            va_end: BASE + 0x20,
        });
        facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![mapping],
                "synthetic_tool_claim_fixture",
            )
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: crate::banks::BOOT_BANK.into(),
            va_start: BASE,
            va_end: BASE + 0x20,
        });
        facts
            .conclude(
                executable_range_subject(crate::banks::BOOT_BANK, BASE, BASE + 0x20),
                ProofState::Proven,
                vec![executable],
                "synthetic_tool_claim_fixture",
            )
            .unwrap();
        let target = BankAddr::new(crate::banks::BOOT_BANK, BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new(crate::banks::BOOT_BANK, BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "synthetic_tool_claim_fixture",
            )
            .unwrap();
        crate::snapshot::compose_materialized_bank_v1(
            &rom,
            &facts,
            crate::snapshot::MaterializedBankInput {
                bank: crate::banks::BOOT_BANK,
                va_start: BASE,
                bytes: &rom.bytes[0x1000..],
                seed_roots: &[BASE],
            },
        )
        .unwrap()
    }

    fn tool() -> ToolIdentity {
        ToolIdentity {
            name: "test-tool".into(),
            version: "1".into(),
            build_sha256: Sha256Digest([7; 32]),
        }
    }

    fn run(
        snapshot: &ProgramSnapshotV1,
        role: ToolRunRole,
        claims: Vec<ToolClaimRecord>,
    ) -> ToolAdapterOutput {
        let input = bank_input_identity_v1(snapshot, crate::banks::BOOT_BANK).unwrap();
        let lineage = vec![discovery_snapshot_lineage_v3(snapshot).unwrap()];
        let jsonl = export_complete_tool_jsonl(CompleteToolRun {
            tool: tool(),
            role: role.clone(),
            input: input.clone(),
            lineage: lineage.clone(),
            resources: ToolResourceDiagnostics {
                input_bytes: u64::from(input.va_end - input.va_start),
                elapsed_millis: 1,
                peak_memory_bytes: None,
                limit_hit: false,
                warnings: Vec::new(),
            },
            claims,
        })
        .unwrap();
        ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role,
                lineage,
                limits: AdapterLimits::default(),
            },
        )
        .unwrap()
    }

    fn run_v2(
        snapshot: &ProgramSnapshotV1,
        role: ToolRunRole,
        claims: Vec<ToolClaimRecord>,
    ) -> ToolAdapterOutput {
        let input = bank_input_identity_v1(snapshot, crate::banks::BOOT_BANK).unwrap();
        let lineage = vec![discovery_snapshot_lineage_v3(snapshot).unwrap()];
        let jsonl = export_complete_tool_jsonl_v2(CompleteToolRun {
            tool: tool(),
            role: role.clone(),
            input: input.clone(),
            lineage: lineage.clone(),
            resources: ToolResourceDiagnostics {
                input_bytes: u64::from(input.va_end - input.va_start),
                elapsed_millis: 1,
                peak_memory_bytes: None,
                limit_hit: false,
                warnings: Vec::new(),
            },
            claims,
        })
        .unwrap();
        ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role,
                lineage,
                limits: AdapterLimits::default(),
            },
        )
        .unwrap()
    }

    fn run_v3(
        snapshot: &ProgramSnapshotV1,
        role: ToolRunRole,
        claims: Vec<ToolClaimRecord>,
    ) -> ToolAdapterOutput {
        let input = bank_input_identity_v1(snapshot, crate::banks::BOOT_BANK).unwrap();
        let lineage = vec![discovery_snapshot_lineage_v3(snapshot).unwrap()];
        let jsonl = crate::tool_adapter::export_complete_tool_jsonl_v3(CompleteToolRun {
            tool: tool(),
            role: role.clone(),
            input: input.clone(),
            lineage: lineage.clone(),
            resources: ToolResourceDiagnostics {
                input_bytes: u64::from(input.va_end - input.va_start),
                elapsed_millis: 1,
                peak_memory_bytes: None,
                limit_hit: false,
                warnings: Vec::new(),
            },
            claims,
        })
        .unwrap();
        ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role,
                lineage,
                limits: AdapterLimits::default(),
            },
        )
        .unwrap()
    }

    fn claim(sequence: u64, id: &str, kind: ToolCandidateKind) -> ToolClaimRecord {
        ToolClaimRecord {
            sequence,
            provider_claim_id: id.into(),
            claim: kind,
        }
    }

    #[test]
    fn all_claim_shapes_freeze_without_mutating_native_snapshot() {
        let snapshot = snapshot();
        let native_before = serde_json::to_vec(&snapshot).unwrap();
        let end = snapshot.banks[0].input.va_end;
        let function = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![
                claim(
                    0,
                    "entry",
                    ToolCandidateKind::FunctionEntry {
                        address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    },
                ),
                claim(
                    1,
                    "extent",
                    ToolCandidateKind::FunctionExtent {
                        range: crate::tool_adapter::BankRange {
                            bank: crate::banks::BOOT_BANK.into(),
                            va_start: BASE,
                            va_end: BASE + 8,
                        },
                    },
                ),
            ],
        );
        let regions = run(
            &snapshot,
            ToolRunRole::RegionCandidates,
            vec![
                claim(
                    0,
                    "exec",
                    ToolCandidateKind::ExecutableRange {
                        range: crate::tool_adapter::BankRange {
                            bank: crate::banks::BOOT_BANK.into(),
                            va_start: BASE,
                            va_end: BASE + 8,
                        },
                    },
                ),
                claim(
                    1,
                    "data",
                    ToolCandidateKind::DataRange {
                        range: crate::tool_adapter::BankRange {
                            bank: crate::banks::BOOT_BANK.into(),
                            va_start: BASE + 8,
                            va_end: end,
                        },
                    },
                ),
            ],
        );
        let aliases = run(
            &snapshot,
            ToolRunRole::SymbolCandidates,
            vec![claim(
                0,
                "alias",
                ToolCandidateKind::SymbolAlias {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    alias: "candidate_name".into(),
                },
            )],
        );
        let frozen = freeze_tool_claims_v1(&snapshot, [&aliases, &function, &regions]).unwrap();
        assert_eq!(frozen.claims.len(), 5);
        assert!(frozen
            .claims
            .iter()
            .all(|item| item.proof_ceiling == CandidateProofCeiling::Candidate));
        let encoded = serde_json::to_vec(&frozen).unwrap();
        let decoded = serde_json::from_slice::<ToolClaimSetV1>(&encoded).unwrap();
        validate_tool_claim_set_v1(&snapshot, &decoded).unwrap();
        assert_eq!(decoded, frozen);
        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), native_before);
    }

    #[test]
    fn discontiguous_body_ranges_freeze_as_candidate_only_without_native_promotion() {
        let snapshot = snapshot();
        let function = run_v2(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![
                claim(
                    0,
                    "entry",
                    ToolCandidateKind::FunctionEntry {
                        address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    },
                ),
                claim(
                    1,
                    "body-0",
                    ToolCandidateKind::FunctionBodyRange {
                        entry: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                        range: crate::tool_adapter::BankRange {
                            bank: crate::banks::BOOT_BANK.into(),
                            va_start: BASE,
                            va_end: BASE + 8,
                        },
                    },
                ),
                claim(
                    2,
                    "body-1",
                    ToolCandidateKind::FunctionBodyRange {
                        entry: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                        range: crate::tool_adapter::BankRange {
                            bank: crate::banks::BOOT_BANK.into(),
                            va_start: BASE + 0x10,
                            va_end: BASE + 0x18,
                        },
                    },
                ),
            ],
        );
        let frozen = freeze_tool_claims_v1(&snapshot, [&function]).unwrap();
        assert_eq!(
            frozen.sources[0].schema_version,
            TOOL_ADAPTER_SCHEMA_VERSION_V2
        );
        assert_eq!(frozen.claims.len(), 3);
        assert!(frozen
            .claims
            .iter()
            .all(|claim| claim.proof_ceiling == CandidateProofCeiling::Candidate));
        assert_eq!(
            frozen
                .claims
                .iter()
                .filter(|claim| matches!(claim.kind, ToolCandidateKind::FunctionBodyRange { .. }))
                .count(),
            2
        );
        validate_tool_claim_set_v1(&snapshot, &frozen).unwrap();

        let mut relabelled_v1 = frozen.clone();
        relabelled_v1.sources[0].schema_version = TOOL_ADAPTER_SCHEMA_VERSION;
        assert!(matches!(
            validate_tool_claim_set_v1(&snapshot, &relabelled_v1),
            Err(ToolClaimIngestError::InvalidClaimShape { .. })
        ));
    }

    #[test]
    fn computed_flow_freezes_as_unknown_candidate_without_native_promotion() {
        let snapshot = snapshot();
        let native_before = serde_json::to_vec(&snapshot).unwrap();
        let flow = run_v3(
            &snapshot,
            ToolRunRole::ControlFlowCandidates,
            vec![claim(
                0,
                "computed-site",
                ToolCandidateKind::ComputedControlFlow {
                    site: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    via_call: false,
                    targets: vec![BankAddr::new(crate::banks::BOOT_BANK, BASE + 0x10)],
                    completeness: crate::tool_adapter::ComputedFlowCompleteness::Unknown,
                },
            )],
        );
        let frozen = freeze_tool_claims_v1(&snapshot, [&flow]).unwrap();
        assert_eq!(
            frozen.sources[0].schema_version,
            TOOL_ADAPTER_SCHEMA_VERSION_V3
        );
        assert_eq!(frozen.claims.len(), 1);
        assert_eq!(
            frozen.claims[0].proof_ceiling,
            CandidateProofCeiling::Candidate
        );
        validate_tool_claim_set_v1(&snapshot, &frozen).unwrap();
        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), native_before);

        let mut relabelled_v2 = frozen.clone();
        relabelled_v2.sources[0].schema_version = TOOL_ADAPTER_SCHEMA_VERSION_V2;
        assert!(matches!(
            validate_tool_claim_set_v1(&snapshot, &relabelled_v2),
            Err(ToolClaimIngestError::InvalidClaimShape { .. })
        ));
    }

    #[test]
    fn run_order_is_deterministic_and_disabling_source_is_exact() {
        let snapshot = snapshot();
        let native_before = serde_json::to_vec(&snapshot).unwrap();
        let first = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![claim(
                0,
                "first",
                ToolCandidateKind::FunctionEntry {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                },
            )],
        );
        let second = run(
            &snapshot,
            ToolRunRole::SymbolCandidates,
            vec![claim(
                0,
                "second",
                ToolCandidateKind::SymbolAlias {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    alias: "name".into(),
                },
            )],
        );
        let a = freeze_tool_claims_v1(&snapshot, [&first, &second]).unwrap();
        let b = freeze_tool_claims_v1(&snapshot, [&second, &first]).unwrap();
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        let without = a.without_source(first.source().source_sha256);
        assert_eq!(without.sources.len(), 1);
        assert_eq!(without.claims.len(), 1);
        assert!(matches!(
            without.claims[0].kind,
            ToolCandidateKind::SymbolAlias { .. }
        ));
        validate_tool_claim_set_v1(&snapshot, &without).unwrap();
        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), native_before);
    }

    #[test]
    fn provider_record_lineage_unions_without_becoming_independent_proof() {
        let snapshot = snapshot();
        let kind = ToolCandidateKind::FunctionEntry {
            address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
        };
        let first = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![claim(0, "provider-a", kind.clone())],
        );
        let second = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![claim(0, "provider-b", kind)],
        );
        assert_eq!(
            first.source().source_sha256,
            second.source().source_sha256,
            "provider record IDs are observation lineage, not semantic source identity"
        );
        let frozen = freeze_tool_claims_v1(&snapshot, [&first, &second, &first]).unwrap();
        assert_eq!(frozen.sources.len(), 1);
        assert_eq!(frozen.claims.len(), 1);
        assert_eq!(frozen.claims[0].observations.len(), 2);
        assert_eq!(
            frozen.claims[0].proof_ceiling,
            CandidateProofCeiling::Candidate
        );
    }

    #[test]
    fn stale_snapshot_lineage_is_rejected() {
        let snapshot = snapshot();
        let valid = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![claim(
                0,
                "entry",
                ToolCandidateKind::FunctionEntry {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                },
            )],
        );
        let mut changed = snapshot.clone();
        changed.normalized_rom_sha256 = "00".repeat(32);
        assert!(matches!(
            freeze_tool_claims_v1(&changed, [&valid]),
            Err(ToolClaimIngestError::SourceInputMismatch { .. })
                | Err(ToolClaimIngestError::StaleSnapshotLineage { .. })
        ));
    }

    #[test]
    fn legacy_snapshot_cannot_reuse_schema_v3_tool_lineage() {
        let mut snapshot = snapshot();
        snapshot.schema_version = crate::snapshot::PROGRAM_SNAPSHOT_SCHEMA_V5;
        assert_eq!(
            program_snapshot_sha256_v3(&snapshot),
            Err(ToolClaimIngestError::UnsupportedSnapshotSchema(
                crate::snapshot::PROGRAM_SNAPSHOT_SCHEMA_V5
            ))
        );
    }

    #[test]
    fn bank_mapping_identity_hashes_the_typed_backing_variant() {
        let snapshot = snapshot();
        let affine = snapshot.banks[0].input.clone();
        let mut materialized = affine.clone();
        materialized.backing = BankBackingSpanV1::Materialized {
            receipt_sha256: "11".repeat(32),
            output_start: 0,
            output_end: materialized.va_end - materialized.va_start,
        };

        assert_ne!(
            snapshot_bank_mapping_sha256_v2(&affine),
            snapshot_bank_mapping_sha256_v2(&materialized)
        );
    }

    #[test]
    fn one_variant_ceiling_rejects_proven_during_deserialization() {
        let json = r#"{"claim_id":"0000000000000000000000000000000000000000000000000000000000000000","source_sha256":"0000000000000000000000000000000000000000000000000000000000000000","kind":{"type":"function_entry","address":{"bank":"boot","pc":2147484672}},"proof_ceiling":"proven","observations":[]}"#;
        assert!(serde_json::from_str::<CanonicalToolClaimV1>(json).is_err());
    }

    #[test]
    fn deserialized_sidecar_rejects_noncanonical_and_fabricated_fields() {
        let snapshot = snapshot();
        let first = run(
            &snapshot,
            ToolRunRole::FunctionBoundaryCandidates,
            vec![claim(
                0,
                "entry",
                ToolCandidateKind::FunctionEntry {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                },
            )],
        );
        let second = run(
            &snapshot,
            ToolRunRole::SymbolCandidates,
            vec![claim(
                0,
                "alias",
                ToolCandidateKind::SymbolAlias {
                    address: BankAddr::new(crate::banks::BOOT_BANK, BASE),
                    alias: "name".into(),
                },
            )],
        );
        let valid = freeze_tool_claims_v1(&snapshot, [&first, &second]).unwrap();

        let mut reordered = valid.clone();
        reordered.sources.reverse();
        assert_eq!(
            validate_tool_claim_set_v1(&snapshot, &reordered),
            Err(ToolClaimIngestError::NonCanonicalSources)
        );

        let mut missing_observation = valid.clone();
        let claim_id = missing_observation.claims[0].claim_id;
        missing_observation.claims[0].observations.clear();
        assert_eq!(
            validate_tool_claim_set_v1(&snapshot, &missing_observation),
            Err(ToolClaimIngestError::InvalidObservations { claim: claim_id })
        );

        let mut fabricated_source = valid;
        let source_id = fabricated_source.sources[0].source_sha256;
        fabricated_source.sources[0].tool.name = "different-tool".into();
        assert_eq!(
            validate_tool_claim_set_v1(&snapshot, &fabricated_source),
            Err(ToolClaimIngestError::InvalidSourceDigest { source: source_id })
        );
    }

    #[test]
    fn spimdisasm_csv_reaches_validated_sidecar_without_native_fact_ingestion() {
        let snapshot = snapshot();
        let native_before = serde_json::to_vec(&snapshot).unwrap();
        let input = bank_input_identity_v1(&snapshot, crate::banks::BOOT_BANK).unwrap();
        let csv = concat!(
            "vrom,address,name,file,length,hash of top bits of words,functions called by this function,non-jal function calls,referenced functions\n",
            "0x1000,0x80000400,func_80000400,,0x8,abc,[],[],[]"
        );
        let export = export_function_info_csv(
            csv.as_bytes(),
            SpimdisasmExportRequest {
                tool: ToolIdentity {
                    name: "spimdisasm".into(),
                    version: "1.42.2".into(),
                    build_sha256: Sha256Digest([9; 32]),
                },
                input,
                parent_lineage: vec![discovery_snapshot_lineage_v3(&snapshot).unwrap()],
                vrom_start: 0x1000,
                diagnostics: SpimdisasmRunDiagnostics {
                    elapsed_millis: 1,
                    peak_memory_bytes: None,
                    warnings: Vec::new(),
                },
            },
        )
        .unwrap();
        let output = ingest_tool_jsonl(&export.jsonl, &export.expectation).unwrap();
        let sidecar = freeze_tool_claims_v1(&snapshot, [&output]).unwrap();
        validate_tool_claim_set_v1(&snapshot, &sidecar).unwrap();
        assert_eq!(sidecar.claims.len(), 2);
        assert_eq!(serde_json::to_vec(&snapshot).unwrap(), native_before);
    }
}
