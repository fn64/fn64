use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogGenerationInstallEvidenceV1 {
    pub resolver: CatalogResolverInstallEvidenceV1,
    pub generations: BackedGenerationCatalogEvidenceV1,
    pub bootstrap: Option<BootstrapOrImportValidationEvidenceV1>,
    pub pending_physical_writes: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub mutation_journal: Option<CanonicalExecutableMutationJournalEvidenceV1>,
}

/// Canonical resolver plus the complete, physically backed precompiled image
/// inventory it is allowed to activate. Construction proves every shard bank
/// and unclaimed static span against the resolver's private program before the
/// pair can enter HostState.
pub struct CatalogGenerationInstallV1 {
    pub(super) resolver: CatalogResolverInstallV1,
    pub(super) generations: BackedPrecompiledGenerationCatalogV1,
}

impl CatalogGenerationInstallV1 {
    pub fn new(
        resolver: CatalogResolverInstallV1,
        generations: BackedPrecompiledGenerationCatalogV1,
    ) -> Result<Self, GenerationCatalogError> {
        resolver.validate_precompiled_generations(&generations)?;
        Ok(Self {
            resolver,
            generations,
        })
    }

    pub fn evidence_snapshot(&self) -> CatalogGenerationInstallEvidenceV1 {
        CatalogGenerationInstallEvidenceV1 {
            resolver: self.resolver.evidence().clone(),
            generations: self.generations.evidence_snapshot(),
            bootstrap: None,
            pending_physical_writes: Vec::new(),
            mutation_journal: None,
        }
    }

    /// Begin the only bootstrap/import transaction eligible to establish the
    /// canonical executable-memory baseline for this exact install.
    pub fn begin_bootstrap_import_v1<'a>(
        &'a self,
        rom: &'a [u8],
        rdram_len: usize,
        tv_type: fn64_runtime::TvType,
    ) -> Result<BootstrapImportTransactionV1<'a>, BootstrapImportErrorV1> {
        BootstrapImportTransactionV1::new(self, rom, rdram_len, tv_type)
    }
}

pub const BOOTSTRAP_IMPORT_VALIDATION_SCHEMA_V1: &str = "fn64.bootstrap-or-import-validation.v1";
pub const BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1: &str =
    "fn64.bootstrap-writer-channel-completion.v1";
pub const CPU_WRITER_RUNTIME_STATE_SCHEMA_V1: &str = "fn64.cpu-instruction-store-runtime-state.v1";
pub const PI_WRITER_RUNTIME_STATE_SCHEMA_V2: &str = "fn64.pi-writer-runtime-state.v2";
pub const SI_WRITER_RUNTIME_STATE_SCHEMA_V1: &str = "fn64.si-writer-runtime-state.v1";
pub const SP_WRITER_RUNTIME_STATE_SCHEMA_V1: &str = "fn64.sp-writer-runtime-state.v1";
pub const HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1: &str = "fn64.host-abi-writer-runtime-state.v1";
pub const RSP_WRITER_RUNTIME_STATE_SCHEMA_V1: &str =
    "fn64.rsp-execution-writeback-runtime-state.v1";
pub const RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1: &str =
    "fn64.rdp-renderer-writer-runtime-state.v1";
