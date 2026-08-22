//! Bounded window timing samples for the live shell's R5 feedback loop.

use std::collections::VecDeque;
use std::time::Duration;

const MAX_SAMPLES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainDecision {
    Step,
    Quiescent,
}

/// Select an exact device deadline that a quiescent retrace pump must service
/// before returning, without consuming the following VI edge.
///
/// `fn64_abi::next_device_deadline` exists for this boundary: raw FullSync and
/// other mid-slice devices can schedule completion one cycle after the guest
/// blocks, while the next VI edge is still a full field away. The shell must
/// advance to that sub-field event and resume the newly-runnable guest in the
/// same pump. A deadline at the VI edge belongs to the next pump.
pub fn subfield_device_deadline(
    current: u64,
    next_device: Option<u64>,
    next_vi: u64,
) -> Option<u64> {
    next_device
        .filter(|deadline| *deadline < next_vi)
        .map(|deadline| deadline.max(current))
}

/// State for exactly one host-driven VI retrace. A framebuffer swap is
/// recorded in the outcome but cannot end the drain; only an empty run queue
/// or a second consecutive turn for the idle thread is quiescence.
pub struct RetraceDrain {
    start_swaps: u64,
    steps: u64,
    ran_idle_thread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetraceOutcome {
    pub swapped: bool,
    pub steps: u64,
}

impl RetraceDrain {
    pub fn new(start_swaps: u64) -> Self {
        Self {
            start_swaps,
            steps: 0,
            ran_idle_thread: false,
        }
    }

    pub fn before_step(&self, next_priority: Option<fn64_runtime::Priority>) -> DrainDecision {
        match next_priority {
            None => DrainDecision::Quiescent,
            Some(fn64_runtime::OS_PRIORITY_IDLE) if self.ran_idle_thread => {
                DrainDecision::Quiescent
            }
            Some(_) => DrainDecision::Step,
        }
    }

