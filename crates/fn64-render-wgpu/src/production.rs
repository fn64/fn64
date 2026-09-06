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
    // pre-existing, and documented in `docs/rt64/RT64-TRIANGLE-WRITEBACK.md`.
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

mod capture;
mod census;
mod plan;
mod state;

pub use census::{task_cpu_phase_running_totals, TaskCpuPhaseRunningTotals};
pub(self) use census::{
    raw_dpc_execute_census, raw_dpc_plan_census, task_compute_census, task_cpu_phase_census,
};
pub(self) use plan::{
    plan_raw_dpc_inner, transaction_sequence, CommandIndex, PlanCollector, ScheduledRawTriangle,
};
#[cfg(test)]
pub(self) use plan::{
    classify_no_raw_triangle_flags, finalize_with_zero_reads, submit_locally,
    ScheduledRawTriangleDecodeError, TriangleIndex,
};
#[cfg(test)]
pub(self) use plan::single_source_probe_journal;

pub use state::{ComputeRasterProbeReceipt, WgpuBackend, WgpuBackendConstructionError};
#[cfg(any(test, feature = "conformance-runner"))]
pub(crate) use state::WgpuCreateError;
pub(self) use state::{
    flush_compute_probe, push_finished_compute_probe, retain_compute_probe_draw,
    ComputeProgramAttribution, ComputeRasterDispatch, ComputeRasterProbe,
    ComputeRasterProbeBuilder, ComputeRasterProbePush, OrderedCpuColorBatch,
    PendingColorPublication, PendingFillPublication, PlannedNoRawTriangleReason,
    PlannedTaskCpuReason, PlannedTaskExecution, RawDpcCarryIn, RdpDrawState,
    COLOR_TARGET_REGISTRY_CAPACITY,
};
pub(self) use capture::{
    CapturedGuestReadAuthority, CapturedGuestReadBytes, TaskGuestReadCapturePool,
};
#[cfg(test)]
pub(self) use capture::IndexedCapturedGuestRead;
#[cfg(test)]
pub(self) use state::PublishedVisualTargetMarker;

#[cfg(test)]
mod tests;
