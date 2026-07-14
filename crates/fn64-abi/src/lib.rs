//! fn64-abi: the extern "C" surface `RecompiledFuncs/*.c` links against.
//!
//! See `docs/DESIGN.md` section 1: this crate is deliberately thin --
//! every symbol here is a signature-and-marshalling adapter over
//! `fn64-runtime`, never a place new policy gets invented. Signatures are
//! transcribed from `aki-recomp/runtime/ABI-SURFACE.md`:
//!
//! - `recomp_context`'s field layout (section (b), from `recomp.h`, MIT):
//!   every `RECOMP_FUNC`/`_recomp` shim takes `(uint8_t* rdram,
//!   recomp_context* ctx)`; `ctx->r4`/`r5`/`r6`/`r7` are `a0`/`a1`/`a2`/`a3`
//!   per MIPS o32 calling convention (recomp_context's own gpr field order,
//!   section (b)) -- the first four integer arguments. A 5th/6th argument
//!   (needed by `osCreateThread`'s 6-arg signature) is stack-passed under
//!   o32: the caller reserves a 4-word "register save area" at
//!   `sp+0x10..sp+0x1C` (public MIPS o32 ABI convention, not vendor-
//!   specific) and stores args 5+ there; `ctx->r29` is `sp` (section (b)
//!   confirms `r29` is a real dereferenced field), so a 5th/6th arg is read
//!   via `rdram[sp+0x10]`/`rdram[sp+0x14]`, exactly like any other
//!   rdram-relative read this crate performs.
//! - `pause_self` (section (a), recomp.h dispatch helpers table: NWXE x3,
//!   NW4E x2 call sites) -- the yield primitive `docs/DESIGN.md` section 2
//!   models as a stackful-coroutine `yield_now()`, now wired to the real
//!   `fn64_runtime::Executor` (wave 2 is complete).
//! - `osCreateMesgQueue_recomp` / `osSendMesg_recomp` / `osRecvMesg_recomp`
//!   / `osCreateThread_recomp` / `osStartThread_recomp` (section (a), the
//!   `_recomp` shim inventory) -- the thread-lifecycle and message-queue
//!   shims `docs/DESIGN.md` section 2 discusses in the most depth (rung
//!   12's reset invariant; rung 18b's root cause in the blocking-send
//!   path; rung 14's pause_self idle-loop fix).
//!
//! ## The executor integration
//!
//! Exactly one `fn64_runtime::Executor` exists per process, in a
//! `thread_local!` (not a true global -- keeps this crate's tests
//! independent of each other and matches `docs/DESIGN.md` section 2's
//! "single executor, one host thread" model: there is never a second
//! executor instance actually driving guest code concurrently with the
//! first, so a thread-local is the correct scope, not a weakening of the
//! single-executor guarantee). Every shim below reaches the executor
//! through `with_executor`, the ONE accessor in this module -- there is no
//! second way to reach `Executor`'s state from this crate, matching
//! `docs/DESIGN.md` section 2's "nothing outside the executor's own module
//! can touch queue/thread state" for the ABI layer's side of that seam.
//!
//! `pause_self`/blocking `osRecvMesg`/blocking `osSendMesg` all need to
//! call `Yielder::suspend` on the CURRENTLY EXECUTING coroutine's own
//! stack -- something only reachable from inside that coroutine's body,
//! never from an `Executor` method called by outside code. Real
//! recompiled C calls a `_recomp` shim as an ordinary synchronous function
//! from inside its own thread's call graph, so by construction every shim
//! below IS running on the coroutine it might need to suspend; `corosensei`
//! exposes the current coroutine's `Yielder` via
//! `Yielder::on_stack`/thread-local lookup is not part of its public API,
//! so this crate threads the active `Yielder` through a second, narrower
//! thread-local (`ACTIVE_YIELDER`) that the executor's `run_one_step`
//! populates for the duration of a `resume()` call -- see
//! `with_active_yielder` below. This is the ONE additional piece of
//! plumbing the ABI layer owns that `fn64-runtime` does not: `fn64-runtime`
//! only knows "the coroutine yielded `Yield::PauseSelf`," it has no
//! opinion on how a Rust closure obtains a `Yielder` to call `.suspend()`
//! with in the first place.
//!
//! ### A real reentrancy bug this design had to close
//!
//! An earlier version of this module had `osSendMesg_recomp`/
//! `osRecvMesg_recomp` call `with_executor` (i.e. `EXECUTOR.borrow_mut()`)
//! from inside the coroutine BODY to pre-check "would this block" before
//! deciding whether to suspend. That is a real, reproducible bug (caught
//! by this crate's own tests, not merely reasoned about): the coroutine
//! body executes on the stack INSIDE `Executor::run_one_step`'s call to
//! `GameThread::resume`, which itself runs inside the OUTER
//! `with_executor` call that invoked `run_one_step` in the first place --
//! so the pre-check's `borrow_mut()` panics with "RefCell already
//! borrowed" the instant any coroutine calls a queue-touching shim. This
//! is exactly the shape of bug this project's whole clean-room rationale
//! exists to catch mechanically rather than by convention: a hidden
//! reentrant caller through an API that looked like a plain accessor.
//!
//! The fix, applied throughout this file: a coroutine body NEVER calls
//! `with_executor` (directly or transitively) for anything that needs to
//! inspect or mutate queue/thread state -- it only ever calls
//! `suspend_active_coroutine`, unconditionally, for every potentially-
//! blocking operation, and lets `fn64_runtime::executor`'s `handle_yield`
//! (which already holds the executor's `&mut self` at the one call site
//! that resumed this coroutine, per `docs/DESIGN.md` section 2's "two
//! steps of one sequential function") decide immediate-delivery vs.
//! real-block. `handle_yield`'s `BlockOnRecv`/`BlockOnSend` arms already
//! implement exactly this "check first, block only if truly not ready"
//! logic (see `fn64-runtime/src/executor.rs`), so the ABI layer doesn't
//! need its own pre-check at all -- duplicating it was both redundant and
//! the source of the reentrancy. The one non-blocking shim that still
//! needs the executor (`osCreateMesgQueue_recomp`, `osSetEventMesg_recomp`,
//! `osSetThreadPri_recomp`) is safe because those are NOT called with a
//! coroutine resume already on the stack in the failure shape above -- but
//! see `current_thread_id` below for how even "which thread am I" is
//! answered without an executor borrow, closing the hazard for good rather
//! than relying on "this particular call site happens not to nest today."
//!
//! This module intentionally does NOT yet implement `osCreateThread`'s
//! entry-point dispatch (calling the real recompiled function the thread
//! should run) -- that requires the overlay/`get_function` lookup table
//! (`docs/DESIGN.md` section 1's `FuncEntry`/`SectionTableEntry`, wave 3's
//! last item), which is a separate, not-yet-landed piece of work. Where
//! that's the gap, this file says so with a loud, named
//! `unimplemented!()` (per `AGENTS.md`'s "loud traps, no silent shrugs")
//! rather than faking a callable stub.

