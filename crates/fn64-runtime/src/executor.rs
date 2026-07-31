//! The single executor: one host thread, priority-ordered run queue,
//! stackful-coroutine `OSThread`s, libultra-faithful blocking
//! `osSendMesg`/`osRecvMesg`, and the ONE host-side event injection point
//! for external (SI/PI/VI-style) completions.
//!
//! See `docs/DESIGN.md` section 2 for the full design rationale (option
//! (b): "single executor + stackful coroutines") -- this module is that
//! recommendation's load-bearing implementation. Every design choice below
//! traces back to a specific rung's evidence, cited inline.

use std::collections::{HashMap, HashSet};

use corosensei::CoroutineResult;

use crate::device::Cycles;
use crate::mesgqueue::{
    Mesg, MesgQueue, MesgQueueActivity, MesgQueueEvidenceSnapshot, RecvResult, SendPlacement,
    SendResult,
};
use crate::peripherals::Peripherals;
use crate::rdram::RdramAddr;
use crate::rsp::{OsTaskHeader, TaskLog};
use crate::si::PifModel;
use crate::thread::{GameThread, Priority, Resume, RunToken, ThreadState, Yield};
use crate::timer::{TimerWheel, TimerWheelEvidenceSnapshot};
use crate::trace::{QueueOpKind, SwitchReason, TaskKind, ThreadId, TraceKind, TraceLog};
use crate::vi::ViState;

/// `OS_EVENT_VI`, per the public libultra manual (`ultra64.h`'s documented
/// event-code table) -- verified against real NWXE call-site evidence too
/// (`aki-recomp/games/NWXE/profile.toml`'s rung-11 `osCreateViManager`
/// writeup: `osSetEventMesg(7, mq, &retraceMsg)`).
pub const OS_EVENT_VI: u32 = 7;

/// The executor: the ONE place in this crate that (a) issues `RunToken`s,
/// (b) owns every `GameThread`'s state transitions, (c) owns every
/// `MesgQueue`'s registration and is the only mutator of blocked lists via
/// `MesgQueue`'s already-narrow API, and (d) is the sole entry point
/// external host events go through (`post_external_message`, below).
///
/// Per `docs/DESIGN.md` section 2's option (b) rationale: because this
/// struct is only ever driven from one host thread (nothing in this crate
/// spawns a `std::thread`), "resume coroutine B" and "coroutine A's last
/// rdram write" have a trivial sequential happens-before relationship. The
/// rung-18 failure mode -- a second host thread's recompiled code touching
/// shared rdram with no lock the scheduler can see -- has no
/// precondition here: there is no second host thread, full stop.
/// OSMesgQueue struct field offsets (libultra `include/ultra64/message.h`,
/// `OSMesgQueue`: `validCount` @ 0x08, `first` @ 0x0C, `msgCount` @ 0x10).
/// Guest code reads these DIRECTLY out of the struct via the `MQ_GET_COUNT`/
/// `MQ_IS_FULL`/`MQ_IS_EMPTY` macros (e.g. `IrqMgr_SendMesgToClients`'s
/// per-client `MQ_IS_FULL(client->queue)` gate) -- fn64 keeps the
/// authoritative queue state in its own `MesgQueue`, so those struct fields
/// MUST be mirrored back into rdram after every mutation or guest reads see
/// stale zeros (a never-initialized queue reads `validCount=0, msgCount=0`,
/// making `MQ_IS_FULL` = `0 >= 0` = ALWAYS TRUE -- the bug that silently
/// dropped every retrace forward to the scheduler and froze OoT after 1 swap).
const MQ_VALIDCOUNT_OFF: u32 = 0x08;
const MQ_FIRST_OFF: u32 = 0x0C;
const MQ_MSGCOUNT_OFF: u32 = 0x10;

/// Pointer-free registration state for the guest RDRAM buffer.
///
/// The legacy length-less registration API cannot truthfully supply a bound,
/// so it remains distinguishable from both absence and a measured length.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RdramRegistrationEvidenceSnapshot {
    Absent,
    LegacyUnbounded,
    Present { len: u64 },
}

/// Scheduler-visible metadata for one guest thread. `started` is retained
/// because stopping and restarting a never-resumed thread must deliver
/// `Resume::Start`, while restarting an established coroutine must not.
/// Native coroutine stack/continuation bytes are intentionally absent.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ThreadEvidenceSnapshot {
    pub id: ThreadId,
    pub priority: Priority,
    pub state: ThreadState,
    pub started: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PendingResumeEvidenceSnapshot {
    pub thread: ThreadId,
    pub resume: Resume,
}

/// Receipt for the executor's terminal host process-exit transition.
///
/// `detached_coroutines` counts started guest stacks that were still
/// suspended and therefore could not be force-unwound across their generated
/// C / `extern "C"` frames. Their allocations are intentionally left for the
/// operating system to reclaim at process exit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ProcessExitSummary {
    pub threads: u32,
    pub detached_coroutines: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorQueueEvidenceSnapshot {
    pub address: RdramAddr,
    pub queue: MesgQueueEvidenceSnapshot,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct EventRegistrationEvidenceSnapshot {
    pub event: u32,
    pub queue_addr: RdramAddr,
    pub msg: Mesg,
}

/// Whether a snapshot was observed between scheduling steps or while a
/// coroutine held the sole run token. `Active` identifies the owner honestly;
/// it does not pretend to encode that coroutine's native continuation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExecutorRunningEvidenceSnapshot {
    Quiescent,
    Active(ThreadId),
}

/// Complete owner-local, pointer-free projection of executor control state.
///
/// This is evidence for deterministic fixed-cycle comparison, not a savestate:
/// native coroutine continuations and diagnostic traces are deliberately not
/// representable. [`Executor::control_evidence_snapshot`] validates all
/// cross-owner scheduler invariants before producing this value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutorControlEvidenceSnapshot {
    pub rdram: RdramRegistrationEvidenceSnapshot,
    /// Canonical ascending `ThreadId` order.
    pub threads: Vec<ThreadEvidenceSnapshot>,
    /// Exact scheduling order; unlike map-owned families, this is not sorted.
    pub run_queue: Vec<ThreadId>,
    /// Canonical ascending `ThreadId` order.
    pub pending_resumes: Vec<PendingResumeEvidenceSnapshot>,
    /// Canonical ascending guest-address order.
    pub queues: Vec<ExecutorQueueEvidenceSnapshot>,
    pub timers: TimerWheelEvidenceSnapshot,
    /// Canonical ascending event-code order.
    pub event_table: Vec<EventRegistrationEvidenceSnapshot>,
    pub running: ExecutorRunningEvidenceSnapshot,
    pub sim_time: u64,
    pub cp0_count: u32,
    pub cp0_count_phase: u8,
    pub cp0_compare: u32,
    pub cp0_timer_pending: bool,
}

/// A corrupt executor relationship that would make control-state evidence
/// ambiguous. These are structural bugs, never legitimate guest outcomes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExecutorControlInvariantError {
    RunQueueUnknownThread(ThreadId),
    DuplicateRunQueueThread(ThreadId),
    RunQueueThreadNotRunnable(ThreadId, ThreadState),
    RunnableThreadMissingFromRunQueue(ThreadId),
    RunningUnknownThread(ThreadId),
    RunningThreadStateMismatch(ThreadId, ThreadState),
    RunningThreadNeverStarted(ThreadId),
    RunningThreadQueued(ThreadId),
    RunningThreadHasPendingResume(ThreadId),
    RunningStateWithoutOwner(ThreadId),
    MultipleRunningThreads(ThreadId, ThreadId),
    WaiterUnknownThread(ThreadId),
    DuplicateWaiterThread(ThreadId),
    ReceiverWaiterStateMismatch(ThreadId, ThreadState),
    SenderWaiterStateMismatch(ThreadId, ThreadState),
    BlockedThreadMissingWaiter(ThreadId),
    PendingResumeUnknownThread(ThreadId),
    PendingResumeThreadNotRunnable(ThreadId, ThreadState),
    PendingResumeThreadNotQueued(ThreadId),
    StartResumeForStartedThread(ThreadId),
    NeverStartedRunnableThreadMissingStart(ThreadId),
}

#[derive(Default)]
pub struct Executor {
    /// Base of the one process-wide rdram buffer, set once at boot via
    /// `set_rdram_base`. Held so queue mutations can mirror `validCount`/
    /// `first`/`msgCount` back into each `OSMesgQueue`'s real rdram struct
    /// (see `MQ_*_OFF` above) -- guest code reads those fields directly.
    /// `None` until set (tests that never boot a real rdram just skip the
    /// mirror, which is correct: there is no guest struct to keep in sync).
    rdram_base: Option<*mut u8>,
    /// Byte length of the `rdram_base` allocation. Zero means "not yet
    /// registered"; the length-less legacy registration stores `usize::MAX`
    /// (no bounds enforcement). Default-derived as 0, which is fine: mirrors
    /// only run once a base is registered, and both registration paths set it.
    rdram_len: usize,
    /// `Box<GameThread>`, NOT a bare `GameThread` -- see `run_one_step`'s
    /// doc comment for the second block/wake defect this closes (the OoT
    /// Main-resume SIGBUS): a `GameThread`'s coroutine body can
    /// synchronously call back into `with_executor` and insert a NEW entry
    /// into this SAME map (spawning another thread) WHILE `run_one_step` is
    /// still holding a live `&mut GameThread` into it for the resume that's
    /// currently executing. A bare-`GameThread`-valued `HashMap` would move
    /// every existing value on a reallocating insert, invalidating that
    /// live reference (UB) -- boxing means a reallocation only ever moves
    /// `Box` HANDLES; each `GameThread` stays at a fixed heap address for
    /// its entire life, so a reference obtained by dereferencing its `Box`
    /// before the reentrant insert stays valid through and after it.
    threads: HashMap<ThreadId, Box<GameThread>>,
    /// The priority-ordered run queue: runnable thread ids. Re-sorted by
    /// priority (descending) whenever a thread becomes runnable, so
    /// `pick_next` is always "the highest-priority entry," matching
    /// libultra's documented thread-manager rule ("the highest priority
    /// runnable thread always runs" -- public libultra manual, Thread
    /// Manager section) cited in `docs/DESIGN.md` section 2.
    run_queue: Vec<ThreadId>,
    queues: HashMap<u32, MesgQueue>,
    timers: TimerWheel,
    /// `osSetEventMesg`'s registration table (`docs/DESIGN.md` section 2:
    /// "a small EventTable... populated by osSetEventMesg_recomp"). Keyed
    /// by the libultra `OS_EVENT_*` code; value is the queue+message an
    /// external event posts, via the exact same `MesgQueue` API a guest
    /// `osSendMesg` would use -- see `post_external_message`, which is the
    /// ONE injection point both this table's automatic posts and any
    /// direct external post go through.
    event_table: HashMap<u32, (u32, Mesg)>,
    /// Currently-running thread, if any -- `None` only before the very
    /// first resume and after the whole run queue has gone idle.
    running: Option<ThreadId>,
    /// What to resume a thread WITH the next time it's picked off the run
    /// queue (e.g. `Resume::Delivered(msg)` for a thread just woken by a
    /// message arrival). Populated by `wake_thread`/`handle_yield`,
    /// consumed by `run_one_step`. A thread absent from this map resumes
    /// with `Resume::Continue` (a plain scheduling-round resume, e.g. after
    /// `pause_self`).
    pending_resume: HashMap<ThreadId, Resume>,
    /// Virtual clock. Advanced only by `advance_time` (the host driver's
    /// entry point for VI-tick-equivalent progress) -- never wall-clock,
    /// per the task's explicit "no wall-clock in core" requirement.
    sim_time: u64,
    /// Always-on monotonic count of guest coroutine resumes. Unlike the
    /// optional diagnostic trace, release-boundary freshness can rely on this
    /// even when tracing is disabled or cleared.
    resume_epoch: u64,
    /// MIPS CP0 Count register. It advances by the same deterministic guest
    /// cycle delta as `sim_time`, but `osSetTime` does not rewrite it: the OS
    /// time base and the hardware free-running Count register are distinct
    /// state on real hardware.
    cp0_count: u32,
    /// Count increments once per two guest CPU cycles. Retaining the odd-cycle
    /// phase makes split checkpoint advances identical to one combined
    /// advance.
    cp0_count_phase: u8,
    /// CP0 Compare and its level-sensitive IP7 latch. Equality raises the
    /// latch; only a Compare write clears it.
    cp0_compare: u32,
    cp0_timer_pending: bool,
    /// Cumulative OS_EVENT_VI ticks `advance_time` has fired since boot.
    /// A COUNT, never a rate: rates need wall-clock, which this crate does not
    /// have by design. `fn64-abi` pairs it with `Instant` (ROADMAP R5 probe 3).
    retrace_ticks_fired: u64,
    trace: TraceLog,
    /// VI/SI/RSP host-side hardware-model state -- see `peripherals.rs`'s
    /// module doc for why these are grouped separately from this struct's
    /// own scheduling/queue/timer state. `Executor`'s own VI/SI/RSP-named
    /// methods (below) are thin delegations to this field, so
    /// `fn64-abi`'s call sites are unaffected by this split.
    peripherals: Peripherals,
}

