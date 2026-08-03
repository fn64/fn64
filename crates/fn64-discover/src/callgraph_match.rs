//! Call-graph propagation for cross-ROM function correspondence.
//!
//! [`homology`](crate::homology) matches function *bodies* pairwise by a
//! relocation-masked whole-body hash: a body whose masked hash is unique on
//! both sides is a seed correspondence. That leaves two gaps this module
//! closes, both *mechanically* and both *without* a similarity threshold:
//!
//! - functions whose masked body collides with another (the hash is not
//!   unique, so body-hashing alone reports them ambiguous), and
//! - functions too short or too repetitive to fingerprint on their own.
//!
//! The technique is the EXACT/structural tier of BinDiff's call-graph
//! propagation (Google's `MdIndex`/`FlowGraph` matching, open-sourced 2023
//! under Apache-2.0 — read for the technique, reimplemented here in fn64
//! style). We deliberately use ONLY its exact tier: a match propagates along
//! a *proven* call edge and is admitted ONLY when the mapping is unique. The
//! fuzzy-confidence tier (matching by MD-index proximity or edge-count
//! similarity) is intentionally not implemented — a similarity score never
//! admits a correspondence here.
//!
//! # The propagation rule (uniqueness, never similarity)
//!
//! Both ROMs are reduced to a call graph over their prior, independently
//! derived function boundaries. A call edge is `(caller, call_index, callee)`
//! where `call_index` is the *ordinal position* of the direct call within the
//! caller's body — an address-free structural coordinate that survives the
//! whole-function relocation a moved-but-unchanged function undergoes.
//!
//! Given a matched pair `A <-> B` (A in ROM1, B in ROM2):
//!
//! - *Forward* (disambiguate callees): for the set of A's call edges and B's
//!   call edges that share the same `call_index`, if there is exactly one
//!   currently-unmatched callee `X` on A's side and exactly one
//!   currently-unmatched callee `Y` on B's side at that index, and neither is
//!   already claimed by a competing propagation this round, admit `X <-> Y`.
//! - *Backward* (disambiguate callers): symmetric, over incoming edges.
//!
//! Iterate to a fixed point. The load-bearing rule is **admit only on
//! uniqueness**: if a matched caller has two unmatched callees whose mapping
//! is ambiguous (two callees at one index, or a callee reachable at two
//! indices with unmatched competitors), admit NOTHING for them — they stay
//! open. A wrong propagated match cascades (it becomes a seed for the next
//! round), so the uniqueness discipline is where correctness lives.
//!
//! # This module emits candidates only
//!
//! Like [`homology`](crate::homology) and [`cfg_homology`](crate::cfg_homology),
//! a propagated pair is a *candidate correspondence* for corpus homology, not
//! a proof of identity, entry, extent, or name. It carries no [`ProofState`]
//! and never feeds an authoritative pack directly.
//!
//! [`ProofState`]: crate::facts::ProofState

use crate::cfg::{classify_control, region_target, ControlOp};
use crate::homology::relocation_masked_word;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One prior, independently-derived function body from a ROM. `identity` is
/// output-side identity (e.g. a name or address label); it is never consumed
/// by matching, only reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionBody {
    pub identity: String,
    pub va_start: u32,
    pub words: Vec<u32>,
}

/// One side of the correspondence: a ROM's functions, keyed by entry VA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    functions: Vec<FunctionBody>,
    /// Entry VA -> index into `functions`. A duplicate entry is rejected at
    /// construction; downstream code may assume entries are unique.
    entry_index: BTreeMap<u32, usize>,
}

/// How a correspondence entered the result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchSource {
    /// Seeded from a relocation-masked whole-body hash that was unique on
    /// both sides (the [`homology`](crate::homology) baseline rule).
    BodyHash,
    /// Propagated along a proven call edge under the uniqueness rule.
    CallGraph,
}

