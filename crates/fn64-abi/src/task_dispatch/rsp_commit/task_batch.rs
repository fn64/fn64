use super::*;

pub(super) fn raw_dpc_task_batch_enabled() -> bool {
    !crate::diag_env::diag_env("FN64_RAW_DPC_TASK_BATCH").is_some_and(|value| value == "0")
}

fn task_guest_read_arena_enabled() -> bool {
    !crate::diag_env::diag_env("FN64_TASK_GUEST_READ_ARENA").is_some_and(|value| value == "0")
}

pub(super) fn renderer_copyback_batch_enabled() -> bool {
    !crate::diag_env::diag_env("FN64_RENDER_COPYBACK_BATCH").is_some_and(|value| value == "0")
}

pub(super) mod renderer_copyback_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    static CALLS: AtomicU64 = AtomicU64::new(0);
    static WRITES: AtomicU64 = AtomicU64::new(0);
    static BYTES: AtomicU64 = AtomicU64::new(0);
    static ELAPSED_NS: AtomicU64 = AtomicU64::new(0);

    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_RENDER_COPYBACK_CENSUS")
                .is_some_and(|value| value == "1")
        })
    }

    pub(in crate::task_dispatch::rsp_commit) fn started() -> Option<std::time::Instant> {
        enabled().then(std::time::Instant::now)
    }

    pub(in crate::task_dispatch::rsp_commit) fn record(
        started: Option<std::time::Instant>,
        writes: usize,
        bytes: usize,
    ) {
        let Some(started) = started else {
            return;
        };
        WRITES.fetch_add(u64::try_from(writes).unwrap_or(u64::MAX), Relaxed);
        BYTES.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Relaxed);
        ELAPSED_NS.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Relaxed,
        );
        let calls = CALLS.fetch_add(1, Relaxed) + 1;
        if calls % 100 == 0 {
            let elapsed_ns = ELAPSED_NS.load(Relaxed);
            eprintln!(
                "[renderer-copyback-census] calls={calls} writes={} bytes={} total_ms={:.3} ms/call={:.3}",
                WRITES.load(Relaxed),
                BYTES.load(Relaxed),
                elapsed_ns as f64 / 1_000_000.0,
                elapsed_ns as f64 / 1_000_000.0 / calls as f64,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskBatchPhaseRunningTotals {
    pub tasks: u64,
    pub members: u64,
    pub total_ns: u64,
    pub setup_ns: u64,
    pub plan_bind_ns: u64,
    pub guest_reads_ns: u64,
    pub staged_writes_ns: u64,
    pub copyback_ns: u64,
    pub publication_ns: u64,
}

/// Existing task-batch clocks, exposed without adding a read or timing site.
pub fn task_batch_phase_running_totals() -> Option<TaskBatchPhaseRunningTotals> {
    task_batch_phase_census::running_totals()
}

mod task_batch_phase_census {
    use super::TaskBatchPhaseRunningTotals;
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    pub(super) enum Phase {
        Setup,
        PlanBind,
        GuestReads,
        StagedWrites,
        Copyback,
        Publication,
    }

    impl Phase {
        const fn index(self) -> usize {
            match self {
                Self::Setup => 0,
                Self::PlanBind => 1,
                Self::GuestReads => 2,
                Self::StagedWrites => 3,
                Self::Copyback => 4,
                Self::Publication => 5,
            }
        }
    }

    const PHASE_COUNT: usize = 6;
    const LABELS: [&str; PHASE_COUNT] = [
        "setup",
        "plan-bind",
        "guest-reads",
        "staged-writes",
        "copyback",
        "publication",
    ];
    static TASKS: AtomicU64 = AtomicU64::new(0);
    static MEMBERS: AtomicU64 = AtomicU64::new(0);
    static TOTAL_NS: AtomicU64 = AtomicU64::new(0);
    static GUEST_READS: AtomicU64 = AtomicU64::new(0);
    static GUEST_READ_BYTES: AtomicU64 = AtomicU64::new(0);
    static UNIQUE_GUEST_RANGES: AtomicU64 = AtomicU64::new(0);
    static UNIQUE_GUEST_BYTES: AtomicU64 = AtomicU64::new(0);
    static PHASE_NS: [AtomicU64; PHASE_COUNT] = [const { AtomicU64::new(0) }; PHASE_COUNT];

    pub(super) fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_TASK_BATCH_PHASE_CENSUS")
                .is_some_and(|value| value == "1")
        })
    }

    pub(super) fn running_totals() -> Option<TaskBatchPhaseRunningTotals> {
        if !enabled() {
            return None;
        }
        Some(TaskBatchPhaseRunningTotals {
            tasks: TASKS.load(Relaxed),
            members: MEMBERS.load(Relaxed),
            total_ns: TOTAL_NS.load(Relaxed),
            setup_ns: PHASE_NS[Phase::Setup.index()].load(Relaxed),
            plan_bind_ns: PHASE_NS[Phase::PlanBind.index()].load(Relaxed),
            guest_reads_ns: PHASE_NS[Phase::GuestReads.index()].load(Relaxed),
            staged_writes_ns: PHASE_NS[Phase::StagedWrites.index()].load(Relaxed),
            copyback_ns: PHASE_NS[Phase::Copyback.index()].load(Relaxed),
            publication_ns: PHASE_NS[Phase::Publication.index()].load(Relaxed),
        })
    }

    pub(super) fn started() -> Option<std::time::Instant> {
        enabled().then(std::time::Instant::now)
    }

    pub(super) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
        let started = started();
        let result = operation();
        finish_phase(phase, started);
        result
    }

    pub(super) fn finish_phase(phase: Phase, started: Option<std::time::Instant>) {
        if let Some(started) = started {
            PHASE_NS[phase.index()].fetch_add(elapsed_ns(started), Relaxed);
        }
    }

    pub(super) fn note_guest_read_shape(
        reads: usize,
        bytes: u64,
        unique_ranges: usize,
        unique_bytes: u64,
    ) {
        if !enabled() {
            return;
        }
        GUEST_READS.fetch_add(
            u64::try_from(reads).expect("task-batch guest-read count exceeds u64"),
            Relaxed,
        );
        GUEST_READ_BYTES.fetch_add(bytes, Relaxed);
        UNIQUE_GUEST_RANGES.fetch_add(
            u64::try_from(unique_ranges).expect("task-batch unique guest-range count exceeds u64"),
            Relaxed,
        );
        UNIQUE_GUEST_BYTES.fetch_add(unique_bytes, Relaxed);
    }

    pub(super) fn finish(started: Option<std::time::Instant>, member_count: usize) {
        let Some(started) = started else {
            return;
        };
        TOTAL_NS.fetch_add(elapsed_ns(started), Relaxed);
        MEMBERS.fetch_add(
            u64::try_from(member_count).expect("task-batch member count exceeds u64"),
            Relaxed,
        );
        let tasks = TASKS.fetch_add(1, Relaxed) + 1;
        if tasks % 30 != 0 {
            return;
        }
        let members = MEMBERS.load(Relaxed);
        let total_ns = TOTAL_NS.load(Relaxed);
        eprintln!(
            "[task-batch-phase] tasks={tasks} members={members} total_ms={:.3} ms/task={:.3} ms/member={:.3}",
            millis(total_ns),
            millis(total_ns) / tasks as f64,
            millis(total_ns) / members as f64,
        );
        eprintln!(
            "[task-batch-phase] guest_reads={} bytes={} unique_ranges={} unique_bytes={} exact_duplicate_bytes={:.1}%",
            GUEST_READS.load(Relaxed),
            GUEST_READ_BYTES.load(Relaxed),
            UNIQUE_GUEST_RANGES.load(Relaxed),
            UNIQUE_GUEST_BYTES.load(Relaxed),
            duplicate_percentage(
                GUEST_READ_BYTES.load(Relaxed),
                UNIQUE_GUEST_BYTES.load(Relaxed),
            ),
        );
        for (label, elapsed) in LABELS.iter().zip(PHASE_NS.iter()) {
            let elapsed_ns = elapsed.load(Relaxed);
            eprintln!(
                "[task-batch-phase]   {label:<13} {:>9.3} ms  {:>7.3} ms/task",
                millis(elapsed_ns),
                millis(elapsed_ns) / tasks as f64,
            );
        }
        let measured_ns = PHASE_NS
            .iter()
            .map(|elapsed| elapsed.load(Relaxed))
            .sum::<u64>();
        let other_ns = total_ns.saturating_sub(measured_ns);
        eprintln!(
            "[task-batch-phase]   session+other {:>9.3} ms  {:>7.3} ms/task",
            millis(other_ns),
            millis(other_ns) / tasks as f64,
        );
    }

    fn elapsed_ns(started: std::time::Instant) -> u64 {
        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    fn millis(nanos: u64) -> f64 {
        nanos as f64 / 1_000_000.0
    }

    fn duplicate_percentage(total: u64, unique: u64) -> f64 {
        if total == 0 {
            return 0.0;
        }
        total.saturating_sub(unique) as f64 * 100.0 / total as f64
    }
}

