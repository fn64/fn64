//! Proven-bank preparation for snapshot composition.
//!
//! This is the shared boundary between discovery facts and any in-process
//! snapshot producer. It admits only proven bank images, re-derives their typed
//! affine or evaluated backing, excludes affine load-time `.bss` tails, and
//! derives deterministic traversal seeds. Those seeds guide closure only; the
//! composer still derives callable-entry authority from the fact database.

use crate::banks;
use crate::facts::{
    evaluated_image_receipt_sha256_v1, function_entry_subject, BankBackingV1,
    EvaluatedImageReceiptV1, Fact, FactDb, FunctionEntryEvidence, ProofState, RomAddressSpace,
};
use crate::materialized_image::{rederive_materialized_image_v1, MaterializedImageLimitsV1};
use crate::snapshot::MaterializedBankInput;
use crate::NormalizedRom;
use std::collections::{BTreeMap, BTreeSet};

const MIB: u64 = 1024 * 1024;

/// Resource envelope applied before any bank bytes are materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareSnapshotBanksLimits {
    pub max_banks: usize,
    /// Maximum retained decoded bytes across all prepared banks.
    pub max_aggregate_materialized_bytes: u64,
    /// Per-image transient evaluator limits. Its decoded-VROM-file limit also
    /// bounds affine VROM/Yaz0 materialization.
    pub materialized_image: MaterializedImageLimitsV1,
}

impl Default for PrepareSnapshotBanksLimits {
    fn default() -> Self {
        Self {
            max_banks: 4096,
            max_aggregate_materialized_bytes: 256 * MIB,
            materialized_image: MaterializedImageLimitsV1::default(),
        }
    }
}

/// One proven bank ready for snapshot composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSnapshotBank {
    pub bank: String,
    pub backing: BankBackingV1,
    pub va_start: u32,
    /// Exclusive end of retained bytes. Affine load-time `.bss` is excluded.
    pub va_end: u32,
    pub bytes: Vec<u8>,
    /// Closure traversal hints, never callable-entry authority. Table-derived
    /// entries here are callable evidence exposed by an admitted table, not a
    /// claim that a call site itself was observed.
    pub traversal_seeds: Vec<u32>,
    /// Stable fact indices needed to re-read a Virtual-ROM source. Physical
    /// affine and evaluated sources require no backing facts.
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
    NoProvenImages,
    AmbiguousBankImage {
        bank: String,
        distinct_images: usize,
    },
    BankLimitExceeded {
        banks: usize,
        limit: usize,
    },
    AggregateMaterializedBytesOverflow,
    AggregateMaterializedBytesLimitExceeded {
        bytes: u64,
        limit: u64,
    },
    InvertedRomOrVaInterval {
        bank: String,
    },
    EmptyImage {
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
    MaterializedOutputExtentMismatch {
        bank: String,
        output_len: u32,
        va_extent: u32,
    },
    UnalignedBank {
        bank: String,
        va_start: u32,
        byte_len: u32,
    },
    UnalignedTraversalSeed {
        bank: String,
        pc: u32,
    },
    RomMaterialization {
        bank: String,
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
        reason: String,
    },
    EvaluatedImageRederivation {
        bank: String,
        receipt_sha256: String,
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
            Self::NoProvenImages => write!(f, "no proven bank image to prepare"),
            Self::AmbiguousBankImage {
                bank,
                distinct_images,
            } => write!(
                f,
                "bank {bank} has {distinct_images} distinct proven image backings"
            ),
            Self::BankLimitExceeded { banks, limit } => {
                write!(
                    f,
                    "proven bank count {banks} exceeds preparation limit {limit}"
                )
            }
            Self::AggregateMaterializedBytesOverflow => {
                write!(f, "aggregate proven-bank retained bytes overflow u64")
            }
            Self::AggregateMaterializedBytesLimitExceeded { bytes, limit } => write!(
                f,
                "aggregate proven-bank retained bytes {bytes} exceeds preparation limit {limit}"
            ),
            Self::InvertedRomOrVaInterval { bank } => {
                write!(f, "bank {bank} has an inverted ROM or VA interval")
            }
            Self::EmptyImage { bank } => {
                write!(f, "bank {bank} has an empty retained image")
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
                write!(f, "bank {bank}'s retained VA prefix overflows u32")
            }
            Self::MaterializedOutputExtentMismatch {
                bank,
                output_len,
                va_extent,
            } => {
                write!(
                    f,
                    "bank {bank} evaluated output length {output_len} does not equal VA extent {va_extent}"
                )
            }
            Self::UnalignedBank {
                bank,
                va_start,
                byte_len,
            } => write!(
                f,
                "bank {bank} VA 0x{va_start:08x} and retained length {byte_len} must be word-aligned"
            ),
            Self::UnalignedTraversalSeed { bank, pc } => {
                write!(f, "bank {bank} has unaligned traversal seed 0x{pc:08x}")
            }
            Self::RomMaterialization {
                bank,
                rom_space,
                rom_start,
                rom_end,
                reason,
            } => write!(
                f,
                "{bank} {rom_space:?} ROM interval [0x{rom_start:x},0x{rom_end:x}): {reason}"
            ),
            Self::EvaluatedImageRederivation {
                bank,
                receipt_sha256,
                reason,
            } => write!(
                f,
                "{bank} evaluated image receipt {receipt_sha256}: {reason}"
            ),
            Self::MaterializedLengthMismatch {
                bank,
                expected,
                actual,
            } => write!(
                f,
                "bank {bank} materialized {actual} bytes for a {expected}-byte retained image"
            ),
        }
    }
}

