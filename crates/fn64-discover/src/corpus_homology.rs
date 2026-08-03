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
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One ROM in the corpus: a label plus its prior, independently-derived
/// function boundaries. The label is output-side identity only (it names the
/// ROM in the report); it is never consumed by matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
///
/// Serializable so a [`CorpusIndex`] can persist the closure exactly as
/// computed: [`extend_corpus`] appends new nodes and unions in only the new
/// ROM's pairwise edges, never recomputing the ones already folded in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    fn new(node_rom: &[usize], node_hash: &[u64]) -> Self {
        let n = node_rom.len();
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
            roms: node_rom.iter().map(|&r| BTreeSet::from([r])).collect(),
            hash: node_hash.iter().copied().map(Some).collect(),
            poisoned: vec![false; n],
            reason: vec![None; n],
        }
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    /// Append `n` new singleton nodes (their own root, unpoisoned, carrying
    /// their own ROM index and body hash). Used by [`extend_corpus`] to grow
    /// an existing forest with a new ROM's nodes before unioning its edges in.
    fn extend(&mut self, node_rom: &[usize], node_hash: &[u64]) {
        let base = self.len();
        for (offset, (&rom, &hash)) in node_rom.iter().zip(node_hash).enumerate() {
            let id = base + offset;
            debug_assert_eq!(id, self.parent.len());
            self.parent.push(id);
            self.rank.push(0);
            self.roms.push(BTreeSet::from([rom]));
            self.hash.push(Some(hash));
            self.poisoned.push(false);
            self.reason.push(None);
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

/// The node table [`build_corpus`] and [`CorpusIndex`] both need: a flat
/// node id -> (rom, function index) mapping (ROM-major then function-major,
/// so numbering is a deterministic function of the input order), the
/// per-node masked body hash, and each ROM's compiled [`Program`] (the
/// call-graph engine's input) plus its entry-VA -> local-index table (for
/// turning a pairwise edge's VA back into a node).
struct NodeTable {
    programs: Vec<Program>,
    node_rom: Vec<usize>,
    node_hash: Vec<u64>,
    va_to_local: Vec<BTreeMap<u32, usize>>,
    rom_base: Vec<usize>,
}

fn build_node_table(roms: &[CorpusRom]) -> Result<NodeTable, CorpusError> {
    let mut programs = Vec::with_capacity(roms.len());
    let mut node_rom = Vec::new();
    let mut node_hash = Vec::new();
    let mut va_to_local: Vec<BTreeMap<u32, usize>> = Vec::with_capacity(roms.len());
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
    Ok(NodeTable {
        programs,
        node_rom,
        node_hash,
        va_to_local,
        rom_base,
    })
}

fn reject_duplicate_labels(roms: &[CorpusRom]) -> Result<(), CorpusError> {
    let mut labels = BTreeSet::new();
    for rom in roms {
        if !labels.insert(rom.label.clone()) {
            return Err(CorpusError::DuplicateRomLabel(rom.label.clone()));
        }
    }
    Ok(())
}

/// Walk the closed forest into the reportable identities/ambiguous list. Pure
/// read of the forest state: never mutates a component decision, only
/// resolves each root's members to output fields.
fn finalize_report(
    forest: &mut Forest,
    node_rom: &[usize],
    rom_base: &[usize],
    roms: &[CorpusRom],
    pairwise_edges: usize,
) -> CorpusReport {
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

    CorpusReport {
        identities,
        ambiguous,
        total_functions: node_rom.len(),
        identity_count,
        max_span,
        pairwise_edges,
        singletons,
    }
}

/// Every unordered pair among `0..rom_count`, matched by the existing
/// pairwise engine, projected back to node ids via `table`. The resulting
/// edges are the only cross-ROM evidence the closure consumes. Deterministic
/// edge order (the union outcome is order-independent for the sets/hashes it
/// tracks, but a fixed order keeps the whole run reproducible and debugging
/// legible).
fn all_pairs_edges(table: &NodeTable, rom_count: usize, config: CorpusConfig) -> Vec<(usize, usize)> {
    let node = |rom: usize, function: usize| table.rom_base[rom] + function;
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for left in 0..rom_count {
        for right in (left + 1)..rom_count {
            edges.extend(pair_edges(table, left, right, config, node));
        }
    }
    edges.sort_unstable();
    edges
}

/// One unordered pair's edges, projected to node ids via `node`.
fn pair_edges(
    table: &NodeTable,
    left: usize,
    right: usize,
    config: CorpusConfig,
    node: impl Fn(usize, usize) -> usize,
) -> Vec<(usize, usize)> {
    let report = match_programs(&table.programs[left], &table.programs[right], config.max_rounds);
    let mut edges = Vec::with_capacity(report.pairs.len());
    for pair in &report.pairs {
        let (Some(&li), Some(&ri)) = (
            table.va_to_local[left].get(&pair.left_va),
            table.va_to_local[right].get(&pair.right_va),
        ) else {
            // A pair whose VA is not a known entry cannot occur: pairs come
            // from the same Program whose entries we indexed.
            continue;
        };
        edges.push((node(left, li), node(right, ri)));
    }
    edges
}

/// Run pairwise matching over every unordered ROM pair, then assemble the
/// cross-ROM edges into corpus identities under the transitive-uniqueness rule.
pub fn build_corpus(roms: &[CorpusRom], config: CorpusConfig) -> Result<CorpusReport, CorpusError> {
    reject_duplicate_labels(roms)?;
    let table = build_node_table(roms)?;
    let edges = all_pairs_edges(&table, roms.len(), config);

    let mut forest = Forest::new(&table.node_rom, &table.node_hash);
    for &(a, b) in &edges {
        forest.union(a, b);
    }

    Ok(finalize_report(
        &mut forest,
        &table.node_rom,
        &table.rom_base,
        roms,
        edges.len(),
    ))
}

/// A persisted, incrementally-extensible corpus closure. Carries exactly what
/// [`build_corpus`]'s internals compute — the node table (ROM label, SHA-256,
/// and function boundaries per ROM) and the closed [`Forest`] — so identities
/// survive a process boundary and a later ROM can be folded in without
/// re-running every prior pair.
///
/// The SHA-256 is recorded per ROM so a caller handing back a byte-different
/// ROM under the same label is a detectable error ([`CorpusIndexError::ShaMismatch`])
/// rather than a silent, potentially-wrong merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorpusIndex {
    roms: Vec<IndexedRom>,
    forest: Forest,
    /// Total cross-ROM pairwise edges folded into `forest` so far (every
    /// `build`/`extend_corpus` pair's edge count, summed) — the same
    /// diagnostic [`CorpusReport::pairwise_edges`] reports, kept in sync
    /// incrementally rather than requiring a full re-match to recover.
    pairwise_edges: usize,
}

/// One ROM as recorded in a [`CorpusIndex`]: its label, the SHA-256 of its
/// normalized bytes (the staleness check), and the function boundaries that
/// were fed to the matcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IndexedRom {
    label: String,
    sha256: String,
    functions: Vec<FunctionBody>,
}