use std::cell::{Cell, RefCell};

use corosensei::Yielder;
use fn64_runtime::{Executor, ExternalEvent, Mesg, Priority, RdramAddr, Resume, ThreadId, Yield};

/// MIPS `recomp_context`, field layout per ABI-SURFACE.md section (b),
/// verbatim struct order from `refs/N64RecompSource/include/recomp.h`
/// (MIT). Only the fields generated code is documented to actually
/// dereference (section (b)'s "fields_actually_touched_by_generated_code")
/// are given real storage; the rest of the real struct (fpr regs, hi/lo,
/// f_odd, status_reg, mips3_float_mode) is out of scope for the symbols
/// implemented here and omitted rather than faked -- a future wave adding
/// a symbol that touches them extends this struct then, with its own
/// citation. `r29` (sp) is included (added this wave) because
/// `osCreateThread`'s stack-passed 5th/6th arguments require it -- see
/// module doc.
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
    pub r29: u64,
}

thread_local! {
    /// The one executor instance -- see module doc's "The executor
    /// integration" section for why a thread-local (not a bare global) is
    /// the correct scope and does not weaken the single-executor
    /// guarantee.
    static EXECUTOR: RefCell<Executor> = RefCell::new(Executor::new());

    /// The `Yielder` for whichever coroutine is currently being resumed,
    /// populated only for the duration of a `resume()` call -- see module
    /// doc. `Cell<Option<*const ()>>` stores a type-erased pointer because
    /// `Yielder<Resume, Yield>`'s concrete type isn't `'static`-nameable
    /// here without threading generics through every shim; the pointer is
    /// only ever dereferenced back to the exact same `Yielder<Resume,
    /// Yield>` type it was created as, for the lifetime of the single
    /// `with_active_yielder` call that installed it (never stored past
    /// that call returning), which is what makes the cast sound.
    static ACTIVE_YIELDER: Cell<Option<*const Yielder<Resume, Yield>>> = const { Cell::new(None) };

    /// Which `ThreadId` is the currently-resumed coroutine, mirroring
    /// `ACTIVE_YIELDER`'s scope exactly (installed/restored by the same
    /// `with_active_yielder` call). Answering "which thread am I" this way
    /// -- rather than asking `Executor::current_thread()`, which would
    /// require an executor borrow from inside the coroutine body -- is
    /// what closes the reentrancy hazard described in the module doc for
    /// good: a coroutine body never needs to touch `EXECUTOR` at all for
    /// its own identity.
    static ACTIVE_THREAD_ID: Cell<Option<ThreadId>> = const { Cell::new(None) };
}

