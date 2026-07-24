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
//! - The formerly trapped VI family now updates typed `ViState`, drives the
//!   configured render backend at swap time, and shares executor retrace
//!   delivery with `osSetEventMesg`. `osSetTimer_recomp` is likewise wired
//!   to the executor's `TimerWheel`.

use std::cell::{Cell, RefCell};

use corosensei::Yielder;
use fn64_audio::AudioBackend;
use fn64_render::RenderBackend;
pub use fn64_render::{ActiveRenderGraphicsApi, RenderBackendEvidence, UcodeId};
use fn64_runtime::{
    Cycles, DeviceFabric, DeviceFault, DeviceNotification, DmaDirection, Executor, ExternalEvent,
    FixedPiTiming, InMemoryRom, Mesg, MmioAddr, OsTaskHeader, PiDma, PiDmaError, PiDmaRequest,
    Priority, RdramAddr, Resume, Section, SectionRegistry, ThreadId, Yield, M_AUDTASK, M_GFXTASK,
};

#[cfg(feature = "recomp-rs")]
pub mod recompiled;

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

/// Stable generated-C function identity recorded at the first instruction of
/// an entered native recompiled body.
///
/// The section index and offset come from N64Recomp's generated section table;
/// native pointer bits are used only to find this metadata and never escape
/// into evidence. `link_vram` is retained explicitly so consumers do not have
/// to recover section geometry in order to report a reached destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeExecutionDestination {
    pub section_index: u32,
    pub function_offset: u32,
    pub link_vram: u32,
}

/// One successfully entered generated-C function, in exact entry order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeExecutionDestinationEvent {
    pub at: Cycles,
    pub destination: NativeExecutionDestination,
}

/// One committed RSP/RDP observation retained in exact ABI execution order.
///
/// This history is distinct from future-affecting device state. Microcode
/// identity comes from the registered renderer's exact-digest catalog over a
/// complete live IMEM image, while the ABI independently retains that image's
/// digest. DPC observations appear only after the renderer accepts the exact
/// command image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspRdpObservationEvent {
    pub at: Cycles,
    pub kind: RspRdpObservationKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RspRdpObservationKind {
    MicrocodeRecognition {
        task_addr: RdramAddr,
        imem_generation: u64,
        text_sha256: [u8; 32],
        /// Physical start of the original task microcode-data image. A
        /// yielded resume retains the identity captured for its initial task;
        /// the rewritten yield-buffer pointer is never represented as source
        /// microcode data.
        data_addr: RdramAddr,
        data_size: u32,
        data_sha256: [u8; 32],
        family: Option<UcodeId>,
    },
    DramDpcCommitted {
        start: u32,
        end: u32,
        command_sha256: [u8; 32],
    },
    XbusDpcCommitted {
        start: u32,
        end: u32,
        command_sha256: [u8; 32],
    },
    ImemReplacementCommitted {
        task_addr: RdramAddr,
        imem_generation: u64,
        text_sha256: [u8; 32],
    },
}

/// Guest cycles charged per generated-C raw RCP register access. Uncached
/// MMIO stalls the real VR4300 for tens of cycles, and charging that time is
/// also what keeps a raw `while (AI_STATUS & FULL)`-style poll loop live:
/// device deadlines (AI drain, PI completion, VI retrace) only fire as
/// virtual time advances, and the C lane has no instruction checkpoints of
/// its own, so an uncharged poll would spin forever inside one scheduling
/// slice (observed: WM2000's audio manager polling `0xA450000C`).
/// ponytail: one flat cost, calibrate per-register timing if a title's
/// faithful-rate work (docs/ROADMAP.md R5) ever needs it.
const C_LANE_RAW_MMIO_ACCESS_CYCLES: u32 = 32;

/// Charge the raw-access stall iff a guest coroutine is executing. Host-side
/// callers (the c_smoke proxy binary, diagnostics) have no coroutine to
/// suspend and no virtual clock to keep honest.
fn charge_c_lane_mmio_access() {
    if ACTIVE_YIELDER.with(|cell| cell.get()).is_some() {
        suspend_active_coroutine(Yield::InstructionCheckpoint {
            instructions: C_LANE_RAW_MMIO_ACCESS_CYCLES,
        });
    }
}

/// Charge the stall a guest device-busy retry costs and give the executor a
/// checkpoint, iff a guest coroutine is executing. This keeps a
/// `while (osEPiStartDma(..) != 0);` retry loop live when the PI command queue
/// is genuinely full: completions only commit as virtual time advances, while
/// a shim-level retry has no instruction checkpoint of its own.
pub(crate) fn charge_guest_device_busy_retry() {
    charge_c_lane_mmio_access();
}

/// How many generated-C loop back-edges one thread may take within a single
/// `run_one_step` resume before fn64 forces an instruction checkpoint. Real
/// VR4300 hardware preempts any spin with the ~60 Hz VI interrupt; fn64's
/// cooperative executor has no such interrupt, so a tight guest loop that
/// polls ordinary RDRAM (not MMIO, not a message queue -- e.g. SM64's
/// `wait_for_audio_frames`: `gAudioFrameCount = 0; while (gAudioFrameCount <
/// n) {}`, waiting on the sound thread the VI retrace is supposed to wake)
/// never yields and `resume()` never returns. Back-edge instrumentation
/// (fn64_mmio_proxy.h's `FN64_BACKEDGE`, injected before every backward
/// `goto` by `build_support.rs`) makes each loop iteration count one edge and
/// suspend on the Nth. The threshold is a stall budget, not an exact cycle
/// count: it must be large enough that ordinary bounded loops (memcpy, table
/// walks) never pay a checkpoint, yet small enough that a genuine spin yields
/// well within one host time-slice. 4096 edges is ~one N64 scanline of the
/// tightest 2-instruction spin.
const C_LANE_BACKEDGE_CHECKPOINT_THRESHOLD: u32 = 4096;

