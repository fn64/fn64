//! Observational relations between receipt-bound tool entries and snapshot facts.
//!
//! This module reports overlap only. It cannot write facts or authorize an
//! entry, root, partition boundary, or owner. The native-relation path never
//! traverses from tool entries. A separate explicitly exploratory probe below
//! does, and returns counts only so that conditional graph reachability cannot
//! be mistaken for independent native evidence.
//!
//! The receipt capability does not authenticate the snapshot producer. A
//! caller may describe these relations as native only after independently
//! establishing that producer provenance.

use crate::candidate_corroboration::ValidatedDiscoveryOnlyToolClaims;
use crate::cfg::{BlockTerminator, Cfg, WordClass};
use crate::facts::{function_entry_subject, BankAddr, Fact, IndirectTransferState, ProofState};
use crate::resolve::build_cfg_value_set_closed;
use crate::tool_adapter::{Sha256Digest, ToolCandidateKind};
use crate::tool_claims::{bank_input_identity_v1, program_snapshot_sha256_v3};
use serde::Serialize;
use std::collections::BTreeSet;

pub const CANDIDATE_RELATION_REPORT_SCHEMA: &str = "fn64.candidate-snapshot-relations";
pub const CANDIDATE_UNREACHED_PROBE_SCHEMA: &str = "fn64.candidate-unreached-cfg-probe";
/// This diagnostic currently exists for the measured resident-bank experiment,
/// not as a general large-bank traversal service. One pass over a one-MiB bank
/// bounds native decode work to 262,144 aligned words.
pub const MAX_UNREACHED_PROBE_BANK_BYTES: usize = 1024 * 1024;
/// The measured experiment has eight roots. Retaining an 8x allowance permits
/// nearby revisions without turning an external sidecar into an unbounded root
/// fan-out mechanism.
pub const MAX_UNREACHED_PROBE_ROOTS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateEntryStateCountsV1 {
    pub absent: usize,
    pub open: usize,
    pub candidate: usize,
    pub supported: usize,
    pub rejected: usize,
    pub conflict: usize,
    pub proven: usize,
}

/// Count-only report safe to retain beside private per-ROM artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateNativeRelationReportV1 {
    pub schema: String,
    pub schema_version: u32,
    pub program_snapshot_sha256: Sha256Digest,
    pub tool_claim_set_sha256: Sha256Digest,
    pub bank: String,
    pub candidate_entries: usize,
    pub snapshot_entry_states: CandidateEntryStateCountsV1,
    pub baseline_unreached_snapshot_entry_states: CandidateEntryStateCountsV1,
    pub baseline_reached: usize,
    pub baseline_unreached: usize,
    pub baseline_proven_code_direct_call_targets: usize,
    pub baseline_exhaustive_resolved_call_targets: usize,
    pub baseline_reached_without_call_relation: usize,
}

/// Candidate-seeded CFG diagnostics for entries absent from the baseline
/// ProvenCode set. Every count remains exploratory because the external
/// candidate selected the traversal roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateUnreachedCfgProbeV1 {
    pub schema: String,
    pub schema_version: u32,
    pub program_snapshot_sha256: Sha256Digest,
    pub tool_claim_set_sha256: Sha256Digest,
    pub bank: String,
    pub selected_unreached_roots: usize,
    pub visited_words: usize,
    pub overlap_baseline_words: usize,
    pub new_words: usize,
    pub basic_blocks: usize,
    pub direct_calls: usize,
    pub tail_transfers: usize,
    pub indirect_sites: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateNativeRelationError {
    SnapshotDigest,
    SelectedBankMissing,
    ClaimOutsideSelectedBank,
    ResourceLimit,
    BankLengthMismatch,
    BankDigestMismatch,
}

fn candidate_entries(
    validated: &ValidatedDiscoveryOnlyToolClaims,
    bank_name: &str,
) -> Result<BTreeSet<u32>, CandidateNativeRelationError> {
    validated
        .claims()
        .claims
        .iter()
        .filter_map(|claim| match &claim.kind {
            ToolCandidateKind::FunctionEntry { address } => Some(address),
            _ => None,
        })
        .map(|address| {
            if address.bank == bank_name {
                Ok(address.pc)
            } else {
                Err(CandidateNativeRelationError::ClaimOutsideSelectedBank)
            }
        })
        .collect()
}

impl std::fmt::Display for CandidateNativeRelationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CandidateNativeRelationError {}

