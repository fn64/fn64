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

/// Domain separator for the v2 PAGE-TREE watched-bytes digest.
///
/// v1 was a flat SHA-256 over `(start, end, bytes)` per watched range. v2
/// hashes each fixed-size page separately and binds the page digests through a
/// root. The two are different functions of the same memory, so a v1 value and
/// a v2 value for identical bytes must never be mistaken for one another --
/// this prefix, the page size, and the page count all enter the hashed message,
/// so a v2 root cannot collide with a v1 digest except by SHA-256 collision.
///
/// Recorded digests that predate this constant are v1 and stay v1: see
/// `docs/plans/checkpoint-digest-cost.md`.
pub const CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V2: &str = "fn64.canonical-watched-bytes-digest.v2";

/// Domain separator for the v3 MERKLE-ROOT watched-bytes digest.
///
/// v2 made the LEAVES incremental -- one SHA-256 per 4096-byte page, rehashed
/// only where bytes moved -- but left the ROOT flat: every commit absorbed all
/// 32 bytes of every page digest. On WM2000's 371 pages that is 11,872 bytes
/// re-hashed for a four-byte guest store, and it measured at 20 of the 26
/// self-time points `sha2::compress` still held after v2.
///
/// v3 keeps the v2 leaf function unchanged in shape and replaces the root with
/// a binary Merkle tree per watched range, plus a small flat root over the
/// range roots. A changed leaf now touches ceil(log2(pages)) internal nodes --
/// 9 for WM2000's 370-page range -- instead of the whole leaf vector.
///
/// A v2 root and a v3 root over identical memory are different values and must
/// never be confused. Every hashed message in v3 carries this prefix, and the
/// leaf, internal-node, range-root and top-root messages each carry a distinct
/// tag byte, so no v3 message is a v2 message and no v3 node can be read at
/// another level of the tree.
///
/// Recorded digests that predate this constant are v1 or v2 and stay so: see
/// `docs/plans/checkpoint-digest-cost.md`.
pub const CANONICAL_WATCHED_BYTES_DIGEST_SCHEMA_V3: &str = "fn64.canonical-watched-bytes-digest.v3";

/// Children per internal node in the v3 watched-bytes Merkle tree.
///
/// Two. The update cost of a single changed leaf is `ceil(log_f(n))` node
/// hashes, each absorbing `f * 32` bytes plus a fixed header, so the bytes
/// hashed per commit go as `f/ln(f) * ln(n)` -- minimised at `f = e`, and 2 is
/// the nearest integer above 1. Concretely, for WM2000's 370 pages: `f=2` is 9
/// nodes over 576 payload bytes, `f=4` is 5 nodes over 640, `f=16` is 3 over
/// 1536. Binary also keeps the incremental update a plain parent walk with no
/// sibling gather.
///
/// The value is hashed into every internal node and both root levels, so it
/// cannot change without changing the digest -- a schema change, not a tuning
/// knob.
pub const CANONICAL_WATCHED_BYTES_FANOUT_V3: usize = 2;

