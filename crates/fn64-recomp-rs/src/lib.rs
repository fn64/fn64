//! `fn64-recomp-rs`: the linked, all-Rust execution runtime for generated
//! N64 VR4300 (MIPS III) runners. The separate `fn64-recomp-rs-codegen` crate
//! owns decoding ROM inputs and emitting typed Rust; generated Cargo packages
//! depend on this crate for architectural state, memory, dispatch, and shared
//! semantic helpers.
//!
//! # Why
//!
//! The N64Recomp adapter emits **untyped C**: every memory access is a raw
//! pointer cast with a hand-written byte swizzle (`*(int16_t*)(rdram + ((reg +
//! off) ^ 2 - 0x…))`). That macro layer is the source of the byte-reinterpret
//! / offset / swizzle bug class this project has fought all session. The
//! runtime/codegen split makes that class *structurally impossible*: the
//! emitted Rust never casts a pointer, the swizzle lives in exactly one audited
//! place ([`runtime::Rdram`]), both crates forbid unsafe code, and every value
//! carries its Rust type.
//!
//! # Scope
//!
//! This crate supplies the decoder types shared with codegen, the typed
//! [`runtime`] generated functions execute against, arbitrary-PC execution,
//! executable-generation catalogs, and the experimental static micro-op
//! executor. Whole-ROM drivers, Rust source emission, and emitter-side source
//! receipts live in `fn64-recomp-rs-codegen`; they are intentionally absent
//! from a generated runner's normal runtime dependency graph.
//!
//! The byte-cited distinction between encoding coverage and full architectural
//! execution is maintained in `crates/fn64-recomp-rs/ISA-COVERAGE.md`.
//! Ordinary integer/control-flow/memory paths are covered; full COP1 floating
//! environment and privileged exception/MMU effects remain explicitly partial.
#![forbid(unsafe_code)]

#[cfg(all(feature = "production-aot", feature = "dev-interpreter"))]
compile_error!("fn64-recomp-rs: production-aot and dev-interpreter are mutually exclusive");

#[cfg(feature = "dev-interpreter")]
mod dev_interpreter_artifact;
#[cfg(feature = "dev-interpreter")]
#[doc(hidden)]
pub use dev_interpreter_artifact::DEV_INTERPRETER_ARTIFACT_MARKER;

/// Feature receipt emitted by a linked executable to attest which CPU lane
/// was compiled into this exact `fn64-recomp-rs` artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticExecutionBuildReceipt {
    pub schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dev_interpreter: bool,
}

/// Return the feature receipt compiled into this library artifact.
pub const fn static_execution_build_receipt() -> StaticExecutionBuildReceipt {
    StaticExecutionBuildReceipt {
        schema: 1,
        aot_runtime: cfg!(feature = "aot-runtime"),
        production_aot: cfg!(feature = "production-aot"),
        dev_interpreter: cfg!(feature = "dev-interpreter"),
    }
}

/// Schema-evolved feature receipt which distinguishes the narrow mapped
/// dynamic capability from the general development interpreter.
///
/// [`StaticExecutionBuildReceipt`] remains the immutable V1 API used by
/// existing attestations. Its lack of a `dynamic_mapped_runtime` field must
/// not be interpreted as evidence that the capability is absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticExecutionBuildReceiptV2 {
    pub schema: u32,
    pub aot_runtime: bool,
    pub production_aot: bool,
    pub dynamic_mapped_runtime: bool,
    pub dev_interpreter: bool,
}

/// Return the source artifact's complete feature-lane receipt.
pub const fn static_execution_build_receipt_v2() -> StaticExecutionBuildReceiptV2 {
    StaticExecutionBuildReceiptV2 {
        schema: 2,
        aot_runtime: cfg!(feature = "aot-runtime"),
        production_aot: cfg!(feature = "production-aot"),
        dynamic_mapped_runtime: cfg!(feature = "dynamic-mapped-runtime"),
        dev_interpreter: cfg!(feature = "dev-interpreter"),
    }
}

