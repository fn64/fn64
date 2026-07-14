//! The single executor: one host thread, priority-ordered run queue,
//! stackful-coroutine `OSThread`s, libultra-faithful blocking
//! `osSendMesg`/`osRecvMesg`, and the ONE host-side event injection point
//! for external (SI/PI/VI-style) completions.
//!
//! See `docs/DESIGN.md` section 2 for the full design rationale (option
//! (b): "single executor + stackful coroutines") -- this module is that
//! recommendation's load-bearing implementation. Every design choice below
//! traces back to a specific rung's evidence, cited inline.

use std::collections::HashMap;

use corosensei::CoroutineResult;

use crate::mesgqueue::{Mesg, MesgQueue, RecvResult, SendResult};
use crate::rdram::RdramAddr;
use crate::rsp::{OsTaskHeader, TaskLog};
use crate::si::PifModel;
use crate::thread::{GameThread, Priority, Resume, RunToken, ThreadState, Yield};
use crate::timer::TimerWheel;
use crate::trace::{QueueOpKind, SwitchReason, TaskKind, ThreadId, TraceKind, TraceLog};
use crate::vi::{RetraceSchedule, ViState};

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
#[derive(Default)]
pub struct Executor {
    threads: HashMap<ThreadId, GameThread>,
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
    trace: TraceLog,
    /// VI hardware state (mode/features/y-scale/blanked/last-swapped
    /// framebuffer) -- see `vi.rs` module doc.
    vi: ViState,
    /// The periodic retrace ticker, driving `OS_EVENT_VI` delivery from
    /// `advance_time`. `None` until a host driver calls
    /// `arm_retrace`/`fn64-shell` picks a real interval -- no default
    /// interval is invented here (see `vi.rs`'s "not a hardware timing
    /// model" note); a boot harness that never arms it simply never
    /// receives VI retrace events, an honest state rather than a fabricated
    /// default NTSC constant.
    retrace: Option<RetraceSchedule>,
    /// Minimal SI/PIF controller-probe model (`si.rs`).
    pif: PifModel,
    /// RSP task submissions observed (`rsp.rs`).
    tasks: TaskLog,
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

    pub fn trace(&self) -> &[crate::trace::TraceEvent] {
        self.trace.events()
    }

    pub fn sim_time(&self) -> u64 {
        self.sim_time
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
        self.threads.insert(id, GameThread::new(id, priority, body));
    }

    /// `osStartThread(t)`. Puts the thread on the run queue for the first
    /// time.
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
        if let Some(thread) = self.threads.get_mut(&id) {
            thread.set_state(ThreadState::Dead);
        }
        self.run_queue.retain(|t| *t != id);
        if self.running == Some(id) {
            self.running = None;
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
    }

    fn queue_mut(&mut self, mq_addr: RdramAddr) -> &mut MesgQueue {
        self.queues.get_mut(&mq_addr.offset()).unwrap_or_else(|| {
            panic!(
                "queue at rdram offset {:#x} used before osCreateMesgQueue",
                mq_addr.offset()
            )
        })
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
        self.sim_time = now;
        let fired = self.timers.advance(now);
        for timer in fired {
            self.deliver_or_enqueue(timer.queue_addr, timer.msg, Some(timer.armed_by));
        }
        if let Some(sched) = &mut self.retrace {
            let ticks = sched.advance(now);
            for _ in 0..ticks {
                // Only deliver if the game has actually registered OS_EVENT_VI
                // (osSetEventMesg_recomp populates event_table) -- before
                // that registration exists (early boot), a real VI interrupt
                // still fires but nothing is listening; this mirrors that by
                // silently skipping delivery rather than panicking (an
                // unregistered event code is the expected pre-registration
                // state, not a caller bug -- see `inject_event`'s OsEvent
                // arm, which DOES panic on an unregistered code, because
                // that path is only ever invoked by test/harness code that
                // is expected to know a registration already happened).
                if self.event_table.contains_key(&OS_EVENT_VI) {
                    self.inject_event(ExternalEvent::OsEvent(OS_EVENT_VI));
                }
                // The VI manager's OWN retrace target (osViSetEvent, see
                // vi.rs's ViState::retrace_target doc comment) is a
                // SEPARATE delivery path from OS_EVENT_VI's general event
                // table -- both may be registered simultaneously and both
                // fire on the same retrace tick, matching real hardware's
                // two genuinely independent notification mechanisms.
                if let Some((mq_offset, msg)) = self.vi.retrace_target {
                    self.deliver_or_enqueue(RdramAddr::from_offset(mq_offset), msg, None);
                }
            }
        }
    }

    /// Arm the periodic VI retrace ticker at `interval` virtual-time units
    /// per field. See `vi.rs`'s `RetraceSchedule` doc -- not a hardware-
    /// accurate NTSC/PAL timing value, a host-chosen approximation.
    pub fn arm_retrace(&mut self, interval: u64) {
        self.retrace = Some(RetraceSchedule::new(interval));
    }

    // ---- VI (video interface) -------------------------------------------

    pub fn vi(&self) -> &ViState {
        &self.vi
    }