/// Bytes per page in the v2 watched-bytes digest.
///
/// 4096 bytes. The choice is bounded on both sides and the middle is flat:
///
/// - Too small and the per-page fixed cost dominates. Each page costs a
///   `Sha256::new`, a 30-byte prefix, three integer fields and a `finalize` --
///   roughly 2 compression blocks of overhead. At 4 KiB the payload is 64
///   blocks, so overhead is ~3%. At 256 B it is ~33%, and the ROOT hash also
///   grows: it covers 32 bytes per page, so a 1.44 MiB region at 256 B pages
///   makes the root itself a 184 KiB hash -- re-run on EVERY commit, which is
///   precisely the cost being removed.
/// - Too large and a one-word store re-hashes more than it must. WM2000's
///   observed writes are single stores, so the recompute is one page: 4 KiB
///   instead of 1.44 MiB is a 369x reduction in hashed bytes.
///
/// At 4 KiB, WM2000's 1,513,056-byte range is 370 pages, so the root hashes
/// 11,840 bytes -- under 1% of the flat digest -- and a single-store commit
/// hashes one 4 KiB page plus that root.
///
/// A power of two, so the page index of an offset is a shift rather than a
/// division. The value is part of the hashed message, so changing it later is a
/// schema change and cannot happen silently.
pub const CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2: usize = 4096;

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
    /// The sealed baseline in LOGICAL guest byte order.
    ///
    /// This is the evidence-bearing form: `digest_snapshot` hashes it, so its
    /// order is part of every receipt and must not change.
    expected: Vec<u8>,
    /// The same baseline in RDRAM STORAGE order, kept in lockstep by
    /// [`WatchedExecutableBytesV1::set_expected`].
    ///
    /// Storage holds native words, so logical byte `n` lives at storage index
    /// `n ^ 3` -- within an aligned word, a 4-byte reversal. Comparing the
    /// live region against the baseline therefore did not need the logical
    /// copy it was making: it allocated 1 MiB, `copy_from_slice`d into it,
    /// reversed 262,144 words, and only then ran the `memcmp` that answers
    /// "unchanged" -- four passes over 1 MiB per dispatch boundary, to reach a
    /// predicate one pass can decide.
    ///
    /// Holding the pre-reversed mirror makes that predicate a direct `memcmp`
    /// against the storage slice. It is derived state, never evidence: nothing
    /// hashes it, and `set_expected` is the only writer, so the two forms
    /// cannot drift.
    expected_storage_order: Vec<u8>,
    /// The v3 Merkle tree over the pages of `expected`, level by level.
    ///
    /// `levels[0]` is the leaf level: one v3 page digest per
    /// [`CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2`]-byte page. `levels[h+1]` is
    /// built from `levels[h]` by pairing adjacent nodes; an odd trailing node
    /// is promoted through a distinct single-child message. The last level
    /// holds exactly one node, the apex, unless the range has no pages at all
    /// -- in which case the tree is empty and the range root binds the absence.
    ///
    /// Derived state, like `expected_storage_order`: it is always exactly what
    /// recomputing every page of `expected` and rebuilding every level would
    /// produce. The dirty leaves are decided by byte comparison against the old
    /// baseline, and every ancestor of a dirty leaf is recomputed, so a stale
    /// node is not representable through the normal path.
    ///
    /// Empty until the baseline is first set.
    expected_page_tree: WatchedPageTreeV3,
}

/// The maintained v3 Merkle tree over one watched range's pages.
///
/// Held level by level rather than as a heap array because the levels of a
/// non-power-of-two tree have irregular sizes, and an explicit `Vec` per level
/// makes the parent index a plain `i / 2` at every height with no padding
/// leaves. Padding was rejected deliberately: filling to a power of two would
/// have to hash something for the pad slots, and any such filler is a value an
/// attacker could aim a real leaf at.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct WatchedPageTreeV3 {
    /// `levels[0]` = leaves. Empty when the range has no pages.
    levels: Vec<Vec<[u8; 32]>>,
}

impl WatchedPageTreeV3 {
    fn leaves(&self) -> &[[u8; 32]] {
        self.levels.first().map(Vec::as_slice).unwrap_or(&[])
    }

    fn leaves_mut(&mut self) -> &mut Vec<[u8; 32]> {
        if self.levels.is_empty() {
            self.levels.push(Vec::new());
        }
        &mut self.levels[0]
    }

    /// The apex: the single node of the top level, or `None` for no pages.
    fn apex(&self) -> Option<&[u8; 32]> {
        let last = self.levels.last()?;
        debug_assert!(last.len() <= 1 || self.levels.len() == 1);
        last.first()
    }

    /// Number of levels a tree over `leaves` leaves has, including the leaf
    /// level. Zero leaves means zero levels.
    fn level_count(leaves: usize) -> usize {
        let mut count = 0usize;
        let mut width = leaves;
        while width > 0 {
            count += 1;
            if width == 1 {
                break;
            }
            width = width.div_ceil(CANONICAL_WATCHED_BYTES_FANOUT_V3);
        }
        count
    }

    /// Rebuild every level above the leaves, from scratch.
    ///
    /// Used at seal and whenever the page count changes. The incremental form
    /// is [`Self::recompute_ancestors`].
    fn rebuild_upper_levels(&mut self, physical_start: u32, physical_end: u32) {
        let leaves = self.leaves().len();
        self.levels.truncate(1);
        if leaves == 0 {
            self.levels.clear();
            return;
        }
        self.levels.reserve(Self::level_count(leaves));
        let mut height = 0u32;
        while self.levels[self.levels.len() - 1].len() > 1 {
            let below = &self.levels[self.levels.len() - 1];
            height += 1;
            let mut level = Vec::with_capacity(below.len().div_ceil(2));
            let mut index = 0u32;
            let mut pair = below.chunks(CANONICAL_WATCHED_BYTES_FANOUT_V3);
            while let Some(children) = pair.next() {
                level.push(receipts::watched_node_digest_v3(
                    physical_start,
                    physical_end,
                    height,
                    index,
                    &children[0],
                    children.get(1),
                ));
                index += 1;
            }
            self.levels.push(level);
        }
    }