pub const DYNAMIC_MAPPED_EXECUTION_SOURCE_SCHEMA_V1: &str =
    "fn64.dynamic-mapped-execution-source.v1";

const DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1: &[(&str, &[u8])] = &[
    ("src/boot.rs", include_bytes!("boot.rs")),
    ("src/decoder.rs", include_bytes!("decoder.rs")),
    (
        "src/dev_interpreter_artifact.rs",
        include_bytes!("dev_interpreter_artifact.rs"),
    ),
    ("src/drive.rs", include_bytes!("drive.rs")),
    ("src/execution.rs", include_bytes!("execution.rs")),
    ("src/fallback.rs", include_bytes!("fallback.rs")),
    ("src/fetch.rs", include_bytes!("fetch.rs")),
    ("src/fpu.rs", include_bytes!("fpu.rs")),
    (
        "src/generated_support.rs",
        include_bytes!("generated_support.rs"),
    ),
    ("src/generation.rs", include_bytes!("generation.rs")),
    ("src/interp.rs", include_bytes!("interp.rs")),
    ("src/lib.rs", include_bytes!("lib.rs")),
    ("src/runtime.rs", include_bytes!("runtime.rs")),
    ("src/semantic.rs", include_bytes!("semantic.rs")),
    (
        "src/static_micro_op.rs",
        include_bytes!("static_micro_op.rs"),
    ),
    (
        "src/static_micro_op_exec.rs",
        include_bytes!("static_micro_op_exec.rs"),
    ),
];

/// Implementation-issued identity for the exact-unit mapped execution
/// capability. This receipt is deliberately separate from
/// [`StaticExecutionBuildReceipt`]: linking the capability does not turn an
/// operational dynamic run into static/release authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DynamicMappedExecutionBuildReceiptV1 {
    schema: &'static str,
    source_sha256: [u8; 32],
    available: bool,
    general_dev_interpreter: bool,
}

impl DynamicMappedExecutionBuildReceiptV1 {
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }

    pub const fn available(self) -> bool {
        self.available
    }

    pub const fn general_dev_interpreter(self) -> bool {
        self.general_dev_interpreter
    }
}

/// Bind dynamic unit identities conservatively to the manifest and every Rust
/// source file in this library crate.
pub fn dynamic_mapped_execution_build_receipt_v1() -> DynamicMappedExecutionBuildReceiptV1 {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(b"fn64:dynamic-mapped-execution-source:v1:");
    for (label, source) in
        std::iter::once(("Cargo.toml", include_bytes!("../Cargo.toml").as_slice()))
            .chain(DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1.iter().copied())
    {
        hasher.update(
            u64::try_from(label.len())
                .expect("dynamic mapped source label length fits u64")
                .to_be_bytes(),
        );
        hasher.update(label.as_bytes());
        hasher.update(
            u64::try_from(source.len())
                .expect("dynamic mapped source length fits u64")
                .to_be_bytes(),
        );
        hasher.update(source);
    }
    DynamicMappedExecutionBuildReceiptV1 {
        schema: DYNAMIC_MAPPED_EXECUTION_SOURCE_SCHEMA_V1,
        source_sha256: hasher.finalize().into(),
        available: cfg!(feature = "dynamic-mapped-runtime"),
        general_dev_interpreter: cfg!(feature = "dev-interpreter"),
    }
}

#[cfg(test)]
mod static_execution_build_receipt_tests {
    use super::{
        dynamic_mapped_execution_build_receipt_v1, static_execution_build_receipt,
        static_execution_build_receipt_v2, DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1,
    };
    use std::path::Path;

