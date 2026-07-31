//! Typed-Rust recompiler adapters over the existing fn64 host ABI.
//!
//! The generated module stays `#![forbid(unsafe_code)]`: it calls ordinary
//! safe [`fn64_recomp_rs::RecompFunc`]s. Raw-pointer reconstruction is
//! confined here, beside the C ABI seam that already owns the identical
//! process-lifetime RDRAM and coroutine contracts.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    num::NonZeroUsize,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use fn64_recomp_rs::{
    enter_pending_interrupt, AotMiss, BackedGenerationCatalogEvidenceV1,
    BackedPrecompiledGenerationCatalogV1, BankId, BlockExit, BlockProgram,
    BlockProgramEvidenceSnapshot, BootContext, BootTvStandard, CallResolution,
    CatalogBlockProgramErrorV1, CatalogBlockProgramV1, CodeBank, CpuException, CpuFault,
    CpuFaultKind, CpuInterruptLine, ExecutableRegion, ExecutionDestinationObservation,
    ExecutionKey, FunctionEntryObservationSchema, GeneratedBankRunner, GenerationCatalogError,
    GenerationError, GenerationId, GenerationLookupError, GuestPc, GuestWriteBoundary,
    GuestWriteEvent, HostFunctionCatalogV1, InstructionBudget, PhysicalFgrState,
    PrecompiledGenerationCatalog, ProgramArtifactIdentity, ProgramIdentityEvidenceSnapshot,
    ProgramIdentitySource, Rdram, RecompContext as RsContext, RecompFunc,
    StaticExecutionBuildReceipt, TransferResolver, TranslatedFunctionIdentity, WriterChannel,
};
use fn64_runtime::{Priority, RdramAddr, ThreadId};
use sha2::Digest;

use super::{with_active_yielder, with_executor, with_host, RecompContext as CContext};

type Lookup = fn(u32) -> RecompFunc;
type CShim = unsafe extern "C" fn(*mut u8, *mut CContext);
const STATUS_FR: u32 = 1 << 26;
const STATUS_BEV: u32 = 1 << 22;
const THREAD_RETURN_SENTINEL: u32 = 0xFFFF_FFFC;
const INITIAL_FPCSR: u32 = crate::system::FPCSR_FS | crate::system::FPCSR_EV;
pub type ProgramEntryLookup = fn(GuestPc) -> Result<ExecutionKey, CpuFault>;
pub type ProgramTransferLookup = fn(BankId, GuestPc) -> Result<ExecutionKey, CpuFault>;
pub type LiveGenerationBuilder = fn(&[u8], u64) -> Result<(CodeBank, GeneratedBankRunner), String>;

struct ObservedExecutableRegion {
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    next_generation: u64,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: Option<ProgramArtifactIdentity>,
    activation: ExecutableActivation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutableActivation {
    EagerPublication,
    FetchBoundary,
}

thread_local! {
    static PENDING_EXECUTABLE_WRITES: RefCell<Vec<(u32, u32)>> = const {
        RefCell::new(Vec::new())
    };
    static PENDING_ATTRIBUTED_EXECUTABLE_WRITES: RefCell<Vec<GuestWriteEvent>> = const {
        RefCell::new(Vec::new())
    };
    static EXECUTABLE_WRITE_RANGES: RefCell<Vec<(u32, u32)>> = const {
        RefCell::new(Vec::new())
    };
    static FUNCTION_LANE_ARTIFACT_IDENTITY: std::cell::Cell<Option<ProgramArtifactIdentity>> =
        const { std::cell::Cell::new(None) };
    static FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA:
        std::cell::Cell<Option<FunctionEntryObservationSchema>> =
        const { std::cell::Cell::new(None) };
    static FUNCTION_EXECUTION_DESTINATIONS:
        RefCell<Vec<FunctionExecutionDestinationObservation>> = const {
        RefCell::new(Vec::new())
    };
    static BLOCK_HOST_BOUNDARIES: RefCell<VecDeque<BlockHostBoundaryObservation>> = const {
        RefCell::new(VecDeque::new())
    };
    static BLOCK_HOST_BOUNDARY_HISTORY_LIMIT: Cell<Option<NonZeroUsize>> = const {
        Cell::new(None)
    };
    static BLOCK_HOST_BOUNDARY_HISTORY_ENABLED: Cell<bool> = const { Cell::new(true) };
    static CPU_INSTRUCTION_STORE_TRACE: RefCell<Option<CpuInstructionStoreTraceV1>> = const {
        RefCell::new(None)
    };
    static RDP_RENDERER_WRITER_TRACE: RefCell<Option<RdpRendererWriterTraceV1>> = const {
        RefCell::new(None)
    };
}

// Interleaving closed: thread A may retain a move-only epoch while thread B
// installs the same program model. A thread-local counter could mint `1` to
// both owners and let A's token satisfy B's arm; this process-global identity
// makes those epochs distinct without using ordering as synchronization.
static NEXT_SP_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CPU_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PI_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_HOST_ABI_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_RSP_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_RDP_RENDERER_WRITER_TRACE_EPOCH_ID: AtomicU64 = AtomicU64::new(1);

fn next_sp_writer_trace_epoch_id() -> u64 {
    NEXT_SP_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("SP writer trace epoch identity overflow"))
}

fn next_cpu_writer_trace_epoch_id() -> u64 {
    NEXT_CPU_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("CPU instruction-store trace epoch identity overflow"))
}

fn next_pi_writer_trace_epoch_id() -> u64 {
    NEXT_PI_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("PI writer trace epoch identity overflow"))
}

fn next_host_abi_writer_trace_epoch_id() -> u64 {
    NEXT_HOST_ABI_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("Host ABI writer trace epoch identity overflow"))
}

fn next_rsp_writer_trace_epoch_id() -> u64 {
    NEXT_RSP_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("RSP writer trace epoch identity overflow"))
}

fn next_rdp_renderer_writer_trace_epoch_id() -> u64 {
    NEXT_RDP_RENDERER_WRITER_TRACE_EPOCH_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("RDP renderer writer trace epoch identity overflow"))
}

#[derive(Debug)]
struct CpuInstructionStoreTraceV1 {
    epoch_id: u64,
    events: Vec<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostAbiWriterTraceEventV1 {
    Started(OpenHostMutationTransactionEvidenceV1),
    Boundary {
        transaction_id: u64,
        thread: ThreadId,
        journal_sequences: Vec<u64>,
    },
    Finished {
        transaction_id: u64,
        thread: ThreadId,
    },
}

#[derive(Clone, Debug)]
struct HostAbiWriterTraceV1 {
    epoch_id: u64,
    initial_journal_entry_count: u64,
    events: Vec<HostAbiWriterTraceEventV1>,
}

#[derive(Clone, Debug)]
struct RdpRendererWriterTraceV1 {
    epoch_id: u64,
    program_model_sha256: [u8; 32],
    initial_journal_entry_count: u64,
    next_journal_entry_index: usize,
    publications: Vec<Vec<u64>>,
    rejected_journal_sequences: Vec<u64>,
}

/// One successfully entered emitted whole-function destination. The artifact
/// identity and `(vram, symbol)` pair are pointer-independent; `at` is the
/// guest device cycle sampled before the first translated instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionExecutionDestinationObservation {
    pub at: fn64_runtime::Cycles,
    pub artifact_identity: ProgramArtifactIdentity,
    pub function: TranslatedFunctionIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockHostBoundaryPhase {
    Enter,
    Exit,
}

/// Architectural state at an exact guest-to-host ABI boundary.
///
/// These observations make host overrides comparable with an independent
/// guest implementation without treating the guest function's address range
/// as interchangeable with the host shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockHostBoundaryObservation {
    pub at: fn64_runtime::Cycles,
    pub thread: ThreadId,
    pub phase: BlockHostBoundaryPhase,
    pub target: GuestPc,
    pub resume: ExecutionKey,
    pub gprs: [u64; 32],
    pub hi: u64,
    pub lo: u64,
    pub cop0_count: u32,
    pub cop0_compare: u32,
    pub cop0_status: u32,
    pub cop0_cause: u32,
    pub cop0_epc: u32,
}

/// Pointer-free architectural evidence published immediately before one
/// canonical outer instruction checkpoint. The cumulative charge is sampled
/// after this slice is charged and shares one ordering across canonical guest
/// threads in the installed program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalThreadCheckpointEvidenceV1 {
    pub thread: ThreadId,
    pub cpu: fn64_recomp_rs::RecompContextEvidenceSnapshotV1,
    pub charged_instructions: u32,
    pub canonical_charged_instructions_at_publication: u64,
    pub pending_exit: BlockExit,
    pub prepared_continuation: Option<CanonicalPreparedContinuationV1>,
}

/// Native continuation state resolved before a canonical checkpoint yields.
///
/// Generation activation must precede the yield so the selected executable
/// identity cannot change while another guest thread runs. Publishing the
/// resulting key keeps the observational checkpoint bound to the state the
/// dormant native frame will consume when it resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalPreparedContinuationV1 {
    ImageChanged { entry: ExecutionKey },
    InactiveGeneration { entry: ExecutionKey },
}

/// Latest observational publication from one canonical guest thread.
///
/// An opaque host marker deliberately makes no claim about resumability or
/// architectural state while arbitrary host ABI code is in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonicalThreadPublicationV1 {
    Exact(CanonicalThreadCheckpointEvidenceV1),
    OpaqueHostInFlight {
        thread: ThreadId,
        target: GuestPc,
        resume: ExecutionKey,
    },
    /// A stopped fault thread retains a native continuation that is not an
    /// independently resumable guest checkpoint. The post-exception CPU and
    /// originating fault are diagnostic only; this variant is deliberately
    /// non-comparable with [`Self::Exact`].
    ParkedFaultOpaque {
        thread: ThreadId,
        post_exception_cpu: fn64_recomp_rs::RecompContextEvidenceSnapshotV1,
        fault: CpuFault,
        canonical_charged_instructions_at_publication: u64,
    },
    Returned {
        thread: ThreadId,
        cpu: fn64_recomp_rs::RecompContextEvidenceSnapshotV1,
    },
}

#[cfg(feature = "dynamic-mapped-runtime")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicMappedEntryCountV1 {
    pub attempted_entry: ExecutionKey,
    pub activations: u64,
    pub charged_instructions: u64,
    pub unsupported_exits: u64,
}

/// Bounded operational hotness/failure summary for one exact dynamic unit.
/// It is deliberately separate from immutable static program evidence and
/// cannot authorize a writer/release receipt.
#[cfg(feature = "dynamic-mapped-runtime")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicMappedExecutionAggregateV1 {
    pub identity: fn64_recomp_rs::DynamicMappedUnitIdentityV1,
    pub admitted_entry: ExecutionKey,
    pub instructions: Vec<fn64_recomp_rs::InstructionWordIdentity>,
    pub attempted_entries: Vec<DynamicMappedEntryCountV1>,
    pub activations: u64,
    /// Instruction-budget units charged by this dynamic unit. Architectural
    /// fault attempts are charged even when they do not retire.
    pub charged_instructions: u64,
    pub unsupported_exits: u64,
    pub first_mutation_sequence: Option<u64>,
    pub last_mutation_sequence: Option<u64>,
    pub last_exit: BlockExit,
}

/// Complete bounded snapshot of operational dynamic execution telemetry.
/// Dropped counters make saturation explicit instead of allowing a long run
/// to grow host memory without limit or silently claiming complete hotness.
#[cfg(feature = "dynamic-mapped-runtime")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicMappedExecutionTelemetryV1 {
    pub resolver_install_sha256: [u8; 32],
    pub program_identity: ProgramIdentityEvidenceSnapshot,
    pub dynamic_source_sha256: [u8; 32],
    pub rom_sha256: Option<[u8; 32]>,
    pub bootstrap_receipt_sha256: Option<[u8; 32]>,
    pub mutation_journal_root_sha256: Option<[u8; 32]>,
    pub mutation_journal_entry_count: u64,
    pub aggregates: Vec<DynamicMappedExecutionAggregateV1>,
    pub aggregate_capacity: u64,
    pub attempted_entries_per_aggregate_capacity: u64,
    pub dropped_identity_activations: u64,
    pub dropped_identity_charged_instructions: u64,
    pub dropped_identity_unsupported_exits: u64,
    pub dropped_attempted_entry_activations: u64,
    pub dropped_attempted_entry_charged_instructions: u64,
    pub dropped_attempted_entry_unsupported_exits: u64,
}

#[cfg(feature = "dynamic-mapped-runtime")]
const DYNAMIC_EXECUTION_AGGREGATE_CAPACITY: usize = 32_768;
#[cfg(feature = "dynamic-mapped-runtime")]
const DYNAMIC_ATTEMPTED_ENTRIES_PER_AGGREGATE_CAPACITY: usize = 16;

/// One immutable arbitrary-PC program plus the active executable-mapping
/// resolvers shared by every guest coroutine in this runtime instance.
#[derive(Clone)]
pub(super) struct LiveBlockProgram {
    program: Rc<RefCell<BlockProgram>>,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: Option<ProgramArtifactIdentity>,
    executable_regions: Rc<RefCell<Vec<ObservedExecutableRegion>>>,
    precompiled_generations: Rc<RefCell<Option<PrecompiledGenerationCatalog>>>,
}

/// Immutable canonical install shared by thread 0 and every spawned OSThread.
/// It is separate from [`LiveBlockProgram`], whose callback and runtime-builder
/// seams remain executable only for compatibility and cannot gain authority.
#[derive(Clone)]
pub(super) struct CanonicalLiveBlockProgramV1 {
    install: Rc<CatalogResolverInstallV1>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_units: Rc<RefCell<Option<fn64_recomp_rs::DynamicMappedUnitCatalogV1>>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_withheld_static_key: Rc<Cell<Option<ExecutionKey>>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_execution_aggregates: Rc<
        RefCell<
            BTreeMap<
                fn64_recomp_rs::DynamicMappedUnitIdentityV1,
                DynamicMappedExecutionAggregateV1,
            >,
        >,
    >,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_identity_activations: Rc<Cell<u64>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_identity_charged_instructions: Rc<Cell<u64>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_identity_unsupported_exits: Rc<Cell<u64>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_attempted_entry_activations: Rc<Cell<u64>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_attempted_entry_charged_instructions: Rc<Cell<u64>>,
    #[cfg(feature = "dynamic-mapped-runtime")]
    dynamic_dropped_attempted_entry_unsupported_exits: Rc<Cell<u64>>,
    canonical_charged_instructions: Rc<Cell<u64>>,
    canonical_instruction_limit: Rc<Cell<Option<u64>>>,
    thread_publications: Rc<RefCell<BTreeMap<ThreadId, CanonicalThreadPublicationV1>>>,
    generations: Option<Rc<RefCell<BackedPrecompiledGenerationCatalogV1>>>,
    mutation_state: Option<Rc<RefCell<CanonicalExecutableMutationStateV1>>>,
    bootstrap_evidence: Option<BootstrapOrImportValidationEvidenceV1>,
    writer_program_model_sha256: [u8; 32],
    bootstrap_writer_completion: Rc<RefCell<Option<ValidatedBootstrapWriterChannelReceiptV1>>>,
    cpu_writer_runtime_state_taken: Rc<Cell<bool>>,
    cpu_writer_trace_epoch_id: Rc<Cell<Option<u64>>>,
    pi_writer_runtime_state_taken: Rc<Cell<bool>>,
    pi_writer_trace_epoch_id: Rc<Cell<Option<u64>>>,
    si_writer_runtime_state_taken: Rc<Cell<bool>>,
    sp_writer_runtime_state_taken: Rc<Cell<bool>>,
    sp_writer_trace_epoch_id: Rc<Cell<Option<u64>>>,
    host_abi_writer_runtime_state_taken: Rc<Cell<bool>>,
    rsp_writer_runtime_state_taken: Rc<Cell<bool>>,
    rsp_writer_trace_epoch_id: Rc<Cell<Option<u64>>>,
    rdp_renderer_writer_runtime_state_taken: Rc<Cell<bool>>,
    rdp_renderer_writer_trace_epoch_id: Rc<Cell<Option<u64>>>,
}

/// One canonical half-open physical write range awaiting executable-image
/// invalidation at the next host boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingExecutableWriteEvidenceSnapshot {
    pub physical_start: u32,
    pub physical_end: u32,
}

pub const CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1: &str =
    "fn64.canonical-executable-mutation-journal.v1";

/// One exact attributed write declaration clipped to the sealed physical
/// executable backing union.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributedExecutableWriteEvidenceV1 {
    pub channel: WriterChannel,
    pub physical_start: u32,
    pub physical_end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutableMutationBatchEvidenceV1 {
    pub sequence: u64,
    pub declared_writes: Vec<AttributedExecutableWriteEvidenceV1>,
    pub changed_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub before_sha256: [u8; 32],
    pub after_sha256: [u8; 32],
    pub invalidated_generations: Vec<GenerationId>,
    pub journal_root_sha256: [u8; 32],
}

/// Pointer-free evidence from the owner that reconciles every ever-admissible
/// executable backing before static dispatch. This is runtime evidence, not
/// yet a channel-closure receipt: mutable API visibility remains a separate
/// structural gate. `journal_root_sha256` authenticates the initial watched
/// baseline and committed entries only. Pending writes and open host frames
/// are transient, non-hash-bound quiescence diagnostics; a future completion
/// constructor must require both to be empty and bind its own final snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalExecutableMutationJournalEvidenceV1 {
    pub schema: String,
    pub watched_ranges: Vec<PendingExecutableWriteEvidenceSnapshot>,
    pub sealed: bool,
    pub expected_sha256: Option<[u8; 32]>,
    pub entries: Vec<ExecutableMutationBatchEvidenceV1>,
    pub journal_root_sha256: [u8; 32],
    pub pending_attributed_writes: usize,
    pub open_host_transactions: Vec<OpenHostMutationTransactionEvidenceV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenHostMutationTransactionEvidenceV1 {
    pub transaction_id: u64,
    pub thread: ThreadId,
    pub target: GuestPc,
    pub resume: ExecutionKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostMutationTransactionTokenV1 {
    transaction_id: u64,
    thread: ThreadId,
}

/// The host scheduler's one guest-visible publication before a selected
/// coroutine resumes. This is deliberately not a catalog host-call frame:
/// there is no guest call target or resume PC to invent for scheduler state.
pub(super) struct SchedulerRunningThreadMirrorV1 {
    selected_thread: ThreadId,
    global: RdramAddr,
    handle: u32,
}

impl SchedulerRunningThreadMirrorV1 {
    pub(super) fn new(selected_thread: ThreadId, global: RdramAddr, handle: u32) -> Self {
        Self {
            selected_thread,
            global,
            handle,
        }
    }
}

#[derive(Clone)]
struct WatchedExecutableBytesV1 {
    physical_start: u32,
    physical_end: u32,
    expected: Vec<u8>,
}

struct CanonicalExecutableMutationStateV1 {
    watched: Vec<WatchedExecutableBytesV1>,
    sealed: bool,
    expected_sha256: Option<[u8; 32]>,
    entries: Vec<ExecutableMutationBatchEvidenceV1>,
    journal_root_sha256: [u8; 32],
    next_sequence: u64,
    next_transaction_id: u64,
    host_transactions: BTreeMap<ThreadId, Vec<OpenHostMutationTransactionEvidenceV1>>,
    host_abi_writer_trace: Option<HostAbiWriterTraceV1>,
    next_child_transaction_id: u64,
    active_child_transaction: Option<u64>,
    poison: Option<String>,
}

/// Canonical state of one dynamically replaceable executable region.
///
/// The builder and its native address are absent. `active_generation` is the
/// generation whose bank is installed now; `next_generation` is retained
/// because it determines the identity passed to the next pure builder call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveExecutableRegionEvidenceSnapshot {
    pub physical_start: u32,
    pub physical_end: u32,
    pub virtual_start: GuestPc,
    pub virtual_end: GuestPc,
    pub active_bank: BankId,
    pub active_generation: u64,
    pub next_generation: u64,
    pub builder_artifact_identity: ProgramArtifactIdentity,
    pub activation: ExecutableActivationEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutableActivationEvidence {
    EagerPublication,
    FetchBoundary,
}

/// Pointer-independent executable evidence for the active typed-Rust lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecompiledProgramEvidenceSnapshot {
    /// Whole-function native callables. Their bodies are opaque at runtime,
    /// so only a producer-supplied artifact identity is authoritative.
    Function {
        identity: ProgramIdentityEvidenceSnapshot,
    },
    /// Bank-qualified arbitrary-PC execution with its complete code image and
    /// future-affecting dynamic-generation state.
    Block {
        program: BlockProgramEvidenceSnapshot,
        dispatch_artifact_identity: ProgramArtifactIdentity,
        instruction_budget: u32,
        executable_regions: Vec<LiveExecutableRegionEvidenceSnapshot>,
        pending_executable_writes: Vec<PendingExecutableWriteEvidenceSnapshot>,
    },
}

/// Historical target-only evidence identity retained for readers of older
/// receipts. New installs always emit V2 below.
pub const CATALOG_RESOLVER_INSTALL_SCHEMA_V1: &str = "fn64.catalog-resolver-install.v1";
pub const CATALOG_RESOLVER_INSTALL_SCHEMA_V2: &str = "fn64.catalog-resolver-install.v2";
pub const ABI_HOST_FUNCTION_CATALOG_SCHEMA_V1: &str = "fn64.abi-host-function-catalog.v1";

/// One call target resolved entirely by the canonical install that owns both
/// the exact host-function inventory and the static guest-code catalog.
#[derive(Clone, Copy)]
pub enum CatalogCallResolutionV1 {
    Host(RecompFunc),
    Guest(ExecutionKey),
}

/// Pointer-free evidence captured when a canonical block program and exact
/// host-target catalog are assembled into one resolver substrate.
///
/// This snapshot describes only the objects owned by [`CatalogResolverInstallV1`].
/// `abi_host_catalog` distinguishes an ABI-issued semantic catalog from the
/// compatibility caller-pointer lane. Neither form alone asserts that the
/// host-target list is total, every transfer resolves, or the install is
/// sufficient for a release gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogResolverInstallEvidenceV1 {
    pub schema: String,
    pub program_identity: ProgramIdentityEvidenceSnapshot,
    pub entry: ExecutionKey,
    pub instruction_budget: u32,
    pub host_target_pcs: Vec<u32>,
    /// Present only when fn64-abi selected every callable and writer-effect
    /// declaration from its private stable-shim registry.
    pub abi_host_catalog: Option<AbiHostFunctionCatalogEvidenceV1>,
    pub dispatch_artifact_identity: ProgramArtifactIdentity,
    pub build_receipt: StaticExecutionBuildReceipt,
}

/// Stable names for the WM canonical catalog's public libultra shims.
///
/// A caller may select one of these names and its guest target PC, but cannot
/// supply the callable or its writer-effect declaration. Both are selected by
/// fn64-abi below, beside the safe-Rust adapters which own the C boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum AbiHostShimV1 {
    OsCreateMesgQueue,
    OsCreateThread,
    OsEPiStartDma,
    OsGetThreadPri,
    OsRecvMesg,
    OsSendMesg,
    OsSetEventMesg,
    OsSiDeviceBusy,
    OsSetThreadPri,
    OsSetTimer,
    OsSpTaskLoad,
    OsSpTaskStartGo,
    OsSpTaskYield,
    OsSpTaskYielded,
    OsStartThread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbiHostShimBindingV1 {
    pub target_pc: u32,
    pub shim: AbiHostShimV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiHostShimBindingEvidenceV1 {
    pub target_pc: u32,
    pub shim: AbiHostShimV1,
    /// Canonical ascending channel declaration derived by fn64-abi.
    pub writer_effects: Vec<WriterChannel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiHostFunctionCatalogEvidenceV1 {
    pub schema: String,
    pub bindings: Vec<AbiHostShimBindingEvidenceV1>,
    pub receipt_sha256: [u8; 32],
}

/// Opaque ABI-issued host catalog. No function-pointer or effect-claim input
/// crosses its public constructor.
pub struct AbiHostFunctionCatalogV1 {
    catalog: HostFunctionCatalogV1,
    evidence: AbiHostFunctionCatalogEvidenceV1,
}

impl AbiHostFunctionCatalogV1 {
    pub fn evidence(&self) -> &AbiHostFunctionCatalogEvidenceV1 {
        &self.evidence
    }

    pub fn has_valid_evidence_hash(&self) -> bool {
        self.evidence.schema == ABI_HOST_FUNCTION_CATALOG_SCHEMA_V1
            && self.evidence.receipt_sha256
                == abi_host_function_catalog_receipt_sha256(&self.evidence)
            && self
                .evidence
                .bindings
                .iter()
                .all(|binding| binding.writer_effects == abi_host_shim_writer_effects(binding.shim))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiHostFunctionCatalogErrorV1 {
    MisalignedTarget { target: u32 },
    DuplicateTarget { target: u32 },
}

impl std::fmt::Display for AbiHostFunctionCatalogErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid ABI host-function catalog: {self:?}")
    }
}

impl std::error::Error for AbiHostFunctionCatalogErrorV1 {}

/// Issue a catalog from stable shim names. The ABI, not the caller, selects
/// both each safe-Rust callable and its conservative writer-effect set.
pub fn issue_abi_host_function_catalog_v1(
    mut bindings: Vec<AbiHostShimBindingV1>,
) -> Result<AbiHostFunctionCatalogV1, AbiHostFunctionCatalogErrorV1> {
    bindings.sort_unstable_by_key(|binding| binding.target_pc);
    for binding in &bindings {
        if !binding.target_pc.is_multiple_of(4) {
            return Err(AbiHostFunctionCatalogErrorV1::MisalignedTarget {
                target: binding.target_pc,
            });
        }
    }
    if let Some(pair) = bindings
        .windows(2)
        .find(|pair| pair[0].target_pc == pair[1].target_pc)
    {
        return Err(AbiHostFunctionCatalogErrorV1::DuplicateTarget {
            target: pair[0].target_pc,
        });
    }
    let catalog = HostFunctionCatalogV1::new(
        bindings
            .iter()
            .map(|binding| (binding.target_pc, abi_host_shim_callable(binding.shim)))
            .collect(),
    )
    .expect("ABI host catalog prevalidation disagrees with HostFunctionCatalogV1");
    let mut evidence = AbiHostFunctionCatalogEvidenceV1 {
        schema: ABI_HOST_FUNCTION_CATALOG_SCHEMA_V1.to_string(),
        bindings: bindings
            .into_iter()
            .map(|binding| AbiHostShimBindingEvidenceV1 {
                target_pc: binding.target_pc,
                shim: binding.shim,
                writer_effects: abi_host_shim_writer_effects(binding.shim),
            })
            .collect(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = abi_host_function_catalog_receipt_sha256(&evidence);
    Ok(AbiHostFunctionCatalogV1 { catalog, evidence })
}

/// Whether a build receipt names the feature lane required by this substrate.
///
/// This predicate is deliberately insufficient for authority. Feature
/// eligibility says nothing about catalog totality, ROM identity, execution
/// coverage, scenario completion, or the absence of fallback paths.
pub const fn catalog_resolver_feature_lane_eligible(receipt: StaticExecutionBuildReceipt) -> bool {
    receipt.production_aot && receipt.aot_runtime && !receipt.dev_interpreter
}

/// Canonical block-program and exact host-target install substrate.
///
/// The owned catalogs remain encapsulated: callers can run the fixed entry,
/// change only validated execution controls, replace the whole validated
/// program, or resolve an exact host PC. No callback or catalog-total policy
/// enters this type. The optional ABI-issued catalog proves only its exact
/// name-to-callable/effect mapping, and construction does not install it into
/// the ABI.
pub struct CatalogResolverInstallV1 {
    program: CatalogBlockProgramV1,
    host_functions: HostFunctionCatalogV1,
    dispatch_artifact_identity: ProgramArtifactIdentity,
    evidence: CatalogResolverInstallEvidenceV1,
}

impl CatalogResolverInstallV1 {
    /// Construct a compatibility install from a caller-supplied function
    /// catalog. It remains executable, but carries no host-semantic authority.
    pub fn new(
        program: CatalogBlockProgramV1,
        host_functions: HostFunctionCatalogV1,
        dispatch_artifact_identity: ProgramArtifactIdentity,
    ) -> Self {
        let evidence =
            Self::capture_evidence(&program, &host_functions, None, dispatch_artifact_identity);
        Self {
            program,
            host_functions,
            dispatch_artifact_identity,
            evidence,
        }
    }

    /// Construct an install whose complete host catalog was issued by fn64-abi.
    pub fn new_with_abi_host_catalog(
        program: CatalogBlockProgramV1,
        host_authority: AbiHostFunctionCatalogV1,
        dispatch_artifact_identity: ProgramArtifactIdentity,
    ) -> Self {
        assert!(
            host_authority.has_valid_evidence_hash(),
            "ABI-issued host catalog evidence hash mismatch"
        );
        let AbiHostFunctionCatalogV1 { catalog, evidence } = host_authority;
        let install_evidence = Self::capture_evidence(
            &program,
            &catalog,
            Some(evidence),
            dispatch_artifact_identity,
        );
        Self {
            program,
            host_functions: catalog,
            dispatch_artifact_identity,
            evidence: install_evidence,
        }
    }

    fn capture_evidence(
        program: &CatalogBlockProgramV1,
        host_functions: &HostFunctionCatalogV1,
        abi_host_catalog: Option<AbiHostFunctionCatalogEvidenceV1>,
        dispatch_artifact_identity: ProgramArtifactIdentity,
    ) -> CatalogResolverInstallEvidenceV1 {
        CatalogResolverInstallEvidenceV1 {
            schema: CATALOG_RESOLVER_INSTALL_SCHEMA_V2.to_string(),
            program_identity: program.identity(),
            entry: program.entry(),
            instruction_budget: program.budget().get(),
            host_target_pcs: host_functions.target_pcs().to_vec(),
            abi_host_catalog,
            dispatch_artifact_identity,
            build_receipt: program.build_receipt(),
        }
    }

    pub fn evidence(&self) -> &CatalogResolverInstallEvidenceV1 {
        &self.evidence
    }

    pub fn has_abi_host_catalog_authority(&self) -> bool {
        self.evidence
            .abi_host_catalog
            .as_ref()
            .is_some_and(|evidence| {
                evidence.schema == ABI_HOST_FUNCTION_CATALOG_SCHEMA_V1
                    && evidence.receipt_sha256 == abi_host_function_catalog_receipt_sha256(evidence)
                    && evidence.bindings.iter().all(|binding| {
                        binding.writer_effects == abi_host_shim_writer_effects(binding.shim)
                    })
                    && evidence
                        .bindings
                        .iter()
                        .map(|binding| binding.target_pc)
                        .eq(self.evidence.host_target_pcs.iter().copied())
            })
    }

    pub const fn entry(&self) -> ExecutionKey {
        self.program.entry()
    }

    pub const fn budget(&self) -> InstructionBudget {
        self.program.budget()
    }

    pub fn run(&self, ctx: &mut RsContext, mem: &mut Rdram<'_>) -> fn64_recomp_rs::BlockRun {
        self.program.run(ctx, mem)
    }

    /// Dispatch from one admitted continuation using only the program and
    /// exact host inventory owned by this install.
    pub fn dispatch_exposing_exceptions_at(
        &self,
        entry: ExecutionKey,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> Result<fn64_recomp_rs::DispatchRun, fn64_recomp_rs::DispatchError> {
        self.dispatch_exposing_exceptions_at_budget(entry, self.budget(), ctx, mem)
    }

    fn dispatch_exposing_exceptions_at_budget(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> Result<fn64_recomp_rs::DispatchRun, fn64_recomp_rs::DispatchError> {
        self.program.dispatch_exposing_exceptions_at_budget(
            entry,
            &self.host_functions,
            budget,
            ctx,
            mem,
        )
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn reserves_bank(&self, bank: BankId) -> bool {
        self.program.reserves_bank(bank)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn reserves_bank_with_generations(
        &self,
        bank: BankId,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> bool {
        self.program
            .reserves_bank_with_generations(bank, generations)
    }

    fn validate_precompiled_generations(
        &self,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<(), GenerationCatalogError> {
        self.program.validate_precompiled_generations(generations)
    }

    fn resolve_entry_with_generations(
        &self,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program
            .resolve_entry_with_generations(target_pc, generations)
    }

    fn resolve_transfer_with_generations(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
        generations: &BackedPrecompiledGenerationCatalogV1,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program
            .resolve_transfer_with_generations(source_bank, target_pc, generations)
    }

    fn dispatch_exposing_exceptions_with_generations_at_budget(
        &self,
        entry: ExecutionKey,
        generations: &BackedPrecompiledGenerationCatalogV1,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> Result<fn64_recomp_rs::DispatchRun, fn64_recomp_rs::DispatchError> {
        self.program
            .dispatch_exposing_exceptions_with_generations_at_budget(
                entry,
                &self.host_functions,
                generations,
                budget,
                ctx,
                mem,
            )
    }

    pub fn program_evidence(&self) -> &BlockProgramEvidenceSnapshot {
        self.program.evidence()
    }

    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.program.copy_execution_destinations()
    }

    /// Resolve a bankless static guest entry against the complete owned
    /// virtual-code catalog. Dynamic and physical generations remain an outer
    /// ABI responsibility and are not guessed here.
    pub fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        self.program.resolve_entry(target_pc)
    }

    /// Resolve one static guest transfer with exact source-bank preference.
    pub fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.program.resolve_transfer(source_bank, target_pc)
    }

    /// Resolve a call against the exact host catalog before consulting static
    /// guest code. The resolved host pointer stays attached to the result, so
    /// execution cannot silently substitute a second or global lookup.
    pub fn resolve_call(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<CatalogCallResolutionV1, CpuFault> {
        if let Some(host) = self.host_functions.resolve(target_pc.get()) {
            Ok(CatalogCallResolutionV1::Host(host))
        } else {
            self.resolve_transfer(source_bank, target_pc)
                .map(CatalogCallResolutionV1::Guest)
        }
    }

    pub fn set_entry(&mut self, entry: ExecutionKey) -> Result<(), CatalogBlockProgramErrorV1> {
        self.program.set_entry(entry)?;
        let abi_host_catalog = self.evidence.abi_host_catalog.clone();
        self.evidence = Self::capture_evidence(
            &self.program,
            &self.host_functions,
            abi_host_catalog,
            self.dispatch_artifact_identity,
        );
        Ok(())
    }

    pub fn set_budget(&mut self, budget: InstructionBudget) {
        self.program.set_budget(budget);
        let abi_host_catalog = self.evidence.abi_host_catalog.clone();
        self.evidence = Self::capture_evidence(
            &self.program,
            &self.host_functions,
            abi_host_catalog,
            self.dispatch_artifact_identity,
        );
    }

    /// Replace the complete, independently validated program while retaining
    /// the exact host catalog and dispatch artifact identity.
    pub fn replace_program(&mut self, program: CatalogBlockProgramV1) {
        self.program = program;
        let abi_host_catalog = self.evidence.abi_host_catalog.clone();
        self.evidence = Self::capture_evidence(
            &self.program,
            &self.host_functions,
            abi_host_catalog,
            self.dispatch_artifact_identity,
        );
    }

    /// Resolve exactly one enumerated host target. Absence carries no policy:
    /// it does not imply that the target is guest code or that catalogs are total.
    pub fn resolve_host(&self, target_pc: u32) -> Option<RecompFunc> {
        self.host_functions.resolve(target_pc)
    }
}

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
    resolver: CatalogResolverInstallV1,
    generations: BackedPrecompiledGenerationCatalogV1,
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
pub const PI_WRITER_RUNTIME_STATE_SCHEMA_V1: &str = "fn64.pi-writer-runtime-state.v1";
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
    evidence: BootstrapWriterChannelCompletionEvidenceV1,
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
    evidence: CpuWriterRuntimeStateEvidenceV1,
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
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    evidence: PiWriterRuntimeStateEvidenceV1,
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
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    evidence: SiWriterRuntimeStateEvidenceV1,
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
    evidence: SpWriterRuntimeStateEvidenceV1,
}

/// One unforgeable fresh-trace epoch owned by a canonical SP audit.
///
/// Construction clears retained device history and re-enables retention. The
/// token is move-only and its live epoch arm is consumed by successful
/// validation, so evidence from an older trace cannot be paired with a later
/// runtime state.
#[derive(Debug)]
pub struct SpWriterRuntimeTraceEpochV1 {
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    evidence: HostAbiWriterRuntimeStateEvidenceV1,
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
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    evidence: RspWriterRuntimeStateEvidenceV1,
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
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    evidence: RdpRendererWriterRuntimeStateEvidenceV1,
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
    epoch_id: u64,
    program_model_sha256: [u8; 32],
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
    storage: Box<[u8]>,
    receipt: BootstrapOrImportValidationReceiptV1,
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
    storage: Box<[u8]>,
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
        let minimum = fn64_recomp_rs::RDRAM_LEN;
        if rdram_len < minimum {
            return Err(BootstrapImportErrorV1::RdramLength {
                actual: rdram_len,
                minimum,
            });
        }
        let mut storage = vec![0; rdram_len].into_boxed_slice();
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_u32(fn64_runtime::RdramAddr::from_offset(0x300), tv_type as u32);
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
        if physical_end > fn64_recomp_rs::RDRAM_LEN as u32 {
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
                fn64_recomp_rs::InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
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
    ((0x8000_0000..0xc000_0000).contains(&address) && physical < fn64_recomp_rs::RDRAM_LEN as u32)
        .then_some(physical)
}

fn executable_physical_ranges(install: &CatalogGenerationInstallV1) -> Vec<(u32, u32)> {
    executable_physical_ranges_for_parts(&install.resolver, Some(&install.generations))
}

fn executable_physical_ranges_for_parts(
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

fn validate_initial_entry_image(
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

fn watched_bytes_sha256(storage: &[u8], ranges: &[(u32, u32)]) -> [u8; 32] {
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

fn resolver_install_definition_sha256(install: &CatalogResolverInstallV1) -> [u8; 32] {
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

fn abi_host_function_catalog_receipt_sha256(
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

fn bootstrap_receipt_sha256(evidence: &BootstrapOrImportValidationEvidenceV1) -> [u8; 32] {
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

fn canonical_mutation_initial_root(
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

fn canonical_mutation_entry_root(
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

fn canonical_writer_program_model_sha256(
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

fn bootstrap_writer_channel_completion_receipt_sha256(
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

fn si_writer_runtime_state_receipt_sha256(evidence: &SiWriterRuntimeStateEvidenceV1) -> [u8; 32] {
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

fn cpu_writer_runtime_state_receipt_sha256(evidence: &CpuWriterRuntimeStateEvidenceV1) -> [u8; 32] {
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

fn pi_writer_runtime_state_receipt_sha256(evidence: &PiWriterRuntimeStateEvidenceV1) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-state-receipt:v1");
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

fn sp_writer_runtime_state_receipt_sha256(evidence: &SpWriterRuntimeStateEvidenceV1) -> [u8; 32] {
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

fn host_abi_writer_runtime_state_receipt_sha256(
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

fn rsp_writer_runtime_state_receipt_sha256(evidence: &RspWriterRuntimeStateEvidenceV1) -> [u8; 32] {
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

fn rdp_renderer_writer_runtime_state_receipt_sha256(
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

fn hash_pi_request(hasher: &mut sha2::Sha256, request: fn64_runtime::PiDmaRequest) {
    hasher.update([match request.direction {
        fn64_runtime::DmaDirection::ToRdram => 0,
        fn64_runtime::DmaDirection::FromRdram => 1,
    }]);
    hasher.update(request.dram_addr.offset().to_be_bytes());
    hasher.update(request.cart_addr.to_be_bytes());
    hasher.update(request.len.to_be_bytes());
}

fn validate_pi_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, u64, u64, u64, u64, [u8; 32]), PiWriterRuntimeStateErrorV1> {
    #[derive(Clone, Copy)]
    struct ActivePi {
        request: fn64_runtime::PiDmaRequest,
        phase: u8,
    }

    let mut active: Option<ActivePi> = None;
    let mut started = 0u64;
    let mut committed = 0u64;
    let mut busy_cleared = 0u64;
    let mut interrupt_raised = 0u64;
    let mut interrupt_cleared = 0u64;
    // The public begin API rejects an already asserted PI line before it
    // clears retained history, so a fresh epoch has one exact initial state.
    let mut interrupt_asserted = false;
    let mut notifications = 0u64;
    let mut to_rdram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-transitions:v1");

    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
            }
        }
        previous_order = Some(order);

        let transition = match event.kind {
            fn64_runtime::DeviceTraceKind::PiDmaStarted(request) => {
                if active.is_some() {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                active = Some(ActivePi { request, phase: 0 });
                started = started
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(request) => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.request != request || current.phase != 0 {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 1;
                committed = committed
                    .checked_add(1)
                    .expect("PI transition count overflow");
                if request.direction == fn64_runtime::DmaDirection::ToRdram {
                    to_rdram_committed = to_rdram_committed
                        .checked_add(1)
                        .expect("PI transition count overflow");
                }
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::PiBusyCleared => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.phase != 1 {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 2;
                busy_cleared = busy_cleared
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((2, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi) => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.phase != 2 || interrupt_asserted {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 3;
                interrupt_asserted = true;
                interrupt_raised = interrupt_raised
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((3, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptCleared(
                fn64_runtime::InterruptSource::Pi,
            ) => {
                if !interrupt_asserted || active.is_some_and(|current| current.phase == 3) {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                interrupt_asserted = false;
                interrupt_cleared = interrupt_cleared
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((5, None))
            }
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ) => {
                let Some(current) = active else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                let completed_request = fn64_runtime::PiDmaRequest {
                    direction: completion.direction,
                    dram_addr: completion.dram_addr,
                    cart_addr: completion.dev_addr,
                    len: completion.len,
                };
                if current.request != completed_request
                    || (current.phase != 3 && !(current.phase == 2 && interrupt_asserted))
                {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                active = None;
                notifications = notifications
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((4, Some(completed_request)))
            }
            _ => None,
        };
        if let Some((tag, request)) = transition {
            transitions = transitions
                .checked_add(1)
                .expect("PI transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_pi_request(&mut hasher, request);
            }
        }
    }

    if active.is_some()
        || started != committed
        || started != busy_cleared
        || started != notifications
    {
        return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
    }
    if transitions == 0 || started == 0 {
        return Err(PiWriterRuntimeStateErrorV1::NoPiTransitions);
    }
    if to_rdram_committed == 0 {
        return Err(PiWriterRuntimeStateErrorV1::NoToRdramCommit);
    }
    Ok((
        started,
        committed,
        busy_cleared,
        interrupt_raised,
        interrupt_cleared,
        notifications,
        to_rdram_committed,
        hasher.finalize().into(),
    ))
}

fn hash_sp_request(hasher: &mut sha2::Sha256, request: fn64_runtime::SpDmaRequest) {
    hasher.update([match request.direction {
        fn64_runtime::SpDmaDirection::RdramToRsp => 0,
        fn64_runtime::SpDmaDirection::RspToRdram => 1,
    }]);
    hasher.update((request.mem_addr.offset() as u32).to_be_bytes());
    hasher.update(request.dram_addr.offset().to_be_bytes());
    hasher.update(request.encoded_len.to_be_bytes());
}

fn validate_sp_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, u64, u64, [u8; 32]), SpWriterRuntimeStateErrorV1> {
    let mut active = None;
    let mut queued = None;
    let mut expect_busy_clear = false;
    let mut started = 0u64;
    let mut queued_count = 0u64;
    let mut committed = 0u64;
    let mut busy_cleared = 0u64;
    let mut rsp_to_rdram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:sp-writer-runtime-transitions:v1");

    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
            }
        }
        previous_order = Some(order);

        // A committed active slot promotes its queued request, or publishes
        // DMA idle, inside the same device event transition. No other retained
        // event can interleave between those two records.
        if let Some(expected) = queued {
            if active.is_none()
                && !matches!(
                    event.kind,
                    fn64_runtime::DeviceTraceKind::SpDmaStarted(actual) if actual == expected
                )
            {
                return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
            }
        } else if expect_busy_clear
            && !matches!(event.kind, fn64_runtime::DeviceTraceKind::SpDmaBusyCleared)
        {
            return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
        }

        let transition = match event.kind {
            fn64_runtime::DeviceTraceKind::SpDmaStarted(request) => {
                if active.is_some() || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                if let Some(expected) = queued.take() {
                    if expected != request {
                        return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                    }
                }
                active = Some(request);
                started = started
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaQueued(request) => {
                if active.is_none() || queued.is_some() || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                queued = Some(request);
                queued_count = queued_count
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(request) => {
                if active != Some(request) || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                active = None;
                committed = committed
                    .checked_add(1)
                    .expect("SP transition count overflow");
                if request.direction == fn64_runtime::SpDmaDirection::RspToRdram {
                    rsp_to_rdram_committed = rsp_to_rdram_committed
                        .checked_add(1)
                        .expect("SP transition count overflow");
                }
                expect_busy_clear = queued.is_none();
                Some((2, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared => {
                if !expect_busy_clear || active.is_some() || queued.is_some() {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                expect_busy_clear = false;
                busy_cleared = busy_cleared
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((3, None))
            }
            _ => None,
        };
        if let Some((tag, request)) = transition {
            transitions = transitions
                .checked_add(1)
                .expect("SP transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_sp_request(&mut hasher, request);
            }
        }
    }
    if active.is_some() || queued.is_some() || expect_busy_clear || started != committed {
        return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
    }
    if transitions == 0 || started == 0 {
        return Err(SpWriterRuntimeStateErrorV1::NoSpTransitions);
    }
    if rsp_to_rdram_committed == 0 {
        return Err(SpWriterRuntimeStateErrorV1::NoRspToRdramCommit);
    }
    Ok((
        started,
        queued_count,
        committed,
        busy_cleared,
        rsp_to_rdram_committed,
        hasher.finalize().into(),
    ))
}

fn hash_si_request(hasher: &mut sha2::Sha256, request: fn64_runtime::SiDmaRequest) {
    let kind = match request.kind {
        fn64_runtime::SiDmaKind::DramToPif => 0,
        fn64_runtime::SiDmaKind::PifToDram => 1,
        fn64_runtime::SiDmaKind::ControllerQuery => 2,
        fn64_runtime::SiDmaKind::ControllerRead => 3,
    };
    hasher.update([kind]);
    hasher.update(request.dram_addr.offset().to_be_bytes());
}

fn validate_si_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, [u8; 32]), SiWriterRuntimeStateErrorV1> {
    #[derive(Clone, Copy)]
    struct ActiveSi {
        request: fn64_runtime::SiDmaRequest,
        phase: u8,
    }

    let mut active: Option<ActiveSi> = None;
    let mut started = 0u64;
    let mut committed = 0u64;
    let mut pif_to_dram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:si-writer-runtime-transitions:v1");
    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
            }
        }
        previous_order = Some(order);
        let tag = match event.kind {
            fn64_runtime::DeviceTraceKind::SiDmaStarted(request) => {
                if active.is_some() {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                active = Some(ActiveSi { request, phase: 0 });
                started = started
                    .checked_add(1)
                    .expect("SI transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SiBytesCommitted(request) => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.request != request || current.phase != 0 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 1;
                committed = committed
                    .checked_add(1)
                    .expect("SI transition count overflow");
                if request.kind == fn64_runtime::SiDmaKind::PifToDram {
                    pif_to_dram_committed = pif_to_dram_committed
                        .checked_add(1)
                        .expect("SI transition count overflow");
                }
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SiBusyCleared => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.phase != 1 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 2;
                Some((2, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Si) => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.phase != 2 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 3;
                Some((3, None))
            }
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(request),
            ) => {
                let Some(current) = active else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.request != request || current.phase != 3 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                active = None;
                Some((4, Some(request)))
            }
            _ => None,
        };
        if let Some((tag, request)) = tag {
            transitions = transitions
                .checked_add(1)
                .expect("SI transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_si_request(&mut hasher, request);
            }
        }
    }
    if active.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
    }
    if started == 0 || transitions == 0 {
        return Err(SiWriterRuntimeStateErrorV1::NoSiTransitions);
    }
    if started != committed {
        return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
    }
    if pif_to_dram_committed == 0 {
        return Err(SiWriterRuntimeStateErrorV1::NoPifToDramCommit);
    }
    Ok((
        started,
        committed,
        pif_to_dram_committed,
        hasher.finalize().into(),
    ))
}

fn validate_bootstrap_binding(
    validated: &ValidatedBootstrapRdramV1,
    install: &CatalogGenerationInstallV1,
) -> Result<(), BootstrapImportErrorV1> {
    let evidence = validated.receipt.evidence();
    if evidence.schema != BOOTSTRAP_IMPORT_VALIDATION_SCHEMA_V1 {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch { field: "schema" });
    }
    if evidence.receipt_sha256 != bootstrap_receipt_sha256(evidence) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "receipt_sha256",
        });
    }
    if evidence.resolver_install_sha256 != resolver_install_definition_sha256(&install.resolver) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "resolver_install_sha256",
        });
    }
    if evidence.generation_catalog_sha256 != install.generations.canonical_definition_sha256() {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "generation_catalog_sha256",
        });
    }
    if evidence.initial_entry != install.resolver.entry() {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "initial_entry",
        });
    }
    let ranges = executable_physical_ranges(install);
    if evidence
        .watched_ranges
        .iter()
        .map(|range| (range.physical_start, range.physical_end))
        .ne(ranges.iter().copied())
    {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "watched_ranges",
        });
    }
    if evidence.watched_sha256 != watched_bytes_sha256(&validated.storage, &ranges) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "watched_sha256",
        });
    }
    validate_initial_entry_image(install, &validated.storage)?;
    let view = fn64_runtime::RdramView::from_storage(&validated.storage);
    let initial_generations = install
        .generations
        .validate_initial_physical_images(|physical| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        })
        .map_err(|error| match error {
            fn64_recomp_rs::InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
                physical_address,
                actual,
            } => BootstrapImportErrorV1::UnrecognizedInitialGenerationImage {
                physical_address,
                actual,
            },
        })?;
    if evidence.initial_generations != initial_generations {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "initial_generations",
        });
    }
    Ok(())
}

fn validate_bootstrap_writer_completion_state(
    program_model_sha256: [u8; 32],
    bootstrap: &BootstrapOrImportValidationEvidenceV1,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
) -> Result<ValidatedBootstrapWriterChannelReceiptV1, BootstrapWriterChannelCompletionErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(BootstrapWriterChannelCompletionErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(BootstrapWriterChannelCompletionErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(BootstrapWriterChannelCompletionErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(BootstrapWriterChannelCompletionErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(BootstrapWriterChannelCompletionErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(BootstrapWriterChannelCompletionErrorV1::ActiveChildTransaction);
    }
    if state.next_transaction_id != 0 || state.next_child_transaction_id != 0 {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedTransactionCounters);
    }
    if state.entries.len() != 1 || state.next_sequence != 1 {
        return Err(
            BootstrapWriterChannelCompletionErrorV1::MissingOrExtraJournalEntries {
                actual: state.entries.len(),
            },
        );
    }

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    if watched_ranges != bootstrap.watched_ranges {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedJournalEntry);
    }
    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(BootstrapWriterChannelCompletionErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(BootstrapWriterChannelCompletionErrorV1::CurrentWatchedBytesMismatch);
    }

    let zero_snapshot = state
        .watched
        .iter()
        .map(|range| vec![0; range.expected.len()])
        .collect::<Vec<_>>();
    let before_sha256 = state.digest_snapshot(&zero_snapshot);
    let initial_root =
        canonical_mutation_initial_root(before_sha256, watched_ranges.iter().copied());
    let expected_declarations = state.clipped_declarations(
        &bootstrap
            .publications
            .iter()
            .map(|publication| GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: publication.physical_start,
                len: publication.physical_end - publication.physical_start,
            })
            .collect::<Vec<_>>(),
    );
    let mut expected_changed_ranges = Vec::new();
    for (range, current) in state.watched.iter().zip(&snapshot) {
        let mut index = 0;
        while index < current.len() {
            if current[index] == 0 {
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < current.len() && current[index] != 0 {
                index += 1;
            }
            expected_changed_ranges.push(PendingExecutableWriteEvidenceSnapshot {
                physical_start: range.physical_start + start as u32,
                physical_end: range.physical_start + index as u32,
            });
        }
    }
    let entry = &state.entries[0];
    if entry.sequence != 0
        || entry.declared_writes != expected_declarations
        || entry.changed_ranges != expected_changed_ranges
        || entry.before_sha256 != before_sha256
        || entry.after_sha256 != final_watched_sha256
        || !entry.invalidated_generations.is_empty()
        || entry.journal_root_sha256 != canonical_mutation_entry_root(initial_root, entry)
        || state.journal_root_sha256 != entry.journal_root_sha256
    {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedJournalEntry);
    }

    let mut evidence = BootstrapWriterChannelCompletionEvidenceV1 {
        schema: BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1.to_string(),
        program_model_sha256,
        bootstrap_receipt_sha256: bootstrap.receipt_sha256,
        rom_sha256: bootstrap.rom_sha256,
        resolver_install_sha256: bootstrap.resolver_install_sha256,
        generation_catalog_sha256: bootstrap.generation_catalog_sha256,
        watched_ranges,
        bootstrap_watched_sha256: bootstrap.watched_sha256,
        initial_generations: bootstrap.initial_generations.clone(),
        journal_entry: entry.clone(),
        final_watched_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = bootstrap_writer_channel_completion_receipt_sha256(&evidence);
    let receipt = ValidatedBootstrapWriterChannelReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(BootstrapWriterChannelCompletionErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

fn validate_cpu_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), CpuWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(CpuWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(CpuWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(CpuWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(CpuWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(CpuWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(CpuWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_cpu_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[(u32, u32)],
) -> Result<ValidatedCpuWriterRuntimeStateReceiptV1, CpuWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(CpuWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(CpuWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(CpuWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_cpu_writer_quiescence(state)?;
    if trace.is_empty() {
        return Err(CpuWriterRuntimeStateErrorV1::NoCpuStores);
    }
    if trace.iter().any(|&(start, len)| {
        len == 0
            || start
                .checked_add(len)
                .is_none_or(|end| end > fn64_recomp_rs::RDRAM_LEN as u32)
    }) {
        return Err(CpuWriterRuntimeStateErrorV1::InvalidCpuStoreRange);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(CpuWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(CpuWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut trace_hasher = sha2::Sha256::new();
    trace_hasher.update(b"fn64:cpu-instruction-store-trace:v1");
    trace_hasher.update(trace_epoch_id.to_be_bytes());
    trace_hasher.update((trace.len() as u64).to_be_bytes());
    for &(physical_start, len) in trace {
        trace_hasher.update(physical_start.to_be_bytes());
        trace_hasher.update(len.to_be_bytes());
    }
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = CpuWriterRuntimeStateEvidenceV1 {
        schema: CPU_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("CPU runtime-state journal entry count exceeds u64"),
        cpu_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::CpuInstructionStore)
                .count(),
        )
        .expect("CPU runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        cpu_store_count: u64::try_from(trace.len()).expect("CPU store trace exceeds u64"),
        cpu_store_trace_sha256: trace_hasher.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = cpu_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedCpuWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(CpuWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

fn validate_pi_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), PiWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(PiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(PiWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(PiWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(PiWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(PiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(PiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_pi_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_pi: bool,
    pending_abi_pi: bool,
) -> Result<ValidatedPiWriterRuntimeStateReceiptV1, PiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(PiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(PiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(PiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_pi_writer_quiescence(state)?;
    if pending_device_pi {
        return Err(PiWriterRuntimeStateErrorV1::PendingDevicePi);
    }
    if pending_abi_pi {
        return Err(PiWriterRuntimeStateErrorV1::PendingAbiPi);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(PiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(PiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let (
        pi_started,
        pi_committed,
        pi_busy_cleared,
        pi_interrupt_raised,
        pi_interrupt_cleared,
        pi_notifications,
        pi_to_rdram_committed,
        pi_transition_sha256,
    ) = validate_pi_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = PiWriterRuntimeStateEvidenceV1 {
        schema: PI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("PI runtime-state journal entry count exceeds u64"),
        pi_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::PiDma)
                .count(),
        )
        .expect("PI runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        pi_started,
        pi_committed,
        pi_busy_cleared,
        pi_interrupt_raised,
        pi_interrupt_cleared,
        pi_notifications,
        pi_to_rdram_committed,
        pi_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = pi_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedPiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(PiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
fn validate_si_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_si: bool,
    pending_abi_si: bool,
) -> Result<ValidatedSiWriterRuntimeStateReceiptV1, SiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(SiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(SiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(SiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(SiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(SiWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(SiWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(SiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    if pending_device_si {
        return Err(SiWriterRuntimeStateErrorV1::PendingDeviceSi);
    }
    if pending_abi_si {
        return Err(SiWriterRuntimeStateErrorV1::PendingAbiSi);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(SiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(SiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let (si_started, si_committed, si_pif_to_dram_committed, si_transition_sha256) =
        validate_si_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = SiWriterRuntimeStateEvidenceV1 {
        schema: SI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("SI runtime-state journal entry count exceeds u64"),
        si_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::SiDma)
                .count(),
        )
        .expect("SI runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        si_started,
        si_committed,
        si_pif_to_dram_committed,
        si_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = si_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedSiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(SiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
fn validate_sp_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_sp_dma: bool,
    pending_device_sp_task: bool,
    pending_abi_sp_work: bool,
) -> Result<ValidatedSpWriterRuntimeStateReceiptV1, SpWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(SpWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(SpWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(SpWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(SpWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(SpWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(SpWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(SpWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(SpWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(SpWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    if pending_device_sp_dma {
        return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpDma);
    }
    if pending_device_sp_task {
        return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpTask);
    }
    if pending_abi_sp_work {
        return Err(SpWriterRuntimeStateErrorV1::PendingAbiSpWork);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(SpWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(SpWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let (
        sp_started,
        sp_queued,
        sp_committed,
        sp_busy_cleared,
        sp_rsp_to_rdram_committed,
        sp_transition_sha256,
    ) = validate_sp_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = SpWriterRuntimeStateEvidenceV1 {
        schema: SP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("SP runtime-state journal entry count exceeds u64"),
        sp_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::SpDma)
                .count(),
        )
        .expect("SP runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        sp_started,
        sp_queued,
        sp_committed,
        sp_busy_cleared,
        sp_rsp_to_rdram_committed,
        sp_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = sp_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedSpWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(SpWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

fn validate_host_abi_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), HostAbiWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(HostAbiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(HostAbiWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(HostAbiWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(HostAbiWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(HostAbiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(HostAbiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_host_abi_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog: Option<&AbiHostFunctionCatalogEvidenceV1>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: Option<&HostAbiWriterTraceV1>,
) -> Result<ValidatedHostAbiWriterRuntimeStateReceiptV1, HostAbiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog) = abi_host_catalog else {
        return Err(HostAbiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(HostAbiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    let Some(trace) = trace else {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    if trace.epoch_id != trace_epoch_id {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
    }
    validate_host_abi_writer_quiescence(state)?;

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(HostAbiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(HostAbiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut stacks = BTreeMap::<ThreadId, Vec<u64>>::new();
    let mut seen_transactions = BTreeMap::<u64, ()>::new();
    let mut traced_sequences = Vec::new();
    let mut transactions_started = 0u64;
    let mut transactions_finished = 0u64;
    let mut ordering_boundaries = 0u64;
    let mut lifecycle = sha2::Sha256::new();
    lifecycle.update(b"fn64:host-abi-writer-lifecycle:v1");
    lifecycle.update(trace.epoch_id.to_be_bytes());
    lifecycle.update(trace.initial_journal_entry_count.to_be_bytes());
    lifecycle.update((trace.events.len() as u64).to_be_bytes());
    for event in &trace.events {
        match event {
            HostAbiWriterTraceEventV1::Started(frame) => {
                let target_is_abi_writer = abi_host_catalog.bindings.iter().any(|binding| {
                    binding.target_pc == frame.target.get()
                        && binding.writer_effects.contains(&WriterChannel::HostAbi)
                });
                if !target_is_abi_writer
                    || seen_transactions.insert(frame.transaction_id, ()).is_some()
                {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                stacks
                    .entry(frame.thread)
                    .or_default()
                    .push(frame.transaction_id);
                transactions_started = transactions_started
                    .checked_add(1)
                    .expect("Host ABI transaction count exceeds u64");
                lifecycle.update([0]);
                lifecycle.update(frame.transaction_id.to_be_bytes());
                lifecycle.update(frame.thread.to_be_bytes());
                lifecycle.update(frame.target.get().to_be_bytes());
                lifecycle.update(frame.resume.bank.get().to_be_bytes());
                lifecycle.update(frame.resume.pc.get().to_be_bytes());
            }
            HostAbiWriterTraceEventV1::Boundary {
                transaction_id,
                thread,
                journal_sequences,
            } => {
                if stacks.get(thread).and_then(|stack| stack.last()).copied()
                    != Some(*transaction_id)
                {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                ordering_boundaries = ordering_boundaries
                    .checked_add(1)
                    .expect("Host ABI ordering-boundary count exceeds u64");
                lifecycle.update([1]);
                lifecycle.update(transaction_id.to_be_bytes());
                lifecycle.update(thread.to_be_bytes());
                lifecycle.update((journal_sequences.len() as u64).to_be_bytes());
                for sequence in journal_sequences {
                    let Ok(index) = usize::try_from(*sequence) else {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    };
                    let Some(entry) = state.entries.get(index) else {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    };
                    if entry.sequence != *sequence
                        || entry
                            .declared_writes
                            .iter()
                            .any(|declaration| declaration.channel != WriterChannel::HostAbi)
                    {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    }
                    traced_sequences.push(*sequence);
                    lifecycle.update(sequence.to_be_bytes());
                }
            }
            HostAbiWriterTraceEventV1::Finished {
                transaction_id,
                thread,
            } => {
                let Some(stack) = stacks.get_mut(thread) else {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                };
                if stack.pop() != Some(*transaction_id) {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                if stack.is_empty() {
                    stacks.remove(thread);
                }
                transactions_finished = transactions_finished
                    .checked_add(1)
                    .expect("Host ABI transaction count exceeds u64");
                lifecycle.update([2]);
                lifecycle.update(transaction_id.to_be_bytes());
                lifecycle.update(thread.to_be_bytes());
            }
        }
    }
    if transactions_started == 0 {
        return Err(HostAbiWriterRuntimeStateErrorV1::NoHostAbiTransactions);
    }
    if !stacks.is_empty() || transactions_finished != transactions_started {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }

    let Ok(initial_index) = usize::try_from(trace.initial_journal_entry_count) else {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    };
    if initial_index > state.entries.len() {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }
    let expected_sequences = state.entries[initial_index..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::HostAbi)
        })
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    if traced_sequences != expected_sequences {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }
    if expected_sequences.is_empty() {
        return Err(HostAbiWriterRuntimeStateErrorV1::NoHostAbiWriteCommit);
    }
    let host_abi_journal_declaration_count = state.entries[initial_index..]
        .iter()
        .flat_map(|entry| &entry.declared_writes)
        .filter(|declaration| declaration.channel == WriterChannel::HostAbi)
        .count();

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = HostAbiWriterRuntimeStateEvidenceV1 {
        schema: HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256: abi_host_catalog.receipt_sha256,
        build_receipt,
        trace_epoch_id,
        initial_journal_entry_count: trace.initial_journal_entry_count,
        final_journal_entry_count: u64::try_from(state.entries.len())
            .expect("Host ABI final journal entry count exceeds u64"),
        watched_ranges,
        host_abi_journal_entry_count: u64::try_from(expected_sequences.len())
            .expect("Host ABI journal entry count exceeds u64"),
        host_abi_journal_declaration_count: u64::try_from(host_abi_journal_declaration_count)
            .expect("Host ABI journal declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        transactions_started,
        transactions_finished,
        ordering_boundaries,
        lifecycle_sha256: lifecycle.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = host_abi_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedHostAbiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(HostAbiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

fn validate_rsp_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), RspWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(RspWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(RspWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(RspWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(RspWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(RspWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(RspWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_rsp_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &crate::task_dispatch::RspWriterTraceSnapshotV1,
    pending_device_rsp_task: bool,
    pending_abi_rsp_work: bool,
) -> Result<ValidatedRspWriterRuntimeStateReceiptV1, RspWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(RspWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(RspWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(RspWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_rsp_writer_quiescence(state)?;
    if pending_device_rsp_task {
        return Err(RspWriterRuntimeStateErrorV1::PendingDeviceRspTask);
    }
    if pending_abi_rsp_work {
        return Err(RspWriterRuntimeStateErrorV1::PendingAbiRspWork);
    }
    if !trace.rejected_journal_sequences.is_empty() {
        return Err(RspWriterRuntimeStateErrorV1::RejectedRspExecutableMutation);
    }
    if trace.commits.is_empty()
        && !trace
            .hle_publications
            .iter()
            .any(|publication| !publication.journal_sequences.is_empty())
    {
        return Err(RspWriterRuntimeStateErrorV1::NoRspWritebacks);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(RspWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(RspWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut interpreter_writeback_count = 0u64;
    let mut translated_audio_hle_publication_count = 0u64;
    let mut trace_hasher = sha2::Sha256::new();
    trace_hasher.update(b"fn64:rsp-execution-writeback-trace:v1");
    trace_hasher.update(trace_epoch_id.to_be_bytes());
    trace_hasher.update((trace.commits.len() as u64).to_be_bytes());
    for observation in &trace.commits {
        if observation.physical_start >= observation.physical_end
            || observation.physical_end > fn64_recomp_rs::RDRAM_LEN as u32
        {
            return Err(RspWriterRuntimeStateErrorV1::InvalidRspWritebackRange);
        }
        let owner = match observation.source {
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { owner } => {
                interpreter_writeback_count = interpreter_writeback_count
                    .checked_add(1)
                    .expect("RSP interpreter writeback count overflow");
                trace_hasher.update([0]);
                owner
            }
            crate::task_dispatch::RspWriterCommitSourceV1::TranslatedAudioHle { .. } => {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspWritebackRange)
            }
        };
        match owner.task_offset() {
            Some(offset) => {
                trace_hasher.update([0]);
                trace_hasher.update(offset.to_be_bytes());
            }
            None => trace_hasher.update([1]),
        }
        trace_hasher.update(owner.admission_generation().get().to_be_bytes());
        trace_hasher.update(observation.physical_start.to_be_bytes());
        trace_hasher.update(observation.physical_end.to_be_bytes());
    }
    trace_hasher.update((trace.hle_publications.len() as u64).to_be_bytes());
    let mut claimed_hle_sequences = std::collections::BTreeSet::new();
    for publication in &trace.hle_publications {
        let owner = match publication.source {
            crate::task_dispatch::RspWriterCommitSourceV1::TranslatedAudioHle { owner } => owner,
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { .. } => {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication)
            }
        };
        translated_audio_hle_publication_count = translated_audio_hle_publication_count
            .checked_add(1)
            .expect("translated audio-HLE publication count overflow");
        trace_hasher.update([1]);
        match owner.task_offset() {
            Some(offset) => {
                trace_hasher.update([0]);
                trace_hasher.update(offset.to_be_bytes());
            }
            None => trace_hasher.update([1]),
        }
        trace_hasher.update(owner.admission_generation().get().to_be_bytes());
        trace_hasher.update((publication.journal_sequences.len() as u64).to_be_bytes());
        for &sequence in &publication.journal_sequences {
            if !claimed_hle_sequences.insert(sequence) {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            }
            let Some(entry) = state
                .entries
                .iter()
                .find(|entry| entry.sequence == sequence)
            else {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            };
            if !entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RspExecutionOrHleWriteback)
            {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            }
            trace_hasher.update(sequence.to_be_bytes());
        }
    }

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = RspWriterRuntimeStateEvidenceV1 {
        schema: RSP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("RSP runtime-state journal entry count exceeds u64"),
        rsp_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| {
                    declaration.channel == WriterChannel::RspExecutionOrHleWriteback
                })
                .count(),
        )
        .expect("RSP runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        interpreter_writeback_count,
        translated_audio_hle_publication_count,
        writeback_range_count: u64::try_from(trace.commits.len())
            .expect("RSP writeback trace exceeds u64"),
        writeback_trace_sha256: trace_hasher.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = rsp_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedRspWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(RspWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

fn validate_rdp_renderer_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), RdpRendererWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::Poisoned);
    }
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingPhysicalWrites);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAttributedWrites);
    }
    if !state.host_transactions.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_rdp_renderer_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    epoch: &RdpRendererWriterRuntimeTraceEpochV1,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &RdpRendererWriterTraceV1,
    pending_device_rsp_task: bool,
    pending_device_dpc_transaction: bool,
    pending_device_dp_completion: bool,
    pending_abi_renderer_work: bool,
) -> Result<ValidatedRdpRendererWriterRuntimeStateReceiptV1, RdpRendererWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(RdpRendererWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    if trace.epoch_id != epoch.epoch_id
        || trace.program_model_sha256 != program_model_sha256
        || epoch.program_model_sha256 != program_model_sha256
    {
        return Err(RdpRendererWriterRuntimeStateErrorV1::TraceEpochMismatch);
    }
    validate_rdp_renderer_writer_quiescence(state)?;
    if pending_device_rsp_task {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask);
    }
    if pending_device_dpc_transaction {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction);
    }
    if pending_device_dp_completion {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion);
    }
    if pending_abi_renderer_work {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork);
    }
    if !trace.rejected_journal_sequences.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    if trace.publications.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NoRendererPublications);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(RdpRendererWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let Ok(initial_index) = usize::try_from(trace.initial_journal_entry_count) else {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    };
    if initial_index > state.entries.len() || trace.next_journal_entry_index > state.entries.len() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    let traced_sequences = trace
        .publications
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let expected_sequences = state.entries[initial_index..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RdpRenderer)
        })
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    if traced_sequences != expected_sequences {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    for sequence in &traced_sequences {
        let Ok(index) = usize::try_from(*sequence) else {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        };
        let Some(entry) = state.entries.get(index) else {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        };
        if entry.sequence != *sequence
            || entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel != WriterChannel::RdpRenderer)
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        }
    }

    let mut publication_trace = sha2::Sha256::new();
    publication_trace.update(b"fn64:rdp-renderer-publication-trace:v1");
    publication_trace.update(trace.epoch_id.to_be_bytes());
    publication_trace.update(trace.initial_journal_entry_count.to_be_bytes());
    publication_trace.update((trace.publications.len() as u64).to_be_bytes());
    for sequences in &trace.publications {
        publication_trace.update((sequences.len() as u64).to_be_bytes());
        for sequence in sequences {
            publication_trace.update(sequence.to_be_bytes());
        }
    }
    let rdp_renderer_journal_declaration_count = state.entries[initial_index..]
        .iter()
        .flat_map(|entry| &entry.declared_writes)
        .filter(|declaration| declaration.channel == WriterChannel::RdpRenderer)
        .count();
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect();
    let mut evidence = RdpRendererWriterRuntimeStateEvidenceV1 {
        schema: RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id: trace.epoch_id,
        initial_journal_entry_count: trace.initial_journal_entry_count,
        final_journal_entry_count: u64::try_from(state.entries.len())
            .expect("RDP renderer final journal entry count exceeds u64"),
        watched_ranges,
        rdp_renderer_journal_entry_count: u64::try_from(expected_sequences.len())
            .expect("RDP renderer journal entry count exceeds u64"),
        rdp_renderer_journal_declaration_count: u64::try_from(
            rdp_renderer_journal_declaration_count,
        )
        .expect("RDP renderer journal declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        renderer_publication_count: u64::try_from(trace.publications.len())
            .expect("RDP renderer publication count exceeds u64"),
        publication_trace_sha256: publication_trace.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = rdp_renderer_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedRdpRendererWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

impl CanonicalExecutableMutationStateV1 {
    fn new(ranges: &[(u32, u32)]) -> Self {
        assert!(
            !ranges.is_empty(),
            "canonical mutation state requires executable backing"
        );
        let mut watched = Vec::with_capacity(ranges.len());
        let mut previous_end = 0;
        for &(physical_start, physical_end) in ranges {
            assert!(
                physical_start < physical_end
                    && physical_end <= fn64_recomp_rs::RDRAM_LEN as u32
                    && (watched.is_empty() || physical_start > previous_end),
                "canonical executable mutation range is invalid or non-canonical: [{physical_start:#010x}, {physical_end:#010x})"
            );
            watched.push(WatchedExecutableBytesV1 {
                physical_start,
                physical_end,
                expected: Vec::new(),
            });
            previous_end = physical_end;
        }
        Self {
            watched,
            sealed: false,
            expected_sha256: None,
            entries: Vec::new(),
            journal_root_sha256: [0; 32],
            next_sequence: 0,
            next_transaction_id: 0,
            host_transactions: BTreeMap::new(),
            host_abi_writer_trace: None,
            next_child_transaction_id: 0,
            active_child_transaction: None,
            poison: None,
        }
    }

    fn assert_not_poisoned(&self) {
        if let Some(reason) = &self.poison {
            recompiled_gap_panic(format!(
                "canonical executable mutation owner is poisoned: {reason}"
            ));
        }
    }

    fn poison(&mut self, reason: String) {
        if self.poison.is_none() {
            self.poison = Some(reason);
        }
    }

    fn begin_child_transaction(&mut self) -> u64 {
        self.assert_not_poisoned();
        assert!(
            self.active_child_transaction.is_none(),
            "canonical executable mutation owner already has an active child writer transaction"
        );
        let id = self.next_child_transaction_id;
        self.next_child_transaction_id = self
            .next_child_transaction_id
            .checked_add(1)
            .expect("canonical child writer transaction id overflow");
        self.active_child_transaction = Some(id);
        id
    }

    fn assert_active_child_transaction(&self, id: u64) {
        self.assert_not_poisoned();
        assert_eq!(
            self.active_child_transaction,
            Some(id),
            "canonical child writer transaction {id} is not the active owner"
        );
    }

    fn finish_child_transaction(&mut self, id: u64) {
        self.assert_active_child_transaction(id);
        self.active_child_transaction = None;
    }

    fn begin_host_transaction(
        &mut self,
        thread: ThreadId,
        target: GuestPc,
        resume: ExecutionKey,
    ) -> HostMutationTransactionTokenV1 {
        self.assert_not_poisoned();
        let transaction_id = self.next_transaction_id;
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(1)
            .expect("canonical host mutation transaction id overflow");
        let frame = OpenHostMutationTransactionEvidenceV1 {
            transaction_id,
            thread,
            target,
            resume,
        };
        self.host_transactions
            .entry(thread)
            .or_default()
            .push(frame);
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Started(frame));
        }
        HostMutationTransactionTokenV1 {
            transaction_id,
            thread,
        }
    }

    fn active_host_transaction(&self, thread: ThreadId) -> Option<HostMutationTransactionTokenV1> {
        self.host_transactions
            .get(&thread)
            .and_then(|stack| stack.last())
            .map(|frame| HostMutationTransactionTokenV1 {
                transaction_id: frame.transaction_id,
                thread,
            })
    }

    fn assert_active_host_transaction(&self, token: HostMutationTransactionTokenV1) {
        self.assert_not_poisoned();
        let actual = self
            .active_host_transaction(token.thread)
            .unwrap_or_else(|| {
                recompiled_gap_panic(format!(
                    "host mutation transaction {} for thread {} is not active",
                    token.transaction_id, token.thread
                ))
            });
        if actual != token {
            recompiled_gap_panic(format!(
                "host mutation transaction stack mismatch for thread {}: expected top {}, received {}",
                token.thread, actual.transaction_id, token.transaction_id
            ));
        }
    }

    fn finish_host_transaction(&mut self, token: HostMutationTransactionTokenV1) {
        self.assert_active_host_transaction(token);
        let stack = self
            .host_transactions
            .get_mut(&token.thread)
            .expect("active host transaction stack disappeared");
        let frame = stack.pop().expect("active host transaction stack is empty");
        assert_eq!(frame.transaction_id, token.transaction_id);
        if stack.is_empty() {
            self.host_transactions.remove(&token.thread);
        }
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Finished {
                transaction_id: token.transaction_id,
                thread: token.thread,
            });
        }
    }

    fn record_host_abi_boundary(
        &mut self,
        token: HostMutationTransactionTokenV1,
        first_new_entry: usize,
    ) {
        self.assert_active_host_transaction(token);
        let journal_sequences = self.entries[first_new_entry..]
            .iter()
            .map(|entry| {
                assert!(
                    entry
                        .declared_writes
                        .iter()
                        .all(|declaration| declaration.channel == WriterChannel::HostAbi),
                    "Host ABI ordering boundary committed a non-HostAbi declaration"
                );
                entry.sequence
            })
            .collect();
        if let Some(trace) = &mut self.host_abi_writer_trace {
            trace.events.push(HostAbiWriterTraceEventV1::Boundary {
                transaction_id: token.transaction_id,
                thread: token.thread,
                journal_sequences,
            });
        }
    }

    fn from_bootstrap(evidence: &BootstrapOrImportValidationEvidenceV1, storage: &[u8]) -> Self {
        let ranges = evidence
            .watched_ranges
            .iter()
            .map(|range| (range.physical_start, range.physical_end))
            .collect::<Vec<_>>();
        assert_eq!(
            watched_bytes_sha256(storage, &ranges),
            evidence.watched_sha256,
            "validated bootstrap watched bytes changed before journal initialization"
        );
        let mut state = Self::new(&ranges);
        state.seal_with(|_| 0);
        let view = fn64_runtime::RdramView::from_storage(storage);
        let snapshot = state
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        let events = evidence
            .publications
            .iter()
            .map(|publication| GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: publication.physical_start,
                len: publication.physical_end - publication.physical_start,
            })
            .collect();
        state.commit_snapshot(snapshot, events, Vec::new());
        state
    }

    fn required_physical_end(&self) -> u32 {
        self.watched
            .last()
            .expect("canonical mutation state has no watched ranges")
            .physical_end
    }

    fn read_snapshot(&self, mut read_physical_byte: impl FnMut(u32) -> u8) -> Vec<Vec<u8>> {
        self.watched
            .iter()
            .map(|range| {
                (range.physical_start..range.physical_end)
                    .map(&mut read_physical_byte)
                    .collect()
            })
            .collect()
    }

    fn digest_snapshot(&self, snapshot: &[Vec<u8>]) -> [u8; 32] {
        let mut digest = sha2::Sha256::new();
        for (range, bytes) in self.watched.iter().zip(snapshot) {
            digest.update(range.physical_start.to_be_bytes());
            digest.update(range.physical_end.to_be_bytes());
            digest.update(bytes);
        }
        digest.finalize().into()
    }

    fn seal_with(&mut self, read_physical_byte: impl FnMut(u32) -> u8) {
        if self.sealed {
            return;
        }
        let snapshot = self.read_snapshot(read_physical_byte);
        let expected_sha256 = self.digest_snapshot(&snapshot);
        for (range, bytes) in self.watched.iter_mut().zip(snapshot) {
            range.expected = bytes;
        }
        self.journal_root_sha256 = canonical_mutation_initial_root(
            expected_sha256,
            self.watched
                .iter()
                .map(|range| PendingExecutableWriteEvidenceSnapshot {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                }),
        );
        self.expected_sha256 = Some(expected_sha256);
        self.sealed = true;
    }

    fn current_changed_ranges(&self, snapshot: &[Vec<u8>]) -> Vec<(u32, u32)> {
        let mut changed = Vec::new();
        for (range, current) in self.watched.iter().zip(snapshot) {
            assert_eq!(range.expected.len(), current.len());
            let mut index = 0;
            while index < current.len() {
                if range.expected[index] == current[index] {
                    index += 1;
                    continue;
                }
                let start = index;
                index += 1;
                while index < current.len() && range.expected[index] != current[index] {
                    index += 1;
                }
                changed.push((
                    range.physical_start + start as u32,
                    range.physical_start + index as u32,
                ));
            }
        }
        changed
    }

    fn clipped_declarations(
        &self,
        events: &[GuestWriteEvent],
    ) -> Vec<AttributedExecutableWriteEvidenceV1> {
        let mut declarations = Vec::new();
        for &event in events {
            let (physical_start, byte_len) = event.range();
            let physical_end = physical_start.checked_add(byte_len).unwrap_or_else(|| {
                recompiled_gap_panic(format!(
                    "attributed executable write overflows: {physical_start:#010x} + {byte_len:#x}"
                ))
            });
            for watched in &self.watched {
                let start = physical_start.max(watched.physical_start);
                let end = physical_end.min(watched.physical_end);
                if start < end {
                    declarations.push(AttributedExecutableWriteEvidenceV1 {
                        channel: event.channel(),
                        physical_start: start,
                        physical_end: end,
                    });
                }
            }
        }
        declarations
    }

    fn first_uncovered_changed_range(
        declarations: &[AttributedExecutableWriteEvidenceV1],
        changed: &[(u32, u32)],
    ) -> Option<(u32, u32)> {
        let mut intervals = declarations
            .iter()
            .map(|declaration| (declaration.physical_start, declaration.physical_end))
            .collect::<Vec<_>>();
        intervals.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(intervals.len());
        for (start, end) in intervals {
            if let Some((_, previous_end)) = merged.last_mut() {
                if start <= *previous_end {
                    *previous_end = (*previous_end).max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }

        let mut interval_index = 0;
        for &(changed_start, changed_end) in changed {
            while interval_index < merged.len() && merged[interval_index].1 <= changed_start {
                interval_index += 1;
            }
            let mut cursor = changed_start;
            let mut candidate = interval_index;
            while candidate < merged.len() && merged[candidate].0 <= cursor {
                cursor = cursor.max(merged[candidate].1);
                if cursor >= changed_end {
                    break;
                }
                candidate += 1;
            }
            if cursor < changed_end {
                return Some((cursor, changed_end));
            }
        }
        None
    }

    fn reconcile_snapshot_before_dispatch(&mut self, snapshot: Vec<Vec<u8>>) {
        self.assert_not_poisoned();
        assert!(
            self.sealed,
            "canonical executable mutation state is not sealed"
        );
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|writes| writes.borrow().len());
        assert_eq!(
            pending, 0,
            "canonical executable dispatch attempted with {pending} attributed write(s) not yet invalidated"
        );
        if let Some((physical_start, physical_end)) =
            self.current_changed_ranges(&snapshot).into_iter().next()
        {
            recompiled_gap_panic(format!(
                "unjournaled executable mutation changed physical RDRAM [{physical_start:#010x}, {physical_end:#010x}) before canonical static dispatch"
            ));
        }
    }

    fn commit_snapshot(
        &mut self,
        snapshot: Vec<Vec<u8>>,
        events: Vec<GuestWriteEvent>,
        mut invalidated_generations: Vec<GenerationId>,
    ) {
        self.assert_not_poisoned();
        assert!(
            self.sealed,
            "canonical executable mutation state is not sealed"
        );
        let changed = self.current_changed_ranges(&snapshot);
        let declarations = self.clipped_declarations(&events);
        if let Some((physical_start, physical_end)) =
            Self::first_uncovered_changed_range(&declarations, &changed)
        {
            recompiled_gap_panic(format!(
                "executable mutation changed physical RDRAM [{physical_start:#010x}, {physical_end:#010x}) outside every attributed writer declaration"
            ));
        }
        if declarations.is_empty() && changed.is_empty() {
            return;
        }

        let before_sha256 = self
            .expected_sha256
            .expect("sealed mutation state has no expected digest");
        let after_sha256 = self.digest_snapshot(&snapshot);
        invalidated_generations.sort_unstable();
        invalidated_generations.dedup();
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("canonical executable mutation sequence overflow");
        let mut entry = ExecutableMutationBatchEvidenceV1 {
            sequence,
            declared_writes: declarations,
            changed_ranges: changed
                .into_iter()
                .map(
                    |(physical_start, physical_end)| PendingExecutableWriteEvidenceSnapshot {
                        physical_start,
                        physical_end,
                    },
                )
                .collect(),
            before_sha256,
            after_sha256,
            invalidated_generations,
            journal_root_sha256: [0; 32],
        };
        entry.journal_root_sha256 = canonical_mutation_entry_root(self.journal_root_sha256, &entry);
        let journal_root_sha256 = entry.journal_root_sha256;
        self.entries.push(entry);
        for (range, bytes) in self.watched.iter_mut().zip(snapshot) {
            range.expected = bytes;
        }
        self.expected_sha256 = Some(after_sha256);
        self.journal_root_sha256 = journal_root_sha256;
    }

    fn evidence_snapshot(&self) -> CanonicalExecutableMutationJournalEvidenceV1 {
        let open_host_transactions = self
            .host_transactions
            .values()
            .flat_map(|stack| stack.iter().copied())
            .collect();
        CanonicalExecutableMutationJournalEvidenceV1 {
            schema: CANONICAL_EXECUTABLE_MUTATION_JOURNAL_SCHEMA_V1.to_string(),
            watched_ranges: self
                .watched
                .iter()
                .map(|range| PendingExecutableWriteEvidenceSnapshot {
                    physical_start: range.physical_start,
                    physical_end: range.physical_end,
                })
                .collect(),
            sealed: self.sealed,
            expected_sha256: self.expected_sha256,
            entries: self.entries.clone(),
            journal_root_sha256: self.journal_root_sha256,
            pending_attributed_writes: PENDING_ATTRIBUTED_EXECUTABLE_WRITES
                .with(|writes| writes.borrow().len()),
            open_host_transactions,
        }
    }
}

impl CanonicalLiveBlockProgramV1 {
    fn charge_canonical_instructions(&self, instructions: u32) {
        assert!(
            instructions > 0,
            "canonical instruction charge must be nonzero"
        );
        let charged = self
            .canonical_charged_instructions
            .get()
            .checked_add(u64::from(instructions))
            .expect("canonical BlockProgram instruction count overflow");
        if let Some(limit) = self.canonical_instruction_limit.get() {
            assert!(
                charged <= limit,
                "canonical BlockProgram exceeded exact instruction limit {limit}: charged {charged}"
            );
        }
        self.canonical_charged_instructions.set(charged);
    }

    fn next_dispatch_budget(&self) -> InstructionBudget {
        let configured = self.install.budget();
        let Some(limit) = self.canonical_instruction_limit.get() else {
            return configured;
        };
        let charged = self.canonical_charged_instructions.get();
        let remaining = limit.checked_sub(charged).unwrap_or_else(|| {
            recompiled_gap_panic(format!(
                "canonical instruction limit {limit} is behind charged work {charged}"
            ))
        });
        if remaining == 0 {
            recompiled_gap_panic(format!(
                "canonical exact checkpoint limit {limit} was already reached"
            ));
        }
        let remaining = u32::try_from(remaining).unwrap_or(u32::MAX);
        InstructionBudget::new(configured.get().min(remaining))
            .expect("canonical exact checkpoint budget was checked against the minimum")
    }

    fn publish_checkpoint(
        &self,
        instructions: u32,
        exit: BlockExit,
        prepared_continuation: Option<CanonicalPreparedContinuationV1>,
        ctx: &RsContext,
    ) {
        let thread = super::current_thread_id("canonical checkpoint publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::Exact(CanonicalThreadCheckpointEvidenceV1 {
                thread,
                cpu: ctx.evidence_snapshot_v1(),
                charged_instructions: instructions,
                canonical_charged_instructions_at_publication: self
                    .canonical_charged_instructions
                    .get(),
                pending_exit: exit,
                prepared_continuation,
            }),
        );
    }

    fn publish_opaque_host(&self, target: GuestPc, resume: ExecutionKey) {
        let thread = super::current_thread_id("canonical host publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::OpaqueHostInFlight {
                thread,
                target,
                resume,
            },
        );
    }

    fn publish_parked_fault(&self, fault: CpuFault, ctx: &RsContext) {
        let thread = super::current_thread_id("canonical parked-fault publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::ParkedFaultOpaque {
                thread,
                post_exception_cpu: ctx.evidence_snapshot_v1(),
                fault,
                canonical_charged_instructions_at_publication: self
                    .canonical_charged_instructions
                    .get(),
            },
        );
    }

    fn publish_returned(&self, ctx: &RsContext) {
        let thread = super::current_thread_id("canonical return publication");
        self.thread_publications.borrow_mut().insert(
            thread,
            CanonicalThreadPublicationV1::Returned {
                thread,
                cpu: ctx.evidence_snapshot_v1(),
            },
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn enable_dynamic_mapped_execution(&self) {
        let mut dynamic = self.dynamic_units.borrow_mut();
        assert!(
            dynamic.is_none(),
            "dynamic mapped execution is already installed"
        );
        *dynamic = Some(fn64_recomp_rs::DynamicMappedUnitCatalogV1::new_linked());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn enable_dynamic_mapped_execution_with_exact_static_key_withheld(
        &self,
        selected: ExecutionKey,
    ) {
        assert_eq!(
            selected,
            self.install.entry(),
            "operational exact-key withholding must select the canonical catalog entry {} rather than {selected}",
            self.install.entry()
        );
        let resolved = self.resolve_transfer(selected.bank, selected.pc).unwrap_or_else(|fault| {
            recompiled_gap_panic(format!(
                "operational exact-key withholding selected {selected}, which is absent from the installed static catalog: {fault}"
            ))
        });
        assert_eq!(
            resolved, selected,
            "operational exact-key withholding selected {selected}, but the installed static catalog resolves that address as {resolved}"
        );
        self.enable_dynamic_mapped_execution();
        self.dynamic_withheld_static_key.set(Some(selected));
    }

    fn dynamic_execution_installed(&self) -> bool {
        #[cfg(feature = "dynamic-mapped-runtime")]
        {
            self.dynamic_units.borrow().is_some()
        }
        #[cfg(not(feature = "dynamic-mapped-runtime"))]
        {
            false
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn record_dynamic_execution(
        &self,
        attempted_entry: ExecutionKey,
        run: &fn64_recomp_rs::DynamicMappedRunV1,
    ) {
        let charged_instructions = u64::from(run.run.instructions);
        let unsupported_exit = matches!(
            run.run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnsupportedInstruction { .. },
                ..
            })
        );
        let mutation_sequence = self
            .mutation_state
            .as_ref()
            .and_then(|state| state.borrow().entries.last().map(|entry| entry.sequence));
        let mut aggregates = self.dynamic_execution_aggregates.borrow_mut();
        if !aggregates.contains_key(&run.identity)
            && aggregates.len() >= DYNAMIC_EXECUTION_AGGREGATE_CAPACITY
        {
            self.dynamic_dropped_identity_activations.set(
                self.dynamic_dropped_identity_activations
                    .get()
                    .checked_add(1)
                    .expect("dropped dynamic identity activation count overflow"),
            );
            self.dynamic_dropped_identity_charged_instructions.set(
                self.dynamic_dropped_identity_charged_instructions
                    .get()
                    .checked_add(charged_instructions)
                    .expect("dropped dynamic identity instruction count overflow"),
            );
            if unsupported_exit {
                self.dynamic_dropped_identity_unsupported_exits.set(
                    self.dynamic_dropped_identity_unsupported_exits
                        .get()
                        .checked_add(1)
                        .expect("dropped dynamic identity unsupported-exit count overflow"),
                );
            }
            return;
        }
        let aggregate =
            aggregates
                .entry(run.identity)
                .or_insert_with(|| DynamicMappedExecutionAggregateV1 {
                    identity: run.identity,
                    admitted_entry: run.entry,
                    instructions: run.instructions.clone(),
                    attempted_entries: Vec::new(),
                    activations: 0,
                    charged_instructions: 0,
                    unsupported_exits: 0,
                    first_mutation_sequence: mutation_sequence,
                    last_mutation_sequence: mutation_sequence,
                    last_exit: run.run.exit,
                });
        assert_eq!(
            aggregate.admitted_entry.bank, run.entry.bank,
            "dynamic identity changed its content-derived bank"
        );
        assert_eq!(
            aggregate.instructions, run.instructions,
            "dynamic identity changed its physical instruction set"
        );
        match aggregate
            .attempted_entries
            .binary_search_by_key(&attempted_entry, |entry| entry.attempted_entry)
        {
            Ok(index) => {
                let entry = &mut aggregate.attempted_entries[index];
                entry.activations = entry
                    .activations
                    .checked_add(1)
                    .expect("dynamic attempted-entry activation count overflow");
                entry.charged_instructions = entry
                    .charged_instructions
                    .checked_add(charged_instructions)
                    .expect("dynamic attempted-entry instruction count overflow");
                if unsupported_exit {
                    entry.unsupported_exits = entry
                        .unsupported_exits
                        .checked_add(1)
                        .expect("dynamic attempted-entry unsupported-exit count overflow");
                }
            }
            Err(index)
                if aggregate.attempted_entries.len()
                    < DYNAMIC_ATTEMPTED_ENTRIES_PER_AGGREGATE_CAPACITY =>
            {
                aggregate.attempted_entries.insert(
                    index,
                    DynamicMappedEntryCountV1 {
                        attempted_entry,
                        activations: 1,
                        charged_instructions,
                        unsupported_exits: u64::from(unsupported_exit),
                    },
                )
            }
            Err(_) => {
                self.dynamic_dropped_attempted_entry_activations.set(
                    self.dynamic_dropped_attempted_entry_activations
                        .get()
                        .checked_add(1)
                        .expect("dropped dynamic attempted-entry activation count overflow"),
                );
                self.dynamic_dropped_attempted_entry_charged_instructions
                    .set(
                        self.dynamic_dropped_attempted_entry_charged_instructions
                            .get()
                            .checked_add(charged_instructions)
                            .expect("dropped dynamic attempted-entry instruction count overflow"),
                    );
                if unsupported_exit {
                    self.dynamic_dropped_attempted_entry_unsupported_exits.set(
                        self.dynamic_dropped_attempted_entry_unsupported_exits
                            .get()
                            .checked_add(1)
                            .expect(
                                "dropped dynamic attempted-entry unsupported-exit count overflow",
                            ),
                    );
                }
            }
        }
        aggregate.activations = aggregate
            .activations
            .checked_add(1)
            .expect("dynamic activation count overflow");
        aggregate.charged_instructions = aggregate
            .charged_instructions
            .checked_add(charged_instructions)
            .expect("dynamic retired-instruction count overflow");
        if unsupported_exit {
            aggregate.unsupported_exits = aggregate
                .unsupported_exits
                .checked_add(1)
                .expect("dynamic unsupported-exit count overflow");
        }
        aggregate.last_mutation_sequence = mutation_sequence.or(aggregate.last_mutation_sequence);
        aggregate.last_exit = run.run.exit;
    }

    fn mint_bootstrap_writer_completion(
        &self,
        storage: &[u8],
    ) -> Result<(), BootstrapWriterChannelCompletionErrorV1> {
        if self.dynamic_execution_installed() {
            return Err(BootstrapWriterChannelCompletionErrorV1::DynamicExecutionInstalled);
        }
        let bootstrap = self
            .bootstrap_evidence
            .as_ref()
            .expect("bootstrap writer completion requires bootstrap evidence");
        let state = self
            .mutation_state
            .as_ref()
            .expect("bootstrap writer completion requires mutation state")
            .borrow();
        let receipt = validate_bootstrap_writer_completion_state(
            self.writer_program_model_sha256,
            bootstrap,
            storage,
            &state,
        )?;
        let mut slot = self.bootstrap_writer_completion.borrow_mut();
        assert!(
            slot.is_none(),
            "bootstrap writer-channel completion authority was already minted"
        );
        *slot = Some(receipt);
        Ok(())
    }

    fn begin_cpu_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<CpuWriterRuntimeTraceEpochV1>, CpuWriterRuntimeStateErrorV1> {
        if self.cpu_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(CpuWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(CpuWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(CpuWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_cpu_writer_quiescence(&state)?;
        drop(state);

        let epoch_id = next_cpu_writer_trace_epoch_id();
        self.cpu_writer_trace_epoch_id.set(Some(epoch_id));
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            *trace.borrow_mut() = Some(CpuInstructionStoreTraceV1 {
                epoch_id,
                events: Vec::new(),
            });
        });
        Ok(Some(CpuWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    fn take_cpu_writer_runtime_state(
        &self,
        epoch: &CpuWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
    ) -> Result<Option<ValidatedCpuWriterRuntimeStateReceiptV1>, CpuWriterRuntimeStateErrorV1> {
        if self.cpu_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.cpu_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(CpuWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            let trace = trace.borrow();
            let trace = trace
                .as_ref()
                .ok_or(CpuWriterRuntimeStateErrorV1::TraceEpochNotArmed)?;
            if trace.epoch_id != epoch.epoch_id {
                return Err(CpuWriterRuntimeStateErrorV1::TraceEpochMismatch);
            }
            Ok(trace.events.clone())
        })?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(CpuWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_cpu_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            &trace,
        )?;
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
        self.cpu_writer_trace_epoch_id.set(None);
        self.cpu_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn begin_host_abi_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<HostAbiWriterRuntimeTraceEpochV1>, HostAbiWriterRuntimeStateErrorV1> {
        if self.host_abi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(HostAbiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(HostAbiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(HostAbiWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let mut state = self
            .mutation_state
            .as_ref()
            .ok_or(HostAbiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow_mut();
        validate_host_abi_writer_quiescence(&state)?;
        let epoch_id = next_host_abi_writer_trace_epoch_id();
        state.host_abi_writer_trace = Some(HostAbiWriterTraceV1 {
            epoch_id,
            initial_journal_entry_count: u64::try_from(state.entries.len())
                .expect("Host ABI initial journal entry count exceeds u64"),
            events: Vec::new(),
        });
        Ok(Some(HostAbiWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    fn take_host_abi_writer_runtime_state(
        &self,
        epoch: &HostAbiWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
    ) -> Result<Option<ValidatedHostAbiWriterRuntimeStateReceiptV1>, HostAbiWriterRuntimeStateErrorV1>
    {
        if self.host_abi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256 {
            return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(HostAbiWriterRuntimeStateErrorV1::Unsealed)?;
        let trace = state.borrow().host_abi_writer_trace.clone();
        if trace.as_ref().map(|trace| trace.epoch_id) != Some(epoch.epoch_id) {
            return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog = self
            .install
            .evidence()
            .abi_host_catalog
            .as_ref()
            .filter(|_| self.install.has_abi_host_catalog_authority());
        let receipt = validate_host_abi_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state.borrow(),
            trace.as_ref(),
        )?;
        state.borrow_mut().host_abi_writer_trace = None;
        self.host_abi_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn begin_rsp_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<RspWriterRuntimeTraceEpochV1>, RspWriterRuntimeStateErrorV1> {
        if self.rsp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(RspWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(RspWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(RspWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RspWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_rsp_writer_quiescence(&state)?;
        drop(state);

        let epoch_id = next_rsp_writer_trace_epoch_id();
        self.rsp_writer_trace_epoch_id.set(Some(epoch_id));
        crate::task_dispatch::begin_rsp_writer_trace_v1(epoch_id);
        Ok(Some(RspWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn take_rsp_writer_runtime_state(
        &self,
        epoch: &RspWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        pending_device_rsp_task: bool,
        pending_abi_rsp_work: bool,
    ) -> Result<Option<ValidatedRspWriterRuntimeStateReceiptV1>, RspWriterRuntimeStateErrorV1> {
        if self.rsp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.rsp_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(RspWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = crate::task_dispatch::rsp_writer_trace_snapshot_v1(epoch.epoch_id)
            .ok_or(RspWriterRuntimeStateErrorV1::TraceEpochMismatch)?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RspWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_rsp_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            &trace,
            pending_device_rsp_task,
            pending_abi_rsp_work,
        )?;
        assert!(
            crate::task_dispatch::finish_rsp_writer_trace_v1(epoch.epoch_id),
            "validated RSP writer trace lost its exact epoch before consume"
        );
        self.rsp_writer_trace_epoch_id.set(None);
        self.rsp_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn begin_rdp_renderer_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<RdpRendererWriterRuntimeTraceEpochV1>, RdpRendererWriterRuntimeStateErrorV1>
    {
        if self.rdp_renderer_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RdpRendererWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_rdp_renderer_writer_quiescence(&state)?;
        let initial_journal_entry_count = u64::try_from(state.entries.len())
            .expect("RDP renderer initial journal entry count exceeds u64");
        let next_journal_entry_index = state.entries.len();
        drop(state);

        // Interleaving closed: OS thread A can retain a move-only epoch while
        // OS thread B installs an identical program model. A thread-local
        // counter could mint the same identity in both threads, allowing A's
        // token to consume B's trace arm; this process-global epoch cannot.
        let epoch_id = next_rdp_renderer_writer_trace_epoch_id();
        self.rdp_renderer_writer_trace_epoch_id.set(Some(epoch_id));
        RDP_RENDERER_WRITER_TRACE.with(|trace| {
            *trace.borrow_mut() = Some(RdpRendererWriterTraceV1 {
                epoch_id,
                program_model_sha256: self.writer_program_model_sha256,
                initial_journal_entry_count,
                next_journal_entry_index,
                publications: Vec::new(),
                rejected_journal_sequences: Vec::new(),
            });
        });
        Ok(Some(RdpRendererWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn take_rdp_renderer_writer_runtime_state(
        &self,
        epoch: &RdpRendererWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        pending_device_rsp_task: bool,
        pending_device_dpc_transaction: bool,
        pending_device_dp_completion: bool,
        pending_abi_renderer_work: bool,
    ) -> Result<
        Option<ValidatedRdpRendererWriterRuntimeStateReceiptV1>,
        RdpRendererWriterRuntimeStateErrorV1,
    > {
        if self.rdp_renderer_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.rdp_renderer_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let trace = RDP_RENDERER_WRITER_TRACE.with(|trace| {
            trace
                .borrow()
                .clone()
                .ok_or(RdpRendererWriterRuntimeStateErrorV1::TraceEpochNotArmed)
        })?;
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(RdpRendererWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_rdp_renderer_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            epoch,
            storage,
            &state,
            &trace,
            pending_device_rsp_task,
            pending_device_dpc_transaction,
            pending_device_dp_completion,
            pending_abi_renderer_work,
        )?;
        RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
        self.rdp_renderer_writer_trace_epoch_id.set(None);
        self.rdp_renderer_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn begin_pi_writer_runtime_trace_epoch(
        &self,
        pending_device_pi: bool,
        pending_abi_pi: bool,
        pending_pi_interrupt: bool,
    ) -> Result<Option<PiWriterRuntimeTraceEpochV1>, PiWriterRuntimeStateErrorV1> {
        if self.pi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(PiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(PiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(PiWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(PiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        validate_pi_writer_quiescence(&state)?;
        if pending_device_pi {
            return Err(PiWriterRuntimeStateErrorV1::PendingDevicePi);
        }
        if pending_abi_pi {
            return Err(PiWriterRuntimeStateErrorV1::PendingAbiPi);
        }
        if pending_pi_interrupt {
            return Err(PiWriterRuntimeStateErrorV1::PendingPiInterrupt);
        }
        drop(state);

        let epoch_id = next_pi_writer_trace_epoch_id();
        self.pi_writer_trace_epoch_id.set(Some(epoch_id));
        Ok(Some(PiWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn take_pi_writer_runtime_state(
        &self,
        epoch: &PiWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_pi: bool,
        pending_abi_pi: bool,
    ) -> Result<Option<ValidatedPiWriterRuntimeStateReceiptV1>, PiWriterRuntimeStateErrorV1> {
        if self.pi_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.pi_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(PiWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(PiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_pi_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            trace,
            pending_device_pi,
            pending_abi_pi,
        )?;
        self.pi_writer_trace_epoch_id.set(None);
        self.pi_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    #[allow(clippy::too_many_arguments)]
    fn take_si_writer_runtime_state(
        &self,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_si: bool,
        pending_abi_si: bool,
    ) -> Result<Option<ValidatedSiWriterRuntimeStateReceiptV1>, SiWriterRuntimeStateErrorV1> {
        if self.si_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(SiWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SiWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_si_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            storage,
            &state,
            trace,
            pending_device_si,
            pending_abi_si,
        )?;
        self.si_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn begin_sp_writer_runtime_trace_epoch(
        &self,
    ) -> Result<Option<SpWriterRuntimeTraceEpochV1>, SpWriterRuntimeStateErrorV1> {
        if self.sp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if self.dynamic_execution_installed() {
            return Err(SpWriterRuntimeStateErrorV1::DynamicExecutionInstalled);
        }
        if self.bootstrap_evidence.is_none() {
            return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if !self.install.has_abi_host_catalog_authority() {
            return Err(SpWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
        }
        if !catalog_resolver_feature_lane_eligible(self.install.evidence().build_receipt) {
            return Err(SpWriterRuntimeStateErrorV1::NonProductionAotBuild);
        }
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SpWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        if !state.sealed || state.expected_sha256.is_none() {
            return Err(SpWriterRuntimeStateErrorV1::Unsealed);
        }
        if state.poison.is_some() {
            return Err(SpWriterRuntimeStateErrorV1::Poisoned);
        }
        if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
            return Err(SpWriterRuntimeStateErrorV1::PendingPhysicalWrites);
        }
        if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
            return Err(SpWriterRuntimeStateErrorV1::PendingAttributedWrites);
        }
        if !state.host_transactions.is_empty() {
            return Err(SpWriterRuntimeStateErrorV1::OpenHostTransactions);
        }
        if state.active_child_transaction.is_some() {
            return Err(SpWriterRuntimeStateErrorV1::ActiveChildTransaction);
        }
        drop(state);

        let epoch_id = next_sp_writer_trace_epoch_id();
        self.sp_writer_trace_epoch_id.set(Some(epoch_id));
        Ok(Some(SpWriterRuntimeTraceEpochV1 {
            epoch_id,
            program_model_sha256: self.writer_program_model_sha256,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn take_sp_writer_runtime_state(
        &self,
        epoch: &SpWriterRuntimeTraceEpochV1,
        storage: &[u8],
        validated_owned_bootstrap: bool,
        trace: &[fn64_runtime::DeviceTraceEvent],
        pending_device_sp_dma: bool,
        pending_device_sp_task: bool,
        pending_abi_sp_work: bool,
    ) -> Result<Option<ValidatedSpWriterRuntimeStateReceiptV1>, SpWriterRuntimeStateErrorV1> {
        if self.sp_writer_runtime_state_taken.get() {
            return Ok(None);
        }
        if epoch.program_model_sha256 != self.writer_program_model_sha256
            || self.sp_writer_trace_epoch_id.get() != Some(epoch.epoch_id)
        {
            return Err(SpWriterRuntimeStateErrorV1::TraceEpochMismatch);
        }
        let abi_host_catalog_receipt_sha256 =
            self.install.has_abi_host_catalog_authority().then(|| {
                self.install
                    .evidence()
                    .abi_host_catalog
                    .as_ref()
                    .expect("validated ABI host authority lost its evidence")
                    .receipt_sha256
            });
        let state = self
            .mutation_state
            .as_ref()
            .ok_or(SpWriterRuntimeStateErrorV1::Unsealed)?
            .borrow();
        let receipt = validate_sp_writer_runtime_state_v1(
            self.writer_program_model_sha256,
            resolver_install_definition_sha256(&self.install),
            abi_host_catalog_receipt_sha256,
            self.install.evidence().build_receipt,
            validated_owned_bootstrap,
            Some(epoch.epoch_id),
            storage,
            &state,
            trace,
            pending_device_sp_dma,
            pending_device_sp_task,
            pending_abi_sp_work,
        )?;
        self.sp_writer_trace_epoch_id.set(None);
        self.sp_writer_runtime_state_taken.set(true);
        Ok(Some(receipt))
    }

    fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        if let Some(generations) = &self.generations {
            return self
                .install
                .resolve_entry_with_generations(target_pc, &generations.borrow());
        }
        self.install.resolve_entry(target_pc)
    }

    fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if let Some(generations) = &self.generations {
            return self.install.resolve_transfer_with_generations(
                source_bank,
                target_pc,
                &generations.borrow(),
            );
        }
        self.install.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<CatalogCallResolutionV1, CpuFault> {
        if let Some(host) = self.install.resolve_host(target_pc.get()) {
            Ok(CatalogCallResolutionV1::Host(host))
        } else {
            self.resolve_transfer(source_bank, target_pc)
                .map(CatalogCallResolutionV1::Guest)
        }
    }

    fn dispatch_exposing_exceptions_at_budget(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> Result<fn64_recomp_rs::DispatchRun, fn64_recomp_rs::DispatchError> {
        if let Some(generations) = &self.generations {
            return self
                .install
                .dispatch_exposing_exceptions_with_generations_at_budget(
                    entry,
                    &generations.borrow(),
                    budget,
                    ctx,
                    mem,
                );
        }
        self.install
            .dispatch_exposing_exceptions_at_budget(entry, budget, ctx, mem)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn reserves_bank(&self, bank: BankId) -> bool {
        if let Some(generations) = &self.generations {
            return self
                .install
                .reserves_bank_with_generations(bank, &generations.borrow());
        }
        self.install.reserves_bank(bank)
    }

    fn activate_for_fetch(
        &self,
        target_pc: GuestPc,
        mem: &Rdram<'_>,
    ) -> Result<ExecutionKey, GenerationLookupError> {
        self.generations
            .as_ref()
            .ok_or(GenerationLookupError::UnmappedPc { pc: target_pc })?
            .borrow_mut()
            .activate_for_fetch(target_pc, mem)
            .map(|resolution| resolution.entry)
    }

    fn reconcile_before_dispatch(&self, mem: &Rdram<'_>) {
        self.reconcile_before_dispatch_with(|physical| {
            mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical))
        });
    }

    fn reconcile_before_dispatch_with(&self, mut read_physical_byte: impl FnMut(u32) -> u8) {
        let Some(state) = &self.mutation_state else {
            return;
        };
        state.borrow_mut().seal_with(&mut read_physical_byte);
        let snapshot = state.borrow().read_snapshot(read_physical_byte);
        state
            .borrow_mut()
            .reconcile_snapshot_before_dispatch(snapshot);
    }

    fn begin_host_abi_transaction(
        &self,
        target: GuestPc,
        resume: ExecutionKey,
        mem: &Rdram<'_>,
    ) -> Option<HostMutationTransactionTokenV1> {
        let Some(state) = &self.mutation_state else {
            return None;
        };
        let thread = super::current_thread_id("catalog host mutation transaction");
        if let Some(outer) = state.borrow().active_host_transaction(thread) {
            self.flush_host_abi_transaction(outer, mem);
        }
        self.reconcile_before_dispatch(mem);
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|events| events.borrow().len());
        assert_eq!(
            pending, 0,
            "catalog host transaction began with {pending} uncommitted child writer event(s)"
        );
        Some(
            state
                .borrow_mut()
                .begin_host_transaction(thread, target, resume),
        )
    }

    fn flush_host_abi_transaction_with(
        &self,
        token: HostMutationTransactionTokenV1,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) {
        let state = self
            .mutation_state
            .as_ref()
            .expect("host transaction exists without canonical mutation state");
        state.borrow().assert_active_host_transaction(token);
        let pending = PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|events| events.borrow().len());
        assert_eq!(
            pending, 0,
            "catalog host transaction {} reached an ordering boundary with {pending} uncommitted child writer event(s)",
            token.transaction_id
        );
        let snapshot = state.borrow().read_snapshot(&mut read_physical_byte);
        let changed = state.borrow().current_changed_ranges(&snapshot);
        let first_new_entry = state.borrow().entries.len();
        for (physical_start, physical_end) in changed {
            fn64_recomp_rs::notify_host_abi_write(physical_start, physical_end - physical_start);
        }
        self.invalidate_pending_physical_writes_with(&mut read_physical_byte);
        state
            .borrow_mut()
            .record_host_abi_boundary(token, first_new_entry);
    }

    fn flush_host_abi_transaction(&self, token: HostMutationTransactionTokenV1, mem: &Rdram<'_>) {
        self.flush_host_abi_transaction_with(token, |physical| {
            mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical))
        });
    }

    fn finish_host_abi_transaction(
        &self,
        token: Option<HostMutationTransactionTokenV1>,
        mem: &Rdram<'_>,
    ) {
        let Some(token) = token else {
            return;
        };
        self.flush_host_abi_transaction(token, mem);
        self.mutation_state
            .as_ref()
            .expect("host transaction lost canonical mutation state")
            .borrow_mut()
            .finish_host_transaction(token);
    }

    fn flush_active_host_abi_transaction_with(
        &self,
        thread: ThreadId,
        read_physical_byte: impl FnMut(u32) -> u8,
    ) {
        let token = self
            .mutation_state
            .as_ref()
            .and_then(|state| state.borrow().active_host_transaction(thread));
        if let Some(token) = token {
            self.flush_host_abi_transaction_with(token, read_physical_byte);
        }
    }

    fn invalidate_pending_physical_writes(&self, mem: &Rdram<'_>) -> Vec<GenerationId> {
        self.invalidate_pending_physical_writes_with(|physical| {
            mem.load_bu(0xffff_ffff_8000_0000 | u64::from(physical))
        })
    }

    fn invalidate_pending_physical_writes_with(
        &self,
        mut read_physical_byte: impl FnMut(u32) -> u8,
    ) -> Vec<GenerationId> {
        let writes =
            PENDING_EXECUTABLE_WRITES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        let events = PENDING_ATTRIBUTED_EXECUTABLE_WRITES
            .with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        let mut invalidated = Vec::new();
        if let Some(generations) = &self.generations {
            let mut generations = generations.borrow_mut();
            for &(physical_start, byte_len) in &writes {
                let physical_end = physical_start.checked_add(byte_len).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical generation write range overflows: {physical_start:#010x} + {byte_len:#x}"
                    ))
                });
                invalidated.extend(
                    generations
                        .invalidate_physical_write(physical_start, physical_end)
                        .unwrap_or_else(|error| {
                            recompiled_gap_panic(format!(
                                "canonical generation write range is invalid: {error}"
                            ))
                        }),
                );
            }
        } else if self.mutation_state.is_none() {
            assert!(
                writes.is_empty() && events.is_empty(),
                "catalog without executable backing retained attributed writes"
            );
            return Vec::new();
        }
        invalidated.sort_unstable();
        invalidated.dedup();
        if let Some(state) = &self.mutation_state {
            state.borrow_mut().seal_with(&mut read_physical_byte);
            let snapshot = state.borrow().read_snapshot(read_physical_byte);
            state
                .borrow_mut()
                .commit_snapshot(snapshot, events, invalidated.clone());
        }
        invalidated
    }

    fn mutation_evidence_snapshot(&self) -> Option<CanonicalExecutableMutationJournalEvidenceV1> {
        self.mutation_state
            .as_ref()
            .map(|state| state.borrow().evidence_snapshot())
    }

    fn generation_evidence_snapshot(&self) -> Option<BackedGenerationCatalogEvidenceV1> {
        self.generations
            .as_ref()
            .map(|generations| generations.borrow().evidence_snapshot())
    }
}

struct LiveTransferResolver {
    live: LiveBlockProgram,
}

impl TransferResolver for LiveTransferResolver {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.live.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if fn64_recomp_rs::resolve_host_function(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

impl LiveBlockProgram {
    fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if let Some(catalog) = self.precompiled_generations.borrow().as_ref() {
            if let Ok(key) = catalog.resolve_active(target_pc) {
                return Ok(key);
            }
        }
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.transfer_lookup)(source_bank, target_pc)
    }

    fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        if let Some(catalog) = self.precompiled_generations.borrow().as_ref() {
            if let Ok(key) = catalog.resolve_active(target_pc) {
                return Ok(key);
            }
        }
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.entry_lookup)(target_pc)
    }
}

fn canonical_pending_executable_writes() -> Vec<PendingExecutableWriteEvidenceSnapshot> {
    let mut writes = PENDING_EXECUTABLE_WRITES.with(|pending| {
        pending
            .borrow()
            .iter()
            .map(|&(physical_start, len)| {
                assert!(len > 0, "pending executable write has zero length");
                let physical_end = physical_start
                    .checked_add(len)
                    .expect("pending executable write exceeds physical address space");
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start,
                    physical_end,
                }
            })
            .collect::<Vec<_>>()
    });
    writes.sort_unstable_by_key(|write| (write.physical_start, write.physical_end));
    let mut canonical: Vec<PendingExecutableWriteEvidenceSnapshot> = Vec::new();
    for write in writes {
        if let Some(previous) = canonical.last_mut() {
            if write.physical_start <= previous.physical_end {
                previous.physical_end = previous.physical_end.max(write.physical_end);
                continue;
            }
        }
        canonical.push(write);
    }
    canonical
}

/// Capture the installed typed-Rust program without runner, resolver,
/// builder, lookup, or native function-pointer values.
///
/// The legacy function install API remains executable for compatibility, but
/// it is intentionally not evidence-capable: callers must use
/// [`set_entry_lookup_with_artifact_identity`] (or the matching boot helper)
/// before this function will describe a function lane. This prevents section
/// geometry or process-specific pointer bits from impersonating code identity.
pub fn recompiled_program_evidence_snapshot() -> Option<RecompiledProgramEvidenceSnapshot> {
    let (function_lane, block_lane, catalog_lane) = with_host(|host| {
        (
            host.recompiled_lookup.is_some(),
            host.recompiled_program.clone(),
            host.canonical_recompiled_program.clone(),
        )
    });
    assert!(
        usize::from(function_lane)
            + usize::from(block_lane.is_some())
            + usize::from(catalog_lane.is_some())
            <= 1,
        "multiple mutually exclusive recompiled lanes are installed simultaneously"
    );
    if function_lane {
        let identity = FUNCTION_LANE_ARTIFACT_IDENTITY
            .with(std::cell::Cell::get)
            .unwrap_or_else(|| {
                panic!(
                    "function-lane release evidence requires a stable host-provided artifact identity"
                )
            });
        return Some(RecompiledProgramEvidenceSnapshot::Function {
            identity: ProgramIdentityEvidenceSnapshot {
                identity,
                source: ProgramIdentitySource::CallerSupplied,
            },
        });
    }
    if let Some(live) = catalog_lane {
        assert!(
            !live.dynamic_execution_installed(),
            "static recompiled-program evidence is unavailable after dynamic mapped execution is installed"
        );
        return Some(RecompiledProgramEvidenceSnapshot::Block {
            program: live.install.program_evidence().clone(),
            dispatch_artifact_identity: live.install.evidence().dispatch_artifact_identity,
            instruction_budget: live.install.budget().get(),
            executable_regions: Vec::new(),
            pending_executable_writes: if live.generations.is_some() {
                canonical_pending_executable_writes()
            } else {
                Vec::new()
            },
        });
    }
    let live = block_lane?;
    let program = live.program.borrow().evidence_snapshot();
    let dispatch_artifact_identity = live.dispatch_artifact_identity.unwrap_or_else(|| {
        panic!(
            "block-lane release evidence requires a stable host-provided dispatch artifact identity"
        )
    });
    let mut executable_regions = live
        .executable_regions
        .borrow()
        .iter()
        .map(|observed| {
            let active_bank = observed.region.active_bank().unwrap_or_else(|| {
                panic!("observed executable region has no active bank during evidence capture")
            });
            let active_generation = observed
                .next_generation
                .checked_sub(1)
                .expect("observed executable region has no active generation");
            let builder_artifact_identity = observed.builder_artifact_identity.unwrap_or_else(|| {
                panic!(
                    "executable-region release evidence requires a stable host-provided builder artifact identity"
                )
            });
            LiveExecutableRegionEvidenceSnapshot {
                physical_start: observed.physical_start,
                physical_end: observed.physical_end,
                virtual_start: observed.region.start(),
                virtual_end: observed.region.end(),
                active_bank,
                active_generation,
                next_generation: observed.next_generation,
                builder_artifact_identity,
                activation: match observed.activation {
                    ExecutableActivation::EagerPublication => {
                        ExecutableActivationEvidence::EagerPublication
                    }
                    ExecutableActivation::FetchBoundary => {
                        ExecutableActivationEvidence::FetchBoundary
                    }
                },
            }
        })
        .collect::<Vec<_>>();
    executable_regions.sort_unstable_by_key(|region| {
        (
            region.physical_start,
            region.physical_end,
            region.virtual_start,
            region.virtual_end,
        )
    });
    Some(RecompiledProgramEvidenceSnapshot::Block {
        program,
        dispatch_artifact_identity,
        instruction_budget: live.budget.get(),
        executable_regions,
        pending_executable_writes: canonical_pending_executable_writes(),
    })
}

/// Capture evidence only for the callback-free canonical catalog owner.
/// Legacy function and block installs always return `None` here, even when
/// they can produce the broader compatibility evidence snapshot above.
pub fn catalog_resolver_install_evidence_snapshot() -> Option<CatalogResolverInstallEvidenceV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map(|live| live.install.evidence().clone())
    })
}

pub fn catalog_generation_install_evidence_snapshot() -> Option<CatalogGenerationInstallEvidenceV1>
{
    with_host(|host| {
        host.canonical_recompiled_program.as_ref().and_then(|live| {
            live.generation_evidence_snapshot().map(|generations| {
                CatalogGenerationInstallEvidenceV1 {
                    resolver: live.install.evidence().clone(),
                    generations,
                    bootstrap: live.bootstrap_evidence.clone(),
                    pending_physical_writes: canonical_pending_executable_writes(),
                    mutation_journal: live.mutation_evidence_snapshot(),
                }
            })
        })
    })
}

/// Runtime mutation evidence exists only for the callback-free canonical
/// generation owner. An unsealed snapshot means installation occurred but no
/// guest dispatch has yet established the immutable bootstrap baseline.
pub fn canonical_executable_mutation_journal_evidence_snapshot(
) -> Option<CanonicalExecutableMutationJournalEvidenceV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .and_then(CanonicalLiveBlockProgramV1::mutation_evidence_snapshot)
    })
}

/// Transfer the one move-only bootstrap writer-channel authority minted by
/// the canonical validated boot path. A second take returns `None`; retained
/// evidence cannot be deserialized or replayed into another capability.
pub fn take_validated_bootstrap_writer_channel_receipt_v1(
) -> Option<ValidatedBootstrapWriterChannelReceiptV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .and_then(|live| live.bootstrap_writer_completion.borrow_mut().take())
    })
}

/// Start one fresh CPU instruction-store audit window.
///
/// The runtime must be quiescent before arming. The returned move-only token
/// is bound to the exact canonical program model; beginning a replacement
/// window supersedes the prior token. This clears only ABI-private CPU trace
/// state and cannot be reconstructed from copied observations.
pub fn begin_cpu_writer_runtime_trace_epoch_v1(
) -> Result<Option<CpuWriterRuntimeTraceEpochV1>, CpuWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        live.begin_cpu_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local CPU-store runtime prerequisite.
///
/// At least one post-commit CPU RDRAM store must have crossed the typed write
/// observer after this exact epoch was armed. Successful validation requires
/// a second quiescent boundary and consumes both the live epoch and the sole
/// receipt. It is not selected-build or writer-denominator authority.
pub fn take_validated_cpu_writer_runtime_state_receipt_v1(
    epoch: &CpuWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedCpuWriterRuntimeStateReceiptV1>, CpuWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_cpu_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
        )
    })
}

/// Start one fresh canonical Host ABI writer audit window.
///
/// The runtime must be quiescent and own an ABI-issued stable-shim catalog;
/// compatibility caller pointers fail closed. The move-only token binds the
/// subsequent exact transaction lifecycle to this canonical program model.
pub fn begin_host_abi_writer_runtime_trace_epoch_v1(
) -> Result<Option<HostAbiWriterRuntimeTraceEpochV1>, HostAbiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        live.begin_host_abi_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local Host ABI writer prerequisite.
///
/// Success requires balanced per-thread LIFO transactions through ABI-issued
/// targets and at least one actual HostAbi executable-journal commit after the
/// exact epoch was armed. A host invocation with no observed write is not
/// promoted into writer authority. This is not denominator completion.
pub fn take_validated_host_abi_writer_runtime_state_receipt_v1(
    epoch: &HostAbiWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedHostAbiWriterRuntimeStateReceiptV1>, HostAbiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_host_abi_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
        )
    })
}

/// Start one fresh ABI-owned RSP writeback audit window.
///
/// The runtime must have no admitted/running/yielded task, in-flight
/// interpreter, retained HLE continuation, or pending SP task. The returned
/// token authenticates interpreter writeback ranges and successful translated
/// audio-HLE executable publications. Rejected HLE journal sequences poison
/// the epoch instead of becoming later success evidence.
pub fn begin_rsp_writer_runtime_trace_epoch_v1(
) -> Result<Option<RspWriterRuntimeTraceEpochV1>, RspWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if host.device_fabric.snapshot().sp_busy {
            return Err(RspWriterRuntimeStateErrorV1::PendingDeviceRspTask);
        }
        if host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1()
        {
            return Err(RspWriterRuntimeStateErrorV1::PendingAbiRspWork);
        }
        live.begin_rsp_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local RSP writeback prerequisite.
///
/// Success requires at least one nonempty interpreter range or one translated
/// HLE executable-journal sequence, exact owner generations, a second
/// quiescent boundary, and unchanged sealed watched state. No denominator
/// accepts this receipt directly.
pub fn take_validated_rsp_writer_runtime_state_receipt_v1(
    epoch: &RspWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedRspWriterRuntimeStateReceiptV1>, RspWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let pending_abi_rsp_work = host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1();
        live.take_rsp_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.snapshot().sp_busy,
            pending_abi_rsp_work,
        )
    })
}

/// Start one fresh renderer-publication audit epoch.
///
/// Arming requires a validated production-AOT owner and no live RSP task,
/// DPC transaction, DP completion, renderer continuation, or ABI task owner.
/// The returned token is ABI-local prerequisite authority only.
pub fn begin_rdp_renderer_writer_runtime_trace_epoch_v1(
) -> Result<Option<RdpRendererWriterRuntimeTraceEpochV1>, RdpRendererWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        let device = host.device_fabric.snapshot();
        if device.sp_busy {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask);
        }
        if device.pending_dpc.is_some() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction);
        }
        if device.dp_busy {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion);
        }
        if host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1()
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork);
        }
        live.begin_rdp_renderer_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local renderer publication prerequisite.
///
/// Success requires at least one backend-committed publication in the exact
/// epoch, a second quiescent boundary, and complete agreement between traced
/// RDP journal sequences and the canonical watched-byte journal.
pub fn take_validated_rdp_renderer_writer_runtime_state_receipt_v1(
    epoch: &RdpRendererWriterRuntimeTraceEpochV1,
) -> Result<
    Option<ValidatedRdpRendererWriterRuntimeStateReceiptV1>,
    RdpRendererWriterRuntimeStateErrorV1,
> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let device = host.device_fabric.snapshot();
        let pending_abi_renderer_work = host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1();
        live.take_rdp_renderer_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            device.sp_busy,
            device.pending_dpc.is_some(),
            device.dp_busy,
            pending_abi_renderer_work,
        )
    })
}

/// Start one fresh, typed PI-DMA writer audit epoch.
///
/// The canonical runtime must be quiescent with no active device request,
/// queued ABI completion owner, or previously asserted PI interrupt. A
/// successful arm clears retained device history and binds the returned
/// move-only token to this exact canonical program model. It is not selected-
/// build or writer-denominator authority.
pub fn begin_pi_writer_runtime_trace_epoch_v1(
) -> Result<Option<PiWriterRuntimeTraceEpochV1>, PiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        let epoch = live.begin_pi_writer_runtime_trace_epoch(
            host.device_fabric.pending_pi_request().is_some(),
            !host.pending_pi_completions.is_empty(),
            host.device_fabric
                .interrupt_pending(fn64_runtime::InterruptSource::Pi),
        )?;
        if epoch.is_some() {
            host.device_fabric.set_trace_enabled(false);
            host.device_fabric.set_trace_enabled(true);
        }
        Ok(epoch)
    })
}

/// Validate and transfer the ABI-local PI-DMA runtime prerequisite.
///
/// The move-only epoch must come from
/// [`begin_pi_writer_runtime_trace_epoch_v1`] for this exact live program.
/// Successful validation proves a balanced PI lifecycle with at least one
/// committed device-to-RDRAM transfer and consumes the sole receipt. It is
/// not writer-denominator completion.
pub fn take_validated_pi_writer_runtime_state_receipt_v1(
    epoch: &PiWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedPiWriterRuntimeStateReceiptV1>, PiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_pi_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.pending_pi_request().is_some(),
            !host.pending_pi_completions.is_empty(),
        )
    })
}

/// Validate and transfer the ABI-local SI runtime-state prerequisite once.
///
/// The canonical runtime must be between guest scheduling steps with no SI
/// request, ABI completion owner, executable write, or writer transaction in
/// flight. A failed attempt does not consume the one successful take, so a
/// host may first drain an already accepted SI request. This receipt is not a
/// writer-denominator completion capability and carries no generated-build
/// authority.
pub fn take_validated_si_writer_runtime_state_receipt_v1(
) -> Result<Option<ValidatedSiWriterRuntimeStateReceiptV1>, SiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let storage = host.owned_runtime_rdram.as_deref().unwrap_or(&[]);
        live.take_si_writer_runtime_state(
            storage,
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.pending_si_request().is_some(),
            host.pending_si_completion.is_some(),
        )
    })
}

/// Start one fresh, typed SP-DMA writer audit epoch.
///
/// The runtime must already be quiescent. This operation discards retained
/// device history, re-enables retention, and returns a move-only token bound
/// to this canonical program model. Unlike the older SI prerequisite, whose
/// selected-child verifier owns trace freshness externally, SP freshness is
/// enforced by the ABI token and cannot be reconstructed from copied events.
pub fn begin_sp_writer_runtime_trace_epoch_v1(
) -> Result<Option<SpWriterRuntimeTraceEpochV1>, SpWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if host.device_fabric.sp_dma_busy() {
            return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpDma);
        }
        if host.device_fabric.snapshot().sp_busy {
            return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpTask);
        }
        if host.loaded_rsp_task.is_some()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
        {
            return Err(SpWriterRuntimeStateErrorV1::PendingAbiSpWork);
        }
        let epoch = live.begin_sp_writer_runtime_trace_epoch()?;
        if epoch.is_some() {
            host.device_fabric.set_trace_enabled(false);
            host.device_fabric.set_trace_enabled(true);
        }
        Ok(epoch)
    })
}

/// Validate and transfer the ABI-local SP-DMA runtime-state prerequisite.
///
/// The move-only epoch must come from
/// [`begin_sp_writer_runtime_trace_epoch_v1`] for this exact live program.
/// A successful receipt proves a balanced raw SP-DMA lifecycle including at
/// least one RSP-to-RDRAM commit; it is not writer-denominator completion.
pub fn take_validated_sp_writer_runtime_state_receipt_v1(
    epoch: &SpWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedSpWriterRuntimeStateReceiptV1>, SpWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let pending_abi_sp_work = host.loaded_rsp_task.is_some()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            );
        live.take_sp_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.sp_dma_busy(),
            host.device_fabric.snapshot().sp_busy,
            pending_abi_sp_work,
        )
    })
}

/// Copy successfully entered arbitrary-PC destinations in exact runner-entry
/// order. An empty vector means either that no block lane is installed or that
/// its admitted program has not executed; callers select the authoritative
/// interpretation from [`recompiled_program_evidence_snapshot`].
pub fn copy_block_execution_destinations() -> Vec<ExecutionDestinationObservation> {
    let (legacy, catalog) = with_host(|host| {
        (
            host.recompiled_program.clone(),
            host.canonical_recompiled_program.clone(),
        )
    });
    if let Some(live) = catalog {
        return live.install.copy_execution_destinations();
    }
    legacy.map_or_else(Vec::new, |live| {
        live.program.borrow().copy_execution_destinations()
    })
}

/// Return the exact total instruction-budget work charged by all OSThreads
/// executing the currently installed canonical `BlockProgram`.
///
/// The counter is reset on install, includes architectural fault attempts,
/// and excludes synthetic host/legacy-C scheduling charges. Callers sampling
/// it after `run_one_step` observe a global scheduler boundary; it is an
/// operational progress measure, not static coverage or release authority.
pub fn canonical_block_charged_instructions_v1() -> Option<u64> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map(|live| live.canonical_charged_instructions.get())
    })
}

/// Bound each subsequent canonical dispatch slice so the process-wide charged
/// instruction counter can stop at one exact operational checkpoint. A final
/// straight instruction may use a one-instruction slice; a branch and delay
/// slot remain indivisible and fail loudly if only one instruction remains.
/// This is scheduling evidence control, not guest state or static execution
/// authority. Clearing the limit restores the install's immutable default
/// slice budget.
pub fn set_canonical_block_instruction_limit_v1(limit: Option<u64>) {
    with_host(|host| {
        let live = host
            .canonical_recompiled_program
            .as_ref()
            .unwrap_or_else(|| panic!("canonical instruction limit requires an installed catalog"));
        if let Some(limit) = limit {
            assert!(
                live.canonical_instruction_limit.get().is_none(),
                "canonical instruction limit is already armed"
            );
            let charged = live.canonical_charged_instructions.get();
            assert!(
                limit > charged,
                "canonical instruction limit {limit} must exceed already charged work {charged}"
            );
        }
        live.canonical_instruction_limit.set(limit);
    });
}

/// Copy each canonical thread's latest pointer-free publication in thread-ID
/// order. This is observational state only: the copy does not establish that
/// every thread is quiescent or that a complete runtime state was captured.
pub fn copy_canonical_thread_publications_v1() -> Vec<CanonicalThreadPublicationV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map_or_else(Vec::new, |live| {
                live.thread_publications
                    .borrow()
                    .values()
                    .cloned()
                    .collect()
            })
    })
}

/// Copy bounded source-bound dynamic execution hotness in full-identity order.
/// Saturation is retained explicitly in the dropped counters. This is
/// operational promotion input only; it is never merged into static
/// destination evidence or writer/release authority.
#[cfg(feature = "dynamic-mapped-runtime")]
pub fn copy_dynamic_mapped_execution_telemetry_v1() -> DynamicMappedExecutionTelemetryV1 {
    let live = with_host(|host| host.canonical_recompiled_program.clone())
        .expect("dynamic execution telemetry requires a canonical catalog install");
    assert!(
        live.dynamic_execution_installed(),
        "dynamic execution telemetry requires an enabled dynamic catalog"
    );
    let mutation = live.mutation_state.as_ref().map(|state| {
        let state = state.borrow();
        (state.journal_root_sha256, state.entries.len() as u64)
    });
    let aggregates = live
        .dynamic_execution_aggregates
        .borrow()
        .values()
        .cloned()
        .collect();
    DynamicMappedExecutionTelemetryV1 {
        resolver_install_sha256: resolver_install_definition_sha256(&live.install),
        program_identity: live.install.evidence().program_identity,
        dynamic_source_sha256: fn64_recomp_rs::dynamic_mapped_execution_build_receipt_v1()
            .source_sha256(),
        rom_sha256: live
            .bootstrap_evidence
            .as_ref()
            .map(|evidence| evidence.rom_sha256),
        bootstrap_receipt_sha256: live
            .bootstrap_evidence
            .as_ref()
            .map(|evidence| evidence.receipt_sha256),
        mutation_journal_root_sha256: mutation.map(|(root, _)| root),
        mutation_journal_entry_count: mutation.map_or(0, |(_, count)| count),
        aggregates,
        aggregate_capacity: DYNAMIC_EXECUTION_AGGREGATE_CAPACITY as u64,
        attempted_entries_per_aggregate_capacity: DYNAMIC_ATTEMPTED_ENTRIES_PER_AGGREGATE_CAPACITY
            as u64,
        dropped_identity_activations: live.dynamic_dropped_identity_activations.get(),
        dropped_identity_charged_instructions: live
            .dynamic_dropped_identity_charged_instructions
            .get(),
        dropped_identity_unsupported_exits: live.dynamic_dropped_identity_unsupported_exits.get(),
        dropped_attempted_entry_activations: live.dynamic_dropped_attempted_entry_activations.get(),
        dropped_attempted_entry_charged_instructions: live
            .dynamic_dropped_attempted_entry_charged_instructions
            .get(),
        dropped_attempted_entry_unsupported_exits: live
            .dynamic_dropped_attempted_entry_unsupported_exits
            .get(),
    }
}

pub fn copy_block_host_boundaries() -> Vec<BlockHostBoundaryObservation> {
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow().iter().copied().collect())
}

/// Bound diagnostic host-boundary history. `None` restores complete history,
/// which is the default required by certification evidence.
pub fn set_block_host_boundary_history_limit(limit: Option<NonZeroUsize>) {
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|installed| installed.set(limit));
    if let Some(limit) = limit {
        BLOCK_HOST_BOUNDARIES.with(|boundaries| {
            let mut boundaries = boundaries.borrow_mut();
            while boundaries.len() > limit.get() {
                boundaries.pop_front();
            }
        });
    }
}

/// Enable or suppress diagnostic host-boundary history. Complete history is
/// enabled by default; suppressing it also clears any retained entries.
pub fn set_block_host_boundary_history_enabled(enabled: bool) {
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|installed| installed.set(enabled));
    if !enabled {
        BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    }
}

fn observe_block_host_boundary(
    phase: BlockHostBoundaryPhase,
    target: GuestPc,
    resume: ExecutionKey,
    ctx: &RsContext,
) {
    if !BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(Cell::get) {
        return;
    }
    BLOCK_HOST_BOUNDARIES.with(|boundaries| {
        let mut boundaries = boundaries.borrow_mut();
        boundaries.push_back(BlockHostBoundaryObservation {
            at: fn64_runtime::Cycles::new(crate::sim_time()),
            thread: super::current_thread_id("block host-boundary observation"),
            phase,
            target,
            resume,
            gprs: ctx.gprs(),
            hi: ctx.hi,
            lo: ctx.lo,
            cop0_count: ctx.cop0_count,
            cop0_compare: ctx.cop0_compare,
            cop0_status: ctx.cop0_status,
            cop0_cause: ctx.cop0_cause,
            cop0_epc: ctx.cop0_epc,
        });
        BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| {
            if let Some(limit) = limit.get() {
                while boundaries.len() > limit.get() {
                    boundaries.pop_front();
                }
            }
        });
    });
}

fn invoke_observed_block_host(
    target: GuestPc,
    resume: ExecutionKey,
    host: RecompFunc,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    observe_block_host_boundary(BlockHostBoundaryPhase::Enter, target, resume, ctx);
    host(ctx, mem);
    observe_block_host_boundary(BlockHostBoundaryPhase::Exit, target, resume, ctx);
}

fn invoke_catalog_block_host(
    live: &CanonicalLiveBlockProgramV1,
    target: GuestPc,
    resume: ExecutionKey,
    host: RecompFunc,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    live.publish_opaque_host(target, resume);
    let transaction = live.begin_host_abi_transaction(target, resume, mem);
    let invocation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invoke_observed_block_host(target, resume, host, ctx, mem);
    }));
    if let Err(payload) = invocation {
        if let Some(transaction) = transaction {
            live.mutation_state
                .as_ref()
                .expect("host transaction lost canonical mutation state while unwinding")
                .borrow_mut()
                .poison(format!(
                    "host ABI transaction {} for thread {} unwound before commit",
                    transaction.transaction_id, transaction.thread
                ));
        }
        std::panic::resume_unwind(payload);
    }
    live.finish_host_abi_transaction(transaction, mem);
}

/// Copy successfully entered emitted whole-function destinations in exact
/// entry order. An installed legacy function lane fails closed: only the API
/// which consumes the generated artifact's observation-schema marker can make
/// this history authoritative.
pub fn copy_function_execution_destinations() -> Vec<FunctionExecutionDestinationObservation> {
    let function_lane = with_host(|host| host.recompiled_lookup.is_some());
    if !function_lane {
        return Vec::new();
    }
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|schema| {
        schema.get().unwrap_or_else(|| {
            panic!(
                "function-lane destination evidence requires the generated artifact's entry-observation schema"
            )
        });
    });
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow().clone())
}

fn observe_function_entry(function: TranslatedFunctionIdentity) {
    let artifact_identity = FUNCTION_LANE_ARTIFACT_IDENTITY
        .with(std::cell::Cell::get)
        .unwrap_or_else(|| {
            panic!("observed function entry has no stable generated-artifact identity")
        });
    let at = fn64_runtime::Cycles::new(crate::sim_time());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| {
        destinations
            .borrow_mut()
            .push(FunctionExecutionDestinationObservation {
                at,
                artifact_identity,
                function,
            });
    });
}

fn observe_renderer_write(event: GuestWriteEvent) {
    if let GuestWriteEvent::NonRdpWrite16 {
        logical_offset,
        value,
        ..
    } = event
    {
        super::task_dispatch::observe_non_rdp_write16(logical_offset, value);
    }
}

fn record_executable_and_renderer_write(event: GuestWriteEvent) {
    let (offset, len) = event.range();
    if event.channel() == WriterChannel::CpuInstructionStore {
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            if let Some(trace) = trace.borrow_mut().as_mut() {
                trace.events.push((offset, len));
            }
        });
    }
    let end = offset.saturating_add(len);
    let intersects_executable = EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges
            .borrow()
            .iter()
            .any(|&(physical_start, physical_end)| offset < physical_end && end > physical_start)
    });
    PENDING_EXECUTABLE_WRITES.with(|writes| writes.borrow_mut().push((offset, len)));
    if intersects_executable {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|writes| writes.borrow_mut().push(event));
    }
    observe_renderer_write(event);
}

fn classify_live_executable_write(event: GuestWriteEvent) -> GuestWriteBoundary {
    let (start, len) = event.range();
    let end = start.saturating_add(len);
    if EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges
            .borrow()
            .iter()
            .any(|&(physical_start, physical_end)| start < physical_end && end > physical_start)
    }) {
        GuestWriteBoundary::ExecutableChanged
    } else {
        GuestWriteBoundary::Continue
    }
}

/// Run one synchronous renderer publication against live RDRAM and attribute
/// every changed executable byte to the renderer channel before the guest can
/// resume. The snapshot is limited to the sealed ever-admissible backing
/// union; ordinary framebuffer writes outside that union incur no journal
/// storage.
fn track_catalog_nested_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
    notify: impl Fn(u32, u32),
) -> R {
    let transaction = begin_catalog_nested_writer(rdram, "tracked renderer/RSP publication");
    if transaction.is_canonical() {
        let result = operation(rdram);
        transaction.commit_changed_bytes(rdram, notify);
        return result;
    }
    let ranges = EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone());
    let before = {
        let view = fn64_runtime::RdramView::from_storage(rdram);
        ranges
            .iter()
            .map(|&(physical_start, physical_end)| {
                assert!(
                    physical_end as usize <= view.len(),
                    "renderer mutation tracker range [{physical_start:#010x}, {physical_end:#010x}) exceeds live RDRAM {:#x}",
                    view.len()
                );
                (physical_start..physical_end)
                    .map(|physical| {
                        view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let result = operation(rdram);
    let view = fn64_runtime::RdramView::from_storage(rdram);
    for (&(physical_start, physical_end), before) in ranges.iter().zip(before) {
        let mut physical = physical_start;
        while physical < physical_end {
            let before_index = (physical - physical_start) as usize;
            if before[before_index] == view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            {
                physical += 1;
                continue;
            }
            let changed_start = physical;
            physical += 1;
            while physical < physical_end
                && before[(physical - physical_start) as usize]
                    != view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            {
                physical += 1;
            }
            notify(changed_start, physical - changed_start);
        }
    }
    transaction.commit(rdram);
    result
}

pub(crate) fn track_rdp_renderer_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> R {
    track_catalog_nested_mutation(rdram, operation, fn64_recomp_rs::notify_rdp_renderer_write)
}

/// Record one renderer operation whose backend contract has crossed a commit
/// boundary. Mutation tracking and this lifecycle mark are deliberately
/// separate: a `NeedsLle` operation does not become a successful publication,
/// and any executable journal sequence it produced invalidates the epoch.
pub(crate) fn record_rdp_renderer_publication_v1() {
    finish_rdp_renderer_operation_v1(true);
}

pub(crate) fn record_rdp_renderer_rejection_v1() {
    finish_rdp_renderer_operation_v1(false);
}

fn finish_rdp_renderer_operation_v1(committed: bool) {
    RDP_RENDERER_WRITER_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let live = with_host(|host| host.canonical_recompiled_program.clone())
            .expect("armed RDP renderer trace lost its canonical program owner");
        assert_eq!(
            live.rdp_renderer_writer_trace_epoch_id.get(),
            Some(trace.epoch_id),
            "RDP renderer publication crossed trace epoch owners"
        );
        assert_eq!(
            live.writer_program_model_sha256, trace.program_model_sha256,
            "RDP renderer publication crossed canonical program models"
        );
        let state = live
            .mutation_state
            .as_ref()
            .expect("armed RDP renderer trace lost its mutation journal")
            .borrow();
        assert!(
            trace.next_journal_entry_index <= state.entries.len(),
            "RDP renderer trace journal cursor exceeds the canonical journal"
        );
        let sequences = state.entries[trace.next_journal_entry_index..]
            .iter()
            .filter(|entry| {
                entry
                    .declared_writes
                    .iter()
                    .any(|declaration| declaration.channel == WriterChannel::RdpRenderer)
            })
            .map(|entry| entry.sequence)
            .collect();
        trace.next_journal_entry_index = state.entries.len();
        if committed {
            trace.publications.push(sequences);
        } else {
            trace.rejected_journal_sequences.extend(sequences);
        }
    });
}

pub(crate) fn track_rsp_execution_or_hle_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> (R, Vec<u64>) {
    let live = with_host(|host| host.canonical_recompiled_program.clone())
        .expect("tracked RSP/HLE publication lost its canonical program owner");
    let state = live
        .mutation_state
        .as_ref()
        .expect("tracked RSP/HLE publication lost its mutation journal");
    let initial_entry_count = state.borrow().entries.len();
    let result = track_catalog_nested_mutation(
        rdram,
        operation,
        fn64_recomp_rs::notify_rsp_execution_or_hle_writeback,
    );
    let journal_sequences = state.borrow().entries[initial_entry_count..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RspExecutionOrHleWriteback)
        })
        .map(|entry| entry.sequence)
        .collect();
    (result, journal_sequences)
}

fn process_executable_writes(
    live: &LiveBlockProgram,
    mut read_logical_byte: impl FnMut(u32) -> u8,
) -> Vec<BankId> {
    let writes =
        PENDING_EXECUTABLE_WRITES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if writes.is_empty() {
        return Vec::new();
    }
    let mut regions = live.executable_regions.borrow_mut();
    let deferred = writes
        .iter()
        .flat_map(|&(start, len)| {
            let end = start.saturating_add(len);
            regions.iter().filter_map(move |observed| {
                if observed.activation != ExecutableActivation::FetchBoundary {
                    return None;
                }
                let deferred_start = start.max(observed.physical_start);
                let deferred_end = end.min(observed.physical_end);
                (deferred_start < deferred_end)
                    .then(|| (deferred_start, deferred_end - deferred_start))
            })
        })
        .collect::<Vec<_>>();
    let mut program = live.program.borrow_mut();
    let mut retired = Vec::new();
    for observed in regions.iter_mut() {
        if observed.activation != ExecutableActivation::EagerPublication {
            continue;
        }
        let touched = writes.iter().any(|&(start, len)| {
            let end = start.saturating_add(len);
            start < observed.physical_end && end > observed.physical_start
        });
        if !touched {
            continue;
        }
        let bytes = (observed.physical_start..observed.physical_end)
            .map(&mut read_logical_byte)
            .collect::<Vec<_>>();
        let generation = observed.next_generation;
        let (code, runner) = (observed.builder)(&bytes, generation).unwrap_or_else(|error| {
            panic!(
                "executable rewrite [{:#010x}, {:#010x}) generation {generation} could not be translated: {error}",
                observed.physical_start, observed.physical_end
            )
        });
        if let Some(previous) = observed
            .region
            .install(&mut program, code, runner)
            .unwrap_or_else(|error| panic!("executable generation install failed: {error}"))
        {
            retired.push(previous);
        }
        observed.next_generation = observed
            .next_generation
            .checked_add(1)
            .expect("executable generation counter overflow");
    }
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().extend(deferred));
    retired
}

fn activate_fetch_generation(
    live: &LiveBlockProgram,
    at: ExecutionKey,
    miss: AotMiss,
    mut read_logical_byte: impl FnMut(u32) -> u8,
) -> Result<ExecutionKey, String> {
    if let Some(catalog) = live.precompiled_generations.borrow_mut().as_mut() {
        return catalog
            .activate_for_fetch_with(at.pc, |vaddr| read_logical_byte(vaddr & 0x1fff_ffff))
            .map(|resolution| resolution.entry)
            .map_err(|error| format!("{miss}; closed AOT pack selection failed: {error}"));
    }
    let mut regions = live.executable_regions.borrow_mut();
    let observed = regions
        .iter_mut()
        .find(|observed| {
            observed.activation == ExecutableActivation::FetchBoundary
                && observed.region.start() == miss.va_start
                && observed.region.end().get() == miss.va_start.get() + miss.byte_len
        })
        .ok_or_else(|| format!("{miss}; no fetch-activated region owns the attempted range"))?;
    if observed.region.active_bank() != Some(miss.expected_bank) {
        return Err(format!(
            "{miss}; active generation changed before fetch activation"
        ));
    }
    let bytes = (observed.physical_start..observed.physical_end)
        .map(&mut read_logical_byte)
        .collect::<Vec<_>>();
    let generation = observed.next_generation;
    let (code, runner) = (observed.builder)(&bytes, generation).map_err(|error| {
        format!("{miss}; no precompiled generation matches the completed image: {error}")
    })?;
    observed
        .region
        .install(&mut live.program.borrow_mut(), code, runner)
        .map_err(|error| format!("fetch-activated generation install failed: {error}"))?;
    observed.next_generation = observed
        .next_generation
        .checked_add(1)
        .ok_or_else(|| "fetch-activated generation counter overflow".to_string())?;
    PENDING_EXECUTABLE_WRITES.with(|pending| {
        pending.borrow_mut().retain(|&(start, len)| {
            let end = start.saturating_add(len);
            end <= observed.physical_start || start >= observed.physical_end
        });
    });
    observed
        .region
        .resolve(at.pc)
        .ok_or_else(|| format!("fetch-activated region does not contain retry PC {}", at.pc))
}

/// Return the static link-time vram for a callback pointer relocated into a
/// currently loaded overlay, if any.
pub fn canonical_vram(vram: u32) -> Option<u32> {
    with_host(|host| host.sections.canonical_vram(vram))
}

/// Install the generated module's dispatcher for both thread 0 and every
/// OSThread subsequently created by `osCreateThread`.
pub fn set_entry_lookup(lookup: Lookup, rdram_len: usize) {
    set_entry_lookup_config(lookup, rdram_len, None);
}

/// Install a function-lane dispatcher and bind the stable identity of its
/// generated native artifact for release evidence.
///
/// The identity must come from the artifact producer (normally its SHA-256),
/// not from section geometry or native callable addresses. Reinstalling via
/// [`set_entry_lookup`] deliberately clears it.
pub fn set_entry_lookup_with_artifact_identity(
    lookup: Lookup,
    rdram_len: usize,
    identity: ProgramArtifactIdentity,
) {
    set_entry_lookup_config(lookup, rdram_len, Some(identity));
}

/// Install a function-lane dispatcher whose generated artifact exports the
/// current whole-function entry-observation schema.
///
/// `schema` must be the `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` constant from
/// the same generated artifact identified by `identity`. Keeping this separate
/// from [`set_entry_lookup_with_artifact_identity`] makes stale or handwritten
/// callable sets non-authoritative instead of silently producing a partial
/// destination history.
pub fn set_entry_lookup_with_execution_observation(
    lookup: Lookup,
    rdram_len: usize,
    identity: ProgramArtifactIdentity,
    schema: FunctionEntryObservationSchema,
) {
    set_entry_lookup_config(lookup, rdram_len, Some(identity));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(Some(schema)));
    fn64_recomp_rs::set_function_entry_observer(Some(observe_function_entry));
}

fn set_entry_lookup_config(
    lookup: Lookup,
    rdram_len: usize,
    identity: Option<ProgramArtifactIdentity>,
) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(identity));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_write_observer(Some(observe_renderer_write));
    fn64_recomp_rs::set_guest_write_boundary_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    with_host(|host| {
        host.recompiled_lookup = Some(lookup);
        host.recompiled_program = None;
        host.canonical_recompiled_program = None;
        host.recompiled_rdram_len = rdram_len;
    });
}

fn set_block_program(program: LiveBlockProgram, rdram_len: usize) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| limit.set(None));
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|enabled| enabled.set(true));
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(None));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
    with_host(|host| {
        host.recompiled_lookup = None;
        host.recompiled_program = Some(program);
        host.canonical_recompiled_program = None;
        host.recompiled_rdram_len = rdram_len;
    });
}

fn set_catalog_block_program(
    install: CatalogResolverInstallV1,
    rdram_len: usize,
) -> CanonicalLiveBlockProgramV1 {
    set_catalog_program_parts(install, None, rdram_len, None)
}

fn set_catalog_generation_program(
    install: CatalogGenerationInstallV1,
    rdram_len: usize,
) -> CanonicalLiveBlockProgramV1 {
    set_catalog_program_parts(install.resolver, Some(install.generations), rdram_len, None)
}

fn set_catalog_program_parts(
    install: CatalogResolverInstallV1,
    generations: Option<BackedPrecompiledGenerationCatalogV1>,
    rdram_len: usize,
    bootstrap: Option<&ValidatedBootstrapRdramV1>,
) -> CanonicalLiveBlockProgramV1 {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    let ranges = executable_physical_ranges_for_parts(&install, generations.as_ref());
    let watched_ranges = ranges
        .iter()
        .map(
            |&(physical_start, physical_end)| PendingExecutableWriteEvidenceSnapshot {
                physical_start,
                physical_end,
            },
        )
        .collect::<Vec<_>>();
    let writer_program_model_sha256 =
        canonical_writer_program_model_sha256(&install, generations.as_ref(), &watched_ranges);
    if let Some(&(_, required_end)) = ranges.last() {
        assert!(
            usize::try_from(required_end).unwrap() <= rdram_len,
            "canonical executable backing ends at physical RDRAM {required_end:#010x}, beyond the installed {rdram_len:#x}-byte allocation"
        );
    }
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
    EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow_mut().clear());
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| limit.set(None));
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|enabled| enabled.set(true));
    FUNCTION_LANE_ARTIFACT_IDENTITY.with(|installed| installed.set(None));
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|installed| installed.set(None));
    fn64_recomp_rs::set_function_entry_observer(None);
    fn64_recomp_rs::set_unsupported_observer(Some(record_recompiled_unsupported));
    fn64_recomp_rs::set_host_lookup(None);
    let mutation_state = (!ranges.is_empty()).then(|| {
        let state = bootstrap.map_or_else(
            || CanonicalExecutableMutationStateV1::new(&ranges),
            |validated| {
                CanonicalExecutableMutationStateV1::from_bootstrap(
                    validated.receipt.evidence(),
                    &validated.storage,
                )
            },
        );
        Rc::new(RefCell::new(state))
    });
    let generations = generations.map(|generations| Rc::new(RefCell::new(generations)));
    if mutation_state.is_some() {
        EXECUTABLE_WRITE_RANGES.with(|installed| {
            installed.borrow_mut().extend_from_slice(&ranges);
        });
        fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
        fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
    } else {
        fn64_recomp_rs::set_guest_write_boundary_observer(None);
        fn64_recomp_rs::set_write_observer(Some(observe_renderer_write));
    }
    let live = CanonicalLiveBlockProgramV1 {
        install: Rc::new(install),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_units: Rc::new(RefCell::new(None)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_withheld_static_key: Rc::new(Cell::new(None)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_execution_aggregates: Rc::new(RefCell::new(BTreeMap::new())),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_activations: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_charged_instructions: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_identity_unsupported_exits: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_activations: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_charged_instructions: Rc::new(Cell::new(0)),
        #[cfg(feature = "dynamic-mapped-runtime")]
        dynamic_dropped_attempted_entry_unsupported_exits: Rc::new(Cell::new(0)),
        canonical_charged_instructions: Rc::new(Cell::new(0)),
        canonical_instruction_limit: Rc::new(Cell::new(None)),
        thread_publications: Rc::new(RefCell::new(BTreeMap::new())),
        generations,
        mutation_state,
        bootstrap_evidence: bootstrap.map(|validated| validated.receipt.evidence().clone()),
        writer_program_model_sha256,
        bootstrap_writer_completion: Rc::new(RefCell::new(None)),
        cpu_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        cpu_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        pi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        pi_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        si_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        sp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        sp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        host_abi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rsp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rsp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        rdp_renderer_writer_runtime_state_taken: Rc::new(Cell::new(false)),
        rdp_renderer_writer_trace_epoch_id: Rc::new(Cell::new(None)),
    };
    with_host(|host| {
        host.recompiled_lookup = None;
        host.recompiled_program = None;
        host.canonical_recompiled_program = Some(live.clone());
        host.recompiled_rdram_len = rdram_len;
    });
    live
}

/// Install the closed, immutable AOT generation inventory used by fetch-time
/// digest selection. Every referenced shard must already be registered in the
/// live `BlockProgram`; activation changes only virtual interval ownership and
/// never invokes a builder, interpreter, or runtime translator.
pub fn install_precompiled_generation_catalog(catalog: PrecompiledGenerationCatalog) {
    let live = with_host(|host| host.recompiled_program.clone()).unwrap_or_else(|| {
        panic!("install_precompiled_generation_catalog: no live BlockProgram is installed")
    });
    catalog
        .validate_program(&live.program.borrow())
        .unwrap_or_else(|error| {
            panic!("install_precompiled_generation_catalog: catalog does not match the live BlockProgram: {error}")
        });
    let mut installed = live.precompiled_generations.borrow_mut();
    assert!(
        installed.is_none(),
        "precompiled generation catalog is already installed"
    );
    *installed = Some(catalog);
}

/// Replace one live executable region with a new immutable bank generation.
/// This is safe at host/device boundaries, where `run_block_program` has
/// already dropped its immutable dispatch borrow. The old bank and runner are
/// retired atomically by [`ExecutableRegion::install`].
pub fn install_live_block_generation(
    region: &mut ExecutableRegion,
    code: CodeBank,
    runner: GeneratedBankRunner,
) -> Result<Option<BankId>, GenerationError> {
    let live = with_host(|host| host.recompiled_program.clone()).unwrap_or_else(|| {
        panic!("install_live_block_generation: no live BlockProgram is installed")
    });
    let result = region.install(&mut live.program.borrow_mut(), code, runner);
    result
}

/// Observe one physical RDRAM span as the backing bytes for an already-live
/// virtual executable region. CPU stores and device DMA writes that intersect
/// the physical span rebuild and atomically replace its active immutable bank
/// at the next host boundary.
///
/// The builder is deliberately a pure function of the final committed byte
/// image and a monotonically increasing generation number. It may select a
/// pre-generated runner today or invoke a future translator, but may not
/// publish half of a code/runner pair.
pub fn register_live_executable_region(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        None,
        ExecutableActivation::EagerPublication,
    );
}

/// Observe a replaceable executable image whose writer may construct it over
/// many guest instructions. Writes mark it dirty, but generation lookup is
/// deferred until an attempted fetch reports the completed live digest.
pub fn register_fetch_activated_executable_region(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        None,
        ExecutableActivation::FetchBoundary,
    );
}

/// Register a dynamic executable region while binding the stable artifact
/// which implements its pure generation builder.
pub fn register_live_executable_region_with_artifact_identity(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: ProgramArtifactIdentity,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        Some(builder_artifact_identity),
        ExecutableActivation::EagerPublication,
    );
}

/// Register a fetch-activated executable image while binding the stable
/// artifact which performs its closed-set generation selection.
pub fn register_fetch_activated_executable_region_with_artifact_identity(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: ProgramArtifactIdentity,
) {
    register_live_executable_region_config(
        physical_start,
        physical_end,
        region,
        builder,
        Some(builder_artifact_identity),
        ExecutableActivation::FetchBoundary,
    );
}

fn register_live_executable_region_config(
    physical_start: u32,
    physical_end: u32,
    region: ExecutableRegion,
    builder: LiveGenerationBuilder,
    builder_artifact_identity: Option<ProgramArtifactIdentity>,
    activation: ExecutableActivation,
) {
    assert!(
        physical_start < physical_end,
        "observed executable RDRAM region must be nonempty"
    );
    assert!(
        physical_start.is_multiple_of(4) && physical_end.is_multiple_of(4),
        "observed executable RDRAM bounds must be instruction-aligned"
    );
    let physical_len = physical_end - physical_start;
    let virtual_len = region.end().get() - region.start().get();
    assert_eq!(
        physical_len, virtual_len,
        "observed physical and virtual executable regions must have equal byte lengths"
    );
    let active = region.active_bank().unwrap_or_else(|| {
        panic!("observed executable region must have an installed active generation")
    });
    let (live, rdram_len) = with_host(|host| {
        (
            host.recompiled_program.clone().unwrap_or_else(|| {
                panic!("register_live_executable_region: no live BlockProgram is installed")
            }),
            host.recompiled_rdram_len,
        )
    });
    assert!(
        usize::try_from(physical_end).expect("physical executable end exceeds usize") <= rdram_len,
        "observed executable RDRAM end {physical_end:#010x} exceeds allocation {rdram_len:#x}"
    );
    {
        let program = live.program.borrow();
        let code = program.code().bank(active).unwrap_or_else(|| {
            panic!("observed executable region references missing active generation {active}")
        });
        assert_eq!(
            (code.vram_start(), code.vram_end()),
            (region.start(), region.end()),
            "observed executable region does not match its active bank"
        );
    }
    let mut regions = live.executable_regions.borrow_mut();
    assert!(
        regions.iter().all(|existing| {
            physical_end <= existing.physical_start || physical_start >= existing.physical_end
        }),
        "observed executable physical region overlaps an existing registration"
    );
    assert!(
        regions.iter().all(|existing| {
            region.end() <= existing.region.start() || region.start() >= existing.region.end()
        }),
        "observed executable virtual region overlaps an existing registration"
    );
    regions.push(ObservedExecutableRegion {
        physical_start,
        physical_end,
        region,
        next_generation: 1,
        builder,
        builder_artifact_identity,
        activation,
    });
    EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges.borrow_mut().push((physical_start, physical_end));
    });
}

/// Apply DMA-originated executable writes after the device fabric has
/// committed all bytes, but before it publishes completion messages or any
/// guest coroutine can resume.
pub(crate) fn process_live_executable_writes_from_host() {
    let (catalog, live) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.recompiled_program.clone(),
        )
    });
    if let Some(catalog) = catalog {
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        let required_end = catalog
            .mutation_state
            .as_ref()
            .map(|state| state.borrow().required_physical_end() as usize)
            .unwrap_or(1);
        assert!(
            !rdram.is_null() && rdram_len >= required_end,
            "canonical generation host write RDRAM allocation {rdram_len:#x} does not cover watched physical end {required_end:#x}"
        );
        // SAFETY: device/host publication runs only while guest execution is
        // suspended; the registered process allocation remains live.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        catalog.invalidate_pending_physical_writes_with(|physical| unsafe {
            storage.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        });
        fn64_recomp_rs::discard_executable_write_boundary();
        return;
    }
    let Some(live) = live else {
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        fn64_recomp_rs::discard_executable_write_boundary();
        return;
    };
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    assert!(
        !rdram.is_null() && rdram_len > 0,
        "live BlockProgram has no process RDRAM allocation"
    );
    // SAFETY: block execution is suspended at this device boundary and the
    // boot contract keeps the one process RDRAM allocation live throughout.
    // Use the raw storage adapter instead of manufacturing a second `&mut`
    // slice while the dormant coroutine retains its checked `Rdram` view.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    process_executable_writes(&live, |offset| unsafe {
        storage.read_u8(fn64_runtime::RdramAddr::from_offset(offset))
    });
    fn64_recomp_rs::discard_executable_write_boundary();
}

/// Commit the active catalog host transaction's current mutation prefix before
/// its coroutine yields control. This closes the exact interleaving
/// `HostAbi write -> coroutine suspend -> device/other-thread same-byte write
/// -> HostAbi resume`: the parent prefix advances the canonical baseline before
/// any child or different guest coroutine can run.
pub(super) fn checkpoint_catalog_host_transaction_before_suspend() {
    let Some(thread) = super::ACTIVE_THREAD_ID.with(Cell::get) else {
        return;
    };
    let (live, rdram, rdram_len) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.runtime_rdram,
            host.runtime_rdram_len,
        )
    });
    let Some(live) = live else {
        return;
    };
    let Some(required_end) = live.mutation_state.as_ref().and_then(|state| {
        let state = state.borrow();
        state
            .active_host_transaction(thread)
            .map(|_| state.required_physical_end() as usize)
    }) else {
        return;
    };
    assert!(
        !rdram.is_null() && rdram_len >= required_end,
        "catalog host transaction checkpoint has no live RDRAM through watched end {required_end:#x}"
    );
    // SAFETY: this hook runs on the currently executing guest coroutine before
    // suspension. The process allocation is stable, and reads finish before
    // the yielder can transfer control to another coroutine.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    live.flush_active_host_abi_transaction_with(thread, |physical| unsafe {
        storage.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
    });
}

/// Move-only ownership of one synchronous child writer publication.
///
/// Dropping an uncommitted canonical token poisons the mutation owner. This
/// closes the unwind interleaving `parent prefix committed -> child mutates ->
/// child unwinds -> parent/guest resumes`: no later dispatch can accept the
/// possibly-partial child image as an unattributed host suffix.
pub(crate) struct CatalogNestedWriterTransactionV1 {
    live: Option<CanonicalLiveBlockProgramV1>,
    transaction_id: Option<u64>,
    thread: Option<ThreadId>,
    operation: &'static str,
    committed: bool,
}

impl CatalogNestedWriterTransactionV1 {
    fn is_canonical(&self) -> bool {
        self.transaction_id.is_some()
    }

    fn assert_thread_owner(&self) {
        if let Some(expected) = self.thread {
            assert_eq!(
                super::ACTIVE_THREAD_ID.with(Cell::get),
                Some(expected),
                "{} child writer transaction changed guest-thread owner before commit",
                self.operation
            );
        }
    }

    fn commit_with(mut self, mut read_physical_byte: impl FnMut(u32) -> u8) {
        self.assert_thread_owner();
        if let Some(live) = &self.live {
            if let Some(transaction_id) = self.transaction_id {
                live.mutation_state
                    .as_ref()
                    .expect("canonical child writer token lost its mutation state")
                    .borrow()
                    .assert_active_child_transaction(transaction_id);
            }
            live.invalidate_pending_physical_writes_with(&mut read_physical_byte);
            fn64_recomp_rs::discard_executable_write_boundary();
            if let Some(transaction_id) = self.transaction_id {
                live.mutation_state
                    .as_ref()
                    .expect("canonical child writer token lost its mutation state")
                    .borrow_mut()
                    .finish_child_transaction(transaction_id);
            }
        }
        self.committed = true;
    }

    pub(crate) fn commit(self, rdram: &[u8]) {
        let view = fn64_runtime::RdramView::from_storage(rdram);
        self.commit_with(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
    }

    fn commit_changed_bytes(self, rdram: &[u8], notify: impl Fn(u32, u32)) {
        self.assert_thread_owner();
        let live = self
            .live
            .as_ref()
            .expect("canonical child writer transaction lost its live owner");
        let state = live
            .mutation_state
            .as_ref()
            .expect("canonical child writer transaction has no mutation state");
        let view = fn64_runtime::RdramView::from_storage(rdram);
        let snapshot = state
            .borrow()
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        let changed = state.borrow().current_changed_ranges(&snapshot);
        for (physical_start, physical_end) in changed {
            notify(physical_start, physical_end - physical_start);
        }
        self.commit(rdram);
    }
}

/// Commit the selected `OSThread *` mirror through the canonical executable
/// mutation owner before the selected coroutine can execute. Compatibility
/// lanes return `false` and retain the legacy raw publication in `host.rs`.
///
/// This publication uses the HostAbi writer channel but intentionally does
/// not enter the host-call lifecycle trace: scheduler selection has no guest
/// call target/resume pair. Consequently the existing host-call-only
/// completion receipt remains open rather than miscounting this boundary.
pub(super) fn commit_scheduler_running_thread_mirror(
    origin: SchedulerRunningThreadMirrorV1,
) -> bool {
    let live = with_host(|host| host.canonical_recompiled_program.clone());
    let Some(live) = live else {
        return false;
    };
    let Some(state) = live.mutation_state.as_ref() else {
        return false;
    };
    let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    let physical_start = origin.global.offset();
    let physical_end = physical_start
        .checked_add(4)
        .expect("scheduler running-thread mirror range overflow");
    assert!(
        !rdram.is_null() && usize::try_from(physical_end).unwrap() <= rdram_len,
        "scheduler running-thread mirror for selected thread {} exceeds registered RDRAM: [{physical_start:#010x}, {physical_end:#010x}) vs {rdram_len:#x}",
        origin.selected_thread
    );

    // SAFETY: scheduler selection runs between coroutine resumes. The process
    // allocation is stable, and all raw reads/writes finish before the selected
    // coroutine receives RunToken.
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    live.reconcile_before_dispatch_with(|physical| unsafe {
        storage.read_u8(RdramAddr::from_offset(physical))
    });
    if unsafe { storage.read_u32(origin.global) } == origin.handle {
        return true;
    }

    let transaction_id = state.borrow_mut().begin_child_transaction();
    let transaction = CatalogNestedWriterTransactionV1 {
        live: Some(live),
        transaction_id: Some(transaction_id),
        thread: None,
        operation: "scheduler running-thread mirror",
        committed: false,
    };
    unsafe { storage.write_u32(origin.global, origin.handle) };
    fn64_recomp_rs::notify_host_abi_write(physical_start, 4);
    transaction
        .commit_with(|physical| unsafe { storage.read_u8(RdramAddr::from_offset(physical)) });
    true
}

impl Drop for CatalogNestedWriterTransactionV1 {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let Some(state) = self
            .live
            .as_ref()
            .and_then(|live| live.mutation_state.as_ref())
        else {
            return;
        };
        state.borrow_mut().poison(format!(
            "{} child writer transaction unwound before commit",
            self.operation
        ));
    }
}

pub(crate) fn begin_catalog_nested_writer(
    rdram: &[u8],
    operation: &'static str,
) -> CatalogNestedWriterTransactionV1 {
    let thread = super::ACTIVE_THREAD_ID.with(Cell::get);
    let live = with_host(|host| host.canonical_recompiled_program.clone());
    if let Some(live) = &live {
        if let Some(state) = &live.mutation_state {
            state.borrow().assert_not_poisoned();
        }
        if let Some(thread) = thread {
            let view = fn64_runtime::RdramView::from_storage(rdram);
            live.flush_active_host_abi_transaction_with(thread, |physical| {
                view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            });
        }
    }
    let transaction_id = live.as_ref().and_then(|live| {
        live.mutation_state
            .as_ref()
            .map(|state| state.borrow_mut().begin_child_transaction())
    });
    CatalogNestedWriterTransactionV1 {
        live,
        transaction_id,
        thread,
        operation,
        committed: false,
    }
}

/// Test-only ownership token for replacing executable-write preflight inputs.
///
/// The token is thread-bound because the guarded state lives in thread-local
/// storage. Nested scopes are supported and restore the immediately preceding
/// state, including while unwinding from an expected loud trap.
#[cfg(all(test, feature = "recomp-rs"))]
#[must_use = "dropping the guard restores the prior executable-write preflight state"]
pub(crate) struct TestExecutableWritePreflightState {
    prior_ranges: Vec<(u32, u32)>,
    prior_pending: Vec<(u32, u32)>,
    prior_attributed: Vec<GuestWriteEvent>,
    _thread_bound: std::marker::PhantomData<Rc<()>>,
}

#[cfg(all(test, feature = "recomp-rs"))]
impl Drop for TestExecutableWritePreflightState {
    fn drop(&mut self) {
        EXECUTABLE_WRITE_RANGES.with(|ranges| {
            PENDING_EXECUTABLE_WRITES.with(|pending| {
                let mut ranges = ranges.borrow_mut();
                let mut pending = pending.borrow_mut();
                *ranges = std::mem::take(&mut self.prior_ranges);
                *pending = std::mem::take(&mut self.prior_pending);
            });
        });
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| {
            *pending.borrow_mut() = std::mem::take(&mut self.prior_attributed);
        });
    }
}

#[cfg(all(test, feature = "recomp-rs"))]
pub(crate) fn scoped_test_executable_write_preflight_state(
    ranges: Vec<(u32, u32)>,
    pending: Vec<(u32, u32)>,
) -> TestExecutableWritePreflightState {
    let (prior_ranges, prior_pending) = EXECUTABLE_WRITE_RANGES.with(|current_ranges| {
        PENDING_EXECUTABLE_WRITES.with(|current_pending| {
            let mut current_ranges = current_ranges.borrow_mut();
            let mut current_pending = current_pending.borrow_mut();
            (
                std::mem::replace(&mut *current_ranges, ranges),
                std::mem::replace(&mut *current_pending, pending),
            )
        })
    });
    let prior_attributed = PENDING_ATTRIBUTED_EXECUTABLE_WRITES
        .with(|current| std::mem::take(&mut *current.borrow_mut()));
    TestExecutableWritePreflightState {
        prior_ranges,
        prior_pending,
        prior_attributed,
        _thread_bound: std::marker::PhantomData,
    }
}

/// Reject a planned host publication that would require fallible executable
/// generation after the guest bytes become visible.
///
/// The verified-audio adapter has no transaction spanning RDRAM, device state,
/// and native-code installation. This read-only check lets ordinary audio-data
/// writes proceed while forcing executable writes to remain on the interpreter
/// path until such a transaction exists.
#[cfg(test)]
pub(crate) fn preflight_non_executable_host_writes(
    writes: &[(usize, usize)],
) -> Result<(), String> {
    EXECUTABLE_WRITE_RANGES.with(|ranges| {
        let ranges = ranges.borrow();
        let pending = PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone());
        for (start, len) in pending {
            let end = start.saturating_add(len);
            if let Some(&(executable_start, executable_end)) = ranges
                .iter()
                .find(|&&(executable_start, executable_end)| {
                    start < executable_end && end > executable_start
                })
            {
                return Err(format!(
                    "pending host write [{start:#010x}, {end:#010x}) overlaps live executable region [{executable_start:#010x}, {executable_end:#010x}); transactional executable publication is unavailable"
                ));
            }
        }
        for &(start, end) in writes {
            let start = u32::try_from(start)
                .map_err(|_| format!("host write start {start:#x} exceeds physical address space"))?;
            let end = u32::try_from(end)
                .map_err(|_| format!("host write end {end:#x} exceeds physical address space"))?;
            if let Some(&(executable_start, executable_end)) = ranges
                .iter()
                .find(|&&(executable_start, executable_end)| {
                    start < executable_end && end > executable_start
                })
            {
                return Err(format!(
                    "verified audio write [{start:#010x}, {end:#010x}) overlaps live executable region [{executable_start:#010x}, {executable_end:#010x}); transactional executable publication is unavailable"
                ));
            }
        }
        Ok(())
    })
}

fn pause_active_recompiled_thread() {
    super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
}

fn read_raw_mmio(vaddr: u64) -> Option<u32> {
    crate::pi::read_raw_mmio_word(vaddr)
}

fn write_raw_mmio(vaddr: u64, value: u32) -> bool {
    crate::pi::write_raw_mmio_word(vaddr, value)
}

fn record_recompiled_unsupported(context: &str) {
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Recompiler,
        "recompiler.cpu.unsupported-instruction",
        context,
        Some(fn64_runtime::Cycles::new(crate::sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
}

fn recompiled_gap_panic(context: impl Into<String>) -> ! {
    let context = context.into();
    record_recompiled_unsupported(&context);
    panic!("{context}")
}

/// Create and start thread 0 with a typed recompiled entrypoint on fn64's existing
/// single executor. No second executor, RDRAM allocation, or host thread is
/// created.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`super::boot_thread0`]'s existing C ABI contract.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram, rdram_len, lookup, entry, None, None, thread_id, priority,
        )
    };
}

/// Boot the function lane while binding its stable generated-artifact
/// identity for pointer-independent release evidence.
///
/// # Safety
/// Identical to [`boot_thread0`]. The identity describes the native artifact
/// containing both `lookup` and `entry`; it is not derived from either pointer.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_with_artifact_identity(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: ProgramArtifactIdentity,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram,
            rdram_len,
            lookup,
            entry,
            Some(artifact_identity),
            None,
            thread_id,
            priority,
        )
    };
}

/// Boot an artifact emitted with authoritative whole-function entry
/// observation enabled.
///
/// `schema` must be the generated artifact's
/// `FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` export.
///
/// # Safety
/// Identical to [`boot_thread0`]. The artifact identity and schema must both
/// describe the native artifact containing `lookup` and `entry`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_with_execution_observation(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: ProgramArtifactIdentity,
    schema: FunctionEntryObservationSchema,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_config(
            rdram,
            rdram_len,
            lookup,
            entry,
            Some(artifact_identity),
            Some(schema),
            thread_id,
            priority,
        )
    };
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_config(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    artifact_identity: Option<ProgramArtifactIdentity>,
    observation_schema: Option<FunctionEntryObservationSchema>,
    thread_id: ThreadId,
    priority: Priority,
) {
    match (artifact_identity, observation_schema) {
        (Some(identity), Some(schema)) => {
            set_entry_lookup_with_execution_observation(lookup, rdram_len, identity, schema);
        }
        (identity, None) => set_entry_lookup_config(lookup, rdram_len, identity),
        (None, Some(_)) => {
            panic!("function entry observation requires a stable generated-artifact identity")
        }
    }
    unsafe { super::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));

    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees the allocation outlives all
                // executor coroutines and contains exactly `rdram_len` bytes.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                entry(&mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Create and start thread 0 with the bank-qualified arbitrary-PC execution
/// program as the live owner. Each instruction checkpoint suspends back to
/// the existing executor, which charges guest instructions to virtual time
/// and services due devices before any coroutine resumes.
///
/// `boot_context` must be the black-box IPL3 handoff captured for the exact
/// installed normalized ROM. Its entry PC and TV standard are checked before
/// the coroutine is created.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`boot_thread0`]'s existing shared-allocation contract.
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_block_program(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_block_program_config(
            rdram,
            rdram_len,
            program,
            entry,
            boot_context,
            entry_lookup,
            transfer_lookup,
            budget,
            None,
            thread_id,
            priority,
        )
    };
}

/// Boot the block lane while binding the stable artifact which supplies its
/// entry/transfer resolver implementations.
///
/// # Safety
/// Identical to [`boot_thread0_block_program`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_block_program_with_artifact_identity(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: ProgramArtifactIdentity,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe {
        boot_thread0_block_program_config(
            rdram,
            rdram_len,
            program,
            entry,
            boot_context,
            entry_lookup,
            transfer_lookup,
            budget,
            Some(dispatch_artifact_identity),
            thread_id,
            priority,
        )
    };
}

/// Boot the callback-free canonical static catalog lane.
///
/// The consumed install is the sole owner of the program, entry, instruction
/// budget, host targets, and dispatch identity. Dynamic image activation is
/// intentionally unavailable in this first authority-capable path: an image
/// change traps loudly instead of entering a legacy builder or resolver.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`boot_thread0_block_program`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_program_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogResolverInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_block_program(install, rdram_len);
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical catalog with explicit operational exact-unit fallback.
/// The dynamic catalog is source-bound and remains outside immutable static
/// program evidence; enabling it makes every static writer/release authority
/// constructor fail with `DynamicExecutionInstalled`.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_program_v1`].
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_program_with_dynamic_mapped_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogResolverInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_block_program(install, rdram_len);
    live.enable_dynamic_mapped_execution();
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical catalog lane with a closed, explicitly physically
/// backed precompiled-generation inventory.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_program_v1`].
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_generation_program_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_generation_program(install, rdram_len);
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the closed precompiled-generation catalog with explicit operational
/// exact-unit fallback for destinations absent from usable AOT.
///
/// # Safety
/// Identical to [`boot_thread0_catalog_generation_program_v1`].
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub unsafe fn boot_thread0_catalog_generation_program_with_dynamic_mapped_v1(
    rdram: *mut u8,
    rdram_len: usize,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let live = set_catalog_generation_program(install, rdram_len);
    live.enable_dynamic_mapped_execution();
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        )
    };
}

/// Boot the canonical generation lane from an owned, validated bootstrap
/// allocation. Unlike the raw-pointer compatibility entry above, this API
/// retains allocation ownership inside HostState and initializes mutation
/// evidence from the bootstrap receipt before the first guest dispatch.
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState now exclusively owns this stable allocation, no guest
    // coroutine has been created or resumed, and all reads finish before the
    // thread-0 constructor below. The validator also rejects any pending
    // writer event or open transaction in the actual live mutation owner.
    let installed_storage = unsafe { std::slice::from_raw_parts(rdram, installed_len) };
    live.mint_bootstrap_writer_completion(installed_storage)
        .unwrap_or_else(|error| panic!("minting bootstrap writer-channel authority: {error}"));
    // SAFETY: the just-installed owned allocation covers physical RDRAM and
    // cannot be moved while HostState retains it. This canonical typed-Rust
    // lane intercepts MMIO; the legacy generated-C sparse mirror is neither
    // read nor synchronized here.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

/// Boot a validated, owned generation catalog with operational exact-unit
/// fallback for destinations absent from usable AOT.
///
/// This preserves the bootstrap, ROM, and executable-mutation provenance used
/// by the canonical generation lane, but deliberately does not mint static
/// writer-channel or release authority: dynamic execution is part of the
/// installed program from its first dispatch.
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_with_dynamic_mapped_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    live.enable_dynamic_mapped_execution();
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState exclusively owns this stable allocation for the
    // lifetime of every executor coroutine created below.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

/// Boot a validated, owned generation catalog while forcing its exact static
/// canonical entry once through the operational dynamic mapped executor.
///
/// The selected key must equal the install entry and resolve to itself in the
/// installed static catalog. The redirect remains armed across a zero-work
/// budget rejection and clears only after that attempted key charges work.
/// The immutable program and its identity are retained unchanged; withholding
/// is applied only at the unified dispatch seam and cannot mint static writer
/// or release authority.
#[cfg(feature = "dynamic-mapped-runtime")]
#[allow(clippy::too_many_arguments)]
pub fn boot_thread0_validated_catalog_generation_program_with_exact_static_key_withheld_v1(
    validated: ValidatedBootstrapRdramV1,
    install: CatalogGenerationInstallV1,
    withheld_static_key: ExecutionKey,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) -> Result<(), BootstrapImportErrorV1> {
    validate_bootstrap_binding(&validated, &install)?;
    let receipt = validated.receipt.evidence();
    let installed_rom = with_host(|host| host.installed_rom);
    if !installed_rom.is_some_and(|installed| {
        installed.byte_len == receipt.rom_byte_len && installed.sha256 == receipt.rom_sha256
    }) {
        return Err(BootstrapImportErrorV1::InstalledRomMismatch);
    }
    let entry = install.resolver.entry();
    validate_block_boot_context(entry.pc, &boot_context);
    let rdram_len = validated.storage.len();
    let live = set_catalog_program_parts(
        install.resolver,
        Some(install.generations),
        rdram_len,
        Some(&validated),
    );
    live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(withheld_static_key);
    let ValidatedBootstrapRdramV1 { storage, .. } = validated;
    let (rdram, installed_len) = crate::host::install_owned_process_rdram(storage);
    assert_eq!(installed_len, rdram_len);
    // SAFETY: HostState exclusively owns this stable allocation for the
    // lifetime of every executor coroutine created below.
    unsafe {
        boot_thread0_catalog_live_v1(
            rdram,
            rdram_len,
            live,
            entry,
            boot_context,
            thread_id,
            priority,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_catalog_live_v1(
    rdram: *mut u8,
    rdram_len: usize,
    live: CanonicalLiveBlockProgramV1,
    entry: ExecutionKey,
    boot_context: BootContext,
    thread_id: ThreadId,
    priority: Priority,
) {
    unsafe { super::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));

    let rdram_addr = rdram as usize;
    let boot_return_pc = boot_context.gprs[31] as u32;
    // The coroutine implementation transfers the closure onto its native
    // stack with a fixed 1 KiB object limit. Keep the growing canonical owner
    // behind one pointer so adding evidence fields cannot silently make boot
    // construction exceed that architectural transfer bound.
    let live = Rc::new(live);
    with_executor(|exec| {
        exec.restore_cp0_clock(
            boot_context.cp0.registers[9] as u32,
            boot_context.cp0.registers[11] as u32,
            boot_context.cp0.registers[13] & CpuInterruptLine::TIMER.cause_bit() as u64 != 0,
        );
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees this one allocation
                // outlives every executor coroutine.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                ctx.restore_boot_context(&boot_context)
                    .unwrap_or_else(|error| panic!("restoring catalog BootContext: {error}"));
                validate_restored_catalog_boot_context(entry, &boot_context, &ctx);
                ctx.set_thread_return_pc(Some(boot_return_pc));
                run_catalog_block_program(live.as_ref(), entry, &mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

fn validate_block_boot_context(entry: GuestPc, boot_context: &BootContext) {
    boot_context
        .validate_for_entry(entry.get())
        .unwrap_or_else(|error| panic!("block-lane BootContext rejected: {error}"));
    let expected_tv_type = match boot_context.region.tv_standard {
        BootTvStandard::Ntsc => fn64_runtime::TvType::Ntsc,
        BootTvStandard::Pal => fn64_runtime::TvType::Pal,
        BootTvStandard::Mpal => fn64_runtime::TvType::Mpal,
    };
    with_host(|host| {
        let installed = host
            .installed_rom
            .unwrap_or_else(|| panic!("block-lane BootContext requires an installed ROM"));
        assert_eq!(
            installed.sha256,
            boot_context.normalized_rom_sha256.bytes(),
            "block-lane BootContext normalized ROM identity does not match the installed ROM"
        );
        assert_eq!(
            host.device_fabric.tv_type(),
            Some(expected_tv_type),
            "block-lane BootContext TV standard does not match the configured device fabric"
        );
    });
}

fn validate_restored_catalog_boot_context(
    entry: ExecutionKey,
    boot_context: &BootContext,
    ctx: &RsContext,
) {
    assert_eq!(
        entry.pc.get(),
        boot_context.entry_pc,
        "catalog dispatch entry differs from the validated BootContext entry"
    );
    let mismatches = ctx
        .boot_context_state_mismatches(boot_context)
        .expect("validating restored catalog BootContext");
    assert!(
        mismatches.is_empty(),
        "catalog context differs from BootContext before first unified dispatch: {mismatches:?}"
    );
}

#[allow(clippy::too_many_arguments)]
unsafe fn boot_thread0_block_program_config(
    rdram: *mut u8,
    rdram_len: usize,
    program: BlockProgram,
    entry: ExecutionKey,
    boot_context: BootContext,
    entry_lookup: ProgramEntryLookup,
    transfer_lookup: ProgramTransferLookup,
    budget: InstructionBudget,
    dispatch_artifact_identity: Option<ProgramArtifactIdentity>,
    thread_id: ThreadId,
    priority: Priority,
) {
    validate_block_boot_context(entry.pc, &boot_context);

    let live = LiveBlockProgram {
        program: Rc::new(RefCell::new(program)),
        entry_lookup,
        transfer_lookup,
        budget,
        dispatch_artifact_identity,
        executable_regions: Rc::new(RefCell::new(Vec::new())),
        precompiled_generations: Rc::new(RefCell::new(None)),
    };
    set_block_program(live.clone(), rdram_len);
    unsafe { super::register_process_rdram(rdram, rdram_len) };
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));
    fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
    fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));

    let rdram_addr = rdram as usize;
    let boot_return_pc = boot_context.gprs[31] as u32;
    with_executor(|exec| {
        exec.restore_cp0_clock(
            boot_context.cp0.registers[9] as u32,
            boot_context.cp0.registers[11] as u32,
            boot_context.cp0.registers[13] & CpuInterruptLine::TIMER.cause_bit() as u64 != 0,
        );
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees this one allocation
                // outlives every executor coroutine.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                ctx.restore_boot_context(&boot_context)
                    .unwrap_or_else(|error| panic!("restoring block-lane BootContext: {error}"));
                // IPL3 enters the ROM header with `jalr`, so a normal return
                // targets the captured `$ra` in SP DMEM rather than the
                // synthetic sentinel used for later OSThreads. That return
                // terminates the bootstrap coroutine; IPL3 is outside the
                // game AOT pack and must not be admitted as guest game code.
                ctx.set_thread_return_pc(Some(boot_return_pc));
                run_block_program(&live, entry, &mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

fn park_host_scheduled_exception(
    canonical_live: Option<&CanonicalLiveBlockProgramV1>,
    fault: CpuFault,
    ctx: &mut RsContext,
) -> bool {
    let CpuFaultKind::Exception { exception, .. } = fault.kind else {
        return false;
    };
    let host_scheduled = super::ACTIVE_THREAD_ID
        .with(|active| active.get())
        .is_some_and(|thread| with_host(|host| host.thread_handle_vrams.contains_key(&thread)));
    if std::env::var_os("FN64_PROFILE_EXCEPTIONS").is_some() {
        let active = super::ACTIVE_THREAD_ID.with(|active| active.get());
        let handles = with_host(|host| host.thread_handle_vrams.clone());
        eprintln!(
            "[fn64-exception-profile] exception={exception:?} active={active:?} thread_handles={handles:?} host_scheduled={host_scheduled}"
        );
    }
    if !host_scheduled {
        return false;
    }

    fault.enter_exception(ctx).unwrap_or_else(|| {
        unreachable!("typed Exception fault must enter architectural exception state")
    });
    // Public libultra event numbering: BREAK is 10 and FAULT is 12. Missing
    // registration is valid for these synchronous events; the current thread
    // still stops, while a registered debugger/fault manager receives its
    // configured message through the executor's single queue path.
    let event = if exception == CpuException::Breakpoint {
        10
    } else {
        12
    };
    with_executor(|executor| {
        executor.inject_optional_os_event(event);
    });
    if let Some(live) = canonical_live {
        live.publish_parked_fault(fault, ctx);
    }
    let resumed = super::suspend_active_coroutine(fn64_runtime::Yield::StopSelf);
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Recompiler,
        "recompiler.cpu.fault-context-resume",
        format!(
            "faulted guest thread was explicitly restarted after {exception:?} with {resumed:?}; resuming a saved fault context is not implemented"
        ),
        Some(fn64_runtime::Cycles::new(crate::sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!(
        "faulted guest thread was explicitly restarted after {exception:?} with {resumed:?}; \
         resuming a saved fault context is not implemented"
    );
}

fn run_block_program(
    live: &LiveBlockProgram,
    mut entry: ExecutionKey,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    loop {
        // Exact timer interleaving: checkpoint suspension -> executor advances
        // Count and latches Compare/IP7 -> coroutine resume -> this sample ->
        // exception entry before the resumed guest block. Sampling after
        // dispatch would allow that block to run once with an overdue timer.
        let (count, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, entry.pc) {
            entry = live.resolve_transfer(entry.bank, vector).unwrap_or_else(|fault| {
                panic!(
                    "live BlockProgram interrupt vector {vector} from {entry} does not resolve: {fault:?}"
                )
            });
        }
        let mut resolver = LiveTransferResolver { live: live.clone() };
        let dispatched = {
            let program = live.program.borrow();
            program
                .dispatch_exposing_exceptions(entry, live.budget, ctx, mem, &mut resolver)
                .unwrap_or_else(|error| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram dispatch failed at {entry}: {error}"
                    ))
                })
        };
        process_executable_writes(live, |offset| {
            mem.load_b(0xFFFF_FFFF_8000_0000u64 + u64::from(offset)) as u8
        });
        let image_changed_entry = match dispatched.exit {
            BlockExit::ImageChanged { at, miss } => Some(
                activate_fetch_generation(live, at, miss, |offset| {
                    mem.load_b(0xFFFF_FFFF_8000_0000u64 + u64::from(offset)) as u8
                })
                .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let unresolved_generation_entry = match dispatched.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind:
                    fn64_recomp_rs::CpuFaultKind::UnknownBank
                    | fn64_recomp_rs::CpuFaultKind::UnmappedPc { .. }
                    | fn64_recomp_rs::CpuFaultKind::UnmappedPhysicalInstruction { .. },
            }) => live
                .precompiled_generations
                .borrow_mut()
                .as_mut()
                .and_then(|catalog| {
                    match catalog.activate_for_fetch_with(at.pc, |vaddr| {
                        mem.load_bu(0xffff_ffff_0000_0000u64 | u64::from(vaddr))
                    }) {
                        Ok(resolution) => Some(resolution.entry),
                        Err(GenerationLookupError::UnmappedPc { .. }) => None,
                        Err(error) => recompiled_gap_panic(format!(
                            "closed AOT pack could not activate attempted fetch at {}: {error}",
                            at.pc
                        )),
                    }
                }),
            _ => None,
        };
        let executable_write_fault_entry = match dispatched.exit {
            BlockExit::ExecutableWriteFault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                Some(
                    live.resolve_transfer(fault.at.bank, vector)
                        .unwrap_or_else(|mapping_fault| {
                            recompiled_gap_panic(format!(
                                "live BlockProgram executable-write exception vector {vector} does not resolve after generation replacement: {mapping_fault:?}"
                            ))
                        }),
                )
            }
            _ => None,
        };
        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            // Commit a handler's same-value Compare write before suspending:
            // otherwise checkpoint time could advance while the executor's
            // IP7 latch remained set, causing an acknowledged interrupt to
            // re-enter immediately after ERET.
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            super::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
        }
        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = live
                    .resolve_transfer(next.bank, next.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram checkpoint {next} no longer resolves: {fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = live
                    .resolve_transfer(source_bank, resume.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram executable-write resume {resume} no longer resolves after generation replacement: {fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                let mut resolver = LiveTransferResolver { live: live.clone() };
                match resolver.resolve_call(source_bank, target_pc, resume) {
                    Ok(CallResolution::Guest(next)) => entry = next,
                    Ok(CallResolution::Host) => {
                        let host = fn64_recomp_rs::resolve_host_function(target_pc.get())
                            .unwrap_or_else(|| {
                                recompiled_gap_panic(format!(
                                    "live BlockProgram requested unknown host call {:#010x}",
                                    target_pc.get()
                                ))
                            });
                        invoke_observed_block_host(target_pc, resume, host, ctx, mem);
                        entry = live
                            .resolve_transfer(source_bank, resume.pc)
                            .unwrap_or_else(|fault| {
                                recompiled_gap_panic(format!(
                                    "live BlockProgram executable-write host resume {resume} no longer resolves after generation replacement: {fault:?}"
                                ))
                            });
                    }
                    Err(fault) => recompiled_gap_panic(format!(
                        "live BlockProgram executable-write call target {target_pc} does not resolve after generation replacement: {fault:?}"
                    )),
                }
            }
            BlockExit::ExecutableWriteFault(_) => {
                entry = executable_write_fault_entry.unwrap_or_else(|| {
                    unreachable!(
                        "executable-write fault continuation was not prepared before suspension"
                    )
                });
            }
            BlockExit::ImageChanged { .. } => {
                entry = image_changed_entry.unwrap_or_else(|| {
                    unreachable!("image-change continuation was not prepared before suspension")
                });
            }
            BlockExit::HostCall { vram, resume } => {
                let host = fn64_recomp_rs::resolve_host_function(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram requested unknown host call {:#010x}",
                        vram.get()
                    ))
                });
                invoke_observed_block_host(vram, resume, host, ctx, mem);
                entry = live
                    .resolve_transfer(resume.bank, resume.pc)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "live BlockProgram host resume {resume} no longer resolves: {fault:?}"
                        ))
                    });
            }
            BlockExit::ThreadReturn => return,
            BlockExit::Fault(_) if unresolved_generation_entry.is_some() => {
                entry = unresolved_generation_entry
                    .expect("attempted-fetch generation was checked above");
            }
            BlockExit::Fault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "live BlockProgram returned {:?} without guest progress",
                    dispatched.exit
                );
                if park_host_scheduled_exception(None, fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                // Architectural exceptions (mid-function BREAK/SYSCALL and the
                // conditional traps, which the block emitter renders as
                // `BlockExit::Fault { kind: Exception }`) are vectored through
                // the installed handler exactly like the executable-write
                // boundary above: `enter_exception` commits EPC/EXL/Cause.BD
                // and returns the BEV-selected vector, then the handler bank is
                // resolved as an ordinary transfer. Only a genuinely
                // non-architectural fault (a real lane gap) stays loud.
                let fault_bank = fault.at.bank;
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    let destinations = live.program.borrow().copy_execution_destinations();
                    let recent_start = destinations.len().saturating_sub(16);
                    let indirect = ctx.indirect_transfer_observations();
                    let indirect_start = indirect.len().saturating_sub(8);
                    recompiled_gap_panic(format!(
                        "live BlockProgram stopped on non-architectural guest fault: {fault:?}; current CP0 status={:#010x} cause={:#010x} epc={:#010x} badvaddr={:#018x}; recent entered destinations={:?}; recent indirect transfers={:?}",
                        ctx.cop0_status,
                        ctx.cop0_cause,
                        ctx.cop0_epc,
                        ctx.cop0_badvaddr,
                        &destinations[recent_start..],
                        &indirect[indirect_start..],
                    ))
                });
                entry = live.resolve_transfer(fault_bank, vector).unwrap_or_else(|mapping_fault| {
                    recompiled_gap_panic(format!(
                        "live BlockProgram exception vector {vector} does not resolve: {mapping_fault:?}"
                    ))
                });
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("BlockProgram::dispatch returned an internal transfer boundary")
            }
        }
    }
}

fn resolve_catalog_transfer_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<ExecutionKey, String> {
    match live.resolve_transfer(source_bank, target_pc) {
        Ok(entry) => Ok(entry),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            live.activate_for_fetch(target_pc, mem)
                .map_err(|error| format!("generation activation at {target_pc} failed: {error}"))?;
            live.resolve_transfer(source_bank, target_pc)
                .map_err(|fault| format!("activated target {target_pc} did not resolve: {fault}"))
        }
        Err(fault) => Err(format!("target {target_pc} does not resolve: {fault}")),
    }
}

fn resolve_catalog_entry_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<ExecutionKey, String> {
    match live.resolve_entry(target_pc) {
        Ok(entry) => Ok(entry),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            live.activate_for_fetch(target_pc, mem)
                .map_err(|error| format!("generation activation at {target_pc} failed: {error}"))?;
            live.resolve_entry(target_pc)
                .map_err(|fault| format!("activated entry {target_pc} did not resolve: {fault}"))
        }
        Err(fault) => Err(format!("entry {target_pc} does not resolve: {fault}")),
    }
}

fn resolve_catalog_call_with_activation(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<CatalogCallResolutionV1, String> {
    match live.resolve_call(source_bank, target_pc) {
        Ok(resolution) => Ok(resolution),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => {
            live.activate_for_fetch(target_pc, mem).map_err(|error| {
                format!("call generation activation at {target_pc} failed: {error}")
            })?;
            live.resolve_call(source_bank, target_pc).map_err(|fault| {
                format!("activated call target {target_pc} did not resolve: {fault}")
            })
        }
        Err(fault) => Err(format!("call target {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnifiedCatalogTargetV1 {
    Static(ExecutionKey),
    Dynamic {
        source_bank: BankId,
        target_pc: GuestPc,
    },
}

#[cfg(feature = "dynamic-mapped-runtime")]
impl UnifiedCatalogTargetV1 {
    const fn key(self) -> ExecutionKey {
        match self {
            Self::Static(entry) => entry,
            Self::Dynamic {
                source_bank,
                target_pc,
            } => ExecutionKey::new(source_bank, target_pc),
        }
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn dynamic_fallback_eligible(fault: CpuFault) -> bool {
    matches!(
        fault.kind,
        CpuFaultKind::UnknownBank
            | CpuFaultKind::UnmappedPc { .. }
            | CpuFaultKind::UnmappedPhysicalInstruction { .. }
            | CpuFaultKind::StaleInstructionIdentity { .. }
            | CpuFaultKind::MissingAotEntry
    )
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn resolve_unified_catalog_target(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogTargetV1, String> {
    match live.resolve_transfer(source_bank, target_pc) {
        Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
        Err(CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => match live.activate_for_fetch(target_pc, mem) {
            Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
            Err(GenerationLookupError::AotMiss(_) | GenerationLookupError::UnmappedPc { .. }) => {
                Ok(UnifiedCatalogTargetV1::Dynamic {
                    source_bank,
                    target_pc,
                })
            }
            Err(error @ GenerationLookupError::AmbiguousLiveImage { .. }) => Err(format!(
                "generation activation at {target_pc} is ambiguous: {error}"
            )),
            Err(error) => Err(format!(
                "generation activation at {target_pc} did not produce an executable owner: {error}"
            )),
        },
        Err(fault) if dynamic_fallback_eligible(fault) => Ok(UnifiedCatalogTargetV1::Dynamic {
            source_bank,
            target_pc,
        }),
        Err(fault) => Err(format!("target {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn resolve_unified_catalog_entry(
    live: &CanonicalLiveBlockProgramV1,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogTargetV1, String> {
    match live.resolve_entry(target_pc) {
        Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
        Err(fault @ CpuFault {
            kind: CpuFaultKind::NoActiveGeneration,
            ..
        }) => match live.activate_for_fetch(target_pc, mem) {
            Ok(entry) => Ok(UnifiedCatalogTargetV1::Static(entry)),
            Err(GenerationLookupError::AotMiss(_) | GenerationLookupError::UnmappedPc { .. }) => {
                Ok(UnifiedCatalogTargetV1::Dynamic {
                    source_bank: fault.at.bank,
                    target_pc,
                })
            }
            Err(error @ GenerationLookupError::AmbiguousLiveImage { .. }) => Err(format!(
                "entry generation activation at {target_pc} is ambiguous: {error}"
            )),
            Err(error) => Err(format!(
                "entry generation activation at {target_pc} did not produce an executable owner: {error}"
            )),
        },
        Err(fault) if dynamic_fallback_eligible(fault) => {
            Ok(UnifiedCatalogTargetV1::Dynamic {
                source_bank: fault.at.bank,
                target_pc,
            })
        }
        Err(fault) => Err(format!("entry {target_pc} does not resolve: {fault}")),
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
enum UnifiedCatalogCallV1 {
    Host,
    Guest(UnifiedCatalogTargetV1),
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn resolve_unified_catalog_call(
    live: &CanonicalLiveBlockProgramV1,
    source_bank: BankId,
    target_pc: GuestPc,
    mem: &Rdram<'_>,
) -> Result<UnifiedCatalogCallV1, String> {
    if let Some(host) = live.install.resolve_host(target_pc.get()) {
        let _ = host;
        return Ok(UnifiedCatalogCallV1::Host);
    }
    resolve_unified_catalog_target(live, source_bank, target_pc, mem)
        .map(UnifiedCatalogCallV1::Guest)
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn checked_add_unified_work(
    instructions: &mut u32,
    blocks: &mut u32,
    added_instructions: u32,
    added_blocks: u32,
) -> Result<(), String> {
    *instructions = instructions
        .checked_add(added_instructions)
        .ok_or_else(|| "unified catalog instruction count overflow".to_string())?;
    *blocks = blocks
        .checked_add(added_blocks)
        .ok_or_else(|| "unified catalog block count overflow".to_string())?;
    Ok(())
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn dispatch_unified_catalog_slice(
    live: &CanonicalLiveBlockProgramV1,
    mut target: UnifiedCatalogTargetV1,
    budget: InstructionBudget,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) -> Result<fn64_recomp_rs::DispatchRun, String> {
    let mut instructions = 0u32;
    let mut blocks = 0u32;

    loop {
        if let UnifiedCatalogTargetV1::Static(entry) = target {
            if live.dynamic_withheld_static_key.get() == Some(entry) {
                target = UnifiedCatalogTargetV1::Dynamic {
                    source_bank: entry.bank,
                    target_pc: entry.pc,
                };
            }
        }
        let remaining = budget
            .get()
            .checked_sub(instructions)
            .ok_or_else(|| "unified catalog consumed more than its slice budget".to_string())?;
        if remaining < InstructionBudget::MIN {
            return Ok(fn64_recomp_rs::DispatchRun {
                exit: BlockExit::Checkpoint(target.key()),
                instructions,
                blocks,
            });
        }
        let turn_budget = InstructionBudget::new(remaining)
            .expect("unified catalog remaining budget was checked");
        let was_dynamic = matches!(target, UnifiedCatalogTargetV1::Dynamic { .. });
        let run = match target {
            UnifiedCatalogTargetV1::Static(entry) => {
                let dispatched = live
                    .dispatch_exposing_exceptions_at_budget(entry, turn_budget, ctx, mem)
                    .map_err(|error| {
                        format!("static catalog dispatch failed at {entry}: {error}")
                    })?;
                checked_add_unified_work(
                    &mut instructions,
                    &mut blocks,
                    dispatched.instructions,
                    dispatched.blocks,
                )?;
                fn64_recomp_rs::BlockRun::new(dispatched.exit, dispatched.instructions)
            }
            UnifiedCatalogTargetV1::Dynamic {
                source_bank,
                target_pc,
            } => {
                let attempted = ExecutionKey::new(source_bank, target_pc);
                let result = {
                    // The exact-unit catalog mutates only its identity map.
                    // Its RefMut may span this one non-suspending interpreter
                    // turn because guest-write/MMIO observers cannot re-enter
                    // catalog dispatch; it is dropped before reconciliation,
                    // host calls, or coroutine suspension.
                    let mut dynamic = live.dynamic_units.borrow_mut();
                    let catalog = dynamic.as_mut().expect(
                        "unified dynamic target exists without an installed dynamic catalog",
                    );
                    catalog.activate_and_run(attempted, turn_budget, ctx, mem, |bank| {
                        live.reserves_bank(bank)
                    })
                };
                match result {
                    Ok(dynamic) => {
                        if dynamic.run.instructions > remaining {
                            return Err(format!(
                                "dynamic mapped unit at {attempted} executed {} instructions with budget {remaining}",
                                dynamic.run.instructions
                            ));
                        }
                        if dynamic.run.instructions == 0
                            && matches!(
                                dynamic.run.exit,
                                BlockExit::Checkpoint(at) if at == dynamic.entry
                            )
                            && !turn_budget
                                .can_fit(0, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
                        {
                            if instructions > 0 {
                                return Ok(fn64_recomp_rs::DispatchRun {
                                    exit: BlockExit::Checkpoint(attempted),
                                    instructions,
                                    blocks,
                                });
                            }
                            return Err(
                                fn64_recomp_rs::DispatchError::IndivisibleUnitExceedsBudget {
                                    at: dynamic.entry,
                                    budget: turn_budget,
                                    required: InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS,
                                }
                                .to_string(),
                            );
                        }
                        live.record_dynamic_execution(attempted, &dynamic);
                        if dynamic.run.instructions > 0
                            && live.dynamic_withheld_static_key.get() == Some(attempted)
                        {
                            live.dynamic_withheld_static_key.set(None);
                        }
                        checked_add_unified_work(
                            &mut instructions,
                            &mut blocks,
                            dynamic.run.instructions,
                            1,
                        )?;
                        dynamic.run
                    }
                    Err(fn64_recomp_rs::DynamicMappedErrorV1::Fetch {
                        fault,
                        attempted_instructions,
                    }) => {
                        if attempted_instructions > remaining {
                            return Err(format!(
                                "dynamic fetch at {attempted} charged {attempted_instructions} instructions with budget {remaining}"
                            ));
                        }
                        checked_add_unified_work(
                            &mut instructions,
                            &mut blocks,
                            attempted_instructions,
                            0,
                        )?;
                        return Ok(fn64_recomp_rs::DispatchRun {
                            exit: BlockExit::Fault(fault),
                            instructions,
                            blocks,
                        });
                    }
                    Err(error) => {
                        return Err(format!(
                            "dynamic mapped activation at {attempted} failed: {error}"
                        ));
                    }
                }
            }
        };

        live.invalidate_pending_physical_writes(mem);
        live.reconcile_before_dispatch(mem);

        if run.instructions == 0
            && matches!(
                run.exit,
                BlockExit::Transfer(_)
                    | BlockExit::ResolveTransfer { .. }
                    | BlockExit::ResolveCall { .. }
                    | BlockExit::ExecutableWrite { .. }
                    | BlockExit::ExecutableWriteResolveCall { .. }
                    | BlockExit::ExecutableWriteFault(_)
            )
        {
            return Err(format!(
                "unified catalog continuing exit made no progress at {}: {:?}",
                target.key(),
                run.exit
            ));
        }

        match run.exit {
            BlockExit::Transfer(next) => {
                target = resolve_unified_catalog_target(live, next.bank, next.pc, mem)?;
            }
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc,
            } => {
                target = resolve_unified_catalog_target(live, source_bank, target_pc, mem)?;
            }
            BlockExit::ResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolve_unified_catalog_call(live, source_bank, target_pc, mem)? {
                UnifiedCatalogCallV1::Host => {
                    return Ok(fn64_recomp_rs::DispatchRun {
                        exit: BlockExit::HostCall {
                            vram: target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                UnifiedCatalogCallV1::Guest(next) => target = next,
            },
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                target = resolve_unified_catalog_target(live, source_bank, resume.pc, mem)?;
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolve_unified_catalog_call(live, source_bank, target_pc, mem)? {
                UnifiedCatalogCallV1::Host => {
                    return Ok(fn64_recomp_rs::DispatchRun {
                        exit: BlockExit::HostCall {
                            vram: target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                UnifiedCatalogCallV1::Guest(next) => target = next,
            },
            BlockExit::ImageChanged { at, .. }
            | BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::NoActiveGeneration,
            }) => {
                target = resolve_unified_catalog_target(live, at.bank, at.pc, mem)?;
            }
            BlockExit::Fault(fault) if !was_dynamic && dynamic_fallback_eligible(fault) => {
                target = UnifiedCatalogTargetV1::Dynamic {
                    source_bank: fault.at.bank,
                    target_pc: fault.at.pc,
                };
            }
            exit => {
                return Ok(fn64_recomp_rs::DispatchRun {
                    exit,
                    instructions,
                    blocks,
                });
            }
        }
    }
}

#[cfg(feature = "dynamic-mapped-runtime")]
fn run_catalog_block_program_dynamic(
    live: &CanonicalLiveBlockProgramV1,
    mut target: UnifiedCatalogTargetV1,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    loop {
        live.reconcile_before_dispatch(mem);
        let current = target.key();
        let (count, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, current.pc) {
            target = resolve_unified_catalog_target(live, current.bank, vector, mem)
                .unwrap_or_else(|error| {
                    recompiled_gap_panic(format!(
                        "unified catalog interrupt vector {vector} from {current} does not resolve: {error}"
                    ))
                });
        }

        let dispatched =
            dispatch_unified_catalog_slice(live, target, live.next_dispatch_budget(), ctx, mem)
                .unwrap_or_else(|error| recompiled_gap_panic(error));
        live.invalidate_pending_physical_writes(mem);

        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            live.charge_canonical_instructions(dispatched.instructions);
            live.publish_checkpoint(dispatched.instructions, dispatched.exit, None, ctx);
            super::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
        }

        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "unified catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                target = resolve_unified_catalog_target(live, next.bank, next.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::HostCall { vram, resume } => {
                let host = live.install.resolve_host(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified catalog produced host target {:#010x} absent from its owned inventory",
                        vram.get()
                    ))
                });
                invoke_catalog_block_host(live, vram, resume, host, ctx, mem);
                target = resolve_unified_catalog_target(live, resume.bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ThreadReturn => {
                live.publish_returned(ctx);
                return;
            }
            BlockExit::Fault(fault) => {
                if park_host_scheduled_exception(Some(live), fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified catalog stopped on non-architectural guest fault after {} instructions: {fault:?}",
                        dispatched.instructions
                    ))
                });
                assert!(
                    dispatched.instructions > 0,
                    "unified catalog architectural fault made no guest progress: {fault:?}"
                );
                target = resolve_unified_catalog_target(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteFault(fault) => {
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "unified executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                target = resolve_unified_catalog_target(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                target = resolve_unified_catalog_target(live, source_bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolve_unified_catalog_call(live, source_bank, target_pc, mem)
                .unwrap_or_else(|error| recompiled_gap_panic(error))
            {
                UnifiedCatalogCallV1::Host => {
                    let host = live
                        .install
                        .resolve_host(target_pc.get())
                        .unwrap_or_else(|| {
                            recompiled_gap_panic(format!(
                                "unified executable-write call lost host target {target_pc}"
                            ))
                        });
                    invoke_catalog_block_host(live, target_pc, resume, host, ctx, mem);
                    target = resolve_unified_catalog_target(live, source_bank, resume.pc, mem)
                        .unwrap_or_else(|error| recompiled_gap_panic(error));
                }
                UnifiedCatalogCallV1::Guest(next) => target = next,
            },
            BlockExit::ImageChanged { at, .. } => {
                target = resolve_unified_catalog_target(live, at.bank, at.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("unified catalog slice returned an internal transfer boundary")
            }
        }
    }
}

fn run_catalog_block_program(
    live: &CanonicalLiveBlockProgramV1,
    mut entry: ExecutionKey,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    if live.dynamic_execution_installed() {
        #[cfg(feature = "dynamic-mapped-runtime")]
        {
            run_catalog_block_program_dynamic(
                live,
                UnifiedCatalogTargetV1::Static(entry),
                ctx,
                mem,
            );
            return;
        }
        #[cfg(not(feature = "dynamic-mapped-runtime"))]
        unreachable!("dynamic execution cannot be installed without its feature");
    }
    live.reconcile_before_dispatch(mem);
    entry = resolve_catalog_transfer_with_activation(live, entry.bank, entry.pc, mem)
        .unwrap_or_else(|error| recompiled_gap_panic(error));
    loop {
        live.reconcile_before_dispatch(mem);
        let (count, compare, timer_pending) = with_executor(|executor| {
            (
                executor.cp0_count(),
                executor.cp0_compare(),
                executor.cp0_timer_pending(),
            )
        });
        ctx.synchronize_cop0_timing(count, compare);
        CpuInterruptLine::TIMER.set_level(ctx, timer_pending);
        CpuInterruptLine::RCP.set_level(ctx, crate::pi::cpu_interrupt_pending());
        if let Some(vector) = enter_pending_interrupt(ctx, entry.pc) {
            entry = resolve_catalog_transfer_with_activation(live, entry.bank, vector, mem)
                .unwrap_or_else(|fault| {
                    recompiled_gap_panic(format!(
                        "canonical catalog interrupt vector {vector} from {entry} does not resolve: {fault:?}"
                    ))
                });
        }

        let dispatched = live
            .dispatch_exposing_exceptions_at_budget(entry, live.next_dispatch_budget(), ctx, mem)
            .unwrap_or_else(|error| {
                recompiled_gap_panic(format!(
                    "canonical catalog dispatch failed at {entry}: {error}"
                ))
            });
        live.invalidate_pending_physical_writes(mem);

        let image_changed_entry = match dispatched.exit {
            BlockExit::ImageChanged { at, .. } => Some(
                live.activate_for_fetch(at.pc, mem)
                    .map_err(|error| {
                        format!("image-change activation at {} failed: {error}", at.pc)
                    })
                    .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let inactive_fault_entry = match dispatched.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::NoActiveGeneration,
            }) => Some(
                live.activate_for_fetch(at.pc, mem)
                    .map_err(|error| format!("fault activation at {} failed: {error}", at.pc))
                    .unwrap_or_else(|error| recompiled_gap_panic(error)),
            ),
            _ => None,
        };
        let prepared_continuation = match (image_changed_entry, inactive_fault_entry) {
            (Some(entry), None) => Some(CanonicalPreparedContinuationV1::ImageChanged { entry }),
            (None, Some(entry)) => {
                Some(CanonicalPreparedContinuationV1::InactiveGeneration { entry })
            }
            (None, None) => None,
            (Some(_), Some(_)) => {
                unreachable!("one catalog exit prepared two native continuations")
            }
        };

        let (count_write, compare_write) = ctx.take_cop0_timing_writes();
        if count_write.is_some() || compare_write.is_some() {
            with_executor(|executor| {
                if let Some(count) = count_write {
                    executor.set_cp0_count(count);
                }
                if let Some(compare) = compare_write {
                    executor.write_cp0_compare(compare);
                }
            });
        }
        if dispatched.instructions > 0 {
            live.charge_canonical_instructions(dispatched.instructions);
            live.publish_checkpoint(
                dispatched.instructions,
                dispatched.exit,
                prepared_continuation,
                ctx,
            );
            super::suspend_active_coroutine(fn64_runtime::Yield::InstructionCheckpoint {
                instructions: dispatched.instructions,
            });
        }

        match dispatched.exit {
            BlockExit::Checkpoint(next) | BlockExit::Yield(next) => {
                assert!(
                    dispatched.instructions > 0,
                    "canonical catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                entry = resolve_catalog_transfer_with_activation(live, next.bank, next.pc, mem)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog continuation {next} does not resolve: {fault:?}"
                        ))
                    });
            }
            BlockExit::HostCall { vram, resume } => {
                let host = live.install.resolve_host(vram.get()).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical catalog dispatch produced host target {:#010x} absent from its owned inventory",
                        vram.get()
                    ))
                });
                invoke_catalog_block_host(live, vram, resume, host, ctx, mem);
                entry = resolve_catalog_transfer_with_activation(live, resume.bank, resume.pc, mem)
                    .unwrap_or_else(|fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog host resume {resume} does not resolve: {fault:?}"
                        ))
                    });
            }
            BlockExit::ThreadReturn => {
                live.publish_returned(ctx);
                return;
            }
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            }) => {
                entry = inactive_fault_entry.expect("inactive fault activation was prepared");
            }
            BlockExit::Fault(fault) => {
                assert!(
                    dispatched.instructions > 0,
                    "canonical catalog returned {:?} without guest progress",
                    dispatched.exit
                );
                if park_host_scheduled_exception(Some(live), fault, ctx) {
                    unreachable!("parking a faulted host-scheduled thread does not return")
                }
                let fault_bank = fault.at.bank;
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical catalog stopped on non-architectural guest fault: {fault:?}"
                    ))
                });
                entry = resolve_catalog_transfer_with_activation(live, fault_bank, vector, mem)
                    .unwrap_or_else(|mapping_fault| {
                        recompiled_gap_panic(format!(
                            "canonical catalog exception vector {vector} does not resolve: {mapping_fault:?}"
                        ))
                    });
            }
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } if live.generations.is_some() => {
                entry = resolve_catalog_transfer_with_activation(live, source_bank, resume.pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } if live.generations.is_some() => {
                match resolve_catalog_call_with_activation(live, source_bank, target_pc, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error))
                {
                    CatalogCallResolutionV1::Guest(next) => entry = next,
                    CatalogCallResolutionV1::Host(host) => {
                        invoke_catalog_block_host(live, target_pc, resume, host, ctx, mem);
                        entry = resolve_catalog_transfer_with_activation(
                            live,
                            source_bank,
                            resume.pc,
                            mem,
                        )
                        .unwrap_or_else(|error| recompiled_gap_panic(error));
                    }
                }
            }
            BlockExit::ExecutableWriteFault(fault) if live.generations.is_some() => {
                let vector = fault.enter_exception(ctx).unwrap_or_else(|| {
                    recompiled_gap_panic(format!(
                        "canonical generation executable-write boundary retained a non-architectural fault: {fault:?}"
                    ))
                });
                entry = resolve_catalog_transfer_with_activation(live, fault.at.bank, vector, mem)
                    .unwrap_or_else(|error| recompiled_gap_panic(error));
            }
            BlockExit::ImageChanged { .. } if live.generations.is_some() => {
                entry = image_changed_entry.expect("image-change activation was prepared");
            }
            BlockExit::ExecutableWrite { .. }
            | BlockExit::ExecutableWriteResolveCall { .. }
            | BlockExit::ExecutableWriteFault(_)
            | BlockExit::ImageChanged { .. } => {
                recompiled_gap_panic(format!(
                    "canonical static catalog encountered an executable-image mutation boundary: {:?}",
                    dispatched.exit
                ));
            }
            BlockExit::Transfer(_)
            | BlockExit::ResolveTransfer { .. }
            | BlockExit::ResolveCall { .. } => {
                unreachable!("catalog dispatch returned an internal transfer boundary")
            }
        }
    }
}

/// Dispatch a newly-created OSThread through the installed typed module.
/// Returns `false` only for the legacy C configuration.
///
/// # Safety
/// `rdram` carries the same process-lifetime allocation contract as
/// `osCreateThread_recomp` and `recompiled::boot_thread0`.
pub(super) unsafe fn run_registered_entry(
    rdram: *mut u8,
    entry_vram: u32,
    arg: u64,
    sp: u64,
    initial_status: Option<u32>,
) -> bool {
    let (catalog, program, registered) = with_host(|host| {
        (
            host.canonical_recompiled_program.clone(),
            host.recompiled_program.clone(),
            host.recompiled_lookup
                .map(|lookup| (lookup, host.recompiled_rdram_len)),
        )
    });
    if let Some(catalog) = catalog {
        let rdram_len = with_host(|host| host.recompiled_rdram_len);
        assert!(
            rdram_len > 0,
            "canonical recompiled program has no RDRAM length"
        );
        // SAFETY: inherited from the caller's shared-allocation contract.
        let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let mut mem = Rdram::new(bytes);
        let entry_pc = GuestPc::new(entry_vram);
        #[cfg(feature = "dynamic-mapped-runtime")]
        if catalog.dynamic_execution_installed() {
            let target =
                resolve_unified_catalog_entry(&catalog, entry_pc, &mem).unwrap_or_else(|error| {
                    panic!(
                    "spawned canonical OSThread entry {entry_vram:#010x} is not executable: {error}"
                )
                });
            let mut ctx = new_osthread_context(initial_status);
            ctx.set_r(4, arg);
            ctx.set_r(29, sp);
            ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
            ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
            run_catalog_block_program_dynamic(&catalog, target, &mut ctx, &mut mem);
            return true;
        }
        let entry = resolve_catalog_entry_with_activation(&catalog, entry_pc, &mem).unwrap_or_else(
            |error| {
                panic!(
                    "spawned canonical OSThread entry {entry_vram:#010x} is not executable: {error}"
                )
            },
        );
        let mut ctx = new_osthread_context(initial_status);
        ctx.set_r(4, arg);
        ctx.set_r(29, sp);
        ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
        ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
        run_catalog_block_program(&catalog, entry, &mut ctx, &mut mem);
        return true;
    }
    if let Some(program) = program {
        let entry = program
            .resolve_entry(GuestPc::new(entry_vram))
            .unwrap_or_else(|fault| {
                panic!("spawned OSThread entry {entry_vram:#010x} is not executable: {fault:?}")
            });
        let rdram_len = with_host(|host| host.recompiled_rdram_len);
        assert!(
            rdram_len > 0,
            "recompiled block program has no RDRAM length"
        );
        // SAFETY: inherited from the caller's shared-allocation contract.
        let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let mut mem = Rdram::new(bytes);
        let mut ctx = new_osthread_context(initial_status);
        ctx.set_r(4, arg);
        ctx.set_r(29, sp);
        ctx.set_r32(31, THREAD_RETURN_SENTINEL as i32);
        ctx.set_thread_return_pc(Some(THREAD_RETURN_SENTINEL));
        run_block_program(&program, entry, &mut ctx, &mut mem);
        return true;
    }
    let Some((lookup, rdram_len)) = registered else {
        return false;
    };
    assert!(rdram_len > 0, "recompiled entry lookup has no RDRAM length");
    // SAFETY: inherited from the caller's shared-allocation contract.
    let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut mem = Rdram::new(bytes);
    let mut ctx = new_osthread_context(initial_status);
    ctx.set_r(4, arg);
    ctx.set_r(29, sp);
    lookup(entry_vram)(&mut ctx, &mut mem);
    true
}

fn c_fpr_image_from_physical(state: PhysicalFgrState, fr: bool) -> [u64; 32] {
    let physical = state.into_words();
    if fr {
        return physical;
    }

    // Valid generated FR=0 operations consume each even slot as one active
    // paired FPR. Direct odd double/64-bit operations are invalid and remain
    // loud, so the unreachable odd slots can carry both corresponding latent
    // upper words, making this a reversible 2048-bit permutation.
    let mut packed = [0u64; 32];
    for pair in 0..16 {
        let even = pair * 2;
        let odd = even + 1;
        packed[even] = u64::from(physical[even] as u32) | (u64::from(physical[odd] as u32) << 32);
        packed[odd] = (physical[even] >> 32) | (physical[odd] & 0xFFFF_FFFF_0000_0000);
    }
    packed
}

fn physical_from_c_fpr_image(packed: [u64; 32], fr: bool) -> PhysicalFgrState {
    if fr {
        return PhysicalFgrState::from_words(packed);
    }

    let mut physical = [0u64; 32];
    for pair in 0..16 {
        let even = pair * 2;
        let odd = even + 1;
        physical[even] = u64::from(packed[even] as u32) | ((packed[odd] as u32 as u64) << 32);
        physical[odd] = (packed[even] >> 32) | (packed[odd] & 0xFFFF_FFFF_0000_0000);
    }
    PhysicalFgrState::from_words(physical)
}

fn c_from_recompiled(ctx: &RsContext) -> CContext {
    let r = ctx.gprs();
    let mut c = CContext::zeroed();
    c.r0 = r[0];
    c.r1 = r[1];
    c.r2 = r[2];
    c.r3 = r[3];
    c.r4 = r[4];
    c.r5 = r[5];
    c.r6 = r[6];
    c.r7 = r[7];
    c.r8 = r[8];
    c.r9 = r[9];
    c.r10 = r[10];
    c.r11 = r[11];
    c.r12 = r[12];
    c.r13 = r[13];
    c.r14 = r[14];
    c.r15 = r[15];
    c.r16 = r[16];
    c.r17 = r[17];
    c.r18 = r[18];
    c.r19 = r[19];
    c.r20 = r[20];
    c.r21 = r[21];
    c.r22 = r[22];
    c.r23 = r[23];
    c.r24 = r[24];
    c.r25 = r[25];
    c.r26 = r[26];
    c.r27 = r[27];
    c.r28 = r[28];
    c.r29 = r[29];
    c.r30 = r[30];
    c.r31 = r[31];
    c.hi = ctx.hi;
    c.lo = ctx.lo;
    c.status_reg = ctx.cop0_status;
    c.mips3_float_mode = u8::from(ctx.cop0_status & STATUS_FR != 0);
    c.set_fpr_u64_bits(c_fpr_image_from_physical(
        ctx.physical_fgr_state(),
        c.mips3_float_mode == 1,
    ));
    c.assert_float_mode_matches_status();
    c
}

fn copy_c_back(c: &CContext, ctx: &mut RsContext) {
    c.assert_float_mode_matches_status();
    ctx.set_gprs([
        c.r0, c.r1, c.r2, c.r3, c.r4, c.r5, c.r6, c.r7, c.r8, c.r9, c.r10, c.r11, c.r12, c.r13,
        c.r14, c.r15, c.r16, c.r17, c.r18, c.r19, c.r20, c.r21, c.r22, c.r23, c.r24, c.r25, c.r26,
        c.r27, c.r28, c.r29, c.r30, c.r31,
    ]);
    ctx.hi = c.hi;
    ctx.lo = c.lo;
    ctx.replace_physical_fgr_state(physical_from_c_fpr_image(
        c.fpr_u64_bits(),
        c.mips3_float_mode == 1,
    ));
    ctx.cop0_status = c.status_reg;
}

#[cfg(test)]
fn is_test_c_shim(shim: CShim) -> bool {
    [
        tests::no_op_fpr_shim as CShim,
        tests::write_f5_word_shim as CShim,
        tests::change_fr_shim as CShim,
        tests::change_bev_shim as CShim,
    ]
    .into_iter()
    .any(|allowed| std::ptr::fn_addr_eq(allowed, shim))
}

fn is_admitted_fr_stable_c_shim(shim: CShim) -> bool {
    is_generated_adapter_c_shim(shim)
        || [
            super::__osInitialize_common_recomp as CShim,
            super::osInitialize_recomp as CShim,
            super::__osInitialize_msp_recomp as CShim,
            super::__osInitialize_kmc_recomp as CShim,
            super::__osInitialize_isv_recomp as CShim,
        ]
        .into_iter()
        .any(|allowed| std::ptr::fn_addr_eq(allowed, shim))
        || cfg!(test) && {
            #[cfg(test)]
            {
                is_test_c_shim(shim)
            }
            #[cfg(not(test))]
            {
                false
            }
        }
}

fn call_c(ctx: &mut RsContext, mem: &mut Rdram<'_>, name: &'static str, shim: CShim) {
    // An exit snapshot cannot observe a shim which changes FR, accesses the
    // other FPR view, then restores FR. Admit only the closed host-shim set
    // whose implementations preserve FR for the entire call.
    assert!(
        is_admitted_fr_stable_c_shim(shim),
        "C shim {name} is not in the FR-stable adapter registry"
    );
    if std::env::var_os("FN64_RECOMP_RS_SHIM_TRACE").is_some() {
        eprintln!("[fn64-recomp-rs-shim] {name}");
    }
    let mut c = c_from_recompiled(ctx);
    // `f_odd` aliases this stack-local context, so arm it only after the C
    // image has reached its stable address.
    c.arm_fpr_alias();
    let entry_fr = c.mips3_float_mode;
    let entry_bev = c.status_reg & STATUS_BEV;
    let rdram = mem.as_mut_slice().as_mut_ptr();
    // SAFETY: `rdram` comes from the live checked Rdram view and `c` is the
    // exact `#[repr(C)]` context the existing ABI shim requires. The shim may
    // suspend/resume this same coroutine, but neither pointer changes while
    // the adapter's stack frame remains live.
    unsafe { shim(rdram, &mut c) };
    c.assert_float_mode_matches_status();
    assert_eq!(
        c.mips3_float_mode, entry_fr,
        "C shim {name} changed Status.FR across the adapter; its packed FPR image and f_odd alias still describe the entry view"
    );
    assert_eq!(
        c.status_reg & STATUS_BEV,
        entry_bev,
        "C shim {name} changed Status.BEV across the adapter; bootstrap-vector reachability requires a typed Status-replacement boundary"
    );
    copy_c_back(&c, ctx);
}

/// Construct the architectural context installed by public `osCreateThread`.
/// The libultra `osCreateThread` manual's DESCRIPTION section specifies that
/// every new thread starts with denormal-result flushing and Invalid exceptions
/// enabled. Keeping this in the context makes coroutine suspension itself the
/// FCSR save/restore boundary.
fn new_osthread_context(initial_status: Option<u32>) -> RsContext {
    let mut ctx = RsContext::new();
    ctx.initialize_invalid_tlb_entries();
    if let Some(status) = initial_status {
        // A libultra-created OSThread starts in the FR=0 paired-register view;
        // it does not inherit the reset thread's FR=1 view. The generated NWXE
        // osCreateThread body makes that constraint concrete by initializing
        // its saved SR to 0x0000_ff03. The host scheduler eagerly retains the
        // caller's other modeled Status fields, but must close this view
        // transition before the new coroutine can execute paired doubles.
        ctx.cop0_status = status & !STATUS_FR;
    }
    ctx.write_fcr(31, INITIAL_FPCSR);
    ctx
}

fn initialize_typed_fpcsr(
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
    name: &'static str,
    shim: CShim,
) {
    call_c(ctx, mem, name, shim);
    ctx.write_fcr(31, INITIAL_FPCSR);
}

pub fn os_initialize_common(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_common_recomp",
        super::__osInitialize_common_recomp,
    );
}

pub fn os_initialize(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(ctx, mem, "osInitialize_recomp", super::osInitialize_recomp);
}

pub fn os_initialize_msp(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_msp_recomp",
        super::__osInitialize_msp_recomp,
    );
}

pub fn os_initialize_kmc(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_kmc_recomp",
        super::__osInitialize_kmc_recomp,
    );
}

pub fn os_initialize_isv(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    initialize_typed_fpcsr(
        ctx,
        mem,
        "__osInitialize_isv_recomp",
        super::__osInitialize_isv_recomp,
    );
}

/// Typed `__osSetFpcCsr`: use the same per-OSThread FCSR authority as emitted
/// CFC1/CTC1. A write which requests an exception stays loud because a host
/// call cannot return the arbitrary-PC lane's typed guest transfer.
pub fn os_set_fpc_csr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    let previous = ctx.read_fcr(31);
    ctx.write_fcr(31, ctx.r_u32(4));
    ctx.set_r32(2, previous as i32);
    if ctx.fcsr_exception_pending() {
        fn64_recomp_rs::trap_unsupported(
            "__osSetFpcCsr wrote an enabled FCSR cause through a host-call boundary",
        );
    }
}

macro_rules! c_adapters {
    ($(($recompiled:ident, $shim:ident)),+ $(,)?) => {
        fn is_generated_adapter_c_shim(shim: CShim) -> bool {
            std::ptr::fn_addr_eq(shim, super::osCreateThread_recomp as CShim)
                $(|| std::ptr::fn_addr_eq(shim, super::$shim as CShim))+
        }

        $(
            pub fn $recompiled(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
                call_c(ctx, mem, stringify!($shim), super::$shim);
            }
        )+
    };
}

thread_local! {
    static PENDING_OSTHREAD_STATUS: std::cell::Cell<Option<u32>> = const {
        std::cell::Cell::new(None)
    };
}

pub(super) fn take_pending_osthread_status() -> Option<u32> {
    PENDING_OSTHREAD_STATUS.with(std::cell::Cell::take)
}

pub fn os_create_thread(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
    PENDING_OSTHREAD_STATUS.with(|pending| {
        assert!(
            pending.replace(Some(ctx.cop0_status)).is_none(),
            "os_create_thread: nested typed OSThread status publication"
        );
    });
    call_c(
        ctx,
        mem,
        "osCreateThread_recomp",
        super::osCreateThread_recomp,
    );
    assert!(
        take_pending_osthread_status().is_none(),
        "os_create_thread: C shim did not consume the typed OSThread status"
    );
}

c_adapters!(
    (is_prout_sync_printf, is_proutSyncPrintf_recomp),
    (check_hardware_msp, __checkHardware_msp_recomp),
    (check_hardware_kmc, __checkHardware_kmc_recomp),
    (check_hardware_isv, __checkHardware_isv_recomp),
    (os_rdb_send, __osRdbSend_recomp),
    (os_start_thread, osStartThread_recomp),
    (os_set_thread_pri, osSetThreadPri_recomp),
    (os_get_thread_pri, osGetThreadPri_recomp),
    (os_create_mesg_queue, osCreateMesgQueue_recomp),
    (os_send_mesg, osSendMesg_recomp),
    (os_recv_mesg, osRecvMesg_recomp),
    (os_set_event_mesg, osSetEventMesg_recomp),
    (os_set_timer, osSetTimer_recomp),
    (os_cart_rom_init, osCartRomInit_recomp),
    (os_pi_read_io, osPiReadIo_recomp),
    (os_pi_start_dma, osPiStartDma_recomp),
    (os_pi_get_status, osPiGetStatus_recomp),
    (os_epi_start_dma, osEPiStartDma_recomp),
    (os_epi_raw_start_dma, osEPiRawStartDma_recomp),
    (os_eeprom_probe, osEepromProbe_recomp),
    (os_eeprom_read, osEepromRead_recomp),
    (os_eeprom_write, osEepromWrite_recomp),
    (os_eeprom_long_read, osEepromLongRead_recomp),
    (os_eeprom_long_write, osEepromLongWrite_recomp),
    (os_pfs_is_plug, osPfsIsPlug_recomp),
    (os_pfs_init_pak, osPfsInitPak_recomp),
    (os_pfs_free_blocks, osPfsFreeBlocks_recomp),
    (os_pfs_allocate_file, osPfsAllocateFile_recomp),
    (os_pfs_delete_file, osPfsDeleteFile_recomp),
    (os_pfs_file_state, osPfsFileState_recomp),
    (os_pfs_find_file, osPfsFindFile_recomp),
    (os_pfs_read_write_file, osPfsReadWriteFile_recomp),
    (os_flash_init, osFlashInit_recomp),
    (os_flash_read_status, osFlashReadStatus_recomp),
    (os_flash_read_id, osFlashReadId_recomp),
    (os_flash_clear_status, osFlashClearStatus_recomp),
    (os_flash_all_erase, osFlashAllErase_recomp),
    (os_flash_all_erase_through, osFlashAllEraseThrough_recomp),
    (os_flash_sector_erase, osFlashSectorErase_recomp),
    (
        os_flash_sector_erase_through,
        osFlashSectorEraseThrough_recomp
    ),
    (os_flash_check_erase_end, osFlashCheckEraseEnd_recomp),
    (os_flash_write_buffer, osFlashWriteBuffer_recomp),
    (os_flash_write_array, osFlashWriteArray_recomp),
    (os_flash_read_array, osFlashReadArray_recomp),
    (os_flash_change, osFlashChange_recomp),
    (os_virtual_to_physical, osVirtualToPhysical_recomp),
    (os_create_pi_manager, osCreatePiManager_recomp),
    (os_si_device_busy, __osSiDeviceBusy_recomp),
    (os_si_raw_start_dma, __osSiRawStartDma_recomp),
    (os_ai_set_frequency, osAiSetFrequency_recomp),
    (os_ai_get_length, osAiGetLength_recomp),
    (os_ai_set_next_buffer, osAiSetNextBuffer_recomp),
    (os_get_mem_size, osGetMemSize_recomp),
    (os_inval_dcache, osInvalDCache_recomp),
    (os_inval_icache, osInvalICache_recomp),
    (os_writeback_dcache, osWritebackDCache_recomp),
    (os_disable_int, __osDisableInt_recomp),
    (os_restore_int, __osRestoreInt_recomp),
    (os_get_thread_id, osGetThreadId_recomp),
    (os_get_time, osGetTime_recomp),
    (os_set_count, osSetCount_recomp),
    (os_sp_task_yielded, osSpTaskYielded_recomp),
    (os_create_vi_manager, osCreateViManager_recomp),
    (os_vi_set_event, osViSetEvent_recomp),
    (os_vi_set_mode, osViSetMode_recomp),
    (os_vi_set_special_features, osViSetSpecialFeatures_recomp),
    (os_vi_set_x_scale, osViSetXScale_recomp),
    (os_vi_set_y_scale, osViSetYScale_recomp),
    (os_vi_swap_buffer, osViSwapBuffer_recomp),
    (os_vi_black, osViBlack_recomp),
    (os_vi_fade, osViFade_recomp),
    (os_vi_repeat_line, osViRepeatLine_recomp),
    (ll_div, __ll_div_recomp),
    (ll_mul, __ll_mul_recomp),
    (ull_div, __ull_div_recomp),
    (ull_rem, __ull_rem_recomp),
    (ull_to_d, __ull_to_d_recomp),
    (ull_to_f, __ull_to_f_recomp),
    (os_pi_get_access, __osPiGetAccess_recomp),
    (os_pi_rel_access, __osPiRelAccess_recomp),
    (os_sp_set_pc, __osSpSetPc_recomp),
    (os_sp_set_status, __osSpSetStatus_recomp),
    (os_cont_get_query, osContGetQuery_recomp),
    (os_cont_get_read_data, osContGetReadData_recomp),
    (os_cont_init, osContInit_recomp),
    (os_cont_set_ch, osContSetCh_recomp),
    (os_cont_start_query, osContStartQuery_recomp),
    (os_cont_start_read_data, osContStartReadData_recomp),
    (os_motor_init, osMotorInit_recomp),
    (os_motor_access, __osMotorAccess_recomp),
    (os_motor_start, osMotorStart_recomp),
    (os_motor_stop, osMotorStop_recomp),
    (os_voice_set_word, osVoiceSetWord_recomp),
    (os_voice_check_word, osVoiceCheckWord_recomp),
    (os_voice_stop_read_data, osVoiceStopReadData_recomp),
    (os_voice_init, osVoiceInit_recomp),
    (os_voice_mask_dictionary, osVoiceMaskDictionary_recomp),
    (os_voice_start_read_data, osVoiceStartReadData_recomp),
    (os_voice_control_gain, osVoiceControlGain_recomp),
    (os_voice_get_read_data, osVoiceGetReadData_recomp),
    (os_voice_clear_dictionary, osVoiceClearDictionary_recomp),
    (os_destroy_thread, osDestroyThread_recomp),
    (os_stop_thread, osStopThread_recomp),
    (os_dp_set_status, osDpSetStatus_recomp),
    (os_dp_set_next_buffer, osDpSetNextBuffer_recomp),
    (os_epi_read_io, osEPiReadIo_recomp),
    (os_epi_write_io, osEPiWriteIo_recomp),
    (os_get_count, osGetCount_recomp),
    (os_jam_mesg, osJamMesg_recomp),
    (os_sp_task_load, osSpTaskLoad_recomp),
    (os_sp_task_start_go, osSpTaskStartGo_recomp),
    (os_sp_task_yield, osSpTaskYield_recomp),
    (os_stop_timer, osStopTimer_recomp),
    (
        os_vi_get_current_framebuffer,
        osViGetCurrentFramebuffer_recomp
    ),
    (os_vi_get_next_framebuffer, osViGetNextFramebuffer_recomp),
    (os_writeback_dcache_all, osWritebackDCacheAll_recomp),
    (os_sp_get_status, __osSpGetStatus_recomp),
    (os_dp_get_status, osDpGetStatus_recomp),
);

fn abi_host_shim_callable(shim: AbiHostShimV1) -> RecompFunc {
    match shim {
        AbiHostShimV1::OsCreateMesgQueue => os_create_mesg_queue,
        AbiHostShimV1::OsCreateThread => os_create_thread,
        AbiHostShimV1::OsEPiStartDma => os_epi_start_dma,
        AbiHostShimV1::OsGetThreadPri => os_get_thread_pri,
        AbiHostShimV1::OsRecvMesg => os_recv_mesg,
        AbiHostShimV1::OsSendMesg => os_send_mesg,
        AbiHostShimV1::OsSetEventMesg => os_set_event_mesg,
        AbiHostShimV1::OsSiDeviceBusy => os_si_device_busy,
        AbiHostShimV1::OsSetThreadPri => os_set_thread_pri,
        AbiHostShimV1::OsSetTimer => os_set_timer,
        AbiHostShimV1::OsSpTaskLoad => os_sp_task_load,
        AbiHostShimV1::OsSpTaskStartGo => os_sp_task_start_go,
        AbiHostShimV1::OsSpTaskYield => os_sp_task_yield,
        AbiHostShimV1::OsSpTaskYielded => os_sp_task_yielded,
        AbiHostShimV1::OsStartThread => os_start_thread,
    }
}

fn abi_host_shim_writer_effects(shim: AbiHostShimV1) -> Vec<WriterChannel> {
    // These are conservative synchronous/nested effects of invoking the shim,
    // not claims about all later guest execution. Every adapter may mutate
    // guest memory through its HostAbi parent transaction. Queue send/receive
    // and thread start can suspend while another guest thread and every device
    // child advance; task start can execute RSP and RDP children synchronously.
    let all_live_channels = || {
        vec![
            WriterChannel::CpuInstructionStore,
            WriterChannel::PiDma,
            WriterChannel::SiDma,
            WriterChannel::SpDma,
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::RdpRenderer,
            WriterChannel::HostAbi,
        ]
    };
    match shim {
        AbiHostShimV1::OsEPiStartDma => {
            vec![WriterChannel::PiDma, WriterChannel::HostAbi]
        }
        AbiHostShimV1::OsRecvMesg | AbiHostShimV1::OsSendMesg | AbiHostShimV1::OsStartThread => {
            all_live_channels()
        }
        AbiHostShimV1::OsSpTaskStartGo => vec![
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::RdpRenderer,
            WriterChannel::HostAbi,
        ],
        AbiHostShimV1::OsSpTaskYield => vec![
            WriterChannel::RspExecutionOrHleWriteback,
            WriterChannel::HostAbi,
        ],
        AbiHostShimV1::OsCreateMesgQueue
        | AbiHostShimV1::OsCreateThread
        | AbiHostShimV1::OsGetThreadPri
        | AbiHostShimV1::OsSetEventMesg
        | AbiHostShimV1::OsSiDeviceBusy
        | AbiHostShimV1::OsSetThreadPri
        | AbiHostShimV1::OsSetTimer
        | AbiHostShimV1::OsSpTaskLoad
        | AbiHostShimV1::OsSpTaskYielded => vec![WriterChannel::HostAbi],
    }
}

/// `__osGetSR`: read this OSThread's typed COP0 Status register.
pub fn os_get_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_status as u64);
}

/// `__osSetSR`: replace this OSThread's typed COP0 Status register.
pub fn os_set_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.cop0_status = ctx.r_u32(4);
}

/// `__osGetCause`: the executor does not synthesize CPU exception frames, so
/// this reads the explicit typed Cause state (normally zero).
pub fn os_get_cause(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_cause as u64);
}

/// `osSetIntMask`: update this typed OSThread's packed mask and the shared MI
/// gate. Unlike the legacy C adapter, the prior value is owned by `ctx`, so
/// coroutine switches cannot make one thread return another thread's mask.
pub fn os_set_int_mask(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    const CPU_INTERRUPT_FIELDS: u32 = 1 | (0xFF << 8);
    let new_mask = ctx.r_u32(4);
    let previous = ctx.replace_os_interrupt_mask(new_mask);
    ctx.cop0_status = (ctx.cop0_status & !CPU_INTERRUPT_FIELDS) | (new_mask & CPU_INTERRUPT_FIELDS);
    crate::pi::set_mi_interrupt_mask((new_mask >> 16) & 0x3F);
    ctx.set_r(2, previous as u64);
}

/// `osGetIntMask`: return this typed OSThread's combined CPU/RCP mask.
pub fn os_get_int_mask(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.os_interrupt_mask() as u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_recomp_rs::{
        run_bank, BackedExecutableSpanV1, BlockRun, BootCicIdentity, BootCop0Context, BootRegion,
        CodeBank, CodeCatalog, CodeSpan, CpuFaultKind, GeneratedBankRunner, GenerationId,
        PhysicalCodeBank, PrecompiledGeneration, PrecompiledGenerationBackingV1, PrecompiledShard,
        Sha256Digest, BOOT_CONTEXT_SCHEMA_V1,
    };
    use sha2::Digest;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static TRANSIENT_FR_SHIM_ENTERED: AtomicBool = AtomicBool::new(false);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_SOURCE_RUNS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_HOST_RUNS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(feature = "dynamic-mapped-runtime")]
    static DYNAMIC_BOOT_RESUME_RUNS: AtomicUsize = AtomicUsize::new(0);

    const INSTALL_BANK: BankId = BankId::new(0xb007);
    const INSTALL_PC: GuestPc = GuestPc::new(0x8000_7000);

    fn install_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_transition_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        if entry.pc == INSTALL_PC {
            ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
            return BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: entry.bank,
                    target_pc: dynamic_pc,
                },
                1,
            );
        }
        assert_eq!(entry.pc, static_resume);
        ctx.set_r32(3, ctx.r_u32(3).wrapping_add(1) as i32);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn exact_withhold_normal_budget_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            INSTALL_PC => panic!("withheld canonical entry executed statically"),
            pc if pc == GuestPc::new(INSTALL_PC.get() + 4) => {
                assert_eq!(
                    budget.get(),
                    7,
                    "one-shot withholding kept static slicing armed"
                );
                BlockRun::new(BlockExit::Yield(entry), 1)
            }
            pc => panic!("unexpected exact-withhold test PC {pc}"),
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_host_precedence_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
        BlockRun::new(
            BlockExit::ResolveCall {
                source_bank: entry.bank,
                target_pc: GuestPc::new(INSTALL_PC.get() + 0x10),
                resume: ExecutionKey::new(entry.bank, GuestPc::new(INSTALL_PC.get() + 0x20)),
            },
            1,
        )
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_tlb_fault_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, ctx.r_u32(2).wrapping_add(1) as i32);
        BlockRun::new(
            BlockExit::ResolveTransfer {
                source_bank: entry.bank,
                target_pc: GuestPc::new(0x0040_0000),
            },
            1,
        )
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn unified_dynamic_writer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x40);
        if entry.pc == INSTALL_PC {
            return BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: entry.bank,
                    target_pc: dynamic_pc,
                },
                1,
            );
        }
        assert_eq!(entry.pc, static_resume);
        ctx.set_r32(3, ctx.r_u32(3).wrapping_add(1) as i32);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn put_physical_word(storage: &mut [u8], physical: u32, word: u32) {
        for (offset, byte) in word.to_be_bytes().into_iter().enumerate() {
            storage[(physical as usize + offset) ^ 3] = byte;
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn dynamic_boot_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        match entry.pc {
            INSTALL_PC => {
                DYNAMIC_BOOT_SOURCE_RUNS.fetch_add(1, Ordering::SeqCst);
                BlockRun::new(
                    BlockExit::ResolveTransfer {
                        source_bank: entry.bank,
                        target_pc: dynamic_pc,
                    },
                    1,
                )
            }
            pc if pc == resume => {
                DYNAMIC_BOOT_RESUME_RUNS.fetch_add(1, Ordering::SeqCst);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            pc => panic!("unexpected dynamic boot test PC {pc}"),
        }
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    fn dynamic_boot_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        DYNAMIC_BOOT_HOST_RUNS.fetch_add(1, Ordering::SeqCst);
        mem.as_mut_slice()[0x7100 ^ 3] = 1;
        super::super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
        assert_eq!(
            mem.as_mut_slice()[0x7100 ^ 3],
            2,
            "external write was not visible before dynamic host resume"
        );
        mem.as_mut_slice()[0x7100 ^ 3] = 3;
    }

    fn bootstrap_return_runner(
        _entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        BlockRun::new(BlockExit::ThreadReturn, 1)
    }

    fn install_test_host(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn alternate_install_test_host(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn install_test_legacy_host_lookup(target: u32) -> Option<RecompFunc> {
        (target == INSTALL_PC.get() + 4).then_some(alternate_install_test_host)
    }

    fn install_test_function_lookup(_target: u32) -> RecompFunc {
        install_test_host
    }

    fn install_test_entry_lookup(target: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Ok(ExecutionKey::new(BankId::new(0xcaff), target))
    }

    fn install_test_transfer_lookup(
        source_bank: BankId,
        target: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        Ok(ExecutionKey::new(source_bank, target))
    }

    fn install_test_program(bank: BankId, artifact_byte: u8) -> CatalogBlockProgramV1 {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([artifact_byte; 32]),
                ),
            )
            .unwrap();
        CatalogBlockProgramV1::new(
            program,
            ExecutionKey::new(bank, INSTALL_PC),
            InstructionBudget::new(2).unwrap(),
        )
        .unwrap()
    }

    fn bootstrap_test_install(expected_word: u32) -> CatalogGenerationInstallV1 {
        let bank = INSTALL_BANK;
        let entry = GuestPc::new(0x8000_7000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![expected_word]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0xb0; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_install_with_additional_banks(
        entry_word: u32,
        static_word: u32,
        physical_word: u32,
    ) -> CatalogGenerationInstallV1 {
        let entry_bank = BankId::new(0xb007);
        let static_bank = BankId::new(0xb008);
        let physical_bank = BankId::new(0xb009);
        let entry = GuestPc::new(0x8000_7000);
        let static_pc = GuestPc::new(0x8000_8000);
        let mut program = BlockProgram::new();
        for (bank, pc, word, artifact_byte) in [
            (entry_bank, entry, entry_word, 0xb0),
            (static_bank, static_pc, static_word, 0xb2),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        bootstrap_return_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        program
            .register_physical_code(
                PhysicalCodeBank::new(physical_bank, 0x9000, vec![physical_word]).unwrap(),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(entry_bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_install_with_generation(
        entry_word: u32,
        generation_word: u32,
    ) -> CatalogGenerationInstallV1 {
        let entry_bank = BankId::new(0xb007);
        let generation_bank = BankId::new(0xb00a);
        let entry = GuestPc::new(0x8000_7000);
        let generation_start = GuestPc::new(0x8000_a000);
        let generation_end = GuestPc::new(0x8000_a004);
        let mut program = BlockProgram::new();
        for (bank, pc, word, artifact_byte) in [
            (entry_bank, entry, entry_word, 0xb0),
            (generation_bank, generation_start, generation_word, 0xba),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        bootstrap_return_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(entry_bank, entry),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xb1; 32]),
        );
        let generation_id = GenerationId::new(0xaaa);
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    generation_id,
                    generation_start,
                    generation_end,
                    generation_start,
                    generation_end,
                    sha2::Sha256::digest(generation_word.to_be_bytes()).into(),
                    vec![
                        PrecompiledShard::new(generation_bank, generation_start, generation_end)
                            .unwrap(),
                    ],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![PrecompiledGenerationBackingV1::new(
                generation_id,
                vec![BackedExecutableSpanV1::new(generation_start, 0xa000, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        CatalogGenerationInstallV1::new(resolver, generations).unwrap()
    }

    fn bootstrap_test_rdram_len() -> usize {
        fn64_recomp_rs::RDRAM_LEN
    }

    #[test]
    fn bootstrap_import_commit_binds_rom_catalog_entry_and_static_watched_bytes() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let evidence = validated.receipt().evidence();
        let expected_rom_sha256: [u8; 32] = sha2::Sha256::digest(&rom).into();

        assert_eq!(validated.len(), bootstrap_test_rdram_len());
        assert_eq!(evidence.rom_sha256, expected_rom_sha256);
        assert_eq!(
            evidence.initial_entry,
            ExecutionKey::new(BankId::new(0xb007), GuestPc::new(0x8000_7000))
        );
        assert_eq!(
            evidence.watched_ranges,
            [PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x7000,
                physical_end: 0x7004,
            }]
        );
        assert_eq!(evidence.publications.len(), 1);
        assert_ne!(evidence.watched_sha256, [0; 32]);
        assert_ne!(evidence.receipt_sha256, [0; 32]);
    }

    #[test]
    fn bootstrap_writer_channel_receipt_is_minted_from_exact_private_journal_state() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let receipt = validate_bootstrap_writer_completion_state(
            canonical_writer_program_model_sha256(
                &install.resolver,
                Some(&install.generations),
                &validated.receipt().evidence().watched_ranges,
            ),
            validated.receipt().evidence(),
            &validated.storage,
            &state,
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(
            evidence.schema,
            BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1
        );
        assert!(receipt.has_valid_evidence_hash());
        assert_ne!(receipt.program_model_sha256(), [0; 32]);
        assert_eq!(evidence.journal_entry.sequence, 0);
        assert!(evidence
            .journal_entry
            .declared_writes
            .iter()
            .all(|write| write.channel == WriterChannel::BootstrapOrImport));
        assert_eq!(
            evidence.journal_entry.after_sha256,
            evidence.final_watched_sha256
        );
        assert_eq!(
            evidence.bootstrap_receipt_sha256,
            validated.receipt().evidence().receipt_sha256
        );
        let foreign = bootstrap_test_install(0x2402_0002);
        assert_ne!(
            receipt.program_model_sha256(),
            canonical_writer_program_model_sha256(
                &foreign.resolver,
                Some(&foreign.generations),
                &validated.receipt().evidence().watched_ranges,
            ),
            "writer model identity must bind the canonical BlockProgram image"
        );
    }

    #[test]
    fn bootstrap_writer_channel_validator_rejects_nonquiescent_state() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let bootstrap = validated.receipt().evidence();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut state =
            CanonicalExecutableMutationStateV1::from_bootstrap(bootstrap, &validated.storage);
        let program_model_sha256 = canonical_writer_program_model_sha256(
            &install.resolver,
            Some(&install.generations),
            &bootstrap.watched_ranges,
        );
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x7000, 1)));
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::PendingPhysicalWrites
        );
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| {
            pending.borrow_mut().push(GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: 0x7000,
                len: 1,
            });
        });
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::PendingAttributedWrites
        );
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let host = state.begin_host_transaction(
            7,
            GuestPc::new(INSTALL_PC.get() + 4),
            ExecutionKey::new(BankId::new(0xb007), INSTALL_PC),
        );
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::OpenHostTransactions
        );
        state.finish_host_transaction(host);

        let child = state.begin_child_transaction();
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::ActiveChildTransaction
        );
        state.finish_child_transaction(child);
        state.poison("synthetic incomplete publication".to_string());
        assert_eq!(
            validate_bootstrap_writer_completion_state(
                program_model_sha256,
                bootstrap,
                &validated.storage,
                &state,
            )
            .unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::Poisoned
        );
    }

    fn pi_test_trace(direction: fn64_runtime::DmaDirection) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::PiDmaRequest {
            direction,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            cart_addr: 0x20,
            len: 4,
        };
        let completion = fn64_runtime::DmaCompletion {
            direction,
            dram_addr: request.dram_addr,
            dev_addr: request.cart_addr,
            len: request.len,
        };
        [
            fn64_runtime::DeviceTraceKind::PiDmaStarted(request),
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::PiBusyCleared,
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi),
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn si_test_trace(kind: fn64_runtime::SiDmaKind) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::SiDmaRequest {
            kind,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x7000),
        };
        [
            fn64_runtime::DeviceTraceKind::SiDmaStarted(request),
            fn64_runtime::DeviceTraceKind::SiBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::SiBusyCleared,
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Si),
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(request),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn sp_test_trace(
        direction: fn64_runtime::SpDmaDirection,
    ) -> Vec<fn64_runtime::DeviceTraceEvent> {
        let request = fn64_runtime::SpDmaRequest {
            direction,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(request),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(request),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
            at: fn64_runtime::Cycles::new(100 + sequence as u64),
            sequence: sequence as u64,
            kind,
        })
        .collect()
    }

    fn production_aot_receipt_for_si_test() -> StaticExecutionBuildReceipt {
        StaticExecutionBuildReceipt {
            schema: 1,
            aot_runtime: true,
            production_aot: true,
            dev_interpreter: false,
        }
    }

    fn rdp_renderer_validator_fixture(
        publications: Vec<Vec<u64>>,
    ) -> (
        [u8; 0x80],
        CanonicalExecutableMutationStateV1,
        RdpRendererWriterRuntimeTraceEpochV1,
        RdpRendererWriterTraceV1,
    ) {
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut storage = [0u8; 0x80];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x40, 0x48)]);
        state.seal_with(|_| 0);
        storage[0x41 ^ 3] = 0xa5;
        let view = fn64_runtime::RdramView::from_storage(&storage);
        let snapshot = state
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::RdpRenderer,
                physical_offset: 0x41,
                len: 1,
            }],
            Vec::new(),
        );
        let epoch = RdpRendererWriterRuntimeTraceEpochV1 {
            epoch_id: 0x71,
            program_model_sha256: [0x72; 32],
        };
        let trace = RdpRendererWriterTraceV1 {
            epoch_id: epoch.epoch_id,
            program_model_sha256: epoch.program_model_sha256,
            initial_journal_entry_count: 0,
            next_journal_entry_index: state.entries.len(),
            publications,
            rejected_journal_sequences: Vec::new(),
        };
        (storage, state, epoch, trace)
    }

    #[test]
    fn rdp_renderer_writer_receipt_binds_publications_to_exact_journal_sequences() {
        let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
        let receipt = validate_rdp_renderer_writer_runtime_state_v1(
            epoch.program_model_sha256,
            [0x73; 32],
            Some([0x74; 32]),
            production_aot_receipt_for_si_test(),
            true,
            &epoch,
            &storage,
            &state,
            &trace,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert!(receipt.has_valid_evidence_hash());
        assert_eq!(receipt.evidence().renderer_publication_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_entry_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_declaration_count, 1);
    }

    #[test]
    fn rdp_renderer_writer_receipt_rejects_unbound_journal_commit() {
        let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![Vec::new()]);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }

    #[test]
    fn rdp_renderer_writer_receipt_rejects_speculative_needs_lle_write() {
        let (storage, state, epoch, mut trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
        trace.rejected_journal_sequences.push(0);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }

    #[test]
    fn rdp_renderer_writer_rejection_precedes_retryable_empty_publication() {
        let (storage, state, epoch, mut trace) = rdp_renderer_validator_fixture(Vec::new());
        trace.rejected_journal_sequences.push(0);
        assert_eq!(
            validate_rdp_renderer_writer_runtime_state_v1(
                epoch.program_model_sha256,
                [0x73; 32],
                Some([0x74; 32]),
                production_aot_receipt_for_si_test(),
                true,
                &epoch,
                &storage,
                &state,
                &trace,
                false,
                false,
                false,
                false,
            )
            .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace
        );
    }

    #[test]
    fn rdp_renderer_writer_receipt_rejects_each_pending_renderer_owner() {
        for (rsp, dpc, dp, abi, expected) in [
            (
                true,
                false,
                false,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask,
            ),
            (
                false,
                true,
                false,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction,
            ),
            (
                false,
                false,
                true,
                false,
                RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion,
            ),
            (
                false,
                false,
                false,
                true,
                RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork,
            ),
        ] {
            let (storage, state, epoch, trace) = rdp_renderer_validator_fixture(vec![vec![0]]);
            assert_eq!(
                validate_rdp_renderer_writer_runtime_state_v1(
                    epoch.program_model_sha256,
                    [0x73; 32],
                    Some([0x74; 32]),
                    production_aot_receipt_for_si_test(),
                    true,
                    &epoch,
                    &storage,
                    &state,
                    &trace,
                    rsp,
                    dpc,
                    dp,
                    abi,
                )
                .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn rdp_renderer_writer_epoch_is_process_unique_across_thread_local_owners() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let ids = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    next_rdp_renderer_writer_trace_epoch_id()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let values = ids
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_ne!(values[0], values[1]);
    }

    struct PublicSiRuntimeStateTestReset;

    impl Drop for PublicSiRuntimeStateTestReset {
        fn drop(&mut self) {
            with_executor(|executor| *executor = fn64_runtime::Executor::new());
            with_host(|host| *host = super::super::HostState::default());
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow_mut().clear());
            CPU_INSTRUCTION_STORE_TRACE.with(|trace| *trace.borrow_mut() = None);
            RDP_RENDERER_WRITER_TRACE.with(|trace| *trace.borrow_mut() = None);
            fn64_recomp_rs::set_write_observer(None);
            fn64_recomp_rs::set_guest_write_boundary_observer(None);
        }
    }

    fn install_public_si_runtime_state_test_owner() -> fn64_runtime::SiDmaRequest {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let CatalogGenerationInstallV1 {
            mut resolver,
            generations,
        } = install;
        let AbiHostFunctionCatalogV1 { catalog, evidence } =
            issue_abi_host_function_catalog_v1(Vec::new()).unwrap();
        resolver.host_functions = catalog;
        resolver.evidence.abi_host_catalog = Some(evidence);
        // Unit tests compile the development feature lane. Override only this
        // private test owner so the public runtime-state path can exercise its
        // production-lane precondition without weakening the real constructor.
        resolver.evidence.build_receipt = production_aot_receipt_for_si_test();
        let bootstrap_evidence = validated.receipt().evidence().clone();
        let watched_ranges = bootstrap_evidence.watched_ranges.clone();
        let writer_program_model_sha256 =
            canonical_writer_program_model_sha256(&resolver, Some(&generations), &watched_ranges);
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            &bootstrap_evidence,
            &validated.storage,
        );
        let live = CanonicalLiveBlockProgramV1 {
            install: Rc::new(resolver),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_units: Rc::new(RefCell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_withheld_static_key: Rc::new(Cell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_execution_aggregates: Rc::new(RefCell::new(BTreeMap::new())),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_unsupported_exits: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_unsupported_exits: Rc::new(Cell::new(0)),
            canonical_charged_instructions: Rc::new(Cell::new(0)),
            canonical_instruction_limit: Rc::new(Cell::new(None)),
            thread_publications: Rc::new(RefCell::new(BTreeMap::new())),
            generations: Some(Rc::new(RefCell::new(generations))),
            mutation_state: Some(Rc::new(RefCell::new(state))),
            bootstrap_evidence: Some(bootstrap_evidence),
            writer_program_model_sha256,
            bootstrap_writer_completion: Rc::new(RefCell::new(None)),
            cpu_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            cpu_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            pi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            pi_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            si_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            host_abi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            rdp_renderer_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rdp_renderer_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        };
        let mut storage = validated.storage;
        let rdram = storage.as_mut_ptr();
        let rdram_len = storage.len();
        with_host(|host| {
            *host = super::super::HostState::default();
            host.runtime_rdram = rdram;
            host.runtime_rdram_len = rdram_len;
            host.owned_runtime_rdram = Some(storage);
            host.canonical_recompiled_program = Some(live);
        });
        EXECUTABLE_WRITE_RANGES.with(|ranges| {
            ranges.borrow_mut().extend(
                watched_ranges
                    .iter()
                    .map(|range| (range.physical_start, range.physical_end)),
            );
        });
        fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        fn64_recomp_rs::set_guest_write_boundary_observer(Some(classify_live_executable_write));
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        crate::load_rom(rom);
        fn64_runtime::SiDmaRequest {
            kind: fn64_runtime::SiDmaKind::PifToDram,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
        }
    }

    #[test]
    fn rdp_renderer_writer_public_path_consumes_one_fresh_publication_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rdp_renderer_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh renderer epoch");
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        // SAFETY: the test owner retains the allocation in HostState for the
        // complete scope and no competing slice exists while this call runs.
        let storage = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        track_rdp_renderer_mutation(storage, |storage| storage[0x7000 ^ 3] ^= 1);
        record_rdp_renderer_publication_v1();

        let receipt = take_validated_rdp_renderer_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("one committed renderer publication must mint one prerequisite");
        assert!(receipt.has_valid_evidence_hash());
        assert_eq!(receipt.evidence().renderer_publication_count, 1);
        assert_eq!(receipt.evidence().rdp_renderer_journal_entry_count, 1);
        assert!(
            take_validated_rdp_renderer_writer_runtime_state_receipt_v1(&epoch)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pi_writer_runtime_state_public_path_owns_fresh_epoch_and_completed_read_dma() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh PI epoch");
        assert!(crate::copy_device_trace().is_empty());
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_0000, 0x6000));
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_0004, 0x20));
        assert!(write_raw_mmio(0xFFFF_FFFF_A460_0008, 3));
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingDevicePi
        );

        crate::pi::advance_device_time(1);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 5);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::PiDmaStarted(
                fn64_runtime::PiDmaRequest {
                    direction: fn64_runtime::DmaDirection::ToRdram,
                    ..
                }
            ))
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(fn64_runtime::DmaCompletion {
                    direction: fn64_runtime::DmaDirection::ToRdram,
                    ..
                })
            ))
        ));
        let receipt = take_validated_pi_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh completed PI lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().pi_started, 1);
        assert_eq!(receipt.evidence().pi_committed, 1);
        assert_eq!(receipt.evidence().pi_to_rdram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_pi_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }

    #[test]
    fn rsp_writer_runtime_state_public_path_binds_task_owned_writeback() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");
        let owner = crate::task_dispatch::RspInterpreterOwner::RawKick {
            admission_generation: crate::task_dispatch::RspTaskAdmissionGeneration::new(
                std::num::NonZeroU64::new(9).unwrap(),
            ),
        };
        crate::task_dispatch::record_test_rsp_writer_commits_v1(
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { owner },
            &[(0x6000, 0x6008)],
        );

        let receipt = take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh task-owned RSP writeback must mint one inner receipt");
        assert_eq!(receipt.evidence().interpreter_writeback_count, 1);
        assert_eq!(receipt.evidence().translated_audio_hle_publication_count, 0);
        assert_eq!(receipt.evidence().writeback_range_count, 1);
        assert_eq!(receipt.evidence().rsp_journal_declaration_count, 0);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }

    unsafe extern "C" fn translated_audio_test_callback(rdram: *mut u8, _task: u32) -> u32 {
        let physical = (INSTALL_PC.get() & 0x1fff_ffff) as usize;
        unsafe { *rdram.add(physical ^ 3) ^= 1 };
        0
    }

    unsafe extern "C" fn rejected_translated_audio_test_callback(
        rdram: *mut u8,
        _task: u32,
    ) -> u32 {
        let physical = (INSTALL_PC.get() & 0x1fff_ffff) as usize;
        unsafe { *rdram.add(physical ^ 3) ^= 1 };
        9
    }

    #[test]
    fn rsp_writer_runtime_state_credits_real_translated_audio_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");

        unsafe {
            crate::task_dispatch::test_dispatch_translated_audio_task_v1(
                0x40,
                translated_audio_test_callback,
            )
        };

        let receipt = take_validated_rsp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("successful translated audio executable publication must mint a receipt");
        assert_eq!(receipt.evidence().translated_audio_hle_publication_count, 1);
        assert_eq!(receipt.evidence().interpreter_writeback_count, 0);
        assert_eq!(receipt.evidence().writeback_range_count, 0);
        assert_eq!(receipt.evidence().rsp_journal_declaration_count, 1);
        assert!(receipt.has_valid_evidence_hash());
    }

    #[test]
    fn rsp_writer_runtime_state_rejects_real_non_break_audio_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh RSP epoch");

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            crate::task_dispatch::test_dispatch_translated_audio_task_v1(
                0x40,
                rejected_translated_audio_test_callback,
            )
        }));
        assert!(rejected.is_err());
        with_host(|host| {
            host.rsp_task_lineages.clear();
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });

        assert_eq!(
            take_validated_rsp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            RspWriterRuntimeStateErrorV1::RejectedRspExecutableMutation
        );
    }

    #[test]
    fn rsp_writer_runtime_state_rejects_pending_owner_and_empty_trace() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let owner = crate::task_dispatch::RspInterpreterOwner::RawKick {
            admission_generation: crate::task_dispatch::RspTaskAdmissionGeneration::new(
                std::num::NonZeroU64::new(10).unwrap(),
            ),
        };
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { owner };
        });
        assert_eq!(
            begin_rsp_writer_runtime_trace_epoch_v1().unwrap_err(),
            RspWriterRuntimeStateErrorV1::PendingAbiRspWork
        );
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });
        let epoch = begin_rsp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("quiescent owner must mint an RSP epoch");
        assert_eq!(
            take_validated_rsp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            RspWriterRuntimeStateErrorV1::NoRspWritebacks
        );
    }

    #[test]
    fn rsp_writer_trace_epoch_ids_are_process_unique_across_threads() {
        let ids = (0..8)
            .map(|_| std::thread::spawn(next_rsp_writer_trace_epoch_id))
            .map(|thread| thread.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 8);
        assert!(ids.iter().all(|id| *id != 0));
    }

    #[test]
    fn pi_writer_runtime_state_rejects_nonwriting_incomplete_and_drifted_lifecycles() {
        assert_eq!(
            validate_pi_transition_trace(&pi_test_trace(fn64_runtime::DmaDirection::FromRdram))
                .unwrap_err(),
            PiWriterRuntimeStateErrorV1::NoToRdramCommit
        );
        let complete = pi_test_trace(fn64_runtime::DmaDirection::ToRdram);
        assert_eq!(
            validate_pi_transition_trace(&complete[..complete.len() - 1]).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
        let mut drifted = complete.clone();
        if let fn64_runtime::DeviceTraceKind::PiBytesCommitted(ref mut request) = drifted[1].kind {
            request.dram_addr = fn64_runtime::RdramAddr::from_offset(0x6010);
        }
        assert_eq!(
            validate_pi_transition_trace(&drifted).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
        let mut nonmonotonic = complete;
        nonmonotonic[4].sequence = 1;
        assert_eq!(
            validate_pi_transition_trace(&nonmonotonic).unwrap_err(),
            PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder
        );
    }

    #[test]
    fn pi_writer_runtime_state_accepts_serialized_requests_while_interrupt_remains_asserted() {
        let mut trace = pi_test_trace(fn64_runtime::DmaDirection::ToRdram);
        let second = fn64_runtime::PiDmaRequest {
            direction: fn64_runtime::DmaDirection::ToRdram,
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            cart_addr: 0x24,
            len: 4,
        };
        let completion = fn64_runtime::DmaCompletion {
            direction: second.direction,
            dram_addr: second.dram_addr,
            dev_addr: second.cart_addr,
            len: second.len,
        };
        for kind in [
            fn64_runtime::DeviceTraceKind::PiDmaStarted(second),
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(second),
            fn64_runtime::DeviceTraceKind::PiBusyCleared,
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ),
        ] {
            let sequence = trace.len() as u64;
            trace.push(fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::Cycles::new(200 + sequence),
                sequence,
                kind,
            });
        }
        let (started, committed, busy, raised, cleared, notifications, writes, digest) =
            validate_pi_transition_trace(&trace).unwrap();
        assert_eq!(
            (
                started,
                committed,
                busy,
                raised,
                cleared,
                notifications,
                writes
            ),
            (2, 2, 2, 1, 0, 2, 2)
        );
        assert_ne!(digest, [0; 32]);
    }

    #[test]
    fn pi_writer_runtime_state_rejects_pending_interrupt_and_superseded_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        with_host(|host| {
            host.device_fabric
                .raise_interrupt(fn64_runtime::InterruptSource::Pi)
        });
        assert_eq!(
            begin_pi_writer_runtime_trace_epoch_v1().unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingPiInterrupt
        );
        with_host(|host| {
            host.device_fabric
                .clear_interrupt(fn64_runtime::InterruptSource::Pi)
        });
        let old = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first PI epoch");
        let current = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement PI epoch");
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            PiWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        assert_eq!(
            take_validated_pi_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            PiWriterRuntimeStateErrorV1::NoPiTransitions
        );
    }

    #[test]
    fn pi_writer_runtime_state_rejects_pending_abi_completion_owner() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_pi_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("fresh PI epoch");
        let (live, storage) = with_host(|host| {
            (
                host.canonical_recompiled_program.clone().unwrap(),
                host.owned_runtime_rdram.as_deref().unwrap().to_vec(),
            )
        });
        assert_eq!(
            live.take_pi_writer_runtime_state(
                &epoch,
                &storage,
                true,
                &pi_test_trace(fn64_runtime::DmaDirection::ToRdram),
                false,
                true,
            )
            .unwrap_err(),
            PiWriterRuntimeStateErrorV1::PendingAbiPi
        );
    }

    #[test]
    fn pi_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_pi_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("PI epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn public_si_runtime_state_path_requires_fresh_completed_device_lifecycle() {
        let _reset = PublicSiRuntimeStateTestReset;
        let request = install_public_si_runtime_state_test_owner();
        crate::set_device_trace_enabled(false);
        crate::set_device_trace_enabled(true);
        assert!(crate::copy_device_trace().is_empty());
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));

        crate::pi::start_live_si_dma(
            request,
            crate::PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len },
        )
        .unwrap();
        assert_eq!(
            take_validated_si_writer_runtime_state_receipt_v1().unwrap_err(),
            SiWriterRuntimeStateErrorV1::PendingDeviceSi
        );

        crate::pi::advance_device_time(1);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 5);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SiDmaStarted(actual)) if actual == request
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(actual)
            )) if actual == request
        ));
        let receipt = take_validated_si_writer_runtime_state_receipt_v1()
            .unwrap()
            .expect("fresh completed SI lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().si_started, 1);
        assert_eq!(receipt.evidence().si_committed, 1);
        assert_eq!(receipt.evidence().si_pif_to_dram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_si_writer_runtime_state_receipt_v1()
            .unwrap()
            .is_none());
    }

    #[test]
    fn sp_writer_runtime_state_public_path_owns_fresh_epoch_and_completed_write_dma() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        assert!(write_raw_mmio(0xFFFF_FFFF_A400_0000, 0x1122_3344));
        let epoch = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one fresh SP epoch");
        assert!(crate::copy_device_trace().is_empty());
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_0000, 0));
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_0004, 0x6000));
        assert!(write_raw_mmio(0xFFFF_FFFF_A404_000C, 7));
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingDeviceSpDma
        );

        crate::pi::advance_device_time(9);
        let trace = crate::copy_device_trace();
        assert_eq!(trace.len(), 3);
        assert!(matches!(
            trace.first().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SpDmaStarted(
                fn64_runtime::SpDmaRequest {
                    direction: fn64_runtime::SpDmaDirection::RspToRdram,
                    ..
                }
            ))
        ));
        assert!(matches!(
            trace.last().map(|event| event.kind),
            Some(fn64_runtime::DeviceTraceKind::SpDmaBusyCleared)
        ));
        let receipt = take_validated_sp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh completed SP lifecycle must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().sp_started, 1);
        assert_eq!(receipt.evidence().sp_committed, 1);
        assert_eq!(receipt.evidence().sp_rsp_to_rdram_committed, 1);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_sp_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }

    #[test]
    fn sp_writer_runtime_state_rejects_nonwriting_and_bad_queued_handoff() {
        assert_eq!(
            validate_sp_transition_trace(&sp_test_trace(fn64_runtime::SpDmaDirection::RdramToRsp))
                .unwrap_err(),
            SpWriterRuntimeStateErrorV1::NoRspToRdramCommit
        );
        let first = fn64_runtime::SpDmaRequest {
            direction: fn64_runtime::SpDmaDirection::RspToRdram,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        let queued = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            ..first
        };
        let wrong = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6020),
            ..first
        };
        let kinds = [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(first),
            fn64_runtime::DeviceTraceKind::SpDmaQueued(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(first),
            fn64_runtime::DeviceTraceKind::SpDmaStarted(wrong),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(wrong),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ];
        let trace = kinds
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::Cycles::new(100 + sequence as u64),
                sequence: sequence as u64,
                kind,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_sp_transition_trace(&trace).unwrap_err(),
            SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder
        );
    }

    #[test]
    fn sp_writer_runtime_state_public_path_rejects_superseded_epoch() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let old = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first SP epoch");
        let current = begin_sp_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement SP epoch");
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            SpWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        assert_eq!(
            take_validated_sp_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            SpWriterRuntimeStateErrorV1::NoSpTransitions
        );
    }

    #[test]
    fn sp_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_sp_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("SP epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn cpu_writer_runtime_state_public_path_owns_fresh_quiescent_store_window() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let epoch = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("canonical owner must mint one CPU-store epoch");
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::NoCpuStores
        );

        record_executable_and_renderer_write(GuestWriteEvent::Range {
            channel: WriterChannel::CpuInstructionStore,
            physical_offset: 0x6000,
            len: 4,
        });
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&epoch).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::PendingPhysicalWrites
        );
        let live = with_host(|host| host.canonical_recompiled_program.clone().unwrap());
        let storage = with_host(|host| host.owned_runtime_rdram.as_deref().unwrap().to_vec());
        let view = fn64_runtime::RdramView::from_storage(&storage);
        live.invalidate_pending_physical_writes_with(|physical| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        });

        let receipt = take_validated_cpu_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .expect("fresh quiescent CPU store must mint one runtime-state prerequisite");
        assert_eq!(receipt.evidence().cpu_store_count, 1);
        assert_eq!(receipt.evidence().cpu_journal_declaration_count, 0);
        assert!(receipt.has_valid_evidence_hash());
        assert!(take_validated_cpu_writer_runtime_state_receipt_v1(&epoch)
            .unwrap()
            .is_none());
    }

    #[test]
    fn cpu_writer_runtime_state_rejects_superseded_epoch_and_invalid_ranges() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        let old = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("first CPU-store epoch");
        let current = begin_cpu_writer_runtime_trace_epoch_v1()
            .unwrap()
            .expect("replacement CPU-store epoch");
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&old).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::TraceEpochMismatch
        );
        record_executable_and_renderer_write(GuestWriteEvent::Range {
            channel: WriterChannel::CpuInstructionStore,
            physical_offset: fn64_recomp_rs::RDRAM_LEN as u32,
            len: 4,
        });
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        assert_eq!(
            take_validated_cpu_writer_runtime_state_receipt_v1(&current).unwrap_err(),
            CpuWriterRuntimeStateErrorV1::InvalidCpuStoreRange
        );
    }

    #[test]
    fn cpu_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_cpu_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("CPU epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn host_abi_writer_runtime_state_binds_exact_catalog_lifecycle_and_journal() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let mut validated = transaction.commit().unwrap();
        let mut state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let target = GuestPc::new(0x8000_1000);
        let host_catalog = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: target.get(),
            shim: AbiHostShimV1::OsCreateMesgQueue,
        }])
        .unwrap();
        let host_catalog_evidence = host_catalog.evidence().clone();
        state.host_abi_writer_trace = Some(HostAbiWriterTraceV1 {
            epoch_id: 41,
            initial_journal_entry_count: 1,
            events: Vec::new(),
        });
        let transaction =
            state.begin_host_transaction(7, target, ExecutionKey::new(INSTALL_BANK, INSTALL_PC));
        unsafe {
            fn64_runtime::RdramPtr::from_storage_ptr(validated.storage.as_mut_ptr()).write_u8(
                fn64_runtime::RdramAddr::from_offset(INSTALL_PC.get() & 0x1fff_ffff),
                0xaa,
            );
        }
        let view = fn64_runtime::RdramView::from_storage(&validated.storage);
        let snapshot = state
            .read_snapshot(|physical| view.read_u8(fn64_runtime::RdramAddr::from_offset(physical)));
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::HostAbi,
                physical_offset: INSTALL_PC.get() & 0x1fff_ffff,
                len: 1,
            }],
            Vec::new(),
        );
        state.record_host_abi_boundary(transaction, 1);
        state.finish_host_transaction(transaction);
        let trace = state.host_abi_writer_trace.clone().unwrap();
        let receipt = validate_host_abi_writer_runtime_state_v1(
            [0x11; 32],
            [0x22; 32],
            Some(&host_catalog_evidence),
            production_aot_receipt_for_si_test(),
            true,
            Some(41),
            &validated.storage,
            &state,
            Some(&trace),
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(evidence.transactions_started, 1);
        assert_eq!(evidence.transactions_finished, 1);
        assert_eq!(evidence.ordering_boundaries, 1);
        assert_eq!(evidence.host_abi_journal_entry_count, 1);
        assert_eq!(evidence.host_abi_journal_declaration_count, 1);
        assert!(receipt.has_valid_evidence_hash());
    }

    #[test]
    fn host_abi_writer_runtime_state_rejects_call_without_write_and_unknown_target() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let target = GuestPc::new(0x8000_1000);
        let host_catalog = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: target.get(),
            shim: AbiHostShimV1::OsCreateMesgQueue,
        }])
        .unwrap();
        let frame = OpenHostMutationTransactionEvidenceV1 {
            transaction_id: 3,
            thread: 5,
            target,
            resume: ExecutionKey::new(INSTALL_BANK, INSTALL_PC),
        };
        let trace = HostAbiWriterTraceV1 {
            epoch_id: 42,
            initial_journal_entry_count: 1,
            events: vec![
                HostAbiWriterTraceEventV1::Started(frame),
                HostAbiWriterTraceEventV1::Boundary {
                    transaction_id: 3,
                    thread: 5,
                    journal_sequences: Vec::new(),
                },
                HostAbiWriterTraceEventV1::Finished {
                    transaction_id: 3,
                    thread: 5,
                },
            ],
        };
        let validate = |trace: &HostAbiWriterTraceV1| {
            validate_host_abi_writer_runtime_state_v1(
                [0x11; 32],
                [0x22; 32],
                Some(host_catalog.evidence()),
                production_aot_receipt_for_si_test(),
                true,
                Some(42),
                &validated.storage,
                &state,
                Some(trace),
            )
            .unwrap_err()
        };
        assert_eq!(
            validate(&trace),
            HostAbiWriterRuntimeStateErrorV1::NoHostAbiWriteCommit
        );
        let mut unknown = trace;
        if let HostAbiWriterTraceEventV1::Started(frame) = &mut unknown.events[0] {
            frame.target = GuestPc::new(0x8000_2000);
        }
        assert_eq!(
            validate(&unknown),
            HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle
        );
    }

    #[test]
    fn host_abi_writer_runtime_state_epoch_ids_are_process_unique_across_threads() {
        let mut ids = (0..16)
            .map(|_| std::thread::spawn(next_host_abi_writer_trace_epoch_id))
            .map(|thread| thread.join().expect("Host ABI epoch mint thread panicked"))
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert!(ids.iter().all(|id| *id != 0));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn sp_writer_runtime_state_public_epoch_rejects_pending_device_and_abi_rsp_owners() {
        let _reset = PublicSiRuntimeStateTestReset;
        let _ = install_public_si_runtime_state_test_owner();
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight {
                    owner: crate::task_dispatch::RspInterpreterOwner::RawKick {
                        admission_generation:
                            crate::task_dispatch::RspTaskAdmissionGeneration::first(),
                    },
                };
        });
        assert_eq!(
            begin_sp_writer_runtime_trace_epoch_v1().unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingAbiSpWork
        );
        with_host(|host| {
            host.rsp_interpreter_state =
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::Reset;
        });
        crate::pi::start_live_rcp_task_with_latency(
            fn64_runtime::RcpTaskCompletionPlan::SpOnly,
            10,
        )
        .unwrap();
        assert_eq!(
            begin_sp_writer_runtime_trace_epoch_v1().unwrap_err(),
            SpWriterRuntimeStateErrorV1::PendingDeviceSpTask
        );
    }

    #[test]
    fn sp_writer_runtime_state_accepts_exact_queued_handoff() {
        let first = fn64_runtime::SpDmaRequest {
            direction: fn64_runtime::SpDmaDirection::RspToRdram,
            mem_addr: fn64_runtime::RspMemAddr::from_register(0),
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6000),
            encoded_len: 7,
        };
        let queued = fn64_runtime::SpDmaRequest {
            dram_addr: fn64_runtime::RdramAddr::from_offset(0x6010),
            ..first
        };
        let kinds = [
            fn64_runtime::DeviceTraceKind::SpDmaStarted(first),
            fn64_runtime::DeviceTraceKind::SpDmaQueued(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(first),
            fn64_runtime::DeviceTraceKind::SpDmaStarted(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(queued),
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared,
        ];
        let trace = kinds
            .into_iter()
            .enumerate()
            .map(|(sequence, kind)| fn64_runtime::DeviceTraceEvent {
                at: fn64_runtime::Cycles::new(100 + sequence as u64),
                sequence: sequence as u64,
                kind,
            })
            .collect::<Vec<_>>();
        let (started, queued, committed, busy_cleared, writes, digest) =
            validate_sp_transition_trace(&trace).unwrap();
        assert_eq!(
            (started, queued, committed, busy_cleared, writes),
            (2, 1, 2, 1, 2)
        );
        assert_ne!(digest, [0; 32]);
    }

    #[test]
    fn si_writer_runtime_state_prerequisite_binds_quiescent_trace_and_journal() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let receipt = validate_si_writer_runtime_state_v1(
            [0x11; 32],
            [0x22; 32],
            Some([0x33; 32]),
            production_aot_receipt_for_si_test(),
            true,
            &validated.storage,
            &state,
            &si_test_trace(fn64_runtime::SiDmaKind::PifToDram),
            false,
            false,
        )
        .unwrap();
        let evidence = receipt.evidence();
        assert_eq!(evidence.schema, SI_WRITER_RUNTIME_STATE_SCHEMA_V1);
        assert_eq!(evidence.si_started, 1);
        assert_eq!(evidence.si_committed, 1);
        assert_eq!(evidence.si_pif_to_dram_committed, 1);
        assert_eq!(evidence.journal_entry_count, 1);
        assert_eq!(evidence.si_journal_declaration_count, 0);
        assert_eq!(evidence.journal_root_sha256, state.journal_root_sha256);
        assert!(receipt.has_valid_evidence_hash());
    }

    #[test]
    fn si_writer_runtime_state_rejects_missing_authority_and_nonquiescence() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let validate = |host, build, pending_device, pending_abi| {
            validate_si_writer_runtime_state_v1(
                [0x11; 32],
                [0x22; 32],
                host,
                build,
                true,
                &validated.storage,
                &state,
                &si_test_trace(fn64_runtime::SiDmaKind::PifToDram),
                pending_device,
                pending_abi,
            )
            .unwrap_err()
        };
        assert_eq!(
            validate(None, production_aot_receipt_for_si_test(), false, false),
            SiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                StaticExecutionBuildReceipt {
                    schema: 1,
                    aot_runtime: true,
                    production_aot: false,
                    dev_interpreter: true,
                },
                false,
                false,
            ),
            SiWriterRuntimeStateErrorV1::NonProductionAotBuild
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                production_aot_receipt_for_si_test(),
                true,
                false,
            ),
            SiWriterRuntimeStateErrorV1::PendingDeviceSi
        );
        assert_eq!(
            validate(
                Some([0x33; 32]),
                production_aot_receipt_for_si_test(),
                false,
                true,
            ),
            SiWriterRuntimeStateErrorV1::PendingAbiSi
        );
    }

    #[test]
    fn si_writer_runtime_state_rejects_incomplete_or_nonwriting_trace() {
        let complete = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        assert_eq!(
            validate_si_transition_trace(&complete[..complete.len() - 1]).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        assert_eq!(
            validate_si_transition_trace(&si_test_trace(fn64_runtime::SiDmaKind::DramToPif))
                .unwrap_err(),
            SiWriterRuntimeStateErrorV1::NoPifToDramCommit
        );
        let mut drifted = complete;
        if let fn64_runtime::DeviceTraceKind::SiBytesCommitted(ref mut request) = drifted[1].kind {
            request.dram_addr = fn64_runtime::RdramAddr::from_offset(0x7040);
        }
        assert_eq!(
            validate_si_transition_trace(&drifted).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        let mut nonmonotonic = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        nonmonotonic[3].at = fn64_runtime::Cycles::new(99);
        assert_eq!(
            validate_si_transition_trace(&nonmonotonic).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
        let mut sequence_regression = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        sequence_regression[3].at = fn64_runtime::Cycles::new(200);
        sequence_regression[3].sequence = 1;
        assert_eq!(
            validate_si_transition_trace(&sequence_regression).unwrap_err(),
            SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder
        );
    }

    #[test]
    fn si_writer_runtime_state_receipt_has_one_successful_take() {
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, INSTALL_PC.get(), 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let CatalogGenerationInstallV1 {
            mut resolver,
            generations,
        } = install;
        let AbiHostFunctionCatalogV1 { catalog, evidence } =
            issue_abi_host_function_catalog_v1(Vec::new()).unwrap();
        resolver.host_functions = catalog;
        resolver.evidence.abi_host_catalog = Some(evidence);
        resolver.evidence.build_receipt = production_aot_receipt_for_si_test();
        let watched_ranges = validated.receipt().evidence().watched_ranges.clone();
        let writer_program_model_sha256 =
            canonical_writer_program_model_sha256(&resolver, Some(&generations), &watched_ranges);
        let state = CanonicalExecutableMutationStateV1::from_bootstrap(
            validated.receipt().evidence(),
            &validated.storage,
        );
        let live = CanonicalLiveBlockProgramV1 {
            install: Rc::new(resolver),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_units: Rc::new(RefCell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_withheld_static_key: Rc::new(Cell::new(None)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_execution_aggregates: Rc::new(RefCell::new(BTreeMap::new())),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_identity_unsupported_exits: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_activations: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_charged_instructions: Rc::new(Cell::new(0)),
            #[cfg(feature = "dynamic-mapped-runtime")]
            dynamic_dropped_attempted_entry_unsupported_exits: Rc::new(Cell::new(0)),
            canonical_charged_instructions: Rc::new(Cell::new(0)),
            canonical_instruction_limit: Rc::new(Cell::new(None)),
            thread_publications: Rc::new(RefCell::new(BTreeMap::new())),
            generations: Some(Rc::new(RefCell::new(generations))),
            mutation_state: Some(Rc::new(RefCell::new(state))),
            bootstrap_evidence: Some(validated.receipt().evidence().clone()),
            writer_program_model_sha256,
            bootstrap_writer_completion: Rc::new(RefCell::new(None)),
            cpu_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            cpu_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            pi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            pi_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            si_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            sp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            host_abi_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rsp_writer_trace_epoch_id: Rc::new(Cell::new(None)),
            rdp_renderer_writer_runtime_state_taken: Rc::new(Cell::new(false)),
            rdp_renderer_writer_trace_epoch_id: Rc::new(Cell::new(None)),
        };
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let trace = si_test_trace(fn64_runtime::SiDmaKind::PifToDram);
        assert!(live
            .take_si_writer_runtime_state(&validated.storage, true, &trace, false, false,)
            .unwrap()
            .is_some());
        assert!(live
            .take_si_writer_runtime_state(&validated.storage, true, &trace, false, false,)
            .unwrap()
            .is_none());
    }

    #[test]
    fn bootstrap_import_rejects_wrong_entry_image_and_conflicting_publication() {
        let install = bootstrap_test_install(0x2402_0001);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&0x2402_0002u32.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&0x2402_0003u32.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(matches!(
            transaction.publish_resident_rom_image(0x24, 0x8000_7000, 4),
            Err(BootstrapImportErrorV1::ConflictingPublication { .. })
        ));
        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::InitialEntryImageMismatch {
                expected: 0x2402_0001,
                actual: 0x2402_0002,
                ..
            })
        ));
    }

    #[test]
    fn bootstrap_import_rejects_a_wrong_non_entry_static_bank() {
        let entry_word = 0x2402_0001;
        let static_word = 0x2403_0002;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&(static_word + 1).to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&physical_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x24, 0x8000_8000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x28, 0x8000_9000, 4)
            .unwrap();

        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::StaticProgramImageMismatch {
                bank,
                pc,
                expected,
                actual,
            }) if bank == BankId::new(0xb008)
                && pc == GuestPc::new(0x8000_8000)
                && expected == static_word
                && actual == static_word + 1
        ));
    }

    #[test]
    fn bootstrap_import_rejects_a_wrong_physical_bank() {
        let entry_word = 0x2402_0001;
        let static_word = 0x2403_0002;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&static_word.to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&(physical_word + 1).to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x24, 0x8000_8000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x28, 0x8000_9000, 4)
            .unwrap();

        assert!(matches!(
            transaction.commit(),
            Err(BootstrapImportErrorV1::PhysicalProgramImageMismatch {
                bank,
                physical_address: 0x9000,
                expected,
                actual,
            }) if bank == BankId::new(0xb009)
                && expected == physical_word
                && actual == physical_word + 1
        ));
    }

    #[test]
    fn bootstrap_import_does_not_expect_future_bytes_for_a_reserved_generation_bank() {
        let entry_word = 0x2402_0001;
        let future_word = 0x3c1a_8003;
        let future_bank = BankId::new(0xb00a);
        let install = bootstrap_test_install_with_generation(entry_word, future_word);
        assert!(install.generations.contains_reserved_bank(future_bank));

        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(transaction
            .commit()
            .unwrap()
            .receipt()
            .evidence()
            .initial_generations
            .is_empty());
    }

    #[test]
    fn bootstrap_import_binds_zero_or_exact_generation_images() {
        let entry_word = 0x2402_0001;
        let generation_word = 0x2403_0002;
        let install = bootstrap_test_install_with_generation(entry_word, generation_word);

        let mut zero_rom = vec![0; 0x40];
        zero_rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        let mut zero = install
            .begin_bootstrap_import_v1(
                &zero_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        zero.publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert!(zero
            .commit()
            .unwrap()
            .receipt()
            .evidence()
            .initial_generations
            .is_empty());

        let mut exact_rom = zero_rom.clone();
        exact_rom[0x24..0x28].copy_from_slice(&generation_word.to_be_bytes());
        let mut exact = install
            .begin_bootstrap_import_v1(
                &exact_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        exact
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        exact
            .publish_resident_rom_image(0x24, 0x8000_a000, 4)
            .unwrap();
        assert_eq!(
            exact
                .commit()
                .unwrap()
                .receipt()
                .evidence()
                .initial_generations,
            [GenerationId::new(0xaaa)]
        );

        let unknown_word = generation_word + 1;
        let mut unknown_rom = zero_rom;
        unknown_rom[0x24..0x28].copy_from_slice(&unknown_word.to_be_bytes());
        let mut unknown = install
            .begin_bootstrap_import_v1(
                &unknown_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        unknown
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        unknown
            .publish_resident_rom_image(0x24, 0x8000_a000, 4)
            .unwrap();
        assert!(matches!(
            unknown.commit(),
            Err(BootstrapImportErrorV1::UnrecognizedInitialGenerationImage {
                physical_address: 0xa000,
                actual: 0x24,
            })
        ));
    }

    #[test]
    fn bootstrap_import_exact_duplicate_is_canonicalized() {
        let install = bootstrap_test_install(0x2402_0001);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&0x2402_0001u32.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        assert_eq!(
            transaction
                .commit()
                .unwrap()
                .receipt()
                .evidence()
                .publications
                .len(),
            1
        );
    }

    #[test]
    fn validated_boot_owns_rdram_and_starts_journal_with_bootstrap_batch() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let entry = GuestPc::new(0x8000_7000);

        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(entry),
            0xb007,
            10,
        )
        .unwrap();

        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(evidence.bootstrap.is_some());
        let journal = evidence.mutation_journal.unwrap();
        assert!(journal.sealed);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].sequence, 0);
        assert!(journal.entries[0]
            .declared_writes
            .iter()
            .all(|write| write.channel == WriterChannel::BootstrapOrImport));
        let completion = take_validated_bootstrap_writer_channel_receipt_v1()
            .expect("validated boot must mint bootstrap writer authority");
        assert!(completion.has_valid_evidence_hash());
        assert_eq!(completion.evidence().journal_entry, journal.entries[0]);
        assert!(take_validated_bootstrap_writer_channel_receipt_v1().is_none());
        let mut steps = 0;
        while !crate::is_thread_dead(0xb007) {
            assert!(
                crate::run_one_step(),
                "validated bootstrap thread stalled before returning"
            );
            steps += 1;
            assert!(steps < 4, "validated bootstrap thread did not return");
        }
        assert!(crate::is_thread_dead(0xb007));
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn canonical_scheduler_mirror_commits_exact_host_abi_write_before_dispatch() {
        let _reset = PublicSiRuntimeStateTestReset;
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());

        let entry_word = 0x2402_0001;
        let static_word = 0;
        let physical_word = 0x2404_0003;
        let install =
            bootstrap_test_install_with_additional_banks(entry_word, static_word, physical_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&entry_word.to_be_bytes());
        rom[0x24..0x28].copy_from_slice(&static_word.to_be_bytes());
        rom[0x28..0x2c].copy_from_slice(&physical_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        for (rom_start, vram_start) in [
            (0x20, 0x8000_7000),
            (0x24, 0x8000_8000),
            (0x28, 0x8000_9000),
        ] {
            transaction
                .publish_resident_rom_image(rom_start, vram_start, 4)
                .unwrap();
        }
        let validated = transaction.commit().unwrap();
        let thread_id = 0xb007;
        let guest_thread_handle = 0x8000_0280;

        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(GuestPc::new(0x8000_7000)),
            thread_id,
            10,
        )
        .unwrap();
        crate::set_guest_running_thread_global(0x8000_8000);
        with_host(|host| {
            host.thread_handle_vrams
                .insert(thread_id, guest_thread_handle);
        });

        let mut steps = 0;
        while !crate::is_thread_dead(thread_id) {
            assert!(
                crate::run_one_step(),
                "validated scheduler-mirror thread stalled before returning"
            );
            steps += 1;
            assert!(
                steps < 4,
                "validated scheduler-mirror thread did not return"
            );
        }

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x8004);
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u32(RdramAddr::from_offset(0x8000)) },
            guest_thread_handle
        );

        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(evidence.pending_physical_writes.is_empty());
        let journal = evidence.mutation_journal.unwrap();
        assert_eq!(journal.pending_attributed_writes, 0);
        assert!(journal.open_host_transactions.is_empty());
        assert_eq!(journal.entries.len(), 2);
        let mirror = &journal.entries[1];
        assert_eq!(
            mirror.declared_writes,
            [AttributedExecutableWriteEvidenceV1 {
                channel: WriterChannel::HostAbi,
                physical_start: 0x8000,
                physical_end: 0x8004,
            }]
        );
        assert_eq!(
            mirror.changed_ranges,
            [
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x8000,
                    physical_end: 0x8001,
                },
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x8002,
                    physical_end: 0x8004,
                },
            ]
        );
        assert_ne!(mirror.before_sha256, mirror.after_sha256);
        assert!(mirror.invalidated_generations.is_empty());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn validated_dynamic_boot_retains_input_provenance_without_static_authority() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let bootstrap = validated.receipt().evidence().clone();

        boot_thread0_validated_catalog_generation_program_with_dynamic_mapped_v1(
            validated,
            install,
            test_boot_context(GuestPc::new(0x8000_7000)),
            0xb008,
            10,
        )
        .unwrap();

        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.rom_sha256, Some(bootstrap.rom_sha256));
        assert_eq!(
            telemetry.bootstrap_receipt_sha256,
            Some(bootstrap.receipt_sha256)
        );
        assert!(telemetry.mutation_journal_root_sha256.is_some());
        assert!(telemetry.aggregates.is_empty());
        assert!(take_validated_bootstrap_writer_channel_receipt_v1().is_none());
        assert_eq!(
            begin_cpu_writer_runtime_trace_epoch_v1().unwrap_err(),
            CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert!(std::panic::catch_unwind(recompiled_program_evidence_snapshot).is_err());
        assert_eq!(canonical_block_charged_instructions_v1(), Some(0));

        crate::run_to_idle();
        assert!(crate::is_thread_dead(0xb008));
        assert_eq!(canonical_block_charged_instructions_v1(), Some(1));
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn validated_boot_rejects_a_receipt_from_another_catalog_before_installing_memory() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let receipt_install = bootstrap_test_install(expected_word);
        let different_install = bootstrap_test_install(0x2402_0002);
        let mut rom = vec![0; 0x40];
        rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(rom.clone());
        let mut transaction = receipt_install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();

        assert!(matches!(
            boot_thread0_validated_catalog_generation_program_v1(
                transaction.commit().unwrap(),
                different_install,
                test_boot_context(GuestPc::new(0x8000_7000)),
                0xb007,
                10,
            ),
            Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
                field: "resolver_install_sha256"
            })
        ));
        with_host(|host| {
            assert!(host.owned_runtime_rdram.is_none());
            assert!(host.runtime_rdram.is_null());
            assert!(host.canonical_recompiled_program.is_none());
        });
    }

    #[test]
    fn validated_boot_rejects_a_different_installed_rom_before_installing_memory() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let expected_word = 0x2402_0001;
        let install = bootstrap_test_install(expected_word);
        let mut receipt_rom = vec![0; 0x40];
        receipt_rom[0x20..0x24].copy_from_slice(&expected_word.to_be_bytes());
        let mut transaction = install
            .begin_bootstrap_import_v1(
                &receipt_rom,
                bootstrap_test_rdram_len(),
                fn64_runtime::TvType::Ntsc,
            )
            .unwrap();
        transaction
            .publish_resident_rom_image(0x20, 0x8000_7000, 4)
            .unwrap();
        let validated = transaction.commit().unwrap();
        let mut installed_rom = receipt_rom;
        installed_rom[0] = 1;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom(installed_rom);

        assert_eq!(
            boot_thread0_validated_catalog_generation_program_v1(
                validated,
                install,
                test_boot_context(GuestPc::new(0x8000_7000)),
                0xb007,
                10,
            ),
            Err(BootstrapImportErrorV1::InstalledRomMismatch)
        );
        with_host(|host| {
            assert!(host.owned_runtime_rdram.is_none());
            assert!(host.runtime_rdram.is_null());
            assert!(host.canonical_recompiled_program.is_none());
        });
    }

    #[test]
    #[should_panic(
        expected = "canonical executable backing ends at physical RDRAM 0x00007004, beyond the installed 0x100-byte allocation"
    )]
    fn canonical_install_rejects_an_allocation_shorter_than_its_static_backing() {
        set_catalog_generation_program(bootstrap_test_install(0x2402_0001), 0x100);
    }

    #[test]
    fn catalog_resolver_install_captures_pointer_free_canonical_evidence() {
        let bank = BankId::new(0xca71);
        let program = install_test_program(bank, 0x11);
        let program_identity = program.identity();
        let build_receipt = program.build_receipt();
        let hosts = HostFunctionCatalogV1::new(vec![
            (0x8000_9000, alternate_install_test_host),
            (0x8000_8000, install_test_host),
            (INSTALL_PC.get(), install_test_host),
        ])
        .unwrap();
        let dispatch = ProgramArtifactIdentity::new([0xd1; 32]);
        let install = CatalogResolverInstallV1::new(program, hosts, dispatch);

        assert_eq!(
            install.evidence(),
            &CatalogResolverInstallEvidenceV1 {
                schema: CATALOG_RESOLVER_INSTALL_SCHEMA_V2.to_string(),
                program_identity,
                entry: ExecutionKey::new(bank, INSTALL_PC),
                instruction_budget: 2,
                host_target_pcs: vec![INSTALL_PC.get(), 0x8000_8000, 0x8000_9000],
                abi_host_catalog: None,
                dispatch_artifact_identity: dispatch,
                build_receipt,
            }
        );
        assert!(!install.has_abi_host_catalog_authority());
        assert_eq!(
            install.resolve_entry(INSTALL_PC).unwrap(),
            ExecutionKey::new(bank, INSTALL_PC)
        );
        let second_word = GuestPc::new(INSTALL_PC.get() + 4);
        assert_eq!(
            install.resolve_transfer(bank, second_word).unwrap(),
            ExecutionKey::new(bank, second_word)
        );
        let CatalogCallResolutionV1::Host(resolved_host) =
            install.resolve_call(bank, INSTALL_PC).unwrap()
        else {
            panic!("host catalog must precede an overlapping guest target");
        };
        assert!(std::ptr::fn_addr_eq(
            resolved_host,
            install_test_host as RecompFunc
        ));
        assert!(matches!(
            install.resolve_call(bank, second_word),
            Ok(CatalogCallResolutionV1::Guest(key))
                if key == ExecutionKey::new(bank, second_word)
        ));
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_8000).unwrap(),
            install_test_host as RecompFunc
        ));
        assert!(install.resolve_host(0x8000_8004).is_none());
    }

    #[test]
    fn abi_issued_host_catalog_selects_callables_and_effects_privately() {
        let authority = issue_abi_host_function_catalog_v1(vec![
            AbiHostShimBindingV1 {
                target_pc: 0x8000_9000,
                shim: AbiHostShimV1::OsRecvMesg,
            },
            AbiHostShimBindingV1 {
                target_pc: 0x8000_8000,
                shim: AbiHostShimV1::OsSiDeviceBusy,
            },
        ])
        .unwrap();
        assert!(authority.has_valid_evidence_hash());
        assert_eq!(
            authority.evidence().bindings,
            vec![
                AbiHostShimBindingEvidenceV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsSiDeviceBusy,
                    writer_effects: vec![WriterChannel::HostAbi],
                },
                AbiHostShimBindingEvidenceV1 {
                    target_pc: 0x8000_9000,
                    shim: AbiHostShimV1::OsRecvMesg,
                    writer_effects: vec![
                        WriterChannel::CpuInstructionStore,
                        WriterChannel::PiDma,
                        WriterChannel::SiDma,
                        WriterChannel::SpDma,
                        WriterChannel::RspExecutionOrHleWriteback,
                        WriterChannel::RdpRenderer,
                        WriterChannel::HostAbi,
                    ],
                },
            ]
        );

        let install = CatalogResolverInstallV1::new_with_abi_host_catalog(
            install_test_program(BankId::new(0xca74), 0x44),
            authority,
            ProgramArtifactIdentity::new([0xd4; 32]),
        );
        assert!(install.has_abi_host_catalog_authority());
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_8000).unwrap(),
            os_si_device_busy as RecompFunc,
        ));
        assert!(std::ptr::fn_addr_eq(
            install.resolve_host(0x8000_9000).unwrap(),
            os_recv_mesg as RecompFunc,
        ));
    }

    #[test]
    fn abi_issued_host_catalog_rejects_invalid_target_geometry() {
        assert!(matches!(
            issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
                target_pc: 0x8000_8002,
                shim: AbiHostShimV1::OsRecvMesg,
            }]),
            Err(AbiHostFunctionCatalogErrorV1::MisalignedTarget {
                target: 0x8000_8002
            })
        ));
        assert!(matches!(
            issue_abi_host_function_catalog_v1(vec![
                AbiHostShimBindingV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsRecvMesg,
                },
                AbiHostShimBindingV1 {
                    target_pc: 0x8000_8000,
                    shim: AbiHostShimV1::OsSiDeviceBusy,
                },
            ]),
            Err(AbiHostFunctionCatalogErrorV1::DuplicateTarget {
                target: 0x8000_8000
            })
        ));
    }

    #[test]
    fn abi_host_semantic_receipt_changes_resolver_and_writer_model_identity() {
        let bank = BankId::new(0xca75);
        let dispatch = ProgramArtifactIdentity::new([0xd5; 32]);
        let arbitrary = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x55),
            HostFunctionCatalogV1::new(vec![(0x8000_8000, os_si_device_busy)]).unwrap(),
            dispatch,
        );
        let authority = issue_abi_host_function_catalog_v1(vec![AbiHostShimBindingV1 {
            target_pc: 0x8000_8000,
            shim: AbiHostShimV1::OsSiDeviceBusy,
        }])
        .unwrap();
        let authoritative = CatalogResolverInstallV1::new_with_abi_host_catalog(
            install_test_program(bank, 0x55),
            authority,
            dispatch,
        );
        assert_ne!(
            resolver_install_definition_sha256(&arbitrary),
            resolver_install_definition_sha256(&authoritative),
        );
        assert_ne!(
            canonical_writer_program_model_sha256(&arbitrary, None, &[]),
            canonical_writer_program_model_sha256(&authoritative, None, &[]),
        );
    }

    #[test]
    fn catalog_resolver_install_exposes_only_validated_execution_controls() {
        let first = BankId::new(0xca72);
        let second = BankId::new(0xca73);
        let hosts = HostFunctionCatalogV1::new(Vec::new()).unwrap();
        let mut install = CatalogResolverInstallV1::new(
            install_test_program(first, 0x22),
            hosts,
            ProgramArtifactIdentity::new([0xd2; 32]),
        );
        let first_identity = install.evidence().program_identity;

        assert_eq!(install.entry(), ExecutionKey::new(first, INSTALL_PC));
        install.set_budget(InstructionBudget::new(7).unwrap());
        assert_eq!(install.budget().get(), 7);
        assert_eq!(install.evidence().instruction_budget, 7);
        assert!(install
            .set_entry(ExecutionKey::new(first, GuestPc::new(INSTALL_PC.get() + 8)))
            .is_err());
        assert_eq!(install.entry(), ExecutionKey::new(first, INSTALL_PC));
        let second_word = ExecutionKey::new(first, GuestPc::new(INSTALL_PC.get() + 4));
        install.set_entry(second_word).unwrap();
        assert_eq!(install.evidence().entry, second_word);
        install
            .set_entry(ExecutionKey::new(first, INSTALL_PC))
            .unwrap();

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        assert_eq!(install.run(&mut ctx, &mut mem).instructions, 1);
        assert_eq!(
            install
                .dispatch_exposing_exceptions_at(install.entry(), &mut ctx, &mut mem)
                .unwrap()
                .exit,
            BlockExit::Yield(install.entry())
        );
        assert_eq!(
            install.program_evidence().identity,
            install.evidence().program_identity
        );
        assert_eq!(install.copy_execution_destinations().len(), 2);

        install.replace_program(install_test_program(second, 0x33));
        assert_eq!(install.entry(), ExecutionKey::new(second, INSTALL_PC));
        assert_eq!(install.budget().get(), 2);
        assert_ne!(install.evidence().program_identity, first_identity);
        assert!(install.evidence().host_target_pcs.is_empty());
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_is_one_shot_and_restores_static_budget() {
        let bank = BankId::new(0xca7b);
        let selected = ExecutionKey::new(bank, INSTALL_PC);
        let neighbor = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 4));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    exact_withhold_normal_budget_runner,
                    ProgramArtifactIdentity::new([0x7b; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(program, selected, InstructionBudget::new(8).unwrap())
                .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdb; 32]),
        );
        let identity = install.evidence().program_identity;
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        let mut storage = vec![0; 0x8000];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.exit, BlockExit::Yield(neighbor));
        assert_eq!(run.instructions, 2);
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
        assert_eq!(live.install.evidence().program_identity, identity);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 1);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries,
            vec![DynamicMappedEntryCountV1 {
                attempted_entry: selected,
                activations: 1,
                charged_instructions: 1,
                unsupported_exits: 0,
            }]
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_rejects_non_entry_member() {
        let program = install_test_program(INSTALL_BANK, 0x7a);
        let install = CatalogResolverInstallV1::new(
            program,
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xda; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        let selected = ExecutionKey::new(INSTALL_BANK, GuestPc::new(INSTALL_PC.get() + 4));

        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        }))
        .expect_err("a non-entry static key was accepted for one-shot withholding");
        let failure = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(failure.contains("must select the canonical catalog entry"));
        assert!(live.dynamic_units.borrow().is_none());
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn dynamic_attempted_alias_with_zero_charge_cannot_borrow_aggregate_work() {
        let live = set_catalog_block_program(
            CatalogResolverInstallV1::new(
                install_test_program(INSTALL_BANK, 0x79),
                HostFunctionCatalogV1::new(Vec::new()).unwrap(),
                ProgramArtifactIdentity::new([0xd9; 32]),
            ),
            0x8000,
        );
        live.enable_dynamic_mapped_execution();
        let first = ExecutionKey::new(INSTALL_BANK, INSTALL_PC);
        let alias = ExecutionKey::new(INSTALL_BANK, GuestPc::new(0xa000_7000));
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, 0x7000, 0x1000_0001);
        put_physical_word(&mut storage, 0x7004, 0);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let positive = live
            .dynamic_units
            .borrow_mut()
            .as_mut()
            .unwrap()
            .activate_and_run(
                first,
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |bank| live.reserves_bank(bank),
            )
            .unwrap();
        assert_eq!(positive.run.instructions, 2);
        live.record_dynamic_execution(first, &positive);

        let zero = live
            .dynamic_units
            .borrow_mut()
            .as_mut()
            .unwrap()
            .activate_and_run(
                alias,
                InstructionBudget::new(1).unwrap(),
                &mut ctx,
                &mut mem,
                |bank| live.reserves_bank(bank),
            )
            .unwrap();
        assert_eq!(zero.identity, positive.identity);
        assert_eq!(zero.run.instructions, 0);
        live.record_dynamic_execution(alias, &zero);

        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        let [aggregate] = telemetry.aggregates.as_slice() else {
            panic!("expected one shared dynamic identity: {telemetry:?}");
        };
        assert_eq!(aggregate.charged_instructions, 2);
        let first_count = aggregate
            .attempted_entries
            .iter()
            .find(|entry| entry.attempted_entry == first)
            .unwrap();
        let alias_count = aggregate
            .attempted_entries
            .iter()
            .find(|entry| entry.attempted_entry == alias)
            .unwrap();
        assert_eq!(first_count.charged_instructions, 2);
        assert_eq!(alias_count.charged_instructions, 0);
        assert_eq!(alias_count.activations, 1);
        assert_eq!(alias_count.unsupported_exits, 0);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_exact_static_key_withhold_preserves_branch_delay_budget() {
        let bank = BankId::new(0xca7a);
        let selected = ExecutionKey::new(bank, INSTALL_PC);
        let target = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 8));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0, 0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([0x7a; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(program, selected, InstructionBudget::new(3).unwrap())
                .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xda; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution_with_exact_static_key_withheld(selected);
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, INSTALL_PC.get() & 0x1fff_ffff, 0x1000_0001);
        put_physical_word(&mut storage, (INSTALL_PC.get() & 0x1fff_ffff) + 4, 0);
        let mut mem = Rdram::new(&mut storage);

        let error = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(1).unwrap(),
            &mut RsContext::new(),
            &mut mem,
        )
        .expect_err("one instruction split a withheld branch/delay pair");
        assert!(error.contains("indivisible instruction unit"));
        assert_eq!(live.dynamic_withheld_static_key.get(), Some(selected));
        assert!(copy_dynamic_mapped_execution_telemetry_v1()
            .aggregates
            .is_empty());

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(selected),
            InstructionBudget::new(3).unwrap(),
            &mut RsContext::new(),
            &mut mem,
        )
        .unwrap();
        assert_eq!(run.exit, BlockExit::Yield(target));
        assert_eq!(run.instructions, 3);
        assert_eq!(live.dynamic_withheld_static_key.get(), None);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].attempted_entry,
            selected
        );
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].charged_instructions,
            2
        );
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0].unsupported_exits,
            0
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_budget_static_miss_dynamic_static_no_replay() {
        let bank = BankId::new(0xca7d);
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_transition_test_runner,
                    ProgramArtifactIdentity::new([0x7d; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdd; 32]),
        );
        let static_identity = install.evidence().program_identity;
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();

        let mut storage = vec![0; 0x8000];
        let jump = 0x0800_0000 | ((static_resume.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut storage, dynamic_pc.get() & 0x1fff_ffff, jump);
        put_physical_word(&mut storage, (dynamic_pc.get() & 0x1fff_ffff) + 4, 0);
        let mut mem = Rdram::new(&mut storage);

        let mut checkpoint_ctx = RsContext::new();
        let checkpoint = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(2).unwrap(),
            &mut checkpoint_ctx,
            &mut mem,
        )
        .expect("prior static work must checkpoint before a final dynamic branch/delay pair");
        assert_eq!(
            checkpoint.exit,
            BlockExit::Checkpoint(ExecutionKey::new(bank, dynamic_pc))
        );
        assert_eq!(checkpoint.instructions, 1);
        assert_eq!(checkpoint.blocks, 1);
        assert_eq!(checkpoint_ctx.r_u32(2), 1);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            1,
            "classifying the indivisible unit admits its exact fetched identity"
        );
        assert!(
            copy_dynamic_mapped_execution_telemetry_v1()
                .aggregates
                .is_empty(),
            "a rejected indivisible unit must not publish execution telemetry"
        );

        let mut ctx = RsContext::new();
        set_canonical_block_instruction_limit_v1(Some(4));
        assert_eq!(live.next_dispatch_budget().get(), 4);

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            live.next_dispatch_budget(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(bank, static_resume))
        );
        assert_eq!(run.instructions, 4);
        live.charge_canonical_instructions(run.instructions);
        assert_eq!(live.canonical_charged_instructions.get(), 4);
        let split = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = live.next_dispatch_budget();
        }))
        .expect_err("dispatch may not continue past the exact limit");
        let split = split
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| split.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(split.contains("limit 4 was already reached"));
        assert_eq!(ctx.r_u32(2), 1, "static source replayed after its miss");
        assert_eq!(
            ctx.r_u32(3),
            1,
            "one-instruction static continuation did not reach the exact ceiling"
        );
        assert_eq!(live.install.evidence().program_identity, static_identity);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            1
        );
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(telemetry.dropped_attempted_entry_activations, 0);
        assert_eq!(telemetry.aggregates[0].activations, 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.aggregates[0].unsupported_exits, 0);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries,
            vec![DynamicMappedEntryCountV1 {
                attempted_entry: ExecutionKey::new(bank, dynamic_pc),
                activations: 1,
                charged_instructions: 2,
                unsupported_exits: 0,
            }]
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_call_host_precedes_dynamic() {
        let bank = BankId::new(0xca7e);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = ExecutionKey::new(bank, GuestPc::new(INSTALL_PC.get() + 0x20));
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume.pc, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_host_precedence_runner,
                    ProgramArtifactIdentity::new([0x7e; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(host_pc.get(), install_test_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xde; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, host_pc.get() & 0x1fff_ffff, 0x2402_0063);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::HostCall {
                vram: host_pc,
                resume,
            }
        );
        assert_eq!(run.instructions, 1);
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0,
            "an exact host binding must win before dynamic admission"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_precompiled_activation_precedes_dynamic() {
        let generation_word = 0x2402_0001;
        let generation_pc = GuestPc::new(0x8000_a000);
        let generation_bank = BankId::new(0xb00a);
        let live = set_catalog_generation_program(
            bootstrap_test_install_with_generation(0, generation_word),
            0xb000,
        );
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0xb000];
        put_physical_word(&mut storage, 0xa000, generation_word);
        let mem = Rdram::new(&mut storage);

        let target =
            resolve_unified_catalog_target(&live, INSTALL_BANK, generation_pc, &mem).unwrap();

        assert_eq!(
            target,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(generation_bank, generation_pc))
        );
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0,
            "a digest-matched precompiled generation must win before dynamic admission"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_dynamic_fetch_fault_preserves_prior_work() {
        let bank = BankId::new(0xca7f);
        let target_pc = GuestPc::new(0x0040_0000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, INSTALL_PC, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_tlb_fault_runner,
                    ProgramArtifactIdentity::new([0x7f; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xdf; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        ctx.initialize_invalid_tlb_entries();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.instructions, 2, "source work plus faulting fetch");
        assert_eq!(ctx.r_u32(2), 1, "static source must not replay");
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { pc, .. },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbRefillLoad,
                    epc,
                    branch_delay: false,
                    bad_vaddr: Some(0x0040_0000),
                    ..
                },
            }) if pc == target_pc && epc == target_pc
        ));
        assert_eq!(
            live.dynamic_units
                .borrow()
                .as_ref()
                .expect("dynamic catalog remains installed")
                .admitted_len(),
            0
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_delay_store_refetches_dynamic_target() {
        let bank = BankId::new(0xca80);
        let branch_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let target_pc = GuestPc::new(branch_pc.get() + 0x0c);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x40);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_dynamic_writer_runner,
                    ProgramArtifactIdentity::new([0x80; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe0; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, branch_pc.get() & 0x1fff_ffff, 0x1000_0002);
        put_physical_word(
            &mut storage,
            (branch_pc.get() & 0x1fff_ffff) + 4,
            0xac88_0000,
        );
        put_physical_word(&mut storage, target_pc.get() & 0x1fff_ffff, 0x2442_0001);
        put_physical_word(&mut storage, (target_pc.get() & 0x1fff_ffff) + 4, 0);
        let replacement_jump = 0x0800_0000 | ((static_resume.get() >> 2) & 0x03ff_ffff);
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0xffff_ffff_0000_0000 | u64::from(target_pc.get()));
        ctx.set_r(8, u64::from(replacement_jump));

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(bank, static_resume))
        );
        assert_eq!(run.instructions, 6);
        assert_eq!(ctx.r_u32(2), 0, "stale target instruction executed");
        assert_eq!(ctx.r_u32(3), 1);
        assert_eq!(
            mem.load_w(0xffff_ffff_0000_0000 | u64::from(target_pc.get())) as u32,
            replacement_jump
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_unsupported_dynamic_word_is_loud_with_prior_count() {
        let bank = BankId::new(0xca81);
        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let static_resume = GuestPc::new(INSTALL_PC.get() + 0x20);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, static_resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    unified_transition_test_runner,
                    ProgramArtifactIdentity::new([0x81; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe1; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        put_physical_word(&mut storage, dynamic_pc.get() & 0x1fff_ffff, 0x4800_0000); // mfc2 zero,cop2r0
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RsContext::new();

        let run = dispatch_unified_catalog_slice(
            &live,
            UnifiedCatalogTargetV1::Static(ExecutionKey::new(bank, INSTALL_PC)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();

        assert_eq!(run.instructions, 1, "only the static source retired");
        assert_eq!(ctx.r_u32(2), 1, "static source replayed after its miss");
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { pc, .. },
                kind: CpuFaultKind::UnsupportedInstruction { word: 0x4800_0000 },
            }) if pc == dynamic_pc
        ));
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.dropped_identity_unsupported_exits, 0);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 0);
        assert_eq!(telemetry.aggregates[0].unsupported_exits, 1);
        assert_eq!(telemetry.aggregates[0].attempted_entries.len(), 1);
        assert_eq!(
            telemetry.aggregates[0].attempted_entries[0],
            DynamicMappedEntryCountV1 {
                attempted_entry: ExecutionKey::new(bank, dynamic_pc),
                activations: 1,
                charged_instructions: 0,
                unsupported_exits: 1,
            }
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_unified_entry_ambiguity_cannot_fall_back_or_mint_static_evidence() {
        let first = BankId::new(0xca82);
        let second = BankId::new(0xca83);
        let mut program = BlockProgram::new();
        for (bank, artifact) in [(first, 0x82), (second, 0x83)] {
            program
                .register(
                    CodeBank::new(bank, INSTALL_PC, vec![0]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        install_test_runner,
                        ProgramArtifactIdentity::new([artifact; 32]),
                    ),
                )
                .unwrap();
        }
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(first, INSTALL_PC),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe2; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();
        let mut storage = vec![0; 0x8000];
        let mem = Rdram::new(&mut storage);

        let error = resolve_unified_catalog_entry(&live, INSTALL_PC, &mem).unwrap_err();
        assert!(
            error.contains("ambiguous"),
            "bankless entry ambiguity was hidden by dynamic fallback: {error}"
        );
        let evidence = std::panic::catch_unwind(recompiled_program_evidence_snapshot);
        assert!(
            evidence.is_err(),
            "dynamic execution exposed an incomplete static program-evidence snapshot"
        );
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_install_rejects_all_static_writer_authority_paths() {
        let bank = BankId::new(0xca84);
        let install = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x84),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xe4; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        live.enable_dynamic_mapped_execution();

        assert_eq!(
            live.mint_bootstrap_writer_completion(&[]).unwrap_err(),
            BootstrapWriterChannelCompletionErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_cpu_writer_runtime_trace_epoch().unwrap_err(),
            CpuWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_host_abi_writer_runtime_trace_epoch()
                .unwrap_err(),
            HostAbiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_rsp_writer_runtime_trace_epoch().unwrap_err(),
            RspWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_rdp_renderer_writer_runtime_trace_epoch()
                .unwrap_err(),
            RdpRendererWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_pi_writer_runtime_trace_epoch(false, false, false)
                .unwrap_err(),
            PiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.take_si_writer_runtime_state(&[], false, &[], false, false)
                .unwrap_err(),
            SiWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
        assert_eq!(
            live.begin_sp_writer_runtime_trace_epoch().unwrap_err(),
            SpWriterRuntimeStateErrorV1::DynamicExecutionInstalled
        );
    }

    #[test]
    fn catalog_resolver_install_preserves_fail_closed_static_resolution() {
        let first = BankId::new(0xca74);
        let second = BankId::new(0xca75);
        let unique = BankId::new(0xca76);
        let unique_pc = GuestPc::new(0x8000_a000);
        let mut program = BlockProgram::new();
        for (bank, pc, artifact_byte) in [
            (second, INSTALL_PC, 0x42),
            (first, INSTALL_PC, 0x41),
            (unique, unique_pc, 0x43),
        ] {
            program
                .register(
                    CodeBank::new(bank, pc, vec![0]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        install_test_runner,
                        ProgramArtifactIdentity::new([artifact_byte; 32]),
                    ),
                )
                .unwrap();
        }
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(first, INSTALL_PC),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd4; 32]),
        );
        let evidence = install.evidence().clone();

        assert!(matches!(
            install.resolve_entry(INSTALL_PC),
            Err(CpuFault {
                kind: CpuFaultKind::AmbiguousPc {
                    first_candidate,
                    second_candidate,
                    candidate_count: 2,
                },
                ..
            }) if first_candidate == first && second_candidate == second
        ));
        assert_eq!(
            install.resolve_transfer(second, INSTALL_PC).unwrap(),
            ExecutionKey::new(second, INSTALL_PC)
        );
        assert_eq!(
            install.resolve_transfer(first, unique_pc).unwrap(),
            ExecutionKey::new(unique, unique_pc)
        );

        let sparse_hole = GuestPc::new(INSTALL_PC.get() + 4);
        assert!(matches!(
            install.resolve_transfer(first, sparse_hole),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        let misaligned = GuestPc::new(INSTALL_PC.get() + 2);
        assert!(matches!(
            install.resolve_entry(misaligned),
            Err(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    ..
                },
                ..
            })
        ));

        let previous = fn64_recomp_rs::set_host_lookup(Some(install_test_legacy_host_lookup));
        assert!(matches!(
            install.resolve_call(first, sparse_hole),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        fn64_recomp_rs::set_host_lookup(previous);
        assert_eq!(install.evidence(), &evidence);
    }

    #[test]
    fn only_canonical_install_populates_catalog_evidence_and_legacy_clears_it() {
        let bank = BankId::new(0xca77);
        let install = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x61),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd5; 32]),
        );
        let expected = install.evidence().clone();
        set_catalog_block_program(install, 0x7008);
        assert_eq!(
            catalog_resolver_install_evidence_snapshot(),
            Some(expected.clone())
        );
        assert!(matches!(
            recompiled_program_evidence_snapshot(),
            Some(RecompiledProgramEvidenceSnapshot::Block {
                dispatch_artifact_identity,
                instruction_budget: 2,
                ref executable_regions,
                ref pending_executable_writes,
                ..
            }) if dispatch_artifact_identity == expected.dispatch_artifact_identity
                && executable_regions.is_empty()
                && pending_executable_writes.is_empty()
        ));

        set_entry_lookup(install_test_function_lookup, 0x100);
        assert_eq!(catalog_resolver_install_evidence_snapshot(), None);

        let second = CatalogResolverInstallV1::new(
            install_test_program(bank, 0x62),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xd6; 32]),
        );
        set_catalog_block_program(second, 0x7008);
        let legacy = LiveBlockProgram {
            program: Rc::new(RefCell::new(BlockProgram::new())),
            entry_lookup: install_test_entry_lookup,
            transfer_lookup: install_test_transfer_lookup,
            budget: InstructionBudget::new(2).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(legacy, 0x100);
        assert_eq!(catalog_resolver_install_evidence_snapshot(), None);
    }

    #[test]
    fn catalog_resolver_feature_predicate_is_only_lane_eligibility() {
        let eligible = StaticExecutionBuildReceipt {
            schema: 1,
            aot_runtime: true,
            production_aot: true,
            dev_interpreter: false,
        };
        assert!(catalog_resolver_feature_lane_eligible(eligible));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                production_aot: false,
                ..eligible
            }
        ));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                aot_runtime: false,
                ..eligible
            }
        ));
        assert!(!catalog_resolver_feature_lane_eligible(
            StaticExecutionBuildReceipt {
                dev_interpreter: true,
                ..eligible
            }
        ));
    }

    #[test]
    #[should_panic(expected = "catalog does not match the live BlockProgram")]
    fn installing_generation_catalog_rejects_a_missing_shard_bank() {
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(BlockProgram::new())),
            entry_lookup: live_entry_lookup,
            transfer_lookup: live_transfer_lookup,
            budget: InstructionBudget::new(2).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(live, 0x100);
        let start = GuestPc::new(0x8000_0100);
        let end = GuestPc::new(start.get() + 4);
        let bank = BankId::new(0xBAD);
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(1),
                    start,
                    end,
                    start,
                    end,
                    [0; 32],
                    vec![PrecompiledShard::new(bank, start, end).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();

        install_precompiled_generation_catalog(catalog);
    }

    fn test_boot_context(entry: GuestPc) -> BootContext {
        if with_host(|host| host.device_fabric.tv_type()).is_none() {
            crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        }
        if with_host(|host| host.installed_rom).is_none() {
            crate::load_rom(vec![0]);
        }
        let rom_sha256 = with_host(|host| {
            host.installed_rom
                .expect("test ROM was installed above")
                .sha256
        });
        let mut gprs = [0u64; 32];
        gprs[31] = u64::from(THREAD_RETURN_SENTINEL);
        let (hi, lo) = if entry == LIVE_ENTRY {
            gprs[20] = 0xffff_ffff_cafe_babe;
            (0x1234, 0x5678)
        } else {
            (0, 0)
        };
        let mut cp0 = [0u64; 32];
        cp0[1] = 31;
        BootContext {
            schema: BOOT_CONTEXT_SCHEMA_V1.to_string(),
            producer: "fn64-abi synthetic block test".to_string(),
            normalized_rom_sha256: Sha256Digest::from_bytes(rom_sha256),
            cic: BootCicIdentity {
                ipl3_sha256: Sha256Digest::from_bytes([0; 32]),
            },
            region: BootRegion {
                destination_code: b'E',
                tv_standard: BootTvStandard::Ntsc,
            },
            entry_pc: entry.get(),
            gprs,
            hi,
            lo,
            cp0: BootCop0Context { registers: cp0 },
        }
    }

    #[test]
    fn catalog_boot_context_is_checked_before_unified_dispatch() {
        let entry = ExecutionKey::new(INSTALL_BANK, INSTALL_PC);
        let boot_context = test_boot_context(INSTALL_PC);
        let mut ctx = RsContext::new();
        ctx.restore_boot_context(&boot_context).unwrap();
        validate_restored_catalog_boot_context(entry, &boot_context, &ctx);

        ctx.set_r32(20, 1);
        let state_failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate_restored_catalog_boot_context(entry, &boot_context, &ctx);
        }))
        .expect_err("a mismatched restored boot register reached unified dispatch");
        let state_failure = state_failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| state_failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(state_failure.contains("before first unified dispatch"));

        let entry_failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            validate_restored_catalog_boot_context(
                ExecutionKey::new(INSTALL_BANK, GuestPc::new(INSTALL_PC.get() + 4)),
                &boot_context,
                &RsContext::new(),
            );
        }))
        .expect_err("a non-BootContext entry reached first unified dispatch");
        let entry_failure = entry_failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| entry_failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(entry_failure.contains("dispatch entry differs"));
    }

    const LIVE_BANK: BankId = BankId::new(0xA11CE);
    const LIVE_SECOND_BANK: BankId = BankId::new(0xA11CF);
    const LIVE_ENTRY: GuestPc = GuestPc::new(0x8000_1000);
    const LIVE_NEXT: GuestPc = GuestPc::new(0x8000_1004);
    const LIVE_HOST: GuestPc = GuestPc::new(0x8000_2000);
    const ORDERED_WRITER_BANK: BankId = BankId::new(0x0ade_0001);
    const ORDERED_WRITER_ENTRY: GuestPc = GuestPc::new(0x8000_7000);
    const ORDERED_WRITER_RESUME: GuestPc = GuestPc::new(0x8000_7004);
    const ORDERED_WRITER_HOST: GuestPc = GuestPc::new(0x8000_7100);
    const ORDERED_SYNC_BANK: BankId = BankId::new(0x0ade_0002);
    const ORDERED_SYNC_ENTRY: GuestPc = GuestPc::new(0x8000_7200);
    const ORDERED_SYNC_RESUME: GuestPc = GuestPc::new(0x8000_7204);
    const ORDERED_SYNC_HOST: GuestPc = GuestPc::new(0x8000_7300);
    const CATALOG_REWRITE_ENTRY: GuestPc = GuestPc::new(0x8000_6000);
    const CATALOG_REWRITE_A: BankId = BankId::new(0xca80);
    const CATALOG_REWRITE_B: BankId = BankId::new(0xca81);
    const PREPARED_STATIC_BANK: BankId = BankId::new(0xca90);
    const PREPARED_GENERATION_BANK: BankId = BankId::new(0xca91);
    const PREPARED_STATIC_ENTRY: GuestPc = GuestPc::new(0x8000_5000);
    const PREPARED_GENERATION_ENTRY: GuestPc = GuestPc::new(0x8000_6000);
    const IRQ_BANK: BankId = BankId::new(0x1A2);
    const IRQ_ENTRY: GuestPc = GuestPc::new(0x8000_0100);
    const IRQ_RESUME: GuestPc = GuestPc::new(0x8000_0104);
    const IRQ_VECTOR: GuestPc = GuestPc::new(0x8000_0180);
    const TIMER_BANK: BankId = BankId::new(0x1A7);
    const REWRITE_OLD_BANK: BankId = BankId::new(0xC0DE_0000);
    const REWRITE_NEW_BANK: BankId = BankId::new(0xC0DE_0001);
    const REWRITE_ENTRY: GuestPc = GuestPc::new(0x8000_3000);
    const REWRITE_RESUME: GuestPc = GuestPc::new(REWRITE_ENTRY.get() + 0x24);
    const REWRITE_PHYSICAL: u32 = 0x80;
    const REWRITE_A_WORDS: [u32; 13] = [
        0x3c09_8000, // lui t1, 0x8000
        0x240c_0055, // addiu t4, zero, 0x55
        0xad2c_0020, // sw t4, 0x20(t1) -- non-executable store
        0x240d_0066, // addiu t5, zero, 0x66
        0xad2d_0024, // sw t5, 0x24(t1) -- proves ordinary stores do not split
        0x240a_0001, // addiu t2, zero, 1 -- prepare the post-store sentinel
        0x3c08_1122, // lui t0, 0x1122
        0x3508_3344, // ori t0, t0, 0x3344
        0xad28_0080, // sw t0, 0x80(t1) -- replaces this executable image
        0xad2a_0010, // generation-A post-store sentinel
        0x03e0_0008, // jr ra
        0,
        0,
    ];
    const REWRITE_B_WORDS: [u32; 13] = [
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0x240b_0002, // addiu t3, zero, 2
        0xad2b_0014, // sw t3, 0x14(t1)
        0x03e0_0008, // jr ra
        0,
    ];
    const DMA_OLD_BANK: BankId = BankId::new(0xD00D_0000);
    const DMA_NEW_BANK: BankId = BankId::new(0xD00D_0001);
    const DMA_ENTRY: GuestPc = GuestPc::new(0x8000_4000);
    const DMA_PHYSICAL: u32 = 0x100;

    thread_local! {
        static LIVE_ACTIVE_BANK: std::cell::Cell<BankId> = const { std::cell::Cell::new(LIVE_BANK) };
        static REWRITE_BUILDS: std::cell::RefCell<Vec<(u64, Vec<u8>)>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static REWRITE_B_ENTRIES: std::cell::RefCell<Vec<ExecutionKey>> = const {
            std::cell::RefCell::new(Vec::new())
        };
        static BOOT_FPCSR_OBSERVATIONS: std::cell::RefCell<Vec<u32>> = const {
            std::cell::RefCell::new(Vec::new())
        };
    }

    fn evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn alternate_evidence_callable(_ctx: &mut RsContext, _mem: &mut Rdram<'_>) {}

    fn evidence_lookup(_vram: u32) -> RecompFunc {
        evidence_callable
    }

    fn alternate_evidence_lookup(_vram: u32) -> RecompFunc {
        alternate_evidence_callable
    }

    fn observe_thread0_fpcsr_boot(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
        os_initialize(ctx, mem);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().push(ctx.read_fcr(31)));
    }

    fn unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("evidence-only builder must not run".to_string())
    }

    fn alternate_unused_evidence_builder(
        _bytes: &[u8],
        _generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        Err("alternate evidence-only builder must not run".to_string())
    }

    fn install_evidence_block_lane(budget: u32, reverse_regions: bool, alternate_builders: bool) {
        let first_bank = BankId::new(0xE100);
        let second_bank = BankId::new(0xE200);
        let first_start = GuestPc::new(0x8000_5000);
        let second_start = GuestPc::new(0x8000_6000);
        let mut program = BlockProgram::new();
        let mut first_region =
            ExecutableRegion::new(first_start, GuestPc::new(first_start.get() + 4));
        let mut second_region =
            ExecutableRegion::new(second_start, GuestPc::new(second_start.get() + 4));
        let runner_artifact = ProgramArtifactIdentity::new([0xE5; 32]);
        first_region
            .install(
                &mut program,
                CodeBank::new(first_bank, first_start, vec![0x1111_2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        second_region
            .install(
                &mut program,
                CodeBank::new(second_bank, second_start, vec![0x3333_4444]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    live_test_runner,
                    runner_artifact,
                ),
            )
            .unwrap();
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(program)),
            entry_lookup: live_entry_lookup,
            transfer_lookup: live_transfer_lookup,
            budget: InstructionBudget::new(budget).unwrap(),
            dispatch_artifact_identity: Some(ProgramArtifactIdentity::new([0xD1; 32])),
            executable_regions: Rc::new(RefCell::new(Vec::new())),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };
        set_block_program(live, 0x100);
        let first_builder = if alternate_builders {
            alternate_unused_evidence_builder
        } else {
            unused_evidence_builder
        };
        let registrations = [
            (0x20, 0x24, first_region, first_builder),
            (0x40, 0x44, second_region, unused_evidence_builder),
        ];
        if reverse_regions {
            for (start, end, region, builder) in registrations.into_iter().rev() {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        } else {
            for (start, end, region, builder) in registrations {
                register_live_executable_region_with_artifact_identity(
                    start,
                    end,
                    region,
                    builder,
                    ProgramArtifactIdentity::new([0xB1; 32]),
                );
            }
        }
    }

    #[test]
    fn translated_cpu_unsupported_gap_records_the_typed_release_event() {
        fn64_runtime::arm_unsupported_events(None).unwrap();
        record_recompiled_unsupported("unsupported COP0 register 7");

        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Recompiler
        );
        assert_eq!(
            events[0].operation,
            concat!("recompiler.cpu.", "unsupported-instruction")
        );
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        assert!(events[0].guest_cycle.is_some());
    }

    #[test]
    fn function_lane_evidence_requires_identity_and_excludes_callable_pointers() {
        with_host(|host| *host = super::super::HostState::default());
        set_entry_lookup(evidence_lookup, 0x100);
        let missing = std::panic::catch_unwind(recompiled_program_evidence_snapshot)
            .expect_err("unidentified function lane must fail evidence capture");
        let message = missing
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| missing.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("stable host-provided artifact identity"));

        let identity = ProgramArtifactIdentity::new([0xA5; 32]);
        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let first = recompiled_program_evidence_snapshot().unwrap();
        set_entry_lookup_with_artifact_identity(alternate_evidence_lookup, 0x100, identity);
        assert_eq!(first, recompiled_program_evidence_snapshot().unwrap());

        set_entry_lookup_with_artifact_identity(
            evidence_lookup,
            0x100,
            ProgramArtifactIdentity::new([0x5A; 32]),
        );
        assert_ne!(first, recompiled_program_evidence_snapshot().unwrap());
    }

    #[test]
    fn function_destination_history_binds_artifact_function_cycle_and_schema() {
        with_host(|host| *host = super::super::HostState::default());
        let identity = ProgramArtifactIdentity::new([0xC3; 32]);
        set_entry_lookup_with_execution_observation(
            evidence_lookup,
            0x100,
            identity,
            fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
        );
        with_executor(|executor| executor.set_sim_time(37));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_1000,
            "entry",
        ));
        with_executor(|executor| executor.set_sim_time(41));
        fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
            0x8000_2000,
            "callee",
        ));

        assert_eq!(
            copy_function_execution_destinations(),
            vec![
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(37),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_1000, "entry"),
                },
                FunctionExecutionDestinationObservation {
                    at: fn64_runtime::Cycles::new(41),
                    artifact_identity: identity,
                    function: TranslatedFunctionIdentity::new(0x8000_2000, "callee"),
                },
            ]
        );

        set_entry_lookup_with_artifact_identity(evidence_lookup, 0x100, identity);
        let stale = std::panic::catch_unwind(copy_function_execution_destinations)
            .expect_err("identity-only function install must not claim a complete history");
        let message = stale
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| stale.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(message.contains("entry-observation schema"));
    }

    #[test]
    fn block_lane_evidence_sorts_regions_and_excludes_builder_pointers() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x42, 2), (0x20, 2), (0x21, 3)]);
        let forward = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(8, true, true);
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0x21, 3), (0x42, 2), (0x20, 2)]);
        let reverse = recompiled_program_evidence_snapshot().unwrap();
        assert_eq!(forward, reverse);

        let RecompiledProgramEvidenceSnapshot::Block {
            instruction_budget,
            executable_regions,
            pending_executable_writes,
            ..
        } = forward
        else {
            panic!("block install captured as function lane")
        };
        assert_eq!(instruction_budget, 8);
        assert_eq!(
            executable_regions
                .iter()
                .map(|region| region.physical_start)
                .collect::<Vec<_>>(),
            vec![0x20, 0x40]
        );
        assert_eq!(
            pending_executable_writes,
            vec![
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x20,
                    physical_end: 0x24,
                },
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x42,
                    physical_end: 0x44,
                },
            ]
        );
    }

    #[test]
    fn block_destination_copy_api_reads_the_live_program_history() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        assert!(copy_block_execution_destinations().is_empty());
        let live = with_host(|host| {
            host.recompiled_program
                .clone()
                .expect("evidence fixture installs a live block program")
        });
        let entry = ExecutionKey::new(BankId::new(0xE100), GuestPc::new(0x8000_5000));
        let mut bytes = [0u8; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        let run = live.program.borrow().run(
            entry,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(run.instructions, 0);
        assert!(matches!(run.exit, BlockExit::Fault(_)));
        assert_eq!(
            copy_block_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity: Some(ProgramArtifactIdentity::new([0xE5; 32])),
                instructions: 0,
            }]
        );
    }

    #[test]
    fn block_lane_evidence_binds_budget_region_generation_and_pending_writes() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        let baseline = recompiled_program_evidence_snapshot().unwrap();

        install_evidence_block_lane(12, false, false);
        let changed_budget = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_budget);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].next_generation = 2;
        let changed_generation = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_generation);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        {
            let mut regions = live.executable_regions.borrow_mut();
            regions[0].physical_start += 4;
            regions[0].physical_end += 4;
        }
        let changed_region_geometry = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_region_geometry);

        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = Some(ProgramArtifactIdentity::new([0xD2; 32]));
        });
        let changed_dispatch_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_dispatch_artifact);

        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity =
            Some(ProgramArtifactIdentity::new([0xB2; 32]));
        let changed_builder_artifact = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_builder_artifact);

        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 4)));
        let changed_pending = recompiled_program_evidence_snapshot().unwrap();
        assert_ne!(baseline, changed_pending);
    }

    #[test]
    #[should_panic(expected = "pending executable write has zero length")]
    fn block_lane_evidence_never_omits_malformed_pending_write() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x30, 0)));
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    #[should_panic(expected = "stable host-provided dispatch artifact identity")]
    fn block_lane_evidence_rejects_unidentified_dispatch_artifact() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        with_host(|host| {
            host.recompiled_program
                .as_mut()
                .unwrap()
                .dispatch_artifact_identity = None;
        });
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    #[should_panic(expected = "stable host-provided builder artifact identity")]
    fn block_lane_evidence_rejects_unidentified_builder_artifact() {
        with_host(|host| *host = super::super::HostState::default());
        install_evidence_block_lane(8, false, false);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        live.executable_regions.borrow_mut()[0].builder_artifact_identity = None;
        let _ = recompiled_program_evidence_snapshot();
    }

    #[test]
    fn verified_host_write_preflight_rejects_only_executable_overlap() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x100, 0x180)], Vec::new());

        assert_eq!(
            preflight_non_executable_host_writes(&[(0x80, 0x100)]),
            Ok(())
        );
        let overlap = preflight_non_executable_host_writes(&[(0x17f, 0x181)]).unwrap_err();
        assert!(overlap.contains("overlaps live executable region"));
        assert!(overlap.contains("transactional executable publication is unavailable"));

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().push((0x120, 4)));
        let pending = preflight_non_executable_host_writes(&[]).unwrap_err();
        assert!(pending.contains("pending host write"));
    }

    #[test]
    fn executable_write_preflight_test_scope_restores_on_unwind() {
        let _outer =
            scoped_test_executable_write_preflight_state(vec![(0x20, 0x40)], vec![(0x24, 4)]);

        let panic = std::panic::catch_unwind(|| {
            let _inner = scoped_test_executable_write_preflight_state(
                vec![(0x100, 0x180)],
                vec![(0x120, 8)],
            );
            assert_eq!(
                EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
                vec![(0x100, 0x180)]
            );
            assert_eq!(
                PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
                vec![(0x120, 8)]
            );
            panic!("expected test-scope unwind");
        });

        assert!(panic.is_err());
        assert_eq!(
            EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()),
            vec![(0x20, 0x40)]
        );
        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x24, 4)]
        );
    }

    #[test]
    fn typed_halfword_write_multiplexes_invalidation_and_renderer_once() {
        use std::{cell::Cell, rc::Rc};

        struct CountBackend(Rc<Cell<u32>>);

        impl fn64_render::RenderBackend for CountBackend {
            fn create(
                &mut self,
                _cfg: &fn64_render::RenderConfig,
            ) -> Result<(), fn64_render::RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                self.0.set(self.0.get() + 1);
                fn64_render::NonRdpWrite16Disposition::AppliedHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
                Ok(fn64_render::FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), fn64_render::RenderError> {
                Ok(())
            }

            fn resize(&mut self, _width: u32, _height: u32) {}

            fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
                &[]
            }
        }

        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let renderer_calls = Rc::new(Cell::new(0));
        crate::set_render_backend(Box::new(CountBackend(renderer_calls.clone())), 0x100);
        let previous =
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut bytes = [0u8; 0x100];
        Rdram::new(&mut bytes).store_h(0xffff_ffff_a000_0040, 0x1235);
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(0x40, 2)]
        );
        assert_eq!(renderer_calls.get(), 1);
        PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
    }

    fn unmapped(bank: BankId, pc: GuestPc, start: GuestPc, end: GuestPc) -> CpuFault {
        CpuFault {
            at: ExecutionKey::new(bank, pc),
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start.get(),
                bank_end: end.get(),
            },
        }
    }

    fn rewrite_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            REWRITE_OLD_BANK,
            pc,
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + 0x34),
        ))
    }

    fn rewrite_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        rewrite_lookup(pc)
    }

    fn rewrite_interpreter_runner(
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let words = match entry.bank {
            REWRITE_OLD_BANK => REWRITE_A_WORDS,
            REWRITE_NEW_BANK => {
                REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().push(entry));
                REWRITE_B_WORDS
            }
            bank => {
                return BlockRun::new(
                    BlockExit::Fault(unmapped(
                        bank,
                        entry.pc,
                        REWRITE_ENTRY,
                        GuestPc::new(REWRITE_ENTRY.get() + 0x34),
                    )),
                    0,
                );
            }
        };
        let mut catalog = CodeCatalog::new();
        catalog
            .register(CodeBank::new(entry.bank, REWRITE_ENTRY, words.to_vec()).unwrap())
            .unwrap();
        let run =
            run_bank(&catalog, entry.bank, entry, budget, ctx, mem).unwrap_or_else(|unsupported| {
                panic!("rewrite interpreter hit unsupported op: {unsupported:?}")
            });
        match run.exit {
            BlockExit::ResolveTransfer { target_pc, .. }
                if ctx.is_thread_return(target_pc.get()) =>
            {
                BlockRun::new(BlockExit::ThreadReturn, run.instructions)
            }
            _ => run,
        }
    }

    fn rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        let expected = std::iter::once(0x1122_3344)
            .chain(REWRITE_A_WORDS.into_iter().skip(1))
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        if generation != 1 || bytes != expected {
            return Err(format!(
                "unexpected CPU rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(REWRITE_NEW_BANK, REWRITE_ENTRY, REWRITE_B_WORDS.to_vec())
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(REWRITE_NEW_BANK, rewrite_interpreter_runner),
        ))
    }

    fn dma_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        Err(unmapped(
            DMA_OLD_BANK,
            pc,
            DMA_ENTRY,
            GuestPc::new(DMA_ENTRY.get() + 8),
        ))
    }

    fn dma_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        dma_lookup(pc)
    }

    fn dma_rewrite_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match (entry.bank, entry.pc) {
            (DMA_OLD_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_A460_0000, DMA_PHYSICAL);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
                mem.store_w(0xFFFF_FFFF_A460_0008, 7);
                BlockRun::new(BlockExit::Checkpoint(entry), 5)
            }
            (DMA_NEW_BANK, DMA_ENTRY) => {
                mem.store_w(0xFFFF_FFFF_8000_0014, 0xD00D_0001);
                BlockRun::new(
                    BlockExit::Transfer(ExecutionKey::new(
                        DMA_NEW_BANK,
                        GuestPc::new(DMA_ENTRY.get() + 4),
                    )),
                    1,
                )
            }
            (DMA_NEW_BANK, pc) if pc == GuestPc::new(DMA_ENTRY.get() + 4) => {
                mem.store_w(0xFFFF_FFFF_8000_0018, 0xD00D_0002);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            (bank, pc) => BlockRun::new(
                BlockExit::Fault(unmapped(
                    bank,
                    pc,
                    DMA_ENTRY,
                    GuestPc::new(DMA_ENTRY.get() + 8),
                )),
                0,
            ),
        }
    }

    fn dma_rewrite_builder(
        bytes: &[u8],
        generation: u64,
    ) -> Result<(CodeBank, GeneratedBankRunner), String> {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().push((generation, bytes.to_vec())));
        if generation != 1 || bytes != [0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78] {
            return Err(format!(
                "unexpected DMA rewrite generation/image: {generation} {bytes:02x?}"
            ));
        }
        Ok((
            CodeBank::new(DMA_NEW_BANK, DMA_ENTRY, vec![1, 1])
                .map_err(|error| error.to_string())?,
            GeneratedBankRunner::new(DMA_NEW_BANK, dma_rewrite_runner),
        ))
    }

    fn live_host(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        ctx.set_r32(2, 0x1234);
        mem.store_w(0xFFFF_FFFF_8000_0000, ctx.r_u32(2));
    }

    fn ordered_writer_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        mem.as_mut_slice()[0x7000 ^ 3] = 1;
        super::super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
        mem.as_mut_slice()[0x7000 ^ 3] = 3;
    }

    fn ordered_writer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            ORDERED_WRITER_ENTRY => BlockRun::new(
                BlockExit::HostCall {
                    vram: ORDERED_WRITER_HOST,
                    resume: ExecutionKey::new(ORDERED_WRITER_BANK, ORDERED_WRITER_RESUME),
                },
                1,
            ),
            ORDERED_WRITER_RESUME => BlockRun::new(BlockExit::ThreadReturn, 1),
            pc => panic!("unexpected ordered-writer test PC {pc}"),
        }
    }

    fn ordered_sync_host(_ctx: &mut RsContext, mem: &mut Rdram<'_>) {
        mem.as_mut_slice()[0x7200 ^ 3] = 1;
        track_rsp_execution_or_hle_mutation(mem.as_mut_slice(), |rdram| {
            rdram[0x7200 ^ 3] = 2;
        });
        track_rdp_renderer_mutation(mem.as_mut_slice(), |rdram| {
            rdram[0x7200 ^ 3] = 3;
        });
        mem.as_mut_slice()[0x7200 ^ 3] = 4;
    }

    fn ordered_sync_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            ORDERED_SYNC_ENTRY => BlockRun::new(
                BlockExit::HostCall {
                    vram: ORDERED_SYNC_HOST,
                    resume: ExecutionKey::new(ORDERED_SYNC_BANK, ORDERED_SYNC_RESUME),
                },
                1,
            ),
            ORDERED_SYNC_RESUME => BlockRun::new(BlockExit::ThreadReturn, 1),
            pc => panic!("unexpected ordered synchronous-writer test PC {pc}"),
        }
    }

    fn live_host_lookup(vram: u32) -> Option<RecompFunc> {
        (vram == LIVE_HOST.get()).then_some(live_host)
    }

    fn forbidden_catalog_legacy_lookup(_vram: u32) -> Option<RecompFunc> {
        panic!("canonical catalog consulted the legacy global host lookup")
    }

    fn live_test_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            LIVE_ENTRY => {
                assert_eq!(ctx.r_u64(20), 0xffff_ffff_cafe_babe);
                assert_eq!(ctx.hi, 0x1234);
                assert_eq!(ctx.lo, 0x5678);
                BlockRun::new(
                    BlockExit::ResolveCall {
                        source_bank: LIVE_BANK,
                        target_pc: LIVE_HOST,
                        resume: ExecutionKey::new(LIVE_BANK, LIVE_NEXT),
                    },
                    3,
                )
            }
            LIVE_NEXT => BlockRun::new(BlockExit::ThreadReturn, 2),
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(LIVE_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: LIVE_ENTRY.get(),
                        bank_end: LIVE_NEXT.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn catalog_rewrite_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.bank {
            CATALOG_REWRITE_A => {
                mem.store_w(0xffff_ffff_8000_0080, 0x2402_0002);
                BlockRun::new(
                    BlockExit::ExecutableWrite {
                        source_bank: CATALOG_REWRITE_A,
                        resume: ExecutionKey::new(CATALOG_REWRITE_A, CATALOG_REWRITE_ENTRY),
                    },
                    1,
                )
            }
            CATALOG_REWRITE_B => {
                mem.store_w(0xffff_ffff_8000_0010, 0x0000_beef);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            bank => unreachable!("unexpected catalog rewrite bank {bank}"),
        }
    }

    fn prepared_generation_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RsContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.bank {
            PREPARED_STATIC_BANK => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(PREPARED_GENERATION_BANK, PREPARED_GENERATION_ENTRY),
                    kind: CpuFaultKind::NoActiveGeneration,
                }),
                1,
            ),
            PREPARED_GENERATION_BANK => BlockRun::new(BlockExit::ThreadReturn, 1),
            bank => unreachable!("unexpected prepared-continuation bank {bank}"),
        }
    }

    fn live_entry_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(LIVE_ACTIVE_BANK.with(std::cell::Cell::get), pc);
        if matches!(pc, LIVE_ENTRY | LIVE_NEXT) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: LIVE_ENTRY.get(),
                    bank_end: LIVE_NEXT.get() + 4,
                },
            })
        }
    }

    fn live_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        live_entry_lookup(pc)
    }

    fn irq_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.set_r(4, 0x0010_0401); // OS_IM_PI
                os_set_int_mask(ctx, mem);
                mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
                mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
                mem.store_w(0xFFFF_FFFF_A460_0008, 3);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, IRQ_RESUME)),
                    5,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0000, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0004, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0008, ctx.cop0_status);
                mem.store_w(0xFFFF_FFFF_A460_0010, 1 << 1); // clear PI interrupt
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(IRQ_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_000C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0010, ctx.cop0_status);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(IRQ_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn irq_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(IRQ_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn irq_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        irq_lookup(pc)
    }

    fn timer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            IRQ_ENTRY => {
                ctx.cop0_status = 1 | CpuInterruptLine::TIMER.cause_bit();
                ctx.write_cop0(9, 0);
                ctx.write_cop0(11, 2);
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, IRQ_RESUME)),
                    4,
                )
            }
            IRQ_VECTOR => {
                mem.store_w(0xFFFF_FFFF_8000_0020, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0024, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0028, ctx.cop0_count);
                ctx.write_cop0(11, ctx.cop0_compare);
                let resume = GuestPc::new(ctx.exception_return_pc());
                BlockRun::new(
                    BlockExit::Checkpoint(ExecutionKey::new(TIMER_BANK, resume)),
                    2,
                )
            }
            IRQ_RESUME => {
                mem.store_w(0xFFFF_FFFF_8000_002C, ctx.cop0_cause);
                mem.store_w(0xFFFF_FFFF_8000_0030, ctx.cop0_count);
                BlockRun::new(BlockExit::ThreadReturn, 2)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(TIMER_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: IRQ_ENTRY.get(),
                        bank_end: IRQ_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn timer_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(TIMER_BANK, pc);
        if matches!(pc, IRQ_ENTRY | IRQ_RESUME | IRQ_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: IRQ_ENTRY.get(),
                    bank_end: IRQ_VECTOR.get() + 4,
                },
            })
        }
    }

    fn timer_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        timer_lookup(pc)
    }

    #[test]
    fn c_adapter_round_trips_all_gprs_and_forces_zero() {
        let mut recompiled = RsContext::new();
        for i in 1..32 {
            recompiled.set_r(i, 0xA000_0000_0000_0000 | i as u64);
        }
        let mut c = c_from_recompiled(&recompiled);
        c.r0 = u64::MAX;
        c.r2 = 0x1234;
        copy_c_back(&c, &mut recompiled);
        assert_eq!(recompiled.r(0), 0);
        assert_eq!(recompiled.r(2), 0x1234);
        assert_eq!(recompiled.r(31), 0xA000_0000_0000_001F);
    }

    pub(super) unsafe extern "C" fn no_op_fpr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.assert_float_mode_matches_status();
        let expected = if ctx.mips3_float_mode == 0 {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f0.u32_halves.1 as *mut u32 }
        } else {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f1.u32_halves.0 as *mut u32 }
        };
        assert_eq!(ctx.f_odd, expected);
    }

    pub(super) unsafe extern "C" fn write_f5_word_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` arms `f_odd` for this live context. N64Recomp's
        // generated odd-register expression for f5 is `(5 - 1) * 2`.
        unsafe { *(*ctx).f_odd.add(8) = 0xDEAD_BEEF };
    }

    pub(super) unsafe extern "C" fn change_fr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
    }

    pub(super) unsafe extern "C" fn change_bev_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        unsafe { &mut *ctx }.status_reg ^= STATUS_BEV;
    }

    unsafe extern "C" fn transient_fr_write_shim(_rdram: *mut u8, ctx: *mut CContext) {
        TRANSIENT_FR_SHIM_ENTERED.store(true, Ordering::SeqCst);
        // Safety: the regression deliberately models a raw ABI shim which
        // changes to the other FPR view, accesses it, then restores the entry
        // mode before returning.
        let ctx = unsafe { &mut *ctx };
        let entry_status = ctx.status_reg;
        let entry_mode = ctx.mips3_float_mode;
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
        // Safety: `arm_fpr_alias` made this pointer live for the transient
        // view. The generated odd-register expression for f5 is `(5-1)*2`.
        unsafe { *ctx.f_odd.add(8) = 0xA11C_E55E };
        ctx.status_reg = entry_status;
        ctx.mips3_float_mode = entry_mode;
        ctx.arm_fpr_alias();
    }

    fn patterned_fgr_state(tag: u64) -> PhysicalFgrState {
        PhysicalFgrState::from_words(std::array::from_fn(|idx| {
            let high = (tag >> 32) as u32 ^ (0x0101_0000 + idx as u32);
            let low = tag as u32 ^ (0x0000_0101 + idx as u32);
            (u64::from(high) << 32) | u64::from(low)
        }))
    }

    #[test]
    fn c_adapter_layout_is_reversible_and_mode_exact() {
        let physical = patterned_fgr_state(0xA5A5_5A5A_DEAD_BEEF);
        let words = physical.into_words();
        for fr in [false, true] {
            let mut source = RsContext::new();
            source.cop0_status = if fr { STATUS_FR } else { 0 };
            source.replace_physical_fgr_state(physical);
            let c = c_from_recompiled(&source);
            c.assert_float_mode_matches_status();
            let image = c.fpr_u64_bits();
            if fr {
                assert_eq!(image, words);
            } else {
                for pair in 0..16 {
                    let even = pair * 2;
                    let odd = even + 1;
                    assert_eq!(
                        image[even],
                        u64::from(words[even] as u32) | (u64::from(words[odd] as u32) << 32)
                    );
                    assert_eq!(
                        image[odd],
                        (words[even] >> 32) | (words[odd] & 0xFFFF_FFFF_0000_0000)
                    );
                }
            }

            let mut restored = RsContext::new();
            copy_c_back(&c, &mut restored);
            assert_eq!(restored.physical_fgr_state(), physical);
            assert_eq!(restored.cop0_status & STATUS_FR != 0, fr);
        }
    }

    #[test]
    fn c_adapter_noop_preserves_every_physical_fgr_in_both_fr_modes() {
        for (fr, bev) in [(false, false), (false, true), (true, false), (true, true)] {
            let expected = patterned_fgr_state(if fr {
                0xA5A5_5A5A_DEAD_BEEF
            } else {
                0x1122_3344_5566_7788
            });
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 } | if bev { STATUS_BEV } else { 0 };
            ctx.replace_physical_fgr_state(expected);
            let mut bytes = [];
            let mut mem = Rdram::new(&mut bytes);

            call_c(&mut ctx, &mut mem, "no_op_fpr_shim", no_op_fpr_shim);

            assert_eq!(ctx.physical_fgr_state(), expected, "FR={fr}");
            assert_eq!(ctx.cop0_status & STATUS_FR != 0, fr);
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, bev);
        }
    }

    #[test]
    fn c_adapter_rejects_bev_changes_before_status_copyback() {
        for entry_bev in [false, true] {
            let mut ctx = RsContext::new();
            ctx.cop0_status = if entry_bev { STATUS_BEV } else { 0 };
            let mut bytes = [];
            let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                call_c(
                    &mut ctx,
                    &mut Rdram::new(&mut bytes),
                    "change_bev_shim",
                    change_bev_shim,
                );
            }));
            assert!(rejected.is_err());
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, entry_bev);
        }
    }

    #[test]
    fn c_adapter_f_odd_write_targets_physical_fgr5_in_both_modes() {
        for fr in [false, true] {
            let initial = patterned_fgr_state(0x1234_5678_9ABC_DEF0).into_words();
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            let mut bytes = [];
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "write_f5_word_shim",
                write_f5_word_shim,
            );
            let mut expected = initial;
            expected[5] = (expected[5] & 0xFFFF_FFFF_0000_0000) | 0xDEAD_BEEF;
            assert_eq!(ctx.physical_fgr_state().into_words(), expected, "FR={fr}");
        }
    }

    #[test]
    fn c_adapter_rejects_an_fr_transition_before_decoding_entry_view_bytes() {
        let expected = patterned_fgr_state(0x0BAD_F00D_CAFE_BABE);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "change_fr_shim",
                change_fr_shim,
            );
        }));
        assert!(rejected.is_err());
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_rejects_a_transient_fr_transition_before_the_shim_runs() {
        TRANSIENT_FR_SHIM_ENTERED.store(false, Ordering::SeqCst);
        let expected = patterned_fgr_state(0x1357_9BDF_2468_ACE0);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "transient_fr_write_shim",
                transient_fr_write_shim,
            );
        }));
        let panic = rejected.expect_err("unadmitted transient-FR shim must be rejected");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("registry rejection must use a string panic payload");
        assert!(
            message.contains("is not in the FR-stable adapter registry"),
            "unexpected rejection: {message}"
        );
        assert!(!TRANSIENT_FR_SHIM_ENTERED.load(Ordering::SeqCst));
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }

    #[test]
    fn c_adapter_float_helpers_return_through_f0_in_both_fr_modes() {
        let value = 0xFEDC_BA98_7654_3210u64;
        for fr in [false, true] {
            let initial = patterned_fgr_state(0xC001_D00D_A55A_5AA5).into_words();

            let mut float_ctx = RsContext::new();
            float_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            float_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            float_ctx.set_r(4, value >> 32);
            float_ctx.set_r(5, value as u32 as u64);
            let mut float_bytes = [];
            ull_to_f(&mut float_ctx, &mut Rdram::new(&mut float_bytes));
            assert_eq!(float_ctx.f_bits(0), (value as f32).to_bits(), "FR={fr}");
            let mut expected_float = initial;
            expected_float[0] =
                (expected_float[0] & 0xFFFF_FFFF_0000_0000) | u64::from((value as f32).to_bits());
            assert_eq!(
                float_ctx.physical_fgr_state().into_words(),
                expected_float,
                "FR={fr} float result changed non-result state"
            );

            let mut double_ctx = RsContext::new();
            double_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            double_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            double_ctx.set_r(4, value >> 32);
            double_ctx.set_r(5, value as u32 as u64);
            let mut double_bytes = [];
            ull_to_d(&mut double_ctx, &mut Rdram::new(&mut double_bytes));
            let result = (value as f64).to_bits();
            assert_eq!(double_ctx.d_bits(0), result, "FR={fr}");
            let mut expected_double = initial;
            if fr {
                expected_double[0] = result;
            } else {
                expected_double[0] =
                    (expected_double[0] & 0xFFFF_FFFF_0000_0000) | u64::from(result as u32);
                expected_double[1] = (expected_double[1] & 0xFFFF_FFFF_0000_0000) | (result >> 32);
            }
            assert_eq!(
                double_ctx.physical_fgr_state().into_words(),
                expected_double,
                "FR={fr} double result changed non-result state"
            );
        }
    }

    #[test]
    fn live_block_program_owns_thread_dispatch_and_charges_instruction_time() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(LIVE_ENTRY, GuestPc::new(LIVE_NEXT.get() + 4));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_BANK));
        region
            .install(
                &mut program,
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(LIVE_BANK, live_test_runner),
            )
            .unwrap();
        let thread_id = 0xB10C;
        let previous_host_lookup = fn64_recomp_rs::set_host_lookup(Some(live_host_lookup));

        // SAFETY: `bytes` remains live until the installed thread has
        // returned and the executor marks it dead below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                test_boot_context(LIVE_ENTRY),
                live_entry_lookup,
                live_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 3);
        assert!(!crate::is_thread_dead(thread_id));
        LIVE_ACTIVE_BANK.with(|active| active.set(LIVE_SECOND_BANK));
        assert_eq!(
            install_live_block_generation(
                &mut region,
                CodeBank::new(LIVE_SECOND_BANK, LIVE_ENTRY, vec![1, 1]).unwrap(),
                GeneratedBankRunner::new(LIVE_SECOND_BANK, live_test_runner),
            )
            .unwrap(),
            Some(LIVE_BANK)
        );
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        assert!(!crate::is_thread_dead(thread_id));
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_catalog_boot_owns_dispatch_host_lookup_and_evidence() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    LIVE_BANK,
                    live_test_runner,
                    ProgramArtifactIdentity::new([0x71; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(LIVE_HOST.get(), live_host)]).unwrap(),
            ProgramArtifactIdentity::new([0x72; 32]),
        );
        let expected_evidence = install.evidence().clone();
        let previous_host_lookup =
            fn64_recomp_rs::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
        let thread_id = 0xca70;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(LIVE_ENTRY),
                thread_id,
                10,
            );
        }

        assert_eq!(
            catalog_resolver_install_evidence_snapshot(),
            Some(expected_evidence)
        );
        assert!(copy_canonical_thread_publications_v1().is_empty());
        assert!(fn64_recomp_rs::resolve_host_function(LIVE_HOST.get()).is_none());
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 3);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(first)] = publications.as_slice() else {
            panic!("expected one exact first checkpoint publication: {publications:?}");
        };
        assert_eq!(first.thread, thread_id);
        assert_eq!(first.charged_instructions, 3);
        assert_eq!(first.canonical_charged_instructions_at_publication, 3);
        assert_eq!(
            first.pending_exit,
            BlockExit::HostCall {
                vram: LIVE_HOST,
                resume: ExecutionKey::new(LIVE_BANK, LIVE_NEXT),
            }
        );
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(second)] = publications.as_slice() else {
            panic!("expected one exact second checkpoint publication: {publications:?}");
        };
        assert_eq!(second.thread, thread_id);
        assert_eq!(second.charged_instructions, 2);
        assert_eq!(second.canonical_charged_instructions_at_publication, 5);
        assert_eq!(second.pending_exit, BlockExit::ThreadReturn);
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let publications = copy_canonical_thread_publications_v1();
        assert!(matches!(
            publications.as_slice(),
            [CanonicalThreadPublicationV1::Returned { thread, .. }] if *thread == thread_id
        ));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        assert_eq!(copy_block_host_boundaries().len(), 2);
        assert_eq!(copy_block_execution_destinations().len(), 2);
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_catalog_scheduler_reaches_a_one_instruction_limit() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let bank = BankId::new(0xca71);
        let entry = GuestPc::new(0x8000_0100);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0x73; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x74; 32]),
        );
        let thread_id = 0xca71;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(entry),
                thread_id,
                10,
            );
        }
        set_canonical_block_instruction_limit_v1(Some(1));

        assert!(crate::run_one_step());
        assert_eq!(canonical_block_charged_instructions_v1(), Some(1));
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = publications.as_slice() else {
            panic!("expected one exact one-instruction publication: {publications:?}");
        };
        assert_eq!(checkpoint.thread, thread_id);
        assert_eq!(checkpoint.charged_instructions, 1);
        assert_eq!(checkpoint.canonical_charged_instructions_at_publication, 1);
        assert_eq!(checkpoint.pending_exit, BlockExit::ThreadReturn);
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_boot_observes_external_write_across_suspended_host_without_replay() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        DYNAMIC_BOOT_SOURCE_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_HOST_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_RESUME_RUNS.store(0, Ordering::SeqCst);

        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x100);
        let mut bytes = vec![0u8; 0x8000];
        let jal = 0x0c00_0000 | ((host_pc.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut bytes, dynamic_pc.get() & 0x1fff_ffff, jal);
        put_physical_word(
            &mut bytes,
            dynamic_pc.get().wrapping_add(4) & 0x1fff_ffff,
            0,
        );

        let bank = BankId::new(0xca85);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    dynamic_boot_runner,
                    ProgramArtifactIdentity::new([0x85; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(host_pc.get(), dynamic_boot_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xe5; 32]),
        );
        let thread_id = 0xca85;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(INSTALL_PC),
                thread_id,
                10,
            );
        }
        set_canonical_block_instruction_limit_v1(Some(4));

        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 0);
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(first)] = publications.as_slice() else {
            panic!("expected one exact dynamic checkpoint publication: {publications:?}");
        };
        assert_eq!(first.thread, thread_id);
        assert_eq!(first.charged_instructions, 3);
        assert_eq!(first.canonical_charged_instructions_at_publication, 3);
        let BlockExit::HostCall {
            vram,
            resume: dynamic_resume,
        } = first.pending_exit
        else {
            panic!("expected a dynamic host-call checkpoint: {first:?}");
        };
        assert_eq!(vram, host_pc);
        assert_eq!(dynamic_resume.pc, resume);
        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(
            copy_canonical_thread_publications_v1(),
            vec![CanonicalThreadPublicationV1::OpaqueHostInFlight {
                thread: thread_id,
                target: host_pc,
                resume: dynamic_resume,
            }]
        );
        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7100);
        // SAFETY: guest execution is suspended, and this raw storage adapter
        // avoids creating a second mutable slice while the dormant coroutine
        // retains its checked `Rdram` view.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            1
        );

        // Model bytes already committed by an external producer. The typed PI
        // gateway is smoke-exercised here, but this plain catalog has no watched
        // mutation state, so real device timing and writer-journal ordering are
        // intentionally separate generation-backed contracts.
        unsafe {
            storage.write_u8(fn64_runtime::RdramAddr::from_offset(0x7100), 2);
        }
        fn64_recomp_rs::notify_pi_dma_write(0x7100, 1);
        process_live_executable_writes_from_host();

        assert!(crate::run_one_step());
        assert!(matches!(
            copy_canonical_thread_publications_v1().as_slice(),
            [CanonicalThreadPublicationV1::OpaqueHostInFlight { thread, .. }]
                if *thread == thread_id
        ));
        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(second)] = publications.as_slice() else {
            panic!("expected one exact resumed checkpoint publication: {publications:?}");
        };
        assert_eq!(second.thread, thread_id);
        assert_eq!(second.charged_instructions, 1);
        assert_eq!(second.canonical_charged_instructions_at_publication, 4);
        assert_eq!(second.pending_exit, BlockExit::ThreadReturn);
        crate::run_to_idle();
        assert!(crate::is_thread_dead(thread_id));
        let publications = copy_canonical_thread_publications_v1();
        assert!(matches!(
            publications.as_slice(),
            [CanonicalThreadPublicationV1::Returned { thread, .. }] if *thread == thread_id
        ));
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_RESUME_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(bytes[0x7100 ^ 3], 3);
        assert_eq!(copy_block_host_boundaries().len(), 2);
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(canonical_block_charged_instructions_v1(), Some(4));
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_dynamic_generation_boot_orders_real_pi_dma_during_suspended_host() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        DYNAMIC_BOOT_SOURCE_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_HOST_RUNS.store(0, Ordering::SeqCst);
        DYNAMIC_BOOT_RESUME_RUNS.store(0, Ordering::SeqCst);

        let dynamic_pc = GuestPc::new(INSTALL_PC.get() + 0x10);
        let resume = GuestPc::new(INSTALL_PC.get() + 0x18);
        let host_pc = GuestPc::new(INSTALL_PC.get() + 0x100);
        let watched_bank = BankId::new(0xca86);
        let mut rom = vec![0u8; 0x21];
        rom[0x20] = 2;
        crate::load_rom_with_fixed_pi_latency(rom, 1);

        let mut bytes = vec![0u8; 0x8000];
        let jal = 0x0c00_0000 | ((host_pc.get() >> 2) & 0x03ff_ffff);
        put_physical_word(&mut bytes, dynamic_pc.get() & 0x1fff_ffff, jal);
        put_physical_word(
            &mut bytes,
            dynamic_pc.get().wrapping_add(4) & 0x1fff_ffff,
            0,
        );

        let bank = BankId::new(0xca85);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::from_spans(
                    bank,
                    vec![
                        CodeSpan::new(bank, INSTALL_PC, vec![0]).unwrap(),
                        CodeSpan::new(bank, resume, vec![0]).unwrap(),
                    ],
                )
                .unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    dynamic_boot_runner,
                    ProgramArtifactIdentity::new([0x85; 32]),
                ),
            )
            .unwrap();
        program
            .register(
                CodeBank::new(watched_bank, host_pc, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    watched_bank,
                    install_test_runner,
                    ProgramArtifactIdentity::new([0x86; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, INSTALL_PC),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(host_pc.get(), dynamic_boot_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xe6; 32]),
        );
        let generation_id = GenerationId::new(0x86);
        let mut generation_catalog = PrecompiledGenerationCatalog::new();
        generation_catalog
            .register(
                PrecompiledGeneration::new(
                    generation_id,
                    host_pc,
                    GuestPc::new(host_pc.get() + 4),
                    host_pc,
                    GuestPc::new(host_pc.get() + 4),
                    sha2::Sha256::digest([0u8; 4]).into(),
                    vec![PrecompiledShard::new(
                        watched_bank,
                        host_pc,
                        GuestPc::new(host_pc.get() + 4),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            generation_catalog,
            vec![PrecompiledGenerationBackingV1::new(
                generation_id,
                vec![BackedExecutableSpanV1::new(host_pc, 0x7100, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca86;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(INSTALL_PC),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 0);
        assert!(crate::run_one_step());
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        let prefix = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(prefix.open_host_transactions.len(), 1);
        assert_eq!(prefix.entries.len(), 1);
        assert_eq!(
            prefix.entries[0].declared_writes[0].channel,
            WriterChannel::HostAbi
        );

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7100);
        // SAFETY: the guest is suspended and the registered process allocation
        // remains live; use the raw adapter rather than aliasing its dormant
        // checked `Rdram` view.
        let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            1
        );
        assert!(write_raw_mmio(0xffff_ffff_a460_0000, 0x7100));
        assert!(write_raw_mmio(0xffff_ffff_a460_0004, 0x20));
        assert!(write_raw_mmio(0xffff_ffff_a460_0008, 0));
        let pi_deadline = with_host(|host| {
            host.device_fabric
                .now()
                .get()
                .checked_add(1)
                .expect("PI completion deadline overflow")
        });
        crate::advance_virtual_time(pi_deadline);
        assert_eq!(
            unsafe { storage.read_u8(fn64_runtime::RdramAddr::from_offset(0x7100)) },
            2
        );
        let after_pi = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(after_pi.open_host_transactions.len(), 1);
        assert_eq!(after_pi.entries.len(), 2);
        assert_eq!(
            after_pi.entries[1].declared_writes[0].channel,
            WriterChannel::PiDma
        );

        assert!(crate::run_one_step());
        crate::run_to_idle();
        assert!(crate::is_thread_dead(thread_id));
        assert_eq!(DYNAMIC_BOOT_SOURCE_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_HOST_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(DYNAMIC_BOOT_RESUME_RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(bytes[0x7100 ^ 3], 3);

        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::PiDma,
                WriterChannel::HostAbi,
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
        let telemetry = copy_dynamic_mapped_execution_telemetry_v1();
        assert_eq!(telemetry.aggregates.len(), 1);
        assert_eq!(telemetry.aggregates[0].charged_instructions, 2);
        assert_eq!(telemetry.dropped_identity_activations, 0);
        assert_eq!(canonical_block_charged_instructions_v1(), Some(4));
    }

    #[test]
    fn canonical_generation_boot_activates_explicit_physical_backing() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1008];
        let image = [0x24, 0x02, 0x00, 0x01, 0x03, 0xe0, 0x00, 0x08];
        for (index, byte) in image.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(LIVE_BANK, LIVE_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    LIVE_BANK,
                    live_test_runner,
                    ProgramArtifactIdentity::new([0x73; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(LIVE_BANK, LIVE_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(LIVE_HOST.get(), live_host)]).unwrap(),
            ProgramArtifactIdentity::new([0x74; 32]),
        );
        let mut generation_catalog = PrecompiledGenerationCatalog::new();
        generation_catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(0x75),
                    LIVE_ENTRY,
                    GuestPc::new(LIVE_ENTRY.get() + 8),
                    LIVE_ENTRY,
                    GuestPc::new(LIVE_ENTRY.get() + 8),
                    sha2::Sha256::digest(image).into(),
                    vec![PrecompiledShard::new(
                        LIVE_BANK,
                        LIVE_ENTRY,
                        GuestPc::new(LIVE_ENTRY.get() + 8),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let backed = BackedPrecompiledGenerationCatalogV1::new(
            generation_catalog,
            vec![PrecompiledGenerationBackingV1::new(
                GenerationId::new(0x75),
                vec![BackedExecutableSpanV1::new(LIVE_ENTRY, 0x80, 8).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let generation_install = CatalogGenerationInstallV1::new(resolver, backed).unwrap();
        let inactive_evidence = generation_install.evidence_snapshot();
        assert!(inactive_evidence.generations.active_segments.is_empty());
        let previous_host_lookup =
            fn64_recomp_rs::set_host_lookup(Some(forbidden_catalog_legacy_lookup));
        let thread_id = 0xca76;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                generation_install,
                test_boot_context(LIVE_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let active_evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert_eq!(active_evidence.resolver, inactive_evidence.resolver);
        assert_eq!(active_evidence.generations.active_segments.len(), 1);
        assert!(active_evidence.pending_physical_writes.is_empty());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0x1234);
        fn64_recomp_rs::set_host_lookup(previous_host_lookup);
    }

    #[test]
    fn canonical_publication_binds_prepared_generation_continuation() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let generation_word = 0x2402_0091u32;
        let generation_image = generation_word.to_be_bytes();
        let mut bytes = vec![0u8; 0x7000];
        for (index, byte) in generation_image.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }

        let mut program = BlockProgram::new();
        for (bank, entry, word, identity) in [
            (PREPARED_STATIC_BANK, PREPARED_STATIC_ENTRY, 0u32, 0x90),
            (
                PREPARED_GENERATION_BANK,
                PREPARED_GENERATION_ENTRY,
                generation_word,
                0x91,
            ),
        ] {
            program
                .register(
                    CodeBank::new(bank, entry, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        prepared_generation_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(PREPARED_STATIC_BANK, PREPARED_STATIC_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x92; 32]),
        );
        let mut catalog = PrecompiledGenerationCatalog::new();
        catalog
            .register(
                PrecompiledGeneration::new(
                    GenerationId::new(0x91),
                    PREPARED_GENERATION_ENTRY,
                    GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    PREPARED_GENERATION_ENTRY,
                    GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    sha2::Sha256::digest(generation_image).into(),
                    vec![PrecompiledShard::new(
                        PREPARED_GENERATION_BANK,
                        PREPARED_GENERATION_ENTRY,
                        GuestPc::new(PREPARED_GENERATION_ENTRY.get() + 4),
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            catalog,
            vec![PrecompiledGenerationBackingV1::new(
                GenerationId::new(0x91),
                vec![BackedExecutableSpanV1::new(PREPARED_GENERATION_ENTRY, 0x80, 4).unwrap()],
            )
            .unwrap()],
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca91;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(PREPARED_STATIC_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = publications.as_slice() else {
            panic!("prepared generation did not publish an exact checkpoint: {publications:?}");
        };
        assert!(matches!(
            checkpoint.pending_exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::NoActiveGeneration,
                ..
            })
        ));
        assert_eq!(
            checkpoint.prepared_continuation,
            Some(CanonicalPreparedContinuationV1::InactiveGeneration {
                entry: ExecutionKey::new(PREPARED_GENERATION_BANK, PREPARED_GENERATION_ENTRY,),
            })
        );

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn canonical_generation_cpu_write_retires_a_before_b_executes() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let image_a = 0x2402_0001u32.to_be_bytes();
        let image_b = 0x2402_0002u32.to_be_bytes();
        let mut bytes = vec![0u8; 0x6004];
        for (index, byte) in image_a.iter().copied().enumerate() {
            bytes[(0x80 + index) ^ 3] = byte;
        }
        let mut program = BlockProgram::new();
        for (bank, word, identity) in [
            (CATALOG_REWRITE_A, 0x2402_0001, 0x81),
            (CATALOG_REWRITE_B, 0x2402_0002, 0x82),
        ] {
            program
                .register(
                    CodeBank::new(bank, CATALOG_REWRITE_ENTRY, vec![word]).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        bank,
                        catalog_rewrite_runner,
                        ProgramArtifactIdentity::new([identity; 32]),
                    ),
                )
                .unwrap();
        }
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(CATALOG_REWRITE_A, CATALOG_REWRITE_ENTRY),
                InstructionBudget::new(4).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0x83; 32]),
        );
        let mut catalog = PrecompiledGenerationCatalog::new();
        for (id, bank, image) in [
            (1, CATALOG_REWRITE_A, image_a),
            (2, CATALOG_REWRITE_B, image_b),
        ] {
            catalog
                .register(
                    PrecompiledGeneration::new(
                        GenerationId::new(id),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        CATALOG_REWRITE_ENTRY,
                        GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        sha2::Sha256::digest(image).into(),
                        vec![PrecompiledShard::new(
                            bank,
                            CATALOG_REWRITE_ENTRY,
                            GuestPc::new(CATALOG_REWRITE_ENTRY.get() + 4),
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let backing = |id| {
            PrecompiledGenerationBackingV1::new(
                GenerationId::new(id),
                vec![BackedExecutableSpanV1::new(CATALOG_REWRITE_ENTRY, 0x80, 4).unwrap()],
            )
            .unwrap()
        };
        let generations =
            BackedPrecompiledGenerationCatalogV1::new(catalog, vec![backing(2), backing(1)])
                .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let thread_id = 0xca82;

        // SAFETY: `bytes` remains live until the installed thread returns.
        unsafe {
            boot_thread0_catalog_generation_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                install,
                test_boot_context(CATALOG_REWRITE_ENTRY),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        let after_cpu_write = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(
            after_cpu_write.generations.active_segments.is_empty(),
            "generation A remained active across its committed executable write"
        );
        let cpu_journal = after_cpu_write.mutation_journal.unwrap();
        assert!(cpu_journal.sealed);
        assert_eq!(cpu_journal.entries.len(), 1);
        assert_eq!(
            cpu_journal.entries[0].declared_writes[0].channel,
            WriterChannel::CpuInstructionStore
        );
        assert_eq!(
            cpu_journal.entries[0].invalidated_generations,
            [GenerationId::new(1)]
        );
        assert_eq!(Rdram::new(&mut bytes).load_w(0xffff_ffff_8000_0010), 0);

        assert!(crate::run_one_step());
        let evidence = catalog_generation_install_evidence_snapshot().unwrap();
        assert_eq!(evidence.generations.active_segments.len(), 1);
        assert_eq!(
            evidence.generations.active_segments[0].generation,
            GenerationId::new(2)
        );
        assert_eq!(
            Rdram::new(&mut bytes).load_w(0xffff_ffff_8000_0010),
            0x0000_beef
        );
        fn64_recomp_rs::notify_host_abi_write(0x80, 4);
        process_live_executable_writes_from_host();
        let after_host_write = catalog_generation_install_evidence_snapshot().unwrap();
        assert!(
            after_host_write.generations.active_segments.is_empty(),
            "host/DMA write notification did not retire generation B"
        );
        let host_journal = after_host_write.mutation_journal.unwrap();
        assert_eq!(host_journal.entries.len(), 2);
        assert_eq!(
            host_journal.entries[1].declared_writes[0].channel,
            WriterChannel::HostAbi
        );
        assert_eq!(
            host_journal.entries[1].invalidated_generations,
            [GenerationId::new(2)]
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn interpreter_cpu_store_retires_generation_before_its_next_instruction() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        REWRITE_B_ENTRIES.with(|entries| entries.borrow_mut().clear());
        let mut bytes = vec![0u8; 0x200];
        fn64_recomp_rs::set_write_observer(None);
        fn64_recomp_rs::set_guest_write_boundary_observer(None);
        {
            let mut mem = Rdram::new(&mut bytes);
            for (index, word) in REWRITE_A_WORDS.into_iter().enumerate() {
                mem.store_w(
                    0xFFFF_FFFF_8000_0000
                        | u64::from(REWRITE_PHYSICAL + u32::try_from(index * 4).unwrap()),
                    word,
                );
            }
        }
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(
            REWRITE_ENTRY,
            GuestPc::new(REWRITE_ENTRY.get() + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap()),
        );
        region
            .install(
                &mut program,
                CodeBank::new(REWRITE_OLD_BANK, REWRITE_ENTRY, REWRITE_A_WORDS.to_vec()).unwrap(),
                GeneratedBankRunner::new(REWRITE_OLD_BANK, rewrite_interpreter_runner),
            )
            .unwrap();
        let thread_id = 0xC0DE;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(REWRITE_OLD_BANK, REWRITE_ENTRY),
                test_boot_context(REWRITE_ENTRY),
                rewrite_lookup,
                rewrite_transfer_lookup,
                InstructionBudget::new(13).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            REWRITE_PHYSICAL,
            REWRITE_PHYSICAL + u32::try_from(REWRITE_A_WORDS.len() * 4).unwrap(),
            region,
            rewrite_builder,
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, 0x55);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0024) as u32, 0x66);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0010) as u32,
            0,
            "generation A executed its post-store sentinel before invalidation"
        );
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_OLD_BANK)
            .is_none());
        assert!(live
            .program
            .borrow()
            .code()
            .bank(REWRITE_NEW_BANK)
            .is_some());
        assert_eq!(
            live.resolve_entry(REWRITE_ENTRY).unwrap().bank,
            REWRITE_NEW_BANK
        );
        assert_eq!(
            live.resolve_entry(REWRITE_RESUME).unwrap(),
            ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)
        );
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(
                1,
                std::iter::once(0x1122_3344)
                    .chain(REWRITE_A_WORDS.into_iter().skip(1))
                    .flat_map(u32::to_be_bytes)
                    .collect::<Vec<_>>()
            )]
        );
        assert!(REWRITE_B_ENTRIES.with(|entries| entries.borrow().is_empty()));

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32, 0);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 2);
        assert_eq!(
            REWRITE_B_ENTRIES.with(|entries| entries.borrow().clone()),
            vec![ExecutionKey::new(REWRITE_NEW_BANK, REWRITE_RESUME)]
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn pi_dma_rebuilds_executable_region_before_completion_is_observable() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x28].copy_from_slice(&[0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x200];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(DMA_ENTRY, GuestPc::new(DMA_ENTRY.get() + 8));
        region
            .install(
                &mut program,
                CodeBank::new(DMA_OLD_BANK, DMA_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(DMA_OLD_BANK, dma_rewrite_runner),
            )
            .unwrap();
        let thread_id = 0xD00D;

        // SAFETY: `bytes` remains live until this thread returns below.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(DMA_OLD_BANK, DMA_ENTRY),
                test_boot_context(DMA_ENTRY),
                dma_lookup,
                dma_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }
        register_live_executable_region(
            DMA_PHYSICAL,
            DMA_PHYSICAL + 8,
            region,
            dma_rewrite_builder,
        );

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        let live = with_host(|host| host.recompiled_program.clone().unwrap());
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_none());
        assert_eq!(live.resolve_entry(DMA_ENTRY).unwrap().bank, DMA_NEW_BANK);
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(1, vec![0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78])]
        );

        assert!(crate::run_one_step());
        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0014) as u32, 0xD00D_0001);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_8000_0018) as u32,
            0xD00D_0002,
            "the already-serviced DMA boundary split generation B's first turn"
        );
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn fetch_activated_region_defers_dirty_image_until_attempted_fetch() {
        REWRITE_BUILDS.with(|builds| builds.borrow_mut().clear());
        PENDING_EXECUTABLE_WRITES
            .with(|pending| *pending.borrow_mut() = vec![(0, 4), (DMA_PHYSICAL - 4, 16)]);
        let completed = [0x3c, 0x08, 0x12, 0x34, 0x35, 0x08, 0x56, 0x78];
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(DMA_ENTRY, GuestPc::new(DMA_ENTRY.get() + 8));
        region
            .install(
                &mut program,
                CodeBank::new(DMA_OLD_BANK, DMA_ENTRY, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new(DMA_OLD_BANK, dma_rewrite_runner),
            )
            .unwrap();
        let live = LiveBlockProgram {
            program: Rc::new(RefCell::new(program)),
            entry_lookup: dma_lookup,
            transfer_lookup: dma_transfer_lookup,
            budget: InstructionBudget::new(8).unwrap(),
            dispatch_artifact_identity: None,
            executable_regions: Rc::new(RefCell::new(vec![ObservedExecutableRegion {
                physical_start: DMA_PHYSICAL,
                physical_end: DMA_PHYSICAL + 8,
                region,
                next_generation: 1,
                builder: dma_rewrite_builder,
                builder_artifact_identity: None,
                activation: ExecutableActivation::FetchBoundary,
            }])),
            precompiled_generations: Rc::new(RefCell::new(None)),
        };

        assert!(process_executable_writes(&live, |offset| completed
            [usize::try_from(offset - DMA_PHYSICAL).unwrap()])
        .is_empty());
        assert!(REWRITE_BUILDS.with(|builds| builds.borrow().is_empty()));
        assert_eq!(
            PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            vec![(DMA_PHYSICAL, 8)]
        );
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_some());

        let attempted = ExecutionKey::new(DMA_OLD_BANK, GuestPc::new(DMA_ENTRY.get() + 4));
        let retry = activate_fetch_generation(
            &live,
            attempted,
            AotMiss {
                expected_bank: DMA_OLD_BANK,
                va_start: DMA_ENTRY,
                byte_len: 8,
                expected_sha256: [0x11; 32],
                actual_sha256: [0x22; 32],
            },
            |offset| completed[usize::try_from(offset - DMA_PHYSICAL).unwrap()],
        )
        .unwrap();
        assert_eq!(retry, ExecutionKey::new(DMA_NEW_BANK, attempted.pc));
        assert!(live.program.borrow().code().bank(DMA_OLD_BANK).is_none());
        assert!(live.program.borrow().code().bank(DMA_NEW_BANK).is_some());
        assert_eq!(
            REWRITE_BUILDS.with(|builds| builds.borrow().clone()),
            vec![(1, completed.to_vec())]
        );
        assert!(PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow().is_empty()));
    }

    const BRK_BANK: BankId = BankId::new(0xB4EA);
    // Entry and the 0x8000_0180 general exception vector must sit in the same
    // registered code bank so the vectored handler PC is admitted; 33 words
    // from 0x8000_0100 spans [0x100, 0x184).
    const BRK_ENTRY: GuestPc = GuestPc::new(0x8000_0100);
    const BRK_VECTOR: GuestPc = GuestPc::new(0x8000_0180);

    // A block that hits a mid-function BREAK: the emitter renders this as
    // `BlockExit::Fault { kind: Exception { Breakpoint } }`. Before the driver
    // fix this reached `recompiled_gap_panic`; now it must vector to the
    // general exception handler like any architectural exception.
    fn brk_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RsContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        match entry.pc {
            BRK_ENTRY => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                    kind: CpuFaultKind::Exception {
                        exception: fn64_recomp_rs::CpuException::Breakpoint,
                        epc: BRK_ENTRY,
                        branch_delay: false,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: None,
                    },
                }),
                1,
            ),
            BRK_VECTOR => {
                // Record the architectural state the vectoring produced so the
                // test can prove we reached the handler with a real BREAK frame.
                mem.store_w(0xFFFF_FFFF_8000_0000, ctx.cop0_epc);
                mem.store_w(0xFFFF_FFFF_8000_0004, ctx.cop0_cause);
                BlockRun::new(BlockExit::ThreadReturn, 1)
            }
            pc => BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: ExecutionKey::new(BRK_BANK, pc),
                    kind: CpuFaultKind::UnmappedPc {
                        bank_start: BRK_ENTRY.get(),
                        bank_end: BRK_VECTOR.get() + 4,
                    },
                }),
                0,
            ),
        }
    }

    fn brk_lookup(pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        let key = ExecutionKey::new(BRK_BANK, pc);
        if matches!(pc, BRK_ENTRY | BRK_VECTOR) {
            Ok(key)
        } else {
            Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: BRK_ENTRY.get(),
                    bank_end: BRK_ENTRY.get() + 4,
                },
            })
        }
    }

    fn brk_transfer_lookup(_source: BankId, pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        brk_lookup(pc)
    }

    fn canonical_brk_install() -> CatalogResolverInstallV1 {
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(BRK_BANK, BRK_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    BRK_BANK,
                    brk_runner,
                    ProgramArtifactIdentity::new([0xb4; 32]),
                ),
            )
            .unwrap();
        CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                InstructionBudget::new(8).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xea; 32]),
        )
    }

    fn assert_canonical_break_parks_with_post_exception_publication(thread_id: ThreadId) {
        assert!(crate::run_one_step());
        let checkpoint_publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::Exact(checkpoint)] = checkpoint_publications.as_slice()
        else {
            panic!("canonical BREAK did not first publish its exact charged checkpoint");
        };
        assert!(matches!(
            checkpoint.pending_exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey { bank, pc },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::Breakpoint,
                    ..
                },
            }) if bank == BRK_BANK && pc == BRK_ENTRY
        ));
        assert_eq!(checkpoint.prepared_continuation, None);

        assert!(crate::run_one_step());
        let publications = copy_canonical_thread_publications_v1();
        let [CanonicalThreadPublicationV1::ParkedFaultOpaque {
            thread,
            post_exception_cpu,
            fault,
            canonical_charged_instructions_at_publication,
        }] = publications.as_slice()
        else {
            panic!("canonical BREAK retained a stale exact publication: {publications:?}");
        };
        assert_eq!(*thread, thread_id);
        assert_eq!(*canonical_charged_instructions_at_publication, 1);
        assert_eq!(post_exception_cpu.cop0_epc, BRK_ENTRY.get());
        assert_eq!((post_exception_cpu.cop0_cause >> 2) & 0x1f, 9);
        assert!(matches!(
            fault,
            CpuFault {
                at: ExecutionKey { bank, pc },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::Breakpoint,
                    ..
                },
            } if *bank == BRK_BANK && *pc == BRK_ENTRY
        ));
        assert!(!crate::is_thread_dead(thread_id));
        // The tested state is intentionally stopped forever; retire its
        // dormant coroutine while the caller's backing RDRAM is still live.
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
    }

    #[test]
    fn canonical_publication_static_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4eb;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }

    #[cfg(feature = "dynamic-mapped-runtime")]
    #[test]
    fn canonical_publication_dynamic_break_replaces_exact_with_parked_fault() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let thread_id = 0xb4ec;

        // SAFETY: `bytes` remains live while the deliberately stopped thread
        // retains its dormant coroutine.
        unsafe {
            boot_thread0_catalog_program_with_dynamic_mapped_v1(
                bytes.as_mut_ptr(),
                bytes.len(),
                canonical_brk_install(),
                test_boot_context(BRK_ENTRY),
                thread_id,
                10,
            );
        }
        with_host(|host| {
            host.thread_handle_vrams.insert(thread_id, 0x8000_0200);
        });

        assert_canonical_break_parks_with_post_exception_publication(thread_id);
    }

    #[test]
    fn block_program_vectors_mid_function_break_instead_of_panicking() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(BRK_BANK, BRK_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(BRK_BANK, brk_runner),
            )
            .unwrap();
        let thread_id = 0xB4EA;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(BRK_BANK, BRK_ENTRY),
                test_boot_context(BRK_ENTRY),
                brk_lookup,
                brk_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        // Runs to completion — reaching the handler and returning — rather than
        // hitting recompiled_gap_panic on the BREAK fault. The entry block, the
        // vectored handler, and the thread-return retire across steps; drive the
        // executor until the thread is dead (bounded so a regression can't spin).
        let mut steps = 0;
        while !crate::is_thread_dead(thread_id) {
            assert!(
                crate::run_one_step(),
                "executor stalled before thread return"
            );
            steps += 1;
            assert!(
                steps < 8,
                "BREAK vectoring did not converge to thread return"
            );
        }

        let mem = Rdram::new(&mut bytes);
        // EPC captured the faulting PC, and Cause.ExcCode == 9 (Breakpoint).
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, BRK_ENTRY.get());
        assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 9);
    }

    #[test]
    fn checkpoint_due_pi_enters_ip2_handler_before_the_next_guest_block() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(IRQ_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(IRQ_BANK, irq_runner),
            )
            .unwrap();
        let thread_id = 0x1A2;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(IRQ_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                irq_lookup,
                irq_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 5);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000), 0);
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 7);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0000) as u32, IRQ_RESUME.get());
            assert_eq!((mem.load_w(0xFFFF_FFFF_8000_0004) as u32 >> 2) & 0x1F, 0);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0004) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_ne!(mem.load_w(0xFFFF_FFFF_8000_0008) as u32 & (1 << 1), 0);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_000C) as u32 & CpuInterruptLine::RCP.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0010) as u32 & (1 << 1), 0);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn checkpoint_count_compare_match_enters_ip7_and_compare_write_acks_it() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let mut bytes = vec![0u8; 0x100];
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(TIMER_BANK, IRQ_ENTRY, vec![0; 33]).unwrap(),
                GeneratedBankRunner::new(TIMER_BANK, timer_runner),
            )
            .unwrap();
        let thread_id = 0x1A7;

        // SAFETY: `bytes` remains live through the thread's final return.
        unsafe {
            boot_thread0_block_program(
                bytes.as_mut_ptr(),
                bytes.len(),
                program,
                ExecutionKey::new(TIMER_BANK, IRQ_ENTRY),
                test_boot_context(IRQ_ENTRY),
                timer_lookup,
                timer_transfer_lookup,
                InstructionBudget::new(8).unwrap(),
                thread_id,
                10,
            );
        }

        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 4);
        assert!(crate::run_one_step());
        assert_eq!(crate::host::sim_time(), 6);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0020) as u32, IRQ_RESUME.get());
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_8000_0024) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0028) as u32, 2);
        }

        assert!(crate::run_one_step());
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_8000_002C) as u32 & CpuInterruptLine::TIMER.cause_bit(),
                0
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0030) as u32, 3);
        }
        assert!(crate::run_one_step());
        assert!(crate::is_thread_dead(thread_id));
    }

    #[test]
    fn status_adapters_are_per_context_state() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0x3400_0001);
        os_set_sr(&mut ctx, &mut mem);
        ctx.set_r(2, 0);
        os_get_sr(&mut ctx, &mut mem);
        assert_eq!(ctx.r_u32(2), 0x3400_0001);
    }

    #[test]
    fn typed_fpcsr_setter_and_new_thread_use_the_generated_cop1_authority() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = new_osthread_context(None);
        let mut second = new_osthread_context(None);

        assert_eq!(first.read_fcr(31), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        first.set_r(4, 3);
        os_set_fpc_csr(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), INITIAL_FPCSR);
        assert_eq!(first.read_fcr(31), 3);
        assert_eq!(second.read_fcr(31), INITIAL_FPCSR);

        second.set_r(4, 2);
        os_set_fpc_csr(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), INITIAL_FPCSR);
        assert_eq!(second.read_fcr(31), 2);
        assert_eq!(first.read_fcr(31), 3);

        let pending: u32 = (1 << 16) | (1 << 11);
        first.set_r(4, u64::from(pending));
        let loud = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            os_set_fpc_csr(&mut first, &mut mem);
        }));
        assert!(
            loud.is_err(),
            "enabled Cause written by host call must stay loud"
        );
        assert_eq!(first.r_u32(2), 3);
        assert_eq!(first.read_fcr(31), pending);
        assert_eq!(second.read_fcr(31), 2);
    }

    /// Public osCreateThread gives each OSThread its own saved FPCSR. This
    /// drives real executor coroutine suspension and alternates A/B/A/B/A/B;
    /// the context-local values must survive switches through another thread.
    #[test]
    fn alternating_osthread_coroutines_preserve_independent_fpcsr() {
        const THREAD_A: ThreadId = 0xF5A0;
        const THREAD_B: ThreadId = 0xF5B0;

        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 3);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 1);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = new_osthread_context(None);
                ctx.write_fcr(31, 2);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
                ctx.write_fcr(31, 0);
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.read_fcr(31));
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        for _ in 0..6 {
            assert!(crate::run_one_step());
        }

        assert_eq!(&*observed_a.borrow(), &[3, 3, 1]);
        assert_eq!(&*observed_b.borrow(), &[2, 2, 0]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    /// Thread 0 is the reset context, not an osCreateThread context. The
    /// public osInitialize contract performs the observable 0 -> FS|EV
    /// transition at the real typed boot entry.
    #[test]
    fn thread0_boot_path_transitions_fpcsr_only_at_os_initialize() {
        const THREAD0: ThreadId = 0xF500;
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        BOOT_FPCSR_OBSERVATIONS.with(|observed| observed.borrow_mut().clear());
        let mut bytes = [0u8; 8];

        unsafe {
            boot_thread0(
                bytes.as_mut_ptr(),
                bytes.len(),
                evidence_lookup,
                observe_thread0_fpcsr_boot,
                THREAD0,
                10,
            );
        }
        crate::run_to_idle();

        BOOT_FPCSR_OBSERVATIONS.with(|observed| {
            assert_eq!(&*observed.borrow(), &[0, INITIAL_FPCSR]);
        });
        assert!(crate::is_thread_dead(THREAD0));
    }

    #[test]
    fn typed_os_initialize_replaces_the_current_context_fpcsr() {
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        crate::load_rom_with_fixed_pi_latency(Vec::new(), 1);
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.write_fcr(31, 3);

        os_initialize(&mut ctx, &mut mem);

        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }

    #[test]
    fn created_osthread_enters_fr0_without_discarding_other_status_fields() {
        let inherited = 0xA5A5_5A5A | STATUS_FR;

        let ctx = new_osthread_context(Some(inherited));

        assert_eq!(ctx.cop0_status, inherited & !STATUS_FR);
        assert_eq!(ctx.read_fcr(31), INITIAL_FPCSR);
    }

    #[test]
    fn alternating_osthread_coroutines_preserve_all_physical_fgr_bits() {
        const THREAD_A: ThreadId = 0xF5C0;
        const THREAD_B: ThreadId = 0xF5D0;
        let state_a = patterned_fgr_state(0x1111_2222_3333_4444);
        let state_b = patterned_fgr_state(0xAAAA_BBBB_CCCC_DDDD);
        let observed_a = Rc::new(RefCell::new(Vec::new()));
        let observed_b = Rc::new(RefCell::new(Vec::new()));
        let observed_a_body = Rc::clone(&observed_a);
        let observed_b_body = Rc::clone(&observed_b);

        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status &= !STATUS_FR;
                ctx.replace_physical_fgr_state(state_a);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_a_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                let _ = first_input;
                let mut ctx = RsContext::new();
                ctx.cop0_status |= STATUS_FR;
                ctx.replace_physical_fgr_state(state_b);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
                let _ = yielder.suspend(fn64_runtime::Yield::PauseSelf);
                observed_b_body.borrow_mut().push(ctx.physical_fgr_state());
            });
            exec.start_thread(THREAD_A);
            exec.start_thread(THREAD_B);
        });

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        assert!(crate::run_one_step());

        assert_eq!(&*observed_a.borrow(), &[state_a, state_a]);
        assert_eq!(&*observed_b.borrow(), &[state_b, state_b]);
        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });
    }

    #[test]
    fn typed_interrupt_masks_return_each_contexts_own_previous_value() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut first = RsContext::new();
        let mut second = RsContext::new();

        first.set_r(4, 0x0010_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0);
        second.set_r(4, 0x0008_0401);
        os_set_int_mask(&mut second, &mut mem);
        assert_eq!(second.r_u32(2), 0);
        first.set_r(4, 0x0004_0401);
        os_set_int_mask(&mut first, &mut mem);
        assert_eq!(first.r_u32(2), 0x0010_0401);
    }

    #[test]
    fn typed_raw_word_accesses_and_sp_shims_share_one_device_fabric_state() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);

        mem.store_w(0xFFFF_FFFF_A408_0000, 0x0A8);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A408_0000) as u32, 0x0A8);

        let mut set = CContext::zeroed();
        set.r4 = 1 << 10;
        unsafe { crate::__osSpSetStatus_recomp(std::ptr::null_mut(), &mut set) };
        assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & (1 << 7), 1 << 7);

        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_sp_dma_replaces_persistent_imem_on_guest_time() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut bytes = vec![0u8; 0x1000];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (index, byte) in [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]
                .into_iter()
                .enumerate()
            {
                view.write_u8(
                    fn64_runtime::RdramAddr::from_offset(0x100 + index as u32),
                    byte,
                );
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A404_0000, 0x1000);
            mem.store_w(0xFFFF_FFFF_A404_0004, 0x100);
            mem.store_w(0xFFFF_FFFF_A404_0008, 7);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }

        crate::advance_virtual_time(8);
        {
            let mem = Rdram::new(&mut bytes);
            assert_ne!(
                mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & fn64_runtime::SP_STATUS_DMA_BUSY,
                0
            );
        }
        crate::advance_virtual_time(9);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A404_0010) as u32 & 4, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1000) as u32, 0x1020_3040);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A400_1004) as u32, 0x5060_7080);
        }
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().sp_imem_generation),
            1
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_pi_registers_drive_the_live_timed_device_fabric() {
        let mut rom = vec![0u8; 0x100];
        rom[0x20..0x24].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        crate::load_rom_with_fixed_pi_latency(rom, 5);
        let mut bytes = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A460_0000, 0x400);
            mem.store_w(0xFFFF_FFFF_A460_0004, 0x20);
            mem.store_w(0xFFFF_FFFF_A460_0008, 3);
            assert_eq!(
                mem.load_w(0xFFFF_FFFF_A460_0010) as u32,
                fn64_runtime::PI_STATUS_DMA_BUSY
            );
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }

        crate::advance_virtual_time(4);
        {
            let mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400), 0);
        }
        crate::advance_virtual_time(5);

        let mem = Rdram::new(&mut bytes);
        assert_eq!(mem.load_w(0xFFFF_FFFF_8000_0400) as u32, 0x1234_5678);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A460_0010), 0);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Pi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_rcp_acknowledgements_clear_the_shared_mi_sources() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let sources = [
            fn64_runtime::InterruptSource::Sp,
            fn64_runtime::InterruptSource::Si,
            fn64_runtime::InterruptSource::Ai,
            fn64_runtime::InterruptSource::Vi,
            fn64_runtime::InterruptSource::Dp,
        ];
        with_host(|host| {
            let fabric = &mut host.device_fabric;
            for source in sources {
                fabric.raise_interrupt(source);
            }
        });

        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A404_0010, 1 << 3);
        mem.store_w(0xFFFF_FFFF_A480_0018, 0);
        mem.store_w(0xFFFF_FFFF_A450_000C, 0);
        mem.store_w(0xFFFF_FFFF_A440_0010, 0);
        mem.store_w(0xFFFF_FFFF_A430_0000, 1 << 11);

        let pending = with_host(|host| host.device_fabric.snapshot().mi_pending);
        assert_eq!(pending & 0x3F, 0);
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_vi_registers_drive_half_line_timing_and_shared_mi() {
        crate::test_support::install_complete_render_backend(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A440_0018, 525);
        mem.store_w(0xFFFF_FFFF_A440_000C, 100);
        crate::vi::arm_vi_retrace(1_000);

        crate::advance_virtual_time(190);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 98);
        crate::advance_virtual_time(191);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );

        mem.store_w(0xFFFF_FFFF_A440_0010, 0xFFFF_FFFF);
        assert_eq!(mem.load_w(0xFFFF_FFFF_A440_0010), 100);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A430_0008) as u32 & fn64_runtime::InterruptSource::Vi.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_ai_registers_schedule_the_live_guest_cycle_fifo() {
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        mem.store_w(0xFFFF_FFFF_A450_0008, 1);
        mem.store_w(0xFFFF_FFFF_A450_0010, 151);
        mem.store_w(0xFFFF_FFFF_A450_0000, 0x1000);
        mem.store_w(0xFFFF_FFFF_A450_0004, 0x80);
        assert_ne!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32 & fn64_runtime::AI_STATUS_BUSY,
            0
        );
        let deadline = with_host(|host| host.device_fabric.next_deadline().unwrap().get());
        crate::advance_virtual_time(deadline);
        assert_eq!(
            mem.load_w(0xFFFF_FFFF_A450_000C) as u32,
            fn64_runtime::AI_STATUS_ENABLED
        );
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot().mi_pending)
                & fn64_runtime::InterruptSource::Ai.bit(),
            0
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn typed_raw_si_registers_run_separate_timed_pif_write_and_read_dmas() {
        let mut bytes = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut bytes);
            for (offset, byte) in [(0, 1), (1, 3), (2, 0xFF), (3, 0), (6, 0xFE)] {
                view.write_u8(fn64_runtime::RdramAddr::from_offset(offset), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = bytes.as_mut_ptr();
            host.runtime_rdram_len = bytes.len();
        });
        let previous = fn64_recomp_rs::set_mmio_hooks(Some(read_raw_mmio), Some(write_raw_mmio));
        {
            let mut mem = Rdram::new(&mut bytes);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0010, 0);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) & 1, 1);
        }
        crate::advance_virtual_time(1);
        {
            let mut mem = Rdram::new(&mut bytes);
            assert_eq!(mem.load_w(0xFFFF_FFFF_A480_0018) as u32, 1 << 12);
            mem.store_w(0xFFFF_FFFF_A480_0018, 0);
            mem.store_w(0xFFFF_FFFF_A480_0000, 0);
            mem.store_w(0xFFFF_FFFF_A480_0004, 0);
        }
        crate::advance_virtual_time(2);
        let view = fn64_runtime::RdramView::from_storage(&bytes);
        assert_eq!(
            (3..6)
                .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset)))
                .collect::<Vec<_>>(),
            vec![0x05, 0, 0]
        );
        fn64_recomp_rs::set_mmio_hooks(previous.0, previous.1);
    }

    #[test]
    fn canonical_mutation_state_traps_unjournaled_executable_bytes_before_dispatch() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x100, 0x108)]);
        state.seal_with(|physical| image[(physical - 0x100) as usize]);
        image[3] = 0x5a;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x100) as usize]);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.reconcile_snapshot_before_dispatch(snapshot);
        }))
        .expect_err("unjournaled executable mutation must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("unjournaled executable mutation"));
        assert!(message.contains("0x00000103"));
    }

    #[test]
    fn canonical_instruction_limit_clamps_the_final_dispatch_slice_exactly() {
        let _reset = PublicSiRuntimeStateTestReset;
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());

        let bank = BankId::new(0xc11a);
        let entry = GuestPc::new(0x8000_7000);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, entry, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    bootstrap_return_runner,
                    ProgramArtifactIdentity::new([0xc1; 32]),
                ),
            )
            .unwrap();
        let install = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(bank, entry),
                InstructionBudget::new(4096).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(Vec::new()).unwrap(),
            ProgramArtifactIdentity::new([0xc2; 32]),
        );
        let live = set_catalog_block_program(install, 0x8000);
        let resolver_evidence = live.install.evidence().clone();

        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1720);
        assert_eq!(live.install.evidence(), &resolver_evidence);
        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            set_canonical_block_instruction_limit_v1(Some(2000));
        }))
        .expect_err("an armed exact limit may not be replaced");
        let duplicate = duplicate
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| duplicate.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(duplicate.contains("already armed"));
        live.charge_canonical_instructions(1718);
        assert_eq!(live.next_dispatch_budget().get(), 2);
        live.charge_canonical_instructions(1);
        assert_eq!(live.next_dispatch_budget().get(), 1);

        set_canonical_block_instruction_limit_v1(None);
        assert_eq!(live.next_dispatch_budget().get(), 4096);
        set_canonical_block_instruction_limit_v1(Some(1720));
        assert_eq!(live.next_dispatch_budget().get(), 1);
        live.charge_canonical_instructions(1);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = live.next_dispatch_budget();
        }))
        .expect_err("dispatch may not continue past the exact limit");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("limit 1720 was already reached"));
    }

    #[test]
    fn canonical_mutation_state_hash_chains_exact_channel_and_invalidation() {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().clear());
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x200, 0x208)]);
        state.seal_with(|physical| image[(physical - 0x200) as usize]);
        let initial_root = state.journal_root_sha256;
        image[2..4].copy_from_slice(&[0xaa, 0xbb]);
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.commit_snapshot(
            snapshot,
            vec![GuestWriteEvent::Range {
                channel: WriterChannel::HostAbi,
                physical_offset: 0x202,
                len: 2,
            }],
            vec![GenerationId::new(7)],
        );

        let evidence = state.evidence_snapshot();
        assert!(evidence.sealed);
        assert_ne!(evidence.journal_root_sha256, initial_root);
        assert_eq!(evidence.entries.len(), 1);
        let entry = &evidence.entries[0];
        assert_eq!(entry.sequence, 0);
        assert_eq!(
            entry.declared_writes,
            [AttributedExecutableWriteEvidenceV1 {
                channel: WriterChannel::HostAbi,
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(
            entry.changed_ranges,
            [PendingExecutableWriteEvidenceSnapshot {
                physical_start: 0x202,
                physical_end: 0x204,
            }]
        );
        assert_eq!(entry.invalidated_generations, [GenerationId::new(7)]);
        let stable = state.read_snapshot(|physical| image[(physical - 0x200) as usize]);
        state.reconcile_snapshot_before_dispatch(stable);
    }

    #[test]
    fn canonical_mutation_state_rejects_changes_outside_attributed_range() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x300, 0x308)]);
        state.seal_with(|physical| image[(physical - 0x300) as usize]);
        image[6] = 1;
        let snapshot = state.read_snapshot(|physical| image[(physical - 0x300) as usize]);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel: WriterChannel::RdpRenderer,
                    physical_offset: 0x300,
                    len: 2,
                }],
                Vec::new(),
            );
        }))
        .expect_err("out-of-declaration executable change must trap");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains("outside every attributed writer declaration"));
    }

    #[test]
    fn renderer_transaction_attributes_exact_changed_executable_bytes() {
        let _state = scoped_test_executable_write_preflight_state(vec![(0x40, 0x48)], Vec::new());
        let previous =
            fn64_recomp_rs::set_write_observer(Some(record_executable_and_renderer_write));
        let mut storage = [0u8; 0x80];
        track_rdp_renderer_mutation(&mut storage, |storage| {
            storage[0x41 ^ 3] = 0xaa;
            storage[0x42 ^ 3] = 0xbb;
            storage[0x70 ^ 3] = 0xcc;
        });
        fn64_recomp_rs::set_write_observer(previous);

        assert_eq!(
            PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| pending.borrow().clone()),
            [GuestWriteEvent::Range {
                channel: WriterChannel::RdpRenderer,
                physical_offset: 0x41,
                len: 2,
            }]
        );
    }

    #[test]
    fn same_byte_nested_writers_commit_in_execution_order() {
        let mut image = [0u8; 8];
        let mut state = CanonicalExecutableMutationStateV1::new(&[(0x400, 0x408)]);
        state.seal_with(|physical| image[(physical - 0x400) as usize]);
        let transaction = state.begin_host_transaction(
            7,
            GuestPc::new(0x8000_0400),
            ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_0404)),
        );

        for (value, channel) in [
            (1, WriterChannel::HostAbi),
            (2, WriterChannel::RspExecutionOrHleWriteback),
            (3, WriterChannel::RdpRenderer),
            (4, WriterChannel::HostAbi),
        ] {
            image[1] = value;
            let snapshot = state.read_snapshot(|physical| image[(physical - 0x400) as usize]);
            state.commit_snapshot(
                snapshot,
                vec![GuestWriteEvent::Range {
                    channel,
                    physical_offset: 0x401,
                    len: 1,
                }],
                Vec::new(),
            );
        }
        state.finish_host_transaction(transaction);

        let evidence = state.evidence_snapshot();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(evidence.entries.len(), 4);
        assert_eq!(
            evidence
                .entries
                .iter()
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }

    #[test]
    fn catalog_host_orders_real_rsp_and_rdp_wrappers_on_the_same_byte() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_SYNC_BANK,
                    ordered_sync_runner,
                    ProgramArtifactIdentity::new([0xaf; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_SYNC_BANK, ORDERED_SYNC_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_SYNC_HOST.get(), ordered_sync_host)]).unwrap(),
            ProgramArtifactIdentity::new([0xb0; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_SYNC_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_SYNC_ENTRY),
            0x0adf,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        crate::run_to_idle();
        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        assert_eq!(
            evidence
                .entries
                .iter()
                .skip(1)
                .map(|entry| entry.declared_writes[0].channel)
                .collect::<Vec<_>>(),
            [
                WriterChannel::HostAbi,
                WriterChannel::RspExecutionOrHleWriteback,
                WriterChannel::RdpRenderer,
                WriterChannel::HostAbi,
            ]
        );
        for entry in evidence.entries.iter().skip(1) {
            assert_eq!(
                entry.changed_ranges,
                [PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x7200,
                    physical_end: 0x7201,
                }]
            );
        }
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }
    }

    #[test]
    fn suspended_host_transaction_orders_same_byte_device_write_before_resume_suffix() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        with_host(|host| *host = super::super::HostState::default());
        let words = [0x2402_0001u32, 0x03e0_0008];
        let rom = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        crate::load_rom_with_fixed_pi_latency(rom.clone(), 1);

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY, words.to_vec()).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    ORDERED_WRITER_BANK,
                    ordered_writer_runner,
                    ProgramArtifactIdentity::new([0xad; 32]),
                ),
            )
            .unwrap();
        let resolver = CatalogResolverInstallV1::new(
            CatalogBlockProgramV1::new(
                program,
                ExecutionKey::new(ORDERED_WRITER_BANK, ORDERED_WRITER_ENTRY),
                InstructionBudget::new(2).unwrap(),
            )
            .unwrap(),
            HostFunctionCatalogV1::new(vec![(ORDERED_WRITER_HOST.get(), ordered_writer_host)])
                .unwrap(),
            ProgramArtifactIdentity::new([0xae; 32]),
        );
        let generations = BackedPrecompiledGenerationCatalogV1::new(
            PrecompiledGenerationCatalog::new(),
            Vec::new(),
        )
        .unwrap();
        let install = CatalogGenerationInstallV1::new(resolver, generations).unwrap();
        let mut bootstrap = install
            .begin_bootstrap_import_v1(&rom, bootstrap_test_rdram_len(), fn64_runtime::TvType::Ntsc)
            .unwrap();
        bootstrap
            .publish_resident_rom_image(0, ORDERED_WRITER_ENTRY.get(), 8)
            .unwrap();
        let validated = bootstrap.commit().unwrap();
        let thread_id = 0x0ade;
        boot_thread0_validated_catalog_generation_program_v1(
            validated,
            install,
            test_boot_context(ORDERED_WRITER_ENTRY),
            thread_id,
            10,
        )
        .unwrap();

        assert!(crate::run_one_step());
        assert!(crate::run_one_step());
        let prefix = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert_eq!(prefix.open_host_transactions.len(), 1);
        assert_eq!(
            prefix.entries.last().unwrap().declared_writes[0].channel,
            WriterChannel::HostAbi
        );

        let (rdram, rdram_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
        assert!(!rdram.is_null() && rdram_len > 0x7000);
        unsafe {
            fn64_runtime::RdramPtr::from_storage_ptr(rdram)
                .write_u8(fn64_runtime::RdramAddr::from_offset(0x7000), 2);
        }
        fn64_recomp_rs::notify_pi_dma_write(0x7000, 1);
        process_live_executable_writes_from_host();

        assert!(crate::run_one_step());
        crate::run_to_idle();

        let evidence = catalog_generation_install_evidence_snapshot()
            .unwrap()
            .mutation_journal
            .unwrap();
        assert!(evidence.open_host_transactions.is_empty());
        let channels = evidence
            .entries
            .iter()
            .skip(1)
            .map(|entry| entry.declared_writes[0].channel)
            .collect::<Vec<_>>();
        assert_eq!(
            channels,
            [
                WriterChannel::HostAbi,
                WriterChannel::PiDma,
                WriterChannel::HostAbi
            ]
        );
        for entries in evidence.entries.windows(2) {
            assert_eq!(entries[0].after_sha256, entries[1].before_sha256);
        }

        let storage = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            track_rdp_renderer_mutation(&mut *storage, |_| {
                panic!("synthetic renderer unwind");
            });
        }))
        .expect_err("uncommitted child writer must unwind");
        assert!(unwind
            .downcast_ref::<&str>()
            .is_some_and(|message| *message == "synthetic renderer unwind"));

        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = begin_catalog_nested_writer(&*storage, "post-unwind publication");
        }))
        .expect_err("a later child writer must reject the poisoned owner");
        let message = poisoned
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| poisoned.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(message.contains(
            "canonical executable mutation owner is poisoned: tracked renderer/RSP publication child writer transaction unwound before commit"
        ));
    }
}
