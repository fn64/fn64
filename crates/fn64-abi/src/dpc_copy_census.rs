//! Isolate the cost of `dispatch_captured_raw_rdp`'s whole-RDRAM staging copy.
//!
//! WHY THIS EXISTS, and why it measures the copy and NOT the seam.
//!
//! `dispatch_captured_raw_rdp` allocates a `staged_end`-byte image (physical
//! RDRAM plus the captured DPC command words), copies all of physical RDRAM
//! into it, hands it to the renderer, and copies the mutated prefix back. On
//! WM2000's gameplay route that is 8 MiB in and 8 MiB out per DPC submission,
//! ~129.6 GB over the route.
//!
//! That byte count is **not evidence**. `docs/plans/perf-method.md` rule 12 was
//! earned by a 5.92 GB clone with exactly this shape whose complete elimination
//! measured **+0.84%, the wrong direction**. And the seam's existing
//! `gfx_lle_rdp_ns` counter cannot settle it either: that timer spans the whole
//! call, so it bills the preflight scan, the renderer dispatch, the mutation
//! tracking, and (before `abc7871`) a large amount of mutation-journal guard
//! work entering through this seam. Reading it as the copy's cost is rule 2's
//! error -- inclusive time read as self time.
//!
//! So this module times the three sub-operations SEPARATELY, each around the
//! narrowest possible region:
//!
//!   * `alloc`    -- `vec![0u8; staged_end]`, the zeroing allocation alone.
//!   * `copy_in`  -- `image[..physical_len].copy_from_slice(real)`.
//!   * `copy_back`-- the `track_rdp_renderer_mutation` copyback of the prefix.
//!
//! Subtracting these from `gfx_lle_rdp_ns` gives what the seam spends on
//! everything else, which is the number that says whether candidate 0 is a
//! target or another rule-12 entry.
//!
//! Gated on `FN64_DPC_COPY_CENSUS` so an unmeasured run pays nothing but an
//! already-resolved `OnceLock` load. Reporting happens from an `atexit` hook
//! registered here rather than from `examples/wm2000-block-boot/src/main.rs`,
//! because that file is hashed verbatim into `DISPATCH_SOURCE_SHA256`
//! (`build.rs`) and editing it would change the canonical program identity --
//! the measured program must stay byte-identical to the unmeasured one. This
//! mirrors `write_barrier`'s `mprotect-barrier` stats reporter.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static ALLOC_NS: AtomicU64 = AtomicU64::new(0);
static COPY_IN_NS: AtomicU64 = AtomicU64::new(0);
static COPY_BACK_NS: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static COPY_IN_BYTES: AtomicU64 = AtomicU64::new(0);
static COPY_BACK_BYTES: AtomicU64 = AtomicU64::new(0);

/// RSP interpreter instructions retired, split by branch, plus the number of
/// `run_imem` re-entries and the IMEM word-vector rebuilds those cost.
///
/// WHY THESE LIVE HERE. `gfx_lle_rsp_ms` and `audio_lle_ms` are the two
/// largest graphics/audio lines after the executor, but the phase counters
/// report only wall time -- there is no instruction count anywhere, so
/// "expensive per instruction" and "cheap per instruction, executed
/// enormously often" are indistinguishable. That is precisely the question
/// perf-method rule 3 says to COUNT rather than infer, and the distinction
/// decides whether the lever is the interpreter's inner loop or the number of
/// instructions the guest asks it to run.
///
/// Note also that `AUDIO_LLE_RSP_NS` currently reads zero on every run:
/// `rsp_commit.rs` arms its per-chunk RSP timer with
/// `gfx_started.map(..)`, which is `None` on the audio branch, so the audio
/// lane's RSP time is never separated out. These counters do not depend on
/// that timer and therefore cover both branches.
static RSP_STEPS_GFX: AtomicU64 = AtomicU64::new(0);
static RSP_STEPS_AUDIO: AtomicU64 = AtomicU64::new(0);
static RSP_ENTRIES: AtomicU64 = AtomicU64::new(0);
static RSP_IMEM_REBUILD_WORDS: AtomicU64 = AtomicU64::new(0);

