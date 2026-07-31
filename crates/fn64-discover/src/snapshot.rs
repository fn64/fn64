//! Deterministic composition of discovery's existing phase outputs into one
//! proof-carrying program artifact.
//!
//! The Rust envelope retains its historical `ProgramSnapshotV1` name, while
//! its serialized schema is V5 after preserving typed semantic callable-entry
//! provenance alongside the authority-rooted and broad traversal closures.
//! It materializes one proven physical ROM-backed bank and verifies
//! the supplied bytes against [`NormalizedRom`] before analysis, runs Phase
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
    function_entry_subject, BankAddr, CandidateDetector, Fact, FactDb, FactProjectionError,
    FactProjectionIndex, FunctionEntryEvidence, IndirectTransferKind, IndirectTransferState,
    ProofState, RomAddressSpace, SemanticCallableContract,
};
use crate::generation_topology::{
    dense_aot_pack_sha256_v1, CatalogBoundExactTransferV1, ExactTransferKindV1,
    GenerationTopologyV1,
};
use crate::host_bindings::{
    discover_os_create_thread_host_binding, HostBindingDiscoveryError, HostBindingSymbol,
};
use crate::loaders::VirtualAddress;
use crate::owner_proof::{
    exact_authority_direct_call, exhaustive_authority_call_site,
    prove_exact_owners_with_callable_authority, AuthoritativeCallableEntries, OwnerAssessment,
    OwnerBlocker, OwnerProofReport,
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

/// V5 retains typed semantic callable-entry provenance in addition to the
/// authority-rooted CFG and broad exploratory traversal CFG. Execution-closure
/// consumers must never promote a caller-supplied traversal hint into evidence
/// that a transfer can execute.
pub const PROGRAM_SNAPSHOT_SCHEMA_V5: u32 = 5;
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
    pub rom_space: RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
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
    MissingRomBacking,
    MultipleRomBackings,
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
            OwnerBlocker::MissingRomBacking => Self::MissingRomBacking,
            OwnerBlocker::MultipleRomBackings => Self::MultipleRomBackings,
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
    MissingProvenMapping {
        bank: String,
        va_start: u32,
        va_end: u32,
    },
    AmbiguousProvenMapping {
        bank: String,
        va_start: u32,
        va_end: u32,
        count: usize,
    },
    MalformedPhysicalMapping {
        bank: String,
    },
    RomBackingOutsideImage {
        rom_start: u32,
        rom_end: u32,
    },
    MaterializedBytesMismatch {
        bank: String,
        rom_start: u32,
        rom_end: u32,
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
            Self::MissingProvenMapping { bank, va_start, va_end } => write!(
                f,
                "no proven mapping for {bank} covers [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::AmbiguousProvenMapping { bank, va_start, va_end, count } => write!(
                f,
                "{count} proven mappings for {bank} cover [0x{va_start:08x},0x{va_end:08x})"
            ),
            Self::MalformedPhysicalMapping { bank } => {
                write!(f, "proven physical mapping for {bank} has unequal ROM and VA extents")
            }
            Self::RomBackingOutsideImage { rom_start, rom_end } => write!(
                f,
                "physical backing [0x{rom_start:x},0x{rom_end:x}) is outside the normalized ROM"
            ),
            Self::MaterializedBytesMismatch { bank, rom_start, rom_end } => write!(
                f,
                "materialized bytes for {bank} differ from normalized ROM [0x{rom_start:x},0x{rom_end:x})"
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

#[derive(Clone, Copy)]
struct CoveringMapping {
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Compose one byte-verified, proven physical ROM-backed bank through
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
    let mut prepared = prepare_materialized_bank(rom, &projected_facts, input)?;
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

    let mappings: Vec<_> = base_facts
        .proven_rom_mappings()
        .into_iter()
        .filter_map(|fact| {
            let Fact::RomMapping {
                bank,
                rom_space,
                rom_start,
                rom_end,
                va_start,
                va_end: map_va_end,
            } = fact
            else {
                return None;
            };
            (bank == input.bank && *va_start <= input.va_start && *map_va_end >= va_end).then_some(
                CoveringMapping {
                    rom_space: *rom_space,
                    rom_start: *rom_start,
                    rom_end: *rom_end,
                    va_start: *va_start,
                    va_end: *map_va_end,
                },
            )
        })
        .collect();
    let mapping = match mappings.as_slice() {
        [] => {
            return Err(SnapshotError::MissingProvenMapping {
                bank: input.bank.into(),
                va_start: input.va_start,
                va_end,
            });
        }
        [mapping] => *mapping,
        mappings => {
            return Err(SnapshotError::AmbiguousProvenMapping {
                bank: input.bank.into(),
                va_start: input.va_start,
                va_end,
                count: mappings.len(),
            });
        }
    };
    // A DMA-loaded overlay's VA extent may exceed its ROM extent: the trailing
    // `.bss` is allocated at load time and has no backing bytes. Accept that
    // shape, but never let a materialized window reach past the ROM-backed
    // prefix (checked once `rom_end` is known below).
    match (
        mapping.rom_end.checked_sub(mapping.rom_start),
        mapping.va_end.checked_sub(mapping.va_start),
    ) {
        (Some(rom_extent), Some(va_extent)) if rom_extent <= va_extent => {}
        _ => {
            return Err(SnapshotError::MalformedPhysicalMapping {
                bank: input.bank.into(),
            });
        }
    }
    let offset = input.va_start - mapping.va_start;
    let rom_start =
        mapping
            .rom_start
            .checked_add(offset)
            .ok_or(SnapshotError::MalformedPhysicalMapping {
                bank: input.bank.into(),
            })?;
    let rom_end =
        rom_start
            .checked_add(byte_len)
            .ok_or(SnapshotError::MalformedPhysicalMapping {
                bank: input.bank.into(),
            })?;
    // The requested window must stay inside the mapping's ROM-backed bytes; a
    // bank that tried to include its `.bss` tail is rejected loudly rather than
    // silently materializing bytes that the ROM does not carry.
    if rom_end > mapping.rom_end {
        return Err(SnapshotError::MalformedPhysicalMapping {
            bank: input.bank.into(),
        });
    }
    // Materialize through the shared ROM-range resolver: a physical bank is
    // sliced from the image, while a VROM (DMA-loaded) overlay is resolved and
    // byte-verified through its one proven file-table record. Both therefore
    // reach the same materialized-bytes check below.
    let backing =
        crate::banks::materialize_rom_range(rom, base_facts, mapping.rom_space, rom_start, rom_end)
            .map_err(|_| SnapshotError::RomBackingOutsideImage { rom_start, rom_end })?;
    let backing = backing.bytes.as_slice();
    if backing != input.bytes {
        return Err(SnapshotError::MaterializedBytesMismatch {
            bank: input.bank.into(),
            rom_start,
            rom_end,
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
            rom_space: mapping.rom_space,
            rom_start,
            rom_end,
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
                    fn64_recomp_rs::decode(word)
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
    // alignment and a proven mapping in this bank.
    let callable_authority = AuthoritativeCallableEntries::from_authority_closure(
        &authority_closure,
        &facts,
        external_authorized_roots,
    );
    // First owner pass supplies entry authority to block proof; executable
    // evidence may not exist yet, so its `NotProvenExecutable` blockers are
    // provisional and it is never serialized.
    let authority_proof = prove_exact_owners_with_callable_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        &callable_authority,
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
    // callable capability, so broad candidate calls cannot become authority
    // through this executable-range feedback. Block proof itself does not
    // consume executable ranges, so no further fixpoint iteration exists.
    conclude_reached_executable_ranges(&block_proof, &mut facts);
    let owner_proof = prove_exact_owners_with_callable_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        &callable_authority,
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
        schema_version: PROGRAM_SNAPSHOT_SCHEMA_V5,
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

/// A proven callable transfer in one bank whose target lands inside another
/// bank's proven VA range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CrossBankAuthoritativeCall {
    source_bank: String,
    source_pc: u32,
    target_pc: u32,
    kind: CrossBankAuthoritativeCallKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CrossBankAuthoritativeCallKind {
    Direct,
    ExhaustiveResolved,
}

#[derive(Debug)]
struct BankInterval {
    input_index: usize,
    bank: String,
    va_start: u32,
    va_end: u32,
}

/// Static point-query index over prepared bank ranges. A balanced max-end tree
/// prunes subtrees whose intervals all end before the target; overlapping
/// ranges are all returned in input order.
struct BankIntervalIndex {
    intervals: Vec<BankInterval>,
    leaf_base: usize,
    max_end_tree: Vec<u32>,
}

impl BankIntervalIndex {
    fn new(prepared: &[PreparedBank]) -> Self {
        Self::from_intervals(
            prepared
                .iter()
                .enumerate()
                .map(|(input_index, bank)| BankInterval {
                    input_index,
                    bank: bank.bank.clone(),
                    va_start: bank.va_start,
                    va_end: bank.va_end,
                })
                .collect(),
        )
    }

    fn from_intervals(mut intervals: Vec<BankInterval>) -> Self {
        intervals.sort_by(|left, right| {
            (
                left.va_start,
                left.va_end,
                left.bank.as_str(),
                left.input_index,
            )
                .cmp(&(
                    right.va_start,
                    right.va_end,
                    right.bank.as_str(),
                    right.input_index,
                ))
        });
        let leaf_base = intervals.len().next_power_of_two().max(1);
        let mut max_end_tree = vec![0; leaf_base * 2];
        for (index, interval) in intervals.iter().enumerate() {
            max_end_tree[leaf_base + index] = interval.va_end;
        }
        for node in (1..leaf_base).rev() {
            max_end_tree[node] = max_end_tree[node * 2].max(max_end_tree[node * 2 + 1]);
        }
        Self {
            intervals,
            leaf_base,
            max_end_tree,
        }
    }

    fn matching_other_banks(&self, source_bank: &str, target_pc: u32) -> Vec<usize> {
        self.matching_other_banks_with_probe_count(source_bank, target_pc)
            .0
    }

    fn matching_other_banks_with_probe_count(
        &self,
        source_bank: &str,
        target_pc: u32,
    ) -> (Vec<usize>, usize) {
        let upper = self
            .intervals
            .partition_point(|interval| interval.va_start <= target_pc);
        let mut matches = Vec::new();
        let mut probes = 0;
        self.query_node(
            1,
            0,
            self.leaf_base,
            upper,
            source_bank,
            target_pc,
            &mut matches,
            &mut probes,
        );
        matches.sort_unstable();
        (matches, probes)
    }

    #[allow(clippy::too_many_arguments)]
    fn query_node(
        &self,
        node: usize,
        range_start: usize,
        range_end: usize,
        upper: usize,
        source_bank: &str,
        target_pc: u32,
        matches: &mut Vec<usize>,
        probes: &mut usize,
    ) {
        if range_start >= upper || self.max_end_tree[node] <= target_pc {
            return;
        }
        if range_end - range_start == 1 {
            *probes += 1;
            if let Some(interval) = self.intervals.get(range_start) {
                if interval.va_end > target_pc && interval.bank != source_bank {
                    matches.push(interval.input_index);
                }
            }
            return;
        }
        let midpoint = range_start + (range_end - range_start) / 2;
        self.query_node(
            node * 2,
            range_start,
            midpoint,
            upper,
            source_bank,
            target_pc,
            matches,
            probes,
        );
        self.query_node(
            node * 2 + 1,
            midpoint,
            range_end,
            upper,
            source_bank,
            target_pc,
            matches,
            probes,
        );
    }
}

#[derive(Default)]
struct SerializedByteCounter(u64);

impl Write for SerializedByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let byte_len = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("serialized byte count exceeds u64"))?;
        self.0 = self
            .0
            .checked_add(byte_len)
            .ok_or_else(|| io::Error::other("serialized byte count exceeds u64"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn check_multi_bank_limits<'a>(
    base_facts: &'a FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<FactProjectionIndex<'a>, SnapshotError> {
    let projection_index =
        FactProjectionIndex::new(base_facts).map_err(SnapshotError::FactProjection)?;
    let total_banks = u64::try_from(inputs.len()).map_err(|_| {
        SnapshotError::CompositionLimitArithmeticOverflow {
            calculation: "bank count conversion",
        }
    })?;
    let global_fact_rows = u64::try_from(projection_index.global_fact_count()).map_err(|_| {
        SnapshotError::CompositionLimitArithmeticOverflow {
            calculation: "global fact row count conversion",
        }
    })?;
    let mut projected_rows = 0u64;
    let mut projected_bytes = 0u64;
    for (bank_index, input) in inputs.iter().enumerate() {
        let projected = projection_index.project(input.bank);
        let rows = u64::try_from(projected.facts().len()).map_err(|_| {
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "projected fact row count conversion",
            }
        })?;
        projected_rows = projected_rows.checked_add(rows).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate projected fact rows",
            },
        )?;
        if projected_rows > limits.max_projected_fact_rows {
            return Err(SnapshotError::ProjectedFactRowsLimitExceeded {
                rows: projected_rows,
                limit: limits.max_projected_fact_rows,
            });
        }

        let mut serialized = SerializedByteCounter::default();
        serde_json::to_writer(&mut serialized, &projected).map_err(|error| {
            SnapshotError::ProjectedFactsSerialization {
                bank: input.bank.to_owned(),
                error: error.to_string(),
            }
        })?;
        projected_bytes = projected_bytes.checked_add(serialized.0).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate projected fact bytes",
            },
        )?;
        if projected_bytes > limits.max_projected_fact_bytes {
            let processed_banks = u64::try_from(bank_index + 1).map_err(|_| {
                SnapshotError::CompositionLimitArithmeticOverflow {
                    calculation: "processed bank count conversion",
                }
            })?;
            let bank_scoped_rows = u64::try_from(projection_index.scoped_fact_count(input.bank))
                .map_err(|_| SnapshotError::CompositionLimitArithmeticOverflow {
                    calculation: "bank-scoped fact row count conversion",
                })?;
            let bank_selected_conclusions = u64::try_from(
                projection_index.selected_conclusion_count(input.bank),
            )
            .map_err(|_| SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "selected conclusion count conversion",
            })?;
            return Err(SnapshotError::ProjectedFactBytesLimitExceeded {
                bank: input.bank.to_owned(),
                bytes: projected_bytes,
                rows: projected_rows,
                bank_rows: rows,
                bank_scoped_rows,
                bank_selected_conclusions,
                largest_justifications: projection_index
                    .largest_selected_justifications(input.bank, 5),
                processed_banks,
                total_banks,
                global_fact_rows,
                limit: limits.max_projected_fact_bytes,
            });
        }
    }
    if std::env::var_os(REPORT_PROJECTION_STATS_ENV).is_some() {
        eprintln!(
            "fn64 projection-stats banks={total_banks} rows={projected_rows} bytes={projected_bytes} global_rows_per_bank={global_fact_rows}"
        );
    }

    let materialized_bytes = inputs.iter().try_fold(0u64, |total, input| {
        let bytes = u64::try_from(input.bytes.len()).map_err(|_| {
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "materialized byte count conversion",
            }
        })?;
        total
            .checked_add(bytes)
            .ok_or(SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "aggregate materialized bytes",
            })
    })?;
    if materialized_bytes > limits.max_aggregate_materialized_bytes {
        return Err(SnapshotError::AggregateMaterializedBytesLimitExceeded {
            bytes: materialized_bytes,
            limit: limits.max_aggregate_materialized_bytes,
        });
    }
    Ok(projection_index)
}

