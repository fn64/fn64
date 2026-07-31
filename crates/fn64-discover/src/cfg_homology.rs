//! Candidate-only structural homology between two discovered CFGs.
//!
//! This module deliberately does not consume instruction bytes, absolute
//! addresses, names, or proof states. It fingerprints block size, terminator
//! and delay-slot shape, and typed control/call topology with a bounded number
//! of deterministic refinement rounds. Equal fingerprints are proposals for
//! further byte/relocation/semantic validation; they are never proof of a
//! function entry, extent, identity, or name.
//!
//! Common leaf and error-handling shapes collide frequently. A match is
//! therefore emitted only for a fingerprint that occurs exactly once in both
//! CFGs. Every other shared fingerprint is an explicit [`AmbiguousGroup`], and
//! fingerprints present on only one side are explicit [`UnmatchedBlock`]s.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// A deterministic, address- and relocation-invariant structural digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StructuralFingerprint(pub [u8; 32]);

/// Hard resource limits for one comparison. These are algorithm inputs, so a
/// caller changing them can include them in its pass/cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomologyLimits {
    /// Maximum blocks accepted in either CFG.
    pub max_blocks: usize,
    /// Maximum typed edges materialized from either CFG.
    pub max_edges: usize,
    /// Weisfeiler-Lehman-style neighborhood refinement rounds.
    pub refinement_rounds: usize,
    /// Maximum addresses retained on each side of an ambiguity report. Exact
    /// totals and a truncation flag are still reported.
    pub max_ambiguous_addresses: usize,
}

impl Default for HomologyLimits {
    fn default() -> Self {
        Self {
            max_blocks: 1_000_000,
            max_edges: 8_000_000,
            refinement_rounds: 6,
            max_ambiguous_addresses: 256,
        }
    }
}

const MAX_REFINEMENT_ROUNDS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomologyError {
    InvalidLimit(&'static str),
    TooManyBlocks {
        side: HomologySide,
        count: usize,
        limit: usize,
    },
    TooManyEdges {
        side: HomologySide,
        count: usize,
        limit: usize,
    },
    DuplicateBlockStart {
        side: HomologySide,
        start_va: u32,
    },
    MalformedBlock {
        side: HomologySide,
        start_va: u32,
        end_va: u32,
    },
}

impl std::fmt::Display for HomologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimit(name) => write!(f, "invalid CFG homology limit: {name}"),
            Self::TooManyBlocks { side, count, limit } => write!(
                f,
                "{side:?} CFG has {count} blocks, exceeding homology limit {limit}"
            ),
            Self::TooManyEdges { side, count, limit } => write!(
                f,
                "{side:?} CFG has at least {count} edges, exceeding homology limit {limit}"
            ),
            Self::DuplicateBlockStart { side, start_va } => write!(
                f,
                "{side:?} CFG contains duplicate block start 0x{start_va:08x}"
            ),
            Self::MalformedBlock {
                side,
                start_va,
                end_va,
            } => write!(
                f,
                "{side:?} CFG block [0x{start_va:08x},0x{end_va:08x}) is empty or not word-aligned"
            ),
        }
    }
}

impl std::error::Error for HomologyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HomologySide {
    Query,
    Reference,
}

/// One block's final candidate fingerprint. `start_va` is output identity,
/// not an input to `fingerprint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintedBlock {
    pub start_va: u32,
    pub fingerprint: StructuralFingerprint,
}

/// Address-invariant structural index for one CFG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfgStructuralIndex {
    pub graph_fingerprint: StructuralFingerprint,
    pub blocks: Vec<FingerprintedBlock>,
    pub edge_count: usize,
    pub refinement_rounds: usize,
}

/// A conservative 1:1 candidate. This is intentionally not a `Fact`, has no
/// `ProofState`, and must not directly feed an authoritative pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomologyCandidate {
    pub query_start_va: u32,
    pub reference_start_va: u32,
    pub fingerprint: StructuralFingerprint,
}

