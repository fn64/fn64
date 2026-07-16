use super::*;

/// Decode fn64's native-word RDRAM representation in guest halfword order and
/// deliver one real AI DMA buffer to the registered backend.
///
/// # Safety
///
/// `rdram` must be valid for the length registered through
/// [`set_audio_rdram_len`].
pub(crate) unsafe fn deliver_ai_buffer(rdram: *mut u8, start: usize, byte_len: usize) {
    let rdram_len = AUDIO_RDRAM_LEN.with(Cell::get);
    let end = start.checked_add(byte_len);
    assert!(
        !rdram.is_null()
            && rdram_len.is_multiple_of(4)
            && end.is_some_and(|end| end <= rdram_len)
            && start.is_multiple_of(2)
            && byte_len.is_multiple_of(2),
        "osAiSetNextBuffer: invalid AI PCM range start={start:#x} bytes={byte_len:#x} rdram_len={rdram_len:#x}"
    );

    // `MEM_H(addr)` reads a native-endian halfword at `(addr ^ 2)`. Apply
    // that exact byte-lane rule per sample so stereo order is preserved; a
    // flat chunks_exact(2) walk swaps every adjacent L/R pair in fn64's
    // native-word backing representation.
    let bytes = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
    let samples: Vec<i16> = (0..byte_len)
        .step_by(2)
        .map(|guest_offset| {
            let physical = (start + guest_offset) ^ 2;
            i16::from_ne_bytes([bytes[physical], bytes[physical + 1]])
        })
        .collect();

    let nonzero = samples.iter().filter(|&&sample| sample != 0).count() as u64;
    let buffer_min = samples.iter().copied().min();
    let buffer_max = samples.iter().copied().max();
    AUDIO_OUTPUT_STATS.with(|cell| {
        let mut stats = cell.get();
        stats.ai_buffers += 1;
        stats.samples += samples.len() as u64;
        stats.nonzero_samples += nonzero;
        stats.min = match (stats.min, buffer_min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (None, value) | (value, None) => value,
        };
        stats.max = match (stats.max, buffer_max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (None, value) | (value, None) => value,
        };
        cell.set(stats);
    });

    // One-shot evidence hook: write the first non-silent live AI buffer as
    // signed 16-bit little-endian PCM, plus self-describing metadata. Waiting
    // for nonzero avoids capturing an expected startup-silence buffer and
    // falsely concluding the synth stayed silent.
    if nonzero != 0 {
        if let Some(path) = std::env::var_os("OOT_DUMP_AUDIO_PCM") {
            AUDIO_PCM_DUMPED.with(|dumped| {
                if !dumped.get() {
                    let pcm: Vec<u8> = samples
                        .iter()
                        .flat_map(|sample| sample.to_le_bytes())
                        .collect();
                    let path = std::path::Path::new(&path);
                    match std::fs::write(path, pcm) {
                        Ok(()) => {
                            let meta = format!(
                                "format=s16le\nchannels=2\nsamples={}\nframes={}\nrange={}..={}\nnonzero_samples={}\nrdram_offset={start}\n",
                                samples.len(),
                                samples.len() / 2,
                                buffer_min.unwrap_or(0),
                                buffer_max.unwrap_or(0),
                                nonzero,
                            );
                            if let Err(error) = std::fs::write(path.with_extension("meta"), meta) {
                                eprintln!("[fn64-abi] failed to write PCM metadata: {error}");
                            }
                            eprintln!(
                                "[fn64-abi] dumped live AI PCM: {} samples, range {}..={} to {path:?}",
                                samples.len(),
                                buffer_min.unwrap_or(0),
                                buffer_max.unwrap_or(0),
                            );
                            dumped.set(true);
                        }
                        Err(error) => eprintln!("[fn64-abi] failed to dump live AI PCM: {error}"),
                    }
                }
            });
        }
    }

    AUDIO_BACKEND.with(|cell| {
        if let Some(backend) = cell.borrow_mut().as_mut() {
            let result = backend.queue_samples(&samples);
            if result.is_ok() {
                AUDIO_OUTPUT_STATS.with(|cell| {
                    let mut stats = cell.get();
                    stats.backend_buffers += 1;
                    cell.set(stats);
                });
            }
            AUDIO_LAST_ERROR.with(|cell| cell.replace(result.err().map(|error| error.to_string())));
        }
    });
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
/// always sets `ctx.r2 = 0` (task complete, did NOT yield -- 0 is the
/// completed value; OS_TASK_YIELDED==1 is the yielded value, sptask.h:20) so
/// the caller's `beq $v0, $zero` path proceeds as if the RSP finished the
/// task, matching real hardware's observable effect on the caller (task done,
/// no re-queue) regardless of whether a backend actually drew anything.
///
/// AUDIO_TASK_NOTE: an audio task (`M_AUDTASK`) causes the registered translated audio ucode function (out-of-tree; e.g. `oot-audio-ucode`'s recompiled OoT aspMain, or WM2000's `wm2000_audio_ucode`, registered via `set_audio_ucode_fn` below -- `fn64-abi` itself contains no game-derived ucode, per `README.md`'s "no game content ships in this repo") to be REALLY CALLED with `(rdram, task_offset)` -- the OSTask's rdram offset, which a recompiled ucode uses to seed its RSP DMEM (see `AudioUcodeFn`'s doc comment). Its `RspExitReason` return is not yet interpreted beyond "it ran"; the header is still recorded via `submit_task`.
///
/// UNKNOWN_TASK_NOTE: an unrecognized task type is recorded (so the trace/count still sees it) but not executed, and this function still sets `ctx.r2 = 0` (complete) -- the same "acknowledge, don't fabricate real hardware effects" stance as the gfx path, since this milestone has no evidence for any other task type on NWXE's boot path.
///
/// Dispatch a graphics task (`M_GFXTASK`) to the registered `dyn
/// RenderBackend`, once, at the point the RSP is actually kicked. Extracted so
/// BOTH task-submission paths can call it: `osSpTaskStartGo_recomp` (the
/// Load+StartGo path OoT and most retail titles use) and
/// `osSpTaskYielded_recomp` (the yield/resume path). A prior version dispatched
/// ONLY from the yield path, so OoT (which never yields the RSP -- it
/// Load+StartGo's every frame) submitted 232 gfx tasks that the backend never
/// saw, producing blank frames. Callers guard on `header.task_type ==
/// M_GFXTASK` and pass the task header's rdram offset `o`.
///
/// If no backend is registered the call is a no-op (the task is still
/// counted by the caller's `submit_task`) -- same "acknowledge, never fake
/// success" stance as the audio path. A backend error is surfaced via
/// `RENDER_LAST_ERROR`, never a MIPS-side fault (real hardware can't report a
/// gfx-ucode failure back to the game thread either).
///
/// # Safety
/// `rdram` valid for the call; `o` a valid task-header offset within it.
unsafe fn dispatch_gfx_task(rdram: *mut u8, o: usize, header: &OsTaskHeader) {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
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
            let rdram_len = RDRAM_LEN.with(|cell| cell.get());
            let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
            // The color framebuffer the VI presents (`osViSwapBuffer`'s frame
            // buffer, e.g. OoT's 0x3b5000/0x3da800) -- NOT `task.output_buff`
            // (OoT's is 0x80151640, the RSP's DRAM command-FIFO output region,
            // a different address). The reference backend rasterizes into its
            // own surface and copies the result here so the VI-presented frame
            // isn't blank. `0` (no VI framebuffer set yet) tells the backend
            // "no known color target": it renders to its own surface only.
            let output_addr = current_vi_framebuffer().unwrap_or(0);
            let result = backend.process_task(rdram_slice, &task, output_addr);
            RENDER_LAST_ERROR.with(|cell| cell.replace(result.err().map(|e| e.to_string())));
        }
    });
    if let Some(started) = started {
        GFX_NS.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            );
        });
        GFX_CALLS.with(|calls| calls.set(calls.get() + 1));
    }
}

