//! Immutable, bank-qualified indexes over authority-rooted intra-bank control
//! transfers and exact cross-bank calls.
//!
//! This is a derived view: it does not append facts, establish callable
//! entries, or turn bounded/observed indirect targets into execution edges.
//! Cross-bank jumps remain outside this view until composition retains a typed
//! jump-authority record. Both directions are built from one canonical edge
//! set so forward and reverse queries cannot disagree. Call indexing consumes
//! only bank-qualified owner and block geometry; byte backing is deliberately
//! irrelevant to this derived graph.

use crate::cfg::{BasicBlock, BlockTerminator};
use crate::facts::{BankAddr, Fact};
use crate::owner_proof::{ExactFunctionOwner, OwnerAssessment};
use crate::resolve::{ClosureResult, IndirectProofState};
use crate::snapshot::{ProgramSnapshotV1, ValidatedComposedSnapshotsV2};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransferEdgeKind {
    Fallthrough,
    BranchTaken,
    BranchFallthrough,
    BranchLikelyTaken,
    BranchLikelyFallthrough,
    DirectCall,
    DirectCallContinuation,
    DirectLinkedBranchCall,
    LinkedBranchFallthrough,
    LocalJump,
    ExhaustiveIndirectCall,
    ExhaustiveIndirectCallContinuation,
    ExhaustiveIndirectJump,
}

impl TransferEdgeKind {
    pub fn is_call(self) -> bool {
        matches!(
            self,
            Self::DirectCall | Self::DirectLinkedBranchCall | Self::ExhaustiveIndirectCall
        )
    }

    pub fn is_call_continuation(self) -> bool {
        matches!(
            self,
            Self::DirectCallContinuation
                | Self::LinkedBranchFallthrough
                | Self::ExhaustiveIndirectCallContinuation
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferEdge {
    /// Exact instruction from which the edge originates. For implicit
    /// fallthrough this is the block's final ordinary instruction.
    pub site: BankAddr,
    /// Authority-rooted basic block containing `site`, when the edge came
    /// directly from that bank's CFG. Cross-bank facts retain the exact site
    /// even if their source block is absent from the target snapshot.
    pub source_block: Option<BankAddr>,
    pub target: BankAddr,
    pub kind: TransferEdgeKind,
}

/// A call whose source block and target entry both have exact owner proof.
/// This projection is intentionally narrower than [`TransferEdge`]: an exact
/// call site with an unresolved historical function boundary stays visible in
/// the raw transfer index but is not mislabeled as caller/callee ownership.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionCallEdge {
    pub caller: BankAddr,
    pub callee: BankAddr,
    pub site: BankAddr,
    pub source_block: BankAddr,
    pub kind: TransferEdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramTransferIndexError {
    EmptySnapshotSet,
    DuplicateBank {
        bank: String,
    },
    InvalidBankRange {
        bank: String,
    },
    CfgBankMismatch {
        expected: String,
        actual: String,
    },
    DuplicateBlockStart {
        bank: String,
        pc: u32,
    },
    InvalidBlockGeometry {
        bank: String,
        start_pc: u32,
    },
    IndirectEvidenceMismatch {
        bank: String,
        site_pc: u32,
    },
    DuplicateAuthorityCall {
        bank: String,
        site_pc: u32,
        target_pc: u32,
    },
    DuplicateProvenOwnerEntry {
        entry: BankAddr,
    },
    DuplicateProvenBlockOwner {
        block: BankAddr,
    },
}

impl std::fmt::Display for ProgramTransferIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySnapshotSet => write!(f, "cannot index an empty snapshot set"),
            Self::DuplicateBank { bank } => write!(f, "duplicate snapshot bank '{bank}'"),
            Self::InvalidBankRange { bank } => {
                write!(f, "invalid snapshot range for bank '{bank}'")
            }
            Self::CfgBankMismatch { expected, actual } => write!(
                f,
                "authority CFG bank '{actual}' does not match snapshot bank '{expected}'"
            ),
            Self::DuplicateBlockStart { bank, pc } => {
                write!(f, "duplicate authority block {bank}:0x{pc:08x}")
            }
            Self::InvalidBlockGeometry { bank, start_pc } => write!(
                f,
                "invalid authority block geometry at {bank}:0x{start_pc:08x}"
            ),
            Self::IndirectEvidenceMismatch { bank, site_pc } => write!(
                f,
                "resolved indirect terminator disagrees with exhaustive evidence at {bank}:0x{site_pc:08x}"
            ),
            Self::DuplicateAuthorityCall {
                bank,
                site_pc,
                target_pc,
            } => write!(
                f,
                "multiple authority blocks claim call {bank}:0x{site_pc:08x} -> 0x{target_pc:08x}"
            ),
            Self::DuplicateProvenOwnerEntry { entry } => write!(
                f,
                "duplicate exact owner entry {}:0x{:08x}",
                entry.bank, entry.pc
            ),
            Self::DuplicateProvenBlockOwner { block } => write!(
                f,
                "exact owners overlap at block {}:0x{:08x}",
                block.bank, block.pc
            ),
        }
    }
}

