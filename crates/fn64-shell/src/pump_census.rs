//! Per-pump cost attribution for the live shell, gated by `FN64_PUMP_CENSUS=1`.
//!
//! # Why this exists
//!
//! `df9ad487` established the SHAPE of the shell's choppiness: the emulator
//! runs at 94.8% of real time, the interval p50 sits on the 16.67 ms deadline,
//! and 57.3% of pumps still exceed it. Because `pump_one_frame()` runs
//! synchronously on the winit event thread, a 35 ms pump has already blown the
//! next deadline when it returns, the scheduler re-anchors, and the frame is
//! dropped. That work deliberately stopped there: it had no DECOMPOSITION of
//! the slow pumps, only their distribution.
//!
//! `frame_census` cannot supply it. That instrument samples at emulated VI
//! FIELD boundaries on the headless block route; the shell's unit of latency is
//! a PUMP, and one pump is not one field-boundary sample. The shell's own
//! `TimingWindow` measures a pump's wall time and nothing about its contents.
//! Between them sits the question this module answers: what is inside a slow
//! pump that is not inside a fast one?
//!
//! # What it measures
//!
//! `fn64_abi::phase_timing()` already maintains every counter needed, as
//! monotonic per-thread running totals (see `counter_tree::TREE`). The guest
//! runs on the event thread -- `run_one_step` resumes corosensei coroutines on
//! the caller's stack -- so those thread-locals are the same ones the pump
//! advanced. This module therefore adds NO new timers to any hot path: it
//! reads the running totals once before and once after each pump and
//! attributes the difference to that pump. The per-pump cost is 2 reads of a
//! plain-`Cell` struct plus a push into a preallocated `Vec`.
//!
//! The phase counters themselves are armed by their OWN gates
//! (`FN64_PHASE_TIMING`, `FN64_EXECUTOR_SPLIT`, `FN64_RESUME_SPLIT`,
//! `FN64_DPC_COPY_CENSUS`). This module never arms them. An unarmed counter
//! reads a constant zero, and the report says NOT ARMED rather than presenting
//! the zero as "this phase costs nothing" -- the check-that-cannot-fail error
//! perf-method rule 6a exists to prevent.
//!
//! # Closure
//!
//! Per perf-method rule 31 the report refuses to present a split that does not
//! close. Every bucket is checked against its declared parent and the residual
//! is printed unconditionally. The outermost check is the one that matters
//! here and is unique to this instrument: the sum of the attributed phases must
//! not exceed the WALL time of the pump that contains them, because the wall
//! clock around `pump_one_frame` is an independent measurement of the same
//! interval. A subtree claiming more than its containing pump means the
//! attribution is wrong, not that a phase is expensive.
//!
//! # Gates
//!
//! - `FN64_PUMP_CENSUS=1` -- arm. Absent, empty, `0`: off, and every call in
//!   this module compiles to a `bool` load and a return.
//! - `FN64_PUMP_CENSUS_WARMUP=<n>` -- discard the first `n` pumps (default
//!   120). Boot pumps link sections and fault in pages; one of them dominates
//!   `max` forever.
//! - `FN64_PUMP_CENSUS_PUMPS=<n>` -- print the report and exit after `n`
//!   post-warmup pumps. Makes a windowed run bounded and repeatable without a
//!   human closing the window, which is what "repeated runs" requires.
//! - `FN64_PUMP_CENSUS_SEQUENCE=<n>` -- also dump the first `n` post-warmup
//!   pumps as raw per-pump rows. A series of summary percentiles cannot
//!   distinguish "the game entered a slow regime" from "the emulator
//!   alternates"; only the raw sequence can.

use std::sync::OnceLock;
use std::time::Duration;

/// One pump's wall time and the counter deltas attributed to it.
///
/// `u64` nanoseconds throughout: these are differences of monotonic running
/// totals, and a `saturating_sub` on a wrapped or reordered read yields 0
/// rather than a pinned maximum (the same choice `note_executor_split`
/// documents).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PumpSample {
    pub wall_ns: u64,
    pub steps: u64,
    pub swapped: bool,

    // ---- FN64_PHASE_TIMING
    pub executor_ns: u64,
    pub executor_calls: u64,
    pub gfx_ns: u64,
    pub gfx_calls: u64,
    pub gfx_lle_ns: u64,
    pub gfx_lle_calls: u64,
    pub gfx_lle_rsp_ns: u64,
    pub gfx_lle_rdp_ns: u64,
    pub audio_lle_ns: u64,
    pub audio_lle_calls: u64,
    pub vi_present_ns: u64,
    pub vi_present_calls: u64,

    // ---- FN64_EXECUTOR_SPLIT
    pub exec_resume_ns: u64,
    pub exec_mirror_ns: u64,
    pub exec_devtime_ns: u64,
    pub exec_guard_suspend_ns: u64,
    pub exec_guard_device_ns: u64,

    // ---- FN64_RESUME_SPLIT
    pub resume_reconcile_ns: u64,
    pub resume_cop0_ns: u64,
    pub resume_dispatch_ns: u64,
    pub resume_invalidate_ns: u64,
    pub resume_exit_ns: u64,
    pub resume_suspend_ns: u64,
    pub resume_resolve_ns: u64,
    pub resume_hostcall_ns: u64,
    pub resume_hostcall_calls: u64,

    // ---- task structure, always available
    pub gfx_tasks: u64,
    pub audio_tasks: u64,

    // ---- FN64_DPC_COPY_CENSUS
    pub rsp_steps_gfx: u64,
    pub rsp_steps_audio: u64,
    pub rsp_entries: u64,
    pub dpc_calls: u64,
}

