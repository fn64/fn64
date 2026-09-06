use super::*;

/// Low-overhead decomposition of the production raw-DPC execute phase.
///
/// `FN64_RAW_DPC_EXEC_CENSUS=1` records only submission-boundary clock reads;
/// it never times a pixel. Every 10,000 completed execution views it prints
/// cumulative nested totals. `stage` is inside `view`, and `color` is inside
/// `stage`; the report prints their residuals explicitly so nested time is not
/// accidentally added twice. When disabled, `timed` calls its closure without
/// reading the clock.
pub(super) mod raw_dpc_execute_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    pub(in crate::production) enum Phase {
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

    pub(in crate::production) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
        timed_observed(phase, false, operation).0
    }

    /// Share one coarse phase clock with a task-local observer. `observed`
    /// receives the elapsed value without arming this census's process totals;
    /// when both consumers are disabled the closure runs without a clock read.
    pub(in crate::production) fn timed_observed<R>(
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

pub(super) mod task_cpu_phase_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    #[repr(usize)]
    pub(in crate::production) enum Phase {
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
    pub(in crate::production) enum SourceSubphase {
        PacketCapturedReadBind,
        LoadAccessBind,
        TransactionBegin,
        WordStageAndBlockValidity,
        FinishProjectEffect,
    }

    #[derive(Clone, Copy, Default)]
    pub(in crate::production) struct SourceCounters {
        pub(in crate::production) loads: u64,
        pub(in crate::production) source_fragments: u64,
        pub(in crate::production) words: u64,
        pub(in crate::production) destination_accesses: u64,
        pub(in crate::production) first_loads: u64,
        pub(in crate::production) cumulative_expected_destination_elements: u64,
        pub(in crate::production) projected_destination_bytes: u64,
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

    pub(in crate::production) struct Task {
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
        pub(in crate::production) fn begin(
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

        pub(in crate::production) fn record_member_total(
            &mut self,
            attributed: bool,
            elapsed_ns: Option<u64>,
        ) {
            self.all_cpu_member_ns = self
                .all_cpu_member_ns
                .saturating_add(elapsed_ns.unwrap_or(0));
            if !attributed {
                return;
            }
            self.members = self.members.saturating_add(1);
            self.cpu_member_ns = self.cpu_member_ns.saturating_add(elapsed_ns.unwrap_or(0));
        }

        pub(in crate::production) fn record_compute_segment(&mut self, elapsed_ns: Option<u64>) {
            self.compute_segment_ns = self
                .compute_segment_ns
                .saturating_add(elapsed_ns.unwrap_or(0));
        }

        pub(in crate::production) fn record_member_envelope(
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

        pub(in crate::production) fn publication_finished(mut self) -> Option<Self> {
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

    pub(in crate::production) fn wants_member_clock() -> bool {
        enabled()
    }

    pub(in crate::production) fn task_started() -> Option<std::time::Instant> {
        enabled().then(std::time::Instant::now)
    }

    pub(in crate::production) fn running_totals() -> Option<super::TaskCpuPhaseRunningTotals> {
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

    pub(in crate::production) fn timed<R>(
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

    pub(in crate::production) fn timed_source<R>(
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

    pub(in crate::production) fn record_source_counters(
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

    pub(in crate::production) fn started(
        task: Option<&Task>,
        attributed: bool,
    ) -> Option<std::time::Instant> {
        (attributed && task.is_some()).then(std::time::Instant::now)
    }

    pub(in crate::production) fn record_started(
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
pub(super) mod task_compute_census {
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

    pub(in crate::production) fn segment_started() -> Option<std::time::Instant> {
        (enabled() || super::task_cpu_phase_census::wants_member_clock())
            .then(std::time::Instant::now)
    }

    pub(in crate::production) fn wants_program_attribution() -> bool {
        enabled()
    }

    pub(in crate::production) fn cpu_started() -> Option<std::time::Instant> {
        (enabled() || super::task_cpu_phase_census::wants_member_clock())
            .then(std::time::Instant::now)
    }

    pub(in crate::production) fn timed_registry_clone<R>(
        bytes: usize,
        operation: impl FnOnce() -> R,
    ) -> R {
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

    pub(in crate::production) fn timed_shadow_clone<R>(
        bytes: usize,
        operation: impl FnOnce() -> R,
    ) -> R {
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

    pub(in crate::production) fn record_segment(
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
    pub(in crate::production) fn begin_enabled_segment_test() {
        TEST_ENABLED.with(|enabled| assert!(!enabled.replace(true)));
        COMPUTE_PROGRAMS.with(|programs| programs.borrow_mut().clear());
    }

    #[cfg(test)]
    pub(in crate::production) fn finish_enabled_segment_test(
    ) -> BTreeMap<super::ComputeProgramAttribution, (u64, u64, u64)> {
        TEST_ENABLED.with(|enabled| assert!(enabled.replace(false)));
        COMPUTE_PROGRAMS.with(|programs| core::mem::take(&mut *programs.borrow_mut()))
    }

    pub(in crate::production) fn record_cpu(
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

    pub(in crate::production) fn record_task(members: usize, cpu_members: usize) {
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
pub(super) mod raw_dpc_plan_census {
    use std::sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    };

    #[derive(Clone, Copy)]
    pub(in crate::production) enum Phase {
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

    pub(in crate::production) fn timed<R>(phase: Phase, operation: impl FnOnce() -> R) -> R {
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
