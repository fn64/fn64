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
use fn64_audio::{AudioBackend, AudioError};
use fn64_render::RenderBackend;
use fn64_runtime::{
    DmaDirection, Executor, ExternalEvent, InMemoryRom, Mesg, OsTaskHeader, PiDma, Priority,
    RdramAddr, Resume, Section, SectionRegistry, ThreadId, Yield, M_AUDTASK, M_GFXTASK,
};

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
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            sections: SectionRegistry::new(),
            pi_dma: None,
            thread_handles: std::collections::HashMap::new(),
            timer_handles: std::collections::HashMap::new(),
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
// recomp.h dispatch helpers: get_function / switch_error / do_break /
// pause_self. Real signatures per module doc.
// ---------------------------------------------------------------------

/// `pause_self(uint8_t *rdram)` -- ONE argument, no `ctx` (see module doc).
#[no_mangle]
pub extern "C" fn pause_self(_rdram: *mut u8) {
    suspend_active_coroutine(Yield::PauseSelf);
}

/// `switch_error(const char* func, uint32_t vram, uint32_t jtbl)` --
/// N64Recomp's codegen'd jump-table-miss trap (M1-WORKLIST.md #2, 214 NWXE
/// call sites). This is, by design, ALWAYS a loud trap: real generated code
/// only reaches this when a `jr`-based switch's computed target matched no
/// known case, which is either a genuine bug in the recompiled program or
/// (far more likely at this project's current stage) a section not yet
/// registered/loaded in `SectionRegistry` -- either way, per `AGENTS.md`,
/// this must never return normally with a guessed fallback.
///
/// # Safety
/// `func` must be a valid nul-terminated C string pointer, as every real
/// call site (`switch_error(__func__, vram, jtbl)`) provides via the
/// compiler's own `__func__`.
#[no_mangle]
pub unsafe extern "C" fn switch_error(func: *const std::os::raw::c_char, vram: u32, jtbl: u32) {
    let func_name = if func.is_null() {
        "<null>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(func) }
            .to_string_lossy()
            .into_owned()
    };
    panic!(
        "switch_error: jump-table dispatch in {func_name} found no matching case for vram \
         {vram:#010x} (jump table at {jtbl:#010x}) -- this is N64Recomp's own generated \
         safety net firing, meaning either a genuine bug in the recompiled program or (more \
         likely at this project's stage) this address's target was never correctly resolved. \
         Per AGENTS.md, this is a loud trap by design, never a silently-guessed fallback."
    );
}

/// `do_break(uint32_t vram)` -- N64Recomp's codegen'd `break`/assert trap
/// (M1-WORKLIST.md #3, 253 NWXE call sites).
#[no_mangle]
pub extern "C" fn do_break(vram: u32) {
    panic!(
        "do_break: recompiled MIPS `break`/undefined-instruction trap fired at vram \
         {vram:#010x} -- this is N64Recomp's own generated safety net, not a silent no-op \
         per AGENTS.md."
    );
}

/// `get_function(int32_t vram) -> recomp_func_t*` -- the overlay/section
/// resolver every `LOOKUP_FUNC` call site depends on (M1-WORKLIST.md #1,
/// 85 NWXE call sites via `LOOKUP_FUNC`). Backed by `SectionRegistry`,
/// registered via `register_section`/`set_section_loaded` below (this
/// crate's own public API, called by `fn64-shell`/tests once at startup
/// from the game's `recomp_overlays.inl`-derived data -- `fn64-runtime`
/// itself has no `.inl`-parsing knowledge, per `docs/DESIGN.md` section 1's
/// crate split).
#[no_mangle]
pub extern "C" fn get_function(vram: i32) -> *const () {
    with_host(|host| host.sections.resolve(vram as u32) as *const ())
}

/// Register a section's `FuncEntry` table with the overlay registry, in the
/// exact shape `ABI-SURFACE.md` section (d) documents (`rom_addr`/
/// `ram_addr`/`size` plus a `(offset, func)` list) -- called once per
/// generated `SectionTableEntry`, in the same order the generated
/// `.inl`'s `section_table[]` declares them, so the returned
/// `SectionIndex` matches `SectionTableEntry.index`.
///
/// # Safety
/// Every `func` pointer must be a valid `RecompFunc` for the lifetime of
/// the process (true for every `FuncEntry.func` in generated C, which are
/// all file-scope `RECOMP_FUNC` definitions with static storage duration).
pub unsafe fn register_section(
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    funcs: &[(u32, u32, RecompFunc)],
) -> fn64_runtime::SectionIndex {
    let entries = funcs
        .iter()
        .map(|&(offset, rom_size, func)| fn64_runtime::FuncEntry {
            func_ptr: func as usize,
            offset,
            rom_size,
        })
        .collect();
    with_host(|host| {
        host.sections.register_section(Section {
            rom_addr,
            ram_addr,
            size,
            funcs: entries,
        })
    })
}

pub fn set_section_loaded(index: fn64_runtime::SectionIndex) {
    with_host(|host| host.sections.set_section_loaded(index));
}

pub fn set_section_unloaded(index: fn64_runtime::SectionIndex) {
    with_host(|host| host.sections.set_section_unloaded(index));
}

/// Install the real ROM bytes the PI/EPI DMA shims read from. Must be
/// called once before any `osEPiStartDma_recomp`/`osCartRomInit_recomp`
/// call, per `README.md`'s "no game content ships in this repo" rule --
/// `fn64-shell` supplies the user's own loaded ROM file's bytes here.
pub fn load_rom(bytes: Vec<u8>) {
    with_host(|host| host.pi_dma = Some(PiDma::new(InMemoryRom::new(bytes))));
}

fn with_pi_dma<R>(shim: &str, f: impl FnOnce(&mut PiDma<InMemoryRom>) -> R) -> R {
    with_host(|host| {
        let dma = host.pi_dma.as_mut().unwrap_or_else(|| {
            panic!(
                "{shim}: no ROM installed -- call fn64_abi::load_rom(bytes) before any PI/EPI \
                 DMA shim runs (see that function's doc comment; this crate never ships game \
                 content, so there is no default ROM to fall back to)"
            )
        });
        f(dma)
    })
}

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
    let id: ThreadId = ctx.r5 as u32;
    let entry_vram = ctx.r6 as u32;
    let arg = ctx.r7;
    let sp = read_stack_word(rdram, ctx.r29, 0x10) as u64;
    let priority = read_stack_word(rdram, ctx.r29, 0x14) as Priority;

    with_host(|host| {
        host.thread_handles.insert(thread_handle.offset(), id);
    });

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
                let func_ptr = get_function(entry_vram as i32);
                let entry: RecompFunc = unsafe { std::mem::transmute(func_ptr) };
                let mut entry_ctx = RecompContext::zeroed();
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

// ---------------------------------------------------------------------
// Message queue: osCreateMesgQueue / osSendMesg / osRecvMesg /
// osSetEventMesg. Unchanged behavior from prior wave, RecompContext
// fields renamed to the full struct (still r4/r5/r6, same o32 slots).
// ---------------------------------------------------------------------

/// `osCreateMesgQueue(OSMesgQueue *mq, OSMesg *msg, s32 count)`.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext`.
#[no_mangle]
pub unsafe extern "C" fn osCreateMesgQueue_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let count = ctx.r6 as usize;
    with_executor(|exec| exec.create_mesg_queue(mq_addr, count.max(1)));
}

/// `osSendMesg(OSMesgQueue *mq, OSMesg msg, s32 flag)`.
///
/// # Safety
/// Same contract as `osCreateMesgQueue_recomp`.
#[no_mangle]
pub unsafe extern "C" fn osSendMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    let may_block = ctx.r6 == OS_MESG_BLOCK;
    if std::env::var("FN64_DEBUG_SEND").is_ok() {
        let tid = ACTIVE_THREAD_ID.with(|c| c.get());
        eprintln!(
            "[DEBUG osSendMesg_recomp] active_thread={tid:?} mq_offset={:#x} msg={msg:#x} \
             may_block={may_block} r29(sp)={:#x}",
            mq_addr.offset(),
            ctx.r29
        );
    }

    match suspend_active_coroutine(Yield::BlockOnSend {
        mq_addr,
        msg,
        may_block,
    }) {
        Resume::SendUnblocked | Resume::WouldBlock => {}
        other => panic!(
            "osSendMesg_recomp: resumed from a BlockOnSend yield with an unexpected Resume \
             variant {other:?}"
        ),
    }
}

/// `OS_MESG_NOBLOCK`/`OS_MESG_BLOCK`, per the public libultra manual.
#[allow(dead_code)]
const OS_MESG_NOBLOCK: u64 = 0;
const OS_MESG_BLOCK: u64 = 1;

/// `osRecvMesg(OSMesgQueue *mq, OSMesg *msg, s32 flag)`.
///
/// # Safety
/// `rdram`/`ctx` must be valid per the same contract as every other shim.
#[no_mangle]
pub unsafe extern "C" fn osRecvMesg_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    // Correction (2026-07-14): check the RAW register for null, not the
    // translated `RdramAddr`. A real `msg == NULL` call (public libultra
    // manual's documented "pass NULL to just wait, discarding the message"
    // form -- OoT's own boot path uses exactly this: `DmaMgr_DmaRomToRam`'s
    // real call site, `games/OOTU/RecompiledFuncs/funcs_0.c:975-980`, is
    // `osRecvMesg(s6, a1=0, 1)`) has `ctx.r5 == 0`, but
    // `RdramAddr::from_gpr(0)` computes `0u64.wrapping_sub(0xFFFFFFFF80000000)`,
    // which is `0x8000_0000`, NOT `0` -- the OLD `msg_out_addr.offset() !=
    // 0` guard below never actually caught a real null pointer, so this
    // shim wrote the delivered message to rdram OFFSET 0x8000_0000 (a real
    // out-of-bounds write for any buffer smaller than that, and a silent
    // wrong-address write even for large enough buffers) every time a real
    // ROM legitimately passed NULL. First caught by `examples/oot-boot`'s
    // real boot run (`osRecvMesg_recomp` SIGSEGV at rdram offset
    // 0x8000_0000-ish inside `DmaMgr_Init`, thread 1's very first blocking
    // receive). Fixed by testing the UNTRANSLATED register value for zero.
    let msg_out_is_null = ctx.r5 == 0;
    let msg_out_addr = RdramAddr::from_gpr(ctx.r5);
    let may_block = ctx.r6 == OS_MESG_BLOCK;

    let delivered = match suspend_active_coroutine(Yield::BlockOnRecv { mq_addr, may_block }) {
        Resume::Delivered(msg) => Some(msg),
        Resume::WouldBlock => None,
        other => panic!(
            "osRecvMesg_recomp: resumed from a BlockOnRecv yield with an unexpected Resume \
             variant {other:?}"
        ),
    };

    if let Some(msg) = delivered {
        if !msg_out_is_null {
            let o = msg_out_addr.offset() as usize;
            // Native byte order, matching MEM_W's real semantics -- see
            // `read_stack_word`'s doc comment for the full correction this
            // wave made (a prior wave's big-endian assumption was wrong).
            unsafe {
                std::ptr::copy_nonoverlapping((msg as i32).to_ne_bytes().as_ptr(), rdram.add(o), 4);
            }
        }
    }
}

/// `osSetEventMesg(OSEvent event, OSMesgQueue *mq, OSMesg msg)`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetEventMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let event = ctx.r4 as u32;
    let mq_addr = RdramAddr::from_gpr(ctx.r5);
    let msg: Mesg = ctx.r6 as u32;
    with_executor(|exec| exec.set_event_mesg(event, mq_addr, msg));
}

/// `osSetTimer(OSTimer *t, OSTime countdown, OSTime interval, OSMesgQueue
/// *mq, OSMesg msg)` -- M1-WORKLIST.md #23, T2, "verify empirically before
/// trusting without a donor cite" per the ladder. `t`=r4 (unused, same
/// OSId-style non-issue as thread shims -- this crate has no per-`OSTimer`
/// struct state, `TimerWheel::set_timer` returns its own `TimerId`, and no
/// shim here yet needs to map a `t` address back to that id, since nothing
/// calls `osStopTimer_recomp` in this milestone's undefined-symbol set).
/// `countdown`=r6 (low word; OSTime's real 64-bit range is not exercised by
/// this milestone's boot-rung evidence, which only cited a single-timer
/// role-match with no byte-donor confirmation -- treating it as a plain
/// 32-bit virtual-tick count is the honest, undecorated reading of what the
/// call site actually passes, not an invented 64-bit reconstruction with no
/// evidence behind it), `interval`/`mq`/`msg` stack-passed at
/// `sp+0x10/0x18/0x1C` (verified against the real call site in
/// `funcs_13.c`: `MEM_W(0X10,...)`, `MEM_W(0X18,...)`, `MEM_W(0X1C,...)`
/// immediately preceding the `jal`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetTimer_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let timer_handle = RdramAddr::from_gpr(ctx.r4);
    let countdown = ctx.r6;
    let interval = read_stack_word(rdram, ctx.r29, 0x10) as u64;
    let mq_addr = RdramAddr::from_gpr(read_stack_word(rdram, ctx.r29, 0x18) as u64);
    let msg = read_stack_word(rdram, ctx.r29, 0x1C) as Mesg;
    let armed_by = current_thread_id("osSetTimer_recomp");
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

// ---------------------------------------------------------------------
// PI/ROM seam: osCartRomInit / osEPiStartDma / osVirtualToPhysical /
// osCreatePiManager / __osSiRawStartDma / osSetIntMask / osInitialize /
// osAiSetFrequency / osSpTaskYielded.
// ---------------------------------------------------------------------

/// `osCartRomInit(void) -> OSPiHandle*` -- no arguments (verified: every
/// real call site is `osCartRomInit_recomp(rdram, ctx)` with no register
/// setup beforehand). Rung 10b's exact fix target: a valid handle must
/// exist before the first real `osEPiStartDma`. This crate has no
/// `OSPiHandle` struct of its own (PI-DMA shims below identify the ROM by
/// "the one installed via `load_rom`," not by a handle pointer this
/// function would return) -- so this is a real, tested no-op beyond
/// asserting a ROM is installed, matching the documented one-time
/// bring-up role without inventing handle-struct fields no shim in this
/// milestone's undefined set actually consumes.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCartRomInit_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    with_pi_dma("osCartRomInit_recomp", |_dma| {});
}

