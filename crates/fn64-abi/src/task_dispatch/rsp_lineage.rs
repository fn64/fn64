
// The writer-trace observation seam below is exercised today only from the
// task_dispatch test tree, so the non-test build sees it dead. It stays: it
// is the non-forgeable dispatch observation window, and deleting it would
// force tests back into this file.
#![allow(dead_code)]
use super::*;

/// Canonical ABI owner for the RSP interpreter registers that are not stored
/// in [`fn64_runtime::LiveDeviceFabric`].
///
/// The device fabric owns DMEM, IMEM, PC, and the guest-visible SP/DPC
/// register image. This value owns the scalar register file, complete vector
/// unit, branch/overlay continuation latches, and a matching copy of the
/// device latches needed to restore one interpreter atomically. Diagnostic
/// instruction accounting is deliberately absent from the carried state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RspInterpreterStateEvidenceSnapshot {
    /// IPL/ROM reset has not yet entered the interpreter.
    Reset,
    /// Complete future-visible state after the last committed interpreter
    /// phase. `RspMachineState::from_architectural_state` restores it with a
    /// fresh diagnostic counter.
    Exact(fn64_audio::rsp::runtime::RspArchitecturalState),
    /// An optimized HLE backend completed successfully but did not expose
    /// the ucode's true terminal scalar/VU image. The carried value is the
    /// rspboot-entry image with its consumed overlay continuation cleared.
    HleCompatibility(fn64_audio::rsp::runtime::RspArchitecturalState),
    /// A direct-IMEM HLE task completed without entering rspboot and without
    /// exposing any terminal scalar/VU image. No later interpreter task may
    /// silently reuse the older exact snapshot.
    HleCompatibilityUnavailable { owner: RspInterpreterOwner },
    /// A synchronous interpreter phase has consumed the ready state. If that
    /// phase unwinds, another task traps instead of silently creating a fresh
    /// core and hiding the interrupted continuation.
    InFlight { owner: RspInterpreterOwner },
}

/// Who holds the RSP interpreter.
///
/// Ownership used to be a bare `(task_offset, admission_generation)` pair,
/// compared jointly at every guard. Folding the pair into one value makes that
/// a single `==` and removes the failure mode where a site checks the offset
/// and forgets the generation — the address-reuse aliasing the generation
/// exists to catch.
///
/// It also lets a **task-free** owner exist. A guest that kicks the RSP with a
/// raw `SP_STATUS` clear-halt has no `OSTask`, so no task offset describes it;
/// inventing one would fabricate admission evidence, and `0` is a legal offset
/// a real task can occupy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspInterpreterOwner {
    /// An admitted `OSTask`: its RDRAM offset plus the generation that admitted
    /// it. Both are load-bearing — the same address can be reused by a later
    /// task, and only the generation distinguishes them.
    Task {
        offset: u32,
        admission_generation: RspTaskAdmissionGeneration,
    },
    /// A raw `SP_STATUS` clear-halt started the RSP outside the task lane.
    /// Carries a generation so successive kicks stay distinguishable, but has
    /// no lineage and never enters `rsp_task_lineages`.
    RawKick {
        admission_generation: RspTaskAdmissionGeneration,
    },
}

/// The ABI-owned RSP path which committed one publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RspWriterCommitSourceV1 {
    Interpreter { owner: RspInterpreterOwner },
    TranslatedAudioHle { owner: RspInterpreterOwner },
}

/// One task-dispatch-owned RSP-to-RDRAM publication in commit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RspWriterCommitObservationV1 {
    pub source: RspWriterCommitSourceV1,
    pub physical_start: u32,
    pub physical_end: u32,
}

/// One successful translated-HLE callback, bound to the executable journal
/// sequences it committed. An empty sequence set is still a successful typed
/// publication boundary, but cannot by itself prove an executable write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RspWriterHlePublicationObservationV1 {
    pub source: RspWriterCommitSourceV1,
    pub journal_sequences: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RspWriterTraceSnapshotV1 {
    pub commits: Vec<RspWriterCommitObservationV1>,
    pub hle_publications: Vec<RspWriterHlePublicationObservationV1>,
    pub rejected_journal_sequences: Vec<u64>,
}

