//! The audio-priority bounded VI join contract, named.
//!
//! The 2026-08-31 fix made the VI-edge join **nonblocking under
//! audio-priority presentation**: `try_advance_async_lle_render_task` waits
//! at most `audio_priority_join_budget()` for the in-flight raw-DPC worker,
//! and on timeout leaves the batch running, counts one `VI_JOIN_SKIPS`, and
//! returns `true` so `present_render_backend` re-presents the previous
//! field instead of stalling guest time -- and with it audio production --
//! on the renderer.
//!
//! Before this module `rg 'vi_join|audio_priority'` matched no test in this
//! crate at all: the load-bearing behavior had a mechanism, a counter, and a
//! knob, but nothing that would notice if any of the three stopped working.
//!
//! **These tests drive the real path, not a model of it.** The backend is a
//! genuine `fn64_render_wgpu::WgpuBackend` registered through the production
//! `set_threaded_render_backend`, so `execute_raw_dpc_task_batch` really runs
//! on the real dedicated `fn64-rdp` worker thread and the join really goes
//! through `ThreadedRenderBackend::poll_raw_dpc_task_batch_bounded`'s
//! `recv_timeout`. The only test-owned part is a `GatedRawDpcBackend`
//! wrapper that makes the worker's own `execute_raw_dpc_task_batch` block on
//! a barrier, which is how a test creates the "worker has not replied yet"
//! condition without needing a genuinely slow GPU. That is the same
//! technique `lifecycle.rs`'s own `DeferredWriteBackend` tests use.
//!
//! ## Two contract facts these tests pin, both of which are easy to get
//! wrong from the brief's one-line description
//!
//! 1. **The skip counter and the re-present live in different functions.**
//!    `VI_JOIN_SKIPS` increments inside
//!    `task_dispatch::try_advance_async_lle_render_task` (lifecycle.rs). The
//!    re-present is *not* there: it is the early `return` at the top of
//!    `task_dispatch::present_render_backend`, which bails out whenever
//!    `audio_priority_vi_presentation() && async_lle_render_pending()`. So a
//!    skipped join is observed as **`RenderBackend::present` never being
//!    called at all** for that retrace.
//!
//! 2. **"The previous field is re-presented" therefore means no new field is
//!    minted.** `present_render_backend` mints a fresh
//!    `PresentedSourceFieldGeneration` only after a successful backend
//!    `present`. Taking the early return skips that entirely, so
//!    `take_presented_source_field()` yields `None` and the host window keeps
//!    displaying the RGBA bytes it already owns. The "same digest" the brief
//!    asks for is thus asserted as *the field bytes the host still holds are
//!    unchanged, because no new ones were produced* -- and, independently,
//!    that guest RDRAM at the framebuffer still holds the prior completed
//!    frame, which is the reason a re-present is a clean frame rather than a
//!    tear.
//!
//! ## Timing assertions and flake risk
//!
//! Test 1 asserts an **upper** bound on how long the VI edge takes: the
//! installed budget plus generous slack. An upper bound can only flake if
//! the machine descheduled the thread for longer than the slack, so the
//! slack is deliberately many times the budget (see `TIMEOUT_SLACK`). The
//! assertion that actually carries the contract is the skip count, which is
//! exact and timing-independent given a worker held on a barrier: the worker
//! provably cannot reply, so `recv_timeout` provably times out.
//!
//! Test 2 makes **no** upper-bound timing assertion at all. It releases the
//! worker before the join and asserts zero skips, which is the correct
//! statement -- asserting that a real GPU batch finishes inside a few
//! milliseconds would be a genuine flake, and is not what the contract says.
//!
//! ## Budget installation and process scope
//!
//! `INSTALLED_BUDGET_MS` is a process-global `AtomicU64` and
//! `audio_priority_join_budget()` latches its value into a `OnceLock` at the
//! first join. Under nextest -- this workspace's authoritative gate, one
//! process per test (`.config/nextest.toml`) -- each test therefore gets its
//! own fresh statics and cannot disturb any other. To stay correct under a
//! shared-process `cargo test` as well, these tests never assume a
//! particular latched budget: test 1 reads the budget back through the
//! elapsed bound it asserts (which holds for the 3ms default and for any
//! installed value up to `MAX_TOLERATED_BUDGET_MS`), and both tests compare
//! **deltas** of `audio_priority_vi_join_skips()` rather than absolute
//! values. `AUDIO_PRIORITY_VI_PRESENTATION` is a plain flag with no latch,
//! so each test restores it in teardown.

