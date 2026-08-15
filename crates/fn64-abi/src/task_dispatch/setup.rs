use super::*;

pub(crate) const RENAMED_ENV_VARS: &[(&str, &str)] = &[
    ("OOT_DUMP_AUDIO_PCM", "FN64_DUMP_AUDIO_PCM"),
    ("OOT_DUMP_AUDIO_TASK", "FN64_DUMP_AUDIO_TASK"),
    ("OOT_AUDIO_UCODE_TIMING", "FN64_AUDIO_UCODE_TIMING"),
    ("OOT_PHASE_TIMING", "FN64_PHASE_TIMING"),
];

pub(crate) const RETIRED_ENV_VARS: &[(&str, &str)] = &[
    (
        "OOT_SKIP_AUDIO_UCODE",
        "call fn64_abi::set_audio_task_diagnostic_skip() explicitly",
    ),
    (
        "FN64_SKIP_AUDIO_UCODE",
        "call fn64_abi::set_audio_task_diagnostic_skip() explicitly",
    ),
];

struct AudioDiagnostics {
    trace_buffers: bool,
    stream_dump_path: Option<std::path::PathBuf>,
    one_shot_dump_path: Option<std::path::PathBuf>,
}

/// Audio evidence switches are launch-time configuration. AI delivery runs at
/// the guest buffer cadence, so it must not rescan the process environment.
fn audio_diagnostics() -> &'static AudioDiagnostics {
    static CONFIG: std::sync::OnceLock<AudioDiagnostics> = std::sync::OnceLock::new();
    CONFIG.get_or_init(|| AudioDiagnostics {
        trace_buffers: std::env::var_os("FN64_TRACE_AI_BUFFERS").is_some(),
        stream_dump_path: std::env::var_os("FN64_DUMP_AUDIO_STREAM_PCM")
            .map(std::path::PathBuf::from),
        one_shot_dump_path: std::env::var_os("FN64_DUMP_AUDIO_PCM")
            .map(std::path::PathBuf::from),
    })
}

/// Panic if any pre-rename `OOT_*` knob is still set, naming its replacement.
///
/// The first audio task/AI-buffer seam scans all retired spellings together.
/// That preserves the loud trap even when the corresponding new knob is not
/// used, while later buffers pay only a completed `OnceLock` check.
pub(crate) fn assert_no_legacy_env_vars() {
    static CHECKED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    CHECKED.get_or_init(|| {
        for (old, new) in RENAMED_ENV_VARS {
            if std::env::var_os(old).is_some() {
                panic!("{}", legacy_env_var_message(old, new));
            }
        }
        for (name, replacement) in RETIRED_ENV_VARS {
            if std::env::var_os(name).is_some() {
                panic!("{name} was retired; {replacement}");
            }
        }
    });
}

/// The trap's message, split out so a test can assert the wording without
/// mutating the shared test process's environment -- doing that trips this
/// very trap inside every sibling test that dispatches an audio task.
pub(crate) fn legacy_env_var_message(old: &str, new: &str) -> String {
    format!(
        "{old} was renamed to {new}; the old name is ignored, so this run \
         would silently not do what you asked. Set {new} instead."
    )
}

pub(crate) const AUDIO_STREAM_DUMP_SECONDS: u64 = 12;