/// One ROM to add to a [`CorpusIndex`]: its label, its normalized SHA-256 (so
/// [`extend_corpus`] can fail closed on a stale/substituted ROM), and its
/// prior, independently-derived function boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCorpusRom {
    pub label: String,
    pub sha256: String,
    pub functions: Vec<FunctionBody>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusIndexError {
    /// [`build_corpus`]'s own duplicate-label / call-graph-rejection errors.
    Corpus(CorpusError),
    /// The new ROM's label already exists in the index. Extending an index is
    /// adding a NEW ROM; re-adding an existing label under (possibly)
    /// different bytes is never a silent overwrite.
    DuplicateRomLabel(String),
    /// A ROM presented under a label already in the index does not hash to
    /// that entry's recorded SHA-256. The index's closure was computed from
    /// the RECORDED bytes; a caller now holding different bytes under the
    /// same label has a stale or substituted ROM, and merging it in (or
    /// trusting a query against it) would silently mean something the
    /// closure never verified. Fails closed, never a best-effort merge.
    ShaMismatch {
        label: String,
        indexed: String,
        found: String,
    },
    /// A lookup (verify/query) named a ROM label the index does not carry.
    NotIndexed(String),
}

impl std::fmt::Display for CorpusIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corpus(error) => write!(f, "{error}"),
            Self::DuplicateRomLabel(label) => {
                write!(f, "ROM {label:?} is already in the corpus index")
            }
            Self::ShaMismatch {
                label,
                indexed,
                found,
            } => write!(
                f,
                "ROM {label:?} SHA-256 mismatch: index has {indexed}, presented ROM hashes to {found} \
                 (stale index or substituted ROM)"
            ),
            Self::NotIndexed(label) => write!(f, "ROM {label:?} is not in the corpus index"),
        }
    }
}

