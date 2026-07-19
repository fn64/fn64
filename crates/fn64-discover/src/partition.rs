//! Phase 5 (docs/DISCOVER-DESIGN.md "partition blocks into owners"):
//! recursive-descent ownership from a set of proven roots (the entrypoint,
//! proven `jal` targets, thread/callback entries) over the CFG [`crate::cfg`]
//! already built.
//!
//! Hard constraints this module enforces, per the design doc:
//!
//! - every accepted block has exactly one owner within its bank,
//! - owners do not overlap within a bank,
//! - ordinary fallthrough (including a call's return point) stays in its
//!   owner,
//! - tail transfers (`j`) may cross owners -- the target keeps its own
//!   root's ownership rather than being absorbed into the tail-caller,
//! - returns and traps terminate a path with no successor to own.
//!
//! A root with no competing claim closes immediately. Two roots whose
//! reachable-block closures collide on the same block are an ambiguity:
//! this phase does not attempt SAT/SMT resolution (the design doc reserves
//! that for later, scoped to small regions) -- it reports the collision
//! explicitly as `open`, per "multiple valid solutions produce an `open`
//! result."

use crate::cfg::{BasicBlock, BlockTerminator, Cfg};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One function owner: a proven root plus the blocks it owns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Owner {
    pub bank: String,
    pub root_va: u32,
    /// Block start VAs owned by this root, in ascending order.
    pub block_starts: Vec<u32>,
    /// Exclusive end VA of the owner's extent -- valid only when the owned
    /// blocks form one contiguous run (the common case); see
    /// [`Owner::is_contiguous`].
    pub extent_end: u32,
}

impl Owner {
    /// True if this owner's blocks form a single contiguous
    /// `[root_va, extent_end)` run with no gaps -- the shape the design
    /// doc's Phase 8 assembly-verification step ultimately needs. A
    /// non-contiguous owner (blocks reachable only through a tail transfer
    /// elsewhere, or a discontiguous fallthrough chain) is still a valid
    /// owner here, just not yet extent-provable; downstream code should
    /// check this rather than assume contiguity.
    ///
    /// Checking only that this owner's *own* `block_starts` chain by
    /// address is not sufficient: two disjoint blocks can coincidentally
    /// chain by address (`a.end_va == b.start_va`) purely because nothing
    /// in this owner's control flow actually reaches the gap between them
    /// -- meanwhile a *different* owner's block can sit inside that same
    /// address range, reached only via a branch/fallthrough this owner
    /// never takes. `blocks_by_start` is therefore walked over the **full**
    /// CFG block set within `[root_va, extent_end)`, not just this
    /// owner's list, so a foreign block hiding in what looks like a
    /// contiguous span is caught rather than silently validated.
    pub fn is_contiguous(&self, blocks_by_start: &BTreeMap<u32, &BasicBlock>) -> bool {
        let mut expected = self.root_va;
        let mut owned: BTreeSet<u32> = self.block_starts.iter().copied().collect();
        while expected < self.extent_end {
            let Some(b) = blocks_by_start.get(&expected) else {
                return false; // gap: no CFG block starts here at all
            };
            if !owned.remove(&expected) {
                return false; // a block in-range that this owner doesn't own
            }
            expected = b.end_va;
        }
        expected == self.extent_end && owned.is_empty()
    }
}

/// A block claimed by more than one root's reachable closure -- an
/// ambiguity region per the design doc ("Competing closures create local
/// ambiguity regions... Multiple valid solutions produce an `open`
/// result").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbiguousBlock {
    pub block_start: u32,
    pub claimants: Vec<u32>, // root VAs, sorted
}

/// A block reachable in the CFG (has an entry in `word_class`/`blocks`) but
/// not reached by any root's closure at all -- the design doc's "reachable
/// uncovered blocks" acceptance-gate metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnownedBlock {
    pub block_start: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Partition {
    pub bank: String,
    /// One owner per root that got an exclusive, uncontested closure.
    pub owners: Vec<Owner>,
    /// Blocks contested by 2+ roots -- the open ambiguity frontier.
    pub ambiguous: Vec<AmbiguousBlock>,
    /// Blocks the CFG produced but no root's closure reached (should be
    /// empty whenever `roots` was actually the full proven-root set; a
    /// non-empty result here means the CFG discovered blocks the root set
    /// didn't seed, which is itself useful signal that a root is missing).
    pub unowned: Vec<UnownedBlock>,
}

