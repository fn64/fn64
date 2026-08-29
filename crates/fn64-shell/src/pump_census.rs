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
//!
//! The same sequence gate also emits `[wall-cadence-seq]` rows. Each row is
//! finalized at the following pump start and joins the exact scheduled
//! deadline, start debt, wake overshoot, reanchor decision, prior pump and
//! redraw costs, intended wait, and the remaining outside-loop residual under
//! the prior pump's index. Separate `[wall-swap-seq]` rows are indexed by the
//! ending swapped pump; keeping that backward-looking interval out of the
//! forward-looking cadence row prevents a false same-row correlation. The
//! final pump has no following start and is deliberately absent rather than
//! assigned a fabricated interval. All clocks are the `Instant`s the shell
//! already reads for its heartbeat pump/present timing; this collector adds no
//! hot clock read.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::framebuffer::{
    PresentCacheMode, PresentDependencyObservation, PresentDependencyReceipt,
};

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
    /// Presentations that ran INSIDE `run_one_step`, and are therefore
    /// counted a SECOND time inside `executor_ns`. Ungated and expected zero:
    /// `counter_tree` declares `vi_present_ns` a root precisely because
    /// presentation runs on the harness's `advance_virtual_time` arm. A
    /// nonzero value refutes that for those pumps, and the two roots may not
    /// be added for them -- which is why this is read rather than assumed.
    pub vi_present_in_executor_calls: u64,

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

    // ---- FN64_TASK_CPU_PHASE_CENSUS, renderer completion ordinals
    pub task_cpu_armed: bool,
    pub task_cpu_completion_before: u64,
    pub task_cpu_completion_after: u64,
    pub task_cpu_envelope_ns: u64,
    pub task_cpu_members: u64,
    pub task_cpu_member_ns: u64,
    pub task_cpu_all_cpu_member_ns: u64,
    pub task_cpu_compute_segment_ns: u64,
    pub task_cpu_renderer_work_ns: u64,
    pub task_cpu_member_accounted_ns: u64,
    pub task_cpu_execution_view_plan_residual_ns: u64,
    pub task_cpu_finalize_coordinator_ns: u64,
    pub task_cpu_post_view_wrapper_residual_ns: u64,
    pub task_cpu_outer_residual_ns: u64,
    pub task_cpu_rdp_front_half_ns: u64,

    // ---- existing ABI session/task-batch clocks joined at pump boundaries
    pub abi_phase_armed: bool,
    pub session_plan_ns: u64,
    pub session_finalize_ns: u64,
    pub session_execute_ns: u64,
    pub session_commit_ns: u64,
    pub task_batch_total_ns: u64,
    pub task_batch_setup_ns: u64,
    pub task_batch_plan_bind_ns: u64,
    pub task_batch_guest_reads_ns: u64,
    pub task_batch_staged_writes_ns: u64,
    pub task_batch_copyback_ns: u64,
    pub task_batch_publication_ns: u64,
    pub task_batch_tasks: u64,

    // ---- FN64_DPC_COPY_CENSUS
    pub rsp_steps_gfx: u64,
    pub rsp_steps_audio: u64,
    pub rsp_entries: u64,
    pub dpc_calls: u64,
}

/// Joined wall-clock attribution for one completed pump-to-pump interval.
///
/// The row is finalized only when the following pump starts. All durations
/// therefore share the exact pump index from [`PumpSample`] without guessing
/// which redraw callback belonged to which interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WallCadenceSample {
    pub pump_index: usize,
    pub pump_start_ns: u64,
    pub scheduled_deadline_ns: u64,
    pub interval_ns: u64,
    pub start_debt_ns: u64,
    pub wake_overshoot_ns: u64,
    pub reanchored: bool,
    pub prior_pump_ns: u64,
    pub prior_present_ns: u64,
    pub intended_wait_ns: u64,
    pub outside_residual_ns: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingWallCadence {
    sample_index: Option<usize>,
    pump_start: Instant,
    pump_end: Instant,
    scheduled_deadline: Instant,
    next_deadline: Instant,
    start_debt_ns: u64,
    reanchored: bool,
    present_ns: u64,
    present_ended: Option<Instant>,
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
    task_cpu: Option<fn64_render_wgpu::TaskCpuPhaseRunningTotals>,
    session: (u64, u64, u64, u64, u64),
    task_batch: Option<fn64_abi::TaskBatchPhaseRunningTotals>,
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
    vi_present_in_executor_calls: u64,
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
        let task_cpu = fn64_render_wgpu::task_cpu_phase_running_totals();
        let session = fn64_abi::session_phase_running_totals();
        let task_batch = fn64_abi::task_batch_phase_running_totals();
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
                vi_present_in_executor_calls: p.vi_present_in_executor_calls,
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
            task_cpu,
            session,
            task_batch,
        }
    }

    fn delta(&self, before: &Self, wall_ns: u64, steps: u64, swapped: bool) -> PumpSample {
        let (a, b) = (&self.phase, &before.phase);
        let task_cpu_armed = self.task_cpu.is_some() || before.task_cpu.is_some();
        let task_after = self.task_cpu.unwrap_or_default();
        let task_before = before.task_cpu.unwrap_or_default();
        let task_delta = fn64_render_wgpu::TaskCpuPhaseRunningTotals {
            completed_tasks: task_after
                .completed_tasks
                .saturating_sub(task_before.completed_tasks),
            task_envelope_ns: task_after
                .task_envelope_ns
                .saturating_sub(task_before.task_envelope_ns),
            attributed_members: task_after
                .attributed_members
                .saturating_sub(task_before.attributed_members),
            cpu_member_ns: task_after
                .cpu_member_ns
                .saturating_sub(task_before.cpu_member_ns),
            all_cpu_member_ns: task_after
                .all_cpu_member_ns
                .saturating_sub(task_before.all_cpu_member_ns),
            compute_segment_ns: task_after
                .compute_segment_ns
                .saturating_sub(task_before.compute_segment_ns),
            source_binding_load_ns: task_after
                .source_binding_load_ns
                .saturating_sub(task_before.source_binding_load_ns),
            prefix_capture_ns: task_after
                .prefix_capture_ns
                .saturating_sub(task_before.prefix_capture_ns),
            schedule_decode_row_prep_raster_ns: task_after
                .schedule_decode_row_prep_raster_ns
                .saturating_sub(task_before.schedule_decode_row_prep_raster_ns),
            candidate_seed_copy_ns: task_after
                .candidate_seed_copy_ns
                .saturating_sub(task_before.candidate_seed_copy_ns),
            execution_view_gross_ns: task_after
                .execution_view_gross_ns
                .saturating_sub(task_before.execution_view_gross_ns),
            finalize_coordinator_ns: task_after
                .finalize_coordinator_ns
                .saturating_sub(task_before.finalize_coordinator_ns),
        };
        let session_after = self.session;
        let session_before = before.session;
        let batch_after = self.task_batch.unwrap_or_default();
        let batch_before = before.task_batch.unwrap_or_default();
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
            vi_present_in_executor_calls: a
                .vi_present_in_executor_calls
                .saturating_sub(b.vi_present_in_executor_calls),
            exec_resume_ns: a.exec_resume_ns.saturating_sub(b.exec_resume_ns),
            exec_mirror_ns: a.exec_mirror_ns.saturating_sub(b.exec_mirror_ns),
            exec_devtime_ns: a.exec_devtime_ns.saturating_sub(b.exec_devtime_ns),
            exec_guard_suspend_ns: a
                .exec_guard_suspend_ns
                .saturating_sub(b.exec_guard_suspend_ns),
            exec_guard_device_ns: a
                .exec_guard_device_ns
                .saturating_sub(b.exec_guard_device_ns),
            resume_reconcile_ns: a.resume_reconcile_ns.saturating_sub(b.resume_reconcile_ns),
            resume_cop0_ns: a.resume_cop0_ns.saturating_sub(b.resume_cop0_ns),
            resume_dispatch_ns: a.resume_dispatch_ns.saturating_sub(b.resume_dispatch_ns),
            resume_invalidate_ns: a
                .resume_invalidate_ns
                .saturating_sub(b.resume_invalidate_ns),
            resume_exit_ns: a.resume_exit_ns.saturating_sub(b.resume_exit_ns),
            resume_suspend_ns: a.resume_suspend_ns.saturating_sub(b.resume_suspend_ns),
            resume_resolve_ns: a.resume_resolve_ns.saturating_sub(b.resume_resolve_ns),
            resume_hostcall_ns: a.resume_hostcall_ns.saturating_sub(b.resume_hostcall_ns),
            resume_hostcall_calls: a
                .resume_hostcall_calls
                .saturating_sub(b.resume_hostcall_calls),
            gfx_tasks: self.gfx_tasks.saturating_sub(before.gfx_tasks),
            audio_tasks: self.audio_tasks.saturating_sub(before.audio_tasks),
            task_cpu_armed,
            task_cpu_completion_before: task_before.completed_tasks,
            task_cpu_completion_after: task_after.completed_tasks,
            task_cpu_envelope_ns: task_delta.task_envelope_ns,
            task_cpu_members: task_delta.attributed_members,
            task_cpu_member_ns: task_delta.cpu_member_ns,
            task_cpu_all_cpu_member_ns: task_delta.all_cpu_member_ns,
            task_cpu_compute_segment_ns: task_delta.compute_segment_ns,
            task_cpu_renderer_work_ns: task_delta.renderer_work_ns(),
            task_cpu_member_accounted_ns: task_delta.member_accounted_ns(),
            task_cpu_execution_view_plan_residual_ns: task_delta
                .execution_view_captured_read_plan_residual_ns(),
            task_cpu_finalize_coordinator_ns: task_delta.finalize_coordinator_ns,
            task_cpu_post_view_wrapper_residual_ns: task_delta.post_view_wrapper_residual_ns(),
            task_cpu_outer_residual_ns: task_delta.outer_task_residual_ns(),
            // The threaded RDP front half ends when the worker is enqueued;
            // the task envelope starts on that worker and can overlap later
            // guest execution. They are independent different-thread clocks,
            // not parent/child clocks, so subtracting either is invalid.
            task_cpu_rdp_front_half_ns: a.gfx_lle_rdp_ns.saturating_sub(b.gfx_lle_rdp_ns),
            abi_phase_armed: (session_after.4 > 0 || session_before.4 > 0)
                && (self.task_batch.is_some() || before.task_batch.is_some()),
            session_plan_ns: session_after.0.saturating_sub(session_before.0),
            session_finalize_ns: session_after.1.saturating_sub(session_before.1),
            session_execute_ns: session_after.2.saturating_sub(session_before.2),
            session_commit_ns: session_after.3.saturating_sub(session_before.3),
            task_batch_total_ns: batch_after.total_ns.saturating_sub(batch_before.total_ns),
            task_batch_setup_ns: batch_after.setup_ns.saturating_sub(batch_before.setup_ns),
            task_batch_plan_bind_ns: batch_after
                .plan_bind_ns
                .saturating_sub(batch_before.plan_bind_ns),
            task_batch_guest_reads_ns: batch_after
                .guest_reads_ns
                .saturating_sub(batch_before.guest_reads_ns),
            task_batch_staged_writes_ns: batch_after
                .staged_writes_ns
                .saturating_sub(batch_before.staged_writes_ns),
            task_batch_copyback_ns: batch_after
                .copyback_ns
                .saturating_sub(batch_before.copyback_ns),
            task_batch_publication_ns: batch_after
                .publication_ns
                .saturating_sub(batch_before.publication_ns),
            task_batch_tasks: batch_after.tasks.saturating_sub(batch_before.tasks),
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
    wall_origin: Option<Instant>,
    pending_wall: Option<PendingWallCadence>,
    wall_samples: Vec<WallCadenceSample>,
    swap_wall_samples: Vec<(usize, u64)>,
    present_dependencies: Vec<(usize, PresentDependencyReceipt)>,
    last_swap_started: Option<Instant>,
    reported: bool,
}

