//! Per-VI-field wall-clock latency census: the measurement the "guaranteed
//! 60fps" bar needs and did not have.
//!
//! # Why this exists
//!
//! The project's acceptance bar is stated as a WORST-CASE bound: every frame
//! under 16.667 ms. As of 2026-08-07 that bar had **no test**. The standard
//! benchmark route (`FN64_BLOCK_MAX_STEPS=19523`) reports `gfx_submits=0` and
//! eight VI interrupts -- it renders nothing, so a p99 frame time cannot be
//! measured on it at all (`docs/plans/perf-method.md:148`). The windowed shell
//! reports `p50/p95/p99/max` per 60-frame heartbeat (`2676139`), but it clears
//! its window at each report, so no aggregate over a chosen region exists, and
//! the shell binary is not always buildable while the shard topology is in
//! flight.
//!
//! # The two ratios, which are not the same number
//!
//! Quoting either alone misleads, and doing so is a documented trap. This
//! census always reports both:
//!
//! - **wall ms per emulated VI field** -- host wall time divided by fields the
//!   guest actually produced. This is the number the 60fps bar tests, because
//!   a field is what a player sees. **16.667 ms is the bar.**
//! - **wall-versus-virtual** -- host wall time divided by the guest virtual
//!   time (`sim_time`, in 93.75 MHz CPU cycles) that elapsed across the same
//!   span. This is the "how many times slower than the console" figure. **1.0x
//!   is the bar.**
//!
//! They diverge whenever the guest does not produce fields at its nominal
//! rate. WM2000 during boot emits fields at roughly 27 Hz, not 60 Hz, so the
//! same run reads as ~2.4x hardware on the first ratio and ~1.1x on the
//! second. Neither number is wrong; quoting one as if it were the other is.
//!
//! # The steady-state window
//!
//! Boot and first-render are a transient. A p99 that includes them measures
//! startup, not gameplay -- the first field of a route can be hundreds of
//! milliseconds while overlays activate and shards fault in, and one such
//! sample dominates `max` forever. `FN64_FRAME_CENSUS_WARMUP_GFX=<n>` discards
//! every field observed before the guest has submitted `n` graphics tasks, so
//! the reported distribution covers sustained rendering only. Fields before
//! the boundary are still COUNTED and reported separately, because "the
//! transient was N fields and M ms" is itself evidence, and silently dropping
//! samples is how a benchmark becomes a lie.
//!
//! Graphics submits are the right gate rather than a step count or a field
//! count: they are the direct evidence that the guest is rendering, and the
//! transient ends precisely when they begin climbing at a steady rate.
//!
//! # One advance is not always one frame
//!
//! `GuestDrain::before_step` cannot advance virtual time while the guest has
//! runnable work, so a single `advance_virtual_time` can commit many overdue VI
//! fields at once. Charging such a span to one "frame" reports a frame time
//! nobody experienced -- a measured 3,028 ms advance turned out to be 22 fields
//! at 137 ms each, during which the guest was running FASTER than its average
//! (`8690d36`). Every latency figure here is therefore PER FIELD, budget
//! breaches are counted per field, and the worst raw advance is reported
//! alongside its field count so a genuine one-field stall is still visible.
//!
//! # Which lane
//!
//! This hooks [`crate::advance_virtual_time`], the single seam where the host
//! commits guest virtual time and the only place a VI retrace is observed. Both
//! the headless batch lane and the windowed shell drive through it, so the
//! census is lane-agnostic and measures **guest + runtime** cost: the emulation
//! itself, with no present or window-system cost included. A windowed run also
//! pays blit and swap, which this deliberately excludes -- see the module docs
//! on `examples/wm2000-block-boot/src/shell.rs` for the lane that includes it.
//! A number from here must not be reported as a player-experienced frame time.
//!
//! # Cost when off
//!
//! `FN64_FRAME_CENSUS=1` arms it; absent, empty, `0`, or any other spelling is
//! off, on the same reasoning as [`crate::write_barrier`]'s gate (a diagnostic
//! whose off lane is silently the on lane is worse than no diagnostic). When
//! off the per-advance cost is one relaxed atomic load and a branch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The 60fps acceptance bar, in milliseconds. One NTSC field.
pub const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;

/// Cap on retained per-field samples. A 1.5M-step route produces a few
/// thousand fields, so this holds a whole run; beyond it the census keeps the
/// FIRST `MAX_SAMPLES` steady-state fields rather than a sliding window,
/// because a truncated-but-contiguous span is summarizable and a silently
/// resampled one is not. Truncation is reported.
const MAX_SAMPLES: usize = 200_000;

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Whether the census is armed. Read on every `advance_virtual_time`, so it is
/// a plain atomic rather than a `OnceLock` deref.
static ARMED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_FRAME_CENSUS"))
}

/// Graphics submits that must accumulate before a field counts toward the
/// steady-state distribution. 0 means "measure from the first field", which
/// includes the boot transient and is almost never what you want.
fn warmup_gfx() -> u64 {
    static WARMUP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *WARMUP.get_or_init(|| {
        std::env::var("FN64_FRAME_CENSUS_WARMUP_GFX")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(0)
    })
}

/// One observed advance: how long the host took, how much guest virtual time
/// it covered, and HOW MANY VI FIELDS IT COMMITTED.
///
/// The field count is not bookkeeping -- it is the correction that makes this
/// measurement honest, and it was established by counter rather than
/// hypothesis (`8690d36`, "the 3-second frame is 22 VI fields, not a stall").
///
/// `GuestDrain::before_step` cannot advance virtual time while the guest has
/// runnable work. A menu transition that stays runnable for 19,112 scheduling
/// steps therefore reaches NO field boundary until it quiesces -- and then one
/// `advance_virtual_time` commits all 22 overdue fields at once. Charging that
/// whole 3,028 ms span to a single "frame" reports a three-second frame that
/// nobody experienced: the guest's clock advanced 22 fields, no time was lost,
/// and the player sees no freeze. That run's guest was in fact running FASTER
/// than average during the span (158 us/step against a 380 us/step mean).
///
/// So `wall_ms / fields` is the per-field latency, and the raw `wall_ms` is
/// the per-advance latency. The distribution is taken over the former; the
/// latter is reported separately so a real host stall (fields=1, huge
/// `wall_ms`) stays visible instead of being averaged away.
#[derive(Clone, Copy)]
struct FieldSample {
    /// Wall time for the whole advance, covering `fields` VI fields.
    wall_ms: f64,
    /// VI fields this one advance committed. Usually 1; larger after a span
    /// where the guest stayed runnable across several field deadlines.
    fields: u32,
    /// Guest CPU cycles (93.75 MHz) committed across this advance.
    virtual_cycles: u64,
    /// Graphics tasks the guest had submitted when the advance closed.
    gfx_submits: u64,
    /// Cumulative work counters at the close of this advance, for the
    /// population diff. See [`Counters`].
    counters: Counters,
}

/// Cumulative host-side work counters, sampled at each field boundary.
///
/// # Why cumulative rather than per-field
///
/// Every underlying counter is a running total that only grows. Sampling the
/// total at each boundary and differencing adjacent samples is the only
/// reduction that cannot double-count, and it needs no reset -- a reset would
/// race with any work in flight and would make a dropped sample corrupt every
/// later field rather than just its own.
///
/// # Why this is the deliverable rather than the latency split
///
/// The distribution being bimodal (p50 fits the 16.667 ms budget, p95 is
/// 2.34x it, ~50/50, almost nothing between) is already known from the
/// latency numbers alone. What is NOT known is whether the two populations
/// are doing DIFFERENT WORK. If the slow fields carry more graphics submits,
/// the story is scene complexity or submit batching. If they carry the same
/// work but more journal boundaries or faults, it is the correctness
/// apparatus. If NO counter differs, the cost is somewhere no counter looks,
/// which is a major finding in its own right and must be reported as such
/// rather than dressed up as a mechanism (perf-method rule 3: count, do not
/// infer).
///
/// # Cost
///
/// Reading these is roughly a dozen relaxed atomic loads plus one
/// `with_executor` borrow at each field boundary -- ~60 per second of
/// emulated time, against a ~16-40 ms field. It happens only when the census
/// is armed. Counters whose own gate is off read a constant zero; the report
/// says so explicitly rather than reporting a zero difference as "no
/// difference between the buckets", which would be exactly the check-that-
/// cannot-fail error perf-method rule 6a is about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counters {
    /// Graphics tasks admitted (`Executor::task_log`). Always available.
    gfx_tasks: u64,
    /// Audio tasks admitted. Always available.
    audio_tasks: u64,
    /// Phase-timer nanoseconds, from `FN64_PHASE_TIMING`. Zero when unset.
    executor_ns: u64,
    gfx_ns: u64,
    gfx_lle_ns: u64,
    gfx_lle_rsp_ns: u64,
    gfx_lle_rdp_ns: u64,
    vi_present_ns: u64,
    audio_lle_ns: u64,
    /// The `executor_ns` split, from `FN64_EXECUTOR_SPLIT`. Zero when unset.
    ///
    /// `executor_ns` is 61% of a WM2000 render field and had no sub-counters,
    /// which is why two-thirds of that field went unnamed for a whole session.
    /// See `PhaseTiming`'s nesting diagram: `exec_mirror_ns` and
    /// `exec_guard_suspend_ns` are INSIDE `exec_resume_ns`, so the report must
    /// subtract rather than sum.
    exec_mirror_ns: u64,
    exec_resume_ns: u64,
    exec_devtime_ns: u64,
    exec_guard_suspend_ns: u64,
    exec_guard_device_ns: u64,
    exec_mirror_calls: u64,
    exec_guard_suspend_calls: u64,
    exec_guard_device_calls: u64,
    /// The `resume NET` split, from `FN64_RESUME_SPLIT`. Zero when unset.
    ///
    /// `resume NET` is 83.2% of a WM2000 render field and had no sub-counters
    /// -- the same position `executor_ns` was in one level up. These name it.
    /// `resume_dispatch_ns` is INCLUSIVE of `gfx_ns` and `audio_lle_ns`, and
    /// `resume_suspend_ns` is inclusive of `exec_guard_suspend_ns`, so the
    /// report subtracts rather than sums (the report is the only place that
    /// knows the nesting; see `resume_split_report`).
    resume_reconcile_ns: u64,
    resume_cop0_ns: u64,
    resume_dispatch_ns: u64,
    resume_invalidate_ns: u64,
    resume_exit_ns: u64,
    resume_suspend_ns: u64,
    resume_resolve_ns: u64,
    resume_hostcall_ns: u64,
    resume_hostcall_calls: u64,
    resume_reconcile_calls: u64,
    resume_dispatch_calls: u64,
    /// VI-presentation reachability, ungated. Counts which side of the
    /// executor seam each presentation ran on, so "presentation is outside
    /// `executor_ns`" is settled by observation rather than by call-graph
    /// inference. `vi_present_in_executor_calls` is expected to be ZERO.
    vi_present_in_executor_calls: u64,
    vi_present_outside_executor_calls: u64,
    /// Phase-timer call counts, from `FN64_PHASE_TIMING`. Zero when unset.
    executor_calls: u64,
    gfx_calls: u64,
    gfx_lle_calls: u64,
    audio_lle_calls: u64,
    /// RSP interpreter instructions retired, from `FN64_DPC_COPY_CENSUS`.
    /// Zero when unset.
    rsp_steps_gfx: u64,
    rsp_steps_audio: u64,
    rsp_entries: u64,
    /// `dispatch_captured_raw_rdp` entries, from `FN64_DPC_COPY_CENSUS`.
    dpc_calls: u64,
    /// The staging-copy phases, from `FN64_DPC_COPY_CENSUS`. Nested under the
    /// RDP seam. Sampled per field because the staging memcpy is an
    /// optimization target and a run total cannot say which population pays
    /// it -- nor whether it is 0.11x the budget or 1.1x.
    dpc_alloc_ns: u64,
    dpc_copy_in_ns: u64,
    dpc_copy_back_ns: u64,
    /// Mutation-journal boundaries, from `FN64_MPROTECT_BARRIER_STATS`.
    /// Zero when unset.
    barrier_served: u64,
    barrier_fell_back: u64,
    barrier_dirty_pages: u64,
    barrier_clean: u64,
}

impl Counters {
    /// Read every counter's running total. Called once per field boundary
    /// when the census is armed.
    fn sample() -> Self {
        let (gfx_tasks, audio_tasks) = crate::task_counts();
        let phase = crate::phase_timing();
        let (rsp_steps_gfx, rsp_steps_audio, rsp_entries, dpc_calls) =
            crate::dpc_copy_census::running_totals();
        let (dpc_alloc_ns, dpc_copy_in_ns, dpc_copy_back_ns) =
            crate::dpc_copy_census::staging_totals();
        // The barrier lives behind `recomp-rs`. Without that feature there is
        // no journal to count boundaries for, so zeros are the true answer and
        // the report's NO DATA line is the correct rendering of them.
        #[cfg(feature = "recomp-rs")]
        let (barrier_served, barrier_fell_back, barrier_dirty_pages, barrier_clean) =
            crate::write_barrier::guard::stats::running_totals();
        #[cfg(not(feature = "recomp-rs"))]
        let (barrier_served, barrier_fell_back, barrier_dirty_pages, barrier_clean) =
            (0, 0, 0, 0);
        Self {
            gfx_tasks,
            audio_tasks,
            executor_ns: phase.executor_ns,
            gfx_ns: phase.gfx_ns,
            gfx_lle_ns: phase.gfx_lle_ns,
            gfx_lle_rsp_ns: phase.gfx_lle_rsp_ns,
            gfx_lle_rdp_ns: phase.gfx_lle_rdp_ns,
            vi_present_ns: phase.vi_present_ns,
            audio_lle_ns: phase.audio_lle_ns,
            exec_mirror_ns: phase.exec_mirror_ns,
            exec_resume_ns: phase.exec_resume_ns,
            exec_devtime_ns: phase.exec_devtime_ns,
            exec_guard_suspend_ns: phase.exec_guard_suspend_ns,
            exec_guard_device_ns: phase.exec_guard_device_ns,
            exec_mirror_calls: phase.exec_mirror_calls,
            exec_guard_suspend_calls: phase.exec_guard_suspend_calls,
            exec_guard_device_calls: phase.exec_guard_device_calls,
            resume_reconcile_ns: phase.resume_reconcile_ns,
            resume_cop0_ns: phase.resume_cop0_ns,
            resume_dispatch_ns: phase.resume_dispatch_ns,
            resume_invalidate_ns: phase.resume_invalidate_ns,
            resume_exit_ns: phase.resume_exit_ns,
            resume_suspend_ns: phase.resume_suspend_ns,
            resume_resolve_ns: phase.resume_resolve_ns,
            resume_hostcall_ns: phase.resume_hostcall_ns,
            resume_hostcall_calls: phase.resume_hostcall_calls,
            resume_reconcile_calls: phase.resume_reconcile_calls,
            resume_dispatch_calls: phase.resume_dispatch_calls,
            vi_present_in_executor_calls: phase.vi_present_in_executor_calls,
            vi_present_outside_executor_calls: phase.vi_present_outside_executor_calls,
            executor_calls: phase.executor_calls,
            gfx_calls: phase.gfx_calls,
            gfx_lle_calls: phase.gfx_lle_calls,
            audio_lle_calls: phase.audio_lle_calls,
            rsp_steps_gfx,
            rsp_steps_audio,
            rsp_entries,
            dpc_calls,
            dpc_alloc_ns,
            dpc_copy_in_ns,
            dpc_copy_back_ns,
            barrier_served,
            barrier_fell_back,
            barrier_dirty_pages,
            barrier_clean,
        }
    }

    /// Work done between two boundaries: `self - earlier`, saturating.
    ///
    /// Saturating rather than wrapping because a decrease is impossible for a
    /// monotone counter, so an apparent one is a bug elsewhere and must read
    /// as zero rather than as `u64::MAX` work.
    fn delta(&self, earlier: &Self) -> Self {
        macro_rules! sub {
            ($($field:ident),* $(,)?) => {
                Self { $($field: self.$field.saturating_sub(earlier.$field),)* }
            };
        }
        sub!(
            gfx_tasks,
            audio_tasks,
            executor_ns,
            gfx_ns,
            gfx_lle_ns,
            gfx_lle_rsp_ns,
            gfx_lle_rdp_ns,
            vi_present_ns,
            audio_lle_ns,
            exec_mirror_ns,
            exec_resume_ns,
            exec_devtime_ns,
            exec_guard_suspend_ns,
            exec_guard_device_ns,
            exec_mirror_calls,
            exec_guard_suspend_calls,
            exec_guard_device_calls,
            resume_reconcile_ns,
            resume_cop0_ns,
            resume_dispatch_ns,
            resume_invalidate_ns,
            resume_exit_ns,
            resume_suspend_ns,
            resume_resolve_ns,
            resume_hostcall_ns,
            resume_hostcall_calls,
            resume_reconcile_calls,
            resume_dispatch_calls,
            vi_present_in_executor_calls,
            vi_present_outside_executor_calls,
            executor_calls,
            gfx_calls,
            gfx_lle_calls,
            audio_lle_calls,
            rsp_steps_gfx,
            rsp_steps_audio,
            rsp_entries,
            dpc_calls,
            dpc_alloc_ns,
            dpc_copy_in_ns,
            dpc_copy_back_ns,
            barrier_served,
            barrier_fell_back,
            barrier_dirty_pages,
            barrier_clean,
        )
    }

