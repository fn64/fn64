use super::*;

// ---------------------------------------------------------------------
// VI family: real, host-state-backed implementations (this wave). See
// fn64_runtime::vi's module doc for the design (host hardware STATE model,
// not a VI-manager thread -- that role is the executor's OS_EVENT_VI
// delivery, already wired via osSetEventMesg_recomp + Executor::advance_time
// per rung 11's osCreateViManager evidence). Every real call site's exact
// argument register per profile.toml's byte-cited rung-11 writeup
// (func_80001410's VI bring-up sequence): `osViSetMode(a0=mode_ptr)`,
// `osViSetSpecialFeatures(a0=features_ptr)`, `osViSetYScale(f12=scale)`,
// `osViSwapBuffer(a0=frameBufPtr)`, `osViBlack(a0=active)`.
// ---------------------------------------------------------------------

/// `osCreateViManager(OSPri pri)` -- `a0`=`ctx->r4` (unused; see doc below).
/// A direct `FuncEntry.func` slot in `recomp_overlays.inl` (N64Recomp skips
/// codegen for it entirely, per `games/NWXE/profile.toml`'s rung-11
/// identification of `func_80032B90`). Real libultra semantics spin up a
/// dedicated VI-manager thread that owns retrace/counter event delivery;
/// per `docs/DESIGN.md` section 2's single-executor-coroutine model, that
/// role is already the executor's own `advance_time`/retrace-ticker
/// machinery (`Executor::vi_set_event`/`arm_retrace`, wired this wave) --
/// there is no second host thread to spin up here. This shim's real, tested
/// effect is therefore intentionally a safe no-op beyond existing as a
/// callable symbol: no separate VI-manager state needs establishing that
/// `Executor::new`'s `Default` didn't already establish, matching the same
/// reasoning `osInitialize_recomp`'s doc comment gives for its own no-op
/// status.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreateViManager_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osViSetEvent(OSMesgQueue *mq, OSMesg msg, u32 retraceCount)` -- `a0`=mq
/// (`ctx->r4`), `a1`=msg (`ctx->r5`), `a2`=retraceCount
/// (`ctx->r6`, accepted but not modeled -- see `ViState::set_event`'s doc
/// comment). A direct `FuncEntry.func` slot (rung 11: `func_80032ED0`, exact
/// 0x58 size match vs donor, `->0x10=mq(a0), ->0x14=msg(a1)`) -- writes
/// directly into the VI manager's own internal retrace-notification target,
/// a mechanism `games/NWXE/profile.toml`'s rung-11 writeup documents as
/// DISTINCT from `osSetEventMesg`'s general `OS_EVENT_*` table (both may be
/// registered and both fire on the same retrace tick, per
/// `Executor::advance_time`'s doc comment).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetEvent_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    with_executor(|exec| exec.vi_set_event(mq_addr, msg));
}

/// `osViSetMode(OSViMode *mode)` -- `a0` = `ctx->r4`, the mode-table vram
/// pointer (rung 11: `func_80032F30`, exact 0x4C size match vs donor,
/// `->0x8=mode(s0)`). This crate does not model `OSViMode`'s internal
/// NTSC/PAL timing-register fields (no shim reads them back; storing the
/// raw pointer is the honest state this milestone needs -- see
/// `ViState::set_mode`'s doc comment).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetMode_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mode_ptr = ctx.r4 as u32;
    with_executor(|exec| exec.vi_set_mode(mode_ptr));
}

/// `osViSetSpecialFeatures(OSViSpecialFeatures *sf)` -- `a0` = `ctx->r4`
/// (rung 11: `func_80032F80`, exact 0x164 size match vs donor).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetSpecialFeatures_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let sf_ptr = ctx.r4 as u32;
    with_executor(|exec| exec.vi_set_special_features(sf_ptr));
}

/// `osViSetYScale(f32 scale)` -- the o32 float-argument convention passes
/// `scale` in `$f12`, not a GPR; `recomp_context`'s `f12: Fpr` union's
/// `halves.0` is the float half the compiler emits for a single-precision
/// arg per `recomp.h`'s calling-convention codegen (rung 11: `func_800330F0`,
/// exact 0x44 size match vs donor, `swc1 $fs0 -> 0x24`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetYScale_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let scale = unsafe { ctx.f12.halves.0 };
    with_executor(|exec| exec.vi_set_y_scale(scale));
}

/// `osViSwapBuffer(void *frameBufPtr)` -- `a0` = `ctx->r4` (rung 11:
/// `func_80033140`, exact 0x44 size match vs donor, `->0x4=framebuffer(s0)`).
/// This is the task's framebuffer-capture trigger point: the returned
/// `RdramAddr` is exactly what a host driver (`fn64-shell`/the boot harness)
/// needs to hash/dump the pointed-to fb region on every swap -- see
/// `Executor::vi_swap_buffer`'s doc comment for why the value is handed
/// back rather than only stored.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSwapBuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let frame_buf = RdramAddr::from_gpr(ctx.r4);
    with_executor(|exec| {
        exec.vi_swap_buffer(frame_buf);
    });
    present_render_backend();
}