impl std::error::Error for ProgramTransferIndexError {}

#[derive(Debug, Clone, Default)]
pub struct ProgramTransferIndex {
    edges: Vec<TransferEdge>,
    outgoing: BTreeMap<BankAddr, Vec<usize>>,
    incoming: BTreeMap<BankAddr, Vec<usize>>,
    function_calls: Vec<FunctionCallEdge>,
    calls_by_caller: BTreeMap<BankAddr, Vec<usize>>,
    calls_by_callee: BTreeMap<BankAddr, Vec<usize>>,
}

impl ProgramTransferIndex {
    /// Build from byte-verified composition authority. Deserialized diagnostic
    /// snapshots deliberately cannot call this public admission boundary.
    pub fn from_validated(
        snapshots: &ValidatedComposedSnapshotsV2,
    ) -> Result<Self, ProgramTransferIndexError> {
        Self::from_snapshot_slice(snapshots.snapshots())
    }

    /// Internal composition hook. Keep this crate-private: `ProgramSnapshotV1`
    /// is a diagnostic wire type and is not independently execution authority.
    pub(crate) fn from_snapshot_slice(
        snapshots: &[ProgramSnapshotV1],
    ) -> Result<Self, ProgramTransferIndexError> {
        if snapshots.is_empty() {
            return Err(ProgramTransferIndexError::EmptySnapshotSet);
        }

        let mut banks = Vec::new();
        let mut facts = Vec::new();
        let mut owners = Vec::new();
        for snapshot in snapshots {
            facts.extend(snapshot.facts.facts());
            for bank in &snapshot.banks {
                banks.push(BankView {
                    bank: bank.input.bank.as_str(),
                    va_start: bank.input.va_start,
                    va_end: bank.input.va_end,
                    closure: &bank.authority_closure,
                });
                owners.extend(bank.owner_proof.assessments.iter().filter_map(exact_owner));
            }
        }
        derive(&banks, facts.into_iter(), owners.into_iter())
    }

    pub fn edges(&self) -> &[TransferEdge] {
        &self.edges
    }

    pub fn outgoing_from<'a>(
        &'a self,
        site: &BankAddr,
    ) -> impl Iterator<Item = &'a TransferEdge> + 'a {
        self.outgoing
            .get(site)
            .into_iter()
            .flatten()
            .map(|&index| &self.edges[index])
    }

    pub fn incoming_to<'a>(
        &'a self,
        target: &BankAddr,
    ) -> impl Iterator<Item = &'a TransferEdge> + 'a {
        self.incoming
            .get(target)
            .into_iter()
            .flatten()
            .map(|&index| &self.edges[index])
    }

    pub fn calls_from_site<'a>(
        &'a self,
        site: &BankAddr,
    ) -> impl Iterator<Item = &'a TransferEdge> + 'a {
        self.outgoing_from(site).filter(|edge| edge.kind.is_call())
    }

    pub fn call_sites_targeting<'a>(
        &'a self,
        target: &BankAddr,
    ) -> impl Iterator<Item = &'a TransferEdge> + 'a {
        self.incoming_to(target).filter(|edge| edge.kind.is_call())
    }

    pub fn function_calls(&self) -> &[FunctionCallEdge] {
        &self.function_calls
    }

    pub fn callees_of<'a>(
        &'a self,
        caller: &BankAddr,
    ) -> impl Iterator<Item = &'a FunctionCallEdge> + 'a {
        self.calls_by_caller
            .get(caller)
            .into_iter()
            .flatten()
            .map(|&index| &self.function_calls[index])
    }

    pub fn callers_of<'a>(
        &'a self,
        callee: &BankAddr,
    ) -> impl Iterator<Item = &'a FunctionCallEdge> + 'a {
        self.calls_by_callee
            .get(callee)
            .into_iter()
            .flatten()
            .map(|&index| &self.function_calls[index])
    }
}

