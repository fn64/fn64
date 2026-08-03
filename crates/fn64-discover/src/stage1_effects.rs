//! Conservative stage-1 effect inventory over byte-verified bank images.
//!
//! This is a narrow negative classifier, not a purity proof. It inventories
//! syntactic COP0/cache/trap words and resolves memory addresses only when a
//! constant is constructed inside one basic block. Unresolved and ambiguous
//! addresses remain open. KSEG1 alone is never called MMIO; the direct-mapped
//! physical target determines the region.

use crate::cfg::{Cfg, WordClass};
use fn64_recomp_rs::decoder::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectDispositionV1 {
    ReachableInstruction,
    ProvenData,
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntrinsicEffectKindV1 {
    Cache,
    Sync,
    Cop0Read,
    Cop0Write,
    Cop0Control,
    Syscall,
    Break,
}

impl IntrinsicEffectKindV1 {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Sync => "sync",
            Self::Cop0Read => "cop0_read",
            Self::Cop0Write => "cop0_write",
            Self::Cop0Control => "cop0_control",
            Self::Syscall => "syscall",
            Self::Break => "break",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IntrinsicEffectSiteV1 {
    pub bank: String,
    pub guest_pc: u32,
    pub raw_word: u32,
    pub kind: IntrinsicEffectKindV1,
    pub cop0_register: Option<u8>,
    /// Count (9) and Random (1) are explicit. Other COP0 reads remain effects.
    pub nondeterministic_cop0_read: bool,
    pub disposition: EffectDispositionV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAccessKindV1 {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectPhysicalRegionV1 {
    Rdram,
    Rcp,
    Pif,
    Other,
}

impl DirectPhysicalRegionV1 {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rdram => "rdram",
            Self::Rcp => "rcp",
            Self::Pif => "pif",
            Self::Other => "other",
        }
    }

    pub fn is_external_effect_region(self) -> bool {
        matches!(self, Self::Rcp | Self::Pif)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "translation", rename_all = "snake_case")]
pub enum MemoryAddressClassV1 {
    DirectPhysical {
        virtual_address: u32,
        physical_address: u32,
        region: DirectPhysicalRegionV1,
    },
    TlbTranslated {
        virtual_address: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryAddressEvidenceV1 {
    Exact {
        address: MemoryAddressClassV1,
    },
    Open {
        locally_derived: Vec<MemoryAddressClassV1>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MemoryEffectSiteV1 {
    pub bank: String,
    pub guest_pc: u32,
    pub raw_word: u32,
    pub access: MemoryAccessKindV1,
    pub base_register: u8,
    pub offset: i16,
    pub address: MemoryAddressEvidenceV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage1EffectReportV1 {
    pub bank: String,
    pub aligned_word_count: u32,
    pub intrinsic_sites: Vec<IntrinsicEffectSiteV1>,
    pub reachable_memory_sites: Vec<MemoryEffectSiteV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage1EffectSummaryV1 {
    pub bank_count: usize,
    pub aligned_word_count: u64,
    pub reachable_intrinsic_by_kind: BTreeMap<String, u64>,
    pub nondeterministic_cop0_read_count: u64,
    pub proven_data_intrinsic_count: u64,
    pub unclassified_intrinsic_count: u64,
    pub reachable_memory_read_count: u64,
    pub reachable_memory_write_count: u64,
    pub exact_direct_memory_by_region: BTreeMap<String, u64>,
    pub exact_tlb_translated_memory_count: u64,
    pub open_memory_address_count: u64,
    pub obvious_external_effect_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage1EffectScanError {
    UnalignedImage,
    AddressOverflow,
    BlockOutsideImage { start: u32, end: u32 },
}

impl std::fmt::Display for Stage1EffectScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "stage-1 effect scan failed: {self:?}")
    }
}

impl std::error::Error for Stage1EffectScanError {}

fn disposition(word_class: Option<WordClass>) -> EffectDispositionV1 {
    match word_class {
        Some(WordClass::ProvenCode) => EffectDispositionV1::ReachableInstruction,
        Some(WordClass::ProvenData) => EffectDispositionV1::ProvenData,
        _ => EffectDispositionV1::Unclassified,
    }
}

fn intrinsic_effect(instruction: &Instruction) -> Option<(IntrinsicEffectKindV1, Option<u8>)> {
    use Instruction::*;
    match instruction {
        Cache { .. } => Some((IntrinsicEffectKindV1::Cache, None)),
        Sync => Some((IntrinsicEffectKindV1::Sync, None)),
        Mfc0 { cop0d, .. } | Dmfc0 { cop0d, .. } => {
            Some((IntrinsicEffectKindV1::Cop0Read, Some(*cop0d)))
        }
        Mtc0 { cop0d, .. } | Dmtc0 { cop0d, .. } => {
            Some((IntrinsicEffectKindV1::Cop0Write, Some(*cop0d)))
        }
        Eret | Tlbwi | Tlbwr | Tlbp | Tlbr => Some((IntrinsicEffectKindV1::Cop0Control, None)),
        Syscall { .. } => Some((IntrinsicEffectKindV1::Syscall, None)),
        Break { .. } => Some((IntrinsicEffectKindV1::Break, None)),
        _ => None,
    }
}

fn memory_access(word: u32) -> Option<(MemoryAccessKindV1, u8, i16)> {
    let opcode = word >> 26;
    let base = ((word >> 21) & 0x1f) as u8;
    let offset = word as u16 as i16;
    let access = match opcode {
        0x1a | 0x1b | 0x20..=0x27 | 0x30..=0x32 | 0x34..=0x37 => MemoryAccessKindV1::Read,
        0x28..=0x2e | 0x38..=0x3f => MemoryAccessKindV1::Write,
        _ => return None,
    };
    Some((access, base, offset))
}

fn classify_virtual_address(virtual_address: u32) -> MemoryAddressClassV1 {
    let segment = virtual_address & 0xe000_0000;
    if matches!(segment, 0x8000_0000 | 0xa000_0000) {
        let physical_address = virtual_address & 0x1fff_ffff;
        let region = if physical_address < 0x0080_0000 {
            DirectPhysicalRegionV1::Rdram
        } else if (0x03f0_0000..=0x04ff_ffff).contains(&physical_address) {
            DirectPhysicalRegionV1::Rcp
        } else if (0x1fc0_0000..=0x1fc0_07ff).contains(&physical_address) {
            DirectPhysicalRegionV1::Pif
        } else {
            DirectPhysicalRegionV1::Other
        };
        MemoryAddressClassV1::DirectPhysical {
            virtual_address,
            physical_address,
            region,
        }
    } else {
        MemoryAddressClassV1::TlbTranslated { virtual_address }
    }
}

fn possible_gpr_destinations(word: u32) -> [Option<u8>; 2] {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    match opcode {
        0x00 | 0x1c => [Some(rd), None],
        0x01 if matches!(rt, 0x10..=0x13) => [Some(31), None],
        0x03 | 0x1d => [Some(31), None],
        0x08..=0x0f | 0x18..=0x1b | 0x20..=0x27 | 0x30 | 0x34 | 0x37 | 0x38 | 0x3c => {
            [Some(rt), None]
        }
        0x10..=0x13 if rs <= 0x02 => [Some(rt), None],
        0x1f => [Some(rt), Some(rd)],
        _ => [None, None],
    }
}

fn update_constants(registers: &mut [Option<u32>; 32], word: u32) {
    use Instruction::*;
    let exact = match decode(word) {
        Lui { rt, imm } => Some((rt, u32::from(imm) << 16)),
        Addi { rt, rs, imm }
        | Addiu { rt, rs, imm }
        | Daddi { rt, rs, imm }
        | Daddiu { rt, rs, imm } => {
            registers[rs as usize].map(|value| (rt, value.wrapping_add(imm as i32 as u32)))
        }
        Ori { rt, rs, imm } => registers[rs as usize].map(|value| (rt, value | u32::from(imm))),
        Add { rd, rs, rt } | Addu { rd, rs, rt } | Dadd { rd, rs, rt } | Daddu { rd, rs, rt } => {
            registers[rs as usize]
                .zip(registers[rt as usize])
                .map(|(lhs, rhs)| (rd, lhs.wrapping_add(rhs)))
        }
        Or { rd, rs, rt } => registers[rs as usize]
            .zip(registers[rt as usize])
            .map(|(lhs, rhs)| (rd, lhs | rhs)),
        _ => None,
    };
    if let Some((register, value)) = exact {
        if register != 0 {
            registers[register as usize] = Some(value);
        }
    } else {
        for register in possible_gpr_destinations(word).into_iter().flatten() {
            if register != 0 {
                registers[register as usize] = None;
            }
        }
    }
    registers[0] = Some(0);
}

#[derive(Default)]
struct AddressObservations {
    locally_derived: BTreeSet<MemoryAddressClassV1>,
    open: bool,
}

/// Scan one bank using only its authority-rooted CFG. The raw intrinsic scan
/// covers every aligned word; constant memory reasoning resets at each block.
pub fn scan_stage1_effects(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    cfg: &Cfg,
) -> Result<Stage1EffectReportV1, Stage1EffectScanError> {
    if !bank_bytes.len().is_multiple_of(4) {
        return Err(Stage1EffectScanError::UnalignedImage);
    }
    let byte_len =
        u32::try_from(bank_bytes.len()).map_err(|_| Stage1EffectScanError::AddressOverflow)?;
    let va_end = va_start
        .checked_add(byte_len)
        .ok_or(Stage1EffectScanError::AddressOverflow)?;

    let mut intrinsic_sites = Vec::new();
    for (index, bytes) in bank_bytes.chunks_exact(4).enumerate() {
        let guest_pc = va_start + index as u32 * 4;
        let raw_word = u32::from_be_bytes(bytes.try_into().unwrap());
        let Some((kind, cop0_register)) = intrinsic_effect(&decode(raw_word)) else {
            continue;
        };
        intrinsic_sites.push(IntrinsicEffectSiteV1 {
            bank: bank.to_owned(),
            guest_pc,
            raw_word,
            kind,
            cop0_register,
            nondeterministic_cop0_read: kind == IntrinsicEffectKindV1::Cop0Read
                && cop0_register.is_some_and(|register| matches!(register, 1 | 9)),
            disposition: disposition(cfg.word_class.get(&guest_pc).copied()),
        });
    }

    type MemoryKey = (u32, u32, MemoryAccessKindV1, u8, i16);
    let mut observations = BTreeMap::<MemoryKey, AddressObservations>::new();
    for block in &cfg.blocks {
        if block.start_va < va_start
            || block.end_va > va_end
            || block.end_va < block.start_va
            || !(block.start_va - va_start).is_multiple_of(4)
            || !(block.end_va - va_start).is_multiple_of(4)
        {
            return Err(Stage1EffectScanError::BlockOutsideImage {
                start: block.start_va,
                end: block.end_va,
            });
        }
        let mut registers: [Option<u32>; 32] = [None; 32];
        registers[0] = Some(0);
        let mut guest_pc = block.start_va;
        while guest_pc < block.end_va {
            let offset = (guest_pc - va_start) as usize;
            let raw_word = u32::from_be_bytes(
                bank_bytes[offset..offset + 4]
                    .try_into()
                    .expect("validated block range is word aligned"),
            );
            if let Some((access, base, immediate)) = memory_access(raw_word) {
                let row = observations
                    .entry((guest_pc, raw_word, access, base, immediate))
                    .or_default();
                if let Some(base_value) = registers[base as usize] {
                    row.locally_derived.insert(classify_virtual_address(
                        base_value.wrapping_add(immediate as i32 as u32),
                    ));
                } else {
                    row.open = true;
                }
            }
            update_constants(&mut registers, raw_word);
            guest_pc += 4;
        }
    }

    let reachable_memory_sites = observations
        .into_iter()
        .map(
            |((guest_pc, raw_word, access, base_register, offset), observation)| {
                let locally_derived = observation.locally_derived.into_iter().collect::<Vec<_>>();
                let address = if !observation.open && locally_derived.len() == 1 {
                    MemoryAddressEvidenceV1::Exact {
                        address: locally_derived[0],
                    }
                } else {
                    MemoryAddressEvidenceV1::Open { locally_derived }
                };
                MemoryEffectSiteV1 {
                    bank: bank.to_owned(),
                    guest_pc,
                    raw_word,
                    access,
                    base_register,
                    offset,
                    address,
                }
            },
        )
        .collect();

    Ok(Stage1EffectReportV1 {
        bank: bank.to_owned(),
        aligned_word_count: byte_len / 4,
        intrinsic_sites,
        reachable_memory_sites,
    })
}

pub fn summarize_stage1_effects(reports: &[Stage1EffectReportV1]) -> Stage1EffectSummaryV1 {
    let mut summary = Stage1EffectSummaryV1 {
        bank_count: reports.len(),
        aligned_word_count: 0,
        reachable_intrinsic_by_kind: BTreeMap::new(),
        nondeterministic_cop0_read_count: 0,
        proven_data_intrinsic_count: 0,
        unclassified_intrinsic_count: 0,
        reachable_memory_read_count: 0,
        reachable_memory_write_count: 0,
        exact_direct_memory_by_region: BTreeMap::new(),
        exact_tlb_translated_memory_count: 0,
        open_memory_address_count: 0,
        obvious_external_effect_count: 0,
    };
    for report in reports {
        summary.aligned_word_count += u64::from(report.aligned_word_count);
        for site in &report.intrinsic_sites {
            match site.disposition {
                EffectDispositionV1::ReachableInstruction => {
                    *summary
                        .reachable_intrinsic_by_kind
                        .entry(site.kind.label().to_owned())
                        .or_default() += 1;
                    if site.nondeterministic_cop0_read {
                        summary.nondeterministic_cop0_read_count += 1;
                    }
                    summary.obvious_external_effect_count += 1;
                }
                EffectDispositionV1::ProvenData => summary.proven_data_intrinsic_count += 1,
                EffectDispositionV1::Unclassified => summary.unclassified_intrinsic_count += 1,
            }
        }
        for site in &report.reachable_memory_sites {
            match site.access {
                MemoryAccessKindV1::Read => summary.reachable_memory_read_count += 1,
                MemoryAccessKindV1::Write => summary.reachable_memory_write_count += 1,
            }
            match &site.address {
                MemoryAddressEvidenceV1::Exact {
                    address: MemoryAddressClassV1::DirectPhysical { region, .. },
                } => {
                    *summary
                        .exact_direct_memory_by_region
                        .entry(region.label().to_owned())
                        .or_default() += 1;
                    if region.is_external_effect_region() {
                        summary.obvious_external_effect_count += 1;
                    }
                }
                MemoryAddressEvidenceV1::Exact {
                    address: MemoryAddressClassV1::TlbTranslated { .. },
                } => summary.exact_tlb_translated_memory_count += 1,
                MemoryAddressEvidenceV1::Open { .. } => summary.open_memory_address_count += 1,
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::build_cfg;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    #[test]
    fn kseg1_is_not_mmio_without_an_mmio_physical_target() {
        let words = [
            0x3c08_a000, // lui t0,0xa000: KSEG1 RDRAM
            0xad00_1000, // sw zero,0x1000(t0)
            0x3c09_a460, // lui t1,0xa460: KSEG1 PI registers
            0x8d2a_0010, // lw t2,0x10(t1)
            0x03e0_0008,
            0,
        ];
        let image = bytes(&words);
        let cfg = build_cfg("bank", &image, 0x8000_0000, &[0x8000_0000]);
        let report = scan_stage1_effects("bank", &image, 0x8000_0000, &cfg).unwrap();
        assert!(matches!(
            report.reachable_memory_sites[0].address,
            MemoryAddressEvidenceV1::Exact {
                address: MemoryAddressClassV1::DirectPhysical {
                    region: DirectPhysicalRegionV1::Rdram,
                    ..
                }
            }
        ));
        assert!(matches!(
            report.reachable_memory_sites[1].address,
            MemoryAddressEvidenceV1::Exact {
                address: MemoryAddressClassV1::DirectPhysical {
                    region: DirectPhysicalRegionV1::Rcp,
                    physical_address: 0x0460_0010,
                    ..
                }
            }
        ));
    }

    #[test]
    fn count_random_and_cache_are_only_impure_when_code() {
        let words = [
            0x4008_4800, // mfc0 t0,Count
            0x4009_0800, // mfc0 t1,Random
            0xbc00_0000, // cache 0,0(zero)
            0x03e0_0008,
            0,
            0x400a_4800, // Count-shaped data outside reached CFG
        ];
        let image = bytes(&words);
        let mut cfg = build_cfg("bank", &image, 0x8000_0000, &[0x8000_0000]);
        cfg.word_class.insert(0x8000_0014, WordClass::ProvenData);
        let report = scan_stage1_effects("bank", &image, 0x8000_0000, &cfg).unwrap();
        let summary = summarize_stage1_effects(&[report]);
        assert_eq!(summary.reachable_intrinsic_by_kind["cop0_read"], 2);
        assert_eq!(summary.reachable_intrinsic_by_kind["cache"], 1);
        assert_eq!(summary.nondeterministic_cop0_read_count, 2);
        assert_eq!(summary.proven_data_intrinsic_count, 1);
        assert_eq!(summary.obvious_external_effect_count, 3);
    }

    #[test]
    fn cross_block_address_construction_stays_open() {
        let words = [0x3c08_a460, 0x1000_0001, 0, 0x8d09_0010, 0x03e0_0008, 0];
        let image = bytes(&words);
        let cfg = build_cfg("bank", &image, 0x8000_0000, &[0x8000_0000, 0x8000_000c]);
        let report = scan_stage1_effects("bank", &image, 0x8000_0000, &cfg).unwrap();
        let load = report
            .reachable_memory_sites
            .iter()
            .find(|site| site.guest_pc == 0x8000_000c)
            .unwrap();
        assert!(matches!(load.address, MemoryAddressEvidenceV1::Open { .. }));
    }
}