/// `osEPiStartDma(OSPiHandle *handle, OSIoMesg *mb, s32 direction)` --
/// `a0`=handle (`ctx->r4`, unused per `osCartRomInit_recomp`'s doc comment),
/// `a1`=mb (`ctx->r5`, an `OSIoMesg*` -- `hdr.pri`/`hdr.retQueue`/`hdr.retMesg`
/// (queue to post completion to) at offsets documented in the libultra
/// manual's `OSIoMesg` layout, plus `dramAddr`/`devAddr`/`size`), `a2`=
/// direction (`ctx->r6`, `OS_READ`=0/`OS_WRITE`=1 per the public manual).
///
/// This crate's `OSIoMesg` field-offset assumptions (standard o32 struct
/// layout: `dramAddr` at +0x8, `devAddr` at +0xC, `size` at +0x10 -- the
/// documented libultra `OSIoMesg` shape after its `OSMesgHdr` header) are
/// NOT yet byte-verified against a real ROM's struct-init call site in this
/// milestone (no rung has isolated the exact offsets); this is flagged
/// honestly here rather than asserted as settled, per `AGENTS.md`'s
/// "prefer 'not verified' over a false 'done.'" The DMA completion posts
/// through `Executor::inject_event(DirectPost)` -- the same "ONE explicit
/// host-side injection point" every other completion source uses
/// (`docs/DESIGN.md` section 2).
///
/// ## Correction (2026-07-14): must set `ctx.r2` (the `$v0` return value)
///
/// A prior wave never wrote a return value at all, leaving `ctx.r2` at
/// whatever stale value the caller's own earlier computation left there.
/// Real `osEPiStartDma` returns `s32`: 0 on successful enqueue, -1 if
/// `!__osPiDevMgr.active` (byte-identical shape confirmed against WCW
/// Revenge's `func_800219B0`,
/// `aki-recomp/refs/WCWnWoRevengeRecomp/disasm/libultra.md` ~line 213).
/// `examples/wm2000-boot`'s real boot run surfaced the consequence: the
/// chunked-DMA loop in NWXE's `func_80000660`
/// (`aki-recomp/games/NWXE/RecompiledFuncs/funcs_0.c`, asm
/// 0x800006E4-0x800006FC) re-issues `osEPiStartDma` while `$v0 != 0` and
/// only falls through to a blocking `osRecvMesg` once `$v0` reads exactly
/// 0 -- with `ctx.r2` never written, that test read garbage left over from
/// an earlier instruction (observed non-zero), so the loop re-issued the
/// same DMA chunk forever: a real, tens-of-seconds unbounded native loop,
/// not a missing host model. This shim performs every DMA synchronously
/// and has no failure path today (`with_pi_dma` panics rather than
/// returning -1 when no ROM is installed, and `FromRdram` is an explicit
/// `unimplemented!()`), so every path that reaches the end of this
/// function represents success -- `ctx.r2 = 0` unconditionally there. A
/// real `-1` return only matters if/when this shim grows genuine
/// asynchronous PI-bus contention modeling, out of scope this wave.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mb_addr = RdramAddr::from_gpr(ctx.r5);
    let direction = if ctx.r6 == 0 {
        DmaDirection::ToRdram
    } else {
        DmaDirection::FromRdram
    };

    // OSIoMesg layout (libultra manual, public documentation): OSMesgHdr
    // (pri: s32, retQueue: OSMesgQueue*, retMesg: OSMesg) occupies the
    // first 3 words (+0x0/+0x4/+0x8 on o32), followed by dramAddr (+0xC),
    // devAddr (+0x10), size (+0x14). NOT byte-donor-verified this wave --
    // see doc comment above.
    //
    // Correction (this wave): a prior wave called `read_stack_word` (which
    // itself calls `RdramAddr::from_gpr`, subtracting the KSEG0 base) with
    // `mb_addr.offset()` -- an ALREADY-rdram-relative offset (KSEG0 already
    // subtracted once, on line computing `mb_addr` above). Subtracting
    // KSEG0 a SECOND time produced a wildly wrong address, first caught by
    // `examples/wm2000-boot`'s actual boot run (a real EXC_BAD_ACCESS deep
    // in this function once boot finally reached its first real PI DMA,
    // thread 6's `func_800222D8` -> ... -> `osEPiStartDma_recomp` call
    // chain). Fixed via `read_offset_word` (below), a sibling helper that
    // takes an ALREADY-resolved rdram offset and does no further KSEG0
    // translation -- the two helpers now have distinct names specifically
    // so this class of double-translation mistake doesn't recur silently at
    // a future call site (per `AGENTS.md`'s "mechanism over patch": fixing
    // just this one call site without a differently-named sibling helper
    // would leave the same trap for the next `RdramAddr`-holding caller).
    let ret_queue = read_offset_word(rdram, mb_addr.offset(), 0x4);
    let ret_mesg = read_offset_word(rdram, mb_addr.offset(), 0x8);
    // dramAddr is a raw vram POINTER the game computed the normal way (e.g.
    // `&someBuffer`), same as any other vram value -- it needs the SAME
    // KSEG0 translation `RdramAddr::from_gpr` performs, not
    // `RdramAddr::from_offset` (which assumes the value is ALREADY an
    // rdram-relative offset with no translation needed). Using
    // `from_offset` here was a real bug (this field's value is a raw vram
    // address like any other, not a pre-resolved offset) -- caught by this
    // wave's own regression test after the sibling double-translation bug
    // (see the correction note above `read_offset_word`'s introduction).
    let dram_addr = RdramAddr::from_gpr(read_offset_word(rdram, mb_addr.offset(), 0xC) as u64);
    let dev_addr = read_offset_word(rdram, mb_addr.offset(), 0x10);
    let len = read_offset_word(rdram, mb_addr.offset(), 0x14);

    let completion = {
        let mut rt_rdram = fn64_runtime::Rdram::new(0);
        // Safety: fn64-abi does not own a fn64_runtime::Rdram wrapper (the
        // raw `rdram` pointer IS the shared buffer, per docs/DESIGN.md
        // section 3) -- construct a zero-length placeholder and instead
        // perform the copy directly against the raw pointer below, mirroring
        // osRecvMesg_recomp's existing pattern of not creating a second,
        // competing Rdram instance over borrowed memory.
        let _ = &mut rt_rdram;
        with_pi_dma("osEPiStartDma_recomp", |dma| match direction {
            DmaDirection::ToRdram => {
                let mut buf = vec![0u8; len as usize];
                dma.read_rom_bytes(dev_addr, &mut buf);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buf.as_ptr(),
                        rdram.add(dram_addr.offset() as usize),
                        buf.len(),
                    );
                }
                fn64_runtime::DmaCompletion {
                    direction,
                    dram_addr,
                    dev_addr,
                    len,
                }
            }
            DmaDirection::FromRdram => {
                unimplemented!(
                    "osEPiStartDma_recomp: OS_WRITE direction (cartridge domain write) has no \
                     backing store in this milestone -- see PiDma::start_dma's doc comment."
                );
            }
        })
    };

    if ret_queue != 0 {
        // retQueue (OSMesgHdr's OSMesgQueue*) is likewise a raw vram
        // pointer, same correction as dramAddr above -- from_gpr, not
        // from_offset.
        with_executor(|exec| {
            exec.inject_event(ExternalEvent::DirectPost {
                queue_addr: RdramAddr::from_gpr(ret_queue as u64),
                msg: ret_mesg,
            })
        });
    }
    let _ = completion;

    // Every path reaching here completed the DMA synchronously and
    // successfully -- see the doc comment's "Correction (2026-07-14)" for
    // why this must be written at all (a stale, unwritten $v0 caused a
    // real infinite retry loop in NWXE's chunked-DMA caller).
    ctx.r2 = 0;
}

/// `osVirtualToPhysical(void* vaddr) -> u32` -- KSEG0/1 virtual-to-physical
/// translation (M1-WORKLIST.md #15, highest call count in the whole
/// undefined set at 104x). Per the public libultra manual: for KSEG0/KSEG1
/// addresses (the only kind generated code passes -- MIPS o32 KSEG0 base
/// `0x80000000`/KSEG1 base `0xA0000000`), physical address is simply the
/// virtual address with the top 3 bits masked off (`vaddr & 0x1FFFFFFF`) --
/// documented, standard MIPS32 segment-translation arithmetic, not a
/// runtime-specific behavior. Returns the result in `ctx->r2` (`$v0`, the
/// o32 single-word return-value register).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osVirtualToPhysical_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let vaddr = ctx.r4 as u32;
    ctx.r2 = (vaddr & 0x1FFF_FFFF) as u64;
}

/// `osCreatePiManager(OSPri pri, OSMesgQueue *cmdQ, OSMesg *cmdBuf, s32
/// cmdMsgCnt)` -- spins up the PI-manager thread. Per `docs/DESIGN.md`
/// section 2's stackful-coroutine model, "the PI manager" is not a second
/// host thread in this design (there is exactly one executor thread) --
/// its role (serializing `osEPiStartDma` requests onto the single PI bus,
/// posting completions) is already what `osEPiStartDma_recomp` above does
/// directly and synchronously (module doc's "async-looking API" note in
/// `rom.rs`). This shim's real, tested effect is therefore just
/// registering `cmdQ` as a genuine `MesgQueue` (so a real ROM's own
/// `osSendMesg`/`osRecvMesg` calls against it, if any, behave correctly),
/// matching the one piece of `osCreatePiManager`'s documented contract this
/// milestone's evidence (rung 9) actually needs: a real, non-garbage
/// message queue existing at `cmdQ`'s address.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreatePiManager_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let cmd_q = RdramAddr::from_gpr(ctx.r5);
    let cmd_msg_cnt = ctx.r7 as usize;
    with_executor(|exec| exec.create_mesg_queue(cmd_q, cmd_msg_cnt.max(1)));
}

/// `__osSiRawStartDma(s32 direction, u8* dramAddr)` -- `a0`=direction
/// (`ctx->r4`; per the public libultra manual and this milestone's real
/// call-site evidence below, `1` = "write this PIF command block from rdram
/// TO the PIF, execute it, and read the response back into the same
/// buffer" -- the SI-manager's synchronous raw-transfer primitive
/// underlying `osContStartQuery`/`osContStartReadData`), `a1`=dramAddr
/// (`ctx->r5`, the PIF command-block buffer's rdram address).
///
/// ## What this really does (this wave, replacing the prior loud trap)
///
/// A real call site (`funcs_15.c` asm 0x80036040-0x80036064, the function
/// this milestone's evidence shows builds a controller-probe PIF block)
/// writes a standard libultra PIF-RAM command block into the buffer before
/// calling this: byte 0 = tx-size (`0xFF` = end-of-block marker observed at
/// offsets 0x26/final), each channel's header is
/// `[tx_size, rx_size, cmd, ...tx_bytes]` followed by `rx_size` response
/// bytes to fill in -- the public libultra manual's documented PIF-RAM
/// protocol (`osContStartQuery`'s `0x01,0x03` 3-byte-tx-then-3-byte-rx
/// status-query shape, `osContStartReadData`'s 1-byte-tx/4-byte-rx
/// read-data shape). This function walks channels 0-3 in that documented
/// format, filling each channel's response bytes from `PifModel`
/// (`fn64_runtime::si` -- "one standard controller on port 0, no pak, ports
/// 1-3 absent," per the task's explicit scope) rather than a fabricated
/// byte pattern, and stops at the first `0xFF` tx-size byte (the documented
/// end-of-block marker) or buffer exhaustion.
///
/// Completion is posted through `OS_EVENT_SI` (5, per the public libultra
/// manual's event-code table) via the SAME `Executor::inject_event` path
/// every other completion source uses -- matching `docs/DESIGN.md`
/// section 2's "closing the asymmetry" design point. If no
/// `osSetEventMesg(5, ...)` registration exists yet (this call happening
/// before the game registers its SI event), the post is silently absent
/// (mirrors `advance_time`'s VI-retrace handling of the same
/// not-yet-registered case) rather than panicking -- the DMA itself still
/// completes and the response bytes are still written, matching real
/// hardware where the SI interrupt fires regardless of whether software
/// has hooked it yet.
///
/// Real-hardware commands this milestone's `PifModel` does NOT model
/// (EEPROM/mempak read-write commands, reset) are represented as
/// `CONT_ABSENT`-shaped responses per channel walked, which is honest for
/// "no such device," not a guessed success.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSiRawStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let dram_addr = RdramAddr::from_gpr(ctx.r5);
    let base = dram_addr.offset() as usize;

    with_executor(|exec| {
        let pif = *exec.pif();
        let mut port = 0usize;
        let mut cursor = base;
        // Documented PIF-RAM command-block format: walk channel headers
        // until the 0xFF end-of-block marker or a bogus/oversized read that
        // would run off any sane buffer (64 bytes is PIF RAM's own real
        // hardware size, per public documentation -- used here only as a
        // runaway guard, not asserted as this buffer's actual allocation).
        for _ in 0..16 {
            let tx_size = unsafe { *rdram.add(cursor) };
            if tx_size == 0xFF {
                break;
            }
            let rx_size = unsafe { *rdram.add(cursor + 1) };
            if tx_size == 0 && rx_size == 0 {
                cursor += 1;
                continue;
            }
            let cmd = unsafe { *rdram.add(cursor + 2) };
            let rx_off = cursor + 2 + tx_size as usize;
            match (cmd, rx_size) {
                // osContStartQuery-shape: 1-byte tx (the 0xFF query command
                // itself is tx_size/cmd, not a separate byte in some
                // encodings; this crate matches on the documented 3-byte
                // status response regardless of the exact tx encoding
                // variant, since PifModel's response doesn't depend on it).
                (_, 3) => {
                    let resp = pif.query_response(port);
                    unsafe {
                        std::ptr::copy_nonoverlapping(resp.as_ptr(), rdram.add(rx_off), 3);
                    }
                }
                (_, 4) => {
                    let resp = pif.read_data_response(port);
                    unsafe {
                        std::ptr::copy_nonoverlapping(resp.as_ptr(), rdram.add(rx_off), 4);
                    }
                }
                _ => {
                    // Unmodeled command shape for this milestone (see doc
                    // comment) -- leave whatever bytes were already there
                    // rather than fabricating a response with no documented
                    // basis.
                }
            }
            cursor = rx_off + rx_size as usize;
            port += 1;
        }
    });

    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        if exec.event_table_contains(OS_EVENT_SI) {
            exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
        }
    });
}

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
    static INT_MASK: Cell<u32> = const { Cell::new(0) };
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

/// `osAiSetFrequency(u32 frequency)` -- configures the audio DAC sample
/// rate. No audio backend exists in this crate yet (`fn64-rt64`/`fn64-shell`
/// own that, per `docs/DESIGN.md` section 1) -- stored as plain host state
/// so a future audio-out wave has a real value to read, rather than
/// discarded silently.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiSetFrequency_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    AI_FREQUENCY.with(|cell| cell.set(ctx.r4 as u32));
}