/// Present the registered graphics backend at the guest's real VI swap
/// boundary. Task submission and VI presentation are distinct on N64; this
/// closes the second half of `RenderBackend` without exposing RT64 or any
/// foreign type outside `fn64-render-rt64`.
pub(crate) fn present_render_backend() {
    RENDER_BACKEND.with(|cell| {
        if let Some(backend) = cell.borrow_mut().as_mut() {
            if let Err(error) = backend.present() {
                RENDER_LAST_ERROR.with(|last| last.replace(Some(error.to_string())));
            }
        }
    });
}

/// Dispatch an audio task (`M_AUDTASK`) once, at the point the RSP is kicked.
/// Two halves, both symmetric with `dispatch_gfx_task`:
///  1. Run the registered translated audio ucode (`AUDIO_UCODE_FN`,
///     `set_audio_ucode_fn`) with `(rdram, task_offset)` -- `o` is the OSTask's
///     rdram OFFSET, which the recompiled ucode's FFI wrapper uses to seed RSP
///     DMEM[0xFC0] (rspboot pre-loads the 64-byte OSTask there; aspMain's first
///     act is `lw 0x18(0xFC0)` = read `ucode_data`). No ucode registered -> the
///     task is still counted by the caller's `submit_task`, honestly reflecting
///     "submitted but this process never ran its ucode."
///  2. Sample delivery happens later at `osAiSetNextBuffer_recomp`, the public
///     AI DMA boundary where the CPU names the actual finished PCM range. It
///     cannot happen here: OoT's live task has zero `OSTask.output_buff`
///     fields and selects output destinations through `A_SAVEBUFF` commands.
///
/// Extracted so BOTH submission paths dispatch it, exactly like
/// `dispatch_gfx_task`: `osSpTaskStartGo_recomp` (the Load+StartGo path OoT's
/// audio driver uses -- `AudioMgr_HandleRetrace` -> scheduler -> `Sched_RunTask`
/// -> `osSpTaskLoad`+`osSpTaskStartGo`, never the yield path) and
/// `osSpTaskYielded_recomp`. A prior version dispatched the ucode ONLY from the
/// yield path, so a real OoT audio task submitted via StartGo would never have
/// run its ucode -- the same latent bug class the gfx path already hit and fixed.
///
/// # Safety
/// `rdram` valid for the call; `o` a valid task-header offset within it and
/// `header` the task header read from that offset.
unsafe fn dispatch_audio_task(rdram: *mut u8, o: usize, header: &OsTaskHeader) {
    debug_assert_eq!(header.task_type, M_AUDTASK);
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    // Debug/perf escape hatch: `OOT_SKIP_AUDIO_UCODE` skips running the
    // recompiled audio ucode (the per-frame RSP synth, currently unoptimized
    // and the dominant per-swap cost). The CPU-side audio driver
    // (AudioThread_UpdateImpl) and this task's completion event still run, so
    // the audio-reset handshake that unblocks Play_Init still completes -- the
    // ucode only produces the actual samples. Use it to iterate on the RENDERER
    // at normal boot speed; a real audio run must NOT set it. No sound when set.
    // Diagnostic: dump the exact rdram + task offset of the FIRST audio task so
    // the recompiled ucode can be replayed offline against the real command
    // list (env `OOT_DUMP_AUDIO_TASK=<path>`). One-shot.
    if let Some(path) = std::env::var_os("OOT_DUMP_AUDIO_TASK") {
        AUDIO_TASK_DUMPED.with(|c| {
            if !c.get() {
                let mut rdram_len = AUDIO_RDRAM_LEN.with(|cell| cell.get());
                if rdram_len == 0 {
                    rdram_len = RDRAM_LEN.with(|cell| cell.get());
                }
                // The harness allocation includes a sparse KSEG1/MMIO mirror
                // hundreds of MiB above physical RDRAM. Audio ucode masks DMA
                // addresses to the N64's physical RDRAM window, so replay only
                // needs the real 8 MiB image; dumping the whole host mapping
                // made one diagnostic task capture 656 MiB.
                rdram_len = rdram_len.min(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
                if rdram_len > 0 {
                    let bytes = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
                    let p = std::path::Path::new(&path);
                    let _ = std::fs::write(p, bytes);
                    let meta = format!("task_offset={o}\nrdram_len={rdram_len}\n");
                    let _ = std::fs::write(p.with_extension("meta"), meta);
                    eprintln!("[fn64-abi] dumped audio task rdram ({rdram_len} B) + task_offset={o} to {path:?}");
                }
                c.set(true);
            }
        });
    }
    let skip_ucode = std::env::var_os("OOT_SKIP_AUDIO_UCODE").is_some();
    if !skip_ucode {
        AUDIO_UCODE_FN.with(|cell| {
            if let Some(f) = cell.get() {
                // Safety: `set_audio_ucode_fn`'s doc comment is the contract --
                // `f` must be the real translated ucode function. See this
                // function's doc comment for why `o` (the OSTask rdram offset) is
                // the second argument, and `AudioUcodeFn`'s doc comment for the
                // widened meaning.
                if AUDIO_UCODE_TIMING.with(|c| c.get()) {
                    let t = std::time::Instant::now();
                    unsafe {
                        f(rdram, o as u32);
                    }
                    let ns = t.elapsed().as_nanos() as u64;
                    AUDIO_UCODE_NS.with(|c| c.set(c.get() + ns));
                    AUDIO_UCODE_CALLS.with(|c| c.set(c.get() + 1));
                } else {
                    unsafe {
                        f(rdram, o as u32);
                    }
                }
            }
        });
    }
    if let Some(started) = started {
        AUDIO_DISPATCH_NS.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            );
        });
        AUDIO_DISPATCH_CALLS.with(|calls| calls.set(calls.get() + 1));
    }
}