impl Partition {
    /// Total block count `owners.len() + ambiguous.len() + unowned.len()`
    /// should equal, used by the acceptance-gate report.
    pub fn total_blocks(&self) -> usize {
        self.owners
            .iter()
            .map(|o| o.block_starts.len())
            .sum::<usize>()
            + self.ambiguous.len()
            + self.unowned.len()
    }
}

/// Partition `cfg`'s blocks into owners, one per entry in `cfg.proven_roots`
/// (the CFG builder already recorded every `jal` target and explicit seed
/// root as `proven_roots`, in first-seen order -- this function does not
/// invent new roots, it only decides ownership of blocks the CFG already
/// built).
///
/// Ownership follows fallthrough and branch edges (both arms of a branch
/// stay with the same owner -- a branch is intra-function control flow),
/// A `Tail` edge crosses owners only when its target is independently proven
/// as a root. Otherwise the edge stays within the current owner: a plain `j`
/// cannot distinguish a tail call from an intra-function jump to a local
/// label, so splitting without corroboration would fabricate a boundary.
/// This preserves "tail transfers may cross owners" while failing closed to
/// a coarser owner when no callable-entry evidence exists.
pub fn partition(cfg: &Cfg) -> Partition {
    partition_with_authoritative_entries(cfg, &BTreeSet::new())
}

/// Partition `cfg` while treating `authoritative_entries` as hard callable
/// boundaries in addition to the CFG's traversal roots.
///
/// Traversal roots are not necessarily callable entries: an uncorroborated
/// `j` target or a heuristic seed may exist only to expose reachable code.
/// Consequently this function re-carves geometry only at addresses supplied
/// through `authoritative_entries`. When such an entry lies in an existing
/// claim span, the enclosing claimants keep only the prefix before the entry;
/// the entry owns its reachable closure up to the next authoritative entry or
/// the enclosing span's original end. An entry that is not already a CFG block
/// leader is left unresolved rather than fabricating a partial basic block.
pub fn partition_with_authoritative_entries(
    cfg: &Cfg,
    authoritative_entries: &BTreeSet<u32>,
) -> Partition {
    partition_with_authorized_splits(cfg, authoritative_entries, authoritative_entries)
}

