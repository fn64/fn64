//! Conservative exact function-owner proof.
//!
//! [`crate::partition`] assigns reachable blocks to roots, but a partition is
//! not by itself an exact function boundary. A root may have been seeded by a
//! detector, a contiguous span may lack proven executable/ROM backing, an
//! incoming edge may expose an interior callable entry, or an unresolved
//! indirect transfer may still enter the span. This module is the type-level
//! boundary between that useful candidate geometry and metadata an emitter is
//! allowed to consume as exact.
//!
//! The proof is intentionally strict. In particular, every unresolved
//! indirect site in the bank blocks exact ownership until value-set analysis
//! either closes it or proves its target domain cannot enter the owner. The
//! current fact model has no exclusion-domain fact, so accepting an owner in
//! the presence of such a site would be an audit assumption rather than a
//! mechanical proof. See `docs/DISCOVER-OWNER-PROOF.md`.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg, WordClass};
use crate::facts::{
    function_entry_subject, BankAddr, Fact, FactDb, IndirectTransferState, ProofState,
    RomAddressSpace,
};
use crate::partition::{same_bank_overlaps, Owner, Partition};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A ROM-backed extent that passed every exact-owner proof rule. Candidate
/// geometry cannot be converted into this type by this module's public API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactFunctionOwner {
    pub entry: BankAddr,
    pub va_end: u32,
    pub rom_space: RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
    pub block_starts: Vec<u32>,
}

impl ExactFunctionOwner {
    pub fn byte_len(&self) -> u32 {
        self.va_end - self.entry.pc
    }
}

/// A control-flow edge whose source is not owned by the function it enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncomingEdgeKind {
    Fallthrough,
    Branch,
    BranchLikely,
    Tail,
    DirectCall,
    ResolvedJump,
    ResolvedCall,
}

/// Whether an unresolved indirect site lies inside the proposed owner or
/// elsewhere in the same active bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectScope {
    Owner,
    Bank,
}

/// A machine-readable reason candidate geometry did not become exact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerBlocker {
    BankNotProven,
    PartitionBankMismatch {
        partition_bank: String,
    },
    EntryNotAuthoritative,
    OwnerMissing,
    DuplicateOwner,
    OwnerBankMismatch {
        owner_bank: String,
    },
    OwnerNotContiguous,
    PartitionAmbiguity {
        block_start: u32,
    },
    PartitionOverlap {
        other_root: u32,
    },
    DuplicateCfgBlockStart {
        block_start: u32,
    },
    MissingCfgBlock {
        block_start: u32,
    },
    MalformedBlock {
        block_start: u32,
        block_end: u32,
    },
    InconsistentTerminator {
        block_start: u32,
    },
    RanOffEnd {
        block_start: u32,
    },
    InvalidInstruction {
        pc: u32,
        word: u32,
    },
    MissingDelaySlot {
        control_pc: u32,
    },
    WordNotProvenCode {
        pc: u32,
        class: Option<WordClass>,
    },
    MissingRomBacking,
    MultipleRomBackings,
    NotProvenExecutable,
    InteriorCallableEntry {
        pc: u32,
    },
    /// An unrefuted (candidate/supported) function-entry claim lies strictly
    /// inside the proposed extent. The extent may span multiple historical
    /// functions — e.g. fallthrough after a call to a non-returning callee
    /// smearing into the next function's prologue — so exactness is withheld
    /// until the claim is proven (becoming an interior callable entry) or
    /// rejected.
    InteriorCandidateEntry {
        pc: u32,
    },
    /// The first bytes after the proposed end that are unreached, non-zero,
    /// and carry no function-entry claim. Such bytes are plausible code with
    /// no attributed owner boundary: the historical function may continue
    /// into them (compilers emit unreachable tails), and byte-identical
    /// neighborhoods have been measured with opposite ground-truth
    /// attributions, so no content rule can decide this. The right boundary
    /// stays unproven.
    TrailingUnattributedCode {
        pc: u32,
        word: u32,
    },
    IncomingEdge {
        source: u32,
        target: u32,
        edge: IncomingEdgeKind,
    },
    ObservedInteriorEntry {
        site: u32,
        target: u32,
    },
    ResolvedJumpLeavesOwner {
        site: u32,
        target: u32,
    },
    ResolvedIndirectNotExhaustive {
        site: u32,
    },
    ResolvedIndirectEvidenceMismatch {
        site: u32,
    },
    UnresolvedIndirect {
        site: u32,
        scope: IndirectScope,
    },
}

