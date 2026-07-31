//! Real-corpus characterization for answer-key-free thread-entry authority.
//!
//! The expected addresses grade the result; neither is supplied to snapshot
//! composition. The input is a user-owned ROM named explicitly by an env var,
//! and no ROM-derived bytes enter this repository.

use fn64_discover::banks::{LoadImageTableInput, StaticRequestDmaInput};
use fn64_discover::callback_flow::{
    discover_callback_argument_contracts, discover_callback_registry_contracts, IndirectListHead,
};
use fn64_discover::facts::{CandidateDetector, FunctionEntryEvidence};
use fn64_discover::host_bindings::discover_os_create_thread_host_binding;
use fn64_discover::owner_proof::{OwnerAssessment, OwnerBlocker};
use fn64_discover::resolve::build_cfg_value_set_closed;
use fn64_discover::run_discovery;
use fn64_discover::run_discovery_with_tables_and_request_dma;
use fn64_discover::snapshot::{
    compose_materialized_bank_v1, compose_materialized_banks_v1, MaterializedBankInput,
};
use fn64_discover::snapshot_inputs::prepare_snapshot_banks;
use fn64_discover::{Fact, ProofState};
use serde::Deserialize;

const OOT_NTSC_10_SHA256: &str = "c916ab315fbe82a22169bff13d6b866e9fddc907461eb6b0a227b82acdf5b506";
const BOOT_ROM_START: usize = 0x1000;
const BOOT_LEN: usize = 0x10_0000;
const BOOT_VA: u32 = 0x8000_0400;
const OS_CREATE_THREAD: u32 = 0x8000_2f20;
const DEV_MGR_THREAD_ENTRY: u32 = 0x8000_3680;

#[derive(Deserialize)]
struct TablesFile {
    load_image_tables: Vec<LoadImageTableInput>,
}

#[derive(Deserialize)]
struct RequestDmaFile {
    request_dma: Vec<StaticRequestDmaInput>,
}

fn assert_os_create_thread_binding(
    env: &str,
    expected_sha256: &str,
    boot_va: u32,
    expected_binding: u32,
) {
    let Some(path) = std::env::var_os(env) else {
        eprintln!("skip: {env} unset");
        return;
    };
    let rom_bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("{env} unreadable: {error}"));
    let (rom, _) = run_discovery(&rom_bytes, None).expect("normalize and discover ROM");
    assert_eq!(rom.sha256, expected_sha256, "wrong ROM revision for {env}");
    let boot = &rom.bytes[BOOT_ROM_START..BOOT_ROM_START + BOOT_LEN];
    let words: Vec<u32> = boot
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte word")))
        .collect();
    assert_eq!(
        discover_os_create_thread_host_binding(&words, boot_va)
            .expect("unique semantic osCreateThread")
            .vram,
        expected_binding
    );
}

#[test]
fn known_corpus_os_create_thread_bindings_are_semantic() {
    assert_os_create_thread_binding(
        "FN64_DISCOVER_MM_ROM",
        "efb1365b3ae362604514c0f9a1a2d11f5dc8688ba5be660a37debf5e3be43f2b",
        0x8008_0000,
        0x8008_9e40,
    );
    assert_os_create_thread_binding(
        "FN64_DISCOVER_SM64_ROM",
        "17ce077343c6133f8c9f2d6d6d9a4ab62c8cd2aa57c40aea1f490b4c8bb21d91",
        0x8024_6000,
        0x8032_26b0,
    );
    assert_os_create_thread_binding(
        "FN64_DISCOVER_K64_ROM",
        "2f579751d7ad2824dfd8a6141570306bfaeda1cff40139ba231c30b8591d681c",
        0x8000_0400,
        0x8002_fbe0,
    );
}

