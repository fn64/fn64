//! Function-boundary-independent proof for reachable executable blocks.
//!
//! This does not weaken [`crate::owner_proof::ExactFunctionOwner`]. It
//! exposes the smaller fact needed by `block_aot`: bytes reached from an
//! authoritative entry through the closed CFG, accepted by the shared
//! decoder, and bound to exactly one proven bank-image span. That span stays
//! typed: evaluated output offsets are never represented as ROM coordinates.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg, WordClass};
use crate::facts::{
    executable_range_subject, BankAddr, BankBackingSpanResolutionV1, BankBackingSpanV1, Fact,
    FactDb, ProofState,
};
use crate::owner_proof::{OwnerAssessment, OwnerBlocker, OwnerProofReport};
use crate::partition::{partition, Partition};
use crate::resolve::ClosureResult;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockProofBlocker {
    AnalysisBankMismatch,
    Unowned,
    NoAuthoritativeReachability { roots: Vec<u32> },
    EntryNotAuthoritative { root: u32 },
    InvalidGeometry,
    WordNotProvenCode { pc: u32, class: Option<WordClass> },
    InvalidInstruction { pc: u32, word: u32 },
    MissingDelaySlot { control_pc: u32 },
    RanOffEnd,
    MissingBankBacking,
    AmbiguousBankBacking,
    InvalidBankBackingGeometry,
}

impl BlockProofBlocker {
    /// The blocker's kind, without its per-site payload, for histogramming
    /// why block proof refused across a whole program.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AnalysisBankMismatch => "analysis_bank_mismatch",
            Self::Unowned => "unowned",
            Self::NoAuthoritativeReachability { .. } => "no_authoritative_reachability",
            Self::EntryNotAuthoritative { .. } => "entry_not_authoritative",
            Self::InvalidGeometry => "invalid_geometry",
            Self::WordNotProvenCode { .. } => "word_not_proven_code",
            Self::InvalidInstruction { .. } => "invalid_instruction",
            Self::MissingDelaySlot { .. } => "missing_delay_slot",
            Self::RanOffEnd => "ran_off_end",
            Self::MissingBankBacking => "missing_bank_backing",
            Self::AmbiguousBankBacking => "ambiguous_bank_backing",
            Self::InvalidBankBackingGeometry => "invalid_bank_backing_geometry",
        }
    }
}

/// Count block-proof refusals by blocker kind across every bank of every
/// composed snapshot.
///
/// One candidate block can carry several blockers; each is counted, so the
/// total exceeds the refused-block count. That is the useful shape: the
/// question this answers is "which refusal reasons dominate", not "how many
/// blocks were refused" (which `proven_blocks` already bounds).
pub fn blocker_histogram(
    snapshots: &[crate::snapshot::ProgramSnapshotV1],
) -> BTreeMap<&'static str, u64> {
    let mut histogram = BTreeMap::new();
    for snapshot in snapshots {
        for bank in &snapshot.banks {
            for assessment in &bank.block_proof.assessments {
                if let BlockAssessment::Candidate { blockers, .. } = assessment {
                    for blocker in blockers {
                        *histogram.entry(blocker.kind()).or_default() += 1;
                    }
                }
            }
        }
    }
    histogram
}

/// Canonical nonempty roots whose independently authoritative CFG closures
/// reach one block.
///
/// Function ownership is deliberately absent: two callable roots may share a
/// block without weakening the proof that those exact bytes are executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AuthoritativeReachabilityRoots(Vec<u32>);

impl AuthoritativeReachabilityRoots {
    pub(crate) fn new(roots: impl IntoIterator<Item = u32>) -> Option<Self> {
        let mut roots = roots.into_iter().collect::<Vec<_>>();
        roots.sort_unstable();
        roots.dedup();
        (!roots.is_empty()).then_some(Self(roots))
    }

