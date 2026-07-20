use super::*;

// ---------------------------------------------------------------------
// OoT (OOTU) boot-critical shims: 33 real undefined symbols surfaced by
// `examples/oot-boot`'s first real `cargo build --release` link attempt
// (aki-recomp/games/OOTU/docs/BOOT-PLAN.md's 44-shim estimate was a
// pre-link guess; the real linker's undefined-symbol list is the ground
// truth used here). Split per each symbol's REAL call-site count in
// `games/OOTU/RecompiledFuncs/*.c` (`grep -rc "<sym>_recomp("`), not the
// doc's estimate:
//
// - Real, load-bearing `jal` call sites in this corpus (64-bit
//   soft-arith, PI-bus mutex, cache/interrupt no-ops, SP register pokes,
//   EPI single-word IO, DP status): implemented for real below.
// - `recomp_overlays.inl`-only entries (function-table slots, address-
//   taken for indirect dispatch by Sched/PadMgr/IrqMgr's own internal
//   thread bodies -- BOOT-PLAN.md rungs 13/15 name these as reached at
//   runtime even though no literal `jal` shows up in this static corpus):
//   implemented for real where the boot ladder's rung analysis says they
//   are reached (SP task control, contmgr, timer stop), loud-trapped
//   where no evidence (this corpus OR the boot doc) shows a reachable
//   call (`__osMotorAccess`, `osMotorInit`, `__osSetFpcCsr`,
//   `__ull_to_d`, `__ull_to_f`, `osJamMesg`) -- per
//   AGENTS.md's "loud traps, no silent shrugs," a fabricated return value
//   for genuinely untested code is worse than refusing.
// ---------------------------------------------------------------------

/// `__ll_div(s64 a, s64 b) -> s64` -- o32 64-bit-argument convention splits
/// each `s64` across a register PAIR (`a`=r4:r5 hi:lo, `b`=r6:r7 hi:lo),
/// result likewise in r2:r3 hi:lo -- verified against the real call site
/// (`funcs_57.c:2165`: `ctx->r4=r6|0; ctx->r5=r7|0; ctx->r6=MEM_W(sp,0x40);
/// ctx->r7=MEM_W(sp,0x44); __ll_div_recomp(...)`, then `MEM_W(sp,0x20)=r2;
/// MEM_W(sp,0x24)=r3`). Standard signed 64-bit division, the documented
/// compiler-rt `__divdi3` shape every MIPS o32 toolchain emits for a
/// 64-bit `/` operator no single MIPS instruction covers.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ll_div_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let a = ((ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64)) as i64;
    let b = ((ctx.r6 as u32 as u64) << 32 | (ctx.r7 as u32 as u64)) as i64;
    let result = if b == 0 {
        // Real hardware/compiler-rt behavior for integer division by zero
        // is undefined; this crate has no evidence any real OoT boot-path
        // call site divides by zero (a divide-by-zero here would itself be
        // a real game-logic bug worth surfacing loudly rather than
        // silently producing a fabricated quotient).
        panic!("__ll_div_recomp: division by zero");
    } else {
        a.wrapping_div(b)
    };
    ctx.r2 = (result >> 32) as u64;
    ctx.r3 = (result & 0xFFFF_FFFF) as u64;
}

/// `__ll_mul(s64 a, s64 b) -> s64` -- same r4:r5/r6:r7 -> r2:r3 hi:lo
/// argument/return shape as `__ll_div_recomp` (verified: `funcs_57.c:2183`'s
/// call site immediately follows `__ll_div_recomp`'s, same register
/// pattern: `ctx->r4=MEM_W(sp,0x40); ctx->r5=MEM_W(sp,0x44); ctx->r6=r2|0;
/// ctx->r7=r3|0; __ll_mul_recomp(...)`). Standard signed 64-bit
/// multiplication (`__muldi3`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ll_mul_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let a = ((ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64)) as i64;
    let b = ((ctx.r6 as u32 as u64) << 32 | (ctx.r7 as u32 as u64)) as i64;
    let result = a.wrapping_mul(b);
    ctx.r2 = (result >> 32) as u64;
    ctx.r3 = (result & 0xFFFF_FFFF) as u64;
}

