//! fn64-abi: the extern "C" surface `RecompiledFuncs/*.c` links against.
//!
//! See `docs/DESIGN.md` section 1: this crate is deliberately thin --
//! every symbol here is a signature-and-marshalling adapter over
//! `fn64-runtime`, never a place new policy gets invented.
//!
//! ## Signatures verified directly against real generated C (this wave)
//!
//! Every prior wave's signature assumption for `pause_self`/`switch_error`/
//! `do_break`/`recomp_context` was WRONG in a way that would not have linked
//! against a real `RecompiledFuncs` archive -- caught this wave by reading
//! `aki-recomp/games/NWXE/RecompiledFuncs/recomp.h` (N64Recomp's own
//! MIT-licensed generated/vendored header, included verbatim by every
//! `RecompiledFuncs/*.c`) directly, rather than re-deriving from
//! `ABI-SURFACE.md`'s prose summary alone:
//!
//! - `recomp_context` is the REAL 32-gpr + 32-fpr + hi/lo/f_odd/status_reg
//!   struct (`recomp.h`'s verbatim `typedef struct {...} recomp_context`),
//!   not a 9-field subset. A previous wave's `RecompContext` only modeled
//!   `r0..r7,r29` -- correct for the symbols it touched, but the wrong
//!   shape to link against the REAL `recomp.h`-including translation units,
//!   since every one of them accesses `recomp_context` through the actual
//!   struct layout their own compiler emitted; a `#[repr(C)]` struct on the
//!   Rust side with fewer fields than the real one is a straight ABI
//!   mismatch the moment any function this crate doesn't define also
//!   touches, e.g., `r30` (`$fp`) or a float register -- verified directly
//!   in the corpus (`funcs_15.c`'s `__osSiRawStartDma_recomp` call site uses
//!   `ctx->r30`). This wave's `RecompContext` is the full verbatim struct.
//! - `pause_self` is `void pause_self(uint8_t *rdram)` -- ONE argument, no
//!   `recomp_context*` -- per `recomp.h`'s own declaration and every real
//!   call site (`grep -n "pause_self(rdram)" RecompiledFuncs/*.c`: always
//!   exactly one argument). A previous wave's `pause_self(*mut u8, *mut
//!   RecompContext)` would not link against the real generated call sites.
//! - `switch_error(const char* func, uint32_t vram, uint32_t jtbl)` and
//!   `do_break(uint32_t vram)` -- real signatures from `recomp.h`, verified
//!   against call sites (`funcs_12.c`: `switch_error(__func__, 0x8002ABCC,
//!   0x8004B130)`; `funcs_11.c`: `do_break(2147643904)`). Neither takes
//!   `rdram`/`ctx` at all.
//! - `get_function(int32_t vram) -> recomp_func_t*` -- one argument, per
//!   `recomp.h`; backed by `fn64_runtime::SectionRegistry::resolve`
//!   (this wave's new overlay-registry piece, `docs/DESIGN.md` section 1's
//!   long-deferred "wave 3's last item").
//! - `osCreateThread(OSThread *t, OSId id, void (*entry)(void*), void* arg,
//!   void* sp, OSPri pri)` -- `t`=r4, `id`=r5, `entry`=r6, `arg`=r7,
//!   `sp`/`pri` stack-passed at `rdram[ctx.r29+0x10]`/`rdram[ctx.r29+0x14]`
//!   (o32 ABI, verified directly against the real call site in
//!   `funcs_0.c`: `MEM_W(0X10, ctx->r29) = ctx->r2` immediately before the
//!   call, i.e. the 5th arg is stored to `sp+0x10` right before the `jal`).
//!   This wave WIRES the real dispatch (the overlay/`get_function` lookup
//!   table this crate's module doc for a previous wave named as the
//!   missing piece) -- `osCreateThread_recomp`/`osStartThread_recomp` are
//!   no longer `unimplemented!()`.
//!
//! ## The executor integration (unchanged from prior waves)
//!
//! Exactly one `fn64_runtime::Executor` exists per process, in a
//! `thread_local!`. Every shim reaches it through `with_executor` -- THE
//! single gateway, see that function's own doc comment for the full
//! reentrancy audit (what `Yield`/`Resume` already close out at the type
//! level vs. the one dynamic case `ReentrantCell` still exists for). A
//! coroutine body never calls `with_executor` to pre-check a
//! potentially-blocking operation (the reentrancy bug a previous wave
//! caught and fixed -- see the "reentrancy" note in `suspend_active_coroutine`'s
//! doc comment); it only ever calls `suspend_active_coroutine`
//! unconditionally and lets the executor's `handle_yield` decide.
//!
//! ## What's new this wave (the M1 gate: link against real WM2000 output)
//!
//! Per `aki-recomp/runtime/M1-WORKLIST.md`'s 23-symbol undefined set:
//! - T1 structural: `get_function`/`switch_error`/`do_break` (this file),
//!   backed by `SectionRegistry` (`fn64-runtime`, new this wave).
//! - T1 PI/ROM seam: `osCartRomInit_recomp`/`osEPiStartDma_recomp`/
//!   `osVirtualToPhysical_recomp`/`osSetIntMask_recomp`/`osInitialize_recomp`/
//!   `osAiSetFrequency_recomp`/`__osSiRawStartDma_recomp`/
//!   `osSpTaskYielded_recomp`, backed by `fn64_runtime::rom::PiDma`/plain
//!   host-state fields on `HostState` (new this wave).
//! - T1 thread lifecycle completion: `osCreateThread_recomp`/
//!   `osStartThread_recomp` now really dispatch via `SectionRegistry`.
//! - T2 loud traps: the VI family (`osViSetMode`/`osViSetSpecialFeatures`/
//!   `osViSetYScale`/`osViSwapBuffer`/`osViBlack`) and `osSetEventMesg`
//!   (already real, unchanged) -- VI shims are loud `unimplemented!()`s
//!   (no real display backend exists yet in this crate; per `AGENTS.md`,
//!   a silently-succeeding VI stub would be worse than refusing).
//!   `osSetTimer_recomp` IS wired for real (the executor's `TimerWheel`
//!   already exists and needs no new host backend, unlike VI's display
//!   surface).

