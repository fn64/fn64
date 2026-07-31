//! Proven-bank preparation for snapshot composition.
//!
//! This is the shared boundary between discovery facts and any in-process
//! snapshot producer. It admits only proven bank mappings, materializes their
//! ROM-backed bytes (including proven VROM/Yaz0 files), excludes load-time
//! `.bss` tails, and derives deterministic traversal seeds. Those seeds guide
//! closure only; the composer still derives callable-entry authority from the
//! fact database.

use crate::banks;
use crate::facts::{
    function_entry_subject, Fact, FactDb, FunctionEntryEvidence, ProofState, RomAddressSpace,
};
use crate::snapshot::MaterializedBankInput;
use crate::NormalizedRom;
use std::collections::{BTreeMap, BTreeSet};

const MIB: u64 = 1024 * 1024;

/// Resource envelope applied before any bank bytes are materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareSnapshotBanksLimits {
    pub max_banks: usize,
    pub max_aggregate_rom_bytes: u64,
    /// Maximum complete decoded VROM file used to serve any requested bank.
    pub max_decoded_vrom_file_bytes: usize,
}

impl Default for PrepareSnapshotBanksLimits {
    fn default() -> Self {
        Self {
            max_banks: 4096,
            max_aggregate_rom_bytes: 256 * MIB,
            // Cartridge addressability tops out at 64 MiB; a decoded file
            // larger than the complete hardware ROM is not admitted by the
            // default in-memory snapshot preparation path.
            max_decoded_vrom_file_bytes: crate::file_table::DEFAULT_MAX_DECODED_VROM_FILE_BYTES,
        }
    }
}

/// One proven, ROM-backed bank ready for snapshot composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSnapshotBank {
    pub bank: String,
    pub rom_space: RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
    pub va_start: u32,
    /// Exclusive end of the ROM-backed prefix, excluding any load-time `.bss`.
    pub va_end: u32,
    pub bytes: Vec<u8>,
    /// Closure traversal hints, never callable-entry authority. Table-derived
    /// entries here are callable evidence exposed by an admitted table, not a
    /// claim that a call site itself was observed.
    pub traversal_seeds: Vec<u32>,
    /// Stable fact indices proving the physical backing of a VROM interval.
    pub backing_evidence: Vec<usize>,
}

/// Deterministically ordered bank inputs whose owned storage outlives the
/// borrowed [`MaterializedBankInput`] view passed to the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSnapshotBanks {
    banks: Vec<PreparedSnapshotBank>,
}

impl PreparedSnapshotBanks {
    pub fn banks(&self) -> &[PreparedSnapshotBank] {
        &self.banks
    }