pub(crate) struct PendingRawDpcTaskBatch {
    rdram: usize,
    reservation: fn64_runtime::device::ReservedDpcSubmissionBatch,
    /// The already-opened transaction for this batch's first member, carried
    /// with the single [`DpcAckGuard`] its `new` minted. The guard travels
    /// with the transaction so the eventual `validate_atomic_completion` can
    /// consume it -- there is no way to reconstruct one here.
    active: Option<(LiveDpcTransaction, DpcAckGuard)>,
    reserved: Vec<fn64_runtime::DpcSubmission>,
    pub(super) observations: Vec<RspRdpObservationKind>,
    full_sync_count: usize,
    member_count: usize,
    task_census_started: Option<std::time::Instant>,
    pub(crate) render_observation: Option<crate::render_observation::PendingRenderBatchObservation>,
    guest_task_observation: Option<(
        crate::render_observation::PendingGuestTaskObservation,
        crate::GuestTaskOutcome,
    )>,
    execution_mechanism: Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
    worker_span: Option<crate::render_observation::RenderWorkerSpan>,
    join_cause: Option<crate::RenderBatchJoinCause>,
    visual_evidence: Option<PendingRawDpcVisualBatchEvidence>,
}

struct PendingRawDpcVisualMemberEvidence {
    capture: fn64_render::OwnedRawDpcCapture,
    guest_read_plan: fn64_render::ir::DeferredGuestReadPlan,
    guest_reads: Vec<fn64_render::RawDpcVisualGuestReadV1>,
}

