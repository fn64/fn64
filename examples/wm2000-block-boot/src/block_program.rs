//! The ONE construction of WM2000's certified dense-AOT catalog program,
//! shared verbatim by both binaries in this package.
//!
//! `main.rs` (the headless batch runner) and `shell.rs` (the interactive
//! windowed runner) must boot the SAME program: the same 35 dense shards, the
//! same physically-backed generation catalog, the same captured
//! exception-vector images, and the same Cargo-source attestation. Duplicating
//! that ~440-line assembly into the second binary would let the two lanes drift
//! silently -- the shell could keep booting while the batch runner's gate
//! caught a regression, or vice versa. Neither would be evidence about the
//! other.
//!
//! This module is `#[path]`-included by both binary crate roots rather than
//! living in a `lib.rs`, because the runners it installs read crate-root state
//! (`main.rs`'s profiling statics and overlay-entry bitmap) that only exists in
//! that binary. The four gate runners are therefore INPUTS
//! ([`GateRunners`]) rather than hardcoded references: each binary supplies the
//! adapters appropriate to its lane, and the resulting artifact identity
//! records which role was installed, so the attestation still names exactly
//! what ran.
//!
//! Both binaries share this package's ONE `OUT_DIR`, so `pack.rs` and
//! `runner.rs` are generated once by `build.rs` and the 35 generated shard
//! crates are compiled once -- the shell lane costs no additional shard build.

use crate::*;

/// The four adapter roles a lane may install in front of the generated dense
/// runners. Keeping them as fields (rather than letting this module reach for
/// `main.rs`'s functions directly) is what makes the module includable by a
/// second binary that has no profiling instrumentation at all.
pub(crate) struct GateRunners {
    /// Wraps EVERY dense bank. `None` selects per-position adapters below,
    /// which is the ordinary (uninstrumented) configuration.
    pub(crate) dense_instrumentation: Option<GeneratedBankFn>,
    /// Wraps the FIRST boot shard only, to validate the black-box BootContext
    /// at the first generated-bank entry.
    pub(crate) entry_context: GeneratedBankFn,
    /// Wraps every overlay shard, recording which recovered generation was
    /// entered.
    pub(crate) overlay_generation: GeneratedBankFn,
    /// Wraps each captured exception-vector image, re-verifying its digest
    /// against live RDRAM before executing it.
    pub(crate) external_digest: GeneratedBankFn,
}

/// Everything the boot seam needs, assembled but not yet installed.
pub(crate) struct ConstructedCatalogProgram {
    pub(crate) catalog_program: CatalogBlockProgramV1,
    pub(crate) generation_catalog: PrecompiledGenerationCatalog,
    pub(crate) generation_backings: Vec<PrecompiledGenerationBackingV1>,
    pub(crate) generated_runner_bindings: Vec<CargoGeneratedRunnerSourceBindingV1>,
    /// Snapshot of the FULLY installed `BlockProgram`, taken immediately before
    /// the catalog sealed it. Read here rather than by the caller because the
    /// program is moved into the seal: this is the only point at which the
    /// complete installed program still exists as such.
    pub(crate) program_evidence: fn64_recomp_rs::BlockProgramEvidenceSnapshot,
}