thread_local! {
    static AI_FREQUENCY: Cell<u32> = const { Cell::new(0) };
    /// AI/VI/PI/SI/SP/DP/MI hardware-register model (`fn64_runtime::mmio`).
    /// Backs both the shim-level `osAi*` family below AND (via
    /// `sync_mmio_into_rdram`, called from `boot_thread0`/before each
    /// coroutine resume) a raw guest `MEM_W` load at the same address --
    /// see `mmio.rs`'s module doc for the real crash
    /// (`docs/BOOT-NOTES-WM2000.md`) this closes.
    static MMIO: RefCell<fn64_runtime::MmioSpace> = RefCell::new(fn64_runtime::MmioSpace::new());
}

/// Write every modeled MMIO register's current value into `rdram`'s real
/// bytes, so a subsequent RAW guest load (not going through any
/// `osXxx_recomp` shim) observes it. Exposed for a harness
/// (`examples/wm2000-boot`) to call right after allocating a
/// `Rdram::new_with_mmio`-sized buffer and before/between coroutine resumes
/// -- see `fn64_runtime::mmio::MmioSpace::sync_into_rdram`'s doc comment
/// for exactly when this needs to be called (after any host mutation of
/// the model, e.g. right after this file's own `osAiSetNextBuffer_recomp`).
///
/// # Safety
/// `rdram` must point to a buffer of at least
/// `fn64_runtime::RDRAM_MMIO_WINDOW_END` bytes (i.e. allocated via
/// `Rdram::new_with_mmio`, not plain `Rdram::new`/a bare `Vec::new`).
pub unsafe fn sync_mmio_into_rdram(rdram: *mut u8) {
    MMIO.with(|cell| unsafe { cell.borrow_mut().sync_into_rdram(rdram) });
}

/// `osAiGetStatus() -> u32` -- no arguments; real hardware `AI_STATUS`
/// register read (`AI_STATUS_BUSY`/`AI_STATUS_FULL` bits, public libultra
/// manual's AI Manager section). Backed by `fn64_runtime::mmio::AiRegs`,
/// the same model a raw guest `MEM_W` at the register's real address reads
/// (see `MMIO`'s doc comment) -- this shim and a raw load return the SAME
/// value, since both go through `AiRegs::status`'s one-shot-busy logic
/// (this call also mutates the one-shot flag, exactly like a real register
/// read would consume the interrupt-pending latch).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetStatus_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let status = MMIO.with(|cell| cell.borrow_mut().ai.status());
    ctx.r2 = status as u64;
}

/// `osAiGetLength() -> u32` -- no arguments; real hardware `AI_LEN` register
/// read (bytes remaining in the current/last DMA). See `AiRegs::length`'s
/// doc comment for why this crate reports the full latched length rather
/// than a fabricated mid-drain value.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiGetLength_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let len = MMIO.with(|cell| cell.borrow().ai.length());
    ctx.r2 = len as u64;
}

/// `osAiSetNextBuffer(void *buf, u32 size) -> s32` -- `buf`=`ctx->r4` (an
/// rdram-relative vram pointer to the audio sample buffer), `size`=`ctx->r5`
/// (bytes). Real hardware effect: latches the DMA source/length and starts
/// the transfer; per the public libultra manual, returns 0 on success or
/// a negative error code if a DMA is already in progress and the queue is
/// full. This crate's DMA is synchronous-modeled (see `AiRegs::set_next_buffer`'s
/// doc comment: "DMA proceeds" stance, same as `rom.rs`'s PI DMA), so this
/// always succeeds (returns 0) -- no evidence yet of a call site needing the
/// error path.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osAiSetNextBuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let buf_addr = RdramAddr::from_gpr(ctx.r4).offset();
    let size = ctx.r5 as u32;
    MMIO.with(|cell| cell.borrow_mut().ai.set_next_buffer(buf_addr, size));
    ctx.r2 = 0;
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

/// `osGetThreadId(OSThread *t) -> OSId` -- `a0`=`ctx->r4`. Resolved via
/// `resolve_thread_arg` (same `NULL`-means-current-thread convention as
/// `osGetThreadPri_recomp`/`osSetThreadPri_recomp` -- the public libultra
/// manual documents the same `t == NULL` convention for this call too).
/// Since this crate's `ThreadId` IS the real `OSId` (see
/// `HostState::thread_handles`' doc comment: it maps `OSThread*` -> the
/// `OSId` a real `osCreateThread(t, id, ...)` call supplied), this is a
/// direct return of the resolved id, not a separate lookup table. Real
/// call sites: `games/OOTU/RecompiledFuncs/funcs_0.c:4152`, `funcs_56.c:643`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osGetThreadId_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let target = resolve_thread_arg(ctx.r4, "osGetThreadId_recomp");
    ctx.r2 = target as u64;
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
    ctx.r2 = with_executor(|exec| exec.sim_time());
}

/// `osSpTaskYielded(OSTask *task) -> s32` -- `a0`=`ctx->r4`, an `OSTask_t*`
/// vram pointer (real call site: `funcs_0.c` asm 0x800010AC,
/// `a0 = s1+0x10`, i.e. the embedded `OSTask_t` inside whatever wrapper
/// struct the game keeps its current task in; the caller only ever reads
/// this function's boolean-shaped return in `ctx->r2`, per `funcs_0.c` asm
/// 0x800010B4's `bnel $v0, $zero, ...`). Public libultra manual's documented
/// `OSTask_t` field layout (`type`@0x0/`flags`@0x4/`ucode_boot`@0x8/
/// `ucode_boot_size`@0xC/`ucode`@0x10/`ucode_size`@0x14/`ucode_data`@0x18/
/// `ucode_data_size`@0x1C/`dram_stack`@0x20/`dram_stack_size`@0x24/
/// `output_buff`@0x28/`data_ptr`@0x30/`data_size`@0x34) is used to read the
/// header for logging/counting (`Executor::submit_task`, this wave's real
/// implementation replacing the prior loud trap) and, for `M_AUDTASK`, to
/// actually CALL the translated audio ucode function per the task's
/// explicit scope.
///
/// ## Real semantics implemented this wave
///
/// GFX_TASK_NOTE: a graphics task (`M_GFXTASK`) is routed through the
/// single registered `dyn RenderBackend` (`set_render_backend`), per
/// `docs/DECOUPLING.md`'s renderer seam -- see `GFX_RENDER_NOTE` below at
/// the actual dispatch call site for the honest current state of what
/// backend is registered in practice (today: `fn64-render-rt64`'s headless
/// `ReferenceBackend` for tests/fixtures; a real RT64-backed backend is not
/// wired up yet, see that crate's module doc). If no backend is
/// registered at all, the task is still just recorded (trace + count) via
/// `Executor::submit_task`, same as before this wave -- this function
/// always sets `ctx.r2 = 1` (task complete, did not yield) so the caller's
/// `bnel` path proceeds as if the RSP finished the task, matching real
/// hardware's observable effect on the caller (task done, no further
/// action expected) regardless of whether a backend actually drew
/// anything.
///
/// AUDIO_TASK_NOTE: an audio task (`M_AUDTASK`) causes the translated `wm2000_audio_ucode` function (out-of-tree; see `examples/wm2000-boot`'s harness, which registers it via `set_audio_ucode_fn` below -- `fn64-abi` itself contains no game-derived ucode C, per `README.md`'s "no game content ships in this repo") to be REALLY CALLED with `(rdram, ucode_addr)`, matching `RSPRecomp`'s documented generated signature. Its `RspExitReason` return is not yet interpreted beyond "it ran"; the header is still recorded via `submit_task`.
///
/// UNKNOWN_TASK_NOTE: an unrecognized task type is recorded (so the trace/count still sees it) but not executed, and this function still sets `ctx.r2 = 1` (complete) -- the same "acknowledge, don't fabricate real hardware effects" stance as the gfx path, since this milestone has no evidence for any other task type on NWXE's boot path.
///
/// # Safety
/// `ctx`/`rdram` must be valid per every other shim's contract in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYielded_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };

    if header.task_type == M_AUDTASK {
        AUDIO_UCODE_FN.with(|cell| {
            if let Some(f) = cell.get() {
                // Safety: `set_audio_ucode_fn`'s doc comment is the
                // contract -- `f` must be the real translated ucode
                // function, matching RSPRecomp's documented
                // `(uint8_t* rdram, uint32_t ucode_addr) -> RspExitReason`
                // signature. `ucode` (offset 0x10 in the header) is the
                // ucode text's rdram-relative address per the same field
                // layout this function already reads.
                unsafe {
                    f(rdram, header.ucode);
                }
            }
            // No ucode function registered (e.g. a test, or the harness
            // hasn't wired it yet): the task is still recorded/counted
            // below, honestly reflecting "an audio task was submitted, but
            // this process never actually ran its ucode" rather than
            // silently pretending it did.
        });
        // AUDIO_BACKEND_NOTE: routes M_AUDTASK's SAMPLE-DELIVERY half
        // through the single registered `dyn AudioBackend`
        // (`set_audio_backend`), the fn64-audio crate's seam, symmetric
        // with `RENDER_BACKEND` below. This is a SEPARATE half from the
        // `AUDIO_UCODE_FN` call just above: whatever that call wrote into
        // `rdram` (real translated ucode output if one is registered, or
        // untouched bytes if not -- `AudioUcodeFn`'s real FFI signature has
        // no other return channel per RSPRecomp's generated code) is read
        // back out of the task's own declared AI DMA output buffer
        // (`output_buff`/`output_buff_size`, the same header fields the
        // gfx path already reads for its own output bounds) and forwarded
        // as interleaved i16 PCM. If no backend is registered, the task is
        // still recorded/counted below, same "no backend -> no fabricated
        // playback" honesty stance as the render seam.
        AUDIO_BACKEND.with(|cell| {
            if let Some(backend) = cell.borrow_mut().as_mut() {
                let rdram_len = AUDIO_RDRAM_LEN.with(|cell| cell.get());
                let render_end = unsafe { read_output_buff_size(rdram, o) };
                let start = header.output_buff as usize;
                let end = start + render_end as usize;
                let result = if rdram_len == 0 || end > rdram_len || !start.is_multiple_of(2) {
                    // No rdram length registered yet, or the task's
                    // declared buffer doesn't fit -- named "not ready"
                    // rather than an out-of-bounds read or a silent skip.
                    Err(AudioError::NotReady(
                        "audio output buffer bounds unavailable or misaligned",
                    ))
                } else {
                    let bytes =
                        unsafe { std::slice::from_raw_parts(rdram.add(start), end - start) };
                    let samples: Vec<i16> = bytes
                        .chunks_exact(2)
                        .map(|c| i16::from_ne_bytes([c[0], c[1]]))
                        .collect();
                    backend.queue_samples(&samples)
                };
                AUDIO_LAST_ERROR.with(|cell| cell.replace(result.err().map(|e| e.to_string())));
            }
        });
    } else if header.task_type == M_GFXTASK {
        // GFX_RENDER_NOTE: routes through the single registered `dyn
        // RenderBackend` (`set_render_backend`), the same executor-event-
        // seam pattern as the audio path above -- fn64-abi never names a
        // concrete backend crate (docs/DECOUPLING.md: "the backend never
        // reaches back into runtime state," and symmetrically, this crate
        // never reaches INTO a specific backend's internals, only the
        // trait). If no backend was registered (a test, or a harness that
        // hasn't wired one up), the task is still recorded/counted below,
        // same honesty stance as the audio path's "no ucode fn registered"
        // case -- never a silent pretend-success.
        RENDER_BACKEND.with(|cell| {
            if let Some(backend) = cell.borrow_mut().as_mut() {
                let render_end = unsafe { read_output_buff_size(rdram, o) };
                let task = fn64_render::OsTask {
                    task_type: header.task_type,
                    flags: header.flags,
                    ucode_boot: header.ucode_boot,
                    ucode_boot_size: header.ucode_boot_size,
                    ucode: header.ucode,
                    ucode_size: header.ucode_size,
                    ucode_data: header.ucode_data,
                    ucode_data_size: header.ucode_data_size,
                    dram_stack: header.dram_stack,
                    dram_stack_size: header.dram_stack_size,
                    output_buff: header.output_buff,
                    output_buff_size: render_end,
                    data_ptr: header.data_ptr,
                    data_size: header.data_size,
                };
                // Safety: `rdram` is valid for this call's duration per
                // this function's own contract; the backend only reads it
                // as `&[u8]` for the length the executor's rdram buffer
                // actually has (`RDRAM_LEN`, set by `set_render_backend`'s
                // caller alongside the backend itself).
                let rdram_len = RDRAM_LEN.with(|cell| cell.get());
                let rdram_slice = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
                // A backend error (unsupported ucode, bad bounds, backend
                // not ready) is intentionally NOT propagated as a MIPS-side
                // fault -- real hardware has no way to report "your gfx
                // task's ucode isn't implemented" back to the game thread
                // either; it is surfaced instead via `RENDER_LAST_ERROR`
                // for a harness/test to inspect, matching this crate's
                // "loud, not silent" rule without inventing a fake libultra
                // error path no real ROM's code checks for.
                let result = backend.process_task(rdram_slice, &task);
                RENDER_LAST_ERROR.with(|cell| cell.replace(result.err().map(|e| e.to_string())));
            }
        });
    }

    with_executor(|exec| exec.submit_task(header));
    ctx.r2 = 1; // task complete, did not yield (see doc comment)
}

/// Real `OSTask_t.t.output_buff_size` (`OSTask_t`'s field at offset 0x2C,
/// between `output_buff`@0x28 and `data_ptr`@0x30 per the public libultra
/// manual's documented layout) -- not part of
/// `fn64_runtime::rsp::OsTaskHeader` (that struct's own doc comment: fields
/// "unused by any call site this milestone reaches... omitted rather than
/// guessed"), but needed here because `fn64_render::OsTask` (the render
/// seam's own task view) does need an output-buffer bound to validate
/// against `rdram`'s length. Read directly rather than widening the shared
/// `OsTaskHeader`/`read_os_task_header`, keeping that struct's documented
/// scope untouched.
///
/// # Safety
/// Same contract as `read_os_task_header`.
unsafe fn read_output_buff_size(rdram: *mut u8, base: usize) -> u32 {
    let mut b = [0u8; 4];
    unsafe { std::ptr::copy_nonoverlapping(rdram.add(base + 0x2C), b.as_mut_ptr(), 4) };
    u32::from_ne_bytes(b)
}

