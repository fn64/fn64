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
//!
//! Fixed-cycle evidence uses [`ViState::evidence_snapshot`] together with
//! [`RetraceSchedule::evidence_snapshot`]. These views include private queued
//! latch/divisor state and the standalone ticker deadline because equal raw VI
//! registers alone do not imply equal behavior at the next retrace.
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

/// Release-evidence view of every future-affecting high-level VI-manager
/// value owned by [`ViState`]. Floating-point scales are retained as their
/// exact IEEE-754 bit patterns so downstream canonical encoders never depend
/// on text formatting or host float serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViEvidenceSnapshot {
    pub mode_ptr: Option<u32>,
    pub next_mode_ptr: Option<u32>,
    pub next_mode_resets_overrides: bool,
    pub special_features: Option<u32>,
    pub next_special_features: Option<u32>,
    pub x_scale_bits: Option<u32>,
    pub y_scale_bits: Option<u32>,
    pub next_x_scale_bits: Option<u32>,
    pub next_y_scale_bits: Option<u32>,
    pub blanked: bool,
    pub next_blanked: Option<bool>,
    pub fade: Option<u16>,
    pub next_fade: PendingViFade,
    pub repeat_line: bool,
    pub next_repeat_line: Option<bool>,
    pub current_framebuffer: Option<u32>,
    pub next_framebuffer: Option<u32>,
    pub swap_count: u64,
    pub retrace_target: Option<(u32, u32)>,
    pub retrace_count: u32,
    pub retrace_phase: u32,
}

/// Three-state encoding of `ViState::next_fade`: no queued change, queued
/// disable, or a queued ten-bit interpolation factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingViFade {
    Unchanged,
    Disabled,
    Factor(u16),
}

impl ViState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot every high-level VI-manager value that can affect current
    /// observation or the result of a later retrace under identical input.
    pub fn evidence_snapshot(&self) -> ViEvidenceSnapshot {
        ViEvidenceSnapshot {
            mode_ptr: self.mode_ptr,
            next_mode_ptr: self.next_mode_ptr,
            next_mode_resets_overrides: self.next_mode_resets_overrides,
            special_features: self.special_features,
            next_special_features: self.next_special_features,
            x_scale_bits: self.x_scale.map(f32::to_bits),
            y_scale_bits: self.y_scale.map(f32::to_bits),
            next_x_scale_bits: self.next_x_scale.map(f32::to_bits),
            next_y_scale_bits: self.next_y_scale.map(f32::to_bits),
            blanked: self.blanked,
            next_blanked: self.next_blanked,
            fade: self.fade,
            next_fade: match self.next_fade {
                None => PendingViFade::Unchanged,
                Some(None) => PendingViFade::Disabled,
                Some(Some(factor)) => PendingViFade::Factor(factor),
            },
            repeat_line: self.repeat_line,
            next_repeat_line: self.next_repeat_line,
            current_framebuffer: self.current_framebuffer.map(RdramAddr::offset),
            next_framebuffer: self.next_framebuffer.map(RdramAddr::offset),
            swap_count: self.swap_count,
            retrace_target: self.retrace_target,
            retrace_count: self.retrace_count,
            retrace_phase: self.retrace_phase,
        }
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
    /// whether the manager-owned framebuffer, special-feature bits, black,
    /// fade, or repeat-line state changed. Mode and scale changes are omitted.
    /// Integrated execution presents every hardware field independently; this
    /// partial manager delta is never presentation admission authority.
    pub fn latch_retrace(&mut self) -> bool {
        let old_special_features = self.special_features;
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
            self.blanked = blanked;
        }
        if let Some(fade) = self.next_fade {
            self.fade = fade;
        }
        if let Some(repeat_line) = self.next_repeat_line {
            self.repeat_line = repeat_line;
        }
        assert!(
            !self.blanked || self.y_scale.unwrap_or(1.0) == 1.0,
            "osViBlack(TRUE) requires an effective Y scale of 1.0"
        );
        assert!(
            !self.blanked || self.fade.is_none(),
            "osViBlack(TRUE) requires osViFade to be disabled"
        );
        assert!(
            !self.blanked || !self.repeat_line,
            "osViBlack(TRUE) requires osViRepeatLine to be disabled"
        );
        assert!(
            self.fade.is_none() || !self.repeat_line,
            "osViFade and osViRepeatLine cannot be enabled together"
        );
        let changed = self.next_framebuffer != self.current_framebuffer;
        if let Some(framebuffer) = self.next_framebuffer {
            self.current_framebuffer = Some(framebuffer);
        }
        changed
            || self.special_features != old_special_features
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

/// Future-affecting state of the standalone compatibility retrace ticker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetraceScheduleEvidenceSnapshot {
    pub interval: u64,
    pub next_due: u64,
}