pub fn report_candidate_native_relations_v1(
    validated: &ValidatedDiscoveryOnlyToolClaims,
) -> Result<CandidateNativeRelationReportV1, CandidateNativeRelationError> {
    let snapshot = validated.snapshot();
    let bank = snapshot
        .banks
        .get(validated.bank_index())
        .ok_or(CandidateNativeRelationError::SelectedBankMissing)?;
    let bank_name = &bank.input.bank;
    let entries = candidate_entries(validated, bank_name)?;

    let mut states = CandidateEntryStateCountsV1::default();
    let mut unreached_states = CandidateEntryStateCountsV1::default();
    let mut reached = BTreeSet::new();
    for pc in &entries {
        let address = BankAddr::new(bank_name, *pc);
        let state = snapshot
            .facts
            .conclusion(&function_entry_subject(&address))
            .map(|conclusion| conclusion.state);
        increment_state_count(&mut states, state);
        if bank.closure.cfg.word_class.get(pc) == Some(&WordClass::ProvenCode) {
            reached.insert(*pc);
        } else {
            increment_state_count(&mut unreached_states, state);
        }
    }

    fn increment_state_count(counts: &mut CandidateEntryStateCountsV1, state: Option<ProofState>) {
        match state {
            None => counts.absent += 1,
            Some(ProofState::Open) => counts.open += 1,
            Some(ProofState::Candidate) => counts.candidate += 1,
            Some(ProofState::Supported) => counts.supported += 1,
            Some(ProofState::Rejected) => counts.rejected += 1,
            Some(ProofState::Conflict) => counts.conflict += 1,
            Some(ProofState::Proven) => counts.proven += 1,
        }
    }

    let mut direct_targets = BTreeSet::new();
    for fact in snapshot.facts.facts() {
        let Fact::DirectCall { source, target } = fact else {
            continue;
        };
        if source.bank == *bank_name
            && target.bank == *bank_name
            && entries.contains(&target.pc)
            && bank.closure.cfg.word_class.get(&source.pc) == Some(&WordClass::ProvenCode)
        {
            direct_targets.insert(target.pc);
        }
    }
    let resolved_targets = baseline_exhaustive_resolved_call_targets(
        &bank.closure.cfg,
        snapshot.facts.facts(),
        &entries,
    );
    let called: BTreeSet<u32> = direct_targets.union(&resolved_targets).copied().collect();

    Ok(CandidateNativeRelationReportV1 {
        schema: CANDIDATE_RELATION_REPORT_SCHEMA.into(),
        schema_version: 1,
        program_snapshot_sha256: program_snapshot_sha256_v3(snapshot)
            .map_err(|_| CandidateNativeRelationError::SnapshotDigest)?,
        tool_claim_set_sha256: validated.tool_claim_set_sha256(),
        bank: bank_name.clone(),
        candidate_entries: entries.len(),
        snapshot_entry_states: states,
        baseline_unreached_snapshot_entry_states: unreached_states,
        baseline_reached: reached.len(),
        baseline_unreached: entries.len() - reached.len(),
        baseline_proven_code_direct_call_targets: direct_targets.len(),
        baseline_exhaustive_resolved_call_targets: resolved_targets.len(),
        baseline_reached_without_call_relation: reached.difference(&called).count(),
    })
}

/// Snapshot-local resolved calls are represented by the CFG terminator plus
/// one exact exhaustive `IndirectTransferAnalysis`. `Fact::ResolvedCall` is
/// used at the cross-bank composition seam and is not the in-bank
/// representation consulted here. Mirror snapshot composition's authority
/// preconditions, but return only an observational target set.
fn baseline_exhaustive_resolved_call_targets(
    cfg: &Cfg,
    facts: &[Fact],
    candidate_entries: &BTreeSet<u32>,
) -> BTreeSet<u32> {
    let mut result = BTreeSet::new();
    for block in &cfg.blocks {
        let BlockTerminator::ResolvedIndirect {
            targets,
            via_call: true,
        } = &block.terminator
        else {
            continue;
        };
        if block.end_va < block.start_va.saturating_add(8) {
            continue;
        }
        let site_pc = block.end_va - 8;
        let Some(delay_pc) = site_pc.checked_add(4) else {
            continue;
        };
        if cfg.word_class.get(&site_pc) != Some(&WordClass::ProvenCode)
            || cfg.word_class.get(&delay_pc) != Some(&WordClass::ProvenCode)
            || !resolved_call_evidence_matches(facts, &cfg.bank, site_pc, targets)
        {
            continue;
        }
        result.extend(
            targets
                .iter()
                .copied()
                .filter(|target| candidate_entries.contains(target)),
        );
    }
    result
}

fn resolved_call_evidence_matches(
    facts: &[Fact],
    bank: &str,
    site_pc: u32,
    cfg_targets: &[u32],
) -> bool {
    let mut expected = cfg_targets.to_vec();
    expected.sort_unstable();
    expected.dedup();
    if expected.is_empty() {
        return false;
    }

    let analyses: Vec<_> = facts
        .iter()
        .filter_map(|fact| {
            let Fact::IndirectTransferAnalysis {
                site,
                via_call: true,
                state,
                kind,
                targets,
                ..
            } = fact
            else {
                return None;
            };
            if site.bank != bank || site.pc != site_pc || targets.is_empty() {
                return None;
            }
            let mut targets = targets.clone();
            targets.sort_unstable();
            targets.dedup();
            Some((*state, *kind, targets))
        })
        .collect();
    matches!(
        analyses.as_slice(),
        [(IndirectTransferState::Exhaustive, Some(_), targets)] if targets == &expected
    )
}