impl PumpCensus {
    pub fn new() -> Self {
        Self {
            armed: enabled(),
            seen: 0,
            before: Totals::default(),
            samples: Vec::new(),
            wall_origin: None,
            pending_wall: None,
            wall_samples: Vec::new(),
            swap_wall_samples: Vec::new(),
            present_dependencies: Vec::new(),
            last_swap_started: None,
            reported: false,
        }
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Read the running totals immediately before a pump. Cheap enough to be
    /// unconditional when armed; a no-op when not.
    pub fn before_pump(&mut self, started: Instant, scheduled_deadline: Instant) {
        if !self.armed {
            return;
        }
        self.finish_wall_interval(started);
        self.wall_origin
            .get_or_insert(started.min(scheduled_deadline));
        if self.samples.capacity() == 0 {
            // One allocation for the whole run, taken on the first armed pump
            // rather than at construction so an unarmed shell allocates
            // nothing. Sized to the bounded run when one was requested.
            let cap = if pump_limit() > 0 { pump_limit() } else { 8192 };
            self.samples.reserve(cap);
            self.wall_samples.reserve(cap);
            self.swap_wall_samples.reserve(cap / 2);
            self.present_dependencies.reserve(cap);
        }
        self.before = Totals::read();
    }

    /// Attribute the counter deltas since `before_pump` to this pump.
    /// Returns true when the requested pump budget is exhausted.
    #[allow(clippy::too_many_arguments)]
    pub fn after_pump(
        &mut self,
        wall: Duration,
        steps: u64,
        swapped: bool,
        started: Instant,
        scheduled_deadline: Instant,
        next_deadline: Instant,
        reanchored: bool,
        present_dependency: Option<PresentDependencyReceipt>,
    ) -> bool {
        if !self.armed {
            return false;
        }
        self.seen += 1;
        let after = Totals::read();
        let sample_index = if self.seen <= warmup() {
            None
        } else {
            self.samples
                .push(after.delta(&self.before, wall.as_nanos() as u64, steps, swapped));
            Some(self.samples.len() - 1)
        };
        let swap_to_swap_ns = (sample_index.is_some() && swapped)
            .then(|| {
                self.last_swap_started
                    .and_then(|previous| started.checked_duration_since(previous))
                    .map(|duration| duration.as_nanos() as u64)
            })
            .flatten();
        if let (Some(index), Some(duration)) = (sample_index, swap_to_swap_ns) {
            self.swap_wall_samples.push((index, duration));
        }
        if let (Some(index), Some(receipt)) = (sample_index, present_dependency) {
            self.present_dependencies.push((index, receipt));
        }
        if sample_index.is_some() && swapped {
            self.last_swap_started = Some(started);
        } else if sample_index.is_none() {
            self.last_swap_started = None;
        }
        debug_assert!(self.pending_wall.is_none());
        self.pending_wall = Some(PendingWallCadence {
            sample_index,
            pump_start: started,
            pump_end: started + wall,
            scheduled_deadline,
            next_deadline,
            start_debt_ns: started
                .checked_duration_since(scheduled_deadline)
                .unwrap_or_default()
                .as_nanos() as u64,
            reanchored,
            present_ns: 0,
            present_ended: None,
        });
        let limit = pump_limit();
        limit > 0 && self.samples.len() >= limit
    }

    /// Join a redraw callback to the pump that requested it. The caller uses
    /// the same start/end clock reads already required by the heartbeat's
    /// presentation timing, so arming this census adds no clock reads here.
    pub fn record_present(&mut self, started: Instant, wall: Duration) {
        if !self.armed {
            return;
        }
        let Some(pending) = self.pending_wall.as_mut() else {
            return;
        };
        if started < pending.pump_start {
            return;
        }
        pending.present_ns = pending.present_ns.saturating_add(wall.as_nanos() as u64);
        pending.present_ended = Some(started + wall);
    }

    fn finish_wall_interval(&mut self, next_started: Instant) {
        let Some(pending) = self.pending_wall.take() else {
            return;
        };
        let Some(pump_index) = pending.sample_index else {
            return;
        };
        let origin = self
            .wall_origin
            .expect("armed wall cadence must retain its first pump origin");
        let ready = pending
            .present_ended
            .map_or(pending.pump_end, |ended| ended.max(pending.pump_end));
        let intended_wait = pending
            .next_deadline
            .checked_duration_since(ready)
            .unwrap_or_default();
        let wake_boundary = pending.next_deadline.max(ready);
        let interval = next_started
            .checked_duration_since(pending.pump_start)
            .unwrap_or_default();
        let outside_residual_ns = (interval.as_nanos() as u64)
            .saturating_sub(
                pending
                    .pump_end
                    .duration_since(pending.pump_start)
                    .as_nanos() as u64,
            )
            .saturating_sub(pending.present_ns)
            .saturating_sub(intended_wait.as_nanos() as u64);
        self.wall_samples.push(WallCadenceSample {
            pump_index,
            pump_start_ns: pending.pump_start.duration_since(origin).as_nanos() as u64,
            scheduled_deadline_ns: pending
                .scheduled_deadline
                .checked_duration_since(origin)
                .unwrap_or_default()
                .as_nanos() as u64,
            interval_ns: interval.as_nanos() as u64,
            start_debt_ns: pending.start_debt_ns,
            wake_overshoot_ns: next_started
                .checked_duration_since(wake_boundary)
                .unwrap_or_default()
                .as_nanos() as u64,
            reanchored: pending.reanchored,
            prior_pump_ns: pending
                .pump_end
                .duration_since(pending.pump_start)
                .as_nanos() as u64,
            prior_present_ns: pending.present_ns,
            intended_wait_ns: intended_wait.as_nanos() as u64,
            outside_residual_ns,
        });
    }

    pub fn samples(&self) -> &[PumpSample] {
        &self.samples
    }

    pub fn wall_samples(&self) -> &[WallCadenceSample] {
        &self.wall_samples
    }

    /// Print once. Idempotent: the bounded-run exit path and any later
    /// teardown both call it, and a report printed twice reads as two runs.
    pub fn report_once(&mut self, renderer: &str) {
        if !self.armed || self.reported {
            return;
        }
        self.reported = true;
        print!("{}", render_report(&self.samples, renderer, sequence_len()));
        print!(
            "{}",
            render_wall_report(&self.wall_samples, &self.swap_wall_samples, sequence_len())
        );
        print!(
            "{}",
            render_present_dependency_report(&self.present_dependencies, sequence_len())
        );
        print!("{}", render_session_phase_report());
        print!("{}", render_executor_yield_census_report());
    }
}

/// The executor owns the only complete view of which typed `Resume` entered
/// each guest thread and which typed `Yield` came back. Keep this beside the
/// pump report so a bounded live run prints the census before its exit path.
fn render_executor_yield_census_report() -> String {
    render_executor_yield_census_snapshot(fn64_abi::executor_yield_census_snapshot())
}

fn render_executor_yield_census_snapshot(
    snapshot: fn64_runtime::ExecutorYieldCensusSnapshot,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let fn64_runtime::ExecutorYieldCensusSnapshot::Armed(report) = snapshot else {
        let _ = writeln!(
            out,
            "[executor-yield-census] NOT ARMED ({})",
            fn64_runtime::EXECUTOR_YIELD_CENSUS_ENV
        );
        return out;
    };

    let _ = writeln!(
        out,
        "[executor-yield-census] threads={} total_resumes={} outer_resume_ms={:.3} \
         max_resume_ms={:.3} per_thread_complete={} checkpoint_charges_complete={}",
        report.threads.len(),
        report.total_resumes,
        report.total_resume_wall_ns as f64 / 1e6,
        report.max_resume_wall_ns as f64 / 1e6,
        report.complete_per_thread(),
        report.complete_checkpoint_charges(),
    );
    for row in &report.threads {
        let resumes: u64 = row.resumes.iter().sum();
        let mean_ns = if resumes == 0 {
            0.0
        } else {
            row.resume_wall_ns as f64 / resumes as f64
        };
        let _ = write!(
            out,
            "[executor-yield-thread] id={} resumes={} outer_ms={:.3} mean_us={:.3} \
             max_ms={:.3} returns={}",
            row.thread,
            resumes,
            row.resume_wall_ns as f64 / 1e6,
            mean_ns / 1e3,
            row.max_resume_wall_ns as f64 / 1e6,
            row.returns,
        );
        for (name, count) in fn64_runtime::RESUME_KIND_NAMES.iter().zip(row.resumes) {
            if count != 0 {
                let _ = write!(out, " resume_{name}={count}");
            }
        }
        for (name, count) in fn64_runtime::YIELD_KIND_NAMES.iter().zip(row.yields) {
            if count != 0 {
                let _ = write!(out, " yield_{name}={count}");
            }
        }
        for charge in &row.checkpoint_charges {
            let _ = write!(
                out,
                " checkpoint_charge_{}={}",
                charge.instructions, charge.count
            );
        }
        if row.checkpoint_charge_overflow != 0 {
            let _ = write!(
                out,
                " checkpoint_charge_overflow={}",
                row.checkpoint_charge_overflow
            );
        }
        if row.checkpoint_owner_next_resume_immediate != 0
            || row.checkpoint_owner_next_resume_interposed != 0
            || row.checkpoint_owner_next_resume_pending != 0
        {
            let _ = write!(
                out,
                " checkpoint_owner_immediate={} checkpoint_owner_interposed={} \
                 checkpoint_owner_pending={} checkpoint_max_interposed={}",
                row.checkpoint_owner_next_resume_immediate,
                row.checkpoint_owner_next_resume_interposed,
                row.checkpoint_owner_next_resume_pending,
                row.checkpoint_max_interposed_resumes,
            );
            for (name, count) in fn64_runtime::YIELD_KIND_NAMES
                .iter()
                .zip(row.checkpoint_owner_next_yields)
            {
                if count != 0 {
                    let _ = write!(out, " checkpoint_next_yield_{name}={count}");
                }
            }
            if row.checkpoint_owner_next_returns != 0 {
                let _ = write!(
                    out,
                    " checkpoint_next_returns={}",
                    row.checkpoint_owner_next_returns
                );
            }
        }
        out.push('\n');
    }
    if report.overflow.row_limit_exceeded {
        let overflow_resumes: u64 = report.overflow.resumes.iter().sum();
        let _ = write!(
            out,
            "[executor-yield-overflow] INCOMPLETE PER-THREAD EVIDENCE: row_limit={} \
             overflow_resumes={} outer_ms={:.3} max_ms={:.3} returns={}",
            fn64_runtime::EXECUTOR_YIELD_CENSUS_THREAD_LIMIT,
            overflow_resumes,
            report.overflow.resume_wall_ns as f64 / 1e6,
            report.overflow.max_resume_wall_ns as f64 / 1e6,
            report.overflow.returns,
        );
        for (name, count) in fn64_runtime::RESUME_KIND_NAMES
            .iter()
            .zip(report.overflow.resumes)
        {
            if count != 0 {
                let _ = write!(out, " resume_{name}={count}");
            }
        }
        for (name, count) in fn64_runtime::YIELD_KIND_NAMES
            .iter()
            .zip(report.overflow.yields)
        {
            if count != 0 {
                let _ = write!(out, " yield_{name}={count}");
            }
        }
        out.push('\n');
    }
    out
}

/// The raw-DPC SESSION phase split, printed beside the pump census.
///
/// WHY IT PRINTS FROM HERE rather than from its own `atexit` hook in
/// `fn64-abi`: the hook demonstrably did not fire on this route -- a run with
/// `FN64_SESSION_PHASE_CENSUS=1` produced a full pump census attributing
/// 66.021 ms to `gfx_lle_rdp_ns` and not one line of phase split. Rather than
/// argue about why (`std::process::exit` and per-image handler ordering both
/// have plausible stories, and perf-method's "cause unknown beats a mechanism
/// that fits" applies), this reads the running totals directly at the one
/// boundary already proven to print. Same reason this module reads
/// `dpc_census_running_totals` rather than waiting for that census's at-exit
/// summary.
///
/// NOT ARMED is reported as NOT ARMED. An unarmed counter reads a constant
/// zero, and presenting that zero as "these phases cost nothing" is the
/// check-that-cannot-fail error (perf-method rule 6a) every gate in this file
/// exists to keep visible.
fn render_session_phase_report() -> String {
    use std::fmt::Write as _;
    let (plan, finalize, execute, commit, submissions) = fn64_abi::session_phase_running_totals();
    let mut out = String::new();
    if submissions == 0 {
        let _ = writeln!(
            out,
            "[session-phase] NOT ARMED or no session submissions observed \
             (FN64_SESSION_PHASE_CENSUS): the zeros below are not costs."
        );
        return out;
    }
    let total = plan + finalize + execute + commit;
    let ms = |ns: u64| ns as f64 / 1e6;
    let share = |ns: u64| {
        if total == 0 {
            0.0
        } else {
            100.0 * ns as f64 / total as f64
        }
    };
    let _ = writeln!(
        out,
        "[session-phase] physical_members={submissions} attributed_total={:.1} ms \
         (plan/finalize are pre-worker, execute is worker, commit is post-join; \
         this cross-thread sum is not process wall time)",
        ms(total)
    );
    for (name, ns) in [
        ("plan", plan),
        ("finalize", finalize),
        ("execute", execute),
        ("commit", commit),
    ] {
        let _ = writeln!(
            out,
            "[session-phase]   {name:<9} {:>10.1} ms  {:>5.1}%  {:>8.3} ms/physical-member",
            ms(ns),
            share(ns),
            ms(ns) / submissions as f64,
        );
    }
    out
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
    Row {
        name: "executor_ns",
        parent: None,
        gate: "FN64_PHASE_TIMING",
        get: |s| s.executor_ns,
    },
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
    Row {
        name: "vi_present_ns",
        parent: None,
        gate: "FN64_PHASE_TIMING",
        get: |s| s.vi_present_ns,
    },
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TaskCpuFrameSpan {
    pumps: u64,
    completion_before: u64,
    completion_after: u64,
    envelope_ns: u64,
    member_ns: u64,
    all_cpu_member_ns: u64,
    compute_segment_ns: u64,
    renderer_work_ns: u64,
    member_accounted_ns: u64,
    execution_view_plan_residual_ns: u64,
    finalize_coordinator_ns: u64,
    post_view_wrapper_residual_ns: u64,
    outer_residual_ns: u64,
    rdp_front_half_ns: u64,
    session_plan_ns: u64,
    session_finalize_ns: u64,
    session_execute_ns: u64,
    session_commit_ns: u64,
    task_batch_total_ns: u64,
    task_batch_setup_ns: u64,
    task_batch_plan_bind_ns: u64,
    task_batch_guest_reads_ns: u64,
    task_batch_staged_writes_ns: u64,
    task_batch_copyback_ns: u64,
    task_batch_publication_ns: u64,
    task_batch_tasks: u64,
}

impl TaskCpuFrameSpan {
    fn push(&mut self, sample: &PumpSample) {
        if self.pumps == 0 {
            self.completion_before = sample.task_cpu_completion_before;
        }
        self.pumps = self.pumps.saturating_add(1);
        self.completion_after = sample.task_cpu_completion_after;
        self.envelope_ns = self.envelope_ns.saturating_add(sample.task_cpu_envelope_ns);
        self.member_ns = self.member_ns.saturating_add(sample.task_cpu_member_ns);
        self.all_cpu_member_ns = self
            .all_cpu_member_ns
            .saturating_add(sample.task_cpu_all_cpu_member_ns);
        self.compute_segment_ns = self
            .compute_segment_ns
            .saturating_add(sample.task_cpu_compute_segment_ns);
        self.renderer_work_ns = self
            .renderer_work_ns
            .saturating_add(sample.task_cpu_renderer_work_ns);
        self.member_accounted_ns = self
            .member_accounted_ns
            .saturating_add(sample.task_cpu_member_accounted_ns);
        self.execution_view_plan_residual_ns = self
            .execution_view_plan_residual_ns
            .saturating_add(sample.task_cpu_execution_view_plan_residual_ns);
        self.finalize_coordinator_ns = self
            .finalize_coordinator_ns
            .saturating_add(sample.task_cpu_finalize_coordinator_ns);
        self.post_view_wrapper_residual_ns = self
            .post_view_wrapper_residual_ns
            .saturating_add(sample.task_cpu_post_view_wrapper_residual_ns);
        self.outer_residual_ns = self
            .outer_residual_ns
            .saturating_add(sample.task_cpu_outer_residual_ns);
        self.rdp_front_half_ns = self
            .rdp_front_half_ns
            .saturating_add(sample.task_cpu_rdp_front_half_ns);
        self.session_plan_ns = self.session_plan_ns.saturating_add(sample.session_plan_ns);
        self.session_finalize_ns = self
            .session_finalize_ns
            .saturating_add(sample.session_finalize_ns);
        self.session_execute_ns = self
            .session_execute_ns
            .saturating_add(sample.session_execute_ns);
        self.session_commit_ns = self
            .session_commit_ns
            .saturating_add(sample.session_commit_ns);
        self.task_batch_total_ns = self
            .task_batch_total_ns
            .saturating_add(sample.task_batch_total_ns);
        self.task_batch_setup_ns = self
            .task_batch_setup_ns
            .saturating_add(sample.task_batch_setup_ns);
        self.task_batch_plan_bind_ns = self
            .task_batch_plan_bind_ns
            .saturating_add(sample.task_batch_plan_bind_ns);
        self.task_batch_guest_reads_ns = self
            .task_batch_guest_reads_ns
            .saturating_add(sample.task_batch_guest_reads_ns);
        self.task_batch_staged_writes_ns = self
            .task_batch_staged_writes_ns
            .saturating_add(sample.task_batch_staged_writes_ns);
        self.task_batch_copyback_ns = self
            .task_batch_copyback_ns
            .saturating_add(sample.task_batch_copyback_ns);
        self.task_batch_publication_ns = self
            .task_batch_publication_ns
            .saturating_add(sample.task_batch_publication_ns);
        self.task_batch_tasks = self
            .task_batch_tasks
            .saturating_add(sample.task_batch_tasks);
    }

    fn execute_outer_ns(self) -> u64 {
        self.session_execute_ns
            .saturating_sub(self.renderer_work_ns)
    }

    fn post_execute_outer_ns(self) -> u64 {
        self.envelope_ns.saturating_sub(self.session_execute_ns)
    }

    fn pre_execute_accounted_ns(self) -> u64 {
        self.task_batch_setup_ns
            .saturating_add(self.task_batch_plan_bind_ns)
            .saturating_add(self.task_batch_guest_reads_ns)
            .saturating_add(self.session_plan_ns)
            .saturating_add(self.session_finalize_ns)
    }

    fn front_half_unattributed_ns(self) -> u64 {
        self.rdp_front_half_ns
            .saturating_sub(self.pre_execute_accounted_ns())
    }

    fn post_execute_accounted_ns(self) -> u64 {
        self.task_batch_staged_writes_ns
            .saturating_add(self.session_commit_ns)
            .saturating_add(self.task_batch_copyback_ns)
            .saturating_add(self.task_batch_publication_ns)
    }

    fn post_execute_unattributed_ns(self) -> u64 {
        self.post_execute_outer_ns()
            .saturating_sub(self.post_execute_accounted_ns())
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TaskCpuFrameFold {
    complete: Vec<TaskCpuFrameSpan>,
    incomplete_prefix: Option<TaskCpuFrameSpan>,
    incomplete_suffix: Option<TaskCpuFrameSpan>,
}

fn fold_task_cpu_drawn_frames(samples: &[PumpSample]) -> TaskCpuFrameFold {
    let mut folded = TaskCpuFrameFold::default();
    let mut current = TaskCpuFrameSpan::default();
    let mut saw_boundary = false;
    for sample in samples {
        current.push(sample);
        if sample.swapped {
            if saw_boundary {
                folded.complete.push(current);
            } else {
                folded.incomplete_prefix = Some(current);
                saw_boundary = true;
            }
            current = TaskCpuFrameSpan::default();
        }
    }
    if current.pumps > 0 {
        if saw_boundary {
            folded.incomplete_suffix = Some(current);
        } else {
            folded.incomplete_prefix = Some(current);
        }
    }
    folded
}

fn render_task_cpu_frame_report(samples: &[PumpSample], sequence: usize) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if !samples.iter().any(|sample| sample.task_cpu_armed) {
        let _ = writeln!(
            out,
            "[task-cpu-frames] NOT ARMED (FN64_TASK_CPU_PHASE_CENSUS); zeros are not costs."
        );
        return out;
    }
    let folded = fold_task_cpu_drawn_frames(samples);
    let rdp_parent_observed = samples.iter().any(|sample| sample.gfx_lle_rdp_ns > 0);
    let prefix_pumps = folded.incomplete_prefix.map_or(0, |span| span.pumps);
    let suffix_pumps = folded.incomplete_suffix.map_or(0, |span| span.pumps);
    let _ = writeln!(
        out,
        "[task-cpu-frames] complete_drawn_frames={} incomplete_prefix_pumps={} \
         incomplete_suffix_pumps={} rdp_parent={} \
         (incomplete spans excluded from frame means)",
        folded.complete.len(),
        prefix_pumps,
        suffix_pumps,
        if rdp_parent_observed {
            "OBSERVED"
        } else {
            "NOT-OBSERVED(zeros are not costs)"
        },
    );
    for (label, span) in [
        ("prefix", folded.incomplete_prefix),
        ("suffix", folded.incomplete_suffix),
    ] {
        let Some(span) = span else { continue };
        let _ = writeln!(
            out,
            "[task-cpu-incomplete-{label}] pumps={} completion_ordinals=({},{}] \
             envelope_ms={:.3} renderer_work_ms={:.3} outer_residual_ms={:.3} \
             rdp_front_half_ms={:.3}",
            span.pumps,
            span.completion_before,
            span.completion_after,
            ms(span.envelope_ns),
            ms(span.renderer_work_ns),
            ms(span.outer_residual_ns),
            ms(span.rdp_front_half_ns),
        );
    }
    let complete_total =
        folded
            .complete
            .iter()
            .fold(TaskCpuFrameSpan::default(), |mut total, frame| {
                total.pumps = total.pumps.saturating_add(frame.pumps);
                total.envelope_ns = total.envelope_ns.saturating_add(frame.envelope_ns);
                total.member_ns = total.member_ns.saturating_add(frame.member_ns);
                total.all_cpu_member_ns = total
                    .all_cpu_member_ns
                    .saturating_add(frame.all_cpu_member_ns);
                total.compute_segment_ns = total
                    .compute_segment_ns
                    .saturating_add(frame.compute_segment_ns);
                total.renderer_work_ns = total
                    .renderer_work_ns
                    .saturating_add(frame.renderer_work_ns);
                total.member_accounted_ns = total
                    .member_accounted_ns
                    .saturating_add(frame.member_accounted_ns);
                total.execution_view_plan_residual_ns = total
                    .execution_view_plan_residual_ns
                    .saturating_add(frame.execution_view_plan_residual_ns);
                total.finalize_coordinator_ns = total
                    .finalize_coordinator_ns
                    .saturating_add(frame.finalize_coordinator_ns);
                total.post_view_wrapper_residual_ns = total
                    .post_view_wrapper_residual_ns
                    .saturating_add(frame.post_view_wrapper_residual_ns);
                total.outer_residual_ns = total
                    .outer_residual_ns
                    .saturating_add(frame.outer_residual_ns);
                total.rdp_front_half_ns = total
                    .rdp_front_half_ns
                    .saturating_add(frame.rdp_front_half_ns);
                total.session_plan_ns = total.session_plan_ns.saturating_add(frame.session_plan_ns);
                total.session_finalize_ns = total
                    .session_finalize_ns
                    .saturating_add(frame.session_finalize_ns);
                total.session_execute_ns = total
                    .session_execute_ns
                    .saturating_add(frame.session_execute_ns);
                total.session_commit_ns = total
                    .session_commit_ns
                    .saturating_add(frame.session_commit_ns);
                total.task_batch_total_ns = total
                    .task_batch_total_ns
                    .saturating_add(frame.task_batch_total_ns);
                total.task_batch_setup_ns = total
                    .task_batch_setup_ns
                    .saturating_add(frame.task_batch_setup_ns);
                total.task_batch_plan_bind_ns = total
                    .task_batch_plan_bind_ns
                    .saturating_add(frame.task_batch_plan_bind_ns);
                total.task_batch_guest_reads_ns = total
                    .task_batch_guest_reads_ns
                    .saturating_add(frame.task_batch_guest_reads_ns);
                total.task_batch_staged_writes_ns = total
                    .task_batch_staged_writes_ns
                    .saturating_add(frame.task_batch_staged_writes_ns);
                total.task_batch_copyback_ns = total
                    .task_batch_copyback_ns
                    .saturating_add(frame.task_batch_copyback_ns);
                total.task_batch_publication_ns = total
                    .task_batch_publication_ns
                    .saturating_add(frame.task_batch_publication_ns);
                total.task_batch_tasks = total
                    .task_batch_tasks
                    .saturating_add(frame.task_batch_tasks);
                total
            });
    let denominator = u64::try_from(folded.complete.len())
        .unwrap_or(u64::MAX)
        .max(1);
    let _ = writeln!(
        out,
        "[task-cpu-frames] complete means: envelope_ms={:.3} hot_member_ms={:.3} \
         all_cpu_member_ms={:.3} compute_segment_ms={:.3} renderer_work_ms={:.3} \
         member_accounted_ms={:.3} view_plan_residual_ms={:.3} \
         finalize_coordinator_ms={:.3} post_view_wrapper_residual_ms={:.3} \
         outer_residual_ms={:.3} rdp_front_half_ms={:.3}",
        ms(complete_total.envelope_ns / denominator),
        ms(complete_total.member_ns / denominator),
        ms(complete_total.all_cpu_member_ns / denominator),
        ms(complete_total.compute_segment_ns / denominator),
        ms(complete_total.renderer_work_ns / denominator),
        ms(complete_total.member_accounted_ns / denominator),
        ms(complete_total.execution_view_plan_residual_ns / denominator),
        ms(complete_total.finalize_coordinator_ns / denominator),
        ms(complete_total.post_view_wrapper_residual_ns / denominator),
        ms(complete_total.outer_residual_ns / denominator),
        ms(complete_total.rdp_front_half_ns / denominator),
    );
    if samples.iter().any(|sample| sample.abi_phase_armed) {
        let complete_completions = folded.complete.iter().fold(0u64, |total, frame| {
            total.saturating_add(
                frame
                    .completion_after
                    .saturating_sub(frame.completion_before),
            )
        });
        let _ = writeln!(
            out,
            "[task-cpu-frames] ABI phase means: execute_outer_ms={:.3} \
             post_execute_outer_ms={:.3} post_execute_accounted_ms={:.3} \
             post_execute_unattributed_ms={:.3} pre_execute_accounted_ms={:.3} \
             front_half_unattributed_ms={:.3} batch_tasks={} completions={}",
            ms(complete_total.execute_outer_ns() / denominator),
            ms(complete_total.post_execute_outer_ns() / denominator),
            ms(complete_total.post_execute_accounted_ns() / denominator),
            ms(complete_total.post_execute_unattributed_ns() / denominator),
            ms(complete_total.pre_execute_accounted_ns() / denominator),
            ms(complete_total.front_half_unattributed_ns() / denominator),
            complete_total.task_batch_tasks,
            complete_completions,
        );
        if complete_total.task_batch_tasks != complete_completions {
            let _ = writeln!(
                out,
                "[task-cpu-frames] ABI JOIN VIOLATION: task-batch tasks={} but renderer \
                 completions={}; phase attribution is not identity-closed.",
                complete_total.task_batch_tasks, complete_completions,
            );
        }
    } else {
        let _ = writeln!(
            out,
            "[task-cpu-frames] ABI phase join NOT ARMED \
             (FN64_TASK_BATCH_PHASE_CENSUS + FN64_SESSION_PHASE_CENSUS); zeros are not costs."
        );
    }
    for (index, frame) in folded.complete.iter().take(sequence).enumerate() {
        let _ = writeln!(
            out,
            "[task-cpu-frame] {index} pumps={} completion_ordinals=({},{}] envelope_ms={:.3} \
             hot_member_ms={:.3} all_cpu_member_ms={:.3} compute_segment_ms={:.3} \
             renderer_work_ms={:.3} member_accounted_ms={:.3} view_plan_residual_ms={:.3} \
             finalize_coordinator_ms={:.3} post_view_wrapper_residual_ms={:.3} \
            outer_residual_ms={:.3} rdp_front_half_ms={:.3}",
            frame.pumps,
            frame.completion_before,
            frame.completion_after,
            ms(frame.envelope_ns),
            ms(frame.member_ns),
            ms(frame.all_cpu_member_ns),
            ms(frame.compute_segment_ns),
            ms(frame.renderer_work_ns),
            ms(frame.member_accounted_ns),
            ms(frame.execution_view_plan_residual_ns),
            ms(frame.finalize_coordinator_ns),
            ms(frame.post_view_wrapper_residual_ns),
            ms(frame.outer_residual_ns),
            ms(frame.rdp_front_half_ns),
        );
        if samples.iter().any(|sample| sample.abi_phase_armed) {
            let _ = writeln!(
                out,
                "[task-abi-frame] {index} execute_outer_ms={:.3} post_execute_outer_ms={:.3} \
                 post_execute_accounted_ms={:.3} post_execute_unattributed_ms={:.3} \
                 pre_execute_accounted_ms={:.3} front_half_unattributed_ms={:.3}",
                ms(frame.execute_outer_ns()),
                ms(frame.post_execute_outer_ns()),
                ms(frame.post_execute_accounted_ns()),
                ms(frame.post_execute_unattributed_ns()),
                ms(frame.pre_execute_accounted_ns()),
                ms(frame.front_half_unattributed_ns()),
            );
        }
    }
    out
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
            p.name,
            p.pumps,
            p.wall_mean_ms,
            p.wall_p50_ms,
            p.wall_p95_ms,
            p.wall_max_ms,
            p.wall_total_ms
        ));
    }
    out.push_str(&render_task_cpu_frame_report(samples, sequence));