/// `osViBlack(u8 active)` -- `a0` = `ctx->r4` (rung 11: `func_800334A0`,
/// exact 0x5C size match vs donor, toggles state bit 0x20 set/clear on
/// `arg&0xFF`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViBlack_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let active = (ctx.r4 & 0xFF) != 0;
    with_executor(|exec| exec.vi_set_black(active));
}

/// Read the most recently swapped VI framebuffer's rdram address, if any --
/// the boot harness's polling hook for the task's "on every osViSwapBuffer,
/// hash the pointed-to fb region" requirement, since a harness driving the
/// executor from outside this crate has no other way to observe
/// `ViState::current_framebuffer` (it's private to `fn64-runtime`'s
/// `Executor`, per this crate's existing "no raw field access" convention).
pub fn current_vi_framebuffer() -> Option<u32> {
    with_executor(|exec| exec.vi().current_framebuffer.map(|a| a.offset()))
}

/// The total number of `osViSwapBuffer` calls observed so far -- see
/// `current_vi_framebuffer`'s doc comment for why this crate exposes a
/// plain function rather than requiring the harness to reach into
/// `Executor` directly.
pub fn vi_swap_count() -> u64 {
    with_executor(|exec| exec.vi().swap_count)
}

/// Arm the VI retrace ticker (`Executor::arm_retrace`) -- see
/// `fn64_runtime::vi`'s module doc for why this is a host-chosen
/// approximation, not a hardware-accurate NTSC/PAL constant.
pub fn arm_vi_retrace(interval: u64) {
    with_executor(|exec| exec.arm_retrace(interval));
}

// ---- retrace cadence probe (ROADMAP R5 probe 3) -------------------------
//
// R5's open frontier is whether the guest's VI retrace fires at 60 Hz. The
// game's audio thread produces on every retrace, so an over-delivering ticker
// would explain BOTH R5 symptoms with one cause: static (the ring pegs at its
// cap and drop-oldest skips playback) and the over-speed feel (game logic
// advances too fast). That hypothesis was unfalsifiable because nothing
// measured the rate -- OOT_SWAP_TIMING times the harness's compute, and the
// shell's WaitUntil paces its own PUMP, neither of which is this clock.
//
// The two clocks this separates:
//   - RetraceSchedule::advance fires on GUEST VIRTUAL time and reports every
//     interval crossed (fn64-runtime/src/vi.rs) -- it has no wall-clock notion
//     and cannot self-check.
//   - Wall time only exists host-side. So the correlation lives here, in
//     fn64-abi: fn64-runtime is deliberately wall-clock-free (DESIGN.md §1),
//     and instrumenting it there would break that property for a diagnostic.
//
// Counts only; never gates or paces anything. Report with retrace_cadence().

use std::cell::Cell;
use std::time::Instant;