pub const CANONICAL_WRITER_PROGRAM_MODEL_SCHEMA_V1: &str = "fn64.canonical-writer-program-model.v1";
pub const CANONICAL_WRITER_PROGRAM_MODEL_SCHEMA_V2: &str = "fn64.canonical-writer-program-model.v2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapPublicationKindV1 {
    Ipl3CartridgeDma,
    ResidentRomImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapPublicationEvidenceV1 {
    pub kind: BootstrapPublicationKindV1,
    pub rom_start: u32,
    pub rom_end: u32,
    pub physical_start: u32,
    pub physical_end: u32,
    pub bytes_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapOrImportValidationEvidenceV1 {
    pub schema: String,
    pub rom_byte_len: u64,
    pub rom_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub generation_catalog_sha256: [u8; 32],
    pub initial_entry: ExecutionKey,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub watched_sha256: [u8; 32],
    pub initial_generations: Vec<GenerationId>,
    pub publications: Vec<BootstrapPublicationEvidenceV1>,
    pub receipt_sha256: [u8; 32],
}

/// Durable evidence behind the opaque bootstrap writer-channel authority.
///
/// The public fields make the retained claim auditable. They do not make it
/// constructible: only [`ValidatedBootstrapWriterChannelReceiptV1`] carries
/// authority, and that move-only wrapper has no public constructor or serde
/// implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapWriterChannelCompletionEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub bootstrap_receipt_sha256: [u8; 32],
    pub rom_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub generation_catalog_sha256: [u8; 32],
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub bootstrap_watched_sha256: [u8; 32],
    pub initial_generations: Vec<GenerationId>,
    pub journal_entry: ExecutableMutationBatchEvidenceV1,
    pub final_watched_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only authority that the exact canonical bootstrap/import publication
/// was reconciled into the executable mutation journal for one program model.
/// Plain evidence or a self-hash cannot manufacture this type.
#[derive(Debug)]
pub struct ValidatedBootstrapWriterChannelReceiptV1 {
    pub(super) evidence: BootstrapWriterChannelCompletionEvidenceV1,
}

impl ValidatedBootstrapWriterChannelReceiptV1 {
    pub fn evidence(&self) -> &BootstrapWriterChannelCompletionEvidenceV1 {
        &self.evidence
    }

    pub fn program_model_sha256(&self) -> [u8; 32] {
        self.evidence.program_model_sha256
    }

    pub fn evidence_sha256(&self) -> [u8; 32] {
        self.evidence.receipt_sha256
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256
            == bootstrap_writer_channel_completion_receipt_sha256(&self.evidence)
    }
}

/// Auditable evidence behind one fresh CPU instruction-store audit window.
///
/// This ABI-local projection binds the typed store observations to a
/// quiescent canonical executable-mutation owner. It has neither selected
/// generated-build authority nor writer-denominator completion authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub journal_entry_count: u64,
    /// Declarations are clipped to executable backing. Ordinary CPU data
    /// stores can exercise the typed path while this count remains zero.
    pub cpu_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub cpu_store_count: u64,
    pub cpu_store_trace_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one fresh, quiescent CPU-store window.
///
/// There is no public constructor, clone, or serialization implementation.
/// Copied evidence cannot recreate this authority.
#[derive(Debug)]
pub struct ValidatedCpuWriterRuntimeStateReceiptV1 {
    pub(super) evidence: CpuWriterRuntimeStateEvidenceV1,
}

impl ValidatedCpuWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &CpuWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == cpu_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// One unforgeable fresh CPU-store audit epoch minted by the canonical owner.
#[derive(Debug)]
pub struct CpuWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

/// Auditable evidence behind one fresh, quiescent PI-DMA audit window.
///
/// The retained transition digest covers the typed device lifecycle and at
/// least one cartridge/save-to-RDRAM byte commit. This ABI-local projection
/// has neither selected generated-build authority nor writer-denominator
/// completion authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub journal_entry_count: u64,
    /// Declarations are clipped to executable backing. A data-only PI DMA can
    /// exercise the typed path while this count remains zero.
    pub pi_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub pi_started: u64,
    pub pi_committed: u64,
    pub pi_busy_cleared: u64,
    pub pi_interrupt_raised: u64,
    pub pi_interrupt_cleared: u64,
    pub pi_notifications: u64,
    pub pi_to_rdram_committed: u64,
    pub pi_transition_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one fresh, completed PI-DMA audit window.
///
/// There is no public constructor, clone, or serialization implementation.
/// Copied evidence cannot recreate this authority.
#[derive(Debug)]
pub struct ValidatedPiWriterRuntimeStateReceiptV1 {
    pub(super) evidence: PiWriterRuntimeStateEvidenceV1,
}

impl ValidatedPiWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &PiWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == pi_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// One unforgeable fresh PI-DMA trace epoch minted by the canonical owner.
#[derive(Debug)]
pub struct PiWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

/// Auditable evidence behind the ABI-local SI runtime-state prerequisite.
///
/// This projection is deliberately not writer-channel completion authority.
/// It says that one canonical runtime was quiescent after a balanced, retained
/// SI transition sequence and that its private executable-mutation owner still
/// matched live RDRAM. It does not prove which separately compiled executable
/// supplied the generated runner bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub journal_entry_count: u64,
    /// Declarations are clipped to the sealed executable backing union. A
    /// controller-buffer PIF DMA outside that union therefore legitimately
    /// contributes zero here even though its typed SI transition is retained.
    pub si_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub si_started: u64,
    pub si_committed: u64,
    pub si_pif_to_dram_committed: u64,
    pub si_transition_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one quiescent SI runtime state.
///
/// There is no public constructor, clone, or serialization implementation.
/// A future build-owned runtime-series validator may consume its evidence;
/// the writer denominator must not accept this prerequisite directly.
#[derive(Debug)]
pub struct ValidatedSiWriterRuntimeStateReceiptV1 {
    pub(super) evidence: SiWriterRuntimeStateEvidenceV1,
}

impl ValidatedSiWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &SiWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == si_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// Auditable evidence behind the ABI-local SP-DMA runtime-state prerequisite.
///
/// This projection authenticates one fresh, quiescent raw SP-DMA epoch and
/// the canonical executable-mutation owner. Raw SP DMA has no OS notification
/// or MI interrupt: its terminal publication is `SpDmaBusyCleared`, while a
/// queued request is published by the immediately following `SpDmaStarted`.
/// Generated-build authority and writer-channel completion are intentionally
/// outside this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub journal_entry_count: u64,
    pub sp_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub sp_started: u64,
    pub sp_queued: u64,
    pub sp_committed: u64,
    pub sp_busy_cleared: u64,
    pub sp_rsp_to_rdram_committed: u64,
    pub sp_transition_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one quiescent, fresh SP-DMA runtime epoch.
///
/// There is no public constructor, clone, or serialization implementation,
/// and the writer denominator must not accept this prerequisite directly.
#[derive(Debug)]
pub struct ValidatedSpWriterRuntimeStateReceiptV1 {
    pub(super) evidence: SpWriterRuntimeStateEvidenceV1,
}

/// One unforgeable fresh-trace epoch owned by a canonical SP audit.
///
/// Construction clears retained device history and re-enables retention. The
/// token is move-only and its live epoch arm is consumed by successful
/// validation, so evidence from an older trace cannot be paired with a later
/// runtime state.
#[derive(Debug)]
pub struct SpWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

/// Auditable evidence behind one fresh canonical Host ABI writer window.
///
/// Only ABI-issued catalog targets participate. The lifecycle digest binds
/// every exact target/resume pair, per-thread LIFO transaction, ordering
/// boundary, and HostAbi journal sequence observed after the epoch was armed.
/// This ABI-local projection is not selected-build or denominator authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostAbiWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub initial_journal_entry_count: u64,
    pub final_journal_entry_count: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub host_abi_journal_entry_count: u64,
    pub host_abi_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub transactions_started: u64,
    pub transactions_finished: u64,
    pub ordering_boundaries: u64,
    pub lifecycle_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one fresh, completed Host ABI write window.
///
/// Copied evidence cannot recreate this authority, and compatibility
/// caller-supplied host pointers cannot enter its constructor.
#[derive(Debug)]
pub struct ValidatedHostAbiWriterRuntimeStateReceiptV1 {
    pub(super) evidence: HostAbiWriterRuntimeStateEvidenceV1,
}

impl ValidatedHostAbiWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &HostAbiWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == host_abi_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// One unforgeable fresh Host ABI transaction epoch minted by the canonical
/// executable-mutation owner.
#[derive(Debug)]
pub struct HostAbiWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

/// Auditable evidence behind one fresh ABI-owned RSP writeback window.
///
/// The task trace includes interpreter publications and successful translated
/// audio-HLE callbacks with exact owner generations. Rejected callback journal
/// entries fail closed instead of being absorbed by a later receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub journal_entry_count: u64,
    pub rsp_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub interpreter_writeback_count: u64,
    pub translated_audio_hle_publication_count: u64,
    pub writeback_range_count: u64,
    pub writeback_trace_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one fresh RSP writeback window.
#[derive(Debug)]
pub struct ValidatedRspWriterRuntimeStateReceiptV1 {
    pub(super) evidence: RspWriterRuntimeStateEvidenceV1,
}

impl ValidatedRspWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &RspWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == rsp_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// One unforgeable fresh RSP trace epoch minted by the canonical owner.
#[derive(Debug)]
pub struct RspWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

/// Auditable evidence behind one fresh ABI-owned renderer publication window.
///
/// A publication is recorded only after an HLE chunk has returned a committed
/// status or a fabric-owned raw-DPC transaction has committed. The journal
/// sequences bind any executable-byte changes from those publications to the
/// canonical watched image. This remains an ABI-local prerequisite: it is not
/// selected-build or writer-denominator authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdpRendererWriterRuntimeStateEvidenceV1 {
    pub schema: String,
    pub program_model_sha256: [u8; 32],
    pub resolver_install_sha256: [u8; 32],
    pub abi_host_catalog_receipt_sha256: [u8; 32],
    pub build_receipt: StaticExecutionBuildReceipt,
    pub trace_epoch_id: u64,
    pub initial_journal_entry_count: u64,
    pub final_journal_entry_count: u64,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub rdp_renderer_journal_entry_count: u64,
    pub rdp_renderer_journal_declaration_count: u64,
    pub journal_root_sha256: [u8; 32],
    pub final_watched_sha256: [u8; 32],
    pub renderer_publication_count: u64,
    pub publication_trace_sha256: [u8; 32],
    pub receipt_sha256: [u8; 32],
}