/// Build the canonical program: install every dense shard against its
/// build-time ROM-derived digest, register the physically-backed generation
/// catalog, admit the captured exception-vector images, and seal the result
/// with the Cargo generated-runner source attestation.
///
/// Every assertion here is load-bearing evidence, not defensive coding: each
/// one binds a runtime artifact to the digest `build.rs` recovered from the
/// ROM. They are identical for both lanes by construction, because both lanes
/// call this function.
pub(crate) fn construct_catalog_program(
    program: fn64_recomp_rs::BlockProgram,
    gates: GateRunners,
    instruction_budget: InstructionBudget,
) -> ConstructedCatalogProgram {
    let mut program = program;
    assert_eq!(
        DENSE_AOT_ARTIFACTS.len(),
        pack::BOOT_SHARDS.len()
            + pack::RESIDENT_TAIL_SHARDS.len()
            + pack::OVERLAY_GENERATIONS
                .iter()
                .map(|generation| generation.shards.len())
                .sum::<usize>()
    );
    assert_eq!(DENSE_AOT_IDENTITIES.len(), DENSE_AOT_ARTIFACTS.len());
    let mut generated_runner_bindings = Vec::with_capacity(DENSE_AOT_ARTIFACTS.len() + 1);
    for (artifact_index, ((artifact, identity), expected)) in DENSE_AOT_ARTIFACTS
        .iter()
        .zip(DENSE_AOT_IDENTITIES)
        .take(pack::BOOT_SHARDS.len())
        .zip(pack::BOOT_SHARDS)
        .enumerate()
    {
        assert_eq!(artifact.bank_id, expected.bank_id);
        assert_eq!(identity.source_sha256, expected.source_sha256);
        let bank = BankId::new(expected.bank_id);
        let code_bank = (artifact.code_bank)();
        assert_eq!(code_bank.id(), bank);
        assert_eq!(code_bank_sha256(&code_bank), expected.code_sha256);
        assert_eq!(code_bank.vram_start(), GuestPc::new(expected.va_start));
        assert_eq!(
            code_bank.vram_end(),
            GuestPc::new(expected.va_start + expected.byte_len)
        );
        let mut region = ExecutableRegion::new(
            GuestPc::new(expected.va_start),
            GuestPc::new(expected.va_start + expected.byte_len),
        );
        let (runner, role) = if let Some(instrumented) = gates.dense_instrumentation {
            (instrumented, GeneratedAdapterRole::DenseInstrumentationGate)
        } else if artifact_index == 0 {
            (gates.entry_context, GeneratedAdapterRole::EntryContextGate)
        } else {
            (artifact.runner, GeneratedAdapterRole::DirectGenerated)
        };
        region
            .install(
                &mut program,
                code_bank,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    runner,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        identity.runner_source_sha256,
                        bank,
                        role,
                    ),
                ),
            )
            .expect("installing dense boot-shard runner");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: identity.runner_source_sha256,
            code_words_sha256: expected.code_sha256,
            vram_start: GuestPc::new(expected.va_start),
            vram_end: GuestPc::new(expected.va_start + expected.byte_len),
            composite_subrunner_count: expected.byte_len.div_ceil(2 * 1024),
            adapter_role: role,
        });
    }
    let dynamic_shards = std::iter::once(&pack::RESIDENT_TAIL_GENERATION)
        .chain(pack::OVERLAY_GENERATIONS.iter())
        .flat_map(|generation| generation.shards.iter());
    for (dynamic_index, ((artifact, identity), expected)) in DENSE_AOT_ARTIFACTS
        .iter()
        .zip(DENSE_AOT_IDENTITIES)
        .skip(pack::BOOT_SHARDS.len())
        .zip(dynamic_shards)
        .enumerate()
    {
        assert_eq!(artifact.bank_id, expected.bank_id);
        assert_eq!(identity.source_sha256, expected.source_sha256);
        let bank = BankId::new(artifact.bank_id);
        let code = (artifact.code_bank)();
        assert_eq!(code.id(), bank);
        assert_eq!(code_bank_sha256(&code), expected.code_sha256);
        assert_eq!(code.vram_start(), GuestPc::new(expected.va_start));
        assert_eq!(
            code.vram_end(),
            GuestPc::new(expected.va_start + expected.byte_len)
        );
        let (runner, role) = if let Some(instrumented) = gates.dense_instrumentation {
            (instrumented, GeneratedAdapterRole::DenseInstrumentationGate)
        } else if dynamic_index < pack::RESIDENT_TAIL_SHARDS.len() {
            (artifact.runner, GeneratedAdapterRole::DirectGenerated)
        } else {
            (
                gates.overlay_generation,
                GeneratedAdapterRole::OverlayGenerationGate,
            )
        };
        program
            .register(
                code,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    runner,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        identity.runner_source_sha256,
                        bank,
                        role,
                    ),
                ),
            )
            .expect("pre-registering immutable dynamic AOT artifact");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: identity.runner_source_sha256,
            code_words_sha256: expected.code_sha256,
            vram_start: GuestPc::new(expected.va_start),
            vram_end: GuestPc::new(expected.va_start + expected.byte_len),
            composite_subrunner_count: expected.byte_len.div_ceil(2 * 1024),
            adapter_role: role,
        });
    }
    let mut generation_catalog = PrecompiledGenerationCatalog::new();
    let mut generation_backings = Vec::new();
    let mut dense_definition_catalog = PrecompiledGenerationCatalog::new();
    let mut dense_definition_backings = Vec::new();
    for generation in
        std::iter::once(&pack::RESIDENT_TAIL_GENERATION).chain(pack::OVERLAY_GENERATIONS.iter())
    {
        let generation_id = GenerationId::new(generation.id);
        let image_start = GuestPc::new(generation.image_start);
        let image_end = GuestPc::new(generation.image_end);
        let invalidation_start = GuestPc::new(generation.invalidation_start);
        let invalidation_end = GuestPc::new(generation.invalidation_end);
        let shards = generation
            .shards
            .iter()
            .map(|shard| {
                PrecompiledShard::new(
                    BankId::new(shard.bank_id),
                    GuestPc::new(shard.va_start),
                    GuestPc::new(shard.va_start + shard.byte_len),
                )
                .expect("generated dynamic shard geometry is valid")
            })
            .collect::<Vec<_>>();
        let compiled_generation = PrecompiledGeneration::new(
            generation_id,
            image_start,
            image_end,
            invalidation_start,
            invalidation_end,
            generation.sha256,
            shards,
        )
        .expect("generated dynamic generation geometry is valid");
        dense_definition_catalog
            .register(compiled_generation.clone())
            .expect("dense generated generation catalog is unambiguous");
        generation_catalog
            .register(compiled_generation)
            .expect("generated dynamic generation catalog is unambiguous");
        assert!(
            (0x8000_0000..0xc000_0000).contains(&invalidation_start.get())
                && invalidation_end.get() <= 0xc000_0000,
            "generated dynamic generation backing must be direct-mapped KSEG"
        );
        let backing = PrecompiledGenerationBackingV1::new(
            generation_id,
            vec![BackedExecutableSpanV1::new(
                invalidation_start,
                invalidation_start.get() & 0x1fff_ffff,
                invalidation_end.get() - invalidation_start.get(),
            )
            .expect("generated dynamic physical backing is valid")],
        )
        .expect("generated dynamic generation backing is contiguous");
        dense_definition_backings.push(backing.clone());
        generation_backings.push(backing);
    }
    let dense_definition = BackedPrecompiledGenerationCatalogV1::new(
        dense_definition_catalog,
        dense_definition_backings,
    )
    .expect("dense generated generations have exact physical backings");
    assert_eq!(
        dense_definition.canonical_definition_sha256(),
        pack::DENSE_GENERATION_CATALOG_DEFINITION_SHA256,
        "runtime dense generation catalog must equal the build-time ROM-derived definition"
    );
    for image in pack::EXTERNAL_EXECUTABLE_IMAGES {
        let bank = BankId::new(image.bank_id);
        let image_start = GuestPc::new(image.va_start);
        let image_end = GuestPc::new(image.va_end);
        register_external_executable_generation(
            &mut generation_catalog,
            &mut generation_backings,
            bank,
            image_start,
            image_end,
            image.sha256,
        );
        let code = CodeBank::new(bank, GuestPc::new(image.va_start), image.words.to_vec())
            .expect("admitting captured exception-vector image");
        assert_eq!(code_bank_sha256(&code), image.sha256);
        let mut region =
            ExecutableRegion::new(GuestPc::new(image.va_start), GuestPc::new(image.va_end));
        region
            .install(
                &mut program,
                code,
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    gates.external_digest,
                    ProgramArtifactIdentity::generated_adapter(
                        pack::ROOT_ADAPTER_SOURCE_SHA256,
                        pack::EXTERNAL_RUNNER_SOURCE_SHA256,
                        bank,
                        GeneratedAdapterRole::ExternalDigestGate,
                    ),
                ),
            )
            .expect("installing captured exception-vector runner");
        generated_runner_bindings.push(CargoGeneratedRunnerSourceBindingV1 {
            bank,
            generated_runner_source_sha256: pack::EXTERNAL_RUNNER_SOURCE_SHA256,
            code_words_sha256: image.sha256,
            vram_start: GuestPc::new(image.va_start),
            vram_end: GuestPc::new(image.va_end),
            composite_subrunner_count: 1,
            adapter_role: GeneratedAdapterRole::ExternalDigestGate,
        });
    }
    let program_evidence = program.evidence_snapshot();
    let catalog_program =
        CatalogBlockProgramV1::new_with_cargo_generated_runner_source_attestation_v2(
            program,
            ExecutionKey::new(entry_bank(), GuestPc::new(pack::ENTRYPOINT)),
            instruction_budget,
            CargoGeneratedProgramSourceAttestationV2 {
                root_adapter_source_sha256: pack::ROOT_ADAPTER_SOURCE_SHA256,
                shard_cargo_source_tree_sha256: pack::SHARD_CARGO_SOURCE_TREE_SHA256,
                expected_emitter_source_sha256: pack::EMITTER_SOURCE_SHA256,
                externally_measured_emitter_source_sha256:
                    fn64_recomp_rs_codegen::generated_runner_emitter_source_receipt_v2()
                        .source_sha256(),
                expected_runtime_source_sha256: pack::RUNTIME_SOURCE_SHA256,
                runtime_source_receipt: fn64_recomp_rs::generated_runner_runtime_source_receipt_v1(),
                runners: &generated_runner_bindings,
            },
        )
        .expect("Cargo-source-attested block program has one admitted fixed entry");
    ConstructedCatalogProgram {
        catalog_program,
        generation_catalog,
        generation_backings,
        generated_runner_bindings,
        program_evidence,
    }
}

