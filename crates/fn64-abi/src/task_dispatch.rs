use super::*;

/// The `OOT_*` spellings these knobs shipped under before they were renamed to
/// the game-agnostic `FN64_*` prefix. Nothing about dumping PCM or skipping
/// audio ucode is OoT-specific; the names were bring-up residue.
///
/// An unset env var means "feature off", so a bare rename would make a stale
/// `OOT_SKIP_AUDIO_UCODE=1` invocation silently do nothing -- the run would
/// look fine and quietly measure the wrong thing. Trap it instead.
const RENAMED_ENV_VARS: &[(&str, &str)] = &[
    ("OOT_SKIP_AUDIO_UCODE", "FN64_SKIP_AUDIO_UCODE"),
    ("OOT_DUMP_AUDIO_PCM", "FN64_DUMP_AUDIO_PCM"),
    ("OOT_DUMP_AUDIO_TASK", "FN64_DUMP_AUDIO_TASK"),
    ("OOT_AUDIO_UCODE_TIMING", "FN64_AUDIO_UCODE_TIMING"),
    ("OOT_PHASE_TIMING", "FN64_PHASE_TIMING"),
];

/// Panic if any pre-rename `OOT_*` knob is still set, naming its replacement.
///
/// Called from the audio task/AI-buffer seams rather than a `thread_local!`
/// initializer: an initializer only runs when its flag is first read, so a
/// stale `OOT_DUMP_AUDIO_PCM` -- whose new name is never consulted on a run
/// that sets only the old one -- would never trip the check.
pub(crate) fn assert_no_legacy_env_vars() {
    for (old, new) in RENAMED_ENV_VARS {
        if std::env::var_os(old).is_some() {
            panic!("{}", legacy_env_var_message(old, new));
        }
    }
}

/// The trap's message, split out so a test can assert the wording without
/// mutating the shared test process's environment -- doing that trips this
/// very trap inside every sibling test that dispatches an audio task.
fn legacy_env_var_message(old: &str, new: &str) -> String {
    format!(
        "{old} was renamed to {new}; the old name is ignored, so this run \
         would silently not do what you asked. Set {new} instead."
    )
}

const AUDIO_STREAM_DUMP_SECONDS: u64 = 12;

struct AudioStreamDump {
    file: std::fs::File,
    path: std::path::PathBuf,
    sample_rate_hz: u32,
    samples_written: u64,
    buffers_written: u64,
}

#[derive(Copy, Clone, Debug, Default)]
struct AudioTaskDumpState {
    seen: u64,
    dumped: bool,
}

fn dump_audio_pcm_stream(samples: &[i16]) {
    use std::io::Write as _;

    let Some(path) = std::env::var_os("FN64_DUMP_AUDIO_STREAM_PCM") else {
        return;
    };
    AUDIO_PCM_STREAM_DUMP.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.is_none() {
            let path = std::path::PathBuf::from(path);
            match std::fs::File::create(&path) {
                Ok(file) => {
                    let sample_rate_hz = AUDIO_GUEST_RATE.with(Cell::get);
                    eprintln!(
                        "[fn64-abi] capturing up to {AUDIO_STREAM_DUMP_SECONDS}s of pre-resample stereo PCM at {sample_rate_hz} Hz to {path:?}"
                    );
                    *state = Some(AudioStreamDump {
                        file,
                        path,
                        sample_rate_hz,
                        samples_written: 0,
                        buffers_written: 0,
                    });
                }
                Err(error) => {
                    eprintln!("[fn64-abi] failed to create streaming PCM dump: {error}");
                    return;
                }
            }
        }

        let dump = state.as_mut().expect("stream dump initialized above");
        let max_samples = u64::from(dump.sample_rate_hz)
            .saturating_mul(2)
            .saturating_mul(AUDIO_STREAM_DUMP_SECONDS);
        let remaining = max_samples.saturating_sub(dump.samples_written);
        let take = usize::try_from(remaining.min(samples.len() as u64)).unwrap_or(samples.len());
        if take == 0 {
            return;
        }
        let bytes: Vec<u8> = samples[..take]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        if let Err(error) = dump.file.write_all(&bytes) {
            eprintln!("[fn64-abi] failed to append streaming PCM dump: {error}");
            return;
        }
        dump.samples_written += take as u64;
        dump.buffers_written += 1;
        let meta = format!(
            "format=s16le\nchannels=2\nsample_rate_hz={}\nsamples={}\nframes={}\nbuffers={}\nseconds={:.6}\n",
            dump.sample_rate_hz,
            dump.samples_written,
            dump.samples_written / 2,
            dump.buffers_written,
            dump.samples_written as f64 / 2.0 / f64::from(dump.sample_rate_hz),
        );
        if let Err(error) = std::fs::write(dump.path.with_extension("meta"), meta) {
            eprintln!("[fn64-abi] failed to update streaming PCM metadata: {error}");
        }
        if dump.samples_written == max_samples {
            eprintln!(
                "[fn64-abi] completed {AUDIO_STREAM_DUMP_SECONDS}s pre-resample PCM capture at {:?}",
                dump.path
            );
        }
    });
}