    pub fn materialized_inputs(&self) -> Vec<MaterializedBankInput<'_>> {
        self.banks
            .iter()
            .map(|bank| MaterializedBankInput {
                bank: &bank.bank,
                va_start: bank.va_start,
                bytes: &bank.bytes,
                seed_roots: &bank.traversal_seeds,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareSnapshotBanksError {
    NoProvenMappings,
    AmbiguousMapping {
        bank: String,
        distinct_geometries: usize,
    },
    BankLimitExceeded {
        banks: usize,
        limit: usize,
    },
    AggregateRomBytesOverflow,
    AggregateRomBytesLimitExceeded {
        bytes: u64,
        limit: u64,
    },
    InvertedInterval {
        bank: String,
    },
    EmptyRomInterval {
        bank: String,
    },
    RomExceedsVa {
        bank: String,
        rom_extent: u32,
        va_extent: u32,
    },
    VaPrefixOverflow {
        bank: String,
    },
    UnalignedBank {
        bank: String,
        va_start: u32,
        rom_extent: u32,
    },
    UnalignedTraversalSeed {
        bank: String,
        pc: u32,
    },
    Materialization {
        bank: String,
        rom_start: u32,
        rom_end: u32,
        reason: String,
    },
    MaterializedLengthMismatch {
        bank: String,
        expected: u32,
        actual: usize,
    },
}

impl std::fmt::Display for PrepareSnapshotBanksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvenMappings => write!(f, "no proven ROM-backed bank to compose"),
            Self::AmbiguousMapping {
                bank,
                distinct_geometries,
            } => write!(
                f,
                "bank {bank} has {distinct_geometries} distinct proven ROM mappings"
            ),
            Self::BankLimitExceeded { banks, limit } => {
                write!(
                    f,
                    "proven bank count {banks} exceeds preparation limit {limit}"
                )
            }
            Self::AggregateRomBytesOverflow => {
                write!(f, "aggregate proven-bank ROM extent overflows u64")
            }
            Self::AggregateRomBytesLimitExceeded { bytes, limit } => write!(
                f,
                "aggregate proven-bank ROM extent {bytes} exceeds preparation limit {limit}"
            ),
            Self::InvertedInterval { bank } => {
                write!(f, "bank {bank} has an inverted ROM or VA interval")
            }
            Self::EmptyRomInterval { bank } => {
                write!(f, "bank {bank} has an empty ROM interval")
            }
            Self::RomExceedsVa {
                bank,
                rom_extent,
                va_extent,
            } => write!(
                f,
                "bank {bank} carries more ROM bytes ({rom_extent}) than VA extent ({va_extent})"
            ),
            Self::VaPrefixOverflow { bank } => {
                write!(f, "bank {bank}'s ROM-backed VA prefix overflows u32")
            }
            Self::UnalignedBank {
                bank,
                va_start,
                rom_extent,
            } => write!(
                f,
                "bank {bank} VA 0x{va_start:08x} and ROM extent {rom_extent} must be word-aligned"
            ),
            Self::UnalignedTraversalSeed { bank, pc } => {
                write!(f, "bank {bank} has unaligned traversal seed 0x{pc:08x}")
            }
            Self::Materialization {
                bank,
                rom_start,
                rom_end,
                reason,
            } => write!(
                f,
                "{bank} ROM interval [0x{rom_start:x},0x{rom_end:x}): {reason}"
            ),
            Self::MaterializedLengthMismatch {
                bank,
                expected,
                actual,
            } => write!(
                f,
                "bank {bank} materialized {actual} bytes for a {expected}-byte ROM interval"
            ),
        }
    }
}

impl std::error::Error for PrepareSnapshotBanksError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MappingGeometry {
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    va_start: u32,
    va_end: u32,
}

/// Prepare every uniquely mapped proven bank in deterministic bank-name order.
///
/// Exact duplicate mapping facts collapse to one input. Distinct proven
/// geometries for the same bank are ambiguous and rejected rather than being
/// emitted as duplicate runtime banks.
pub fn prepare_snapshot_banks(
    rom: &NormalizedRom,
    facts: &FactDb,
) -> Result<PreparedSnapshotBanks, PrepareSnapshotBanksError> {
    prepare_snapshot_banks_with_limits(rom, facts, PrepareSnapshotBanksLimits::default())
}

