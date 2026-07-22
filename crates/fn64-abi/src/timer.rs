use super::*;

/// `osSetTimer(OSTimer *t, OSTime countdown, OSTime interval, OSMesgQueue
/// *mq, OSMesg msg)` -- M1-WORKLIST.md #23, T2, "verify empirically before
/// trusting without a donor cite" per the ladder. `t`=r4 (unused, same
/// OSId-style non-issue as thread shims -- this crate has no per-`OSTimer`
/// struct state, `TimerWheel::set_timer` returns its own `TimerId`, and no
/// shim here yet needs to map a `t` address back to that id, since nothing
/// calls `osStopTimer_recomp` in this milestone's undefined-symbol set).
/// `countdown` is a 64-bit `OSTime`, passed as the o32 register pair a2:a3
/// (`ctx.r6`=HIGH word, `ctx.r7`=LOW word -- MIPS big-endian even/odd pair
/// alignment for a 64-bit 2nd argument). Reassembled as `(r6 << 32) | r7`.
/// Reading only `r6` (the OLD code) got the HIGH word, which is 0 for every
/// value under 2^32 -- so OoT's own RCP-timeout timer
/// (`Graph_ExecuteAndDraw`: `osSetTimer(&timer, OS_USEC_TO_CYCLES(3000000),
/// 0, &gfxCtx->queue, 666)`) armed with countdown=0 and fired IMMEDIATELY
/// every `advance_time` tick instead of after 3 virtual seconds. Verified
/// byte-exact against the real call site
/// (`games/OOTU/RecompiledFuncs/funcs_40.c`, PC 0x800A13C0-0x800A13F4):
/// `lui $a3,0x861; ori $a3,$a3,0xC468` => a3=0x0861C468 (LOW word, the
/// 140,378,216-cycle 3s countdown), `addiu $a2,$zero,0x0` => a2=0 (HIGH).
/// `interval`/`mq`/`msg` stack-passed: interval is likewise a 64-bit pair at
/// `sp+0x10`(HIGH):`sp+0x14`(LOW), `mq` at `sp+0x18`, `msg` at `sp+0x1C`
/// (same funcs_40.c call site: `sw $t6,0x10; sw $t7,0x14; sw $a1,0x18;
/// sw $t5,0x1C`, both interval halves 0 here).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetTimer_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let timer_handle = RdramAddr::from_gpr(ctx.r4);
    let countdown = ((ctx.r6 & 0xFFFF_FFFF) << 32) | (ctx.r7 & 0xFFFF_FFFF);
    let interval_hi = read_stack_word(rdram, ctx.r29, 0x10) as u64;
    let interval_lo = read_stack_word(rdram, ctx.r29, 0x14) as u64;
    let interval = (interval_hi << 32) | interval_lo;
    let mq_addr = RdramAddr::from_gpr(read_stack_word(rdram, ctx.r29, 0x18) as u64);
    let msg = read_stack_word(rdram, ctx.r29, 0x1C) as Mesg;
    let armed_by = current_thread_id("osSetTimer_recomp");
    if crate::boot_probe_enabled() {
        eprintln!(
            "[boot-probe] osSetTimer(countdown={countdown:#x}, interval={interval:#x}, mq={:#010x}, msg={msg:#x}) by thread {armed_by:#x}",
            mq_addr.offset() + 0x8000_0000
        );
    }
    let id = with_executor(|exec| exec.set_timer(countdown, interval, mq_addr, msg, armed_by));
    // Recorded so a later real osStopTimer(t) call (same OSTimer* handle,
    // per libultra's documented API -- OoT's boot-critical set per
    // BOOT-PLAN.md's rung-13 note) can look up the TimerWheel-internal id.
    // See `timer_handles`' doc comment for why this mirrors
    // `osCreateThread_recomp`'s `thread_handles` pattern exactly.
    with_host(|host| {
        host.timer_handles.insert(timer_handle.offset(), id);
    });
}

