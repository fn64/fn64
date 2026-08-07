use super::*;

thread_local! {
    /// The single registered graphics backend, if the shell/harness has
    /// called `set_render_backend`. `RefCell` (not `Cell`, unlike
    /// `AUDIO_UCODE_FN`) because a `Box<dyn RenderBackend>` is not `Copy`
    /// and needs `&mut` access across calls to drive its own internal
    /// state (`create`/`process_task`/`present`).
    pub(crate) static RENDER_BACKEND: RefCell<Option<Box<dyn RenderBackend>>> = const { RefCell::new(None) };
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
    pub(crate) static RDRAM_LEN: Cell<usize> = const { Cell::new(0) };
    /// The most recent `RenderBackend::process_task` error, if any,
    /// stringified -- a harness/test observability hook (see
    /// `GFX_RENDER_NOTE`'s doc comment for why this isn't surfaced as a
    /// MIPS-side fault instead).
    pub(crate) static RENDER_LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The scheduler owns only this opaque token and immutable task identity;
    /// renderer-local stacks/state remain behind `RenderBackend`.
    pub(crate) static HLE_RENDER_CONTINUATION: RefCell<Option<HleRenderContinuation>> = const { RefCell::new(None) };

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
    pub(crate) static AUDIO_TASK_DUMP: Cell<AudioTaskDumpState> =
        const { Cell::new(AudioTaskDumpState { seen: 0, dumped: false }) };
    pub(crate) static AUDIO_PCM_DUMPED: Cell<bool> = const { Cell::new(false) };
    pub(crate) static AUDIO_PCM_STREAM_DUMP: RefCell<Option<AudioStreamDump>> = const { RefCell::new(None) };
    pub(crate) static AUDIO_OUTPUT_STATS: Cell<AudioOutputStats> = const { Cell::new(AudioOutputStats::new()) };
    pub(crate) static AUDIO_DIGEST_CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };

    /// Coarse wall-time attribution for the rs-lane OoT performance harness.
    /// Kept behind an environment flag so ordinary execution pays no
    /// `Instant::now` cost at task or executor boundaries.
    pub(crate) static PHASE_TIMING: Cell<bool> =
        Cell::new(std::env::var_os("FN64_PHASE_TIMING").is_some());
    pub(crate) static EXECUTOR_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static EXECUTOR_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_LLE_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_LLE_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_LLE_RSP_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static GFX_LLE_RDP_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_DISPATCH_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_DISPATCH_CALLS: Cell<u64> = const { Cell::new(0) };
    /// VI retrace presentation (`present_render_backend` -> `RenderBackend::
    /// present`, which for the reference backend is the whole `vi::scanout`
    /// filter chain). Timed separately from `GFX_NS` because presentation is
    /// a PER-FIELD cost that does not scale with scene complexity, while
    /// graphics task dispatch is per submit: conflating them hides a fixed
    /// per-frame overhead inside a variable one.
    pub(crate) static VI_PRESENT_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static VI_PRESENT_CALLS: Cell<u64> = const { Cell::new(0) };
    /// Non-graphics LLE (the audio ucode under `LleAccuracy`). Distinct from
    /// `AUDIO_DISPATCH_NS`, which only covers the `Translated` callback path
    /// -- a lane WM2000 does not take, so that counter reads zero while the
    /// interpreter runs thousands of tasks.
    pub(crate) static AUDIO_LLE_NS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_LLE_CALLS: Cell<u64> = const { Cell::new(0) };
    pub(crate) static AUDIO_LLE_RSP_NS: Cell<u64> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HleRenderContinuationPhase {
    Running,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HleRenderContinuation {
    pub(crate) phase: HleRenderContinuationPhase,
    pub(crate) token: fn64_render::RenderTaskContinuation,
    pub(crate) task_addr: RdramAddr,
    pub(crate) task: OsTaskHeader,
    pub(crate) rdram: usize,
    pub(crate) output_addr: u32,
    pub(crate) dp_full_sync: fn64_render::DpFullSyncStatus,
    pub(crate) completion_latency: u64,
    pub(crate) rspboot_state: Option<fn64_audio::rsp::runtime::RspMachineState>,
}

pub(crate) fn merge_dp_full_sync(
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

pub(crate) fn retain_running_hle_continuation(
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
        let completion_latency = pending.completion_latency;
        let task_addr = pending.task_addr;
        let rspboot_state = pending.rspboot_state.clone();
        let dp_full_sync = pending.dp_full_sync;
        HLE_RENDER_CONTINUATION.with(|cell| cell.replace(Some(pending)));
        crate::pi::write_live_sp_status(fn64_runtime::SP_SET_YIELDED);
        crate::pi::finish_live_rcp_task(
            rcp_completion_plan(dp_full_sync, "chunk-boundary HLE yield"),
            completion_latency,
        )
        .unwrap_or_else(|error| panic!("chunk-boundary HLE yield completion: {error}"));
        commit_rsp_hle_compatibility(task_addr, rspboot_state);
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
            commit_rsp_hle_compatibility(pending.task_addr, pending.rspboot_state);
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
            commit_rsp_hle_compatibility(pending.task_addr, pending.rspboot_state);
        }
        fn64_render::RenderTaskChunkStatus::NeedsLle { .. } => {
            panic!("resumed HLE continuation requested LLE after committing an earlier chunk")
        }
    }
}