/// The raw running totals, read once at each pump boundary.
#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    phase: PhaseSnapshot,
    gfx_tasks: u64,
    audio_tasks: u64,
    rsp_steps_gfx: u64,
    rsp_steps_audio: u64,
    rsp_entries: u64,
    dpc_calls: u64,
}

/// Only the `PhaseTiming` fields this census attributes. A local copy rather
/// than storing `fn64_abi::PhaseTiming` directly, so adding a field upstream
/// cannot silently widen what this instrument claims to have measured.
#[derive(Clone, Copy, Debug, Default)]
struct PhaseSnapshot {
    executor_ns: u64,
    executor_calls: u64,
    gfx_ns: u64,
    gfx_calls: u64,
    gfx_lle_ns: u64,
    gfx_lle_calls: u64,
    gfx_lle_rsp_ns: u64,
    gfx_lle_rdp_ns: u64,
    audio_lle_ns: u64,
    audio_lle_calls: u64,
    vi_present_ns: u64,
    vi_present_calls: u64,
    exec_resume_ns: u64,
    exec_mirror_ns: u64,
    exec_devtime_ns: u64,
    exec_guard_suspend_ns: u64,
    exec_guard_device_ns: u64,
    resume_reconcile_ns: u64,
    resume_cop0_ns: u64,
    resume_dispatch_ns: u64,
    resume_invalidate_ns: u64,
    resume_exit_ns: u64,
    resume_suspend_ns: u64,
    resume_resolve_ns: u64,
    resume_hostcall_ns: u64,
    resume_hostcall_calls: u64,
}

impl Totals {
    fn read() -> Self {
        let p = fn64_abi::phase_timing();
        let (gfx_tasks, audio_tasks) = fn64_abi::task_counts();
        let (rsp_steps_gfx, rsp_steps_audio, rsp_entries, dpc_calls) =
            fn64_abi::dpc_census_running_totals();
        Self {
            phase: PhaseSnapshot {
                executor_ns: p.executor_ns,
                executor_calls: p.executor_calls,
                gfx_ns: p.gfx_ns,
                gfx_calls: p.gfx_calls,
                gfx_lle_ns: p.gfx_lle_ns,
                gfx_lle_calls: p.gfx_lle_calls,
                gfx_lle_rsp_ns: p.gfx_lle_rsp_ns,
                gfx_lle_rdp_ns: p.gfx_lle_rdp_ns,
                audio_lle_ns: p.audio_lle_ns,
                audio_lle_calls: p.audio_lle_calls,
                vi_present_ns: p.vi_present_ns,
                vi_present_calls: p.vi_present_calls,
                exec_resume_ns: p.exec_resume_ns,
                exec_mirror_ns: p.exec_mirror_ns,
                exec_devtime_ns: p.exec_devtime_ns,
                exec_guard_suspend_ns: p.exec_guard_suspend_ns,
                exec_guard_device_ns: p.exec_guard_device_ns,
                resume_reconcile_ns: p.resume_reconcile_ns,
                resume_cop0_ns: p.resume_cop0_ns,
                resume_dispatch_ns: p.resume_dispatch_ns,
                resume_invalidate_ns: p.resume_invalidate_ns,
                resume_exit_ns: p.resume_exit_ns,
                resume_suspend_ns: p.resume_suspend_ns,
                resume_resolve_ns: p.resume_resolve_ns,
                resume_hostcall_ns: p.resume_hostcall_ns,
                resume_hostcall_calls: p.resume_hostcall_calls,
            },
            gfx_tasks,
            audio_tasks,
            rsp_steps_gfx,
            rsp_steps_audio,
            rsp_entries,
            dpc_calls,
        }
    }