use super::*;

/// The budget these tests install. Small enough that test 1 is fast, large
/// enough that it is not confusable with zero (which is the "unset"
/// sentinel `set_audio_priority_join_budget_ms` deliberately ignores).
const TEST_BUDGET_MS: u64 = 5;

/// The largest budget this file's elapsed assertion tolerates. If a shared
/// process latched some other budget first, an elapsed time under this bound
/// still proves the join was *bounded* -- which is the contract -- rather
/// than proving the exact number, which the latch may not let us choose.
const MAX_TOLERATED_BUDGET_MS: u64 = 50;

/// Slack over the budget for the elapsed upper bound. Deliberately large
/// relative to the budget: this is a scheduler-noise allowance on a
/// contended CI box, not a precision measurement. The contract being tested
/// is "bounded, not unbounded", and an unbounded join would block until the
/// worker was released -- which in test 1 is *after* the assertion, so a
/// regression fails by deadlock or by an elapsed time far beyond any slack,
/// never by a few milliseconds of jitter.
const TIMEOUT_SLACK_MS: u64 = 250;

/// The budget test 2 installs. Large on purpose: that test's claim is "a
/// reply that arrives inside the budget is joined, not skipped", so the
/// budget must comfortably cover the microseconds between the worker
/// finishing its batch body and its completion landing in the channel.
/// Making it generous removes a race; it does not weaken the claim, because
/// the reply genuinely does arrive inside it.
///
/// Installing a *different* budget here than test 1 relies on nextest's
/// one-process-per-test model (`.config/nextest.toml`), under which
/// `audio_priority_join_budget()`'s `OnceLock` latch is per test. Under a
/// shared-process `cargo test` whichever test joins first would latch its
/// value for both -- which is why test 1's elapsed bound tolerates any
/// budget up to `MAX_TOLERATED_BUDGET_MS` and both tests assert skip
/// *deltas* rather than absolute counts, so the pair still passes in either
/// order and in either runner.
const JOINED_BUDGET_MS: u64 = 30;

/// A `FullBackend` that delegates every method to a real `WgpuBackend` and
/// changes exactly one thing: `execute_raw_dpc_task_batch` blocks on a
/// channel until the test releases it.
///
/// This is how a test manufactures the "renderer worker has not replied yet"
/// state that the audio-priority join exists to handle. Nothing else is
/// faked -- the batch really executes on the real worker thread, against the
/// real backend, and produces the real result once released. The wrapper
/// tells no lies: it only delays.
struct GatedRawDpcBackend {
    inner: fn64_render_wgpu::WgpuBackend,
    /// Signalled by the worker once it is inside
    /// `execute_raw_dpc_task_batch`, so the test can be certain the
    /// blocked-worker state is real and not a race it merely hopes for.
    entered: std::sync::mpsc::SyncSender<()>,
    /// Blocks the worker until the test sends. A disconnect (test dropped
    /// the sender) releases it too, so a failing test cannot wedge the
    /// process in `ThreadedRenderBackend::drop`.
    release: std::sync::mpsc::Receiver<()>,
    /// Signalled once the real backend's batch body has returned, so a test
    /// can wait for the work itself without consuming the worker's
    /// completion (which would bypass the join under test).
    finished: std::sync::mpsc::SyncSender<()>,
}

impl fn64_render::RenderBackend for GatedRawDpcBackend {
    fn create(&mut self, cfg: &fn64_render::RenderConfig) -> Result<(), fn64_render::RenderError> {
        self.inner.create(cfg)
    }

    fn observe_non_rdp_write16(
        &mut self,
        write: fn64_render::NonRdpWrite16,
    ) -> fn64_render::NonRdpWrite16Disposition {
        self.inner.observe_non_rdp_write16(write)
    }

    fn deferred_non_rdp_write16_disposition(
        &self,
    ) -> Option<fn64_render::NonRdpWrite16Disposition> {
        self.inner.deferred_non_rdp_write16_disposition()
    }