    /// Every counter as `(label, value)`, in report order. One list so the
    /// bucket diff cannot silently omit a counter the sampler collects.
    fn labelled(&self) -> [(&'static str, u64); 45] {
        [
            ("gfx_tasks", self.gfx_tasks),
            ("audio_tasks", self.audio_tasks),
            ("executor_ns", self.executor_ns),
            ("gfx_ns", self.gfx_ns),
            ("gfx_lle_ns", self.gfx_lle_ns),
            ("gfx_lle_rsp_ns", self.gfx_lle_rsp_ns),
            ("gfx_lle_rdp_ns", self.gfx_lle_rdp_ns),
            ("vi_present_ns", self.vi_present_ns),
            ("audio_lle_ns", self.audio_lle_ns),
            ("executor_calls", self.executor_calls),
            ("gfx_calls", self.gfx_calls),
            ("gfx_lle_calls", self.gfx_lle_calls),
            ("audio_lle_calls", self.audio_lle_calls),
            ("rsp_steps_gfx", self.rsp_steps_gfx),
            ("rsp_steps_audio", self.rsp_steps_audio),
            ("rsp_entries", self.rsp_entries),
            ("dpc_calls", self.dpc_calls),
            ("dpc_alloc_ns", self.dpc_alloc_ns),
            ("dpc_copy_in_ns", self.dpc_copy_in_ns),
            ("dpc_copy_back_ns", self.dpc_copy_back_ns),
            ("barrier_served", self.barrier_served),
            ("barrier_fell_back", self.barrier_fell_back),
            ("barrier_dirty_pages", self.barrier_dirty_pages),
            ("barrier_clean", self.barrier_clean),
            ("exec_mirror_ns", self.exec_mirror_ns),
            ("exec_resume_ns", self.exec_resume_ns),
            ("exec_devtime_ns", self.exec_devtime_ns),
            ("exec_guard_suspend_ns", self.exec_guard_suspend_ns),
            ("exec_guard_device_ns", self.exec_guard_device_ns),
            ("exec_mirror_calls", self.exec_mirror_calls),
            ("exec_guard_suspend_calls", self.exec_guard_suspend_calls),
            ("exec_guard_device_calls", self.exec_guard_device_calls),
            ("resume_reconcile_ns", self.resume_reconcile_ns),
            ("resume_cop0_ns", self.resume_cop0_ns),
            ("resume_dispatch_ns", self.resume_dispatch_ns),
            ("resume_invalidate_ns", self.resume_invalidate_ns),
            ("resume_exit_ns", self.resume_exit_ns),
            ("resume_suspend_ns", self.resume_suspend_ns),
            ("resume_resolve_ns", self.resume_resolve_ns),
            ("resume_hostcall_ns", self.resume_hostcall_ns),
            ("resume_hostcall_calls", self.resume_hostcall_calls),
            ("resume_reconcile_calls", self.resume_reconcile_calls),
            ("resume_dispatch_calls", self.resume_dispatch_calls),
            (
                "vi_present_in_executor_calls",
                self.vi_present_in_executor_calls,
            ),
            (
                "vi_present_outside_executor_calls",
                self.vi_present_outside_executor_calls,
            ),
        ]
    }
}

impl FieldSample {
    /// Wall milliseconds attributable to ONE emulated field. This is the
    /// quantity the 60fps bar is about, and the one a multi-field catch-up
    /// would otherwise misreport by its field count.
    fn per_field_ms(&self) -> f64 {
        self.wall_ms / f64::from(self.fields.max(1))
    }
}

/// Whether to sample work counters per field and emit the population split.
///
/// A SEPARATE gate from `FN64_FRAME_CENSUS`, deliberately. The plain census
/// costs one `Instant::now` per field; this adds a `with_executor` borrow and
/// ~20 atomic loads. That is small against a 16-40 ms field, but the whole
/// point of an A/B is that the measured program matches the unmeasured one,
/// and every historical frame-census number was taken WITHOUT this. Making it
/// opt-in keeps those numbers comparable and makes the perturbation
/// measurable: run both ways and diff the mean.
pub fn population_split_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_FRAME_CENSUS_POPULATIONS"))
}

/// How many consecutive steady-state per-field costs to print verbatim, for
/// the periodicity check. 0 (the default) prints none.
///
/// # Why a raw dump and not a computed statistic
///
/// A ~50/50 split with a tight p50 and a tight p95 has two very different
/// explanations that no summary statistic separates: strict alternation
/// (every other field does extra work -- structural, and probably cheap to
/// name) versus a random Bernoulli draw (a much harder problem). An
/// autocorrelation number would answer "is there periodicity" but not "what
/// is the period", and a wrong guess at the period is exactly the kind of
/// inference perf-method rule 3 forbids. The sequence itself answers both and
/// costs nothing to produce.
fn sequence_dump_len() -> usize {
    static LEN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LEN.get_or_init(|| {
        std::env::var("FN64_FRAME_CENSUS_SEQUENCE")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(0)
    })
}

/// Skip this many steady-state fields before the sequence dump begins, so the
/// dump can be taken from deep inside the steady state rather than at its
/// leading edge.
fn sequence_dump_skip() -> usize {
    static SKIP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SKIP.get_or_init(|| {
        std::env::var("FN64_FRAME_CENSUS_SEQUENCE_SKIP")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .unwrap_or(0)
    })
}

#[derive(Default)]
struct Census {
    /// Counter totals at the previous field boundary, when the population
    /// split is armed. `None` until the first boundary.
    last_counters: Option<Counters>,
    /// Wall clock at the previous field boundary. `None` until the first
    /// field, because a duration needs two endpoints and the span from process
    /// start to the first field is setup, not a frame.
    last_boundary: Option<Instant>,
    last_sim_time: u64,
    /// Fields observed before the warmup gate opened.
    transient_fields: u64,
    transient_wall_ms: f64,
    /// Steady-state samples, in observation order.
    samples: Vec<FieldSample>,
    /// Steady-state fields dropped because `MAX_SAMPLES` was reached.
    truncated: u64,
    /// Whether the warmup gate has opened. Latched: a gfx-submit count cannot
    /// go backwards, but latching makes the boundary a single point in the run
    /// rather than a predicate re-evaluated per field.
    steady: bool,
    /// Field ordinal and gfx-submit count at which steady state began.
    steady_began_at_field: u64,
    steady_began_at_gfx: u64,
    total_fields: u64,
}

static CENSUS: Mutex<Option<Census>> = Mutex::new(None);

/// Record one committed VI retrace. Called from `advance_virtual_time` when it
/// observes a nonzero retrace tick count.
///
/// `retrace_ticks` may exceed 1 when the fabric commits several overdue fields
/// in one advance. Those are counted as fields but produce a SINGLE wall
/// sample attributed to the whole catch-up, because the host genuinely spent
/// that one span producing them -- splitting it evenly would invent per-field
/// timings that were never observed and would flatter the distribution.
pub(crate) fn observe_vi_fields(retrace_ticks: u32, now_sim_time: u64) {
    if !ARMED.load(Ordering::Relaxed) {
        return;
    }
    let at = Instant::now();
    // The counter read happens BEFORE the lock and after the timestamp, so
    // the wall sample brackets guest work and not this census's own
    // bookkeeping.
    let counters = population_split_enabled().then(Counters::sample);
    let gfx_submits = counters.map_or_else(|| crate::task_counts().0, |c| c.gfx_tasks);
    let mut guard = CENSUS.lock().expect("frame census poisoned");
    let census = guard.get_or_insert_with(Census::default);
    census.total_fields += u64::from(retrace_ticks);
    // `None` when the split is off: nothing is stored, nothing is differenced,
    // and every recorded delta is the zero value.
    let counter_delta = counters
        .and_then(|now| census.last_counters.replace(now).map(|prev| now.delta(&prev)))
        .unwrap_or_default();

    let Some(previous) = census.last_boundary.replace(at) else {
        // First field: no previous boundary, so no duration exists yet.
        census.last_sim_time = now_sim_time;
        return;
    };
    let wall_ms = at.duration_since(previous).as_secs_f64() * 1000.0;
    let virtual_cycles = now_sim_time.saturating_sub(census.last_sim_time);
    census.last_sim_time = now_sim_time;

    if !census.steady {
        if gfx_submits < warmup_gfx() {
            census.transient_fields += u64::from(retrace_ticks);
            census.transient_wall_ms += wall_ms;
            return;
        }
        census.steady = true;
        census.steady_began_at_field = census.total_fields;
        census.steady_began_at_gfx = gfx_submits;
    }

    if census.samples.len() >= MAX_SAMPLES {
        census.truncated += 1;
        return;
    }
    census.samples.push(FieldSample {
        wall_ms,
        fields: retrace_ticks,
        virtual_cycles,
        gfx_submits,
        counters: counter_delta,
    });
}

/// Arm the census and the at-exit report. Idempotent; returns immediately when
/// the gate is absent, which is every non-diagnostic run.
///
/// Installed from `advance_virtual_time` rather than from a harness `main`, so
/// no harness source is edited -- notably not
/// `examples/wm2000-block-boot/src/main.rs`, whose bytes are hashed into the
/// canonical program identity (`build.rs` reads it into
/// `DISPATCH_SOURCE_SHA256`). Adding the census there would change the
/// program's identity digest for a diagnostic.
pub fn install() {
    if !enabled() {
        // THE TRAP THIS AVOIDS: if `FN64_PROFILE` is set but the census gate
        // did not arm, returning here silently would emit NOTHING -- no report
        // and no warning, because the warning would live behind the very gate
        // that failed. That is the shape that cost two 25-minute runs, and
        // `scripts/byte-identity-1p5M.txt` documents a live instance of it.
        //
        // So the refusal is raised HERE, on the path where the gate is OFF,
        // and it is not conditioned on the census gate it reports about.
        if crate::profile::enabled() {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                eprint!(
                    "{} FN64_PROFILE is set but FN64_FRAME_CENSUS did not arm, so no census \
                     exists to report. Nothing below is a measurement.\n",
                    crate::profile::TAG,
                );
                use std::io::Write as _;
                let _ = std::io::stderr().flush();
                std::process::exit(70);
            });
        }
        return;
    }
    static ARMED_ONCE: std::sync::Once = std::sync::Once::new();
    ARMED_ONCE.call_once(|| {
        ARMED.store(true, Ordering::Relaxed);
        extern "C" fn at_exit() {
            let text = report();
            print!("{text}");
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
            // Requirement: under FN64_PROFILE, refuse rather than present a
            // plausible subset. A non-zero exit is what makes the refusal
            // impossible to skim past -- a printed warning has already been
            // filtered, missed and acted on twice.
            if crate::profile::enabled() && text.contains("REFUSING TO PRINT") {
                let _ = std::io::stderr().flush();
                std::process::exit(70);
            }
        }
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(at_exit) };
    });
}

/// Nearest-rank percentile over an ascending slice. Deterministic, and the
/// same rule `fn64_shell::timing` uses, so the two lanes' numbers are
/// comparable.
fn nearest_rank(sorted: &[f64], percentile: usize) -> f64 {
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

/// The distribution over one span of observed fields, plus both ratios.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDistribution {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    /// Samples that exceeded the 16.667 ms budget.
    pub over_budget: usize,
    /// Total host wall time across the span.
    pub wall_ms: f64,
    /// Total guest virtual time across the span, in 93.75 MHz CPU cycles.
    pub virtual_cycles: u64,
    /// VI fields the span committed. Exceeds `samples` whenever any advance
    /// committed more than one overdue field.
    pub fields: u64,
    /// The largest raw per-ADVANCE wall time, before per-field normalization,
    /// and how many fields that advance committed.
    ///
    /// Kept because normalization must not hide a real host stall. A 1,475 ms
    /// advance that committed 22 fields is a 67 ms/field catch-up and no
    /// freeze; a 1,475 ms advance that committed 1 field is a genuine
    /// one-and-a-half-second hitch. Only this pair distinguishes them.
    pub max_advance_ms: f64,
    pub max_advance_fields: u32,
    /// Graphics tasks the guest submitted across the span.
    ///
    /// This is what makes "sustained rendering" a checked claim rather than an
    /// assumption: a span with a beautiful latency distribution and zero
    /// graphics submits is measuring an idle guest, which is exactly the
    /// failure mode of the standard benchmark route (`gfx_submits=0` -- it
    /// renders nothing, so its frame times mean nothing).
    pub gfx_submits: u64,
}

impl FrameDistribution {
    fn from_samples(samples: &[FieldSample]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        // PER-FIELD, not per-advance. One advance can commit several overdue
        // fields; charging its whole span to one "frame" reports a frame time
        // nobody experienced. See `FieldSample`'s doc comment for the counter
        // evidence behind this (`8690d36`).
        let mut ms: Vec<f64> = samples.iter().map(FieldSample::per_field_ms).collect();
        let wall_ms: f64 = samples.iter().map(|s| s.wall_ms).sum();
        let virtual_cycles: u64 = samples.iter().map(|s| s.virtual_cycles).sum();
        let fields: u64 = samples.iter().map(|s| u64::from(s.fields.max(1))).sum();
        let worst_advance = samples
            .iter()
            .max_by(|a, b| a.wall_ms.total_cmp(&b.wall_ms))
            .expect("non-empty checked above");
        // Submits ACROSS the span: the running total at its end minus the
        // total at its start. `gfx_submits` is cumulative per sample, so a
        // difference is the only correct reduction -- summing them would
        // multiply the total by the sample count.
        let gfx_submits = samples
            .last()
            .expect("non-empty checked above")
            .gfx_submits
            .saturating_sub(samples[0].gfx_submits);
        // Count FIELDS over budget, not advances: a catch-up advance that
        // committed 22 fields at 67 ms each missed the bar 22 times, not once.
        let over_budget = samples
            .iter()
            .filter(|s| s.per_field_ms() > FRAME_BUDGET_MS)
            .map(|s| u64::from(s.fields.max(1)) as usize)
            .sum();
        ms.sort_by(f64::total_cmp);
        Some(Self {
            samples: ms.len(),
            p50_ms: nearest_rank(&ms, 50),
            p95_ms: nearest_rank(&ms, 95),
            p99_ms: nearest_rank(&ms, 99),
            max_ms: *ms.last().expect("non-empty checked above"),
            // Wall time divided by FIELDS, so the mean is per emulated field
            // and directly comparable to the 16.667 ms budget.
            mean_ms: wall_ms / fields as f64,
            over_budget,
            wall_ms,
            virtual_cycles,
            fields,
            max_advance_ms: worst_advance.wall_ms,
            max_advance_fields: worst_advance.fields.max(1),
            gfx_submits,
        })
    }

    /// Graphics tasks per emulated field across the span. A steady-state
    /// window should sit near 1: roughly one display list per field. A value
    /// near zero means the span was not rendering and its latency numbers
    /// describe an idle guest.
    pub fn gfx_per_field(&self) -> f64 {
        self.gfx_submits as f64 / self.fields as f64
    }

    /// Whether any advance in this span committed more than one VI field. When
    /// true, `max_ms` is a NORMALIZED per-field figure and the raw worst
    /// advance is `max_advance_ms` over `max_advance_fields` fields.
    pub fn has_multi_field_advances(&self) -> bool {
        self.fields > self.samples as u64
    }

    /// Whether EVERY field in the span fit inside one 60 Hz field. The bar is
    /// worst-case, so this reads `max`, not the median -- a p50 of 8 ms beside
    /// a max of 45 ms is a miss on every 45 ms frame, and neither p50 nor p95
    /// shows it.
    pub fn holds_60fps(&self) -> bool {
        self.max_ms <= FRAME_BUDGET_MS
    }

    /// Wall milliseconds per emulated VI field. **This is what the 60fps bar
    /// tests**; the target is 16.667.
    pub fn wall_ms_per_field(&self) -> f64 {
        self.mean_ms
    }

    /// Host wall time divided by guest virtual time. **This is the
    /// "times slower than hardware" figure**; the target is 1.0. It is a
    /// different question from `wall_ms_per_field` and the two diverge
    /// whenever the guest does not produce fields at its nominal rate.
    ///
    /// `None` when no virtual time elapsed, which would make the ratio
    /// meaningless rather than infinite.
    pub fn wall_versus_virtual(&self) -> Option<f64> {
        if self.virtual_cycles == 0 {
            return None;
        }
        let virtual_ms =
            self.virtual_cycles as f64 / fn64_runtime::CPU_CLOCK_HZ as f64 * 1000.0;
        Some(self.wall_ms / virtual_ms)
    }

    /// Fields per second the guest actually produced, from virtual time. This
    /// is the number that explains the gap between the two ratios: a guest
    /// emitting fields at 27 Hz spends 37 ms of VIRTUAL time per field, so
    /// 40 ms of wall time per field is only 1.1x slow, not 2.4x.
    ///
    /// `None` when no virtual time elapsed.
    pub fn guest_field_hz(&self) -> Option<f64> {
        if self.virtual_cycles == 0 {
            return None;
        }
        let virtual_seconds = self.virtual_cycles as f64 / fn64_runtime::CPU_CLOCK_HZ as f64;
        Some(self.fields as f64 / virtual_seconds)
    }
}

/// One population of fields: those on one side of the 16.667 ms line.
///
/// The two are reported side by side because the interesting quantity is the
/// DIFFERENCE, not either bucket's absolute numbers. Both buckets' latency
/// statistics are already implied by the whole-span distribution; the counters
/// are not.
#[derive(Debug, Clone, Copy, Default)]
struct Bucket {
    advances: usize,
    fields: u64,
    wall_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    /// The tail. Reported alongside p50/p95 because a mean has hidden the real
    /// distribution twice on this project -- a bimodal field split, then a
    /// trimodal windowed one -- and the 60fps bar is a WORST-CASE bound, so the
    /// tail is the quantity it actually tests.
    p99_ms: f64,
    mean_ms: f64,
    counters: Counters,
    /// Per-field p50/p95 for each `resume NET` phase, in ms.
    ///
    /// # Why a distribution and not just the mean
    ///
    /// The owner's complaint is not that emulation is slow on average -- it is
    /// that it is CHOPPY. `pump_ms` (the shell's wrapper around exactly this
    /// work) runs p50 ~20 ms with p95 67-88: **the step spikes 3-4x above its
    /// own median**, and that is what breaks the pace. A mean-only split cannot
    /// see that, and worse, it can hide it: one flat bucket and one spiky
    /// bucket average into an unremarkable pair of numbers. Reporting means
    /// where the distribution was the finding is the same error that hid the
    /// render cost for a day, one level down.
    ///
    /// # Why it is free
    ///
    /// The per-field values already exist -- each `FieldSample` carries its own
    /// counter DELTA, and this function already has the whole slice in hand and
    /// currently just sums it. So percentiles need **no new clock reads in the
    /// hot loop** and no retained state: they are computed here, at report
    /// time, from data that was being discarded. `Bucket` stays `Copy`.
    ///
    /// # Why no regime threshold
    ///
    /// A tempting alternative was a low/high regime partition with a constant
    /// boundary. Rejected: the windowed evidence that motivated it was revised
    /// once already (asserted bimodal, actually trimodal), and **a threshold
    /// constant bakes the current guess into the instrument**. Percentiles plus
    /// the raw per-field dump (`FN64_FRAME_CENSUS_SEQUENCE`) let the clustering
    /// be done on the bucket data afterward, so a wrong prior cannot be
    /// encoded. See perf-method rule 29.
    phase_p50_ms: PhasePercentiles,
    phase_p95_ms: PhasePercentiles,
}