fn validate_unique_bank_names(inputs: &[MaterializedBankInput<'_>]) -> Result<(), SnapshotError> {
    let mut names = BTreeSet::new();
    for input in inputs {
        if input.bank.is_empty() || input.bank.trim() != input.bank {
            return Err(SnapshotError::InvalidBankName);
        }
        if !names.insert(input.bank) {
            return Err(SnapshotError::DuplicateBankName {
                bank: input.bank.to_owned(),
            });
        }
    }
    Ok(())
}

fn insert_cross_bank_authority(
    cross_calls: &mut BTreeMap<String, BTreeSet<CrossBankAuthoritativeCall>>,
    target_bank: &str,
    call: CrossBankAuthoritativeCall,
    record_count: &mut u64,
    limit: u64,
) -> Result<(), SnapshotError> {
    if cross_calls
        .entry(target_bank.to_owned())
        .or_default()
        .insert(call)
    {
        *record_count = record_count.checked_add(1).ok_or(
            SnapshotError::CompositionLimitArithmeticOverflow {
                calculation: "cross-bank authority record count",
            },
        )?;
        if *record_count > limit {
            return Err(SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: *record_count,
                limit,
            });
        }
    }
    Ok(())
}

fn build_cross_bank_authority_closure(
    base_facts: &FactDb,
    bank: &str,
    bytes: &[u8],
    va_start: u32,
    authorized_callable_roots: &BTreeSet<u32>,
    cross_bank_reachability_roots: &BTreeSet<u32>,
) -> ClosureResult {
    let mut roots: BTreeSet<u32> = base_facts
        .proven_function_entries(bank)
        .into_iter()
        .collect();
    roots.extend(authorized_callable_roots.iter().copied());
    roots.extend(cross_bank_reachability_roots.iter().copied());
    build_cfg_value_set_closed(
        bank,
        bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
}

fn authority_reachable_direct_calls(source: &PreparedBank) -> BTreeSet<(u32, u32)> {
    let cfg = &source.authority_closure.cfg;
    cfg.blocks
        .iter()
        .filter_map(|block| exact_authority_direct_call(cfg, block))
        .collect()
}

fn authority_reachable_direct_jumps(source: &PreparedBank) -> BTreeSet<(u32, u32)> {
    let cfg = &source.authority_closure.cfg;
    cfg.blocks
        .iter()
        .filter_map(|block| {
            let crate::cfg::BlockTerminator::Tail { target } = &block.terminator else {
                return None;
            };
            let source_pc = block.end_va.checked_sub(8)?;
            (cfg.word_class.get(&source_pc) == Some(&crate::cfg::WordClass::ProvenCode)
                && cfg.word_class.get(&(source_pc + 4)) == Some(&crate::cfg::WordClass::ProvenCode)
                && cfg.tail_transfers.contains(&(source_pc, *target)))
            .then_some((source_pc, *target))
        })
        .collect()
}

/// Compose several byte-verified banks together, letting a proven direct `jal`
/// in any one bank confer callable-entry authority on the target bank it lands
/// in. Returns one [`ProgramSnapshotV1`] per input bank, in input order.
///
/// This is the multi-bank counterpart to [`compose_materialized_bank_v1`]. Each
/// bank is prepared (validated, byte-verified, closure-built) exactly as in the
/// single-bank path; the only added authority is cross-bank. A direct call
/// from proven code, or a computed call whose typed analysis is
/// exhaustive and exactly matches its CFG terminator, whose target lands
/// aligned inside bank Y's proven VA range becomes an authoritative callable
/// root of bank Y. These are the identical two authority rules already used
/// for same-bank calls, extended across the catalog boundary. Open/bounded
/// computed calls and tail transfers never confer authority. A bank composed
/// alone here (no siblings) is byte-identical to
/// `compose_materialized_bank_v1`.
pub fn compose_materialized_banks_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
) -> Result<Vec<ProgramSnapshotV1>, SnapshotError> {
    compose_materialized_banks_v1_with_limits(
        rom,
        base_facts,
        inputs,
        MultiBankCompositionLimits::default(),
    )
}

/// Compose diagnostic snapshots within an explicit all-in-memory resource
/// envelope. Every output snapshot contains an exact bank-indexed projection
/// of the source fact database, including global and cross-bank evidence.
pub fn compose_materialized_banks_v1_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<Vec<ProgramSnapshotV1>, SnapshotError> {
    Ok(
        compose_materialized_banks_validated_v2_with_limits(rom, base_facts, inputs, limits)?
            .into_diagnostic_snapshots(),
    )
}

/// Compose several banks and retain opaque execution authority for the exact
/// byte-verified V2 results. Cross-bank direct and exhaustive resolved-call
/// authority is derived inside this constructor; serialized `Fact` or block
/// reports cannot manufacture this wrapper.
pub fn compose_materialized_banks_validated_v2(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    compose_materialized_banks_validated_v2_with_limits(
        rom,
        base_facts,
        inputs,
        MultiBankCompositionLimits::default(),
    )
}

/// Compose authoritative snapshots within an explicit all-in-memory resource
/// envelope. Limits are checked before retaining any bank-local fact database;
/// the cross-bank record limit is enforced as unique authority is derived.
pub fn compose_materialized_banks_validated_v2_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    limits: MultiBankCompositionLimits,
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    compose_materialized_banks_catalog_bound_with_limits(rom, base_facts, inputs, &[], limits)
}

/// Compose with move-only, catalog-bound authority for exact transfers whose
/// target VA is covered by multiple prepared generations. A capability is
/// consumed only when its ROM and complete `(source bank, site, kind, target)`
/// identity match an authority-reached edge. Calls confer callable authority;
/// jumps confer reachability only.
pub fn compose_materialized_banks_catalog_bound_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    dense_pack: &DenseAotPackV1,
    topology: &GenerationTopologyV1,
    catalog_definition_sha256: [u8; 32],
    capabilities: &[CatalogBoundExactTransferV1],
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    let dense_pack_sha256 = dense_aot_pack_sha256_v1(dense_pack);
    if let Some(index) = capabilities.iter().position(|capability| {
        !capability.matches_composition_identity(
            &rom.sha256,
            dense_pack_sha256,
            topology,
            catalog_definition_sha256,
        )
    }) {
        return Err(SnapshotError::CatalogCapabilityIdentityMismatch { index });
    }
    compose_materialized_banks_catalog_bound_with_limits(
        rom,
        base_facts,
        inputs,
        capabilities,
        MultiBankCompositionLimits::default(),
    )
}