impl OwnerBlocker {
    fn is_ambiguity(&self) -> bool {
        matches!(
            self,
            Self::PartitionBankMismatch { .. }
                | Self::DuplicateOwner
                | Self::OwnerBankMismatch { .. }
                | Self::PartitionAmbiguity { .. }
                | Self::PartitionOverlap { .. }
                | Self::DuplicateCfgBlockStart { .. }
                | Self::MissingCfgBlock { .. }
                | Self::MalformedBlock { .. }
                | Self::InconsistentTerminator { .. }
                | Self::MultipleRomBackings
                | Self::InteriorCallableEntry { .. }
                | Self::IncomingEdge { .. }
                | Self::ObservedInteriorEntry { .. }
                | Self::ResolvedJumpLeavesOwner { .. }
                | Self::ResolvedIndirectEvidenceMismatch { .. }
        )
    }
}

/// Non-authoritative geometry retained for the next analysis pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerFrontier {
    pub entry: BankAddr,
    pub proposed_va_end: Option<u32>,
    pub blockers: Vec<OwnerBlocker>,
}

/// Only `Proven` carries an [`ExactFunctionOwner`]. Consumers cannot
/// accidentally treat a candidate end address as an emitter-ready extent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OwnerAssessment {
    Proven { owner: ExactFunctionOwner },
    Candidate { frontier: OwnerFrontier },
    Ambiguous { frontier: OwnerFrontier },
}