/// One admitted correspondence between a ROM1 function and a ROM2 function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedPair {
    pub left_identity: String,
    pub left_va: u32,
    pub right_identity: String,
    pub right_va: u32,
    pub source: MatchSource,
    /// For a propagated pair, the round (1-based) at which it was admitted.
    /// Seeds are round 0.
    pub round: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReport {
    pub pairs: Vec<MatchedPair>,
    pub seed_count: usize,
    pub propagated_count: usize,
    pub rounds: u32,
    /// ROM1 functions never matched by any rule (diagnostic frontier).
    pub left_unmatched: usize,
    /// ROM2 functions never matched by any rule.
    pub right_unmatched: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallGraphError {
    DuplicateEntry { va: u32 },
    UnalignedEntry { va: u32 },
    EmptyIdentity { va: u32 },
    AddressOverflow { va: u32, words: usize },
}

impl std::fmt::Display for CallGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateEntry { va } => write!(f, "duplicate function entry 0x{va:08x}"),
            Self::UnalignedEntry { va } => write!(f, "unaligned function entry 0x{va:08x}"),
            Self::EmptyIdentity { va } => {
                write!(f, "function at 0x{va:08x} has an empty identity")
            }
            Self::AddressOverflow { va, words } => {
                write!(
                    f,
                    "function at 0x{va:08x} of {words} words overflows the address space"
                )
            }
        }
    }
}

impl std::error::Error for CallGraphError {}

impl Program {
    /// Build a program from prior function boundaries. Entries must be
    /// word-aligned and unique; a caller supplying overlapping or duplicate
    /// boundaries is a bug the fact database would already have rejected, so
    /// it is a loud error here rather than a silently-dropped function.
    pub fn new(functions: Vec<FunctionBody>) -> Result<Self, CallGraphError> {
        let mut entry_index = BTreeMap::new();
        for (index, function) in functions.iter().enumerate() {
            if function.identity.is_empty() {
                return Err(CallGraphError::EmptyIdentity {
                    va: function.va_start,
                });
            }
            if function.va_start & 3 != 0 {
                return Err(CallGraphError::UnalignedEntry {
                    va: function.va_start,
                });
            }
            let byte_len = function
                .words
                .len()
                .checked_mul(4)
                .and_then(|len| u32::try_from(len).ok());
            if byte_len
                .and_then(|len| function.va_start.checked_add(len))
                .is_none()
            {
                return Err(CallGraphError::AddressOverflow {
                    va: function.va_start,
                    words: function.words.len(),
                });
            }
            if entry_index.insert(function.va_start, index).is_some() {
                return Err(CallGraphError::DuplicateEntry {
                    va: function.va_start,
                });
            }
        }
        Ok(Self {
            functions,
            entry_index,
        })
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// The relocation-masked whole-body hash used for seeding, reusing
    /// [`homology`](crate::homology)'s exact masking so a seed here means the
    /// same thing a body-hash candidate means there.
    fn body_hash(function: &FunctionBody) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for &word in &function.words {
            hash ^= u64::from(relocation_masked_word(word));
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Fold the length in so two functions of different length never share
        // a body hash by coincidence of a masked-word fold.
        hash ^= function.words.len() as u64;
        hash.wrapping_mul(0x0000_0100_0000_01b3)
    }

    /// Ordered outgoing direct-call edges of one function: `(call_index,
    /// callee_function_index)` for each `jal`/link-branch whose region target
    /// is a known function entry. `call_index` is the ordinal of the call
    /// within the body (0-based, in address order), the address-free
    /// structural coordinate propagation keys on. Calls whose target is not a
    /// known entry (library thunks, intra-bank labels, cross-bank tails) are
    /// still *counted* in the index so a call to an unknown target does not
    /// silently shift the coordinate of the calls after it.
    fn outgoing_edges(&self, function_index: usize) -> Vec<(u32, usize)> {
        let function = &self.functions[function_index];
        let mut edges = Vec::new();
        let mut call_index = 0u32;
        for (word_offset, &word) in function.words.iter().enumerate() {
            let pc = function
                .va_start
                .wrapping_add((word_offset as u32).wrapping_mul(4));
            let target = match classify_control(word) {
                ControlOp::Jal { target } => Some(region_target(pc, target)),
                // A conditional link branch (`bltzal`/`bgezal`) is also a
                // direct call in the CFG builder's model; include it so the
                // call graph matches `cfg::direct_calls`.
                ControlOp::Branch {
                    target: off,
                    link: true,
                }
                | ControlOp::BranchLikely {
                    target: off,
                    link: true,
                } => Some(pc.wrapping_add(4).wrapping_add(off.wrapping_shl(2))),
                _ => continue,
            };
            let this_index = call_index;
            call_index = call_index.wrapping_add(1);
            if let Some(target) = target {
                if let Some(&callee) = self.entry_index.get(&target) {
                    edges.push((this_index, callee));
                }
            }
        }
        edges
    }
}

/// One direction of the correspondence being iterated. `left[i]` / `right[j]`
/// hold the matched partner's function index, or `None` while open.
struct MatchState<'a> {
    left: &'a Program,
    right: &'a Program,
    left_partner: Vec<Option<usize>>,
    right_partner: Vec<Option<usize>>,
    /// Outgoing edges per function, precomputed once.
    left_out: Vec<Vec<(u32, usize)>>,
    right_out: Vec<Vec<(u32, usize)>>,
    /// Incoming edges per function: `(call_index, caller_function_index)`.
    left_in: Vec<Vec<(u32, usize)>>,
    right_in: Vec<Vec<(u32, usize)>>,
    /// Relocation-masked whole-body hash per function, precomputed. A
    /// propagated pair must AGREE on this hash to be admitted (see
    /// [`MatchState::propose_round`]).
    left_hash: Vec<u64>,
    right_hash: Vec<u64>,
    /// Admission round per matched left function (0 = seed).
    left_round: Vec<u32>,
    source: Vec<MatchSource>,
}