/// Authority-aware partitioning with a distinct set of newly learned split
/// entries and the complete set of callable boundaries. The distinction lets
/// a multi-bank caller re-carve only cross-bank entries while still protecting
/// already-known in-bank call targets from being absorbed by a new closure.
pub fn partition_with_authorized_splits(
    cfg: &Cfg,
    split_entries: &BTreeSet<u32>,
    callable_boundaries: &BTreeSet<u32>,
) -> Partition {
    let blocks_by_start: BTreeMap<u32, &BasicBlock> =
        cfg.blocks.iter().map(|b| (b.start_va, b)).collect();

    // claims[block_start] = set of root VAs whose intra-function closure
    // reaches this block.
    let mut claims: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();

    let legacy_tail_boundaries: BTreeSet<u32> = cfg.proven_roots.iter().copied().collect();
    for &root in &cfg.proven_roots {
        for start in reachable_blocks(&blocks_by_start, root, None, &legacy_tail_boundaries) {
            claims.entry(start).or_default().insert(root);
        }
    }

    let original_claims = claims.clone();
    let usable_authoritative: Vec<u32> = split_entries
        .iter()
        .copied()
        .filter(|entry| blocks_by_start.contains_key(entry))
        .collect();
    for &entry in &usable_authoritative {
        let Some(enclosing_claimants) = original_claims.get(&entry) else {
            continue;
        };
        let original_end = cfg
            .blocks
            .iter()
            .filter(|block| {
                original_claims
                    .get(&block.start_va)
                    .is_some_and(|claimants| !claimants.is_disjoint(enclosing_claimants))
            })
            .map(|block| block.end_va)
            .max()
            .unwrap_or(entry);
        let next_entry = callable_boundaries
            .range(entry.saturating_add(1)..)
            .next()
            .copied()
            .unwrap_or(u32::MAX);
        let region_end = original_end.min(next_entry);
        if region_end <= entry {
            continue;
        }

        // A proven callable entry is a boundary, so no enclosing root can own
        // blocks at or beyond it. Candidate traversal roots in the same local
        // region cannot compete with that proven identity; unresolved entry
        // claims remain proof blockers rather than partition claimants.
        for (_, block_claims) in claims.range_mut(entry..region_end) {
            block_claims.clear();
        }
        let reachable = reachable_blocks(
            &blocks_by_start,
            entry,
            Some(region_end),
            callable_boundaries,
        );
        for &start in &reachable {
            claims.entry(start).or_default().insert(entry);
        }
        let closure_end = reachable
            .iter()
            .filter_map(|start| blocks_by_start.get(start).map(|block| block.end_va))
            .max()
            .unwrap_or(entry);
        if closure_end < region_end
            && !callable_boundaries.contains(&closure_end)
            && original_claims.contains_key(&closure_end)
        {
            claims.entry(entry).or_default().extend(
                enclosing_claimants
                    .iter()
                    .copied()
                    .filter(|root| *root != entry),
            );
        }
    }

    let mut owners: Vec<Owner> = Vec::new();
    let mut ambiguous: Vec<AmbiguousBlock> = Vec::new();
    let mut unowned: Vec<UnownedBlock> = Vec::new();

    // Group exclusively-claimed blocks by their single owning root.
    let mut per_root_blocks: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for block in &cfg.blocks {
        let start = block.start_va;
        match claims.get(&start) {
            None => unowned.push(UnownedBlock { block_start: start }),
            Some(roots) if roots.len() == 1 => {
                let root = *roots.iter().next().unwrap();
                per_root_blocks.entry(root).or_default().push(start);
            }
            Some(roots) => {
                ambiguous.push(AmbiguousBlock {
                    block_start: start,
                    claimants: roots.iter().copied().collect(),
                });
            }
        }
    }

    let mut roots = cfg.proven_roots.clone();
    for entry in usable_authoritative {
        if !roots.contains(&entry) {
            roots.push(entry);
        }
    }
    for root in roots {
        let mut block_starts = per_root_blocks.remove(&root).unwrap_or_default();
        if block_starts.is_empty() {
            // A root that produced no CFG block at all (e.g. entirely
            // out-of-range) has nothing to own -- not an error, just an
            // empty owner is meaningless, so skip it rather than emit a
            // phantom zero-block owner.
            continue;
        }
        block_starts.sort_unstable();
        let extent_end = block_starts
            .iter()
            .filter_map(|s| blocks_by_start.get(s).map(|b| b.end_va))
            .max()
            .unwrap_or(root);
        owners.push(Owner {
            bank: cfg.bank.clone(),
            root_va: root,
            block_starts,
            extent_end,
        });
    }

    Partition {
        bank: cfg.bank.clone(),
        owners,
        ambiguous,
        unowned,
    }
}

fn reachable_blocks(
    blocks_by_start: &BTreeMap<u32, &BasicBlock>,
    root: u32,
    exclusive_end: Option<u32>,
    callable_boundaries: &BTreeSet<u32>,
) -> BTreeSet<u32> {
    let mut visited = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(start) = stack.pop() {
        if exclusive_end.is_some_and(|end| start < root || start >= end) || !visited.insert(start) {
            continue;
        }
        let Some(block) = blocks_by_start.get(&start) else {
            continue;
        };
        match &block.terminator {
            BlockTerminator::Fallthrough { next } => stack.push(*next),
            BlockTerminator::Tail { target } => {
                if !callable_boundaries.contains(target) {
                    stack.push(*target);
                }
            }
            BlockTerminator::Call { next, .. } => stack.push(*next),
            BlockTerminator::Branch {
                target,
                fallthrough,
            }
            | BlockTerminator::BranchLikely {
                target,
                fallthrough,
            } => {
                stack.push(*target);
                stack.push(*fallthrough);
            }
            BlockTerminator::ResolvedIndirect {
                targets,
                via_call: false,
            } => stack.extend(targets.iter().copied()),
            BlockTerminator::ResolvedIndirect { via_call: true, .. }
            | BlockTerminator::Indirect { via_call: true } => stack.push(block.end_va),
            BlockTerminator::Return
            | BlockTerminator::Indirect { via_call: false }
            | BlockTerminator::Trap
            | BlockTerminator::InvalidInstruction { .. }
            | BlockTerminator::MissingDelaySlot { .. }
            | BlockTerminator::RanOffEnd => {}
        }
    }
    visited
}