/// Prepare proven banks within an explicit pre-materialization envelope.
pub fn prepare_snapshot_banks_with_limits(
    rom: &NormalizedRom,
    facts: &FactDb,
    limits: PrepareSnapshotBanksLimits,
) -> Result<PreparedSnapshotBanks, PrepareSnapshotBanksError> {
    let mut geometries: BTreeMap<String, BTreeSet<MappingGeometry>> = BTreeMap::new();
    for fact in facts.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            va_end,
        } = fact
        else {
            unreachable!("proven_rom_mappings returned a non-mapping fact")
        };
        geometries
            .entry(bank.clone())
            .or_default()
            .insert(MappingGeometry {
                rom_space: *rom_space,
                rom_start: *rom_start,
                rom_end: *rom_end,
                va_start: *va_start,
                va_end: *va_end,
            });
    }
    if geometries.is_empty() {
        return Err(PrepareSnapshotBanksError::NoProvenMappings);
    }

    if geometries.len() > limits.max_banks {
        return Err(PrepareSnapshotBanksError::BankLimitExceeded {
            banks: geometries.len(),
            limit: limits.max_banks,
        });
    }

    // Validate every unique geometry and its retained-byte budget before the
    // first physical slice copy or VROM decompression allocation.
    let mut unique = Vec::with_capacity(geometries.len());
    let mut aggregate_rom_bytes = 0u64;
    for (bank, candidates) in geometries {
        if candidates.len() != 1 {
            return Err(PrepareSnapshotBanksError::AmbiguousMapping {
                bank,
                distinct_geometries: candidates.len(),
            });
        }
        let geometry = candidates.into_iter().next().unwrap();
        let rom_extent = geometry
            .rom_end
            .checked_sub(geometry.rom_start)
            .ok_or_else(|| PrepareSnapshotBanksError::InvertedInterval { bank: bank.clone() })?;
        aggregate_rom_bytes = aggregate_rom_bytes
            .checked_add(u64::from(rom_extent))
            .ok_or(PrepareSnapshotBanksError::AggregateRomBytesOverflow)?;
        if aggregate_rom_bytes > limits.max_aggregate_rom_bytes {
            return Err(PrepareSnapshotBanksError::AggregateRomBytesLimitExceeded {
                bytes: aggregate_rom_bytes,
                limit: limits.max_aggregate_rom_bytes,
            });
        }
        unique.push((bank, geometry));
    }

    let mut prepared = Vec::with_capacity(unique.len());
    for (bank, geometry) in unique {
        let (Some(rom_extent), Some(va_extent)) = (
            geometry.rom_end.checked_sub(geometry.rom_start),
            geometry.va_end.checked_sub(geometry.va_start),
        ) else {
            return Err(PrepareSnapshotBanksError::InvertedInterval { bank });
        };
        if rom_extent == 0 {
            return Err(PrepareSnapshotBanksError::EmptyRomInterval { bank });
        }
        let va_end = geometry
            .va_start
            .checked_add(rom_extent)
            .ok_or_else(|| PrepareSnapshotBanksError::VaPrefixOverflow { bank: bank.clone() })?;
        if !geometry.va_start.is_multiple_of(4) || !rom_extent.is_multiple_of(4) {
            return Err(PrepareSnapshotBanksError::UnalignedBank {
                bank,
                va_start: geometry.va_start,
                rom_extent,
            });
        }
        if rom_extent > va_extent {
            return Err(PrepareSnapshotBanksError::RomExceedsVa {
                bank,
                rom_extent,
                va_extent,
            });
        }
        let materialized = banks::materialize_rom_range_bounded(
            rom,
            facts,
            geometry.rom_space,
            geometry.rom_start,
            geometry.rom_end,
            limits.max_decoded_vrom_file_bytes,
        )
        .map_err(|reason| PrepareSnapshotBanksError::Materialization {
            bank: bank.clone(),
            rom_start: geometry.rom_start,
            rom_end: geometry.rom_end,
            reason,
        })?;
        if materialized.bytes.len() != rom_extent as usize {
            return Err(PrepareSnapshotBanksError::MaterializedLengthMismatch {
                bank,
                expected: rom_extent,
                actual: materialized.bytes.len(),
            });
        }
        let traversal_seeds = traversal_seeds(facts, &bank, geometry.va_start, va_end);
        if let Some(pc) = traversal_seeds.iter().copied().find(|pc| pc % 4 != 0) {
            return Err(PrepareSnapshotBanksError::UnalignedTraversalSeed { bank, pc });
        }
        prepared.push(PreparedSnapshotBank {
            bank,
            rom_space: geometry.rom_space,
            rom_start: geometry.rom_start,
            rom_end: geometry.rom_end,
            va_start: geometry.va_start,
            va_end,
            bytes: materialized.bytes,
            traversal_seeds,
            backing_evidence: materialized.backing_evidence,
        });
    }
    Ok(PreparedSnapshotBanks { banks: prepared })
}

fn traversal_seeds(facts: &FactDb, bank: &str, va_start: u32, va_end: u32) -> Vec<u32> {
    let mut roots: BTreeSet<u32> = facts.proven_function_entries(bank).into_iter().collect();
    roots.retain(|pc| *pc >= va_start && *pc < va_end);
    for fact in facts.facts() {
        let Fact::FunctionEntryClaim {
            target,
            evidence,
            proposed_state,
            ..
        } = fact
        else {
            continue;
        };
        if target.bank != bank
            || target.pc < va_start
            || target.pc >= va_end
            || !matches!(
                proposed_state,
                ProofState::Candidate | ProofState::Supported | ProofState::Proven
            )
            || !matches!(
                evidence,
                FunctionEntryEvidence::DirectJal { .. }
                    | FunctionEntryEvidence::ResolvedJalr { .. }
                    | FunctionEntryEvidence::ExhaustiveIndirectCall { .. }
                    | FunctionEntryEvidence::TableEntry { .. }
                    | FunctionEntryEvidence::HandlerTablePointer { .. }
            )
            || facts
                .conclusion(&function_entry_subject(target))
                .is_some_and(|conclusion| {
                    matches!(conclusion.state, ProofState::Open | ProofState::Conflict)
                })
        {
            continue;
        }
        roots.insert(target.pc);
    }
    roots.into_iter().collect()
}
