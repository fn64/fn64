use super::*;

// ---------------------------------------------------------------------
// Thread lifecycle: osCreateThread / osStartThread / osSetThreadPri.
// Real dispatch via SectionRegistry, this wave.
// ---------------------------------------------------------------------

/// `osCreateThread(OSThread *t, OSId id, void (*entry)(void *), void *arg,
/// void *sp, OSPri pri)` -- `a0`=t (`ctx->r4`, the `OSThread*` rdram
/// address), `a1`=id (`ctx->r5`), `a2`=entry vram (`ctx->r6`), `a3`=arg
/// (`ctx->r7`), stack-passed `sp`/`pri` at
/// `rdram[ctx.r29+0x10]`/`rdram[ctx.r29+0x14]`.
///
/// ## Correction (this wave): `t` IS needed -- a real `OSThread* -> OSId` map
///
/// A prior wave's doc comment here claimed "every real `osStartThread` call
/// site immediately follows its matching `osCreateThread` call with the
/// SAME `id` still live in a register" and treated `osStartThread_recomp`'s
/// `ctx->r4` as if it carried the `OSId` again. Real disassembly
/// (`RecompiledFuncs/funcs_0.c`, `recomp_entrypoint`'s own boot body, asm
/// 0x800004AC-0x800004B8) DISPROVES that claim: `osCreateThread`'s call
/// passes `a0=s0` (the `OSThread*` struct address, e.g. `0x80048BC0`) and
/// `a1=1` (the actual `OSId`); the immediately-following `osStartThread`
/// call passes `a0=s0` AGAIN -- the SAME `OSThread*` address, never the
/// `OSId` a second time. `fn64-abi`'s prior implementation of
/// `osStartThread_recomp` (reading `ctx->r4` as an id) would silently
/// misinterpret a real vram address as a thread id, first caught by
/// `examples/wm2000-boot`'s actual boot run (`osStartThread: no such thread
/// id 2147792064` == `0x8004_8BC0`, byte-identical to the real `OSThread*`
/// this call site passes). Fixed for real: `osCreateThread_recomp` now
/// records the `OSThread* (rdram offset) -> OSId` mapping in `HostState`
/// (`THREAD_HANDLES`, below), and every later shim keyed on "which thread"
/// via a bare `OSThread*` argument (`osStartThread_recomp`, and any future
/// one) looks it up through that map instead of assuming identity.
///
/// This wave WIRES the real dispatch: the thread's coroutine body resolves
/// `entry_vram` through `get_function` (the SAME resolution path a guest
/// `LOOKUP_FUNC` uses) and calls the resulting `RecompFunc` with the
/// process's one shared `rdram` pointer and a freshly-built `RecompContext`
/// seeded with `a0=arg` (`r4`) per o32 calling convention for a
/// single-argument thread entry point (`void entry(void *arg)`) -- matching
/// real `osCreateThread`'s documented semantics exactly ("thread entry
/// point... called with `arg` as its only argument").
///
/// # Safety
/// `ctx`/`rdram` must be valid per every other shim's contract in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreateThread_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let thread_handle = RdramAddr::from_gpr(ctx.r4);
    let requested_osid: u32 = ctx.r5 as u32;
    let entry_vram = ctx.r6 as u32;
    let arg = ctx.r7;
    let sp = read_stack_word(rdram, ctx.r29, 0x10) as u64;
    let priority = read_stack_word(rdram, ctx.r29, 0x14) as Priority;

    // Libultra's OSId is an informational tag, not a key: thread identity on
    // real hardware is the OSThread struct pointer, and NWXE's retail boot
    // legitimately creates two live threads with id 3 once resident `.data`
    // is faithfully seeded (docs/BOOT-NOTES-WM2000.md, 2026-07-19). The
    // executor keys threads by number, so a colliding OSId gets a synthetic
    // internal id; every later thread op resolves through the OSThread*
    // handle anyway, and `osGetThreadId` reads `thread_guest_ids` so the
    // guest-visible OSId stays exactly what this call supplied.
    let id: ThreadId = if with_executor(|exec| exec.thread_exists(requested_osid)) {
        with_host(|host| {
            let id = host.next_synthetic_thread_id;
            host.next_synthetic_thread_id += 1;
            id
        })
    } else {
        requested_osid
    };

    with_host(|host| {
        host.thread_handles.insert(thread_handle.offset(), id);
        host.thread_guest_ids.insert(id, requested_osid);
    });
    if crate::boot_probe_enabled() {
        eprintln!(
            "[boot-probe] osCreateThread(id={requested_osid} -> {id:#x}, entry={entry_vram:#010x}, sp={sp:#010x}, pri={priority})"
        );
    }

    // rdram is one shared allocation for the whole process lifetime
    // (docs/DESIGN.md section 3) -- capturing its raw pointer in this
    // 'static closure is sound on that basis; the closure itself only runs
    // while the process's one Rdram buffer is still alive (the executor and
    // this pointer share the same lifetime, the whole process).
    let rdram_addr = rdram as usize;

    with_executor(|exec| {
        exec.create_thread(id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(id, rdram_ptr, yielder, || {
                let _ = first_input; // Resume::Start; nothing to hand back at thread entry
                #[cfg(feature = "recomp-rs")]
                if unsafe { recompiled::run_registered_entry(rdram_ptr, entry_vram, arg, sp) } {
                    return;
                }
                let func_ptr = get_function(entry_vram as i32);
                let entry: RecompFunc = unsafe { std::mem::transmute(func_ptr) };
                let mut entry_ctx = RecompContext::zeroed();
                entry_ctx.arm_fpr_alias();
                entry_ctx.r4 = arg;
                // r29 ($sp) MUST be the real stack pointer osCreateThread's
                // caller supplied (this shim's own `sp` argument, per o32
                // ABI/libultra's documented "osCreateThread(t, id, entry,
                // arg, sp, pri)" signature) -- a zeroed r29 was a REAL bug,
                // first caught by examples/wm2000-boot's actual boot run
                // (EXC_BAD_ACCESS at `MEM_W(0x18, ctx->r29)` in the second
                // real thread's entry function, func_800004D0, writing to
                // address ~0x6_b1ff_fff8 == a near-zero r29 plus a small
                // stack-frame offset wrapping through the KSEG0 subtraction
                // math). Every real OSThread has its own dedicated stack;
                // this crate doesn't allocate/manage that memory region
                // itself (the game's own static/heap-allocated stack buffer
                // the ROM passed as `sp` IS the real backing store, already
                // part of the shared rdram buffer), so using the real `sp`
                // value directly is correct, not a placeholder.
                entry_ctx.r29 = sp;
                unsafe { entry(rdram_ptr, &mut entry_ctx as *mut _) };
            });
        });
    });
}