    fn process_task(
        &mut self,
        rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &fn64_render::OsTask,
        output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, fn64_render::RenderError> {
        self.inner
            .process_task(rdram, rsp_memory, task, output_addr)
    }

    fn last_dp_full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.inner.last_dp_full_sync()
    }

    fn present(
        &mut self,
        request: fn64_render::PresentRequest<'_>,
    ) -> Result<(), fn64_render::RenderError> {
        self.inner.present(request)
    }

    fn take_presented_source_field(&mut self) -> fn64_render::PresentedSourceFieldAvailability {
        self.inner.take_presented_source_field()
    }

    fn take_presented_post_vi_field(&mut self) -> fn64_render::PresentedPostViFieldAvailability {
        self.inner.take_presented_post_vi_field()
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.inner.resize(w, h);
    }

    fn identify_microcode(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    ) -> Option<UcodeId> {
        self.inner.identify_microcode(imem)
    }

    fn identify_microcode_pair(
        &self,
        imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        data: fn64_render::MicrocodeDataImageIdentity,
    ) -> Option<UcodeId> {
        self.inner.identify_microcode_pair(imem, data)
    }

    fn supported_ucodes(&self) -> &[UcodeId] {
        self.inner.supported_ucodes()
    }
}

impl fn64_render::RawDpcBackend for GatedRawDpcBackend {
    fn raw_dpc_progression(&self) -> fn64_render::RawDpcProgression {
        self.inner.raw_dpc_progression()
    }

    fn raw_dpc_ir_capability(&self) -> fn64_render::RawDpcIrCapability {
        self.inner.raw_dpc_ir_capability()
    }

    fn raw_dpc_task_batch_capability(&self) -> fn64_render::RawDpcTaskBatchCapability {
        self.inner.raw_dpc_task_batch_capability()
    }

    fn plan_raw_dpc(
        &mut self,
        request: fn64_render::RawDpcPlanRequest,
    ) -> Result<fn64_render::PlannedRawDpcSubmission, fn64_render::RenderError> {
        self.inner.plan_raw_dpc(request)
    }

    fn plan_raw_dpc_task_batch(
        &mut self,
        requests: Vec<fn64_render::RawDpcPlanRequest>,
    ) -> Result<Vec<fn64_render::PlannedRawDpcSubmission>, fn64_render::RenderError> {
        self.inner.plan_raw_dpc_task_batch(requests)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: fn64_render::BoundSubmittedRawDpc,
    ) -> Result<fn64_render::BackendPreparedRawDpc, fn64_render::RenderError> {
        self.inner.execute_raw_dpc(bound)
    }

    /// The one delayed method. Blocking here is what puts the real
    /// `fn64-rdp` worker into the state the audio-priority join exists for:
    /// the emulation thread reaches a VI edge while the worker still owns
    /// the backend and has sent nothing on its completion channel.
    ///
    /// Everything else on this impl is verbatim delegation, so the batch
    /// really is planned, executed, and published by the production
    /// `WgpuBackend`; the wrapper only chooses *when* the execution starts.
    fn execute_raw_dpc_task_batch(
        &mut self,
        bounds: Vec<fn64_render::BoundSubmittedRawDpc>,
    ) -> Result<Vec<fn64_render::BackendPreparedRawDpc>, fn64_render::RenderError> {
        // Tell the test the worker is genuinely inside the batch. Ignore a
        // send error: it only means the test already gave up and dropped the
        // receiver, in which case falling through to the release wait (which
        // will also be disconnected) unwedges the worker.
        let _ = self.entered.send(());
        // `Err` means the test dropped its sender; treat that as a release
        // rather than a panic so the worker always terminates.
        let _ = self.release.recv();
        let result = self.inner.execute_raw_dpc_task_batch(bounds);
        let _ = self.finished.send(());
        result
    }

    fn take_raw_dpc_task_batch_execution_mechanism(
        &mut self,
    ) -> Option<fn64_render::RawDpcTaskBatchExecutionMechanism> {
        self.inner.take_raw_dpc_task_batch_execution_mechanism()
    }

    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<fn64_render::ir::CompletedWrite> {
        self.inner.staged_guest_render_target_writes(submission)
    }

    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Vec<std::sync::Arc<[u8]>> {
        self.inner.committed_guest_render_target_bytes(submission)
    }