impl RetraceSchedule {
    pub fn new(interval: u64) -> Self {
        RetraceSchedule {
            interval: interval.max(1),
            next_due: interval.max(1),
        }
    }

    pub const fn evidence_snapshot(&self) -> RetraceScheduleEvidenceSnapshot {
        RetraceScheduleEvidenceSnapshot {
            interval: self.interval,
            next_due: self.next_due,
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
    fn feature_only_transition_is_scanout_visible() {
        let mut vi = ViState::new();
        vi.set_special_features(0x01);
        assert!(vi.latch_retrace());
        assert_eq!(vi.special_features, Some(0x01));
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
    #[should_panic(expected = "osViBlack(TRUE) requires an effective Y scale of 1.0")]
    fn already_black_scanout_rejects_a_later_non_unit_y_scale() {
        let mut vi = ViState::new();
        vi.set_black(true);
        vi.latch_retrace();
        vi.set_y_scale(0.5);
        vi.latch_retrace();
    }

    #[test]
    #[should_panic(expected = "osViBlack(TRUE) requires osViFade to be disabled")]
    fn already_black_scanout_rejects_a_later_fade() {
        let mut vi = ViState::new();
        vi.set_black(true);
        vi.latch_retrace();
        vi.set_fade(true, 0x0200);
        vi.latch_retrace();
    }

    #[test]
    #[should_panic(expected = "osViBlack(TRUE) requires osViRepeatLine to be disabled")]
    fn already_black_scanout_rejects_a_later_repeated_line() {
        let mut vi = ViState::new();
        vi.set_black(true);
        vi.latch_retrace();
        vi.set_repeat_line(true);
        vi.latch_retrace();
    }

    #[test]
    fn unblank_and_scanout_effect_may_latch_together() {
        let mut vi = ViState::new();
        vi.set_black(true);
        vi.latch_retrace();
        vi.set_black(false);
        vi.set_fade(true, 0x0200);
        assert!(vi.latch_retrace());
        assert!(!vi.blanked);
        assert_eq!(vi.fade, Some(0x0200));
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

    #[test]
    fn evidence_snapshot_preserves_every_active_and_pending_vi_family() {
        let mut vi = ViState::new();
        assert_eq!(vi.evidence_snapshot().next_fade, PendingViFade::Unchanged);
        vi.set_mode(0x8000_1000);
        vi.set_special_features(0x11);
        vi.set_x_scale(0.5);
        vi.set_y_scale(0.75);
        vi.set_fade(true, 0x155);
        vi.swap_buffer(RdramAddr::from_offset(0x1000));
        assert!(vi.latch_retrace());
        assert_eq!(
            vi.evidence_snapshot().next_fade,
            PendingViFade::Factor(0x155)
        );

        vi.set_mode(0x8000_2000);
        vi.set_special_features(0x22);
        vi.set_x_scale(-0.0);
        vi.set_y_scale(1.25);
        vi.set_black(false);
        vi.set_fade(false, 0);
        vi.set_repeat_line(true);
        vi.swap_buffer(RdramAddr::from_offset(0x2000));
        vi.set_event(RdramAddr::from_offset(0x3000), 0x44, 3);
        assert_eq!(vi.manager_target_for_retrace(), None);

        assert_eq!(
            vi.evidence_snapshot(),
            ViEvidenceSnapshot {
                mode_ptr: Some(0x8000_1000),
                next_mode_ptr: Some(0x8000_2000),
                next_mode_resets_overrides: true,
                special_features: Some(0x11),
                next_special_features: Some(0x22),
                x_scale_bits: Some(0.5f32.to_bits()),
                y_scale_bits: Some(0.75f32.to_bits()),
                next_x_scale_bits: Some((-0.0f32).to_bits()),
                next_y_scale_bits: Some(1.25f32.to_bits()),
                blanked: false,
                next_blanked: Some(false),
                fade: Some(0x155),
                next_fade: PendingViFade::Disabled,
                repeat_line: false,
                next_repeat_line: Some(true),
                current_framebuffer: Some(0x1000),
                next_framebuffer: Some(0x2000),
                swap_count: 2,
                retrace_target: Some((0x3000, 0x44)),
                retrace_count: 3,
                retrace_phase: 1,
            }
        );
    }

    #[test]
    fn retrace_schedule_evidence_preserves_policy_and_next_deadline() {
        let mut schedule = RetraceSchedule::new(10);
        assert_eq!(
            schedule.evidence_snapshot(),
            RetraceScheduleEvidenceSnapshot {
                interval: 10,
                next_due: 10,
            }
        );
        assert_eq!(schedule.advance(25), 2);
        assert_eq!(
            schedule.evidence_snapshot(),
            RetraceScheduleEvidenceSnapshot {
                interval: 10,
                next_due: 30,
            }
        );
    }
}