    /// Recompute the ancestors of the leaves in `dirty`, bottom up.
    ///
    /// `dirty` must be sorted and deduplicated. Each level's dirty set is the
    /// parent indices of the level below, so a single changed leaf touches
    /// exactly one node per level -- `ceil(log2(pages))` hashes -- and a
    /// clustered set of changed leaves shares ancestors instead of rehashing
    /// them once each.
    ///
    /// CONSERVATIVE BY CONSTRUCTION, in the same sense as the leaf refresh: the
    /// caller decides `dirty` by byte comparison, and EVERY ancestor of every
    /// dirty leaf is recomputed here without exception. A node whose entire
    /// subtree is clean has, by that comparison, identical leaves, and
    /// identical children produce an identical node.
    fn recompute_ancestors(&mut self, physical_start: u32, physical_end: u32, dirty: &[usize]) {
        if dirty.is_empty() || self.levels.len() < 2 {
            return;
        }
        let mut below_dirty: Vec<usize> = dirty.to_vec();
        let mut parents: Vec<usize> = Vec::with_capacity(below_dirty.len());
        for height in 1..self.levels.len() {
            parents.clear();
            let mut last: Option<usize> = None;
            for &child in &below_dirty {
                let parent = child / CANONICAL_WATCHED_BYTES_FANOUT_V3;
                if last != Some(parent) {
                    parents.push(parent);
                    last = Some(parent);
                }
            }
            let (lower, upper) = self.levels.split_at_mut(height);
            let below = &lower[height - 1];
            let level = &mut upper[0];
            for &parent in &parents {
                let lo = parent * CANONICAL_WATCHED_BYTES_FANOUT_V3;
                level[parent] = receipts::watched_node_digest_v3(
                    physical_start,
                    physical_end,
                    height as u32,
                    parent as u32,
                    &below[lo],
                    below.get(lo + 1),
                );
            }
            std::mem::swap(&mut below_dirty, &mut parents);
        }
    }
}

impl WatchedExecutableBytesV1 {
    /// Replace the baseline, refreshing the storage-order mirror with it.
    ///
    /// The single writer of `expected`, so the mirror cannot go stale.
    /// Returns the buffer it replaced so the caller can recycle it.
    #[must_use]
    fn set_expected(&mut self, bytes: Vec<u8>) -> Vec<u8> {
        self.refresh_page_digests(&bytes);
        self.expected_storage_order.clear();
        self.expected_storage_order.extend_from_slice(&bytes);
        // Mirror `RdramView::copy_logical_bytes`' mapping exactly: only the
        // word-aligned body is a reversal, and the unaligned head/tail bytes
        // are compared per byte by `matches_storage` rather than reversed here.
        let head = Self::head_len(self.physical_start, bytes.len());
        let body = (bytes.len() - head) & !3;
        for word in self.expected_storage_order[head..head + body].chunks_exact_mut(4) {
            word.reverse();
        }
        std::mem::replace(&mut self.expected, bytes)
    }

    /// Number of v2 pages covering `len` bytes.
    fn page_count(len: usize) -> usize {
        len.div_ceil(CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2)
    }

    /// Bring `expected_page_digests` in line with `bytes`, rehashing only the
    /// pages whose bytes actually differ from the current baseline.
    ///
    /// CONSERVATIVE BY CONSTRUCTION. The dirty set is decided here, by
    /// comparing the incoming bytes against `expected` page by page -- not by
    /// trusting a writer's declaration, not by consuming
    /// `current_changed_ranges`, and not by any dirty flag maintained
    /// elsewhere. A page is skipped only when its bytes are *equal*, proven by
    /// a `memcmp` at the moment of the update, and equal bytes have an equal
    /// digest by definition. There is no path by which a changed page keeps a
    /// stale digest, because nothing other than byte equality can cause a skip.
    ///
    /// The whole-baseline cases -- first seal, or a length change -- rehash
    /// every page.
    fn refresh_page_digests(&mut self, bytes: &[u8]) {
        let pages = Self::page_count(bytes.len());
        let reusable =
            self.expected_page_tree.leaves().len() == pages && self.expected.len() == bytes.len();
        if !reusable {
            let (physical_start, physical_end) = (self.physical_start, self.physical_end);
            let leaves = self.expected_page_tree.leaves_mut();
            leaves.clear();
            leaves.reserve(pages);
            for index in 0..pages {
                let lo = index * CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2;
                let hi = (lo + CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2).min(bytes.len());
                leaves.push(receipts::watched_page_digest_v3(
                    physical_start,
                    physical_end,
                    index as u32,
                    &bytes[lo..hi],
                ));
            }
            self.expected_page_tree
                .rebuild_upper_levels(physical_start, physical_end);
            return;
        }
        let mut dirty: Vec<usize> = Vec::new();
        for index in 0..pages {
            let lo = index * CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2;
            let hi = (lo + CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2).min(bytes.len());
            if self.expected[lo..hi] == bytes[lo..hi] {
                continue;
            }
            self.expected_page_tree.leaves_mut()[index] = receipts::watched_page_digest_v3(
                self.physical_start,
                self.physical_end,
                index as u32,
                &bytes[lo..hi],
            );
            dirty.push(index);
        }
        self.expected_page_tree
            .recompute_ancestors(self.physical_start, self.physical_end, &dirty);
    }

