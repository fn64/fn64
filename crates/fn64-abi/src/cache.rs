use super::*;

/// `osInvalDCache(void *vaddr, s32 nbytes)` -- real hardware effect:
/// invalidates a range of the CPU's data cache (no host-visible effect
/// beyond memory ordering, since this crate has no CPU cache model of its
/// own -- rdram is a single Rust-owned buffer with no cache layer sitting
/// in front of it). Real call sites: `games/OOTU/RecompiledFuncs/funcs_0.c`
/// (x3), `funcs_49.c`. A safe, correct no-op: real N64 cache-maintenance
/// ops have no architecturally-visible effect other than "subsequent
/// reads see up-to-date memory," which is already unconditionally true for
/// a flat host buffer with no caching.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osInvalDCache_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osInvalICache(void *vaddr, s32 nbytes)` -- instruction-cache
/// counterpart to `osInvalDCache_recomp`; same no-op reasoning (no
/// instruction cache model in this crate -- generated code is real native
/// machine code the host CPU already keeps coherent). Real call sites:
/// `games/OOTU/RecompiledFuncs/funcs_0.c` (x3).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osInvalICache_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osWritebackDCache(void *vaddr, s32 nbytes)` -- writes dirty cache lines
/// back to RDRAM. Same no-op reasoning as `osInvalDCache_recomp`. Real call
/// site: `games/OOTU/RecompiledFuncs/funcs_49.c:687`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osWritebackDCache_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osWritebackDCacheAll(void)` -- no arguments; writes back the ENTIRE
/// data cache (vs. `osWritebackDCache_recomp`'s ranged variant, already
/// implemented above). Zero real call sites in this corpus (function-
/// table slot only, `recomp_overlays.inl:2969`) -- same no-cache-model
/// no-op reasoning as `osWritebackDCache_recomp`/`osInvalDCache_recomp`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osWritebackDCacheAll_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn cache_maintenance_ops_are_safe_callable_noops() {
        unsafe {
            osInvalDCache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
            osInvalICache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
            osWritebackDCache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
        }
    }
}