struct BankView<'a> {
    bank: &'a str,
    va_start: u32,
    va_end: u32,
    closure: &'a ClosureResult,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeKey {
    site: BankAddr,
    target: BankAddr,
    kind: TransferEdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CallAuthorityKey {
    site: BankAddr,
    target_pc: u32,
    class: CallAuthorityClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CallAuthorityClass {
    Direct,
    ExhaustiveIndirect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CallAuthority {
    source_block: BankAddr,
    kind: TransferEdgeKind,
}

fn exact_owner(assessment: &OwnerAssessment) -> Option<&ExactFunctionOwner> {
    match assessment {
        OwnerAssessment::Proven { owner } => Some(owner),
        OwnerAssessment::Candidate { .. } | OwnerAssessment::Ambiguous { .. } => None,
    }
}

fn derive<'a>(
    banks: &[BankView<'a>],
    facts: impl Iterator<Item = &'a Fact>,
    owners: impl Iterator<Item = &'a ExactFunctionOwner>,
) -> Result<ProgramTransferIndex, ProgramTransferIndexError> {
    let mut by_name = BTreeMap::new();
    for (index, bank) in banks.iter().enumerate() {
        if bank.bank.trim().is_empty()
            || bank.va_start >= bank.va_end
            || !bank.va_start.is_multiple_of(4)
            || !bank.va_end.is_multiple_of(4)
        {
            return Err(ProgramTransferIndexError::InvalidBankRange {
                bank: bank.bank.into(),
            });
        }
        if bank.closure.cfg.bank != bank.bank {
            return Err(ProgramTransferIndexError::CfgBankMismatch {
                expected: bank.bank.into(),
                actual: bank.closure.cfg.bank.clone(),
            });
        }
        if by_name.insert(bank.bank, index).is_some() {
            return Err(ProgramTransferIndexError::DuplicateBank {
                bank: bank.bank.into(),
            });
        }
    }

    let mut canonical: BTreeMap<EdgeKey, Option<BankAddr>> = BTreeMap::new();
    let mut call_authority = BTreeMap::new();
    for bank in banks {
        add_cfg_edges(bank, &mut canonical)?;
        index_call_authority(bank, &mut call_authority)?;
    }

    // Call facts supply the target bank for cross-bank transfers. They enter
    // only when the source bank's authority closure independently contains
    // the exact transfer; broad/candidate facts therefore cannot widen this
    // index.
    for fact in facts {
        let (source, target, requested_kind) = match fact {
            Fact::DirectCall { source, target } => (source, target, TransferEdgeKind::DirectCall),
            Fact::ResolvedCall { source, target } => {
                (source, target, TransferEdgeKind::ExhaustiveIndirectCall)
            }
            _ => continue,
        };
        if !by_name.contains_key(source.bank.as_str()) {
            continue;
        }
        if !by_name.contains_key(target.bank.as_str()) {
            continue;
        }
        let target_bank = &banks[by_name[target.bank.as_str()]];
        if target.pc < target_bank.va_start
            || target.pc >= target_bank.va_end
            || !target.pc.is_multiple_of(4)
        {
            continue;
        }
        let class = match requested_kind {
            TransferEdgeKind::DirectCall => CallAuthorityClass::Direct,
            TransferEdgeKind::ExhaustiveIndirectCall => CallAuthorityClass::ExhaustiveIndirect,
            _ => unreachable!(),
        };
        let Some(authority) = call_authority.get(&CallAuthorityKey {
            site: source.clone(),
            target_pc: target.pc,
            class,
        }) else {
            continue;
        };
        insert_edge(
            &mut canonical,
            EdgeKey {
                site: source.clone(),
                target: target.clone(),
                kind: authority.kind,
            },
            Some(authority.source_block.clone()),
        );
    }

    let edges = canonical
        .into_iter()
        .map(|(key, source_block)| TransferEdge {
            site: key.site,
            source_block,
            target: key.target,
            kind: key.kind,
        })
        .collect::<Vec<_>>();
    let mut outgoing: BTreeMap<BankAddr, Vec<usize>> = BTreeMap::new();
    let mut incoming: BTreeMap<BankAddr, Vec<usize>> = BTreeMap::new();
    for (index, edge) in edges.iter().enumerate() {
        outgoing.entry(edge.site.clone()).or_default().push(index);
        incoming.entry(edge.target.clone()).or_default().push(index);
    }
    let mut owner_by_entry = BTreeMap::new();
    let mut owner_by_block = BTreeMap::new();
    for owner in owners {
        if owner_by_entry
            .insert(owner.entry.clone(), owner.entry.clone())
            .is_some()
        {
            return Err(ProgramTransferIndexError::DuplicateProvenOwnerEntry {
                entry: owner.entry.clone(),
            });
        }
        for &block_pc in &owner.block_starts {
            let block = BankAddr::new(owner.entry.bank.as_str(), block_pc);
            if owner_by_block
                .insert(block.clone(), owner.entry.clone())
                .is_some()
            {
                return Err(ProgramTransferIndexError::DuplicateProvenBlockOwner { block });
            }
        }
    }
    let mut function_calls = edges
        .iter()
        .filter(|edge| edge.kind.is_call())
        .filter_map(|edge| {
            let source_block = edge.source_block.as_ref()?;
            let caller = owner_by_block.get(source_block)?;
            let callee = owner_by_entry.get(&edge.target)?;
            Some(FunctionCallEdge {
                caller: caller.clone(),
                callee: callee.clone(),
                site: edge.site.clone(),
                source_block: source_block.clone(),
                kind: edge.kind,
            })
        })
        .collect::<Vec<_>>();
    function_calls.sort();
    function_calls.dedup();
    let mut calls_by_caller: BTreeMap<BankAddr, Vec<usize>> = BTreeMap::new();
    let mut calls_by_callee: BTreeMap<BankAddr, Vec<usize>> = BTreeMap::new();
    for (index, edge) in function_calls.iter().enumerate() {
        calls_by_caller
            .entry(edge.caller.clone())
            .or_default()
            .push(index);
        calls_by_callee
            .entry(edge.callee.clone())
            .or_default()
            .push(index);
    }
    Ok(ProgramTransferIndex {
        edges,
        outgoing,
        incoming,
        function_calls,
        calls_by_caller,
        calls_by_callee,
    })
}

fn add_cfg_edges(
    bank: &BankView<'_>,
    edges: &mut BTreeMap<EdgeKey, Option<BankAddr>>,
) -> Result<(), ProgramTransferIndexError> {
    let cfg = &bank.closure.cfg;
    let mut block_starts = BTreeSet::new();
    for block in &cfg.blocks {
        // `DataFence` and `SelfReferentialBranch` are the two terminators
        // descent can produce with zero decoded words: both refuse to read
        // anything before ending the block, so `start_va == end_va` is valid
        // geometry for them alone.
        let fenced_at_start = match block.terminator {
            BlockTerminator::DataFence { at } | BlockTerminator::SelfReferentialBranch { at } => {
                at == block.start_va
            }
            _ => false,
        };
        let valid_zero_length_fence = block.start_va == block.end_va && fenced_at_start;
        if block.start_va < bank.va_start
            || block.end_va > bank.va_end
            || (block.start_va >= block.end_va && !valid_zero_length_fence)
            || !block.start_va.is_multiple_of(4)
            || !block.end_va.is_multiple_of(4)
        {
            return Err(ProgramTransferIndexError::InvalidBlockGeometry {
                bank: bank.bank.into(),
                start_pc: block.start_va,
            });
        }
        if !block_starts.insert(block.start_va) {
            return Err(ProgramTransferIndexError::DuplicateBlockStart {
                bank: bank.bank.into(),
                pc: block.start_va,
            });
        }
    }

    for block in &cfg.blocks {
        let source_block = BankAddr::new(bank.bank, block.start_va);
        let mut add = |site: u32, target: u32, kind: TransferEdgeKind| {
            // A raw VA is not a bank identity. Local CFG edges are admitted
            // only when the target is an exact block in the same authority
            // closure; cross-bank identity must come from a typed call fact.
            if block_starts.contains(&target) {
                insert_edge(
                    edges,
                    EdgeKey {
                        site: BankAddr::new(bank.bank, site),
                        target: BankAddr::new(bank.bank, target),
                        kind,
                    },
                    Some(source_block.clone()),
                );
            }
        };
        match &block.terminator {
            BlockTerminator::Fallthrough { next } => add(
                fallthrough_site(bank.bank, block)?,
                *next,
                TransferEdgeKind::Fallthrough,
            ),
            BlockTerminator::Tail { target } => add(
                delay_site(bank.bank, block)?,
                *target,
                TransferEdgeKind::LocalJump,
            ),
            BlockTerminator::Call { target, next } => {
                let site = delay_site(bank.bank, block)?;
                add(site, *target, TransferEdgeKind::DirectCall);
                add(site, *next, TransferEdgeKind::DirectCallContinuation);
            }
            BlockTerminator::Branch {
                target,
                fallthrough,
                link,
            } => {
                let site = delay_site(bank.bank, block)?;
                if *link {
                    add(site, *target, TransferEdgeKind::DirectLinkedBranchCall);
                    add(
                        site,
                        *fallthrough,
                        TransferEdgeKind::LinkedBranchFallthrough,
                    );
                } else {
                    add(site, *target, TransferEdgeKind::BranchTaken);
                    add(site, *fallthrough, TransferEdgeKind::BranchFallthrough);
                }
            }
            BlockTerminator::BranchLikely {
                target,
                fallthrough,
                link,
            } => {
                let site = delay_site(bank.bank, block)?;
                if *link {
                    add(site, *target, TransferEdgeKind::DirectLinkedBranchCall);
                    add(
                        site,
                        *fallthrough,
                        TransferEdgeKind::LinkedBranchFallthrough,
                    );
                } else {
                    add(site, *target, TransferEdgeKind::BranchLikelyTaken);
                    add(
                        site,
                        *fallthrough,
                        TransferEdgeKind::BranchLikelyFallthrough,
                    );
                }
            }
            BlockTerminator::ResolvedIndirect { targets, via_call } => {
                let site = delay_site(bank.bank, block)?;
                validate_exhaustive_resolution(bank, site, targets, *via_call)?;
                let kind = if *via_call {
                    TransferEdgeKind::ExhaustiveIndirectCall
                } else {
                    TransferEdgeKind::ExhaustiveIndirectJump
                };
                for &target in targets {
                    add(site, target, kind);
                }
                if *via_call {
                    add(
                        site,
                        block.end_va,
                        TransferEdgeKind::ExhaustiveIndirectCallContinuation,
                    );
                }
            }
            BlockTerminator::Indirect { .. }
            | BlockTerminator::Return
            | BlockTerminator::Trap
            | BlockTerminator::InvalidInstruction { .. }
            | BlockTerminator::MissingDelaySlot { .. }
            | BlockTerminator::RanOffEnd
            | BlockTerminator::DataFence { .. }
            | BlockTerminator::SelfReferentialBranch { .. } => {}
        }
    }
    Ok(())
}

fn insert_edge(
    edges: &mut BTreeMap<EdgeKey, Option<BankAddr>>,
    key: EdgeKey,
    source_block: Option<BankAddr>,
) {
    edges
        .entry(key)
        .and_modify(|present| {
            if present.is_none() {
                *present = source_block.clone();
            }
        })
        .or_insert(source_block);
}

fn delay_site(bank: &str, block: &BasicBlock) -> Result<u32, ProgramTransferIndexError> {
    block
        .end_va
        .checked_sub(8)
        .filter(|site| *site >= block.start_va)
        .ok_or_else(|| ProgramTransferIndexError::InvalidBlockGeometry {
            bank: bank.into(),
            start_pc: block.start_va,
        })
}

fn fallthrough_site(bank: &str, block: &BasicBlock) -> Result<u32, ProgramTransferIndexError> {
    block
        .end_va
        .checked_sub(4)
        .filter(|site| *site >= block.start_va)
        .ok_or_else(|| ProgramTransferIndexError::InvalidBlockGeometry {
            bank: bank.into(),
            start_pc: block.start_va,
        })
}

fn validate_exhaustive_resolution(
    bank: &BankView<'_>,
    site: u32,
    targets: &[u32],
    via_call: bool,
) -> Result<(), ProgramTransferIndexError> {
    let canonical_targets = targets.iter().copied().collect::<BTreeSet<_>>();
    let matching = bank
        .closure
        .indirect
        .iter()
        .filter(|resolution| resolution.site_pc == site && resolution.via_call == via_call)
        .collect::<Vec<_>>();
    let valid = canonical_targets.len() == targets.len()
        && matching.len() == 1
        && matching[0].state == IndirectProofState::Exhaustive
        && matching[0].targets.iter().copied().collect::<BTreeSet<_>>() == canonical_targets
        && matching[0].targets.len() == canonical_targets.len();
    if valid {
        Ok(())
    } else {
        Err(ProgramTransferIndexError::IndirectEvidenceMismatch {
            bank: bank.bank.into(),
            site_pc: site,
        })
    }
}

fn index_call_authority(
    bank: &BankView<'_>,
    authority: &mut BTreeMap<CallAuthorityKey, CallAuthority>,
) -> Result<(), ProgramTransferIndexError> {
    for block in &bank.closure.cfg.blocks {
        let source_block = BankAddr::new(bank.bank, block.start_va);
        let site = match &block.terminator {
            BlockTerminator::Call { .. }
            | BlockTerminator::Branch { link: true, .. }
            | BlockTerminator::BranchLikely { link: true, .. }
            | BlockTerminator::ResolvedIndirect { via_call: true, .. } => {
                delay_site(bank.bank, block)?
            }
            _ => continue,
        };
        let (targets, class, kind): (&[u32], _, _) = match &block.terminator {
            BlockTerminator::Call { target, .. } => (
                std::slice::from_ref(target),
                CallAuthorityClass::Direct,
                TransferEdgeKind::DirectCall,
            ),
            BlockTerminator::Branch {
                target, link: true, ..
            }
            | BlockTerminator::BranchLikely {
                target, link: true, ..
            } => (
                std::slice::from_ref(target),
                CallAuthorityClass::Direct,
                TransferEdgeKind::DirectLinkedBranchCall,
            ),
            BlockTerminator::ResolvedIndirect {
                targets,
                via_call: true,
            } => (
                targets.as_slice(),
                CallAuthorityClass::ExhaustiveIndirect,
                TransferEdgeKind::ExhaustiveIndirectCall,
            ),
            _ => unreachable!(),
        };
        for &target_pc in targets {
            let key = CallAuthorityKey {
                site: BankAddr::new(bank.bank, site),
                target_pc,
                class,
            };
            let value = CallAuthority {
                source_block: source_block.clone(),
                kind,
            };
            if authority.insert(key, value).is_some() {
                return Err(ProgramTransferIndexError::DuplicateAuthorityCall {
                    bank: bank.bank.into(),
                    site_pc: site,
                    target_pc,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Cfg, IndirectSite, WordClass};
    use crate::facts::{BankBackingSpanV1, FactDb, RomAddressSpace};
    use crate::owner_proof::OwnerFrontier;
    use crate::resolve::{IndirectResolution, IndirectResolutionKind};

    const A: u32 = 0x8000_0000;
    const B: u32 = 0x8020_0000;

    fn cfg(bank: &str, blocks: Vec<BasicBlock>) -> Cfg {
        Cfg {
            bank: bank.into(),
            word_class: BTreeMap::from([(A, WordClass::ProvenCode)]),
            blocks,
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::<IndirectSite>::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            rejected_transfer_targets: Vec::new(),
            proven_roots: Vec::new(),
        }
    }

    fn closure(bank: &str, blocks: Vec<BasicBlock>) -> ClosureResult {
        ClosureResult {
            cfg: cfg(bank, blocks),
            indirect: Vec::new(),
        }
    }

    fn owner(bank: &str, entry: u32, block_starts: Vec<u32>) -> ExactFunctionOwner {
        ExactFunctionOwner {
            entry: BankAddr::new(bank, entry),
            va_end: entry + 0x20,
            backing: BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0,
                rom_end: 0x20,
            },
            block_starts,
        }
    }

    #[test]
    fn forward_and_reverse_are_exact_inverses() {
        let closure = closure(
            "a",
            vec![
                BasicBlock {
                    start_va: A,
                    end_va: A + 8,
                    terminator: BlockTerminator::Fallthrough { next: A + 8 },
                },
                BasicBlock {
                    start_va: A + 8,
                    end_va: A + 16,
                    terminator: BlockTerminator::Return,
                },
            ],
        );
        let banks = [BankView {
            bank: "a",
            va_start: A,
            va_end: A + 0x100,
            closure: &closure,
        }];
        let index = derive(&banks, std::iter::empty(), std::iter::empty()).unwrap();
        for edge in index.edges() {
            assert!(index.outgoing_from(&edge.site).any(|found| found == edge));
            assert!(index.incoming_to(&edge.target).any(|found| found == edge));
        }
        assert_eq!(index.edges().len(), 1);
    }

    #[test]
    fn cross_bank_direct_and_resolved_calls_require_source_cfg_authority() {
        let direct_site = A;
        let resolved_site = A + 8;
        let mut source = closure(
            "a",
            vec![
                BasicBlock {
                    start_va: A,
                    end_va: A + 8,
                    terminator: BlockTerminator::Call {
                        target: B,
                        next: A + 8,
                    },
                },
                BasicBlock {
                    start_va: A + 8,
                    end_va: A + 16,
                    terminator: BlockTerminator::ResolvedIndirect {
                        targets: vec![B + 8],
                        via_call: true,
                    },
                },
            ],
        );
        source.indirect.push(IndirectResolution {
            site_pc: resolved_site,
            via_call: true,
            state: IndirectProofState::Exhaustive,
            kind: Some(IndirectResolutionKind::Constant),
            targets: vec![B + 8],
            memory_sources: Vec::new(),
        });
        let target = closure(
            "b",
            vec![
                BasicBlock {
                    start_va: B,
                    end_va: B + 8,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: B + 8,
                    end_va: B + 16,
                    terminator: BlockTerminator::Return,
                },
            ],
        );
        let banks = [
            BankView {
                bank: "a",
                va_start: A,
                va_end: A + 0x100,
                closure: &source,
            },
            BankView {
                bank: "b",
                va_start: B,
                va_end: B + 0x100,
                closure: &target,
            },
        ];
        let facts = [
            Fact::DirectCall {
                source: BankAddr::new("a", direct_site),
                target: BankAddr::new("b", B),
            },
            Fact::ResolvedCall {
                source: BankAddr::new("a", resolved_site),
                target: BankAddr::new("b", B + 8),
            },
        ];
        let mut owners = [
            owner("a", A, vec![A, A + 8]),
            owner("b", B, vec![B]),
            owner("b", B + 8, vec![B + 8]),
        ];
        // Caller/callee ownership is bank-qualified control-flow geometry;
        // materialized byte provenance cannot change this graph.
        owners[0].backing = BankBackingSpanV1::Materialized {
            receipt_sha256: "11".repeat(32),
            output_start: 0,
            output_end: 0x20,
        };
        let index = derive(&banks, facts.iter(), owners.iter()).unwrap();
        assert_eq!(
            index.call_sites_targeting(&BankAddr::new("b", B)).count(),
            1
        );
        assert_eq!(
            index
                .call_sites_targeting(&BankAddr::new("b", B + 8))
                .count(),
            1
        );
        assert_eq!(
            index
                .calls_from_site(&BankAddr::new("a", direct_site))
                .count(),
            1
        );
        assert_eq!(
            index
                .calls_from_site(&BankAddr::new("a", resolved_site))
                .count(),
            1
        );
        let forward = index
            .callees_of(&BankAddr::new("a", A))
            .map(|edge| (edge.site.pc, edge.callee.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            forward,
            vec![
                (direct_site, BankAddr::new("b", B)),
                (resolved_site, BankAddr::new("b", B + 8)),
            ]
        );
        assert_eq!(
            index
                .callers_of(&BankAddr::new("b", B + 8))
                .map(|edge| edge.caller.clone())
                .collect::<Vec<_>>(),
            vec![BankAddr::new("a", A)]
        );
        for edge in index.function_calls() {
            assert!(index.callees_of(&edge.caller).any(|found| found == edge));
            assert!(index.callers_of(&edge.callee).any(|found| found == edge));
        }

        let reordered_banks = [
            BankView {
                bank: "b",
                va_start: B,
                va_end: B + 0x100,
                closure: &target,
            },
            BankView {
                bank: "a",
                va_start: A,
                va_end: A + 0x100,
                closure: &source,
            },
        ];
        let reordered_facts = [facts[1].clone(), facts[0].clone()];
        let reordered_owners = [owners[2].clone(), owners[1].clone(), owners[0].clone()];
        let reordered = derive(
            &reordered_banks,
            reordered_facts.iter(),
            reordered_owners.iter(),
        )
        .unwrap();
        assert_eq!(reordered.edges(), index.edges());
        assert_eq!(reordered.function_calls(), index.function_calls());

        let no_exact_owners: [ExactFunctionOwner; 0] = [];
        let raw_only = derive(&banks, facts.iter(), no_exact_owners.iter()).unwrap();
        assert_eq!(
            raw_only
                .edges()
                .iter()
                .filter(|edge| edge.kind.is_call())
                .count(),
            2
        );
        assert!(raw_only.function_calls().is_empty());
    }

    #[test]
    fn bounded_open_and_observed_targets_never_inflate_authority() {
        let site = A;
        let mut closure = closure(
            "a",
            vec![BasicBlock {
                start_va: A,
                end_va: A + 8,
                terminator: BlockTerminator::Indirect { via_call: true },
            }],
        );
        closure.indirect.push(IndirectResolution {
            site_pc: site,
            via_call: true,
            state: IndirectProofState::Bounded,
            kind: Some(IndirectResolutionKind::MemoryValueSet),
            targets: vec![A + 0x40],
            memory_sources: vec![A + 0x80],
        });
        closure.cfg.indirect_sites.push(IndirectSite {
            pc: site,
            via_call: true,
        });
        let banks = [BankView {
            bank: "a",
            va_start: A,
            va_end: A + 0x100,
            closure: &closure,
        }];
        let facts = [
            Fact::ObservedIndirectTarget {
                site: BankAddr::new("a", site),
                target: BankAddr::new("a", A + 0x40),
                trace: "diagnostic".into(),
            },
            Fact::ResolvedCall {
                source: BankAddr::new("a", site),
                target: BankAddr::new("a", A + 0x40),
            },
        ];
        let candidate_only_owners: [ExactFunctionOwner; 0] = [];
        let index = derive(&banks, facts.iter(), candidate_only_owners.iter()).unwrap();
        assert!(index.edges().is_empty());
        assert_eq!(
            index
                .call_sites_targeting(&BankAddr::new("a", A + 0x40))
                .count(),
            0
        );
        assert!(index.function_calls().is_empty());

        // Keep this type use explicit: no FactDb mutation or conclusion is
        // part of index construction.
        let facts_before = FactDb::new();
        assert!(facts_before.facts().is_empty());
    }

    #[test]
    fn candidate_and_ambiguous_assessments_do_not_create_function_edges() {
        let source = closure(
            "a",
            vec![BasicBlock {
                start_va: A,
                end_va: A + 8,
                terminator: BlockTerminator::Call {
                    target: B,
                    next: A + 8,
                },
            }],
        );
        let target = closure(
            "b",
            vec![BasicBlock {
                start_va: B,
                end_va: B + 8,
                terminator: BlockTerminator::Return,
            }],
        );
        let banks = [
            BankView {
                bank: "a",
                va_start: A,
                va_end: A + 0x100,
                closure: &source,
            },
            BankView {
                bank: "b",
                va_start: B,
                va_end: B + 0x100,
                closure: &target,
            },
        ];
        let facts = [Fact::DirectCall {
            source: BankAddr::new("a", A),
            target: BankAddr::new("b", B),
        }];
        let assessments = [
            OwnerAssessment::Candidate {
                frontier: OwnerFrontier {
                    entry: BankAddr::new("a", A),
                    proposed_va_end: Some(A + 8),
                    blockers: Vec::new(),
                },
            },
            OwnerAssessment::Ambiguous {
                frontier: OwnerFrontier {
                    entry: BankAddr::new("b", B),
                    proposed_va_end: Some(B + 8),
                    blockers: Vec::new(),
                },
            },
        ];
        let owners = assessments.iter().filter_map(exact_owner);
        let index = derive(&banks, facts.iter(), owners).unwrap();

        assert_eq!(index.calls_from_site(&BankAddr::new("a", A)).count(), 1);
        assert!(index.function_calls().is_empty());
    }

    #[test]
    fn cross_bank_jump_is_omitted_without_typed_jump_authority() {
        let source = closure(
            "a",
            vec![BasicBlock {
                start_va: A,
                end_va: A + 8,
                terminator: BlockTerminator::Tail { target: B },
            }],
        );
        let target = closure(
            "b",
            vec![BasicBlock {
                start_va: B,
                end_va: B + 8,
                terminator: BlockTerminator::Return,
            }],
        );
        let banks = [
            BankView {
                bank: "a",
                va_start: A,
                va_end: A + 0x100,
                closure: &source,
            },
            BankView {
                bank: "b",
                va_start: B,
                va_end: B + 0x100,
                closure: &target,
            },
        ];
        let index = derive(&banks, std::iter::empty(), std::iter::empty()).unwrap();

        assert!(index.edges().is_empty());
        assert_eq!(index.incoming_to(&BankAddr::new("b", B)).count(), 0);
    }
}