thread_local! {
    /// Total OS_EVENT_VI ticks the schedule has fired since the first one.
    static RETRACE_TICKS: Cell<u64> = const { Cell::new(0) };
    /// Wall-clock at the FIRST tick -- the baseline every rate is measured
    /// against. Deliberately not process start: arming happens after boot
    /// setup, so process start would dilute the rate with startup time.
    static RETRACE_FIRST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Record `n` retrace ticks as fired. Called from the delivery seam, so it
/// counts what the GUEST actually receives -- not what the schedule computed
/// and not what the host pump requested.
pub(crate) fn note_retrace_ticks(n: u32) {
    if n == 0 {
        return;
    }
    RETRACE_FIRST.with(|f| {
        if f.get().is_none() {
            f.set(Some(Instant::now()));
        }
    });
    RETRACE_TICKS.with(|c| c.set(c.get() + u64::from(n)));
}

/// Observed VI retrace cadence: `(ticks, elapsed_secs, hz)` since the first
/// tick, or `None` before any fired.
///
/// `hz` is the number to read: NTSC wants ~60. Materially above it means the
/// ticker over-delivers and R5 probe 3 is confirmed; at ~60 the cause is
/// elsewhere and probe 3 is REFUTED -- which is worth just as much, since it
/// is currently the leading hypothesis.
///
/// Returns None (not a zeroed struct) before the first tick, so "not armed
/// yet" can never be misread as "0 Hz".
pub fn retrace_cadence() -> Option<(u64, f64, f64)> {
    let first = RETRACE_FIRST.with(Cell::get)?;
    let ticks = RETRACE_TICKS.with(Cell::get);
    let secs = first.elapsed().as_secs_f64();
    // The first tick starts the clock, so it is the baseline, not a sample:
    // N ticks span N-1 intervals. At N=1 there is no interval yet -> 0.0 Hz
    // rather than a divide-by-zero or a fabricated rate.
    let hz = if secs > 0.0 {
        (ticks.saturating_sub(1)) as f64 / secs
    } else {
        0.0
    };
    Some((ticks, secs, hz))
}

#[cfg(test)]
mod retrace_cadence_tests {
    use super::*;

    #[test]
    fn no_ticks_reports_none_rather_than_zero_hz() {
        RETRACE_FIRST.with(|f| f.set(None));
        RETRACE_TICKS.with(|c| c.set(0));
        assert!(
            retrace_cadence().is_none(),
            "before the first tick the probe must say 'no data', never 0 Hz -- \
             an unarmed ticker misread as a stalled one would send R5 chasing a \
             ghost"
        );
    }

    #[test]
    fn ticks_accumulate_and_report_a_rate() {
        RETRACE_FIRST.with(|f| f.set(None));
        RETRACE_TICKS.with(|c| c.set(0));
        note_retrace_ticks(1);
        note_retrace_ticks(3);
        let (ticks, secs, _hz) = retrace_cadence().expect("armed after a tick");
        assert_eq!(ticks, 4, "every fired tick counts, including batched ones");
        assert!(secs >= 0.0);
    }

    #[test]
    fn zero_ticks_does_not_arm_the_baseline() {
        RETRACE_FIRST.with(|f| f.set(None));
        RETRACE_TICKS.with(|c| c.set(0));
        note_retrace_ticks(0);
        assert!(
            retrace_cadence().is_none(),
            "a 0-tick call means the schedule fired nothing; it must not start \
             the clock, or the measured window would begin before any retrace"
        );
    }
}

/// `osViGetCurrentFramebuffer(void) -> void*` -- no arguments; returns the
/// currently-displayed (not next-queued) framebuffer's vram pointer.
/// Function-table slot only (`recomp_overlays.inl:2974`). This crate's
/// `ViState` (`vi.rs`) tracks only ONE "most recently swapped" framebuffer
/// field (`current_framebuffer`, already exposed via
/// `current_vi_framebuffer`) -- no separate "currently displayed vs. next
/// queued" double-buffer distinction exists yet, so this returns the same
/// value `osViSwapBuffer`'s last call recorded, an honest approximation
/// (single most-recent value) rather than a fabricated second buffer this
/// crate has no state for.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViGetCurrentFramebuffer_recomp(
    _rdram: *mut u8,
    ctx: *mut RecompContext,
) {
    let ctx = unsafe { &mut *ctx };
    let fb = with_executor(|exec| exec.vi().current_framebuffer);
    // Return the SIGN-EXTENDED KSEG0 pointer the game passed to
    // osViSwapBuffer, NOT the bare physical offset: Sched_HandleRetrace
    // compares this `$v0` result `==` against `pendingSwapBuf1->swapBuffer`
    // (funcs_41.c PC 0x800A3288, `bnel $v1, $v0`), and that operand is loaded
    // via `MEM_W` = `*(int32_t*)` -- SIGN-extended, so a KSEG0 fb like
    // 0x803B5000 arrives as 0xFFFFFFFF_803B5000. Returning a zero-extended
    // u32 (0x00000000_803B5000) made `bnel` see them as unequal and the
    // framebuffer-swap chain that drives every frame-2+ swap never advanced
    // (curBuf/pendingSwapBuf1 frozen), stalling boot at exactly 1 swap.
    // `as i32 as u64` reproduces MEM_W's own sign extension. See
    // `RdramAddr::to_kseg0`.
    ctx.r2 = fb.map(|a| a.to_kseg0() as i32 as u64).unwrap_or(0);
}

/// `osViGetNextFramebuffer(void) -> void*` -- same reasoning/return value
/// as `osViGetCurrentFramebuffer_recomp` (this crate has no separate
/// pending-vs-current double-buffer state; both report the same most-
/// recent swap). Function-table slot only (`recomp_overlays.inl:65`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViGetNextFramebuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let fb = with_executor(|exec| exec.vi().current_framebuffer);
    // Sign-extended KSEG0 pointer, same reasoning as
    // osViGetCurrentFramebuffer_recomp (MEM_W-compatible sign extension).
    ctx.r2 = fb.map(|a| a.to_kseg0() as i32 as u64).unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn os_vi_set_mode_stores_mode_ptr_and_swap_buffer_updates_current_framebuffer() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8004_1234;
        unsafe { osViSetMode_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        with_executor(|exec| assert_eq!(exec.vi().mode_ptr, Some(0x8004_1234)));

        let mut swap_ctx = ctx_zeroed();
        swap_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut swap_ctx as *mut _) };
        assert_eq!(current_vi_framebuffer(), Some(0x10_0000));
        assert_eq!(vi_swap_count(), 1);
    }

    #[test]
    fn os_vi_swap_buffer_presents_the_registered_render_backend() {
        use fn64_render::{FrameStatus, RenderConfig, RenderError, UcodeId};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        struct CountingBackend(Arc<AtomicU32>);

        impl RenderBackend for CountingBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(&mut self) -> Result<(), RenderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let presents = Arc::new(AtomicU32::new(0));
        set_render_backend(Box::new(CountingBackend(Arc::clone(&presents))), 0);

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };

        assert_eq!(presents.load(Ordering::SeqCst), 1);
        assert_eq!(last_render_error(), None);
    }
}