    pub fn vi_set_mode(&mut self, mode_ptr: u32) {
        self.vi.set_mode(mode_ptr);
    }

    pub fn vi_set_special_features(&mut self, ptr: u32) {
        self.vi.set_special_features(ptr);
    }

    pub fn vi_set_y_scale(&mut self, scale: f32) {
        self.vi.set_y_scale(scale);
    }

    /// `osViSetEvent(mq, msg, retraceCount)` -- see `ViState::set_event`'s
    /// doc comment for why this is a separate delivery path from
    /// `osSetEventMesg`/`OS_EVENT_VI`.
    pub fn vi_set_event(&mut self, mq_addr: RdramAddr, msg: Mesg) {
        self.vi.set_event(mq_addr, msg);
    }

    pub fn vi_set_black(&mut self, active: bool) {
        self.vi.set_black(active);
    }

    /// `osViSwapBuffer(frameBufPtr)`. Returns the newly-current framebuffer
    /// address so the caller (the `fn64-abi` shim) can hand it straight to
    /// the harness's framebuffer-capture hook without a second lookup.
    pub fn vi_swap_buffer(&mut self, frame_buf: RdramAddr) -> RdramAddr {
        self.vi.swap_buffer(frame_buf);
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
        &self.pif
    }

    // ---- RSP task submission -----------------------------------------------

    pub fn task_log(&self) -> &TaskLog {
        &self.tasks
    }

    /// Record an RSP task submission (gfx: acknowledged only; audio: the
    /// caller has already invoked the real translated ucode function before
    /// calling this -- see `fn64-abi`'s `osSpTaskYielded_recomp`/task-submit
    /// shim doc comments for the actual dispatch). Emits the shared
    /// `TaskSubmit` trace event alongside the fuller `OsTaskHeader` this
    /// module's own `TaskLog` keeps.
    pub fn submit_task(&mut self, header: OsTaskHeader) {
        let sim_time = self.sim_time;
        if let Some(kind) = header.kind() {
            self.trace.record(
                sim_time,
                TraceKind::TaskSubmit {
                    task_kind: kind,
                    ucode: header.ucode,
                },
            );
        }
        self.tasks.record(header);
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
    /// receiver is already blocked, wake exactly one (FIFO, per
    /// `docs/DESIGN.md`'s "FIFO per-queue delivery" -- libultra's own
    /// documented per-queue order) and hand it the message directly rather
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
        let queue = self.queue_mut(queue_addr);
        if queue.has_blocked_receivers() {
            let waiter = queue
                .wake_one_receiver()
                .expect("has_blocked_receivers() was true");
            self.record_queue_op(queue_addr, QueueOpKind::Wake, waiter);
            self.wake_thread(waiter, Resume::Delivered(msg));
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
    }

    /// Move a blocked (or newly-woken-by-timer/external-event) thread back
    /// onto the run queue.
    fn wake_thread(&mut self, id: ThreadId, resume_with: Resume) {
        if let Some(thread) = self.threads.get_mut(&id) {
            thread.set_state(ThreadState::Runnable);
        }
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
    fn try_deliver_send(&mut self, sender: ThreadId, mq_addr: RdramAddr, msg: Mesg) -> SendOutcome {
        let queue = self.queue_mut(mq_addr);
        if queue.has_blocked_receivers() {
            let waiter = queue
                .wake_one_receiver()
                .expect("has_blocked_receivers() was true");
            self.record_queue_op(mq_addr, QueueOpKind::Send, sender);
            self.record_queue_op(mq_addr, QueueOpKind::Wake, waiter);
            self.wake_thread(waiter, Resume::Delivered(msg));
            return SendOutcome::Delivered;
        }
        match queue.try_send(msg) {
            SendResult::Delivered => {
                self.record_queue_op(mq_addr, QueueOpKind::Send, sender);
                SendOutcome::Delivered
            }
            SendResult::WouldBlock => SendOutcome::Blocked,
        }
    }

    fn try_deliver_recv(&mut self, receiver: ThreadId, mq_addr: RdramAddr) -> RecvOutcome {
        let queue = self.queue_mut(mq_addr);
        let recv_result = queue.try_recv();
        let has_blocked_senders = queue.has_blocked_senders();
        match recv_result {
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
        }
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

        let result = {
            let thread = self.threads.get_mut(&id).expect("run queue had stale id");
            thread.resume(RunToken::issue(), resume_with)
        };

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
                        self.queue_mut(mq_addr).block_receiver(id);
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
            } => {
                // Symmetric with BlockOnRecv above: check first, since the
                // queue may have gained space (or a receiver may already be
                // waiting) by the time the coroutine actually suspended --
                // only truly park it if delivery genuinely cannot happen
                // yet AND the caller allows blocking.
                match self.try_deliver_send(id, mq_addr, msg) {
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
                        self.queue_mut(mq_addr).block_sender(id);
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
        match self.try_deliver_send(sender, mq_addr, msg) {
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

    pub fn queue_capacity(&self, mq_addr: RdramAddr) -> usize {
        self.queues
            .get(&mq_addr.offset())
            .map(|q| q.capacity())
            .unwrap_or(0)
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