/// Move-only ABI-local proof of one fresh, quiescent renderer publication
/// window. Plain evidence and copied hashes cannot construct this authority.
#[derive(Debug)]
pub struct ValidatedRdpRendererWriterRuntimeStateReceiptV1 {
    pub(super) evidence: RdpRendererWriterRuntimeStateEvidenceV1,
}

impl ValidatedRdpRendererWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &RdpRendererWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256
            == rdp_renderer_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// One process-unique renderer trace epoch minted by the canonical owner.
#[derive(Debug)]
pub struct RdpRendererWriterRuntimeTraceEpochV1 {
    pub(super) epoch_id: u64,
    pub(super) program_model_sha256: [u8; 32],
}

impl ValidatedSpWriterRuntimeStateReceiptV1 {
    pub fn evidence(&self) -> &SpWriterRuntimeStateEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.receipt_sha256 == sp_writer_runtime_state_receipt_sha256(&self.evidence)
    }
}

/// Opaque receipt minted only by a successful bootstrap/import transaction.
pub struct BootstrapOrImportValidationReceiptV1 {
    evidence: BootstrapOrImportValidationEvidenceV1,
}

impl BootstrapOrImportValidationReceiptV1 {
    pub fn evidence(&self) -> &BootstrapOrImportValidationEvidenceV1 {
        &self.evidence
    }
}

/// Owned RDRAM whose executable baseline has been validated against one exact
/// ROM and canonical catalog install. No mutable slice or raw pointer escapes.
pub struct ValidatedBootstrapRdramV1 {
    pub(super) storage: crate::write_barrier::ProcessRdram,
    pub(super) receipt: BootstrapOrImportValidationReceiptV1,
}

impl ValidatedBootstrapRdramV1 {
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn receipt(&self) -> &BootstrapOrImportValidationReceiptV1 {
        &self.receipt
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapImportErrorV1 {
    RdramLength {
        actual: usize,
        minimum: usize,
    },
    RomRange {
        start: u32,
        end: u32,
        rom_len: usize,
    },
    RdramRange {
        start: u32,
        end: u32,
    },
    ConflictingPublication {
        existing_start: u32,
        existing_end: u32,
        requested_start: u32,
        requested_end: u32,
    },
    InitialEntryBankMissing {
        entry: ExecutionKey,
    },
    InitialEntryNotRdramBacked {
        entry: ExecutionKey,
    },
    InitialEntryImageMismatch {
        bank: BankId,
        pc: GuestPc,
        expected: u32,
        actual: u32,
    },
    StaticProgramImageMismatch {
        bank: BankId,
        pc: GuestPc,
        expected: u32,
        actual: u32,
    },
    PhysicalProgramImageMismatch {
        bank: BankId,
        physical_address: u32,
        expected: u32,
        actual: u32,
    },
    UnrecognizedInitialGenerationImage {
        physical_address: u32,
        actual: u8,
    },
    UnattributedWatchedByte {
        physical_address: u32,
    },
    ReceiptBindingMismatch {
        field: &'static str,
    },
    InstalledRomMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapWriterChannelCompletionErrorV1 {
    DynamicExecutionInstalled,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    UnexpectedTransactionCounters,
    MissingOrExtraJournalEntries { actual: usize },
    UnexpectedJournalEntry,
    CurrentWatchedBytesMismatch,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    PendingDeviceSi,
    PendingAbiSi,
    CurrentWatchedBytesMismatch,
    NoSiTransitions,
    InvalidSiTransitionOrder,
    NoPifToDramCommit,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    CurrentWatchedBytesMismatch,
    NoCpuStores,
    InvalidCpuStoreRange,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PiWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    PendingDevicePi,
    PendingAbiPi,
    PendingPiInterrupt,
    CurrentWatchedBytesMismatch,
    NoPiTransitions,
    InvalidPiTransitionOrder,
    NoToRdramCommit,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    PendingDeviceSpDma,
    PendingDeviceSpTask,
    PendingAbiSpWork,
    CurrentWatchedBytesMismatch,
    NoSpTransitions,
    InvalidSpTransitionOrder,
    NoRspToRdramCommit,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAbiWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    CurrentWatchedBytesMismatch,
    NoHostAbiTransactions,
    InvalidHostAbiLifecycle,
    NoHostAbiWriteCommit,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RspWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    PendingDeviceRspTask,
    PendingAbiRspWork,
    CurrentWatchedBytesMismatch,
    NoRspWritebacks,
    InvalidRspWritebackRange,
    InvalidRspHlePublication,
    RejectedRspExecutableMutation,
    ReceiptHashMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RdpRendererWriterRuntimeStateErrorV1 {
    NotValidatedOwnedBootstrap,
    MissingAbiHostCatalogAuthority,
    NonProductionAotBuild,
    DynamicExecutionInstalled,
    TraceEpochNotArmed,
    TraceEpochMismatch,
    Unsealed,
    Poisoned,
    PendingPhysicalWrites,
    PendingAttributedWrites,
    OpenHostTransactions,
    ActiveChildTransaction,
    PendingDeviceRspTask,
    PendingDeviceDpcTransaction,
    PendingDeviceDpCompletion,
    PendingAbiRendererWork,
    CurrentWatchedBytesMismatch,
    NoRendererPublications,
    InvalidRendererPublicationTrace,
    ReceiptHashMismatch,
}

impl std::fmt::Display for BootstrapWriterChannelCompletionErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid bootstrap writer-channel completion: {self:?}"
        )
    }
}

impl std::error::Error for BootstrapWriterChannelCompletionErrorV1 {}

impl std::fmt::Display for SiWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid SI writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for SiWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for CpuWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid CPU instruction-store runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for CpuWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for PiWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid PI writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for PiWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for SpWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid SP writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for SpWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for HostAbiWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid Host ABI writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for HostAbiWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for RspWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid RSP writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for RspWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for RdpRendererWriterRuntimeStateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid RDP renderer writer runtime-state prerequisite: {self:?}"
        )
    }
}

impl std::error::Error for RdpRendererWriterRuntimeStateErrorV1 {}

impl std::fmt::Display for BootstrapImportErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid bootstrap/import transaction: {self:?}")
    }
}