fn with_executor<R>(f: impl FnOnce(&mut Executor) -> R) -> R {
    EXECUTOR.with(|e| f(&mut e.borrow_mut()))
}

/// Install `yielder`/`thread_id` as the active ones for the duration of
/// `f`. Called from exactly one place conceptually -- the trampoline that
/// resumes a coroutine's body (see `tests` below for how the test harness
/// stands in for that trampoline, since wave 3's real `osCreateThread`
/// entry-point dispatch, per module doc, isn't wired yet). Restores the
/// previous values on exit (supports nested calls faithfully, though in
/// this crate's current single-executor-thread model nesting never
/// actually occurs).
pub fn with_active_yielder<R>(
    thread_id: ThreadId,
    yielder: &Yielder<Resume, Yield>,
    f: impl FnOnce() -> R,
) -> R {
    let ptr = yielder as *const Yielder<Resume, Yield>;
    let previous_yielder = ACTIVE_YIELDER.with(|cell| cell.replace(Some(ptr)));
    let previous_id = ACTIVE_THREAD_ID.with(|cell| cell.replace(Some(thread_id)));
    let result = f();
    ACTIVE_YIELDER.with(|cell| cell.set(previous_yielder));
    ACTIVE_THREAD_ID.with(|cell| cell.set(previous_id));
    result
}

/// The `ThreadId` of the coroutine currently executing a `_recomp` shim.
/// Never touches `EXECUTOR` -- see module doc's reentrancy note. No shim in
/// this file needs its own thread id yet (delivery/blocking decisions are
/// keyed by queue address, and the executor's `handle_yield` already knows
/// which `GameThread` yielded from its own bookkeeping) -- kept `pub(crate)`
/// for the next shim that does (e.g. a future `osSetTimer_recomp`
/// attributing `armed_by`), rather than deleted and rediscovered.
#[allow(dead_code)]
fn current_thread_id(shim: &str) -> ThreadId {
    ACTIVE_THREAD_ID.with(|cell| cell.get()).unwrap_or_else(|| {
        panic!(
            "{shim}: no active thread id installed -- this _recomp shim was called from \
             outside a resumed coroutine's body (see with_active_yielder)"
        )
    })
}

/// Suspend the currently-active coroutine with `yield_value`, per module
/// doc's "The executor integration." Panics loudly (never a silent no-op)
/// if called outside `with_active_yielder`'s scope -- i.e. from code that
/// isn't actually running as a resumed coroutine body, which would
/// otherwise be a `_recomp` shim silently failing to yield at all (exactly
/// the class of bug rung 14 was: "never yields... nothing else ever runs
/// again").
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
    // Safety: `ptr` was installed by `with_active_yielder` and is only
    // ever non-None for the dynamic extent of that function's `f()` call,
    // on the same thread (thread-local); this function is only reachable
    // from inside that extent (a `_recomp` shim called from a resumed
    // coroutine body), so the pointee is guaranteed live for this call.
    let yielder = unsafe { &*ptr };
    yielder.suspend(yield_value)
}