/// # Safety
/// `ctx`/`rdram` must be valid per every other shim's contract in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYielded_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };

    if header.task_type == M_AUDTASK {
        unsafe { dispatch_audio_task(rdram, o, &header) };
    } else if header.task_type == M_GFXTASK {
        unsafe { dispatch_gfx_task(rdram, o, &header) };
    }

    with_executor(|exec| exec.submit_task(header));
    // Return 0 = task COMPLETED (did not yield). sptaskyielded.c returns
    // OS_TASK_YIELDED (==1, sptask.h:20) only if the SP status has
    // SP_STATUS_YIELDED set. This crate dispatches synchronously and runs the
    // task to completion (never actually yields the SP), so the honest return
    // is 0. Returning 1 would tell Sched_HandleReply (sched.c:577) the task
    // yielded, re-queuing an already-finished task to the gfx list head to
    // be re-run forever (funcs_41.c:1587 branches on this $v0).
    ctx.r2 = 0;
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

    /// The single registered audio backend, if the shell/harness has called
    /// `set_audio_backend`. Finished samples enter it at
    /// `osAiSetNextBuffer_recomp`, the real AI DMA boundary.
    pub(crate) static AUDIO_BACKEND: RefCell<Option<Box<dyn AudioBackend>>> = const { RefCell::new(None) };
    /// The rdram buffer length the registered audio backend should treat
    /// as valid, set once by `set_audio_backend`'s caller. Mirrors
    /// `RDRAM_LEN`'s role for the render seam.
    pub(crate) static AUDIO_RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
    /// The most recent `AudioBackend::queue_samples` error, if any,
    /// stringified. Mirrors `RENDER_LAST_ERROR`.
    pub(crate) static AUDIO_LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };

    /// Perf profiling: when true (env `OOT_AUDIO_UCODE_TIMING`), the M_AUDTASK
    /// dispatch times each recompiled-ucode call and accumulates total ns +
    /// call count for a caller to read via `audio_ucode_timing()`.
    pub(crate) static AUDIO_UCODE_TIMING: Cell<bool> =
        Cell::new(std::env::var_os("OOT_AUDIO_UCODE_TIMING").is_some());
    pub(crate) static AUDIO_UCODE_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_UCODE_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_TASK_DUMPED: Cell<bool> = const { Cell::new(false) };
    pub(crate) static AUDIO_PCM_DUMPED: Cell<bool> = const { Cell::new(false) };
    pub(crate) static AUDIO_OUTPUT_STATS: Cell<AudioOutputStats> = const { Cell::new(AudioOutputStats::new()) };

    /// Coarse wall-time attribution for the native OoT performance harness.
    /// Kept behind an environment flag so ordinary execution pays no
    /// `Instant::now` cost at task or executor boundaries.
    pub(crate) static PHASE_TIMING: Cell<bool> =
        Cell::new(std::env::var_os("OOT_PHASE_TIMING").is_some());
    pub(crate) static EXECUTOR_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static EXECUTOR_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_DISPATCH_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_DISPATCH_CALLS: Cell<u64> = const { Cell::new(0) };
}