pub fn probe_baseline_unreached_candidates_v1(
    validated: &ValidatedDiscoveryOnlyToolClaims,
    bank_bytes: &[u8],
) -> Result<CandidateUnreachedCfgProbeV1, CandidateNativeRelationError> {
    let snapshot = validated.snapshot();
    let bank = snapshot
        .banks
        .get(validated.bank_index())
        .ok_or(CandidateNativeRelationError::SelectedBankMissing)?;
    let bank_name = &bank.input.bank;
    let input = bank_input_identity_v1(snapshot, bank_name)
        .map_err(|_| CandidateNativeRelationError::SelectedBankMissing)?;
    let expected_len = input.va_end.saturating_sub(input.va_start) as usize;
    if expected_len == 0
        || expected_len > MAX_UNREACHED_PROBE_BANK_BYTES
        || !expected_len.is_multiple_of(4)
    {
        return Err(CandidateNativeRelationError::ResourceLimit);
    }
    if bank_bytes.len() != expected_len {
        return Err(CandidateNativeRelationError::BankLengthMismatch);
    }
    if Sha256Digest::of(bank_bytes) != input.bank_bytes_sha256 {
        return Err(CandidateNativeRelationError::BankDigestMismatch);
    }
    let entries = candidate_entries(validated, bank_name)?;
    let roots: Vec<u32> = entries
        .into_iter()
        .filter(|pc| bank.closure.cfg.word_class.get(pc) != Some(&WordClass::ProvenCode))
        .collect();
    if roots.len() > MAX_UNREACHED_PROBE_ROOTS {
        return Err(CandidateNativeRelationError::ResourceLimit);
    }
    let cfg = build_cfg_value_set_closed(bank_name, bank_bytes, input.va_start, &roots).cfg;
    let overlap = cfg
        .word_class
        .keys()
        .filter(|pc| bank.closure.cfg.word_class.contains_key(pc))
        .count();
    Ok(CandidateUnreachedCfgProbeV1 {
        schema: CANDIDATE_UNREACHED_PROBE_SCHEMA.into(),
        schema_version: 1,
        program_snapshot_sha256: program_snapshot_sha256_v3(snapshot)
            .map_err(|_| CandidateNativeRelationError::SnapshotDigest)?,
        tool_claim_set_sha256: validated.tool_claim_set_sha256(),
        bank: bank_name.clone(),
        selected_unreached_roots: roots.len(),
        visited_words: cfg.word_class.len(),
        overlap_baseline_words: overlap,
        new_words: cfg.word_class.len() - overlap,
        basic_blocks: cfg.blocks.len(),
        direct_calls: cfg.direct_calls.len(),
        tail_transfers: cfg.tail_transfers.len(),
        indirect_sites: cfg.indirect_sites.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::BasicBlock;
    use crate::facts::IndirectTransferKind;

    const BANK: &str = "bank";
    const SITE: u32 = 0x8000_1000;
    const TARGET: u32 = 0x8000_2000;

    fn cfg() -> Cfg {
        Cfg {
            bank: BANK.into(),
            word_class: [
                (SITE, WordClass::ProvenCode),
                (SITE + 4, WordClass::ProvenCode),
                (TARGET, WordClass::ProvenCode),
            ]
            .into_iter()
            .collect(),
            blocks: vec![BasicBlock {
                start_va: SITE,
                end_va: SITE + 8,
                terminator: BlockTerminator::ResolvedIndirect {
                    targets: vec![TARGET],
                    via_call: true,
                },
            }],
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            proven_roots: Vec::new(),
        }
    }

    fn exhaustive_analysis() -> Fact {
        Fact::IndirectTransferAnalysis {
            site: BankAddr::new(BANK, SITE),
            via_call: true,
            state: IndirectTransferState::Exhaustive,
            kind: Some(IndirectTransferKind::Constant),
            targets: vec![TARGET],
            memory_sources: Vec::new(),
        }
    }

    #[test]
    fn counts_exact_in_bank_exhaustive_resolved_call_relation() {
        let entries = BTreeSet::from([TARGET]);
        assert_eq!(
            baseline_exhaustive_resolved_call_targets(&cfg(), &[exhaustive_analysis()], &entries),
            entries
        );
    }

    #[test]
    fn resolved_call_relation_fails_closed_on_nonexact_evidence() {
        let entries = BTreeSet::from([TARGET]);
        let mut bounded = exhaustive_analysis();
        let Fact::IndirectTransferAnalysis { state, .. } = &mut bounded else {
            unreachable!()
        };
        *state = IndirectTransferState::Bounded;
        assert!(baseline_exhaustive_resolved_call_targets(&cfg(), &[bounded], &entries).is_empty());

        let duplicate = exhaustive_analysis();
        assert!(baseline_exhaustive_resolved_call_targets(
            &cfg(),
            &[exhaustive_analysis(), duplicate],
            &entries,
        )
        .is_empty());

        let mut missing_delay = cfg();
        missing_delay.word_class.remove(&(SITE + 4));
        assert!(baseline_exhaustive_resolved_call_targets(
            &missing_delay,
            &[exhaustive_analysis()],
            &entries,
        )
        .is_empty());
    }
}