/// One value per `resume NET` phase, in report order. A struct rather than an
/// array so a mis-ordered field is a compile error rather than a silently
/// swapped column.
#[derive(Debug, Clone, Copy, Default)]
struct PhasePercentiles {
    reconcile: f64,
    cop0: f64,
    dispatch: f64,
    invalidate: f64,
    exit: f64,
    suspend: f64,
    resolve: f64,
    hostcall: f64,
}

impl Bucket {
    fn from_samples(samples: &[&FieldSample]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut ms: Vec<f64> = samples.iter().map(|s| s.per_field_ms()).collect();
        ms.sort_by(f64::total_cmp);
        let wall_ms: f64 = samples.iter().map(|s| s.wall_ms).sum();
        let fields: u64 = samples.iter().map(|s| u64::from(s.fields.max(1))).sum();
        // Sum the per-field DELTAS, which is a total over the bucket. This is
        // sound where summing cumulative snapshots would not be -- the same
        // distinction `span_submits_are_a_difference_not_a_sum` pins for
        // `gfx_submits`.
        let mut counters = Counters::default();
        for sample in samples {
            let c = &sample.counters;
            macro_rules! add {
                ($($field:ident),* $(,)?) => { $(counters.$field += c.$field;)* };
            }
            add!(
                gfx_tasks,
                audio_tasks,
                executor_ns,
                gfx_ns,
                gfx_lle_ns,
                gfx_lle_rsp_ns,
                gfx_lle_rdp_ns,
                vi_present_ns,
                audio_lle_ns,
                executor_calls,
                gfx_calls,
                gfx_lle_calls,
                audio_lle_calls,
                rsp_steps_gfx,
                rsp_steps_audio,
                rsp_entries,
                dpc_calls,
                dpc_alloc_ns,
                dpc_copy_in_ns,
                dpc_copy_back_ns,
                barrier_served,
                barrier_fell_back,
                barrier_dirty_pages,
                barrier_clean,
                exec_mirror_ns,
                exec_resume_ns,
                exec_devtime_ns,
                exec_guard_suspend_ns,
                exec_guard_device_ns,
                exec_mirror_calls,
                exec_guard_suspend_calls,
                exec_guard_device_calls,
                resume_reconcile_ns,
                resume_cop0_ns,
                resume_dispatch_ns,
                resume_invalidate_ns,
                resume_exit_ns,
                resume_suspend_ns,
                resume_resolve_ns,
                resume_hostcall_ns,
                resume_hostcall_calls,
                resume_reconcile_calls,
                resume_dispatch_calls,
                vi_present_in_executor_calls,
                vi_present_outside_executor_calls,
            );
        }
        // Per-phase distributions, from the same per-field deltas summed
        // above. Each field's phase cost is divided by that field's own field
        // count, matching `per_field_ms` -- one advance can commit several
        // overdue fields, and charging a multi-field advance's whole cost to
        // one "frame" reports a latency nobody experienced.
        let percentiles = |pick: fn(&Counters) -> u64| {
            let mut v: Vec<f64> = samples
                .iter()
                .map(|s| pick(&s.counters) as f64 / 1.0e6 / f64::from(s.fields.max(1)))
                .collect();
            v.sort_by(f64::total_cmp);
            (nearest_rank(&v, 50), nearest_rank(&v, 95))
        };
        let (reconcile_p50, reconcile_p95) = percentiles(|c| c.resume_reconcile_ns);
        let (cop0_p50, cop0_p95) = percentiles(|c| c.resume_cop0_ns);
        let (dispatch_p50, dispatch_p95) = percentiles(|c| c.resume_dispatch_ns);
        let (invalidate_p50, invalidate_p95) = percentiles(|c| c.resume_invalidate_ns);
        let (exit_p50, exit_p95) = percentiles(|c| c.resume_exit_ns);
        let (suspend_p50, suspend_p95) = percentiles(|c| c.resume_suspend_ns);
        let (resolve_p50, resolve_p95) = percentiles(|c| c.resume_resolve_ns);
        let (hostcall_p50, hostcall_p95) = percentiles(|c| c.resume_hostcall_ns);
        Self {
            advances: ms.len(),
            fields,
            wall_ms,
            p50_ms: nearest_rank(&ms, 50),
            p95_ms: nearest_rank(&ms, 95),
            p99_ms: nearest_rank(&ms, 99),
            mean_ms: wall_ms / fields as f64,
            counters,
            phase_p50_ms: PhasePercentiles {
                reconcile: reconcile_p50,
                cop0: cop0_p50,
                dispatch: dispatch_p50,
                invalidate: invalidate_p50,
                exit: exit_p50,
                suspend: suspend_p50,
                resolve: resolve_p50,
                hostcall: hostcall_p50,
            },
            phase_p95_ms: PhasePercentiles {
                reconcile: reconcile_p95,
                cop0: cop0_p95,
                dispatch: dispatch_p95,
                invalidate: invalidate_p95,
                exit: exit_p95,
                suspend: suspend_p95,
                resolve: resolve_p95,
                hostcall: hostcall_p95,
            },
        }
    }
}

/// The bimodal split: the same span partitioned at the 16.667 ms budget, with
/// each bucket's per-field work counters.
///
/// # The question this answers
///
/// As of 2026-08-08 the gameplay route sits at p50 16.41 ms (INSIDE the
/// budget) and p95 39.03 (2.34x it), with 50.0% of fields over the line and
/// almost nothing near it. A uniform few-percent win therefore moves the
/// over-budget count by ~nothing -- measured: a real 1.6% mean improvement
/// moved it by 2 fields out of 11,321. So "what is expensive on average" is
/// the wrong question and "what makes the slow half slow" is the right one.
///
/// This reports, per bucket, the work counters divided by that bucket's field
/// count. Reading it:
///
/// - slow fields carry more `gfx_tasks` -> scene complexity or submit batching
/// - slow fields carry the same work but more `barrier_*` -> the apparatus
/// - slow fields carry more `rsp_steps_*` -> ucode workload
/// - no counter differs -> the cost is not where any counter looks
///
/// The last outcome is a real finding, not a failed measurement, and the
/// report says so in those words so it cannot be quietly rounded into a
/// mechanism.
#[derive(Debug, Clone, Copy)]
pub struct PopulationSplit {
    fast: Bucket,
    slow: Bucket,
    /// Whether counter sampling was armed. When false every counter reads
    /// zero and the report must say NO DATA rather than "no difference" --
    /// otherwise it is a check that returns the same answer whatever the
    /// state, which is perf-method rule 6a's error.
    armed: bool,
}

impl PopulationSplit {
    fn from_samples(samples: &[FieldSample], armed: bool) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let (slow, fast): (Vec<&FieldSample>, Vec<&FieldSample>) = samples
            .iter()
            .partition(|s| s.per_field_ms() > FRAME_BUDGET_MS);
        Some(Self {
            fast: Bucket::from_samples(&fast),
            slow: Bucket::from_samples(&slow),
            armed,
        })
    }
}

/// The whole census: the steady-state distribution and the transient it
/// excluded.
#[derive(Debug, Clone, Copy)]
pub struct FrameCensusReport {
    pub steady: Option<FrameDistribution>,
    pub total_fields: u64,
    pub transient_fields: u64,
    pub transient_wall_ms: f64,
    pub truncated: u64,
    pub warmup_gfx: u64,
    pub steady_began_at_field: u64,
    pub steady_began_at_gfx: u64,
    /// The fast/slow population split, when `FN64_FRAME_CENSUS_POPULATIONS`
    /// armed counter sampling.
    pub populations: Option<PopulationSplit>,
}

/// Snapshot the census without clearing it.
pub fn snapshot() -> Option<FrameCensusReport> {
    let guard = CENSUS.lock().expect("frame census poisoned");
    let census = guard.as_ref()?;
    Some(FrameCensusReport {
        steady: FrameDistribution::from_samples(&census.samples),
        populations: population_split_enabled()
            .then(|| PopulationSplit::from_samples(&census.samples, true))
            .flatten(),
        total_fields: census.total_fields,
        transient_fields: census.transient_fields,
        transient_wall_ms: census.transient_wall_ms,
        truncated: census.truncated,
        warmup_gfx: warmup_gfx(),
        steady_began_at_field: census.steady_began_at_field,
        steady_began_at_gfx: census.steady_began_at_gfx,
    })
}

/// Lag-1 autocorrelation of the per-field cost sequence, and the contingency
/// table of over-budget against graphics submits.
///
/// # Why lag-1 autocorrelation is the decisive number
///
/// A ~50/50 split with a tight p50 and a tight p95 has three candidate
/// explanations, and no summary statistic already computed separates them:
///
/// | shape | lag-1 | mechanism |
/// |---|---:|---|
/// | strict alternation `fSfSfS` | strongly NEGATIVE | the guest renders every second field |
/// | contiguous blocks `fffSSS` | strongly POSITIVE | route-phase mixture |
/// | random 50/50 | ~ZERO | neither; the cost is not periodic at all |
///
/// The three predictions are opposite in sign, so one number decides between
/// them. Synthetic sequences through this exact formula give -0.998, +0.993
/// and +0.004 respectively -- pinned as a test, so the decision rule was
/// fixed before any real data existed and cannot be fitted afterwards.
///
/// **Lag-1 is blind to periods other than 2.** A period-4 sequence like
/// `ffSSffSS` reads only weakly positive and a period-3 one reads weakly
/// negative -- both easily mistaken for "random". That is precisely why the
/// raw `f`/`S` string is printed FIRST and this number second: the eye finds
/// a longer cycle that this statistic cannot, and reading the number without
/// the string would be reading an instrument instead of an outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Periodicity {
    /// Lag-1 autocorrelation over the steady-state per-field costs.
    pub lag1: f64,
    /// Autocorrelation at lags 2..=6, so a longer cycle is visible as a
    /// number and not only in the printed string.
    pub lags: [f64; 6],
    pub samples: usize,
    /// Fields whose advance carried no new graphics submit, split by budget.
    pub no_submit_fast: u64,
    pub no_submit_slow: u64,
    /// Fields whose advance carried at least one new graphics submit.
    pub submit_fast: u64,
    pub submit_slow: u64,
    /// Mean new submits on fields that carried any, split by budget.
    pub submits_per_submitting_fast: f64,
    pub submits_per_submitting_slow: f64,
}

/// Autocorrelation of `values` at `lag`, by the standard estimator: the
/// lag-`lag` cross-product of mean-centred values over the total variance.
///
/// Returns 0.0 for a constant sequence (zero variance) rather than `NaN`,
/// because "no variation" is the honest reading and a `NaN` propagating into
/// a report reads as a broken instrument.
fn autocorrelation(values: &[f64], lag: usize) -> f64 {
    if values.len() <= lag || values.is_empty() {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance: f64 = values.iter().map(|v| (v - mean) * (v - mean)).sum();
    if variance <= 0.0 {
        return 0.0;
    }
    let covariance: f64 = values
        .windows(lag + 1)
        .map(|w| (w[0] - mean) * (w[lag] - mean))
        .sum();
    covariance / variance
}

/// Compute the periodicity statistics over the retained steady-state samples.
fn periodicity() -> Option<Periodicity> {
    let guard = CENSUS.lock().expect("frame census poisoned");
    let census = guard.as_ref()?;
    if census.samples.len() < 8 {
        return None;
    }
    let costs: Vec<f64> = census
        .samples
        .iter()
        .map(FieldSample::per_field_ms)
        .collect();
    let mut lags = [0.0f64; 6];
    for (i, slot) in lags.iter_mut().enumerate() {
        *slot = autocorrelation(&costs, i + 2);
    }
    // The submit delta per advance, from the cumulative counts the census has
    // always recorded. A difference, never a sum -- the same reduction
    // `span_submits_are_a_difference_not_a_sum` pins for the span total.
    let mut table = Periodicity {
        lag1: autocorrelation(&costs, 1),
        lags,
        samples: costs.len(),
        no_submit_fast: 0,
        no_submit_slow: 0,
        submit_fast: 0,
        submit_slow: 0,
        submits_per_submitting_fast: 0.0,
        submits_per_submitting_slow: 0.0,
    };
    let (mut fast_submits, mut slow_submits) = (0u64, 0u64);
    for pair in census.samples.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        let delta = current.gfx_submits.saturating_sub(previous.gfx_submits);
        let slow = current.per_field_ms() > FRAME_BUDGET_MS;
        match (delta == 0, slow) {
            (true, false) => table.no_submit_fast += 1,
            (true, true) => table.no_submit_slow += 1,
            (false, false) => {
                table.submit_fast += 1;
                fast_submits += delta;
            }
            (false, true) => {
                table.submit_slow += 1;
                slow_submits += delta;
            }
        }
    }
    table.submits_per_submitting_fast =
        fast_submits as f64 / table.submit_fast.max(1) as f64;
    table.submits_per_submitting_slow =
        slow_submits as f64 / table.submit_slow.max(1) as f64;
    Some(table)
}

/// The raw per-field cost sequence, for the periodicity check.
///
/// Returns `(skip, costs)` where `costs` is at most
/// `FN64_FRAME_CENSUS_SEQUENCE` consecutive steady-state per-field
/// milliseconds beginning `FN64_FRAME_CENSUS_SEQUENCE_SKIP` fields in.
/// Empty when the dump is not requested.
fn sequence_dump() -> (usize, Vec<(f64, u64)>) {
    let len = sequence_dump_len();
    if len == 0 {
        return (0, Vec::new());
    }
    let skip = sequence_dump_skip();
    let guard = CENSUS.lock().expect("frame census poisoned");
    let Some(census) = guard.as_ref() else {
        return (skip, Vec::new());
    };
    // Submit DELTA per advance, from the cumulative counts. Available whether
    // or not the population split is armed, because `gfx_submits` has always
    // been recorded -- only `report()` discarded it.
    let costs = census
        .samples
        .windows(2)
        .skip(skip)
        .take(len)
        .map(|pair| {
            (
                pair[1].per_field_ms(),
                pair[1].gfx_submits.saturating_sub(pair[0].gfx_submits),
            )
        })
        .collect();
    (skip, costs)
}

/// The at-exit text. Every line is prefixed `[frame-census]` so a route log can
/// be grepped for exactly this.
pub fn report() -> String {
    let Some(report) = snapshot() else {
        return String::from("[frame-census] no VI fields observed\n");
    };
    let mut out = String::new();
    out.push_str(&format!(
        "[frame-census] total_fields={} transient_fields={} (warmup_gfx={}, {:.0}ms) \
         steady_began_at_field={} steady_began_at_gfx={} truncated={}\n",
        report.total_fields,
        report.transient_fields,
        report.warmup_gfx,
        report.transient_wall_ms,
        report.steady_began_at_field,
        report.steady_began_at_gfx,
        report.truncated,
    ));
    let Some(steady) = report.steady else {
        out.push_str(
            "[frame-census] no steady-state fields: the warmup gate never opened. \
             Lower FN64_FRAME_CENSUS_WARMUP_GFX or run a longer route.\n",
        );
        return out;
    };
    // Prove the span was actually rendering before believing its latency.
    out.push_str(&format!(
        "[frame-census] steady-state rendering evidence: gfx_submits={} across the span \
         ({:.2} per field){}\n",
        steady.gfx_submits,
        steady.gfx_per_field(),
        if steady.gfx_submits == 0 {
            " -- WARNING: zero submits, this span rendered NOTHING and its \
             latency describes an idle guest"
        } else {
            ""
        },
    ));
    out.push_str(&format!(
        "[frame-census] steady-state PER-FIELD latency fields={} (advances={}) \
         p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms mean={:.2}ms \
         over_16.667ms={} ({:.1}%) holds_60fps={}\n",
        steady.fields,
        steady.samples,
        steady.p50_ms,
        steady.p95_ms,
        steady.p99_ms,
        steady.max_ms,
        steady.mean_ms,
        steady.over_budget,
        steady.over_budget as f64 / steady.fields as f64 * 100.0,
        steady.holds_60fps(),
    ));
    // Say when normalization was actually load-bearing, and show the raw worst
    // advance so a genuine host stall is not hidden behind a per-field mean.
    if steady.has_multi_field_advances() {
        out.push_str(&format!(
            "[frame-census] {} of those fields were committed by multi-field catch-up \
             advances; the worst single advance was {:.1}ms covering {} field(s) \
             = {:.2}ms/field. A catch-up is NOT a freeze: the guest stayed runnable \
             across those deadlines, its clock advanced every field, and no time was \
             lost (see 8690d36).\n",
            steady.fields - steady.samples as u64,
            steady.max_advance_ms,
            steady.max_advance_fields,
            steady.max_advance_ms / f64::from(steady.max_advance_fields),
        ));
    }
    // Both ratios, always, on one line, because quoting either alone is the
    // documented trap this census exists to make impossible.
    let versus = steady
        .wall_versus_virtual()
        .map_or_else(|| "n/a".to_string(), |r| format!("{r:.3}x"));
    let hz = steady
        .guest_field_hz()
        .map_or_else(|| "n/a".to_string(), |h| format!("{h:.1}Hz"));
    out.push_str(&format!(
        "[frame-census] RATIO A (the 60fps bar): {:.2} wall ms per emulated VI field, \
         target 16.667 -- {:.2}x the budget\n",
        steady.wall_ms_per_field(),
        steady.wall_ms_per_field() / FRAME_BUDGET_MS,
    ));
    out.push_str(&format!(
        "[frame-census] RATIO B (vs hardware): {versus} wall-versus-virtual, target 1.000x; \
         guest emitted fields at {hz} of a nominal 60Hz\n"
    ));
    out.push_str(
        "[frame-census] the two differ because the guest does not emit fields at 60Hz; \
         A is what a player sees, B is how much slower than the console the emulation runs. \
         Excludes present/window cost: this is guest+runtime only.\n",
    );
    // The sequence FIRST, then the statistic over it. A few hundred
    // characters of `f`/`S` shows a period-3 or period-4 cycle that lag-1
    // autocorrelation cannot see, so reading the number without the string is
    // reading an instrument instead of an outcome.
    out.push_str(&sequence_report());
    if let Some(p) = periodicity() {
        out.push_str(&periodicity_report(&p));
    }
    if let Some(split) = report.populations {
        out.push_str(&population_report(&split));
    }
    out
}

