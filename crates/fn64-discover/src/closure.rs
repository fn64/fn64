//! Execution-closure scoreboard: a pure MEASUREMENT layer over the facts an
//! already-composed [`ProgramSnapshotV1`] carries.
//!
//! The full-game gate (DISCOVER-PLAN, UNIVERSAL-RUNTIME-PLAN) is "zero
//! `unsupported` execution destinations": every reachable CPU transfer
//! destination must be classifiable as `exact_aot`, `block_aot`,
//! `dynamic_mips`, or `unsupported`. This module counts that classification
//! per ROM (across all composed banks). Source edges are projected from the
//! final broad CFG only when the authority-rooted closure retained by
//! [`crate::snapshot`] contains the exact same source, destination, and edge
//! kind, or when a consecutive plain fallthrough is wholly inside one
//! authority block's ordinary prefix. The broader exploratory closure can
//! therefore supply final block boundaries without manufacturing executable
//! reachability. This module never runs discovery, mutates a fact, or promotes
//! anything.
//!
//! # Honest scope
//!
//! This is a STATIC reachability scoreboard from PROVEN roots. It measures
//! what discovery has closed, not what a live run touches. A destination that
//! is only reachable at runtime (through mutable memory / arguments) surfaces
//! here as an OPEN indirect site (counted `dynamic_mips`) or is simply never
//! reached — it does not become `unsupported` merely by being invisible to a
//! static pass. The number to drive to zero is `unsupported`: a CONCRETE,
//! statically-resolved transfer whose destination lands in data or outside
//! every known mapping, with no interpreter-fallback story. `dynamic_mips`
//! (open/bounded indirects, mapped-but-unproven code) is fallback-covered and
//! reported honestly, but is not a release blocker.

use crate::block_proof::{BlockAssessment, BlockProofBlocker};
use crate::cfg::{BasicBlock, BlockTerminator, WordClass};
use crate::facts::Fact;
use crate::owner_proof::OwnerAssessment;
use crate::resolve::{IndirectProofState, IndirectResolutionKind};
use crate::snapshot::{BankSnapshotV1, OwnerBlockerKind, ProgramSnapshotV1};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The four classes a reachable CPU transfer destination can land in, in
/// order of decreasing recompilation strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationClass {
    /// Inside a proven exact-owner function: recompiled ahead-of-time, exact.
    ExactAot,
    /// A proven executable block (reached-proven-code) not inside an exact
    /// owner: still recompilable ahead-of-time at block granularity.
    BlockAot,
    /// A reachable computed/indirect transfer whose targets are not statically
    /// admitted as code — an OPEN or merely BOUNDED indirect site, or a
    /// concrete destination that is mapped but not proven executable. Covered
    /// by the interpreter fallback; not a release blocker, but reported.
    DynamicMips,
    /// A reachable transfer whose destination cannot be classified at all:
    /// it lands outside every known mapping, or into proven data, or is an
    /// unresolved-and-unbounded computed transfer with no fallback story.
    /// THIS is the release-blocker count that must reach zero.
    Unsupported,
}

impl DestinationClass {
    pub const ALL: [DestinationClass; 4] = [
        DestinationClass::ExactAot,
        DestinationClass::BlockAot,
        DestinationClass::DynamicMips,
        DestinationClass::Unsupported,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DestinationClass::ExactAot => "exact_aot",
            DestinationClass::BlockAot => "block_aot",
            DestinationClass::DynamicMips => "dynamic_mips",
            DestinationClass::Unsupported => "unsupported",
        }
    }
}

/// Why a destination fell in the class it did — kept for auditing the scores,
/// never collapsed into the headline count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationReason {
    /// A concrete destination VA inside a proven exact owner.
    InExactOwner,
    /// A concrete destination VA covered by a proven executable block.
    InProvenBlock,
    /// An OPEN indirect site (`jr`/`jalr` whose targets discovery did not
    /// close). Its would-be targets are the interpreter's job.
    OpenIndirectSite,
    /// A BOUNDED-but-not-exhaustive indirect resolution: a finite over-set
    /// exists but was not admitted to CFG closure.
    BoundedIndirectSite,
    /// A concrete destination VA that is mapped in some bank but whose word is
    /// not proven code (mapped-but-not-proven-executable). The word was never
    /// proven either way -- unknown, or only a candidate.
    MappedNotProvenCode,
    /// A concrete destination VA the CFG PROVED is code, which block proof
    /// nonetheless declined to admit as a block.
    ///
    /// Same class as [`Self::MappedNotProvenCode`] -- the interpreter covers
    /// both -- but a very different finding: this one is a block-admission
    /// refusal to audit, not a word that failed to decode. Folding the two
    /// together read as "discovery could not tell what this is" when discovery
    /// had in fact already proven it was code.
    ProvenCodeNoOwner,
    /// A concrete destination VA that lands on proven data.
    IntoProvenData,
    /// A concrete destination VA outside every known bank mapping.
    OutsideAllMappings,
}

impl DestinationReason {
    pub const ALL: [Self; 8] = [
        Self::InExactOwner,
        Self::InProvenBlock,
        Self::OpenIndirectSite,
        Self::BoundedIndirectSite,
        Self::MappedNotProvenCode,
        Self::ProvenCodeNoOwner,
        Self::IntoProvenData,
        Self::OutsideAllMappings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::InExactOwner => "in_exact_owner",
            Self::InProvenBlock => "in_proven_block",
            Self::OpenIndirectSite => "open_indirect_site",
            Self::BoundedIndirectSite => "bounded_indirect_site",
            Self::MappedNotProvenCode => "mapped_not_proven_code",
            Self::ProvenCodeNoOwner => "proven_code_no_owner",
            Self::IntoProvenData => "into_proven_data",
            Self::OutsideAllMappings => "outside_all_mappings",
        }
    }

    fn class(self) -> DestinationClass {
        match self {
            DestinationReason::InExactOwner => DestinationClass::ExactAot,
            DestinationReason::InProvenBlock => DestinationClass::BlockAot,
            DestinationReason::OpenIndirectSite
            | DestinationReason::BoundedIndirectSite
            | DestinationReason::MappedNotProvenCode
            | DestinationReason::ProvenCodeNoOwner => DestinationClass::DynamicMips,
            DestinationReason::IntoProvenData | DestinationReason::OutsideAllMappings => {
                DestinationClass::Unsupported
            }
        }
    }
}

/// A per-class count of destinations plus the bytes they cover. Bytes are the
/// covered instruction-word bytes for concrete destinations (4 per aligned
/// word slot uniquely counted) and are zero for indirect SITES, which have no
/// single destination address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassTally {
    pub destinations: u64,
    pub bytes: u64,
}

