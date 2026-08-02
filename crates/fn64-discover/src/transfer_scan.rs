//! Bounded, typed transfer inventory over already-built CFG closures.
//!
//! This scanner classifies transfers exposed by each supplied [`ClosureResult`].
//! `ProvenFactRoots` therefore leaves the inventory open unless an opaque,
//! analyzer-owned catalog-total authority proves that every aligned catalog PC
//! is admitted and every dynamic transfer is guarded. The scanner consumes no
//! symbols or generated disassembly. Bounded/open indirect evidence is retained
//! even when that authority makes the runtime transfer non-blocking.

use crate::cfg::BlockTerminator;
use crate::resolve::{ClosureResult, IndirectProofState, IndirectResolutionKind};
use crate::source_closure::{
    IndirectDispositionV1, IndirectTransferFrontierV1, TransferInventoryV1, TransferSummaryV1,
    MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1,
};
use fn64_recomp_rs::{
    static_execution_build_receipt, CatalogResolverPolicyEvidenceV1,
    CATALOG_RESOLVER_POLICY_NAME_V1,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug)]
pub struct TransferScanBankInput<'a> {
    pub bank: &'a str,
    pub bank_id: u64,
    pub va_start: u32,
    pub va_end: u32,
    pub closure: &'a ClosureResult,
    pub root_coverage: TransferRootCoverageV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransferRootCoverageV1 {
    ProvenFactRoots,
    /// Caller claim retained for diagnostics only. V1 has no analyzer-owned
    /// callable-entry denominator, so this can never close the inventory.
    CallerAssertedExhaustiveCallableEntries,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOwnerKindV1 {
    DenseGeneration,
    ExternalExecutableImage,
}

#[derive(Clone, Copy, Debug)]
pub struct TransferOwnerInput<'a> {
    pub bank: &'a str,
    pub bank_id: u64,
    pub va_start: u32,
    pub va_end: u32,
    pub kind: TransferOwnerKindV1,
}