thread_local! {
    /// The single registered graphics backend, if the shell/harness has
    /// called `set_render_backend`. `RefCell` (not `Cell`, unlike
    /// `AUDIO_UCODE_FN`) because a `Box<dyn RenderBackend>` is not `Copy`
    /// and needs `&mut` access across calls to drive its own internal
    /// state (`create`/`process_task`/`present`).
    static RENDER_BACKEND: RefCell<Option<Box<dyn RenderBackend>>> = const { RefCell::new(None) };
    /// The rdram buffer length the registered backend should treat as
    /// valid, set once by `set_render_backend`'s caller. Needed because
    /// `osSpTaskYielded_recomp` only receives a raw `*mut u8` (matching
    /// generated code's own `RECOMP_FUNC` signature), not a length --
    /// exactly the reason `fn64_runtime::Rdram` exists as an owned buffer
    /// with a known size elsewhere in this crate; this mirrors that same
    /// length knowledge for the one raw-pointer call site that needs it.
    static RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
    /// The most recent `RenderBackend::process_task` error, if any,
    /// stringified -- a harness/test observability hook (see
    /// `GFX_RENDER_NOTE`'s doc comment for why this isn't surfaced as a
    /// MIPS-side fault instead).
    static RENDER_LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };

    /// The single registered audio backend, if the shell/harness has
    /// called `set_audio_backend`. Mirrors `RENDER_BACKEND` exactly --
    /// see `AUDIO_BACKEND_NOTE` at its dispatch call site.
    static AUDIO_BACKEND: RefCell<Option<Box<dyn AudioBackend>>> = const { RefCell::new(None) };
    /// The rdram buffer length the registered audio backend should treat
    /// as valid, set once by `set_audio_backend`'s caller. Mirrors
    /// `RDRAM_LEN`'s role for the render seam.
    static AUDIO_RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
    /// The most recent `AudioBackend::queue_samples` error, if any,
    /// stringified. Mirrors `RENDER_LAST_ERROR`.
    static AUDIO_LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Register the audio backend `osSpTaskYielded_recomp` dispatches
/// `M_AUDTASK` sample-delivery through, and the rdram buffer length it may
/// safely read. Mirrors `set_render_backend`'s shape exactly -- see
/// `AUDIO_BACKEND_NOTE` at the dispatch call site for what half of the
/// audio path this covers (sample delivery, not ucode execution).
pub fn set_audio_backend(backend: Box<dyn AudioBackend>, rdram_len: usize) {
    AUDIO_BACKEND.with(|cell| cell.replace(Some(backend)));
    AUDIO_RDRAM_LEN.with(|cell| cell.set(rdram_len));
}

/// The most recent registered audio backend's `queue_samples` error, if
/// the last `M_AUDTASK` sample-delivery dispatch failed. `None` if no
/// audio task has run yet, the last one succeeded, or no backend is
/// registered at all. Mirrors `last_render_error`.
pub fn last_audio_error() -> Option<String> {
    AUDIO_LAST_ERROR.with(|cell| cell.borrow().clone())
}

/// Register the graphics backend `osSpTaskYielded_recomp` dispatches
/// `M_GFXTASK` submissions to, and the rdram buffer length it may safely
/// read (`rdram_len` must match the actual backing buffer's size -- a
/// mismatch here is a caller bug, not something this function can check
/// given it only stores a length, not the buffer itself). Mirrors
/// `set_audio_ucode_fn`'s "the shell wires this once at startup" shape,
/// generalized to a trait object since a graphics backend is stateful
/// (unlike a single ucode function pointer).
pub fn set_render_backend(backend: Box<dyn RenderBackend>, rdram_len: usize) {
    RENDER_BACKEND.with(|cell| cell.replace(Some(backend)));
    RDRAM_LEN.with(|cell| cell.set(rdram_len));
}

/// The most recent registered backend's `process_task` error, if the last
/// `M_GFXTASK` dispatch failed. `None` if no gfx task has run yet, the last
/// one succeeded, or no backend is registered at all. A test/harness
/// observability hook -- see `set_render_backend`'s doc comment.
pub fn last_render_error() -> Option<String> {
    RENDER_LAST_ERROR.with(|cell| cell.borrow().clone())
}

/// Real translated audio-ucode function signature, per
/// `aki-recomp/games/NWXE/rsp/wm2000_audio.toml`'s
/// `output_function_name = "wm2000_audio_ucode"` and its generated C's own
/// signature (`RspExitReason wm2000_audio_ucode(uint8_t* rdram, uint32_t
/// ucode_addr)`). `RspExitReason` is an RSPRecomp-defined enum this crate
/// does not need to interpret (see `osSpTaskYielded_recomp`'s doc comment:
/// "not yet interpreted beyond 'it ran'") -- represented here as a plain
/// `u32` return the FFI boundary doesn't need to name further.
pub type AudioUcodeFn = unsafe extern "C" fn(*mut u8, u32) -> u32;

thread_local! {
    /// The real, out-of-tree translated audio ucode function, if the host
    /// (the boot harness) has linked and registered one via
    /// `set_audio_ucode_fn`. `None` in any test/context that never calls
    /// that -- `osSpTaskYielded_recomp` treats that as "can't actually run
    /// the ucode" (see its doc comment), never a silent substitute.
    static AUDIO_UCODE_FN: Cell<Option<AudioUcodeFn>> = const { Cell::new(None) };
}

/// Register the real translated audio ucode function. Called once by the
/// boot harness (`examples/wm2000-boot`) after linking WM2000's
/// out-of-tree-compiled `wm2000_audio.cpp` -- `fn64-abi` never contains
/// this function's body itself (`README.md`'s "no game content ships in
/// this repo" rule), only the call-site plumbing that invokes whatever the
/// harness supplies.
///
/// # Safety
/// `f` must have the real `RspExitReason(uint8_t*, uint32_t)` signature
/// RSPRecomp generates and must remain valid for the process's lifetime
/// (true for a file-scope C function with static storage duration, which is
/// what RSPRecomp emits).
pub unsafe fn set_audio_ucode_fn(f: AudioUcodeFn) {
    AUDIO_UCODE_FN.with(|cell| cell.set(Some(f)));
}

/// Read the public libultra manual's documented `OSTask_t` field layout
/// (see `osSpTaskYielded_recomp`'s doc comment for the byte offsets) out of
/// `rdram` at `base` (already an rdram-relative offset, not a raw vram/gpr
/// value -- callers translate first via `RdramAddr`).
///
/// # Safety
/// `rdram` must be valid for at least `base + 0x38` bytes.
unsafe fn read_os_task_header(rdram: *mut u8, base: usize) -> OsTaskHeader {
    // Native byte order, matching MEM_W's real semantics -- see
    // `read_stack_word`'s doc comment for the full correction this wave made.
    let w = |off: usize| -> u32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(rdram.add(base + off), b.as_mut_ptr(), 4) };
        u32::from_ne_bytes(b)
    };
    OsTaskHeader {
        task_type: w(0x0),
        flags: w(0x4),
        ucode_boot: w(0x8),
        ucode_boot_size: w(0xC),
        ucode: w(0x10),
        ucode_size: w(0x14),
        ucode_data: w(0x18),
        ucode_data_size: w(0x1C),
        dram_stack: w(0x20),
        dram_stack_size: w(0x24),
        output_buff: w(0x28),
        data_ptr: w(0x30),
        data_size: w(0x34),
    }
}

// ---------------------------------------------------------------------
// VI family: real, host-state-backed implementations (this wave). See
// fn64_runtime::vi's module doc for the design (host hardware STATE model,
// not a VI-manager thread -- that role is the executor's OS_EVENT_VI
// delivery, already wired via osSetEventMesg_recomp + Executor::advance_time
// per rung 11's osCreateViManager evidence). Every real call site's exact
// argument register per profile.toml's byte-cited rung-11 writeup
// (func_80001410's VI bring-up sequence): `osViSetMode(a0=mode_ptr)`,
// `osViSetSpecialFeatures(a0=features_ptr)`, `osViSetYScale(f12=scale)`,
// `osViSwapBuffer(a0=frameBufPtr)`, `osViBlack(a0=active)`.
// ---------------------------------------------------------------------

/// `osCreateViManager(OSPri pri)` -- `a0`=`ctx->r4` (unused; see doc below).
/// A direct `FuncEntry.func` slot in `recomp_overlays.inl` (N64Recomp skips
/// codegen for it entirely, per `games/NWXE/profile.toml`'s rung-11
/// identification of `func_80032B90`). Real libultra semantics spin up a
/// dedicated VI-manager thread that owns retrace/counter event delivery;
/// per `docs/DESIGN.md` section 2's single-executor-coroutine model, that
/// role is already the executor's own `advance_time`/retrace-ticker
/// machinery (`Executor::vi_set_event`/`arm_retrace`, wired this wave) --
/// there is no second host thread to spin up here. This shim's real, tested
/// effect is therefore intentionally a safe no-op beyond existing as a
/// callable symbol: no separate VI-manager state needs establishing that
/// `Executor::new`'s `Default` didn't already establish, matching the same
/// reasoning `osInitialize_recomp`'s doc comment gives for its own no-op
/// status.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreateViManager_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osViSetEvent(OSMesgQueue *mq, OSMesg msg, u32 retraceCount)` -- `a0`=mq
/// (`ctx->r4`), `a1`=msg (`ctx->r5`), `a2`=retraceCount
/// (`ctx->r6`, accepted but not modeled -- see `ViState::set_event`'s doc
/// comment). A direct `FuncEntry.func` slot (rung 11: `func_80032ED0`, exact
/// 0x58 size match vs donor, `->0x10=mq(a0), ->0x14=msg(a1)`) -- writes
/// directly into the VI manager's own internal retrace-notification target,
/// a mechanism `games/NWXE/profile.toml`'s rung-11 writeup documents as
/// DISTINCT from `osSetEventMesg`'s general `OS_EVENT_*` table (both may be
/// registered and both fire on the same retrace tick, per
/// `Executor::advance_time`'s doc comment).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetEvent_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    with_executor(|exec| exec.vi_set_event(mq_addr, msg));
}

/// `osViSetMode(OSViMode *mode)` -- `a0` = `ctx->r4`, the mode-table vram
/// pointer (rung 11: `func_80032F30`, exact 0x4C size match vs donor,
/// `->0x8=mode(s0)`). This crate does not model `OSViMode`'s internal
/// NTSC/PAL timing-register fields (no shim reads them back; storing the
/// raw pointer is the honest state this milestone needs -- see
/// `ViState::set_mode`'s doc comment).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetMode_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mode_ptr = ctx.r4 as u32;
    with_executor(|exec| exec.vi_set_mode(mode_ptr));
}

/// `osViSetSpecialFeatures(OSViSpecialFeatures *sf)` -- `a0` = `ctx->r4`
/// (rung 11: `func_80032F80`, exact 0x164 size match vs donor).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetSpecialFeatures_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let sf_ptr = ctx.r4 as u32;
    with_executor(|exec| exec.vi_set_special_features(sf_ptr));
}

/// `osViSetYScale(f32 scale)` -- the o32 float-argument convention passes
/// `scale` in `$f12`, not a GPR; `recomp_context`'s `f12: Fpr` union's
/// `halves.0` is the float half the compiler emits for a single-precision
/// arg per `recomp.h`'s calling-convention codegen (rung 11: `func_800330F0`,
/// exact 0x44 size match vs donor, `swc1 $fs0 -> 0x24`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSetYScale_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let scale = unsafe { ctx.f12.halves.0 };
    with_executor(|exec| exec.vi_set_y_scale(scale));
}

/// `osViSwapBuffer(void *frameBufPtr)` -- `a0` = `ctx->r4` (rung 11:
/// `func_80033140`, exact 0x44 size match vs donor, `->0x4=framebuffer(s0)`).
/// This is the task's framebuffer-capture trigger point: the returned
/// `RdramAddr` is exactly what a host driver (`fn64-shell`/the boot harness)
/// needs to hash/dump the pointed-to fb region on every swap -- see
/// `Executor::vi_swap_buffer`'s doc comment for why the value is handed
/// back rather than only stored.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViSwapBuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let frame_buf = RdramAddr::from_gpr(ctx.r4);
    with_executor(|exec| {
        exec.vi_swap_buffer(frame_buf);
    });
}

/// `osViBlack(u8 active)` -- `a0` = `ctx->r4` (rung 11: `func_800334A0`,
/// exact 0x5C size match vs donor, toggles state bit 0x20 set/clear on
/// `arg&0xFF`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViBlack_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let active = (ctx.r4 & 0xFF) != 0;
    with_executor(|exec| exec.vi_set_black(active));
}

/// Read the most recently swapped VI framebuffer's rdram address, if any --
/// the boot harness's polling hook for the task's "on every osViSwapBuffer,
/// hash the pointed-to fb region" requirement, since a harness driving the
/// executor from outside this crate has no other way to observe
/// `ViState::current_framebuffer` (it's private to `fn64-runtime`'s
/// `Executor`, per this crate's existing "no raw field access" convention).
pub fn current_vi_framebuffer() -> Option<u32> {
    with_executor(|exec| exec.vi().current_framebuffer.map(|a| a.offset()))
}

/// The total number of `osViSwapBuffer` calls observed so far -- see
/// `current_vi_framebuffer`'s doc comment for why this crate exposes a
/// plain function rather than requiring the harness to reach into
/// `Executor` directly.
pub fn vi_swap_count() -> u64 {
    with_executor(|exec| exec.vi().swap_count)
}

/// Arm the VI retrace ticker (`Executor::arm_retrace`) -- see
/// `fn64_runtime::vi`'s module doc for why this is a host-chosen
/// approximation, not a hardware-accurate NTSC/PAL constant.
pub fn arm_vi_retrace(interval: u64) {
    with_executor(|exec| exec.arm_retrace(interval));
}

// ---------------------------------------------------------------------
// Host-facing (non-`_recomp`) helpers.
// ---------------------------------------------------------------------

/// Host-side entry point for injecting an external (SI/PI/VI-style)
/// completion into the executor -- not a `_recomp` shim.
pub fn inject_external_event(event: ExternalEvent) {
    with_executor(|exec| exec.inject_event(event));
}

/// Host-side virtual-clock driver.
pub fn advance_virtual_time(now: u64) {
    with_executor(|exec| exec.advance_time(now));
}

/// Create and start thread 0 running `recomp_entrypoint` -- the harness's
/// boot entry point. `recomp_entrypoint`'s own body (verified directly
/// against `RecompiledFuncs/funcs_0.c`) computes its own jump target from
/// literal immediates with no dependency on incoming register state, so an
/// all-zero `RecompContext` is the correct, real starting state (matching
/// what a fresh `OSThread`'s initial register file honestly is before any
/// game code has run), not a placeholder.
///
/// # Safety
/// `rdram` must be a valid pointer to the process's one shared rdram
/// buffer, live for at least as long as any coroutine spawned here might
/// run (the whole process, per `docs/DESIGN.md` section 3). `entry` must be
/// `recomp_entrypoint` (or a real `recomp_func_t`-shaped function with the
/// same contract) -- the boot harness passes the real generated symbol.
pub unsafe fn boot_thread0(
    rdram: *mut u8,
    entry: RecompFunc,
    thread_id: ThreadId,
    priority: Priority,
) {
    let rdram_addr = rdram as usize;
    with_executor(|exec| {
        exec.create_thread(thread_id, priority, move |yielder, first_input| {
            let rdram_ptr = rdram_addr as *mut u8;
            with_active_yielder(thread_id, rdram_ptr, yielder, || {
                let _ = first_input;
                let mut ctx = RecompContext::zeroed();
                unsafe { entry(rdram_ptr, &mut ctx as *mut _) };
            });
        });
        exec.start_thread(thread_id);
    });
}