/// The single, explicit host-side injection point external completions
/// (SI/PI/VI-style) enter through. See module doc and `docs/DESIGN.md`
/// section 2's "VI/timer event delivery" / "SI/PI completion messages"
/// writeups: an external event "posts a message and returns to whatever
/// the CPU was doing" on real hardware -- it never itself executes as a
/// second runnable game thread. This enum is deliberately the ONLY
/// parameter shape `Executor::inject_event` accepts, so there is no second,
/// looser API (e.g. a raw `queue_addr, msg` pair with no named source) that
/// could bypass the intent of "structurally impossible to touch
/// queue/thread state from outside the executor" -- every legal injection
/// is a named, closed enum variant, reviewed as a whole here rather than
/// discoverable one ad hoc call site at a time.
#[derive(Copy, Clone, Debug)]
pub enum ExternalEvent {
    /// A libultra `OS_EVENT_*` code fired (SI/PI/VI/AI/etc.); looked up in
    /// the `EventTable` and posted through the same `MesgQueue` path a
    /// guest `osSendMesg` uses -- see `docs/DESIGN.md` section 2's
    /// "closing the asymmetry" paragraph: one code path, whether the
    /// sender is guest code or the host driver.
    OsEvent(u32),
    /// A direct post to a specific queue, for completions that aren't
    /// modeled as a libultra `OS_EVENT_*` code (e.g. a DMA controller with
    /// its own private completion queue). Still funneled through the same
    /// `deliver_or_block` logic as everything else.
    DirectPost { queue_addr: RdramAddr, msg: Mesg },
}