/// A structural fingerprint shared by both CFGs but not unique on both sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousGroup {
    pub fingerprint: StructuralFingerprint,
    pub query_start_vas: Vec<u32>,
    pub reference_start_vas: Vec<u32>,
    pub total_query_blocks: usize,
    pub total_reference_blocks: usize,
    pub query_addresses_truncated: bool,
    pub reference_addresses_truncated: bool,
}

/// A block for which the other CFG has no equal structural fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedBlock {
    pub side: HomologySide,
    pub start_va: u32,
    pub fingerprint: StructuralFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomologyReport {
    pub query_graph_fingerprint: StructuralFingerprint,
    pub reference_graph_fingerprint: StructuralFingerprint,
    pub candidates: Vec<HomologyCandidate>,
    pub ambiguous: Vec<AmbiguousGroup>,
    pub unmatched: Vec<UnmatchedBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum EdgeKind {
    Fallthrough = 1,
    Tail = 2,
    CallTarget = 3,
    CallContinuation = 4,
    BranchTaken = 5,
    BranchFallthrough = 6,
    LikelyTaken = 7,
    LikelyAnnulledFallthrough = 8,
    IndirectJumpTarget = 9,
    IndirectCallTarget = 10,
    IndirectCallContinuation = 11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EdgeTarget {
    Internal(usize),
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StructuralEdge {
    kind: EdgeKind,
    target: EdgeTarget,
}

#[derive(Debug, Clone, Copy)]
struct LocalShape {
    word_count: u32,
    terminator_tag: u8,
    has_delay_slot: bool,
    annuls_delay_slot_on_fallthrough: bool,
    resolved_target_count: u32,
}

/// Build an address-invariant index for a CFG. Public callers normally use
/// [`match_cfgs`], while a corpus index can persist these block fingerprints
/// as candidate-search keys.
pub fn fingerprint_cfg(
    cfg: &Cfg,
    side: HomologySide,
    limits: HomologyLimits,
) -> Result<CfgStructuralIndex, HomologyError> {
    validate_limits(limits)?;
    if cfg.blocks.len() > limits.max_blocks {
        return Err(HomologyError::TooManyBlocks {
            side,
            count: cfg.blocks.len(),
            limit: limits.max_blocks,
        });
    }

    let mut ordered: Vec<&BasicBlock> = cfg.blocks.iter().collect();
    ordered.sort_by_key(|block| (block.start_va, block.end_va));
    for pair in ordered.windows(2) {
        if pair[0].start_va == pair[1].start_va {
            return Err(HomologyError::DuplicateBlockStart {
                side,
                start_va: pair[0].start_va,
            });
        }
    }

    let starts: BTreeMap<u32, usize> = ordered
        .iter()
        .enumerate()
        .map(|(index, block)| (block.start_va, index))
        .collect();
    let mut local = Vec::with_capacity(ordered.len());
    let mut edges = Vec::with_capacity(ordered.len());
    let mut edge_count = 0usize;

    for block in &ordered {
        let Some(byte_len) = block.end_va.checked_sub(block.start_va) else {
            return Err(HomologyError::MalformedBlock {
                side,
                start_va: block.start_va,
                end_va: block.end_va,
            });
        };
        if byte_len == 0 || !byte_len.is_multiple_of(4) {
            return Err(HomologyError::MalformedBlock {
                side,
                start_va: block.start_va,
                end_va: block.end_va,
            });
        }
        // Reject from the terminator's existing target count before cloning
        // any target list into our edge representation. The limit therefore
        // bounds this pass's allocation as well as its eventual graph size.
        let declared_edges =
            declared_edge_count(&block.terminator).ok_or(HomologyError::TooManyEdges {
                side,
                count: usize::MAX,
                limit: limits.max_edges,
            })?;
        edge_count = edge_count
            .checked_add(declared_edges)
            .ok_or(HomologyError::TooManyEdges {
                side,
                count: usize::MAX,
                limit: limits.max_edges,
            })?;
        if edge_count > limits.max_edges {
            return Err(HomologyError::TooManyEdges {
                side,
                count: edge_count,
                limit: limits.max_edges,
            });
        }
        let (shape, block_edges) = describe_block(block, &starts);
        local.push(LocalShape {
            word_count: byte_len / 4,
            ..shape
        });
        edges.push(block_edges);
    }

    let mut labels: Vec<StructuralFingerprint> = local.iter().map(hash_local).collect();
    for round in 0..limits.refinement_rounds {
        labels = local
            .iter()
            .enumerate()
            .map(|(index, shape)| {
                hash_refined(round, *shape, labels[index], &edges[index], &labels)
            })
            .collect();
    }

    let blocks: Vec<FingerprintedBlock> = ordered
        .iter()
        .zip(&labels)
        .map(|(block, &fingerprint)| FingerprintedBlock {
            start_va: block.start_va,
            fingerprint,
        })
        .collect();
    let graph_fingerprint = hash_graph(&labels, &edges);
    Ok(CfgStructuralIndex {
        graph_fingerprint,
        blocks,
        edge_count,
        refinement_rounds: limits.refinement_rounds,
    })
}

/// Compare two CFGs and return candidate, ambiguous, and unmatched partitions.
/// Only a fingerprint unique in both CFGs becomes a candidate.
pub fn match_cfgs(
    query: &Cfg,
    reference: &Cfg,
    limits: HomologyLimits,
) -> Result<HomologyReport, HomologyError> {
    let query_index = fingerprint_cfg(query, HomologySide::Query, limits)?;
    let reference_index = fingerprint_cfg(reference, HomologySide::Reference, limits)?;
    let query_buckets = buckets(&query_index.blocks);
    let reference_buckets = buckets(&reference_index.blocks);
    let fingerprints: BTreeSet<_> = query_buckets
        .keys()
        .chain(reference_buckets.keys())
        .copied()
        .collect();

    let mut candidates = Vec::new();
    let mut ambiguous = Vec::new();
    let mut unmatched = Vec::new();
    for fingerprint in fingerprints {
        let query_blocks = query_buckets
            .get(&fingerprint)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let reference_blocks = reference_buckets
            .get(&fingerprint)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        match (query_blocks, reference_blocks) {
            ([query_start_va], [reference_start_va]) => candidates.push(HomologyCandidate {
                query_start_va: *query_start_va,
                reference_start_va: *reference_start_va,
                fingerprint,
            }),
            ([], reference_only) => {
                unmatched.extend(reference_only.iter().map(|&start_va| UnmatchedBlock {
                    side: HomologySide::Reference,
                    start_va,
                    fingerprint,
                }))
            }
            (query_only, []) => {
                unmatched.extend(query_only.iter().map(|&start_va| UnmatchedBlock {
                    side: HomologySide::Query,
                    start_va,
                    fingerprint,
                }))
            }
            (query_many, reference_many) => {
                let (query_start_vas, query_addresses_truncated) =
                    bounded_addresses(query_many, limits.max_ambiguous_addresses);
                let (reference_start_vas, reference_addresses_truncated) =
                    bounded_addresses(reference_many, limits.max_ambiguous_addresses);
                ambiguous.push(AmbiguousGroup {
                    fingerprint,
                    query_start_vas,
                    reference_start_vas,
                    total_query_blocks: query_many.len(),
                    total_reference_blocks: reference_many.len(),
                    query_addresses_truncated,
                    reference_addresses_truncated,
                });
            }
        }
    }
    candidates.sort_by_key(|candidate| (candidate.query_start_va, candidate.reference_start_va));
    ambiguous.sort_by_key(|group| group.fingerprint);
    unmatched.sort_by_key(|block| (block.side, block.start_va, block.fingerprint));

    Ok(HomologyReport {
        query_graph_fingerprint: query_index.graph_fingerprint,
        reference_graph_fingerprint: reference_index.graph_fingerprint,
        candidates,
        ambiguous,
        unmatched,
    })
}

fn validate_limits(limits: HomologyLimits) -> Result<(), HomologyError> {
    if limits.max_blocks == 0 {
        return Err(HomologyError::InvalidLimit("max_blocks must be nonzero"));
    }
    if limits.max_edges == 0 {
        return Err(HomologyError::InvalidLimit("max_edges must be nonzero"));
    }
    if limits.refinement_rounds > MAX_REFINEMENT_ROUNDS {
        return Err(HomologyError::InvalidLimit(
            "refinement_rounds exceeds hard bound 32",
        ));
    }
    if limits.max_ambiguous_addresses == 0 {
        return Err(HomologyError::InvalidLimit(
            "max_ambiguous_addresses must be nonzero",
        ));
    }
    Ok(())
}

fn declared_edge_count(terminator: &BlockTerminator) -> Option<usize> {
    match terminator {
        BlockTerminator::Fallthrough { .. } | BlockTerminator::Tail { .. } => Some(1),
        BlockTerminator::Call { .. }
        | BlockTerminator::Branch { .. }
        | BlockTerminator::BranchLikely { .. } => Some(2),
        BlockTerminator::Indirect { via_call: true } => Some(1),
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: false,
        } => Some(targets.len()),
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: true,
        } => targets.len().checked_add(1),
        BlockTerminator::Return
        | BlockTerminator::Indirect { via_call: false }
        | BlockTerminator::Trap
        | BlockTerminator::InvalidInstruction { .. }
        | BlockTerminator::MissingDelaySlot { .. }
        | BlockTerminator::RanOffEnd
        | BlockTerminator::DataFence { .. } => Some(0),
    }
}

fn describe_block(
    block: &BasicBlock,
    starts: &BTreeMap<u32, usize>,
) -> (LocalShape, Vec<StructuralEdge>) {
    let target = |address: u32| {
        starts
            .get(&address)
            .copied()
            .map_or(EdgeTarget::External, EdgeTarget::Internal)
    };
    let edge = |kind, address| StructuralEdge {
        kind,
        target: target(address),
    };

    let (terminator_tag, has_delay_slot, annuls, resolved_target_count, mut edges) =
        match &block.terminator {
            BlockTerminator::Fallthrough { next } => {
                (1, false, false, 0, vec![edge(EdgeKind::Fallthrough, *next)])
            }
            BlockTerminator::Tail { target } => {
                (2, true, false, 0, vec![edge(EdgeKind::Tail, *target)])
            }
            BlockTerminator::Call { target, next } => (
                3,
                true,
                false,
                0,
                vec![
                    edge(EdgeKind::CallTarget, *target),
                    edge(EdgeKind::CallContinuation, *next),
                ],
            ),
            BlockTerminator::Branch {
                target,
                fallthrough,
                ..
            } => (
                4,
                true,
                false,
                0,
                vec![
                    edge(EdgeKind::BranchTaken, *target),
                    edge(EdgeKind::BranchFallthrough, *fallthrough),
                ],
            ),
            BlockTerminator::BranchLikely {
                target,
                fallthrough,
                ..
            } => (
                5,
                true,
                true,
                0,
                vec![
                    edge(EdgeKind::LikelyTaken, *target),
                    edge(EdgeKind::LikelyAnnulledFallthrough, *fallthrough),
                ],
            ),
            BlockTerminator::Return => (6, true, false, 0, Vec::new()),
            BlockTerminator::Indirect { via_call: false } => (7, true, false, 0, Vec::new()),
            BlockTerminator::Indirect { via_call: true } => (
                8,
                true,
                false,
                0,
                vec![edge(EdgeKind::IndirectCallContinuation, block.end_va)],
            ),
            BlockTerminator::ResolvedIndirect {
                targets,
                via_call: false,
            } => (
                9,
                true,
                false,
                targets.len().try_into().unwrap_or(u32::MAX),
                targets
                    .iter()
                    .map(|&address| edge(EdgeKind::IndirectJumpTarget, address))
                    .collect(),
            ),
            BlockTerminator::ResolvedIndirect {
                targets,
                via_call: true,
            } => {
                let mut edges: Vec<_> = targets
                    .iter()
                    .map(|&address| edge(EdgeKind::IndirectCallTarget, address))
                    .collect();
                edges.push(edge(EdgeKind::IndirectCallContinuation, block.end_va));
                (
                    10,
                    true,
                    false,
                    targets.len().try_into().unwrap_or(u32::MAX),
                    edges,
                )
            }
            BlockTerminator::Trap => (11, false, false, 0, Vec::new()),
            BlockTerminator::RanOffEnd => (12, false, false, 0, Vec::new()),
            BlockTerminator::DataFence { .. } => (13, false, false, 0, Vec::new()),
            BlockTerminator::InvalidInstruction { .. } => (13, false, false, 0, Vec::new()),
            BlockTerminator::MissingDelaySlot { .. } => (14, false, false, 0, Vec::new()),
        };
    edges.sort_unstable();
    edges.dedup();
    (
        LocalShape {
            word_count: 0,
            terminator_tag,
            has_delay_slot,
            annuls_delay_slot_on_fallthrough: annuls,
            resolved_target_count,
        },
        edges,
    )
}

fn hash_local(shape: &LocalShape) -> StructuralFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.cfg-homology.local.v1\0");
    hasher.update(shape.word_count.to_le_bytes());
    hasher.update([shape.terminator_tag]);
    hasher.update([u8::from(shape.has_delay_slot)]);
    hasher.update([u8::from(shape.annuls_delay_slot_on_fallthrough)]);
    hasher.update(shape.resolved_target_count.to_le_bytes());
    StructuralFingerprint(hasher.finalize().into())
}