impl std::error::Error for BootstrapImportErrorV1 {}

/// Pre-boot owner of the process allocation. Publication is restricted to
/// exact ROM slices; completion consumes the transaction and seals storage.
pub struct BootstrapImportTransactionV1<'a> {
    install: &'a CatalogGenerationInstallV1,
    rom: &'a [u8],
    rom_sha256: [u8; 32],
    /// The process allocation, page-aligned when the host granted one.
    ///
    /// Allocated in its final form here rather than converted later: the
    /// transaction publishes ROM slices straight into these bytes and
    /// `commit` moves them into `ValidatedBootstrapRdramV1` unchanged, so
    /// allocating a `Box<[u8]>` and re-homing it would mean copying 8 MiB and
    /// would leave the published bytes momentarily in two places.
    pub(super) storage: crate::write_barrier::ProcessRdram,
    publications: Vec<BootstrapPublicationEvidenceV1>,
}

impl<'a> BootstrapImportTransactionV1<'a> {
    fn new(
        install: &'a CatalogGenerationInstallV1,
        rom: &'a [u8],
        rdram_len: usize,
        tv_type: fn64_runtime::TvType,
    ) -> Result<Self, BootstrapImportErrorV1> {
        // The canonical block lane routes raw guest MMIO through typed hooks.
        // The 0x2900_0000-byte sparse mirror exists only for generated C,
        // whose pointer macros cannot be intercepted; carrying it into this
        // owned all-Rust lane would waste roughly 648 MiB per process.
        let minimum = fn64_cpu_runtime::RDRAM_LEN;
        if rdram_len < minimum {
            return Err(BootstrapImportErrorV1::RdramLength {
                actual: rdram_len,
                minimum,
            });
        }
        // Page-aligned when the host grants the mapping, heap otherwise. Both
        // are zero-filled -- anonymous `mmap` is guaranteed zeroed, which is
        // what the `vec![0; rdram_len]` this replaced relied on -- so the
        // bytes the transaction starts from are identical either way.
        let mut storage = crate::write_barrier::ProcessRdram::new(rdram_len);
        fn64_runtime::IplBootGlobals::cold(tv_type).install(&mut storage);
        Ok(Self {
            install,
            rom,
            rom_sha256: sha2::Sha256::digest(rom).into(),
            storage,
            publications: Vec::new(),
        })
    }

    pub fn publish_ipl3_cartridge_dma(&mut self) -> Result<(), BootstrapImportErrorV1> {
        self.publish_rom_slice(
            BootstrapPublicationKindV1::Ipl3CartridgeDma,
            0x1000,
            0x8000_0400,
            0x10_0000,
        )
    }

    pub fn publish_resident_rom_image(
        &mut self,
        rom_start: u32,
        ram_address: u32,
        byte_len: u32,
    ) -> Result<(), BootstrapImportErrorV1> {
        self.publish_rom_slice(
            BootstrapPublicationKindV1::ResidentRomImage,
            rom_start,
            ram_address,
            byte_len,
        )
    }

    fn publish_rom_slice(
        &mut self,
        kind: BootstrapPublicationKindV1,
        rom_start: u32,
        ram_address: u32,
        byte_len: u32,
    ) -> Result<(), BootstrapImportErrorV1> {
        let rom_end = rom_start
            .checked_add(byte_len)
            .ok_or(BootstrapImportErrorV1::RomRange {
                start: rom_start,
                end: u32::MAX,
                rom_len: self.rom.len(),
            })?;
        if byte_len == 0 || rom_end as usize > self.rom.len() {
            return Err(BootstrapImportErrorV1::RomRange {
                start: rom_start,
                end: rom_end,
                rom_len: self.rom.len(),
            });
        }
        let physical_start = direct_rdram_physical_address(ram_address).ok_or(
            BootstrapImportErrorV1::RdramRange {
                start: ram_address,
                end: ram_address.saturating_add(byte_len),
            },
        )?;
        let physical_end =
            physical_start
                .checked_add(byte_len)
                .ok_or(BootstrapImportErrorV1::RdramRange {
                    start: physical_start,
                    end: u32::MAX,
                })?;
        if physical_end > fn64_cpu_runtime::RDRAM_LEN as u32 {
            return Err(BootstrapImportErrorV1::RdramRange {
                start: physical_start,
                end: physical_end,
            });
        }
        let bytes = &self.rom[rom_start as usize..rom_end as usize];
        let bytes_sha256 = sha2::Sha256::digest(bytes).into();
        if let Some(existing) = self.publications.iter().find(|existing| {
            physical_start < existing.physical_end && physical_end > existing.physical_start
        }) {
            if (
                existing.rom_start,
                existing.rom_end,
                existing.physical_start,
                existing.physical_end,
                existing.bytes_sha256,
            ) == (
                rom_start,
                rom_end,
                physical_start,
                physical_end,
                bytes_sha256,
            ) {
                return Ok(());
            }
            return Err(BootstrapImportErrorV1::ConflictingPublication {
                existing_start: existing.physical_start,
                existing_end: existing.physical_end,
                requested_start: physical_start,
                requested_end: physical_end,
            });
        }
        fn64_runtime::RdramViewMut::from_storage(&mut self.storage)
            .write_logical_bytes(fn64_runtime::RdramAddr::from_offset(physical_start), bytes);
        self.publications.push(BootstrapPublicationEvidenceV1 {
            kind,
            rom_start,
            rom_end,
            physical_start,
            physical_end,
            bytes_sha256,
        });
        Ok(())
    }