#[derive(Debug)]
pub(crate) struct RspWriterTraceV1 {
    pub(crate) epoch_id: u64,
    pub(crate) commits: Vec<RspWriterCommitObservationV1>,
    pub(crate) hle_publications: Vec<RspWriterHlePublicationObservationV1>,
    pub(crate) rejected_journal_sequences: Vec<u64>,
}

thread_local! {
    static RSP_WRITER_TRACE_V1: RefCell<Option<RspWriterTraceV1>> = const {
        RefCell::new(None)
    };
}

/// Arm the task-dispatch half of one fresh RSP writer audit epoch.
///
/// Canonical program/journal quiescence belongs to the recompiler owner. This
/// function owns only the non-forgeable task-dispatch observation window and
/// therefore remains crate-private.
pub(crate) fn begin_rsp_writer_trace_v1(epoch_id: u64) {
    assert_ne!(epoch_id, 0, "RSP writer trace epoch must be nonzero");
    RSP_WRITER_TRACE_V1.with(|trace| {
        *trace.borrow_mut() = Some(RspWriterTraceV1 {
            epoch_id,
            commits: Vec::new(),
            hle_publications: Vec::new(),
            rejected_journal_sequences: Vec::new(),
        });
    });
}

/// Copy observations only when `epoch_id` still names the live trace arm.
pub(crate) fn rsp_writer_trace_snapshot_v1(epoch_id: u64) -> Option<RspWriterTraceSnapshotV1> {
    RSP_WRITER_TRACE_V1.with(|trace| {
        let trace = trace.borrow();
        let trace = trace.as_ref()?;
        (trace.epoch_id == epoch_id).then(|| RspWriterTraceSnapshotV1 {
            commits: trace.commits.clone(),
            hle_publications: trace.hle_publications.clone(),
            rejected_journal_sequences: trace.rejected_journal_sequences.clone(),
        })
    })
}

/// Consume the exact task-dispatch observation window after validation.
pub(crate) fn finish_rsp_writer_trace_v1(epoch_id: u64) -> bool {
    RSP_WRITER_TRACE_V1.with(|trace| {
        let mut trace = trace.borrow_mut();
        if trace
            .as_ref()
            .is_none_or(|trace| trace.epoch_id != epoch_id)
        {
            return false;
        }
        *trace = None;
        true
    })
}

/// Whether an optimized HLE task still owns a resumable publication phase.
///
/// The canonical recompiler validator combines this task-local fact with the
/// `HostState` task/interpreter owners while it already holds that state; this
/// split avoids a nested `with_host` borrow at the audit boundary.
pub(crate) fn hle_rsp_writer_work_pending_v1() -> bool {
    HLE_RENDER_CONTINUATION.with(|continuation| continuation.borrow().is_some())
}

pub(crate) fn record_rsp_writer_commits_v1(source: RspWriterCommitSourceV1, written: &[(usize, usize)]) {
    RSP_WRITER_TRACE_V1.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        for &(start, end) in written {
            assert!(start < end, "RSP writer commit range must be nonempty");
            assert!(
                end <= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
                "RSP writer commit range [{start:#x}, {end:#x}) exceeds physical RDRAM"
            );
            trace.commits.push(RspWriterCommitObservationV1 {
                source,
                physical_start: u32::try_from(start).expect("RSP writer commit start exceeds u32"),
                physical_end: u32::try_from(end).expect("RSP writer commit end exceeds u32"),
            });
        }
    });
}

pub(crate) fn finish_translated_audio_hle_publication_v1(
    source: RspWriterCommitSourceV1,
    journal_sequences: Vec<u64>,
    committed: bool,
) {
    assert!(
        matches!(source, RspWriterCommitSourceV1::TranslatedAudioHle { .. }),
        "translated-HLE lifecycle requires a translated-HLE source"
    );
    RSP_WRITER_TRACE_V1.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        if committed {
            trace
                .hle_publications
                .push(RspWriterHlePublicationObservationV1 {
                    source,
                    journal_sequences,
                });
        } else {
            trace.rejected_journal_sequences.extend(journal_sequences);
        }
    });
}