/// The exact ABI host-function catalog WM2000 requires, issued by fn64-abi.
///
/// These fifteen bindings are the discovered libultra entry points the block
/// lane services on the host side instead of executing guest code for. Both
/// lanes must issue the identical catalog: a shell that omitted one would
/// diverge from the certified batch run at exactly the call the omission
/// covers.
pub(crate) fn issue_wm2000_host_function_catalog() -> fn64_abi::recompiled::AbiHostFunctionCatalogV1
{
    use fn64_abi::recompiled::{AbiHostShimBindingV1 as Binding, AbiHostShimV1 as Shim};
    fn64_abi::recompiled::issue_abi_host_function_catalog_v1(vec![
        Binding {
            target_pc: pack::OS_SI_DEVICE_BUSY,
            shim: Shim::OsSiDeviceBusy,
        },
        Binding {
            target_pc: pack::OS_CREATE_MESG_QUEUE,
            shim: Shim::OsCreateMesgQueue,
        },
        Binding {
            target_pc: pack::OS_EPI_START_DMA,
            shim: Shim::OsEPiStartDma,
        },
        Binding {
            target_pc: pack::OS_RECV_MESG,
            shim: Shim::OsRecvMesg,
        },
        Binding {
            target_pc: pack::OS_SEND_MESG,
            shim: Shim::OsSendMesg,
        },
        Binding {
            target_pc: pack::OS_CREATE_THREAD,
            shim: Shim::OsCreateThread,
        },
        Binding {
            target_pc: pack::OS_SET_EVENT_MESG,
            shim: Shim::OsSetEventMesg,
        },
        Binding {
            target_pc: pack::OS_START_THREAD,
            shim: Shim::OsStartThread,
        },
        Binding {
            target_pc: pack::OS_GET_THREAD_PRI,
            shim: Shim::OsGetThreadPri,
        },
        Binding {
            target_pc: pack::OS_SET_THREAD_PRI,
            shim: Shim::OsSetThreadPri,
        },
        Binding {
            target_pc: pack::OS_SET_TIMER,
            shim: Shim::OsSetTimer,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_LOAD,
            shim: Shim::OsSpTaskLoad,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_START_GO,
            shim: Shim::OsSpTaskStartGo,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_YIELD,
            shim: Shim::OsSpTaskYield,
        },
        Binding {
            target_pc: pack::OS_SP_TASK_YIELDED,
            shim: Shim::OsSpTaskYielded,
        },
    ])
    .expect("ABI-issued host-function catalog is exact and unambiguous")
}