    fn take_raw_dpc_visual_target_snapshot(
        &mut self,
        submission: fn64_render::ir::SubmissionIdentity,
    ) -> Result<
        fn64_render::RawDpcVisualTargetSnapshotV1,
        fn64_render::RawDpcVisualTargetSnapshotRefusal,
    > {
        self.inner.take_raw_dpc_visual_target_snapshot(submission)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: fn64_render::ReadyRawDpcCommitCapsule<'_>,
    ) -> fn64_render::CommittedRawDpcOutcome {
        self.inner.publish_raw_dpc(publication)
    }
}

impl fn64_render::SettingsSink for GatedRawDpcBackend {}

/// Handles the test keeps on a registered [`GatedRawDpcBackend`].
struct GateHandles {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::SyncSender<()>,
    finished: std::sync::mpsc::Receiver<()>,
}

/// Register a real `WgpuBackend`, wrapped so its raw-DPC execution can be
/// gated, on the **threaded** worker -- the only registration under which
/// `poll_raw_dpc_task_batch_bounded` does anything (the `Local` arm returns
/// `None` unconditionally, which is a fact the tests below depend on and is
/// why neither uses `set_render_backend`).
fn register_gated_threaded_backend(rdram_len: usize) -> GateHandles {
    let (mut inner, session) =
        fn64_render_wgpu::WgpuBackend::try_new().expect("WgpuBackend::try_new is infallible here");
    let _ = inner.create(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    });
    // Opt in to source-field retention, so a successful retrace hands back
    // the actual RGBA bytes rather than `Unsupported`. Without this the
    // generation would still be minted (it is minted before the
    // Ready/Unsupported branch) but there would be no pixels to digest, and
    // "the previous field is re-presented" would be an assertion about a
    // counter instead of about an image.
    inner.enable_presented_source_field_delivery();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(1);
    set_threaded_render_backend(
        Box::new(GatedRawDpcBackend {
            inner,
            entered: entered_tx,
            release: release_rx,
            finished: finished_tx,
        }),
        rdram_len,
    );
    set_raw_dpc_session(session);
    GateHandles {
        entered: entered_rx,
        release: release_tx,
        finished: finished_rx,
    }
}

/// Undo everything these tests register, including the presentation policy
/// flag, so a shared-process run leaves no audio-priority state behind.
///
/// `INSTALLED_BUDGET_MS` is deliberately **not** reset to zero here: zero is
/// the "unset" sentinel and `audio_priority_join_budget()` has already
/// latched by this point, so storing it back would be a lie about what the
/// process will use. Under nextest the whole process ends here anyway.
fn teardown_audio_priority() {
    crate::task_dispatch::set_audio_priority_vi_presentation(false);
    teardown();
}

