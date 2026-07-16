use super::*;

/// `osSetIntMask(u32 mask) -> u32` (previous mask). Real hardware semantics
/// (CPU interrupt-enable mask) have no host-visible effect on this
/// single-threaded coroutine executor (`docs/DESIGN.md` section 2: there is
/// exactly one host thread, so there is no real concurrent interrupt this
/// mask could race against) -- modeled as a simple stored value with the
/// documented "returns the previous mask" contract, since every real call
/// site's actual behavioral dependency (per rung 9/rung 11's citations) is
/// on the paired critical section it wraps being atomic, which is already
/// guaranteed structurally by the single-executor-thread model, not by this
/// mask's bit pattern.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetIntMask_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let new_mask = ctx.r4 as u32;
    let previous = INT_MASK.with(|cell| cell.replace(new_mask));
    ctx.r2 = previous as u64;
}

thread_local! {
    pub(crate) static INT_MASK: Cell<u32> = const { Cell::new(0) };
}

/// `osInitialize(void)` -- top-level libultra bring-up. Real semantics
/// (thread-0 creation, PI/SP scaffolding) are already covered by this
/// crate's own `osCreateThread_recomp`/`osCreatePiManager_recomp` shims,
/// which the ROM itself calls separately (rung 2: `osInitialize` is the
/// caller of the SI-raw-IO functions during the PIF terminate-boot
/// handshake, not itself the thing that creates the main thread in this
/// corpus's boot sequence -- `recomp_entrypoint` calls `osInitialize_recomp`
/// BEFORE its own `osCreateThread`/`osStartThread` pair, per `funcs_0.c`).
/// This shim's real, tested effect: nothing beyond being a safe, callable
/// no-op -- there is no additional host-state this milestone's evidence
/// shows `osInitialize` itself needs to establish beyond what the
/// executor's `Default` already does at construction (empty run queue, no
/// threads, per `Executor::new`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osInitialize_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

// ---------------------------------------------------------------------
// Batch-generated trivial shims: thin wrappers over machinery this crate
// already has (executor scheduling, cache-op no-ops, thread-handle
// resolution), scoped to the shims `aki-recomp`'s OOTU generated corpus
// currently has REAL call sites for (per this wave's `grep -rl
// "<sym>_recomp("` sweep over `games/*/RecompiledFuncs/funcs_*.c`) --
// matrix-guided per `docs/COMPLETENESS.md`'s "don't build surface no game
// calls" rule. Each shim's doc comment below cites its real call site.
// ---------------------------------------------------------------------

/// `osGetMemSize(void) -> u32` -- no arguments (public libultra manual);
/// returns the total RDRAM size in bytes. Real call site:
/// `games/OOTU/RecompiledFuncs/funcs_0.c:142`. This crate's `Rdram` is a
/// fixed-size buffer (`fn64_runtime::rdram::DEFAULT_RDRAM_SIZE`, 8 MB) --
/// returning that constant is the real, correct answer (not a fabricated
/// value), since every target game runs on the same 8 MB console
/// configuration (`rdram.rs`'s own doc comment).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetMemSize_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u64;
}

/// `__osDisableInt(void) -> u32` -- real hardware effect: disables CPU
/// interrupts, returning the previous interrupt-enable state (an `SR`
/// register snapshot) so a matching `__osRestoreInt` can restore it. This
/// crate has no interrupt model (`docs/DESIGN.md`'s single-executor,
/// single-host-thread design means there is no concurrent interrupt
/// delivery to race against -- see `executor.rs`'s own doc comment on why
/// that hazard class doesn't exist here) -- returns a fixed "was enabled"
/// sentinel (`1`, matching `osSetIntMask_recomp`'s existing convention of
/// returning the previous mask value) since no evidence shows any call
/// site branching on the exact previous value beyond feeding it back to
/// `__osRestoreInt`. Real call sites: `games/OOTU/RecompiledFuncs/funcs_0.c`
/// (x2).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osDisableInt_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = 1;
}

/// `__osRestoreInt(u32 mask)` -- restores the interrupt-enable state a
/// prior `__osDisableInt` returned. No-op counterpart to
/// `__osDisableInt_recomp` (see that shim's doc comment for why this crate
/// has nothing real to restore). Real call sites:
/// `games/OOTU/RecompiledFuncs/funcs_0.c` (x2).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osRestoreInt_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osGetTime(void) -> OSTime` -- no arguments; returns the current system
/// time counter (`u64`). This crate has no wall-clock (only the executor's
/// virtual `sim_time`, per `docs/DESIGN.md`'s "no wall-clock in core" rule
/// -- see `Executor::sim_time`'s doc comment), which is the real,
/// reproducible value to return here: a differential trace comparing two
/// runs needs `osGetTime` to track the SAME virtual clock every other
/// timing decision in this crate already uses, not an independent
/// wall-clock reading. Real call sites: `games/OOTU/RecompiledFuncs/funcs_0.c`
/// (x2), `funcs_24.c:763`, `funcs_56.c:657`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetTime_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    // OSTime is u64 (time.h:6); an o32 64-bit return splits $v0:$v1 =
    // HIGH:LOW word, matching this crate's own convention (__ll_div_recomp
    // etc.: `r2 = result>>32; r3 = result & 0xFFFFFFFF`). Callers reconstruct
    // the u64 from both words (funcs_56.c ~1152 `sw $v0,0x20; sw $v1,0x24`;
    // funcs_24.c ~5923) -- writing only r2 left r3 stale and corrupted both
    // halves of the reconstructed timestamp.
    let t = with_executor(|exec| exec.sim_time());
    ctx.r2 = t >> 32;
    ctx.r3 = t & 0xFFFF_FFFF;
}