/// The scoreboard for one ROM: the per-class tallies, the reason histogram,
/// and the two headline numbers (`unsupported` — the release blocker — and
/// `dynamic_mips` — fallback-covered, reported honestly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureScoreboard {
    pub total_destinations: u64,
    pub per_class: BTreeMap<String, ClassTally>,
    pub per_reason: BTreeMap<String, u64>,
    pub unsupported: u64,
    pub dynamic_mips: u64,
}

impl ClosureScoreboard {
    pub fn tally(&self, class: DestinationClass) -> ClassTally {
        self.per_class
            .get(class.label())
            .copied()
            .unwrap_or_default()
    }

    pub fn reason_count(&self, reason: DestinationReason) -> u64 {
        self.per_reason
            .get(reason.label())
            .copied()
            .unwrap_or_default()
    }
}

/// A concrete destination VA plus how it was classified. Kept so a grader can
/// hold out the ROM dump and check that no `exact_aot`/`block_aot` destination
/// lands where the dump says "data".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassifiedDestination {
    pub va: u32,
    pub reason: DestinationReason,
}

impl ClassifiedDestination {
    pub fn class(&self) -> DestinationClass {
        self.reason.class()
    }
}

/// The concrete control-transfer shape that introduced an unsupported target.
///
/// This is diagnostic provenance, not a new execution authority. In
/// particular, a direct call still needs an exact host-catalog or guest-code
/// owner before a consumer may treat it as executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcreteTransferKind {
    Tail,
    Call,
    CallContinuation,
    BranchTaken { link: bool },
    BranchFallthrough { link: bool },
    BranchLikelyTaken { link: bool },
    BranchLikelyFallthrough { link: bool },
    ResolvedIndirectJump,
    ResolvedIndirectCall,
    ResolvedIndirectCallContinuation,
    OpenIndirectCallContinuation,
    PlainFallthrough,
    DelayEntryContinuation,
    RanOffEndFallthrough,
}

/// One concrete successor emitted by a CFG terminator.
///
/// This is the single edge denominator shared by the scoreboard, classified
/// destination list, and retained unsupported audit. `source_site_va` names
/// the control word for delayed transfers and the final ordinary word for a
/// plain or ran-off-end fallthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcreteSuccessor {
    pub destination_va: u32,
    pub source_site_va: u32,
    pub kind: ConcreteTransferKind,
}

/// One authority-projected CFG edge contributing to a retained concrete
/// destination classification.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IncomingTransferV1 {
    pub bank: String,
    pub block_start_va: u32,
    pub block_end_va: u32,
    /// Control word for a delayed transfer, or final ordinary word for a
    /// plain or ran-off-end fallthrough.
    pub source_site_va: u32,
    pub kind: ConcreteTransferKind,
}

/// Address-level evidence for one unsupported concrete destination.
///
/// The historical scoreboard retained only a bounded printed VA list. This
/// shape lets an opt-in gate artifact preserve every incoming edge so a later
/// mapping investigation does not need the ROM merely to recover the original
/// punch list. It intentionally carries no constructor that can promote a
/// destination: host targets, exception-vector images, resident/overlay load
/// mappings, and runtime TLB mappings remain separate typed authorities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedDestinationAuditV1 {
    pub destination_va: u32,
    pub reason: DestinationReason,
    pub incoming: Vec<IncomingTransferV1>,
}

/// Block-proof refusal relevant to one retained dynamic concrete destination.
///
/// Only blocker kinds are retained: blocker payloads may contain decoded ROM
/// words, which do not belong in this path-free measurement artifact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicBlockProofV1 {
    pub bank: String,
    pub block_start_va: u32,
    pub block_end_va: u32,
    pub blocker_kinds: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicOwnerAssessmentStateV1 {
    Candidate,
    Ambiguous,
}

/// Owner-proof refusal attached through the candidate block's partition
/// claimant, again reduced to non-ROM-bearing blocker kinds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicOwnerProofV1 {
    pub bank: String,
    pub entry_va: u32,
    pub state: DynamicOwnerAssessmentStateV1,
    pub proposed_va_end: Option<u32>,
    pub blocker_kinds: Vec<OwnerBlockerKind>,
}

/// Exact retained evidence for one concrete `dynamic_mips` destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicConcreteDestinationAuditV1 {
    pub destination_va: u32,
    pub reason: DestinationReason,
    pub incoming: Vec<IncomingTransferV1>,
    pub block_proof: Vec<DynamicBlockProofV1>,
    pub owner_proof: Vec<DynamicOwnerProofV1>,
}

/// Exact retained evidence for one bounded/open indirect site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicIndirectSiteAuditV1 {
    pub bank: String,
    pub site_pc: u32,
    pub via_call: bool,
    pub state: IndirectProofState,
    pub kind: Option<IndirectResolutionKind>,
    pub targets: Vec<u32>,
    pub memory_sources: Vec<u32>,
}

/// A proven exact-owner extent, denormalized from the composed snapshot for
/// fast point classification.
#[derive(Clone, Copy)]
struct OwnerExtent {
    start: u32,
    end: u32,
}

/// A proven executable block extent.
#[derive(Clone, Copy)]
struct BlockExtent {
    start: u32,
    end: u32,
}

/// One proven ROM->VA mapping's VA interval, for "is this address mapped at
/// all" tests independent of proof-of-code.
#[derive(Clone, Copy)]
struct MappedVaRange {
    start: u32,
    end: u32,
}

/// The union of proof geometry across every composed bank of one ROM. A
/// destination is classified against the WHOLE program, because a cross-bank
/// call lands in a sibling bank's owner, not the caller's.
struct ProgramGeometry {
    owners: Vec<OwnerExtent>,
    blocks: Vec<BlockExtent>,
    mapped: Vec<MappedVaRange>,
    /// VA -> word classification, unioned across banks. Absent == never
    /// visited by any traversal (equivalent to `Unknown`).
    word_class: BTreeMap<u32, WordClass>,
}