/// Run one scheduling step (see `Executor::run_one_step`'s doc comment).
/// Returns `false` when nothing was runnable -- the harness should then
/// call `advance_virtual_time` to make host-driven progress (VI retrace,
/// due timers) before trying again.
///
/// This is THE seam that re-arms the coroutine-context thread-locals (see
/// `THREAD_CONTEXTS`' doc comment) to the thread ABOUT to be resumed --
/// every caller in this crate (including this crate's own tests) must
/// dispatch a scheduling step through this function (or `run_to_idle`,
/// below), never a bare `exec.run_one_step()`/`exec.run_to_idle()` inside a
/// `with_executor` closure, or the re-arm is skipped and the bug this wave
/// fixed reappears.
pub fn run_one_step() -> bool {
    with_executor(|exec| {
        // `peek_next_thread` is a read-only preview of exactly which thread
        // `exec.run_one_step()` is about to resume -- read it BEFORE the
        // resume so the correct thread's context is active for the entire
        // duration of that resume (including every `_recomp` shim call the
        // thread's body makes after waking, up to and including its next
        // suspend). `None` means nothing is runnable -- no resume will
        // happen, so no context needs arming.
        match exec.peek_next_thread() {
            Some(id) => with_rearmed_context(id, || exec.run_one_step()),
            None => exec.run_one_step(),
        }
    })
}

/// Run until the run queue is idle (every thread finished or blocked). See
/// `run_one_step`'s doc comment -- this loops it rather than calling
/// `Executor::run_to_idle` directly, so every individual resume inside the
/// loop gets its own correctly-armed context (a single re-arm before the
/// whole loop would be exactly as wrong as the original bug once a second
/// thread's turn came up).
pub fn run_to_idle() {
    while run_one_step() {}
}

/// Whether thread `id` has finished (its coroutine returned or was never
/// created) -- the harness's "has boot's thread 0 died" check.
pub fn is_thread_dead(id: ThreadId) -> bool {
    with_executor(|exec| exec.is_thread_dead(id))
}

/// Gfx/audio task submission counts observed so far (`Executor::task_log`).
pub fn task_counts() -> (u64, u64) {
    with_executor(|exec| (exec.task_log().gfx_count(), exec.task_log().audio_count()))
}

/// Copy the full recorded trace out as an owned `Vec` -- the harness's
/// entry point for emitting `docs/DESIGN.md` section 4's shared
/// `TraceEvent` stream to a file.
pub fn copy_trace() -> Vec<fn64_runtime::TraceEvent> {
    with_executor(|exec| exec.trace().to_vec())
}

/// Arm incremental crash-safe trace flushing -- every trace event recorded
/// from this call onward is appended+flushed to `path` immediately, not
/// just buffered in memory for `copy_trace`'s end-of-run snapshot. Call
/// this BEFORE booting thread 0, so a SIGSEGV/abort mid-boot still leaves
/// every event up to the crash on disk. See
/// `fn64_runtime::TraceLog::set_sink_file`'s doc comment for the incident
/// (WM2000 rung-3 frontier) this fixes.
pub fn set_trace_sink_file(path: &str) -> std::io::Result<()> {
    with_executor(|exec| exec.set_trace_sink_file(path))
}

/// The executor's current virtual-clock reading.
pub fn sim_time() -> u64 {
    with_executor(|exec| exec.sim_time())
}

// ---------------------------------------------------------------------
// Small shared helpers.
// ---------------------------------------------------------------------

impl RecompContext {
    /// An all-zero `RecompContext` -- used to seed a freshly-dispatched
    /// thread entry point's register state (`osCreateThread_recomp`).
    /// `f_odd` is a raw pointer with no valid target in this all-zero state
    /// (null); no shim in this milestone's undefined set touches it (see
    /// this crate's earlier note: "no direct ctx-> touches of status_reg/
    /// mips3_float_mode/f_odd were found" per ABI-SURFACE.md section (b)),
    /// so null is the honest, unfaked value rather than a fabricated
    /// pointee.
    pub fn zeroed() -> Self {
        // Safety: RecompContext is a `#[repr(C)]` struct of plain integers
        // and one raw pointer, all of which are valid when all-zero (a
        // null pointer is a valid `*mut u32` bit pattern). `Fpr` is a
        // `#[repr(C)]` union of plain numeric types, likewise valid
        // zeroed.
        unsafe { std::mem::zeroed() }
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

/// `__osPiGetAccess(void)` -- no arguments (verified: real call site
/// `funcs_0.c` asm 0x80001608, a bare `jal` with no register setup
/// immediately before it, same no-arg shape `osCartRomInit_recomp`'s doc
/// comment already established for this corpus's PI-bus bring-up
/// sequence). Real hardware effect: acquires the PI-bus mutex so this
/// thread has exclusive access for a following DMA/IO sequence. Per
/// `docs/DESIGN.md`'s single-executor-thread model there is no real
/// concurrent PI-bus contention to arbitrate (see `osSetIntMask_recomp`'s
/// doc comment for the identical reasoning already applied to the
/// interrupt-mask shim) -- a safe no-op beyond existing as a callable
/// symbol.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiGetAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `__osPiRelAccess(void)` -- no arguments (verified: both real call sites
/// in `funcs_0.c`, asm 0x80001628 and 0x800017B8, are bare `jal`s with no
/// register setup beforehand). Releases the mutex `__osPiGetAccess_recomp`
/// acquires; same no-op reasoning.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osPiRelAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

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

/// `osContGetQuery(int channel, OSContStatus *data) -> s32` -- `a0`=channel
/// (`ctx->r4`), `a1`=data (`ctx->r5`). Public libultra manual's documented
/// counterpart to `osContStartQuery`/`__osSiRawStartDma`'s PIF probe --
/// `OSContStatus` is `{type: u16, status: u8, errno: u8}`, the SAME 3-byte
/// `[type_hi, type_lo, status]` shape `PifModel::query_response` already
/// produces for `__osSiRawStartDma_recomp`'s raw PIF-block path (rung 15's
/// `osContStartQuery`/`osContGetQuery` pair is PadMgr's own higher-level
/// wrapper over that same raw mechanism, per the public manual). No real
/// `jal` call site in this corpus's static analysis (function-table slot
/// only, `recomp_overlays.inl:2934`) -- reached via PadMgr's own internal
/// polling per BOOT-PLAN.md rung 15, so implemented for real rather than
/// loud-trapped.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetQuery_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let channel = ctx.r4 as usize;
    let data_addr = RdramAddr::from_gpr(ctx.r5).offset() as usize;
    let resp = with_executor(|exec| exec.pif().query_response(channel));
    unsafe {
        // OSContStatus: type (u16, big-endian on real hardware's PIF wire
        // format, matching query_response's existing [hi, lo, status]
        // byte order) followed by status/errno bytes.
        *rdram.add(data_addr) = resp[0];
        *rdram.add(data_addr + 1) = resp[1];
        *rdram.add(data_addr + 2) = resp[2];
        *rdram.add(data_addr + 3) = 0; // errno: no error modeled
    }
    ctx.r2 = 0;
}

/// `osContGetReadData(OSContPad *pad) -> s32` -- `a0`=`ctx->r4`. Public
/// libultra manual's documented `OSContPad` layout: `button` (u16),
/// `stick_x`/`stick_y` (s8 each), `errno` (u8) -- the same 4-byte idle
/// shape `PifModel::read_data_response` already returns for
/// `__osSiRawStartDma_recomp`'s raw path. Function-table slot only
/// (`recomp_overlays.inl:2920`), reached via PadMgr's internal polling
/// (BOOT-PLAN.md rung 15) -- implemented for real.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContGetReadData_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let pad_addr = RdramAddr::from_gpr(ctx.r4).offset() as usize;
    let resp = with_executor(|exec| exec.pif().read_data_response(0));
    unsafe {
        std::ptr::copy_nonoverlapping(resp.as_ptr(), rdram.add(pad_addr), 4);
    }
    ctx.r2 = 0;
}

/// `osContInit(OSMesgQueue *mq, u8 *bitpattern, OSContStatus *data) -> s32`
/// -- `a0`=mq (`ctx->r4`), `a1`=bitpattern (`ctx->r5`), `a2`=data
/// (`ctx->r6`). Public libultra manual's documented one-time controller-
/// manager bring-up: probes all 4 ports and sets one bit per populated
/// port in `*bitpattern`. Function-table slot only
/// (`recomp_overlays.inl:2918`), reached from `PadMgr_Init`
/// (BOOT-PLAN.md rung 15's forcing-function call) -- implemented for real
/// against `PifModel`'s "port 0 populated, 1-3 absent" model
/// (`si.rs`'s module doc, this task's explicit scope).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContInit_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let bitpattern_addr = RdramAddr::from_gpr(ctx.r5).offset() as usize;
    let data_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    let mut mask: u16 = 0;
    with_executor(|exec| {
        let pif = *exec.pif();
        for port in 0..4u32 {
            let resp = pif.query_response(port as usize);
            let absent = (resp[2] & fn64_runtime::si::CONT_ABSENT) != 0;
            if !absent {
                mask |= 1 << port;
                let off = data_addr + (port as usize) * 4;
                unsafe {
                    *rdram.add(off) = resp[0];
                    *rdram.add(off + 1) = resp[1];
                    *rdram.add(off + 2) = resp[2];
                    *rdram.add(off + 3) = 0;
                }
            }
        }
    });
    unsafe {
        // OSContStatus's own u16 field the public manual documents this
        // out-param as writing (present-port bitmask, hi byte first per
        // the same PIF wire-order convention used throughout this file).
        *rdram.add(bitpattern_addr) = (mask >> 8) as u8;
        *rdram.add(bitpattern_addr + 1) = (mask & 0xFF) as u8;
    }
    ctx.r2 = 0;
}

/// `osContSetCh(u8 ch) -> s32` -- `a0`=`ctx->r4`. Public libultra manual:
/// restricts subsequent controller-manager polling to the first `ch`
/// channels. This crate's `PifModel` always reports the same fixed 4-port
/// state regardless of channel count (`si.rs`'s module doc: "one standard
/// controller on port 0... ports 1-3 absent" is not parameterized by a
/// runtime channel-count setting) -- stored as plain host state for
/// fidelity/logging, with no other behavioral effect, matching
/// `osAiSetFrequency_recomp`'s existing "store it, no consumer needs it
/// yet" pattern for an unconsumed configuration value. Function-table slot
/// only (`recomp_overlays.inl:2958`), reached from `PadMgr_Init`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContSetCh_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    CONT_CHANNELS.with(|cell| cell.set((ctx.r4 & 0xFF) as u8));
    ctx.r2 = 0;
}

thread_local! {
    static CONT_CHANNELS: Cell<u8> = const { Cell::new(4) };
}

/// `osContStartQuery(OSMesgQueue *mq) -> s32` -- `a0`=`ctx->r4`. Public
/// libultra manual: kicks off an async PIF status-query DMA, posting
/// completion to `mq`. This crate's PI/SI DMA is synchronous-modeled
/// throughout (`__osSiRawStartDma_recomp`'s doc comment: "every path... is
/// success"/completes immediately) -- consistent with that, this shim
/// posts the `OS_EVENT_SI` completion (mirroring
/// `__osSiRawStartDma_recomp`'s own event-post at the bottom of this file)
/// immediately rather than modeling a real async gap, since no evidence
/// shows any call site depending on a delay here. Function-table slot only
/// (`recomp_overlays.inl:2933`), reached from `PadMgr_Init`/its polling
/// thread body (BOOT-PLAN.md rung 15).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartQuery_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0);
        exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
    });
    ctx.r2 = 0;
}

/// `osContStartReadData(OSMesgQueue *mq) -> s32` -- same shape/reasoning as
/// `osContStartQuery_recomp` (Public libultra manual's paired async
/// button/stick-read DMA kickoff). Function-table slot only
/// (`recomp_overlays.inl:2919`), reached from PadMgr's polling thread body.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osContStartReadData_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    const OS_EVENT_SI: u32 = 5;
    with_executor(|exec| {
        exec.set_event_mesg(OS_EVENT_SI, mq_addr, 0);
        exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SI));
    });
    ctx.r2 = 0;
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

/// `osEPiReadIo(OSPiHandle *handle, u32 devAddr, void *dramAddr) -> s32` --
/// `a0`=handle (`ctx->r4`, unused, same as `osEPiStartDma_recomp`'s
/// `osCartRomInit_recomp`-established handle stance), `a1`=devAddr
/// (`ctx->r5`), `a2`=dramAddr (`ctx->r6`) -- verified against the real call
/// site (`funcs_0.c:2611`: `ctx->r4=MEM_W(...)` a handle-shaped global,
/// `ctx->r5=0x3C` a devAddr, `ctx->r6=sp+0x24` a stack dramAddr). Public
/// libultra manual: a SYNCHRONOUS single 4-byte cartridge-domain read (no
/// `OSIoMesg`/queue involved, unlike `osEPiStartDma`'s async multi-byte
/// transfer) -- reads one word directly from ROM at `devAddr` into
/// `*dramAddr`.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiReadIo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let dev_addr = ctx.r5 as u32;
    let dram_addr = RdramAddr::from_gpr(ctx.r6).offset() as usize;
    with_pi_dma("osEPiReadIo_recomp", |dma| {
        let mut buf = [0u8; 4];
        dma.read_rom_bytes(dev_addr, &mut buf);
        unsafe {
            std::ptr::copy_nonoverlapping(buf.as_ptr(), rdram.add(dram_addr), 4);
        }
    });
    ctx.r2 = 0;
}

/// `osEPiWriteIo(OSPiHandle *handle, u32 devAddr, u32 data) -> s32` --
/// `a0`=handle (unused), `a1`=devAddr (`ctx->r5`), `a2`=data (`ctx->r6`).
/// Public libultra manual's synchronous single-word cartridge-domain
/// WRITE counterpart to `osEPiReadIo_recomp`. `PiDma`/`InMemoryRom` (this
/// crate's ROM backing) has no write-to-cart-domain support (`rom.rs`'s
/// `PiDma` doc: ROM is read-only host state) -- consistent with
/// `osEPiStartDma_recomp`'s existing `DmaDirection::FromRdram`
/// `unimplemented!()` stance for the same underlying gap, this is a loud
/// trap rather than a silent no-op, since a real cartridge write silently
/// discarded would be a correctness lie a differential trace could not
/// catch.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiWriteIo_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osEPiWriteIo_recomp: cartridge-domain writes have no backing store in this milestone \
         (InMemoryRom is read-only, matching osEPiStartDma_recomp's existing OS_WRITE gap) -- \
         real call site is games/OOTU/RecompiledFuncs/funcs_0.c, needs a real write-back model \
         before this can return anything but a loud trap."
    );
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

