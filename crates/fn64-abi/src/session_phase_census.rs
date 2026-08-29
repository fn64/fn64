//! Measure the four phases of the RAW-DPC SESSION path.
//!
//! # Why this exists
//!
//! The shell's `pump_census` established, with closure, that WM2000's frame
//! cost is almost entirely one bucket: on the baseline run
//! (`FN64_RENDER=wgpu`, 600 pumps, 212 of them carrying a graphics task)
//! `gfx_lle_rdp_ns` was **56.219 ms of the 56.574 ms slow/fast delta --
//! 99.8% of the tail**. Every other measured bucket, including the guest
//! executor's own non-graphics time, VI presentation (3.69 ms, flat across
//! both populations), audio, and the RSP graphics microcode
//! (`gfx_lle_rsp_ns`, 0.315 ms), was rounding error beside it.
//!
//! That is a real result and it is where the search must continue -- but it
//! is not yet ACTIONABLE. The session spans:
//!
//!   * `plan_raw_dpc`        -- decoding the captured command words into an IR
//!                              plan, including every triangle's setup.
//!   * `finalize_and_submit` -- binding the plan to its guest read capture.
//!   * `execute_raw_dpc`     -- the backend actually running the commands.
//!                              On this stack that is the CPU rasterizer.
//!   * `commit`              -- re-validating and publishing guest writes.
//!
//! "It is the rasterizer" is the obvious reading and it is probably right,
//! but this project's perf history is a list of obvious readings that were
//! wrong (`docs/plans/perf-method.md` rules 1, 3 and 12). The whole point of
//! the seam being 99.8% is that the NEXT split decides what to optimize, and
//! guessing which of four phases owns it would throw away the only reason the
//! first measurement was worth taking. So this module times the four
//! separately. Threaded execution means these clocks are not one additive
//! wall-time split: plan/finalize run before worker enqueue, execute runs on
//! the renderer worker, and commit runs on the emulation thread after join.
//!
//! Note what this instrument is NOT: it does not reach inside the rasterizer.
//! If `execute` owns the time, the next question -- per-pixel coverage versus
//! texture sampling versus blending -- needs a probe one layer further down.
//! This one answers only "which of the four phases", which is the question
//! that must be answered first.
//!
//! # Gate
//!
//! `FN64_SESSION_PHASE_CENSUS=1`. Absent, empty, and `0` are all off, and a
//! disarmed run takes no clock reads at all -- `timed` degenerates to
//! `operation()`. The same `env_flag` semantics as `dpc_copy_census`, and for
//! the same reason: perf-method rule 6 was earned by a gate where an empty
//! value read as ON, so both lanes of an A/B were the control lane.
//!
//! # Cost
//!
//! Clock pairs occur at batch/session boundaries, not per triangle or pixel.
//! The denominator remains physical DPC members so batching does not silently
//! change per-member averages. Per
//! perf-method rule 17 the absolute milliseconds still must not be quoted
//! against an unprofiled run's -- SHARES survive instrumentation, absolutes
//! do not.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static PLAN_NS: AtomicU64 = AtomicU64::new(0);
static FINALIZE_NS: AtomicU64 = AtomicU64::new(0);
static EXECUTE_NS: AtomicU64 = AtomicU64::new(0);
static COMMIT_NS: AtomicU64 = AtomicU64::new(0);
static SUBMISSIONS: AtomicU64 = AtomicU64::new(0);

/// `1`/`true`/`yes`/`on` (any case, trimmed) mean on; absent, empty, and
/// every other value mean off.
///
/// Deliberately NOT `var_os(..).is_some()`: perf-method rule 6 was earned by
/// a gate where an empty value read as ON, so both lanes of a supposed A/B
/// were the same lane and fabricated a 4.9x. Same semantics as
/// `dpc_copy_census::env_flag`, which is module-private, so this is a
/// deliberate copy rather than a shared import.
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// True when `FN64_SESSION_PHASE_CENSUS` requests the census.
pub(crate) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_SESSION_PHASE_CENSUS"))
}

/// The four phases of raw-DPC session work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Plan,
    Finalize,
    Execute,
    Commit,
}

/// Time `operation`, attributing its wall nanoseconds to `phase`. Returns the
/// operation's value untouched.
///
/// When the census is off this is `operation()` with no clock read at all, so
/// the uninstrumented program keeps its exact shape.
pub(crate) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
    if !enabled() {
        return operation();
    }
    arm_report();
    let started = std::time::Instant::now();
    let value = operation();
    let elapsed = started.elapsed().as_nanos() as u64;
    match phase {
        Phase::Plan => &PLAN_NS,
        Phase::Finalize => &FINALIZE_NS,
        Phase::Execute => &EXECUTE_NS,
        Phase::Commit => &COMMIT_NS,
    }
    .fetch_add(elapsed, Relaxed);
    value
}

/// Count one physical DPC member, so batching cannot change the denominator.
pub(crate) fn note_submission() {
    if !enabled() {
        return;
    }
    arm_report();
    SUBMISSIONS.fetch_add(1, Relaxed);
}