    /// Bytes before the first word-aligned storage word, as `copy_logical_bytes` computes it.
    fn head_len(physical_start: u32, len: usize) -> usize {
        (((4 - (physical_start % 4)) % 4) as usize).min(len)
    }

    /// Append the ranges where live storage differs from the baseline.
    ///
    /// Exactly the ranges `current_changed_ranges` derives from
    /// `read_snapshot_from_view`, decided without materializing the snapshot.
    /// `matches_view` already answers the BOOLEAN form of this question in one
    /// `memcmp` per range; this is the same comparison carried far enough to
    /// name the differing bytes, so the commit path no longer has to copy and
    /// word-reverse 1.44 MiB just to find the handful of bytes that moved.
    ///
    /// LANE MAPPING. Logical byte `n` lives at storage index `n ^ 3`. Across
    /// the word-aligned body that XOR is precisely a 4-byte reversal, so the
    /// body is compared as raw storage against the pre-reversed
    /// `expected_storage_order` mirror and only the words that differ are
    /// walked -- and inside such a word, storage lane `k` is logical lane
    /// `3 - k`. The at-most-three head and tail bytes stay on the same
    /// per-byte `read_u8` path `copy_logical_bytes` uses, so an unaligned
    /// range cannot be decided by a different rule than the copy would apply.
    ///
    /// Returns `false` when storage is out of range, leaving `out` as it found
    /// it: the caller then falls back to the copying path so the panic an
    /// unmapped byte owes is raised by exactly the code that raised it before.
    #[must_use]
    fn changed_ranges_into(
        &self,
        view: &fn64_runtime::RdramView<'_>,
        out: &mut Vec<(u32, u32)>,
    ) -> bool {
        let len = self.expected.len();
        debug_assert_eq!(len, (self.physical_end - self.physical_start) as usize);
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        let base = fn64_runtime::RdramAddr::from_offset(self.physical_start);

        // One coalescing sink for all three lanes. `current_changed_ranges`
        // emits maximal runs of consecutive differing LOGICAL bytes, and a run
        // can cross the head/body and body/tail seams, so the runs cannot be
        // closed per lane -- they are closed when a byte matches or the region
        // ends.
        let first = out.len();
        let physical_start = self.physical_start;
        let push = |out: &mut Vec<(u32, u32)>, index: usize| {
            let physical = physical_start + index as u32;
            // Extend the open run only if it is one THIS range opened: a run
            // from an earlier watched range must never absorb a byte from this
            // one, even where the two happen to abut.
            if out.len() > first {
                if let Some((_, end)) = out.last_mut() {
                    if *end == physical {
                        *end = physical + 1;
                        return;
                    }
                }
            }
            out.push((physical, physical + 1));
        };

        for index in 0..head {
            let addr = match base.checked_add(index as u32) {
                Some(addr) => addr,
                None => {
                    out.truncate(first);
                    return false;
                }
            };
            if view.read_u8(addr) != self.expected[index] {
                push(out, index);
            }
        }

        if body > 0 {
            let start = self.physical_start as usize + head;
            let Some(live) = view.storage_slice(start, body) else {
                out.truncate(first);
                return false;
            };
            let mirror = &self.expected_storage_order[head..head + body];
            // The overwhelmingly common answer is "nothing changed", and the
            // whole body settles that in one `memcmp`. Only when it fails does
            // anything walk words.
            if live != mirror {
                // Chunk the scan so equal stretches are skipped a `memcmp` at a
                // time rather than a word at a time. Chunk boundaries cannot
                // affect the result: every differing byte is still visited, and
                // `push` coalesces across them.
                const CHUNK: usize = 256;
                let mut word = 0;
                let words = body / 4;
                while word < words {
                    let chunk = CHUNK.min(words - word);
                    if live[word * 4..(word + chunk) * 4]
                        == mirror[word * 4..(word + chunk) * 4]
                    {
                        word += chunk;
                        continue;
                    }
                    for word in word..word + chunk {
                        let at = word * 4;
                        if live[at..at + 4] == mirror[at..at + 4] {
                            continue;
                        }
                        // Storage lane `k` of an aligned word is logical lane
                        // `3 - k`, so walk the lanes in logical order.
                        for lane in 0..4 {
                            if live[at + 3 - lane] != mirror[at + 3 - lane] {
                                push(out, head + at + lane);
                            }
                        }
                    }
                    word += chunk;
                }
            }
        }

        for index in head + body..len {
            let addr = match base.checked_add(index as u32) {
                Some(addr) => addr,
                None => {
                    out.truncate(first);
                    return false;
                }
            };
            if view.read_u8(addr) != self.expected[index] {
                push(out, index);
            }
        }
        true
    }

