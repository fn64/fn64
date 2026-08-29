//! Bounded window timing samples for the live shell's R5 feedback loop.

use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::time::Duration;

const MAX_SAMPLES: usize = 600;

/// One exact rendered cue observed only after its host presentation succeeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoSyncLandmark {
    pub rgba_hash: u64,
    pub occurrence: NonZeroU64,
    pub stage: fn64_abi::PresentedViFieldStage,
    pub presentation_generation: u64,
    pub swap_count: u64,
    pub retrace_at: fn64_runtime::EmulatedInstant,
    pub presented_at: std::time::Instant,
}

/// Configurable video half of the A/V synchronization probe.
///
/// Repeated images are normal during a 30 Hz game on a 60 Hz VI. Selecting an
/// explicit one-based occurrence makes the cue identity stable without
/// embedding knowledge of any title in the shell.
pub struct VideoSyncProbe {
    target_hash: u64,
    target_occurrence: NonZeroU64,
    matching_fields: u64,
    settled: bool,
}

impl VideoSyncProbe {
    pub fn from_env() -> Option<Self> {
        let Some(raw_hash) = std::env::var_os("FN64_AV_SYNC_VIDEO_HASH") else {
            assert!(
                std::env::var_os("FN64_AV_SYNC_VIDEO_OCCURRENCE").is_none(),
                "FN64_AV_SYNC_VIDEO_OCCURRENCE requires FN64_AV_SYNC_VIDEO_HASH"
            );
            return None;
        };
        let raw_hash = raw_hash
            .to_str()
            .expect("FN64_AV_SYNC_VIDEO_HASH must be UTF-8");
        let hexadecimal = raw_hash
            .strip_prefix("0x")
            .or_else(|| raw_hash.strip_prefix("0X"))
            .unwrap_or(raw_hash);
        let target_hash = u64::from_str_radix(hexadecimal, 16)
            .expect("FN64_AV_SYNC_VIDEO_HASH must be a hexadecimal u64");
        let target_occurrence = std::env::var("FN64_AV_SYNC_VIDEO_OCCURRENCE")
            .map(|raw| {
                raw.parse::<u64>()
                    .ok()
                    .and_then(NonZeroU64::new)
                    .expect("FN64_AV_SYNC_VIDEO_OCCURRENCE must be a nonzero decimal u64")
            })
            .unwrap_or(NonZeroU64::MIN);
        Some(Self::new(target_hash, target_occurrence))
    }

    pub const fn new(target_hash: u64, target_occurrence: NonZeroU64) -> Self {
        Self {
            target_hash,
            target_occurrence,
            matching_fields: 0,
            settled: false,
        }
    }

    /// Whether another successful presentation can still satisfy this probe.
    /// A settled probe retains its landmark but no longer demands a frame hash.
    pub const fn needs_hash(&self) -> bool {
        !self.settled
    }

    pub fn observe_successful_present(
        &mut self,
        rgba_hash: u64,
        stage: fn64_abi::PresentedViFieldStage,
        presentation_generation: u64,
        swap_count: u64,
        retrace_at: fn64_runtime::EmulatedInstant,
        presented_at: std::time::Instant,
    ) -> Option<VideoSyncLandmark> {
        if self.settled || rgba_hash != self.target_hash {
            return None;
        }
        self.matching_fields = self
            .matching_fields
            .checked_add(1)
            .expect("A/V video landmark occurrence overflow");
        if self.matching_fields != self.target_occurrence.get() {
            return None;
        }
        self.settled = true;
        Some(VideoSyncLandmark {
            rgba_hash,
            occurrence: self.target_occurrence,
            stage,
            presentation_generation,
            swap_count,
            retrace_at,
            presented_at,
        })
    }
}

/// Convert the device fabric's live VI field interval to the host deadline
/// used to inject its next interrupt. Both VI and AI derive from the same
/// typed television clock; rounding once at the wall-clock edge keeps that
/// authority instead of replacing it with a nominal host refresh constant.
pub fn vi_field_wall_duration(field_cycles: u64) -> Duration {
    emulated_duration_to_wall(fn64_runtime::Cycles::new(field_cycles))
}

fn emulated_duration_to_wall(cycles: fn64_runtime::Cycles) -> Duration {
    assert!(cycles.get() != 0, "emulated wall duration must be nonzero");
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    let numerator = u128::from(cycles.get()) * NANOS_PER_SECOND;
    let denominator = u128::from(fn64_runtime::CPU_CLOCK_HZ);
    let rounded = (numerator + denominator / 2) / denominator;
    Duration::from_nanos(
        u64::try_from(rounded).expect("VI wall duration exceeds std::time::Duration nanos"),
    )
}