fn compose_materialized_banks_catalog_bound_with_limits(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
    capabilities: &[CatalogBoundExactTransferV1],
    limits: MultiBankCompositionLimits,
) -> Result<ValidatedComposedSnapshotsV2, SnapshotError> {
    validate_unique_bank_names(inputs)?;
    let projection_index = check_multi_bank_limits(base_facts, inputs, limits)?;
    let mut prepared: Vec<PreparedBank> = inputs
        .iter()
        .map(|input| {
            let projected_facts = projection_index.project(input.bank);
            prepare_materialized_bank(
                rom,
                &projected_facts,
                MaterializedBankInput {
                    bank: input.bank,
                    va_start: input.va_start,
                    bytes: input.bytes,
                    seed_roots: input.seed_roots,
                },
            )
        })
        .collect::<Result<_, _>>()?;
    let interval_index = BankIntervalIndex::new(&prepared);

    // Collect every proven-source direct call whose target lands inside some
    // OTHER prepared bank's proven VA range, keyed by that target bank. The
    // source bank's own closure has already recorded its in-bank calls; here we
    // look only across bank boundaries, which no single-bank composition sees.
    let mut cross_calls: BTreeMap<String, BTreeSet<CrossBankAuthoritativeCall>> = BTreeMap::new();
    // A target covered by exactly one bank identifies both the bytes and the
    // generation unambiguously. Physical and proven VROM-backed banks are
    // equally byte-verifiable here. A VA covered by several generations does
    // not identify executable bytes, so it confers neither reachability nor
    // semantic authority until a typed activation-compatibility capability
    // selects the generation.
    let mut cross_call_count = 0;
    loop {
        let mut newly_reachable = BTreeMap::<String, BTreeSet<u32>>::new();
        let mut newly_semantic = BTreeMap::<String, BTreeSet<u32>>::new();
        for source in &prepared {
            let mut authority_transfers = authority_reachable_direct_calls(source)
                .into_iter()
                .map(|(source_pc, target_pc)| {
                    (
                        source_pc,
                        target_pc,
                        ExactTransferKindV1::Call,
                        Some(CrossBankAuthoritativeCallKind::Direct),
                    )
                })
                .collect::<Vec<_>>();
            authority_transfers.extend(authority_reachable_direct_jumps(source).into_iter().map(
                |(source_pc, target_pc)| (source_pc, target_pc, ExactTransferKindV1::Jump, None),
            ));
            for block in &source.authority_closure.cfg.blocks {
                let crate::cfg::BlockTerminator::ResolvedIndirect {
                    targets,
                    via_call: true,
                } = &block.terminator
                else {
                    continue;
                };
                let Some(source_pc) = authoritative_resolved_call_site(source, block, targets)
                else {
                    continue;
                };
                authority_transfers.extend(targets.iter().copied().map(|target_pc| {
                    (
                        source_pc,
                        target_pc,
                        ExactTransferKindV1::Call,
                        Some(CrossBankAuthoritativeCallKind::ExhaustiveResolved),
                    )
                }));
            }
            authority_transfers.sort_unstable();
            authority_transfers.dedup();
            for (source_pc, target_pc, transfer_kind, call_kind) in authority_transfers {
                if !target_pc.is_multiple_of(4)
                    || (source.va_start <= target_pc && target_pc < source.va_end)
                {
                    continue;
                }
                let target_indices =
                    interval_index.matching_other_banks(source.bank.as_str(), target_pc);
                let target_index = match target_indices.as_slice() {
                    [target_index] => Some(*target_index),
                    [] => None,
                    _ => capabilities.iter().find_map(|capability| {
                        let (cap_source_bank, cap_source_pc, cap_kind, cap_target_pc) =
                            capability.exact_edge();
                        (capability.normalized_rom_sha256() == rom.sha256
                            && cap_source_bank == source.bank
                            && cap_source_pc == source_pc
                            && cap_kind == transfer_kind
                            && cap_target_pc == target_pc)
                            .then(|| capability.selected_target().0)
                            .and_then(|target_bank| {
                                target_indices
                                    .iter()
                                    .copied()
                                    .find(|index| prepared[*index].bank == target_bank)
                            })
                    }),
                };
                let Some(target_index) = target_index else {
                    continue;
                };
                let target_bank = prepared[target_index].bank.as_str();
                if call_kind.is_some()
                    && !prepared[target_index]
                        .semantic_cross_bank_roots
                        .contains(&target_pc)
                {
                    newly_semantic
                        .entry(target_bank.to_owned())
                        .or_default()
                        .insert(target_pc);
                }
                if !prepared[target_index]
                    .cross_bank_reachability_roots
                    .contains(&target_pc)
                {
                    newly_reachable
                        .entry(prepared[target_index].bank.clone())
                        .or_default()
                        .insert(target_pc);
                }
                if let Some(kind) = call_kind {
                    insert_cross_bank_authority(
                        &mut cross_calls,
                        prepared[target_index].bank.as_str(),
                        CrossBankAuthoritativeCall {
                            source_bank: source.bank.clone(),
                            source_pc,
                            target_pc,
                            kind,
                        },
                        &mut cross_call_count,
                        limits.max_cross_bank_authority_records,
                    )?;
                }
            }
        }

        if newly_reachable.is_empty() && newly_semantic.is_empty() {
            break;
        }
        let changed_banks: BTreeSet<String> = newly_reachable
            .keys()
            .chain(newly_semantic.keys())
            .cloned()
            .collect();
        for bank in &mut prepared {
            if changed_banks.contains(&bank.bank) {
                let empty = BTreeSet::new();
                let projected_facts = projection_index.project(&bank.bank);
                expand_prepared_cross_bank_authority(
                    bank,
                    &projected_facts,
                    newly_reachable.get(&bank.bank).unwrap_or(&empty),
                    newly_semantic.get(&bank.bank).unwrap_or(&empty),
                )?;
            }
        }
    }

    // Traversal hints are diagnostic coverage only. Delay their potentially
    // large CFGs until direct and exhaustive-resolved callable authority reach
    // one monotone fixed point, then build each broad closure once.
    for bank in &mut prepared {
        let projected_facts = projection_index.project(&bank.bank);
        refresh_prepared_traversal_closure(bank, &projected_facts)?;
    }

    let mut snapshots = Vec::with_capacity(prepared.len());
    for mut bank in prepared {
        let calls = cross_calls.remove(&bank.bank).unwrap_or_default();
        // Record the real cross-bank edge as a fact and promote its target to an
        // external authorized root. The fact makes the incoming edge visible to
        // owner proof (a cross-bank call into an interior is still an ambiguity
        // blocker; only a call to the exact entry confers authority); the root
        // set is what discharges `EntryNotAuthoritative` for that entry.
        //
        // Deliberately NOT re-seeded into the CFG closure: the target bank's
        // own traversal already reaches this code, and injecting hundreds of
        // extra partition roots fractures the partition into ambiguity (measured
        // to erase even the in-bank owners). Authority alone is the sound,
        // additive change — a same-bank direct call's authority extended across
        // the boundary, nothing weaker and nothing that re-shapes the partition.
        let mut external_roots = bank.authorized_callable_roots.clone();
        for call in &calls {
            let source = BankAddr::new(call.source_bank.as_str(), call.source_pc);
            let target = BankAddr::new(bank.bank.as_str(), call.target_pc);
            insert_unique(
                &mut bank.facts,
                match call.kind {
                    CrossBankAuthoritativeCallKind::Direct => Fact::DirectCall { source, target },
                    CrossBankAuthoritativeCallKind::ExhaustiveResolved => {
                        Fact::ResolvedCall { source, target }
                    }
                },
            );
            external_roots.insert(call.target_pc);
            bank.cross_bank_reachability_roots.insert(call.target_pc);
        }
        // Vetted cross-bank roots must enter the authority closure so block
        // reachability and both owner passes share the same authority. Keep
        // them out of the already-built broad closure: re-partitioning that
        // geometry was measured to fracture owners into ambiguity.
        let authority_closure = build_cross_bank_authority_closure(
            base_facts,
            bank.bank.as_str(),
            &bank.bytes,
            bank.va_start,
            &bank.authorized_callable_roots,
            &bank.cross_bank_reachability_roots,
        );
        bank.authority_closure = authority_closure;
        validate_authoritative_delay_slot_roots(&bank)?;
        snapshots.push(finish_materialized_bank(rom, bank, &external_roots)?);
    }
    Ok(ValidatedComposedSnapshotsV2 { snapshots })
}

