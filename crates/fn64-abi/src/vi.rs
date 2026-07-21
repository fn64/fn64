use super::*;

// ---------------------------------------------------------------------
// VI family: real, host-state-backed implementations (this wave). See
// fn64_runtime::vi's module doc for the design (host hardware STATE model,
// not a VI-manager thread -- that role is the executor's OS_EVENT_VI
// delivery, already wired via osSetEventMesg_recomp + Executor::advance_time
// per rung 11's osCreateViManager evidence). Every real call site's exact
// argument register per profile.toml's byte-cited rung-11 writeup
// (func_80001410's VI bring-up sequence): `osViSetMode(a0=mode_ptr)`,
// `osViSetSpecialFeatures(a0=feature commands)`, X/Y scale (`f12=scale`),
// `osViSwapBuffer(a0=frameBufPtr)`, `osViBlack(a0=active)`.
// ---------------------------------------------------------------------

/// `osCreateViManager(OSPri pri)` -- `a0`=`ctx->r4` (unused; see doc below).
/// A direct `FuncEntry.func` slot in `recomp_overlays.inl` (N64Recomp skips
/// codegen for it entirely, per `games/NWXE/profile.toml`'s rung-11
/// identification of `func_80032B90`). Real libultra semantics spin up a
/// dedicated VI-manager thread that owns retrace/counter event delivery;
/// per `docs/DESIGN.md` section 2's single-executor-coroutine model, that
/// role is already the shared `DeviceFabric` VI schedule plus the executor's
/// `deliver_vi_retrace` queue machinery --
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
/// (`ctx->r6`). A direct `FuncEntry.func` slot (rung 11: `func_80032ED0`, exact
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
    let retrace_count = ctx.r6 as u32;
    with_executor(|exec| exec.vi_set_event(mq_addr, msg, retrace_count));
}

/// `osViSetMode(OSViMode *mode)` -- `a0` = `ctx->r4`, the mode-table vram
/// pointer (rung 11: `func_80032F30`, exact 0x4C size match vs donor,
/// `->0x8=mode(s0)`). The public libultra `os_vi.h` definitions give the
/// clean-room `OSViMode`/`OSViCommonRegs`/`OSViFieldRegs` layout. Decode its
/// common and both field register images now, then let the shared VI device
/// latch the common image and parity-selected field at V-blank.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetMode_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mode_ptr = ctx.r4 as u32;
    assert!(!rdram.is_null(), "osViSetMode: null RDRAM pointer");
    let mode = RdramAddr::from_gpr(ctx.r4);
    let storage = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let read = |offset| unsafe {
        storage.read_u32(
            mode.checked_add(offset)
                .expect("osViSetMode OSViMode field address overflow"),
        )
    };
    let registers = [
        read(4),  // VI_STATUS <- common.ctrl
        0,        // VI_ORIGIN <- framebuffer base + selected field.origin
        read(8),  // VI_WIDTH <- common.width
        0,        // VI_INTR <- selected field.vIntr
        0,        // VI_CURRENT is sampled/ack-only, never mode state
        read(12), // VI_BURST <- common.burst
        read(16), // VI_V_SYNC <- common.vSync
        read(20), // VI_H_SYNC <- common.hSync
        read(24), // VI_LEAP <- common.leap
        read(28), // VI_H_START <- common.hStart
        0,        // VI_V_START <- selected field.vStart
        0,        // VI_V_BURST <- selected field.vBurst
        read(32), // VI_X_SCALE <- common.xScale
        0,        // VI_Y_SCALE <- selected field.yScale
    ];
    let fields = [
        [read(40), read(44), read(48), read(52), read(56)],
        [read(60), read(64), read(68), read(72), read(76)],
    ];
    crate::pi::queue_live_vi_mode(registers, fields);
    with_executor(|exec| exec.vi_set_mode(mode_ptr));
}

/// `osViSetSpecialFeatures(u32 func)` -- `a0` = `ctx->r4` (rung 11:
/// `func_80032F80`, exact 0x164 size match vs donor). The public VI manual
/// defines ON/OFF command pairs for gamma, gamma dither, divot, and the
/// dither filter; calls accumulate against the queued mode/control image and
/// latch at the next retrace.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetSpecialFeatures_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let features = ctx.r4 as u32;
    crate::pi::queue_live_vi_special_features(features);
    with_executor(|exec| exec.vi_set_special_features(features));
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
    crate::pi::queue_live_vi_y_scale(scale);
    with_executor(|exec| exec.vi_set_y_scale(scale));
}