/// `1`/`true`/`yes`/`on` (any case, trimmed) mean on; absent, empty, and every
/// other value mean off.
///
/// Deliberately NOT `var_os(..).is_some()`: perf-method rule 6 was earned by a
/// gate where an empty value read as ON, so both lanes of a supposed A/B were
/// the same lane and fabricated a 4.9x. Same semantics as
/// `write_barrier::env_flag` and `frame_census::env_flag`, which are each
/// module-private, so this is a third copy rather than a shared import.
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// True when `FN64_DPC_COPY_CENSUS` requests the census.
pub(crate) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| env_flag("FN64_DPC_COPY_CENSUS"))
}

/// Time `operation`, attributing its wall nanoseconds to `phase`, and record
/// `bytes` moved. Returns the operation's value untouched.
///
/// When the census is off this is `operation()` with no clock read at all, so
/// the uninstrumented program keeps its exact shape.
pub(crate) fn timed<R>(phase: Phase, bytes: u64, operation: impl FnOnce() -> R) -> R {
    if !enabled() {
        return operation();
    }
    arm_report();
    let started = std::time::Instant::now();
    let value = operation();
    let elapsed = started.elapsed().as_nanos() as u64;
    let (ns, byte_counter) = match phase {
        Phase::Alloc => (&ALLOC_NS, &ALLOC_BYTES),
        Phase::CopyIn => (&COPY_IN_NS, &COPY_IN_BYTES),
        Phase::CopyBack => (&COPY_BACK_NS, &COPY_BACK_BYTES),
    };
    ns.fetch_add(elapsed, Relaxed);
    byte_counter.fetch_add(bytes, Relaxed);
    value
}