    /// Refresh the baseline over only the ranges that changed.
    ///
    /// `set_expected` rewrites all three baseline forms over the whole watched
    /// region -- 1.44 MiB of `expected`, 1.44 MiB of `expected_storage_order`,
    /// and a `memcmp` per page to find the dirty pages. When the changed set is
    /// known that is almost entirely wasted: the bytes outside it are already
    /// correct in both mirrors, and their page digests are already correct too.
    ///
    /// `changed` is in physical addresses, ascending, disjoint, and clipped to
    /// this range by construction (it comes from `changed_ranges_into` on this
    /// same range). Bytes are read from the view with the same lane mapping
    /// `copy_logical_bytes` applies, so `expected` lands byte-identical to what
    /// the full-copy path would have produced -- which is the invariant the
    /// mutation journal's guarantee rests on.
    fn apply_changed_from_view(
        &mut self,
        view: &fn64_runtime::RdramView<'_>,
        changed: &[(u32, u32)],
    ) {
        if changed.is_empty() {
            return;
        }
        for &(physical_start, physical_end) in changed {
            debug_assert!(
                self.physical_start <= physical_start && physical_end <= self.physical_end
            );
            let lo = (physical_start - self.physical_start) as usize;
            let hi = (physical_end - self.physical_start) as usize;
            view.copy_logical_bytes(
                fn64_runtime::RdramAddr::from_offset(physical_start),
                &mut self.expected[lo..hi],
            );
        }
        // Rebuild the storage-order mirror over the same spans, from the
        // freshly updated logical bytes, so the two forms cannot drift.
        //
        // Widened to whole words across the body because the mirror is
        // word-reversed there: a partial word cannot be reversed in isolation.
        // Widening only re-derives bytes that are already correct, so it
        // changes no value.
        let len = self.expected.len();
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        for &(physical_start, physical_end) in changed {
            let lo = (physical_start - self.physical_start) as usize;
            let hi = (physical_end - self.physical_start) as usize;
            // Head and tail lanes are stored un-reversed.
            for index in (lo..hi.min(head)).chain((lo.max(head + body))..hi) {
                self.expected_storage_order[index] = self.expected[index];
            }
            let word_lo = lo.max(head).min(head + body);
            let word_hi = hi.max(head).min(head + body);
            if word_lo >= word_hi {
                continue;
            }
            let word_lo = head + ((word_lo - head) & !3);
            let word_hi = head + (word_hi - head).div_ceil(4) * 4;
            self.expected_storage_order[word_lo..word_hi]
                .copy_from_slice(&self.expected[word_lo..word_hi]);
            for word in self.expected_storage_order[word_lo..word_hi].chunks_exact_mut(4) {
                word.reverse();
            }
        }
        self.refresh_page_digests_over(changed);
    }

    /// Rehash exactly the pages the changed ranges touch.
    ///
    /// Same contract as [`Self::refresh_page_digests`] and same conservatism,
    /// reached from the other direction: that form finds the dirty pages by
    /// comparing every page against the OLD baseline, which it can only do
    /// while both baselines exist. Here `expected` has already been updated in
    /// place, so the dirty set comes from `changed` -- which is itself derived
    /// from a byte-for-byte comparison of live storage against the old
    /// baseline, not from any writer's declaration. A page outside every
    /// changed range has, by that comparison, identical bytes, and identical
    /// bytes have an identical digest.
    fn refresh_page_digests_over(&mut self, changed: &[(u32, u32)]) {
        let len = self.expected.len();
        debug_assert_eq!(self.expected_page_tree.leaves().len(), Self::page_count(len));
        let mut dirty: Vec<usize> = Vec::new();
        let mut previous: Option<usize> = None;
        for &(physical_start, physical_end) in changed {
            let lo = (physical_start - self.physical_start) as usize;
            let hi = (physical_end - self.physical_start) as usize;
            let first = lo / CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2;
            let last = (hi - 1) / CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2;
            for index in first..=last {
                // Adjacent changed ranges routinely share a page; hashing it
                // twice is correct but wasteful, and the ranges are ascending
                // so the previous index is the only possible repeat.
                if previous == Some(index) {
                    continue;
                }
                previous = Some(index);
                let page_lo = index * CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2;
                let page_hi = (page_lo + CANONICAL_WATCHED_BYTES_PAGE_BYTES_V2).min(len);
                self.expected_page_tree.leaves_mut()[index] = receipts::watched_page_digest_v3(
                    self.physical_start,
                    self.physical_end,
                    index as u32,
                    &self.expected[page_lo..page_hi],
                );
                dirty.push(index);
            }
        }
        // `changed` is ascending and the per-range page spans are visited in
        // order, so `dirty` is already sorted and deduplicated -- the property
        // `recompute_ancestors` requires.
        debug_assert!(dirty.windows(2).all(|pair| pair[0] < pair[1]));
        self.expected_page_tree
            .recompute_ancestors(self.physical_start, self.physical_end, &dirty);
    }

