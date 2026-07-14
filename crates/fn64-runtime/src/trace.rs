//! The shared differential-testing event trace. See `docs/DESIGN.md`
//! section 4 ("Shared event-trace format") and section 3's "First-class
//! watch/diagnostic hooks": this is the SAME global monotonic sequence
//! counter section 3 designs for `Rdram::write_*` attribution, reused here
//! for executor-level events (thread switch, queue op, timer fire) so both
//! diagnostic stories share one counter and one mental model, per
//! `AGENTS.md`'s "mechanism over patch" rule -- one counter, two consumers,
//! not two competing sequence numbers that could disagree.
//!
//! `TraceEvent`/`TraceKind` are the exact shapes `docs/DESIGN.md` section 4
//! specifies for the reference-runtime A/B comparator; nothing here is
//! fn64-internal-only vocabulary. A future `fn64-shell --trace-compare`
//! consumes this stream verbatim.

use std::sync::atomic::{AtomicU64, Ordering};

/// The one global sequence counter, per `docs/DESIGN.md` section 3: "there
/// is exactly one write path per §3.1, so there is exactly one place to
/// increment." Reused here for every executor-visible event (not just rdram
/// writes) so a single sequence number totally orders "everything this
/// process did," which is what makes the section-4 comparator's "first
/// divergence: sequence number" report meaningful across two different
/// runtimes' otherwise-uncorrelated internal clocks.
static GLOBAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Allocate the next sequence number. Every `Rdram::write_*` and every
/// executor-level event (thread switch, queue op, timer fire) calls this
/// exactly once per event -- see module doc for why one counter serves both.
pub fn next_sequence() -> u64 {
    GLOBAL_SEQUENCE.fetch_add(1, Ordering::SeqCst)
}

/// Reset the counter. Test-only: production code never rewinds sequence
/// numbers (the log's whole value is that they are monotonic and comparable
/// run-over-run), but each `#[test]` wants its own trace to start at a known
/// point rather than accumulating state across the whole test binary.
#[cfg(test)]
pub fn reset_sequence_for_test() {
    GLOBAL_SEQUENCE.store(0, Ordering::SeqCst);
}

pub type ThreadId = u32;

/// Why a `ThreadSwitch` happened. Named per `docs/DESIGN.md` section 2's
/// yield-site inventory (pause_self, blocking osRecvMesg/osSendMesg, a
/// host-driven wake) so a trace reader can tell "this thread gave up the
/// CPU voluntarily" from "an external event made a higher-priority thread
/// runnable," which is exactly the distinction rung 18's postmortem needed
/// and the reference runtime's trace could not give cleanly.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SwitchReason {
    /// `pause_self` / a voluntary cooperative yield (rung 14's idle-loop
    /// fix: this is what a spin-loop MUST call instead of never yielding).
    PauseSelf,
    /// Blocked in `osRecvMesg` on an empty queue.
    BlockedOnRecv,
    /// Blocked in `osSendMesg` on a full queue.
    BlockedOnSend,
    /// Woken because a message became available / space freed.
    Woken,
    /// Woken because a timer fired and posted to this thread's queue.
    TimerFired,
    /// The executor picked the next runnable thread after the previous one
    /// yielded/blocked/finished, with no more specific reason to report.
    Scheduled,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum QueueOpKind {
    Send,
    Recv,
    Block,
    Wake,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DmaDirection {
    ToRdram,
    FromRdram,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TaskKind {
    Graphics,
    Audio,
}

/// See `docs/DESIGN.md` section 4's `TraceKind` -- transcribed verbatim
/// (field names, variants) so this type IS the wire format the comparator
/// expects, not a look-alike that needs translating.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum TraceKind {
    ThreadSwitch {
        from: Option<ThreadId>,
        to: ThreadId,
        reason: SwitchReason,
    },
    QueueOp {
        queue: crate::RdramAddr,
        op: QueueOpKind,
        thread: ThreadId,
    },
    Dma {
        direction: DmaDirection,
        dram: crate::RdramAddr,
        dev_addr: u32,
        len: u32,
    },
    TaskSubmit {
        task_kind: TaskKind,
        ucode: u32,
    },
}

/// See `docs/DESIGN.md` section 4's `TraceEvent`. `sim_time` is the
/// executor's virtual clock (§1 of this module's caller, `timer.rs`) --
/// never wall-clock, per the task's "Timers... driven by a virtual clock
/// the host advances -- no wall-clock in core" requirement.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct TraceEvent {
    pub seq: u64,
    pub sim_time: u64,
    pub kind: TraceKind,
}

/// Where emitted `TraceEvent`s go. Kept as a `Vec` behind the executor
/// rather than any global/thread-local sink: per the task's "ONE explicit
/// host-side injection point" requirement for external events, the trace
/// sink is likewise owned by the one `Executor` instance, not ambient
/// global state a second copy of the runtime (e.g. two independent test
/// executors running in the same process) could cross-contaminate.
#[derive(Default)]
pub struct TraceLog {
    events: Vec<TraceEvent>,
}

impl TraceLog {
    pub fn record(&mut self, sim_time: u64, kind: TraceKind) {
        self.events.push(TraceEvent {
            seq: next_sequence(),
            sim_time,
            kind,
        });
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }
}
