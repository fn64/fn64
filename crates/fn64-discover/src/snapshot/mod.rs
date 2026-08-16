//! Deterministic composition of discovery's existing phase outputs into one
//! proof-carrying program artifact.
//!
//! The Rust envelope retains its historical `ProgramSnapshotV1` name, while
//! its serialized schema is V6 after preserving the selected bank image as a
//! typed affine-ROM or evaluator-produced backing span. It re-derives that
//! backing from [`NormalizedRom`] and the projected [`FactDb`], verifies the
//! supplied bytes before analysis, runs Phase
//! 4-6 closure, writes closure-derived facts into a cloned [`FactDb`], then
//! partitions and proves owners from that same fact snapshot. Reached
//! proven-code blocks additionally become typed `ExecutableRange` facts
//! (exactly the reached bytes, gaps never bridged — see
//! [`conclude_reached_executable_ranges`]) before the final owner pass, so
//! executable coverage over reached code is evidence, not an input
//! assumption. Traversal seeds guide reachability but never become entry
//! authority by themselves.

use crate::block_proof::{
    conclude_reached_executable_ranges, prove_reachable_blocks_with_authority_projection,
    AuthorityReachabilityProjection, BlockProofReport,
};
use crate::callback_flow::{
    discover_callback_argument_contracts, discover_callback_registry_contracts, CallbackFlowError,
};
use crate::coverage::{report_with_owner_proofs, CoverageReport, OwnerProofCoverageError};
use crate::dense_aot_pack::DenseAotPackV1;
use crate::facts::{
    function_entry_subject, BankAddr, BankBackingSpanResolutionV1, BankBackingSpanV1,
    CandidateDetector, Fact, FactDb, FactProjectionError, FactProjectionIndex,
    FunctionEntryEvidence, IndirectTransferKind, IndirectTransferState, ProofState,
    SemanticCallableContract,
};
use crate::generation_topology::{
    dense_aot_pack_sha256_v1, CatalogBoundExactTransferV1, ExactTransferKindV1,
    GenerationTopologyV1,
};
use crate::host_bindings::{
    discover_os_create_thread_host_binding, HostBindingDiscoveryError, HostBindingSymbol,
};
use crate::loaders::VirtualAddress;
use crate::materialized_image::{
    materialize_backing_span_v1, MaterializedBackingSpanCacheV1, MaterializedBackingSpanErrorV1,
    MaterializedImageErrorV1, MaterializedImageLimitsV1,
};
use crate::owner_proof::{
    exact_authority_direct_call, exhaustive_authority_call_site, prove_exact_owners_with_authority,
    OwnerAssessment, OwnerBlocker, OwnerProofAuthority, OwnerProofReport,
};
use crate::partition::{partition, partition_with_authorized_splits, Partition};
use crate::pi_dma::{slice_pointer_arg_call_contracts, PiDmaSliceError};
use crate::resolve::{
    build_cfg_value_set_closed, ClosureResult, IndirectProofState, IndirectResolutionKind,
};
use crate::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

/// Legacy affine-only snapshot wire, retained only so readers can reject it by
/// its exact historical value. New composition never emits V5.
pub const PROGRAM_SNAPSHOT_SCHEMA_V5: u32 = 5;
/// V6 retains a tagged affine-ROM or evaluator-produced backing on every bank
/// input, owner, and proven block.
pub const PROGRAM_SNAPSHOT_SCHEMA_V6: u32 = 6;
const REPORT_PROJECTION_STATS_ENV: &str = "FN64_DISCOVER_REPORT_PROJECTION_STATS";