use std::cell::{Cell, RefCell};

use corosensei::Yielder;
use fn64_audio::AudioBackend;
use fn64_render::RenderBackend;
use fn64_runtime::{
    DmaDirection, Executor, ExternalEvent, InMemoryRom, Mesg, OsTaskHeader, PiDma, Priority,
    RdramAddr, Resume, Section, SectionRegistry, ThreadId, Yield, M_AUDTASK, M_GFXTASK,
};

#[cfg(feature = "native-recomp")]
pub mod native;

/// MIPS `recomp_context`, the REAL verbatim layout from `recomp.h` (MIT) --
/// see module doc's "Signatures verified directly against real generated C"
/// for why a prior wave's 9-field subset was an ABI mismatch. `fpr` mirrors
/// `recomp.h`'s union (double / {float,float} / {u32,u32} / u64); no shim in
/// this crate reads float fields yet, but the struct must be layout-correct
/// end to end since real recompiled C accesses fields this crate doesn't
/// otherwise touch (e.g. `r30` in `__osSiRawStartDma`'s real call site).
#[repr(C)]
#[derive(Copy, Clone)]
pub union Fpr {
    pub d: f64,
    pub halves: (f32, f32),
    pub u32_halves: (u32, u32),
    pub u64_bits: u64,
}

#[repr(C)]
pub struct RecompContext {
    pub r0: u64,
    pub r1: u64,
    pub r2: u64,
    pub r3: u64,
    pub r4: u64,
    pub r5: u64,
    pub r6: u64,
    pub r7: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub r16: u64,
    pub r17: u64,
    pub r18: u64,
    pub r19: u64,
    pub r20: u64,
    pub r21: u64,
    pub r22: u64,
    pub r23: u64,
    pub r24: u64,
    pub r25: u64,
    pub r26: u64,
    pub r27: u64,
    pub r28: u64,
    pub r29: u64,
    pub r30: u64,
    pub r31: u64,
    pub f0: Fpr,
    pub f1: Fpr,
    pub f2: Fpr,
    pub f3: Fpr,
    pub f4: Fpr,
    pub f5: Fpr,
    pub f6: Fpr,
    pub f7: Fpr,
    pub f8: Fpr,
    pub f9: Fpr,
    pub f10: Fpr,
    pub f11: Fpr,
    pub f12: Fpr,
    pub f13: Fpr,
    pub f14: Fpr,
    pub f15: Fpr,
    pub f16: Fpr,
    pub f17: Fpr,
    pub f18: Fpr,
    pub f19: Fpr,
    pub f20: Fpr,
    pub f21: Fpr,
    pub f22: Fpr,
    pub f23: Fpr,
    pub f24: Fpr,
    pub f25: Fpr,
    pub f26: Fpr,
    pub f27: Fpr,
    pub f28: Fpr,
    pub f29: Fpr,
    pub f30: Fpr,
    pub f31: Fpr,
    pub hi: u64,
    pub lo: u64,
    pub f_odd: *mut u32,
    pub status_reg: u32,
    pub mips3_float_mode: u8,
}

