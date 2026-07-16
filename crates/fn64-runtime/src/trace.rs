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

use std::fs::File;
use std::io::Write;
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
pub struct TraceLog {
    /// Differential tracing is diagnostic work, not emulated-machine state.
    /// Runtime users can disable it for production-speed execution without
    /// changing scheduler, queue, DMA, or task behavior.
    enabled: bool,
    events: Vec<TraceEvent>,
    /// Optional incremental sink: when set (`set_sink_file`), every
    /// `record()` call appends+flushes this event's `Debug` line to the
    /// file IMMEDIATELY, not just to the in-memory `events` Vec. Added
    /// after a real crash (WM2000 boot rung 3, `func_80026F18` SIGSEGV)
    /// lost the entire session's trace: the harness only ever serialized
    /// `events` to disk at the very end of a clean run, so a SIGSEGV mid-
    /// boot left zero evidence on disk despite the crash happening after
    /// dozens of real trace-worthy events. A `File` (not a `BufWriter`) is
    /// used deliberately, with an explicit `flush()` after every write --
    /// buffered-and-unflushed output is exactly as lost on a hard crash as
    /// never writing it, so "flush every event" is the actual fix, not an
    /// optimization to skip under time pressure.
    sink: Option<File>,
}

impl Default for TraceLog {
    fn default() -> Self {
        Self {
            enabled: true,
            events: Vec::new(),
            sink: None,
        }
    }
}

impl TraceLog {
    pub fn record(&mut self, sim_time: u64, kind: TraceKind) {
        if !self.enabled {
            return;
        }
        let event = TraceEvent {
            seq: next_sequence(),
            sim_time,
            kind,
        };
        if let Some(file) = self.sink.as_mut() {
            let line = format!("{event:?}\n");
            // Best-effort: a write/flush failure here must not panic or
            // abort the boot -- this is a diagnostic side channel, not
            // part of the emulated machine's behavior. Silently continuing
            // (rather than disabling the sink) matches "flush every event"
            // as the steady-state contract; a transient failure (e.g. a
            // full disk) shouldn't quietly stop future events from trying.
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
        self.events.push(event);
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }

    /// Enable or disable diagnostic event capture. Disabling clears any
    /// previously captured events and closes the crash-safe sink so a caller
    /// cannot accidentally keep paying trace I/O after opting out.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.events.clear();
            self.sink = None;
        }
    }

    /// Arm incremental disk flushing: every subsequent `record()` call
    /// appends its line to `path` (truncating any prior content) and
    /// flushes before returning, so a SIGSEGV/abort immediately after the
    /// Nth event still leaves all N lines on disk. Returns the `io::Error`
    /// if the file couldn't be created, so the caller can decide whether a
    /// missing trace file is fatal for their use case (the harness treats
    /// it as a loud warning, not a panic -- see `main.rs`).
    pub fn set_sink_file(&mut self, path: &str) -> std::io::Result<()> {
        self.enabled = true;
        self.sink = Some(File::create(path)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the WM2000 rung-3 harness gap: a `TraceLog`
    /// with a sink armed must have every recorded event already durable on
    /// disk WITHOUT ever calling any "finalize"/"close"/`Drop` step -- the
    /// whole point is surviving a SIGSEGV, which runs no destructors at
    /// all. This test never calls a shutdown/flush-on-drop method; it
    /// reads the file back while `log` is still very much alive and
    /// un-dropped, simulating "the process is about to be killed right
    /// here" by just... not doing anything special before reading.
    #[test]
    fn record_flushes_each_event_to_the_sink_file_immediately() {
        reset_sequence_for_test();
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "fn64-trace-test-{}-{}.jsonl",
            std::process::id(),
            next_sequence()
        ));
        let path_str = path.to_str().unwrap();

        let mut log = TraceLog::default();
        log.set_sink_file(path_str)
            .expect("creating the sink file must succeed");

        log.record(
            10,
            TraceKind::ThreadSwitch {
                from: None,
                to: 0,
                reason: SwitchReason::Scheduled,
            },
        );
        // Read back after ONE event, before recording the second -- proves
        // each individual record() call is durable on its own, not just
        // "eventually all get there by the time we check."
        let after_one =
            std::fs::read_to_string(&path).expect("trace file must exist after 1 record()");
        assert_eq!(
            after_one.lines().count(),
            1,
            "exactly 1 line must be on disk after exactly 1 record() call, with no flush/close \
             step called -- got: {after_one:?}"
        );

        log.record(
            20,
            TraceKind::QueueOp {
                queue: crate::rdram::RdramAddr::from_offset(0x0000_0000),
                op: QueueOpKind::Send,
                thread: 1,
            },
        );
        log.record(
            30,
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Graphics,
                ucode: 0x8000_1000,
            },
        );

        // Still no explicit flush/close call anywhere above -- this is the
        // "crash right now" checkpoint for 3 events.
        let after_three =
            std::fs::read_to_string(&path).expect("trace file must exist after 3 record()s");
        let lines: Vec<&str> = after_three.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "exactly 3 lines must be on disk after exactly 3 record() calls with zero \
             flush/close calls -- got: {after_three:?}"
        );
        assert!(lines[0].contains("ThreadSwitch"));
        assert!(lines[1].contains("QueueOp"));
        assert!(lines[2].contains("TaskSubmit"));

        // Sanity: the in-memory copy (what `events()`/`copy_trace` expose
        // on a CLEAN exit) still has all 3 too -- the sink is additive, not
        // a replacement for the existing in-memory path.
        assert_eq!(log.events().len(), 3);

        let _ = std::fs::remove_file(&path);
    }

    /// A `TraceLog` with no sink armed (the pre-fix default, and any test/
    /// caller that never calls `set_sink_file`) must behave exactly as
    /// before: `record()` only appends in-memory, no filesystem I/O at all.
    #[test]
    fn record_without_a_sink_does_not_touch_the_filesystem() {
        reset_sequence_for_test();
        let mut log = TraceLog::default();
        log.record(
            1,
            TraceKind::ThreadSwitch {
                from: None,
                to: 0,
                reason: SwitchReason::Scheduled,
            },
        );
        assert_eq!(log.events().len(), 1);
        assert!(log.sink.is_none());
    }

    #[test]
    fn disabled_trace_drops_events_without_allocating() {
        let mut log = TraceLog::default();
        log.set_enabled(false);
        log.record(
            1,
            TraceKind::ThreadSwitch {
                from: None,
                to: 0,
                reason: SwitchReason::Scheduled,
            },
        );
        assert!(log.events().is_empty());
        assert_eq!(log.events.capacity(), 0);
    }
}
