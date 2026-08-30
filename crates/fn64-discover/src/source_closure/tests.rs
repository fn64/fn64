use super::*;

const ROM_SHA: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PACK_SHA: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const IMAGE_SHA: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn open_transfer_scan() -> TransferScanV1 {
    let closure = crate::resolve::ClosureResult {
        cfg: crate::cfg::Cfg {
            bank: "resident".to_string(),
            word_class: std::collections::BTreeMap::new(),
            blocks: Vec::new(),
            direct_calls: Vec::new(),
            tail_transfers: Vec::new(),
            indirect_sites: Vec::new(),
            plain_delay_entry_aliases: Vec::new(),
            unsupported_delay_entries: Vec::new(),
            rejected_transfer_targets: Vec::new(),
            proven_roots: vec![0x8000_0000],
        },
        indirect: Vec::new(),
    };
    crate::transfer_scan::scan_transfers_v1(
        &[crate::transfer_scan::TransferScanBankInput {
            bank: "resident",
            bank_id: 7,
            va_start: 0x8000_0000,
            va_end: 0x8001_0000,
            closure: &closure,
            root_coverage: crate::transfer_scan::TransferRootCoverageV1::ProvenFactRoots,
        }],
        &[crate::transfer_scan::TransferOwnerInput {
            bank: "resident",
            bank_id: 7,
            va_start: 0x8000_0000,
            va_end: 0x8001_0000,
            kind: crate::transfer_scan::TransferOwnerKindV1::DenseGeneration,
        }],
        &[],
    )
    .unwrap()
}

fn external_vector_images() -> Vec<ExternalExecutableImageIdentityV1> {
    MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1
        .into_iter()
        .map(|destination| ExternalExecutableImageIdentityV1 {
            image_id: format!("vector-{destination:08x}"),
            lineage: "cpu_written".to_string(),
            generation: 0,
            va_start: destination,
            byte_len: 4,
            sha256: IMAGE_SHA.to_string(),
            first_executed_pc: destination,
        })
        .collect()
}

fn closed_exception_vectors() -> Vec<ModeledExceptionVectorV1> {
    external_vector_images()
        .iter()
        .map(|image| ModeledExceptionVectorV1 {
            destination: image.va_start,
            disposition: ExceptionVectorDispositionV1::ExactCodeOwner(image.into()),
        })
        .collect()
}

fn closed_host_bindings() -> Vec<HostBindingV1> {
    crate::host_bindings::WM_BLOCK_RUNTIME_HOST_SYMBOLS
        .into_iter()
        .enumerate()
        .map(|(index, symbol)| {
            let symbol = HostBindingSymbolV1::from(symbol);
            HostBindingV1 {
                bank: "resident".to_string(),
                guest_vram: 0x8000_2000 + index as u32 * 4,
                symbol,
                current_status_effect: symbol.current_status_effect(),
                spawned_status_effect: symbol.spawned_status_effect(),
            }
        })
        .collect()
}