/// Aggregate evidence from real `osAiSetNextBuffer` submissions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioOutputStats {
    pub ai_buffers: u64,
    pub backend_buffers: u64,
    pub samples: u64,
    pub nonzero_samples: u64,
    pub min: Option<i16>,
    pub max: Option<i16>,
}

impl AudioOutputStats {
    pub(crate) const fn new() -> Self {
        Self {
            ai_buffers: 0,
            backend_buffers: 0,
            samples: 0,
            nonzero_samples: 0,
            min: None,
            max: None,
        }
    }
}

pub fn audio_output_stats() -> AudioOutputStats {
    AUDIO_OUTPUT_STATS.with(Cell::get)
}

/// Coarse host wall-time totals collected when `OOT_PHASE_TIMING` is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTiming {
    pub executor_ns: u64,
    pub executor_calls: u64,
    pub gfx_ns: u64,
    pub gfx_calls: u64,
    pub audio_dispatch_ns: u64,
    pub audio_dispatch_calls: u64,
}

pub fn phase_timing() -> PhaseTiming {
    PhaseTiming {
        executor_ns: EXECUTOR_NS.with(Cell::get),
        executor_calls: EXECUTOR_CALLS.with(Cell::get),
        gfx_ns: GFX_NS.with(Cell::get),
        gfx_calls: GFX_CALLS.with(Cell::get),
        audio_dispatch_ns: AUDIO_DISPATCH_NS.with(Cell::get),
        audio_dispatch_calls: AUDIO_DISPATCH_CALLS.with(Cell::get),
    }
}