/// Decode fn64's native-word RDRAM representation in guest halfword order and
/// deliver one real AI DMA buffer to the registered backend.
///
/// # Safety
///
/// `rdram` must be valid for the length registered through
/// [`set_audio_rdram_len`].
pub(crate) unsafe fn deliver_ai_buffer(rdram: *mut u8, start: usize, byte_len: usize) {
    assert_no_legacy_env_vars();
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

    let bytes = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
    let view = fn64_runtime::RdramView::from_storage(bytes);
    let start_addr =
        RdramAddr::from_offset(u32::try_from(start).expect("AI PCM RDRAM start exceeds u32"));
    let samples: Vec<i16> = (0..byte_len)
        .step_by(2)
        .map(|guest_offset| {
            view.read_i16(
                start_addr
                    .checked_add(
                        u32::try_from(guest_offset).expect("AI PCM buffer length exceeds u32"),
                    )
                    .expect("AI PCM logical address overflow"),
            )
        })
        .collect();

    let nonzero = samples.iter().filter(|&&sample| sample != 0).count() as u64;
    let buffer_min = samples.iter().copied().min();
    let buffer_max = samples.iter().copied().max();
    let ai_index = AUDIO_OUTPUT_STATS.with(|cell| {
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
        let ai_index = stats.ai_buffers;
        cell.set(stats);
        ai_index
    });

    if std::env::var_os("FN64_TRACE_AI_BUFFERS").is_some() {
        eprintln!(
            "[fn64-abi] ai_buffer #{ai_index}: start={start:#x} bytes={byte_len:#x} samples={} nonzero={nonzero} range={}..={}",
            samples.len(),
            buffer_min.unwrap_or(0),
            buffer_max.unwrap_or(0),
        );
    }

    // One-shot evidence hook: write the first non-silent live AI buffer as
    // signed 16-bit little-endian PCM, plus self-describing metadata. Waiting
    // for nonzero avoids capturing an expected startup-silence buffer and
    // falsely concluding the synth stayed silent.
    if nonzero != 0 {
        if let Some(path) = std::env::var_os("FN64_DUMP_AUDIO_PCM") {
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

    // Opt-in bounded evidence across AI-buffer seams. Unlike the one-shot
    // hook above, this preserves startup silence and joins enough consecutive
    // DMAs to expose periodic clicks/buzz before host resampling.
    dump_audio_pcm_stream(&samples);

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

/// Run one renderer operation through the process's single registered backend.
/// Missing registration and named backend errors are one loud failure class;
/// no caller may independently turn either into a successful task completion.
fn with_render_backend<T>(
    context: &'static str,
    operation: impl FnOnce(&mut dyn RenderBackend) -> Result<T, fn64_render::RenderError>,
) -> T {
    RENDER_BACKEND.with(|cell| {
        let mut registered = cell.borrow_mut();
        let backend = registered
            .as_mut()
            .unwrap_or_else(|| panic!("{context}: no render backend registered"));
        match operation(backend.as_mut()) {
            Ok(value) => {
                RENDER_LAST_ERROR.with(|last| last.replace(None));
                value
            }
            Err(error) => {
                let reason = error.to_string();
                RENDER_LAST_ERROR.with(|last| last.replace(Some(reason.clone())));
                panic!("{context}: {reason}");
            }
        }
    })
}

/// Physical color buffer selected by the VI manager at this renderer
/// boundary. Raw DPC submissions need the same explicit value as HLE tasks;
/// making a backend infer it from prior calls breaks CPU-only RDP streams and
/// tasks rejected to LLE before any HLE work is committed.
fn render_output_addr() -> u32 {
    crate::vi::next_vi_framebuffer()
        .or_else(current_vi_framebuffer)
        .unwrap_or(0)
}

/// Dispatch a graphics task (`M_GFXTASK`) to the registered `dyn
/// RenderBackend`, once, at the point the RSP is actually kicked by
/// `osSpTaskStartGo_recomp`. A prior version dispatched only from
/// `osSpTaskYielded_recomp`, so a normal Load+StartGo task never reached the
/// backend and a yielded query could re-run completed work. The caller guards
/// on `header.task_type == M_GFXTASK` and passes the task header's RDRAM offset.
///
/// Missing backends and backend errors are loud named failures. Completing an
/// SP task after either condition would wake the scheduler while fabricating a
/// frame that was never processed.
///
/// # Safety
/// `rdram` valid for the call; `o` a valid task-header offset within it.
unsafe fn dispatch_gfx_task(rdram: *mut u8, header: &OsTaskHeader) -> fn64_render::FrameStatus {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    let status = with_render_backend("dispatch_gfx_task", |backend| {
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
            output_buff_size: header.output_buff_size,
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
        let output_addr = render_output_addr();
        with_host(|host| {
            backend.process_task(
                rdram_slice,
                host.device_fabric.rsp_memory_mut(),
                &task,
                output_addr,
            )
        })
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
    status
}

/// Present the registered graphics backend at the guest's real VI swap
/// boundary. Task submission and VI presentation are distinct on N64; this
/// closes the second half of `RenderBackend` without exposing RT64 or any
/// foreign type outside `fn64-render-rt64`.
pub(crate) fn present_render_backend(vi: fn64_render::ViPresentation) {
    with_render_backend("present_render_backend", |backend| backend.present(vi));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LleTaskResult {
    steps: u64,
    needs_dp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HleBootResult {
    steps: u64,
    task: OsTaskHeader,
}

/// Publish the RSP core's guest-visible rdram effects after a task/boot run.
///
/// The RSP machine now executes directly against the real guest RDRAM slice
/// (its ONLY rdram store path is `dma_write`, which logs every written span),
/// so the bytes are already in place -- what remains is the recompiler-side
/// bookkeeping a host-side write owes: notify each written range and re-check
/// live executable pages. A prior version snapshotted the FULL host RDRAM
/// mapping and memcmp-diffed all of it per task; on the wm2000 harness that
/// mapping is 656 MiB (`RDRAM_MMIO_WINDOW_END`), which capped the whole boot
/// at ~5 RSP tasks/second of pure memcpy+memcmp.
fn commit_rsp_rdram_writes(written: &[(usize, usize)]) {
    if written.is_empty() {
        return;
    }
    #[cfg(feature = "recomp-rs")]
    {
        for &(start, end) in written {
            fn64_recomp_rs::notify_guest_write(start as u32, (end - start) as u32);
        }
        crate::recompiled::process_live_executable_writes_from_host();
    }
}

/// Write the forensic capture for a runaway LLE task (see
/// `FN64_RSP_LLE_DEBUG_DIR` in `dispatch_lle_task`): the admitted DMEM/IMEM
/// images, the current DMEM/IMEM images, the scalar machine state, a ring of
/// the most recent PCs, the parsed OSTask header, and a logical-byte-order
/// window of the task's rdram data area.
#[allow(clippy::too_many_arguments)]
fn dump_lle_debug_state(
    dir: &std::path::Path,
    initial_dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    initial_imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    initial_pc: u32,
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    machine: &fn64_audio::rsp::runtime::RspMachine<'_>,
    abort_pc: u32,
    total_steps: u64,
    overlays: u64,
    pc_ring: &std::collections::VecDeque<u32>,
) {
    use std::fmt::Write as _;
    if let Err(error) = std::fs::create_dir_all(dir) {
        eprintln!("[fn64-abi] LLE debug dump: cannot create {dir:?}: {error}");
        return;
    }
    let write = |name: &str, bytes: &[u8]| {
        if let Err(error) = std::fs::write(dir.join(name), bytes) {
            eprintln!("[fn64-abi] LLE debug dump: cannot write {name}: {error}");
        }
    };
    write("initial_dmem.bin", initial_dmem);
    write("initial_imem.bin", initial_imem);
    write("final_dmem.bin", &machine.dmem_logical());
    write("final_imem.bin", imem);

    let mut state = String::new();
    let _ = writeln!(state, "abort_pc {abort_pc:#06x}");
    let _ = writeln!(state, "initial_pc {initial_pc:#06x}");
    let _ = writeln!(state, "total_steps {total_steps}");
    let _ = writeln!(state, "overlays {overlays}");
    let _ = writeln!(state, "sp_status {:#010x}", machine.sp_status());
    let _ = writeln!(state, "sp_semaphore {}", machine.sp_semaphore_latch());
    let _ = writeln!(state, "dma_mem_address {:#010x}", machine.ctx.dma_mem_address);
    let _ = writeln!(
        state,
        "dma_dram_address {:#010x}",
        machine.ctx.dma_dram_address
    );
    for reg in 0..32u8 {
        let _ = writeln!(state, "r{reg} {:#010x}", machine.reg(reg));
    }
    write("state.txt", state.as_bytes());

    let mut ring = String::new();
    for pc in pc_ring {
        let _ = writeln!(ring, "{pc:#06x}");
    }
    write("pc_ring.txt", ring.as_bytes());

    // OSTask header (rspboot leaves it at DMEM 0xFC0; public libultra layout).
    let field = |image: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE], offset: usize| {
        u32::from_be_bytes(
            image[0xfc0 + offset..0xfc0 + offset + 4]
                .try_into()
                .expect("four OSTask bytes"),
        )
    };
    let mut header = String::new();
    for (name, offset) in [
        ("type", 0x00),
        ("flags", 0x04),
        ("ucode_boot", 0x08),
        ("ucode_boot_size", 0x0c),
        ("ucode", 0x10),
        ("ucode_size", 0x14),
        ("ucode_data", 0x18),
        ("ucode_data_size", 0x1c),
        ("dram_stack", 0x20),
        ("dram_stack_size", 0x24),
        ("output_buff", 0x28),
        ("output_buff_size", 0x2c),
        ("data_ptr", 0x30),
        ("data_size", 0x34),
        ("yield_data_ptr", 0x38),
        ("yield_data_size", 0x3c),
    ] {
        let _ = writeln!(header, "{name} {:#010x}", field(initial_dmem, offset));
    }
    write("task_header.txt", header.as_bytes());

    // The task's rdram data area, exported in logical (big-endian guest) byte
    // order: fn64's rdram backing stores each 32-bit word natively, so the
    // logical byte at `offset` lives at `offset ^ 3`.
    let data_ptr = (field(initial_dmem, 0x30) as usize) & 0x00ff_ffff;
    let data_size = (field(initial_dmem, 0x34) as usize).clamp(0x40, 0x20000);
    let end = (data_ptr + data_size).min(machine.rdram.len());
    if data_ptr < end {
        let logical: Vec<u8> = (data_ptr..end)
            .map(|offset| machine.rdram.get(offset ^ 3).copied().unwrap_or(0))
            .collect();
        write("task_data_logical.bin", &logical);
    }
}

fn commit_rsp_memory_state(
    dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    overlays: u64,
    pc: u32,
    status: u32,
) {
    with_host(|host| {
        let memory = host.device_fabric.rsp_memory_mut();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
                dmem,
            )
            .expect("RSP DMEM commit failed");
        for _ in 0..overlays {
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    imem,
                )
                .expect("RSP IMEM generation commit failed");
        }
        host.device_fabric.commit_rsp_execution_state(pc, status);
    });
}

/// Run the persistent RSP state from its admitted PC through BREAK, resolving
/// every IMEM overlay generation and forwarding DPC work to the renderer.
/// This is the universal clean-room path for custom/unknown task types.
unsafe fn dispatch_lle_task(rdram: *mut u8) -> LleTaskResult {
    const CHUNK_STEPS: u64 = 1 << 20;
    const MAX_TASK_STEPS: u64 = 1 << 26;

    let (mut dmem, mut imem, status, mut pc, rdram_len) = with_host(|host| {
        let fabric = &host.device_fabric;
        (
            *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
            *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
            fabric.sp_status(),
            fabric.sp_pc(),
            host.runtime_rdram_len,
        )
    });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP LLE task has no registered process RDRAM allocation"
    );
    // The machine executes directly against the guest allocation: bounded DMA
    // through a checked slice, with every written span logged for
    // `commit_rsp_rdram_writes`. No aliasing access happens while the borrow
    // lives -- the loop below touches only the machine, and `with_host` is
    // not re-entered until after `drop(machine)`.
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.load_dmem_logical(&dmem);
    machine.set_sp_status_raw(
        status & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
    );
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    // Env-gated forensic capture (`FN64_RSP_LLE_DEBUG_DIR=<dir>`): snapshot
    // the admitted task state up front, single-step the final stretch before
    // the admission bound to build a PC ring, and dump everything to files
    // when the bound is exceeded so a runaway loop can be analyzed offline.
    let debug_dir = std::env::var_os("FN64_RSP_LLE_DEBUG_DIR").map(std::path::PathBuf::from);
    const DEBUG_TAIL_STEPS: u64 = 1 << 16;
    const DEBUG_PC_RING: usize = 4096;
    let debug_initial: Option<(
        [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        u32,
    )> = debug_dir.as_ref().map(|_| (dmem, imem, pc));
    let mut debug_pc_ring: std::collections::VecDeque<u32> =
        std::collections::VecDeque::with_capacity(DEBUG_PC_RING);
    loop {
        let words: Vec<u32> = imem
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
            .collect();
        let chunk = if debug_dir.is_some() {
            // Land exactly on the tail boundary, then single-step so the PC
            // ring records every instruction leading up to the bound.
            let tail_start = MAX_TASK_STEPS - DEBUG_TAIL_STEPS;
            if total_steps >= tail_start {
                1
            } else {
                CHUNK_STEPS.min(tail_start - total_steps)
            }
        } else {
            CHUNK_STEPS
        };
        let result = fn64_audio::rsp::run_imem(&words, pc, &mut machine, chunk);
        total_steps = total_steps
            .checked_add(result.steps)
            .expect("RSP task step counter overflow");
        if debug_dir.is_some() && chunk == 1 {
            if debug_pc_ring.len() == DEBUG_PC_RING {
                debug_pc_ring.pop_front();
            }
            debug_pc_ring.push_back(result.pc);
        }
        if total_steps > MAX_TASK_STEPS {
            if let (Some(dir), Some((initial_dmem, initial_imem, initial_pc))) =
                (debug_dir.as_ref(), debug_initial.as_ref())
            {
                dump_lle_debug_state(
                    dir,
                    initial_dmem,
                    initial_imem,
                    *initial_pc,
                    &imem,
                    &machine,
                    result.pc,
                    total_steps,
                    overlays,
                    &debug_pc_ring,
                );
            }
            panic!(
                "RSP task exceeded deterministic {MAX_TASK_STEPS}-instruction admission bound at PC {:#06x}",
                result.pc
            );
        }
        pc = result.pc;
        match result.reason {
            fn64_audio::rsp::RspExitReason::Broke => break,
            fn64_audio::rsp::RspExitReason::SwapOverlay => {
                machine.complete_imem_dma(&mut imem);
                overlays = overlays
                    .checked_add(1)
                    .expect("RSP IMEM overlay generation counter overflow");
            }
            fn64_audio::rsp::RspExitReason::StepLimit => {}
            reason => panic!(
                "RSP LLE task stopped at PC {:#06x} after {total_steps} instructions: {reason:?}",
                result.pc
            ),
        }
    }

    dmem = machine.dmem_logical();
    let final_status = machine.sp_status();
    let dp_submissions = machine.take_dp_submissions();
    let rdram_writes = machine.take_rdram_writes();
    drop(machine);

    commit_rsp_rdram_writes(&rdram_writes);
    commit_rsp_memory_state(&dmem, &imem, overlays, pc, final_status);

    // Consecutive DPC_END extensions are ONE hardware command stream, not
    // independent lists: a 16-byte command (G_TEXRECT, G_FILLRECT with
    // sync, raw triangles) may straddle two END writes -- F3DEX xbus 2.08
    // extends its run 8 bytes at a time -- so per-submission decode would
    // trap on a "truncated" command that hardware simply stalls on until
    // the next END write. Coalesce before dispatch: XBUS runs concatenate
    // their submission-time payload bytes (the DMEM ring is reused across
    // generations); DRAM runs merge address-contiguous ranges.
    let mut index = 0;
    while index < dp_submissions.len() {
        if dp_submissions[index].xbus {
            let mut stream = Vec::new();
            while index < dp_submissions.len() && dp_submissions[index].xbus {
                let submission = &dp_submissions[index];
                assert_eq!(
                    submission.payload.len(),
                    (submission.end.wrapping_sub(submission.start)) as usize,
                    "RSP XBUS DPC range [{:#010x}, {:#010x}) payload was not captured at \
                     submission time",
                    submission.start,
                    submission.end
                );
                stream.extend_from_slice(&submission.payload);
                index += 1;
            }
            unsafe { dispatch_raw_rdp_xbus(rdram, &stream) };
        } else {
            let start = dp_submissions[index].start;
            let mut end = dp_submissions[index].end;
            index += 1;
            while index < dp_submissions.len()
                && !dp_submissions[index].xbus
                && dp_submissions[index].start == end
            {
                end = dp_submissions[index].end;
                index += 1;
            }
            assert!(
                start < end
                    && start.is_multiple_of(8)
                    && end.is_multiple_of(8)
                    && end as usize <= rdram_len,
                "RSP DPC range [{start:#010x}, {end:#010x}) is invalid for RDRAM length \
                 {rdram_len:#x}",
            );
            if std::env::var_os("FN64_XBUS_STREAM_DUMP_DIR").is_some() {
                eprintln!(
                    "[fn64-abi] LLE task dispatching DRAM raw-RDP group [{start:#010x}, \
                     {end:#010x})"
                );
            }
            unsafe { dispatch_raw_rdp(rdram, start, end) };
        }
    }

    LleTaskResult {
        steps: total_steps.max(1),
        needs_dp: !dp_submissions.is_empty(),
    }
}

/// Execute the admitted rspboot until control first reaches bytes loaded by
/// an IMEM DMA, then commit its memory, PC, and SP-status effects before an
/// optimized graphics/audio backend represents the ucode phase.
///
/// The phase boundary comes from the public SGI RSP guide's task protocol:
/// rspboot consumes the task header, loads the selected ucode into IMEM, and
/// starts that ucode. Scalar/VU registers are intentionally local to this
/// optimization boundary; HLE backends consume the public `OSTask` contract,
/// while every guest-observable memory and device effect is committed.
unsafe fn dispatch_hle_rspboot(rdram: *mut u8) -> HleBootResult {
    const BOOT_CHUNK_STEPS: u64 = 1 << 12;
    const MAX_BOOT_STEPS: u64 = 1 << 20;

    let (mut dmem, mut imem, status, mut pc, rdram_len) = with_host(|host| {
        let fabric = &host.device_fabric;
        (
            *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
            *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
            fabric.sp_status(),
            fabric.sp_pc(),
            host.runtime_rdram_len,
        )
    });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP HLE rspboot has no registered process RDRAM allocation"
    );
    // Direct guest-RDRAM execution with span-logged writes; see
    // `dispatch_lle_task`'s matching comment and `commit_rsp_rdram_writes`.
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.load_dmem_logical(&dmem);
    machine.set_sp_status_raw(
        status & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
    );
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    let mut loaded_spans = Vec::new();

    loop {
        let execution_pc = if machine.ctx.resume_address != 0 {
            0x1000 | (machine.ctx.resume_address & 0x0fff)
        } else {
            0x1000 | (pc & 0x0fff)
        };
        if loaded_spans
            .iter()
            .copied()
            .any(|span: fn64_audio::rsp::runtime::ImemDmaSpan| span.contains_pc(execution_pc))
        {
            pc = execution_pc;
            break;
        }

        let words: Vec<u32> = imem
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
            .collect();
        let budget = if loaded_spans.is_empty() {
            BOOT_CHUNK_STEPS
        } else {
            1
        };
        let result = fn64_audio::rsp::run_imem(&words, pc, &mut machine, budget);
        total_steps = total_steps
            .checked_add(result.steps)
            .expect("RSP rspboot step counter overflow");
        assert!(
            total_steps <= MAX_BOOT_STEPS,
            "RSP HLE rspboot exceeded deterministic {MAX_BOOT_STEPS}-instruction bound at PC {:#06x}",
            result.pc
        );
        pc = result.pc;
        match result.reason {
            fn64_audio::rsp::RspExitReason::SwapOverlay => {
                loaded_spans.push(machine.pending_imem_dma_span());
                machine.complete_imem_dma(&mut imem);
                overlays = overlays
                    .checked_add(1)
                    .expect("RSP rspboot IMEM generation counter overflow");
            }
            fn64_audio::rsp::RspExitReason::StepLimit => {}
            fn64_audio::rsp::RspExitReason::Broke => panic!(
                "RSP HLE rspboot reached BREAK before entering DMA-loaded ucode at PC {:#06x}",
                result.pc
            ),
            reason => panic!(
                "RSP HLE rspboot stopped at PC {:#06x} after {total_steps} instructions: {reason:?}",
                result.pc
            ),
        }
    }

    dmem = machine.dmem_logical();
    let task = os_task_header_from_words(|field| {
        let start = 0x0fc0 + field;
        u32::from_be_bytes(
            dmem[start..start + 4]
                .try_into()
                .expect("four OSTask DMEM bytes"),
        )
    });
    let final_status = machine.sp_status();
    let dp_submissions = machine.take_dp_submissions();
    assert!(
        dp_submissions.is_empty(),
        "RSP HLE rspboot submitted {} DPC range(s) before entering ucode",
        dp_submissions.len()
    );
    let rdram_writes = machine.take_rdram_writes();
    drop(machine);

    commit_rsp_rdram_writes(&rdram_writes);
    commit_rsp_memory_state(&dmem, &imem, overlays, pc, final_status);
    HleBootResult {
        steps: total_steps.max(1),
        task,
    }
}

/// Submit one bounded DRAM-backed raw RDP command list to the registered
/// renderer. DPC state is committed by the caller before this runs; missing
/// backends and backend failures trap rather than completing nonexistent work.
pub(crate) unsafe fn dispatch_raw_rdp(rdram: *mut u8, start: u32, end: u32) {
    with_render_backend("dispatch_raw_rdp", |backend| {
        let rdram_len = RDRAM_LEN.with(Cell::get);
        let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
        backend.process_rdp_commands(rdram_slice, start, end, render_output_addr())
    });
}

/// Submit one coalesced XBUS command stream (logical big-endian bytes,
/// captured from DMEM at each DPC_END submission -- see `RspDpSubmission::
/// payload`). The renderer seam accepts an RDRAM image, so the command span
/// is staged after the real allocation in a private image. Only the original
/// RDRAM prefix is copied back after rendering; the staging range is
/// unobservable.
unsafe fn dispatch_raw_rdp_xbus(rdram: *mut u8, stream: &[u8]) {
    assert!(
        !stream.is_empty() && stream.len().is_multiple_of(8),
        "RSP XBUS DPC stream length {:#x} must be nonempty and 8-byte aligned",
        stream.len()
    );
    let rdram_len = RDRAM_LEN.with(Cell::get);
    assert!(
        rdram_len != 0,
        "RSP XBUS DPC submission requires a registered renderer RDRAM length"
    );
    // Observability knob: `FN64_XBUS_STREAM_DUMP_DIR=<dir>` writes each
    // coalesced XBUS command stream (logical big-endian bytes, exactly what
    // the raw-RDP decoder sees) as `<dir>/xbus-NNNN.bin`, capped at 16
    // streams. This is how a stream that traps in the decoder is diagnosed
    // offline instead of by guesswork.
    if let Some(dir) = std::env::var_os("FN64_XBUS_STREAM_DUMP_DIR") {
        thread_local! {
            static XBUS_DUMP_INDEX: Cell<u64> = const { Cell::new(0) };
        }
        let index = XBUS_DUMP_INDEX.with(|cell| {
            let index = cell.get();
            cell.set(index + 1);
            index
        });
        if index < 16 {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir)
                .unwrap_or_else(|error| panic!("FN64_XBUS_STREAM_DUMP_DIR {dir:?}: {error}"));
            let path = dir.join(format!("xbus-{index:04}.bin"));
            std::fs::write(&path, stream)
                .unwrap_or_else(|error| panic!("writing XBUS stream dump {path:?}: {error}"));
            eprintln!(
                "[fn64-abi] dumped XBUS stream #{index} ({} bytes) to {}",
                stream.len(),
                path.display()
            );
        }
    }
    let real = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let staging_start = (rdram_len + 7) & !7;
    let mut image = vec![0u8; staging_start + stream.len()];
    image[..rdram_len].copy_from_slice(real);
    for (word_index, word) in stream.chunks_exact(4).enumerate() {
        let value = u32::from_be_bytes(word.try_into().expect("four stream bytes"));
        let offset = staging_start + word_index * 4;
        image[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    let staged_end = staging_start + stream.len();
    with_render_backend("dispatch_raw_rdp_xbus", |backend| {
        backend
            .process_rdp_commands(
                &mut image,
                staging_start as u32,
                staged_end as u32,
                render_output_addr(),
            )
            .map(|_| ())
    });
    real.copy_from_slice(&image[..rdram_len]);
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
/// This runs only from `osSpTaskStartGo_recomp`, the Load+StartGo path OoT's
/// audio driver uses (`AudioMgr_HandleRetrace` -> scheduler -> `Sched_RunTask`
/// -> `osSpTaskLoad`+`osSpTaskStartGo`). A prior version dispatched the ucode
/// only from the later yield-status query, so a normal task never ran while a
/// query could run it twice.
///
/// # Safety
/// `rdram` valid for the call; `o` a valid task-header offset within it and
/// `header` the task header read from that offset.
unsafe fn dispatch_audio_task(rdram: *mut u8, o: usize, header: &OsTaskHeader) {
    debug_assert_eq!(header.task_type, M_AUDTASK);
    // Before PHASE_TIMING's thread_local initializer reads the NEW name: a run
    // that set only the old spelling must trap here, not silently proceed.
    assert_no_legacy_env_vars();
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    // Debug/perf escape hatch: `FN64_SKIP_AUDIO_UCODE` skips running the
    // recompiled audio ucode (the per-frame RSP synth, currently unoptimized
    // and the dominant per-swap cost). The CPU-side audio driver
    // (AudioThread_UpdateImpl) and this task's completion event still run, so
    // the audio-reset handshake that unblocks Play_Init still completes -- the
    // ucode only produces the actual samples. Use it to iterate on the RENDERER
    // at normal boot speed; a real audio run must NOT set it. No sound when set.
    // Diagnostic: dump the exact rdram + task offset of one audio task so the
    // recompiled ucode can be replayed offline against the real command list.
    // `FN64_DUMP_AUDIO_TASK_INDEX` is one-based and defaults to the first task;
    // use a later task to capture audible music instead of startup silence.
    if let Some(path) = std::env::var_os("FN64_DUMP_AUDIO_TASK") {
        let target_index = std::env::var("FN64_DUMP_AUDIO_TASK_INDEX")
            .ok()
            .map(|raw| {
                raw.parse::<u64>().unwrap_or_else(|_| {
                    panic!("FN64_DUMP_AUDIO_TASK_INDEX must be a positive integer, got {raw:?}")
                })
            })
            .unwrap_or(1);
        assert!(
            target_index != 0,
            "FN64_DUMP_AUDIO_TASK_INDEX is one-based; got 0"
        );
        AUDIO_TASK_DUMP.with(|dump| {
            let mut state = dump.get();
            state.seen = state.seen.saturating_add(1);
            if !state.dumped && state.seen == target_index {
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
                    let meta =
                        format!("task_offset={o}\nrdram_len={rdram_len}\ntask_index={}\n", state.seen);
                    let _ = std::fs::write(p.with_extension("meta"), meta);
                    eprintln!("[fn64-abi] dumped audio task #{} rdram ({rdram_len} B) + task_offset={o} to {path:?}", state.seen);
                }
                state.dumped = true;
            }
            dump.set(state);
        });
    }
    let skip_ucode = std::env::var_os("FN64_SKIP_AUDIO_UCODE").is_some();
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

/// `osSpTaskYielded(OSTask *task) -> OSYieldResult` observes the public RSP
/// task-yield handshake after the SP completion event. SIG1/
/// `SP_STATUS_YIELDED` means microcode saved resumable state in the task's
/// yield buffer. In that case libultra prepares the same task for restart by
/// setting `OS_TASK_YIELDED`, replacing its ucode-data pointer and size with
/// the yield-buffer fields, and returning `OS_TASK_YIELDED`. Otherwise the
/// task completed before honoring SIG0 and this returns zero without changing
/// the task.
///
/// This is an observation/preparation call, never a submission point. Running
/// a graphics backend or audio ucode here would execute work a second time
/// after `osSpTaskStartGo_recomp` already completed it.
///
/// # Safety
/// `ctx`/`rdram` must be valid per every other shim's contract in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYielded_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    if crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED == 0 {
        ctx.r2 = 0;
        return;
    }

    let header = unsafe { read_os_task_header(rdram, o) };
    unsafe {
        write_os_task_word(rdram, o, 0x04, header.flags | fn64_runtime::OS_TASK_YIELDED);
        write_os_task_word(rdram, o, 0x18, header.yield_data_ptr);
        write_os_task_word(rdram, o, 0x1c, header.yield_data_size);
    }
    ctx.r2 = u64::from(fn64_runtime::OS_TASK_YIELDED);
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

    /// Perf profiling: when true (env `FN64_AUDIO_UCODE_TIMING`), the M_AUDTASK
    /// dispatch times each recompiled-ucode call and accumulates total ns +
    /// call count for a caller to read via `audio_ucode_timing()`.
    pub(crate) static AUDIO_UCODE_TIMING: Cell<bool> =
        Cell::new(std::env::var_os("FN64_AUDIO_UCODE_TIMING").is_some());
    pub(crate) static AUDIO_UCODE_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_UCODE_CALLS: Cell<u64> = const { Cell::new(0) };
    static AUDIO_TASK_DUMP: Cell<AudioTaskDumpState> =
        const { Cell::new(AudioTaskDumpState { seen: 0, dumped: false }) };
    pub(crate) static AUDIO_PCM_DUMPED: Cell<bool> = const { Cell::new(false) };
    static AUDIO_PCM_STREAM_DUMP: RefCell<Option<AudioStreamDump>> = const { RefCell::new(None) };
    pub(crate) static AUDIO_OUTPUT_STATS: Cell<AudioOutputStats> = const { Cell::new(AudioOutputStats::new()) };

    /// Coarse wall-time attribution for the rs-lane OoT performance harness.
    /// Kept behind an environment flag so ordinary execution pays no
    /// `Instant::now` cost at task or executor boundaries.
    pub(crate) static PHASE_TIMING: Cell<bool> =
        Cell::new(std::env::var_os("FN64_PHASE_TIMING").is_some());
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

/// Frames currently buffered in the registered backend's output ring, or
/// `None` when no backend is registered (or it reports an error). This is
/// host-delivery telemetry only; the emulated `AI_LEN` register reports the
/// current DMA through `audio_remaining_guest_bytes` instead.
pub fn audio_frames_remaining() -> Option<u32> {
    AUDIO_BACKEND.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|backend| backend.frames_remaining().ok())
    })
}

pub fn audio_output_stats() -> AudioOutputStats {
    AUDIO_OUTPUT_STATS.with(Cell::get)
}

pub fn audio_stream_health() -> Option<fn64_audio::AudioStreamHealth> {
    AUDIO_BACKEND.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|backend| backend.stream_health())
    })
}

