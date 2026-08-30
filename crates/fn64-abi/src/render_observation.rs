//! Bounded, opt-in host observations of guest-task and raw-DPC execution.
//!
//! These records explain where renderer work overlaps the guest thread. They
//! are not device events and never participate in emulated scheduling: every
//! timestamp is sampled only after the shell explicitly enables observation.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

const MAX_COMPLETED_BATCHES: usize = 4096;
const MAX_COMPLETED_GUEST_TASKS: usize = 16_384;
const MAX_COMPLETED_VI_SCANOUTS: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchExecutionMode {
    Local,
    Worker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestCpuDispatchLane {
    CanonicalBlockProgram,
    AbiFunctionUnattributed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestRspDispatchLane {
    Interpreted,
    Translated,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestTaskKind {
    Graphics,
    Audio,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestTaskOutcome {
    Completed,
    Yielded,
    AbandonedAtProcessExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestTaskDispatchThread {
    Executor(fn64_runtime::ThreadId),
    Unattributed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestTaskQueueIdentity {
    NotApplicable,
    RawDpcTaskBatch { batch_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuestTaskRdpExecution {
    Cpu {
        members: usize,
    },
    Compute {
        members: usize,
    },
    Mixed {
        cpu_members: usize,
        compute_members: usize,
    },
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestTaskObservationKey {
    pub task_offset: u32,
    pub admission_generation: u64,
}

#[derive(Clone, Debug)]
pub struct GuestTaskObservation {
    pub key: GuestTaskObservationKey,
    pub resumed_from_admission_generation: Option<u64>,
    pub kind: GuestTaskKind,
    pub outcome: GuestTaskOutcome,
    pub dispatch_cycle: fn64_runtime::EmulatedInstant,
    pub completion_cycle: fn64_runtime::EmulatedInstant,
    pub dispatch_host_at: Instant,
    pub completion_host_at: Instant,
    pub cpu_dispatch_lane: GuestCpuDispatchLane,
    pub dispatch_thread: GuestTaskDispatchThread,
    pub rsp_dispatch_lane: GuestRspDispatchLane,
    pub rdp_execution: GuestTaskRdpExecution,
    pub queue: GuestTaskQueueIdentity,
    pub host_thread: RenderBatchHostThread,
    pub coherence_reason: Option<RenderBatchJoinCause>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchRdpLane {
    Cpu,
    Compute,
    Mixed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchHostThread {
    Emulation,
    RdpWorker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchJoinCause {
    ViVisibility,
    LaterGraphics,
    DmemDependency,
    LaterGraphicsAndDmemDependency,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderWorkerSpan {
    pub started_at: Instant,
    pub finished_at: Instant,
    /// Scheduled CPU time consumed by this worker while the batch executed.
    /// `None` means the host does not expose a per-thread CPU clock.
    pub cpu_time: Option<Duration>,
}

/// One host-time span for the renderer's VI work at an exact retrace.
///
/// This is diagnostic host evidence only. Neither timestamp participates in
/// emulated scheduling. Source and post-VI availability share one operation
/// span because one backend `present` call computes both outcomes and may make
/// at most one stage ready for the window thread.
#[derive(Clone, Copy, Debug)]
pub struct ViScanoutObservation {
    pub retrace_at: fn64_runtime::EmulatedInstant,
    pub source_generation: u64,
    pub source_ready: bool,
    pub post_vi_generation: u64,
    pub post_vi_ready: bool,
    pub started_at: Instant,
    pub finished_at: Instant,
}

/// Read the calling host thread's scheduled CPU clock for diagnostic spans.
///
/// This is deliberately separate from emulated time and never participates in
/// scheduling. Wall time minus this clock measures non-CPU wall time, which
/// can include voluntary blocking as well as host descheduling; identifying
/// either cause requires a scheduler or sampling trace.
#[cfg(unix)]
pub(crate) fn thread_cpu_time() -> Option<Duration> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `value` is valid writable storage for one `timespec`; the call
    // reads only the current thread's clock and retains no pointer.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut value) };
    if status != 0 || value.tv_sec < 0 || !(0..1_000_000_000).contains(&value.tv_nsec) {
        return None;
    }
    Some(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
}

#[cfg(not(unix))]
pub(crate) fn thread_cpu_time() -> Option<Duration> {
    None
}

#[derive(Clone, Copy, Debug)]
pub struct RenderBatchJoinSpan {
    pub cause: RenderBatchJoinCause,
    pub requested_at: Instant,
    pub returned_at: Instant,
}

/// One task-relative `DP_END` boundary retained for host diagnostics.
///
/// The step is absent only when the source submission was constructed
/// synthetically instead of executing an RSP `DP_END`. Neither the byte
/// offset nor the diagnostic step participates in emulated scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderBatchDpEndBoundaryObservation {
    pub command_end_byte_offset: u32,
    pub dp_end_step: Option<fn64_audio::rsp::runtime::RspDpEndStep>,
}

/// Content-free structural timing seed for one exact raw-DPC batch member.
///
/// This is opt-in diagnostic evidence. The structural workload makes no
/// pixel, area, cycle, timing, admission, or execution claim, and no field in
/// this record is read by renderer or device scheduling policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderBatchMemberTimingObservation {
    pub member_ordinal: u32,
    pub transaction: fn64_runtime::DpcTransactionId,
    pub structural_workload: fn64_render::RawRdpStructuralWorkload,
    pub dp_end_boundaries: Vec<RenderBatchDpEndBoundaryObservation>,
}

#[derive(Clone, Debug)]
pub struct RenderBatchObservation {
    pub batch_id: u64,
    pub member_count: usize,
    pub members: Vec<RenderBatchMemberTimingObservation>,
    pub dispatch_cycle: fn64_runtime::EmulatedInstant,
    pub publication_cycle: fn64_runtime::EmulatedInstant,
    pub completion_cycle: fn64_runtime::EmulatedInstant,
    pub dispatch_host_at: Instant,
    pub completion_host_at: Instant,
    pub cpu_dispatch_lane: GuestCpuDispatchLane,
    pub rsp_dispatch_lane: GuestRspDispatchLane,
    pub rdp_lane: RenderBatchRdpLane,
    pub rdp_cpu_members: Option<usize>,
    pub rdp_compute_members: Option<usize>,
    pub host_thread: RenderBatchHostThread,
    pub execution_mode: RenderBatchExecutionMode,
    pub worker: Option<RenderWorkerSpan>,
    pub join: Option<RenderBatchJoinSpan>,
    pub staged_writes: Duration,
    pub commit: Duration,
    pub copyback: Duration,
    pub publication: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchIncompleteReason {
    ProcessExitBeforeCompletion,
}

#[derive(Clone, Debug)]
pub struct RenderBatchIncompleteObservation {
    pub batch_id: u64,
    pub member_count: usize,
    pub dispatch_cycle: fn64_runtime::EmulatedInstant,
    pub dispatch_host_at: Instant,
    pub reason: RenderBatchIncompleteReason,
}

#[derive(Debug)]
pub(crate) struct PendingRenderBatchObservation {
    batch_id: u64,
    member_count: usize,
    members: Vec<RenderBatchMemberTimingObservation>,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
    publication_cycle: Option<fn64_runtime::EmulatedInstant>,
    dispatch_host_at: Instant,
    cpu_dispatch_lane: GuestCpuDispatchLane,
    worker: Option<RenderWorkerSpan>,
    join: Option<(RenderBatchJoinCause, Instant)>,
    staged_writes: Duration,
    commit: Duration,
    copyback: Duration,
    publication: Duration,
    mechanism: Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
}

#[derive(Debug)]
pub(crate) struct PendingGuestTaskObservation {
    key: GuestTaskObservationKey,
    resumed_from_admission_generation: Option<u64>,
    kind: GuestTaskKind,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
    dispatch_host_at: Instant,
    cpu_dispatch_lane: GuestCpuDispatchLane,
    dispatch_thread: GuestTaskDispatchThread,
}

pub(crate) struct CompletedRenderBatchObservation {
    pending: PendingRenderBatchObservation,
    completion_cycle: fn64_runtime::EmulatedInstant,
    completion_host_at: Instant,
}

thread_local! {
    static CONFIGURED: Cell<bool> = const { Cell::new(false) };
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static NEXT_BATCH_ID: Cell<u64> = const { Cell::new(0) };
    static COMPLETED: RefCell<Vec<RenderBatchObservation>> = const { RefCell::new(Vec::new()) };
    static COMPLETED_GUEST_TASKS: RefCell<Vec<GuestTaskObservation>> = const { RefCell::new(Vec::new()) };
    static COMPLETED_VI_SCANOUTS: RefCell<Vec<ViScanoutObservation>> = const { RefCell::new(Vec::new()) };
}

/// Enable or disable host renderer observations before emulation begins.
///
/// The shell owns this diagnostic gate. It is deliberately an explicit call,
/// rather than another environment lookup inside the worker, so all records
/// share the shell trace's one host epoch and run identity.
pub fn set_render_batch_observation_enabled(enabled: bool) {
    CONFIGURED.with(|configured| {
        assert!(
            !configured.replace(true),
            "render batch observation may be configured only once per emulation thread"
        );
    });
    ENABLED.with(|cell| cell.set(enabled));
    NEXT_BATCH_ID.with(|cell| cell.set(0));
    COMPLETED.with(|cell| {
        assert!(
            cell.borrow().is_empty(),
            "render batch observation enabled with stale completed records"
        );
    });
    COMPLETED_GUEST_TASKS.with(|cell| {
        assert!(
            cell.borrow().is_empty(),
            "guest task observation enabled with stale completed records"
        );
    });
    COMPLETED_VI_SCANOUTS.with(|cell| {
        assert!(
            cell.borrow().is_empty(),
            "VI scanout observation enabled with stale completed records"
        );
    });
}

pub fn drain_render_batch_observations(destination: &mut Vec<RenderBatchObservation>) {
    COMPLETED.with(|cell| destination.extend(cell.borrow_mut().drain(..)));
}

pub fn drain_guest_task_observations(destination: &mut Vec<GuestTaskObservation>) {
    COMPLETED_GUEST_TASKS.with(|cell| destination.extend(cell.borrow_mut().drain(..)));
}

pub fn drain_vi_scanout_observations(destination: &mut Vec<ViScanoutObservation>) {
    COMPLETED_VI_SCANOUTS.with(|cell| destination.extend(cell.borrow_mut().drain(..)));
}

pub(crate) fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

pub(crate) fn vi_scanout_started() -> Option<Instant> {
    enabled().then(Instant::now)
}

pub(crate) fn record_vi_scanout(
    started_at: Instant,
    retrace_at: fn64_runtime::EmulatedInstant,
    source_generation: u64,
    source_ready: bool,
    post_vi_generation: u64,
    post_vi_ready: bool,
    finished_at: Instant,
) {
    assert!(
        finished_at >= started_at,
        "VI scanout observation finished before it started"
    );
    COMPLETED_VI_SCANOUTS.with(|cell| {
        let mut completed = cell.borrow_mut();
        assert!(
            completed.len() < MAX_COMPLETED_VI_SCANOUTS,
            "VI scanout observation exceeded its {MAX_COMPLETED_VI_SCANOUTS}-record bound"
        );
        completed.push(ViScanoutObservation {
            retrace_at,
            source_generation,
            source_ready,
            post_vi_generation,
            post_vi_ready,
            started_at,
            finished_at,
        });
    });
}

#[cfg(feature = "recomp-rs")]
fn current_cpu_dispatch_lane() -> GuestCpuDispatchLane {
    if crate::with_host(|host| host.canonical_recompiled_program.is_some()) {
        GuestCpuDispatchLane::CanonicalBlockProgram
    } else {
        GuestCpuDispatchLane::AbiFunctionUnattributed
    }
}

fn current_dispatch_thread() -> GuestTaskDispatchThread {
    crate::ACTIVE_THREAD_ID.with(|cell| {
        cell.get()
            .map(GuestTaskDispatchThread::Executor)
            .unwrap_or(GuestTaskDispatchThread::Unattributed)
    })
}

pub(crate) fn begin_guest_task(
    task_offset: u32,
    admission_generation: u64,
    resumed_from_admission_generation: Option<u64>,
    kind: GuestTaskKind,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
) -> Option<PendingGuestTaskObservation> {
    if !enabled() {
        return None;
    }
    assert_ne!(
        admission_generation, 0,
        "guest task observation admission generation must be nonzero"
    );
    if let Some(previous) = resumed_from_admission_generation {
        assert!(
            previous < admission_generation,
            "guest task observation resume generation must precede its admission"
        );
    }
    Some(PendingGuestTaskObservation {
        key: GuestTaskObservationKey {
            task_offset,
            admission_generation,
        },
        resumed_from_admission_generation,
        kind,
        dispatch_cycle,
        dispatch_host_at: Instant::now(),
        cpu_dispatch_lane: current_cpu_dispatch_lane(),
        dispatch_thread: current_dispatch_thread(),
    })
}

#[cfg(not(feature = "recomp-rs"))]
fn current_cpu_dispatch_lane() -> GuestCpuDispatchLane {
    GuestCpuDispatchLane::AbiFunctionUnattributed
}

pub(crate) fn begin(
    member_count: usize,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
    members: Option<Vec<RenderBatchMemberTimingObservation>>,
) -> Option<PendingRenderBatchObservation> {
    if !enabled() {
        assert!(
            members.is_none(),
            "disabled render observation must not allocate member timing seeds"
        );
        return None;
    }
    assert!(
        member_count > 0,
        "render observation batch must have a member"
    );
    let members = members.expect("enabled render observation requires member timing seeds");
    assert_eq!(
        members.len(),
        member_count,
        "render observation member timing count diverged from its task batch"
    );
    for (expected, member) in members.iter().enumerate() {
        assert_eq!(
            usize::try_from(member.member_ordinal).expect("member ordinal fits usize"),
            expected,
            "render observation member timing ordinals must be exact and ordered"
        );
    }
    let batch_id = NEXT_BATCH_ID.with(|cell| {
        let id = cell.get();
        cell.set(
            id.checked_add(1)
                .expect("render observation batch ID overflow"),
        );
        id
    });
    Some(PendingRenderBatchObservation {
        batch_id,
        member_count,
        members,
        dispatch_cycle,
        publication_cycle: None,
        dispatch_host_at: Instant::now(),
        cpu_dispatch_lane: current_cpu_dispatch_lane(),
        worker: None,
        join: None,
        staged_writes: Duration::ZERO,
        commit: Duration::ZERO,
        copyback: Duration::ZERO,
        publication: Duration::ZERO,
        mechanism: None,
    })
}

impl PendingRenderBatchObservation {
    pub(crate) const fn batch_id(&self) -> u64 {
        self.batch_id
    }

    pub(crate) fn set_execution_mechanism(
        &mut self,
        mechanism: Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
    ) {
        assert!(
            self.mechanism.is_none(),
            "render execution mechanism recorded twice"
        );
        if let Some(mechanism) = mechanism {
            assert_eq!(
                mechanism.member_count(),
                self.member_count,
                "render execution mechanism member count diverged from its task batch"
            );
            self.mechanism = Some(mechanism);
        }
    }

    pub(crate) fn set_worker_span(&mut self, span: Option<RenderWorkerSpan>) {
        assert!(self.worker.is_none(), "render worker span recorded twice");
        self.worker = span;
    }

    pub(crate) fn note_join(&mut self, cause: RenderBatchJoinCause) {
        assert!(self.join.is_none(), "render batch joined twice");
        self.join = Some((cause, Instant::now()));
    }

    pub(crate) fn phase_started(&self) -> Instant {
        Instant::now()
    }

    pub(crate) fn finish_staged_writes(&mut self, started: Instant) {
        add_elapsed(&mut self.staged_writes, started);
    }

    pub(crate) fn finish_commit(&mut self, started: Instant) {
        add_elapsed(&mut self.commit, started);
    }

    pub(crate) fn finish_copyback(&mut self, started: Instant) {
        add_elapsed(&mut self.copyback, started);
    }

    pub(crate) fn finish_publication(&mut self, started: Instant) {
        add_elapsed(&mut self.publication, started);
    }

    pub(crate) fn note_publication_cycle(&mut self, cycle: fn64_runtime::EmulatedInstant) {
        if let Some(first) = self.publication_cycle {
            assert_eq!(
                cycle, first,
                "one raw-DPC task batch published members at different emulated cycles"
            );
        } else {
            self.publication_cycle = Some(cycle);
        }
    }

    pub(crate) fn complete(
        self,
        completion_cycle: fn64_runtime::EmulatedInstant,
    ) -> CompletedRenderBatchObservation {
        let publication_cycle = self
            .publication_cycle
            .expect("completed render observation has no publication cycle");
        assert!(
            completion_cycle >= publication_cycle,
            "render observation completed before publication"
        );
        CompletedRenderBatchObservation {
            pending: self,
            completion_cycle,
            completion_host_at: Instant::now(),
        }
    }

    pub(crate) fn into_incomplete(
        self,
        reason: RenderBatchIncompleteReason,
    ) -> RenderBatchIncompleteObservation {
        RenderBatchIncompleteObservation {
            batch_id: self.batch_id,
            member_count: self.member_count,
            dispatch_cycle: self.dispatch_cycle,
            dispatch_host_at: self.dispatch_host_at,
            reason,
        }
    }
}

impl PendingGuestTaskObservation {
    pub(crate) const fn kind(&self) -> GuestTaskKind {
        self.kind
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete(
        self,
        outcome: GuestTaskOutcome,
        completion_cycle: fn64_runtime::EmulatedInstant,
        rsp_dispatch_lane: GuestRspDispatchLane,
        rdp_execution: GuestTaskRdpExecution,
        queue: GuestTaskQueueIdentity,
        host_thread: RenderBatchHostThread,
        coherence_reason: Option<RenderBatchJoinCause>,
    ) -> GuestTaskObservation {
        assert!(
            completion_cycle >= self.dispatch_cycle,
            "guest task observation completed before dispatch"
        );
        match (self.kind, rdp_execution) {
            (GuestTaskKind::Audio, GuestTaskRdpExecution::NotApplicable) => {}
            (GuestTaskKind::Audio, _) => {
                panic!("audio guest task observation must report RDP as not applicable")
            }
            (_, GuestTaskRdpExecution::NotApplicable) => {
                panic!("non-audio guest task observation cannot report RDP as not applicable")
            }
            _ => {}
        }
        match (queue, host_thread, coherence_reason) {
            (
                GuestTaskQueueIdentity::RawDpcTaskBatch { .. },
                RenderBatchHostThread::RdpWorker,
                None,
            ) if outcome == GuestTaskOutcome::AbandonedAtProcessExit => {}
            (
                GuestTaskQueueIdentity::RawDpcTaskBatch { .. },
                RenderBatchHostThread::RdpWorker,
                Some(_),
            )
            | (
                GuestTaskQueueIdentity::RawDpcTaskBatch { .. },
                RenderBatchHostThread::Emulation,
                None,
            )
            | (GuestTaskQueueIdentity::NotApplicable, RenderBatchHostThread::Emulation, None) => {}
            (GuestTaskQueueIdentity::RawDpcTaskBatch { .. }, _, _) => {
                panic!("raw-DPC guest task queue/thread/coherence identity is inconsistent")
            }
            (GuestTaskQueueIdentity::NotApplicable, _, _) => {
                panic!("nonqueued guest task cannot claim worker or coherence identity")
            }
        }
        GuestTaskObservation {
            key: self.key,
            resumed_from_admission_generation: self.resumed_from_admission_generation,
            kind: self.kind,
            outcome,
            dispatch_cycle: self.dispatch_cycle,
            completion_cycle,
            dispatch_host_at: self.dispatch_host_at,
            completion_host_at: Instant::now(),
            cpu_dispatch_lane: self.cpu_dispatch_lane,
            dispatch_thread: self.dispatch_thread,
            rsp_dispatch_lane,
            rdp_execution,
            queue,
            host_thread,
            coherence_reason,
        }
    }
}

pub(crate) fn rdp_execution_from_mechanism(
    mechanism: Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
) -> GuestTaskRdpExecution {
    match mechanism {
        Some(mechanism) if mechanism.cpu_members == 0 => GuestTaskRdpExecution::Compute {
            members: mechanism.compute_members,
        },
        Some(mechanism) if mechanism.compute_members == 0 => GuestTaskRdpExecution::Cpu {
            members: mechanism.cpu_members,
        },
        Some(mechanism) => GuestTaskRdpExecution::Mixed {
            cpu_members: mechanism.cpu_members,
            compute_members: mechanism.compute_members,
        },
        None => GuestTaskRdpExecution::Unavailable,
    }
}

impl CompletedRenderBatchObservation {
    pub(crate) fn seal(self, returned_at: Option<Instant>) -> RenderBatchObservation {
        let join = match (self.pending.join, returned_at) {
            (Some((cause, requested_at)), Some(returned_at)) => {
                assert!(
                    returned_at >= requested_at,
                    "render join returned before request"
                );
                Some(RenderBatchJoinSpan {
                    cause,
                    requested_at,
                    returned_at,
                })
            }
            (None, None) => None,
            _ => panic!("render observation join request and return must be complete together"),
        };
        RenderBatchObservation {
            batch_id: self.pending.batch_id,
            member_count: self.pending.member_count,
            members: self.pending.members,
            dispatch_cycle: self.pending.dispatch_cycle,
            publication_cycle: self
                .pending
                .publication_cycle
                .expect("completed render observation lost its publication cycle"),
            completion_cycle: self.completion_cycle,
            dispatch_host_at: self.pending.dispatch_host_at,
            completion_host_at: self.completion_host_at,
            cpu_dispatch_lane: self.pending.cpu_dispatch_lane,
            rsp_dispatch_lane: GuestRspDispatchLane::Interpreted,
            rdp_lane: match self.pending.mechanism {
                Some(mechanism) if mechanism.cpu_members == 0 => RenderBatchRdpLane::Compute,
                Some(mechanism) if mechanism.compute_members == 0 => RenderBatchRdpLane::Cpu,
                Some(_) => RenderBatchRdpLane::Mixed,
                None => RenderBatchRdpLane::Unavailable,
            },
            rdp_cpu_members: self.pending.mechanism.map(|value| value.cpu_members),
            rdp_compute_members: self.pending.mechanism.map(|value| value.compute_members),
            host_thread: if self.pending.worker.is_some() {
                RenderBatchHostThread::RdpWorker
            } else {
                RenderBatchHostThread::Emulation
            },
            execution_mode: if self.pending.worker.is_some() {
                RenderBatchExecutionMode::Worker
            } else {
                RenderBatchExecutionMode::Local
            },
            worker: self.pending.worker,
            join,
            staged_writes: self.pending.staged_writes,
            commit: self.pending.commit,
            copyback: self.pending.copyback,
            publication: self.pending.publication,
        }
    }
}

pub(crate) fn record_completed(observation: RenderBatchObservation) {
    COMPLETED.with(|cell| {
        let mut completed = cell.borrow_mut();
        assert!(
            completed.len() < MAX_COMPLETED_BATCHES,
            "render observation exceeded its {MAX_COMPLETED_BATCHES}-batch bound"
        );
        completed.push(observation);
    });
}

pub(crate) fn record_completed_guest_task(observation: GuestTaskObservation) {
    COMPLETED_GUEST_TASKS.with(|cell| {
        let mut completed = cell.borrow_mut();
        assert!(
            completed.len() < MAX_COMPLETED_GUEST_TASKS,
            "guest task observation exceeded its {MAX_COMPLETED_GUEST_TASKS}-task bound"
        );
        assert!(
            completed.iter().all(|prior| prior.key != observation.key),
            "guest task observation key ({:#010x}, {}) completed twice",
            observation.key.task_offset,
            observation.key.admission_generation,
        );
        completed.push(observation);
    });
}

fn add_elapsed(total: &mut Duration, started: Instant) {
    *total = total
        .checked_add(started.elapsed())
        .expect("render observation phase duration overflow");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_timing(
        member_ordinal: u32,
        token: u64,
        dp_end_step: Option<u64>,
    ) -> RenderBatchMemberTimingObservation {
        let submission = fn64_runtime::DpcSubmission {
            token,
            source: fn64_runtime::DpcSubmissionSource::Rdram,
            start: 0x100,
            end: 0x108,
        };
        RenderBatchMemberTimingObservation {
            member_ordinal,
            transaction: fn64_runtime::DpcTransactionId::from_submission(submission),
            structural_workload: fn64_render::RawRdpStructuralWorkload::default(),
            dp_end_boundaries: vec![RenderBatchDpEndBoundaryObservation {
                command_end_byte_offset: 8,
                dp_end_step: dp_end_step.map(fn64_audio::rsp::runtime::RspDpEndStep::new),
            }],
        }
    }

    fn member_timings(count: usize) -> Vec<RenderBatchMemberTimingObservation> {
        (0..count)
            .map(|index| {
                member_timing(
                    u32::try_from(index).unwrap(),
                    u64::try_from(index + 1).unwrap(),
                    Some(u64::try_from(index + 5).unwrap()),
                )
            })
            .collect()
    }

    #[test]
    fn disabled_observation_takes_no_record_and_enabled_record_is_drained() {
        ENABLED.with(|cell| cell.set(false));
        assert!(begin(2, fn64_runtime::EmulatedInstant::new(10), None).is_none());
        let mut records = Vec::new();
        drain_render_batch_observations(&mut records);
        assert!(records.is_empty());

        ENABLED.with(|cell| cell.set(true));
        let mut pending = begin(
            3,
            fn64_runtime::EmulatedInstant::new(20),
            Some(member_timings(3)),
        )
        .unwrap();
        pending.note_join(RenderBatchJoinCause::ViVisibility);
        pending.note_publication_cycle(fn64_runtime::EmulatedInstant::new(25));
        let completed = pending.complete(fn64_runtime::EmulatedInstant::new(30));
        record_completed(completed.seal(Some(Instant::now())));
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].batch_id, 0);
        assert_eq!(records[0].member_count, 3);
        assert_eq!(records[0].members, member_timings(3));
        assert_eq!(records[0].dispatch_cycle.get(), 20);
        assert_eq!(records[0].publication_cycle.get(), 25);
        assert_eq!(records[0].completion_cycle.get(), 30);
        assert!(records[0].completion_host_at >= records[0].dispatch_host_at);
        assert_eq!(
            records[0].rsp_dispatch_lane,
            GuestRspDispatchLane::Interpreted
        );
        assert_eq!(records[0].rdp_lane, RenderBatchRdpLane::Unavailable);
        assert_eq!(
            records[0].join.as_ref().map(|join| join.cause),
            Some(RenderBatchJoinCause::ViVisibility)
        );
        records.clear();
        drain_render_batch_observations(&mut records);
        assert!(records.is_empty());
        ENABLED.with(|cell| cell.set(false));
    }

    #[test]
    fn vi_scanout_observation_is_gated_bounded_and_identity_preserving() {
        ENABLED.with(|cell| cell.set(false));
        let edge = fn64_runtime::EmulatedInstant::new(77);
        assert!(vi_scanout_started().is_none());
        let mut records = Vec::new();
        drain_vi_scanout_observations(&mut records);
        assert!(records.is_empty());

        ENABLED.with(|cell| cell.set(true));
        let started = vi_scanout_started().expect("enabled observation owns a start");
        let finished = Instant::now();
        record_vi_scanout(started, edge, 9, true, 10, false, finished);
        drain_vi_scanout_observations(&mut records);
        assert_eq!(records.len(), 1);
        let record = records[0];
        assert_eq!(record.retrace_at, edge);
        assert_eq!(record.source_generation, 9);
        assert!(record.source_ready);
        assert_eq!(record.post_vi_generation, 10);
        assert!(!record.post_vi_ready);
        assert_eq!(record.started_at, started);
        assert_eq!(record.finished_at, finished);
        records.clear();
        drain_vi_scanout_observations(&mut records);
        assert!(records.is_empty());
        ENABLED.with(|cell| cell.set(false));
    }

    fn completed_fixture(batch_id: u64) -> RenderBatchObservation {
        RenderBatchObservation {
            batch_id,
            member_count: 1,
            members: vec![member_timing(0, batch_id + 1, None)],
            dispatch_cycle: fn64_runtime::EmulatedInstant::new(batch_id),
            publication_cycle: fn64_runtime::EmulatedInstant::new(batch_id + 1),
            completion_cycle: fn64_runtime::EmulatedInstant::new(batch_id + 1),
            dispatch_host_at: Instant::now(),
            completion_host_at: Instant::now(),
            cpu_dispatch_lane: GuestCpuDispatchLane::AbiFunctionUnattributed,
            rsp_dispatch_lane: GuestRspDispatchLane::Interpreted,
            rdp_lane: RenderBatchRdpLane::Unavailable,
            rdp_cpu_members: None,
            rdp_compute_members: None,
            host_thread: RenderBatchHostThread::Emulation,
            execution_mode: RenderBatchExecutionMode::Local,
            worker: None,
            join: None,
            staged_writes: Duration::ZERO,
            commit: Duration::ZERO,
            copyback: Duration::ZERO,
            publication: Duration::ZERO,
        }
    }

    #[test]
    fn execution_mechanism_is_exact_and_member_mismatch_traps() {
        ENABLED.with(|cell| cell.set(true));
        let mut pending = begin(
            3,
            fn64_runtime::EmulatedInstant::new(20),
            Some(member_timings(3)),
        )
        .unwrap();
        pending.set_execution_mechanism(fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(
            1, 2,
        ));
        pending.note_publication_cycle(fn64_runtime::EmulatedInstant::new(21));
        let record = pending
            .complete(fn64_runtime::EmulatedInstant::new(21))
            .seal(None);
        assert_eq!(record.rdp_lane, RenderBatchRdpLane::Mixed);
        assert_eq!(record.rdp_cpu_members, Some(1));
        assert_eq!(record.rdp_compute_members, Some(2));

        let mut mismatched = begin(
            2,
            fn64_runtime::EmulatedInstant::new(30),
            Some(member_timings(2)),
        )
        .unwrap();
        let trap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mismatched.set_execution_mechanism(
                fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(1, 2),
            );
        }));
        assert!(trap.is_err());
        ENABLED.with(|cell| cell.set(false));
    }

    #[test]
    fn member_timing_identity_order_none_steps_and_publication_cycle_are_exact() {
        ENABLED.with(|cell| cell.set(true));
        let members = vec![member_timing(0, 41, Some(5)), member_timing(1, 42, None)];
        let expected = members.clone();
        let mut pending = begin(2, fn64_runtime::EmulatedInstant::new(100), Some(members)).unwrap();
        pending.note_publication_cycle(fn64_runtime::EmulatedInstant::new(105));
        pending.note_publication_cycle(fn64_runtime::EmulatedInstant::new(105));
        let record = pending
            .complete(fn64_runtime::EmulatedInstant::new(110))
            .seal(None);
        assert_eq!(record.members, expected);
        assert_eq!(record.members[0].member_ordinal, 0);
        assert_eq!(record.members[0].transaction.get(), 41);
        assert_eq!(
            record.members[0].dp_end_boundaries[0]
                .dp_end_step
                .map(fn64_audio::rsp::runtime::RspDpEndStep::get),
            Some(5)
        );
        assert_eq!(record.members[1].member_ordinal, 1);
        assert_eq!(record.members[1].transaction.get(), 42);
        assert_eq!(record.members[1].dp_end_boundaries[0].dp_end_step, None);
        assert_eq!(record.publication_cycle.get(), 105);

        let wrong_order = vec![member_timing(1, 51, None), member_timing(0, 52, None)];
        let order_trap = std::panic::catch_unwind(|| {
            begin(
                2,
                fn64_runtime::EmulatedInstant::new(120),
                Some(wrong_order),
            )
        });
        assert!(order_trap.is_err());

        let mut split_publication = begin(
            2,
            fn64_runtime::EmulatedInstant::new(130),
            Some(member_timings(2)),
        )
        .unwrap();
        split_publication.note_publication_cycle(fn64_runtime::EmulatedInstant::new(135));
        let cycle_trap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            split_publication.note_publication_cycle(fn64_runtime::EmulatedInstant::new(136));
        }));
        assert!(cycle_trap.is_err());
        ENABLED.with(|cell| cell.set(false));
    }

    #[test]
    fn completed_bound_traps_before_growth_and_drain_restores_capacity() {
        COMPLETED.with(|cell| cell.borrow_mut().clear());
        for batch_id in 0..MAX_COMPLETED_BATCHES as u64 {
            record_completed(completed_fixture(batch_id));
        }
        let overflow = std::panic::catch_unwind(|| {
            record_completed(completed_fixture(MAX_COMPLETED_BATCHES as u64));
        });
        assert!(overflow.is_err());

        let mut records = Vec::new();
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), MAX_COMPLETED_BATCHES);
        record_completed(completed_fixture(MAX_COMPLETED_BATCHES as u64));
        records.clear();
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn incomplete_observation_retains_dispatch_identity_without_completing_work() {
        ENABLED.with(|cell| cell.set(true));
        NEXT_BATCH_ID.with(|cell| cell.set(7));
        let pending = begin(
            3,
            fn64_runtime::EmulatedInstant::new(20),
            Some(member_timings(3)),
        )
        .unwrap();
        let incomplete =
            pending.into_incomplete(RenderBatchIncompleteReason::ProcessExitBeforeCompletion);
        assert_eq!(incomplete.batch_id, 7);
        assert_eq!(incomplete.member_count, 3);
        assert_eq!(incomplete.dispatch_cycle.get(), 20);
        assert_eq!(
            incomplete.reason,
            RenderBatchIncompleteReason::ProcessExitBeforeCompletion
        );
        ENABLED.with(|cell| cell.set(false));
        NEXT_BATCH_ID.with(|cell| cell.set(0));
    }

    #[test]
    fn guest_task_key_resume_and_typed_audio_terminal_are_retained_once() {
        ENABLED.with(|cell| cell.set(true));
        COMPLETED_GUEST_TASKS.with(|cell| cell.borrow_mut().clear());
        let pending = begin_guest_task(
            0x140,
            8,
            Some(7),
            GuestTaskKind::Audio,
            fn64_runtime::EmulatedInstant::new(20),
        )
        .unwrap();
        record_completed_guest_task(pending.complete(
            GuestTaskOutcome::Completed,
            fn64_runtime::EmulatedInstant::new(30),
            GuestRspDispatchLane::Translated,
            GuestTaskRdpExecution::NotApplicable,
            GuestTaskQueueIdentity::NotApplicable,
            RenderBatchHostThread::Emulation,
            None,
        ));
        let mut records = Vec::new();
        drain_guest_task_observations(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key.task_offset, 0x140);
        assert_eq!(records[0].key.admission_generation, 8);
        assert_eq!(records[0].resumed_from_admission_generation, Some(7));
        assert_eq!(records[0].outcome, GuestTaskOutcome::Completed);
        assert_eq!(
            records[0].rsp_dispatch_lane,
            GuestRspDispatchLane::Translated
        );
        assert_eq!(
            records[0].rdp_execution,
            GuestTaskRdpExecution::NotApplicable
        );
        ENABLED.with(|cell| cell.set(false));
    }

    #[test]
    fn guest_task_rdp_mechanism_and_invalid_applicability_fail_closed() {
        assert_eq!(
            rdp_execution_from_mechanism(fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(
                1, 2
            )),
            GuestTaskRdpExecution::Mixed {
                cpu_members: 1,
                compute_members: 2,
            }
        );
        ENABLED.with(|cell| cell.set(true));
        let pending = begin_guest_task(
            0x180,
            9,
            None,
            GuestTaskKind::Graphics,
            fn64_runtime::EmulatedInstant::new(40),
        )
        .unwrap();
        let trap = std::panic::catch_unwind(|| {
            pending.complete(
                GuestTaskOutcome::Completed,
                fn64_runtime::EmulatedInstant::new(41),
                GuestRspDispatchLane::Translated,
                GuestTaskRdpExecution::NotApplicable,
                GuestTaskQueueIdentity::NotApplicable,
                RenderBatchHostThread::Emulation,
                None,
            )
        });
        assert!(trap.is_err());
        ENABLED.with(|cell| cell.set(false));
    }
}