/// Runtime image supplied to the one-bank V1 composer. The bank may be the
/// resident image or an overlay; its identity and physical backing must be
/// proven in `base_facts`. `seed_roots` are traversal hints only; owner proof
/// still requires independent authority.
pub struct MaterializedBankInput<'a> {
    pub bank: &'a str,
    pub va_start: u32,
    pub bytes: &'a [u8],
    pub seed_roots: &'a [u32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BankInputDigestV1 {
    pub bank: String,
    pub va_start: u32,
    pub va_end: u32,
    pub backing: BankBackingSpanV1,
    pub bytes_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BankSnapshotV1 {
    pub input: BankInputDigestV1,
    /// CFG reached only from typed callable authority. `closure` deliberately
    /// remains broader so candidate roots can improve discovery coverage
    /// without entering execution-closure evidence.
    pub authority_closure: ClosureResult,
    pub closure: ClosureResult,
    pub partition: Partition,
    pub owner_proof: OwnerProofReport,
    pub block_proof: BlockProofReport,
    pub blocker_histogram: Vec<OwnerBlockerSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramSnapshotV1 {
    pub schema_version: u32,
    pub normalized_rom_sha256: String,
    pub facts: FactDb,
    /// V1 produces exactly one bank, but the envelope is already bank-keyed
    /// so the portable schema need not change when composition becomes
    /// multi-bank.
    pub banks: Vec<BankSnapshotV1>,
    pub coverage: CoverageReport,
}

/// Move-only authority for snapshots produced by the byte-verifying V3
/// composition pipeline.
///
/// `ProgramSnapshotV1` remains serializable for inspection and diagnostic
/// tooling, but its public wire is not execution authority: a caller can edit
/// a JSON proof report.  This wrapper has no public constructor, `Clone`, or
/// deserializer.  The authoritative BlockPack emitter accepts only this type
/// plus an index into its exact composed snapshot set.
///
/// This capability proves that composition ran and byte-verified every bank
/// against the supplied facts and normalized ROM. It does not authenticate
/// the provenance of `base_facts`: callers at an admission boundary must
/// obtain those facts from trusted discovery/evidence ingestion rather than
/// deserialize or author `Proven` conclusions themselves.
///
/// ```compile_fail
/// let _forged = fn64_discover::snapshot::ValidatedComposedSnapshotsV2 {
///     snapshots: Vec::new(),
/// };
/// ```
///
/// ```compile_fail
/// # use fn64_discover::{NormalizedRom, snapshot::ProgramSnapshotV1};
/// fn promote_deserialized(snapshot: &ProgramSnapshotV1, rom: &NormalizedRom) {
///     let _ = fn64_discover::block_pack::emit_validated_block_pack_v2(snapshot, 0, rom);
/// }
/// ```
#[derive(Debug)]
pub struct ValidatedComposedSnapshotsV2 {
    snapshots: Vec<ProgramSnapshotV1>,
}

impl ValidatedComposedSnapshotsV2 {
    pub fn snapshots(&self) -> &[ProgramSnapshotV1] {
        &self.snapshots
    }

    pub(crate) fn snapshot(&self, index: usize) -> Option<&ProgramSnapshotV1> {
        self.snapshots.get(index)
    }

    /// Discard execution authority and retain only the diagnostic/interchange
    /// snapshots. There is deliberately no inverse conversion.
    pub fn into_diagnostic_snapshots(self) -> Vec<ProgramSnapshotV1> {
        self.snapshots
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerBlockerKind {
    BankNotProven,
    PartitionBankMismatch,
    EntryNotAuthoritative,
    OwnerMissing,
    DuplicateOwner,
    OwnerBankMismatch,
    OwnerNotContiguous,
    PartitionAmbiguity,
    PartitionOverlap,
    DuplicateCfgBlockStart,
    MissingCfgBlock,
    MalformedBlock,
    InconsistentTerminator,
    RanOffEnd,
    InvalidInstruction,
    MissingDelaySlot,
    WordNotProvenCode,
    MissingBankBacking,
    AmbiguousBankBacking,
    InvalidBankBackingGeometry,
    NotProvenExecutable,
    InteriorCallableEntry,
    InteriorCandidateEntry,
    TrailingUnattributedCode,
    IncomingEdge,
    ObservedInteriorEntry,
    ResolvedJumpLeavesOwner,
    ResolvedIndirectNotExhaustive,
    ResolvedIndirectEvidenceMismatch,
    UnresolvedIndirect,
}

impl From<&OwnerBlocker> for OwnerBlockerKind {
    fn from(blocker: &OwnerBlocker) -> Self {
        match blocker {
            OwnerBlocker::BankNotProven => Self::BankNotProven,
            OwnerBlocker::PartitionBankMismatch { .. } => Self::PartitionBankMismatch,
            OwnerBlocker::EntryNotAuthoritative => Self::EntryNotAuthoritative,
            OwnerBlocker::OwnerMissing => Self::OwnerMissing,
            OwnerBlocker::DuplicateOwner => Self::DuplicateOwner,
            OwnerBlocker::OwnerBankMismatch { .. } => Self::OwnerBankMismatch,
            OwnerBlocker::OwnerNotContiguous => Self::OwnerNotContiguous,
            OwnerBlocker::PartitionAmbiguity { .. } => Self::PartitionAmbiguity,
            OwnerBlocker::PartitionOverlap { .. } => Self::PartitionOverlap,
            OwnerBlocker::DuplicateCfgBlockStart { .. } => Self::DuplicateCfgBlockStart,
            OwnerBlocker::MissingCfgBlock { .. } => Self::MissingCfgBlock,
            OwnerBlocker::MalformedBlock { .. } => Self::MalformedBlock,
            OwnerBlocker::InconsistentTerminator { .. } => Self::InconsistentTerminator,
            OwnerBlocker::RanOffEnd { .. } => Self::RanOffEnd,
            OwnerBlocker::InvalidInstruction { .. } => Self::InvalidInstruction,
            OwnerBlocker::MissingDelaySlot { .. } => Self::MissingDelaySlot,
            OwnerBlocker::WordNotProvenCode { .. } => Self::WordNotProvenCode,
            OwnerBlocker::MissingBankBacking => Self::MissingBankBacking,
            OwnerBlocker::AmbiguousBankBacking => Self::AmbiguousBankBacking,
            OwnerBlocker::InvalidBankBackingGeometry => Self::InvalidBankBackingGeometry,
            OwnerBlocker::NotProvenExecutable => Self::NotProvenExecutable,
            OwnerBlocker::InteriorCallableEntry { .. } => Self::InteriorCallableEntry,
            OwnerBlocker::InteriorCandidateEntry { .. } => Self::InteriorCandidateEntry,
            OwnerBlocker::TrailingUnattributedCode { .. } => Self::TrailingUnattributedCode,
            OwnerBlocker::IncomingEdge { .. } => Self::IncomingEdge,
            OwnerBlocker::ObservedInteriorEntry { .. } => Self::ObservedInteriorEntry,
            OwnerBlocker::ResolvedJumpLeavesOwner { .. } => Self::ResolvedJumpLeavesOwner,
            OwnerBlocker::ResolvedIndirectNotExhaustive { .. } => {
                Self::ResolvedIndirectNotExhaustive
            }
            OwnerBlocker::ResolvedIndirectEvidenceMismatch { .. } => {
                Self::ResolvedIndirectEvidenceMismatch
            }
            OwnerBlocker::UnresolvedIndirect { .. } => Self::UnresolvedIndirect,
        }
    }
}

impl OwnerBlockerKind {
    /// Stable content-free label for summary and aggregate diagnostics. The
    /// explicit spelling keeps those consumers independent of Debug/stderr.
    pub const fn diagnostic_label(self) -> &'static str {
        match self {
            Self::BankNotProven => "bank_not_proven",
            Self::PartitionBankMismatch => "partition_bank_mismatch",
            Self::EntryNotAuthoritative => "entry_not_authoritative",
            Self::OwnerMissing => "owner_missing",
            Self::DuplicateOwner => "duplicate_owner",
            Self::OwnerBankMismatch => "owner_bank_mismatch",
            Self::OwnerNotContiguous => "owner_not_contiguous",
            Self::PartitionAmbiguity => "partition_ambiguity",
            Self::PartitionOverlap => "partition_overlap",
            Self::DuplicateCfgBlockStart => "duplicate_cfg_block_start",
            Self::MissingCfgBlock => "missing_cfg_block",
            Self::MalformedBlock => "malformed_block",
            Self::InconsistentTerminator => "inconsistent_terminator",
            Self::RanOffEnd => "ran_off_end",
            Self::InvalidInstruction => "invalid_instruction",
            Self::MissingDelaySlot => "missing_delay_slot",
            Self::WordNotProvenCode => "word_not_proven_code",
            Self::MissingBankBacking => "missing_bank_backing",
            Self::AmbiguousBankBacking => "ambiguous_bank_backing",
            Self::InvalidBankBackingGeometry => "invalid_bank_backing_geometry",
            Self::NotProvenExecutable => "not_proven_executable",
            Self::InteriorCallableEntry => "interior_callable_entry",
            Self::InteriorCandidateEntry => "interior_candidate_entry",
            Self::TrailingUnattributedCode => "trailing_unattributed_code",
            Self::IncomingEdge => "incoming_edge",
            Self::ObservedInteriorEntry => "observed_interior_entry",
            Self::ResolvedJumpLeavesOwner => "resolved_jump_leaves_owner",
            Self::ResolvedIndirectNotExhaustive => "resolved_indirect_not_exhaustive",
            Self::ResolvedIndirectEvidenceMismatch => "resolved_indirect_evidence_mismatch",
            Self::UnresolvedIndirect => "unresolved_indirect",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerBlockerSummary {
    pub kind: OwnerBlockerKind,
    /// Number of owner assessments containing at least one blocker of this
    /// category.
    pub affected_assessments: u64,
    /// Total blockers of this category, retaining distinct-site multiplicity.
    pub occurrences: u64,
    /// Assessments whose entire remaining frontier consists of this blocker
    /// category (possibly at multiple sites). This is the immediate payoff
    /// if that category alone is discharged.
    pub sole_blocker_assessments: u64,
}

pub fn owner_blocker_histogram(report: &OwnerProofReport) -> Vec<OwnerBlockerSummary> {
    let mut counts = BTreeMap::<OwnerBlockerKind, (u64, u64, u64)>::new();
    for assessment in &report.assessments {
        let blockers = match assessment {
            OwnerAssessment::Proven { .. } => continue,
            OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
                &frontier.blockers
            }
        };
        let mut seen = BTreeSet::new();
        for blocker in blockers {
            let kind = OwnerBlockerKind::from(blocker);
            counts.entry(kind).or_default().1 += 1;
            seen.insert(kind);
        }
        let sole_kind = (seen.len() == 1).then(|| *seen.first().expect("one blocker kind"));
        for kind in seen {
            counts.entry(kind).or_default().0 += 1;
        }
        if let Some(kind) = sole_kind {
            counts.entry(kind).or_default().2 += 1;
        }
    }
    counts
        .into_iter()
        .map(
            |(kind, (affected_assessments, occurrences, sole_blocker_assessments))| {
                OwnerBlockerSummary {
                    kind,
                    affected_assessments,
                    occurrences,
                    sole_blocker_assessments,
                }
            },
        )
        .collect()
}

/// Fail-closed resource envelope for the compatibility multi-bank composer.
///
/// Each bank snapshot stores its exact bank-indexed [`FactDb`] projection.
/// Limits cover aggregate projected rows and compact serialized bytes; a row
/// count alone cannot bound facts with variable-length strings or target
/// vectors. Materialized bytes and derived cross-bank authority records have
/// independent aggregate budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiBankCompositionLimits {
    pub max_projected_fact_rows: u64,
    pub max_projected_fact_bytes: u64,
    pub max_aggregate_materialized_bytes: u64,
    pub max_cross_bank_authority_records: u64,
    pub materialized_image: MaterializedImageLimitsV1,
}

impl Default for MultiBankCompositionLimits {
    fn default() -> Self {
        Self {
            // Synthetic profiling measured about 138-144 live bytes per
            // projected small fact. Four million leaves substantial space below
            // a 2 GiB process envelope for larger real facts and bank proofs.
            max_projected_fact_rows: 4_000_000,
            // Compact wire bytes are a second, shape-sensitive bound. A 256
            // MiB projected facts leave room for Rust allocation overhead,
            // closures, owner reports, and the output serializer.
            max_projected_fact_bytes: 256 * 1024 * 1024,
            max_aggregate_materialized_bytes: 256 * 1024 * 1024,
            max_cross_bank_authority_records: 1_048_576,
            materialized_image: MaterializedImageLimitsV1::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    FactProjection(FactProjectionError),
    ProjectedFactsSerialization {
        bank: String,
        error: String,
    },
    CompositionLimitArithmeticOverflow {
        calculation: &'static str,
    },
    ProjectedFactRowsLimitExceeded {
        rows: u64,
        limit: u64,
    },
    ProjectedFactBytesLimitExceeded {
        bank: String,
        bytes: u64,
        rows: u64,
        bank_rows: u64,
        bank_scoped_rows: u64,
        bank_selected_conclusions: u64,
        largest_justifications: Vec<(String, usize)>,
        processed_banks: u64,
        total_banks: u64,
        global_fact_rows: u64,
        limit: u64,
    },
    AggregateMaterializedBytesLimitExceeded {
        bytes: u64,
        limit: u64,
    },
    CrossBankAuthorityRecordsLimitExceeded {
        records: u64,
        limit: u64,
    },
    CatalogCapabilityIdentityMismatch {
        index: usize,
    },
    AmbiguousOsCreateThreadBinding {
        bank: String,
        candidates: Vec<u32>,
    },
    HostBindingDiscovery(HostBindingDiscoveryError),
    CallbackFlow(CallbackFlowError),
    CallableArgumentSlice(PiDmaSliceError),
    SemanticEntryConclusion {
        target: BankAddr,
        detail: String,
    },
    InvalidBankName,
    DuplicateBankName {
        bank: String,
    },
    EmptyBank,
    UnalignedBank {
        va_start: u32,
        byte_len: usize,
    },
    BankAddressOverflow {
        va_start: u32,
        byte_len: usize,
    },
    RootUnaligned {
        root: u32,
    },
    RootOutsideBank {
        root: u32,
        va_start: u32,
        va_end: u32,
    },
    AuthoritativeRootOnDelaySlot {
        bank: String,
        root: u32,
        control_pc: u32,
    },
    UnsupportedControlDelayEntry {
        bank: String,
        entry: u32,
        control_pc: u32,
    },
    MissingProvenBacking {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    AmbiguousProvenBacking {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    InvalidProvenBackingGeometry {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    MissingEvaluatedImageReceipt {
        bank: String,
        receipt_sha256: String,
    },
    AmbiguousEvaluatedImageReceipt {
        bank: String,
        receipt_sha256: String,
        count: usize,
    },
    BackingMaterialization {
        bank: String,
        backing: BankBackingSpanV1,
        reason: String,
    },
    EvaluatedImageRederivation {
        bank: String,
        receipt_sha256: String,
        error: MaterializedImageErrorV1,
    },
    MaterializedBytesMismatch {
        bank: String,
        backing: BankBackingSpanV1,
    },
    ExecutableRangeOutsideBank {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    Coverage(OwnerProofCoverageError),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FactProjection(error) => write!(f, "indexing facts by bank: {error}"),
            Self::ProjectedFactsSerialization { bank, error } => write!(
                f,
                "serializing projected facts for bank {bank} for composition limits: {error}"
            ),
            Self::CompositionLimitArithmeticOverflow { calculation } => {
                write!(f, "multi-bank composition limit arithmetic overflow: {calculation}")
            }
            Self::ProjectedFactRowsLimitExceeded { rows, limit } => write!(
                f,
                "multi-bank composition projected fact rows {rows} exceeds {limit}"
            ),
            Self::ProjectedFactBytesLimitExceeded {
                bank,
                bytes,
                rows,
                bank_rows,
                bank_scoped_rows,
                bank_selected_conclusions,
                largest_justifications,
                processed_banks,
                total_banks,
                global_fact_rows,
                limit,
            } => write!(
                f,
                "multi-bank composition projected fact bytes {bytes} across {rows} rows after {processed_banks}/{total_banks} banks exceeds {limit}; current bank {bank} has {bank_rows} projected rows ({global_fact_rows} global, {bank_scoped_rows} directly scoped) and {bank_selected_conclusions} selected conclusions; largest justifications {largest_justifications:?}"
            ),
            Self::AggregateMaterializedBytesLimitExceeded { bytes, limit } => write!(
                f,
                "multi-bank composition materialized bytes {bytes} exceeds {limit}"
            ),
            Self::CrossBankAuthorityRecordsLimitExceeded { records, limit } => write!(
                f,
                "multi-bank composition cross-bank authority records {records} exceeds {limit}"
            ),
            Self::CatalogCapabilityIdentityMismatch { index } => write!(
                f,
                "catalog-bound exact-transfer capability {index} does not match the complete composition identity"
            ),
            Self::AmbiguousOsCreateThreadBinding { bank, candidates } => write!(
                f,
                "bank {bank} has {} semantic osCreateThread matches; callable-entry authority is ambiguous",
                candidates.len()
            ),
            Self::CallableArgumentSlice(error) => {
                write!(f, "slicing osCreateThread entry arguments: {error:?}")
            }
            Self::SemanticEntryConclusion { target, detail } => write!(
                f,
                "publishing semantic callable entry {}:0x{:08x}: {detail}",
                target.bank, target.pc
            ),
            Self::HostBindingDiscovery(error) => {
                write!(f, "discovering osCreateThread binding: {error:?}")
            }
            Self::CallbackFlow(error) => {
                write!(f, "deriving callback-argument contracts: {error:?}")
            }
            Self::InvalidBankName => write!(f, "bank name must be nonempty and canonical"),
            Self::DuplicateBankName { bank } => {
                write!(f, "multi-bank composition contains duplicate bank {bank}")
            }
            Self::EmptyBank => write!(f, "materialized bank is empty"),
            Self::UnalignedBank { va_start, byte_len } => write!(
                f,
                "materialized bank VA 0x{va_start:08x} and length {byte_len} must be word-aligned"
            ),
            Self::BankAddressOverflow { va_start, byte_len } => write!(
                f,
                "materialized bank 0x{va_start:08x} + {byte_len} bytes overflows the VA domain"
            ),
            Self::RootUnaligned { root } => write!(f, "root 0x{root:08x} is not word-aligned"),
            Self::RootOutsideBank { root, va_start, va_end } => write!(
                f,
                "root 0x{root:08x} is outside [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::AuthoritativeRootOnDelaySlot {
                bank,
                root,
                control_pc,
            } => write!(
                f,
                "authoritative root {bank}:0x{root:08x} is the delay slot of control instruction 0x{control_pc:08x}"
            ),
            Self::UnsupportedControlDelayEntry {
                bank,
                entry,
                control_pc,
            } => write!(
                f,
                "exact entry {bank}:0x{entry:08x} is control-shaped while also serving as the delay word of 0x{control_pc:08x}"
            ),
            Self::MissingProvenBacking { bank, va_start, va_end } => write!(
                f,
                "no proven bank image for {bank} covers [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::AmbiguousProvenBacking { bank, va_start, va_end } => write!(
                f,
                "several distinct proven bank images for {bank} prevent selecting [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::InvalidProvenBackingGeometry {
                bank,
                va_start,
                va_end,
            } => write!(
                f,
                "proven bank image for {bank} cannot represent [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::MissingEvaluatedImageReceipt {
                bank,
                receipt_sha256,
            } => write!(
                f,
                "materialized backing for {bank} has no exact evaluated-image receipt {receipt_sha256}"
            ),
            Self::AmbiguousEvaluatedImageReceipt {
                bank,
                receipt_sha256,
                count,
            } => write!(
                f,
                "materialized backing for {bank} has {count} distinct evaluated-image receipts with identity {receipt_sha256}"
            ),
            Self::BackingMaterialization {
                bank,
                backing,
                reason,
            } => write!(f, "materializing {bank} backing {backing:?}: {reason}"),
            Self::EvaluatedImageRederivation {
                bank,
                receipt_sha256,
                error,
            } => write!(
                f,
                "re-deriving {bank} evaluated image {receipt_sha256}: {error}"
            ),
            Self::MaterializedBytesMismatch { bank, backing } => write!(
                f,
                "supplied bytes for {bank} differ from re-derived backing {backing:?}"
            ),
            Self::ExecutableRangeOutsideBank { bank, va_start, va_end } => write!(
                f,
                "proven executable range for {bank} [0x{va_start:08x},0x{va_end:08x}) crosses the materialized bank boundary"
            ),
            Self::Coverage(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<OwnerProofCoverageError> for SnapshotError {
    fn from(error: OwnerProofCoverageError) -> Self {
        Self::Coverage(error)
    }
}

/// Compose one byte-verified, proven bank image through
/// exact-owner proof, using only in-bank authority. Behavior and signature are
/// preserved for existing callers; the multi-bank path is a separate entry
/// point that adds cross-bank authority without touching this one.
pub fn compose_materialized_bank_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    input: MaterializedBankInput<'_>,
) -> Result<ProgramSnapshotV1, SnapshotError> {
    let mut snapshots =
        compose_materialized_bank_validated_v2(rom, base_facts, input)?.into_diagnostic_snapshots();
    Ok(snapshots
        .pop()
        .expect("single-bank validated composition produced no snapshot"))
}

/// Compose one bank and retain opaque execution authority for the exact
/// byte-verified V2 result.
pub fn compose_materialized_bank_validated_v2(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    input: MaterializedBankInput<'_>,
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    let projection_index =
        FactProjectionIndex::new(base_facts).map_err(SnapshotError::FactProjection)?;
    let projected_facts = projection_index.project(input.bank);
    let mut prepared = prepare_materialized_bank(
        rom,
        &projected_facts,
        input,
        MaterializedImageLimitsV1::default(),
    )?;
    refresh_prepared_traversal_closure(&mut prepared, &projected_facts)?;
    let authorized_roots = prepared.authorized_callable_roots.clone();
    Ok(ValidatedComposedSnapshotsV2 {
        snapshots: vec![finish_materialized_bank(rom, prepared, &authorized_roots)?],
    })
}

/// Validate, byte-verify, and build the CFG closure of one materialized bank,
/// integrating its in-bank direct-call and indirect-transfer facts. Owner
/// proof is deferred to [`finish_materialized_bank`] so a caller may first
/// gather cross-bank authority from every prepared bank.
fn prepare_materialized_bank(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    input: MaterializedBankInput<'_>,
    materialized_image_limits: MaterializedImageLimitsV1,
) -> Result<PreparedBank, SnapshotError> {
    if input.bank.is_empty() || input.bank.trim() != input.bank {
        return Err(SnapshotError::InvalidBankName);
    }
    if input.bytes.is_empty() {
        return Err(SnapshotError::EmptyBank);
    }
    if !input.va_start.is_multiple_of(4) || !input.bytes.len().is_multiple_of(4) {
        return Err(SnapshotError::UnalignedBank {
            va_start: input.va_start,
            byte_len: input.bytes.len(),
        });
    }
    let byte_len =
        u32::try_from(input.bytes.len()).map_err(|_| SnapshotError::BankAddressOverflow {
            va_start: input.va_start,
            byte_len: input.bytes.len(),
        })?;
    let va_end =
        input
            .va_start
            .checked_add(byte_len)
            .ok_or(SnapshotError::BankAddressOverflow {
                va_start: input.va_start,
                byte_len: input.bytes.len(),
            })?;

    let mut roots = BTreeSet::new();
    for &root in input
        .seed_roots
        .iter()
        .chain(base_facts.proven_function_entries(input.bank).iter())
    {
        if !root.is_multiple_of(4) {
            return Err(SnapshotError::RootUnaligned { root });
        }
        if root < input.va_start || root >= va_end {
            return Err(SnapshotError::RootOutsideBank {
                root,
                va_start: input.va_start,
                va_end,
            });
        }
        roots.insert(root);
    }

    let backing =
        match base_facts.resolve_proven_bank_backing_span(input.bank, input.va_start, va_end) {
            BankBackingSpanResolutionV1::Missing => {
                return Err(SnapshotError::MissingProvenBacking {
                    bank: input.bank.into(),
                    va_start: input.va_start,
                    va_end,
                });
            }
            BankBackingSpanResolutionV1::Ambiguous => {
                return Err(SnapshotError::AmbiguousProvenBacking {
                    bank: input.bank.into(),
                    va_start: input.va_start,
                    va_end,
                });
            }
            BankBackingSpanResolutionV1::InvalidGeometry => {
                return Err(SnapshotError::InvalidProvenBackingGeometry {
                    bank: input.bank.into(),
                    va_start: input.va_start,
                    va_end,
                });
            }
            BankBackingSpanResolutionV1::Unique(backing) => backing,
        };
    let rederived = materialize_selected_backing(
        rom,
        base_facts,
        input.bank,
        input.va_start,
        va_end,
        &backing,
        materialized_image_limits,
    )?;
    if rederived != input.bytes {
        return Err(SnapshotError::MaterializedBytesMismatch {
            bank: input.bank.into(),
            backing,
        });
    }
    for (exec_start, exec_end) in base_facts.proven_executable_ranges(input.bank) {
        let touches = exec_start < va_end && exec_end > input.va_start;
        if touches && (exec_start < input.va_start || exec_end > va_end) {
            return Err(SnapshotError::ExecutableRangeOutsideBank {
                bank: input.bank.into(),
                va_start: exec_start,
                va_end: exec_end,
            });
        }
    }

    let traversal_roots = roots.clone();
    let authority_roots = base_facts.proven_hardware_function_entries(input.bank);
    let semantic_callable_entries = derive_semantic_callable_argument_roots(
        input.bank,
        input.bytes,
        input.va_start,
        va_end,
        &authority_roots,
    )?;
    let authorized_callable_roots = semantic_callable_root_set(&semantic_callable_entries);
    let authority_closure = build_cross_bank_authority_closure(
        base_facts,
        input.bank,
        input.bytes,
        input.va_start,
        &authorized_callable_roots,
        &BTreeSet::new(),
    );
    let mut facts = base_facts.clone();
    let closure = authority_closure.clone();
    integrate_closure_facts(&mut facts, input.bank, input.va_start, va_end, &closure);
    integrate_semantic_callable_entry_facts(&mut facts, &semantic_callable_entries)?;

    Ok(PreparedBank {
        bank: input.bank.into(),
        va_start: input.va_start,
        va_end,
        bytes: input.bytes.to_vec(),
        digest: BankInputDigestV1 {
            bank: input.bank.into(),
            va_start: input.va_start,
            va_end,
            backing,
            bytes_sha256: sha256_hex(input.bytes),
        },
        facts,
        closure,
        traversal_roots,
        semantic_callable_entries,
        authorized_callable_roots,
        cross_bank_reachability_roots: BTreeSet::new(),
        semantic_cross_bank_roots: BTreeSet::new(),
        authority_closure,
    })
}

fn materialize_selected_backing(
    rom: &NormalizedRom,
    facts: &FactDb,
    bank: &str,
    va_start: u32,
    va_end: u32,
    backing: &BankBackingSpanV1,
    limits: MaterializedImageLimitsV1,
) -> Result<Vec<u8>, SnapshotError> {
    materialize_backing_span_v1(
        rom,
        Some(facts),
        bank,
        va_start,
        va_end,
        backing,
        limits,
        &mut MaterializedBackingSpanCacheV1::default(),
    )
    .map_err(|error| match error {
        MaterializedBackingSpanErrorV1::MissingEvaluatedImageReceipt { receipt_sha256 } => {
            SnapshotError::MissingEvaluatedImageReceipt {
                bank: bank.to_owned(),
                receipt_sha256,
            }
        }
        MaterializedBackingSpanErrorV1::AmbiguousEvaluatedImageReceipt {
            receipt_sha256,
            count,
        } => SnapshotError::AmbiguousEvaluatedImageReceipt {
            bank: bank.to_owned(),
            receipt_sha256,
            count,
        },
        MaterializedBackingSpanErrorV1::EvaluatedImageRederivation {
            receipt_sha256,
            error,
        } => SnapshotError::EvaluatedImageRederivation {
            bank: bank.to_owned(),
            receipt_sha256,
            error,
        },
        MaterializedBackingSpanErrorV1::InvalidGeometry => {
            SnapshotError::InvalidProvenBackingGeometry {
                bank: bank.to_owned(),
                va_start,
                va_end,
            }
        }
        error => SnapshotError::BackingMaterialization {
            bank: bank.to_owned(),
            backing: backing.clone(),
            reason: format!("{error:?}"),
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SemanticCallableEntry {
    target: BankAddr,
    evidence: FunctionEntryEvidence,
}

fn semantic_callable_root_set(entries: &BTreeSet<SemanticCallableEntry>) -> BTreeSet<u32> {
    entries.iter().map(|entry| entry.target.pc).collect()
}

/// Inductively recover same-bank callable entries from reachable calls whose
/// pointer arguments have mechanically proven transfer semantics, retaining
/// the exact contract and call provenance that authorized each target.
///
/// The proof chain starts only at typed, proven hardware entries. Ordinary
/// traversal seeds deliberately do not enter this function: otherwise a
/// caller-supplied analysis hint could bootstrap callable authority. The
/// public `osCreateThread` contract supplies `$a2`; for other functions an
/// o32 argument must reach a reachable `jalr` target through exact register or
/// stack flow. Pointer slices are cached per `(callee, argument)`; only call
/// and delay words proven by the current authority closure are admitted.
fn derive_semantic_callable_argument_roots(
    bank: &str,
    bytes: &[u8],
    va_start: u32,
    va_end: u32,
    hardware_roots: &[u32],
) -> Result<BTreeSet<SemanticCallableEntry>, SnapshotError> {
    if hardware_roots.is_empty() {
        return Ok(BTreeSet::new());
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte bank word")))
        .collect();
    let binding = match discover_os_create_thread_host_binding(&words, va_start) {
        Ok(binding) => Some(binding),
        Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
            symbol: HostBindingSymbol::OsCreateThread,
            candidates,
        }) if candidates.is_empty() => None,
        Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
            symbol: HostBindingSymbol::OsCreateThread,
            candidates,
        }) => {
            return Err(SnapshotError::AmbiguousOsCreateThreadBinding {
                bank: bank.to_owned(),
                candidates,
            });
        }
        Err(error) => return Err(SnapshotError::HostBindingDiscovery(error)),
    };
    let mut requested_contracts = BTreeMap::<(u32, u8), BTreeSet<SemanticCallableContract>>::new();
    if let Some(binding) = binding {
        requested_contracts
            .entry((binding.vram, 6u8))
            .or_default()
            .insert(SemanticCallableContract::OsCreateThread);
    }

    let mut slice_cache = BTreeMap::new();
    let mut roots: BTreeSet<u32> = hardware_roots.iter().copied().collect();
    let mut authorized = BTreeSet::new();
    let mut entries = BTreeSet::new();
    loop {
        let root_vec: Vec<u32> = roots.iter().copied().collect();
        let closure = build_cfg_value_set_closed(bank, bytes, va_start, &root_vec);
        for contract in discover_callback_argument_contracts(&closure.cfg, bytes, va_start)
            .map_err(SnapshotError::CallbackFlow)?
        {
            requested_contracts
                .entry((contract.callee, contract.pointer_arg_register))
                .or_default()
                .insert(SemanticCallableContract::ArgumentToJalr {
                    jalr_sites: contract
                        .jalr_sites
                        .into_iter()
                        .map(|pc| BankAddr::new(bank, pc))
                        .collect(),
                });
        }
        for contract in discover_callback_registry_contracts(&closure.cfg, bytes, va_start)
            .map_err(SnapshotError::CallbackFlow)?
        {
            requested_contracts
                .entry((contract.registrar, contract.pointer_arg_register))
                .or_default()
                .insert(SemanticCallableContract::CallbackRegistry {
                    dispatcher: BankAddr::new(bank, contract.dispatcher),
                    callback_store_site: BankAddr::new(bank, contract.callback_store_site),
                    list_insert_site: BankAddr::new(bank, contract.list_insert_site),
                    jalr_site: BankAddr::new(bank, contract.jalr_site),
                });
        }
        let pending: Vec<_> = requested_contracts
            .keys()
            .filter(|contract| !slice_cache.contains_key(*contract))
            .map(|&(callee, register)| (VirtualAddress::new(callee), register))
            .collect();
        for slice in slice_pointer_arg_call_contracts(
            &words,
            VirtualAddress::new(va_start),
            0x0080_0000,
            &pending,
        )
        .map_err(SnapshotError::CallableArgumentSlice)?
        {
            slice_cache
                .entry((slice.callee.get(), slice.pointer_register))
                .or_insert_with(Vec::new)
                .push(slice);
        }
        for &(callee, register) in &pending {
            slice_cache.entry((callee.get(), register)).or_default();
        }
        let mut added = false;
        for slice in slice_cache.values().flatten() {
            let call_pc = slice.call_pc.get();
            let Some(delay_pc) = call_pc.checked_add(4) else {
                continue;
            };
            if closure.cfg.word_class.get(&call_pc) != Some(&crate::cfg::WordClass::ProvenCode)
                || closure.cfg.word_class.get(&delay_pc) != Some(&crate::cfg::WordClass::ProvenCode)
            {
                continue;
            }
            let Some(target) = slice.pointer.proven().map(|target| target.get()) else {
                continue;
            };
            if !target.is_multiple_of(4) || target < va_start || target >= va_end {
                continue;
            }
            let contract_key = (slice.callee.get(), slice.pointer_register);
            let Some(contracts) = requested_contracts.get(&contract_key) else {
                continue;
            };
            for contract in contracts {
                entries.insert(SemanticCallableEntry {
                    target: BankAddr::new(bank, target),
                    evidence: FunctionEntryEvidence::SemanticCallableArgument {
                        call_site: BankAddr::new(bank, call_pc),
                        callee: BankAddr::new(bank, slice.callee.get()),
                        pointer_register: slice.pointer_register,
                        contract: contract.clone(),
                    },
                });
            }
            if authorized.insert(target) {
                roots.insert(target);
                added = true;
            }
        }
        if !added {
            return Ok(entries);
        }
        debug_assert!(roots.len() <= words.len());
    }
}

/// A byte-verified, closure-built bank awaiting owner proof. Separating this
/// from owner proof lets the multi-bank composer build every bank's closure
/// first, collect cross-bank direct-call authority, then prove each bank's
/// owners against the enriched authority — without changing single-bank
/// behavior, which just proves immediately with no external authority.
struct PreparedBank {
    bank: String,
    va_start: u32,
    va_end: u32,
    bytes: Vec<u8>,
    digest: BankInputDigestV1,
    /// Base facts already enriched with this bank's in-bank direct-call and
    /// indirect-transfer analysis facts.
    facts: FactDb,
    closure: ClosureResult,
    /// Original closure roots before semantic callback targets are added.
    /// Cross-bank semantic expansion rebuilds from this exact set so a prior
    /// analysis result cannot become its own authority.
    traversal_roots: BTreeSet<u32>,
    /// Recomputed, typed semantic proof records. These stay prepared-bank
    /// local so a serialized snapshot can never bootstrap a later run.
    semantic_callable_entries: BTreeSet<SemanticCallableEntry>,
    /// Same-bank callable roots derived inside byte-verified composition from
    /// hardware-entry authority and mechanically proven callable arguments.
    authorized_callable_roots: BTreeSet<u32>,
    /// Reachability roots conferred by exact cross-bank calls whose target
    /// resolves to exactly one prepared generation. Overlapping target VAs
    /// remain blocked until activation compatibility is typed authority.
    cross_bank_reachability_roots: BTreeSet<u32>,
    /// Cross-bank roots whose target identity is unique. Only this subset may
    /// seed semantic callback-argument derivation.
    semantic_cross_bank_roots: BTreeSet<u32>,
    /// Authority-only closure used by cross-bank proof. It excludes traversal
    /// hints and is retained until this bank gains another authoritative root.
    authority_closure: ClosureResult,
}

fn expand_prepared_cross_bank_authority(
    prepared: &mut PreparedBank,
    base_facts: &FactDb,
    reachability_roots: &BTreeSet<u32>,
    semantic_roots: &BTreeSet<u32>,
) -> Result<(), SnapshotError> {
    if reachability_roots.is_empty()
        && semantic_roots.is_empty()
        && prepared.cross_bank_reachability_roots.is_empty()
    {
        return Ok(());
    }
    let prior_reachability = prepared.cross_bank_reachability_roots.clone();
    let prior_semantic = prepared.semantic_cross_bank_roots.clone();
    prepared
        .cross_bank_reachability_roots
        .extend(reachability_roots.iter().copied());
    prepared
        .semantic_cross_bank_roots
        .extend(semantic_roots.iter().copied());
    let mut authority_roots: BTreeSet<u32> = base_facts
        .proven_hardware_function_entries(prepared.bank.as_str())
        .into_iter()
        .collect();
    authority_roots.extend(prepared.semantic_cross_bank_roots.iter().copied());
    let semantic_entries = derive_semantic_callable_argument_roots(
        prepared.bank.as_str(),
        &prepared.bytes,
        prepared.va_start,
        prepared.va_end,
        &authority_roots.into_iter().collect::<Vec<_>>(),
    )?;
    let authorized = semantic_callable_root_set(&semantic_entries);
    if semantic_entries == prepared.semantic_callable_entries
        && prior_reachability == prepared.cross_bank_reachability_roots
        && prior_semantic == prepared.semantic_cross_bank_roots
    {
        return Ok(());
    }

    prepared.semantic_callable_entries = semantic_entries;
    prepared.authorized_callable_roots = authorized;
    let authority_closure = build_cross_bank_authority_closure(
        base_facts,
        prepared.bank.as_str(),
        &prepared.bytes,
        prepared.va_start,
        &prepared.authorized_callable_roots,
        &prepared.cross_bank_reachability_roots,
    );
    prepared.authority_closure = authority_closure;
    validate_authoritative_delay_slot_roots(prepared)
}

fn authority_delay_slots(
    closure: &ClosureResult,
    bank_bytes: &[u8],
    va_start: u32,
) -> BTreeMap<u32, u32> {
    closure
        .cfg
        .blocks
        .iter()
        .filter_map(|block| {
            let (delay_slot, control_pc) = match &block.terminator {
                crate::cfg::BlockTerminator::Tail { .. }
                | crate::cfg::BlockTerminator::Call { .. }
                | crate::cfg::BlockTerminator::Branch { .. }
                | crate::cfg::BlockTerminator::BranchLikely { .. }
                | crate::cfg::BlockTerminator::Return
                | crate::cfg::BlockTerminator::Indirect { .. }
                | crate::cfg::BlockTerminator::ResolvedIndirect { .. } => {
                    (block.end_va.checked_sub(4)?, block.end_va.checked_sub(8)?)
                }
                crate::cfg::BlockTerminator::Fallthrough { next } if *next == block.end_va => {
                    let control_pc = block.end_va.checked_sub(4)?;
                    let offset = usize::try_from(control_pc.checked_sub(va_start)?).ok()?;
                    let word = u32::from_be_bytes(
                        bank_bytes
                            .get(offset..offset.checked_add(4)?)?
                            .try_into()
                            .ok()?,
                    );
                    fn64_cpu_runtime::decode(word)
                        .has_delay_slot()
                        .then_some((block.end_va, control_pc))?
                }
                _ => return None,
            };
            (closure.cfg.word_class.get(&control_pc) == Some(&crate::cfg::WordClass::ProvenCode))
                .then_some(())?;
            (closure.cfg.word_class.get(&delay_slot) == Some(&crate::cfg::WordClass::ProvenCode))
                .then_some(())?;
            Some((delay_slot, control_pc))
        })
        .collect()
}

fn validate_authoritative_delay_slot_roots(prepared: &PreparedBank) -> Result<(), SnapshotError> {
    if let Some(entry) = prepared
        .authority_closure
        .cfg
        .unsupported_delay_entries
        .first()
    {
        return Err(SnapshotError::UnsupportedControlDelayEntry {
            bank: prepared.bank.clone(),
            entry: entry.entry_va,
            control_pc: entry.control_pc,
        });
    }
    let authority_roots = prepared
        .authority_closure
        .cfg
        .proven_roots
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if let Some((root, control_pc)) = authority_delay_slots(
        &prepared.authority_closure,
        &prepared.bytes,
        prepared.va_start,
    )
    .into_iter()
    .find(|(delay_slot, _)| {
        authority_roots.contains(delay_slot)
            && !prepared
                .authority_closure
                .cfg
                .plain_delay_entry_aliases
                .iter()
                .any(|alias| alias.entry_va == *delay_slot)
    }) {
        return Err(SnapshotError::AuthoritativeRootOnDelaySlot {
            bank: prepared.bank.clone(),
            root,
            control_pc,
        });
    }
    Ok(())
}

fn refresh_prepared_traversal_closure(
    prepared: &mut PreparedBank,
    base_facts: &FactDb,
) -> Result<(), SnapshotError> {
    validate_authoritative_delay_slot_roots(prepared)?;
    let mut roots = prepared.traversal_roots.clone();
    roots.extend(prepared.cross_bank_reachability_roots.iter().copied());
    roots.extend(prepared.authorized_callable_roots.iter().copied());
    for (delay_slot, _) in authority_delay_slots(
        &prepared.authority_closure,
        &prepared.bytes,
        prepared.va_start,
    ) {
        if prepared
            .authority_closure
            .cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| alias.entry_va == delay_slot)
        {
            continue;
        }
        if !roots.contains(&delay_slot) {
            continue;
        }
        roots.remove(&delay_slot);
    }
    let closure = build_cfg_value_set_closed(
        prepared.bank.as_str(),
        &prepared.bytes,
        prepared.va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    );
    let mut facts = base_facts.clone();
    integrate_closure_facts(
        &mut facts,
        prepared.bank.as_str(),
        prepared.va_start,
        prepared.va_end,
        &closure,
    );
    integrate_semantic_callable_entry_facts(&mut facts, &prepared.semantic_callable_entries)?;
    prepared.facts = facts;
    prepared.closure = closure;
    Ok(())
}

/// Partition, prove owners (with any externally-conferred callable roots),
/// derive executable evidence, and package one bank's snapshot.
fn finish_materialized_bank(
    rom: &NormalizedRom,
    prepared: PreparedBank,
    external_authorized_roots: &BTreeSet<u32>,
) -> Result<ProgramSnapshotV1, SnapshotError> {
    let PreparedBank {
        va_start,
        digest,
        mut facts,
        mut closure,
        authority_closure,
        bytes,
        ..
    } = prepared;

    ensure_authoritative_block_leaders(&mut closure, external_authorized_roots);
    let callable_boundaries =
        authoritative_partition_entries(&closure, &facts, external_authorized_roots);
    let partition = if external_authorized_roots.is_empty() {
        // Preserve the resident/single-bank construction byte-for-byte. The
        // authority-aware rule is needed only when composition adds callable
        // boundaries the bank-local CFG could not know.
        partition(&closure.cfg)
    } else {
        partition_with_authorized_splits(
            &closure.cfg,
            external_authorized_roots,
            &callable_boundaries,
        )
    };
    // External roots were derived from authority-reached sibling calls.
    // `ensure_authoritative_block_leaders` above makes each one representable
    // in this broad CFG; the capability constructor independently requires
    // alignment and a proven mapping in this bank. The same capability also
    // records which indirect sites are authority-reachable, so candidate-only
    // broad sites cannot withhold exactness from unrelated owners.
    let owner_proof_authority = OwnerProofAuthority::from_authority_closure(
        &authority_closure,
        &facts,
        external_authorized_roots,
    );
    // First owner pass supplies entry authority to block proof; executable
    // evidence may not exist yet, so its `NotProvenExecutable` blockers are
    // provisional and it is never serialized.
    let authority_proof = prove_exact_owners_with_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        &owner_proof_authority,
    );
    // Candidate traversal roots may split broad partition geometry, but they
    // cannot confer execution authority. Project only exact claimant roots
    // from the separately built authority closure onto fully-contained broad
    // blocks; candidate owners remain candidates.
    let authority_reachability =
        AuthorityReachabilityProjection::from_authority_closure(&authority_closure);
    let block_proof = prove_reachable_blocks_with_authority_projection(
        &closure.cfg,
        &partition,
        &authority_proof,
        &facts,
        &authority_reachability,
    );
    // Bytes reached as proven code from an authoritative entry are proven
    // executable — exactly those bytes, never the gaps between them. Record
    // them as typed facts, then re-prove owners against the enriched
    // evidence. Both owner passes consume the same authority-closure-derived
    // owner-proof capability, so broad candidate calls cannot become authority
    // and broad candidate-only indirect sites cannot become blockers through
    // this executable-range feedback. Block proof itself does not consume
    // executable ranges, so no further fixpoint iteration exists.
    conclude_reached_executable_ranges(&block_proof, &mut facts);
    let owner_proof = prove_exact_owners_with_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        &owner_proof_authority,
    );
    let blocker_histogram = owner_blocker_histogram(&owner_proof);
    let coverage = report_with_owner_proofs(rom.len(), &facts, std::slice::from_ref(&owner_proof))?;
    let bank = BankSnapshotV1 {
        input: digest,
        authority_closure,
        closure,
        partition,
        owner_proof,
        block_proof,
        blocker_histogram,
    };
    debug_assert_eq!(bank.input.bank, bank.owner_proof.bank);
    Ok(ProgramSnapshotV1 {
        schema_version: PROGRAM_SNAPSHOT_SCHEMA_V6,
        normalized_rom_sha256: rom.sha256.clone(),
        facts,
        banks: vec![bank],
        coverage,
    })
}

fn integrate_closure_facts(
    facts: &mut FactDb,
    bank: &str,
    va_start: u32,
    va_end: u32,
    closure: &ClosureResult,
) {
    for &(source_pc, target_pc) in &closure.cfg.direct_calls {
        // A target outside this materialized bank has no mechanically known
        // bank identity yet. The CFG retains that edge; the canonical fact
        // log waits for load-image resolution instead of guessing a bank.
        if target_pc >= va_start && target_pc < va_end {
            insert_unique(
                facts,
                Fact::DirectCall {
                    source: BankAddr::new(bank, source_pc),
                    target: BankAddr::new(bank, target_pc),
                },
            );
        }
    }
    for resolution in &closure.indirect {
        insert_unique(
            facts,
            Fact::IndirectTransferAnalysis {
                site: BankAddr::new(bank, resolution.site_pc),
                via_call: resolution.via_call,
                state: match resolution.state {
                    IndirectProofState::Exhaustive => IndirectTransferState::Exhaustive,
                    IndirectProofState::Bounded => IndirectTransferState::Bounded,
                    IndirectProofState::Open => IndirectTransferState::Open,
                },
                kind: resolution.kind.map(|kind| match kind {
                    IndirectResolutionKind::Constant => IndirectTransferKind::Constant,
                    IndirectResolutionKind::MemoryValueSet => IndirectTransferKind::MemoryValueSet,
                    IndirectResolutionKind::JumpTable => IndirectTransferKind::JumpTable,
                }),
                targets: resolution.targets.clone(),
                memory_sources: resolution.memory_sources.clone(),
            },
        );
    }
}

fn integrate_semantic_callable_entry_facts(
    facts: &mut FactDb,
    entries: &BTreeSet<SemanticCallableEntry>,
) -> Result<(), SnapshotError> {
    for entry in entries {
        let claim = Fact::FunctionEntryClaim {
            target: entry.target.clone(),
            detector: CandidateDetector::SemanticCallableArgument,
            evidence: entry.evidence.clone(),
            proposed_state: ProofState::Proven,
        };
        let fact_index = match facts.facts().iter().position(|fact| fact == &claim) {
            Some(index) => index,
            None => facts.insert(claim),
        };
        let subject = function_entry_subject(&entry.target);
        let mut justifications = facts
            .conclusion(&subject)
            .map(|conclusion| conclusion.justified_by.clone())
            .unwrap_or_default();
        justifications.push(fact_index);
        justifications.sort_unstable();
        justifications.dedup();
        facts
            .conclude(
                subject,
                ProofState::Proven,
                justifications,
                "semantic_callable_argument_from_authority_closure",
            )
            .map_err(|error| SnapshotError::SemanticEntryConclusion {
                target: entry.target.clone(),
                detail: error.to_string(),
            })?;
    }
    Ok(())
}

fn ensure_authoritative_block_leaders(
    closure: &mut ClosureResult,
    external_authorized_roots: &BTreeSet<u32>,
) {
    for &entry in external_authorized_roots {
        if !entry.is_multiple_of(4)
            || closure
                .cfg
                .plain_delay_entry_aliases
                .iter()
                .any(|alias| alias.entry_va == entry)
            || closure
                .cfg
                .blocks
                .iter()
                .any(|block| block.start_va == entry)
        {
            continue;
        }
        let Some(index) = closure
            .cfg
            .blocks
            .iter()
            .position(|block| block.start_va < entry && entry < block.end_va)
        else {
            continue;
        };
        let mut suffix = closure.cfg.blocks[index].clone();
        suffix.start_va = entry;
        closure.cfg.blocks[index].end_va = entry;
        closure.cfg.blocks[index].terminator = crate::cfg::BlockTerminator::Tail { target: entry };
        closure.cfg.blocks.push(suffix);
    }
    closure.cfg.blocks.sort_by_key(|block| block.start_va);
    for &entry in external_authorized_roots {
        if (closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == entry)
            || closure
                .cfg
                .plain_delay_entry_aliases
                .iter()
                .any(|alias| alias.entry_va == entry))
            && !closure.cfg.proven_roots.contains(&entry)
        {
            closure.cfg.proven_roots.push(entry);
        }
    }
    for block in &mut closure.cfg.blocks {
        if matches!(
            block.terminator,
            crate::cfg::BlockTerminator::Fallthrough { next }
                if external_authorized_roots.contains(&next)
        ) {
            block.terminator = crate::cfg::BlockTerminator::Tail {
                target: block.end_va,
            };
        }
    }
}

fn authoritative_partition_entries(
    closure: &ClosureResult,
    facts: &FactDb,
    external_authorized_roots: &BTreeSet<u32>,
) -> BTreeSet<u32> {
    let cfg = &closure.cfg;
    let mut entries: BTreeSet<u32> = facts
        .proven_function_entries(&cfg.bank)
        .into_iter()
        .collect();
    entries.extend(external_authorized_roots.iter().copied());
    entries.extend(cfg.direct_calls.iter().filter_map(|&(source, target)| {
        (cfg.word_class.get(&source) == Some(&crate::cfg::WordClass::ProvenCode)).then_some(target)
    }));
    for block in &cfg.blocks {
        let crate::cfg::BlockTerminator::ResolvedIndirect {
            targets,
            via_call: true,
        } = &block.terminator
        else {
            continue;
        };
        if block.end_va >= block.start_va.saturating_add(8)
            && cfg.word_class.get(&(block.end_va - 8)) == Some(&crate::cfg::WordClass::ProvenCode)
            && cfg.word_class.get(&(block.end_va - 4)) == Some(&crate::cfg::WordClass::ProvenCode)
        {
            entries.extend(targets.iter().copied());
        }
    }
    entries
}

fn insert_unique(db: &mut FactDb, fact: Fact) {
    if !db.facts().contains(&fact) {
        db.insert(fact);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

mod multi_bank;
pub use multi_bank::*;

#[cfg(test)]
mod tests;