pub fn audio_rates() -> Option<(u32, u32)> {
    let guest_rate = AUDIO_GUEST_RATE.with(Cell::get);
    AUDIO_BACKEND.with(|cell| {
        let borrowed = cell.borrow();
        let stream_rate = borrowed.as_ref()?.stream_rate_hz()?;
        Some((guest_rate, stream_rate))
    })
}

/// Coarse host wall-time totals collected when `FN64_PHASE_TIMING` is set.
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
/// boot, for perf profiling. Only nonzero when `FN64_AUDIO_UCODE_TIMING` is set.
pub fn audio_ucode_timing() -> (u64, u64) {
    (
        AUDIO_UCODE_NS.with(|c| c.get()),
        AUDIO_UCODE_CALLS.with(|c| c.get()),
    )
}

/// Forward the game's true AI DAC rate (`osAiSetFrequency`'s successful
/// return value) to the registered backend so its producer-side resample
/// ratio tracks the guest, and remember it for rate telemetry. No-op when no
/// backend is registered.
pub(crate) fn notify_audio_frequency(sample_rate_hz: u32) {
    AUDIO_GUEST_RATE.with(|cell| cell.set(sample_rate_hz));
    AUDIO_BACKEND.with(|cell| {
        if let Some(backend) = cell.borrow_mut().as_mut() {
            backend.set_frequency(sample_rate_hz);
        }
    });
}

