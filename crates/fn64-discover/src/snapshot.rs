//! Deterministic composition of discovery's existing phase outputs into one
//! proof-carrying program artifact.
//!
//! V1 intentionally materializes one proven physical ROM-backed bank. It verifies
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
    conclude_reached_executable_ranges, prove_reachable_blocks, BlockProofReport,
};
use crate::coverage::{report_with_owner_proofs, CoverageReport, OwnerProofCoverageError};
use crate::facts::{
    BankAddr, Fact, FactDb, IndirectTransferKind, IndirectTransferState, RomAddressSpace,
};
use crate::owner_proof::{
    prove_exact_owners_with_external_authority, OwnerAssessment, OwnerBlocker, OwnerProofReport,
};
use crate::partition::{partition, partition_with_authorized_splits, Partition};
use crate::resolve::{
    build_cfg_value_set_closed, ClosureResult, IndirectProofState, IndirectResolutionKind,
};
use crate::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const PROGRAM_SNAPSHOT_SCHEMA_V1: u32 = 1;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidBankName,
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
            Self::InvalidBankName => write!(f, "bank name must be nonempty and canonical"),
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
    let prepared = prepare_materialized_bank(rom, base_facts, input)?;
    finish_materialized_bank(rom, prepared, &BTreeSet::new())
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
    let backing = crate::banks::materialize_rom_range(
        rom,
        base_facts,
        mapping.rom_space,
        rom_start,
        rom_end,
    )
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

    let mut facts = base_facts.clone();
    let root_vec: Vec<u32> = roots.into_iter().collect();
    let closure = build_cfg_value_set_closed(input.bank, input.bytes, input.va_start, &root_vec);
    integrate_closure_facts(&mut facts, input.bank, input.va_start, va_end, &closure);

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
    })
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
    // First owner pass supplies entry authority to block proof; executable
    // evidence may not exist yet, so its `NotProvenExecutable` blockers are
    // provisional and it is never serialized.
    let authority_proof = prove_exact_owners_with_external_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        external_authorized_roots,
    );
    let block_proof = prove_reachable_blocks(&closure.cfg, &partition, &authority_proof, &facts);
    // Bytes reached as proven code from an authoritative entry are proven
    // executable — exactly those bytes, never the gaps between them. Record
    // them as typed facts, then re-prove owners against the enriched
    // evidence. Block proof itself does not consume executable ranges, and
    // entry authority is unaffected, so no further fixpoint iteration exists.
    conclude_reached_executable_ranges(&block_proof, &mut facts);
    let owner_proof = prove_exact_owners_with_external_authority(
        &closure.cfg,
        &partition,
        &facts,
        &bytes,
        va_start,
        external_authorized_roots,
    );
    let blocker_histogram = owner_blocker_histogram(&owner_proof);
    let coverage = report_with_owner_proofs(rom.len(), &facts, std::slice::from_ref(&owner_proof))?;
    let bank = BankSnapshotV1 {
        input: digest,
        closure,
        partition,
        owner_proof,
        block_proof,
        blocker_histogram,
    };
    debug_assert_eq!(bank.input.bank, bank.owner_proof.bank);
    Ok(ProgramSnapshotV1 {
        schema_version: PROGRAM_SNAPSHOT_SCHEMA_V1,
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

fn ensure_authoritative_block_leaders(
    closure: &mut ClosureResult,
    external_authorized_roots: &BTreeSet<u32>,
) {
    for &entry in external_authorized_roots {
        if !entry.is_multiple_of(4)
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
        if closure
            .cfg
            .blocks
            .iter()
            .any(|block| block.start_va == entry)
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

/// A proven direct `jal` in one bank whose target lands inside another bank's
/// proven VA range. Its source word is proven code and its target is aligned
/// and in-range — the same conditions a same-bank direct call already carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CrossBankDirectCall {
    source_bank: String,
    source_pc: u32,
    target_pc: u32,
}

/// Compose several byte-verified banks together, letting a proven direct `jal`
/// in any one bank confer callable-entry authority on the target bank it lands
/// in. Returns one [`ProgramSnapshotV1`] per input bank, in input order.
///
/// This is the multi-bank counterpart to [`compose_materialized_bank_v1`]. Each
/// bank is prepared (validated, byte-verified, closure-built) exactly as in the
/// single-bank path; the only added authority is cross-bank: a
/// `DirectCall{source, target}` from proven code in bank X whose `target` lands
/// aligned inside bank Y's proven VA range becomes an authoritative callable
/// root of bank Y — the identical rule a same-bank direct call already applies,
/// extended across the bank boundary. Computed/tail transfers never confer this
/// authority; only a direct `jal` from proven bytes does. A bank composed alone
/// here (no siblings) is byte-identical to `compose_materialized_bank_v1`.
pub fn compose_materialized_banks_v1(
    rom: &NormalizedRom,
    base_facts: &FactDb,
    inputs: &[MaterializedBankInput<'_>],
) -> Result<Vec<ProgramSnapshotV1>, SnapshotError> {
    let prepared: Vec<PreparedBank> = inputs
        .iter()
        .map(|input| {
            prepare_materialized_bank(
                rom,
                base_facts,
                MaterializedBankInput {
                    bank: input.bank,
                    va_start: input.va_start,
                    bytes: input.bytes,
                    seed_roots: input.seed_roots,
                },
            )
        })
        .collect::<Result<_, _>>()?;

    // Collect every proven-source direct call whose target lands inside some
    // OTHER prepared bank's proven VA range, keyed by that target bank. The
    // source bank's own closure has already recorded its in-bank calls; here we
    // look only across bank boundaries, which no single-bank composition sees.
    let mut cross_calls: BTreeMap<String, BTreeSet<CrossBankDirectCall>> = BTreeMap::new();
    for source in &prepared {
        for &(source_pc, target_pc) in &source.closure.cfg.direct_calls {
            // SOUNDNESS: the source `jal` word must be proven code, not a
            // jal-shaped word in unproven bytes.
            if source.closure.cfg.word_class.get(&source_pc)
                != Some(&crate::cfg::WordClass::ProvenCode)
            {
                continue;
            }
            for target in &prepared {
                if target.bank == source.bank {
                    continue;
                }
                // SOUNDNESS: the target must land aligned inside the target
                // bank's proven VA range. Same-bank calls are handled by that
                // bank's own closure; only genuinely cross-bank edges get here.
                if target_pc >= target.va_start
                    && target_pc < target.va_end
                    && target_pc.is_multiple_of(4)
                {
                    cross_calls.entry(target.bank.clone()).or_default().insert(
                        CrossBankDirectCall {
                            source_bank: source.bank.clone(),
                            source_pc,
                            target_pc,
                        },
                    );
                }
            }
        }
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
        let mut external_roots = BTreeSet::new();
        for call in &calls {
            insert_unique(
                &mut bank.facts,
                Fact::DirectCall {
                    source: BankAddr::new(call.source_bank.as_str(), call.source_pc),
                    target: BankAddr::new(bank.bank.as_str(), call.target_pc),
                },
            );
            external_roots.insert(call.target_pc);
        }
        snapshots.push(finish_materialized_bank(rom, bank, &external_roots)?);
    }
    Ok(snapshots)
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
        executable_range_subject, function_entry_subject, CandidateDetector, FunctionEntryEvidence,
        ProloguePattern, ProofState,
    };
    use crate::normalize;

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

        assert_eq!(snapshot.schema_version, PROGRAM_SNAPSHOT_SCHEMA_V1);
        assert_eq!(snapshot.coverage.function_owners.exact_owners, 1);
        assert_eq!(snapshot.banks[0].block_proof.proven_blocks, 1);
        assert!(snapshot.banks[0].blocker_histogram.is_empty());
        assert_eq!(snapshot.banks[0].input.rom_start, ROM_START);
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
        let snapshot = compose(&rom, &facts, &bytes, &[BASE]).unwrap();
        let pack = crate::block_pack::emit_block_pack_v1(&snapshot, &rom).unwrap();
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
    fn cross_bank_computed_call_confers_no_authority() {
        // X reaches Y through a computed `jalr $ra, $t9` (t9 built with
        // lui/addiu), NOT a direct `jal`. A cross-bank computed/tail transfer is
        // NOT the direct-call authority rule, so Y's entry stays unauthorized.
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
            !callee_owner_is_proven(&snapshots),
            "a cross-bank computed call must not authorize the callee entry"
        );
        assert!(callee_entry_not_authoritative(&snapshots));
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
