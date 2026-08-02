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

/// Which executable-write queue is non-empty, if either.
///
/// Nine call sites guard on writer quiescence before sealing a receipt, and
/// each raises its own channel-specific error enum, so only the predicate is
/// shared -- the caller keeps its own `Err(...)`. Five of the nine had a named
/// `validate_*_writer_quiescence` wrapper and four (SI, SP, the bootstrap
/// receipt, and the SP begin-path) open-coded the same two `.with(|pending|
/// !pending.borrow().is_empty())` reads.
///
/// The value here is coupling rather than line count: it reduces the number of
/// places that reach into `PENDING_EXECUTABLE_WRITES` and
/// `PENDING_ATTRIBUTED_EXECUTABLE_WRITES` from nine to one, which is what makes
/// those thread-locals movable later.
///
/// Physical writes are reported before attributed ones, matching the order
/// every existing site checked them in.
fn pending_executable_write_violation() -> Option<PendingWriteViolation> {
    if PENDING_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Some(PendingWriteViolation::Physical);
    }
    if PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|pending| !pending.borrow().is_empty()) {
        return Some(PendingWriteViolation::Attributed);
    }
    None
}

/// The queue [`pending_executable_write_violation`] found non-empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingWriteViolation {
    Physical,
    Attributed,
}

/// Mint the next epoch identity for one writer channel.
///
/// The six counters above stay six distinct statics -- see the interleaving
/// note there -- because each channel needs its own identity space. Only the
/// minting code is shared: it was six byte-identical bodies differing solely
/// in which static they read and which channel they named on overflow.
///
/// `channel` appears only in the overflow panic, so a caller that passes the
/// wrong name degrades a diagnostic rather than the identity itself.
fn next_writer_trace_epoch_id(counter: &'static AtomicU64, channel: &'static str) -> u64 {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch_id| {
            epoch_id.checked_add(1)
        })
        .unwrap_or_else(|_| panic!("{channel} writer trace epoch identity overflow"))
}

fn next_sp_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(&NEXT_SP_WRITER_TRACE_EPOCH_ID, "SP")
}

fn next_cpu_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(
        &NEXT_CPU_WRITER_TRACE_EPOCH_ID,
        "CPU instruction-store",
    )
}

fn next_pi_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(&NEXT_PI_WRITER_TRACE_EPOCH_ID, "PI")
}

fn next_host_abi_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(&NEXT_HOST_ABI_WRITER_TRACE_EPOCH_ID, "Host ABI")
}

fn next_rsp_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(&NEXT_RSP_WRITER_TRACE_EPOCH_ID, "RSP")
}

fn next_rdp_renderer_writer_trace_epoch_id() -> u64 {
    next_writer_trace_epoch_id(&NEXT_RDP_RENDERER_WRITER_TRACE_EPOCH_ID, "RDP renderer")
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

mod receipts;
mod validation;
mod live_program;
mod snapshots;
mod execution;
mod runners;

#[cfg(test)]
mod tests;