thread_local! {
    /// The true AI DAC rate last forwarded by `notify_audio_frequency`
    /// (0 = the game has not set a frequency yet).
    pub(crate) static AUDIO_GUEST_RATE: Cell<u32> = const { Cell::new(0) };
}

/// Bytes remaining in the current emulated AI DMA -- i.e. what hardware's
/// `AI_LEN` register counts down as the DAC drains. Host stream prebuffering
/// is intentionally excluded: exposing the whole cpal ring here made guest
/// buffer sizing depend on host latency instead of the N64 DMA boundary.
pub(crate) fn audio_remaining_guest_bytes() -> Option<u32> {
    AUDIO_BACKEND.with(|cell| {
        let borrowed = cell.borrow();
        let backend = borrowed.as_ref()?;
        backend.current_dma_bytes_remaining()
    })
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

/// Register the graphics backend `osSpTaskStartGo_recomp` dispatches
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

    /// Opt-in: run `M_AUDTASK`s through `dispatch_lle_task` -- the SAME
    /// clean-room RSP interpreter replay every non-catalog gfx task already
    /// takes -- instead of `dispatch_audio_task`'s registered-function path.
    /// This executes the game's real, in-ROM audio ucode instruction by
    /// instruction against guest RDRAM, so the CPU-side sound driver
    /// observes the genuine mixer/sequence state the RSP wrote (a harness
    /// with no linkable translated ucode otherwise runs a stand-in that
    /// writes nothing, starving any driver handshake that reads RSP
    /// output). Off by default: existing tests/hosts that submit synthetic
    /// audio tasks with no real admitted ucode must keep the
    /// count-but-don't-run behavior. Set via `set_audio_task_lle` or env
    /// `FN64_AUDIO_LLE=1`.
    static AUDIO_TASK_LLE: Cell<bool> =
        Cell::new(std::env::var_os("FN64_AUDIO_LLE").is_some());
}