/// `osJamMesg(OSMesgQueue *mq, OSMesg msg, s32 flag)` -- priority-jump
/// variant of `osSendMesg` (public libultra manual: inserts at the FRONT
/// of the queue rather than the back). Zero real call sites in this corpus
/// and BOOT-PLAN.md's own "not observed on the traced happy path" note
/// (IrqMgr's PRENMI handling, off the first-frame boot ladder) -- loud-
/// trapped rather than silently approximated as a plain `osSendMesg`
/// (`fn64_runtime::MesgQueue`/`Executor` has no front-insert primitive
/// today; faking one via `try_send`'s back-of-queue semantics would be a
/// real ordering lie for a queue with >1 pending message, silently wrong
/// in exactly the multi-message case that would matter).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osJamMesg_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osJamMesg_recomp: no front-of-queue insert primitive exists in fn64_runtime::MesgQueue \
         yet, and no real call site in games/OOTU/RecompiledFuncs exercises this (BOOT-PLAN.md: \
         reached only off IrqMgr's PRENMI path, not the first-frame boot ladder) -- a fabricated \
         back-of-queue insert would silently misorder a queue with >1 pending message."
    );
}

/// `osMotorInit(OSMesgQueue *mq, OSPfs *pfs, int channel) -> s32` --
/// Rumble Pak initialization. Zero real call sites in this corpus and
/// BOOT-PLAN.md's own "rumble-pak specific... not required for a picture
/// on screen" note. `PifModel` (`si.rs`) explicitly models "no pak" on
/// every port (this task's stated scope) -- loud-trapped rather than
/// fabricating a fake accessory-present/success response, since a real
/// game branching on this return value deserves a named failure, not a
/// silently-wrong "rumble pak found."
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osMotorInit_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osMotorInit_recomp: no Rumble Pak modeled (PifModel's explicit 'no pak' scope, \
         si.rs's module doc) and no real call site in games/OOTU/RecompiledFuncs exercises \
         this on the boot path (BOOT-PLAN.md: 'not required for a picture on screen') -- a \
         fabricated success/failure response would be an unearned guess."
    );
}

/// `__osMotorAccess(OSPfs *pfs, int accesslib)` -- Rumble Pak channel-access
/// mutex primitive, reached only from PadMgr's deeper controller-pak
/// polling (BOOT-PLAN.md: after `osContStartQuery` succeeds; not required
/// for a picture on screen). Zero real call sites in this corpus
/// (function-table slot only, `recomp_overlays.inl:2916`). Same
/// no-accessory-modeled reasoning as `osMotorInit_recomp` -- loud trap.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osMotorAccess_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "__osMotorAccess_recomp: no Rumble Pak modeled (see osMotorInit_recomp's doc comment) \
         and no real call site in games/OOTU/RecompiledFuncs exercises this on the boot path."
    );
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

/// `osSpTaskLoad(OSSpTask *sptask)` -- loads a task descriptor into the
/// (this crate's single, synchronous) SP-task-dispatch pipeline. Public
/// libultra manual: normally a bookkeeping step distinct from
/// `osSpTaskStartGo` (which actually kicks the RSP), used by `Sched`'s own
/// internal task-processing helpers (BOOT-PLAN.md rung 13: `sched.c:252,
/// 441,453`) to submit a task before yielding for its completion. This
/// crate's task-dispatch model is already synchronous-on-submit
/// (`osSpTaskYielded_recomp`'s doc comment: task execution + completion
/// happen inline, no real async RSP-timing gap) -- `osSpTaskLoad`'s real
/// effect here is recording the task header via the SAME
/// `Executor::submit_task` path `osSpTaskYielded_recomp` already uses, so
/// the trace/task-log sees every real submission regardless of which of
/// the two libultra entry points a given caller uses. No real `jal` call
/// site in this corpus (function-table slot only,
/// `recomp_overlays.inl:2914`), reached from `Sched_ThreadEntry`'s task-
/// processing helpers per BOOT-PLAN.md rung 13.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskLoad_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };
    with_executor(|exec| exec.submit_task(header));
}

/// `osSpTaskStartGo(OSSpTask *sptask)` -- the actual RSP-kickoff half of
/// the pair `osSpTaskLoad_recomp` above bookkeeps. Since this crate's
/// dispatch model runs a task's real effect (audio ucode call / gfx
/// backend dispatch) synchronously at `osSpTaskYielded_recomp`, not at
/// `osSpTaskLoad`/`osSpTaskStartGo` time, this shim's own real effect is
/// intentionally limited to existing as a callable symbol with no double-
/// dispatch -- see that function's doc comment for where the actual
/// ucode/gfx-backend call happens. A real hardware `osSpTaskStartGo`
/// writes RSP `SP_STATUS`/kicks execution; this crate has no separate
/// RSP-register model to poke (same reasoning as `__osSpSetStatus_recomp`
/// above).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskStartGo_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

/// `osSpTaskYield(void)` -- signals the RSP to yield its current task back
/// to the CPU, returning immediately (asynchronous request, not a
/// blocking wait -- `osSpTaskYielded` is the separate poll/wait-for-
/// completion call, already implemented above). Verified real call site:
/// `funcs_41.c:32`, a bare `jal` with no register setup. This crate's
/// synchronous dispatch model means a task has always already fully run
/// to completion by the time control returns from submission (no
/// mid-task yield state to request) -- a safe no-op beyond existing as a
/// callable symbol, matching `__osSpSetPc_recomp`'s "no real concurrent
/// RSP hardware to model" stance.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYield_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {}

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

/// `osViGetCurrentFramebuffer(void) -> void*` -- no arguments; returns the
/// currently-displayed (not next-queued) framebuffer's vram pointer.
/// Function-table slot only (`recomp_overlays.inl:2974`). This crate's
/// `ViState` (`vi.rs`) tracks only ONE "most recently swapped" framebuffer
/// field (`current_framebuffer`, already exposed via
/// `current_vi_framebuffer`) -- no separate "currently displayed vs. next
/// queued" double-buffer distinction exists yet, so this returns the same
/// value `osViSwapBuffer`'s last call recorded, an honest approximation
/// (single most-recent value) rather than a fabricated second buffer this
/// crate has no state for.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViGetCurrentFramebuffer_recomp(
    _rdram: *mut u8,
    ctx: *mut RecompContext,
) {
    let ctx = unsafe { &mut *ctx };
    let fb = with_executor(|exec| exec.vi().current_framebuffer);
    ctx.r2 = fb.map(|a| a.offset() as u64).unwrap_or(0);
}

/// `osViGetNextFramebuffer(void) -> void*` -- same reasoning/return value
/// as `osViGetCurrentFramebuffer_recomp` (this crate has no separate
/// pending-vs-current double-buffer state; both report the same most-
/// recent swap). Function-table slot only (`recomp_overlays.inl:65`).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osViGetNextFramebuffer_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let fb = with_executor(|exec| exec.vi().current_framebuffer);
    ctx.r2 = fb.map(|a| a.offset() as u64).unwrap_or(0);
}

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