/// The periodicity verdict: lag-1 autocorrelation with a decision rule fixed
/// in advance, plus the over-budget-by-submits contingency table.
fn periodicity_report(p: &Periodicity) -> String {
    let mut out = format!(
        "[frame-periodicity] lag1={:.3} over {} steady-state fields \
         (lag2={:.3} lag3={:.3} lag4={:.3} lag5={:.3} lag6={:.3})\n",
        p.lag1, p.samples, p.lags[0], p.lags[1], p.lags[2], p.lags[3], p.lags[4],
    );
    // The rule was written before the number existed. Do not soften it.
    let verdict = if p.lag1 <= -0.3 {
        "STRONGLY NEGATIVE -> ALTERNATION. The slow fields interleave with the fast ones \
         rather than clustering, which is the signature of the guest doing extra work on \
         every second field."
    } else if p.lag1 >= 0.3 {
        "STRONGLY POSITIVE -> CONTIGUOUS BLOCKS. Slow fields cluster together, so this is a \
         route-phase mixture (some stretches of the route are expensive) and not a per-field \
         cycle."
    } else {
        "NEAR ZERO -> NEITHER. The sequence is not period-2 alternating and slow fields do \
         not cluster. Check the printed pattern for a longer cycle before concluding it is \
         random: lag-1 is blind to period 3 and 4."
    };
    out.push_str(&format!("[frame-periodicity] {verdict}\n"));
    let submitting = p.submit_fast + p.submit_slow;
    let silent = p.no_submit_fast + p.no_submit_slow;
    out.push_str(&format!(
        "[frame-periodicity] contingency (advance carried new gfx submits x over budget):\n\
         [frame-periodicity]   submits=0  fast={:>6} slow={:>6}  (total {silent})\n\
         [frame-periodicity]   submits>0  fast={:>6} slow={:>6}  (total {submitting}), \
         mean submits when nonzero: fast={:.2} slow={:.2}\n",
        p.no_submit_fast,
        p.no_submit_slow,
        p.submit_fast,
        p.submit_slow,
        p.submits_per_submitting_fast,
        p.submits_per_submitting_slow,
    ));
    // Independent of the autocorrelation: does the submit count EXPLAIN the
    // split, or merely accompany it?
    let slow_total = p.no_submit_slow + p.submit_slow;
    let fast_total = p.no_submit_fast + p.submit_fast;
    if slow_total > 0 && fast_total > 0 {
        let slow_submitting = p.submit_slow as f64 / slow_total as f64;
        let fast_submitting = p.submit_fast as f64 / fast_total as f64;
        out.push_str(&format!(
            "[frame-periodicity] {:.1}% of SLOW fields carried a submit vs {:.1}% of FAST \
             fields. {}\n",
            100.0 * slow_submitting,
            100.0 * fast_submitting,
            if slow_submitting > 0.8 && fast_submitting < 0.2 {
                "The submit count EXPLAINS the split: slow fields render, fast fields do not."
            } else if (slow_submitting - fast_submitting).abs() < 0.1 {
                "Submits do NOT explain the split: both populations render at the same rate, \
                 so whatever makes the slow half slow is not the presence of a display list."
            } else {
                "Partial association: submits shift the odds but do not partition the \
                 populations."
            },
        ));
    }
    out
}

/// The bucket diff: the deliverable of the bimodality investigation.
fn population_report(split: &PopulationSplit) -> String {
    let mut out = String::new();
    let (fast, slow) = (&split.fast, &split.slow);
    let total_fields = fast.fields + slow.fields;
    if total_fields == 0 {
        return String::from("[frame-populations] no steady-state fields to split\n");
    }
    let total_wall = fast.wall_ms + slow.wall_ms;
    out.push_str(&format!(
        "[frame-populations] fast(<={:.3}ms) fields={} ({:.1}%) advances={} mean={:.2} \
         p50={:.2} p95={:.2} wall={:.0}ms ({:.1}% of span)\n",
        FRAME_BUDGET_MS,
        fast.fields,
        100.0 * fast.fields as f64 / total_fields as f64,
        fast.advances,
        fast.mean_ms,
        fast.p50_ms,
        fast.p95_ms,
        fast.wall_ms,
        100.0 * fast.wall_ms / total_wall.max(f64::MIN_POSITIVE),
    ));
    out.push_str(&format!(
        "[frame-populations] slow( >{:.3}ms) fields={} ({:.1}%) advances={} mean={:.2} \
         p50={:.2} p95={:.2} wall={:.0}ms ({:.1}% of span)\n",
        FRAME_BUDGET_MS,
        slow.fields,
        100.0 * slow.fields as f64 / total_fields as f64,
        slow.advances,
        slow.mean_ms,
        slow.p50_ms,
        slow.p95_ms,
        slow.wall_ms,
        100.0 * slow.wall_ms / total_wall.max(f64::MIN_POSITIVE),
    ));
    // The headline: how much of the gap to the bar lives in the slow half.
    // If the slow population were made as cheap as the fast one, the span
    // would cost this instead -- an upper bound on any fix that targets it.
    out.push_str(&format!(
        "[frame-populations] slow/fast mean ratio {:.2}x. Making every slow field cost what a \
         fast one does would put the whole span at the fast mean, {:.2}ms/field = {:.2}x the \
         {:.3}ms budget -- the CEILING on any fix aimed only at the slow half, and it \
         {} the bar. The span currently spends {:.1}% of its wall time in {:.1}% of its \
         fields.\n",
        if fast.mean_ms > 0.0 {
            slow.mean_ms / fast.mean_ms
        } else {
            0.0
        },
        fast.mean_ms,
        fast.mean_ms / FRAME_BUDGET_MS,
        FRAME_BUDGET_MS,
        if fast.mean_ms <= FRAME_BUDGET_MS {
            "CLEARS"
        } else {
            "does NOT clear"
        },
        100.0 * slow.wall_ms / total_wall.max(f64::MIN_POSITIVE),
        100.0 * slow.fields as f64 / total_fields as f64,
    ));
    if !split.armed {
        out.push_str(
            "[frame-populations] counters NOT SAMPLED (FN64_FRAME_CENSUS_POPULATIONS unset): \
             every value below would read zero in BOTH buckets whatever the truth, so an \
             absence of difference here would mean nothing. This is not a null result.\n",
        );
        return out;
    }
    // Per FIELD, so the two buckets are comparable despite different sizes.
    // A ratio of 1.00 means the slow fields did the same amount of that work
    // as the fast ones -- which, for a bucket that costs 2.3x as much wall
    // time, means that counter does not explain the split.
    let mut differing = 0usize;
    let mut all_zero = 0usize;
    for ((label, fast_total), (_, slow_total)) in fast
        .counters
        .labelled()
        .into_iter()
        .zip(slow.counters.labelled())
    {
        let fast_per = fast_total as f64 / fast.fields.max(1) as f64;
        let slow_per = slow_total as f64 / slow.fields.max(1) as f64;
        if fast_total == 0 && slow_total == 0 {
            all_zero += 1;
            out.push_str(&format!(
                "[frame-populations]   {label:<22} fast=0 slow=0 -- NO DATA (its own gate is \
                 off, or the workload never reaches it)\n"
            ));
            continue;
        }
        let ratio = if fast_per > 0.0 {
            slow_per / fast_per
        } else {
            f64::INFINITY
        };
        // 1.10x is the threshold for "this counter distinguishes the buckets".
        // Chosen because the split to explain is 2.3x in wall time: a counter
        // moving less than 10% cannot be the mechanism, and one moving 2x
        // plausibly is. Deliberately loose -- the report prints every ratio,
        // so the flag is a reading aid and the numbers are the evidence.
        let differs = !(0.909..=1.10).contains(&ratio);
        if differs {
            differing += 1;
        }
        out.push_str(&format!(
            "[frame-populations]   {label:<22} fast={fast_per:>12.3}/field \
             slow={slow_per:>12.3}/field  ratio={ratio:>7.3}x{}\n",
            if differs { "  <== DIFFERS" } else { "" },
        ));
    }
    out.push_str(&format!(
        "[frame-populations] {differing} counter(s) differ by >10% between the populations; \
         {all_zero} had no data.\n"
    ));
    if differing == 0 {
        out.push_str(
            "[frame-populations] NO COUNTER DISTINGUISHES THE TWO POPULATIONS. The slow fields \
             cost ~2x the wall time while doing the same measurable work, so the cost is \
             somewhere none of these counters looks. That is a finding, not a failed \
             measurement: it rules out scene complexity, submit batching, ucode workload and \
             journal-boundary count as the mechanism, and points at something per-field and \
             untimed.\n",
        );
    }
    out.push_str(&executor_split_report(&fast, &slow));
    out.push_str(&resume_split_report(&fast, &slow));
    // The composed, tree-checked view. Additive: the sections above are
    // unchanged and committed scripts still parse them.
    if crate::profile::enabled() {
        out.push_str(&profile_report(split));
    }
    out
}

/// Decompose `executor_ns` -- the counter that measured 21.72 ms of a 35.84 ms
/// WM2000 render field, 61%, with no sub-counters -- into named phases, per
/// population.
///
/// # Why this does subtraction rather than printing the raw counters
///
/// The generic per-counter loop above already prints every `exec_*` value as a
/// per-field ratio, and that is NOT sufficient. Three of these counters are
/// nested inside another (`exec_mirror_ns` and `exec_guard_suspend_ns` inside
/// `exec_resume_ns`; `exec_guard_device_ns` inside `exec_devtime_ns`), so
/// reading them as a list of peers overstates the total and understates the
/// residual. Reading an inclusive counter as a peer of its parent is exactly
/// how the 21.72 ms stayed hidden in the first place -- `executor_ns` was
/// tabulated beside `gfx_ns` when it CONTAINS it (perf-method rule 2). This
/// section exists so the same mistake cannot be made one level down.
///
/// # The residual is printed even when it is small
///
/// `executor_ns - (resume + devtime)` is `host::run_one_step`'s own frame. It
/// is expected to be near zero. It is printed anyway, and labelled, because an
/// unnamed remainder is the thing that went unnoticed for a session; a
/// remainder that is reported and boring is strictly better than one that is
/// absent and assumed boring.
fn executor_split_report(fast: &Bucket, slow: &Bucket) -> String {
    let armed = fast.counters.exec_resume_ns > 0 || slow.counters.exec_resume_ns > 0;
    if !armed {
        return String::from(
            "[executor-split] NOT ARMED (FN64_EXECUTOR_SPLIT unset). The executor_ns \
             decomposition is absent, not zero -- do not read this as 'the phases cost \
             nothing'.\n",
        );
    }
    let mut out = String::from(
        "[executor-split] executor_ns decomposed. NESTING: mirror and guard_suspend are INSIDE \
         resume; guard_device is INSIDE devtime. Rows marked (of ...) are nested, not peers -- \
         do not add them to the total.\n",
    );
    for (name, bucket) in [("fast", fast), ("slow", slow)] {
        let fields = bucket.fields.max(1) as f64;
        let c = &bucket.counters;
        // ns totals -> ms per field in this population.
        let ms = |ns: u64| ns as f64 / 1.0e6 / fields;
        let per = |n: u64| n as f64 / fields;
        let executor = ms(c.executor_ns);
        let resume = ms(c.exec_resume_ns);
        let devtime = ms(c.exec_devtime_ns);
        let mirror = ms(c.exec_mirror_ns);
        let guard_suspend = ms(c.exec_guard_suspend_ns);
        let guard_device = ms(c.exec_guard_device_ns);
        // The quantity the whole exercise is for: what is left of the resume
        // once the apparatus nested inside it is removed. This is the closest
        // thing to "the guest executing recompiled MIPS plus the runtime it
        // calls" that these counters can express.
        let resume_net = (resume - mirror - guard_suspend).max(0.0);
        let residual = (executor - resume - devtime).max(0.0);
        let guard_total = mirror + guard_suspend + guard_device;
        let share = |v: f64| {
            if executor > 0.0 {
                100.0 * v / executor
            } else {
                0.0
            }
        };
        out.push_str(&format!(
            "[executor-split] {name}: executor_ns={executor:.3}ms/field over \
             {:.1} calls/field ({:.1}us/call)\n",
            per(c.executor_calls),
            if c.executor_calls > 0 {
                c.executor_ns as f64 / 1.0e3 / c.executor_calls as f64
            } else {
                0.0
            },
        ));
        for (label, value, nested, calls) in [
            ("resume (Executor step)", resume, false, 0u64),
            ("  (of) mirror boundary", mirror, true, c.exec_mirror_calls),
            (
                "  (of) guard @ suspend",
                guard_suspend,
                true,
                c.exec_guard_suspend_calls,
            ),
            ("  (of) resume NET", resume_net, true, 0),
            ("devtime (advance)", devtime, false, 0),
            (
                "  (of) guard @ device",
                guard_device,
                true,
                c.exec_guard_device_calls,
            ),
            ("residual (run_one_step frame)", residual, false, 0),
        ] {
            let call_note = if calls > 0 {
                format!(
                    "  {:.1} calls/field, {:.2}us/call",
                    per(calls),
                    value * 1000.0 / per(calls).max(f64::MIN_POSITIVE),
                )
            } else {
                String::new()
            };
            out.push_str(&format!(
                "[executor-split] {name}:   {label:<30} {value:>8.3}ms/field {:>6.1}% of \
                 executor{}{call_note}\n",
                share(value),
                if nested { "  [nested]" } else { "" },
            ));
        }
        out.push_str(&format!(
            "[executor-split] {name}: APPARATUS (mirror + guard@suspend + guard@device) = \
             {guard_total:.3}ms/field = {:.1}% of executor_ns. GUEST+RUNTIME (resume net) = \
             {resume_net:.3}ms/field = {:.1}%.\n",
            share(guard_total),
            share(resume_net),
        ));
    }
    out.push_str(
        "[executor-split] Read the SLOW row against the 16.667ms budget: that is the field the \
         60fps bar fails on. A saving in a row that is large on the fast row and small on the \
         slow one pays into the population that already has headroom.\n",
    );
    out
}