    /// The v3 root of this range: the apex bound to the range's own geometry.
    fn range_root_v3(&self) -> [u8; 32] {
        receipts::watched_range_root_digest_v3(
            self.physical_start,
            self.physical_end,
            self.expected_page_tree.leaves().len() as u64,
            self.expected_page_tree.apex(),
        )
    }

    /// Whether live RDRAM storage still equals the sealed baseline.
    ///
    /// Exactly the predicate `expected == read_snapshot_from_view(..)` computes,
    /// without materializing the snapshot: the aligned body is one `memcmp`
    /// against the pre-reversed mirror, and the at-most-three head and tail
    /// bytes stay on the same per-byte lane-XOR path the copy uses, so an
    /// unaligned range cannot be decided by a different rule than it was
    /// before.
    fn matches_storage(&self, view: &fn64_runtime::RdramView<'_>) -> bool {
        let len = self.expected.len();
        debug_assert_eq!(len, (self.physical_end - self.physical_start) as usize);
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        let base = fn64_runtime::RdramAddr::from_offset(self.physical_start);
        for index in (0..head).chain(head + body..len) {
            let addr = match base.checked_add(index as u32) {
                Some(addr) => addr,
                None => return false,
            };
            if view.read_u8(addr) != self.expected[index] {
                return false;
            }
        }
        if body == 0 {
            return true;
        }
        let start = self.physical_start as usize + head;
        match view.storage_slice(start, body) {
            Some(live) => live == &self.expected_storage_order[head..head + body],
            // Out of range: fall back to reporting a difference so the copying
            // path runs and raises the panic it owes for an unmapped byte.
            None => false,
        }
    }

    /// Widen dirty spans to whole storage words and merge the result.
    ///
    /// Two separate jobs that must happen in this order.
    ///
    /// WIDENING is forced by the storage-order mirror: across the body it is
    /// word-reversed, so a partial word cannot be compared against it in
    /// isolation. Widening only brings in bytes already known equal, so it
    /// changes no verdict.
    ///
    /// MERGING is forced by widening. Two spans that fall in the same storage
    /// word both widen to cover it, and a per-span loop would then visit that
    /// word twice -- emitting its differing bytes twice, and producing a
    /// changed-range list with duplicated and out-of-order entries that does
    /// not equal what the full scan produces. Merging after widening makes
    /// every word belong to exactly one span, which restores the invariant the
    /// coalescing `push` depends on: spans arrive ascending and disjoint, so a
    /// run of differing bytes is walked once, in order.
    ///
    /// (Found by `barrier_restricted_changed_ranges_match_the_full_scan`, which
    /// is exactly the failure it exists to catch.)
    fn word_align_spans(&self, spans: &[(u32, u32)]) -> Vec<(u32, u32)> {
        let len = self.expected.len();
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(spans.len());
        for &(span_start, span_end) in spans {
            let lo = (span_start - self.physical_start) as usize;
            let hi = (span_end - self.physical_start) as usize;
            // Widen only the part that lands in the word-reversed body; the
            // head and tail lanes are stored un-reversed and are compared per
            // byte, so they need no widening.
            let mut wide_lo = lo;
            let mut wide_hi = hi;
            let body_lo = lo.max(head).min(head + body);
            let body_hi = hi.max(head).min(head + body);
            if body_lo < body_hi {
                wide_lo = wide_lo.min(head + ((body_lo - head) & !3));
                wide_hi = wide_hi.max(head + (body_hi - head).div_ceil(4) * 4);
            }
            let widened = (
                self.physical_start + wide_lo as u32,
                self.physical_start + wide_hi as u32,
            );
            match out.last_mut() {
                Some((_, end)) if *end >= widened.0 => *end = (*end).max(widened.1),
                _ => out.push(widened),
            }
        }
        out
    }