/// `pause_self` (recomp.h dispatch helper, ABI-SURFACE.md section (a)).
/// The yield primitive `docs/DESIGN.md` section 2 designates as a
/// stackful-coroutine `yield_now()` call -- wired for real to the
/// executor's `Yield::PauseSelf` path (rung 14's fix: a spin loop that
/// calls this every iteration always gives up the CPU, never starves
/// anything).
#[no_mangle]
pub extern "C" fn pause_self(_rdram: *mut u8, _ctx: *mut RecompContext) {
    suspend_active_coroutine(Yield::PauseSelf);
}

/// `osCreateMesgQueue_recomp` (ABI-SURFACE.md section (a): NWXE x20 call
/// sites currently named). MIPS signature `osCreateMesgQueue(OSMesgQueue
/// *mq, OSMesg *msg, s32 count)` -- `a0`=mq (`ctx->r4`), `a1`=msg
/// (`ctx->r5`), `a2`=count (`ctx->r6`), per o32 calling convention and
/// `recomp_context`'s gpr field order (section (b)).
///
/// Always produces a genuinely empty queue (`Executor::create_mesg_queue`,
/// backed by `fn64_runtime::MesgQueue::new`) -- this is rung 12's
/// load-bearing reset, see `docs/DESIGN.md` section 2 and 3, and the
/// `rung_12_*` tests in `fn64-runtime/tests/rung_regressions.rs`: there is
/// no path here that could leave a stale/sentinel value in a blocked list,
/// including at a REUSED queue address, because `create_mesg_queue`
/// unconditionally replaces whatever was there.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext`, as
/// every `RECOMP_FUNC`/`_recomp` shim's caller (N64Recomp-generated C) is
/// contractually required to pass (ABI-SURFACE.md section (b)/(a)).
#[no_mangle]
pub unsafe extern "C" fn osCreateMesgQueue_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let count = ctx.r6 as usize;
    with_executor(|exec| exec.create_mesg_queue(mq_addr, count.max(1)));
}

/// `osSendMesg_recomp` (ABI-SURFACE.md section (a): NWXE x27 call sites
/// currently named). MIPS signature `osSendMesg(OSMesgQueue *mq, OSMesg
/// msg, s32 flag)` -- `a0`=mq (`ctx->r4`), `a1`=msg (`ctx->r5`), `a2`=flag
/// (`ctx->r6`, `OS_MESG_BLOCK`/`OS_MESG_NOBLOCK`).
///
/// This is rung 18b's exact root-cause path (`docs/DESIGN.md` section 2):
/// the reference runtime's crash was eventually traced to a genuinely
/// concurrent second host thread's `osSendMesg` blocking-insert on a
/// shared queue struct, invisible to the scheduler's own lock. Here the
/// blocking path goes through `Executor::send_mesg` (a check on the single
/// executor thread) and, if it must actually block, suspends THIS
/// coroutine with `Yield::BlockOnSend` -- see the `rung_18_*` tests in
/// `fn64-runtime/tests/rung_regressions.rs` for why no concurrent writer
/// can ever observe an inconsistent in-between state here: there is no
/// second host thread to do so.
///
/// # Safety
/// `ctx` must be a valid, non-null pointer to a live `RecompContext` (same
/// contract as `osCreateMesgQueue_recomp` above).
#[no_mangle]
pub unsafe extern "C" fn osSendMesg_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg: Mesg = ctx.r5 as u32;
    let may_block = ctx.r6 == OS_MESG_BLOCK;

    // Unconditionally suspend -- NEVER pre-check via `with_executor` from
    // here (see module doc's reentrancy note): this coroutine body is
    // already running on the stack the executor's own `&mut self` borrow
    // holds open, so any call back into `EXECUTOR` from this point would
    // panic on a re-borrow. `fn64_runtime::executor`'s `handle_yield`
    // performs the real check-then-deliver-or-block logic (and, for
    // `may_block: false`, the real check-then-drop) from the one place
    // that safely holds `&mut Executor` -- see that module's `BlockOnSend`
    // arm.
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

/// `OS_MESG_NOBLOCK`/`OS_MESG_BLOCK`, per the public libultra manual's
/// Message Manager section. `NOBLOCK` has no non-test call site in this
/// crate yet (no shim currently exercises the drop-on-full-queue path
/// outside a test), but it's the real, documented flag value, not a
/// leftover -- kept `pub(crate)` and allowed rather than deleted so the
/// next `_recomp` shim reached that passes flag=0 doesn't have to
/// rediscover this constant.
#[allow(dead_code)]
const OS_MESG_NOBLOCK: u64 = 0;
const OS_MESG_BLOCK: u64 = 1;

