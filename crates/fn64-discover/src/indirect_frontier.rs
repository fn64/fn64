//! Content-free shape census for the unresolved-indirect frontier.
//!
//! This module is diagnostic only. It classifies the transfer register and
//! its nearest writer in the same basic block; it does not claim a reaching
//! definition across predecessors and cannot promote discovery authority.

use crate::cfg::{BasicBlock, BlockTerminator};
use crate::resolve::{written_gpr, ClosureResult, IndirectProofState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterFamilyV1 {
    Zero,
    AssemblerTemporary,
    ReturnValue,
    Argument,
    Temporary,
    Saved,
    Kernel,
    GlobalPointer,
    StackPointer,
    FramePointer,
    ReturnAddress,
}

impl RegisterFamilyV1 {
    fn from_register(register: u8) -> Self {
        match register {
            0 => Self::Zero,
            1 => Self::AssemblerTemporary,
            2..=3 => Self::ReturnValue,
            4..=7 => Self::Argument,
            8..=15 | 24..=25 => Self::Temporary,
            16..=23 => Self::Saved,
            26..=27 => Self::Kernel,
            28 => Self::GlobalPointer,
            29 => Self::StackPointer,
            30 => Self::FramePointer,
            31 => Self::ReturnAddress,
            _ => unreachable!("a MIPS GPR index is five bits"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalDefinitionShapeV1 {
    /// No writer exists between this basic block's start and the transfer.
    LiveIn,
    Load {
        base: RegisterFamilyV1,
    },
    RegisterCopy {
        source: RegisterFamilyV1,
    },
    Immediate,
    Arithmetic,
    CoprocessorMove,
    AtomicResult,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectShapeCountV1 {
    pub via_call: bool,
    pub transfer_register: RegisterFamilyV1,
    pub local_definition: LocalDefinitionShapeV1,
    pub sites: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectFrontierV1 {
    pub open_sites: u64,
    pub shapes: Vec<OpenIndirectShapeCountV1>,
}

impl OpenIndirectFrontierV1 {
    pub fn merge(&mut self, other: &Self) {
        let mut counts = self.shape_counts();
        for row in &other.shapes {
            *counts
                .entry((row.via_call, row.transfer_register, row.local_definition))
                .or_default() += row.sites;
        }
        self.open_sites += other.open_sites;
        self.shapes = rows_from_counts(counts);
    }

    fn shape_counts(&self) -> BTreeMap<(bool, RegisterFamilyV1, LocalDefinitionShapeV1), u64> {
        self.shapes
            .iter()
            .map(|row| {
                (
                    (row.via_call, row.transfer_register, row.local_definition),
                    row.sites,
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIndirectFrontierError {
    DuplicateIndirectBlock,
    OpenSiteMissingIndirectBlock,
    TransferKindMismatch,
    TransferWordUnavailable,
    TransferOpcodeMismatch,
    BlockWordUnavailable,
}

impl std::fmt::Display for OpenIndirectFrontierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DuplicateIndirectBlock => "two indirect blocks have the same transfer site",
            Self::OpenSiteMissingIndirectBlock => {
                "an open resolution has no matching indirect CFG block"
            }
            Self::TransferKindMismatch => {
                "an open resolution disagrees with its CFG block's transfer kind"
            }
            Self::TransferWordUnavailable => "an indirect transfer word is outside the bank",
            Self::TransferOpcodeMismatch => "an indirect CFG block does not end in jr/jalr",
            Self::BlockWordUnavailable => "an indirect block word is outside the bank",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OpenIndirectFrontierError {}

/// Classify final `Open` indirects by content-free local instruction shape.
///
/// The nearest writer is intentionally limited to the site's own basic block.
/// `LiveIn` therefore includes values defined in predecessors as well as true
/// entry-state values. This census is an opportunity-ranking instrument, not
/// an inter-block data-flow result.
pub fn classify_open_indirects_v1(
    closure: &ClosureResult,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<OpenIndirectFrontierV1, OpenIndirectFrontierError> {
    let mut indirect_blocks = BTreeMap::new();
    for block in &closure.cfg.blocks {
        let BlockTerminator::Indirect { via_call } = block.terminator else {
            continue;
        };
        let Some(site_pc) = block.end_va.checked_sub(8) else {
            return Err(OpenIndirectFrontierError::TransferWordUnavailable);
        };
        if indirect_blocks.insert(site_pc, (block, via_call)).is_some() {
            return Err(OpenIndirectFrontierError::DuplicateIndirectBlock);
        }
    }

    let mut counts = BTreeMap::new();
    let mut open_sites = 0u64;
    for resolution in &closure.indirect {
        if resolution.state != IndirectProofState::Open {
            continue;
        }
        let Some((block, via_call)) = indirect_blocks.get(&resolution.site_pc).copied() else {
            return Err(OpenIndirectFrontierError::OpenSiteMissingIndirectBlock);
        };
        if via_call != resolution.via_call {
            return Err(OpenIndirectFrontierError::TransferKindMismatch);
        }
        let transfer_word = read_word(bank_bytes, va_start, resolution.site_pc)
            .ok_or(OpenIndirectFrontierError::TransferWordUnavailable)?;
        let transfer_register = transfer_register(transfer_word, via_call)
            .ok_or(OpenIndirectFrontierError::TransferOpcodeMismatch)?;
        let local_definition = nearest_local_definition(
            block,
            resolution.site_pc,
            transfer_register,
            bank_bytes,
            va_start,
        )?;
        *counts
            .entry((
                via_call,
                RegisterFamilyV1::from_register(transfer_register),
                local_definition,
            ))
            .or_default() += 1;
        open_sites += 1;
    }

    Ok(OpenIndirectFrontierV1 {
        open_sites,
        shapes: rows_from_counts(counts),
    })
}

fn rows_from_counts(
    counts: BTreeMap<(bool, RegisterFamilyV1, LocalDefinitionShapeV1), u64>,
) -> Vec<OpenIndirectShapeCountV1> {
    counts
        .into_iter()
        .map(
            |((via_call, transfer_register, local_definition), sites)| OpenIndirectShapeCountV1 {
                via_call,
                transfer_register,
                local_definition,
                sites,
            },
        )
        .collect()
}

fn read_word(bank_bytes: &[u8], va_start: u32, pc: u32) -> Option<u32> {
    let offset = pc.checked_sub(va_start)? as usize;
    let bytes = bank_bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn transfer_register(word: u32, via_call: bool) -> Option<u8> {
    if word >> 26 != 0 {
        return None;
    }
    let expected_funct = if via_call { 0x09 } else { 0x08 };
    ((word & 0x3f) == expected_funct).then_some(((word >> 21) & 0x1f) as u8)
}

fn nearest_local_definition(
    block: &BasicBlock,
    site_pc: u32,
    transfer_register: u8,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<LocalDefinitionShapeV1, OpenIndirectFrontierError> {
    let mut nearest = None;
    let mut pc = block.start_va;
    while pc < site_pc {
        let word = read_word(bank_bytes, va_start, pc)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
        if written_gpr(word) == Some(transfer_register) {
            nearest = Some(classify_writer(word));
        }
        pc = pc
            .checked_add(4)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
    }
    Ok(nearest.unwrap_or(LocalDefinitionShapeV1::LiveIn))
}

fn classify_writer(word: u32) -> LocalDefinitionShapeV1 {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let shamt = ((word >> 6) & 0x1f) as u8;
    let funct = word & 0x3f;

    if matches!(opcode, 0x1a | 0x1b | 0x20..=0x27 | 0x30 | 0x34 | 0x37) {
        return LocalDefinitionShapeV1::Load {
            base: RegisterFamilyV1::from_register(rs),
        };
    }
    if opcode == 0 {
        let copy_source = match funct {
            0x20 | 0x21 | 0x25 | 0x2c | 0x2d if rt == 0 => Some(rs),
            0x20 | 0x21 | 0x25 | 0x2c | 0x2d if rs == 0 => Some(rt),
            0x00 | 0x38 if shamt == 0 => Some(rt),
            _ => None,
        };
        return copy_source.map_or(LocalDefinitionShapeV1::Arithmetic, |source| {
            LocalDefinitionShapeV1::RegisterCopy {
                source: RegisterFamilyV1::from_register(source),
            }
        });
    }
    if matches!(opcode, 0x08..=0x0f | 0x18 | 0x19) {
        return LocalDefinitionShapeV1::Immediate;
    }
    if matches!(opcode, 0x10..=0x13) && matches!(rs, 0x00..=0x02) {
        return LocalDefinitionShapeV1::CoprocessorMove;
    }
    if matches!(opcode, 0x38 | 0x3c) {
        return LocalDefinitionShapeV1::AtomicResult;
    }
    LocalDefinitionShapeV1::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Cfg, IndirectSite};
    use crate::resolve::{IndirectResolution, IndirectResolutionKind};
    use std::collections::BTreeMap;

    fn closure(words: &[u32], via_call: bool) -> (ClosureResult, Vec<u8>) {
        let bytes = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let site_pc = 0x8000_0000 + (words.len() as u32 - 2) * 4;
        let block = BasicBlock {
            start_va: 0x8000_0000,
            end_va: 0x8000_0000 + words.len() as u32 * 4,
            terminator: BlockTerminator::Indirect { via_call },
        };
        let cfg = Cfg {
            bank: "test".into(),
            word_class: BTreeMap::new(),
            blocks: vec![block],
            direct_calls: vec![],
            tail_transfers: vec![],
            indirect_sites: vec![IndirectSite {
                pc: site_pc,
                via_call,
            }],
            plain_delay_entry_aliases: vec![],
            unsupported_delay_entries: vec![],
            proven_roots: vec![],
        };
        let indirect = vec![IndirectResolution {
            site_pc,
            via_call,
            state: IndirectProofState::Open,
            kind: None::<IndirectResolutionKind>,
            targets: vec![],
            memory_sources: vec![],
        }];
        (ClosureResult { cfg, indirect }, bytes)
    }

    #[test]
    fn classifies_stack_load_feeding_computed_jump() {
        let (closure, bytes) = closure(&[0x8fb9_0010, 0x0320_0008, 0], false);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(report.open_sites, 1);
        assert_eq!(
            report.shapes[0],
            OpenIndirectShapeCountV1 {
                via_call: false,
                transfer_register: RegisterFamilyV1::Temporary,
                local_definition: LocalDefinitionShapeV1::Load {
                    base: RegisterFamilyV1::StackPointer,
                },
                sites: 1,
            }
        );
    }

    #[test]
    fn classifies_live_in_computed_call() {
        let (closure, bytes) = closure(&[0x0320_f809, 0], true);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::LiveIn
        );
        assert!(report.shapes[0].via_call);
    }

    #[test]
    fn classifies_argument_register_copy() {
        let (closure, bytes) = closure(&[0x0080_c825, 0x0320_0008, 0], false);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::RegisterCopy {
                source: RegisterFamilyV1::Argument,
            }
        );
    }

    #[test]
    fn bounded_sites_are_not_counted() {
        let (mut closure, bytes) = closure(&[0x0320_0008, 0], false);
        closure.indirect[0].state = IndirectProofState::Bounded;
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(report.open_sites, 0);
        assert!(report.shapes.is_empty());
    }
}