fn hash_refined(
    round: usize,
    shape: LocalShape,
    previous: StructuralFingerprint,
    edges: &[StructuralEdge],
    labels: &[StructuralFingerprint],
) -> StructuralFingerprint {
    let mut neighborhood: Vec<(u8, u8, StructuralFingerprint)> = edges
        .iter()
        .map(|edge| match edge.target {
            EdgeTarget::Internal(index) => (edge.kind as u8, 0, labels[index]),
            EdgeTarget::External => (edge.kind as u8, 1, external_fingerprint(edge.kind)),
        })
        .collect();
    neighborhood.sort_unstable();

    let mut hasher = Sha256::new();
    hasher.update(b"fn64.cfg-homology.refine.v1\0");
    hasher.update((round as u32).to_le_bytes());
    hasher.update(hash_local(&shape).0);
    hasher.update(previous.0);
    hasher.update((neighborhood.len() as u64).to_le_bytes());
    for (kind, external, label) in neighborhood {
        hasher.update([kind, external]);
        hasher.update(label.0);
    }
    StructuralFingerprint(hasher.finalize().into())
}

fn external_fingerprint(kind: EdgeKind) -> StructuralFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.cfg-homology.external.v1\0");
    hasher.update([kind as u8]);
    StructuralFingerprint(hasher.finalize().into())
}