/// Monotonic correspondence between the emulated master clock and host wall
/// time. Deadlines derive from one epoch, avoiding per-field rounding drift.
/// When host work falls at least one field behind, the epoch may move later;
/// it never moves earlier, so recovery cannot run guest fields faster than
/// the programmed hardware cadence.
#[derive(Clone, Copy, Debug)]
pub struct EmulatedWallClock {
    emulated_epoch: fn64_runtime::EmulatedInstant,
    wall_epoch: std::time::Instant,
}

impl EmulatedWallClock {
    pub const fn new(
        emulated_epoch: fn64_runtime::EmulatedInstant,
        wall_epoch: std::time::Instant,
    ) -> Self {
        Self {
            emulated_epoch,
            wall_epoch,
        }
    }

    pub fn deadline(self, target: fn64_runtime::EmulatedInstant) -> std::time::Instant {
        let elapsed = target
            .checked_duration_since(self.emulated_epoch)
            .unwrap_or_else(|| {
                panic!(
                    "emulated wall-clock target {} precedes epoch {}",
                    target, self.emulated_epoch
                )
            });
        if elapsed == fn64_runtime::Cycles::ZERO {
            return self.wall_epoch;
        }
        self.wall_epoch
            .checked_add(emulated_duration_to_wall(elapsed))
            .expect("emulated wall-clock deadline exceeds host Instant range")
    }