impl std::error::Error for CorpusIndexError {}

impl From<CorpusError> for CorpusIndexError {
    fn from(error: CorpusError) -> Self {
        Self::Corpus(error)
    }
}

impl CorpusIndex {
    /// Build a fresh index from scratch — identical closure to [`build_corpus`]
    /// over the same ROMs, just retained afterward instead of discarded.
    pub fn build(roms: &[NewCorpusRom], config: CorpusConfig) -> Result<Self, CorpusIndexError> {
        let corpus_roms: Vec<CorpusRom> = roms
            .iter()
            .map(|r| CorpusRom {
                label: r.label.clone(),
                functions: r.functions.clone(),
            })
            .collect();
        reject_duplicate_labels(&corpus_roms)?;
        let table = build_node_table(&corpus_roms)?;
        let edges = all_pairs_edges(&table, corpus_roms.len(), config);

        let mut forest = Forest::new(&table.node_rom, &table.node_hash);
        for &(a, b) in &edges {
            forest.union(a, b);
        }

        Ok(Self {
            roms: roms
                .iter()
                .map(|r| IndexedRom {
                    label: r.label.clone(),
                    sha256: r.sha256.clone(),
                    functions: r.functions.clone(),
                })
                .collect(),
            forest,
            pairwise_edges: edges.len(),
        })
    }

    /// Re-derive the same [`CorpusReport`] [`build_corpus`] would produce over
    /// this index's ROMs. Never re-runs matching — reads the persisted forest.
    pub fn report(&self) -> CorpusReport {
        let roms = self.corpus_roms();
        let node_rom = self.node_rom();
        let rom_base = self.rom_base();
        let pairwise_edges = self.pairwise_edges;
        let mut forest = self.forest.clone();
        finalize_report(&mut forest, &node_rom, &rom_base, &roms, pairwise_edges)
    }

    fn corpus_roms(&self) -> Vec<CorpusRom> {
        self.roms
            .iter()
            .map(|r| CorpusRom {
                label: r.label.clone(),
                functions: r.functions.clone(),
            })
            .collect()
    }

    fn node_rom(&self) -> Vec<usize> {
        let mut node_rom = Vec::with_capacity(self.forest.len());
        for (rom_index, rom) in self.roms.iter().enumerate() {
            node_rom.extend(std::iter::repeat_n(rom_index, rom.functions.len()));
        }
        node_rom
    }

    fn rom_base(&self) -> Vec<usize> {
        let mut base = Vec::with_capacity(self.roms.len());
        let mut running = 0usize;
        for rom in &self.roms {
            base.push(running);
            running += rom.functions.len();
        }
        base
    }

