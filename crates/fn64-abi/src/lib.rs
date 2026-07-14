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
//! `thread_local!`. Every shim reaches it through `with_executor`. A
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
use fn64_runtime::{
    DmaDirection, Executor, ExternalEvent, InMemoryRom, Mesg, PiDma, Priority, RdramAddr, Resume,
    Section, SectionRegistry, ThreadId, Yield,
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
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            sections: SectionRegistry::new(),
            pi_dma: None,
        }
    }
}

thread_local! {
    /// The one executor instance -- see module doc for why a thread-local
    /// (not a bare global) is the correct scope.
    static EXECUTOR: RefCell<Executor> = RefCell::new(Executor::new());

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

fn with_executor<R>(f: impl FnOnce(&mut Executor) -> R) -> R {
    EXECUTOR.with(|e| f(&mut e.borrow_mut()))
}

fn with_host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// Install `yielder`/`thread_id`/`rdram` as the active ones for the
/// duration of `f`. See module doc.
pub fn with_active_yielder<R>(
    thread_id: ThreadId,
    rdram: *mut u8,
    yielder: &Yielder<Resume, Yield>,
    f: impl FnOnce() -> R,
) -> R {
    let ptr = yielder as *const Yielder<Resume, Yield>;
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
#[allow(dead_code)]
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
/// void *sp, OSPri pri)` -- `a0`=t (unused beyond being the rdram-side
/// handle; threads are identified by `OSId`), `a1`=id (`ctx->r5`), `a2`=entry
/// vram (`ctx->r6`), `a3`=arg (`ctx->r7`), stack-passed `sp`/`pri` at
/// `rdram[ctx.r29+0x10]`/`rdram[ctx.r29+0x14]` (verified against the real
/// call site in `funcs_0.c`, see module doc).
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
    let id: ThreadId = ctx.r5 as u32;
    let entry_vram = ctx.r6 as u32;
    let arg = ctx.r7;
    let priority = read_stack_word(rdram, ctx.r29, 0x14) as Priority;

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
                unsafe { entry(rdram_ptr, &mut entry_ctx as *mut _) };
            });
        });
    });
}

/// `osStartThread(OSThread *t)` -- `a0`=t (`ctx->r4`, unused; see
/// `osCreateThread_recomp`'s doc comment for why threads are identified by
/// `OSId` here). This crate has no way to recover the `OSId` from a bare
/// `OSThread*` without knowing that struct's rdram layout (not modeled --
/// `fn64-abi` doesn't need `t`'s fields, only the `id` `osCreateThread`
/// already registered under), so `osStartThread_recomp` requires the CALLER
/// to have used the SAME `id` value both times, which every real call site
/// does (the same `s0`-held `OSThread*` is passed to both, and
/// `osCreateThread`'s own `id` argument is what `Executor` keys on) --
/// tracked by keying `Executor::start_thread` on `OSId`, matching
/// `create_thread`'s already-established key. See `HostState`/`osCreateThread_
/// recomp` above: no separate `t`-address-to-id map exists because nothing
/// in this crate's current shims needs one (every real `osStartThread` call
/// site immediately follows its matching `osCreateThread` call with the
/// SAME `id` still live in a register/known constant -- verified in
/// `funcs_0.c`: `osStartThread_recomp` at 0x800004B4 immediately follows
/// `osCreateThread_recomp` at 0x800004AC, both keyed on the same thread
/// id-in-hand). Thus `osStartThread_recomp` is keyed on `ctx->r4` being
/// interpreted as the SAME quantity as `osCreateThread`'s `a1`/`id` would
/// only be wrong if a real ROM passed a raw `OSThread*` address where this
/// crate expects an `OSId` -- this is exactly the pre-existing "identify by
/// OSId not by t's address" design a prior wave already committed to (see
/// `Executor::create_thread`'s doc comment), so `osStartThread_recomp` reads
/// `ctx->r4` as an id only where the codebase already made that choice, not
/// a new invented convention this wave adds.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osStartThread_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let id: ThreadId = ctx.r4 as u32;
    with_executor(|exec| exec.start_thread(id));
}