fn hash_graph(
    labels: &[StructuralFingerprint],
    edges: &[Vec<StructuralEdge>],
) -> StructuralFingerprint {
    let mut nodes = labels.to_vec();
    nodes.sort_unstable();
    let mut graph_edges = Vec::new();
    for (source, outgoing) in edges.iter().enumerate() {
        for edge in outgoing {
            let (external, target) = match edge.target {
                EdgeTarget::Internal(index) => (0, labels[index]),
                EdgeTarget::External => (1, external_fingerprint(edge.kind)),
            };
            graph_edges.push((labels[source], edge.kind as u8, external, target));
        }
    }
    graph_edges.sort_unstable();

    let mut hasher = Sha256::new();
    hasher.update(b"fn64.cfg-homology.graph.v1\0");
    hasher.update((nodes.len() as u64).to_le_bytes());
    for node in nodes {
        hasher.update(node.0);
    }
    hasher.update((graph_edges.len() as u64).to_le_bytes());
    for (source, kind, external, target) in graph_edges {
        hasher.update(source.0);
        hasher.update([kind, external]);
        hasher.update(target.0);
    }
    StructuralFingerprint(hasher.finalize().into())
}

fn buckets(blocks: &[FingerprintedBlock]) -> BTreeMap<StructuralFingerprint, Vec<u32>> {
    let mut out: BTreeMap<StructuralFingerprint, Vec<u32>> = BTreeMap::new();
    for block in blocks {
        out.entry(block.fingerprint)
            .or_default()
            .push(block.start_va);
    }
    for starts in out.values_mut() {
        starts.sort_unstable();
    }
    out
}