/// Route `M_AUDTASK`s through the clean-room RSP LLE interpreter (see
/// `AUDIO_TASK_LLE`'s doc comment). Host-facing seam, same shape as
/// `set_audio_ucode_fn`.
pub fn set_audio_task_lle(enabled: bool) {
    AUDIO_TASK_LLE.with(|cell| cell.set(enabled));
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
/// `rdram` must be valid for at least `base + 0x40` bytes.
unsafe fn read_os_task_header(rdram: *mut u8, base: usize) -> OsTaskHeader {
    // Native byte order, matching MEM_W's real semantics -- see
    // `read_stack_word`'s doc comment for the full correction this wave made.
    let w = |off: usize| -> u32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(rdram.add(base + off), b.as_mut_ptr(), 4) };
        u32::from_ne_bytes(b)
    };
    os_task_header_from_words(w)
}

fn os_task_header_from_words(mut w: impl FnMut(usize) -> u32) -> OsTaskHeader {
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
        output_buff_size: w(0x2C),
        data_ptr: w(0x30),
        data_size: w(0x34),
        yield_data_ptr: w(0x38),
        yield_data_size: w(0x3C),
    }
}

/// Store one native-word `OSTask_t` field in the same backing layout used by
/// generated `MEM_W` accesses.
///
/// # Safety
/// `rdram` must be valid for `base + field + 4` bytes.
unsafe fn write_os_task_word(rdram: *mut u8, base: usize, field: usize, value: u32) {
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_ne_bytes().as_ptr(), rdram.add(base + field), 4)
    };
}

/// `osSpTaskLoad(OSSpTask *sptask)` -- performs the public RSP guide's
/// CPU-side admission algorithm: with SP halted, copy the complete 64-byte
/// `OSTask` to DMEM `0xfc0`, copy aligned rspboot bytes to IMEM `0`, and set
/// PC to zero. The raw SP DMA registers use timed active/pending slots; this
/// synchronous OS call represents its documented DMA-and-poll loops as
/// complete when it returns. It also records the header through the same task
/// log used by the HLE dispatcher.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskLoad_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &*ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };
    // A newly admitted task must not inherit either half of the preceding
    // task's yield handshake. In particular, stale SIG1 would make the next
    // `osSpTaskYielded` rewrite a task that actually completed normally.
    crate::pi::write_live_sp_status(fn64_runtime::SP_CLR_YIELD | fn64_runtime::SP_CLR_YIELDED);
    unsafe { crate::pi::admit_live_sp_task(rdram, task_addr, header) }
        .unwrap_or_else(|error| panic!("osSpTaskLoad_recomp: {error}"));
    with_executor(|exec| exec.submit_task(header));
}

/// `osSpTaskStartGo(OSSpTask *sptask)` -- the actual RSP-kickoff half of
/// the pair `osSpTaskLoad_recomp` above bookkeeps. `a0` = `ctx->r4` is the
/// `OSTask*` (same pointer shape `osSpTaskLoad`/`osSpTaskYielded` read).
///
/// This crate executes the admitted rspboot through its IMEM-DMA handoff,
/// then runs the selected task's HLE effect (audio ucode call or gfx backend
/// dispatch) synchronously while the shim owns the guest. Its externally
/// visible completion is scheduled separately, with the measured rspboot
/// instruction count included in SP latency. What a real
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
/// reached. Scheduling the completion events in `DeviceFabric` closes that
/// gap without making them visible inside the kickoff call.
///
/// We schedule SP-done for every task, and DP-done
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
    let ctx = unsafe { &*ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    let header = unsafe { read_os_task_header(rdram, o) };
    let is_gfx = header.task_type == M_GFXTASK;

    // Kicking the RSP is where the HLE task effect runs, so the work happens
    // here -- this is the path OoT uses (Load then
    // StartGo, never the yield path) for BOTH its gfx and its audio tasks.
    // Dispatch before scheduling completion so the work is done by the time
    // the scheduler is woken. A graphics task rasterizes; an audio task
    // runs its registered ucode + forwards samples (previously dispatched only
    // from the never-taken yield path -- same latent bug the gfx path hit).
    let boot = if is_gfx || header.task_type == M_AUDTASK {
        let boot = unsafe { dispatch_hle_rspboot(rdram) };
        assert_eq!(
            boot.task.task_type, header.task_type,
            "RSP rspboot changed OSTask type from {} to {}; HLE selection is no longer valid",
            header.task_type, boot.task.task_type
        );
        Some(boot)
    } else {
        None
    };
    let hle_header = boot.map_or(header, |boot| boot.task);
    let needs_dp = if is_gfx {
        let status = unsafe { dispatch_gfx_task(rdram, &hle_header) };
        match status {
            fn64_render::FrameStatus::Complete => true,
            fn64_render::FrameStatus::Yielded => {
                crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
                false
            }
            fn64_render::FrameStatus::NeedsLle { .. } => {
                // The renderer's preflight is transactional, so persistent
                // state is still exactly the post-rspboot ucode entry. Run
                // the complete ucode phase through LLE; attempting a
                // mid-HLE transplant would fabricate scalar/VU registers.
                let lle = unsafe { dispatch_lle_task(rdram) };
                let boot_steps = boot.expect("gfx LLE fallback requires rspboot").steps;
                crate::pi::start_live_rcp_task_with_latency(
                    lle.needs_dp,
                    boot_steps.saturating_add(lle.steps),
                )
                .unwrap_or_else(|error| {
                    panic!("osSpTaskStartGo_recomp gfx LLE completion: {error}")
                });
                return;
            }
        }
    } else if header.task_type == M_AUDTASK {
        if AUDIO_TASK_LLE.with(Cell::get) {
            // Honest fallback parity with gfx: replay the game's real audio
            // ucode through the RSP interpreter (see `AUDIO_TASK_LLE`).
            // rspboot already ran above, so persistent RSP state is at the
            // ucode entry -- exactly the state `dispatch_lle_task` continues.
            let lle = unsafe { dispatch_lle_task(rdram) };
            let boot_steps = boot.expect("audio LLE requires rspboot").steps;
            crate::pi::start_live_rcp_task_with_latency(
                lle.needs_dp,
                boot_steps.saturating_add(lle.steps),
            )
            .unwrap_or_else(|error| {
                panic!("osSpTaskStartGo_recomp audio LLE completion: {error}")
            });
            return;
        }
        unsafe { dispatch_audio_task(rdram, o, &hle_header) };
        false
    } else {
        let lle = unsafe { dispatch_lle_task(rdram) };
        crate::pi::start_live_rcp_task_with_latency(lle.needs_dp, lle.steps)
            .unwrap_or_else(|error| panic!("osSpTaskStartGo_recomp LLE completion: {error}"));
        return;
    };

    let boot_steps = boot.expect("known HLE task must execute rspboot").steps;
    crate::pi::start_live_rcp_task_with_latency(needs_dp, boot_steps.saturating_add(1))
        .unwrap_or_else(|error| panic!("osSpTaskStartGo_recomp: {error}"));
}