fn input() -> ExecutableSourceFrontierInputV1 {
    ExecutableSourceFrontierInputV1 {
        producer: "source-frontier-test-v1".to_string(),
        normalized_rom_sha256: ROM_SHA.to_string(),
        dense_aot_pack_sha256: PACK_SHA.to_string(),
        initial_cop0_status: InitialCop0StatusAuthorityV1::BootContext {
            boot_context_sha256: "4444444444444444444444444444444444444444444444444444444444444444"
                .to_string(),
            producer: "black-box-test".to_string(),
            normalized_rom_sha256: ROM_SHA.to_string(),
            ipl3_sha256: "5555555555555555555555555555555555555555555555555555555555555555"
                .to_string(),
            destination_code: b'E',
            tv_standard: InitialBootTvStandardV1::Ntsc,
            entry_pc: 0x8000_0400,
            cp0_status: 0x3400_0000,
        },
        dense_generations: vec![DenseGenerationIdentityV1 {
            name: "resident".to_string(),
            bank_id: 7,
            source_rom_start: 0x1000,
            source_rom_end: 0x11_000,
            load_start: 0x8000_0000,
            load_end: 0x8001_0000,
            loaded_sha256: IMAGE_SHA.to_string(),
        }],
        external_images: external_vector_images(),
        exception_vectors: closed_exception_vectors(),
        host_bindings: closed_host_bindings(),
        cache_sites: vec![CacheSiteV1 {
            bank: "resident".to_string(),
            guest_pc: 0x8000_3000,
            raw_word: 0xbc89_0000,
            decoded_op: "cache".to_string(),
            base_register: 4,
            offset: 0,
            word_class: "instruction_invalidate".to_string(),
            disposition: CacheSiteDispositionV1::ReachableInstruction,
            evidence: "a0_a1".to_string(),
        }],
        direct_dma_findings: vec![DirectDmaFindingV1 {
            caller_bank: "resident".to_string(),
            caller_pc: 0x8000_4000,
            primitive_bank: "resident".to_string(),
            primitive_pc: 0x8000_4100,
            device_start: 0x1000,
            device_end: 0x1040,
            rdram_start: 0x10000,
            rdram_end: 0x10040,
            destination_owner: ExecutableDestinationOwnerV1::DenseGeneration {
                bank_id: 7,
                va_start: 0x8001_0000,
                va_end: 0x8001_0040,
            },
        }],
        direct_dma_blockers: Vec::new(),
        raw_pi_primitives: vec![RawPiPrimitiveV1 {
            bank: "resident".to_string(),
            entry_pc: 0x8000_4100,
            symbol: "raw_pi_dma".to_string(),
            register_site_pcs: vec![0x8000_4120, 0x8000_4110],
            callers: vec![RawPiCallerV1 {
                caller_bank: "resident".to_string(),
                caller_pc: 0x8000_4000,
                primitive_pc: 0x8000_4100,
                resolution: WriterResolutionV1::AdmittedDenseGeneration,
                evidence: "resident".to_string(),
            }],
        }],
        cpu_store_watched_destinations: Vec::new(),
        cpu_store_scans: Vec::new(),
        cop0_status_scans: vec![Cop0StatusScanV1 {
            bank: "resident".to_string(),
            bank_id: 7,
            aligned_word_count: 0x4000,
            proven_code_writes: Vec::new(),
            proven_data_words: Vec::new(),
            unclassified_writes: Vec::new(),
            proven_code_value_proofs: Vec::new(),
            open_indirect_sites: Vec::new(),
        }],
        external_cop0_status_scans: external_vector_images()
            .into_iter()
            .map(|image| ExternalCop0StatusScanV1 {
                image_id: image.image_id,
                generation: image.generation,
                va_start: image.va_start,
                byte_len: image.byte_len,
                sha256: image.sha256,
                first_executed_pc: image.va_start,
                aligned_word_count: 1,
                proven_code_writes: Vec::new(),
                proven_data_words: Vec::new(),
                unclassified_writes: Vec::new(),
                proven_code_value_proofs: Vec::new(),
                open_indirect_sites: Vec::new(),
            })
            .collect(),
        conditional_cpu_word_stores: Vec::new(),
        open_cpu_word_stores: Vec::new(),
        transfer_scan: TransferScanV1::synthetic_complete_for_validator_test(vec![
            IndirectTransferFrontierV1 {
                bank: "resident".to_string(),
                guest_pc: 0x8000_5000,
                transfer_kind: "jalr".to_string(),
                disposition: IndirectDispositionV1::Closed,
                evidence: "complete table".to_string(),
            },
        ]),
        open_writer_classes: Vec::new(),
    }
}

#[test]
fn construction_is_order_independent_and_deduplicated() {
    let first = ExecutableSourceFrontierV1::new(input()).unwrap();
    let mut permuted = input();
    permuted.external_images.reverse();
    permuted
        .external_images
        .push(permuted.external_images[0].clone());
    permuted.host_bindings.reverse();
    permuted
        .host_bindings
        .push(permuted.host_bindings[0].clone());
    permuted.raw_pi_primitives[0].register_site_pcs.reverse();
    permuted.raw_pi_primitives[0]
        .register_site_pcs
        .push(0x8000_4110);
    let second = ExecutableSourceFrontierV1::new(permuted).unwrap();

    assert_eq!(first, second);
    assert_eq!(
        first.canonical_json_bytes().unwrap(),
        second.canonical_json_bytes().unwrap()
    );
    assert_eq!(
        first.canonical_sha256().unwrap(),
        second.canonical_sha256().unwrap()
    );
    assert_eq!(first.external_images.len(), 6);
    assert_eq!(first.host_bindings.len(), 15);
    assert_eq!(
        first.raw_pi_primitives[0].register_site_pcs,
        vec![0x8000_4110, 0x8000_4120]
    );
}

