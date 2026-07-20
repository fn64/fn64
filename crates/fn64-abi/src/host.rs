use super::*;
use crate::task_dispatch::{EXECUTOR_CALLS, EXECUTOR_NS, PHASE_TIMING};

// ---------------------------------------------------------------------
// Host-facing (non-`_recomp`) helpers.
// ---------------------------------------------------------------------

/// Host-side entry point for injecting an external (SI/PI/VI-style)
/// completion into the executor -- not a `_recomp` shim.
pub fn inject_external_event(event: ExternalEvent) {
    with_executor(|exec| exec.inject_event(event));
}

/// Host-side virtual-clock driver.
///
/// Also samples the VI retrace cadence probe (ROADMAP R5 probe 3): this is the
/// only place the host advances the guest's virtual clock, so it is the one
/// seam where wall-clock and fired-tick counts can be correlated. The delta is
/// read back from the executor rather than predicted, so the probe counts what
/// actually fired, not what the caller expected to fire.
pub fn advance_virtual_time(now: u64) {
    crate::pi::advance_device_time(now);
    with_executor(|exec| exec.advance_time(now));
}

/// Create and start thread 0 running `recomp_entrypoint` -- the harness's
/// boot entry point. `recomp_entrypoint`'s own body (verified directly
/// against `RecompiledFuncs/funcs_0.c`) computes its own jump target from
/// literal immediates with no dependency on incoming register state, so an
/// all-zero `RecompContext` is the correct, real starting state (matching
/// what a fresh `OSThread`'s initial register file honestly is before any
/// game code has run), not a placeholder.
///
/// # Safety
/// `rdram` must be a valid pointer to the process's one shared rdram
/// buffer, live for at least as long as any coroutine spawned here might
/// run (the whole process, per `docs/DESIGN.md` section 3). `entry` must be
/// `recomp_entrypoint` (or a real `recomp_func_t`-shaped function with the
/// same contract) -- the boot harness passes the real generated symbol.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        // Register the process-wide rdram base so the executor can mirror
        // each OSMesgQueue's validCount/first/msgCount back into the guest's
        // real struct after every queue mutation -- guest code reads those
        // fields directly (MQ_IS_FULL/MQ_GET_COUNT). SAFETY: `rdram` is the
        // harness's single live rdram buffer, valid for the whole run, the
        // same buffer every shim already receives.
        unsafe { exec.set_rdram_base(rdram) };
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                let mut ctx = RecompContext::zeroed();
                ctx.arm_fpr_alias();
                unsafe { entry(rdram_ptr, &mut ctx as *mut _) };
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Run one scheduling step (see `Executor::run_one_step`'s doc comment).
/// Returns `false` when nothing was runnable -- the harness should then
/// call `advance_virtual_time` to make host-driven progress (VI retrace,
/// due timers) before trying again.
///
/// This is THE seam that re-arms the coroutine-context thread-locals (see
/// `THREAD_CONTEXTS`' doc comment) to the thread ABOUT to be resumed --
/// every caller in this crate (including this crate's own tests) must
/// dispatch a scheduling step through this function (or `run_to_idle`,
/// below), never a bare `exec.run_one_step()`/`exec.run_to_idle()` inside a
/// `with_executor` closure, or the re-arm is skipped and the bug this wave
/// fixed reappears.
pub fn run_one_step() -> bool {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    let (stepped, now) = with_executor(|exec| {
        // `peek_next_thread` is a read-only preview of exactly which thread
        // `exec.run_one_step()` is about to resume -- read it BEFORE the
        // resume so the correct thread's context is active for the entire
        // duration of that resume (including every `_recomp` shim call the
        // thread's body makes after waking, up to and including its next
        // suspend). `None` means nothing is runnable -- no resume will
        // happen, so no context needs arming.
        let stepped = match exec.peek_next_thread() {
            Some(id) => with_rearmed_context(id, || exec.run_one_step()),
            None => exec.run_one_step(),
        };
        (stepped, exec.sim_time())
    });
    if stepped {
        // `run_one_step` returns only after a yielding coroutine is fully
        // suspended. Commit the ABI-owned device fabric at that exact guest
        // time before any later scheduling step can resume a guest. This
        // closes the interleaving checkpoint-yield -> same-thread resume ->
        // overdue PI completion, which would otherwise execute one extra
        // translated block before bytes/MI/queue state became observable.
        crate::pi::advance_device_time(now);
    }
    if let Some(started) = started {
        EXECUTOR_NS.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            );
        });
        EXECUTOR_CALLS.with(|calls| calls.set(calls.get() + 1));
    }
    stepped
}

