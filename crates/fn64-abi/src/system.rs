use super::*;

/// `osSetIntMask(u32 mask) -> u32` (previous mask). The public libultra manual
/// defines one combined `OSIntMask`: Status.IE/IM in bits 0/8..15 and the six
/// RCP MI masks in bits 16..21. The CPU fields are per-context while MI is the
/// shared device gate, so both authorities must change together.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetIntMask_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let new_mask = ctx.r4 as u32;
    let previous = INT_MASK.with(|cell| cell.replace(new_mask));
    const CPU_INTERRUPT_FIELDS: u32 = 1 | (0xFF << 8);
    ctx.status_reg = (ctx.status_reg & !CPU_INTERRUPT_FIELDS) | (new_mask & CPU_INTERRUPT_FIELDS);
    crate::pi::set_mi_interrupt_mask((new_mask >> 16) & 0x3F);
    ctx.r2 = previous as u64;
}

thread_local! {
    pub(crate) static INT_MASK: Cell<u32> = const { Cell::new(0) };
    static FPC_CSR: Cell<u32> = const { Cell::new(0) };
}

// Public libultra manuals name these as FPCSR_FS (flush denormals to zero)
// and FPCSR_EV (enable invalid-operation exceptions). The values are the
// public R4300 FCSR bit layout, not behavior recovered from a GPL runtime.
const FPCSR_FS: u32 = 0x0100_0000;
const FPCSR_EV: u32 = 0x0000_0800;

pub(crate) fn initialize_common() {
    FPC_CSR.with(|csr| csr.set(FPCSR_FS | FPCSR_EV));
}

/// `__osInitialize_common(void)` -- initialization shared by retail and the
/// MSP/KMC/ISV development-hardware entry points. The public `osInitialize`
/// and internal-routine manuals require FPCSR_FS and FPCSR_EV at startup.
/// Exception-vector installation and debug-port TLB mapping remain executor
/// responsibilities: recompiled functions are the smallest resumable unit,
/// so fn64 never executes or fetches from the original exception vectors.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osInitialize_common_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    initialize_common();
}

/// `osInitialize(void)` -- top-level libultra bring-up. The shared observable
/// CPU state is initialized here; PI/SP managers and application threads are
/// still created by their dedicated public calls.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osInitialize_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    initialize_common();
}

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

/// `__osDisableInt(void) -> u32` -- clear this context's Status.IE and return
/// its previous value for `__osRestoreInt`. Real call sites:
/// `games/OOTU/RecompiledFuncs/funcs_0.c` (x2).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osDisableInt_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let previous = ctx.status_reg & 1;
    ctx.status_reg &= !1;
    ctx.r2 = previous as u64;
}

/// `__osRestoreInt(u32 mask)` -- restore Status.IE from the value returned by
/// `__osDisableInt_recomp`. Real call sites:
/// `games/OOTU/RecompiledFuncs/funcs_0.c` (x2).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osRestoreInt_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.status_reg = (ctx.status_reg & !1) | (ctx.r4 as u32 & 1);
}

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
/// register read (a free-running cycle counter). `Executor` keeps it distinct
/// from the OS time base: deterministic virtual-time advances increment both,
/// while `osSetTime` changes only OSTime and `osSetCount` changes only Count.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetCount_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    ctx.r2 = with_executor(|exec| exec.cp0_count()) as u64;
    if crate::boot_probe_enabled() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            eprintln!("[boot-probe] osGetCount -> {:#x}", ctx.r2);
        }
    }
}

/// `osSetCount(u32 count)` -- writes the MIPS CP0 Count register without
/// changing the OS time base.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetCount_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let count = unsafe { &*ctx }.r4 as u32;
    with_executor(|exec| exec.set_cp0_count(count));
}

/// `__osSetFpcCsr(u32 value) -> u32` -- the public internal-routine manual
/// specifies that it returns the previous MIPS FPU control/status register
/// before installing the new value. The register is CPU-global state, so one
/// executor-thread-local cell backs every guest thread. Generated host FP
/// operations do not yet apply its rounding/exception bits; that behavioral
/// gap remains explicit even though register reads/writes are now correct.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSetFpcCsr_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let previous = FPC_CSR.with(|csr| csr.replace(ctx.r4 as u32));
    ctx.r2 = previous as u64;
}