/// Drive the batch far enough that a raw-DPC worker owns the backend and a
/// `PendingRawDpcTaskBatch` is installed in `ASYNC_LLE_RENDER_CONTINUATION`,
/// exactly as `dispatch_lle_task` leaves it in production.
///
/// This is the real RSP-driven producer: an IMEM program writes
/// DPC_START/DPC_END twice through COP0, the same shape
/// `threaded_rsp_batch_joins_its_full_sync_receipt_to_the_real_dp_notification`
/// uses, so the two runs coalesce into one transactional task batch handed
/// to the worker thread.
///
/// Returns once the worker has confirmed it is inside
/// `execute_raw_dpc_task_batch`, so the caller is not racing the handoff.
fn start_gated_batch(rdram: &mut [u8], gate: &GateHandles) {
    const DPC_START: u32 = 0x100;
    const DPC_START_2: u32 = 0x200;
    // The proven two-run fixture. It must be TMEM-load bearing: the
    // production plan/execute path refuses a batch that reaches execution
    // with zero TMEM loads ("raw-DPC plan reached execution with zero TMEM
    // loads"), so a pure state-and-fill split is not an admissible batch and
    // could not be used to reach the worker at all. This is the same pair
    // `threaded_rsp_batch_joins_its_full_sync_receipt_to_the_real_dp_notification`
    // uses.
    let first_bytes = words_to_be_bytes(&one_load_block_words());
    let second_bytes = words_to_be_bytes(&one_load_block_then_full_sync_words());
    let dpc_end = DPC_START + first_bytes.len() as u32;
    let dpc_end_2 = DPC_START_2 + second_bytes.len() as u32;
    let task_addr = RdramAddr::from_offset(0);

    with_host(|host| {
        let memory = host.device_fabric.rsp_memory_mut();
        for (offset, bytes) in [(DPC_START, &first_bytes), (DPC_START_2, &second_bytes)] {
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(
                        fn64_runtime::RspMemoryBank::Dmem,
                        u16::try_from(offset).unwrap(),
                    ),
                    bytes,
                )
                .expect("DMEM command write must succeed");
        }
        let program = [
            addiu_zero(2, DPC_START),
            mtc0(2, 8),
            addiu_zero(3, 0b10),
            mtc0(3, 11),
            addiu_zero(4, dpc_end),
            mtc0(4, 9),
            addiu_zero(2, DPC_START_2),
            mtc0(2, 8),
            addiu_zero(4, dpc_end_2),
            mtc0(4, 9),
            0x0000_000d,
        ];
        let bytes: Vec<u8> = program.into_iter().flat_map(u32::to_be_bytes).collect();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                &bytes,
            )
            .expect("IMEM program write must succeed");
    });
    install_running_task_lineage(task_addr, RspTaskAdmissionGeneration::first());

    let mut result = unsafe {
        dispatch_lle_task(
            rdram.as_mut_ptr(),
            Some(task_addr),
            false,
            None,
            None,
            None,
            None,
        )
    };
    let pending = result
        .pending_raw_dpc_task_batch
        .take()
        .expect("the threaded backend must return an owned pending batch");
    ASYNC_LLE_RENDER_CONTINUATION.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "no earlier continuation may be outstanding"
        );
        cell.replace(Some(pending));
    });

    gate.entered
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the gated raw-DPC worker must enter its batch");
    assert!(
        crate::task_dispatch::async_lle_render_pending(),
        "a gated worker must leave an async raw-DPC continuation pending"
    );
}

/// The VI-edge sequence the production host runs: `settle_renderer_before_vi`
/// decides the join, then the retrace drain presents. Both halves are
/// exercised because the skip counter lives in the first and the re-present
/// lives in the second.
fn drive_vi_edge() -> (bool, std::time::Duration) {
    let started = std::time::Instant::now();
    let still_pending = crate::task_dispatch::try_advance_async_lle_render_task(
        crate::RenderBatchJoinCause::ViVisibility,
    );
    let elapsed = started.elapsed();
    let presentation = ntsc_replicate_presentation(
        FILL_TARGET_ADDR,
        FILL_TARGET_WIDTH,
        FILL_TARGET_WIDTH,
        FILL_TARGET_HEIGHT,
    );
    crate::task_dispatch::present_render_backend(
        presentation,
        fn64_runtime::EmulatedInstant::new(presentation.noise_seed),
    );
    (still_pending, elapsed)
}

/// Present one real field, and return the guest bytes behind it.
///
/// This establishes the "previous field" that a skipped join must
/// re-present. It runs the full production retrace path, so the delivery it
/// mints is a genuine `PresentedSourceFieldDelivery::Ready` and the RGBA
/// bytes the host owns afterwards are the ones a re-present keeps showing.
fn present_previous_field(
    rdram: &mut [u8],
) -> (crate::vi::PresentedSourceFieldGeneration, Vec<u8>, Vec<u8>) {
    let contents = poison_fill_target(rdram);
    let presentation = ntsc_replicate_presentation(
        FILL_TARGET_ADDR,
        FILL_TARGET_WIDTH,
        FILL_TARGET_WIDTH,
        FILL_TARGET_HEIGHT,
    );
    crate::task_dispatch::present_render_backend(
        presentation,
        fn64_runtime::EmulatedInstant::new(presentation.noise_seed),
    );
    let delivered = crate::take_presented_source_field()
        .expect("the baseline retrace must mint a field to be re-presented later");
    assert!(
        matches!(
            delivered,
            crate::vi::PresentedSourceFieldDelivery::Ready { .. }
        ),
        "the baseline retrace must deliver a ready field"
    );
    let crate::vi::PresentedSourceFieldDelivery::Ready { field, .. } = &delivered else {
        unreachable!("just asserted Ready")
    };
    let rgba = field.rgba8().to_vec();
    assert!(
        !rgba.is_empty(),
        "a ready source field must carry the retrace's actual pixels"
    );
    (delivered.generation(), contents, rgba)
}