impl Executor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate relationships distributed across the scheduler, queues, and
    /// pending-resume table. A failure means the executor has already entered
    /// a state its next scheduling operation cannot interpret unambiguously.
    pub fn validate_control_evidence_invariants(
        &self,
    ) -> Result<(), ExecutorControlInvariantError> {
        let mut queued = HashSet::with_capacity(self.run_queue.len());
        for &id in &self.run_queue {
            let Some(thread) = self.threads.get(&id) else {
                return Err(ExecutorControlInvariantError::RunQueueUnknownThread(id));
            };
            if !queued.insert(id) {
                return Err(ExecutorControlInvariantError::DuplicateRunQueueThread(id));
            }
            if thread.state() != ThreadState::Runnable {
                return Err(ExecutorControlInvariantError::RunQueueThreadNotRunnable(
                    id,
                    thread.state(),
                ));
            }
        }

        let mut state_running = None;
        for (&id, thread) in &self.threads {
            match thread.state() {
                ThreadState::Runnable if !queued.contains(&id) => {
                    return Err(
                        ExecutorControlInvariantError::RunnableThreadMissingFromRunQueue(id),
                    );
                }
                ThreadState::Running => {
                    if let Some(first) = state_running.replace(id) {
                        return Err(ExecutorControlInvariantError::MultipleRunningThreads(
                            first, id,
                        ));
                    }
                }
                _ => {}
            }
        }

        match self.running {
            Some(id) => {
                let Some(thread) = self.threads.get(&id) else {
                    return Err(ExecutorControlInvariantError::RunningUnknownThread(id));
                };
                if thread.state() != ThreadState::Running {
                    return Err(ExecutorControlInvariantError::RunningThreadStateMismatch(
                        id,
                        thread.state(),
                    ));
                }
                if !thread.has_started() {
                    return Err(ExecutorControlInvariantError::RunningThreadNeverStarted(id));
                }
                if queued.contains(&id) {
                    return Err(ExecutorControlInvariantError::RunningThreadQueued(id));
                }
                if self.pending_resume.contains_key(&id) {
                    return Err(ExecutorControlInvariantError::RunningThreadHasPendingResume(id));
                }
                if state_running != Some(id) {
                    return Err(ExecutorControlInvariantError::RunningStateWithoutOwner(id));
                }
            }
            None => {
                if let Some(id) = state_running {
                    return Err(ExecutorControlInvariantError::RunningStateWithoutOwner(id));
                }
            }
        }

        let mut waiters = HashSet::new();
        for queue in self.queues.values() {
            let snapshot = queue.evidence_snapshot();
            for blocked in snapshot.blocked_receivers {
                let id = blocked.id;
                let Some(thread) = self.threads.get(&id) else {
                    return Err(ExecutorControlInvariantError::WaiterUnknownThread(id));
                };
                if !waiters.insert(id) {
                    return Err(ExecutorControlInvariantError::DuplicateWaiterThread(id));
                }
                if thread.state() != ThreadState::BlockedOnRecv {
                    return Err(ExecutorControlInvariantError::ReceiverWaiterStateMismatch(
                        id,
                        thread.state(),
                    ));
                }
            }
            for blocked in snapshot.blocked_senders {
                let id = blocked.id;
                let Some(thread) = self.threads.get(&id) else {
                    return Err(ExecutorControlInvariantError::WaiterUnknownThread(id));
                };
                if !waiters.insert(id) {
                    return Err(ExecutorControlInvariantError::DuplicateWaiterThread(id));
                }
                if thread.state() != ThreadState::BlockedOnSend {
                    return Err(ExecutorControlInvariantError::SenderWaiterStateMismatch(
                        id,
                        thread.state(),
                    ));
                }
            }
        }
        for (&id, thread) in &self.threads {
            if matches!(
                thread.state(),
                ThreadState::BlockedOnRecv | ThreadState::BlockedOnSend
            ) && !waiters.contains(&id)
            {
                return Err(ExecutorControlInvariantError::BlockedThreadMissingWaiter(
                    id,
                ));
            }
        }

        for (&id, &resume) in &self.pending_resume {
            let Some(thread) = self.threads.get(&id) else {
                return Err(ExecutorControlInvariantError::PendingResumeUnknownThread(
                    id,
                ));
            };
            if thread.state() != ThreadState::Runnable {
                return Err(
                    ExecutorControlInvariantError::PendingResumeThreadNotRunnable(
                        id,
                        thread.state(),
                    ),
                );
            }
            if !queued.contains(&id) {
                return Err(ExecutorControlInvariantError::PendingResumeThreadNotQueued(
                    id,
                ));
            }
            if resume == Resume::Start && thread.has_started() {
                return Err(ExecutorControlInvariantError::StartResumeForStartedThread(
                    id,
                ));
            }
        }
        for (&id, thread) in &self.threads {
            if thread.state() == ThreadState::Runnable
                && !thread.has_started()
                && self.pending_resume.get(&id) != Some(&Resume::Start)
            {
                return Err(
                    ExecutorControlInvariantError::NeverStartedRunnableThreadMissingStart(id),
                );
            }
        }
        Ok(())
    }

    /// Capture deterministic executor-control evidence. Hash-owned families
    /// are canonicalized; behaviorally meaningful FIFO/run order is retained.
    /// Traces, host pointers, and native coroutine continuations are excluded.
    pub fn control_evidence_snapshot(&self) -> ExecutorControlEvidenceSnapshot {
        self.validate_control_evidence_invariants()
            .unwrap_or_else(|error| {
                panic!("Executor control evidence invariant failed: {error:?}")
            });

        let rdram = match (self.rdram_base, self.rdram_len) {
            (None, _) => RdramRegistrationEvidenceSnapshot::Absent,
            (Some(_), usize::MAX) => RdramRegistrationEvidenceSnapshot::LegacyUnbounded,
            (Some(_), len) => RdramRegistrationEvidenceSnapshot::Present {
                len: u64::try_from(len).expect("RDRAM length does not fit evidence width"),
            },
        };
        let mut threads: Vec<_> = self
            .threads
            .values()
            .map(|thread| ThreadEvidenceSnapshot {
                id: thread.id,
                priority: thread.priority,
                state: thread.state(),
                started: thread.has_started(),
            })
            .collect();
        threads.sort_by_key(|thread| thread.id);

        let mut pending_resumes: Vec<_> = self
            .pending_resume
            .iter()
            .map(|(&thread, &resume)| PendingResumeEvidenceSnapshot { thread, resume })
            .collect();
        pending_resumes.sort_by_key(|pending| pending.thread);

        let mut queues: Vec<_> = self
            .queues
            .iter()
            .map(|(&address, queue)| ExecutorQueueEvidenceSnapshot {
                address: RdramAddr::from_offset(address),
                queue: queue.evidence_snapshot(),
            })
            .collect();
        queues.sort_by_key(|queue| queue.address.offset());

        let mut event_table: Vec<_> = self
            .event_table
            .iter()
            .map(
                |(&event, &(queue_addr, msg))| EventRegistrationEvidenceSnapshot {
                    event,
                    queue_addr: RdramAddr::from_offset(queue_addr),
                    msg,
                },
            )
            .collect();
        event_table.sort_by_key(|registration| registration.event);

        ExecutorControlEvidenceSnapshot {
            rdram,
            threads,
            run_queue: self.run_queue.clone(),
            pending_resumes,
            queues,
            timers: self.timers.evidence_snapshot(),
            event_table,
            running: self.running.map_or(
                ExecutorRunningEvidenceSnapshot::Quiescent,
                ExecutorRunningEvidenceSnapshot::Active,
            ),
            sim_time: self.sim_time,
            cp0_count: self.cp0_count,
            cp0_count_phase: self.cp0_count_phase,
            cp0_compare: self.cp0_compare,
            cp0_timer_pending: self.cp0_timer_pending,
        }
    }

    pub fn peripherals_evidence_snapshot(&self) -> crate::PeripheralsEvidenceSnapshot {
        self.peripherals.evidence_snapshot()
    }

    pub fn trace(&self) -> &[crate::trace::TraceEvent] {
        self.trace.events()
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.trace.set_enabled(enabled);
    }

    /// Arm incremental crash-safe trace flushing -- see
    /// `TraceLog::set_sink_file`'s doc comment for why this exists (a
    /// SIGSEGV mid-boot must not lose the whole session's trace).
    pub fn set_trace_sink_file(&mut self, path: &str) -> std::io::Result<()> {
        self.trace.set_sink_file(path)
    }

    pub fn sim_time(&self) -> u64 {
        self.sim_time
    }

    pub fn resume_epoch(&self) -> u64 {
        self.resume_epoch
    }

    /// Cumulative OS_EVENT_VI retrace ticks fired since boot (ROADMAP R5
    /// probe 3). Pair with host wall-clock to get a rate -- this crate cannot,
    /// and must not, do that itself.
    pub fn retrace_ticks_fired(&self) -> u64 {
        self.retrace_ticks_fired
    }

    /// `osSetTime(OSTime time)` -- per the public libultra manual, sets the
    /// system's current time counter. This crate has no wall-clock (only
    /// virtual `sim_time`, per `docs/DESIGN.md`'s explicit "no wall-clock in
    /// core" rule), so `osGetTime`'s counterpart reads `sim_time()` and this
    /// setter reassigns it directly. CP0 Count is separate hardware state:
    /// changing OSTime does not rewrite Count, while later positive guest-time
    /// advances continue to drive both timer scheduling and Count progress.
    pub fn set_sim_time(&mut self, time: u64) {
        self.sim_time = time;
    }

    pub fn cp0_count(&self) -> u32 {
        self.cp0_count
    }

    pub fn set_cp0_count(&mut self, count: u32) {
        self.cp0_count = count;
        self.cp0_count_phase = 0;
    }

    /// Restore the CPU clock state captured at an external architectural
    /// handoff. Unlike [`Self::write_cp0_compare`], this is not an emulated
    /// MTC0 and therefore retains the captured IP7 latch.
    pub fn restore_cp0_clock(&mut self, count: u32, compare: u32, timer_pending: bool) {
        self.cp0_count = count;
        self.cp0_count_phase = 0;
        self.cp0_compare = compare;
        self.cp0_timer_pending = timer_pending;
    }

    pub fn cp0_compare(&self) -> u32 {
        self.cp0_compare
    }

    /// MTC0 Compare both replaces the threshold and acknowledges IP7.
    pub fn write_cp0_compare(&mut self, compare: u32) {
        self.cp0_compare = compare;
        self.cp0_timer_pending = false;
    }

    pub fn cp0_timer_pending(&self) -> bool {
        self.cp0_timer_pending
    }

    /// Register the process-wide rdram base so queue mutations can mirror
    /// their `validCount`/`first`/`msgCount` back into the guest's real
    /// `OSMesgQueue` struct (see `MQ_*_OFF` and `rdram_base`'s field doc).
    ///
    /// # Safety
    /// `base` must point to the single live rdram buffer this executor's
    /// guest code runs against, valid for the executor's whole lifetime
    /// (the boot harness's one `rdram` Vec -- same buffer every shim already
    /// receives). Passing a dangling or wrong-sized base is UB, identical to
    /// the contract every `*_recomp` shim's `rdram` argument already carries.
    pub unsafe fn set_rdram_base(&mut self, base: *mut u8) {
        self.set_rdram_base_with_len(base, usize::MAX);
    }

    /// Like [`Self::set_rdram_base`], with the allocation's byte length so
    /// queue mirrors can bounds-check guest-supplied `OSMesgQueue*` addresses
    /// and fail loudly instead of writing out of bounds (a corrupt guest
    /// pointer used to SIGSEGV with no diagnosis).
    ///
    /// # Safety
    /// Same contract as [`Self::set_rdram_base`].
    pub unsafe fn set_rdram_base_with_len(&mut self, base: *mut u8, len: usize) {
        self.rdram_base = Some(base);
        self.rdram_len = len;
    }

    /// Mirror a queue's live count/head into its rdram `OSMesgQueue` struct
    /// so guest `MQ_GET_COUNT`/`MQ_IS_FULL`/`MQ_IS_EMPTY` reads see truth.
    /// No-op if no rdram base is registered (unit tests) or the queue isn't
    /// known. `msgCount` (capacity) is written too, defending against a
    /// guest that reads it before libultra's real `osCreateMesgQueue` would
    /// have set it -- our `create_mesg_queue` shim replaces that function.
    fn mirror_queue_to_rdram(&self, mq_addr: RdramAddr) {
        let Some(base) = self.rdram_base else { return };
        let Some(queue) = self.queues.get(&mq_addr.offset()) else {
            return;
        };
        let valid = queue.valid_count() as i32;
        let first = queue.first_index() as i32;
        let msgcount = queue.capacity() as i32;
        // Same KSEG0-relative translation MEM_W uses: RdramAddr::offset() is
        // already the physical rdram byte offset, so write straight there in
        // native byte order (matching the recomp's `*(int32_t*)` MEM_W).
        let end = mq_addr.offset() as usize + MQ_MSGCOUNT_OFF as usize + 4;
        assert!(
            end <= self.rdram_len,
            "OSMesgQueue mirror at rdram offset {:#x} runs past the {:#x}-byte RDRAM \
             allocation -- guest code handed the executor a corrupt OSMesgQueue pointer",
            mq_addr.offset(),
            self.rdram_len,
        );
        let write = |off: u32, val: i32| unsafe {
            let o = mq_addr.offset().wrapping_add(off) as usize;
            std::ptr::copy_nonoverlapping(val.to_ne_bytes().as_ptr(), base.add(o), 4);
        };
        write(MQ_VALIDCOUNT_OFF, valid);
        write(MQ_FIRST_OFF, first);
        write(MQ_MSGCOUNT_OFF, msgcount);
    }

    // ---- OSThread lifecycle -------------------------------------------

    /// `osCreateThread(t, id, entry, arg, stack_top, pri)`. Does not make
    /// the thread runnable -- matching real libultra, `osStartThread` does
    /// that. `body` is the thread's entry-point closure (an `fn64-abi` shim
    /// supplies the real recompiled-entry-point trampoline; see
    /// `docs/DESIGN.md` section 1).
    pub fn create_thread(
        &mut self,
        id: ThreadId,
        priority: Priority,
        body: impl FnOnce(&corosensei::Yielder<Resume, Yield>, Resume) + 'static,
    ) {
        assert!(
            !self.threads.contains_key(&id),
            "osCreateThread: thread id {id} already exists"
        );
        self.threads
            .insert(id, Box::new(GameThread::new(id, priority, body)));
    }

    /// `osStartThread(t)`. Puts a stopped thread on the run queue. Its first
    /// start installs `Resume::Start`; a later restart preserves the
    /// coroutine's already-established continuation protocol.
    pub fn start_thread(&mut self, id: ThreadId) {
        let thread = self
            .threads
            .get_mut(&id)
            .unwrap_or_else(|| panic!("osStartThread: no such thread id {id}"));
        assert_eq!(
            thread.state(),
            ThreadState::Stopped,
            "osStartThread: thread {id} is not in the Stopped state"
        );
        if !thread.has_started() {
            self.pending_resume.insert(id, Resume::Start);
        }
        thread.set_state(ThreadState::Runnable);
        self.run_queue.push(id);
        self.sort_run_queue();
    }

    /// `osSetThreadPri(t, pri)`. Re-sorts the run queue if the thread is
    /// currently runnable, so a priority change takes effect on the very
    /// next scheduling decision -- matching libultra's documented
    /// immediate-effect semantics (raising a blocked/runnable thread's
    /// priority above the running thread's preempts it at the next
    /// yield/scheduling point).
    pub fn set_thread_pri(&mut self, id: ThreadId, priority: Priority) {
        let thread = self
            .threads
            .get_mut(&id)
            .unwrap_or_else(|| panic!("osSetThreadPri: no such thread id {id}"));
        thread.priority = priority;
        self.sort_run_queue();
    }

    pub fn thread_pri(&self, id: ThreadId) -> Priority {
        self.threads
            .get(&id)
            .unwrap_or_else(|| panic!("osGetThreadPri: no such thread id {id}"))
            .priority
    }

    /// `osDestroyThread(t)` / a thread's coroutine body returning. Removes
    /// it from the run queue and any blocked list it might be on.
    pub fn destroy_thread(&mut self, id: ThreadId) {
        self.remove_thread_from_waiters(id);
        if let Some(thread) = self.threads.get_mut(&id) {
            thread.set_state(ThreadState::Dead);
        }
        self.run_queue.retain(|t| *t != id);
        self.pending_resume.remove(&id);
        if self.running == Some(id) {
            self.running = None;
        }
    }

    /// Make this executor safe to drop at the terminal host process boundary.
    ///
    /// This is not guest thread destruction and does not resume guest code.
    /// The caller must be between scheduling steps (`running == None`) and
    /// must never use the executor again. Started, unfinished coroutine
    /// objects are detached so their `Drop` implementation cannot force an
    /// unwind through generated C and non-unwind ABI frames.
    pub fn prepare_process_exit(&mut self) -> ProcessExitSummary {
        assert!(
            self.running.is_none(),
            "Executor::prepare_process_exit cannot run while guest thread {:?} owns the run token",
            self.running
        );
        self.validate_control_evidence_invariants()
            .unwrap_or_else(|error| {
                panic!("Executor::prepare_process_exit found corrupt scheduler state: {error:?}")
            });
        let threads = u32::try_from(self.threads.len())
            .expect("Executor::prepare_process_exit thread count exceeds u32");
        let detached_coroutines = self
            .threads
            .values_mut()
            .map(|thread| usize::from(thread.detach_for_process_exit()))
            .sum::<usize>();
        self.threads.clear();
        self.run_queue.clear();
        self.pending_resume.clear();
        ProcessExitSummary {
            threads,
            detached_coroutines: u32::try_from(detached_coroutines)
                .expect("Executor::prepare_process_exit detached count exceeds u32"),
        }
    }

    /// `osStopThread(t)` -- per the public libultra manual's Thread Manager
    /// section, distinct from `osDestroyThread`: removes the thread from
    /// the run queue (it stops being scheduled) but does NOT tear down its
    /// identity/stack the way destroy does -- a stopped thread can be
    /// `osStartThread`'d again later, matching real hardware's documented
    /// "stop, don't destroy" semantics. Implemented as the same run-queue
    /// removal `destroy_thread` does, but setting `ThreadState::Stopped`
    /// (the same state `osCreateThread` itself starts a thread in, per
    /// `GameThread::new`) rather than `Dead`, and leaving
    /// `HostState::thread_handles`' identity mapping untouched (that's
    /// `fn64-abi`'s concern, not this module's -- `destroy_thread` doesn't
    /// touch it either).
    pub fn stop_thread(&mut self, id: ThreadId) {
        self.remove_thread_from_waiters(id);
        if let Some(thread) = self.threads.get_mut(&id) {
            thread.set_state(ThreadState::Stopped);
        }
        self.run_queue.retain(|t| *t != id);
        // Interleaving closed here: A is woken with a pending queue result,
        // B stops A before its next resume, then B restarts A. The stopped
        // interval cancels that scheduled resume; retaining it would replay a
        // stale delivery after restart (and violates the invariant that every
        // pending resume belongs to a currently runnable thread).
        self.pending_resume.remove(&id);
        if self.running == Some(id) {
            self.running = None;
        }
    }

    fn remove_thread_from_waiters(&mut self, id: ThreadId) {
        // Interleaving closed here: thread A is parked on a queue, thread B
        // stops or destroys A, then a device/timer post reaches that queue.
        // Removing A from every queue-owned role before changing its state
        // prevents the later post from popping the stale id and making A
        // runnable again. One sweep covers recv and send waiters across every
        // queue, including any inconsistent duplicate that must not survive.
        for queue in self.queues.values_mut() {
            queue.remove_waiter(id);
        }
    }

    fn sort_run_queue(&mut self) {
        let threads = &self.threads;
        // Descending priority; stable sort preserves FIFO order among
        // equal priorities, matching libultra's documented "equal-priority
        // threads run round-robin in the order they became runnable"
        // thread-manager rule.
        self.run_queue
            .sort_by_key(|id| std::cmp::Reverse(threads[id].priority));
    }

    // ---- OSMesgQueue registration ---------------------------------------

    /// `osCreateMesgQueue(mq, msg, count)`. Always produces a genuinely
    /// empty `MesgQueue` (rung 12: `MesgQueue::new` is the only
    /// constructor and always starts with empty blocked lists -- see
    /// `mesgqueue.rs`'s module doc). Re-creating at an already-used address
    /// (the reference runtime's exact rung-12 failure surface: an
    /// un-reset queue struct at a reused address) always replaces the
    /// entry wholesale via this one path, never a partial field write.
    pub fn create_mesg_queue(&mut self, mq_addr: RdramAddr, capacity: usize) {
        self.queues
            .insert(mq_addr.offset(), MesgQueue::new(capacity.max(1)));
        // Initialize the guest's rdram struct (validCount=0, first=0,
        // msgCount=capacity) so a guest reading MQ_IS_FULL/MQ_GET_COUNT
        // before ever sending sees a correct empty-with-capacity queue,
        // not the stale zeros a never-mirrored struct would show.
        self.mirror_queue_to_rdram(mq_addr);
    }

    fn queue_mut(&mut self, mq_addr: RdramAddr) -> &mut MesgQueue {
        // An untracked queue is one the guest never passed to
        // `osCreateMesgQueue` -- i.e. a bzero'd `OSMesgQueue` struct used
        // directly. Real libultra honors such a queue as zero-capacity
        // (`msgCount == 0`): every NOBLOCK send finds it full (-1), every
        // NOBLOCK recv finds it empty (-1). OoT's audio driver relies on
        // exactly this for `gAudioCtx.asyncLoadUnkMediumQueue`, which the
        // decomp never creates (`audio/internal/load.c:1652,1717-1718`). Lazily
        // install a zero-capacity queue rather than panicking, so we execute
        // that real behavior faithfully instead of aborting. See
        // `MesgQueue::zero_capacity`'s doc comment. A genuine harness gap
        // (a queue that SHOULD have been created with capacity) still surfaces
        // as its own downstream symptom, not a masked no-op, because a
        // wrongly-zero-capacity queue makes every send/recv fail visibly.
        self.queues
            .entry(mq_addr.offset())
            .or_insert_with(MesgQueue::zero_capacity)
    }

    // ---- osSetEventMesg / external event injection ----------------------

    /// `osSetEventMesg(event, mq, msg)`.
    pub fn set_event_mesg(&mut self, event: u32, mq_addr: RdramAddr, msg: Mesg) {
        self.event_table.insert(event, (mq_addr.offset(), msg));
    }

    /// Whether a guest `osSetEventMesg(code, ..)` registration exists yet --
    /// used by host-driven event sources (VI retrace, SI DMA completion) to
    /// decide whether to actually post via `inject_event` or silently skip,
    /// matching real hardware where the interrupt fires either way but only
    /// has an observable effect once software has hooked it. See
    /// `advance_time`'s VI-retrace handling and `fn64-abi`'s
    /// `__osSiRawStartDma_recomp` for the two current callers.
    pub fn event_table_contains(&self, event: u32) -> bool {
        self.event_table.contains_key(&event)
    }

    /// Return the exact queue/message target registered for an OS event.
    /// Synchronous manager calls use this read-only view to validate that
    /// the queue they are about to block on is the event target that can
    /// wake them; accepting only the event code would permit a completion
    /// to post to a different queue and strand the caller forever.
    pub fn event_registration(&self, event: u32) -> Option<(RdramAddr, Mesg)> {
        self.event_table
            .get(&event)
            .map(|&(queue_offset, msg)| (RdramAddr::from_offset(queue_offset), msg))
    }

    /// THE single, explicit host-side injection point. Every SI/PI/VI/AI
    /// completion, and every fired timer (see `advance_time`), funnels
    /// through this same function -- see `ExternalEvent`'s doc comment and
    /// `docs/DESIGN.md` section 2's "closing the asymmetry" paragraph.
    /// Nothing outside `Executor` can reach `MesgQueue`'s blocked lists or
    /// `run_queue` at all (both are private fields; `MesgQueue` itself has
    /// no public raw-field-write path per rung 12's module doc), so this
    /// function is not merely "the recommended way in" -- it is
    /// structurally the ONLY way in, which is the rung-18 "bypass write"
    /// class made unrepresentable: there is no second function signature
    /// anywhere in this crate that a future caller could reach for instead.
    pub fn inject_event(&mut self, event: ExternalEvent) {
        let (queue_addr, msg) = match event {
            ExternalEvent::OsEvent(code) => {
                let (addr, msg) = *self.event_table.get(&code).unwrap_or_else(|| {
                    panic!("inject_event: OS_EVENT code {code} has no osSetEventMesg registration")
                });
                (RdramAddr::from_offset(addr), msg)
            }
            ExternalEvent::DirectPost { queue_addr, msg } => (queue_addr, msg),
        };
        self.deliver_or_enqueue(queue_addr, msg, None);
    }

    /// Post a synchronous CPU exception event when the guest registered one.
    /// Libultra permits fault/break events to be unregistered; in that case
    /// the faulted thread still stops, but there is no queue notification.
    /// Peripheral completion paths keep using [`Self::inject_event`] so a
    /// missing required registration remains loud there.
    pub fn inject_optional_os_event(&mut self, code: u32) -> bool {
        let Some(&(addr, msg)) = self.event_table.get(&code) else {
            return false;
        };
        self.deliver_or_enqueue(RdramAddr::from_offset(addr), msg, None);
        true
    }

    /// Advance the virtual clock the host drives (VI-tick equivalent).
    /// Fires any due timers, posting each one's message through the exact
    /// same `deliver_or_enqueue` path `inject_event` uses -- per
    /// `docs/DESIGN.md` section 2, timer expiry is a host-side scheduling
    /// input, never a coroutine of its own. Also drives the VI retrace
    /// ticker (if armed via `arm_retrace`) -- a real VI interrupt "posts a
    /// message and returns to whatever the CPU was doing" (`docs/DESIGN.md`
    /// section 2's exact framing), which for `OS_EVENT_VI` means routing
    /// through the SAME `event_table`-registration path a guest
    /// `osSetEventMesg` call already populates (the `osCreateViManager`
    /// call site's `osSetEventMesg(7, mq, &retraceMsg)`, per
    /// `games/NWXE/profile.toml`'s rung-11 evidence cited in `vi.rs`) --
    /// never a second, VI-specific delivery path.
    pub fn advance_time(&mut self, now: u64) {
        let elapsed = now.checked_sub(self.sim_time).unwrap_or_else(|| {
            panic!(
                "Executor::advance_time: virtual time cannot move backward from {} to {now}",
                self.sim_time
            )
        });
        let total_cycles = u64::from(self.cp0_count_phase)
            .checked_add(elapsed)
            .expect("CP0 Count phase overflow");
        let increments = total_cycles / 2;
        self.cp0_count_phase = (total_cycles & 1) as u8;
        if increments != 0 {
            let distance = u64::from(self.cp0_compare.wrapping_sub(self.cp0_count));
            let increments_to_match = if distance == 0 { 1u64 << 32 } else { distance };
            if increments >= increments_to_match {
                self.cp0_timer_pending = true;
            }
            self.cp0_count = self.cp0_count.wrapping_add(increments as u32);
        }
        self.peripherals.advance_transfer_paks_to(Cycles::new(now));
        self.sim_time = now;
        let fired = self.timers.advance(now);
        for timer in fired {
            self.deliver_or_enqueue(timer.queue_addr, timer.msg, Some(timer.armed_by));
        }
        // The retrace tick's OWN advance (RetraceSchedule::advance, whether
        // OS_EVENT_VI fired N times this call) is `Peripherals`' job; actual
        // message DELIVERY stays here, since only `Executor` can reach
        // `event_table`/`deliver_or_enqueue` (see `peripherals.rs`'s module
        // doc for why the event table itself is not a peripheral).
        if let Some(tick) = self.peripherals.advance_retrace(now) {
            // Cumulative count of OS_EVENT_VI ticks the schedule has fired.
            // Counted here (not in Peripherals) because this is the seam that
            // knows a tick was really produced. Deliberately a COUNT and not a
            // rate: a rate needs wall-clock, and this crate is wall-clock-free
            // by design (DESIGN.md §1) -- fn64-abi correlates it against
            // Instant to answer R5 probe 3.
            for _ in 0..tick.event_vi_ticks {
                self.deliver_vi_retrace();
            }
        }
    }

    /// Apply one VI retrace. The integrated `DeviceFabric` caller invokes
    /// this only after MI has latched; the standalone executor ticker has no
    /// MI owner. Pending VI state becomes
    /// current before either notification path can wake a guest thread.
    /// Returns whether manager-owned framebuffer, special-feature bits,
    /// black, fade, or repeat-line state changed at this V-blank; mode and
    /// scale changes are omitted. The integrated device path presents every
    /// field because field registers and retrace-seeded noise can change even
    /// when this high-level state does not. Callers must not use this partial
    /// manager delta as presentation admission authority.
    pub fn deliver_vi_retrace(&mut self) -> bool {
        let framebuffer_changed = self.peripherals.vi_latch_retrace();
        self.retrace_ticks_fired = self.retrace_ticks_fired.saturating_add(1);
        // An unregistered OS event is normal during early boot: the hardware
        // source still fires even though no queue is listening yet.
        if self.event_table.contains_key(&OS_EVENT_VI) {
            self.inject_event(ExternalEvent::OsEvent(OS_EVENT_VI));
        }
        // `osViSetEvent` owns a second VI-manager target independent of the
        // general OS_EVENT_VI table, and both fire on the same interrupt.
        if let Some((mq_offset, msg)) = self.peripherals.vi_manager_target_for_retrace() {
            self.deliver_or_enqueue(RdramAddr::from_offset(mq_offset), msg, None);
        }
        framebuffer_changed
    }

    /// Arm the periodic VI retrace ticker at `interval` virtual-time units
    /// per field. See `vi.rs`'s `RetraceSchedule` doc -- not a hardware-
    /// accurate NTSC/PAL timing value, a host-chosen approximation.
    pub fn arm_retrace(&mut self, interval: u64) {
        self.peripherals.arm_retrace(interval);
    }

    // ---- VI (video interface) -------------------------------------------
    //
    // Thin delegations to `Peripherals` -- see `peripherals.rs`'s module doc.
    // No behavior lives here; every method's real implementation is on
    // `Peripherals`, unchanged from when it was this same method's own body.

    pub fn vi(&self) -> &ViState {
        self.peripherals.vi()
    }

    pub fn vi_set_mode(&mut self, mode_ptr: u32) {
        self.peripherals.vi_set_mode(mode_ptr);
    }

    pub fn vi_set_special_features(&mut self, features: u32) {
        self.peripherals.vi_set_special_features(features);
    }

    pub fn vi_set_y_scale(&mut self, scale: f32) {
        self.peripherals.vi_set_y_scale(scale);
    }

    pub fn vi_set_x_scale(&mut self, scale: f32) {
        self.peripherals.vi_set_x_scale(scale);
    }

    /// `osViSetEvent(mq, msg, retraceCount)` -- see `ViState::set_event`'s
    /// doc comment for why this is a separate delivery path from
    /// `osSetEventMesg`/`OS_EVENT_VI`.
    pub fn vi_set_event(&mut self, mq_addr: RdramAddr, msg: Mesg, retrace_count: u32) {
        self.peripherals.vi_set_event(mq_addr, msg, retrace_count);
    }

    pub fn vi_set_black(&mut self, active: bool) {
        self.peripherals.vi_set_black(active);
    }

    pub fn vi_set_fade(&mut self, active: bool, factor: u16) {
        self.peripherals.vi_set_fade(active, factor);
    }

    pub fn vi_set_repeat_line(&mut self, active: bool) {
        self.peripherals.vi_set_repeat_line(active);
    }

    /// `osViSwapBuffer(frameBufPtr)`. Returns the newly-current framebuffer
    /// address so the caller (the `fn64-abi` shim) can hand it straight to
    /// the harness's framebuffer-capture hook without a second lookup. Trace
    /// recording stays here (needs `sim_time`, an `Executor`-owned field --
    /// see `peripherals.rs`'s module doc).
    pub fn vi_swap_buffer(&mut self, frame_buf: RdramAddr) -> RdramAddr {
        self.peripherals.vi_swap_buffer(frame_buf);
        let sim_time = self.sim_time;
        self.trace.record(
            sim_time,
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Graphics,
                ucode: frame_buf.offset(),
            },
        );
        frame_buf
    }

    // ---- SI/PIF (controller probe) ---------------------------------------

    pub fn pif(&self) -> &PifModel {
        self.peripherals.pif()
    }

    /// Feed a controller's live button/stick state for `port` -- the host side
    /// of the input seam. A subsequent `osContGetReadData` for that port
    /// reflects it. See `si::PifModel::set_input` / `peripherals.rs`.
    pub fn set_controller_input(&mut self, port: usize, input: crate::si::ContInput) {
        self.peripherals.set_controller_input(port, input);
    }

    pub fn set_controller_port_state(&mut self, port: usize, state: crate::si::PortState) {
        self.peripherals.set_controller_port_state(port, state);
        self.peripherals
            .advance_transfer_paks_to(Cycles::new(self.sim_time));
    }

    pub fn attach_controller_pak(&mut self, port: usize, pak: crate::pfs::ControllerPak) {
        self.peripherals.attach_controller_pak(port, pak);
    }

    pub fn set_rumble(&mut self, port: usize, active: bool) -> Result<(), crate::si::RumbleError> {
        self.peripherals.set_rumble(port, active)
    }

    pub fn controller_pak(&self, port: usize) -> Option<&crate::pfs::ControllerPak> {
        self.peripherals.controller_pak(port)
    }

    pub fn controller_pak_mut(&mut self, port: usize) -> Option<&mut crate::pfs::ControllerPak> {
        self.peripherals.controller_pak_mut(port)
    }

    pub fn transfer_pak(&self, port: usize) -> Option<&crate::transfer_pak::TransferPak> {
        self.peripherals.transfer_pak(port)
    }

    pub fn transfer_pak_mut(
        &mut self,
        port: usize,
    ) -> Option<&mut crate::transfer_pak::TransferPak> {
        self.peripherals.transfer_pak_mut(port)
    }

    pub fn insert_transfer_pak_cartridge(
        &mut self,
        port: usize,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
    ) -> Result<(), crate::transfer_pak::TransferPakError> {
        self.peripherals
            .advance_transfer_paks_to(Cycles::new(self.sim_time));
        self.peripherals
            .insert_transfer_pak_cartridge(port, rom, ram)
    }

    pub fn insert_transfer_pak_cartridge_with_battery(
        &mut self,
        port: usize,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
        restore: Option<crate::transfer_pak::Mbc3BatteryRestore>,
    ) -> Result<(), crate::transfer_pak::TransferPakError> {
        self.peripherals
            .advance_transfer_paks_to(Cycles::new(self.sim_time));
        self.peripherals
            .insert_transfer_pak_cartridge_with_battery(port, rom, ram, restore)
    }

    pub fn checkpoint_transfer_pak_battery(
        &mut self,
        port: usize,
        checkpoint: crate::transfer_pak::HostUnixNanos,
    ) -> Result<
        Option<crate::transfer_pak::Mbc3BatteryMetadata>,
        crate::transfer_pak::TransferPakError,
    > {
        self.peripherals.checkpoint_transfer_pak_battery(
            port,
            Cycles::new(self.sim_time),
            checkpoint,
        )
    }

    pub fn advance_transfer_paks_to(&mut self, now: Cycles) {
        self.peripherals.advance_transfer_paks_to(now);
    }

    pub fn voice_unit_mut(&mut self, port: usize) -> Option<&mut crate::voice::VoiceUnit> {
        self.peripherals.voice_unit_mut(port)
    }

    pub fn voice_unit(&self, port: usize) -> Option<&crate::voice::VoiceUnit> {
        self.peripherals.voice_unit(port)
    }

    // ---- RSP task submission -----------------------------------------------

    pub fn task_log(&self) -> &TaskLog {
        self.peripherals.task_log()
    }

    /// Retain the complete header admitted by `osSpTaskLoad`. Admission is
    /// deliberately distinct from the later task kickoff trace.
    pub fn admit_task(&mut self, header: OsTaskHeader) {
        self.peripherals.admit_task(header);
    }

    /// Record the `osSpTaskStartGo` boundary after the loaded-task token has
    /// been consumed. A load that is replaced before kickoff cannot emit this
    /// event or satisfy an execution-qualified release closure path.
    pub fn start_task(&mut self, header: OsTaskHeader) {
        let sim_time = self.sim_time;
        let ucode = header.ucode;
        if let Some(kind) = header.kind() {
            self.trace.record(
                sim_time,
                TraceKind::TaskSubmit {
                    task_kind: kind,
                    ucode,
                },
            );
        }
    }

    /// `osSetTimer(t, countdown, interval, mq, msg)`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_timer(
        &mut self,
        countdown: u64,
        interval: u64,
        mq_addr: RdramAddr,
        msg: Mesg,
        armed_by: ThreadId,
    ) -> crate::timer::TimerId {
        self.timers
            .set_timer(self.sim_time, countdown, interval, mq_addr, msg, armed_by)
    }

    /// `osStopTimer(t)`.
    pub fn stop_timer(&mut self, id: crate::timer::TimerId) {
        self.timers.stop_timer(id);
    }

    /// Shared delivery logic for both a guest `osSendMesg` and an external
    /// post (`inject_event`/`advance_time`): try a non-blocking send; if a
    /// receiver is already blocked, wake exactly one in libultra waiter order
    /// (highest priority first, FIFO among ties) and hand it the message directly rather
    /// than routing it back through the ring buffer, matching the
    /// documented `osRecvMesg`/`osSendMesg` handoff shape. If the queue is
    /// full and nothing can be done, the message is dropped -- this is the
    /// real `osSendMesg(..., OS_MESG_NOBLOCK)` semantics for a full queue,
    /// and it's what an external, non-blockable source (a VI retrace, a
    /// completed DMA) must fall back to since there's no guest coroutine
    /// context to block. `attributed_thread` is `None` for genuinely
    /// external sources; used only for trace attribution.
    fn deliver_or_enqueue(
        &mut self,
        queue_addr: RdramAddr,
        msg: Mesg,
        attributed_thread: Option<ThreadId>,
    ) {
        if std::env::var("FN64_DEBUG_SEND").is_ok() {
            eprintln!(
                "[DEBUG deliver_or_enqueue] queue_offset={:#x} msg={msg:#x} attributed_thread={attributed_thread:?}",
                queue_addr.offset()
            );
        }
        let queue = self.queue_mut(queue_addr);
        if queue.has_blocked_receivers() {
            let waiter = queue
                .wake_one_receiver()
                .expect("has_blocked_receivers() was true");
            self.record_queue_op(queue_addr, QueueOpKind::Wake, waiter);
            self.wake_thread(waiter, Resume::Delivered(msg));
            // A direct hand-off to a blocked receiver leaves valid_count
            // unchanged, but mirror anyway to stay unconditionally in sync.
            self.mirror_queue_to_rdram(queue_addr);
            return;
        }
        match queue.try_send(msg) {
            SendResult::Delivered => {
                self.record_queue_op(
                    queue_addr,
                    QueueOpKind::Send,
                    attributed_thread.unwrap_or(0),
                );
            }
            SendResult::WouldBlock => {
                // Full queue, no blocked receiver, no guest coroutine to
                // park (this is an external/timer source) -- real hardware
                // drops the message here exactly as OS_MESG_NOBLOCK would;
                // there is nothing else a non-coroutine caller could do.
            }
        }
        self.mirror_queue_to_rdram(queue_addr);
    }

    /// Move a blocked (or newly-woken-by-timer/external-event) thread back
    /// onto the run queue.
    fn wake_thread(&mut self, id: ThreadId, resume_with: Resume) {
        let thread = self
            .threads
            .get_mut(&id)
            .unwrap_or_else(|| panic!("queue waiter references unknown thread id {id}"));
        assert!(
            matches!(
                thread.state(),
                ThreadState::BlockedOnRecv | ThreadState::BlockedOnSend
            ),
            "queue waiter references thread {id} in non-blocked state {:?}",
            thread.state()
        );
        thread.set_state(ThreadState::Runnable);
        self.run_queue.push(id);
        self.sort_run_queue();
        self.pending_resume.insert(id, resume_with);
    }

    // ---- osSendMesg / osRecvMesg, the guest-facing blocking API ---------

    /// `osSendMesg(mq, msg, flag)`'s core logic, called from the
    /// currently-running thread's yield point (see `run_one_step`). Returns
    /// `SendOutcome::Blocked` if the caller must yield
    /// (`Yield::BlockOnSend`) -- the caller (this module's own
    /// `run_one_step`, standing in for the `fn64-abi` shim's dispatch) is
    /// responsible for registering on the blocked list and actually
    /// suspending; see module doc and `docs/DESIGN.md` section 2's
    /// "Send/recv as coroutine yield points, not thread ops" -- those two
    /// steps happen back-to-back with nothing else running in between,
    /// because this whole function executes on the single executor thread.
    fn try_deliver_send(
        &mut self,
        sender: ThreadId,
        mq_addr: RdramAddr,
        msg: Mesg,
        placement: SendPlacement,
    ) -> SendOutcome {
        if std::env::var("FN64_DEBUG_SEND").is_ok() {
            eprintln!(
                "[DEBUG try_deliver_send] sender={sender} mq_addr_offset={:#x} msg={msg:#x} \
                 placement={placement:?}",
                mq_addr.offset()
            );
        }
        let queue = self.queue_mut(mq_addr);
        if queue.has_blocked_receivers() {
            // Direct hand-off to a waiting receiver: identical for send and
            // jam (there is nothing queued, so head vs tail is moot).
            let waiter = queue
                .wake_one_receiver()
                .expect("has_blocked_receivers() was true");
            self.record_queue_op(mq_addr, QueueOpKind::Send, sender);
            self.record_queue_op(mq_addr, QueueOpKind::Wake, waiter);
            self.wake_thread(waiter, Resume::Delivered(msg));
            self.mirror_queue_to_rdram(mq_addr);
            return SendOutcome::Delivered;
        }
        // Front-insert for jam (osJamMesg), tail-insert for send.
        let insert = match placement {
            SendPlacement::Head => queue.try_jam(msg),
            SendPlacement::Tail => queue.try_send(msg),
        };
        let outcome = match insert {
            SendResult::Delivered => {
                self.record_queue_op(mq_addr, QueueOpKind::Send, sender);
                SendOutcome::Delivered
            }
            SendResult::WouldBlock => SendOutcome::Blocked,
        };
        self.mirror_queue_to_rdram(mq_addr);
        outcome
    }

    fn try_deliver_recv(&mut self, receiver: ThreadId, mq_addr: RdramAddr) -> RecvOutcome {
        let queue = self.queue_mut(mq_addr);
        let recv_result = queue.try_recv();
        let has_blocked_senders = queue.has_blocked_senders();
        let outcome = match recv_result {
            RecvResult::Delivered(msg) => {
                self.record_queue_op(mq_addr, QueueOpKind::Recv, receiver);
                if has_blocked_senders {
                    let waiter = self
                        .queue_mut(mq_addr)
                        .wake_one_sender()
                        .expect("has_blocked_senders() was true");
                    self.record_queue_op(mq_addr, QueueOpKind::Wake, waiter);
                    self.wake_thread(waiter, Resume::SendUnblocked);
                }
                RecvOutcome::Delivered(msg)
            }
            RecvResult::WouldBlock => RecvOutcome::Blocked,
        };
        self.mirror_queue_to_rdram(mq_addr);
        outcome
    }

    fn record_queue_op(&mut self, queue_addr: RdramAddr, op: QueueOpKind, thread: ThreadId) {
        let sim_time = self.sim_time;
        self.trace.record(
            sim_time,
            TraceKind::QueueOp {
                queue: queue_addr,
                op,
                thread,
            },
        );
    }

    // ---- The run loop ----------------------------------------------------

    fn pick_next(&self) -> Option<ThreadId> {
        self.run_queue.first().copied()
    }

    /// Read-only preview of which `ThreadId` the next `run_one_step` call
    /// will resume (or `None` if nothing is runnable), with no mutation.
    /// Needed by `fn64-abi`'s coroutine-context plumbing: the per-thread
    /// `ACTIVE_YIELDER`/`ACTIVE_THREAD_ID`/`ACTIVE_RDRAM` thread-locals (see
    /// that crate's module doc) must be re-armed to the ABOUT-TO-BE-RESUMED
    /// thread's own saved values immediately before every single resume --
    /// not just once at thread creation -- since every `GameThread`
    /// coroutine shares that same native OS thread's thread-locals. This
    /// lets the caller look up "whose context do I need to install" BEFORE
    /// calling `run_one_step`, without duplicating this crate's scheduling
    /// policy (still `pick_next`'s exclusive job) outside `Executor`.
    pub fn peek_next_thread(&self) -> Option<ThreadId> {
        self.pick_next()
    }

    /// Priority of the thread the next [`run_one_step`](Self::run_one_step)
    /// will resume. Host drivers use this to recognize the libultra idle
    /// thread as quiescence instead of burning an arbitrary scheduling-step
    /// budget repeatedly resuming its voluntary-yield loop.
    pub fn peek_next_priority(&self) -> Option<Priority> {
        let id = self.pick_next()?;
        Some(
            self.threads
                .get(&id)
                .expect("run queue had stale id")
                .priority,
        )
    }

    /// Run exactly one scheduling step: pick the highest-priority runnable
    /// thread and resume it until it yields or finishes, handling the
    /// yield's semantics (pause_self / blocking send / blocking recv)
    /// before returning. This is the ONLY place `RunToken::issue()` is
    /// called and the ONLY place any `GameThread::resume` is called from --
    /// see `thread.rs`'s `RunToken` doc comment for why that makes two
    /// concurrent resumes a compile-time impossibility, not a runtime
    /// discipline.
    ///
    /// Returns `false` if nothing was runnable (the caller -- the host
    /// driver -- should call `advance_time` to make progress, e.g. firing
    /// the next timer or waiting for the next external event).
    pub fn run_one_step(&mut self) -> bool {
        let Some(id) = self.pick_next() else {
            return false;
        };
        self.run_queue.retain(|t| *t != id);

        let resume_with = self.pending_resume.remove(&id).unwrap_or(Resume::Continue);
        let from = self.running;
        self.running = Some(id);
        {
            let thread = self.threads.get_mut(&id).expect("run queue had stale id");
            thread.set_state(ThreadState::Running);
        }

        let sim_time = self.sim_time;
        let reason = match &resume_with {
            Resume::Start => SwitchReason::Scheduled,
            Resume::Continue => SwitchReason::Scheduled,
            Resume::Delivered(_) => SwitchReason::Woken,
            Resume::SendUnblocked => SwitchReason::Woken,
            Resume::WouldBlock => SwitchReason::Scheduled,
        };
        self.trace.record(
            sim_time,
            TraceKind::ThreadSwitch {
                from,
                to: id,
                reason,
            },
        );

        // `self.threads` is keyed to `Box<GameThread>` (not a bare
        // `GameThread`) specifically so this next line is sound -- see
        // `threads`' field doc comment for the second block/wake defect
        // this closes (the OoT Main-resume SIGBUS,
        // `fn64-diff`'s first-divergence report): `thread.resume(..)` runs
        // the coroutine body, which may synchronously call BACK into
        // `with_executor` (e.g. the thread's own body calling
        // `osCreateThread_recomp` to spawn another thread -- an ordinary,
        // supported nested call, no second `RunToken` involved -- see
        // `fn64-abi`'s `ReentrantCell` doc). That nested call can insert a
        // NEW entry into this SAME map (`create_thread`'s
        // `self.threads.insert`), and a `HashMap` insert that grows the
        // table reallocates its bucket array. If the map stored `GameThread`
        // BY VALUE, that reallocation would MOVE every existing value --
        // including the very `GameThread`/`Coroutine` whose `resume()` is
        // executing above us on the call stack right now, out from under
        // the `&mut GameThread` this line borrows -- a dangling reference
        // in use for the rest of the resume (real UB, and the actual
        // failure was delayed rather than immediate: the corrupted
        // `Coroutine`'s internal resume-target state only got read again,
        // and crashed with PC landing in unmapped memory, on that SAME
        // thread's NEXT resume, arbitrarily later -- exactly the OoT
        // Main-resume SIGBUS's shape). Boxing means the map's bucket array
        // reallocating only ever moves `Box<GameThread>` HANDLES (pointers)
        // -- the `GameThread` each one points to stays at a fixed heap
        // address for its entire life, so a `&mut GameThread` obtained by
        // dereferencing a `Box` before a reentrant insert remains valid
        // through and after that insert.
        self.resume_epoch = self
            .resume_epoch
            .checked_add(1)
            .expect("executor resume epoch overflow");
        let thread = self.threads.get_mut(&id).expect("run queue had stale id");
        let result = thread.resume(RunToken::issue(), resume_with);

        match result {
            CoroutineResult::Return(()) => {
                self.destroy_thread(id);
            }
            CoroutineResult::Yield(yielded) => {
                self.handle_yield(id, yielded);
            }
        }
        true
    }

    fn handle_yield(&mut self, id: ThreadId, yielded: Yield) {
        match yielded {
            Yield::PauseSelf => {
                // Rung 14: a voluntary yield with no blocking condition.
                // Immediately runnable again next round -- this is the
                // exact semantics that fixes an idle spin loop: it gives up
                // the CPU every iteration instead of never yielding.
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.set_state(ThreadState::Runnable);
                }
                self.run_queue.push(id);
                self.sort_run_queue();
                if self.running == Some(id) {
                    self.running = None;
                }
            }
            Yield::StopSelf => {
                // Generated-C pause_self has no guest loop-back continuation.
                // Parking here closes the interleaving where an assert-hang
                // resumed at the following host statement and corrupted state.
                self.remove_thread_from_waiters(id);
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.set_state(ThreadState::Stopped);
                }
                self.run_queue.retain(|thread| *thread != id);
                self.pending_resume.remove(&id);
                if self.running == Some(id) {
                    self.running = None;
                }
            }
            Yield::InstructionCheckpoint { instructions } => {
                assert!(
                    instructions > 0,
                    "translated instruction checkpoint must make guest progress"
                );
                let now = self
                    .sim_time
                    .checked_add(u64::from(instructions))
                    .expect("translated instruction checkpoint overflows virtual time");
                // The coroutine is suspended at this point. Advance core
                // timer/peripheral time before requeueing it; the ABI wrapper
                // commits its device fabric at this same timestamp after
                // `run_one_step` returns and before another resume.
                self.advance_time(now);
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.set_state(ThreadState::Runnable);
                }
                self.run_queue.push(id);
                self.sort_run_queue();
                if self.running == Some(id) {
                    self.running = None;
                }
            }
            Yield::BlockOnRecv { mq_addr, may_block } => {
                match self.try_deliver_recv(id, mq_addr) {
                    RecvOutcome::Delivered(msg) => {
                        // Immediately re-runnable with the result -- either
                        // a message was already there the instant we
                        // suspended, or (may_block: false) this IS the
                        // whole point: a non-blocking recv attempt that
                        // succeeded.
                        self.pending_resume.insert(id, Resume::Delivered(msg));
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::Runnable);
                        }
                        self.run_queue.push(id);
                        self.sort_run_queue();
                    }
                    RecvOutcome::Blocked if may_block => {
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::BlockedOnRecv);
                        }
                        self.record_queue_op(mq_addr, QueueOpKind::Block, id);
                        // libultra inserts waiters by descending priority;
                        // stable equal priorities preserve arrival order.
                        let priority = self.thread_pri(id);
                        self.queue_mut(mq_addr).block_receiver(id, priority);
                    }
                    RecvOutcome::Blocked => {
                        // OS_MESG_NOBLOCK on an empty queue: never parked,
                        // immediately re-runnable next round with the
                        // "nothing available" outcome -- this is the ONE
                        // path that made this yield unconditional in the
                        // first place (see fn64-abi's module doc): the ABI
                        // layer cannot check this itself without
                        // re-entering the executor from inside the
                        // coroutine body it's already running on.
                        self.pending_resume.insert(id, Resume::WouldBlock);
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::Runnable);
                        }
                        self.run_queue.push(id);
                        self.sort_run_queue();
                    }
                }
                if self.running == Some(id) {
                    self.running = None;
                }
            }
            Yield::BlockOnSend {
                mq_addr,
                msg,
                may_block,
                jam,
            } => {
                // Symmetric with BlockOnRecv above: check first, since the
                // queue may have gained space (or a receiver may already be
                // waiting) by the time the coroutine actually suspended --
                // only truly park it if delivery genuinely cannot happen
                // yet AND the caller allows blocking.
                let placement = if jam {
                    SendPlacement::Head
                } else {
                    SendPlacement::Tail
                };
                match self.try_deliver_send(id, mq_addr, msg, placement) {
                    SendOutcome::Delivered => {
                        self.pending_resume.insert(id, Resume::SendUnblocked);
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::Runnable);
                        }
                        self.run_queue.push(id);
                        self.sort_run_queue();
                    }
                    SendOutcome::Blocked if may_block => {
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::BlockedOnSend);
                        }
                        self.record_queue_op(mq_addr, QueueOpKind::Block, id);
                        // Capture priority at the park boundary, matching the
                        // ordering rule used for blocked receivers above.
                        let priority = self.thread_pri(id);
                        self.queue_mut(mq_addr)
                            .block_sender(id, priority, msg, placement);
                    }
                    SendOutcome::Blocked => {
                        // OS_MESG_NOBLOCK on a full queue: dropped, never
                        // parked, immediately re-runnable with WouldBlock.
                        self.pending_resume.insert(id, Resume::WouldBlock);
                        if let Some(thread) = self.threads.get_mut(&id) {
                            thread.set_state(ThreadState::Runnable);
                        }
                        self.run_queue.push(id);
                        self.sort_run_queue();
                    }
                }
                if self.running == Some(id) {
                    self.running = None;
                }
            }
        }
    }

    /// Guest-facing `osSendMesg(mq, msg, flag)` entry point, called by the
    /// `fn64-abi` shim for the currently-running thread. `blocking`
    /// corresponds to `flag == OS_MESG_BLOCK`. Returns whether it was
    /// delivered immediately, would need to block (caller must arrange to
    /// yield with `Yield::BlockOnSend` -- see `fn64-abi`'s shim, which owns
    /// the actual coroutine suspend call since only the coroutine body
    /// itself can call `Yielder::suspend`), or was dropped
    /// (`OS_MESG_NOBLOCK` on a full queue).
    pub fn send_mesg(
        &mut self,
        sender: ThreadId,
        mq_addr: RdramAddr,
        msg: Mesg,
        blocking: bool,
    ) -> SendMesgOutcome {
        match self.try_deliver_send(sender, mq_addr, msg, SendPlacement::Tail) {
            SendOutcome::Delivered => SendMesgOutcome::Delivered,
            SendOutcome::Blocked if blocking => SendMesgOutcome::MustYield,
            SendOutcome::Blocked => SendMesgOutcome::DroppedWouldBlock,
        }
    }

    /// Guest-facing `osRecvMesg(mq, msg, flag)` entry point.
    pub fn recv_mesg(
        &mut self,
        receiver: ThreadId,
        mq_addr: RdramAddr,
        blocking: bool,
    ) -> RecvMesgOutcome {
        match self.try_deliver_recv(receiver, mq_addr) {
            RecvOutcome::Delivered(msg) => RecvMesgOutcome::Delivered(msg),
            RecvOutcome::Blocked if blocking => RecvMesgOutcome::MustYield,
            RecvOutcome::Blocked => RecvMesgOutcome::WouldBlock,
        }
    }

    /// `osGetThreadId`/introspection convenience for tests and the ABI
    /// layer: which thread, if any, is presently the one holding the
    /// (conceptual) `RunToken`.
    pub fn current_thread(&self) -> Option<ThreadId> {
        self.running
    }

    pub fn is_thread_dead(&self, id: ThreadId) -> bool {
        self.threads.get(&id).map(|t| t.is_dead()).unwrap_or(true)
    }

    /// Whether `id` is occupied by any thread, alive or dead. The ABI layer
    /// uses this to detect guest OSId collisions before `create_thread`'s
    /// loud duplicate-id trap fires: libultra's OSId is an informational tag
    /// with no uniqueness contract (thread identity on hardware is the
    /// OSThread struct), so a retail boot may legitimately reuse a number.
    pub fn thread_exists(&self, id: ThreadId) -> bool {
        self.threads.contains_key(&id)
    }

    pub fn queue_capacity(&self, mq_addr: RdramAddr) -> usize {
        self.queues
            .get(&mq_addr.offset())
            .map(|q| q.capacity())
            .unwrap_or(0)
    }

    pub fn queue_valid_count(&self, mq_addr: RdramAddr) -> Option<usize> {
        self.queues
            .get(&mq_addr.offset())
            .map(MesgQueue::valid_count)
    }

    /// Complete queue-use preflight for synchronous APIs whose public contract
    /// requires an unshared queue. In particular, an empty queue with an older
    /// blocked receiver is not private: that receiver would steal the next
    /// completion post and strand the synchronous caller.
    pub fn queue_activity(&self, mq_addr: RdramAddr) -> Option<MesgQueueActivity> {
        self.queues.get(&mq_addr.offset()).map(MesgQueue::activity)
    }

    /// Run until the run queue is empty (every thread finished, blocked, or
    /// none were ever runnable). Test/harness convenience -- a real host
    /// driver instead interleaves `run_one_step` with its own frame pacing
    /// and calls `advance_time`/`inject_event` between steps.
    pub fn run_to_idle(&mut self) {
        while self.run_one_step() {}
    }
}