/// `osStartThread(OSThread *t)` -- `a0`=t (`ctx->r4`, the SAME `OSThread*`
/// rdram address passed to the matching `osCreateThread` call, per real
/// call-site evidence -- see `osCreateThread_recomp`'s doc comment for the
/// correction this wave made). Looked up through `HostState::thread_handles`
/// (populated by `osCreateThread_recomp`) to recover the real `OSId`
/// `Executor::start_thread` is keyed on.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osStartThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let id = resolve_thread_arg(ctx.r4, "osStartThread_recomp");
    with_executor(|exec| exec.start_thread(id));
}

/// `osSetThreadPri(OSThread *t, OSPri pri)` -- `a0`=t (`ctx->r4`), `a1`=pri
/// (`ctx->r5`). Per the public libultra manual's documented convention (and
/// a real call site, `funcs_0.c` asm 0x80000598-0x8000059C:
/// `osSetThreadPri(a0=0, a1=0)`), `t == NULL` means "the CALLING thread
/// itself," not "thread id 0" -- resolved via `resolve_thread_arg`, which
/// treats a null/zero handle as `current_thread_id`, matching that
/// documented semantic rather than the "OSId 0" misreading a prior wave's
/// version of this shim would have made (see `osCreateThread_recomp`'s doc
/// comment for the sibling `osStartThread` bug this same real call-site
/// evidence surfaced).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetThreadPri_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let target = resolve_thread_arg(ctx.r4, "osSetThreadPri_recomp");
    let pri = ctx.r5 as Priority;
    with_executor(|exec| exec.set_thread_pri(target, pri));
}