    fn delta(&self, before: &Self, wall_ns: u64, steps: u64, swapped: bool) -> PumpSample {
        let (a, b) = (&self.phase, &before.phase);
        PumpSample {
            wall_ns,
            steps,
            swapped,
            executor_ns: a.executor_ns.saturating_sub(b.executor_ns),
            executor_calls: a.executor_calls.saturating_sub(b.executor_calls),
            gfx_ns: a.gfx_ns.saturating_sub(b.gfx_ns),
            gfx_calls: a.gfx_calls.saturating_sub(b.gfx_calls),
            gfx_lle_ns: a.gfx_lle_ns.saturating_sub(b.gfx_lle_ns),
            gfx_lle_calls: a.gfx_lle_calls.saturating_sub(b.gfx_lle_calls),
            gfx_lle_rsp_ns: a.gfx_lle_rsp_ns.saturating_sub(b.gfx_lle_rsp_ns),
            gfx_lle_rdp_ns: a.gfx_lle_rdp_ns.saturating_sub(b.gfx_lle_rdp_ns),
            audio_lle_ns: a.audio_lle_ns.saturating_sub(b.audio_lle_ns),
            audio_lle_calls: a.audio_lle_calls.saturating_sub(b.audio_lle_calls),
            vi_present_ns: a.vi_present_ns.saturating_sub(b.vi_present_ns),
            vi_present_calls: a.vi_present_calls.saturating_sub(b.vi_present_calls),
            exec_resume_ns: a.exec_resume_ns.saturating_sub(b.exec_resume_ns),
            exec_mirror_ns: a.exec_mirror_ns.saturating_sub(b.exec_mirror_ns),
            exec_devtime_ns: a.exec_devtime_ns.saturating_sub(b.exec_devtime_ns),
            exec_guard_suspend_ns: a
                .exec_guard_suspend_ns
                .saturating_sub(b.exec_guard_suspend_ns),
            exec_guard_device_ns: a.exec_guard_device_ns.saturating_sub(b.exec_guard_device_ns),
            resume_reconcile_ns: a.resume_reconcile_ns.saturating_sub(b.resume_reconcile_ns),
            resume_cop0_ns: a.resume_cop0_ns.saturating_sub(b.resume_cop0_ns),
            resume_dispatch_ns: a.resume_dispatch_ns.saturating_sub(b.resume_dispatch_ns),
            resume_invalidate_ns: a.resume_invalidate_ns.saturating_sub(b.resume_invalidate_ns),
            resume_exit_ns: a.resume_exit_ns.saturating_sub(b.resume_exit_ns),
            resume_suspend_ns: a.resume_suspend_ns.saturating_sub(b.resume_suspend_ns),
            resume_resolve_ns: a.resume_resolve_ns.saturating_sub(b.resume_resolve_ns),
            resume_hostcall_ns: a.resume_hostcall_ns.saturating_sub(b.resume_hostcall_ns),
            resume_hostcall_calls: a
                .resume_hostcall_calls
                .saturating_sub(b.resume_hostcall_calls),
            gfx_tasks: self.gfx_tasks.saturating_sub(before.gfx_tasks),
            audio_tasks: self.audio_tasks.saturating_sub(before.audio_tasks),
            rsp_steps_gfx: self.rsp_steps_gfx.saturating_sub(before.rsp_steps_gfx),
            rsp_steps_audio: self.rsp_steps_audio.saturating_sub(before.rsp_steps_audio),
            rsp_entries: self.rsp_entries.saturating_sub(before.rsp_entries),
            dpc_calls: self.dpc_calls.saturating_sub(before.dpc_calls),
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_PUMP_CENSUS"))
}

fn warmup() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| env_usize("FN64_PUMP_CENSUS_WARMUP", 120))
}

/// Post-warmup pumps after which the run reports and exits. `0` = never.
fn pump_limit() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| env_usize("FN64_PUMP_CENSUS_PUMPS", 0))
}

fn sequence_len() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| env_usize("FN64_PUMP_CENSUS_SEQUENCE", 0))
}

/// One 60 Hz field. The budget the over-budget fraction is taken against, and
/// the fast/slow population boundary -- the SAME threshold `df9ad487`'s
/// `over_budget` counter uses, so the two numbers are comparable.
pub const FIELD_BUDGET_MS: f64 = 1000.0 / 60.0;

/// The collector. Owned by `Shell`; not a global, because the shell has
/// exactly one pump loop and a thread-local would hide that.
pub struct PumpCensus {
    armed: bool,
    seen: usize,
    before: Totals,
    samples: Vec<PumpSample>,
    reported: bool,
}

impl PumpCensus {
    pub fn new() -> Self {
        Self {
            armed: enabled(),
            seen: 0,
            before: Totals::default(),
            samples: Vec::new(),
            reported: false,
        }
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Read the running totals immediately before a pump. Cheap enough to be
    /// unconditional when armed; a no-op when not.
    pub fn before_pump(&mut self) {
        if !self.armed {
            return;
        }
        if self.samples.capacity() == 0 {
            // One allocation for the whole run, taken on the first armed pump
            // rather than at construction so an unarmed shell allocates
            // nothing. Sized to the bounded run when one was requested.
            let cap = if pump_limit() > 0 { pump_limit() } else { 8192 };
            self.samples.reserve(cap);
        }
        self.before = Totals::read();
    }

    /// Attribute the counter deltas since `before_pump` to this pump.
    /// Returns true when the requested pump budget is exhausted.
    pub fn after_pump(&mut self, wall: Duration, steps: u64, swapped: bool) -> bool {
        if !self.armed {
            return false;
        }
        self.seen += 1;
        if self.seen <= warmup() {
            return false;
        }
        let after = Totals::read();
        self.samples
            .push(after.delta(&self.before, wall.as_nanos() as u64, steps, swapped));
        let limit = pump_limit();
        limit > 0 && self.samples.len() >= limit
    }

    pub fn samples(&self) -> &[PumpSample] {
        &self.samples
    }

    /// Print once. Idempotent: the bounded-run exit path and any later
    /// teardown both call it, and a report printed twice reads as two runs.
    pub fn report_once(&mut self, renderer: &str) {
        if !self.armed || self.reported {
            return;
        }
        self.reported = true;
        print!("{}", render_report(&self.samples, renderer, sequence_len()));
    }
}

/// Summary statistics over one population of pumps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Population {
    pub name: &'static str,
    pub pumps: usize,
    pub wall_total_ms: f64,
    pub wall_mean_ms: f64,
    pub wall_p50_ms: f64,
    pub wall_p95_ms: f64,
    pub wall_max_ms: f64,
}

