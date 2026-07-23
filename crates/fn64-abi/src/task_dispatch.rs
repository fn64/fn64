use super::*;
use sha2::{Digest, Sha256};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderDispatchResult {
    status: fn64_render::FrameStatus,
    dp_full_sync: fn64_render::DpFullSyncStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderChunkDispatchResult {
    status: fn64_render::RenderTaskChunkStatus,
    dp_full_sync: fn64_render::DpFullSyncStatus,
    chunking: fn64_render::RenderTaskChunking,
}

fn render_task(header: &OsTaskHeader) -> fn64_render::OsTask {
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
unsafe fn dispatch_gfx_task(rdram: *mut u8, header: &OsTaskHeader) -> RenderDispatchResult {
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

unsafe fn dispatch_gfx_task_chunk(
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

/// Present the registered graphics backend at the guest's real VI retrace
/// boundary. Task submission and VI presentation are distinct on N64; this
/// closes the second half of `RenderBackend` without exposing RT64 or any
/// foreign type outside `fn64-render-rt64`.
pub(crate) fn present_render_backend(vi: fn64_render::ViPresentation) {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LleTaskResult {
    steps: u64,
    dp_full_sync: fn64_render::DpFullSyncStatus,
}

#[derive(Clone, Debug)]
struct HleBootResult {
    steps: u64,
    task: OsTaskHeader,
    machine_state: fn64_audio::rsp::runtime::RspMachineState,
}

#[derive(Clone, Debug)]
struct PendingImemReplacement {
    generation: u64,
    image: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
}

fn imem_sha256(imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE]) -> [u8; 32] {
    Sha256::digest(imem).into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskMicrocodeDataIdentity {
    addr: RdramAddr,
    size: u32,
    sha256: [u8; 32],
}

impl TaskMicrocodeDataIdentity {
    fn evidence_snapshot(self) -> RspTaskDataIdentityEvidenceSnapshot {
        RspTaskDataIdentityEvidenceSnapshot {
            rdram_offset: self.addr.offset(),
            byte_len: self.size,
            sha256: self.sha256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RspTaskLineagePhase {
    Running,
    ResumeAuthorized,
    ResumeLoaded,
}

impl RspTaskLineagePhase {
    fn evidence_snapshot(self) -> RspTaskLineagePhaseEvidenceSnapshot {
        match self {
            Self::Running => RspTaskLineagePhaseEvidenceSnapshot::Running,
            Self::ResumeAuthorized => RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized,
            Self::ResumeLoaded => RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RspTaskLineage {
    original_header: OsTaskHeader,
    data_identity: Option<TaskMicrocodeDataIdentity>,
    phase: RspTaskLineagePhase,
}

impl RspTaskLineage {
    pub(crate) fn evidence_snapshot(&self, task_offset: u32) -> RspTaskLineageEvidenceSnapshot {
        RspTaskLineageEvidenceSnapshot {
            task_offset,
            original_header: self.original_header,
            data_identity: self
                .data_identity
                .map(TaskMicrocodeDataIdentity::evidence_snapshot),
            phase: self.phase.evidence_snapshot(),
        }
    }

    fn yielded_header(self) -> OsTaskHeader {
        OsTaskHeader {
            flags: self.original_header.flags | fn64_runtime::OS_TASK_YIELDED,
            ucode_data: self.original_header.yield_data_ptr,
            ucode_data_size: self.original_header.yield_data_size,
            ..self.original_header
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoadedRspTask {
    task_addr: RdramAddr,
    header: OsTaskHeader,
    resumed_data_identity: Option<TaskMicrocodeDataIdentity>,
}

impl LoadedRspTask {
    pub(crate) fn evidence_snapshot(&self) -> LoadedRspTaskEvidenceSnapshot {
        LoadedRspTaskEvidenceSnapshot {
            task_offset: self.task_addr.offset(),
            header: self.header,
            resumed_data_identity: self
                .resumed_data_identity
                .map(TaskMicrocodeDataIdentity::evidence_snapshot),
        }
    }
}

/// Capture the original task microcode-data image at the RSP kickoff boundary.
///
/// The source address and size come from the typed header retained by
/// `osSpTaskLoad`, never from the mutable CPU `OSTask` storage. SP_DRAM_ADDR
/// canonicalizes addresses to 24 bits; the result must remain inside physical
/// RDRAM even when the host allocation appends sparse MMIO backing.
///
/// # Safety
/// `rdram` must address the process allocation registered in `HostState`.
unsafe fn task_microcode_data_identity(
    rdram: *mut u8,
    task_addr: RdramAddr,
    source_addr: u32,
    size: u32,
) -> TaskMicrocodeDataIdentity {
    let (registered_rdram, allocation_len) =
        with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    assert!(
        !rdram.is_null() && allocation_len != 0,
        "RSP task {:#010x} microcode-data capture has no registered process RDRAM allocation",
        task_addr.offset()
    );
    assert_eq!(
        registered_rdram, rdram,
        "RSP task {:#010x} microcode-data capture does not use the registered process RDRAM allocation",
        task_addr.offset()
    );
    let addr = RdramAddr::from_offset(source_addr & 0x00ff_ffff);
    let start = addr.offset() as usize;
    let end = start.checked_add(size as usize).unwrap_or_else(|| {
        panic!(
            "RSP task {:#010x} microcode-data range overflows host usize: start={:#010x} size={size:#x}",
            task_addr.offset(),
            addr.offset()
        )
    });
    assert!(
        end <= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "RSP task {:#010x} microcode-data range [{:#010x}, {end:#010x}) exceeds physical RDRAM length {:#x}",
        task_addr.offset(),
        addr.offset(),
        fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
    );
    assert!(
        end <= allocation_len,
        "RSP task {:#010x} microcode-data range [{:#010x}, {end:#010x}) exceeds registered allocation length {allocation_len:#x}",
        task_addr.offset(),
        addr.offset(),
    );

    let memory = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let mut digest = Sha256::new();
    for offset in 0..size {
        let byte_addr = addr.checked_add(offset).unwrap_or_else(|| {
            panic!(
                "RSP task {:#010x} microcode-data logical address overflow at byte {offset:#x}",
                task_addr.offset()
            )
        });
        digest.update([unsafe { memory.read_u8(byte_addr) }]);
    }
    TaskMicrocodeDataIdentity {
        addr,
        size,
        sha256: digest.finalize().into(),
    }
}

fn identify_microcode_pair(
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    data: TaskMicrocodeDataIdentity,
    authoritative_family: Option<fn64_render::UcodeId>,
) -> Option<fn64_render::UcodeId> {
    let backend_family = with_render_backend("identify_microcode_pair", |backend| {
        Ok(backend.identify_microcode_pair(
            imem,
            fn64_render::MicrocodeDataImageIdentity {
                bytes: data.size,
                sha256: data.sha256,
            },
        ))
    });
    match (authoritative_family, backend_family) {
        (Some(authoritative), Some(backend)) if authoritative != backend => {
            panic!(
                "pinned microcode classifier identified {authoritative:?}, but the backend pair catalog claimed {backend:?}"
            )
        }
        (Some(authoritative), _) => Some(authoritative),
        (None, backend) => backend,
    }
}

/// Classify the immutable task-entry raw text/data storage through the pinned
/// MIT RT64 identity table. This does not admit HLE; it prevents a backend or
/// private pair declaration from choosing the family written to LLE evidence.
///
/// # Safety
/// `rdram` must be the registered process allocation.
unsafe fn classify_task_microcode_family(
    rdram: *mut u8,
    header: &OsTaskHeader,
) -> Option<fn64_render::UcodeId> {
    let storage = unsafe { renderer_rdram_slice(rdram) };
    let window = fn64_render::capture_task_admission_raw_window(
        storage,
        RdramAddr::from_offset(header.ucode & 0x00ff_ffff),
        RdramAddr::from_offset(header.ucode_data & 0x00ff_ffff),
        fn64_render::F3DZEX2_RAW_WINDOW_SIZE,
    )?;
    fn64_render::identify_f3dzex2(&window).map(fn64_render::F3dzex2Variant::family)
}

fn canonical_rdp_words_sha256(words: &[u32]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for word in words {
        digest.update(word.to_be_bytes());
    }
    digest.finalize().into()
}

fn dpc_observation(xbus: bool, start: u32, end: u32, words: &[u32]) -> RspRdpObservationKind {
    let command_sha256 = canonical_rdp_words_sha256(words);
    if xbus {
        RspRdpObservationKind::XbusDpcCommitted {
            start,
            end,
            command_sha256,
        }
    } else {
        RspRdpObservationKind::DramDpcCommitted {
            start,
            end,
            command_sha256,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdmittedTaskImageShape {
    BootOverlay,
    DirectImem,
}

#[derive(Clone, Debug)]
enum AdmittedHleEntry {
    BootOverlay(Box<HleBootResult>),
    DirectImem { task: OsTaskHeader },
}

impl AdmittedHleEntry {
    fn task(&self) -> OsTaskHeader {
        match self {
            Self::BootOverlay(boot) => boot.task,
            Self::DirectImem { task } => *task,
        }
    }

    fn pre_ucode_steps(&self) -> u64 {
        match self {
            Self::BootOverlay(boot) => boot.steps,
            Self::DirectImem { .. } => 0,
        }
    }

    fn into_lle_machine_state(self) -> Option<fn64_audio::rsp::runtime::RspMachineState> {
        match self {
            Self::BootOverlay(boot) => Some(boot.machine_state),
            Self::DirectImem { .. } => None,
        }
    }
}

fn aligned_sp_image_size(size: u32) -> Option<u32> {
    size.checked_add(7)
        .map(|size| size & !7)
        .filter(|size| *size != 0 && *size as usize <= fn64_runtime::RSP_MEMORY_BANK_SIZE)
}

fn admitted_task_image_shape(header: &OsTaskHeader) -> AdmittedTaskImageShape {
    let boot = header.ucode_boot & 0x1fff_ffff;
    let ucode = header.ucode & 0x1fff_ffff;
    let direct_image = boot == ucode
        && boot.is_multiple_of(8)
        && header.ucode_size != 0
        && header.ucode_size as usize <= fn64_runtime::RSP_MEMORY_BANK_SIZE
        && aligned_sp_image_size(header.ucode_boot_size)
            .is_some_and(|copy_size| copy_size >= header.ucode_size);
    if direct_image {
        AdmittedTaskImageShape::DirectImem
    } else {
        AdmittedTaskImageShape::BootOverlay
    }
}

/// Host policy for the graphics microcode phase of an admitted `M_GFXTASK`.
///
/// Both policies classify the admitted image first. Boot-overlay tasks execute
/// rspboot through the clean-room RSP interpreter and commit its complete
/// post-DMA machine state; direct IMEM images already enter at the fabric's PC
/// zero. `HleOptimized` then offers that task-entry state to the registered
/// graphics backend, retaining the transactional LLE fallback for an
/// unadmitted digest. `LleAccuracy` instead continues the loaded graphics
/// microcode through the existing interpreter unconditionally and forwards
/// only its raw DPC submissions to the backend.
/// The latter avoids making an HLE decoder's arithmetic part of an accuracy
/// claim; it does not select a different RDP implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsTaskExecutionPolicy {
    /// Prefer an exact-digest HLE implementation, falling back transactionally.
    #[default]
    HleOptimized,
    /// Execute every loaded graphics microcode instruction through LLE.
    LleAccuracy,
}

/// Exact registered renderer state frozen for fixed-cycle release evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderEnvironmentEvidenceSnapshot {
    pub backend: fn64_render::RenderBackendEvidence,
    pub execution_policy: GraphicsTaskExecutionPolicy,
}

impl RenderEnvironmentEvidenceSnapshot {
    /// Renderer-owned TV standard from the last successful backend creation.
    /// An unidentified compatibility backend has no release authority.
    pub const fn renderer_tv_type(&self) -> Option<fn64_runtime::TvType> {
        self.backend.tv_type()
    }
}

/// Publish the RSP core's guest-visible RDRAM effects after direct execution.
/// The bytes are already in the live allocation; only recompiler bookkeeping
/// and executable-page invalidation remain.
#[cfg(feature = "recomp-rs")]
fn commit_rsp_rdram_writes(written: &[(usize, usize)]) {
    if written.is_empty() {
        return;
    }
    for &(start, end) in written {
        fn64_recomp_rs::notify_guest_write(start as u32, (end - start) as u32);
    }
    crate::recompiled::process_live_executable_writes_from_host();
}

#[cfg(not(feature = "recomp-rs"))]
fn commit_rsp_rdram_writes(_written: &[(usize, usize)]) {}

fn rsp_visible_rdram_len(allocation_len: usize) -> usize {
    allocation_len.min(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE)
}

/// Expose the renderer and RDP to physical RDRAM, never the host-only MMIO
/// backing appended to the generated-code allocation. Retail commands can
/// address the final byte of the 8 MiB device, so a short registration is a
/// caller error rather than a reason to truncate the hardware-visible span.
unsafe fn renderer_rdram_slice<'a>(rdram: *mut u8) -> &'a mut [u8] {
    let allocation_len = RDRAM_LEN.with(Cell::get);
    assert!(
        allocation_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "renderer RDRAM allocation length {allocation_len:#x} does not cover the required 8 MiB physical device"
    );
    unsafe { std::slice::from_raw_parts_mut(rdram, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE) }
}

fn rsp_dma_storage_layout(
    allocation_len: usize,
    static_aliases: Vec<std::ops::Range<u32>>,
) -> (Vec<std::ops::Range<usize>>, usize) {
    let physical_len = rsp_visible_rdram_len(allocation_len);
    let mut ranges: Vec<_> = std::iter::once(0..physical_len).collect();
    let mut snapshot_len = physical_len;
    for alias in static_aliases {
        let start = alias.start as usize;
        let end = alias.end as usize;
        assert!(
            start < end && end <= allocation_len,
            "loaded static-overlay RSP alias [{start:#x}, {end:#x}) is invalid for host RDRAM \
             allocation length {allocation_len:#x}"
        );
        ranges.push(start..end);
        snapshot_len = snapshot_len.max(end);
    }
    (ranges, snapshot_len)
}

unsafe fn trace_rsp_rdram_words(rdram: *const u8, rdram_len: usize) {
    let Some(spec) = std::env::var_os("RSP_TRACE_RDRAM_WORDS") else {
        return;
    };
    let spec = spec
        .to_str()
        .unwrap_or_else(|| panic!("RSP_TRACE_RDRAM_WORDS must be UTF-8"));
    let (offset, count) = spec
        .split_once(':')
        .unwrap_or_else(|| panic!("RSP_TRACE_RDRAM_WORDS must be OFFSET:COUNT, got {spec:?}"));
    let offset = usize::from_str_radix(offset.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("RSP_TRACE_RDRAM_WORDS offset must be hexadecimal"));
    let count = count
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("RSP_TRACE_RDRAM_WORDS count must be decimal"));
    let byte_len = count
        .checked_mul(4)
        .expect("RSP_TRACE_RDRAM_WORDS byte length overflow");
    let end = offset
        .checked_add(byte_len)
        .expect("RSP_TRACE_RDRAM_WORDS range overflow");
    assert!(
        end <= rdram_len,
        "RSP_TRACE_RDRAM_WORDS range exceeds host allocation"
    );
    let bytes = unsafe { std::slice::from_raw_parts(rdram.add(offset), byte_len) };
    let words: Vec<_> = bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four RDRAM bytes")))
        .collect();
    eprintln!("[fn64-abi/rsp] RDRAM {offset:#x} words={words:08x?}");
}

fn trace_rsp_dmem_words(dmem: &[u8], overlay: u64, pc: u32) {
    let Some(spec) = std::env::var_os("RSP_TRACE_DMEM_WORDS") else {
        return;
    };
    let spec = spec
        .to_str()
        .unwrap_or_else(|| panic!("RSP_TRACE_DMEM_WORDS must be UTF-8"));
    let (offset, count) = spec
        .split_once(':')
        .unwrap_or_else(|| panic!("RSP_TRACE_DMEM_WORDS must be OFFSET:COUNT, got {spec:?}"));
    let offset = usize::from_str_radix(offset.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("RSP_TRACE_DMEM_WORDS offset must be hexadecimal"));
    let count = count
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("RSP_TRACE_DMEM_WORDS count must be decimal"));
    let byte_len = count
        .checked_mul(4)
        .expect("RSP_TRACE_DMEM_WORDS byte length overflow");
    let end = offset
        .checked_add(byte_len)
        .expect("RSP_TRACE_DMEM_WORDS range overflow");
    assert!(end <= dmem.len(), "RSP_TRACE_DMEM_WORDS range exceeds DMEM");
    let words: Vec<_> = dmem[offset..end]
        .chunks_exact(4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four DMEM bytes")))
        .collect();
    eprintln!("[fn64-abi/rsp] overlay={overlay} pc={pc:#06x} DMEM {offset:#x} words={words:08x?}");
}

fn lle_debug_task_data(rdram: &[u8], source_addr: u32, source_size: u32) -> Option<Vec<u8>> {
    let addr = RdramAddr::from_offset(source_addr & 0x00ff_ffff);
    let requested_len = (source_size as usize).clamp(0x40, 0x20000);
    let start = addr.offset() as usize;
    let end = start
        .checked_add(requested_len)
        .expect("LLE debug task-data range overflow")
        .min(rdram.len());
    if start >= end {
        return None;
    }

    let mut logical = vec![0; end - start];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(addr, &mut logical);
    Some(logical)
}

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
    let _ = writeln!(
        state,
        "dma_mem_address {:#010x}",
        machine.ctx.dma_mem_address
    );
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

    if let Some(logical) = lle_debug_task_data(
        machine.rdram,
        field(initial_dmem, 0x30),
        field(initial_dmem, 0x34),
    ) {
        write("task_data_logical.bin", &logical);
    }
    let raw_len = machine
        .rdram
        .len()
        .min(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
    write("rdram_raw.bin", &machine.rdram[..raw_len]);
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
unsafe fn dispatch_lle_task(
    rdram: *mut u8,
    task_addr: RdramAddr,
    recognize_graphics_microcode: bool,
    machine_state: Option<fn64_audio::rsp::runtime::RspMachineState>,
    microcode_data: Option<TaskMicrocodeDataIdentity>,
    authoritative_family: Option<fn64_render::UcodeId>,
) -> LleTaskResult {
    const CHUNK_STEPS: u64 = 1 << 20;
    const MAX_TASK_STEPS: u64 = 1 << 26;

    let (mut dmem, mut imem, status, mut pc, imem_generation, rdram_len, static_aliases) =
        with_host(|host| {
            let fabric = &host.device_fabric;
            (
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
                fabric.sp_status(),
                fabric.sp_pc(),
                fabric.rsp_memory().imem_generation(),
                host.runtime_rdram_len,
                host.sections.loaded_static_storage_ranges(),
            )
        });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP LLE task has no registered process RDRAM allocation"
    );
    let initial_imem = imem;
    unsafe { trace_rsp_rdram_words(rdram, rdram_len) };
    let (dma_ranges, _) = rsp_dma_storage_layout(rdram_len, static_aliases);
    // Execute directly against the live allocation. RSP stores are bounded by
    // the admitted DMA ranges and logged by the machine for post-run JIT
    // invalidation; no full-allocation snapshot or memcmp is needed.
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.set_dma_rdram_ranges(dma_ranges);
    machine.load_dmem_logical(&dmem);
    machine.set_sp_status_raw(
        status & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
    );
    if let Some(state) = machine_state {
        machine.restore_state(state);
    }
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    let mut replacements = Vec::new();
    let debug_dir = std::env::var_os("FN64_RSP_LLE_DEBUG_DIR").map(std::path::PathBuf::from);
    const DEBUG_TAIL_STEPS: u64 = 1 << 16;
    const DEBUG_PC_RING: usize = 4096;
    let debug_initial = debug_dir.as_ref().map(|_| (dmem, imem, pc));
    let mut debug_pc_ring = std::collections::VecDeque::with_capacity(DEBUG_PC_RING);
    trace_rsp_dmem_words(&machine.dmem_logical(), overlays, pc);
    loop {
        let words: Vec<u32> = imem
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
            .collect();
        let chunk = if debug_dir.is_some() {
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
                replacements.push(PendingImemReplacement {
                    generation: imem_generation
                        .checked_add(overlays)
                        .expect("RSP IMEM generation evidence overflow"),
                    image: imem,
                });
                trace_rsp_dmem_words(&machine.dmem_logical(), overlays, pc);
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
    let committed_generation = with_host(|host| host.device_fabric.rsp_memory().imem_generation());
    assert_eq!(
        committed_generation,
        imem_generation
            .checked_add(overlays)
            .expect("RSP IMEM generation evidence overflow"),
        "RSP transactional IMEM replacement count diverged from the committed fabric generation"
    );

    let mut dp_full_sync = fn64_render::DpFullSyncStatus::NotReached;
    let mut dpc_observations = Vec::with_capacity(dp_submissions.len());
    let trace_limit = std::env::var("RSP_TRACE_DPC_WORDS").ok().map(|raw| {
        raw.parse::<usize>()
            .unwrap_or_else(|_| panic!("RSP_TRACE_DPC_WORDS must be decimal, got {raw:?}"))
    });
    // Consecutive END extensions are one hardware command stream. A 16-byte
    // command can straddle two 8-byte END writes, so decoding each submission
    // separately creates a false truncated-command trap.
    let mut index = 0;
    while index < dp_submissions.len() {
        let (start, end, xbus, words) = if dp_submissions[index].xbus {
            let start = dp_submissions[index].start;
            let mut end = dp_submissions[index].end;
            let mut stream = Vec::new();
            while index < dp_submissions.len() && dp_submissions[index].xbus {
                let submission = &dp_submissions[index];
                assert_eq!(
                    submission.payload.len(),
                    submission.end.wrapping_sub(submission.start) as usize,
                    "RSP XBUS DPC range [{:#010x}, {:#010x}) payload was not captured at submission time",
                    submission.start,
                    submission.end
                );
                stream.extend_from_slice(&submission.payload);
                end = submission.end;
                index += 1;
            }
            let words = stream
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().expect("four XBUS bytes")))
                .collect::<Vec<_>>();
            (start, end, true, words)
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
            let storage = unsafe { renderer_rdram_slice(rdram) };
            let words = storage[start as usize..end as usize]
                .chunks_exact(4)
                .map(|word| u32::from_ne_bytes(word.try_into().expect("four RDRAM bytes")))
                .collect::<Vec<_>>();
            (start, end, false, words)
        };
        if let Some(limit) = trace_limit {
            let traced = words.iter().copied().take(limit).collect::<Vec<_>>();
            eprintln!(
                "[fn64-rsp-dpc] range [{start:#010x}, {end:#010x}) xbus={xbus} words={traced:08x?}"
            );
        }
        let source = if xbus {
            fn64_runtime::DpcSubmissionSource::Dmem
        } else {
            fn64_runtime::DpcSubmissionSource::Rdram
        };
        let submission = with_host(|host| {
            host.device_fabric
                .request_dpc_submission(source, start, end)
        })
        .unwrap_or_else(|error| panic!("RSP DPC submission rejected: {error}"));
        let mut transaction = LiveDpcTransaction::new(submission);
        let (full_sync, observation) =
            unsafe { dispatch_captured_raw_rdp(rdram, &words, start, end, xbus, &mut transaction) };
        transaction.commit();
        dpc_observations.push(observation);
        if full_sync == fn64_render::DpFullSyncStatus::Reached {
            dp_full_sync = fn64_render::DpFullSyncStatus::Reached;
        }
    }

    let mut observations = Vec::new();
    let recognition_data = if recognize_graphics_microcode {
        Some(microcode_data.unwrap_or_else(|| {
            panic!(
                "RSP graphics task {:#010x} has no task-start microcode-data identity",
                task_addr.offset()
            )
        }))
    } else {
        None
    };
    if let Some(data) = recognition_data {
        observations.push(RspRdpObservationKind::MicrocodeRecognition {
            task_addr,
            imem_generation,
            text_sha256: imem_sha256(&initial_imem),
            data_addr: data.addr,
            data_size: data.size,
            data_sha256: data.sha256,
            family: identify_microcode_pair(&initial_imem, data, authoritative_family),
        });
    }
    for replacement in replacements {
        observations.push(RspRdpObservationKind::ImemReplacementCommitted {
            task_addr,
            imem_generation: replacement.generation,
            text_sha256: imem_sha256(&replacement.image),
        });
        if let Some(data) = recognition_data {
            observations.push(RspRdpObservationKind::MicrocodeRecognition {
                task_addr,
                imem_generation: replacement.generation,
                text_sha256: imem_sha256(&replacement.image),
                data_addr: data.addr,
                data_size: data.size,
                data_sha256: data.sha256,
                family: identify_microcode_pair(&replacement.image, data, authoritative_family),
            });
        }
    }
    observations.extend(dpc_observations);
    record_rsp_rdp_observations(observations);

    LleTaskResult {
        steps: total_steps.max(1),
        dp_full_sync,
    }
}

/// Execute the admitted rspboot until control first reaches bytes loaded by
/// an IMEM DMA, then commit its memory, PC, and SP-status effects before an
/// optimized graphics/audio backend represents the ucode phase.
///
/// The phase boundary comes from the public SGI RSP guide's task protocol:
/// rspboot consumes the task header, loads the selected ucode into IMEM, and
/// starts that ucode. HLE backends consume the public `OSTask` contract, while
/// a transactional LLE fallback receives the complete non-memory machine
/// snapshot alongside the committed memory image.
unsafe fn dispatch_hle_rspboot(rdram: *mut u8, task_addr: RdramAddr) -> HleBootResult {
    const BOOT_CHUNK_STEPS: u64 = 1 << 12;
    const MAX_BOOT_STEPS: u64 = 1 << 20;

    let (mut dmem, mut imem, status, mut pc, imem_generation, rdram_len, static_aliases) =
        with_host(|host| {
            let fabric = &host.device_fabric;
            (
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
                fabric.sp_status(),
                fabric.sp_pc(),
                fabric.rsp_memory().imem_generation(),
                host.runtime_rdram_len,
                host.sections.loaded_static_storage_ranges(),
            )
        });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP HLE rspboot has no registered process RDRAM allocation"
    );
    let (dma_ranges, _) = rsp_dma_storage_layout(rdram_len, static_aliases);
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.set_dma_rdram_ranges(dma_ranges);
    machine.load_dmem_logical(&dmem);
    machine.set_sp_status_raw(
        status & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
    );
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    let mut loaded_spans = Vec::new();
    let mut replacements = Vec::new();

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
        let imem_word_0 = u32::from_be_bytes(imem[0..4].try_into().expect("one IMEM word"));
        let task_boot = u32::from_be_bytes(dmem[0x0fc8..0x0fcc].try_into().expect("OSTask boot"));
        let task_boot_size =
            u32::from_be_bytes(dmem[0x0fcc..0x0fd0].try_into().expect("OSTask boot size"));
        let task_ucode = u32::from_be_bytes(dmem[0x0fd0..0x0fd4].try_into().expect("OSTask ucode"));
        assert!(
            total_steps <= MAX_BOOT_STEPS,
            "RSP HLE rspboot exceeded deterministic {MAX_BOOT_STEPS}-instruction bound at PC \
             {:#06x}; IMEM[0]={imem_word_0:#010x}, SP status={status:#010x}, admitted OSTask \
             boot={task_boot:#010x}/size={task_boot_size:#x}, ucode={task_ucode:#010x}",
            result.pc,
        );
        pc = result.pc;
        match result.reason {
            fn64_audio::rsp::RspExitReason::SwapOverlay => {
                loaded_spans.push(machine.pending_imem_dma_span());
                machine.complete_imem_dma(&mut imem);
                overlays = overlays
                    .checked_add(1)
                    .expect("RSP rspboot IMEM generation counter overflow");
                replacements.push(PendingImemReplacement {
                    generation: imem_generation
                        .checked_add(overlays)
                        .expect("RSP rspboot IMEM generation evidence overflow"),
                    image: imem,
                });
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
    let machine_state = machine.snapshot_state();
    assert!(
        dp_submissions.is_empty(),
        "RSP HLE rspboot submitted {} DPC range(s) before entering ucode",
        dp_submissions.len()
    );
    let rdram_writes = machine.take_rdram_writes();
    drop(machine);

    commit_rsp_rdram_writes(&rdram_writes);
    commit_rsp_memory_state(&dmem, &imem, overlays, pc, final_status);
    let committed_generation = with_host(|host| host.device_fabric.rsp_memory().imem_generation());
    assert_eq!(
        committed_generation,
        imem_generation
            .checked_add(overlays)
            .expect("RSP rspboot IMEM generation evidence overflow"),
        "RSP rspboot IMEM replacement count diverged from the committed fabric generation"
    );
    record_rsp_rdp_observations(
        replacements
            .into_iter()
            .map(
                |replacement| RspRdpObservationKind::ImemReplacementCommitted {
                    task_addr,
                    imem_generation: replacement.generation,
                    text_sha256: imem_sha256(&replacement.image),
                },
            )
            .collect(),
    );
    HleBootResult {
        steps: total_steps.max(1),
        task,
        machine_state,
    }
}

/// Own one fabric-issued DPC transaction across renderer execution. A backend
/// panic unwinds through this guard and cancels the exact token, so a rejected
/// range cannot remain busy or later advance CURRENT as if it had rendered.
struct LiveDpcTransaction {
    token: Option<u64>,
    acknowledgment: Option<fn64_runtime::DpcScheduledExecution>,
}

impl LiveDpcTransaction {
    fn new(submission: fn64_runtime::DpcSubmission) -> Self {
        with_host(|host| {
            assert_eq!(
                host.device_fabric.pending_dpc_submission(),
                Some(submission),
                "renderer received DPC transaction which the device fabric does not own"
            );
        });
        // Install cancellation ownership before any shared-ack construction.
        // If an admitted fabric range cannot form the compatibility quantum,
        // unwinding this guard restores the exact pre-admission DPC state.
        // The exact-token assertion stays first so a bad caller cannot make
        // this guard cancel some other transaction while unwinding.
        let mut transaction = Self {
            token: Some(submission.token),
            acknowledgment: None,
        };
        let source = submission.source;
        let start = fn64_runtime::DpcCursor::new(source, submission.start)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC start cursor: {error:?}"));
        let end = fn64_runtime::DpcCursor::new(source, submission.end)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC end cursor: {error:?}"));
        // Phase B deliberately assigns no device-time meaning to this one
        // compatibility quantum. Zero is an internal acknowledgment sentinel;
        // production still performs one synchronous atomic backend call.
        let sentinel = fn64_runtime::Cycles::new(0);
        let mut acknowledgment = fn64_runtime::DpcScheduledExecution::new(
            submission,
            sentinel,
            vec![fn64_runtime::DpcQuantumPlan {
                at: sentinel,
                id: fn64_runtime::DpcQuantumId::new(1),
                start,
                end,
            }],
        )
        .unwrap_or_else(|error| {
            panic!("fabric DPC transaction cannot form an atomic ack: {error:?}")
        });
        let fn64_runtime::DpcAdvance::Blocked { at, action } = acknowledgment
            .advance_to(sentinel)
            .unwrap_or_else(|error| panic!("arming atomic DPC acknowledgment: {error:?}"))
        else {
            panic!("atomic DPC acknowledgment passed its sole external-work barrier")
        };
        assert_eq!(at, sentinel);
        assert_eq!(action.transaction, acknowledgment.transaction());
        assert_eq!(action.start, start);
        assert_eq!(action.end, end);
        transaction.acknowledgment = Some(acknowledgment);
        transaction
    }

    /// Validate the compatibility backend's sole atomic completion before
    /// publishing its shadow memory. This carries no timing authority.
    fn validate_atomic_completion(&mut self) {
        let acknowledgment = self
            .acknowledgment
            .as_mut()
            .expect("atomic DPC transaction has no acknowledgment owner");
        let fn64_runtime::DpcScheduledPhase::AwaitingAck(request) = acknowledgment.phase() else {
            panic!("atomic DPC transaction lost its acknowledgment owner before validation")
        };
        acknowledgment
            .acknowledge(fn64_runtime::DpcBackendQuantumAck {
                transaction: request.transaction,
                quantum: request.quantum,
                committed_through: request.end,
                status: fn64_runtime::DpcBackendQuantumStatus::Complete,
            })
            .unwrap_or_else(|error| panic!("validating atomic DPC acknowledgment: {error:?}"));
        assert_eq!(
            acknowledgment.phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC acknowledgment did not consume its sole quantum"
        );
    }

    fn commit(mut self) {
        let token = *self
            .token
            .as_ref()
            .expect("DPC transaction committed twice");
        assert_eq!(
            self.acknowledgment
                .as_ref()
                .expect("atomic DPC transaction has no acknowledgment owner")
                .phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC transaction committed before acknowledgment validation"
        );
        with_host(|host| host.device_fabric.commit_dpc_submission(token))
            .unwrap_or_else(|error| panic!("committing rendered DPC transaction: {error}"));
        self.token.take();
    }
}

impl Drop for LiveDpcTransaction {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        with_host(|host| host.device_fabric.cancel_dpc_submission(token))
            .unwrap_or_else(|error| panic!("cancelling rejected DPC transaction: {error}"));
    }
}

/// Own the ABI side of an explicitly scheduled raw-DPC renderer transaction.
///
/// The renderer receives only a shadow image. Once it has been called, any
/// error or malformed success poisons the schedule: the backend may already
/// have consumed its private continuation, so retrying the same request would
/// duplicate work. A valid acknowledgment publishes schedule state, shadow
/// memory, continuation, and cumulative FullSync evidence as one transition.
#[cfg(test)]
struct ScheduledRawDpcTransaction {
    execution: fn64_runtime::DpcScheduledExecution,
    continuation: Option<fn64_render::RenderRawDpcContinuation>,
    full_sync: fn64_render::DpFullSyncStatus,
}

#[cfg(test)]
#[derive(Debug)]
enum ScheduledRawDpcError {
    Schedule(fn64_runtime::DpcScheduleError),
    Backend(fn64_render::RenderError),
    UnidentifiedFullSync,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScheduledRawDpcAdvance {
    Reached {
        at: fn64_runtime::Cycles,
    },
    Committed {
        at: fn64_runtime::Cycles,
        phase: fn64_runtime::DpcScheduledPhase,
        full_sync: fn64_render::DpFullSyncStatus,
    },
}

#[cfg(test)]
impl ScheduledRawDpcTransaction {
    fn new(execution: fn64_runtime::DpcScheduledExecution) -> Self {
        Self {
            execution,
            continuation: None,
            full_sync: fn64_render::DpFullSyncStatus::NotReached,
        }
    }

    fn advance_one(
        &mut self,
        requested: fn64_runtime::Cycles,
        backend: &mut dyn RenderBackend,
        rdram: &mut [u8],
        output_addr: u32,
    ) -> Result<ScheduledRawDpcAdvance, ScheduledRawDpcError> {
        let (at, action) = match self
            .execution
            .advance_to(requested)
            .map_err(ScheduledRawDpcError::Schedule)?
        {
            fn64_runtime::DpcAdvance::Reached { at } => {
                return Ok(ScheduledRawDpcAdvance::Reached { at });
            }
            fn64_runtime::DpcAdvance::Blocked { at, action } => (at, action),
        };

        let step = self.continuation.take().map_or(
            fn64_render::RawDpcStep::Start,
            fn64_render::RawDpcStep::Resume,
        );
        let mut shadow = rdram.to_vec();
        let ack = match backend.process_rdp_command_chunk(
            &mut shadow,
            fn64_render::RawDpcQuantum {
                request: action,
                output_addr,
            },
            step,
        ) {
            Ok(ack) => ack,
            Err(error) => {
                self.execution.poison();
                return Err(ScheduledRawDpcError::Backend(error));
            }
        };
        if ack.full_sync == fn64_render::DpFullSyncStatus::Unidentified {
            self.execution.poison();
            return Err(ScheduledRawDpcError::UnidentifiedFullSync);
        }
        let (status, continuation) = match ack.status {
            fn64_render::RawDpcChunkStatus::Continue(token) => {
                (fn64_runtime::DpcBackendQuantumStatus::Continue, Some(token))
            }
            fn64_render::RawDpcChunkStatus::Complete => {
                (fn64_runtime::DpcBackendQuantumStatus::Complete, None)
            }
        };
        if let Err(error) = self
            .execution
            .acknowledge(fn64_runtime::DpcBackendQuantumAck {
                transaction: ack.transaction,
                quantum: ack.quantum,
                committed_through: ack.committed_through,
                status,
            })
        {
            self.execution.poison();
            return Err(ScheduledRawDpcError::Schedule(error));
        }

        // No fallible work follows this point. These fields form the ABI's
        // publication boundary for the already-validated backend result.
        rdram.copy_from_slice(&shadow);
        self.continuation = continuation;
        if ack.full_sync == fn64_render::DpFullSyncStatus::Reached {
            self.full_sync = fn64_render::DpFullSyncStatus::Reached;
        }
        Ok(ScheduledRawDpcAdvance::Committed {
            at,
            phase: self.execution.phase(),
            full_sync: self.full_sync,
        })
    }

    fn phase(&self) -> fn64_runtime::DpcScheduledPhase {
        self.execution.phase()
    }

    fn cursor(&self) -> fn64_runtime::DpcCursor {
        self.execution.cursor()
    }

    fn continuation(&self) -> Option<fn64_render::RenderRawDpcContinuation> {
        self.continuation
    }

    fn full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.full_sync
    }
}

fn complete_committed_dpc(
    transaction: LiveDpcTransaction,
    full_sync: fn64_render::DpFullSyncStatus,
    observation: RspRdpObservationKind,
    operation: &'static str,
) {
    transaction.commit();
    record_rsp_rdp_observations(vec![observation]);
    if full_sync == fn64_render::DpFullSyncStatus::Reached {
        crate::pi::start_live_dp_full_sync()
            .unwrap_or_else(|error| panic!("{operation}: DP FullSync completion: {error}"));
    }
}

fn preflight_raw_dpc_completion(
    image: &[u8],
    start: u32,
    end: u32,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    let inspected = fn64_render::inspect_raw_rdp_full_sync(image, start, end)
        .unwrap_or_else(|error| panic!("{operation}: {error}"));
    if inspected == fn64_render::DpFullSyncStatus::Reached {
        // Interleaving closed here: a prior FullSync may remain pending while
        // the guest submits another DPC range. Device advancement and guest
        // callbacks cannot run during renderer dispatch, so observing an
        // empty slot here reserves it through the later synchronous commit;
        // observing an occupied slot rejects before backend or RDRAM mutation.
        with_host(|host| {
            host.device_fabric
                .preflight_dp_full_sync(fn64_runtime::Cycles::new(1))
        })
        .unwrap_or_else(|error| panic!("{operation}: DP FullSync completion: {error}"));
    }
    inspected
}

fn require_matching_raw_dpc_completion(
    inspected: fn64_render::DpFullSyncStatus,
    rendered: fn64_render::DpFullSyncStatus,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    assert_eq!(
        rendered, inspected,
        "{operation}: renderer FullSync evidence disagrees with the submitted raw RDP command stream"
    );
    rendered
}

/// Submit one fabric-owned CPU DPC transaction to the registered renderer.
/// DRAM reads the registered physical device; XBUS snapshots persistent DMEM
/// at the accepted END boundary. Renderer acceptance commits CURRENT before
/// FullSync can schedule DP completion; rejection cancels the token and
/// records no observation.
pub(crate) unsafe fn dispatch_dpc_submission(
    rdram: *mut u8,
    submission: fn64_runtime::DpcSubmission,
) {
    let start = submission.start;
    let end = submission.end;
    let mut transaction = LiveDpcTransaction::new(submission);
    let (full_sync, observation, operation) = match submission.source {
        fn64_runtime::DpcSubmissionSource::Rdram => {
            let (words, full_sync) = {
                let real = unsafe { renderer_rdram_slice(rdram) };
                let start_usize = start as usize;
                let end_usize = end as usize;
                assert!(
                    start_usize < end_usize
                        && start_usize.is_multiple_of(8)
                        && end_usize.is_multiple_of(8),
                    "DRAM DPC range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
                );
                assert!(
                    end_usize <= real.len(),
                    "DRAM DPC range end {end:#010x} exceeds registered RDRAM length {:#x}",
                    real.len()
                );
                let words = real[start_usize..end_usize]
                    .chunks_exact(4)
                    .map(|word| u32::from_ne_bytes(word.try_into().expect("four RDRAM bytes")))
                    .collect::<Vec<_>>();
                let mut image = real.to_vec();
                let inspected =
                    preflight_raw_dpc_completion(&image, start, end, "dispatch_raw_rdp");
                let result = with_render_backend("dispatch_raw_rdp", |backend| {
                    let status = backend.process_rdp_commands(
                        &mut image,
                        start,
                        end,
                        render_output_addr(),
                    )?;
                    Ok(RenderDispatchResult {
                        status,
                        dp_full_sync: backend.last_dp_full_sync(),
                    })
                });
                let rendered = require_committed_full_sync_evidence(result, "dispatch_raw_rdp");
                let full_sync =
                    require_matching_raw_dpc_completion(inspected, rendered, "dispatch_raw_rdp");
                transaction.validate_atomic_completion();
                real.copy_from_slice(&image);
                (words, full_sync)
            };
            (
                full_sync,
                dpc_observation(false, start, end, &words),
                "dispatch_raw_rdp",
            )
        }
        fn64_runtime::DpcSubmissionSource::Dmem => {
            let dmem = with_host(|host| {
                *host
                    .device_fabric
                    .rsp_memory()
                    .bank(fn64_runtime::RspMemoryBank::Dmem)
            });
            let words = dmem[start as usize..end as usize]
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().expect("four DMEM bytes")))
                .collect::<Vec<_>>();
            let (full_sync, observation) = unsafe {
                dispatch_captured_raw_rdp(rdram, &words, start, end, true, &mut transaction)
            };
            (full_sync, observation, "dispatch_raw_rdp_xbus")
        }
    };
    complete_committed_dpc(transaction, full_sync, observation, operation);
}

#[cfg_attr(not(test), allow(dead_code))]
unsafe fn dispatch_raw_rdp(rdram: *mut u8, start: u32, end: u32) {
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            start,
            end,
        )
    })
    .unwrap_or_else(|error| panic!("dispatch_raw_rdp: DPC submission rejected: {error}"));
    unsafe { dispatch_dpc_submission(rdram, submission) };
}

/// Submit an XBUS DPC range whose command bytes live in persistent RSP DMEM.
/// The renderer seam accepts an RDRAM image, so the command span is staged
/// after the real allocation in a private image. Only the original RDRAM
/// prefix is copied back after rendering; the staging range is unobservable.
#[cfg_attr(not(test), allow(dead_code))]
unsafe fn dispatch_raw_rdp_xbus(
    rdram: *mut u8,
    dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    start: u32,
    end: u32,
) {
    let start = start & 0x0fff;
    let end = end & 0x0fff;
    let start_usize = start as usize;
    let end_usize = end as usize;
    assert!(
        start < end && start.is_multiple_of(8) && end.is_multiple_of(8),
        "RSP XBUS DPC range [{start:#05x}, {end:#05x}) must be nonempty and 8-byte aligned"
    );
    assert!(
        end_usize <= dmem.len(),
        "RSP XBUS DPC range end {end:#05x} exceeds 4 KiB DMEM"
    );
    let words = dmem[start_usize..end_usize]
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four DMEM bytes")))
        .collect::<Vec<_>>();
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Dmem,
            start,
            end,
        )
    })
    .unwrap_or_else(|error| panic!("dispatch_raw_rdp_xbus: DPC submission rejected: {error}"));
    let mut transaction = LiveDpcTransaction::new(submission);
    let (full_sync, observation) =
        unsafe { dispatch_captured_raw_rdp(rdram, &words, start, end, true, &mut transaction) };
    complete_committed_dpc(transaction, full_sync, observation, "dispatch_raw_rdp_xbus");
}

/// Submit command words already captured at the DPC CMD_END boundary through
/// an address-disjoint staging range. RDP commands carry physical image and
/// texture addresses in their payload; their own fetch address is not part of
/// command semantics, so staging preserves those references while preventing
/// framebuffer writeback from corrupting a later captured FIFO range.
unsafe fn dispatch_captured_raw_rdp(
    rdram: *mut u8,
    words: &[u32],
    source_start: u32,
    source_end: u32,
    xbus: bool,
    transaction: &mut LiveDpcTransaction,
) -> (fn64_render::DpFullSyncStatus, RspRdpObservationKind) {
    assert!(
        !words.is_empty() && words.len().is_multiple_of(2),
        "captured RSP DPC submission must contain a nonempty whole number of 64-bit commands"
    );
    if !xbus {
        assert_eq!(
            source_end.checked_sub(source_start),
            Some(u32::try_from(words.len() * 4).expect("captured RSP DPC byte length exceeds u32")),
            "captured DRAM DPC source range does not match the captured command image"
        );
    }
    let real = unsafe { renderer_rdram_slice(rdram) };
    let physical_len = real.len();
    if xbus {
        if let Some(dir) = std::env::var_os("FN64_XBUS_STREAM_DUMP_DIR") {
            thread_local! {
                static XBUS_DUMP_INDEX: Cell<u64> = const { Cell::new(0) };
            }
            let index = XBUS_DUMP_INDEX.with(|cell| {
                let index = cell.get();
                cell.set(index + 1);
                index
            });
            let skip = std::env::var("FN64_XBUS_STREAM_DUMP_SKIP")
                .ok()
                .map_or(0, |raw| {
                    raw.parse::<u64>().unwrap_or_else(|_| {
                        panic!("FN64_XBUS_STREAM_DUMP_SKIP must be a u64, got {raw:?}")
                    })
                });
            if index >= skip && index < skip.saturating_add(16) {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir)
                    .unwrap_or_else(|error| panic!("FN64_XBUS_STREAM_DUMP_DIR {dir:?}: {error}"));
                let stream = words
                    .iter()
                    .flat_map(|word| word.to_be_bytes())
                    .collect::<Vec<_>>();
                let path = dir.join(format!("xbus-{index:04}.bin"));
                std::fs::write(&path, stream)
                    .unwrap_or_else(|error| panic!("writing XBUS stream dump {path:?}: {error}"));
                eprintln!(
                    "[fn64-abi] dumped XBUS stream #{index} ({} bytes) to {}",
                    words.len() * 4,
                    path.display()
                );
                let dump_rdram = std::env::var("FN64_XBUS_STREAM_DUMP_RDRAM")
                    .ok()
                    .map(|raw| {
                        raw.parse::<u64>().unwrap_or_else(|_| {
                            panic!("FN64_XBUS_STREAM_DUMP_RDRAM must be a u64, got {raw:?}")
                        })
                    });
                if dump_rdram == Some(index) {
                    let rdram_path = dir.join(format!("rdram-{index:04}.bin"));
                    std::fs::write(&rdram_path, &*real).unwrap_or_else(|error| {
                        panic!("writing RDRAM dump {rdram_path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-abi] dumped RDRAM image ({physical_len:#x} bytes) to {}",
                        rdram_path.display()
                    );
                }
            }
        }
    }
    let staging_start = physical_len;
    let staged_end = staging_start + words.len() * 4;
    assert!(
        staged_end <= 0x0100_0000,
        "captured RSP DPC staging range [{staging_start:#010x}, {staged_end:#010x}) exceeds the 24-bit RDP address space"
    );
    let mut image = vec![0u8; staged_end];
    image[..physical_len].copy_from_slice(real);
    for (word_index, value) in words.iter().copied().enumerate() {
        let offset = staging_start + word_index * 4;
        image[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    let inspected = preflight_raw_dpc_completion(
        &image,
        staging_start as u32,
        staged_end as u32,
        "dispatch_captured_raw_rdp",
    );
    let result = with_render_backend("dispatch_captured_raw_rdp", |backend| {
        let status = backend.process_rdp_commands(
            &mut image,
            staging_start as u32,
            staged_end as u32,
            render_output_addr(),
        )?;
        Ok(RenderDispatchResult {
            status,
            dp_full_sync: backend.last_dp_full_sync(),
        })
    });
    let rendered = require_committed_full_sync_evidence(result, "dispatch_captured_raw_rdp");
    let full_sync =
        require_matching_raw_dpc_completion(inspected, rendered, "dispatch_captured_raw_rdp");
    transaction.validate_atomic_completion();
    if xbus && std::env::var_os("FN64_XBUS_DIFF_TRACE").is_some() {
        let mut offset = 0usize;
        while offset < physical_len {
            if image[offset] != real[offset] {
                let start = offset;
                while offset < physical_len && image[offset] != real[offset] {
                    offset += 1;
                }
                eprintln!(
                    "[fn64-abi] XBUS diff: renderer changed rdram [{start:#010x}, {offset:#010x}) ({} bytes)",
                    offset - start
                );
            } else {
                offset += 1;
            }
        }
    }
    real.copy_from_slice(&image[..physical_len]);
    (
        full_sync,
        dpc_observation(xbus, source_start, source_end, words),
    )
}

fn require_committed_full_sync_evidence(
    result: RenderDispatchResult,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    match result.status {
        fn64_render::FrameStatus::Complete => match result.dp_full_sync {
            fn64_render::DpFullSyncStatus::Unidentified => {
                panic!("{operation}: renderer completed without identifying DP FullSync state")
            }
            status => status,
        },
        fn64_render::FrameStatus::Yielded => {
            panic!("{operation}: raw RDP submission cannot yield as an RSP task")
        }
        fn64_render::FrameStatus::NeedsLle { .. } => {
            panic!("{operation}: raw RDP submission cannot request RSP LLE fallback")
        }
    }
}

/// Dispatch a translated audio task (`M_AUDTASK`) once, at the point the RSP
/// is kicked. `o` is the OSTask's RDRAM offset, which the translated ucode's
/// FFI wrapper uses to seed RSP DMEM[0xFC0]. Missing execution policy and every
/// callback result other than RSP BREAK trap before completion or an
/// execution-qualified task trace can be produced.
///
/// Sample delivery happens later at `osAiSetNextBuffer_recomp`, the public AI
/// DMA boundary where the CPU names the actual finished PCM range. It cannot
/// happen here: OoT's live task has zero `OSTask.output_buff` fields and
/// selects output destinations through `A_SAVEBUFF` commands.
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
unsafe fn dispatch_audio_task(
    rdram: *mut u8,
    o: usize,
    header: &OsTaskHeader,
    callback: AudioUcodeFn,
) {
    debug_assert_eq!(header.task_type, M_AUDTASK);
    // Before PHASE_TIMING's thread_local initializer reads the NEW name: a run
    // that set only the old spelling must trap here, not silently proceed.
    assert_no_legacy_env_vars();
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    // Phase timing measures the translated audio ucode (the per-frame RSP
    // synth, currently unoptimized and the dominant per-swap cost).
    // The shared task dump happens before policy dispatch in
    // `osSpTaskStartGo_recomp`, so translated and LLE tasks expose the same
    // immutable input boundary.
    //
    // The CPU-side audio driver (AudioThread_UpdateImpl) and this task's
    // completion event still run, so the audio-reset handshake that unblocks
    // Play_Init still completes.
    // Safety: translated registration atomically pairs this callback with its
    // process-lifetime evidence identity. `o` is the admitted OSTask offset.
    let reason = if AUDIO_UCODE_TIMING.with(|c| c.get()) {
        let t = std::time::Instant::now();
        let reason = unsafe { callback(rdram, o as u32) };
        let ns = t.elapsed().as_nanos() as u64;
        AUDIO_UCODE_NS.with(|c| c.set(c.get() + ns));
        AUDIO_UCODE_CALLS.with(|c| c.set(c.get() + 1));
        reason
    } else {
        unsafe { callback(rdram, o as u32) }
    };
    if reason != 0 {
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Audio,
            "audio.translated-ucode.exit-reason",
            format!("task_offset={o:#010x} exit_reason={reason}"),
            Some(fn64_runtime::Cycles::new(crate::sim_time())),
            fn64_runtime::UnsupportedDisposition::LoudTrap,
        );
        panic!(
            "audio.translated-ucode.exit-reason: task {o:#010x} returned non-BREAK reason {reason}"
        );
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

/// Capture one immutable audio-task input before translated or LLE execution.
///
/// The output is private diagnostic material and remains outside git. Keeping
/// this at the common task-kick boundary makes a capture independent of the
/// installed execution policy, which is required for later LLE/HLE replay.
///
/// # Safety
/// `rdram` must cover the registered process RDRAM length, and `task_offset`
/// must name the admitted `OSTask` inside it.
unsafe fn maybe_dump_audio_task_input(rdram: *mut u8, task_offset: usize) {
    let Some(path) = std::env::var_os("FN64_DUMP_AUDIO_TASK") else {
        return;
    };
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
            let (registered_rdram, registered_len) =
                with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
            assert_eq!(
                registered_rdram, rdram,
                "FN64_DUMP_AUDIO_TASK task pointer does not match registered process RDRAM"
            );
            let rdram_len = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
            assert!(
                registered_len >= rdram_len,
                "FN64_DUMP_AUDIO_TASK registered process RDRAM is {registered_len:#x} bytes; \
                 physical capture requires {rdram_len:#x}"
            );
            assert!(
                task_offset
                    .checked_add(core::mem::size_of::<OsTaskHeader>())
                    .is_some_and(|end| end <= rdram_len),
                "FN64_DUMP_AUDIO_TASK task offset {task_offset:#x} is outside physical RDRAM"
            );
            let bytes = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
            let path = std::path::Path::new(&path);
            std::fs::write(path, bytes).unwrap_or_else(|error| {
                panic!("write private audio task capture {path:?}: {error}")
            });
            let meta = format!(
                "task_offset={task_offset}\nrdram_len={rdram_len}\ntask_index={}\n",
                state.seen
            );
            let meta_path = path.with_extension("meta");
            std::fs::write(&meta_path, meta).unwrap_or_else(|error| {
                panic!("write private audio task metadata {meta_path:?}: {error}")
            });
            eprintln!(
                "[fn64-abi] dumped audio task #{} input ({rdram_len} B, task_offset={task_offset}) to {path:?}",
                state.seen
            );
            state.dumped = true;
        }
        dump.set(state);
    });
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
        with_host(|host| {
            let retire = host
                .rsp_task_lineages
                .get(&task_addr.offset())
                .is_some_and(|lineage| lineage.phase == RspTaskLineagePhase::Running);
            if retire {
                host.rsp_task_lineages.remove(&task_addr.offset());
            }
        });
        ctx.r2 = 0;
        return;
    }

    let header = unsafe { read_os_task_header(rdram, o) };
    let lineage = with_host(|host| host.rsp_task_lineages.get(&task_addr.offset()).copied())
        .unwrap_or_else(|| {
            panic!(
                "osSpTaskYielded_recomp: yielded RSP task {:#010x} has no started task lineage",
                task_addr.offset()
            )
        });
    assert_eq!(
        lineage.phase,
        RspTaskLineagePhase::Running,
        "osSpTaskYielded_recomp: yielded RSP task {:#010x} cannot authorize a resume from phase {:?}",
        task_addr.offset(),
        lineage.phase,
    );
    let expected = if header.flags & fn64_runtime::OS_TASK_YIELDED != 0 {
        lineage.yielded_header()
    } else {
        lineage.original_header
    };
    assert_eq!(
        header,
        expected,
        "osSpTaskYielded_recomp: task {:#010x} changed after its loaded task admission",
        task_addr.offset()
    );
    unsafe {
        write_os_task_word(rdram, o, 0x04, header.flags | fn64_runtime::OS_TASK_YIELDED);
        write_os_task_word(rdram, o, 0x18, header.yield_data_ptr);
        write_os_task_word(rdram, o, 0x1c, header.yield_data_size);
    }
    with_host(|host| {
        let lineage = host
            .rsp_task_lineages
            .get_mut(&task_addr.offset())
            .expect("yielded task lineage was validated above");
        assert_eq!(
            lineage.phase,
            RspTaskLineagePhase::Running,
            "osSpTaskYielded_recomp: yielded RSP task {:#010x} changed lineage phase during its public rewrite",
            task_addr.offset()
        );
        lineage.phase = RspTaskLineagePhase::ResumeAuthorized;
    });
    ctx.r2 = u64::from(fn64_runtime::OS_TASK_YIELDED);
}

thread_local! {
    /// The single registered graphics backend, if the shell/harness has
    /// called `set_render_backend`. `RefCell` (not `Cell`, unlike
    /// `AUDIO_UCODE_FN`) because a `Box<dyn RenderBackend>` is not `Copy`
    /// and needs `&mut` access across calls to drive its own internal
    /// state (`create`/`process_task`/`present`).
    static RENDER_BACKEND: RefCell<Option<Box<dyn RenderBackend>>> = const { RefCell::new(None) };
    /// Graphics microcode execution is a host policy, not a renderer
    /// capability guess. Compatibility callers retain the optimized default;
    /// accuracy/release harnesses opt into LLE at registration.
    static GRAPHICS_TASK_EXECUTION_POLICY: Cell<GraphicsTaskExecutionPolicy> =
        const { Cell::new(GraphicsTaskExecutionPolicy::HleOptimized) };
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
    /// The scheduler owns only this opaque token and immutable task identity;
    /// renderer-local stacks/state remain behind `RenderBackend`.
    static HLE_RENDER_CONTINUATION: RefCell<Option<HleRenderContinuation>> = const { RefCell::new(None) };

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
    static AUDIO_DIGEST_CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HleRenderContinuationPhase {
    Running,
    Suspended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HleRenderContinuation {
    phase: HleRenderContinuationPhase,
    token: fn64_render::RenderTaskContinuation,
    task_addr: RdramAddr,
    task: OsTaskHeader,
    rdram: usize,
    output_addr: u32,
    dp_full_sync: fn64_render::DpFullSyncStatus,
    completion_latency: u64,
}

fn merge_dp_full_sync(
    prior: fn64_render::DpFullSyncStatus,
    next: fn64_render::DpFullSyncStatus,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    match (prior, next) {
        (_, fn64_render::DpFullSyncStatus::Unidentified) => {
            panic!("{operation}: resumable renderer chunk did not identify DP FullSync state")
        }
        (fn64_render::DpFullSyncStatus::Reached, _)
        | (_, fn64_render::DpFullSyncStatus::Reached) => fn64_render::DpFullSyncStatus::Reached,
        _ => fn64_render::DpFullSyncStatus::NotReached,
    }
}

fn retain_running_hle_continuation(
    mut pending: HleRenderContinuation,
    result: RenderChunkDispatchResult,
    operation: &'static str,
) {
    let fn64_render::RenderTaskChunkStatus::Continue(token) = result.status else {
        panic!("{operation}: internal continuation retention requires Continue")
    };
    assert_eq!(
        result.chunking,
        fn64_render::RenderTaskChunking::Resumable,
        "{operation}: atomic backend returned a resumable continuation"
    );
    pending.token = token;
    pending.phase = HleRenderContinuationPhase::Running;
    pending.dp_full_sync = merge_dp_full_sync(pending.dp_full_sync, result.dp_full_sync, operation);
    HLE_RENDER_CONTINUATION.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "{operation}: renderer continuation ownership is already occupied"
        );
        cell.replace(Some(pending));
    });
}

/// Advance at most one committed HLE renderer chunk at a host scheduling
/// boundary. Returning to the host after each `Continue` is what gives guest
/// code a real interval in which to issue SIG0.
pub(crate) fn advance_hle_render_task() {
    let Some(mut pending) = HLE_RENDER_CONTINUATION.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    if pending.phase == HleRenderContinuationPhase::Suspended {
        HLE_RENDER_CONTINUATION.with(|cell| cell.replace(Some(pending)));
        return;
    }

    // Interleaving closed here: renderer chunk A has committed and returned
    // its owned continuation; guest CPU execution may then set SIG0 before
    // host boundary B. B must observe SIG0 before consuming the token, or the
    // next chunk would run past the sole representable yield boundary.
    if crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELD != 0 {
        pending.phase = HleRenderContinuationPhase::Suspended;
        HLE_RENDER_CONTINUATION.with(|cell| cell.replace(Some(pending)));
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
        crate::pi::finish_live_rcp_task(
            rcp_completion_plan(pending.dp_full_sync, "chunk-boundary HLE yield"),
            pending.completion_latency,
        )
        .unwrap_or_else(|error| panic!("chunk-boundary HLE yield completion: {error}"));
        return;
    }

    let result = unsafe {
        dispatch_gfx_task_chunk(
            pending.rdram as *mut u8,
            &pending.task,
            fn64_render::RenderTaskStep::Resume(pending.token),
            pending.output_addr,
        )
    };
    match result.status {
        fn64_render::RenderTaskChunkStatus::Continue(_) => {
            pending.completion_latency = 1;
            retain_running_hle_continuation(pending, result, "resume HLE chunk")
        }
        fn64_render::RenderTaskChunkStatus::Complete => {
            let full_sync = merge_dp_full_sync(
                pending.dp_full_sync,
                result.dp_full_sync,
                "complete HLE chunk",
            );
            crate::pi::finish_live_rcp_task(
                rcp_completion_plan(full_sync, "complete HLE chunk"),
                1,
            )
            .unwrap_or_else(|error| panic!("complete HLE chunk completion: {error}"));
            retire_running_rsp_task_lineage(pending.task_addr, "complete HLE chunk");
        }
        fn64_render::RenderTaskChunkStatus::Yielded => {
            assert_ne!(
                result.dp_full_sync,
                fn64_render::DpFullSyncStatus::Reached,
                "cooperatively yielded HLE chunk cannot also complete DP FullSync"
            );
            crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
            crate::pi::finish_live_rcp_task(fn64_runtime::RcpTaskCompletionPlan::SpOnly, 1)
                .unwrap_or_else(|error| panic!("cooperative HLE chunk completion: {error}"));
        }
        fn64_render::RenderTaskChunkStatus::NeedsLle { .. } => {
            panic!("resumed HLE continuation requested LLE after committing an earlier chunk")
        }
    }
}

pub(crate) fn hle_render_needs_progress() -> bool {
    HLE_RENDER_CONTINUATION.with(|cell| {
        cell.borrow()
            .is_some_and(|pending| pending.phase == HleRenderContinuationPhase::Running)
    })
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

/// Begin or end deterministic capture of guest PCM at the AI boundary.
/// Enabling clears an earlier capture; disabling releases its storage.
pub fn set_audio_digest_capture(enabled: bool) {
    AUDIO_DIGEST_CAPTURE.with(|cell| {
        *cell.borrow_mut() = enabled.then(Vec::new);
    });
}

/// Copy the pre-resample stereo s16le stream accumulated since capture began.
/// `None` distinguishes a host that did not request audio evidence from a
/// requested, exercised capture that legitimately contains zero bytes.
pub fn copy_audio_digest_bytes() -> Option<Vec<u8>> {
    AUDIO_DIGEST_CAPTURE.with(|cell| cell.borrow().clone())
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
/// `M_GFXTASK` submissions to, and the RDRAM buffer length it may safely
/// read. The host must separately call [`crate::register_process_rdram`] (or
/// [`crate::boot_thread0`], which performs that registration) before the
/// first VI retrace. `rdram_len` must match that allocation's size; a mismatch
/// is a caller bug. Mirrors
/// `set_audio_ucode_fn`'s "the shell wires this once at startup" shape,
/// generalized to a trait object since a graphics backend is stateful
/// (unlike a single ucode function pointer). This compatibility entry point
/// intentionally selects [`GraphicsTaskExecutionPolicy::HleOptimized`]; a
/// caller making an accuracy claim must use [`set_render_backend_with_policy`]
/// and opt in explicitly.
pub fn set_render_backend(backend: Box<dyn RenderBackend>, rdram_len: usize) {
    set_render_backend_with_policy(
        backend,
        rdram_len,
        GraphicsTaskExecutionPolicy::HleOptimized,
    );
}

/// Register a graphics backend and choose how graphics microcode executes.
///
/// Use [`GraphicsTaskExecutionPolicy::LleAccuracy`] for release/parity evidence
/// that must execute the ROM's loaded RSP program rather than an HLE model.
/// [`set_render_backend`] intentionally preserves the historical optimized
/// policy for interactive shells and callers whose performance contract has
/// not opted into LLE.
pub fn set_render_backend_with_policy(
    backend: Box<dyn RenderBackend>,
    rdram_len: usize,
    policy: GraphicsTaskExecutionPolicy,
) {
    HLE_RENDER_CONTINUATION.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "set_render_backend_with_policy: cannot replace a backend that owns an HLE continuation"
        );
    });
    RENDER_BACKEND.with(|cell| cell.replace(Some(backend)));
    RDRAM_LEN.with(|cell| cell.set(rdram_len));
    GRAPHICS_TASK_EXECUTION_POLICY.with(|cell| cell.set(policy));
}

/// The most recent registered backend's `process_task` error, if the last
/// `M_GFXTASK` dispatch failed. `None` if no gfx task has run yet, the last
/// one succeeded, or no backend is registered at all. A test/harness
/// observability hook -- see `set_render_backend`'s doc comment.
pub fn last_render_error() -> Option<String> {
    RENDER_LAST_ERROR.with(|cell| cell.borrow().clone())
}

/// Drop registered host backends at the terminal process boundary while the
/// caller's RDRAM allocation is still live.
///
/// A bounded host run may stop at the committed boundary represented by an
/// HLE continuation. Process exit abandons that token before dropping the
/// renderer that owns its continuation state; it must not resume guest or
/// renderer work merely to reach a more convenient teardown point.
pub(crate) fn drop_backends_for_process_exit() {
    HLE_RENDER_CONTINUATION.with(|cell| cell.borrow_mut().take());
    let render_backend = RENDER_BACKEND.with(|cell| cell.borrow_mut().take());
    let audio_backend = AUDIO_BACKEND.with(|cell| cell.borrow_mut().take());
    let audio_stream_dump = AUDIO_PCM_STREAM_DUMP.with(|cell| cell.borrow_mut().take());
    RDRAM_LEN.with(|cell| cell.set(0));
    AUDIO_RDRAM_LEN.with(|cell| cell.set(0));
    drop(audio_stream_dump);
    drop(audio_backend);
    drop(render_backend);
}

/// Capture the registered backend's most recent completed presentation for a
/// fixed-cycle release report. This deliberately goes through the owned
/// `RenderBackend` seam: a host neither downcasts the backend nor reaches into
/// RT64 after registration. Unsupported capture and a missing presentation
/// remain typed errors.
pub fn capture_render_release_frame(
) -> Result<fn64_render::RenderReleaseCapture, fn64_render::RenderError> {
    if HLE_RENDER_CONTINUATION.with(|cell| cell.borrow().is_some()) {
        return Err(fn64_render::RenderError::Backend {
            backend: "render-release-capture",
            reason: "an HLE renderer continuation is still live".into(),
        });
    }
    RENDER_BACKEND.with(|cell| {
        let mut registered = cell.borrow_mut();
        let backend = registered
            .as_mut()
            .ok_or(fn64_render::RenderError::NotReady(
                "capture_render_release_frame: no render backend registered",
            ))?;
        let result = backend.release_capture();
        RENDER_LAST_ERROR.with(|last| {
            last.replace(result.as_ref().err().map(ToString::to_string));
        });
        result
    })
}

/// Snapshot the concrete registered backend and graphics execution policy.
/// The backend self-reports through the trait object; callers cannot attach a
/// separate label after registration.
pub fn render_environment_evidence_snapshot() -> RenderEnvironmentEvidenceSnapshot {
    assert!(
        HLE_RENDER_CONTINUATION.with(|cell| cell.borrow().is_none()),
        "render environment evidence cannot omit a live HLE renderer continuation"
    );
    let backend = RENDER_BACKEND.with(|cell| {
        cell.borrow().as_ref().map_or(
            fn64_render::RenderBackendEvidence::Unidentified,
            |backend| backend.release_environment(),
        )
    });
    RenderEnvironmentEvidenceSnapshot {
        backend,
        execution_policy: GRAPHICS_TASK_EXECUTION_POLICY.with(Cell::get),
    }
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

fn loaded_rsp_task_from_header(task_addr: RdramAddr, header: OsTaskHeader) -> LoadedRspTask {
    let resumed_data_identity = if header.flags & fn64_runtime::OS_TASK_YIELDED != 0 {
        let lineage = with_host(|host| host.rsp_task_lineages.get(&task_addr.offset()).copied())
            .unwrap_or_else(|| {
                panic!(
                    "osSpTaskLoad_recomp: yielded RSP task {:#010x} has no retained task lineage",
                    task_addr.offset()
                )
            });
        assert_eq!(
            lineage.phase,
            RspTaskLineagePhase::ResumeAuthorized,
            "osSpTaskLoad_recomp: yielded RSP task {:#010x} has no unused resume authorization (phase {:?})",
            task_addr.offset(),
            lineage.phase,
        );
        assert_eq!(
            header,
            lineage.yielded_header(),
            "osSpTaskLoad_recomp: yielded RSP task {:#010x} does not match its retained task lineage",
            task_addr.offset()
        );
        lineage.data_identity
    } else {
        None
    };
    LoadedRspTask {
        task_addr,
        header,
        resumed_data_identity,
    }
}

fn retain_loaded_rsp_task(loaded: LoadedRspTask) {
    with_host(|host| {
        if let Some(replaced) = host.loaded_rsp_task.take() {
            let remove_replaced_lineage = host
                .rsp_task_lineages
                .get(&replaced.task_addr.offset())
                .is_some_and(|lineage| lineage.phase == RspTaskLineagePhase::ResumeLoaded);
            if remove_replaced_lineage {
                host.rsp_task_lineages.remove(&replaced.task_addr.offset());
            }
        }
        if loaded.header.flags & fn64_runtime::OS_TASK_YIELDED == 0 {
            host.rsp_task_lineages.remove(&loaded.task_addr.offset());
        } else {
            let lineage = host
                .rsp_task_lineages
                .get_mut(&loaded.task_addr.offset())
                .expect("yielded loaded task lineage was validated before SP admission");
            assert_eq!(
                lineage.phase,
                RspTaskLineagePhase::ResumeAuthorized,
                "osSpTaskLoad_recomp: yielded RSP task {:#010x} consumed a stale resume authorization",
                loaded.task_addr.offset()
            );
            lineage.phase = RspTaskLineagePhase::ResumeLoaded;
        }
        // RSP has one admitted task image. A later successful Load replaces
        // that image and therefore replaces the sole unconsumed token too.
        host.loaded_rsp_task = Some(loaded);
    });
}

fn take_loaded_rsp_task(task_addr: RdramAddr) -> LoadedRspTask {
    with_host(|host| {
        let loaded = host.loaded_rsp_task.as_ref().unwrap_or_else(|| {
            panic!(
                "osSpTaskStartGo_recomp: task {:#010x} has no unconsumed osSpTaskLoad admission",
                task_addr.offset()
            )
        });
        assert_eq!(
            loaded.task_addr, task_addr,
            "osSpTaskStartGo_recomp: task {:#010x} does not own the loaded RSP task token for {:#010x}",
            task_addr.offset(),
            loaded.task_addr.offset()
        );
        host.loaded_rsp_task
            .take()
            .expect("loaded RSP task was present above")
    })
}

fn retain_started_rsp_task_lineage(
    loaded: LoadedRspTask,
    data_identity: Option<TaskMicrocodeDataIdentity>,
) {
    with_host(|host| {
        let running = host
            .rsp_task_lineages
            .iter()
            .find_map(|(&task_offset, lineage)| {
                (lineage.phase == RspTaskLineagePhase::Running).then_some(task_offset)
            });
        assert!(
            running.is_none(),
            "osSpTaskStartGo_recomp: task {:#010x} cannot start while task {:#010x} owns the Running RSP lineage",
            loaded.task_addr.offset(),
            running.unwrap_or_default(),
        );
        if loaded.header.flags & fn64_runtime::OS_TASK_YIELDED != 0 {
            let lineage = host
                .rsp_task_lineages
                .get_mut(&loaded.task_addr.offset())
                .unwrap_or_else(|| {
                    panic!(
                        "osSpTaskStartGo_recomp: yielded RSP task {:#010x} lost its retained task lineage",
                        loaded.task_addr.offset()
                    )
                });
            assert_eq!(
                lineage.data_identity, data_identity,
                "osSpTaskStartGo_recomp: yielded RSP task {:#010x} changed its original microcode-data identity",
                loaded.task_addr.offset()
            );
            assert_eq!(
                lineage.phase,
                RspTaskLineagePhase::ResumeLoaded,
                "osSpTaskStartGo_recomp: yielded RSP task {:#010x} does not own a loaded resume token",
                loaded.task_addr.offset()
            );
            lineage.phase = RspTaskLineagePhase::Running;
        } else {
            let previous = host.rsp_task_lineages.insert(
                loaded.task_addr.offset(),
                RspTaskLineage {
                    original_header: loaded.header,
                    data_identity,
                    phase: RspTaskLineagePhase::Running,
                },
            );
            assert!(
                previous.is_none(),
                "osSpTaskStartGo_recomp: fresh RSP task {:#010x} unexpectedly retained an older lineage",
                loaded.task_addr.offset()
            );
        }
    });
}

fn retire_running_rsp_task_lineage(task_addr: RdramAddr, operation: &'static str) {
    with_host(|host| {
        let lineage = host
            .rsp_task_lineages
            .get(&task_addr.offset())
            .unwrap_or_else(|| {
                panic!(
                    "{operation}: task {:#010x} has no Running RSP lineage to retire",
                    task_addr.offset()
                )
            });
        assert_eq!(
            lineage.phase,
            RspTaskLineagePhase::Running,
            "{operation}: task {:#010x} cannot retire RSP lineage phase {:?}",
            task_addr.offset(),
            lineage.phase,
        );
        host.rsp_task_lineages.remove(&task_addr.offset());
    });
}

fn retire_rsp_task_lineage_after_synchronous_result(task_addr: RdramAddr, operation: &'static str) {
    if crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED == 0 {
        retire_running_rsp_task_lineage(task_addr, operation);
    }
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
    let loaded = loaded_rsp_task_from_header(task_addr, header);
    // A newly admitted task must not inherit either half of the preceding
    // task's yield handshake. In particular, stale SIG1 would make the next
    // `osSpTaskYielded` rewrite a task that actually completed normally.
    crate::pi::write_live_sp_status(fn64_runtime::SP_CLR_YIELD | fn64_runtime::SP_CLR_YIELDED);
    let boot_size = aligned_sp_image_size(header.ucode_boot_size).unwrap_or_else(|| {
        panic!(
            "osSpTaskLoad_recomp: invalid rspboot size {:#x}",
            header.ucode_boot_size
        )
    }) as usize;
    let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
    let memory = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
    let boot = with_host(|host| {
        let image = host.rsp_boot_images.entry(boot_addr).or_default();
        for offset in image.len()..boot_size {
            image.push(unsafe {
                memory.read_u8(RdramAddr::from_offset(
                    boot_addr
                        .checked_add(offset as u32)
                        .expect("rspboot cache address overflow"),
                ))
            });
        }
        image[..boot_size].to_vec()
    });
    unsafe { crate::pi::admit_live_sp_task(rdram, task_addr, header, &boot) }
        .unwrap_or_else(|error| panic!("osSpTaskLoad_recomp: {error}"));
    retain_loaded_rsp_task(loaded);
    if std::env::var_os("RSP_TRACE_TASK").is_some() {
        let memory = unsafe { fn64_runtime::RdramPtr::from_storage_ptr(rdram) };
        let boot_word = unsafe {
            memory.read_u32(fn64_runtime::RdramAddr::from_gpr(u64::from(
                header.ucode_boot,
            )))
        };
        let ucode_word =
            unsafe { memory.read_u32(fn64_runtime::RdramAddr::from_gpr(u64::from(header.ucode))) };
        let imem_words = with_host(|host| {
            let imem = host
                .device_fabric
                .rsp_memory()
                .bank(fn64_runtime::RspMemoryBank::Imem);
            [
                u32::from_be_bytes(imem[0..4].try_into().expect("one IMEM word")),
                u32::from_be_bytes(imem[4..8].try_into().expect("one IMEM word")),
            ]
        });
        eprintln!(
            "[fn64-rsp-task] admit task={:#010x} type={} boot={:#010x}/size={:#x} \
             word={boot_word:#010x} ucode={:#010x}/size={:#x} word={ucode_word:#010x} \
             IMEM[0..8]={imem_words:08x?}",
            task_addr.offset(),
            header.task_type,
            header.ucode_boot,
            header.ucode_boot_size,
            header.ucode,
            header.ucode_size,
        );
    }
    with_executor(|exec| exec.admit_task(header));
}

/// `osSpTaskStartGo(OSSpTask *sptask)` -- the actual RSP-kickoff half of
/// the pair `osSpTaskLoad_recomp` above bookkeeps. `a0` = `ctx->r4` is the
/// `OSTask*` (same pointer shape `osSpTaskLoad`/`osSpTaskYielded` read).
///
/// This crate classifies boot-overlay versus direct-IMEM admission. It either
/// executes rspboot through its IMEM-DMA handoff or enters the already-loaded
/// image at PC zero, then runs the selected task effect (audio translated/LLE
/// execution, or the graphics policy's HLE/LLE ucode phase) synchronously
/// while the shim owns the guest. Its externally visible completion is
/// scheduled separately, with measured pre-ucode work included in SP latency.
/// What a real
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
    let loaded = take_loaded_rsp_task(task_addr);
    let o = task_addr.offset() as usize;
    let header = loaded.header;
    let is_gfx = header.task_type == M_GFXTASK;
    let audio_policy = (header.task_type == M_AUDTASK)
        .then(|| require_audio_task_execution_policy(task_addr, &header));
    if header.task_type == M_AUDTASK {
        unsafe { maybe_dump_audio_task_input(rdram, o) };
    }
    let recognition_header = if header.flags & fn64_runtime::OS_TASK_YIELDED != 0 {
        with_host(|host| {
            host.rsp_task_lineages
                .get(&task_addr.offset())
                .unwrap_or_else(|| {
                    panic!(
                        "osSpTaskStartGo_recomp: yielded RSP task {:#010x} lost its original microcode identity header",
                        task_addr.offset()
                    )
                })
                .original_header
        })
    } else {
        header
    };
    let authoritative_microcode_family = if is_gfx {
        unsafe { classify_task_microcode_family(rdram, &recognition_header) }
    } else {
        None
    };
    // Hash fresh task data at the actual kickoff boundary using only the
    // address/size admitted by Load. A yielded token owns the original
    // identity directly and never hashes its rewritten yield buffer.
    let initial_microcode_data = if is_gfx {
        Some(loaded.resumed_data_identity.unwrap_or_else(|| unsafe {
            task_microcode_data_identity(
                rdram,
                task_addr,
                header.ucode_data,
                header.ucode_data_size,
            )
        }))
    } else {
        None
    };
    retain_started_rsp_task_lineage(loaded, initial_microcode_data);
    with_executor(|exec| exec.start_task(header));

    // Kicking the RSP is where the selected task effect runs, so the work
    // happens here -- this is the path OoT uses (Load then
    // StartGo, never the yield path) for BOTH its gfx and its audio tasks.
    // Dispatch before scheduling completion so the work is done by the time
    // the scheduler is woken. A graphics task rasterizes; an audio task
    // runs its registered ucode + forwards samples (previously dispatched only
    // from the never-taken yield path -- same latent bug the gfx path hit).
    let mut hle_entry = if is_gfx || header.task_type == M_AUDTASK {
        Some(match admitted_task_image_shape(&header) {
            AdmittedTaskImageShape::BootOverlay => {
                let boot = unsafe { dispatch_hle_rspboot(rdram, task_addr) };
                assert_eq!(
                    boot.task.task_type, header.task_type,
                    "RSP rspboot changed OSTask type from {} to {}; HLE selection is no longer valid",
                    header.task_type, boot.task.task_type
                );
                AdmittedHleEntry::BootOverlay(Box::new(boot))
            }
            // osSpTaskLoad has already installed this complete image at IMEM
            // zero and reset PC to zero. Executing it as rspboot would consume
            // the ucode's legitimate terminal BREAK while waiting for an IMEM
            // DMA generation that this admission shape does not require.
            AdmittedTaskImageShape::DirectImem => AdmittedHleEntry::DirectImem { task: header },
        })
    } else {
        None
    };
    let hle_header = hle_entry.as_ref().map_or(header, AdmittedHleEntry::task);
    let dp_full_sync = if is_gfx
        && GRAPHICS_TASK_EXECUTION_POLICY.with(Cell::get)
            == GraphicsTaskExecutionPolicy::LleAccuracy
    {
        let entry = hle_entry
            .take()
            .expect("gfx accuracy LLE requires an admitted HLE entry");
        let pre_ucode_steps = entry.pre_ucode_steps();
        let microcode_data = initial_microcode_data
            .expect("gfx accuracy LLE requires admitted microcode-data identity");
        let lle = unsafe {
            dispatch_lle_task(
                rdram,
                task_addr,
                true,
                entry.into_lle_machine_state(),
                Some(microcode_data),
                authoritative_microcode_family,
            )
        };
        crate::pi::start_live_rcp_task_with_latency(
            rcp_completion_plan(lle.dp_full_sync, "gfx accuracy LLE"),
            pre_ucode_steps.saturating_add(lle.steps),
        )
        .unwrap_or_else(|error| {
            panic!("osSpTaskStartGo_recomp gfx accuracy LLE completion: {error}")
        });
        retire_rsp_task_lineage_after_synchronous_result(task_addr, "gfx accuracy LLE");
        return;
    } else if is_gfx {
        let retained = HLE_RENDER_CONTINUATION.with(|cell| cell.borrow_mut().take());
        let (step, output_addr, prior_full_sync, resumed_internal) = match retained {
            Some(pending) => {
                assert_eq!(
                    pending.phase,
                    HleRenderContinuationPhase::Suspended,
                    "osSpTaskStartGo_recomp: cannot start while an HLE continuation is running"
                );
                assert_eq!(
                    pending.task_addr, task_addr,
                    "osSpTaskStartGo_recomp: yielded task address does not own the retained renderer continuation"
                );
                assert_ne!(
                    hle_header.flags & fn64_runtime::OS_TASK_YIELDED,
                    0,
                    "osSpTaskStartGo_recomp: retained renderer continuation requires OS_TASK_YIELDED"
                );
                assert_eq!(
                    (hle_header.ucode_data, hle_header.ucode_data_size),
                    (pending.task.yield_data_ptr, pending.task.yield_data_size),
                    "osSpTaskStartGo_recomp: yielded task buffer does not match retained continuation owner"
                );
                (
                    fn64_render::RenderTaskStep::Resume(pending.token),
                    pending.output_addr,
                    pending.dp_full_sync,
                    true,
                )
            }
            None => (
                fn64_render::RenderTaskStep::Start,
                render_output_addr(),
                fn64_render::DpFullSyncStatus::NotReached,
                false,
            ),
        };
        let chunk_completion_latency = hle_entry
            .as_ref()
            .expect("gfx HLE chunk requires an admitted entry")
            .pre_ucode_steps()
            .saturating_add(1);
        let result = unsafe { dispatch_gfx_task_chunk(rdram, &hle_header, step, output_addr) };
        match result.status {
            fn64_render::RenderTaskChunkStatus::Complete => {
                merge_dp_full_sync(prior_full_sync, result.dp_full_sync, "complete HLE task")
            }
            fn64_render::RenderTaskChunkStatus::Continue(token) => {
                crate::pi::begin_live_rcp_task().unwrap_or_else(|error| {
                    panic!("osSpTaskStartGo_recomp chunked HLE admission: {error}")
                });
                retain_running_hle_continuation(
                    HleRenderContinuation {
                        phase: HleRenderContinuationPhase::Running,
                        token,
                        task_addr,
                        task: hle_header,
                        rdram: rdram as usize,
                        output_addr,
                        dp_full_sync: prior_full_sync,
                        completion_latency: chunk_completion_latency,
                    },
                    result,
                    if resumed_internal {
                        "resume HLE task"
                    } else {
                        "start HLE task"
                    },
                );
                return;
            }
            fn64_render::RenderTaskChunkStatus::Yielded => {
                assert_ne!(
                    result.dp_full_sync,
                    fn64_render::DpFullSyncStatus::Reached,
                    "yielded HLE graphics task cannot also report completed DP FullSync"
                );
                crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
                fn64_render::DpFullSyncStatus::NotReached
            }
            fn64_render::RenderTaskChunkStatus::NeedsLle { ucode_sha256 } => {
                assert!(
                    !resumed_internal,
                    "resumed HLE continuation requested LLE after committing an earlier chunk"
                );
                let mut digest = String::with_capacity(64);
                for byte in ucode_sha256 {
                    use std::fmt::Write as _;
                    write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
                }
                fn64_runtime::record_unsupported_event(
                    fn64_runtime::UnsupportedSubsystem::Render,
                    "render.hle-ucode.needs-lle",
                    format!("microcode_sha256={digest}"),
                    Some(fn64_runtime::Cycles::new(crate::sim_time())),
                    fn64_runtime::UnsupportedDisposition::NeedsLle,
                );
                // The renderer's preflight is transactional, so persistent
                // state is still exactly the classified ucode entry. Run the
                // complete phase through LLE with the boot snapshot or the
                // untouched direct PC-zero state; this is task-entry
                // continuation, not a fabricated mid-HLE transplant.
                let entry = hle_entry
                    .take()
                    .expect("gfx LLE fallback requires an admitted HLE entry");
                let pre_ucode_steps = entry.pre_ucode_steps();
                let microcode_data = initial_microcode_data
                    .expect("gfx LLE fallback requires admitted microcode-data identity");
                let lle = unsafe {
                    dispatch_lle_task(
                        rdram,
                        task_addr,
                        true,
                        entry.into_lle_machine_state(),
                        Some(microcode_data),
                        authoritative_microcode_family,
                    )
                };
                crate::pi::start_live_rcp_task_with_latency(
                    rcp_completion_plan(lle.dp_full_sync, "gfx LLE fallback"),
                    pre_ucode_steps.saturating_add(lle.steps),
                )
                .unwrap_or_else(|error| {
                    panic!("osSpTaskStartGo_recomp gfx LLE completion: {error}")
                });
                retire_rsp_task_lineage_after_synchronous_result(task_addr, "gfx LLE fallback");
                return;
            }
        }
    } else if header.task_type == M_AUDTASK {
        unsafe { dispatch_audio_task(rdram, o, &hle_header) };
        fn64_render::DpFullSyncStatus::NotReached
    } else {
        let lle = unsafe { dispatch_lle_task(rdram, task_addr, false, None, None, None) };
        crate::pi::start_live_rcp_task_with_latency(
            rcp_completion_plan(lle.dp_full_sync, "custom-task LLE"),
            lle.steps,
        )
        .unwrap_or_else(|error| panic!("osSpTaskStartGo_recomp LLE completion: {error}"));
        retire_rsp_task_lineage_after_synchronous_result(task_addr, "custom-task LLE");
        return;
    };

    let pre_ucode_steps = hle_entry
        .expect("known HLE task must have an admitted entry")
        .pre_ucode_steps();
    crate::pi::start_live_rcp_task_with_latency(
        rcp_completion_plan(dp_full_sync, "known HLE task"),
        pre_ucode_steps.saturating_add(1),
    )
    .unwrap_or_else(|error| panic!("osSpTaskStartGo_recomp: {error}"));
    retire_rsp_task_lineage_after_synchronous_result(task_addr, "known HLE task");
}

fn rcp_completion_plan(
    dp_full_sync: fn64_render::DpFullSyncStatus,
    operation: &'static str,
) -> fn64_runtime::RcpTaskCompletionPlan {
    match dp_full_sync {
        fn64_render::DpFullSyncStatus::Reached => {
            fn64_runtime::RcpTaskCompletionPlan::SpThenDpFullSync
        }
        fn64_render::DpFullSyncStatus::NotReached => fn64_runtime::RcpTaskCompletionPlan::SpOnly,
        fn64_render::DpFullSyncStatus::Unidentified => {
            panic!("{operation}: renderer completed without identifying DP FullSync state")
        }
    }
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
    use std::rc::Rc;

    macro_rules! no_rust_hidden_sidecar {
        () => {
            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
            }
        };
    }

    fn prepare_renderer_rdram(rdram: &mut Vec<u8>) {
        rdram.resize(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE, 0);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
    }

    struct StatusRenderBackend(FrameStatus);

    impl RenderBackend for StatusRenderBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(self.0)
        }

        fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
            if self.0 == FrameStatus::Complete {
                fn64_render::DpFullSyncStatus::Reached
            } else {
                fn64_render::DpFullSyncStatus::NotReached
            }
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    struct UnsupportedUcodeBackend;

    impl RenderBackend for UnsupportedUcodeBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Err(RenderError::UnsupportedUcode {
                ucode_addr: task.ucode,
            })
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    struct ExactIdentityBackend {
        admitted: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        admitted_data: fn64_render::MicrocodeDataImageIdentity,
        family: UcodeId,
    }

    impl RenderBackend for ExactIdentityBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            Ok(FrameStatus::Complete)
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn identify_microcode(
            &self,
            imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        ) -> Option<UcodeId> {
            (imem == &self.admitted).then_some(self.family)
        }

        fn identify_microcode_pair(
            &self,
            imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            data: fn64_render::MicrocodeDataImageIdentity,
        ) -> Option<UcodeId> {
            (imem == &self.admitted && data == self.admitted_data).then_some(self.family)
        }

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    #[test]
    fn render_task_maps_kseg0_display_list_pointer_to_a_physical_offset() {
        // Regression: WM2000 submits its display list at a KSEG0 virtual
        // address (0x8038ce30). Before masking, this reached rt64 ingress raw
        // and tripped "display-list address 0x8038ce30 is not a physical RDRAM
        // offset", panicking the shell on its first gfx task.
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            data_ptr: 0x8038_ce30,
            ..Default::default()
        };
        assert_eq!(render_task(&header).data_ptr, 0x0038_ce30);
        // An already-physical pointer passes through unchanged.
        let physical = OsTaskHeader {
            data_ptr: 0x0038_ce30,
            ..Default::default()
        };
        assert_eq!(render_task(&physical).data_ptr, 0x0038_ce30);
    }

    #[test]
    fn generated_c_kseg1_same_value_halfword_keeps_visible_bytes_and_renderer_sidecar_coherent() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct WriteCaptureBackend {
            write: Rc<Cell<Option<fn64_render::NonRdpWrite16>>>,
            hidden_bits: Rc<Cell<Option<u8>>>,
        }

        impl RenderBackend for WriteCaptureBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                self.write.set(Some(write));
                self.hidden_bits
                    .set(Some(if write.value() & 1 == 0 { 0 } else { 3 }));
                fn64_render::NonRdpWrite16Disposition::AppliedHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        RENDER_BACKEND.with(|cell| cell.replace(None));
        assert_eq!(observe_non_rdp_write16(0x40, 0x1235), None);

        let captured = Rc::new(Cell::new(None));
        let hidden_bits = Rc::new(Cell::new(None));
        set_render_backend(
            Box::new(WriteCaptureBackend {
                write: captured.clone(),
                hidden_bits: hidden_bits.clone(),
            }),
            0x100,
        );
        let mut visible = vec![0u8; 0x100];
        fn64_runtime::RdramViewMut::from_storage(&mut visible)
            .write_u16(fn64_runtime::RdramAddr::from_offset(0x40), 0x1235);
        crate::fn64_c_rdram_write(0xffff_ffff_a000_0040, 2, 0x1235);
        let write = captured
            .get()
            .expect("generated-C SH event was not delivered");
        assert_eq!(write.logical_offset().offset(), 0x40);
        assert_eq!(write.value(), 0x1235);
        assert_eq!(
            fn64_runtime::RdramView::from_storage(&visible)
                .read_u16(fn64_runtime::RdramAddr::from_offset(0x40)),
            0x1235
        );
        assert_eq!(hidden_bits.get(), Some(3));

        // A second identical assignment is still a distinct architectural
        // write and must be delivered rather than suppressed by equality.
        captured.set(None);
        crate::fn64_c_rdram_write(0xffff_ffff_a000_0040, 2, 0x1235);
        assert_eq!(captured.get(), Some(write));
        assert_eq!(hidden_bits.get(), Some(3));
    }

    #[test]
    fn generated_c_rdram_callback_rejects_mapped_aliases() {
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_8000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_a000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0000_8000_0040),
            Some(0x40)
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0000_a000_0040),
            Some(0x40)
        );
        assert_eq!(crate::generated_c_rdram_physical_offset(0x40), None);
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0xffff_ffff_c000_0040),
            None
        );
        assert_eq!(
            crate::generated_c_rdram_physical_offset(0x0000_0001_8000_0040),
            None
        );
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

            no_rust_hidden_sidecar!();

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

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let mut rdram = vec![0u8; 0x1000];
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(RspMemoryBackend), rdram.len());
        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ..Default::default()
        };
        let status = unsafe { dispatch_gfx_task(rdram.as_mut_ptr(), &header) };
        assert_eq!(status.status, FrameStatus::Complete);
        assert_eq!(
            status.dp_full_sync,
            fn64_render::DpFullSyncStatus::Unidentified
        );
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
            0x2407_7777,
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
    fn unsupported_backend_ucode_records_typed_event_before_loud_failure() {
        set_render_backend(Box::new(UnsupportedUcodeBackend), 0);
        fn64_runtime::arm_unsupported_events(None).unwrap();
        let mut rdram = [];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let task = fn64_render::OsTask {
            ucode: 0x0012_3450,
            ..Default::default()
        };

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_render_backend::<FrameStatus>("unsupported_ucode_test", |backend| {
                backend.process_task(&mut rdram, &mut rsp_memory, &task, 0)
            });
        }))
        .expect_err("unsupported backend ucode must remain a loud failure");

        assert!(
            panic_message(panic.as_ref()).contains("unsupported ucode at rdram offset 0x00123450")
        );
        assert_eq!(
            last_render_error().as_deref(),
            Some("unsupported ucode at rdram offset 0x00123450")
        );
        let events = fn64_runtime::copy_unsupported_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].subsystem,
            fn64_runtime::UnsupportedSubsystem::Render
        );
        assert_eq!(events[0].operation, "render.backend.unsupported-ucode");
        assert_eq!(
            events[0].context,
            "unsupported_ucode_test: backend rejected unlisted microcode at RDRAM offset 0x00123450"
        );
        assert_eq!(events[0].guest_cycle, Some(fn64_runtime::Cycles::ZERO));
        assert_eq!(
            events[0].disposition,
            fn64_runtime::UnsupportedDisposition::LoudTrap
        );
        fn64_runtime::complete_unsupported_observation(fn64_runtime::Cycles::ZERO, &"0".repeat(64));
        RENDER_BACKEND.with(|cell| cell.replace(None));
    }

    #[test]
    fn release_capture_crosses_the_owned_renderer_seam_without_downcasting() {
        struct CaptureBackend;

        impl RenderBackend for CaptureBackend {
            fn release_environment(&self) -> fn64_render::RenderBackendEvidence {
                fn64_render::RenderBackendEvidence::Rt64 {
                    tv_type: fn64_runtime::TvType::Pal,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    graphics_api: fn64_render::ActiveRenderGraphicsApi::Vulkan,
                    settings_sha256: [0x5a; 32],
                    replacement_packs_active: false,
                }
            }

            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn release_capture(
                &mut self,
            ) -> Result<fn64_render::RenderReleaseCapture, RenderError> {
                Ok(fn64_render::RenderReleaseCapture {
                    guest_cycle: 0x1234,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    settings_sha256: [0x5a; 32],
                    width: 2,
                    height: 1,
                    row_bytes: 8,
                    format: fn64_render::ReleaseCaptureFormat::PostViBgra8Unorm,
                    workload_id: std::num::NonZeroU64::new(5).unwrap(),
                    present_id: 7,
                    bytes: vec![1, 2, 3, 4, 5, 6, 7, 8],
                })
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        set_render_backend(Box::new(CaptureBackend), 0);
        let capture = capture_render_release_frame().unwrap();
        assert_eq!(capture.guest_cycle, 0x1234);
        assert_eq!(capture.backend_identity, "synthetic-release-backend");
        assert!(capture.source_authoritative);
        assert_eq!(capture.workload_id.get(), 5);
        assert_eq!(capture.settings_sha256, [0x5a; 32]);
        assert_eq!(capture.present_id, 7);
        assert_eq!(capture.bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(last_render_error(), None);
        assert_eq!(
            render_environment_evidence_snapshot(),
            RenderEnvironmentEvidenceSnapshot {
                backend: fn64_render::RenderBackendEvidence::Rt64 {
                    tv_type: fn64_runtime::TvType::Pal,
                    backend_identity: "synthetic-release-backend".to_string(),
                    source_authoritative: true,
                    graphics_api: fn64_render::ActiveRenderGraphicsApi::Vulkan,
                    settings_sha256: [0x5a; 32],
                    replacement_packs_active: false,
                },
                execution_policy: GraphicsTaskExecutionPolicy::HleOptimized,
            }
        );
        assert_eq!(
            render_environment_evidence_snapshot().renderer_tv_type(),
            Some(fn64_runtime::TvType::Pal)
        );
    }

    #[test]
    fn unidentified_renderer_snapshot_cannot_fabricate_tv_authority() {
        set_render_backend(Box::new(StatusRenderBackend(FrameStatus::Complete)), 0);
        let snapshot = render_environment_evidence_snapshot();
        assert_eq!(
            snapshot.backend,
            fn64_render::RenderBackendEvidence::Unidentified
        );
        assert_eq!(snapshot.renderer_tv_type(), None);
    }

    #[test]
    fn rsp_visibility_excludes_host_only_address_windows() {
        assert_eq!(
            rsp_visible_rdram_len(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x2490_0000),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE
        );
        assert_eq!(rsp_visible_rdram_len(0x1000), 0x1000);

        let (ranges, snapshot_len) = rsp_dma_storage_layout(
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x2000,
            std::iter::once(0x80_0000..0x80_1000).collect(),
        );
        assert_eq!(
            ranges,
            vec![
                0..fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
                0x80_0000..0x80_1000
            ]
        );
        assert_eq!(snapshot_len, 0x80_1000);
    }

    #[test]
    fn lle_debug_task_data_preserves_logical_order_at_the_rdram_boundary() {
        let logical = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87];
        let mut storage = [0u8; 8];
        fn64_runtime::RdramViewMut::from_storage(&mut storage)
            .write_logical_bytes(RdramAddr::from_offset(0), &logical);

        assert_eq!(
            lle_debug_task_data(&storage, 0xab00_0001, 1).as_deref(),
            Some(&logical[1..]),
            "the diagnostic's 0x40-byte minimum must truncate at RDRAM's end in guest byte order"
        );
        assert_eq!(
            lle_debug_task_data(&storage, storage.len() as u32, 1),
            None,
            "a task-data start at the allocation boundary must not create an empty dump"
        );
    }

    #[test]
    fn lle_debug_task_data_loudly_rejects_an_unmapped_native_word_lane() {
        let storage = [0u8; 7];
        let panic = std::panic::catch_unwind(|| lle_debug_task_data(&storage, 4, 1))
            .expect_err("an incomplete final native word must trap instead of supplying zero");
        assert!(panic_message(panic.as_ref())
            .contains("read_u8: logical RDRAM range 0x4..0x5 maps outside 7 storage bytes"));
    }

    #[test]
    fn renderer_entries_expose_exact_physical_rdram_and_its_last_byte() {
        use std::rc::Rc;

        crate::load_rom(Vec::new());

        struct SpanBackend(Rc<RefCell<Vec<(usize, u8)>>>);

        impl RenderBackend for SpanBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0
                    .borrow_mut()
                    .push((rdram.len(), *rdram.last().unwrap()));
                Ok(FrameStatus::Complete)
            }

            fn process_rdp_commands(
                &mut self,
                rdram: &mut [u8],
                _start: u32,
                _end: u32,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0
                    .borrow_mut()
                    .push((rdram.len(), *rdram.last().unwrap()));
                Ok(FrameStatus::Complete)
            }

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let physical_len = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
        let mut allocation = vec![0u8; physical_len + 0x2000];
        allocation[physical_len - 1] = 0xa5;
        allocation[physical_len] = 0x5a;
        let observations = Rc::new(RefCell::new(Vec::new()));
        set_render_backend(
            Box::new(SpanBackend(Rc::clone(&observations))),
            allocation.len(),
        );

        let header = OsTaskHeader {
            task_type: fn64_runtime::M_GFXTASK,
            ..Default::default()
        };
        unsafe {
            dispatch_gfx_task(allocation.as_mut_ptr(), &header);
            dispatch_raw_rdp(allocation.as_mut_ptr(), 0, 8);
        }

        assert_eq!(observations.borrow().as_slice(), [(physical_len, 0xa5); 2]);
        assert_eq!(allocation[physical_len], 0x5a);
        assert_eq!(
            copy_rsp_rdp_observations()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![RspRdpObservationKind::DramDpcCommitted {
                start: 0,
                end: 8,
                command_sha256: canonical_rdp_words_sha256(&[0, 0]),
            }]
        );
    }

    #[test]
    fn renderer_entry_rejects_a_registration_shorter_than_physical_rdram() {
        let mut allocation = [0u8; 1];
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            allocation.len(),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_gfx_task(allocation.as_mut_ptr(), &OsTaskHeader::default())
        }))
        .expect_err("a short renderer registration must trap before constructing a slice");
        assert!(panic_message(panic.as_ref()).contains("does not cover the required 8 MiB"));
    }

    #[test]
    fn completed_renderer_without_dp_full_sync_evidence_traps() {
        let panic = std::panic::catch_unwind(|| {
            rcp_completion_plan(
                fn64_render::DpFullSyncStatus::Unidentified,
                "synthetic completed renderer",
            )
        })
        .expect_err("successful graphics completion must identify FullSync state");
        assert!(panic_message(panic.as_ref()).contains(
            "synthetic completed renderer: renderer completed without identifying DP FullSync state"
        ));
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

    #[test]
    fn rejected_raw_dpc_does_not_enter_the_committed_observation_history() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
        );

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), 0, 8)
        }));

        assert!(rejected.is_err());
        assert!(copy_rsp_rdp_observations().is_empty());
        let snapshot = with_host(|host| host.device_fabric.snapshot());
        assert_eq!(snapshot.pending_dpc, None);
        assert_eq!(snapshot.dpc_start, 0);
        assert_eq!(snapshot.dpc_end, 0);
        assert_eq!(snapshot.dpc_current, 0);
        assert_eq!(snapshot.dpc_status, 0);
    }

    #[derive(Clone, Copy, Debug)]
    enum RawMutationOutcome {
        Complete,
        Error,
        Panic,
        Yielded,
    }

    struct MutatingRawBackend {
        calls: Rc<Cell<u32>>,
        outcome: RawMutationOutcome,
        mutation_offset: usize,
    }

    impl RenderBackend for MutatingRawBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        no_rust_hidden_sidecar!();

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            unreachable!("raw DPC regression backend received an HLE task")
        }

        fn process_rdp_commands(
            &mut self,
            rdram: &mut [u8],
            _start: u32,
            _end: u32,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            rdram[self.mutation_offset] = call as u8;
            match self.outcome {
                RawMutationOutcome::Complete => Ok(FrameStatus::Complete),
                RawMutationOutcome::Error => Err(RenderError::Backend {
                    backend: "synthetic-raw",
                    reason: "mutate-then-error".to_owned(),
                }),
                RawMutationOutcome::Panic => panic!("mutating raw backend panic"),
                RawMutationOutcome::Yielded => Ok(FrameStatus::Yielded),
            }
        }

        fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
            fn64_render::DpFullSyncStatus::Reached
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    #[test]
    fn rejected_raw_renderer_mutations_never_reach_live_rdram() {
        const START: usize = 0x100;
        const MUTATION: usize = 0x400;

        for (outcome, expected) in [
            (RawMutationOutcome::Error, "mutate-then-error"),
            (RawMutationOutcome::Panic, "mutating raw backend panic"),
            (
                RawMutationOutcome::Yielded,
                "raw RDP submission cannot yield as an RSP task",
            ),
        ] {
            crate::load_rom(Vec::new());
            let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
            rdram[START..START + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
            rdram[MUTATION] = 0x5a;
            let calls = Rc::new(Cell::new(0));
            set_render_backend(
                Box::new(MutatingRawBackend {
                    calls: Rc::clone(&calls),
                    outcome,
                    mutation_offset: MUTATION,
                }),
                rdram.len(),
            );
            let before_rdram = rdram.clone();
            let before_device = with_host(|host| host.device_fabric.snapshot());

            let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                dispatch_raw_rdp(rdram.as_mut_ptr(), START as u32, (START + 8) as u32)
            }))
            .expect_err("mutating raw renderer rejection must remain loud");

            assert!(
                panic_message(rejected.as_ref()).contains(expected),
                "{outcome:?} produced unexpected panic: {}",
                panic_message(rejected.as_ref())
            );
            assert_eq!(calls.get(), 1);
            assert_eq!(rdram, before_rdram, "{outcome:?} leaked renderer bytes");
            assert_eq!(
                with_host(|host| host.device_fabric.snapshot()),
                before_device,
                "{outcome:?} changed guest-visible DPC state"
            );
            assert!(copy_rsp_rdp_observations().is_empty());
        }
    }

    #[test]
    fn mismatched_raw_full_sync_evidence_rejects_before_rdram_or_dpc_commit() {
        const START: usize = 0x100;
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), START as u32, (START + 8) as u32)
        }))
        .expect_err("backend FullSync evidence must match the submitted command stream");

        assert!(panic_message(rejected.as_ref()).contains(
            "renderer FullSync evidence disagrees with the submitted raw RDP command stream"
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert!(copy_rsp_rdp_observations().is_empty());
    }

    #[test]
    fn mismatched_captured_full_sync_evidence_rejects_before_rdram_or_dpc_commit() {
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, 8)
        }))
        .expect_err("captured FullSync evidence must match the staged command stream");

        assert!(panic_message(rejected.as_ref()).contains(
            "renderer FullSync evidence disagrees with the submitted raw RDP command stream"
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert!(copy_rsp_rdp_observations().is_empty());
    }

    #[test]
    fn atomic_ack_validation_failure_precedes_xbus_publication_and_rolls_back() {
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[MUTATION] = 0x5a;
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );
        let submission = with_host(|host| {
            host.device_fabric
                .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Dmem, 0, 8)
        })
        .unwrap();
        let mut transaction = LiveDpcTransaction::new(submission);
        let fn64_runtime::DpcScheduledPhase::AwaitingAck(request) = transaction
            .acknowledgment
            .as_ref()
            .expect("test transaction owns its atomic acknowledgment")
            .phase()
        else {
            panic!("production atomic transaction did not stop at its sole ack barrier")
        };
        assert_eq!(
            request.transaction,
            fn64_runtime::DpcTransactionId::from_submission(submission)
        );
        assert_eq!(request.quantum, fn64_runtime::DpcQuantumId::new(1));
        assert_eq!(request.start.source(), submission.source);
        assert_eq!(request.start.address(), submission.start);
        assert_eq!(request.end.source(), submission.source);
        assert_eq!(request.end.address(), submission.end);
        transaction
            .acknowledgment
            .as_mut()
            .expect("test transaction owns its atomic acknowledgment")
            .poison();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_captured_raw_rdp(
                rdram.as_mut_ptr(),
                &[0xe900_0000, 0],
                0,
                8,
                true,
                &mut transaction,
            )
        }))
        .expect_err("poisoned atomic acknowledgment must remain loud");
        assert!(panic_message(rejected.as_ref())
            .contains("lost its acknowledgment owner before validation"));
        assert_eq!(
            calls.get(),
            1,
            "validation remains after backend acceptance"
        );
        assert_eq!(rdram, before_rdram, "ack failure published the XBUS shadow");
        assert!(copy_rsp_rdp_observations().is_empty());

        drop(transaction);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device,
            "ack failure did not restore the pre-admission DPC state"
        );
    }

    #[test]
    fn second_raw_full_sync_rejects_before_renderer_or_rdram_mutation() {
        const FIRST: usize = 0x100;
        const SECOND: usize = 0x108;
        const MUTATION: usize = 0x400;

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        rdram[FIRST..FIRST + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        rdram[SECOND..SECOND + 4].copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        let calls = Rc::new(Cell::new(0));
        set_render_backend(
            Box::new(MutatingRawBackend {
                calls: Rc::clone(&calls),
                outcome: RawMutationOutcome::Complete,
                mutation_offset: MUTATION,
            }),
            rdram.len(),
        );

        unsafe { dispatch_raw_rdp(rdram.as_mut_ptr(), FIRST as u32, SECOND as u32) };
        assert_eq!(calls.get(), 1);
        let before_rdram = rdram.clone();
        let before_device = with_host(|host| host.device_fabric.snapshot());
        let before_observations = copy_rsp_rdp_observations();

        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            dispatch_raw_rdp(rdram.as_mut_ptr(), SECOND as u32, (SECOND + 8) as u32)
        }))
        .expect_err("a second unserviced raw FullSync must remain loud");

        assert!(panic_message(rejected.as_ref()).contains("graphics task start while DP is busy"));
        assert_eq!(
            calls.get(),
            1,
            "the occupied DP slot must reject before renderer entry"
        );
        assert_eq!(rdram, before_rdram);
        assert_eq!(
            with_host(|host| host.device_fabric.snapshot()),
            before_device
        );
        assert_eq!(copy_rsp_rdp_observations(), before_observations);
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
        crate::set_trace_enabled(true);
        let prior_starts = crate::copy_trace()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    fn64_runtime::TraceKind::TaskSubmit {
                        task_kind: fn64_runtime::TaskKind::Graphics,
                        ucode,
                    } if ucode == header.ucode
                )
            })
            .count();
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
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Graphics,
                            ucode,
                        } if ucode == header.ucode
                    )
                })
                .count(),
            prior_starts,
            "osSpTaskLoad admission cannot emit the StartGo-qualified TaskSubmit trace"
        );
    }

    #[test]
    fn repeated_task_load_uses_the_cpu_cached_rspboot_image_after_rsp_dma_writes() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        with_host(|host| host.rsp_boot_images.clear());
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let boot_off = u32::from_ne_bytes(rdram[HEADER + 8..HEADER + 12].try_into().unwrap());
        let original = with_host(|host| {
            host.device_fabric
                .rsp_memory()
                .read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 8)
                .unwrap()
        });

        fn64_runtime::RdramViewMut::from_storage(&mut rdram)
            .write_logical_bytes(RdramAddr::from_offset(boot_off), &[0; 8]);
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let mut physical = [0xff; 8];
        fn64_runtime::RdramView::from_storage(&rdram)
            .copy_logical_bytes(RdramAddr::from_offset(boot_off), &mut physical);
        assert_eq!(physical, [0; 8], "the RSP's physical write stays visible");
        with_host(|host| {
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), 8)
                    .unwrap(),
                original,
                "osSpTaskLoad must re-DMA the CPU-cached boot text"
            );
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

        let boot = unsafe {
            dispatch_hle_rspboot(rdram.as_mut_ptr(), RdramAddr::from_offset(HEADER as u32))
        };

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
            dispatch_hle_rspboot(rdram.as_mut_ptr(), RdramAddr::from_offset(HEADER as u32))
        }))
        .expect_err("rspboot BREAK before ucode must trap");
        assert!(panic_message(panic.as_ref())
            .contains("RSP HLE rspboot reached BREAK before entering DMA-loaded ucode"));
    }

    #[test]
    fn direct_imem_shape_requires_the_admitted_boot_copy_to_cover_the_ucode() {
        let direct = OsTaskHeader {
            ucode_boot: 0x8000_0200,
            ucode_boot_size: 0x1000,
            ucode: 0xA000_0200,
            ucode_size: 0x1000,
            ..Default::default()
        };
        assert_eq!(
            admitted_task_image_shape(&direct),
            AdmittedTaskImageShape::DirectImem
        );
        assert_eq!(
            admitted_task_image_shape(&OsTaskHeader {
                ucode_boot_size: 8,
                ucode_size: 16,
                ..direct
            }),
            AdmittedTaskImageShape::BootOverlay,
            "equal pointers alone must not bypass rspboot when the admitted copy is incomplete"
        );
    }

    #[test]
    fn direct_imem_graphics_task_starts_lle_at_pc_zero_without_rspboot_overlay() {
        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        const DATA: u32 = 0x181;
        const MUTATED_DATA: u32 = 0x1d1;
        const INITIAL_DATA: [u8; 5] = [0x01, 0x23, 0x45, 0x67, 0x89];
        const START_DATA: [u8; 5] = [0xfe, 0xdc, 0xba, 0x98, 0x76];
        const MUTATED_DATA_BYTES: [u8; 3] = [0xaa, 0xbb, 0xcc];
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x220];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 12),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, 12),
            (0x18, 0xA000_0000 | DATA),
            (0x1c, INITIAL_DATA.len() as u32),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, word) in [0x2408_1234u32, 0xac08_0100, 0x0000_000d]
            .into_iter()
            .enumerate()
        {
            let offset = IMAGE + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in INITIAL_DATA.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        let admitted_header = unsafe { read_os_task_header(rdram.as_mut_ptr(), HEADER) };
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            crate::host_evidence_snapshot().loaded_rsp_task,
            Some(LoadedRspTaskEvidenceSnapshot {
                task_offset: HEADER as u32,
                header: admitted_header,
                resumed_data_identity: None,
            })
        );
        // StartGo hashes current bytes at the address/size admitted by Load.
        // Mutating the CPU header to a second source must not change that
        // admitted source, while mutation of source A's bytes remains visible.
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in START_DATA.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
            for (offset, byte) in MUTATED_DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(MUTATED_DATA + offset as u32), byte);
            }
        }
        rdram[HEADER + 0x18..HEADER + 0x1c]
            .copy_from_slice(&(0xA000_0000 | MUTATED_DATA).to_ne_bytes());
        rdram[HEADER + 0x1c..HEADER + 0x20]
            .copy_from_slice(&(MUTATED_DATA_BYTES.len() as u32).to_ne_bytes());
        let expected_at = Cycles::new(sim_time());
        let (imem_generation, expected_digest) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                memory.imem_generation(),
                imem_sha256(memory.bank(fn64_runtime::RspMemoryBank::Imem)),
            )
        });
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let host_evidence = crate::host_evidence_snapshot();
        assert_eq!(host_evidence.loaded_rsp_task, None);
        assert!(
            host_evidence.rsp_task_lineages.is_empty(),
            "a synchronous normal completion must retire its Running lineage"
        );

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
                0x0000_1234,
                "direct-image LLE must execute the instruction at admitted IMEM PC zero"
            );
            assert!(fabric.snapshot().sp_busy);
        });
        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![RspRdpObservationEvent {
                at: expected_at,
                kind: RspRdpObservationKind::MicrocodeRecognition {
                    task_addr: RdramAddr::from_offset(HEADER as u32),
                    imem_generation,
                    text_sha256: expected_digest,
                    data_addr: RdramAddr::from_offset(DATA),
                    data_size: START_DATA.len() as u32,
                    data_sha256: Sha256::digest(START_DATA).into(),
                    family: None,
                },
            }]
        );
        crate::advance_virtual_time(3);
    }

    #[test]
    fn yielded_resume_reuses_typed_original_data_lineage_until_rom_reset() {
        const TASK: u32 = 0x40;
        const ORIGINAL: u32 = 0x101;
        const YIELD: u32 = 0x181;
        const ORIGINAL_BYTES: [u8; 5] = [0x11, 0x22, 0x33, 0x44, 0x55];
        const YIELD_BYTES: [u8; 5] = [0xaa, 0xbb, 0xcc, 0xdd, 0xee];

        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in ORIGINAL_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(ORIGINAL + offset as u32), byte);
            }
            for (offset, byte) in YIELD_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(YIELD + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let task_addr = RdramAddr::from_offset(TASK);
        let initial_header = OsTaskHeader {
            task_type: M_GFXTASK,
            ucode_data: 0x8000_0000 | ORIGINAL,
            ucode_data_size: ORIGINAL_BYTES.len() as u32,
            yield_data_ptr: 0xA000_0000 | YIELD,
            yield_data_size: YIELD_BYTES.len() as u32,
            ..Default::default()
        };
        let original = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                task_addr,
                initial_header.ucode_data,
                initial_header.ucode_data_size,
            )
        };
        with_host(|host| {
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    original_header: initial_header,
                    data_identity: Some(original),
                    phase: RspTaskLineagePhase::ResumeAuthorized,
                },
            );
        });

        let resumed_header = OsTaskHeader {
            flags: fn64_runtime::OS_TASK_YIELDED,
            ucode_data: initial_header.yield_data_ptr,
            ucode_data_size: initial_header.yield_data_size,
            ..initial_header
        };
        let resumed = loaded_rsp_task_from_header(task_addr, resumed_header);
        assert_eq!(resumed.resumed_data_identity, Some(original));
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
        let yield_sha256: [u8; 32] = Sha256::digest(YIELD_BYTES).into();
        assert_ne!(
            resumed
                .resumed_data_identity
                .expect("yielded load retains data identity")
                .sha256,
            yield_sha256
        );

        retain_loaded_rsp_task(resumed);
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded
        );
        let replay =
            std::panic::catch_unwind(|| loaded_rsp_task_from_header(task_addr, resumed_header))
                .unwrap_err();
        let replay_message = panic_message(replay.as_ref());
        assert!(replay_message.contains("has no unused resume authorization"));

        let loaded = take_loaded_rsp_task(task_addr);
        retain_started_rsp_task_lineage(loaded, Some(original));
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::Running
        );

        crate::load_rom(Vec::new());
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            loaded_rsp_task_from_header(task_addr, resumed_header)
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("yielded RSP task 0x00000040 has no retained task lineage"));
    }

    #[test]
    fn rom_reset_invalidates_unconsumed_loaded_task_authority() {
        let task_addr = RdramAddr::from_offset(0x40);
        retain_loaded_rsp_task(LoadedRspTask {
            task_addr,
            header: OsTaskHeader {
                task_type: M_GFXTASK,
                ..Default::default()
            },
            resumed_data_identity: None,
        });
        crate::load_rom(Vec::new());

        let panic = std::panic::catch_unwind(|| take_loaded_rsp_task(task_addr)).unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("has no unconsumed osSpTaskLoad admission"));
        assert!(crate::host_evidence_snapshot().rsp_task_lineages.is_empty());
    }

    #[test]
    fn loading_one_suspended_task_preserves_other_resume_authorizations() {
        crate::load_rom(Vec::new());
        let original = |yield_data_ptr| OsTaskHeader {
            yield_data_ptr,
            yield_data_size: 0x40,
            ..Default::default()
        };
        let first_addr = RdramAddr::from_offset(0x40);
        let second_addr = RdramAddr::from_offset(0x80);
        let first = RspTaskLineage {
            original_header: original(0x180),
            data_identity: None,
            phase: RspTaskLineagePhase::ResumeAuthorized,
        };
        let second = RspTaskLineage {
            original_header: original(0x1c0),
            data_identity: None,
            phase: RspTaskLineagePhase::ResumeAuthorized,
        };
        with_host(|host| {
            host.rsp_task_lineages.insert(first_addr.offset(), first);
            host.rsp_task_lineages.insert(second_addr.offset(), second);
        });

        let loaded = loaded_rsp_task_from_header(first_addr, first.yielded_header());
        retain_loaded_rsp_task(loaded);
        let snapshot = crate::host_evidence_snapshot();
        assert_eq!(snapshot.rsp_task_lineages.len(), 2);
        assert_eq!(
            snapshot.rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded
        );
        assert_eq!(
            snapshot.rsp_task_lineages[1].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );

        let loaded = take_loaded_rsp_task(first_addr);
        retain_started_rsp_task_lineage(loaded, None);
        retire_running_rsp_task_lineage(first_addr, "multiple-suspended test completion");
        let snapshot = crate::host_evidence_snapshot();
        assert_eq!(snapshot.rsp_task_lineages.len(), 1);
        assert_eq!(
            snapshot.rsp_task_lineages[0].task_offset,
            second_addr.offset()
        );
        assert_eq!(
            snapshot.rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
    }

    #[test]
    fn fresh_load_reuse_cancels_same_address_resume_authorization() {
        crate::load_rom(Vec::new());
        let task_addr = RdramAddr::from_offset(0x40);
        with_host(|host| {
            host.rsp_task_lineages.insert(
                task_addr.offset(),
                RspTaskLineage {
                    original_header: OsTaskHeader {
                        yield_data_ptr: 0x180,
                        yield_data_size: 0x40,
                        ..Default::default()
                    },
                    data_identity: None,
                    phase: RspTaskLineagePhase::ResumeAuthorized,
                },
            );
        });

        retain_loaded_rsp_task(LoadedRspTask {
            task_addr,
            header: OsTaskHeader::default(),
            resumed_data_identity: None,
        });

        assert!(crate::host_evidence_snapshot().rsp_task_lineages.is_empty());
    }

    #[test]
    fn microcode_data_capture_rejects_out_of_bounds_task_range() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x100];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let header = OsTaskHeader {
            task_type: M_GFXTASK,
            ucode_data: 0x8000_00ff,
            ucode_data_size: 2,
            ..Default::default()
        };
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                header.ucode_data,
                header.ucode_data_size,
            )
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("microcode-data range [0x000000ff, 0x00000101)"));
        assert!(message.contains("registered allocation length 0x100"));
    }

    #[test]
    fn microcode_data_capture_uses_sp_dram_addr_high_alias() {
        const DATA: u32 = 0x81;
        const BYTES: [u8; 5] = [0x10, 0x32, 0x54, 0x76, 0x98];
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x100];
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
        }
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let identity = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                0xab00_0000 | DATA,
                BYTES.len() as u32,
            )
        };

        assert_eq!(identity.addr, RdramAddr::from_offset(DATA));
        let expected_sha256: [u8; 32] = Sha256::digest(BYTES).into();
        assert_eq!(identity.sha256, expected_sha256);
    }

    #[test]
    fn microcode_data_capture_rejects_sparse_host_bytes_beyond_physical_rdram() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE + 0x100];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0x40),
                fn64_runtime::rdram::DEFAULT_RDRAM_SIZE as u32 - 1,
                2,
            )
        }))
        .unwrap_err();
        let message = panic_message(panic.as_ref());
        assert!(message.contains("microcode-data range [0x007fffff, 0x00800001)"));
        assert!(message.contains("exceeds physical RDRAM length 0x800000"));
    }

    #[test]
    fn text_only_backend_identity_cannot_set_a_microcode_pair_family() {
        struct TextOnlyBackend {
            admitted: [u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        }

        impl RenderBackend for TextOnlyBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn identify_microcode(
                &self,
                imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
            ) -> Option<UcodeId> {
                (imem == &self.admitted).then_some(UcodeId::F3dex2)
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let admitted = [0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let backend = TextOnlyBackend { admitted };
        assert_eq!(backend.identify_microcode(&admitted), Some(UcodeId::F3dex2));
        set_render_backend(Box::new(backend), fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
        assert_eq!(
            identify_microcode_pair(
                &admitted,
                TaskMicrocodeDataIdentity {
                    addr: RdramAddr::from_offset(0x100),
                    size: 3,
                    sha256: Sha256::digest([1, 2, 3]).into(),
                },
                None,
            ),
            None
        );
    }

    #[test]
    fn pinned_family_authority_fills_absent_backend_identity_and_rejects_conflict() {
        let imem = [0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let data = TaskMicrocodeDataIdentity {
            addr: RdramAddr::from_offset(0x100),
            size: 3,
            sha256: Sha256::digest([1, 2, 3]).into(),
        };
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        assert_eq!(
            identify_microcode_pair(&imem, data, Some(UcodeId::F3dzex2)),
            Some(UcodeId::F3dzex2)
        );

        set_render_backend(
            Box::new(ExactIdentityBackend {
                admitted: imem,
                admitted_data: fn64_render::MicrocodeDataImageIdentity {
                    bytes: data.size,
                    sha256: data.sha256,
                },
                family: UcodeId::F3dex2,
            }),
            fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        );
        let panic = std::panic::catch_unwind(|| {
            identify_microcode_pair(&imem, data, Some(UcodeId::F3dzex2))
        })
        .unwrap_err();
        assert!(panic_message(panic.as_ref()).contains("backend pair catalog claimed F3dex2"));
    }

    #[test]
    fn lle_microcode_recognition_requires_the_backends_exact_text_data_pair() {
        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x100;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x200];
        for (field, value) in [
            (0x00, fn64_runtime::M_GFXTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, 12),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, 12),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        for (index, word) in [0x2408_4321u32, 0xac08_0100, 0x0000_000d]
            .into_iter()
            .enumerate()
        {
            let offset = IMAGE + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        prepare_renderer_rdram(&mut rdram);
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        let (admitted, imem_generation) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                *memory.bank(fn64_runtime::RspMemoryBank::Imem),
                memory.imem_generation(),
            )
        });
        let expected_digest = imem_sha256(&admitted);
        let expected_at = Cycles::new(sim_time());
        set_render_backend_with_policy(
            Box::new(ExactIdentityBackend {
                admitted,
                admitted_data: fn64_render::MicrocodeDataImageIdentity {
                    bytes: 0,
                    sha256: Sha256::digest([]).into(),
                },
                family: UcodeId::F3dzex2,
            }),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![RspRdpObservationEvent {
                at: expected_at,
                kind: RspRdpObservationKind::MicrocodeRecognition {
                    task_addr: RdramAddr::from_offset(HEADER as u32),
                    imem_generation,
                    text_sha256: expected_digest,
                    data_addr: RdramAddr::from_offset(0),
                    data_size: 0,
                    data_sha256: Sha256::digest([]).into(),
                    family: Some(UcodeId::F3dzex2),
                },
            }]
        );
    }

    #[test]
    fn graphics_hle_unsupported_fallback_records_then_replays_untouched_ucode_through_lle() {
        const HEADER: usize = 0x40;
        let mut rdram = vec![0u8; 0x200];
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        admit_synthetic_hle_task(&mut rdram, HEADER, &mut ctx);
        let ucode_off =
            u32::from_ne_bytes(rdram[HEADER + 0x10..HEADER + 0x14].try_into().unwrap()) as usize;
        for (index, word) in [0x2405_5678u32, 0xac07_0100].into_iter().enumerate() {
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
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(StatusRenderBackend(FrameStatus::NeedsLle {
                ucode_sha256: [0; 32],
            })),
            rdram.len(),
        );
        fn64_runtime::arm_unsupported_events(None).unwrap();

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let unsupported = fn64_runtime::copy_unsupported_events();
        assert_eq!(unsupported.len(), 1);
        assert!(unsupported[0].operation.starts_with("render.hle-ucode."));
        assert_eq!(
            unsupported[0].disposition,
            fn64_runtime::UnsupportedDisposition::NeedsLle
        );
        assert_eq!(unsupported[0].guest_cycle, Some(fn64_runtime::Cycles::ZERO));

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
                0x0000_7777,
                "LLE fallback must retain the rspboot jump-delay scalar register state"
            );
            assert_eq!(fabric.sp_pc(), 0x88);
            assert!(
                fabric.snapshot().sp_busy,
                "the LLE BREAK schedules externally visible SP completion"
            );
        });
    }

    #[test]
    fn graphics_lle_accuracy_policy_forwards_raw_dpc_without_hle_dispatch() {
        use std::cell::RefCell;
        use std::rc::Rc;

        crate::load_rom(Vec::new());

        type DpcCall = (u32, u32, u32, u32);
        struct LleDpcBackend {
            hle_calls: Rc<Cell<u32>>,
            dpc_calls: Rc<RefCell<Vec<DpcCall>>>,
        }

        impl RenderBackend for LleDpcBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.hle_calls.set(self.hle_calls.get() + 1);
                Ok(FrameStatus::Complete)
            }

            fn process_rdp_commands(
                &mut self,
                rdram: &mut [u8],
                start: u32,
                end: u32,
                output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                let first = fn64_runtime::RdramView::from_storage(rdram)
                    .read_u32(fn64_runtime::RdramAddr::from_offset(start));
                self.dpc_calls
                    .borrow_mut()
                    .push((start, end, output_addr, first));
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::Reached
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER: usize = 0x40;
        const DPC_START: u32 = 0x180;
        const DPC_END: u32 = 0x188;
        const VI_OUTPUT: u32 = 0x100;
        const MICROCODE_DATA: u32 = 0x1a1;
        const MICROCODE_DATA_BYTES: [u8; 5] = [0x13, 0x57, 0x9b, 0xdf, 0x24];
        let mtc0 = |rt: u32, rd: u32| (0x10 << 26) | (0x04 << 21) | (rt << 16) | (rd << 11);
        let mut rdram = vec![0u8; 0x200];
        rdram[DPC_START as usize..DPC_START as usize + 4]
            .copy_from_slice(&0xe900_0000u32.to_ne_bytes());
        rdram[DPC_START as usize + 4..DPC_END as usize].copy_from_slice(&0u32.to_ne_bytes());
        rdram[HEADER..HEADER + 4].copy_from_slice(&fn64_runtime::M_GFXTASK.to_ne_bytes());
        rdram[HEADER + 0x18..HEADER + 0x1c]
            .copy_from_slice(&(0xA000_0000 | MICROCODE_DATA).to_ne_bytes());
        rdram[HEADER + 0x1c..HEADER + 0x20]
            .copy_from_slice(&(MICROCODE_DATA_BYTES.len() as u32).to_ne_bytes());
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in MICROCODE_DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(MICROCODE_DATA + offset as u32), byte);
            }
        }
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
        let hle_calls = Rc::new(Cell::new(0));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(LleDpcBackend {
                hle_calls: Rc::clone(&hle_calls),
                dpc_calls: Rc::clone(&submissions),
            }),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let mut vi_ctx = ctx_zeroed();
        vi_ctx.r4 = u64::from(0x8000_0000 | VI_OUTPUT);
        unsafe { crate::vi::osViSwapBuffer_recomp(rdram.as_mut_ptr(), &mut vi_ctx) };

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        let submissions = submissions.borrow();
        assert_eq!(
            hle_calls.get(),
            0,
            "LLE accuracy policy must not offer graphics microcode to HLE"
        );
        assert_eq!(submissions.len(), 1);
        let (start, end, output, first) = submissions[0];
        assert_eq!(end - start, DPC_END - DPC_START);
        assert_eq!(output, VI_OUTPUT);
        assert_eq!(first, 0xe900_0000);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(snapshot.sp_busy);
            assert!(snapshot.dp_busy);
        });
        let observations = copy_rsp_rdp_observations();
        assert_eq!(observations.len(), 3);
        let microcode_data_sha256: [u8; 32] = Sha256::digest(MICROCODE_DATA_BYTES).into();
        let replacement_generation = match &observations[0].kind {
            RspRdpObservationKind::ImemReplacementCommitted {
                task_addr,
                imem_generation,
                ..
            } => {
                assert_eq!(*task_addr, RdramAddr::from_offset(HEADER as u32));
                *imem_generation
            }
            ref other => panic!("expected rspboot replacement first, got {other:?}"),
        };
        assert!(matches!(
            &observations[1].kind,
            RspRdpObservationKind::MicrocodeRecognition {
                task_addr,
                imem_generation,
                data_addr,
                data_size,
                data_sha256,
                family: None,
                ..
            } if *task_addr == RdramAddr::from_offset(HEADER as u32)
                && *imem_generation == replacement_generation
                && *data_addr == RdramAddr::from_offset(MICROCODE_DATA)
                && *data_size == MICROCODE_DATA_BYTES.len() as u32
                && *data_sha256 == microcode_data_sha256
        ));
        assert_eq!(
            observations[2].kind,
            RspRdpObservationKind::DramDpcCommitted {
                start: DPC_START,
                end: DPC_END,
                command_sha256: canonical_rdp_words_sha256(&[0xe900_0000, 0]),
            }
        );
    }

    #[test]
    fn graphics_hle_optimized_policy_remains_explicitly_selectable() {
        use std::rc::Rc;

        struct CountingHleBackend(Rc<Cell<u32>>);

        impl RenderBackend for CountingHleBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                self.0.set(self.0.get() + 1);
                Ok(FrameStatus::Complete)
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

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
        let calls = Rc::new(Cell::new(0));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend_with_policy(
            Box::new(CountingHleBackend(Rc::clone(&calls))),
            rdram.len(),
            GraphicsTaskExecutionPolicy::HleOptimized,
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

        assert_eq!(calls.get(), 1);
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(snapshot.sp_busy);
            assert!(
                !snapshot.dp_busy,
                "an HLE graphics task without FullSync must schedule SP only"
            );
            assert_eq!(
                host.device_fabric
                    .rsp_memory()
                    .read_word(fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        0x100,
                    ))
                    .unwrap(),
                0,
                "optimized HLE must retain the loaded ucode behind its backend boundary"
            );
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

        let result = unsafe {
            dispatch_lle_task(
                rdram.as_mut_ptr(),
                RdramAddr::from_offset(0),
                false,
                None,
                None,
                None,
            )
        };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 3,
                dp_full_sync: fn64_render::DpFullSyncStatus::NotReached,
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
        retain_loaded_rsp_task(LoadedRspTask {
            task_addr: RdramAddr::from_offset(HEADER as u32),
            header: OsTaskHeader::default(),
            resumed_data_identity: None,
        });

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
    fn two_consecutive_normal_tasks_retire_running_lineage_without_yield_query() {
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x1000];
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
            let program = [0x0000_000du32];
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

        for task_offset in [0x40, 0x80] {
            crate::pi::set_live_sp_pc(0);
            let task_addr = RdramAddr::from_offset(task_offset);
            retain_loaded_rsp_task(LoadedRspTask {
                task_addr,
                header: OsTaskHeader::default(),
                resumed_data_identity: None,
            });
            ctx.r4 = 0x8000_0000 + u64::from(task_offset);

            unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };

            assert!(
                crate::host_evidence_snapshot().rsp_task_lineages.is_empty(),
                "normal task {task_offset:#x} must retire before another task starts"
            );
            let deadline = crate::next_device_deadline().expect("normal task completion deadline");
            crate::advance_virtual_time(deadline);
        }
    }

    #[test]
    fn unknown_task_lle_resolves_rspboot_style_imem_overlay_and_resumes() {
        const DATA: u32 = 0x281;
        const DATA_BYTES: [u8; 7] = [0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc];
        crate::load_rom(Vec::new());
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
        prepare_renderer_rdram(&mut rdram);
        for (index, word) in overlay.into_iter().enumerate() {
            let offset = 0x200 + index * 4;
            rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
        }
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            for (offset, byte) in DATA_BYTES.into_iter().enumerate() {
                view.write_u8(RdramAddr::from_offset(DATA + offset as u32), byte);
            }
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
        let (generation_before, initial_digest) = with_host(|host| {
            let memory = host.device_fabric.rsp_memory();
            (
                memory.imem_generation(),
                imem_sha256(memory.bank(fn64_runtime::RspMemoryBank::Imem)),
            )
        });
        set_render_backend_with_policy(
            Box::new(StatusRenderBackend(FrameStatus::Complete)),
            rdram.len(),
            GraphicsTaskExecutionPolicy::LleAccuracy,
        );
        let expected_at = Cycles::new(sim_time());

        let task_addr = RdramAddr::from_offset(0x40);
        let task = OsTaskHeader {
            ucode_data: 0x8000_0000 | DATA,
            ucode_data_size: DATA_BYTES.len() as u32,
            ..Default::default()
        };
        let microcode_data = unsafe {
            task_microcode_data_identity(
                rdram.as_mut_ptr(),
                task_addr,
                task.ucode_data,
                task.ucode_data_size,
            )
        };
        let result = unsafe {
            dispatch_lle_task(
                rdram.as_mut_ptr(),
                task_addr,
                true,
                None,
                Some(microcode_data),
                None,
            )
        };

        assert_eq!(
            result,
            LleTaskResult {
                steps: 9,
                dp_full_sync: fn64_render::DpFullSyncStatus::NotReached,
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
        let final_digest = with_host(|host| {
            imem_sha256(
                host.device_fabric
                    .rsp_memory()
                    .bank(fn64_runtime::RspMemoryBank::Imem),
            )
        });
        assert_eq!(
            copy_rsp_rdp_observations(),
            vec![
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::MicrocodeRecognition {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before,
                        text_sha256: initial_digest,
                        data_addr: microcode_data.addr,
                        data_size: microcode_data.size,
                        data_sha256: microcode_data.sha256,
                        family: None,
                    },
                },
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::ImemReplacementCommitted {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before + 1,
                        text_sha256: final_digest,
                    },
                },
                RspRdpObservationEvent {
                    at: expected_at,
                    kind: RspRdpObservationKind::MicrocodeRecognition {
                        task_addr: RdramAddr::from_offset(0x40),
                        imem_generation: generation_before + 1,
                        text_sha256: final_digest,
                        data_addr: microcode_data.addr,
                        data_size: microcode_data.size,
                        data_sha256: microcode_data.sha256,
                        family: None,
                    },
                },
            ]
        );
    }

    #[test]
    fn xbus_dpc_submission_stages_logical_dmem_commands_for_renderer() {
        use fn64_render::RenderConfig;

        crate::load_rom(Vec::new());
        const TARGET: u32 = 0x400;
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 4] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0003, TARGET),
            (0xf700_0000, 0x07c1_07c1),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        ];
        let mut dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            dmem[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            dmem[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, (commands.len() * 8) as u32);
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
        assert_eq!(
            copy_rsp_rdp_observations()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![RspRdpObservationKind::XbusDpcCommitted {
                start: 0,
                end: (commands.len() * 8) as u32,
                command_sha256: canonical_rdp_words_sha256(
                    &commands
                        .into_iter()
                        .flat_map(|(w0, w1)| [w0, w1])
                        .collect::<Vec<_>>()
                ),
            }]
        );
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
            // lft=0: the vertical XM minor edge sits left of the rightward-
            // sloping XH major edge (right-major geometry).
            (0x0900_0000 | yl, (ym << 16) | yh),
            (1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32),
            (1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32),
            (1 << 16, 0),
            (4 << 16, 0),
            (0, 0),
            (0xe900_0000, 0),
        ];
        let mut dmem = [0u8; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = index * 8;
            dmem[offset..offset + 4].copy_from_slice(&w0.to_be_bytes());
            dmem[offset + 4..offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let mut backend = fn64_render_reference::ReferenceBackend::new().with_f3dex2();
        backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());

        unsafe {
            dispatch_raw_rdp_xbus(rdram.as_mut_ptr(), &dmem, 0, (commands.len() * 8) as u32);
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
        prepare_renderer_rdram(&mut rdram);
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
        prepare_renderer_rdram(&mut rdram);
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

            no_rust_hidden_sidecar!();

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

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

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
        prepare_renderer_rdram(&mut rdram);
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

    #[test]
    fn chunked_hle_observes_sig0_between_commits_and_consumes_resume_once() {
        use std::sync::{Arc, Mutex};

        struct ChunkedBackend {
            steps: Arc<Mutex<Vec<fn64_render::RenderTaskStep>>>,
        }

        impl RenderBackend for ChunkedBackend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            no_rust_hidden_sidecar!();

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                Err(RenderError::Backend {
                    backend: "chunked-test",
                    reason: "atomic entry must not be used".into(),
                })
            }

            fn process_task_chunk(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
                step: fn64_render::RenderTaskStep,
            ) -> Result<fn64_render::RenderTaskChunkStatus, RenderError> {
                self.steps.lock().unwrap().push(step);
                Ok(match step {
                    fn64_render::RenderTaskStep::Start => {
                        fn64_render::RenderTaskChunkStatus::Continue(
                            fn64_render::RenderTaskContinuation::new(1),
                        )
                    }
                    fn64_render::RenderTaskStep::Resume(token) if token.get() == 1 => {
                        fn64_render::RenderTaskChunkStatus::Continue(
                            fn64_render::RenderTaskContinuation::new(2),
                        )
                    }
                    fn64_render::RenderTaskStep::Resume(token) if token.get() == 2 => {
                        fn64_render::RenderTaskChunkStatus::Complete
                    }
                    fn64_render::RenderTaskStep::Resume(token) => panic!(
                        "unexpected or multiply consumed continuation token {}",
                        token.get()
                    ),
                })
            }

            fn task_chunking(&self) -> fn64_render::RenderTaskChunking {
                fn64_render::RenderTaskChunking::Resumable
            }

            fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
                fn64_render::DpFullSyncStatus::NotReached
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        const HEADER_OFF: usize = 0x40;
        const YIELD_DATA: u32 = 0x200;
        const YIELD_SIZE: u32 = 0x80;
        crate::load_rom(Vec::new());
        let mut rdram = vec![0u8; 0x400];
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
        let steps = Arc::new(Mutex::new(Vec::new()));
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(
            Box::new(ChunkedBackend {
                steps: Arc::clone(&steps),
            }),
            rdram.len(),
        );

        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [fn64_render::RenderTaskStep::Start]
        );
        assert!(with_host(|host| host.device_fabric.snapshot().sp_busy));
        assert_eq!(
            crate::next_device_deadline(),
            Some(crate::sim_time()),
            "a running continuation must remain visible to the host pump"
        );

        unsafe { osSpTaskYield_recomp(rdram.as_mut_ptr(), &mut ctx) };
        crate::advance_virtual_time(8);
        assert_eq!(
            steps.lock().unwrap().len(),
            1,
            "SIG0 must win before token consumption"
        );
        assert_ne!(
            crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED,
            0
        );
        unsafe { osSpTaskYielded_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(ctx.r2, u64::from(fn64_runtime::OS_TASK_YIELDED));

        unsafe { osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx) };
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx) };
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [
                fn64_render::RenderTaskStep::Start,
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(1))
            ]
        );
        crate::advance_virtual_time(16);
        crate::advance_virtual_time(17);
        assert_eq!(
            steps.lock().unwrap().as_slice(),
            [
                fn64_render::RenderTaskStep::Start,
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(1)),
                fn64_render::RenderTaskStep::Resume(fn64_render::RenderTaskContinuation::new(2))
            ],
            "each backend continuation is consumed exactly once"
        );
        with_host(|host| {
            let snapshot = host.device_fabric.snapshot();
            assert!(!snapshot.sp_busy);
            assert!(!snapshot.dp_busy);
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
    /// `fn64_render_reference::ReferenceBackend`, a real F3DEX2-family display
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
        use fn64_render_reference::{gbi, ReferenceBackend};

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
        // A second ordered primitive forces the production ReferenceBackend
        // through one real continuation/resume boundary at this ABI seam.
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
        backend.create(&RenderConfig::ntsc(64, 64)).unwrap();
        prepare_renderer_rdram(&mut rdram);
        set_render_backend(Box::new(backend), rdram.len());
        unsafe { osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx as *mut _) };
        assert!(
            hle_render_needs_progress(),
            "the first real ReferenceBackend operation must retain its backend-owned continuation"
        );
        advance_hle_render_task();
        assert!(
            !hle_render_needs_progress(),
            "the second real ReferenceBackend operation must consume the continuation exactly once"
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
        // error-free StartGo result above, this closes the loop end-to-end:
        // the seam call really executed the real decode+rasterize path on
        // this fixture, not a silent no-op.
        let mut direct = ReferenceBackend::new().with_clear_color([1, 2, 3, 255]);
        direct.create(&RenderConfig::ntsc(64, 64)).unwrap();
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
        crate::set_trace_enabled(true);

        let mut rdram = vec![0u8; 128];
        let header_off = 0x30usize;
        rdram[header_off..header_off + 4].copy_from_slice(&fn64_runtime::M_AUDTASK.to_ne_bytes());

        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + header_off as u64;
        let prior_starts = crate::copy_trace()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    fn64_runtime::TraceKind::TaskSubmit {
                        task_kind: fn64_runtime::TaskKind::Audio,
                        ..
                    }
                )
            })
            .count();
        admit_synthetic_hle_task(&mut rdram, header_off, &mut ctx);
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Audio,
                            ..
                        }
                    )
                })
                .count(),
            prior_starts,
            "audio admission alone cannot claim task execution"
        );
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
        assert_eq!(
            crate::copy_trace()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        fn64_runtime::TraceKind::TaskSubmit {
                            task_kind: fn64_runtime::TaskKind::Audio,
                            ..
                        }
                    )
                })
                .count(),
            prior_starts + 1,
            "audio StartGo must emit exactly one execution-qualified task trace"
        );
        crate::advance_virtual_time(8);
    }

    #[test]
    fn os_sp_task_start_go_dispatches_a_direct_4k_audio_image_without_rspboot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);

        unsafe extern "C" fn fake_ucode(_rdram: *mut u8, _task_offset: u32) -> u32 {
            CALLED.store(true, Ordering::SeqCst);
            0
        }

        const HEADER: usize = 0x40;
        const IMAGE: usize = 0x200;
        crate::load_rom(Vec::new());
        unsafe { set_audio_ucode_fn(fake_ucode) };
        CALLED.store(false, Ordering::SeqCst);
        let mut rdram = vec![0u8; IMAGE + fn64_runtime::RSP_MEMORY_BANK_SIZE];
        for (field, value) in [
            (0x00, fn64_runtime::M_AUDTASK),
            (0x08, 0x8000_0000 | IMAGE as u32),
            (0x0c, fn64_runtime::RSP_MEMORY_BANK_SIZE as u32),
            (0x10, 0xA000_0000 | IMAGE as u32),
            (0x14, fn64_runtime::RSP_MEMORY_BANK_SIZE as u32),
        ] {
            rdram[HEADER + field..HEADER + field + 4].copy_from_slice(&value.to_ne_bytes());
        }
        // A direct ucode is allowed to terminate with BREAK. If StartGo
        // mistakes this image for rspboot, this first word takes the existing
        // loud "BREAK before DMA-loaded ucode" trap instead of calling HLE.
        rdram[IMAGE..IMAGE + 4].copy_from_slice(&0x0000_000du32.to_ne_bytes());
        with_host(|host| {
            host.runtime_rdram = rdram.as_mut_ptr();
            host.runtime_rdram_len = rdram.len();
        });
        let mut ctx = ctx_zeroed();
        ctx.r4 = 0x8000_0000 + HEADER as u64;
        unsafe {
            osSpTaskLoad_recomp(rdram.as_mut_ptr(), &mut ctx);
            osSpTaskStartGo_recomp(rdram.as_mut_ptr(), &mut ctx);
        }

        assert!(
            CALLED.load(Ordering::SeqCst),
            "a complete direct IMEM audio image must enter its registered HLE backend"
        );
        assert_eq!(with_executor(|exec| exec.task_log().audio_count()), 1);
        crate::advance_virtual_time(1);
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
        with_host(|host| {
            host.rsp_task_lineages.insert(
                HEADER_OFF as u32,
                RspTaskLineage {
                    original_header: OsTaskHeader {
                        flags: FLAGS,
                        ucode_data: OLD_UCODE_DATA,
                        ucode_data_size: OLD_UCODE_DATA_SIZE,
                        yield_data_ptr: YIELD_DATA,
                        yield_data_size: YIELD_DATA_SIZE,
                        ..Default::default()
                    },
                    data_identity: None,
                    phase: RspTaskLineagePhase::Running,
                },
            );
        });

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
        assert_eq!(
            crate::host_evidence_snapshot().rsp_task_lineages[0].phase,
            RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized
        );
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

    #[test]
    fn audio_digest_capture_distinguishes_unrequested_empty_and_real_pcm() {
        set_audio_digest_capture(false);
        assert_eq!(copy_audio_digest_bytes(), None);

        let mut rdram = vec![0u8; 8];
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(RdramAddr::from_offset(0), 0x1234);
        view.write_u16(RdramAddr::from_offset(2), 0xfffe);
        set_audio_rdram_len(rdram.len());
        set_audio_digest_capture(true);
        unsafe { deliver_ai_buffer(rdram.as_mut_ptr(), 0, 4) };
        assert_eq!(
            copy_audio_digest_bytes(),
            Some(vec![0x34, 0x12, 0xfe, 0xff])
        );
        set_audio_digest_capture(false);
    }

    #[derive(Clone, Copy)]
    enum ScheduledRawDpcReply {
        BackendError,
        WrongTransaction,
        WrongQuantum,
        WrongCursor,
        Continue(fn64_render::DpFullSyncStatus),
        Complete(fn64_render::DpFullSyncStatus),
    }

    struct ScheduledRawDpcBackend {
        replies: std::collections::VecDeque<ScheduledRawDpcReply>,
        calls: usize,
        steps: Vec<fn64_render::RawDpcStep>,
    }

    impl ScheduledRawDpcBackend {
        fn new(replies: impl IntoIterator<Item = ScheduledRawDpcReply>) -> Self {
            Self {
                replies: replies.into_iter().collect(),
                calls: 0,
                steps: Vec::new(),
            }
        }
    }

    impl RenderBackend for ScheduledRawDpcBackend {
        fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
            Ok(())
        }

        fn observe_non_rdp_write16(
            &mut self,
            _write: fn64_render::NonRdpWrite16,
        ) -> fn64_render::NonRdpWrite16Disposition {
            fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
        }

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &fn64_render::OsTask,
            _output_addr: u32,
        ) -> Result<FrameStatus, RenderError> {
            unreachable!("scheduled raw-DPC test cannot dispatch an HLE task")
        }

        fn raw_dpc_progression(&self) -> fn64_render::RawDpcProgression {
            fn64_render::RawDpcProgression::Acknowledged
        }

        fn process_rdp_command_chunk(
            &mut self,
            rdram: &mut [u8],
            quantum: fn64_render::RawDpcQuantum,
            step: fn64_render::RawDpcStep,
        ) -> Result<fn64_render::RawDpcChunkAck, RenderError> {
            self.calls += 1;
            self.steps.push(step);
            rdram[self.calls - 1] = 0xa0 + self.calls as u8;
            let reply = self
                .replies
                .pop_front()
                .expect("scheduled raw-DPC test exhausted backend replies");
            let mut ack = fn64_render::RawDpcChunkAck {
                transaction: quantum.request.transaction,
                quantum: quantum.request.quantum,
                committed_through: quantum.request.end,
                status: fn64_render::RawDpcChunkStatus::Continue(
                    fn64_render::RenderRawDpcContinuation::new(91),
                ),
                full_sync: fn64_render::DpFullSyncStatus::NotReached,
            };
            match reply {
                ScheduledRawDpcReply::BackendError => Err(RenderError::Backend {
                    backend: "scheduled-raw-dpc-test",
                    reason: "injected failure after shadow mutation".into(),
                }),
                ScheduledRawDpcReply::WrongTransaction => {
                    ack.transaction = fn64_runtime::DpcTransactionId::from_submission(
                        fn64_runtime::DpcSubmission {
                            token: quantum.request.transaction.get() + 1,
                            source: quantum.request.start.source(),
                            start: quantum.request.start.address(),
                            end: quantum.request.end.address(),
                        },
                    );
                    Ok(ack)
                }
                ScheduledRawDpcReply::WrongQuantum => {
                    ack.quantum =
                        fn64_runtime::DpcQuantumId::new(quantum.request.quantum.get() + 1);
                    Ok(ack)
                }
                ScheduledRawDpcReply::WrongCursor => {
                    ack.committed_through = quantum.request.start;
                    Ok(ack)
                }
                ScheduledRawDpcReply::Continue(full_sync) => {
                    ack.full_sync = full_sync;
                    Ok(ack)
                }
                ScheduledRawDpcReply::Complete(full_sync) => {
                    ack.status = fn64_render::RawDpcChunkStatus::Complete;
                    ack.full_sync = full_sync;
                    Ok(ack)
                }
            }
        }

        fn present(
            &mut self,
            _request: fn64_render::PresentRequest<'_>,
        ) -> Result<(), RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[UcodeId] {
            &[]
        }
    }

    fn scheduled_raw_dpc_transaction() -> ScheduledRawDpcTransaction {
        let source = fn64_runtime::DpcSubmissionSource::Rdram;
        let cursor = |address| fn64_runtime::DpcCursor::new(source, address).unwrap();
        ScheduledRawDpcTransaction::new(
            fn64_runtime::DpcScheduledExecution::new(
                fn64_runtime::DpcSubmission {
                    token: 5,
                    source,
                    start: 0x100,
                    end: 0x110,
                },
                fn64_runtime::Cycles::new(0),
                vec![
                    fn64_runtime::DpcQuantumPlan {
                        at: fn64_runtime::Cycles::new(2),
                        id: fn64_runtime::DpcQuantumId::new(1),
                        start: cursor(0x100),
                        end: cursor(0x108),
                    },
                    fn64_runtime::DpcQuantumPlan {
                        at: fn64_runtime::Cycles::new(3),
                        id: fn64_runtime::DpcQuantumId::new(2),
                        start: cursor(0x108),
                        end: cursor(0x110),
                    },
                ],
            )
            .unwrap(),
        )
    }

    #[test]
    fn malformed_or_failed_raw_dpc_backend_result_poisons_without_publication() {
        use ScheduledRawDpcReply::{BackendError, WrongCursor, WrongQuantum, WrongTransaction};

        for reply in [BackendError, WrongTransaction, WrongQuantum, WrongCursor] {
            let mut transaction = scheduled_raw_dpc_transaction();
            let start = transaction.cursor();
            let mut backend = ScheduledRawDpcBackend::new([reply]);
            let mut live = vec![0x11; 16];
            let error = transaction
                .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
                .unwrap_err();
            match error {
                ScheduledRawDpcError::Backend(error) => {
                    assert!(error.to_string().contains("injected failure"));
                }
                ScheduledRawDpcError::Schedule(_) => {}
                ScheduledRawDpcError::UnidentifiedFullSync => {
                    panic!("identity-mismatch cases cannot fail FullSync validation")
                }
            }
            assert_eq!(live, vec![0x11; 16]);
            assert_eq!(transaction.cursor(), start);
            assert_eq!(transaction.continuation(), None);
            assert_eq!(
                transaction.phase(),
                fn64_runtime::DpcScheduledPhase::Poisoned
            );
            assert!(matches!(
                transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
                Err(ScheduledRawDpcError::Schedule(
                    fn64_runtime::DpcScheduleError::Poisoned
                ))
            ));
            assert_eq!(backend.calls, 1, "a poisoned transaction cannot retry work");
        }
    }

    #[test]
    fn raw_dpc_status_must_match_remaining_schedule_before_publication() {
        let mut early = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([ScheduledRawDpcReply::Complete(
            fn64_render::DpFullSyncStatus::NotReached,
        )]);
        let mut live = vec![0x22; 16];
        assert!(matches!(
            early.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::EarlyComplete { .. }
            ))
        ));
        assert_eq!(live, vec![0x22; 16]);
        assert_eq!(early.phase(), fn64_runtime::DpcScheduledPhase::Poisoned);

        let mut final_continue = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::NotReached),
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::NotReached),
        ]);
        let mut live = vec![0x33; 16];
        final_continue
            .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
            .unwrap();
        let first_image = live.clone();
        assert!(matches!(
            final_continue.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::FinalContinue { .. }
            ))
        ));
        assert_eq!(
            live, first_image,
            "the malformed final shadow stays private"
        );
        assert_eq!(
            final_continue.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
        assert_eq!(final_continue.continuation(), None);
    }

    #[test]
    fn second_raw_dpc_backend_error_preserves_only_the_first_commit() {
        let mut transaction = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::Reached),
            ScheduledRawDpcReply::BackendError,
        ]);
        let mut live = vec![0x55; 16];

        transaction
            .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0)
            .unwrap();
        let first_image = live.clone();
        assert_eq!(first_image[0], 0xa1);
        assert_eq!(first_image[1], 0x55);
        assert_eq!(
            transaction.continuation(),
            Some(fn64_render::RenderRawDpcContinuation::new(91))
        );
        assert_eq!(
            transaction.full_sync(),
            fn64_render::DpFullSyncStatus::Reached
        );

        assert!(matches!(
            transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Backend(_))
        ));
        assert_eq!(
            backend.steps,
            vec![
                fn64_render::RawDpcStep::Start,
                fn64_render::RawDpcStep::Resume(fn64_render::RenderRawDpcContinuation::new(91)),
            ]
        );
        assert_eq!(live, first_image, "the second shadow stays unpublished");
        assert_eq!(live[1], 0x55, "the second backend mutation stayed private");
        assert_eq!(transaction.continuation(), None);
        assert_eq!(
            transaction.full_sync(),
            fn64_render::DpFullSyncStatus::Reached,
            "a rejected later quantum cannot erase prior committed FullSync evidence"
        );
        assert_eq!(
            transaction.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
        assert!(matches!(
            transaction.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0),
            Err(ScheduledRawDpcError::Schedule(
                fn64_runtime::DpcScheduleError::Poisoned
            ))
        ));
        assert_eq!(backend.calls, 2, "poison prevents a second-quantum retry");
    }

    #[test]
    fn raw_dpc_full_sync_is_identified_and_sticky_across_valid_commits() {
        let mut transaction = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([
            ScheduledRawDpcReply::Continue(fn64_render::DpFullSyncStatus::Reached),
            ScheduledRawDpcReply::Complete(fn64_render::DpFullSyncStatus::NotReached),
        ]);
        let mut live = vec![0; 16];
        for expected_phase in [
            fn64_runtime::DpcScheduledPhase::Scheduled,
            fn64_runtime::DpcScheduledPhase::Complete,
        ] {
            assert!(matches!(
                transaction
                    .advance_one(
                        fn64_runtime::Cycles::new(10),
                        &mut backend,
                        &mut live,
                        0,
                    )
                    .unwrap(),
                ScheduledRawDpcAdvance::Committed {
                    phase,
                    full_sync: fn64_render::DpFullSyncStatus::Reached,
                    ..
                } if phase == expected_phase
            ));
        }

        let mut unidentified = scheduled_raw_dpc_transaction();
        let mut backend = ScheduledRawDpcBackend::new([ScheduledRawDpcReply::Continue(
            fn64_render::DpFullSyncStatus::Unidentified,
        )]);
        let mut live = vec![0x44; 16];
        assert!(matches!(
            unidentified.advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,),
            Err(ScheduledRawDpcError::UnidentifiedFullSync)
        ));
        assert_eq!(live, vec![0x44; 16]);
        assert_eq!(
            unidentified.phase(),
            fn64_runtime::DpcScheduledPhase::Poisoned
        );
    }

    #[test]
    fn synthetic_scheduled_dpc_keeps_renderer_continuation_in_the_abi_lane() {
        struct Backend;

        impl RenderBackend for Backend {
            fn create(&mut self, _cfg: &RenderConfig) -> Result<(), RenderError> {
                Ok(())
            }

            fn observe_non_rdp_write16(
                &mut self,
                _write: fn64_render::NonRdpWrite16,
            ) -> fn64_render::NonRdpWrite16Disposition {
                fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
            }

            fn process_task(
                &mut self,
                _rdram: &mut [u8],
                _rsp_memory: &mut fn64_runtime::RspMemory,
                _task: &fn64_render::OsTask,
                _output_addr: u32,
            ) -> Result<FrameStatus, RenderError> {
                unreachable!("synthetic raw-DPC test cannot dispatch an HLE task")
            }

            fn raw_dpc_progression(&self) -> fn64_render::RawDpcProgression {
                fn64_render::RawDpcProgression::Acknowledged
            }

            fn process_rdp_command_chunk(
                &mut self,
                rdram: &mut [u8],
                quantum: fn64_render::RawDpcQuantum,
                step: fn64_render::RawDpcStep,
            ) -> Result<fn64_render::RawDpcChunkAck, RenderError> {
                let index = usize::try_from(quantum.request.start.address() - 0x100).unwrap();
                rdram[index] = quantum.request.quantum.get() as u8;
                let status = match step {
                    fn64_render::RawDpcStep::Start => fn64_render::RawDpcChunkStatus::Continue(
                        fn64_render::RenderRawDpcContinuation::new(77),
                    ),
                    fn64_render::RawDpcStep::Resume(token) if token.get() == 77 => {
                        fn64_render::RawDpcChunkStatus::Complete
                    }
                    fn64_render::RawDpcStep::Resume(token) => {
                        panic!("ABI supplied stale raw-DPC continuation {}", token.get())
                    }
                };
                Ok(fn64_render::RawDpcChunkAck {
                    transaction: quantum.request.transaction,
                    quantum: quantum.request.quantum,
                    committed_through: quantum.request.end,
                    status,
                    full_sync: fn64_render::DpFullSyncStatus::NotReached,
                })
            }

            fn present(
                &mut self,
                _request: fn64_render::PresentRequest<'_>,
            ) -> Result<(), RenderError> {
                Ok(())
            }

            fn resize(&mut self, _w: u32, _h: u32) {}

            fn supported_ucodes(&self) -> &[UcodeId] {
                &[]
            }
        }

        let source = fn64_runtime::DpcSubmissionSource::Rdram;
        let cursor = |address| fn64_runtime::DpcCursor::new(source, address).unwrap();
        let execution = fn64_runtime::DpcScheduledExecution::new(
            fn64_runtime::DpcSubmission {
                token: 5,
                source,
                start: 0x100,
                end: 0x110,
            },
            fn64_runtime::Cycles::new(0),
            vec![
                fn64_runtime::DpcQuantumPlan {
                    at: fn64_runtime::Cycles::new(2),
                    id: fn64_runtime::DpcQuantumId::new(1),
                    start: cursor(0x100),
                    end: cursor(0x108),
                },
                fn64_runtime::DpcQuantumPlan {
                    at: fn64_runtime::Cycles::new(3),
                    id: fn64_runtime::DpcQuantumId::new(2),
                    start: cursor(0x108),
                    end: cursor(0x110),
                },
            ],
        )
        .unwrap();
        let mut backend = Backend;
        let mut transaction = ScheduledRawDpcTransaction::new(execution);
        let mut live = vec![0u8; 16];

        for (expected_at, expected_phase) in [
            (2, fn64_runtime::DpcScheduledPhase::Scheduled),
            (3, fn64_runtime::DpcScheduledPhase::Complete),
        ] {
            assert_eq!(
                transaction
                    .advance_one(fn64_runtime::Cycles::new(10), &mut backend, &mut live, 0,)
                    .unwrap(),
                ScheduledRawDpcAdvance::Committed {
                    at: fn64_runtime::Cycles::new(expected_at),
                    phase: expected_phase,
                    full_sync: fn64_render::DpFullSyncStatus::NotReached,
                }
            );
        }
        assert_eq!(live[0], 1);
        assert_eq!(live[8], 2);
        assert_eq!(transaction.continuation(), None);
        assert_eq!(
            transaction.phase(),
            fn64_runtime::DpcScheduledPhase::Complete
        );
    }
}