impl ProgramGeometry {
    fn from_snapshots(snapshots: &[ProgramSnapshotV1]) -> Self {
        let mut owners = Vec::new();
        let mut blocks = Vec::new();
        let mut mapped = Vec::new();
        let mut word_class: BTreeMap<u32, WordClass> = BTreeMap::new();

        for snapshot in snapshots {
            for fact in snapshot.facts.proven_rom_mappings() {
                if let Fact::RomMapping {
                    va_start, va_end, ..
                } = fact
                {
                    mapped.push(MappedVaRange {
                        start: *va_start,
                        end: *va_end,
                    });
                }
            }
            for bank in &snapshot.banks {
                for assessment in &bank.owner_proof.assessments {
                    if let OwnerAssessment::Proven { owner } = assessment {
                        owners.push(OwnerExtent {
                            start: owner.entry.pc,
                            end: owner.va_end,
                        });
                    }
                }
                for assessment in &bank.block_proof.assessments {
                    if let BlockAssessment::Proven { block } = assessment {
                        blocks.push(BlockExtent {
                            start: block.start_va,
                            end: block.end_va,
                        });
                    }
                }
                for (&va, &class) in &bank.closure.cfg.word_class {
                    let entry = word_class.entry(va).or_insert(WordClass::Unknown);
                    *entry = entry.merge(class);
                }
            }
        }
        owners.sort_by_key(|extent| extent.start);
        blocks.sort_by_key(|extent| extent.start);
        mapped.sort_by_key(|range| range.start);
        Self {
            owners,
            blocks,
            mapped,
            word_class,
        }
    }

    fn in_owner(&self, va: u32) -> bool {
        self.owners
            .iter()
            .any(|extent| va >= extent.start && va < extent.end)
    }

    fn in_block(&self, va: u32) -> bool {
        self.blocks
            .iter()
            .any(|extent| va >= extent.start && va < extent.end)
    }

    fn is_mapped(&self, va: u32) -> bool {
        self.mapped
            .iter()
            .any(|range| va >= range.start && va < range.end)
    }

    /// Classify one CONCRETE destination VA (a target with a known address).
    fn classify_concrete(&self, va: u32) -> DestinationReason {
        if self.in_owner(va) {
            return DestinationReason::InExactOwner;
        }
        if self.in_block(va) {
            return DestinationReason::InProvenBlock;
        }
        if !self.is_mapped(va) {
            return DestinationReason::OutsideAllMappings;
        }
        // Mapped, but not inside a proven owner or block. Consult the unioned
        // word class: proven data is a hard blocker; anything not proven code
        // is mapped-but-unproven — interpreter-fallback territory.
        match self.word_class.get(&va).copied() {
            Some(WordClass::ProvenData) | Some(WordClass::Conflict) => {
                DestinationReason::IntoProvenData
            }
            Some(WordClass::ProvenCode) => DestinationReason::ProvenCodeNoOwner,
            _ => DestinationReason::MappedNotProvenCode,
        }
    }
}

/// Enumerate every concrete successor represented by one CFG terminator.
///
/// Open indirect targets are intentionally absent because no concrete VA is
/// known, but an indirect call's concrete return continuation is retained.
/// Delayed call/branch continuations are equally real successors and must not
/// disappear merely because they are commonly the next block in address
/// order. The composer has already validated block geometry; malformed blocks
/// never reach closure measurement.
pub fn concrete_successors(block: &BasicBlock) -> Vec<ConcreteSuccessor> {
    let delayed = || {
        block
            .end_va
            .checked_sub(8)
            .expect("delayed transfer block includes control and delay words")
    };
    let ordinary = || {
        block
            .end_va
            .checked_sub(4)
            .expect("plain fallthrough block includes one ordinary word")
    };
    let successor = |destination_va, source_site_va, kind| ConcreteSuccessor {
        destination_va,
        source_site_va,
        kind,
    };

    match &block.terminator {
        BlockTerminator::Tail { target } => {
            vec![successor(*target, delayed(), ConcreteTransferKind::Tail)]
        }
        BlockTerminator::Call { target, next } => vec![
            successor(*target, delayed(), ConcreteTransferKind::Call),
            successor(*next, delayed(), ConcreteTransferKind::CallContinuation),
        ],
        BlockTerminator::Branch {
            target,
            fallthrough,
            link,
        } => vec![
            successor(
                *target,
                delayed(),
                ConcreteTransferKind::BranchTaken { link: *link },
            ),
            successor(
                *fallthrough,
                delayed(),
                ConcreteTransferKind::BranchFallthrough { link: *link },
            ),
        ],
        BlockTerminator::BranchLikely {
            target,
            fallthrough,
            link,
        } => vec![
            successor(
                *target,
                delayed(),
                ConcreteTransferKind::BranchLikelyTaken { link: *link },
            ),
            successor(
                *fallthrough,
                delayed(),
                ConcreteTransferKind::BranchLikelyFallthrough { link: *link },
            ),
        ],
        BlockTerminator::ResolvedIndirect { targets, via_call } => {
            let mut successors = targets
                .iter()
                .copied()
                .map(|target| {
                    successor(
                        target,
                        delayed(),
                        if *via_call {
                            ConcreteTransferKind::ResolvedIndirectCall
                        } else {
                            ConcreteTransferKind::ResolvedIndirectJump
                        },
                    )
                })
                .collect::<Vec<_>>();
            if *via_call {
                successors.push(successor(
                    block.end_va,
                    delayed(),
                    ConcreteTransferKind::ResolvedIndirectCallContinuation,
                ));
            }
            successors
        }
        BlockTerminator::Indirect { via_call: true } => vec![successor(
            block.end_va,
            delayed(),
            ConcreteTransferKind::OpenIndirectCallContinuation,
        )],
        BlockTerminator::Fallthrough { next } => vec![successor(
            *next,
            ordinary(),
            ConcreteTransferKind::PlainFallthrough,
        )],
        BlockTerminator::RanOffEnd => vec![successor(
            block.end_va,
            ordinary(),
            ConcreteTransferKind::RanOffEndFallthrough,
        )],
        BlockTerminator::Indirect { via_call: false }
        | BlockTerminator::Return
        | BlockTerminator::Trap
        | BlockTerminator::InvalidInstruction { .. }
        | BlockTerminator::MissingDelaySlot { .. }
        | BlockTerminator::DataFence { .. } => Vec::new(),
    }
}

fn ordinary_prefix_end(block: &BasicBlock) -> u32 {
    match &block.terminator {
        BlockTerminator::Tail { .. }
        | BlockTerminator::Call { .. }
        | BlockTerminator::Branch { .. }
        | BlockTerminator::BranchLikely { .. }
        | BlockTerminator::Return
        | BlockTerminator::Indirect { .. }
        | BlockTerminator::ResolvedIndirect { .. } => block.end_va.saturating_sub(8),
        BlockTerminator::Fallthrough { .. } | BlockTerminator::Trap => {
            block.end_va.saturating_sub(4)
        }
        BlockTerminator::InvalidInstruction { pc, .. }
        | BlockTerminator::MissingDelaySlot { control_pc: pc } => *pc,
        BlockTerminator::RanOffEnd | BlockTerminator::DataFence { .. } => block.end_va,
    }
}

