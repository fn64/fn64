//! VI (Video Interface) state + retrace event delivery + framebuffer swap
//! capture.
//!
//! ## Provenance
//!
//! Semantics come from the public libultra manual's VI Manager section
//! (`osViSetMode`/`osViSetSpecialFeatures`/`osViSetYScale`/`osViSwapBuffer`/
//! `osViBlack`, `OS_EVENT_VI` retrace delivery via `osViSetEvent`/
//! `osCreateViManager`) plus `aki-recomp/runtime/M1-WORKLIST.md`'s rung-11
//! evidence (the VI-bring-up call cluster's exact call order and sizes) and
//! `docs/DESIGN.md` section 2's "VI/timer event delivery... model them as
//! executor-level scheduling inputs" design point. No GPL runtime VI
//! implementation was read.
//!
//! ## Design
//!
//! Real VI hardware free-runs a retrace counter and fires an interrupt every
//! field; libultra's VI manager thread receives that interrupt as an
//! `OS_EVENT_VI` message and is the thing that actually calls
//! `osViSwapBuffer` in response to the game's own request queued earlier.
//! This module does not model a whole VI-manager *thread* (`docs/DESIGN.md`
//! section 2: that's the executor's job, driven by `Executor::inject_event`
//! against the `EventTable` `osSetEventMesg_recomp` populates for
//! `OS_EVENT_VI` == 3 per the rung-11 citation) -- it owns only the
//! host-side hardware model: current mode/features/y-scale, the "is the
//! screen blanked" bit, and (the actual deliverable for this milestone) the
//! most recently swapped framebuffer pointer, which is what the boot harness
//! needs to capture and hash/dump.
//!
//! `VirtualClockRetrace` is a simple periodic-tick model driven by the same
//! virtual clock `Executor::advance_time` already advances (per
//! `docs/DESIGN.md`'s "no wall-clock in core" rule) -- NOT a real VI
//! timing-register simulation (no evidence yet cites the exact NTSC/PAL
//! field-duration constant this ROM's `osViSetMode` argument selects; a
//! fabricated cycle-accurate value would be exactly the kind of unearned
//! precision `AGENTS.md` asks us to avoid). It fires a retrace every
//! `retrace_interval` virtual-time units, a caller-supplied approximation
//! (the harness picks a value; not asserted as hardware-accurate).
use crate::rdram::RdramAddr;

/// `OS_VI_NTSC_LPN1`/etc mode selector -- stored opaquely (the raw `OSViMode*`
/// vram this milestone doesn't need to interpret byte-for-byte, since no
/// shim yet reads mode-table fields back out; see `osViSetMode`'s doc
/// comment in `fn64-abi` for what IS verified: the call happens, with this
/// argument).
#[derive(Copy, Clone, Debug, Default)]
pub struct ViState {
    /// Raw vram pointer to the last `OSViMode*` passed to `osViSetMode`, or
    /// `None` before the first call.
    pub mode_ptr: Option<u32>,
    /// Raw vram pointer to the last `OSViSpecialFeatures*` passed to
    /// `osViSetSpecialFeatures`.
    pub special_features_ptr: Option<u32>,
    pub y_scale: Option<f32>,
    /// `osViBlack(active)`'s last-set value.
    pub blanked: bool,
    /// The most recent `osViSwapBuffer(frameBufPtr)` argument -- an rdram
    /// address, per the documented signature. `None` before the first swap.
    pub current_framebuffer: Option<RdramAddr>,
    /// Total `osViSwapBuffer` calls observed -- the task's "count them"
    /// requirement, mirrored here for a host-side introspection point
    /// distinct from the trace log (which also records a `TaskSubmit`-shaped
    /// event per swap via the caller).
    pub swap_count: u64,
    /// The VI-manager's own retrace notification target, set by
    /// `osViSetEvent(mq, msg, retraceCount)` -- per
    /// `aki-recomp/games/NWXE/profile.toml`'s rung-11 evidence, this is a
    /// DIFFERENT mechanism from `osSetEventMesg`'s general
    /// `OS_EVENT_*`-keyed table: it writes directly into the VI manager's
    /// own internal `__osViNext`-shaped struct (`->0x10=mq, ->0x14=msg`),
    /// which is what the VI manager posts to on retrace REGARDLESS of
    /// whether the game separately registered `OS_EVENT_VI` via
    /// `osSetEventMesg`. Modeled here as its own field rather than reusing
    /// the executor's `event_table`, since real hardware keeps these as two
    /// genuinely separate delivery paths (see `vi_set_event`'s doc comment).
    pub retrace_target: Option<(u32, u32)>, // (mq_addr_offset, msg)
}

