//! Function-boundary-independent proof for reachable executable blocks.
//!
//! This does not weaken [`crate::owner_proof::ExactFunctionOwner`]. It
//! exposes the smaller fact needed by `block_aot`: bytes reached from an
//! authoritative entry through the closed CFG, accepted by the shared
//! decoder, and bound to exactly one proven ROM mapping.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg, WordClass};
use crate::facts::{executable_range_subject, BankAddr, Fact, FactDb, ProofState, RomAddressSpace};
use crate::owner_proof::{OwnerAssessment, OwnerBlocker, OwnerProofReport};
use crate::partition::Partition;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockProofBlocker {
    AnalysisBankMismatch,
    Unowned,
    AmbiguousOwners { roots: Vec<u32> },
    EntryNotAuthoritative { root: u32 },
    InvalidGeometry,
    WordNotProvenCode { pc: u32, class: Option<WordClass> },
    InvalidInstruction { pc: u32, word: u32 },
    MissingDelaySlot { control_pc: u32 },
    RanOffEnd,
    MissingRomBacking,
    MultipleRomBackings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReachableCodeBlock {
    pub bank: String,
    pub start_va: u32,
    pub end_va: u32,
    pub owner_root: u32,
    pub rom_space: RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
    pub terminator: BlockTerminator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BlockAssessment {
    Proven {
        block: ReachableCodeBlock,
    },
    Candidate {
        start_va: u32,
        end_va: u32,
        blockers: Vec<BlockProofBlocker>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockProofReport {
    pub bank: String,
    pub assessments: Vec<BlockAssessment>,
    pub proven_blocks: u64,
    pub proven_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Backing {
    space: RomAddressSpace,
    start: u32,
    end: u32,
}

pub fn prove_reachable_blocks(
    cfg: &Cfg,
    partition: &Partition,
    owners: &OwnerProofReport,
    facts: &FactDb,
) -> BlockProofReport {
    let owner_of: BTreeMap<u32, u32> = partition
        .owners
        .iter()
        .flat_map(|owner| {
            owner
                .block_starts
                .iter()
                .map(move |&start| (start, owner.root_va))
        })
        .collect();
    let ambiguous: BTreeMap<u32, Vec<u32>> = partition
        .ambiguous
        .iter()
        .map(|block| (block.block_start, block.claimants.clone()))
        .collect();
    let authoritative: BTreeMap<u32, bool> = owners
        .assessments
        .iter()
        .map(|assessment| {
            let root = assessment.entry().pc;
            let blocked = match assessment {
                OwnerAssessment::Proven { .. } => false,
                OwnerAssessment::Candidate { frontier }
                | OwnerAssessment::Ambiguous { frontier } => frontier
                    .blockers
                    .contains(&OwnerBlocker::EntryNotAuthoritative),
            };
            (root, !blocked)
        })
        .collect();

    let mut assessments = Vec::with_capacity(cfg.blocks.len());
    let mut proven_blocks = 0;
    let mut proven_bytes = 0;
    for block in &cfg.blocks {
        let mut blockers = BTreeSet::new();
        if partition.bank != cfg.bank || owners.bank != cfg.bank {
            blockers.insert(BlockProofBlocker::AnalysisBankMismatch);
        }
        let owner_root = owner_of.get(&block.start_va).copied();
        if let Some(roots) = ambiguous.get(&block.start_va) {
            blockers.insert(BlockProofBlocker::AmbiguousOwners {
                roots: roots.clone(),
            });
        } else if owner_root.is_none() {
            blockers.insert(BlockProofBlocker::Unowned);
        }
        if let Some(root) = owner_root {
            if !authoritative.get(&root).copied().unwrap_or(false) {
                blockers.insert(BlockProofBlocker::EntryNotAuthoritative { root });
            }
        }
        validate_block(block, cfg, &mut blockers);
        let backings = block_backings(facts, &cfg.bank, block.start_va, block.end_va);
        let backing = match backings.as_slice() {
            [] => {
                blockers.insert(BlockProofBlocker::MissingRomBacking);
                None
            }
            [backing] => Some(*backing),
            _ => {
                blockers.insert(BlockProofBlocker::MultipleRomBackings);
                None
            }
        };
        if blockers.is_empty() {
            let backing = backing.expect("one backing when no blocker remains");
            let root = owner_root.expect("one owner when no blocker remains");
            proven_blocks += 1;
            proven_bytes += u64::from(block.end_va - block.start_va);
            assessments.push(BlockAssessment::Proven {
                block: ReachableCodeBlock {
                    bank: cfg.bank.clone(),
                    start_va: block.start_va,
                    end_va: block.end_va,
                    owner_root: root,
                    rom_space: backing.space,
                    rom_start: backing.start,
                    rom_end: backing.end,
                    terminator: block.terminator.clone(),
                },
            });
        } else {
            assessments.push(BlockAssessment::Candidate {
                start_va: block.start_va,
                end_va: block.end_va,
                blockers: blockers.into_iter().collect(),
            });
        }
    }
    BlockProofReport {
        bank: cfg.bank.clone(),
        assessments,
        proven_blocks,
        proven_bytes,
    }
}

/// One executable interval derived from the union of reached proven-code
/// block geometry, with the conclusion state this pass recorded for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedExecutableRange {
    pub va_start: u32,
    pub va_end: u32,
    pub state: ProofState,
}

/// Convert proven reached-code blocks into typed, evidence-carrying
/// [`Fact::ExecutableRange`] facts for the report's bank.
///
/// Soundness boundary: a word reached by CFG closure from an authoritative
/// entry is demonstrably executed under the proven mapping, so the union of
/// proven block intervals is proven executable. Exactly those bytes are
/// claimed: adjacent/overlapping proven blocks merge into one interval, but
/// a gap between reached blocks is never bridged — unreached bytes stay
/// unproven. Region scores and content statistics play no role here; a
/// score-threshold promotion rule was measured and rejected
/// (docs/DISCOVER-PLAN.md).
///
/// A subject already `Rejected`/`Conflict` is never silently promoted: the
/// new reachability evidence is recorded and the conclusion moves to (or
/// stays) `Conflict`, preserving the disagreement instead of overwriting it.
pub fn conclude_reached_executable_ranges(
    report: &BlockProofReport,
    facts: &mut FactDb,
) -> Vec<DerivedExecutableRange> {
    let mut intervals: Vec<(u32, u32)> = report
        .assessments
        .iter()
        .filter_map(|assessment| match assessment {
            BlockAssessment::Proven { block } => Some((block.start_va, block.end_va)),
            BlockAssessment::Candidate { .. } => None,
        })
        .collect();
    intervals.sort_unstable();

    let mut merged: Vec<(u32, u32, u64)> = Vec::new();
    for (start, end) in intervals {
        match merged.last_mut() {
            Some((_, merged_end, blocks)) if start <= *merged_end => {
                *merged_end = (*merged_end).max(end);
                *blocks += 1;
            }
            _ => merged.push((start, end, 1)),
        }
    }

    merged
        .into_iter()
        .map(|(va_start, va_end, blocks)| {
            let subject = executable_range_subject(&report.bank, va_start, va_end);
            let contradicted = matches!(
                facts.conclusion(&subject).map(|conclusion| conclusion.state),
                Some(ProofState::Rejected | ProofState::Conflict)
            );
            let range = facts.insert(Fact::ExecutableRange {
                bank: report.bank.clone(),
                va_start,
                va_end,
            });
            let evidence = facts.insert(Fact::Evidence {
                subject: BankAddr::new(&report.bank, va_start),
                note: format!(
                    "executable [0x{va_start:08x},0x{va_end:08x}): union of {blocks} reached proven-code block(s) from authoritative-entry CFG closure"
                ),
            });
            let (state, rule) = if contradicted {
                (
                    ProofState::Conflict,
                    "reached_proven_code_closure_contradicted",
                )
            } else {
                (ProofState::Proven, "reached_proven_code_closure")
            };
            facts
                .conclude(subject, state, vec![range, evidence], rule)
                .expect("reached-code executable conclusions are monotonic by construction");
            DerivedExecutableRange {
                va_start,
                va_end,
                state,
            }
        })
        .collect()
}

fn validate_block(block: &BasicBlock, cfg: &Cfg, blockers: &mut BTreeSet<BlockProofBlocker>) {
    if !block.start_va.is_multiple_of(4)
        || !block.end_va.is_multiple_of(4)
        || block.end_va <= block.start_va
    {
        blockers.insert(BlockProofBlocker::InvalidGeometry);
        return;
    }
    for pc in (block.start_va..block.end_va).step_by(4) {
        let class = cfg.word_class.get(&pc).copied();
        if class != Some(WordClass::ProvenCode) {
            blockers.insert(BlockProofBlocker::WordNotProvenCode { pc, class });
        }
    }
    match block.terminator {
        BlockTerminator::InvalidInstruction { pc, word } => {
            blockers.insert(BlockProofBlocker::InvalidInstruction { pc, word });
        }
        BlockTerminator::MissingDelaySlot { control_pc } => {
            blockers.insert(BlockProofBlocker::MissingDelaySlot { control_pc });
        }
        BlockTerminator::RanOffEnd => {
            blockers.insert(BlockProofBlocker::RanOffEnd);
        }
        _ => {}
    }
}

fn block_backings(facts: &FactDb, bank: &str, start: u32, end: u32) -> Vec<Backing> {
    let mut out = BTreeSet::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank: mapped_bank,
            rom_space,
            rom_start,
            va_start,
            va_end,
            ..
        } = fact
        else {
            continue;
        };
        if mapped_bank == bank
            && start >= *va_start
            && end <= *va_end
            && va_end.checked_sub(*va_start) == fact_rom_len(fact)
        {
            let offset = start - *va_start;
            let Some(backing_start) = rom_start.checked_add(offset) else {
                continue;
            };
            let Some(backing_end) = backing_start.checked_add(end - start) else {
                continue;
            };
            out.insert(Backing {
                space: *rom_space,
                start: backing_start,
                end: backing_end,
            });
        }
    }
    out.into_iter().collect()
}

fn fact_rom_len(fact: &Fact) -> Option<u32> {
    let Fact::RomMapping {
        rom_start, rom_end, ..
    } = fact
    else {
        return None;
    };
    rom_end.checked_sub(*rom_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0x8000_0000;

    fn proven_block(start_va: u32, end_va: u32) -> BlockAssessment {
        BlockAssessment::Proven {
            block: ReachableCodeBlock {
                bank: "bank".into(),
                start_va,
                end_va,
                owner_root: BASE,
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x1000 + (start_va - BASE),
                rom_end: 0x1000 + (end_va - BASE),
                terminator: BlockTerminator::Return,
            },
        }
    }

    fn report_of(assessments: Vec<BlockAssessment>) -> BlockProofReport {
        let proven_blocks = assessments
            .iter()
            .filter(|assessment| matches!(assessment, BlockAssessment::Proven { .. }))
            .count() as u64;
        BlockProofReport {
            bank: "bank".into(),
            assessments,
            proven_blocks,
            proven_bytes: 0,
        }
    }

    #[test]
    fn reached_blocks_conclude_proven_ranges_without_bridging_gaps() {
        // Two adjacent proven blocks merge into one interval; the third sits
        // past an unreached gap and must stay a separate interval.
        let report = report_of(vec![
            proven_block(BASE, BASE + 8),
            proven_block(BASE + 8, BASE + 0x10),
            proven_block(BASE + 0x20, BASE + 0x28),
        ]);
        let mut facts = FactDb::new();
        let derived = conclude_reached_executable_ranges(&report, &mut facts);
        assert_eq!(
            derived,
            vec![
                DerivedExecutableRange {
                    va_start: BASE,
                    va_end: BASE + 0x10,
                    state: ProofState::Proven,
                },
                DerivedExecutableRange {
                    va_start: BASE + 0x20,
                    va_end: BASE + 0x28,
                    state: ProofState::Proven,
                },
            ]
        );
        let ranges = facts.proven_executable_ranges("bank");
        assert_eq!(
            ranges,
            vec![(BASE, BASE + 0x10), (BASE + 0x20, BASE + 0x28)]
        );
        // The gap byte interval is covered by no proven range.
        assert!(!ranges
            .iter()
            .any(|&(start, end)| start <= BASE + 0x10 && end > BASE + 0x10));
    }

    #[test]
    fn unproven_blocks_derive_no_executable_evidence() {
        let report = report_of(vec![BlockAssessment::Candidate {
            start_va: BASE,
            end_va: BASE + 8,
            blockers: vec![BlockProofBlocker::Unowned],
        }]);
        let mut facts = FactDb::new();
        assert!(conclude_reached_executable_ranges(&report, &mut facts).is_empty());
        assert!(facts.proven_executable_ranges("bank").is_empty());
        assert!(facts.facts().is_empty());
    }

    #[test]
    fn contradicted_range_subject_is_not_silently_promoted() {
        let report = report_of(vec![proven_block(BASE, BASE + 8)]);
        let mut facts = FactDb::new();
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + 8),
                ProofState::Rejected,
                vec![],
                "test_prior_rejection",
            )
            .unwrap();
        let derived = conclude_reached_executable_ranges(&report, &mut facts);
        assert_eq!(derived[0].state, ProofState::Conflict);
        assert!(facts.proven_executable_ranges("bank").is_empty());
        assert_eq!(
            facts
                .conclusion(&executable_range_subject("bank", BASE, BASE + 8))
                .unwrap()
                .state,
            ProofState::Conflict
        );
    }
}
