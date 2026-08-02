//! Conservative exact function-owner proof.
//!
//! [`crate::partition`] assigns reachable blocks to roots, but a partition is
//! not by itself an exact function boundary. A root may have been seeded by a
//! detector, a contiguous span may lack proven executable/image backing, an
//! incoming edge may expose an interior callable entry, or an unresolved
//! indirect transfer may still enter the span. This module is the type-level
//! boundary between that useful candidate geometry and metadata an emitter is
//! allowed to consume as exact.
//!
//! The proof is intentionally strict. In snapshot composition, every
//! authority-reachable unresolved indirect site blocks exact ownership until
//! value-set analysis either closes it or proves its target domain cannot enter
//! the owner. The public API has no authority-only closure and therefore checks
//! every unresolved site in its CFG. See `docs/DISCOVER-OWNER-PROOF.md`.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg, WordClass};
use crate::facts::{
    function_entry_subject, BankAddr, BankBackingSpanResolutionV1, BankBackingSpanV1, Fact, FactDb,
    IndirectTransferState, ProofState,
};
use crate::partition::{same_bank_overlaps, Owner, Partition};
use crate::resolve::{ClosureResult, IndirectProofState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// An image-backed extent that passed every exact-owner proof rule. Candidate
/// geometry cannot be converted into this type by this module's public API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactFunctionOwner {
    pub entry: BankAddr,
    pub va_end: u32,
    pub backing: BankBackingSpanV1,
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
    MissingBankBacking,
    AmbiguousBankBacking,
    InvalidBankBackingGeometry,
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
                | Self::AmbiguousBankBacking
                | Self::InvalidBankBackingGeometry
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

/// Bank-bound owner-proof authority for snapshot composition.
///
/// The only constructor consumes the authority-only closure. Candidate calls
/// and indirect sites found solely through broad traversal therefore cannot
/// confer entry authority or add unresolved-indirect blockers during
/// snapshot's executable-evidence pass.
pub(crate) struct OwnerProofAuthority {
    bank: String,
    entries: BTreeSet<u32>,
    /// Syntactic indirect sites present in the authority-only closure. A site
    /// found solely through broad candidate traversal cannot execute from any
    /// proven root and therefore cannot block unrelated exact owners.
    indirect_sites: BTreeSet<AuthorityIndirectSite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AuthorityIndirectSite {
    pc: u32,
    via_call: bool,
}

impl AuthorityIndirectSite {
    const fn new(pc: u32, via_call: bool) -> Self {
        Self { pc, via_call }
    }
}

pub(crate) fn exact_authority_direct_call(cfg: &Cfg, block: &BasicBlock) -> Option<(u32, u32)> {
    let BlockTerminator::Call { target, .. } = &block.terminator else {
        return None;
    };
    if block.end_va < block.start_va.saturating_add(8) {
        return None;
    }
    let source_pc = block.end_va - 8;
    (cfg.word_class.get(&source_pc) == Some(&WordClass::ProvenCode)
        && cfg.word_class.get(&(source_pc + 4)) == Some(&WordClass::ProvenCode)
        && cfg.direct_calls.contains(&(source_pc, *target)))
    .then_some((source_pc, *target))
}

pub(crate) fn exhaustive_authority_call_site(
    authority_closure: &ClosureResult,
    block: &BasicBlock,
) -> Option<u32> {
    let BlockTerminator::ResolvedIndirect {
        targets,
        via_call: true,
    } = &block.terminator
    else {
        return None;
    };
    if block.end_va < block.start_va.saturating_add(8) {
        return None;
    }
    let site_pc = block.end_va - 8;
    let cfg = &authority_closure.cfg;
    if cfg.bank.is_empty()
        || cfg.word_class.get(&site_pc) != Some(&WordClass::ProvenCode)
        || cfg.word_class.get(&(site_pc + 4)) != Some(&WordClass::ProvenCode)
    {
        return None;
    }
    let mut records = authority_closure
        .indirect
        .iter()
        .filter(|record| record.site_pc == site_pc && record.via_call);
    let record = records.next()?;
    if records.next().is_some()
        || record.state != IndirectProofState::Exhaustive
        || record.kind.is_none()
        || record.targets.is_empty()
    {
        return None;
    }
    let mut cfg_targets = targets.clone();
    cfg_targets.sort_unstable();
    cfg_targets.dedup();
    let mut evidence_targets = record.targets.clone();
    evidence_targets.sort_unstable();
    evidence_targets.dedup();
    (cfg_targets == evidence_targets).then_some(site_pc)
}

impl OwnerProofAuthority {
    pub(crate) fn from_authority_closure(
        authority_closure: &ClosureResult,
        facts: &FactDb,
        external_authorized_roots: &BTreeSet<u32>,
    ) -> Self {
        let cfg = &authority_closure.cfg;
        let mut entries = BTreeSet::new();
        for root in facts.proven_function_entries(&cfg.bank) {
            if facts
                .conclusion(&function_entry_subject(&BankAddr::new(&cfg.bank, root)))
                .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
            {
                entries.insert(root);
            }
        }
        for block in &cfg.blocks {
            if let Some((_, target)) = exact_authority_direct_call(cfg, block) {
                if cfg.blocks.iter().any(|block| block.start_va == target)
                    || cfg
                        .plain_delay_entry_aliases
                        .iter()
                        .any(|alias| alias.entry_va == target)
                {
                    entries.insert(target);
                }
            }
        }
        for block in &cfg.blocks {
            if exhaustive_authority_call_site(authority_closure, block).is_none() {
                continue;
            }
            if let BlockTerminator::ResolvedIndirect {
                targets,
                via_call: true,
            } = &block.terminator
            {
                entries.extend(targets.iter().copied().filter(|target| {
                    cfg.blocks.iter().any(|block| block.start_va == *target)
                        || cfg
                            .plain_delay_entry_aliases
                            .iter()
                            .any(|alias| alias.entry_va == *target)
                }));
            }
        }
        let proven_images = facts.proven_bank_images();
        entries.extend(external_authorized_roots.iter().copied().filter(|&root| {
            root.is_multiple_of(4)
                && proven_images.iter().any(|image| {
                    image.bank == cfg.bank && image.va_start <= root && root < image.va_end
                })
        }));
        Self {
            bank: cfg.bank.clone(),
            entries,
            indirect_sites: authority_closure
                .indirect
                .iter()
                .map(|site| AuthorityIndirectSite::new(site.site_pc, site.via_call))
                .collect(),
        }
    }

    fn contains(&self, bank: &str, root: u32) -> bool {
        self.bank == bank && self.entries.contains(&root)
    }
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
    prove_exact_owners_with_external_authority(
        cfg,
        partition,
        facts,
        image_bytes,
        image_va_start,
        &BTreeSet::new(),
    )
}

/// Same as [`prove_exact_owners`], but additionally treats every VA in
/// `external_authorized_roots` as an authoritative callable entry of this
/// bank.
///
/// Each VA in the set is a target this bank's [`Cfg`] cannot see the authority
/// for: a direct `jal` living in *another* proven bank whose source word is
/// proven code and whose target lands inside this bank's proven VA range. The
/// multi-bank composer vets those two conditions before populating the set —
/// exactly the same authority rule a same-bank direct call already confers via
/// [`Cfg::direct_calls`], extended across the bank boundary. This function
/// applies no weaker rule; it trusts the caller to have proven the source, the
/// alignment, and the in-range landing, because those facts are not present in
/// this bank's own CFG.
pub fn prove_exact_owners_with_external_authority(
    cfg: &Cfg,
    partition: &Partition,
    facts: &FactDb,
    image_bytes: &[u8],
    image_va_start: u32,
    external_authorized_roots: &BTreeSet<u32>,
) -> OwnerProofReport {
    prove_exact_owners_inner(
        cfg,
        partition,
        facts,
        image_bytes,
        image_va_start,
        external_authorized_roots,
        None,
    )
}

pub(crate) fn prove_exact_owners_with_authority(
    cfg: &Cfg,
    partition: &Partition,
    facts: &FactDb,
    image_bytes: &[u8],
    image_va_start: u32,
    authority: &OwnerProofAuthority,
) -> OwnerProofReport {
    prove_exact_owners_inner(
        cfg,
        partition,
        facts,
        image_bytes,
        image_va_start,
        &BTreeSet::new(),
        Some(authority),
    )
}

#[allow(clippy::too_many_arguments)]
fn prove_exact_owners_inner(
    cfg: &Cfg,
    partition: &Partition,
    facts: &FactDb,
    image_bytes: &[u8],
    image_va_start: u32,
    external_authorized_roots: &BTreeSet<u32>,
    owner_proof_authority: Option<&OwnerProofAuthority>,
) -> OwnerProofReport {
    let mut roots: BTreeSet<u32> = cfg.proven_roots.iter().copied().collect();
    roots.extend(partition.owners.iter().map(|owner| owner.root_va));
    roots.extend(
        partition
            .ambiguous
            .iter()
            .flat_map(|block| block.claimants.iter().copied()),
    );
    roots.extend(external_authorized_roots.iter().copied());
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
    let indirect_target_domains = collect_indirect_target_domains(facts, &cfg.bank);

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
                external_authorized_roots,
                owner_proof_authority,
                &indirect_target_domains,
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
    external_authorized_roots: &BTreeSet<u32>,
    owner_proof_authority: Option<&OwnerProofAuthority>,
    indirect_target_domains: &BTreeMap<(u32, bool), BTreeSet<u32>>,
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
    let entry_authoritative = owner_proof_authority.map_or_else(
        || {
            external_authorized_roots.contains(&root)
                || entry_is_authoritative(root, cfg, facts, blocks_by_start, proven_entries)
        },
        |authority| authority.contains(&cfg.bank, root),
    );
    if !entry_authoritative {
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

        match facts.resolve_proven_bank_backing_span(&cfg.bank, root, owner.extent_end) {
            BankBackingSpanResolutionV1::Missing => {
                blockers.insert(OwnerBlocker::MissingBankBacking);
            }
            BankBackingSpanResolutionV1::Unique(backing) => exact_backing = Some(backing),
            BankBackingSpanResolutionV1::Ambiguous => {
                blockers.insert(OwnerBlocker::AmbiguousBankBacking);
            }
            BankBackingSpanResolutionV1::InvalidGeometry => {
                blockers.insert(OwnerBlocker::InvalidBankBackingGeometry);
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
        validate_indirects(
            owner,
            cfg,
            facts,
            owner_proof_authority,
            indirect_target_domains,
            &mut blockers,
        );
    }

    if blockers.is_empty() {
        let owner = owner.expect("an exact assessment must have one partition owner");
        let backing = exact_backing.expect("an exact assessment must have one bank backing");
        return OwnerAssessment::Proven {
            owner: ExactFunctionOwner {
                entry,
                va_end: owner.extent_end,
                backing,
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
            ..
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
            ..
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
        BlockTerminator::RanOffEnd | BlockTerminator::DataFence { .. } => {
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
            Fact::ResolvedCall { source, target } => {
                let foreign_source = source.bank != owner.bank || !contains(source.pc);
                if target.bank == owner.bank
                    && contains(target.pc)
                    && target.pc != owner.root_va
                    && foreign_source
                {
                    blockers.insert(OwnerBlocker::IncomingEdge {
                        source: source.pc,
                        target: target.pc,
                        edge: IncomingEdgeKind::ResolvedCall,
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
                ..
            } => {
                edges.push((*target, IncomingEdgeKind::Branch));
                edges.push((*fallthrough, IncomingEdgeKind::Fallthrough));
            }
            BlockTerminator::BranchLikely {
                target,
                fallthrough,
                ..
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
            | BlockTerminator::RanOffEnd
            | BlockTerminator::DataFence { .. } => {}
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
    owner_proof_authority: Option<&OwnerProofAuthority>,
    target_domains: &BTreeMap<(u32, bool), BTreeSet<u32>>,
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
        if let Some(authority) =
            owner_proof_authority.filter(|authority| authority.bank == cfg.bank)
        {
            if !authority
                .indirect_sites
                .contains(&AuthorityIndirectSite::new(site.pc, site.via_call))
            {
                continue;
            }
        }
        let site_scope = scope(site.pc);
        if site_scope == IndirectScope::Bank {
            if target_domains
                .get(&(site.pc, site.via_call))
                .is_some_and(|targets| {
                    !targets.is_empty()
                        && targets
                            .iter()
                            .all(|target| *target < owner.root_va || *target >= owner.extent_end)
                })
            {
                continue;
            }
        }
        blockers.insert(OwnerBlocker::UnresolvedIndirect {
            site: site.pc,
            scope: site_scope,
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

/// Index exclusion-only target domains once per bank. The fact database is
/// append-only and can retain analyses from more than one closure pass, so a
/// site is admitted only when every retained analysis projects to the same
/// guard-bounded domain. An open, exhaustive, differently-kinded, or differing
/// target-set record invalidates the site rather than letting stale evidence
/// discharge a blocker.
fn collect_indirect_target_domains(
    facts: &FactDb,
    bank: &str,
) -> BTreeMap<(u32, bool), BTreeSet<u32>> {
    let mut domains = BTreeMap::<(u32, bool), Option<BTreeSet<u32>>>::new();
    for fact in facts.facts() {
        let Fact::IndirectTransferAnalysis { site, via_call, .. } = fact else {
            continue;
        };
        if site.bank != bank {
            continue;
        }
        let key = (site.pc, *via_call);
        let projected = fact
            .indirect_target_domain_v1()
            .map(|domain| domain.targets.into_iter().collect::<BTreeSet<_>>());
        match domains.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(projected);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref() != projected.as_ref() {
                    entry.insert(None);
                }
            }
        }
    }
    domains
        .into_iter()
        .filter_map(|(site, targets)| targets.map(|targets| (site, targets)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{build_cfg, build_cfg_with_indirect};
    use crate::facts::{
        evaluated_image_receipt_sha256_v1, CandidateDetector, EvaluatedImageReceiptV1,
        FunctionEntryEvidence, IndirectTransferKind, MaterializationEvaluatorV1,
        MaterializedImageSourceV1, MaterializedImageSuffixV1, ProloguePattern, RomAddressSpace,
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
        add_executable_and_entries(&mut facts, bytes_len, entries);
        facts
    }

    fn evaluated_receipt(output_len: u32) -> EvaluatedImageReceiptV1 {
        EvaluatedImageReceiptV1 {
            evaluator: MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            source: MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x2000,
                rom_end: 0x2040,
                cursor: 4,
            },
            source_sha256: "11".repeat(32),
            output_len,
            output_sha256: "22".repeat(32),
            streams: Vec::new(),
            trailing_suffix: MaterializedImageSuffixV1 {
                offset: 0,
                len: 0,
                sha256: "33".repeat(32),
            },
        }
    }

    fn materialized_facts_for(image_len: u32, receipt_output_len: u32, entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + image_len,
            receipt: evaluated_receipt(receipt_output_len),
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![image],
                "test_materialized_image",
            )
            .unwrap();
        add_executable_and_entries(&mut facts, image_len, entries);
        facts
    }

    fn add_executable_and_entries(facts: &mut FactDb, bytes_len: u32, entries: &[u32]) {
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
    fn owner_proof_authority_is_bank_bound_and_fails_closed_on_mismatch() {
        let site = BASE + 0x20;
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, site]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[]);
        let authority_closure = ClosureResult {
            cfg: Cfg {
                bank: "other".into(),
                word_class: BTreeMap::new(),
                blocks: Vec::new(),
                direct_calls: Vec::new(),
                tail_transfers: Vec::new(),
                indirect_sites: Vec::new(),
                plain_delay_entry_aliases: Vec::new(),
                unsupported_delay_entries: Vec::new(),
                proven_roots: Vec::new(),
            },
            indirect: Vec::new(),
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authority_closure,
            &facts,
            &BTreeSet::from([BASE]),
        );
        let report =
            prove_exact_owners_with_authority(&cfg, &partition, &facts, &bytes, BASE, &authority);

        let blockers = &frontier(&report.assessments[0]).blockers;
        assert!(blockers.contains(&OwnerBlocker::EntryNotAuthoritative));
        assert!(blockers.contains(&OwnerBlocker::UnresolvedIndirect {
            site,
            scope: IndirectScope::Bank,
        }));
    }

    #[test]
    fn exact_direct_call_authority_rejects_malformed_or_unproven_delay_geometry() {
        let target = BASE + 0x20;
        let block = BasicBlock {
            start_va: BASE,
            end_va: BASE + 8,
            terminator: BlockTerminator::Call {
                target,
                next: BASE + 8,
            },
        };
        let mut cfg = Cfg {
            bank: "bank".into(),
            word_class: [
                (BASE, WordClass::ProvenCode),
                (BASE + 4, WordClass::ProvenCode),
            ]
            .into_iter()
            .collect(),
            blocks: vec![block.clone()],
            direct_calls: vec![(BASE, target)],
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            proven_roots: vec![BASE],
        };
        assert_eq!(
            exact_authority_direct_call(&cfg, &block),
            Some((BASE, target))
        );

        let malformed = BasicBlock {
            end_va: BASE + 4,
            ..block.clone()
        };
        assert_eq!(exact_authority_direct_call(&cfg, &malformed), None);

        cfg.word_class.insert(BASE + 4, WordClass::Unknown);
        assert_eq!(exact_authority_direct_call(&cfg, &block), None);

        cfg.word_class.insert(BASE + 4, WordClass::ProvenCode);
        cfg.direct_calls.clear();
        assert_eq!(exact_authority_direct_call(&cfg, &block), None);
    }

    #[test]
    fn exact_owner_requires_and_carries_typed_affine_backing() {
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
        assert_eq!(
            owner.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: 0x1000,
                rom_end: 0x100c,
            }
        );
        assert_eq!(owner.byte_len(), 12);
    }

    #[test]
    fn exact_owner_carries_materialized_output_offsets_without_rom_coordinates() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = materialized_facts_for(bytes.len() as u32, bytes.len() as u32, &[BASE]);
        let expected_receipt = evaluated_receipt(bytes.len() as u32);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let OwnerAssessment::Proven { owner } = &report.assessments[0] else {
            panic!(
                "expected exact materialized owner: {:?}",
                report.assessments[0]
            );
        };
        assert_eq!(
            owner.backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256: evaluated_image_receipt_sha256_v1(&expected_receipt),
                output_start: 0,
                output_end: 12,
            }
        );
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
    fn distinct_bank_backings_are_ambiguous() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        let first = facts.conclusion("bank:bank").unwrap().justified_by[0];
        let second = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x2000,
            rom_end: 0x2008,
            va_start: BASE,
            va_end: BASE + 8,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![first, second],
                "test competing proven backing",
            )
            .unwrap();

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::AmbiguousBankBacking));
    }

    #[test]
    fn invalid_bank_backing_geometry_is_ambiguous() {
        let bytes = asm(&[JR_RA, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let facts = materialized_facts_for(bytes.len() as u32, 12, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(matches!(
            report.assessments[0],
            OwnerAssessment::Ambiguous { .. }
        ));
        assert!(frontier(&report.assessments[0])
            .blockers
            .contains(&OwnerBlocker::InvalidBankBackingGeometry));
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
    fn candidate_only_indirect_does_not_enter_authority_owner_proof() {
        let site = BASE + 0x20;
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let broad_cfg = build_cfg("bank", &bytes, BASE, &[BASE, site]);
        let partition = partition(&broad_cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let authority_closure = ClosureResult {
            cfg: build_cfg("bank", &bytes, BASE, &[BASE]),
            indirect: Vec::new(),
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authority_closure,
            &facts,
            &BTreeSet::new(),
        );
        let report = prove_exact_owners_with_authority(
            &broad_cfg, &partition, &facts, &bytes, BASE, &authority,
        );
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(matches!(caller, OwnerAssessment::Proven { .. }));

        let authoritative_site = ClosureResult {
            cfg: broad_cfg.clone(),
            indirect: vec![crate::resolve::IndirectResolution {
                site_pc: site,
                via_call: true,
                state: IndirectProofState::Open,
                kind: None,
                targets: Vec::new(),
                memory_sources: Vec::new(),
            }],
        };
        let authority = OwnerProofAuthority::from_authority_closure(
            &authoritative_site,
            &facts,
            &BTreeSet::new(),
        );
        let blocked = prove_exact_owners_with_authority(
            &broad_cfg, &partition, &facts, &bytes, BASE, &authority,
        );
        let caller = blocked
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn guard_bounded_domain_disjoint_from_owner_discharges_bank_scoped_site() {
        let site = BASE + 0x20;
        let jal = 0x0c00_0000 | ((site >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![site],
            memory_sources: vec![BASE + 0x18],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        let still_blocked = match caller {
            OwnerAssessment::Proven { .. } => false,
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                frontier
                    .blockers
                    .contains(&OwnerBlocker::UnresolvedIndirect {
                        site,
                        scope: IndirectScope::Bank,
                    })
            }
        };
        assert!(!still_blocked);

        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Open,
            kind: None,
            targets: vec![],
            memory_sources: vec![],
        });
        let conflicted = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = conflicted
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn bounded_domain_that_can_enter_owner_keeps_bank_scoped_blocker() {
        let site = BASE + 0x20;
        let jal = 0x0c00_0000 | ((site >> 2) & 0x03ff_ffff);
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&jalr_t9.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", site),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![BASE + 4],
            memory_sources: vec![BASE + 0x18],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let caller = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == BASE)
            .unwrap();
        assert!(frontier(caller)
            .blockers
            .contains(&OwnerBlocker::UnresolvedIndirect {
                site,
                scope: IndirectScope::Bank,
            }));
    }

    #[test]
    fn bounded_domain_never_discharges_owner_scoped_site() {
        let jalr_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let bytes = asm(&[jalr_t9, NOP]);
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE]);
        let partition = partition(&cfg);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: true,
            state: IndirectTransferState::Bounded,
            kind: Some(IndirectTransferKind::JumpTable),
            targets: vec![BASE + 0x100],
            memory_sources: vec![BASE + 0x80],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        assert!(frontier(&report.assessments[0]).blockers.contains(
            &OwnerBlocker::UnresolvedIndirect {
                site: BASE,
                scope: IndirectScope::Owner,
            }
        ));
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
    fn exhaustive_jalr_target_is_an_authoritative_entry() {
        // An exhaustive computed CALL (`jalr $ra, $t9`) whose proven target set
        // is represented in the CFG promotes that target to an authoritative
        // callable root, exactly like a direct `jal`. The target carries no
        // entry fact of its own: its authority comes solely from being a proven
        // reachable computed-call destination.
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jalr_ra_t9, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let target = BASE + 0x20;
        let mut exhaustive = BTreeMap::new();
        exhaustive.insert(BASE, vec![target]);
        let cfg = build_cfg_with_indirect("bank", &bytes, BASE, &[BASE], &exhaustive);
        let partition = partition(&cfg);
        // Only the caller BASE has an entry fact; the target does not.
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts.insert(Fact::IndirectTransferAnalysis {
            site: BankAddr::new("bank", BASE),
            via_call: true,
            state: IndirectTransferState::Exhaustive,
            kind: Some(IndirectTransferKind::Constant),
            targets: vec![target],
            memory_sources: vec![],
        });

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .expect("the exhaustive-call target is assessed");
        assert!(
            matches!(target_assessment, OwnerAssessment::Proven { .. }),
            "exhaustive computed-call target should be a proven owner: {target_assessment:?}"
        );
    }

    #[test]
    fn non_exhaustive_jalr_target_is_not_authoritative() {
        // Same computed call, but the site is NOT exhaustively resolved: the
        // CFG keeps it an open `Indirect`, so its runtime destination is
        // unproven. A block that merely looks like a function at BASE+0x20 must
        // NOT be admitted as an owner — it lacks any authoritative entry.
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut bytes = asm(&[jalr_ra_t9, NOP, JR_RA, NOP]);
        bytes.resize(0x28, 0);
        bytes[0x20..0x24].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x24..0x28].copy_from_slice(&NOP.to_be_bytes());
        let target = BASE + 0x20;
        // No exhaustive map entry: the `jalr` stays an open indirect site.
        // Seed the target as a traversal root so it is still assessed, proving
        // that traversal reach alone never confers entry authority.
        let cfg = build_cfg("bank", &bytes, BASE, &[BASE, target]);
        let partition = partition(&cfg);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        let report = prove_exact_owners(&cfg, &partition, &facts, &bytes, BASE);
        let target_assessment = report
            .assessments
            .iter()
            .find(|assessment| assessment.entry().pc == target)
            .expect("the seeded target is assessed");
        assert!(
            frontier(target_assessment)
                .blockers
                .contains(&OwnerBlocker::EntryNotAuthoritative),
            "a non-exhaustive computed-call target must not be authoritative: {target_assessment:?}"
        );
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