/// Internal outcome of attempting to deliver a send without blocking.
enum SendOutcome {
    Delivered,
    Blocked,
}

/// Internal outcome of attempting to deliver a recv without blocking.
enum RecvOutcome {
    Delivered(Mesg),
    Blocked,
}

/// `osSendMesg`'s observable outcome from the guest/ABI caller's point of
/// view -- see `docs/DESIGN.md` section 2's "Send/recv as coroutine yield
/// points, not thread ops": `MustYield` is the signal the `fn64-abi` shim
/// uses to actually call `Yielder::suspend(Yield::BlockOnSend(..))` on the
/// coroutine's own stack (only the coroutine body can do that -- the
/// executor cannot suspend a coroutine it isn't currently resuming).
#[derive(Debug, PartialEq, Eq)]
pub enum SendMesgOutcome {
    Delivered,
    /// `OS_MESG_BLOCK` on a full queue with no waiting receiver: caller
    /// must yield with `Yield::BlockOnSend`.
    MustYield,
    /// `OS_MESG_NOBLOCK` on a full queue: message dropped, no yield.
    DroppedWouldBlock,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecvMesgOutcome {
    Delivered(Mesg),
    /// `OS_MESG_BLOCK` on an empty queue: caller must yield with
    /// `Yield::BlockOnRecv`.
    MustYield,
    /// `OS_MESG_NOBLOCK` on an empty queue: no message, no yield.
    WouldBlock,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_i32(buf: &[u8], off: usize) -> i32 {
        i32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    #[test]
    fn peek_next_priority_distinguishes_work_from_the_idle_thread() {
        let mut exec = Executor::new();
        exec.create_thread(1, crate::thread::OS_PRIORITY_IDLE, |_yielder, _resume| {});
        exec.create_thread(2, 10, |_yielder, _resume| {});
        exec.start_thread(1);
        exec.start_thread(2);

        assert_eq!(exec.peek_next_priority(), Some(10));
        assert!(exec.run_one_step());
        assert_eq!(
            exec.peek_next_priority(),
            Some(crate::thread::OS_PRIORITY_IDLE)
        );
    }

    /// Regression: the guest's rdram `OSMesgQueue` struct
    /// (`validCount`@0x08, `first`@0x0C, `msgCount`@0x10) MUST be kept in
    /// sync with the executor's authoritative `MesgQueue` after creation and
    /// every mutation. Guest code reads those fields DIRECTLY via
    /// `MQ_GET_COUNT`/`MQ_IS_FULL` (e.g. `IrqMgr_SendMesgToClients`'s
    /// `MQ_IS_FULL(client->queue)` gate). Before this fix, the struct stayed
    /// zero-initialized, so `MQ_IS_FULL` = `0 >= 0` = ALWAYS TRUE, silently
    /// dropping every VI-retrace forward to the OoT scheduler and freezing
    /// boot at exactly 1 framebuffer swap.
    ///
    /// Distinguishable values chosen so a regression can't pass by accident:
    /// capacity 5 (not 0/1), and a partial fill of 3 (not 0, not full).
    #[test]
    fn queue_struct_mirrored_into_rdram_on_create_and_send() {
        // A queue at a byte offset with room for the 0x18-byte struct.
        const Q_OFF: u32 = 0x1000;
        const CAPACITY: usize = 5;
        let mut rdram = vec![0u8; 0x2000];

        let mut exec = Executor::new();
        unsafe { exec.set_rdram_base(rdram.as_mut_ptr()) };

        let q = RdramAddr::from_offset(Q_OFF);
        exec.create_mesg_queue(q, CAPACITY);

        // After creation: validCount==0, first==0, msgCount==capacity.
        let base = Q_OFF as usize;
        assert_eq!(read_i32(&rdram, base + 0x08), 0, "validCount after create");
        assert_eq!(read_i32(&rdram, base + 0x0C), 0, "first after create");
        assert_eq!(
            read_i32(&rdram, base + 0x10),
            CAPACITY as i32,
            "msgCount after create MUST equal capacity, else MQ_IS_FULL reads garbage"
        );

        // Post three messages via the external/timer path (no blocked
        // receiver), then confirm validCount tracked each enqueue -- 3, a
        // value distinct from 0 (empty) and 5 (full).
        for msg in [0x11u32, 0x22, 0x33] {
            exec.inject_event(ExternalEvent::DirectPost { queue_addr: q, msg });
        }
        assert_eq!(
            read_i32(&rdram, base + 0x08),
            3,
            "validCount MUST mirror the 3 enqueued messages (so MQ_GET_COUNT/MQ_IS_FULL are correct)"
        );
        assert_eq!(
            read_i32(&rdram, base + 0x10),
            CAPACITY as i32,
            "msgCount stays capacity"
        );
        // MQ_IS_FULL semantics the guest computes: validCount >= msgCount.
        assert!(
            read_i32(&rdram, base + 0x08) < read_i32(&rdram, base + 0x10),
            "a partially-filled queue MUST NOT read as full (the bug: it always did)"
        );
    }

    /// Regression: a queue the guest NEVER passed to `osCreateMesgQueue` (a
    /// bzero'd `OSMesgQueue` struct used directly) must behave as a real
    /// zero-capacity queue: NOBLOCK send finds it full (dropped), NOBLOCK recv
    /// finds it empty (would-block) -- BOTH returning the -1 the guest relies
    /// on. OoT's audio driver depends on exactly this for
    /// `gAudioCtx.asyncLoadUnkMediumQueue`, which the decomp never creates and
    /// only ever NOBLOCK-sends/recvs (`audio/internal/load.c:1652,1717-1718`).
    ///
    /// Fail-against-bug: before the `queue_mut` lazy-zero-capacity fix, the
    /// FIRST touch of such a queue PANICKED ("used before osCreateMesgQueue"),
    /// aborting the whole boot at ~VI swap 2 the moment the (newly un-stubbed)
    /// audio load path ran. This test would have panicked instead of asserting.
    #[test]
    fn untracked_queue_behaves_as_zero_capacity_not_a_panic() {
        let mut exec = Executor::new();
        // A queue address that was NEVER created via osCreateMesgQueue.
        let q = RdramAddr::from_offset(0x4321);

        // NOBLOCK send: a zero-capacity queue is always full -> dropped, not a
        // panic, not a fake "delivered".
        assert_eq!(
            exec.send_mesg(0, q, 0xDEAD, /* blocking */ false),
            SendMesgOutcome::DroppedWouldBlock,
            "NOBLOCK send to an untracked (bzero'd) queue must report full/dropped (guest -1)"
        );

        // NOBLOCK recv: a zero-capacity queue is always empty -> would-block.
        assert_eq!(
            exec.recv_mesg(0, q, /* blocking */ false),
            RecvMesgOutcome::WouldBlock,
            "NOBLOCK recv from an untracked (bzero'd) queue must report empty (guest -1)"
        );

        // The lazy install must be genuinely zero-capacity, so it can never
        // silently accept a message a real bzero'd queue would have rejected.
        assert_eq!(
            exec.queue_capacity(q),
            0,
            "untracked queue must be zero-capacity"
        );
    }

    /// Without a registered rdram base (unit-test executors that never boot a
    /// real rdram), the mirror is a safe no-op -- never a null deref.
    #[test]
    fn mirror_is_noop_without_rdram_base() {
        let mut exec = Executor::new();
        let q = RdramAddr::from_offset(0x2000);
        exec.create_mesg_queue(q, 4); // must not panic / deref null
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: q,
            msg: 1,
        });
        assert_eq!(exec.queue_capacity(q), 4);
    }