/// Count one `dispatch_captured_raw_rdp` entry, so the report can divide.
pub(crate) fn note_call() {
    if !enabled() {
        return;
    }
    arm_report();
    CALLS.fetch_add(1, Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Alloc,
    CopyIn,
    CopyBack,
}

/// Record one `run_imem` re-entry: the instructions it retired, whether it was
/// the graphics or the audio branch, and the IMEM words rebuilt to make the
/// call. `words` is the length of the `Vec<u32>` the caller reconstructs from
/// the IMEM image before every chunk.
pub(crate) fn note_rsp_chunk(graphics: bool, steps: u64, words: u64) {
    if !enabled() {
        return;
    }
    arm_report();
    RSP_ENTRIES.fetch_add(1, Relaxed);
    RSP_IMEM_REBUILD_WORDS.fetch_add(words, Relaxed);
    if graphics {
        RSP_STEPS_GFX.fetch_add(steps, Relaxed);
    } else {
        RSP_STEPS_AUDIO.fetch_add(steps, Relaxed);
    }
}

/// Running totals for the counters `frame_census` samples per VI field.
///
/// `(rsp_steps_gfx, rsp_steps_audio, rsp_entries, dpc_calls)`. Read
/// unconditionally -- four relaxed atomic loads -- because the bimodal census
/// arms this module's counters through its own gate and needs the running
/// value at a field boundary, not the at-exit total. When
/// `FN64_DPC_COPY_CENSUS` is off every counter reads zero, which is the
/// correct answer: nothing was counted.
pub(crate) fn running_totals() -> (u64, u64, u64, u64) {
    (
        RSP_STEPS_GFX.load(Relaxed),
        RSP_STEPS_AUDIO.load(Relaxed),
        RSP_ENTRIES.load(Relaxed),
        CALLS.load(Relaxed),
    )
}

fn arm_report() {
    extern "C" fn at_exit() {
        let calls = CALLS.load(Relaxed);
        if calls == 0 {
            return;
        }
        let alloc = ALLOC_NS.load(Relaxed);
        let copy_in = COPY_IN_NS.load(Relaxed);
        let copy_back = COPY_BACK_NS.load(Relaxed);
        let total = alloc + copy_in + copy_back;
        let ms = |ns: u64| ns as f64 / 1e6;
        let per_call_us = |ns: u64| ns as f64 / calls as f64 / 1e3;
        let gib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        println!(
            "[dpc-copy-census] calls={calls} \
             alloc_ms={:.3} ({:.3} us/call, {:.2} GiB) \
             copy_in_ms={:.3} ({:.3} us/call, {:.2} GiB) \
             copy_back_ms={:.3} ({:.3} us/call, {:.2} GiB) \
             total_ms={:.3} ({:.3} us/call)",
            ms(alloc),
            per_call_us(alloc),
            gib(ALLOC_BYTES.load(Relaxed)),
            ms(copy_in),
            per_call_us(copy_in),
            gib(COPY_IN_BYTES.load(Relaxed)),
            ms(copy_back),
            per_call_us(copy_back),
            gib(COPY_BACK_BYTES.load(Relaxed)),
            ms(total),
            per_call_us(total),
        );
        let gfx_steps = RSP_STEPS_GFX.load(Relaxed);
        let audio_steps = RSP_STEPS_AUDIO.load(Relaxed);
        let entries = RSP_ENTRIES.load(Relaxed);
        if entries > 0 {
            println!(
                "[rsp-step-census] entries={entries} \
                 gfx_steps={gfx_steps} audio_steps={audio_steps} total_steps={} \
                 steps_per_entry={:.1} imem_rebuild_words={} ({:.2} GiB of u32 rebuild)",
                gfx_steps + audio_steps,
                (gfx_steps + audio_steps) as f64 / entries as f64,
                RSP_IMEM_REBUILD_WORDS.load(Relaxed),
                RSP_IMEM_REBUILD_WORDS.load(Relaxed) as f64 * 4.0 / (1024.0 * 1024.0 * 1024.0),
            );
        }
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
        let name = "FN64_DPC_COPY_CENSUS_GATE_TEST";
        std::env::remove_var(name);
        assert!(!env_flag(name), "absent must be off");
        std::env::set_var(name, "");
        assert!(!env_flag(name), "EMPTY MUST BE OFF -- this is rule 6");
        std::env::set_var(name, "0");
        assert!(!env_flag(name), "0 must be off");
        for on in ["1", "true", "TRUE", "yes", "on", " 1 "] {
            std::env::set_var(name, on);
            assert!(env_flag(name), "{on:?} must be on");
        }
        std::env::remove_var(name);
    }

    /// `timed` must return the operation's value unchanged whether or not the
    /// census is on -- an instrument that alters the program measures a
    /// different program.
    #[test]
    fn timed_is_value_transparent() {
        assert_eq!(timed(Phase::Alloc, 0, || 41 + 1), 42);
        assert_eq!(timed(Phase::CopyIn, 0, || "x".to_string()), "x");
        assert_eq!(timed(Phase::CopyBack, 0, || ()), ());
    }

    /// The three phases must accumulate into distinct counters. A single
    /// shared counter would report the copy's cost as the allocation's and
    /// make the whole measurement unfalsifiable.
    #[test]
    fn the_three_phases_are_distinct_counters() {
        assert_ne!(Phase::Alloc, Phase::CopyIn);
        assert_ne!(Phase::CopyIn, Phase::CopyBack);
        assert_ne!(Phase::Alloc, Phase::CopyBack);
        let counters = [
            std::ptr::addr_of!(ALLOC_NS),
            std::ptr::addr_of!(COPY_IN_NS),
            std::ptr::addr_of!(COPY_BACK_NS),
        ];
        for (index, first) in counters.iter().enumerate() {
            for second in &counters[index + 1..] {
                assert_ne!(*first, *second, "phase counters must not alias");
            }
        }
    }
}