/// `osSpTaskYield(void)` -- signals the RSP to yield its current task back
/// to the CPU, returning immediately (asynchronous request, not a
/// blocking wait -- `osSpTaskYielded` is the separate poll/wait-for-
/// completion call, already implemented above). Verified real call site:
/// `funcs_41.c:32`, a bare `jal` with no register setup. This crate's
/// SIG0 is still recorded in the live SP status register even though the
/// current HLE task path executes atomically. That makes raw MMIO, custom LLE
/// microcode, and the OS shim share one observable handshake instead of
/// silently discarding the request. Mid-HLE-task preemption remains a separate
/// scheduler/timing frontier.
///
/// # Safety
/// Same contract as every other shim in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYield_recomp(_rdram: *mut u8, _ctx: *mut RecompContext) {
    crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELD);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use fn64_render::{FrameStatus, RenderConfig, RenderError, UcodeId};
    use fn64_runtime::RecvMesgOutcome;

    struct StatusRenderBackend(FrameStatus);

    impl RenderBackend for StatusRenderBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(self.0)
        }

        fn present(&mut self, _vi: fn64_render::ViPresentation) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
        payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic")
    }

    #[test]
    fn graphics_backend_receives_the_device_fabrics_persistent_rsp_memory() {
        struct RspMemoryBackend;

        impl RenderBackend for RspMemoryBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                rsp_memory
                    .write_bytes(fn64_runtime::RspMemAddr::from_register(0x120), b"rsp-live")
                    .unwrap();
                Ok(FrameStatus::Complete)
            }

            fn present(&mut self, _vi: fn64_render::ViPresentation) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let mut rdram = vec![0u8; 0x1000];
        set_render_backend(Box::new(RspMemoryBackend), rdram.len());
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ..Default::default()
        };
        let status = unsafe { dispatch_gfx_task(rdram.as_mut_ptr(), &header) };
        assert_eq!(status, FrameStatus::Complete);
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_bytes(fn64_runtime::RspMemAddr::from_register(0x120), 8)
                    .unwrap(),
                b"rsp-live"
            );
        });
    }

    /// Install a minimal public-protocol rspboot which DMA-loads eight bytes
    /// at IMEM 0x1080 and jumps there, then admit the task through the real
    /// `osSpTaskLoad` shim. Words use the native backing representation which
    /// `RdramPtr` exposes as guest big-endian logical bytes.
    fn admit_synthetic_hle_task(rdram: &mut Vec<u8>, header_off: usize, ctx: &mut RecompContext) {
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let boot_off = (rdram.len() + 7) & !7;
        let ucode_off = boot_off + 32;
        assert!(ucode_off <= i16::MAX as usize);
        rdram.resize(ucode_off + 8, 0);
        let boot = [
            0x2402_0000 | ucode_off as u32,
            mtc0(2, 1),
            0x2403_1080,
            mtc0(3, 0),
            0x2404_0007,
            mtc0(4, 2),
            0x0800_0020,
            0,
        ];
        for (index, word) in boot.into_iter().enumerate() {
            let offset = boot_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        for (field, value) in [
            (0x08, boot_off as u32),
            (0x0c, 32),
            (0x10, ucode_off as u32),
            (0x14, 8),
        ] {
            rdram[header_off + field..header_off + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), ctx) };
    }

    #[test]
    fn every_renderer_entry_traps_when_no_backend_is_registered() {
        RENDER_BACKEND.with(|cell| cell.replace(None));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<()>("renderer_gate_test", |_| Ok(()));
        }))
        .expect_err("missing renderer must panic");
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        assert!(panic_message(panic.as_ref())
            .contains("renderer_gate_test: no render backend registered"));
    }

    #[test]
    fn every_renderer_entry_traps_and_records_a_backend_error() {
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<()>("renderer_gate_test", |_| {
                Err(RenderError::Backend {
                    backend: "synthetic",
                    reason: "deliberate failure".to_owned(),
                })
            });
        }))
        .expect_err("renderer error must panic");
        assert!(panic_message(panic.as_ref())
            .contains("renderer_gate_test: synthetic backend error: deliberate failure"));
        assert_eq!(
            last_render_error().as_deref(),
            Some("synthetic backend error: deliberate failure")
        );
    }

    /// The rename is only honest if the retired spelling is LOUD: an unset var
    /// means "feature off", so a silently-ignored `OOT_*` name would let a
    /// stale invocation look like a clean run while measuring the wrong thing.
    /// Every retired name must reach a message naming its replacement -- that
    /// string is the whole trap, and a typo'd table entry would gut it.
    #[test]
    fn every_legacy_env_var_names_its_replacement() {
        for (old, new) in RENAMED_ENV_VARS {
            assert!(old.starts_with("OOT_") && new.starts_with("FN64_"));
            let message = legacy_env_var_message(old, new);
            assert!(
                message.contains(new),
                "the trap for {old} must name {new}, or the operator cannot act on it"
            );
        }
    }

    #[test]
    fn os_sp_task_load_admits_complete_header_and_rspboot_to_persistent_rsp_memory() {
        const TASK_OFFSET: usize = 0x100;
        const BOOT_OFFSET: usize = 0x200;
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        let mut rdram = vec![0u8; 0x400];
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            flags: 0x1122_3344,
            ucode_boot: 0x8000_0000 | BOOT_OFFSET as u32,
            ucode_boot_size: 13,
            ucode: 0x3456,
            ucode_size: 0x789A,
            ucode_data: 0xBCDE,
            ucode_data_size: 0x20,
            dram_stack: 0x1234,
            dram_stack_size: 0x40,
            output_buff: 0x5678,
            output_buff_size: 0x9ABC,
            data_ptr: 0xDEF0,
            data_size: 0x80,
            yield_data_ptr: 0x1357,
            yield_data_size: 0x2468,
        };
        let words = [
            header.task_type,
            header.flags,
            header.ucode_boot,
            header.ucode_boot_size,
            header.ucode,
            header.ucode_size,
            header.ucode_data,
            header.ucode_data_size,
            header.dram_stack,
            header.dram_stack_size,
            header.output_buff,
            header.output_buff_size,
            header.data_ptr,
            header.data_size,
            header.yield_data_ptr,
            header.yield_data_size,
        ];
        for (index, word) in words.into_iter().enumerate() {
            let start = TASK_OFFSET + index * 4;
            rdram[start..start + 4].copy_from_slice(&word.to_ne_bytes());
        }
        let boot = (0..16).map(|value| 0xA0 + value).collect::<Vec<u8>>();
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (index, byte) in boot.iter().copied().enumerate() {
                view.write_u8(RdramAddr::from_offset((BOOT_OFFSET + index) as u32), byte);
            }
        }
        let prior_count = with_executor(|exec| exec.task_log().submissions().len());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + TASK_OFFSET as u64;
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };

        with_host(|host| {
            let rsp = host.device_fabric.rsp_memory();
            assert_eq!(
                rsp.read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 16)
                    .unwrap(),
                boot
            );
            let task = rsp
                .read_bytes(fn64_runtime::RspMemAddr::from_register(0x0FC0), 64)
                .unwrap();
            assert_eq!(&task[0..4], &header.task_type.to_be_bytes());
            assert_eq!(&task[8..12], &header.ucode_boot.to_be_bytes());
            assert_eq!(&task[60..64], &header.yield_data_size.to_be_bytes());
            assert_eq!(rsp.imem_generation(), 1);
        });
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A408_0000),
            Some(0)
        );
        with_executor(|exec| {
            assert_eq!(exec.task_log().submissions().len(), prior_count + 1);
            assert_eq!(exec.task_log().submissions().last(), Some(&header));
        });
    }

    #[test]
    fn hle_rspboot_commits_overlay_and_stops_before_executing_loaded_ucode() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off = u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap());
        for (index, word) in [0x2405_5678u32, 0xac05_0100].into_iter().enumerate() {
            let offset = ucode_off as usize + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        let generation_before = with_host(|host| host.device_fabric.rsp_memory().imem_generation());

        let boot = unsafe { dispatch_hle_rspboot(rdram.as_mut_ptr()) };

        assert_eq!(boot.steps, 7);
        assert_eq!(boot.task.task_type, fn64_runtime::M_GFXTASK);
        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(fabric.sp_pc(), 0x80);
            assert_eq!(fabric.rsp_memory().imem_generation(), generation_before + 1);
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0,
                "the first loaded-ucode instruction must remain behind the HLE boundary"
            );
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Imem,
                        0x80,
                    ))
                    .unwrap(),
                0x2405_5678
            );
        });
    }

    #[test]
    fn hle_rspboot_traps_if_boot_breaks_before_loading_ucode() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        with_host(|host| {
            host.device_fabric
                .rsp_memory_mut()
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    0x0000_000d,
                )
                .unwrap();
        });

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_hle_rspboot(rdram.as_mut_ptr())
        }))
        .expect_err("rspboot BREAK before ucode must trap");
        assert!(panic_message(panic.as_ref())
            .contains("RSP HLE rspboot reached BREAK before entering DMA-loaded ucode"));
    }

    #[test]
    fn graphics_hle_fallback_replays_the_untouched_ucode_phase_through_lle() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2405_5678u32, 0xac05_0100].into_iter().enumerate() {
            let offset = ucode_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        with_host(|host| {
            host.device_fabric
                .rsp_memory_mut()
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x88),
                    0x0000_000d,
                )
                .unwrap();
        });
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::NeedsLle {
                ucode_sha256: [0; 32],
            })),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_5678
            );
            assert_eq!(fabric.sp_pc(), 0x88);
            assert!(
                fabric.snapshot().sp_busy,
                "the LLE BREAK schedules externally visible SP completion"
            );
        });
    }

    #[test]
    fn graphics_hle_fallback_forwards_lle_dpc_submissions() {
        use std::cell::RefCell;
        use std::rc::Rc;

        struct LleDpcBackend(Rc<RefCell<Vec<(u32, u32, u32)>>>);

        impl RenderBackend for LleDpcBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::NeedsLle {
                    ucode_sha256: [0; 32],
                })
            }

            fn process_rdp_commands(
                &mut self,
                _rdram: &mut [u8],
                start: u32,
                end: u32,
                output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0.borrow_mut().push((start, end, output_addr));
                Ok(FrameStatus::Complete)
            }

            fn present(&mut self, _vi: fn64_render::ViPresentation) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER: usize = 0x40;
        const DPC_START: u32 = 0x180;
        const DPC_END: u32 = 0x188;
        const VI_OUTPUT: u32 = 0x100;
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2402_0000 | DPC_START, mtc0(2, 8)]
            .into_iter()
            .enumerate()
        {
            let offset = ucode_off + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        with_host(|host| {
            let memory = host.device_fabric.rsp_memory_mut();
            for (offset, word) in [
                (0x88, 0x2403_0000 | DPC_END),
                (0x8c, mtc0(3, 9)),
                (0x90, 0x0000_000d),
            ] {
                memory
                    .write_word(
                        fn64_runtime::RspMemAddr::from_parts(
                            fn64_runtime::RspMemoryBank::Imem,
                            offset,
                        ),
                        word,
                    )
                    .unwrap();
            }
        });
        let submissions = Rc::new(RefCell::new(Vec::new()));
        set_render_backend(
            Box::new(LleDpcBackend(Rc::clone(&submissions))),
            rdram.len(),
        );
        let mut vi_ctx = ctx_zeroed();
        vi_ctx.r4 = u64::from(0x8000_0000 | VI_OUTPUT);
        unsafe { crate::vi::osViSwapBuffer_recomp(rdram.as_mut_ptr(), &mut vi_ctx) };

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(*submissions.borrow(), vec![(DPC_START, DPC_END, VI_OUTPUT)]);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(snapshot.sp_busy);
            assert!(snapshot.dp_busy);
        });
    }

    #[test]
    fn unknown_task_lle_executes_persistent_imem_through_break() {
        let mut rdram = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x2402_1234u32, 0xAC02_0100, 0x0000_000D];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
        });

        let result = unsafe { dispatch_lle_task(rdram.as_mut_ptr()) };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 3,
                needs_dp: false
            }
        );
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0x0000_1234
            );
            assert_eq!(
                host.device_fabric.sp_status()
                    & (fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
                fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE
            );
        });
    }

    #[test]
    fn os_sp_task_start_go_routes_unknown_task_through_lle() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x1000];
        // task_type zero is intentionally not one of the exact HLE selectors.
        rdram[HEADER..HEADER + 4].copy_from_slice(&0u32.to_ne_bytes());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x2402_3456u32, 0xAC02_0108, 0x0000_000D];
            let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
            host.device_fabric
                .rsp_memory_mut()
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &bytes,
                )
                .unwrap();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        with_host(|host| {
            let fabric = &host.device_fabric;
            assert_eq!(
                fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x108,
                    ))
                    .unwrap(),
                0x0000_3456
            );
            assert!(
                fabric.snapshot().sp_busy,
                "LLE BREAK schedules externally visible SP completion"
            );
        });
    }

    #[test]
    fn unknown_task_lle_resolves_rspboot_style_imem_overlay_and_resumes() {
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let boot = [
            0x2402_0200u32,
            mtc0(2, 1),
            0x2403_1000,
            mtc0(3, 0),
            0x2404_001F,
            mtc0(4, 2),
            0,
            0,
        ];
        let overlay = [0u32, 0, 0, 0, 0, 0, 0x2405_5678, 0xAC05_0104];
        let mut rdram = vec![0u8; 0x1000];
        for (index, word) in overlay.into_iter().enumerate() {
            let offset = 0x200 + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        // The 32-byte DMA resumes at 0x1018; put BREAK in the still-existing
        // word immediately after the overlay transfer.
        let boot_bytes: Vec<u8> = boot.into_iter().flat_map(u32::to_be_bytes).collect();
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let memory = host.device_fabric.rsp_memory_mut();
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    &boot_bytes,
                )
                .unwrap();
            memory
                .write_word(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0x20),
                    0x0000_000D,
                )
                .unwrap();
        });
        let generation_before = with_host(|host| host.device_fabric.rsp_memory().imem_generation());

        let result = unsafe { dispatch_lle_task(rdram.as_mut_ptr()) };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 9,
                needs_dp: false
            }
        );
        with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            assert_eq!(memory.imem_generation(), generation_before + 1);
            assert_eq!(
                memory
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x104,
                    ))
                    .unwrap(),
                0x0000_5678
            );
        });
    }

    #[test]
    fn xbus_dpc_submission_stages_logical_dmem_commands_for_renderer() {
        use fn64_render::RenderConfig;

        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 4] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0003, TARGET),
            (0xf700_0000, 0x07c1_07c1),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        ];
        let mut stream = vec![0u8; commands.len() * 8];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            stream[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            stream[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_rt64::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(4, 2)).unwrap();
        set_render_backend(Box::new(backend), rdram.len());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &stream);
        }

        assert_eq!(last_render_error(), None);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        for index in 0..8 {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + index * 2)),
                0x07c1,
                "XBUS raw RDP pixel {index}"
            );
        }
    }

    #[test]
    fn xbus_dpc_submission_executes_variable_width_raw_z_triangle() {
        use fn64_render::RenderConfig;

        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let commands: [(u32, u32); 9] = [
            (0xff10_0007, TARGET),
            (0xfa00_0000, 0xff00_00ff),
            (0x0980_0000 | yl, (ym << 16) | yh),
            (1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32),
            (1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32),
            (1 << 16, 0),
            (4 << 16, 0),
            (0, 0),
            (0xe900_0000, 0),
        ];
        let mut stream = vec![0u8; commands.len() * 8];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            stream[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            stream[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_rt64::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::new(8, 8)).unwrap();
        set_render_backend(Box::new(backend), rdram.len());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &stream);
        }

        assert_eq!(last_render_error(), None);
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + (4 * 8 + 2) * 2
            )),
            0xf801
        );
    }

    #[test]
    fn os_sp_task_yielded_completed_query_does_not_resubmit_gfx_task() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 128];
        // OSTask_t header at offset 0x10 (mirrors the real call site's
        // s1+0x10 addressing): type = M_GFXTASK at +0x0.
        let header_off = 0x10usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        let before = with_executor(|exec| exec.task_log().gfx_count());
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            ctx.r2, 0,
            "task reported complete (0), not OS_TASK_YIELDED (1)"
        );
        assert_eq!(with_executor(|exec| exec.task_log().gfx_count()), before);
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
        crate::load_rom_with_fixed_pi_latency(vec![0; 0x100], 1);
        crate::pi::set_mi_interrupt_mask(
            fn64_runtime::InterruptSource::Sp.bit() | fn64_runtime::InterruptSource::Dp.bit(),
        );

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
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        let before = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_eq!(
            before
                & (fn64_runtime::InterruptSource::Sp.bit()
                    | fn64_runtime::InterruptSource::Dp.bit()),
            0
        );
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, interrupt_q, false)),
            RecvMesgOutcome::WouldBlock
        );

        crate::advance_virtual_time(8);
        let after_sp = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(after_sp & fn64_runtime::InterruptSource::Sp.bit(), 0);
        assert_eq!(after_sp & fn64_runtime::InterruptSource::Dp.bit(), 0);

        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG),
                "osSpTaskStartGo must post OS_EVENT_SP -> RSP_DONE_MSG"
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock,
                "DP completion must not collapse into the SP deadline"
            );
        });

        crate::advance_virtual_time(9);
        let after_dp = crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap();
        assert_ne!(after_dp & fn64_runtime::InterruptSource::Dp.bit(), 0);
        with_executor(|exec| {
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

    #[test]
    fn yielded_render_backend_sets_sig1_and_completes_sp_without_dp() {
        const OS_EVENT_SP: u32 = 4;
        const OS_EVENT_DP: u32 = 9;
        const RSP_DONE_MSG: u32 = 667;
        const RDP_DONE_MSG: u32 = 668;
        const HEADER_OFF: usize = 0x20;
        const YIELD_DATA: u32 = 0x180;
        const YIELD_SIZE: u32 = 0x200;

        crate::load_rom(Vec::new());
        crate::pi::set_mi_interrupt_mask(
            fn64_runtime::InterruptSource::Sp.bit() | fn64_runtime::InterruptSource::Dp.bit(),
        );
        let interrupt_q = RdramAddr::from_offset(0x0009_2000);
        with_executor(|exec| {
            exec.create_mesg_queue(interrupt_q, 4);
            exec.set_event_mesg(OS_EVENT_SP, interrupt_q, RSP_DONE_MSG);
            exec.set_event_mesg(OS_EVENT_DP, interrupt_q, RDP_DONE_MSG);
        });

        let mut rdram = vec![0u8; 0x300];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Yielded)),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0
        );

        crate::advance_virtual_time(8);
        with_executor(|exec| {
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::Delivered(RSP_DONE_MSG)
            );
            assert_eq!(
                exec.recv_mesg(99, interrupt_q, false),
                RecvMesgOutcome::WouldBlock
            );
        });
        crate::advance_virtual_time(10);
        assert_eq!(
            crate::pi::read_live_device_mmio(0xFFFF_FFFF_A430_0008).unwrap()
                & fn64_runtime::InterruptSource::Dp.bit(),
            0,
            "a yielded display list has not reached DPFullSync"
        );

        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));
        let word = |field: usize| {
            u32::from_ne_bytes(
                rdram[HEADER_OFF + field..HEADER_OFF + field + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(word(0x18), YIELD_DATA);
        assert_eq!(word(0x1c), YIELD_SIZE);
    }

    #[test]
    fn yielded_render_task_reloads_and_resumes_from_its_saved_buffer() {
        use std::sync::{Arc, Mutex};

        struct SequenceBackend {
            calls: Arc<Mutex<Vec<fn64_render::OsTask>>>,
        }

        impl RenderBackend for SequenceBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                let mut calls = self.calls.lock().unwrap();
                calls.push(*task);
                Ok(if calls.len() == 1 {
                    FrameStatus::Yielded
                } else {
                    FrameStatus::Complete
                })
            }

            fn present(&mut self, _vi: fn64_render::ViPresentation) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER_OFF: usize = 0x40;
        const INITIAL_DATA: u32 = 0x140;
        const INITIAL_SIZE: u32 = 0x40;
        const YIELD_DATA: u32 = 0x200;
        const YIELD_SIZE: u32 = 0x180;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x400];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x18, INITIAL_DATA),
            (0x1c, INITIAL_SIZE),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_render_backend(
            Box::new(SequenceBackend {
                calls: Arc::clone(&calls),
            }),
            rdram.len(),
        );
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        crate::advance_virtual_time(8);
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            crate::pi::live_sp_status()
                & (fn64_runtime::SP_STATUS_YIELD | fn64_runtime::SP_STATUS_YIELDED),
            0
        );
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        crate::advance_virtual_time(17);

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].flags & fn64_runtime::OS_TASK_YIELDED, 0);
        assert_eq!(calls[0].ucode_data, INITIAL_DATA);
        assert_ne!(calls[1].flags & fn64_runtime::OS_TASK_YIELDED, 0);
        assert_eq!(calls[1].ucode_data, YIELD_DATA);
        assert_eq!(calls[1].ucode_data_size, YIELD_SIZE);
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
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert_eq!(
            with_executor(|exec| exec.recv_mesg(99, interrupt_q, false)),
            RecvMesgOutcome::WouldBlock
        );
        crate::advance_virtual_time(8);

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
    /// planted in the SAME `rdram` buffer `osSpTaskStartGo_recomp` reads
    /// its task header from, and the call is made through the real
    /// `extern "C"` shim, not by calling the backend directly. This is the
    /// "wire the executor gfx-task seam" gate: the FULL path (recomp shim
    /// -> registered `dyn RenderBackend` -> rasterizer -> framebuffer) is
    /// exercised, not just its two halves in isolation.
    #[test]
    fn os_sp_task_start_go_routes_gfx_tasks_through_the_registered_render_backend() {
        use fn64_render::RenderConfig;
        use fn64_render_rt64::{gbi, ReferenceBackend};

        const RDRAM_LEN: usize = 0x4000;
        const VTX_ADDR: usize = 0x1000;
        const DL_ADDR: usize = 0x2000;
        const HEADER_OFF: usize = 0x10;

        crate::load_rom(Vec::new());
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

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER_OFF, &mut ctx);
        let mut backend = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        backend.create(&RenderConfig::new(64, 64)).unwrap();
        set_render_backend(Box::new(backend), rdram.len());
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

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
        // error-free StartGo result above, this closes the loop end-to-end:
        // the seam call really executed the real decode+rasterize path on
        // this fixture, not a silent no-op.
        let mut direct = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        direct.create(&RenderConfig::new(64, 64)).unwrap();
        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: DL_ADDR as u32,
            ..Default::default()
        };
        direct
            .process_task(&mut rdram, &mut fn64_runtime::RspMemory::new(), &task, 0)
            .unwrap();
        assert!(
            direct
                .framebuffer()
                .unwrap()
                .has_non_uniform_content(1, 2, 3, 255),
            "the same fixture bytes must produce a non-clear frame through the reference backend"
        );
        crate::advance_virtual_time(9);
    }

    #[test]
    fn os_sp_task_yielded_query_does_not_call_audio_ucode_again() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        static SEEN_UCODE_ADDR: AtomicU32 = AtomicU32::new(0);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            SEEN_UCODE_ADDR.store(task_offset, Ordering::SeqCst);
            0
        }
        unsafe { set_audio_ucode_fn(fake_ucode) };
        CALLED.store(false, Ordering::SeqCst);
        SEEN_UCODE_ADDR.store(0, Ordering::SeqCst);
        crate::load_rom(Vec::new());

        let mut rdram = vec![0u8; 128];
        let header_off = 0x20usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());
        rdram[header_off + 0x10..header_off + 0x14].copy_from_slice(&0xDEADu32.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(!CALLED.load(Ordering::SeqCst));
        assert_eq!(SEEN_UCODE_ADDR.load(Ordering::SeqCst), 0);
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
        CALLED.store(false, Ordering::SeqCst);
        SEEN_OFFSET.store(0, Ordering::SeqCst);
        crate::load_rom(Vec::new());

        let mut rdram = vec![0u8; 128];
        let header_off = 0x30usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert!(
            CALLED.load(Ordering::SeqCst),
            "osSpTaskStartGo must call the real ucode fn for an M_AUDTASK (the OoT path)"
        );
        assert_eq!(
            SEEN_OFFSET.load(Ordering::SeqCst),
            header_off as u32,
            "ucode receives the OSTask rdram offset"
        );
        crate::advance_virtual_time(8);
    }

    #[test]
    fn os_sp_task_yield_sets_the_public_sig0_request() {
        crate::load_rom(Vec::new());
        let mut rdram = [0u8; 4];
        let mut ctx = ctx_zeroed();

        unsafe { osSpTaskYield_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELD,
            0
        );
        assert_eq!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0,
            "the CPU request must not fabricate the microcode acknowledgement"
        );
    }

    #[test]
    fn os_sp_task_yielded_prepares_the_saved_task_for_restart() {
        const HEADER_OFF: usize = 0x40;
        const FLAGS: u32 = 0x20;
        const OLD_UCODE_DATA: u32 = 0x1234;
        const OLD_UCODE_DATA_SIZE: u32 = 0x80;
        const YIELD_DATA: u32 = 0x4321;
        const YIELD_DATA_SIZE: u32 = 0x900;

        crate::load_rom(Vec::new());
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
        let mut rdram = vec![0u8; HEADER_OFF + 0x40];
        for (field, value) in [
            (0x04, FLAGS),
            (0x18, OLD_UCODE_DATA),
            (0x1c, OLD_UCODE_DATA_SIZE),
            (0x38, YIELD_DATA),
            (0x3c, YIELD_DATA_SIZE),
        ] {
            rdram[HEADER_OFF + field..HEADER_OFF + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;

        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        let word = |field: usize| {
            u32::from_ne_bytes(
                rdram[HEADER_OFF + field..HEADER_OFF + field + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));
        assert_eq!(word(0x04), FLAGS | fn64_runtime::OS_TASK_YIELDED);
        assert_eq!(word(0x18), YIELD_DATA);
        assert_eq!(word(0x1c), YIELD_DATA_SIZE);
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0,
            "the observation call must not invent an undocumented signal clear"
        );
    }

    #[test]
    fn os_sp_task_load_clears_stale_yield_handshake_bits() {
        const HEADER_OFF: usize = 0x40;
        const RSPBOOT_OFF: u32 = 0x100;

        crate::load_rom(Vec::new());
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELD | fn64_runtime::SP_SET_YIELDED);
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER_OFF + 0x08..HEADER_OFF + 0x0c].copy_from_slice(&RSPBOOT_OFF.to_ne_bytes());
        rdram[HEADER_OFF + 0x0c..HEADER_OFF + 0x10].copy_from_slice(&8u32.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER_OFF as u64;

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };

        assert_eq!(
            crate::pi::live_sp_status()
                & (fn64_runtime::SP_STATUS_YIELD | fn64_runtime::SP_STATUS_YIELDED),
            0
        );
    }

    /// osSpTaskYielded, in this crate's synchronous run-to-completion model,
    /// must report task COMPLETED (0), not OS_TASK_YIELDED (1). Returning 1
    /// makes the scheduler re-queue an already-finished task forever. Fails
    /// against the bug (`ctx.r2 = 1`).
    #[test]
    fn os_sp_task_yielded_reports_completed_not_yielded() {
        crate::load_rom(Vec::new());
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