/// `osRecvMesg_recomp` (ABI-SURFACE.md section (a): NWXE x71, NW4E x74
/// call sites -- the single most-called `_recomp` shim in the corpus).
/// MIPS signature `osRecvMesg(OSMesgQueue *mq, OSMesg *msg, s32 flag)` --
/// `a0`=mq (`ctx->r4`), `a1`=msg-out-pointer (`ctx->r5`), `a2`=flag
/// (`ctx->r6`).
///
/// On delivery, writes the received message to `*msg` via `MEM_W` (int32
/// width, matching `OSMesg`'s `void*`-sized-but-word-stored real layout on
/// this 32-bit-pointer target) -- the same rdram-write path every other
/// rdram mutation in this crate uses, per `docs/DESIGN.md` section 3.
///
/// # Safety
/// `rdram`/`ctx` must be valid per the same contract as every other shim
/// in this file.
#[no_mangle]
pub unsafe extern "C" fn osRecvMesg_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let mq_addr = RdramAddr::from_gpr(ctx.r4);
    let msg_out_addr = RdramAddr::from_gpr(ctx.r5);
    let may_block = ctx.r6 == OS_MESG_BLOCK;

    // Unconditionally suspend -- see osSendMesg_recomp's comment and
    // module doc's reentrancy note: no `with_executor` pre-check from
    // inside a coroutine body. `handle_yield`'s `BlockOnRecv` arm performs
    // the real rung-12-respecting check-then-block-or-deliver-or-drop
    // logic once it observes this yield.
    let delivered = match suspend_active_coroutine(Yield::BlockOnRecv { mq_addr, may_block }) {
        Resume::Delivered(msg) => Some(msg),
        Resume::WouldBlock => None,
        other => panic!(
            "osRecvMesg_recomp: resumed from a BlockOnRecv yield with an unexpected Resume \
             variant {other:?} -- the executor must always resume a recv-blocked coroutine \
             with either Resume::Delivered or Resume::WouldBlock"
        ),
    };

    if let Some(msg) = delivered {
        // Real osRecvMesg writes 0 if msg_out is NULL (OSMesg* may be
        // passed as NULL to just wait/consume); guard the write, don't
        // silently corrupt rdram address 0. This shim does not own an
        // `Rdram` instance -- `rdram` IS the shared buffer per
        // docs/DESIGN.md section 3, borrowed for the duration of this
        // call like every `RECOMP_FUNC` receives it -- so the write below
        // replicates `Rdram::write_w`'s exact semantics (word-aligned, no
        // byte-lane XOR, big-endian) directly against the raw pointer
        // rather than constructing a second, competing `Rdram` over
        // borrowed memory.
        if msg_out_addr.offset() != 0 {
            let o = msg_out_addr.offset() as usize;
            unsafe {
                std::ptr::copy_nonoverlapping((msg as i32).to_be_bytes().as_ptr(), rdram.add(o), 4);
            }
        }
    }
}

/// `osCreateThread_recomp` (ABI-SURFACE.md section (a): NWXE x5, NW4E x4
/// call sites). MIPS signature `osCreateThread(OSThread *t, OSId id, void
/// (*entry)(void *), void *arg, void *sp, OSPri pri)` -- `a0`=t (unused
/// here beyond being the thread's rdram-side handle; this crate identifies
/// threads by `OSId`, not by `t`'s address, since `Executor` has no notion
/// of the `t` struct's rdram layout), `a1`=id (`ctx->r5`), `a2`=entry
/// (`ctx->r6`), `a3`=arg (`ctx->r7`), and the o32-stack-passed 5th/6th args
/// `sp`/`pri` read from `rdram[ctx.r29 + 0x10]`/`rdram[ctx.r29 + 0x14]` --
/// see module doc's "stack-passed argument" note.
///
/// Does NOT yet dispatch to the real recompiled `entry` function pointer:
/// that requires the overlay/`get_function` lookup table (`docs/DESIGN.md`
/// section 1's `FuncEntry`, wave 3's last item), which doesn't exist in
/// this crate yet. Per `AGENTS.md`'s "loud traps, no silent shrugs," this
/// stays a named `unimplemented!()` rather than creating a thread whose
/// body silently does nothing (which would let boot "progress" while
/// actually running no guest code at all -- a worse failure than refusing
/// to proceed).
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osCreateThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let _id: ThreadId = ctx.r5 as u32;
    let _entry_vram = ctx.r6 as u32;
    let _arg = ctx.r7;
    unimplemented!(
        "osCreateThread_recomp: real dispatch to the recompiled entry point at vram {:#x} \
         requires the overlay/get_function lookup table (docs/DESIGN.md section 1's FuncEntry, \
         wave 3's last item), which is not wired into fn64-abi yet. This must stay a loud, \
         named panic rather than silently creating a thread with an empty/no-op body, per \
         AGENTS.md -- a game 'booting' past this point while running no guest code at all would \
         be a worse, harder-to-diagnose failure than refusing to proceed.",
        _entry_vram
    );
}