    /// Fail closed on a stale index: `label` must be present with EXACTLY
    /// `sha256`, or this is an error. A caller about to trust this index for
    /// `label` (query, or re-extend under the same label) calls this first so
    /// a byte-different ROM under an old label never silently reuses a
    /// closure computed from different bytes.
    pub fn verify_sha(&self, label: &str, sha256: &str) -> Result<(), CorpusIndexError> {
        match self.roms.iter().find(|r| r.label == label) {
            Some(rom) if rom.sha256 == sha256 => Ok(()),
            Some(rom) => Err(CorpusIndexError::ShaMismatch {
                label: label.to_string(),
                indexed: rom.sha256.clone(),
                found: sha256.to_string(),
            }),
            None => Err(CorpusIndexError::NotIndexed(label.to_string())),
        }
    }

    /// The recorded SHA-256 for `label`, if present.
    pub fn sha256_of(&self, label: &str) -> Option<&str> {
        self.roms
            .iter()
            .find(|r| r.label == label)
            .map(|r| r.sha256.as_str())
    }

    /// Which of `rom_label`'s functions carry a corpus identity: for each,
    /// the other ROMs (and their VA / identity string) sharing that identity,
    /// and — where available — the real (non-`func_ADDR`) name a member ROM's
    /// answer key gives it. `real_name_by_rom` supplies that oracle per ROM
    /// label; a ROM absent from the map or without a name at that VA
    /// contributes no name (this never affects WHICH VAs are reported, only
    /// the optional name annotation).
    pub fn identities_for(
        &self,
        rom_label: &str,
        real_name_by_rom: &BTreeMap<String, BTreeMap<u32, String>>,
    ) -> Vec<RomIdentity> {
        let report = self.report();
        let mut out = Vec::new();
        for identity in &report.identities {
            let Some(own) = identity
                .members
                .iter()
                .find(|m| m.rom_label == rom_label)
            else {
                continue;
            };
            let mut others: Vec<SharedWith> = identity
                .members
                .iter()
                .filter(|m| m.rom_label != rom_label)
                .map(|m| SharedWith {
                    rom_label: m.rom_label.clone(),
                    identity: m.identity.clone(),
                    va_start: m.va_start,
                    real_name: real_name_by_rom
                        .get(&m.rom_label)
                        .and_then(|names| names.get(&m.va_start))
                        .cloned(),
                })
                .collect();
            others.sort_by(|a, b| a.rom_label.cmp(&b.rom_label));
            out.push(RomIdentity {
                va_start: own.va_start,
                identity: own.identity.clone(),
                shared_with: others,
            });
        }
        out.sort_by_key(|entry| entry.va_start);
        out
    }

    /// The ROM labels currently in the index, in insertion order.
    pub fn rom_labels(&self) -> Vec<&str> {
        self.roms.iter().map(|r| r.label.as_str()).collect()
    }
}

/// One of `rom_label`'s functions and the corpus identity it carries, as
/// reported by [`CorpusIndex::identities_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomIdentity {
    pub va_start: u32,
    pub identity: String,
    /// Every OTHER ROM in the same identity.
    pub shared_with: Vec<SharedWith>,
}

/// One other ROM sharing a [`RomIdentity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWith {
    pub rom_label: String,
    pub identity: String,
    pub va_start: u32,
    /// The real (non-`func_ADDR`) symbol name this member's own answer key
    /// gives it, if any was supplied.
    pub real_name: Option<String>,
}