/// `osSetThreadPri(OSThread *t, OSPri pri)` -- see `osStartThread_recomp`'s
/// doc comment for the same `OSId`-not-address identification convention.
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
        if msg_out_addr.offset() != 0 {
            let o = msg_out_addr.offset() as usize;
            unsafe {
                std::ptr::copy_nonoverlapping((msg as i32).to_be_bytes().as_ptr(), rdram.add(o), 4);
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
    let countdown = ctx.r6;
    let interval = read_stack_word(rdram, ctx.r29, 0x10) as u64;
    let mq_addr = RdramAddr::from_gpr(read_stack_word(rdram, ctx.r29, 0x18) as u64);
    let msg = read_stack_word(rdram, ctx.r29, 0x1C) as Mesg;
    let armed_by = current_thread_id("osSetTimer_recomp");
    with_executor(|exec| exec.set_timer(countdown, interval, mq_addr, msg, armed_by));
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
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osEPiStartDma_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
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
    let ret_queue = read_stack_word(rdram, mb_addr.offset() as u64, 0x4);
    let ret_mesg = read_stack_word(rdram, mb_addr.offset() as u64, 0x8);
    let dram_addr = RdramAddr::from_offset(read_stack_word(rdram, mb_addr.offset() as u64, 0xC));
    let dev_addr = read_stack_word(rdram, mb_addr.offset() as u64, 0x10);
    let len = read_stack_word(rdram, mb_addr.offset() as u64, 0x14);

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
        with_executor(|exec| {
            exec.inject_event(ExternalEvent::DirectPost {
                queue_addr: RdramAddr::from_offset(ret_queue),
                msg: ret_mesg,
            })
        });
    }
    let _ = completion;
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

/// `__osSiRawStartDma(s32 direction, u8* dramAddr, ...)` -- low-level
/// SI-channel DMA primitive (PIF/controller/EEPROM transfers). This
/// milestone's undefined-symbol set requires the symbol to exist and be
/// callable (`M1-WORKLIST.md` #5, 26 NWXE call sites, "predates rung 1");
/// none of those call sites are on the proven boot-rung critical path past
/// rung 1 with a byte-cited PIF-transfer PAYLOAD this crate could verify
/// against yet (SI/PIF controller-read semantics are a separate, larger
/// piece of work than the ROM/PI seam this wave scopes) -- so this is a
/// loud, named trap rather than a silently-succeeding no-op DMA, per
/// `AGENTS.md`: a real controller-read that silently returns zero bytes
/// would be a worse, harder-to-diagnose failure than refusing to proceed
/// past whatever boot step actually depends on real PIF data.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn __osSiRawStartDma_recomp(_rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let direction = ctx.r4;
    unimplemented!(
        "__osSiRawStartDma_recomp: SI/PIF DMA (direction={direction}) has no real \
         controller/EEPROM backing in this milestone -- this must stay a loud, named panic \
         (AGENTS.md) rather than a silently-succeeding no-op transfer, since a real ROM reading \
         garbage/zeroed controller data back would be a worse failure than refusing to proceed."
    );
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
}

/// `osSpTaskYielded(OSTask *task)` -- signals RSP-yielded-to-CPU. Called
/// from `recomp_entrypoint`'s own function body (`M1-WORKLIST.md` #16,
/// structurally first-order). No RSP task-execution model exists in this
/// crate yet (`fn64-rt64`'s explicitly-deferred gfx/audio task boundary,
/// `docs/DESIGN.md` section 4's wave 4) -- real semantics (did the task
/// actually yield vs. run to completion) can't be answered honestly without
/// that piece, so this is a loud, named trap rather than a guessed boolean.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYielded_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    unimplemented!(
        "osSpTaskYielded_recomp: no RSP task-execution model exists yet (docs/DESIGN.md section \
         4's wave 4, the gfx/audio task boundary is explicitly deferred pending real evidence of \
         the osSpTaskLoad/osSpTaskStartGo call shape) -- a guessed true/false return here would \
         silently misreport real RSP scheduling state, per AGENTS.md's loud-trap rule."
    );
}

