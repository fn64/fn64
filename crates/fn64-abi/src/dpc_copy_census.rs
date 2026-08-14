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

/// How much of the copyback is real, and what it costs to find out.
///
/// WHY: `copy_back` moves the same 8 MiB as `copy_in` and measures **3.4x
/// slower** -- 455.9 vs 133.2 us/call, 17.1 vs 58.7 GB/s (rt64-rep1/rep2,
/// two reps agreeing to 1.8%). A bulk memcpy does not vary by 3.4x on byte
/// count alone, so most of that excess is what writing into the LIVE mapping
/// costs versus streaming into fresh `mmap` pages -- page faults against the
/// re-armed mprotect barrier among them.
///
/// That makes "copy back only the bytes the renderer actually changed" the
/// obvious move, and it is exactly the shape perf-method rule 12 was earned
/// by: a 5.92 GB clone whose complete elimination measured **+0.84%, the
/// wrong direction**. So the question is settled by COUNTING before anything
/// is optimized (rule 3): if the renderer dirties most of the image, a
/// compare-then-copy is pure added cost and the idea dies here.
///
/// `DIFF_NS` times the comparison itself so the tradeoff is visible rather
/// than inferred -- the scan is only worth it if it costs less than the
/// copyback it avoids.
static COPY_BACK_CHANGED_BYTES: AtomicU64 = AtomicU64::new(0);
static COPY_BACK_CHANGED_RUNS: AtomicU64 = AtomicU64::new(0);
static DIFF_NS: AtomicU64 = AtomicU64::new(0);

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

/// Measure how much of the staged image the renderer actually changed.
///
/// Runs ONLY under the census gate, and deliberately does no copying: it is a
/// pure observation of the quantity that decides whether narrowing the
/// copyback can pay. Returns immediately when disarmed, so the shipped
/// program keeps its exact shape.
///
/// `changed` is the byte count that differs, `runs` the number of maximal
/// differing spans -- both matter. A million changed bytes in one contiguous
/// run is one `memcpy`; the same count in ten thousand runs is ten thousand
/// short ones, and the second is not obviously cheaper than copying the lot.
pub(crate) fn note_copy_back_diff(staged: &[u8], live: &[u8]) {
    if !enabled() {
        return;
    }
    arm_report();
    debug_assert_eq!(
        staged.len(),
        live.len(),
        "copyback diff census compares the staged prefix against live RDRAM"
    );
    let started = std::time::Instant::now();
    let mut changed = 0u64;
    let mut runs = 0u64;
    let mut index = 0usize;
    let len = staged.len().min(live.len());
    while index < len {
        if staged[index] == live[index] {
            index += 1;
            continue;
        }
        runs += 1;
        let start = index;
        while index < len && staged[index] != live[index] {
            index += 1;
        }
        changed += (index - start) as u64;
    }
    DIFF_NS.fetch_add(started.elapsed().as_nanos() as u64, Relaxed);
    COPY_BACK_CHANGED_BYTES.fetch_add(changed, Relaxed);
    COPY_BACK_CHANGED_RUNS.fetch_add(runs, Relaxed);
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

/// The staging-copy phase timers, for per-field sampling.
///
/// These previously existed only as a whole-run `atexit` summary, which is why
/// a per-field figure for them could not be found in a frozen benchmark log:
/// the numbers were never sampled per field, so no amount of re-reading an old
/// log would produce them. The staging memcpy is now a live optimization
/// target, and a target needs a per-field cost against the 16.667 ms budget --
/// not a run total that hides which population pays it.
///
/// Returned as `(alloc, copy_in, copy_back)` in nanoseconds.
pub(crate) fn staging_totals() -> (u64, u64, u64) {
    (
        ALLOC_NS.load(Relaxed),
        COPY_IN_NS.load(Relaxed),
        COPY_BACK_NS.load(Relaxed),
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
        let changed = COPY_BACK_CHANGED_BYTES.load(Relaxed);
        let runs = COPY_BACK_CHANGED_RUNS.load(Relaxed);
        let diff_ns = DIFF_NS.load(Relaxed);
        let copy_back_bytes = COPY_BACK_BYTES.load(Relaxed);
        if copy_back_bytes > 0 && diff_ns > 0 {
            // The decision line. `changed_share` is what perf-method rule 12
            // asks for: if the renderer dirties most of the image there is
            // nothing to narrow, and `diff_us/call` says whether finding out
            // costs less than the copyback it would avoid.
            println!(
                "[dpc-copyback-diff] changed_bytes={changed} of {copy_back_bytes} \
                 ({:.4}% of the image) runs={runs} ({:.1} runs/call, {:.0} bytes/run) \
                 diff_ms={:.3} ({:.3} us/call) vs copy_back {:.3} us/call",
                changed as f64 / copy_back_bytes as f64 * 100.0,
                runs as f64 / calls as f64,
                if runs > 0 { changed as f64 / runs as f64 } else { 0.0 },
                ms(diff_ns),
                per_call_us(diff_ns),
                per_call_us(copy_back),
            );
        }
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

    /// The copyback diff must COUNT, and it must count the right thing.
    ///
    /// perf-method rule 6a: before trusting a check, confirm it CAN fail. The
    /// decision this counter drives is "is the changed fraction small enough
    /// that narrowing the copyback pays", so a counter that reported a small
    /// fraction regardless of the input would be unfalsifiable in exactly the
    /// direction that ships a wrong optimization.
    ///
    /// Exercises the scan directly rather than through `note_copy_back_diff`,
    /// which is gated on a memoized `OnceLock` and accumulates into process
    /// statics that other tests in this binary also write.
    #[test]
    fn the_copyback_diff_counts_changed_bytes_and_runs() {
        // Same closed-form scan the census performs, lifted so the accounting
        // can be asserted without touching the shared counters.
        fn scan(staged: &[u8], live: &[u8]) -> (u64, u64) {
            let (mut changed, mut runs, mut index) = (0u64, 0u64, 0usize);
            let len = staged.len().min(live.len());
            while index < len {
                if staged[index] == live[index] {
                    index += 1;
                    continue;
                }
                runs += 1;
                let start = index;
                while index < len && staged[index] != live[index] {
                    index += 1;
                }
                changed += (index - start) as u64;
            }
            (changed, runs)
        }
        assert_eq!(scan(&[0; 64], &[0; 64]), (0, 0), "identical must report nothing");
        assert_eq!(scan(&[1; 64], &[0; 64]), (64, 1), "wholly different is ONE run");
        // Two disjoint runs, so a counter that merely summed bytes and called
        // every difference one span would fail here.
        let mut staged = [0u8; 16];
        staged[2] = 1;
        staged[3] = 1;
        staged[9] = 1;
        assert_eq!(scan(&staged, &[0; 16]), (3, 2));
        // Adjacent differing bytes are one run, not two.
        assert_eq!(scan(&[0, 1, 1, 0], &[0, 0, 0, 0]), (2, 1));
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
