//! T3 Phase B: the concrete `WgpuBackend` production raw-DPC seam.
//!
//! T0 (`fn64-render`) froze the sealed session/coordinator typestate seam
//! and its generic physical-state coordinator; T1
//! (`crate::raw_dpc::production_adapter`) froze the private-decoder ->
//! neutral-plan push loop; T3 Phase A
//! (`crate::PendingTmemTransaction::into_physical_successor`) froze the
//! inactive-successor construction `RawDpcCoordinator::complete_execution`'s
//! `next_physical` parameter needs. This module is the remaining piece: a
//! concrete, object-safe `WgpuBackend` that owns a
//! `fn64_render::RawDpcCoordinator<PhysicalTmemState>`, plans through T1's
//! adapter, executes a sealed `BoundSubmittedRawDpc` using only its
//! authority-scoped `execution_view` (never a bare ticket), and publishes
//! through exactly `self.coordinator.prepare_publication(publication).commit()`.
//!
//! Scope: TMEM loads, admitted state/triangle commands, and fill-cycle
//! `FillRectangle` color-target writes; no FullSync. No ABI/T4 ingress, no
//! visible presentation, no raster parity, no native GPU. This backend's
//! `process_task`/`present` are honest, named rejections -- this slice
//! proves the raw-DPC production seam, not general gfx-task execution.
//!
//! **Guest-write boundary.** An admitted `FillRectangle` declares
//! guest-visible `RenderTarget` *journal* writes and commits them through
//! `RawDpcAbiSession::commit_guest_render_target_writes`. Nothing **in this
//! crate** modifies guest RDRAM, and that remains true after the copyback
//! landed: `execute_fill_rectangle` still produces an owned `Vec<u8>`,
//! `ResidentPublication::publish` still writes into a backend-local `Vec`,
//! and a `CompletedWrite` is still a range plus a content digest, never
//! bytes in motion.
//!
//! What changed is that this backend now also *hands over* those bytes, on
//! request, through [`RenderBackend::committed_guest_render_target_bytes`].
//! It does not write them. The RDRAM copy is performed by `fn64-abi`'s
//! `task_dispatch::rsp_commit::copy_committed_guest_writes`, strictly after
//! the guest commit succeeded, and it re-derives each committed
//! `ContentDigest` from these bytes before writing any of them. Code here
//! may be described as "producing the bytes a committed write names", never
//! as "publishing to guest memory".

use fn64_render::{
    BackendPreparedRawDpc, BoundSubmittedRawDpc, CommittedRawDpcOutcome, ExactRawDpcPlanVisitor,
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcCoordinator, RawDpcExecutionBatch,
    RawDpcExecutionView, RawDpcIrCapability, RawDpcPlanRequest, RawDpcSemanticCommandRef,
    RawDpcBackend, RawDpcTaskBatchCapability, RdpStateCommand, RdpTriangleCommand,
    ReadyRawDpcCommitCapsule, RenderBackend, RenderConfig, RenderError, SettingsSink,
    TmemLoadSemantics, TmemLoadShape, TriangleSource,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CapturedGuestRead, CompletedWrite,
    DecodedTicket, DeferredBackendEffectReport, DeferredGuestRead, FastContentDigest,
    PhysicalRange, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    SubmittedTicket, TicketAuthoritySet, ValidationError, WorkloadAdmission, WorkloadPacket,
};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::knobs::ProbePolicy;
use crate::raw_dpc::push_planning_decoded_raw_dpc;
use crate::targets::{
    admitted_triangle_fixture, execute_combined_fill_rectangle_owned, execute_fill_rectangle_owned,
    CandidateColorTarget, ColorCoverageState, ColorTargetExecutionBatch, CompletedColorTargetWrite,
    CompletedTaskColorSegment, ComputeCoverageTriangle, ComputeHotColorBatch,
    ComputeHotColorDispatch, ComputeRasterAdmissionRefusal, ComputeRasterBatch,
    ComputeRasterBatchBuilder, ComputeRasterDrawAdmission, ComputeRasterProgramKey,
    OrderedCpuCandidateReservation, OrderedCpuColorContinuity, ResolvedFragmentBlendParams,
    SparseInitializedColorCheckpoint, TargetRectangle, TaskColorInput,
};
use crate::tmem::{
    project_committed_tmem, DeferredPhysicalTmemSuccessor, TileBindingParams, TmemGpuProjection,
};
#[cfg(test)]
use crate::RawDpcDecodeError;
use crate::{
    AlphaCompare, BlendColorInput, BlendModeState, Color4, ColorImage, ColorTargetExtent,
    ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, CombineParams, CycleType, FillColor,
    FillExecutionError, FillRectangle, HeadlessBackend, InitializedCandidateColorTarget,
    MissingTriangleDrawState, OtherMode, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemState, PrimColor, RdpState, ResolvedBlendCycle, RetrievedTriangleDraw, TargetError,
    TmemLoadSourceIdentity, TmemTransferWord, TriangleDrawOutput, TrianglePipelineDeviceOutcome,
    TrianglePipelineError, TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
    UninitializedTrianglePipeline, TMEM_SAMPLE_STATUS_OK,
};

/// Low-overhead decomposition of the production raw-DPC execute phase.
///
/// `FN64_RAW_DPC_EXEC_CENSUS=1` records only submission-boundary clock reads;
/// it never times a pixel. Every 10,000 completed execution views it prints
/// cumulative nested totals. `stage` is inside `view`, and `color` is inside
/// `stage`; the report prints their residuals explicitly so nested time is not
/// accidentally added twice. When disabled, `timed` calls its closure without
/// reading the clock.
mod raw_dpc_execute_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    pub(super) enum Phase {
        View,
        Stage,
        Color,
        ColorFill,
        ColorTexrect,
        ColorTriangle,
        ColorFinalize,
        Complete,
        DrawValidation,
    }

    static VIEW_NS: AtomicU64 = AtomicU64::new(0);
    static STAGE_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_FILL_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_TEXRECT_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_TRIANGLE_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_FINALIZE_NS: AtomicU64 = AtomicU64::new(0);
    static COLOR_FILL_CALLS: AtomicU64 = AtomicU64::new(0);
    static COLOR_TEXRECT_CALLS: AtomicU64 = AtomicU64::new(0);
    static COLOR_TRIANGLE_CALLS: AtomicU64 = AtomicU64::new(0);
    static COLOR_FINALIZE_CALLS: AtomicU64 = AtomicU64::new(0);
    static COMPLETE_NS: AtomicU64 = AtomicU64::new(0);
    static DRAW_VALIDATION_NS: AtomicU64 = AtomicU64::new(0);
    static SUBMISSIONS: AtomicU64 = AtomicU64::new(0);

    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_RAW_DPC_EXEC_CENSUS")
                .is_some_and(|value| value.trim() == "1")
        })
    }

    pub(super) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
        timed_observed(phase, false, operation).0
    }

    /// Share one coarse phase clock with a task-local observer. `observed`
    /// receives the elapsed value without arming this census's process totals;
    /// when both consumers are disabled the closure runs without a clock read.
    pub(super) fn timed_observed<R>(
        phase: Phase,
        observed: bool,
        operation: impl FnOnce() -> R,
    ) -> (R, Option<u64>) {
        let census_enabled = enabled();
        if !census_enabled && !observed {
            return (operation(), None);
        }
        let started = std::time::Instant::now();
        let value = operation();
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if census_enabled {
            match phase {
                Phase::View => &VIEW_NS,
                Phase::Stage => &STAGE_NS,
                Phase::Color => &COLOR_NS,
                Phase::ColorFill => &COLOR_FILL_NS,
                Phase::ColorTexrect => &COLOR_TEXRECT_NS,
                Phase::ColorTriangle => &COLOR_TRIANGLE_NS,
                Phase::ColorFinalize => &COLOR_FINALIZE_NS,
                Phase::Complete => &COMPLETE_NS,
                Phase::DrawValidation => &DRAW_VALIDATION_NS,
            }
            .fetch_add(elapsed, Relaxed);
            match phase {
                Phase::ColorFill => &COLOR_FILL_CALLS,
                Phase::ColorTexrect => &COLOR_TEXRECT_CALLS,
                Phase::ColorTriangle => &COLOR_TRIANGLE_CALLS,
                Phase::ColorFinalize => &COLOR_FINALIZE_CALLS,
                _ => &SUBMISSIONS,
            }
            .fetch_add(
                u64::from(matches!(
                    phase,
                    Phase::ColorFill
                        | Phase::ColorTexrect
                        | Phase::ColorTriangle
                        | Phase::ColorFinalize
                )),
                Relaxed,
            );
            if matches!(phase, Phase::View) {
                let submissions = SUBMISSIONS.fetch_add(1, Relaxed) + 1;
                if submissions % 10_000 == 0 {
                    report(submissions);
                }
            }
        }
        (value, observed.then_some(elapsed))
    }

    fn report(submissions: u64) {
        let view = VIEW_NS.load(Relaxed);
        let stage = STAGE_NS.load(Relaxed);
        let color = COLOR_NS.load(Relaxed);
        let fill = COLOR_FILL_NS.load(Relaxed);
        let texrect = COLOR_TEXRECT_NS.load(Relaxed);
        let triangle = COLOR_TRIANGLE_NS.load(Relaxed);
        let finalize = COLOR_FINALIZE_NS.load(Relaxed);
        let complete = COMPLETE_NS.load(Relaxed);
        let draw = DRAW_VALIDATION_NS.load(Relaxed);
        let ms = |ns: u64| ns as f64 / 1e6;
        let per = |ns: u64| ms(ns) / submissions as f64;
        println!(
            "[fn64-execute-census] submissions={submissions} accounted_ms={:.3} view_ms={:.3} \
             view_residual_ms={:.3} stage_ms={:.3} stage_non_color_ms={:.3} color_ms={:.3} \
             complete_ms={:.3} draw_validation_ms={:.3}",
            ms(view.saturating_add(complete).saturating_add(draw)),
            ms(view),
            ms(view.saturating_sub(stage)),
            ms(stage),
            ms(stage.saturating_sub(color)),
            ms(color),
            ms(complete),
            ms(draw),
        );
        let color_commands = fill.saturating_add(texrect).saturating_add(triangle);
        println!(
            "[fn64-execute-census] color_breakdown_ms setup_and_loop={:.3} fill={:.3} \
             texrect={:.3} triangle={:.3} finalize={:.3} calls fill={} texrect={} triangle={} \
             finalize={}",
            ms(color
                .saturating_sub(color_commands)
                .saturating_sub(finalize)),
            ms(fill),
            ms(texrect),
            ms(triangle),
            ms(finalize),
            COLOR_FILL_CALLS.load(Relaxed),
            COLOR_TEXRECT_CALLS.load(Relaxed),
            COLOR_TRIANGLE_CALLS.load(Relaxed),
            COLOR_FINALIZE_CALLS.load(Relaxed),
        );
        println!(
            "[fn64-execute-census] per_submission_ms view={:.6} stage={:.6} color={:.6} \
             complete={:.6} draw_validation={:.6}",
            per(view),
            per(stage),
            per(color),
            per(complete),
            per(draw),
        );
    }
}

/// Coarse attribution for the ordered, depth-free, triangle-only
/// `0x0018_acff` CPU task path. One fixed-size accumulator follows a task
/// through its last packet publication and is merged into process totals
/// exactly once. Decode/row preparation and raster remain one schedule clock:
/// the existing loop interleaves them, and separating them would require a
/// semantic restructuring or per-draw clocks. The disabled path performs only
/// the cached flag check made by [`Task::begin`]; no clocks, census-owned
/// allocations, or program classification are reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskCpuPhaseRunningTotals {
    pub completed_tasks: u64,
    pub task_envelope_ns: u64,
    pub attributed_members: u64,
    pub cpu_member_ns: u64,
    pub all_cpu_member_ns: u64,
    pub compute_segment_ns: u64,
    pub source_binding_load_ns: u64,
    pub prefix_capture_ns: u64,
    pub schedule_decode_row_prep_raster_ns: u64,
    pub candidate_seed_copy_ns: u64,
    pub execution_view_gross_ns: u64,
    pub finalize_coordinator_ns: u64,
}

impl TaskCpuPhaseRunningTotals {
    pub fn member_accounted_ns(self) -> u64 {
        self.source_binding_load_ns
            .saturating_add(self.prefix_capture_ns)
            .saturating_add(self.schedule_decode_row_prep_raster_ns)
            .saturating_add(self.candidate_seed_copy_ns)
    }

    pub fn execution_view_captured_read_plan_residual_ns(self) -> u64 {
        self.execution_view_gross_ns
            .saturating_sub(self.member_accounted_ns())
    }

    pub fn post_view_wrapper_residual_ns(self) -> u64 {
        self.cpu_member_ns
            .saturating_sub(self.execution_view_gross_ns)
            .saturating_sub(self.finalize_coordinator_ns)
    }

    pub fn outer_task_residual_ns(self) -> u64 {
        self.task_envelope_ns
            .saturating_sub(self.renderer_work_ns())
    }

    pub fn renderer_work_ns(self) -> u64 {
        self.all_cpu_member_ns
            .saturating_add(self.compute_segment_ns)
    }
}

mod task_cpu_phase_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    #[repr(usize)]
    pub(super) enum Phase {
        SourceBindingLoad,
        PrefixCapture,
        ScheduleDecodeRowPrepRaster,
        CandidateSeedCopy,
        ExecutionViewGross,
        FinalizeCoordinator,
        SparseCheckpoint,
        GuestPayloadMaterialization,
        SparsePublication,
    }

    const PHASE_COUNT: usize = 9;
    const SOURCE_SUBPHASE_COUNT: usize = 5;
    const MEMBER_PHASES: [Phase; 4] = [
        Phase::SourceBindingLoad,
        Phase::PrefixCapture,
        Phase::ScheduleDecodeRowPrepRaster,
        Phase::CandidateSeedCopy,
    ];

    static TASKS: AtomicU64 = AtomicU64::new(0);
    static TASK_ENVELOPE_NS: AtomicU64 = AtomicU64::new(0);
    static MEMBERS: AtomicU64 = AtomicU64::new(0);
    static CPU_MEMBER_NS: AtomicU64 = AtomicU64::new(0);
    static ALL_CPU_MEMBER_NS: AtomicU64 = AtomicU64::new(0);
    static COMPUTE_SEGMENT_NS: AtomicU64 = AtomicU64::new(0);
    static PHASE_NS: [AtomicU64; PHASE_COUNT] = [const { AtomicU64::new(0) }; PHASE_COUNT];
    static SOURCE_SUBPHASE_NS: [AtomicU64; SOURCE_SUBPHASE_COUNT] =
        [const { AtomicU64::new(0) }; SOURCE_SUBPHASE_COUNT];

    #[derive(Clone, Copy)]
    #[repr(usize)]
    pub(super) enum SourceSubphase {
        PacketCapturedReadBind,
        LoadAccessBind,
        TransactionBegin,
        WordStageAndBlockValidity,
        FinishProjectEffect,
    }

    #[derive(Clone, Copy, Default)]
    pub(super) struct SourceCounters {
        pub(super) loads: u64,
        pub(super) source_fragments: u64,
        pub(super) words: u64,
        pub(super) destination_accesses: u64,
        pub(super) first_loads: u64,
        pub(super) cumulative_expected_destination_elements: u64,
        pub(super) projected_destination_bytes: u64,
    }

    impl SourceCounters {
        fn merge_from(&mut self, other: Self) {
            self.loads = self.loads.saturating_add(other.loads);
            self.source_fragments = self.source_fragments.saturating_add(other.source_fragments);
            self.words = self.words.saturating_add(other.words);
            self.destination_accesses = self
                .destination_accesses
                .saturating_add(other.destination_accesses);
            self.first_loads = self.first_loads.saturating_add(other.first_loads);
            self.cumulative_expected_destination_elements = self
                .cumulative_expected_destination_elements
                .saturating_add(other.cumulative_expected_destination_elements);
            self.projected_destination_bytes = self
                .projected_destination_bytes
                .saturating_add(other.projected_destination_bytes);
        }
    }

    static SOURCE_COUNTERS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

    pub(super) struct Task {
        phase_ns: [u64; PHASE_COUNT],
        source_subphase_ns: [u64; SOURCE_SUBPHASE_COUNT],
        source_counters: SourceCounters,
        cpu_member_ns: u64,
        all_cpu_member_ns: u64,
        compute_segment_ns: u64,
        members: u64,
        publications_remaining: usize,
        envelope_started: Option<std::time::Instant>,
        task_envelope_ns: u64,
    }

    impl Task {
        pub(super) fn begin(
            publications: usize,
            envelope_started: Option<std::time::Instant>,
        ) -> Option<Self> {
            assert!(
                publications > 0,
                "a task CPU phase census requires at least one eventual publication"
            );
            if !enabled() {
                return None;
            }
            Some(Self {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 0,
                all_cpu_member_ns: 0,
                compute_segment_ns: 0,
                members: 0,
                publications_remaining: publications,
                envelope_started,
                task_envelope_ns: 0,
            })
        }

        pub(super) fn record_member_total(&mut self, attributed: bool, elapsed_ns: Option<u64>) {
            self.all_cpu_member_ns = self
                .all_cpu_member_ns
                .saturating_add(elapsed_ns.unwrap_or(0));
            if !attributed {
                return;
            }
            self.members = self.members.saturating_add(1);
            self.cpu_member_ns = self.cpu_member_ns.saturating_add(elapsed_ns.unwrap_or(0));
        }

        pub(super) fn record_compute_segment(&mut self, elapsed_ns: Option<u64>) {
            self.compute_segment_ns = self
                .compute_segment_ns
                .saturating_add(elapsed_ns.unwrap_or(0));
        }

        pub(super) fn record_member_envelope(
            &mut self,
            attributed: bool,
            execution_view_ns: Option<u64>,
            finalize_coordinator_ns: Option<u64>,
        ) {
            if !attributed {
                return;
            }
            self.record(Phase::ExecutionViewGross, execution_view_ns.unwrap_or(0));
            self.record(
                Phase::FinalizeCoordinator,
                finalize_coordinator_ns.unwrap_or(0),
            );
        }

        fn record(&mut self, phase: Phase, elapsed_ns: u64) {
            self.phase_ns[phase as usize] =
                self.phase_ns[phase as usize].saturating_add(elapsed_ns);
        }

        fn record_source_subphase(&mut self, phase: SourceSubphase, elapsed_ns: u64) {
            self.source_subphase_ns[phase as usize] =
                self.source_subphase_ns[phase as usize].saturating_add(elapsed_ns);
        }

        fn record_source_counters(&mut self, counters: SourceCounters) {
            self.source_counters.merge_from(counters);
        }

        #[cfg(test)]
        fn source_subphase_accounted_ns(&self) -> u64 {
            self.source_subphase_ns
                .iter()
                .copied()
                .fold(0u64, u64::saturating_add)
        }

        #[cfg(test)]
        fn source_residual_ns(&self) -> u64 {
            self.phase_ns[Phase::SourceBindingLoad as usize]
                .saturating_sub(self.source_subphase_accounted_ns())
        }

        pub(super) fn publication_finished(mut self) -> Option<Self> {
            self.publications_remaining = self
                .publications_remaining
                .checked_sub(1)
                .expect("a task CPU phase census observes at most one publication per member");
            if self.publications_remaining == 0 {
                self.task_envelope_ns = self
                    .envelope_started
                    .map(|started| u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX))
                    .unwrap_or(0);
                merge(self);
                None
            } else {
                Some(self)
            }
        }

        #[cfg(test)]
        fn member_accounted_ns(&self) -> u64 {
            MEMBER_PHASES.iter().fold(0, |total, phase| {
                total.saturating_add(self.phase_ns[*phase as usize])
            })
        }

        #[cfg(test)]
        fn residual_ns(&self) -> u64 {
            self.cpu_member_ns
                .saturating_sub(self.member_accounted_ns())
        }
    }

    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_TASK_CPU_PHASE_CENSUS")
                .is_some_and(|value| value.trim() == "1")
        })
    }

    pub(super) fn wants_member_clock() -> bool {
        enabled()
    }

    pub(super) fn task_started() -> Option<std::time::Instant> {
        enabled().then(std::time::Instant::now)
    }

    pub(super) fn running_totals() -> Option<super::TaskCpuPhaseRunningTotals> {
        if !enabled() {
            return None;
        }
        Some(super::TaskCpuPhaseRunningTotals {
            completed_tasks: TASKS.load(Relaxed),
            task_envelope_ns: TASK_ENVELOPE_NS.load(Relaxed),
            attributed_members: MEMBERS.load(Relaxed),
            cpu_member_ns: CPU_MEMBER_NS.load(Relaxed),
            all_cpu_member_ns: ALL_CPU_MEMBER_NS.load(Relaxed),
            compute_segment_ns: COMPUTE_SEGMENT_NS.load(Relaxed),
            source_binding_load_ns: PHASE_NS[Phase::SourceBindingLoad as usize].load(Relaxed),
            prefix_capture_ns: PHASE_NS[Phase::PrefixCapture as usize].load(Relaxed),
            schedule_decode_row_prep_raster_ns: PHASE_NS
                [Phase::ScheduleDecodeRowPrepRaster as usize]
                .load(Relaxed),
            candidate_seed_copy_ns: PHASE_NS[Phase::CandidateSeedCopy as usize].load(Relaxed),
            execution_view_gross_ns: PHASE_NS[Phase::ExecutionViewGross as usize].load(Relaxed),
            finalize_coordinator_ns: PHASE_NS[Phase::FinalizeCoordinator as usize].load(Relaxed),
        })
    }

    pub(super) fn timed<R>(
        task: Option<&mut Task>,
        attributed: bool,
        phase: Phase,
        operation: impl FnOnce() -> R,
    ) -> R {
        timed_optional_with_clock(
            task.filter(|_| attributed),
            phase,
            operation,
            std::time::Instant::now,
        )
    }

    pub(super) fn timed_source<R>(
        task: Option<&mut Task>,
        attributed: bool,
        phase: SourceSubphase,
        operation: impl FnOnce() -> R,
    ) -> R {
        timed_source_optional_with_clock(
            task.filter(|_| attributed),
            phase,
            operation,
            std::time::Instant::now,
        )
    }

    pub(super) fn record_source_counters(
        task: Option<&mut Task>,
        attributed: bool,
        counters: impl FnOnce() -> SourceCounters,
    ) {
        if attributed {
            if let Some(task) = task {
                task.record_source_counters(counters());
            }
        }
    }

    pub(super) fn started(task: Option<&Task>, attributed: bool) -> Option<std::time::Instant> {
        (attributed && task.is_some()).then(std::time::Instant::now)
    }

    pub(super) fn record_started(
        task: Option<&mut Task>,
        phase: Phase,
        started: Option<std::time::Instant>,
    ) {
        let (Some(task), Some(started)) = (task, started) else {
            return;
        };
        task.record(
            phase,
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
    }

    fn timed_with_clock<R, I: Copy>(
        task: &mut Task,
        phase: Phase,
        operation: impl FnOnce() -> R,
        mut now: impl FnMut() -> I,
    ) -> R
    where
        I: InstantLike,
    {
        let started = now();
        let value = operation();
        task.record(phase, started.elapsed_ns(now()));
        value
    }

    fn timed_optional_with_clock<R, I: Copy>(
        task: Option<&mut Task>,
        phase: Phase,
        operation: impl FnOnce() -> R,
        now: impl FnMut() -> I,
    ) -> R
    where
        I: InstantLike,
    {
        let Some(task) = task else {
            return operation();
        };
        timed_with_clock(task, phase, operation, now)
    }

    fn timed_source_with_clock<R, I: Copy>(
        task: &mut Task,
        phase: SourceSubphase,
        operation: impl FnOnce() -> R,
        mut now: impl FnMut() -> I,
    ) -> R
    where
        I: InstantLike,
    {
        let started = now();
        let value = operation();
        task.record_source_subphase(phase, started.elapsed_ns(now()));
        value
    }

    fn timed_source_optional_with_clock<R, I: Copy>(
        task: Option<&mut Task>,
        phase: SourceSubphase,
        operation: impl FnOnce() -> R,
        now: impl FnMut() -> I,
    ) -> R
    where
        I: InstantLike,
    {
        let Some(task) = task else {
            return operation();
        };
        timed_source_with_clock(task, phase, operation, now)
    }

    trait InstantLike {
        fn elapsed_ns(self, later: Self) -> u64;
    }

    impl InstantLike for std::time::Instant {
        fn elapsed_ns(self, later: Self) -> u64 {
            u64::try_from(later.duration_since(self).as_nanos()).unwrap_or(u64::MAX)
        }
    }

    fn merge(task: Task) {
        TASK_ENVELOPE_NS.fetch_add(task.task_envelope_ns, Relaxed);
        MEMBERS.fetch_add(task.members, Relaxed);
        CPU_MEMBER_NS.fetch_add(task.cpu_member_ns, Relaxed);
        ALL_CPU_MEMBER_NS.fetch_add(task.all_cpu_member_ns, Relaxed);
        COMPUTE_SEGMENT_NS.fetch_add(task.compute_segment_ns, Relaxed);
        for (total, elapsed) in PHASE_NS.iter().zip(task.phase_ns) {
            total.fetch_add(elapsed, Relaxed);
        }
        for (total, elapsed) in SOURCE_SUBPHASE_NS.iter().zip(task.source_subphase_ns) {
            total.fetch_add(elapsed, Relaxed);
        }
        for (total, value) in SOURCE_COUNTERS.iter().zip([
            task.source_counters.loads,
            task.source_counters.source_fragments,
            task.source_counters.words,
            task.source_counters.destination_accesses,
            task.source_counters.first_loads,
            task.source_counters
                .cumulative_expected_destination_elements,
            task.source_counters.projected_destination_bytes,
        ]) {
            total.fetch_add(value, Relaxed);
        }
        let tasks = TASKS.fetch_add(1, Relaxed) + 1;
        if tasks % 30 == 0 {
            report(tasks);
        }
    }

    fn report(tasks: u64) {
        let phases: [u64; PHASE_COUNT] =
            core::array::from_fn(|index| PHASE_NS[index].load(Relaxed));
        let cpu_member_ns = CPU_MEMBER_NS.load(Relaxed);
        let source_subphases: [u64; SOURCE_SUBPHASE_COUNT] =
            core::array::from_fn(|index| SOURCE_SUBPHASE_NS[index].load(Relaxed));
        let source_subphase_ns = source_subphases
            .iter()
            .copied()
            .fold(0u64, u64::saturating_add);
        let member_accounted_ns = MEMBER_PHASES.iter().fold(0u64, |total, phase| {
            total.saturating_add(phases[*phase as usize])
        });
        let ms = |ns: u64| ns as f64 / 1_000_000.0;
        eprintln!(
            "[task-cpu-phase-census] tasks={tasks} task_envelope_ms={:.3} \
             all_cpu_member_ms={:.3} compute_segment_ms={:.3} renderer_work_ms={:.3} \
             outer_task_residual_ms={:.3} members={} cpu_member_total_ms={:.3} \
             member_accounted_ms={:.3} residual_ms={:.3} source_binding_load_ms={:.3} \
             source_subphase_sum_ms={:.3} source_residual_ms={:.3} \
             source_packet_captured_read_bind_ms={:.3} source_load_access_bind_ms={:.3} \
             source_transaction_begin_ms={:.3} source_word_stage_block_validity_ms={:.3} \
             source_finish_project_effect_ms={:.3} loads={} source_fragments={} words={} \
             destination_accesses={} first_loads={} \
             cumulative_expected_destination_elements={} projected_destination_bytes={} \
             prefix_capture_ms={:.3} schedule_decode_row_prep_raster_ms={:.3} \
             candidate_seed_copy_ms={:.3} execution_view_gross_ms={:.3} \
             execution_view_captured_read_plan_residual_ms={:.3} \
             finalize_coordinator_ms={:.3} post_view_wrapper_residual_ms={:.3} \
             sparse_checkpoint_ms={:.3} guest_payload_materialization_ms={:.3} \
             sparse_publication_ms={:.3}",
            ms(TASK_ENVELOPE_NS.load(Relaxed)),
            ms(ALL_CPU_MEMBER_NS.load(Relaxed)),
            ms(COMPUTE_SEGMENT_NS.load(Relaxed)),
            ms(ALL_CPU_MEMBER_NS
                .load(Relaxed)
                .saturating_add(COMPUTE_SEGMENT_NS.load(Relaxed))),
            ms(TASK_ENVELOPE_NS.load(Relaxed).saturating_sub(
                ALL_CPU_MEMBER_NS
                    .load(Relaxed)
                    .saturating_add(COMPUTE_SEGMENT_NS.load(Relaxed)),
            )),
            MEMBERS.load(Relaxed),
            ms(cpu_member_ns),
            ms(member_accounted_ns),
            ms(cpu_member_ns.saturating_sub(member_accounted_ns)),
            ms(phases[Phase::SourceBindingLoad as usize]),
            ms(source_subphase_ns),
            ms(phases[Phase::SourceBindingLoad as usize].saturating_sub(source_subphase_ns)),
            ms(source_subphases[SourceSubphase::PacketCapturedReadBind as usize]),
            ms(source_subphases[SourceSubphase::LoadAccessBind as usize]),
            ms(source_subphases[SourceSubphase::TransactionBegin as usize]),
            ms(source_subphases[SourceSubphase::WordStageAndBlockValidity as usize]),
            ms(source_subphases[SourceSubphase::FinishProjectEffect as usize]),
            SOURCE_COUNTERS[0].load(Relaxed),
            SOURCE_COUNTERS[1].load(Relaxed),
            SOURCE_COUNTERS[2].load(Relaxed),
            SOURCE_COUNTERS[3].load(Relaxed),
            SOURCE_COUNTERS[4].load(Relaxed),
            SOURCE_COUNTERS[5].load(Relaxed),
            SOURCE_COUNTERS[6].load(Relaxed),
            ms(phases[Phase::PrefixCapture as usize]),
            ms(phases[Phase::ScheduleDecodeRowPrepRaster as usize]),
            ms(phases[Phase::CandidateSeedCopy as usize]),
            ms(phases[Phase::ExecutionViewGross as usize]),
            ms(phases[Phase::ExecutionViewGross as usize].saturating_sub(member_accounted_ns)),
            ms(phases[Phase::FinalizeCoordinator as usize]),
            ms(cpu_member_ns
                .saturating_sub(phases[Phase::ExecutionViewGross as usize])
                .saturating_sub(phases[Phase::FinalizeCoordinator as usize])),
            ms(phases[Phase::SparseCheckpoint as usize]),
            ms(phases[Phase::GuestPayloadMaterialization as usize]),
            ms(phases[Phase::SparsePublication as usize]),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[derive(Clone, Copy)]
        struct TestInstant(u64);

        impl InstantLike for TestInstant {
            fn elapsed_ns(self, later: Self) -> u64 {
                later.0 - self.0
            }
        }

        #[test]
        fn phase_totals_close_against_the_existing_cpu_member_clock() {
            let mut task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 100,
                all_cpu_member_ns: 100,
                compute_segment_ns: 0,
                members: 1,
                publications_remaining: 1,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            task.record(Phase::SourceBindingLoad, 11);
            task.record(Phase::PrefixCapture, 13);
            task.record(Phase::ScheduleDecodeRowPrepRaster, 36);
            task.record(Phase::CandidateSeedCopy, 23);
            assert_eq!(task.member_accounted_ns(), 83);
            assert_eq!(task.residual_ns(), 17);
            task.cpu_member_ns = 1;
            assert_eq!(task.residual_ns(), 0, "clock nesting must saturate");
        }

        #[test]
        fn coarse_member_envelopes_split_the_residual_and_saturate() {
            let totals = super::super::TaskCpuPhaseRunningTotals {
                task_envelope_ns: 200,
                cpu_member_ns: 100,
                all_cpu_member_ns: 120,
                compute_segment_ns: 30,
                source_binding_load_ns: 10,
                prefix_capture_ns: 10,
                schedule_decode_row_prep_raster_ns: 30,
                candidate_seed_copy_ns: 10,
                execution_view_gross_ns: 75,
                finalize_coordinator_ns: 20,
                ..Default::default()
            };
            assert_eq!(totals.member_accounted_ns(), 60);
            assert_eq!(totals.execution_view_captured_read_plan_residual_ns(), 15);
            assert_eq!(totals.post_view_wrapper_residual_ns(), 5);
            assert_eq!(totals.renderer_work_ns(), 150);
            assert_eq!(totals.outer_task_residual_ns(), 50);

            let inverted = super::super::TaskCpuPhaseRunningTotals {
                cpu_member_ns: 1,
                all_cpu_member_ns: 3,
                compute_segment_ns: 2,
                task_envelope_ns: 4,
                source_binding_load_ns: 4,
                execution_view_gross_ns: 3,
                finalize_coordinator_ns: 2,
                ..Default::default()
            };
            assert_eq!(inverted.execution_view_captured_read_plan_residual_ns(), 0);
            assert_eq!(inverted.post_view_wrapper_residual_ns(), 0);
            assert_eq!(inverted.outer_task_residual_ns(), 0);
        }

        #[test]
        fn non_attributed_member_records_no_coarse_envelope() {
            let mut task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 0,
                all_cpu_member_ns: 0,
                compute_segment_ns: 0,
                members: 0,
                publications_remaining: 1,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            task.record_member_envelope(false, Some(75), Some(20));
            assert_eq!(task.phase_ns[Phase::ExecutionViewGross as usize], 0);
            assert_eq!(task.phase_ns[Phase::FinalizeCoordinator as usize], 0);
            task.record_member_total(false, Some(7));
            task.record_compute_segment(Some(5));
            assert_eq!(task.cpu_member_ns, 0);
            assert_eq!(task.all_cpu_member_ns, 7);
            assert_eq!(task.compute_segment_ns, 5);
            task.record_member_envelope(true, Some(75), Some(20));
            assert_eq!(task.phase_ns[Phase::ExecutionViewGross as usize], 75);
            assert_eq!(task.phase_ns[Phase::FinalizeCoordinator as usize], 20);
        }

        #[test]
        fn completion_ordinal_advances_only_at_the_final_publication() {
            let before = TASKS.load(Relaxed);
            let task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 0,
                all_cpu_member_ns: 0,
                compute_segment_ns: 0,
                members: 0,
                publications_remaining: 2,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            let task = task
                .publication_finished()
                .expect("the first publication retains the task-local census");
            assert_eq!(TASKS.load(Relaxed), before);
            assert!(task.publication_finished().is_none());
            assert_eq!(TASKS.load(Relaxed), before + 1);
        }

        #[test]
        fn disabled_timing_path_runs_the_closure_without_reading_a_clock() {
            let mut calls = 0;
            let value = timed_optional_with_clock::<_, TestInstant>(
                None,
                Phase::PrefixCapture,
                || {
                    calls += 1;
                    7
                },
                || panic!("disabled timing must not read a clock"),
            );
            assert_eq!(value, 7);
            assert_eq!(calls, 1);

            let mut task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 0,
                all_cpu_member_ns: 0,
                compute_segment_ns: 0,
                members: 0,
                publications_remaining: 1,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            let mut ticks = [TestInstant(3), TestInstant(14)].into_iter();
            let value = timed_with_clock(
                &mut task,
                Phase::PrefixCapture,
                || 9,
                || {
                    ticks
                        .next()
                        .expect("enabled timing reads exactly two clocks")
                },
            );
            assert_eq!(value, 9);
            assert_eq!(task.phase_ns[Phase::PrefixCapture as usize], 11);
            assert!(ticks.next().is_none());
        }

        #[test]
        fn captured_read_binding_time_is_accounted_to_the_member_load_phase() {
            let mut task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 29,
                all_cpu_member_ns: 29,
                compute_segment_ns: 0,
                members: 1,
                publications_remaining: 1,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            let mut ticks = [TestInstant(5), TestInstant(18)].into_iter();
            let value = timed_optional_with_clock(
                Some(&mut task),
                Phase::SourceBindingLoad,
                || 17,
                || {
                    ticks
                        .next()
                        .expect("enabled binding timing reads exactly two clocks")
                },
            );
            assert_eq!(value, 17);
            assert_eq!(task.phase_ns[Phase::SourceBindingLoad as usize], 13);
            assert_eq!(task.member_accounted_ns(), 13);
            assert_eq!(task.residual_ns(), 16);
            assert!(ticks.next().is_none());
        }

        #[test]
        fn source_subphases_close_against_the_existing_source_total() {
            let mut task = Task {
                phase_ns: [0; PHASE_COUNT],
                source_subphase_ns: [0; SOURCE_SUBPHASE_COUNT],
                source_counters: SourceCounters::default(),
                cpu_member_ns: 0,
                all_cpu_member_ns: 0,
                compute_segment_ns: 0,
                members: 1,
                publications_remaining: 1,
                envelope_started: None,
                task_envelope_ns: 0,
            };
            task.record(Phase::SourceBindingLoad, 101);
            task.record_source_subphase(SourceSubphase::PacketCapturedReadBind, 7);
            task.record_source_subphase(SourceSubphase::LoadAccessBind, 11);
            task.record_source_subphase(SourceSubphase::TransactionBegin, 13);
            task.record_source_subphase(SourceSubphase::WordStageAndBlockValidity, 17);
            task.record_source_subphase(SourceSubphase::FinishProjectEffect, 19);
            assert_eq!(task.source_subphase_accounted_ns(), 67);
            assert_eq!(task.source_residual_ns(), 34);
            task.phase_ns[Phase::SourceBindingLoad as usize] = 1;
            assert_eq!(task.source_residual_ns(), 0, "clock nesting must saturate");
        }

        #[test]
        fn disabled_source_subphase_path_reads_neither_clock_nor_counters() {
            let mut calls = 0;
            let value = timed_source_optional_with_clock::<_, TestInstant>(
                None,
                SourceSubphase::LoadAccessBind,
                || {
                    calls += 1;
                    23
                },
                || panic!("disabled source timing must not read a clock"),
            );
            assert_eq!(value, 23);
            assert_eq!(calls, 1);

            record_source_counters(None, true, || {
                panic!("disabled source counters must remain uncomputed")
            });
        }

        #[test]
        fn exact_hot_program_and_shape_are_closed_under_single_fact_mutations() {
            let combine = super::super::CombineParams::from_wire(0xfc15_fea3, 0xf00f_f23f);
            let other = super::super::OtherMode::from_wire(0x0018_acff, 0x0f0a_7008);
            assert!(super::super::task_cpu_phase_hot_program(
                combine, other, true, true
            ));

            let program_mutations = [
                (
                    super::super::CombineParams::from_wire(0xfc15_fea2, 0xf00f_f23f),
                    other,
                    true,
                    true,
                ),
                (
                    super::super::CombineParams::from_wire(0xfc15_fea3, 0xf00f_f23e),
                    other,
                    true,
                    true,
                ),
                (
                    combine,
                    super::super::OtherMode::from_wire(0x0018_acfe, 0x0f0a_7008),
                    true,
                    true,
                ),
                (
                    combine,
                    super::super::OtherMode::from_wire(0x0018_acff, 0x0f0a_7009),
                    true,
                    true,
                ),
                (
                    combine,
                    super::super::OtherMode::from_wire(0x0018_acff, 0x0f0a_7018),
                    true,
                    true,
                ),
                (
                    combine,
                    super::super::OtherMode::from_wire(0x0018_acff, 0x0f0a_7028),
                    true,
                    true,
                ),
                (combine, other, false, true),
                (combine, other, true, false),
            ];
            for (combine, other, shaded, textured) in program_mutations {
                assert!(!super::super::task_cpu_phase_hot_program(
                    combine, other, shaded, textured
                ));
            }

            assert!(super::super::task_cpu_phase_shape(
                true, true, 0, 0, 1, false, false
            ));
            for shape in [
                (false, true, 0, 0, 1, false, false),
                (true, false, 0, 0, 1, false, false),
                (true, true, 1, 0, 1, false, false),
                (true, true, 0, 1, 1, false, false),
                (true, true, 0, 0, 0, false, false),
                (true, true, 0, 0, 1, true, false),
                (true, true, 0, 0, 1, false, true),
            ] {
                assert!(!super::super::task_cpu_phase_shape(
                    shape.0, shape.1, shape.2, shape.3, shape.4, shape.5, shape.6
                ));
            }
        }
    }
}

/// Running totals for completed Wgpu raw-DPC task batches. The task ordinal
/// advances only after the final publication; it is deliberately independent
/// of ABI `gfx_tasks`, which counts earlier `osSpTaskLoad` admissions.
pub fn task_cpu_phase_running_totals() -> Option<TaskCpuPhaseRunningTotals> {
    task_cpu_phase_census::running_totals()
}

/// Task-transport census for the opt-in compute replacement. It records only
/// task/segment boundaries and prints every 30 graphics tasks, making short
/// live A/B runs answer whether the kernel was actually reached and how much
/// of the task remained on the CPU path.
mod task_compute_census {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    static TASKS: AtomicU64 = AtomicU64::new(0);
    static MEMBERS: AtomicU64 = AtomicU64::new(0);
    static SEGMENTS: AtomicU64 = AtomicU64::new(0);
    static COMPUTE_MEMBERS: AtomicU64 = AtomicU64::new(0);
    static CPU_MEMBERS: AtomicU64 = AtomicU64::new(0);
    static COMPUTE_NS: AtomicU64 = AtomicU64::new(0);
    static TIMED_CPU_MEMBERS: AtomicU64 = AtomicU64::new(0);
    static TIMED_CPU_NS: AtomicU64 = AtomicU64::new(0);
    static REGISTRY_CLONE_CALLS: AtomicU64 = AtomicU64::new(0);
    static REGISTRY_CLONE_BYTES: AtomicU64 = AtomicU64::new(0);
    static REGISTRY_CLONE_NS: AtomicU64 = AtomicU64::new(0);
    static SHADOW_CLONE_CALLS: AtomicU64 = AtomicU64::new(0);
    static SHADOW_CLONE_BYTES: AtomicU64 = AtomicU64::new(0);
    static SHADOW_CLONE_NS: AtomicU64 = AtomicU64::new(0);

    thread_local! {
        static CPU_REASONS: RefCell<BTreeMap<super::TaskComputeCpuReason, (u64, u64)>> =
            RefCell::new(BTreeMap::new());
        static TASK_CPU_REASONS: RefCell<BTreeMap<super::TaskComputeCpuReason, (u64, u64)>> =
            RefCell::new(BTreeMap::new());
        static TASK_COMPUTE: RefCell<(u64, u64)> = const { RefCell::new((0, 0)) };
        static COMPUTE_PROGRAMS: RefCell<BTreeMap<super::ComputeProgramAttribution, (u64, u64, u64)>> =
            RefCell::new(BTreeMap::new());
        static TASK_COMPUTE_PROGRAMS: RefCell<BTreeMap<super::ComputeProgramAttribution, (u64, u64, u64)>> =
            RefCell::new(BTreeMap::new());
        #[cfg(test)]
        static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn enabled() -> bool {
        #[cfg(test)]
        if TEST_ENABLED.with(std::cell::Cell::get) {
            return true;
        }
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_TASK_COMPUTE_CENSUS")
                .is_some_and(|value| value.trim() == "1")
                || tail_enabled()
        })
    }

    fn tail_enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_TASK_COMPUTE_TAIL_CENSUS")
                .is_some_and(|value| value.trim() == "1")
        })
    }

    pub(super) fn segment_started() -> Option<std::time::Instant> {
        (enabled() || super::task_cpu_phase_census::wants_member_clock())
            .then(std::time::Instant::now)
    }

    pub(super) fn wants_program_attribution() -> bool {
        enabled()
    }

    pub(super) fn cpu_started() -> Option<std::time::Instant> {
        (enabled() || super::task_cpu_phase_census::wants_member_clock())
            .then(std::time::Instant::now)
    }

    pub(super) fn timed_registry_clone<R>(bytes: usize, operation: impl FnOnce() -> R) -> R {
        if !enabled() {
            return operation();
        }
        let started = std::time::Instant::now();
        let value = operation();
        REGISTRY_CLONE_CALLS.fetch_add(1, Relaxed);
        REGISTRY_CLONE_BYTES.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Relaxed);
        REGISTRY_CLONE_NS.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Relaxed,
        );
        value
    }

    pub(super) fn timed_shadow_clone<R>(bytes: usize, operation: impl FnOnce() -> R) -> R {
        if !enabled() {
            return operation();
        }
        let started = std::time::Instant::now();
        let value = operation();
        SHADOW_CLONE_CALLS.fetch_add(1, Relaxed);
        SHADOW_CLONE_BYTES.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Relaxed);
        SHADOW_CLONE_NS.fetch_add(
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Relaxed,
        );
        value
    }

    pub(super) fn record_segment(
        members: usize,
        program: Option<super::ComputeProgramAttribution>,
        started: Option<std::time::Instant>,
    ) -> Option<u64> {
        let elapsed =
            started.map(|started| u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        if !enabled() {
            return elapsed;
        }
        let program = program.expect("enabled segment census classifies its program mix");
        SEGMENTS.fetch_add(1, Relaxed);
        COMPUTE_MEMBERS.fetch_add(u64::try_from(members).unwrap_or(u64::MAX), Relaxed);
        let elapsed = elapsed.unwrap_or(0);
        COMPUTE_NS.fetch_add(elapsed, Relaxed);
        COMPUTE_PROGRAMS.with(|programs| {
            accumulate_program(&mut programs.borrow_mut(), program, members, elapsed);
        });
        if tail_enabled() {
            TASK_COMPUTE.with(|task| {
                let mut task = task.borrow_mut();
                task.0 = task
                    .0
                    .saturating_add(u64::try_from(members).unwrap_or(u64::MAX));
                task.1 = task.1.saturating_add(elapsed);
            });
            TASK_COMPUTE_PROGRAMS.with(|programs| {
                accumulate_program(&mut programs.borrow_mut(), program, members, elapsed);
            });
        }
        Some(elapsed)
    }

    fn accumulate_program(
        programs: &mut BTreeMap<super::ComputeProgramAttribution, (u64, u64, u64)>,
        program: super::ComputeProgramAttribution,
        members: usize,
        elapsed_ns: u64,
    ) {
        let entry = programs.entry(program).or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry
            .1
            .saturating_add(u64::try_from(members).unwrap_or(u64::MAX));
        entry.2 = entry.2.saturating_add(elapsed_ns);
    }

    #[cfg(test)]
    pub(super) fn begin_enabled_segment_test() {
        TEST_ENABLED.with(|enabled| assert!(!enabled.replace(true)));
        COMPUTE_PROGRAMS.with(|programs| programs.borrow_mut().clear());
    }

    #[cfg(test)]
    pub(super) fn finish_enabled_segment_test(
    ) -> BTreeMap<super::ComputeProgramAttribution, (u64, u64, u64)> {
        TEST_ENABLED.with(|enabled| assert!(enabled.replace(false)));
        COMPUTE_PROGRAMS.with(|programs| core::mem::take(&mut *programs.borrow_mut()))
    }

    pub(super) fn record_cpu(
        reason: super::TaskComputeCpuReason,
        started: Option<std::time::Instant>,
    ) -> Option<u64> {
        let Some(started) = started else {
            return None;
        };
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if !enabled() {
            return Some(elapsed);
        }
        TIMED_CPU_MEMBERS.fetch_add(1, Relaxed);
        TIMED_CPU_NS.fetch_add(elapsed, Relaxed);
        CPU_REASONS.with(|reasons| {
            let mut reasons = reasons.borrow_mut();
            accumulate_reason(&mut reasons, reason, elapsed);
        });
        if tail_enabled() {
            TASK_CPU_REASONS.with(|reasons| {
                let mut reasons = reasons.borrow_mut();
                accumulate_reason(&mut reasons, reason, elapsed);
            });
        }
        Some(elapsed)
    }

    fn accumulate_reason(
        reasons: &mut BTreeMap<super::TaskComputeCpuReason, (u64, u64)>,
        reason: super::TaskComputeCpuReason,
        elapsed_ns: u64,
    ) {
        let entry = reasons.entry(reason).or_default();
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(elapsed_ns);
    }

    fn reason_name(reason: super::TaskComputeCpuReason) -> String {
        match reason {
            super::TaskComputeCpuReason::ExactAdmissionRejected(
                super::TaskComputeAdmissionRefusal::ProgramBits(words),
            )
            | super::TaskComputeCpuReason::Planned(super::PlannedTaskCpuReason::DefinitelyCpu(
                super::TaskComputeAdmissionRefusal::ProgramBits(words),
            )) => {
                let [lo, hi, omh, oml] = words;
                format!("program_bits:{lo:08x}/{hi:08x}/{omh:08x}/{oml:08x}")
            }
            super::TaskComputeCpuReason::ExactAdmissionRejected(
                super::TaskComputeAdmissionRefusal::CycleType(words),
            )
            | super::TaskComputeCpuReason::Planned(super::PlannedTaskCpuReason::DefinitelyCpu(
                super::TaskComputeAdmissionRefusal::CycleType(words),
            )) => {
                let [lo, hi, omh, oml] = words;
                format!("cycle_type:{lo:08x}/{hi:08x}/{omh:08x}/{oml:08x}")
            }
            _ => format!("{reason:?}"),
        }
    }

    pub(super) fn record_task(members: usize, cpu_members: usize) {
        if !enabled() {
            return;
        }
        MEMBERS.fetch_add(u64::try_from(members).unwrap_or(u64::MAX), Relaxed);
        CPU_MEMBERS.fetch_add(u64::try_from(cpu_members).unwrap_or(u64::MAX), Relaxed);
        let tasks = TASKS.fetch_add(1, Relaxed) + 1;
        if tail_enabled() {
            let (compute_members, compute_ns) = TASK_COMPUTE.with(|task| {
                let mut task = task.borrow_mut();
                core::mem::take(&mut *task)
            });
            let reasons =
                TASK_CPU_REASONS.with(|reasons| core::mem::take(&mut *reasons.borrow_mut()));
            let programs =
                TASK_COMPUTE_PROGRAMS.with(|programs| core::mem::take(&mut *programs.borrow_mut()));
            let reason_fields = reasons
                .iter()
                .map(|(reason, (members, elapsed_ns))| {
                    format!(
                        "{}={members}:{:.3}",
                        reason_name(*reason),
                        *elapsed_ns as f64 / 1_000_000.0,
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            let program_fields = programs
                .iter()
                .map(|(program, (segments, members, elapsed_ns))| {
                    format!(
                        "{program:?}={segments}:{members}:{:.3}",
                        *elapsed_ns as f64 / 1_000_000.0,
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            eprintln!(
                "[task-compute-tail] task={tasks} members={members} cpu_members={cpu_members} \
                 compute_members={compute_members} compute_ms={:.3} \
                 programs={program_fields} cpu={reason_fields}",
                compute_ns as f64 / 1_000_000.0,
            );
        }
        if tasks % 30 == 0 {
            let compute_members = COMPUTE_MEMBERS.load(Relaxed);
            let compute_ns = COMPUTE_NS.load(Relaxed);
            let timed_cpu_members = TIMED_CPU_MEMBERS.load(Relaxed);
            let timed_cpu_ns = TIMED_CPU_NS.load(Relaxed);
            let registry_clone_ns = REGISTRY_CLONE_NS.load(Relaxed);
            let shadow_clone_ns = SHADOW_CLONE_NS.load(Relaxed);
            eprintln!(
                "[task-compute-census] tasks={tasks} members={} compute_segments={} \
                 compute_members={} cpu_members={} compute_total_ms={:.3} \
                 compute_ms/member={:.3} timed_cpu_members={} timed_cpu_total_ms={:.3} \
                 timed_cpu_ms/member={:.3} registry_clone_calls={} registry_clone_bytes={} \
                 registry_clone_total_ms={:.3} shadow_clone_calls={} shadow_clone_bytes={} \
                 shadow_clone_total_ms={:.3}",
                MEMBERS.load(Relaxed),
                SEGMENTS.load(Relaxed),
                compute_members,
                CPU_MEMBERS.load(Relaxed),
                compute_ns as f64 / 1_000_000.0,
                compute_ns as f64 / 1_000_000.0 / compute_members.max(1) as f64,
                timed_cpu_members,
                timed_cpu_ns as f64 / 1_000_000.0,
                timed_cpu_ns as f64 / 1_000_000.0 / timed_cpu_members.max(1) as f64,
                REGISTRY_CLONE_CALLS.load(Relaxed),
                REGISTRY_CLONE_BYTES.load(Relaxed),
                registry_clone_ns as f64 / 1_000_000.0,
                SHADOW_CLONE_CALLS.load(Relaxed),
                SHADOW_CLONE_BYTES.load(Relaxed),
                shadow_clone_ns as f64 / 1_000_000.0,
            );
            CPU_REASONS.with(|reasons| {
                for (reason, (members, elapsed_ns)) in reasons.borrow().iter() {
                    eprintln!(
                        "[task-compute-reason] reason={} members={} total_ms={:.3} ms/member={:.3}",
                        reason_name(*reason),
                        members,
                        *elapsed_ns as f64 / 1_000_000.0,
                        *elapsed_ns as f64 / 1_000_000.0 / (*members).max(1) as f64,
                    );
                }
            });
            COMPUTE_PROGRAMS.with(|programs| {
                for (program, (segments, members, elapsed_ns)) in programs.borrow().iter() {
                    eprintln!(
                        "[task-compute-program] program={program:?} segments={segments} \
                         members={members} total_ms={:.3} ms/member={:.3}",
                        *elapsed_ns as f64 / 1_000_000.0,
                        *elapsed_ns as f64 / 1_000_000.0 / (*members).max(1) as f64,
                    );
                }
            });
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn with_census_enabled<R>(operation: impl FnOnce() -> R) -> R {
            begin_enabled_segment_test();
            let value = operation();
            finish_enabled_segment_test();
            value
        }

        #[test]
        fn reason_totals_keep_distinct_program_keys_and_close() {
            let first = super::super::TaskComputeCpuReason::ExactAdmissionRejected(
                super::super::TaskComputeAdmissionRefusal::ProgramBits([1, 2, 3, 4]),
            );
            let second = super::super::TaskComputeCpuReason::ExactAdmissionRejected(
                super::super::TaskComputeAdmissionRefusal::ProgramBits([5, 6, 7, 8]),
            );
            let mut totals = BTreeMap::new();
            accumulate_reason(&mut totals, first, 11);
            accumulate_reason(&mut totals, first, 13);
            accumulate_reason(&mut totals, second, 17);
            assert_eq!(totals.get(&first), Some(&(2, 24)));
            assert_eq!(totals.get(&second), Some(&(1, 17)));
            assert_eq!(totals.values().map(|(members, _)| members).sum::<u64>(), 3);
            assert_eq!(totals.values().map(|(_, ns)| ns).sum::<u64>(), 41);
            assert_eq!(
                reason_name(super::super::TaskComputeCpuReason::ExactAdmissionRejected(
                    super::super::TaskComputeAdmissionRefusal::CycleType([1, 2, 3, 4]),
                )),
                "cycle_type:00000001/00000002/00000003/00000004"
            );
            assert_eq!(
                reason_name(super::super::TaskComputeCpuReason::Planned(
                    super::super::PlannedTaskCpuReason::DefinitelyCpu(
                        super::super::TaskComputeAdmissionRefusal::CycleType([1, 2, 3, 4]),
                    ),
                )),
                "cycle_type:00000001/00000002/00000003/00000004"
            );

            let mut programs = BTreeMap::new();
            accumulate_program(
                &mut programs,
                super::super::ComputeProgramAttribution::Program(0),
                2,
                11,
            );
            accumulate_program(
                &mut programs,
                super::super::ComputeProgramAttribution::Program(0),
                3,
                13,
            );
            accumulate_program(
                &mut programs,
                super::super::ComputeProgramAttribution::MixedPrograms,
                5,
                17,
            );
            assert_eq!(
                programs
                    .values()
                    .map(|(segments, _, _)| segments)
                    .sum::<u64>(),
                3,
            );
            assert_eq!(
                programs
                    .values()
                    .map(|(_, members, _)| members)
                    .sum::<u64>(),
                10,
            );
            assert_eq!(programs.values().map(|(_, _, ns)| ns).sum::<u64>(), 41,);
        }

        #[test]
        fn enabled_segment_path_records_nonempty_program_member_denominator() {
            with_census_enabled(|| {
                let started = segment_started();
                let _ = record_segment(
                    2,
                    Some(super::super::ComputeProgramAttribution::Program(2)),
                    started,
                );
                COMPUTE_PROGRAMS.with(|programs| {
                    let programs = programs.borrow();
                    let (segments, members, _) = programs
                        .get(&super::super::ComputeProgramAttribution::Program(2))
                        .copied()
                        .expect("enabled census records the typed program bucket");
                    assert_eq!((segments, members), (1, 2));
                    assert_eq!(
                        programs
                            .values()
                            .map(|(segments, _, _)| segments)
                            .sum::<u64>(),
                        1,
                    );
                    assert_eq!(
                        programs
                            .values()
                            .map(|(_, members, _)| members)
                            .sum::<u64>(),
                        2,
                    );
                });
            });
        }
    }
}

/// Low-overhead decomposition of the production raw-DPC planning phase.
///
/// `FN64_RAW_DPC_PLAN_CENSUS=1` records clocks only at submission boundaries
/// and reports cumulative totals every 10,000 completed plans. Keeping this
/// separate from the execute census makes the probe decode's cost visible:
/// planning currently decodes once to derive the exact resource journal and
/// once more to construct the authoritative plan against that journal.
mod raw_dpc_plan_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    pub(super) enum Phase {
        Prepare,
        DecodeAndDerive,
        AdmitAndSeal,
    }

    static PREPARE_NS: AtomicU64 = AtomicU64::new(0);
    static DECODE_AND_DERIVE_NS: AtomicU64 = AtomicU64::new(0);
    static ADMIT_AND_SEAL_NS: AtomicU64 = AtomicU64::new(0);
    static PLANS: AtomicU64 = AtomicU64::new(0);

    fn enabled() -> bool {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            crate::diag_env::diag_env("FN64_RAW_DPC_PLAN_CENSUS")
                .is_some_and(|value| value.trim() == "1")
        })
    }

    pub(super) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
        if !enabled() {
            return operation();
        }
        let started = std::time::Instant::now();
        let value = operation();
        let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        match phase {
            Phase::Prepare => &PREPARE_NS,
            Phase::DecodeAndDerive => &DECODE_AND_DERIVE_NS,
            Phase::AdmitAndSeal => &ADMIT_AND_SEAL_NS,
        }
        .fetch_add(elapsed, Relaxed);
        if matches!(phase, Phase::AdmitAndSeal) {
            let plans = PLANS.fetch_add(1, Relaxed) + 1;
            if plans % 10_000 == 0 {
                report(plans);
            }
        }
        value
    }

    fn accounted_ns(prepare: u64, decode_and_derive: u64, admit_and_seal: u64) -> u64 {
        prepare
            .saturating_add(decode_and_derive)
            .saturating_add(admit_and_seal)
    }

    fn report(plans: u64) {
        let prepare = PREPARE_NS.load(Relaxed);
        let decode_and_derive = DECODE_AND_DERIVE_NS.load(Relaxed);
        let admit_and_seal = ADMIT_AND_SEAL_NS.load(Relaxed);
        let total = accounted_ns(prepare, decode_and_derive, admit_and_seal);
        let ms = |ns: u64| ns as f64 / 1e6;
        println!(
            "[fn64-plan-census] plans={plans} accounted_ms={:.3} prepare_ms={:.3} \
             decode_and_derive_ms={:.3} admit_and_seal_ms={:.3} per_plan_ms={:.6}",
            ms(total),
            ms(prepare),
            ms(decode_and_derive),
            ms(admit_and_seal),
            ms(total) / plans as f64,
        );
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn one_pass_census_closes_and_saturates() {
            assert_eq!(super::accounted_ns(11, 13, 17), 41);
            assert_eq!(super::accounted_ns(u64::MAX, 1, 1), u64::MAX);
        }
    }
}

/// The ten durable RDP draw registers the plan walk seeds from durable state
/// and then tracks, in stream order, as the packet's own state commands
/// arrive.
///
/// Before task 4.1 these ten fields existed in three places: as
/// `PlanCollector`'s ten `current_*` fields, mirrored field-for-field by
/// `RawDpcCarryIn`, and re-listed as nine positional parameters in a
/// test-only `seeded_from_parts` constructor. The three copies had to be
/// edited together for every register added, and the positional constructor
/// silently accepted a transposed pair of same-typed arguments. One struct
/// owning both the fields and their per-command update rule
/// ([`RdpDrawState::apply`]) makes each register appear once.
///
/// One value represents one stream instant; its fields cannot drift across
/// the packet boundary independently.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RdpDrawState {
    /// `SetOtherMode` current at this stream position.
    other_mode: Option<OtherMode>,
    /// `SetCombine` current at this stream position.
    combine: Option<CombineParams>,
    /// `G_SETBLENDCOLOR` current at this stream position.
    blend_color: Color4,
    /// `G_SETENVCOLOR` current at this stream position.
    env_color: Color4,
    /// `G_SETPRIMCOLOR` current at this stream position.
    prim_color: PrimColor,
    /// `G_SETFOGCOLOR` current at this stream position. Needed by the
    /// production blend-cycle wiring's `Fog` selector.
    fog_color: Color4,
    /// `G_SETSCISSOR` current at this stream position. Seeding matters more
    /// here than for the color registers: a display list commonly sets the
    /// scissor once per frame and then submits several packets, so a
    /// per-packet reset would unscissor every packet after the first.
    scissor: Option<crate::targets::RdpScissorRect>,
    /// `G_SETPRIMDEPTH` current at this stream position. Read by the CPU
    /// raster path's z-compare under `G_ZS_PRIM`.
    prim_depth: Option<crate::state::PrimDepth>,
    /// `G_SETCIMG` current at this stream position.
    ///
    /// **The durable seed is load-bearing, not defensive.** The decoder
    /// already derives a texrect's declared `ColorFramebuffer` write
    /// accesses from `state.color_image()` -- durable state -- in
    /// `raw_dpc::plan_texture_rectangle`. Deriving the executor's
    /// [`ColorTargetKey`] from a packet-local `FillRectangle` instead made
    /// those two derivations disagree for any packet whose texrects follow
    /// an earlier packet's `SetColorImage`: measured on WM2000, a real
    /// packet of 14 texrects, 4 TMEM loads and **zero** fills, whose
    /// texrects every one declared a real four-access write run.
    color_image: Option<ColorImage>,
    /// Every tile's `SetTile`/`SetTileSize` current at this stream position,
    /// indexed by the RDP's own 0..=7 tile index.
    ///
    /// The RDP's eight tile descriptors are durable registers, so a packet
    /// that re-declares none still has them. Seeding `[(None, None); 8]`
    /// made every texrect in such a packet a `TexrectUnboundTile` refusal:
    /// measured on WM2000, the packet that follows the load-free texrect
    /// admission carries 46 texrects and an entirely empty tile table.
    ///
    /// The whole table, not tile 0 alone, because EVERY admitted draw
    /// names its own tile in its own wire word: a texture rectangle in
    /// word 1 bits 26:24, a raw triangle in word 0 bits 18:16. Tracking
    /// only tile 0 made every non-zero-tile texrect an `UnboundTile`
    /// refusal (WM2000's do not name tile 0), and made every non-zero-tile
    /// raw triangle silently bind tile 0's descriptor in the GPU uniform.
    tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
}

impl RdpDrawState {
    /// Every durable RDP draw register as of `state`.
    fn capture(state: &RdpState) -> Self {
        Self {
            other_mode: state.other_mode(),
            combine: state.combine(),
            blend_color: state.blend_color(),
            env_color: state.env_color(),
            prim_color: state.prim_color(),
            fog_color: state.fog_color(),
            scissor: state.scissor(),
            prim_depth: state.prim_depth(),
            color_image: state.color_image(),
            tiles: durable_neutral_tiles(state),
        }
    }

    /// Advances this running value by one state command, in plan order.
    ///
    /// The single implementation of the seed-then-track update rule, moved
    /// here verbatim from `PlanCollector`'s own `ExactRawDpcPlanVisitor`
    /// arms -- same order, same conditions. Commands this walk reads no
    /// field of leave the value untouched; they are still counted for
    /// `command_index` continuity by the caller.
    fn apply(&mut self, state: &RdpStateCommand) {
        match state {
            RdpStateCommand::SetOtherMode { other_mode, .. } => {
                self.other_mode = Some(OtherMode::from_wire(other_mode.high, other_mode.low));
            }
            RdpStateCommand::SetCombine { combine, .. } => {
                self.combine = Some(CombineParams::from_wire(combine.low, combine.high));
            }
            RdpStateCommand::SetBlendColor { color, .. } => {
                self.blend_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetEnvColor { color, .. } => {
                self.env_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetPrimColor { color, .. } => {
                self.prim_color = PrimColor::from_wire(
                    u32::from(color.lod_frac) | (u32::from(color.lod_min) << 8),
                    color.color,
                );
            }
            RdpStateCommand::SetFogColor { color, .. } => {
                self.fog_color = Color4::from_wire(color.value);
            }
            RdpStateCommand::SetPrimDepth { depth, .. } => {
                // Reconstruct the wire form (`z` in bits 16:31, `dz` in
                // bits 0:15) so `PrimDepth::from_wire` re-applies the
                // 15-bit z / 16-bit dz masks the decoder used -- the same
                // recovery `TriangleDrawStateCollector` performs.
                self.prim_depth = Some(crate::state::PrimDepth::from_wire(
                    (u32::from(depth.z) << 16) | u32::from(depth.dz),
                ));
            }
            // Latched verbatim in wire quarter-pixels. Public libultra
            // `include/ultra64/gbi.h:4794-4837` encodes each coordinate
            // as a twelve-bit value scaled by four (or accepts the
            // fractional wire value directly).
            RdpStateCommand::SetScissor { scissor, .. } => {
                self.scissor = Some(crate::targets::RdpScissorRect::from_wire_quarter_pixels(
                    scissor.mode,
                    scissor.upper_left_x,
                    scissor.upper_left_y,
                    scissor.lower_right_x,
                    scissor.lower_right_y,
                ));
            }
            RdpStateCommand::SetColorImage { image, .. } => {
                self.color_image = Some(ColorImage::from_wire(
                    image_format(image.format),
                    pixel_size(image.size),
                    image.width,
                    image.address,
                ));
            }
            RdpStateCommand::SetTile {
                tile_index,
                descriptor,
                ..
            } => {
                if let Some(slot) = self.tiles.get_mut(usize::from(*tile_index)) {
                    slot.0 = Some(*descriptor);
                }
            }
            RdpStateCommand::SetTileSize {
                tile_index, size, ..
            } => {
                if let Some(slot) = self.tiles.get_mut(usize::from(*tile_index)) {
                    slot.1 = Some(*size);
                }
            }
            _ => {}
        }
    }
}

impl Default for RdpDrawState {
    fn default() -> Self {
        Self::capture(&RdpState::default())
    }
}

/// Every durable RDP register the execution walk may read before this
/// submission issues its first state command. One value represents one stream
/// instant; its fields cannot drift across the packet boundary independently.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct RawDpcCarryIn {
    draw: RdpDrawState,
}

impl RawDpcCarryIn {
    fn capture(state: &RdpState) -> Self {
        Self {
            draw: RdpDrawState::capture(state),
        }
    }
}

/// Planning can cheaply rule out members whose command shape can never form
/// a compute segment. `ComputeCandidate` is deliberately not an execution
/// capability: exact program/TMEM/tile admission happens against captured
/// execution state and yields a separate move-only `ComputeEligibleTaskMember`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlannedTaskCpuReason {
    NoRawTriangle(PlannedNoRawTriangleReason),
    MixedFillOrTexrect,
    DefinitelyCpu(TaskComputeAdmissionRefusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlannedNoRawTriangleReason {
    FillOnly,
    TexrectOnly,
    FillAndTexrect,
    TmemLoadOnly,
    FillAndTmemLoad,
    TexrectAndTmemLoad,
    FillTexrectAndTmemLoad,
    SyncStateOnly,
    NoOpOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ComputeProgramAttribution {
    Program(u32),
    MixedPrograms,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlannedTaskExecution {
    Cpu(PlannedTaskCpuReason),
    ComputeCandidate,
}

struct PlannedRawDpcTaskMember {
    carry_in: RawDpcCarryIn,
    execution: PlannedTaskExecution,
}

/// One pending value binds every member's carry-in state to its planning
/// disposition. It is installed only after the whole batch plans, so a
/// mid-batch failure cannot advance durable RDP state or leave parallel queue
/// prefixes for a later execution call to mis-pair.
struct PlannedRawDpcTaskBatch {
    members: VecDeque<PlannedRawDpcTaskMember>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentedFieldDelivery {
    ConcreteDiagnostic,
    Source,
    PostVi,
}

/// The pure-Rust wgpu production raw-DPC backend. Owns its coordinator
/// outright -- there is exactly one route to one, at construction, per
/// `RawDpcBackendAuthority::into_coordinator`'s own doc comment.
pub struct WgpuBackend {
    coordinator: RawDpcCoordinator<PhysicalTmemState>,
    /// Durable logical RDP state (`SetTile`/`SetTextureImage`/`SyncLoad`
    /// fields `RdpState` tracks) carried across submissions. `decode_raw_dpc`
    /// always forks a throwaway copy to decode against
    /// (`RdpState::fork_for_decode`) and never mutates its input, so holding
    /// this durably and applying each successful real decode's
    /// `RdpStateDelta` back onto it (`RdpState::apply`) is what gives
    /// `plan_raw_dpc` continuity across submissions instead of silently
    /// re-decoding every command stream from a fresh default state, which
    /// would be wrong for any submission whose commands depend on state a
    /// prior submission set (e.g. a `SetTile` from submission N read by a
    /// `LoadBlock` in submission N+1).
    rdp_state: RdpState,
    /// The matching successful plan's complete pre-delta register state.
    /// Captured immediately before `RdpState::apply`; execution consumes this
    /// value as a unit, so no register can be seeded from the packet's final
    /// state while a sibling is seeded from its carry-in state.
    raw_dpc_carry_in_before_last_plan: Option<RawDpcCarryIn>,
    /// The one move-only task value retained by the explicitly batched plan
    /// seam. Ordinary planning never writes it.
    pending_raw_dpc_task_batch: Option<PlannedRawDpcTaskBatch>,
    /// `Some` only after a successful `RenderBackend::create`; `try_new`
    /// never populates it. Always `Some` together with
    /// `triangle_target_extent`, never one without the other.
    triangle_pipeline: Option<Box<TrianglePipelineRenderer>>,
    /// The render-target extent for triangle draws, sized from `create`'s
    /// own `RenderConfig`. Always `Some` together with `triangle_pipeline`,
    /// never one without the other; replaced atomically with it on every
    /// `create()` call.
    triangle_target_extent: Option<TriangleTargetExtent>,
    /// The most recent successful triangle draw's GPU-observed output.
    /// Replaced only when every triangle in a draw call succeeds; a
    /// failed draw leaves the prior value untouched. Never an accumulated
    /// history, never a persistent framebuffer.
    triangle_draw_output: Option<TriangleDrawOutput>,
    /// Every launch-time probe/diagnostic boolean this backend holds,
    /// resolved ONCE at construction from the host's [`crate::WgpuKnobs`].
    ///
    /// Before task 2.2b these were seven loose `bool` fields, each read
    /// straight from the environment inside `try_new`. Collecting them lets
    /// a caller (a test, or `fn64-shell`) state the policy as a value
    /// instead of mutating the process environment, and puts every default
    /// in one documented place.
    probes: ProbePolicy,
    /// Active only around an explicitly bounded offline replay window. The
    /// window retains each packet's typed compute fixtures until one final
    /// submit can prove exact intermediate target checkpoints.
    compute_raster_checkpoint_probe: Option<ComputeRasterCheckpointProbe>,
    /// Most recent successful probe execution, consumed by the offline
    /// replay's phase accounting. An ineligible packet leaves this `None`.
    compute_raster_probe_receipt: Option<ComputeRasterProbeReceipt>,
    /// Most recent packet whose guest-visible target was produced by the
    /// replacement chain rather than the CPU rasterizer.
    compute_raster_replace_receipt: Option<ComputeRasterProbeReceipt>,
    task_cpu_phase_census: Option<task_cpu_phase_census::Task>,
    last_task_batch_execution_mechanism: Option<fn64_render::RawDpcTaskBatchExecutionMechanism>,
    last_published_visual_target: Option<(
        fn64_render_ir::SubmissionIdentity,
        PublishedVisualTargetMarker,
    )>,
    /// The host-configured framebuffer extent from the most recent
    /// `RenderBackend::create` call, recorded *before* the GPU device
    /// request rather than inside its success branch.
    ///
    /// This is the only color-image height source this backend has: the
    /// RDP's `SetColorImage` carries `format`/`size`/`width`/`address` and
    /// **no height** field (`crate::ColorImage`), so an admitted
    /// `FillRectangle`'s `ColorTargetKey` must take its height from here.
    /// Deliberately separate from `triangle_target_extent` (which is only
    /// populated on GPU-device success): a `FillRectangle` is executed
    /// entirely CPU-side and has no adapter dependency, so gating fill
    /// admission on a real GPU would make an adapterless host silently
    /// unable to execute a command it is fully capable of executing.
    ///
    /// Honest nonclaim: this is a *host-configured* height, not a
    /// wire-decoded one. The RDP never states a color image's height, so a
    /// stream setting an offscreen color image of a different height would
    /// derive a `ColorTargetKey` whose range is wrong. That mismatch
    /// surfaces loudly (`RectangleOutOfBounds` from
    /// `CandidateColorTarget::plan_rows`, or `AliasedResidentTarget`), never
    /// as a silently mis-sized publish.
    configured_target_extent: Option<TriangleTargetExtent>,
    /// `None` until the first admitted `FillRectangle` reaches
    /// `execute_raw_dpc_inner`. Built there, from that capture's own
    /// `PhysicalMemoryLayout` -- neither `try_new` nor `create` has a layout
    /// to build it from (`RenderConfig` carries a pixel extent, not an RDRAM
    /// byte size), and inventing one would be a fabricated fact. A later
    /// capture whose layout differs is rejected loudly by
    /// `ColorTargetRegistry::begin_candidate`'s existing
    /// `MemoryLayoutMismatch` check, never by silently rebuilding the
    /// registry and dropping every resident generation.
    color_targets: Option<ColorTargetRegistry>,
    /// Set by `execute_raw_dpc_inner` when a fill staged an
    /// `InitializedCandidateColorTarget`; redeemed by `publish_raw_dpc`.
    /// See [`PendingFillPublication`].
    pending_fill_publication: Option<PendingFillPublication>,
    /// Ordered color successors produced by one task batch. Each token is
    /// redeemed only by its own later per-submission publication.
    task_batch_pending_fill_publications: VecDeque<PendingFillPublication>,
    /// The most recent successfully presented VI field, and nothing else.
    ///
    /// `None` until the first `present` succeeds. A `present` that returns a
    /// named refusal or a typed bounds/alignment error leaves the previous
    /// field in place rather than clearing it: the retrace that failed
    /// produced no image, and discarding the last good one would fabricate a
    /// black frame the VI never scanned out. A *successful* present always
    /// replaces it, so this is never an accumulated history.
    presented_field: Option<crate::PresentedField>,
    /// Selects one explicit stage owner. The source and post-VI receipts are
    /// different types and cannot both claim one presentation boundary.
    presented_field_delivery: PresentedFieldDelivery,
    presented_source_field: Option<fn64_render::PresentedSourceField>,
    presented_post_vi_field: Option<fn64_render::PresentedPostViField>,
}

/// One completed game-derived hottest-state compute differential. The time
/// includes the prototype's uploads, dispatch, waits, and two readbacks; it
/// intentionally does not pretend those costs are shader-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComputeRasterProbeReceipt {
    submission_count: u32,
    batch_count: u32,
    draw_count: u32,
    target_pixels: u32,
    admission_elapsed: Duration,
    elapsed: Duration,
    effects_elapsed: Duration,
}

impl ComputeRasterProbeReceipt {
    pub const fn submission_count(self) -> u32 {
        self.submission_count
    }

    pub const fn batch_count(self) -> u32 {
        self.batch_count
    }

    pub const fn draw_count(self) -> u32 {
        self.draw_count
    }

    pub const fn target_pixels(self) -> u32 {
        self.target_pixels
    }

    pub const fn elapsed(self) -> Duration {
        self.elapsed
    }

    pub const fn admission_elapsed(self) -> Duration {
        self.admission_elapsed
    }

    pub const fn effects_elapsed(self) -> Duration {
        self.effects_elapsed
    }
}

struct ComputeRasterProbe {
    ordinal: u64,
    batch: ComputeRasterBatch,
    extent: TriangleTargetExtent,
    resident_bytes: Vec<u8>,
    triangles: Box<[ComputeCoverageTriangle]>,
    tmem: TmemGpuProjection,
    tile: TileBindingParams,
    expected_bytes: Vec<u8>,
}

/// Replay-only proof vehicle for the task transport: every packet must add
/// at least one complete probe, and every probe must begin with the exact
/// bytes produced by its predecessor. Those constraints make the retained
/// checkpoint limits real packet boundaries rather than a synthetic stream.
struct ComputeRasterCheckpointProbe {
    probes: Vec<ComputeRasterProbe>,
    checkpoint_limits: Vec<usize>,
    packet_count: usize,
    restore_probe_enabled: bool,
}

impl ComputeRasterCheckpointProbe {
    fn new(restore_probe_enabled: bool) -> Self {
        Self {
            probes: Vec::new(),
            checkpoint_limits: Vec::new(),
            packet_count: 0,
            restore_probe_enabled,
        }
    }

    fn push_packet(
        &mut self,
        packet: Vec<ComputeRasterProbe>,
        has_target_write: bool,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        if packet.is_empty() {
            if has_target_write {
                return Err(
                    WgpuRawDpcExecutionError::ComputeRasterCheckpointPacketIneligible {
                        packet: self.packet_count,
                    },
                );
            }
            self.packet_count += 1;
            return Ok(());
        }
        for (index, probe) in packet.iter().enumerate() {
            let previous = if index == 0 {
                self.probes.last()
            } else {
                Some(&packet[index - 1])
            };
            if let Some(previous) = previous {
                if probe.extent != previous.extent
                    || probe.batch.target() != previous.batch.target()
                    || probe.resident_bytes != previous.expected_bytes
                {
                    return Err(
                        WgpuRawDpcExecutionError::ComputeRasterCheckpointDiscontinuity {
                            previous_ordinal: previous.ordinal,
                            ordinal: probe.ordinal,
                        },
                    );
                }
            }
        }
        self.probes.extend(packet);
        self.checkpoint_limits.push(self.probes.len());
        self.packet_count += 1;
        Ok(())
    }
}

struct ComputeRasterDispatch {
    batch: ComputeRasterBatch,
    extent: TriangleTargetExtent,
    triangles: Box<[ComputeCoverageTriangle]>,
    tmem: TmemGpuProjection,
    tile: TileBindingParams,
}

struct ComputeRasterProbeBuilder {
    batch: ComputeRasterBatchBuilder,
    extent: TriangleTargetExtent,
    resident_bytes: Vec<u8>,
    triangles: Vec<ComputeCoverageTriangle>,
    shared_tmem_identity: Option<crate::TmemSnapshotIdentity>,
    shared_tmem: Option<TmemGpuProjection>,
    shared_tile: Option<TileBindingParams>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputeRasterProbePush {
    Admitted,
    SplitDispatch,
    Refused(ComputeRasterAdmissionRefusal),
}

impl ComputeRasterProbeBuilder {
    fn new(candidate: &CandidateColorTarget, resident_bytes: Vec<u8>) -> Self {
        let key = candidate.key();
        Self {
            batch: ComputeRasterBatchBuilder::new(key, candidate.generation()),
            extent: TriangleTargetExtent {
                width: key.extent().width(),
                height: key.extent().height(),
            },
            resident_bytes,
            triangles: Vec::new(),
            shared_tmem_identity: None,
            shared_tmem: None,
            shared_tile: None,
        }
    }

    fn push<S: crate::TmemByteSource + ?Sized>(
        &mut self,
        collector: &ExecutionCollector<'_>,
        candidate: &CandidateColorTarget,
        index: CommandIndex,
        tmem: &S,
    ) -> Result<ComputeRasterProbePush, WgpuRawDpcExecutionError> {
        let scheduled = &collector.plan.raw_triangle_commands[index];
        let draw = collector.plan.triangles[scheduled.triangle_index]
            .draw
            .as_ref()
            .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
        let triangle = decode_scheduled_raw_triangle(collector, index)?;
        let program = match ComputeRasterProgramKey::try_admit(
            candidate.key(),
            draw.combine_params,
            draw.other_mode,
            triangle.flags().textured(),
        ) {
            Ok(program) => program,
            Err(reason) => return Ok(ComputeRasterProbePush::Refused(reason)),
        };
        let accesses = scheduled_raw_triangle_accesses(collector, candidate, index)?;
        let identity = crate::TmemByteSource::snapshot(tmem);
        let projection = crate::project_tmem(tmem);
        let tile = draw
            .tile_binding
            .with_lut_mode(draw.other_mode.texture_lut_mode());
        if self
            .shared_tmem_identity
            .is_some_and(|shared| shared != identity)
            || self.shared_tmem.is_some_and(|shared| shared != projection)
            || self.shared_tile.is_some_and(|shared| shared != tile)
        {
            return Ok(ComputeRasterProbePush::SplitDispatch);
        }
        let admission = match ComputeRasterDrawAdmission::try_new(
            candidate.key(),
            scheduled.command_index,
            scheduled.triangle_index.get(),
            program,
            identity,
            accesses,
        ) {
            Ok(admission) => admission,
            Err(reason) => return Ok(ComputeRasterProbePush::Refused(reason)),
        };
        if let Err(reason) = self.batch.push(admission) {
            return Ok(ComputeRasterProbePush::Refused(reason));
        }
        self.shared_tmem_identity = Some(identity);
        self.shared_tmem = Some(projection);
        self.shared_tile = Some(tile);
        self.triangles.push(
            ComputeCoverageTriangle::from_raw(triangle)
                .with_material(draw.env_color, draw.prim_color)
                .with_program(program.shader_id()),
        );
        Ok(ComputeRasterProbePush::Admitted)
    }

    fn finish_dispatch(self) -> Option<(ComputeRasterDispatch, Vec<u8>)> {
        Some((
            ComputeRasterDispatch {
                batch: self.batch.finish().ok()?,
                extent: self.extent,
                triangles: self.triangles.into_boxed_slice(),
                tmem: self.shared_tmem?,
                tile: self.shared_tile?,
            },
            self.resident_bytes,
        ))
    }

    fn finish(self, ordinal: u64, expected_bytes: Vec<u8>) -> Option<ComputeRasterProbe> {
        let (dispatch, resident_bytes) = self.finish_dispatch()?;
        Some(ComputeRasterProbe {
            ordinal,
            batch: dispatch.batch,
            extent: dispatch.extent,
            resident_bytes,
            triangles: dispatch.triangles,
            tmem: dispatch.tmem,
            tile: dispatch.tile,
            expected_bytes,
        })
    }
}

fn retain_compute_probe_draw<S: crate::TmemByteSource + ?Sized>(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    tmem: &S,
    resident_before: &[u8],
) -> Result<Option<ComputeRasterProbeBuilder>, WgpuRawDpcExecutionError> {
    if let Some(active) = builder.as_mut() {
        if active.push(collector, candidate, index, tmem)? == ComputeRasterProbePush::Admitted {
            return Ok(None);
        }
    }
    let previous = builder.take();
    let mut next = ComputeRasterProbeBuilder::new(candidate, resident_before.to_vec());
    if next.push(collector, candidate, index, tmem)? == ComputeRasterProbePush::Admitted {
        *builder = Some(next);
    }
    Ok(previous)
}

fn flush_compute_probe(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    ordinal: u64,
    expected_bytes: &[u8],
    probes: &mut Vec<ComputeRasterProbe>,
) {
    let Some(builder) = builder.take() else {
        return;
    };
    push_finished_compute_probe(builder, ordinal, expected_bytes, probes);
}

fn push_finished_compute_probe(
    builder: ComputeRasterProbeBuilder,
    ordinal: u64,
    expected_bytes: &[u8],
    probes: &mut Vec<ComputeRasterProbe>,
) {
    if let Some(probe) = builder.finish(ordinal, expected_bytes.to_vec()) {
        probes.push(probe);
    }
}

fn validate_compute_probe_output(
    first_probe: &ComputeRasterProbe,
    last_probe: &ComputeRasterProbe,
    actual_bytes: &[u8],
) -> Result<(), WgpuRawDpcExecutionError> {
    if actual_bytes.len() != last_probe.expected_bytes.len() {
        return Err(WgpuRawDpcExecutionError::ComputeRasterProbeLength {
            expected: last_probe.expected_bytes.len(),
            actual: actual_bytes.len(),
        });
    }
    let Some(byte) = actual_bytes
        .iter()
        .zip(&last_probe.expected_bytes)
        .position(|(actual, expected)| actual != expected)
    else {
        return Ok(());
    };
    let pixel = byte / 2;
    let pair = pixel * 2;
    let expected = u16::from_be_bytes([
        last_probe.expected_bytes[pair],
        last_probe.expected_bytes[pair + 1],
    ]);
    let actual = u16::from_be_bytes([actual_bytes[pair], actual_bytes[pair + 1]]);
    let first_draw = first_probe
        .batch
        .draws()
        .first()
        .expect("a sealed compute batch contains at least one draw");
    let last_draw = last_probe
        .batch
        .draws()
        .last()
        .expect("a sealed compute batch contains at least one draw");
    Err(WgpuRawDpcExecutionError::ComputeRasterProbeMismatch {
        ordinal: first_probe.ordinal,
        first_program: first_draw.program().words(),
        last_program: last_draw.program().words(),
        first_command_index: first_draw.command_index(),
        last_command_index: last_draw.command_index(),
        first_triangle_index: first_draw.triangle_index(),
        last_triangle_index: last_draw.triangle_index(),
        x: pixel as u32 % last_probe.extent.width,
        y: pixel as u32 / last_probe.extent.width,
        expected,
        actual,
    })
}

/// Capacity rationale: 4 is the fixed bounded ceiling for concurrently
/// resident color targets in this slice (a color image, a Z-adjacent second
/// target, and two generations of churn headroom). It is a scope bound, not
/// a measured hardware limit -- `TargetError::RegistryFull` is the loud
/// rejection if a real stream exceeds it, never eviction.
const COLOR_TARGET_REGISTRY_CAPACITY: usize = 4;

/// One admitted color-target write, staged and validated during
/// `execute_raw_dpc` but deliberately **not yet published** into the
/// registry. Keyed by the submission it belongs to so `publish_raw_dpc` can
/// prove the capsule it is about to publish is the same submission that
/// staged this write -- never "whatever fill was staged last".
///
/// Why a deferred token rather than a held `ResidentPublication`: that type
/// exclusively borrows the registry, and the guest-commit call it would have
/// to be held across happens in `fn64-abi`, which reaches this backend only
/// through a `RefCell<Option<Box<dyn RenderBackend>>>` in a `with` block
/// that has already returned by then. A borrow cannot survive that; a token
/// can.
///
/// The staged guest writes live here alongside the
/// `InitializedCandidateColorTarget` so the token carries both halves of the
/// same staged fact and they cannot drift.
struct PendingFillPublication {
    submission: fn64_render_ir::SubmissionIdentity,
    color: PendingColorPublication,
    /// Sparse publication already sealed from the same final accumulator
    /// that produced `guest_writes`. Present only while an ordered CPU member
    /// is waiting for [`OrderedCpuColorBatch::finish_member`] to retain the
    /// full accumulator as its successor input.
    prepared_sparse_checkpoint: Option<SparseInitializedColorCheckpoint>,
    /// The exact N `CompletedWrite`s this fill contributed to the
    /// submission's `BackendEffectReport`, in journal order.
    guest_writes: Vec<CompletedWrite>,
    cpu_phase_attributed: bool,
    exact_physical_coverage: bool,
}

#[derive(Clone, Copy)]
enum PublishedVisualTargetMarker {
    Exact(ColorTargetKey),
    NoColorTarget,
    ComputeCoverageUnavailable,
}

enum PendingColorPublication {
    Full(InitializedCandidateColorTarget),
    Sparse(SparseInitializedColorCheckpoint),
}

impl PendingColorPublication {
    fn full(&self) -> &InitializedCandidateColorTarget {
        match self {
            Self::Full(initialized) => initialized,
            Self::Sparse(_) => panic!("a sparse CPU checkpoint cannot enter a compute segment"),
        }
    }
}

/// One move-only full target threaded across compatible adjacent CPU task
/// members. Each completed member yields a separate sparse publication
/// capability; this value retains only the image the next raster consumes.
struct OrderedCpuColorBatch {
    generations: ColorTargetExecutionBatch,
    tail: Option<InitializedCandidateColorTarget>,
    continuity: Option<OrderedCpuColorContinuity>,
    active: Option<OrderedCpuCandidateReservation>,
}

impl OrderedCpuColorBatch {
    fn new() -> Self {
        Self {
            generations: ColorTargetExecutionBatch::new(),
            tail: None,
            continuity: None,
            active: None,
        }
    }

    fn flush(&mut self, registry: &mut ColorTargetRegistry) -> Result<(), TargetError> {
        assert!(
            self.active.is_none(),
            "an ordered CPU color batch cannot flush an unfinished member"
        );
        if let Some(tail) = self.tail.take() {
            let segment = self
                .continuity
                .take()
                .expect("an ordered CPU tail has continuity authority")
                .finish(tail)?;
            registry.commit_owned_task_shadow_segment(segment)?;
        }
        self.generations = ColorTargetExecutionBatch::new();
        self.continuity = None;
        Ok(())
    }

    fn begin_member(
        &mut self,
        registry: &mut ColorTargetRegistry,
        key: ColorTargetKey,
    ) -> Result<(CandidateColorTarget, Option<(Vec<u8>, ColorCoverageState)>), TargetError> {
        assert!(
            self.active.is_none(),
            "an ordered CPU color candidate must complete before its successor begins"
        );
        if self.tail.as_ref().is_some_and(|tail| tail.key() != key) {
            self.flush(registry)?;
        }
        let (candidate, input) = self.generations.begin_candidate(registry, key)?;
        let reservation = OrderedCpuCandidateReservation::new(&candidate);
        let accumulator = match input {
            TaskColorInput::PriorTaskCheckpoint => {
                let tail = self
                    .tail
                    .take()
                    .expect("a prior task checkpoint owns the CPU accumulator");
                assert_eq!(tail.key(), key);
                assert_eq!(Some(tail.generation()), candidate.predecessor());
                Some(tail.into_task_accumulator())
            }
            TaskColorInput::DurableRegistry if candidate.predecessor().is_none() => Some((
                vec![0; key.range().len() as usize],
                ColorCoverageState::unknown(key.extent()),
            )),
            TaskColorInput::DurableRegistry => None,
        };
        self.active = Some(reservation);
        Ok((candidate, accumulator))
    }

    fn finish_member(
        &mut self,
        mut pending: PendingFillPublication,
    ) -> Result<PendingFillPublication, TargetError> {
        let Some(reservation) = self.active.take() else {
            assert!(
                pending.prepared_sparse_checkpoint.is_none(),
                "a prepared sparse checkpoint requires an active ordered CPU reservation"
            );
            return Ok(pending);
        };
        let prepared_sparse_checkpoint = pending.prepared_sparse_checkpoint.take();
        let PendingColorPublication::Full(initialized) = pending.color else {
            panic!("an ordered CPU member must complete with a full accumulator")
        };
        let checkpoint = match prepared_sparse_checkpoint {
            Some(checkpoint) => checkpoint,
            None => initialized.sparse_checkpoint(&pending.guest_writes)?,
        };
        self.continuity = Some(match self.continuity.take() {
            Some(continuity) => continuity.append(reservation, &initialized)?,
            None => OrderedCpuColorContinuity::start(reservation, &initialized)?,
        });
        self.tail = Some(initialized);
        pending.color = PendingColorPublication::Sparse(checkpoint);
        Ok(pending)
    }
}

/// Failure constructing a fresh [`WgpuBackend`]. Both sources are the T0
/// sealed session split's own fallible constructor
/// (`fn64_render::new_raw_dpc_roles`) and this crate's physical TMEM state
/// identity authority; neither is expected to fail in ordinary operation.
#[derive(Debug)]
pub enum WgpuBackendConstructionError {
    RawDpcRoles(ValidationError),
    PhysicalTmemState(PhysicalTmemError),
}

impl core::fmt::Display for WgpuBackendConstructionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RawDpcRoles(error) => write!(formatter, "raw-DPC role split failed: {error}"),
            Self::PhysicalTmemState(error) => {
                write!(
                    formatter,
                    "physical TMEM state construction failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WgpuBackendConstructionError {}

impl WgpuBackend {
    /// Construct a fresh backend and its paired ABI session together. The
    /// session is the caller's (ABI-side, per T0's role split): this
    /// backend keeps only the paired `RawDpcBackendAuthority`, consumed
    /// immediately into its owned coordinator, exactly as
    /// `RawDpcBackendAuthority::into_coordinator`'s doc comment describes
    /// ("a backend obtains one exactly once, at construction").
    pub fn try_new() -> Result<(Self, RawDpcAbiSession), WgpuBackendConstructionError> {
        Self::try_new_with_knobs(&crate::WgpuKnobs::default())
    }

    /// Construct a backend whose probe/diagnostic policy is the host's,
    /// rather than the documented defaults `try_new` uses.
    ///
    /// This is the seam `fn64-shell` uses: it resolves one process-wide
    /// `Knobs` (flag > `fn64.toml` > `FN64_*` compat > default) and hands the
    /// wgpu slice of it here. Before task 2.2b this crate read those seven
    /// variables itself, at construction, with no way for a caller to state
    /// the policy except by mutating the process environment -- which no test
    /// could do safely in a shared-process test binary.
    pub fn try_new_with_knobs(
        knobs: &crate::WgpuKnobs,
    ) -> Result<(Self, RawDpcAbiSession), WgpuBackendConstructionError> {
        let (session, authority) =
            fn64_render::new_raw_dpc_roles().map_err(WgpuBackendConstructionError::RawDpcRoles)?;
        let initial = PhysicalTmemState::try_new()
            .map_err(WgpuBackendConstructionError::PhysicalTmemState)?;
        Ok((
            Self {
                coordinator: authority.into_coordinator(initial),
                rdp_state: RdpState::default(),
                raw_dpc_carry_in_before_last_plan: None,
                pending_raw_dpc_task_batch: None,
                triangle_pipeline: None,
                triangle_target_extent: None,
                triangle_draw_output: None,
                probes: ProbePolicy::from_knobs(knobs),
                compute_raster_checkpoint_probe: None,
                compute_raster_probe_receipt: None,
                compute_raster_replace_receipt: None,
                task_cpu_phase_census: None,
                last_task_batch_execution_mechanism: None,
                last_published_visual_target: None,
                configured_target_extent: None,
                color_targets: None,
                pending_fill_publication: None,
                task_batch_pending_fill_publications: VecDeque::new(),
                presented_field: None,
                presented_field_delivery: PresentedFieldDelivery::ConcreteDiagnostic,
                presented_source_field: None,
                presented_post_vi_field: None,
            },
            session,
        ))
    }

    /// The most recently presented VI field, or `None` before the first
    /// successful `present`.
    ///
    /// Exposed for diagnostics and tests, mirroring `physical_tmem()` and
    /// `color_targets()`'s convention on this struct. This is the retrieval
    /// half of `RenderBackend::present`'s own contract for a headless
    /// backend ("finalize it as retrievable").
    pub fn presented_field(&self) -> Option<&crate::PresentedField> {
        self.presented_field.as_ref()
    }

    /// Select move-only shell-compatible source-field delivery. Direct
    /// backend users retain the filtered `presented_field` behavior unless
    /// they explicitly commit to consuming this source receipt.
    pub fn enable_presented_source_field_delivery(&mut self) {
        self.presented_field_delivery = PresentedFieldDelivery::Source;
        self.presented_source_field = None;
        self.presented_post_vi_field = None;
        self.presented_field = None;
    }

    /// Consume the most recent game-derived compute-raster probe timing.
    /// `None` means either probing is disabled or the last packet was not
    /// one closed hottest-state batch.
    pub fn take_compute_raster_probe_receipt(&mut self) -> Option<ComputeRasterProbeReceipt> {
        self.compute_raster_probe_receipt.take()
    }

    /// Diagnostic replay control. It changes only whether an additional
    /// CPU/compute differential is collected; production pixels and the CPU
    /// execution path are unchanged.
    pub fn set_compute_raster_probe_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_probe_enabled = enabled;
        self.compute_raster_probe_receipt = None;
    }

    /// Selects ordered on-device chaining for the diagnostic compute probe.
    /// Enabling this does not enable probing by itself.
    pub fn set_compute_raster_chain_probe_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_chain_probe_enabled = enabled;
        self.compute_raster_probe_receipt = None;
    }

    /// Begin an offline task-window checkpoint differential. Normal CPU
    /// execution, guest commits, and publication remain packet-local; only
    /// the additional compute fixtures are retained until `finish`.
    pub fn begin_compute_raster_checkpoint_probe(&mut self) {
        assert!(
            self.compute_raster_checkpoint_probe.is_none(),
            "compute-raster checkpoint probe already active"
        );
        let restore_probe_enabled = self.probes.compute_raster_probe_enabled;
        self.probes.compute_raster_probe_enabled = true;
        self.compute_raster_probe_receipt = None;
        self.compute_raster_checkpoint_probe =
            Some(ComputeRasterCheckpointProbe::new(restore_probe_enabled));
    }

    /// Submit the retained task window once and compare the GPU target at
    /// every original packet boundary with that packet's CPU-produced bytes.
    pub fn finish_compute_raster_checkpoint_probe(
        &mut self,
    ) -> Result<ComputeRasterProbeReceipt, RenderError> {
        let retained = self
            .compute_raster_checkpoint_probe
            .take()
            .expect("compute-raster checkpoint probe is not active");
        self.probes.compute_raster_probe_enabled = retained.restore_probe_enabled;
        let first = retained
            .probes
            .first()
            .expect("an active checkpoint probe accepted at least one packet");
        let dispatches: Vec<_> = retained
            .probes
            .iter()
            .map(|probe| ComputeHotColorDispatch {
                triangles: &probe.triangles,
                tmem: &probe.tmem,
                tile: probe.tile,
                first_row: 0,
                row_count: probe.extent.height,
                first_column: 0,
                column_count: probe.extent.width,
            })
            .collect();
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
            .map_err(RenderError::from)?;
        let started = Instant::now();
        let outputs = pipeline
            .compute_triangle_hot_color_chain_checkpoints(
                first.extent,
                &first.resident_bytes,
                &dispatches,
                &retained.checkpoint_limits,
            )
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)
            .map_err(RenderError::from)?;
        let mut outputs = ExactCheckpointImages::try_new(outputs, retained.checkpoint_limits.len())
            .map_err(RenderError::from)?;
        for &limit in &retained.checkpoint_limits {
            let actual = outputs.take_next();
            let last = &retained.probes[limit - 1];
            validate_compute_probe_output(first, last, &actual).map_err(RenderError::from)?;
        }
        outputs.finish();
        let elapsed = started.elapsed();
        let draw_count = retained.probes.iter().try_fold(0u32, |count, probe| {
            count.checked_add(u32::try_from(probe.batch.draws().len()).ok()?)
        });
        Ok(ComputeRasterProbeReceipt {
            submission_count: 1,
            batch_count: u32::try_from(retained.probes.len())
                .expect("bounded checkpoint batch count fits u32"),
            draw_count: draw_count.expect("bounded checkpoint draw count fits u32"),
            target_pixels: first.extent.width * first.extent.height,
            admission_elapsed: Duration::ZERO,
            elapsed,
            effects_elapsed: Duration::ZERO,
        })
    }

    /// Selects the transaction-integrated compute replacement for eligible
    /// packets. It remains an explicit A/B control until live certification.
    pub fn set_compute_raster_replace_enabled(&mut self, enabled: bool) {
        self.probes.compute_raster_replace_enabled = enabled;
        self.compute_raster_replace_receipt = None;
    }

    pub fn take_compute_raster_replace_receipt(&mut self) -> Option<ComputeRasterProbeReceipt> {
        self.compute_raster_replace_receipt.take()
    }

    pub fn set_task_compute_raster_enabled(&mut self, enabled: bool) {
        self.probes.task_compute_raster_enabled = enabled;
    }

    pub fn set_task_cpu_color_batch_enabled(&mut self, enabled: bool) {
        self.probes.task_cpu_color_batch_enabled = enabled;
    }

    /// The currently-published physical TMEM state. Exposed for diagnostics
    /// and tests; production callers reach committed TMEM content only
    /// through this same coordinator-owned value.
    pub fn physical_tmem(&self) -> &PhysicalTmemState {
        self.coordinator.physical()
    }

    /// The current durable logical RDP state. Exposed for diagnostics and
    /// tests; advances only through a successful `plan_raw_dpc` call.
    pub fn rdp_state(&self) -> &RdpState {
        &self.rdp_state
    }

    /// The resident color targets this backend has published, or `None` if
    /// no admitted `FillRectangle` has ever reached execution (the registry
    /// is built lazily, from the first admitted fill's own capture layout).
    /// Exposed for diagnostics and tests, mirroring `physical_tmem()`'s own
    /// convention on this struct.
    pub fn color_targets(&self) -> Option<&ColorTargetRegistry> {
        self.color_targets.as_ref()
    }

    /// Whether a staged-but-unpublished fill token is currently held.
    /// Exposed for the nonmutation tests, which must be able to prove a
    /// rejected fill left no token behind.
    pub fn has_pending_fill_publication(&self) -> bool {
        self.pending_fill_publication.is_some()
            || !self.task_batch_pending_fill_publications.is_empty()
    }

    /// `RenderBackend::create`'s body: block once, synchronously, on
    /// `UninitializedTrianglePipeline::request()`, storing the resulting
    /// renderer or reporting a richly-typed failure. `pub(crate)`, not
    /// fully `pub` (unlike `WgpuRawDpcExecutionError`, which is public
    /// because external callers reach it through
    /// `RenderBackend::execute_raw_dpc`'s conversion path): nothing
    /// outside this crate has a reason to call `create_inner` instead of
    /// the trait's `create`, so there is no reason to widen this crate's
    /// public API surface for it. It exists, distinct from `create`
    /// itself, only so this module's own `#[cfg(test)]` code can assert
    /// on the exact `WgpuCreateError` variant -- specifically
    /// distinguishing a genuine `NoAdapter` from any other failure --
    /// which `RenderBackend::create`'s own `Result<(), RenderError>`
    /// signature cannot preserve once converted (`RenderError::Backend`'s
    /// `reason` is a plain `String`).
    ///
    /// Also derives and stores `triangle_target_extent` from `cfg`
    /// (§1e: `TriangleTargetExtent { width: cfg.width, height:
    /// cfg.height }`, an identity mapping -- no RDP viewport/scissor
    /// concept exists to derive anything narrower from), so a later
    /// triangle draw knows what render-target size to use without `cfg`
    /// being threaded through every call.
    /// Whether `create_inner` recorded the host-configured framebuffer extent.
    ///
    /// Exposed for the adapterless harness, which asserts the extent survived
    /// a `NoAdapter` create rather than reaching into the field itself.
    #[cfg(any(test, feature = "conformance-runner"))]
    pub(crate) fn has_configured_target_extent(&self) -> bool {
        self.configured_target_extent.is_some()
    }

    /// Keep adapterless conformance fixtures on the authoritative CPU raster
    /// path after `create_inner` has proved that no diagnostic GPU is
    /// available. Real backend construction still returns `NoAdapter`, and
    /// host-GPU tests still require successful creation before reaching this
    /// test-only seam.
    #[cfg(any(test, feature = "conformance-runner"))]
    pub(crate) fn disable_adapterless_gpu_diagnostic(&mut self) {
        assert!(
            self.triangle_pipeline.is_none(),
            "an adapterless fallback cannot discard a live triangle pipeline"
        );
        self.probes.gpu_triangle_draw_enabled = false;
    }

    pub(crate) fn create_inner(&mut self, cfg: &RenderConfig) -> Result<(), WgpuCreateError> {
        // Recorded before the device request, unlike `triangle_target_extent`
        // below: an admitted `FillRectangle` is executed entirely CPU-side
        // and needs only this host-configured height, so a host with no GPU
        // adapter must still be able to execute one. See
        // `configured_target_extent`'s own doc for the nonclaim this carries.
        self.configured_target_extent = Some(TriangleTargetExtent {
            width: cfg.width,
            height: cfg.height,
        });
        // **The same height, given to the DECODER.** `plan_raw_triangle`
        // declares one write per covered scanline and `SetColorImage` carries
        // no height, so without this the decoder's only bound is installed
        // RDRAM -- and a triangle taller than the target declares ranges past
        // its end, which `verify_accesses_inside` then refuses for the whole
        // packet. Set from the SAME `cfg` field as `configured_target_extent`
        // above, on the same line of control flow, so the decoder's bound and
        // the executor's extent cannot drift apart.
        self.rdp_state.set_color_target_height(cfg.height);
        let outcome = pollster::block_on(
            UninitializedTrianglePipeline::new(HeadlessBackend::default()).request(),
        )
        .map_err(WgpuCreateError::Request)?;
        match outcome {
            TrianglePipelineDeviceOutcome::Ready(renderer) => {
                self.triangle_pipeline = Some(renderer);
                self.triangle_target_extent = Some(TriangleTargetExtent {
                    width: cfg.width,
                    height: cfg.height,
                });
                Ok(())
            }
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => {
                Err(WgpuCreateError::NoAdapter(no_adapter))
            }
        }
    }

    /// The most recent triangle draw's real GPU-observed color/depth
    /// output (§1e), or `None` if no triangle-bearing `execute_raw_dpc`
    /// call has succeeded yet. Exposed for diagnostics and tests, mirroring
    /// `physical_tmem()`/`rdp_state()`'s own diagnostic-accessor
    /// convention on this struct -- never an accumulated history, never a
    /// persistent framebuffer.
    pub fn last_triangle_draw(&self) -> Option<&TriangleDrawOutput> {
        self.triangle_draw_output.as_ref()
    }

    /// Test-only escape hatch into the real `TrianglePipelineRenderer` this
    /// backend already owns (post-`create_inner`), so the CPU-vs-WGSL
    /// differential (published committed-TMEM textured-draw card §4/§7) can
    /// call `submit_admitted_triangle` directly with literal
    /// `NeutralTriangleVertex` UVs -- avoiding the separate, already-
    /// characterized RT64 fixed-point texcoord wire-decode pipeline
    /// (`raw_dpc::triangle_vertices::decode_texture`), which is not this
    /// slice's own arithmetic and would only add an unrelated correctness
    /// risk to a differential whose entire purpose is validating the WGSL
    /// TMEM-sampling port itself. `production.rs`'s own `RetrievedTriangleDraw`-
    /// literal test fixtures (e.g. `fixture_vertex`) already establish this
    /// same "construct the vertex/tile-state literally, skip wire decode"
    /// convention for host-GPU pipeline tests.
    #[cfg(all(test, feature = "host-gpu-tests"))]
    pub(crate) fn triangle_pipeline_for_test(&mut self) -> &mut TrianglePipelineRenderer {
        self.triangle_pipeline
            .as_mut()
            .expect("create_inner must succeed before this test accessor is used")
    }

    /// Maps every collected triangle, in stream order, into one
    /// `TriangleFixture` each (via `targets::triangle_pipeline`'s
    /// `admitted_triangle_fixture`, the same conversion
    /// `submit_admitted_triangle` uses for its own single-fixture path),
    /// using the identity `TriangleRasterParams` derived once from the
    /// stored `triangle_target_extent` (never recomputed per triangle,
    /// never defaulted) plus each draw's own viewport-derived
    /// `screen_scale`/`screen_offset`. The whole batch then submits through
    /// exactly one `TrianglePipelineRenderer::submit_triangles(&fixtures)`
    /// call, in the same order the fixtures were collected: one shared
    /// render pass, one `LoadOp::Clear`, no reordering or coalescing of
    /// draws. This is required, not incidental -- `submit_triangles` clears
    /// its color+depth target once, before the first draw in the pass, so
    /// a multi-triangle primitive (a `TextureRectangle`/
    /// `TextureRectangleFlip` always admits as exactly two triangles) or an
    /// ordinary sequence of several `RawTriangle` draws in one
    /// `execute_raw_dpc` call must land in the same pass to all survive
    /// into `last_triangle_draw()`'s single output; submitting them one
    /// call at a time would re-clear the target between triangles and
    /// silently discard every draw but the last.
    ///
    /// A pre-submit mapping error (a missing draw's `MissingTriangleDrawState`)
    /// is surfaced before any fixture reaches the GPU. A batch submission or
    /// shader-status error is only detected after submission -- `complete()`
    /// observes real GPU output -- but `self.triangle_draw_output` is still
    /// never touched until every stage (mapping, submission, readback,
    /// shader-status check) succeeds, so a failure anywhere in the pipeline
    /// leaves the prior successful value in place, unchanged, never cleared:
    /// an old-but-real result outlives a failed attempt to replace it,
    /// matching this file's own "never a silent partial state" convention
    /// elsewhere. Zero triangles is a successful no-op: no fixtures, no
    /// submission, `last_triangle_draw()` untouched.
    fn draw_admitted_triangles(
        &mut self,
        triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        pending_tmem: Option<Vec<TmemGpuProjection>>,
        project_gpu_tmem: bool,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        // **The TMEM byte images this draw samples: one per triangle, from
        // this packet's OWN sealed transaction, not the published slot.**
        //
        // Projected once per triangle, at each triangle's own stream
        // position, because within one packet TMEM is not one image: a
        // packet's own loads change it as they run.
        // `project_pending_tmem_per_triangle` builds the list upstream,
        // where the sealed transaction is still alive, selecting each entry
        // with the same `prefix_before` the CPU texel reader uses. This
        // loop only indexes it.
        //
        // What changed is WHICH image, and it had to. Projecting
        // `self.coordinator.physical()` -- the published, already-committed
        // slot -- reads a state that does not contain this packet's own
        // `LoadBlock`/`LoadTile`/`LoadTLUT`, because publication happens
        // strictly after execution. Measured at `87b2f5b0`: a texrect whose
        // latched combine references `TEXEL0` failed with
        // `TmemSampleFailed { status: 2 }`
        // (`TMEM_SAMPLE_STATUS_INVALID_BYTE`) -- the shader read addresses
        // the published projection reported invalid. The same fixture with
        // `set_combine(0, 0)` executed cleanly, because a combine that
        // references no texel makes the shader short-circuit before it ever
        // samples TMEM. The control passing was never evidence the GPU
        // sampled correctly; it was evidence it never sampled at all. That
        // is why the GPU path had never actually fetched a texrect's texels.
        //
        // `None` is not a silent fallback -- it is a different, narrower
        // packet shape with its own correct answer. A packet that staged no
        // TMEM transaction at all (`StagedOutcome::NoPhysicalSuccessor`: raw
        // triangles, zero loads) has nothing pending to project, and the
        // published slot is then the honest image rather than a substitute
        // for one: there is no load in this packet for it to be missing.
        // A texrect in a packet with no load reaches the `None` arm too,
        // and committed TMEM is the correct image for it for the same
        // reason `TexrectTmemSource::Committed` is on the CPU side: the
        // packet staged no proposal, so durable state is not a substitute
        // for one -- it is what the RDP's TMEM holds. Both paths therefore
        // project the SAME image for the same packet, which is what keeps
        // the GPU raster and the CPU texel reader from disagreeing about a
        // load-free texrect. Neither arm is a guess.
        //
        // One image per triangle either way, so the loop below indexes a
        // single list and never re-decides which source an entry came from.
        // The `None` arm repeats the committed projection because that IS
        // one image for every triangle in a load-free packet -- there is no
        // load in it to change TMEM mid-packet -- so materialising the
        // repeat removes the second code path that could disagree with the
        // first, at a clone the pipeline was going to make per fixture
        // regardless.
        let per_triangle = project_gpu_tmem.then(|| match pending_tmem {
            Some(projections) => projections,
            None => vec![project_committed_tmem(self.coordinator.physical()); triangles.len()],
        });
        // A list shorter than the draw would leave a triangle with no
        // image, and the only images available to substitute are the two
        // this whole change exists to withhold: another triangle's, or the
        // whole-packet post-image. Refused by name rather than padded.
        // `project_pending_tmem_per_triangle` walks
        // `plan.triangle_commands` while `execute_raw_dpc` draws
        // `plan.triangles`, two vectors pushed at one site, so a mismatch
        // is a structural break rather than a length a caller could
        // legitimately vary.
        if let Some(per_triangle) = &per_triangle {
            if per_triangle.len() != triangles.len() {
                return Err(WgpuRawDpcExecutionError::TmemProjectionCountMismatch {
                    projections: per_triangle.len(),
                    triangles: triangles.len(),
                });
            }
        }

        let mut fixtures = Vec::with_capacity(triangles.len());
        for (triangle_index, draw) in triangles.into_iter().enumerate() {
            let draw = draw.map_err(WgpuRawDpcExecutionError::MissingTriangleDrawState)?;
            // **`tlut_en` is a `SetOtherModes` bit, not a `SetTile` field.**
            // Neither `TileBindingParams` constructor can know it, so the
            // TLUT mode is stamped here from the SAME `RetrievedTriangleDraw`
            // snapshot the combiner/blend/coverage state above came from --
            // the `OtherMode` current at THIS triangle's own stream
            // position, never the walk's running final value.
            //
            // Without this the shader saw `lut_mode = Disabled` for every
            // draw and refused WM2000's IA4-under-`G_TT_RGBA16` tile with
            // `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT`, for a format the
            // hardware ignores while the TLUT is on.
            let lut_mode = draw.other_mode.texture_lut_mode();
            let tile_binding = draw.tile_binding.with_lut_mode(lut_mode);
            let blend_mode = BlendModeState {
                other_mode: draw.other_mode,
                blend_color_register: draw.blend_color.rgba8(),
                fog_color: draw.fog_color.rgba8(),
            };
            let cycle_count = blend_mode.cycle_count();
            let cycle0 = blend_mode.cycle(0);
            let cycle1 = blend_mode.cycle(1);
            let active_cycles = match cycle_count {
                0 => [None, None],
                1 => [Some(cycle0), None],
                _ => [Some(cycle0), Some(cycle1)],
            };
            if active_cycles
                .into_iter()
                .flatten()
                .any(ResolvedBlendCycle::requires_framebuffer_alpha)
            {
                return Err(WgpuRawDpcExecutionError::BlendRequiresFramebuffer { triangle_index });
            }
            let reads_framebuffer_color = active_cycles.into_iter().flatten().any(|cycle| {
                matches!(cycle.p, BlendColorInput::Framebuffer)
                    || matches!(cycle.m, BlendColorInput::Framebuffer)
            });
            let blend_params = ResolvedFragmentBlendParams {
                cycle_count,
                cycle0,
                cycle1,
                blend_color: draw.blend_color,
                fog_color: draw.fog_color,
                reads_framebuffer_color,
            };

            // Everything above is renderer-neutral admission and always
            // runs. Extent, TMEM projection, and pipeline construction below
            // belong only to the optional diagnostic GPU draw. Requiring
            // `RenderBackend::create` for them when that draw is disabled
            // would reject the authoritative CPU raster path in adapterless
            // ABI consumers.
            if !self.probes.gpu_triangle_draw_enabled {
                continue;
            }
            let extent = self
                .triangle_target_extent
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            // Per-triangle, not loop-invariant: a `TextureRectangle`-sourced
            // draw's `screen_scale`/`screen_offset` come from its own
            // `viewport` override (RT64's `convertViewportRect`,
            // `rt64_framebuffer_renderer.cpp:1656-1658`); a `RawTriangle`
            // keeps today's hardcoded identity, byte-identical to before
            // this field existed.
            let (screen_scale, screen_offset) = match draw.viewport {
                None => ([1.0, 1.0], [0.0, 0.0]),
                Some(viewport) => {
                    let left = viewport.left as f32;
                    let top = viewport.top as f32;
                    let right = viewport.right as f32;
                    let bottom = viewport.bottom as f32;
                    let width = extent.width as f32;
                    let height = extent.height as f32;
                    (
                        [(right - left) / width, (bottom - top) / height],
                        [
                            (left + (right - left) / 2.0 - width / 2.0) / (width / 2.0),
                            (height / 2.0 - (top + (bottom - top) / 2.0)) / (height / 2.0),
                        ],
                    )
                }
            };
            let raster_params = TriangleRasterParams {
                resolution: [extent.width as f32, extent.height as f32],
                screen_scale,
                screen_offset,
            };
            if let Some(per_triangle) = &per_triangle {
                fixtures.push(admitted_triangle_fixture(
                    draw.vertices,
                    draw.other_mode,
                    draw.combine_params,
                    raster_params,
                    extent,
                    per_triangle[triangle_index],
                    tile_binding,
                    draw.blend_color,
                    draw.env_color,
                    draw.prim_color,
                    blend_params,
                    draw.source == TriangleSource::TextureRectangle,
                ));
            }
        }

        if !self.probes.gpu_triangle_draw_enabled {
            return Ok(());
        }
        if fixtures.is_empty() {
            self.triangle_pipeline
                .as_ref()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            return Ok(());
        }

        // **Everything above this line is admission validation and always runs.**
        //
        // An earlier version of this gate sat at the CALL SITE and skipped
        // this whole function on the play path. That was wrong: this is not
        // a pure output-populating draw, it is a fallible validation
        // boundary. Skipping it returned `Ok(())` for packets that should
        // have been refused -- `TriangleDrawBeforeCreate`,
        // `TmemProjectionCountMismatch`, `MissingTriangleDrawState`,
        // `BlendRequiresFramebuffer` above all
        // stopped firing. Found by an independent audit, which also showed
        // `TriangleDrawBeforeCreate` is narrower: it belongs to the optional
        // GPU diagnostic draw and is checked only when that draw is enabled.
        //
        // Only the GPU submission below is skipped, and only its *output* is
        // diagnostic: `triangle_draw_output` is "never an accumulated
        // history, never a persistent framebuffer" and `present` refuses to
        // scan it out. Guest pixels come from the CPU rasterizer.
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let in_flight = pipeline
            .submit_triangles(&fixtures)
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
        let output = in_flight
            .complete()
            .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
        // Observable shader failure status (card audit repair): propagate
        // any fragment's non-OK `tmem_sample.wgsl` status to a named Rust
        // execution error -- never silently accepted as though the batch's
        // texture sampling succeeded everywhere.
        if let Some(&status) = output
            .tmem_sample_status
            .iter()
            .find(|&&status| status != TMEM_SAMPLE_STATUS_OK)
        {
            // **Measure the tile, do not infer it.** The status attachment
            // is per-pixel over the whole batch, so a status code alone
            // names no triangle. Attribution is exact for the format
            // refusal specifically: `tmem_sample.wgsl` raises
            // `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT` from a pure predicate
            // over `TileBindingParams` alone, before any addressing runs,
            // so re-evaluating that same predicate over this batch's own
            // fixtures identifies exactly the fixtures that could have
            // written it. For the other statuses (which depend on the
            // fragment's UVs, not only its tile) the first bound fixture is
            // reported instead, and the triangle index is the only field
            // that is then approximate.
            let culprit = fixtures
                .iter()
                .position(|fixture| {
                    fixture.tile_binding.bound != 0
                        && !fixture.tile_binding.is_supported_direct_format()
                })
                .or_else(|| {
                    fixtures
                        .iter()
                        .position(|fixture| fixture.tile_binding.bound != 0)
                })
                .unwrap_or(0);
            let tile = fixtures[culprit].tile_binding;
            return Err(WgpuRawDpcExecutionError::TmemSampleFailed {
                status,
                triangle_index: culprit,
                tile_format: tile.format,
                tile_pixel_size: tile.pixel_size,
                tile_lut_mode: tile.lut_mode,
            });
        }
        self.triangle_draw_output = Some(output);
        Ok(())
    }
}

/// Named, loud rejection for one `create_inner` call -- kept distinct
/// from `WgpuRawDpcExecutionError`/`WgpuBackendConstructionError` because
/// this is specifically the triangle-pipeline device-request failure
/// surface. `pub(crate)`, matching `create_inner`'s own visibility (see
/// its doc comment): this exists so this crate's own tests can
/// distinguish `NoAdapter` (no exotic device failure, just no matching
/// adapter on this host) from `Request` (a genuine `TrianglePipelineError`
/// -- adapter/device request rejected, or the pipeline prewarm itself
/// reported a device error) before either is collapsed into
/// `RenderError::Backend`'s plain `String` reason at the trait boundary.
#[derive(Debug)]
pub(crate) enum WgpuCreateError {
    NoAdapter(crate::NoAdapter),
    Request(TrianglePipelineError),
}

impl core::fmt::Display for WgpuCreateError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoAdapter(no_adapter) => write!(
                formatter,
                "no GPU adapter available for the triangle-draw pipeline: {no_adapter:?}"
            ),
            Self::Request(error) => {
                write!(
                    formatter,
                    "triangle-pipeline device request failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WgpuCreateError {}

impl From<WgpuCreateError> for RenderError {
    fn from(error: WgpuCreateError) -> Self {
        RenderError::Backend {
            backend: "render-wgpu/create",
            reason: error.to_string(),
        }
    }
}

/// An index into `PlanCollector::triangles` (and, index-parallel with it,
/// `PlanCollector::triangle_commands`). Distinguished from
/// [`CommandIndex`] because the two spaces disagree: a triangle index
/// counts admitted triangles, a command index counts wire commands, and a
/// texture rectangle's two triangles share one command index but take two
/// consecutive triangle indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct TriangleIndex(u32);

impl TriangleIndex {
    fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("triangle index fits in u32"))
    }

    fn get(self) -> usize {
        self.0 as usize
    }
}

/// An index into `PlanCollector::raw_triangle_commands`. See
/// [`TriangleIndex`] for why this is not the same space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CommandIndex(u32);

impl CommandIndex {
    fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("command index fits in u32"))
    }

    fn get(self) -> usize {
        self.0 as usize
    }
}

/// One admitted `Triangle` command's retrieved draw state, folded with its
/// index-parallel neutral tile table.
///
/// Previously two separate `Vec`s on `PlanCollector` (`triangles` and
/// `triangle_neutral_tiles`), pushed at the same site and read at the same
/// index at every call site; folding them into one `Vec<PlannedTriangle>`
/// makes that parallelism a type-level invariant instead of a convention
/// documented on each field.
struct PlannedTriangle {
    draw: Result<RetrievedTriangleDraw, MissingTriangleDrawState>,
    neutral_tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
}

impl std::ops::Index<TriangleIndex> for Vec<PlannedTriangle> {
    type Output = PlannedTriangle;

    fn index(&self, index: TriangleIndex) -> &Self::Output {
        &self[index.get()]
    }
}

impl std::ops::Index<CommandIndex> for Vec<ScheduledRawTriangle> {
    type Output = ScheduledRawTriangle;

    fn index(&self, index: CommandIndex) -> &Self::Output {
        &self[index.get()]
    }
}

/// Collects every TMEM load in the complete neutral plan, in plan order
/// (`command_index` records each load's position among *every* plan
/// command, matching T1's own `push_decoded_raw_dpc` numbering, even though
/// `State` commands are not retained here), plus every access, plus every
/// admitted `Triangle` command's own vertices/command-time `OtherMode`/
/// `CombineParams` snapshot, exactly as
/// [`fn64_render::ExactValidatedRawDpcPlan::visit`] lends them through
/// [`BoundSubmittedRawDpc::execution_view`]/
/// [`RawDpcCoordinator::execution_view`] -- nonextracting, borrowed for the
/// duration of one `execution_view` call only. This is the sole route
/// `execute_raw_dpc` uses to reach plan contents; it never widens access to
/// a bare ticket. `State` commands other than `SetOtherMode`/`SetCombine`
/// (`SetTile`/`SetTileSize`/`SetTextureImage`/`SyncLoad`, etc.) carry no
/// resource access of their own and no field this executor reads --
/// `TmemLoadSemantics` already carries its own staged
/// `source_image`/`tile_descriptor`/`epoch` directly -- so they are counted
/// for `command_index` continuity but not stored.
///
/// The `Triangle`/`SetOtherMode`/`SetCombine` handling below deliberately
/// duplicates `raw_dpc::triangle_draw_data::TriangleDrawStateCollector`'s
/// exact per-command logic (the walk-local [`RdpDrawState`]'s
/// `other_mode`/`combine`, snapshotted onto each triangle at its own stream
/// position, never a single whole-plan-final value) rather than reusing
/// that type directly: `RawDpcExecutionView::plan_visited` is generic over
/// exactly one visitor type, fixed at this file's own `execute_raw_dpc_
/// inner` call site, so there is no route to lend one sealed plan to two
/// independent visitors in the same `execution_view` call. This is a
/// duplication of behavior, not of trust -- if `TriangleDrawStateCollector`
/// changes, this file's own copy must be updated to match.
struct PlanCollector {
    loads: Vec<(u32, TmemLoadSemantics)>,
    accesses: Vec<ResourceAccess>,
    next_command_index: u32,
    /// Every durable RDP draw register current at the walk's current stream
    /// position -- seeded from `WgpuBackend.rdp_state`'s durable values at
    /// construction time (`Self::seeded`), then advanced by
    /// [`RdpDrawState::apply`] on every state command in plan order.
    ///
    /// Each snapshot taken onto a `PlannedTriangle` or a fill is taken at
    /// that command's own stream position -- never a single
    /// whole-plan-final value.
    draw: RdpDrawState,
    /// One entry per admitted `Triangle` command, in plan order. `draw`'s
    /// `Err` names exactly which state (`OtherMode` or `CombineParams`) was
    /// still unset at that triangle's own stream position -- never a
    /// silent default, matching `TriangleDrawStateCollector`'s own
    /// documented absence handling. `neutral_tiles` is the **neutral**
    /// `SetTile`/`SetTileSize` table current at that same stream position
    /// (see [`PlannedTriangle`] for why this is folded rather than a
    /// second parallel `Vec`).
    triangles: Vec<PlannedTriangle>,
    /// One entry per admitted `FillRectangle`, in plan order: its
    /// decode-order command index, command, and render state current at
    /// **its own** stream position. Fill cycle consumes the command's fill
    /// color; one-/two-cycle consumes the snapshotted combiner and color
    /// registers through the existing texture-rectangle pixel stages.
    ///
    /// The `OtherMode` snapshot is not redundant with
    /// `draw.other_mode`. That field is the walk's running value, which
    /// by the end of the plan holds whatever the *last* `SetOtherMode` set
    /// -- and a real stream sets Fill cycle for the fill and then Copy
    /// cycle for a following texture rectangle, so reading the running
    /// value at execute time rejects the fill with `NotFillCycle` for a
    /// mode it never ran under. Snapshotting per command is the same rule
    /// `triangles` already follows and states in its own doc: "never a
    /// single whole-plan-final value".
    ///
    /// The `RdpScissorRect` snapshot is taken at the same stream position
    /// and for the same reason. Pinned RT64 intersects its current scissor
    /// with each draw rectangle (`src/hle/rt64_rdp.cpp:1214-1223`, commit
    /// `f0728a2`), so fn64 snapshots the scissor current where THIS fill
    /// sits, not the one a later `SetScissor` installs for a following
    /// primitive. The exact relatch sequencing is fn64's own reading and is
    /// not independently confirmed against an allowed hardware reference.
    /// `None` here means
    /// the plan issued no `SetScissor` before this fill, and the consumer
    /// (`fill_scissor_or_full_target`) supplies the whole-target fallback,
    /// exactly as the texrect path already does.
    fills: Vec<(
        u32,
        fn64_render::RdpFillRectangleCommand,
        Option<OtherMode>,
        Option<crate::targets::RdpScissorRect>,
        Option<CombineParams>,
        Color4,
        PrimColor,
        Color4,
        Color4,
    )>,
    /// One entry per admitted `FullSync` site, in plan order, paired with
    /// its own decode-order command index.
    ///
    /// Collected for accounting only -- this backend performs no GPU work
    /// for a sync and schedules no DP completion (the device fabric does
    /// that, from the ABI seam). Retaining the site keeps the executed plan
    /// able to account for every command it carried instead of silently
    /// losing one.
    full_sync_sites: Vec<(u32, fn64_render::RdpFullSyncSite)>,
    /// The wire command index each admitted triangle was produced at,
    /// parallel to `triangles` and pushed at the same site each
    /// `PlannedTriangle` is.
    ///
    /// A texture rectangle contributes two entries carrying the *same*
    /// index: both halves come from one wire command and both must sample
    /// TMEM as of that one stream position. Splitting them would let a
    /// rectangle's two triangles disagree about which load they saw.
    ///
    /// Needed because the GPU raster path selects a TMEM projection per
    /// triangle for exactly the reason the CPU texel reader selects a
    /// prefix per texrect: within one packet, TMEM is not one image.
    triangle_commands: Vec<u32>,
    /// One entry per admitted `TextureRectangle` **wire command**, in plan
    /// order: the declared `RenderTarget` write-access span the decoder
    /// recorded for it (`None` when it declared no write), and the index
    /// into `triangles` of the first of the two triangles it was admitted
    /// as.
    ///
    /// A texrect is admitted as two `TriangleSource::TextureRectangle`
    /// triangles (`production_adapter`'s own split) and **both halves carry
    /// the identical span**, so counting texrects means counting distinct
    /// originating commands, not triangles -- adjacent pairs are collapsed
    /// here. Counting triangles instead would double every texrect and
    /// reject a single legal one as two.
    texrect_commands: Vec<(
        Option<fn64_render::TriangleAccessSpan>,
        usize,
        u8,
        u32,
        bool,
    )>,
    /// One entry per admitted **flat raw triangle** that declared a
    /// destination write: its declared access span, its index into
    /// `triangles`, its own stream command index, and its exact decoded
    /// wire payload.
    ///
    /// Separate from `texrect_commands` because the two use different
    /// executors, and the pairing rule that collapses a texrect's two halves
    /// has no analogue here: a raw triangle is exactly one triangle. Merging
    /// them would mean re-deriving "which kind is this" at execute time from
    /// a field the collector already knows at push time.
    ///
    /// A raw triangle that declared NO write is absent from this list
    /// entirely -- `None` and "not present" would be the same value, and
    /// this list's only consumer is the schedule, which must not schedule
    /// an undeclared triangle.
    raw_triangle_commands: Vec<ScheduledRawTriangle>,
}

struct ScheduledRawTriangle {
    span: fn64_render::TriangleAccessSpan,
    triangle_index: TriangleIndex,
    command_index: u32,
    decoded: Result<crate::raw_dpc::RawTriangle, ScheduledRawTriangleDecodeError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScheduledRawTriangleDecodeError {
    MissingOpcode,
    Decode(crate::raw_dpc::TriangleDecodeError),
}

impl PlanCollector {
    /// Seeds the whole [`RdpDrawState`] from `WgpuBackend`'s own durable
    /// `rdp_state` instead of `None` -- a real constructor parameter, never
    /// a synthetic plan-stream entry. This is the draw-state-*retrieval*
    /// half of durable cross-submission carry-in; the admission-time half
    /// (`push_decoded_raw_dpc`'s own `TriangleBeforeAnyOtherMode` gate)
    /// already seeds identically from the same `rdp_state`, via
    /// `decode_raw_dpc`'s existing `durable_state` parameter -- see this
    /// card's own design notes for why neither half needed a signature
    /// change to close this gap.
    fn seeded(carry_in: RawDpcCarryIn) -> Self {
        Self {
            loads: Vec::new(),
            accesses: Vec::new(),
            next_command_index: 0,
            draw: carry_in.draw,
            triangles: Vec::new(),
            fills: Vec::new(),
            full_sync_sites: Vec::new(),
            triangle_commands: Vec::new(),
            texrect_commands: Vec::new(),
            raw_triangle_commands: Vec::new(),
        }
    }
}

impl ExactRawDpcPlanVisitor for PlanCollector {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
        let command_index = self.next_command_index;
        self.next_command_index += 1;
        match command {
            RawDpcSemanticCommandRef::TmemLoad(load) => {
                self.loads.push((command_index, load.clone()));
            }
            // Every state command's update rule lives in
            // `RdpDrawState::apply` -- the plan walk and the carry-in value
            // advance by the same code.
            RawDpcSemanticCommandRef::State(state) => self.draw.apply(state),
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand {
                vertices,
                source,
                viewport,
                texrect_accesses,
                raw_words,
                ..
            }) => {
                let triangle_index = self.triangles.len();
                // **The tile this draw actually names, not tile 0.**
                //
                // A `TextureRectangle` selects its tile in wire word 1 bits
                // 26:24 -- the same field the `texrect_commands` push below
                // already reads, from the same retained `raw_words`, so this
                // is one field read at one more site, not a second decode
                // path. Binding tile 0 unconditionally was measured to
                // report `TMEM_SAMPLE_STATUS_NO_TILE_BINDING` for every
                // texrect naming any other tile, and this crate's own
                // composed fixtures name tile 7; two of them had to be moved
                // to tile 0 to exercise the GPU path at all before this.
                //
                // A `RawTriangle` names its tile in wire word 0 bits
                // 18:16 -- the same field `RawTriangle::decode` reads as
                // `tile` and `execute_scheduled_raw_triangle` (the CPU
                // reader) already binds from. This arm previously froze the
                // index to 0, with a comment claiming the triangle "carries
                // no tile field of its own to read". That claim was false,
                // and the consequence was silent: a triangle naming any
                // other tile had the GPU uniform sample tile 0's descriptor
                // instead of its own. Reading the field here from the
                // command's own retained `raw_words` is the same one-field
                // read the texrect arm above already performs, so the two
                // paths resolve the SAME tile for the same triangle.
                //
                // `draw.tiles` is the whole 8-entry table as of this
                // command's stream position (the same table
                // `triangle_neutral_tiles` snapshots for the CPU reader), so
                // the two paths now resolve the SAME tile for the same
                // draw -- texrect or raw triangle alike -- instead of
                // disagreeing whenever tile != 0.
                // The shared implementation, not a second copy: this file
                // and `TriangleDrawStateCollector` must resolve the same
                // tile for the same draw, and when each carried its own
                // copy of this arithmetic they drifted.
                let bound_tile_index = crate::raw_dpc::bound_tile_index(*source, raw_words);
                let tile_binding = match self
                    .draw
                    .tiles
                    .get(bound_tile_index)
                    .copied()
                    .unwrap_or((None, None))
                {
                    (Some(descriptor), Some(size)) => {
                        TileBindingParams::from_neutral(descriptor, size)
                    }
                    _ => TileBindingParams::unbound(),
                };
                let snapshot = (|| {
                    let other_mode = self
                        .draw
                        .other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .draw
                        .combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    // Retrieval-time admission gate (card §4a), duplicated
                    // from `TriangleDrawStateCollector` per this struct's
                    // own module doc: `Dither` never reaches
                    // `submit_admitted_triangle` -- a loud, named panic here,
                    // not a silent None/Threshold coercion. Wire encoding 2
                    // is not reserved. Pinned RT64's shader branches only
                    // for `G_AC_DITHER` and `G_AC_THRESHOLD`, so wire
                    // encoding 2 falls through to no compare
                    // (`src/shaders/RasterPS.hlsl:203-213`, commit
                    // `f0728a2`).
                    match other_mode.alpha_compare() {
                        AlphaCompare::Dither => panic!(
                            "triangle #{triangle_index} (plan order) selected G_AC_DITHER \
                             alpha-compare, which has no fragment-callable RT64 PRNG binding in \
                             this pipeline (no frame-count uniform exists to seed it honestly; \
                             see fn64-alpha-compare-production-card.md \u{a7}2)"
                        ),
                        // `Threshold` compares the fragment alpha against
                        // `G_SETBLENDCOLOR.a`. That register always holds a
                        // value -- zero until the guest writes one -- so
                        // there is nothing to refuse here: a plan with no
                        // `SetBlendColor` compares against 0, and
                        // `alpha >= 0` passes, which is what the reference
                        // lane and RT64 both do. See `RdpState`'s
                        // constant-color field doc for the citations.
                        AlphaCompare::Threshold | AlphaCompare::None => {}
                    };
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        source: *source,
                        viewport: *viewport,
                        other_mode,
                        combine_params,
                        tile_binding,
                        blend_color: self.draw.blend_color,
                        env_color: self.draw.env_color,
                        prim_color: self.draw.prim_color,
                        fog_color: self.draw.fog_color,
                        scissor: self.draw.scissor,
                        prim_depth: self.draw.prim_depth,
                    })
                })();
                // The whole tile table as of this command's own stream
                // position, not just tile 0's entry: a texture rectangle
                // names its own tile in its wire word and this file cannot
                // know which until it reads that word at execute time.
                self.triangles.push(PlannedTriangle {
                    draw: snapshot,
                    neutral_tiles: self.draw.tiles,
                });
                // **A texture rectangle's two triangles take the FIRST
                // half's command index, not their own.**
                //
                // The adapter assigns each half its own index -- measured
                // on the sprite-strip fixture, the halves are (11, 12),
                // (20, 21), ... -- so pushing `command_index` unchanged
                // would let the two halves select prefixes independently.
                // In that fixture no load falls between 11 and 12, so it
                // happens to be harmless; that is an accident of the
                // spacing, not a guarantee. A rectangle whose halves
                // straddled a load would tear along its own diagonal, one
                // triangle carrying texels the other never saw.
                //
                // The pairing rule is `texrect_commands`' own
                // `previous_was_first_half`, applied here rather than
                // re-derived, so one fact about "which triangles are one
                // rectangle" has one implementation.
                let second_half = *source == TriangleSource::TextureRectangle
                    && self
                        .texrect_commands
                        .last()
                        .is_some_and(|(_, first, _, _, _)| *first + 1 == triangle_index);
                self.triangle_commands.push(if second_half {
                    *self
                        .triangle_commands
                        .last()
                        .expect("a second half always follows a first half that pushed its own")
                } else {
                    command_index
                });
                // One texture rectangle is admitted as TWO
                // `TriangleSource::TextureRectangle` triangles sharing one
                // originating wire command, and the adapter pushes them
                // back to back with identical `location`. Recording only
                // the first of each adjacent pair recovers the count of
                // *commands*, which is what the declared-write span is
                // keyed on -- counting triangles would double every
                // texrect and reject a single legal one as "two".
                // A raw triangle carries its own declared span in the same
                // field a texrect uses, because the adapter pushes both
                // through one `RdpTriangleCommand`. `None` means it declared
                // no write -- outside the flat-opaque subset, no staged
                // colour image, Fill cycle, or a row outside installed RDRAM
                // -- and it is simply absent here, so the schedule cannot
                // reach it.
                if *source == TriangleSource::RawTriangle {
                    if let Some(span) = *texrect_accesses {
                        // Decode the triangle's authoritative wire words
                        // directly into the exact scheduled carrier. The
                        // neutral plan still owns those words for provenance;
                        // the executor must not reconstruct coefficients from
                        // the lossy projected `NeutralTriangleVertex` triple.
                        let decoded = raw_words.first().map_or(
                            Err(ScheduledRawTriangleDecodeError::MissingOpcode),
                            |word| {
                                let opcode = ((word >> 24) & 0x3f) as u8;
                                crate::raw_dpc::RawTriangle::decode_u32_words(opcode, raw_words)
                                    .map_err(ScheduledRawTriangleDecodeError::Decode)
                            },
                        );
                        self.raw_triangle_commands.push(ScheduledRawTriangle {
                            span,
                            triangle_index: TriangleIndex::new(triangle_index),
                            command_index,
                            decoded,
                        });
                    }
                }
                if *source == TriangleSource::TextureRectangle {
                    let previous_was_first_half = self
                        .texrect_commands
                        .last()
                        .is_some_and(|(_, first, _, _, _)| *first + 1 == triangle_index);
                    if !previous_was_first_half {
                        // The tile index is wire word 1 bits 26:24, the
                        // same field `texrect_words_in_target` writes and
                        // `RawTextureRectangle` decodes. Read from the
                        // command's own retained `raw_words` rather than
                        // re-decoded from a second source.
                        let tile_index = raw_words
                            .get(1)
                            .map(|word| ((word >> 24) & 0x7) as u8)
                            .unwrap_or(0);
                        self.texrect_commands.push((
                            *texrect_accesses,
                            triangle_index,
                            tile_index,
                            command_index,
                            raw_words
                                .first()
                                .is_some_and(|word| ((word >> 24) & 0x3f) == 0x25),
                        ));
                    }
                }
            }
            // Mandatory alongside `push_fill_rectangle`'s admission: the
            // enum is `#[non_exhaustive]`, so a produced variant with no arm
            // here falls into the catch-all below and panics at execute time
            // rather than failing to compile.
            RawDpcSemanticCommandRef::FillRectangle(fill) => {
                self.fills.push((
                    command_index,
                    fill.clone(),
                    self.draw.other_mode,
                    self.draw.scissor,
                    self.draw.combine,
                    self.draw.env_color,
                    self.draw.prim_color,
                    self.draw.blend_color,
                    self.draw.fog_color,
                ));
            }
            // Mandatory alongside `push_full_sync_site`'s admission, for the
            // same `#[non_exhaustive]` reason as the arm above.
            //
            // Collected, not executed. A `SYNC_FULL` site has no GPU work:
            // its whole effect is on the RDP pipeline and the DP interrupt
            // line, and the DP completion is scheduled by the device fabric
            // (`start_dp_full_sync`, driven from the ABI seam), never by this
            // backend. Dropping it silently would be wrong in the other
            // direction, though -- the site is retained so the executed plan
            // still accounts for every command the plan carried.
            //
            // Nonclaim: retaining a site here is not an observation of a DP
            // interrupt. `site.boundary.interrupt_after()` is the only field
            // that could carry one, and this backend never writes it.
            RawDpcSemanticCommandRef::FullSyncSite(site) => {
                self.full_sync_sites.push((command_index, site.clone()));
            }
            other => unreachable!(
                "RawDpcSemanticCommandRef gained a variant WgpuBackend does not know about: \
                 {other:?}"
            ),
        }
    }

    fn access(&mut self, access: ResourceAccess) {
        self.accesses.push(access);
    }
}

/// Execution-view sink that drives the complete TMEM staging pipeline.
/// `RawDpcExecutionView`'s three callbacks fire in a fixed order --
/// `plan_visited`, then `captured_reads`, then `submitted_packet` -- and
/// none of `CapturedGuestRead`, `WorkloadPacket`, or the lent plan itself
/// is `Clone` or outlives the call. Rather than trying to retain borrowed
/// data past `execution_view`'s return (which the sealed API does not
/// allow), this collector accumulates the plan and captured reads in the
/// first two callbacks, then performs the entire stage/finish/effect-report
/// pipeline inside `submitted_packet` -- the one callback where
/// `&WorkloadPacket` (which `BackendEffectReport::try_new` requires) is
/// still in scope. `outcome` carries the result out; `execute_raw_dpc_inner`
/// takes it after `execution_view` returns.
struct CapturedGuestReadBytes(Arc<[u8]>);

impl CapturedGuestReadBytes {
    fn copied(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[cfg(test)]
    fn shares_allocation_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One packet's journal binding to immutable captured bytes. This remains a
/// distinct value even when the task pool shares its payload allocation with
/// another packet's binding.
struct CapturedGuestReadBinding {
    read: DeferredGuestRead,
    bytes: CapturedGuestReadBytes,
}

struct IndexedCapturedGuestRead {
    access: ResourceAccess,
    bytes: CapturedGuestReadBytes,
}

/// Packet-sized, access-indexed authority over finalized captured reads.
///
/// `pending` exists only between `captured_reads` and `submitted_packet` in
/// the sealed execution view. Binding consumes it once, validates every
/// descriptor against the packet's exact journal access, and leaves direct
/// indexing as the only production lookup path.
#[derive(Default)]
struct CapturedGuestReadAuthority {
    pending: Vec<CapturedGuestReadBinding>,
    by_access: Vec<Option<IndexedCapturedGuestRead>>,
}

impl CapturedGuestReadAuthority {
    fn clear_and_reserve(&mut self, len: usize) {
        self.pending.clear();
        self.pending.reserve(len);
        self.by_access.clear();
    }

    fn push(&mut self, read: DeferredGuestRead, bytes: CapturedGuestReadBytes) {
        self.pending.push(CapturedGuestReadBinding { read, bytes });
    }

    fn bind_packet(&mut self, packet: &WorkloadPacket) -> Result<(), WgpuRawDpcExecutionError> {
        self.bind_accesses(packet.journal().accesses())
    }

    fn bind_accesses(
        &mut self,
        accesses: &[ResourceAccess],
    ) -> Result<(), WgpuRawDpcExecutionError> {
        self.by_access.clear();
        self.by_access.resize_with(accesses.len(), || None);

        for binding in self.pending.drain(..) {
            let access_index = binding.read.access_index();
            let index = usize::try_from(access_index).map_err(|_| {
                WgpuRawDpcExecutionError::CapturedSourceAccessOutOfRange { access_index }
            })?;
            let expected = accesses
                .get(index)
                .copied()
                .ok_or(WgpuRawDpcExecutionError::CapturedSourceAccessOutOfRange { access_index })?;
            let descriptor_matches = binding.read.operation() == expected.operation()
                && expected.mode() == AccessMode::Read
                && expected.purpose() == AccessPurpose::TmemLoadSource
                && matches!(
                    expected.region(),
                    ResourceRegion::Rdram { resource, range }
                        if resource == binding.read.resource() && range == binding.read.range()
                );
            if !descriptor_matches {
                return Err(WgpuRawDpcExecutionError::CapturedSourceAccessMismatch {
                    access_index,
                });
            }
            let slot = &mut self.by_access[index];
            if slot.is_some() {
                return Err(WgpuRawDpcExecutionError::DuplicateCapturedSource { access_index });
            }
            *slot = Some(IndexedCapturedGuestRead {
                access: expected,
                bytes: binding.bytes,
            });
        }

        for (index, access) in accesses.iter().enumerate() {
            if access.purpose() == AccessPurpose::TmemLoadSource && self.by_access[index].is_none()
            {
                return Err(WgpuRawDpcExecutionError::MissingCapturedSourceAccess {
                    access_index: u32::try_from(index)
                        .expect("packet resource-access count exceeds u32"),
                });
            }
        }
        Ok(())
    }

    fn bytes(&self, access_index: u32, expected: ResourceAccess) -> Option<&[u8]> {
        let indexed = self
            .by_access
            .get(usize::try_from(access_index).ok()?)?
            .as_ref()?;
        (indexed.access == expected).then(|| indexed.bytes.as_slice())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TaskGuestReadPayloadKey {
    range: PhysicalRange,
    content: FastContentDigest,
}

/// Task-local ownership of immutable guest-read payloads.
///
/// Each packet keeps its own access-index binding in `ExecutionCollector`;
/// only byte storage for an identical physical range and content identity is
/// shared. The byte comparison is load-bearing: the fast digest selects a
/// small candidate bucket but can never authorize reuse by itself.
#[derive(Default)]
struct TaskGuestReadCapturePool {
    payloads: HashMap<TaskGuestReadPayloadKey, Vec<Arc<[u8]>>>,
}

impl TaskGuestReadCapturePool {
    fn intern(&mut self, captured: &CapturedGuestRead) -> CapturedGuestReadBytes {
        self.intern_parts(
            captured.read().range(),
            captured.fast_content(),
            captured.bytes(),
        )
    }

    fn intern_parts(
        &mut self,
        range: PhysicalRange,
        content: FastContentDigest,
        bytes: &[u8],
    ) -> CapturedGuestReadBytes {
        let candidates = self
            .payloads
            .entry(TaskGuestReadPayloadKey { range, content })
            .or_default();
        if let Some(existing) = candidates
            .iter()
            .find(|existing| existing.as_ref() == bytes)
        {
            return CapturedGuestReadBytes(Arc::clone(existing));
        }
        let owned: Arc<[u8]> = Arc::from(bytes);
        candidates.push(Arc::clone(&owned));
        CapturedGuestReadBytes(owned)
    }
}

struct ExecutionCollector<'coord> {
    physical: &'coord PhysicalTmemState,
    queue: fn64_render_ir::QueueIdentity,
    ordinal: u64,
    submission: fn64_render_ir::SubmissionIdentity,
    plan: PlanCollector,
    reads: CapturedGuestReadAuthority,
    task_guest_read_pool: Option<&'coord mut TaskGuestReadCapturePool>,
    outcome: Option<Result<StagedOutcome, WgpuRawDpcExecutionError>>,
    /// The lazily-built color-target registry, borrowed for the duration of
    /// this execution so `stage_and_report` can `begin_candidate` against it
    /// and read a resident's prior device bytes. Only *read* here: the
    /// registry is never mutated during execution, which is exactly what
    /// makes the publication deferral honest (see
    /// [`PendingFillPublication`]).
    color_targets: &'coord mut Option<ColorTargetRegistry>,
    /// The host-configured framebuffer extent, this backend's only
    /// color-image height source. `None` before any
    /// `RenderBackend::create`, which rejects an admitted fill loudly with
    /// `NoColorTargetHeight` rather than inventing a height.
    configured_target_extent: Option<TriangleTargetExtent>,
    /// **The TMEM image this packet's triangles must sample, projected from
    /// the packet's own pending post-image.**
    ///
    /// Carried out of `stage_and_report` for the same structural reason
    /// `outcome` is: the `PendingTmemTransaction` is move-only, sealed
    /// inside `submitted_packet`, and consumed by
    /// `into_physical_successor` before `execute_raw_dpc_inner` returns --
    /// so by the time `draw_admitted_triangles` runs, the post-image is
    /// gone. A `TmemGpuProjection` is a plain owned byte image
    /// (`[u8; 4096]` plus its validity bitmap), so projecting it while the
    /// transaction is still borrowed and carrying the *bytes* out is the
    /// only ordering that works. It is deliberately **not** the borrow
    /// itself: keeping `PendingTmemImage<'_>` alive here would block the
    /// publication that must follow.
    ///
    /// `None` for a packet that staged no TMEM transaction at all --
    /// including a load-free texrect packet, which is a real WM2000 shape
    /// (measured: its fourth packet is 46 texrects, 0 loads, 0 fills).
    /// `draw_admitted_triangles` then projects durable committed TMEM,
    /// which is not a silent fallback: the packet proposed nothing, so
    /// durable state is the only image in existence, and it is the same one
    /// `TexrectTmemSource::Committed` hands the CPU texel reader for that
    /// packet.
    draw_tmem: Option<Vec<TmemGpuProjection>>,
    /// Whether this run materializes diagnostic GPU triangle projections.
    /// The measurement control can request them without submitting a draw;
    /// the guest-visible CPU raster path never consumes them.
    project_gpu_tmem: bool,
    /// Whether to collect one strict hottest-state CPU/compute differential
    /// from this packet's already-resolved production schedule.
    collect_compute_probe: bool,
    /// Filled only when the complete color-command schedule is one closed
    /// probe batch. Any boundary or unadmitted state leaves it absent.
    compute_probes: Vec<ComputeRasterProbe>,
    compute_replacement_enabled: bool,
    /// Mutable device executor borrowed from the backend only when the
    /// replacement A/B is enabled. The color stage consumes it before
    /// effect validation so GPU bytes, not a post-completion probe, become
    /// the transaction's typed completion.
    compute_replacement_pipeline: Option<&'coord mut TrianglePipelineRenderer>,
    compute_replacement_receipt: Option<ComputeRasterProbeReceipt>,
    /// Present only while the task executor is assembling an ordered device
    /// transaction. It reserves generation successors without publishing
    /// placeholder color bytes.
    color_execution_batch: Option<&'coord mut ColorTargetExecutionBatch>,
    ordered_cpu_color_batch: Option<&'coord mut OrderedCpuColorBatch>,
    task_cpu_phase_census: Option<&'coord mut task_cpu_phase_census::Task>,
    /// Selects planning-only compute admission. The resulting owned plan is
    /// retained below and completed after the task's single GPU submission.
    defer_compute_replacement: bool,
    deferred_compute: Option<DeferredComputeColor>,
}

/// What `stage_and_report` found for one sealed plan, structurally
/// distinguishing "this plan has TMEM loads to stage" from "this plan has
/// no TMEM loads and no physical successor to offer" (`WgpuBackend`
/// production triangle-draw integration card §1c) -- the caller
/// (`execute_raw_dpc_inner`) uses this to choose which
/// `RawDpcCoordinator` completion method is even reachable, never by
/// re-deriving "is this plan write-bearing" itself. Mechanical, not a
/// judgment call: a plan reaches `NoPhysicalSuccessor` only when
/// `collector.plan.loads` is empty and the plan still carries at least one
/// command with no declared write -- checked once, here, at the one place
/// those facts are already gathered -- never inferred from the *presence*
/// of triangles alone, since a plan could in principle carry both (mixed
/// plans stay on the `TmemLoads` path unconditionally, per the
/// `is_empty()` check).
enum StagedOutcome {
    TmemLoads(BackendEffectReport, PhysicalTmemState),
    /// This plan completed zero TMEM loads, declared zero guest-visible
    /// writes, and therefore has no `PhysicalTmemState` successor to
    /// install -- the `complete_execution_preserving_physical` route.
    ///
    /// **Two producers, one shape.** A triangle-only plan (§1c) reaches it
    /// because a raw triangle rasters into a GPU attachment and declares no
    /// journal write. A **sync-only** plan reaches it for the stricter
    /// reason that `SYNC_FULL` declares no `ResourceAccess` at all
    /// (`RdpFullSyncSite`'s own doc: "a sync reads and writes no
    /// resource") -- its whole effect is on the RDP pipeline and the DP
    /// interrupt line, both scheduled by the device fabric, never by this
    /// backend. Both are genuinely completable packets with nothing for
    /// this backend to raster, which is a different statement from having
    /// nothing to do.
    ///
    /// The variant makes no zero-write *claim*: the destination proves it.
    /// `complete_execution_preserving_physical` builds its own explicitly
    /// empty write list and lets `BackendEffectReport::try_new` check it
    /// against the packet's real journal, so a packet routed here that
    /// secretly declared a write is still rejected with
    /// `EffectCountMismatch`.
    NoPhysicalSuccessor,
    /// This plan staged at least one guest-visible color-target write and
    /// completed zero TMEM loads. Structurally distinct from both siblings:
    /// unlike `NoPhysicalSuccessor` it carries a nonempty `BackendEffectReport` (so
    /// `complete_execution_preserving_physical`, which builds its own empty
    /// one, is not a legal destination), and unlike `TmemLoads` it offers no
    /// `PhysicalTmemState` successor -- a color-target write does not touch
    /// physical TMEM at all.
    ///
    /// Carries the staged fill token out of `stage_and_report` so
    /// `execute_raw_dpc_inner` can hand it to the backend only after the
    /// coordinator accepted the completion.
    GuestWritesOnly(BackendEffectReport, StagedFill),
    /// This plan staged BOTH at least one guest-visible color-target write
    /// and at least one completed TMEM load, in one packet. Carries what
    /// each sibling carries and nothing new: the merged
    /// [`BackendEffectReport`] (both sources' writes, in the journal's own
    /// order -- see `merged_fill_and_tmem_writes`), the
    /// `PhysicalTmemState` successor the TMEM half produced, and the fill
    /// half's staged publication token.
    ///
    /// Routes to `complete_execution`, exactly as [`Self::TmemLoads`] does
    /// and for the same reason: it is the only completion that takes a
    /// physical successor, and the TMEM half genuinely has one. The
    /// fill-only sibling's `complete_execution_preserving_physical_with_effects`
    /// is not a legal destination here -- it never writes a successor slot,
    /// so the TMEM postimage this packet loaded would be silently discarded
    /// while its writes were still reported as completed.
    MixedFillAndTmemLoads(BackendEffectReport, PhysicalTmemState, StagedFill),
    DeferredGuestWritesOnly(DeferredBackendEffectReport, DeferredComputeColor),
    DeferredMixedColorAndTmem {
        effects: DeferredBackendEffectReport,
        tmem: DeferredPhysicalTmemSuccessor,
        color: DeferredComputeColor,
    },
}

struct DeferredComputeColor {
    candidate: CandidateColorTarget,
    plan: ComputeRasterReplacementPlan,
    program_attribution: ComputeProgramAttribution,
    initial_bytes: Option<Vec<u8>>,
}

/// One fill's execution result, staged inside `stage_and_report` and moved
/// out through [`StagedOutcome::GuestWritesOnly`]. Becomes a
/// [`PendingFillPublication`] once `execute_raw_dpc_inner` knows which
/// submission it belongs to and the coordinator has accepted the completion.
struct StagedFill {
    initialized: InitializedCandidateColorTarget,
    guest_writes: Vec<CompletedWrite>,
    prepared_sparse_checkpoint: Option<SparseInitializedColorCheckpoint>,
    cpu_phase_attributed: bool,
}

impl RawDpcExecutionView<PlanCollector> for ExecutionCollector<'_> {
    fn plan_visited(&mut self, plan_visitor: &mut PlanCollector) {
        // `PlanCollector` has no `Default` (its real construction always
        // takes an explicit durable-state seed, `Self::seeded`) -- the
        // placeholder left behind here is discarded immediately by the
        // caller (`execute_raw_dpc_inner`'s `let _ = plan_visitor;`), so
        // its exact seed values are irrelevant.
        self.plan = core::mem::replace(
            plan_visitor,
            PlanCollector::seeded(RawDpcCarryIn::default()),
        );
    }

    fn captured_reads(&mut self, reads: &[CapturedGuestRead]) {
        self.reads.clear_and_reserve(reads.len());
        for captured in reads {
            let bytes = match self.task_guest_read_pool.as_deref_mut() {
                Some(pool) => pool.intern(captured),
                None => CapturedGuestReadBytes::copied(captured.bytes()),
            };
            self.reads.push(captured.read(), bytes);
        }
    }

    fn submitted_packet(&mut self, packet: &WorkloadPacket) {
        let cpu_phase_attributed =
            self.task_cpu_phase_census.is_some() && ordered_depth_free_acff_triangle_member(self);
        let binding_started = task_cpu_phase_census::started(
            self.task_cpu_phase_census.as_deref(),
            cpu_phase_attributed,
        );
        let binding = task_cpu_phase_census::timed_source(
            self.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::SourceSubphase::PacketCapturedReadBind,
            || self.reads.bind_packet(packet),
        );
        task_cpu_phase_census::record_started(
            self.task_cpu_phase_census.as_deref_mut(),
            task_cpu_phase_census::Phase::SourceBindingLoad,
            binding_started,
        );
        if let Err(error) = binding {
            self.outcome = Some(Err(error));
            return;
        }
        self.outcome = Some(raw_dpc_execute_census::timed(
            raw_dpc_execute_census::Phase::Stage,
            || stage_and_report(self, packet),
        ));
    }
}

/// Named, loud rejection for one raw-DPC execution attempt. Every variant
/// either names a v11-scope boundary this backend does not admit or wraps
/// an inner typed rejection (TMEM physical staging, or T0/IR's own
/// `ValidationError`). Never a silent partial execution.
#[derive(Debug)]
pub enum WgpuRawDpcExecutionError {
    /// A destination access run for one load did not resolve to a
    /// contiguous, correctly-purposed TMEM-write slice at its declared
    /// `destination_access_index` -- the plan's own access list disagrees
    /// with what this executor expects from T1's push order.
    MalformedDestinationAccessRun {
        command_index: u32,
    },
    /// A load's source bytes were not found among the finalized captured
    /// guest reads at its declared `source_access_index` -- the plan's own
    /// access list disagrees with what the ABI-side capture supplied.
    MissingCapturedSource {
        command_index: u32,
    },
    CapturedSourceAccessOutOfRange {
        access_index: u32,
    },
    DuplicateCapturedSource {
        access_index: u32,
    },
    CapturedSourceAccessMismatch {
        access_index: u32,
    },
    MissingCapturedSourceAccess {
        access_index: u32,
    },
    /// A plan with zero TMEM loads AND zero admitted triangles reached
    /// execution -- there is nothing for this backend to do with it.
    NoCompletedLoads,
    Physical(PhysicalTmemError),
    Effect(ValidationError),
    Coordinator(ValidationError),
    /// A triangle's own command-time `OtherMode`/`CombineParams` snapshot
    /// was never established (`PlanCollector`'s own `MissingTriangleDrawState`)
    /// -- never silently skipped.
    MissingTriangleDrawState(MissingTriangleDrawState),
    /// The real GPU triangle-draw pipeline rejected a draw (device
    /// poisoned, submission/readback failure) -- never silently skipped.
    TriangleDraw(TrianglePipelineError),
    /// The explicitly-enabled game-derived probe produced a different byte
    /// from the existing CPU executor's complete target.
    ComputeRasterProbeMismatch {
        ordinal: u64,
        first_program: [u32; 4],
        last_program: [u32; 4],
        first_command_index: u32,
        last_command_index: u32,
        first_triangle_index: usize,
        last_triangle_index: usize,
        x: u32,
        y: u32,
        expected: u16,
        actual: u16,
    },
    /// The explicitly-enabled game-derived probe returned a target of the
    /// wrong length, so no byte-for-byte comparison is possible.
    ComputeRasterProbeLength {
        expected: usize,
        actual: usize,
    },
    /// The compute chain returned a different number of packet images than
    /// the ordered checkpoint request. Pairing with `Iterator::zip` would
    /// silently discard the unmatched authority on either side.
    ComputeRasterCheckpointCount {
        expected: usize,
        actual: usize,
    },
    /// An ordered chain crossed a target generation or extent boundary.
    /// Such a boundary requires publication/seed recovery, not an on-device
    /// target copy, so the diagnostic refuses instead of composing targets.
    ComputeRasterProbeChainIncompatible {
        ordinal: u64,
        batch: usize,
    },
    /// A task checkpoint probe encountered a packet with no complete typed
    /// compute representation. Silently skipping it could hide target work.
    ComputeRasterCheckpointPacketIneligible {
        packet: usize,
    },
    /// Consecutive retained probes did not form one exact target history.
    ComputeRasterCheckpointDiscontinuity {
        previous_ordinal: u64,
        ordinal: u64,
    },
    AcffRowBinEmptySegment,
    AcffRowBinEmptyMember {
        member: usize,
        ordinal: u64,
    },
    AcffRowBinDiscontinuous {
        member: usize,
        ordinal: u64,
    },
    AcffRowBinMissingPrefix {
        member: usize,
        ordinal: u64,
        position: u32,
    },
    AcffRowBinRaster {
        member: usize,
        ordinal: u64,
        draw: usize,
        source: crate::targets::TexrectExecutionError,
    },
    AcffRowBinInvalidWorkers {
        workers: usize,
    },
    AcffRowBinInitialStateLength {
        expected_bytes: usize,
        actual_bytes: usize,
        expected_coverage: usize,
        actual_coverage: usize,
    },
    AcffRowBinProgramMismatch {
        member: usize,
        ordinal: u64,
    },
    AcffRowBinCheckpointAccessMismatch {
        member: usize,
        ordinal: u64,
    },
    /// Exact execution-time admission proved that a coarse planning
    /// candidate has no complete typed compute representation. This is a
    /// normal, explicitly typed CPU disposition; only the task-segment
    /// dispatcher consumes it. Every other caller treats it as an error.
    TaskBatchComputeNotAdmitted {
        ordinal: u64,
        reason: TaskComputeAdmissionRefusal,
    },
    /// A triangle-bearing plan reached execution with no successful prior
    /// `RenderBackend::create` call -- `triangle_pipeline`/
    /// `triangle_target_extent` are always `Some` together (§1a/§1e) or
    /// both `None`; this is the caller-contract violation of the latter.
    TriangleDrawBeforeCreate,
    /// At least one fragment of an admitted triangle's draw reported a
    /// non-`TMEM_SAMPLE_STATUS_OK` status through `tmem_sample.wgsl`'s
    /// observable shader-failure-status channel (published committed-TMEM
    /// textured-draw card, audit repair) -- a missing tile binding, a
    /// reversed clamp extent, an invalid (never-loaded) TMEM byte in the
    /// triangle's UV footprint, or an unsupported production format.
    /// `status` is the first such code found, in row-major fragment order;
    /// never silently accepted as a successful textured draw.
    /// `tile_format`/`tile_pixel_size`/`tile_lut_mode` are the wire codes
    /// this triangle's own `TileBindingParams` uniform carried into the
    /// shader -- measured at the abort, not inferred from the CPU-side
    /// tile. A status-4 abort naming a format is actionable; one naming
    /// only `4` sent a prior lane looking at the wrong tile.
    TmemSampleFailed {
        status: u32,
        triangle_index: usize,
        tile_format: u32,
        tile_pixel_size: u32,
        tile_lut_mode: u32,
    },
    /// A triangle's resolved blend cycle selected
    /// [`crate::blend::BlendBInput::FramebufferAlpha`] on an active cycle
    /// (`ResolvedBlendCycle::requires_framebuffer_alpha`) -- the
    /// coverage-count half of the framebuffer-memory dependency, which this
    /// crate still does not implement (no coverage-count GPU write exists
    /// anywhere in this crate; see `crates/fn64-render-wgpu/README.md`'s
    /// blend-wiring sections). A cycle selecting only
    /// [`crate::blend::BlendColorInput::Framebuffer`] on `P`/`M` is no
    /// longer rejected here -- that destination-*color* subset is admitted
    /// and rendered via the framebuffer-color snapshot path. Rejected before
    /// GPU submission, never silently rendered opaque and never given a
    /// manufactured coverage count.
    BlendRequiresFramebuffer {
        triangle_index: usize,
    },
    /// An admitted `FillRectangle` reached execution with no prior
    /// `RenderBackend::create` call, so this backend has no color-image
    /// height at all. The RDP's `SetColorImage` carries no height field, and
    /// inventing one would fabricate the target's identity and byte range --
    /// rejected loudly instead.
    NoColorTargetHeight,
    /// The registry, candidate, or executor rejected an admitted fill.
    Target(TargetError),
    /// The fill executor itself rejected the rectangle -- unsupported cycle,
    /// missing combined state, an unsafe pixel-pipeline mode, a fractional
    /// edge, or missing resident/seed bytes.
    FillExecution(FillExecutionError),
    /// A recorded fill access span could not be bound back to the plan's own
    /// ordered access list.
    FillAccessSpan(crate::raw_dpc::FillAccessSpanError),
    /// One of an admitted fill's declared accesses was not an RDRAM region,
    /// so no byte range could be sliced for it. A fill access is always an
    /// RDRAM `ColorFramebuffer` region by construction; this is the loud
    /// rejection if the plan and the executor ever disagree.
    FillAccessRegionKind {
        access_index: u32,
    },
    /// One of an admitted fill's declared accesses named a byte range that
    /// is not a subrange of its own color target's full extent, so no
    /// device-byte slice corresponds to it.
    FillAccessOutsideTarget {
        access_index: u32,
    },
    /// A write access this packet's own resource journal declares was not
    /// claimed by any staged write when `merged_fill_and_tmem_writes` built
    /// the composed effect list -- the journal declared a write neither the
    /// fill half nor the TMEM half produced. Rejected by name here rather
    /// than handed to `BackendEffectReport::try_new` as a short list, whose
    /// count mismatch would not say *which* access went unproduced.
    /// A fill declared a colour-image seed read, but no captured bytes
    /// arrived for that access index. Never downgraded to "no seed": the
    /// untouched pixels would then be fabricated zeros, which is the exact
    /// defect the seed exists to remove.
    MissingFillSeedBytes {
        access_index: u32,
    },
    MergedWriteUnclaimed {
        access_index: u32,
    },
    /// A staged write claimed no declared write access -- this backend
    /// produced an effect the packet's own journal never declared. The
    /// inverse of [`Self::MergedWriteUnclaimed`], and equally a defect:
    /// admitting it would publish a write outside the journal's authority.
    MergedWriteUndeclared {
        access_index: u32,
    },
    /// A `TextureRectangle` reached the executor but its own
    /// `SetTile`/`SetTileSize` were never staged at its stream position, so
    /// there is no tile descriptor to sample through. Never defaulted to a
    /// zeroed tile, which would silently sample TMEM word zero and produce
    /// plausible pixels with no proven texel fetch.
    TexrectUnboundTile {
        triangle_index: usize,
    },
    /// A `TextureRectangle` reached the executor without a
    /// `RectViewportPixels`. Only a `TriangleSource::TextureRectangle`
    /// triangle carries one, and only that source reaches this path, so
    /// its absence means the plan and the decoder disagree.
    TexrectMissingViewport {
        triangle_index: usize,
    },
    /// This packet composes a `TextureRectangle` with TMEM loads, but the
    /// texrect declared no journal write access (no staged `SetColorImage`,
    /// an unsupported color format, a fractional or reversed rectangle, or
    /// flip -- see `raw_dpc::mod`'s `plan_texture_rectangle`). Executing it
    /// anyway would write bytes the journal never declared, which
    /// `merged_fill_and_tmem_writes` would then reject less specifically as
    /// `MergedWriteUndeclared`. Named here instead.
    TexrectDeclaredNoWrite {
        triangle_index: usize,
    },
    /// A raw triangle the schedule reached carries a recorded access span
    /// that no longer resolves to a non-empty slice of the plan's own
    /// access list.
    ///
    /// Unreachable by construction -- `PlanCollector` only records a raw
    /// triangle at all when its span is `Some`, and the span was written by
    /// `plan_raw_triangle` at the moment it pushed those very accesses --
    /// which is exactly why it is a named error rather than an `expect`:
    /// if the invariant ever breaks, the failure must name the triangle,
    /// not abort the process.
    RawTriangleDeclaredNoWrite {
        triangle_index: usize,
    },
    /// A raw triangle's own retained wire words did not re-decode as a
    /// base-edge (0x08) triangle.
    ///
    /// Also unreachable by construction: `plan_raw_triangle` only declares
    /// a write for a flat triangle, whose wire form is exactly the 32 bytes
    /// `RawTriangle::decode` already accepted once during decode. Named
    /// rather than `expect`ed for the same reason as the variant above.
    RawTriangleWireWordsUndecodable {
        triangle_index: usize,
    },
    /// A `TextureRectangle` was admitted in a packet with no TMEM load, so
    /// there is no pending post-image for it to sample. Census-measured,
    /// this shape does not occur in WM2000 (0 of 219 decode entries carry a
    /// texrect without a load in the same entry); it is refused by name
    /// rather than silently sampling stale committed state.
    /// A pending TMEM post-image reported a `Committed` snapshot identity.
    /// A proposal has no durable `(state, generation)` pair to name, so a
    /// committed receipt for one is a forgery; see the check site for why
    /// this is verified rather than trusted to the type system.
    PendingTmemImageClaimedCommitted {
        triangle_index: usize,
    },
    /// The same forgery check as [`Self::PendingTmemImageClaimedCommitted`],
    /// at the *other* place a pending post-image is consumed: the GPU-side
    /// projections `draw_admitted_triangles` samples. Separate variant, not
    /// a shared one, because the two sites answer for different things: the
    /// CPU variant names the texrect whose *pixels* would carry a forged
    /// receipt, while this one names a projection built before any fixture
    /// exists. `project_pending_tmem_per_triangle` does now walk the
    /// triangles in order, so an index could be supplied -- but it would be
    /// an index into `plan.triangle_commands`, not the `triangle_index` the
    /// CPU variant reports, and offering two different numbers under one
    /// field name is worse than offering none. The forgery is a property of
    /// the *source* here, identical for every entry, so the first entry to
    /// reach it is not more culpable than the rest.
    PendingTmemProjectionClaimedCommitted,
    /// The per-triangle TMEM projection list handed to
    /// `draw_admitted_triangles` is not the same length as the triangle
    /// list it must cover. Both come from walks over the same plan
    /// (`plan.triangle_commands` and `plan.triangles`, pushed at one site),
    /// so a disagreement is a structural break -- and the only images
    /// available to fill a gap are another triangle's or the whole-packet
    /// post-image, which is exactly what per-position selection exists to
    /// withhold. Refused rather than padded.
    TmemProjectionCountMismatch {
        projections: usize,
        triangles: usize,
    },
    /// The mirror of [`Self::PendingTmemImageClaimedCommitted`]: durable
    /// `PhysicalTmemState`, selected for a packet that staged no TMEM load,
    /// reported a `Proposed` snapshot identity. Durable state has no
    /// proposal digest to name, so a proposed receipt from it is a forgery
    /// in the other direction, and a texrect would be attributing its
    /// pixels to a transaction that does not exist.
    CommittedTmemImageClaimedProposed {
        triangle_index: usize,
    },
    /// A packet staged at least one color-target command (a
    /// `FillRectangle` or a `TextureRectangle`) but no `SetColorImage` was
    /// current at its stream position -- neither carried in this packet nor
    /// durable from an earlier one. There is no destination image to derive
    /// a [`ColorTargetKey`] from, and one is never invented.
    ///
    /// Distinct from [`Self::NoColorTargetHeight`], which is the *height*
    /// half of the same key: that names a missing `RenderBackend::create`,
    /// this names a missing RDP register.
    NoStagedColorImage,
    /// An admitted `FillRectangle`'s own `color_image` field disagrees with
    /// the `SetColorImage` register current at its stream position. The
    /// decoder derives write accesses from the register and the executor
    /// composes into the key derived from it, so a disagreeing fill would
    /// write pixels into one image while declaring them against another.
    /// Refused by name rather than silently preferring either source.
    FillColorImageDisagreesWithRegister {
        command_index: u32,
    },
    TexrectExecution(crate::targets::TexrectExecutionError),
}

impl core::fmt::Display for WgpuRawDpcExecutionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MalformedDestinationAccessRun { command_index } => write!(
                formatter,
                "raw-DPC command #{command_index}'s destination access run is malformed"
            ),
            Self::MissingCapturedSource { command_index } => write!(
                formatter,
                "raw-DPC command #{command_index}'s source bytes are missing from the captured \
                 guest reads"
            ),
            Self::CapturedSourceAccessOutOfRange { access_index } => write!(
                formatter,
                "captured guest-read access index {access_index} is outside the submitted packet"
            ),
            Self::DuplicateCapturedSource { access_index } => write!(
                formatter,
                "captured guest-read access index {access_index} is bound more than once"
            ),
            Self::CapturedSourceAccessMismatch { access_index } => write!(
                formatter,
                "captured guest-read access index {access_index} does not match the submitted \
                 packet's exact TMEM-source access"
            ),
            Self::MissingCapturedSourceAccess { access_index } => write!(
                formatter,
                "submitted packet TMEM-source access index {access_index} has no captured binding"
            ),
            Self::NoCompletedLoads => {
                formatter.write_str("raw-DPC plan reached execution with zero TMEM loads")
            }
            Self::Physical(error) => write!(formatter, "physical TMEM staging failed: {error}"),
            Self::Effect(error) => write!(formatter, "backend effect report failed: {error}"),
            Self::Coordinator(error) => write!(formatter, "coordinator execution failed: {error}"),
            Self::MissingTriangleDrawState(error) => {
                write!(formatter, "triangle draw state missing: {error}")
            }
            Self::TriangleDraw(error) => write!(formatter, "triangle draw failed: {error}"),
            Self::ComputeRasterProbeMismatch {
                ordinal,
                first_program,
                last_program,
                first_command_index,
                last_command_index,
                first_triangle_index,
                last_triangle_index,
                x,
                y,
                expected,
                actual,
            } => write!(
                formatter,
                "compute-raster probe disagreed with the CPU target in packet ordinal {ordinal}, \
                 program range {first_program:08x?}..={last_program:08x?}, command range \
                 #{first_command_index}..=#{last_command_index}, triangle range \
                 #{first_triangle_index}..=#{last_triangle_index}, pixel ({x}, {y}): expected RGBA16 \
                 {expected:#06x}, got {actual:#06x}"
            ),
            Self::ComputeRasterProbeLength { expected, actual } => write!(
                formatter,
                "compute-raster probe returned {actual} target bytes; CPU produced {expected}"
            ),
            Self::ComputeRasterCheckpointCount { expected, actual } => write!(
                formatter,
                "compute-raster chain returned {actual} checkpoint images; requested {expected}"
            ),
            Self::ComputeRasterProbeChainIncompatible { ordinal, batch } => write!(
                formatter,
                "compute-raster probe batch {batch} in packet ordinal {ordinal} crosses an \
                 on-device target generation or extent boundary"
            ),
            Self::ComputeRasterCheckpointPacketIneligible { packet } => write!(
                formatter,
                "compute-raster checkpoint packet {packet} has no complete typed compute batch"
            ),
            Self::ComputeRasterCheckpointDiscontinuity {
                previous_ordinal,
                ordinal,
            } => write!(
                formatter,
                "compute-raster checkpoint target history is discontinuous between packet \
                ordinals {previous_ordinal} and {ordinal}"
            ),
            Self::AcffRowBinEmptySegment => {
                formatter.write_str("ACFF row-bin execution requires a non-empty task segment")
            }
            Self::AcffRowBinEmptyMember { member, ordinal } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) has no admitted draws"
            ),
            Self::AcffRowBinDiscontinuous { member, ordinal } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) is not the exact target-generation successor of its predecessor"
            ),
            Self::AcffRowBinMissingPrefix {
                member,
                ordinal,
                position,
            } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) draw at command {position} has no sealed earlier TMEM prefix"
            ),
            Self::AcffRowBinRaster {
                member,
                ordinal,
                draw,
                source,
            } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) draw {draw} failed: {source}"
            ),
            Self::AcffRowBinInvalidWorkers { workers } => write!(
                formatter,
                "ACFF row-bin worker count {workers} is invalid; expected 2, 4, 6, or 8"
            ),
            Self::AcffRowBinInitialStateLength {
                expected_bytes,
                actual_bytes,
                expected_coverage,
                actual_coverage,
            } => write!(
                formatter,
                "ACFF row-bin initial state has {actual_bytes} visible bytes/{actual_coverage} coverage cells; expected {expected_bytes}/{expected_coverage}"
            ),
            Self::AcffRowBinProgramMismatch { member, ordinal } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) does not match the exact RGBA16 shaded+textured fc15fea3/f00ff23f + 0018acff/0f0a7008 admission"
            ),
            Self::AcffRowBinCheckpointAccessMismatch { member, ordinal } => write!(
                formatter,
                "ACFF row-bin task member {member} (ordinal {ordinal}) command writes do not equal its checkpoint journal writes in exact order"
            ),
            Self::TaskBatchComputeNotAdmitted { ordinal, reason } => write!(
                formatter,
                "raw-DPC task member ordinal {ordinal} was explicitly not admitted by the typed compute executor: {reason:?}"
            ),
            Self::TriangleDrawBeforeCreate => formatter.write_str(
                "a triangle-bearing plan reached execution with no successful prior \
                 RenderBackend::create call",
            ),
            Self::TmemSampleFailed {
                status,
                triangle_index,
                tile_format,
                tile_pixel_size,
                tile_lut_mode,
            } => write!(
                formatter,
                "a triangle draw's fragment shader reported a non-OK tmem_sample.wgsl status: \
                 {status} (triangle #{triangle_index} in plan order, tile format code \
                 {tile_format}, pixel-size code {tile_pixel_size}, TLUT-mode code \
                 {tile_lut_mode})"
            ),
            Self::BlendRequiresFramebuffer { triangle_index } => write!(
                formatter,
                "triangle #{triangle_index} (plan order) selected a blend-cycle input that reads \
                 the framebuffer alpha (coverage count); this crate does not yet implement \
                 framebuffer-alpha-dependent blending"
            ),
            Self::NoStagedColorImage => formatter.write_str(
                "a color-target command reached execution with no SetColorImage current at its \
                 stream position, in this packet or durable from an earlier one, so there is no \
                 destination image to compose into",
            ),
            Self::FillColorImageDisagreesWithRegister { command_index } => write!(
                formatter,
                "FillRectangle command #{command_index} carries a color image that differs from \
                 the SetColorImage register current at its own stream position"
            ),
            Self::NoColorTargetHeight => formatter.write_str(
                "an admitted FillRectangle reached execution before any RenderBackend::create \
                 call, so this backend has no color-image height; the RDP's SetColorImage \
                 carries no height field and one is never invented",
            ),
            Self::Target(error) => write!(formatter, "color target rejected the fill: {error}"),
            Self::FillExecution(error) => {
                write!(formatter, "fill executor rejected the rectangle: {error}")
            }
            Self::FillAccessSpan(error) => {
                write!(formatter, "fill access span did not bind: {error}")
            }
            Self::FillAccessRegionKind { access_index } => write!(
                formatter,
                "FillRectangle access #{access_index} is not an RDRAM region, so no device-byte \
                 slice corresponds to it"
            ),
            Self::FillAccessOutsideTarget { access_index } => write!(
                formatter,
                "FillRectangle access #{access_index} names a range outside its own color \
                 target's full extent"
            ),
            Self::MissingFillSeedBytes { access_index } => write!(
                formatter,
                "fill declared a color-image seed read at access {access_index} but no captured \
                 bytes arrived for it; the untouched pixels would be fabricated zeros"
            ),
            Self::MergedWriteUnclaimed { access_index } => write!(
                formatter,
                "the packet's journal declares write access #{access_index}, but neither the \
                 fill nor the TMEM half of this composed packet staged a write claiming it"
            ),
            Self::MergedWriteUndeclared { access_index } => write!(
                formatter,
                "a staged write claiming access #{access_index} matches no write access this \
                 packet's own journal declares"
            ),
            Self::TexrectUnboundTile { triangle_index } => write!(
                formatter,
                "texture-rectangle triangle #{triangle_index} (plan order) has no SetTile/\
                 SetTileSize staged at its own stream position, so there is no tile descriptor \
                 to sample TMEM through"
            ),
            Self::TexrectMissingViewport { triangle_index } => write!(
                formatter,
                "texture-rectangle triangle #{triangle_index} (plan order) carries no \
                 RectViewportPixels; only the decoder produces one and every texrect-sourced \
                 triangle must have it"
            ),
            Self::TexrectDeclaredNoWrite { triangle_index } => write!(
                formatter,
                "texture-rectangle triangle #{triangle_index} (plan order) declared no journal \
                 write access, so executing it would write bytes this packet never declared"
            ),
            Self::RawTriangleDeclaredNoWrite { triangle_index } => write!(
                formatter,
                "raw triangle #{triangle_index} (plan order) was scheduled with a recorded \
                 access span that resolves to no accesses"
            ),
            Self::RawTriangleWireWordsUndecodable { triangle_index } => write!(
                formatter,
                "raw triangle #{triangle_index} (plan order) retained wire words that do not \
                 re-decode as a base-edge triangle"
            ),
            Self::PendingTmemImageClaimedCommitted { triangle_index } => write!(
                formatter,
                "the pending TMEM post-image sampled by texture-rectangle triangle \
                 #{triangle_index} reported a Committed snapshot identity; a sealed but \
                 unpublished transaction has no durable (state, generation) pair to name"
            ),
            Self::CommittedTmemImageClaimedProposed { triangle_index } => write!(
                formatter,
                "texture rectangle #{triangle_index} (plan order) sampled durable committed TMEM \
                 that reported a Proposed snapshot identity"
            ),
            Self::TmemProjectionCountMismatch {
                projections,
                triangles,
            } => write!(
                formatter,
                "this packet's triangle draw was handed {projections} per-triangle TMEM \
                 projections for {triangles} triangles; every triangle must sample the image at \
                 its own stream position and none may borrow another's"
            ),
            Self::PendingTmemProjectionClaimedCommitted => formatter.write_str(
                "the pending TMEM post-image projected for this packet's triangle draw reported \
                 a Committed snapshot identity; a sealed but unpublished transaction has no \
                 durable (state, generation) pair to name",
            ),
            Self::TexrectExecution(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<TargetError> for WgpuRawDpcExecutionError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

impl From<PhysicalTmemError> for WgpuRawDpcExecutionError {
    fn from(error: PhysicalTmemError) -> Self {
        Self::Physical(error)
    }
}

impl From<FillExecutionError> for WgpuRawDpcExecutionError {
    fn from(error: FillExecutionError) -> Self {
        Self::FillExecution(error)
    }
}

impl From<crate::targets::TexrectExecutionError> for WgpuRawDpcExecutionError {
    fn from(error: crate::targets::TexrectExecutionError) -> Self {
        Self::TexrectExecution(error)
    }
}

impl std::error::Error for WgpuRawDpcExecutionError {}

impl From<WgpuRawDpcExecutionError> for RenderError {
    fn from(error: WgpuRawDpcExecutionError) -> Self {
        RenderError::Backend {
            backend: "render-wgpu/raw-dpc-execute",
            reason: error.to_string(),
        }
    }
}

/// Slice `accesses[start..]` down to the exact contiguous run of
/// `AccessMode::Write`/`AccessPurpose::TmemLoadDestination` accesses whose
/// `ResourceAccess::operation()` ordinals are consecutive starting at
/// `start`'s own operation -- exactly the destination-fragment run T1's
/// `push_tmem_load` pushed for one load (`destination` first, then any
/// `extra_destination_accesses` via `push_command_decode_access`, per
/// `crate::raw_dpc::production_adapter`'s own doc comment). There is no
/// explicit destination-access *count* on `TmemLoadSemantics`; operation-id
/// contiguity is the same fact `fn64-render`'s own `access_identity`/
/// `validate_packet_slice` machinery already relies on for the
/// decoder-typed path.
fn destination_access_run(accesses: &[ResourceAccess], start: usize) -> &[ResourceAccess] {
    let Some(first) = accesses.get(start) else {
        return &[];
    };
    let first_operation = first.operation().get();
    let mut end = start;
    for (offset, access) in accesses[start..].iter().enumerate() {
        let expected_operation = match first_operation.checked_add(offset as u32) {
            Some(value) => value,
            None => break,
        };
        if access.operation().get() != expected_operation
            || access.mode() != AccessMode::Write
            || access.purpose() != AccessPurpose::TmemLoadDestination
        {
            break;
        }
        end = start + offset + 1;
    }
    &accesses[start..end]
}

/// One transfer word's exact captured source-byte slice, bound first to the
/// **access** the word names (`word.source_access_index()`, resolved
/// against the load's own `source_access_index()` base) and then by
/// `word.source_access_byte_offset()`/`defined_source_byte_mask()` within
/// that access's captured bytes -- mirrors `load_tile.rs`'s `bytes_for_word`
/// exactly, including its two-step binding.
///
/// The offset is relative to the word's own source access, never to the
/// concatenation of the run: `tmem::physical`'s word projection subtracts
/// the preceding accesses' byte total (`logical_offset - preceding`) before
/// storing it. Slicing a flattened run at that offset would silently read
/// the wrong row for every access after the first.
fn word_source_bytes<'a>(
    reads: &'a CapturedGuestReadAuthority,
    source_accesses: &[ResourceAccess],
    first_access_index: u32,
    word: TmemTransferWord,
) -> Option<&'a [u8]> {
    let relative = word.source_access_index().checked_sub(first_access_index)?;
    let expected = *source_accesses.get(usize::try_from(relative).ok()?)?;
    let access_bytes = reads.bytes(word.source_access_index(), expected)?;
    let defined = word.defined_source_byte_mask().count_ones() as usize;
    let start = word.source_access_byte_offset() as usize;
    let end = start.checked_add(defined)?;
    access_bytes.get(start..end)
}

fn map_physical_lanes(
    load: &TmemLoadSemantics,
    word: TmemTransferWord,
    bytes: &[u8],
) -> Result<[Option<u8>; 8], PhysicalTmemError> {
    match load.shape() {
        TmemLoadShape::Block => Ok(crate::tmem::map_physical_lanes_block(word, bytes)),
        // LoadTile shares LoadBlock's exact mapping; see
        // `tmem::execute::mod`'s re-export comment for why only one copy is
        // reused here.
        TmemLoadShape::Tile => Ok(crate::tmem::map_physical_lanes_block(word, bytes)),
        TmemLoadShape::Tlut => crate::tmem::map_physical_lanes_tlut(word, bytes)
            .map_err(|_| PhysicalTmemError::InvalidPhysicalFragment),
    }
}

impl RenderBackend for WgpuBackend {
    /// Requests a real GPU device and derives this backend's triangle
    /// render-target extent from `cfg`, both eagerly and synchronously
    /// (blocks on `UninitializedTrianglePipeline::request()`). A repeated
    /// call is a full reset: `triangle_pipeline`/`triangle_target_extent`
    /// are always replaced together, from a fresh device request, never
    /// partially. Thin wrapper over `Self::create_inner`, which returns
    /// the richly-typed `WgpuCreateError` this crate's own tests need to
    /// distinguish a genuine `NoAdapter` from any other failure --
    /// `RenderError::Backend`'s plain `String` reason cannot preserve
    /// that distinction once converted.
    fn create(&mut self, cfg: &RenderConfig) -> Result<(), RenderError> {
        self.create_inner(cfg).map_err(RenderError::from)
    }

    fn observe_non_rdp_write16(
        &mut self,
        _write: fn64_render::NonRdpWrite16,
    ) -> fn64_render::NonRdpWrite16Disposition {
        fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar
    }

    fn deferred_non_rdp_write16_disposition(
        &self,
    ) -> Option<fn64_render::NonRdpWrite16Disposition> {
        Some(fn64_render::NonRdpWrite16Disposition::NoRustHiddenSidecar)
    }

    /// This backend has no HLE display-list front end: it holds no geometry
    /// microcode catalog, no segment/matrix/vertex state, and no GBI command
    /// decoder. Its whole graphics surface is the raw-DPC seam
    /// (`process_rdp_commands` / `plan_raw_dpc` / `execute_raw_dpc` /
    /// `publish_raw_dpc`), which consumes RDP command words, not `Gfx` words.
    ///
    /// So the only truthful answer to "execute this graphics task" is the
    /// trait's existing one for a microcode this backend cannot high-level
    /// emulate: [`FrameStatus::NeedsLle`], carrying the live IMEM digest.
    /// `fn64-abi`'s `osSpTaskStartGo_recomp` then runs the task's microcode
    /// on the RSP interpreter (`dispatch_lle_task`), and the RDP commands
    /// that microcode writes into DMEM arrive back here through the XBUS
    /// raw-DPC path this backend does implement. That is the same disposition
    /// `ReferenceBackend` reaches for this title -- measured on WM2000
    /// (NWXE), whose every graphics task carries a well-formed F3DEX2 display
    /// list under IMEM digest
    /// `c50d2949c23baae24e706e8e1a5abf2dd315d00aff4cfdd567a03fe81807d1be`,
    /// which is in no catalog, so `ReferenceBackend::process_task` returns
    /// `NeedsLle` for all of them. It is a *disposition*, not a silent no-op:
    /// the task is executed, by the RSP, and `fn64-abi` records a
    /// `render.hle-ucode.needs-lle` unsupported event naming the digest.
    ///
    /// The digest is read from live IMEM, the same authority
    /// `GeometryUcodeCatalog::require_text` and `ReferenceBackend` use, so
    /// the reported microcode identity cannot disagree with theirs. A task
    /// that is not `M_GFXTASK` is still a routing bug rather than a
    /// microcode this backend declines, and stays a loud named error.
    fn process_task(
        &mut self,
        _rdram: &mut [u8],
        rsp_memory: &mut fn64_runtime::RspMemory,
        task: &fn64_render::OsTask,
        _output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, RenderError> {
        if task.task_type != fn64_render::M_GFXTASK {
            return Err(RenderError::Backend {
                backend: "render-wgpu",
                reason: format!(
                    "graphics task dispatch received task type {}; only M_GFXTASK ({}) reaches \
                     this seam",
                    task.task_type,
                    fn64_render::M_GFXTASK
                ),
            });
        }
        let ucode_sha256 =
            fn64_render::UcodeDigest::from_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                .as_bytes();
        Ok(fn64_render::FrameStatus::NeedsLle { ucode_sha256 })
    }

    fn enable_presented_post_vi_field_delivery(&mut self) -> Result<(), RenderError> {
        self.presented_field_delivery = PresentedFieldDelivery::PostVi;
        self.presented_source_field = None;
        self.presented_post_vi_field = None;
        self.presented_field = None;
        Ok(())
    }

    /// **Shape (a): a real scanout, not a refusal.** This replaces the named
    /// "presentation is out of scope" rejection that made a registered
    /// `WgpuBackend` fatal at the first VI retrace, because
    /// `present_render_backend` -> `with_render_backend` turns any backend
    /// error into a panic by design (`fn64-abi`'s `setup.rs`).
    ///
    /// A VI retrace is not rasterization. The trait's own doc says so:
    /// "`osViSwapBuffer` only selects which rendered buffer a later field
    /// consumes." What present owes is a *scanout* of the buffer the guest's
    /// latched VI registers point at -- and for this backend that buffer is
    /// in guest RDRAM, because an admitted `FillRectangle`'s bytes are copied
    /// back there by `fn64-abi`'s `copy_committed_guest_writes`. So present
    /// reads guest memory; it does not need a resident image and does not
    /// invent one.
    ///
    /// **Both `PresentMemory` variants are handled, and they are not
    /// symmetric.**
    ///
    /// - `Physical` -- implemented. `crate::vi_scanout` reads the programmed
    ///   source rectangle through the retrace-scoped `PhysicalRdramRead`
    ///   capability, which is the same lane-mapped authority (`^2`/`^3`) the
    ///   reference backend's `load_vi_source` reads through and
    ///   `RdramViewMut::write_u16` writes through. Every VI filter it does
    ///   not implement is refused by its own name (`ViScanoutRefusal`),
    ///   never silently skipped.
    /// - `BackendResidentCompatibility` -- **refused, specifically.** This
    ///   backend holds no resident scanout image to present. Its color
    ///   targets are published *to guest RDRAM* (`ColorTargetRegistry` plus
    ///   `copy_committed_guest_writes`), and `triangle_draw_output` is a
    ///   single draw's readback, replaced per submission and never a
    ///   framebuffer the VI would sample. Presenting it would claim the last
    ///   triangle draw was the field the guest programmed, which for a
    ///   double-buffered title is simply a different buffer. The error names
    ///   that, and names the request forms that do work.
    ///
    /// A failed present leaves the previously presented field in place; see
    /// `presented_field`'s own doc for why discarding it would fabricate a
    /// frame.
    fn present(&mut self, request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
        let (vi, memory) = request.into_parts();
        let memory = match memory {
            fn64_render::PresentMemory::Physical(memory) => memory,
            fn64_render::PresentMemory::BackendResidentCompatibility => {
                return Err(RenderError::Backend {
                    backend: "render-wgpu",
                    reason: "PresentMemory::BackendResidentCompatibility is not supported: \
                             WgpuBackend retains no resident scanout image. Its color targets \
                             are published into guest RDRAM and its triangle_draw_output is one \
                             submission's readback, not a VI-sampled framebuffer. Use \
                             PresentRequest::live or PresentRequest::physical_compatibility, \
                             which supply the physical memory this backend scans out."
                        .to_string(),
                })
            }
        };
        match self.presented_field_delivery {
            PresentedFieldDelivery::Source => {
                self.presented_source_field =
                    crate::vi_scanout::scan_out_rgba5551_source_field(vi, &memory)?;
                self.presented_field = None;
            }
            PresentedFieldDelivery::PostVi => {
                let field = crate::vi_scanout::scan_out_guest_rdram(vi, &memory)?;
                let post_vi = fn64_render::PresentedPostViField::rgba8888(
                    field.presentation,
                    field.width,
                    field.height,
                    field.rgba8,
                )?;
                self.presented_post_vi_field = Some(post_vi);
                self.presented_field = None;
            }
            PresentedFieldDelivery::ConcreteDiagnostic => {
                let field = crate::vi_scanout::scan_out_guest_rdram(vi, &memory)?;
                self.presented_field = Some(field);
            }
        }
        Ok(())
    }

    fn take_presented_source_field(&mut self) -> fn64_render::PresentedSourceFieldAvailability {
        self.presented_source_field.take().map_or(
            fn64_render::PresentedSourceFieldAvailability::Unsupported,
            fn64_render::PresentedSourceFieldAvailability::Ready,
        )
    }

    fn take_presented_post_vi_field(&mut self) -> fn64_render::PresentedPostViFieldAvailability {
        self.presented_post_vi_field.take().map_or(
            fn64_render::PresentedPostViFieldAvailability::Unsupported,
            fn64_render::PresentedPostViFieldAvailability::Ready,
        )
    }

    /// **Shape (a): a real reconfiguration, not a refusal.** This replaces a
    /// silent no-op stub, which AGENTS.md forbids outright.
    ///
    /// A resize here is exactly and only a change of *target geometry*.
    /// That is the whole of it, because this backend owns no device
    /// resource whose size is fixed at `create` time:
    /// `TrianglePipelineRenderer` holds an instance, a device, a queue,
    /// four extent-independent `RenderPipeline`s, a bind-group layout and
    /// one fixed-size dummy buffer -- no swapchain, no persistent color or
    /// depth attachment. Its color+depth textures are created fresh per
    /// submission from the extent handed to `submit_triangles`
    /// (`targets::triangle_pipeline`'s own "fresh per-submission color+depth
    /// texture pair ... not a persistent swapchain"). So there is nothing to
    /// reallocate and no device to re-request: recording the new extent *is*
    /// the recovery, and the next submission builds its attachments at the
    /// new size. Reusing `create_inner` would be actively wrong, not merely
    /// redundant -- it would blow away a live `triangle_pipeline` and
    /// re-request an adapter to reach the identical state.
    ///
    /// Both extents are written, and the pairing invariant
    /// `triangle_pipeline.is_some() == triangle_target_extent.is_some()`
    /// (§1a/§1e) is preserved by construction: `triangle_target_extent` is
    /// updated through `Option::as_mut`, so a resize before any successful
    /// `create` leaves it `None` rather than populating an extent with no
    /// pipeline behind it. `configured_target_extent` is written
    /// unconditionally for the same reason `create_inner` records it before
    /// the device request -- it is the CPU-side fill path's only color-image
    /// height, and an adapterless host must still be able to resize the
    /// target a `FillRectangle` executes into.
    ///
    /// **Zero dimensions are recorded, not clamped and not dropped.** The
    /// trait contract makes `resize` infallible and directs a backend that
    /// cannot honor one to surface it at the next call; both consumers of
    /// these extents already do exactly that, by name --
    /// `ColorTargetExtent::try_new` returns `TargetError::ZeroExtent` and
    /// `validate_triangle_extent` returns `TrianglePipelineError::ZeroExtent`.
    /// Clamping to 1 would fabricate a target geometry the host never asked
    /// for and silently publish a resident of the wrong byte range; ignoring
    /// the call would be the silent no-op this replaces. Recording it routes
    /// a minimized window to a named rejection at the point of use.
    ///
    /// **A pending fill token survives a resize, deliberately.** A staged
    /// `PendingFillPublication` carries an `InitializedCandidateColorTarget`
    /// whose `ColorTargetKey` -- address, extent and byte range -- was
    /// sealed inside the token when the fill executed, and
    /// `ColorTargetRegistry::prepare_publication` reads only that captured
    /// key; it never consults the backend's current
    /// `configured_target_extent`. The invariant is therefore already in the
    /// type system rather than in this method: a resize *cannot* retarget an
    /// executed fill, so there is nothing for this method to guard. Dropping
    /// the token instead would discard the guest-write report of a
    /// submission that completed correctly, and the next
    /// `staged_guest_render_target_writes` would return an empty list --
    /// failing that submission loudly with `EffectCountMismatch` for a
    /// window resize it had nothing to do with. A resize is a statement
    /// about the geometry of *future* targets, never a retroactive
    /// invalidation of a fill that already ran.
    ///
    /// Guest-write nonclaim (module doc): nothing here touches guest RDRAM.
    /// These are host-side extent fields; a resize writes no memory the
    /// guest can observe.
    fn resize(&mut self, w: u32, h: u32) {
        let extent = TriangleTargetExtent {
            width: w,
            height: h,
        };
        self.configured_target_extent = Some(extent);
        // **The decoder's height bound follows the resize too.**
        //
        // `create` sets both from the same `cfg` field and its comment there
        // says the two "cannot drift apart" -- but this method updated only
        // one of them, so every resize left the decoder bounding against the
        // height `create` configured. That was latent while nothing sized
        // anything from it; a partial fill's colour-image seed does, and the
        // drift produced a 256-byte seed for a 128-byte target
        // (`the_adapterless_fill_path_still_works_after_a_resize`, which is
        // how it was found).
        self.rdp_state.set_color_target_height(h);
        if let Some(triangle_target_extent) = self.triangle_target_extent.as_mut() {
            *triangle_target_extent = extent;
        }
    }

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }
}

impl RawDpcBackend for WgpuBackend {
    /// Returned unconditionally, never varying with whether a fill or a
    /// FullSync site has yet been admitted: a capability is a statement about
    /// what this backend *will* admit, not about what it has admitted, and a
    /// value that changes under the caller is worse than a wider constant
    /// one.
    ///
    /// Nonclaim: the `SiteOnly` half of this variant's name is load-bearing.
    /// This backend decodes a `SYNC_FULL` opcode and binds it to the
    /// capture's boundary; it does not observe a DP interrupt and does not
    /// claim the guest did. See `RawDpcIrCapability`'s own doc for why that
    /// distinction cannot be recovered from the capability value alone and
    /// lives in the supplied `FullSyncBoundary` instead.
    fn raw_dpc_ir_capability(&self) -> RawDpcIrCapability {
        RawDpcIrCapability::TransactionalTmemFillFullSyncSiteOnly
    }

    fn raw_dpc_task_batch_capability(&self) -> RawDpcTaskBatchCapability {
        RawDpcTaskBatchCapability::Transactional
    }

    fn plan_raw_dpc(
        &mut self,
        request: RawDpcPlanRequest,
    ) -> Result<PlannedRawDpcSubmission, RenderError> {
        let (planned, delta, _) = plan_raw_dpc_inner(&self.coordinator, &self.rdp_state, request)
            .map_err(|reason| RenderError::Backend {
            backend: "render-wgpu/raw-dpc-plan",
            reason,
        })?;
        // This single capture is intentionally adjacent to and before the
        // fold: splitting it by register recreates mixed-time carry-in.
        self.raw_dpc_carry_in_before_last_plan = Some(RawDpcCarryIn::capture(&self.rdp_state));
        self.rdp_state.apply(&delta);
        Ok(planned)
    }

    fn plan_raw_dpc_task_batch(
        &mut self,
        requests: Vec<RawDpcPlanRequest>,
    ) -> Result<Vec<PlannedRawDpcSubmission>, RenderError> {
        if requests.is_empty() {
            return Err(RenderError::Backend {
                backend: "render-wgpu/raw-dpc-task-batch-plan",
                reason: "a production raw-DPC task batch cannot be empty".to_string(),
            });
        }
        assert!(
            self.pending_raw_dpc_task_batch.is_none(),
            "a prior raw-DPC task batch was planned but not executed"
        );
        let mut next_state = self.rdp_state.fork_for_decode();
        let mut planned = Vec::with_capacity(requests.len());
        let mut members = VecDeque::with_capacity(requests.len());
        for request in requests {
            let carry_in = RawDpcCarryIn::capture(&next_state);
            let (member, delta, execution) =
                plan_raw_dpc_inner(&self.coordinator, &next_state, request).map_err(|reason| {
                    RenderError::Backend {
                        backend: "render-wgpu/raw-dpc-task-batch-plan",
                        reason,
                    }
                })?;
            next_state.apply(&delta);
            members.push_back(PlannedRawDpcTaskMember {
                carry_in,
                execution,
            });
            planned.push(member);
        }
        self.rdp_state = next_state;
        self.pending_raw_dpc_task_batch = Some(PlannedRawDpcTaskBatch { members });
        Ok(planned)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, RenderError> {
        let replacement_pipeline = if self.probes.compute_raster_replace_enabled {
            self.triangle_pipeline.as_deref_mut()
        } else {
            None
        };
        let (prepared, triangles, pending, draw_tmem, mut compute_probes, replacement_receipt) =
            execute_raw_dpc_inner(
                &mut self.coordinator,
                bound,
                self.raw_dpc_carry_in_before_last_plan
                    .unwrap_or_else(|| RawDpcCarryIn::capture(&self.rdp_state)),
                &mut self.color_targets,
                self.configured_target_extent,
                self.probes.project_gpu_tmem,
                self.probes.compute_raster_probe_enabled,
                self.probes.compute_raster_replace_enabled,
                replacement_pipeline,
                None,
                None,
                None,
            )
            .map_err(RenderError::from)?;

        self.compute_raster_probe_receipt = None;
        self.compute_raster_replace_receipt = replacement_receipt;
        if let Some(checkpoint) = self.compute_raster_checkpoint_probe.as_mut() {
            checkpoint
                .push_packet(core::mem::take(&mut compute_probes), pending.is_some())
                .map_err(RenderError::from)?;
        }
        let mut probe_elapsed = Duration::ZERO;
        let mut probe_draws = 0u32;
        let mut probe_pixels = 0u32;
        let mut probe_batches = 0u32;
        for probe in &compute_probes {
            probe_batches += 1;
            probe_draws += u32::try_from(probe.batch.draws().len())
                .expect("bounded raw-DPC draw count fits u32");
            probe_pixels += probe.extent.width * probe.extent.height;
        }
        if !compute_probes.is_empty() {
            let pipeline = self
                .triangle_pipeline
                .as_mut()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
                .map_err(RenderError::from)?;
            let started = Instant::now();
            if self.probes.compute_raster_chain_probe_enabled {
                let first = &compute_probes[0];
                for (batch, probe) in compute_probes.iter().enumerate().skip(1) {
                    if probe.ordinal != first.ordinal
                        || probe.extent.width != first.extent.width
                        || probe.extent.height != first.extent.height
                        || probe.batch.target() != first.batch.target()
                        || probe.batch.generation() != first.batch.generation()
                    {
                        return Err(RenderError::from(
                            WgpuRawDpcExecutionError::ComputeRasterProbeChainIncompatible {
                                ordinal: first.ordinal,
                                batch,
                            },
                        ));
                    }
                }
                let dispatches: Vec<_> = compute_probes
                    .iter()
                    .map(|probe| ComputeHotColorDispatch {
                        triangles: &probe.triangles,
                        tmem: &probe.tmem,
                        tile: probe.tile,
                        first_row: 0,
                        row_count: probe.extent.height,
                        first_column: 0,
                        column_count: probe.extent.width,
                    })
                    .collect();
                let actual = pipeline
                    .compute_triangle_hot_color_chain(
                        first.extent,
                        &first.resident_bytes,
                        &dispatches,
                    )
                    .map_err(WgpuRawDpcExecutionError::TriangleDraw)
                    .map_err(RenderError::from)?;
                validate_compute_probe_output(
                    first,
                    compute_probes.last().expect("non-empty probe chain"),
                    &actual,
                )
                .map_err(RenderError::from)?;
            } else {
                let inputs: Vec<_> = compute_probes
                    .iter()
                    .map(|probe| ComputeHotColorBatch {
                        extent: probe.extent,
                        resident_bytes: &probe.resident_bytes,
                        triangles: &probe.triangles,
                        tmem: &probe.tmem,
                        tile: probe.tile,
                    })
                    .collect();
                let outputs = pipeline
                    .compute_triangle_hot_color_batches(&inputs)
                    .map_err(WgpuRawDpcExecutionError::TriangleDraw)
                    .map_err(RenderError::from)?;
                for (probe, actual) in compute_probes.iter().zip(outputs) {
                    validate_compute_probe_output(probe, probe, &actual)
                        .map_err(RenderError::from)?;
                }
            }
            probe_elapsed = started.elapsed();
        }
        if probe_batches != 0 {
            self.compute_raster_probe_receipt = Some(ComputeRasterProbeReceipt {
                submission_count: 1,
                batch_count: probe_batches,
                draw_count: probe_draws,
                target_pixels: probe_pixels,
                admission_elapsed: Duration::ZERO,
                elapsed: probe_elapsed,
                effects_elapsed: Duration::ZERO,
            });
        }

        // **The GPU triangle draw is diagnostic, and it is 65% of this
        // backend's frame time.**
        //
        // `draw_admitted_triangles` fills `self.triangle_draw_output`, which
        // this file documents as "the most recent triangle draw's real
        // GPU-observed color/depth output ... never an accumulated history,
        // never a persistent framebuffer", and which `present` refuses to
        // scan out by name: "one submission's readback, not a VI-sampled
        // framebuffer". Guest-visible pixels come from the CPU rasterizer
        // (`targets::raw_triangle`) writing RDRAM, which VI samples.
        //
        // Measured on WM2000 (rs + wgpu, bounded census, 1200 pumps),
        // timing each layer directly rather than deriving it:
        //
        //     session census `Execute`      18.18 s
        //       draw_admitted_triangles    ~13    s   <-- THIS, unpresented
        //       execute_raw_dpc_inner        5.29 s
        //         raster_triangle            3.88 s   (94-102 ns/px, normal)
        //
        // So ~65% of `execute` cannot change a presented pixel. Every reader
        // of `last_triangle_draw()` is a `#[cfg(test)]` assertion or a
        // `Debug` impl -- verified by grep, 16 call sites, zero in production
        // code -- so skipping it on the play path loses no guest-visible
        // behaviour and no test coverage.
        //
        // Kept, not deleted: the host-GPU suite drives real WGSL on a live
        // adapter through this path. `FN64_GPU_TRIANGLE_DRAW=1` forces it on
        // for a play run that wants to exercise the pipeline.
        if !triangles.is_empty() {
            // Always called: it validates. Only its GPU submission is gated,
            // inside the function -- see the note there.
            raw_dpc_execute_census::timed(raw_dpc_execute_census::Phase::DrawValidation, || {
                self.draw_admitted_triangles(triangles, draw_tmem, self.probes.project_gpu_tmem)
            })
            .map_err(RenderError::from)?;
        }

        // Stored only after every fallible step of THIS submission has
        // succeeded, and replaced rather than merged.
        //
        // Ordering: a triangle draw that fails (`TriangleDrawBeforeCreate`
        // on an adapterless host, for one) makes this call return `Err`,
        // and a submission that failed must leave no redeemable token
        // behind -- so the store happens after the draw, never before it.
        //
        // Replacement: a token still held from an earlier submission was
        // never redeemed, and carrying it forward would let a later
        // `publish_raw_dpc` publish a fill that belongs to a submission
        // that already retired. Dropping it leaves the registry at its
        // prior generation, which is the correct "nothing published"
        // outcome. On the error path above, the stale token is likewise
        // left untouched rather than replaced -- it was already
        // unredeemable by submission identity, and this submission
        // produced nothing to put in its place.
        self.pending_fill_publication = pending;

        Ok(prepared)
    }

    fn execute_raw_dpc_task_batch(
        &mut self,
        bounds: Vec<BoundSubmittedRawDpc>,
    ) -> Result<Vec<BackendPreparedRawDpc>, RenderError> {
        assert!(
            self.last_task_batch_execution_mechanism.is_none(),
            "raw-DPC task mechanism must be consumed before the next task batch"
        );
        let task_cpu_phase_started = task_cpu_phase_census::task_started();
        assert!(
            self.task_cpu_phase_census.is_none(),
            "a task CPU phase census must reach its final publication before the next task"
        );
        let planned_batch =
            self.pending_raw_dpc_task_batch
                .take()
                .ok_or_else(|| RenderError::Backend {
                    backend: "render-wgpu/raw-dpc-task-batch-execute",
                    reason: "no planned raw-DPC task batch is pending".to_string(),
                })?;
        if bounds.is_empty() || bounds.len() != planned_batch.members.len() {
            return Err(RenderError::Backend {
                backend: "render-wgpu/raw-dpc-task-batch-execute",
                reason: format!(
                    "bound member count {} does not match planned member count {}",
                    bounds.len(),
                    planned_batch.members.len()
                ),
            });
        }
        assert!(
            self.pending_fill_publication.is_none()
                && self.task_batch_pending_fill_publications.is_empty(),
            "task-batch execution requires no unpublished color completion"
        );

        let registry_clone_bytes = self
            .color_targets
            .as_ref()
            .map(|registry| {
                registry
                    .residents()
                    .iter()
                    .map(|resident| resident.device_bytes().device_bytes().len())
                    .sum()
            })
            .unwrap_or(0);
        let mut private_color_targets =
            task_compute_census::timed_registry_clone(registry_clone_bytes, || {
                self.color_targets.clone()
            });
        let mut pending_publications = VecDeque::new();
        let mut deferred_draws = Vec::with_capacity(bounds.len());
        let mut prepared = Vec::with_capacity(bounds.len());
        let mut guest_read_pool = TaskGuestReadCapturePool::default();
        let mut ordered_cpu_color_batch = OrderedCpuColorBatch::new();
        let mut task_cpu_phase_census =
            task_cpu_phase_census::Task::begin(bounds.len(), task_cpu_phase_started);
        let mut compute_members = 0usize;
        let mut cpu_members = 0usize;
        {
            let mut batch = self.coordinator.begin_execution_batch();
            let mut members = bounds
                .into_iter()
                .zip(planned_batch.members)
                .map(|(bound, member)| {
                    (
                        bound,
                        member.carry_in,
                        TaskMemberDispatch::Planned(member.execution),
                    )
                })
                .peekable();
            let mut retry_as_cpu = None;
            while let Some((bound, carry_in, dispatch)) =
                retry_as_cpu.take().or_else(|| members.next())
            {
                if dispatch == TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate)
                    && self.probes.task_compute_raster_enabled
                {
                    if ordered_cpu_color_batch.tail.is_some() {
                        ordered_cpu_color_batch
                            .flush(private_color_targets.as_mut().expect(
                                "an ordered CPU color accumulator implies a private registry",
                            ))
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                    }
                    let segment_started = task_compute_census::segment_started();
                    let mut color_batch = ColorTargetExecutionBatch::new();
                    let mut staged_segment = Vec::new();
                    let mut next = Some((bound, carry_in));
                    loop {
                        let (bound, carry_in) = next.take().expect("compute segment member exists");
                        let physical = staged_segment
                            .iter()
                            .rev()
                            .find_map(|member: &StagedRawDpcMember| match &member.outcome {
                                StagedOutcome::DeferredMixedColorAndTmem { tmem, .. } => {
                                    Some(tmem.physical())
                                }
                                _ => None,
                            })
                            .unwrap_or_else(|| batch.physical());
                        let disposition = admit_task_compute_member(
                            &batch,
                            Some(physical),
                            bound,
                            carry_in,
                            &mut private_color_targets,
                            self.configured_target_extent,
                            self.probes.project_gpu_tmem,
                            &mut color_batch,
                            &mut guest_read_pool,
                        )
                        .map_err(RenderError::from)?;
                        let staged = match disposition {
                            TaskComputeDisposition::Compute(ComputeEligibleTaskMember(staged)) => {
                                staged
                            }
                            TaskComputeDisposition::Cpu { bound, reason } => {
                                retry_as_cpu =
                                    Some((bound, carry_in, TaskMemberDispatch::Cpu(reason)));
                                break;
                            }
                        };
                        staged_segment.push(staged);
                        next = match members.peek() {
                            Some((
                                _,
                                _,
                                TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate),
                            )) => {
                                let (bound, carry_in, _) =
                                    members.next().expect("peeked compute member disappeared");
                                Some((bound, carry_in))
                            }
                            _ => None,
                        };
                        if next.is_none() {
                            break;
                        }
                    }
                    if staged_segment.is_empty() {
                        continue;
                    }
                    let pipeline = self
                        .triangle_pipeline
                        .as_deref_mut()
                        .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)
                        .map_err(RenderError::from)?;
                    let segment_members = staged_segment.len();
                    let program_attribution = task_compute_census::wants_program_attribution()
                        .then(|| compute_segment_program_attribution(&staged_segment));
                    let completed_segment =
                        complete_deferred_compute_segment(&mut batch, pipeline, staged_segment)
                            .map_err(RenderError::from)?;
                    let compute_elapsed = task_compute_census::record_segment(
                        segment_members,
                        program_attribution,
                        segment_started,
                    );
                    if let Some(task) = task_cpu_phase_census.as_mut() {
                        task.record_compute_segment(compute_elapsed);
                    }
                    compute_members += segment_members;
                    let mut completed = completed_segment.iter();
                    let first = completed
                        .next()
                        .expect("a staged compute segment completes at least one member");
                    let mut shadow_segment =
                        CompletedTaskColorSegment::new(first.pending.color.full());
                    for member in completed {
                        shadow_segment
                            .append(member.pending.color.full())
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                    }
                    // `shadow_segment` has already validated every generation
                    // edge. Its full tail bytes are copied into the private
                    // registry only when another task member can consume
                    // them. A terminal segment's pending publications own all
                    // checkpoint images through `ExactCheckpointImages`; the
                    // private shadow has no remaining reader.
                    if retry_as_cpu.is_some() || members.peek().is_some() {
                        let shadow_byte_count = completed_segment
                            .last()
                            .expect("a completed compute segment is nonempty")
                            .pending
                            .color
                            .full()
                            .device_bytes()
                            .device_bytes()
                            .len();
                        task_compute_census::timed_shadow_clone(shadow_byte_count, || {
                            private_color_targets
                                .as_mut()
                                .expect("a staged color completion built the private registry")
                                .commit_task_shadow_segment(shadow_segment)
                        })
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?;
                    }
                    for completed in completed_segment {
                        pending_publications.push_back(completed.pending);
                        deferred_draws.push((completed.triangles, completed.draw_tmem));
                        prepared.push(completed.prepared);
                    }
                } else {
                    cpu_members += 1;
                    let reason = match dispatch {
                        TaskMemberDispatch::Planned(PlannedTaskExecution::Cpu(reason)) => {
                            TaskComputeCpuReason::Planned(reason)
                        }
                        TaskMemberDispatch::Planned(PlannedTaskExecution::ComputeCandidate) => {
                            TaskComputeCpuReason::ComputeDisabled
                        }
                        TaskMemberDispatch::Cpu(reason) => reason,
                    };
                    let cpu_started = task_compute_census::cpu_started();
                    let ordered_cpu_batch = self
                        .probes
                        .task_cpu_color_batch_enabled
                        .then_some(&mut ordered_cpu_color_batch);
                    let (member, triangles, pending, draw_tmem, _, _) = execute_raw_dpc_inner(
                        &mut batch,
                        bound,
                        carry_in,
                        &mut private_color_targets,
                        self.configured_target_extent,
                        self.probes.project_gpu_tmem,
                        false,
                        false,
                        None,
                        Some(&mut guest_read_pool),
                        ordered_cpu_batch,
                        task_cpu_phase_census.as_mut(),
                    )
                    .map_err(RenderError::from)?;
                    let cpu_phase_attributed = pending
                        .as_ref()
                        .is_some_and(|pending| pending.cpu_phase_attributed);
                    let cpu_elapsed = task_compute_census::record_cpu(reason, cpu_started);
                    if let Some(task) = task_cpu_phase_census.as_mut() {
                        task.record_member_total(cpu_phase_attributed, cpu_elapsed);
                    }
                    if let Some(pending) = pending {
                        let pending = task_cpu_phase_census::timed(
                            task_cpu_phase_census.as_mut(),
                            pending.cpu_phase_attributed,
                            task_cpu_phase_census::Phase::SparseCheckpoint,
                            || ordered_cpu_color_batch.finish_member(pending),
                        )
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?;
                        if members.peek().is_some()
                            && matches!(&pending.color, PendingColorPublication::Full(_))
                        {
                            let shadow_byte_count =
                                pending.color.full().device_bytes().device_bytes().len();
                            task_compute_census::timed_shadow_clone(shadow_byte_count, || {
                                private_color_targets
                                    .as_mut()
                                    .expect("a staged color completion built the private registry")
                                    .commit_task_shadow_segment(CompletedTaskColorSegment::new(
                                        pending.color.full(),
                                    ))
                            })
                            .map_err(WgpuRawDpcExecutionError::Target)
                            .map_err(RenderError::from)?;
                        }
                        pending_publications.push_back(pending);
                    } else {
                        assert!(
                            ordered_cpu_color_batch.active.is_none(),
                            "an ordered CPU color member must produce publication authority"
                        );
                    }
                    deferred_draws.push((triangles, draw_tmem));
                    prepared.push(member);
                }
            }
            batch.finish();
        }
        debug_assert_eq!(compute_members + cpu_members, prepared.len());
        task_compute_census::record_task(prepared.len(), cpu_members);

        if self.color_targets.is_none() {
            if let Some(private) = private_color_targets.as_ref() {
                self.color_targets = Some(
                    ColorTargetRegistry::try_new(private.layout(), COLOR_TARGET_REGISTRY_CAPACITY)
                        .map_err(WgpuRawDpcExecutionError::Target)
                        .map_err(RenderError::from)?,
                );
            }
        }
        for (triangles, draw_tmem) in deferred_draws {
            if !triangles.is_empty() {
                self.draw_admitted_triangles(triangles, draw_tmem, self.probes.project_gpu_tmem)
                    .map_err(RenderError::from)?;
            }
        }
        self.task_batch_pending_fill_publications = pending_publications;
        self.task_cpu_phase_census = task_cpu_phase_census;
        self.last_task_batch_execution_mechanism = Some(
            fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(cpu_members, compute_members)
                .expect("a successful raw-DPC task batch executes at least one member"),
        );
        Ok(prepared)
    }

    fn take_raw_dpc_task_batch_execution_mechanism(
        &mut self,
    ) -> Option<fn64_render::RawDpcTaskBatchExecutionMechanism> {
        self.last_task_batch_execution_mechanism.take()
    }

    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<CompletedWrite> {
        // A submission mismatch deliberately yields an EMPTY list rather
        // than a panic or the wrong fill's writes: the caller then takes the
        // zero-write commit branch, which fails loudly with
        // `EffectCountMismatch` against the packet's own nonempty
        // guest-write journal. A loud rejection, never a quiet wrong publish.
        self.pending_fill_publication
            .as_ref()
            .filter(|pending| pending.submission == submission)
            .or_else(|| {
                self.task_batch_pending_fill_publications
                    .iter()
                    .find(|pending| pending.submission == submission)
            })
            .map(|pending| pending.guest_writes.clone())
            .unwrap_or_default()
    }

    /// The bytes behind the same pending fill token
    /// [`Self::staged_guest_render_target_writes`] reported ranges for, in
    /// the identical order -- one `Vec<u8>` per `CompletedWrite`, sliced out
    /// of the fill's own full-extent `DeviceColorBytes` buffer at each
    /// write's declared physical range.
    ///
    /// The slicing is the same physical -> buffer-relative arithmetic
    /// `fill_completed_writes` used to build the digests in the first place,
    /// so these bytes are by construction the exact bytes those digests
    /// cover. The caller re-derives each digest anyway (see the trait
    /// method's own doc) rather than trusting that construction argument.
    ///
    /// A submission mismatch yields an EMPTY list, exactly as the sibling
    /// method does and for the same reason: a caller that committed a
    /// nonempty write list and then gets no bytes must fail loudly, never
    /// copy some other submission's pixels into guest memory.
    ///
    /// Nonclaim: this method writes nothing. It hands owned copies to a
    /// caller that owns the RDRAM allocation; whether any guest byte changes
    /// is that caller's decision, made after its own commit succeeded.
    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<Arc<[u8]>> {
        let Some(pending) = self
            .pending_fill_publication
            .as_ref()
            .filter(|pending| pending.submission == submission)
            .or_else(|| {
                self.task_batch_pending_fill_publications
                    .iter()
                    .find(|pending| pending.submission == submission)
            })
        else {
            return Vec::new();
        };

        match &pending.color {
            PendingColorPublication::Sparse(checkpoint) => {
                let started = task_cpu_phase_census::started(
                    self.task_cpu_phase_census.as_ref(),
                    pending.cpu_phase_attributed,
                );
                let payloads = if shared_copyback_payloads_enabled() {
                    checkpoint.shared_payloads().collect()
                } else {
                    checkpoint.payloads().map(Arc::<[u8]>::from).collect()
                };
                task_cpu_phase_census::record_started(
                    self.task_cpu_phase_census.as_mut(),
                    task_cpu_phase_census::Phase::GuestPayloadMaterialization,
                    started,
                );
                return payloads;
            }
            PendingColorPublication::Full(_) => {}
        }
        let initialized = pending.color.full();
        let key = initialized.key();
        let base = key.address().get();
        let buffer = initialized.device_bytes().device_bytes();
        pending
            .guest_writes
            .iter()
            .map(|write| {
                let fn64_render_ir::ResourceRegion::Rdram { range, .. } = write.access().region()
                else {
                    panic!(
                        "a staged guest render-target write must name an RDRAM region; \
                         fill_completed_writes rejected every other kind when it built this list"
                    );
                };
                let start = (range.start().get() - base) as usize;
                let end = start + range.len() as usize;
                buffer
                    .get(start..end)
                    .expect(
                        "every staged write's range lies inside its own color target's \
                         full-extent buffer -- fill_completed_writes sliced these same \
                         bounds to compute the digests",
                    )
                    .into()
            })
            .collect()
    }

    fn take_raw_dpc_visual_target_snapshot(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Result<
        fn64_render::RawDpcVisualTargetSnapshotV1,
        fn64_render::RawDpcVisualTargetSnapshotRefusal,
    > {
        let Some((expected, marker)) = self.last_published_visual_target.take() else {
            return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget);
        };
        if expected != submission {
            return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::SubmissionMismatch);
        }
        let key = match marker {
            PublishedVisualTargetMarker::Exact(key) => key,
            PublishedVisualTargetMarker::NoColorTarget => {
                return Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget)
            }
            PublishedVisualTargetMarker::ComputeCoverageUnavailable => {
                return Err(
                    fn64_render::RawDpcVisualTargetSnapshotRefusal::ComputeCoverageUnavailable,
                )
            }
        };
        let resident = self
            .color_targets
            .as_ref()
            .and_then(|registry| {
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key() == key)
            })
            .ok_or(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget)?;
        resident.visual_snapshot(submission)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: ReadyRawDpcCommitCapsule<'_>,
    ) -> CommittedRawDpcOutcome {
        // Taken unconditionally: a stale token from an earlier submission
        // must never survive into a later one. Its
        // `InitializedCandidateColorTarget` is simply dropped, leaving the
        // registry at its prior generation -- the loud-rejection policy's
        // "nothing published" outcome, reached by construction rather than
        // by a rollback step.
        let submission = publication.submission();
        let pending = if self
            .task_batch_pending_fill_publications
            .front()
            .is_some_and(|pending| pending.submission == submission)
        {
            self.task_batch_pending_fill_publications.pop_front()
        } else {
            self.pending_fill_publication.take()
        };
        // Published after `commit()`, deliberately: `ReadyPublication::commit`
        // is the documented straight-line, infallible body, and
        // `prepare_publication` has already asserted every queue/submission/
        // retirement identity fact. Advancing the registry only after that
        // means a resident generation exists only for a submission that
        // genuinely reached `Published`. The one residual window is a panic
        // between the two; on that path the process is already unwinding and
        // the token has been taken, so the registry stays at its prior
        // generation -- the correct outcome, not a leak.
        let outcome = self.coordinator.prepare_publication(publication).commit();
        let mut task_cpu_phase_census = self.task_cpu_phase_census.take();

        let visual_marker = if let Some(pending) = pending {
            assert_eq!(
                pending.submission, submission,
                "publish_raw_dpc received a capsule for a different submission than the one \
                 execute_raw_dpc staged a color-target write for"
            );
            let registry = self
                .color_targets
                .as_mut()
                .expect("a staged fill publication implies the registry was built");
            let target_key = match &pending.color {
                PendingColorPublication::Full(initialized) => initialized.key(),
                PendingColorPublication::Sparse(checkpoint) => checkpoint.key(),
            };
            let exact_physical_coverage = pending.exact_physical_coverage;
            match pending.color {
                PendingColorPublication::Full(initialized) => {
                    registry
                        .prepare_publication(initialized)
                        .unwrap_or_else(|error| {
                            panic!("color-target publication rejected after guest commit: {error}")
                        })
                        .publish();
                }
                PendingColorPublication::Sparse(checkpoint) => {
                    let started = task_cpu_phase_census::started(
                        task_cpu_phase_census.as_ref(),
                        pending.cpu_phase_attributed,
                    );
                    registry
                        .commit_sparse_checkpoint(checkpoint)
                        .unwrap_or_else(|error| {
                            panic!("sparse color-target publication rejected after guest commit: {error}")
                        });
                    task_cpu_phase_census::record_started(
                        task_cpu_phase_census.as_mut(),
                        task_cpu_phase_census::Phase::SparsePublication,
                        started,
                    );
                }
            }
            if exact_physical_coverage {
                PublishedVisualTargetMarker::Exact(target_key)
            } else {
                PublishedVisualTargetMarker::ComputeCoverageUnavailable
            }
        } else {
            PublishedVisualTargetMarker::NoColorTarget
        };
        self.last_published_visual_target = Some((submission, visual_marker));
        self.task_cpu_phase_census =
            task_cpu_phase_census.and_then(task_cpu_phase_census::Task::publication_finished);
        outcome
    }
}

impl SettingsSink for WgpuBackend {}

/// `plan_raw_dpc`'s body: decode `request`'s capture once through T1's typed
/// planning decoder, push every admitted command through T0's sealed writer,
/// and seal the decoder-derived journal. The seed journal exists only to
/// satisfy capture preflight; [`crate::raw_dpc::PlanningDecodedRawDpc`]
/// cannot enter ordinary execution and carries the authoritative access plan
/// derived during that same decode.
///
/// **The decode observes `durable_state`, and that is load-bearing.**
/// The journal a capture declares is not a function of its bytes alone: a
/// `FillRectangle`/`TextureRectangle` reads its destination back off
/// `RdpState::color_image()`, which an *earlier* submission's
/// `SetColorImage` may have staged. `plan_texture_rectangle` treats a
/// missing color image as "declares no write" (`return Ok(())`) rather than
/// as an error, so deriving against `RdpState::default()` silently returns a
/// shorter access list. The derivation therefore observes the same durable
/// predecessor as command planning.
///
/// Decoding against durable state is side-effect-free:
/// `decode_raw_dpc` takes `&RdpState` and forks it (`fork_for_decode`), so
/// neither pass can mutate the caller's state, and only the real pass's
/// `state_delta` is ever applied. Every `SubmittedTicket`
/// minted here is through a throwaway, locally owned `TicketAuthoritySet` --
/// `crate::decode_raw_dpc` only needs one that is internally consistent
/// with the capture it decodes, never the "real" production queue (that
/// queue identity is proven separately, by `RawDpcBackendAuthority::
/// begin_plan`'s own paired-queue assertion against `request`).
fn plan_raw_dpc_inner(
    coordinator: &RawDpcCoordinator<PhysicalTmemState>,
    durable_state: &RdpState,
    request: RawDpcPlanRequest,
) -> Result<
    (
        PlannedRawDpcSubmission,
        crate::RdpStateDelta,
        PlannedTaskExecution,
    ),
    String,
> {
    let capture = request.capture();
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let submission_start = submission.start();
    let capture_words = submission.command_words();

    let ticket = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::Prepare, || {
        let seed_journal = planning_seed_journal(&submission, layout)
            .map_err(|error| format!("raw-DPC plan seed journal failed: {error}"))?;
        let decoded_ticket = finalize_with_zero_reads(
            layout,
            capture.transaction_sequence(),
            submission,
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            seed_journal,
        )
        .map_err(|error| format!("raw-DPC plan preflight failed: {error}"))?;
        submit_locally(decoded_ticket)
            .map_err(|error| format!("raw-DPC plan submission failed: {error}"))
    })?;

    let decoded = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::DecodeAndDerive, || {
        crate::raw_dpc::decode_raw_dpc_for_planning(ticket, durable_state)
    })
    .map_err(|error| format!("raw-DPC plan decode failed: {error}"))?;
    let accesses = decoded.resource_plan().accesses().to_vec();
    let declared = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum::<u32>();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(fn64_render_ir::MAX_RESOURCE_ACCESSES, declared.max(1))
            .map_err(|error| format!("raw-DPC plan journal limits failed: {error}"))?,
        accesses,
    )
    .map_err(|error| format!("raw-DPC plan journal failed: {error}"))?;
    let delta = decoded.state_delta().clone();
    let execution = classify_task_execution(decoded.commands(), durable_state);

    let planned = raw_dpc_plan_census::timed(raw_dpc_plan_census::Phase::AdmitAndSeal, || {
        let mut writer = coordinator.begin_plan(request);
        push_planning_decoded_raw_dpc(
            &mut writer,
            &decoded,
            &capture_words,
            layout,
            submission_start,
        )
        .map_err(|error| format!("raw-DPC plan admission failed: {error}"))?;
        writer
            .finish(journal)
            .map_err(|error| format!("raw-DPC plan seal failed: {error}"))
    })?;
    Ok((planned, delta, execution))
}

fn classify_task_execution(
    commands: &[crate::DecodedRawDpcCommand],
    durable_state: &RdpState,
) -> PlannedTaskExecution {
    let has_raw_triangle = commands
        .iter()
        .any(|command| matches!(command.kind(), crate::RawDpcCommandKind::RawTriangle(_)));
    if !has_raw_triangle {
        return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::NoRawTriangle(
            classify_no_raw_triangle(commands),
        ));
    }
    if commands.iter().any(|command| {
        matches!(
            command.kind(),
            crate::RawDpcCommandKind::FillRectangle(_)
                | crate::RawDpcCommandKind::TextureRectangle(_)
        )
    }) {
        return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::MixedFillOrTexrect);
    }

    let mut other_mode = durable_state.other_mode();
    let mut combine = durable_state.combine();
    for command in commands {
        match command.kind() {
            crate::RawDpcCommandKind::SetOtherMode(value) => other_mode = Some(value),
            crate::RawDpcCommandKind::SetCombine(value) => combine = Some(value),
            crate::RawDpcCommandKind::RawTriangle(triangle) => {
                let (Some(combine), Some(other_mode)) = (combine, other_mode) else {
                    // Missing state remains a runtime admission concern so its
                    // existing loud typed refusal is not converted to CPU.
                    continue;
                };
                if let Err(reason) = ComputeRasterProgramKey::try_admit_program(
                    combine,
                    other_mode,
                    triangle.flags().textured(),
                ) {
                    return PlannedTaskExecution::Cpu(PlannedTaskCpuReason::DefinitelyCpu(
                        reason.into(),
                    ));
                }
            }
            _ => {}
        }
    }
    PlannedTaskExecution::ComputeCandidate
}

fn classify_no_raw_triangle(
    commands: &[crate::DecodedRawDpcCommand],
) -> PlannedNoRawTriangleReason {
    let mut fill = false;
    let mut texrect = false;
    let mut tmem_load = false;
    let mut sync_or_state = false;
    for command in commands {
        match command.kind() {
            crate::RawDpcCommandKind::FillRectangle(_) => fill = true,
            crate::RawDpcCommandKind::TextureRectangle(_) => texrect = true,
            crate::RawDpcCommandKind::LoadBlock(_)
            | crate::RawDpcCommandKind::LoadTile(_)
            | crate::RawDpcCommandKind::LoadTlut(_) => tmem_load = true,
            crate::RawDpcCommandKind::NoOp { .. } => {}
            crate::RawDpcCommandKind::RawTriangle(_) => {
                unreachable!("no-triangle classification received a raw triangle")
            }
            _ => sync_or_state = true,
        }
    }
    classify_no_raw_triangle_flags(fill, texrect, tmem_load, sync_or_state)
}

fn classify_no_raw_triangle_flags(
    fill: bool,
    texrect: bool,
    tmem_load: bool,
    sync_or_state: bool,
) -> PlannedNoRawTriangleReason {
    match (fill, texrect, tmem_load) {
        (true, false, false) => PlannedNoRawTriangleReason::FillOnly,
        (false, true, false) => PlannedNoRawTriangleReason::TexrectOnly,
        (true, true, false) => PlannedNoRawTriangleReason::FillAndTexrect,
        (false, false, true) => PlannedNoRawTriangleReason::TmemLoadOnly,
        (true, false, true) => PlannedNoRawTriangleReason::FillAndTmemLoad,
        (false, true, true) => PlannedNoRawTriangleReason::TexrectAndTmemLoad,
        (true, true, true) => PlannedNoRawTriangleReason::FillTexrectAndTmemLoad,
        (false, false, false) if sync_or_state => PlannedNoRawTriangleReason::SyncStateOnly,
        (false, false, false) => PlannedNoRawTriangleReason::NoOpOnly,
    }
}

fn submit_locally(decoded: DecodedTicket) -> Result<SubmittedTicket, ValidationError> {
    let (mut queue, _, _) = TicketAuthoritySet::try_new()?.into_roles();
    queue.submit(decoded)
}

/// `fn64_render::decode_raw_dpc_capture` hard-codes
/// `DeferredGuestReadCapture::empty()`, which only satisfies a plan whose
/// guest-read plan is itself empty -- never true here, since every admitted
/// TMEM load declares at least one `TmemLoadSource` read. `plan_raw_dpc`'s
/// internal planning decode exists purely to learn the command
/// structure/journal shape and drive T1's push loop -- it is not the
/// production submission the ABI session's `finalize_and_submit` performs
/// later with the real captured bytes -- so a correctly *sized*, zero-filled
/// capture is exactly as valid here as any other byte content: `finish`'s
/// own access-count/order check never inspects read content, only shape.
///
/// `full_sync_boundaries` is NOT zero-filled the way the read bytes are, and
/// must be the originating capture's own list. Stream derivation requires one
/// boundary per decoded `SYNC_FULL` opcode, so an empty list here would fail
/// the planning decode with `MissingFullSyncObservation` for any
/// capture containing a FullSync -- making the site unplannable no matter
/// what its producer supplied. Shape, unlike content, is load-bearing here.
fn finalize_with_zero_reads(
    layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    submission: fn64_render::OwnedRawDpcSubmission,
    cmd_end: fn64_render_ir::TemporalBoundary,
    full_sync_boundaries: Vec<fn64_render_ir::FullSyncBoundary>,
    journal: ResourceJournal,
) -> Result<DecodedTicket, ValidationError> {
    let preflight = fn64_render::preflight_raw_dpc_capture(
        layout,
        transaction_sequence,
        submission,
        cmd_end,
        full_sync_boundaries,
        journal,
    )?;
    let capture = fn64_render_ir::DeferredGuestReadCapture::new(
        preflight
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                fn64_render_ir::CapturedGuestRead::try_new(
                    *read,
                    vec![0; read.range().len() as usize],
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    preflight.finalize(capture)
}

/// A minimal, self-consistent command-decode seed journal sufficient to pass
/// capture preflight.
/// The typed planning decoder derives the authoritative access list instead
/// of admitting this seed as execution authority.
///
/// The command-decode access's region kind must match `submission.source()`
/// exactly (`fn64_render_ir::workload::validate_one_to_one_command_reads`
/// keys a stream's expected read by `RawStreamKind`, not by byte range
/// alone): `RawDpcSource::Rdram` needs `ResourceRegion::Rdram { resource:
/// RdramResource::RawCommands, .. }`; `RawDpcSource::XbusDmem` needs
/// `ResourceRegion::RspDmem(DmemRange)`, the same 4 KiB DMEM-relative
/// address space `submission.start()`/`end()` are already expressed in for
/// an XBUS submission (`OwnedRawDpcSubmission::validate_range` bounds XBUS
/// ranges to `RSP_DMEM_BYTES`, never the RDP's 24-bit physical space).
///
/// It contains no speculative TMEM source: the typed planning decode derives
/// those exact ranges, and preflight need not allocate or zero bytes for a
/// placeholder read that execution can never consume.
fn planning_seed_journal(
    submission: &fn64_render::OwnedRawDpcSubmission,
    layout: fn64_render_ir::PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    use fn64_render_ir::{DmemRange, OperationId, RdramResource, ResourceRegion};
    let start = submission.start();
    let command_bytes = u32::try_from(submission.command_words().len() * 4)
        .expect("bounded command stream fits u32 bytes");
    let command_access = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        match submission.source() {
            fn64_render::RawDpcSource::Rdram => ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout.range(start, start + command_bytes)?,
            },
            fn64_render::RawDpcSource::XbusDmem => {
                ResourceRegion::RspDmem(DmemRange::try_new(start, start + command_bytes)?)
            }
        },
    )?;
    let accesses = vec![command_access];
    let declared = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum::<u32>();
    ResourceJournal::try_new(
        ResourceJournalLimits::try_new(64, declared.max(1))?,
        accesses,
    )
}

#[cfg(test)]
fn single_source_probe_journal(
    submission: &fn64_render::OwnedRawDpcSubmission,
    layout: fn64_render_ir::PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    planning_seed_journal(submission, layout)
}

/// `plan_raw_dpc` always constructs raw-DPC admission (via
/// `fn64_render::decode_raw_dpc_capture`, which hard-codes
/// `WorkloadAdmission::RawDpc`), and `RawDpcCoordinator::execution_view`
/// only ever lends a plan T1's raw-DPC push loop admitted -- a graphics-task
/// packet can never reach this executor. Traps rather than silently
/// defaulting a sequence number, matching AGENTS.md's loud-trap rule.
fn transaction_sequence(packet: &WorkloadPacket) -> u64 {
    match packet.admission() {
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        } => transaction_sequence,
        WorkloadAdmission::GraphicsTask(_) => unreachable!(
            "WgpuBackend's raw-DPC execution seam only ever receives RawDpc-admitted packets"
        ),
    }
}

/// The monomorphic completion surface shared by the ordinary two-slot
/// coordinator and its task-scoped ordered batch guard. Keeping this local
/// prevents the backend from extracting plan contents or completion
/// authority merely to choose a transport lifetime.
trait PhysicalExecutionCoordinator {
    fn physical(&self) -> &PhysicalTmemState;

    fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
        &self,
        bound: &BoundSubmittedRawDpc,
        plan_visitor: &mut PV,
        view: &mut V,
    );

    fn complete_execution(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
        next_physical: PhysicalTmemState,
    ) -> Result<BackendPreparedRawDpc, ValidationError>;

    fn complete_execution_preserving_physical(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, ValidationError>;

    fn complete_execution_preserving_physical_with_effects(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
    ) -> Result<BackendPreparedRawDpc, ValidationError>;
}

impl PhysicalExecutionCoordinator for RawDpcCoordinator<PhysicalTmemState> {
    fn physical(&self) -> &PhysicalTmemState {
        RawDpcCoordinator::physical(self)
    }

    fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
        &self,
        bound: &BoundSubmittedRawDpc,
        plan_visitor: &mut PV,
        view: &mut V,
    ) {
        RawDpcCoordinator::execution_view(self, bound, plan_visitor, view)
    }

    fn complete_execution(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
        next_physical: PhysicalTmemState,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcCoordinator::complete_execution(self, bound, effects, next_physical)
    }

    fn complete_execution_preserving_physical(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcCoordinator::complete_execution_preserving_physical(self, bound)
    }

    fn complete_execution_preserving_physical_with_effects(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcCoordinator::complete_execution_preserving_physical_with_effects(self, bound, effects)
    }
}

impl PhysicalExecutionCoordinator for RawDpcExecutionBatch<'_, PhysicalTmemState> {
    fn physical(&self) -> &PhysicalTmemState {
        RawDpcExecutionBatch::physical(self)
    }

    fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
        &self,
        bound: &BoundSubmittedRawDpc,
        plan_visitor: &mut PV,
        view: &mut V,
    ) {
        RawDpcExecutionBatch::execution_view(self, bound, plan_visitor, view)
    }

    fn complete_execution(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
        next_physical: PhysicalTmemState,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcExecutionBatch::complete_execution(self, bound, effects, next_physical)
    }

    fn complete_execution_preserving_physical(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcExecutionBatch::complete_execution_preserving_physical(self, bound)
    }

    fn complete_execution_preserving_physical_with_effects(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: BackendEffectReport,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        RawDpcExecutionBatch::complete_execution_preserving_physical_with_effects(
            self, bound, effects,
        )
    }
}

/// `execute_raw_dpc`'s body: lend the sealed plan through `execution_view`
/// (which drives the whole stage/finish/effect-report pipeline inside its
/// own `submitted_packet` callback -- see [`ExecutionCollector`]'s doc
/// comment for why), then hand the resulting `BackendEffectReport` and
/// `into_physical_successor` (T3 Phase A) result to
/// `RawDpcCoordinator::complete_execution`.
///
/// `carry_in` is the complete pre-delta state captured for this plan. The walk
/// advances its fields in command order; it never consults the already-folded
/// durable state while retrieving this packet's draws.
struct StagedRawDpcMember {
    bound: BoundSubmittedRawDpc,
    outcome: StagedOutcome,
    triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    draw_tmem: Option<Vec<TmemGpuProjection>>,
    compute_probes: Vec<ComputeRasterProbe>,
    compute_replacement_receipt: Option<ComputeRasterProbeReceipt>,
    execution_view_gross_ns: Option<u64>,
}

fn compute_segment_program_attribution(
    members: &[StagedRawDpcMember],
) -> ComputeProgramAttribution {
    compute_program_attribution_from_members(members.iter().map(|member| {
        let color = match &member.outcome {
            StagedOutcome::DeferredGuestWritesOnly(_, color)
            | StagedOutcome::DeferredMixedColorAndTmem { color, .. } => color,
            _ => unreachable!("a compute-eligible segment contains only deferred outcomes"),
        };
        color.program_attribution
    }))
}

fn compute_program_attribution_from_members(
    programs: impl IntoIterator<Item = ComputeProgramAttribution>,
) -> ComputeProgramAttribution {
    let mut program = None;
    for attribution in programs {
        let id = match attribution {
            ComputeProgramAttribution::Program(id) => id,
            ComputeProgramAttribution::MixedPrograms => {
                return ComputeProgramAttribution::MixedPrograms;
            }
        };
        match program {
            None => program = Some(id),
            Some(first) if first == id => {}
            Some(_) => return ComputeProgramAttribution::MixedPrograms,
        }
    }
    ComputeProgramAttribution::Program(
        program.expect("an admitted compute segment contains at least one member"),
    )
}

fn compute_program_attribution_from_ids(
    ids: impl IntoIterator<Item = u32>,
) -> ComputeProgramAttribution {
    let mut program = None;
    for id in ids {
        match program {
            None => program = Some(id),
            Some(first) if first == id => {}
            Some(_) => return ComputeProgramAttribution::MixedPrograms,
        }
    }
    ComputeProgramAttribution::Program(
        program.expect("an admitted deferred compute plan contains at least one draw"),
    )
}

struct StagedRawDpcFailure {
    bound: BoundSubmittedRawDpc,
    error: WgpuRawDpcExecutionError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TaskComputeCpuReason {
    Planned(PlannedTaskCpuReason),
    ComputeDisabled,
    ExactAdmissionRejected(TaskComputeAdmissionRefusal),
    CompletionShapeNotDeferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskMemberDispatch {
    Planned(PlannedTaskExecution),
    Cpu(TaskComputeCpuReason),
}

/// Only a successfully staged deferred completion is the capability to enter
/// a device segment. A planning candidate that either fails exact admission or
/// stages an ordinary completion retains its bound submission for the ordered
/// CPU path. No generic executor error can construct the CPU variant.
struct ComputeEligibleTaskMember(StagedRawDpcMember);

enum TaskComputeDisposition {
    Compute(ComputeEligibleTaskMember),
    Cpu {
        bound: BoundSubmittedRawDpc,
        reason: TaskComputeCpuReason,
    },
}

fn stage_raw_dpc_member<C: PhysicalExecutionCoordinator>(
    coordinator: &C,
    physical_override: Option<&PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
    carry_in: RawDpcCarryIn,
    color_targets: &mut Option<ColorTargetRegistry>,
    configured_target_extent: Option<TriangleTargetExtent>,
    project_gpu_tmem: bool,
    collect_compute_probe: bool,
    compute_replacement_enabled: bool,
    compute_replacement_pipeline: Option<&mut TrianglePipelineRenderer>,
    color_execution_batch: Option<&mut ColorTargetExecutionBatch>,
    ordered_cpu_color_batch: Option<&mut OrderedCpuColorBatch>,
    task_cpu_phase_census: Option<&mut task_cpu_phase_census::Task>,
    defer_compute_replacement: bool,
    task_guest_read_pool: Option<&mut TaskGuestReadCapturePool>,
) -> Result<StagedRawDpcMember, StagedRawDpcFailure> {
    let observe_task_envelope = task_cpu_phase_census.is_some();
    let mut plan_visitor = PlanCollector::seeded(carry_in);
    let mut view = ExecutionCollector {
        physical: physical_override.unwrap_or_else(|| coordinator.physical()),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::seeded(carry_in),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool,
        outcome: None,
        color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem,
        collect_compute_probe,
        compute_probes: Vec::new(),
        compute_replacement_enabled,
        compute_replacement_pipeline,
        compute_replacement_receipt: None,
        color_execution_batch,
        ordered_cpu_color_batch,
        task_cpu_phase_census,
        defer_compute_replacement,
        deferred_compute: None,
    };
    let (_, execution_view_gross_ns) = raw_dpc_execute_census::timed_observed(
        raw_dpc_execute_census::Phase::View,
        observe_task_envelope,
        || coordinator.execution_view(&bound, &mut plan_visitor, &mut view),
    );
    let _ = plan_visitor; // plan contents were moved into `view.plan` by `plan_visited`

    let outcome = view
        .outcome
        .expect("execution_view always calls submitted_packet exactly once");
    match outcome {
        Ok(outcome) => Ok(StagedRawDpcMember {
            bound,
            outcome,
            triangles: view
                .plan
                .triangles
                .into_iter()
                .map(|planned| planned.draw)
                .collect(),
            draw_tmem: view.draw_tmem,
            compute_probes: view.compute_probes,
            compute_replacement_receipt: view.compute_replacement_receipt,
            execution_view_gross_ns,
        }),
        Err(error) => Err(StagedRawDpcFailure { bound, error }),
    }
}

#[allow(clippy::too_many_arguments)]
fn admit_task_compute_member<C: PhysicalExecutionCoordinator>(
    coordinator: &C,
    physical_override: Option<&PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
    carry_in: RawDpcCarryIn,
    color_targets: &mut Option<ColorTargetRegistry>,
    configured_target_extent: Option<TriangleTargetExtent>,
    project_gpu_tmem: bool,
    color_execution_batch: &mut ColorTargetExecutionBatch,
    task_guest_read_pool: &mut TaskGuestReadCapturePool,
) -> Result<TaskComputeDisposition, WgpuRawDpcExecutionError> {
    match stage_raw_dpc_member(
        coordinator,
        physical_override,
        bound,
        carry_in,
        color_targets,
        configured_target_extent,
        project_gpu_tmem,
        false,
        false,
        None,
        Some(color_execution_batch),
        None,
        None,
        true,
        Some(task_guest_read_pool),
    ) {
        Ok(staged)
            if matches!(
                staged.outcome,
                StagedOutcome::DeferredGuestWritesOnly(..)
                    | StagedOutcome::DeferredMixedColorAndTmem { .. }
            ) =>
        {
            Ok(TaskComputeDisposition::Compute(ComputeEligibleTaskMember(
                staged,
            )))
        }
        Ok(staged) => Ok(TaskComputeDisposition::Cpu {
            bound: staged.bound,
            reason: TaskComputeCpuReason::CompletionShapeNotDeferred,
        }),
        Err(StagedRawDpcFailure {
            bound,
            error: WgpuRawDpcExecutionError::TaskBatchComputeNotAdmitted { reason, .. },
        }) => Ok(TaskComputeDisposition::Cpu {
            bound,
            reason: TaskComputeCpuReason::ExactAdmissionRejected(reason),
        }),
        Err(failure) => Err(failure.error),
    }
}

fn complete_staged_raw_dpc_member<C: PhysicalExecutionCoordinator>(
    coordinator: &mut C,
    staged_member: StagedRawDpcMember,
    observe_task_envelope: bool,
    exact_physical_coverage: bool,
) -> Result<
    (
        BackendPreparedRawDpc,
        Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        Option<PendingFillPublication>,
        Option<Vec<TmemGpuProjection>>,
        Vec<ComputeRasterProbe>,
        Option<ComputeRasterProbeReceipt>,
        Option<u64>,
        Option<u64>,
    ),
    WgpuRawDpcExecutionError,
> {
    let StagedRawDpcMember {
        bound,
        outcome,
        triangles,
        draw_tmem,
        compute_probes,
        compute_replacement_receipt,
        execution_view_gross_ns,
    } = staged_member;
    let submission = bound.submission();
    let (completed, finalize_coordinator_ns) = raw_dpc_execute_census::timed_observed(
        raw_dpc_execute_census::Phase::Complete,
        observe_task_envelope,
        || -> Result<_, WgpuRawDpcExecutionError> {
            let mut pending = None;
            let prepared = match outcome {
                StagedOutcome::TmemLoads(effects, next_physical) => coordinator
                    .complete_execution(bound, effects, next_physical)
                    .map_err(WgpuRawDpcExecutionError::Coordinator)?,
                // Mechanical, not inferred: `StagedOutcome::NoPhysicalSuccessor` is only
                // ever produced when `stage_and_report` observed zero completed
                // TMEM loads AND at least one admitted triangle (§1c) -- a mixed
                // plan (loads + triangles) always takes the `TmemLoads` arm above,
                // never this one. `complete_execution_preserving_physical` itself
                // additionally, structurally rejects any packet whose journal
                // declares write accesses (its own internal
                // `BackendEffectReport::try_new(packet, Vec::new())` call fails
                // `validate_effects`'s access-count check if the journal expects
                // any writes) -- this branch selection and that internal
                // validation are two independent enforcements of the same
                // invariant, not one relying on the other.
                StagedOutcome::NoPhysicalSuccessor => coordinator
                    .complete_execution_preserving_physical(bound)
                    .map_err(WgpuRawDpcExecutionError::Coordinator)?,
                // A fill-only plan: real guest-visible writes, no physical-TMEM
                // successor. `complete_execution_preserving_physical` is not a legal
                // destination -- it builds its own *empty* effect report and would
                // reject this packet's nonempty write journal -- and
                // `complete_execution` has no `PhysicalTmemState` successor to
                // offer, because a color-target write never touches physical TMEM.
                //
                // The staged token is only recorded *after* the coordinator accepts
                // the completion: a rejected completion must leave
                // `pending_fill_publication` untouched, so a later `publish_raw_dpc`
                // can never redeem a fill whose submission never completed.
                StagedOutcome::GuestWritesOnly(effects, staged) => {
                    let prepared = coordinator
                        .complete_execution_preserving_physical_with_effects(bound, effects)
                        .map_err(WgpuRawDpcExecutionError::Coordinator)?;
                    pending = Some(PendingFillPublication {
                        submission,
                        color: PendingColorPublication::Full(staged.initialized),
                        prepared_sparse_checkpoint: staged.prepared_sparse_checkpoint,
                        guest_writes: staged.guest_writes,
                        cpu_phase_attributed: staged.cpu_phase_attributed,
                        exact_physical_coverage,
                    });
                    prepared
                }
                // Both sources in one packet. `complete_execution` -- the same arm
                // a TMEM-only packet takes -- because this packet genuinely has a
                // physical successor to install, and it is the only completion that
                // installs one. The fill token is recorded exactly as the fill-only
                // arm records it, and for the identical reason: only after the
                // coordinator has accepted the completion, so a rejected completion
                // leaves nothing a later `publish_raw_dpc` could redeem.
                //
                // The two publications this packet produces stay distinct and each
                // stays gated on its own acceptance -- the physical-TMEM successor
                // by `complete_execution`'s inactive-slot record, redeemed when
                // `prepare_publication`/`commit` flips the active slot, and the
                // resident color generation by this token, redeemed separately in
                // `publish_raw_dpc`. Composition merged the *write report*; it did
                // not merge the two publication identities.
                StagedOutcome::MixedFillAndTmemLoads(effects, next_physical, staged) => {
                    let prepared = coordinator
                        .complete_execution(bound, effects, next_physical)
                        .map_err(WgpuRawDpcExecutionError::Coordinator)?;
                    pending = Some(PendingFillPublication {
                        submission,
                        color: PendingColorPublication::Full(staged.initialized),
                        prepared_sparse_checkpoint: staged.prepared_sparse_checkpoint,
                        guest_writes: staged.guest_writes,
                        cpu_phase_attributed: staged.cpu_phase_attributed,
                        exact_physical_coverage,
                    });
                    prepared
                }
                StagedOutcome::DeferredGuestWritesOnly(..)
                | StagedOutcome::DeferredMixedColorAndTmem { .. } => unreachable!(
                    "ordinary raw-DPC execution cannot produce a deferred task-batch outcome"
                ),
            };
            Ok((prepared, pending))
        },
    );
    let (prepared, pending) = completed?;

    Ok((
        prepared,
        triangles,
        pending,
        draw_tmem,
        compute_probes,
        compute_replacement_receipt,
        execution_view_gross_ns,
        finalize_coordinator_ns,
    ))
}

fn execute_raw_dpc_inner<C: PhysicalExecutionCoordinator>(
    coordinator: &mut C,
    bound: BoundSubmittedRawDpc,
    carry_in: RawDpcCarryIn,
    color_targets: &mut Option<ColorTargetRegistry>,
    configured_target_extent: Option<TriangleTargetExtent>,
    project_gpu_tmem: bool,
    collect_compute_probe: bool,
    compute_replacement_enabled: bool,
    compute_replacement_pipeline: Option<&mut TrianglePipelineRenderer>,
    task_guest_read_pool: Option<&mut TaskGuestReadCapturePool>,
    ordered_cpu_color_batch: Option<&mut OrderedCpuColorBatch>,
    mut task_cpu_phase_census: Option<&mut task_cpu_phase_census::Task>,
) -> Result<
    (
        BackendPreparedRawDpc,
        Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        Option<PendingFillPublication>,
        Option<Vec<TmemGpuProjection>>,
        Vec<ComputeRasterProbe>,
        Option<ComputeRasterProbeReceipt>,
    ),
    WgpuRawDpcExecutionError,
> {
    let staged = stage_raw_dpc_member(
        coordinator,
        None,
        bound,
        carry_in,
        color_targets,
        configured_target_extent,
        project_gpu_tmem,
        collect_compute_probe,
        compute_replacement_enabled,
        compute_replacement_pipeline,
        None,
        ordered_cpu_color_batch,
        task_cpu_phase_census.as_deref_mut(),
        false,
        task_guest_read_pool,
    )
    .map_err(|failure| failure.error)?;
    let (prepared, triangles, pending, draw_tmem, probes, receipt, view_ns, complete_ns) =
        complete_staged_raw_dpc_member(coordinator, staged, task_cpu_phase_census.is_some(), true)?;
    let attributed = pending
        .as_ref()
        .is_some_and(|pending| pending.cpu_phase_attributed);
    if let Some(task) = task_cpu_phase_census {
        task.record_member_envelope(attributed, view_ns, complete_ns);
    }
    Ok((prepared, triangles, pending, draw_tmem, probes, receipt))
}

struct CompletedDeferredSegmentMember {
    prepared: BackendPreparedRawDpc,
    pending: PendingFillPublication,
    triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    draw_tmem: Option<Vec<TmemGpuProjection>>,
}

/// Move-only redemption of exactly one device image per requested packet
/// checkpoint. Construction closes cardinality before any candidate or guest
/// effect is mutated; callers can then consume images in order without a
/// truncating `zip` side channel.
#[must_use]
struct ExactCheckpointImages {
    images: std::vec::IntoIter<Vec<u8>>,
}

impl ExactCheckpointImages {
    fn try_new(images: Vec<Vec<u8>>, expected: usize) -> Result<Self, WgpuRawDpcExecutionError> {
        if images.len() != expected {
            return Err(WgpuRawDpcExecutionError::ComputeRasterCheckpointCount {
                expected,
                actual: images.len(),
            });
        }
        Ok(Self {
            images: images.into_iter(),
        })
    }

    fn take_next(&mut self) -> Vec<u8> {
        self.images
            .next()
            .expect("validated checkpoint cardinality has an image for every redemption")
    }

    fn finish(self) {
        assert_eq!(
            self.images.len(),
            0,
            "validated checkpoint cardinality is redeemed exactly once"
        );
    }
}

fn merge_deferred_packet_writes(
    expected: &[ResourceAccess],
    color_writes: &[CompletedWrite],
    tmem_writes: &[CompletedWrite],
) -> Result<Vec<CompletedWrite>, WgpuRawDpcExecutionError> {
    let mut staged: Vec<(CompletedWrite, bool)> = color_writes
        .iter()
        .chain(tmem_writes)
        .map(|write| (*write, false))
        .collect();
    let mut merged = Vec::with_capacity(staged.len());
    for declared in expected.iter().filter(|access| access.mode().writes()) {
        let claimed = staged
            .iter_mut()
            .find(|(write, taken)| !*taken && write.access() == *declared)
            .ok_or(WgpuRawDpcExecutionError::MergedWriteUnclaimed {
                access_index: declared.operation().get(),
            })?;
        claimed.1 = true;
        merged.push(claimed.0);
    }
    if let Some((write, _)) = staged.iter().find(|(_, taken)| !*taken) {
        return Err(WgpuRawDpcExecutionError::MergedWriteUndeclared {
            access_index: write.access().operation().get(),
        });
    }
    Ok(merged)
}

fn complete_deferred_compute_segment<C: PhysicalExecutionCoordinator>(
    coordinator: &mut C,
    pipeline: &mut TrianglePipelineRenderer,
    staged_members: Vec<StagedRawDpcMember>,
) -> Result<Vec<CompletedDeferredSegmentMember>, WgpuRawDpcExecutionError> {
    let first_member = staged_members
        .first()
        .expect("a deferred compute segment is non-empty");
    let first_color = match &first_member.outcome {
        StagedOutcome::DeferredGuestWritesOnly(_, color)
        | StagedOutcome::DeferredMixedColorAndTmem { color, .. } => color,
        _ => unreachable!("a deferred compute segment contains only deferred outcomes"),
    };
    let extent = first_color.plan.dispatches[0].extent;
    let key = first_color.candidate.key();
    let initial_bytes = first_color
        .initial_bytes
        .as_deref()
        .expect("a deferred compute segment head owns the durable target seed");
    let mut dispatches = Vec::new();
    let mut checkpoint_limits = Vec::with_capacity(staged_members.len());
    let mut prior_generation = first_color.candidate.predecessor();
    for (member_index, member) in staged_members.iter().enumerate() {
        let color = match &member.outcome {
            StagedOutcome::DeferredGuestWritesOnly(_, color)
            | StagedOutcome::DeferredMixedColorAndTmem { color, .. } => color,
            _ => unreachable!("a deferred compute segment contains only deferred outcomes"),
        };
        if member_index != 0 && color.initial_bytes.is_some() {
            return Err(
                WgpuRawDpcExecutionError::ComputeRasterProbeChainIncompatible {
                    ordinal: member.bound.ordinal(),
                    batch: member_index,
                },
            );
        }
        if color.candidate.key() != key
            || color
                .plan
                .dispatches
                .iter()
                .any(|dispatch| dispatch.extent != extent)
            || color.candidate.predecessor() != prior_generation
        {
            return Err(
                WgpuRawDpcExecutionError::ComputeRasterProbeChainIncompatible {
                    ordinal: member.bound.ordinal(),
                    batch: member_index,
                },
            );
        }
        prior_generation = Some(color.candidate.generation());
        for dispatch in &color.plan.dispatches {
            let accesses: Vec<_> = dispatch
                .batch
                .draws()
                .iter()
                .flat_map(ComputeRasterDrawAdmission::accesses)
                .copied()
                .collect();
            let first_triangle_index = dispatch
                .batch
                .draws()
                .first()
                .expect("a sealed compute dispatch has an admitted draw")
                .triangle_index();
            let claimed = claimed_rectangle_from_accesses(key, &accesses, first_triangle_index)?;
            let target_width = key.extent().width();
            let (first_column, column_count) = if compute_column_bounds_enabled() {
                let first = claimed.x() & !1;
                let limit = claimed
                    .x()
                    .checked_add(claimed.width())
                    .expect("claimed rectangle was checked when constructed")
                    .checked_add(1)
                    .map(|limit| limit & !1)
                    .unwrap_or(target_width)
                    .min(target_width);
                (first, limit - first)
            } else {
                (0, target_width)
            };
            dispatches.push(ComputeHotColorDispatch {
                triangles: &dispatch.triangles,
                tmem: &dispatch.tmem,
                tile: dispatch.tile,
                first_row: claimed.y(),
                row_count: claimed.height(),
                first_column,
                column_count,
            });
        }
        checkpoint_limits.push(dispatches.len());
    }
    let outputs = pipeline
        .compute_triangle_hot_color_chain_checkpoints(
            extent,
            initial_bytes,
            &dispatches,
            &checkpoint_limits,
        )
        .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
    let mut outputs = ExactCheckpointImages::try_new(outputs, staged_members.len())?;

    let mut completed_members = Vec::with_capacity(staged_members.len());
    for mut member in staged_members {
        let output = outputs.take_next();
        let submission = member.bound.submission();
        let outcome = core::mem::replace(&mut member.outcome, StagedOutcome::NoPhysicalSuccessor);
        let (deferred_effects, deferred_tmem, color) = match outcome {
            StagedOutcome::DeferredGuestWritesOnly(effects, color) => (effects, None, color),
            StagedOutcome::DeferredMixedColorAndTmem {
                effects,
                tmem,
                color,
            } => (effects, Some(tmem), color),
            _ => unreachable!("a deferred compute segment contains only deferred outcomes"),
        };
        let device_bytes = crate::DeviceColorBytes::new_for_fill(
            key,
            color.candidate.generation(),
            key.format(),
            output,
        )?;
        let completed = CompletedColorTargetWrite::new_for_fill(
            key,
            color.candidate.generation(),
            key.range(),
            color.plan.claimed,
            device_bytes,
        );
        let guest_writes =
            fill_completed_writes(key, completed.device_bytes(), &color.plan.declared)?;
        let initialized = color.candidate.admit_completed_initialization(completed)?;
        let writes = if let Some(tmem) = deferred_tmem.as_ref() {
            merge_deferred_packet_writes(
                deferred_effects.expected_writes(),
                &guest_writes,
                tmem.proposed_effects(),
            )?
        } else {
            guest_writes.clone()
        };
        let effects = deferred_effects
            .complete(writes)
            .map_err(WgpuRawDpcExecutionError::Effect)?;
        member.outcome = if let Some(tmem) = deferred_tmem {
            let next_physical = tmem
                .complete(&effects)
                .map_err(WgpuRawDpcExecutionError::Physical)?;
            StagedOutcome::MixedFillAndTmemLoads(
                effects,
                next_physical,
                StagedFill {
                    initialized,
                    guest_writes,
                    prepared_sparse_checkpoint: None,
                    cpu_phase_attributed: false,
                },
            )
        } else {
            StagedOutcome::GuestWritesOnly(
                effects,
                StagedFill {
                    initialized,
                    guest_writes,
                    prepared_sparse_checkpoint: None,
                    cpu_phase_attributed: false,
                },
            )
        };
        let (prepared, _triangles, pending, _draw_tmem, _, _, _, _) =
            complete_staged_raw_dpc_member(coordinator, member, false, false)?;
        let pending = pending.expect("a deferred color completion produces a publication token");
        assert_eq!(pending.submission, submission);
        completed_members.push(CompletedDeferredSegmentMember {
            prepared,
            pending,
            // The compute checkpoint is this packet's authoritative color
            // execution. Re-submitting the same triangles through the
            // diagnostic attachment path would add one wait per packet and
            // cannot affect guest RDRAM or VI presentation.
            triangles: Vec::new(),
            draw_tmem: None,
        });
    }
    outputs.finish();
    Ok(completed_members)
}

/// The pipeline `submitted_packet` runs once `&WorkloadPacket` is in scope:
/// stage every ordered TMEM load into one packet-local transaction via
/// `PhysicalTmemState::stage_neutral_transfer` (T3 Phase B's own neutral
/// counterpart to the decoder-typed `stage_transfer`), seal it into a
/// `PendingTmemTransaction`, compute the exact `BackendEffectReport` from
/// its own proposed effects, and derive this transaction's
/// `into_physical_successor` (T3 Phase A) candidate. Returns
/// `StagedOutcome::NoPhysicalSuccessor` instead of staging anything when
/// the plan has zero TMEM loads but still carries a completable command --
/// an admitted triangle (§1c) or a `SYNC_FULL` site -- and
/// `NoCompletedLoads` only when the plan carries none of the three.
fn stage_and_report(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<StagedOutcome, WgpuRawDpcExecutionError> {
    // **A fill composed with a raw triangle is admitted, and the ordering
    // the old refusal asked for already exists.**
    //
    // This was `MixedFillAndTrianglePacket`, reading "the CPU-side fill and
    // the GPU triangle raster target are disjoint with no defined
    // composition or ordering between them". That sentence described the
    // code at the commit that wrote it. It does not describe this file any
    // more, and measuring WM2000's own packet is what forced the re-read.
    //
    // **The measured packet.** Instrumented at this site and run on the
    // real ROM through the all-Rust lane (`FN64_RECOMP=rs`,
    // `FN64_RENDER=wgpu`, two controllers, the committed match lead-in),
    // WM2000 raises this refusal at VI swap 2873 with the shape recorded in
    // `docs/WM2000-FILL-TRIANGLE-EVIDENCE.txt`.
    //
    // **Why no composition is missing.** Every colour-target-writing
    // command in a packet -- `FillRectangle`, `TextureRectangle`, and a
    // flat `RawTriangle` that declared a write -- is executed by
    // `stage_color_commands` against ONE shared full-extent accumulation
    // buffer, in the packet's own stream order, recovered by sorting on the
    // decoder's `command_index`. `ColorCommandKind` has an arm for each of
    // the three; each arm's output becomes the next command's resident
    // bytes ("the single line that makes N compose"); and every declared
    // access's digest is computed once at the end against the final composed
    // buffer. `targets/raw_triangle.rs`'s own module doc states the point
    // directly: it is a CPU rasterizer "producing the same
    // `CompletedColorTargetWrite` the fill and texrect executors produce",
    // chosen over the GPU path precisely because "the guest-visible path has
    // no GPU in it".
    //
    // So the fill is not CPU-side while the triangle is GPU-side. They are
    // both CPU-side, in one buffer, in journal order -- the identical seam
    // the fill+texrect pair has composed through since the N-command
    // accumulation landed, and which `merged_fill_and_tmem_writes` then
    // re-derives from the resource journal independently.
    //
    // **A raw triangle that declared NO write is also fine, for the reason
    // the texrect sibling below already establishes at length.** Such a
    // triangle contributes no `ResourceAccess`, stages no `CompletedWrite`,
    // and reaches only `draw_admitted_triangles` -- whose output `present`
    // refuses to scan out by name and which nothing copies into guest
    // RDRAM. The effect report is byte-for-byte the one the same packet
    // without the triangle produces. That was already true, and already
    // admitted, when the other command in the packet was a texrect; a fill
    // does not make it less true.
    //
    // **What still refuses, and it is not a shape gate.** The invariant
    // that actually protects this seam is journal exactness, checked per
    // access rather than per packet shape: `merged_fill_and_tmem_writes`
    // refuses `MergedWriteUnclaimed` when the journal declares a write no
    // source staged and `MergedWriteUndeclared` when a source staged a write
    // the journal never declared, and `fill_completed_writes` refuses
    // `FillAccessOutsideTarget` for any declared range outside the target.
    // Those are kept and are what the retargeted tests now pin. A shape gate
    // could only ever approximate them, and here it approximated them by
    // dropping six real guest-visible commands to withhold one that was
    // never going to be visible either way.
    //
    // **What is NOT claimed.** Admitting this packet does not make the GPU
    // triangle raster guest-visible; that writeback gap is separate,
    // pre-existing, and documented in `docs/RT64-TRIANGLE-WRITEBACK.md`.
    // **A texrect composed with a raw triangle is admitted, and the
    // ordering the refusal asked for already exists.**
    //
    // This was a `MixedTexrectAndRawTrianglePacket` refusal reading "the
    // texrect composes onto the CPU-side color buffer through its own
    // declared journal writes while a raw triangle declares no write access
    // at all, so the two have no defined ordering". Both halves of that
    // sentence are true. The conclusion does not follow, and measuring the
    // packet is what showed why.
    //
    // **The measured packet.** Instrumented at this site and run on the real
    // ROM through the all-Rust lane (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`),
    // the packet WM2000 aborts on is **6 texrects, 9 TMEM loads, 1 raw
    // triangle, 0 fills** -- and the raw triangle is **strictly last**, at
    // wire command 91, after every texrect (commands 2, 10, 18, 26, 34, 42)
    // and every load (7, 15, 23, 31, 39, 54, 58, 81, 88). There is no
    // interleaving to order: the shape is "all the texrects, then one
    // triangle".
    //
    // **Why no ordering is missing.** The two sources do not write the same
    // thing, and only one of them writes anything the guest can observe:
    //
    // - A texrect's pixels reach the guest through `stage_color_commands`'s
    //   accumulation buffer, then `ColorTargetRegistry`, then `fn64-abi`'s
    //   `copy_committed_guest_writes` -- gated on the journal's declared
    //   `ColorFramebuffer` writes.
    // - A raw triangle's raster reaches `triangle_draw_output`, which
    //   `last_triangle_draw` describes as "the most recent triangle draw's
    //   real GPU-observed color/depth output ... never an accumulated
    //   history, never a persistent framebuffer", and which `present`
    //   refuses to scan out by name: it is "one submission's readback, not a
    //   VI-sampled framebuffer". **Nothing copies it into guest RDRAM.**
    //
    // So "the two have no defined ordering" describes a composition that is
    // not attempted. There is exactly one guest-visible destination in this
    // packet and exactly one source writing to it, and the order among
    // *those* writes is already derived -- `stage_color_commands` sorts the
    // schedule on the decoder's own `command_index`, and
    // `merged_fill_and_tmem_writes` independently re-derives the same order
    // from the resource journal.
    //
    // **Admitting the triangle adds nothing for the journal to order.** A
    // `RawTriangle` pushes no `ResourceAccess` at all (the decoder's
    // `0x08..=0x0f` arm decodes the triangle and pushes the command; unlike
    // `FILL_RECTANGLE` and `TEXRECT` it calls no planner). It therefore
    // contributes neither a declared journal write nor a staged
    // `CompletedWrite`, and `merged_fill_and_tmem_writes`'s two-sided
    // exactness check -- every declared write claimed exactly once, every
    // staged write claimed -- sees the identical pair of lists it would see
    // with the triangle absent. The effect report this packet produces is
    // byte-for-byte the one the same packet minus its last command produces.
    //
    // **The fill+triangle sibling reached the same conclusion, later, from
    // its own measurement.** This paragraph used to end "that one stays
    // refused", on the reasoning that the fill+raw-triangle pair had never
    // been measured in WM2000's stream so admitting it would be widening on
    // inference. It has now been measured -- VI swap 2873, 60 fill-cycle
    // fills and one raw triangle that DECLARED a five-row write -- and the
    // refusal was removed for the reasons recorded above this function's
    // first paragraph. The two cases were never actually different: both
    // reduce to "the GPU raster is guest-invisible for every packet shape,
    // and every declared write is journal-ordered regardless of which
    // command produced it".
    //
    // **What is NOT claimed.** Admitting this packet does not make the raw
    // triangle guest-visible. It is not visible today in a triangle-only
    // packet either: `StagedOutcome::NoPhysicalSuccessor`'s own doc already
    // records that "a raw triangle rasters into a GPU attachment and
    // declares no journal write", and that packet shape is admitted. The
    // missing RDRAM writeback for the GPU raster path is a separate,
    // pre-existing gap that this arm never closed and could not have
    // closed by refusing -- refusing dropped the texrects too, which DO
    // reach the guest. The refusal cost six real guest-visible rectangles
    // to withhold one triangle that was never going to be visible either
    // way.
    //
    // **Per-triangle TMEM still resolves correctly for the raw triangle.**
    // `project_pending_tmem_per_triangle` walks `plan.triangle_commands`
    // and selects each entry with `prefix_before`, which is a fact about
    // stream position and not about triangle source. The raw triangle at
    // command 91 therefore samples the prefix sealed by the load at command
    // 88 -- the last load before it -- exactly as a texrect in that position
    // would. No arm of that function distinguishes the two sources, so
    // nothing there needed widening.
    // **A texrect in a packet with no TMEM load samples durable committed
    // TMEM, and that is the RDP's own semantics, not a fallback.**
    //
    // This was a `TexrectWithoutTmemLoad` refusal, justified by a census
    // reading "0 of WM2000's 219 decode entries carry a texrect without a
    // load in the same entry". The count was correct; the window was not.
    // That census covered 219 decode entries of boot/attract and was
    // superseded twice in its own doc (383 -> 1,056 -> 4,454 VI fields,
    // 219 -> 2,219 -> 5,792 entries). Re-measured on the real ROM through
    // the shell's `FN64_RENDER=wgpu` seam, the fourth packet WM2000 issues
    // is **46 texrects, 0 loads, 0 fills** -- the refused shape, from the
    // game, one packet after the packet whose 4 loads filled the tiles it
    // samples.
    //
    // The refusal's own worry -- "silently sampling stale committed state"
    // -- does not apply to the state actually read here. TMEM is durable
    // RDP hardware state; committed `PhysicalTmemState` is not a stale
    // approximation of a proposal, it is the published result of every
    // earlier packet's loads, which is the only thing a load-free packet's
    // texrect could observe on hardware. The guard against inventing texels
    // is preserved and strengthened rather than dropped: the read goes
    // through the same single `sample_point`/`read_texel` path a pending
    // read uses, an invalid TMEM byte is still a named refusal from that
    // reader rather than a zero, and `CommittedTmemImageClaimedProposed`
    // now checks the identity crossing in the direction this arm requires.
    //
    // **Within-packet ordering is semantics, and this is where it is
    // honoured.**
    //
    // The RDP's TMEM is durable *within* a packet exactly as it is across
    // packets: a texture rectangle samples whatever TMEM holds at its own
    // stream position, which is the result of every load BEFORE it and no
    // load after it. A single post-image sealed from all of a packet's
    // loads answers that question with the future's data.
    //
    // That was measured, not hypothesised. With no ordering gate at all, a
    // stream whose texrect precedes its `LoadBlock` executed and produced
    // byte-identical texrect rows to the correctly-ordered stream (the same
    // three `CompletedWrite` content digests) -- ordering was not semantics,
    // it was ignored. A `TexrectBeforeItsOwnLoad` refusal named that defect
    // honestly while there was no per-position image to serve.
    //
    // ## The shape WM2000 actually emits, measured
    //
    // Dumped from the real ROM on the all-Rust stack (`FN64_RECOMP=rs` +
    // `FN64_RENDER=wgpu`), the sixth packet WM2000 issues is a **sprite
    // strip**: one TLUT load followed by seven `LoadTile`/texrect PAIRS in
    // strict alternation.
    //
    // ```text
    // cmd 33  LoadTLUT  tile 7  TMEM 2048..2176
    // cmd 39  LoadTile  tile 7  TMEM    0..1576   (49 source rows)
    // cmd 42  TEXRECT   tile 0
    // cmd 47  LoadTile  tile 7  TMEM    0..1960
    // cmd 50  TEXRECT
    // cmd 55  LoadTile  tile 7  TMEM    0..1960
    // cmd 58  TEXRECT
    // cmd 63  LoadTile  tile 7  TMEM    0..1960
    // cmd 66  TEXRECT
    // cmd 71  LoadTile  tile 7  TMEM    0..1576
    // cmd 74  TEXRECT
    // cmd 79  LoadTile  tile 7  TMEM    0..1608
    // cmd 82  TEXRECT
    // cmd 87  LoadTile  tile 7  TMEM    0..2000
    // cmd 90  TEXRECT
    // ```
    //
    // Every one of the seven `LoadTile`s writes the SAME TMEM range from
    // word 0. They overwrite each other, so a once-per-packet post-image is
    // not merely too coarse here -- it is maximally wrong: it holds only the
    // SEVENTH load's texels, and all seven texrects would draw the seventh
    // sprite. Refusing the packet was correct while that was the only
    // alternative; serving each texrect its own position is correct
    // outright, and is what the loop below does.
    //
    // ## Which load each texrect observes, derived from those positions
    //
    // `prefix_before` selects the last load whose command index is strictly
    // below the texrect's, so the seven pairs map one-to-one and in order:
    //
    // ```text
    // texrect 42 -> load at cmd 39 (TMEM    0..1576)
    // texrect 50 -> load at cmd 47 (TMEM    0..1960)
    // texrect 58 -> load at cmd 55 (TMEM    0..1960)
    // texrect 66 -> load at cmd 63 (TMEM    0..1960)
    // texrect 74 -> load at cmd 71 (TMEM    0..1576)
    // texrect 82 -> load at cmd 79 (TMEM    0..1608)
    // texrect 90 -> load at cmd 87 (TMEM    0..2000)
    // ```
    //
    // Under a once-per-packet seal every one of those right-hand entries is
    // instead cmd 87's load -- the same image seven times, which is the
    // defect stated as a table.
    //
    // The TLUT at cmd 33 is the one load NO texrect selects, and correctly
    // so: it is not the last load below any of them. It is also not lost.
    // It writes TMEM 2048..2176, disjoint from the sprite range at word 0,
    // so every prefix from cmd 39 onward still carries it -- a prefix is
    // cumulative TMEM state, not one load's footprint. All seven texrects
    // therefore share one palette and differ only in their texels, which is
    // exactly what a sprite strip is.
    //
    // `wm2000_sixth_packet_positions_map_each_texrect_to_the_load_before_it`
    // pins this table against `prefix_before` itself.
    //
    // ## Per-position views, not per-position transactions
    //
    // The seal stays **once per packet**, and every structure that assumes
    // one post-image per packet is untouched:
    //
    // 1. `into_pending` still runs exactly once, still consuming the
    //    `PhysicalTmemPacketTransaction` by value and still requiring
    //    access-for-access coverage of EVERY journal `TmemLoadDestination`
    //    write. Nothing seals after load 1 of 8.
    // 2. `PhysicalTmemBinding`'s single `next_generation` is claimed once,
    //    by that one seal. A prefix claims no generation.
    // 3. `proposal_identity` is computed once over the whole projection and
    //    effect list. A prefix read reports it verbatim; the move-only sealed
    //    transaction prevents later mutation, while the diagnostic audit can
    //    recompute it when explicitly armed.
    // 4. The TMEM loop and `stage_color_commands` remain sequential PHASES.
    //    `capture_prefix` copies `bytes`/`valid` out during the load loop --
    //    it is a read, it cannot fail, and it touches no registry -- so
    //    color staging still runs strictly last and a TMEM rejection still
    //    leaves no color token in existence.
    //
    // What made this small is that `finish_load` returns the packet
    // transaction BY VALUE with `bytes`/`valid` already current for every
    // load so far, so a prefix is a copy of arrays that already exist; and
    // `execute_scheduled_texrect` is already generic over `TmemByteSource`,
    // so the per-position image is served through the one sampler rather
    // than a parallel reader.
    //
    // The GPU half is unchanged and remains one `TmemGpuProjection` per
    // `draw_admitted_triangles` call: it projects the sealed post-image,
    // which is what a triangle drawn after the whole packet observes.
    // A color-target command with no TMEM load keeps its own dedicated
    // path: the packet has no physical successor to offer, so it must not
    // reach `complete_execution`.
    //
    // Reached by a texrect as well as a fill. Both write the same resource
    // through the same accumulation seam and neither stages a TMEM
    // transaction, so the routing question is "did this packet complete a
    // load", not "which command wrote". Gating on `fills` alone sent a
    // load-free texrect packet down the transaction path, where
    // `into_pending` has nothing to seal.
    //
    // **Only a texrect that DECLARED a write counts.** The decoder emits no
    // `ColorFramebuffer` access for a texrect with no staged
    // `SetColorImage` (`raw_dpc::plan_texture_rectangle` returns early), and
    // such a packet has no color target to compose into at all -- it
    // rasters through the GPU triangle path and nothing else. Routing it
    // here on the mere presence of a texrect would ask `color_target_key`
    // for a key that cannot exist and refuse a packet the decoder
    // deliberately admitted. Read off the decoder's own recorded span, not
    // re-derived.
    let writing_texrect_count = collector
        .plan
        .texrect_commands
        .iter()
        .filter(|(span, _, _, _, _)| span.is_some())
        .count();
    // **A flat raw triangle that declared a write is a colour-target
    // command, and routes exactly like a texrect that declared one.**
    //
    // `raw_triangle_commands` holds ONLY the triangles whose span is
    // `Some`, so its length is already the "declared a write" count -- no
    // filter needed, and no way to mistake "present" for "declared".
    //
    // Missing this line was measured, not reasoned: a triangle-only packet
    // declared its three per-row journal writes and then took the
    // no-colour-command branch, so `stage_color_commands` never ran and the
    // backend produced zero effects against a journal requiring three
    // ("backend effect count is 0; exact journal requires 3"). The journal
    // caught it, which is the design working -- but the packet was refused
    // rather than drawn.
    let writing_raw_triangle_count = collector.plan.raw_triangle_commands.len();
    let cpu_phase_attributed = collector.task_cpu_phase_census.is_some()
        && ordered_depth_free_acff_triangle_member(collector);
    if (!collector.plan.fills.is_empty()
        || writing_texrect_count > 0
        || writing_raw_triangle_count > 0)
        && collector.plan.loads.is_empty()
    {
        return stage_fills_and_report(collector, packet);
    }

    let source = TmemLoadSourceIdentity::new(
        packet.identity(),
        packet.journal().identity(),
        collector.submission,
        packet.memory_layout(),
    );
    let sequence = transaction_sequence(packet);

    let mut packet_transaction: Option<PhysicalTmemPacketTransaction> = None;
    // **TMEM as of each load, keyed by the load's own stream position.**
    //
    // Appended in load order, which is stream order (`plan.loads` is filled
    // by the decoder's single stream walk), so a texrect at command C reads
    // the LAST entry whose command index is below C -- see
    // `tmem_source_for_command`.
    let mut prefixes: Vec<(u32, crate::tmem::TmemPrefixSnapshot)> = Vec::new();
    let mut expected_destination_elements = 0u64;

    for (command_index, load) in collector.plan.loads.iter() {
        let command_index = *command_index;
        let load_started = task_cpu_phase_census::started(
            collector.task_cpu_phase_census.as_deref(),
            cpu_phase_attributed,
        );

        let (destination_accesses, source_accesses) = task_cpu_phase_census::timed_source(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::SourceSubphase::LoadAccessBind,
            || {
                let destination_accesses = destination_access_run(
                    &collector.plan.accesses,
                    load.destination_access_index() as usize,
                );
                let source_accesses = load.sources();
                (destination_accesses, source_accesses)
            },
        );
        if destination_accesses.is_empty() {
            return Err(WgpuRawDpcExecutionError::MalformedDestinationAccessRun { command_index });
        }
        let first_load = packet_transaction.is_none();
        task_cpu_phase_census::record_source_counters(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            || {
                let destination_access_count =
                    u64::try_from(destination_accesses.len()).unwrap_or(u64::MAX);
                expected_destination_elements =
                    expected_destination_elements.saturating_add(destination_access_count);
                task_cpu_phase_census::SourceCounters {
                    loads: 1,
                    source_fragments: u64::try_from(source_accesses.len()).unwrap_or(u64::MAX),
                    words: u64::try_from(load.transfer_words().len()).unwrap_or(u64::MAX),
                    destination_accesses: destination_access_count,
                    first_loads: u64::from(first_load),
                    cumulative_expected_destination_elements: expected_destination_elements,
                    projected_destination_bytes: destination_accesses.iter().fold(
                        0u64,
                        |total, access| {
                            total.saturating_add(u64::from(access.region().declared_bytes()))
                        },
                    ),
                }
            },
        );
        let mut staged = task_cpu_phase_census::timed_source(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::SourceSubphase::TransactionBegin,
            || match packet_transaction.take() {
                None => collector
                    .physical
                    .stage_neutral_transfer(
                        source,
                        collector.queue,
                        collector.ordinal,
                        sequence,
                        load,
                        destination_accesses,
                    )
                    .map_err(WgpuRawDpcExecutionError::Physical),
                Some(packet) => packet
                    .stage_neutral_transfer_next(source, load, destination_accesses)
                    .map_err(WgpuRawDpcExecutionError::Physical),
            },
        )?;

        task_cpu_phase_census::timed_source(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::SourceSubphase::WordStageAndBlockValidity,
            || -> Result<(), WgpuRawDpcExecutionError> {
                let tracks_block_footprint = matches!(load.shape(), TmemLoadShape::Block);
                let mut block_footprint: Option<(u16, u16)> = None;
                while let Some(word) = staged.next_expected_word() {
                    if tracks_block_footprint {
                        if let crate::tmem::TmemTransferPhysicalWord::Linear(_) = word.physical() {
                            let destination = word.destination_word();
                            block_footprint = Some(match block_footprint {
                                None => (destination, destination),
                                Some((low, high)) => (low.min(destination), high.max(destination)),
                            });
                        }
                    }
                    let bytes = word_source_bytes(
                        &collector.reads,
                        source_accesses,
                        load.source_access_index(),
                        word,
                    )
                    .ok_or(WgpuRawDpcExecutionError::MissingCapturedSource { command_index })?;
                    let physical_lanes = map_physical_lanes(load, word, bytes)
                        .map_err(WgpuRawDpcExecutionError::Physical)?;
                    staged
                        .stage_next_physical_lanes(physical_lanes)
                        .map_err(WgpuRawDpcExecutionError::Physical)?;
                }

                // A LoadBlock with DXT >= 0x800 advances its destination by more than
                // one TMEM word per source word -- the row advance for tile rows
                // >= 1 -- so it writes scattered words (e.g. DXT=0x800 -> words 0, 2,
                // 4, 6) and skips the words between them. Hardware and both oracles
                // (RT64, angrylion) read those skipped words back as their prior,
                // zero-initialised content; only fn64's validity tracking would
                // otherwise refuse a render tile that re-describes the block with a
                // `line` reading a skipped word. The sweep is contiguous, so mark
                // every word in `[low, high]` valid. Scoped to Block: a LoadTile
                // that reads outside its own footprint still refuses (its interior
                // has no such sweep gaps), keeping the WM2000 origin-term guard.
                if let Some((low_word, high_word)) = block_footprint {
                    staged.mark_block_footprint_valid(low_word, high_word);
                }
                Ok(())
            },
        )?;

        let finished = task_cpu_phase_census::timed_source(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::SourceSubphase::FinishProjectEffect,
            || {
                staged
                    .finish_load()
                    .map_err(WgpuRawDpcExecutionError::Physical)
            },
        )?;
        task_cpu_phase_census::record_started(
            collector.task_cpu_phase_census.as_deref_mut(),
            task_cpu_phase_census::Phase::SourceBindingLoad,
            load_started,
        );
        // Taken after THIS load and before the next stages anything, so the
        // snapshot is exactly what TMEM holds at this command's position.
        // A read of arrays that already exist: it cannot fail and touches
        // no registry, so the load loop stays a pure phase.
        let prefix = task_cpu_phase_census::timed(
            collector.task_cpu_phase_census.as_deref_mut(),
            cpu_phase_attributed,
            task_cpu_phase_census::Phase::PrefixCapture,
            || finished.capture_prefix(crate::tmem::TmemLoadStreamPosition::new(command_index)),
        );
        prefixes.push((command_index, prefix));
        packet_transaction = Some(finished);
    }

    let packet_transaction = match packet_transaction {
        Some(packet_transaction) => packet_transaction,
        None => {
            // No TMEM load completed a transaction -- mechanically
            // distinguish "this plan still carries a completable command"
            // (§1c: route to the coordinator's preserving-physical
            // completion, never to `complete_execution`, which has no
            // successor to offer for an empty transaction) from "this plan
            // carries no command at all" (still `NoCompletedLoads`).
            //
            // **A `SYNC_FULL` site is such a command, and refusing it was
            // this guard's defect.** Measured on the real ROM through the
            // all-Rust lane (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`), WM2000
            // aborts here on a packet of exactly one wire command --
            // `wire_opcode = 0xE9` (`G_RDPFULLSYNC`), raw words
            // `[0xE9000000, 0x07000000]` -- with zero loads, triangles,
            // texrects and fills, and a single `ResourceAccess`:
            // `Read`/`CommandDecode` over the 8 `RspDmem` bytes of the sync
            // command itself. Its site carried `dp_slot_reserved: true`, so
            // `plan_raw_dpc` deliberately admitted it and then execution
            // refused it.
            //
            // `PlanCollector`'s own `FullSyncSite` arm already states the
            // semantics this branch now honours: the site is "collected,
            // not executed ... retained so the executed plan still accounts
            // for every command the plan carried". Dropping the packet is
            // the failure mode that arm calls out in the other direction.
            //
            // This is not a widening. The zero-write property is *proved*
            // at the destination, not assumed here: `SYNC_FULL` declares no
            // `ResourceAccess` by construction (`RdpFullSyncSite`: "a sync
            // reads and writes no resource"), and
            // `complete_execution_preserving_physical` independently
            // rechecks an explicitly empty write list against the packet's
            // real journal, rejecting any write-bearing packet with
            // `EffectCountMismatch` regardless of how it got routed.
            //
            // A plan carrying literally nothing -- no load, no triangle, no
            // sync -- is still `NoCompletedLoads`: there is no command whose
            // completion this backend could account for.
            return if collector.plan.triangles.is_empty()
                && collector.plan.full_sync_sites.is_empty()
            {
                Err(WgpuRawDpcExecutionError::NoCompletedLoads)
            } else {
                Ok(StagedOutcome::NoPhysicalSuccessor)
            };
        }
    };
    let pending = packet_transaction
        .into_pending()
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    let tmem_writes: Vec<CompletedWrite> = pending.proposed_effects().to_vec();

    // **The GPU half's TMEM images, one per triangle, selected by the SAME
    // `prefix_before` rule the CPU texel reader uses.**
    //
    // Per triangle, not per packet, for the reason the CPU side is per
    // texrect: within one packet TMEM is not one image. A single shared
    // projection holds only the last load's texels, so WM2000's seven
    // interleaved LoadTile/texrect pairs would raster the seventh sprite
    // seven times.
    //
    // Projected here, not in `draw_admitted_triangles`, because here is the
    // only place the sealed transaction exists: it is move-only and
    // `into_physical_successor` consumes it a few lines below, strictly
    // before the triangles are drawn. Projecting the *published* slot at
    // draw time instead -- what this call replaces -- reads a state that
    // does not yet contain this packet's own load, which is exactly the
    // defect: a texrect whose combine references `TEXEL0` sampled invalid
    // bytes and the fragment shader reported
    // `TMEM_SAMPLE_STATUS_INVALID_BYTE`.
    if collector.project_gpu_tmem {
        collector.draw_tmem = Some(project_pending_tmem_per_triangle(
            &collector.plan.triangle_commands,
            &prefixes,
            &pending,
            collector.physical,
        )?);
    }

    // **The color-target half: every admitted fill and texrect in this
    // packet, accumulated into one buffer in the packet's own command
    // order.**
    //
    // Staged only now -- after every TMEM load in the packet has staged
    // successfully. A color command that executed before a TMEM load
    // failed would leave an `InitializedCandidateColorTarget` built
    // against a registry generation this packet is about to abandon;
    // staging last means a TMEM rejection returns `Err` with no token in
    // existence at all, which is the same "nothing published" outcome the
    // fill-only path reaches by never storing the token on the error path.
    let staged_fill = raw_dpc_execute_census::timed(raw_dpc_execute_census::Phase::Color, || {
        stage_color_commands(
            collector,
            packet,
            TexrectTmemSource::Pending {
                pending: &pending,
                prefixes: &prefixes,
            },
        )
    })?;

    let Some(staged_fill) = staged_fill else {
        if let Some(color) = collector.deferred_compute.take() {
            let effects = BackendEffectReport::defer(packet);
            let tmem = pending
                .defer_physical_successor(collector.physical, effects.expected_writes())
                .map_err(WgpuRawDpcExecutionError::Physical)?;
            return Ok(StagedOutcome::DeferredMixedColorAndTmem {
                effects,
                tmem,
                color,
            });
        }
        let effects = BackendEffectReport::try_new(packet, tmem_writes)
            .map_err(WgpuRawDpcExecutionError::Effect)?;
        let next_physical = pending
            .into_physical_successor(collector.physical, &effects)
            .map_err(WgpuRawDpcExecutionError::Physical)?;
        return Ok(StagedOutcome::TmemLoads(effects, next_physical));
    };

    // All three sources' writes go into one claim pool. The journal still
    // decides the order, position by position, exactly as before -- adding
    // a third source changed what is available to claim, not who chooses.
    // `staged_fill.guest_writes` carries BOTH the fill's and the texrect's
    // runs (see the match above), so one list is offered to the merge --
    // chaining the texrect's again would offer each of its writes twice and
    // let a journal declaring one claim a duplicate.
    let merged = merged_fill_and_tmem_writes(packet, &staged_fill.guest_writes, &tmem_writes)?;
    let effects =
        BackendEffectReport::try_new(packet, merged).map_err(WgpuRawDpcExecutionError::Effect)?;

    // The TMEM half still vouches for exactly its own proposed writes, and
    // for them alone: `into_physical_successor`'s `validate_backend_effects`
    // walks the merged report as an order-preserving SUPERSEQUENCE of
    // `tmem_writes`, so the fill's interleaved `RenderTarget` writes are
    // neither vouched for by this transaction nor treated as its omission.
    // A merged list missing a TMEM write, reordering two of them, or
    // carrying wrong TMEM content is still rejected by name there.
    let next_physical = pending
        .into_physical_successor(collector.physical, &effects)
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    Ok(StagedOutcome::MixedFillAndTmemLoads(
        effects,
        next_physical,
        staged_fill,
    ))
}

/// Merge one packet's fill writes and TMEM writes into the single ordered
/// list `BackendEffectReport::try_new` requires -- **in the packet's own
/// resource-journal order, which is neither source's order and is not a
/// choice this function makes.**
///
/// `fn64_render_ir`'s `validate_effects` compares the supplied list against
/// `journal().write_accesses()` position by position, so the correct order
/// is fully determined by the journal and any other order is a named
/// rejection rather than a different-but-valid answer. The journal in turn
/// is the decoder's own `planned` vector: `raw_dpc::push_access` assigns
/// each access an `OperationId` equal to its index in that vector, and
/// `plan_fill` and the TMEM command decoder append into that one vector as
/// the command stream is walked. So journal order **is** RDP command order,
/// and this function recovers the interleaving rather than inventing one.
///
/// Implemented as a lookup keyed on the exact `ResourceAccess`, driven by
/// the journal: every declared write access is resolved to the one staged
/// `CompletedWrite` that claims it. That makes the ordering a *derivation*
/// from the journal, not a merge policy -- a sort key, a concatenation, or
/// an `OperationId` comparison would each be a second, independent model of
/// the same fact, and could drift from it.
///
/// Every declared write must be claimed exactly once, and every staged
/// write must be claimed: an unclaimed staged write would mean this backend
/// executed something the journal never declared, and an unclaimed journal
/// access would mean it declared something no source produced. Both are
/// named errors, never a short list that `try_new` would then reject with a
/// less specific count mismatch.
fn merged_fill_and_tmem_writes(
    packet: &WorkloadPacket,
    fill_writes: &[CompletedWrite],
    tmem_writes: &[CompletedWrite],
) -> Result<Vec<CompletedWrite>, WgpuRawDpcExecutionError> {
    let mut staged: Vec<(CompletedWrite, bool)> = fill_writes
        .iter()
        .chain(tmem_writes)
        .map(|write| (*write, false))
        .collect();

    let mut merged = Vec::with_capacity(staged.len());
    for declared in packet.journal().accesses() {
        if !declared.mode().writes() {
            continue;
        }
        let claimed = staged
            .iter_mut()
            .find(|(write, taken)| !*taken && write.access() == *declared)
            .ok_or(WgpuRawDpcExecutionError::MergedWriteUnclaimed {
                access_index: declared.operation().get(),
            })?;
        claimed.1 = true;
        merged.push(claimed.0);
    }

    if let Some((write, _)) = staged.iter().find(|(_, taken)| !*taken) {
        return Err(WgpuRawDpcExecutionError::MergedWriteUndeclared {
            access_index: write.access().operation().get(),
        });
    }

    Ok(merged)
}

/// Executes every admitted `FillRectangle` this plan carried and reports
/// the exact ordered `CompletedWrite` list they contributed, **without**
/// mutating the color-target registry.
///
/// The registry is read (to find a resident predecessor's prior device
/// bytes) and, on the first admitted fill ever, built -- but no resident is
/// added or replaced here. Publication is deferred to `publish_raw_dpc`,
/// behind the submission-keyed [`PendingFillPublication`] token, because
/// the guest commit that must precede it happens in `fn64-abi` after this
/// call has already returned and released its borrow.
///
/// Nonclaim: nothing here writes guest RDRAM. `execute_fill_rectangle`
/// produces an owned `Vec<u8>`, and the `CompletedWrite`s are ranges plus
/// content digests, not bytes in motion. That is still exactly true after
/// the RDRAM copyback landed -- the copy is performed by `fn64-abi`, from
/// bytes this backend hands over separately through
/// `committed_guest_render_target_bytes`, and only after the guest commit
/// has succeeded.
fn stage_fills_and_report(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<StagedOutcome, WgpuRawDpcExecutionError> {
    // Durable TMEM, because this path is reached only for a packet whose
    // `plan.loads` is empty -- there is no proposal in existence to sample.
    // A texrect here observes exactly what an earlier packet published,
    // which is what the hardware's TMEM holds at this stream position.
    let staged = raw_dpc_execute_census::timed(raw_dpc_execute_census::Phase::Color, || {
        stage_color_commands(
            collector,
            packet,
            TexrectTmemSource::Committed(collector.physical),
        )
    })?;
    let Some(staged) = staged else {
        let color = collector
            .deferred_compute
            .take()
            .expect("planning-only color staging retains its compute plan");
        return Ok(StagedOutcome::DeferredGuestWritesOnly(
            BackendEffectReport::defer(packet),
            color,
        ));
    };
    let effects = BackendEffectReport::try_new(packet, staged.guest_writes.clone())
        .map_err(WgpuRawDpcExecutionError::Effect)?;
    Ok(StagedOutcome::GuestWritesOnly(effects, staged))
}

/// Which color-target-writing command one entry of the ordered accumulation
/// schedule names, paired with its index into the plan's own per-kind list.
///
/// Deliberately carries only the *index*, never the command payload: the
/// payload is read back out of `collector.plan` at execution time so this
/// schedule cannot become a second, drifting copy of the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorCommandKind {
    Fill(usize),
    Texrect(usize),
    /// A flat raw triangle that declared a per-scanline write run. Indexes
    /// `plan.raw_triangle_commands`, which holds only the triangles that
    /// declared one.
    RawTriangle(usize),
}

/// **Which TMEM image this packet's texrects sample, chosen once per packet
/// by the packet's own load count -- never a fallback.**
///
/// The RDP's TMEM is durable across submissions. A texrect samples whatever
/// is in TMEM at its own stream position, which is:
///
/// - [`Self::Pending`] -- this packet's own staged loads, when the packet
///   carries at least one. **Not one image for the whole packet:** the
///   variant carries the per-load prefix snapshots alongside the sealed
///   transaction, and each texrect is served the prefix taken after the
///   last load BEFORE its own stream position. A texrect that precedes
///   every load in its packet reads no prefix at all and falls to durable
///   committed TMEM, because that is what TMEM holds at that position.
/// - [`Self::Committed`] -- the coordinator's durable [`PhysicalTmemState`],
///   when the packet carries **zero** loads. There is no proposal to
///   observe, and the durable state is not "stale": it is precisely the
///   result of every load an earlier packet already published, which is the
///   only thing the hardware's TMEM could contain.
///
/// The two are not interchangeable and the choice is not a heuristic. A
/// packet with loads must **not** read `Committed`, because the coordinator
/// has not published this packet's own loads yet and the texrect would miss
/// texels the wire stream placed before it -- the defect commit `3a1a6a73`
/// measured as `TMEM_SAMPLE_STATUS_INVALID_BYTE`. A packet without loads
/// must not read a `Pending` image, because none exists.
///
/// **This is deliberately not a fallback for a missing pending image.** The
/// selection is made from `plan.loads.is_empty()` -- a fact about the wire
/// stream -- before any staging runs, not from a `None` observed after
/// staging failed. A `None` where a pending image was expected stays a
/// named refusal.
enum TexrectTmemSource<'a> {
    Pending {
        pending: &'a crate::tmem::PendingTmemTransaction,
        /// TMEM as of each of this packet's loads, keyed by the load's own
        /// stream command index, in stream order.
        prefixes: &'a [(u32, crate::tmem::TmemPrefixSnapshot)],
    },
    Committed(&'a PhysicalTmemState),
}

struct ComputeRasterReplacementPlan {
    dispatches: Vec<ComputeRasterDispatch>,
    declared: Vec<ResourceAccess>,
    claimed: TargetRectangle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskComputeAdmissionRefusal {
    MixedColorCommands,
    TargetFormat,
    Untextured,
    AffineTexture,
    Depth,
    CycleType([u32; 4]),
    ProgramBits([u32; 4]),
    EmptyAccesses,
    AccessMode,
    AccessPurpose,
    AccessRegion,
    AccessOutsideTarget,
    CommandOrder,
    EmptyDispatch,
    NoDispatches,
}

impl From<ComputeRasterAdmissionRefusal> for TaskComputeAdmissionRefusal {
    fn from(reason: ComputeRasterAdmissionRefusal) -> Self {
        match reason {
            ComputeRasterAdmissionRefusal::TargetFormat => Self::TargetFormat,
            ComputeRasterAdmissionRefusal::Untextured => Self::Untextured,
            ComputeRasterAdmissionRefusal::AffineTexture => Self::AffineTexture,
            ComputeRasterAdmissionRefusal::Depth => Self::Depth,
            ComputeRasterAdmissionRefusal::CycleType(words) => Self::CycleType(words),
            ComputeRasterAdmissionRefusal::ProgramBits(words) => Self::ProgramBits(words),
            ComputeRasterAdmissionRefusal::EmptyAccesses => Self::EmptyAccesses,
            ComputeRasterAdmissionRefusal::AccessMode => Self::AccessMode,
            ComputeRasterAdmissionRefusal::AccessPurpose => Self::AccessPurpose,
            ComputeRasterAdmissionRefusal::AccessRegion => Self::AccessRegion,
            ComputeRasterAdmissionRefusal::AccessOutsideTarget => Self::AccessOutsideTarget,
            ComputeRasterAdmissionRefusal::CommandOrder => Self::CommandOrder,
        }
    }
}

enum ComputeRasterReplacementAdmission {
    Admitted(ComputeRasterReplacementPlan),
    Refused(TaskComputeAdmissionRefusal),
}

fn compute_replacement_target_pixels(
    plan: &ComputeRasterReplacementPlan,
    key: ColorTargetKey,
    target_width: u32,
) -> Result<u32, WgpuRawDpcExecutionError> {
    plan.dispatches.iter().try_fold(0u32, |count, dispatch| {
        let accesses: Vec<_> = dispatch
            .batch
            .draws()
            .iter()
            .flat_map(ComputeRasterDrawAdmission::accesses)
            .copied()
            .collect();
        let first_triangle_index = dispatch
            .batch
            .draws()
            .first()
            .expect("a sealed compute dispatch has an admitted draw")
            .triangle_index();
        let claimed = claimed_rectangle_from_accesses(key, &accesses, first_triangle_index)?;
        let column_count = if compute_column_bounds_enabled() {
            let first = claimed.x() & !1;
            let limit = claimed
                .x()
                .checked_add(claimed.width())
                .expect("claimed rectangle was checked when constructed")
                .checked_add(1)
                .map(|limit| limit & !1)
                .unwrap_or(target_width)
                .min(target_width);
            limit - first
        } else {
            target_width
        };
        let dispatch_pixels = column_count
            .checked_mul(claimed.height())
            .expect("bounded replacement dispatch target-pixel count fits u32");
        Ok(count
            .checked_add(dispatch_pixels)
            .expect("bounded replacement chain target-pixel count fits u32"))
    })
}

fn retain_compute_replacement_draw<S: crate::TmemByteSource + ?Sized>(
    builder: &mut Option<ComputeRasterProbeBuilder>,
    dispatches: &mut Vec<ComputeRasterDispatch>,
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    tmem: &S,
) -> Result<Option<TaskComputeAdmissionRefusal>, WgpuRawDpcExecutionError> {
    if let Some(active) = builder.as_mut() {
        match active.push(collector, candidate, index, tmem)? {
            ComputeRasterProbePush::Admitted => return Ok(None),
            ComputeRasterProbePush::SplitDispatch => {}
            ComputeRasterProbePush::Refused(reason) => return Ok(Some(reason.into())),
        }
    }
    if let Some(previous) = builder.take() {
        let Some((dispatch, _)) = previous.finish_dispatch() else {
            return Ok(Some(TaskComputeAdmissionRefusal::EmptyDispatch));
        };
        dispatches.push(dispatch);
    }
    let mut next = ComputeRasterProbeBuilder::new(candidate, Vec::new());
    match next.push(collector, candidate, index, tmem)? {
        ComputeRasterProbePush::Admitted => {}
        ComputeRasterProbePush::SplitDispatch => {
            return Ok(Some(TaskComputeAdmissionRefusal::EmptyDispatch));
        }
        ComputeRasterProbePush::Refused(reason) => return Ok(Some(reason.into())),
    }
    *builder = Some(next);
    Ok(None)
}

fn claimed_rectangle_from_accesses(
    key: ColorTargetKey,
    accesses: &[ResourceAccess],
    triangle_index: usize,
) -> Result<TargetRectangle, WgpuRawDpcExecutionError> {
    verify_accesses_inside(accesses, key)?;
    let base = key.address().get();
    let target_width = key.extent().width();
    let bytes_per_pixel = key.format().bytes_per_pixel();
    let mut claimed = None;
    for access in accesses {
        let fn64_render_ir::ResourceRegion::Rdram { range, .. } = access.region() else {
            return Err(WgpuRawDpcExecutionError::FillAccessRegionKind {
                access_index: access.operation().get(),
            });
        };
        let offset = range.start().get().checked_sub(base).ok_or(
            WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            },
        )?;
        if offset % bytes_per_pixel != 0 || range.len() == 0 || range.len() % bytes_per_pixel != 0 {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
        let first_pixel = offset / bytes_per_pixel;
        let x = first_pixel % target_width;
        let y = first_pixel / target_width;
        let width = range.len() / bytes_per_pixel;
        if x.checked_add(width)
            .is_none_or(|right| right > target_width)
        {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
        claimed = Some(union_target_rectangle(
            TargetRectangle::try_new(x, y, width, 1)?,
            claimed,
        ));
    }
    claimed.ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite { triangle_index })
}

fn plan_compute_raster_replacement(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    schedule: &[(u32, ColorCommandKind)],
    tmem: &TexrectTmemSource<'_>,
) -> Result<ComputeRasterReplacementAdmission, WgpuRawDpcExecutionError> {
    if schedule
        .iter()
        .any(|(_, kind)| !matches!(kind, ColorCommandKind::RawTriangle(_)))
    {
        return Ok(ComputeRasterReplacementAdmission::Refused(
            TaskComputeAdmissionRefusal::MixedColorCommands,
        ));
    }
    let mut builder = None;
    let mut dispatches = Vec::new();
    for (_, kind) in schedule {
        let ColorCommandKind::RawTriangle(index) = *kind else {
            unreachable!("the all-raw-triangle preflight rejected every other command kind")
        };
        let index = CommandIndex::new(index);
        let command_index = collector.plan.raw_triangle_commands[index].command_index;
        let refusal = match tmem {
            TexrectTmemSource::Pending { pending, prefixes } => {
                match prefix_before(prefixes, command_index) {
                    Some(prefix) => retain_compute_replacement_draw(
                        &mut builder,
                        &mut dispatches,
                        collector,
                        candidate,
                        index,
                        &pending.prefix_image(prefix)?,
                    )?,
                    None => retain_compute_replacement_draw(
                        &mut builder,
                        &mut dispatches,
                        collector,
                        candidate,
                        index,
                        collector.physical,
                    )?,
                }
            }
            TexrectTmemSource::Committed(state) => retain_compute_replacement_draw(
                &mut builder,
                &mut dispatches,
                collector,
                candidate,
                index,
                *state,
            )?,
        };
        if let Some(reason) = refusal {
            return Ok(ComputeRasterReplacementAdmission::Refused(reason));
        }
    }
    if let Some(builder) = builder {
        let Some((dispatch, _)) = builder.finish_dispatch() else {
            return Ok(ComputeRasterReplacementAdmission::Refused(
                TaskComputeAdmissionRefusal::EmptyDispatch,
            ));
        };
        dispatches.push(dispatch);
    }
    if dispatches.is_empty() {
        return Ok(ComputeRasterReplacementAdmission::Refused(
            TaskComputeAdmissionRefusal::NoDispatches,
        ));
    }
    let declared: Vec<_> = dispatches
        .iter()
        .flat_map(|dispatch| dispatch.batch.draws())
        .flat_map(ComputeRasterDrawAdmission::accesses)
        .copied()
        .collect();
    let first_triangle_index = dispatches[0]
        .batch
        .draws()
        .first()
        .expect("a sealed compute dispatch has an admitted draw")
        .triangle_index();
    let claimed =
        claimed_rectangle_from_accesses(candidate.key(), &declared, first_triangle_index)?;
    Ok(ComputeRasterReplacementAdmission::Admitted(
        ComputeRasterReplacementPlan {
            dispatches,
            declared,
            claimed,
        },
    ))
}

/// **The N-command accumulation seam.**
///
/// Executes every admitted `FillRectangle` and `TextureRectangle` this
/// packet carried against one shared full-extent buffer, in the packet's
/// own command order, and returns the single staged token the composed
/// result publishes through.
///
/// ## Why one buffer and one candidate, not N of each
///
/// `begin_candidate` derives its generation from the registry, and this
/// staging path deliberately does not publish into the registry (that is
/// `publish_raw_dpc`'s job, after the guest commit). So a second
/// `begin_candidate` call would hand back the *same* generation as the
/// first, not a successor -- N candidates would be N copies of one
/// candidate, and N `admit_completed_initialization` calls would publish N
/// initializations of a single generation. One candidate is therefore not
/// an optimization; it is the only shape that does not forge a generation.
///
/// The buffer is threaded the same way for the same reason: each command
/// takes the accumulated buffer as its own `resident_bytes` and its
/// full-extent output *becomes* the accumulator for the next. That is what
/// makes a later command's pixels win an overlap and an earlier command's
/// pixels survive outside it -- the accumulation is the composition, not a
/// blend policy layered over it.
///
/// ## Order is derived, never chosen
///
/// The schedule is built by sorting on the `command_index` the decoder's
/// own stream walk assigned (`PlanCollector::command` increments it once
/// per wire command, and both `fills` and `texrect_commands` record it).
/// That index is the packet's command order by construction. It is a
/// *recovery* of the stream's order, not a policy: `merged_fill_and_tmem_
/// writes` independently re-derives the same order from the resource
/// journal to build the effect report, and the two agreeing is the
/// cross-check. Note the asymmetry that makes this real evidence rather
/// than a tautology -- the journal is `raw_dpc::push_access`'s `planned`
/// vector and the command index is `PlanCollector`'s own counter, two
/// separate walks of the same stream.
///
/// ## Digest staleness across N commands
///
/// A `CompletedWrite` claims "this range holds content with this digest".
/// With N commands writing one buffer at overlapping ranges, a digest
/// computed when its own command staged describes a buffer state that no
/// longer exists the moment any later command touches an overlapping byte
/// -- and `rsp_commit`'s `copy_committed_guest_writes` re-derives every
/// digest from the bytes before writing any of them and aborts the whole
/// copy on a mismatch. Measured: it did, naming write #0, when only the
/// two-command case was handled.
///
/// The fix is not per-command patching but a single rule: **every write's
/// digest is computed once, against the final composed buffer, after the
/// last command has run.** Each command contributes only its *accesses*
/// (the journal's fact, never re-derived) during the loop; the digests are
/// all filled in together at the end, so no write can carry a digest from
/// an intermediate state. A per-command recomputation would be O(N^2) and,
/// worse, would still be stale for every write except the last.
fn stage_color_commands(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
    tmem: TexrectTmemSource<'_>,
) -> Result<Option<StagedFill>, WgpuRawDpcExecutionError> {
    // The ordered schedule, recovered from the decoder's own per-command
    // stream index. `sort_by_key` on that index is not a sort *policy*: the
    // index IS the stream position, so this recovers an interleaving the
    // decoder already fixed rather than imposing one. Stable, so two
    // entries that somehow shared an index would keep their relative plan
    // order rather than being silently transposed.
    let cpu_phase_attributed = collector.task_cpu_phase_census.is_some()
        && ordered_depth_free_acff_triangle_member(collector);
    let plan = &collector.plan;
    let schedule = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::ScheduleDecodeRowPrepRaster,
        || {
            let mut schedule: Vec<(u32, ColorCommandKind)> =
                plan.fills
                    .iter()
                    .enumerate()
                    .map(|(index, (command_index, ..))| {
                        (*command_index, ColorCommandKind::Fill(index))
                    })
                    .chain(plan.texrect_commands.iter().enumerate().map(
                        |(index, (_, _, _, command_index, _))| {
                            (*command_index, ColorCommandKind::Texrect(index))
                        },
                    ))
                    .chain(plan.raw_triangle_commands.iter().enumerate().map(
                        |(index, scheduled)| {
                            (
                                scheduled.command_index,
                                ColorCommandKind::RawTriangle(index),
                            )
                        },
                    ))
                    .collect();
            schedule.sort_by_key(|(command_index, _)| *command_index);
            schedule
        },
    );
    if schedule.is_empty() {
        return Ok(None);
    }

    // The candidate, and the target key, derived once from this packet's
    // own staged `SetColorImage`. Every command in the schedule composes
    // into the same target by construction -- `key_of_declared_render_
    // target` cross-checks each texrect's declared accesses against this
    // key's range, and a fill naming a different image would produce a
    // different key here and be caught by the same check.
    let key = color_target_key(collector, packet)?;
    if collector.defer_compute_replacement {
        let registry = collector
            .color_targets
            .as_ref()
            .expect("color_target_key populates the registry");
        let batch = collector
            .color_execution_batch
            .as_deref()
            .expect("deferred compute execution requires a task color planner");
        let (preview, task_input) = batch.preview_candidate(registry, key)?;
        let plan = match plan_compute_raster_replacement(collector, &preview, &schedule, &tmem)? {
            ComputeRasterReplacementAdmission::Admitted(plan) => plan,
            ComputeRasterReplacementAdmission::Refused(reason) => {
                return Err(WgpuRawDpcExecutionError::TaskBatchComputeNotAdmitted {
                    ordinal: collector.ordinal,
                    reason,
                });
            }
        };
        let program_attribution = compute_program_attribution_from_ids(
            plan.dispatches
                .iter()
                .flat_map(|dispatch| dispatch.batch.draws())
                .map(|draw| draw.program().shader_id()),
        );

        // Exact admission completed without mutating the generation planner.
        // Reserve only now. No other operation can interleave between preview
        // and reservation because both values are held inside this exclusive
        // execution borrow.
        let (candidate, reserved_input) = collector
            .color_execution_batch
            .as_deref_mut()
            .expect("the previewed task color planner remains present")
            .begin_candidate(registry, key)?;
        assert_eq!(candidate, preview);
        assert_eq!(reserved_input, task_input);

        let initial_bytes = match task_input {
            TaskColorInput::DurableRegistry => Some(
                registry
                    .residents()
                    .iter()
                    .find(|resident| resident.key() == key)
                    .map(|resident| resident.device_bytes().device_bytes().to_vec())
                    .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?,
            ),
            TaskColorInput::PriorTaskCheckpoint => None,
        };
        collector.deferred_compute = Some(DeferredComputeColor {
            candidate,
            plan,
            program_attribution,
            initial_bytes,
        });
        return Ok(None);
    }

    let wants_depth = collector.plan.triangles.iter().any(|planned| {
        planned.draw.as_ref().is_ok_and(|draw| {
            draw.other_mode.depth_compare_enabled() || draw.other_mode.depth_update_enabled()
        })
    });
    let ordered_cpu_eligible = !wants_depth
        && schedule
            .iter()
            .all(|(_, command)| matches!(command, ColorCommandKind::RawTriangle(_)))
        && !collector.defer_compute_replacement
        && !collector.compute_replacement_enabled;

    let mut ordered_seed: Option<(Vec<u8>, ColorCoverageState)> = None;
    let (candidate, task_input) = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::CandidateSeedCopy,
        || -> Result<_, WgpuRawDpcExecutionError> {
            Ok(match collector.ordered_cpu_color_batch.as_deref_mut() {
                Some(batch) if ordered_cpu_eligible => {
                    let registry = collector
                        .color_targets
                        .as_mut()
                        .expect("color_target_key populates the registry");
                    let (candidate, seed) = batch.begin_member(registry, key)?;
                    ordered_seed = seed;
                    (candidate, None)
                }
                Some(batch) => {
                    if batch.tail.is_some() {
                        batch.flush(
                            collector
                                .color_targets
                                .as_mut()
                                .expect("an ordered CPU accumulator implies a registry"),
                        )?;
                    }
                    let registry = collector
                        .color_targets
                        .as_ref()
                        .expect("color_target_key populates the registry");
                    (registry.begin_candidate(key)?, None)
                }
                None => match collector.color_execution_batch.as_deref_mut() {
                    Some(batch) => {
                        let registry = collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry");
                        let (candidate, input) = batch.begin_candidate(registry, key)?;
                        (candidate, Some(input))
                    }
                    None => {
                        let registry = collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry");
                        (registry.begin_candidate(key)?, None)
                    }
                },
            })
        },
    )?;

    // The accumulator. Seeded from the resident's real prior bytes when
    // this target already exists, and left `None` for a brand-new target --
    // exactly the distinction `execute_fill_rectangle` already draws, and
    // deliberately NOT flattened to a zero buffer here, which would
    // fabricate content for a resident whose bytes failed to thread.
    let (mut accumulated, mut ordered_coverage) = task_cpu_phase_census::timed(
        collector.task_cpu_phase_census.as_deref_mut(),
        cpu_phase_attributed,
        task_cpu_phase_census::Phase::CandidateSeedCopy,
        || {
            if ordered_cpu_eligible {
                let seed = ordered_seed.or_else(|| {
                    if task_input == Some(TaskColorInput::PriorTaskCheckpoint) {
                        None
                    } else {
                        collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry")
                            .residents()
                            .iter()
                            .find(|resident| resident.key() == key)
                            .map(|resident| {
                                (
                                    resident.device_bytes().device_bytes().to_vec(),
                                    resident.coverage().clone(),
                                )
                            })
                    }
                });
                match seed {
                    Some((bytes, coverage)) => (Some(bytes), Some(coverage)),
                    None => (
                        None,
                        Some(ColorCoverageState::unknown(candidate.key().extent())),
                    ),
                }
            } else {
                let bytes = ordered_seed.map(|(bytes, _)| bytes).or_else(|| {
                    if task_input == Some(TaskColorInput::PriorTaskCheckpoint) {
                        None
                    } else {
                        collector
                            .color_targets
                            .as_ref()
                            .expect("color_target_key populates the registry")
                            .residents()
                            .iter()
                            .find(|resident| resident.key() == key)
                            .map(|resident| resident.device_bytes().device_bytes().to_vec())
                    }
                });
                (bytes, None)
            }
        },
    );

    // **The depth accumulator, one RDP depth-memory cell per target pixel,
    // persisting across every draw in this packet's schedule.** It is the
    // z-buffer: a later draw's fragment sees the depth an earlier draw
    // committed, which is what makes overlapping triangles at different
    // depths resolve. Allocated (seeded to `(0, 0)` -- the value a zeroed
    // guest z-image decodes to) only when some raw triangle in this packet
    // actually requests a depth compare or update; a packet with no z-wired
    // draw keeps it `None` and every draw resolves by painter's order,
    // exactly as before. The z-image binding (`SetZImage`/`SetMaskImage`) is
    // what legalises those OtherMode z bits in the admitted subset -- they
    // are only ever set in a packet that also bound a z-image -- so keying
    // the accumulator off the z bits is equivalent here to keying it off the
    // binding, without threading the address through the neutral IR.
    let mut depth_accum: Option<Vec<crate::targets::DepthCell>> =
        wants_depth.then(|| vec![(0u32, 0u8); key.extent().pixels() as usize]);

    if collector.compute_replacement_enabled {
        let admission_started = Instant::now();
        let replacement =
            match plan_compute_raster_replacement(collector, &candidate, &schedule, &tmem)? {
                ComputeRasterReplacementAdmission::Admitted(plan) => Some(plan),
                ComputeRasterReplacementAdmission::Refused(_) => None,
            }
            .map(|plan| -> Result<_, WgpuRawDpcExecutionError> {
                let target_pixels = compute_replacement_target_pixels(
                    &plan,
                    key,
                    candidate.key().extent().width(),
                )?;
                Ok((plan, target_pixels))
            })
            .transpose()?;
        if let Some((plan, target_pixels)) = replacement.filter(|(_, target_pixels)| {
            compute_raster_replacement_admitted(*target_pixels, compute_raster_min_target_pixels())
        }) {
            let admission_elapsed = admission_started.elapsed();
            let initial = accumulated
                .take()
                .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?;
            let pipeline = collector
                .compute_replacement_pipeline
                .as_deref_mut()
                .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
            let dispatches: Vec<_> = plan
                .dispatches
                .iter()
                .map(|dispatch| {
                    let accesses: Vec<_> = dispatch
                        .batch
                        .draws()
                        .iter()
                        .flat_map(ComputeRasterDrawAdmission::accesses)
                        .copied()
                        .collect();
                    let first_triangle_index = dispatch
                        .batch
                        .draws()
                        .first()
                        .expect("a sealed compute dispatch has an admitted draw")
                        .triangle_index();
                    let claimed =
                        claimed_rectangle_from_accesses(key, &accesses, first_triangle_index)?;
                    let target_width = candidate.key().extent().width();
                    let (first_column, column_count) = if compute_column_bounds_enabled() {
                        let first = claimed.x() & !1;
                        let limit = claimed
                            .x()
                            .checked_add(claimed.width())
                            .expect("claimed rectangle was checked when constructed")
                            .checked_add(1)
                            .map(|limit| limit & !1)
                            .unwrap_or(target_width)
                            .min(target_width);
                        (first, limit - first)
                    } else {
                        (0, target_width)
                    };
                    Ok(ComputeHotColorDispatch {
                        triangles: &dispatch.triangles,
                        tmem: &dispatch.tmem,
                        tile: dispatch.tile,
                        first_row: claimed.y(),
                        row_count: claimed.height(),
                        first_column,
                        column_count,
                    })
                })
                .collect::<Result<_, WgpuRawDpcExecutionError>>()?;
            let extent = plan.dispatches[0].extent;
            let started = Instant::now();
            let output = pipeline
                .compute_triangle_hot_color_chain(extent, &initial, &dispatches)
                .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
            let elapsed = started.elapsed();
            let draw_count = plan.dispatches.iter().try_fold(0u32, |count, dispatch| {
                count.checked_add(u32::try_from(dispatch.batch.draws().len()).ok()?)
            });
            let draw_count = draw_count.expect("bounded raw-DPC replacement draw count fits u32");
            let batch_count = u32::try_from(plan.dispatches.len())
                .expect("bounded raw-DPC replacement batch count fits u32");
            let effects_started = Instant::now();
            let device_bytes = crate::DeviceColorBytes::new_for_fill(
                key,
                candidate.generation(),
                key.format(),
                output,
            )?;
            let completed = CompletedColorTargetWrite::new_for_fill(
                key,
                candidate.generation(),
                key.range(),
                plan.claimed,
                device_bytes,
            );
            let guest_writes =
                fill_completed_writes(key, completed.device_bytes(), &plan.declared)?;
            let initialized = candidate.admit_completed_initialization(completed)?;
            let effects_elapsed = effects_started.elapsed();
            collector.compute_replacement_receipt = Some(ComputeRasterProbeReceipt {
                submission_count: 1,
                batch_count,
                draw_count,
                target_pixels,
                admission_elapsed,
                elapsed,
                effects_elapsed,
            });
            return Ok(Some(StagedFill {
                initialized,
                guest_writes,
                prepared_sparse_checkpoint: None,
                cpu_phase_attributed: false,
            }));
        }
    }

    // Accesses only, in schedule order. Digests are deliberately absent
    // until the loop ends -- see this function's own doc on staleness.
    let mut declared: Vec<ResourceAccess> = Vec::new();
    let mut claimed: Option<TargetRectangle> = None;
    let mut last_completed: Option<CompletedColorTargetWrite> = None;

    let move_accumulator = move_color_accumulator_enabled();
    let own_command_input = own_color_command_input_enabled();
    let mut compute_probe_builder = None;
    let schedule_started = task_cpu_phase_census::started(
        collector.task_cpu_phase_census.as_deref(),
        cpu_phase_attributed,
    );
    for (schedule_index, (_, kind)) in schedule.iter().enumerate() {
        if compute_probe_builder.is_some() && !matches!(*kind, ColorCommandKind::RawTriangle(_)) {
            let expected = accumulated
                .as_deref()
                .expect("an active compute batch has resident CPU output");
            flush_compute_probe(
                &mut compute_probe_builder,
                collector.ordinal,
                expected,
                &mut collector.compute_probes,
            );
        }
        let color_phase = match *kind {
            ColorCommandKind::Fill(_) => raw_dpc_execute_census::Phase::ColorFill,
            ColorCommandKind::Texrect(_) => raw_dpc_execute_census::Phase::ColorTexrect,
            ColorCommandKind::RawTriangle(_) => raw_dpc_execute_census::Phase::ColorTriangle,
        };
        let (completed, accesses) = raw_dpc_execute_census::timed(
            color_phase,
            || -> Result<_, WgpuRawDpcExecutionError> {
                Ok(match *kind {
                    ColorCommandKind::Fill(index) => execute_scheduled_fill(
                        collector,
                        &candidate,
                        index,
                        if own_command_input {
                            accumulated.take()
                        } else {
                            accumulated.clone()
                        },
                    )?,
                    ColorCommandKind::Texrect(index) => {
                        // **A texrect samples TMEM at its OWN stream position.**
                        //
                        // `stage_and_report` chose the family once, upstream, from
                        // a fact about the wire stream (does this packet carry
                        // loads at all). Within the pending family the position is
                        // still per command, because TMEM is durable within a
                        // packet: a texrect observes every load before it and no
                        // load after it. Selecting on the command index the
                        // decoder's own stream walk assigned -- the same index this
                        // schedule is sorted by -- keeps that a recovery of the
                        // stream's order rather than a policy.
                        let resident =
                            color_command_input(&mut accumulated, own_command_input, key)?;
                        let command_index = collector.plan.texrect_commands[index].3;
                        match tmem {
                            TexrectTmemSource::Pending { pending, prefixes } => {
                                match prefix_before(prefixes, command_index) {
                                    Some(prefix) => execute_scheduled_texrect(
                                        collector,
                                        &candidate,
                                        &pending.prefix_image(prefix)?,
                                        true,
                                        index,
                                        resident,
                                        claimed,
                                    )?,
                                    // No load precedes this texrect in its own
                                    // packet, so what TMEM holds here is exactly
                                    // what an earlier packet published: durable
                                    // committed state, read through the same one
                                    // sampler. Not a fallback for a missing image
                                    // -- the absence of a preceding load IS the
                                    // stream fact that makes committed correct.
                                    None => execute_scheduled_texrect(
                                        collector,
                                        &candidate,
                                        collector.physical,
                                        false,
                                        index,
                                        resident,
                                        claimed,
                                    )?,
                                }
                            }
                            TexrectTmemSource::Committed(state) => execute_scheduled_texrect(
                                collector, &candidate, state, false, index, resident, claimed,
                            )?,
                        }
                    }
                    ColorCommandKind::RawTriangle(index) => {
                        let index = CommandIndex::new(index);
                        // Same resident-bytes requirement as a texrect and for the
                        // same reason: a triangle writes a sub-region, so every
                        // pixel outside it must come from real prior content.
                        let resident =
                            color_command_input(&mut accumulated, own_command_input, key)?;
                        let command_coverage = ordered_coverage
                            .as_mut()
                            .map(ColorCoverageState::take_for_command);
                        // **A raw triangle samples TMEM at its OWN stream position,
                        // by the SAME rule a texrect does.**
                        //
                        // Not a parallel implementation: this is the identical
                        // `prefix_before` call over the identical `prefixes` slice,
                        // dispatched on the identical `TexrectTmemSource` the arm
                        // above matches on. WM2000's own triangle packets carry NINE
                        // TMEM loads each, so "which load did this draw see" is a
                        // live question for a triangle exactly as it is for a
                        // texrect -- and answering it with a per-packet image would
                        // draw every triangle with the ninth load's texels.
                        let command_index =
                            collector.plan.raw_triangle_commands[index].command_index;
                        match tmem {
                            TexrectTmemSource::Pending { pending, prefixes } => {
                                match prefix_before(prefixes, command_index) {
                                    Some(prefix) => {
                                        let image = pending.prefix_image(prefix)?;
                                        if collector.collect_compute_probe {
                                            if let Some(previous) = retain_compute_probe_draw(
                                                &mut compute_probe_builder,
                                                collector,
                                                &candidate,
                                                index,
                                                &image,
                                                resident.as_ref(),
                                            )? {
                                                push_finished_compute_probe(
                                                    previous,
                                                    collector.ordinal,
                                                    resident.as_ref(),
                                                    &mut collector.compute_probes,
                                                );
                                            }
                                        }
                                        execute_scheduled_raw_triangle(
                                            collector,
                                            &candidate,
                                            index,
                                            resident,
                                            &image,
                                            true,
                                            depth_accum.as_deref_mut(),
                                            command_coverage,
                                        )?
                                    }
                                    // No load precedes this triangle in its own
                                    // packet, so TMEM holds exactly what an earlier
                                    // packet published -- durable committed state,
                                    // read through the same one sampler. The absence
                                    // of a preceding load IS the stream fact that
                                    // makes committed correct; it is not a fallback.
                                    None => {
                                        if collector.collect_compute_probe {
                                            if let Some(previous) = retain_compute_probe_draw(
                                                &mut compute_probe_builder,
                                                collector,
                                                &candidate,
                                                index,
                                                collector.physical,
                                                resident.as_ref(),
                                            )? {
                                                push_finished_compute_probe(
                                                    previous,
                                                    collector.ordinal,
                                                    resident.as_ref(),
                                                    &mut collector.compute_probes,
                                                );
                                            }
                                        }
                                        execute_scheduled_raw_triangle(
                                            collector,
                                            &candidate,
                                            index,
                                            resident,
                                            collector.physical,
                                            false,
                                            depth_accum.as_deref_mut(),
                                            command_coverage,
                                        )?
                                    }
                                }
                            }
                            TexrectTmemSource::Committed(state) => {
                                if collector.collect_compute_probe {
                                    if let Some(previous) = retain_compute_probe_draw(
                                        &mut compute_probe_builder,
                                        collector,
                                        &candidate,
                                        index,
                                        state,
                                        resident.as_ref(),
                                    )? {
                                        push_finished_compute_probe(
                                            previous,
                                            collector.ordinal,
                                            resident.as_ref(),
                                            &mut collector.compute_probes,
                                        );
                                    }
                                }
                                execute_scheduled_raw_triangle(
                                    collector,
                                    &candidate,
                                    index,
                                    resident,
                                    state,
                                    false,
                                    depth_accum.as_deref_mut(),
                                    command_coverage,
                                )?
                            }
                        }
                    }
                })
            },
        )?;
        if schedule_index + 1 == schedule.len() {
            flush_compute_probe(
                &mut compute_probe_builder,
                collector.ordinal,
                completed.device_bytes().device_bytes(),
                &mut collector.compute_probes,
            );
        }
        claimed = Some(union_target_rectangle(completed.rectangle(), claimed));
        declared.extend(accesses);
        // This command's owned output becomes the next command's resident
        // bytes. Intermediate completions have no consumer: only the last
        // completion can be admitted and published. Moving their existing
        // buffer therefore preserves the single owner instead of cloning a
        // complete target and immediately dropping the original. The last
        // command needs no next accumulator at all.
        //
        // Fresh Time Profiler attribution on the WM2000 rs+wgpu lane assigns
        // 1,646/27,437 exclusive samples to the former clone in this
        // function. `FN64_MOVE_COLOR_ACCUMULATOR=0` retains that exact clone
        // path as the same-binary measurement control.
        if move_accumulator {
            if schedule_index + 1 == schedule.len() {
                last_completed = Some(completed);
            } else if ordered_coverage.is_some() {
                let (bytes, coverage) = completed.into_task_accumulator();
                accumulated = Some(bytes);
                ordered_coverage = Some(coverage);
            } else {
                accumulated = Some(
                    completed
                        .into_device_color_bytes()
                        .into_device_bytes()
                        .into_vec(),
                );
            }
        } else {
            accumulated = Some(completed.device_bytes().device_bytes().to_vec());
            if ordered_coverage.is_some() {
                ordered_coverage = Some(completed.coverage().clone());
            }
            last_completed = Some(completed);
        }
    }
    task_cpu_phase_census::record_started(
        collector.task_cpu_phase_census.as_deref_mut(),
        task_cpu_phase_census::Phase::ScheduleDecodeRowPrepRaster,
        schedule_started,
    );

    raw_dpc_execute_census::timed(
        raw_dpc_execute_census::Phase::ColorFinalize,
        || -> Result<_, WgpuRawDpcExecutionError> {
            let completed = last_completed.expect("a non-empty schedule ran at least one command");
            // **Every digest, computed once, against the final buffer.** No write
            // in this list can describe an intermediate state, because none of them
            // existed until now -- `declared` carried only accesses through the
            // loop, and this is the single call that turns them into digests.
            //
            // `fill_completed_writes` is the existing per-access digest derivation,
            // reused rather than duplicated: what changed with N commands is *when*
            // it is called (once, at the end) and over *which* buffer (the composed
            // one), not how a digest is derived from an access.
            // The claimed rectangle is the union of every command's own, which is
            // what `admit_completed_initialization` reads to decide whether a
            // brand-new target is fully initialized. Reporting one command's
            // rectangle would understate what N proved.
            let completed = completed.with_claimed_rectangle(
                claimed.expect("a non-empty schedule claimed at least one rectangle"),
            );
            let initialized = candidate.admit_completed_initialization(completed)?;
            let (guest_writes, prepared_sparse_checkpoint) = if fused_sparse_checkpoint_enabled()
                && collector
                    .ordered_cpu_color_batch
                    .as_deref()
                    .is_some_and(|batch| batch.active.is_some())
            {
                let (checkpoint, writes) =
                    initialized.sparse_checkpoint_from_accesses(&declared)?;
                (writes, Some(checkpoint))
            } else {
                (
                    fill_completed_writes(key, initialized.device_bytes(), &declared)?,
                    None,
                )
            };
            Ok(Some(StagedFill {
                initialized,
                guest_writes,
                prepared_sparse_checkpoint,
                cpu_phase_attributed,
            }))
        },
    )
}

fn ordered_depth_free_acff_triangle_member(collector: &ExecutionCollector<'_>) -> bool {
    task_cpu_phase_shape(
        collector.ordered_cpu_color_batch.is_some(),
        collector.plan.draw.color_image.is_some_and(|image| {
            image.format() == crate::ImageFormat::Rgba && image.size() == crate::PixelSize::Bits16
        }),
        collector.plan.fills.len(),
        collector.plan.texrect_commands.len(),
        collector.plan.raw_triangle_commands.len(),
        collector.defer_compute_replacement,
        collector.compute_replacement_enabled,
    ) && collector
        .plan
        .raw_triangle_commands
        .iter()
        .all(|scheduled| {
            collector.plan.triangles[scheduled.triangle_index]
                .draw
                .as_ref()
                .is_ok_and(|draw| {
                    task_cpu_phase_hot_program(
                        draw.combine_params,
                        draw.other_mode,
                        scheduled
                            .decoded
                            .as_ref()
                            .is_ok_and(|triangle| triangle.flags().shaded()),
                        scheduled
                            .decoded
                            .as_ref()
                            .is_ok_and(|triangle| triangle.flags().textured()),
                    )
                })
        })
}

const fn task_cpu_phase_shape(
    ordered_batch: bool,
    rgba16_target: bool,
    fill_count: usize,
    texrect_count: usize,
    raw_triangle_count: usize,
    deferred_compute: bool,
    compute_replacement: bool,
) -> bool {
    ordered_batch
        && rgba16_target
        && fill_count == 0
        && texrect_count == 0
        && raw_triangle_count > 0
        && !deferred_compute
        && !compute_replacement
}

const fn task_cpu_phase_hot_program(
    combine: CombineParams,
    other_mode: OtherMode,
    shaded: bool,
    textured: bool,
) -> bool {
    combine.low() == 0xfc15_fea3
        && combine.high() == 0xf00f_f23f
        && other_mode.high() == 0x0018_acff
        && other_mode.low() == 0x0f0a_7008
        && !other_mode.depth_compare_enabled()
        && !other_mode.depth_update_enabled()
        && shaded
        && textured
}

fn move_color_accumulator_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match crate::diag_env::diag_env("FN64_MOVE_COLOR_ACCUMULATOR") {
        Some(value) if value == "0" => false,
        Some(value) if value == "1" => true,
        Some(value) => panic!("FN64_MOVE_COLOR_ACCUMULATOR must be exactly 0 or 1, got {value:?}"),
        None => true,
    })
}

fn fused_sparse_checkpoint_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    // Was `env_default_one`, deleted with the rest of the ad-hoc env layer in
    // task 2.2b. Spelled out here rather than reintroducing a helper: this is
    // the ONLY remaining default-on diagnostic in this crate, and the arms
    // below are the same ones `env_default_one` had.
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_FUSED_SPARSE_CHECKPOINT") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_FUSED_SPARSE_CHECKPOINT must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

fn shared_copyback_payloads_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_RENDER_COPYBACK_PAYLOAD_SHARE") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_RENDER_COPYBACK_PAYLOAD_SHARE must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

fn compute_column_bounds_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(
        || match crate::diag_env::diag_env("FN64_COMPUTE_RASTER_COLUMN_BOUNDS") {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(value) => {
                panic!("FN64_COMPUTE_RASTER_COLUMN_BOUNDS must be exactly 0 or 1, got {value:?}")
            }
            None => true,
        },
    )
}

fn compute_raster_min_target_pixels() -> u32 {
    static MINIMUM: OnceLock<u32> = OnceLock::new();
    *MINIMUM.get_or_init(
        || match crate::diag_env::diag_env("FN64_COMPUTE_RASTER_MIN_TARGET_PIXELS") {
            Some(value) => value.parse::<u32>().unwrap_or_else(|error| {
                panic!(
                    "FN64_COMPUTE_RASTER_MIN_TARGET_PIXELS must be a decimal u32, got {value:?}: {error}"
                )
            }),
            None => 16_384,
        },
    )
}

const fn compute_raster_replacement_admitted(target_pixels: u32, minimum: u32) -> bool {
    target_pixels >= minimum
}

fn own_color_command_input_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match crate::diag_env::diag_env("FN64_OWN_COLOR_COMMAND_INPUT") {
        Some(value) if value == "0" => false,
        Some(value) if value == "1" => true,
        Some(value) => panic!("FN64_OWN_COLOR_COMMAND_INPUT must be exactly 0 or 1, got {value:?}"),
        None => true,
    })
}

fn color_command_input<'a>(
    accumulated: &'a mut Option<Vec<u8>>,
    owned: bool,
    key: crate::targets::ColorTargetKey,
) -> Result<Cow<'a, [u8]>, crate::targets::TexrectExecutionError> {
    if owned {
        accumulated
            .take()
            .map(Cow::Owned)
            .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })
    } else {
        accumulated
            .as_deref()
            .map(Cow::Borrowed)
            .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })
    }
}

/// The TMEM prefix a command at `command_index` observes: the one taken
/// after the LAST load whose own stream position is strictly earlier.
///
/// Strictly earlier, never equal: a load and the texrect that samples it are
/// separate wire commands with separate indices, and an equal index would
/// mean one command was both, which the decoder cannot produce. `None` means
/// no load in this packet precedes the command, so it observes durable
/// committed TMEM instead.
///
/// `prefixes` is in stream order by construction (`stage_and_report` appends
/// one per load as it walks `plan.loads`, which the decoder filled in its
/// single stream walk), so the last qualifying entry is the latest one.
fn prefix_before(
    prefixes: &[(u32, crate::tmem::TmemPrefixSnapshot)],
    command_index: u32,
) -> Option<&crate::tmem::TmemPrefixSnapshot> {
    prefixes
        .iter()
        .rev()
        .find(|(load_command, _)| *load_command < command_index)
        .map(|(_, prefix)| prefix)
}

/// The smallest rectangle containing both, or `covered` alone when there is
/// no prior claim.
fn union_target_rectangle(
    covered: TargetRectangle,
    prior: Option<TargetRectangle>,
) -> TargetRectangle {
    let Some(prior) = prior else {
        return covered;
    };
    let x = covered.x().min(prior.x());
    let y = covered.y().min(prior.y());
    let right = (covered.x() + covered.width()).max(prior.x() + prior.width());
    let bottom = (covered.y() + covered.height()).max(prior.y() + prior.height());
    TargetRectangle::try_new(x, y, right - x, bottom - y)
        .expect("a union of two in-bounds rectangles is in bounds")
}

/// This packet's color-target key, derived from the `SetColorImage`
/// current at the packet's stream position -- `PlanCollector`'s tracked
/// `draw.color_image`, which is seeded from `WgpuBackend`'s durable
/// `rdp_state` and updated by any `SetColorImage` this packet carries.
///
/// **Not read off the first `FillRectangle`.** That was the previous
/// derivation and it was wrong in a way no fill-bearing packet could
/// expose: the RDP's color-image register is durable across submissions,
/// so a packet may compose into a target it never re-declares. The
/// decoder's own `raw_dpc::plan_texture_rectangle` already derives a
/// texrect's declared `ColorFramebuffer` write accesses from that same
/// durable `state.color_image()`, so reading a packet-local fill here made
/// the executor and the decoder answer one question two ways. Measured on
/// WM2000: a real packet of 14 texrects, 4 loads and zero fills, every
/// texrect declaring a four-access write run, aborted the run because
/// `fills.first()` was `None`.
///
/// The fill is retained as a **cross-check**, not a source: a fill whose
/// own `color_image` disagrees with the tracked register is a decoder /
/// executor divergence and is refused by name rather than silently
/// preferring either.
///
/// Builds the registry on the first admitted color-target command ever,
/// exactly as the fill path did, and for the same reason: neither
/// `try_new` nor `create` has a memory layout to build it from.
fn color_target_key(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<ColorTargetKey, WgpuRawDpcExecutionError> {
    let image = collector
        .plan
        .draw
        .color_image
        .ok_or(WgpuRawDpcExecutionError::NoStagedColorImage)?;
    if let Some((command_index, fill, ..)) = collector.plan.fills.first() {
        let declared = ColorImage::from_wire(
            image_format(fill.color_image.format),
            pixel_size(fill.color_image.size),
            fill.color_image.width,
            fill.color_image.address,
        );
        if declared != image {
            return Err(
                WgpuRawDpcExecutionError::FillColorImageDisagreesWithRegister {
                    command_index: *command_index,
                },
            );
        }
    }
    let Some(extent) = collector.configured_target_extent else {
        return Err(WgpuRawDpcExecutionError::NoColorTargetHeight);
    };
    let format = ColorTargetFormat::try_from_rdp(image.format(), image.size())?;
    let key = ColorTargetKey::try_new(
        image.address(),
        ColorTargetExtent::try_new(image.width(), extent.height)?,
        format,
    )?;
    if collector.color_targets.is_none() {
        *collector.color_targets = Some(ColorTargetRegistry::try_new(
            packet.memory_layout(),
            COLOR_TARGET_REGISTRY_CAPACITY,
        )?);
    }
    Ok(key)
}

/// Converts one captured guest-RDRAM range from the storage byte order the
/// capture delivers into the flat logical order [`crate::DeviceColorBytes`]
/// is expressed in.
///
/// **Not cosmetic, and not a guess.** `fn64-runtime`'s RDRAM stores guest
/// bytes in native words under a per-width XOR byte-lane mapping --
/// `write_u8` indexes `range(addr, 1, 3)`, i.e. `offset ^ 3` on a
/// little-endian host (`fn64-runtime/src/rdram.rs:623-627`), which is
/// exactly why `copy_committed_guest_writes` copies OUT through
/// `write_logical_bytes` rather than a raw `copy_from_slice`. The ABI's
/// guest-read capture slices the live allocation directly
/// (`fn64-abi/src/task_dispatch/rsp_commit.rs`), so what arrives here is
/// storage order, and reading it as logical bytes byte-swaps every pixel.
///
/// The conformance runner records the same trap from the other direction:
/// a raw slice copy there "reported every pixel as byte-swapped against the
/// reference backend -- a runner defect that would have been read as a
/// renderer defect".
///
/// This is the inverse of that copy: `logical[i] = storage[i ^ 3]`, applied
/// within each aligned 4-byte word so a range that is not word-aligned or
/// not a whole number of words still maps every byte it does carry.
fn logical_bytes_from_captured_rdram(captured: &[u8]) -> Vec<u8> {
    captured
        .iter()
        .enumerate()
        .map(|(index, _)| {
            // The lane swap is defined within each aligned word; `^ 3` on
            // the index inside the word, not on the whole-buffer index,
            // so the tail of a partial word is still addressed correctly.
            let word = index & !3;
            let lane = (index & 3) ^ 3;
            captured[word + lane]
        })
        .collect()
}

/// Executes the fill at `index` of the plan's own fill list against the
/// accumulated buffer, returning its completion and its declared accesses.
fn execute_scheduled_fill(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: usize,
    accumulated: Option<Vec<u8>>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (
        _,
        fill,
        fill_other_mode,
        fill_scissor,
        fill_combine,
        fill_env_color,
        fill_prim_color,
        fill_blend_color,
        fill_fog_color,
    ) = &collector.plan.fills[index];
    let fill_seed_access = &fill.seed_access_index;

    // This fill's OWN `OtherMode`, snapshotted at its stream position --
    // not the walk's running value, which a later `SetOtherMode` (a
    // following texture rectangle switching to Copy cycle, say) would have
    // already overwritten with a mode the fill never ran under.
    let Some(other_mode) = *fill_other_mode else {
        return Err(WgpuRawDpcExecutionError::FillExecution(
            FillExecutionError::NotFillCycle,
        ));
    };

    // **The same scissor the texrect path already honours, from the same
    // latched state and through the same whole-target fallback.**
    // Pinned RT64 clips by intersecting its current scissor and draw
    // rectangle (`src/hle/rt64_rdp.cpp:1214-1223`, commit `f0728a2`). Reusing
    // `texrect_scissor_or_full_target` rather than writing a second
    // fallback keeps one model of "no SetScissor means the whole target".
    let scissor = texrect_scissor_or_full_target(*fill_scissor, candidate.key().extent());

    // **The seed for a partial fill's untouched pixels.**
    //
    // Preference order, and each rung is a different fact rather than a
    // fallback chain looking for something that works:
    //
    // 1. `accumulated` -- an earlier command in THIS packet already
    //    composed into the buffer, so it is the freshest content and
    //    already in device byte order.
    // 2. the declared colour-image seed read -- the guest's own framebuffer
    //    bytes, which is what hardware leaves outside a fill and what
    //    `fn64-render-reference` loads (`backend/imp.rs:440-447`).
    // 3. `None` -- the fill is full extent and declared no seed, so every
    //    byte comes from the command itself.
    //
    // A declared seed that failed to thread is NOT silently downgraded to
    // `None`: that would resurrect the fabricated zeros this whole path
    // exists to remove, and the differential measured them as
    // `wgpu: 0x0000` against a `0xffff` key. `MissingSeedBytes` names it.
    let seed = match (accumulated, fill_seed_access) {
        (Some(bytes), _) => Some(bytes),
        (None, Some(access_index)) => {
            let access_position = usize::try_from(*access_index).map_err(|_| {
                WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                }
            })?;
            let expected = collector
                .plan
                .accesses
                .get(access_position)
                .copied()
                .ok_or(WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                })?;
            let captured = collector.reads.bytes(*access_index, expected).ok_or(
                WgpuRawDpcExecutionError::MissingFillSeedBytes {
                    access_index: *access_index,
                },
            )?;
            Some(logical_bytes_from_captured_rdram(captured))
        }
        (None, None) => None,
    };

    let rectangle = FillRectangle::from_wire_fields(
        fill.upper_left_x,
        fill.upper_left_y,
        fill.lower_right_x,
        fill.lower_right_y,
    );
    let completed = match other_mode.cycle_type() {
        CycleType::Fill => execute_fill_rectangle_owned(
            candidate,
            other_mode,
            FillColor::from_wire(
                fill.fill_color
                    .ok_or(FillExecutionError::MissingCombinedState {
                        register: "SetFillColor",
                    })?
                    .value,
            ),
            rectangle,
            scissor,
            seed,
        )?,
        CycleType::OneCycle | CycleType::TwoCycle => execute_combined_fill_rectangle_owned(
            candidate,
            other_mode,
            (*fill_combine).ok_or(FillExecutionError::MissingCombinedState {
                register: "SetCombine",
            })?,
            *fill_env_color,
            *fill_prim_color,
            *fill_blend_color,
            *fill_fog_color,
            rectangle,
            scissor,
            seed,
        )?,
        CycleType::Copy => {
            return Err(WgpuRawDpcExecutionError::FillExecution(
                FillExecutionError::UnsupportedCombinedCycle {
                    cycle_type: CycleType::Copy,
                },
            ))
        }
    };
    let accesses = fill_accesses(&collector.plan.accesses, fill)?.to_vec();
    Ok((completed, accesses))
}

fn decode_scheduled_raw_triangle(
    collector: &ExecutionCollector<'_>,
    index: CommandIndex,
) -> Result<crate::raw_dpc::RawTriangle, WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    decoded_scheduled_raw_triangle(scheduled)
}

fn decoded_scheduled_raw_triangle(
    scheduled: &ScheduledRawTriangle,
) -> Result<crate::raw_dpc::RawTriangle, WgpuRawDpcExecutionError> {
    scheduled.decoded.map_err(
        |_| WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable {
            triangle_index: scheduled.triangle_index.get(),
        },
    )
}

fn scheduled_raw_triangle_accesses(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
) -> Result<Vec<ResourceAccess>, WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    let start = scheduled.span.first_access_index as usize;
    let end = start
        .checked_add(scheduled.span.access_count as usize)
        .ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: scheduled.triangle_index.get(),
        })?;
    let accesses = collector
        .plan
        .accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: scheduled.triangle_index.get(),
        })?
        .to_vec();
    verify_accesses_inside(&accesses, candidate.key())?;
    Ok(accesses)
}

/// Executes the flat raw triangle at `index` of the plan's own
/// `raw_triangle_commands` list against the accumulated buffer, returning
/// its completion and its declared accesses.
///
/// Every geometric fact is taken from the decoder, never re-derived: the
/// exact edge coefficients carried from the command's own authoritative wire
/// words, and the declared write run from the span the decoder recorded when
/// it pushed those accesses. The one number this function computes itself is
/// the declared row COUNT, which it hands the executor so the executor can
/// prove its own raster covers exactly those rows.
#[allow(clippy::too_many_arguments)]
fn execute_scheduled_raw_triangle<S: crate::TmemByteSource + ?Sized>(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: CommandIndex,
    resident_bytes: Cow<'_, [u8]>,
    tmem: &S,
    expect_proposed: bool,
    depth_cells: Option<&mut [crate::targets::DepthCell]>,
    coverage: Option<ColorCoverageState>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let scheduled = &collector.plan.raw_triangle_commands[index];
    let triangle_index = scheduled.triangle_index;
    let draw_state = collector.plan.triangles[triangle_index]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;

    // **Decoded once from this command's own authoritative wire words**, then
    // carried exactly rather than reconstructed from the projected
    // `NeutralTriangleVertex` triple. The projection is screen-space and
    // lossy; it cannot recover dxhdy/dxmdy/dxldy at all.
    let triangle = decode_scheduled_raw_triangle(collector, index)?;
    let accesses = scheduled_raw_triangle_accesses(collector, candidate, index)?;

    // **The tile binding, for a TEXTURED triangle only, resolved from the
    // triangle's OWN wire tile field.**
    //
    // `RawTriangle::tile()` is wire word 0 bits 18:16 -- a real field the
    // triangle carries. `PlanCollector`'s `bound_tile_index` reads the same
    // field from the same word for the GPU uniform path, so the two paths
    // agree by construction; reading it here from the decoded command is the
    // same shape `execute_scheduled_texrect` already uses for a texrect,
    // which reads its own tile from its own wire word rather than the
    // uniform.
    //
    // The tile TABLE is `triangle_neutral_tiles`, the snapshot taken at this
    // triangle's own stream position -- so a packet that re-tiles between
    // draws gives each draw its own binding. Absent means no `SetTile`/
    // `SetTileSize` was staged for that index at this position: refused by
    // name, never defaulted to a zeroed tile that would silently sample TMEM
    // word zero.
    let texture = if triangle.flags().textured() {
        let tile_index = usize::from(triangle.tile().get());
        let (Some(descriptor), Some(size)) =
            collector.plan.triangles[triangle_index].neutral_tiles[tile_index]
        else {
            return Err(WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: triangle_index.get(),
            });
        };
        let tile = crate::targets::TexrectTileBinding::try_from_neutral(descriptor, size)
            .map_err(|_| WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: triangle_index.get(),
            })?;
        // High bit 15 is `en_tlut`, bit 14 `tlut_type`; with the enable bit
        // clear fn64 treats the TLUT as off. Pinned RT64 likewise maps only
        // the exact `G_TT_RGBA16` and `G_TT_IA16` values to a TLUT and maps
        // every other value to `None`
        // (`src/hle/rt64_rdp_tmem.cpp:176-185`, commit `f0728a2`).
        let lut_mode = draw_state.other_mode.texture_lut_mode();
        // The image this call was handed must answer the identity its CALLER
        // selected: a pending post-image answers `Proposed`, durable state
        // answers `Committed`. Checked here rather than trusted, exactly as
        // `execute_scheduled_texrect` checks it and for the same reason --
        // both variants inhabit one enum, so a wrong `snapshot()` impl
        // compiles.
        verify_tmem_identity(tmem, expect_proposed, triangle_index.get())?;
        Some(crate::targets::RawTriangleTexture {
            tile,
            tmem,
            lut_mode,
        })
    } else {
        None
    };

    let shading = crate::targets::TexrectShading::new(
        draw_state.combine_params,
        draw_state.env_color,
        draw_state.prim_color,
    );
    let blend_registers =
        crate::targets::TexrectBlendRegisters::new(draw_state.blend_color, draw_state.fog_color);
    // **The z-buffer wiring, present only when a depth accumulator was
    // threaded (i.e. this packet bound a z-image).** The compare/update
    // gates and the z source are read from THIS draw's own snapshotted
    // `OtherMode`, and the primitive depth from its snapshotted
    // `SetPrimDepth` -- both the same command-time-snapshot values every
    // other register on `draw_state` uses. Absent depth cells means no
    // z-image was bound, so the draw resolves by painter's order exactly
    // as before this change.
    let depth = depth_cells.map(|cells| crate::targets::RawTriangleDepth {
        cells,
        compare: draw_state.other_mode.depth_compare_enabled(),
        update: draw_state.other_mode.depth_update_enabled(),
        mode: draw_state.other_mode.depth_mode(),
        source_is_primitive: draw_state.other_mode.primitive_depth_source(),
        prim_depth: draw_state.prim_depth,
    });
    let completed = match coverage {
        Some(coverage) => crate::targets::execute_raw_triangle_with_coverage(
            candidate,
            draw_state.other_mode,
            &triangle,
            shading,
            blend_registers,
            resident_bytes,
            &accesses,
            texture,
            depth,
            coverage,
        )?,
        None => crate::targets::execute_raw_triangle(
            candidate,
            draw_state.other_mode,
            &triangle,
            shading,
            blend_registers,
            resident_bytes,
            &accesses,
            texture,
            depth,
        )?,
    };
    Ok((completed, accesses))
}

/// Executes the texrect at `index` of the plan's own texrect list against
/// the accumulated buffer, returning its completion and declared accesses.
///
/// Every geometric fact is taken from the decoder, never re-derived: the
/// pixel extent from the `RectViewportPixels` `texture_rectangle_vertices`
/// produced, and the declared write run from the span the decoder recorded
/// when it pushed those accesses.
#[allow(clippy::too_many_arguments)]
fn execute_scheduled_texrect<S: crate::TmemByteSource + ?Sized>(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    tmem: &S,
    expect_proposed: bool,
    index: usize,
    resident_bytes: Cow<'_, [u8]>,
    already_initialized: Option<TargetRectangle>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (span, triangle_index, tile_index, _, flipped_axes) =
        collector.plan.texrect_commands[index];
    // A texrect that declared no write must not execute: it would write
    // bytes the journal never declared, which `merged_fill_and_tmem_writes`
    // would then reject as `MergedWriteUndeclared` -- a correct but less
    // specific diagnosis than naming the real cause here.
    let span = span.ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?;

    let draw_state = collector.plan.triangles[triangle_index]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
    let viewport = draw_state
        .viewport
        .ok_or(WgpuRawDpcExecutionError::TexrectMissingViewport { triangle_index })?;
    let other_mode = draw_state.other_mode;

    // The complete neutral tile pair, not the GPU `TileBindingParams`,
    // because the CPU reader's indexed path needs `palette` and that
    // uniform layout has no such field. Absent means no `SetTile`/
    // `SetTileSize` was staged at this texrect's own stream position --
    // refused by name, never defaulted to a zeroed tile that would
    // silently sample TMEM word zero.
    let (Some(descriptor), Some(size)) =
        collector.plan.triangles[triangle_index].neutral_tiles[usize::from(tile_index)]
    else {
        return Err(WgpuRawDpcExecutionError::TexrectUnboundTile { triangle_index });
    };
    let tile = crate::targets::TexrectTileBinding::try_from_neutral(descriptor, size)
        .map_err(|_| WgpuRawDpcExecutionError::TexrectUnboundTile { triangle_index })?;

    // The two opposite texcoord corners `texture_rectangle_vertices`
    // produced. Its six-vertex texcoord order is
    // `[u1v1, u2v1, u1v2, u2v2, u2v1, u1v2]` (the unflipped arm of its own
    // `triTcFloats` push order), and `production_adapter` splits those six
    // into triangles `0,1,2` and `3,4,5`. So the upper-left pair `(u1, v1)`
    // is vertex 0 = first triangle's index 0, and the lower-right pair
    // `(u2, v2)` is vertex **3** = the SECOND triangle's index **0**.
    //
    // Not index 2 of the second triangle: that is vertex 5, whose texcoord
    // is `(u1, v2)` -- the lower-LEFT corner. Measured, not reasoned:
    // reading index 2 gave an S span of 0 (`lr = [0.0, 0.75]`), every pixel
    // in a row sampled texel 0, and the committed-TMEM oracle disagreed at
    // the first pixel of row 1.
    let upper_left = draw_state.vertices[0].texcoord;
    let second = collector.plan.triangles[triangle_index + 1]
        .draw
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
    let lower_right = second.vertices[0].texcoord;
    let mut draw = crate::targets::TexrectDraw::try_from_viewport_and_texcoords(
        viewport,
        upper_left,
        lower_right,
    )?;
    if flipped_axes {
        draw = draw.with_flipped_axes();
    }

    // Locate the declared run by the decoder's own recorded span.
    let start = span.first_access_index as usize;
    let end = start
        .checked_add(span.access_count as usize)
        .ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?;
    let accesses = collector
        .plan
        .accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?
        .to_vec();

    // Cross-check, not an assumption: every declared access must fall
    // inside the candidate key's own range, which was derived from the
    // packet's `SetColorImage` by a path independent of the decoder's
    // `plan_render_target_rows`.
    let key = candidate.key();
    verify_accesses_inside(&accesses, key)?;

    // High bit 15 is `en_tlut`, bit 14 `tlut_type`; with the enable bit clear
    // fn64 treats the TLUT as off. Pinned RT64 likewise maps only the exact
    // `G_TT_RGBA16` and `G_TT_IA16` values to a TLUT and maps every other
    // value to `None` (`src/hle/rt64_rdp_tmem.cpp:176-185`, commit
    // `f0728a2`).
    let lut_mode = other_mode.texture_lut_mode();
    // **The committed/pending distinction, asserted where it is crossed.**
    //
    // The image this call was handed must answer the identity its *caller*
    // selected, not merely a well-formed one: a pending post-image answers
    // `TmemSnapshotIdentity::Proposed`, durable state answers `Committed`.
    // Checked here rather than trusted, because the type system cannot:
    // both variants inhabit one enum, so a wrong `snapshot()` impl
    // compiles. Measured: forging `Committed` in `PendingTmemImage`'s impl
    // passed the entire suite before this check existed.
    //
    // Both directions are checked, not just one. A committed image reaching
    // the load-bearing packet's arm would mean the texrect silently missed
    // its own packet's loads; a proposed image reaching the load-free arm
    // would mean a proposal was fabricated for a packet that staged none.
    verify_tmem_identity(tmem, expect_proposed, triangle_index)?;
    // The one-cycle shading state, taken from the SAME
    // `RetrievedTriangleDraw` snapshot the other-mode, viewport and tile
    // above came from -- i.e. the registers current at THIS texrect's own
    // stream position. That per-command sourcing is what lets a packet
    // carry several one-cycle texrects each running a different combiner
    // program against the accumulated buffer: the schedule loop calls this
    // function once per texrect, and each call reads its own snapshot
    // rather than the walk's running final value.
    //
    // `combine_params` is non-optional on that struct (a triangle with no
    // `SetCombine` fails retrieval with `MissingTriangleDrawState` before
    // reaching here); `env_color`/`prim_color` are `Option` because their
    // registers may genuinely be unset, and the executor refuses only when
    // the program actually reads the unset one -- and only in one-cycle,
    // since Copy consults no program at all.
    let shading = crate::targets::TexrectShading::new(
        draw_state.combine_params,
        draw_state.env_color,
        draw_state.prim_color,
    );
    // The blender's two color registers, taken from the SAME
    // `RetrievedTriangleDraw` snapshot the combiner's registers above came
    // from -- i.e. the values current at THIS texrect's own stream
    // position, not the walk's running final value.
    let blend_registers =
        crate::targets::TexrectBlendRegisters::new(draw_state.blend_color, draw_state.fog_color);
    // The scissor current at THIS texrect's own stream position, from the
    // same `RetrievedTriangleDraw` snapshot as the registers above.
    //
    // **The fallback when the plan issued no `SetScissor` is the whole
    // colour target, not an empty rect and not a refusal.** The RDP's clip
    // registers hold a value from reset onward; a display list that never
    // writes them draws unscissored, and refusing it here would refuse
    // every stream that relies on the boot-time rect. The target extent is
    // this consumer's own honest widest bound -- which is exactly why
    // `RetrievedTriangleDraw::scissor` leaves the fallback to the consumer
    // instead of defaulting in the collector, which does not know it.
    //
    // Quarter-pixels, because public libultra's `gDPSetScissor` encodes each
    // coordinate after multiplying it by four
    // (`include/ultra64/gbi.h:4794-4804`); `extent` is in pixels, so it is
    // scaled by four.
    let scissor = texrect_scissor_or_full_target(draw_state.scissor, candidate.key().extent());
    let completed = crate::targets::execute_texture_rectangle(
        candidate,
        other_mode,
        draw,
        tile,
        tmem,
        lut_mode,
        shading,
        blend_registers,
        scissor,
        resident_bytes,
        already_initialized,
    )?;
    Ok((completed, accesses))
}

/// The scissor one texrect is clipped against: the rect latched at its own
/// stream position, or the whole colour target when the plan has issued
/// none.
///
/// **The fallback is the target's own extent, not an empty rect and not a
/// refusal.** The RDP's clip registers hold a value from reset onward, so a
/// display list that never writes them draws unscissored; refusing such a
/// stream would refuse every one that relies on the boot-time rect. The
/// target extent is this consumer's honest widest bound -- which is exactly
/// why `RetrievedTriangleDraw::scissor` leaves the fallback to the
/// consumer rather than defaulting in the collector, which does not know
/// the extent.
///
/// Quarter-pixels, because public libultra's `gDPSetScissor` encodes each
/// coordinate after multiplying it by four
/// (`include/ultra64/gbi.h:4794-4804`), while `extent` is in pixels -- hence
/// the factor of four on each axis.
///
/// A named function rather than an inline closure so a mutation that
/// derives the height bound from the width is reachable from a unit test;
/// while it was inline, exactly that mutation left the whole suite green.
fn texrect_scissor_or_full_target(
    latched: Option<crate::targets::RdpScissorRect>,
    extent: crate::targets::ColorTargetExtent,
) -> crate::targets::RdpScissorRect {
    latched.unwrap_or_else(|| {
        let quarter = |pixels: u32| u16::try_from(pixels.saturating_mul(4)).unwrap_or(u16::MAX);
        crate::targets::RdpScissorRect::from_wire_quarter_pixels(
            0,
            0,
            0,
            quarter(extent.width()),
            quarter(extent.height()),
        )
    })
}

/// **One TMEM projection per admitted triangle, each taken at that
/// triangle's own stream position, through the same one projection the
/// committed path uses.**
///
/// ## Why per triangle and not per packet
///
/// The GPU half had the CPU half's old defect one layer up: a single
/// projection was sealed per `draw_admitted_triangles` call and shared by
/// every triangle in it. Within one packet TMEM is not one image --
/// WM2000's measured sixth packet interleaves seven `LoadTile`s with seven
/// texrects, all loading from TMEM word zero, so a shared projection holds
/// only the seventh load's texels and the raster draws the seventh sprite
/// seven times.
///
/// The position rule is `prefix_before`, **called here rather than
/// reimplemented**, so the CPU texel reader and the GPU raster cannot
/// disagree about which load a given command observed. Both arms match the
/// CPU side exactly: a prefix when a load precedes the triangle, durable
/// committed state when none does.
///
/// A texture rectangle's two triangles carry the same
/// `plan.triangle_commands` entry, so both halves of one rectangle project
/// the same image and cannot split across a load.
///
/// ## Cost
///
/// Bounded by the triangle count, and `submit_triangles` already creates a
/// TMEM bytes buffer and validity buffer **per fixture**
/// (`triangle_pipeline.rs`'s per-fixture `create_buffer`/`write_buffer`
/// pair), so this changed *what* each fixture uploads, not how much. No
/// pipeline, bind-group, or shader change was required.
///
/// ## What it does not relax
///
/// Every entry is `project_tmem`, at `S = PendingTmemPrefixImage` for the
/// prefix arm and `S = PhysicalTmemState` for the committed one. There is
/// no second address walk, no second validity gate, and no second bitmap
/// packing, and the CPU texel reader reaches the same two sources through
/// the same `TmemByteSource`. A pending/published *disagreement* about how
/// bytes reach the shader is therefore unrepresentable rather than merely
/// tested for -- the only thing that differs between the two calls is which
/// image is handed in.
///
/// **What the published-slot projection was protecting, and how this
/// preserves it.** The `project_committed_tmem(coordinator.physical())`
/// call the pending projection replaced was not arbitrary: reading the active
/// slot guaranteed the GPU could only ever sample bytes some publication had
/// actually durably installed, so no GPU-observed pixel could be attributed
/// to a proposal that publication later rejected. That guarantee is real and
/// it is preserved here in the same way the CPU texel reader preserves it --
/// not by refusing to read pending bytes, but by refusing to let a pending
/// read *claim* to be a durable one. The check below is
/// `execute_scheduled_texrect`'s check, at the other crossing:
///
/// - **No publication.** This borrows the transaction and copies bytes out.
///   It cannot commit, cannot advance a generation, and cannot produce a
///   `PhysicalTmemState`. `into_physical_successor` still runs afterwards
///   with every base-state, generation, epoch and backend-effect check, and
///   the sealed-proposal revalidation remains available diagnostically. If
///   any check rejects, this packet's
///   `execute_raw_dpc` returns `Err` and no draw output is stored -- the
///   pixels never become observable.
/// - **No forged snapshot identity.** Verified, not trusted: both
///   `TmemSnapshotIdentity` variants inhabit one enum, so a wrong
///   `snapshot()` impl compiles. Measured at the sibling site -- forging
///   `Committed` in `PendingTmemImage`'s impl passed the entire suite before
///   `execute_scheduled_texrect`'s equivalent check existed.
/// - **No effect-report participation.** Reading is not a write. Nothing
///   projected here enters `proposed_effects`, so the sealed proposal and
///   `validate_backend_effects`' supersequence walk see exactly what they saw
///   before.
///
/// Nonclaim: the returned `TmemGpuProjection` carries bytes, not identity.
/// It is not a receipt and nothing downstream may read a publication out of
/// it; the identity assertion happens here, at the crossing, and does not
/// travel with the bytes.
fn project_pending_tmem_per_triangle(
    triangle_commands: &[u32],
    prefixes: &[(u32, crate::tmem::TmemPrefixSnapshot)],
    pending: &crate::tmem::PendingTmemTransaction,
    committed: &PhysicalTmemState,
) -> Result<Vec<TmemGpuProjection>, WgpuRawDpcExecutionError> {
    triangle_commands
        .iter()
        .map(
            |&command_index| match prefix_before(prefixes, command_index) {
                Some(prefix) => project_proposed_image(&pending.prefix_image(prefix)?),
                // No load precedes this triangle in its own packet, so durable
                // committed state is what TMEM holds at its position -- the
                // same answer, from the same fact, that `stage_color_commands`
                // gives a texrect in the same position. Not a fallback for a
                // missing image: the absence of a preceding load IS the stream
                // fact that makes committed correct.
                None => Ok(project_committed_tmem(committed)),
            },
        )
        .collect()
}

/// **The committed/pending identity crossing, both directions, at one
/// site.**
///
/// A texrect's TMEM image must answer the identity its caller
/// *selected*, not merely a well-formed one. `expect_proposed` is
/// `TexrectTmemSource`'s own choice, made per packet from
/// `plan.loads.is_empty()`; this checks the image agrees.
///
/// Checked rather than trusted because the type system cannot: `Committed`
/// and `Proposed` inhabit one enum, so a wrong `snapshot()` impl compiles.
/// Measured on the pending direction: forging `Committed` in
/// `PendingTmemImage`'s impl passed the entire suite before that half
/// existed.
///
/// Both directions, not one. A committed image reaching a load-bearing
/// packet would mean the texrect silently missed its own packet's loads --
/// the `TMEM_SAMPLE_STATUS_INVALID_BYTE` defect commit `3a1a6a73` measured.
/// A proposed image reaching a load-free packet would mean a proposal was
/// fabricated for a packet that staged none.
///
/// Split out from `execute_scheduled_texrect` so a lying source can reach
/// it: no real image can, since both real impls are correct, and a refusal
/// with no test is a claim with no evidence.
fn verify_tmem_identity<S: crate::TmemByteSource + ?Sized>(
    tmem: &S,
    expect_proposed: bool,
    triangle_index: usize,
) -> Result<(), WgpuRawDpcExecutionError> {
    let snapshot = crate::TmemByteSource::snapshot(tmem);
    match (expect_proposed, snapshot.is_committed()) {
        (true, true) => {
            Err(WgpuRawDpcExecutionError::PendingTmemImageClaimedCommitted { triangle_index })
        }
        (false, false) => {
            Err(WgpuRawDpcExecutionError::CommittedTmemImageClaimedProposed { triangle_index })
        }
        _ => Ok(()),
    }
}

/// [`project_pending_tmem_per_triangle`]'s per-entry body, generic over the
/// source, so the forgery refusal can be exercised by a test.
///
/// Split out for exactly one reason: `PendingTmemImage`'s own `snapshot()`
/// impl is correct, so no *real* pending image can drive the refusal, and a
/// refusal with no test is a claim with no evidence -- this crate's own
/// convention (see `merged_fill_and_tmem_writes`' two loud arms, tested at
/// the function for the same reason). A test source that answers
/// `Committed` is the only way to reach the arm, and it is reachable only
/// through this generic seam, never from production code: the sole
/// production caller above passes a real `PendingTmemPrefixImage`.
fn project_proposed_image<S: crate::TmemByteSource + ?Sized>(
    image: &S,
) -> Result<TmemGpuProjection, WgpuRawDpcExecutionError> {
    if image.snapshot().is_committed() {
        return Err(WgpuRawDpcExecutionError::PendingTmemProjectionClaimedCommitted);
    }
    Ok(crate::project_tmem(image))
}

/// Every declared access must fall inside the color target's own range.
///
/// The decoder computed those ranges from the same `SetColorImage` by an
/// independent path (`plan_render_target_rows`), so agreement here is real
/// evidence the two derivations match, and disagreement is a loud refusal
/// rather than a write landing outside the target the registry tracks.
fn verify_accesses_inside(
    accesses: &[ResourceAccess],
    key: ColorTargetKey,
) -> Result<(), WgpuRawDpcExecutionError> {
    let target = key.range();
    for access in accesses {
        let inside = match access.region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                range.start().get() >= target.start().get()
                    && range.start().get() + range.len() <= target.start().get() + target.len()
            }
            _ => false,
        };
        if !inside {
            return Err(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                access_index: access.operation().get(),
            });
        }
    }
    Ok(())
}

/// This fill's own declared access run, located by the
/// `first_access_index`/`access_count` span the decoder recorded when it
/// pushed them -- never re-derived from the rectangle's geometry, which
/// would be a second independent derivation of the same fact.
fn fill_accesses<'plan>(
    accesses: &'plan [ResourceAccess],
    fill: &fn64_render::RdpFillRectangleCommand,
) -> Result<&'plan [ResourceAccess], WgpuRawDpcExecutionError> {
    let start = fill.first_access_index as usize;
    let end = start.checked_add(fill.access_count as usize).ok_or(
        WgpuRawDpcExecutionError::FillAccessOutsideTarget {
            access_index: fill.first_access_index,
        },
    )?;
    accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
            access_index: fill.first_access_index,
        })
}

/// Derives the exact ordered `CompletedWrite` list for one admitted
/// FillRectangle, one per `ResourceAccess` the decoder declared for it.
///
/// Each write's `byte_count` is its own access's
/// `region().declared_bytes()` and its `content` digest covers exactly those
/// bytes, sliced out of the full-extent `DeviceColorBytes` the executor
/// produced. Deliberately **not** one digest over the whole buffer: a
/// `CompletedWrite` claims "these declared_bytes at this range now hold
/// content with this digest", and hashing bytes outside the declared range
/// would make the claim unfalsifiable for the range it names while silently
/// importing untouched inter-row bytes into the hash of a range that does
/// not contain them.
///
/// The full-extent buffer is the *source*; each per-row access's own
/// declared span is the *unit*. Those are two different invariants -- the
/// registry needs a complete byte buffer for the resident generation, the
/// journal needs exactly the bytes the fill claims -- and this function is
/// where they are correctly decoupled.
fn fill_completed_writes(
    key: ColorTargetKey,
    device_bytes: &crate::DeviceColorBytes,
    accesses: &[ResourceAccess],
) -> Result<Vec<CompletedWrite>, WgpuRawDpcExecutionError> {
    let base = key.address().get();
    let buffer = device_bytes.device_bytes();
    accesses
        .iter()
        .map(|access| {
            let range = match access.region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => range,
                _ => {
                    return Err(WgpuRawDpcExecutionError::FillAccessRegionKind {
                        access_index: access.operation().get(),
                    })
                }
            };
            // Physical -> buffer-relative. The access range is a subrange of
            // the target's own range by construction (both derive from the
            // same `SetColorImage` address/width), but that is re-checked
            // here rather than assumed.
            let start = range.start().get().checked_sub(base).ok_or(
                WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                    access_index: access.operation().get(),
                },
            )? as usize;
            let len = range.len() as usize;
            let slice = start
                .checked_add(len)
                .and_then(|end| buffer.get(start..end))
                .ok_or(WgpuRawDpcExecutionError::FillAccessOutsideTarget {
                    access_index: access.operation().get(),
                })?;
            CompletedWrite::try_from_bytes(*access, slice).map_err(WgpuRawDpcExecutionError::Effect)
        })
        .collect()
}

fn image_format(format: fn64_render::NeutralImageFormat) -> crate::ImageFormat {
    match format {
        fn64_render::NeutralImageFormat::Rgba => crate::ImageFormat::Rgba,
        fn64_render::NeutralImageFormat::Yuv => crate::ImageFormat::Yuv,
        fn64_render::NeutralImageFormat::ColorIndex => crate::ImageFormat::ColorIndex,
        fn64_render::NeutralImageFormat::IntensityAlpha => crate::ImageFormat::IntensityAlpha,
        fn64_render::NeutralImageFormat::Intensity => crate::ImageFormat::Intensity,
    }
}

/// `image_format`'s inverse: the neutral mirror of a crate-typed
/// [`crate::ImageFormat`].
///
/// Exists so `PlanCollector`'s tile table can be seeded from `RdpState`'s
/// durable, crate-typed `TileState` while the table itself keeps storing
/// neutral mirrors -- the shape both its consumers
/// (`TileBindingParams::from_neutral` for the GPU uniform,
/// `TexrectTileBinding::try_from_neutral` for the CPU reader) already read.
/// Converting the *seed* is a five-function addition; converting the table
/// would mean a new typed-to-uniform path for the GPU binding, which is a
/// different change.
fn neutral_image_format(format: crate::ImageFormat) -> fn64_render::NeutralImageFormat {
    match format {
        crate::ImageFormat::Rgba => fn64_render::NeutralImageFormat::Rgba,
        crate::ImageFormat::Yuv => fn64_render::NeutralImageFormat::Yuv,
        crate::ImageFormat::ColorIndex => fn64_render::NeutralImageFormat::ColorIndex,
        crate::ImageFormat::IntensityAlpha => fn64_render::NeutralImageFormat::IntensityAlpha,
        crate::ImageFormat::Intensity => fn64_render::NeutralImageFormat::Intensity,
    }
}

/// `pixel_size`'s inverse; see [`neutral_image_format`].
fn neutral_pixel_size(size: crate::PixelSize) -> fn64_render::NeutralPixelSize {
    match size {
        crate::PixelSize::Bits4 => fn64_render::NeutralPixelSize::Bits4,
        crate::PixelSize::Bits8 => fn64_render::NeutralPixelSize::Bits8,
        crate::PixelSize::Bits16 => fn64_render::NeutralPixelSize::Bits16,
        crate::PixelSize::Bits32 => fn64_render::NeutralPixelSize::Bits32,
    }
}

/// The neutral mirror of one durable [`crate::TileDescriptor`], field for
/// field. The inverse of `TexrectTileBinding::try_from_neutral`'s own
/// decode, and deliberately total: every field on the neutral mirror has
/// exactly one accessor on the typed descriptor, so there is nothing to
/// default and nothing to drop.
fn neutral_tile_descriptor(
    descriptor: crate::TileDescriptor,
) -> fn64_render::NeutralTileDescriptor {
    fn64_render::NeutralTileDescriptor {
        format: neutral_image_format(descriptor.format()),
        size: neutral_pixel_size(descriptor.size()),
        line_words: descriptor.line_words(),
        tmem_word_address: descriptor.tmem().get(),
        palette: descriptor.palette(),
        s_mode: fn64_render::NeutralTileAddressMode {
            mirror: descriptor.s_mode().mirror(),
            clamp: descriptor.s_mode().clamp(),
        },
        mask_s: descriptor.mask_s(),
        shift_s: descriptor.shift_s(),
        t_mode: fn64_render::NeutralTileAddressMode {
            mirror: descriptor.t_mode().mirror(),
            clamp: descriptor.t_mode().clamp(),
        },
        mask_t: descriptor.mask_t(),
        shift_t: descriptor.shift_t(),
    }
}

/// The neutral mirror of one durable [`crate::TileSize`], in the same raw
/// 10.2 fixed-point encoding the neutral struct documents.
fn neutral_tile_size(size: crate::TileSize) -> fn64_render::NeutralTileSize {
    fn64_render::NeutralTileSize {
        low_s: size.low_s().raw(),
        low_t: size.low_t().raw(),
        high_s: size.high_s().raw(),
        high_t: size.high_t().raw(),
    }
}

/// `RdpState`'s eight durable tile registers as the neutral pair
/// `PlanCollector` tracks, for seeding one packet's walk.
///
/// A tile with no `SetTile` or no `SetTileSize` ever issued stays `None` on
/// that half -- absence is carried through, never defaulted to a zeroed
/// descriptor that would silently sample TMEM word zero.
fn durable_neutral_tiles(
    state: &RdpState,
) -> [(
    Option<fn64_render::NeutralTileDescriptor>,
    Option<fn64_render::NeutralTileSize>,
); 8] {
    let mut tiles = [(None, None); 8];
    for (index, slot) in tiles.iter_mut().enumerate() {
        let Ok(tile_index) = crate::TileIndex::try_new(index as u8) else {
            continue;
        };
        let tile = state.tmem().tile(tile_index);
        slot.0 = tile.descriptor().map(neutral_tile_descriptor);
        slot.1 = tile.size().map(neutral_tile_size);
    }
    tiles
}

fn pixel_size(size: fn64_render::NeutralPixelSize) -> crate::PixelSize {
    match size {
        fn64_render::NeutralPixelSize::Bits4 => crate::PixelSize::Bits4,
        fn64_render::NeutralPixelSize::Bits8 => crate::PixelSize::Bits8,
        fn64_render::NeutralPixelSize::Bits16 => crate::PixelSize::Bits16,
        fn64_render::NeutralPixelSize::Bits32 => crate::PixelSize::Bits32,
    }
}

#[cfg(test)]
mod tests;