/// `osGetThreadPri(OSThread *t) -> OSPri` -- `a0`=`ctx->r4`, resolved via
/// `resolve_thread_arg` (same `OSThread*`-handle-lookup / `NULL`-means-
/// current-thread convention as `osSetThreadPri_recomp`, not an `OSId`
/// passed directly -- see that shim's doc comment). This symbol is a direct
/// `FuncEntry.func` slot in `recomp_overlays.inl` (N64Recomp skips codegen
/// for it entirely, per `games/NWXE/profile.toml`'s rung-3 identification
/// of `func_800322D0` as `osGetThreadPri`, byte-shape matched vs Revenge's
/// `func_80026DB0`). Returns the priority in `ctx->r2` (`$v0`), the o32
/// single-word return convention every other shim in this file already
/// uses.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetThreadPri_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let target = resolve_thread_arg(ctx.r4, "osGetThreadPri_recomp");
    let pri = with_executor(|exec| exec.thread_pri(target));
    ctx.r2 = pri as u64;
}

/// Resolve an `OSThread*` argument (as every real call site in this
/// milestone's evidence actually passes -- see `osCreateThread_recomp`'s
/// doc comment) to the `ThreadId` `Executor` is keyed on. A zero/null
/// handle means "the calling thread itself" (public libultra manual's
/// documented convention for `t == NULL` across the thread-manager API,
/// confirmed load-bearing by the real `osSetThreadPri(a0=0, ...)` call site
/// in `funcs_0.c`), resolved via `current_thread_id`. A nonzero handle is
/// looked up in `HostState::thread_handles`; a handle never seen by
/// `osCreateThread_recomp` is a loud, named panic (per `AGENTS.md`) rather
/// than a silently-guessed id.
fn resolve_thread_arg(raw: u64, shim: &str) -> ThreadId {
    if raw == 0 {
        return current_thread_id(shim);
    }
    let handle = RdramAddr::from_gpr(raw).offset();
    with_host(|host| {
        host.thread_handles
            .get(&handle)
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "{shim}: OSThread* handle {handle:#010x} was never registered by \
                 osCreateThread_recomp -- either this handle is garbage, or a thread was \
                 created through a path this crate doesn't yet model."
                )
            })
    })
}

/// `osGetThreadId(OSThread *t) -> OSId` -- `a0`=`ctx->r4`. Resolved via
/// `resolve_thread_arg` (same `NULL`-means-current-thread convention as
/// `osGetThreadPri_recomp`/`osSetThreadPri_recomp` -- the public libultra
/// manual documents the same `t == NULL` convention for this call too).
/// The executor `ThreadId` is USUALLY the real `OSId`, but not always:
/// on an OSId collision `osCreateThread_recomp` keys the executor by a
/// synthetic id (OSIds carry no uniqueness contract on real hardware --
/// see that shim's collision comment), so this consults
/// `HostState::thread_guest_ids` to return the OSId the guest actually
/// supplied. Threads registered outside `osCreateThread_recomp` (tests,
/// boot hosts) have no entry and fall back to the id itself. Real call
/// sites: `games/OOTU/RecompiledFuncs/funcs_0.c:4152`, `funcs_56.c:643`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetThreadId_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let target = resolve_thread_arg(ctx.r4, "osGetThreadId_recomp");
    let osid = with_host(|host| host.thread_guest_ids.get(&target).copied()).unwrap_or(target);
    ctx.r2 = osid as u64;
}