#[test]
fn canonical_digest_changes_with_bound_evidence() {
    let baseline = ExecutableSourceFrontierV1::new(input()).unwrap();
    let mut changed = input();
    changed.host_bindings[0].guest_vram += 0x1000;
    let changed = ExecutableSourceFrontierV1::new(changed).unwrap();
    assert_ne!(
        baseline.canonical_sha256().unwrap(),
        changed.canonical_sha256().unwrap()
    );
}

#[test]
fn canonical_receipt_retains_the_opaque_transfer_scan_evidence() {
    let baseline = ExecutableSourceFrontierV1::new(input()).unwrap();
    let mut changed = input();
    changed.transfer_scan = open_transfer_scan();
    let changed = ExecutableSourceFrontierV1::new(changed).unwrap();

    assert_ne!(
        baseline.canonical_sha256().unwrap(),
        changed.canonical_sha256().unwrap()
    );
    assert_eq!(changed.transfer_scan.inventory, TransferInventoryV1::Open);
    assert!(!changed.transfer_scan.blockers.is_empty());
    let json: serde_json::Value =
        serde_json::from_slice(&changed.canonical_json_bytes().unwrap()).unwrap();
    assert!(json["transfer_scan"]["blockers"].is_array());
    assert!(json.get("transfer_summary").is_none());
    assert!(json.get("transfer_inventory").is_none());
    assert!(json.get("indirect_frontier").is_none());
}

#[test]
fn initial_status_authority_is_rom_bound_and_fail_closed() {
    let mut missing = input();
    missing.initial_cop0_status = InitialCop0StatusAuthorityV1::Missing;
    assert!(ExecutableSourceFrontierV1::new(missing)
        .unwrap()
        .has_open_frontier());

    let mut bev_set = input();
    let InitialCop0StatusAuthorityV1::BootContext { cp0_status, .. } =
        &mut bev_set.initial_cop0_status
    else {
        unreachable!()
    };
    *cp0_status |= STATUS_BEV;
    assert!(ExecutableSourceFrontierV1::new(bev_set)
        .unwrap()
        .has_open_frontier());

    let mut mismatched = input();
    let InitialCop0StatusAuthorityV1::BootContext {
        normalized_rom_sha256,
        ..
    } = &mut mismatched.initial_cop0_status
    else {
        unreachable!()
    };
    *normalized_rom_sha256 = PACK_SHA.to_string();
    assert_eq!(
        ExecutableSourceFrontierV1::new(mismatched),
        Err(SourceFrontierError::InvalidInitialCop0StatusAuthority {
            field: "normalized_rom_sha256",
        })
    );
}