/// A `recomp_func_t*` -- `extern "C" fn(*mut u8, *mut RecompContext)`, the
/// real signature every `RECOMP_FUNC`/section `FuncEntry.func` shares (per
/// `recomp.h`: `typedef void (recomp_func_t)(uint8_t* rdram, recomp_context*
/// ctx);`).
pub type RecompFunc = unsafe extern "C" fn(*mut u8, *mut RecompContext);

/// Host-side, non-guest state this crate owns beyond the executor: the
/// overlay/section registry `get_function` resolves against, and the PI/ROM
/// DMA engine. Kept alongside (not inside) `fn64_runtime::Executor` --
/// `Executor` is guest-scheduling state (`docs/DESIGN.md` section 2);
/// sections/ROM are a orthogonal, PI-manager-owned resource per
/// `docs/DESIGN.md` section 1's crate-boundary reasoning and `rom.rs`'s own
/// module doc ("no Executor dependency in this crate" for `PiDma`) -- this
/// struct is the seam that DOES know about both, exactly where the task
/// says the PI/ROM completion posts through `inject_event`.
struct HostState {
    sections: SectionRegistry,
    /// `None` until `fn64-shell`/a test supplies a real ROM via
    /// `set_rom`/`load_test_rom` -- PI-DMA shims called before that panic
    /// loudly (see `with_pi_dma`) rather than silently no-op'ing, matching
    /// `AGENTS.md`'s "loud traps."
    pi_dma: Option<PiDma<InMemoryRom>>,
    /// Guest-visible `OSPiHandle*` returned by `osCartRomInit`. The handle
    /// storage is game-owned BSS, so the boot host supplies its link address;
    /// leaving it unset is a loud trap rather than returning a stale `$v0`.
    cart_rom_handle_vram: Option<u32>,
    /// `OSThread*` (rdram-relative offset) -> `OSId`, populated by
    /// `osCreateThread_recomp`. Needed because real call sites pass the SAME
    /// `OSThread*` handle to `osStartThread`/`osSetThreadPri`/etc, NOT the
    /// `OSId` a second time -- see `osCreateThread_recomp`'s doc comment for
    /// the real disassembly evidence that disproved a prior wave's opposite
    /// assumption.
    thread_handles: std::collections::HashMap<u32, ThreadId>,
    /// `OSTimer*` (rdram-relative offset) -> `TimerId`, populated by
    /// `osSetTimer_recomp`. Same shape as `thread_handles`: a real
    /// `osStopTimer(t)` call site (OoT's boot-critical set, per
    /// BOOT-PLAN.md's rung-13 note) passes the SAME `OSTimer*` struct
    /// address `osSetTimer` was given, never the `TimerWheel`-internal
    /// `TimerId` a second time.
    timer_handles: std::collections::HashMap<u32, fn64_runtime::timer::TimerId>,
    /// Typed-Rust whole-ROM dispatcher installed by a native boot host. When
    /// present, `osCreateThread` resolves the new OSThread's entry through
    /// this table and owns a native `RecompContext` inside the SAME executor
    /// coroutine used by the C path.
    #[cfg(feature = "native-recomp")]
    native_lookup: Option<fn(u32) -> fn64_recomp_native::RecompFunc>,
    /// Length of the process-wide RDRAM/MMIO allocation behind `ACTIVE_RDRAM`.
    /// Required to rebuild the checked native `Rdram` view at a spawned
    /// thread's entry without creating a second memory model or allocation.
    #[cfg(feature = "native-recomp")]
    native_rdram_len: usize,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            sections: SectionRegistry::new(),
            pi_dma: None,
            cart_rom_handle_vram: None,
            thread_handles: std::collections::HashMap::new(),
            timer_handles: std::collections::HashMap::new(),
            #[cfg(feature = "native-recomp")]
            native_lookup: None,
            #[cfg(feature = "native-recomp")]
            native_rdram_len: 0,
        }
    }
}

/// Reentrant-safe interior mutability for `Executor`, replacing a plain
/// `RefCell` (see `with_executor`'s doc comment -- the crate's one gateway
/// to this cell -- for the real bug this fixes and the audited verdict on
/// why it is still needed after `Yield`/`Resume` closed the OTHER
/// reentrancy shape). `ReentrantCell` only guards against what WOULD be a
/// real bug: two overlapping calls trying to actually dereference the
/// pointer at once, which cannot happen on one thread without unsafe code
/// elsewhere doing something even more wrong.
struct ReentrantCell<T> {
    inner: std::cell::UnsafeCell<T>,
}

impl<T> ReentrantCell<T> {
    const fn new(value: T) -> Self {
        ReentrantCell {
            inner: std::cell::UnsafeCell::new(value),
        }
    }