impl OwnerAssessment {
    pub fn entry(&self) -> &BankAddr {
        match self {
            Self::Proven { owner } => &owner.entry,
            Self::Candidate { frontier } | Self::Ambiguous { frontier } => &frontier.entry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerProofReport {
    pub bank: String,
    pub assessments: Vec<OwnerAssessment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Backing {
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
}

/// Prove exact extents for every root represented by `cfg` or `partition`.
///
/// `cfg` and `partition` must describe the same bank, and `image_bytes` at
/// `image_va_start` must be the same materialized image the CFG was built
/// from — the right-boundary rule inspects the unreached bytes immediately
/// after each proposed extent, which the CFG itself never decoded.
/// Function-entry authority comes only from a `Proven` fact conclusion, a
/// direct call from proven code, or an exhaustive computed call represented
/// in the CFG.
pub fn prove_exact_owners(
    cfg: &Cfg,
    partition: &Partition,
    facts: &FactDb,
    image_bytes: &[u8],
    image_va_start: u32,
) -> OwnerProofReport {
    let mut roots: BTreeSet<u32> = cfg.proven_roots.iter().copied().collect();
    roots.extend(partition.owners.iter().map(|owner| owner.root_va));
    roots.extend(
        partition
            .ambiguous
            .iter()
            .flat_map(|block| block.claimants.iter().copied()),
    );

    let mut blocks_by_start: BTreeMap<u32, &BasicBlock> = BTreeMap::new();
    let mut duplicate_blocks = BTreeSet::new();
    for block in &cfg.blocks {
        if blocks_by_start.insert(block.start_va, block).is_some() {
            duplicate_blocks.insert(block.start_va);
        }
    }

    let owner_of_block: BTreeMap<u32, u32> = partition
        .owners
        .iter()
        .flat_map(|owner| {
            owner
                .block_starts
                .iter()
                .map(move |&start| (start, owner.root_va))
        })
        .collect();
    let proven_entries: BTreeSet<u32> = facts
        .proven_function_entries(&cfg.bank)
        .into_iter()
        .collect();
    // Every unrefuted entry claim (candidate, supported, or proven). A claim
    // attributes an owner boundary without itself proving an owner.
    let entry_claims: BTreeSet<u32> = facts
        .candidate_function_entries(&cfg.bank)
        .into_iter()
        .chain(proven_entries.iter().copied())
        .collect();
    let bank_is_proven = facts
        .conclusion(&format!("bank:{}", cfg.bank))
        .is_some_and(|conclusion| conclusion.state == ProofState::Proven);
    let overlaps = same_bank_overlaps(partition, cfg);

    let assessments = roots
        .into_iter()
        .map(|root| {
            assess_root(
                root,
                cfg,
                partition,
                facts,
                &blocks_by_start,
                &duplicate_blocks,
                &owner_of_block,
                &proven_entries,
                &entry_claims,
                bank_is_proven,
                &overlaps,
                image_bytes,
                image_va_start,
            )
        })
        .collect();

    OwnerProofReport {
        bank: cfg.bank.clone(),
        assessments,
    }
}

#[allow(clippy::too_many_arguments)]
fn assess_root(
    root: u32,
    cfg: &Cfg,
    partition: &Partition,
    facts: &FactDb,
    blocks_by_start: &BTreeMap<u32, &BasicBlock>,
    duplicate_blocks: &BTreeSet<u32>,
    owner_of_block: &BTreeMap<u32, u32>,
    proven_entries: &BTreeSet<u32>,
    entry_claims: &BTreeSet<u32>,
    bank_is_proven: bool,
    overlaps: &[(u32, u32)],
    image_bytes: &[u8],
    image_va_start: u32,
) -> OwnerAssessment {
    let entry = BankAddr::new(&cfg.bank, root);
    let mut blockers = BTreeSet::new();

    if !bank_is_proven {
        blockers.insert(OwnerBlocker::BankNotProven);
    }
    if partition.bank != cfg.bank {
        blockers.insert(OwnerBlocker::PartitionBankMismatch {
            partition_bank: partition.bank.clone(),
        });
    }
    if !entry_is_authoritative(root, cfg, facts, blocks_by_start, proven_entries) {
        blockers.insert(OwnerBlocker::EntryNotAuthoritative);
    }

    let owners: Vec<&Owner> = partition
        .owners
        .iter()
        .filter(|owner| owner.root_va == root)
        .collect();
    if owners.is_empty() {
        blockers.insert(OwnerBlocker::OwnerMissing);
    }
    if owners.len() > 1 {
        blockers.insert(OwnerBlocker::DuplicateOwner);
    }
    let owner = owners.first().copied();
    let proposed_va_end = owner.map(|owner| owner.extent_end);

    for ambiguous in &partition.ambiguous {
        if ambiguous.claimants.contains(&root) {
            blockers.insert(OwnerBlocker::PartitionAmbiguity {
                block_start: ambiguous.block_start,
            });
        }
    }
    for &(left, right) in overlaps {
        if left == root || right == root {
            blockers.insert(OwnerBlocker::PartitionOverlap {
                other_root: if left == root { right } else { left },
            });
        }
    }

    let mut exact_backing = None;
    if let Some(owner) = owner {
        if owner.bank != cfg.bank {
            blockers.insert(OwnerBlocker::OwnerBankMismatch {
                owner_bank: owner.bank.clone(),
            });
        }
        if !owner.is_contiguous(blocks_by_start) {
            blockers.insert(OwnerBlocker::OwnerNotContiguous);
        }
        for &start in &owner.block_starts {
            if duplicate_blocks.contains(&start) {
                blockers.insert(OwnerBlocker::DuplicateCfgBlockStart { block_start: start });
            }
            let Some(block) = blocks_by_start.get(&start).copied() else {
                blockers.insert(OwnerBlocker::MissingCfgBlock { block_start: start });
                continue;
            };
            validate_block(owner, block, blocks_by_start, cfg, &mut blockers);
        }
        for pc in (root..owner.extent_end).step_by(4) {
            let class = cfg.word_class.get(&pc).copied();
            if class != Some(WordClass::ProvenCode) {
                blockers.insert(OwnerBlocker::WordNotProvenCode { pc, class });
            }
        }

        let backings = owner_backings(facts, &cfg.bank, root, owner.extent_end);
        match backings.as_slice() {
            [] => {
                blockers.insert(OwnerBlocker::MissingRomBacking);
            }
            [backing] => exact_backing = Some(*backing),
            _ => {
                blockers.insert(OwnerBlocker::MultipleRomBackings);
            }
        }
        if !interval_is_covered(
            root,
            owner.extent_end,
            &facts.proven_executable_ranges(&cfg.bank),
        ) {
            blockers.insert(OwnerBlocker::NotProvenExecutable);
        }

        for &claim in entry_claims {
            if claim > root && claim < owner.extent_end && !proven_entries.contains(&claim) {
                blockers.insert(OwnerBlocker::InteriorCandidateEntry { pc: claim });
            }
        }
        if let Some((pc, word)) = trailing_unattributed_code(
            owner.extent_end,
            cfg,
            entry_claims,
            image_bytes,
            image_va_start,
        ) {
            blockers.insert(OwnerBlocker::TrailingUnattributedCode { pc, word });
        }

        validate_incoming(
            owner,
            cfg,
            facts,
            owner_of_block,
            proven_entries,
            &mut blockers,
        );
        validate_indirects(owner, cfg, facts, &mut blockers);
    }

    if blockers.is_empty() {
        let owner = owner.expect("an exact assessment must have one partition owner");
        let backing = exact_backing.expect("an exact assessment must have one ROM backing");
        return OwnerAssessment::Proven {
            owner: ExactFunctionOwner {
                entry,
                va_end: owner.extent_end,
                rom_space: backing.rom_space,
                rom_start: backing.rom_start,
                rom_end: backing.rom_end,
                block_starts: owner.block_starts.clone(),
            },
        };
    }

    let frontier = OwnerFrontier {
        entry,
        proposed_va_end,
        blockers: blockers.iter().cloned().collect(),
    };
    if blockers.iter().any(OwnerBlocker::is_ambiguity) {
        OwnerAssessment::Ambiguous { frontier }
    } else {
        OwnerAssessment::Candidate { frontier }
    }
}

/// Walk the unreached bytes immediately after `extent_end` and return the
/// first word that no mechanism attributes: not reached proven code, not the
/// site of any function-entry claim, and not zero padding. `None` means the
/// right boundary is closed — the walk hit reached code, an entry claim, the
/// end of the materialized image, or the address-space end, with only zero
/// words in between.
fn trailing_unattributed_code(
    extent_end: u32,
    cfg: &Cfg,
    entry_claims: &BTreeSet<u32>,
    image_bytes: &[u8],
    image_va_start: u32,
) -> Option<(u32, u32)> {
    let mut va = extent_end;
    loop {
        let offset = va.checked_sub(image_va_start)? as usize;
        if offset.checked_add(4)? > image_bytes.len() {
            // Ran past the materialized image: no bytes left to attribute.
            return None;
        }
        if cfg.word_class.get(&va) == Some(&WordClass::ProvenCode) {
            return None;
        }
        if entry_claims.contains(&va) {
            return None;
        }
        let word = u32::from_be_bytes(
            image_bytes[offset..offset + 4]
                .try_into()
                .expect("four bytes were just bounds-checked"),
        );
        if word != 0 {
            return Some((va, word));
        }
        // Address-space end with only padding seen: boundary is closed.
        va = va.checked_add(4)?;
    }
}

fn entry_is_authoritative(
    root: u32,
    cfg: &Cfg,
    facts: &FactDb,
    blocks_by_start: &BTreeMap<u32, &BasicBlock>,
    proven_entries: &BTreeSet<u32>,
) -> bool {
    if proven_entries.contains(&root)
        && facts
            .conclusion(&function_entry_subject(&BankAddr::new(&cfg.bank, root)))
            .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
    {
        return true;
    }
    if cfg.direct_calls.iter().any(|&(source, target)| {
        target == root && cfg.word_class.get(&source) == Some(&WordClass::ProvenCode)
    }) {
        return true;
    }
    blocks_by_start.values().any(|block| {
        block.end_va >= block.start_va.saturating_add(8)
            && cfg.word_class.get(&(block.end_va - 8)) == Some(&WordClass::ProvenCode)
            && cfg.word_class.get(&(block.end_va - 4)) == Some(&WordClass::ProvenCode)
            && matches!(
                &block.terminator,
                BlockTerminator::ResolvedIndirect {
                    targets,
                    via_call: true
                } if targets.contains(&root)
            )
    })
}

fn owner_backings(facts: &FactDb, bank: &str, start: u32, end: u32) -> Vec<Backing> {
    let mut out = BTreeSet::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank: fact_bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        if fact_bank != bank || end <= start || start < *va_start || end > *va_end {
            continue;
        }
        let Some(backed_va_end) = va_start.checked_add(rom_end.saturating_sub(*rom_start)) else {
            continue;
        };
        if end > backed_va_end {
            continue;
        }
        let start_delta = start - va_start;
        let end_delta = end - va_start;
        let (Some(mapped_start), Some(mapped_end)) = (
            rom_start.checked_add(start_delta),
            rom_start.checked_add(end_delta),
        ) else {
            continue;
        };
        out.insert(Backing {
            rom_space: *rom_space,
            rom_start: mapped_start,
            rom_end: mapped_end,
        });
    }
    out.into_iter().collect()
}

fn interval_is_covered(start: u32, end: u32, ranges: &[(u32, u32)]) -> bool {
    if end <= start {
        return false;
    }
    let mut cursor = start;
    for &(range_start, range_end) in ranges {
        if range_end <= cursor || range_start > cursor {
            continue;
        }
        cursor = cursor.max(range_end);
        if cursor >= end {
            return true;
        }
    }
    false
}

fn validate_block(
    owner: &Owner,
    block: &BasicBlock,
    blocks_by_start: &BTreeMap<u32, &BasicBlock>,
    cfg: &Cfg,
    blockers: &mut BTreeSet<OwnerBlocker>,
) {
    let aligned = block.start_va.is_multiple_of(4) && block.end_va.is_multiple_of(4);
    if !aligned
        || block.end_va <= block.start_va
        || block.start_va < owner.root_va
        || block.end_va > owner.extent_end
    {
        blockers.insert(OwnerBlocker::MalformedBlock {
            block_start: block.start_va,
            block_end: block.end_va,
        });
        return;
    }

    let needs_delay = matches!(
        block.terminator,
        BlockTerminator::Tail { .. }
            | BlockTerminator::Call { .. }
            | BlockTerminator::Branch { .. }
            | BlockTerminator::BranchLikely { .. }
            | BlockTerminator::Return
            | BlockTerminator::Indirect { .. }
            | BlockTerminator::ResolvedIndirect { .. }
    );
    let minimum = if needs_delay { 8 } else { 4 };
    if block.end_va - block.start_va < minimum {
        blockers.insert(OwnerBlocker::MalformedBlock {
            block_start: block.start_va,
            block_end: block.end_va,
        });
        return;
    }
    if needs_delay {
        for pc in [block.end_va - 8, block.end_va - 4] {
            if cfg.word_class.get(&pc) != Some(&WordClass::ProvenCode) {
                blockers.insert(OwnerBlocker::WordNotProvenCode {
                    pc,
                    class: cfg.word_class.get(&pc).copied(),
                });
            }
        }
    }

    let owned_starts: BTreeSet<u32> = owner.block_starts.iter().copied().collect();
    let internal_target_is_block = |target: u32| {
        target >= owner.root_va
            && target < owner.extent_end
            && owned_starts.contains(&target)
            && blocks_by_start.contains_key(&target)
    };
    let consistent = match &block.terminator {
        BlockTerminator::Fallthrough { next } => {
            *next == block.end_va && internal_target_is_block(*next)
        }
        BlockTerminator::Tail { target } => {
            !(*target >= owner.root_va && *target < owner.extent_end)
                || internal_target_is_block(*target)
        }
        BlockTerminator::Call { next, .. } => {
            *next == block.end_va && internal_target_is_block(*next)
        }
        BlockTerminator::Branch {
            target,
            fallthrough,
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
        } => {
            *fallthrough == block.end_va
                && internal_target_is_block(*target)
                && internal_target_is_block(*fallthrough)
        }
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: false,
        } => {
            !targets.is_empty()
                && targets
                    .iter()
                    .all(|target| internal_target_is_block(*target))
        }
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: true,
        } => !targets.is_empty() && internal_target_is_block(block.end_va),
        BlockTerminator::Indirect { .. } => {
            blockers.insert(OwnerBlocker::UnresolvedIndirect {
                site: block.end_va - 8,
                scope: IndirectScope::Owner,
            });
            true
        }
        BlockTerminator::Return | BlockTerminator::Trap => true,
        BlockTerminator::InvalidInstruction { pc, word } => {
            blockers.insert(OwnerBlocker::InvalidInstruction {
                pc: *pc,
                word: *word,
            });
            true
        }
        BlockTerminator::MissingDelaySlot { control_pc } => {
            blockers.insert(OwnerBlocker::MissingDelaySlot {
                control_pc: *control_pc,
            });
            true
        }
        BlockTerminator::RanOffEnd => {
            blockers.insert(OwnerBlocker::RanOffEnd {
                block_start: block.start_va,
            });
            true
        }
    };
    if !consistent {
        blockers.insert(OwnerBlocker::InconsistentTerminator {
            block_start: block.start_va,
        });
    }
    if let BlockTerminator::ResolvedIndirect {
        targets,
        via_call: false,
    } = &block.terminator
    {
        let site = block.end_va - 8;
        for &target in targets {
            if target < owner.root_va || target >= owner.extent_end {
                blockers.insert(OwnerBlocker::ResolvedJumpLeavesOwner { site, target });
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_incoming(
    owner: &Owner,
    cfg: &Cfg,
    facts: &FactDb,
    owner_of_block: &BTreeMap<u32, u32>,
    proven_entries: &BTreeSet<u32>,
    blockers: &mut BTreeSet<OwnerBlocker>,
) {
    let contains = |pc: u32| pc >= owner.root_va && pc < owner.extent_end;
    for &entry in cfg.proven_roots.iter().chain(proven_entries) {
        if contains(entry) && entry != owner.root_va {
            blockers.insert(OwnerBlocker::InteriorCallableEntry { pc: entry });
        }
    }

    for fact in facts.facts() {
        match fact {
            Fact::DirectCall { source, target } => {
                let foreign_source = source.bank != owner.bank || !contains(source.pc);
                if target.bank == owner.bank
                    && contains(target.pc)
                    && target.pc != owner.root_va
                    && foreign_source
                {
                    blockers.insert(OwnerBlocker::IncomingEdge {
                        source: source.pc,
                        target: target.pc,
                        edge: IncomingEdgeKind::DirectCall,
                    });
                }
            }
            Fact::ObservedIndirectTarget { site, target, .. }
                if target.bank == owner.bank
                    && contains(target.pc)
                    && target.pc != owner.root_va =>
            {
                blockers.insert(OwnerBlocker::ObservedInteriorEntry {
                    site: site.pc,
                    target: target.pc,
                });
            }
            _ => {}
        }
    }

    for block in &cfg.blocks {
        if owner_of_block.get(&block.start_va) == Some(&owner.root_va) {
            continue;
        }
        let mut edges = Vec::new();
        match &block.terminator {
            BlockTerminator::Fallthrough { next } => {
                edges.push((*next, IncomingEdgeKind::Fallthrough));
            }
            BlockTerminator::Tail { target } => edges.push((*target, IncomingEdgeKind::Tail)),
            BlockTerminator::Call { target, next } => {
                edges.push((*target, IncomingEdgeKind::DirectCall));
                edges.push((*next, IncomingEdgeKind::Fallthrough));
            }
            BlockTerminator::Branch {
                target,
                fallthrough,
            } => {
                edges.push((*target, IncomingEdgeKind::Branch));
                edges.push((*fallthrough, IncomingEdgeKind::Fallthrough));
            }
            BlockTerminator::BranchLikely {
                target,
                fallthrough,
            } => {
                edges.push((*target, IncomingEdgeKind::BranchLikely));
                edges.push((*fallthrough, IncomingEdgeKind::Fallthrough));
            }
            BlockTerminator::ResolvedIndirect { targets, via_call } => {
                let kind = if *via_call {
                    IncomingEdgeKind::ResolvedCall
                } else {
                    IncomingEdgeKind::ResolvedJump
                };
                edges.extend(targets.iter().copied().map(|target| (target, kind)));
                if *via_call {
                    edges.push((block.end_va, IncomingEdgeKind::Fallthrough));
                }
            }
            BlockTerminator::Indirect { .. }
            | BlockTerminator::Return
            | BlockTerminator::Trap
            | BlockTerminator::InvalidInstruction { .. }
            | BlockTerminator::MissingDelaySlot { .. }
            | BlockTerminator::RanOffEnd => {}
        }
        for (target, edge) in edges {
            if !contains(target) {
                continue;
            }
            let callable_entry_edge = target == owner.root_va
                && matches!(
                    edge,
                    IncomingEdgeKind::Tail
                        | IncomingEdgeKind::DirectCall
                        | IncomingEdgeKind::ResolvedCall
                );
            if !callable_entry_edge {
                blockers.insert(OwnerBlocker::IncomingEdge {
                    source: block.start_va,
                    target,
                    edge,
                });
            }
        }
    }
}

fn validate_indirects(
    owner: &Owner,
    cfg: &Cfg,
    facts: &FactDb,
    blockers: &mut BTreeSet<OwnerBlocker>,
) {
    let scope = |site: u32| {
        if site >= owner.root_va && site < owner.extent_end {
            IndirectScope::Owner
        } else {
            IndirectScope::Bank
        }
    };
    for site in &cfg.indirect_sites {
        blockers.insert(OwnerBlocker::UnresolvedIndirect {
            site: site.pc,
            scope: scope(site.pc),
        });
    }

    for block in &cfg.blocks {
        let BlockTerminator::ResolvedIndirect {
            targets,
            via_call: cfg_via_call,
        } = &block.terminator
        else {
            continue;
        };
        if block.end_va < block.start_va.saturating_add(8) {
            continue;
        }
        let site_pc = block.end_va - 8;
        let mut exhaustive_sets = BTreeSet::new();
        for fact in facts.facts() {
            let Fact::IndirectTransferAnalysis {
                site,
                via_call,
                state,
                kind,
                targets,
                ..
            } = fact
            else {
                continue;
            };
            if site.bank == owner.bank
                && site.pc == site_pc
                && via_call == cfg_via_call
                && *state == IndirectTransferState::Exhaustive
                && kind.is_some()
                && !targets.is_empty()
            {
                let mut normalized = targets.clone();
                normalized.sort_unstable();
                normalized.dedup();
                exhaustive_sets.insert(normalized);
            }
        }
        if exhaustive_sets.is_empty() {
            blockers.insert(OwnerBlocker::ResolvedIndirectNotExhaustive { site: site_pc });
            continue;
        }
        let mut cfg_targets = targets.clone();
        cfg_targets.sort_unstable();
        cfg_targets.dedup();
        if exhaustive_sets.len() != 1 || !exhaustive_sets.contains(&cfg_targets) {
            blockers.insert(OwnerBlocker::ResolvedIndirectEvidenceMismatch { site: site_pc });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{build_cfg, build_cfg_with_indirect};
    use crate::facts::{
        CandidateDetector, FunctionEntryEvidence, IndirectTransferKind, ProloguePattern,
    };
    use crate::partition::partition;

    const BASE: u32 = 0x8000_0000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn facts_for(bytes_len: u32, entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1000 + bytes_len,
            va_start: BASE,
            va_end: BASE + bytes_len,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + bytes_len,
        });
        facts
            .conclude(
                crate::facts::executable_range_subject("bank", BASE, BASE + bytes_len),
                ProofState::Proven,
                vec![executable],
                "test_executable",
            )
            .unwrap();
        for &entry in entries {
            let target = BankAddr::new("bank", entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new("bank", entry + 4),
                },
                proposed_state: ProofState::Proven,
            });
            facts
                .conclude(
                    function_entry_subject(&target),
                    ProofState::Proven,
                    vec![claim],
                    "test_entry",
                )
                .unwrap();
        }
        facts
    }

    fn frontier(assessment: &OwnerAssessment) -> &OwnerFrontier {
        match assessment {
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                frontier
            }
            OwnerAssessment::Proven { .. } => panic!("expected unresolved frontier"),
        }
    }

    #[test]
    fn exact_owner_requires_and_carries_rom_backed_extent() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let OwnerAssessment::Proven { owner } = &report.assessments[0] else {
            panic!("expected exact owner: {:?}", report.assessments[0]);
        };
        assert_eq!(owner.entry, BankAddr::new("bank", BASE));
        assert_eq!(owner.va_end, BASE + 12);
        assert_eq!(owner.rom_start, 0x1000);
        assert_eq!(owner.rom_end, 0x100c);
        assert_eq!(owner.byte_len(), 12);
    }

    #[test]
    fn a_seeded_root_without_entry_proof_remains_candidate() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::EntryNotAuthoritative));
    }

    #[test]
    fn missing_executable_evidence_withholds_exact_extent() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let subject = crate::facts::executable_range_subject("bank", BASE, BASE + 8);
        facts
            .conclude(subject, ProofState::Conflict, vec![], "test_conflict")
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::NotProvenExecutable));
    }

    #[test]
    fn direct_call_from_proven_code_authorizes_its_target_entry() {
        let target = BASE + 0x20;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .unwrap();
        assert!(matches!(target_assessment, OwnerAssessment::Proven { .. }));
    }

    #[test]
    fn running_off_the_image_never_proves_an_extent() {
        let bytes = asm(&[NOP, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::RanOffEnd { block_start: BASE }));
    }

    #[test]
    fn invalid_instruction_is_a_typed_owner_blocker() {
        let unknown = 0x7801_2345;
        let bytes = asm(&[unknown]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::InvalidInstruction {
                pc: BASE,
                word: unknown,
            }
        ));
    }

    #[test]
    fn missing_delay_slot_is_a_typed_owner_blocker() {
        let bytes = asm(&[JR_RA]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::MissingDelaySlot { control_pc: BASE }));
    }

    #[test]
    fn distinct_rom_backings_are_ambiguous() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x2000,
            rom_end: 0x2008,
            va_start: BASE,
            va_end: BASE + 8,
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::MultipleRomBackings));
    }

    #[test]
    fn proven_external_call_to_interior_is_ambiguous() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::DirectCall {
            source: BankAddr::new("other", 0x9000_0000),
            target: BankAddr::new("bank", BASE + 4),
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::IncomingEdge {
                source: 0x9000_0000,
                target: BASE + 4,
                edge: IncomingEdgeKind::DirectCall,
            }));
    }

    #[test]
    fn unresolved_indirect_anywhere_in_bank_blocks_exact_incoming_closure() {
        let target = BASE + 0x20;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site: target,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn resolved_indirect_requires_one_matching_exhaustive_fact() {
        let jr_t9 = (25u32 << 21) | 0x08;
        let bytes = asm(&[jr_t9, NOP, JR_RA, NOP]);
        let mut targets = BTreeMap::new();
        targets.insert(BASE, vec![BASE + 8]);
        let cfg = build_cfg_with_indirect("bank", &bytes, BASE, &[BASE], &targets);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);

        let missing = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&missing.assessments[0])
            .blockers
            .contains(&OwnerBlocker::ResolvedIndirectNotExhaustive { site: BASE }));

        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: false,
            state: IndirectTransferState::Open,
            kind: None,
            targets: vec![],
            memory_sources: vec![],
        });
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: false,
            state: IndirectTransferState::Exhaustive,
            kind: Some(IndirectTransferKind::Constant),
            targets: vec![BASE + 8],
            memory_sources: vec![],
        });

        let closed = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            closed.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn competing_root_closure_is_ambiguous() {
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, BASE + 4]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE, BASE + 4]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(report
            .assessments
            .iter()
            .all(|assessment| matches!(assessment, OwnerAssessment::Ambiguous { .. })));
        assert!(report
            .assessments
            .iter()
            .any(|assessment| frontier(assessment)
                .blockers
                .iter()
                .any(|blocker| matches!(blocker, OwnerBlocker::PartitionAmbiguity { .. }))));
    }

    #[test]
    fn observed_indirect_entry_into_interior_is_ambiguous() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::ObservedIndirectTarget {
            site: BankAddr::new("other", 0x9000_0000),
            target: BankAddr::new("bank", BASE + 4),
            trace: "synthetic".into(),
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::ObservedInteriorEntry {
                site: 0x9000_0000,
                target: BASE + 4,
            }
        ));
    }

    #[test]
    fn delay_slot_must_be_proven_code() {
        let bytes = asm(&[JR_RA, NOP]);
        let mut cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        cfg.word_class.insert(BASE + 4, WordClass::CandidateCode);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::WordNotProvenCode {
                pc: BASE + 4,
                class: Some(WordClass::CandidateCode),
            }
        ));
    }

    #[test]
    fn interior_candidate_entry_claim_withholds_exactness_until_resolved() {
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let interior = BankAddr::new("bank", BASE + 8);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: interior.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: interior.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("bank", BASE + 12),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&interior),
                ProofState::Candidate,
                vec![claim],
                "test_interior_candidate",
            )
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::InteriorCandidateEntry { pc: BASE + 8 }));

        // Rejecting the claim discharges the blocker: the extent is exact.
        facts
            .conclude(
                function_entry_subject(&interior),
                ProofState::Rejected,
                vec![claim],
                "test_refuted",
            )
            .unwrap();
        let resolved = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            resolved.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn trailing_unattributed_code_blocks_the_right_boundary() {
        // The function returns at +4/+8; the unreached word after it is a
        // bare `jr $ra` with no entry claim — plausible code attributed to
        // nothing, so the proposed end stays unproven.
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Candidate { .. }
        ));
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::TrailingUnattributedCode {
                pc: BASE + 8,
                word: JR_RA,
            }
        ));

        // An entry claim at exactly the boundary attributes the trailing
        // bytes and closes the right edge.
        let neighbor = BankAddr::new("bank", BASE + 8);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: neighbor.clone(),
            detector: CandidateDetector::JalTarget,
            evidence: FunctionEntryEvidence::DirectJal {
                call_site: BankAddr::new("bank", BASE + 0x100),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&neighbor),
                ProofState::Candidate,
                vec![claim],
                "test_boundary_claim",
            )
            .unwrap();
        let closed = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            closed.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn zero_padding_to_the_image_end_keeps_the_right_boundary_closed() {
        let bytes = asm(&[JR_RA, NOP, 0, 0]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Proven { .. }
        ));
    }

    #[test]
    fn report_is_byte_deterministic() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let left = serde_json::to_vec(&prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE))
            .unwrap();
        let right = serde_json::to_vec(&prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE))
            .unwrap();
        assert_eq!(left, right);
    }
}
