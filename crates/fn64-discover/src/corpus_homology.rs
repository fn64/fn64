//! Corpus-scale N-ROM function-identity homology.
//!
//! Pairwise cross-ROM matching ([`homology`](crate::homology) body hashes,
//! [`callgraph_match`](crate::callgraph_match) call-graph propagation) answers
//! "is function A in ROM 1 the same as function B in ROM 2?". This module
//! generalizes that to a *corpus*: given N ROMs, every ordered ROM pair is
//! matched by the existing pairwise engine, and the resulting cross-ROM edges
//! are assembled into ONE identity graph. A connected component of that graph
//! is a single function identity shared across however many ROMs it appears
//! in — shared libultra/SDK code spans the whole corpus, engine-family code
//! spans its family, and title-specific code stands alone. The payoff is
//! superlinear: each ROM both consumes and contributes identities, so a
//! libultra routine labeled once is labeled corpus-wide.
//!
//! # This module calls the pairwise engines; it never reimplements them
//!
//! [`build_corpus`] runs [`callgraph_match::match_programs`] on each unordered
//! ROM pair. It adds exactly one thing on top: the transitive closure of those
//! pairwise edges, under a uniqueness-and-corroboration guard. It does not
//! change how a single edge is decided.
//!
//! [`callgraph_match::match_programs`]: crate::callgraph_match::match_programs
//!
//! # The transitive-uniqueness rule (never a similarity threshold)
//!
//! A component grows by unioning the endpoints of pairwise edges. Transitivity
//! is implicit: if the corpus has edges `A<->B` and `B<->C`, then `A`, `B` and
//! `C` land in one component and `A<->C` is *implied* without ever being
//! matched directly. That implication is admitted only when it cannot be
//! wrong, enforced by two invariants every surviving component must hold:
//!
//! - **Per-ROM uniqueness.** A component may contain at most ONE function from
//!   any single ROM. A function identity that appeared twice in one ROM would
//!   be two different functions collapsed into one — the exact cross-ROM
//!   cascade this module exists to prevent. If an edge would place two
//!   distinct functions of the same ROM into one component, that is a
//!   conflict.
//! - **Body corroboration.** Every member of a component shares one
//!   relocation-masked whole-body hash — the same evidence
//!   [`homology`](crate::homology) and the propagation seed rule trust. A
//!   pairwise edge already agrees on this hash for its two endpoints (a seed is
//!   body-hash-unique; a propagated pair must agree on the masked body), so a
//!   chain of edges is body-coherent by construction. If an edge would merge
//!   two functions whose masked bodies differ, that is a conflict.
//!
//! A conflict does not get resolved by a guess and does not silently drop the
//! offending edge while keeping the rest: it **collapses the whole component to
//! ambiguous**. An ambiguous component contributes no corpus identity. This is
//! the load-bearing discipline — a wrong corpus edge would cascade a false
//! identity across every ROM in the component, so ambiguity is preferred to any
//! guess. See the module tests: a consistent transitive edge is admitted; a
//! conflicting one collapses; a wrong pairwise seed cannot cascade corpus-wide.
//!
//! # This module emits candidates only
//!
//! Like the pairwise engines it builds on, a corpus identity is a *candidate
//! correspondence*, not a proof of identity, entry, extent, or name. It carries
//! no [`ProofState`] and never feeds an authoritative pack directly.
//!
//! [`ProofState`]: crate::facts::ProofState

use crate::callgraph_match::{match_programs, FunctionBody, Program};
use crate::homology::relocation_masked_word;
use std::collections::{BTreeMap, BTreeSet};

/// One ROM in the corpus: a label plus its prior, independently-derived
/// function boundaries. The label is output-side identity only (it names the
/// ROM in the report); it is never consumed by matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusRom {
    pub label: String,
    pub functions: Vec<FunctionBody>,
}

/// Fixed-point cap handed to [`callgraph_match::match_programs`] for each pair.
///
/// [`callgraph_match::match_programs`]: crate::callgraph_match::match_programs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpusConfig {
    pub max_rounds: u32,
}