    pub fn commit(mut self) -> Result<ValidatedBootstrapRdramV1, BootstrapImportErrorV1> {
        self.publications.sort_by_key(|publication| {
            (
                publication.physical_start,
                publication.physical_end,
                publication.rom_start,
                publication.rom_end,
            )
        });
        let watched_ranges = executable_physical_ranges(self.install);
        validate_initial_entry_image(self.install, &self.storage)?;
        let view = fn64_runtime::RdramView::from_storage(&self.storage);
        let initial_generations = self
            .install
            .generations
            .validate_initial_physical_images(|physical| {
                view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            })
            .map_err(|error| match error {
                fn64_cpu_runtime::InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
                    physical_address,
                    actual,
                } => BootstrapImportErrorV1::UnrecognizedInitialGenerationImage {
                    physical_address,
                    actual,
                },
            })?;
        for &(physical_start, physical_end) in &watched_ranges {
            for physical_address in physical_start..physical_end {
                let byte = view.read_u8(fn64_runtime::RdramAddr::from_offset(physical_address));
                if byte != 0
                    && !self.publications.iter().any(|publication| {
                        publication.physical_start <= physical_address
                            && physical_address < publication.physical_end
                    })
                {
                    return Err(BootstrapImportErrorV1::UnattributedWatchedByte {
                        physical_address,
                    });
                }
            }
        }
        let watched_sha256 = watched_bytes_sha256(&self.storage, &watched_ranges);
        let resolver_install_sha256 = resolver_install_definition_sha256(&self.install.resolver);
        let generation_catalog_sha256 = self.install.generations.canonical_definition_sha256();
        let watched_ranges = watched_ranges
            .into_iter()
            .map(
                |(physical_start, physical_end)| PendingExecutableWriteEvidenceSnapshot {
                    physical_start,
                    physical_end,
                },
            )
            .collect::<Vec<_>>();
        let mut evidence = BootstrapOrImportValidationEvidenceV1 {
            schema: BOOTSTRAP_IMPORT_VALIDATION_SCHEMA_V1.to_string(),
            rom_byte_len: u64::try_from(self.rom.len())
                .expect("bootstrap ROM length exceeds receipt wire"),
            rom_sha256: self.rom_sha256,
            resolver_install_sha256,
            generation_catalog_sha256,
            initial_entry: self.install.resolver.entry(),
            watched_ranges,
            watched_sha256,
            initial_generations,
            publications: self.publications,
            receipt_sha256: [0; 32],
        };
        evidence.receipt_sha256 = bootstrap_receipt_sha256(&evidence);
        Ok(ValidatedBootstrapRdramV1 {
            storage: self.storage,
            receipt: BootstrapOrImportValidationReceiptV1 { evidence },
        })
    }
}

fn direct_rdram_physical_address(address: u32) -> Option<u32> {
    let physical = address & 0x1fff_ffff;
    ((0x8000_0000..0xc000_0000).contains(&address) && physical < fn64_cpu_runtime::RDRAM_LEN as u32)
        .then_some(physical)
}

pub(super) fn executable_physical_ranges(install: &CatalogGenerationInstallV1) -> Vec<(u32, u32)> {
    executable_physical_ranges_for_parts(&install.resolver, Some(&install.generations))
}

pub(super) fn executable_physical_ranges_for_parts(
    resolver: &CatalogResolverInstallV1,
    generations: Option<&BackedPrecompiledGenerationCatalogV1>,
) -> Vec<(u32, u32)> {
    let mut ranges = resolver
        .program_evidence()
        .banks
        .iter()
        .flat_map(|bank| &bank.spans)
        .filter_map(|span| {
            let start = direct_rdram_physical_address(span.vram_start.get())?;
            let byte_len = u32::try_from(span.words.len().checked_mul(4)?).ok()?;
            Some((start, start.checked_add(byte_len)?))
        })
        .chain(
            resolver
                .program_evidence()
                .physical_banks
                .iter()
                .flat_map(|bank| &bank.spans)
                .filter_map(|span| {
                    let byte_len = u32::try_from(span.words.len().checked_mul(4)?).ok()?;
                    Some((
                        span.physical_start,
                        span.physical_start.checked_add(byte_len)?,
                    ))
                }),
        )
        .collect::<Vec<_>>();
    if let Some(generations) = generations {
        ranges.extend(
            generations
                .physical_invalidation_ranges()
                .into_iter()
                .map(|range| (range.physical_start(), range.physical_end())),
        );
    }
    ranges.sort_unstable();
    let mut canonical: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(previous) = canonical.last_mut() {
            if start <= previous.1 {
                previous.1 = previous.1.max(end);
                continue;
            }
        }
        canonical.push((start, end));
    }
    canonical
}

pub(super) fn validate_initial_entry_image(
    install: &CatalogGenerationInstallV1,
    storage: &[u8],
) -> Result<(), BootstrapImportErrorV1> {
    let entry = install.resolver.entry();
    let bank = install
        .resolver
        .program_evidence()
        .banks
        .iter()
        .find(|bank| bank.id == entry.bank)
        .ok_or(BootstrapImportErrorV1::InitialEntryBankMissing { entry })?;
    let view = fn64_runtime::RdramView::from_storage(storage);
    let entry_backed = bank.spans.iter().any(|span| {
        let span_end = span.vram_start.get() + u32::try_from(span.words.len() * 4).unwrap();
        direct_rdram_physical_address(span.vram_start.get()).is_some()
            && span.vram_start.get() <= entry.pc.get()
            && entry.pc.get() < span_end
    });
    if !entry_backed {
        return Err(BootstrapImportErrorV1::InitialEntryNotRdramBacked { entry });
    }

    // Generation shard banks are mutually exclusive immutable alternatives.
    // Their live physical image is validated by the generation digest, not by
    // comparing one backing against every alternative bank. Every unreserved
    // direct-RDRAM bank, however, is initially resident static code and must
    // agree word-for-word before the bootstrap receipt can seal it.
    for static_bank in install
        .resolver
        .program_evidence()
        .banks
        .iter()
        .filter(|bank| !install.generations.contains_reserved_bank(bank.id))
    {
        for span in &static_bank.spans {
            let Some(physical_start) = direct_rdram_physical_address(span.vram_start.get()) else {
                continue;
            };
            for (index, expected) in span.words.iter().copied().enumerate() {
                let byte_offset = u32::try_from(index * 4).unwrap();
                let physical = physical_start + byte_offset;
                let actual = view.read_u32(fn64_runtime::RdramAddr::from_offset(physical));
                if actual != expected {
                    let pc = GuestPc::new(span.vram_start.get() + byte_offset);
                    return Err(if static_bank.id == entry.bank {
                        BootstrapImportErrorV1::InitialEntryImageMismatch {
                            bank: static_bank.id,
                            pc,
                            expected,
                            actual,
                        }
                    } else {
                        BootstrapImportErrorV1::StaticProgramImageMismatch {
                            bank: static_bank.id,
                            pc,
                            expected,
                            actual,
                        }
                    });
                }
            }
        }
    }

    // Physical instruction catalogs are independently executable authority;
    // validating only their virtual aliases would leave a second admitted
    // fetch image outside the sealed bootstrap proof.
    for physical_bank in install
        .resolver
        .program_evidence()
        .physical_banks
        .iter()
        .filter(|bank| !install.generations.contains_reserved_bank(bank.id))
    {
        for span in &physical_bank.spans {
            for (index, expected) in span.words.iter().copied().enumerate() {
                let physical_address = span.physical_start + u32::try_from(index * 4).unwrap();
                let actual = view.read_u32(fn64_runtime::RdramAddr::from_offset(physical_address));
                if actual != expected {
                    return Err(BootstrapImportErrorV1::PhysicalProgramImageMismatch {
                        bank: physical_bank.id,
                        physical_address,
                        expected,
                        actual,
                    });
                }
            }
        }
    }
    Ok(())
}