    /// Borrow `&mut T` for the duration of `f`. Nesting (calling `with`
    /// again from inside `f`) is exactly the supported case this type
    /// exists for -- see the type's doc comment for why that's sound here.
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // Safety: single-threaded by construction (thread_local storage,
        // never Sync/Send across threads); nested calls never
        // simultaneously dereference the pointer (only ever one active
        // `&mut T` borrow "in flight" at the innermost currently-executing
        // frame) -- see the type doc comment for the full argument.
        let ptr = self.inner.get();
        f(unsafe { &mut *ptr })
    }
}

thread_local! {
    /// The one executor instance -- see module doc for why a thread-local
    /// (not a bare global) is the correct scope. Private with no accessor
    /// other than `with_executor` (below) -- see that function's doc comment
    /// for the full reentrancy audit, including why `ReentrantCell` (not
    /// `RefCell`) is the right cell type here.
    static EXECUTOR: ReentrantCell<Executor> = ReentrantCell::new(Executor::new());

    /// Overlay/section registry + PI/ROM state -- see `HostState` doc.
    /// Separate `RefCell` from `EXECUTOR` (not merged into one struct)
    /// because `get_function`/PI-DMA shims and executor-touching shims are
    /// never called re-entrantly against each other in a way that would
    /// need one combined borrow -- keeping them separate means a
    /// `get_function` lookup from inside a coroutine body (extremely
    /// common: `LOOKUP_FUNC` fires on nearly every indirect call) never
    /// risks colliding with an outstanding `EXECUTOR` borrow at all, closing
    /// off an entire class of the reentrancy hazard this module's doc
    /// discusses by construction rather than by care at each call site.
    static HOST: RefCell<HostState> = RefCell::new(HostState::default());

    /// The `Yielder` for whichever coroutine is currently being resumed --
    /// see module doc.
    static ACTIVE_YIELDER: Cell<Option<*const Yielder<Resume, Yield>>> = const { Cell::new(None) };

    /// Which `ThreadId` is the currently-resumed coroutine.
    static ACTIVE_THREAD_ID: Cell<Option<ThreadId>> = const { Cell::new(None) };

    /// The raw `rdram` pointer for whichever coroutine is currently being
    /// resumed. Needed because `osCreateThread_recomp`'s real dispatch
    /// (this wave) must call the resolved `RecompFunc` with the SAME
    /// `rdram` pointer the whole process shares (`docs/DESIGN.md` section
    /// 3: "one shared buffer... passed by reference to everyone") -- a
    /// spawned thread's body closure has no other way to obtain it, since
    /// it does not itself receive `rdram` as a parameter (only the
    /// `_recomp` shim that called `osCreateThread_recomp` did). Installed/
    /// restored alongside `ACTIVE_YIELDER`/`ACTIVE_THREAD_ID` by the same
    /// `with_active_yielder` call.
    static ACTIVE_RDRAM: Cell<*mut u8> = const { Cell::new(std::ptr::null_mut()) };
}

/// A registered thread's `(Yielder, rdram)` pair -- see `THREAD_CONTEXTS`.
type ThreadContext = (*const Yielder<Resume, Yield>, *mut u8);

thread_local! {
    /// Per-thread `(Yielder, rdram)` registry -- see `run_one_step`'s doc
    /// comment for the bug this closes (2026-07-14): `with_active_yielder`
    /// only ever runs ONCE per thread, wrapping that thread's entire body
    /// closure, so it correctly arms `ACTIVE_YIELDER`/`ACTIVE_THREAD_ID`/
    /// `ACTIVE_RDRAM` for that thread's FIRST run segment -- but every
    /// `GameThread` coroutine shares this same native OS thread's
    /// thread-locals, and a suspended thread's own restore-to-`previous`
    /// code cannot run again until its body genuinely returns (the thread
    /// dies). So the moment a SECOND thread starts (or any already-started
    /// thread is resumed after some OTHER thread most recently ran), the
    /// active cells are stale, and a `_recomp` shim on the wrong/no
    /// coroutine's native stack can call `Yielder::suspend` on a `Yielder`
    /// that does not belong to the stack currently executing -- corrupting
    /// that other coroutine's saved resume context (the OoT Main-resume
    /// SIGBUS at PC=0x1: `fn64-diff`'s first-divergence report). This
    /// registry lets `run_one_step` re-arm the ABOUT-TO-BE-RESUMED thread's
    /// own `(Yielder, rdram)` immediately before every single resume, not
    /// just the first. Entries are inserted once (at thread creation, by
    /// `with_active_yielder`) and never removed -- a `Yielder` pointer
    /// stays valid for its coroutine's entire lifetime (the coroutine's
    /// native stack the pointer refers into isn't freed until the
    /// `GameThread`/`Coroutine` itself is dropped, which outlives every
    /// `run_one_step` call this registry is consulted from), and a dead
    /// thread is never picked by `peek_next_thread`/`pick_next` again, so a
    /// stale entry for a dead thread is simply never looked up.
    static THREAD_CONTEXTS: RefCell<std::collections::HashMap<ThreadId, ThreadContext>> =
        RefCell::new(std::collections::HashMap::new());
}