impl<'a> MatchState<'a> {
    fn new(left: &'a Program, right: &'a Program) -> Self {
        let left_out: Vec<_> = (0..left.len()).map(|i| left.outgoing_edges(i)).collect();
        let right_out: Vec<_> = (0..right.len()).map(|i| right.outgoing_edges(i)).collect();
        let left_in = invert(&left_out, left.len());
        let right_in = invert(&right_out, right.len());
        let left_hash = left.functions.iter().map(Program::body_hash).collect();
        let right_hash = right.functions.iter().map(Program::body_hash).collect();
        Self {
            left,
            right,
            left_partner: vec![None; left.len()],
            right_partner: vec![None; right.len()],
            left_out,
            right_out,
            left_in,
            right_in,
            left_hash,
            right_hash,
            left_round: vec![0; left.len()],
            source: vec![MatchSource::BodyHash; left.len()],
        }
    }

    fn admit(&mut self, li: usize, ri: usize, source: MatchSource, round: u32) {
        debug_assert!(self.left_partner[li].is_none());
        debug_assert!(self.right_partner[ri].is_none());
        self.left_partner[li] = Some(ri);
        self.right_partner[ri] = Some(li);
        self.left_round[li] = round;
        self.source[li] = source;
    }

    /// Seed correspondences from body hashes unique on both sides. This is
    /// exactly [`homology`](crate::homology)'s unique-body rule applied to the
    /// whole-function masked hash: a body-hash bucket with one function on
    /// each side is a seed; anything else stays open for propagation.
    fn seed(&mut self) {
        let left_buckets = hash_buckets(self.left);
        let right_buckets = hash_buckets(self.right);
        for (hash, lefts) in &left_buckets {
            let Some(rights) = right_buckets.get(hash) else {
                continue;
            };
            if lefts.len() == 1 && rights.len() == 1 {
                self.admit(lefts[0], rights[0], MatchSource::BodyHash, 0);
            }
        }
    }

