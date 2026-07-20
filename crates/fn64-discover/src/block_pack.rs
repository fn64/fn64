//! Versioned, content-bound Recompiler Pack for function-independent blocks.
//!
//! The portable pack contains identities, geometry, terminators, and hashes,
//! never ROM words. Materialization re-verifies the normalized ROM and each
//! block digest before exposing instruction words to a code generator.

use crate::block_proof::{BlockAssessment, ReachableCodeBlock};
use crate::cfg::WordClass;
use crate::snapshot::ProgramSnapshotV1;
use crate::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

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
        let geometry =
            complete_severed_delay_slots(&proven, &bank_snapshot.closure.cfg.word_class, rom);
        validate_completed_geometry(bank, &geometry)?;
        let bank_id = stable_bank_id(&snapshot.normalized_rom_sha256, bank);
        if !bank_ids.insert(bank_id) {
            return Err(BlockPackError::OverlappingBlocks {
                bank: bank.clone(),
                left: 0,
                right: 0,
            });
        }
        let mut blocks = Vec::with_capacity(geometry.len());
        for (block, geom) in proven.iter().zip(&geometry) {
            if block.rom_space != crate::facts::RomAddressSpace::Physical {
                return Err(BlockPackError::NonPhysicalBacking {
                    bank: bank.clone(),
                    start_va: block.start_va,
                });
            }
            let bytes = rom
                .bytes
                .get(geom.rom_start as usize..geom.rom_end as usize)
                .ok_or(BlockPackError::RomRangeOutsideImage {
                    bank: bank.clone(),
                    rom_start: geom.rom_start,
                    rom_end: geom.rom_end,
                })?;
            blocks.push(PackedBlockV1 {
                start_va: geom.start_va,
                end_va: geom.end_va,
                rom_start: geom.rom_start,
                rom_end: geom.rom_end,
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

/// A proven block's emitted VA/ROM extents after delay-slot completion. Byte
/// length is preserved between the VA and ROM views.
#[derive(Clone, Copy)]
struct CompletedGeometry {
    start_va: u32,
    end_va: u32,
    rom_start: u32,
    rom_end: u32,
}

/// Realize the "control transfer and its delay slot are one architecturally
/// inseparable unit" invariant (DISCOVER-DESIGN Phase 4) at emission time.
///
/// `canonicalize_blocks` cuts a block at any later-discovered leader inside it,
/// replacing the control terminator with `Fallthrough` and handing the trailing
/// words to the leader's block. When the leader lands *on* a delay slot — the
/// rare MIPS "`jal`/branch into a delay slot" hazard — the cut strands the
/// control transfer (now the block's last word) from its delay slot. If the
/// leader's block is itself proven the delay slot is admitted and nothing is
/// needed; but when that leader is a contested owner root its block is only a
/// candidate and is dropped, so the delay slot is admitted by no proven block.
///
/// This re-attaches exactly that one stranded delay slot: a proven block whose
/// final word is a delay-slotted control transfer and whose end address (that
/// control transfer's delay slot) is *proven code* admitted by no proven block
/// is extended by that one contiguous ROM word. The `ProvenCode` requirement is
/// the precise discriminator: a genuine control transfer's delay slot is always
/// proven code (the CFG's delay-slot validation marks it so before the block is
/// proven), so it is exactly the stranded word to re-attach. A block whose last
/// word merely *decodes* as a control transfer but is actually a delay slot or
/// misclassified data has an unproven (or absent) successor word, so this leaves
/// it untouched — the emitter handles that word as the ordinary or unexecutable
/// instruction it is.
///
/// Soundness: the delay slot is architecturally proven code reached whenever the
/// control transfer is, its ROM backing is the block's own next contiguous word
/// under the same physical mapping, and the extension only ever fills a word no
/// proven block admits — so it never overlaps a sibling. Blocks whose delay slot
/// is already admitted are left byte-for-byte unchanged, so the
/// CFG/owner/block-proof geometry the scoreboard reads is untouched: this
/// regroups proven words for emission, it never changes which words are proven
/// code.
fn complete_severed_delay_slots(
    blocks: &[&ReachableCodeBlock],
    word_class: &BTreeMap<u32, WordClass>,
    rom: &NormalizedRom,
) -> Vec<CompletedGeometry> {
    let admitted: BTreeSet<u32> = blocks
        .iter()
        .flat_map(|block| (block.start_va..block.end_va).step_by(4))
        .collect();
    blocks
        .iter()
        .map(|block| {
            let mut geom = CompletedGeometry {
                start_va: block.start_va,
                end_va: block.end_va,
                rom_start: block.rom_start,
                rom_end: block.rom_end,
            };
            let delay_slot_va = block.end_va;
            let last_word_control = block
                .rom_end
                .checked_sub(4)
                .and_then(|off| rom.bytes.get(off as usize..off as usize + 4))
                .map(|bytes| {
                    fn64_recomp_rs::decode(u32::from_be_bytes(bytes.try_into().unwrap()))
                        .has_delay_slot()
                })
                .unwrap_or(false);
            if last_word_control
                && word_class.get(&delay_slot_va) == Some(&WordClass::ProvenCode)
                && !admitted.contains(&delay_slot_va)
            {
                if let (Some(end_va), Some(rom_end)) =
                    (geom.end_va.checked_add(4), geom.rom_end.checked_add(4))
                {
                    geom.end_va = end_va;
                    geom.rom_end = rom_end;
                }
            }
            geom
        })
        .collect()
}

fn validate_completed_geometry(
    bank: &str,
    blocks: &[CompletedGeometry],
) -> Result<(), BlockPackError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::BlockTerminator;
    use crate::facts::RomAddressSpace;
    use crate::normalize;

    const BASE: u32 = 0x8000_0000;
    const ROM_START: u32 = 0x1000;
    const JR_RA: u32 = 0x03e0_0008; // jr $ra (control transfer, has delay slot)
    const NOP: u32 = 0x0000_0000;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    fn rom_with(words: &[u32]) -> NormalizedRom {
        let bank = asm(words);
        let mut bytes = vec![0u8; ROM_START as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&BASE.to_be_bytes());
        bytes[ROM_START as usize..].copy_from_slice(&bank);
        normalize(&bytes).unwrap()
    }

    fn block(start_va: u32, end_va: u32, terminator: BlockTerminator) -> ReachableCodeBlock {
        ReachableCodeBlock {
            bank: "boot".into(),
            start_va,
            end_va,
            owner_root: BASE,
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_START + (start_va - BASE),
            rom_end: ROM_START + (end_va - BASE),
            terminator,
        }
    }

    /// A `jal` whose target lands ON a delay slot makes that delay-slot address
    /// a canonical block leader; `canonicalize_blocks` truncates the control
    /// block there and drops the ambiguous leader block, so the control transfer
    /// is stranded at its block's last word with a proven-but-unadmitted delay
    /// slot. `complete_severed_delay_slots` must re-attach that one word, keeping
    /// the control transfer and its delay slot in one emitted unit.
    #[test]
    fn severed_proven_delay_slot_is_reattached_to_its_control_block() {
        // Bank: nop ; jr $ra ; nop(delay). A separate leader landed on the delay
        // slot (index 2), truncating the block to [0,2) with the jr as its last
        // word and the delay slot admitted by no proven block.
        let rom = rom_with(&[NOP, JR_RA, NOP]);
        // Control block ends at the delay slot (BASE+8): jr at BASE+4 is its last
        // word, delay slot BASE+8 is proven code but in no proven block.
        let control = block(
            BASE,
            BASE + 8,
            BlockTerminator::Fallthrough { next: BASE + 8 },
        );
        let word_class = BTreeMap::from([
            (BASE, WordClass::ProvenCode),
            (BASE + 4, WordClass::ProvenCode),
            (BASE + 8, WordClass::ProvenCode),
        ]);
        let geom = complete_severed_delay_slots(&[&control], &word_class, &rom);
        assert_eq!(geom.len(), 1);
        // Re-attached: the block now spans through the delay slot (BASE+0xC).
        assert_eq!(geom[0].end_va, BASE + 0x0C);
        assert_eq!(geom[0].rom_end, ROM_START + 0x0C);
    }

    /// When the delay slot of a severed control transfer is already admitted by
    /// the next proven block, nothing is re-attached: the unit is whole and
    /// extending would overlap the sibling.
    #[test]
    fn already_admitted_delay_slot_is_left_unchanged() {
        let rom = rom_with(&[NOP, JR_RA, NOP, JR_RA, NOP]);
        let control = block(
            BASE,
            BASE + 8,
            BlockTerminator::Fallthrough { next: BASE + 8 },
        );
        // The delay slot BASE+8 starts the next proven block, so it is admitted.
        let next = block(BASE + 8, BASE + 0x14, BlockTerminator::Return);
        let word_class: BTreeMap<u32, WordClass> = (0..5)
            .map(|i| (BASE + i * 4, WordClass::ProvenCode))
            .collect();
        let geom = complete_severed_delay_slots(&[&control, &next], &word_class, &rom);
        assert_eq!(
            geom[0].end_va,
            BASE + 8,
            "admitted delay slot must not extend"
        );
        assert_eq!(geom[1].start_va, BASE + 8);
    }

    /// A block whose last word merely DECODES as a control transfer but is a
    /// delay slot / misclassified data (its successor word is unproven) is left
    /// untouched: the `ProvenCode` discriminator excludes it, so the emitter —
    /// not the pack — handles that word.
    #[test]
    fn control_shaped_word_with_unproven_successor_is_not_extended() {
        let rom = rom_with(&[NOP, JR_RA, JR_RA]);
        let control = block(BASE, BASE + 0x0C, BlockTerminator::Return);
        // The block's last word (BASE+8) decodes as jr but its successor BASE+0xC
        // is NOT proven code (data run) — do not extend.
        let word_class = BTreeMap::from([
            (BASE, WordClass::ProvenCode),
            (BASE + 4, WordClass::ProvenCode),
            (BASE + 8, WordClass::ProvenCode),
        ]);
        let geom = complete_severed_delay_slots(&[&control], &word_class, &rom);
        assert_eq!(
            geom[0].end_va,
            BASE + 0x0C,
            "unproven successor must not be pulled in"
        );
    }
}