#[test]
fn mm_callback_contracts_are_derived_from_callee_dataflow() {
    let Some(path) = std::env::var_os("FN64_DISCOVER_MM_ROM") else {
        eprintln!("skip: FN64_DISCOVER_MM_ROM unset");
        return;
    };
    let rom_bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("FN64_DISCOVER_MM_ROM unreadable: {error}"));
    let (rom, facts) = run_discovery(&rom_bytes, None).expect("normalize and discover MM");
    assert_eq!(
        rom.sha256, "efb1365b3ae362604514c0f9a1a2d11f5dc8688ba5be660a37debf5e3be43f2b",
        "wrong MM ROM revision"
    );
    let boot = &rom.bytes[BOOT_ROM_START..BOOT_ROM_START + BOOT_LEN];
    let snapshot = compose_materialized_bank_v1(
        &rom,
        &facts,
        MaterializedBankInput {
            bank: "boot",
            va_start: 0x8008_0000,
            bytes: boot,
            seed_roots: &[],
        },
    )
    .expect("byte-verified MM boot snapshot");
    let contracts =
        discover_callback_argument_contracts(&snapshot.banks[0].closure.cfg, boot, 0x8008_0000)
            .expect("callback argument analysis");
    let partition = fn64_discover::partition::partition(&snapshot.banks[0].closure.cfg);
    let printf_owner = partition
        .owners
        .iter()
        .find(|owner| owner.root_va == 0x8008_e050);
    let printf_ambiguity = partition
        .ambiguous
        .iter()
        .find(|block| block.block_start == 0x8008_e050);
    assert!(
        contracts.iter().any(|contract| {
            contract.callee == 0x8008_e050
                && contract.pointer_arg_register == 4
                && contract.jalr_sites.contains(&0x8008_e0f8)
        }),
        "derived contracts: {contracts:#x?}; root={} owner={printf_owner:#x?} ambiguity={printf_ambiguity:#x?}",
        snapshot.banks[0]
            .closure
            .cfg
            .proven_roots
            .contains(&0x8008_e050),
    );

    let boot_registry_contracts =
        discover_callback_registry_contracts(&snapshot.banks[0].closure.cfg, boot, 0x8008_0000)
            .expect("callback registry analysis");
    let registrar_owner = partition
        .owners
        .iter()
        .find(|owner| owner.root_va == 0x8008_19f0);
    let dispatcher_owner = partition
        .owners
        .iter()
        .find(|owner| owner.root_va == 0x8008_358c);
    assert!(
        boot_registry_contracts.is_empty() && dispatcher_owner.is_none(),
        "hardware-only boot authority must not bootstrap an unproven dispatcher: {boot_registry_contracts:#x?}; dispatcher_owner={dispatcher_owner:#x?}"
    );

    // Test-only characterization of the exact missing frontier: a later bank
    // calls Fault_Init, which creates this thread entry. Once that cross-bank
    // authority exists, its direct call reaches the dispatcher and the
    // registry proof must become mechanical. These roots are assertions, not
    // production inputs to snapshot composition.
    let characterized = build_cfg_value_set_closed(
        "mm-callback-characterization",
        boot,
        0x8008_0000,
        &[0x8008_19f0, 0x8008_3828],
    );
    let registry_contracts =
        discover_callback_registry_contracts(&characterized.cfg, boot, 0x8008_0000)
            .expect("callback registry analysis with characterized authority");
    assert!(
        registry_contracts.iter().any(|contract| {
            contract.registrar == 0x8008_19f0
                && contract.dispatcher == 0x8008_358c
                && contract.object_arg_register == 4
                && contract.pointer_arg_register == 5
                && contract.callback_offset == 4
                && contract.link_offset == 0
                && contract.list_head
                    == IndirectListHead {
                        pointer_word_address: 0x8009_be50,
                        field_offset: 0x7d8,
                    }
                && matches!(contract.callback_store_site, 0x8008_1a54 | 0x8008_1a74)
                && contract.list_insert_site == 0x8008_1a98
                && contract.jalr_site == 0x8008_3634
        }),
        "derived registry contracts: {registry_contracts:#x?}; registrar_owner={registrar_owner:#x?}"
    );
}

