//! Bounded window timing samples for the live shell's R5 feedback loop.

use std::collections::VecDeque;
use std::time::Duration;

const MAX_SAMPLES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainDecision {
    Step,
    Quiescent,
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
            })
        );
        assert_eq!(window.take_stats(), None);
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
}