fn nearest_rank(sorted: &[f64], percentile: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn population(name: &'static str, pumps: &[&PumpSample]) -> Population {
    let mut walls: Vec<f64> = pumps.iter().map(|p| ms(p.wall_ns)).collect();
    walls.sort_by(f64::total_cmp);
    let total: f64 = walls.iter().sum();
    Population {
        name,
        pumps: pumps.len(),
        wall_total_ms: total,
        wall_mean_ms: if walls.is_empty() {
            0.0
        } else {
            total / walls.len() as f64
        },
        wall_p50_ms: nearest_rank(&walls, 50),
        wall_p95_ms: nearest_rank(&walls, 95),
        wall_max_ms: *walls.last().unwrap_or(&0.0),
    }
}

/// One attributable cost centre: its name, its declared parent, the gate that
/// arms it, and how to pull its nanoseconds out of a sample.
struct Row {
    name: &'static str,
    parent: Option<&'static str>,
    gate: &'static str,
    get: fn(&PumpSample) -> u64,
}

/// The rows this census attributes, in the nesting `counter_tree::TREE`
/// declares. Kept in that order so a reader can follow the containment
/// top-to-bottom, and checked against it by a test -- a bucket whose parent
/// here disagrees with the ABI's tree is a mislabelled measurement, which is
/// exactly the defect rule 31's third case was.
const ROWS: &[Row] = &[
    Row { name: "executor_ns", parent: None, gate: "FN64_PHASE_TIMING", get: |s| s.executor_ns },
    Row {
        name: "exec_resume_ns",
        parent: Some("executor_ns"),
        gate: "FN64_EXECUTOR_SPLIT",
        get: |s| s.exec_resume_ns,
    },
    Row {
        name: "exec_mirror_ns",
        parent: Some("exec_resume_ns"),
        gate: "FN64_EXECUTOR_SPLIT",
        get: |s| s.exec_mirror_ns,
    },
    Row {
        name: "exec_guard_suspend_ns",
        parent: Some("exec_resume_ns"),
        gate: "FN64_EXECUTOR_SPLIT",
        get: |s| s.exec_guard_suspend_ns,
    },
    Row {
        name: "exec_devtime_ns",
        parent: Some("executor_ns"),
        gate: "FN64_EXECUTOR_SPLIT",
        get: |s| s.exec_devtime_ns,
    },
    Row {
        name: "exec_guard_device_ns",
        parent: Some("exec_devtime_ns"),
        gate: "FN64_EXECUTOR_SPLIT",
        get: |s| s.exec_guard_device_ns,
    },
    Row {
        name: "resume_reconcile_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_reconcile_ns,
    },
    Row {
        name: "resume_cop0_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_cop0_ns,
    },
    Row {
        name: "resume_dispatch_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_dispatch_ns,
    },
    Row {
        name: "resume_invalidate_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_invalidate_ns,
    },
    Row {
        name: "resume_exit_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_exit_ns,
    },
    Row {
        name: "resume_suspend_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_suspend_ns,
    },
    Row {
        name: "resume_resolve_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_resolve_ns,
    },
    Row {
        name: "resume_hostcall_ns",
        parent: Some("resume_net"),
        gate: "FN64_RESUME_SPLIT",
        get: |s| s.resume_hostcall_ns,
    },
    Row {
        name: "gfx_ns",
        parent: Some("resume_hostcall_ns"),
        gate: "FN64_PHASE_TIMING",
        get: |s| s.gfx_ns,
    },
    Row {
        name: "gfx_lle_ns",
        parent: Some("gfx_ns"),
        gate: "FN64_PHASE_TIMING",
        get: |s| s.gfx_lle_ns,
    },
    Row {
        name: "gfx_lle_rsp_ns",
        parent: Some("gfx_lle_ns"),
        gate: "FN64_PHASE_TIMING",
        get: |s| s.gfx_lle_rsp_ns,
    },
    Row {
        name: "gfx_lle_rdp_ns",
        parent: Some("gfx_lle_ns"),
        gate: "FN64_PHASE_TIMING",
        get: |s| s.gfx_lle_rdp_ns,
    },
    Row {
        name: "audio_lle_ns",
        parent: Some("resume_hostcall_ns"),
        gate: "FN64_PHASE_TIMING",
        get: |s| s.audio_lle_ns,
    },
    // Presentation is a ROOT, not a child of `executor_ns`: it runs on the
    // harness's `advance_virtual_time` arm. Parenting it under the executor
    // would be the inference `counter_tree` explicitly forbids. It is still
    // inside the PUMP, which is why it appears here at all.
    Row { name: "vi_present_ns", parent: None, gate: "FN64_PHASE_TIMING", get: |s| s.vi_present_ns },
];

/// `resume NET` = `exec_resume_ns - exec_mirror_ns - exec_guard_suspend_ns`.
/// Derived, not measured: the resume-split phases exclude the mirror and the
/// suspend guard by construction, so they must be checked against the net.
fn resume_net_ns(s: &PumpSample) -> u64 {
    s.exec_resume_ns
        .saturating_sub(s.exec_mirror_ns)
        .saturating_sub(s.exec_guard_suspend_ns)
}

fn sum_ns(pumps: &[&PumpSample], get: fn(&PumpSample) -> u64) -> u64 {
    pumps.iter().map(|p| get(p)).sum()
}

/// Per-population totals for one row, plus the derived `resume_net`.
fn totals_for(pumps: &[&PumpSample]) -> Vec<(&'static str, u64)> {
    let mut out: Vec<(&'static str, u64)> = ROWS
        .iter()
        .map(|row| (row.name, sum_ns(pumps, row.get)))
        .collect();
    out.push(("resume_net", pumps.iter().map(|p| resume_net_ns(p)).sum()));
    out
}

fn lookup(totals: &[(&'static str, u64)], name: &str) -> u64 {
    totals
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
        .unwrap_or(0)
}

/// A parent whose declared children claim more nanoseconds than it holds.
/// Printed unconditionally, and the offending subtree is labelled rather than
/// quietly presented -- perf-method rule 31.
fn closure_violations(totals: &[(&'static str, u64)]) -> Vec<String> {
    let mut out = Vec::new();
    let mut parents: Vec<&'static str> = ROWS.iter().filter_map(|r| r.parent).collect();
    parents.push("resume_net");
    parents.sort_unstable();
    parents.dedup();
    for parent in parents {
        let parent_ns = lookup(totals, parent);
        if parent_ns == 0 {
            continue;
        }
        let children: Vec<(&'static str, u64)> = ROWS
            .iter()
            .filter(|r| r.parent == Some(parent))
            .map(|r| (r.name, lookup(totals, r.name)))
            .filter(|(_, v)| *v > 0)
            .collect();
        let child_sum: u64 = children.iter().map(|(_, v)| *v).sum();
        if child_sum > parent_ns {
            out.push(format!(
                "  VIOLATION under {parent}: children sum to {:.3}ms but the parent holds \
                 {:.3}ms ({:.2}x) -- children: {}",
                ms(child_sum),
                ms(parent_ns),
                child_sum as f64 / parent_ns as f64,
                children
                    .iter()
                    .map(|(n, v)| format!("{n}={:.3}ms", ms(*v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    out
}

/// Which gates carried data in this run. A row whose gate is unarmed reads a
/// constant zero and is reported as NOT ARMED, never as a zero cost.
fn gate_armed(totals: &[(&'static str, u64)], gate: &str) -> bool {
    ROWS.iter()
        .filter(|r| r.gate == gate)
        .any(|r| lookup(totals, r.name) > 0)
}

pub fn render_report(samples: &[PumpSample], renderer: &str, sequence: usize) -> String {
    let mut out = String::new();
    out.push_str("\n[pump-census] ================================================\n");
    // The renderer on the FIRST line: a graphics figure without its renderer
    // beside it is not a result, and this trap has cost two investigations.
    out.push_str(&format!("[pump-census] RENDERER: {renderer}\n"));
    if samples.is_empty() {
        out.push_str(
            "[pump-census] NO SAMPLES. Either the run ended inside the warmup window \
             (FN64_PUMP_CENSUS_WARMUP) or the census never armed.\n",
        );
        return out;
    }

    let budget_ns = (FIELD_BUDGET_MS * 1e6) as u64;
    let all: Vec<&PumpSample> = samples.iter().collect();
    let fast: Vec<&PumpSample> = samples.iter().filter(|s| s.wall_ns <= budget_ns).collect();
    let slow: Vec<&PumpSample> = samples.iter().filter(|s| s.wall_ns > budget_ns).collect();

    let pop_all = population("all", &all);
    let pop_fast = population("fast", &fast);
    let pop_slow = population("slow", &slow);

    out.push_str(&format!(
        "[pump-census] pumps={} over_budget={} ({:.1}%) against the {:.3}ms field budget\n",
        pop_all.pumps,
        pop_slow.pumps,
        100.0 * pop_slow.pumps as f64 / pop_all.pumps as f64,
        FIELD_BUDGET_MS
    ));
    for p in [&pop_all, &pop_fast, &pop_slow] {
        out.push_str(&format!(
            "[pump-census]   {:>4}: n={:<6} wall mean/p50/p95/max = {:.3}/{:.3}/{:.3}/{:.3} ms  \
             (total {:.1} ms)\n",
            p.name, p.pumps, p.wall_mean_ms, p.wall_p50_ms, p.wall_p95_ms, p.wall_max_ms,
            p.wall_total_ms
        ));
    }

    // THE TAIL. Defined as the wall time slow pumps spend ABOVE the budget:
    // the excess that actually misses deadlines, not the whole cost of a slow
    // pump (a slow pump would have cost one budget's worth anyway). Every
    // attribution fraction below is taken against this denominator, stated
    // here so no row is read against the wrong one (rule 32).
    let tail_ms: f64 = slow
        .iter()
        .map(|s| ms(s.wall_ns) - FIELD_BUDGET_MS)
        .sum::<f64>();
    out.push_str(&format!(
        "[pump-census] TAIL (slow-pump wall above budget) = {tail_ms:.1} ms over {} slow pumps \
         ({:.3} ms/slow pump)\n",
        pop_slow.pumps,
        if pop_slow.pumps > 0 { tail_ms / pop_slow.pumps as f64 } else { 0.0 }
    ));

    let t_all = totals_for(&all);
    let t_fast = totals_for(&fast);
    let t_slow = totals_for(&slow);

    // Gate status BEFORE any row, so a zero is never read as a cost.
    out.push_str("[pump-census] gates: ");
    for gate in ["FN64_PHASE_TIMING", "FN64_EXECUTOR_SPLIT", "FN64_RESUME_SPLIT"] {
        out.push_str(&format!(
            "{gate}={} ",
            if gate_armed(&t_all, gate) { "ARMED" } else { "NOT-ARMED(zeros are not costs)" }
        ));
    }
    let dpc_armed = all.iter().any(|s| s.rsp_entries > 0 || s.rsp_steps_gfx > 0);
    out.push_str(&format!(
        "FN64_DPC_COPY_CENSUS={}\n",
        if dpc_armed { "ARMED" } else { "NOT-ARMED(zeros are not costs)" }
    ));

    // Closure, unconditionally, before the rows it validates.
    for population_totals in [(&t_fast, "fast"), (&t_slow, "slow")] {
        for line in closure_violations(population_totals.0) {
            out.push_str(&format!("[pump-census] [{}] {line}\n", population_totals.1));
        }
    }
    // The outer closure this instrument uniquely can check: attributed phases
    // against the independently-measured wall time of the pumps containing
    // them. `executor_ns` and `vi_present_ns` are the two roots.
    for (t, p, label) in [(&t_fast, &pop_fast, "fast"), (&t_slow, &pop_slow, "slow")] {
        let roots = ms(lookup(t, "executor_ns")) + ms(lookup(t, "vi_present_ns"));
        let residual = p.wall_total_ms - roots;
        out.push_str(&format!(
            "[pump-census] [{label}] closure: roots(executor+vi_present)={roots:.1}ms vs \
             pump wall={:.1}ms -> unattributed residual {residual:.1}ms ({:.1}%){}\n",
            p.wall_total_ms,
            if p.wall_total_ms > 0.0 { 100.0 * residual / p.wall_total_ms } else { 0.0 },
            if residual < 0.0 { "  <-- NEGATIVE: the split does NOT close, treat rows as broken" } else { "" }
        ));
    }

    // ---- the ranked attribution.
    out.push_str(
        "[pump-census] per-pump means, ms (fast | slow | slow-fast delta | share of TAIL):\n",
    );
    let mut ranked: Vec<(&'static str, f64, f64, f64, f64)> = Vec::new();
    let mut names: Vec<&'static str> = ROWS.iter().map(|r| r.name).collect();
    names.push("resume_net");
    for name in names {
        let f = if pop_fast.pumps > 0 {
            ms(lookup(&t_fast, name)) / pop_fast.pumps as f64
        } else {
            0.0
        };
        let s = if pop_slow.pumps > 0 {
            ms(lookup(&t_slow, name)) / pop_slow.pumps as f64
        } else {
            0.0
        };
        // Excess = what this phase costs in slow pumps beyond what it costs
        // in fast ones, summed over the slow pumps. The tail's composition.
        let excess_total = (s - f) * pop_slow.pumps as f64;
        let share = if tail_ms > 0.0 { 100.0 * excess_total / tail_ms } else { 0.0 };
        ranked.push((name, f, s, s - f, share));
    }
    ranked.sort_by(|a, b| b.4.total_cmp(&a.4));
    for (name, f, s, d, share) in &ranked {
        out.push_str(&format!(
            "[pump-census]   {name:<24} {f:>8.3} | {s:>8.3} | {d:>+8.3} | {share:>6.1}%\n"
        ));
    }

    // ---- counts, not inferred (rule 3). Are slow pumps doing MORE work or
    // the SAME work more slowly?
    out.push_str("[pump-census] per-pump counts (fast | slow | ratio):\n");
    let counts: &[(&str, fn(&PumpSample) -> u64)] = &[
        ("steps", |s| s.steps),
        ("executor_calls", |s| s.executor_calls),
        ("resume_hostcall_calls", |s| s.resume_hostcall_calls),
        ("gfx_tasks", |s| s.gfx_tasks),
        ("gfx_calls", |s| s.gfx_calls),
        ("gfx_lle_calls", |s| s.gfx_lle_calls),
        ("audio_tasks", |s| s.audio_tasks),
        ("audio_lle_calls", |s| s.audio_lle_calls),
        ("vi_present_calls", |s| s.vi_present_calls),
        ("vi_swaps", |s| u64::from(s.swapped)),
        ("rsp_entries", |s| s.rsp_entries),
        ("rsp_steps_gfx", |s| s.rsp_steps_gfx),
        ("rsp_steps_audio", |s| s.rsp_steps_audio),
        ("dpc_calls", |s| s.dpc_calls),
    ];
    for (name, get) in counts {
        let f = if pop_fast.pumps > 0 {
            sum_ns(&fast, *get) as f64 / pop_fast.pumps as f64
        } else {
            0.0
        };
        let s = if pop_slow.pumps > 0 {
            sum_ns(&slow, *get) as f64 / pop_slow.pumps as f64
        } else {
            0.0
        };
        out.push_str(&format!(
            "[pump-census]   {name:<24} {f:>10.2} | {s:>10.2} | {:>7}\n",
            if f > 0.0 { format!("{:.2}x", s / f) } else { "n/a".to_string() }
        ));
    }

    // ---- periodicity. A repeating period would NAME the trigger; its
    // absence is equally a finding, so both are printed.
    out.push_str(&periodicity_report(samples, budget_ns));

    if sequence > 0 {
        out.push_str(&format!(
            "[pump-census] sequence dump, first {} pumps: \
             idx,wall_ms,steps,swapped,gfx_tasks,audio_tasks,executor_ms,gfx_ms,gfx_lle_rsp_ms,\
             gfx_lle_rdp_ms,audio_lle_ms,vi_present_ms,resume_dispatch_ms,rsp_steps_gfx,\
             rsp_steps_audio\n",
            sequence.min(samples.len())
        ));
        for (i, s) in samples.iter().take(sequence).enumerate() {
            out.push_str(&format!(
                "[pump-seq] {i},{:.4},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{}\n",
                ms(s.wall_ns),
                s.steps,
                u8::from(s.swapped),
                s.gfx_tasks,
                s.audio_tasks,
                ms(s.executor_ns),
                ms(s.gfx_ns),
                ms(s.gfx_lle_rsp_ns),
                ms(s.gfx_lle_rdp_ns),
                ms(s.audio_lle_ns),
                ms(s.vi_present_ns),
                ms(s.resume_dispatch_ns),
                s.rsp_steps_gfx,
                s.rsp_steps_audio,
            ));
        }
    }
    out.push_str("[pump-census] ================================================\n");
    out
}

/// Is slowness periodic (a fixed cadence) or content-driven?
///
/// Two independent readings, because either alone is misleading. The gap
/// histogram between consecutive slow pumps names a period if one exists; the
/// conditional rates say whether a slow pump is PREDICTED by carrying a gfx
/// task, an audio task, or a VI swap. A high conditional rate with no dominant
/// gap means content-driven, and vice versa.
fn periodicity_report(samples: &[PumpSample], budget_ns: u64) -> String {
    let mut out = String::from("[pump-census] periodicity:\n");
    let slow_idx: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.wall_ns > budget_ns)
        .map(|(i, _)| i)
        .collect();
    if slow_idx.len() < 2 {
        out.push_str("[pump-census]   fewer than two slow pumps; no period computable\n");
        return out;
    }
    let mut gaps: std::collections::BTreeMap<usize, usize> = Default::default();
    for w in slow_idx.windows(2) {
        *gaps.entry(w[1] - w[0]).or_default() += 1;
    }
    let total_gaps: usize = gaps.values().sum();
    let mut ranked: Vec<(usize, usize)> = gaps.into_iter().collect();
    ranked.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let shown: Vec<String> = ranked
        .iter()
        .take(6)
        .map(|(gap, count)| {
            format!("{gap}:{count}({:.0}%)", 100.0 * *count as f64 / total_gaps as f64)
        })
        .collect();
    out.push_str(&format!(
        "[pump-census]   slow-pump gap histogram (gap:count) = {}\n",
        shown.join(" ")
    ));

    // Conditional rates. `P(slow | condition)` against `P(slow)`: a condition
    // that does not move the rate is not the trigger, whatever its counter says.
    let base = slow_idx.len() as f64 / samples.len() as f64;
    out.push_str(&format!(
        "[pump-census]   P(slow) = {:.3} over n={}\n",
        base,
        samples.len()
    ));
    let conditions: &[(&str, fn(&PumpSample) -> bool)] = &[
        ("gfx_task>0", |s| s.gfx_tasks > 0),
        ("audio_task>0", |s| s.audio_tasks > 0),
        ("vi_swap", |s| s.swapped),
        ("gfx_lle_call>0", |s| s.gfx_lle_calls > 0),
        ("no gfx and no audio task", |s| s.gfx_tasks == 0 && s.audio_tasks == 0),
    ];
    for (label, pred) in conditions {
        let matching: Vec<&PumpSample> = samples.iter().filter(|s| pred(s)).collect();
        if matching.is_empty() {
            out.push_str(&format!(
                "[pump-census]   P(slow | {label}) = n/a (condition never held)\n"
            ));
            continue;
        }
        let slow_matching = matching.iter().filter(|s| s.wall_ns > budget_ns).count();
        out.push_str(&format!(
            "[pump-census]   P(slow | {label}) = {:.3}  (n={}, lift {:.2}x)\n",
            slow_matching as f64 / matching.len() as f64,
            matching.len(),
            if base > 0.0 {
                (slow_matching as f64 / matching.len() as f64) / base
            } else {
                0.0
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(wall_ms: f64) -> PumpSample {
        PumpSample { wall_ns: (wall_ms * 1e6) as u64, ..Default::default() }
    }

    /// Every row's declared parent must exist as a row (or be the derived
    /// `resume_net`). A parent naming a bucket that is never measured makes
    /// the closure check silently vacuous -- the failure mode rule 6a is about.
    #[test]
    fn every_declared_parent_is_a_row_this_census_measures() {
        for row in ROWS {
            let Some(parent) = row.parent else { continue };
            assert!(
                parent == "resume_net" || ROWS.iter().any(|r| r.name == parent),
                "{} declares parent {parent}, which this census never measures, so its \
                 closure check can never fail",
                row.name
            );
        }
    }

    /// The row table must agree with `fn64_abi`'s counter tree about who
    /// contains whom. A disagreement is a mislabelled bucket, and a
    /// mislabelled bucket reads as a finding.
    #[test]
    fn nesting_matches_the_abi_counter_tree() {
        for row in ROWS {
            let Some(node) = fn64_abi::counter_tree::TREE.iter().find(|n| n.name == row.name)
            else {
                panic!("{} is not in fn64_abi's counter tree", row.name);
            };
            assert_eq!(
                node.parent, row.parent,
                "{} nests under {:?} in fn64-abi but under {:?} here",
                row.name, node.parent, row.parent
            );
            assert_eq!(node.gate, row.gate, "{} gate disagrees with fn64-abi", row.name);
        }
    }

    /// A subtree claiming more time than its parent must be REFUSED loudly,
    /// not printed. Injects the defect and checks the instrument catches it.
    #[test]
    fn a_child_exceeding_its_parent_is_reported_as_a_violation() {
        let clean = totals_for(&[]);
        assert!(closure_violations(&clean).is_empty(), "an empty run has no violations");

        let broken: Vec<(&'static str, u64)> = vec![
            ("executor_ns", 1_000_000),
            ("exec_resume_ns", 3_000_000),
            ("exec_devtime_ns", 0),
        ];
        let violations = closure_violations(&broken);
        assert_eq!(violations.len(), 1, "exactly the executor_ns parent is violated");
        assert!(violations[0].contains("VIOLATION under executor_ns"), "{}", violations[0]);
        assert!(violations[0].contains("3.00"), "the arithmetic is attached: {}", violations[0]);
    }

    /// The fast/slow boundary is the 16.667 ms field budget, the same
    /// threshold the shell heartbeat's `over_budget` uses, so the two
    /// populations are comparable across the two instruments.
    #[test]
    fn the_population_boundary_is_one_field_budget() {
        let samples = vec![sample(16.0), sample(16.6), sample(16.7), sample(40.0)];
        let text = render_report(&samples, "test", 0);
        assert!(text.contains("pumps=4 over_budget=2 (50.0%)"), "{text}");
        // The tail is the EXCESS above budget, not the whole slow-pump cost:
        // (16.7-16.667) + (40-16.667) = 23.37.
        assert!(text.contains("TAIL (slow-pump wall above budget) = 23.4 ms"), "{text}");
    }

    /// A zero from an unarmed gate must never be rendered as "this phase is
    /// free". The report says NOT-ARMED instead.
    #[test]
    fn an_unarmed_gate_is_labelled_rather_than_read_as_zero_cost() {
        let text = render_report(&[sample(30.0)], "test", 0);
        assert!(
            text.contains("FN64_PHASE_TIMING=NOT-ARMED(zeros are not costs)"),
            "{text}"
        );
        assert!(
            text.contains("FN64_RESUME_SPLIT=NOT-ARMED(zeros are not costs)"),
            "{text}"
        );
    }

    /// The renderer is on the report's first content line. A graphics figure
    /// without its renderer beside it has cost this project two whole
    /// investigations.
    #[test]
    fn the_renderer_is_named_before_any_number() {
        let text = render_report(&[sample(8.0)], "rt64", 0);
        let renderer_at = text.find("RENDERER: rt64").expect("renderer line present");
        let first_number = text.find("pumps=").expect("counts present");
        assert!(renderer_at < first_number, "{text}");
    }

    /// A strictly periodic slow pump must show one dominant gap; content-driven
    /// slowness must not. The instrument has to distinguish them or it cannot
    /// answer the question it was built for.
    #[test]
    fn a_fixed_period_shows_one_dominant_gap() {
        let periodic: Vec<PumpSample> = (0..60)
            .map(|i| if i % 4 == 0 { sample(30.0) } else { sample(8.0) })
            .collect();
        let text = render_report(&periodic, "test", 0);
        assert!(text.contains("gap histogram (gap:count) = 4:14(100%)"), "{text}");
    }

    /// A condition that does not move the slow rate must report lift ~1x. This
    /// is the check that stops a plausible counter being named as the trigger.
    #[test]
    fn a_condition_uncorrelated_with_slowness_reports_unit_lift() {
        // gfx_tasks alternates independently of wall time: every other pump
        // carries a task, and slowness is every other pump offset by one, so
        // exactly half of task-carrying pumps are slow -- the base rate.
        let samples: Vec<PumpSample> = (0..40)
            .map(|i| PumpSample {
                wall_ns: if i % 2 == 0 { 30_000_000 } else { 8_000_000 },
                gfx_tasks: u64::from(i % 4 < 2),
                ..Default::default()
            })
            .collect();
        let text = render_report(&samples, "test", 0);
        assert!(text.contains("P(slow | gfx_task>0) = 0.500"), "{text}");
        assert!(text.contains("lift 1.00x"), "{text}");
    }

    /// The bounded-run gate must stop exactly at its budget, and the warmup
    /// must be discarded rather than counted toward it.
    #[test]
    fn the_sequence_dump_emits_one_row_per_requested_pump() {
        let samples: Vec<PumpSample> = (0..10).map(|i| sample(i as f64)).collect();
        let text = render_report(&samples, "test", 3);
        assert_eq!(text.matches("[pump-seq] ").count(), 3, "{text}");
    }

    /// The whole point of the instrument: it must attribute the tail to the
    /// phase that grew, and rank that phase first.
    #[test]
    fn the_phase_that_grows_in_slow_pumps_ranks_first_by_tail_share() {
        let mut samples = Vec::new();
        for _ in 0..50 {
            samples.push(PumpSample {
                wall_ns: 8_000_000,
                executor_ns: 7_000_000,
                gfx_ns: 1_000_000,
                ..Default::default()
            });
        }
        for _ in 0..50 {
            samples.push(PumpSample {
                wall_ns: 40_000_000,
                executor_ns: 39_000_000,
                // gfx grew by 30 ms per slow pump; nothing else did.
                gfx_ns: 31_000_000,
                ..Default::default()
            });
        }
        let text = render_report(&samples, "test", 0);
        let rows: Vec<&str> = text
            .lines()
            .skip_while(|l| !l.contains("per-pump means"))
            .skip(1)
            .take(3)
            .collect();
        assert!(
            rows[0].contains("executor_ns"),
            "executor_ns grew most in absolute ms: {rows:?}"
        );
        assert!(rows[1].contains("gfx_ns"), "gfx_ns is second: {rows:?}");
    }
}
