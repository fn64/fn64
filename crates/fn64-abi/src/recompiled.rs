//! Typed-Rust recompiler adapters over the existing fn64 host ABI.
//!
//! The generated module stays `#![forbid(unsafe_code)]`: it calls ordinary
//! safe [`fn64_recomp_rs::RecompFunc`]s. Raw-pointer reconstruction is
//! confined here, beside the C ABI seam that already owns the identical
//! process-lifetime RDRAM and coroutine contracts.

use fn64_recomp_rs::{Rdram, RecompContext as RsContext, RecompFunc};
use fn64_runtime::{Priority, ThreadId};

use super::{with_active_yielder, with_executor, with_host, RecompContext as CContext};

type Lookup = fn(u32) -> RecompFunc;
type CShim = unsafe extern "C" fn(*mut u8, *mut CContext);

/// Return the static link-time vram for a callback pointer relocated into a
/// currently loaded overlay, if any.
pub fn canonical_vram(vram: u32) -> Option<u32> {
    with_host(|host| host.sections.canonical_vram(vram))
}

/// Install the generated module's dispatcher for both thread 0 and every
/// OSThread subsequently created by `osCreateThread`.
pub fn set_entry_lookup(lookup: Lookup, rdram_len: usize) {
    assert!(rdram_len > 0, "recompiled RDRAM length must be nonzero");
    with_host(|host| {
        host.recompiled_lookup = Some(lookup);
        host.recompiled_rdram_len = rdram_len;
    });
}

fn pause_active_recompiled_thread() {
    super::suspend_active_coroutine(fn64_runtime::Yield::PauseSelf);
}

/// Create and start thread 0 with a typed recompiled entrypoint on fn64's existing
/// single executor. No second executor, RDRAM allocation, or host thread is
/// created.
///
/// # Safety
/// `rdram` must address `rdram_len` live bytes for every coroutine's lifetime,
/// exactly like [`super::boot_thread0`]'s existing C ABI contract.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    rdram_len: usize,
    lookup: Lookup,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    set_entry_lookup(lookup, rdram_len);
    fn64_recomp_rs::set_host_pause(Some(pause_active_recompiled_thread));

    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        // SAFETY: inherited from this function's process-lifetime buffer
        // contract; this is the same executor registration as the C path.
        unsafe { exec.set_rdram_base(rdram) };
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                // SAFETY: the boot host guarantees the allocation outlives all
                // executor coroutines and contains exactly `rdram_len` bytes.
                let bytes = unsafe { std::slice::from_raw_parts_mut(rdram_ptr, rdram_len) };
                let mut mem = Rdram::new(bytes);
                let mut ctx = RsContext::new();
                entry(&mut ctx, &mut mem);
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Dispatch a newly-created OSThread through the installed typed module.
/// Returns `false` only for the legacy C configuration.
///
/// # Safety
/// `rdram` carries the same process-lifetime allocation contract as
/// `osCreateThread_recomp` and `recompiled::boot_thread0`.
pub(super) unsafe fn run_registered_entry(
    rdram: *mut u8,
    entry_vram: u32,
    arg: u64,
    sp: u64,
) -> bool {
    let registered = with_host(|host| {
        host.recompiled_lookup
            .map(|lookup| (lookup, host.recompiled_rdram_len))
    });
    let Some((lookup, rdram_len)) = registered else {
        return false;
    };
    assert!(rdram_len > 0, "recompiled entry lookup has no RDRAM length");
    // SAFETY: inherited from the caller's shared-allocation contract.
    let bytes = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut mem = Rdram::new(bytes);
    let mut ctx = RsContext::new();
    ctx.set_r(4, arg);
    ctx.set_r(29, sp);
    lookup(entry_vram)(&mut ctx, &mut mem);
    true
}