thread_local! {
    /// Per-resume back-edge counter. Reset to zero every time the executor
    /// hands control to a coroutine (see `reset_backedge_budget`), so the
    /// threshold bounds edges *within one resume*, never across a thread's
    /// whole lifetime -- a long-running game that legitimately loops billions
    /// of times over its lifetime still only pays a checkpoint once per
    /// C_LANE_BACKEDGE_CHECKPOINT_THRESHOLD edges of any single uninterrupted
    /// spin.
    static BACKEDGE_BUDGET: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Reset the per-resume back-edge budget. Called by the coroutine-context
/// plumbing immediately before each resume so the threshold is measured per
/// scheduling slice.
pub(crate) fn reset_backedge_budget() {
    BACKEDGE_BUDGET.with(|cell| cell.set(0));
}

/// Generated-C loop back-edge observer. Injected before every backward `goto`
/// in the recompiled bodies (see `fn64_mmio_proxy.h`'s `FN64_BACKEDGE` macro
/// and `build_support.rs`'s injection pass). The common path is a single
/// increment-and-compare; only when a thread has taken
/// `C_LANE_BACKEDGE_CHECKPOINT_THRESHOLD` back-edges since its last resume
/// without otherwise yielding does it force one instruction checkpoint,
/// letting the executor advance virtual time (firing pending VI retraces / AI
/// drains) and run other threads before this one resumes and re-checks its
/// poll condition. No coroutine active (host-side diagnostics) => no-op.
#[no_mangle]
pub extern "C" fn fn64_c_backedge() {
    let over_budget = BACKEDGE_BUDGET.with(|cell| {
        let next = cell.get().wrapping_add(1);
        if next >= C_LANE_BACKEDGE_CHECKPOINT_THRESHOLD {
            cell.set(0);
            true
        } else {
            cell.set(next);
            false
        }
    });
    if over_budget && ACTIVE_YIELDER.with(|cell| cell.get()).is_some() {
        // Charge the back-edges' worth of guest instructions and yield. The
        // thread genuinely executed ~threshold loop iterations since its last
        // checkpoint, so advancing virtual time by that many cycles is the
        // honest accounting -- and, crucially, it lets the executor reach a
        // pending device deadline (an audio-frame completion, a VI retrace) in
        // O(deadline / threshold) resume round-trips instead of one per cycle.
        // A too-small charge would keep a multi-million-cycle wait (SM64's
        // `audio_reset_session` note-silence loop waits up to 4 s of audio
        // frames) crawling forward one checkpoint at a time.
        suspend_active_coroutine(Yield::InstructionCheckpoint {
            instructions: C_LANE_BACKEDGE_CHECKPOINT_THRESHOLD,
        });
    }
}

/// Boot diagnostic: log when a thread's `$sp` jumps 16KB regions between
/// message-queue calls -- catches stack switches/corruption cheaply.
pub(crate) fn probe_sp_region(site: &str, ctx: &RecompContext) {
    if !boot_probe_enabled() {
        return;
    }
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static LAST: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
    }
    let thread = ACTIVE_THREAD_ID.with(|cell| cell.get()).unwrap_or(u32::MAX);
    let sp = ctx.r29 as u32;
    LAST.with(|map| {
        let mut map = map.borrow_mut();
        if let Some(prev) = map.insert(thread, sp >> 14) {
            if prev != sp >> 14 {
                eprintln!(
                    "[boot-probe] thread {thread:#x} sp region change at {site}: {:#010x}-region -> sp={sp:#010x}",
                    prev << 14
                );
            }
        }
    });
}

/// Env-gated boot diagnostics (`FN64_BOOT_PROBE=1`): one-line traces of the
/// OS-level events that boot state machines hinge on (event registration,
/// raw SI kicks). Cheap enough to keep compiled in; silent unless armed.
pub(crate) fn boot_probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FN64_BOOT_PROBE").is_some())
}

/// Generated-C word-MMIO proxy entry points. The proxy header calls these
/// only for KSEG1 RCP addresses; ordinary RDRAM accesses stay inline.
#[no_mangle]
pub extern "C" fn fn64_c_mmio_read_w(vaddr: u64) -> i32 {
    charge_c_lane_mmio_access();
    pi::read_raw_mmio_word(vaddr).unwrap_or_else(|| {
        panic!("generated-C raw MMIO read is outside the modeled RCP window: {vaddr:#018X}")
    }) as i32
}

#[no_mangle]
pub extern "C" fn fn64_c_mmio_write_w(vaddr: u64, value: u32) {
    charge_c_lane_mmio_access();
    assert!(
        pi::write_raw_mmio_word(vaddr, value),
        "generated-C raw MMIO write is outside the modeled RCP window: {vaddr:#018X}"
    );
}