    pub fn as_slice(&self) -> &[u32] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthoritativeReachabilityRoots {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let roots = Vec::<u32>::deserialize(deserializer)?;
        if roots.is_empty() || roots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(serde::de::Error::custom(
                "authoritative reachability roots must be nonempty, sorted, and unique",
            ));
        }
        Ok(Self(roots))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReachableCodeBlock {
    pub bank: String,
    pub start_va: u32,
    pub end_va: u32,
    pub authoritative_roots: AuthoritativeReachabilityRoots,
    pub backing: BankBackingSpanV1,
    pub terminator: BlockTerminator,
}

/// A proven-code block in a proven-installed bank image, carrying no
/// authoritative reachability claim.
///
/// Structurally [`ReachableCodeBlock`] minus `authoritative_roots` -- the
/// field is absent rather than empty so the weaker claim cannot be mistaken
/// for the stronger one by a consumer that forgets to check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledCodeBlock {
    pub bank: String,
    pub start_va: u32,
    pub end_va: u32,
    pub backing: BankBackingSpanV1,
    pub terminator: BlockTerminator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BlockAssessment {
    Proven {
        block: ReachableCodeBlock,
    },
    /// Every requirement of [`Self::Proven`] except authoritative
    /// reachability: the words are `ProvenCode`, the terminator is sound, and
    /// exactly one proven bank-image subspan backs the bytes -- but no
    /// independently authoritative root reaches the block.
    ///
    /// This is a strictly weaker claim than `block_aot` and is deliberately a
    /// separate variant rather than a relaxation of it. It exists for banks
    /// whose image is installed by a proven activation (an overlay DMA with
    /// exact recovered geometry) and whose entry point is therefore never
    /// named by any proven fact: the hardware installs the image, so the bytes
    /// are genuinely resident code, but nothing proves where execution enters
    /// them. Admitting these under `Proven` would silently redefine every
    /// historical `block_aot` figure; keeping them separate preserves that
    /// meaning while still allowing emission.
    Installed {
        block: InstalledCodeBlock,
        /// Retained so the missing evidence stays visible in reports rather
        /// than being erased by admission.
        blockers: Vec<BlockProofBlocker>,
    },
    Candidate {
        start_va: u32,
        end_va: u32,
        blockers: Vec<BlockProofBlocker>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockProofReport {
    pub bank: String,
    pub assessments: Vec<BlockAssessment>,
    pub proven_blocks: u64,
    pub proven_bytes: u64,
    /// Blocks admitted as [`BlockAssessment::Installed`]: proven-installed and
    /// decoding as code, but with no authoritative reachability. Counted
    /// separately so a consumer can never fold the weaker claim into
    /// `proven_blocks`.
    #[serde(default)]
    pub installed_blocks: u64,
    #[serde(default)]
    pub installed_bytes: u64,
}

#[derive(Clone)]
struct AuthorityBlockReachability {
    start_va: u32,
    end_va: u32,
    delay_slot_va: Option<u32>,
    roots: AuthoritativeReachabilityRoots,
}

fn authority_delay_slot_va(block: &BasicBlock) -> Option<u32> {
    match block.terminator {
        BlockTerminator::Tail { .. }
        | BlockTerminator::Call { .. }
        | BlockTerminator::Branch { .. }
        | BlockTerminator::BranchLikely { .. }
        | BlockTerminator::Return
        | BlockTerminator::Indirect { .. }
        | BlockTerminator::ResolvedIndirect { .. } => block.end_va.checked_sub(4),
        BlockTerminator::Fallthrough { .. }
        | BlockTerminator::Trap
        | BlockTerminator::InvalidInstruction { .. }
        | BlockTerminator::MissingDelaySlot { .. }
        | BlockTerminator::RanOffEnd
        | BlockTerminator::DataFence { .. } => None,
    }
}

/// Crate-private projection of per-block roots from the authority-only CFG.
///
/// The only constructor partitions the supplied authority closure itself.
/// Broad traversal roots never enter this type. A broad block may consume a
/// record only when one authority block fully contains its exact extent.
pub(crate) struct AuthorityReachabilityProjection {
    bank: String,
    blocks: Vec<AuthorityBlockReachability>,
}

impl AuthorityReachabilityProjection {
    pub(crate) fn from_authority_closure(authority_closure: &ClosureResult) -> Self {
        let cfg = &authority_closure.cfg;
        let authority_partition = partition(cfg);
        let owner_of: BTreeMap<u32, u32> = authority_partition
            .owners
            .iter()
            .flat_map(|owner| {
                owner
                    .block_starts
                    .iter()
                    .map(move |&start| (start, owner.root_va))
            })
            .collect();
        let ambiguous: BTreeMap<u32, Vec<u32>> = authority_partition
            .ambiguous
            .iter()
            .map(|block| (block.block_start, block.claimants.clone()))
            .collect();
        let mut blocks = cfg
            .blocks
            .iter()
            .filter_map(|block| {
                let roots = ambiguous.get(&block.start_va).cloned().or_else(|| {
                    owner_of
                        .get(&block.start_va)
                        .copied()
                        .map(|root| vec![root])
                })?;
                Some(AuthorityBlockReachability {
                    start_va: block.start_va,
                    end_va: block.end_va,
                    delay_slot_va: authority_delay_slot_va(block),
                    roots: AuthoritativeReachabilityRoots::new(roots)
                        .expect("authority partition claimants are nonempty"),
                })
            })
            .collect::<Vec<_>>();
        blocks.sort_by_key(|block| (block.start_va, block.end_va));
        Self {
            bank: cfg.bank.clone(),
            blocks,
        }
    }

    fn roots_for(&self, block: &BasicBlock) -> Option<&AuthoritativeReachabilityRoots> {
        let mut containing = self.blocks.iter().filter(|authority| {
            authority.start_va <= block.start_va
                && block.end_va <= authority.end_va
                && authority.delay_slot_va != Some(block.start_va)
                && authority.delay_slot_va != Some(block.end_va)
        });
        let only = containing.next()?;
        containing.next().is_none().then_some(&only.roots)
    }
}

pub fn prove_reachable_blocks(
    cfg: &Cfg,
    partition: &Partition,
    owners: &OwnerProofReport,
    facts: &FactDb,
) -> BlockProofReport {
    prove_reachable_blocks_inner(cfg, partition, owners, facts, None)
}

pub(crate) fn prove_reachable_blocks_with_authority_projection(
    cfg: &Cfg,
    partition: &Partition,
    owners: &OwnerProofReport,
    facts: &FactDb,
    authority: &AuthorityReachabilityProjection,
) -> BlockProofReport {
    prove_reachable_blocks_inner(cfg, partition, owners, facts, Some(authority))
}

fn prove_reachable_blocks_inner(
    cfg: &Cfg,
    partition: &Partition,
    owners: &OwnerProofReport,
    facts: &FactDb,
    authority: Option<&AuthorityReachabilityProjection>,
) -> BlockProofReport {
    let owner_of: BTreeMap<u32, u32> = partition
        .owners
        .iter()
        .flat_map(|owner| {
            owner
                .block_starts
                .iter()
                .map(move |&start| (start, owner.root_va))
        })
        .collect();
    let ambiguous: BTreeMap<u32, Vec<u32>> = partition
        .ambiguous
        .iter()
        .map(|block| (block.block_start, block.claimants.clone()))
        .collect();
    let authoritative: BTreeMap<u32, bool> = owners
        .assessments
        .iter()
        .map(|assessment| {
            let root = assessment.entry().pc;
            let blocked = match assessment {
                OwnerAssessment::Proven { .. } => false,
                OwnerAssessment::Candidate { frontier }
                | OwnerAssessment::Ambiguous { frontier } => frontier
                    .blockers
                    .contains(&OwnerBlocker::EntryNotAuthoritative),
            };
            (root, !blocked)
        })
        .collect();

    let mut assessments = Vec::with_capacity(cfg.blocks.len());
    let mut proven_blocks = 0;
    let mut proven_bytes = 0;
    let mut installed_blocks = 0;
    let mut installed_bytes = 0;
    // A bank whose image a proven activation installs. Entry authority for
    // such a bank is never established by any proven fact -- an overlay
    // descriptor names where the image is placed, not where it is entered
    // (measured: VPW2 overlay entry offsets are 0, 7, and 12 words past the
    // load address, so no structural rule recovers them).
    let bank_is_installed = !facts
        .proven_activation_load_addresses(&cfg.bank)
        .is_empty();
    for block in &cfg.blocks {
        let mut blockers = BTreeSet::new();
        if partition.bank != cfg.bank
            || owners.bank != cfg.bank
            || authority.is_some_and(|projection| projection.bank != cfg.bank)
        {
            blockers.insert(BlockProofBlocker::AnalysisBankMismatch);
        }
        let mut claimant_roots = ambiguous
            .get(&block.start_va)
            .cloned()
            .or_else(|| {
                owner_of
                    .get(&block.start_va)
                    .copied()
                    .map(|root| vec![root])
            })
            .unwrap_or_default();
        claimant_roots.sort_unstable();
        claimant_roots.dedup();
        let reached_roots = match authority {
            Some(projection) => projection
                .roots_for(block)
                .map(|roots| roots.as_slice().to_vec())
                .unwrap_or_default(),
            None => claimant_roots
                .iter()
                .copied()
                .filter(|root| authoritative.get(root).copied().unwrap_or(false))
                .collect(),
        };
        let authoritative_roots = AuthoritativeReachabilityRoots::new(reached_roots);
        if claimant_roots.is_empty() && authoritative_roots.is_none() {
            blockers.insert(BlockProofBlocker::Unowned);
        }
        if authoritative_roots.is_none() && !claimant_roots.is_empty() {
            if let [root] = claimant_roots.as_slice() {
                blockers.insert(BlockProofBlocker::EntryNotAuthoritative { root: *root });
            } else {
                blockers.insert(BlockProofBlocker::NoAuthoritativeReachability {
                    roots: claimant_roots,
                });
            }
        }
        validate_block(block, cfg, &mut blockers);
        let backing =
            match facts.resolve_proven_bank_backing_span(&cfg.bank, block.start_va, block.end_va) {
                BankBackingSpanResolutionV1::Missing => {
                    blockers.insert(BlockProofBlocker::MissingBankBacking);
                    None
                }
                BankBackingSpanResolutionV1::Unique(backing) => Some(backing),
                BankBackingSpanResolutionV1::Ambiguous => {
                    blockers.insert(BlockProofBlocker::AmbiguousBankBacking);
                    None
                }
                BankBackingSpanResolutionV1::InvalidGeometry => {
                    blockers.insert(BlockProofBlocker::InvalidBankBackingGeometry);
                    None
                }
            };
        if blockers.is_empty() {
            let backing = backing.expect("one backing when no blocker remains");
            let authoritative_roots = authoritative_roots
                .expect("nonempty authoritative reachability when no blocker remains");
            proven_blocks += 1;
            proven_bytes += u64::from(block.end_va - block.start_va);
            assessments.push(BlockAssessment::Proven {
                block: ReachableCodeBlock {
                    bank: cfg.bank.clone(),
                    start_va: block.start_va,
                    end_va: block.end_va,
                    authoritative_roots,
                    backing,
                    terminator: block.terminator.clone(),
                },
            });
        } else if bank_is_installed && only_reachability_blockers(&blockers) {
            // Reachability is the ONLY thing missing, and the bank's image is
            // installed by a proven activation. Every byte-level requirement
            // -- ProvenCode words, sound terminator, unique proven backing --
            // already held above, so the block is genuinely resident code
            // whose entry point no proven fact names. Admitting it here keeps
            // that distinction explicit instead of promoting it to Proven.
            let backing = backing.expect("one backing when only reachability blocks");
            installed_blocks += 1;
            installed_bytes += u64::from(block.end_va - block.start_va);
            assessments.push(BlockAssessment::Installed {
                block: InstalledCodeBlock {
                    bank: cfg.bank.clone(),
                    start_va: block.start_va,
                    end_va: block.end_va,
                    backing,
                    terminator: block.terminator.clone(),
                },
                blockers: blockers.into_iter().collect(),
            });
        } else {
            assessments.push(BlockAssessment::Candidate {
                start_va: block.start_va,
                end_va: block.end_va,
                blockers: blockers.into_iter().collect(),
            });
        }
    }
    BlockProofReport {
        bank: cfg.bank.clone(),
        assessments,
        proven_blocks,
        proven_bytes,
        installed_blocks,
        installed_bytes,
    }
}

/// Whether every blocker is a missing-reachability finding.
///
/// These three are the only blockers a proven installation may waive: they all
/// say "no authoritative root reaches this block", never "these bytes are not
/// code". Any byte-level blocker -- `WordNotProvenCode`, `InvalidInstruction`,
/// `MissingDelaySlot`, `RanOffEnd`, or any backing failure -- keeps the block a
/// candidate, because installation says nothing about whether the words decode.
fn only_reachability_blockers(blockers: &BTreeSet<BlockProofBlocker>) -> bool {
    !blockers.is_empty()
        && blockers.iter().all(|blocker| {
            matches!(
                blocker,
                BlockProofBlocker::Unowned
                    | BlockProofBlocker::EntryNotAuthoritative { .. }
                    | BlockProofBlocker::NoAuthoritativeReachability { .. }
            )
        })
}

/// One executable interval derived from the union of reached proven-code
/// block geometry, with the conclusion state this pass recorded for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedExecutableRange {
    pub va_start: u32,
    pub va_end: u32,
    pub state: ProofState,
}

/// Convert proven reached-code blocks into typed, evidence-carrying
/// [`Fact::ExecutableRange`] facts for the report's bank.
///
/// Soundness boundary: a word reached by CFG closure from an authoritative
/// entry is demonstrably executed under the proven mapping, so the union of
/// proven block intervals is proven executable. Exactly those bytes are
/// claimed: adjacent/overlapping proven blocks merge into one interval, but
/// a gap between reached blocks is never bridged — unreached bytes stay
/// unproven. Region scores and content statistics play no role here; a
/// score-threshold promotion rule was measured and rejected
/// (docs/DISCOVER-PLAN.md).
///
/// A subject already `Rejected`/`Conflict` is never silently promoted: the
/// new reachability evidence is recorded and the conclusion moves to (or
/// stays) `Conflict`, preserving the disagreement instead of overwriting it.
pub fn conclude_reached_executable_ranges(
    report: &BlockProofReport,
    facts: &mut FactDb,
) -> Vec<DerivedExecutableRange> {
    let mut intervals: Vec<(u32, u32)> = report
        .assessments
        .iter()
        .filter_map(|assessment| match assessment {
            BlockAssessment::Proven { block } => Some((block.start_va, block.end_va)),
            // Installed blocks are deliberately excluded. This function
            // concludes `ExecutableRange` FACTS, which downstream passes read
            // as authoritative reachability; admitting a block that carries no
            // reachability claim would launder the weaker claim into the fact
            // database and let it seed further proofs.
            BlockAssessment::Installed { .. } | BlockAssessment::Candidate { .. } => None,
        })
        .collect();
    intervals.sort_unstable();

    let mut merged: Vec<(u32, u32, u64)> = Vec::new();
    for (start, end) in intervals {
        match merged.last_mut() {
            Some((_, merged_end, blocks)) if start <= *merged_end => {
                *merged_end = (*merged_end).max(end);
                *blocks += 1;
            }
            _ => merged.push((start, end, 1)),
        }
    }

    merged
        .into_iter()
        .map(|(va_start, va_end, blocks)| {
            let subject = executable_range_subject(&report.bank, va_start, va_end);
            let contradicted = matches!(
                facts.conclusion(&subject).map(|conclusion| conclusion.state),
                Some(ProofState::Rejected | ProofState::Conflict)
            );
            let range = facts.insert(Fact::ExecutableRange {
                bank: report.bank.clone(),
                va_start,
                va_end,
            });
            let evidence = facts.insert(Fact::Evidence {
                subject: BankAddr::new(&report.bank, va_start),
                note: format!(
                    "executable [0x{va_start:08x},0x{va_end:08x}): union of {blocks} reached proven-code block(s) from authoritative-entry CFG closure"
                ),
            });
            let (state, rule) = if contradicted {
                (
                    ProofState::Conflict,
                    "reached_proven_code_closure_contradicted",
                )
            } else {
                (ProofState::Proven, "reached_proven_code_closure")
            };
            facts
                .conclude(subject, state, vec![range, evidence], rule)
                .expect("reached-code executable conclusions are monotonic by construction");
            DerivedExecutableRange {
                va_start,
                va_end,
                state,
            }
        })
        .collect()
}

fn validate_block(block: &BasicBlock, cfg: &Cfg, blockers: &mut BTreeSet<BlockProofBlocker>) {
    if !block.start_va.is_multiple_of(4)
        || !block.end_va.is_multiple_of(4)
        || block.end_va <= block.start_va
    {
        blockers.insert(BlockProofBlocker::InvalidGeometry);
        return;
    }
    for pc in (block.start_va..block.end_va).step_by(4) {
        let class = cfg.word_class.get(&pc).copied();
        if class != Some(WordClass::ProvenCode) {
            blockers.insert(BlockProofBlocker::WordNotProvenCode { pc, class });
        }
    }
    match block.terminator {
        BlockTerminator::InvalidInstruction { pc, word } => {
            blockers.insert(BlockProofBlocker::InvalidInstruction { pc, word });
        }
        BlockTerminator::MissingDelaySlot { control_pc } => {
            blockers.insert(BlockProofBlocker::MissingDelaySlot { control_pc });
        }
        BlockTerminator::RanOffEnd | BlockTerminator::DataFence { .. } => {
            blockers.insert(BlockProofBlocker::RanOffEnd);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        evaluated_image_receipt_sha256_v1, EvaluatedImageReceiptV1, MaterializationEvaluatorV1,
        MaterializedByteRangeV1, MaterializedImageSourceV1, MaterializedImageStreamV1,
        MaterializedImageSuffixV1, RomAddressSpace,
    };
    use crate::owner_proof::OwnerFrontier;
    use crate::partition::{AmbiguousBlock, Owner, Partition};

    const BASE: u32 = 0x8000_0000;

    fn proven_block(start_va: u32, end_va: u32) -> BlockAssessment {
        BlockAssessment::Proven {
            block: ReachableCodeBlock {
                bank: "bank".into(),
                start_va,
                end_va,
                authoritative_roots: AuthoritativeReachabilityRoots::new([BASE]).unwrap(),
                backing: BankBackingSpanV1::RomAffine {
                    rom_space: RomAddressSpace::Physical,
                    rom_start: 0x1000 + (start_va - BASE),
                    rom_end: 0x1000 + (end_va - BASE),
                },
                terminator: BlockTerminator::Return,
            },
        }
    }

    fn report_of(assessments: Vec<BlockAssessment>) -> BlockProofReport {
        let proven_blocks = assessments
            .iter()
            .filter(|assessment| matches!(assessment, BlockAssessment::Proven { .. }))
            .count() as u64;
        BlockProofReport {
            bank: "bank".into(),
            assessments,
            proven_blocks,
            proven_bytes: 0,
            installed_blocks: 0,
            installed_bytes: 0,
        }
    }

    fn one_block_cfg() -> Cfg {
        Cfg {
            bank: "bank".into(),
            word_class: [
                (BASE, WordClass::ProvenCode),
                (BASE + 4, WordClass::ProvenCode),
            ]
            .into_iter()
            .collect(),
            blocks: vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            proven_roots: Vec::new(),
        }
    }

    fn mapped_facts() -> FactDb {
        mapped_facts_len(8)
    }

    fn mapped_facts_len(byte_len: u32) -> FactDb {
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1000 + byte_len,
            va_start: BASE,
            va_end: BASE + byte_len,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
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
            streams: vec![MaterializedImageStreamV1 {
                source_range: MaterializedByteRangeV1 { start: 4, end: 32 },
                encoded_range: MaterializedByteRangeV1 { start: 10, end: 32 },
                output_range: MaterializedByteRangeV1 {
                    start: 0,
                    end: output_len,
                },
                declared_output_len: output_len,
                source_sha256: "33".repeat(32),
                output_sha256: "22".repeat(32),
            }],
            trailing_suffix: MaterializedImageSuffixV1 {
                offset: 32,
                len: 32,
                sha256: "44".repeat(32),
            },
        }
    }

    fn materialized_facts(output_len: u32, va_len: u32) -> (FactDb, String) {
        let mut facts = FactDb::new();
        let receipt = evaluated_receipt(output_len);
        let digest = evaluated_image_receipt_sha256_v1(&receipt);
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "bank".into(),
            va_start: BASE,
            va_end: BASE + va_len,
            receipt,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![image],
                "test_materialized_image",
            )
            .unwrap();
        (facts, digest)
    }

    fn cfg_from_blocks(blocks: Vec<BasicBlock>, roots: Vec<u32>) -> Cfg {
        let word_class = blocks
            .iter()
            .flat_map(|block| (block.start_va..block.end_va).step_by(4))
            .map(|pc| (pc, WordClass::ProvenCode))
            .collect();
        Cfg {
            bank: "bank".into(),
            word_class,
            blocks,
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            proven_roots: roots,
        }
    }

    fn closure_of(cfg: Cfg) -> ClosureResult {
        ClosureResult {
            cfg,
            indirect: Vec::new(),
        }
    }

    fn owner_partition(block_start: u32, block_end: u32, root: u32) -> Partition {
        Partition {
            bank: "bank".into(),
            owners: vec![Owner {
                bank: "bank".into(),
                root_va: root,
                block_starts: vec![block_start],
                extent_end: block_end,
            }],
            ambiguous: Vec::new(),
            unowned: Vec::new(),
        }
    }

    fn projected_report(
        broad: &Cfg,
        broad_partition: &Partition,
        owners: &OwnerProofReport,
        authority: &ClosureResult,
        mapped_len: u32,
    ) -> BlockProofReport {
        let projection = AuthorityReachabilityProjection::from_authority_closure(authority);
        prove_reachable_blocks_with_authority_projection(
            broad,
            broad_partition,
            owners,
            &mapped_facts_len(mapped_len),
            &projection,
        )
    }

    fn owner_report(roots: &[(u32, bool)]) -> OwnerProofReport {
        OwnerProofReport {
            bank: "bank".into(),
            assessments: roots
                .iter()
                .map(|&(root, is_authoritative)| OwnerAssessment::Candidate {
                    frontier: OwnerFrontier {
                        entry: BankAddr::new("bank", root),
                        proposed_va_end: None,
                        blockers: (!is_authoritative)
                            .then_some(OwnerBlocker::EntryNotAuthoritative)
                            .into_iter()
                            .collect(),
                    },
                })
                .collect(),
        }
    }

    fn ambiguous_partition(roots: &[u32]) -> Partition {
        Partition {
            bank: "bank".into(),
            owners: Vec::new(),
            ambiguous: vec![AmbiguousBlock {
                block_start: BASE,
                claimants: roots.to_vec(),
            }],
            unowned: Vec::new(),
        }
    }

    #[test]
    fn shared_block_with_multiple_authoritative_roots_is_proven() {
        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &ambiguous_partition(&[BASE + 0x40, BASE]),
            &owner_report(&[(BASE, true), (BASE + 0x40, true)]),
            &mapped_facts(),
        );
        let BlockAssessment::Proven { block } = &report.assessments[0] else {
            panic!("shared authoritative reachability was not admitted");
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE, BASE + 0x40]);
    }

    #[test]
    fn one_authoritative_claimant_is_sufficient_and_heuristic_claimant_is_excluded() {
        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &ambiguous_partition(&[BASE, BASE + 0x40]),
            &owner_report(&[(BASE, true), (BASE + 0x40, false)]),
            &mapped_facts(),
        );
        let BlockAssessment::Proven { block } = &report.assessments[0] else {
            panic!("authoritative reachability was hidden by function-owner ambiguity");
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE]);
    }

    #[test]
    fn shared_block_without_an_authoritative_claimant_stays_open() {
        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &ambiguous_partition(&[BASE, BASE + 0x40]),
            &owner_report(&[(BASE, false), (BASE + 0x40, false)]),
            &mapped_facts(),
        );
        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::NoAuthoritativeReachability {
                    roots: vec![BASE, BASE + 0x40],
                })
        ));
    }

    #[test]
    fn shared_authority_does_not_bypass_backing_or_decoder_proof() {
        let partition = ambiguous_partition(&[BASE, BASE + 0x40]);
        let owners = owner_report(&[(BASE, true), (BASE + 0x40, true)]);
        let no_backing =
            prove_reachable_blocks(&one_block_cfg(), &partition, &owners, &FactDb::new());
        assert!(matches!(
            &no_backing.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::MissingBankBacking)
        ));

        let mut invalid_word = one_block_cfg();
        invalid_word
            .word_class
            .insert(BASE + 4, WordClass::CandidateCode);
        let invalid = prove_reachable_blocks(&invalid_word, &partition, &owners, &mapped_facts());
        assert!(matches!(
            &invalid.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::WordNotProvenCode {
                    pc: BASE + 4,
                    class: Some(WordClass::CandidateCode),
                })
        ));
    }

    #[test]
    fn materialized_block_retains_output_offsets_without_rom_coordinates() {
        let (facts, receipt_sha256) = materialized_facts(8, 8);
        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &owner_partition(BASE, BASE + 8, BASE),
            &owner_report(&[(BASE, true)]),
            &facts,
        );

        let BlockAssessment::Proven { block } = &report.assessments[0] else {
            panic!("materialized block was not admitted");
        };
        assert_eq!(
            block.backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256,
                output_start: 0,
                output_end: 8,
            }
        );
        let wire = serde_json::to_value(block).unwrap();
        assert!(wire.get("rom_start").is_none());
        assert!(wire.get("rom_end").is_none());
    }

    #[test]
    fn competing_proven_bank_images_block_proof_before_interval_selection() {
        let (mut facts, _) = materialized_facts(8, 8);
        let materialized = facts.conclusion("bank:bank").unwrap().justified_by[0];
        let affine = facts.insert(Fact::RomMapping {
            bank: "bank".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x1008,
            va_start: BASE,
            va_end: BASE + 8,
        });
        facts
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![materialized, affine],
                "test_competing_image",
            )
            .unwrap();

        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &owner_partition(BASE, BASE + 8, BASE),
            &owner_report(&[(BASE, true)]),
            &facts,
        );
        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::AmbiguousBankBacking)
        ));
    }

    #[test]
    fn invalid_materialized_geometry_is_a_distinct_blocker() {
        let (facts, _) = materialized_facts(4, 8);
        let report = prove_reachable_blocks(
            &one_block_cfg(),
            &owner_partition(BASE, BASE + 8, BASE),
            &owner_report(&[(BASE, true)]),
            &facts,
        );
        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::InvalidBankBackingGeometry)
        ));
    }

    #[test]
    fn authority_projection_proves_fully_contained_candidate_split_without_promoting_owner() {
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 0x10,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE + 4,
                end_va: BASE + 0x10,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE + 4],
        );
        let owners = owner_report(&[(BASE + 4, false)]);
        let report = projected_report(
            &broad,
            &owner_partition(BASE + 4, BASE + 0x10, BASE + 4),
            &owners,
            &authority,
            0x10,
        );

        let BlockAssessment::Proven { block } = &report.assessments[0] else {
            panic!("fully-contained authority reachability was not projected");
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE]);
        assert!(matches!(
            &owners.assessments[0],
            OwnerAssessment::Candidate { frontier }
                if frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
        ));
    }

    #[test]
    fn authority_projection_proves_candidate_tail_root_reached_by_authority() {
        let tail = BASE + 0x10;
        let blocks = vec![
            BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Tail { target: tail },
            },
            BasicBlock {
                start_va: tail,
                end_va: tail + 8,
                terminator: BlockTerminator::Return,
            },
        ];
        let authority = closure_of(cfg_from_blocks(blocks.clone(), vec![BASE]));
        let broad = cfg_from_blocks(blocks, vec![BASE, tail]);
        let owners = owner_report(&[(BASE, true), (tail, false)]);
        let report = projected_report(&broad, &partition(&broad), &owners, &authority, 0x18);

        let BlockAssessment::Proven { block } = report
            .assessments
            .iter()
            .find(|assessment| {
                matches!(assessment, BlockAssessment::Proven { block } if block.start_va == tail)
            })
            .expect("tail block assessment")
        else {
            unreachable!()
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE]);
    }

    #[test]
    fn authority_projection_preserves_all_shared_authoritative_claimants() {
        let second_root = BASE + 8;
        let shared = BASE + 0x10;
        let blocks = vec![
            BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Tail { target: shared },
            },
            BasicBlock {
                start_va: second_root,
                end_va: second_root + 8,
                terminator: BlockTerminator::Tail { target: shared },
            },
            BasicBlock {
                start_va: shared,
                end_va: shared + 8,
                terminator: BlockTerminator::Return,
            },
        ];
        let authority = closure_of(cfg_from_blocks(blocks.clone(), vec![second_root, BASE]));
        let broad = cfg_from_blocks(blocks, vec![BASE, second_root, shared]);
        let owners = owner_report(&[(BASE, true), (second_root, true), (shared, false)]);
        let report = projected_report(&broad, &partition(&broad), &owners, &authority, 0x18);

        let BlockAssessment::Proven { block } = report
            .assessments
            .iter()
            .find(|assessment| {
                matches!(assessment, BlockAssessment::Proven { block } if block.start_va == shared)
            })
            .expect("shared block assessment")
        else {
            unreachable!()
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE, second_root]);
    }

    #[test]
    fn authority_projection_rejects_disconnected_candidate_root() {
        let candidate = BASE + 0x10;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![BasicBlock {
                start_va: candidate,
                end_va: candidate + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![candidate],
        );
        let report = projected_report(
            &broad,
            &owner_partition(candidate, candidate + 8, candidate),
            &owner_report(&[(candidate, false)]),
            &authority,
            0x18,
        );

        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::EntryNotAuthoritative {
                    root: candidate,
                })
        ));
    }

    #[test]
    fn authority_projection_rejects_partial_overlap() {
        let candidate = BASE + 4;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![BasicBlock {
                start_va: candidate,
                end_va: BASE + 0x0c,
                terminator: BlockTerminator::Return,
            }],
            vec![candidate],
        );
        let report = projected_report(
            &broad,
            &owner_partition(candidate, BASE + 0x0c, candidate),
            &owner_report(&[(candidate, false)]),
            &authority,
            0x0c,
        );

        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::EntryNotAuthoritative {
                    root: candidate,
                })
        ));
    }

    #[test]
    fn authority_projection_rejects_candidate_shaped_call_bytes() {
        let candidate = BASE + 0x10;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![BasicBlock {
                start_va: candidate,
                end_va: candidate + 8,
                terminator: BlockTerminator::Call {
                    target: 0x8090_0000,
                    next: candidate + 8,
                },
            }],
            vec![candidate],
        );
        let report = projected_report(
            &broad,
            &owner_partition(candidate, candidate + 8, candidate),
            &owner_report(&[(candidate, false)]),
            &authority,
            0x18,
        );

        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::EntryNotAuthoritative {
                    root: candidate,
                })
        ));
    }

    #[test]
    fn authority_projection_does_not_import_candidate_call_target_authority() {
        let target = BASE + 8;
        let candidate_caller = BASE + 0x18;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 0x10,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![
                BasicBlock {
                    start_va: target,
                    end_va: BASE + 0x10,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: candidate_caller,
                    end_va: candidate_caller + 8,
                    terminator: BlockTerminator::Call {
                        target,
                        next: candidate_caller + 8,
                    },
                },
            ],
            vec![target, candidate_caller],
        );
        let broad_partition = Partition {
            bank: "bank".into(),
            owners: vec![
                Owner {
                    bank: "bank".into(),
                    root_va: target,
                    block_starts: vec![target],
                    extent_end: BASE + 0x10,
                },
                Owner {
                    bank: "bank".into(),
                    root_va: candidate_caller,
                    block_starts: vec![candidate_caller],
                    extent_end: candidate_caller + 8,
                },
            ],
            ambiguous: Vec::new(),
            unowned: Vec::new(),
        };
        let owners = owner_report(&[(target, false), (candidate_caller, false)]);
        let report = projected_report(&broad, &broad_partition, &owners, &authority, 0x20);

        let BlockAssessment::Proven { block } = &report.assessments[0] else {
            panic!("authority-reached target block should remain executable");
        };
        assert_eq!(block.authoritative_roots.as_slice(), &[BASE]);
        assert!(matches!(
            &owners.assessments[0],
            OwnerAssessment::Candidate { frontier }
                if frontier.entry.pc == target
                    && frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
        ));
        assert!(matches!(
            &report.assessments[1],
            BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&BlockProofBlocker::EntryNotAuthoritative {
                    root: candidate_caller,
                })
        ));
    }

    #[test]
    fn authority_projection_rejects_boundaries_that_sever_a_delay_slot() {
        let delay_slot = BASE + 4;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![
                BasicBlock {
                    start_va: BASE,
                    end_va: delay_slot,
                    terminator: BlockTerminator::Fallthrough { next: delay_slot },
                },
                BasicBlock {
                    start_va: delay_slot,
                    end_va: BASE + 8,
                    terminator: BlockTerminator::Trap,
                },
            ],
            vec![BASE, delay_slot],
        );
        let broad_partition = Partition {
            bank: "bank".into(),
            owners: vec![
                Owner {
                    bank: "bank".into(),
                    root_va: BASE,
                    block_starts: vec![BASE],
                    extent_end: delay_slot,
                },
                Owner {
                    bank: "bank".into(),
                    root_va: delay_slot,
                    block_starts: vec![delay_slot],
                    extent_end: BASE + 8,
                },
            ],
            ambiguous: Vec::new(),
            unowned: Vec::new(),
        };
        let report = projected_report(
            &broad,
            &broad_partition,
            &owner_report(&[(BASE, true), (delay_slot, false)]),
            &authority,
            8,
        );

        assert!(report
            .assessments
            .iter()
            .all(|assessment| matches!(assessment, BlockAssessment::Candidate { .. })));
    }

    #[test]
    fn authority_projection_rejects_block_spanning_two_authority_blocks() {
        let authority = closure_of(cfg_from_blocks(
            vec![
                BasicBlock {
                    start_va: BASE,
                    end_va: BASE + 8,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: BASE + 8,
                    end_va: BASE + 0x10,
                    terminator: BlockTerminator::Return,
                },
            ],
            vec![BASE, BASE + 8],
        ));
        let broad = cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 0x10,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        );
        let report = projected_report(
            &broad,
            &owner_partition(BASE, BASE + 0x10, BASE),
            &owner_report(&[(BASE, true)]),
            &authority,
            0x10,
        );

        assert!(matches!(
            &report.assessments[0],
            BlockAssessment::Candidate { .. }
        ));
    }

    #[test]
    fn authority_projection_preserves_malformed_and_data_fence_blockers() {
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        for (terminator, expected) in [
            (
                BlockTerminator::InvalidInstruction {
                    pc: BASE + 4,
                    word: 0xffff_ffff,
                },
                BlockProofBlocker::InvalidInstruction {
                    pc: BASE + 4,
                    word: 0xffff_ffff,
                },
            ),
            (
                BlockTerminator::DataFence { at: BASE + 8 },
                BlockProofBlocker::RanOffEnd,
            ),
        ] {
            let broad = cfg_from_blocks(
                vec![BasicBlock {
                    start_va: BASE,
                    end_va: BASE + 8,
                    terminator,
                }],
                vec![BASE],
            );
            let report = projected_report(
                &broad,
                &owner_partition(BASE, BASE + 8, BASE),
                &owner_report(&[(BASE, true)]),
                &authority,
                8,
            );
            assert!(matches!(
                &report.assessments[0],
                BlockAssessment::Candidate { blockers, .. } if blockers.contains(&expected)
            ));
        }
    }

    #[test]
    fn authority_projection_does_not_import_broad_resolved_indirect_targets() {
        let target = BASE + 0x10;
        let authority = closure_of(cfg_from_blocks(
            vec![BasicBlock {
                start_va: BASE,
                end_va: BASE + 8,
                terminator: BlockTerminator::Return,
            }],
            vec![BASE],
        ));
        let broad = cfg_from_blocks(
            vec![
                BasicBlock {
                    start_va: BASE,
                    end_va: BASE + 8,
                    terminator: BlockTerminator::ResolvedIndirect {
                        targets: vec![target],
                        via_call: false,
                    },
                },
                BasicBlock {
                    start_va: target,
                    end_va: target + 8,
                    terminator: BlockTerminator::Return,
                },
            ],
            vec![BASE, target],
        );
        let report = projected_report(
            &broad,
            &partition(&broad),
            &owner_report(&[(BASE, true), (target, false)]),
            &authority,
            0x18,
        );

        assert!(matches!(
            report.assessments.iter().find(|assessment| matches!(
                assessment,
                BlockAssessment::Candidate { start_va, .. } if *start_va == target
            )),
            Some(BlockAssessment::Candidate { blockers, .. })
                if blockers.contains(&BlockProofBlocker::NoAuthoritativeReachability {
                    roots: vec![BASE, target],
                })
        ));
    }

    #[test]
    fn authoritative_roots_wire_rejects_empty_or_noncanonical_authority() {
        assert!(serde_json::from_str::<AuthoritativeReachabilityRoots>("[]").is_err());
        assert!(
            serde_json::from_str::<AuthoritativeReachabilityRoots>(&format!(
                "[{},{}]",
                BASE + 4,
                BASE
            ))
            .is_err()
        );
        assert!(
            serde_json::from_str::<AuthoritativeReachabilityRoots>(&format!("[{0},{0}]", BASE))
                .is_err()
        );
    }

    #[test]
    fn reached_blocks_conclude_proven_ranges_without_bridging_gaps() {
        // Two adjacent proven blocks merge into one interval; the third sits
        // past an unreached gap and must stay a separate interval.
        let report = report_of(vec![
            proven_block(BASE, BASE + 8),
            proven_block(BASE + 8, BASE + 0x10),
            proven_block(BASE + 0x20, BASE + 0x28),
        ]);
        let mut facts = FactDb::new();
        let derived = conclude_reached_executable_ranges(&report, &mut facts);
        assert_eq!(
            derived,
            vec![
                DerivedExecutableRange {
                    va_start: BASE,
                    va_end: BASE + 0x10,
                    state: ProofState::Proven,
                },
                DerivedExecutableRange {
                    va_start: BASE + 0x20,
                    va_end: BASE + 0x28,
                    state: ProofState::Proven,
                },
            ]
        );
        let ranges = facts.proven_executable_ranges("bank");
        assert_eq!(
            ranges,
            vec![(BASE, BASE + 0x10), (BASE + 0x20, BASE + 0x28)]
        );
        // The gap byte interval is covered by no proven range.
        assert!(!ranges
            .iter()
            .any(|&(start, end)| start <= BASE + 0x10 && end > BASE + 0x10));
    }

    #[test]
    fn unproven_blocks_derive_no_executable_evidence() {
        let report = report_of(vec![BlockAssessment::Candidate {
            start_va: BASE,
            end_va: BASE + 8,
            blockers: vec![BlockProofBlocker::Unowned],
        }]);
        let mut facts = FactDb::new();
        assert!(conclude_reached_executable_ranges(&report, &mut facts).is_empty());
        assert!(facts.proven_executable_ranges("bank").is_empty());
        assert!(facts.facts().is_empty());
    }

    #[test]
    fn contradicted_range_subject_is_not_silently_promoted() {
        let report = report_of(vec![proven_block(BASE, BASE + 8)]);
        let mut facts = FactDb::new();
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + 8),
                ProofState::Rejected,
                vec![],
                "test_prior_rejection",
            )
            .unwrap();
        let derived = conclude_reached_executable_ranges(&report, &mut facts);
        assert_eq!(derived[0].state, ProofState::Conflict);
        assert!(facts.proven_executable_ranges("bank").is_empty());
        assert_eq!(
            facts
                .conclusion(&executable_range_subject("bank", BASE, BASE + 8))
                .unwrap()
                .state,
            ProofState::Conflict
        );
    }
}
