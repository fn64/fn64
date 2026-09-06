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
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcBackend, RawDpcCoordinator,
    RawDpcExecutionBatch, RawDpcExecutionView, RawDpcIrCapability, RawDpcPlanRequest,
    RawDpcSemanticCommandRef, RawDpcTaskBatchCapability, RdpStateCommand, RdpTriangleCommand,
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

mod capture;
mod census;
mod color;
mod convert;
mod execute;
mod plan;
mod state;

pub(self) use census::{
    raw_dpc_execute_census, raw_dpc_plan_census, task_compute_census, task_cpu_phase_census,
};
pub use census::{task_cpu_phase_running_totals, TaskCpuPhaseRunningTotals};
#[cfg(test)]
pub(self) use plan::single_source_probe_journal;
#[cfg(test)]
pub(self) use plan::{
    classify_no_raw_triangle_flags, finalize_with_zero_reads, submit_locally,
    ScheduledRawTriangleDecodeError, TriangleIndex,
};
pub(self) use plan::{
    plan_raw_dpc_inner, transaction_sequence, CommandIndex, PlanCollector, ScheduledRawTriangle,
};

#[cfg(test)]
pub(self) use capture::IndexedCapturedGuestRead;
pub(self) use capture::{
    CapturedGuestReadAuthority, CapturedGuestReadBytes, TaskGuestReadCapturePool,
};
#[cfg(test)]
pub(self) use state::PublishedVisualTargetMarker;
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
pub use state::{ComputeRasterProbeReceipt, WgpuBackend, WgpuBackendConstructionError};

pub(self) use convert::{
    decode_scheduled_raw_triangle, execute_scheduled_fill, execute_scheduled_raw_triangle,
    execute_scheduled_texrect, fill_completed_writes, scheduled_raw_triangle_accesses,
    verify_accesses_inside,
};
#[cfg(test)]
pub(self) use convert::{
    decoded_scheduled_raw_triangle, project_proposed_image, texrect_scissor_or_full_target,
    verify_tmem_identity,
};
pub use execute::WgpuRawDpcExecutionError;
pub(self) use execute::{
    admit_task_compute_member, complete_deferred_compute_segment,
    compute_program_attribution_from_ids, compute_segment_program_attribution,
    execute_raw_dpc_inner, ComputeEligibleTaskMember, DeferredComputeColor, ExactCheckpointImages,
    ExecutionCollector, StagedFill, StagedOutcome, StagedRawDpcMember, TaskComputeCpuReason,
    TaskComputeDisposition, TaskMemberDispatch,
};
#[cfg(test)]
pub(self) use execute::{
    compute_program_attribution_from_members, merged_fill_and_tmem_writes, word_source_bytes,
};

pub use color::TaskComputeAdmissionRefusal;
pub(self) use color::{
    claimed_rectangle_from_accesses, compute_column_bounds_enabled,
    logical_bytes_from_captured_rdram, ordered_depth_free_acff_triangle_member, prefix_before,
    shared_copyback_payloads_enabled, stage_color_commands, ComputeRasterReplacementPlan,
    TexrectTmemSource,
};
#[cfg(test)]
pub(self) use color::{
    color_command_input, compute_raster_replacement_admitted, task_cpu_phase_hot_program,
    task_cpu_phase_shape,
};
pub(self) use convert::{
    durable_neutral_tiles, image_format, pixel_size, project_pending_tmem_per_triangle,
};

#[cfg(test)]
mod tests;