pub(super) fn watched_bytes_sha256(storage: &[u8], ranges: &[(u32, u32)]) -> [u8; 32] {
    let view = fn64_runtime::RdramView::from_storage(storage);
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:bootstrap-watched-bytes:v1");
    for &(start, end) in ranges {
        hasher.update(start.to_be_bytes());
        hasher.update(end.to_be_bytes());
        for physical in start..end {
            hasher.update([view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))]);
        }
    }
    hasher.finalize().into()
}

/// The v2 digest of one page of one watched range.
///
/// SUPERSEDED by [`watched_page_digest_v3`]. Retained, and reachable only from
/// tests, as the reference construction the version-distinguishability tests
/// hash against: a v2 value and a v3 value over identical memory must differ,
/// and that is checked rather than asserted.
///
/// Binds the schema, the page size, the range the page belongs to, the page's
/// index within that range, and its bytes. Because the range bounds and the
/// index are inside the leaf, a page's digest is not reusable at any other
/// position -- two ranges that happen to hold identical bytes still produce
/// different leaves, and so does the same page moved within a range.
///
/// The final page of a range may be shorter than [`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2`];
/// its actual length is hashed, so a short final page cannot be confused with a
/// full one that happens to be zero-padded.
#[cfg(test)]
pub(super) fn watched_page_digest_v2(
    physical_start: u32,
    physical_end: u32,
    page_index: u32,
    bytes: &[u8],
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V2.as_bytes());
    hasher.update([0x00]); // leaf tag: distinguishes a page from the root below
    hasher.update((CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64).to_be_bytes());
    hasher.update(physical_start.to_be_bytes());
    hasher.update(physical_end.to_be_bytes());
    hasher.update(page_index.to_be_bytes());
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

/// The v2 root over every page of every watched range.
///
/// SUPERSEDED by [`watched_root_digest_v3`], which replaces this flat absorb of
/// every page digest with a Merkle tree. Retained, test-only, as the reference
/// the version-distinguishability tests compare against.
///
/// `ranges` yields `(physical_start, physical_end, page_digests)` in watched
/// order. The root depends ONLY on that -- on the range bounds and the page
/// digests, in range order and page order. It does not depend on which pages
/// were recomputed, in what order, or on how many commits preceded, which is
/// what makes an incrementally maintained root equal to one computed from
/// scratch.
///
/// The range count and each range's page count are hashed, so no regrouping of
/// pages between ranges can produce the same root.
#[cfg(test)]
pub(super) fn watched_root_digest_v2<'a>(
    ranges: impl ExactSizeIterator<Item = (u32, u32, &'a [[u8; 32]])>,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V2.as_bytes());
    hasher.update([0x01]); // root tag
    hasher.update((CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64).to_be_bytes());
    hasher.update((ranges.len() as u64).to_be_bytes());
    for (physical_start, physical_end, pages) in ranges {
        hasher.update(physical_start.to_be_bytes());
        hasher.update(physical_end.to_be_bytes());
        hasher.update((pages.len() as u64).to_be_bytes());
        for page in pages {
            hasher.update(page);
        }
    }
    hasher.finalize().into()
}

/// The v3 digest of one page of one watched range.
///
/// Identical in shape to [`watched_page_digest_v2`] -- same bound fields, same
/// order -- but under the v3 schema prefix, so a v2 leaf and a v3 leaf over the
/// same page are different values. The leaf tag stays `0x00`; the schema string
/// is what separates the versions, and it is the first thing hashed.
///
/// The final page of a range may be shorter than
/// [`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2`]; its actual length is hashed, so a
/// short final page cannot be confused with a zero-padded full one.
pub(super) fn watched_page_digest_v3(
    physical_start: u32,
    physical_end: u32,
    page_index: u32,
    bytes: &[u8],
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3.as_bytes());
    hasher.update([0x00]); // leaf tag
    hasher.update((CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64).to_be_bytes());
    hasher.update(physical_start.to_be_bytes());
    hasher.update(physical_end.to_be_bytes());
    hasher.update(page_index.to_be_bytes());
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    hasher.finalize().into()
}

/// One internal node of a v3 per-range Merkle tree.
///
/// `height` is 1 for the parent of two leaves and increases toward the range
/// root; `index` is the node's position within its level, counting from zero.
/// Both are bound, so a node cannot be replayed at another position or another
/// level -- with a fixed fanout of two and a bound leaf count, that pins the
/// whole shape.
///
/// `right` is `None` for the last node of an odd level. That case hashes a
/// distinct tag rather than duplicating the left child, because duplication is
/// the classic Merkle malleability: with `H(x||x)` a tree of `2n` leaves whose
/// second half repeats the first can be confused with a tree of `n`. Here the
/// arities are different messages outright.
pub(super) fn watched_node_digest_v3(
    physical_start: u32,
    physical_end: u32,
    height: u32,
    index: u32,
    left: &[u8; 32],
    right: Option<&[u8; 32]>,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3.as_bytes());
    // Distinct tags for the two arities: a promoted single child is not the
    // same message as a pair, so no regrouping of levels can collide.
    hasher.update([if right.is_some() { 0x02 } else { 0x03 }]);
    hasher.update((CANONICAL_WATCHED_BYTES_FANOUT_V3 as u64).to_be_bytes());
    hasher.update(physical_start.to_be_bytes());
    hasher.update(physical_end.to_be_bytes());
    hasher.update(height.to_be_bytes());
    hasher.update(index.to_be_bytes());
    hasher.update(left);
    if let Some(right) = right {
        hasher.update(right);
    }
    hasher.finalize().into()
}

/// The v3 root of one watched range's page tree.
///
/// Binds the range bounds, the page size, the fanout and the page COUNT around
/// the tree's apex, so two ranges cannot be regrouped into one, and a tree
/// cannot be reinterpreted with a different leaf count. `apex` is the single
/// surviving node after the levels collapse; an empty range (no pages) has no
/// apex and hashes the absence explicitly.
pub(super) fn watched_range_root_digest_v3(
    physical_start: u32,
    physical_end: u32,
    page_count: u64,
    apex: Option<&[u8; 32]>,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3.as_bytes());
    hasher.update([0x04]); // range-root tag
    hasher.update((CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64).to_be_bytes());
    hasher.update((CANONICAL_WATCHED_BYTES_FANOUT_V3 as u64).to_be_bytes());
    hasher.update(physical_start.to_be_bytes());
    hasher.update(physical_end.to_be_bytes());
    hasher.update(page_count.to_be_bytes());
    match apex {
        Some(apex) => {
            hasher.update([0x01]);
            hasher.update(apex);
        }
        None => hasher.update([0x00]),
    }
    hasher.finalize().into()
}