struct PendingRawDpcVisualBatchEvidence {
    identity: [u8; 32],
    members: Vec<PendingRawDpcVisualMemberEvidence>,
}

fn capture_raw_dpc_visual_vi_registers() -> fn64_render::ViScanoutRegisters {
    with_host(|host| crate::pi::read_vi_scanout_registers(&mut host.device_fabric))
}

impl PendingRawDpcTaskBatch {
    pub(crate) fn note_join(&mut self, cause: crate::RenderBatchJoinCause) {
        assert!(
            self.join_cause.replace(cause).is_none(),
            "raw-DPC guest task joined twice"
        );
        if let Some(observation) = self.render_observation.as_mut() {
            observation.note_join(cause);
        }
    }

    pub(crate) fn take_process_exit_guest_task_observation(
        &mut self,
    ) -> Option<crate::GuestTaskObservation> {
        let (observation, _) = self.guest_task_observation.take()?;
        let batch_id = self
            .render_observation
            .as_ref()
            .expect("guest task raw-DPC queue lost its paired batch observation")
            .batch_id();
        Some(observation.complete(
            crate::GuestTaskOutcome::AbandonedAtProcessExit,
            crate::emulated_now(),
            crate::GuestRspDispatchLane::Interpreted,
            crate::GuestTaskRdpExecution::Unavailable,
            crate::GuestTaskQueueIdentity::RawDpcTaskBatch { batch_id },
            crate::RenderBatchHostThread::RdpWorker,
            None,
        ))
    }
}

pub(super) enum RawDpcTaskBatchDispatch {
    Complete(
        fn64_render::DpFullSyncStatus,
        Vec<RspRdpObservationKind>,
        Option<crate::render_observation::CompletedRenderBatchObservation>,
        Option<crate::GuestTaskObservation>,
    ),
    Pending(PendingRawDpcTaskBatch),
}