fn is_compatible_internal_plain_fallthrough(
    authority_blocks: &[BasicBlock],
    successor: ConcreteSuccessor,
) -> bool {
    successor.kind == ConcreteTransferKind::PlainFallthrough
        && successor.source_site_va.checked_add(4) == Some(successor.destination_va)
        && authority_blocks.iter().any(|authority| {
            authority.start_va <= successor.source_site_va
                && successor.source_site_va < ordinary_prefix_end(authority)
                && successor.destination_va < authority.end_va
        })
}

/// Project final CFG successors through authority at their exact source site.
///
/// Traversal hints may split or otherwise refine the broad CFG's block shape,
/// so enumerating the separately-built authority CFG can lose a modeled edge
/// at such a boundary. Conversely, enumerating every broad block lets a
/// candidate root manufacture execution. Source reachability alone is not
/// enough: a candidate split can attach a different terminator to the same
/// word. Retain a broad edge only when the authority CFG contains the exact
/// `(source site, destination, kind)` edge, except that an ordinary consecutive
/// fallthrough may refine the interior prefix of one authority block. That
/// compatibility never crosses the authority block's terminal/control boundary.
fn authority_projected_successors(bank: &BankSnapshotV1) -> Vec<(&BasicBlock, ConcreteSuccessor)> {
    let authority_blocks = bank.authority_closure.cfg.blocks.as_slice();
    let authority_successors = bank
        .authority_closure
        .cfg
        .blocks
        .iter()
        .flat_map(|block| concrete_successors(block))
        .map(|successor| {
            (
                successor.source_site_va,
                successor.destination_va,
                successor.kind,
            )
        })
        .collect::<BTreeSet<_>>();
    let mut projected = bank
        .closure
        .cfg
        .blocks
        .iter()
        .flat_map(|block| {
            concrete_successors(block)
                .into_iter()
                .filter(|successor| {
                    authority_successors.contains(&(
                        successor.source_site_va,
                        successor.destination_va,
                        successor.kind,
                    )) || is_compatible_internal_plain_fallthrough(authority_blocks, *successor)
                })
                .map(move |successor| (block, successor))
        })
        .collect::<Vec<_>>();
    for alias in &bank.closure.cfg.plain_delay_entry_aliases {
        if !bank
            .authority_closure
            .cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|authority| authority == alias)
        {
            continue;
        }
        let Some(predecessor) = bank.closure.cfg.blocks.iter().find(|block| {
            block.start_va <= alias.control_pc && block.end_va == alias.continuation_va
        }) else {
            continue;
        };
        projected.push((
            predecessor,
            ConcreteSuccessor {
                destination_va: alias.continuation_va,
                source_site_va: alias.entry_va,
                kind: ConcreteTransferKind::DelayEntryContinuation,
            },
        ));
    }
    projected
}

/// Enumerate every reachable CPU transfer destination across all composed
/// banks of one ROM and classify each. Concrete successors (taken targets,
/// direct and indirect-call continuations, branch fallthroughs, and ordinary
/// fallthroughs) are deduplicated by VA so a hot function reached a thousand
/// times counts once. OPEN and BOUNDED indirect SITES are counted per site
/// (each is a distinct place the CPU can leave statically closed code) and
/// carry no single VA.
pub fn scoreboard(snapshots: &[ProgramSnapshotV1]) -> ClosureScoreboard {
    let geometry = ProgramGeometry::from_snapshots(snapshots);

    // Concrete destination VAs, deduplicated, mapped to their classification.
    let mut concrete: BTreeMap<u32, DestinationReason> = BTreeMap::new();
    let record = |va: u32, geometry: &ProgramGeometry, concrete: &mut BTreeMap<u32, _>| {
        concrete
            .entry(va)
            .or_insert_with(|| geometry.classify_concrete(va));
    };

    // Per-class tallies; indirect sites are appended after concrete VAs.
    let mut per_reason: BTreeMap<DestinationReason, u64> = DestinationReason::ALL
        .into_iter()
        .map(|reason| (reason, 0))
        .collect();

    for snapshot in snapshots {
        for bank in &snapshot.banks {
            for (_, successor) in authority_projected_successors(bank) {
                record(successor.destination_va, &geometry, &mut concrete);
            }
            // Indirect SITES: one entry per site. Exhaustive resolutions are
            // already reflected as concrete `ResolvedIndirect` targets above,
            // so only Open/Bounded sites are counted here.
            for resolution in &bank.authority_closure.indirect {
                use crate::resolve::IndirectProofState;
                match resolution.state {
                    IndirectProofState::Exhaustive => {}
                    IndirectProofState::Open => {
                        *per_reason
                            .entry(DestinationReason::OpenIndirectSite)
                            .or_default() += 1;
                    }
                    IndirectProofState::Bounded => {
                        *per_reason
                            .entry(DestinationReason::BoundedIndirectSite)
                            .or_default() += 1;
                    }
                }
            }
        }
    }

    // Fold the concrete VAs into the reason histogram.
    for reason in concrete.values() {
        *per_reason.entry(*reason).or_default() += 1;
    }

    // Build the class tallies. Concrete destinations contribute 4 bytes each
    // (one aligned instruction word slot); indirect sites contribute no bytes.
    let mut per_class: BTreeMap<DestinationClass, ClassTally> = DestinationClass::ALL
        .into_iter()
        .map(|class| (class, ClassTally::default()))
        .collect();
    for (va, reason) in &concrete {
        let _ = va;
        let entry = per_class.entry(reason.class()).or_default();
        entry.destinations += 1;
        entry.bytes += 4;
    }
    for (reason, count) in &per_reason {
        // Indirect sites (no concrete VA) still add to the destination count
        // of their class but not to bytes.
        if matches!(
            reason,
            DestinationReason::OpenIndirectSite | DestinationReason::BoundedIndirectSite
        ) {
            let entry = per_class.entry(reason.class()).or_default();
            entry.destinations += count;
        }
    }

    let total_destinations = per_class.values().map(|tally| tally.destinations).sum();
    let unsupported = per_class
        .get(&DestinationClass::Unsupported)
        .map(|tally| tally.destinations)
        .unwrap_or(0);
    let dynamic_mips = per_class
        .get(&DestinationClass::DynamicMips)
        .map(|tally| tally.destinations)
        .unwrap_or(0);

    ClosureScoreboard {
        total_destinations,
        per_class: per_class
            .into_iter()
            .map(|(class, tally)| (class.label().to_string(), tally))
            .collect(),
        per_reason: per_reason
            .into_iter()
            .map(|(reason, count)| (reason.label().to_string(), count))
            .collect(),
        unsupported,
        dynamic_mips,
    }
}