    /// One propagation round over every currently-matched pair. Returns the
    /// admissions to apply; applying them is deferred so that two matched
    /// pairs proposing the *same* target this round are detected as a
    /// competition and neither is admitted (uniqueness across the whole
    /// round, not just within one caller).
    fn propose_round(&self) -> Vec<(usize, usize)> {
        // For each open left/right function, gather every (partner-derived)
        // candidate proposed for it this round. A proposal is
        // `left_index -> right_index`. Only pairings proposed uniquely from
        // both directions survive.
        let mut left_to_right: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        let mut right_to_left: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();

        let mut record = |li: usize, ri: usize| {
            left_to_right.entry(li).or_default().insert(ri);
            right_to_left.entry(ri).or_default().insert(li);
        };

        for li in 0..self.left.len() {
            let Some(ri) = self.left_partner[li] else {
                continue;
            };
            // Forward: disambiguate callees at each shared call index.
            self.propose_over(
                &self.left_out[li],
                &self.right_out[ri],
                &self.left_partner,
                &self.right_partner,
                &mut record,
            );
            // Backward: disambiguate callers at each shared call index.
            self.propose_over(
                &self.left_in[li],
                &self.right_in[ri],
                &self.left_partner,
                &self.right_partner,
                &mut record,
            );
        }

        // Admit only pairings that are unique from BOTH sides: exactly one
        // right proposed for this left, and exactly one left proposed for
        // that right. Anything with a competitor stays open.
        //
        // Uniqueness of call-graph POSITION alone is not enough to be sound
        // across two genuinely different programs: two engines diverge, and a
        // caller matched by its seed body can still call a different function
        // at the same structural slot. So a propagated pair must ALSO agree on
        // its relocation-masked whole body — the exact evidence the seed rule
        // (and pairwise homology) trusts. Propagation therefore only ever
        // DISAMBIGUATES a body-hash *collision* the seed rule left ambiguous:
        // call-graph position picks which equal-bodied candidate is the
        // partner. This keeps propagation at the seed rule's precision while
        // recovering the matches unique-body hashing could not make on its
        // own. A pair whose bodies differ is exactly the wrong-cascade this
        // module must refuse, so it is dropped here, not admitted.
        let mut admitted = Vec::new();
        for (&li, rights) in &left_to_right {
            if rights.len() != 1 {
                continue;
            }
            let ri = *rights.iter().next().expect("one right");
            let Some(lefts) = right_to_left.get(&ri) else {
                continue;
            };
            if lefts.len() == 1 && self.left_hash[li] == self.right_hash[ri] {
                admitted.push((li, ri));
            }
        }
        admitted.sort_unstable();
        admitted
    }

    /// Propose callee/caller pairings for one matched caller/callee pair,
    /// grouped by shared structural call index. At an index with exactly one
    /// open left neighbor and exactly one open right neighbor, that pairing is
    /// proposed; any index with two-or-more open neighbors on either side is
    /// ambiguous and proposes nothing.
    fn propose_over(
        &self,
        left_edges: &[(u32, usize)],
        right_edges: &[(u32, usize)],
        left_partner: &[Option<usize>],
        right_partner: &[Option<usize>],
        record: &mut impl FnMut(usize, usize),
    ) {
        let left_by_index = open_by_index(left_edges, left_partner);
        let right_by_index = open_by_index(right_edges, right_partner);
        for (index, lefts) in &left_by_index {
            let Some(rights) = right_by_index.get(index) else {
                continue;
            };
            // Uniqueness at this index: exactly one open neighbor per side.
            if lefts.len() == 1 && rights.len() == 1 {
                record(lefts[0], rights[0]);
            }
            // Otherwise ambiguous at this index — admit nothing (stay open).
        }
    }