/// The v3 top root over every watched range's range root.
///
/// This is the value that replaces the v2 flat root. It hashes 32 bytes per
/// RANGE -- 64 bytes on WM2000 -- instead of 32 bytes per page, which is what
/// makes a commit cost `O(log pages)` rather than `O(pages)`.
///
/// The range count is bound, and each range root already binds its own bounds
/// and page count, so no redistribution of pages between ranges reaches the
/// same top root.
pub(super) fn watched_root_digest_v3<'a>(
    range_roots: impl ExactSizeIterator<Item = &'a [u8; 32]>,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3.as_bytes());
    hasher.update([0x05]); // top-root tag
    hasher.update((CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2 as u64).to_be_bytes());
    hasher.update((CANONICAL_WATCHED_BYTES_FANOUT_V3 as u64).to_be_bytes());
    hasher.update((range_roots.len() as u64).to_be_bytes());
    for root in range_roots {
        hasher.update(root);
    }
    hasher.finalize().into()
}

pub(super) fn resolver_install_definition_sha256(install: &CatalogResolverInstallV1) -> [u8; 32] {
    let evidence = install.evidence();
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:catalog-resolver-install-definition:v2");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_identity.identity.bytes());
    hasher.update([match evidence.program_identity.source {
        ProgramIdentitySource::CallerSupplied => 0,
        ProgramIdentitySource::CanonicalBlockProgramSha256 => 1,
    }]);
    hasher.update(evidence.entry.bank.get().to_be_bytes());
    hasher.update(evidence.entry.pc.get().to_be_bytes());
    hasher.update(evidence.instruction_budget.to_be_bytes());
    hasher.update((evidence.host_target_pcs.len() as u64).to_be_bytes());
    for target in &evidence.host_target_pcs {
        hasher.update(target.to_be_bytes());
    }
    match &evidence.abi_host_catalog {
        Some(catalog) => {
            hasher.update([1]);
            hasher.update(catalog.receipt_sha256);
        }
        None => hasher.update([0]),
    }
    hasher.update(evidence.dispatch_artifact_identity.bytes());
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.finalize().into()
}

pub(super) fn abi_host_function_catalog_receipt_sha256(
    evidence: &AbiHostFunctionCatalogEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:abi-host-function-catalog-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update((evidence.bindings.len() as u64).to_be_bytes());
    for binding in &evidence.bindings {
        hasher.update(binding.target_pc.to_be_bytes());
        hasher.update([binding.shim as u8]);
        hasher.update((binding.writer_effects.len() as u64).to_be_bytes());
        for channel in &binding.writer_effects {
            hasher.update([*channel as u8]);
        }
    }
    hasher.finalize().into()
}

pub(super) fn bootstrap_receipt_sha256(
    evidence: &BootstrapOrImportValidationEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:bootstrap-or-import-validation-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.rom_byte_len.to_be_bytes());
    hasher.update(evidence.rom_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.generation_catalog_sha256);
    hasher.update(evidence.initial_entry.bank.get().to_be_bytes());
    hasher.update(evidence.initial_entry.pc.get().to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.watched_sha256);
    hasher.update((evidence.initial_generations.len() as u64).to_be_bytes());
    for generation in &evidence.initial_generations {
        hasher.update(generation.get().to_be_bytes());
    }
    hasher.update((evidence.publications.len() as u64).to_be_bytes());
    for publication in &evidence.publications {
        hasher.update([match publication.kind {
            BootstrapPublicationKindV1::Ipl3CartridgeDma => 0,
            BootstrapPublicationKindV1::ResidentRomImage => 1,
        }]);
        hasher.update(publication.rom_start.to_be_bytes());
        hasher.update(publication.rom_end.to_be_bytes());
        hasher.update(publication.physical_start.to_be_bytes());
        hasher.update(publication.physical_end.to_be_bytes());
        hasher.update(publication.bytes_sha256);
    }
    hasher.finalize().into()
}

pub(super) fn canonical_mutation_initial_root(
    expected_sha256: [u8; 32],
    ranges: impl IntoIterator<Item = PendingExecutableWriteEvidenceSnapshot>,
) -> [u8; 32] {
    let mut root = sha2::Sha256::new();
    root.update(CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1.as_bytes());
    root.update(expected_sha256);
    for range in ranges {
        root.update(range.physical_start.to_be_bytes());
        root.update(range.physical_end.to_be_bytes());
    }
    root.finalize().into()
}

pub(super) fn canonical_mutation_entry_root(
    previous_root: [u8; 32],
    entry: &ExecutableMutationBatchEvidenceV1,
) -> [u8; 32] {
    let mut root = sha2::Sha256::new();
    root.update(previous_root);
    root.update(entry.sequence.to_be_bytes());
    root.update(entry.before_sha256);
    root.update(entry.after_sha256);
    for declaration in &entry.declared_writes {
        root.update([declaration.channel as u8]);
        root.update(declaration.physical_start.to_be_bytes());
        root.update(declaration.physical_end.to_be_bytes());
    }
    for range in &entry.changed_ranges {
        root.update(range.physical_start.to_be_bytes());
        root.update(range.physical_end.to_be_bytes());
    }
    for generation in &entry.invalidated_generations {
        root.update(generation.get().to_be_bytes());
    }
    root.finalize().into()
}

pub(super) fn canonical_writer_program_model_sha256(
    resolver: &CatalogResolverInstallV1,
    generations: Option<&BackedPrecompiledGenerationCatalogV1>,
    watched_ranges: &[PendingExecutableWriteEvidenceSnapshot],
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(CANONICAL_WRITER_PROGRAM_MODEL_SCHEMA_V2.as_bytes());
    // The resolver definition begins with the canonical BlockProgram identity,
    // which itself binds every code word and generated-runner artifact.
    hasher.update(resolver_install_definition_sha256(resolver));
    match generations {
        Some(generations) => {
            hasher.update([1]);
            hasher.update(generations.canonical_definition_sha256());
        }
        None => hasher.update([0]),
    }
    hasher.update((watched_ranges.len() as u64).to_be_bytes());
    for range in watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.finalize().into()
}