/// **Contract, half 1: a renderer worker that has not replied does not stall
/// the VI edge. The join returns inside the budget, counts exactly one skip,
/// and the previous field is re-presented.**
///
/// The worker is held inside `execute_raw_dpc_task_batch` on a channel for
/// the whole of the assertion window, so it provably cannot reply: this is
/// not a "probably slow enough" test. An unbounded join -- the pre-fix
/// behavior, and what a regression would restore -- would block right here
/// until the release at the end of the test, which is *after* every
/// assertion, so a regression cannot pass by luck.
///
/// The three assertions are the three halves of the fix:
///
/// - **bounded**: elapsed is under budget + slack (an upper bound, so
///   scheduler noise can only add, never subtract -- see the module comment
///   on flake risk);
/// - **counted**: `audio_priority_vi_join_skips()` goes up by exactly one,
///   which is exact and timing-independent;
/// - **re-presented**: `RenderBackend::present` is never reached, so no new
///   `PresentedSourceFieldGeneration` is minted past the baseline one and the
///   host keeps the RGBA bytes it already owns; and the guest bytes behind
///   that field are unchanged, which is *why* re-presenting it is clean.
#[test]
fn a_vi_edge_whose_renderer_has_not_replied_skips_within_budget_and_re_presents() {
    crate::load_rom(Vec::new());
    crate::task_dispatch::set_audio_priority_join_budget_ms(TEST_BUDGET_MS);
    crate::task_dispatch::set_audio_priority_vi_presentation(true);
    let mut rdram = rdram_with_texture_source();
    let gate = register_gated_threaded_backend(rdram.len());
    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
    });

    // The field the skipped retrace must keep showing.
    let (previous_generation, previous_bytes, previous_rgba) = present_previous_field(&mut rdram);

    let skips_before = crate::audio_priority_vi_join_skips();
    start_gated_batch(&mut rdram, &gate);
    let (still_pending, elapsed) = drive_vi_edge();

    // bounded.
    assert!(
        still_pending,
        "a VI join that timed out must report the batch still pending, so the caller re-presents"
    );
    let bound = std::time::Duration::from_millis(MAX_TOLERATED_BUDGET_MS + TIMEOUT_SLACK_MS);
    assert!(
        elapsed < bound,
        "the audio-priority VI join must return within its budget plus slack, not block on the \
         renderer: took {elapsed:?}, bound {bound:?}"
    );

    // counted.
    assert_eq!(
        crate::audio_priority_vi_join_skips(),
        skips_before + 1,
        "one timed-out VI join must count exactly one skip"
    );

    // re-presented: `present_render_backend` took its early return, so no
    // backend present happened and no new field generation was minted past
    // the baseline.
    assert!(
        crate::take_presented_source_field().is_none(),
        "a skipped join must not mint a new presented field -- the host keeps the bytes it owns"
    );
    assert!(
        crate::last_render_error().is_none(),
        "a skipped join must not surface a render error"
    );

    // ...and the guest bytes behind the field the host is still displaying
    // are byte-identical to what they were, which is what makes re-presenting
    // it a clean frame rather than a tear.
    assert_eq!(
        read_fill_target_logical(&rdram),
        previous_bytes,
        "a skipped join must leave the previous completed frame's guest bytes untouched"
    );
    assert!(
        crate::task_dispatch::async_lle_render_pending(),
        "a skipped join must leave the batch running rather than dropping it"
    );

    // Release the worker and join it, so teardown does not block in
    // `ThreadedRenderBackend::drop` and the batch's own effects are not left
    // half-applied for a later test in the same process.
    gate.release
        .send(())
        .expect("releasing the gated raw-DPC worker");
    crate::task_dispatch::advance_async_lle_render_task(crate::RenderBatchJoinCause::LaterGraphics);

    // The re-present really was a *re*-present of the same field: the next
    // successful retrace over the same unchanged guest memory mints a
    // strictly later generation but hands back **byte-identical pixels**.
    // That is the digest the brief asks for, taken from the real RGBA the
    // host would put on screen -- so "the previous field is re-presented" is
    // asserted as an image, not merely as an absent counter.
    let (next_generation, _, next_rgba) = present_previous_field(&mut rdram);
    assert!(
        next_generation > previous_generation,
        "the retrace after the skip must mint a later generation than the field it re-presented"
    );
    assert_eq!(
        next_rgba, previous_rgba,
        "the field surrounding a skipped join must be byte-identical -- the skip re-presents the \
         previous image rather than producing a new or torn one"
    );

    teardown_audio_priority();
}