/// `osStartThread_recomp` (ABI-SURFACE.md section (a): NWXE x6, NW4E x4
/// call sites). MIPS signature `osStartThread(OSThread *t)` -- `a0`=t
/// (`ctx->r4`). Same real-dispatch gap as `osCreateThread_recomp` above:
/// there is no `GameThread` to start without a real entry-point trampoline
/// wired first, so this is also a named, loud panic rather than a silent
/// no-op. The `Executor::start_thread` API itself IS fully implemented and
/// tested (see `fn64-runtime/tests/rung_regressions.rs`'s `rung_14_*`
/// tests, which drive it directly with synthetic Rust closures standing in
/// for a recompiled entry point) -- what's missing here is only the glue
/// that turns a guest `vram` entry address into such a closure.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osStartThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let _t_addr = ctx.r4;
    unimplemented!(
        "osStartThread_recomp: cannot start a GameThread whose osCreateThread_recomp call \
         already had to panic for the same real-entry-point-dispatch gap (see that shim's doc \
         comment) -- staying loud here rather than silently succeeding on a thread that was \
         never really created."
    );
}

/// `osSetThreadPri_recomp` (ABI-SURFACE.md section (a), currently NWXE-
/// only x1, "the other game's port stage simply hasn't renamed a call site
/// yet" per that section -- implemented from the union per
/// `docs/DESIGN.md` section 5's wave-3 guidance). MIPS signature
/// `osSetThreadPri(OSThread *t, OSPri pri)` -- this crate identifies the
/// target thread by `OSId` rather than `t`'s rdram address (see
/// `osCreateThread_recomp`'s doc comment for why); `a1`=pri (`ctx->r5`).
/// Fully wired to `Executor::set_thread_pri` -- no dispatch gap here, since
/// changing an existing thread's priority needs no entry-point lookup.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSetThreadPri_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let target: ThreadId = ctx.r4 as u32;
    let pri = ctx.r5 as Priority;
    with_executor(|exec| exec.set_thread_pri(target, pri));
}

/// `osSetEventMesg_recomp` (ABI-SURFACE.md section (a): NWXE x2, NW4E x2
/// call sites). MIPS signature `osSetEventMesg(OSEvent event, OSMesgQueue
/// *mq, OSMesg msg)`.
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

/// Host-side entry point for injecting an external (SI/PI/VI-style)
/// completion into the executor -- NOT a `_recomp` shim (nothing in
/// recompiled guest code calls this; it's `fn64-shell`'s hook, per
/// `docs/DESIGN.md` section 2's "ONE explicit host-side injection point").
/// Exposed here rather than requiring `fn64-shell` to depend on
/// `fn64-runtime` directly for this one call, keeping `fn64-abi` as the
/// single seam between "the executor" and "everything that isn't
/// recompiled guest code," per this crate's own module doc.
pub fn inject_external_event(event: ExternalEvent) {
    with_executor(|exec| exec.inject_event(event));
}

/// Host-side virtual-clock driver -- see `inject_external_event`'s doc
/// comment; same non-`_recomp` host-facing category.
pub fn advance_virtual_time(now: u64) {
    with_executor(|exec| exec.advance_time(now));
}

#[cfg(test)]
mod tests {
    use super::*;
    use corosensei::{Coroutine, CoroutineResult};
    use fn64_runtime::{RecvMesgOutcome, SendMesgOutcome};