/// THE single gateway to `EXECUTOR`. Every `_recomp` shim, every host-facing
/// helper, and every test in this crate that touches the executor goes
/// through this one function -- `EXECUTOR` itself is a private `thread_local`
/// with no other accessor, so "does some call site bypass the reentrancy
/// story below" is a closed question by construction, not a convention to
/// audit call-site-by-call-site.
///
/// ## Audit verdict: `ReentrantCell` is still required (2026-07-14)
///
/// The task this wave answers: `Yield`/`Resume` (`thread.rs`) already make
/// ONE reentrancy shape a compile-time non-issue -- a coroutine can never
/// directly call back into `Executor::run_one_step`'s own resume loop,
/// because the only handle that could drive a second resume (`RunToken`) is
/// non-`Copy`, privately constructed, and issued exactly once per
/// `run_one_step` call (`thread.rs`'s `RunToken` doc comment). That is a
/// *scheduling* reentrancy guarantee: no second `GameThread::resume` can ever
/// be invoked while a first is on the stack.
///
/// `ReentrantCell` guards a DIFFERENT, narrower case that the type-level
/// guarantee above does not and cannot cover, because it isn't a resume at
/// all -- it's an ordinary nested function call:
///
/// - `Executor::run_one_step` calls `with_executor(|exec| ...)` is not
///   literally true -- rather, `fn64-abi`'s own top-level `run_one_step`
///   helper (below) calls `with_executor(|exec| exec.run_one_step())`, so
///   `EXECUTOR`'s borrow is already open when `exec.run_one_step()` starts.
/// - `run_one_step` calls `GameThread::resume`, which runs the coroutine
///   body -- ordinary, synchronous, non-yielding Rust code -- until it
///   either returns or hits a real `Yielder::suspend` point.
/// - That coroutine body is a real recompiled `OSThread`'s entry point (or,
///   via `osCreateThread_recomp`, a THREAD IT ITSELF SPAWNS -- see the
///   `a_running_threads_own_body_can_call_os_create_thread_recomp_...` test),
///   which is free to call any other `_recomp` shim as an ordinary function
///   call with no suspend point at all -- `osCreateThread_recomp`,
///   `osSetEventMesg_recomp`, every VI setter, `osSetTimer_recomp`, etc. all
///   call `with_executor` themselves, synchronously, with no yield in
///   between.
///
/// This is the residual case: a **synchronous, non-yielding nested call**
/// into `with_executor` from code already running underneath an outer
/// `with_executor` call on the same native stack. `Yield`/`Resume` cannot
/// see this at all -- there is no suspend point here for either type to
/// govern; the coroutine body never calls `Yielder::suspend`, so from the
/// executor's/scheduler's point of view nothing about "which thread holds
/// the `RunToken`" changes mid-call. The hazard is purely about `&mut
/// Executor` aliasing on the borrow-checker's terms, not about two threads
/// or two resumes.
///
/// It is memory-safe despite looking like aliasing: the OUTER
/// `with_executor` closure (`run_one_step`'s own body, or `run_to_idle`'s
/// loop) does not read or write `Executor` state again until the INNER,
/// nested `with_executor` call returns -- the two "live" `&mut` references
/// are simultaneously IN SCOPE on the call stack but never simultaneously
/// DEREFERENCED. `RefCell`'s dynamic, stack-blind borrow tracking cannot
/// distinguish that from true concurrent aliasing (it panics the instant a
/// second `borrow_mut()` happens while the first is outstanding, regardless
/// of whether the first is actually being touched right now) -- which is
/// exactly the "already borrowed" panic `examples/wm2000-boot`'s boot
/// harness hit for real (recomp_entrypoint's very first real
/// `osCreateThread` call, made from inside `run_one_step`'s own resume).
///
/// ## Why this can't be funneled away structurally, only made minimal here
///
/// A stackless (async/Future) redesign could in principle make "the
/// coroutine body calls another shim synchronously" impossible by forcing
/// every shim call to be an awaited suspend point -- but `docs/DESIGN.md`
/// section 2 already rejected async for this exact workload (recompiled C's
/// call graph has no natural `.await` points). Short of that redesign, this
/// crate already does the two things option (a) of this wave's task asks
/// for: (1) there is exactly ONE gateway (`with_executor`, this function --
/// not "a documented convention," an structurally closed set, since
/// `EXECUTOR` has no other accessor) and (2) the residual dynamic case is
/// named precisely, right here, rather than left as a vague "reentrancy is
/// possible, be careful" note. `ReentrantCell` is that gateway's
/// implementation detail, not a second, parallel safety mechanism -- remove
/// it and this exact function would panic on the nested call this doc
/// comment describes, with no compile-time signal beforehand.
fn with_executor<R>(f: impl FnOnce(&mut Executor) -> R) -> R {
    EXECUTOR.with(|e| e.with(f))
}