fn authoritative_resolved_call_site(
    source: &PreparedBank,
    block: &crate::cfg::BasicBlock,
    cfg_targets: &[u32],
) -> Option<u32> {
    let crate::cfg::BlockTerminator::ResolvedIndirect { targets, .. } = &block.terminator else {
        return None;
    };
    (targets == cfg_targets)
        .then(|| exhaustive_authority_call_site(&source.authority_closure, block))
        .flatten()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{
        executable_range_subject, function_entry_subject, load_image_table_record_subject,
        CandidateDetector, FunctionEntryEvidence, MappingAddressSpace, ProloguePattern, ProofState,
    };
    use crate::normalize;

    const BASE: u32 = 0x8000_0000;
    const ROM_START: u32 = 0x1000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | (target >> 2 & 0x03ff_ffff)
    }

    fn create_thread_fixture(pc: u32) -> [u32; 42] {
        let mut words = [0; 42];
        words[0] = 0x27bd_ffe8;
        words[2] = 0x0080_9821;
        words[7] = 0x2402_0001;
        words[8] = 0x00e0_4825;
        words[10] = 0xae62_0100;
        words[11] = 0xae63_0104;
        words[12] = 0xae62_0118;
        words[13] = 0xae62_0128;
        words[14] = 0xae65_0014;
        words[15] = 0xae60_0000;
        words[16] = 0xae60_0008;
        words[17] = 0xae66_011c;
        words[18] = 0xae68_0038;
        words[19] = 0xae69_003c;
        words[20] = 0xae64_012c;
        words[21] = 0xae60_0018;
        words[22] = 0xa662_0010;
        words[23] = 0xa660_0012;
        words[24] = 0x8fa2_002c;
        words[25] = 0xae62_0004;
        words[32] = 0x8fab_0028;
        words[34] = 0xae6a_00f0;
        words[35] = 0xae6b_00f4;
        words[39] = jal(pc + 0x1000);
        words[41] = JR_RA;
        words
    }

    fn write_create_thread_call(words: &mut [u32], index: usize, callee: u32, entry: u32) {
        words[index] = 0x3c06_0000 | entry >> 16;
        words[index + 1] = 0x24c6_0000 | entry as u16 as u32;
        words[index + 2] = jal(callee);
        words[index + 3] = NOP;
        words[index + 4] = JR_RA;
        words[index + 5] = NOP;
    }

    #[test]
    fn reachable_create_thread_entry_arguments_close_to_a_fixed_point() {
        const CREATE_INDEX: usize = 32;
        const THREAD_A_INDEX: usize = 128;
        const THREAD_B_INDEX: usize = 144;
        const DECOY_INDEX: usize = 160;
        const DECOY_TARGET_INDEX: usize = 176;
        let create = BASE + CREATE_INDEX as u32 * 4;
        let thread_a = BASE + THREAD_A_INDEX as u32 * 4;
        let thread_b = BASE + THREAD_B_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 184];
        write_create_thread_call(&mut words, 0, create, thread_a);
        words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        write_create_thread_call(&mut words, THREAD_A_INDEX, create, thread_b);
        words[THREAD_B_INDEX] = JR_RA;
        words[THREAD_B_INDEX + 1] = NOP;
        write_create_thread_call(&mut words, DECOY_INDEX, create, decoy_target);
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert_eq!(roots, BTreeSet::from([thread_a, thread_b]));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn reachable_argument_to_jalr_contract_authorizes_constant_callers_only() {
        const CALLBACK_CONSUMER_INDEX: usize = 32;
        const TARGET_INDEX: usize = 64;
        const DECOY_CALLER_INDEX: usize = 80;
        const DECOY_TARGET_INDEX: usize = 96;
        let consumer = BASE + CALLBACK_CONSUMER_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 112];
        words[0] = 0x3c04_0000 | target >> 16;
        words[1] = 0x2484_0000 | target as u16 as u32;
        words[2] = jal(consumer);
        words[3] = NOP;
        words[4] = JR_RA;
        words[5] = NOP;
        words[CALLBACK_CONSUMER_INDEX] = 0x0080_a025; // move s4, a0
        words[CALLBACK_CONSUMER_INDEX + 1] = 0x0280_f809; // jalr s4
        words[CALLBACK_CONSUMER_INDEX + 2] = NOP;
        words[CALLBACK_CONSUMER_INDEX + 3] = JR_RA;
        words[CALLBACK_CONSUMER_INDEX + 4] = NOP;
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        words[DECOY_CALLER_INDEX] = 0x3c04_0000 | decoy_target >> 16;
        words[DECOY_CALLER_INDEX + 1] = 0x2484_0000 | decoy_target as u16 as u32;
        words[DECOY_CALLER_INDEX + 2] = jal(consumer);
        words[DECOY_CALLER_INDEX + 3] = NOP;
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert!(roots.contains(&target));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn reachable_registry_dispatch_authorizes_constant_registrar_callers_only() {
        const REGISTRAR_INDEX: usize = 32;
        const DISPATCHER_INDEX: usize = 64;
        const TARGET_INDEX: usize = 100;
        const DECOY_CALLER_INDEX: usize = 120;
        const DECOY_TARGET_INDEX: usize = 140;
        let registrar = BASE + REGISTRAR_INDEX as u32 * 4;
        let dispatcher = BASE + DISPATCHER_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let decoy_target = BASE + DECOY_TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 160];
        words[0] = 0x3c05_0000 | target >> 16;
        words[1] = 0x24a5_0000 | target as u16 as u32;
        words[2] = jal(registrar);
        words[3] = NOP;
        words[4] = jal(dispatcher);
        words[5] = NOP;
        words[6] = JR_RA;
        words[7] = NOP;
        words[REGISTRAR_INDEX..REGISTRAR_INDEX + 11].copy_from_slice(&[
            0x0080_8025, // move s0, a0
            0xae05_0004, // sw   a1, 4(s0): callback
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0240, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d2a_0020, // lw   t2, 0x20(t1): old head
            0xae0a_0000, // sw   t2, 0(s0): link
            0x8d0b_0000, // lw   t3, 0(t0)
            0xad70_0020, // sw   s0, 0x20(t3): publish
            JR_RA,
            NOP,
        ]);
        words[DISPATCHER_INDEX..DISPATCHER_INDEX + 10].copy_from_slice(&[
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0240, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d30_0020, // lw   s0, 0x20(t1): head
            0x8e19_0004, // lw   t9, 4(s0): callback
            0x0320_f809, // jalr t9
            NOP,
            0x8e10_0000, // lw   s0, 0(s0): link
            JR_RA,
            NOP,
        ]);
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        words[DECOY_CALLER_INDEX] = 0x3c05_0000 | decoy_target >> 16;
        words[DECOY_CALLER_INDEX + 1] = 0x24a5_0000 | decoy_target as u16 as u32;
        words[DECOY_CALLER_INDEX + 2] = jal(registrar);
        words[DECOY_CALLER_INDEX + 3] = NOP;
        words[DECOY_TARGET_INDEX] = JR_RA;
        words[DECOY_TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        let authorized = derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap();

        let roots = semantic_callable_root_set(&authorized);
        assert!(roots.contains(&target));
        assert!(!roots.contains(&decoy_target));
    }

    #[test]
    fn traversal_hint_cannot_bootstrap_create_thread_authority() {
        const CREATE_INDEX: usize = 32;
        const DECOY_INDEX: usize = 96;
        const TARGET_INDEX: usize = 112;
        let create = BASE + CREATE_INDEX as u32 * 4;
        let target = BASE + TARGET_INDEX as u32 * 4;
        let mut words = vec![NOP; 128];
        words[0] = JR_RA;
        words[1] = NOP;
        words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        write_create_thread_call(&mut words, DECOY_INDEX, create, target);
        words[TARGET_INDEX] = JR_RA;
        words[TARGET_INDEX + 1] = NOP;
        let bytes = asm(&words);

        // The decoy address could be an ordinary MaterializedBankInput seed,
        // but this authority helper accepts hardware roots only. Starting at
        // the real hardware entry therefore cannot reach or promote it.
        assert!(derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn ambiguous_create_thread_identity_fails_composition_closed() {
        const FIRST_INDEX: usize = 16;
        const SECOND_INDEX: usize = 80;
        let mut words = vec![NOP; 128];
        words[FIRST_INDEX..FIRST_INDEX + 42]
            .copy_from_slice(&create_thread_fixture(BASE + FIRST_INDEX as u32 * 4));
        words[SECOND_INDEX..SECOND_INDEX + 42]
            .copy_from_slice(&create_thread_fixture(BASE + SECOND_INDEX as u32 * 4));
        let bytes = asm(&words);

        assert!(matches!(
            derive_semantic_callable_argument_roots(
                "bank",
                &bytes,
                BASE,
                BASE + bytes.len() as u32,
                &[BASE],
            ),
            Err(SnapshotError::AmbiguousOsCreateThreadBinding { candidates, .. })
                if candidates.len() == 2
        ));
    }

    #[test]
    fn invalid_create_thread_entry_operands_remain_non_authoritative() {
        const CREATE_INDEX: usize = 32;
        let create = BASE + CREATE_INDEX as u32 * 4;
        for target in [BASE + 0x201, BASE - 4, BASE + 0x1000] {
            let mut words = vec![NOP; 128];
            write_create_thread_call(&mut words, 0, create, target);
            words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
            let bytes = asm(&words);
            assert!(derive_semantic_callable_argument_roots(
                "bank",
                &bytes,
                BASE,
                BASE + bytes.len() as u32,
                &[BASE],
            )
            .unwrap()
            .is_empty());
        }

        let mut unresolved = vec![NOP; 128];
        unresolved[0] = 0x8c06_0000; // lw a2, 0(zero): memory value is open.
        unresolved[1] = jal(create);
        unresolved[2] = NOP;
        unresolved[3] = JR_RA;
        unresolved[4] = NOP;
        unresolved[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        let bytes = asm(&unresolved);
        assert!(derive_semantic_callable_argument_roots(
            "bank",
            &bytes,
            BASE,
            BASE + bytes.len() as u32,
            &[BASE],
        )
        .unwrap()
        .is_empty());
    }

    fn rom_with_bank(bank: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; ROM_START as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..].copy_from_slice(bank);
        normalize(&bytes).unwrap()
    }

    fn facts_for(byte_len: u32, authoritative_entries: &[u32]) -> FactDb {
        let mut facts = facts_without_executable(byte_len, authoritative_entries);
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
                "test_executable",
            )
            .unwrap();
        facts
    }

    fn facts_without_executable(byte_len: u32, authoritative_entries: &[u32]) -> FactDb {
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
            .conclude(
                "bank:bank",
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
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
                    "test_entry",
                )
                .unwrap();
        }
        facts
    }

    fn compose<'a>(
        rom: &NormalizedRom,
        facts: &FactDb,
        bytes: &'a [u8],
        roots: &'a [u32],
    ) -> Result<ProgramSnapshotV1, SnapshotError> {
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
    }

    #[test]
    fn authoritative_return_function_composes_to_one_exact_owner() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert_eq!(snapshot.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V5);
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 1);
        assert!(snapshot.banks[0].blocker_histogram.is_empty());
        assert_eq!(snapshot.banks[0].input.rom_start, ROM_START);
    }

    #[test]
    fn two_pc_candidate_delay_slot_root_is_suppressed_but_fact_is_retained() {
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let target = BankAddr::new("bank", BASE + 4);
        let claim = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("bank", BASE + 8),
            },
            proposed_state: ProofState::Candidate,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Candidate,
                vec![claim],
                "test_candidate",
            )
            .unwrap();

        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 4]).unwrap();

        assert!(snapshot.banks[0]
            .authority_closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE
                && block.end_va == BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Return)));
        assert!(!snapshot.banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == BASE + 4));
        assert!(snapshot
            .facts
            .candidate_function_entries("bank")
            .contains(&(BASE + 4)));
    }

    #[test]
    fn two_pc_authoritative_plain_delay_slot_root_is_an_alias() {
        let bytes = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE, BASE + 4]);

        let snapshot = compose(&rom, &facts, &bytes, &[BASE, BASE + 4]).unwrap();
        let cfg = &snapshot.banks[0].authority_closure.cfg;
        assert!(cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| { alias.entry_va == BASE + 4 && alias.control_pc == BASE }));
        assert!(!cfg.blocks.iter().any(|block| block.start_va == BASE + 4));
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == BASE
                && block.end_va == BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Return)
        }));
    }

    #[test]
    fn authoritative_control_shaped_delay_entry_fails_loud() {
        let bytes = asm(&[JR_RA, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE, BASE + 4]);

        assert!(matches!(
            compose(&rom, &facts, &bytes, &[BASE, BASE + 4]),
            Err(SnapshotError::UnsupportedControlDelayEntry {
                bank,
                entry,
                control_pc,
            }) if bank == "bank" && entry == BASE + 4 && control_pc == BASE
        ));
    }

    #[test]
    fn candidate_call_cannot_promote_authority_reached_interior_owner() {
        let target = BASE + 4;
        let candidate_caller = BASE + 0x10;
        let bytes = asm(&[NOP, NOP, JR_RA, NOP, jal(target), NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE, candidate_caller]).unwrap();

        assert!(snapshot.banks[0]
            .block_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                crate::block_proof::BlockAssessment::Proven { block }
                    if block.start_va == target
                        && block.authoritative_roots.as_slice() == [BASE]
            )));
        assert!(snapshot.banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Candidate { frontier }
                    | OwnerAssessment::Ambiguous { frontier }
                    if frontier.entry.pc == target
                        && frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
            )));
    }

    #[test]
    fn proven_nonresident_physical_bank_composes_without_resident_special_case() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = FactDb::new();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "overlay".into(),
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START,
            rom_end: ROM_START + bytes.len() as u32,
            va_start: BASE,
            va_end: BASE + bytes.len() as u32,
        });
        facts
            .conclude(
                "bank:overlay",
                ProofState::Proven,
                vec![mapping],
                "test_overlay_mapping",
            )
            .unwrap();
        let target = BankAddr::new("overlay", BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("overlay", BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "test_overlay_entry",
            )
            .unwrap();

        let snapshot = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "overlay",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();

        assert_eq!(snapshot.banks[0].input.bank, "overlay");
        assert_eq!(snapshot.banks[0].owner_proof.bank, "overlay");
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
    }

    #[test]
    fn validated_vrom_composition_emits_v2_but_not_legacy_v1() {
        const VROM_START: u32 = 0x0020_0000;

        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = FactDb::new();
        let file = facts.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x800,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: VROM_START,
            source_end: VROM_START + bytes.len() as u32,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: ROM_START,
            destination_end: ROM_START + bytes.len() as u32,
        });
        facts
            .conclude(
                load_image_table_record_subject("files", 0),
                ProofState::Proven,
                vec![file],
                "test_vrom_file",
            )
            .unwrap();
        let mapping = facts.insert(Fact::RomMapping {
            bank: "overlay".into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: VROM_START,
            rom_end: VROM_START + bytes.len() as u32,
            va_start: BASE,
            va_end: BASE + bytes.len() as u32,
        });
        facts
            .conclude(
                "bank:overlay",
                ProofState::Proven,
                vec![mapping, file],
                "test_vrom_mapping",
            )
            .unwrap();
        let target = BankAddr::new("overlay", BASE);
        let entry = facts.insert(Fact::FunctionEntryClaim {
            target: target.clone(),
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue {
                stack_adjust: target.clone(),
                frame_size: 16,
                pattern: ProloguePattern::LeafWithMatchedRestore,
                corroborating_site: BankAddr::new("overlay", BASE + 4),
            },
            proposed_state: ProofState::Proven,
        });
        facts
            .conclude(
                function_entry_subject(&target),
                ProofState::Proven,
                vec![entry],
                "test_vrom_entry",
            )
            .unwrap();

        let validated = compose_materialized_bank_validated_v2(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "overlay",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let pack = crate::block_pack::emit_validated_block_pack_v2(&validated, 0, &rom).unwrap();
        assert_eq!(pack.schema_version, crate::block_pack::BLOCK_PACK_SCHEMA_V2);
        assert_eq!(pack.banks[0].blocks[0].rom_space, RomAddressSpace::Virtual);
        assert!(matches!(
            crate::block_pack::emit_block_pack_v1(&validated.snapshots()[0], &rom),
            Err(crate::block_pack::BlockPackError::LegacySchemaVirtualBacking {
                bank,
                start_va: BASE,
            }) if bank == "overlay"
        ));
        assert!(matches!(
            crate::block_pack::materialize_block_pack(&pack, &rom),
            Err(crate::block_pack::BlockPackError::VromRequiresFacts {
                bank,
                start_va: BASE,
            }) if bank == "overlay"
        ));
        let materialized =
            crate::block_pack::materialize_block_pack_with_facts(&pack, &rom, Some(&facts))
                .unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![JR_RA, NOP]);
    }

    #[test]
    fn traversal_seed_does_not_become_entry_authority() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert_eq!(snapshot.coverage.function_owners.candidate_owners, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 0);
        assert!(snapshot.banks[0].blocker_histogram.iter().any(|summary| {
            summary.kind == OwnerBlockerKind::EntryNotAuthoritative
                && summary.affected_assessments == 1
        }));
    }

    #[test]
    fn closure_indirect_fact_matches_owner_proof_evidence() {
        // Construct 0x80000020 in t2, jump there, then return.
        let lui_t2 = 0x3c0a_8000;
        let addiu_t2 = 0x254a_0020;
        let jr_t2 = 0x0140_0008;
        let mut bytes = asm(&[lui_t2, addiu_t2, jr_t2, NOP]);
        bytes.resize(0x20, 0);
        bytes.extend_from_slice(&asm(&[JR_RA, NOP]));
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        assert!(snapshot.facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::IndirectTransferAnalysis {
                site,
                state: IndirectTransferState::Exhaustive,
                targets,
                ..
            } if site.pc == BASE + 8 && targets == &[BASE + 0x20]
        )));
        assert!(!snapshot.banks[0]
            .blocker_histogram
            .iter()
            .any(|summary| { summary.kind == OwnerBlockerKind::ResolvedIndirectEvidenceMismatch }));
    }

    #[test]
    fn authoritative_block_proof_does_not_require_section_boundary_evidence() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        facts
            .conclude(
                executable_range_subject("bank", BASE, BASE + bytes.len() as u32),
                ProofState::Conflict,
                vec![],
                "test_remove_executable_authority",
            )
            .unwrap();
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 0);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_bytes, 8);
    }

    #[test]
    fn reached_closure_derives_executable_evidence_and_admits_owner() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        // No executable evidence was supplied; the reached proven-code block
        // itself became the typed proven range and admitted the owner.
        assert_eq!(
            snapshot.facts.proven_executable_ranges("bank"),
            vec![(BASE, BASE + 8)]
        );
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
        assert!(snapshot.banks[0].blocker_histogram.is_empty());
    }

    #[test]
    fn owner_spanning_unreached_gap_stays_blocked() {
        // `j` over two unreached words to a return block: closure reaches
        // [BASE,BASE+8) and [BASE+0x10,BASE+0x18) but never the gap between.
        let j_over_gap = 0x0800_0000 | (((BASE + 0x10) >> 2) & 0x03ff_ffff);
        let bytes = asm(&[j_over_gap, NOP, NOP, NOP, JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_without_executable(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();

        // Both reached blocks became proven executable ranges; the gap
        // between them was not smeared over.
        assert_eq!(
            snapshot.facts.proven_executable_ranges("bank"),
            vec![(BASE, BASE + 8), (BASE + 0x10, BASE + 0x18)]
        );
        // The owner's proposed extent spans the gap, so admission stays
        // blocked: derived ranges prove reached bytes, never the extent.
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 0);
        let histogram = &snapshot.banks[0].blocker_histogram;
        assert!(histogram
            .iter()
            .any(|summary| summary.kind == OwnerBlockerKind::NotProvenExecutable));
        assert!(histogram
            .iter()
            .any(|summary| summary.kind == OwnerBlockerKind::OwnerNotContiguous));
    }

    #[test]
    fn block_proof_rejects_shared_decoder_failures() {
        let unknown = 0x7801_2345;
        let bytes = asm(&[unknown]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 0);
        assert!(matches!(
            &snapshot.banks[0].block_proof.assessments[0],
            crate::block_proof::BlockAssessment::Candidate { blockers, .. }
                if blockers.contains(&crate::block_proof::BlockProofBlocker::InvalidInstruction {
                    pc: BASE,
                    word: unknown,
                })
        ));
    }

    #[test]
    fn block_pack_round_trip_binds_geometry_without_serializing_words() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let validated = compose_materialized_bank_validated_v2(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let pack = crate::block_pack::emit_validated_block_pack_v2(&validated, 0, &rom).unwrap();
        assert_eq!(pack.schema_version, crate::block_pack::BLOCK_PACK_SCHEMA_V2);
        let diagnostic =
            crate::block_pack::emit_block_pack_v1(&validated.snapshots()[0], &rom).unwrap();
        assert_eq!(
            diagnostic.schema_version,
            crate::block_pack::BLOCK_PACK_SCHEMA_V1
        );
        assert!(matches!(
            crate::block_pack::emit_validated_block_pack_v2(&validated, 1, &rom),
            Err(
                crate::block_pack::BlockPackError::ValidatedSnapshotIndexOutsideComposition {
                    index: 1,
                    count: 1,
                }
            )
        ));
        let mut caller_authored = validated.snapshots()[0].clone();
        caller_authored.schema_version = 1;
        assert!(matches!(
            crate::block_pack::emit_block_pack_v1(&caller_authored, &rom),
            Err(
                crate::block_pack::BlockPackError::UnsupportedSnapshotSchema {
                    expected: PROGRAM_SNAPSHOT_SCHEMA_V5,
                    actual: 1,
                }
            )
        ));
        let json = serde_json::to_string(&pack).unwrap();
        assert!(!json.contains("\"words\""));
        let materialized = crate::block_pack::materialize_block_pack(&pack, &rom).unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![JR_RA, NOP]);
        let runner = crate::block_pack::emit_materialized_bank_runner(
            &materialized[0],
            "run_materialized_bank",
        );
        assert!(runner.contains(&format!("{BASE:#010X} => {{")));
        assert!(runner.contains("Sparse bank-qualified MIPS runner"));
        let code_bank = crate::block_pack::materialized_code_bank(&materialized[0]).unwrap();
        assert_eq!(code_bank.instruction_count(), 2);
        let mut catalog = fn64_recomp_rs::CodeCatalog::new();
        let bank_id = code_bank.id();
        catalog.register(code_bank).unwrap();
        assert_eq!(
            catalog
                .resolve(fn64_recomp_rs::ExecutionKey::new(
                    bank_id,
                    fn64_recomp_rs::GuestPc::new(BASE),
                ))
                .unwrap()
                .word,
            JR_RA
        );

        let mut changed = rom.clone();
        changed.bytes[ROM_START as usize] ^= 1;
        assert!(matches!(
            crate::block_pack::materialize_block_pack(&pack, &changed),
            Err(crate::block_pack::BlockPackError::BlockDigestMismatch { .. })
        ));
    }

    #[test]
    fn materialized_bytes_are_bound_to_the_normalized_rom() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let different = asm(&[NOP, NOP]);
        assert!(matches!(
            compose(&rom, &facts, &different, &[BASE]),
            Err(SnapshotError::MaterializedBytesMismatch { .. })
        ));
    }

    #[test]
    fn malformed_geometry_fails_loudly() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        assert!(matches!(
            compose_materialized_bank_v1(
                &rom,
                &facts,
                MaterializedBankInput {
                    bank: "bank",
                    va_start: BASE + 1,
                    bytes: &bytes,
                    seed_roots: &[BASE + 1],
                }
            ),
            Err(SnapshotError::UnalignedBank { .. })
        ));
        assert!(matches!(
            compose(&rom, &facts, &bytes, &[BASE + 2]),
            Err(SnapshotError::RootUnaligned { .. })
        ));
    }

    #[test]
    fn serialization_is_deterministic_and_contains_no_rom_bytes() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let expected =
            serde_json::to_vec(&compose(&rom, &facts, &bytes, &[BASE]).unwrap()).unwrap();
        let text = String::from_utf8(expected.clone()).unwrap();
        assert!(!text.contains("\"bytes\":"));
        assert!(text.contains("\"bytes_sha256\":"));
        for _ in 0..10 {
            let actual =
                serde_json::to_vec(&compose(&rom, &facts, &bytes, &[BASE]).unwrap()).unwrap();
            assert_eq!(actual, expected);
        }
    }

    // ---- Multi-bank cross-bank authority ----
    //
    // Bank X ("caller") at VA X_BASE holds an authoritative returning function
    // that `jal`s into bank Y ("callee") at Y_BASE. Both banks live in the same
    // 256 MB region so a real MIPS `jal` can address across them. Y_BASE names a
    // valid returning function with NO in-bank authority; only X's proven direct
    // call can authorize it.

    const X_BASE: u32 = 0x8000_0000;
    const X_ROM: u32 = 0x1000;
    const Y_BASE: u32 = 0x8000_1000;
    const Y_ROM: u32 = 0x2000;

    fn jal_to(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    /// A ROM holding bank X bytes at `X_ROM` and bank Y bytes at `Y_ROM`.
    fn rom_with_two_banks(x_bytes: &[u8], y_bytes: &[u8]) -> NormalizedRom {
        let mut bytes = vec![0u8; Y_ROM as usize + y_bytes.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        bytes[X_ROM as usize..X_ROM as usize + x_bytes.len()].copy_from_slice(x_bytes);
        bytes[Y_ROM as usize..Y_ROM as usize + y_bytes.len()].copy_from_slice(y_bytes);
        normalize(&bytes).unwrap()
    }

    /// Prove one physical bank mapping and, optionally, an authoritative entry.
    fn prove_bank(
        facts: &mut FactDb,
        bank: &str,
        rom_start: u32,
        va_start: u32,
        byte_len: u32,
        authoritative_entries: &[u32],
    ) {
        let mapping = facts.insert(Fact::RomMapping {
            bank: bank.into(),
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end: rom_start + byte_len,
            va_start,
            va_end: va_start + byte_len,
        });
        facts
            .conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test_mapping",
            )
            .unwrap();
        for &entry in authoritative_entries {
            let target = BankAddr::new(bank, entry);
            let claim = facts.insert(Fact::FunctionEntryClaim {
                target: target.clone(),
                detector: CandidateDetector::ProloguePattern,
                evidence: FunctionEntryEvidence::Prologue {
                    stack_adjust: target.clone(),
                    frame_size: 16,
                    pattern: ProloguePattern::LeafWithMatchedRestore,
                    corroborating_site: BankAddr::new(bank, entry + 4),
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

    fn prove_vrom_bank(
        facts: &mut FactDb,
        bank: &str,
        vrom_start: u32,
        physical_start: u32,
        va_start: u32,
        byte_len: u32,
    ) {
        let record = facts.insert(Fact::LoadImageTableRecord {
            table: "files".into(),
            bank: None,
            table_space: RomAddressSpace::Physical,
            table_offset: 0x800,
            index: 0,
            source_space: MappingAddressSpace::VirtualRom,
            source_start: vrom_start,
            source_end: vrom_start + byte_len,
            destination_space: MappingAddressSpace::PhysicalRom,
            destination_start: physical_start,
            destination_end: physical_start + byte_len,
        });
        facts
            .conclude(
                load_image_table_record_subject("files", 0),
                ProofState::Proven,
                vec![record],
                "test_vrom_file",
            )
            .unwrap();
        let mapping = facts.insert(Fact::RomMapping {
            bank: bank.into(),
            rom_space: RomAddressSpace::Virtual,
            rom_start: vrom_start,
            rom_end: vrom_start + byte_len,
            va_start,
            va_end: va_start + byte_len,
        });
        facts
            .conclude(
                format!("bank:{bank}"),
                ProofState::Proven,
                vec![mapping],
                "test_vrom_mapping",
            )
            .unwrap();
    }

    fn two_bank_inputs<'a>(
        x_bytes: &'a [u8],
        y_bytes: &'a [u8],
        x_seeds: &'a [u32],
        y_seeds: &'a [u32],
    ) -> [MaterializedBankInput<'a>; 2] {
        [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: x_bytes,
                seed_roots: x_seeds,
            },
            MaterializedBankInput {
                bank: "callee",
                va_start: Y_BASE,
                bytes: y_bytes,
                seed_roots: y_seeds,
            },
        ]
    }

    fn callee_owner_is_proven(snapshots: &[ProgramSnapshotV1]) -> bool {
        snapshots[1]
            .banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Y_BASE)
            })
    }

    fn callee_entry_not_authoritative(snapshots: &[ProgramSnapshotV1]) -> bool {
        snapshots[1].banks[0].owner_proof.assessments.iter().any(|assessment| {
            assessment.entry().pc == Y_BASE
                && matches!(
                    assessment,
                    OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier }
                        if frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
                )
        })
    }

    #[test]
    fn proven_cross_bank_jal_authorizes_a_callee_owner() {
        // X calls Y with a real `jal`; Y is a bare returning function with no
        // in-bank authority. Cross-bank composition must admit Y's owner.
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            callee_owner_is_proven(&snapshots),
            "a proven cross-bank jal should authorize the callee entry: {:?}",
            snapshots[1].banks[0].owner_proof.assessments
        );
        // The honest cross-bank edge is recorded as a fact on the callee.
        assert!(snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller" && target.bank == "callee" && target.pc == Y_BASE
        )));
    }

    #[test]
    fn unique_cross_bank_call_seeds_semantic_thread_entry_recovery() {
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        let semantic_claim = snapshots[1]
            .facts
            .facts()
            .iter()
            .enumerate()
            .find_map(|(index, fact)| match fact {
                Fact::FunctionEntryClaim {
                    target,
                    detector: CandidateDetector::SemanticCallableArgument,
                    evidence:
                        FunctionEntryEvidence::SemanticCallableArgument {
                            call_site,
                            callee,
                            pointer_register: 6,
                            contract: SemanticCallableContract::OsCreateThread,
                        },
                    proposed_state: ProofState::Proven,
                } if target.bank == "callee"
                    && target.pc == thread
                    && call_site.bank == "callee"
                    && call_site.pc == Y_BASE + 8
                    && callee.bank == "callee"
                    && callee.pc == create =>
                {
                    Some(index)
                }
                _ => None,
            })
            .expect("semantic authority must retain its exact call contract");
        let conclusion = snapshots[1]
            .facts
            .conclusion(&function_entry_subject(&BankAddr::new("callee", thread)))
            .expect("semantic entry must have a conclusion");
        assert_eq!(conclusion.state, ProofState::Proven);
        assert!(conclusion.justified_by.contains(&semantic_claim));

        let wire = serde_json::to_vec(&snapshots[1]).unwrap();
        let round_trip: ProgramSnapshotV1 = serde_json::from_slice(&wire).unwrap();
        assert_eq!(round_trip.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V5);
        assert_eq!(
            serde_json::to_vec(&round_trip.facts).unwrap(),
            serde_json::to_vec(&snapshots[1].facts).unwrap()
        );
    }

    #[test]
    fn unique_vrom_cross_calls_reach_a_semantic_fixed_point() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        const Y_VROM: u32 = 0x5000;
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Z_BASE + CREATE_INDEX as u32 * 4;
        let thread = Z_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[jal_to(Z_BASE), NOP, JR_RA, NOP]);
        let mut z_words = vec![NOP; 112];
        write_create_thread_call(&mut z_words, 0, create, thread);
        z_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        z_words[THREAD_INDEX] = JR_RA;
        z_words[THREAD_INDEX + 1] = NOP;
        let z = asm(&z_words);

        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_vrom_bank(&mut facts, "loaded", Y_VROM, Y_ROM, Y_BASE, y.len() as u32);
        prove_bank(&mut facts, "resident", Z_ROM, Z_BASE, z.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "loaded",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "resident",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
        ];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[2].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        assert!(snapshots[2].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "loaded"
                    && source.pc == Y_BASE
                    && target.bank == "resident"
                    && target.pc == Z_BASE
        )));
    }

    #[test]
    fn resolved_then_direct_cross_bank_chain_reaches_one_fixed_point() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        let x = asm(&[
            0x3c19_0000 | (Y_BASE >> 16),
            0x3739_0000 | (Y_BASE & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let y = asm(&[jal_to(Z_BASE), NOP, JR_RA, NOP]);
        let z = asm(&[JR_RA, NOP]);
        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "leaf", Z_ROM, Z_BASE, z.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[2].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE
            )));
        assert!(snapshots[2].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "middle"
                    && source.pc == Y_BASE
                    && target.bank == "leaf"
                    && target.pc == Z_BASE
        )));

        let reversed = [
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
        ];
        let reversed_snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 2,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        assert!(reversed_snapshots[0].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE
            )));
        assert!(reversed_snapshots[0]
            .facts
            .facts()
            .iter()
            .any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "middle"
                        && source.pc == Y_BASE
                        && target.bank == "leaf"
                        && target.pc == Z_BASE
            )));
        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 1,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: 2,
                limit: 1,
            }
        );
    }

    #[test]
    fn transitive_cross_bank_plain_delay_slot_root_is_an_alias() {
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x3000;
        let delay_slot = Z_BASE + 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[jal_to(delay_slot), NOP, JR_RA, NOP]);
        let z = asm(&[
            0x1000_0003, // beq $zero,$zero,Z_BASE+0x10
            0x2404_0007, // plain shared delay entry
            0x0100_0008, // jr $t0: open, denying an exact owner
            NOP,
            JR_RA,
            NOP,
        ]);
        let mut raw = vec![0u8; Z_ROM as usize + z.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Z_ROM as usize..Z_ROM as usize + z.len()].copy_from_slice(&z);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "leaf", Z_ROM, Z_BASE, z.len() as u32, &[Z_BASE]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "leaf",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[Z_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let cfg = &snapshots[2].banks[0].authority_closure.cfg;
        assert!(cfg
            .plain_delay_entry_aliases
            .iter()
            .any(|alias| { alias.entry_va == delay_slot && alias.control_pc == Z_BASE }));
        assert!(!cfg.blocks.iter().any(|block| block.start_va == delay_slot));
        assert!(cfg.blocks.iter().any(|block| {
            block.start_va == Z_BASE
                && block.end_va == Z_BASE + 8
                && matches!(block.terminator, crate::cfg::BlockTerminator::Branch { .. })
        }));
        let classified = crate::closure::classified_destinations(&snapshots);
        assert!(classified.iter().any(|destination| {
            destination.va == delay_slot
                && destination.reason == crate::closure::DestinationReason::InProvenBlock
                && destination.class() == crate::closure::DestinationClass::BlockAot
        }));
    }

    #[test]
    fn traversal_hint_cannot_seed_cross_bank_semantic_authority() {
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(&mut facts, "caller", X_ROM, X_BASE, x.len() as u32, &[]);
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(snapshots[1].banks[0]
            .closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == Y_BASE));
        assert!(snapshots[1].banks[0]
            .authority_closure
            .cfg
            .blocks
            .is_empty());
        assert!(!snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| {
                matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
            }));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::FunctionEntryClaim {
                detector: CandidateDetector::SemanticCallableArgument,
                ..
            }
        )));
    }

    #[test]
    fn overlapping_cross_bank_targets_do_not_seed_semantic_authority() {
        const Y2_ROM: u32 = 0x3000;
        const CREATE_INDEX: usize = 32;
        const THREAD_INDEX: usize = 96;
        let create = Y_BASE + CREATE_INDEX as u32 * 4;
        let thread = Y_BASE + THREAD_INDEX as u32 * 4;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let mut y_words = vec![NOP; 112];
        write_create_thread_call(&mut y_words, 0, create, thread);
        y_words[CREATE_INDEX..CREATE_INDEX + 42].copy_from_slice(&create_thread_fixture(create));
        y_words[THREAD_INDEX] = JR_RA;
        y_words[THREAD_INDEX + 1] = NOP;
        let y = asm(&y_words);
        let mut raw = vec![0u8; Y2_ROM as usize + y.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Y2_ROM as usize..Y2_ROM as usize + y.len()].copy_from_slice(&y);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee_a", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "callee_b", Y2_ROM, Y_BASE, y.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0]
                .authority_closure
                .cfg
                .proven_roots
                .contains(&Y_BASE));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller" && target.bank == snapshot.banks[0].input.bank
            )));
            assert!(!snapshot.banks[0]
                .owner_proof
                .assessments
                .iter()
                .any(|assessment| {
                    matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == thread)
                }));
        }
    }

    #[test]
    fn overlapping_target_banks_receive_no_cross_bank_authority() {
        const Y2_ROM: u32 = 0x3000;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y1 = asm(&[JR_RA, NOP]);
        let y2 = y1.clone();
        let mut raw = vec![0u8; Y2_ROM as usize + y2.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y1.len()].copy_from_slice(&y1);
        raw[Y2_ROM as usize..Y2_ROM as usize + y2.len()].copy_from_slice(&y2);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee_a", Y_ROM, Y_BASE, y1.len() as u32, &[]);
        prove_bank(&mut facts, "callee_b", Y2_ROM, Y_BASE, y2.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y1,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y2,
                seed_roots: &[Y_BASE],
            },
        ];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0].owner_proof.assessments.iter().any(
                |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Y_BASE)
            ));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller" && target.bank == snapshot.banks[0].input.bank
            )));
        }
    }

    #[test]
    fn late_overlapping_roots_do_not_propagate_authority() {
        const Y2_ROM: u32 = 0x3000;
        const Z_BASE: u32 = 0x8000_2000;
        const Z_ROM: u32 = 0x4000;
        const W_BASE: u32 = 0x8000_3000;
        const W1_ROM: u32 = 0x5000;
        const W2_ROM: u32 = 0x6000;
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[
            jal_to(Z_BASE),
            NOP,
            0x3c19_0000 | (W_BASE >> 16),
            0x3739_0000 | (W_BASE & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let z = asm(&[JR_RA, NOP]);
        let w = asm(&[JR_RA, NOP]);
        let mut raw = vec![0u8; W2_ROM as usize + w.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        for (rom_start, bytes) in [
            (X_ROM, x.as_slice()),
            (Y_ROM, y.as_slice()),
            (Y2_ROM, y.as_slice()),
            (Z_ROM, z.as_slice()),
            (W1_ROM, w.as_slice()),
            (W2_ROM, w.as_slice()),
        ] {
            raw[rom_start as usize..rom_start as usize + bytes.len()].copy_from_slice(bytes);
        }
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "middle_a", Y_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "middle_b", Y2_ROM, Y_BASE, y.len() as u32, &[]);
        prove_bank(&mut facts, "unique", Z_ROM, Z_BASE, z.len() as u32, &[]);
        prove_bank(&mut facts, "overlap_a", W1_ROM, W_BASE, w.len() as u32, &[]);
        prove_bank(&mut facts, "overlap_b", W2_ROM, W_BASE, w.len() as u32, &[]);
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "middle_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "unique",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_a",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_b",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let unique = &snapshots[3];
        assert!(!unique.banks[0].owner_proof.assessments.iter().any(
            |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == Z_BASE)
        ));
        for source_bank in ["middle_a", "middle_b"] {
            assert!(!unique.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == source_bank && target.bank == "unique" && target.pc == Z_BASE
            )));
        }
        for overlap in &snapshots[4..] {
            assert!(!overlap.banks[0].owner_proof.assessments.iter().any(
                |assessment| matches!(assessment, OwnerAssessment::Proven { owner } if owner.entry.pc == W_BASE)
            ));
            for source_bank in ["middle_a", "middle_b"] {
                assert!(!overlap.facts.facts().iter().any(|fact| matches!(
                    fact,
                    Fact::ResolvedCall { source, target }
                        if source.bank == source_bank
                            && target.bank == overlap.banks[0].input.bank
                            && target.pc == W_BASE
                )));
            }
        }

        let reversed = [
            MaterializedBankInput {
                bank: "overlap_b",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "overlap_a",
                va_start: W_BASE,
                bytes: &w,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "unique",
                va_start: Z_BASE,
                bytes: &z,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "middle_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[],
            },
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
        ];
        let reversed_snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &reversed,
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        let cross_call_facts = |snapshots: &[ProgramSnapshotV1]| {
            snapshots
                .iter()
                .flat_map(|snapshot| snapshot.facts.facts())
                .filter(|fact| matches!(fact, Fact::DirectCall { .. } | Fact::ResolvedCall { .. }))
                .map(|fact| serde_json::to_string(fact).unwrap())
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(
            cross_call_facts(&snapshots),
            cross_call_facts(&reversed_snapshots)
        );
        assert!(cross_call_facts(&snapshots).is_empty());
    }

    #[test]
    fn overlapping_cross_bank_delay_slot_target_is_not_authorized() {
        const Y2_ROM: u32 = 0x3000;
        let delay_slot = Y_BASE + 4;
        let x = asm(&[jal_to(delay_slot), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP, JR_RA, NOP]);
        let mut raw = vec![0u8; Y2_ROM as usize + y.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + x.len()].copy_from_slice(&x);
        raw[Y_ROM as usize..Y_ROM as usize + y.len()].copy_from_slice(&y);
        raw[Y2_ROM as usize..Y2_ROM as usize + y.len()].copy_from_slice(&y);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(
            &mut facts,
            "callee_a",
            Y_ROM,
            Y_BASE,
            y.len() as u32,
            &[Y_BASE],
        );
        prove_bank(
            &mut facts,
            "callee_b",
            Y2_ROM,
            Y_BASE,
            y.len() as u32,
            &[Y_BASE],
        );
        let inputs = [
            MaterializedBankInput {
                bank: "caller",
                va_start: X_BASE,
                bytes: &x,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "callee_a",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
            MaterializedBankInput {
                bank: "callee_b",
                va_start: Y_BASE,
                bytes: &y,
                seed_roots: &[Y_BASE],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        for snapshot in &snapshots[1..] {
            assert!(!snapshot.banks[0]
                .authority_closure
                .cfg
                .proven_roots
                .contains(&delay_slot));
            assert!(!snapshot.facts.facts().iter().any(|fact| matches!(
                fact,
                Fact::DirectCall { source, target }
                    if source.bank == "caller"
                        && target.bank == snapshot.banks[0].input.bank
                        && target.pc == delay_slot
            )));
        }
    }

    fn locally_contained_overlap_fixture(
        source: &[u8],
        sibling_va: u32,
        sibling: &[u8],
    ) -> (NormalizedRom, FactDb) {
        let mut raw = vec![0u8; Y_ROM as usize + sibling.len()];
        raw[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        raw[8..12].copy_from_slice(&X_BASE.to_be_bytes());
        raw[X_ROM as usize..X_ROM as usize + source.len()].copy_from_slice(source);
        raw[Y_ROM as usize..Y_ROM as usize + sibling.len()].copy_from_slice(sibling);
        let rom = normalize(&raw).unwrap();
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "source",
            X_ROM,
            X_BASE,
            source.len() as u32,
            &[X_BASE],
        );
        prove_bank(
            &mut facts,
            "sibling",
            Y_ROM,
            sibling_va,
            sibling.len() as u32,
            &[],
        );
        (rom, facts)
    }

    fn assert_local_target_did_not_authorize_sibling(snapshots: &[ProgramSnapshotV1], target: u32) {
        assert!(snapshots[1].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| matches!(
                assessment,
                OwnerAssessment::Candidate { frontier }
                    | OwnerAssessment::Ambiguous { frontier }
                    if frontier.entry.pc == target
                        && frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
            )));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| match fact {
            Fact::DirectCall {
                source,
                target: edge_target,
            }
            | Fact::ResolvedCall {
                source,
                target: edge_target,
            } => {
                source.bank == "source" && edge_target.bank == "sibling"
            }
            _ => false,
        }));
    }

    #[test]
    fn locally_contained_direct_call_does_not_authorize_overlapping_sibling() {
        let target = X_BASE + 8;
        let source = asm(&[jal_to(target), NOP, JR_RA, NOP]);
        let sibling = asm(&[JR_RA, NOP]);
        let (rom, facts) = locally_contained_overlap_fixture(&source, target, &sibling);
        let inputs = [
            MaterializedBankInput {
                bank: "source",
                va_start: X_BASE,
                bytes: &source,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "sibling",
                va_start: target,
                bytes: &sibling,
                seed_roots: &[target],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert_local_target_did_not_authorize_sibling(&snapshots, target);
    }

    #[test]
    fn locally_contained_resolved_call_does_not_authorize_overlapping_sibling() {
        let target = X_BASE + 0x10;
        let source = asm(&[
            0x3c19_0000 | (target >> 16),
            0x3739_0000 | (target & 0xffff),
            (25u32 << 21) | (31u32 << 11) | 0x09,
            NOP,
            JR_RA,
            NOP,
        ]);
        let sibling = asm(&[JR_RA, NOP]);
        let (rom, facts) = locally_contained_overlap_fixture(&source, target, &sibling);
        let inputs = [
            MaterializedBankInput {
                bank: "source",
                va_start: X_BASE,
                bytes: &source,
                seed_roots: &[X_BASE],
            },
            MaterializedBankInput {
                bank: "sibling",
                va_start: target,
                bytes: &sibling,
                seed_roots: &[target],
            },
        ];

        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert_local_target_did_not_authorize_sibling(&snapshots, target);
    }

    #[test]
    fn multi_bank_limits_fail_before_unbounded_composition() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let inputs = [MaterializedBankInput {
            bank: "bank",
            va_start: BASE,
            bytes: &bytes,
            seed_roots: &[BASE],
        }];

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_rows: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::ProjectedFactRowsLimitExceeded { .. }
        ));

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_bytes: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::ProjectedFactBytesLimitExceeded { .. }
        ));

        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_aggregate_materialized_bytes: (bytes.len() - 1) as u64,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::AggregateMaterializedBytesLimitExceeded { .. }
        ));
    }

    #[test]
    fn projected_limits_do_not_multiply_irrelevant_bank_facts() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let mut facts = facts_for(bytes.len() as u32, &[BASE]);
        for index in 0..100u32 {
            facts.insert(Fact::BlockStart {
                bank: format!("irrelevant_{index}"),
                pc: 0x8100_0000 + index * 4,
            });
        }
        let inputs = [MaterializedBankInput {
            bank: "bank",
            va_start: BASE,
            bytes: &bytes,
            seed_roots: &[BASE],
        }];

        let snapshots = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &inputs,
            MultiBankCompositionLimits {
                max_projected_fact_rows: 3,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].facts.facts().iter().any(|fact| {
            matches!(fact, Fact::BlockStart { bank, .. } if bank.starts_with("irrelevant_"))
        }));
    }

    #[test]
    #[ignore = "private regression: requires the local OoT ROM and runs the full closure gate"]
    fn oot_projection_runs_full_gate_below_default_limits() {
        let rom = std::env::var("FN64_DISCOVER_OOT_ROM")
            .expect("set FN64_DISCOVER_OOT_ROM to the private OoT ROM path");
        assert!(
            std::path::Path::new(&rom).is_file(),
            "missing private OoT ROM"
        );
        let output = std::process::Command::new(env!("CARGO"))
            .args([
                "run",
                "--quiet",
                "-p",
                "fn64-discover",
                "--bin",
                "gate_closure",
            ])
            .env("FN64_DISCOVER_OOT_ROM", &rom)
            .env(REPORT_PROJECTION_STATS_ENV, "1")
            .env_remove("FN64_DISCOVER_NW4E_ROM")
            .env_remove("FN64_DISCOVER_NWXE_ROM")
            .output()
            .expect("run gate_closure");
        assert!(
            output.status.success(),
            "gate failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("OoT"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stats = stderr
            .lines()
            .find(|line| line.starts_with("fn64 projection-stats "))
            .expect("projection stats receipt");
        eprintln!("{stats}");
    }

    #[test]
    fn duplicate_input_bank_names_fail_closed() {
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let inputs = [
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        ];

        assert_eq!(
            compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap_err(),
            SnapshotError::DuplicateBankName {
                bank: "bank".into(),
            }
        );
    }

    #[test]
    fn cross_bank_authority_limit_counts_unique_records() {
        let x = asm(&[jal_to(Y_BASE), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);
        let error = compose_materialized_banks_v1_with_limits(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]),
            MultiBankCompositionLimits {
                max_cross_bank_authority_records: 0,
                ..MultiBankCompositionLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            SnapshotError::CrossBankAuthorityRecordsLimitExceeded {
                records: 1,
                limit: 0,
            }
        );
    }

    #[test]
    fn interval_index_preserves_overlaps_and_prunes_disjoint_banks() {
        let count = 16_384usize;
        let intervals = (0..count)
            .map(|input_index| {
                let va_start = 0x8000_0000 + (input_index as u32) * 16;
                BankInterval {
                    input_index,
                    bank: format!("bank_{input_index}"),
                    va_start,
                    va_end: va_start + 8,
                }
            })
            .collect();
        let index = BankIntervalIndex::from_intervals(intervals);
        let mut probes = 0;
        for input_index in 0..count {
            let target = 0x8000_0000 + (input_index as u32) * 16;
            let (matches, query_probes) =
                index.matching_other_banks_with_probe_count("source", target);
            assert_eq!(matches, vec![input_index]);
            probes += query_probes;
        }
        assert_eq!(
            probes, count,
            "disjoint point queries must not rescan the catalog"
        );

        let overlapping = BankIntervalIndex::from_intervals(vec![
            BankInterval {
                input_index: 2,
                bank: "callee_b".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 16,
            },
            BankInterval {
                input_index: 1,
                bank: "callee_a".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 8,
            },
            BankInterval {
                input_index: 0,
                bank: "source".into(),
                va_start: Y_BASE,
                va_end: Y_BASE + 4,
            },
        ]);
        assert_eq!(
            overlapping.matching_other_banks("source", Y_BASE),
            vec![1, 2]
        );
        assert_eq!(
            overlapping.matching_other_banks("source", Y_BASE + 8),
            vec![2]
        );
    }

    #[test]
    fn proven_cross_bank_jal_splits_an_interior_callee_entry() {
        let interior = Y_BASE + 4;
        let x = asm(&[jal_to(interior), NOP, JR_RA, NOP]);
        let y = asm(&[NOP, JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        let bank = &snapshots[1].banks[0];
        let prefix = bank
            .partition
            .owners
            .iter()
            .find(|owner| owner.root_va == Y_BASE)
            .expect("the enclosing owner keeps its prefix");
        assert_eq!(prefix.extent_end, interior);
        let split = bank
            .partition
            .owners
            .iter()
            .find(|owner| owner.root_va == interior)
            .expect("the authorized interior entry gets its own owner");
        assert_eq!(split.extent_end, Y_BASE + 12);
        assert!(bank.owner_proof.assessments.iter().any(|assessment| {
            matches!(
                assessment,
                OwnerAssessment::Proven { owner }
                    if owner.entry.pc == interior && owner.va_end == Y_BASE + 12
            )
        }));
    }

    #[test]
    fn single_bank_composition_leaves_callee_unauthorized() {
        // The same callee composed ALONE (no caller sibling) has no in-bank
        // authority, so its entry stays non-authoritative. This is the control
        // that the win above comes from the cross-bank edge, not the geometry.
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&asm(&[JR_RA, NOP]), &y);
        let mut facts = FactDb::new();
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let callee_only = [MaterializedBankInput {
            bank: "callee",
            va_start: Y_BASE,
            bytes: &y,
            seed_roots: &[Y_BASE],
        }];
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &callee_only).unwrap();
        // snapshots[0] is the callee here; the helper indexes [1], so assert
        // directly on assessment [0].
        assert!(snapshots[0].banks[0]
            .owner_proof
            .assessments
            .iter()
            .any(|assessment| assessment.entry().pc == Y_BASE
                && matches!(
                    assessment,
                    OwnerAssessment::Candidate { frontier }
                        | OwnerAssessment::Ambiguous { frontier }
                        if frontier.blockers.contains(&OwnerBlocker::EntryNotAuthoritative)
                )));
    }

    #[test]
    fn cross_bank_jal_from_unproven_code_confers_no_authority() {
        // The `jal` word in X is NOT reached as proven code: X is seeded with a
        // root that never reaches the call site, so the source word stays
        // unproven. A jal-shaped word in unproven bytes proves nothing.
        //
        // X layout: [0x00] JR_RA / NOP  (the only reached, authoritative fn)
        //           [0x08] jal Y / NOP  (unreached — never proven code)
        let x = asm(&[JR_RA, NOP, jal_to(Y_BASE), NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        // Seed X only at X_BASE (the returning fn); the jal at X_BASE+8 is never
        // traversed, so its word_class is not ProvenCode.
        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            !callee_owner_is_proven(&snapshots),
            "an unproven-source jal must not authorize the callee"
        );
        assert!(callee_entry_not_authoritative(&snapshots));
        // No cross-bank DirectCall fact was minted from unproven source bytes.
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller" && target.bank == "callee"
        )));
    }

    #[test]
    fn cross_bank_jal_from_candidate_traversal_confers_no_authority() {
        let candidate_caller = X_BASE + 8;
        let x = asm(&[JR_RA, NOP, jal_to(Y_BASE), NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let snapshots = compose_materialized_banks_v1(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE, candidate_caller], &[Y_BASE]),
        )
        .unwrap();
        assert!(callee_entry_not_authoritative(&snapshots));
        assert!(!snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::DirectCall { source, target }
                if source.bank == "caller"
                    && source.pc == candidate_caller
                    && target.bank == "callee"
        )));
    }

    #[test]
    fn cross_bank_jal_missing_the_callee_range_confers_no_authority() {
        // X `jal`s an address that lands in NEITHER bank's proven VA range
        // (a gap between X and Y). No bank claims it, so no authority is
        // conferred and Y's own entry is untouched.
        let stray = Y_BASE - 0x100; // between X and Y, mapped by no bank
        let x = asm(&[jal_to(stray), NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            !callee_owner_is_proven(&snapshots),
            "a jal missing the callee's VA range must not authorize its entry"
        );
        assert!(callee_entry_not_authoritative(&snapshots));
    }

    #[test]
    fn exhaustive_cross_bank_computed_call_authorizes_callee_and_serializes_its_kind() {
        // X reaches Y through a computed `jalr $ra, $t9` (t9 built with
        // lui/ori). The value-set proof is exhaustive, so the cross-bank rule
        // is exactly the same authority already accepted within one bank.
        let lui_t9 = 0x3c19_0000 | (Y_BASE >> 16); // lui $t9, hi(Y)
        let ori_t9 = 0x3739_0000 | (Y_BASE & 0xffff); // ori $t9, $t9, lo(Y)
        let jalr_ra_t9 = (25u32 << 21) | (31u32 << 11) | 0x09; // jalr $ra, $t9
        let x = asm(&[lui_t9, ori_t9, jalr_ra_t9, NOP, JR_RA, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let inputs = two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]);
        let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs).unwrap();
        assert!(
            callee_owner_is_proven(&snapshots),
            "an exhaustive cross-bank computed call should authorize the callee entry"
        );
        assert_eq!(snapshots[1].schema_version, PROGRAM_SNAPSHOT_SCHEMA_V5);
        assert!(snapshots[1].facts.facts().iter().any(|fact| matches!(
            fact,
            Fact::ResolvedCall { source, target }
                if source.bank == "caller"
                    && source.pc == X_BASE + 8
                    && target.bank == "callee"
                    && target.pc == Y_BASE
        )));
        let wire = serde_json::to_value(&snapshots[1]).unwrap();
        assert_eq!(wire["schema_version"], PROGRAM_SNAPSHOT_SCHEMA_V5);
        assert!(wire["facts"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact.get("ResolvedCall").is_some()));
    }

    #[test]
    fn cross_bank_computed_jump_confers_no_callable_authority() {
        let lui_t9 = 0x3c19_0000 | (Y_BASE >> 16);
        let ori_t9 = 0x3739_0000 | (Y_BASE & 0xffff);
        let jr_t9 = (25u32 << 21) | 0x08;
        let x = asm(&[lui_t9, ori_t9, jr_t9, NOP]);
        let y = asm(&[JR_RA, NOP]);
        let rom = rom_with_two_banks(&x, &y);
        let mut facts = FactDb::new();
        prove_bank(
            &mut facts,
            "caller",
            X_ROM,
            X_BASE,
            x.len() as u32,
            &[X_BASE],
        );
        prove_bank(&mut facts, "callee", Y_ROM, Y_BASE, y.len() as u32, &[]);

        let snapshots = compose_materialized_banks_v1(
            &rom,
            &facts,
            &two_bank_inputs(&x, &y, &[X_BASE], &[Y_BASE]),
        )
        .unwrap();
        assert!(callee_entry_not_authoritative(&snapshots));
        assert!(!snapshots[1]
            .facts
            .facts()
            .iter()
            .any(|fact| matches!(fact, Fact::ResolvedCall { .. })));
    }

    fn prepared_resolved_call(
        state: IndirectTransferState,
        evidence_sets: &[Vec<u32>],
        source_word_proven: bool,
        delay_word_proven: bool,
    ) -> (PreparedBank, crate::cfg::BasicBlock) {
        let site_pc = X_BASE;
        let block = crate::cfg::BasicBlock {
            start_va: site_pc,
            end_va: site_pc + 8,
            terminator: crate::cfg::BlockTerminator::ResolvedIndirect {
                targets: vec![Y_BASE],
                via_call: true,
            },
        };
        let mut facts = FactDb::new();
        for targets in evidence_sets {
            facts.insert(Fact::IndirectTransferAnalysis {
                site: BankAddr::new("caller", site_pc),
                via_call: true,
                state,
                kind: Some(IndirectTransferKind::Constant),
                targets: targets.clone(),
                memory_sources: Vec::new(),
            });
        }
        let closure = ClosureResult {
            cfg: crate::cfg::Cfg {
                bank: "caller".into(),
                word_class: [
                    (
                        site_pc,
                        if source_word_proven {
                            crate::cfg::WordClass::ProvenCode
                        } else {
                            crate::cfg::WordClass::Unknown
                        },
                    ),
                    (
                        site_pc + 4,
                        if delay_word_proven {
                            crate::cfg::WordClass::ProvenCode
                        } else {
                            crate::cfg::WordClass::Unknown
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                blocks: vec![block.clone()],
                direct_calls: Vec::new(),
                tail_transfers: Vec::new(),
                indirect_sites: Vec::new(),
                plain_delay_entry_aliases: Vec::new(),
                unsupported_delay_entries: Vec::new(),
                proven_roots: vec![site_pc],
            },
            indirect: evidence_sets
                .iter()
                .map(|targets| crate::resolve::IndirectResolution {
                    site_pc,
                    via_call: true,
                    state: match state {
                        IndirectTransferState::Exhaustive => IndirectProofState::Exhaustive,
                        IndirectTransferState::Bounded => IndirectProofState::Bounded,
                        IndirectTransferState::Open => IndirectProofState::Open,
                    },
                    kind: Some(IndirectResolutionKind::Constant),
                    targets: targets.clone(),
                    memory_sources: Vec::new(),
                })
                .collect(),
        };
        (
            PreparedBank {
                bank: "caller".into(),
                va_start: X_BASE,
                va_end: X_BASE + 8,
                bytes: vec![0; 8],
                digest: BankInputDigestV1 {
                    bank: "caller".into(),
                    va_start: X_BASE,
                    va_end: X_BASE + 8,
                    rom_space: RomAddressSpace::Physical,
                    rom_start: X_ROM,
                    rom_end: X_ROM + 8,
                    bytes_sha256: sha256_hex(&[0; 8]),
                },
                facts,
                authority_closure: closure.clone(),
                closure,
                traversal_roots: BTreeSet::from([site_pc]),
                semantic_callable_entries: BTreeSet::new(),
                authorized_callable_roots: BTreeSet::new(),
                cross_bank_reachability_roots: BTreeSet::new(),
                semantic_cross_bank_roots: BTreeSet::new(),
            },
            block,
        )
    }

    #[test]
    fn bounded_cross_bank_call_set_stays_non_authoritative() {
        let (source, block) =
            prepared_resolved_call(IndirectTransferState::Bounded, &[vec![Y_BASE]], true, true);
        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn open_cross_bank_call_claim_stays_non_authoritative() {
        let (source, block) =
            prepared_resolved_call(IndirectTransferState::Open, &[vec![Y_BASE]], true, true);
        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn broad_and_authority_resolved_targets_must_agree() {
        let (source, mut broad_block) = prepared_resolved_call(
            IndirectTransferState::Exhaustive,
            &[vec![Y_BASE]],
            true,
            true,
        );
        broad_block.terminator = crate::cfg::BlockTerminator::ResolvedIndirect {
            targets: vec![Y_BASE + 4],
            via_call: true,
        };
        assert_eq!(
            authoritative_resolved_call_site(&source, &broad_block, &[Y_BASE + 4]),
            None
        );
    }

    #[test]
    fn unresolved_source_or_delay_word_stays_non_authoritative() {
        for (source_proven, delay_proven) in [(false, true), (true, false)] {
            let (source, block) = prepared_resolved_call(
                IndirectTransferState::Exhaustive,
                &[vec![Y_BASE]],
                source_proven,
                delay_proven,
            );
            assert_eq!(
                authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
                None
            );
        }
    }

    #[test]
    fn duplicate_disagreeing_or_mismatched_analysis_stays_non_authoritative() {
        for evidence_sets in [
            vec![vec![Y_BASE + 4]],
            vec![vec![Y_BASE], vec![Y_BASE + 4]],
            vec![vec![Y_BASE], vec![Y_BASE]],
        ] {
            let (source, block) = prepared_resolved_call(
                IndirectTransferState::Exhaustive,
                &evidence_sets,
                true,
                true,
            );
            assert_eq!(
                authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
                None
            );
        }
    }

    #[test]
    fn broad_only_exhaustive_cross_bank_call_stays_non_authoritative() {
        let (mut source, block) = prepared_resolved_call(
            IndirectTransferState::Exhaustive,
            &[vec![Y_BASE]],
            true,
            true,
        );
        source.authority_closure.cfg.blocks.clear();
        source.authority_closure.cfg.word_class.clear();

        assert_eq!(
            authoritative_resolved_call_site(&source, &block, &[Y_BASE]),
            None
        );
    }

    #[test]
    fn multi_bank_solo_matches_single_bank_composition() {
        // A bank composed alone through the multi-bank entry point must be
        // byte-identical to `compose_materialized_bank_v1`: the no-sibling path
        // adds no authority and re-shapes nothing.
        let bytes = asm(&[JR_RA, NOP]);
        let rom = rom_with_bank(&bytes);
        let facts = facts_for(bytes.len() as u32, &[BASE]);
        let single = compose_materialized_bank_v1(
            &rom,
            &facts,
            MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            },
        )
        .unwrap();
        let multi = compose_materialized_banks_v1(
            &rom,
            &facts,
            &[MaterializedBankInput {
                bank: "bank",
                va_start: BASE,
                bytes: &bytes,
                seed_roots: &[BASE],
            }],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&single).unwrap(),
            serde_json::to_vec(&multi[0]).unwrap()
        );
    }
}