/// Split `resume NET` -- the 83.2% of a render field that the level above
/// names but does not decompose.
///
/// # Why this exists
///
/// `executor_ns` was 61% of a render field with no sub-counters, and splitting
/// it revealed that the named apparatus is only 16.4%: **deleting every guard,
/// mirror and journal seam together leaves 46.8 ms against a 16.667 ms
/// budget.** What remains -- `resume NET`, translated guest code plus the
/// runtime it calls synchronously -- is where the 60fps bar is actually won or
/// lost, and it was one undifferentiated bucket. This is the same rule 2 move,
/// one level deeper.
///
/// # The nesting, which is the whole difficulty
///
/// Two of these rows are INCLUSIVE of counters reported elsewhere, and reading
/// them as peers is precisely the error that hid the original 21.72 ms:
///
/// - `resume_dispatch_ns` contains `gfx_ns` and `audio_lle_ns`. The guest
///   reaches graphics and audio by writing SP registers, which lands in
///   `task_dispatch::rsp_commit` on the guest's own stack, inside the dispatch
///   call. So "translated guest code" is `dispatch - gfx - audio`, and the
///   report prints that subtraction rather than leaving it to the reader.
/// - `resume_suspend_ns` is **retired and reads zero**: no timer spans the
///   coroutine suspend, because `suspend_active_coroutine` is a stackful switch
///   and a wall clock across it measures other threads' work. See the SUSPEND
///   GAP row.
///
/// And one counter is deliberately NOT subtracted: `vi_present_ns` is not in
/// this tree at all. See the reachability line the report prints.
///
/// # Closure is the honesty check
///
/// The seven phases plus a residual must sum to `resume NET`. The residual is
/// printed unconditionally and as a percentage, because a decomposition that
/// does not close is not a decomposition -- and if the residual is large, the
/// residual is the finding rather than an embarrassment to be trimmed.
fn resume_split_report(fast: &Bucket, slow: &Bucket) -> String {
    let armed = fast.counters.resume_dispatch_ns > 0 || slow.counters.resume_dispatch_ns > 0;
    let mut out = String::new();

    // The reachability result travels FIRST and unconditionally, because it is
    // ungated and answers a question the gated rows cannot: whether
    // `vi_present_ns` belongs inside `executor_ns` at all. A warning that rides
    // on a gate can be silenced by deselecting that gate (perf-method rule 27).
    let inside = fast.counters.vi_present_in_executor_calls
        + slow.counters.vi_present_in_executor_calls;
    let outside = fast.counters.vi_present_outside_executor_calls
        + slow.counters.vi_present_outside_executor_calls;
    if inside == 0 && outside > 0 {
        out.push_str(&format!(
            "[resume-split] VI REACHABILITY: {outside} presentations, ALL outside run_one_step, \
             0 inside. CONFIRMED BY OBSERVATION: vi_present_ns is NOT nested in executor_ns -- it \
             runs on the harness's advance_virtual_time arm. Do not subtract it from executor \
             self time.\n",
        ));
    } else if inside > 0 {
        out.push_str(&format!(
            "[resume-split] VI REACHABILITY: {inside} presentations INSIDE run_one_step and \
             {outside} outside. REFUTED: vi_present_ns IS (at least partly) nested in \
             executor_ns. The claim that presentation is harness-only is WRONG -- retract it.\n",
        ));
    } else {
        out.push_str(
            "[resume-split] VI REACHABILITY: no presentations observed; the seam is untested on \
             this route, which is not the same as confirmed.\n",
        );
    }

    if !armed {
        out.push_str(
            "[resume-split] NOT ARMED (FN64_RESUME_SPLIT unset). The resume NET decomposition is \
             absent, not zero -- do not read this as 'the guest costs nothing'.\n",
        );
        return out;
    }
    out.push_str(
        "[resume-split] resume NET decomposed into SIX phases plus a NAMED GAP. NESTING: \
         gfx+audio are INSIDE dispatch. Rows marked (of ...) are nested, not peers.\n",
    );
    out.push_str(
        "[resume-split] THE GAP IS DELIBERATE: suspend_active_coroutine is a stackful switch, \
         so the executor runs OTHER threads before this stack continues. A timer spanning it \
         measures wall time across a context switch -- measured -697% residual when tried. The \
         part that matters is exec_guard_suspend_ns; the rest is executor scheduling and belongs \
         to the parent counter. Named, not attributed.\n",
    );
    for (name, bucket) in [("fast", fast), ("slow", slow)] {
        let fields = bucket.fields.max(1) as f64;
        let c = &bucket.counters;
        let ms = |ns: u64| ns as f64 / 1.0e6 / fields;
        let per = |n: u64| n as f64 / fields;
        // Re-derive resume NET here rather than inheriting a published figure:
        // it is pinned to a binary (DISPATCH_SOURCE_SHA256) and a route, and a
        // bucket subtracted from someone else's total is a cross-route
        // subtraction wearing a disguise.
        let resume_net = (ms(c.exec_resume_ns) - ms(c.exec_mirror_ns)
            - ms(c.exec_guard_suspend_ns))
        .max(0.0);
        let reconcile = ms(c.resume_reconcile_ns);
        let cop0 = ms(c.resume_cop0_ns);
        let dispatch = ms(c.resume_dispatch_ns);
        let invalidate = ms(c.resume_invalidate_ns);
        let exit = ms(c.resume_exit_ns);
        let suspend = ms(c.resume_suspend_ns);
        let resolve = ms(c.resume_resolve_ns);
        let hostcall = ms(c.resume_hostcall_ns);
        let named =
            reconcile + cop0 + dispatch + invalidate + exit + suspend + resolve + hostcall;
        // Every phase has parked time subtracted by `ResumePhaseClock::lap`, so
        // these are ON-STACK costs and should close against `resume NET` --
        // which is itself wall-clock and therefore INCLUDES the time this
        // coroutine spent suspended while other threads ran. The gap is that
        // parked time: real, expected, positive, and not attributable to any
        // phase of THIS stack.
        let gap = resume_net - named;
        // The quantity the whole exercise is for: recompiled MIPS plus the
        // memory runtime, with the graphics and audio nested inside dispatch
        // taken back out.
        let gfx = ms(c.gfx_ns);
        let audio = ms(c.audio_lle_ns);
        // `dispatch` is now translated guest code DIRECTLY: graphics and audio
        // are reached through OS-call shims, which are the `hostcall` bucket,
        // so nothing needs subtracting out of dispatch. What remains inside
        // `hostcall` after gfx+audio is the rest of the guest's OS-call surface.
        let guest_code = dispatch;
        let hostcall_other = (hostcall - gfx - audio).max(0.0);
        let share = |v: f64| {
            if resume_net > 0.0 {
                100.0 * v / resume_net
            } else {
                0.0
            }
        };
        out.push_str(&format!(
            "[resume-split] {name}: resume NET={resume_net:.3}ms/field over {:.1} \
             dispatches/field\n",
            per(c.resume_dispatch_calls),
        ));
        for (label, value, nested) in [
            ("reconcile @1033", reconcile, false),
            ("cop0 sync + interrupts", cop0, false),
            ("dispatch = TRANSLATED GUEST CODE", dispatch, false),
            ("host calls (OS shims)", hostcall, false),
            ("  (of) gfx_ns", gfx, true),
            ("  (of) audio_lle_ns", audio, true),
            ("  (of) other OS-call work", hostcall_other, true),
            ("invalidate writes", invalidate, false),
            ("exit + publish checkpoint", exit, false),
            ("suspend (on-stack, parked subtracted)", suspend, false),
            ("resolve next entry", resolve, false),
        ] {
            out.push_str(&format!(
                "[resume-split] {name}:   {label:<34} {value:>8.3}ms/field {:>6.1}% of resume \
                 NET{}\n",
                share(value),
                if nested { "  [nested]" } else { "" },
            ));
        }
        out.push_str(&format!(
            "[resume-split] {name}:   {:<34} {gap:>8.3}ms/field {:>6.1}% of resume NET  \
             [CLOSURE: named={named:.3}]\n",
            "PARKED (other threads ran)",
            share(gap),
        ));
        // A NEGATIVE gap still means the instrument is broken: the phases claim
        // more time than their own parent contains, which is impossible. That
        // is the check that caught the lap-across-the-suspend defect, and it
        // must survive the fix rather than be relaxed by it. A large POSITIVE
        // gap is expected here and is not a failure.
        if gap < 0.0 && share(gap.abs()) > 5.0 {
            out.push_str(&format!(
                "[resume-split] {name}: NEGATIVE GAP -- the phases claim MORE than resume NET \
                 contains, which is impossible. The instrument is broken (a timer spanning the \
                 coroutine suspend does this); do not read the phases above.\n",
            ));
        }
        out.push_str(&format!(
            "[resume-split] {name}: TRANSLATED GUEST CODE = {guest_code:.3}ms/field = {:.1}%. \
             GRAPHICS+AUDIO = {:.3}ms/field = {:.1}%. DISPATCH-LOOP MACHINERY = {:.3}ms/field \
             = {:.1}%.\n",
            share(guest_code),
            gfx + audio,
            share(gfx + audio),
            reconcile + cop0 + invalidate + exit + suspend + resolve + hostcall_other,
            share(reconcile + cop0 + invalidate + exit + suspend + resolve + hostcall_other),
        ));
        // The DISTRIBUTION, because the owner's complaint is choppiness rather
        // than slowness: the shell's `pump_ms` runs p50 ~20 with p95 67-88, so
        // the step spikes 3-4x above its own median. A phase whose p95/p50 is
        // far above the others is where that spike lives, and a mean-only
        // table cannot show it.
        let p50 = &bucket.phase_p50_ms;
        let p95 = &bucket.phase_p95_ms;
        for (label, a, b) in [
            ("reconcile @1033", p50.reconcile, p95.reconcile),
            ("cop0 sync + interrupts", p50.cop0, p95.cop0),
            ("dispatch (GUEST)", p50.dispatch, p95.dispatch),
            ("invalidate writes", p50.invalidate, p95.invalidate),
            ("exit + publish checkpoint", p50.exit, p95.exit),
            ("suspend (on-stack)", p50.suspend, p95.suspend),
            ("host calls (OS shims)", p50.hostcall, p95.hostcall),
            ("resolve next entry", p50.resolve, p95.resolve),
        ] {
            out.push_str(&format!(
                "[resume-split] {name}: SPREAD {label:<28} p50={a:>8.3} p95={b:>8.3} \
                 ms/field  p95/p50={:.1}x\n",
                if a > 0.0 { b / a } else { 0.0 },
            ));
        }
        out.push_str(&format!(
            "[resume-split] {name}: field p50={:.3} p95={:.3} ms  p95/p50={:.1}x -- compare \
             against the phase spreads above: the phase whose ratio matches the FIELD's is the \
             one carrying the choppiness.\n",
            bucket.p50_ms,
            bucket.p95_ms,
            if bucket.p50_ms > 0.0 {
                bucket.p95_ms / bucket.p50_ms
            } else {
                0.0
            },
        ));
    }
    out.push_str(
        "[resume-split] Read the SLOW row. If TRANSLATED GUEST CODE dominates, the bar is a \
         code-quality problem and no amount of runtime trimming reaches it; if DISPATCH-LOOP \
         OVERHEAD dominates, the per-step apparatus is the target.\n",
    );
    out
}