    /// Defer the wall epoch when `target` is at least `late_by` behind `now`.
    ///
    /// The target becomes due at `now`, and every later emulated deadline
    /// keeps its exact hardware-relative distance. This caps recovery at the
    /// emulated cadence without changing emulated time or consulting audio.
    pub fn defer_if_late(
        &mut self,
        target: fn64_runtime::EmulatedInstant,
        now: std::time::Instant,
        late_by: Duration,
    ) -> bool {
        assert!(!late_by.is_zero(), "wall-clock lateness threshold must be nonzero");
        let deadline = self.deadline(target);
        let Some(lateness) = now.checked_duration_since(deadline) else {
            return false;
        };
        if lateness < late_by {
            return false;
        }
        self.wall_epoch = self
            .wall_epoch
            .checked_add(lateness)
            .expect("deferred emulated wall epoch exceeds host Instant range");
        debug_assert_eq!(self.deadline(target), now);
        true
    }
}

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
    fn wall_pacing_uses_the_programmed_vi_field_not_a_nominal_sixty_hz_constant() {
        assert_eq!(
            vi_field_wall_duration(1_567_042),
            Duration::from_nanos(16_715_115)
        );
        assert_eq!(
            vi_field_wall_duration(fn64_runtime::TvType::Pal.nominal_field_cycles()),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn emulated_wall_clock_maps_absolute_cycles_without_per_field_drift() {
        let wall = std::time::Instant::now();
        let clock = EmulatedWallClock::new(fn64_runtime::EmulatedInstant::new(10), wall);

        assert_eq!(
            clock.deadline(fn64_runtime::EmulatedInstant::new(10)),
            wall
        );
        assert_eq!(
            clock
                .deadline(fn64_runtime::EmulatedInstant::new(13))
                .duration_since(wall),
            Duration::from_nanos(32),
            "three master cycles are rounded once from the fixed epoch"
        );
        assert_eq!(
            clock
                .deadline(fn64_runtime::EmulatedInstant::new(
                    10 + fn64_runtime::CPU_CLOCK_HZ,
                ))
                .duration_since(wall),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn one_field_lateness_defers_future_deadlines_without_catch_up() {
        let wall = std::time::Instant::now();
        let field_cycles = fn64_runtime::Cycles::new(1_567_042);
        let field_wall = vi_field_wall_duration(field_cycles.get());
        let first = fn64_runtime::EmulatedInstant::new(field_cycles.get());
        let second = first
            .checked_add(field_cycles)
            .expect("two test VI fields fit in the emulated clock");
        let mut clock = EmulatedWallClock::new(fn64_runtime::EmulatedInstant::ZERO, wall);
        let original_spacing = clock
            .deadline(second)
            .duration_since(clock.deadline(first));
        let stalled = wall + Duration::from_millis(140);

        assert!(clock.defer_if_late(first, stalled, field_wall));
        assert_eq!(clock.deadline(first), stalled);
        assert_eq!(clock.deadline(second).duration_since(stalled), original_spacing);
    }

    #[test]
    fn subfield_jitter_cannot_rewrite_the_wall_epoch() {
        let wall = std::time::Instant::now();
        let field_cycles = fn64_runtime::Cycles::new(1_567_042);
        let field_wall = vi_field_wall_duration(field_cycles.get());
        let first = fn64_runtime::EmulatedInstant::new(field_cycles.get());
        let mut clock = EmulatedWallClock::new(fn64_runtime::EmulatedInstant::ZERO, wall);
        let original = clock.deadline(first);

        assert!(!clock.defer_if_late(
            first,
            original + field_wall - Duration::from_nanos(1),
            field_wall,
        ));
        assert_eq!(clock.deadline(first), original);
    }

    #[test]
    fn an_on_time_edge_cannot_rewrite_the_wall_epoch() {
        let wall = std::time::Instant::now();
        let field_cycles = fn64_runtime::Cycles::new(1_567_042);
        let field_wall = vi_field_wall_duration(field_cycles.get());
        let first = fn64_runtime::EmulatedInstant::new(field_cycles.get());
        let mut clock = EmulatedWallClock::new(fn64_runtime::EmulatedInstant::ZERO, wall);
        let original = clock.deadline(first);

        assert!(!clock.defer_if_late(first, original, field_wall));
        assert_eq!(clock.deadline(first), original);
    }

    #[test]
    fn repeated_deferrals_only_move_future_deadlines_later() {
        let wall = std::time::Instant::now();
        let field_cycles = fn64_runtime::Cycles::new(1_567_042);
        let field_wall = vi_field_wall_duration(field_cycles.get());
        let first = fn64_runtime::EmulatedInstant::new(field_cycles.get());
        let second = first
            .checked_add(field_cycles)
            .expect("two test VI fields fit in the emulated clock");
        let third = second
            .checked_add(field_cycles)
            .expect("three test VI fields fit in the emulated clock");
        let mut clock = EmulatedWallClock::new(fn64_runtime::EmulatedInstant::ZERO, wall);

        let first_observed = clock.deadline(first) + Duration::from_millis(80);
        assert!(clock.defer_if_late(first, first_observed, field_wall));
        let third_after_first = clock.deadline(third);

        let second_observed = clock.deadline(second) + Duration::from_millis(40);
        assert!(clock.defer_if_late(second, second_observed, field_wall));
        assert_eq!(clock.deadline(second), second_observed);
        assert!(clock.deadline(third) > third_after_first);
    }

    #[test]
    #[should_panic(expected = "precedes epoch")]
    fn emulated_wall_clock_rejects_regressing_targets() {
        EmulatedWallClock::new(
            fn64_runtime::EmulatedInstant::new(10),
            std::time::Instant::now(),
        )
        .deadline(fn64_runtime::EmulatedInstant::new(9));
    }

    #[test]
    fn video_sync_probe_binds_the_selected_repeat_to_its_vi_edge_and_successful_present() {
        let mut probe = VideoSyncProbe::new(0x1234, NonZeroU64::new(2).unwrap());
        assert!(probe.needs_hash());
        let wall = std::time::Instant::now();
        assert_eq!(
            probe.observe_successful_present(
                0x1234,
                fn64_abi::PresentedViFieldStage::PostVi,
                7,
                9,
                fn64_runtime::EmulatedInstant::new(100),
                wall,
            ),
            None,
            "the first repeated field is not the selected occurrence"
        );
        assert_eq!(
            probe.observe_successful_present(
                0xbeef,
                fn64_abi::PresentedViFieldStage::PostVi,
                8,
                10,
                fn64_runtime::EmulatedInstant::new(200),
                wall + Duration::from_millis(1),
            ),
            None,
            "unrelated frames do not advance the occurrence"
        );
        let selected_wall = wall + Duration::from_millis(2);
        assert_eq!(
            probe.observe_successful_present(
                0x1234,
                fn64_abi::PresentedViFieldStage::PostVi,
                9,
                11,
                fn64_runtime::EmulatedInstant::new(300),
                selected_wall,
            ),
            Some(VideoSyncLandmark {
                rgba_hash: 0x1234,
                occurrence: NonZeroU64::new(2).unwrap(),
                stage: fn64_abi::PresentedViFieldStage::PostVi,
                presentation_generation: 9,
                swap_count: 11,
                retrace_at: fn64_runtime::EmulatedInstant::new(300),
                presented_at: selected_wall,
            })
        );
        assert!(!probe.needs_hash());
        assert_eq!(
            probe.observe_successful_present(
                0x1234,
                fn64_abi::PresentedViFieldStage::PostVi,
                10,
                12,
                fn64_runtime::EmulatedInstant::new(400),
                wall + Duration::from_millis(3),
            ),
            None,
            "a settled cue cannot be rebound to a later field"
        );
    }

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