#[derive(Clone, Copy, Debug)]
pub struct HostTransferTargetInput<'a> {
    pub bank: &'a str,
    pub guest_pc: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferScanCoverageV1 {
    BoundedReachableCfg,
    Exhaustive,
    CatalogTotal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogGuardedTransferKindV1 {
    Return,
    TrapException,
    IndirectJump,
    IndirectCall,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogGuardedTransferV1 {
    pub bank: String,
    pub bank_id: u64,
    pub site_pc: u32,
    pub kind: CatalogGuardedTransferKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTotalOwnerEvidenceV1 {
    pub bank: String,
    pub bank_id: u64,
    pub va_start: u32,
    pub va_end: u32,
    pub kind: TransferOwnerKindV1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTotalHostTargetEvidenceV1 {
    pub bank: String,
    pub guest_pc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTotalTransferAuthorityEvidenceV1 {
    pub policy: String,
    pub owners: Vec<CatalogTotalOwnerEvidenceV1>,
    pub host_targets: Vec<CatalogTotalHostTargetEvidenceV1>,
    pub exception_vectors: Vec<u32>,
}

/// Opaque proof that the supplied owner catalog is aligned and scan-total and
/// that generated dynamic transfers are guarded by the implementation-issued
/// canonical resolver policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogTotalTransferAuthorityV1 {
    evidence: CatalogTotalTransferAuthorityEvidenceV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectTransferKindV1 {
    Jump,
    Call,
    CallContinuation,
    BranchTaken,
    BranchFallthrough,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectTransferDispositionV1 {
    GuestOwner { bank: String, bank_id: u64 },
    InstalledHost { bank: String, guest_pc: u32 },
    AmbiguousOwners,
    OutsideCatalog,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectTransferV1 {
    pub source_bank: String,
    pub source_bank_id: u64,
    pub site_pc: u32,
    pub kind: DirectTransferKindV1,
    pub target: u32,
    pub disposition: DirectTransferDispositionV1,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "blocker", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransferBlockerV1 {
    RootDenominatorOpen {
        bank: String,
        bank_id: u64,
    },
    CallableEntryDenominatorUnverified {
        bank: String,
        bank_id: u64,
    },
    RequiredOwnerScanMissing {
        bank: String,
        bank_id: u64,
        kind: TransferOwnerKindV1,
    },
    DirectTargetOutsideCatalog {
        bank: String,
        site_pc: u32,
        target: u32,
    },
    DirectTargetAmbiguous {
        bank: String,
        site_pc: u32,
        target: u32,
    },
    IndirectBounded {
        bank: String,
        site_pc: u32,
    },
    IndirectOpen {
        bank: String,
        site_pc: u32,
    },
    IndirectTargetOutsideCatalog {
        bank: String,
        site_pc: u32,
        target: u32,
    },
    IndirectTargetAmbiguous {
        bank: String,
        site_pc: u32,
        target: u32,
    },
    ReturnAddressFlowOpen {
        bank: String,
        site_pc: u32,
    },
    RanOffEnd {
        bank: String,
        site_pc: u32,
    },
    MalformedDelaySlot {
        bank: String,
        site_pc: u32,
    },
    InvalidInstruction {
        bank: String,
        site_pc: u32,
        word: u32,
    },
    TrapExceptionFlowOpen {
        bank: String,
        site_pc: u32,
    },
    DataFenceReached {
        bank: String,
        site_pc: u32,
    },
    SelfReferentialBranchReached {
        bank: String,
        site_pc: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferScanV1 {
    coverage: TransferScanCoverageV1,
    summary: TransferSummaryV1,
    inventory: TransferInventoryV1,
    direct: Vec<DirectTransferV1>,
    indirect_frontier: Vec<IndirectTransferFrontierV1>,
    catalog_guarded: Vec<CatalogGuardedTransferV1>,
    catalog_total_authority: Option<CatalogTotalTransferAuthorityEvidenceV1>,
    blockers: Vec<TransferBlockerV1>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferScanEvidenceV1 {
    pub coverage: TransferScanCoverageV1,
    pub summary: TransferSummaryV1,
    pub inventory: TransferInventoryV1,
    pub direct: Vec<DirectTransferV1>,
    pub indirect_frontier: Vec<IndirectTransferFrontierV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog_guarded: Vec<CatalogGuardedTransferV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_total_authority: Option<CatalogTotalTransferAuthorityEvidenceV1>,
    pub blockers: Vec<TransferBlockerV1>,
}

impl TransferScanV1 {
    pub const fn coverage(&self) -> TransferScanCoverageV1 {
        self.coverage
    }

    pub const fn summary(&self) -> &TransferSummaryV1 {
        &self.summary
    }

    pub const fn inventory(&self) -> TransferInventoryV1 {
        self.inventory
    }

    pub fn direct(&self) -> &[DirectTransferV1] {
        &self.direct
    }

    pub fn indirect_frontier(&self) -> &[IndirectTransferFrontierV1] {
        &self.indirect_frontier
    }

    pub fn catalog_guarded(&self) -> &[CatalogGuardedTransferV1] {
        &self.catalog_guarded
    }

    pub fn blockers(&self) -> &[TransferBlockerV1] {
        &self.blockers
    }

    pub(crate) fn into_evidence(self) -> TransferScanEvidenceV1 {
        TransferScanEvidenceV1 {
            coverage: self.coverage,
            summary: self.summary,
            inventory: self.inventory,
            direct: self.direct,
            indirect_frontier: self.indirect_frontier,
            catalog_guarded: self.catalog_guarded,
            catalog_total_authority: self.catalog_total_authority,
            blockers: self.blockers,
        }
    }

    #[cfg(test)]
    pub(crate) fn synthetic_complete_for_validator_test(
        indirect_frontier: Vec<IndirectTransferFrontierV1>,
    ) -> Self {
        Self {
            coverage: TransferScanCoverageV1::Exhaustive,
            summary: TransferSummaryV1 {
                direct_total: 0,
                direct_guest: 0,
                direct_host: 0,
                direct_open: 0,
                indirect_closed: indirect_frontier.len() as u64,
                indirect_bounded: 0,
                indirect_open: 0,
            },
            inventory: TransferInventoryV1::Complete,
            direct: Vec::new(),
            indirect_frontier,
            catalog_guarded: Vec::new(),
            catalog_total_authority: None,
            blockers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferScanError {
    EmptyScan,
    InvalidBankRange { bank: String },
    InvalidOwnerRange { bank: String },
    InvalidHostTarget { bank: String, guest_pc: u32 },
    ClosureBankMismatch { expected: String, actual: String },
    ScanBankMissingOwner { bank: String, bank_id: u64 },
    DuplicateScanBank { bank: String, bank_id: u64 },
    DuplicateOwner { bank: String, bank_id: u64 },
    DuplicateHostTarget { guest_pc: u32 },
    DuplicateIndirectSite { bank: String, site_pc: u32 },
    InvalidBlockGeometry { bank: String, start_pc: u32 },
    OverlappingBlocks { bank: String },
    DirectDenominatorMismatch { bank: String },
    IndirectDenominatorMismatch { bank: String },
    IndirectEvidenceMismatch { bank: String, site_pc: u32 },
    DuplicateDirectTransfer { bank: String, site_pc: u32 },
    CatalogAuthorityOwnerCoverageMismatch,
    CatalogAuthorityPolicyIncomplete,
    CatalogAuthorityInputMismatch,
}

/// Classify every direct and indirect transfer exposed by the supplied
/// closures. Results and blockers are canonicalized independently of input
/// order. Without a catalog-total authority, a return remains open because CFG
/// reachability does not prove `$ra` provenance, the thread-return sentinel,
/// or thread-zero's IPL3 return.
pub fn scan_transfers_v1(
    banks: &[TransferScanBankInput<'_>],
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
) -> Result<TransferScanV1, TransferScanError> {
    scan_transfers_inner_v1(banks, owners, host_targets, None)
}

/// Validate owner/scan catalog totality against sealed resolver-policy
/// evidence. The returned authority is bound to the exact canonical owner and
/// host-target inputs and cannot be reused with a different catalog.
pub fn validate_catalog_total_transfer_authority_v1(
    banks: &[TransferScanBankInput<'_>],
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
    policy: &CatalogResolverPolicyEvidenceV1,
) -> Result<CatalogTotalTransferAuthorityV1, TransferScanError> {
    validate_inputs(banks, owners, host_targets)?;

    let owner_coverage_is_exact = owners.len() == banks.len()
        && owners.iter().all(|owner| {
            banks.iter().any(|bank| {
                bank.bank == owner.bank
                    && bank.bank_id == owner.bank_id
                    && bank.va_start == owner.va_start
                    && bank.va_end == owner.va_end
            })
        });
    if !owner_coverage_is_exact {
        return Err(TransferScanError::CatalogAuthorityOwnerCoverageMismatch);
    }
    let build_receipt = policy.build_receipt();
    if policy.policy() != CATALOG_RESOLVER_POLICY_NAME_V1
        || policy.exception_vectors() != &MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        || !policy.aligned_pc_admission()
        || !policy.exact_active_owner_resolution()
        || !policy.explicit_thread_return_boundary()
        || !policy.misaligned_target_fault()
        || !policy.unmapped_or_ambiguous_target_fault()
        || !policy.traps_enter_shared_resolver()
        || build_receipt != static_execution_build_receipt()
        || build_receipt.schema != 1
        || !build_receipt.aot_runtime
        || build_receipt.dev_interpreter
    {
        return Err(TransferScanError::CatalogAuthorityPolicyIncomplete);
    }

    let mut owner_evidence = owners
        .iter()
        .map(|owner| CatalogTotalOwnerEvidenceV1 {
            bank: owner.bank.to_string(),
            bank_id: owner.bank_id,
            va_start: owner.va_start,
            va_end: owner.va_end,
            kind: owner.kind,
        })
        .collect::<Vec<_>>();
    owner_evidence.sort_unstable();
    let mut canonical_host_targets = host_targets
        .iter()
        .map(|host| CatalogTotalHostTargetEvidenceV1 {
            bank: host.bank.to_string(),
            guest_pc: host.guest_pc,
        })
        .collect::<Vec<_>>();
    canonical_host_targets.sort_unstable();
    Ok(CatalogTotalTransferAuthorityV1 {
        evidence: CatalogTotalTransferAuthorityEvidenceV1 {
            policy: policy.policy().to_string(),
            owners: owner_evidence,
            host_targets: canonical_host_targets,
            exception_vectors: policy.exception_vectors().to_vec(),
        },
    })
}

/// Scan using a catalog-total authority previously validated for these exact
/// owner and host-target inputs.
pub fn scan_transfers_with_catalog_total_authority_v1(
    banks: &[TransferScanBankInput<'_>],
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
    authority: &CatalogTotalTransferAuthorityV1,
) -> Result<TransferScanV1, TransferScanError> {
    scan_transfers_inner_v1(banks, owners, host_targets, Some(authority))
}

fn scan_transfers_inner_v1(
    banks: &[TransferScanBankInput<'_>],
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
    authority: Option<&CatalogTotalTransferAuthorityV1>,
) -> Result<TransferScanV1, TransferScanError> {
    validate_inputs(banks, owners, host_targets)?;
    if let Some(authority) = authority {
        let mut expected_owners = owners
            .iter()
            .map(|owner| CatalogTotalOwnerEvidenceV1 {
                bank: owner.bank.to_string(),
                bank_id: owner.bank_id,
                va_start: owner.va_start,
                va_end: owner.va_end,
                kind: owner.kind,
            })
            .collect::<Vec<_>>();
        expected_owners.sort_unstable();
        let mut expected_hosts = host_targets
            .iter()
            .map(|host| CatalogTotalHostTargetEvidenceV1 {
                bank: host.bank.to_string(),
                guest_pc: host.guest_pc,
            })
            .collect::<Vec<_>>();
        expected_hosts.sort_unstable();
        if authority.evidence.owners != expected_owners
            || authority.evidence.host_targets != expected_hosts
            || authority.evidence.exception_vectors
                != MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1.as_slice()
            || authority.evidence.policy != "fn64_dense_aot_catalog_resolver_v1"
        {
            return Err(TransferScanError::CatalogAuthorityInputMismatch);
        }
    }

    let mut direct = Vec::new();
    let mut indirect_frontier = Vec::new();
    let mut catalog_guarded = Vec::new();
    let mut blockers = Vec::new();

    for bank in banks {
        if authority.is_none() {
            blockers.push(match bank.root_coverage {
                TransferRootCoverageV1::ProvenFactRoots => TransferBlockerV1::RootDenominatorOpen {
                    bank: bank.bank.to_string(),
                    bank_id: bank.bank_id,
                },
                TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries => {
                    TransferBlockerV1::CallableEntryDenominatorUnverified {
                        bank: bank.bank.to_string(),
                        bank_id: bank.bank_id,
                    }
                }
            });
        }

        for block in &bank.closure.cfg.blocks {
            match &block.terminator {
                BlockTerminator::Tail { target } => push_direct(
                    &mut direct,
                    &mut blockers,
                    bank,
                    owners,
                    host_targets,
                    delay_control_site_pc(block.start_va, block.end_va)
                        .expect("delay-slot block geometry was validated"),
                    DirectTransferKindV1::Jump,
                    *target,
                ),
                BlockTerminator::Call { target, next } => {
                    let site_pc = delay_control_site_pc(block.start_va, block.end_va)
                        .expect("delay-slot block geometry was validated");
                    push_direct(
                        &mut direct,
                        &mut blockers,
                        bank,
                        owners,
                        host_targets,
                        site_pc,
                        DirectTransferKindV1::Call,
                        *target,
                    );
                    push_direct(
                        &mut direct,
                        &mut blockers,
                        bank,
                        owners,
                        host_targets,
                        site_pc,
                        DirectTransferKindV1::CallContinuation,
                        *next,
                    );
                }
                BlockTerminator::Branch {
                    target,
                    fallthrough,
                    link,
                }
                | BlockTerminator::BranchLikely {
                    target,
                    fallthrough,
                    link,
                } => {
                    let site_pc = delay_control_site_pc(block.start_va, block.end_va)
                        .expect("delay-slot block geometry was validated");
                    let via_call = *link;
                    push_direct(
                        &mut direct,
                        &mut blockers,
                        bank,
                        owners,
                        host_targets,
                        site_pc,
                        if via_call {
                            DirectTransferKindV1::Call
                        } else {
                            DirectTransferKindV1::BranchTaken
                        },
                        *target,
                    );
                    push_direct(
                        &mut direct,
                        &mut blockers,
                        bank,
                        owners,
                        host_targets,
                        site_pc,
                        if via_call {
                            DirectTransferKindV1::CallContinuation
                        } else {
                            DirectTransferKindV1::BranchFallthrough
                        },
                        *fallthrough,
                    );
                }
                BlockTerminator::Return => {
                    let site_pc = delay_control_site_pc(block.start_va, block.end_va)
                        .expect("delay-slot block geometry was validated");
                    if authority.is_some() {
                        catalog_guarded.push(CatalogGuardedTransferV1 {
                            bank: bank.bank.to_string(),
                            bank_id: bank.bank_id,
                            site_pc,
                            kind: CatalogGuardedTransferKindV1::Return,
                        });
                    } else {
                        blockers.push(TransferBlockerV1::ReturnAddressFlowOpen {
                            bank: bank.bank.to_string(),
                            site_pc,
                        });
                    }
                }
                BlockTerminator::Fallthrough { next } => push_direct(
                    &mut direct,
                    &mut blockers,
                    bank,
                    owners,
                    host_targets,
                    ordinary_terminal_site_pc(block.start_va, block.end_va)
                        .expect("block geometry was validated"),
                    DirectTransferKindV1::BranchFallthrough,
                    *next,
                ),
                BlockTerminator::RanOffEnd => {
                    blockers.push(TransferBlockerV1::RanOffEnd {
                        bank: bank.bank.to_string(),
                        site_pc: ordinary_terminal_site_pc(block.start_va, block.end_va)
                            .expect("block geometry was validated"),
                    });
                }
                BlockTerminator::MissingDelaySlot { control_pc } => {
                    blockers.push(TransferBlockerV1::MalformedDelaySlot {
                        bank: bank.bank.to_string(),
                        site_pc: *control_pc,
                    });
                }
                BlockTerminator::ResolvedIndirect { via_call, .. }
                | BlockTerminator::Indirect { via_call } => {
                    if *via_call {
                        push_direct(
                            &mut direct,
                            &mut blockers,
                            bank,
                            owners,
                            host_targets,
                            delay_control_site_pc(block.start_va, block.end_va)
                                .expect("delay-slot block geometry was validated"),
                            DirectTransferKindV1::CallContinuation,
                            block.end_va,
                        );
                    }
                }
                BlockTerminator::Trap => {
                    let site_pc = ordinary_terminal_site_pc(block.start_va, block.end_va)
                        .expect("block geometry was validated");
                    if authority.is_some() {
                        catalog_guarded.push(CatalogGuardedTransferV1 {
                            bank: bank.bank.to_string(),
                            bank_id: bank.bank_id,
                            site_pc,
                            kind: CatalogGuardedTransferKindV1::TrapException,
                        });
                    } else {
                        blockers.push(TransferBlockerV1::TrapExceptionFlowOpen {
                            bank: bank.bank.to_string(),
                            site_pc,
                        });
                    }
                }
                BlockTerminator::InvalidInstruction { pc, word } => {
                    blockers.push(TransferBlockerV1::InvalidInstruction {
                        bank: bank.bank.to_string(),
                        site_pc: *pc,
                        word: *word,
                    });
                }
                BlockTerminator::DataFence { at } => {
                    blockers.push(TransferBlockerV1::DataFenceReached {
                        bank: bank.bank.to_string(),
                        site_pc: *at,
                    });
                }
                BlockTerminator::SelfReferentialBranch { at } => {
                    blockers.push(TransferBlockerV1::SelfReferentialBranchReached {
                        bank: bank.bank.to_string(),
                        site_pc: *at,
                    });
                }
            }
        }

        let mut seen_indirect = BTreeSet::new();
        for resolution in &bank.closure.indirect {
            if !seen_indirect.insert(resolution.site_pc) {
                return Err(TransferScanError::DuplicateIndirectSite {
                    bank: bank.bank.to_string(),
                    site_pc: resolution.site_pc,
                });
            }
            let transfer_kind = if resolution.via_call {
                "indirect_call"
            } else {
                "indirect_jump"
            };
            let (disposition, evidence) = match resolution.state {
                IndirectProofState::Bounded => {
                    if authority.is_some() {
                        catalog_guarded.push(CatalogGuardedTransferV1 {
                            bank: bank.bank.to_string(),
                            bank_id: bank.bank_id,
                            site_pc: resolution.site_pc,
                            kind: if resolution.via_call {
                                CatalogGuardedTransferKindV1::IndirectCall
                            } else {
                                CatalogGuardedTransferKindV1::IndirectJump
                            },
                        });
                        (
                            IndirectDispositionV1::Closed,
                            "bounded_target_set_guarded_by_catalog_total_resolver".to_string(),
                        )
                    } else {
                        blockers.push(TransferBlockerV1::IndirectBounded {
                            bank: bank.bank.to_string(),
                            site_pc: resolution.site_pc,
                        });
                        (
                            IndirectDispositionV1::Bounded,
                            "bounded_target_set_not_exhaustive".to_string(),
                        )
                    }
                }
                IndirectProofState::Open => {
                    if authority.is_some() {
                        catalog_guarded.push(CatalogGuardedTransferV1 {
                            bank: bank.bank.to_string(),
                            bank_id: bank.bank_id,
                            site_pc: resolution.site_pc,
                            kind: if resolution.via_call {
                                CatalogGuardedTransferKindV1::IndirectCall
                            } else {
                                CatalogGuardedTransferKindV1::IndirectJump
                            },
                        });
                        (
                            IndirectDispositionV1::Closed,
                            "open_target_set_guarded_by_catalog_total_resolver".to_string(),
                        )
                    } else {
                        blockers.push(TransferBlockerV1::IndirectOpen {
                            bank: bank.bank.to_string(),
                            site_pc: resolution.site_pc,
                        });
                        (IndirectDispositionV1::Open, "target_set_open".to_string())
                    }
                }
                IndirectProofState::Exhaustive => {
                    let mut ownership_open = false;
                    for &target in &resolution.targets {
                        match classify_target(
                            bank,
                            target,
                            resolution.via_call,
                            owners,
                            host_targets,
                        ) {
                            DirectTransferDispositionV1::GuestOwner { .. }
                            | DirectTransferDispositionV1::InstalledHost { .. } => {}
                            DirectTransferDispositionV1::AmbiguousOwners => {
                                ownership_open = true;
                                blockers.push(TransferBlockerV1::IndirectTargetAmbiguous {
                                    bank: bank.bank.to_string(),
                                    site_pc: resolution.site_pc,
                                    target,
                                });
                            }
                            DirectTransferDispositionV1::OutsideCatalog => {
                                ownership_open = true;
                                blockers.push(TransferBlockerV1::IndirectTargetOutsideCatalog {
                                    bank: bank.bank.to_string(),
                                    site_pc: resolution.site_pc,
                                    target,
                                });
                            }
                        }
                    }
                    if ownership_open || resolution.targets.is_empty() {
                        if resolution.targets.is_empty() {
                            blockers.push(TransferBlockerV1::IndirectOpen {
                                bank: bank.bank.to_string(),
                                site_pc: resolution.site_pc,
                            });
                        }
                        (
                            IndirectDispositionV1::Open,
                            "exhaustive_target_ownership_open".to_string(),
                        )
                    } else {
                        (
                            IndirectDispositionV1::Closed,
                            format!(
                                "exhaustive_{}_targets_uniquely_owned",
                                resolution_kind_label(resolution.kind)
                            ),
                        )
                    }
                }
            };
            indirect_frontier.push(IndirectTransferFrontierV1 {
                bank: bank.bank.to_string(),
                guest_pc: resolution.site_pc,
                transfer_kind: transfer_kind.to_string(),
                disposition,
                evidence,
            });
        }
    }

    for owner in owners {
        if !banks
            .iter()
            .any(|bank| bank.bank == owner.bank && bank.bank_id == owner.bank_id)
        {
            blockers.push(TransferBlockerV1::RequiredOwnerScanMissing {
                bank: owner.bank.to_string(),
                bank_id: owner.bank_id,
                kind: owner.kind,
            });
        }
    }

    direct.sort_unstable();
    if let Some(pair) = direct.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(TransferScanError::DuplicateDirectTransfer {
            bank: pair[0].source_bank.clone(),
            site_pc: pair[0].site_pc,
        });
    }
    indirect_frontier.sort_unstable();
    indirect_frontier.dedup();
    catalog_guarded.sort_unstable();
    catalog_guarded.dedup();
    blockers.sort_unstable();
    blockers.dedup();

    let direct_host = direct
        .iter()
        .filter(|finding| {
            matches!(
                &finding.disposition,
                DirectTransferDispositionV1::InstalledHost { .. }
            )
        })
        .count() as u64;
    let direct_guest = direct
        .iter()
        .filter(|finding| {
            matches!(
                &finding.disposition,
                DirectTransferDispositionV1::GuestOwner { .. }
            )
        })
        .count() as u64;
    let direct_open = direct
        .iter()
        .filter(|finding| {
            matches!(
                &finding.disposition,
                DirectTransferDispositionV1::AmbiguousOwners
                    | DirectTransferDispositionV1::OutsideCatalog
            )
        })
        .count() as u64;
    let indirect_closed = indirect_frontier
        .iter()
        .filter(|site| site.disposition == IndirectDispositionV1::Closed)
        .count() as u64;
    let indirect_bounded = indirect_frontier
        .iter()
        .filter(|site| site.disposition == IndirectDispositionV1::Bounded)
        .count() as u64;
    let indirect_open = indirect_frontier
        .iter()
        .filter(|site| site.disposition == IndirectDispositionV1::Open)
        .count() as u64;
    let inventory = if blockers.is_empty() {
        TransferInventoryV1::Complete
    } else {
        TransferInventoryV1::Open
    };
    let coverage = if inventory == TransferInventoryV1::Complete && authority.is_some() {
        TransferScanCoverageV1::CatalogTotal
    } else if inventory == TransferInventoryV1::Complete {
        TransferScanCoverageV1::Exhaustive
    } else {
        TransferScanCoverageV1::BoundedReachableCfg
    };

    Ok(TransferScanV1 {
        coverage,
        summary: TransferSummaryV1 {
            direct_total: direct.len() as u64,
            direct_guest,
            direct_host,
            direct_open,
            indirect_closed,
            indirect_bounded,
            indirect_open,
        },
        inventory,
        direct,
        indirect_frontier,
        catalog_guarded,
        catalog_total_authority: authority.map(|authority| authority.evidence.clone()),
        blockers,
    })
}

fn validate_inputs(
    banks: &[TransferScanBankInput<'_>],
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
) -> Result<(), TransferScanError> {
    if banks.is_empty() {
        return Err(TransferScanError::EmptyScan);
    }
    let mut scans = BTreeSet::new();
    for bank in banks {
        if bank.bank.trim().is_empty()
            || !valid_range(bank.va_start, bank.va_end)
            || bank.closure.cfg.bank != bank.bank
        {
            if bank.closure.cfg.bank != bank.bank {
                return Err(TransferScanError::ClosureBankMismatch {
                    expected: bank.bank.to_string(),
                    actual: bank.closure.cfg.bank.clone(),
                });
            }
            return Err(TransferScanError::InvalidBankRange {
                bank: bank.bank.to_string(),
            });
        }
        if !scans.insert((bank.bank, bank.bank_id)) {
            return Err(TransferScanError::DuplicateScanBank {
                bank: bank.bank.to_string(),
                bank_id: bank.bank_id,
            });
        }
        let exact_owner = owners.iter().any(|owner| {
            owner.bank == bank.bank
                && owner.bank_id == bank.bank_id
                && owner.va_start == bank.va_start
                && owner.va_end == bank.va_end
        });
        if !exact_owner {
            return Err(TransferScanError::ScanBankMissingOwner {
                bank: bank.bank.to_string(),
                bank_id: bank.bank_id,
            });
        }
        let mut indirect_sites = BTreeSet::new();
        let mut unresolved_indirect_sites = BTreeSet::new();
        let mut expected_direct_calls = BTreeSet::new();
        let mut expected_tail_transfers = BTreeSet::new();
        let mut block_ranges = Vec::new();
        for block in &bank.closure.cfg.blocks {
            // `DataFence` and `SelfReferentialBranch` are the two terminators
            // descent can produce with zero decoded words; `start_va ==
            // end_va` is valid geometry for them alone.
            let fenced_at_start = match block.terminator {
                BlockTerminator::DataFence { at } | BlockTerminator::SelfReferentialBranch { at } => {
                    at == block.start_va
                }
                _ => false,
            };
            let zero_length_data_fence = block.start_va == block.end_va && fenced_at_start;
            if block.start_va < bank.va_start
                || block.end_va > bank.va_end
                || (block.start_va >= block.end_va && !zero_length_data_fence)
                || !block.start_va.is_multiple_of(4)
                || !block.end_va.is_multiple_of(4)
            {
                return Err(TransferScanError::InvalidBlockGeometry {
                    bank: bank.bank.to_string(),
                    start_pc: block.start_va,
                });
            }
            block_ranges.push((block.start_va, block.end_va));
            let needs_delay = matches!(
                block.terminator,
                BlockTerminator::Tail { .. }
                    | BlockTerminator::Call { .. }
                    | BlockTerminator::Branch { .. }
                    | BlockTerminator::BranchLikely { .. }
                    | BlockTerminator::Return
                    | BlockTerminator::Indirect { .. }
                    | BlockTerminator::ResolvedIndirect { .. }
            );
            if needs_delay && delay_control_site_pc(block.start_va, block.end_va).is_none() {
                return Err(TransferScanError::InvalidBlockGeometry {
                    bank: bank.bank.to_string(),
                    start_pc: block.start_va,
                });
            }
            match &block.terminator {
                BlockTerminator::Call { target, .. } => {
                    expected_direct_calls.insert((
                        delay_control_site_pc(block.start_va, block.end_va).unwrap(),
                        *target,
                    ));
                }
                BlockTerminator::Tail { target } => {
                    expected_tail_transfers.insert((
                        delay_control_site_pc(block.start_va, block.end_va).unwrap(),
                        *target,
                    ));
                }
                BlockTerminator::Branch { target, link, .. }
                | BlockTerminator::BranchLikely { target, link, .. } => {
                    if *link {
                        expected_direct_calls.insert((
                            delay_control_site_pc(block.start_va, block.end_va).unwrap(),
                            *target,
                        ));
                    }
                }
                BlockTerminator::Indirect { via_call }
                | BlockTerminator::ResolvedIndirect { via_call, .. } => {
                    let site = delay_control_site_pc(block.start_va, block.end_va).unwrap();
                    if !indirect_sites.insert((site, *via_call)) {
                        return Err(TransferScanError::DuplicateIndirectSite {
                            bank: bank.bank.to_string(),
                            site_pc: site,
                        });
                    }
                    if matches!(&block.terminator, BlockTerminator::Indirect { .. }) {
                        unresolved_indirect_sites.insert((site, *via_call));
                    }
                }
                _ => {}
            }
        }
        block_ranges.sort_unstable();
        if block_ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(TransferScanError::OverlappingBlocks {
                bank: bank.bank.to_string(),
            });
        }
        let direct_calls = bank
            .closure
            .cfg
            .direct_calls
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let tail_transfers = bank
            .closure
            .cfg
            .tail_transfers
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if direct_calls.len() != bank.closure.cfg.direct_calls.len()
            || tail_transfers.len() != bank.closure.cfg.tail_transfers.len()
            || direct_calls != expected_direct_calls
            || tail_transfers != expected_tail_transfers
        {
            return Err(TransferScanError::DirectDenominatorMismatch {
                bank: bank.bank.to_string(),
            });
        }
        let cfg_unresolved_sites = bank
            .closure
            .cfg
            .indirect_sites
            .iter()
            .map(|site| (site.pc, site.via_call))
            .collect::<BTreeSet<_>>();
        if cfg_unresolved_sites.len() != bank.closure.cfg.indirect_sites.len()
            || cfg_unresolved_sites != unresolved_indirect_sites
        {
            return Err(TransferScanError::IndirectDenominatorMismatch {
                bank: bank.bank.to_string(),
            });
        }
        let resolution_sites = bank
            .closure
            .indirect
            .iter()
            .map(|resolution| (resolution.site_pc, resolution.via_call))
            .collect::<BTreeSet<_>>();
        if indirect_sites != resolution_sites
            || resolution_sites.len() != bank.closure.indirect.len()
        {
            return Err(TransferScanError::IndirectDenominatorMismatch {
                bank: bank.bank.to_string(),
            });
        }
        for block in &bank.closure.cfg.blocks {
            let (targets, via_call, resolved) = match &block.terminator {
                BlockTerminator::ResolvedIndirect { targets, via_call } => {
                    (targets.as_slice(), *via_call, true)
                }
                BlockTerminator::Indirect { via_call } => (&[][..], *via_call, false),
                _ => continue,
            };
            let site_pc = delay_control_site_pc(block.start_va, block.end_va).unwrap();
            let resolution = bank
                .closure
                .indirect
                .iter()
                .find(|resolution| resolution.site_pc == site_pc && resolution.via_call == via_call)
                .expect("indirect denominator equality was validated");
            let cfg_targets = targets.iter().copied().collect::<BTreeSet<_>>();
            let evidence_targets = resolution.targets.iter().copied().collect::<BTreeSet<_>>();
            let evidence_is_canonical = evidence_targets.len() == resolution.targets.len();
            let cfg_is_canonical = cfg_targets.len() == targets.len();
            if resolved != (resolution.state == IndirectProofState::Exhaustive)
                || (resolved && cfg_targets != evidence_targets)
                || !evidence_is_canonical
                || !cfg_is_canonical
            {
                return Err(TransferScanError::IndirectEvidenceMismatch {
                    bank: bank.bank.to_string(),
                    site_pc,
                });
            }
        }
    }
    let mut owner_identities = BTreeSet::new();
    for owner in owners {
        if owner.bank.trim().is_empty() || !valid_range(owner.va_start, owner.va_end) {
            return Err(TransferScanError::InvalidOwnerRange {
                bank: owner.bank.to_string(),
            });
        }
        if !owner_identities.insert((owner.bank, owner.bank_id)) {
            return Err(TransferScanError::DuplicateOwner {
                bank: owner.bank.to_string(),
                bank_id: owner.bank_id,
            });
        }
    }
    let mut hosts = BTreeSet::new();
    for host in host_targets {
        if host.bank.trim().is_empty() || !host.guest_pc.is_multiple_of(4) {
            return Err(TransferScanError::InvalidHostTarget {
                bank: host.bank.to_string(),
                guest_pc: host.guest_pc,
            });
        }
        if !hosts.insert(host.guest_pc) {
            return Err(TransferScanError::DuplicateHostTarget {
                guest_pc: host.guest_pc,
            });
        }
    }
    Ok(())
}

fn valid_range(start: u32, end: u32) -> bool {
    start < end && start.is_multiple_of(4) && end.is_multiple_of(4)
}

fn delay_control_site_pc(start_va: u32, end_va: u32) -> Option<u32> {
    end_va.checked_sub(8).filter(|pc| *pc >= start_va)
}

fn ordinary_terminal_site_pc(start_va: u32, end_va: u32) -> Option<u32> {
    end_va.checked_sub(4).filter(|pc| *pc >= start_va)
}

#[allow(clippy::too_many_arguments)]
fn push_direct(
    direct: &mut Vec<DirectTransferV1>,
    blockers: &mut Vec<TransferBlockerV1>,
    bank: &TransferScanBankInput<'_>,
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
    site_pc: u32,
    kind: DirectTransferKindV1,
    target: u32,
) {
    let disposition = classify_target(
        bank,
        target,
        kind == DirectTransferKindV1::Call,
        owners,
        host_targets,
    );
    match &disposition {
        DirectTransferDispositionV1::OutsideCatalog => {
            blockers.push(TransferBlockerV1::DirectTargetOutsideCatalog {
                bank: bank.bank.to_string(),
                site_pc,
                target,
            });
        }
        DirectTransferDispositionV1::AmbiguousOwners => {
            blockers.push(TransferBlockerV1::DirectTargetAmbiguous {
                bank: bank.bank.to_string(),
                site_pc,
                target,
            });
        }
        DirectTransferDispositionV1::GuestOwner { .. }
        | DirectTransferDispositionV1::InstalledHost { .. } => {}
    }
    direct.push(DirectTransferV1 {
        source_bank: bank.bank.to_string(),
        source_bank_id: bank.bank_id,
        site_pc,
        kind,
        target,
        disposition,
    });
}

fn classify_target(
    source: &TransferScanBankInput<'_>,
    target: u32,
    host_call_allowed: bool,
    owners: &[TransferOwnerInput<'_>],
    host_targets: &[HostTransferTargetInput<'_>],
) -> DirectTransferDispositionV1 {
    if !target.is_multiple_of(4) {
        return DirectTransferDispositionV1::OutsideCatalog;
    }
    if host_call_allowed {
        if let Some(host) = host_targets.iter().find(|host| host.guest_pc == target) {
            return DirectTransferDispositionV1::InstalledHost {
                bank: host.bank.to_string(),
                guest_pc: target,
            };
        }
    }
    let mut matching = owners
        .iter()
        .filter(|owner| {
            owner.va_start <= target
                && target
                    .checked_add(4)
                    .is_some_and(|target_end| target_end <= owner.va_end)
        })
        .collect::<Vec<_>>();
    if let Some(owner) = matching
        .iter()
        .find(|owner| owner.bank == source.bank && owner.bank_id == source.bank_id)
    {
        return DirectTransferDispositionV1::GuestOwner {
            bank: owner.bank.to_string(),
            bank_id: owner.bank_id,
        };
    }
    matching.sort_by_key(|owner| (owner.bank, owner.bank_id));
    matching.dedup_by_key(|owner| (owner.bank, owner.bank_id));
    match matching.as_slice() {
        [owner] => DirectTransferDispositionV1::GuestOwner {
            bank: owner.bank.to_string(),
            bank_id: owner.bank_id,
        },
        [] => DirectTransferDispositionV1::OutsideCatalog,
        _ => DirectTransferDispositionV1::AmbiguousOwners,
    }
}

fn resolution_kind_label(kind: Option<IndirectResolutionKind>) -> &'static str {
    match kind {
        Some(IndirectResolutionKind::Constant) => "constant",
        Some(IndirectResolutionKind::MemoryValueSet) => "memory_value_set",
        Some(IndirectResolutionKind::JumpTable) => "jump_table",
        None => "unspecified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{BasicBlock, Cfg};
    use crate::resolve::IndirectResolution;
    use std::collections::BTreeMap;

    fn closure(
        bank: &str,
        blocks: Vec<BasicBlock>,
        indirect: Vec<IndirectResolution>,
    ) -> ClosureResult {
        let mut direct_calls = Vec::new();
        let mut tail_transfers = Vec::new();
        let mut indirect_sites = Vec::new();
        for block in &blocks {
            let site_pc = delay_control_site_pc(block.start_va, block.end_va);
            match &block.terminator {
                BlockTerminator::Call { target, .. } => {
                    if let Some(site_pc) = site_pc {
                        direct_calls.push((site_pc, *target));
                    }
                }
                BlockTerminator::Tail { target } => {
                    if let Some(site_pc) = site_pc {
                        tail_transfers.push((site_pc, *target));
                    }
                }
                BlockTerminator::Branch { target, link, .. }
                | BlockTerminator::BranchLikely { target, link, .. }
                    if *link =>
                {
                    direct_calls.push((site_pc.unwrap(), *target));
                }
                BlockTerminator::Indirect { via_call } => {
                    if let Some(site_pc) = site_pc {
                        indirect_sites.push(crate::cfg::IndirectSite {
                            pc: site_pc,
                            via_call: *via_call,
                        });
                    }
                }
                _ => {}
            }
        }
        ClosureResult {
            cfg: Cfg {
                bank: bank.to_string(),
                word_class: BTreeMap::new(),
                blocks,
                direct_calls,
                tail_transfers,
                indirect_sites,
                plain_delay_entry_aliases: Vec::new(),
                unsupported_delay_entries: Vec::new(),
                rejected_transfer_targets: Vec::new(),
                proven_roots: vec![0x8000_1000],
            },
            indirect,
        }
    }

    fn owner<'a>(bank: &'a str, bank_id: u64, start: u32, end: u32) -> TransferOwnerInput<'a> {
        TransferOwnerInput {
            bank,
            bank_id,
            va_start: start,
            va_end: end,
            kind: TransferOwnerKindV1::DenseGeneration,
        }
    }

    fn complete_policy() -> CatalogResolverPolicyEvidenceV1 {
        fn64_recomp_rs::catalog_resolver_policy_evidence_v1()
    }

    /// Whether the linked `fn64-recomp-rs` artifact can grant catalog-total
    /// authority at all.
    ///
    /// `validate_catalog_total_transfer_authority_v1` refuses a build whose
    /// receipt reports `dev_interpreter`, which is deliberate: the dev
    /// interpreter is a development lane and must never carry production
    /// transfer authority. `fn64-discover` asks for
    /// `default-features = false, features = ["aot-runtime"]`, so building
    /// this crate alone satisfies that. Cargo unifies features across a
    /// workspace build, though, and `fn64-abi`'s dev-dependency requests
    /// `dev-interpreter` -- so under `cargo nextest run --workspace` the
    /// receipt this crate observes reports `dev_interpreter: true` and every
    /// grant is correctly refused.
    ///
    /// Tests that assert a *successful* grant therefore describe a lane the
    /// workspace build is not in. They skip rather than fail: the refusal is
    /// the rule working, not a regression. Tests asserting a refusal stay
    /// unconditional, because refusal holds in both configurations.
    fn catalog_total_authority_is_grantable() -> bool {
        !fn64_recomp_rs::static_execution_build_receipt().dev_interpreter
    }

    #[test]
    fn direct_guest_and_exact_host_calls_are_classified() {
        let closure = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1000,
                    end_va: 0x8000_1008,
                    terminator: BlockTerminator::Call {
                        target: 0x8000_3000,
                        next: 0x8000_1008,
                    },
                },
                BasicBlock {
                    start_va: 0x8000_1008,
                    end_va: 0x8000_1010,
                    terminator: BlockTerminator::Tail {
                        target: 0x8000_1020,
                    },
                },
            ],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let owners = [owner("resident", 1, 0x8000_1000, 0x8000_1100)];
        let hosts = [HostTransferTargetInput {
            bank: "resident",
            guest_pc: 0x8000_3000,
        }];
        let scan = scan_transfers_v1(&banks, &owners, &hosts).unwrap();

        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert_eq!(scan.summary.direct_host, 1);
        assert_eq!(scan.summary.direct_guest, 2);
        assert_eq!(scan.summary.direct_open, 0);
        assert!(scan.blockers.iter().any(|blocker| matches!(
            blocker,
            TransferBlockerV1::CallableEntryDenominatorUnverified { .. }
        )));
    }

    #[test]
    fn branch_edges_are_distinct_and_outside_target_stays_open() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1000,
                end_va: 0x8000_1008,
                terminator: BlockTerminator::Branch {
                    target: 0x9000_0000,
                    fallthrough: 0x8000_1008,
                    link: false,
                },
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert_eq!(scan.direct.len(), 2);
        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert!(scan.blockers.iter().any(|blocker| matches!(
            blocker,
            TransferBlockerV1::DirectTargetOutsideCatalog {
                target: 0x9000_0000,
                ..
            }
        )));
    }

    #[test]
    fn bounded_and_open_indirect_sites_remain_typed_frontier() {
        let closure = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1010,
                    end_va: 0x8000_1018,
                    terminator: BlockTerminator::Indirect { via_call: false },
                },
                BasicBlock {
                    start_va: 0x8000_1020,
                    end_va: 0x8000_1028,
                    terminator: BlockTerminator::Indirect { via_call: true },
                },
            ],
            vec![
                IndirectResolution {
                    site_pc: 0x8000_1010,
                    via_call: false,
                    state: IndirectProofState::Bounded,
                    kind: Some(IndirectResolutionKind::JumpTable),
                    targets: vec![0x8000_1020],
                    memory_sources: Vec::new(),
                },
                IndirectResolution {
                    site_pc: 0x8000_1020,
                    via_call: true,
                    state: IndirectProofState::Open,
                    kind: None,
                    targets: Vec::new(),
                    memory_sources: Vec::new(),
                },
            ],
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let owners = [owner("resident", 1, 0x8000_1000, 0x8000_1100)];
        let scan = scan_transfers_v1(&banks, &owners, &[]).unwrap();

        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert_eq!(scan.summary.indirect_bounded, 1);
        assert_eq!(scan.summary.indirect_open, 1);
    }

    #[test]
    fn exhaustive_indirect_requires_unique_target_ownership() {
        let closure = closure(
            "source",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::ResolvedIndirect {
                    targets: vec![0x8000_3000],
                    via_call: true,
                },
            }],
            vec![IndirectResolution {
                site_pc: 0x8000_1010,
                via_call: true,
                state: IndirectProofState::Exhaustive,
                kind: Some(IndirectResolutionKind::Constant),
                targets: vec![0x8000_3000],
                memory_sources: Vec::new(),
            }],
        );
        let banks = [TransferScanBankInput {
            bank: "source",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let source = owner("source", 1, 0x8000_1000, 0x8000_1100);
        let left = owner("left", 2, 0x8000_3000, 0x8000_3100);
        let right = owner("right", 3, 0x8000_3000, 0x8000_3100);
        let scan = scan_transfers_v1(&banks, &[source, left, right], &[]).unwrap();

        assert_eq!(
            scan.indirect_frontier[0].disposition,
            IndirectDispositionV1::Open
        );
        assert!(scan.blockers.iter().any(|blocker| matches!(
            blocker,
            TransferBlockerV1::IndirectTargetAmbiguous {
                target: 0x8000_3000,
                ..
            }
        )));
    }

    #[test]
    fn jr_ra_remains_open_without_return_provenance() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1000,
                end_va: 0x8000_1008,
                terminator: BlockTerminator::Return,
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert!(scan
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TransferBlockerV1::ReturnAddressFlowOpen { .. })));
    }

    #[test]
    fn current_wm_root_coverage_cannot_close_inventory() {
        let closure = closure("resident", Vec::new(), Vec::new());
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert_eq!(scan.coverage, TransferScanCoverageV1::BoundedReachableCfg);
        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert!(matches!(
            scan.blockers.as_slice(),
            [TransferBlockerV1::RootDenominatorOpen { .. }]
        ));
    }

    #[test]
    fn caller_asserted_exhaustive_roots_cannot_close_inventory() {
        let closure = closure("resident", Vec::new(), Vec::new());
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::CallerAssertedExhaustiveCallableEntries,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert_eq!(scan.inventory, TransferInventoryV1::Open);
        assert!(matches!(
            scan.blockers.as_slice(),
            [TransferBlockerV1::CallableEntryDenominatorUnverified { .. }]
        ));
    }

    #[test]
    fn catalog_total_authority_retains_dynamic_sites_without_blocking() {
        if !catalog_total_authority_is_grantable() {
            return;
        }
        let closure = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1000,
                    end_va: 0x8000_1008,
                    terminator: BlockTerminator::Return,
                },
                BasicBlock {
                    start_va: 0x8000_1010,
                    end_va: 0x8000_1014,
                    terminator: BlockTerminator::Trap,
                },
                BasicBlock {
                    start_va: 0x8000_1020,
                    end_va: 0x8000_1028,
                    terminator: BlockTerminator::Indirect { via_call: false },
                },
                BasicBlock {
                    start_va: 0x8000_1030,
                    end_va: 0x8000_1038,
                    terminator: BlockTerminator::Indirect { via_call: true },
                },
            ],
            vec![
                IndirectResolution {
                    site_pc: 0x8000_1020,
                    via_call: false,
                    state: IndirectProofState::Bounded,
                    kind: Some(IndirectResolutionKind::JumpTable),
                    targets: vec![0x8000_1040],
                    memory_sources: Vec::new(),
                },
                IndirectResolution {
                    site_pc: 0x8000_1030,
                    via_call: true,
                    state: IndirectProofState::Open,
                    kind: None,
                    targets: Vec::new(),
                    memory_sources: Vec::new(),
                },
            ],
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let owners = [owner("resident", 1, 0x8000_1000, 0x8000_1100)];
        let bounded = scan_transfers_v1(&banks, &owners, &[]).unwrap();
        let bounded_json = serde_json::to_value(bounded.into_evidence()).unwrap();
        assert!(bounded_json.get("catalog_guarded").is_none());
        assert!(bounded_json.get("catalog_total_authority").is_none());

        let policy = complete_policy();
        let authority = validate_catalog_total_transfer_authority_v1(&banks, &owners, &[], &policy);
        // Workspace feature unification deliberately links the development
        // interpreter into this artifact. Such an artifact cannot issue
        // catalog-total authority; package-scoped AOT tests exercise the
        // successful arm without weakening the production admission rule.
        if policy.build_receipt().dev_interpreter {
            assert_eq!(
                authority,
                Err(TransferScanError::CatalogAuthorityPolicyIncomplete)
            );
            return;
        }
        let authority = authority.unwrap();
        let scan = scan_transfers_with_catalog_total_authority_v1(&banks, &owners, &[], &authority)
            .unwrap();

        assert_eq!(scan.coverage, TransferScanCoverageV1::CatalogTotal);
        assert_eq!(scan.inventory, TransferInventoryV1::Complete);
        assert_eq!(scan.summary.indirect_closed, 2);
        assert_eq!(scan.summary.indirect_bounded, 0);
        assert_eq!(scan.summary.indirect_open, 0);
        assert!(scan.blockers.is_empty());
        assert_eq!(scan.catalog_guarded.len(), 4);
        assert!(scan.catalog_guarded.iter().any(|site| {
            site.site_pc == 0x8000_1000 && site.kind == CatalogGuardedTransferKindV1::Return
        }));
        assert!(scan.catalog_guarded.iter().any(|site| {
            site.site_pc == 0x8000_1010 && site.kind == CatalogGuardedTransferKindV1::TrapException
        }));
        assert!(scan.indirect_frontier.iter().all(|site| {
            site.disposition == IndirectDispositionV1::Closed
                && site.evidence.contains("guarded_by_catalog_total_resolver")
        }));
        assert!(scan.catalog_total_authority.is_some());
        let catalog_json = serde_json::to_value(scan.into_evidence()).unwrap();
        assert!(catalog_json.get("catalog_guarded").is_some());
        assert!(catalog_json.get("catalog_total_authority").is_some());
    }

    #[test]
    fn catalog_total_authority_requires_implementation_policy_and_exact_owner_coverage() {
        let closure = closure("resident", Vec::new(), Vec::new());
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let resident = owner("resident", 1, 0x8000_1000, 0x8000_1100);
        let extra = owner("extra", 2, 0x8000_2000, 0x8000_2100);

        assert_eq!(
            validate_catalog_total_transfer_authority_v1(
                &banks,
                &[resident, extra],
                &[],
                &complete_policy(),
            ),
            Err(TransferScanError::CatalogAuthorityOwnerCoverageMismatch)
        );

        let policy = complete_policy();
        assert_eq!(policy.policy(), CATALOG_RESOLVER_POLICY_NAME_V1);
        assert_eq!(
            policy.exception_vectors(),
            &MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        );
        assert_eq!(policy.build_receipt(), static_execution_build_receipt());

        let exact_owners = [owner("resident", 1, 0x8000_1000, 0x8000_1100)];
        let linked_result =
            validate_catalog_total_transfer_authority_v1(&banks, &exact_owners, &[], &policy);
        if policy.build_receipt().dev_interpreter {
            assert_eq!(
                linked_result,
                Err(TransferScanError::CatalogAuthorityPolicyIncomplete)
            );
        } else {
            assert!(linked_result.is_ok());
        }
    }

    #[test]
    fn catalog_total_authority_is_bound_to_exact_catalog_inputs() {
        if !catalog_total_authority_is_grantable() {
            return;
        }
        let closure = closure("resident", Vec::new(), Vec::new());
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let owners = [owner("resident", 1, 0x8000_1000, 0x8000_1100)];
        let policy = complete_policy();
        let authority = validate_catalog_total_transfer_authority_v1(&banks, &owners, &[], &policy);
        // Workspace feature unification deliberately links the development
        // interpreter into this artifact. Such an artifact cannot issue
        // catalog-total authority; package-scoped AOT tests exercise the
        // successful arm without weakening the production admission rule.
        if policy.build_receipt().dev_interpreter {
            assert_eq!(
                authority,
                Err(TransferScanError::CatalogAuthorityPolicyIncomplete)
            );
            return;
        }
        let authority = authority.unwrap();
        let different_hosts = [HostTransferTargetInput {
            bank: "resident",
            guest_pc: 0x8000_3000,
        }];

        assert_eq!(
            scan_transfers_with_catalog_total_authority_v1(
                &banks,
                &owners,
                &different_hosts,
                &authority,
            ),
            Err(TransferScanError::CatalogAuthorityInputMismatch)
        );
    }

    #[test]
    fn cfg_and_indirect_resolution_denominators_must_match() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::Indirect { via_call: false },
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];

        assert!(matches!(
            scan_transfers_v1(
                &banks,
                &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
                &[],
            ),
            Err(TransferScanError::IndirectDenominatorMismatch { .. })
        ));
    }

    #[test]
    fn malformed_control_block_is_rejected() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1014,
                terminator: BlockTerminator::Tail {
                    target: 0x8000_1020,
                },
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];

        assert!(matches!(
            scan_transfers_v1(
                &banks,
                &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
                &[],
            ),
            Err(TransferScanError::InvalidBlockGeometry { .. })
        ));
    }

    #[test]
    fn zero_length_data_fence_block_is_valid_but_open() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1010,
                terminator: BlockTerminator::DataFence { at: 0x8000_1010 },
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert!(scan.blockers.iter().any(|blocker| matches!(
            blocker,
            TransferBlockerV1::DataFenceReached {
                site_pc: 0x8000_1010,
                ..
            }
        )));
    }

    #[test]
    fn ran_off_end_and_outside_direct_target_are_counted_open() {
        let closure = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1000,
                    end_va: 0x8000_1004,
                    terminator: BlockTerminator::RanOffEnd,
                },
                BasicBlock {
                    start_va: 0x8000_1010,
                    end_va: 0x8000_1018,
                    terminator: BlockTerminator::Tail {
                        target: 0x9000_0000,
                    },
                },
            ],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert_eq!(scan.summary.direct_total, 1);
        assert_eq!(scan.summary.direct_open, 1);
        assert!(scan
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TransferBlockerV1::RanOffEnd { .. })));
    }

    #[test]
    fn resolved_indirect_targets_and_state_must_match_cfg() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::ResolvedIndirect {
                    targets: vec![0x8000_1020],
                    via_call: false,
                },
            }],
            vec![IndirectResolution {
                site_pc: 0x8000_1010,
                via_call: false,
                state: IndirectProofState::Exhaustive,
                kind: Some(IndirectResolutionKind::JumpTable),
                targets: vec![0x8000_1030],
                memory_sources: Vec::new(),
            }],
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];

        assert!(matches!(
            scan_transfers_v1(
                &banks,
                &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
                &[],
            ),
            Err(TransferScanError::IndirectEvidenceMismatch { .. })
        ));
    }

    #[test]
    fn indirect_call_records_its_continuation() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::Indirect { via_call: true },
            }],
            vec![IndirectResolution {
                site_pc: 0x8000_1010,
                via_call: true,
                state: IndirectProofState::Open,
                kind: None,
                targets: Vec::new(),
                memory_sources: Vec::new(),
            }],
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert!(scan.direct.iter().any(|edge| {
            edge.kind == DirectTransferKindV1::CallContinuation && edge.target == 0x8000_1018
        }));
    }

    #[test]
    fn branch_and_link_preserves_call_semantics() {
        let closure = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::Branch {
                    target: 0x8000_3000,
                    fallthrough: 0x8000_1018,
                    link: true,
                },
            }],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[HostTransferTargetInput {
                bank: "resident",
                guest_pc: 0x8000_3000,
            }],
        )
        .unwrap();

        assert!(scan.direct.iter().any(|edge| {
            edge.kind == DirectTransferKindV1::Call
                && matches!(
                    edge.disposition,
                    DirectTransferDispositionV1::InstalledHost { .. }
                )
        }));
        assert!(scan.direct.iter().any(|edge| {
            edge.kind == DirectTransferKindV1::CallContinuation && edge.target == 0x8000_1018
        }));
    }

    #[test]
    fn cfg_direct_denominator_and_block_partition_are_exact() {
        let mut missing_tail = closure(
            "resident",
            vec![BasicBlock {
                start_va: 0x8000_1010,
                end_va: 0x8000_1018,
                terminator: BlockTerminator::Tail {
                    target: 0x8000_1020,
                },
            }],
            Vec::new(),
        );
        missing_tail.cfg.tail_transfers.clear();
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &missing_tail,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        assert!(matches!(
            scan_transfers_v1(
                &banks,
                &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
                &[],
            ),
            Err(TransferScanError::DirectDenominatorMismatch { .. })
        ));

        let overlap = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1000,
                    end_va: 0x8000_1008,
                    terminator: BlockTerminator::Fallthrough { next: 0x8000_1008 },
                },
                BasicBlock {
                    start_va: 0x8000_1004,
                    end_va: 0x8000_100c,
                    terminator: BlockTerminator::RanOffEnd,
                },
            ],
            Vec::new(),
        );
        let overlap_banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &overlap,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        assert!(matches!(
            scan_transfers_v1(
                &overlap_banks,
                &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
                &[],
            ),
            Err(TransferScanError::OverlappingBlocks { .. })
        ));
    }

    #[test]
    fn decoder_trap_and_data_fence_terminators_block_closure() {
        let closure = closure(
            "resident",
            vec![
                BasicBlock {
                    start_va: 0x8000_1000,
                    end_va: 0x8000_1004,
                    terminator: BlockTerminator::InvalidInstruction {
                        pc: 0x8000_1000,
                        word: 0xffff_ffff,
                    },
                },
                BasicBlock {
                    start_va: 0x8000_1010,
                    end_va: 0x8000_1014,
                    terminator: BlockTerminator::Trap,
                },
                BasicBlock {
                    start_va: 0x8000_1020,
                    end_va: 0x8000_1024,
                    terminator: BlockTerminator::DataFence { at: 0x8000_1024 },
                },
            ],
            Vec::new(),
        );
        let banks = [TransferScanBankInput {
            bank: "resident",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        }];
        let scan = scan_transfers_v1(
            &banks,
            &[owner("resident", 1, 0x8000_1000, 0x8000_1100)],
            &[],
        )
        .unwrap();

        assert!(scan
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TransferBlockerV1::InvalidInstruction { .. })));
        assert!(scan
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TransferBlockerV1::TrapExceptionFlowOpen { .. })));
        assert!(scan
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TransferBlockerV1::DataFenceReached { .. })));
    }

    #[test]
    fn input_order_does_not_change_scan() {
        let left_closure = closure("left", Vec::new(), Vec::new());
        let right_closure = closure("right", Vec::new(), Vec::new());
        let left_bank = TransferScanBankInput {
            bank: "left",
            bank_id: 1,
            va_start: 0x8000_1000,
            va_end: 0x8000_1100,
            closure: &left_closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        };
        let right_bank = TransferScanBankInput {
            bank: "right",
            bank_id: 2,
            va_start: 0x8000_2000,
            va_end: 0x8000_2100,
            closure: &right_closure,
            root_coverage: TransferRootCoverageV1::ProvenFactRoots,
        };
        let left_owner = owner("left", 1, 0x8000_1000, 0x8000_1100);
        let right_owner = owner("right", 2, 0x8000_2000, 0x8000_2100);

        let forward =
            scan_transfers_v1(&[left_bank, right_bank], &[left_owner, right_owner], &[]).unwrap();
        let reverse =
            scan_transfers_v1(&[right_bank, left_bank], &[right_owner, left_owner], &[]).unwrap();
        assert_eq!(forward, reverse);
    }
}