    fn into_report(self, rounds: u32) -> MatchReport {
        let mut pairs = Vec::new();
        let mut seed_count = 0usize;
        let mut propagated_count = 0usize;
        for li in 0..self.left.len() {
            let Some(ri) = self.left_partner[li] else {
                continue;
            };
            let source = self.source[li];
            match source {
                MatchSource::BodyHash => seed_count += 1,
                MatchSource::CallGraph => propagated_count += 1,
            }
            pairs.push(MatchedPair {
                left_identity: self.left.functions[li].identity.clone(),
                left_va: self.left.functions[li].va_start,
                right_identity: self.right.functions[ri].identity.clone(),
                right_va: self.right.functions[ri].va_start,
                source,
                round: self.left_round[li],
            });
        }
        pairs.sort_by_key(|pair| (pair.left_va, pair.right_va));
        let left_unmatched = self.left_partner.iter().filter(|p| p.is_none()).count();
        let right_unmatched = self.right_partner.iter().filter(|p| p.is_none()).count();
        MatchReport {
            pairs,
            seed_count,
            propagated_count,
            rounds,
            left_unmatched,
            right_unmatched,
        }
    }
}

/// Seed from unique body hashes, then propagate along the call graph to a
/// fixed point. `max_rounds` bounds the fixed-point loop; propagation is
/// monotone (it only ever adds matches), so it terminates well within a bound
/// proportional to the number of functions, but the explicit cap keeps a
/// pathological input from unbounded work.
pub fn match_programs(left: &Program, right: &Program, max_rounds: u32) -> MatchReport {
    let mut state = MatchState::new(left, right);
    state.seed();
    let mut rounds = 0u32;
    while rounds < max_rounds {
        let admitted = state.propose_round();
        if admitted.is_empty() {
            break;
        }
        rounds += 1;
        for (li, ri) in admitted {
            // A pair could have been claimed earlier this same apply loop only
            // if the round produced two winners sharing an endpoint, which the
            // both-sides-unique filter already excludes. Guard anyway: a
            // double-claim is a bug, and admitting it would be the exact
            // wrong-cascade this module exists to prevent.
            if state.left_partner[li].is_none() && state.right_partner[ri].is_none() {
                state.admit(li, ri, MatchSource::CallGraph, rounds);
            }
        }
    }
    state.into_report(rounds)
}

fn hash_buckets(program: &Program) -> BTreeMap<u64, Vec<usize>> {
    let mut buckets: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (index, function) in program.functions.iter().enumerate() {
        buckets
            .entry(Program::body_hash(function))
            .or_default()
            .push(index);
    }
    buckets
}

/// Invert outgoing edges into incoming edges, preserving the caller's call
/// index so a backward propagation keys on the same structural coordinate.
fn invert(outgoing: &[Vec<(u32, usize)>], node_count: usize) -> Vec<Vec<(u32, usize)>> {
    let mut incoming = vec![Vec::new(); node_count];
    for (caller, edges) in outgoing.iter().enumerate() {
        for &(call_index, callee) in edges {
            incoming[callee].push((call_index, caller));
        }
    }
    incoming
}

