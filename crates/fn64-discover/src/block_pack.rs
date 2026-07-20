//! Versioned, content-bound Recompiler Pack for function-independent blocks.
//!
//! The portable pack contains identities, geometry, terminators, and hashes,
//! never ROM words. Materialization re-verifies the normalized ROM and each
//! block digest before exposing instruction words to a code generator.

use crate::block_proof::{BlockAssessment, ReachableCodeBlock};
use crate::snapshot::ProgramSnapshotV1;
use crate::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const BLOCK_PACK_SCHEMA_V1: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedBlockV1 {
    pub start_va: u32,
    pub end_va: u32,
    pub rom_start: u32,
    pub rom_end: u32,
    pub bytes_sha256: String,
    pub terminator: crate::cfg::BlockTerminator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedBankV1 {
    pub bank: String,
    pub bank_id: u64,
    pub blocks: Vec<PackedBlockV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockPackV1 {
    pub schema_version: u32,
    pub normalized_rom_sha256: String,
    pub banks: Vec<PackedBankV1>,
}

#[derive(Debug, Clone)]
pub struct MaterializedPackedBlock {
    pub start_va: u32,
    pub words: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct MaterializedPackedBank {
    pub bank: String,
    pub bank_id: u64,
    pub blocks: Vec<MaterializedPackedBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPackError {
    RomIdentityMismatch,
    NoProvenBlocks {
        bank: String,
    },
    NonPhysicalBacking {
        bank: String,
        start_va: u32,
    },
    InvalidGeometry {
        bank: String,
        start_va: u32,
    },
    OverlappingBlocks {
        bank: String,
        left: u32,
        right: u32,
    },
    RomRangeOutsideImage {
        bank: String,
        rom_start: u32,
        rom_end: u32,
    },
    BlockDigestMismatch {
        bank: String,
        start_va: u32,
    },
}

impl std::fmt::Display for BlockPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for BlockPackError {}

pub fn emit_block_pack_v1(
    snapshot: &ProgramSnapshotV1,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    if snapshot.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut banks = Vec::with_capacity(snapshot.banks.len());
    let mut bank_ids = BTreeSet::new();
    for bank_snapshot in &snapshot.banks {
        let bank = &bank_snapshot.input.bank;
        let mut proven: Vec<&ReachableCodeBlock> = bank_snapshot
            .block_proof
            .assessments
            .iter()
            .filter_map(|assessment| match assessment {
                BlockAssessment::Proven { block } => Some(block),
                BlockAssessment::Candidate { .. } => None,
            })
            .collect();
        proven.sort_by_key(|block| block.start_va);
        if proven.is_empty() {
            return Err(BlockPackError::NoProvenBlocks { bank: bank.clone() });
        }
        validate_geometry(bank, &proven)?;
        let bank_id = stable_bank_id(&snapshot.normalized_rom_sha256, bank);
        if !bank_ids.insert(bank_id) {
            return Err(BlockPackError::OverlappingBlocks {
                bank: bank.clone(),
                left: 0,
                right: 0,
            });
        }
        let mut blocks = Vec::with_capacity(proven.len());
        for block in proven {
            if block.rom_space != crate::facts::RomAddressSpace::Physical {
                return Err(BlockPackError::NonPhysicalBacking {
                    bank: bank.clone(),
                    start_va: block.start_va,
                });
            }
            let bytes = rom
                .bytes
                .get(block.rom_start as usize..block.rom_end as usize)
                .ok_or(BlockPackError::RomRangeOutsideImage {
                    bank: bank.clone(),
                    rom_start: block.rom_start,
                    rom_end: block.rom_end,
                })?;
            blocks.push(PackedBlockV1 {
                start_va: block.start_va,
                end_va: block.end_va,
                rom_start: block.rom_start,
                rom_end: block.rom_end,
                bytes_sha256: sha256_hex(bytes),
                terminator: block.terminator.clone(),
            });
        }
        banks.push(PackedBankV1 {
            bank: bank.clone(),
            bank_id,
            blocks,
        });
    }
    banks.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(BlockPackV1 {
        schema_version: BLOCK_PACK_SCHEMA_V1,
        normalized_rom_sha256: snapshot.normalized_rom_sha256.clone(),
        banks,
    })
}

pub fn materialize_block_pack(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
) -> Result<Vec<MaterializedPackedBank>, BlockPackError> {
    if pack.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut output = Vec::with_capacity(pack.banks.len());
    for bank in &pack.banks {
        let mut blocks = Vec::with_capacity(bank.blocks.len());
        for block in &bank.blocks {
            let bytes = rom
                .bytes
                .get(block.rom_start as usize..block.rom_end as usize)
                .ok_or(BlockPackError::RomRangeOutsideImage {
                    bank: bank.bank.clone(),
                    rom_start: block.rom_start,
                    rom_end: block.rom_end,
                })?;
            if sha256_hex(bytes) != block.bytes_sha256 {
                return Err(BlockPackError::BlockDigestMismatch {
                    bank: bank.bank.clone(),
                    start_va: block.start_va,
                });
            }
            blocks.push(MaterializedPackedBlock {
                start_va: block.start_va,
                words: bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                    .collect(),
            });
        }
        output.push(MaterializedPackedBank {
            bank: bank.bank.clone(),
            bank_id: bank.bank_id,
            blocks,
        });
    }
    Ok(output)
}

/// Feed a re-verified materialized bank into the sparse arbitrary-PC emitter.
///
/// The adapter deliberately preserves the pack's disjoint spans. It never
/// widens them to one bounding interval, so bytes in code/data gaps cannot be
/// decoded or acquire same-bank transfer authority.
pub fn emit_materialized_bank_runner(bank: &MaterializedPackedBank, name: &str) -> String {
    let blocks: Vec<fn64_recomp_rs::BankBlockInput<'_>> = bank
        .blocks
        .iter()
        .map(|block| fn64_recomp_rs::BankBlockInput {
            vram: block.start_va,
            words: &block.words,
        })
        .collect();
    fn64_recomp_rs::emit_sparse_bank_runner(&fn64_recomp_rs::SparseBankInput {
        name,
        bank: fn64_recomp_rs::BankId::new(bank.bank_id),
        blocks: &blocks,
    })
}

/// Convert a re-verified pack bank into the runtime's owned sparse catalog
/// type without flattening gaps.
pub fn materialized_code_bank(
    bank: &MaterializedPackedBank,
) -> Result<fn64_recomp_rs::CodeBank, fn64_recomp_rs::BankError> {
    let id = fn64_recomp_rs::BankId::new(bank.bank_id);
    let spans = bank
        .blocks
        .iter()
        .map(|block| {
            fn64_recomp_rs::CodeSpan::new(
                id,
                fn64_recomp_rs::GuestPc::new(block.start_va),
                block.words.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    fn64_recomp_rs::CodeBank::from_spans(id, spans)
}

fn validate_geometry(bank: &str, blocks: &[&ReachableCodeBlock]) -> Result<(), BlockPackError> {
    let mut previous_end = None;
    for block in blocks {
        if !block.start_va.is_multiple_of(4)
            || !block.end_va.is_multiple_of(4)
            || block.end_va <= block.start_va
            || block.rom_end.checked_sub(block.rom_start)
                != block.end_va.checked_sub(block.start_va)
        {
            return Err(BlockPackError::InvalidGeometry {
                bank: bank.into(),
                start_va: block.start_va,
            });
        }
        if let Some(end) = previous_end {
            if block.start_va < end {
                return Err(BlockPackError::OverlappingBlocks {
                    bank: bank.into(),
                    left: end,
                    right: block.start_va,
                });
            }
        }
        previous_end = Some(block.end_va);
    }
    Ok(())
}

fn stable_bank_id(rom_sha256: &str, bank: &str) -> u64 {
    let digest = Sha256::digest(format!("fn64:block-pack:v1:{rom_sha256}:{bank}").as_bytes());
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