    #[test]
    fn cp0_count_runs_at_half_cpu_rate_and_compare_latches_ip7() {
        let mut exec = Executor::new();
        exec.set_cp0_count(0xFFFF_FFFE);
        exec.write_cp0_compare(0);

        exec.advance_time(1);
        assert_eq!(exec.cp0_count(), 0xFFFF_FFFE);
        assert!(!exec.cp0_timer_pending());
        exec.advance_time(2);
        assert_eq!(exec.cp0_count(), 0xFFFF_FFFF);
        assert!(!exec.cp0_timer_pending());
        exec.advance_time(4);
        assert_eq!(exec.cp0_count(), 0);
        assert!(exec.cp0_timer_pending());

        exec.write_cp0_compare(0x1234_5678);
        assert_eq!(exec.cp0_compare(), 0x1234_5678);
        assert!(!exec.cp0_timer_pending());
    }

    #[test]
    fn boot_clock_restore_retains_captured_compare_latch() {
        let mut exec = Executor::new();
        exec.restore_cp0_clock(0x1234_5678, 0x9abc_def0, true);
        assert_eq!(exec.cp0_count(), 0x1234_5678);
        assert_eq!(exec.cp0_compare(), 0x9abc_def0);
        assert!(exec.cp0_timer_pending());
    }