/// **Contract, half 2: a renderer worker that replies inside the budget is
/// joined normally. Zero skips, and the new field is presented.**
///
/// This is the case that makes the fix a *bounded* join rather than a
/// blanket "never wait": light scenes finish inside the budget and show no
/// visual change at all. Without this half, deleting the join entirely and
/// always skipping would still pass half 1.
///
/// The worker is released **before** the VI edge and its batch body awaited,
/// so `recv_timeout` finds the result already queued or arriving within
/// microseconds. That is deliberate: asserting that a real GPU batch happens
/// to finish inside a few milliseconds would be a genuine flake and is not
/// what the contract claims. What is asserted is the consequence -- a worker
/// whose reply is available is joined, not skipped.
#[test]
fn a_vi_edge_whose_renderer_replied_in_budget_joins_with_no_skip_and_presents_the_new_field() {
    crate::load_rom(Vec::new());
    crate::task_dispatch::set_audio_priority_join_budget_ms(JOINED_BUDGET_MS);
    crate::task_dispatch::set_audio_priority_vi_presentation(true);
    let mut rdram = rdram_with_texture_source();
    let gate = register_gated_threaded_backend(rdram.len());
    with_host(|host| {
        host.runtime_rdram = rdram.as_mut_ptr();
        host.runtime_rdram_len = rdram.len();
    });

    let (previous_generation, _, _) = present_previous_field(&mut rdram);

    let skips_before = crate::audio_priority_vi_join_skips();
    start_gated_batch(&mut rdram, &gate);

    // Release first, and wait until the batch body has actually returned, so
    // the bounded poll below meets a reply that is already in flight. No
    // dependence on GPU speed.
    gate.release
        .send(())
        .expect("releasing the gated raw-DPC worker");
    wait_for_batch_body(&gate);

    let (still_pending, _elapsed) = drive_vi_edge();

    assert!(
        !still_pending,
        "a worker that already replied must be joined at the VI edge, not skipped"
    );
    assert_eq!(
        crate::audio_priority_vi_join_skips(),
        skips_before,
        "a join that met a ready worker must count no skip"
    );
    assert!(
        !crate::task_dispatch::async_lle_render_pending(),
        "a completed join must clear the async raw-DPC continuation"
    );

    // The new field: `present_render_backend` ran its full body this time,
    // so a fresh, strictly later generation was minted -- the opposite of
    // half 1's outcome on the same seam.
    let delivered = crate::take_presented_source_field()
        .expect("a joined VI edge must present, minting a new field generation");
    assert!(
        matches!(
            delivered,
            crate::vi::PresentedSourceFieldDelivery::Ready { .. }
        ),
        "the joined retrace must deliver a ready field, not an unsupported one"
    );
    assert!(
        delivered.generation() > previous_generation,
        "a joined retrace must present a NEW field, not re-present the previous generation"
    );
    assert!(
        crate::last_render_error().is_none(),
        "a joined present must not surface a render error"
    );

    teardown_audio_priority();
}

/// Block until the gated backend reports that its batch body has returned.
///
/// This is the wrapper's own `finished` signal, fired after the real
/// `WgpuBackend::execute_raw_dpc_task_batch` returns and therefore just
/// before the worker loop sends on its completion channel. Waiting on it
/// deliberately does **not** consume the completion -- consuming it here
/// would bypass the very join test 2 is about.
///
/// A handoff window remains between this signal and the worker's `send`.
/// Test 2 closes it not by racing but by installing a budget far larger than
/// any plausible scheduling delay (see `JOINED_BUDGET_MS`), so the bounded
/// `recv_timeout` waits out the window instead of skipping. That keeps the
/// test's claim honest: it asserts "a reply that arrives inside the budget
/// is joined", which is exactly the contract, rather than asserting a
/// particular wall-clock speed.
fn wait_for_batch_body(gate: &GateHandles) {
    gate.finished
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the released raw-DPC worker never finished its batch body");
}