/// Total accumulated recompiled-audio-ucode time (ns) and call count since
/// boot, for perf profiling. Only nonzero when `OOT_AUDIO_UCODE_TIMING` is set.
pub fn audio_ucode_timing() -> (u64, u64) {
    (
        AUDIO_UCODE_NS.with(|c| c.get()),
        AUDIO_UCODE_CALLS.with(|c| c.get()),
    )
}

/// Register the audio backend `osAiSetNextBuffer_recomp` delivers finished AI
/// PCM through, and the rdram buffer length it may safely read. This covers
/// sample delivery, not ucode execution.
pub fn set_audio_backend(backend: Box<dyn AudioBackend>, rdram_len: usize) {
    AUDIO_BACKEND.with(|cell| cell.replace(Some(backend)));
    set_audio_rdram_len(rdram_len);
}

/// Register the shared RDRAM bound for AI-buffer validation and live PCM
/// evidence even when no host output device is available.
pub fn set_audio_rdram_len(rdram_len: usize) {
    AUDIO_RDRAM_LEN.with(|cell| cell.set(rdram_len));
}

/// The most recent registered audio backend's `queue_samples` error from an AI
/// buffer submission. `None` if no AI buffer has been delivered yet, the last
/// one succeeded, or no backend is registered. Mirrors `last_render_error`.
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