/// `osGetCount(void) -> u32` -- no arguments; real hardware `Count` COP0
/// register read (a free-running cycle counter). This crate has no COP0
/// register model and no evidence (function-table slot only,
/// `recomp_overlays.inl:82`, zero real call sites in this corpus) any boot-
/// path code branches on its exact value beyond timing/profiling use --
/// backed by the SAME virtual clock `osGetTime_recomp` already exposes
/// (`Executor::sim_time`), matching that shim's "differential-trace-
/// reproducible" reasoning rather than a wall-clock or a fabricated cycle
/// count.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetCount_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = with_executor(|exec| exec.sim_time()) as u32 as u64;
}

/// `__osSetFpcCsr(u32 value) -> u32` -- sets/reads the MIPS FPU control/
/// status register. Zero real call sites in this corpus (function-table
/// slot only, `recomp_overlays.inl:88`) -- this crate's generated-code
/// execution model has no FPU-exception-mode host state at all (every FP
/// op RecompiledFuncs emits is plain host-native float arithmetic, per
/// `RecompContext`'s `Fpr` union doc comment; there is no CSR whose bits
/// this crate's arithmetic actually consults). Loud-trapped rather than
/// returning a fabricated "no exceptions enabled" CSR value with no call
/// site to verify it against.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSetFpcCsr_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__osSetFpcCsr_recomp: no FPU-CSR host state exists in this crate's execution model \
         (RecompContext's Fpr union is plain host-native float arithmetic, no exception-mode \
         bits consulted) and no real call site in games/OOTU/RecompiledFuncs exercises this."
    );
}

/// `osSetTime(OSTime time)` -- sets the virtual system-time base. `time` is
/// a 64-bit value split r4:r5 hi:lo (standard o32 convention, same shape as
/// `__ll_div_recomp`'s arguments -- NOT independently confirmed for this
/// symbol though, since this corpus has zero real call sites,
/// function-table slot only per `recomp_overlays.inl:2955`). Loud-trapped:
/// `Executor::sim_time` has a getter (`osGetTime_recomp`) but no public
/// setter today, and BOOT-PLAN.md flags this specific symbol as
/// "re-verify against source if link errors persist" -- exactly the
/// "prefer not verified over a false done" case.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetTime_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osSetTime_recomp: no real call site in games/OOTU/RecompiledFuncs exercises this \
         (function-table slot only) and Executor::sim_time has no public setter yet -- \
         BOOT-PLAN.md itself flags this symbol as unconfirmed, re-verify against source before \
         implementing rather than guessing the register shape."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn os_get_mem_size_reports_the_real_rdram_size() {
        let mut ctx = ctx_zeroed();
        unsafe { osGetMemSize_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u64);
    }

    #[test]
    fn disable_restore_int_are_safe_and_disable_returns_nonzero() {
        let mut ctx = ctx_zeroed();
        unsafe { __osDisableInt_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_ne!(ctx.r2, 0, "a previous-enabled-state sentinel, not zero");
        unsafe { __osRestoreInt_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _) };
    }

    #[test]
    fn os_get_time_tracks_the_executors_virtual_clock() {
        // OSTime is reconstructed from $v0:$v1 = HIGH:LOW word (o32 64-bit
        // return); see os_get_time_splits_u64_high_low_across_v0_v1.
        let ostime = |ctx: &RecompContext| (ctx.r2 << 32) | (ctx.r3 & 0xFFFF_FFFF);
        let mut ctx = ctx_zeroed();
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        let t0 = ostime(&ctx);
        with_executor(|exec| exec.advance_time(exec.sim_time() + 500));
        let mut ctx2 = ctx_zeroed();
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut ctx2 as *mut _) };
        assert!(
            ostime(&ctx2) >= t0 + 500,
            "osGetTime must track sim_time advancing, not a fixed value"
        );
    }

    #[test]
    fn os_set_int_mask_returns_previous_mask() {
        let mut ctx1 = ctx_zeroed();
        ctx1.r4 = 1;
        unsafe { osSetIntMask_recomp(std::ptr::null_mut(), &mut ctx1 as *mut _) };
        assert_eq!(ctx1.r2, 0); // previous was 0

        let mut ctx2 = ctx_zeroed();
        ctx2.r4 = 2;
        unsafe { osSetIntMask_recomp(std::ptr::null_mut(), &mut ctx2 as *mut _) };
        assert_eq!(ctx2.r2, 1); // previous was 1
    }

    #[test]
    fn os_initialize_is_a_safe_callable_noop() {
        unsafe { osInitialize_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _) };
    }

    /// osGetTime returns a u64 OSTime split $v0:$v1 = HIGH:LOW word (this
    /// crate's 64-bit-return convention, __ll_div_recomp). A caller stores
    /// r2 then r3 as two consecutive words to reconstruct the u64. Fails
    /// against the bug (r2 = full u64, r3 never written): r2 would truncate
    /// to the LOW word and r3 stays stale.
    #[test]
    fn os_get_time_splits_u64_high_low_across_v0_v1() {
        // A time whose high and low words are BOTH nonzero and distinct, so a
        // dropped/swapped half is caught (not masked by a zero word).
        let t: u64 = 0x1122_3344_5566_7788;
        with_executor(|exec| exec.advance_time(t));

        let mut ctx = ctx_zeroed();
        ctx.r3 = 0xDEAD_BEEF; // stale $v1: the bug leaves this untouched.
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 0x1122_3344, "$v0 = HIGH word");
        assert_eq!(ctx.r3, 0x5566_7788, "$v1 = LOW word");
    }
}
