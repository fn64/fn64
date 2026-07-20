//! VI (Video Interface) state + retrace event delivery + framebuffer swap
//! capture.
//!
//! ## Provenance
//!
//! Semantics come from the public libultra manual's VI Manager section
//! (`osViSetMode`/`osViSetSpecialFeatures`/`osViSetXScale`/`osViSetYScale`/`osViSwapBuffer`/
//! `osViBlack`/`osViFade`/`osViRepeatLine`, `OS_EVENT_VI` retrace delivery via `osViSetEvent`/
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
//! executor-side libultra state: current and next mode/features/scales,
//! blanking, and framebuffer pointers. Pending changes latch together at the
//! next VI interrupt, matching the public VI Manager manual.
//!
//! `RetraceSchedule` remains the standalone `Executor` compatibility ticker.
//! Integrated ABI execution instead schedules VI in `DeviceFabric`, which
//! owns the raw register image and half-line/interrupt behavior. Both paths
//! call the same pending-state latch and message-delivery method. Integrated
//! execution derives each field duration from the IPL-selected NTSC/PAL/MPAL
//! video clock and the live H_SYNC/V_SYNC pair, using the public register
//! units. The nominal public 60/50 Hz rate is only the pre-mode bootstrap;
//! `RetraceSchedule` remains an explicit compatibility/test ticker.
use crate::rdram::RdramAddr;

/// Current/next libultra VI-manager state. `fn64-abi` separately decodes the
/// public `OSViMode` layout into `DeviceFabric`'s raw register image.
#[derive(Copy, Clone, Debug, Default)]
pub struct ViState {
    /// Raw vram pointer to the last `OSViMode*` passed to `osViSetMode`, or
    /// `None` before the first call.
    pub mode_ptr: Option<u32>,
    pub next_mode_ptr: Option<u32>,
    next_mode_resets_overrides: bool,
    /// Last public `osViSetSpecialFeatures(u32)` command bitmask.
    pub special_features: Option<u32>,
    pub next_special_features: Option<u32>,
    pub x_scale: Option<f32>,
    pub y_scale: Option<f32>,
    pub next_x_scale: Option<f32>,
    pub next_y_scale: Option<f32>,
    /// `osViBlack(active)`'s last-set value.
    pub blanked: bool,
    pub next_blanked: Option<bool>,
    /// Active public `osViFade` interpolation factor, or disabled.
    pub fade: Option<u16>,
    pub next_fade: Option<Option<u16>>,
    /// Active public `osViRepeatLine` state.
    pub repeat_line: bool,
    pub next_repeat_line: Option<bool>,
    /// The most recent `osViSwapBuffer(frameBufPtr)` argument -- an rdram
    /// address, per the documented signature. `None` before the first swap.
    pub current_framebuffer: Option<RdramAddr>,
    /// Most recent framebuffer requested through `osViSwapBuffer`; becomes
    /// current only when [`Self::latch_retrace`] runs.
    pub next_framebuffer: Option<RdramAddr>,
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
    /// Number of hardware retraces per VI-manager notification.
    pub retrace_count: u32,
    retrace_phase: u32,
}