fn with_host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// Install `yielder`/`thread_id`/`rdram` as the active ones for the
/// duration of `f`. See module doc.
///
/// Also registers `(yielder, rdram)` in `THREAD_CONTEXTS` under `thread_id`
/// -- this call only happens ONCE per thread (wrapping that thread's entire
/// body closure, from `osCreateThread_recomp`/`boot_thread0`/test helpers),
/// so this is the one place that ever learns a given thread's `Yielder`
/// pointer. `run_one_step` (below) is what re-arms `ACTIVE_YIELDER`/
/// `ACTIVE_THREAD_ID`/`ACTIVE_RDRAM` from this registry before every
/// subsequent resume -- see `THREAD_CONTEXTS`' doc comment for the bug this
/// closes.
pub fn with_active_yielder<R>(
    thread_id: ThreadId,
    rdram: *mut u8,
    yielder: &Yielder<Resume, Yield>,
    f: impl FnOnce() -> R,
) -> R {
    let ptr = yielder as *const Yielder<Resume, Yield>;
    THREAD_CONTEXTS.with(|cell| cell.borrow_mut().insert(thread_id, (ptr, rdram)));
    let previous_yielder = ACTIVE_YIELDER.with(|cell| cell.replace(Some(ptr)));
    let previous_id = ACTIVE_THREAD_ID.with(|cell| cell.replace(Some(thread_id)));
    let previous_rdram = ACTIVE_RDRAM.with(|cell| cell.replace(rdram));
    let result = f();
    ACTIVE_YIELDER.with(|cell| cell.set(previous_yielder));
    ACTIVE_THREAD_ID.with(|cell| cell.set(previous_id));
    ACTIVE_RDRAM.with(|cell| cell.set(previous_rdram));
    result
}

/// Re-arm `ACTIVE_YIELDER`/`ACTIVE_THREAD_ID`/`ACTIVE_RDRAM` to `thread_id`'s
/// own registered `(Yielder, rdram)` (from `THREAD_CONTEXTS`, populated once
/// by that thread's own `with_active_yielder` call at creation), run `f`,
/// then restore whatever was active before. This is THE fix for the
/// coroutine-context-corruption bug (see `THREAD_CONTEXTS`' doc comment):
/// every `GameThread::resume` must go through this so the thread actually
/// about to run always has ITS OWN context active, never a stale one left
/// over from whichever thread most recently ran.
///
/// If `thread_id` has no registered context yet (this run_one_step is about
/// to resume a thread's coroutine for the very first time, `Resume::Start`
/// -- that thread's OWN `with_active_yielder` call hasn't executed yet,
/// since it lives inside the coroutine body being resumed), this is a
/// no-op passthrough: the FIRST resume is exactly the case the original,
/// single `with_active_yielder` call (inside the coroutine body) already
/// handles correctly by itself.
fn with_rearmed_context<R>(thread_id: ThreadId, f: impl FnOnce() -> R) -> R {
    let registered = THREAD_CONTEXTS.with(|cell| cell.borrow().get(&thread_id).copied());
    let Some((ptr, rdram)) = registered else {
        return f();
    };
    let previous_yielder = ACTIVE_YIELDER.with(|cell| cell.replace(Some(ptr)));
    let previous_id = ACTIVE_THREAD_ID.with(|cell| cell.replace(Some(thread_id)));
    let previous_rdram = ACTIVE_RDRAM.with(|cell| cell.replace(rdram));
    let result = f();
    ACTIVE_YIELDER.with(|cell| cell.set(previous_yielder));
    ACTIVE_THREAD_ID.with(|cell| cell.set(previous_id));
    ACTIVE_RDRAM.with(|cell| cell.set(previous_rdram));
    result
}

/// The `ThreadId` of the coroutine currently executing a `_recomp` shim.
fn current_thread_id(shim: &str) -> ThreadId {
    ACTIVE_THREAD_ID.with(|cell| cell.get()).unwrap_or_else(|| {
        panic!(
            "{shim}: no active thread id installed -- this _recomp shim was called from \
             outside a resumed coroutine's body (see with_active_yielder)"
        )
    })
}