/// Priority the next [`run_one_step`] will dispatch, or `None` when the run
/// queue is empty. The window driver uses `OS_PRIORITY_IDLE` as the explicit
/// quiescence boundary documented by libultra rather than guessing from a
/// wall-time or step-count threshold.
pub fn next_runnable_priority() -> Option<Priority> {
    with_executor(|exec| exec.peek_next_priority())
}

/// Run until the run queue is idle (every thread finished or blocked). See
/// `run_one_step`'s doc comment -- this loops it rather than calling
/// `Executor::run_to_idle` directly, so every individual resume inside the
/// loop gets its own correctly-armed context (a single re-arm before the
/// whole loop would be exactly as wrong as the original bug once a second
/// thread's turn came up).
pub fn run_to_idle() {
    while run_one_step() {}
}

/// Whether thread `id` has finished (its coroutine returned or was never
/// created) -- the harness's "has boot's thread 0 died" check.
pub fn is_thread_dead(id: ThreadId) -> bool {
    with_executor(|exec| exec.is_thread_dead(id))
}

/// Gfx/audio task submission counts observed so far (`Executor::task_log`).
pub fn task_counts() -> (u64, u64) {
    with_executor(|exec| (exec.task_log().gfx_count(), exec.task_log().audio_count()))
}

/// Copy the full recorded trace out as an owned `Vec` -- the harness's
/// entry point for emitting `docs/DESIGN.md` section 4's shared
/// `TraceEvent` stream to a file.
pub fn copy_trace() -> Vec<fn64_runtime::TraceEvent> {
    with_executor(|exec| exec.trace().to_vec())
}

/// Enable or disable differential event capture. This controls diagnostics
/// only; emulated scheduling and peripheral behavior are unchanged.
pub fn set_trace_enabled(enabled: bool) {
    with_executor(|exec| exec.set_trace_enabled(enabled));
}

/// Arm incremental crash-safe trace flushing -- every trace event recorded
/// from this call onward is appended+flushed to `path` immediately, not
/// just buffered in memory for `copy_trace`'s end-of-run snapshot. Call
/// this BEFORE booting thread 0, so a SIGSEGV/abort mid-boot still leaves
/// every event up to the crash on disk. See
/// `fn64_runtime::TraceLog::set_sink_file`'s doc comment for the incident
/// (WM2000 rung-3 frontier) this fixes.
pub fn set_trace_sink_file(path: &str) -> std::io::Result<()> {
    with_executor(|exec| exec.set_trace_sink_file(path))
}

