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
//   call (`__osMotorAccess`, `osMotorInit`, `__osSetFpcCsr`, `__ull_rem`,
//   `__ull_to_d`, `__ull_to_f`, `osJamMesg`, `osSetTime`) -- per
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

/// `__ull_rem(u64 a, u64 b) -> u64` -- unsigned 64-bit remainder,
/// `__umoddi3`-shaped compiler-rt helper. Zero real call sites in this
/// corpus (function-table slot only, `recomp_overlays.inl:56`) -- loud-
/// trapped since (unlike `__ll_div`/`__ll_mul`/`__ull_div`, which DO have
/// real call sites establishing their exact register shape) no call site
/// here confirms the r4:r5/r6:r7 argument-pair convention actually holds
/// for this specific symbol in this corpus; implementing an unverified
/// signature would be exactly the "plausible-sounding story, not actual
/// bytes" AGENTS.md warns against.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_rem_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__ull_rem_recomp: no real call site in games/OOTU/RecompiledFuncs exercises this \
         (function-table slot only) -- register-shape convention not independently confirmed \
         for this symbol in this corpus, see __ll_div_recomp's doc comment for the sibling \
         helpers that DO have verified call sites."
    );
}

/// `__ull_to_d(u64 a) -> f64` -- unsigned 64-bit-to-double conversion,
/// `__floatundidf`-shaped compiler-rt helper. Zero real call sites in this
/// corpus (function-table slot only, `recomp_overlays.inl:2971`) -- same
/// "unverified for this symbol" loud-trap reasoning as `__ull_rem_recomp`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_to_d_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__ull_to_d_recomp: no real call site in games/OOTU/RecompiledFuncs exercises this \
         (function-table slot only) -- see __ull_rem_recomp's doc comment for the reasoning."
    );
}

/// `__ull_to_f(u64 a) -> f32` -- unsigned 64-bit-to-float conversion,
/// `__floatundisf`-shaped compiler-rt helper. Same reasoning as
/// `__ull_to_d_recomp` (function-table slot only,
/// `recomp_overlays.inl:2972`, zero real call sites).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __ull_to_f_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__ull_to_f_recomp: no real call site in games/OOTU/RecompiledFuncs exercises this \
         (function-table slot only) -- see __ull_rem_recomp's doc comment for the reasoning."
    );
}