pub(crate) fn hle_render_needs_progress() -> bool {
    HLE_RENDER_CONTINUATION.with(|cell| {
        cell.borrow()
            .as_ref()
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

/// The registered backend's cumulative host-stream delivery counters, or
/// `None` when no backend is registered or the backend does not track them.
///
/// `max_callback_gap_us` is measured on cpal's own realtime thread, which
/// makes it the one statistic reachable from here that observes the HOST
/// rather than the guest. That is what lets a frame-latency investigation
/// separate the two: if the emulation thread stalls for seconds while the
/// audio callback keeps its cadence, the stall is ours; if both stop together,
/// it is the machine's.
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
    pub gfx_lle_ns: u64,
    pub gfx_lle_calls: u64,
    pub gfx_lle_rsp_ns: u64,
    pub gfx_lle_rdp_ns: u64,
    pub audio_dispatch_ns: u64,
    pub audio_dispatch_calls: u64,
    /// VI retrace presentation: the reference backend's `vi::scanout` filter
    /// chain, once per emulated field. Not nested inside `gfx_ns`.
    pub vi_present_ns: u64,
    pub vi_present_calls: u64,
    /// Audio (non-graphics) LLE microcode interpretation. Not nested inside
    /// `gfx_ns`; disjoint from `audio_dispatch_ns`.
    pub audio_lle_ns: u64,
    pub audio_lle_calls: u64,
    pub audio_lle_rsp_ns: u64,
}

pub fn phase_timing() -> PhaseTiming {
    PhaseTiming {
        executor_ns: EXECUTOR_NS.with(Cell::get),
        executor_calls: EXECUTOR_CALLS.with(Cell::get),
        gfx_ns: GFX_NS.with(Cell::get),
        gfx_calls: GFX_CALLS.with(Cell::get),
        gfx_lle_ns: GFX_LLE_NS.with(Cell::get),
        gfx_lle_calls: GFX_LLE_CALLS.with(Cell::get),
        gfx_lle_rsp_ns: GFX_LLE_RSP_NS.with(Cell::get),
        gfx_lle_rdp_ns: GFX_LLE_RDP_NS.with(Cell::get),
        audio_dispatch_ns: AUDIO_DISPATCH_NS.with(Cell::get),
        audio_dispatch_calls: AUDIO_DISPATCH_CALLS.with(Cell::get),
        vi_present_ns: VI_PRESENT_NS.with(Cell::get),
        vi_present_calls: VI_PRESENT_CALLS.with(Cell::get),
        audio_lle_ns: AUDIO_LLE_NS.with(Cell::get),
        audio_lle_calls: AUDIO_LLE_CALLS.with(Cell::get),
        audio_lle_rsp_ns: AUDIO_LLE_RSP_NS.with(Cell::get),
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
/// is a caller bug. This compatibility entry point
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
/// this crate accepts only at the public BREAK discriminant (`0`) -- a plain
/// `u32` return keeps the generated module's enum out of this ABI.
pub type AudioUcodeFn = unsafe extern "C" fn(*mut u8, u32) -> u32;

thread_local! {
    /// The out-of-tree translated audio ucode paired atomically with the
    /// installed ROM's `Translated` policy. All other policies own `None`.
    static AUDIO_UCODE_FN: Cell<Option<AudioUcodeFn>> = const { Cell::new(None) };
}

pub(crate) fn reset_audio_task_execution_for_rom() {
    with_host(|host| {
        host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::Reset;
        host.audio_task_execution = AudioTaskExecutionPolicy::Unconfigured;
        host.audio_task_execution_admitted = false;
        host.audio_task_execution_started = false;
    });
    AUDIO_UCODE_FN.with(|cell| cell.set(None));
}

pub(crate) fn install_audio_task_execution(policy: AudioTaskExecutionPolicy, callback: Option<AudioUcodeFn>) {
    assert_no_legacy_env_vars();
    assert_ne!(policy, AudioTaskExecutionPolicy::Unconfigured);
    assert_eq!(
        matches!(policy, AudioTaskExecutionPolicy::Translated { .. }),
        callback.is_some(),
        "translated audio execution must own exactly one callback"
    );
    with_host(|host| {
        assert!(
            host.rom_installed,
            "audio task execution policy requires an installed ROM"
        );
        assert_eq!(
            host.audio_task_execution,
            AudioTaskExecutionPolicy::Unconfigured,
            "audio task execution policy was already installed for this ROM as {:?}",
            host.audio_task_execution
        );
        assert!(
            !host.audio_task_execution_admitted && !host.audio_task_execution_started,
            "audio task execution policy cannot be installed after an audio task was admitted"
        );
        host.audio_task_execution = policy;
    });
    AUDIO_UCODE_FN.with(|cell| cell.set(callback));
}

/// Atomically register a translated audio ucode and its exact host artifact
/// identity. The identity distinguishes executable configurations but does not
/// prove a correspondence with arbitrary live IMEM; release evidence uses LLE.
///
/// # Safety
/// `f` must have the real `RspExitReason(uint8_t*, uint32_t)` signature
/// RSPRecomp generates and must remain valid for the process's lifetime
/// (true for a file-scope C function with static storage duration, which is
/// what RSPRecomp emits). `artifact_sha256` must identify the exact translated
/// module containing `f`.
pub unsafe fn set_translated_audio_ucode(f: AudioUcodeFn, artifact_sha256: [u8; 32]) {
    assert_ne!(
        artifact_sha256, [0; 32],
        "translated audio artifact identity cannot be all zero"
    );
    install_audio_task_execution(
        AudioTaskExecutionPolicy::Translated { artifact_sha256 },
        Some(f),
    );
}

/// Execute every admitted audio microcode instruction through the clean-room
/// RSP interpreter.
pub fn set_audio_task_lle_accuracy() {
    install_audio_task_execution(AudioTaskExecutionPolicy::LleAccuracy, None);
}

/// Explicitly skip audio synthesis for render-only diagnostic probes.
/// Fixed-cycle release evidence rejects this policy.
pub fn set_audio_task_diagnostic_skip() {
    install_audio_task_execution(AudioTaskExecutionPolicy::DiagnosticSkip, None);
}

pub(crate) fn require_audio_task_execution_policy(
    task_addr: RdramAddr,
    header: &OsTaskHeader,
) -> AudioTaskExecutionPolicy {
    debug_assert_eq!(header.task_type, M_AUDTASK);
    let policy = with_host(|host| {
        host.audio_task_execution_started = true;
        host.audio_task_execution
    });
    if policy == AudioTaskExecutionPolicy::Unconfigured {
        let context = format!(
            "task={:#010x} type={} ucode={:#010x}/size={:#x}",
            task_addr.offset(),
            header.task_type,
            header.ucode,
            header.ucode_size
        );
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Audio,
            "audio.task.missing-execution-policy",
            context.clone(),
            Some(fn64_runtime::Cycles::new(crate::sim_time())),
            fn64_runtime::UnsupportedDisposition::LoudTrap,
        );
        panic!("audio.task.missing-execution-policy: {context}")
    }
    policy
}

/// Read the public libultra manual's documented `OSTask_t` field layout
/// (see `osSpTaskYielded_recomp`'s doc comment for the byte offsets) out of
/// `rdram` at `base` (already an rdram-relative offset, not a raw vram/gpr
/// value -- callers translate first via `RdramAddr`).
///
/// # Safety
/// `rdram` must be valid for at least `base + 0x40` bytes.
pub(crate) unsafe fn read_os_task_header(rdram: *mut u8, base: usize) -> OsTaskHeader {
    // Native byte order, matching MEM_W's real semantics -- see
    // `read_stack_word`'s doc comment for the full correction this wave made.
    let w = |off: usize| -> u32 {
        let mut b = [0u8; 4];
        unsafe { std::ptr::copy_nonoverlapping(rdram.add(base + off), b.as_mut_ptr(), 4) };
        u32::from_ne_bytes(b)
    };
    os_task_header_from_words(w)
}

pub(crate) fn os_task_header_from_words(mut w: impl FnMut(usize) -> u32) -> OsTaskHeader {
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
pub(crate) unsafe fn write_os_task_word(rdram: *mut u8, base: usize, field: usize, value: u32) {
    unsafe {
        std::ptr::copy_nonoverlapping(value.to_ne_bytes().as_ptr(), rdram.add(base + field), 4)
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingLoadedRspTask {
    pub(crate) task_addr: RdramAddr,
    pub(crate) header: OsTaskHeader,
    pub(crate) resumed_data_identity: Option<TaskMicrocodeDataIdentity>,
}

pub(crate) fn loaded_rsp_task_from_header(task_addr: RdramAddr, header: OsTaskHeader) -> PendingLoadedRspTask {
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
    PendingLoadedRspTask {
        task_addr,
        header,
        resumed_data_identity,
    }
}

pub(crate) fn retain_loaded_rsp_task(pending: PendingLoadedRspTask) {
    with_host(|host| {
        let loaded = LoadedRspTask {
            task_addr: pending.task_addr,
            admission_generation: host.next_rsp_task_admission_generation.advance(),
            header: pending.header,
            resumed_data_identity: pending.resumed_data_identity,
        };
        if loaded.header.task_type == M_AUDTASK {
            host.audio_task_execution_admitted = true;
        }
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
            lineage.admission_generation = loaded.admission_generation;
            lineage.phase = RspTaskLineagePhase::ResumeLoaded;
        }
        // RSP has one admitted task image. A later successful Load replaces
        // that image and therefore replaces the sole unconsumed token too.
        host.loaded_rsp_task = Some(loaded);
    });
}

pub(crate) fn take_loaded_rsp_task(task_addr: RdramAddr) -> LoadedRspTask {
    with_host(|host| {
        let loaded = host.loaded_rsp_task.as_ref().unwrap_or_else(|| {
            panic!(
                "osSpTaskStartGo_recomp: task {:#010x} has no unconsumed osSpTaskLoad admission",
                task_addr.offset()
            )
        });
        assert_eq!(
            loaded.task_addr,
            task_addr,
            "osSpTaskStartGo_recomp: task {:#010x} does not own the loaded RSP task token for {:#010x}",
            task_addr.offset(),
            loaded.task_addr.offset()
        );
        host.loaded_rsp_task
            .take()
            .expect("loaded RSP task was present above")
    })
}

pub(crate) fn retain_started_rsp_task_lineage(
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
        // The other direction of the same exclusion: a raw SP kick owns the
        // interpreter without any lineage, so the Running scan above cannot see
        // it. Without this a task would start on top of a live raw kick and
        // inherit its scalar/VU state as if it were its own.
        if let RspInterpreterStateEvidenceSnapshot::InFlight {
            owner: owner @ RspInterpreterOwner::RawKick { .. },
        } = host.rsp_interpreter_state
        {
            panic!(
                "osSpTaskStartGo_recomp: task {:#010x} cannot start while {} owns the interpreter",
                loaded.task_addr.offset(),
                owner.describe()
            );
        }
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
                lineage.data_identity,
                data_identity,
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
                    admission_generation: loaded.admission_generation,
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

pub(crate) fn retire_running_rsp_task_lineage(task_addr: RdramAddr, operation: &'static str) {
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

pub(crate) fn retire_rsp_task_lineage_after_synchronous_result(task_addr: RdramAddr, operation: &'static str) {
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
    if header.task_type != M_AUDTASK {
        with_executor(|exec| exec.start_task(header));
    }

    // Kicking the RSP is where the selected task effect runs, so the work
    // happens here -- this is the path OoT uses (Load then
    // StartGo, never the yield path) for BOTH its gfx and its audio tasks.
    // Dispatch before scheduling completion so the work is done by the time
    // the scheduler is woken. A graphics task rasterizes; an audio task
    // runs its registered ucode + forwards samples (previously dispatched only
    // from the never-taken yield path -- same latent bug the gfx path hit).
    let resumes_hle_continuation = HLE_RENDER_CONTINUATION.with(|cell| {
        let retained = cell.borrow();
        retained.as_ref().is_some_and(|pending| {
            assert_eq!(
                pending.phase,
                HleRenderContinuationPhase::Suspended,
                "osSpTaskStartGo_recomp: cannot start while an HLE continuation is running"
            );
            assert_eq!(
                pending.task_addr, task_addr,
                "osSpTaskStartGo_recomp: task does not own the retained renderer continuation"
            );
            true
        })
    });
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
            AdmittedTaskImageShape::DirectImem => {
                let lle_machine_state = if resumes_hle_continuation {
                    resume_direct_hle_phase(task_addr);
                    None
                } else {
                    Some(Box::new(unsafe {
                        begin_direct_hle_phase(rdram, task_addr)
                    }))
                };
                AdmittedHleEntry::DirectImem {
                    task: header,
                    lle_machine_state,
                }
            }
        })
    } else {
        None
    };
    let hle_header = hle_entry.as_ref().map_or(header, AdmittedHleEntry::task);
    let hle_compatibility_state = hle_entry
        .as_ref()
        .and_then(AdmittedHleEntry::hle_compatibility_state);
    let graphics_policy = GRAPHICS_TASK_EXECUTION_POLICY.with(Cell::get);
    let diagnostic_full_sync = is_gfx
        .then(|| diagnostic_graphics_dp_full_sync(graphics_policy))
        .flatten();
    let dp_full_sync = if let Some(full_sync) = diagnostic_full_sync {
        full_sync
    } else if is_gfx && graphics_policy == GraphicsTaskExecutionPolicy::LleAccuracy {
        let entry = hle_entry
            .take()
            .expect("gfx accuracy LLE requires an admitted HLE entry");
        let pre_ucode_steps = entry.pre_ucode_steps();
        let microcode_data = initial_microcode_data
            .expect("gfx accuracy LLE requires admitted microcode-data identity");
        let lle = unsafe {
            dispatch_lle_task(
                rdram,
                Some(task_addr),
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
                        rspboot_state: hle_compatibility_state.clone(),
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
                        Some(task_addr),
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
        match audio_policy.expect("audio task must preflight its execution policy") {
            AudioTaskExecutionPolicy::Unconfigured => {
                unreachable!("audio execution policy preflight rejects unconfigured tasks")
            }
            AudioTaskExecutionPolicy::Translated { .. } => {
                let callback = AUDIO_UCODE_FN
                    .with(Cell::get)
                    .expect("translated audio execution lost its atomically registered callback");
                unsafe { dispatch_audio_task(rdram, o, &hle_header, callback) };
                with_executor(|exec| exec.start_task(header));
                fn64_render::DpFullSyncStatus::NotReached
            }
            AudioTaskExecutionPolicy::LleAccuracy => {
                let entry = hle_entry
                    .take()
                    .expect("audio accuracy LLE requires an admitted task entry");
                let pre_ucode_steps = entry.pre_ucode_steps();
                let lle = unsafe {
                    dispatch_lle_task(
                        rdram,
                        Some(task_addr),
                        false,
                        entry.into_lle_machine_state(),
                        None,
                        None,
                    )
                };
                crate::pi::start_live_rcp_task_with_latency(
                    rcp_completion_plan(lle.dp_full_sync, "audio accuracy LLE"),
                    pre_ucode_steps.saturating_add(lle.steps),
                )
                .unwrap_or_else(|error| {
                    panic!("osSpTaskStartGo_recomp audio accuracy LLE completion: {error}")
                });
                with_executor(|exec| exec.start_task(header));
                retire_rsp_task_lineage_after_synchronous_result(task_addr, "audio accuracy LLE");
                return;
            }
            AudioTaskExecutionPolicy::DiagnosticSkip => fn64_render::DpFullSyncStatus::NotReached,
        }
    } else {
        let lle = unsafe { dispatch_lle_task(rdram, Some(task_addr), false, None, None, None) };
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
    // LLEAccuracy commits an exact terminal image inside dispatch_lle_task.
    // Optimized HLE has no post-ucode scalar/VU result, so only a successful
    // backend + device scheduling path may publish its explicitly labeled
    // rspboot-entry compatibility image. A backend panic leaves InFlight and
    // the next task traps rather than disguising a partial renderer effect.
    commit_rsp_hle_compatibility(task_addr, hle_compatibility_state);
    retire_rsp_task_lineage_after_synchronous_result(task_addr, "known HLE task");
}

pub(crate) fn rcp_completion_plan(
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

pub(crate) fn diagnostic_graphics_dp_full_sync(
    policy: GraphicsTaskExecutionPolicy,
) -> Option<fn64_render::DpFullSyncStatus> {
    (policy == GraphicsTaskExecutionPolicy::DiagnosticSkip)
        .then_some(fn64_render::DpFullSyncStatus::Reached)
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

/// A guest started the RSP by clearing HALT through raw `SP_STATUS` MMIO
/// rather than through the libultra task shim.
///
/// This runs the same LLE interpreter the task lane runs; the only difference
/// is that interpreter ownership is a [`RspInterpreterOwner::RawKick`] rather
/// than a task lineage, because there is no `OSTask` to key on. SP_PC and IMEM
/// are already latched in the fabric, so `pc` is diagnostic only.
///
/// This is what makes an unknown ROM able to drive the RSP at all: a guest
/// running its own libultra kicks the RSP itself, so `osSpTaskStartGo` never
/// needs to be identified for the RSP to run.
pub(crate) unsafe fn dispatch_raw_rsp_start(rdram: *mut u8, pc: u32) {
    assert!(
        !rdram.is_null(),
        "raw SP_STATUS clear-halt at SP_PC {pc:#06x} has no registered process RDRAM"
    );
    let lle = unsafe { dispatch_lle_task(rdram, None, false, None, None, None) };
    crate::pi::start_live_rcp_task_with_latency(
        rcp_completion_plan(lle.dp_full_sync, "raw SP kick"),
        lle.steps,
    )
    .unwrap_or_else(|error| {
        panic!("raw SP_STATUS clear-halt at SP_PC {pc:#06x} completion: {error}")
    });
    // No lineage to retire: a raw kick never entered `rsp_task_lineages`.
}
