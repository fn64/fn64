//! `Peripherals`: VI/SI/RSP host-side hardware-model state, extracted from
//! `Executor` so the executor's own surface stays scheduling + queues +
//! timers + the single `inject_event` door.
//!
//! ## Why this split, and why now
//!
//! Before this module existed, `Executor` directly owned `vi: ViState`,
//! `retrace: Option<RetraceSchedule>`, `pif: PifModel`, and `tasks: TaskLog`,
//! with every VI/SI/RSP-facing method (`vi_set_mode`, `vi_swap_buffer`,
//! `pif()`, `submit_task`, `arm_retrace`, etc.) implemented directly as
//! `impl Executor` methods that touched those fields. That made `Executor`
//! a god-object: scheduling/queue/timer logic (its actual job, per
//! `docs/DESIGN.md` section 2) sat in the same `impl` block, and often the
//! same file region, as VI mode-setting and PIF-probe formatting, which have
//! nothing to do with the single-runnable-coroutine invariant `Executor`
//! exists to enforce.
//!
//! This is a **pure structural move, zero behavior change**: every method
//! below has the exact same body it had as an `Executor` method (same field
//! reads/writes, same trace-recording call shape), just relocated. `Executor`
//! now holds one `peripherals: Peripherals` field and re-exposes the same
//! public methods as thin one-line delegations (see `executor.rs`'s "VI"/
//! "SI/PIF"/"RSP task submission" sections) -- callers in `fn64-abi` did not
//! change at all, which is the point: the ABI surface this crate promises
//! (`docs/DESIGN.md` section 1: "fn64-abi... every symbol here is a
//! signature-and-marshalling adapter") is unaffected by where the
//! implementation actually lives.
//!
//! `event_table` (the general `osSetEventMesg`-populated `OS_EVENT_*` ->
//! `(queue, msg)` table) deliberately STAYS on `Executor`, not here: it is
//! genuinely shared scheduling machinery (both a guest `osSetEventMesg`
//! registration and the VI retrace ticker's `OS_EVENT_VI` lookup go through
//! it, and `inject_event`'s `ExternalEvent::OsEvent` arm is peripheral-
//! agnostic -- it has no idea whether a given event code "belongs" to VI, SI,
//! or something else entirely). Moving it here would just relocate the
//! god-object problem one file over; it belongs with the run queue/blocked-
//! list machinery it's peered with in `Executor`.
//!
//! Trace recording (`TraceLog`) also stays on `Executor`: it needs
//! `sim_time`, which is `Executor`'s virtual clock, not a peripheral's own
//! state. `Peripherals`' methods that used to also call
//! `self.trace.record(...)` (`vi_swap_buffer`, `submit_task`) now return the
//! plain data the caller needs to record that same event itself -- see each
//! method's doc comment for the exact shape carried over.

use crate::rdram::RdramAddr;
use crate::rsp::{OsTaskHeader, TaskLog};
use crate::si::PifModel;
use crate::vi::{RetraceSchedule, ViState};

/// VI (video interface) + SI/PIF (controller probe) + RSP (task submission)
/// host-side hardware-model state. See module doc for why these three are
/// grouped: they are the peripherals `docs/DESIGN.md` section 1/2 describes
/// as host-driven models with no coroutine of their own, as distinct from
/// `Executor`'s scheduling/queue/timer state.
#[derive(Default)]
pub struct Peripherals {
    /// VI hardware state (mode/features/y-scale/blanked/last-swapped
    /// framebuffer) -- see `vi.rs` module doc.
    vi: ViState,
    /// The periodic retrace ticker, driving `OS_EVENT_VI` delivery from
    /// `Executor::advance_time`. `None` until a host driver calls
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

/// What `Peripherals::advance_retrace` found this tick -- the caller
/// (`Executor::advance_time`) still owns actually delivering these through
/// `inject_event`/`deliver_or_enqueue`, since delivery needs executor-owned
/// queue/blocked-list state `Peripherals` has no access to (by design --
/// see module doc).
pub struct RetraceTick {
    /// How many `OS_EVENT_VI` retrace ticks fired this call (see
    /// `RetraceSchedule::advance`'s doc comment for why this can be >1).
    pub event_vi_ticks: u32,
    /// The VI manager's own `osViSetEvent` retrace target, if one has been
    /// registered -- delivered once per call regardless of
    /// `event_vi_ticks`' count, matching `Executor::advance_time`'s prior
    /// behavior exactly (one `deliver_or_enqueue` per fired tick, inside the
    /// same loop `event_vi_ticks` counts).
    pub retrace_target: Option<(u32, u32)>,
}

impl Peripherals {
    pub fn new() -> Self {
        Self::default()
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
    /// `osSetEventMesg`.
    pub fn vi_set_event(&mut self, mq_addr: RdramAddr, msg: crate::mesgqueue::Mesg) {
        self.vi.set_event(mq_addr, msg);
    }

    pub fn vi_set_black(&mut self, active: bool) {
        self.vi.set_black(active);
    }

    /// `osViSwapBuffer(frameBufPtr)`. Returns the newly-current framebuffer
    /// address, matching `Executor::vi_swap_buffer`'s previous return shape
    /// exactly -- the caller (`Executor`) still records the shared
    /// `TaskSubmit` trace event itself (see module doc: trace recording
    /// needs `sim_time`, which lives on `Executor`, not here).
    pub fn vi_swap_buffer(&mut self, frame_buf: RdramAddr) -> RdramAddr {
        self.vi.swap_buffer(frame_buf);
        frame_buf
    }

    /// Arm the periodic VI retrace ticker at `interval` virtual-time units
    /// per field. See `vi.rs`'s `RetraceSchedule` doc -- not a hardware-
    /// accurate NTSC/PAL timing value, a host-chosen approximation.
    pub fn arm_retrace(&mut self, interval: u64) {
        self.retrace = Some(RetraceSchedule::new(interval));
    }

    /// Advance the retrace ticker to `now`, if armed. Returns `None` if
    /// never armed (matching `Executor::advance_time`'s prior "no `if let
    /// Some(sched)`, nothing happens" behavior exactly), else the tick
    /// counts the caller needs to actually deliver (see `RetraceTick`'s doc
    /// comment for why delivery itself stays the caller's job).
    pub fn advance_retrace(&mut self, now: u64) -> Option<RetraceTick> {
        let sched = self.retrace.as_mut()?;
        let event_vi_ticks = sched.advance(now);
        Some(RetraceTick {
            event_vi_ticks,
            retrace_target: self.vi.retrace_target,
        })
    }

    // ---- SI/PIF (controller probe) ---------------------------------------

    pub fn pif(&self) -> &PifModel {
        &self.pif
    }

    // ---- RSP task submission -----------------------------------------------

    pub fn task_log(&self) -> &TaskLog {
        &self.tasks
    }

    /// Record an RSP task submission. Returns the task's `TaskKind`, if any,
    /// so the caller (`Executor::submit_task`) can still emit the shared
    /// `TaskSubmit` trace event itself (see module doc: trace recording
    /// needs `sim_time`, not modeled here) -- same information
    /// `Executor::submit_task`'s prior single-body version derived from
    /// `header.kind()` before calling `self.tasks.record(header)`.
    pub fn submit_task(&mut self, header: OsTaskHeader) -> Option<crate::trace::TaskKind> {
        let kind = header.kind();
        self.tasks.record(header);
        kind
    }
}