pub(super) fn dispatch_raw_dpc_task_batch_via_session(
    rdram: *mut u8,
    runs: Vec<CoalescedDpRun>,
    deferred_dpc_history: &fn64_audio::rsp::runtime::RspDeferredDpcHistory,
    guest_task_observation: Option<(
        crate::render_observation::PendingGuestTaskObservation,
        crate::GuestTaskOutcome,
    )>,
) -> RawDpcTaskBatchDispatch {
    assert!(
        !runs.is_empty(),
        "a task batch must contain at least one DPC run"
    );
    let task_census_started = task_batch_phase_census::started();
    let setup_census_started = task_batch_phase_census::started();
    let member_count = runs.len();
    let real = unsafe { renderer_rdram_slice(rdram) };
    let mut structural_workloads =
        crate::render_observation::enabled().then(|| Vec::with_capacity(member_count));
    let requests: Vec<_> = runs
        .iter()
        .map(|run| {
            let workload = fn64_render::inspect_raw_rdp_structural_workload(&run.words)
                .unwrap_or_else(|error| panic!("task-batch structural scan: {error}"))
                .complete()
                .expect("a coalesced task run has no incomplete command tail");
            let sites = workload.sync_sites().full();
            if let Some(workloads) = structural_workloads.as_mut() {
                workloads.push(workload);
            }
            (
                if run.xbus {
                    fn64_runtime::DpcSubmissionSource::Dmem
                } else {
                    fn64_runtime::DpcSubmissionSource::Rdram
                },
                run.start,
                run.end,
                1u64.checked_add(u64::try_from(sites).expect("FullSync site count fits u64") * 2)
                    .expect("task-batch temporal span overflow"),
            )
        })
        .collect();
    let mut reservation = with_host(|host| {
        host.device_fabric
            .reserve_dpc_submission_batch_with_temporal_spans(&requests)
    })
    .unwrap_or_else(|error| panic!("reserving raw-DPC task batch: {error}"));
    let reserved = reservation.submissions().to_vec();

    let mut captures = Vec::with_capacity(runs.len());
    let mut observations = Vec::with_capacity(runs.len());
    let mut read_epoch_boundaries = Vec::with_capacity(runs.len());
    let mut timing_members = structural_workloads
        .as_ref()
        .map(|workloads| Vec::with_capacity(workloads.len()));
    let mut full_sync_count = 0usize;
    for (member_index, (run, reserved)) in runs.into_iter().zip(&reserved).enumerate() {
        if let Some(members) = timing_members.as_mut() {
            let member_ordinal =
                u32::try_from(member_index).expect("raw-DPC task member ordinal exceeds u32");
            members.push(crate::RenderBatchMemberTimingObservation {
                member_ordinal,
                transaction: fn64_runtime::DpcTransactionId::from_submission(*reserved),
                structural_workload: structural_workloads
                    .as_ref()
                    .expect("timing members require retained structural workloads")[member_index],
                dp_end_boundaries: run
                    .read_epoch_boundaries
                    .iter()
                    .map(|boundary| crate::RenderBatchDpEndBoundaryObservation {
                        command_end_byte_offset: boundary.command_end_byte_offset,
                        dp_end_step: boundary.dp_end_step,
                    })
                    .collect(),
            });
        }
        read_epoch_boundaries.push(run.read_epoch_boundaries);
        let submission = if run.xbus {
            fn64_render::OwnedRawDpcSubmission::from_xbus_payload(
                run.start,
                run.end,
                run.words
                    .iter()
                    .flat_map(|word| word.to_be_bytes())
                    .collect(),
            )
        } else {
            fn64_render::OwnedRawDpcSubmission::from_rdram_words(run.start, run.end, run.words)
        }
        .unwrap_or_else(|error| panic!("RSP DPC task-batch capture rejected: {error:?}"));
        let (capture, observation, sites) =
            build_task_batch_capture(real, SessionRawDpcSource { submission }, reserved.token);
        full_sync_count = full_sync_count
            .checked_add(sites)
            .expect("task FullSync count overflow");
        captures.push(capture);
        observations.push(observation);
    }
    assert!(
        full_sync_count <= 1,
        "one RSP task cannot reserve the single live DP FullSync slot more than once"
    );
    let visual_captures = crate::visual_checkpoint_observation::enabled().then(|| {
        let identity = fn64_render::raw_dpc_visual_task_batch_identity_v1(&captures)
            .unwrap_or_else(|error| panic!("raw-DPC visual task-batch identity: {error:?}"));
        (identity, captures.clone())
    });
    task_batch_phase_census::finish_phase(
        task_batch_phase_census::Phase::Setup,
        setup_census_started,
    );

    // The census denominator remains physical DPC submissions, even though
    // this path deliberately collapses their renderer transaction. Counting
    // the task as one would make the A/B's per-submission phase averages
    // incomparable precisely when batching is enabled.
    for _ in 0..member_count {
        crate::session_phase_census::note_submission();
    }
    let planned = RENDER_BACKEND.with(|backend_cell| {
        RAW_DPC_SESSION.with(|session_cell| {
            let mut backend = backend_cell.borrow_mut();
            let backend = backend
                .as_mut()
                .expect("task-batch raw-DPC backend vanished");
            let session = session_cell.borrow();
            let session = session
                .as_ref()
                .expect("task-batch raw-DPC session vanished");
            let plan_requests =
                task_batch_phase_census::timed(task_batch_phase_census::Phase::PlanBind, || {
                    captures
                        .into_iter()
                        .map(|capture| session.plan_request(capture))
                        .collect()
                });
            crate::session_phase_census::timed(crate::session_phase_census::Phase::Plan, || {
                backend
                    .backend_mut("plan_raw_dpc_task_batch")
                    .plan_raw_dpc_task_batch(plan_requests)
                    .unwrap_or_else(|error| panic!("plan_raw_dpc_task_batch: {error}"))
            })
        })
    });
    // Match the ordinary path's census boundary exactly: capturing logical
    // guest bytes is outside `Phase::Finalize`; only typed
    // `finalize_and_submit` validation is inside it. Including capture here
    // would charge batching for work both lanes perform and fabricate a
    // finalize regression in the A/B.
    if task_batch_phase_census::enabled() {
        let mut unique_ranges = std::collections::HashSet::new();
        let mut read_count = 0usize;
        let mut read_bytes = 0u64;
        for member in &planned {
            for read in member.guest_read_plan().reads() {
                read_count = read_count
                    .checked_add(1)
                    .expect("task-batch guest-read count overflow");
                read_bytes = read_bytes
                    .checked_add(u64::from(read.range().len()))
                    .expect("task-batch guest-read byte count overflow");
                unique_ranges.insert(read.range());
            }
        }
        let unique_bytes = unique_ranges.iter().fold(0u64, |total, range| {
            total
                .checked_add(u64::from(range.len()))
                .expect("task-batch unique guest-read byte count overflow")
        });
        task_batch_phase_census::note_guest_read_shape(
            read_count,
            read_bytes,
            unique_ranges.len(),
            unique_bytes,
        );
    }
    let use_guest_read_arena = task_guest_read_arena_enabled();
    let mut guest_read_arena = TaskGuestReadCaptureArena::new(real, deferred_dpc_history);
    let planned_with_reads =
        task_batch_phase_census::timed(task_batch_phase_census::Phase::GuestReads, || {
            planned
                .into_iter()
                .zip(read_epoch_boundaries)
                .map(|(member, boundaries)| {
                    let reads = if use_guest_read_arena {
                        guest_read_arena.capture(member.guest_read_plan(), &boundaries)
                    } else {
                        capture_task_batch_guest_reads(
                            &member,
                            real,
                            deferred_dpc_history,
                            &boundaries,
                        )
                    };
                    (member, reads)
                })
                .collect::<Vec<_>>()
        });
    let visual_evidence = visual_captures.map(|(identity, captures)| {
        assert_eq!(captures.len(), planned_with_reads.len());
        let members = captures
            .into_iter()
            .zip(planned_with_reads.iter())
            .map(
                |(capture, (planned, reads))| PendingRawDpcVisualMemberEvidence {
                    capture,
                    guest_read_plan: planned.guest_read_plan().clone(),
                    guest_reads: reads
                        .reads()
                        .iter()
                        .map(|read| {
                            fn64_render::RawDpcVisualGuestReadV1::new(
                                read.read(),
                                read.content_digest(),
                            )
                        })
                        .collect(),
                },
            )
            .collect();
        PendingRawDpcVisualBatchEvidence { identity, members }
    });
    let bounds =
        crate::session_phase_census::timed(crate::session_phase_census::Phase::Finalize, || {
            RAW_DPC_SESSION.with(|session_cell| {
                let mut session = session_cell.borrow_mut();
                let session = session
                    .as_mut()
                    .expect("task-batch raw-DPC session vanished");
                planned_with_reads
                    .into_iter()
                    .map(|(member, reads)| {
                        session
                            .finalize_and_submit(member, reads)
                            .unwrap_or_else(|error| {
                                panic!("task-batch finalize_and_submit: {error}")
                            })
                    })
                    .collect::<Vec<_>>()
            })
        });
    // The RDP becomes busy when the first command range is handed off, not
    // when host rasterization later finishes. Activate that exact reserved
    // identity before the worker starts; its cancellation guard remains on
    // the emulation thread while immutable render inputs move away.
    let first_expected = reserved[0];
    let first_active = with_host(|host| {
        host.device_fabric
            .activate_reserved_dpc_submission(&mut reservation)
    })
    .unwrap_or_else(|error| panic!("activating initial raw-DPC task member: {error}"))
    .expect("a completed RSP task cannot activate a frozen DPC reservation");
    assert_eq!(first_active, first_expected);
    let render_observation =
        crate::render_observation::begin(member_count, crate::emulated_now(), timing_members);
    assert!(
        guest_task_observation.is_none() || render_observation.is_some(),
        "guest-task raw-DPC observation lost its paired batch observation"
    );
    let prepared = RENDER_BACKEND.with(|backend_cell| {
        let mut backend = backend_cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("task-batch raw-DPC backend vanished");
        backend.start_raw_dpc_task_batch(bounds, render_observation.is_some())
    });
    if prepared.is_none() {
        maybe_complete_dpc_dma_after_worker_handoff(first_active.token);
    }
    let mut pending = PendingRawDpcTaskBatch {
        rdram: rdram as usize,
        reservation,
        active: Some(LiveDpcTransaction::new(first_active)),
        // `new` returns `(LiveDpcTransaction, DpcAckGuard)`, which is exactly
        // this field's tuple shape.
        reserved,
        observations,
        full_sync_count,
        member_count,
        task_census_started,
        render_observation,
        guest_task_observation,
        execution_mechanism: None,
        worker_span: None,
        join_cause: None,
        visual_evidence,
    };
    let Some(prepared) = prepared else {
        return RawDpcTaskBatchDispatch::Pending(pending);
    };
    if let Some(observation) = pending.render_observation.as_mut() {
        observation.set_worker_span(prepared.worker_span);
        observation.set_execution_mechanism(prepared.mechanism);
    }
    pending.worker_span = prepared.worker_span;
    pending.execution_mechanism = prepared.mechanism;
    let prepared = prepared
        .result
        .unwrap_or_else(|error| panic!("execute_raw_dpc_task_batch: {error}"));
    finish_raw_dpc_task_batch_via_session(prepared, pending)
}