#[test]
fn mm_fault_init_loaded_bank_caller_closes_through_cross_bank_fixed_point() {
    const FAULT_INIT: u32 = 0x8008_3bc4;
    const CALLER_ENTRY: u32 = 0x8017_4bf0;
    const CALL_SITE: u32 = 0x8017_4c28;
    let started = std::time::Instant::now();
    let Some(path) = std::env::var_os("FN64_DISCOVER_MM_ROM") else {
        eprintln!("skip: FN64_DISCOVER_MM_ROM unset");
        return;
    };
    let rom_bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("FN64_DISCOVER_MM_ROM unreadable: {error}"));
    let tables: TablesFile =
        toml::from_str(include_str!("../reference/mm-n64-us-load-tables.toml"))
            .expect("parse MM load-table claims");
    let request_dma: RequestDmaFile =
        toml::from_str(include_str!("../reference/mm-n64-us-request-dma.toml"))
            .expect("parse MM request-DMA claims");
    let (rom, facts, _) = run_discovery_with_tables_and_request_dma(
        &rom_bytes,
        None,
        &tables.load_image_tables,
        &request_dma.request_dma,
    )
    .expect("discover MM banks");
    let discovered = started.elapsed();
    assert_eq!(
        rom.sha256, "efb1365b3ae362604514c0f9a1a2d11f5dc8688ba5be660a37debf5e3be43f2b",
        "wrong MM ROM revision"
    );
    let prepared = prepare_snapshot_banks(&rom, &facts).expect("prepare MM banks");
    let banks_prepared = started.elapsed();
    let caller = prepared
        .banks()
        .iter()
        .find(|bank| bank.bank == "request_dma_0")
        .expect("prepared MM code bank");
    assert!(facts.facts().iter().any(|fact| matches!(
        fact,
        Fact::FunctionEntryClaim {
            target,
            detector: CandidateDetector::ProloguePattern,
            evidence: FunctionEntryEvidence::Prologue { .. },
            proposed_state: ProofState::Candidate,
        } if target.bank == caller.bank && target.pc == CALLER_ENTRY
    )));
    let call_offset = usize::try_from(CALL_SITE - caller.va_start).expect("call offset");
    let call_word = u32::from_be_bytes(
        caller.bytes[call_offset..call_offset + 4]
            .try_into()
            .expect("call word"),
    );
    assert_eq!(call_word, 0x0c00_0000 | (FAULT_INIT >> 2 & 0x03ff_ffff));
    assert!(!prepared.banks().iter().any(|bank| {
        bank.bytes.chunks_exact(4).any(|word| {
            u32::from_be_bytes(word.try_into().expect("four-byte word")) == CALLER_ENTRY
        })
    }));

    let inputs: Vec<_> = prepared
        .banks()
        .iter()
        .filter(|bank| matches!(bank.bank.as_str(), "boot" | "request_dma_0"))
        .map(|bank| MaterializedBankInput {
            bank: &bank.bank,
            va_start: bank.va_start,
            bytes: &bank.bytes,
            seed_roots: &bank.traversal_seeds,
        })
        .collect();
    assert_eq!(inputs.len(), 2, "MM boot/code bank pair must be unique");
    let snapshots = compose_materialized_banks_v1(&rom, &facts, &inputs)
        .expect("compose MM boot/code fixed point");
    let snapshots_composed = started.elapsed();
    let code = snapshots
        .iter()
        .find(|snapshot| snapshot.banks[0].input.bank == "request_dma_0")
        .expect("composed MM code bank");
    assert_eq!(
        code.banks[0].closure.cfg.word_class.get(&CALL_SITE),
        Some(&fn64_discover::cfg::WordClass::ProvenCode)
    );
    assert!(code.banks[0]
        .closure
        .cfg
        .direct_calls
        .contains(&(CALL_SITE, FAULT_INIT)));

    let boot = snapshots
        .iter()
        .find(|snapshot| snapshot.banks[0].input.bank == "boot")
        .expect("composed MM boot bank");
    assert!(
        boot.banks[0]
            .closure
            .cfg
            .proven_roots
            .contains(&0x8008_3828),
        "Fault_Init authority must mechanically recover Fault_ThreadEntry"
    );
    let boot_bytes = &prepared
        .banks()
        .iter()
        .find(|bank| bank.bank == "boot")
        .expect("prepared MM boot bank")
        .bytes;
    let contracts = discover_callback_registry_contracts(
        &boot.banks[0].closure.cfg,
        boot_bytes,
        boot.banks[0].input.va_start,
    )
    .expect("registry contract after cross-bank fixed point");
    assert!(contracts.iter().any(|contract| {
        contract.registrar == 0x8008_19f0
            && contract.dispatcher == 0x8008_358c
            && contract.pointer_arg_register == 5
            && contract.jalr_site == 0x8008_3634
    }));
    eprintln!(
        "MM fixed-point profile: discovery={discovered:?} preparation={:?} composition={:?} total={:?}",
        banks_prepared - discovered,
        snapshots_composed - banks_prepared,
        started.elapsed()
    );
}

#[test]
fn oot_thread_entry_is_authorized_without_an_entry_argument_manifest() {
    let Some(path) = std::env::var_os("FN64_DISCOVER_OOT_ROM") else {
        eprintln!("skip: FN64_DISCOVER_OOT_ROM unset");
        return;
    };
    let rom_bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("FN64_DISCOVER_OOT_ROM unreadable: {error}"));
    let (rom, facts) = run_discovery(&rom_bytes, None).expect("normalize and discover OoT");
    assert_eq!(rom.sha256, OOT_NTSC_10_SHA256, "wrong OoT ROM revision");
    let boot = &rom.bytes[BOOT_ROM_START..BOOT_ROM_START + BOOT_LEN];
    let words: Vec<u32> = boot
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte word")))
        .collect();
    assert_eq!(
        discover_os_create_thread_host_binding(&words, BOOT_VA)
            .expect("unique semantic osCreateThread")
            .vram,
        OS_CREATE_THREAD
    );

    let snapshot = compose_materialized_bank_v1(
        &rom,
        &facts,
        MaterializedBankInput {
            bank: "boot",
            va_start: BOOT_VA,
            bytes: boot,
            seed_roots: &[],
        },
    )
    .expect("byte-verified boot snapshot");
    let bank = &snapshot.banks[0];
    assert!(bank
        .closure
        .cfg
        .proven_roots
        .contains(&DEV_MGR_THREAD_ENTRY));
    let assessment = bank
        .owner_proof
        .assessments
        .iter()
        .find(|assessment| assessment.entry().pc == DEV_MGR_THREAD_ENTRY)
        .expect("thread entry owner assessment");
    if let OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } =
        assessment
    {
        assert!(
            !frontier
                .blockers
                .contains(&OwnerBlocker::EntryNotAuthoritative),
            "semantic osCreateThread call did not confer callable authority"
        );
    }
}