/// `osViSetXScale(f32 scale)` -- horizontal counterpart to
/// `osViSetYScale_recomp`, with the same public libultra single-precision
/// signature and therefore the same o32 `$f12` argument representation.
/// The VI state retains both axes independently so later register-level VI
/// output can consume the requested horizontal scale without an ABI change.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetXScale_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let scale = unsafe { ctx.f12.halves.0 };
    crate::pi::queue_live_vi_x_scale(scale);
    with_executor(|exec| exec.vi_set_x_scale(scale));
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

/// `osViFade(u8 active, u16 factor)` -- public VI-manager scanout effect.
/// The 10-bit factor interpolates corresponding pixels from the first two
/// framebuffer rows; the latched result fills the screen at V-blank.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViFade_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let active = (ctx.r4 & 0xff) != 0;
    let factor = (ctx.r5 & 0xffff) as u16;
    with_executor(|exec| exec.vi_set_fade(active, factor));
}

/// `osViRepeatLine(u8 active)` -- repeat the first framebuffer row over the
/// entire scanout image beginning at the next V-blank.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViRepeatLine_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    with_executor(|exec| exec.vi_set_repeat_line((ctx.r4 & 0xff) != 0));
}

/// Read the currently displayed VI framebuffer's RDRAM address, if any --
/// the boot harness's polling hook for the task's "on every osViSwapBuffer,
/// hash the pointed-to fb region" requirement, since a harness driving the
/// executor from outside this crate has no other way to observe
/// `ViState::current_framebuffer` (it's private to `fn64-runtime`'s
/// `Executor`, per this crate's existing "no raw field access" convention).
pub fn current_vi_framebuffer() -> Option<u32> {
    with_executor(|exec| exec.vi().current_framebuffer.map(|a| a.offset()))
}

/// Read the framebuffer most recently queued for the next V-blank.
pub fn next_vi_framebuffer() -> Option<u32> {
    with_executor(|exec| exec.vi().next_framebuffer.map(|a| a.offset()))
}

/// The total number of `osViSwapBuffer` calls observed so far -- see
/// `current_vi_framebuffer`'s doc comment for why this crate exposes a
/// plain function rather than requiring the harness to reach into
/// `Executor` directly.
pub fn vi_swap_count() -> u64 {
    with_executor(|exec| exec.vi().swap_count)
}

/// Install an explicit VI field-duration override for compatibility tests or
/// embedders without IPL state. Production boot uses `configure_tv_type`,
/// allowing live VI timing registers to derive the interval.
pub fn arm_vi_retrace(interval: u64) {
    crate::pi::arm_live_vi(interval)
        .unwrap_or_else(|error| panic!("arm_vi_retrace failed: {error}"));
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
/// Function-table slot only (`recomp_overlays.inl:2974`). A pending swap is
/// distinct and does not become current until the next VI interrupt.
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

/// `osViGetNextFramebuffer(void) -> void*` -- returns the most recent pending
/// swap even before V-blank. Function-table slot only
/// (`recomp_overlays.inl:65`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViGetNextFramebuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let fb = with_executor(|exec| exec.vi().next_framebuffer);
    // Sign-extended KSEG0 pointer, same reasoning as
    // osViGetCurrentFramebuffer_recomp (MEM_W-compatible sign extension).
    ctx.r2 = fb.map(|a| a.to_kseg0() as i32 as u64).unwrap_or(0);
}

/// `osViGetCurrentLine(void) -> u32` samples the public VI_CURRENT half-line
/// register. In interlaced mode its low bit is the current field number.
///
/// # Safety
/// `ctx` must point to a live writable recompilation context.
#[no_mangle]
pub unsafe extern "C" fn osViGetCurrentLine_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = u64::from(
        crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0010)
            .expect("VI_CURRENT register is not mapped"),
    );
}