pub(crate) struct AudioStreamDump {
    pub(crate) file: std::fs::File,
    pub(crate) path: std::path::PathBuf,
    pub(crate) sample_rate_hz: u32,
    pub(crate) samples_written: u64,
    pub(crate) buffers_written: u64,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct AudioTaskDumpState {
    pub(crate) seen: u64,
    pub(crate) dumped: bool,
}

pub(crate) fn dump_audio_pcm_stream(samples: &[i16]) {
    use std::io::Write as _;

    let Some(path) = audio_diagnostics().stream_dump_path.as_deref() else {
        return;
    };
    AUDIO_PCM_STREAM_DUMP.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.is_none() {
            match std::fs::File::create(path) {
                Ok(file) => {
                    let sample_rate_hz = AUDIO_GUEST_RATE.with(Cell::get);
                    eprintln!(
                        "[fn64-abi] capturing up to {AUDIO_STREAM_DUMP_SECONDS}s of pre-resample stereo PCM at {sample_rate_hz} Hz to {path:?}"
                    );
                    *state = Some(AudioStreamDump {
                        file,
                        path: path.to_path_buf(),
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
    let mut samples = AUDIO_SAMPLE_SCRATCH.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    samples.clear();
    samples.reserve(byte_len / 2);
    samples.extend((0..byte_len).step_by(2).map(|guest_offset| {
        view.read_i16(
            start_addr
                .checked_add(
                    u32::try_from(guest_offset).expect("AI PCM buffer length exceeds u32"),
                )
                .expect("AI PCM logical address overflow"),
        )
    }));

    AUDIO_DIGEST_CAPTURE.with(|cell| {
        let mut capture = cell.borrow_mut();
        if let Some(bytes) = capture.as_mut() {
            bytes.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
        }
    });

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

    if audio_diagnostics().trace_buffers {
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
        if let Some(path) = audio_diagnostics().one_shot_dump_path.as_deref() {
            AUDIO_PCM_DUMPED.with(|dumped| {
                if !dumped.get() {
                    let pcm: Vec<u8> = samples
                        .iter()
                        .flat_map(|sample| sample.to_le_bytes())
                        .collect();
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
            let result = backend.queue_samples(fn64_audio::GuestPcm16::new(
                &samples,
                fn64_audio::ChannelCount::STEREO,
            ));
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
    AUDIO_SAMPLE_SCRATCH.with(|cell| *cell.borrow_mut() = samples);
}

/// Run one renderer operation through the process's single registered backend.
/// Missing registration and named backend errors are one loud failure class;
/// no caller may independently turn either into a successful task completion.
pub(crate) fn with_render_backend<T>(
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
                if let fn64_render::RenderError::UnsupportedUcode { ucode_addr } = &error {
                    fn64_runtime::record_unsupported_event(
                        fn64_runtime::UnsupportedSubsystem::Render,
                        "render.backend.unsupported-ucode",
                        format!(
                            "{context}: backend rejected unlisted microcode at RDRAM offset {ucode_addr:#010x}"
                        ),
                        Some(fn64_runtime::Cycles::new(crate::sim_time())),
                        fn64_runtime::UnsupportedDisposition::LoudTrap,
                    );
                }
                RENDER_LAST_ERROR.with(|last| last.replace(Some(reason.clone())));
                panic!("{context}: {reason}");
            }
        }
    })
}

/// Deliver one completed CPU halfword store to the currently registered
/// renderer. Guest execution is single-threaded and renderer calls never run
/// guest code, so a borrow collision is a real recursive-entry bug rather
/// than contention to retry. No registered renderer is a valid early-boot or
/// unit-test state and therefore has no sidecar owner to notify.
pub(crate) fn observe_non_rdp_write16(
    logical_offset: u32,
    value: u16,
) -> Option<fn64_render::NonRdpWrite16Disposition> {
    let write = fn64_render::NonRdpWrite16::new(logical_offset, value);
    RENDER_BACKEND.with(|cell| {
        let mut registered = cell.try_borrow_mut().unwrap_or_else(|_| {
            panic!(
                "observe_non_rdp_write16: recursive renderer entry while delivering physical RDRAM halfword {logical_offset:#x}"
            )
        });
        registered
            .as_mut()
            .map(|backend| backend.observe_non_rdp_write16(write))
    })
}

/// Physical color buffer selected by the VI manager at this renderer
/// boundary. Raw DPC submissions need the same explicit value as HLE tasks;
/// making a backend infer it from prior calls breaks CPU-only RDP streams and
/// tasks rejected to LLE before any HLE work is committed.
pub(crate) fn render_output_addr() -> u32 {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderDispatchResult {
    pub(crate) status: fn64_render::FrameStatus,
    pub(crate) dp_full_sync: fn64_render::DpFullSyncStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderChunkDispatchResult {
    pub(crate) status: fn64_render::RenderTaskChunkStatus,
    pub(crate) dp_full_sync: fn64_render::DpFullSyncStatus,
    pub(crate) chunking: fn64_render::RenderTaskChunking,
}

pub(crate) fn render_task(header: &OsTaskHeader) -> fn64_render::OsTask {
    fn64_render::OsTask {
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
        // The display-list pointer arrives as a guest KSEG0/KSEG1 virtual
        // address (WM2000's is 0x8038ce30); the renderer backend indexes
        // physical RDRAM and rt64 ingress rejects any non-physical offset.
        // Strip the segment bits with the standard KSEG0/1 -> physical map;
        // ingress then range-checks the 8 MiB window. Already-physical offsets
        // (e.g. OoT's) pass through unchanged. Only `data_ptr` is masked here
        // because it is the only address rt64 ingress validates.
        data_ptr: header.data_ptr & 0x1fff_ffff,
        data_size: header.data_size,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) unsafe fn dispatch_gfx_task(rdram: *mut u8, header: &OsTaskHeader) -> RenderDispatchResult {
    let result = unsafe {
        dispatch_gfx_task_chunk(
            rdram,
            header,
            fn64_render::RenderTaskStep::Start,
            render_output_addr(),
        )
    };
    let status = match result.status {
        fn64_render::RenderTaskChunkStatus::Complete => fn64_render::FrameStatus::Complete,
        fn64_render::RenderTaskChunkStatus::Yielded => fn64_render::FrameStatus::Yielded,
        fn64_render::RenderTaskChunkStatus::NeedsLle { ucode_sha256 } => {
            fn64_render::FrameStatus::NeedsLle { ucode_sha256 }
        }
        fn64_render::RenderTaskChunkStatus::Continue(token) => panic!(
            "dispatch_gfx_task: resumable backend retained continuation token {}; caller requires atomic completion",
            token.get()
        ),
    };
    RenderDispatchResult {
        status,
        dp_full_sync: result.dp_full_sync,
    }
}

pub(crate) unsafe fn dispatch_gfx_task_chunk(
    rdram: *mut u8,
    header: &OsTaskHeader,
    step: fn64_render::RenderTaskStep,
    output_addr: u32,
) -> RenderChunkDispatchResult {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    let status = with_render_backend("dispatch_gfx_task_chunk", |backend| {
        let task = render_task(header);
        let rdram_slice = unsafe { renderer_rdram_slice(rdram) };
        // The color framebuffer the VI presents (`osViSwapBuffer`'s frame
        // buffer, e.g. OoT's 0x3b5000/0x3da800) -- NOT `task.output_buff`
        // (OoT's is 0x80151640, the RSP's DRAM command-FIFO output region,
        // a different address). The reference backend rasterizes into its
        // own surface and copies the result here so the VI-presented frame
        // isn't blank. `0` (no VI framebuffer set yet) tells the backend
        // "no known color target": it renders to its own surface only.
        track_rdp_renderer_mutation(rdram_slice, |rdram_slice| {
            with_host(|host| {
                let status = backend.process_task_chunk(
                    rdram_slice,
                    host.device_fabric.rsp_memory_mut(),
                    &task,
                    output_addr,
                    step,
                )?;
                Ok(RenderChunkDispatchResult {
                    status,
                    dp_full_sync: backend.last_dp_full_sync(),
                    chunking: backend.task_chunking(),
                })
            })
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
    if matches!(
        status.status,
        fn64_render::RenderTaskChunkStatus::NeedsLle { .. }
    ) {
        record_rdp_renderer_rejection_v1();
    } else {
        record_rdp_renderer_publication_v1();
    }
    status
}

/// Present the registered graphics backend at the guest's real VI retrace
/// boundary. Task submission and VI presentation are distinct on N64; this
/// closes the second half of `RenderBackend` without exposing RT64 or any
/// foreign type outside `fn64-render-rt64`.
pub(crate) fn present_render_backend(vi: fn64_render::ViPresentation) {
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    let (rdram, allocation_len) = with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    // SAFETY: every retrace presentation runs after device commit and before
    // any guest coroutine resumes. The boot contract keeps this one process
    // allocation live, while the higher-ranked capability prevents a backend
    // from retaining it beyond the call. No competing Rust slice is created:
    // typed recompiled execution may retain its dormant checked RDRAM borrow.
    unsafe {
        fn64_runtime::with_physical_rdram_read(rdram, allocation_len, |memory| {
            with_render_backend("present_render_backend", |backend| {
                backend.present(fn64_render::PresentRequest::live(vi, memory))
            })
        })
    }
    if let Some(started) = started {
        let elapsed_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        VI_PRESENT_NS.with(|total| total.set(total.get().saturating_add(elapsed_ns)));
        VI_PRESENT_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
    }
    // Settle by observation whether presentation is inside `executor_ns`.
    // `telemetry.rs` subtracts `vi_present_ns` out of `executor_ns` as though
    // it were nested; the call graph says it is not (this function's only
    // caller is `advance_device_time_step`, which `run_one_step` and the
    // harness's `advance_virtual_time` both reach, and only the former is
    // inside `executor_ns`). Counting which side each call actually ran on
    // replaces that two-step inference with a measurement that can refute it.
    if INSIDE_RUN_ONE_STEP.with(Cell::get) {
        VI_PRESENT_IN_EXECUTOR_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    } else {
        VI_PRESENT_OUTSIDE_EXECUTOR_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LleTaskResult {
    pub(crate) steps: u64,
    pub(crate) dp_full_sync: fn64_render::DpFullSyncStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct HleBootResult {
    pub(crate) steps: u64,
    pub(crate) task: OsTaskHeader,
    pub(crate) machine_state: fn64_audio::rsp::runtime::RspMachineState,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingImemReplacement {
    pub(crate) generation: u64,
    pub(crate) image: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
}
