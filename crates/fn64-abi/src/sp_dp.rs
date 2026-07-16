use super::*;

/// `__osSpSetPc(u32 pc)` -- `a0`=`ctx->r4` (verified: `funcs_57.c:1011`,
/// `ctx->r4 = 0 | 0;` immediately before the call -- a direct RSP-PC
/// register poke, part of the SP task-load sequence `osSpTaskLoad_recomp`
/// below also models). This crate has no RSP-register host state of its
/// own (task dispatch is handled synchronously and wholesale by
/// `osSpTaskYielded_recomp`, not by individually-set SP registers) -- a
/// safe no-op beyond existing as a callable symbol, matching
/// `osSetIntMask_recomp`'s "no real concurrent hardware to model" stance.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpSetPc_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `__osSpSetStatus(u32 status)` -- `a0`=`ctx->r4` (verified:
/// `funcs_55.c:30`, `ctx->r4 = ADD32(0, 0x4082);` immediately before the
/// call). Same no-op reasoning as `__osSpSetPc_recomp`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpSetStatus_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osDpSetStatus(u32 status)` -- `a0`=`ctx->r4` (verified: `funcs_55.c:22`,
/// `ctx->r4 = ADD32(0, 0x28);` immediately before the call). Real hardware
/// effect: writes the RDP `DP_STATUS` command register (clear/set flags for
/// XBUS/freeze/flush, per public N64 hardware documentation). This crate's
/// `MmioSpace` (`mmio.rs`) does not yet model a `DpRegs` block (only
/// AI/VI/PI/SI are modeled, per that module's own base-address table
/// comment) -- stored as plain thread-local host state for observability,
/// with no consumer needing it back yet (same "store it honestly, no
/// fabricated side effect" stance as `osContSetCh_recomp`), rather than
/// inventing DP register semantics with no call site exercising them.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpSetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    DP_STATUS.with(|cell| cell.set(ctx.r4 as u32));
}

thread_local! {
    static DP_STATUS: Cell<u32> = const { Cell::new(0) };
}

/// `__osSpGetStatus(void) -> u32` -- raw RCP `SP_STATUS_REG` read (real
/// hardware: `IO_READ(SP_STATUS_REG)`). Newly reachable after the
/// 2026-07-14 stub-set fix (`RcpUtils_PrintRegisterStatus`, a debug-print
/// helper, is no longer stubbed empty) -- this crate has no RCP
/// register-file model (`fn64_runtime`'s `Executor` tracks task/DMA state
/// as typed Rust structs, not raw MMIO bit layout), so a fabricated status
/// word would be an unearned guess about SP halt/broke/dma-busy bits a
/// caller could branch on. Loud trap, matching every other unmodeled-
/// hardware-register shim in this file.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSpGetStatus_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__osSpGetStatus_recomp: no RCP SP_STATUS_REG bit-layout model exists in this crate -- \
         reachable from RcpUtils_PrintRegisterStatus (games/OOTU/RecompiledFuncs), a debug-print \
         helper unstubbed by the 2026-07-14 gen_stubs.py false-positive-stub fix. A fabricated \
         status word would be an unearned guess a real caller could branch on."
    );
}

/// `osDpGetStatus(void) -> u32` -- raw RCP `DP_STATUS_REG` read, same shape
/// as `__osSpGetStatus_recomp` above (RDP half of the pair). Same
/// newly-reachable-via-`RcpUtils_PrintRegisterStatus` provenance and same
/// no-register-model reasoning.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDpGetStatus_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osDpGetStatus_recomp: no RCP DP_STATUS_REG bit-layout model exists in this crate -- \
         reachable from RcpUtils_PrintRegisterStatus (games/OOTU/RecompiledFuncs), a debug-print \
         helper unstubbed by the 2026-07-14 gen_stubs.py false-positive-stub fix. A fabricated \
         status word would be an unearned guess a real caller could branch on."
    );
}
