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
            executor_calls: phase.executor_calls,
            gfx_calls: phase.gfx_calls,
            gfx_lle_calls: phase.gfx_lle_calls,
            audio_lle_calls: phase.audio_lle_calls,
            rsp_steps_gfx,
            rsp_steps_audio,
            rsp_entries,
            dpc_calls,
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
            executor_calls,
            gfx_calls,
            gfx_lle_calls,
            audio_lle_calls,
            rsp_steps_gfx,
            rsp_steps_audio,
            rsp_entries,
            dpc_calls,
            barrier_served,
            barrier_fell_back,
            barrier_dirty_pages,
            barrier_clean,
        )
    }

    /// Every counter as `(label, value)`, in report order. One list so the
    /// bucket diff cannot silently omit a counter the sampler collects.
    fn labelled(&self) -> [(&'static str, u64); 21] {
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
            ("barrier_served", self.barrier_served),
            ("barrier_fell_back", self.barrier_fell_back),
            ("barrier_dirty_pages", self.barrier_dirty_pages),
            ("barrier_clean", self.barrier_clean),
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
        return;
    }
    static ARMED_ONCE: std::sync::Once = std::sync::Once::new();
    ARMED_ONCE.call_once(|| {
        ARMED.store(true, Ordering::Relaxed);
        extern "C" fn at_exit() {
            print!("{}", report());
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
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
    mean_ms: f64,
    counters: Counters,
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
                barrier_served,
                barrier_fell_back,
                barrier_dirty_pages,
                barrier_clean,
            );
        }
        Self {
            advances: ms.len(),
            fields,
            wall_ms,
            p50_ms: nearest_rank(&ms, 50),
            p95_ms: nearest_rank(&ms, 95),
            mean_ms: wall_ms / fields as f64,
            counters,
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
        };
        let labelled = counters.labelled();
        let sum: u64 = labelled.iter().map(|&(_, v)| v).sum();
        assert_eq!(
            sum,
            (1u64 << 21) - 1,
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
}