    pub fn record_step(&mut self, priority: fn64_runtime::Priority) {
        self.steps += 1;
        if priority == fn64_runtime::OS_PRIORITY_IDLE {
            self.ran_idle_thread = true;
        }
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn finish(self, current_swaps: u64) -> RetraceOutcome {
        RetraceOutcome {
            swapped: current_swaps > self.start_swaps,
            steps: self.steps,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimingStats {
    pub samples: usize,
    pub median_ms: f64,
    pub p95_ms: f64,
    /// The tail percentile and the outright worst sample in the window.
    ///
    /// "Guaranteed 60fps" is a bound on the WORST frame, not on the average
    /// one: a run whose median is 8 ms and whose max is 45 ms misses the bar
    /// on every frame that took 45 ms, and neither the median nor p95 shows
    /// it. `max_ms` is the only statistic here that can falsify the claim,
    /// and `p99_ms` says whether a breach is a lone outlier or routine.
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl TimingStats {
    /// Whether every sample in this window fit inside one 60 Hz frame
    /// (16.667 ms). Worst-case, per the acceptance bar -- not the mean.
    pub fn holds_60fps(&self) -> bool {
        self.max_ms <= 1000.0 / 60.0
    }
}

#[derive(Default)]
pub struct TimingWindow {
    samples: VecDeque<Duration>,
}

impl TimingWindow {
    pub fn record(&mut self, duration: Duration) {
        if self.samples.len() == MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(duration);
    }

    /// Summarize and clear the current observation window. Nearest-rank
    /// percentiles keep the result deterministic and make a single slow frame
    /// visible in p95 once the window contains at least 20 samples.
    pub fn take_stats(&mut self) -> Option<TimingStats> {
        if self.samples.is_empty() {
            return None;
        }
        let mut milliseconds: Vec<f64> = self
            .samples
            .drain(..)
            .map(|duration| duration.as_secs_f64() * 1000.0)
            .collect();
        milliseconds.sort_by(f64::total_cmp);
        Some(TimingStats {
            samples: milliseconds.len(),
            median_ms: nearest_rank(&milliseconds, 50),
            p95_ms: nearest_rank(&milliseconds, 95),
            p99_ms: nearest_rank(&milliseconds, 99),
            max_ms: *milliseconds
                .last()
                .expect("the empty window returned early above"),
        })
    }
}

fn nearest_rank(sorted: &[f64], percentile: usize) -> f64 {
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_nearest_rank_median_and_p95_then_clears() {
        let mut window = TimingWindow::default();
        for milliseconds in 1..=20 {
            window.record(Duration::from_millis(milliseconds));
        }

        assert_eq!(
            window.take_stats(),
            Some(TimingStats {
                samples: 20,
                median_ms: 10.0,
                p95_ms: 19.0,
                p99_ms: 20.0,
                max_ms: 20.0,
            })
        );
        assert_eq!(window.take_stats(), None);
    }

    /// The worst-case bar cannot be read off the median or p95: this window is
    /// comfortably fast by both and still misses 60fps on one frame in fifty.
    #[test]
    fn a_single_long_frame_fails_the_60fps_bound_while_median_and_p95_stay_fast()
    {
        let mut window = TimingWindow::default();
        for _ in 0..49 {
            window.record(Duration::from_millis(8));
        }
        window.record(Duration::from_millis(45));

        let stats = window.take_stats().expect("50 samples recorded");
        assert_eq!(stats.median_ms, 8.0);
        assert_eq!(stats.p95_ms, 8.0, "p95 cannot see one frame in fifty");
        assert_eq!(stats.max_ms, 45.0);
        assert!(
            !stats.holds_60fps(),
            "a 45 ms frame misses the worst-case 60fps bar"
        );
    }

    #[test]
    fn a_window_entirely_inside_one_field_holds_the_bound() {
        let mut window = TimingWindow::default();
        for _ in 0..50 {
            window.record(Duration::from_micros(16_000));
        }

        let stats = window.take_stats().expect("50 samples recorded");
        assert_eq!(stats.max_ms, 16.0);
        assert!(stats.holds_60fps(), "16.0 ms fits inside a 16.667 ms field");
    }

    #[test]
    fn remains_bounded_to_the_latest_samples() {
        let mut window = TimingWindow::default();
        for milliseconds in 0..=MAX_SAMPLES {
            window.record(Duration::from_millis(milliseconds as u64));
        }

        let stats = window.take_stats().unwrap();
        assert_eq!(stats.samples, MAX_SAMPLES);
        assert_eq!(stats.median_ms, 300.0);
        assert_eq!(stats.p95_ms, 570.0);
        assert_eq!(stats.p99_ms, 594.0);
        assert_eq!(
            stats.max_ms, 600.0,
            "the oldest sample is evicted, so the window's max is the newest"
        );
    }

    #[test]
    fn framebuffer_swap_is_observed_without_ending_the_retrace_drain() {
        let mut drain = RetraceDrain::new(10);

        assert_eq!(drain.before_step(Some(20)), DrainDecision::Step);
        drain.record_step(20); // this step produced swap 11

        assert_eq!(
            drain.before_step(Some(10)),
            DrainDecision::Step,
            "same-retrace work after a swap must run before time advances"
        );
        drain.record_step(10);
        assert_eq!(drain.before_step(Some(0)), DrainDecision::Step);
        drain.record_step(0);
        assert_eq!(drain.before_step(Some(0)), DrainDecision::Quiescent);

        assert_eq!(
            drain.finish(11),
            RetraceOutcome {
                swapped: true,
                steps: 3,
            }
        );
    }

    /// The live raw-DPC path schedules FullSync completion one cycle after
    /// synchronous renderer publication. Rounding that deadline up to the next
    /// VI pump inserts an entire field between DP completion and the scheduler
    /// wake, turning WM2000's two-field frame into the measured three-pump
    /// 3.5/4.4/33 ms rhythm.
    #[test]
    fn a_quiescent_pump_services_full_sync_before_the_next_vi_edge() {
        let current = 10_000;
        let next_vi = 20_000;

        assert_eq!(
            subfield_device_deadline(current, Some(current + 1), next_vi),
            Some(current + 1),
            "the one-cycle DP completion must remain in this retrace pump"
        );
        assert_eq!(
            subfield_device_deadline(current, Some(next_vi), next_vi),
            None,
            "the following VI edge starts the next retrace pump"
        );
    }
}