#[test]
fn frontier_detects_open_writers_cache_and_indirects() {
    let closed = ExecutableSourceFrontierV1::new(input()).unwrap();
    assert!(!closed.has_open_frontier());

    let mut writer = input();
    writer.open_writer_classes = vec![OpenWriterClass::CpuCopyStoreOrDecompression];
    assert!(ExecutableSourceFrontierV1::new(writer)
        .unwrap()
        .has_open_frontier());

    let mut inventory = input();
    inventory.transfer_scan = open_transfer_scan();
    assert!(ExecutableSourceFrontierV1::new(inventory)
        .unwrap()
        .has_open_frontier());

    let mut cache = input();
    cache.cache_sites[0].disposition = CacheSiteDispositionV1::Unclassified;
    assert!(ExecutableSourceFrontierV1::new(cache)
        .unwrap()
        .has_open_frontier());

    let mut host_copyback = input();
    host_copyback.host_bindings[0].current_status_effect =
        HostCurrentStatusEffectV1::CBridgeCopyBackUnclassified;
    assert_eq!(
        ExecutableSourceFrontierV1::new(host_copyback),
        Err(SourceFrontierError::InvalidHostBindingCatalog)
    );

    let child_status = ExecutableSourceFrontierV1::new(input()).unwrap();
    assert!(child_status.host_bindings.iter().any(|binding| {
        binding.symbol == HostBindingSymbolV1::OsCreateThread
            && binding.spawned_status_effect
                == HostSpawnedStatusEffectV1::GeneratedSavedSrPostEretClearsBev
    }));
    assert!(!child_status.has_open_frontier());

    let mut status = input();
    status.cop0_status_scans[0]
        .proven_code_writes
        .push(Cop0StatusWriteSiteV1 {
            site_pc: 0x8000_6000,
            instruction_word: 0x4088_6000, // mtc0 t0,Status
            source_register: 8,
            kind: Cop0StatusWriteKindV1::Mtc0,
        });
    status.cop0_status_scans[0]
        .proven_code_value_proofs
        .push(Cop0StatusValueProofV1 {
            site_pc: 0x8000_6000,
            values: Vec::new(),
            known_zero: 0,
            known_one: 0,
            blockers: vec![Cop0StatusValueBlockerV1::ValueOpen],
        });
    assert!(ExecutableSourceFrontierV1::new(status.clone())
        .unwrap()
        .has_open_frontier());
    status.cop0_status_scans[0].proven_code_value_proofs[0].known_zero = STATUS_BEV;
    assert!(!ExecutableSourceFrontierV1::new(status.clone())
        .unwrap()
        .has_open_frontier());
    let mut contradictory_bits = status.clone();
    contradictory_bits.cop0_status_scans[0].proven_code_value_proofs[0].known_one = STATUS_BEV;
    assert!(matches!(
        ExecutableSourceFrontierV1::new(contradictory_bits),
        Err(SourceFrontierError::InvalidCop0StatusScan { .. })
    ));
    status.cop0_status_scans[0].proven_code_value_proofs[0] = Cop0StatusValueProofV1 {
        site_pc: 0x8000_6000,
        values: vec![0x3400_0000],
        known_zero: !0x3400_0000,
        known_one: 0x3400_0000,
        blockers: Vec::new(),
    };
    assert!(!ExecutableSourceFrontierV1::new(status.clone())
        .unwrap()
        .has_open_frontier());
    status.cop0_status_scans[0].proven_code_value_proofs[0] = Cop0StatusValueProofV1 {
        site_pc: 0x8000_6000,
        values: vec![0x3440_0000],
        known_zero: !0x3440_0000,
        known_one: 0x3440_0000,
        blockers: Vec::new(),
    };
    assert!(ExecutableSourceFrontierV1::new(status)
        .unwrap()
        .has_open_frontier());

    let mut dma_blocker = input();
    dma_blocker.direct_dma_blockers.push(DirectDmaBlockerV1 {
        caller_bank: "resident".to_string(),
        caller_pc: Some(0x8000_6000),
        primitive_bank: "resident".to_string(),
        code: DirectDmaBlockerCodeV1::MutableDescriptor,
        writer_class: OpenWriterClass::MutableDmaDescriptorOutsideSlice,
        reason: "descriptor value is outside the proven slice".to_string(),
    });
    assert!(ExecutableSourceFrontierV1::new(dma_blocker)
        .unwrap()
        .has_open_frontier());

    let mut raw_caller = input();
    raw_caller.raw_pi_primitives[0].callers[0].resolution = WriterResolutionV1::Bounded;
    let raw_caller = ExecutableSourceFrontierV1::new(raw_caller).unwrap();
    assert!(raw_caller.has_open_frontier());
    assert!(raw_caller
        .open_writer_classes
        .contains(&OpenWriterClass::IndirectPiEpiCall));
}

