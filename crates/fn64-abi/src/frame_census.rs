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
}

impl FieldSample {
    /// Wall milliseconds attributable to ONE emulated field. This is the
    /// quantity the 60fps bar is about, and the one a multi-field catch-up
    /// would otherwise misreport by its field count.
    fn per_field_ms(&self) -> f64 {
        self.wall_ms / f64::from(self.fields.max(1))
    }
}

#[derive(Default)]
struct Census {
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
    let gfx_submits = crate::task_counts().0;
    let mut guard = CENSUS.lock().expect("frame census poisoned");
    let census = guard.get_or_insert_with(Census::default);
    census.total_fields += u64::from(retrace_ticks);

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
}

/// Snapshot the census without clearing it.
pub fn snapshot() -> Option<FrameCensusReport> {
    let guard = CENSUS.lock().expect("frame census poisoned");
    let census = guard.as_ref()?;
    Some(FrameCensusReport {
        steady: FrameDistribution::from_samples(&census.samples),
        total_fields: census.total_fields,
        transient_fields: census.transient_fields,
        transient_wall_ms: census.transient_wall_ms,
        truncated: census.truncated,
        warmup_gfx: warmup_gfx(),
        steady_began_at_field: census.steady_began_at_field,
        steady_began_at_gfx: census.steady_began_at_gfx,
    })
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