impl ViState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mode(&mut self, mode_ptr: u32) {
        self.next_mode_ptr = Some(mode_ptr);
        self.next_mode_resets_overrides = true;
        self.next_special_features = None;
        self.next_x_scale = None;
        self.next_y_scale = None;
        self.next_blanked = None;
        self.next_fade = None;
        self.next_repeat_line = None;
    }

    pub fn set_special_features(&mut self, features: u32) {
        self.next_special_features = Some(features);
    }

    pub fn set_y_scale(&mut self, scale: f32) {
        self.next_y_scale = Some(scale);
    }

    pub fn set_x_scale(&mut self, scale: f32) {
        self.next_x_scale = Some(scale);
    }

    pub fn set_black(&mut self, active: bool) {
        self.next_blanked = Some(active);
    }

    pub fn set_fade(&mut self, active: bool, factor: u16) {
        assert!(factor <= 0x03ff, "osViFade factor exceeds 10 bits");
        self.next_fade = Some(active.then_some(factor));
    }

    pub fn set_repeat_line(&mut self, active: bool) {
        self.next_repeat_line = Some(active);
    }

    /// Queue `frameBufPtr` for the next V-blank and return the previously
    /// queued pointer. The currently displayed framebuffer is unchanged.
    pub fn swap_buffer(&mut self, frame_buf: RdramAddr) -> Option<RdramAddr> {
        self.swap_count += 1;
        self.next_framebuffer.replace(frame_buf)
    }

    /// Atomically apply every VI-manager change pending for V-blank. Returns
    /// whether scanout-visible state changed, allowing the host renderer to
    /// present framebuffer swaps and VI scanout-effect transitions at this
    /// boundary rather than at submission time.
    pub fn latch_retrace(&mut self) -> bool {
        let old_blanked = self.blanked;
        let old_fade = self.fade;
        let old_repeat_line = self.repeat_line;
        if let Some(mode) = self.next_mode_ptr {
            self.mode_ptr = Some(mode);
        }
        if self.next_mode_resets_overrides {
            self.special_features = None;
            self.x_scale = None;
            self.y_scale = None;
            self.blanked = false;
            self.fade = None;
            self.repeat_line = false;
            self.next_mode_resets_overrides = false;
        }
        if let Some(features) = self.next_special_features {
            self.special_features = Some(features);
        }
        if let Some(scale) = self.next_x_scale {
            self.x_scale = Some(scale);
        }
        if let Some(scale) = self.next_y_scale {
            self.y_scale = Some(scale);
        }
        if let Some(blanked) = self.next_blanked {
            let effective_y_scale = self.next_y_scale.or(self.y_scale).unwrap_or(1.0);
            assert!(
                !blanked || effective_y_scale == 1.0,
                "osViBlack(TRUE) requires an effective Y scale of 1.0"
            );
            assert!(
                !blanked || self.next_fade.unwrap_or(self.fade).is_none(),
                "osViBlack(TRUE) requires osViFade to be disabled"
            );
            assert!(
                !blanked || !self.next_repeat_line.unwrap_or(self.repeat_line),
                "osViBlack(TRUE) requires osViRepeatLine to be disabled"
            );
            self.blanked = blanked;
        }
        if let Some(fade) = self.next_fade {
            self.fade = fade;
        }
        if let Some(repeat_line) = self.next_repeat_line {
            self.repeat_line = repeat_line;
        }
        assert!(
            self.fade.is_none() || !self.repeat_line,
            "osViFade and osViRepeatLine cannot be enabled together"
        );
        let changed = self.next_framebuffer != self.current_framebuffer;
        if let Some(framebuffer) = self.next_framebuffer {
            self.current_framebuffer = Some(framebuffer);
        }
        changed
            || self.blanked != old_blanked
            || self.fade != old_fade
            || self.repeat_line != old_repeat_line
    }

    /// `osViSetEvent(mq, msg, retraceCount)` -- see `retrace_target`'s doc
    /// comment for why this is separate from `osSetEventMesg`.
    pub fn set_event(&mut self, mq_addr: RdramAddr, msg: u32, retrace_count: u32) {
        assert!(
            retrace_count > 0,
            "osViSetEvent: retraceCount must be nonzero"
        );
        self.retrace_target = Some((mq_addr.offset(), msg));
        self.retrace_count = retrace_count;
        self.retrace_phase = 0;
    }

    /// Advance the VI-manager's public retrace divisor and return its target
    /// only on the selected field. The general `OS_EVENT_VI` path remains
    /// independent and fires on every hardware interrupt.
    pub fn manager_target_for_retrace(&mut self) -> Option<(u32, u32)> {
        let target = self.retrace_target?;
        self.retrace_phase += 1;
        if self.retrace_phase == self.retrace_count {
            self.retrace_phase = 0;
            Some(target)
        } else {
            None
        }
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
        assert_eq!(vi.current_framebuffer, None);
        assert!(vi.latch_retrace());
        assert_eq!(vi.current_framebuffer, Some(RdramAddr::from_offset(0x1000)));
        let prev = vi.swap_buffer(RdramAddr::from_offset(0x2000));
        assert_eq!(prev, Some(RdramAddr::from_offset(0x1000)));
        assert_eq!(vi.current_framebuffer, Some(RdramAddr::from_offset(0x1000)));
        assert!(vi.latch_retrace());
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
        assert!(!vi.blanked);
        vi.set_mode(0x8004_1234);
        vi.set_black(true);
        assert_eq!(vi.next_mode_ptr, Some(0x8004_1234));
        vi.set_special_features(0x55);
        assert_eq!(vi.next_special_features, Some(0x55));
        vi.set_x_scale(0.75);
        assert_eq!(vi.next_x_scale, Some(0.75));
        vi.set_y_scale(1.0);
        assert_eq!(vi.next_y_scale, Some(1.0));
        assert!(vi.latch_retrace());
        assert!(vi.blanked);
        assert_eq!(vi.mode_ptr, Some(0x8004_1234));
        assert_eq!(vi.special_features, Some(0x55));
        assert_eq!(vi.x_scale, Some(0.75));
        assert_eq!(vi.y_scale, Some(1.0));
        assert!(!vi.latch_retrace());
    }

    #[test]
    #[should_panic(expected = "osViBlack(TRUE) requires an effective Y scale of 1.0")]
    fn black_rejects_non_unit_effective_y_scale_at_the_latch_boundary() {
        let mut vi = ViState::new();
        vi.set_y_scale(0.5);
        vi.set_black(true);
        vi.latch_retrace();
    }

    #[test]
    fn vi_manager_retrace_count_divides_only_its_registered_target() {
        let mut vi = ViState::new();
        vi.set_event(RdramAddr::from_offset(0x200), 0x32, 2);

        assert_eq!(vi.manager_target_for_retrace(), None);
        assert_eq!(vi.manager_target_for_retrace(), Some((0x200, 0x32)));
        assert_eq!(vi.manager_target_for_retrace(), None);
        assert_eq!(vi.manager_target_for_retrace(), Some((0x200, 0x32)));
    }

    #[test]
    fn queued_mode_resets_earlier_scale_and_feature_overrides() {
        let mut vi = ViState::new();
        vi.set_x_scale(0.5);
        vi.set_y_scale(0.75);
        vi.set_special_features(0xaa);
        vi.set_black(true);
        vi.set_fade(true, 0x100);
        vi.set_mode(0x8000_1000);
        vi.latch_retrace();

        assert_eq!(vi.mode_ptr, Some(0x8000_1000));
        assert_eq!(vi.x_scale, None);
        assert_eq!(vi.y_scale, None);
        assert_eq!(vi.special_features, None);
        assert!(!vi.blanked);
        assert_eq!(vi.fade, None);
        assert!(!vi.repeat_line);
    }

    #[test]
    fn fade_and_repeat_line_are_distinct_vblank_latched_scanout_changes() {
        let mut vi = ViState::new();
        vi.set_fade(true, 0x0200);
        assert!(vi.latch_retrace());
        assert_eq!(vi.fade, Some(0x0200));
        assert!(!vi.repeat_line);
        assert!(!vi.latch_retrace());

        vi.set_fade(false, 0);
        assert!(vi.latch_retrace());
        assert_eq!(vi.fade, None);

        vi.set_repeat_line(true);
        assert!(vi.latch_retrace());
        assert!(vi.repeat_line);
    }
}