#[test]
fn diagnostics_separate_total_inventory_from_open_categories() {
    let closed = ExecutableSourceFrontierV1::new(input()).unwrap();
    let diagnostics = closed.diagnostics();
    assert_eq!(diagnostics.open_exception_vectors, 0);
    assert_eq!(diagnostics.open_writer_classes, 0);
    assert_eq!(diagnostics.cache_sites, 1);
    assert_eq!(diagnostics.unclassified_cache_sites, 0);
    assert_eq!(diagnostics.raw_pi_primitives, 1);
    assert_eq!(diagnostics.raw_pi_open_callers, 0);
    assert_eq!(diagnostics.cop0_unclassified_writes, 0);
    assert_eq!(diagnostics.cop0_value_open, 0);
    assert!(diagnostics.transfer_inventory_complete);

    let mut open = input();
    open.open_writer_classes = vec![OpenWriterClass::CpuCopyStoreOrDecompression];
    open.cache_sites[0].disposition = CacheSiteDispositionV1::Unclassified;
    open.raw_pi_primitives[0].callers[0].resolution = WriterResolutionV1::Open;
    open.transfer_scan = open_transfer_scan();
    let diagnostics = ExecutableSourceFrontierV1::new(open).unwrap().diagnostics();
    // The constructor derives IndirectPiEpiCall from the open raw caller
    // in addition to the explicitly supplied CPU-copy writer class.
    assert_eq!(diagnostics.open_writer_classes, 2);
    assert_eq!(diagnostics.unclassified_cache_sites, 1);
    assert_eq!(diagnostics.raw_pi_open_callers, 1);
    assert!(!diagnostics.transfer_inventory_complete);
}

#[test]
fn cop0_status_scan_is_generation_bound_and_decode_checked() {
    let mut missing = input();
    missing.cop0_status_scans.clear();
    assert_eq!(
        ExecutableSourceFrontierV1::new(missing),
        Err(SourceFrontierError::MissingCop0StatusScan {
            bank: "resident".to_string(),
            bank_id: 7,
        })
    );

    let mut invalid = input();
    invalid.cop0_status_scans[0]
        .unclassified_writes
        .push(Cop0StatusWriteSiteV1 {
            site_pc: 0x8000_6000,
            instruction_word: 0,
            source_register: 8,
            kind: Cop0StatusWriteKindV1::Mtc0,
        });
    assert!(matches!(
        ExecutableSourceFrontierV1::new(invalid),
        Err(SourceFrontierError::InvalidCop0StatusScan { .. })
    ));

    let mut missing_external = input();
    let removed = missing_external.external_cop0_status_scans.pop().unwrap();
    assert_eq!(
        ExecutableSourceFrontierV1::new(missing_external),
        Err(SourceFrontierError::MissingExternalCop0StatusScan {
            image_id: removed.image_id,
            generation: removed.generation,
        })
    );

    let mut external_write = input();
    let site_pc = external_write.external_cop0_status_scans[0].va_start;
    external_write.external_cop0_status_scans[0]
        .unclassified_writes
        .push(Cop0StatusWriteSiteV1 {
            site_pc,
            instruction_word: 0x4088_6000,
            source_register: 8,
            kind: Cop0StatusWriteKindV1::Mtc0,
        });
    assert!(ExecutableSourceFrontierV1::new(external_write)
        .unwrap()
        .has_open_frontier());
}

#[test]
fn bev_vectors_require_the_in_process_closed_status_and_source_invariant() {
    let mut closed = input();
    for vector in &mut closed.exception_vectors {
        if BEV_EXCEPTION_VECTOR_DESTINATIONS_V1.contains(&vector.destination) {
            vector.disposition = ExceptionVectorDispositionV1::BevClearInvariant;
        }
    }
    let receipt = ExecutableSourceFrontierV1::new(closed.clone()).unwrap();
    assert!(!receipt.has_open_frontier());

    let mut missing_initial = closed.clone();
    missing_initial.initial_cop0_status = InitialCop0StatusAuthorityV1::Missing;
    assert!(matches!(
        ExecutableSourceFrontierV1::new(missing_initial),
        Err(SourceFrontierError::InvalidBevClearVectorUnreachability { .. })
    ));

    let mut open_transfer = closed;
    open_transfer.transfer_scan = open_transfer_scan();
    assert!(matches!(
        ExecutableSourceFrontierV1::new(open_transfer),
        Err(SourceFrontierError::InvalidBevClearVectorUnreachability { .. })
    ));

    let mut open_normal_vector = input();
    for vector in &mut open_normal_vector.exception_vectors {
        if BEV_EXCEPTION_VECTOR_DESTINATIONS_V1.contains(&vector.destination) {
            vector.disposition = ExceptionVectorDispositionV1::BevClearInvariant;
        }
    }
    open_normal_vector.exception_vectors[0].disposition = ExceptionVectorDispositionV1::Open {
        reason: "normal handler owner is absent".to_string(),
    };
    assert!(matches!(
        ExecutableSourceFrontierV1::new(open_normal_vector),
        Err(SourceFrontierError::InvalidBevClearVectorUnreachability { .. })
    ));

    let mut forged_host = input();
    forged_host.host_bindings[0].current_status_effect = HostCurrentStatusEffectV1::PreservesBev;
    assert_eq!(
        ExecutableSourceFrontierV1::new(forged_host),
        Err(SourceFrontierError::InvalidHostBindingCatalog)
    );

    let mut missing_host = input();
    missing_host.host_bindings.pop();
    assert_eq!(
        ExecutableSourceFrontierV1::new(missing_host),
        Err(SourceFrontierError::InvalidHostBindingCatalog)
    );

    let mut wrong_vector = input();
    wrong_vector.exception_vectors[0].disposition = ExceptionVectorDispositionV1::BevClearInvariant;
    assert_eq!(
        ExecutableSourceFrontierV1::new(wrong_vector),
        Err(SourceFrontierError::InvalidBevClearVectorUnreachability {
            destination: 0x8000_0000,
        })
    );
}