/// Whether the immutable-worker-handoff DMA-idle experiment is enabled.
/// Only `1`, `true`, `yes`, and `on` (case-insensitive, trimmed) enable it.
pub fn early_dma_idle_experiment_enabled() -> bool {
    crate::diag_env::diag_env("FN64_EXPERIMENT_EARLY_DMA_IDLE").is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn maybe_complete_dpc_dma_after_worker_handoff(token: u64) {
    if !early_dma_idle_experiment_enabled() {
        return;
    }
    with_host(|host| {
        host.device_fabric
            .complete_dpc_dma_after_worker_handoff(token)
    })
    .unwrap_or_else(|error| panic!("completing DPC DMA after worker handoff: {error}"));
}

fn finish_raw_dpc_task_batch_via_session(
    prepared: Vec<fn64_render::BackendPreparedRawDpc>,
    mut pending: PendingRawDpcTaskBatch,
) -> RawDpcTaskBatchDispatch {
    let real = unsafe { renderer_rdram_slice(pending.rdram as *mut u8) };
    let PendingRawDpcTaskBatch {
        reservation,
        active,
        reserved,
        observations,
        full_sync_count,
        member_count,
        task_census_started,
        render_observation,
        guest_task_observation,
        execution_mechanism,
        worker_span,
        join_cause,
        visual_evidence,
        ..
    } = &mut pending;
    assert_eq!(prepared.len(), reserved.len());

    for (member_index, (member, expected_fabric)) in prepared
        .into_iter()
        .zip(reserved.iter().copied())
        .enumerate()
    {
        let submission = member.submission();
        let observation_started = render_observation
            .as_ref()
            .map(crate::render_observation::PendingRenderBatchObservation::phase_started);
        let staged_writes =
            task_batch_phase_census::timed(task_batch_phase_census::Phase::StagedWrites, || {
                RENDER_BACKEND.with(|cell| {
                    cell.borrow_mut()
                        .as_mut()
                        .expect("task-batch raw-DPC backend vanished")
                        .backend_mut("staged_guest_render_target_writes")
                        .staged_guest_render_target_writes(submission)
                })
            });
        if let (Some(observation), Some(started)) =
            (render_observation.as_mut(), observation_started)
        {
            observation.finish_staged_writes(started);
        }
        let copy_writes = staged_writes.clone();
        let observation_started = render_observation
            .as_ref()
            .map(crate::render_observation::PendingRenderBatchObservation::phase_started);
        let committed =
            crate::session_phase_census::timed(crate::session_phase_census::Phase::Commit, || {
                RAW_DPC_SESSION.with(|cell| {
                    let mut session = cell.borrow_mut();
                    let session = session
                        .as_mut()
                        .expect("task-batch raw-DPC session vanished");
                    if staged_writes.is_empty() {
                        session.commit_zero_guest_writes(member)
                    } else {
                        session.commit_guest_render_target_writes(member, staged_writes)
                    }
                    .unwrap_or_else(|error| panic!("task-batch guest commit: {error}"))
                })
            });
        if let (Some(observation), Some(started)) =
            (render_observation.as_mut(), observation_started)
        {
            observation.finish_commit(started);
        }
        if !copy_writes.is_empty() {
            let observation_started = render_observation
                .as_ref()
                .map(crate::render_observation::PendingRenderBatchObservation::phase_started);
            task_batch_phase_census::timed(task_batch_phase_census::Phase::Copyback, || {
                copy_committed_guest_writes(real, submission, &copy_writes);
            });
            if let (Some(observation), Some(started)) =
                (render_observation.as_mut(), observation_started)
            {
                observation.finish_copyback(started);
            }
        }

        let publication_census_started = task_batch_phase_census::started();
        let observation_started = render_observation
            .as_ref()
            .map(crate::render_observation::PendingRenderBatchObservation::phase_started);
        let (mut transaction, ack) = if let Some((transaction, ack)) = active.take() {
            assert_eq!(
                transaction
                    .token
                    .expect("initial active DPC transaction was unexpectedly disarmed"),
                expected_fabric.token,
                "initial active DPC identity diverged from the token bound into its render plan"
            );
            (transaction, ack)
        } else {
            let activated = with_host(|host| {
                host.device_fabric
                    .activate_reserved_dpc_submission(reservation)
            })
            .unwrap_or_else(|error| panic!("activating reserved raw-DPC submission: {error}"))
            .expect("a completed RSP task cannot activate a frozen DPC reservation");
            assert_eq!(
                activated, expected_fabric,
                "activated DPC identity diverged from the token bound into its render plan"
            );
            LiveDpcTransaction::new(activated)
        };
        transaction.validate_atomic_completion(ack);
        transaction.with_ready_commit(|ready| {
            RAW_DPC_SESSION.with(|session_cell| {
                let mut session = session_cell.borrow_mut();
                let session = session
                    .as_mut()
                    .expect("task-batch raw-DPC session vanished");
                let capsule = session
                    .seal_publication(committed, ready)
                    .unwrap_or_else(|error| panic!("task-batch seal_publication: {error}"));
                RENDER_BACKEND.with(|backend_cell| {
                    backend_cell
                        .borrow_mut()
                        .as_mut()
                        .expect("task-batch raw-DPC backend vanished")
                        .backend_mut("publish_raw_dpc")
                        .publish_raw_dpc(capsule)
                })
            })
        });
        record_rdp_renderer_publication_v1();
        if let Some(observation) = render_observation.as_mut() {
            observation.note_publication_cycle(crate::emulated_now());
        }
        task_batch_phase_census::finish_phase(
            task_batch_phase_census::Phase::Publication,
            publication_census_started,
        );
        if let (Some(observation), Some(started)) =
            (render_observation.as_mut(), observation_started)
        {
            observation.finish_publication(started);
        }
        if let Some(evidence) = visual_evidence.as_ref() {
            let member_evidence = &evidence.members[member_index];
            let member_ordinal =
                u32::try_from(member_index).expect("raw-DPC visual member ordinal exceeds u32");
            let target = RENDER_BACKEND.with(|cell| {
                cell.borrow_mut()
                    .as_mut()
                    .expect("task-batch raw-DPC backend vanished")
                    .backend_mut("take_raw_dpc_visual_target_snapshot")
                    .take_raw_dpc_visual_target_snapshot(submission)
            });
            let result = match target {
                Err(refusal) => Err(crate::RawDpcVisualCheckpointObservationRefusal::Target(
                    refusal,
                )),
                Ok(target) => {
                    let vi_registers = capture_raw_dpc_visual_vi_registers();
                    let memory_bytes =
                        u32::try_from(real.len()).expect("registered RDRAM allocation fits u32");
                    let post_copyback_rdram = fn64_runtime::RdramView::from_storage(real)
                        .read_logical_bytes(fn64_runtime::RdramAddr::from_offset(0), memory_bytes);
                    fn64_render::raw_dpc_visual_checkpoint_evidence_v1(
                        fn64_render::RawDpcVisualCheckpointInputV1 {
                            task_batch_identity: evidence.identity,
                            member_ordinal,
                            capture_source:
                                fn64_render::RawDpcVisualCaptureSource::ExactLiveTransaction,
                            capture: &member_evidence.capture,
                            guest_read_plan: &member_evidence.guest_read_plan,
                            guest_reads: &member_evidence.guest_reads,
                            vi_registers: Some(vi_registers),
                            target_address: target.target_address(),
                            target_width: target.target_width(),
                            target_height: target.target_height(),
                            target_format: target.target_format(),
                            target_device_bytes: target.target_device_bytes(),
                            coverage: target.coverage(),
                            post_copyback_rdram: &post_copyback_rdram,
                        },
                    )
                    .map_err(crate::RawDpcVisualCheckpointObservationRefusal::Checkpoint)
                }
            };
            crate::visual_checkpoint_observation::record(
                crate::RawDpcVisualCheckpointObservation {
                    task_batch_identity: evidence.identity,
                    member_ordinal,
                    result,
                },
            );
        }
    }
    assert_eq!(reservation.remaining(), 0);
    task_batch_phase_census::finish(*task_census_started, *member_count);
    let completed_guest_task_observation =
        guest_task_observation.take().map(|(observation, outcome)| {
            let batch_id = render_observation
                .as_ref()
                .expect("guest task raw-DPC queue lost its paired batch observation")
                .batch_id();
            let host_thread = if worker_span.is_some() {
                crate::RenderBatchHostThread::RdpWorker
            } else {
                crate::RenderBatchHostThread::Emulation
            };
            observation.complete(
                outcome,
                crate::emulated_now(),
                crate::GuestRspDispatchLane::Interpreted,
                crate::render_observation::rdp_execution_from_mechanism(*execution_mechanism),
                crate::GuestTaskQueueIdentity::RawDpcTaskBatch { batch_id },
                host_thread,
                *join_cause,
            )
        });
    let render_observation = render_observation
        .take()
        .map(|observation| observation.complete(crate::emulated_now()));
    RawDpcTaskBatchDispatch::Complete(
        if *full_sync_count == 0 {
            fn64_render::DpFullSyncStatus::NotReached
        } else {
            fn64_render::DpFullSyncStatus::Reached
        },
        core::mem::take(observations),
        render_observation,
        completed_guest_task_observation,
    )
}

pub(crate) fn poll_pending_raw_dpc_task_batch(
    pending: PendingRawDpcTaskBatch,
    wait: bool,
) -> Result<
    PendingRawDpcTaskBatch,
    (
        fn64_render::DpFullSyncStatus,
        Option<crate::render_observation::CompletedRenderBatchObservation>,
    ),
> {
    let Some(prepared) = poll_raw_dpc_worker(wait) else {
        return Ok(pending);
    };
    finish_prepared_raw_dpc_task_batch(pending, prepared)
}

/// Ask the registered backend for the worker's finished execution. `wait`
/// blocks until it completes; `false` returns `None` while it still runs.
pub(crate) fn poll_raw_dpc_worker(wait: bool) -> Option<ThreadedRawDpcBatchExecution> {
    RENDER_BACKEND.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .expect("pending raw-DPC worker lost its registered backend")
            .poll_raw_dpc_task_batch(wait)
    })
}