    fn ctx_with(r4: u64, r5: u64, r6: u64) -> RecompContext {
        RecompContext {
            r0: 0,
            r1: 0,
            r2: 0,
            r3: 0,
            r4,
            r5,
            r6,
            r7: 0,
            r29: 0,
        }
    }

    /// Test-only stand-in for the not-yet-implemented `osCreateThread_
    /// recomp`/`osStartThread_recomp` entry-point dispatch (see those
    /// shims' doc comments): registers a `GameThread` directly with the
    /// executor and drives its coroutine body through `with_active_yielder`
    /// exactly the way a real trampoline would, so `pause_self`/
    /// `osSendMesg_recomp`/`osRecvMesg_recomp`'s actual suspend-the-active-
    /// coroutine logic gets exercised end-to-end through the real
    /// `extern "C"` symbols, not a shortcut that bypasses them.
    fn spawn_test_thread(id: ThreadId, pri: Priority, body: impl FnOnce() + 'static) {
        with_executor(|exec| {
            exec.create_thread(id, pri, move |yielder, first_input| {
                with_active_yielder(id, yielder, || {
                    // Stash first_input's delivered-message case for a
                    // body that immediately expects one; most test bodies
                    // below don't need this (they call the shims, which
                    // themselves call suspend_active_coroutine and get the
                    // real Resume value back).
                    let _ = first_input;
                    body();
                });
            });
            exec.start_thread(id);
        });
    }

    /// Drains the executor until idle, installing `with_active_yielder`
    /// around each `resume()` the same way `Executor::run_one_step`'s real
    /// trampoline integration will once wave 3 finishes wiring
    /// `osCreateThread_recomp`. This exists ONLY in this test module
    /// because `fn64-abi`'s `EXECUTOR`/`ACTIVE_YIELDER` thread-locals are
    /// private to this crate; `fn64-runtime`'s own executor doesn't know
    /// about `Yielder` installation at all (that's this crate's seam, per
    /// module doc).
    fn run_to_idle_with_yielder_plumbing() {
        // fn64_runtime::Executor::run_one_step already calls
        // GameThread::resume, which calls corosensei::Coroutine::resume,
        // which runs the closure passed to GameThread::new/Executor::
        // create_thread synchronously on the SAME call stack -- so by the
        // time `with_active_yielder` runs (inside that closure), it's
        // already nested inside the resume() call. This means
        // with_active_yielder installs itself correctly without this
        // helper needing to do anything beyond just calling run_one_step;
        // the closure passed to create_thread (see spawn_test_thread)
        // performs the installation itself, at the right moment, every
        // time it's resumed (corosensei re-enters the SAME closure
        // invocation across yields via its own stack, so
        // with_active_yielder's install happens once per thread's whole
        // lifetime and its Yielder reference stays valid for every
        // subsequent resume, since it's the same stack frame each time).
        with_executor(|exec| exec.run_to_idle());
    }

    #[test]
    fn pause_self_yields_via_real_executor_and_thread_keeps_running() {
        let ran_twice = std::rc::Rc::new(std::cell::RefCell::new(0));
        let ran_twice2 = ran_twice.clone();
        spawn_test_thread(100, 5, move || {
            pause_self(std::ptr::null_mut(), std::ptr::null_mut());
            *ran_twice2.borrow_mut() += 1;
        });
        with_executor(|exec| {
            assert!(exec.run_one_step()); // enters body, hits pause_self, yields
        });
        assert_eq!(
            *ran_twice.borrow(),
            0,
            "must not have run past pause_self yet"
        );
        with_executor(|exec| {
            assert!(exec.run_one_step()); // resumed, runs to completion
        });
        assert_eq!(*ran_twice.borrow(), 1);
    }

    #[test]
    fn create_then_nonblocking_send_succeeds() {
        let mq_vram: u64 = 0xFFFF_FFFF_8005_7228; // sign-extended KSEG0 form
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

        // Fill the one-slot queue directly via the executor (standing in
        // for an earlier real osSendMesg call).
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

        with_executor(|exec| {
            exec.run_one_step();
        });
        assert!(
            !*delivered_second.borrow(),
            "must still be blocked, queue was full"
        );

        // Drain the queue for real (a receiver consuming message 1) --
        // must wake the blocked sender via the SAME executor path
        // fn64-runtime's rung_18 tests already exercise directly.
        with_executor(|exec| {
            let outcome = exec.recv_mesg(999, mq_addr, false);
            assert_eq!(outcome, RecvMesgOutcome::Delivered(1));
        });
        run_to_idle_with_yielder_plumbing();
        assert!(
            *delivered_second.borrow(),
            "blocked sender woken once space freed"
        );
    }