/// The `FN64_PROFILE` report: one authoritative decomposition, tree-checked,
/// with both denominators on every row.
///
/// # What this adds over the sections above
///
/// The sections above are correct and stay as they are. What they lack is
/// enforcement: each re-derives the counter nesting by hand, and three of them
/// got it wrong in one evening. This function routes the same numbers through
/// [`crate::counter_tree`], so a child exceeding its parent **refuses to print
/// the affected subtree** rather than presenting it as a finding.
///
/// It also states every row against BOTH denominators. "20.9% of resume NET"
/// is what let three modest-looking rows be read as small when they summed to
/// 1.29x the frame budget -- the opposite conclusion.
fn profile_report(split: &PopulationSplit) -> String {
    use crate::profile;

    let mut out = String::new();
    out.push_str(&format!(
        "{} ================ FN64_PROFILE ================\n",
        profile::TAG,
    ));

    // Arming is verified BY EFFECT: did the channel produce data? An env echo
    // cannot distinguish `FN64_EXECUTOR_SPLIT=0` (which ARMS) from
    // `FN64_FRAME_CENSUS_POPULATIONS=0` (which disarms).
    let c = &split.slow.counters;
    let f = &split.fast.counters;
    let witness = |gate: &str| match gate {
        "FN64_FRAME_CENSUS" => split.fast.fields + split.slow.fields > 0,
        "FN64_FRAME_CENSUS_POPULATIONS" => split.armed,
        "FN64_PHASE_TIMING" => c.executor_ns > 0 || f.executor_ns > 0,
        "FN64_EXECUTOR_SPLIT" => c.exec_resume_ns > 0 || f.exec_resume_ns > 0,
        "FN64_RESUME_SPLIT" => c.resume_dispatch_ns > 0 || f.resume_dispatch_ns > 0,
        // The sequence dump is a request for N fields, not a counter; it is
        // armed iff a length was asked for.
        "FN64_FRAME_CENSUS_SEQUENCE" => sequence_dump_len() > 0,
        // Witnessed by the RSP instruction counters rather than the staging
        // timers: a route that never reaches `dispatch_captured_raw_rdp`
        // legitimately has zero staging time, but any rendering route retires
        // RSP instructions. Using the staging timers here would report a
        // correct zero as a broken channel.
        "FN64_DPC_COPY_CENSUS" => {
            c.rsp_entries > 0 || f.rsp_entries > 0 || c.dpc_calls > 0 || f.dpc_calls > 0
        }
        _ => false,
    };
    let missing = profile::verify(&witness);

    // Provenance travels FIRST and always: a per-field figure without its
    // route and binary is not a result, and reconstructing provenance after
    // the fact cost the worst hours of the evening this was built in.
    out.push_str(&profile::Provenance::collect(&missing).report());
    // The instrument's own cost, from an armed/control pair. `--profile` runs
    // the control lane first and passes its ms/field in; absent that, the
    // header says UNMEASURED rather than implying zero.
    let armed_ms = {
        let f = split.fast.wall_ms + split.slow.wall_ms;
        let n = (split.fast.fields + split.slow.fields).max(1) as f64;
        f / n
    };
    // NAMED TO AVOID A NEAR-MISS COLLISION: `FN64_PROFILE_CONTROL` already
    // exists and means something unrelated (the typed executor control
    // snapshot, `main.rs:1506`). `FN64_PROFILE_CONTROL_MS` would sit one
    // suffix away from it and read as its variant, so this uses a distinct
    // stem instead.
    let control_ms = std::env::var("FN64_PROFILE_BASELINE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok());
    out.push_str(&profile::perturbation_report(armed_ms, control_ms));
    out.push_str(&profile::scope_legend());

    // Refuse rather than print a plausible subset.
    if !missing.is_empty() {
        out.push_str(&profile::not_armed_report(&missing));
        return out;
    }

    for (name, bucket) in [("fast", &split.fast), ("slow", &split.slow)] {
        let fields = bucket.fields.max(1) as f64;
        let k = &bucket.counters;
        let ms = |ns: u64| ns as f64 / 1.0e6 / fields;
        // The tree's view of this population, in ms/field.
        let lookup = |counter: &str| -> f64 {
            match counter {
                "executor_ns" => ms(k.executor_ns),
                "exec_resume_ns" => ms(k.exec_resume_ns),
                "exec_mirror_ns" => ms(k.exec_mirror_ns),
                "exec_guard_suspend_ns" => ms(k.exec_guard_suspend_ns),
                "exec_devtime_ns" => ms(k.exec_devtime_ns),
                "exec_guard_device_ns" => ms(k.exec_guard_device_ns),
                "resume_reconcile_ns" => ms(k.resume_reconcile_ns),
                "resume_cop0_ns" => ms(k.resume_cop0_ns),
                "resume_dispatch_ns" => ms(k.resume_dispatch_ns),
                "resume_invalidate_ns" => ms(k.resume_invalidate_ns),
                "resume_exit_ns" => ms(k.resume_exit_ns),
                "resume_suspend_ns" => ms(k.resume_suspend_ns),
                "resume_resolve_ns" => ms(k.resume_resolve_ns),
                "resume_hostcall_ns" => ms(k.resume_hostcall_ns),
                "gfx_ns" => ms(k.gfx_ns),
                "gfx_lle_ns" => ms(k.gfx_lle_ns),
                "gfx_lle_rsp_ns" => ms(k.gfx_lle_rsp_ns),
                "gfx_lle_rdp_ns" => ms(k.gfx_lle_rdp_ns),
                "dpc_alloc_ns" => ms(k.dpc_alloc_ns),
                "dpc_copy_in_ns" => ms(k.dpc_copy_in_ns),
                "dpc_copy_back_ns" => ms(k.dpc_copy_back_ns),
                "audio_lle_ns" => ms(k.audio_lle_ns),
                "vi_present_ns" => ms(k.vi_present_ns),
                _ => 0.0,
            }
        };

        // THE CHECK. Runs before any row is formatted.
        let violations = crate::counter_tree::validate(&lookup);

        let resume_net = crate::counter_tree::value_of("resume_net", &lookup);
        out.push_str(&format!(
            "{} {name}: fields={} p50={:.3} p95={:.3} p99={:.3} mean={:.3} ms/field \
             ({:.2}x budget at p50, {:.2}x at p95)\n",
            profile::TAG,
            bucket.fields,
            bucket.p50_ms,
            bucket.p95_ms,
            bucket.p99_ms,
            bucket.mean_ms,
            bucket.p50_ms / profile::FRAME_BUDGET_MS,
            bucket.p95_ms / profile::FRAME_BUDGET_MS,
        ));

        // THE OUTERMOST CHECK, which the tree alone cannot make: the whole
        // decomposition must fit inside the MEASURED FIELD. `executor_ns` is
        // the tree's root, but the field's wall time is the real container,
        // and it is not a counter -- so a decomposition claiming 45.687 ms
        // inside a 10.000 ms field passes every parent/child test and is still
        // impossible. Found by reading this report's own first output: the
        // fast bucket showed 2.74x budget of resume NET inside a 0.60x field.
        let field_ms = bucket.mean_ms;
        let claimed = lookup("executor_ns").max(resume_net);
        if field_ms > 0.0
            && claimed > field_ms * (1.0 + crate::counter_tree::CLOSURE_TOLERANCE)
        {
            out.push_str(&format!(
                "{} {name}: DECOMPOSITION EXCEEDS ITS FIELD -- the phases claim {claimed:.3}ms \
                 inside a measured {field_ms:.3}ms field ({:.1}x). No parent/child check can \
                 catch this because the field's wall time is not a counter. THE INSTRUMENT IS \
                 BROKEN; DO NOT READ THE ROWS BELOW.\n",
                profile::TAG,
                claimed / field_ms,
            ));
        }

        if !violations.is_empty() {
            for v in &violations {
                out.push_str(&v.report());
            }
        }

        // `resume NET` and its siblings. The mirror is a SIBLING here while
        // being nested inside `exec_resume_ns` -- the relationship the tree
        // declares and three hand-written report sites got wrong.
        let mirror = lookup("exec_mirror_ns");
        let devtime = lookup("exec_devtime_ns");
        if !crate::counter_tree::suppressed_by("resume_net", &violations) {
            out.push_str(&profile::row(
                "resume NET (guest + runtime)",
                resume_net,
                lookup("executor_ns"),
                "executor",
            ));
            out.push_str(&profile::row(
                "mirror boundary [sibling of NET]",
                mirror,
                lookup("executor_ns"),
                "executor",
            ));
            out.push_str(&profile::row(
                "devtime (advance)",
                devtime,
                lookup("executor_ns"),
                "executor",
            ));
        }

        // The resume NET decomposition, every row against BOTH denominators.
        let mut named = Vec::new();
        for (label, counter) in [
            ("reconcile @1033", "resume_reconcile_ns"),
            ("cop0 sync + interrupts", "resume_cop0_ns"),
            ("dispatch = TRANSLATED GUEST CODE", "resume_dispatch_ns"),
            ("host calls (OS shims)", "resume_hostcall_ns"),
            ("invalidate writes", "resume_invalidate_ns"),
            ("exit + publish checkpoint", "resume_exit_ns"),
            ("suspend (on-stack)", "resume_suspend_ns"),
            ("resolve next entry", "resume_resolve_ns"),
        ] {
            if crate::counter_tree::suppressed_by(counter, &violations) {
                continue;
            }
            let v = lookup(counter);
            named.push(v);
            out.push_str(&profile::row(label, v, resume_net, "resume NET"));
        }

        // Graphics, nested inside the host calls.
        for (label, counter) in [
            ("  (of) gfx_ns", "gfx_ns"),
            ("    (of) RSP interpretation", "gfx_lle_rsp_ns"),
            ("    (of) RDP rasterization", "gfx_lle_rdp_ns"),
            // The staging copy, named per phase. An optimization target needs
            // its own row against the budget, not a share of a run total.
            ("      (of) staging alloc", "dpc_alloc_ns"),
            ("      (of) staging copy_in", "dpc_copy_in_ns"),
            ("      (of) staging copy_back", "dpc_copy_back_ns"),
            ("  (of) audio_lle_ns", "audio_lle_ns"),
        ] {
            if crate::counter_tree::suppressed_by(counter, &violations) {
                continue;
            }
            // Each row against ITS OWN parent, taken from the tree rather than
            // a denominator hardcoded at the call site. Hardcoding is how a
            // staging cost gets stated as a share of host calls when it is
            // really a share of the RDP seam -- a share of the wrong
            // denominator is not a size.
            let parent = crate::counter_tree::node(counter)
                .and_then(|n| n.parent)
                .unwrap_or("resume_hostcall_ns");
            // A readable name for the denominator. The raw counter identifier
            // is correct but reads as jargon in a column a human scans.
            let parent_label = match parent {
                "resume_hostcall_ns" => "host calls",
                "gfx_ns" => "gfx",
                "gfx_lle_ns" => "gfx LLE",
                "gfx_lle_rdp_ns" => "RDP",
                "resume_net" => "resume NET",
                other => other,
            };
            out.push_str(&profile::row(
                label,
                lookup(counter),
                lookup(parent),
                parent_label,
            ));
        }

        // Closure, preserved exactly: printed unconditionally, and a negative
        // residual is an instrument failure rather than a small number.
        let named_total: f64 = named.iter().sum();
        let gap = resume_net - named_total;
        out.push_str(&profile::row(
            "PARKED (other threads ran)",
            gap,
            resume_net,
            "resume NET",
        ));
        if gap < 0.0 && resume_net > 0.0 && (gap.abs() / resume_net) > 0.05 {
            out.push_str(&format!(
                "{} {name}: NEGATIVE RESIDUAL -- the phases claim MORE than resume NET \
                 contains, which is impossible. THE INSTRUMENT IS BROKEN; DO NOT READ THE ROWS \
                 ABOVE.\n",
                profile::TAG,
            ));
        }

        // THE SUM, computed in a tool rather than eyed. Every row above can be
        // individually right while the total contradicts the conclusion drawn
        // from them.
        out.push_str(&profile::total_row(
            "HOST-SIDE TOTAL (excl. graphics)",
            &[
                lookup("resume_reconcile_ns"),
                lookup("resume_cop0_ns"),
                lookup("resume_dispatch_ns"),
                lookup("resume_invalidate_ns"),
                lookup("resume_exit_ns"),
                lookup("resume_resolve_ns"),
                mirror,
                devtime,
                gap.max(0.0),
            ],
        ));
    }
    out.push_str(&format!(
        "{} Every row states BOTH its share of its parent AND its ratio to the {:.3}ms budget. \
         A share of the wrong denominator is not a size: check each row against the \
         denominator THE DECISION uses.\n",
        profile::TAG,
        profile::FRAME_BUDGET_MS,
    ));
    out
}

/// The raw per-field cost sequence. Printed rather than summarized: see
/// [`sequence_dump_len`] for why no statistic substitutes for it.
fn sequence_report() -> String {
    let (skip, costs) = sequence_dump();
    if costs.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "[frame-sequence] {} consecutive steady-state fields starting at steady field {skip}. \
         Each entry is per-field ms and the NEW gfx submits that advance carried. A '<' marks \
         a field inside the {:.3}ms budget.\n",
        costs.len(),
        FRAME_BUDGET_MS,
    );
    // A compact fast/slow string first: alternation is visible at a glance
    // here and invisible in a column of floats.
    let pattern: String = costs
        .iter()
        .map(|&(ms, _)| if ms > FRAME_BUDGET_MS { 'S' } else { 'f' })
        .collect();
    for (i, chunk) in pattern.as_bytes().chunks(80).enumerate() {
        out.push_str(&format!(
            "[frame-sequence] pattern[{:>5}] {}\n",
            skip + i * 80,
            String::from_utf8_lossy(chunk),
        ));
    }
    for (i, &(ms, gfx)) in costs.iter().enumerate() {
        out.push_str(&format!(
            "[frame-sequence] {:>6} {ms:>9.3}ms gfx={gfx}{}\n",
            skip + i,
            if ms > FRAME_BUDGET_MS { "" } else { " <" },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One advance committing exactly one field -- the ordinary case.
    fn sample(wall_ms: f64, virtual_cycles: u64) -> FieldSample {
        FieldSample {
            wall_ms,
            fields: 1,
            virtual_cycles,
            gfx_submits: 0,
            counters: Counters::default(),
        }
    }

    /// Cumulative submit counts, as the census records them.
    fn rendering_span(count: usize, wall_ms: f64) -> Vec<FieldSample> {
        (0..count)
            .map(|i| FieldSample {
                wall_ms,
                fields: 1,
                virtual_cycles: 0,
                gfx_submits: 1_000 + i as u64,
                counters: Counters::default(),
            })
            .collect()
    }

    #[test]
    fn reports_nearest_rank_percentiles_and_counts_budget_breaches() {
        let samples: Vec<FieldSample> = (1..=100).map(|ms| sample(ms as f64, 0)).collect();
        let d = FrameDistribution::from_samples(&samples).expect("100 samples");
        assert_eq!(d.samples, 100);
        assert_eq!(d.p50_ms, 50.0);
        assert_eq!(d.p95_ms, 95.0);
        assert_eq!(d.p99_ms, 99.0);
        assert_eq!(d.max_ms, 100.0);
        // 17..=100 exceed 16.667 ms; 1..=16 do not.
        assert_eq!(d.over_budget, 84);
        assert!(!d.holds_60fps());
    }

    /// The worst-case bar cannot be read off p50 or p95 -- the whole reason
    /// this reports a distribution rather than a mean.
    #[test]
    fn one_slow_field_in_fifty_fails_the_bar_while_p50_and_p95_stay_fast() {
        let mut samples: Vec<FieldSample> = (0..49).map(|_| sample(8.0, 0)).collect();
        samples.push(sample(45.0, 0));
        let d = FrameDistribution::from_samples(&samples).expect("50 samples");
        assert_eq!(d.p50_ms, 8.0);
        assert_eq!(d.p95_ms, 8.0, "p95 cannot see one field in fifty");
        assert_eq!(d.max_ms, 45.0);
        assert_eq!(d.over_budget, 1);
        assert!(!d.holds_60fps());
    }

    #[test]
    fn a_span_entirely_inside_one_field_holds_the_bound() {
        let samples: Vec<FieldSample> = (0..50).map(|_| sample(16.0, 0)).collect();
        let d = FrameDistribution::from_samples(&samples).expect("50 samples");
        assert_eq!(d.over_budget, 0);
        assert!(d.holds_60fps(), "16.0 ms fits inside a 16.667 ms field");
    }

    /// The documented trap, pinned as a test: the SAME span reads as 2.4x on
    /// the frame-budget ratio and 1.1x on wall-versus-virtual, because the
    /// guest emitted fields at ~27 Hz rather than 60 Hz. A benchmark that
    /// reported only one of these would be misleading in a specific,
    /// previously-made way.
    #[test]
    fn the_two_ratios_diverge_when_the_guest_underproduces_fields() {
        // 100 fields, 40 ms of wall time each. The guest charged 37 ms of
        // virtual time per field (~27 Hz), not the nominal 16.667 ms.
        let virtual_cycles_per_field =
            (fn64_runtime::CPU_CLOCK_HZ as f64 * 0.037).round() as u64;
        let samples: Vec<FieldSample> = (0..100)
            .map(|_| sample(40.0, virtual_cycles_per_field))
            .collect();
        let d = FrameDistribution::from_samples(&samples).expect("100 samples");

        // Ratio A: against the 60fps budget.
        assert!((d.wall_ms_per_field() - 40.0).abs() < 1e-9);
        let budget_ratio = d.wall_ms_per_field() / FRAME_BUDGET_MS;
        assert!(
            (budget_ratio - 2.4).abs() < 0.01,
            "expected ~2.4x the frame budget, got {budget_ratio}"
        );

        // Ratio B: against the guest's own clock.
        let versus = d.wall_versus_virtual().expect("virtual time elapsed");
        assert!(
            (versus - 1.081).abs() < 0.01,
            "expected ~1.08x wall-versus-virtual, got {versus}"
        );

        // And the reason they differ is a measurable, reportable quantity.
        let hz = d.guest_field_hz().expect("virtual time elapsed");
        assert!((hz - 27.0).abs() < 0.1, "expected ~27Hz, got {hz}");

        assert!(
            budget_ratio > versus * 2.0,
            "the trap is that these two are far apart; if they were close, \
             quoting one for the other would be harmless"
        );
    }

    /// A ratio needs a denominator. Reporting `inf` (or a panic) for a span
    /// where no virtual time elapsed would be worse than saying so.
    #[test]
    fn ratios_are_absent_rather_than_infinite_without_virtual_time() {
        let samples: Vec<FieldSample> = (0..10).map(|_| sample(5.0, 0)).collect();
        let d = FrameDistribution::from_samples(&samples).expect("10 samples");
        assert_eq!(d.wall_versus_virtual(), None);
        assert_eq!(d.guest_field_hz(), None);
        // The budget ratio is still well defined and still the bar.
        assert!((d.wall_ms_per_field() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_span_has_no_distribution() {
        assert!(FrameDistribution::from_samples(&[]).is_none());
    }

    /// The exact scenario a peer agent attributed by counter (`8690d36`): a
    /// 3,028 ms advance that committed 22 VI fields. Charging it to one frame
    /// reports a three-second frame nobody experienced -- the guest stayed
    /// runnable across those 22 deadlines, its clock advanced every one, and
    /// the guest was running FASTER than average during the span.
    ///
    /// Normalizing gives 137.6 ms/field, which is a real and reportable miss,
    /// but 22x smaller than the artifact.
    #[test]
    fn a_multi_field_catch_up_is_normalized_rather_than_reported_as_one_huge_frame() {
        let mut samples: Vec<FieldSample> = (0..99).map(|_| sample(10.0, 0)).collect();
        samples.push(FieldSample {
            wall_ms: 3028.0,
            fields: 22,
            virtual_cycles: 0,
            gfx_submits: 0,
            counters: Counters::default(),
        });
        let d = FrameDistribution::from_samples(&samples).expect("100 advances");

        assert_eq!(d.samples, 100, "100 advances");
        assert_eq!(d.fields, 121, "99 single fields + 22 from the catch-up");
        assert!(
            (d.max_ms - 3028.0 / 22.0).abs() < 0.01,
            "max must be the PER-FIELD 137.6ms, not the raw 3028ms; got {}",
            d.max_ms
        );
        assert!(d.has_multi_field_advances());

        // The raw advance is still retained, so a real stall stays visible.
        assert_eq!(d.max_advance_ms, 3028.0);
        assert_eq!(d.max_advance_fields, 22);

        // And all 22 fields are counted as budget misses, not just one.
        assert_eq!(d.over_budget, 22);
    }

    /// Normalization must not launder an actual host stall. Same wall time,
    /// one field: this one IS a one-and-a-half-second hitch and must read as
    /// one.
    #[test]
    fn a_single_field_stall_is_not_normalized_away() {
        let mut samples: Vec<FieldSample> = (0..99).map(|_| sample(10.0, 0)).collect();
        samples.push(sample(1475.0, 0));
        let d = FrameDistribution::from_samples(&samples).expect("100 advances");

        assert_eq!(d.fields, 100, "no catch-up occurred");
        assert!(!d.has_multi_field_advances());
        assert_eq!(
            d.max_ms, 1475.0,
            "a one-field advance keeps its full wall time"
        );
        assert_eq!(d.over_budget, 1);
    }

    /// The mean is per FIELD, so it is directly comparable to 16.667 ms. A
    /// per-advance mean would read high by the catch-up factor.
    #[test]
    fn the_mean_is_per_field_not_per_advance() {
        let samples = vec![
            sample(10.0, 0),
            FieldSample {
                wall_ms: 90.0,
                fields: 9,
                virtual_cycles: 0,
                gfx_submits: 0,
                counters: Counters::default(),
            },
        ];
        let d = FrameDistribution::from_samples(&samples).expect("2 advances");
        assert_eq!(d.fields, 10);
        assert_eq!(d.wall_ms, 100.0);
        assert!(
            (d.mean_ms - 10.0).abs() < 1e-9,
            "100ms over 10 FIELDS is 10ms/field, not 50ms/advance; got {}",
            d.mean_ms
        );
    }

    /// The warmup gate must not leak the transient into the first steady
    /// sample. `observe_vi_fields` advances `last_boundary` on EVERY field,
    /// including gated ones, so the first steady sample is measured from the
    /// last transient field rather than from an earlier point. Had the gate
    /// skipped the timestamp update, the first steady sample would absorb the
    /// entire boot transient -- hundreds of milliseconds -- and would own
    /// `max` for the rest of the run, which is precisely the startup-in-the-
    /// p99 error the window exists to prevent.
    #[test]
    fn the_warmup_boundary_does_not_leak_transient_time_into_the_first_sample() {
        let mut census = Census::default();
        let base = Instant::now();

        // Simulate the observe path's bookkeeping across a gate opening.
        // Transient: one very slow field.
        census.last_boundary = Some(base);
        let transient_end = base + std::time::Duration::from_millis(500);
        let transient_ms =
            transient_end.duration_since(base).as_secs_f64() * 1000.0;
        census.transient_wall_ms += transient_ms;
        census.transient_fields += 1;
        census.last_boundary = Some(transient_end);

        // Steady: the next field takes 10 ms, measured from the transient's
        // END, not from `base`.
        let steady_end = transient_end + std::time::Duration::from_millis(10);
        let steady_ms = steady_end
            .duration_since(census.last_boundary.expect("set above"))
            .as_secs_f64()
            * 1000.0;
        census.samples.push(sample(steady_ms, 0));

        let d = FrameDistribution::from_samples(&census.samples).expect("1 sample");
        assert!(
            (d.max_ms - 10.0).abs() < 1.0,
            "first steady sample must be ~10ms, not ~510ms; got {}",
            d.max_ms
        );
        assert!(
            d.holds_60fps() || d.max_ms < 20.0,
            "the 500ms transient must not appear in the steady distribution"
        );
        assert!(
            (census.transient_wall_ms - 500.0).abs() < 1.0,
            "and the transient must still be REPORTED, not discarded silently"
        );
    }

    /// `gfx_submits` is cumulative per sample, so the span total is a
    /// DIFFERENCE. Summing them would report 100,450 submits for a 100-field
    /// span that rendered 99, and would make every idle span look busy.
    #[test]
    fn span_submits_are_a_difference_not_a_sum() {
        let d = FrameDistribution::from_samples(&rendering_span(100, 10.0))
            .expect("100 samples");
        assert_eq!(d.gfx_submits, 99, "1099 - 1000");
        assert!((d.gfx_per_field() - 0.99).abs() < 1e-9);
    }

    /// The failure mode this census exists to make visible: a span can post an
    /// excellent latency distribution while rendering nothing at all, which is
    /// exactly what the standard 19,523-step benchmark route does
    /// (`gfx_submits=0`). Latency over an idle guest is not a frame time.
    #[test]
    fn an_idle_span_is_distinguishable_from_a_rendering_one_at_identical_latency() {
        let idle: Vec<FieldSample> = (0..100).map(|_| sample(4.0, 0)).collect();
        let rendering = rendering_span(100, 4.0);

        let idle = FrameDistribution::from_samples(&idle).expect("100 samples");
        let rendering = FrameDistribution::from_samples(&rendering).expect("100 samples");

        // Identical on every latency statistic, including the bar itself.
        assert_eq!(idle.p50_ms, rendering.p50_ms);
        assert_eq!(idle.max_ms, rendering.max_ms);
        assert!(idle.holds_60fps() && rendering.holds_60fps());

        // Separable only by the rendering evidence.
        assert_eq!(idle.gfx_submits, 0);
        assert_eq!(rendering.gfx_submits, 99);
    }

    /// A sample carrying a per-field counter delta.
    fn sample_with(wall_ms: f64, counters: Counters) -> FieldSample {
        FieldSample {
            wall_ms,
            fields: 1,
            virtual_cycles: 0,
            gfx_submits: 0,
            counters,
        }
    }

    /// The partition is at the budget, and it is exclusive at the boundary
    /// exactly as `over_budget` is -- a field of precisely 16.667 ms is fast,
    /// because `holds_60fps` accepts it. Two definitions of "over budget" in
    /// one module would let the split disagree with the headline count.
    #[test]
    fn the_split_partitions_at_the_same_boundary_the_over_budget_count_uses() {
        let mut samples: Vec<FieldSample> = (0..30).map(|_| sample(10.0, 0)).collect();
        samples.extend((0..70).map(|_| sample(40.0, 0)));
        samples.push(sample(FRAME_BUDGET_MS, 0));

        let d = FrameDistribution::from_samples(&samples).expect("101 samples");
        let split = PopulationSplit::from_samples(&samples, true).expect("101 samples");

        assert_eq!(split.slow.fields, 70);
        assert_eq!(split.fast.fields, 31, "the exactly-16.667ms field is FAST");
        assert_eq!(
            split.slow.fields as usize, d.over_budget,
            "the split's slow bucket must equal the headline over-budget count"
        );
    }

    /// The mechanism-naming case: slow fields carrying more graphics tasks.
    /// The per-field ratio must show it, and must not be confused by the
    /// buckets having different sizes.
    #[test]
    fn a_counter_that_differs_between_populations_reads_as_a_ratio_per_field() {
        let fast = Counters {
            gfx_tasks: 1,
            ..Counters::default()
        };
        let slow = Counters {
            gfx_tasks: 3,
            ..Counters::default()
        };
        // Deliberately unequal bucket sizes: 80 fast, 20 slow. A per-BUCKET
        // total would read 80 vs 60 and invert the finding.
        let mut samples: Vec<FieldSample> =
            (0..80).map(|_| sample_with(10.0, fast)).collect();
        samples.extend((0..20).map(|_| sample_with(40.0, slow)));

        let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");
        assert_eq!(split.fast.counters.gfx_tasks, 80);
        assert_eq!(split.slow.counters.gfx_tasks, 60);

        let text = population_report(&split);
        assert!(
            text.contains("<== DIFFERS"),
            "a 3x per-field difference must be flagged; got:\n{text}"
        );
        assert!(
            text.contains("ratio=  3.000x"),
            "the ratio must be PER FIELD (3/1), not per bucket (60/80); got:\n{text}"
        );
        assert!(
            !text.contains("NO COUNTER DISTINGUISHES"),
            "a counter does distinguish them here"
        );
    }

    /// The bucket accumulator (`Bucket::from_samples`'s `add!`) is a
    /// hand-maintained field list, and a counter omitted from it accumulates
    /// to ZERO in both populations while `labelled()` dutifully prints it --
    /// which reads as "this counter does not distinguish the populations",
    /// the single most misleading output this module can produce. Adding the
    /// executor-split counters hit exactly that: they were in the struct, the
    /// sampler and the report, and silently absent from `add!`.
    ///
    /// This pins it generically. Every field `labelled()` reports must survive
    /// a round trip through `from_samples`, so a future counter cannot be
    /// added to the struct and forgotten in the accumulator.
    #[test]
    fn every_labelled_counter_survives_bucket_accumulation() {
        // 1 in every field, so a surviving counter sums to the field count and
        // a dropped one sums to zero -- unambiguous either way.
        let ones = Counters {
            gfx_tasks: 1,
            audio_tasks: 1,
            executor_ns: 1,
            gfx_ns: 1,
            gfx_lle_ns: 1,
            gfx_lle_rsp_ns: 1,
            gfx_lle_rdp_ns: 1,
            vi_present_ns: 1,
            audio_lle_ns: 1,
            executor_calls: 1,
            gfx_calls: 1,
            gfx_lle_calls: 1,
            audio_lle_calls: 1,
            rsp_steps_gfx: 1,
            rsp_steps_audio: 1,
            rsp_entries: 1,
            dpc_calls: 1,
            barrier_served: 1,
            barrier_fell_back: 1,
            barrier_dirty_pages: 1,
            barrier_clean: 1,
            exec_mirror_ns: 1,
            exec_resume_ns: 1,
            exec_devtime_ns: 1,
            exec_guard_suspend_ns: 1,
            exec_guard_device_ns: 1,
            exec_mirror_calls: 1,
            exec_guard_suspend_calls: 1,
            exec_guard_device_calls: 1,
            resume_reconcile_ns: 1,
            resume_cop0_ns: 1,
            resume_dispatch_ns: 1,
            resume_invalidate_ns: 1,
            resume_exit_ns: 1,
            resume_suspend_ns: 1,
            resume_resolve_ns: 1,
            resume_hostcall_ns: 1,
            resume_hostcall_calls: 1,
            resume_reconcile_calls: 1,
            resume_dispatch_calls: 1,
            vi_present_in_executor_calls: 1,
            vi_present_outside_executor_calls: 1,
            dpc_alloc_ns: 1,
            dpc_copy_in_ns: 1,
            dpc_copy_back_ns: 1,
        };
        let samples: Vec<FieldSample> = (0..10).map(|_| sample_with(20.0, ones)).collect();
        let refs: Vec<&FieldSample> = samples.iter().collect();
        let bucket = Bucket::from_samples(&refs);
        for (label, value) in bucket.counters.labelled() {
            assert_eq!(
                value, 10,
                "`{label}` is in `labelled()` but did not accumulate through \
                 `Bucket::from_samples`'s `add!` list -- it would read zero in BOTH \
                 populations and be reported as 'does not distinguish them'"
            );
        }
    }

    /// Rule 6a: before trusting the split, confirm it can FAIL. An unarmed
    /// run must say ABSENT, not print zeros -- a zero decomposition is
    /// indistinguishable from "the apparatus costs nothing", which is the
    /// exact wrong conclusion this instrument exists to test for.
    #[test]
    fn an_unarmed_executor_split_reports_absent_rather_than_zero() {
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = executor_split_report(&split.fast, &split.slow);
        assert!(
            text.contains("NOT ARMED"),
            "an unarmed split must say so; got:\n{text}"
        );
        assert!(
            !text.contains("APPARATUS"),
            "an unarmed split must not print an apparatus share; got:\n{text}"
        );
    }

    /// The arithmetic the whole section exists for: nested counters are
    /// SUBTRACTED, never summed. Pinned with numbers where a summing bug is
    /// visible -- resume 10ms containing mirror 4ms and guard 3ms leaves a NET
    /// of 3ms, and an implementation that treated them as peers would report
    /// 17ms of phases inside a 12ms executor, which is impossible.
    #[test]
    fn nested_executor_phases_are_subtracted_not_summed() {
        // Per field, in ns: executor 12ms = resume 10ms + devtime 1.5ms
        // + 0.5ms residual. Inside resume: mirror 4ms, guard@suspend 3ms.
        let counters = Counters {
            executor_ns: 12_000_000,
            executor_calls: 100,
            exec_resume_ns: 10_000_000,
            exec_devtime_ns: 1_500_000,
            exec_mirror_ns: 4_000_000,
            exec_mirror_calls: 100,
            exec_guard_suspend_ns: 3_000_000,
            exec_guard_suspend_calls: 400,
            exec_guard_device_ns: 500_000,
            exec_guard_device_calls: 10,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| {
                sample_with(
                    if i % 2 == 0 { 40.0 } else { 10.0 },
                    // Both populations carry identical per-field work here;
                    // this test is about the arithmetic, not the split.
                    counters,
                )
            })
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = executor_split_report(&split.fast, &split.slow);

        // resume NET = 10 - 4 - 3 = 3ms, NOT 10 and NOT 17.
        assert!(
            text.contains("resume NET") && text.contains("3.000ms/field"),
            "resume net must subtract the nested phases (10-4-3=3); got:\n{text}"
        );
        // APPARATUS = mirror 4 + guard@suspend 3 + guard@device 0.5 = 7.5ms.
        assert!(
            text.contains("7.500ms/field"),
            "apparatus must sum the three guard phases (4+3+0.5=7.5); got:\n{text}"
        );
        // Residual = 12 - 10 - 1.5 = 0.5ms, reported rather than dropped.
        assert!(
            text.contains("residual") && text.contains("0.500ms/field"),
            "the run_one_step residual must be reported (12-10-1.5=0.5); got:\n{text}"
        );
        assert!(
            text.contains("[nested]"),
            "nested rows must be marked so they are not read as peers; got:\n{text}"
        );
    }

    /// Rule 6a one level deeper: an unarmed `resume NET` split must say ABSENT
    /// rather than print a decomposition of zeros. A zero here would read as
    /// "translated guest code costs nothing", which is the most misleading
    /// sentence this module could emit.
    #[test]
    fn an_unarmed_resume_split_reports_absent_rather_than_zero() {
        let split = PopulationSplit::from_samples(
            &(0..40)
                .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
                .collect::<Vec<_>>(),
            true,
        )
        .expect("40 samples");
        let text = resume_split_report(&split.fast, &split.slow);
        assert!(
            text.contains("NOT ARMED") && text.contains("absent, not zero"),
            "an unarmed resume split must distinguish absent from zero; got:\n{text}"
        );
        assert!(
            !text.contains("TRANSLATED GUEST CODE ="),
            "an unarmed split must not print a guest-code figure at all; got:\n{text}"
        );
    }

    /// The arithmetic the whole report rests on: `dispatch` is INCLUSIVE of
    /// graphics and audio, so translated guest code is the subtraction, and
    /// the seven phases must close against a re-derived `resume NET`.
    #[test]
    fn resume_split_subtracts_nested_graphics_and_closes_against_resume_net() {
        // resume NET = resume 20 - mirror 4 - guard@suspend 1 = 15ms.
        // Phases: reconcile 1 + cop0 2 + dispatch 3 + invalidate 0.5
        //       + exit 0.5 + resolve 0.4 + hostcall 7 = 14.4.
        // PARKED = 15 - 14.4 = 0.6.
        // gfx 6 + audio 1 nest inside HOSTCALL (7), not dispatch.
        // TRANSLATED GUEST CODE is dispatch itself = 3.
        let counters = Counters {
            executor_ns: 25_000_000,
            executor_calls: 100,
            exec_resume_ns: 20_000_000,
            exec_mirror_ns: 4_000_000,
            exec_guard_suspend_ns: 1_000_000,
            gfx_ns: 6_000_000,
            audio_lle_ns: 1_000_000,
            resume_reconcile_ns: 1_000_000,
            resume_cop0_ns: 2_000_000,
            resume_dispatch_ns: 3_000_000,
            resume_dispatch_calls: 100,
            resume_invalidate_ns: 500_000,
            resume_exit_ns: 500_000,
            resume_resolve_ns: 400_000,
            resume_hostcall_ns: 7_000_000,
            resume_hostcall_calls: 20,
            vi_present_outside_executor_calls: 7,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = resume_split_report(&split.fast, &split.slow);

        assert!(
            text.contains("resume NET=15.000ms/field"),
            "resume NET must be re-derived as 20-4-1=15, not inherited; got:\n{text}"
        );
        // The headline: dispatch IS translated guest code, 3ms.
        assert!(
            text.contains("TRANSLATED GUEST CODE = 3.000ms/field"),
            "dispatch is translated guest code directly; got:\n{text}"
        );
        // gfx+audio nest in HOSTCALL and must not exceed it -- the inversion
        // that a child exceeding its parent exposed on the real route.
        assert!(
            text.contains("host calls (OS shims)") && text.contains("7.000ms/field"),
            "the host-call bucket must be its own row; got:\n{text}"
        );
        // Closure: 15 - 14.4 = 0.6, named as the suspend gap rather than
        // absorbed into a phase or hidden.
        assert!(
            text.contains("PARKED (other threads ran)") && text.contains("0.600ms/field"),
            "parked time must be reported so the split's closure is visible; got:\n{text}"
        );
        // A POSITIVE gap is expected and must not be flagged as brokenness.
        assert!(
            !text.contains("NEGATIVE GAP"),
            "a positive suspend gap is the normal case, not an instrument fault; got:\n{text}"
        );
    }

    /// The check that caught the real defect, pinned so the fix cannot relax
    /// it. Phases claiming MORE than their own parent contains is impossible,
    /// and it is exactly what a timer spanning the coroutine suspend produced
    /// (-697% on the first smoke run). A positive gap is normal; a negative one
    /// means the instrument is lying and the phases must not be read.
    #[test]
    fn phases_exceeding_resume_net_are_reported_as_a_broken_instrument() {
        let counters = Counters {
            executor_ns: 25_000_000,
            executor_calls: 100,
            exec_resume_ns: 10_000_000,
            // 40ms of phases inside a 10ms parent: impossible.
            resume_dispatch_ns: 40_000_000,
            resume_dispatch_calls: 100,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = resume_split_report(&split.fast, &split.slow);
        assert!(
            text.contains("NEGATIVE GAP") && text.contains("instrument is broken"),
            "phases exceeding resume NET must be called an instrument fault, not a finding; \
             got:\n{text}"
        );
    }

    /// A decomposition that does not close must SAY so. Pre-registered at 5%:
    /// the point of a stated tolerance is that a large residual becomes the
    /// finding rather than being quietly presented as a set of phases.
    #[test]
    fn the_suspend_gap_is_always_printed_even_when_it_dominates() {
        // resume NET = 20ms with only 5ms on-stack: a 15ms suspend gap, 75% of
        // the parent. That is a legitimate outcome once the clock stops at the
        // switch -- but it must be VISIBLE, because an unnamed 75% is exactly
        // how the original 21.72 ms stayed hidden for a session. The report
        // must print it as a row and must not silently absorb it.
        let counters = Counters {
            executor_ns: 25_000_000,
            executor_calls: 100,
            exec_resume_ns: 20_000_000,
            resume_dispatch_ns: 5_000_000,
            resume_dispatch_calls: 100,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = resume_split_report(&split.fast, &split.slow);
        assert!(
            text.contains("PARKED (other threads ran)") && text.contains("15.000ms/field"),
            "a dominating suspend gap must still be printed as its own row; got:\n{text}"
        );
        // It is a gap, not an instrument fault -- do not cry wolf on it.
        assert!(
            !text.contains("NEGATIVE GAP"),
            "a positive gap is not brokenness; got:\n{text}"
        );
    }

    /// A mean-only split cannot tell a flat bucket from a spiky one, and the
    /// owner's complaint is choppiness rather than slowness. This pins that
    /// the spread rows actually distinguish the two: same MEAN, different
    /// distribution, and the report must say so.
    #[test]
    fn the_spread_rows_distinguish_a_spiky_phase_from_a_flat_one_at_equal_mean() {
        // `dispatch` alternates 2ms / 18ms (mean 10, p95/p50 = 9x).
        // `cop0` is a flat 10ms every field (mean 10, p95/p50 = 1x).
        // A mean-only table would show these as identical.
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| {
                let spiky = if i % 2 == 0 { 2_000_000 } else { 18_000_000 };
                sample_with(
                    40.0,
                    Counters {
                        exec_resume_ns: 40_000_000,
                        resume_dispatch_ns: spiky,
                        resume_dispatch_calls: 1,
                        resume_cop0_ns: 10_000_000,
                        ..Counters::default()
                    },
                )
            })
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = resume_split_report(&split.fast, &split.slow);

        // Both phases carry the same total, so the mean rows agree...
        assert!(
            text.contains("SPREAD dispatch (GUEST)"),
            "the spread rows must be emitted; got:\n{text}"
        );
        // ...and the spread rows must not.
        let spiky_line = text
            .lines()
            .find(|l| l.contains("SPREAD dispatch (GUEST)"))
            .expect("dispatch spread row");
        let flat_line = text
            .lines()
            .find(|l| l.contains("SPREAD cop0"))
            .expect("cop0 spread row");
        assert!(
            spiky_line.contains("9.0x"),
            "an alternating 2/18ms phase must report a 9x spread; got:\n{spiky_line}"
        );
        assert!(
            flat_line.contains("1.0x"),
            "a constant phase must report a 1x spread; got:\n{flat_line}"
        );
    }

    /// The VI reachability check must be able to REFUTE its own hypothesis.
    /// A check that reports "confirmed" regardless of the state it inspects is
    /// not a check (rule 6a), so both outcomes are pinned here.
    #[test]
    fn vi_reachability_reports_confirmation_and_refutation_distinctly() {
        let confirming = Counters {
            vi_present_outside_executor_calls: 12,
            ..Counters::default()
        };
        let refuting = Counters {
            vi_present_in_executor_calls: 3,
            vi_present_outside_executor_calls: 9,
            ..Counters::default()
        };
        let render = |counters| {
            let samples: Vec<FieldSample> = (0..40)
                .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
                .collect();
            let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
            resume_split_report(&split.fast, &split.slow)
        };
        let confirmed = render(confirming);
        assert!(
            confirmed.contains("CONFIRMED BY OBSERVATION")
                && confirmed.contains("NOT nested in executor_ns"),
            "all-outside presentations must confirm the claim; got:\n{confirmed}"
        );
        let refuted = render(refuting);
        assert!(
            refuted.contains("REFUTED") && refuted.contains("retract it"),
            "any inside-executor presentation must REFUTE the claim, not soften it; \
             got:\n{refuted}"
        );
        // The two outcomes must be distinguishable, which is the property that
        // makes this a check rather than a restatement.
        assert_ne!(
            confirmed.contains("REFUTED"),
            refuted.contains("REFUTED"),
            "the reachability check must distinguish its two outcomes"
        );
    }

    /// The finding that must not be manufactured away: two populations 4x
    /// apart in wall time doing identical measurable work. The report has to
    /// say so in those words.
    #[test]
    fn identical_work_in_both_populations_reports_no_counter_distinguishes_them() {
        let work = Counters {
            gfx_tasks: 2,
            rsp_steps_gfx: 500,
            barrier_served: 700,
            ..Counters::default()
        };
        let mut samples: Vec<FieldSample> =
            (0..50).map(|_| sample_with(10.0, work)).collect();
        samples.extend((0..50).map(|_| sample_with(40.0, work)));

        let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");
        let text = population_report(&split);
        assert!(
            text.contains("NO COUNTER DISTINGUISHES THE TWO POPULATIONS"),
            "got:\n{text}"
        );
        assert!(!text.contains("<== DIFFERS"), "got:\n{text}");
        // And the split itself is real: the wall times genuinely differ 4x.
        assert!((split.slow.mean_ms / split.fast.mean_ms - 4.0).abs() < 1e-9);
    }

    /// **Perf-method rule 6a**: a check that returns the same answer whatever
    /// the state is not a check. With counter sampling off every counter is
    /// zero in both buckets, which is indistinguishable from "the two
    /// populations do identical work" -- the single most consequential finding
    /// this instrument can report. The unarmed report must therefore say NO
    /// DATA and must NOT say the populations are indistinguishable.
    #[test]
    fn an_unarmed_split_says_no_data_rather_than_no_difference() {
        let mut samples: Vec<FieldSample> = (0..50).map(|_| sample(10.0, 0)).collect();
        samples.extend((0..50).map(|_| sample(40.0, 0)));

        let unarmed = population_report(
            &PopulationSplit::from_samples(&samples, false).expect("100 samples"),
        );
        assert!(unarmed.contains("counters NOT SAMPLED"), "got:\n{unarmed}");
        assert!(
            !unarmed.contains("NO COUNTER DISTINGUISHES"),
            "an unarmed run must not be able to produce the headline negative finding; \
             got:\n{unarmed}"
        );

        // The ten-second test: run the check against the state it must reject.
        // Armed, over the SAME samples, it reaches the opposite verdict --
        // so the label is a function of the observation, not a constant.
        let armed = population_report(
            &PopulationSplit::from_samples(&samples, true).expect("100 samples"),
        );
        assert!(armed.contains("NO COUNTER DISTINGUISHES"), "got:\n{armed}");
        assert!(!armed.contains("counters NOT SAMPLED"), "got:\n{armed}");
    }

    /// Both buckets keep their own latency statistics, and the slow bucket's
    /// p50 is the number that says how far the slow population actually is
    /// from the bar -- the whole-span p50 (which already fits) cannot say it.
    #[test]
    fn each_bucket_reports_its_own_distribution() {
        let mut samples: Vec<FieldSample> = (0..50).map(|_| sample(12.0, 0)).collect();
        samples.extend((0..50).map(|_| sample(39.0, 0)));

        let d = FrameDistribution::from_samples(&samples).expect("100 samples");
        let split = PopulationSplit::from_samples(&samples, true).expect("100 samples");

        assert!(d.p50_ms <= FRAME_BUDGET_MS, "the whole-span p50 fits");
        assert_eq!(split.fast.p50_ms, 12.0);
        assert_eq!(split.slow.p50_ms, 39.0, "and the slow half is 2.3x the bar");
    }

    /// Counter deltas are SUMS over a bucket, unlike `gfx_submits`, which is a
    /// cumulative snapshot and must be differenced. Getting these two
    /// reductions the wrong way round is the error
    /// `span_submits_are_a_difference_not_a_sum` pins for the other one.
    #[test]
    fn bucket_counters_sum_per_field_deltas_rather_than_differencing_snapshots() {
        let delta = Counters {
            rsp_steps_audio: 7,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..10).map(|_| sample_with(40.0, delta)).collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("10 samples");
        assert_eq!(split.slow.counters.rsp_steps_audio, 70, "10 fields x 7");
        assert_eq!(split.fast.advances, 0);
    }

    /// A monotone counter cannot decrease, so an apparent decrease is a bug
    /// elsewhere and must read as zero work -- never as `u64::MAX` work, which
    /// would poison every aggregate downstream of it.
    #[test]
    fn a_counter_going_backwards_saturates_to_zero_rather_than_wrapping() {
        let later = Counters {
            gfx_tasks: 5,
            ..Counters::default()
        };
        let earlier = Counters {
            gfx_tasks: 9,
            ..Counters::default()
        };
        assert_eq!(later.delta(&earlier).gfx_tasks, 0);
    }

    /// Every counter the sampler collects must appear in the report. A field
    /// added to `Counters` and forgotten in `labelled` would be invisible --
    /// and an invisible counter is indistinguishable from one that does not
    /// differ, which is the exact confusion this instrument exists to avoid.
    #[test]
    fn every_counter_field_is_labelled_for_the_report() {
        // Distinct nonzero values, so a duplicated or omitted field shows up
        // as a wrong sum rather than as a coincidental match.
        let counters = Counters {
            gfx_tasks: 1,
            audio_tasks: 2,
            executor_ns: 4,
            gfx_ns: 8,
            gfx_lle_ns: 16,
            gfx_lle_rsp_ns: 32,
            gfx_lle_rdp_ns: 64,
            vi_present_ns: 128,
            audio_lle_ns: 256,
            executor_calls: 512,
            gfx_calls: 1024,
            gfx_lle_calls: 2048,
            audio_lle_calls: 4096,
            rsp_steps_gfx: 8192,
            rsp_steps_audio: 16384,
            rsp_entries: 32768,
            dpc_calls: 65536,
            barrier_served: 131_072,
            barrier_fell_back: 262_144,
            barrier_dirty_pages: 524_288,
            barrier_clean: 1_048_576,
            exec_mirror_ns: 1 << 21,
            exec_resume_ns: 1 << 22,
            exec_devtime_ns: 1 << 23,
            exec_guard_suspend_ns: 1 << 24,
            exec_guard_device_ns: 1 << 25,
            exec_mirror_calls: 1 << 26,
            exec_guard_suspend_calls: 1 << 27,
            exec_guard_device_calls: 1 << 28,
            resume_reconcile_ns: 1 << 29,
            resume_cop0_ns: 1 << 30,
            resume_dispatch_ns: 1 << 31,
            resume_invalidate_ns: 1 << 32,
            resume_exit_ns: 1 << 33,
            resume_suspend_ns: 1 << 34,
            resume_resolve_ns: 1 << 35,
            resume_hostcall_ns: 1 << 40,
            resume_hostcall_calls: 1 << 41,
            resume_reconcile_calls: 1 << 36,
            resume_dispatch_calls: 1 << 37,
            vi_present_in_executor_calls: 1 << 38,
            vi_present_outside_executor_calls: 1 << 39,
            dpc_alloc_ns: 1 << 42,
            dpc_copy_in_ns: 1 << 43,
            dpc_copy_back_ns: 1 << 44,
        };
        let labelled = counters.labelled();
        let sum: u64 = labelled.iter().map(|&(_, v)| v).sum();
        assert_eq!(
            sum,
            (1u64 << 45) - 1,
            "each distinct power of two must appear exactly once in `labelled`"
        );
        let mut names: Vec<&str> = labelled.iter().map(|&(n, _)| n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "labels must be unique");
    }

    /// **The decision rule, pinned before any real data existed.**
    ///
    /// Three candidate shapes for a ~50/50 split give three autocorrelations
    /// of opposite sign through this exact estimator. Recording the synthetic
    /// values as a test means the rule cannot be quietly restated once the
    /// measurement lands -- if a future change to `autocorrelation` moved
    /// these, the verdict text would be reinterpreting a different statistic
    /// under the same name.
    #[test]
    fn the_three_candidate_shapes_give_autocorrelations_of_opposite_sign() {
        let alternating: Vec<f64> = (0..1000)
            .map(|i| if i % 2 == 0 { 12.0 } else { 33.0 })
            .collect();
        let blocks: Vec<f64> = (0..1000)
            .map(|i| if (i / 100) % 2 == 0 { 12.0 } else { 33.0 })
            .collect();
        // A fixed 50/50 shuffle, not an RNG: the test must be deterministic.
        let pseudo_random: Vec<f64> = (0..1000)
            .map(|i: u64| {
                let mut h = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                h ^= h >> 29;
                h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                h ^= h >> 32;
                if h % 2 == 0 {
                    12.0
                } else {
                    33.0
                }
            })
            .collect();

        let alt = autocorrelation(&alternating, 1);
        let blk = autocorrelation(&blocks, 1);
        let rnd = autocorrelation(&pseudo_random, 1);

        assert!(alt < -0.9, "strict alternation must read ~-1.0, got {alt}");
        assert!(blk > 0.9, "contiguous blocks must read ~+1.0, got {blk}");
        assert!(
            rnd.abs() < 0.3,
            "an unstructured 50/50 sequence must read near zero, got {rnd}"
        );
        // And the three verdicts they select are genuinely different, which is
        // what makes this a check rather than three numbers.
        for (value, expect) in [
            (alt, "ALTERNATION"),
            (blk, "CONTIGUOUS BLOCKS"),
            (rnd, "NEITHER"),
        ] {
            let p = Periodicity {
                lag1: value,
                lags: [0.0; 6],
                samples: 1000,
                no_submit_fast: 0,
                no_submit_slow: 0,
                submit_fast: 0,
                submit_slow: 0,
                submits_per_submitting_fast: 0.0,
                submits_per_submitting_slow: 0.0,
            };
            let text = periodicity_report(&p);
            assert!(
                text.contains(expect),
                "lag1={value} must select {expect}; got:\n{text}"
            );
        }
    }

    /// Lag-1 is blind to periods other than 2, and the report must not be
    /// read as if it were not. A period-4 sequence is genuinely periodic and
    /// lag-1 places it in the "NEITHER" band -- which is why the raw pattern
    /// string is printed first and the higher lags are reported beside it.
    #[test]
    fn lag_one_cannot_see_a_period_four_cycle_but_lag_four_can() {
        let period_four: Vec<f64> = (0..1000)
            .map(|i| if (i / 2) % 2 == 0 { 12.0 } else { 33.0 })
            .collect();
        let lag1 = autocorrelation(&period_four, 1);
        let lag4 = autocorrelation(&period_four, 4);
        assert!(
            lag1.abs() < 0.3,
            "a period-4 cycle lands in the NEITHER band at lag 1, got {lag1}"
        );
        assert!(
            lag4 > 0.9,
            "but lag 4 sees it clearly, which is why lags 2..=6 are reported; got {lag4}"
        );
    }

    /// A constant sequence has zero variance. The estimator must say "no
    /// variation" rather than emit a `NaN`, which would render as a broken
    /// instrument and could be misread as a missing measurement.
    #[test]
    fn a_constant_sequence_autocorrelates_to_zero_rather_than_nan() {
        let flat = vec![16.0; 100];
        let value = autocorrelation(&flat, 1);
        assert!(value.is_finite(), "got {value}");
        assert_eq!(value, 0.0);
    }

    /// The contingency verdict must be a function of the table, not a
    /// constant printed beside it -- rule 15's second half. Two tables, two
    /// opposite conclusions, from the same code path.
    #[test]
    fn the_contingency_verdict_follows_the_table_in_both_directions() {
        let explains = Periodicity {
            lag1: -0.9,
            lags: [0.0; 6],
            samples: 1000,
            // Slow fields nearly all render, fast fields nearly none.
            no_submit_fast: 490,
            no_submit_slow: 10,
            submit_fast: 10,
            submit_slow: 490,
            submits_per_submitting_fast: 1.0,
            submits_per_submitting_slow: 2.9,
        };
        let text = periodicity_report(&explains);
        assert!(text.contains("submit count EXPLAINS the split"), "{text}");

        let does_not = Periodicity {
            // Both populations render at the same rate.
            no_submit_fast: 250,
            no_submit_slow: 250,
            submit_fast: 250,
            submit_slow: 250,
            ..explains
        };
        let text = periodicity_report(&does_not);
        assert!(
            text.contains("Submits do NOT explain the split"),
            "the same code must reach the opposite verdict on the opposite \
             table; got:\n{text}"
        );
    }

    /// The gate must agree with `write_barrier`'s: an empty value, `0`, and an
    /// absent variable all mean off, so no spelling of "off" reads as on.
    #[test]
    fn only_affirmative_spellings_arm_the_census() {
        let name = "FN64_FRAME_CENSUS_TEST_GATE";
        for off in ["", "0", "no", "off", "false", "  "] {
            std::env::set_var(name, off);
            assert!(!env_flag(name), "{off:?} must read as off");
        }
        for on in ["1", "true", "yes", "on", " ON "] {
            std::env::set_var(name, on);
            assert!(env_flag(name), "{on:?} must read as on");
        }
        std::env::remove_var(name);
        assert!(!env_flag(name), "an absent variable must read as off");
    }

    // ---- FN64_PROFILE composition ------------------------------------
    //
    // These exercise `profile_report` directly rather than through the env
    // gate: `profile::enabled()` memoizes a `OnceLock`, so a test that set the
    // variable would leak into every other test in the binary and would be
    // order-dependent. Calling the formatter with known counters tests the
    // thing that can actually be wrong.

    /// The counters behind the recorded acceptance table, as ns totals over
    /// one field. Slow population, 1.5M-step route.
    fn acceptance_counters() -> Counters {
        Counters {
            // The recorded slow field is 56.23 ms and `resume NET` is 45.687 ms
            // INSIDE it -- 83.2% (perf-method.md:2725-2727, :3064). An earlier
            // draft of this fixture used 45.687 as the FIELD, which made
            // `exec_resume` 55 ms inside a 45.687 ms field: impossible, and the
            // outermost check correctly rejected it. The fixture was wrong,
            // not the check -- which is the check earning its place.
            executor_ns: 56_230_000,
            executor_calls: 100,
            exec_resume_ns: 55_000_000,
            exec_mirror_ns: 8_848_000,
            exec_guard_suspend_ns: 465_000,
            resume_dispatch_ns: 9_528_000,
            resume_dispatch_calls: 100,
            resume_hostcall_ns: 32_700_000,
            gfx_ns: 32_119_000,
            gfx_lle_ns: 32_033_000,
            gfx_lle_rsp_ns: 5_637_000,
            gfx_lle_rdp_ns: 26_396_000,
            resume_reconcile_ns: 1_000_000,
            resume_cop0_ns: 900_000,
            resume_invalidate_ns: 500_000,
            resume_exit_ns: 400_000,
            resume_resolve_ns: 300_000,
            // Witnesses the DPC census channel; without it the report
            // correctly refuses, which is the check working.
            rsp_entries: 100,
            dpc_alloc_ns: 120_000,
            dpc_copy_in_ns: 900_000,
            dpc_copy_back_ns: 750_000,
            ..Counters::default()
        }
    }

    fn acceptance_split() -> PopulationSplit {
        let counters = acceptance_counters();
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 56.230 } else { 10.0 }, counters))
            .collect();
        PopulationSplit::from_samples(&samples, true).expect("40 samples")
    }

    /// The sequence channel is the one constituent with no counter to witness:
    /// it is a request for N fields, so "did it arm" is "was a length asked
    /// for". `sequence_dump_len` memoizes a `OnceLock`, so a test cannot set
    /// the variable and observe an effect -- this arms it once for the whole
    /// test binary, before any test reads it.
    ///
    /// Without this the report correctly REFUSES in every profile test, which
    /// is the check doing its job rather than a bug.
    fn arm_sequence_channel_for_tests() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            if std::env::var_os("FN64_FRAME_CENSUS_SEQUENCE").is_none() {
                // SAFETY: test-only, and `Once` serializes it against the
                // other profile tests that call this first.
                unsafe { std::env::set_var("FN64_FRAME_CENSUS_SEQUENCE", "400") };
            }
        });
        // Force the memoized read now, so it cannot be captured as 0 later.
        assert!(sequence_dump_len() > 0, "sequence channel must arm for these tests");
    }

    /// EVERY ROW STATES BOTH DENOMINATORS. This is the fix for the single most
    /// consequential error: "20.9% of resume NET" is what let guest code be
    /// named the target, when "0.57x budget" alongside it would have shown
    /// three modest-looking rows summing past the budget.
    #[test]
    fn the_profile_report_states_both_denominators_on_every_row() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        let guest = text
            .lines()
            .find(|l| l.contains("TRANSLATED GUEST CODE"))
            .expect("guest-code row present");
        assert!(guest.contains("9.528ms/field"), "{guest}");
        assert!(guest.contains("% of resume NET"), "share of parent: {guest}");
        assert!(guest.contains("x budget"), "ratio to budget: {guest}");
    }

    /// The rows are SUMMED in code, against the denominator the decision uses.
    /// Every individual number can be right while the sum contradicts the
    /// conclusion drawn from them.
    #[test]
    fn the_profile_report_sums_the_rows_against_the_budget() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        assert!(
            text.contains("SUM OF THE ROWS ABOVE"),
            "the report must add the rows up, not leave it to the eye:\n{text}",
        );
    }

    /// Percentiles, never a bare mean: a mean has hidden the distribution
    /// twice on this project.
    #[test]
    fn the_profile_report_gives_percentiles_per_population() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        for population in ["fast", "slow"] {
            let line = text
                .lines()
                .find(|l| l.contains(&format!("{population}: fields=")))
                .unwrap_or_else(|| panic!("{population} summary line present in:\n{text}"));
            for stat in ["p50=", "p95=", "p99="] {
                assert!(line.contains(stat), "{population} missing {stat}: {line}");
            }
        }
    }

    /// Provenance travels with the numbers. Reconstructing where a figure came
    /// from cost the worst hours of the evening this was built in.
    #[test]
    fn the_profile_report_carries_its_own_provenance() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        assert!(text.contains("PROVENANCE"), "{text}");
        assert!(text.contains("binary:"), "{text}");
        assert!(text.contains("route:"), "{text}");
    }

    /// The scope legend is present at the point of reading, because six tags
    /// carry `gfx_submits` with four different meanings.
    #[test]
    fn the_profile_report_disambiguates_colliding_names() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        assert!(text.contains("NAME SCOPES"), "{text}");
        assert!(text.contains("NOT a contradiction"), "{text}");
    }

    /// THE CHECK, end to end: a child exceeding its parent must refuse to
    /// print that subtree rather than presenting it as a finding. This is the
    /// `gfx_ns` 21.5 > parent 7.7 defect, which a human caught by eye.
    #[test]
    fn the_profile_report_refuses_a_subtree_whose_child_exceeds_its_parent() {
        arm_sequence_channel_for_tests();
        let counters = Counters {
            executor_ns: 40_000_000,
            executor_calls: 100,
            exec_resume_ns: 38_000_000,
            resume_dispatch_ns: 1_000_000,
            resume_dispatch_calls: 100,
            // 21.5ms of graphics inside a 7.7ms host-call parent: impossible.
            resume_hostcall_ns: 7_700_000,
            gfx_ns: 21_500_000,
            // Witness for the DPC channel: without it the report refuses on
            // an unarmed channel BEFORE reaching the tree check under test.
            rsp_entries: 100,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = profile_report(&split);
        assert!(
            text.contains("TREE VIOLATION") && text.contains("REFUSING"),
            "a child exceeding its parent must refuse the subtree:\n{text}",
        );
        assert!(
            !text.contains("(of) RDP rasterization"),
            "the refused subtree's rows must NOT be printed:\n{text}",
        );
    }

    /// A healthy decomposition must NOT trip the refusal, or the check fires
    /// always and means nothing (rule 6a).
    #[test]
    fn a_healthy_profile_report_prints_its_rows() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        println!("{text}");
        assert!(
            !text.contains("TREE VIOLATION"),
            "the acceptance counters must close:\n{text}",
        );
        assert!(text.contains("TRANSLATED GUEST CODE"), "{text}");
        assert!(text.contains("RDP rasterization"), "{text}");
    }

    /// An unarmed channel must refuse the whole report rather than present a
    /// plausible subset, and must NAME the gate that failed.
    #[test]
    fn the_profile_report_refuses_when_a_channel_did_not_arm() {
        // Population counters never sampled: `armed: false`.
        let samples: Vec<FieldSample> = (0..40)
            .map(|i| sample_with(if i % 2 == 0 { 40.0 } else { 10.0 }, Counters::default()))
            .collect();
        let split = PopulationSplit::from_samples(&samples, false).expect("40 samples");
        let text = profile_report(&split);
        assert!(
            text.contains("REFUSING TO PRINT"),
            "a partial profile must be refused:\n{text}",
        );
        assert!(
            text.contains("FN64_FRAME_CENSUS_POPULATIONS"),
            "the refusal must name the missing gate:\n{text}",
        );
        assert!(
            !text.contains("TRANSLATED GUEST CODE"),
            "no rows may be printed alongside a refusal:\n{text}",
        );
    }

    /// THE OUTERMOST CHECK, which the parent/child tree cannot make.
    ///
    /// Found by reading this report's own first real output: the fast bucket
    /// claimed `resume NET` = 45.687 ms inside a measured 10.000 ms field --
    /// 2.74x the budget of phases inside a 0.60x field. Every parent/child
    /// relation held, because the field's wall time is not a counter and so is
    /// not in the tree. An impossible decomposition passed the check that
    /// exists to catch impossible decompositions.
    #[test]
    fn a_decomposition_larger_than_its_own_field_is_caught() {
        arm_sequence_channel_for_tests();
        // 45ms of phases inside a 10ms field: impossible, and invisible to
        // every parent/child test because the tree closes internally.
        let counters = Counters {
            executor_ns: 45_000_000,
            executor_calls: 100,
            exec_resume_ns: 45_000_000,
            resume_dispatch_ns: 45_000_000,
            resume_dispatch_calls: 100,
            rsp_entries: 100,
            ..Counters::default()
        };
        let samples: Vec<FieldSample> = (0..40)
            .map(|_| sample_with(10.0, counters))
            .collect();
        let split = PopulationSplit::from_samples(&samples, true).expect("40 samples");
        let text = profile_report(&split);
        assert!(
            text.contains("DECOMPOSITION EXCEEDS ITS FIELD"),
            "phases larger than the field they sit in must be refused:\n{text}",
        );
        // And the tree itself must be silent here -- proving this check is
        // catching something the parent/child relations genuinely cannot.
        let ms = |ns: u64| ns as f64 / 1.0e6 / 40.0;
        let lookup = |c: &str| match c {
            "executor_ns" | "exec_resume_ns" => ms(counters.executor_ns * 40),
            "resume_dispatch_ns" => ms(counters.resume_dispatch_ns * 40),
            _ => 0.0,
        };
        assert!(
            crate::counter_tree::validate(&lookup).is_empty(),
            "premise of this test: the tree closes internally, so only the \
             field-level check can catch it",
        );
    }

    /// A decomposition that FITS its field must not trip the outermost check,
    /// or it fires always and means nothing.
    #[test]
    fn a_decomposition_within_its_field_is_not_flagged() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        let slow_section = text
            .split("slow: fields=")
            .nth(1)
            .expect("slow section present");
        assert!(
            !slow_section.contains("DECOMPOSITION EXCEEDS ITS FIELD"),
            "45.687ms of resume NET inside a 56.230ms field must be accepted:\n{slow_section}",
        );
    }

    /// The mirror is a SIBLING of resume NET while being nested inside
    /// `exec_resume_ns`. That single relationship is what prose made
    /// confusable and three hand-written report sites got wrong.
    #[test]
    fn the_profile_report_places_the_mirror_beside_resume_net() {
        arm_sequence_channel_for_tests();
        let text = profile_report(&acceptance_split());
        let mirror = text
            .lines()
            .find(|l| l.contains("mirror boundary"))
            .expect("mirror row present");
        assert!(mirror.contains("sibling of NET"), "{mirror}");
        assert!(mirror.contains("8.848ms/field"), "{mirror}");
    }
}