/// Bounded-wait variant of [`poll_raw_dpc_worker`]: gives the worker up to
/// `budget` to finish before returning `None`.
pub(crate) fn poll_raw_dpc_worker_bounded(
    budget: std::time::Duration,
) -> Option<ThreadedRawDpcBatchExecution> {
    RENDER_BACKEND.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .expect("pending raw-DPC worker lost its registered backend")
            .poll_raw_dpc_task_batch_bounded(budget)
    })
}

/// Completion half of [`poll_pending_raw_dpc_task_batch`], separated so a
/// nonblocking caller can decide (and note) the join only after the worker
/// is known to have finished.
pub(crate) fn finish_prepared_raw_dpc_task_batch(
    pending: PendingRawDpcTaskBatch,
    prepared: ThreadedRawDpcBatchExecution,
) -> Result<
    PendingRawDpcTaskBatch,
    (
        fn64_render::DpFullSyncStatus,
        Option<crate::render_observation::CompletedRenderBatchObservation>,
    ),
> {
    let mut pending = pending;
    if let Some(observation) = pending.render_observation.as_mut() {
        observation.set_worker_span(prepared.worker_span);
        observation.set_execution_mechanism(prepared.mechanism);
    }
    pending.worker_span = prepared.worker_span;
    pending.execution_mechanism = prepared.mechanism;
    let prepared = prepared
        .result
        .unwrap_or_else(|error| panic!("execute_raw_dpc_task_batch: {error}"));
    match finish_raw_dpc_task_batch_via_session(prepared, pending) {
        RawDpcTaskBatchDispatch::Complete(
            full_sync,
            observations,
            render_observation,
            guest_task_observation,
        ) => {
            record_rsp_rdp_observations(observations);
            if let Some(observation) = guest_task_observation {
                crate::render_observation::record_completed_guest_task(observation);
            }
            Err((full_sync, render_observation))
        }
        RawDpcTaskBatchDispatch::Pending(_) => {
            unreachable!("a joined raw-DPC worker cannot remain pending")
        }
    }
}