    /// [`Self::matches_storage`], reading only the spans the barrier reported.
    ///
    /// # Why this decides the same question
    ///
    /// The barrier was armed at a boundary that had just PROVEN this range
    /// equals its baseline, and a byte of an `mprotect(PROT_READ)` page cannot
    /// change without a write fault the handler recorded. So for every byte
    /// outside `spans`:
    ///
    ///   - it equalled `expected` when the barrier armed, and
    ///   - it has not been written since,
    ///
    /// therefore it still equals `expected`, and comparing it would be a
    /// decided question. Only the bytes inside `spans` are undecided, and those
    /// are compared here by the same code, byte for byte, as the full scan.
    ///
    /// `spans` is a SUPERSET of what actually changed -- a store that rewrites
    /// a byte with its own value faults but changes nothing, and a fault marks
    /// a whole 16 KiB page. That direction is the safe one: it can only cause
    /// bytes to be compared that need not have been, never the reverse.
    ///
    /// `spans` must be ascending, disjoint, and already clipped to this range.
    fn matches_storage_within(
        &self,
        view: &fn64_runtime::RdramView<'_>,
        spans: &[(u32, u32)],
    ) -> bool {
        if spans.is_empty() {
            // The barrier proved no page of this range was written since it
            // armed over a region equal to the baseline. Nothing to compare.
            return true;
        }
        let len = self.expected.len();
        debug_assert_eq!(len, (self.physical_end - self.physical_start) as usize);
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        let base = fn64_runtime::RdramAddr::from_offset(self.physical_start);
        for &(span_start, span_end) in self.word_align_spans(spans).iter() {
            debug_assert!(
                self.physical_start <= span_start && span_end <= self.physical_end,
                "dirty span is not clipped to this watched range"
            );
            let lo = (span_start - self.physical_start) as usize;
            let hi = (span_end - self.physical_start) as usize;

            // Head and tail lanes: the same per-byte `read_u8` path the full
            // scan uses, so an unaligned edge cannot be decided by a different
            // rule here than there.
            for index in (lo..hi.min(head)).chain(lo.max(head + body)..hi) {
                let addr = match base.checked_add(index as u32) {
                    Some(addr) => addr,
                    None => return false,
                };
                if view.read_u8(addr) != self.expected[index] {
                    return false;
                }
            }

            // Body: the same `memcmp` against the pre-reversed mirror, over the
            // span only. Already word-aligned by `word_align_spans`.
            let word_lo = lo.max(head).min(head + body);
            let word_hi = hi.max(head).min(head + body);
            if word_lo >= word_hi {
                continue;
            }
            let start = self.physical_start as usize + word_lo;
            match view.storage_slice(start, word_hi - word_lo) {
                Some(live) => {
                    if live != &self.expected_storage_order[word_lo..word_hi] {
                        return false;
                    }
                }
                // Unmapped: report a difference so the copying path runs and
                // raises the panic an unmapped byte owes, exactly as the full
                // scan does.
                None => return false,
            }
        }
        true
    }

