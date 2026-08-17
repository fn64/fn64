use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static ENABLED: AtomicBool = AtomicBool::new(false);
static INIT: AtomicBool = AtomicBool::new(false);
static VTX_LOGGED: AtomicU64 = AtomicU64::new(0);
// clip-w histogram counters for the frame:
pub static W_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static W_ONSCREEN: AtomicU64 = AtomicU64::new(0);
pub static W_PATHOLOGICAL: AtomicU64 = AtomicU64::new(0);
// screen-space depth (pz) range tracker (stored as i32 bits of f32):
pub static PZ_MIN: AtomicU64 = AtomicU64::new(u64::MAX);
pub static PZ_MAX: AtomicU64 = AtomicU64::new(0);

/// Record one screen-space depth `pz` into the frame's [min,max] tracker.
pub fn note_pz(pz: f32) {
    if !on() || !pz.is_finite() {
        return;
    }
    // Offset f32 into a monotonic u64 key so min/max compares work.
    let key = (pz * 1000.0) as i64 + (1i64 << 40);
    let key = key.max(0) as u64;
    PZ_MIN.fetch_min(key, Ordering::Relaxed);
    PZ_MAX.fetch_max(key, Ordering::Relaxed);
}

pub fn on() -> bool {
    if crate::speculative_observations_suppressed() {
        return false;
    }
    if !INIT.swap(true, Ordering::Relaxed) {
        ENABLED.store(crate::debug_flag("FN64_DUMP_PROJ"), Ordering::Relaxed);
    }
    ENABLED.load(Ordering::Relaxed)
}
/// Only log the first N vertices verbosely, but keep counting all.
pub fn should_log_vtx() -> bool {
    on() && VTX_LOGGED.fetch_add(1, Ordering::Relaxed) < 24
}
/// Reset per-frame counters so a summary reflects ONE frame, not the
/// cumulative boot. Called at the start of each F3DEX2 decode.
pub fn reset_frame() {
    if !on() {
        return;
    }
    W_TOTAL.store(0, Ordering::Relaxed);
    W_ONSCREEN.store(0, Ordering::Relaxed);
    W_PATHOLOGICAL.store(0, Ordering::Relaxed);
    PZ_MIN.store(u64::MAX, Ordering::Relaxed);
    PZ_MAX.store(0, Ordering::Relaxed);
    VTX_LOGGED.store(0, Ordering::Relaxed);
}
pub fn note_w(w: f32, onscreen: bool) {
    if !on() {
        return;
    }
    W_TOTAL.fetch_add(1, Ordering::Relaxed);
    if onscreen {
        W_ONSCREEN.fetch_add(1, Ordering::Relaxed);
    }
    if !w.is_finite() || w.abs() > 1.0e5 {
        W_PATHOLOGICAL.fetch_add(1, Ordering::Relaxed);
    }
}
pub fn summary() {
    if !on() {
        return;
    }
    let t = W_TOTAL.load(Ordering::Relaxed);
    let on = W_ONSCREEN.load(Ordering::Relaxed);
    let path = W_PATHOLOGICAL.load(Ordering::Relaxed);
    if t > 0 {
        let pzmin = (PZ_MIN.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
        let pzmax = (PZ_MAX.load(Ordering::Relaxed) as i64 - (1i64 << 40)) as f64 / 1000.0;
        eprintln!(
            "[FN64_DUMP_PROJ] SUMMARY: {t} projected vtx | on-screen NDC-cube: {on} ({:.1}%) | pathological |w|>1e5 or non-finite: {path} ({:.1}%) | screen-z(pz) range [{pzmin:.2}, {pzmax:.2}] (nearer=smaller, z-test is `z<depth`)",
            100.0 * on as f64 / t as f64,
            100.0 * path as f64 / t as f64
        );
    }
}
