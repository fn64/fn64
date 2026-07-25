//! Execution-closure scoreboard: a pure MEASUREMENT layer over the facts an
//! already-composed [`ProgramSnapshotV1`] carries.
//!
//! The full-game gate (DISCOVER-PLAN, UNIVERSAL-RUNTIME-PLAN) is "zero
//! `unsupported` execution destinations": every reachable CPU transfer
//! destination must be classifiable as `exact_aot`, `block_aot`,
//! `dynamic_mips`, or `unsupported`. This module counts that classification
//! per ROM (across all composed banks). It reads only the closure, owner
//! proof, and block proof already produced by [`crate::snapshot`]; it never
//! runs discovery, never mutates a fact, and never promotes anything.
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

use crate::block_proof::BlockAssessment;
use crate::cfg::{BlockTerminator, WordClass};
use crate::facts::Fact;
use crate::owner_proof::OwnerAssessment;
use crate::snapshot::ProgramSnapshotV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
pub struct ClassTally {
    pub destinations: u64,
    pub bytes: u64,
}

/// The scoreboard for one ROM: the per-class tallies, the reason histogram,
/// and the two headline numbers (`unsupported` — the release blocker — and
/// `dynamic_mips` — fallback-covered, reported honestly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Enumerate every reachable CPU transfer destination across all composed
/// banks of one ROM and classify each. Concrete destinations (branch/call/
/// tail/resolved-indirect targets) are deduplicated by VA so a hot function
/// reached a thousand times counts once. OPEN and BOUNDED indirect SITES are
/// counted per site (each is a distinct place the CPU can leave statically
/// closed code) and carry no single VA.
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
    let mut per_reason: BTreeMap<DestinationReason, u64> = BTreeMap::new();

    for snapshot in snapshots {
        for bank in &snapshot.banks {
            let cfg = &bank.closure.cfg;
            for block in &cfg.blocks {
                match &block.terminator {
                    BlockTerminator::Tail { target } => record(*target, &geometry, &mut concrete),
                    BlockTerminator::Call { target, .. } => {
                        record(*target, &geometry, &mut concrete)
                    }
                    BlockTerminator::Branch { target, .. }
                    | BlockTerminator::BranchLikely { target, .. } => {
                        record(*target, &geometry, &mut concrete)
                    }
                    BlockTerminator::ResolvedIndirect { targets, .. } => {
                        for &target in targets {
                            record(target, &geometry, &mut concrete);
                        }
                    }
                    // Open indirect: counted below from `closure.indirect`,
                    // which is the authoritative per-site record. Fallthrough,
                    // Return, Trap, and the malformed terminators are not
                    // transfer destinations.
                    BlockTerminator::Indirect { .. }
                    | BlockTerminator::Fallthrough { .. }
                    | BlockTerminator::Return
                    | BlockTerminator::Trap
                    | BlockTerminator::InvalidInstruction { .. }
                    | BlockTerminator::MissingDelaySlot { .. }
                    | BlockTerminator::RanOffEnd
                    | BlockTerminator::DataFence { .. } => {}
                }
            }
            // Indirect SITES: one entry per site. Exhaustive resolutions are
            // already reflected as concrete `ResolvedIndirect` targets above,
            // so only Open/Bounded sites are counted here.
            for resolution in &bank.closure.indirect {
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
    let mut per_class: BTreeMap<DestinationClass, ClassTally> = BTreeMap::new();
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
            .map(|(reason, count)| (reason_label(reason).to_string(), count))
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
            for block in &bank.closure.cfg.blocks {
                let targets: Vec<u32> = match &block.terminator {
                    BlockTerminator::Tail { target }
                    | BlockTerminator::Call { target, .. }
                    | BlockTerminator::Branch { target, .. }
                    | BlockTerminator::BranchLikely { target, .. } => vec![*target],
                    BlockTerminator::ResolvedIndirect { targets, .. } => targets.clone(),
                    _ => Vec::new(),
                };
                for target in targets {
                    concrete
                        .entry(target)
                        .or_insert_with(|| geometry.classify_concrete(target));
                }
            }
        }
    }
    concrete
        .into_iter()
        .map(|(va, reason)| ClassifiedDestination { va, reason })
        .collect()
}

fn reason_label(reason: DestinationReason) -> &'static str {
    match reason {
        DestinationReason::InExactOwner => "in_exact_owner",
        DestinationReason::InProvenBlock => "in_proven_block",
        DestinationReason::OpenIndirectSite => "open_indirect_site",
        DestinationReason::BoundedIndirectSite => "bounded_indirect_site",
        DestinationReason::MappedNotProvenCode => "mapped_not_proven_code",
        DestinationReason::ProvenCodeNoOwner => "proven_code_no_owner",
        DestinationReason::IntoProvenData => "into_proven_data",
        DestinationReason::OutsideAllMappings => "outside_all_mappings",
    }
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
        // destination lands inside that owner => exact_aot.
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
            1,
            "exactly the jal destination is inside an exact owner: {board:?}"
        );
        assert_eq!(board.unsupported, 0);
    }

    #[test]
    fn reached_block_not_in_owner_classifies_block_aot() {
        // An AUTHORITATIVE function whose body contains an OPEN indirect
        // (`jr $t0`, register never constructed). The open indirect denies the
        // function an exact-owner extent, but the branch's fall-through and
        // taken blocks are each individually proven reachable code. The branch
        // target therefore classifies block_aot: proven block, no exact owner.
        //
        //   BASE:   beq $zero,$zero,+2   (taken target = BASE+0x0c)
        //   BASE+4: nop                  (delay slot)
        //   BASE+8: jr   $t0             (OPEN indirect -> owner not exact)
        //   BASE+c: jr   $ra ; nop       (the branch target block, proven)
        let beq = 0x1000_0002;
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