    #[test]
    fn split_cp0_count_advances_preserve_the_odd_cycle_phase() {
        let mut split = Executor::new();
        split.advance_time(1);
        split.advance_time(3);
        split.advance_time(7);

        let mut combined = Executor::new();
        combined.advance_time(7);
        assert_eq!(split.cp0_count(), combined.cp0_count());
        assert_eq!(split.cp0_count(), 3);
    }

    #[test]
    fn executor_delivers_start_once_then_continue_after_pause() {
        let inputs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = inputs.clone();
        let mut exec = Executor::new();
        exec.create_thread(1, 1, move |yielder, first| {
            observed.borrow_mut().push(first);
            let resumed = yielder.suspend(Yield::PauseSelf);
            observed.borrow_mut().push(resumed);
        });
        exec.start_thread(1);

        assert!(exec.run_one_step());
        assert_eq!(&*inputs.borrow(), &[Resume::Start]);
        assert!(exec.run_one_step());
        assert_eq!(&*inputs.borrow(), &[Resume::Start, Resume::Continue]);
        assert!(exec.is_thread_dead(1));
    }

    /// Exact interleaving regression: A blocks receiving, B destroys A, then
    /// a host event posts to the queue. The post must enqueue normally; it
    /// must not pop A's stale waiter id and revive the destroyed coroutine.
    #[test]
    fn destroyed_blocked_receiver_cannot_be_revived_by_a_later_post() {
        let mut exec = Executor::new();
        let queue = RdramAddr::from_offset(0x3000);
        exec.create_mesg_queue(queue, 1);
        exec.create_thread(1, 1, move |yielder, _| {
            let _ = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: queue,
                may_block: true,
            });
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());