/// Suspend the currently-active coroutine with `yield_value`. Panics
/// loudly if called outside `with_active_yielder`'s scope.
fn suspend_active_coroutine(yield_value: Yield) -> Resume {
    let ptr = ACTIVE_YIELDER.with(|cell| cell.get()).unwrap_or_else(|| {
        panic!(
            "suspend_active_coroutine: no active Yielder installed -- this _recomp shim was \
             called from outside a resumed coroutine's body, so there is no coroutine stack to \
             suspend. This must panic loudly rather than silently continuing without yielding \
             (AGENTS.md's 'no silent shrugs'), since a silent continue here is exactly rung 14's \
             failure mode: code that should give up the CPU but doesn't."
        )
    });
    // Safety: see prior wave's identical note -- `ptr` is only ever
    // non-None for the dynamic extent of the installing `with_active_yielder`
    // call, on the same thread.
    let yielder = unsafe { &*ptr };
    yielder.suspend(yield_value)
}

// ---------------------------------------------------------------------
// Small shared helpers.
// ---------------------------------------------------------------------

impl RecompContext {
    /// An all-zero `RecompContext` -- used to seed a freshly-dispatched
    /// thread entry point's register state (`osCreateThread_recomp`).
    ///
    /// `f_odd` is left null here; it is a SELF-REFERENTIAL pointer into this
    /// same context's FPR file, so it can only be set once the context has a
    /// stable address. Every dispatch site MUST call `arm_fpr_alias()` on the
    /// context (at its final address, before running any recompiled function)
    /// -- see that method's doc comment.
    pub fn zeroed() -> Self {
        // Safety: RecompContext is a `#[repr(C)]` struct of plain integers
        // and one raw pointer, all of which are valid when all-zero (a
        // null pointer is a valid `*mut u32` bit pattern). `Fpr` is a
        // `#[repr(C)]` union of plain numeric types, likewise valid
        // zeroed.
        unsafe { std::mem::zeroed() }
    }

    /// Point `f_odd` at this context's own FPR file so recompiled `mtc1`/
    /// `sdc1`-to-odd-register stores land in-register instead of faulting.
    ///
    /// Generated C addresses an odd float register `$fN` (N odd) as
    /// `ctx->f_odd[(N-1)*2]`, treating `f_odd` as a `uint32_t*` cursor into
    /// the `fpr f0..f31` array. With FR=0 (`mips3_float_mode == 0`, the state
    /// libultra boots every OSThread in), the odd register's bits alias the
    /// HIGH 32-bit word of its even partner: for `$f9`, index `(9-1)*2 = 16`,
    /// byte `16*4 = 0x40` past `f_odd`, which must equal `&f8.u32h`. That
    /// holds exactly when `f_odd == &f0.u32h` (the fpr union's second u32,
    /// byte 4 of `f0`): `&f0.u32h + 0x40` == byte `0x44` == `f8`'s high word.
    /// This matches the `recomp.h` fpr layout (`{u32l, u32h}` at bytes 0/4,
    /// 8-byte stride) the generated C was emitted against.
    ///
    /// Was the OoT-boot SIGSEGV-at-0x40 root cause: `f_odd` stayed null from
    /// `zeroed()`, so `guLookAtHiliteF`'s first `mtc1 $at, $f9`
    /// (`ctx->f_odd[16] = ...`, funcs_57.c:4519) dereferenced null+0x40.
    ///
    /// # Safety
    /// The pointer aliases `self`; `self` must not move for as long as any
    /// recompiled code holds/uses this context (guaranteed at the dispatch
    /// sites, which build the context and immediately run the entry function
    /// with it, never relocating it mid-run).
    pub fn arm_fpr_alias(&mut self) {
        // `u32_halves.1` is the high 32-bit word of `f0` (byte offset 4),
        // i.e. recomp.h's `f0.u32h` -- the FR=0 base the index math above
        // requires.
        self.f_odd = unsafe { &mut self.f0.u32_halves.1 as *mut u32 };
    }
}