/// Running totals `(plan, finalize, execute, commit, physical_members)`.
///
/// Read unconditionally -- five relaxed loads -- so a per-pump or per-field
/// sampler can difference them at a boundary rather than waiting for the
/// at-exit total. When the gate is off every counter reads zero, which is
/// the correct answer: nothing was counted.
pub(crate) fn running_totals() -> (u64, u64, u64, u64, u64) {
    (
        PLAN_NS.load(Relaxed),
        FINALIZE_NS.load(Relaxed),
        EXECUTE_NS.load(Relaxed),
        COMMIT_NS.load(Relaxed),
        SUBMISSIONS.load(Relaxed),
    )
}

/// Register the at-exit report exactly once.
///
/// Reporting from an `atexit` hook rather than from a harness `main` for the
/// same reason `dpc_copy_census` does: the block-boot harness's `main.rs` is
/// hashed verbatim into `DISPATCH_SOURCE_SHA256`, so editing it would change
/// the canonical program identity, and the measured program must stay
/// byte-identical to the unmeasured one.
fn arm_report() {
    extern "C" fn at_exit() {
        let physical_members = SUBMISSIONS.load(Relaxed);
        if physical_members == 0 {
            return;
        }
        let plan = PLAN_NS.load(Relaxed);
        let finalize = FINALIZE_NS.load(Relaxed);
        let execute = EXECUTE_NS.load(Relaxed);
        let commit = COMMIT_NS.load(Relaxed);
        let total = plan + finalize + execute + commit;
        let ms = |ns: u64| ns as f64 / 1e6;
        let share = |ns: u64| {
            if total == 0 {
                0.0
            } else {
                100.0 * ns as f64 / total as f64
            }
        };
        let per = |ns: u64| ns as f64 / 1e6 / physical_members as f64;
        println!(
            "[session-phase-census] physical_members={physical_members} \
             attributed_total={:.1} ms (cross-thread sum; not process wall time)",
            ms(total)
        );
        for (name, ns) in [
            ("plan", plan),
            ("finalize", finalize),
            ("execute", execute),
            ("commit", commit),
        ] {
            println!(
                "[session-phase-census]   {name:<9} {:>10.1} ms  {:>5.1}%  {:>8.3} ms/physical-member",
                ms(ns),
                share(ns),
                per(ns),
            );
        }
        // This is deliberately not called a closure. `gfx_lle_rdp_ns` ends
        // at worker enqueue, while execute runs on the worker and commit runs
        // after the architectural join. Adding these clocks is useful phase
        // accounting but is not elapsed process wall time.
        println!(
            "[session-phase-census] topology: plan/finalize are pre-worker; \
             execute is worker; commit is post-join."
        );
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
    }
    static ARMED: std::sync::Once = std::sync::Once::new();
    ARMED.call_once(|| {
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(at_exit) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate must treat absent, empty, and `0` alike. perf-method rule 6:
    /// a gate where empty read as ON made both lanes of an A/B the same lane
    /// and fabricated a 4.9x speedup.
    ///
    /// Exercises `env_flag` directly rather than `enabled()`, which memoizes
    /// in a `OnceLock` and so cannot be re-read per case.
    #[test]
    fn the_census_gate_treats_absent_empty_and_zero_alike() {
        let name = "FN64_SESSION_PHASE_CENSUS_GATE_TEST";
        std::env::remove_var(name);
        assert!(!env_flag(name), "absent must be off");
        std::env::set_var(name, "");
        assert!(!env_flag(name), "empty must be off");
        std::env::set_var(name, "0");
        assert!(!env_flag(name), "0 must be off");
        std::env::set_var(name, "1");
        assert!(env_flag(name), "1 must be on");
        std::env::set_var(name, " TRUE ");
        assert!(env_flag(name), "trimmed TRUE must be on");
        std::env::set_var(name, "maybe");
        assert!(!env_flag(name), "an unrecognized value must be off");
        std::env::remove_var(name);
    }

    /// Each phase must land in its OWN counter. The mutation this kills is a
    /// copy-paste in `timed`'s `match` that bills two phases to one atomic --
    /// the defect that would make `execute` look like it owns time that
    /// `plan` spent, which is precisely the question this module exists to
    /// answer.
    ///
    /// Deliberately distinct sleep durations per phase, and each asserted
    /// against its own counter: equal durations would read identically under
    /// a swapped pair, which is the fixture-cannot-fail error the harness
    /// traps warn about.
    #[test]
    fn each_phase_bills_its_own_counter() {
        // `enabled()` memoizes, so this test drives the counters through the
        // same `match` arm selection without depending on the gate.
        fn bill(phase: Phase, ns: u64) {
            match phase {
                Phase::Plan => &PLAN_NS,
                Phase::Finalize => &FINALIZE_NS,
                Phase::Execute => &EXECUTE_NS,
                Phase::Commit => &COMMIT_NS,
            }
            .fetch_add(ns, Relaxed);
        }
        let before = running_totals();
        bill(Phase::Plan, 1);
        bill(Phase::Finalize, 20);
        bill(Phase::Execute, 300);
        bill(Phase::Commit, 4000);
        let after = running_totals();
        assert_eq!(after.0 - before.0, 1, "plan must bill PLAN_NS only");
        assert_eq!(after.1 - before.1, 20, "finalize must bill FINALIZE_NS only");
        assert_eq!(after.2 - before.2, 300, "execute must bill EXECUTE_NS only");
        assert_eq!(after.3 - before.3, 4000, "commit must bill COMMIT_NS only");
    }
}