/// Enumerate the concrete classified destinations (VA + reason), sorted by VA.
/// Grading uses this to hold out the ROM dump and reject any `exact_aot`/
/// `block_aot` destination the dump calls data.
pub fn classified_destinations(snapshots: &[ProgramSnapshotV1]) -> Vec<ClassifiedDestination> {
    let geometry = ProgramGeometry::from_snapshots(snapshots);
    let mut concrete: BTreeMap<u32, DestinationReason> = BTreeMap::new();
    for snapshot in snapshots {
        for bank in &snapshot.banks {
            for (_, successor) in authority_projected_successors(bank) {
                concrete
                    .entry(successor.destination_va)
                    .or_insert_with(|| geometry.classify_concrete(successor.destination_va));
            }
        }
    }
    concrete
        .into_iter()
        .map(|(va, reason)| ClassifiedDestination { va, reason })
        .collect()
}

/// Retain every concrete destination currently assigned to `dynamic_mips`,
/// including its authoritative incoming edges and the block/owner refusal
/// kinds that cover the destination. No decoded words or ROM bytes enter the
/// result.
pub fn dynamic_concrete_destination_audit_v1(
    snapshots: &[ProgramSnapshotV1],
) -> Vec<DynamicConcreteDestinationAuditV1> {
    let geometry = ProgramGeometry::from_snapshots(snapshots);
    let mut incoming = BTreeMap::<u32, Vec<IncomingTransferV1>>::new();
    for snapshot in snapshots {
        for bank in &snapshot.banks {
            for (block, successor) in authority_projected_successors(bank) {
                if geometry.classify_concrete(successor.destination_va).class()
                    != DestinationClass::DynamicMips
                {
                    continue;
                }
                incoming
                    .entry(successor.destination_va)
                    .or_default()
                    .push(IncomingTransferV1 {
                        bank: bank.input.bank.clone(),
                        block_start_va: block.start_va,
                        block_end_va: block.end_va,
                        source_site_va: successor.source_site_va,
                        kind: successor.kind,
                    });
            }
        }
    }

    incoming
        .into_iter()
        .map(|(destination_va, mut incoming)| {
            incoming.sort_unstable();
            incoming.dedup();
            let mut block_proof = Vec::new();
            let mut owner_proof = Vec::new();
            for snapshot in snapshots {
                for bank in &snapshot.banks {
                    for assessment in &bank.block_proof.assessments {
                        let BlockAssessment::Candidate {
                            start_va,
                            end_va,
                            blockers,
                        } = assessment
                        else {
                            continue;
                        };
                        if destination_va < *start_va || destination_va >= *end_va {
                            continue;
                        }
                        let mut blocker_kinds = blockers
                            .iter()
                            .map(BlockProofBlocker::kind)
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        blocker_kinds.sort_unstable();
                        blocker_kinds.dedup();
                        block_proof.push(DynamicBlockProofV1 {
                            bank: bank.input.bank.clone(),
                            block_start_va: *start_va,
                            block_end_va: *end_va,
                            blocker_kinds,
                        });

                        let mut claimant_roots = bank
                            .partition
                            .owners
                            .iter()
                            .filter(|owner| owner.block_starts.contains(start_va))
                            .map(|owner| owner.root_va)
                            .collect::<Vec<_>>();
                        claimant_roots.extend(
                            bank.partition
                                .ambiguous
                                .iter()
                                .filter(|block| block.block_start == *start_va)
                                .flat_map(|block| block.claimants.iter().copied()),
                        );
                        claimant_roots.sort_unstable();
                        claimant_roots.dedup();
                        for assessment in &bank.owner_proof.assessments {
                            if claimant_roots
                                .binary_search(&assessment.entry().pc)
                                .is_err()
                            {
                                continue;
                            }
                            let (state, frontier) = match assessment {
                                OwnerAssessment::Candidate { frontier } => {
                                    (DynamicOwnerAssessmentStateV1::Candidate, frontier)
                                }
                                OwnerAssessment::Ambiguous { frontier } => {
                                    (DynamicOwnerAssessmentStateV1::Ambiguous, frontier)
                                }
                                OwnerAssessment::Proven { .. } => continue,
                            };
                            let mut blocker_kinds = frontier
                                .blockers
                                .iter()
                                .map(OwnerBlockerKind::from)
                                .collect::<Vec<_>>();
                            blocker_kinds.sort_unstable();
                            blocker_kinds.dedup();
                            owner_proof.push(DynamicOwnerProofV1 {
                                bank: bank.input.bank.clone(),
                                entry_va: frontier.entry.pc,
                                state,
                                proposed_va_end: frontier.proposed_va_end,
                                blocker_kinds,
                            });
                        }
                    }
                }
            }
            block_proof.sort_unstable();
            block_proof.dedup();
            owner_proof.sort_unstable();
            owner_proof.dedup();
            DynamicConcreteDestinationAuditV1 {
                destination_va,
                reason: geometry.classify_concrete(destination_va),
                incoming,
                block_proof,
                owner_proof,
            }
        })
        .collect()
}

/// Retain every authority-reachable indirect site that still requires the
/// dynamic lane. Target and memory-source sets are canonicalized without
/// consulting bank bytes.
pub fn dynamic_indirect_site_audit_v1(
    snapshots: &[ProgramSnapshotV1],
) -> Vec<DynamicIndirectSiteAuditV1> {
    let mut sites = snapshots
        .iter()
        .flat_map(|snapshot| &snapshot.banks)
        .flat_map(|bank| {
            bank.authority_closure
                .indirect
                .iter()
                .filter(|resolution| resolution.state != IndirectProofState::Exhaustive)
                .map(|resolution| {
                    let mut targets = resolution.targets.clone();
                    targets.sort_unstable();
                    targets.dedup();
                    let mut memory_sources = resolution.memory_sources.clone();
                    memory_sources.sort_unstable();
                    memory_sources.dedup();
                    DynamicIndirectSiteAuditV1 {
                        bank: bank.input.bank.clone(),
                        site_pc: resolution.site_pc,
                        via_call: resolution.via_call,
                        state: resolution.state,
                        kind: resolution.kind,
                        targets,
                        memory_sources,
                    }
                })
        })
        .collect::<Vec<_>>();
    // Do not deduplicate: the scoreboard counts one record per bank/site
    // resolution, and this list preserves that exact denominator.
    sites.sort_by(|left, right| {
        (left.bank.as_str(), left.site_pc, left.via_call).cmp(&(
            right.bank.as_str(),
            right.site_pc,
            right.via_call,
        ))
    });
    sites
}