fn c_from_recompiled(ctx: &RsContext) -> CContext {
    let r = ctx.gprs();
    let mut c = CContext::zeroed();
    c.r0 = r[0];
    c.r1 = r[1];
    c.r2 = r[2];
    c.r3 = r[3];
    c.r4 = r[4];
    c.r5 = r[5];
    c.r6 = r[6];
    c.r7 = r[7];
    c.r8 = r[8];
    c.r9 = r[9];
    c.r10 = r[10];
    c.r11 = r[11];
    c.r12 = r[12];
    c.r13 = r[13];
    c.r14 = r[14];
    c.r15 = r[15];
    c.r16 = r[16];
    c.r17 = r[17];
    c.r18 = r[18];
    c.r19 = r[19];
    c.r20 = r[20];
    c.r21 = r[21];
    c.r22 = r[22];
    c.r23 = r[23];
    c.r24 = r[24];
    c.r25 = r[25];
    c.r26 = r[26];
    c.r27 = r[27];
    c.r28 = r[28];
    c.r29 = r[29];
    c.r30 = r[30];
    c.r31 = r[31];
    c.hi = ctx.hi;
    c.lo = ctx.lo;
    c.status_reg = ctx.cop0_status;
    c
}

fn copy_c_back(c: &CContext, ctx: &mut RsContext) {
    ctx.set_gprs([
        c.r0, c.r1, c.r2, c.r3, c.r4, c.r5, c.r6, c.r7, c.r8, c.r9, c.r10, c.r11, c.r12, c.r13,
        c.r14, c.r15, c.r16, c.r17, c.r18, c.r19, c.r20, c.r21, c.r22, c.r23, c.r24, c.r25, c.r26,
        c.r27, c.r28, c.r29, c.r30, c.r31,
    ]);
    ctx.hi = c.hi;
    ctx.lo = c.lo;
    ctx.cop0_status = c.status_reg;
}

fn call_c(ctx: &mut RsContext, mem: &mut Rdram<'_>, name: &'static str, shim: CShim) {
    if std::env::var_os("FN64_RECOMP_RS_SHIM_TRACE").is_some() {
        eprintln!("[fn64-recomp-rs-shim] {name}");
    }
    let mut c = c_from_recompiled(ctx);
    let rdram = mem.as_mut_slice().as_mut_ptr();
    // SAFETY: `rdram` comes from the live checked Rdram view and `c` is the
    // exact `#[repr(C)]` context the existing ABI shim requires. The shim may
    // suspend/resume this same coroutine, but neither pointer changes while
    // the adapter's stack frame remains live.
    unsafe { shim(rdram, &mut c) };
    copy_c_back(&c, ctx);
}

macro_rules! c_adapters {
    ($(($recompiled:ident, $shim:ident)),+ $(,)?) => {$ (
        pub fn $recompiled(ctx: &mut RsContext, mem: &mut Rdram<'_>) {
            call_c(ctx, mem, stringify!($shim), super::$shim);
        }
    )+ };
}