impl ViState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mode(&mut self, mode_ptr: u32) {
        self.mode_ptr = Some(mode_ptr);
    }

    pub fn set_special_features(&mut self, ptr: u32) {
        self.special_features_ptr = Some(ptr);
    }

    pub fn set_y_scale(&mut self, scale: f32) {
        self.y_scale = Some(scale);
    }

    pub fn set_black(&mut self, active: bool) {
        self.blanked = active;
    }

    /// `osViSwapBuffer(frameBufPtr)`. Returns the PREVIOUS framebuffer (the
    /// one being replaced), so a caller wanting "what was on screen before
    /// this swap" can still see it -- not used by this milestone's shims,
    /// but a real, tested piece of behavior rather than a discarded value.
    pub fn swap_buffer(&mut self, frame_buf: RdramAddr) -> Option<RdramAddr> {
        self.swap_count += 1;
        self.current_framebuffer.replace(frame_buf)
    }

    /// `osViSetEvent(mq, msg, retraceCount)` -- see `retrace_target`'s doc
    /// comment for why this is separate from `osSetEventMesg`. `retraceCount`
    /// is accepted but not modeled numerically (no boot-rung evidence yet
    /// exercises a non-1 value; storing an unused field would be an
    /// unearned-precision guess per `AGENTS.md`).
    pub fn set_event(&mut self, mq_addr: RdramAddr, msg: u32) {
        self.retrace_target = Some((mq_addr.offset(), msg));
    }
}

/// A simple virtual-clock-driven periodic retrace ticker. Not a hardware
/// timing model (see module doc) -- just "every N virtual-time units, a
/// retrace happened," which is what `Executor::advance_time` needs to decide
/// whether to fire `OS_EVENT_VI` this tick.
#[derive(Copy, Clone, Debug)]
pub struct RetraceSchedule {
    pub interval: u64,
    next_due: u64,
}

impl RetraceSchedule {
    pub fn new(interval: u64) -> Self {
        RetraceSchedule {
            interval: interval.max(1),
            next_due: interval.max(1),
        }
    }

    /// Advance to `now`; returns how many retrace ticks fired (usually 0 or
    /// 1, but more if the host skipped several intervals in one jump -- a
    /// real VI's interrupt would fire once per field regardless of how late
    /// software services it, so this reports every interval crossed, not
    /// just "at least one").
    pub fn advance(&mut self, now: u64) -> u32 {
        let mut fired = 0;
        while now >= self.next_due {
            self.next_due += self.interval;
            fired += 1;
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_buffer_records_current_and_returns_previous() {
        let mut vi = ViState::new();
        assert_eq!(vi.swap_buffer(RdramAddr::from_offset(0x1000)), None);
        assert_eq!(vi.swap_count, 1);
        let prev = vi.swap_buffer(RdramAddr::from_offset(0x2000));
        assert_eq!(prev, Some(RdramAddr::from_offset(0x1000)));
        assert_eq!(vi.current_framebuffer, Some(RdramAddr::from_offset(0x2000)));
        assert_eq!(vi.swap_count, 2);
    }

    #[test]
    fn retrace_schedule_fires_once_per_interval_crossed() {
        let mut sched = RetraceSchedule::new(100);
        assert_eq!(sched.advance(50), 0);
        assert_eq!(sched.advance(100), 1);
        assert_eq!(sched.advance(150), 0);
        assert_eq!(sched.advance(200), 1);
        // Jump far ahead: multiple intervals crossed at once.
        assert_eq!(sched.advance(500), 3);
    }

    #[test]
    fn black_and_features_are_stored() {
        let mut vi = ViState::new();
        vi.set_black(true);
        assert!(vi.blanked);
        vi.set_mode(0x8004_1234);
        assert_eq!(vi.mode_ptr, Some(0x8004_1234));
        vi.set_special_features(0x8004_5678);
        assert_eq!(vi.special_features_ptr, Some(0x8004_5678));
        vi.set_y_scale(1.0);
        assert_eq!(vi.y_scale, Some(1.0));
    }
}
