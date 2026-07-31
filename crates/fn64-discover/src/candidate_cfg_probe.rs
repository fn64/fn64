//! Bounded, candidate-only native CFG diagnostics for external function-entry
//! claims.
//!
//! This is an experiment bridge, not a conclusion path. It revalidates an
//! exact snapshot-bound [`ToolClaimSetV1`], verifies the materialized bank,
//! and runs native traversal independently for each candidate entry plus once
//! for their union. The native CFG and closure values never leave this module:
//! only counts and coverage diagnostics are returned. Consequently this API
//! cannot create facts, partitions, owners, block proofs, snapshots, or any
//! other authority-bearing artifact.

use crate::resolve::build_cfg_value_set_closed;
use crate::snapshot::ProgramSnapshotV1;
use crate::tool_adapter::{Sha256Digest, ToolCandidateKind};
use crate::tool_claims::{
    bank_input_identity_v1, program_snapshot_sha256_v2, validate_tool_claim_set_v1, ToolClaimSetV1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CANDIDATE_CFG_PROBE_SCHEMA: &str = "fn64.candidate-cfg-probe";
pub const CANDIDATE_CFG_PROBE_SCHEMA_VERSION_V1: u32 = 1;
pub const MAX_CANDIDATE_CFG_ROOTS: usize = 4096;
pub const MAX_CANDIDATE_CFG_VISITED_WORDS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCfgProbeLimits {
    pub max_roots: usize,
    pub max_aggregate_visited_words: usize,
}

impl Default for CandidateCfgProbeLimits {
    fn default() -> Self {
        Self {
            max_roots: MAX_CANDIDATE_CFG_ROOTS,
            max_aggregate_visited_words: MAX_CANDIDATE_CFG_VISITED_WORDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateCfgProbeState {
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCfgTraversalDiagnosticsV1 {
    pub candidate_entry: Option<u32>,
    pub visited_words: usize,
    pub bank_words: usize,
    pub coverage_basis_points: u16,
    pub basic_blocks: usize,
    pub direct_calls: usize,
    pub tail_transfers: usize,
    pub indirect_sites: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCfgProbeCapsV1 {
    pub max_roots: usize,
    pub max_aggregate_visited_words: usize,
    pub root_limit_hit: bool,
    pub visited_word_limit_hit: bool,
    pub skipped_roots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCfgImpactDeltaV1 {
    pub baseline_snapshot_visited_words: usize,
    pub union_overlap_words: Option<usize>,
    pub union_new_words: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateExecutableRangeV1 {
    pub va_start: u32,
    pub va_end: u32,
}

/// Path-free candidate diagnostics suitable for stdout or a private receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCfgProbeReportV1 {
    pub schema: String,
    pub schema_version: u32,
    pub state: CandidateCfgProbeState,
    pub program_snapshot_sha256: Sha256Digest,
    pub tool_claim_set_sha256: Sha256Digest,
    pub bank: String,
    pub bank_bytes_sha256: Sha256Digest,
    pub mapping_sha256: Sha256Digest,
    pub candidate_entry_claims: usize,
    pub candidate_executable_ranges: Vec<CandidateExecutableRangeV1>,
    pub selected_candidate_entries: usize,
    pub aggregate_visited_words: usize,
    pub caps: CandidateCfgProbeCapsV1,
    pub impact: CandidateCfgImpactDeltaV1,
    /// One pass seeded by the selected candidate entries. `None` means the
    /// conservative work reservation could not admit even this pass.
    pub union: Option<CandidateCfgTraversalDiagnosticsV1>,
    /// Canonical-address order. Missing selected entries are counted in
    /// `caps.skipped_roots` rather than represented by fabricated results.
    pub independent: Vec<CandidateCfgTraversalDiagnosticsV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateCfgProbeError {
    InvalidLimits,
    InvalidClaimSet(String),
    InvalidBankIdentity(String),
    InvalidBankGeometry,
    InvalidSnapshotBankDiagnostics,
    BankLengthMismatch { expected: usize, actual: usize },
    BankDigestMismatch,
    ClaimSetSerialization(String),
}

impl std::fmt::Display for CandidateCfgProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CandidateCfgProbeError {}

pub fn run_candidate_cfg_probe_v1(
    snapshot: &ProgramSnapshotV1,
    claims: &ToolClaimSetV1,
    bank: &str,
    bank_bytes: &[u8],
) -> Result<CandidateCfgProbeReportV1, CandidateCfgProbeError> {
    run_candidate_cfg_probe_v1_with_limits(
        snapshot,
        claims,
        bank,
        bank_bytes,
        CandidateCfgProbeLimits::default(),
    )
}

pub fn run_candidate_cfg_probe_v1_with_limits(
    snapshot: &ProgramSnapshotV1,
    claims: &ToolClaimSetV1,
    bank: &str,
    bank_bytes: &[u8],
    limits: CandidateCfgProbeLimits,
) -> Result<CandidateCfgProbeReportV1, CandidateCfgProbeError> {
    if limits.max_roots > MAX_CANDIDATE_CFG_ROOTS
        || limits.max_aggregate_visited_words > MAX_CANDIDATE_CFG_VISITED_WORDS
    {
        return Err(CandidateCfgProbeError::InvalidLimits);
    }
    validate_tool_claim_set_v1(snapshot, claims)
        .map_err(|error| CandidateCfgProbeError::InvalidClaimSet(error.to_string()))?;
    let input = bank_input_identity_v1(snapshot, bank)
        .map_err(|error| CandidateCfgProbeError::InvalidBankIdentity(error.to_string()))?;
    let expected_len = input
        .va_end
        .checked_sub(input.va_start)
        .ok_or(CandidateCfgProbeError::InvalidBankGeometry)? as usize;
    if expected_len == 0 || !expected_len.is_multiple_of(4) {
        return Err(CandidateCfgProbeError::InvalidBankGeometry);
    }
    if bank_bytes.len() != expected_len {
        return Err(CandidateCfgProbeError::BankLengthMismatch {
            expected: expected_len,
            actual: bank_bytes.len(),
        });
    }
    if Sha256Digest::of(bank_bytes) != input.bank_bytes_sha256 {
        return Err(CandidateCfgProbeError::BankDigestMismatch);
    }
    let snapshot_bank = snapshot
        .banks
        .iter()
        .find(|item| item.input.bank == bank)
        .ok_or_else(|| CandidateCfgProbeError::InvalidBankIdentity(bank.into()))?;
    if snapshot_bank.closure.cfg.bank != bank
        || snapshot_bank
            .closure
            .cfg
            .word_class
            .keys()
            .any(|pc| !pc.is_multiple_of(4) || *pc < input.va_start || *pc >= input.va_end)
    {
        return Err(CandidateCfgProbeError::InvalidSnapshotBankDiagnostics);
    }
    let baseline_words: BTreeSet<u32> = snapshot_bank
        .closure
        .cfg
        .word_class
        .keys()
        .copied()
        .collect();

    let canonical_entries: Vec<u32> = claims
        .claims
        .iter()
        .filter_map(|claim| match &claim.kind {
            ToolCandidateKind::FunctionEntry { address } if address.bank == bank => {
                Some(address.pc)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut candidate_executable_ranges = claims
        .claims
        .iter()
        .filter_map(|claim| match &claim.kind {
            ToolCandidateKind::ExecutableRange { range } if range.bank == bank => {
                Some(CandidateExecutableRangeV1 {
                    va_start: range.va_start,
                    va_end: range.va_end,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    candidate_executable_ranges.sort_by_key(|range| (range.va_start, range.va_end));
    candidate_executable_ranges.dedup();
    let selected: Vec<u32> = canonical_entries
        .iter()
        .take(limits.max_roots)
        .copied()
        .collect();
    let root_limit_hit = selected.len() != canonical_entries.len();
    let bank_words = bank_bytes.len() / 4;
    let mut aggregate_visited_words = 0usize;
    let mut visited_word_limit_hit = false;

    // `build_cfg_value_set_closed` has no work-budget parameter. Reserving a
    // full bank before every call is conservative, but proves the aggregate
    // number of words any admitted calls can visit cannot cross the cap.
    let mut run_pass = |entries: &[u32], candidate_entry: Option<u32>, compare_baseline: bool| {
        let remaining = limits
            .max_aggregate_visited_words
            .saturating_sub(aggregate_visited_words);
        if bank_words > remaining {
            visited_word_limit_hit = true;
            return None;
        }
        let closure = build_cfg_value_set_closed(bank, bank_bytes, input.va_start, entries);
        let visited_words = closure.cfg.word_class.len();
        debug_assert!(visited_words <= bank_words);
        aggregate_visited_words += visited_words;
        let impact = compare_baseline.then(|| {
            let overlap = closure
                .cfg
                .word_class
                .keys()
                .filter(|pc| baseline_words.contains(pc))
                .count();
            (overlap, visited_words - overlap)
        });
        Some((
            CandidateCfgTraversalDiagnosticsV1 {
                candidate_entry,
                visited_words,
                bank_words,
                coverage_basis_points: ((visited_words as u64 * 10_000) / bank_words as u64) as u16,
                basic_blocks: closure.cfg.blocks.len(),
                direct_calls: closure.cfg.direct_calls.len(),
                tail_transfers: closure.cfg.tail_transfers.len(),
                indirect_sites: closure.cfg.indirect_sites.len(),
            },
            impact,
        ))
    };

    let (union, union_impact) = if selected.is_empty() {
        (
            Some(CandidateCfgTraversalDiagnosticsV1 {
                candidate_entry: None,
                visited_words: 0,
                bank_words,
                coverage_basis_points: 0,
                basic_blocks: 0,
                direct_calls: 0,
                tail_transfers: 0,
                indirect_sites: 0,
            }),
            Some((0, 0)),
        )
    } else {
        match run_pass(&selected, None, true) {
            Some((diagnostics, impact)) => (Some(diagnostics), impact),
            None => (None, None),
        }
    };
    let mut independent = Vec::new();
    if union.is_some() {
        for entry in &selected {
            let Some((diagnostics, _)) = run_pass(std::slice::from_ref(entry), Some(*entry), false)
            else {
                break;
            };
            independent.push(diagnostics);
        }
    }
    let skipped_roots = canonical_entries.len().saturating_sub(independent.len());
    let state = if root_limit_hit || visited_word_limit_hit || skipped_roots != 0 {
        CandidateCfgProbeState::Partial
    } else {
        CandidateCfgProbeState::Complete
    };
    let encoded_claims = serde_json::to_vec(claims)
        .map_err(|error| CandidateCfgProbeError::ClaimSetSerialization(error.to_string()))?;

    Ok(CandidateCfgProbeReportV1 {
        schema: CANDIDATE_CFG_PROBE_SCHEMA.into(),
        schema_version: CANDIDATE_CFG_PROBE_SCHEMA_VERSION_V1,
        state,
        program_snapshot_sha256: program_snapshot_sha256_v2(snapshot)
            .map_err(|error| CandidateCfgProbeError::InvalidClaimSet(error.to_string()))?,
        tool_claim_set_sha256: Sha256Digest::of(&encoded_claims),
        bank: bank.into(),
        bank_bytes_sha256: input.bank_bytes_sha256,
        mapping_sha256: input.mapping_sha256,
        candidate_entry_claims: canonical_entries.len(),
        candidate_executable_ranges,
        selected_candidate_entries: selected.len(),
        aggregate_visited_words,
        caps: CandidateCfgProbeCapsV1 {
            max_roots: limits.max_roots,
            max_aggregate_visited_words: limits.max_aggregate_visited_words,
            root_limit_hit,
            visited_word_limit_hit,
            skipped_roots,
        },
        impact: CandidateCfgImpactDeltaV1 {
            baseline_snapshot_visited_words: baseline_words.len(),
            union_overlap_words: union_impact.map(|(overlap, _)| overlap),
            union_new_words: union_impact.map(|(_, new)| new),
        },
        union,
        independent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, BankAddr, CandidateDetector, Fact,
        FactDb, FunctionEntryEvidence, ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::snapshot::MaterializedBankInput;
    use crate::tool_adapter::{
        export_complete_tool_jsonl, ingest_tool_jsonl, AdapterLimits, CompleteToolRun,
        ToolAdapterExpectation, ToolClaimRecord, ToolIdentity, ToolResourceDiagnostics,
        ToolRunRole,
    };
    use crate::tool_claims::{
        bank_input_identity_v1, discovery_snapshot_lineage_v2, freeze_tool_claims_v1,
    };

    const BANK: &str = crate::banks::BOOT_BANK;
    const BASE: u32 = 0x8000_0400;

    fn fixture() -> (ProgramSnapshotV1, ToolClaimSetV1, Vec<u8>) {
        let mut image = vec![0u8; 0x1040];
        image[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        image[8..12].copy_from_slice(&BASE.to_be_bytes());
        let bank_words = [
            0x0c00_0104u32, // jal BASE + 0x10
            0x0000_0000,
            0x03e0_0008,
            0x0000_0000,
            0x03e0_0008,
            0x0000_0000,
        ];
        for (index, word) in bank_words.into_iter().enumerate() {
            image[0x1000 + index * 4..0x1004 + index * 4].copy_from_slice(&word.to_be_bytes());
        }
        let rom = crate::normalize(&image).unwrap();
        let bank_bytes = rom.bytes[0x1000..].to_vec();
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: BANK.into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1040,
            va_start: BASE,
            va_end: BASE + 0x40,
        });
        facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![mapping],
                "candidate_cfg_probe_fixture",
            )
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: BANK.into(),
            va_start: BASE,
            va_end: BASE + 0x40,
        });
        facts
            .conclude(
                executable_range_subject(BANK, BASE, BASE + 0x40),
                ProofState::Proven,
                vec![executable],
                "candidate_cfg_probe_fixture",
            )
            .unwrap();
        let target = BankAddr::new(BANK, BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new(BANK, BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "candidate_cfg_probe_fixture",
            )
            .unwrap();
        let snapshot = crate::snapshot::compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: BANK,
                va_start: BASE,
                bytes: &bank_bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let input = bank_input_identity_v1(&snapshot, BANK).unwrap();
        let lineage = vec![discovery_snapshot_lineage_v2(&snapshot).unwrap()];
        let role = ToolRunRole::FunctionBoundaryCandidates;
        let jsonl = export_complete_tool_jsonl(CompleteToolRun {
            tool: ToolIdentity {
                name: "candidate-probe-fixture".into(),
                version: "1".into(),
                build_sha256: Sha256Digest([7; 32]),
            },
            role: role.clone(),
            input: input.clone(),
            lineage: lineage.clone(),
            claims: vec![
                ToolClaimRecord {
                    sequence: 0,
                    provider_claim_id: "base".into(),
                    claim: ToolCandidateKind::FunctionEntry {
                        address: BankAddr::new(BANK, BASE),
                    },
                },
                ToolClaimRecord {
                    sequence: 1,
                    provider_claim_id: "target".into(),
                    claim: ToolCandidateKind::FunctionEntry {
                        address: BankAddr::new(BANK, BASE + 0x10),
                    },
                },
            ],
            resources: ToolResourceDiagnostics {
                input_bytes: bank_bytes.len() as u64,
                elapsed_millis: 1,
                peak_memory_bytes: None,
                limit_hit: false,
                warnings: Vec::new(),
            },
        })
        .unwrap();
        let output = ingest_tool_jsonl(
            &jsonl,
            &ToolAdapterExpectation {
                input,
                role,
                lineage,
                limits: AdapterLimits::default(),
            },
        )
        .unwrap();
        let claims = freeze_tool_claims_v1(&snapshot, [&output]).unwrap();
        (snapshot, claims, bank_bytes)
    }

    #[test]
    fn emits_only_bounded_candidate_diagnostics() {
        let (snapshot, claims, bank_bytes) = fixture();
        let report = run_candidate_cfg_probe_v1(&snapshot, &claims, BANK, &bank_bytes).unwrap();
        assert_eq!(report.state, CandidateCfgProbeState::Complete);
        assert_eq!(report.candidate_entry_claims, 2);
        assert_eq!(report.independent.len(), 2);
        assert!(report.union.as_ref().unwrap().visited_words > 0);
        assert!(report.impact.baseline_snapshot_visited_words > 0);
        assert!(report.impact.union_overlap_words.unwrap() > 0);
        assert_eq!(
            report.impact.union_overlap_words.unwrap() + report.impact.union_new_words.unwrap(),
            report.union.as_ref().unwrap().visited_words
        );
        let json = serde_json::to_value(&report).unwrap();
        for forbidden in [
            "facts",
            "cfg",
            "partition",
            "owner",
            "block_proof",
            "snapshot",
        ] {
            assert!(
                json.get(forbidden).is_none(),
                "unexpected authority field {forbidden}"
            );
        }
    }

    #[test]
    fn stale_claim_set_and_wrong_bank_bytes_fail_closed() {
        let (snapshot, claims, mut bank_bytes) = fixture();
        let mut stale = claims.clone();
        stale.program_snapshot_sha256 = Sha256Digest([9; 32]);
        assert!(matches!(
            run_candidate_cfg_probe_v1(&snapshot, &stale, BANK, &bank_bytes),
            Err(CandidateCfgProbeError::InvalidClaimSet(_))
        ));
        bank_bytes[0] ^= 1;
        assert_eq!(
            run_candidate_cfg_probe_v1(&snapshot, &claims, BANK, &bank_bytes),
            Err(CandidateCfgProbeError::BankDigestMismatch)
        );
    }

    #[test]
    fn root_and_work_caps_are_explicit_partial_states() {
        let (snapshot, claims, bank_bytes) = fixture();
        let root_capped = run_candidate_cfg_probe_v1_with_limits(
            &snapshot,
            &claims,
            BANK,
            &bank_bytes,
            CandidateCfgProbeLimits {
                max_roots: 1,
                max_aggregate_visited_words: MAX_CANDIDATE_CFG_VISITED_WORDS,
            },
        )
        .unwrap();
        assert_eq!(root_capped.state, CandidateCfgProbeState::Partial);
        assert!(root_capped.caps.root_limit_hit);
        assert_eq!(root_capped.selected_candidate_entries, 1);

        let work_capped = run_candidate_cfg_probe_v1_with_limits(
            &snapshot,
            &claims,
            BANK,
            &bank_bytes,
            CandidateCfgProbeLimits {
                max_roots: MAX_CANDIDATE_CFG_ROOTS,
                max_aggregate_visited_words: bank_bytes.len() / 4,
            },
        )
        .unwrap();
        assert_eq!(work_capped.state, CandidateCfgProbeState::Partial);
        assert!(work_capped.caps.visited_word_limit_hit);
        assert!(work_capped.aggregate_visited_words <= bank_bytes.len() / 4);
        assert!(work_capped.independent.is_empty());
        assert!(work_capped.impact.union_overlap_words.unwrap() > 0);
    }
}