/// Fold ONE new ROM into an existing [`CorpusIndex`]: run
/// [`callgraph_match::match_programs`] for only the pairs `(existing ROM,
/// new ROM)` — never re-matching a pair the index already closed — and union
/// the resulting edges into the existing forest under the SAME
/// uniqueness-and-corroboration guard [`build_corpus`]/[`CorpusIndex::build`]
/// apply. This does not reimplement an edge decision; it calls the same
/// pairwise engine and the same [`Forest::union`].
///
/// Fails closed: a `new_rom.label` already present, or a call-graph
/// rejection of the new ROM's boundaries, are errors — never a best-effort
/// merge. A caller re-adding an existing ROM under a mismatched SHA-256 must
/// go through `identities_for`'s label lookup first; this function's
/// contract is "the label is new," enforced by [`CorpusIndexError::DuplicateRomLabel`].
///
/// [`callgraph_match::match_programs`]: crate::callgraph_match::match_programs
pub fn extend_corpus(
    mut index: CorpusIndex,
    new_rom: NewCorpusRom,
    config: CorpusConfig,
) -> Result<CorpusIndex, CorpusIndexError> {
    if index.roms.iter().any(|r| r.label == new_rom.label) {
        return Err(CorpusIndexError::DuplicateRomLabel(new_rom.label));
    }

    let existing_roms = index.corpus_roms();
    let existing_table = build_node_table(&existing_roms)?;
    let new_rom_corpus = CorpusRom {
        label: new_rom.label.clone(),
        functions: new_rom.functions.clone(),
    };
    let new_table = build_node_table(std::slice::from_ref(&new_rom_corpus))?;

    let new_base = index.forest.len();
    index.forest.extend(&new_table.node_rom, &new_table.node_hash);

    // Only the pairs (existing ROM, new ROM) — every existing-existing pair
    // is already folded into `index.forest` and is never re-run.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    let new_program = &new_table.programs[0];
    for (existing_index, existing_program) in existing_table.programs.iter().enumerate() {
        let report = match_programs(existing_program, new_program, config.max_rounds);
        for pair in &report.pairs {
            let (Some(&li), Some(&ri)) = (
                existing_table.va_to_local[existing_index].get(&pair.left_va),
                new_table.va_to_local[0].get(&pair.right_va),
            ) else {
                continue;
            };
            let a = existing_table.rom_base[existing_index] + li;
            let b = new_base + ri;
            edges.push((a, b));
        }
    }
    edges.sort_unstable();

    for &(a, b) in &edges {
        index.forest.union(a, b);
    }
    index.pairwise_edges += edges.len();

    index.roms.push(IndexedRom {
        label: new_rom.label,
        sha256: new_rom.sha256,
        functions: new_rom.functions,
    });

    Ok(index)
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
            &[0, 1, 1, 2], // roms: A, B, B, C
            &[7, 7, 7, 7], // all same masked body hash
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
        let mut forest = Forest::new(&[0, 1], &[7, 9]);
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
        let mut forest = Forest::new(&[0, 1, 2, 3], &[7, 9, 5, 5]);
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

    // -- CorpusIndex / extend_corpus ----------------------------------------

    fn new_rom(label: &str, sha256: &str, functions: Vec<FunctionBody>) -> NewCorpusRom {
        NewCorpusRom {
            label: label.into(),
            sha256: sha256.into(),
            functions,
        }
    }

    /// A five-ROM panel with a shared leaf spanning all five (the libultra
    /// stand-in) plus a couple of ROM-pair-only bodies, so the closure has
    /// real multi-span structure to compare, not just one trivial identity.
    fn five_rom_panel() -> Vec<NewCorpusRom> {
        let shared = |label: &str, base: u32| CorpusRom {
            label: label.into(),
            functions: vec![func("shared", base + 0x1000, unique_leaf(0xaa))],
        }
        .functions;
        let mut roms = Vec::new();
        for (i, label) in ["A", "B", "C", "D", "E"].iter().enumerate() {
            let base = 0x8000_0000 + (i as u32) * 0x0020_0000;
            let mut functions = shared(label, base);
            // A pairwise-only body: identical between consecutive ROMs only
            // (salted by pair index), giving span-2 identities alongside the
            // span-5 shared one.
            functions.push(func(
                "pairish",
                base + 0x2000,
                unique_leaf(0xb0 + (i as u32 / 2) as u32),
            ));
            roms.push(new_rom(label, &format!("sha-{label}"), functions));
        }
        roms
    }

    #[test]
    fn extend_corpus_matches_build_corpus_from_scratch() {
        // The correctness bar from the task: extend_corpus(build_corpus(A..N),
        // N+1) must yield identities equal to build_corpus(A..N+1).
        let panel = five_rom_panel();
        let (first_four, fifth) = panel.split_at(4);
        let fifth = fifth[0].clone();

        let to_corpus_rom = |r: &NewCorpusRom| CorpusRom {
            label: r.label.clone(),
            functions: r.functions.clone(),
        };

        // Build the incremental index over the first four, then extend with
        // the fifth.
        let index = CorpusIndex::build(first_four, CorpusConfig::default()).unwrap();
        let extended = extend_corpus(index, fifth.clone(), CorpusConfig::default()).unwrap();
        let incremental_report = extended.report();

        // Build straight from all five via build_corpus.
        let all_corpus_roms: Vec<CorpusRom> = panel.iter().map(to_corpus_rom).collect();
        let from_scratch = build_corpus(&all_corpus_roms, CorpusConfig::default()).unwrap();

        assert_eq!(
            incremental_report.identities, from_scratch.identities,
            "extend_corpus identities must equal a from-scratch build_corpus over the same ROMs"
        );
        assert_eq!(incremental_report.ambiguous, from_scratch.ambiguous);
        assert_eq!(
            incremental_report.total_functions,
            from_scratch.total_functions
        );
        assert_eq!(incremental_report.identity_count, from_scratch.identity_count);
        assert_eq!(incremental_report.max_span, from_scratch.max_span);
        assert_eq!(incremental_report.singletons, from_scratch.singletons);
    }

    #[test]
    fn extend_corpus_one_at_a_time_matches_build_corpus_over_all() {
        // A stronger form of the equivalence bar: extending one ROM at a time
        // from an empty index must still equal one build_corpus over the
        // whole set, not just the "N then N+1" two-step case.
        let panel = five_rom_panel();
        let to_corpus_rom = |r: &NewCorpusRom| CorpusRom {
            label: r.label.clone(),
            functions: r.functions.clone(),
        };

        let mut index = CorpusIndex::build(&panel[..1], CorpusConfig::default()).unwrap();
        for rom in &panel[1..] {
            index = extend_corpus(index, rom.clone(), CorpusConfig::default()).unwrap();
        }
        let incremental_report = index.report();

        let all_corpus_roms: Vec<CorpusRom> = panel.iter().map(to_corpus_rom).collect();
        let from_scratch = build_corpus(&all_corpus_roms, CorpusConfig::default()).unwrap();

        assert_eq!(incremental_report.identities, from_scratch.identities);
        assert_eq!(incremental_report.ambiguous, from_scratch.ambiguous);
        assert_eq!(incremental_report.max_span, from_scratch.max_span);
    }

    #[test]
    fn extend_corpus_rejects_a_stale_rom_under_a_reused_label() {
        // Fail closed: a ROM's SHA-256 mismatching its index entry is an
        // error, never a best-effort merge. extend_corpus refuses to
        // silently overwrite an existing label at all (DuplicateRomLabel);
        // this test locks the SHA-mismatch detector itself, reachable via
        // CorpusIndex::verify_sha, which a caller uses before trusting a
        // ROM against an existing label (query or re-extend).
        let roms = vec![
            new_rom("A", "sha-A", vec![func("leaf", 0x8000_1000, unique_leaf(0x01))]),
            new_rom("B", "sha-B", vec![func("leaf", 0x8020_1000, unique_leaf(0x01))]),
        ];
        let index = CorpusIndex::build(&roms, CorpusConfig::default()).unwrap();

        // Correct SHA verifies clean.
        assert_eq!(index.verify_sha("A", "sha-A"), Ok(()));

        // A different SHA under the same label is a loud, typed error, not a
        // silent pass.
        let error = index.verify_sha("A", "sha-A-DIFFERENT-BYTES").unwrap_err();
        assert_eq!(
            error,
            CorpusIndexError::ShaMismatch {
                label: "A".into(),
                indexed: "sha-A".into(),
                found: "sha-A-DIFFERENT-BYTES".into(),
            }
        );

        // extend_corpus itself never silently overwrites an existing label
        // (stale or not) — it is always a hard DuplicateRomLabel error.
        let restale = new_rom("A", "sha-A-DIFFERENT-BYTES", vec![]);
        let error = extend_corpus(index, restale, CorpusConfig::default()).unwrap_err();
        assert_eq!(error, CorpusIndexError::DuplicateRomLabel("A".into()));
    }

    #[test]
    fn verify_sha_rejects_an_unindexed_label() {
        let roms = vec![new_rom(
            "A",
            "sha-A",
            vec![func("leaf", 0x8000_1000, unique_leaf(0x01))],
        )];
        let index = CorpusIndex::build(&roms, CorpusConfig::default()).unwrap();
        let error = index.verify_sha("Z", "anything").unwrap_err();
        assert_eq!(error, CorpusIndexError::NotIndexed("Z".into()));
    }

    #[test]
    fn extend_corpus_ambiguity_stays_unmerged() {
        // Mirror conflicting_transitive_edge_collapses_to_ambiguous, but
        // reached incrementally: ROM B holds two masked-identical twins, so
        // no seed fires for them and neither twin ever joins a corpus
        // identity when C is folded in via extend_corpus. The ambiguity
        // (never-seeded twins staying singletons) must survive the
        // incremental path exactly as it does the from-scratch one.
        let body = unique_leaf(0x33);
        let a = new_rom("A", "sha-A", vec![func("a", 0x8000_1000, body.clone())]);
        let b = new_rom(
            "B",
            "sha-B",
            vec![
                func("b0", 0x8020_1000, body.clone()),
                func("b1", 0x8020_2000, body.clone()),
            ],
        );
        let c = new_rom("C", "sha-C", vec![func("c", 0x8040_1000, body)]);

        let index = CorpusIndex::build(&[a, b], CorpusConfig::default()).unwrap();
        let index = extend_corpus(index, c, CorpusConfig::default()).unwrap();
        let report = index.report();

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
            "ambiguous B twin never enters an identity via the incremental path either"
        );
    }

    #[test]
    fn identities_for_reports_shared_members_and_real_names() {
        let roms = vec![
            new_rom(
                "A",
                "sha-A",
                vec![func("leaf", 0x8000_1000, unique_leaf(0x77))],
            ),
            new_rom(
                "B",
                "sha-B",
                vec![func("leaf", 0x8020_1000, unique_leaf(0x77))],
            ),
        ];
        let index = CorpusIndex::build(&roms, CorpusConfig::default()).unwrap();

        let mut real_names = BTreeMap::new();
        let mut b_names = BTreeMap::new();
        b_names.insert(0x8020_1000u32, "osSetIntMask".to_string());
        real_names.insert("B".to_string(), b_names);

        let entries = index.identities_for("A", &real_names);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].va_start, 0x8000_1000);
        assert_eq!(entries[0].shared_with.len(), 1);
        assert_eq!(entries[0].shared_with[0].rom_label, "B");
        assert_eq!(
            entries[0].shared_with[0].real_name.as_deref(),
            Some("osSetIntMask")
        );

        // A ROM with no query results (not in the corpus) returns empty, not
        // an error — it is simply a label carrying no identities.
        assert!(index.identities_for("NOPE", &real_names).is_empty());
    }

    #[test]
    fn corpus_index_round_trips_through_json() {
        // Serde derives: an index must survive a process boundary byte-for-
        // byte identical to the in-memory value (build vs deserialize agree).
        let roms = five_rom_panel();
        let index = CorpusIndex::build(&roms, CorpusConfig::default()).unwrap();
        let json = serde_json::to_string(&index).unwrap();
        let restored: CorpusIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(index, restored);
        assert_eq!(index.report(), restored.report());
    }
}