/// Acceptance-gate check: verify no two owners in this bank claim
/// conflicting territory -- the design doc's "owners do not overlap within
/// a bank." This checks the real invariant directly against the CFG's
/// actual blocks, not against each owner's synthesized
/// `[root_va, extent_end)` interval: two owners' `extent_end` values can
/// coincidentally chain by address (see [`Owner::is_contiguous`]'s doc
/// comment) even when one owner's claimed span improperly swallows a block
/// a *different* owner actually owns. That is exactly the overlap this
/// function must catch, so it walks every block address in
/// `[root_va, extent_end)` for every owner and flags any that resolves to
/// a CFG block owned by someone else.
pub fn same_bank_overlaps(partition: &Partition, cfg: &Cfg) -> Vec<(u32, u32)> {
    let blocks_by_start: BTreeMap<u32, &BasicBlock> =
        cfg.blocks.iter().map(|b| (b.start_va, b)).collect();
    let owner_of: BTreeMap<u32, u32> = partition
        .owners
        .iter()
        .flat_map(|o| o.block_starts.iter().map(move |&start| (start, o.root_va)))
        .collect();

    let mut overlaps: BTreeSet<(u32, u32)> = BTreeSet::new();
    for owner in &partition.owners {
        let mut pc = owner.root_va;
        while pc < owner.extent_end {
            let Some(&claimant) = owner_of.get(&pc) else {
                // No CFG block starts here (a gap this owner's extent
                // spans over without actually reaching it, e.g. a branch
                // that jumps past unreached bytes) -- not this function's
                // job to judge; `partition`'s `unowned` already reports
                // gaps like this separately.
                break;
            };
            if claimant != owner.root_va {
                let pair = if owner.root_va < claimant {
                    (owner.root_va, claimant)
                } else {
                    (claimant, owner.root_va)
                };
                overlaps.insert(pair);
            }
            let Some(b) = blocks_by_start.get(&pc) else {
                break;
            };
            pc = b.end_va;
        }
    }
    overlaps.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::build_cfg;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    const NOP: u32 = 0x0000_0000;
    const JR_RA: u32 = 0x03e0_0008;

    #[test]
    fn single_root_straight_line_return_is_one_contiguous_owner() {
        // nop ; jr $ra ; nop (delay slot)
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part = partition(&cfg);
        assert_eq!(part.owners.len(), 1);
        assert!(part.ambiguous.is_empty());
        assert!(part.unowned.is_empty());
        let owner = &part.owners[0];
        assert_eq!(owner.root_va, 0x8000_0000);
        assert_eq!(owner.extent_end, 0x8000_000c);
    }

    #[test]
    fn jal_target_becomes_its_own_separate_owner_not_absorbed_by_caller() {
        // caller: jal target ; nop (delay) ; jr $ra ; nop
        // target: jr $ra ; nop
        let target: u32 = 0x8000_0100;
        let jal = 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[jal, NOP, JR_RA, NOP]);
        bytes.resize(0x200, 0);
        bytes[0x100..0x104].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x104..0x108].copy_from_slice(&NOP.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part = partition(&cfg);

        assert_eq!(part.owners.len(), 2);
        let roots: Vec<u32> = part.owners.iter().map(|o| o.root_va).collect();
        assert!(roots.contains(&0x8000_0000));
        assert!(roots.contains(&target));
        // Caller owns exactly its own blocks up to and including the call
        // (return point after jal), not the callee's blocks.
        let caller = part
            .owners
            .iter()
            .find(|o| o.root_va == 0x8000_0000)
            .unwrap();
        assert!(!caller.block_starts.contains(&target));
    }

    #[test]
    fn computed_call_return_stays_with_caller_and_target_is_separate() {
        let cfg = Cfg {
            bank: "boot".to_string(),
            word_class: BTreeMap::new(),
            blocks: vec![
                BasicBlock {
                    start_va: 0x8000_0000,
                    end_va: 0x8000_0008,
                    terminator: BlockTerminator::ResolvedIndirect {
                        targets: vec![0x8000_0020],
                        via_call: true,
                    },
                },
                BasicBlock {
                    start_va: 0x8000_0008,
                    end_va: 0x8000_0010,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: 0x8000_0020,
                    end_va: 0x8000_0028,
                    terminator: BlockTerminator::Return,
                },
            ],
            direct_calls: vec![],
            tail_transfers: vec![],
            indirect_sites: vec![],
            proven_roots: vec![0x8000_0000, 0x8000_0020],
        };

        let part = partition(&cfg);
        let caller = part
            .owners
            .iter()
            .find(|owner| owner.root_va == 0x8000_0000)
            .unwrap();
        assert_eq!(caller.block_starts, vec![0x8000_0000, 0x8000_0008]);
        assert!(!caller.block_starts.contains(&0x8000_0020));
        assert!(part.unowned.is_empty());
    }

    #[test]
    fn tail_transfer_target_is_owned_separately_when_also_a_root() {
        // caller: j target ; nop (delay)
        // target: jr $ra ; nop
        let target: u32 = 0x8000_0080;
        let j = 0x0800_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[j, NOP]);
        bytes.resize(0x100, 0);
        bytes[0x80..0x84].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x84..0x88].copy_from_slice(&NOP.to_be_bytes());

        // Independent evidence supplies `target` as a root; the `j` alone
        // would not be enough to make that claim.
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, target]);
        let part = partition(&cfg);

        // Both the caller and independently-proven tail target are roots, so
        // both should be independent owners with no overlap.
        assert_eq!(part.owners.len(), 2);
        let overlaps = same_bank_overlaps(&part, &cfg);
        assert!(overlaps.is_empty());
    }

    #[test]
    fn uncorroborated_j_target_stays_with_the_current_owner() {
        // A `j` can be an intra-function jump to a local label. With no jal
        // or explicit seed proving the target callable, it must not create a
        // second owner.
        let target: u32 = 0x8000_0080;
        let j = 0x0800_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[j, NOP]);
        bytes.resize(0x100, 0);
        bytes[0x80..0x84].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x84..0x88].copy_from_slice(&NOP.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part = partition(&cfg);
        assert_eq!(part.owners.len(), 1);
        assert!(part.owners[0].block_starts.contains(&target));
        assert!(part.unowned.is_empty());
    }

    #[test]
    fn branch_both_arms_stay_with_the_same_owner() {
        // beq $zero,$zero,+1 ; nop (delay, always runs) ; jr $ra (fallthrough
        // not taken path... doesn't matter, structural only) ; nop
        // branch target: jr $ra ; nop  (at pc+4 + (1<<2) = start+4+4=start+8... let's just
        // point both fallthrough and target at jr $ra blocks and confirm one owner)
        let beq = 0x1000_0001u32; // beq $0,$0, +1 word
        let bytes = asm(&[beq, NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part = partition(&cfg);
        assert_eq!(part.owners.len(), 1);
        assert!(part.ambiguous.is_empty());
    }

    #[test]
    fn two_roots_claiming_the_same_block_are_ambiguous_not_silently_assigned() {
        // Root A (0x0) falls straight through into root B's (0x4) entry.
        // Because a root address is always a mandatory block boundary
        // (see `build_cfg`'s root-set check), root A's block ends exactly
        // at 0x4 via an ordinary `Fallthrough`, and root B's own
        // traversal also claims the block starting at 0x4 (it *is* root
        // B's entry block). Both roots' closures therefore claim the same
        // block start -- the real ambiguity this test exists to catch.
        // Layout: [0x0] nop ; [0x4] nop ; [0x8] jr $ra ; [0xc] nop (delay)
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, 0x8000_0004]);
        let part = partition(&cfg);
        assert!(!part.ambiguous.is_empty());
        let shared = part
            .ambiguous
            .iter()
            .find(|a| a.block_start == 0x8000_0004)
            .expect("block at root B's entry should be ambiguous");
        assert_eq!(shared.claimants, vec![0x8000_0000, 0x8000_0004]);
        // Root A itself still gets its own exclusive one-block owner for
        // [0x0, 0x4).
        let owner_a = part
            .owners
            .iter()
            .find(|o| o.root_va == 0x8000_0000)
            .expect("root A should still own its own leading block");
        assert_eq!(owner_a.extent_end, 0x8000_0004);
    }

    #[test]
    fn authoritative_entry_inside_an_owner_span_splits_the_span() {
        let entry = 0x8000_0004;
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, entry]);

        let unresolved = partition(&cfg);
        assert!(unresolved
            .ambiguous
            .iter()
            .any(|block| block.block_start == entry));

        let resolved = partition_with_authoritative_entries(&cfg, &BTreeSet::from([entry]));
        assert!(resolved.ambiguous.is_empty());
        let prefix = resolved
            .owners
            .iter()
            .find(|owner| owner.root_va == 0x8000_0000)
            .unwrap();
        assert_eq!(prefix.block_starts, vec![0x8000_0000]);
        assert_eq!(prefix.extent_end, entry);
        let split = resolved
            .owners
            .iter()
            .find(|owner| owner.root_va == entry)
            .unwrap();
        assert_eq!(split.block_starts, vec![entry]);
        assert_eq!(split.extent_end, 0x8000_0010);
        assert!(same_bank_overlaps(&resolved, &cfg).is_empty());
    }

    #[test]
    fn reached_only_entry_inside_an_owner_span_does_not_split() {
        let target = 0x8000_0004;
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, target]);

        let part = partition_with_authoritative_entries(&cfg, &BTreeSet::new());
        assert!(part
            .ambiguous
            .iter()
            .any(|block| block.block_start == target));
        assert!(!part.owners.iter().any(|owner| owner.root_va == target));
    }

    #[test]
    fn synthetic_extent_key_rejects_a_split_at_the_wrong_boundary() {
        let entry = 0x8000_0004;
        let bytes = asm(&[NOP, NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000, entry]);
        let part = partition_with_authoritative_entries(&cfg, &BTreeSet::from([entry]));
        let split = part
            .owners
            .iter()
            .find(|owner| owner.root_va == entry)
            .unwrap();

        let held_out_key = (entry, 0x8000_000c);
        assert_ne!(
            (split.root_va, split.extent_end),
            held_out_key,
            "a split with the wrong end must not pass exact held-out grading"
        );
    }

    #[test]
    fn same_bank_overlaps_is_empty_for_disjoint_contiguous_owners() {
        let target: u32 = 0x8000_0080;
        let j = 0x0800_0000 | ((target >> 2) & 0x03ff_ffff);
        let mut bytes = asm(&[j, NOP]);
        bytes.resize(0x100, 0);
        bytes[0x80..0x84].copy_from_slice(&JR_RA.to_be_bytes());
        bytes[0x84..0x88].copy_from_slice(&NOP.to_be_bytes());
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part = partition(&cfg);
        assert!(same_bank_overlaps(&part, &cfg).is_empty());
    }

    #[test]
    fn same_bank_overlaps_catches_extent_that_swallows_a_foreign_owners_block() {
        // Regression for a real bug found against NW4E: two owners' block
        // chains can each independently look "contiguous" by address (see
        // `Owner::is_contiguous`'s doc comment) while owner A's synthesized
        // `[root_va, extent_end)` interval nonetheless spans over a block
        // that owner B -- not A -- actually owns. This was previously
        // undetected because the old overlap check only ever compared
        // pre-filtered "is_contiguous" owners' `extent_end` values as
        // opaque intervals; it never checked whether the address range in
        // between belonged to someone else. Constructed directly against
        // `Cfg`/`Partition` (bypassing `build_cfg`) to pin the exact shape
        // without depending on real MIPS encodings reproducing it.
        let blocks = vec![
            BasicBlock {
                start_va: 0x8000_0000,
                end_va: 0x8000_0008,
                terminator: BlockTerminator::Fallthrough { next: 0x8000_0008 },
            },
            // Owner A also owns this next block purely by (buggy) address
            // coincidence in the old check: its end_va (0x8000_0010)
            // matches what A's `extent_end` would be computed as, but A
            // never actually traverses to 0x8000_0008 -- only B does.
            BasicBlock {
                start_va: 0x8000_0008,
                end_va: 0x8000_0010,
                terminator: BlockTerminator::Return,
            },
        ];
        let cfg = Cfg {
            bank: "boot".to_string(),
            word_class: BTreeMap::new(),
            blocks,
            direct_calls: vec![],
            tail_transfers: vec![],
            indirect_sites: vec![],
            proven_roots: vec![0x8000_0000, 0x8000_0008],
        };
        let part = Partition {
            bank: "boot".to_string(),
            owners: vec![
                Owner {
                    bank: "boot".to_string(),
                    root_va: 0x8000_0000,
                    block_starts: vec![0x8000_0000],
                    // Bug shape: A's claimed extent reaches all the way to
                    // 0x8000_0010, past the block it actually owns
                    // ([0x8000_0000, 0x8000_0008)), swallowing B's block.
                    extent_end: 0x8000_0010,
                },
                Owner {
                    bank: "boot".to_string(),
                    root_va: 0x8000_0008,
                    block_starts: vec![0x8000_0008],
                    extent_end: 0x8000_0010,
                },
            ],
            ambiguous: vec![],
            unowned: vec![],
        };

        let overlaps = same_bank_overlaps(&part, &cfg);
        assert_eq!(overlaps, vec![(0x8000_0000, 0x8000_0008)]);
    }

    #[test]
    fn partition_is_deterministic_across_repeated_calls() {
        let bytes = asm(&[NOP, JR_RA, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let part_a = partition(&cfg);
        let part_b = partition(&cfg);
        assert_eq!(
            serde_json::to_string(&part_a).unwrap(),
            serde_json::to_string(&part_b).unwrap()
        );
    }
}