#[test]
fn conflicting_external_image_identity_is_rejected() {
    let mut ambiguous = input();
    let mut conflict = ambiguous.external_images[0].clone();
    conflict.sha256 = ROM_SHA.to_string();
    ambiguous.external_images.push(conflict.clone());

    assert_eq!(
        ExecutableSourceFrontierV1::new(ambiguous),
        Err(SourceFrontierError::AmbiguousExternalImageIdentity {
            image_id: conflict.image_id,
            generation: conflict.generation,
        })
    );

    let mut overlapping = input();
    overlapping
        .external_images
        .iter_mut()
        .find(|image| image.va_start == 0x8000_0180)
        .unwrap()
        .byte_len = 8;
    overlapping
        .external_images
        .push(ExternalExecutableImageIdentityV1 {
            image_id: "overlap-between-entries".to_string(),
            lineage: "cpu_written".to_string(),
            generation: 0,
            va_start: 0x8000_0184,
            byte_len: 4,
            sha256: ROM_SHA.to_string(),
            first_executed_pc: 0x8000_0184,
        });
    assert!(matches!(
        ExecutableSourceFrontierV1::new(overlapping),
        Err(SourceFrontierError::OverlappingExternalImages { .. })
    ));
}

#[test]
fn modeled_exception_vectors_are_exact_and_open_vectors_keep_writer_open() {
    let mut missing = input();
    missing
        .exception_vectors
        .retain(|vector| vector.destination != 0x8000_0000);
    assert_eq!(
        ExecutableSourceFrontierV1::new(missing),
        Err(SourceFrontierError::MissingModeledExceptionVector {
            destination: 0x8000_0000,
        })
    );

    let mut unexpected = input();
    unexpected.exception_vectors.push(ModeledExceptionVectorV1 {
        destination: 0x8000_0100,
        disposition: ExceptionVectorDispositionV1::Open {
            reason: "cache-error vector is outside the modeled universe".to_string(),
        },
    });
    assert_eq!(
        ExecutableSourceFrontierV1::new(unexpected),
        Err(SourceFrontierError::UnexpectedModeledExceptionVector {
            destination: 0x8000_0100,
        })
    );

    let mut open = input();
    open.exception_vectors[0].disposition = ExceptionVectorDispositionV1::Open {
        reason: "no exact code owner or state proof".to_string(),
    };
    let open = ExecutableSourceFrontierV1::new(open).unwrap();
    assert!(open.has_open_frontier());
    assert!(open
        .open_writer_classes
        .contains(&OpenWriterClass::UnadmittedExceptionOrBevVector));
}

#[test]
fn vector_owner_must_bind_an_external_image_covering_the_entry_word() {
    let mut outside = input();
    let vector = outside
        .exception_vectors
        .iter_mut()
        .find(|vector| vector.destination == 0x8000_0180)
        .unwrap();
    let ExceptionVectorDispositionV1::ExactCodeOwner(owner) = &mut vector.disposition else {
        panic!("test fixture must use an exact owner for the general vector");
    };
    owner.va_start = 0x8000_0184;
    assert_eq!(
        ExecutableSourceFrontierV1::new(outside),
        Err(SourceFrontierError::InvalidExceptionVectorOwner {
            destination: 0x8000_0180,
        })
    );

    let mut range_only = input();
    let vector = range_only
        .exception_vectors
        .iter_mut()
        .find(|vector| vector.destination == 0x8000_0180)
        .unwrap();
    let ExceptionVectorDispositionV1::ExactCodeOwner(owner) = &mut vector.disposition else {
        panic!("test fixture must use an exact owner for the general vector");
    };
    owner.first_executed_pc = 0x8000_0184;
    assert_eq!(
        ExecutableSourceFrontierV1::new(range_only),
        Err(SourceFrontierError::InvalidExceptionVectorOwner {
            destination: 0x8000_0180,
        })
    );
}