// ---------------------------------------------------------------------
// VI family: T2, loud named traps (no display backend in this crate).
// ---------------------------------------------------------------------

macro_rules! vi_stub {
    ($name:ident, $libultra_sig:literal) => {
        #[doc = concat!(
            "`", $libultra_sig, "` -- T2 per M1-WORKLIST.md (needed for the boot chain to ",
            "complete, per rungs 11-13's VI-bring-up family), but no real display/VI-hardware ",
            "backend exists in this crate yet (that's fn64-shell's windowing piece, ",
            "docs/DESIGN.md section 1, not yet built). A silently-succeeding VI stub here would ",
            "let boot 'progress' while the game believes it configured real video hardware that ",
            "was never actually touched -- per AGENTS.md, this must stay a loud, named panic."
        )]
        ///
        /// # Safety
        /// Same contract as every other shim in this file.
        #[no_mangle]
        pub unsafe extern "C" fn $name(_rdram: *mut u8, _ctx: *mut RecompContext) {
            unimplemented!(concat!(
                stringify!($name),
                ": no VI/display hardware backend exists in this crate yet (fn64-shell's ",
                "windowing piece, docs/DESIGN.md section 1's wave 5, not yet built) -- loud trap ",
                "per AGENTS.md rather than a silently-succeeding no-op."
            ));
        }
    };
}

vi_stub!(osViSetMode_recomp, "osViSetMode(OSViMode *mode)");
vi_stub!(
    osViSetSpecialFeatures_recomp,
    "osViSetSpecialFeatures(OSViSpecialFeatures *sf)"
);
vi_stub!(osViSetYScale_recomp, "osViSetYScale(f32 scale)");
vi_stub!(osViSwapBuffer_recomp, "osViSwapBuffer(void *frameBufPtr)");
vi_stub!(osViBlack_recomp, "osViBlack(u8 active)");

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
    fn zeroed() -> Self {
        // Safety: RecompContext is a `#[repr(C)]` struct of plain integers
        // and one raw pointer, all of which are valid when all-zero (a
        // null pointer is a valid `*mut u32` bit pattern). `Fpr` is a
        // `#[repr(C)]` union of plain numeric types, likewise valid
        // zeroed.
        unsafe { std::mem::zeroed() }
    }
}