pub(super) fn bootstrap_writer_channel_completion_receipt_sha256(
    evidence: &BootstrapWriterChannelCompletionEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:bootstrap-writer-channel-completion-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.bootstrap_receipt_sha256);
    hasher.update(evidence.rom_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.generation_catalog_sha256);
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.bootstrap_watched_sha256);
    hasher.update((evidence.initial_generations.len() as u64).to_be_bytes());
    for generation in &evidence.initial_generations {
        hasher.update(generation.get().to_be_bytes());
    }
    let entry = &evidence.journal_entry;
    hasher.update(entry.sequence.to_be_bytes());
    hasher.update((entry.declared_writes.len() as u64).to_be_bytes());
    for declaration in &entry.declared_writes {
        hasher.update([declaration.channel as u8]);
        hasher.update(declaration.physical_start.to_be_bytes());
        hasher.update(declaration.physical_end.to_be_bytes());
    }
    hasher.update((entry.changed_ranges.len() as u64).to_be_bytes());
    for range in &entry.changed_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(entry.before_sha256);
    hasher.update(entry.after_sha256);
    hasher.update((entry.invalidated_generations.len() as u64).to_be_bytes());
    for generation in &entry.invalidated_generations {
        hasher.update(generation.get().to_be_bytes());
    }
    hasher.update(entry.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.finalize().into()
}

pub(super) fn si_writer_runtime_state_receipt_sha256(
    evidence: &SiWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:si-writer-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.journal_entry_count.to_be_bytes());
    hasher.update(evidence.si_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.si_started.to_be_bytes());
    hasher.update(evidence.si_committed.to_be_bytes());
    hasher.update(evidence.si_pif_to_dram_committed.to_be_bytes());
    hasher.update(evidence.si_transition_sha256);
    hasher.finalize().into()
}

pub(super) fn cpu_writer_runtime_state_receipt_sha256(
    evidence: &CpuWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:cpu-instruction-store-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.journal_entry_count.to_be_bytes());
    hasher.update(evidence.cpu_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.cpu_store_count.to_be_bytes());
    hasher.update(evidence.cpu_store_trace_sha256);
    hasher.finalize().into()
}

pub(super) fn pi_writer_runtime_state_receipt_sha256(
    evidence: &PiWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-state-receipt:v2");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.journal_entry_count.to_be_bytes());
    hasher.update(evidence.pi_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.pi_started.to_be_bytes());
    hasher.update(evidence.pi_committed.to_be_bytes());
    hasher.update(evidence.pi_busy_cleared.to_be_bytes());
    hasher.update(evidence.pi_interrupt_raised.to_be_bytes());
    hasher.update(evidence.pi_interrupt_cleared.to_be_bytes());
    hasher.update(evidence.pi_notifications.to_be_bytes());
    hasher.update(evidence.pi_to_rdram_committed.to_be_bytes());
    hasher.update(evidence.pi_transition_sha256);
    hasher.finalize().into()
}

pub(super) fn sp_writer_runtime_state_receipt_sha256(
    evidence: &SpWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:sp-writer-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.journal_entry_count.to_be_bytes());
    hasher.update(evidence.sp_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.sp_started.to_be_bytes());
    hasher.update(evidence.sp_queued.to_be_bytes());
    hasher.update(evidence.sp_committed.to_be_bytes());
    hasher.update(evidence.sp_busy_cleared.to_be_bytes());
    hasher.update(evidence.sp_rsp_to_rdram_committed.to_be_bytes());
    hasher.update(evidence.sp_transition_sha256);
    hasher.finalize().into()
}

pub(super) fn host_abi_writer_runtime_state_receipt_sha256(
    evidence: &HostAbiWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:host-abi-writer-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update(evidence.initial_journal_entry_count.to_be_bytes());
    hasher.update(evidence.final_journal_entry_count.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.host_abi_journal_entry_count.to_be_bytes());
    hasher.update(evidence.host_abi_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.transactions_started.to_be_bytes());
    hasher.update(evidence.transactions_finished.to_be_bytes());
    hasher.update(evidence.ordering_boundaries.to_be_bytes());
    hasher.update(evidence.lifecycle_sha256);
    hasher.finalize().into()
}

pub(super) fn rsp_writer_runtime_state_receipt_sha256(
    evidence: &RspWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:rsp-execution-writeback-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.journal_entry_count.to_be_bytes());
    hasher.update(evidence.rsp_journal_declaration_count.to_be_bytes());
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.interpreter_writeback_count.to_be_bytes());
    hasher.update(
        evidence
            .translated_audio_hle_publication_count
            .to_be_bytes(),
    );
    hasher.update(evidence.writeback_range_count.to_be_bytes());
    hasher.update(evidence.writeback_trace_sha256);
    hasher.finalize().into()
}

pub(super) fn rdp_renderer_writer_runtime_state_receipt_sha256(
    evidence: &RdpRendererWriterRuntimeStateEvidenceV1,
) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:rdp-renderer-writer-runtime-state-receipt:v1");
    hasher.update((evidence.schema.len() as u64).to_be_bytes());
    hasher.update(evidence.schema.as_bytes());
    hasher.update(evidence.program_model_sha256);
    hasher.update(evidence.resolver_install_sha256);
    hasher.update(evidence.abi_host_catalog_receipt_sha256);
    hasher.update(evidence.build_receipt.schema.to_be_bytes());
    hasher.update([
        evidence.build_receipt.aot_runtime as u8,
        evidence.build_receipt.production_aot as u8,
        evidence.build_receipt.dev_interpreter as u8,
    ]);
    hasher.update(evidence.trace_epoch_id.to_be_bytes());
    hasher.update(evidence.initial_journal_entry_count.to_be_bytes());
    hasher.update(evidence.final_journal_entry_count.to_be_bytes());
    hasher.update((evidence.watched_ranges.len() as u64).to_be_bytes());
    for range in &evidence.watched_ranges {
        hasher.update(range.physical_start.to_be_bytes());
        hasher.update(range.physical_end.to_be_bytes());
    }
    hasher.update(evidence.rdp_renderer_journal_entry_count.to_be_bytes());
    hasher.update(
        evidence
            .rdp_renderer_journal_declaration_count
            .to_be_bytes(),
    );
    hasher.update(evidence.journal_root_sha256);
    hasher.update(evidence.final_watched_sha256);
    hasher.update(evidence.renderer_publication_count.to_be_bytes());
    hasher.update(evidence.publication_trace_sha256);
    hasher.finalize().into()
}

pub(super) fn hash_pi_request(hasher: &mut sha2::Sha256, request: fn64_runtime::PiDmaRequest) {
    hasher.update([match request.direction {
        fn64_runtime::DmaDirection::ToRdram => 0,
        fn64_runtime::DmaDirection::FromRdram => 1,
    }]);
    hasher.update(request.dram_addr.offset().to_be_bytes());
    match request.device {
        fn64_runtime::PiDeviceAddress::RomOffset(offset) => {
            hasher.update([0]);
            hasher.update(offset.to_be_bytes());
        }
        fn64_runtime::PiDeviceAddress::SramOffset(offset) => {
            hasher.update([1]);
            hasher.update(offset.to_be_bytes());
        }
    }
    hasher.update(request.len.to_be_bytes());
}