#[cfg(test)]
pub(crate) fn record_test_rsp_writer_commits_v1(
    source: RspWriterCommitSourceV1,
    written: &[(usize, usize)],
) {
    record_rsp_writer_commits_v1(source, written);
}

impl RspInterpreterOwner {
    /// An owner for an admitted task at `offset`.
    pub const fn task(offset: u32, admission_generation: RspTaskAdmissionGeneration) -> Self {
        Self::Task {
            offset,
            admission_generation,
        }
    }

    /// The admitting generation, whichever owner kind this is.
    pub const fn admission_generation(self) -> RspTaskAdmissionGeneration {
        match self {
            Self::Task {
                admission_generation,
                ..
            }
            | Self::RawKick {
                admission_generation,
            } => admission_generation,
        }
    }

    /// The owning task's RDRAM offset, or `None` for a raw kick. Callers that
    /// need a task — lineage lookup, observation labelling — must handle the
    /// `None` rather than substitute a placeholder offset.
    pub const fn task_offset(self) -> Option<u32> {
        match self {
            Self::Task { offset, .. } => Some(offset),
            Self::RawKick { .. } => None,
        }
    }

    /// How to name this owner in a diagnostic. A raw kick has no task address,
    /// so messages say what it is rather than printing a placeholder offset.
    /// The generation is always included: it is the field that catches aliasing
    /// between two owners that share an address.
    pub fn describe(self) -> String {
        match self {
            Self::Task {
                offset,
                admission_generation,
            } => format!(
                "task {offset:#010x} generation {}",
                admission_generation.get()
            ),
            Self::RawKick {
                admission_generation,
            } => format!("raw SP kick generation {}", admission_generation.get()),
        }
    }
}