/// The executor's current virtual-clock reading.
pub fn sim_time() -> u64 {
    with_executor(|exec| exec.sim_time())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    /// Regression: `arm_fpr_alias` must make an odd-register `mtc1` (which
    /// generated C emits as `ctx->f_odd[(N-1)*2] = value`) land in the HIGH
    /// 32-bit word of register N's even partner (FR=0 lane), not fault
    /// through a null `f_odd`. This was the OoT-boot SIGSEGV-at-0x40 in
    /// `guLookAtHiliteF` (funcs_57.c:4519, `mtc1 $at, $f9`): `f_odd` was
    /// left null by `zeroed()`, so `f_odd[16]` dereferenced 0x40.
    ///
    /// Verified to fail against the bug: comment out the `arm_fpr_alias()`
    /// call below and the test segfaults on the null-pointer write (exactly
    /// the boot fault) instead of asserting.
    #[test]
    fn arm_fpr_alias_routes_odd_mtc1_to_even_partner_high_word() {
        // The context must live at a stable address for the whole test --
        // `arm_fpr_alias` stores a self-referential pointer. Box it so the
        // pointer stays valid across the field reads below.
        let mut ctx = Box::new(ctx_zeroed());
        ctx.arm_fpr_alias();

        // Distinguishable per-register sentinels so an off-by-one landing in
        // the wrong register (or the wrong 32-bit lane) is caught, not just a
        // "didn't crash" pass.
        // Generated C for `mtc1 <val>, $fN` (N odd): ctx->f_odd[(N-1)*2] = val
        let cases: &[(usize, u32)] = &[(9, 0xDEAD_0009), (7, 0xBEEF_0007), (17, 0xCAFE_0017)];
        for &(n, val) in cases {
            // Safety: f_odd was just armed to point into this context's own
            // fpr file; the index math mirrors the generated C exactly.
            unsafe {
                *ctx.f_odd.add((n - 1) * 2) = val;
            }
        }

        // $f9's bits alias f8's high word (byte 0x44 == &f0.u32h + 0x40).
        assert_eq!(unsafe { ctx.f8.u32_halves.1 }, 0xDEAD_0009, "f9 -> f8.u32h");
        // $f7 -> f6.u32h; $f17 -> f16.u32h.
        assert_eq!(unsafe { ctx.f6.u32_halves.1 }, 0xBEEF_0007, "f7 -> f6.u32h");
        assert_eq!(
            unsafe { ctx.f16.u32_halves.1 },
            0xCAFE_0017,
            "f17 -> f16.u32h"
        );

        // The LOW words of those even partners (their own $fN even-register
        // value) must be untouched -- proves the write hit the odd lane, not
        // the even register.
        assert_eq!(unsafe { ctx.f8.u32_halves.0 }, 0, "f8 low word untouched");
        assert_eq!(unsafe { ctx.f6.u32_halves.0 }, 0, "f6 low word untouched");
        assert_eq!(unsafe { ctx.f16.u32_halves.0 }, 0, "f16 low word untouched");
    }

    #[test]
    fn retrace_progress_raises_the_shared_mi_vi_source() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::pi::set_mi_interrupt_mask(fn64_runtime::InterruptSource::Vi.bit());
        crate::vi::arm_vi_retrace(10);

        advance_virtual_time(10);
        let pending = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(pending & fn64_runtime::InterruptSource::Vi.bit(), 0);
        assert!(crate::pi::cpu_interrupt_pending());
    }

    #[test]
    fn vi_retrace_latches_state_and_mi_before_both_message_paths() {
        with_executor(|executor| *executor = fn64_runtime::Executor::new());
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::test_support::install_complete_render_backend(0);
        let os_queue = RdramAddr::from_offset(0x100);
        let manager_queue = RdramAddr::from_offset(0x200);
        let framebuffer = RdramAddr::from_offset(0x30_0000);
        with_executor(|executor| {
            executor.create_mesg_queue(os_queue, 1);
            executor.create_mesg_queue(manager_queue, 1);
            executor.set_event_mesg(fn64_runtime::executor::OS_EVENT_VI, os_queue, 0x31);
            executor.vi_set_event(manager_queue, 0x32, 1);
            executor.vi_swap_buffer(framebuffer);
        });
        crate::vi::arm_vi_retrace(10);

        advance_virtual_time(9);
        assert!(!with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Vi)));
        with_executor(|executor| {
            assert_eq!(executor.vi().current_framebuffer, None);
            assert_eq!(
                executor.recv_mesg(99, os_queue, false),
                fn64_runtime::RecvMesgOutcome::WouldBlock
            );
            assert_eq!(
                executor.recv_mesg(99, manager_queue, false),
                fn64_runtime::RecvMesgOutcome::WouldBlock
            );
        });

        advance_virtual_time(10);
        assert!(with_host(|host| host
            .device_fabric
            .interrupt_pending(fn64_runtime::InterruptSource::Vi)));
        with_executor(|executor| {
            assert_eq!(executor.vi().current_framebuffer, Some(framebuffer));
            assert_eq!(
                executor.recv_mesg(99, os_queue, false),
                fn64_runtime::RecvMesgOutcome::Delivered(0x31)
            );
            assert_eq!(
                executor.recv_mesg(99, manager_queue, false),
                fn64_runtime::RecvMesgOutcome::Delivered(0x32)
            );
        });
    }
}