impl std::error::Error for PrepareSnapshotBanksError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ImageGeometry {
    RomAffine {
        rom_space: RomAddressSpace,
        rom_start: u32,
        rom_end: u32,
        va_start: u32,
        va_end: u32,
    },
    Materialized {
        va_start: u32,
        va_end: u32,
        receipt: EvaluatedImageReceiptV1,
    },
}

/// Prepare every uniquely backed proven bank in deterministic bank-name order.
///
/// Exact duplicate facts collapse to one input. Distinct affine/materialized
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
    let mut geometries: BTreeMap<String, BTreeSet<ImageGeometry>> = BTreeMap::new();
    for fact in facts.facts() {
        let (bank, geometry) = match fact {
            Fact::RomMapping {
                bank,
                rom_space,
                rom_start,
                rom_end,
                va_start,
                va_end,
            } => (
                bank,
                ImageGeometry::RomAffine {
                    rom_space: *rom_space,
                    rom_start: *rom_start,
                    rom_end: *rom_end,
                    va_start: *va_start,
                    va_end: *va_end,
                },
            ),
            Fact::EvaluatedImage {
                bank,
                va_start,
                va_end,
                receipt,
            } => (
                bank,
                ImageGeometry::Materialized {
                    va_start: *va_start,
                    va_end: *va_end,
                    receipt: receipt.clone(),
                },
            ),
            _ => continue,
        };
        if !facts
            .conclusion(&format!("bank:{bank}"))
            .is_some_and(|conclusion| conclusion.state == ProofState::Proven)
        {
            continue;
        }
        geometries.entry(bank.clone()).or_default().insert(geometry);
    }
    if geometries.is_empty() {
        return Err(PrepareSnapshotBanksError::NoProvenImages);
    }

    if geometries.len() > limits.max_banks {
        return Err(PrepareSnapshotBanksError::BankLimitExceeded {
            banks: geometries.len(),
            limit: limits.max_banks,
        });
    }

    // Validate every unique geometry and its decoded retained-byte budget
    // before the first physical slice copy, VROM decode, or evaluator output
    // allocation.
    let mut unique = Vec::with_capacity(geometries.len());
    let mut aggregate_materialized_bytes = 0u64;
    for (bank, candidates) in geometries {
        if candidates.len() != 1 {
            return Err(PrepareSnapshotBanksError::AmbiguousBankImage {
                bank,
                distinct_images: candidates.len(),
            });
        }
        let geometry = candidates.into_iter().next().unwrap();
        let (va_start, byte_len) = match &geometry {
            ImageGeometry::RomAffine {
                rom_start,
                rom_end,
                va_start,
                va_end,
                ..
            } => {
                let rom_extent = rom_end.checked_sub(*rom_start).ok_or_else(|| {
                    PrepareSnapshotBanksError::InvertedRomOrVaInterval { bank: bank.clone() }
                })?;
                let va_extent = va_end.checked_sub(*va_start).ok_or_else(|| {
                    PrepareSnapshotBanksError::InvertedRomOrVaInterval { bank: bank.clone() }
                })?;
                if rom_extent == 0 {
                    return Err(PrepareSnapshotBanksError::EmptyImage { bank });
                }
                va_start.checked_add(rom_extent).ok_or_else(|| {
                    PrepareSnapshotBanksError::VaPrefixOverflow { bank: bank.clone() }
                })?;
                if rom_extent > va_extent {
                    return Err(PrepareSnapshotBanksError::RomExceedsVa {
                        bank,
                        rom_extent,
                        va_extent,
                    });
                }
                (*va_start, rom_extent)
            }
            ImageGeometry::Materialized {
                va_start,
                va_end,
                receipt,
            } => {
                let va_extent = va_end.checked_sub(*va_start).ok_or_else(|| {
                    PrepareSnapshotBanksError::InvertedRomOrVaInterval { bank: bank.clone() }
                })?;
                if receipt.output_len == 0 {
                    return Err(PrepareSnapshotBanksError::EmptyImage { bank });
                }
                if receipt.output_len != va_extent {
                    return Err(
                        PrepareSnapshotBanksError::MaterializedOutputExtentMismatch {
                            bank,
                            output_len: receipt.output_len,
                            va_extent,
                        },
                    );
                }
                (*va_start, receipt.output_len)
            }
        };
        va_start
            .checked_add(byte_len)
            .ok_or_else(|| PrepareSnapshotBanksError::VaPrefixOverflow { bank: bank.clone() })?;
        if !va_start.is_multiple_of(4) || !byte_len.is_multiple_of(4) {
            return Err(PrepareSnapshotBanksError::UnalignedBank {
                bank,
                va_start,
                byte_len,
            });
        }
        aggregate_materialized_bytes = aggregate_materialized_bytes
            .checked_add(u64::from(byte_len))
            .ok_or(PrepareSnapshotBanksError::AggregateMaterializedBytesOverflow)?;
        if aggregate_materialized_bytes > limits.max_aggregate_materialized_bytes {
            return Err(
                PrepareSnapshotBanksError::AggregateMaterializedBytesLimitExceeded {
                    bytes: aggregate_materialized_bytes,
                    limit: limits.max_aggregate_materialized_bytes,
                },
            );
        }
        unique.push((bank, geometry));
    }

    let mut prepared = Vec::with_capacity(unique.len());
    for (bank, geometry) in unique {
        let (backing, va_start, va_end, bytes, backing_evidence) = match geometry {
            ImageGeometry::RomAffine {
                rom_space,
                rom_start,
                rom_end,
                va_start,
                ..
            } => {
                let byte_len = rom_end - rom_start;
                let va_end = va_start.checked_add(byte_len).ok_or_else(|| {
                    PrepareSnapshotBanksError::VaPrefixOverflow { bank: bank.clone() }
                })?;
                let materialized = banks::materialize_rom_range_bounded(
                    rom,
                    facts,
                    rom_space,
                    rom_start,
                    rom_end,
                    limits.materialized_image.max_decoded_vrom_file_bytes,
                )
                .map_err(|reason| {
                    PrepareSnapshotBanksError::RomMaterialization {
                        bank: bank.clone(),
                        rom_space,
                        rom_start,
                        rom_end,
                        reason,
                    }
                })?;
                (
                    BankBackingV1::RomAffine {
                        rom_space,
                        rom_start,
                        rom_end,
                    },
                    va_start,
                    va_end,
                    materialized.bytes,
                    materialized.backing_evidence,
                )
            }
            ImageGeometry::Materialized {
                va_start,
                va_end,
                receipt,
            } => {
                let receipt_sha256 = evaluated_image_receipt_sha256_v1(&receipt);
                let evaluation =
                    rederive_materialized_image_v1(rom, facts, &receipt, limits.materialized_image)
                        .map_err(
                            |error| PrepareSnapshotBanksError::EvaluatedImageRederivation {
                                bank: bank.clone(),
                                receipt_sha256: receipt_sha256.clone(),
                                reason: error.to_string(),
                            },
                        )?;
                (
                    BankBackingV1::Materialized {
                        receipt_sha256,
                        output_len: receipt.output_len,
                    },
                    va_start,
                    va_end,
                    evaluation.bytes().to_vec(),
                    evaluation.source_backing_evidence().to_vec(),
                )
            }
        };
        let expected = va_end - va_start;
        if bytes.len() != expected as usize {
            return Err(PrepareSnapshotBanksError::MaterializedLengthMismatch {
                bank,
                expected,
                actual: bytes.len(),
            });
        }
        let traversal_seeds = traversal_seeds(facts, &bank, va_start, va_end);
        if let Some(pc) = traversal_seeds.iter().copied().find(|pc| pc % 4 != 0) {
            return Err(PrepareSnapshotBanksError::UnalignedTraversalSeed { bank, pc });
        }
        prepared.push(PreparedSnapshotBank {
            bank,
            backing,
            va_start,
            va_end,
            bytes,
            traversal_seeds,
            backing_evidence,
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