/// `__ull_div(u64 a, u64 b) -> u64` -- unsigned counterpart to
/// `__ll_div_recomp`, same r4:r5/r6:r7 -> r2:r3 argument/return shape
/// (verified: `funcs_0.c:4342`'s call site, `ctx->r4=r2|0; ctx->r5=r3|0;
/// ctx->r6=0; ctx->r7=0x40; __ull_div_recomp(...)`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_div_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let a = (ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64);
    let b = (ctx.r6 as u32 as u64) << 32 | (ctx.r7 as u32 as u64);
    let result = if b == 0 {
        panic!("__ull_div_recomp: division by zero");
    } else {
        a.wrapping_div(b)
    };
    ctx.r2 = result >> 32;
    ctx.r3 = result & 0xFFFF_FFFF;
}

/// `__ull_rem(u64 a, u64 b) -> u64` -- unsigned remainder counterpart to
/// `__ull_div_recomp`. Compiler arithmetic helpers use the same o32 ABI for
/// identical argument and result types: r4:r5/r6:r7 -> r2:r3, whose exact
/// word ordering is established by `__ull_div_recomp`'s generated-C call
/// site. The operation is the `__umoddi3` remainder paired with that
/// quotient helper. A zero divisor remains a loud failure: C leaves integer
/// division by zero undefined, so manufacturing a remainder would conceal
/// guest corruption.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_rem_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let a = (ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64);
    let b = (ctx.r6 as u32 as u64) << 32 | (ctx.r7 as u32 as u64);
    assert_ne!(b, 0, "__ull_rem_recomp: division by zero");
    let result = a % b;
    ctx.r2 = result >> 32;
    ctx.r3 = result & 0xFFFF_FFFF;
}

/// `__ull_to_d(u64 a) -> f64` -- unsigned 64-bit-to-double conversion,
/// `__floatundidf`-shaped compiler-rt helper. Zero real call sites in this
/// corpus (function-table slot only, `recomp_overlays.inl:2971`). The System
/// V MIPS ABI supplement specifies a double result in the `$f0/$f1` pair;
/// N64Recomp's `Fpr` union represents that pair as `f0.d`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_to_d_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let value = (ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64);
    ctx.f0.d = value as f64;
}

/// `__ull_to_f(u64 a) -> f32` -- unsigned 64-bit-to-float conversion,
/// `__floatundisf`-shaped compiler-rt helper. The same MIPS ABI table places
/// a single-precision result in `$f0`, represented by `f0.halves.0` in the
/// generated context.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_to_f_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let value = (ctx.r4 as u32 as u64) << 32 | (ctx.r5 as u32 as u64);
    ctx.f0.halves.0 = value as f32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ctx_zeroed;

    #[test]
    fn ull_rem_uses_o32_u64_argument_and_result_pairs() {
        let a = 0xFEDC_BA98_7654_3210_u64;
        let b = 0x0000_0001_2345_6789_u64;
        let mut ctx = ctx_zeroed();
        ctx.r4 = a >> 32;
        ctx.r5 = a as u32 as u64;
        ctx.r6 = b >> 32;
        ctx.r7 = b as u32 as u64;

        unsafe { __ull_rem_recomp(std::ptr::null_mut(), &mut ctx) };

        let result = (ctx.r2 << 32) | (ctx.r3 & 0xFFFF_FFFF);
        assert_eq!(result, a % b);
    }

    #[test]
    fn ull_float_conversions_return_through_f0_with_ieee_rounding() {
        let value = 0xFEDC_BA98_7654_3210_u64;
        let mut double_ctx = ctx_zeroed();
        double_ctx.r4 = value >> 32;
        double_ctx.r5 = value as u32 as u64;
        unsafe { __ull_to_d_recomp(std::ptr::null_mut(), &mut double_ctx) };
        assert_eq!(
            unsafe { double_ctx.f0.d }.to_bits(),
            (value as f64).to_bits()
        );

        let mut float_ctx = ctx_zeroed();
        float_ctx.r4 = value >> 32;
        float_ctx.r5 = value as u32 as u64;
        unsafe { __ull_to_f_recomp(std::ptr::null_mut(), &mut float_ctx) };
        assert_eq!(
            unsafe { float_ctx.f0.halves.0 }.to_bits(),
            (value as f32).to_bits()
        );
    }
}