    #[test]
    fn blocking_recv_on_empty_queue_yields_and_receives_real_message() {
        let mq_vram: u64 = 0xFFFF_FFFF_8007_0000;
        let mq_addr = RdramAddr::from_gpr(mq_vram);
        let mut create_ctx = ctx_with(mq_vram, 0, 1);
        unsafe { osCreateMesgQueue_recomp(std::ptr::null_mut(), &mut create_ctx as *mut _) };

        // Backing "rdram" for the msg-out write.
        let mut rdram = vec![0u8; 64];
        let rdram_ptr = rdram.as_mut_ptr();
        let msg_out_vram: u64 = 0xFFFF_FFFF_8000_0020; // offset 0x20 into our fake rdram

        spawn_test_thread(103, 1, move || {
            let mut recv_ctx = ctx_with(mq_vram, msg_out_vram, OS_MESG_BLOCK);
            unsafe { osRecvMesg_recomp(rdram_ptr, &mut recv_ctx as *mut _) };
        });
        with_executor(|exec| {
            exec.run_one_step();
        }); // enters, blocks on recv

        with_executor(|exec| {
            let outcome = exec.send_mesg(0, mq_addr, 0x1234_5678, false);
            assert_eq!(outcome, SendMesgOutcome::Delivered);
        });
        run_to_idle_with_yielder_plumbing();

        let written = i32::from_be_bytes(rdram[0x20..0x24].try_into().unwrap());
        assert_eq!(written, 0x1234_5678);
    }

    #[test]
    fn set_thread_pri_takes_effect_on_run_queue_order() {
        spawn_test_thread(200, 1, || {});
        let mut ctx = ctx_with(200, 50, 0);
        unsafe { osSetThreadPri_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        with_executor(|exec| {
            assert_eq!(exec.thread_pri(200), 50);
        });
    }

    // `osCreateThread_recomp`/`osStartThread_recomp`/`pause_self`-with-no-
    // active-yielder are all plain `extern "C" fn`s (the real ABI shape
    // generated C calls, matching every other symbol here) -- a Rust panic
    // cannot unwind across that boundary and aborts the process instead
    // (Rust's own defined behavior for an unwind reaching a non-"C-unwind"
    // extern boundary). That abort IS the loud trap `AGENTS.md` requires,
    // so each is verified as a subprocess exit rather than `catch_unwind`/
    // `#[should_panic]`, which require an in-process catchable unwind and
    // would otherwise abort the whole test harness -- the same pattern (and
    // the same reasoning) as the original `pause_self` smoke test this
    // crate shipped with wave 1.
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
    fn os_create_thread_stays_a_loud_named_panic_not_a_silent_noop() {
        assert_subprocess_aborts("tests::__os_create_thread_abort_subprocess_entry");
    }

    #[test]
    #[ignore] // only ever run directly, by the subprocess harness above
    fn __os_create_thread_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let mut ctx = ctx_with(0, 1, 0x8000_1234);
            unsafe { osCreateThread_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        }
    }

    #[test]
    fn os_start_thread_stays_a_loud_named_panic_not_a_silent_noop() {
        assert_subprocess_aborts("tests::__os_start_thread_abort_subprocess_entry");
    }

    #[test]
    #[ignore] // only ever run directly, by the subprocess harness above
    fn __os_start_thread_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let mut ctx = ctx_with(0, 0, 0);
            unsafe { osStartThread_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
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
            pause_self(std::ptr::null_mut(), std::ptr::null_mut());
        }
    }

    // Keep corosensei's CoroutineResult import exercised in a trivial way
    // so a future refactor that removes with_active_yielder's need for it
    // doesn't leave an unused-import warning silently masking a real
    // simplification opportunity.
    #[test]
    fn coroutine_result_yield_and_return_are_distinguishable() {
        let mut co: Coroutine<(), i32, i32> = Coroutine::new(|yielder, _| {
            yielder.suspend(1);
            2
        });
        assert_eq!(co.resume(()), CoroutineResult::Yield(1));
        assert_eq!(co.resume(()), CoroutineResult::Return(2));
    }
}