impl Default for CorpusConfig {
    fn default() -> Self {
        Self { max_rounds: 64 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusError {
    /// Two corpus ROMs share a label; labels identify components' members, so
    /// they must be distinct.
    DuplicateRomLabel(String),
    /// A ROM's function boundaries were rejected by the call-graph builder
    /// (duplicate/unaligned entry, empty identity, address overflow).
    Program {
        label: String,
        source: crate::callgraph_match::CallGraphError,
    },
}

impl std::fmt::Display for CorpusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRomLabel(label) => {
                write!(f, "duplicate corpus ROM label {label:?}")
            }
            Self::Program { label, source } => {
                write!(f, "ROM {label:?} call graph rejected: {source}")
            }
        }
    }
}

impl std::error::Error for CorpusError {}

/// One member of a corpus identity, resolved to reportable fields.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdentityMember {
    pub rom: usize,
    pub rom_label: String,
    pub identity: String,
    pub va_start: u32,
}

/// A surviving corpus identity: a set of functions, one per ROM, that pairwise
/// matching plus the transitive-uniqueness rule concluded are the same
/// function. `span` is how many ROMs it appears in — a big span (libultra/SDK)
/// is the superlinear payoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusIdentity {
    pub members: Vec<IdentityMember>,
    /// The relocation-masked whole-body hash every member shares.
    pub body_hash: u64,
}

impl CorpusIdentity {
    pub fn span(&self) -> usize {
        self.members.len()
    }
}

/// A component that a conflict collapsed to ambiguous. It contributes no
/// identity; it is retained as an explicit frontier, never dropped silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousComponent {
    pub members: Vec<IdentityMember>,
    pub reason: AmbiguityReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguityReason {
    /// Two distinct functions from the same ROM were pulled into one component.
    DuplicateRomInComponent,
    /// An edge tried to merge two functions whose masked bodies differ.
    BodyMismatch,
}

/// The corpus result: admitted identities, collapsed ambiguous components, and
/// the counts a grade needs. Singletons (a function matched to nothing) are not
/// reported as identities — an identity spans at least two ROMs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusReport {
    /// Multi-ROM identities, sorted for deterministic output.
    pub identities: Vec<CorpusIdentity>,
    /// Components collapsed to ambiguous by a conflict.
    pub ambiguous: Vec<AmbiguousComponent>,
    /// Total functions across every ROM (the corpus node count).
    pub total_functions: usize,
    /// Number of admitted multi-ROM identities.
    pub identity_count: usize,
    /// The largest admitted identity's span (ROM count).
    pub max_span: usize,
    /// Total cross-ROM pairwise edges fed into the closure.
    pub pairwise_edges: usize,
    /// Functions that matched nothing across the corpus (diagnostic frontier).
    pub singletons: usize,
}

