//! `osSetTimer`/`osStopTimer` semantics: a countdown/interval timer that
//! posts a message to an `OSMesgQueue` when it fires, per the libultra
//! manual's Timer Manager section (public documentation) and the boot-rung
//! evidence in `docs/DESIGN.md` section 6's provenance table (rung 13,
//! `aki-recomp/games/NWXE/profile.toml`: `func_80032600`'s role-matched
//! `osSetTimer(OSTimer*, OSTime countdown, OSTime interval, OSMesgQueue*,
//! OSMesg)` shape -- list-node-zero-init header, OSTime pair, conditional
//! mq/msg fields, sentinel-headed insert-by-deadline list).
//!
//! Driven entirely by a virtual clock the executor advances (`Executor::
//! advance_time`/the VI-tick driver) -- never `std::time`/wall-clock, per
//! the task's explicit requirement and per `docs/DESIGN.md` section 4's
//! `sim_time: u64` field ("OS_CYCLES-comparable virtual time, not wall
//! clock"): a differential trace compared against the reference runtime
//! must be reproducible independent of host scheduling jitter, which a
//! wall-clock timer could never guarantee.

use crate::mesgqueue::Mesg;
use crate::trace::ThreadId;

pub type TimerId = u32;

/// One registered timer. `interval == 0` means "one-shot": it fires once at
/// `deadline` and is not re-armed (matches libultra's documented
/// `osSetTimer` semantics: a zero `interval` argument makes the timer
/// non-repeating).
struct Timer {
    deadline: u64,
    interval: u64,
    queue_addr: crate::RdramAddr,
    msg: Mesg,
    /// The thread that armed this timer, purely for trace attribution
    /// (`TraceKind::QueueOp`'s `thread` field) -- not used for scheduling
    /// decisions, since the message goes to whatever thread(s) are blocked
    /// on `queue_addr`, not necessarily the arming thread.
    armed_by: ThreadId,
}

/// One pending timer in the exact order [`TimerWheel::advance`] will inspect
/// it. Equal-deadline entries retain their current stable FIFO order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimerEvidenceSnapshot {
    pub id: TimerId,
    pub deadline: u64,
    pub interval: u64,
    pub queue_addr: crate::RdramAddr,
    pub msg: Mesg,
    pub armed_by: ThreadId,
}

/// Complete pointer-free scheduling evidence for the timer wheel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerWheelEvidenceSnapshot {
    pub next_id: TimerId,
    /// Deadline order, with stable insertion order among ties. This is the
    /// same ordering rule used by [`TimerWheel::advance`].
    pub firing_order: Vec<TimerEvidenceSnapshot>,
}

/// The timer wheel. Owned by the executor (see `executor.rs`), which is the
/// only thing that both advances virtual time and can act on a fired
/// timer's queue-post side effect -- matching §2's "VI/timer event delivery"
/// design: timer expiry is a host/executor-level scheduling input, never a
/// coroutine of its own.
#[derive(Default)]
pub struct TimerWheel {
    next_id: TimerId,
    timers: Vec<(TimerId, Timer)>,
}

/// A timer that just fired, returned by `TimerWheel::advance` for the
/// caller (the executor) to actually post to the named queue -- kept as
/// data rather than the wheel posting directly, so this module stays
/// free of any `MesgQueue`/executor dependency (mirrors `mesgqueue.rs`'s
/// existing split between "what changed" and "who acts on it").
pub struct FiredTimer {
    pub queue_addr: crate::RdramAddr,
    pub msg: Mesg,
    pub armed_by: ThreadId,
}

impl TimerWheel {
    /// Project the future firing schedule without sorting or otherwise
    /// mutating the live wheel.
    pub fn evidence_snapshot(&self) -> TimerWheelEvidenceSnapshot {
        let mut firing_order: Vec<_> = self
            .timers
            .iter()
            .map(|&(id, ref timer)| TimerEvidenceSnapshot {
                id,
                deadline: timer.deadline,
                interval: timer.interval,
                queue_addr: timer.queue_addr,
                msg: timer.msg,
                armed_by: timer.armed_by,
            })
            .collect();
        // Stable sort is intentional: `advance` uses the same operation, so
        // equal-deadline timers retain their current FIFO order.
        firing_order.sort_by_key(|timer| timer.deadline);
        TimerWheelEvidenceSnapshot {
            next_id: self.next_id,
            firing_order,
        }
    }