/// Real translated audio-ucode function signature. Matches RSPRecomp's
/// generated `RspExitReason <name>(uint8_t* rdram, uint32_t)` shape, but the
/// second `u32` carries the **OSTask rdram offset** (`osSpTaskYielded_recomp`
/// passes `o`), not the ucode-text address: a recompiled ucode bakes its own
/// IMEM text in and instead needs the task structure to seed its RSP DMEM
/// (rspboot loads the 64-byte OSTask into DMEM 0xFC0; the audio ucode reads
/// `ucode_data`@0x18 from there). `RspExitReason` is an RSPRecomp-defined enum
/// this crate does not interpret beyond "it ran" -- a plain `u32` return.
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
/// the pair `osSpTaskLoad_recomp` above bookkeeps. `a0` = `ctx->r4` is the
/// `OSTask*` (same pointer shape `osSpTaskLoad`/`osSpTaskYielded` read).
///
/// This crate's dispatch model runs a task's real effect (audio ucode
/// call / gfx backend dispatch) synchronously at `osSpTaskYielded_recomp`,
/// so there is deliberately no double-dispatch here. What a real
/// `osSpTaskStartGo` DOES have that this stub was missing: kicking the RSP
/// eventually raises the SP-done interrupt (and, for a task that drives the
/// RDP to a `DPFullSync`, the DP-done interrupt), which libultra delivers
/// as `OS_EVENT_SP` (=4) / `OS_EVENT_DP` (=9) to whatever queue the game
/// registered via `osSetEventMesg`.
///
/// OoT's Scheduler registers exactly those (`sched.c:704-705`:
/// `osSetEventMesg(OS_EVENT_SP, &sc->interruptQueue, RSP_DONE_MSG=667)` and
/// `osSetEventMesg(OS_EVENT_DP, &sc->interruptQueue, RDP_DONE_MSG=668)`),
/// kicks the task here from `Sched_RunTask` (`sched.c:459`), and its
/// `Sched_ThreadEntry` loop (`sched.c:648`) blocks on `interruptQueue`
/// waiting for those done-messages. Without them the scheduler thread never
/// wakes, so `Sched_TaskComplete` (`sched.c:393`) never posts to the gfx
/// task's `msgQueue` (= `gfxCtx->queue`), so `Graph_ExecuteAndDraw`'s
/// `osRecvMesg(&gfxCtx->queue, ...)` (`graph.c:234`) blocks forever and
/// `osViSwapBuffer` (`graph.c:76/78`, via `Sched_SwapFrameBuffer`) is never
/// reached. Injecting the completion event(s) here closes that gap.
///
/// We inject SP-done for every task (any RSP task raises it), and DP-done
/// additionally for a graphics task (`M_GFXTASK`) -- OoT's gfx task sets
/// `OS_SC_NEEDS_RDP` (`graph.c:309`) and its scheduler blocks on BOTH
/// `Sched_TaskComplete`'s `!(state & (OS_SC_DP | OS_SC_SP))` (`sched.c:397`)
/// before posting the wake. Both events are guarded by
/// `event_table_contains` so a task submitted before the game registered
/// the event (or a game/test that never registers it) is a silent skip, not
/// a panic -- matching `osContStartQuery_recomp`'s `OS_EVENT_SI` guard.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskStartGo_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    const OS_EVENT_SP: u32 = 4; // ultra64/message.h: SP task-done interrupt
    const OS_EVENT_DP: u32 = 9; // ultra64/message.h: DP full-sync interrupt

    let ctx = unsafe { &*ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };
    let is_gfx = header.task_type == M_GFXTASK;

    // Kicking the RSP IS where the task runs in this synchronous model, so the
    // task's real effect happens here -- this is the path OoT uses (Load then
    // StartGo, never the yield path) for BOTH its gfx and its audio tasks.
    // Dispatch before injecting the completion events so the work is done by the
    // time the scheduler is woken. A graphics task rasterizes; an audio task
    // runs its registered ucode + forwards samples (previously dispatched only
    // from the never-taken yield path -- same latent bug the gfx path hit).
    if is_gfx {
        unsafe { dispatch_gfx_task(rdram, o, &header) };
    } else if header.task_type == M_AUDTASK {
        unsafe { dispatch_audio_task(rdram, o, &header) };
    }

    with_executor(|exec| {
        if exec.event_table_contains(OS_EVENT_SP) {
            exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_SP));
        }
        if is_gfx && exec.event_table_contains(OS_EVENT_DP) {
            exec.inject_event(ExternalEvent::OsEvent(OS_EVENT_DP));
        }
    });
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use fn64_runtime::RecvMesgOutcome;

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

        assert_eq!(
            ctx.r2, 0,
            "task reported complete (0), not OS_TASK_YIELDED (1)"
        );
        with_executor(|exec| {
            assert_eq!(exec.task_log().gfx_count(), 1);
            assert_eq!(exec.task_log().audio_count(), 0);
        });
    }

    /// Regression for the "gfx task submitted, framebuffer never swaps"
    /// deadlock: `osSpTaskStartGo_recomp` MUST post the SP-done (and, for a
    /// graphics task, the DP-done) completion event to whatever queue the
    /// game registered via `osSetEventMesg`, mirroring OoT's Scheduler
    /// (`sched.c:704-705`: `osSetEventMesg(OS_EVENT_SP, &interruptQueue,
    /// RSP_DONE_MSG=667)` / `osSetEventMesg(OS_EVENT_DP, ..., RDP_DONE_MSG=
    /// 668)`). Without these, `Sched_ThreadEntry`'s `osRecvMesg` on
    /// `interruptQueue` (`sched.c:656`) never wakes, `Sched_TaskComplete`
    /// (`sched.c:393`) never posts to `gfxCtx->queue`, and
    /// `Graph_ExecuteAndDraw`'s `osRecvMesg` (`graph.c:234`) blocks forever
    /// -> `osViSwapBuffer` is never reached (observed as `vi_swaps=0` in
    /// `examples/oot-boot`).
    ///
    /// The prior stub was an empty `{}`, so reintroducing it (delete the
    /// two `inject_event` calls) makes both `recv_mesg` asserts below fail
    /// with `WouldBlock` -- verified by hand before committing, not a
    /// green-against-the-bug check.
    #[test]
    fn os_sp_task_start_go_posts_sp_and_dp_completion_to_registered_queue() {
        // OoT's real event->message mapping (sched.c).
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;

        // A distinct queue address so this test can't collide with the
        // shared thread-local executor's other queues (same isolation
        // rationale as the rung tests' hand-picked addresses).
        let interrupt_q = RdramAddr::from_offset(0x0009_0000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        // A graphics task header (M_GFXTASK at +0x0), read from ctx.r4 the
        // same way the real `Sched_RunTask` call site passes `&spTask->list`.
        let mut rdram = vec![0u8; 128];
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        // Both completion messages must now be sitting in the registered
        // queue, in SP-then-DP order (RSP finishes before RDP). A dummy
        // receiver id (99) drains them non-blocking; no thread was blocked,
        // so delivery comes straight from the ring buffer.
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG),
                "osSpTaskStartGo must post OS_EVENT_SP -> RSP_DONE_MSG"
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RDP_DONE_MSG),
                "a graphics task's osSpTaskStartGo must ALSO post OS_EVENT_DP -> RDP_DONE_MSG"
            );
            // Nothing else was posted.
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "exactly two completion messages, no more"
            );
        });
    }

    /// A NON-graphics RSP task (e.g. `M_AUDTASK`) posts ONLY the SP-done
    /// event, never DP-done -- OoT's audio task doesn't set OS_SC_NEEDS_RDP,
    /// so injecting a spurious RDP_DONE_MSG would desync the scheduler's
    /// `curRDPTask` bookkeeping. Reintroducing the `is_gfx` gate as an
    /// unconditional DP inject makes the final `WouldBlock` assert fail.
    #[test]
    fn os_sp_task_start_go_audio_task_posts_only_sp() {
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;

        let interrupt_q = RdramAddr::from_offset(0x0009_1000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        let mut rdram = vec![0u8; 128];
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG),
                "an audio task's osSpTaskStartGo posts OS_EVENT_SP"
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "a non-graphics task must NOT post OS_EVENT_DP"
            );
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

        assert_eq!(
            ctx.r2, 0,
            "task reported complete (0), not OS_TASK_YIELDED (1)"
        );
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
        direct.process_task(&mut rdram, &task, 0).unwrap();
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

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_UCODE_ADDR.store(task_offset, Ordering::SeqCst);
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
        // The second arg is the OSTask's rdram OFFSET (see `AudioUcodeFn`'s
        // doc comment: a recompiled ucode bakes its IMEM text in and needs the
        // task structure, not the ucode-text address). Here that's
        // `header_off` (0x20), the offset of the OSTask within `rdram`.
        assert_eq!(SEEN_UCODE_ADDR.load(Ordering::SeqCst), header_off as u32);
        with_executor(|exec| assert!(exec.task_log().audio_count() >= 1));
    }

    /// Fail-against-bug: OoT's audio driver submits its `M_AUDTASK` via the
    /// Load+StartGo path (`AudioMgr_HandleRetrace` -> scheduler ->
    /// `Sched_RunTask` -> `osSpTaskLoad`+`osSpTaskStartGo`), NEVER the yield
    /// path. Before the fix, `osSpTaskStartGo_recomp` dispatched only
    /// `M_GFXTASK`, so a real audio task kicked here never ran its ucode -- the
    /// recompiled aspMain would never execute and no samples would be produced,
    /// even once the audio thread was submitting tasks. This asserts StartGo
    /// really invokes the registered ucode for `M_AUDTASK`, symmetric with the
    /// gfx-from-StartGo fix (commit 73a191a) and the yield-path test above.
    #[test]
    fn os_sp_task_start_go_calls_the_registered_audio_ucode_fn_for_real() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        static SEEN_OFFSET: AtomicU32 = AtomicU32::new(0);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_OFFSET.store(task_offset, Ordering::SeqCst);
            0
        }
        unsafe { set_audio_ucode_fn(fake_ucode) };

        let mut rdram = vec![0u8; 128];
        let header_off = 0x30usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(
            CALLED.load(Ordering::SeqCst),
            "osSpTaskStartGo must call the real ucode fn for an M_AUDTASK (the OoT path)"
        );
        assert_eq!(
            SEEN_OFFSET.load(Ordering::SeqCst),
            header_off as u32,
            "ucode receives the OSTask rdram offset, same contract as the yield path"
        );
    }

    /// osSpTaskYielded, in this crate's synchronous run-to-completion model,
    /// must report task COMPLETED (0), not OS_TASK_YIELDED (1). Returning 1
    /// makes the scheduler re-queue an already-finished task forever. Fails
    /// against the bug (`ctx.r2 = 1`).
    #[test]
    fn os_sp_task_yielded_reports_completed_not_yielded() {
        // Minimal OSTask header at rdram offset 0x40, task_type = 0 (unknown:
        // recorded but no backend/ucode fired). Buffer covers base+0x38.
        let mut rdram = vec![0u8; 256];
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0040; // KSEG0 -> offset 0x40
        ctx.r2 = 0xFFFF_FFFF; // stale $v0.
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            ctx.r2, 0,
            "0 = completed (did not yield); 1 = OS_TASK_YIELDED"
        );
    }
}
