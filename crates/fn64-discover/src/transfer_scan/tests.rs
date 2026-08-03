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