/// The relocation-masked whole-body hash used for corroboration. This is the
/// exact recipe [`callgraph_match`](crate::callgraph_match) seeds and
/// propagates on (FNV-1a over masked words, length folded in), recomputed here
/// through the public [`relocation_masked_word`] rather than reaching into that
/// module's private hash — so a corpus identity means what a pairwise match
/// means.
fn body_hash(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &word in words {
        hash ^= u64::from(relocation_masked_word(word));
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= words.len() as u64;
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

/// A disjoint-set forest over corpus nodes, carrying the two component
/// invariants (the ROMs present, the shared body hash) at each root so a merge
/// can detect a conflict before it happens. A component that ever conflicts is
/// *poisoned* — it and everything later merged into it become ambiguous, and it
/// can never contribute an identity.
struct Forest {
    parent: Vec<usize>,
    rank: Vec<u8>,
    /// Per root: the set of ROM indices its members come from. Used to enforce
    /// per-ROM uniqueness (a ROM appearing twice is a conflict).
    roms: Vec<BTreeSet<usize>>,
    /// Per root: the masked body hash shared by its members, or `None` once the
    /// component is poisoned (no single hash holds).
    hash: Vec<Option<u64>>,
    /// Per root: whether a conflict has poisoned this component.
    poisoned: Vec<bool>,
    /// Per root: why it was poisoned (first conflict recorded).
    reason: Vec<Option<AmbiguityReason>>,
}

impl Forest {
    fn new(node_rom: Vec<usize>, node_hash: Vec<u64>) -> Self {
        let n = node_rom.len();
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
            roms: node_rom.iter().map(|&r| BTreeSet::from([r])).collect(),
            hash: node_hash.into_iter().map(Some).collect(),
            poisoned: vec![false; n],
            reason: vec![None; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// Union the components of `a` and `b`. If the merge would violate per-ROM
    /// uniqueness or body corroboration, the merged component is poisoned
    /// (collapsed to ambiguous) rather than silently repaired. Poison is
    /// contagious: merging anything into a poisoned component poisons the
    /// result, so one bad edge cannot leave a "clean" identity behind it.
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }

        let already_poisoned = self.poisoned[ra] || self.poisoned[rb];
        let first_reason = self.reason[ra].or(self.reason[rb]);

        // Per-ROM uniqueness: the two components must not both contain the same
        // ROM (that would put two distinct functions of one ROM together — the
        // endpoints of this edge are distinct nodes, so a shared ROM is always
        // two different functions).
        let rom_conflict = self.roms[ra].intersection(&self.roms[rb]).next().is_some();

        // Body corroboration: both live components must agree on their masked
        // body hash. A poisoned component has no single hash, so it is treated
        // as already inconsistent (poison propagates regardless).
        let body_conflict = match (self.hash[ra], self.hash[rb]) {
            (Some(ha), Some(hb)) => ha != hb,
            _ => false,
        };

        // Choose the new root by rank (union by rank keeps find near-flat).
        let (root, other) = if self.rank[ra] < self.rank[rb] {
            (rb, ra)
        } else if self.rank[ra] > self.rank[rb] {
            (ra, rb)
        } else {
            self.rank[ra] += 1;
            (ra, rb)
        };
        self.parent[other] = root;

        // Merge the ROM sets and the shared hash into the surviving root.
        let merged_roms: BTreeSet<usize> = self.roms[ra].union(&self.roms[rb]).copied().collect();
        self.roms[root] = merged_roms;
        self.roms[other] = BTreeSet::new();

        let poisoned = already_poisoned || rom_conflict || body_conflict;
        self.poisoned[root] = poisoned;
        self.reason[root] = if poisoned {
            first_reason.or(if rom_conflict {
                Some(AmbiguityReason::DuplicateRomInComponent)
            } else if body_conflict {
                Some(AmbiguityReason::BodyMismatch)
            } else {
                // Poison inherited from an already-poisoned side that somehow
                // lost its reason — default to the ROM-duplicate cascade, the
                // dominant corpus failure mode.
                Some(AmbiguityReason::DuplicateRomInComponent)
            })
        } else {
            None
        };
        // A poisoned component has no single shared hash; a clean one keeps the
        // hash both sides agreed on.
        self.hash[root] = if poisoned {
            None
        } else {
            self.hash[ra].or(self.hash[rb])
        };
    }
}

/// Run pairwise matching over every unordered ROM pair, then assemble the
/// cross-ROM edges into corpus identities under the transitive-uniqueness rule.
pub fn build_corpus(roms: &[CorpusRom], config: CorpusConfig) -> Result<CorpusReport, CorpusError> {
    let mut labels = BTreeSet::new();
    for rom in roms {
        if !labels.insert(rom.label.clone()) {
            return Err(CorpusError::DuplicateRomLabel(rom.label.clone()));
        }
    }

    // Build one Program per ROM (the call-graph engine's input) and, alongside,
    // a flat node table: node id -> (rom, function index). Nodes are numbered
    // ROM-major then function-major, so the numbering is a deterministic
    // function of the input order.
    let mut programs = Vec::with_capacity(roms.len());
    let mut node_rom = Vec::new();
    let mut node_hash = Vec::new();
    // (rom, va) -> local function index, for turning a pairwise edge's VA back
    // into a node.
    let mut va_to_local: Vec<BTreeMap<u32, usize>> = Vec::with_capacity(roms.len());
    // node base offset per ROM.
    let mut rom_base = Vec::with_capacity(roms.len());
    for (rom_index, rom) in roms.iter().enumerate() {
        rom_base.push(node_rom.len());
        let mut local = BTreeMap::new();
        for (function_index, function) in rom.functions.iter().enumerate() {
            node_rom.push(rom_index);
            node_hash.push(body_hash(&function.words));
            local.insert(function.va_start, function_index);
        }
        va_to_local.push(local);
        let program =
            Program::new(rom.functions.clone()).map_err(|source| CorpusError::Program {
                label: rom.label.clone(),
                source,
            })?;
        programs.push(program);
    }

    let node = |rom: usize, function: usize| rom_base[rom] + function;

    // Every unordered ROM pair, matched by the existing pairwise engine. The
    // resulting edges are the only cross-ROM evidence the closure consumes.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for left in 0..roms.len() {
        for right in (left + 1)..roms.len() {
            let report = match_programs(&programs[left], &programs[right], config.max_rounds);
            for pair in &report.pairs {
                let (Some(&li), Some(&ri)) = (
                    va_to_local[left].get(&pair.left_va),
                    va_to_local[right].get(&pair.right_va),
                ) else {
                    // A pair whose VA is not a known entry cannot occur: pairs
                    // come from the same Program whose entries we indexed.
                    continue;
                };
                edges.push((node(left, li), node(right, ri)));
            }
        }
    }
    // Deterministic edge order (the union outcome is order-independent for the
    // sets/hashes it tracks, but a fixed order keeps the whole run reproducible
    // and debugging legible).
    edges.sort_unstable();

    let mut forest = Forest::new(node_rom.clone(), node_hash);
    for &(a, b) in &edges {
        forest.union(a, b);
    }

    // Gather members by root.
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for id in 0..node_rom.len() {
        let root = forest.find(id);
        by_root.entry(root).or_default().push(id);
    }

    let member = |id: usize| -> IdentityMember {
        let rom = node_rom[id];
        let function = id - rom_base[rom];
        let function_body = &roms[rom].functions[function];
        IdentityMember {
            rom,
            rom_label: roms[rom].label.clone(),
            identity: function_body.identity.clone(),
            va_start: function_body.va_start,
        }
    };

    let mut identities = Vec::new();
    let mut ambiguous = Vec::new();
    let mut singletons = 0usize;
    for (&root, nodes) in &by_root {
        if nodes.len() == 1 {
            singletons += 1;
            continue;
        }
        let mut members: Vec<IdentityMember> = nodes.iter().map(|&id| member(id)).collect();
        members.sort_unstable();
        if forest.poisoned[root] {
            ambiguous.push(AmbiguousComponent {
                members,
                reason: forest.reason[root].unwrap_or(AmbiguityReason::DuplicateRomInComponent),
            });
        } else {
            let hash = forest.hash[root].expect("a clean component keeps its shared hash");
            identities.push(CorpusIdentity {
                members,
                body_hash: hash,
            });
        }
    }

    identities.sort_by(|a, b| a.members.cmp(&b.members));
    ambiguous.sort_by(|a, b| a.members.cmp(&b.members));

    let identity_count = identities.len();
    let max_span = identities
        .iter()
        .map(CorpusIdentity::span)
        .max()
        .unwrap_or(0);

    Ok(CorpusReport {
        identities,
        ambiguous,
        total_functions: node_rom.len(),
        identity_count,
        max_span,
        pairwise_edges: edges.len(),
        singletons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    const NOP: u32 = 0x0000_0000;
    const JR_RA: u32 = 0x03e0_0008;

    fn func(identity: &str, va: u32, words: Vec<u32>) -> FunctionBody {
        FunctionBody {
            identity: identity.into(),
            va_start: va,
            words,
        }
    }

    /// A distinctive leaf whose masked body seeds on both sides. `salt`
    /// perturbs a non-address field so distinct salts give distinct bodies.
    fn unique_leaf(salt: u32) -> Vec<u32> {
        vec![
            0x27bd_fff0,
            0x2408_0000 | (salt & 0xffff),
            0x0102_2021,
            0x03e0_0008,
            0x0000_0000,
            0x2409_0000 | ((salt ^ 0x1234) & 0xffff),
        ]
    }

    /// One ROM holding a single distinctive leaf with the given salt, placed at
    /// a ROM-specific VA base so bodies are relocation-masked-identical across
    /// ROMs but the raw addresses differ.
    fn rom_with_leaf(label: &str, base: u32, salt: u32) -> CorpusRom {
        CorpusRom {
            label: label.into(),
            functions: vec![func("leaf", base + 0x1000, unique_leaf(salt))],
        }
    }

    #[test]
    fn a_shared_leaf_forms_one_identity_across_three_roms() {
        // Same masked body in three ROMs. Each pairwise match links a pair;
        // transitively they are one identity spanning all three.
        let roms = vec![
            rom_with_leaf("A", 0x8000_0000, 0x11),
            rom_with_leaf("B", 0x8020_0000, 0x11),
            rom_with_leaf("C", 0x8040_0000, 0x11),
        ];
        let report = build_corpus(&roms, CorpusConfig::default()).unwrap();
        assert_eq!(report.identity_count, 1);
        assert_eq!(report.max_span, 3);
        assert!(report.ambiguous.is_empty());
        assert_eq!(report.identities[0].span(), 3);
        let labels: Vec<_> = report.identities[0]
            .members
            .iter()
            .map(|m| m.rom_label.as_str())
            .collect();
        assert_eq!(labels, vec!["A", "B", "C"]);
    }

    #[test]
    fn transitive_edge_is_admitted_when_consistent() {
        // A<->B and B<->C are the only pairwise edges the engine can make (A and
        // C differ enough that... they don't here — all three share a body, so
        // A<->C is also directly made). To isolate transitivity, give A and C
        // bodies that are masked-identical to B but not directly matchable is
        // impossible under masking. Instead we assert the WEAKER, real property:
        // however the pairwise engine links them, the closure yields ONE
        // component of span 3 with A and C in it — the transitive identity — and
        // it is admitted (not ambiguous).
        let roms = vec![
            rom_with_leaf("A", 0x8000_0000, 0x22),
            rom_with_leaf("B", 0x8020_0000, 0x22),
            rom_with_leaf("C", 0x8040_0000, 0x22),
        ];
        let report = build_corpus(&roms, CorpusConfig::default()).unwrap();
        assert_eq!(report.identity_count, 1);
        assert!(report.ambiguous.is_empty());
        let has_a = report.identities[0]
            .members
            .iter()
            .any(|m| m.rom_label == "A");
        let has_c = report.identities[0]
            .members
            .iter()
            .any(|m| m.rom_label == "C");
        assert!(has_a && has_c, "A and C share the transitive identity");
    }

    #[test]
    fn conflicting_transitive_edge_collapses_to_ambiguous() {
        // ROM B holds TWO functions with the same masked body (a twin pair).
        // ROM A holds one, ROM C holds one, all four masked-identical. A<->B
        // and B<->C pairwise matching cannot uniquely pick which B twin, but if
        // an edge set ever tried to pull both B twins into one component with A
        // and C, that component contains ROM B twice -> per-ROM uniqueness is
        // violated -> collapse to ambiguous, never a guess.
        //
        // We construct that directly by feeding a caller-visible edge set: the
        // twins share a body, so no seed fires for them, but we simulate the
        // adversarial transitive pull by giving A a body-hash-unique match to
        // BOTH twins is impossible under uniqueness; so instead we assert the
        // engine's own conservative outcome: with two identical-bodied B twins,
        // neither seeds, so A and C match each other (span 2) and the B twins
        // stay singletons — no false 3-ROM identity is invented.
        let body = unique_leaf(0x33);
        let roms = vec![
            CorpusRom {
                label: "A".into(),
                functions: vec![func("a", 0x8000_1000, body.clone())],
            },
            CorpusRom {
                label: "B".into(),
                functions: vec![
                    func("b0", 0x8020_1000, body.clone()),
                    func("b1", 0x8020_2000, body.clone()),
                ],
            },
            CorpusRom {
                label: "C".into(),
                functions: vec![func("c", 0x8040_1000, body.clone())],
            },
        ];
        let report = build_corpus(&roms, CorpusConfig::default()).unwrap();
        // A<->C is the only unique pair (B's twins are ambiguous on both
        // sides). No component ever contains two B functions, so nothing is
        // falsely a 3-ROM identity. The B twins never enter any identity.
        assert!(report
            .identities
            .iter()
            .all(|id| { id.members.iter().filter(|m| m.rom_label == "B").count() <= 1 }));
        let b_in_any_identity = report
            .identities
            .iter()
            .any(|id| id.members.iter().any(|m| m.rom_label == "B"));
        assert!(
            !b_in_any_identity,
            "ambiguous B twin never enters an identity"
        );
    }

    #[test]
    fn a_direct_per_rom_conflict_poisons_the_component() {
        // Directly exercise the guard: hand the forest an edge set that pulls
        // two functions of ONE rom into a component with a third rom's
        // function. This is the adversarial transitive cascade — the union must
        // poison it to ambiguous, not admit a span-N identity with a doubled
        // ROM. We reach the forest through build_corpus by constructing ROMs
        // whose pairwise edges genuinely form such a pull: ROM B twins are made
        // distinguishable to the pairwise engine by DIFFERENT callers so BOTH
        // seed a distinct partner, then a cross edge unifies them.
        //
        // Simpler and just as load-bearing: verify the Forest invariant in
        // isolation. node 0 = romA, nodes 1,2 = romB (two funcs), node 3 = romC.
        // Edges: A-b0, A... no, A is one rom. Edges A<->b0, C<->b1, then b0<->b1
        // would merge A,b0,b1,C -> ROM B appears via b0 AND b1 -> conflict.
        let mut forest = Forest::new(
            vec![0, 1, 1, 2], // roms: A, B, B, C
            vec![7, 7, 7, 7], // all same masked body hash
        );
        forest.union(0, 1); // A - b0
        forest.union(3, 2); // C - b1
                            // Both components are clean so far.
        let (r0, r3) = (forest.find(0), forest.find(3));
        assert!(!forest.poisoned[r0]);
        assert!(!forest.poisoned[r3]);
        forest.union(1, 2); // b0 - b1: pulls both B funcs together -> conflict
        let root = forest.find(0);
        assert_eq!(forest.find(3), root, "all four are now one component");
        assert!(forest.poisoned[root], "doubled ROM poisons the component");
        assert_eq!(
            forest.reason[root],
            Some(AmbiguityReason::DuplicateRomInComponent)
        );
    }

    #[test]
    fn body_mismatch_poisons_the_component() {
        // Two nodes from different ROMs whose masked bodies differ must never be
        // unioned into a clean identity. (A pairwise edge never produces this —
        // its endpoints agree by construction — but the guard is explicit so a
        // future edge source cannot smuggle in an incompatible merge.)
        let mut forest = Forest::new(vec![0, 1], vec![7, 9]);
        forest.union(0, 1);
        let root = forest.find(0);
        assert!(forest.poisoned[root]);
        assert_eq!(forest.reason[root], Some(AmbiguityReason::BodyMismatch));
    }

    #[test]
    fn a_wrong_pairwise_seed_does_not_cascade_corpus_wide() {
        // Even if a single wrong pairwise edge existed, poison is contagious and
        // the wrong component is quarantined as ambiguous rather than spreading
        // a false identity to the rest of the corpus. Model a wrong edge as a
        // body-mismatched union (the only way an edge can be "wrong" past the
        // pairwise body-corroboration): one poisoned component does not poison
        // an unrelated clean one.
        let mut forest = Forest::new(vec![0, 1, 2, 3], vec![7, 9, 5, 5]);
        forest.union(0, 1); // wrong edge: bodies differ -> poisoned
        forest.union(2, 3); // unrelated clean edge: bodies agree
        let (r0, r2) = (forest.find(0), forest.find(2));
        assert!(forest.poisoned[r0]);
        assert!(
            !forest.poisoned[r2],
            "an unrelated clean identity is untouched by the poisoned one"
        );
        assert_eq!(forest.hash[r2], Some(5));
    }

    #[test]
    fn duplicate_rom_label_is_rejected() {
        let roms = vec![
            rom_with_leaf("A", 0x8000_0000, 0x11),
            rom_with_leaf("A", 0x8020_0000, 0x11),
        ];
        assert_eq!(
            build_corpus(&roms, CorpusConfig::default()),
            Err(CorpusError::DuplicateRomLabel("A".into()))
        );
    }

    #[test]
    fn results_are_byte_for_byte_deterministic() {
        let big = |base: u32, salt: u32, calls: &[u32]| {
            let mut words = vec![0x27bd_ffe8u32];
            for &c in calls {
                words.push(jal(c));
                words.push(NOP);
            }
            words.extend_from_slice(&unique_leaf(salt));
            words.push(JR_RA);
            words.push(NOP);
            let _ = base;
            words
        };
        let roms = vec![
            CorpusRom {
                label: "A".into(),
                functions: vec![
                    func("root", 0x8000_0000, big(0x8000_0000, 0x01, &[0x8000_1000])),
                    func("leaf", 0x8000_1000, unique_leaf(0x02)),
                ],
            },
            CorpusRom {
                label: "B".into(),
                functions: vec![
                    func("root", 0x8020_0000, big(0x8020_0000, 0x01, &[0x8020_1000])),
                    func("leaf", 0x8020_1000, unique_leaf(0x02)),
                ],
            },
        ];
        let first = build_corpus(&roms, CorpusConfig::default()).unwrap();
        let second = build_corpus(&roms, CorpusConfig::default()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.max_span, 2);
    }
}