fn bounded_addresses(addresses: &[u32], limit: usize) -> (Vec<u32>, bool) {
    (
        addresses.iter().take(limit).copied().collect(),
        addresses.len() > limit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{BasicBlock, BlockTerminator, Cfg};
    use std::collections::BTreeMap;

    fn cfg(blocks: Vec<BasicBlock>) -> Cfg {
        Cfg {
            bank: "synthetic".to_string(),
            word_class: BTreeMap::new(),
            blocks,
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            proven_roots: Vec::new(),
        }
    }

    fn call_return_graph(base: u32) -> Cfg {
        cfg(vec![
            BasicBlock {
                start_va: base,
                end_va: base + 8,
                terminator: BlockTerminator::Call {
                    target: base + 0x20,
                    next: base + 8,
                },
            },
            BasicBlock {
                start_va: base + 8,
                end_va: base + 0x10,
                terminator: BlockTerminator::Return,
            },
            BasicBlock {
                start_va: base + 0x20,
                end_va: base + 0x24,
                terminator: BlockTerminator::Trap,
            },
        ])
    }

    #[test]
    fn shifted_addresses_and_bank_names_match_one_to_one() {
        let query = call_return_graph(0x8000_0000);
        let mut reference = call_return_graph(0x8012_3000);
        reference.bank = "different-name".to_string();
        let report = match_cfgs(&query, &reference, HomologyLimits::default()).unwrap();
        assert_eq!(report.candidates.len(), 3);
        assert!(report.ambiguous.is_empty());
        assert!(report.unmatched.is_empty());
        assert_eq!(
            report.query_graph_fingerprint,
            report.reference_graph_fingerprint
        );
    }

    #[test]
    fn identical_leaf_shapes_are_explicitly_ambiguous() {
        let leaves = |base| {
            cfg(vec![
                BasicBlock {
                    start_va: base,
                    end_va: base + 8,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: base + 0x20,
                    end_va: base + 0x28,
                    terminator: BlockTerminator::Return,
                },
            ])
        };
        let report = match_cfgs(
            &leaves(0x8000_0000),
            &leaves(0x8010_0000),
            HomologyLimits::default(),
        )
        .unwrap();
        assert!(report.candidates.is_empty());
        assert!(report.unmatched.is_empty());
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(report.ambiguous[0].total_query_blocks, 2);
        assert_eq!(report.ambiguous[0].total_reference_blocks, 2);
    }

    #[test]
    fn absent_structural_peers_are_unmatched_on_both_sides() {
        let query = cfg(vec![BasicBlock {
            start_va: 0x8000_0000,
            end_va: 0x8000_0008,
            terminator: BlockTerminator::Return,
        }]);
        let reference = cfg(vec![BasicBlock {
            start_va: 0x8010_0000,
            end_va: 0x8010_0004,
            terminator: BlockTerminator::Trap,
        }]);
        let report = match_cfgs(&query, &reference, HomologyLimits::default()).unwrap();
        assert!(report.candidates.is_empty());
        assert!(report.ambiguous.is_empty());
        assert_eq!(report.unmatched.len(), 2);
        assert!(report
            .unmatched
            .iter()
            .any(|block| block.side == HomologySide::Query));
        assert!(report
            .unmatched
            .iter()
            .any(|block| block.side == HomologySide::Reference));
    }

    #[test]
    fn block_and_resolved_target_order_do_not_change_fingerprints() {
        let block_a = BasicBlock {
            start_va: 0x8000_0000,
            end_va: 0x8000_0008,
            terminator: BlockTerminator::ResolvedIndirect {
                targets: vec![0x8000_0040, 0x8000_0020],
                via_call: false,
            },
        };
        let block_b = BasicBlock {
            start_va: 0x8000_0020,
            end_va: 0x8000_0028,
            terminator: BlockTerminator::Return,
        };
        let block_c = BasicBlock {
            start_va: 0x8000_0040,
            end_va: 0x8000_0044,
            terminator: BlockTerminator::Trap,
        };
        let first = cfg(vec![block_a.clone(), block_b.clone(), block_c.clone()]);
        let mut reordered_a = block_a;
        reordered_a.terminator = BlockTerminator::ResolvedIndirect {
            targets: vec![0x8000_0020, 0x8000_0040],
            via_call: false,
        };
        let second = cfg(vec![block_c, reordered_a, block_b]);
        assert_eq!(
            fingerprint_cfg(&first, HomologySide::Query, HomologyLimits::default()).unwrap(),
            fingerprint_cfg(&second, HomologySide::Query, HomologyLimits::default()).unwrap()
        );
    }

    #[test]
    fn branch_likely_annulment_is_not_collapsed_into_plain_branch() {
        let graph = |likely| {
            cfg(vec![
                BasicBlock {
                    start_va: 0x8000_0000,
                    end_va: 0x8000_0008,
                    terminator: if likely {
                        BlockTerminator::BranchLikely {
                            target: 0x8000_0020,
                            fallthrough: 0x8000_0008,
                            link: false,
                        }
                    } else {
                        BlockTerminator::Branch {
                            target: 0x8000_0020,
                            fallthrough: 0x8000_0008,
                            link: false,
                        }
                    },
                },
                BasicBlock {
                    start_va: 0x8000_0008,
                    end_va: 0x8000_0010,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: 0x8000_0020,
                    end_va: 0x8000_0024,
                    terminator: BlockTerminator::Trap,
                },
            ])
        };
        let ordinary = fingerprint_cfg(
            &graph(false),
            HomologySide::Query,
            HomologyLimits::default(),
        )
        .unwrap();
        let likely = fingerprint_cfg(
            &graph(true),
            HomologySide::Reference,
            HomologyLimits::default(),
        )
        .unwrap();
        assert_ne!(ordinary.graph_fingerprint, likely.graph_fingerprint);
    }

    #[test]
    fn comparison_is_deterministic_across_repeated_runs() {
        let query = call_return_graph(0x8000_0000);
        let reference = call_return_graph(0x8020_0000);
        let expected = match_cfgs(&query, &reference, HomologyLimits::default()).unwrap();
        for _ in 0..20 {
            assert_eq!(
                match_cfgs(&query, &reference, HomologyLimits::default()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn limits_fail_loudly_before_unbounded_work() {
        let graph = call_return_graph(0x8000_0000);
        let limits = HomologyLimits {
            max_blocks: 2,
            ..HomologyLimits::default()
        };
        assert!(matches!(
            fingerprint_cfg(&graph, HomologySide::Query, limits),
            Err(HomologyError::TooManyBlocks { count: 3, .. })
        ));

        let limits = HomologyLimits {
            max_edges: 1,
            ..HomologyLimits::default()
        };
        assert!(matches!(
            fingerprint_cfg(&graph, HomologySide::Query, limits),
            Err(HomologyError::TooManyEdges { .. })
        ));
    }

    #[test]
    fn ambiguity_address_lists_are_bounded_but_totals_remain_exact() {
        let leaves = |base| {
            cfg((0u32..5)
                .map(|index| BasicBlock {
                    start_va: base + index * 0x20,
                    end_va: base + index * 0x20 + 8,
                    terminator: BlockTerminator::Return,
                })
                .collect())
        };
        let limits = HomologyLimits {
            max_ambiguous_addresses: 2,
            ..HomologyLimits::default()
        };
        let report = match_cfgs(&leaves(0x8000_0000), &leaves(0x8010_0000), limits).unwrap();
        assert_eq!(report.ambiguous[0].query_start_vas.len(), 2);
        assert_eq!(report.ambiguous[0].total_query_blocks, 5);
        assert!(report.ambiguous[0].query_addresses_truncated);
        assert!(report.ambiguous[0].reference_addresses_truncated);
    }
}