    /// `osSetTimer(t, countdown, interval, mq, msg)`. `countdown` and
    /// `interval` are `OSTime` (already-converted virtual-clock ticks, not
    /// raw `OS_CYCLES` -- that conversion is `fn64-abi`'s job per
    /// `docs/DESIGN.md` section 1's "dumb adapter" framing).
    pub fn set_timer(
        &mut self,
        now: u64,
        countdown: u64,
        interval: u64,
        queue_addr: crate::RdramAddr,
        msg: Mesg,
        armed_by: ThreadId,
    ) -> TimerId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.timers.push((
            id,
            Timer {
                deadline: now.saturating_add(countdown),
                interval,
                queue_addr,
                msg,
                armed_by,
            },
        ));
        id
    }

    /// `osStopTimer(t)`. No-op if the timer already fired (one-shot) or was
    /// never registered -- matching real hardware, where stopping an
    /// already-elapsed one-shot timer is a documented no-op, not an error.
    pub fn stop_timer(&mut self, id: TimerId) {
        self.timers.retain(|(tid, _)| *tid != id);
    }

    /// Advance the virtual clock to `now`, firing (and, for repeating
    /// timers, re-arming) everything whose deadline has passed. Returns
    /// fired timers in deadline order, matching libultra's own
    /// insertion-sorted list (rung 13's "insert into a sentinel-headed
    /// linked list... comparing against a current-time global... to
    /// insertion-sort by deadline").
    pub fn advance(&mut self, now: u64) -> Vec<FiredTimer> {
        let mut fired = Vec::new();
        let mut still_pending = Vec::with_capacity(self.timers.len());

        // Stable order by deadline so simultaneous-tick timers fire in the
        // order they were originally inserted among equal deadlines --
        // avoids an arbitrary Vec-retain order becoming a source of
        // trace-comparator noise against the reference runtime's own
        // deterministic list-insert order.
        self.timers.sort_by_key(|(_, t)| t.deadline);

        for (id, mut timer) in self.timers.drain(..) {
            if timer.deadline > now {
                still_pending.push((id, timer));
                continue;
            }
            fired.push(FiredTimer {
                queue_addr: timer.queue_addr,
                msg: timer.msg,
                armed_by: timer.armed_by,
            });
            if timer.interval > 0 {
                timer.deadline = now.saturating_add(timer.interval);
                still_pending.push((id, timer));
            }
            // interval == 0: one-shot, dropped after firing.
        }

        self.timers = still_pending;
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RdramAddr;

    fn addr(n: u32) -> RdramAddr {
        RdramAddr::from_offset(n)
    }

    #[test]
    fn one_shot_timer_fires_once() {
        let mut wheel = TimerWheel::default();
        wheel.set_timer(0, 10, 0, addr(0x100), 7, 1);

        assert!(wheel.advance(5).is_empty());
        let fired = wheel.advance(10);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].msg, 7);

        // Does not fire again even much later.
        assert!(wheel.advance(1000).is_empty());
    }

    #[test]
    fn interval_timer_reposts_repeatedly() {
        let mut wheel = TimerWheel::default();
        wheel.set_timer(0, 10, 10, addr(0x200), 1, 1);

        assert_eq!(wheel.advance(10).len(), 1);
        assert_eq!(wheel.advance(15).len(), 0);
        assert_eq!(wheel.advance(20).len(), 1);
        assert_eq!(wheel.advance(30).len(), 1);
    }

    #[test]
    fn stop_timer_prevents_future_fires() {
        let mut wheel = TimerWheel::default();
        let id = wheel.set_timer(0, 10, 10, addr(0x300), 1, 1);
        assert_eq!(wheel.advance(10).len(), 1);
        wheel.stop_timer(id);
        assert!(wheel.advance(20).is_empty());
    }

    #[test]
    fn timers_fire_in_deadline_order() {
        let mut wheel = TimerWheel::default();
        wheel.set_timer(0, 20, 0, addr(0x1), 20, 1);
        wheel.set_timer(0, 5, 0, addr(0x2), 5, 1);
        wheel.set_timer(0, 10, 0, addr(0x3), 10, 1);

        let fired = wheel.advance(20);
        let order: Vec<_> = fired.iter().map(|f| f.msg).collect();
        assert_eq!(order, vec![5, 10, 20]);
    }

    #[test]
    fn evidence_snapshot_preserves_stable_tie_order_without_mutating_wheel() {
        let mut wheel = TimerWheel::default();
        wheel.set_timer(0, 20, 0, addr(0x20), 0x20, 2);
        wheel.set_timer(0, 10, 0, addr(0x11), 0x11, 1);
        wheel.set_timer(0, 10, 0, addr(0x12), 0x12, 1);

        let before = wheel.evidence_snapshot();
        assert_eq!(before.next_id, 3);
        assert_eq!(
            before
                .firing_order
                .iter()
                .map(|timer| timer.msg)
                .collect::<Vec<_>>(),
            vec![0x11, 0x12, 0x20]
        );
        assert_eq!(wheel.evidence_snapshot(), before);

        let fired = wheel.advance(20);
        assert_eq!(
            fired.iter().map(|timer| timer.msg).collect::<Vec<_>>(),
            vec![0x11, 0x12, 0x20]
        );
    }

    #[test]
    fn evidence_snapshot_distinguishes_equal_deadline_insertion_order() {
        let mut first = TimerWheel::default();
        first.set_timer(0, 10, 0, addr(1), 1, 1);
        first.set_timer(0, 10, 0, addr(2), 2, 1);

        let mut reversed = TimerWheel::default();
        reversed.set_timer(0, 10, 0, addr(2), 2, 1);
        reversed.set_timer(0, 10, 0, addr(1), 1, 1);

        assert_ne!(first.evidence_snapshot(), reversed.evidence_snapshot());
    }
}