/// `osViGetCurrentField(void) -> u32` returns zero for non-interlaced output
/// and the alternating VI field bit for interlaced output.
///
/// # Safety
/// `ctx` must point to a live writable recompilation context.
#[no_mangle]
pub unsafe extern "C" fn osViGetCurrentField_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = u64::from(crate::pi::live_vi_field());
}

/// `osViGetStatus(void) -> u32` reads the live VI status/control register.
///
/// # Safety
/// `ctx` must point to a live writable recompilation context.
#[no_mangle]
pub unsafe extern "C" fn osViGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = u64::from(
        crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0000)
            .expect("VI_STATUS register is not mapped"),
    );
}

/// `osViGetCurrentMode(void) -> OSViMode*` returns the mode latched at the
/// last VI interrupt, not a newly queued mode. The pointer retains the same
/// sign-extended KSEG0 representation as the framebuffer query shims.
///
/// # Safety
/// `ctx` must point to a live writable recompilation context.
#[no_mangle]
pub unsafe extern "C" fn osViGetCurrentMode_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mode = with_executor(|exec| exec.vi().mode_ptr);
    ctx.r2 = mode.map(|pointer| pointer as i32 as u64).unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use fn64_render::{FrameStatus, RenderConfig, RenderError, UcodeId};
    use std::sync::{Arc, Mutex};

    struct ViCaptureBackend {
        presentations: Arc<Mutex<Vec<fn64_render::ViPresentation>>>,
    }

    struct ReferencePixelBackend {
        inner: fn64_render_reference::ReferenceBackend,
        frames: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl RenderBackend for ViCaptureBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        fn observe_non_rdp_write16(
            &mut self,
            _write: fn64_render::NonRdpWrite16,
        ) -> fn64_render::NonRdpWrite16Disposition {
            fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
        }

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(FrameStatus::Complete)
        }

        fn present(&mut self, request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
            let (presentation, _) = request.into_parts();
            self.presentations.lock().unwrap().push(presentation);
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    impl RenderBackend for ReferencePixelBackend {
        fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
            self.inner.create(cfg)
        }

        fn observe_non_rdp_write16(
            &mut self,
            write: fn64_render::NonRdpWrite16,
        ) -> fn64_render::NonRdpWrite16Disposition {
            self.inner.observe_non_rdp_write16(write)
        }

        fn process_task(
            &mut self,
            rdram: &mut [u8],
            rsp_memory: &mut fn64_runtime::RspMemory,
            task: &fn64_render::OsTask,
            output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            self.inner
                .process_task(rdram, rsp_memory, task, output_addr)
        }

        fn present(&mut self, request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
            self.inner.present(request)?;
            self.frames
                .lock()
                .unwrap()
                .push(self.inner.presented_framebuffer().unwrap().pixels.clone());
            Ok(())
        }

        fn resize(&mut self, width: u32, height: u32) {
            self.inner.resize(width, height);
        }

        fn supported_ucodes(&self) -> &[UcodeId] {
            self.inner.supported_ucodes()
        }
    }

    fn install_vi_capture_backend() -> Arc<Mutex<Vec<fn64_render::ViPresentation>>> {
        let presentations = Arc::new(Mutex::new(Vec::new()));
        crate::test_support::install_test_present_rdram();
        set_render_backend(
            Box::new(ViCaptureBackend {
                presentations: Arc::clone(&presentations),
            }),
            0,
        );
        presentations
    }

    #[test]
    fn os_vi_scale_shims_latch_mode_relative_fixed_point_registers() {
        crate::test_support::install_complete_render_backend(0);
        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 2);
            view.write_u32(mode.checked_add(32).unwrap(), 0x1003_0400);
            view.write_u32(mode.checked_add(44).unwrap(), 0x2004_0400);
            view.write_u32(mode.checked_add(64).unwrap(), 0x3005_0400);
        }
        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };

        let mut x_ctx = ctx_zeroed();
        x_ctx.f12.halves.0 = 0.75;
        unsafe { osViSetXScale_recomp(std::ptr::null_mut(), &mut x_ctx) };

        let mut y_ctx = ctx_zeroed();
        y_ctx.f12.halves.0 = 0.5;
        unsafe { osViSetYScale_recomp(std::ptr::null_mut(), &mut y_ctx) };

        with_executor(|exec| {
            assert_eq!(exec.vi().next_x_scale, Some(0.75));
            assert_eq!(exec.vi().next_y_scale, Some(0.5));
        });
        assert!(crate::pi::write_live_device_mmio(0xFFFF_FFFF_A440_0018, 0));
        assert!(crate::pi::write_live_device_mmio(0xFFFF_FFFF_A440_000C, 0));
        let base_time = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base_time + 10);
        with_executor(|exec| {
            assert_eq!(exec.vi().x_scale, Some(0.75));
            assert_eq!(exec.vi().y_scale, Some(0.5));
        });
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0030),
            Some(0x1003_0300)
        );
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0034),
            Some(0x2004_0200)
        );
    }

    #[test]
    fn os_vi_set_event_marshals_the_public_retrace_divisor() {
        let queue = RdramAddr::from_offset(0x200);
        with_executor(|exec| exec.create_mesg_queue(queue, 1));
        let mut ctx = ctx_with(0xFFFF_FFFF_8000_0200, 0x32, 2);
        unsafe { osViSetEvent_recomp(std::ptr::null_mut(), &mut ctx) };

        with_executor(|exec| {
            assert_eq!(exec.vi().retrace_target, Some((0x200, 0x32)));
            assert_eq!(exec.vi().retrace_count, 2);
            assert!(!exec.deliver_vi_retrace());
            assert_eq!(
                exec.recv_mesg(99, queue, false),
                fn64_runtime::RecvMesgOutcome::WouldBlock
            );
            assert!(!exec.deliver_vi_retrace());
            assert_eq!(
                exec.recv_mesg(99, queue, false),
                fn64_runtime::RecvMesgOutcome::Delivered(0x32)
            );
        });
    }

    #[test]
    fn os_vi_set_mode_stores_mode_ptr_and_swap_buffer_updates_current_framebuffer() {
        crate::test_support::install_complete_render_backend(0);
        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 0x0000_0002);
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(28).unwrap(), 0x006c_02ec);
            view.write_u32(mode.checked_add(48).unwrap(), 0x0025_01ff);
            view.write_u32(mode.checked_add(40).unwrap(), 0);
            view.write_u32(mode.checked_add(56).unwrap(), 100);
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        with_executor(|exec| assert_eq!(exec.vi().next_mode_ptr, Some(mode.to_kseg0())));
        let mut mode_ctx = ctx_zeroed();
        unsafe { osViGetCurrentMode_recomp(std::ptr::null_mut(), &mut mode_ctx) };
        assert_eq!(mode_ctx.r2, 0);

        let mut swap_ctx = ctx_zeroed();
        swap_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut swap_ctx as *mut _) };
        assert_eq!(current_vi_framebuffer(), None);
        assert_eq!(next_vi_framebuffer(), Some(0x10_0000));
        assert_eq!(vi_swap_count(), 1);
        arm_vi_retrace(10);
        crate::advance_virtual_time(10);
        assert_eq!(current_vi_framebuffer(), Some(0x10_0000));
        with_executor(|exec| assert_eq!(exec.vi().mode_ptr, Some(mode.to_kseg0())));
        unsafe { osViGetCurrentMode_recomp(std::ptr::null_mut(), &mut mode_ctx) };
        assert_eq!(mode_ctx.r2, mode.to_kseg0() as i32 as u64);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert_eq!(snapshot.vi_v_sync, 525);
            assert_eq!(snapshot.vi_intr, 100);
            assert_eq!(
                host.device_fabric
                    .read_mmio(MmioAddr::new(0xA440_0004))
                    .unwrap(),
                0x10_0000
            );
        });
    }

    #[test]
    fn latched_os_vi_mode_replaces_the_nominal_bootstrap_field_interval() {
        crate::test_support::install_complete_render_backend(0);
        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 0x0000_0002);
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(20).unwrap(), 3_093);
            view.write_u32(mode.checked_add(56).unwrap(), 100);
        }

        let bootstrap = crate::configure_tv_type(fn64_runtime::TvType::Ntsc);
        assert_eq!(bootstrap, 1_562_500);
        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };

        crate::advance_virtual_time(bootstrap);
        assert_eq!(
            crate::vi_field_interval(),
            fn64_runtime::TvType::Ntsc.programmed_field_cycles(3_093, 525)
        );
        assert_eq!(crate::configured_tv_type(), fn64_runtime::TvType::Ntsc);
    }

    #[test]
    fn os_vi_mode_alternates_both_public_field_images_and_offsets_framebuffer_origin() {
        crate::test_support::install_complete_render_backend(0);
        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 0x42);
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(28).unwrap(), 0x006c_02ec);
            for (offset, value) in [
                (40, 0x20),
                (44, 0x111),
                (48, 0x0025_01ff),
                (52, 0x133),
                (56, 0),
                (60, 0x80),
                (64, 0x211),
                (68, 0x0026_0200),
                (72, 0x233),
                (76, 0),
            ] {
                view.write_u32(mode.checked_add(offset).unwrap(), value);
            }
        }

        assert!(crate::pi::write_live_device_mmio(0xFFFF_FFFF_A440_0018, 0));
        assert!(crate::pi::write_live_device_mmio(0xFFFF_FFFF_A440_000C, 0));
        let base_time = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);

        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };
        let mut swap_ctx = ctx_zeroed();
        swap_ctx.r4 = 0xFFFF_FFFF_8000_1000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut swap_ctx) };

        crate::advance_virtual_time(base_time + 10);
        assert_eq!(crate::pi::live_vi_field(), 1);
        for (address, expected) in [
            (0xFFFF_FFFF_A440_0004, 0x1080),
            (0xFFFF_FFFF_A440_0034, 0x211),
            (0xFFFF_FFFF_A440_0028, 0x0026_0200),
            (0xFFFF_FFFF_A440_002C, 0x233),
        ] {
            assert_eq!(crate::pi::read_live_device_mmio(address), Some(expected));
        }

        crate::advance_virtual_time(base_time + 20);
        assert_eq!(crate::pi::live_vi_field(), 0);
        for (address, expected) in [
            (0xFFFF_FFFF_A440_0004, 0x1020),
            (0xFFFF_FFFF_A440_0034, 0x111),
            (0xFFFF_FFFF_A440_0028, 0x0025_01ff),
            (0xFFFF_FFFF_A440_002C, 0x133),
        ] {
            assert_eq!(crate::pi::read_live_device_mmio(address), Some(expected));
        }
    }

    #[test]
    fn os_vi_query_shims_report_live_status_line_and_interlaced_field() {
        crate::test_support::install_complete_render_backend(0);
        let base = with_host(|host| host.device_fabric.now().get());
        assert!(crate::pi::write_live_device_mmio(
            0xFFFF_FFFF_A440_0000,
            1 << 6
        ));
        assert!(crate::pi::write_live_device_mmio(
            0xFFFF_FFFF_A440_0018,
            525
        ));
        arm_vi_retrace(1_000);

        let mut ctx = ctx_zeroed();
        unsafe { osViGetStatus_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 1 << 6);
        unsafe { osViGetCurrentField_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 0);
        unsafe { osViGetCurrentLine_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 0);

        crate::advance_virtual_time(base + 1_000);
        unsafe { osViGetCurrentField_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 1);
        unsafe { osViGetCurrentLine_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 1);
    }

    #[test]
    fn os_vi_special_features_mutate_queued_control_and_latch_at_retrace() {
        crate::test_support::install_complete_render_backend(0x100);
        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 0x0000_0002);
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(40).unwrap(), 0x0022_2000);
            view.write_u32(mode.checked_add(56).unwrap(), 100);
        }
        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };

        let mut features_ctx = ctx_zeroed();
        features_ctx.r4 = 0x55;
        unsafe { osViSetSpecialFeatures_recomp(std::ptr::null_mut(), &mut features_ctx) };
        let mut status_ctx = ctx_zeroed();
        unsafe { osViGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx) };
        assert_eq!(status_ctx.r2, 0);

        let base = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base + 10);
        unsafe { osViGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx) };
        assert_eq!(status_ctx.r2, 0x0001_001e);

        features_ctx.r4 = 0xaa;
        unsafe { osViSetSpecialFeatures_recomp(std::ptr::null_mut(), &mut features_ctx) };
        unsafe { osViGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx) };
        assert_eq!(status_ctx.r2, 0x0001_001e);
        crate::advance_virtual_time(base + 20);
        unsafe { osViGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx) };
        assert_eq!(status_ctx.r2, 0x0000_0002);
    }

    #[test]
    fn vi_swap_and_black_transitions_present_typed_vi_state_at_vblank() {
        use fn64_render::{FrameStatus, RenderConfig, RenderError, UcodeId};
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};

        struct CountingBackend {
            presents: Arc<AtomicU32>,
            last_blanked: Arc<AtomicU32>,
            last_fade: Arc<AtomicU32>,
            last_repeat_line: Arc<AtomicU32>,
            last_scanout: Arc<Mutex<Option<fn64_render::ViScanoutState>>>,
        }

        impl RenderBackend for CountingBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                let (vi, _) = request.into_parts();
                self.last_blanked
                    .store(u32::from(vi.blanked), Ordering::SeqCst);
                self.last_fade
                    .store(vi.fade.map_or(u32::MAX, u32::from), Ordering::SeqCst);
                self.last_repeat_line
                    .store(u32::from(vi.repeat_line), Ordering::SeqCst);
                *self.last_scanout.lock().unwrap() = Some(vi.scanout);
                self.presents.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let presents = Arc::new(AtomicU32::new(0));
        let last_blanked = Arc::new(AtomicU32::new(0));
        let last_fade = Arc::new(AtomicU32::new(u32::MAX));
        let last_repeat_line = Arc::new(AtomicU32::new(0));
        let last_scanout = Arc::new(Mutex::new(None));
        crate::test_support::install_test_present_rdram();
        set_render_backend(
            Box::new(CountingBackend {
                presents: Arc::clone(&presents),
                last_blanked: Arc::clone(&last_blanked),
                last_fade: Arc::clone(&last_fade),
                last_repeat_line: Arc::clone(&last_repeat_line),
                last_scanout: Arc::clone(&last_scanout),
            }),
            0,
        );

        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 2);
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(12).unwrap(), 0x03e5_2239);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(20).unwrap(), 3093);
            view.write_u32(mode.checked_add(24).unwrap(), 0x0c15_0c15);
            view.write_u32(mode.checked_add(28).unwrap(), 0x006c_02ec);
            view.write_u32(mode.checked_add(32).unwrap(), 0x0100_0400);
            view.write_u32(mode.checked_add(40).unwrap(), 0x80);
            view.write_u32(mode.checked_add(44).unwrap(), 0x0200_0400);
            view.write_u32(mode.checked_add(48).unwrap(), 0x0025_01ff);
            view.write_u32(mode.checked_add(52).unwrap(), 0x000e_0204);
            view.write_u32(mode.checked_add(56).unwrap(), 100);
            view.write_u32(mode.checked_add(64).unwrap(), 0x0300_0400);
        }
        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };

        assert_eq!(presents.load(Ordering::SeqCst), 0);
        let base = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base + 10);
        assert_eq!(presents.load(Ordering::SeqCst), 1);
        assert_eq!(last_blanked.load(Ordering::SeqCst), 0);
        let scanout = last_scanout.lock().unwrap().unwrap();
        let registers = scanout.registers().unwrap();
        assert_eq!(
            registers.words(),
            [
                2,
                0x0010_0080,
                320,
                100,
                0,
                0x03e5_2239,
                525,
                3093,
                0x0c15_0c15,
                0x006c_02ec,
                0x0025_01ff,
                0x000e_0204,
                0x0100_0400,
                0x0200_0400,
            ]
        );
        let resample = registers.resample();
        assert_eq!(resample.x.step_u2_10(), 0x400);
        assert_eq!(resample.x.offset_u2_10(), 0x100);
        assert_eq!(resample.y.step_u2_10(), 0x400);
        assert_eq!(resample.y.offset_u2_10(), 0x200);
        assert_eq!(resample.field, fn64_render::ViScanoutField::Progressive);
        let active_window = registers.active_window().unwrap();
        assert_eq!(active_window.horizontal_register(), 0x006c_02ec);
        assert_eq!(active_window.vertical_register(), 0x0025_01ff);
        assert_eq!(
            (active_window.output_width(), active_window.output_height()),
            (640, 237)
        );

        ctx.r4 = 1;
        unsafe { osViBlack_recomp(std::ptr::null_mut(), &mut ctx) };
        crate::advance_virtual_time(base + 20);
        assert_eq!(presents.load(Ordering::SeqCst), 2);
        assert_eq!(last_blanked.load(Ordering::SeqCst), 1);

        ctx.r4 = 0;
        unsafe { osViBlack_recomp(std::ptr::null_mut(), &mut ctx) };
        crate::advance_virtual_time(base + 30);
        assert_eq!(presents.load(Ordering::SeqCst), 3);
        assert_eq!(last_blanked.load(Ordering::SeqCst), 0);

        ctx.r4 = 1;
        ctx.r5 = 0x0200;
        unsafe { osViFade_recomp(std::ptr::null_mut(), &mut ctx) };
        crate::advance_virtual_time(base + 40);
        assert_eq!(presents.load(Ordering::SeqCst), 4);
        assert_eq!(last_fade.load(Ordering::SeqCst), 0x0200);

        ctx.r4 = 0;
        ctx.r5 = 0;
        unsafe { osViFade_recomp(std::ptr::null_mut(), &mut ctx) };
        crate::advance_virtual_time(base + 50);
        assert_eq!(presents.load(Ordering::SeqCst), 5);
        assert_eq!(last_fade.load(Ordering::SeqCst), u32::MAX);

        ctx.r4 = 1;
        unsafe { osViRepeatLine_recomp(std::ptr::null_mut(), &mut ctx) };
        crate::advance_virtual_time(base + 60);
        assert_eq!(presents.load(Ordering::SeqCst), 6);
        assert_eq!(last_repeat_line.load(Ordering::SeqCst), 1);
        assert_eq!(last_render_error(), None);
    }

    #[test]
    fn batched_retraces_present_each_interlaced_field_at_its_deadline() {
        let presentations = install_vi_capture_backend();

        let mut rdram = vec![0u8; 0x100];
        let mode = RdramAddr::from_offset(0x20);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            view.write_u32(mode.checked_add(4).unwrap(), 2 | (1 << 6));
            view.write_u32(mode.checked_add(8).unwrap(), 320);
            view.write_u32(mode.checked_add(16).unwrap(), 525);
            view.write_u32(mode.checked_add(28).unwrap(), 0x006c_02ec);
            view.write_u32(mode.checked_add(32).unwrap(), 0x0100_0400);
            view.write_u32(mode.checked_add(40).unwrap(), 0x20);
            view.write_u32(mode.checked_add(44).unwrap(), 0x0200_0400);
            view.write_u32(mode.checked_add(48).unwrap(), 0x0026_0200);
            view.write_u32(mode.checked_add(60).unwrap(), 0x40);
            view.write_u32(mode.checked_add(64).unwrap(), 0x0300_0400);
            view.write_u32(mode.checked_add(68).unwrap(), 0x0025_01ff);
        }
        let mut mode_ctx = ctx_zeroed();
        mode_ctx.r4 = u64::from(mode.to_kseg0());
        unsafe { osViSetMode_recomp(rdram.as_mut_ptr(), &mut mode_ctx) };

        let mut swap_ctx = ctx_zeroed();
        swap_ctx.r4 = 0x8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut swap_ctx) };

        let base = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base + 40);

        let captured = presentations.lock().unwrap();
        assert_eq!(captured.len(), 4);
        assert_eq!(captured[0].noise_seed, base + 10);
        let first = captured[0].scanout.registers().unwrap();
        assert_eq!(first.origin(), 0x0010_0040);
        assert_eq!(first.words()[4], 1);
        assert_eq!(
            first.active_window().unwrap().vertical_register(),
            0x0025_01ff
        );
        assert_eq!(first.y_scale_register(), 0x0300_0400);
        assert_eq!(
            first.resample().field,
            fn64_render::ViScanoutField::InterlacedOdd
        );
        assert_eq!(captured[1].noise_seed, base + 20);
        let second = captured[1].scanout.registers().unwrap();
        assert_eq!(second.origin(), 0x0010_0020);
        assert_eq!(second.words()[4], 0);
        assert_eq!(
            second.active_window().unwrap().vertical_register(),
            0x0026_0200
        );
        assert_eq!(second.y_scale_register(), 0x0200_0400);
        assert_eq!(
            second.resample().field,
            fn64_render::ViScanoutField::InterlacedEven
        );
        assert_eq!(captured[2].noise_seed, base + 30);
        assert_eq!(captured[2].scanout, captured[0].scanout);
        assert_eq!(captured[3].noise_seed, base + 40);
        assert_eq!(captured[3].scanout, captured[1].scanout);
        drop(captured);

        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0004),
            Some(0x0010_0020)
        );
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0010),
            Some(0)
        );
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0028),
            Some(0x0026_0200)
        );
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A440_0034),
            Some(0x0200_0400)
        );
    }

    #[test]
    fn raw_mmio_vi_image_and_seed_cross_each_progressive_retrace_without_an_os_vi_mode() {
        let presentations = install_vi_capture_backend();
        let expected = [
            3,
            0x0012_3456,
            320,
            0,
            0,
            0x03e5_2239,
            525,
            3093,
            0x0c15_0c15,
            0xfc6c_f2ec,
            0xfc25_f1ff,
            0x000e_0204,
            0xf100_f400,
            0xf200_f400,
        ];
        for (index, value) in expected.into_iter().enumerate() {
            if index == 4 {
                continue;
            }
            let address = 0xFFFF_FFFF_A440_0000
                + u64::try_from(index).expect("VI register index exceeds u64") * 4;
            assert!(crate::pi::write_live_device_mmio(address, value));
        }

        let base = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base + 20);

        let captured = presentations.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].noise_seed, base + 10);
        assert_eq!(captured[1].noise_seed, base + 20);
        assert!(!captured[0].blanked);
        let registers = captured[0].scanout.registers().unwrap();
        assert_eq!(registers.words(), expected);
        assert_eq!(captured[1].scanout.registers().unwrap(), registers);
        assert_eq!(registers.active_window().unwrap().output_width(), 640);
        assert_eq!(registers.active_window().unwrap().output_height(), 237);
        assert_eq!(registers.resample().x.offset_u2_10(), 0x100);
        assert_eq!(registers.resample().x.step_u2_10(), 0x400);
        assert_eq!(registers.resample().y.offset_u2_10(), 0x200);
        assert_eq!(registers.resample().y.step_u2_10(), 0x400);
    }

    #[test]
    fn raw_mmio_retrace_rereads_live_origin_bytes_without_a_graphics_task() {
        const ORIGIN: u32 = 0x120;
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, pixel) in [
                0xf801u16, 0x07c1, 0xf83f, 0xffc1, 0x003f, 0xffff, 0x0001, 0x07ff,
            ]
            .into_iter()
            .enumerate()
            {
                view.write_u16(RdramAddr::from_offset(ORIGIN + index as u32 * 2), pixel);
            }
        }

        let frames = Arc::new(Mutex::new(Vec::new()));
        let mut inner = fn64_render_reference::ReferenceBackend::new();
        inner.create(&RenderConfig::ntsc(4, 2)).unwrap();
        set_render_backend(
            Box::new(ReferencePixelBackend {
                inner,
                frames: Arc::clone(&frames),
            }),
            rdram.len(),
        );
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        for (index, value) in [
            0x302,
            ORIGIN,
            0xf000_0004,
            0,
            0,
            0,
            0,
            0,
            0,
            (100 << 16) | 102,
            (20 << 16) | 24,
            0,
            0x400,
            0x400,
        ]
        .into_iter()
        .enumerate()
        {
            if index == 4 {
                continue;
            }
            assert!(crate::pi::write_live_device_mmio(
                0xFFFF_FFFF_A440_0000 + index as u64 * 4,
                value,
            ));
        }

        let base = with_host(|host| host.device_fabric.now().get());
        arm_vi_retrace(10);
        crate::advance_virtual_time(base + 10);
        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_u16(RdramAddr::from_offset(ORIGIN), 0x07ff);
        crate::advance_virtual_time(base + 20);

        let captured = frames.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert_eq!(
            captured[0],
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
        );
        assert_eq!(&captured[1][..4], &[0, 255, 255, 255]);
        drop(captured);
        with_host(|host| *host = HostState::default());
    }
}