#[test]
fn opaque_unreachability_claim_is_rejected_until_a_validator_exists() {
    let mut unsupported = input();
    unsupported.exception_vectors[0].disposition =
        ExceptionVectorDispositionV1::MachineCheckedUnreachability(
            MachineCheckedUnreachabilityV1 {
                proof_schema: "fn64.opaque-claim.v1".to_string(),
                proof_sha256: PACK_SHA.to_string(),
            },
        );
    let destination = unsupported.exception_vectors[0].destination;
    assert_eq!(
        ExecutableSourceFrontierV1::new(unsupported),
        Err(SourceFrontierError::InvalidExceptionVectorUnreachability { destination })
    );
}

#[test]
fn conditional_cpu_store_is_generation_bound_and_stays_open() {
    let mut conditional = input();
    conditional.cpu_store_watched_destinations = MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1.to_vec();
    conditional.cpu_store_scans = vec![CpuStoreScanV1 {
        bank: "resident".to_string(),
        bank_id: 7,
        proven_root_count: 1,
        reachable_block_count: 1,
        conditional_store_count: 1,
        open_store_count: 0,
        coverage: CpuStoreScanCoverageV1::BoundedReachableCfg,
    }];
    conditional.conditional_cpu_word_stores = vec![ConditionalCpuWordStoreV1 {
        writer_bank: "resident".to_string(),
        writer_bank_id: 7,
        site_pc: 0x8000_2000,
        destination: 0x8000_0000,
        value: 0x1234_5678,
        source_bank: "resident".to_string(),
        source_bank_id: 7,
        source_address: 0x8000_3000,
        source_value: 0x1234_5678,
        open_requirements: vec![
            ConditionalCpuWordStoreRequirementV1::StoreSiteExecutes,
            ConditionalCpuWordStoreRequirementV1::SourceStableUntilLoad,
        ],
    }];
    let receipt = ExecutableSourceFrontierV1::new(conditional).unwrap();
    assert!(receipt.has_open_frontier());
    assert_eq!(
        receipt.conditional_cpu_word_stores[0].open_requirements,
        vec![
            ConditionalCpuWordStoreRequirementV1::SourceStableUntilLoad,
            ConditionalCpuWordStoreRequirementV1::StoreSiteExecutes,
        ]
    );
    assert!(receipt
        .open_writer_classes
        .contains(&OpenWriterClass::CpuCopyStoreOrDecompression));

    let mut wrong_bank = input();
    let mut store = receipt.conditional_cpu_word_stores[0].clone();
    store.writer_bank_id = 8;
    wrong_bank.conditional_cpu_word_stores.push(store);
    assert!(matches!(
        ExecutableSourceFrontierV1::new(wrong_bank),
        Err(SourceFrontierError::InvalidConditionalCpuWordStore { .. })
    ));
}

#[test]
fn zero_finding_cpu_store_scan_is_evidence_not_open_frontier() {
    let mut closed = input();
    closed.cpu_store_watched_destinations = MODELED_EXCEPTION_VECTOR_DESTINATIONS_V1.to_vec();
    closed.cpu_store_scans = vec![CpuStoreScanV1 {
        bank: "resident".to_string(),
        bank_id: 7,
        proven_root_count: 1,
        reachable_block_count: 1,
        conditional_store_count: 0,
        open_store_count: 0,
        coverage: CpuStoreScanCoverageV1::BoundedReachableCfg,
    }];

    let receipt = ExecutableSourceFrontierV1::new(closed).unwrap();
    assert_eq!(receipt.cpu_store_scans.len(), 1);
    assert!(!receipt.has_open_frontier());
}