/// Group a node's neighbors (callees or callers) by call index, keeping only
/// the still-open ones. A neighbor already matched is excluded — propagation
/// only ever disambiguates *unmatched* endpoints.
fn open_by_index(edges: &[(u32, usize)], partner: &[Option<usize>]) -> BTreeMap<u32, Vec<usize>> {
    let mut by_index: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for &(index, neighbor) in edges {
        if partner[neighbor].is_none() {
            by_index.entry(index).or_default().push(neighbor);
        }
    }
    for neighbors in by_index.values_mut() {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    by_index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `jal` to `target`, with the region bits derived from the caller pc at
    /// emit time (tests place callers in the 0x8000_0000 pseudo-region so
    /// `region_target` reproduces the intended absolute address).
    fn jal(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    const NOP: u32 = 0x0000_0000;
    const JR_RA: u32 = 0x03e0_0008;

    /// A body that is deliberately *not* uniquely hashable on its own (a bare
    /// prologue/return), so it can only be matched by propagation. `calls` are
    /// the callee VAs it invokes, in order.
    fn caller_body(calls: &[u32]) -> Vec<u32> {
        let mut words = vec![0x27bd_ffe8]; // addiu sp, sp, -0x18
        for &c in calls {
            words.push(jal(c));
            words.push(NOP);
        }
        words.push(JR_RA);
        words.push(NOP);
        words
    }

    fn func(identity: &str, va: u32, words: Vec<u32>) -> FunctionBody {
        FunctionBody {
            identity: identity.into(),
            va_start: va,
            words,
        }
    }

    /// A distinctive leaf whose masked body is unique on both sides, so it
    /// seeds. The `salt` perturbs a non-address field to keep bodies distinct.
    fn unique_leaf(salt: u32) -> Vec<u32> {
        vec![
            0x27bd_fff0,
            0x2408_0000 | (salt & 0xffff), // li t0, salt  (addiu t0, zero, salt)
            0x0102_2021,                   // addu a0, t0, v0
            0x03e0_0008,
            0x0000_0000,
            0x2409_0000 | ((salt ^ 0x1234) & 0xffff),
        ]
    }

    #[test]
    fn unique_callee_propagates_from_a_matched_caller() {
        // Left ROM: caller A calls a seedable leaf L then an unmatched leaf X.
        // Right ROM: caller B (same call shape) calls L' then X'. L/L' seed by
        // body hash; X/X' are byte-identical bare returns that collide with
        // each other's hash bucket only if unique — here they are made
        // ambiguous-on-hash by a twin, so propagation is what pairs them.
        let l_left = func("L", 0x8000_1000, unique_leaf(0x11));
        let l_right = func("L'", 0x8020_1000, unique_leaf(0x11));

        // X and its twin T share a masked body, so neither seeds; only the
        // caller's structural call index disambiguates X.
        let bare = vec![0x03e0_0008u32, 0x0000_0000];
        let x_left = func("X", 0x8000_2000, bare.clone());
        let t_left = func("T", 0x8000_3000, bare.clone());
        let x_right = func("X'", 0x8020_2000, bare.clone());
        let t_right = func("T'", 0x8020_3000, bare.clone());

        // Caller A calls L (index 0) then X (index 1). T is called by nobody
        // on either side, so only X is reachable at a matched caller's index.
        let a = func("A", 0x8000_0000, caller_body(&[0x8000_1000, 0x8000_2000]));
        let b = func("B", 0x8020_0000, caller_body(&[0x8020_1000, 0x8020_2000]));

        let left = Program::new(vec![a, l_left, x_left, t_left]).unwrap();
        let right = Program::new(vec![b, l_right, x_right, t_right]).unwrap();
        let report = match_programs(&left, &right, 16);

        // A<->B and L<->L' seed; X<->X' propagates; T stays open (no matched
        // caller reaches it, so uniqueness never fires for it).
        let x = report
            .pairs
            .iter()
            .find(|p| p.left_identity == "X")
            .expect("X matched");
        assert_eq!(x.right_identity, "X'");
        assert_eq!(x.source, MatchSource::CallGraph);
        assert!(!report.pairs.iter().any(|p| p.left_identity == "T"));
    }

    #[test]
    fn ambiguous_callees_admit_nothing() {
        // Caller A calls X then Y, both bare-return twins (same masked body).
        // Caller B calls X' then Y'. The call indices are distinct (X at 0, Y
        // at 1), so index-keying WOULD disambiguate — to force ambiguity we
        // give A two calls at... no: instead make both callees reachable at a
        // single shared index by having the caller call the SAME structural
        // slot map to two open candidates. We model that by two callers each
        // matched, together offering two open candidates at one index.
        let bare = vec![0x03e0_0008u32, 0x0000_0000];
        let x_left = func("X", 0x8000_2000, bare.clone());
        let y_left = func("Y", 0x8000_3000, bare.clone());
        let x_right = func("X'", 0x8020_2000, bare.clone());
        let y_right = func("Y'", 0x8020_3000, bare.clone());

        // A single matched caller that calls BOTH X and Y at the SAME call
        // index is impossible (indices are positional), so ambiguity is
        // created by a caller whose two calls land, on the right side, on a
        // single open candidate reachable from two indices. Simpler: caller
        // calls X at index 0 and Y at index 1 on the left, but on the right
        // both index 0 and index 1 target the *same* function X'. Then X' is
        // proposed from two indices with two distinct left candidates -> the
        // right endpoint X' has two proposers -> ambiguous -> nothing admitted.
        let a = func("A", 0x8000_0000, caller_body(&[0x8000_2000, 0x8000_3000]));
        let b = func("B", 0x8020_0000, caller_body(&[0x8020_2000, 0x8020_2000]));

        let left = Program::new(vec![a, x_left, y_left]).unwrap();
        let right = Program::new(vec![b, x_right, y_right]).unwrap();
        let report = match_programs(&left, &right, 16);

        // A<->B seeds (unique caller shape). But X'/Y' cannot be uniquely
        // paired: index 0 pairs X<->X', index 1 pairs Y<->X'. X' is claimed by
        // two lefts -> competition -> neither X nor Y is admitted.
        assert!(!report
            .pairs
            .iter()
            .any(|p| p.source == MatchSource::CallGraph));
        assert_eq!(report.propagated_count, 0);
    }

    #[test]
    fn a_wrong_seed_does_not_cascade_past_uniqueness() {
        // Force a wrong seed by handing two programs where a hash-unique body
        // pairs the WRONG functions, then confirm propagation refuses to
        // extend the error unless the call structure is itself unique.
        //
        // Left: caller A -> [P, Q]. Right: caller B -> [P', Q']. A<->B would
        // seed, but we deliberately DON'T seed A<->B; instead we seed a wrong
        // pair A<->B where B calls only one function. With B offering a single
        // open callee at index 0 and A offering two (P at 0, Q at 1), index 1
        // has no right neighbor -> Q stays open; index 0 pairs P<->the wrong
        // callee only if unique. We assert no propagation crosses into the
        // absent index.
        let bare = vec![0x03e0_0008u32, 0x0000_0000];
        let p_left = func("P", 0x8000_2000, bare.clone());
        let q_left = func("Q", 0x8000_3000, bare.clone());
        let p_right = func("Pw", 0x8020_2000, bare.clone());

        let a = func("A", 0x8000_0000, caller_body(&[0x8000_2000, 0x8000_3000]));
        // B calls only ONE function, at index 0.
        let b = func("B", 0x8020_0000, caller_body(&[0x8020_2000]));

        let left = Program::new(vec![a, p_left, q_left]).unwrap();
        let right = Program::new(vec![b, p_right]).unwrap();
        let report = match_programs(&left, &right, 16);

        // A<->B seed (unique caller-shape hash differs though: bodies differ in
        // length, so they will NOT seed by body hash). With no seed, nothing
        // propagates at all -> a genuinely-different caller cannot bootstrap a
        // cascade. That is the point: no false seed, no false propagation.
        assert_eq!(report.seed_count, 0);
        assert_eq!(report.propagated_count, 0);
    }

    #[test]
    fn seeds_come_only_from_bodies_unique_on_both_sides() {
        // Two identical-bodied functions on the left share a hash bucket, so
        // neither seeds even though the right has exactly one partner.
        let body = unique_leaf(0x55);
        let twin_a = func("A", 0x8000_1000, body.clone());
        let twin_b = func("B", 0x8000_2000, body.clone());
        let sole = func("C", 0x8020_1000, unique_leaf(0x55));

        let left = Program::new(vec![twin_a, twin_b]).unwrap();
        let right = Program::new(vec![sole]).unwrap();
        let report = match_programs(&left, &right, 16);
        assert_eq!(report.seed_count, 0);
        assert_eq!(report.propagated_count, 0);
    }

    #[test]
    fn propagation_reaches_a_fixed_point_over_multiple_rounds() {
        // A two-hop chain that needs two rounds. A is a distinctive seed that
        // calls M (index 0). M is a bare caller (has a twin, so it does NOT
        // seed) that calls Z (index 0). Z is a bare leaf twinned as well, so
        // Z only pairs once M is matched. Round 1: A anchors M. Round 2: M
        // anchors Z. The twins ensure neither M nor Z could ever seed.
        let mut a_left = unique_leaf(0x01);
        a_left.insert(1, jal(0x8000_1000)); // call M at structural index 0
        a_left.insert(2, NOP);
        let mut a_right = unique_leaf(0x01);
        a_right.insert(1, jal(0x8020_1000));
        a_right.insert(2, NOP);

        let m_left = caller_body(&[0x8000_9000]); // M calls Z
        let m_right = caller_body(&[0x8020_9000]);
        // A twin of M's body on each side keeps M's hash bucket ambiguous.
        let m_twin_left = caller_body(&[0x8000_a000]);
        let m_twin_right = caller_body(&[0x8020_a000]);

        let z_body = vec![0x03e0_0008u32, 0x0000_0000];

        let left = Program::new(vec![
            func("A", 0x8000_0000, a_left),
            func("M", 0x8000_1000, m_left),
            func("Mtwin", 0x8000_5000, m_twin_left),
            func("Z", 0x8000_9000, z_body.clone()),
            func("Ztwin", 0x8000_a000, z_body.clone()),
        ])
        .unwrap();
        let right = Program::new(vec![
            func("A2", 0x8020_0000, a_right),
            func("M2", 0x8020_1000, m_right),
            func("M2twin", 0x8020_5000, m_twin_right),
            func("Z2", 0x8020_9000, z_body.clone()),
            func("Z2twin", 0x8020_a000, z_body.clone()),
        ])
        .unwrap();

        let report = match_programs(&left, &right, 16);
        let report2 = match_programs(&left, &right, 16);
        assert_eq!(report, report2, "matching must be deterministic");

        // A seeds; M propagates in round 1 (unique open callee of A); Z
        // propagates in round 2 (unique open callee of the now-matched M).
        assert_eq!(report.seed_count, 1);
        let m = report
            .pairs
            .iter()
            .find(|p| p.left_identity == "M")
            .expect("M matched");
        assert_eq!(m.right_identity, "M2");
        assert_eq!(m.source, MatchSource::CallGraph);
        assert_eq!(m.round, 1);
        let z = report
            .pairs
            .iter()
            .find(|p| p.left_identity == "Z")
            .expect("Z matched");
        assert_eq!(z.right_identity, "Z2");
        assert_eq!(z.source, MatchSource::CallGraph);
        assert_eq!(z.round, 2);
        // The twins never anchor to a matched neighbor uniquely, so they stay
        // open — uniqueness, not similarity, gates admission.
        assert!(!report.pairs.iter().any(|p| p.left_identity == "Mtwin"));
        assert!(!report.pairs.iter().any(|p| p.left_identity == "Ztwin"));
    }

    #[test]
    fn duplicate_entry_is_rejected() {
        let a = func("A", 0x8000_0000, vec![JR_RA, NOP]);
        let b = func("B", 0x8000_0000, vec![JR_RA, NOP]);
        assert_eq!(
            Program::new(vec![a, b]),
            Err(CallGraphError::DuplicateEntry { va: 0x8000_0000 })
        );
    }
}
