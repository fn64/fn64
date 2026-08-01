//! Fail-closed diagnostic geometry for a boot-installed TLB execution view.
//!
//! [`crate::resolve::analyze_constant_tlb_transfers`] proves the correlated
//! raw COP0 state. This module delegates address translation to the execution
//! runtime and intersects the result with an independently proven physical
//! backing interval. It does not mint a ROM mapping or mutate the fact log.

use crate::resolve::{ConstantTlbTransferAnalysisV1, TlbTransferBlockerV1, TlbWriteProofV1};
use fn64_recomp_rs::runtime::{InstructionTranslationDiagnosticErrorV1, RecompContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_DIAGNOSTIC_BACKING_BYTES: u32 = 8 * 1024 * 1024;
const SUPPORTED_PAGE_MASKS: [u32; 7] = [
    0,
    0x0000_6000,
    0x0001_e000,
    0x0007_e000,
    0x001f_e000,
    0x007f_e000,
    0x01ff_e000,
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BootTlbAliasBlockerV1 {
    TransferProof(TlbTransferBlockerV1),
    MissingTarget,
    BackingUnaligned,
    BackingEmpty,
    BackingTooLarge {
        bytes: u32,
        limit: u32,
    },
    BackingRangeOverflow,
    InitialTlbStateUnproven {
        known_entries: u8,
    },
    UnsupportedPageMask {
        tlbwi_pc: u32,
        page_mask_raw: u32,
    },
    TranslationUnsupportedPageMask {
        index: usize,
        page_mask_raw: u32,
    },
    CompetingTlbMatches {
        vaddr: u32,
        first_index: usize,
        second_index: usize,
    },
    TranslationFault,
    TargetOutsideBacking {
        target_physical: u32,
    },
    AliasRangeOverflow,
}

/// One content-free address view backed byte-for-byte by an already admitted
/// physical interval. This remains diagnostic authority only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootTlbAliasSpanV1 {
    pub transfer_pc: u32,
    pub target_va: u32,
    pub target_physical: u32,
    pub physical_start: u32,
    pub physical_end: u32,
    pub alias_va_start: u32,
    pub alias_va_end: u32,
    pub active_writes: Vec<TlbWriteProofV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootTlbAliasTransferDiagnosticV1 {
    pub transfer_pc: u32,
    pub target_va: Option<u32>,
    pub active_writes: Vec<TlbWriteProofV1>,
    /// Geometry under the retained blockers. This is useful for deciding
    /// whether closing the blocker has leverage; it is never admission.
    pub conditional_alias: Option<BootTlbAliasSpanV1>,
    pub blockers: Vec<BootTlbAliasBlockerV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootTlbAliasDiagnosticV1 {
    pub transfers: Vec<BootTlbAliasTransferDiagnosticV1>,
}

fn runtime_with_writes(
    writes: &[TlbWriteProofV1],
    entry_hi_at_transfer: u64,
) -> Option<RecompContext> {
    if writes.iter().any(|write| {
        !SUPPORTED_PAGE_MASKS.contains(&(write.page_mask_raw & 0x01ff_e000))
            || write.page_mask_raw & !0x01ff_e000 != 0
    }) {
        return None;
    }
    let mut runtime = RecompContext::new();
    runtime.initialize_invalid_tlb_entries();
    for write in writes {
        runtime.write_cop0(0, write.index_raw);
        runtime.write_cop0(2, write.entry_lo0_raw);
        runtime.write_cop0(3, write.entry_lo1_raw);
        runtime.write_cop0(5, write.page_mask_raw);
        runtime.write_cop0(10, write.entry_hi_raw);
        runtime.tlbwi_record();
    }
    runtime.write_cop0_64(10, entry_hi_at_transfer);
    Some(runtime)
}

fn translated(runtime: &RecompContext, va: u32) -> Result<Option<u32>, BootTlbAliasBlockerV1> {
    match runtime.translate_instruction_address_diagnostic_v1(va) {
        Ok(address) => Ok(Some(address.get())),
        Err(InstructionTranslationDiagnosticErrorV1::Access(_)) => Ok(None),
        Err(InstructionTranslationDiagnosticErrorV1::InvalidPageMaskEncoding {
            index,
            page_mask_raw,
        }) => Err(BootTlbAliasBlockerV1::TranslationUnsupportedPageMask {
            index,
            page_mask_raw,
        }),
        Err(InstructionTranslationDiagnosticErrorV1::MultipleTlbMatches {
            vaddr,
            first_index,
            second_index,
        }) => Err(BootTlbAliasBlockerV1::CompetingTlbMatches {
            vaddr: vaddr as u32,
            first_index,
            second_index,
        }),
    }
}

/// Intersect path-invariant boot TLB state with one proven physical backing
/// interval. The returned interval is the maximal word-contiguous component
/// containing the transfer target and preserving exact VA-to-physical
/// identity under the runtime translator.
pub fn derive_boot_tlb_alias_diagnostic(
    analysis: &ConstantTlbTransferAnalysisV1,
    backing_physical_start: u32,
    backing_bytes: u32,
) -> BootTlbAliasDiagnosticV1 {
    let shared_geometry_blocker = if backing_bytes == 0 {
        Some(BootTlbAliasBlockerV1::BackingEmpty)
    } else if !backing_physical_start.is_multiple_of(4) || !backing_bytes.is_multiple_of(4) {
        Some(BootTlbAliasBlockerV1::BackingUnaligned)
    } else if backing_bytes > MAX_DIAGNOSTIC_BACKING_BYTES {
        Some(BootTlbAliasBlockerV1::BackingTooLarge {
            bytes: backing_bytes,
            limit: MAX_DIAGNOSTIC_BACKING_BYTES,
        })
    } else if backing_physical_start.checked_add(backing_bytes).is_none() {
        Some(BootTlbAliasBlockerV1::BackingRangeOverflow)
    } else {
        None
    };

    let transfers = analysis
        .transfers
        .iter()
        .map(|transfer| {
            let mut blockers = transfer
                .blockers
                .iter()
                .cloned()
                .map(BootTlbAliasBlockerV1::TransferProof)
                .collect::<Vec<_>>();
            if let Some(blocker) = shared_geometry_blocker.clone() {
                blockers.push(blocker);
            }
            let Some(target_va) = transfer.target else {
                blockers.push(BootTlbAliasBlockerV1::MissingTarget);
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: None,
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            };
            for write in &transfer.active_writes {
                if !SUPPORTED_PAGE_MASKS.contains(&(write.page_mask_raw & 0x01ff_e000))
                    || write.page_mask_raw & !0x01ff_e000 != 0
                {
                    blockers.push(BootTlbAliasBlockerV1::UnsupportedPageMask {
                        tlbwi_pc: write.tlbwi_pc,
                        page_mask_raw: write.page_mask_raw,
                    });
                }
            }
            let known_entries = transfer
                .active_writes
                .iter()
                .map(|write| write.index_raw & 31)
                .collect::<BTreeSet<_>>()
                .len();
            if known_entries < 32 {
                blockers.push(BootTlbAliasBlockerV1::InitialTlbStateUnproven {
                    known_entries: u8::try_from(known_entries).unwrap_or(u8::MAX),
                });
            }
            let has_geometry_blocker = blockers.iter().any(|blocker| {
                matches!(
                    blocker,
                    BootTlbAliasBlockerV1::BackingUnaligned
                        | BootTlbAliasBlockerV1::BackingEmpty
                        | BootTlbAliasBlockerV1::BackingTooLarge { .. }
                        | BootTlbAliasBlockerV1::BackingRangeOverflow
                        | BootTlbAliasBlockerV1::UnsupportedPageMask { .. }
                )
            });
            if has_geometry_blocker || transfer.active_writes.is_empty() {
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            }
            let Some(entry_hi_at_transfer) = transfer.entry_hi_at_transfer else {
                blockers.push(BootTlbAliasBlockerV1::TransferProof(
                    TlbTransferBlockerV1::TlbSetupOpen { cop0d: 10 },
                ));
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            };
            let Some(runtime) = runtime_with_writes(&transfer.active_writes, entry_hi_at_transfer)
            else {
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            };
            let target_physical = match translated(&runtime, target_va) {
                Ok(Some(target_physical)) => target_physical,
                Ok(None) => {
                    blockers.push(BootTlbAliasBlockerV1::TranslationFault);
                    blockers.sort();
                    blockers.dedup();
                    return BootTlbAliasTransferDiagnosticV1 {
                        transfer_pc: transfer.transfer_pc,
                        target_va: Some(target_va),
                        active_writes: transfer.active_writes.clone(),
                        conditional_alias: None,
                        blockers,
                    };
                }
                Err(blocker) => {
                    blockers.push(blocker);
                    blockers.sort();
                    blockers.dedup();
                    return BootTlbAliasTransferDiagnosticV1 {
                        transfer_pc: transfer.transfer_pc,
                        target_va: Some(target_va),
                        active_writes: transfer.active_writes.clone(),
                        conditional_alias: None,
                        blockers,
                    };
                }
            };
            let backing_end = backing_physical_start + backing_bytes;
            if target_physical < backing_physical_start || target_physical >= backing_end {
                blockers.push(BootTlbAliasBlockerV1::TargetOutsideBacking { target_physical });
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            }
            let delta = i64::from(target_va) - i64::from(target_physical);
            let alias_va = |physical: u32| {
                u32::try_from(i64::from(physical) + delta)
                    .map_err(|_| BootTlbAliasBlockerV1::AliasRangeOverflow)
            };
            let matches = |physical: u32| -> Result<bool, BootTlbAliasBlockerV1> {
                let va = alias_va(physical)?;
                Ok(translated(&runtime, va)? == Some(physical))
            };
            let mut physical_start = target_physical & !3;
            let mut translation_blocker = None;
            while physical_start > backing_physical_start {
                let previous = physical_start - 4;
                match matches(previous) {
                    Ok(true) => physical_start = previous,
                    Ok(false) => break,
                    Err(blocker) => {
                        translation_blocker = Some(blocker);
                        break;
                    }
                }
            }
            let mut physical_end = (target_physical & !3) + 4;
            while translation_blocker.is_none() && physical_end < backing_end {
                match matches(physical_end) {
                    Ok(true) => physical_end += 4,
                    Ok(false) => break,
                    Err(blocker) => {
                        translation_blocker = Some(blocker);
                    }
                }
            }
            if let Some(blocker) = translation_blocker {
                blockers.push(blocker);
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            }
            let (Ok(alias_va_start), Ok(alias_va_end)) =
                (alias_va(physical_start), alias_va(physical_end))
            else {
                blockers.push(BootTlbAliasBlockerV1::AliasRangeOverflow);
                blockers.sort();
                blockers.dedup();
                return BootTlbAliasTransferDiagnosticV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va: Some(target_va),
                    active_writes: transfer.active_writes.clone(),
                    conditional_alias: None,
                    blockers,
                };
            };
            blockers.sort();
            blockers.dedup();
            BootTlbAliasTransferDiagnosticV1 {
                transfer_pc: transfer.transfer_pc,
                target_va: Some(target_va),
                active_writes: transfer.active_writes.clone(),
                conditional_alias: Some(BootTlbAliasSpanV1 {
                    transfer_pc: transfer.transfer_pc,
                    target_va,
                    target_physical,
                    physical_start,
                    physical_end,
                    alias_va_start,
                    alias_va_end,
                    active_writes: transfer.active_writes.clone(),
                }),
                blockers,
            }
        })
        .collect();
    BootTlbAliasDiagnosticV1 { transfers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::build_cfg;
    use crate::resolve::{ConstantTlbTransferAnalysisV1, TlbTransferProofV1};

    fn ge_write() -> TlbWriteProofV1 {
        TlbWriteProofV1 {
            tlbwi_pc: 0x8000_04a8,
            index_raw: 1,
            page_mask_raw: 0x007f_e000,
            entry_hi_raw: 0x7000_0000,
            entry_lo0_raw: 0x1f,
            entry_lo1_raw: 0x1,
        }
    }

    fn transfer(active_writes: Vec<TlbWriteProofV1>) -> ConstantTlbTransferAnalysisV1 {
        ConstantTlbTransferAnalysisV1 {
            transfers: vec![TlbTransferProofV1 {
                transfer_pc: 0x8000_04fc,
                target: Some(0x7000_0510),
                entry_hi_at_transfer: Some(0x7000_0000),
                active_writes,
                blockers: Vec::new(),
            }],
        }
    }

    fn mtc0(rt: u8, cop0d: u8) -> u32 {
        (0x10 << 26) | (4 << 21) | ((rt as u32) << 16) | ((cop0d as u32) << 11)
    }

    fn instruction_bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    #[test]
    fn runtime_translation_intersects_backing_but_retains_initial_state_frontier() {
        let analysis = transfer(vec![ge_write()]);
        let report = derive_boot_tlb_alias_diagnostic(&analysis, 0x400, 0x10_0000);
        let alias = report.transfers[0]
            .conditional_alias
            .as_ref()
            .expect("exact alias");
        assert_eq!(alias.target_physical, 0x510);
        assert_eq!(alias.physical_start, 0x400);
        assert_eq!(alias.physical_end, 0x10_0400);
        assert_eq!(alias.alias_va_start, 0x7000_0400);
        assert_eq!(alias.alias_va_end, 0x7010_0400);
        assert_eq!(
            report.transfers[0].blockers,
            vec![BootTlbAliasBlockerV1::InitialTlbStateUnproven { known_entries: 1 }]
        );
    }

    #[test]
    fn analyzer_to_diagnostic_retains_unproven_initial_entries() {
        const START: u32 = 0x8000_0400;
        let words = [
            0x2408_0001, // addiu t0,zero,1: Index
            0x2409_001f, // addiu t1,zero,0x1f: EntryLo0
            0x240a_0001, // addiu t2,zero,1: EntryLo1
            0x3c0b_007f, // lui t3,0x007f
            0x356b_e000, // ori t3,t3,0xe000: PageMask
            0x3c0c_7000, // lui t4,0x7000: EntryHi
            mtc0(8, 0),
            mtc0(9, 2),
            mtc0(10, 3),
            mtc0(11, 5),
            mtc0(12, 10),
            0x4200_0002, // tlbwi
            0x3c0d_7000, // lui t5,0x7000
            0x35ad_0510, // ori t5,t5,0x0510
            0x01a0_0008, // jr t5
            0,
        ];
        let image = instruction_bytes(&words);
        let cfg = build_cfg("boot-tlb-alias-e2e", &image, START, &[START]);
        let analysis = crate::resolve::analyze_constant_tlb_transfers(&cfg, &image, START);
        let report = derive_boot_tlb_alias_diagnostic(&analysis, 0x400, 0x10_0000);

        let [transfer] = report.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert!(transfer.conditional_alias.is_some());
        assert_eq!(
            transfer.blockers,
            vec![BootTlbAliasBlockerV1::InitialTlbStateUnproven { known_entries: 1 }]
        );
    }

    #[test]
    fn alias_interval_cannot_wrap_the_32_bit_address_space() {
        let report = derive_boot_tlb_alias_diagnostic(
            &ConstantTlbTransferAnalysisV1 {
                transfers: vec![TlbTransferProofV1 {
                    transfer_pc: 0x8000_0100,
                    target: Some(0xffff_f010),
                    entry_hi_at_transfer: Some(0xffff_e000),
                    active_writes: vec![TlbWriteProofV1 {
                        tlbwi_pc: 0x8000_00f0,
                        index_raw: 1,
                        page_mask_raw: 0,
                        entry_hi_raw: 0xffff_e000,
                        entry_lo0_raw: 0b111,
                        entry_lo1_raw: 0b111,
                    }],
                    blockers: Vec::new(),
                }],
            },
            0,
            0x2000,
        );

        let [transfer] = report.transfers.as_slice() else {
            panic!("expected one computed transfer")
        };
        assert!(transfer.conditional_alias.is_none());
        assert!(transfer
            .blockers
            .contains(&BootTlbAliasBlockerV1::AliasRangeOverflow));
        assert!(transfer
            .blockers
            .contains(&BootTlbAliasBlockerV1::InitialTlbStateUnproven { known_entries: 1 }));
    }

    #[test]
    fn unbacked_target_and_transfer_frontier_remain_open() {
        let analysis = ConstantTlbTransferAnalysisV1 {
            transfers: vec![TlbTransferProofV1 {
                transfer_pc: 0x8000_04fc,
                target: Some(0x7040_0100),
                entry_hi_at_transfer: Some(0x7000_0000),
                active_writes: vec![ge_write()],
                blockers: Vec::new(),
            }],
        };
        let report = derive_boot_tlb_alias_diagnostic(&analysis, 0x400, 0x10_0000);
        assert!(report.transfers[0].conditional_alias.is_none());
        assert!(report.transfers[0].blockers.iter().any(|blocker| matches!(
            blocker,
            BootTlbAliasBlockerV1::TranslationFault
                | BootTlbAliasBlockerV1::TargetOutsideBacking { .. }
        )));
        assert!(report.transfers[0]
            .blockers
            .contains(&BootTlbAliasBlockerV1::InitialTlbStateUnproven { known_entries: 1 }));

        let blocked = ConstantTlbTransferAnalysisV1 {
            transfers: vec![TlbTransferProofV1 {
                transfer_pc: 1,
                target: Some(0x7000_0510),
                entry_hi_at_transfer: Some(0x7000_0000),
                active_writes: vec![ge_write()],
                blockers: vec![TlbTransferBlockerV1::TlbPathDisagreement],
            }],
        };
        let report = derive_boot_tlb_alias_diagnostic(&blocked, 0x400, 0x10_0000);
        assert!(report.transfers[0].conditional_alias.is_some());
        assert!(report.transfers[0]
            .blockers
            .contains(&BootTlbAliasBlockerV1::TransferProof(
                TlbTransferBlockerV1::TlbPathDisagreement
            )));
        assert!(report.transfers[0]
            .blockers
            .contains(&BootTlbAliasBlockerV1::InitialTlbStateUnproven { known_entries: 1 }));
    }
}