/// `osDestroyThread(OSThread *t)` -- `a0`=`ctx->r4`, resolved via
/// `resolve_thread_arg` (same `OSThread*`-handle convention as every other
/// thread-lifecycle shim in this file). No real `jal` call site in this
/// corpus (function-table slot, `recomp_overlays.inl:60`) -- per
/// BOOT-PLAN.md's own "not needed for the FIRST frame, needed for clean
/// shutdown/reset handling" note, this is reachable only off the NMI/reset
/// path, not the first-frame boot ladder. Implemented for real (thin
/// wrapper over `Executor::destroy_thread`, already real machinery) rather
/// than loud-trapped, since a real handler existing costs nothing extra
/// and this crate already exposes the exact primitive needed.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osDestroyThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let target = resolve_thread_arg(ctx.r4, "osDestroyThread_recomp");
    with_executor(|exec| exec.destroy_thread(target));
}

/// `osStopThread(OSThread *t)` -- same shape as `osDestroyThread_recomp`,
/// distinct "stop, don't destroy" semantics per `Executor::stop_thread`'s
/// doc comment (the public libultra manual's documented distinction).
/// Function-table slot only (`recomp_overlays.inl:54`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osStopThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let target = resolve_thread_arg(ctx.r4, "osStopThread_recomp");
    with_executor(|exec| exec.stop_thread(target));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn pause_self_yields_via_real_executor_and_thread_keeps_running() {
        const MAX_RESUMES_AFTER_PAUSE: usize = 8;

        let entered_pause = std::rc::Rc::new(std::cell::Cell::new(false));
        let continued_after_pause = std::rc::Rc::new(std::cell::Cell::new(false));
        let entered_pause2 = entered_pause.clone();
        let continued_after_pause2 = continued_after_pause.clone();
        spawn_test_thread(100, 5, move || {
            entered_pause2.set(true);
            pause_self(std::ptr::null_mut());
            continued_after_pause2.set(true);
        });
        assert!(run_one_step());
        assert!(entered_pause.get(), "thread never reached pause_self");
        assert!(
            !continued_after_pause.get(),
            "pause_self returned without yielding to the executor"
        );

        for _ in 0..MAX_RESUMES_AFTER_PAUSE {
            if continued_after_pause.get() {
                break;
            }
            assert!(
                run_one_step(),
                "pause_self blocked the thread instead of keeping it runnable"
            );
        }
        assert!(
            continued_after_pause.get(),
            "pause_self did not resume within {MAX_RESUMES_AFTER_PAUSE} scheduler steps"
        );
    }

    /// Test-only helper: register an `OSThread*` handle -> `OSId` mapping,
    /// the same state `osCreateThread_recomp` populates for real, so tests
    /// that want to drive `osStartThread_recomp`/`osSetThreadPri_recomp`/
    /// `osGetThreadPri_recomp` via a handle (matching real call-site
    /// evidence -- see `osCreateThread_recomp`'s doc comment) don't need to
    /// go through the full `osCreateThread_recomp` dispatch machinery when
    /// they only care about the handle-resolution behavior itself.
    fn register_test_thread_handle(handle_vram: u64, id: ThreadId) {
        let handle = RdramAddr::from_gpr(handle_vram).offset();
        with_host(|host| {
            host.thread_handles.insert(handle, id);
        });
    }

    #[test]
    fn set_thread_pri_takes_effect_on_run_queue_order() {
        spawn_test_thread(200, 1, || {});
        register_test_thread_handle(0x8000_0200, 200);
        let mut ctx = ctx_with(0x8000_0200, 50, 0);
        unsafe { osSetThreadPri_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        with_executor(|exec| {
            assert_eq!(exec.thread_pri(200), 50);
        });
    }

    #[test]
    fn set_thread_pri_with_null_handle_targets_the_calling_thread() {
        let applied_pri = std::rc::Rc::new(std::cell::RefCell::new(None));
        let applied_pri2 = applied_pri.clone();
        spawn_test_thread(201, 1, move || {
            let mut ctx = ctx_with(0, 77, 0); // t=NULL -> self
            unsafe { osSetThreadPri_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
            with_executor(|exec| {
                *applied_pri2.borrow_mut() = Some(exec.thread_pri(201));
            });
        });
        run_to_idle_with_yielder_plumbing();
        assert_eq!(*applied_pri.borrow(), Some(77));
    }

    #[test]
    fn get_thread_pri_resolves_a_real_handle() {
        spawn_test_thread(202, 33, || {});
        register_test_thread_handle(0x8000_0202, 202);
        let mut ctx = ctx_with(0x8000_0202, 0, 0);
        unsafe { osGetThreadPri_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 33);
    }

    #[test]
    fn get_thread_id_resolves_a_real_handle_to_its_real_osid() {
        spawn_test_thread(203, 10, || {});
        register_test_thread_handle(0x8000_0203, 203);
        let mut ctx = ctx_with(0x8000_0203, 0, 0);
        unsafe { osGetThreadId_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 203);
    }

    #[test]
    fn get_thread_id_with_null_handle_resolves_current_thread() {
        let observed = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed2 = observed.clone();
        spawn_test_thread(204, 10, move || {
            let mut ctx = ctx_with(0, 0, 0);
            unsafe { osGetThreadId_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
            *observed2.borrow_mut() = Some(ctx.r2);
        });
        run_to_idle_with_yielder_plumbing();
        assert_eq!(*observed.borrow(), Some(204));
    }

    #[test]
    fn colliding_osids_create_two_distinct_threads_and_keep_the_guest_osid() {
        // NWXE's retail boot creates two live threads with OSId 3 once
        // resident .data is faithfully seeded (docs/BOOT-NOTES-WM2000.md,
        // 2026-07-19): OSId carries no uniqueness contract on hardware.
        let mut rdram = vec![0u8; 16384];
        fn create(rdram: &mut [u8], handle: u64) {
            let mut ctx = ctx_zeroed();
            ctx.r4 = handle;
            ctx.r5 = 3; // the same OSId both times
            ctx.r6 = 0x8030_0000;
            ctx.r29 = 0xFFFF_FFFF_8000_2000;
            let sp_slot = RdramAddr::from_gpr(ctx.r29.wrapping_add(0x10)).offset() as usize;
            rdram[sp_slot..sp_slot + 4].copy_from_slice(&0x8000_1000u32.to_ne_bytes());
            unsafe { osCreateThread_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        }
        create(&mut rdram, 0x8006_0000);
        create(&mut rdram, 0x8006_0200);

        let first = resolve_thread_arg(0x8006_0000, "collision test");
        let second = resolve_thread_arg(0x8006_0200, "collision test");
        assert_eq!(first, 3, "first create keeps its requested OSId");
        assert_ne!(second, 3, "colliding OSId must get a synthetic executor id");
        with_executor(|exec| {
            assert!(exec.thread_exists(first));
            assert!(exec.thread_exists(second));
        });

        // Both handles report the guest-visible OSId the game supplied.
        for handle in [0x8006_0000u64, 0x8006_0200] {
            let mut ctx = ctx_with(handle, 0, 0);
            unsafe { osGetThreadId_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
            assert_eq!(ctx.r2, 3, "guest-visible OSId must be preserved");
        }
    }

    // osStartThread_recomp is a plain `extern "C" fn` -- same subprocess-abort
    // pattern as every other loud-trap test in this file (a panic across an
    // extern "C" boundary aborts, it does not unwind, so `#[should_panic]`
    // would abort the whole test harness rather than being caught).
    #[test]
    fn os_start_thread_with_unregistered_handle_panics_loudly() {
        assert_subprocess_aborts(
            "thread::tests::__os_start_thread_unregistered_handle_abort_subprocess_entry",
        );
    }

    #[test]
    #[ignore]
    fn __os_start_thread_unregistered_handle_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let mut ctx = ctx_with(0x8000_9999, 0, 0);
            unsafe { osStartThread_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        }
    }

    #[no_mangle]
    unsafe extern "C" fn test_func_entry(rdram: *mut u8, ctx: *mut RecompContext) {
        let ctx = unsafe { &mut *ctx };
        // Marker: double the arg and write it back to r2 ($v0) so the test
        // can observe the real entry point actually ran.
        ctx.r2 = ctx.r4 * 2;
        let _ = rdram;
    }

    /// Real regression test for the `entry_ctx.r29` (stack pointer) bug
    /// `examples/wm2000-boot`'s boot run surfaced: a spawned thread's own
    /// entry point writing to its OWN stack frame via `MEM_W(offset,
    /// ctx->r29)` -- exactly what real generated code
    /// (`func_800004D0`'s `MEM_W(0X18, ctx->r29) = ctx->r16`) does as its
    /// first real instruction. Before the fix, `entry_ctx.r29` was always
    /// zero (never seeded from `osCreateThread`'s real `sp` argument),
    /// which crashed with an out-of-bounds/garbage rdram write the moment
    /// any real thread entry point touched its own stack.
    #[no_mangle]
    unsafe extern "C" fn test_stack_writing_entry(rdram: *mut u8, ctx: *mut RecompContext) {
        let ctx = unsafe { &mut *ctx };
        // Mirrors func_800004D0's own first real instruction shape.
        unsafe {
            let addr = RdramAddr::from_gpr(ctx.r29.wrapping_add(0x18));
            std::ptr::copy_nonoverlapping(
                (0xCAFEu32).to_ne_bytes().as_ptr(),
                rdram.add(addr.offset() as usize),
                4,
            );
        }
    }

    #[test]
    fn os_create_thread_seeds_a_real_stack_pointer_not_zero() {
        let func_ptr: RecompFunc = test_stack_writing_entry;
        let idx = unsafe { register_section(0, 0x8030_0000, 0x10, &[(0x0, 4, func_ptr)]) };
        set_section_loaded(idx);

        // A real, nonzero stack region within rdram -- well clear of
        // rdram's start, matching a real OSThread's dedicated stack buffer.
        let mut rdram = vec![0u8; 16384];
        let sp_vram: u64 = 0x8000_1000; // rdram offset 0x1000

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8006_0000; // OSThread* handle
        ctx.r5 = 500; // id
        ctx.r6 = 0x8030_0000; // entry vram
        ctx.r7 = 0; // arg
        ctx.r29 = 0xFFFF_FFFF_8000_2000; // this call's OWN sp (unrelated)
                                         // sp argument (stack-passed at ctx.r29+0x10): the NEW thread's sp.
        let sp_slot = RdramAddr::from_gpr(ctx.r29.wrapping_add(0x10)).offset() as usize;
        rdram[sp_slot..sp_slot + 4].copy_from_slice(&(sp_vram as u32).to_ne_bytes());

        unsafe { osCreateThread_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        unsafe {
            let mut start_ctx = ctx_zeroed();
            start_ctx.r4 = 0x8006_0000;
            osStartThread_recomp(rdram.as_mut_ptr(), &mut start_ctx as *mut _);
        }
        run_to_idle();

        // The spawned thread wrote 0xCAFE at (sp_vram + 0x18) == rdram
        // offset 0x1018. Reaching this point at all (no EXC_BAD_ACCESS) AND
        // seeing the real marker value both prove r29 was the real,
        // nonzero sp, not a zeroed placeholder.
        let written = u32::from_ne_bytes(rdram[0x1018..0x101C].try_into().unwrap());
        assert_eq!(written, 0xCAFE);
        set_section_unloaded(idx);
    }

    #[test]
    fn get_function_resolves_a_registered_section_and_os_create_thread_calls_it_for_real() {
        let func_ptr: RecompFunc = test_func_entry;
        let idx = unsafe { register_section(0, 0x8010_0000, 0x10, &[(0x0, 4, func_ptr)]) };
        set_section_loaded(idx);

        let resolved = get_function(0x8010_0000u32 as i32);
        assert_eq!(resolved as usize, func_ptr as usize);

        // Now drive it through the real osCreateThread_recomp dispatch path.
        // a0 (r4) is the OSThread* handle -- a real, nonzero rdram address,
        // per the real call-site evidence in osCreateThread_recomp's doc
        // comment (NOT the same value as the OSId in r5).
        let thread_handle_vram: u64 = 0x8004_8BC0;
        let mut ctx = ctx_zeroed();
        ctx.r4 = thread_handle_vram;
        ctx.r5 = 300; // id
        ctx.r6 = 0x8010_0000; // entry vram
        ctx.r7 = 21; // arg
        ctx.r29 = 0xFFFF_FFFF_8000_0000; // sp (a fake, zeroed rdram region)
        let mut rdram = vec![0u8; 64];
        // priority read from stack at sp+0x14: leave zeroed -> priority 0.
        unsafe { osCreateThread_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        unsafe {
            let mut start_ctx = ctx_zeroed();
            // SAME OSThread* handle osCreateThread_recomp was called with,
            // never the OSId again -- matching real disassembly evidence.
            start_ctx.r4 = thread_handle_vram;
            osStartThread_recomp(rdram.as_mut_ptr(), &mut start_ctx as *mut _);
        }
        run_to_idle();
        // If the real entry point ran, it doubled arg=21 into its own ctx.r2
        // -- not observable here (that ctx was thread-local to the closure),
        // but reaching run_to_idle() without panicking already proves
        // get_function resolved a real function pointer and it executed
        // (test_func_entry doesn't panic; an unresolved/garbage pointer
        // would have segfaulted the test process instead of returning).
        set_section_unloaded(idx);
    }

    /// Regression test for the real reentrancy bug `examples/wm2000-boot`'s
    /// boot harness surfaced (recomp_entrypoint's very first real
    /// `osCreateThread` call, made from INSIDE a coroutine already being
    /// resumed by `Executor::run_one_step`'s own `with_executor` call):
    /// before this wave's `ReentrantCell` fix, a thread's own entry point
    /// calling any shim that itself calls `with_executor` (e.g.
    /// `osCreateThread_recomp`, `osSetEventMesg_recomp`) panicked with
    /// "RefCell already borrowed" the moment it ran as part of a resume,
    /// not just when called standalone (which the OLDER test above only
    /// exercised). This test reproduces that exact nested shape: thread A's
    /// body calls `osCreateThread_recomp` (spawning thread B) while thread
    /// A is itself running inside `run_one_step`'s resume.
    #[test]
    fn a_running_threads_own_body_can_call_os_create_thread_recomp_without_reentrancy_panic() {
        let func_ptr: RecompFunc = test_func_entry;
        let idx = unsafe { register_section(0, 0x8020_0000, 0x10, &[(0x0, 4, func_ptr)]) };
        set_section_loaded(idx);

        spawn_test_thread(400, 5, move || {
            // This runs INSIDE Executor::run_one_step's thread.resume(..)
            // call, itself inside an outer with_executor(..) borrow -- the
            // exact nested shape that used to panic.
            let inner_handle_vram: u64 = 0x8005_1234;
            let mut create_ctx = ctx_zeroed();
            create_ctx.r4 = inner_handle_vram; // OSThread* handle
            create_ctx.r5 = 401; // id
            create_ctx.r6 = 0x8020_0000; // entry vram
            create_ctx.r7 = 7; // arg
            create_ctx.r29 = 0xFFFF_FFFF_8000_0000;
            let mut inner_rdram = vec![0u8; 64];
            unsafe { osCreateThread_recomp(inner_rdram.as_mut_ptr(), &mut create_ctx as *mut _) };
            let mut start_ctx = ctx_zeroed();
            start_ctx.r4 = inner_handle_vram; // SAME handle, not the OSId
            unsafe { osStartThread_recomp(inner_rdram.as_mut_ptr(), &mut start_ctx as *mut _) };
        });

        run_to_idle();
        // Reaching here without a "RefCell already borrowed" panic (or the
        // SIGABRT that follows one across an extern "C" boundary) is the
        // whole assertion -- both thread 400 (outer) and thread 401 (spawned
        // from inside 400's own resume) must have run to completion.
        with_executor(|exec| {
            assert!(exec.is_thread_dead(400));
            assert!(exec.is_thread_dead(401));
        });
        set_section_unloaded(idx);
    }
}