/// Retain every CFG edge that contributed to an unsupported concrete target.
///
/// Results are canonical: destinations sort by VA and incoming edges sort by
/// bank/source/kind with exact duplicates removed. The classification is the
/// same whole-program [`ProgramGeometry`] used by [`scoreboard`], so this
/// diagnostic cannot drift into an independent definition of `unsupported`.
pub fn unsupported_destination_audit_v1(
    snapshots: &[ProgramSnapshotV1],
) -> Vec<UnsupportedDestinationAuditV1> {
    let geometry = ProgramGeometry::from_snapshots(snapshots);
    let mut incoming = BTreeMap::<u32, Vec<IncomingTransferV1>>::new();

    for snapshot in snapshots {
        for bank in &snapshot.banks {
            for (block, successor) in authority_projected_successors(bank) {
                let destination_va = successor.destination_va;
                if geometry.classify_concrete(destination_va).class()
                    != DestinationClass::Unsupported
                {
                    continue;
                }
                incoming
                    .entry(destination_va)
                    .or_default()
                    .push(IncomingTransferV1 {
                        bank: bank.input.bank.clone(),
                        block_start_va: block.start_va,
                        block_end_va: block.end_va,
                        source_site_va: successor.source_site_va,
                        kind: successor.kind,
                    });
            }
        }
    }

    incoming
        .into_iter()
        .map(|(destination_va, mut incoming)| {
            incoming.sort_unstable();
            incoming.dedup();
            UnsupportedDestinationAuditV1 {
                destination_va,
                reason: geometry.classify_concrete(destination_va),
                incoming,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, BankAddr, CandidateDetector,
        FunctionEntryEvidence, ProloguePattern, ProofState, RomAddressSpace,
    };
    use crate::normalize;
    use crate::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
    use crate::{FactDb, NormalizedRom};

    const BASE: u32 = 0x8000_0000;
    const ROM_START: u32 = 0x1000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn rom_with_bank(bank: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; ROM_START as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..].copy_from_slice(bank);
        normalize(&bytes).unwrap()
    }

    /// Facts with a proven physical mapping + executable range covering the
    /// whole bank, plus the listed authoritative function entries.
    fn facts_for(byte_len: u32, authoritative_entries: &[u32]) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + byte_len,
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude("bank:bank", ProofState::Proven, vec![mapping], "test")
            .unwrap();
        let executable = facts.insert(Fact::ExecutableRange {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + byte_len),
                ProofState::Proven,
                vec![executable],
                "test",
            )
            .unwrap();
        for &entry in authoritative_entries {
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
                    "test",
                )
                .unwrap();
        }
        facts
    }

    fn compose(
        rom: &NormalizedRom,
        facts: &FactDb,
        bytes: &[u8],
        roots: &[u32],
    ) -> ProgramSnapshotV1 {
        compose_materialized_bank_v1(
            rom,
            facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes,
                seed_roots: roots,
            },
        )
        .unwrap()
    }

    #[test]
    fn destination_inside_exact_owner_classifies_exact_aot() {
        // Caller at BASE `jal`s a callee at BASE+0x10. The caller returns
        // before the callee (BASE+8 is `jr $ra`), so the callee has NO
        // incoming fallthrough edge and forms a proven exact owner. The call
        // destination lands inside that owner => exact_aot. The shared
        // successor denominator also retains the caller's BASE+8 continuation.
        let callee = BASE + 0x10;
        let jal_callee = 0x0c00_0000 | (callee >> 2) & 0x03ff_ffff;
        let bytes = asm(&[jal_callee, NOP, JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE, callee]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, callee]);

        // The callee must actually be a proven exact owner for the assertion
        // to mean what the test name says.
        let proven_owner = snapshot.banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == callee)
            });
        assert!(proven_owner, "callee is a proven exact owner: {snapshot:?}");

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(
            board.tally(DestinationClass::ExactAot).destinations,
            2,
            "the jal destination and return continuation are exact-owner successors: {board:?}"
        );
        assert_eq!(board.unsupported, 0);
        assert_eq!(board.per_class.len(), DestinationClass::ALL.len());
        assert!(DestinationClass::ALL
            .into_iter()
            .all(|class| board.per_class.contains_key(class.label())));
        assert_eq!(board.per_reason.len(), DestinationReason::ALL.len());
        assert!(DestinationReason::ALL
            .into_iter()
            .all(|reason| board.per_reason.contains_key(reason.label())));
    }

    #[test]
    fn reached_block_not_in_owner_classifies_block_aot() {
        // An AUTHORITATIVE function whose body contains an OPEN indirect
        // (`jr $t0`, register never constructed). The open indirect denies the
        // function an exact-owner extent, but the branch's fall-through and
        // taken blocks are each individually proven reachable code. The branch
        // target therefore classifies block_aot: proven block, no exact owner.
        //
        //   BASE:   beq $zero,$zero,+3   (taken target = BASE+0x10)
        //   BASE+4: nop                  (delay slot)
        //   BASE+8: jr   $t0             (OPEN indirect -> owner not exact)
        //   BASE+c: nop                  (indirect delay slot)
        //   BASE+10: jr  $ra ; nop       (the branch target block, proven)
        let beq = 0x1000_0003;
        let jr_t0 = 0x0100_0008;
        let bytes = asm(&[beq, NOP, jr_t0, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        // Precondition: BASE is NOT a proven exact owner (open indirect), yet
        // the branch-target block IS proven.
        let has_proven_owner = snapshot.banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(assessment, OwnerAssessment::Proven { .. }));
        assert!(
            !has_proven_owner,
            "the open indirect must deny an exact owner: {snapshot:?}"
        );
        assert!(
            snapshot.banks[0].block_proof.proven_blocks >= 1,
            "at least one block proves: {snapshot:?}"
        );

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(
            board.tally(DestinationClass::ExactAot).destinations,
            0,
            "no exact owner => nothing exact_aot: {board:?}"
        );
        assert!(
            board.tally(DestinationClass::BlockAot).destinations >= 1,
            "the branch target is a proven reachable block: {board:?}"
        );
    }

    #[test]
    fn proven_code_outside_every_admitted_block_gets_its_own_label() {
        // A word the CFG PROVED is code, which block proof nonetheless did not
        // admit. It used to fall into the catch-all arm and be reported as
        // `mapped_not_proven_code` -- indistinguishable from a word that never
        // decoded -- so a block-admission refusal read as a decoding failure.
        let geometry = ProgramGeometry {
            owners: Vec::new(),
            blocks: Vec::new(),
            mapped: vec![MappedVaRange {
                start: BASE,
                end: BASE + 0x100,
            }],
            word_class: BTreeMap::from([
                (BASE, WordClass::ProvenCode),
                (BASE + 4, WordClass::CandidateCode),
                (BASE + 8, WordClass::Unknown),
                (BASE + 12, WordClass::ProvenData),
                (BASE + 16, WordClass::Conflict),
            ]),
        };

        assert_eq!(
            geometry.classify_concrete(BASE),
            DestinationReason::ProvenCodeNoOwner
        );
        for va in [BASE + 4, BASE + 8] {
            assert_eq!(
                geometry.classify_concrete(va),
                DestinationReason::MappedNotProvenCode,
                "unproven words keep the original label: {va:#010x}"
            );
        }
        for va in [BASE + 12, BASE + 16] {
            assert_eq!(
                geometry.classify_concrete(va),
                DestinationReason::IntoProvenData,
                "proven data and conflicts are unchanged: {va:#010x}"
            );
        }
        // A word never visited at all is absent from the map entirely.
        assert_eq!(
            geometry.classify_concrete(BASE + 0x20),
            DestinationReason::MappedNotProvenCode
        );

        // Labelling split only: the class -- and therefore every headline
        // count -- is unchanged.
        assert_eq!(
            DestinationReason::ProvenCodeNoOwner.class(),
            DestinationReason::MappedNotProvenCode.class()
        );
        assert_eq!(
            DestinationReason::ProvenCodeNoOwner.class(),
            DestinationClass::DynamicMips
        );
    }

    #[test]
    fn open_indirect_targets_classify_dynamic_mips() {
        // `jr $t0` with $t0 never constructed: an OPEN indirect site.
        let jr_t0 = 0x0100_0008;
        let bytes = asm(&[jr_t0, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(
            board.dynamic_mips, 1,
            "one open indirect site is one dynamic_mips destination: {board:?}"
        );
        assert_eq!(
            *board.per_reason.get("open_indirect_site").unwrap(),
            1,
            "{board:?}"
        );
        assert_eq!(board.unsupported, 0);
    }

    #[test]
    fn transfer_into_unmapped_classifies_unsupported() {
        // A `j` to an address far outside the mapped bank. The tail target is
        // concrete and lands outside every mapping.
        let far = 0x8090_0000u32;
        let j_far = 0x0800_0000 | (far >> 2) & 0x03ff_ffff;
        let bytes = asm(&[j_far, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(
            board.unsupported, 1,
            "the j-target is outside every mapping: {board:?}"
        );
        assert_eq!(
            *board.per_reason.get("outside_all_mappings").unwrap(),
            1,
            "{board:?}"
        );

        let audit = unsupported_destination_audit_v1(std::slice::from_ref(&snapshot));
        assert_eq!(
            audit,
            vec![UnsupportedDestinationAuditV1 {
                destination_va: far,
                reason: DestinationReason::OutsideAllMappings,
                incoming: vec![IncomingTransferV1 {
                    bank: "bank".to_string(),
                    block_start_va: BASE,
                    block_end_va: BASE + 8,
                    source_site_va: BASE,
                    kind: ConcreteTransferKind::Tail,
                }],
            }]
        );
    }

    #[test]
    fn candidate_traversal_root_cannot_manufacture_an_unsupported_transfer() {
        let far = 0x8090_0000u32;
        let jal_far = 0x0c00_0000 | (far >> 2) & 0x03ff_ffff;
        let bytes = asm(&[JR_RA, NOP, jal_far, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);

        // BASE+8 is useful as an exploratory traversal hint, but only BASE is
        // an authoritative function entry. The broad CFG may inspect the
        // jal-shaped bytes without making their target executable evidence.
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 8]);
        assert!(snapshot.banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE + 8));
        assert!(!snapshot.banks[0]
            .authority_closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE + 8));

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(board.unsupported, 0, "{board:?}");
        assert!(!classified_destinations(std::slice::from_ref(&snapshot))
            .iter()
            .any(|destination| destination.va == far));
        assert!(unsupported_destination_audit_v1(std::slice::from_ref(&snapshot)).is_empty());
    }

    #[test]
    fn authoritative_non_owner_block_retains_its_concrete_transfer() {
        let far = 0x8090_0000u32;
        let jump_far = 0x0800_0000 | (far >> 2) & 0x03ff_ffff;
        let branch_to_jump = 0x1000_0003;
        let jr_t0 = 0x0100_0008;
        let bytes = asm(&[branch_to_jump, NOP, jr_t0, NOP, jump_far, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        assert!(!snapshot.banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(assessment, OwnerAssessment::Proven { .. })));
        assert!(snapshot.banks[0]
            .block_proof
            .assessments
            .iter()
            .any(|assessment| matches!(assessment, BlockAssessment::Proven { block } if block.start_va == BASE + 0x10)));

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(board.unsupported, 1, "{board:?}");
        let audit = unsupported_destination_audit_v1(std::slice::from_ref(&snapshot));
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].destination_va, far);
        assert_eq!(audit[0].incoming[0].source_site_va, BASE + 0x10);
    }

    #[test]
    fn plain_fallthrough_past_mapped_bank_end_is_retained_as_unsupported() {
        let bytes = asm(&[NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let mut snapshot = compose(&rom, &facts, &bytes, &[BASE]);
        assert_eq!(
            snapshot.banks[0]
                .authority_closure
                .cfg
                .word_class
                .get(&BASE),
            Some(&WordClass::ProvenCode)
        );
        assert!(matches!(
            snapshot.banks[0].authority_closure.cfg.blocks[0].terminator,
            BlockTerminator::RanOffEnd
        ));
        let block = snapshot.banks[0]
            .closure
            .cfg
            .blocks
            .iter_mut()
            .find(|block| block.start_va == BASE)
            .expect("one-word root block");
        assert!(matches!(block.terminator, BlockTerminator::RanOffEnd));
        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(board.unsupported, 1, "{board:?}");
        assert_eq!(
            classified_destinations(std::slice::from_ref(&snapshot)),
            vec![ClassifiedDestination {
                va: BASE + 4,
                reason: DestinationReason::OutsideAllMappings,
            }]
        );
        let audit = unsupported_destination_audit_v1(std::slice::from_ref(&snapshot));
        assert_eq!(
            audit,
            vec![UnsupportedDestinationAuditV1 {
                destination_va: BASE + 4,
                reason: DestinationReason::OutsideAllMappings,
                incoming: vec![IncomingTransferV1 {
                    bank: "bank".to_string(),
                    block_start_va: BASE,
                    block_end_va: BASE + 4,
                    source_site_va: BASE,
                    kind: ConcreteTransferKind::RanOffEndFallthrough,
                }],
            }]
        );
    }

    #[test]
    fn call_target_and_end_of_bank_continuation_are_both_retained() {
        let far = 0x8090_0000u32;
        let jal_far = 0x0c00_0000 | (far >> 2) & 0x03ff_ffff;
        let bytes = asm(&[jal_far, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        let board = scoreboard(std::slice::from_ref(&snapshot));
        assert_eq!(board.unsupported, 2, "{board:?}");
        let audit = unsupported_destination_audit_v1(std::slice::from_ref(&snapshot));
        assert_eq!(
            audit
                .iter()
                .map(|destination| destination.destination_va)
                .collect::<Vec<_>>(),
            [BASE + 8, far]
        );
        assert_eq!(
            audit[0].incoming[0].kind,
            ConcreteTransferKind::CallContinuation
        );
        assert_eq!(audit[1].incoming[0].kind, ConcreteTransferKind::Call);
    }

    #[test]
    fn typed_successor_enumerator_retains_all_modeled_edge_shapes() {
        let call = BasicBlock {
            start_va: BASE,
            end_va: BASE + 8,
            terminator: BlockTerminator::Call {
                target: BASE + 0x40,
                next: BASE + 8,
            },
        };
        assert_eq!(
            concrete_successors(&call),
            [
                ConcreteSuccessor {
                    destination_va: BASE + 0x40,
                    source_site_va: BASE,
                    kind: ConcreteTransferKind::Call,
                },
                ConcreteSuccessor {
                    destination_va: BASE + 8,
                    source_site_va: BASE,
                    kind: ConcreteTransferKind::CallContinuation,
                },
            ]
        );

        let branch = BasicBlock {
            start_va: BASE,
            end_va: BASE + 8,
            terminator: BlockTerminator::BranchLikely {
                target: BASE + 0x40,
                fallthrough: BASE + 8,
                link: true,
            },
        };
        assert_eq!(concrete_successors(&branch).len(), 2);
        assert_eq!(
            concrete_successors(&branch)[1].kind,
            ConcreteTransferKind::BranchLikelyFallthrough { link: true }
        );

        let indirect_call = BasicBlock {
            start_va: BASE,
            end_va: BASE + 8,
            terminator: BlockTerminator::ResolvedIndirect {
                targets: vec![BASE + 0x40, BASE + 0x80],
                via_call: true,
            },
        };
        let successors = concrete_successors(&indirect_call);
        assert_eq!(successors.len(), 3);
        assert_eq!(
            successors[2],
            ConcreteSuccessor {
                destination_va: BASE + 8,
                source_site_va: BASE,
                kind: ConcreteTransferKind::ResolvedIndirectCallContinuation,
            }
        );

        let fallthrough = BasicBlock {
            start_va: BASE,
            end_va: BASE + 4,
            terminator: BlockTerminator::Fallthrough { next: BASE + 4 },
        };
        assert_eq!(
            concrete_successors(&fallthrough),
            [ConcreteSuccessor {
                destination_va: BASE + 4,
                source_site_va: BASE,
                kind: ConcreteTransferKind::PlainFallthrough,
            }]
        );

        let ran_off_end = BasicBlock {
            start_va: BASE,
            end_va: BASE + 4,
            terminator: BlockTerminator::RanOffEnd,
        };
        assert_eq!(
            concrete_successors(&ran_off_end),
            [ConcreteSuccessor {
                destination_va: BASE + 4,
                source_site_va: BASE,
                kind: ConcreteTransferKind::RanOffEndFallthrough,
            }]
        );
    }

    #[test]
    fn unsupported_audit_retains_and_deduplicates_every_incoming_edge() {
        let far = 0x8090_0000u32;
        let j_far = 0x0800_0000 | (far >> 2) & 0x03ff_ffff;
        // Two independently authoritative blocks tail to the same unmapped
        // target. Both source edges must survive while the destination remains
        // one scoreboard item.
        let bytes = asm(&[j_far, NOP, j_far, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE, BASE + 8]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 8]);

        let audit = unsupported_destination_audit_v1(std::slice::from_ref(&snapshot));
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].destination_va, far);
        assert_eq!(audit[0].incoming.len(), 2, "{audit:?}");
        assert_eq!(audit[0].incoming[0].source_site_va, BASE);
        assert_eq!(audit[0].incoming[1].source_site_va, BASE + 8);

        let duplicate_snapshots = [snapshot.clone(), snapshot];
        assert_eq!(
            unsupported_destination_audit_v1(&duplicate_snapshots),
            audit,
            "duplicate snapshot evidence must not duplicate identical edges"
        );
    }

    #[test]
    fn two_pc_fake_broad_edge_at_proven_source_is_not_projected() {
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let mut snapshot = compose(&rom, &facts, &bytes, &[BASE]);

        snapshot.banks[0].closure.cfg.blocks[0].terminator =
            BlockTerminator::Tail { target: BASE + 8 };

        assert!(authority_projected_successors(&snapshot.banks[0]).is_empty());
    }

    #[test]
    fn two_pc_internal_plain_fallthrough_refines_only_ordinary_authority_prefix() {
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let mut snapshot = compose(&rom, &facts, &bytes, &[BASE]);
        let broad = &mut snapshot.banks[0].closure.cfg.blocks[0];

        broad.end_va = BASE + 4;
        broad.terminator = BlockTerminator::Fallthrough { next: BASE + 4 };
        assert_eq!(
            authority_projected_successors(&snapshot.banks[0])
                .into_iter()
                .map(|(_, successor)| successor)
                .collect::<Vec<_>>(),
            [ConcreteSuccessor {
                destination_va: BASE + 4,
                source_site_va: BASE,
                kind: ConcreteTransferKind::PlainFallthrough,
            }]
        );

        let broad = &mut snapshot.banks[0].closure.cfg.blocks[0];
        broad.terminator = BlockTerminator::Fallthrough { next: BASE + 8 };
        assert!(authority_projected_successors(&snapshot.banks[0]).is_empty());

        let broad = &mut snapshot.banks[0].closure.cfg.blocks[0];
        broad.end_va = BASE + 8;
        broad.terminator = BlockTerminator::Tail { target: BASE + 4 };
        assert!(authority_projected_successors(&snapshot.banks[0]).is_empty());

        let broad = &mut snapshot.banks[0].closure.cfg.blocks[0];
        broad.end_va = BASE + 12;
        broad.terminator = BlockTerminator::Fallthrough { next: BASE + 12 };
        assert!(authority_projected_successors(&snapshot.banks[0]).is_empty());
    }

    #[test]
    fn scoreboard_is_deterministic() {
        let jal_callee = 0x0c00_0000 | ((BASE + 8) >> 2) & 0x03ff_ffff;
        let bytes = asm(&[jal_callee, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE, BASE + 8]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 8]);
        let first = serde_json::to_string(&scoreboard(std::slice::from_ref(&snapshot))).unwrap();
        for _ in 0..10 {
            let again =
                serde_json::to_string(&scoreboard(std::slice::from_ref(&snapshot))).unwrap();
            assert_eq!(again, first);
        }
    }
}