/// Read a 32-bit word from `rdram` at `base_gpr + stack_offset`, i.e. the
/// o32 stack-argument-area read every stack-passed 5th+ argument in this
/// file needs (`osCreateThread`'s `sp`/`pri`, `osSetTimer`'s
/// `interval`/`mq`/`msg`, `osEPiStartDma`'s `OSIoMesg` fields).
///
/// ## Correction (this wave): `MEM_W` is NATIVE-endian, not big-endian
///
/// A prior wave's doc comment here (and `fn64_runtime::Rdram::read_w`/
/// `write_w`'s identical assumption) claimed `MEM_W` performs an explicit
/// big-endian word access ("no byte-lane XOR... sign-extended"). This is
/// WRONG, first caught by `examples/wm2000-boot`'s actual boot run (a
/// spawned thread's real stack pointer, read via this function, came back
/// byte-swapped -- `0x70BE0480` instead of the correct `0x8004BE70`, an
/// exact little-endian/big-endian mirror of each other). Verified directly
/// against `recomp.h` (MIT, the ABI this crate serves) itself:
/// `#define MEM_W(offset, reg) (*(int32_t*)(rdram + ((reg)+(offset) -
/// 0xFFFFFFFF80000000)))` -- a PLAIN C POINTER DEREFERENCE, not a manual
/// byte-by-byte big-endian assembly. On any real (little-endian) host this
/// compiles to a native little-endian load/store. `MEM_H`/`MEM_B`'s
/// `^2`/`^3` byte-lane XOR exists PRECISELY BECAUSE word storage is
/// native-endian: XORing the sub-word offset is what makes a big-endian-CPU
/// address land on the correct byte within an otherwise little-endian-
/// stored word -- i.e. the WORD accessor was never byte-swapped to begin
/// with; only the SUB-WORD ones need the XOR correction, which only makes
/// sense as a correction against a native-endian backing store. This
/// crate's own `rdram.rs` module doc mistranscribed "ABI-SURFACE.md section
/// (c)" in a way that doesn't match `recomp.h`'s actual macro -- fixed for
/// real here (and in `fn64_runtime::Rdram`'s word accessors, this same
/// wave); every previously-"verified" claim of "no byte-lane XOR... sign-
/// extended" for `MEM_W` in this codebase's comments should be read as
/// "native host byte order" going forward, not "big-endian."
///
/// This is deliberately NOT `fn64_runtime::Rdram::read_w` -- that method
/// requires owning an `Rdram` instance, but every `_recomp` shim only ever
/// borrows the raw `rdram` pointer generated C hands it (`docs/DESIGN.md`
/// section 3: "one shared buffer... borrowed... never owned"), so this
/// helper replicates `MEM_W`'s REAL semantics (word-aligned, native host
/// byte order, no byte-lane XOR at word granularity) directly against the
/// raw pointer.
///
/// # Safety
/// `rdram` must be a valid pointer to at least `base_gpr + stack_offset +
/// 4` bytes, per every shim's own contract in this file.
unsafe fn read_stack_word(rdram: *mut u8, base_gpr: u64, stack_offset: u32) -> u32 {
    let addr = RdramAddr::from_gpr(base_gpr.wrapping_add(stack_offset as u64));
    let o = addr.offset() as usize;
    let mut bytes = [0u8; 4];
    unsafe {
        std::ptr::copy_nonoverlapping(rdram.add(o), bytes.as_mut_ptr(), 4);
    }
    u32::from_ne_bytes(bytes)
}

/// Read a 32-bit word from `rdram` at `base_offset + extra_offset`, where
/// `base_offset` is an ALREADY-resolved rdram-relative byte offset (e.g.
/// `RdramAddr::offset()`'s return value) -- NOT a raw vram/gpr value.
/// Deliberately a DIFFERENT function from `read_stack_word` (which takes a
/// raw `gpr`/vram value and performs the KSEG0 translation itself), so a
/// caller that already has a `RdramAddr` cannot accidentally re-apply the
/// KSEG0 subtraction a second time -- see `osEPiStartDma_recomp`'s doc
/// comment for the real double-translation bug this distinction fixes.
///
/// # Safety
/// `rdram` must be a valid pointer to at least `base_offset + extra_offset
/// + 4` bytes.
unsafe fn read_offset_word(rdram: *mut u8, base_offset: u32, extra_offset: u32) -> u32 {
    let o = (base_offset + extra_offset) as usize;
    let mut bytes = [0u8; 4];
    unsafe {
        std::ptr::copy_nonoverlapping(rdram.add(o), bytes.as_mut_ptr(), 4);
    }
    u32::from_ne_bytes(bytes)
}

mod ai;
mod cache;
mod dispatch;
mod host;
mod mesgqueue;
mod pi;
mod si;
mod softmath;
mod sp_dp;
mod system;
mod task_dispatch;
mod thread;
mod timer;
mod vi;

pub use ai::*;
pub use cache::*;
pub use dispatch::*;
pub use host::*;
pub use mesgqueue::*;
pub use pi::*;
pub use si::*;
pub use softmath::*;
pub use sp_dp::*;
pub use system::*;
pub use task_dispatch::*;
pub use thread::*;
pub use timer::*;
pub use vi::*;

#[cfg(feature = "native-recomp")]
pub(crate) use system::INT_MASK;

#[cfg(test)]
mod test_support;