        exec.destroy_thread(1);
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0xABCD,
        });

        assert!(exec.is_thread_dead(1));
        assert_eq!(exec.peek_next_thread(), None);
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::Delivered(0xABCD)
        );
    }

    /// Exact interleaving regression: A blocks jamming into a full queue, B
    /// stops A, then B receives and frees a slot. The receive must not replay
    /// A's removed blocked operation or make A runnable again.
    #[test]
    fn stopped_blocked_sender_cannot_be_revived_when_space_frees() {
        let mut exec = Executor::new();
        let queue = RdramAddr::from_offset(0x4000);
        exec.create_mesg_queue(queue, 1);
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0x1111,
        });
        exec.create_thread(1, 1, move |yielder, _| {
            let _ = yielder.suspend(Yield::BlockOnSend {
                mq_addr: queue,
                msg: 0x2222,
                may_block: true,
                jam: true,
            });
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());

        exec.stop_thread(1);
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::Delivered(0x1111)
        );
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::WouldBlock
        );
        assert_eq!(exec.peek_next_thread(), None);
        assert!(!exec.is_thread_dead(1));
    }

    #[test]
    fn control_evidence_canonicalizes_hash_owned_insertion_order() {
        fn build(reversed: bool) -> Executor {
            let mut exec = Executor::new();
            let thread_ids = if reversed { [9, 3] } else { [3, 9] };
            for id in thread_ids {
                exec.create_thread(id, id as Priority, |_yielder, _resume| {});
            }

            let queue_offsets = if reversed {
                [0x2200, 0x1100]
            } else {
                [0x1100, 0x2200]
            };
            for offset in queue_offsets {
                let queue = RdramAddr::from_offset(offset);
                exec.create_mesg_queue(queue, 3);
                exec.inject_event(ExternalEvent::DirectPost {
                    queue_addr: queue,
                    msg: offset,
                });
            }

            let events = if reversed {
                [(8, 0x2200, 0x88), (2, 0x1100, 0x22)]
            } else {
                [(2, 0x1100, 0x22), (8, 0x2200, 0x88)]
            };
            for (event, queue, msg) in events {
                exec.set_event_mesg(event, RdramAddr::from_offset(queue), msg);
            }
            exec
        }

        let snapshot = build(false).control_evidence_snapshot();
        assert_eq!(snapshot, build(true).control_evidence_snapshot());
        assert_eq!(
            snapshot
                .threads
                .iter()
                .map(|thread| thread.id)
                .collect::<Vec<_>>(),
            vec![3, 9]
        );
        assert_eq!(
            snapshot
                .queues
                .iter()
                .map(|queue| queue.address.offset())
                .collect::<Vec<_>>(),
            vec![0x1100, 0x2200]
        );
        assert_eq!(
            snapshot
                .event_table
                .iter()
                .map(|registration| registration.event)
                .collect::<Vec<_>>(),
            vec![2, 8]
        );
    }

    #[test]
    fn control_evidence_preserves_exact_equal_priority_run_order() {
        fn build(order: [ThreadId; 2]) -> ExecutorControlEvidenceSnapshot {
            let mut exec = Executor::new();
            exec.create_thread(1, 10, |_yielder, _resume| {});
            exec.create_thread(2, 10, |_yielder, _resume| {});
            exec.start_thread(order[0]);
            exec.start_thread(order[1]);
            exec.control_evidence_snapshot()
        }

        let first = build([1, 2]);
        let reversed = build([2, 1]);
        assert_eq!(first.threads, reversed.threads);
        assert_eq!(first.pending_resumes, reversed.pending_resumes);
        assert_eq!(
            first
                .pending_resumes
                .iter()
                .map(|pending| pending.thread)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_ne!(first.run_queue, reversed.run_queue);
    }

    #[test]
    fn control_evidence_is_pointer_independent_and_nonmutating() {
        let mut first_rdram = vec![0u8; 0x4000];
        let mut second_rdram = vec![0u8; 0x4000];
        assert_ne!(first_rdram.as_mut_ptr(), second_rdram.as_mut_ptr());

        let mut first = Executor::new();
        let mut second = Executor::new();
        unsafe {
            first.set_rdram_base_with_len(first_rdram.as_mut_ptr(), first_rdram.len());
            second.set_rdram_base_with_len(second_rdram.as_mut_ptr(), second_rdram.len());
        }
        first.create_thread(4, 7, |_yielder, _resume| {});
        second.create_thread(4, 7, |_yielder, _resume| {});

        let snapshot = first.control_evidence_snapshot();
        assert_eq!(snapshot, second.control_evidence_snapshot());
        assert_eq!(first.control_evidence_snapshot(), snapshot);
        assert_eq!(first.peek_next_thread(), None);
    }

    #[test]
    fn control_evidence_detects_each_owner_family_perturbation() {
        let baseline = Executor::new().control_evidence_snapshot();

        let mut rdram_bytes = vec![0u8; 32];
        let mut rdram = Executor::new();
        unsafe { rdram.set_rdram_base_with_len(rdram_bytes.as_mut_ptr(), rdram_bytes.len()) };
        assert_ne!(baseline.rdram, rdram.control_evidence_snapshot().rdram);

        let mut thread = Executor::new();
        thread.create_thread(1, 3, |_yielder, _resume| {});
        assert_ne!(baseline.threads, thread.control_evidence_snapshot().threads);

        let mut queue = Executor::new();
        queue.create_mesg_queue(RdramAddr::from_offset(0x100), 2);
        assert_ne!(baseline.queues, queue.control_evidence_snapshot().queues);

        let mut timer = Executor::new();
        timer.set_timer(7, 0, RdramAddr::from_offset(0x100), 5, 1);
        assert_ne!(baseline.timers, timer.control_evidence_snapshot().timers);

        let mut event = Executor::new();
        event.set_event_mesg(7, RdramAddr::from_offset(0x100), 5);
        assert_ne!(
            baseline.event_table,
            event.control_evidence_snapshot().event_table
        );

        let mut clock = Executor::new();
        clock.write_cp0_compare(1);
        clock.advance_time(3);
        let clock = clock.control_evidence_snapshot();
        assert_ne!(baseline.sim_time, clock.sim_time);
        assert_ne!(baseline.cp0_count, clock.cp0_count);
        assert_ne!(baseline.cp0_count_phase, clock.cp0_count_phase);
        assert_ne!(baseline.cp0_compare, clock.cp0_compare);
        assert_ne!(baseline.cp0_timer_pending, clock.cp0_timer_pending);

        let mut active = Executor::new();
        active.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        active.start_thread(1);
        assert!(active.run_one_step());
        active.run_queue.clear();
        active
            .threads
            .get_mut(&1)
            .expect("thread exists")
            .set_state(ThreadState::Running);
        active.running = Some(1);
        assert_eq!(
            active.control_evidence_snapshot().running,
            ExecutorRunningEvidenceSnapshot::Active(1)
        );

        let mut pending = Executor::new();
        pending.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        pending.start_thread(1);
        assert!(pending.run_one_step());
        let without_pending = pending.control_evidence_snapshot();
        pending.pending_resume.insert(1, Resume::WouldBlock);
        let with_pending = pending.control_evidence_snapshot();
        assert_eq!(without_pending.threads, with_pending.threads);
        assert_eq!(without_pending.run_queue, with_pending.run_queue);
        assert_ne!(
            without_pending.pending_resumes,
            with_pending.pending_resumes
        );
    }

    #[test]
    fn control_evidence_rejects_corrupt_cross_owner_relationships() {
        let mut duplicate = Executor::new();
        duplicate.create_thread(1, 1, |_yielder, _resume| {});
        duplicate.start_thread(1);
        duplicate.run_queue.push(1);
        assert_eq!(
            duplicate.validate_control_evidence_invariants(),
            Err(ExecutorControlInvariantError::DuplicateRunQueueThread(1))
        );

        let mut wrong_waiter_state = Executor::new();
        let queue = RdramAddr::from_offset(0x100);
        wrong_waiter_state.create_mesg_queue(queue, 1);
        wrong_waiter_state.create_thread(2, 1, |_yielder, _resume| {});
        wrong_waiter_state.queue_mut(queue).block_receiver(2, 1);
        assert_eq!(
            wrong_waiter_state.validate_control_evidence_invariants(),
            Err(ExecutorControlInvariantError::ReceiverWaiterStateMismatch(
                2,
                ThreadState::Stopped
            ))
        );

        let mut stale_resume = Executor::new();
        stale_resume.create_thread(3, 1, |_yielder, _resume| {});
        stale_resume.pending_resume.insert(3, Resume::Continue);
        assert_eq!(
            stale_resume.validate_control_evidence_invariants(),
            Err(
                ExecutorControlInvariantError::PendingResumeThreadNotRunnable(
                    3,
                    ThreadState::Stopped
                )
            )
        );
    }

    #[test]
    fn stopping_a_woken_thread_discards_its_stale_pending_resume() {
        let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed_by_thread = observed.clone();
        let queue = RdramAddr::from_offset(0x100);
        let mut exec = Executor::new();
        exec.create_mesg_queue(queue, 1);
        exec.create_thread(1, 1, move |yielder, first| {
            observed_by_thread.borrow_mut().push(first);
            let resumed = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: queue,
                may_block: true,
            });
            observed_by_thread.borrow_mut().push(resumed);
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0xCAFE,
        });

        exec.stop_thread(1);
        exec.start_thread(1);
        assert!(exec.run_one_step());
        assert_eq!(&*observed.borrow(), &[Resume::Start, Resume::Continue]);
    }

    #[test]
    fn opaque_native_continuations_are_intentionally_evidence_equal() {
        let mut yields_again = Executor::new();
        yields_again.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
            yielder.suspend(Yield::PauseSelf);
        });
        yields_again.start_thread(1);
        assert!(yields_again.run_one_step());

        let mut returns_next = Executor::new();
        returns_next.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        returns_next.start_thread(1);
        assert!(returns_next.run_one_step());

        assert_eq!(
            yields_again.control_evidence_snapshot(),
            returns_next.control_evidence_snapshot(),
            "native continuation differences are outside this evidence projection"
        );

        assert!(yields_again.run_one_step());
        assert!(returns_next.run_one_step());
        assert_ne!(
            yields_again.control_evidence_snapshot(),
            returns_next.control_evidence_snapshot(),
            "the next scheduling step exposes the intentionally opaque difference"
        );
    }

    #[test]
    fn process_exit_rejects_an_active_run_token_owner() {
        let mut exec = Executor::new();
        exec.running = Some(77);
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exec.prepare_process_exit()));
        assert!(panic.is_err());
        exec.running = None;
    }
}