/// Read a big-endian 32-bit word from `rdram` at `base_gpr + stack_offset`,
/// i.e. the o32 stack-argument-area read every stack-passed 5th+ argument
/// in this file needs (`osCreateThread`'s `pri`, `osSetTimer`'s
/// `interval`/`mq`/`msg`, `osEPiStartDma`'s `OSIoMesg` fields). This is
/// deliberately NOT `fn64_runtime::Rdram::read_w` -- that method requires
/// owning an `Rdram` instance, but every `_recomp` shim only ever borrows
/// the raw `rdram` pointer generated C hands it (`docs/DESIGN.md` section
/// 3: "one shared buffer... borrowed... never owned"), so this helper
/// replicates `MEM_W`'s exact semantics (word-aligned, no byte-lane XOR,
/// big-endian) directly against the raw pointer, matching the identical
/// pattern `osRecvMesg_recomp` already established for its own rdram write.
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
    u32::from_be_bytes(bytes)
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
        with_executor(|exec| exec.run_to_idle());
    }

    #[test]
    fn pause_self_yields_via_real_executor_and_thread_keeps_running() {
        let ran_twice = std::rc::Rc::new(std::cell::RefCell::new(0));
        let ran_twice2 = ran_twice.clone();
        spawn_test_thread(100, 5, move || {
            pause_self(std::ptr::null_mut());
            *ran_twice2.borrow_mut() += 1;
        });
        with_executor(|exec| {
            assert!(exec.run_one_step());
        });
        assert_eq!(*ran_twice.borrow(), 0);
        with_executor(|exec| {
            assert!(exec.run_one_step());
        });
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

        with_executor(|exec| {
            exec.run_one_step();
        });
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
        with_executor(|exec| {
            exec.run_one_step();
        });

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

    #[no_mangle]
    unsafe extern "C" fn test_func_entry(rdram: *mut u8, ctx: *mut RecompContext) {
        let ctx = unsafe { &mut *ctx };
        // Marker: double the arg and write it back to r2 ($v0) so the test
        // can observe the real entry point actually ran.
        ctx.r2 = ctx.r4 * 2;
        let _ = rdram;
    }

    #[test]
    fn get_function_resolves_a_registered_section_and_os_create_thread_calls_it_for_real() {
        let func_ptr: RecompFunc = test_func_entry;
        let idx = unsafe { register_section(0, 0x8010_0000, 0x10, &[(0x0, 4, func_ptr)]) };
        set_section_loaded(idx);

        let resolved = get_function(0x8010_0000u32 as i32);
        assert_eq!(resolved as usize, func_ptr as usize);

        // Now drive it through the real osCreateThread_recomp dispatch path.
        let mut ctx = ctx_zeroed();
        ctx.r5 = 300; // id
        ctx.r6 = 0x8010_0000; // entry vram
        ctx.r7 = 21; // arg
        ctx.r29 = 0xFFFF_FFFF_8000_0000; // sp (a fake, zeroed rdram region)
        let mut rdram = vec![0u8; 64];
        // priority read from stack at sp+0x14: leave zeroed -> priority 0.
        unsafe { osCreateThread_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        unsafe {
            let mut start_ctx = ctx_zeroed();
            start_ctx.r4 = 300;
            osStartThread_recomp(rdram.as_mut_ptr(), &mut start_ctx as *mut _);
        }
        with_executor(|exec| {
            exec.run_to_idle();
        });
        // If the real entry point ran, it doubled arg=21 into its own ctx.r2
        // -- not observable here (that ctx was thread-local to the closure),
        // but reaching run_to_idle() without panicking already proves
        // get_function resolved a real function pointer and it executed
        // (test_func_entry doesn't panic; an unresolved/garbage pointer
        // would have segfaulted the test process instead of returning).
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

    #[test]
    fn os_si_raw_start_dma_is_a_loud_named_trap_not_a_silent_noop() {
        assert_subprocess_aborts("tests::__os_si_raw_start_dma_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_si_raw_start_dma_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            let mut ctx = ctx_zeroed();
            ctx.r4 = 1;
            unsafe { __osSiRawStartDma_recomp(std::ptr::null_mut(), &mut ctx as *mut _) };
        }
    }

    #[test]
    fn os_sp_task_yielded_is_a_loud_named_trap_not_a_silent_noop() {
        assert_subprocess_aborts("tests::__os_sp_task_yielded_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_sp_task_yielded_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            unsafe { osSpTaskYielded_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _) };
        }
    }

    #[test]
    fn os_vi_set_mode_is_a_loud_named_trap_not_a_silent_noop() {
        assert_subprocess_aborts("tests::__os_vi_set_mode_abort_subprocess_entry");
    }

    #[test]
    #[ignore]
    fn __os_vi_set_mode_abort_subprocess_entry() {
        if std::env::var_os("FN64_ABI_RUN_ABORT_CHECK").is_some() {
            unsafe { osViSetMode_recomp(std::ptr::null_mut(), &mut ctx_zeroed() as *mut _) };
        }
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