/// `osStopTimer(OSTimer *t) -> s32` -- `a0`=`ctx->r4`, resolved via
/// `HostState::timer_handles` (same `OSTimer*`-handle-lookup convention as
/// `resolve_thread_arg`/`thread_handles`, see that field's doc comment).
/// Function-table slot only (`recomp_overlays.inl:2930`), reached from
/// IrqMgr's PRENMI timer-cancellation path per BOOT-PLAN.md. A handle
/// never registered by `osSetTimer_recomp` is a loud, named panic rather
/// than a silent no-op, matching `resolve_thread_arg`'s existing "never
/// silently guess" precedent for the identical class of lookup-miss.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osStopTimer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let handle = RdramAddr::from_gpr(ctx.r4).offset();
    let id = with_host(|host| {
        host.timer_handles.get(&handle).copied().unwrap_or_else(|| {
            panic!(
                "osStopTimer_recomp: OSTimer* handle {handle:#010x} was never registered by \
                 osSetTimer_recomp -- either this handle is garbage, or a timer was armed \
                 through a path this crate doesn't yet model."
            )
        })
    });
    with_executor(|exec| exec.stop_timer(id));
    ctx.r2 = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use fn64_runtime::RecvMesgOutcome;

    /// Regression: `osSetTimer`'s `OSTime countdown` is a 64-bit value passed
    /// in the o32 register pair a2:a3 (`ctx.r6`=HIGH word, `ctx.r7`=LOW word).
    /// The old shim read only `ctx.r6` (the HIGH word), which is 0 for any
    /// countdown under 2^32 -- so OoT's own 3-second RCP-timeout timer
    /// (`Graph_ExecuteAndDraw`, funcs_40.c PC 0x800A13C0: `lui $a3,0x861; ori
    /// $a3,$a3,0xC468` => a3=0x0861C468, `addiu $a2,$zero,0x0` => a2=0) armed
    /// with countdown=0 and fired IMMEDIATELY on the very next
    /// `advance_virtual_time` tick, spinning the Graph thread instead of
    /// pacing it 3 virtual seconds out.
    ///
    /// Distinguishable values: countdown 0x0861_C468 (the real 3s value, high
    /// word 0), armed at t=0; a probe at t=1000 (well before it) must NOT
    /// deliver, and a probe at t=0x0861_C468 must. The buggy high-word-only
    /// read would deliver at t=1000 (countdown seen as 0), failing the first
    /// assert.
    #[test]
    fn os_set_timer_assembles_64bit_countdown_from_a2_a3_register_pair() {
        with_executor(|exec| *exec = fn64_runtime::Executor::new());

        // Real rdram so the stack-passed interval/mq/msg reads resolve. Leaked
        // to a 'static pointer so it stays valid across the spawned coroutine
        // that arms the timer (the arm must run inside a resumed thread body
        // so `current_thread_id` has an active id, matching real dispatch).
        let rdram: &'static mut [u8] =
            vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize].leak();
        let rdram_ptr = rdram.as_mut_ptr();

        // The queue the timer posts to, and the message it posts.
        let mq_vram: u64 = 0xFFFF_FFFF_8006_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        const TIMER_MSG: u32 = 0x1234_5678;
        with_executor(|exec| exec.create_mesg_queue(mq_addr, 4));

        // arg layout: a0=OSTimer* (r4), countdown 64-bit in a2:a3 (r6:r7),
        // interval 64-bit at sp+0x10:0x14, mq at sp+0x18, msg at sp+0x1C.
        const COUNTDOWN: u32 = 0x0861_C468; // OS_USEC_TO_CYCLES(3000000), high word 0
        let sp: u64 = 0xFFFF_FFFF_8000_4000;
        {
            let mut put = |off: u32, v: u32| {
                let o = RdramAddr::from_gpr(sp.wrapping_add(off as u64)).offset() as usize;
                rdram[o..o + 4].copy_from_slice(&v.to_ne_bytes());
            };
            put(0x10, 0); // interval HIGH = 0
            put(0x14, 0); // interval LOW  = 0 (one-shot)
            put(0x18, mq_vram as u32); // mq
            put(0x1C, TIMER_MSG); // msg
        }

        // Arm the timer from inside a resumed thread body (installs an active
        // thread id, as every real _recomp call has).
        spawn_test_thread(200, 1, move || {
            let mut ctx = ctx_zeroed();
            ctx.r4 = 0xFFFF_FFFF_8006_1000; // OSTimer* handle
            ctx.r6 = 0; // a2 = countdown HIGH word (zero for a <2^32 value)
            ctx.r7 = COUNTDOWN as u64; // a3 = countdown LOW word
            ctx.r29 = sp;
            unsafe { osSetTimer_recomp(rdram_ptr, &mut ctx as *mut _) };
        });
        run_to_idle();

        // Probe WELL before the real deadline: nothing may be delivered yet.
        // (The bug delivered here, having read countdown as 0.)
        advance_virtual_time(1000);
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(0, mq_addr, false),
                RecvMesgOutcome::WouldBlock,
                "a 0x0861C468-cycle countdown must NOT fire at t=1000 -- if it did, the shim \
                 read only the (zero) HIGH word of the 64-bit countdown"
            );
        });

        // Advance to the real deadline: now it must deliver exactly once.
        advance_virtual_time(COUNTDOWN as u64);
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(0, mq_addr, false),
                RecvMesgOutcome::Delivered(TIMER_MSG),
                "the timer must fire at its true 64-bit deadline"
            );
        });
    }
}