    // THE TAIL, and WHICH tail. Two denominators, both printed, because a
    // share is meaningless without the one it was taken against (rule 32) and
    // the first draft of this report used the wrong one:
    //
    // - `excess_ms`  = slow-pump wall MINUS the fast-population mean, summed.
    //   This is the quantity the per-phase rows decompose: every row below is
    //   "what this phase costs in a slow pump beyond what it costs in a fast
    //   one", so its denominator must be the same difference at the top level.
    //   The rows sum to it by construction, which is what makes the shares
    //   add to ~100% and lets a residual be read as unattributed.
    // - `over_budget_ms` = slow-pump wall above the 16.667 ms field budget.
    //   This is the quantity that actually MISSES DEADLINES. It is the smaller
    //   and more conservative figure whenever fast pumps finish early, and it
    //   is the one to quote when asking "how much must fall to hold 60 Hz".
    //
    // They are not interchangeable. Taking phase excesses against the
    // over-budget figure produced shares summing to 1200% in this
    // instrument's first real output -- an arithmetic tell that the
    // denominator was wrong, not that twelve phases each owned the tail.
    let excess_ms: f64 = slow
        .iter()
        .map(|s| ms(s.wall_ns) - pop_fast.wall_mean_ms)
        .sum::<f64>();
    let over_budget_ms: f64 = slow
        .iter()
        .map(|s| ms(s.wall_ns) - FIELD_BUDGET_MS)
        .sum::<f64>();
    let tail_ms = excess_ms;
    out.push_str(&format!(
        "[pump-census] TAIL, two denominators over {} slow pumps:\n\
         [pump-census]   excess over the fast-population mean ({:.3} ms) = {excess_ms:.1} ms \
         ({:.3} ms/slow pump)  <-- the rows below decompose THIS\n\
         [pump-census]   excess over the {:.3} ms field budget       = {over_budget_ms:.1} ms \
         ({:.3} ms/slow pump)  <-- the part that misses deadlines\n",
        pop_slow.pumps,
        pop_fast.wall_mean_ms,
        if pop_slow.pumps > 0 {
            excess_ms / pop_slow.pumps as f64
        } else {
            0.0
        },
        FIELD_BUDGET_MS,
        if pop_slow.pumps > 0 {
            over_budget_ms / pop_slow.pumps as f64
        } else {
            0.0
        },
    ));