/// `osSetTime(OSTime time)` -- sets the virtual system-time base. `time` is
/// a 64-bit value split r4:r5 hi:lo (standard o32 convention, same shape as
/// `__ll_div_recomp`'s arguments). The public libultra timer contract says
/// this sets the system time counter, so it updates the same deterministic
/// executor clock `osGetTime_recomp` reads. This keeps timer behavior inside
/// the runtime's no-wall-clock model.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetTime_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let time = (ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64);
    with_executor(|exec| exec.set_sim_time(time));
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
    fn disable_restore_int_round_trips_an_enabled_state() {
        let mut ctx = ctx_zeroed();
        ctx.status_reg = 1;
        unsafe { __osDisableInt_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 1);
        ctx.r4 = ctx.r2;
        unsafe { __osRestoreInt_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.status_reg & 1, 1);
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
        INT_MASK.with(|mask| mask.set(0));
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
    fn os_set_int_mask_updates_context_status_and_the_mi_gate() {
        INT_MASK.with(|mask| mask.set(0));
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut ctx = ctx_zeroed();
        ctx.status_reg = 0x3400_0002;
        ctx.r4 = 0x0010_0401; // OS_IM_PI
        unsafe { osSetIntMask_recomp(std::ptr::null_mut(), &mut ctx) };

        assert_eq!(ctx.status_reg & 0x0000_FF01, 0x0000_0401);
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_000C),
            Some(fn64_runtime::InterruptSource::Pi.bit())
        );
    }

    #[test]
    fn disable_and_restore_interrupts_mutate_status_ie() {
        let mut ctx = ctx_zeroed();
        ctx.status_reg = 0x3400_0401;
        unsafe { __osDisableInt_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.r2, 1);
        assert_eq!(ctx.status_reg, 0x3400_0400);
        ctx.r4 = ctx.r2;
        unsafe { __osRestoreInt_recomp(std::ptr::null_mut(), &mut ctx) };
        assert_eq!(ctx.status_reg, 0x3400_0401);
    }

    #[test]
    fn os_initialize_installs_the_public_fpcsr_startup_bits() {
        FPC_CSR.with(|csr| csr.set(0));
        unsafe { osInitialize_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _) };
        let mut replace = ctx_zeroed();
        replace.r4 = 0;
        unsafe { __osSetFpcCsr_recomp(std::ptr::null_mut(), &mut replace) };
        assert_eq!(replace.r2, FPCSR_FS as u64 | FPCSR_EV as u64);
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

    #[test]
    fn os_set_time_and_get_time_round_trip_both_words() {
        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0x89AB_CDEF;
        set_ctx.r5 = 0x0123_4567;
        unsafe { osSetTime_recomp(std::ptr::null_mut(), &mut set_ctx) };

        let mut get_ctx = ctx_zeroed();
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut get_ctx) };
        assert_eq!(get_ctx.r2, 0x89AB_CDEF);
        assert_eq!(get_ctx.r3, 0x0123_4567);
    }

    #[test]
    fn count_register_is_settable_and_advances_without_aliasing_os_time() {
        let mut count_ctx = ctx_zeroed();
        count_ctx.r4 = 0xFFFF_FFF0;
        unsafe { osSetCount_recomp(std::ptr::null_mut(), &mut count_ctx) };

        let old_time = with_executor(|exec| exec.sim_time());
        with_executor(|exec| exec.advance_time(old_time + 0x20));
        let mut get_count = ctx_zeroed();
        unsafe { osGetCount_recomp(std::ptr::null_mut(), &mut get_count) };
        assert_eq!(
            get_count.r2, 0,
            "CP0 Count increments once per two CPU cycles and wraps at 32 bits"
        );

        let mut set_time = ctx_zeroed();
        set_time.r4 = 0x1234_5678;
        set_time.r5 = 0x9ABC_DEF0;
        unsafe { osSetTime_recomp(std::ptr::null_mut(), &mut set_time) };
        let mut unchanged_count = ctx_zeroed();
        unsafe { osGetCount_recomp(std::ptr::null_mut(), &mut unchanged_count) };
        assert_eq!(unchanged_count.r2, 0);
    }

    #[test]
    fn set_fpc_csr_returns_the_previous_register_value() {
        FPC_CSR.with(|csr| csr.set(0));
        let mut first = ctx_zeroed();
        first.r4 = 0x0100_0C01;
        unsafe { __osSetFpcCsr_recomp(std::ptr::null_mut(), &mut first) };
        assert_eq!(first.r2, 0);

        let mut second = ctx_zeroed();
        second.r4 = 0x0000_0003;
        unsafe { __osSetFpcCsr_recomp(std::ptr::null_mut(), &mut second) };
        assert_eq!(second.r2, 0x0100_0C01);
    }
}