c_adapters!(
    (os_create_thread, osCreateThread_recomp),
    (os_start_thread, osStartThread_recomp),
    (os_set_thread_pri, osSetThreadPri_recomp),
    (os_get_thread_pri, osGetThreadPri_recomp),
    (os_create_mesg_queue, osCreateMesgQueue_recomp),
    (os_send_mesg, osSendMesg_recomp),
    (os_recv_mesg, osRecvMesg_recomp),
    (os_set_event_mesg, osSetEventMesg_recomp),
    (os_set_timer, osSetTimer_recomp),
    (os_cart_rom_init, osCartRomInit_recomp),
    (os_epi_start_dma, osEPiStartDma_recomp),
    (os_virtual_to_physical, osVirtualToPhysical_recomp),
    (os_create_pi_manager, osCreatePiManager_recomp),
    (os_si_raw_start_dma, __osSiRawStartDma_recomp),
    (os_set_int_mask, osSetIntMask_recomp),
    (os_initialize, osInitialize_recomp),
    (os_ai_set_frequency, osAiSetFrequency_recomp),
    (os_ai_get_length, osAiGetLength_recomp),
    (os_ai_set_next_buffer, osAiSetNextBuffer_recomp),
    (os_get_mem_size, osGetMemSize_recomp),
    (os_inval_dcache, osInvalDCache_recomp),
    (os_inval_icache, osInvalICache_recomp),
    (os_writeback_dcache, osWritebackDCache_recomp),
    (os_disable_int, __osDisableInt_recomp),
    (os_restore_int, __osRestoreInt_recomp),
    (os_get_thread_id, osGetThreadId_recomp),
    (os_get_time, osGetTime_recomp),
    (os_sp_task_yielded, osSpTaskYielded_recomp),
    (os_create_vi_manager, osCreateViManager_recomp),
    (os_vi_set_event, osViSetEvent_recomp),
    (os_vi_set_mode, osViSetMode_recomp),
    (os_vi_set_special_features, osViSetSpecialFeatures_recomp),
    (os_vi_set_y_scale, osViSetYScale_recomp),
    (os_vi_swap_buffer, osViSwapBuffer_recomp),
    (os_vi_black, osViBlack_recomp),
    (ll_div, __ll_div_recomp),
    (ll_mul, __ll_mul_recomp),
    (ull_div, __ull_div_recomp),
    (os_pi_get_access, __osPiGetAccess_recomp),
    (os_pi_rel_access, __osPiRelAccess_recomp),
    (os_sp_set_pc, __osSpSetPc_recomp),
    (os_sp_set_status, __osSpSetStatus_recomp),
    (os_cont_get_query, osContGetQuery_recomp),
    (os_cont_get_read_data, osContGetReadData_recomp),
    (os_cont_init, osContInit_recomp),
    (os_cont_set_ch, osContSetCh_recomp),
    (os_cont_start_query, osContStartQuery_recomp),
    (os_cont_start_read_data, osContStartReadData_recomp),
    (os_destroy_thread, osDestroyThread_recomp),
    (os_stop_thread, osStopThread_recomp),
    (os_dp_set_status, osDpSetStatus_recomp),
    (os_epi_read_io, osEPiReadIo_recomp),
    (os_epi_write_io, osEPiWriteIo_recomp),
    (os_get_count, osGetCount_recomp),
    (os_jam_mesg, osJamMesg_recomp),
    (os_sp_task_load, osSpTaskLoad_recomp),
    (os_sp_task_start_go, osSpTaskStartGo_recomp),
    (os_sp_task_yield, osSpTaskYield_recomp),
    (os_stop_timer, osStopTimer_recomp),
    (
        os_vi_get_current_framebuffer,
        osViGetCurrentFramebuffer_recomp
    ),
    (os_vi_get_next_framebuffer, osViGetNextFramebuffer_recomp),
    (os_writeback_dcache_all, osWritebackDCacheAll_recomp),
    (os_sp_get_status, __osSpGetStatus_recomp),
    (os_dp_get_status, osDpGetStatus_recomp),
);

/// `__osGetSR`: read this OSThread's typed COP0 Status register.
pub fn os_get_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_status as u64);
}

/// `__osSetSR`: replace this OSThread's typed COP0 Status register.
pub fn os_set_sr(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.cop0_status = ctx.r_u32(4);
}

/// `__osGetCause`: the executor does not synthesize CPU exception frames, so
/// this reads the explicit typed Cause state (normally zero).
pub fn os_get_cause(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, ctx.cop0_cause as u64);
}

/// `osGetIntMask`: return the same process-local interrupt mask maintained by
/// `osSetIntMask_recomp`.
pub fn os_get_int_mask(ctx: &mut RsContext, _mem: &mut Rdram<'_>) {
    ctx.set_r(2, super::INT_MASK.with(|cell| cell.get()) as u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_adapter_round_trips_all_gprs_and_forces_zero() {
        let mut recompiled = RsContext::new();
        for i in 1..32 {
            recompiled.set_r(i, 0xA000_0000_0000_0000 | i as u64);
        }
        let mut c = c_from_recompiled(&recompiled);
        c.r0 = u64::MAX;
        c.r2 = 0x1234;
        copy_c_back(&c, &mut recompiled);
        assert_eq!(recompiled.r(0), 0);
        assert_eq!(recompiled.r(2), 0x1234);
        assert_eq!(recompiled.r(31), 0xA000_0000_0000_001F);
    }

    #[test]
    fn status_adapters_are_per_context_state() {
        let mut bytes = [0; 4];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RsContext::new();
        ctx.set_r(4, 0x3400_0001);
        os_set_sr(&mut ctx, &mut mem);
        ctx.set_r(2, 0);
        os_get_sr(&mut ctx, &mut mem);
        assert_eq!(ctx.r_u32(2), 0x3400_0001);
    }
}