    let t_all = totals_for(&all);
    let t_fast = totals_for(&fast);
    let t_slow = totals_for(&slow);

    // Gate status BEFORE any row, so a zero is never read as a cost.
    out.push_str("[pump-census] gates: ");
    for gate in [
        "FN64_PHASE_TIMING",
        "FN64_EXECUTOR_SPLIT",
        "FN64_RESUME_SPLIT",
    ] {
        out.push_str(&format!(
            "{gate}={} ",
            if gate_armed(&t_all, gate) {
                "ARMED"
            } else {
                "NOT-ARMED(zeros are not costs)"
            }
        ));
    }
    let dpc_armed = all.iter().any(|s| s.rsp_entries > 0 || s.rsp_steps_gfx > 0);
    out.push_str(&format!(
        "FN64_DPC_COPY_CENSUS={}\n",
        if dpc_armed {
            "ARMED"
        } else {
            "NOT-ARMED(zeros are not costs)"
        }
    ));

    // Closure, unconditionally, before the rows it validates.
    for population_totals in [(&t_fast, "fast"), (&t_slow, "slow")] {
        for line in closure_violations(population_totals.0) {
            out.push_str(&format!("[pump-census] [{}] {line}\n", population_totals.1));
        }
    }
    // The outer closure this instrument uniquely can check: attributed phases
    // against the independently-measured wall time of the pumps containing
    // them. `executor_ns` and `vi_present_ns` are the two roots -- but only
    // while presentation stays OUTSIDE the executor, which `counter_tree`
    // declares and `vi_present_in_executor_calls` exists to test rather than
    // assume. On a pump where a presentation ran inside `run_one_step` the
    // two roots OVERLAP, so adding them double-counts that presentation and
    // the closure goes negative. That is a measured fact about the pump, not
    // a broken instrument, so the two cases are reported separately: a
    // negative residual among the CLEAN pumps means the rows are wrong, while
    // overlapping pumps are excluded and counted.
    for (population, p, label) in [(&fast, &pop_fast, "fast"), (&slow, &pop_slow, "slow")] {
        let (clean, overlapped): (Vec<&PumpSample>, Vec<&PumpSample>) = population
            .iter()
            .partition(|s| s.vi_present_in_executor_calls == 0);
        let clean_wall: f64 = clean.iter().map(|s| ms(s.wall_ns)).sum();
        let clean_roots: f64 = clean
            .iter()
            .map(|s| ms(s.executor_ns) + ms(s.vi_present_ns))
            .sum();
        let residual = clean_wall - clean_roots;
        out.push_str(&format!(
            "[pump-census] [{label}] closure over the {} pumps whose presentation stayed \
             OUTSIDE the executor: roots(executor+vi_present)={clean_roots:.1}ms vs pump \
             wall={clean_wall:.1}ms -> unattributed residual {residual:.1}ms ({:.1}%){}\n",
            clean.len(),
            if clean_wall > 0.0 {
                100.0 * residual / clean_wall
            } else {
                0.0
            },
            if residual < -0.005 * clean_wall.max(1.0) {
                "  <-- NEGATIVE beyond tolerance: the split does NOT close, treat rows as broken"
            } else {
                ""
            }
        ));
        if !overlapped.is_empty() {
            let ov_wall: f64 = overlapped.iter().map(|s| ms(s.wall_ns)).sum();
            let ov_present: f64 = overlapped.iter().map(|s| ms(s.vi_present_ns)).sum();
            out.push_str(&format!(
                "[pump-census] [{label}] EXCLUDED from that closure: {} of {} pumps ran a \
                 presentation INSIDE the executor ({:.1}% of the population, {:.1}ms wall, \
                 {:.1}ms of vi_present double-counted inside executor_ns). Their \
                 `vi_present_ns` row is NOT additive with `executor_ns`.\n",
                overlapped.len(),
                p.pumps,
                100.0 * overlapped.len() as f64 / p.pumps.max(1) as f64,
                ov_wall,
                ov_present,
            ));
        }
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
        let share = if tail_ms > 0.0 {
            100.0 * excess_total / tail_ms
        } else {
            0.0
        };
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
        ("vi_present_IN_executor", |s| s.vi_present_in_executor_calls),
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
            if f > 0.0 {
                format!("{:.2}x", s / f)
            } else {
                "n/a".to_string()
            }
        ));
    }

    // ---- periodicity. A repeating period would NAME the trigger; its
    // absence is equally a finding, so both are printed.
    out.push_str(&periodicity_report(samples, budget_ns));

    if sequence > 0 {
        out.push_str("[pump-census] sequence schema: fn64.pump-sequence.v2\n");
        out.push_str(&format!(
            "[pump-census] sequence dump, first {} pumps: \
             idx,wall_ms,steps,swapped,gfx_tasks,audio_tasks,executor_ms,gfx_ms,gfx_lle_rsp_ms,\
             gfx_lle_rdp_ms,audio_lle_ms,vi_present_ms,resume_dispatch_ms,rsp_steps_gfx,\
             rsp_steps_audio,task_completion_before,task_completion_after,task_envelope_ms,\
             task_hot_member_ms,task_all_cpu_member_ms,task_compute_segment_ms,task_renderer_work_ms,\
             task_member_accounted_ms,task_view_plan_residual_ms,\
             task_finalize_coordinator_ms,task_post_view_wrapper_residual_ms,\
             task_outer_residual_ms,task_rdp_front_half_ms,session_plan_ms,\
             session_finalize_ms,session_execute_ms,session_commit_ms,task_batch_total_ms,\
             task_batch_setup_ms,task_batch_plan_bind_ms,task_batch_guest_reads_ms,\
             task_batch_staged_writes_ms,task_batch_copyback_ms,task_batch_publication_ms,\
             task_batch_tasks\n",
            sequence.min(samples.len())
        ));
        for (i, s) in samples.iter().take(sequence).enumerate() {
            out.push_str(&format!(
                "[pump-seq] {i},{:.4},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{}\n",
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
                s.task_cpu_completion_before,
                s.task_cpu_completion_after,
                ms(s.task_cpu_envelope_ns),
                ms(s.task_cpu_member_ns),
                ms(s.task_cpu_all_cpu_member_ns),
                ms(s.task_cpu_compute_segment_ns),
                ms(s.task_cpu_renderer_work_ns),
                ms(s.task_cpu_member_accounted_ns),
                ms(s.task_cpu_execution_view_plan_residual_ns),
                ms(s.task_cpu_finalize_coordinator_ns),
                ms(s.task_cpu_post_view_wrapper_residual_ns),
                ms(s.task_cpu_outer_residual_ns),
                ms(s.task_cpu_rdp_front_half_ns),
                ms(s.session_plan_ns),
                ms(s.session_finalize_ns),
                ms(s.session_execute_ns),
                ms(s.session_commit_ns),
                ms(s.task_batch_total_ns),
                ms(s.task_batch_setup_ns),
                ms(s.task_batch_plan_bind_ns),
                ms(s.task_batch_guest_reads_ns),
                ms(s.task_batch_staged_writes_ns),
                ms(s.task_batch_copyback_ns),
                ms(s.task_batch_publication_ns),
                s.task_batch_tasks,
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
            format!(
                "{gap}:{count}({:.0}%)",
                100.0 * *count as f64 / total_gaps as f64
            )
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
        ("no gfx and no audio task", |s| {
            s.gfx_tasks == 0 && s.audio_tasks == 0
        }),
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

fn render_wall_report(
    samples: &[WallCadenceSample],
    swap_samples: &[(usize, u64)],
    sequence: usize,
) -> String {
    let mut out = String::new();
    if samples.is_empty() {
        out.push_str(
            "[wall-cadence] no completed pump-to-pump intervals (the final pump is intentionally pending)\n",
        );
        return out;
    }
    let reanchors = samples.iter().filter(|sample| sample.reanchored).count();
    let sum = |get: fn(&WallCadenceSample) -> u64| -> f64 { ms(samples.iter().map(get).sum()) };
    out.push_str(&format!(
        "[wall-cadence] intervals={} swap_intervals={} reanchors={} \
         interval_total_ms={:.3} pump_ms={:.3} present_ms={:.3} intended_wait_ms={:.3} \
         outside_residual_ms={:.3}\n",
        samples.len(),
        swap_samples.len(),
        reanchors,
        sum(|sample| sample.interval_ns),
        sum(|sample| sample.prior_pump_ns),
        sum(|sample| sample.prior_present_ns),
        sum(|sample| sample.intended_wait_ns),
        sum(|sample| sample.outside_residual_ns),
    ));
    if sequence > 0 {
        out.push_str(
            "[wall-cadence] sequence: idx,pump_start_ms,scheduled_deadline_ms,interval_ms,\
             start_debt_ms,wake_overshoot_ms,reanchored,prior_pump_ms,\
             prior_present_ms,intended_wait_ms,outside_residual_ms\n",
        );
        for sample in samples.iter().take(sequence) {
            out.push_str(&format!(
                "[wall-cadence-seq] {},{:.4},{:.4},{:.4},{:.4},{:.4},{},{:.4},{:.4},{:.4},{:.4}\n",
                sample.pump_index,
                ms(sample.pump_start_ns),
                ms(sample.scheduled_deadline_ns),
                ms(sample.interval_ns),
                ms(sample.start_debt_ns),
                ms(sample.wake_overshoot_ns),
                u8::from(sample.reanchored),
                ms(sample.prior_pump_ns),
                ms(sample.prior_present_ns),
                ms(sample.intended_wait_ns),
                ms(sample.outside_residual_ns),
            ));
        }
        for (pump_index, duration_ns) in swap_samples.iter().take(sequence) {
            out.push_str(&format!(
                "[wall-swap-seq] {pump_index},{:.4}\n",
                ms(*duration_ns)
            ));
        }
    }
    out
}

fn render_present_dependency_report(
    samples: &[(usize, PresentDependencyReceipt)],
    sequence: usize,
) -> String {
    if samples.is_empty() {
        return String::new();
    }
    let cacheable = samples
        .iter()
        .filter(|(_, receipt)| {
            matches!(
                receipt.dependency,
                PresentDependencyObservation::Cacheable(_)
            )
        })
        .count();
    let hits = samples
        .iter()
        .filter(|(_, receipt)| receipt.exact_hit)
        .count();
    let suppressed = samples
        .iter()
        .filter(|(_, receipt)| receipt.suppress_redraw)
        .count();
    let probe_ns = samples
        .iter()
        .map(|(_, receipt)| receipt.probe_ns)
        .sum::<u64>();
    let mut out = format!(
        "[present-dependency] receipts={} cacheable={} uncacheable={} exact_hits={} suppressed={} probe_ms={:.3}\n",
        samples.len(),
        cacheable,
        samples.len().saturating_sub(cacheable),
        hits,
        suppressed,
        ms(probe_ns),
    );
    for (pump_index, receipt) in samples.iter().take(sequence) {
        let mode = match receipt.mode {
            PresentCacheMode::Disabled => "Disabled",
            PresentCacheMode::Observe => "Observe",
            PresentCacheMode::Suppress => "Suppress",
        };
        let disposition = if receipt.suppress_redraw {
            "Suppress"
        } else {
            "Redraw"
        };
        let common = format!(
            "overscan={} zoom_fill={} generation={} invalidations={} probe_ns={}",
            receipt.policy.overscan(),
            u8::from(receipt.policy.zoom_fill()),
            receipt.generation,
            receipt.invalidations,
            receipt.probe_ns,
        );
        match receipt.dependency {
            PresentDependencyObservation::Cacheable(dependency) => out.push_str(&format!(
                "[present-dependency-seq] pump={pump_index} mode={mode} dependency=Cacheable \
                 {common} start={} src_stride={} dst_width={} dst_height={} blanked={} bytes={} \
                 fnv_digest={:016x} sha256={} exact_hit={} disposition={disposition}\n",
                dependency.start,
                dependency.src_stride,
                dependency.dst_width,
                dependency.dst_height,
                u8::from(dependency.blanked),
                dependency.bytes,
                dependency.fnv_digest,
                hex_sha256(dependency.sha256),
                u8::from(receipt.exact_hit),
            )),
            PresentDependencyObservation::Uncacheable(reason) => out.push_str(&format!(
                "[present-dependency-seq] pump={pump_index} mode={mode} \
                 dependency=Uncacheable {common} reason={} exact_hit=0 disposition={disposition}\n",
                reason.name(),
            )),
        }
    }
    out
}

fn hex_sha256(digest: [u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_yield_census_reports_actual_gate_state_and_overflow() {
        let unarmed = render_executor_yield_census_snapshot(
            fn64_runtime::ExecutorYieldCensusSnapshot::Unarmed,
        );
        assert!(unarmed.contains("NOT ARMED"), "{unarmed}");

        let report = fn64_runtime::ExecutorYieldCensusReport {
            threads: vec![fn64_runtime::ExecutorThreadYieldCensus {
                thread: 7,
                resumes: [1, 2, 3, 4, 5],
                yields: [1, 0, 2, 0, 3, 0, 4, 0, 5],
                returns: 1,
                resume_wall_ns: 20_000,
                max_resume_wall_ns: 8_000,
                checkpoint_charges: vec![fn64_runtime::ExecutorCheckpointChargeCensus {
                    instructions: 250,
                    count: 2,
                }],
                checkpoint_charge_overflow: 0,
                checkpoint_owner_next_resume_immediate: 1,
                checkpoint_owner_next_resume_interposed: 1,
                checkpoint_owner_next_resume_pending: 0,
                checkpoint_max_interposed_resumes: 3,
                checkpoint_owner_next_yields: [0, 0, 0, 2, 0, 0, 0, 0, 0],
                checkpoint_owner_next_returns: 0,
            }],
            overflow: fn64_runtime::ExecutorYieldCensusOverflow {
                row_limit_exceeded: true,
                resumes: [1, 0, 0, 0, 0],
                yields: [1, 0, 0, 0, 0, 0, 0, 0, 0],
                returns: 0,
                resume_wall_ns: 1_000,
                max_resume_wall_ns: 1_000,
            },
            total_resumes: 16,
            total_resume_wall_ns: 21_000,
            max_resume_wall_ns: 8_000,
        };
        let armed = render_executor_yield_census_snapshot(
            fn64_runtime::ExecutorYieldCensusSnapshot::Armed(report),
        );
        assert!(armed.contains("per_thread_complete=false"), "{armed}");
        assert!(armed.contains("id=7 resumes=15"), "{armed}");
        assert!(armed.contains("yield_instruction_checkpoint=2"), "{armed}");
        assert!(armed.contains("checkpoint_charge_250=2"), "{armed}");
        assert!(armed.contains("checkpoint_owner_immediate=1"), "{armed}");
        assert!(armed.contains("checkpoint_owner_interposed=1"), "{armed}");
        assert!(
            armed.contains("checkpoint_next_yield_recv_block=2"),
            "{armed}"
        );
        assert!(armed.contains("INCOMPLETE PER-THREAD EVIDENCE"), "{armed}");
        assert!(!armed.contains("NOT ARMED"), "{armed}");
    }

    fn sample(wall_ms: f64) -> PumpSample {
        PumpSample {
            wall_ns: (wall_ms * 1e6) as u64,
            ..Default::default()
        }
    }

    fn dependency_receipt(mode: PresentCacheMode, hit: bool) -> PresentDependencyReceipt {
        PresentDependencyReceipt {
            mode,
            policy: crate::framebuffer::PresentPolicy::new(3, true),
            dependency: PresentDependencyObservation::Cacheable(
                crate::framebuffer::CacheablePresentDependency {
                    start: 16,
                    src_stride: 320,
                    dst_width: 319,
                    dst_height: 240,
                    blanked: false,
                    bytes: 153_600,
                    fnv_digest: 0x0123_4567_89ab_cdef,
                    sha256: [0xab; 32],
                },
            ),
            exact_hit: hit,
            suppress_redraw: mode == PresentCacheMode::Suppress && hit,
            generation: 7,
            invalidations: 2,
            probe_ns: 125_000,
        }
    }

    #[test]
    fn bounded_final_pump_keeps_its_present_dependency_receipt() {
        let mut census = PumpCensus::new();
        census.armed = true;
        census.seen = warmup();
        let started = Instant::now();
        census.before_pump(started, started);
        let _ = census.after_pump(
            Duration::from_millis(1),
            1,
            true,
            started,
            started,
            started + Duration::from_millis(16),
            false,
            Some(dependency_receipt(PresentCacheMode::Observe, true)),
        );
        assert_eq!(census.samples.len(), 1);
        assert_eq!(
            census.present_dependencies,
            vec![(0, dependency_receipt(PresentCacheMode::Observe, true))]
        );
        let report = render_present_dependency_report(&census.present_dependencies, 1);
        assert!(report.contains("pump=0 mode=Observe dependency=Cacheable"));
    }

    #[test]
    fn wall_cadence_lifecycle_preserves_final_swap_while_last_interval_is_pending() {
        let mut census = PumpCensus::new();
        census.armed = true;
        let mut started = Instant::now();
        for _ in 0..warmup() {
            census.before_pump(started, started);
            let _ = census.after_pump(
                Duration::from_millis(1),
                1,
                false,
                started,
                started,
                started + Duration::from_millis(16),
                false,
                None,
            );
            started += Duration::from_millis(17);
        }

        census.before_pump(started, started);
        let _ = census.after_pump(
            Duration::from_millis(10),
            1,
            true,
            started,
            started,
            started + Duration::from_millis(16),
            false,
            None,
        );
        census.record_present(
            started + Duration::from_millis(10),
            Duration::from_millis(1),
        );
        let second = started + Duration::from_millis(17);
        census.before_pump(second, second);
        let _ = census.after_pump(
            Duration::from_millis(2),
            1,
            false,
            second,
            second,
            second + Duration::from_millis(16),
            false,
            None,
        );
        let third = started + Duration::from_millis(33);
        census.before_pump(third, third);
        let _ = census.after_pump(
            Duration::from_millis(3),
            1,
            true,
            third,
            third,
            third + Duration::from_millis(16),
            false,
            None,
        );

        assert_eq!(census.samples.len(), 3);
        assert_eq!(
            census.wall_samples.len(),
            2,
            "the final interval is pending"
        );
        assert_eq!(census.swap_wall_samples, vec![(2, 33_000_000)]);
        let first = census.wall_samples[0];
        assert_eq!(first.pump_index, 0);
        assert_eq!(first.interval_ns, 17_000_000);
        assert_eq!(first.prior_pump_ns, 10_000_000);
        assert_eq!(first.prior_present_ns, 1_000_000);
        assert_eq!(first.intended_wait_ns, 5_000_000);
        assert_eq!(first.wake_overshoot_ns, 1_000_000);
        assert_eq!(first.outside_residual_ns, 1_000_000);

        let report = render_wall_report(&census.wall_samples, &census.swap_wall_samples, 3);
        assert_eq!(report.matches("[wall-cadence-seq] ").count(), 2, "{report}");
        assert!(report.contains("[wall-swap-seq] 2,33.0000"), "{report}");
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
            let Some(node) = fn64_abi::counter_tree::TREE
                .iter()
                .find(|n| n.name == row.name)
            else {
                panic!("{} is not in fn64_abi's counter tree", row.name);
            };
            assert_eq!(
                node.parent, row.parent,
                "{} nests under {:?} in fn64-abi but under {:?} here",
                row.name, node.parent, row.parent
            );
            assert_eq!(
                node.gate, row.gate,
                "{} gate disagrees with fn64-abi",
                row.name
            );
        }
    }

    /// A subtree claiming more time than its parent must be REFUSED loudly,
    /// not printed. Injects the defect and checks the instrument catches it.
    #[test]
    fn a_child_exceeding_its_parent_is_reported_as_a_violation() {
        let clean = totals_for(&[]);
        assert!(
            closure_violations(&clean).is_empty(),
            "an empty run has no violations"
        );

        let broken: Vec<(&'static str, u64)> = vec![
            ("executor_ns", 1_000_000),
            ("exec_resume_ns", 3_000_000),
            ("exec_devtime_ns", 0),
        ];
        let violations = closure_violations(&broken);
        assert_eq!(
            violations.len(),
            1,
            "exactly the executor_ns parent is violated"
        );
        assert!(
            violations[0].contains("VIOLATION under executor_ns"),
            "{}",
            violations[0]
        );
        assert!(
            violations[0].contains("3.00"),
            "the arithmetic is attached: {}",
            violations[0]
        );
    }

    /// The fast/slow boundary is the 16.667 ms field budget, the same
    /// threshold the shell heartbeat's `over_budget` uses, so the two
    /// populations are comparable across the two instruments.
    #[test]
    fn the_population_boundary_is_one_field_budget() {
        let samples = vec![sample(16.0), sample(16.6), sample(16.7), sample(40.0)];
        let text = render_report(&samples, "test", 0);
        assert!(text.contains("pumps=4 over_budget=2 (50.0%)"), "{text}");
        // Both denominators, and they must differ: the fast mean is 16.3 ms,
        // so excess-over-fast is (16.7-16.3)+(40-16.3) = 24.1, while
        // excess-over-budget is (16.7-16.667)+(40-16.667) = 23.4. Quoting one
        // where the other was meant is the error this pair exists to prevent.
        assert!(
            text.contains("excess over the fast-population mean (16.300 ms) = 24.1 ms"),
            "{text}"
        );
        assert!(
            text.contains("excess over the 16.667 ms field budget       = 23.4 ms"),
            "{text}"
        );
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
            .map(|i| {
                if i % 4 == 0 {
                    sample(30.0)
                } else {
                    sample(8.0)
                }
            })
            .collect();
        let text = render_report(&periodic, "test", 0);
        assert!(
            text.contains("gap histogram (gap:count) = 4:14(100%)"),
            "{text}"
        );
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
        for row in text
            .lines()
            .filter_map(|line| line.strip_prefix("[pump-seq] "))
        {
            assert_eq!(row.split(',').count(), 40, "{row}");
        }
    }

    #[test]
    fn completion_ordinals_delta_independently_of_gfx_admissions() {
        let before = Totals {
            gfx_tasks: 40,
            phase: PhaseSnapshot {
                gfx_lle_rdp_ns: 100,
                ..Default::default()
            },
            task_cpu: Some(fn64_render_wgpu::TaskCpuPhaseRunningTotals {
                completed_tasks: 7,
                task_envelope_ns: 100,
                cpu_member_ns: 60,
                all_cpu_member_ns: 80,
                compute_segment_ns: 10,
                execution_view_gross_ns: 45,
                finalize_coordinator_ns: 10,
                ..Default::default()
            }),
            session: (10, 20, 30, 40, 1),
            task_batch: Some(fn64_abi::TaskBatchPhaseRunningTotals {
                tasks: 7,
                total_ns: 100,
                setup_ns: 5,
                ..Default::default()
            }),
            ..Default::default()
        };
        let after = Totals {
            gfx_tasks: 43,
            phase: PhaseSnapshot {
                gfx_lle_rdp_ns: 250,
                ..Default::default()
            },
            task_cpu: Some(fn64_render_wgpu::TaskCpuPhaseRunningTotals {
                completed_tasks: 8,
                task_envelope_ns: 220,
                attributed_members: 2,
                cpu_member_ns: 130,
                all_cpu_member_ns: 170,
                compute_segment_ns: 30,
                source_binding_load_ns: 10,
                prefix_capture_ns: 10,
                schedule_decode_row_prep_raster_ns: 20,
                candidate_seed_copy_ns: 10,
                execution_view_gross_ns: 100,
                finalize_coordinator_ns: 25,
                ..Default::default()
            }),
            session: (20, 35, 150, 55, 2),
            task_batch: Some(fn64_abi::TaskBatchPhaseRunningTotals {
                tasks: 8,
                total_ns: 250,
                setup_ns: 15,
                plan_bind_ns: 5,
                guest_reads_ns: 7,
                staged_writes_ns: 8,
                copyback_ns: 4,
                publication_ns: 6,
                ..Default::default()
            }),
            ..Default::default()
        };
        let sample = after.delta(&before, 1_000, 1, false);
        assert_eq!(sample.gfx_tasks, 3);
        assert_eq!(
            (
                sample.task_cpu_completion_before,
                sample.task_cpu_completion_after
            ),
            (7, 8)
        );
        assert_eq!(sample.task_cpu_envelope_ns, 120);
        assert_eq!(sample.task_cpu_member_ns, 70);
        assert_eq!(sample.task_cpu_all_cpu_member_ns, 90);
        assert_eq!(sample.task_cpu_compute_segment_ns, 20);
        assert_eq!(sample.task_cpu_renderer_work_ns, 110);
        assert_eq!(sample.task_cpu_member_accounted_ns, 50);
        assert_eq!(sample.task_cpu_execution_view_plan_residual_ns, 5);
        assert_eq!(sample.task_cpu_finalize_coordinator_ns, 15);
        assert_eq!(sample.task_cpu_post_view_wrapper_residual_ns, 0);
        assert_eq!(sample.task_cpu_outer_residual_ns, 10);
        assert_eq!(sample.task_cpu_rdp_front_half_ns, 150);
        assert!(sample.abi_phase_armed);
        assert_eq!(sample.session_execute_ns, 120);
        assert_eq!(sample.task_batch_total_ns, 150);
        assert_eq!(sample.task_batch_setup_ns, 10);
        assert_eq!(sample.task_batch_tasks, 1);
    }

    #[test]
    fn abi_phase_split_closes_outer_and_outside_residuals() {
        let span = TaskCpuFrameSpan {
            envelope_ns: 180,
            renderer_work_ns: 100,
            rdp_front_half_ns: 70,
            session_plan_ns: 10,
            session_finalize_ns: 5,
            session_execute_ns: 130,
            session_commit_ns: 7,
            task_batch_setup_ns: 12,
            task_batch_plan_bind_ns: 3,
            task_batch_guest_reads_ns: 8,
            task_batch_staged_writes_ns: 9,
            task_batch_copyback_ns: 4,
            task_batch_publication_ns: 6,
            ..Default::default()
        };
        assert_eq!(span.execute_outer_ns(), 30);
        assert_eq!(span.post_execute_outer_ns(), 50);
        assert_eq!(span.execute_outer_ns() + span.post_execute_outer_ns(), 80);
        assert_eq!(span.post_execute_accounted_ns(), 26);
        assert_eq!(span.post_execute_unattributed_ns(), 24);
        assert_eq!(span.pre_execute_accounted_ns(), 38);
        assert_eq!(span.front_half_unattributed_ns(), 32);

        let saturated = TaskCpuFrameSpan {
            envelope_ns: 1,
            renderer_work_ns: 2,
            session_execute_ns: 3,
            ..Default::default()
        };
        assert_eq!(saturated.execute_outer_ns(), 1);
        assert_eq!(saturated.post_execute_outer_ns(), 0);
    }

    #[test]
    fn drawn_frame_fold_excludes_and_accounts_for_incomplete_spans() {
        let samples = vec![
            PumpSample {
                task_cpu_armed: true,
                task_cpu_completion_before: 4,
                task_cpu_completion_after: 5,
                task_cpu_member_ns: 10,
                ..Default::default()
            },
            PumpSample {
                swapped: true,
                task_cpu_armed: true,
                task_cpu_completion_before: 5,
                task_cpu_completion_after: 6,
                task_cpu_member_ns: 20,
                ..Default::default()
            },
            PumpSample {
                task_cpu_armed: true,
                task_cpu_completion_before: 6,
                task_cpu_completion_after: 7,
                task_cpu_member_ns: 30,
                ..Default::default()
            },
            PumpSample {
                swapped: true,
                task_cpu_armed: true,
                task_cpu_completion_before: 7,
                task_cpu_completion_after: 8,
                task_cpu_member_ns: 40,
                ..Default::default()
            },
            PumpSample {
                task_cpu_armed: true,
                task_cpu_completion_before: 8,
                task_cpu_completion_after: 9,
                task_cpu_member_ns: 50,
                ..Default::default()
            },
        ];
        let folded = fold_task_cpu_drawn_frames(&samples);
        assert_eq!(folded.incomplete_prefix.unwrap().pumps, 2);
        assert_eq!(folded.incomplete_prefix.unwrap().member_ns, 30);
        assert_eq!(folded.complete.len(), 1);
        assert_eq!(folded.complete[0].pumps, 2);
        assert_eq!(folded.complete[0].member_ns, 70);
        assert_eq!(
            (
                folded.complete[0].completion_before,
                folded.complete[0].completion_after
            ),
            (6, 8)
        );
        assert_eq!(folded.incomplete_suffix.unwrap().pumps, 1);
        assert_eq!(folded.incomplete_suffix.unwrap().member_ns, 50);
        let text = render_task_cpu_frame_report(&samples, 0);
        assert!(
            text.contains(
                "complete_drawn_frames=1 incomplete_prefix_pumps=2 incomplete_suffix_pumps=1"
            ),
            "{text}"
        );
        assert!(
            text.contains("[task-cpu-incomplete-prefix] pumps=2"),
            "{text}"
        );
        assert!(
            text.contains("[task-cpu-incomplete-suffix] pumps=1"),
            "{text}"
        );
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