    fn collect_library_rust_sources(
        directory: &Path,
        manifest_dir: &Path,
        labels: &mut Vec<String>,
    ) {
        for entry in std::fs::read_dir(directory).expect("read library source directory") {
            let entry = entry.expect("read library source entry");
            let path = entry.path();
            let file_type = entry.file_type().expect("read library source file type");
            if file_type.is_dir() {
                collect_library_rust_sources(&path, manifest_dir, labels);
            } else if file_type.is_file()
                && path.extension().is_some_and(|extension| extension == "rs")
            {
                let relative = path
                    .strip_prefix(manifest_dir)
                    .expect("library source remains below crate manifest");
                labels.push(
                    relative
                        .components()
                        .map(|component| {
                            component
                                .as_os_str()
                                .to_str()
                                .expect("library source label is UTF-8")
                        })
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }

    #[test]
    fn receipt_matches_the_compiled_feature_lane() {
        let receipt = static_execution_build_receipt();
        assert_eq!(receipt.schema, 1);
        assert_eq!(receipt.aot_runtime, cfg!(feature = "aot-runtime"));
        assert_eq!(receipt.production_aot, cfg!(feature = "production-aot"));
        assert_eq!(receipt.dev_interpreter, cfg!(feature = "dev-interpreter"));
        assert!(!(receipt.production_aot && receipt.dev_interpreter));
        assert!(!receipt.production_aot || receipt.aot_runtime);
        assert!(!receipt.dev_interpreter || receipt.aot_runtime);

        let receipt_v2 = static_execution_build_receipt_v2();
        assert_eq!(receipt_v2.schema, 2);
        assert_eq!(receipt_v2.aot_runtime, cfg!(feature = "aot-runtime"));
        assert_eq!(receipt_v2.production_aot, cfg!(feature = "production-aot"));
        assert_eq!(
            receipt_v2.dynamic_mapped_runtime,
            cfg!(feature = "dynamic-mapped-runtime")
        );
        assert_eq!(
            receipt_v2.dev_interpreter,
            cfg!(feature = "dev-interpreter")
        );
        assert!(!(receipt_v2.production_aot && receipt_v2.dev_interpreter));
        assert!(!receipt_v2.production_aot || receipt_v2.aot_runtime);
        assert!(!receipt_v2.dynamic_mapped_runtime || receipt_v2.aot_runtime);
        assert!(!receipt_v2.dev_interpreter || receipt_v2.dynamic_mapped_runtime);
    }

    #[test]
    fn dynamic_receipt_is_source_bound_and_feature_exact() {
        let receipt = dynamic_mapped_execution_build_receipt_v1();
        assert_ne!(receipt.source_sha256(), [0; 32]);
        assert_eq!(
            receipt.available(),
            cfg!(feature = "dynamic-mapped-runtime")
        );
        assert_eq!(
            receipt.general_dev_interpreter(),
            cfg!(feature = "dev-interpreter")
        );
        assert!(!receipt.general_dev_interpreter() || receipt.available());
    }

    #[test]
    fn dynamic_receipt_source_inventory_covers_every_library_rust_source() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut actual = Vec::new();
        collect_library_rust_sources(&manifest_dir.join("src"), manifest_dir, &mut actual);
        actual.sort();

        let mut included = DYNAMIC_MAPPED_EXECUTION_LIBRARY_SOURCES_V1
            .iter()
            .map(|(label, _)| (*label).to_owned())
            .collect::<Vec<_>>();
        included.sort();

        assert_eq!(included, actual);
    }
}

pub mod boot;
pub mod decoder;
pub mod drive;
pub mod execution;
#[cfg(feature = "dev-interpreter")]
pub mod fallback;
pub mod fetch;
pub mod fpu;
pub mod generated_support;
pub mod generation;
#[cfg(feature = "dev-interpreter")]
pub mod interp;
#[cfg(all(not(feature = "dev-interpreter"), feature = "dynamic-mapped-runtime"))]
mod interp;
pub mod runtime;
mod semantic;
pub mod static_micro_op;
pub mod static_micro_op_exec;

pub use boot::{
    BootCicIdentity, BootContext, BootContextError, BootContextStateField,
    BootContextStateMismatch, BootCop0Context, BootRegion, BootTvStandard, Sha256Digest,
    BOOT_CONTEXT_SCHEMA_V1,
};
pub use decoder::{decode, Instruction};
pub use drive::ExecutorAction;
pub use execution::{
    catalog_resolver_policy_evidence_v1, dispatch_until_boundary, enter_pending_interrupt,
    finalize_executable_write_exit, generated_runner_runtime_source_receipt_v1,
    generated_runner_runtime_source_receipt_v2, post_straight_instruction_exit,
    verify_precompiled_image, verify_precompiled_instruction_word, AotMiss, BankError, BankId,
    BankWordKind, BlockExit, BlockProgram, BlockProgramEvidenceSnapshot, BlockRun, BlockRunner,
    CallResolution, CargoGeneratedProgramSourceAttestationV2, CargoGeneratedRunnerSourceBindingV1,
    CatalogBlockProgramErrorV1, CatalogBlockProgramV1, CatalogResolverPolicyEvidenceV1, CodeBank,
    CodeBankEvidenceSnapshot, CodeCatalog, CodeSpan, CodeSpanEvidenceSnapshot, CpuException,
    CpuFault, CpuFaultKind, CpuInterruptLine, DispatchError, DispatchRun, ExecutableRegion,
    ExecutionDestinationObservation, ExecutionKey, GeneratedAdapterRole, GeneratedBankFn,
    GeneratedBankRunner, GeneratedRunnerRuntimeSourceReceiptV1,
    GeneratedRunnerRuntimeSourceReceiptV2, GeneratedRunnerSourceAttestationErrorV1,
    GeneratedRunnerSourceAttestationV2, GenerationError, GuestPc, InstructionBudget,
    InstructionWordIdentity, ProgramArtifactIdentity, ProgramError,
    ProgramIdentityEvidenceSnapshot, ProgramIdentitySource, ResolvedInstruction, TransferResolver,
    CATALOG_RESOLVER_EXCEPTION_VECTORS_V1, CATALOG_RESOLVER_POLICY_NAME_V1,
    GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V1, GENERATED_RUNNER_RUNTIME_SOURCE_SCHEMA_V2,
    GENERATED_RUNNER_SOURCE_ATTESTATION_SCHEMA_V2, GENERATED_RUNNER_SOURCE_BINDING_DOMAIN_V2,
};
#[cfg(feature = "dev-interpreter")]
pub use fallback::{EvidenceClass, FallbackProgram, FallbackRunner};
#[cfg(feature = "dev-interpreter")]
pub use fetch::run_mapped_bank;
pub use fetch::{
    fetch_instruction, FetchedInstruction, InstructionFetchSite, MappedAotBlock, MappedAotError,
    MappedAotEvidenceSnapshot, PhysicalCodeBank, PhysicalCodeBankEvidenceSnapshot,
    PhysicalCodeCatalog, PhysicalCodeError, PhysicalCodeSpan, PhysicalCodeSpanEvidenceSnapshot,
};
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
pub use fetch::{
    DynamicMappedErrorV1, DynamicMappedRunV1, DynamicMappedUnitCatalogV1,
    DynamicMappedUnitIdentityV1,
};
pub use generation::{
    set_backed_generation_activation_observer_v1, ActiveGenerationSegment, BackedExecutableSpanV1,
    BackedGenerationActivationObservationV1, BackedGenerationActivationObserverV1,
    BackedGenerationCatalogErrorV1, BackedGenerationCatalogEvidenceV1,
    BackedPrecompiledGenerationCatalogV1, GenerationCatalogError, GenerationId,
    GenerationLookupError, GenerationResolution, InitialGenerationImageErrorV1,
    PhysicalInvalidationRangeV1, PrecompiledGeneration, PrecompiledGenerationBackingEvidenceV1,
    PrecompiledGenerationBackingV1, PrecompiledGenerationCatalog, PrecompiledGenerationEvidenceV1,
    PrecompiledShard, BACKED_GENERATION_CATALOG_EVIDENCE_SCHEMA_V1,
};
#[cfg(feature = "dev-interpreter")]
pub use interp::{run_bank, run_bank_with_mmio, MmioOutcome, MmioPort, NoMmio, UnsupportedOp};
pub use runtime::{
    call_host_or_recompiled, discard_executable_write_boundary, guest_write_token,
    notify_bootstrap_or_import_write, notify_cpu_instruction_store, notify_function_entry,
    notify_host_abi_write, notify_pi_dma_write, notify_rdp_renderer_write,
    notify_rsp_execution_or_hle_writeback, notify_si_dma_write, notify_sp_dma_write, pause_self,
    resolve_host_function, round_ties_even_f32, round_ties_even_f64, set_function_entry_observer,
    set_guest_write_boundary_observer, set_host_lookup, set_host_pause, set_mmio_hooks,
    set_read_observer, set_unsupported_observer, set_write_observer,
    take_executable_write_boundary, trap_unsupported, DataAccessError, DataAccessKind,
    FpuException, FunctionEntryObservationSchema, FunctionEntryObserver, GuestReadEvent,
    GuestWriteBoundary, GuestWriteBoundaryObserver, GuestWriteEvent, HostFunctionCatalogErrorV1,
    HostFunctionCatalogV1, HostLookup, HostPause, IndirectTransferObservation,
    InstructionTranslationDiagnosticErrorV1, MmioRead, MmioWrite, PhysicalFgrState, Rdram,
    ReadObserver, RecompContext, RecompContextEvidenceSnapshotV1, RecompFunc, TlbEntryRaw,
    TlbFault, TlbFaultKind, TranslatedDataAddress, TranslatedFunctionIdentity,
    TranslatedInstructionAddress, UnsupportedObserver, WriteObserver, WriterChannel,
    FUNCTION_ENTRY_OBSERVATION_SCHEMA, RDRAM_LEN, RDRAM_VBASE,
};
pub use static_micro_op::{
    static_micro_op_format_source_receipt_v1, static_micro_op_format_source_receipt_v2,
    StaticMicroOpFormatSourceReceiptV1, StaticMicroOpFormatSourceReceiptV2,
    StaticMicroOpRecordErrorV1, StaticMicroOpRecordV1, STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V1,
    STATIC_MICRO_OP_FORMAT_SOURCE_SCHEMA_V2, STATIC_MICRO_OP_HEADER_V1_BYTES,
    STATIC_MICRO_OP_MAGIC_V1, STATIC_MICRO_OP_MAGIC_V2,
    STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1, STATIC_MICRO_OP_PACK_SCHEMA_V1,
    STATIC_MICRO_OP_PACK_SCHEMA_V2, STATIC_MICRO_OP_RECORD_V1_BYTES,
    STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES, STATIC_MICRO_OP_SPAN_HEADER_V2_BYTES,
};
pub use static_micro_op_exec::{
    static_micro_op_execution_build_receipt_v1, static_micro_op_execution_build_receipt_v2,
    static_micro_op_execution_build_receipt_v3, static_micro_op_executor_source_receipt_v1,
    static_micro_op_executor_source_receipt_v2, static_micro_op_executor_source_receipt_v3,
    AdmittedStaticMicroOpProgramV1, AdmittedStaticMicroOpProgramV2,
    StaticMicroOpExecutionBuildReceiptV1, StaticMicroOpExecutionBuildReceiptV2,
    StaticMicroOpExecutionBuildReceiptV3, StaticMicroOpExecutorSourceReceiptV1,
    StaticMicroOpExecutorSourceReceiptV2, StaticMicroOpExecutorSourceReceiptV3,
    StaticMicroOpPackErrorV1, STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V1,
    STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V2, STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V3,
    STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V1, STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V2,
    STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V3,
};