    /// [`Self::changed_ranges_into`], reading only the spans the barrier
    /// reported.
    ///
    /// Same argument as [`Self::matches_storage_within`]: bytes outside `spans`
    /// are provably still equal to the baseline, so they cannot contribute a
    /// changed range. Inside the spans this delegates to the very same
    /// comparison the full scan runs, so the ranges it names are identical --
    /// which the equivalence test asserts over randomized contents rather than
    /// taking on the strength of this paragraph.
    ///
    /// One subtlety this must preserve: `changed_ranges_into` coalesces
    /// maximal runs of consecutive differing LOGICAL bytes, and a run can cross
    /// a span boundary. Spans are whole pages (or unions of them) and are
    /// merged before arrival, so two adjacent differing bytes are never split
    /// across two spans unless the pages themselves abut -- and `push` below
    /// coalesces across that seam by comparing against the open run's end,
    /// exactly as the full scan does across its chunk boundaries.
    #[must_use]
    fn changed_ranges_within(
        &self,
        view: &fn64_runtime::RdramView<'_>,
        spans: &[(u32, u32)],
        out: &mut Vec<(u32, u32)>,
    ) -> bool {
        if spans.is_empty() {
            return true;
        }
        let len = self.expected.len();
        debug_assert_eq!(len, (self.physical_end - self.physical_start) as usize);
        let head = Self::head_len(self.physical_start, len);
        let body = (len - head) & !3;
        let base = fn64_runtime::RdramAddr::from_offset(self.physical_start);

        let first = out.len();
        let physical_start = self.physical_start;
        let push = |out: &mut Vec<(u32, u32)>, index: usize| {
            let physical = physical_start + index as u32;
            if out.len() > first {
                if let Some((_, end)) = out.last_mut() {
                    if *end == physical {
                        *end = physical + 1;
                        return;
                    }
                }
            }
            out.push((physical, physical + 1));
        };

        for &(span_start, span_end) in self.word_align_spans(spans).iter() {
            debug_assert!(
                self.physical_start <= span_start && span_end <= self.physical_end,
                "dirty span is not clipped to this watched range"
            );
            let lo = (span_start - self.physical_start) as usize;
            let hi = (span_end - self.physical_start) as usize;

            for index in lo..hi.min(head) {
                let addr = match base.checked_add(index as u32) {
                    Some(addr) => addr,
                    None => {
                        out.truncate(first);
                        return false;
                    }
                };
                if view.read_u8(addr) != self.expected[index] {
                    push(out, index);
                }
            }

            // Already word-aligned and merged by `word_align_spans`, so each
            // storage word belongs to exactly one span and is visited once.
            let word_lo = lo.max(head).min(head + body);
            let word_hi = hi.max(head).min(head + body);
            if word_lo < word_hi {
                let start = self.physical_start as usize + word_lo;
                let Some(live) = view.storage_slice(start, word_hi - word_lo) else {
                    out.truncate(first);
                    return false;
                };
                let mirror = &self.expected_storage_order[word_lo..word_hi];
                if live != mirror {
                    let words = (word_hi - word_lo) / 4;
                    for word in 0..words {
                        let at = word * 4;
                        if live[at..at + 4] == mirror[at..at + 4] {
                            continue;
                        }
                        // Storage lane `k` of an aligned word is logical lane
                        // `3 - k`, so walk the lanes in logical order.
                        for lane in 0..4 {
                            if live[at + 3 - lane] != mirror[at + 3 - lane] {
                                push(out, word_lo + at + lane);
                            }
                        }
                    }
                }
            }

            for index in lo.max(head + body)..hi {
                let addr = match base.checked_add(index as u32) {
                    Some(addr) => addr,
                    None => {
                        out.truncate(first);
                        return false;
                    }
                };
                if view.read_u8(addr) != self.expected[index] {
                    push(out, index);
                }
            }
        }
        true
    }
}

struct CanonicalExecutableMutationStateV1 {
    watched: Vec<WatchedExecutableBytesV1>,
    /// The changed set in flight, parked here for the duration of one commit.
    ///
    /// `commit_changed` needs the list both to build the journal entry and to
    /// hand to the baseline-adoption closure, and that closure takes `&mut
    /// Self` -- so the list cannot simultaneously be borrowed out of a local.
    /// Parking it on the state for the length of the call is what lets both
    /// commit forms share one body. Always empty between commits; nothing
    /// reads it outside `commit_changed` and the closures it invokes.
    pending_changed: Vec<(u32, u32)>,
    /// Retired baseline buffers, reused by `read_snapshot_from_view`.
    ///
    /// Pure allocator hygiene: nothing reads their contents, they are cleared
    /// on return and fully overwritten before use. `RefCell` because the
    /// snapshot read runs behind a shared borrow of the state.
    recycled: RefCell<Vec<Vec<u8>>>,
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
    /// `osEPiWriteIo(OSPiHandle *, u32 devAddr, u32 data)`.
    ///
    /// Programmed single-word device IO, the path a FlashRAM title uses to
    /// issue save-media commands. An SRAM title never links it.
    OsEPiWriteIo,
    /// `osEPiReadIo(OSPiHandle *, u32 devAddr, u32 *data)`, the read
    /// counterpart used for command status and identity polls.
    OsEPiReadIo,
    /// `osFlashInit(void) -> OSPiHandle *`.
    OsFlashInit,
    /// `osFlashSectorErase(u32 page_num) -> s32`.
    OsFlashSectorErase,
    /// `osFlashReadArray(OSIoMesg *, s32, u32, void *, u32, OSMesgQueue *)`.
    OsFlashReadArray,
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
mod host_memory;
mod runners;

// `execution`, `receipts`, `runners`, and `snapshots` carry this module's
// public surface, so their globs stay `pub use`. `validation` holds only
// crate-internal validators, so it re-exports nothing public; it is imported
// (not re-exported) purely so sibling modules reach those validators through
// their own `use super::*`. `live_program` needs no glob at all: it declares
// only `impl` blocks for types declared here, and inherent impls are always
// in scope with their type.
pub use execution::*;
pub use host_memory::{declare_guest_physical_write, read_guest_physical, write_guest_physical};
pub use receipts::*;
pub use runners::*;
pub use snapshots::*;
use validation::*;

#[cfg(test)]
mod tests;