/// `osLeoDiskInit(void) -> s32` -- 64DD (Disk Drive) subsystem init. OoT
/// (OOTU) is a cartridge-only retail title; `leomain`/`LeoCJCreateLeoManager`
/// /`LeoCACreateLeoManager` (this symbol's only callers, newly reachable
/// after the 2026-07-14 stub-set fix) are 64DD-family debug/dev-kit code
/// paths dead on real retail hardware and never exercised by this crate's
/// PI/cartridge-only `InMemoryRom` model (`rom.rs`) -- no 64DD drive state
/// exists to initialize. Loud trap rather than a fabricated "drive present"
/// success/failure code.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osLeoDiskInit_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osLeoDiskInit_recomp: no 64DD (Disk Drive) subsystem exists in this crate -- \
         OOTU is cartridge-only retail; reachable only from leomain/LeoC{{J,A}}CreateLeoManager \
         (games/OOTU/RecompiledFuncs), 64DD-family dev-kit code paths unstubbed by the \
         2026-07-14 gen_stubs.py false-positive-stub fix. A fabricated init result would be \
         an unearned guess about hardware this crate never models."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::{RecvMesgOutcome, SendMesgOutcome};

    fn ctx_zeroed() -> RecompContext {
        RecompContext::zeroed()
    }

    fn ctx_with(r4: u64, r5: u64, r6: u64) -> RecompContext {
        let mut ctx = ctx_zeroed();
        ctx.r4 = r4;
        ctx.r5 = r5;
        ctx.r6 = r6;
        ctx
    }

    fn spawn_test_thread(id: ThreadId, pri: Priority, body: impl FnOnce() + 'static) {
        with_executor(|exec| {
            exec.create_thread(id, pri, move |yielder, first_input| {
                with_active_yielder(id, std::ptr::null_mut(), yielder, || {
                    let _ = first_input;
                    body();
                });
            });
            exec.start_thread(id);
        });
    }

    fn run_to_idle_with_yielder_plumbing() {
        run_to_idle();
    }

    #[test]
    fn pause_self_yields_via_real_executor_and_thread_keeps_running() {
        let ran_twice = std::rc::Rc::new(std::cell::RefCell::new(0));
        let ran_twice2 = ran_twice.clone();
        spawn_test_thread(100, 5, move || {
            pause_self(std::ptr::null_mut());
            *ran_twice2.borrow_mut() += 1;
        });
        assert!(run_one_step());
        assert_eq!(*ran_twice.borrow(), 0);
        assert!(run_one_step());
        assert_eq!(*ran_twice.borrow(), 1);
    }

    #[test]
    fn create_then_nonblocking_send_succeeds() {
        let mq_vram: u64 = 0xFFFF_FFFF_8005_7228;
        let mut create_ctx = ctx_with(mq_vram, 0, 4);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        spawn_test_thread(101, 1, move || {
            let mut send_ctx = ctx_with(mq_vram, 0xABCD, OS_MESG_NOBLOCK);
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
        });
        run_to_idle_with_yielder_plumbing();

        with_executor(|exec| {
            let addr = RdramAddr::from_gpr(mq_vram);
            assert_eq!(exec.queue_capacity(addr), 4);
        });
    }

    #[test]
    fn blocking_send_on_full_queue_actually_yields_and_is_woken() {
        let mq_vram: u64 = 0xFFFF_FFFF_8006_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        with_executor(|exec| {
            let outcome = exec.send_mesg(0, mq_addr, 1, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });

        let delivered_second = std::rc::Rc::new(std::cell::RefCell::new(false));
        let delivered_second2 = delivered_second.clone();
        spawn_test_thread(102, 1, move || {
            let mut send_ctx = ctx_with(mq_vram, 2, OS_MESG_BLOCK);
            unsafe { osSendMesg_recomp(std::ptr::null_mut(), &mut send_ctx as *mut _) };
            *delivered_second2.borrow_mut() = true;
        });

        run_one_step();
        assert!(!*delivered_second.borrow());

        with_executor(|exec| {
            let outcome = exec.recv_mesg(999, mq_addr, false);
            assert_eq!(outcome, RecvMesgOutcome::Delivered(1));
        });
        run_to_idle_with_yielder_plumbing();
        assert!(*delivered_second.borrow());
    }

    #[test]
    fn blocking_recv_on_empty_queue_yields_and_receives_real_message() {
        let mq_vram: u64 = 0xFFFF_FFFF_8007_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        let mut rdram = vec![0u8; 64];
        let rdram_ptr = rdram.as_mut_ptr();
        let msg_out_vram: u64 = 0xFFFF_FFFF_8000_0020;

        spawn_test_thread(103, 1, move || {
            let mut recv_ctx = ctx_with(mq_vram, msg_out_vram, OS_MESG_BLOCK);
            unsafe { osRecvMesg_recomp(rdram_ptr, &mut recv_ctx as *mut _) };
        });
        run_one_step();

        with_executor(|exec| {
            let outcome = exec.send_mesg(0, mq_addr, 0x1234_5678, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });
        run_to_idle_with_yielder_plumbing();

        // Native byte order -- see read_stack_word's doc comment for why
        // MEM_W is a native-endian word access, not big-endian.
        let written = i32::from_ne_bytes(rdram[0x20..0x24].try_into().unwrap());
        assert_eq!(written, 0x1234_5678);
    }

    /// Regression test for the coroutine-context-corruption bug the OoT
    /// boot harness hit (Main-resume SIGBUS, PC=0x1 at its 5th `Woken`
    /// resume, immediately after another thread blocked -- see
    /// `crates/fn64-diff/docs/2026-07-14-first-divergence-report.md`).
    ///
    /// `with_active_yielder` (this file) installed `ACTIVE_YIELDER`/
    /// `ACTIVE_THREAD_ID`/`ACTIVE_RDRAM` exactly ONCE per thread, wrapping
    /// the thread's ENTIRE body closure -- correct for the coroutine's own
    /// suspend calls, but never re-armed on each resume. Because every
    /// `GameThread` coroutine runs on the SAME native OS thread and shares
    /// these `thread_local!` cells, once thread A suspends (its body's
    /// `with_active_yielder` call is paused mid-flight, its restore-to-
    /// `previous` code never runs until the body truly returns), the cells
    /// are left pointing at thread A's `Yielder`/id/rdram. If thread B is
    /// then resumed and calls a `_recomp` shim that suspends, it read
    /// thread A's stale `ACTIVE_YIELDER` and called `Yielder::suspend` on
    /// the WRONG coroutine's handle -- corrupting that coroutine's native
    /// resume context. This reproduces the exact two-thread interleaved
    /// shape: thread A blocks first (on a full send queue), then thread B
    /// blocks (on an empty recv queue) while A is still parked -- mirroring
    /// "thread 18 blocks, then thread 3's next resume lands at PC=0x1."
    /// Both are then woken and MUST each observe their own identity/rdram,
    /// not the other's.
    #[test]
    fn two_threads_blocked_and_woken_interleaved_each_resume_their_own_context() {
        // A full queue (capacity 1, already holds one message) so thread A
        // blocks on send; a separate empty queue so thread B blocks on recv.
        let send_mq_vram: u64 = 0xFFFF_FFFF_8009_0000;
        let send_mq_addr = RdramAddr::from_gpr(send_mq_vram);
        let mut create_send_q = ctx_with(send_mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_send_q as *mut _) };
        with_executor(|exec| {
            assert_eq!(
                exec.send_mesg(0, send_mq_addr, 0xFFFF_FFFF, false),
                SendMesgOutcome::Delivered
            );
        });

        let recv_mq_vram: u64 = 0xFFFF_FFFF_800A_0000;
        let mut create_recv_q = ctx_with(recv_mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_recv_q as *mut _) };

        // Two SEPARATE rdram buffers, each tagged with a value only that
        // thread's own body should ever be able to observe/write, so any
        // cross-wire between the two coroutines' contexts is directly
        // visible rather than needing a crash to notice.
        let mut rdram_a = vec![0u8; 64];
        let rdram_a_ptr = rdram_a.as_mut_ptr();
        let mut rdram_b = vec![0u8; 64];
        let rdram_b_ptr = rdram_b.as_mut_ptr();

        let observed_a = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed_a2 = observed_a.clone();
        let observed_b = std::rc::Rc::new(std::cell::RefCell::new(None));
        let observed_b2 = observed_b.clone();

        const THREAD_A: ThreadId = 500;
        const THREAD_B: ThreadId = 501;

        // Thread A: blocks on osSendMesg (queue already full). Once
        // unblocked, records its OWN thread id (via osGetThreadId_recomp,
        // which reads ACTIVE_THREAD_ID) and writes a marker into ITS OWN
        // rdram buffer at a fixed offset.
        with_executor(|exec| {
            exec.create_thread(THREAD_A, 5, move |yielder, first_input| {
                with_active_yielder(THREAD_A, rdram_a_ptr, yielder, || {
                    let _ = first_input;
                    let mut send_ctx = ctx_with(send_mq_vram, 0xAAAA, OS_MESG_BLOCK);
                    unsafe { osSendMesg_recomp(rdram_a_ptr, &mut send_ctx as *mut _) };
                    let mut id_ctx = ctx_with(0, 0, 0);
                    unsafe { osGetThreadId_recomp(rdram_a_ptr, &mut id_ctx as *mut _) };
                    *observed_a2.borrow_mut() = Some(id_ctx.r2);
                    unsafe {
                        std::ptr::write(rdram_a_ptr.add(0x30) as *mut u32, 0xA0A0_A0A0u32);
                    }
                });
            });
            exec.start_thread(THREAD_A);
        });

        // Thread B: blocks on osRecvMesg (queue empty). Same shape, its own
        // buffer/marker/expected id.
        with_executor(|exec| {
            exec.create_thread(THREAD_B, 5, move |yielder, first_input| {
                with_active_yielder(THREAD_B, rdram_b_ptr, yielder, || {
                    let _ = first_input;
                    let msg_out_vram: u64 = 0xFFFF_FFFF_8000_0040;
                    let mut recv_ctx = ctx_with(recv_mq_vram, msg_out_vram, OS_MESG_BLOCK);
                    unsafe { osRecvMesg_recomp(rdram_b_ptr, &mut recv_ctx as *mut _) };
                    let mut id_ctx = ctx_with(0, 0, 0);
                    unsafe { osGetThreadId_recomp(rdram_b_ptr, &mut id_ctx as *mut _) };
                    *observed_b2.borrow_mut() = Some(id_ctx.r2);
                    unsafe {
                        std::ptr::write(rdram_b_ptr.add(0x30) as *mut u32, 0xB0B0_B0B0u32);
                    }
                });
            });
            exec.start_thread(THREAD_B);
        });

        // Run both threads until they've each hit their blocking yield --
        // thread A blocks first, THEN thread B blocks while A is still
        // parked (the exact "thread 18 blocks while thread 3 is blocked,
        // then thread 3 resumes" interleaving from the divergence report).
        assert!(run_one_step()); // thread A runs, blocks on send
        assert!(run_one_step()); // thread B runs, blocks on recv
        with_executor(|exec| {
            assert!(!exec.is_thread_dead(THREAD_A));
            assert!(!exec.is_thread_dead(THREAD_B));
        });

        // Wake thread A FIRST (drain a slot on its send queue) while
        // ACTIVE_YIELDER/ACTIVE_THREAD_ID are still stale from thread B --
        // B was the last thread whose body actually ran `with_active_
        // yielder`'s install (it started and blocked AFTER A did), so if
        // the install is never re-armed on resume, A's post-wake
        // `osGetThreadId_recomp` call will incorrectly read B's id. This is
        // the precise "thread 18 blocks [here: B], then thread 3 [here: A]
        // resumes into the wrong saved context" interleaving from the
        // divergence report.
        with_executor(|exec| {
            let outcome = exec.recv_mesg(999, send_mq_addr, false);
            assert_eq!(outcome, RecvMesgOutcome::Delivered(0xFFFF_FFFF));
        });
        assert!(run_one_step()); // resumes A only
                                 // Now wake and resume B.
        with_executor(|exec| {
            let recv_addr = RdramAddr::from_gpr(recv_mq_vram);
            let outcome = exec.send_mesg(0, recv_addr, 0xBEEF, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });
        run_to_idle();

        with_executor(|exec| {
            assert!(exec.is_thread_dead(THREAD_A));
            assert!(exec.is_thread_dead(THREAD_B));
        });

        // The actual assertion: each thread must have resumed into ITS OWN
        // saved context -- its own thread id via ACTIVE_THREAD_ID, and its
        // own rdram buffer's marker, never the other thread's.
        assert_eq!(
            *observed_a.borrow(),
            Some(THREAD_A as u64),
            "thread A resumed with a stale/wrong ACTIVE_THREAD_ID -- classic sign of \
             ACTIVE_YIELDER/ACTIVE_THREAD_ID left pointing at whichever thread suspended \
             most recently instead of the thread actually being resumed"
        );
        assert_eq!(
            *observed_b.borrow(),
            Some(THREAD_B as u64),
            "thread B resumed with a stale/wrong ACTIVE_THREAD_ID"
        );
        let marker_a = unsafe { std::ptr::read(rdram_a_ptr.add(0x30) as *const u32) };
        let marker_b = unsafe { std::ptr::read(rdram_b_ptr.add(0x30) as *const u32) };
        assert_eq!(
            marker_a, 0xA0A0_A0A0,
            "thread A must write into its OWN rdram buffer, not thread B's"
        );
        assert_eq!(
            marker_b, 0xB0B0_B0B0,
            "thread B must write into its OWN rdram buffer, not thread A's"
        );
        // Cross-check the buffers weren't swapped/aliased: A's buffer must
        // NOT carry B's marker and vice versa.
        let marker_a_has_b =
            unsafe { std::ptr::read(rdram_a_ptr.add(0x30) as *const u32) } == 0xB0B0_B0B0u32;
        let marker_b_has_a =
            unsafe { std::ptr::read(rdram_b_ptr.add(0x30) as *const u32) } == 0xA0A0_A0A0u32;
        assert!(!marker_a_has_b && !marker_b_has_a);
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
    fn os_get_mem_size_reports_the_real_rdram_size() {
        let mut ctx = ctx_zeroed();
        unsafe { osGetMemSize_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u64);
    }

    #[test]
    fn cache_maintenance_ops_are_safe_callable_noops() {
        unsafe {
            osInvalDCache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
            osInvalICache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
            osWritebackDCache_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _);
        }
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
        let mut ctx = ctx_zeroed();
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        let t0 = ctx.r2;
        with_executor(|exec| exec.advance_time(exec.sim_time() + 500));
        let mut ctx2 = ctx_zeroed();
        unsafe { osGetTime_recomp(std::ptr::null_mut(), &mut ctx2 as *mut _) };
        assert!(
            ctx2.r2 >= t0 + 500,
            "osGetTime must track sim_time advancing, not a fixed value"
        );
    }

    // osStartThread_recomp is a plain `extern "C" fn` -- same subprocess-abort
    // pattern as every other loud-trap test in this file (a panic across an
    // extern "C" boundary aborts, it does not unwind, so `#[should_panic]`
    // would abort the whole test harness rather than being caught).
    #[test]
    fn os_start_thread_with_unregistered_handle_panics_loudly() {
        assert_subprocess_aborts(
            "tests::__os_start_thread_unregistered_handle_abort_subprocess_entry",
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

    #[test]
    fn os_virtual_to_physical_masks_kseg0() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_1234;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_1234);
    }

    #[test]
    fn os_virtual_to_physical_masks_kseg1() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0xA000_5678;
        unsafe { osVirtualToPhysical_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x0000_5678);
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

    #[test]
    fn os_ai_set_frequency_stores_value() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 48000;
        unsafe { osAiSetFrequency_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(AI_FREQUENCY.with(|c| c.get()), 48000);
    }

    #[test]
    fn os_ai_set_next_buffer_then_get_status_reports_busy_once() {
        // Reset MMIO's AI state to a known starting point -- other tests in
        // this file share the same thread_local, and test order is not
        // guaranteed.
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());

        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000; // buf, a plausible vram address
        set_ctx.r5 = 0x100; // size
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };
        assert_eq!(set_ctx.r2, 0, "osAiSetNextBuffer reports success");

        let mut status_ctx = ctx_zeroed();
        unsafe { osAiGetStatus_recomp(std::ptr::null_mut(), &mut status_ctx as *mut _) };
        assert_eq!(
            status_ctx.r2 as u32 & fn64_runtime::AI_STATUS_BUSY,
            fn64_runtime::AI_STATUS_BUSY,
            "first status read after a submit observes busy"
        );

        let mut second_ctx = ctx_zeroed();
        unsafe { osAiGetStatus_recomp(std::ptr::null_mut(), &mut second_ctx as *mut _) };
        assert_eq!(second_ctx.r2, 0, "busy is one-shot");
    }

    #[test]
    fn os_ai_get_length_reports_latched_length() {
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());

        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        set_ctx.r5 = 0x40;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };

        let mut ctx = ctx_zeroed();
        unsafe { osAiGetLength_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, 0x40);
    }

    #[test]
    fn sync_mmio_into_rdram_backs_a_raw_guest_ai_status_load() {
        MMIO.with(|cell| *cell.borrow_mut() = fn64_runtime::MmioSpace::new());
        let mut set_ctx = ctx_zeroed();
        set_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        set_ctx.r5 = 0x40;
        unsafe { osAiSetNextBuffer_recomp(std::ptr::null_mut(), &mut set_ctx as *mut _) };

        let mut buf = vec![0u8; fn64_runtime::RDRAM_MMIO_WINDOW_END as usize];
        unsafe { sync_mmio_into_rdram(buf.as_mut_ptr()) };

        // The exact real address docs/BOOT-NOTES-WM2000.md's LLDB backtrace
        // named: a raw guest lw at AI_STATUS (0xA450000C).
        let ai_status = RdramAddr::from_gpr(0xA450_000C);
        let o = ai_status.offset() as usize;
        let raw = i32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
        assert_eq!(
            raw as u32 & fn64_runtime::AI_STATUS_BUSY,
            fn64_runtime::AI_STATUS_BUSY
        );
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

    // `get_function`/`pause_self`/`switch_error`/`do_break`/the VI-family loud traps and
    // `__osSiRawStartDma_recomp`/`osSpTaskYielded_recomp` are all plain
    // `extern "C" fn`s -- a Rust panic cannot unwind across that boundary
    // and aborts the process instead, so each is verified as a subprocess
    // exit rather than `#[should_panic]`, which requires an in-process
    // catchable unwind and would otherwise abort the whole test harness --
    // same pattern a prior wave established for `osCreateThread_recomp`/
    // `osStartThread_recomp` before this wave wired their real dispatch.
    fn assert_subprocess_aborts(test_name: &str) {
        let exe = std::env::current_exe().expect("current_exe");
        let status = std::process::Command::new(exe)
            .arg("--exact")
            .arg(test_name)
            .arg("--ignored")
            .arg("--nocapture")
            .env("FN64_ABI_RUN_ABORT_CHECK", "1")
            .status()
            .expect("failed to spawn subprocess");
        assert!(
            !status.success(),
            "{test_name} must abort (loud trap), not return successfully"
        );
    }

    #[test]
    fn get_function_miss_panics_naming_the_vram() {
        assert_subprocess_aborts("tests::__get_function_miss_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __get_function_miss_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            get_function(0x1234_5678u32 as i32);
        }
    }

    #[test]
    fn pause_self_outside_any_coroutine_panics_loudly() {
        assert_subprocess_aborts("tests::__pause_self_no_yielder_abort_subprocess_entry");
    }

    #[test]
    #[ignore] // only ever run directly, by the subprocess harness above
    fn __pause_self_no_yielder_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            pause_self(std::ptr::null_mut());
        }
    }

    #[test]
    fn switch_error_always_aborts() {
        assert_subprocess_aborts("tests::__switch_error_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __switch_error_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let func = std::ffi::CString::new("test_func").unwrap();
            unsafe { switch_error(func.as_ptr(), 0x8002_ABCC, 0x8004_B130) };
        }
    }

    #[test]
    fn do_break_always_aborts() {
        assert_subprocess_aborts("tests::__do_break_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __do_break_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            do_break(0x8000_1234);
        }
    }

    /// `__osSiRawStartDma_recomp` is real this wave (replacing the prior
    /// loud trap) -- verifies a port-0 status-query channel (tx_size=1,
    /// rx_size=3) gets `PifModel::query_response(0)`'s real bytes written
    /// back, and that an absent port (1) gets `CONT_ABSENT` set.
    #[test]
    fn os_si_raw_start_dma_fills_real_pif_query_responses() {
        let mut rdram = vec![0u8; 64];
        // Channel 0: tx_size=1, rx_size=3, cmd=0xFF (query), 1 tx byte, then
        // 3 response bytes to be filled at offset 3..6.
        rdram[0] = 1;
        rdram[1] = 3;
        rdram[2] = 0xFF;
        rdram[3] = 0; // the 1 tx byte
                      // rdram[4..7] is the response area for this channel (rx_off = 0+2+1=3,
                      // so response bytes land at 3..6 -- recompute: cursor=0, tx_size=1,
                      // rx_off = cursor+2+tx_size = 0+2+1 = 3, filled 3..6).
                      // Channel 1 starts at cursor = rx_off + rx_size = 3+3 = 6.
        rdram[6] = 1; // tx_size
        rdram[7] = 3; // rx_size
        rdram[8] = 0xFF; // cmd
        rdram[9] = 0; // tx byte
                      // response area for channel 1: rx_off = 6+2+1=9, filled 9..12.
        rdram[12] = 0xFF; // end-of-block marker, channel 2 onward absent

        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_0000; // dramAddr vram -> rdram offset 0
        unsafe { __osSiRawStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Port 0: standard controller, no pak, not absent.
        assert_eq!(&rdram[3..6], &[0x05, 0x00, 0x00]);
        // Port 1: absent bit set.
        assert_eq!(
            rdram[9 + 2] & fn64_runtime::CONT_ABSENT,
            fn64_runtime::CONT_ABSENT
        );
    }

    #[test]
    fn os_sp_task_yielded_records_gfx_task_and_acks_complete() {
        let mut rdram = vec![0u8; 128];
        // OSTask_t header at offset 0x10 (mirrors the real call site's
        // s1+0x10 addressing): type = M_GFXTASK at +0x0.
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 1, "task reported complete, not yielded");
        with_executor(|exec| {
            assert_eq!(exec.task_log().gfx_count(), 1);
            assert_eq!(exec.task_log().audio_count(), 0);
        });
    }

    /// Proves the executor gfx-task seam actually reaches a real `dyn
    /// RenderBackend` end-to-end: `set_render_backend` registers a real
    /// `fn64_render_rt64::ReferenceBackend`, a real F3DEX2-family display
    /// list (same tiny triangle fixture shape as
    /// `fn64-render-rt64/tests/fixture_replay.rs` -- see that file's doc
    /// comment for why this is a hand-built, not ROM-captured, fixture) is
    /// planted in the SAME `rdram` buffer `osSpTaskYielded_recomp` reads
    /// its task header from, and the call is made through the real
    /// `extern "C"` shim, not by calling the backend directly. This is the
    /// "wire the executor gfx-task seam" gate: the FULL path (recomp shim
    /// -> registered `dyn RenderBackend` -> rasterizer -> framebuffer) is
    /// exercised, not just its two halves in isolation.
    #[test]
    fn os_sp_task_yielded_routes_gfx_tasks_through_the_registered_render_backend() {
        use fn64_render::RenderConfig;
        use fn64_render_rt64::{gbi, ReferenceBackend};

        const RDRAM_LEN: usize = 0x4000;
        const VTX_ADDR: usize = 0x1000;
        const DL_ADDR: usize = 0x2000;
        const HEADER_OFF: usize = 0x10;

        let mut rdram = vec![0u8; RDRAM_LEN];

        // Same 3-vertex red/green/blue triangle shape as the
        // fn64-render-rt64 fixture: SDK's public 16-byte Vtx_t
        // position-color layout.
        let verts: [([i16; 2], [u8; 4]); 3] = [
            ([8, 8], [255, 0, 0, 255]),
            ([56, 8], [0, 255, 0, 255]),
            ([32, 56], [0, 0, 255, 255]),
        ];
        for (i, (xy, rgba)) in verts.iter().enumerate() {
            let off = VTX_ADDR + i * 16;
            rdram[off..off + 2].copy_from_slice(&xy[0].to_be_bytes());
            rdram[off + 2..off + 4].copy_from_slice(&xy[1].to_be_bytes());
            rdram[off + 12..off + 16].copy_from_slice(rgba);
        }

        let mut dl = Vec::new();
        let w0 = ((gbi::G_VTX as u32) << 24) | (3u32 << 12);
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&(VTX_ADDR as u32).to_be_bytes());
        let w0 = (gbi::G_TRI1 as u32) << 24;
        let w1 = (1u32 << 8) | 2u32; // v0 index is 0, so its <<16 term is omitted (identity op)
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&w1.to_be_bytes());
        let w0 = (gbi::G_ENDDL as u32) << 24;
        dl.extend_from_slice(&w0.to_be_bytes());
        dl.extend_from_slice(&0u32.to_be_bytes());
        rdram[DL_ADDR..DL_ADDR + dl.len()].copy_from_slice(&dl);

        // OSTask_t header: type=M_GFXTASK@0x0, data_ptr=DL_ADDR@0x30.
        rdram[HEADER_OFF..HEADER_OFF + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        rdram[HEADER_OFF + 0x30..HEADER_OFF + 0x34]
            .copy_from_slice(&(DL_ADDR as u32).to_ne_bytes());

        let mut backend = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        backend.create(&RenderConfig::new(64, 64)).unwrap();
        set_render_backend(Box::new(backend), RDRAM_LEN);

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 1, "task reported complete, not yielded");
        assert_eq!(
            last_render_error(),
            None,
            "the real backend must not report an error for a valid fixture -- rules out \
             NotReady/UnsupportedUcode/InvalidTaskBounds, i.e. the seam-routed call really \
             reached process_task and it really succeeded"
        );

        // `dyn RenderBackend` deliberately has no `Any` bound (keeping the
        // shared trait minimal per docs/DECOUPLING.md), so the registered
        // trait object's framebuffer can't be inspected back out through
        // this seam. Independently confirm the exact same fixture bytes
        // DO produce a non-clear frame via a second, directly-owned
        // `ReferenceBackend` (the same concrete type just registered,
        // exercised the same way `fn64-render-rt64/tests/fixture_replay.rs`
        // already proves in isolation) -- combined with the error-free
        // `ctx.r2 == 1` result above, this closes the loop end-to-end:
        // the seam call really executed the real decode+rasterize path on
        // this fixture, not a silent no-op.
        let mut direct = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        direct.create(&RenderConfig::new(64, 64)).unwrap();
        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: DL_ADDR as u32,
            ..Default::default()
        };
        direct.process_task(&rdram, &task).unwrap();
        assert!(
            direct
                .framebuffer()
                .unwrap()
                .has_non_uniform_content(1, 2, 3, 255),
            "the same fixture bytes must produce a non-clear frame through the reference backend"
        );
    }

    #[test]
    fn os_sp_task_yielded_calls_the_registered_audio_ucode_fn_for_real() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        static SEEN_UCODE_ADDR: AtomicU32 = AtomicU32::new(0);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, ucode_addr: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_UCODE_ADDR.store(ucode_addr, Ordering::SeqCst);
            0
        }
        unsafe { set_audio_ucode_fn(fake_ucode) };

        let mut rdram = vec![0u8; 128];
        let header_off = 0x20usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        rdram[header_off + 0x10..header_off + 0x14].copy_from_slice(&0xDEADu32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(
            CALLED.load(Ordering::SeqCst),
            "real ucode fn must be called for M_AUDTASK"
        );
        assert_eq!(SEEN_UCODE_ADDR.load(Ordering::SeqCst), 0xDEAD);
        with_executor(|exec| assert!(exec.task_log().audio_count() >= 1));
    }

    #[test]
    fn os_sp_task_yielded_routes_m_audtask_to_the_registered_audio_backend() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        /// A cpal-less fake `AudioBackend` -- proves the seam really
        /// reaches a registered `dyn AudioBackend` (not just that
        /// `AUDIO_UCODE_FN` was called), mirroring
        /// `render_backend_dyn_dispatch_reaches_the_registered_reference_backend`'s
        /// shape for the gfx seam. Not `fn64_audio::CpalBackend` itself --
        /// a real audio device isn't guaranteed in a test/CI sandbox.
        struct CountingBackend {
            ready: bool,
            frames_seen: Arc<AtomicU32>,
        }
        impl AudioBackend for CountingBackend {
            fn create(&mut self, _cfg: &fn64_audio::AudioConfig) -> Result<(), AudioError> {
                self.ready = true;
                Ok(())
            }
            fn queue_samples(&mut self, samples: &[i16]) -> Result<(), AudioError> {
                if !self.ready {
                    return Err(AudioError::NotReady("create() not called"));
                }
                self.frames_seen
                    .fetch_add(samples.len() as u32 / 2, Ordering::SeqCst);
                Ok(())
            }
            fn frames_remaining(&self) -> Result<u32, AudioError> {
                Ok(self.frames_seen.load(Ordering::SeqCst))
            }
            fn set_frequency(&mut self, _sample_rate_hz: u32) {}
        }

        const RDRAM_LEN: usize = 4096;
        let mut rdram = vec![0u8; RDRAM_LEN];
        const HEADER_OFF: usize = 0x40;
        const AI_BUF_OFF: usize = 0x800; // arbitrary in-bounds output buffer
        const AI_BUF_FRAMES: u32 = 8; // 8 stereo frames = 16 i16 samples = 32 bytes
        rdram[HEADER_OFF..HEADER_OFF + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        rdram[HEADER_OFF + 0x28..HEADER_OFF + 0x2C]
            .copy_from_slice(&(AI_BUF_OFF as u32).to_ne_bytes()); // output_buff
        rdram[HEADER_OFF + 0x2C..HEADER_OFF + 0x30]
            .copy_from_slice(&(AI_BUF_FRAMES * 2 * 2).to_ne_bytes()); // output_buff_size (bytes, 2 channels * 2 bytes/sample)

        let frames_seen = Arc::new(AtomicU32::new(0));
        let mut backend = CountingBackend {
            ready: false,
            frames_seen: Arc::clone(&frames_seen),
        };
        backend
            .create(&fn64_audio::AudioConfig::new(32000, 2))
            .unwrap();
        set_audio_backend(Box::new(backend), RDRAM_LEN);

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(ctx.r2, 1, "task reported complete, not yielded");
        assert_eq!(
            last_audio_error(),
            None,
            "the real backend must not report an error for a valid fixture -- rules out \
             NotReady, i.e. the seam-routed call really reached queue_samples and it really \
             succeeded"
        );
        assert_eq!(
            frames_seen.load(Ordering::SeqCst),
            AI_BUF_FRAMES,
            "queue_samples must really have been called with the task's declared AI buffer, \
             not skipped"
        );
    }

    #[test]
    fn os_vi_set_mode_stores_mode_ptr_and_swap_buffer_updates_current_framebuffer() {
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8004_1234;
        unsafe { osViSetMode_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        with_executor(|exec| assert_eq!(exec.vi().mode_ptr, Some(0x8004_1234)));

        let mut swap_ctx = ctx_zeroed();
        swap_ctx.r4 = 0xFFFF_FFFF_8010_0000;
        unsafe { osViSwapBuffer_recomp(std::ptr::null_mut(), &mut swap_ctx as *mut _) };
        assert_eq!(current_vi_framebuffer(), Some(0x10_0000));
        assert_eq!(vi_swap_count(), 1);
    }

    /// Regression test for the real double-KSEG0-translation bug
    /// `examples/wm2000-boot`'s boot run surfaced (a genuine
    /// EXC_BAD_ACCESS deep in `osEPiStartDma_recomp`'s field reads, once
    /// boot finally reached its first real PI DMA on thread 6): `mb_addr`
    /// is placed at a REALISTIC nonzero vram address (not offset 0, which
    /// would hide the bug -- 0 minus 0 is still 0), and the OSIoMesg
    /// fields are placed at their real rdram offsets relative to that vram
    /// address, not relative to 0.
    #[test]
    fn os_epi_start_dma_reads_real_fields_at_a_nonzero_mb_address() {
        // Use a fresh ROM per test (with_pi_dma's HOST state is thread-local
        // per test since each #[test] gets its own OS thread by default).
        load_rom(vec![0xABu8; 0x1000]);

        let mut rdram = vec![0u8; 0x10000];
        let mb_vram: u64 = 0x8000_2000; // a REAL, nonzero vram address
        let mb_offset = 0x2000usize;

        // OSIoMesg fields at mb_offset + {0x4 (retQueue), 0x8 (retMesg),
        // 0xC (dramAddr), 0x10 (devAddr), 0x14 (size)} -- native byte order,
        // per this wave's MEM_W correction.
        let dram_target_vram: u32 = 0x8000_5000;
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes()); // no retQueue
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&dram_target_vram.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&0x10u32.to_ne_bytes()); // devAddr
        rdram[mb_offset + 0x14..mb_offset + 0x18].copy_from_slice(&4u32.to_ne_bytes()); // len

        let mut ctx = ctx_zeroed();
        ctx.r5 = mb_vram;
        ctx.r6 = 0; // OS_READ / ToRdram
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // dramAddr (0x8000_5000) -> rdram offset 0x5000; the DMA should
        // have copied 4 bytes of 0xAB from ROM offset 0x10 there. Reaching
        // this point at all (no EXC_BAD_ACCESS) already proves mb_addr's
        // fields were read from the CORRECT (non-double-translated)
        // offset; the copied bytes confirm the DMA itself used the right
        // dramAddr/devAddr/len too.
        assert_eq!(&rdram[0x5000..0x5004], &[0xAB, 0xAB, 0xAB, 0xAB]);
    }

    /// Regression test for the real infinite-loop bug `examples/wm2000-boot`
    /// surfaced (2026-07-14): `osEPiStartDma_recomp` never wrote `ctx.r2`
    /// ($v0), so NWXE's chunked-DMA caller (`func_80000660`, asm
    /// 0x800006E4-0x800006FC: `bne $v0, $zero, L_800006E4`) read whatever
    /// stale value `r2` already held and looped forever instead of falling
    /// through to `osRecvMesg`. Seed `ctx.r2` with a realistic STALE
    /// NON-ZERO value beforehand (mirroring the real caller's register
    /// state at the call site) so a regression that stops writing `ctx.r2`
    /// would fail this test even though a zero-initialized `ctx` would
    /// have hidden the bug.
    #[test]
    fn os_epi_start_dma_writes_zero_return_value_even_with_stale_nonzero_r2() {
        load_rom(vec![0xCDu8; 0x1000]);

        let mut rdram = vec![0u8; 0x10000];
        let mb_offset = 0x2000usize;
        rdram[mb_offset + 0x4..mb_offset + 0x8].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0xC..mb_offset + 0x10].copy_from_slice(&0x8000_5000u32.to_ne_bytes());
        rdram[mb_offset + 0x10..mb_offset + 0x14].copy_from_slice(&0u32.to_ne_bytes());
        rdram[mb_offset + 0x14..mb_offset + 0x18].copy_from_slice(&4u32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r5 = 0x8000_2000;
        ctx.r6 = 0; // OS_READ / ToRdram
        ctx.r2 = 0x1234; // stale non-zero, as a real caller's register would hold
        unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "osEPiStartDma_recomp must overwrite $v0 with 0 (success) on every \
             synchronous-completion path, or NWXE's chunked-DMA retry loop spins forever"
        );
    }

    #[test]
    fn os_epi_start_dma_without_a_loaded_rom_is_a_loud_named_trap() {
        assert_subprocess_aborts("tests::__os_epi_start_dma_no_rom_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_epi_start_dma_no_rom_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            // mb points at an all-zero rdram region -> ret_queue==0 (no
            // completion post attempted), dev_addr==0, len==0 -- the load-
            // bearing assertion here is that with_pi_dma panics because no
            // ROM was ever installed in this fresh subprocess, not that the
            // (deliberately trivial) transfer parameters are realistic.
            let mut ctx = ctx_zeroed();
            let mut rdram = vec![0u8; 64];
            ctx.r5 = 0; // mb address 0
            ctx.r6 = 0; // direction = ToRdram
            unsafe { osEPiStartDma_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        }
    }
}