#[no_mangle]
pub extern "C" fn fn64_c_mmio_bad_width(vaddr: u64, width: u32, is_write: u32) {
    let operation = if is_write == 0 { "read" } else { "write" };
    let context = format!(
        "generated-C raw MMIO {operation} at {vaddr:#018X} used unsupported {width}-byte access; RCP registers require modeled word semantics"
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.generated-c-mmio.bad-width",
        &context,
        Some(fn64_runtime::Cycles::new(sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}");
}

/// Trap generated-C naturally aligned loads/stores before the host pointer
/// cast can turn them into a byte-lane chimera. MIPS lw/sw/lh/sh raise an
/// address-error exception for this shape.
#[no_mangle]
pub extern "C" fn fn64_c_mem_unaligned(vaddr: u64, width: u32, is_write: u32) {
    let operation = if is_write == 0 { "load" } else { "store" };
    let context = format!(
        "generated-C {width}-byte {operation} at unaligned guest address {vaddr:#018X}; real \
         hardware raises an address-error exception here (MIPS lw/sw/lh/sh alignment rule)"
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.generated-c-memory.unaligned",
        &context,
        Some(fn64_runtime::Cycles::new(sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}");
}

#[no_mangle]
pub extern "C" fn fn64_c_bad_direct_address(vaddr: u64, width: u32, is_write: u32) {
    let operation = if is_write == 0 { "read" } else { "write" };
    let context = format!(
        "generated-C direct-device {operation} at {vaddr:#018X} used unsupported mapped address with width {width}; only zero- or sign-extended KSEG0/KSEG1 are modeled"
    );
    fn64_runtime::record_unsupported_event(
        fn64_runtime::UnsupportedSubsystem::Abi,
        "abi.generated-c-direct-device.bad-address",
        &context,
        Some(fn64_runtime::Cycles::new(sim_time())),
        fn64_runtime::UnsupportedDisposition::LoudTrap,
    );
    panic!("{context}");
}

fn generated_c_rdram_physical_offset(vaddr: u64) -> Option<u32> {
    let upper = vaddr >> 32;
    let low = vaddr as u32;
    let physical_offset = low & 0x1fff_ffff;
    ((upper == 0 || upper == u32::MAX as u64)
        && (0x8000_0000..0xc000_0000).contains(&low)
        && physical_offset < fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32)
        .then_some(physical_offset)
}

#[no_mangle]
pub extern "C" fn fn64_c_rdram_write(vaddr: u64, width: u32, value: u64) {
    assert!(
        matches!(width, 1 | 2 | 4 | 8),
        "generated-C RDRAM write at {vaddr:#018x} reported invalid width {width}"
    );
    let physical_offset = generated_c_rdram_physical_offset(vaddr).unwrap_or_else(|| {
        panic!(
            "generated-C RDRAM write at {vaddr:#018x} is outside the modeled zero- or sign-extended KSEG0/KSEG1 physical RDRAM aliases"
        )
    });
    let end = physical_offset.checked_add(width).unwrap_or_else(|| {
        panic!("generated-C RDRAM write at {vaddr:#018x} width {width} overflows physical address")
    });
    assert!(
        physical_offset.is_multiple_of(width)
            && end <= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32,
        "generated-C RDRAM write at {vaddr:#018x} has invalid aligned physical range {physical_offset:#x}..{end:#x} for width {width}"
    );
    #[cfg(feature = "recomp-rs")]
    fn64_recomp_rs::notify_guest_write(physical_offset, width);
    if width == 2 {
        task_dispatch::observe_non_rdp_write16(physical_offset, value as u16);
    }
}

type LiveDeviceFabric = DeviceFabric<InMemoryRom, FixedPiTiming>;

#[derive(Clone, Copy)]
struct PendingPiCompletion {
    request: PiDmaRequest,
    rdram: *mut u8,
    rdram_len: usize,
    ret_queue: Option<RdramAddr>,
    ret_mesg: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PfsIsPlugTransaction {
    thread: ThreadId,
    queue: RdramAddr,
    message: Mesg,
    result_addr: RdramAddr,
    bitpattern: u8,
}

#[derive(Clone, Copy)]
enum PendingSiCompletionOwner {
    /// A raw PIF DMA owns the registered process allocation until byte commit.
    ProcessRdram { rdram: *mut u8, rdram_len: usize },
    /// Asynchronous Controller Manager calls notify the live OS_EVENT_SI target.
    OsEvent,
    /// Synchronous PFS owns both its exact completion route and future output.
    PfsIsPlug(PfsIsPlugTransaction),
}

#[derive(Clone, Copy)]
struct PendingSiCompletion {
    request: fn64_runtime::SiDmaRequest,
    owner: PendingSiCompletionOwner,
}

#[derive(Clone, Copy)]
struct PendingViMode {
    registers: [u32; 14],
    fields: [[u32; 5]; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPiCompletionEvidenceSnapshot {
    pub request: PiDmaRequest,
    pub rdram_len: u64,
    pub ret_queue: Option<RdramAddr>,
    pub ret_mesg: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PfsIsPlugTransactionEvidenceSnapshot {
    pub thread: ThreadId,
    pub queue: RdramAddr,
    pub message: Mesg,
    pub result_addr: RdramAddr,
    pub bitpattern: u8,
}

impl From<PfsIsPlugTransaction> for PfsIsPlugTransactionEvidenceSnapshot {
    fn from(value: PfsIsPlugTransaction) -> Self {
        Self {
            thread: value.thread,
            queue: value.queue,
            message: value.message,
            result_addr: value.result_addr,
            bitpattern: value.bitpattern,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingSiCompletionOwnerEvidenceSnapshot {
    ProcessRdram { rdram_len: u64 },
    OsEvent,
    PfsIsPlug(PfsIsPlugTransactionEvidenceSnapshot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSiCompletionEvidenceSnapshot {
    pub request: fn64_runtime::SiDmaRequest,
    pub owner: PendingSiCompletionOwnerEvidenceSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingViModeEvidenceSnapshot {
    pub registers: [u32; 14],
    pub fields: [[u32; 5]; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiViEvidenceSnapshot {
    pub pending_mode: Option<PendingViModeEvidenceSnapshot>,
    pub active_mode: Option<PendingViModeEvidenceSnapshot>,
    pub pending_control: Option<u32>,
    pub pending_x_scale_bits: Option<u32>,
    pub pending_y_scale_bits: Option<u32>,
    pub active_x_scale_bits: u32,
    pub active_y_scale_bits: u32,
}

/// Release-evidence view spanning both owners above the raw device fabric:
/// executor-owned peripherals and the ABI's manager-side completion/latch
/// metadata. Process pointer values are deliberately excluded; their stable
/// identity is enforced by the one registered-RDRAM invariant, while the
/// lengths and guest-visible request/delivery fields are future-affecting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePeripheralEvidenceSnapshot {
    pub peripherals: fn64_runtime::PeripheralsEvidenceSnapshot,
    pub pending_pi_completions: Vec<PendingPiCompletionEvidenceSnapshot>,
    pub pending_si_completion: Option<PendingSiCompletionEvidenceSnapshot>,
    /// Canonical ascending thread order. These transactions have posted their
    /// private completion but have not yet resumed to publish the output byte.
    pub completed_pfs_is_plug: Vec<PfsIsPlugTransactionEvidenceSnapshot>,
    pub vi: AbiViEvidenceSnapshot,
}

/// One retained CPU-side rspboot source image. RDRAM offsets are sorted in
/// [`AbiHostEvidenceSnapshot`] so hash-table insertion order cannot perturb a
/// release encoding; the bytes remain exact because the next task load can
/// observe every one of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspBootImageEvidenceSnapshot {
    pub rdram_offset: u32,
    pub bytes: Vec<u8>,
}

/// Exact logical identity of one task-named microcode-data image hashed at an
/// RSP kickoff boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspTaskDataIdentityEvidenceSnapshot {
    pub rdram_offset: u32,
    pub byte_len: u32,
    pub sha256: [u8; 32],
}

/// The one task header most recently copied into RSP DMEM by
/// `osSpTaskLoad`. `resumed_data_identity` is present only when the loaded
/// header is a validated yielded rewrite whose original data image was
/// retained through task lineage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedRspTaskEvidenceSnapshot {
    pub task_offset: u32,
    pub header: OsTaskHeader,
    pub resumed_data_identity: Option<RspTaskDataIdentityEvidenceSnapshot>,
}

/// Original task/data identity retained for a task that may be rewritten and
/// reloaded after a public RSP yield handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RspTaskLineagePhaseEvidenceSnapshot {
    Running,
    ResumeAuthorized,
    ResumeLoaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspTaskLineageEvidenceSnapshot {
    pub task_offset: u32,
    pub original_header: OsTaskHeader,
    pub data_identity: Option<RspTaskDataIdentityEvidenceSnapshot>,
    pub phase: RspTaskLineagePhaseEvidenceSnapshot,
}

/// Stable identity of the cartridge image installed behind the PI bus.
///
/// The digest is captured while the host still owns the installation bytes;
/// release evidence never serializes the ROM itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstalledRomEvidenceSnapshot {
    pub byte_len: u64,
    pub sha256: [u8; 32],
}

/// Pointer-free identity of the one process RDRAM registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredRdramEvidenceSnapshot {
    pub present: bool,
    pub byte_len: u64,
}

/// Cartridge-mounted save hardware. Controller Pak is intentionally absent:
/// it belongs to a controller port and cannot be installed or certified as a
/// cartridge save device through this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartridgeSaveType {
    Eeprom4k,
    Eeprom16k,
    SramBanked,
    FlashRam,
}

impl CartridgeSaveType {
    pub const fn byte_len(self) -> usize {
        match self {
            Self::Eeprom4k => fn64_runtime::SaveType::Eeprom4k.byte_len(),
            Self::Eeprom16k => fn64_runtime::SaveType::Eeprom16k.byte_len(),
            Self::SramBanked => fn64_runtime::SaveType::SramBanked.byte_len(),
            Self::FlashRam => fn64_runtime::SaveType::FlashRam.byte_len(),
        }
    }
}

/// Release-evidence state of cartridge save configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartridgeSaveEvidenceSnapshot {
    /// A compatibility registration path was used, or the host has not made
    /// an explicit no-save assertion. Live release capture rejects this.
    Unidentified,
    NoCartridgeSave,
    Configured(CartridgeSaveType),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadHandleEvidenceSnapshot {
    pub osthread_offset: u32,
    pub executor_thread_id: ThreadId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadGuestIdEvidenceSnapshot {
    pub executor_thread_id: ThreadId,
    pub guest_os_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerHandleEvidenceSnapshot {
    pub ostimer_offset: u32,
    pub timer_id: fn64_runtime::timer::TimerId,
}

/// Future-affecting state owned by libultra's controller manager rather than
/// by the physical PIF ports. The public Controller Manager manual makes the
/// default four channels, one-time initialization, and `osContSetCh` polling
/// limit observable through later query/read buffer extents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControllerManagerEvidenceSnapshot {
    pub initialized: bool,
    pub channels: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ControllerManagerState {
    initialized: bool,
    channels: u8,
}

impl Default for ControllerManagerState {
    fn default() -> Self {
        Self {
            initialized: false,
            channels: 4,
        }
    }
}

impl ControllerManagerState {
    fn initialize(&mut self) -> bool {
        if self.initialized {
            return false;
        }
        self.initialized = true;
        self.channels = 4;
        true
    }

    fn set_channels(&mut self, channels: u8) {
        assert!(
            channels <= 4,
            "osContSetCh: channel count {channels} exceeds MAXCONTROLLERS (4)"
        );
        self.channels = if self.initialized { channels } else { 4 };
    }

    fn channels(self) -> usize {
        usize::from(self.channels)
    }

    fn evidence_snapshot(self) -> ControllerManagerEvidenceSnapshot {
        ControllerManagerEvidenceSnapshot {
            initialized: self.initialized,
            channels: self.channels,
        }
    }
}

/// Complete owner-local, future-affecting view of ABI [`HostState`] above the
/// separately captured raw [`device_evidence_snapshot`] channel.
///
/// Hash-backed maps are canonicalized by their semantic keys. Native RDRAM
/// and generated-function pointers, append-only debug/operation/destination
/// logs, and derived section lookup caches are deliberately excluded: none
/// may change a later ABI result. Recompiled lane/program identity is a
/// separate feature-gated owner seam in `recompiled` and must be bound
/// alongside this snapshot by a release schema that admits that lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiHostEvidenceSnapshot {
    pub runtime_peripherals: RuntimePeripheralEvidenceSnapshot,
    pub controller_manager: ControllerManagerEvidenceSnapshot,
    pub flash: save::FlashEvidenceSnapshot,
    pub sections: fn64_runtime::SectionRegistryEvidenceSnapshot,
    pub rsp_boot_images: Vec<RspBootImageEvidenceSnapshot>,
    pub loaded_rsp_task: Option<LoadedRspTaskEvidenceSnapshot>,
    pub rsp_task_lineages: Vec<RspTaskLineageEvidenceSnapshot>,
    pub audio_task_execution: task_dispatch::AudioTaskExecutionPolicy,
    pub rom_installed: bool,
    pub installed_rom: Option<InstalledRomEvidenceSnapshot>,
    pub cartridge_save: CartridgeSaveEvidenceSnapshot,
    pub cart_rom_handle_vram: Option<u32>,
    pub flash_handle_vram: Option<u32>,
    pub leo_disk: Option<pi::LeoDiskConfig>,
    pub thread_handles: Vec<ThreadHandleEvidenceSnapshot>,
    pub thread_guest_ids: Vec<ThreadGuestIdEvidenceSnapshot>,
    pub timer_handles: Vec<TimerHandleEvidenceSnapshot>,
    pub next_synthetic_thread_id: ThreadId,
    pub registered_rdram: RegisteredRdramEvidenceSnapshot,
    pub debug_hardware: debug::DebugHardware,
}

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
    /// Always-present RCP/MI authority. Cartridge storage begins empty so
    /// interrupts and non-PI devices cannot depend on ROM load order;
    /// `rom_installed` preserves PI DMA's loud missing-ROM failure.
    device_fabric: LiveDeviceFabric,
    rom_installed: bool,
    /// Stable content identity captured at the ROM-install boundary. Keeping
    /// this beside `rom_installed` lets evidence distinguish an explicitly
    /// installed zero-byte synthetic cartridge from no cartridge at all.
    installed_rom: Option<InstalledRomEvidenceSnapshot>,
    /// Exact cartridge save identity supplied at the same boundary that
    /// installs its storage. This is separate from controller-port PFS state.
    cartridge_save: CartridgeSaveEvidenceSnapshot,
    /// OS-side requests accepted by the PI manager, in submission order.
    /// The front entry owns the hardware fabric's sole in-flight PI transfer;
    /// later entries are accepted manager work which has not reached the bus.
    /// Keeping that distinction in this queue prevents managed EPI callers
    /// from observing raw `PiBusy` contention between guest threads.
    pending_pi_completions: std::collections::VecDeque<PendingPiCompletion>,
    pending_si_completion: Option<PendingSiCompletion>,
    completed_pfs_is_plug: std::collections::BTreeMap<ThreadId, PfsIsPlugTransaction>,
    /// Libultra Controller Manager policy above the raw PIF device model.
    /// Explicit raw Joybus packets retain their encoded channel addressing;
    /// high-level query/read adapters poll only this manager-selected prefix.
    controller_manager: ControllerManagerState,
    /// Public `OSViMode` register image queued by `osViSetMode`; the VI
    /// manager applies it at the next V-blank rather than at the shim call.
    pending_vi_mode: Option<PendingViMode>,
    /// Last latched mode's two field-dependent register images. Common
    /// registers latch once; these five words alternate with VI field parity.
    active_vi_mode: Option<PendingViMode>,
    /// Standalone VI control update queued by `osViSetSpecialFeatures` when
    /// no mode image is pending. Multiple calls accumulate in order.
    pending_vi_control: Option<u32>,
    /// Public X/Y scale coefficients queued for the next VI interrupt.
    pending_vi_x_scale: Option<f32>,
    pending_vi_y_scale: Option<f32>,
    /// Active coefficients multiply the selected mode's 2.10 register base.
    active_vi_x_scale: f32,
    active_vi_y_scale: f32,
    /// Process-wide allocation used by typed raw-MMIO starts. Managed shim
    /// starts record their call-local pointer/required extent directly.
    runtime_rdram: *mut u8,
    runtime_rdram_len: usize,
    /// CPU-side images of immutable `OSTask::ucode_boot` ranges. The public
    /// task contract points these fields at rspboot text, and the real CPU's
    /// non-coherent data cache can retain that text while a CIC/custom RSP
    /// task writes a response over the same physical DRAM bytes. Generated
    /// CPU code and devices otherwise share one host buffer, so this typed
    /// cache preserves the source image which `osSpTaskLoad` re-DMAs at the
    /// beginning of every task.
    rsp_boot_images: std::collections::HashMap<u32, Vec<u8>>,
    /// Exact CPU task header/source most recently admitted to RSP DMEM. The
    /// only StartGo path consumes this token; it never rereads mutable guest
    /// task fields after Load returns.
    loaded_rsp_task: Option<task_dispatch::LoadedRspTask>,
    /// Original task/data identities retained independently of append-only
    /// observation history so a yielded reload cannot inherit evidence from
    /// an unrelated task that reused the same guest address.
    rsp_task_lineages: std::collections::HashMap<u32, task_dispatch::RspTaskLineage>,
    /// Installed-ROM audio microcode executor selected atomically with any
    /// translated callback identity. It is reset only when a new ROM is
    /// installed and is immutable for that ROM session.
    audio_task_execution: task_dispatch::AudioTaskExecutionPolicy,
    audio_task_execution_admitted: bool,
    audio_task_execution_started: bool,
    /// Guest-visible `OSPiHandle*` returned by `osCartRomInit`. The handle
    /// storage is game-owned BSS, so the boot host supplies its link address;
    /// leaving it unset is a loud trap rather than returning a stale `$v0`.
    cart_rom_handle_vram: Option<u32>,
    /// Guest BSS storage for the `OSPiHandle*` returned by `osFlashInit`.
    /// Flash and cartridge handles describe different PI domains and must
    /// never be silently aliased.
    flash_handle_vram: Option<u32>,
    /// Host-supplied 64DD-register EPI handle. Disk images and their timing
    /// are optional runtime inputs, so retail cartridge boots leave this
    /// absent and a real 64DD caller must configure it explicitly.
    leo_disk: Option<pi::LeoDiskConfig>,
    /// Stateful FlashRAM command sequencing (write buffer and through-erase
    /// completion) lives beside the one save backing store it controls.
    flash: save::FlashState,
    /// Host-selected development-hardware profile used by the clean-room
    /// `__checkHardware_*` shims. A retail/default host reports none rather
    /// than pretending an unavailable debug transport exists.
    debug_hardware: debug::DebugHardware,
    /// Lossless host side of the RDB/printf transport. Debug output must not
    /// disappear into a silent no-op; shells consume this queue explicitly.
    debug_packets: Vec<debug::DebugPacket>,
    /// Successful authoritative save/PFS operations retained for the live
    /// release gate. Requests are appended only after their storage action
    /// succeeds; rejected calls and staging-only operations never appear.
    save_operations: Vec<fn64_runtime::SaveOperationEvent>,
    /// Successful controller/accessory behavior retained for release-matrix
    /// admission. Configuration and probes are excluded; only guest-visible
    /// reads, writes, or controls append here.
    controller_operations: Vec<fn64_runtime::ControllerOperationEvent>,
    /// Exact microcode-recognition and committed RSP/RDP mechanism history.
    /// This is release observation, not future-affecting device state.
    rsp_rdp_observations: Vec<RspRdpObservationEvent>,
    /// Stable destinations entered by prepared generated-C bodies. Native
    /// pointers are retained only in the registration map used to translate
    /// the in-body hook back to generated section identity.
    native_execution_destinations: Vec<NativeExecutionDestinationEvent>,
    native_destination_by_pointer: std::collections::HashMap<usize, NativeExecutionDestination>,
    /// `OSThread*` (rdram-relative offset) -> `OSId`, populated by
    /// `osCreateThread_recomp`. Needed because real call sites pass the SAME
    /// `OSThread*` handle to `osStartThread`/`osSetThreadPri`/etc, NOT the
    /// `OSId` a second time -- see `osCreateThread_recomp`'s doc comment for
    /// the real disassembly evidence that disproved a prior wave's opposite
    /// assumption.
    thread_handles: std::collections::HashMap<u32, ThreadId>,
    /// Executor `ThreadId` -> the OSId the guest's `osCreateThread` actually
    /// supplied. Libultra's OSId is an informational tag with NO uniqueness
    /// contract -- thread identity on real hardware is the OSThread struct
    /// pointer, and NWXE's retail boot creates two live threads with id 3
    /// (`func_80001410`'s create is gated on a `.data` byte whose ROM
    /// initializer is 0x01, then `func_80026DE0` reuses id 3 for the audio
    /// manager -- see docs/BOOT-NOTES-WM2000.md, 2026-07-19). On a
    /// collision `osCreateThread_recomp` keys the executor by a synthetic
    /// id instead, and this map preserves the guest-visible OSId for
    /// `osGetThreadId_recomp`.
    thread_guest_ids: std::collections::HashMap<ThreadId, u32>,
    /// Next synthetic executor id handed out on an OSId collision. Starts
    /// far above any plausible guest OSId so remapped threads are obvious
    /// in trace output.
    next_synthetic_thread_id: ThreadId,
    /// `OSTimer*` (rdram-relative offset) -> `TimerId`, populated by
    /// `osSetTimer_recomp`. Same shape as `thread_handles`: a real
    /// `osStopTimer(t)` call site (OoT's boot-critical set, per
    /// BOOT-PLAN.md's rung-13 note) passes the SAME `OSTimer*` struct
    /// address `osSetTimer` was given, never the `TimerWheel`-internal
    /// `TimerId` a second time.
    timer_handles: std::collections::HashMap<u32, fn64_runtime::timer::TimerId>,
    /// Typed-Rust whole-ROM dispatcher installed by an rs-lane boot host. When
    /// present, `osCreateThread` resolves the new OSThread's entry through
    /// this table and owns an rs-lane `RecompContext` inside the SAME executor
    /// coroutine used by the C path.
    #[cfg(feature = "recomp-rs")]
    recompiled_lookup: Option<fn(u32) -> fn64_recomp_rs::RecompFunc>,
    /// Bank-qualified arbitrary-PC program installed by the universal rs
    /// lane. This is mutually exclusive with `recompiled_lookup`: spawned
    /// OSThreads enter through its explicit PC-to-bank resolver and retain
    /// the same owned program/generation identities as thread 0.
    #[cfg(feature = "recomp-rs")]
    recompiled_program: Option<recompiled::LiveBlockProgram>,
    /// Length of the process-wide RDRAM/MMIO allocation behind `ACTIVE_RDRAM`.
    /// Required to rebuild the checked rs-lane `Rdram` view at a spawned
    /// thread's entry without creating a second memory model or allocation.
    #[cfg(feature = "recomp-rs")]
    recompiled_rdram_len: usize,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            sections: SectionRegistry::new(),
            device_fabric: DeviceFabric::new(
                PiDma::new(InMemoryRom::new(Vec::new())),
                FixedPiTiming(Cycles::new(1)),
            ),
            rom_installed: false,
            installed_rom: None,
            cartridge_save: CartridgeSaveEvidenceSnapshot::Unidentified,
            pending_pi_completions: std::collections::VecDeque::new(),
            pending_si_completion: None,
            completed_pfs_is_plug: std::collections::BTreeMap::new(),
            controller_manager: ControllerManagerState::default(),
            pending_vi_mode: None,
            active_vi_mode: None,
            pending_vi_control: None,
            pending_vi_x_scale: None,
            pending_vi_y_scale: None,
            active_vi_x_scale: 1.0,
            active_vi_y_scale: 1.0,
            runtime_rdram: std::ptr::null_mut(),
            runtime_rdram_len: 0,
            rsp_boot_images: std::collections::HashMap::new(),
            loaded_rsp_task: None,
            rsp_task_lineages: std::collections::HashMap::new(),
            audio_task_execution: task_dispatch::AudioTaskExecutionPolicy::Unconfigured,
            audio_task_execution_admitted: false,
            audio_task_execution_started: false,
            cart_rom_handle_vram: None,
            flash_handle_vram: None,
            leo_disk: None,
            flash: save::FlashState::default(),
            debug_hardware: debug::DebugHardware::default(),
            debug_packets: Vec::new(),
            save_operations: Vec::new(),
            controller_operations: Vec::new(),
            rsp_rdp_observations: Vec::new(),
            native_execution_destinations: Vec::new(),
            native_destination_by_pointer: std::collections::HashMap::new(),
            thread_handles: std::collections::HashMap::new(),
            thread_guest_ids: std::collections::HashMap::new(),
            next_synthetic_thread_id: 0xF000_0000,
            timer_handles: std::collections::HashMap::new(),
            #[cfg(feature = "recomp-rs")]
            recompiled_lookup: None,
            #[cfg(feature = "recomp-rs")]
            recompiled_program: None,
            #[cfg(feature = "recomp-rs")]
            recompiled_rdram_len: 0,
        }
    }
}

/// Select the IPL television standard for the shared VI/AI clock authority.
/// Returns the currently armed VI field interval in guest CPU cycles.
pub fn configure_tv_type(tv_type: fn64_runtime::TvType) -> u64 {
    with_host(|host| {
        host.device_fabric
            .configure_tv_type(tv_type)
            .unwrap_or_else(|error| panic!("configure_tv_type failed: {error}"))
            .get()
    })
}

/// Active public video clock. Hosts that have not installed IPL state retain
/// the historical NTSC default; real boot allocations call
/// [`configure_tv_type`] before guest code starts.
pub fn vi_clock_hz() -> u32 {
    with_host(|host| {
        host.device_fabric
            .tv_type()
            .unwrap_or_default()
            .vi_clock_hz()
    })
}

pub fn configured_tv_type() -> fn64_runtime::TvType {
    with_host(|host| host.device_fabric.tv_type().unwrap_or_default())
}

pub fn vi_field_interval() -> Option<u64> {
    with_host(|host| host.device_fabric.vi_field_interval().map(Cycles::get))
}

/// Exact currently scheduled VI interrupt edge. Unlike reconstructing an edge
/// from a host-owned interval accumulator, this remains monotonic when guest
/// checkpoints advance time and follows VI timing-register reschedules.
pub fn next_vi_deadline() -> Option<u64> {
    with_host(|host| host.device_fabric.next_vi_deadline().map(Cycles::get))
}

/// Guest-visible device state for a fixed-cycle release digest.
///
/// The snapshot is read from the same `DeviceFabric` used by raw MMIO and
/// libultra shims. Hosts must compare `snapshot.now` with the executor's
/// [`sim_time`] before making a fixed-cycle claim; `FixedCycleDigestGate`
/// enforces that equality when the snapshot is captured.
pub fn device_snapshot() -> fn64_runtime::DeviceSnapshot {
    with_host(|host| host.device_fabric.snapshot())
}

/// Complete modeled-fabric state for the fixed-cycle release artifact.
/// Unlike [`device_snapshot`], this includes future-affecting internal
/// memories, queues, deadlines, policy bytes, and cartridge save state.
pub fn device_evidence_snapshot() -> fn64_runtime::DeviceEvidenceSnapshot {
    with_host(|host| host.device_fabric.evidence_snapshot())
}

/// Complete pointer-free scheduler, queue, timer, event-registration, clock,
/// and registered-RDRAM control state owned by the live executor. Native
/// coroutine continuations remain outside this portable evidence seam.
pub fn executor_control_evidence_snapshot() -> fn64_runtime::ExecutorControlEvidenceSnapshot {
    with_executor(|executor| executor.control_evidence_snapshot())
}

/// Complete executor-owned controller, accessory, and high-level VI state for
/// the fixed-cycle release artifact. This complements the device-fabric view:
/// neither owner is treated as a proxy for the other.
pub fn peripherals_evidence_snapshot() -> RuntimePeripheralEvidenceSnapshot {
    let peripherals = with_executor(|executor| executor.peripherals_evidence_snapshot());
    with_host(|host| runtime_peripherals_from_host(host, peripherals))
}

fn runtime_peripherals_from_host(
    host: &HostState,
    peripherals: fn64_runtime::PeripheralsEvidenceSnapshot,
) -> RuntimePeripheralEvidenceSnapshot {
    RuntimePeripheralEvidenceSnapshot {
        peripherals,
        pending_pi_completions: host
            .pending_pi_completions
            .iter()
            .map(|pending| {
                assert_eq!(
                    pending.rdram, host.runtime_rdram,
                    "pending PI completion does not reference the registered process RDRAM"
                );
                assert_eq!(
                    pending.rdram_len, host.runtime_rdram_len,
                    "pending PI completion has a different process RDRAM extent"
                );
                PendingPiCompletionEvidenceSnapshot {
                    request: pending.request,
                    rdram_len: u64::try_from(pending.rdram_len)
                        .expect("process RDRAM length exceeds release-evidence wire"),
                    ret_queue: pending.ret_queue,
                    ret_mesg: pending.ret_mesg,
                }
            })
            .collect(),
        pending_si_completion: host.pending_si_completion.map(|pending| {
            let owner = match pending.owner {
                PendingSiCompletionOwner::ProcessRdram { rdram, rdram_len } => {
                    assert_eq!(
                        rdram, host.runtime_rdram,
                        "pending SI completion does not reference the registered process RDRAM"
                    );
                    assert_eq!(
                        rdram_len, host.runtime_rdram_len,
                        "pending SI completion has a different process RDRAM extent"
                    );
                    PendingSiCompletionOwnerEvidenceSnapshot::ProcessRdram {
                        rdram_len: u64::try_from(rdram_len)
                            .expect("process RDRAM length exceeds release-evidence wire"),
                    }
                }
                PendingSiCompletionOwner::OsEvent => {
                    PendingSiCompletionOwnerEvidenceSnapshot::OsEvent
                }
                PendingSiCompletionOwner::PfsIsPlug(transaction) => {
                    PendingSiCompletionOwnerEvidenceSnapshot::PfsIsPlug(transaction.into())
                }
            };
            PendingSiCompletionEvidenceSnapshot {
                request: pending.request,
                owner,
            }
        }),
        completed_pfs_is_plug: host
            .completed_pfs_is_plug
            .values()
            .copied()
            .map(Into::into)
            .collect(),
        vi: AbiViEvidenceSnapshot {
            pending_mode: host
                .pending_vi_mode
                .map(|mode| PendingViModeEvidenceSnapshot {
                    registers: mode.registers,
                    fields: mode.fields,
                }),
            active_mode: host
                .active_vi_mode
                .map(|mode| PendingViModeEvidenceSnapshot {
                    registers: mode.registers,
                    fields: mode.fields,
                }),
            pending_control: host.pending_vi_control,
            pending_x_scale_bits: host.pending_vi_x_scale.map(f32::to_bits),
            pending_y_scale_bits: host.pending_vi_y_scale.map(f32::to_bits),
            active_x_scale_bits: host.active_vi_x_scale.to_bits(),
            active_y_scale_bits: host.active_vi_y_scale.to_bits(),
        },
    }
}

/// Compiler-enforced classification of every HostState field. Adding a field
/// requires deciding whether it enters this owner snapshot, an existing
/// release channel, a diagnostic-only exclusion, or the recompiler companion
/// seam; there is deliberately no `..` escape hatch.
fn classify_host_evidence_fields(host: &HostState) {
    let HostState {
        sections: _,
        // Already complete in `device_evidence_snapshot`; this aggregate
        // binds only the ABI-owned state above that raw fabric.
        device_fabric: _,
        rom_installed: _,
        installed_rom: _,
        cartridge_save: _,
        pending_pi_completions: _,
        pending_si_completion: _,
        completed_pfs_is_plug: _,
        controller_manager: _,
        pending_vi_mode: _,
        active_vi_mode: _,
        pending_vi_control: _,
        pending_vi_x_scale: _,
        pending_vi_y_scale: _,
        active_vi_x_scale: _,
        active_vi_y_scale: _,
        runtime_rdram: _,
        runtime_rdram_len: _,
        rsp_boot_images: _,
        loaded_rsp_task: _,
        rsp_task_lineages: _,
        audio_task_execution: _,
        // Configuration guard only; with an immutable installed policy it
        // cannot change a later guest result independently of that policy.
        audio_task_execution_admitted: _,
        audio_task_execution_started: _,
        cart_rom_handle_vram: _,
        flash_handle_vram: _,
        leo_disk: _,
        flash: _,
        debug_hardware: _,
        // Append-only evidence/diagnostic outputs cannot affect a later ABI
        // result and are tested not to enter the snapshot.
        debug_packets: _,
        save_operations: _,
        controller_operations: _,
        rsp_rdp_observations: _,
        native_execution_destinations: _,
        native_destination_by_pointer: _,
        thread_handles: _,
        thread_guest_ids: _,
        next_synthetic_thread_id: _,
        timer_handles: _,
        // These have a separately owned, feature-gated evidence seam in
        // `recompiled`; the schema aggregator must bind that companion.
        #[cfg(feature = "recomp-rs")]
            recompiled_lookup: _,
        #[cfg(feature = "recomp-rs")]
            recompiled_program: _,
        #[cfg(feature = "recomp-rs")]
            recompiled_rdram_len: _,
    } = host;
}

/// Capture every ABI-owned HostState field that can change a later result,
/// without consuming queues or exposing process-specific pointers.
///
/// This is an owner-local seam for the current release schema; historical
/// schemas that omitted this aggregate must not be relabeled as if they did.
pub fn host_evidence_snapshot() -> AbiHostEvidenceSnapshot {
    let peripherals = with_executor(|executor| executor.peripherals_evidence_snapshot());
    with_host(|host| {
        classify_host_evidence_fields(host);
        assert_eq!(
            host.rom_installed,
            host.installed_rom.is_some(),
            "ROM installation flag and retained ROM identity disagree"
        );
        if let Some(installed) = host.installed_rom {
            assert_eq!(
                installed.byte_len,
                u64::try_from(host.device_fabric.pi_dma_mut().rom_len())
                    .expect("installed ROM length exceeds evidence wire"),
                "retained ROM length disagrees with the live PI engine"
            );
        }
        let installed_save_len = host.device_fabric.pi_dma_mut().save_len();
        match host.cartridge_save {
            CartridgeSaveEvidenceSnapshot::Unidentified => {}
            CartridgeSaveEvidenceSnapshot::NoCartridgeSave => assert!(
                installed_save_len.is_none(),
                "no-cartridge-save evidence disagrees with installed PI save storage"
            ),
            CartridgeSaveEvidenceSnapshot::Configured(save_type) => assert_eq!(
                installed_save_len,
                Some(save_type.byte_len()),
                "typed cartridge-save evidence disagrees with installed PI save storage"
            ),
        }

        let rdram_present = !host.runtime_rdram.is_null();
        assert_eq!(
            rdram_present,
            host.runtime_rdram_len != 0,
            "registered RDRAM pointer presence and length disagree"
        );

        let mut rsp_boot_images: Vec<_> = host
            .rsp_boot_images
            .iter()
            .map(|(&rdram_offset, bytes)| RspBootImageEvidenceSnapshot {
                rdram_offset,
                bytes: bytes.clone(),
            })
            .collect();
        rsp_boot_images.sort_unstable_by_key(|image| image.rdram_offset);

        let loaded_rsp_task = host
            .loaded_rsp_task
            .as_ref()
            .map(task_dispatch::LoadedRspTask::evidence_snapshot);
        let mut rsp_task_lineages: Vec<_> = host
            .rsp_task_lineages
            .iter()
            .map(|(&task_offset, lineage)| lineage.evidence_snapshot(task_offset))
            .collect();
        rsp_task_lineages.sort_unstable_by_key(|lineage| lineage.task_offset);

        let mut thread_handles: Vec<_> = host
            .thread_handles
            .iter()
            .map(
                |(&osthread_offset, &executor_thread_id)| ThreadHandleEvidenceSnapshot {
                    osthread_offset,
                    executor_thread_id,
                },
            )
            .collect();
        thread_handles.sort_unstable_by_key(|entry| entry.osthread_offset);

        let mut thread_guest_ids: Vec<_> = host
            .thread_guest_ids
            .iter()
            .map(
                |(&executor_thread_id, &guest_os_id)| ThreadGuestIdEvidenceSnapshot {
                    executor_thread_id,
                    guest_os_id,
                },
            )
            .collect();
        thread_guest_ids.sort_unstable_by_key(|entry| entry.executor_thread_id);

        let mut timer_handles: Vec<_> = host
            .timer_handles
            .iter()
            .map(|(&ostimer_offset, &timer_id)| TimerHandleEvidenceSnapshot {
                ostimer_offset,
                timer_id,
            })
            .collect();
        timer_handles.sort_unstable_by_key(|entry| entry.ostimer_offset);

        AbiHostEvidenceSnapshot {
            runtime_peripherals: runtime_peripherals_from_host(host, peripherals),
            controller_manager: host.controller_manager.evidence_snapshot(),
            flash: host.flash.evidence_snapshot(),
            sections: host.sections.evidence_snapshot(),
            rsp_boot_images,
            loaded_rsp_task,
            rsp_task_lineages,
            audio_task_execution: host.audio_task_execution,
            rom_installed: host.rom_installed,
            installed_rom: host.installed_rom,
            cartridge_save: host.cartridge_save,
            cart_rom_handle_vram: host.cart_rom_handle_vram,
            flash_handle_vram: host.flash_handle_vram,
            leo_disk: host.leo_disk,
            thread_handles,
            thread_guest_ids,
            timer_handles,
            next_synthetic_thread_id: host.next_synthetic_thread_id,
            registered_rdram: RegisteredRdramEvidenceSnapshot {
                present: rdram_present,
                byte_len: u64::try_from(host.runtime_rdram_len)
                    .expect("registered RDRAM length exceeds evidence wire"),
            },
            debug_hardware: host.debug_hardware,
        }
    })
}

/// Copy the typed, guest-cycle-ordered device-fabric transition trace.
///
/// Unlike the executor's optional differential trace, this is the fabric's
/// authoritative record of accepted and committed PI/SI/AI/SP operations.
/// Release evidence uses commit/completion variants rather than inferring DMA
/// activity from queue traffic, addresses, or shim names.
pub fn copy_device_trace() -> Vec<fn64_runtime::DeviceTraceEvent> {
    with_host(|host| host.device_fabric.trace().to_vec())
}

/// Copy successful typed save operations observed through authoritative ABI
/// storage boundaries. This is separate from device DMA trace because PFS and
/// synchronous Flash APIs do not traverse `DeviceFabric`.
pub fn copy_save_operations() -> Vec<fn64_runtime::SaveOperationEvent> {
    with_host(|host| {
        assert!(
            host.device_fabric.pi_dma().save_operations().is_empty(),
            "copy_save_operations: undrained PiDma observations make cross-owner save-operation order unknowable"
        );
        host.save_operations.clone()
    })
}

/// Copy successful controller/accessory operations in guest-cycle and call
/// order. Unlike the frozen peripheral state, this proves that a configured
/// device was actually exercised by the guest.
pub fn copy_controller_operations() -> Vec<fn64_runtime::ControllerOperationEvent> {
    with_host(|host| host.controller_operations.clone())
}

/// Copy exact committed RSP/RDP observations in ABI execution order.
///
/// Capability lists, task headers, and pending device requests never enter
/// this history. Microcode entries require an exact lookup over the complete
/// live IMEM image; mechanism entries require their corresponding memory or
/// renderer commit to have succeeded.
pub fn copy_rsp_rdp_observations() -> Vec<RspRdpObservationEvent> {
    with_host(|host| host.rsp_rdp_observations.clone())
}

pub(crate) fn record_rsp_rdp_observations(kinds: Vec<RspRdpObservationKind>) {
    if kinds.is_empty() {
        return;
    }
    let at = Cycles::new(sim_time());
    with_host(|host| {
        host.rsp_rdp_observations.extend(
            kinds
                .into_iter()
                .map(|kind| RspRdpObservationEvent { at, kind }),
        );
    });
}

pub(crate) fn record_controller_operation(
    port: usize,
    device: fn64_runtime::ControllerOperationDevice,
    operation: fn64_runtime::ControllerOperationKind,
) {
    let port = u8::try_from(port).expect("controller operation port exceeds u8");
    assert!(
        port < 4,
        "controller operation port {port} exceeds four-port PIF"
    );
    let at = Cycles::new(sim_time());
    with_host(|host| {
        host.controller_operations
            .push(fn64_runtime::ControllerOperationEvent {
                at,
                port,
                device,
                operation,
            });
    });
}

pub(crate) fn record_save_operation(
    device: fn64_runtime::SaveType,
    operation: fn64_runtime::SaveOperationKind,
    offset: usize,
    len: usize,
) {
    assert!(len > 0, "save evidence cannot record a zero-byte operation");
    let offset = u32::try_from(offset).expect("save evidence offset exceeds u32");
    let len = u32::try_from(len).expect("save evidence length exceeds u32");
    let at = Cycles::new(sim_time());
    with_host(|host| {
        host.save_operations.push(fn64_runtime::SaveOperationEvent {
            at,
            device,
            operation,
            offset,
            len,
        });
    });
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

enum ExecutorSlot {
    Active(Box<Executor>),
    PreparedForProcessExit,
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
    static EXECUTOR: ReentrantCell<ExecutorSlot> =
        ReentrantCell::new(ExecutorSlot::Active(Box::new(Executor::new())));

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
    EXECUTOR.with(|slot| {
        slot.with(|slot| match slot {
            ExecutorSlot::Active(executor) => f(executor),
            ExecutorSlot::PreparedForProcessExit => {
                panic!("fn64 executor used after prepare_process_exit detached its guest stacks")
            }
        })
    })
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
    // Fresh back-edge stall budget for this scheduling slice: the forced-
    // checkpoint threshold (`fn64_c_backedge`) bounds a spin *within one
    // resume*, so it must start from zero on every resume, not accumulate
    // across a thread's whole lifetime.
    reset_backedge_budget();
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
    // Fresh back-edge stall budget for this scheduling slice: the forced-
    // checkpoint threshold (`fn64_c_backedge`) bounds a spin *within one
    // resume*, so it must start from zero on every resume, not accumulate
    // across a thread's whole lifetime.
    reset_backedge_budget();
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
    // Every OS-call yield charges a flat slice of guest time first. The C
    // lane has no instruction counting, so without this a guest loop of
    // OS calls (e.g. a degenerate audio refeed) generates unbounded work
    // per virtual instant -- impossible on silicon, where the call path
    // itself costs cycles. Checkpoint yields are exempt: those lanes
    // already count their own instructions.
    // ponytail: one flat cost; per-call calibration belongs to the R5
    // faithful-rate work if a title ever needs it.
    const C_LANE_OS_CALL_CYCLES: u32 = 250;
    if !matches!(yield_value, Yield::InstructionCheckpoint { .. }) {
        let _ = yielder.suspend(Yield::InstructionCheckpoint {
            instructions: C_LANE_OS_CALL_CYCLES,
        });
    }
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

    pub(crate) fn fpr_u64_bits(&self) -> [u64; 32] {
        // Safety: each union contains a valid raw 64-bit pattern regardless
        // of which typed member the generated shim last wrote.
        unsafe {
            [
                self.f0.u64_bits,
                self.f1.u64_bits,
                self.f2.u64_bits,
                self.f3.u64_bits,
                self.f4.u64_bits,
                self.f5.u64_bits,
                self.f6.u64_bits,
                self.f7.u64_bits,
                self.f8.u64_bits,
                self.f9.u64_bits,
                self.f10.u64_bits,
                self.f11.u64_bits,
                self.f12.u64_bits,
                self.f13.u64_bits,
                self.f14.u64_bits,
                self.f15.u64_bits,
                self.f16.u64_bits,
                self.f17.u64_bits,
                self.f18.u64_bits,
                self.f19.u64_bits,
                self.f20.u64_bits,
                self.f21.u64_bits,
                self.f22.u64_bits,
                self.f23.u64_bits,
                self.f24.u64_bits,
                self.f25.u64_bits,
                self.f26.u64_bits,
                self.f27.u64_bits,
                self.f28.u64_bits,
                self.f29.u64_bits,
                self.f30.u64_bits,
                self.f31.u64_bits,
            ]
        }
    }

    pub(crate) fn set_fpr_u64_bits(&mut self, bits: [u64; 32]) {
        self.f0 = Fpr { u64_bits: bits[0] };
        self.f1 = Fpr { u64_bits: bits[1] };
        self.f2 = Fpr { u64_bits: bits[2] };
        self.f3 = Fpr { u64_bits: bits[3] };
        self.f4 = Fpr { u64_bits: bits[4] };
        self.f5 = Fpr { u64_bits: bits[5] };
        self.f6 = Fpr { u64_bits: bits[6] };
        self.f7 = Fpr { u64_bits: bits[7] };
        self.f8 = Fpr { u64_bits: bits[8] };
        self.f9 = Fpr { u64_bits: bits[9] };
        self.f10 = Fpr { u64_bits: bits[10] };
        self.f11 = Fpr { u64_bits: bits[11] };
        self.f12 = Fpr { u64_bits: bits[12] };
        self.f13 = Fpr { u64_bits: bits[13] };
        self.f14 = Fpr { u64_bits: bits[14] };
        self.f15 = Fpr { u64_bits: bits[15] };
        self.f16 = Fpr { u64_bits: bits[16] };
        self.f17 = Fpr { u64_bits: bits[17] };
        self.f18 = Fpr { u64_bits: bits[18] };
        self.f19 = Fpr { u64_bits: bits[19] };
        self.f20 = Fpr { u64_bits: bits[20] };
        self.f21 = Fpr { u64_bits: bits[21] };
        self.f22 = Fpr { u64_bits: bits[22] };
        self.f23 = Fpr { u64_bits: bits[23] };
        self.f24 = Fpr { u64_bits: bits[24] };
        self.f25 = Fpr { u64_bits: bits[25] };
        self.f26 = Fpr { u64_bits: bits[26] };
        self.f27 = Fpr { u64_bits: bits[27] };
        self.f28 = Fpr { u64_bits: bits[28] };
        self.f29 = Fpr { u64_bits: bits[29] };
        self.f30 = Fpr { u64_bits: bits[30] };
        self.f31 = Fpr { u64_bits: bits[31] };
    }

    pub(crate) fn assert_float_mode_matches_status(&self) {
        const STATUS_FR: u32 = 1 << 26;
        assert!(
            self.mips3_float_mode <= 1,
            "recomp_context mips3_float_mode must be 0 or 1"
        );
        assert_eq!(
            self.mips3_float_mode == 1,
            self.status_reg & STATUS_FR != 0,
            "recomp_context status_reg.FR and mips3_float_mode diverged"
        );
    }

    /// Point `f_odd` at this context's active FPR view so recompiled odd
    /// single-register accesses land in-register instead of faulting.
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
    /// 8-byte stride) the generated C was emitted against. With FR=1 the
    /// cursor instead starts at `f1.u32l`, making the same index expression
    /// reach each odd independent FPR's low word.
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
        self.assert_float_mode_matches_status();
        self.f_odd = if self.mips3_float_mode == 0 {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut self.f0.u32_halves.1 as *mut u32 }
        } else {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut self.f1.u32_halves.0 as *mut u32 }
        };
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
mod debug;
mod dispatch;
mod gbpak;
mod host;
mod mesgqueue;
mod pfs;
mod pi;
mod save;
mod si;
mod softmath;
mod sp_dp;
mod system;
mod task_dispatch;
mod thread;
mod timer;
mod vi;
mod voice;

pub use ai::*;
pub use cache::*;
pub use debug::*;
pub use dispatch::*;
pub use gbpak::*;
pub use host::*;
pub use mesgqueue::*;
pub use pfs::*;
pub use pi::*;
pub use save::*;
pub use si::*;
pub use softmath::*;
pub use sp_dp::*;
pub use system::*;
pub use task_dispatch::*;
pub use thread::*;
pub use timer::*;
pub use vi::*;
pub use voice::*;

#[cfg(test)]
mod test_support;