pub(crate) fn imem_sha256(imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE]) -> [u8; 32] {
    Sha256::digest(imem).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskMicrocodeDataIdentity {
    pub(crate) addr: RdramAddr,
    pub(crate) size: u32,
    pub(crate) sha256: [u8; 32],
}

impl TaskMicrocodeDataIdentity {
    pub(crate) fn evidence_snapshot(self) -> RspTaskDataIdentityEvidenceSnapshot {
        RspTaskDataIdentityEvidenceSnapshot {
            rdram_offset: self.addr.offset(),
            byte_len: self.size,
            sha256: self.sha256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RspTaskLineagePhase {
    Running,
    ResumeAuthorized,
    ResumeLoaded,
}

impl RspTaskLineagePhase {
    pub(crate) fn evidence_snapshot(self) -> RspTaskLineagePhaseEvidenceSnapshot {
        match self {
            Self::Running => RspTaskLineagePhaseEvidenceSnapshot::Running,
            Self::ResumeAuthorized => RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized,
            Self::ResumeLoaded => RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RspTaskLineage {
    pub(crate) admission_generation: RspTaskAdmissionGeneration,
    pub(crate) original_header: OsTaskHeader,
    pub(crate) data_identity: Option<TaskMicrocodeDataIdentity>,
    pub(crate) phase: RspTaskLineagePhase,
}

impl RspTaskLineage {
    pub(crate) fn evidence_snapshot(&self, task_offset: u32) -> RspTaskLineageEvidenceSnapshot {
        RspTaskLineageEvidenceSnapshot {
            task_offset,
            admission_generation: self.admission_generation.get(),
            original_header: self.original_header,
            data_identity: self
                .data_identity
                .map(TaskMicrocodeDataIdentity::evidence_snapshot),
            phase: self.phase.evidence_snapshot(),
        }
    }

    pub(crate) fn yielded_header(self) -> OsTaskHeader {
        OsTaskHeader {
            flags: self.original_header.flags | fn64_runtime::OS_TASK_YIELDED,
            ucode_data: self.original_header.yield_data_ptr,
            ucode_data_size: self.original_header.yield_data_size,
            ..self.original_header
        }
    }
}

/// Process-monotonic identity of one successfully admitted `osSpTaskLoad`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspTaskAdmissionGeneration(pub(crate) NonZeroU64);

impl RspTaskAdmissionGeneration {
    /// Constructs an evidence value from a nonzero admission generation.
    ///
    /// Runtime admission mints these monotonically; this constructor exists
    /// for evidence-schema consumers and fixtures that must reproduce an
    /// already-observed generation without making zero representable.
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub(crate) const fn first() -> Self {
        Self::new(NonZeroU64::MIN)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub(crate) fn advance(&mut self) -> Self {
        let current = *self;
        self.0 = NonZeroU64::new(
            self.0
                .get()
                .checked_add(1)
                .expect("RSP task admission generation overflow"),
        )
        .expect("incremented RSP task generation cannot be zero");
        current
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoadedRspTask {
    pub(crate) task_addr: RdramAddr,
    pub(crate) admission_generation: RspTaskAdmissionGeneration,
    pub(crate) header: OsTaskHeader,
    pub(crate) resumed_data_identity: Option<TaskMicrocodeDataIdentity>,
}

impl LoadedRspTask {
    pub(crate) fn evidence_snapshot(&self) -> LoadedRspTaskEvidenceSnapshot {
        LoadedRspTaskEvidenceSnapshot {
            task_offset: self.task_addr.offset(),
            admission_generation: self.admission_generation.get(),
            header: self.header,
            resumed_data_identity: self
                .resumed_data_identity
                .map(TaskMicrocodeDataIdentity::evidence_snapshot),
        }
    }
}

/// Capture the original task microcode-data image at the RSP kickoff boundary.
///
/// The source address and size come from the typed header retained by
/// `osSpTaskLoad`, never from the mutable CPU `OSTask` storage. SP_DRAM_ADDR
/// canonicalizes addresses to 24 bits; the result must remain inside physical
/// RDRAM even when the host allocation appends sparse MMIO backing.
///
/// # Safety
/// `rdram` must address the process allocation registered in `HostState`.
pub(crate) unsafe fn task_microcode_data_identity(
    rdram: *mut u8,
    task_addr: RdramAddr,
    source_addr: u32,
    size: u32,
) -> TaskMicrocodeDataIdentity {
    let (registered_rdram, allocation_len) =
        with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    assert!(
        !rdram.is_null() && allocation_len != 0,
        "RSP task {:#010x} microcode-data capture has no registered process RDRAM allocation",
        task_addr.offset()
    );
    assert_eq!(
        registered_rdram,
        rdram,
        "RSP task {:#010x} microcode-data capture does not use the registered process RDRAM allocation",
        task_addr.offset()
    );
    let addr = RdramAddr::from_offset(source_addr & 0x00ff_ffff);
    let start = addr.offset() as usize;
    let end = start.checked_add(size as usize).unwrap_or_else(|| {
        panic!(
            "RSP task {:#010x} microcode-data range overflows host usize: start={:#010x} size={size:#x}",
            task_addr.offset(),
            addr.offset()
        )
    });
    assert!(
        end <= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "RSP task {:#010x} microcode-data range [{:#010x}, {end:#010x}) exceeds physical RDRAM length {:#x}",
        task_addr.offset(),
        addr.offset(),
        fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
    );
    assert!(
        end <= allocation_len,
        "RSP task {:#010x} microcode-data range [{:#010x}, {end:#010x}) exceeds registered allocation length {allocation_len:#x}",
        task_addr.offset(),
        addr.offset(),
    );

    let memory = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let mut digest = Sha256::new();
    // Batches the SAME logical byte sequence the per-byte loop produced.
    //
    // The sequence is NOT a contiguous slice of host storage: `RdramPtr`
    // reads logical byte `a` from storage index `a ^ 3` (the native-word lane
    // mapping, `fn64_runtime::rdram`'s `range(.., lane_xor = 3)`). Chunking
    // over raw storage would digest the bytes in lane order and silently
    // change `TaskMicrocodeDataIdentity::sha256` -- which the release gate
    // reads as `rsp_rdp.ordered[].observation.data_sha256`. So the staging
    // buffer is filled through the same `read_u8` accessor, preserving the
    // logical order exactly, and only the `Digest::update` calls are batched.
    // `microcode_data_identity_batches_the_swizzled_logical_order` pins the
    // result against a literal per-byte digest.
    const CHUNK: usize = 4096;
    let mut staged = [0u8; CHUNK];
    let mut offset = 0u32;
    while offset < size {
        let span = CHUNK.min((size - offset) as usize);
        for slot in 0..span {
            let byte_addr = addr.checked_add(offset + slot as u32).unwrap_or_else(|| {
                panic!(
                    "RSP task {:#010x} microcode-data logical address overflow at byte {:#x}",
                    task_addr.offset(),
                    offset + slot as u32
                )
            });
            staged[slot] = unsafe { memory.read_u8(byte_addr) };
        }
        digest.update(&staged[..span]);
        offset += span as u32;
    }
    TaskMicrocodeDataIdentity {
        addr,
        size,
        sha256: digest.finalize().into(),
    }
}

pub(crate) fn identify_microcode_pair(
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    data: TaskMicrocodeDataIdentity,
    authoritative_family: Option<fn64_render::UcodeId>,
) -> Option<fn64_render::UcodeId> {
    let backend_family = with_render_backend("identify_microcode_pair", |backend| {
        Ok(backend.identify_microcode_pair(
            imem,
            fn64_render::MicrocodeDataImageIdentity {
                bytes: data.size,
                sha256: data.sha256,
            },
        ))
    });
    match (authoritative_family, backend_family) {
        (Some(authoritative), Some(backend)) if authoritative != backend => {
            panic!(
                "pinned microcode classifier identified {authoritative:?}, but the backend pair catalog claimed {backend:?}"
            )
        }
        (Some(authoritative), _) => Some(authoritative),
        (None, backend) => backend,
    }
}

/// Classify the immutable task-entry raw text/data storage through the pinned
/// MIT RT64 identity table. This does not admit HLE; it prevents a backend or
/// private pair declaration from choosing the family written to LLE evidence.
///
/// # Safety
/// `rdram` must be the registered process allocation.
pub(crate) unsafe fn classify_task_microcode_family(
    rdram: *mut u8,
    header: &OsTaskHeader,
) -> Option<fn64_render::UcodeId> {
    let storage = unsafe { renderer_rdram_slice(rdram) };
    let window = fn64_render::capture_task_admission_raw_window(
        storage,
        RdramAddr::from_offset(header.ucode & 0x00ff_ffff),
        RdramAddr::from_offset(header.ucode_data & 0x00ff_ffff),
        fn64_render::F3DZEX2_RAW_WINDOW_SIZE,
    )?;
    fn64_render::identify_f3dzex2(&window).map(fn64_render::F3dzex2Variant::family)
}

/// Number of command words staged per `Digest::update` call.
///
/// Pure batching of the SAME byte sequence: SHA-256 is defined over the
/// concatenation, so `update(a); update(b)` and `update(a ++ b)` produce an
/// identical digest. The value in the release-gate evidence
/// (`RspRdpObservationKind::{Dram,Xbus}DpcCommitted::command_sha256`,
/// validated by `release_gate/publication.rs`) is therefore unchanged by
/// construction, and `canonical_rdp_words_sha256_matches_per_word_updates`
/// pins that against the literal per-word loop this replaced.
///
/// 1024 words is 4 KiB per update -- comfortably past the point where the
/// per-call overhead of the digest's block buffer stops dominating, while
/// keeping the scratch buffer inside a stack-friendly, cache-resident size.
const RDP_WORD_DIGEST_CHUNK: usize = 1024;

/// Digest the canonical big-endian image of an RDP command stream.
///
/// The words arrive as host-order `u32`, so the big-endian conversion is
/// load-bearing: it, not the host's byte order, defines the digested
/// sequence. Batching converts a chunk at a time instead of calling
/// `Digest::update` once per 4-byte word.
pub(crate) fn canonical_rdp_words_sha256(words: &[u32]) -> [u8; 32] {
    let mut digest = Sha256::new();
    let mut staged = [0u8; RDP_WORD_DIGEST_CHUNK * 4];
    for chunk in words.chunks(RDP_WORD_DIGEST_CHUNK) {
        let staged = &mut staged[..chunk.len() * 4];
        for (slot, word) in chunk.iter().enumerate() {
            staged[slot * 4..slot * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest.update(&*staged);
    }
    digest.finalize().into()
}

pub(crate) fn dpc_observation(xbus: bool, start: u32, end: u32, words: &[u32]) -> RspRdpObservationKind {
    let command_sha256 = canonical_rdp_words_sha256(words);
    if xbus {
        RspRdpObservationKind::XbusDpcCommitted {
            start,
            end,
            command_sha256,
        }
    } else {
        RspRdpObservationKind::DramDpcCommitted {
            start,
            end,
            command_sha256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmittedTaskImageShape {
    BootOverlay,
    DirectImem,
}

#[derive(Clone, Debug)]
pub(crate) enum AdmittedHleEntry {
    BootOverlay(Box<HleBootResult>),
    DirectImem {
        task: OsTaskHeader,
        lle_machine_state: Option<Box<fn64_audio::rsp::runtime::RspMachineState>>,
    },
}

impl AdmittedHleEntry {
    pub(crate) fn task(&self) -> OsTaskHeader {
        match self {
            Self::BootOverlay(boot) => boot.task,
            Self::DirectImem { task, .. } => *task,
        }
    }

    pub(crate) fn pre_ucode_steps(&self) -> u64 {
        match self {
            Self::BootOverlay(boot) => boot.steps,
            Self::DirectImem { .. } => 0,
        }
    }

    pub(crate) fn into_lle_machine_state(self) -> Option<fn64_audio::rsp::runtime::RspMachineState> {
        match self {
            Self::BootOverlay(boot) => Some(boot.machine_state),
            Self::DirectImem {
                lle_machine_state, ..
            } => lle_machine_state.map(|state| *state),
        }
    }

    pub(crate) fn hle_compatibility_state(&self) -> Option<fn64_audio::rsp::runtime::RspMachineState> {
        match self {
            Self::BootOverlay(boot) => Some(boot.machine_state.clone()),
            Self::DirectImem { .. } => None,
        }
    }
}

/// Acquire the persistent interpreter owner for a direct-IMEM optimized phase
/// before any backend can mutate renderer state or schedule completion.
///
/// The returned snapshot is the untouched PC-zero continuation used if HLE
/// preflight requests LLE. A prior implementation waited until the final
/// compatibility commit: a different task could remain `InFlight` while this
/// task mutated the backend, then trap only after that mutation. Acquiring the
/// owner here closes that exact interleaving; a backend unwind deliberately
/// leaves this same-task owner `InFlight`.
pub(crate) unsafe fn begin_direct_hle_phase(
    rdram: *mut u8,
    task_addr: RdramAddr,
) -> fn64_audio::rsp::runtime::RspMachineState {
    let (dmem, rdram_len, static_aliases) = with_host(|host| {
        (
            *host
                .device_fabric
                .rsp_memory()
                .bank(fn64_runtime::RspMemoryBank::Dmem),
            host.runtime_rdram_len,
            host.sections.loaded_static_storage_ranges(),
        )
    });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "direct-IMEM HLE task has no registered process RDRAM allocation"
    );
    let (dma_ranges, _) = rsp_dma_storage_layout(rdram_len, static_aliases);
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.set_dma_rdram_ranges(dma_ranges);
    machine.load_dmem_logical(&dmem);
    begin_rsp_interpreter_phase(task_interpreter_owner(task_addr), &mut machine);
    machine.snapshot_state()
}

pub(crate) fn resume_direct_hle_phase(task_addr: RdramAddr) {
    let admission_generation = running_task_admission_generation(task_addr);
    with_host(|host| {
        match host.rsp_interpreter_state {
        // Same task address, strictly older generation: this is the suspended
        // owner being reclaimed by its own readmission.
        RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
            owner: RspInterpreterOwner::Task {
                offset,
                admission_generation: prior_generation,
            },
        } if offset == task_addr.offset()
            && prior_generation.get() < admission_generation.get() =>
        {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: RspInterpreterOwner::task(offset, admission_generation),
            };
        }
        RspInterpreterStateEvidenceSnapshot::InFlight { owner }
        | RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable { owner } => {
            match owner.task_offset() {
                Some(task_offset) => panic!(
                    "direct-IMEM HLE task {:#010x} cannot resume state owned by task {task_offset:#010x}",
                    task_addr.offset()
                ),
                None => panic!(
                    "direct-IMEM HLE task {:#010x} cannot resume state owned by a raw SP kick",
                    task_addr.offset()
                ),
            }
        }
        _ => panic!(
            "direct-IMEM HLE task {:#010x} cannot resume without its suspended compatibility owner",
            task_addr.offset()
        ),
    }
    });
}

/// Resolves the interpreter owner for a task that is already admitted and
/// Running. A raw SP kick has no lineage and must use
/// [`acquire_raw_kick_interpreter_owner`] instead.
pub(crate) fn task_interpreter_owner(task_addr: RdramAddr) -> RspInterpreterOwner {
    RspInterpreterOwner::task(
        task_addr.offset(),
        running_task_admission_generation(task_addr),
    )
}

/// Mints the owner for a raw `SP_STATUS` clear-halt kick, which arrives with no
/// `OSTask` and therefore no admitted lineage.
///
/// The generation comes from the same process-monotonic counter task admissions
/// use: a raw kick is a real RSP start and must be distinguishable from every
/// other one, including a later kick that would otherwise alias it.
///
/// Mutual exclusion with the task lane is asserted here rather than left to the
/// interpreter-state check alone: a Running lineage means a task owns the RSP
/// even at moments when the interpreter state is not yet `InFlight`.
pub(crate) fn acquire_raw_kick_interpreter_owner() -> RspInterpreterOwner {
    with_host(|host| {
        if let Some((offset, lineage)) = host
            .rsp_task_lineages
            .iter()
            .find(|(_, lineage)| lineage.phase == RspTaskLineagePhase::Running)
        {
            panic!(
                "raw SP kick cannot start while task {offset:#010x} generation {} owns the RSP",
                lineage.admission_generation.get()
            );
        }
        RspInterpreterOwner::RawKick {
            admission_generation: host.next_rsp_task_admission_generation.advance(),
        }
    })
}

pub(crate) fn running_task_admission_generation(task_addr: RdramAddr) -> RspTaskAdmissionGeneration {
    with_host(|host| {
        let lineage = host
            .rsp_task_lineages
            .get(&task_addr.offset())
            .unwrap_or_else(|| {
                panic!(
                    "RSP task {:#010x} has no admitted task lineage",
                    task_addr.offset()
                )
            });
        assert_eq!(
            lineage.phase,
            RspTaskLineagePhase::Running,
            "RSP task {:#010x} cannot acquire interpreter ownership from lineage phase {:?}",
            task_addr.offset(),
            lineage.phase
        );
        lineage.admission_generation
    })
}

pub(crate) fn aligned_sp_image_size(size: u32) -> Option<u32> {
    size.checked_add(7)
        .map(|size| size & !7)
        .filter(|size| *size != 0 && *size as usize <= fn64_runtime::RSP_MEMORY_BANK_SIZE)
}

pub(crate) fn admitted_task_image_shape(header: &OsTaskHeader) -> AdmittedTaskImageShape {
    let boot = header.ucode_boot & 0x1fff_ffff;
    let ucode = header.ucode & 0x1fff_ffff;
    let direct_image = boot == ucode
        && boot.is_multiple_of(8)
        && header.ucode_size != 0
        && header.ucode_size as usize <= fn64_runtime::RSP_MEMORY_BANK_SIZE
        && aligned_sp_image_size(header.ucode_boot_size)
            .is_some_and(|copy_size| copy_size >= header.ucode_size);
    if direct_image {
        AdmittedTaskImageShape::DirectImem
    } else {
        AdmittedTaskImageShape::BootOverlay
    }
}
