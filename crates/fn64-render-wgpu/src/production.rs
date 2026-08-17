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
//! Scope, matching card v11 and the T3 ticket DAG exactly: TMEM-only,
//! no-FullSync, no-guest-write raw-DPC execution/publication. No ABI/T4
//! ingress, no visible presentation, no raster parity, no native GPU. This
//! backend's `process_task`/`present` are honest, named rejections -- this
//! slice proves the raw-DPC production seam, not general gfx-task execution.

use fn64_render::{
    BackendPreparedRawDpc, BoundSubmittedRawDpc, CommittedRawDpcOutcome, ExactRawDpcPlanVisitor,
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcCoordinator, RawDpcExecutionView,
    RawDpcIrCapability, RawDpcPlanRequest, RawDpcSemanticCommandRef, RdpStateCommand,
    RdpTriangleCommand, ReadyRawDpcCommitCapsule, RenderBackend, RenderConfig, RenderError,
    TmemLoadSemantics, TmemLoadShape,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CapturedGuestRead, CompletedWrite,
    DecodedTicket, ResourceAccess, ResourceJournal, ResourceJournalLimits, SubmittedTicket,
    TicketAuthoritySet, ValidationError, WorkloadAdmission, WorkloadPacket,
};

use crate::raw_dpc::push_decoded_raw_dpc;
use crate::{
    CombineParams, HeadlessBackend, MissingTriangleDrawState, OtherMode, PhysicalTmemError,
    PhysicalTmemPacketTransaction, PhysicalTmemState, RawDpcDecodeError, RdpState,
    RetrievedTriangleDraw, TmemLoadSourceIdentity, TmemTransferWord, TriangleDrawOutput,
    TrianglePipelineDeviceOutcome, TrianglePipelineError, TrianglePipelineRenderer,
    TriangleRasterParams, TriangleTargetExtent, UninitializedTrianglePipeline,
};

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
        let (session, authority) =
            fn64_render::new_raw_dpc_roles().map_err(WgpuBackendConstructionError::RawDpcRoles)?;
        let initial = PhysicalTmemState::try_new()
            .map_err(WgpuBackendConstructionError::PhysicalTmemState)?;
        Ok((
            Self {
                coordinator: authority.into_coordinator(initial),
                rdp_state: RdpState::default(),
                triangle_pipeline: None,
                triangle_target_extent: None,
                triangle_draw_output: None,
            },
            session,
        ))
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
    pub(crate) fn create_inner(&mut self, cfg: &RenderConfig) -> Result<(), WgpuCreateError> {
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

    /// Draws every collected triangle, in stream order, through
    /// `TrianglePipelineRenderer::submit_admitted_triangle`, using the
    /// identity `TriangleRasterParams` derived once from the stored
    /// `triangle_target_extent` (never recomputed per triangle, never
    /// defaulted). `last_triangle_draw()` updates only after every
    /// triangle in this call draws successfully -- a failure partway
    /// through leaves the prior successful value in place, unchanged,
    /// never cleared: an old-but-real result outlives a failed attempt to
    /// replace it, matching this file's own "never a silent partial
    /// state" convention elsewhere.
    fn draw_admitted_triangles(
        &mut self,
        triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let extent = self
            .triangle_target_extent
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let raster_params = TriangleRasterParams {
            resolution: [extent.width as f32, extent.height as f32],
            screen_scale: [1.0, 1.0],
            screen_offset: [0.0, 0.0],
        };

        let mut last_output = None;
        for draw in triangles {
            let draw = draw.map_err(WgpuRawDpcExecutionError::MissingTriangleDrawState)?;
            let in_flight = pipeline
                .submit_admitted_triangle(
                    draw.vertices,
                    draw.other_mode,
                    draw.combine_params,
                    raster_params,
                    extent,
                )
                .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
            let output = in_flight
                .complete()
                .map_err(WgpuRawDpcExecutionError::TriangleDraw)?;
            last_output = Some(output);
        }

        if let Some(output) = last_output {
            self.triangle_draw_output = Some(output);
        }
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
/// exact per-command logic (walk-local `current_other_mode`/
/// `current_combine`, snapshotted onto each triangle at its own stream
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
    /// `OtherMode`/`CombineParams` current at the walk's current stream
    /// position -- seeded from `WgpuBackend.rdp_state`'s durable value at
    /// construction time (`Self::seeded`), then updated on every
    /// `SetOtherMode`/`SetCombine` command in plan order.
    current_other_mode: Option<OtherMode>,
    current_combine: Option<CombineParams>,
    /// One entry per admitted `Triangle` command, in plan order. `Err`
    /// names exactly which state (`OtherMode` or `CombineParams`) was
    /// still unset at that triangle's own stream position -- never a
    /// silent default, matching `TriangleDrawStateCollector`'s own
    /// documented absence handling.
    triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
}

impl PlanCollector {
    /// Seeds `current_other_mode`/`current_combine` from `WgpuBackend`'s
    /// own durable `rdp_state` instead of `None` -- a real constructor
    /// parameter, never a synthetic plan-stream entry. This is the
    /// draw-state-*retrieval* half of durable cross-submission carry-in;
    /// the admission-time half (`push_decoded_raw_dpc`'s own
    /// `TriangleBeforeAnyOtherMode` gate) already seeds identically from
    /// the same `rdp_state`, via `decode_raw_dpc`'s existing
    /// `durable_state` parameter -- see this card's own design notes for
    /// why neither half needed a signature change to close this gap.
    fn seeded(other_mode: Option<OtherMode>, combine: Option<CombineParams>) -> Self {
        Self {
            loads: Vec::new(),
            accesses: Vec::new(),
            next_command_index: 0,
            current_other_mode: other_mode,
            current_combine: combine,
            triangles: Vec::new(),
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
            RawDpcSemanticCommandRef::State(state) => match state {
                RdpStateCommand::SetOtherMode { other_mode, .. } => {
                    self.current_other_mode =
                        Some(OtherMode::from_wire(other_mode.high, other_mode.low));
                }
                RdpStateCommand::SetCombine { combine, .. } => {
                    self.current_combine =
                        Some(CombineParams::from_wire(combine.low, combine.high));
                }
                _ => {}
            },
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand { vertices, .. }) => {
                let triangle_index = self.triangles.len();
                let snapshot = (|| {
                    let other_mode = self
                        .current_other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .current_combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        other_mode,
                        combine_params,
                    })
                })();
                self.triangles.push(snapshot);
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
struct ExecutionCollector<'coord> {
    physical: &'coord PhysicalTmemState,
    queue: fn64_render_ir::QueueIdentity,
    ordinal: u64,
    submission: fn64_render_ir::SubmissionIdentity,
    plan: PlanCollector,
    reads: Vec<(u32, Vec<u8>)>,
    outcome: Option<Result<StagedOutcome, WgpuRawDpcExecutionError>>,
}

/// What `stage_and_report` found for one sealed plan, structurally
/// distinguishing "this plan has TMEM loads to stage" from "this plan has
/// no TMEM loads, only triangles" (`WgpuBackend` production triangle-draw
/// integration card §1c) -- the caller (`execute_raw_dpc_inner`) uses this
/// to choose which `RawDpcCoordinator` completion method is even
/// reachable, never by re-deriving "is this plan write-bearing" itself.
/// Mechanical, not a judgment call: a plan reaches `TriangleOnly` only
/// when `collector.plan.loads` is empty and `collector.plan.triangles` is
/// not (checked once, here, at the one place both facts are already
/// gathered) -- never inferred from the *presence* of triangles alone,
/// since a plan could in principle carry both (mixed plans stay on the
/// `TmemLoads` path unconditionally, per the `is_empty()` check).
enum StagedOutcome {
    TmemLoads(BackendEffectReport, PhysicalTmemState),
    TriangleOnly,
}

impl RawDpcExecutionView<PlanCollector> for ExecutionCollector<'_> {
    fn plan_visited(&mut self, plan_visitor: &mut PlanCollector) {
        // `PlanCollector` has no `Default` (its real construction always
        // takes an explicit durable-state seed, `Self::seeded`) -- the
        // placeholder left behind here is discarded immediately by the
        // caller (`execute_raw_dpc_inner`'s `let _ = plan_visitor;`), so
        // its exact seed values are irrelevant.
        self.plan = core::mem::replace(plan_visitor, PlanCollector::seeded(None, None));
    }

    fn captured_reads(&mut self, reads: &[CapturedGuestRead]) {
        self.reads = reads
            .iter()
            .map(|captured| (captured.read().access_index(), captured.bytes().to_vec()))
            .collect();
    }

    fn submitted_packet(&mut self, packet: &WorkloadPacket) {
        self.outcome = Some(stage_and_report(self, packet));
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
    /// A triangle-bearing plan reached execution with no successful prior
    /// `RenderBackend::create` call -- `triangle_pipeline`/
    /// `triangle_target_extent` are always `Some` together (§1a/§1e) or
    /// both `None`; this is the caller-contract violation of the latter.
    TriangleDrawBeforeCreate,
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
            Self::TriangleDrawBeforeCreate => formatter.write_str(
                "a triangle-bearing plan reached execution with no successful prior \
                 RenderBackend::create call",
            ),
        }
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

/// The exact captured source bytes for one load, bound at its declared
/// `source_access_index` -- mirrors
/// `crate::tmem::execute::load_block::ExactLoadBlockGuestReads::bytes_for_word`'s
/// binding rule (match on `CapturedGuestRead::read().access_index()`), but
/// against [`ExecutionCollector`]'s owned `(access_index, bytes)` pairs
/// (extracted from `execution_view`'s finalized `&[CapturedGuestRead]` in
/// `captured_reads`, since neither the slice nor its elements outlive that
/// call).
fn load_source_bytes<'a>(
    reads: &'a [(u32, Vec<u8>)],
    load: &TmemLoadSemantics,
) -> Option<&'a [u8]> {
    reads
        .iter()
        .find(|(access_index, _)| *access_index == load.source_access_index())
        .map(|(_, bytes)| bytes.as_slice())
}

/// One transfer word's exact captured source-byte slice, bound by
/// `word.source_access_byte_offset()`/`defined_source_byte_mask()` into the
/// load's whole captured source range -- mirrors `load_block.rs`'s
/// `bytes_for_word` exactly (defined byte count, offset, bounds-checked
/// slice).
fn word_source_bytes(source_bytes: &[u8], word: TmemTransferWord) -> Option<&[u8]> {
    let defined = word.defined_source_byte_mask().count_ones() as usize;
    let start = word.source_access_byte_offset() as usize;
    let end = start.checked_add(defined)?;
    source_bytes.get(start..end)
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

    fn process_task(
        &mut self,
        _rdram: &mut [u8],
        _rsp_memory: &mut fn64_runtime::RspMemory,
        _task: &fn64_render::OsTask,
        _output_addr: u32,
    ) -> Result<fn64_render::FrameStatus, RenderError> {
        Err(RenderError::Backend {
            backend: "render-wgpu",
            reason: "WgpuBackend implements only the T3 production raw-DPC seam; general gfx \
                      task execution is out of scope"
                .to_string(),
        })
    }

    fn present(&mut self, _request: fn64_render::PresentRequest<'_>) -> Result<(), RenderError> {
        Err(RenderError::Backend {
            backend: "render-wgpu",
            reason: "WgpuBackend implements only the T3 production raw-DPC seam; presentation \
                      is out of scope"
                .to_string(),
        })
    }

    fn resize(&mut self, _w: u32, _h: u32) {}

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }

    fn raw_dpc_ir_capability(&self) -> RawDpcIrCapability {
        RawDpcIrCapability::TransactionalTmemNoFullSync
    }

    fn plan_raw_dpc(
        &mut self,
        request: RawDpcPlanRequest,
    ) -> Result<PlannedRawDpcSubmission, RenderError> {
        let (planned, delta) = plan_raw_dpc_inner(&self.coordinator, &self.rdp_state, request)
            .map_err(|reason| RenderError::Backend {
                backend: "render-wgpu/raw-dpc-plan",
                reason,
            })?;
        self.rdp_state.apply(&delta);
        Ok(planned)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, RenderError> {
        let (prepared, triangles) = execute_raw_dpc_inner(
            &mut self.coordinator,
            bound,
            self.rdp_state.other_mode(),
            self.rdp_state.combine(),
        )
        .map_err(RenderError::from)?;

        if !triangles.is_empty() {
            self.draw_admitted_triangles(triangles)
                .map_err(RenderError::from)?;
        }

        Ok(prepared)
    }

    fn publish_raw_dpc(
        &mut self,
        publication: ReadyRawDpcCommitCapsule<'_>,
    ) -> CommittedRawDpcOutcome {
        self.coordinator.prepare_publication(publication).commit()
    }
}

/// `plan_raw_dpc`'s body: decode `request`'s capture through T1's real
/// decoder, push every admitted command through T0's sealed writer, and
/// seal the result. `fn64_render::ExactRawDpcPlanWriter::finish` requires
/// the exact journal T1's decode used; that journal is not knowable ahead
/// of decoding (it depends on the capture's own admitted TMEM sources), so
/// this mirrors T1's own test harness's two-pass probe: decode once against
/// a throwaway single-source journal, read the real access list back off
/// `RawDpcDecodeError::JournalMismatch::expected` when the probe
/// (correctly) disagrees, then decode again for real. Every `SubmittedTicket`
/// minted here is through a throwaway, locally owned `TicketAuthoritySet` --
/// `crate::decode_raw_dpc` only needs one that is internally consistent
/// with the capture it decodes, never the "real" production queue (that
/// queue identity is proven separately, by `RawDpcBackendAuthority::
/// begin_plan`'s own paired-queue assertion against `request`).
fn plan_raw_dpc_inner(
    coordinator: &RawDpcCoordinator<PhysicalTmemState>,
    durable_state: &RdpState,
    request: RawDpcPlanRequest,
) -> Result<(PlannedRawDpcSubmission, crate::RdpStateDelta), String> {
    let capture = request.capture();
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let submission_start = submission.start();
    let capture_words = submission.command_words();

    let probe_journal = single_source_probe_journal(&submission, layout)
        .map_err(|error| format!("raw-DPC plan probe journal failed: {error}"))?;
    let probe_decoded = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission.clone(),
        capture.cmd_end(),
        probe_journal,
    )
    .map_err(|error| format!("raw-DPC plan probe preflight failed: {error}"))?;
    let probe_ticket = submit_locally(probe_decoded)
        .map_err(|error| format!("raw-DPC plan probe submission failed: {error}"))?;

    let journal = match crate::decode_raw_dpc(probe_ticket, &RdpState::default()) {
        Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => {
            let accesses = expected.into_vec();
            let declared = accesses
                .iter()
                .map(|access| access.region().declared_bytes())
                .sum::<u32>();
            ResourceJournal::try_new(
                ResourceJournalLimits::try_new(
                    fn64_render_ir::MAX_RESOURCE_ACCESSES,
                    declared.max(1),
                )
                .map_err(|error| format!("raw-DPC plan journal limits failed: {error}"))?,
                accesses,
            )
            .map_err(|error| format!("raw-DPC plan journal failed: {error}"))?
        }
        Ok(_) => {
            return Err(
                "raw-DPC plan probe unexpectedly succeeded against a single-source journal"
                    .to_string(),
            )
        }
        Err(error) => return Err(format!("raw-DPC plan probe decode failed: {error}")),
    };

    let decoded_ticket = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission,
        capture.cmd_end(),
        journal.clone(),
    )
    .map_err(|error| format!("raw-DPC plan preflight failed: {error}"))?;
    let ticket = submit_locally(decoded_ticket)
        .map_err(|error| format!("raw-DPC plan submission failed: {error}"))?;

    let decoded = crate::decode_raw_dpc(ticket, durable_state)
        .map_err(|error| format!("raw-DPC plan decode failed: {error}"))?;
    let delta = decoded.state_delta().clone();

    let mut writer = coordinator.begin_plan(request);
    push_decoded_raw_dpc(
        &mut writer,
        &decoded,
        &capture_words,
        layout,
        submission_start,
    )
    .map_err(|error| format!("raw-DPC plan admission failed: {error}"))?;
    let planned = writer
        .finish(journal)
        .map_err(|error| format!("raw-DPC plan seal failed: {error}"))?;
    Ok((planned, delta))
}

fn submit_locally(decoded: DecodedTicket) -> Result<SubmittedTicket, ValidationError> {
    let (mut queue, _, _) = TicketAuthoritySet::try_new()?.into_roles();
    queue.submit(decoded)
}

/// `fn64_render::decode_raw_dpc_capture` hard-codes
/// `DeferredGuestReadCapture::empty()`, which only satisfies a plan whose
/// guest-read plan is itself empty -- never true here, since every admitted
/// TMEM load declares at least one `TmemLoadSource` read. `plan_raw_dpc`'s
/// two internal decode passes (the single-source probe, and the real
/// journal-backed decode) both exist purely to learn the command
/// structure/journal shape and drive T1's push loop -- neither one is the
/// production submission the ABI session's `finalize_and_submit` performs
/// later with the real captured bytes -- so a correctly *sized*, zero-filled
/// capture is exactly as valid here as any other byte content: `finish`'s
/// own access-count/order check (and, for the probe, the deliberate
/// `JournalMismatch` this function is built to catch) never inspects read
/// content, only shape.
fn finalize_with_zero_reads(
    layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    submission: fn64_render::OwnedRawDpcSubmission,
    cmd_end: fn64_render_ir::TemporalBoundary,
    journal: ResourceJournal,
) -> Result<DecodedTicket, ValidationError> {
    let preflight = fn64_render::preflight_raw_dpc_capture(
        layout,
        transaction_sequence,
        submission,
        cmd_end,
        Vec::new(),
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

/// A minimal, self-consistent probe journal (command-decode access plus one
/// whole-capture TMEM-source access) sufficient only to drive one decode
/// attempt whose sole purpose is reading back the real access list via
/// `JournalMismatch::expected`. Mirrors
/// `crate::raw_dpc::production_adapter::tests::journal_for`.
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
/// The TMEM-source access stays `ResourceRegion::Rdram { resource:
/// RdramResource::Buffer, .. }` for both sources: every admitted TMEM
/// load's source bytes are RDP-physical RDRAM addresses regardless of which
/// bus carried the command stream (`crate::raw_dpc::production_adapter`'s
/// push loop; XBUS changes only where the *command words* come from). For
/// an XBUS submission this probe access intentionally does NOT reuse
/// `submission.start()`/`end()` (DMEM-relative, wrong address space for an
/// RDRAM buffer read) -- it covers the same-sized span at RDRAM offset 0
/// instead. This is only a self-consistent probe (its own doc comment: read
/// back the real access list via the deliberate `JournalMismatch` it
/// causes), never the real journal `plan_raw_dpc_inner` submits for
/// execution, so its exact placement is arbitrary as long as it lies in
/// bounds and is internally consistent.
fn single_source_probe_journal(
    submission: &fn64_render::OwnedRawDpcSubmission,
    layout: fn64_render_ir::PhysicalMemoryLayout,
) -> Result<ResourceJournal, ValidationError> {
    use fn64_render_ir::{DmemRange, OperationId, RdramResource, ResourceRegion};
    let start = submission.start();
    let end = submission.end();
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
    let source_bytes = end.saturating_sub(start).max(1);
    let source_access = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Read,
        AccessPurpose::TmemLoadSource,
        ResourceRegion::Rdram {
            resource: RdramResource::Buffer,
            range: match submission.source() {
                fn64_render::RawDpcSource::Rdram => layout.range(start, end)?,
                fn64_render::RawDpcSource::XbusDmem => layout.range(0, source_bytes)?,
            },
        },
    )?;
    let accesses = vec![command_access, source_access];
    let declared = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum::<u32>();
    ResourceJournal::try_new(
        ResourceJournalLimits::try_new(64, declared.max(1))?,
        accesses,
    )
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

/// `execute_raw_dpc`'s body: lend the sealed plan through `execution_view`
/// (which drives the whole stage/finish/effect-report pipeline inside its
/// own `submitted_packet` callback -- see [`ExecutionCollector`]'s doc
/// comment for why), then hand the resulting `BackendEffectReport` and
/// `into_physical_successor` (T3 Phase A) result to
/// `RawDpcCoordinator::complete_execution`.
///
/// `durable_other_mode`/`durable_combine` are `WgpuBackend.rdp_state`'s
/// own current values, passed in by the trait method (which has `self`)
/// since this is a free function taking only `coordinator` -- they seed
/// `PlanCollector`'s walk (`PlanCollector::seeded`) so a triangle in this
/// submission with no `SetOtherMode`/`SetCombine` of its own still
/// resolves its draw state from durable cross-submission carry-in, not
/// `None`.
fn execute_raw_dpc_inner(
    coordinator: &mut RawDpcCoordinator<PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
    durable_other_mode: Option<OtherMode>,
    durable_combine: Option<CombineParams>,
) -> Result<
    (
        BackendPreparedRawDpc,
        Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    ),
    WgpuRawDpcExecutionError,
> {
    let mut plan_visitor = PlanCollector::seeded(durable_other_mode, durable_combine);
    let mut view = ExecutionCollector {
        physical: coordinator.physical(),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::seeded(durable_other_mode, durable_combine),
        reads: Vec::new(),
        outcome: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    let _ = plan_visitor; // plan contents were moved into `view.plan` by `plan_visited`

    let outcome = view
        .outcome
        .expect("execution_view always calls submitted_packet exactly once")?;
    let triangles = view.plan.triangles;

    let prepared = match outcome {
        StagedOutcome::TmemLoads(effects, next_physical) => coordinator
            .complete_execution(bound, effects, next_physical)
            .map_err(WgpuRawDpcExecutionError::Coordinator)?,
        // Mechanical, not inferred: `StagedOutcome::TriangleOnly` is only
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
        StagedOutcome::TriangleOnly => coordinator
            .complete_execution_preserving_physical(bound)
            .map_err(WgpuRawDpcExecutionError::Coordinator)?,
    };

    Ok((prepared, triangles))
}

/// The pipeline `submitted_packet` runs once `&WorkloadPacket` is in scope:
/// stage every ordered TMEM load into one packet-local transaction via
/// `PhysicalTmemState::stage_neutral_transfer` (T3 Phase B's own neutral
/// counterpart to the decoder-typed `stage_transfer`), seal it into a
/// `PendingTmemTransaction`, compute the exact `BackendEffectReport` from
/// its own proposed effects, and derive this transaction's
/// `into_physical_successor` (T3 Phase A) candidate. Returns
/// `StagedOutcome::TriangleOnly` instead of staging anything when the
/// plan has zero TMEM loads but at least one admitted triangle (§1c) --
/// still `NoCompletedLoads` when the plan has neither.
fn stage_and_report(
    collector: &ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<StagedOutcome, WgpuRawDpcExecutionError> {
    let source = TmemLoadSourceIdentity::new(
        packet.identity(),
        packet.journal().identity(),
        collector.submission,
        packet.memory_layout(),
    );
    let sequence = transaction_sequence(packet);

    let mut packet_transaction: Option<PhysicalTmemPacketTransaction> = None;

    for (command_index, load) in collector.plan.loads.iter() {
        let command_index = *command_index;

        let destination_accesses = destination_access_run(
            &collector.plan.accesses,
            load.destination_access_index() as usize,
        );
        if destination_accesses.is_empty() {
            return Err(WgpuRawDpcExecutionError::MalformedDestinationAccessRun { command_index });
        }
        let source_bytes = load_source_bytes(&collector.reads, load)
            .ok_or(WgpuRawDpcExecutionError::MissingCapturedSource { command_index })?;

        let mut staged = match packet_transaction.take() {
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
                .map_err(WgpuRawDpcExecutionError::Physical)?,
            Some(packet) => packet
                .stage_neutral_transfer_next(source, load, destination_accesses)
                .map_err(WgpuRawDpcExecutionError::Physical)?,
        };

        for word in staged.expected_words().to_vec() {
            let bytes = word_source_bytes(source_bytes, word)
                .ok_or(WgpuRawDpcExecutionError::MissingCapturedSource { command_index })?;
            let physical_lanes = map_physical_lanes(load, word, bytes)
                .map_err(WgpuRawDpcExecutionError::Physical)?;
            let payload = staged
                .physical_word_payload(word, physical_lanes)
                .map_err(WgpuRawDpcExecutionError::Physical)?;
            staged
                .stage_word(payload)
                .map_err(WgpuRawDpcExecutionError::Physical)?;
        }

        packet_transaction = Some(
            staged
                .finish_load()
                .map_err(WgpuRawDpcExecutionError::Physical)?,
        );
    }

    let packet_transaction = match packet_transaction {
        Some(packet_transaction) => packet_transaction,
        None => {
            // No TMEM load completed a transaction -- mechanically
            // distinguish "this plan has triangles instead" (§1c: route
            // to the coordinator's preserving-physical completion, never
            // to `complete_execution`, which has no successor to offer
            // for an empty transaction) from "this plan has nothing at
            // all" (still `NoCompletedLoads`, unchanged).
            return if collector.plan.triangles.is_empty() {
                Err(WgpuRawDpcExecutionError::NoCompletedLoads)
            } else {
                Ok(StagedOutcome::TriangleOnly)
            };
        }
    };
    let pending = packet_transaction
        .into_pending()
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    let writes: Vec<CompletedWrite> = pending.proposed_effects().to_vec();
    let effects =
        BackendEffectReport::try_new(packet, writes).map_err(WgpuRawDpcExecutionError::Effect)?;

    let next_physical = pending
        .into_physical_successor(collector.physical, &effects)
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    Ok(StagedOutcome::TmemLoads(effects, next_physical))
}

#[cfg(test)]
mod tests {
    use fn64_render::OwnedRawDpcSubmission;
    use fn64_render_ir::{
        CapturedGuestRead, DeferredGuestReadCapture, DpInterruptState, TemporalBoundary,
    };

    use super::*;

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    const SET_TEXTURE_IMAGE: u8 = 0x3d;
    const SET_TILE: u8 = 0x35;
    const LOAD_SYNC: u8 = 0x26;
    const LOAD_BLOCK: u8 = 0x33;
    const FULL_SYNC: u8 = 0x29;
    const SET_OTHER_MODE: u8 = 0x2f;
    const SET_COMBINE: u8 = 0x3c;
    const RAW_TRIANGLE_BASE_EDGE: u8 = 0x08;

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    fn set_other_mode(cycle_type: u32, low: u32) -> [u32; 2] {
        [word(SET_OTHER_MODE, cycle_type << 20), low]
    }

    fn set_combine(payload: u32, high: u32) -> [u32; 2] {
        [word(SET_COMBINE, payload & 0x00ff_ffff), high]
    }

    /// One base-edge (non-shaded, non-textured, non-Z) triangle command's
    /// eight raw wire words -- mirrors
    /// `raw_dpc::production_adapter::tests::triangle_base_edge_words`
    /// exactly (that helper is private to its own module's tests, so this
    /// is a local, identical copy, not a shared import).
    fn triangle_base_edge_words(tile: u32, level: u32, yl: u16) -> [u32; 8] {
        let w0 = word(
            RAW_TRIANGLE_BASE_EDGE,
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(yl),
        );
        [
            w0,
            0,
            0x0010_0000,
            0,
            0x0020_0000,
            0x0000_8000,
            0x0005_0000,
            0,
        ]
    }

    fn set_texture_image(format: u32, size: u32, width: u32, address: u32) -> [u32; 2] {
        [
            word(SET_TEXTURE_IMAGE, format << 21 | size << 19 | (width - 1)),
            address,
        ]
    }

    fn set_tile(tile: u32, line: u32, tmem: u32) -> [u32; 2] {
        [word(SET_TILE, 2 << 19 | line << 9 | tmem), tile << 24]
    }

    fn load_sync() -> [u32; 2] {
        [word(LOAD_SYNC, 0), 0]
    }

    /// One admitted, TMEM-only raw-DPC command stream: SetTextureImage,
    /// SetTile, LoadSync, LoadBlock -- the same admitted TMEM/state subset
    /// v11 freezes, and the same fixture shape T1's own
    /// `production_adapter::tests` module uses.
    fn one_load_block_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words
    }

    /// A mixed plan: one admitted `TmemLoad` (identical shape to
    /// `one_load_block_words`) PLUS `SetOtherMode`/`SetCombine`/one
    /// admitted `RawTriangle` -- proves the loads+triangle branch-selection
    /// rule (§1c): a plan with at least one TMEM load must always take the
    /// real successor route (`complete_execution`), never the
    /// preserving-physical route, regardless of the triangle's presence.
    fn mixed_load_and_triangle_words() -> Vec<u32> {
        let mut words = one_load_block_words();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(triangle_base_edge_words(7, 2, 0));
        words
    }

    /// A triangle-only plan: `SetOtherMode`/`SetCombine`/one admitted
    /// `RawTriangle`, zero TMEM loads -- exercises `stage_and_report`'s
    /// `StagedOutcome::TriangleOnly` arm and
    /// `RawDpcCoordinator::complete_execution_preserving_physical`.
    fn triangle_only_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(triangle_base_edge_words(7, 2, 0));
        words
    }

    const RAW_TRIANGLE_SHADED: u8 = 0x0c;

    /// A shaded (0x0c), non-textured, non-Z triangle covering the whole
    /// 8x8 target with a FLAT uniform shade color -- mirrors
    /// `targets::triangle_pipeline::tests::host_gpu_tests::
    /// shaded_covering_triangle_words` exactly (see that function's own
    /// doc for the full field-by-field derivation); duplicated here, not
    /// imported, since that helper is private to its own module's tests.
    fn shaded_covering_triangle_words(color_255: [u32; 4]) -> Vec<u32> {
        let mut words = vec![
            word(RAW_TRIANGLE_SHADED, 32u32),
            0,
            (8i32 << 16) as u32,
            0,
            0,
            0,
            0,
            0,
        ];
        let base_w0 = (color_255[0] << 16) | (color_255[1] & 0xffff);
        let base_w1 = (color_255[2] << 16) | (color_255[3] & 0xffff);
        words.extend([
            base_w0, base_w1, // shade[0]
            0, 0, // shade[1] (dx)
            0, 0, // shade[2] (base low half, zero)
            0, 0, // shade[3] (dx low half)
            0, 0, // shade[4] (de)
            0, 0, // shade[5] (unused by decode_shade)
            0, 0, // shade[6] (de low half)
            0, 0, // shade[7] (unused)
        ]);
        words
    }

    /// Two independent `LoadBlock`s in one submission, each preceded by its
    /// own `SetTile`/`LoadSync` (a fresh `LoadSync` mints a strictly newer
    /// `TmemLoadEpoch`, satisfying `neutral_validate_transfer`'s
    /// `EpochNotNewer` ordering check) and targeting disjoint TMEM word
    /// offsets (`tmem=0` vs `tmem=0x100`) so their destination ranges never
    /// overlap. This is the only fixture in this module that exercises
    /// `PhysicalTmemPacketTransaction::stage_neutral_transfer_next` -- every
    /// other fixture here has exactly one load, which never chains past
    /// `PhysicalTmemState::stage_neutral_transfer`'s first-load path.
    fn two_load_block_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words.extend(set_tile(6, 2, 0x100));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 6 << 24 | 9 << 12 | 0x0800]);
        words
    }

    fn capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
        fn64_render::OwnedRawDpcCapture::new(
            submission,
            layout,
            7,
            TemporalBoundary::new(1, DpInterruptState::Clear),
        )
    }

    /// Same fixture shape as `capture`, but sourced from XBUS/DMEM instead
    /// of RDRAM -- T4's second ABI producer shape (MMIO XBUS, RSP XBUS).
    /// `RawDpcSource::XbusDmem` bounds ranges to the 4 KiB DMEM bank
    /// (`OwnedRawDpcSubmission::validate_range`), unlike the RDRAM-bounded
    /// `capture` helper above, so this starts at DMEM offset 0.
    fn xbus_capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let start = 0u32;
        let end = u32::try_from(words.len() * 4).unwrap();
        let payload: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let submission = OwnedRawDpcSubmission::from_xbus_payload(start, end, payload).unwrap();
        fn64_render::OwnedRawDpcCapture::new(
            submission,
            layout,
            7,
            TemporalBoundary::new(1, DpInterruptState::Clear),
        )
    }

    /// Drives `backend.plan_raw_dpc` for real (through the two-pass probe
    /// internal to `plan_raw_dpc_inner`), fills the plan's own deferred
    /// guest-read plan with deterministic bytes, and returns the sealed
    /// `PlannedRawDpcSubmission` plus the bytes used (so a hostile test can
    /// assert on the physical postimage those bytes should produce).
    fn plan_with_deterministic_reads(
        backend: &mut WgpuBackend,
        session: &RawDpcAbiSession,
        words: Vec<u32>,
    ) -> (PlannedRawDpcSubmission, Vec<u8>) {
        let request = session.plan_request(capture(words));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("fixture plans cleanly");
        let source_bytes: Vec<u8> = (0..planned.guest_read_plan().reads()[0].range().len())
            .map(|index| index as u8)
            .collect();
        (planned, source_bytes)
    }

    /// Same as `plan_with_deterministic_reads`, but for a fixture that
    /// declares zero `TmemLoadSource` reads (a triangle-only plan) --
    /// `plan_with_deterministic_reads`'s own `reads()[0]` indexing would
    /// panic on an empty guest-read plan, so this asserts the expectation
    /// explicitly instead of assuming it.
    fn plan_with_no_reads(
        backend: &mut WgpuBackend,
        session: &RawDpcAbiSession,
        words: Vec<u32>,
    ) -> PlannedRawDpcSubmission {
        let request = session.plan_request(capture(words));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("fixture plans cleanly");
        assert!(
            planned.guest_read_plan().reads().is_empty(),
            "a triangle-only plan must declare zero TmemLoadSource reads"
        );
        planned
    }

    /// Plans a multi-load fixture and fills *every* read the resulting
    /// `guest_read_plan` declares (one `TmemLoadSource` per load) with its
    /// own deterministic byte pattern, keyed by read index so two reads of
    /// equal length still get distinguishable content -- unlike
    /// `plan_with_deterministic_reads`/`guest_read_capture` above, which
    /// only ever fill (and only ever need to fill) a single load's one read.
    fn plan_with_deterministic_reads_for_every_load(
        backend: &mut WgpuBackend,
        session: &RawDpcAbiSession,
        words: Vec<u32>,
    ) -> (PlannedRawDpcSubmission, Vec<Vec<u8>>) {
        let request = session.plan_request(capture(words));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("fixture plans cleanly");
        let per_read_bytes: Vec<Vec<u8>> = planned
            .guest_read_plan()
            .reads()
            .iter()
            .enumerate()
            .map(|(read_index, read)| {
                (0..read.range().len())
                    .map(|byte_index| (read_index as u8).wrapping_add(byte_index as u8))
                    .collect()
            })
            .collect();
        (planned, per_read_bytes)
    }

    fn guest_read_capture_per_read(
        planned: &PlannedRawDpcSubmission,
        per_read_bytes: &[Vec<u8>],
    ) -> DeferredGuestReadCapture {
        DeferredGuestReadCapture::new(
            planned
                .guest_read_plan()
                .reads()
                .iter()
                .zip(per_read_bytes)
                .map(|(read, bytes)| CapturedGuestRead::try_new(*read, bytes.clone()).unwrap())
                .collect(),
        )
    }

    fn guest_read_capture(
        planned: &PlannedRawDpcSubmission,
        source_bytes: &[u8],
    ) -> DeferredGuestReadCapture {
        DeferredGuestReadCapture::new(
            planned
                .guest_read_plan()
                .reads()
                .iter()
                .map(|read| CapturedGuestRead::try_new(*read, source_bytes.to_vec()).unwrap())
                .collect(),
        )
    }

    fn admitted_fabric(
    ) -> fn64_runtime::DeviceFabric<fn64_runtime::rom::InMemoryRom, fn64_runtime::FixedPiTiming>
    {
        let mut fabric = fn64_runtime::DeviceFabric::new(
            fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
            fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
        );
        fabric
            .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
            .unwrap()
            .expect("fresh fabric is never frozen");
        fabric
    }

    /// End-to-end: plan -> execute -> commit zero guest writes -> seal ->
    /// publish, driven entirely through `WgpuBackend`'s `RenderBackend`
    /// methods (`plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc`) plus the
    /// ABI-owned `RawDpcAbiSession` calls a real caller would make around
    /// them. Proves the whole production seam actually completes and flips
    /// the coordinator's active physical slot.
    #[test]
    fn plan_execute_publish_completes_and_flips_active_physical_slot() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();

        let prepared = backend.execute_raw_dpc(bound).unwrap();
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);

        assert_ne!(
            backend.physical_tmem().identity(),
            initial_identity,
            "publish must flip the coordinator's active slot to the executed candidate"
        );
        assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
    }

    /// End-to-end, real Metal execution: a mixed plan (TMEM load +
    /// triangle) must plan/execute/publish and flip the coordinator's
    /// active physical slot exactly like a TMEM-only plan does (mirrors
    /// `plan_execute_publish_completes_and_flips_active_physical_slot`'s
    /// own full sequence) -- proving the real successor route
    /// (`complete_execution`), not the preserving-physical route, was
    /// actually used for a mixed plan. If the preserving-physical route
    /// had been used instead, `complete_execution_preserving_physical`'s
    /// own internal `BackendEffectReport::try_new(packet, Vec::new())`
    /// call would have failed outright (the load's own journal entry
    /// declares a real write access, `Vec::new()` declares zero, and
    /// `validate_effects` rejects any count mismatch) -- so this test's
    /// mere success, not just the slot flip, is itself evidence the
    /// correct route was taken.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn mixed_load_and_triangle_plan_uses_the_real_successor_route_not_preserving() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        match backend.create_inner(&test_render_config()) {
            Ok(()) => {}
            Err(WgpuCreateError::NoAdapter(no_adapter)) => {
                panic!(
                    "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
                );
            }
            Err(other) => panic!("create() failed for an unexpected reason: {other}"),
        }
        let initial_identity = backend.physical_tmem().identity();

        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, mixed_load_and_triangle_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();

        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("a mixed load+triangle plan must execute successfully");
        assert!(
            backend.last_triangle_draw().is_some(),
            "the mixed plan's triangle must still be drawn during execute_raw_dpc"
        );
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);
        assert_ne!(
            backend.physical_tmem().identity(),
            initial_identity,
            "a mixed plan's TMEM load must still flip the active physical slot on publish, via \
             the real complete_execution successor route"
        );
    }

    /// End-to-end, real Metal execution: a triangle-only plan (zero TMEM
    /// loads) completes via `complete_execution_preserving_physical` and
    /// its publish must leave the active physical slot's identity
    /// UNCHANGED (the opposite assertion direction from every TMEM-load
    /// test in this module) -- there is no successor state to flip to,
    /// by design (§1c).
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn triangle_only_plan_completes_via_preserving_physical_and_never_flips_the_slot() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        match backend.create_inner(&test_render_config()) {
            Ok(()) => {}
            Err(WgpuCreateError::NoAdapter(no_adapter)) => {
                panic!(
                    "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
                );
            }
            Err(other) => panic!("create() failed for an unexpected reason: {other}"),
        }
        let initial_identity = backend.physical_tmem().identity();

        let planned = plan_with_no_reads(&mut backend, &session, triangle_only_words());
        let guest_capture = guest_read_capture(&planned, &[]);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();

        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("a triangle-only plan must execute successfully via preserving_physical");
        assert!(
            backend.last_triangle_draw().is_some(),
            "the triangle-only plan's triangle must still be drawn"
        );
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);
        assert_eq!(
            backend.physical_tmem().identity(),
            initial_identity,
            "a triangle-only plan has no TMEM successor to flip to -- the active physical slot's \
             identity must remain exactly what it was before, proving complete_execution (the \
             route that WOULD flip it) was never used"
        );
    }

    /// The real end-to-end test (§2): a real decoded capture containing
    /// `SetOtherMode`/`SetCombine`/one `RawTriangle`, pushed through the
    /// actual production entry points
    /// (`WgpuBackend::create`/`plan_raw_dpc`/`execute_raw_dpc`), asserted
    /// against real GPU-observed pixel output -- matching the rigor
    /// `targets::triangle_pipeline::tests`'s own
    /// `required_host_draws_a_real_admitted_triangle_matching_the_combiner_oracle`
    /// already established for its own standalone (non-`WgpuBackend`)
    /// proof, but through the actual `RenderBackend` seam this card
    /// closes, not a bare coordinator.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn wgpu_backend_draws_a_real_admitted_triangle_matching_the_combiner_oracle() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        match backend.create_inner(&test_render_config()) {
            Ok(()) => {}
            Err(WgpuCreateError::NoAdapter(no_adapter)) => {
                panic!(
                    "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
                );
            }
            Err(other) => panic!("create() failed for an unexpected reason: {other}"),
        }

        // SHADE-passthrough SetCombine: (A-B)*C+D collapses to D=SHADE.
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 4;
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 4;
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;

        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(low, high));
        let triangle_color_255 = [64u32, 128, 192, 255];
        words.extend(shaded_covering_triangle_words(triangle_color_255));

        // `set_combine(payload, high)` masks `payload` to the low 24 bits
        // and bakes the `SET_COMBINE` opcode byte into the top 8 bits of
        // the wire word -- `CombineParams::from_wire(w0, w1)` stores `w0`
        // unmasked (`combiner.rs`'s own module doc), so the expected value
        // is derived from the exact same wire word this fixture pushed,
        // not read back from the sealed plan (which exposes no such
        // accessor -- this mirrors how the standalone parallel-lane test
        // cross-checks against its own raw decoded ticket, not the plan).
        let combine_params = CombineParams::from_wire(word(SET_COMBINE, low & 0x00ff_ffff), high);
        let planned = plan_with_no_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &[]);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();

        backend
            .execute_raw_dpc(bound)
            .expect("the fixture stays inside the admitted state+triangle subset");

        let output = backend.last_triangle_draw().expect(
            "a successful triangle-bearing execute_raw_dpc must populate last_triangle_draw",
        );

        // Known-covered pixel, flat shade -> every covered pixel has the
        // same combiner output, no barycentric interpolation needed.
        let shade_color = [
            triangle_color_255[0] as f32 / 255.0,
            triangle_color_255[1] as f32 / 255.0,
            triangle_color_255[2] as f32 / 255.0,
            triangle_color_255[3] as f32 / 255.0,
        ];
        let inputs = crate::combiner::CombinerInputs {
            tex_val0: [0.0; 4],
            tex_val1: [0.0; 4],
            prim_color: [0.0; 4],
            shade_color,
            env_color: [0.0; 4],
            key_center: [0.0; 3],
            key_scale: [0.0; 3],
            lod_fraction: 0.0,
            prim_lod_frac: 0.0,
            noise: 0.0,
            k4: 0.0,
            k5: 0.0,
        };
        let (expected_color, _alpha_compare) =
            crate::combiner::run_one_cycle(combine_params, inputs);
        let expected_u8 = expected_color.map(|component| (component * 255.0).round() as u8);

        let pixel_index = (output.extent.width + 1) as usize * 4;
        let observed = [
            output.color_rgba8[pixel_index],
            output.color_rgba8[pixel_index + 1],
            output.color_rgba8[pixel_index + 2],
            output.color_rgba8[pixel_index + 3],
        ];
        for channel in 0..4 {
            assert!(
                observed[channel].abs_diff(expected_u8[channel]) <= 2,
                "pixel (1,1) channel {channel}: observed {observed:?} vs expected {expected_u8:?} \
                 (real decoded CombineParams via the real WgpuBackend production seam)"
            );
        }
        assert_eq!(expected_u8, triangle_color_255.map(|c| c as u8));
    }

    /// `create()`'s success stores `triangle_pipeline`/`triangle_target_extent`
    /// atomically, together -- a repeated `create()` call with a changed
    /// `RenderConfig` extent updates both, never one without the other.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn repeated_create_with_a_changed_extent_updates_pipeline_and_extent_together() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("first create() must succeed on a real adapter");
        assert_eq!(
            backend.triangle_target_extent,
            Some(TriangleTargetExtent {
                width: 8,
                height: 8
            })
        );

        let changed_config = fn64_render::RenderConfig {
            width: 16,
            height: 16,
            tv_type: fn64_runtime::TvType::default(),
        };
        backend
            .create_inner(&changed_config)
            .expect("a second create() call with a different extent must also succeed");
        assert!(backend.triangle_pipeline.is_some());
        assert_eq!(
            backend.triangle_target_extent,
            Some(TriangleTargetExtent {
                width: 16,
                height: 16
            }),
            "a repeated create() call must update the stored extent to match its own \
             RenderConfig, not retain the first call's value"
        );
    }

    /// `last_triangle_draw()` update timing (§1e): a failed
    /// `draw_admitted_triangles` call leaves whatever prior successful
    /// output was already stored completely untouched -- never cleared,
    /// never partially overwritten. Calls `draw_admitted_triangles`
    /// directly with a deliberately failing second triangle so the
    /// failure is deterministic, rather than relying on a real pipeline
    /// error.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn a_failed_triangle_draw_leaves_the_prior_successful_output_untouched() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("create() must succeed on a real adapter");

        let good_triangle = RetrievedTriangleDraw {
            vertices: [
                fixture_vertex(0.0),
                fixture_vertex(1.0),
                fixture_vertex(2.0),
            ],
            other_mode: OtherMode::from_wire(0, 0),
            combine_params: CombineParams::from_wire(0, 0),
        };
        backend
            .draw_admitted_triangles(vec![Ok(good_triangle)])
            .expect("a single valid triangle must draw successfully");
        let first_output_extent = backend
            .last_triangle_draw()
            .expect("the first successful draw must populate last_triangle_draw")
            .extent;

        let failing_triangles = vec![
            Ok(good_triangle),
            Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 1 }),
        ];
        let result = backend.draw_admitted_triangles(failing_triangles);
        assert!(
            result.is_err(),
            "a batch containing a MissingTriangleDrawState entry must fail, not silently skip it"
        );

        let output_after_failure = backend
            .last_triangle_draw()
            .expect("the prior successful output must still be present after a later failure");
        assert_eq!(
            output_after_failure.extent, first_output_extent,
            "a failed draw_admitted_triangles call must leave the prior successful \
             last_triangle_draw() value completely untouched, never cleared"
        );
    }

    /// Positive: `raw_dpc_ir_capability` reports the real v11 TMEM-only
    /// capability, not the trait's `Unsupported` default -- a caller must
    /// be able to tell this backend apart from a non-raw-DPC-capable one
    /// without attempting a submission.
    #[test]
    fn raw_dpc_ir_capability_reports_transactional_tmem_no_full_sync() {
        let (backend, _session) = WgpuBackend::try_new().unwrap();
        assert_eq!(
            backend.raw_dpc_ir_capability(),
            RawDpcIrCapability::TransactionalTmemNoFullSync
        );
    }

    /// Hostile: T1's push loop rejects any command outside the admitted
    /// TMEM/state subset -- `plan_raw_dpc` must surface that as a loud
    /// `RenderError`, never a silently truncated plan.
    #[test]
    fn plan_raw_dpc_rejects_a_full_sync_command_loudly() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = one_load_block_words();
        words.extend([word(FULL_SYNC, 0), 0]);

        let request = session.plan_request(capture(words));
        let result = backend.plan_raw_dpc(request);
        assert!(
            result.is_err(),
            "FullSync must be rejected, not silently admitted into the plan"
        );
    }

    /// T4 characterization: `plan_raw_dpc` must accept a genuinely
    /// XBUS-sourced capture (`RawDpcSource::XbusDmem`), not only the
    /// RDRAM-sourced captures every other fixture in this module exercises.
    /// Regression coverage for the bug this task found and fixed:
    /// `single_source_probe_journal`'s command-decode access previously
    /// always declared an RDRAM `RawCommands` region, which
    /// `validate_one_to_one_command_reads` (fn64-render-ir) rejects for an
    /// XBUS-sourced stream with `MissingCommandReadDeclaration` -- meaning
    /// every ABI XBUS producer (MMIO XBUS, RSP XBUS) would have panicked on
    /// its first `plan_raw_dpc` call the moment a T4 session was
    /// registered, despite `WgpuBackend`'s own capability advertising
    /// `TransactionalTmemNoFullSync` with no source-kind carve-out.
    #[test]
    fn plan_raw_dpc_accepts_a_genuinely_xbus_sourced_capture() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let request = session.plan_request(xbus_capture(one_load_block_words()));
        let planned = backend.plan_raw_dpc(request);
        assert!(
            planned.is_ok(),
            "an admitted TMEM-only XBUS capture must plan cleanly: {:?}",
            planned.err()
        );
    }

    /// Hostile (nonmutation): dropping the sealed capsule before
    /// `prepare_publication` cancels -- the coordinator's active physical
    /// slot must be completely unchanged, exactly like T0's own
    /// `seal_publication_advances_to_fabric_prepare`/cancellation tests.
    #[test]
    fn dropping_the_capsule_before_prepare_publication_does_not_mutate_active_physical_state() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let prepared = backend.execute_raw_dpc(bound).unwrap();
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        drop(capsule);

        assert_eq!(
            backend.physical_tmem().identity(),
            initial_identity,
            "a dropped, never-published capsule must leave the coordinator's active slot \
             completely untouched"
        );
    }

    /// Hostile (abandoned-ready): `complete_execution` records a ready
    /// physical candidate in the coordinator's inactive slot, but if that
    /// ordinal's `ReadyPublication` is never obtained (e.g. the caller
    /// abandons the flow after `execute_raw_dpc` without ever publishing),
    /// the active slot must remain the original value -- there is no route
    /// from "executed" alone to a physical-state flip.
    #[test]
    fn executing_without_publishing_never_flips_the_active_physical_slot() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let _prepared = backend.execute_raw_dpc(bound).unwrap();

        assert_eq!(
            backend.physical_tmem().identity(),
            initial_identity,
            "execute_raw_dpc alone (no publish_raw_dpc) must never flip the active slot"
        );
    }

    /// Joint-publication: a successful `publish_raw_dpc` call is the one
    /// place the physical-slot flip, the concrete fabric commit, and the
    /// `Published` terminal outcome all happen together, in the same
    /// non-`Result`, callback-free call -- proven by observing all three
    /// facts change atomically across that single call (none change before
    /// it, all three have changed by the time it returns).
    #[test]
    fn publish_raw_dpc_jointly_commits_physical_slot_fabric_and_published_outcome() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend.execute_raw_dpc(bound).unwrap();
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        // Before publish_raw_dpc: neither the physical slot nor the fabric
        // has moved yet.
        assert_eq!(backend.physical_tmem().identity(), initial_identity);
        assert_eq!(fabric.rsp_execution_state().dpc_current, 0x100);

        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);

        assert_eq!(outcome.submission(), submission);
        assert_ne!(backend.physical_tmem().identity(), initial_identity);
        assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
    }

    /// No-route-to-fabric-only-publication: `WgpuBackend::publish_raw_dpc`
    /// is the object-safe trait method a caller uses; its own body is
    /// exactly `self.coordinator.prepare_publication(publication).commit()`
    /// (source below), and `fn64_render::ReadyRawDpcCommitCapsule` itself
    /// exposes no bare `commit`/`CommittedRawDpcOutcome`-returning method
    /// (enforced by T0's own colocated source-shape sweep in
    /// `fn64-render`). This test asserts the source-level shape on the
    /// `fn64-render-wgpu` side: `publish_raw_dpc`'s body contains no
    /// intermediate step between obtaining the capsule and calling
    /// `commit()` -- no fabric-only path exists that could reach
    /// `Published` without also flipping this backend's own physical slot.
    #[test]
    fn publish_raw_dpc_source_is_exactly_prepare_publication_then_commit() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("fn publish_raw_dpc(")
            .expect("publish_raw_dpc must exist in this file");
        let body = &source[body_start..body_start + 400];
        assert!(
            body.contains("self.coordinator.prepare_publication(publication).commit()"),
            "publish_raw_dpc's body must be exactly \
             `self.coordinator.prepare_publication(publication).commit()` -- \
             one non-Result, callback-free terminal path"
        );
    }

    /// Multi-load coverage: a plan with two independent `LoadBlock`s must
    /// execute both -- `into_pending`'s destination-coverage check (backed
    /// by `PhysicalTmemPacketTransaction::expected_destination_accesses`,
    /// which `PhysicalTmemState::stage_neutral_transfer` seeds for the first
    /// load and `stage_neutral_transfer_next` extends for every load after
    /// it) must see every load's destinations, not just the first one's.
    /// Regression coverage for the exact shape a prior version of
    /// `stage_neutral_transfer`/`stage_neutral_transfer_next` got wrong:
    /// either freezing coverage at load one (any second-and-later load's
    /// destinations silently uncounted) or double-counting load one's own
    /// destinations against itself.
    #[test]
    fn plan_execute_publish_completes_with_two_chained_tmem_loads() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let (planned, per_read_bytes) = plan_with_deterministic_reads_for_every_load(
            &mut backend,
            &session,
            two_load_block_words(),
        );
        assert_eq!(
            planned.guest_read_plan().reads().len(),
            2,
            "fixture must actually declare two independent TmemLoadSource reads"
        );
        let guest_capture = guest_read_capture_per_read(&planned, &per_read_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();

        let prepared = backend.execute_raw_dpc(bound).unwrap();
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);
        assert_ne!(
            backend.physical_tmem().identity(),
            initial_identity,
            "a two-load plan must still complete and flip the active physical slot"
        );
    }

    /// State continuity, source-shape half: the TMEM-only admitted subset
    /// has no command that behaviorally *depends* on carried-over
    /// `RdpState` (the one command that reads `state.color_image()` back,
    /// `FillRectangle`, is out of v11's admitted TMEM-only scope), so a
    /// black-box test cannot distinguish "state is threaded through" from
    /// "state is discarded but happens to look populated" purely by
    /// observing `plan_raw_dpc`'s success/failure. This test instead pins
    /// down the source-level fact that makes state threading real: the
    /// *real* (non-probe) decode call inside `plan_raw_dpc_inner` passes
    /// `durable_state` -- the caller-supplied `&RdpState`, not a fresh
    /// `RdpState::default()` -- exactly once, mirroring
    /// `publish_raw_dpc_source_is_exactly_prepare_publication_then_commit`'s
    /// source-shape idiom. The companion behavioral test below
    /// (`plan_raw_dpc_carries_durable_rdp_state_across_submissions`) proves
    /// the field actually accumulates; this one proves decoding actually
    /// consults it instead of a hardcoded default.
    #[test]
    fn plan_raw_dpc_inner_decodes_the_real_pass_against_durable_state_not_default() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("fn plan_raw_dpc_inner(")
            .expect("plan_raw_dpc_inner must exist in this file");
        let next_fn = source[body_start + 1..]
            .find("\nfn ")
            .map(|offset| body_start + 1 + offset)
            .unwrap_or(source.len());
        let body = &source[body_start..next_fn];
        let real_decode_uses_durable_state =
            body.contains("crate::decode_raw_dpc(ticket, durable_state)");
        assert!(
            real_decode_uses_durable_state,
            "plan_raw_dpc_inner's real (non-probe) decode call must pass `durable_state`, \
             not a fresh `RdpState::default()` -- otherwise no submission's state ever \
             carries forward to the next"
        );
        let default_state_appearances = body.matches("RdpState::default()").count();
        assert_eq!(
            default_state_appearances, 1,
            "RdpState::default() must appear exactly once in plan_raw_dpc_inner -- only the \
             throwaway single-source probe decode is allowed to use it; the real decode must \
             use durable_state"
        );
    }

    /// State continuity, behavioral half: `WgpuBackend` must carry durable
    /// `RdpState` (specifically its `tmem()` field here -- `SetTile`
    /// stages into `TmemState`, the one durable-state field the admitted
    /// TMEM-only command subset actually populates; `SetColorImage` is the
    /// distinct command that would populate `color_image()`, and it is not
    /// part of this admitted subset) forward from one `plan_raw_dpc` call to
    /// the next, rather than re-decoding every submission against a fresh
    /// default. Proven here by observing `backend.rdp_state().tmem()`
    /// actually change away from default after a plan that issues a real
    /// `SetTile`, then staying at that value (not reverting to default)
    /// once a second, independent submission plans after it.
    #[test]
    fn plan_raw_dpc_carries_durable_rdp_state_across_submissions() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        assert_eq!(
            backend.rdp_state(),
            &RdpState::default(),
            "a fresh backend starts at default RDP state"
        );

        let tile = crate::tmem::TileIndex::try_new(7).unwrap();

        let request_one = session.plan_request(capture(one_load_block_words()));
        backend
            .plan_raw_dpc(request_one)
            .expect("first submission plans cleanly");
        let epoch_after_first = backend.rdp_state().tmem().tile(tile).last_load_epoch();
        assert!(
            epoch_after_first.is_some(),
            "the first submission's SetTile/LoadBlock must be reflected in durable RDP state"
        );

        // The identical fixture again: if durable state reset to default
        // between submissions, this second call would derive the exact same
        // first epoch as the first call did -- `TmemState`'s own
        // `next_load_epoch` counter is what actually distinguishes "state
        // carried forward" from "state silently reset and looks populated
        // only because this submission repopulated it itself".
        let request_two = session.plan_request(capture(one_load_block_words()));
        backend
            .plan_raw_dpc(request_two)
            .expect("second submission plans cleanly against the carried-forward state");
        let epoch_after_second = backend.rdp_state().tmem().tile(tile).last_load_epoch();
        assert!(
            epoch_after_second.map(|epoch| epoch.get())
                > epoch_after_first.map(|epoch| epoch.get()),
            "a second submission's load epoch ({epoch_after_second:?}) must strictly advance \
             past the first submission's ({epoch_after_first:?}) -- if durable state had reset \
             to default between submissions, both would report the same first epoch"
        );
    }

    /// Two `LoadBlock`s in one submission whose destination TMEM ranges
    /// actually collide (both target `tmem=0`, unlike
    /// `two_load_block_words`'s disjoint `0`/`0x100` split) is legitimate
    /// RDP hardware behavior -- a later load may correctly overwrite bytes
    /// an earlier one just wrote, and physical-TMEM overlap resolution is
    /// exactly what `PhysicalTmemState`'s transaction machinery already
    /// proves at the unit level
    /// (`overlapping_loads_snapshot_intermediate_effect_and_publish_final_postimage`
    /// in `tmem::physical::tests`, and TLUT's own
    /// `back_to_back_loads_to_overlapping_destinations_each_produce_independent_plans`).
    /// This test proves the *full* `WgpuBackend` seam -- `plan_raw_dpc`
    /// through `execute_raw_dpc` through `publish_raw_dpc` -- carries that
    /// same last-write-wins overlap resolution end to end without rejecting
    /// it, complementing the disjoint-destination coverage in
    /// `plan_execute_publish_completes_with_two_chained_tmem_loads`.
    #[test]
    fn plan_execute_publish_completes_with_two_loads_to_overlapping_tmem_destinations() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let initial_identity = backend.physical_tmem().identity();

        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 7 << 24 | 9 << 12 | 0x0800]);
        words.extend(set_tile(6, 2, 0)); // same tmem=0 as the first load above
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 2 << 12 | 1), 6 << 24 | 9 << 12 | 0x0800]);

        let (planned, per_read_bytes) =
            plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
        let guest_capture = guest_read_capture_per_read(&planned, &per_read_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();

        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("overlapping TMEM destinations must complete, not reject");
        let committed = session.commit_zero_guest_writes(prepared).unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();

        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);
        assert_ne!(
            backend.physical_tmem().identity(),
            initial_identity,
            "an overlapping-destination plan must still complete and flip the active slot, \
             exactly like the disjoint-destination case"
        );
    }

    /// Current-identity base: `execute_raw_dpc` always stages against
    /// `coordinator.physical()` -- the currently *active* slot, re-read
    /// fresh on every call -- which only ever changes via a completed
    /// `publish_raw_dpc`, never via `execute_raw_dpc` alone (see
    /// `executing_without_publishing_never_flips_the_active_physical_slot`).
    /// A second, independent `execute_raw_dpc` against that same
    /// still-current active base (no publish between the two calls) is
    /// therefore legitimate, not stale, and must succeed -- proven here by
    /// executing the identical fixture twice in a row against one backend.
    /// (An initial version of this test wrongly expected the second call to
    /// reject as "stale"; it does not, because nothing about the active
    /// slot's identity changed between the two calls, and each call's own
    /// decoded epoch legitimately advances via the durable `RdpState`
    /// continuity `plan_raw_dpc_carries_durable_rdp_state_across_submissions`
    /// proves -- there is no route to a genuinely stale base through this
    /// backend's public, sequential API without a publish in between, and a
    /// publish is exactly what turns "current" into "stale for the old
    /// base".)
    #[test]
    fn executing_the_same_fixture_twice_against_the_same_current_active_base_both_succeed() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        let (planned_one, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture_one = guest_read_capture(&planned_one, &source_bytes);
        let bound_one = session
            .finalize_and_submit(planned_one, guest_capture_one)
            .unwrap();
        backend
            .execute_raw_dpc(bound_one)
            .expect("first execution against the current active base succeeds");

        let (planned_two, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture_two = guest_read_capture(&planned_two, &source_bytes);
        let bound_two = session
            .finalize_and_submit(planned_two, guest_capture_two)
            .unwrap();

        backend.execute_raw_dpc(bound_two).expect(
            "a second execution against the same still-current active base (no publish \
             between the two calls) must also succeed, not be rejected as stale",
        );
    }

    fn fixture_location(command_index: u32) -> fn64_render::RawDpcCommandLocation {
        fn64_render::RawDpcCommandLocation {
            command_index,
            stream_index: 0,
            chunk_index: 0,
            source_address: fn64_render_ir::PhysicalAddress::try_new(0x1000)
                .expect("fixture address is in-bounds"),
            source_byte_offset: 0,
            source_byte_len: 8,
            wire_opcode: 0x08,
        }
    }

    fn fixture_vertex(seed: f32) -> fn64_render::NeutralTriangleVertex {
        fn64_render::NeutralTriangleVertex {
            x: seed,
            y: seed + 1.0,
            z: seed + 2.0,
            w: 1.0,
            color: [seed, seed, seed, 1.0],
            texcoord: [0.0, 0.0],
        }
    }

    fn fixture_triangle(seed: f32) -> RdpTriangleCommand {
        RdpTriangleCommand {
            location: fixture_location(0),
            raw_words: Box::new([]),
            vertices: core::array::from_fn(|index| fixture_vertex(seed + index as f32)),
        }
    }

    fn fixture_set_other_mode(high: u32, low: u32) -> RdpStateCommand {
        RdpStateCommand::SetOtherMode {
            location: fixture_location(0),
            raw_words: Box::new([0, 0]),
            other_mode: fn64_render::NeutralOtherMode { high, low },
            before: None,
            after: fn64_render::RdpStateIdentity::of_other_mode(fn64_render::NeutralOtherMode {
                high,
                low,
            }),
        }
    }

    fn fixture_set_combine(low: u32, high: u32) -> RdpStateCommand {
        RdpStateCommand::SetCombine {
            location: fixture_location(0),
            raw_words: Box::new([0, 0]),
            combine: fn64_render::NeutralCombineParams { low, high },
            before: None,
            after: fn64_render::RdpStateIdentity::of_combine(fn64_render::NeutralCombineParams {
                low,
                high,
            }),
        }
    }

    /// A triangle visited with no `SetOtherMode`/`SetCombine` anywhere --
    /// neither seeded nor in-plan -- must be a loud, named rejection, not
    /// a silent default. Proves `PlanCollector::seeded(None, None)`
    /// (unseeded) genuinely leaves `current_other_mode`/`current_combine`
    /// at `None` rather than defaulting them.
    #[test]
    fn plan_collector_rejects_a_triangle_visited_with_no_state_established_at_all() {
        let mut collector = PlanCollector::seeded(None, None);
        let triangle = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        assert_eq!(collector.triangles.len(), 1);
        assert!(
            matches!(
                collector.triangles[0],
                Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 0 })
            ),
            "expected NoOtherMode at triangle_index 0, got {:?}",
            collector.triangles[0]
        );
    }

    /// `PlanCollector::seeded` with a real durable value closes the
    /// cross-submission carry-in gap: a triangle with no in-plan
    /// `SetOtherMode`/`SetCombine` of its own still resolves cleanly when
    /// seeded from a durable value, mirroring
    /// `production_adapter.rs`'s own
    /// `raw_triangle_is_admitted_using_durable_other_mode_carried_from_a_prior_submission`
    /// at the retrieval layer instead of the admission layer.
    #[test]
    fn plan_collector_seeded_resolves_a_triangle_with_no_in_plan_state_of_its_own() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let seed_combine = CombineParams::from_wire(0, 0);
        let mut collector = PlanCollector::seeded(Some(seed_other_mode), Some(seed_combine));
        let triangle = fixture_triangle(1.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        assert_eq!(collector.triangles.len(), 1);
        let retrieved = collector.triangles[0]
            .as_ref()
            .expect("a triangle with durably-seeded state must resolve, not reject");
        assert_eq!(retrieved.vertices, triangle.vertices);
        assert_eq!(retrieved.other_mode, seed_other_mode);
        assert_eq!(retrieved.combine_params, seed_combine);
    }

    /// Two triangles separated by an intervening `SetCombine` change must
    /// collect **two different** snapshots, not one collapsed
    /// whole-plan-final value -- the exact regression this design avoids
    /// (see `production_adapter.rs`'s own `TriangleDrawStateCollector`
    /// module doc, which independent review found and fixed this same
    /// defect for). The first triangle sees the seeded value; the second
    /// sees the value after the intervening `SetCombine`.
    #[test]
    fn plan_collector_snapshots_each_triangle_at_its_own_stream_position_not_the_final_value() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let seed_combine = CombineParams::from_wire(0, 0);
        let mut collector = PlanCollector::seeded(Some(seed_other_mode), Some(seed_combine));

        let first_triangle = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&first_triangle));

        let changed_combine = fixture_set_combine(0, 1);
        collector.command(RawDpcSemanticCommandRef::State(&changed_combine));

        let second_triangle = fixture_triangle(10.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&second_triangle));

        assert_eq!(collector.triangles.len(), 2);
        let first_retrieved = collector.triangles[0]
            .as_ref()
            .expect("first triangle resolves against the seeded value");
        let second_retrieved = collector.triangles[1]
            .as_ref()
            .expect("second triangle resolves against the post-SetCombine value");
        assert_eq!(
            first_retrieved.combine_params, seed_combine,
            "the first triangle must NOT be retroactively affected by a SetCombine that comes \
             after it in plan order"
        );
        assert_ne!(
            second_retrieved.combine_params, first_retrieved.combine_params,
            "the second triangle must see the changed combine, proving per-triangle snapshots \
             are not collapsed onto one shared value"
        );
    }

    /// A real `SetOtherMode` visited before a triangle overrides the seed
    /// -- the seed is only a starting value, never a fixed override, per
    /// this design's own documented ordering semantics.
    #[test]
    fn plan_collector_lets_an_in_plan_set_other_mode_override_the_seed() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let mut collector =
            PlanCollector::seeded(Some(seed_other_mode), Some(CombineParams::from_wire(0, 0)));

        let changed_other_mode = fixture_set_other_mode(1 << 19, 0);
        collector.command(RawDpcSemanticCommandRef::State(&changed_other_mode));

        let triangle = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let retrieved = collector.triangles[0].as_ref().unwrap();
        assert_ne!(
            retrieved.other_mode, seed_other_mode,
            "an in-plan SetOtherMode must override the seed, not be shadowed by it"
        );
        assert_eq!(retrieved.other_mode, OtherMode::from_wire(1 << 19, 0));
    }

    /// A plan with a triangle and no TMEM load must walk cleanly (no
    /// panic) -- `PlanCollector` is now exhaustive over
    /// `RawDpcSemanticCommandRef`'s real variant set instead of treating
    /// `Triangle` as `unreachable!()`.
    #[test]
    fn plan_collector_walks_a_triangle_only_plan_without_panicking() {
        let mut collector = PlanCollector::seeded(
            Some(OtherMode::from_wire(0, 0)),
            Some(CombineParams::from_wire(0, 0)),
        );
        let triangle = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        assert!(collector.loads.is_empty());
        assert_eq!(collector.triangles.len(), 1);
    }

    fn test_render_config() -> fn64_render::RenderConfig {
        fn64_render::RenderConfig {
            width: 8,
            height: 8,
            tv_type: fn64_runtime::TvType::default(),
        }
    }

    /// Positive: `RenderBackend::create` is a no-op on `WgpuBackend`'s
    /// existing TMEM-only tests -- none of them call `create` at all, so
    /// this backend's whole TMEM-only test surface above is completely
    /// unaffected by `create`'s new eager triangle-pipeline
    /// initialization. This test only proves `create` itself has not
    /// broken compilation/basic construction on a backend that never
    /// touches a triangle -- it deliberately does NOT call `create` before
    /// exercising the existing TMEM-only path, matching every other test
    /// in this module.
    #[test]
    fn tmem_only_path_never_calls_create_and_is_unaffected_by_its_existence() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        // No backend.create(...) call here, deliberately -- mirrors every
        // other TMEM-only test in this module.
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        backend
            .execute_raw_dpc(bound)
            .expect("TMEM-only execution must succeed without ever calling create()");
    }

    #[cfg(feature = "host-gpu-tests")]
    mod host_gpu_tests {
        use super::*;

        /// Required host evidence: a real adapter request succeeds and
        /// `WgpuBackend::create` stores a real `TrianglePipelineRenderer`,
        /// specifically a Metal adapter (asserted below via
        /// `adapter_info().backend`, not merely "some adapter, whatever
        /// it is" -- `host-gpu-tests` is this crate's real-Metal
        /// qualification gate). `create_inner` (not `create`) is called
        /// directly so a `NoAdapter` outcome is distinguishable, by type,
        /// from any other failure -- not to make it non-panicking: a
        /// `NoAdapter` here is still a loud, named panic (`required host
        /// GPU evidence unavailable`), matching this crate's own existing
        /// convention for required host-GPU test evidence
        /// (`device/mod.rs`'s `host_gpu_tests` module panics identically
        /// on its own `HeadlessDeviceOutcome::NoAdapter`). The value of
        /// the typed `WgpuCreateError` here is that this panic message
        /// names exactly which failure occurred, instead of an opaque
        /// `RenderError::Backend` string a caller would have to parse.
        #[test]
        fn create_requests_a_real_metal_adapter_and_stores_the_triangle_pipeline() {
            let (mut backend, _session) = WgpuBackend::try_new().unwrap();
            match backend.create_inner(&test_render_config()) {
                Ok(()) => {
                    let renderer = backend
                        .triangle_pipeline
                        .as_ref()
                        .expect("a successful create() must store a real TrianglePipelineRenderer");
                    assert_eq!(
                        renderer.adapter_info().backend,
                        wgpu::Backend::Metal,
                        "this test qualifies real Metal execution specifically, not merely \
                         some adapter -- got {:?}",
                        renderer.adapter_info()
                    );
                }
                Err(WgpuCreateError::NoAdapter(no_adapter)) => {
                    panic!(
                        "required host GPU evidence unavailable: typed no-adapter for {no_adapter:?}"
                    );
                }
                Err(other) => panic!("create() failed for an unexpected reason: {other}"),
            }
        }

        /// Repeated `create()` calls are an explicit full reset (card
        /// §1a): re-requesting a device from scratch must succeed again,
        /// not error as "already initialized" and not silently no-op.
        #[test]
        fn repeated_create_calls_reset_the_triangle_pipeline_each_time() {
            let (mut backend, _session) = WgpuBackend::try_new().unwrap();
            backend
                .create_inner(&test_render_config())
                .expect("first create() must succeed on a real adapter");
            assert!(backend.triangle_pipeline.is_some());
            backend
                .create_inner(&test_render_config())
                .expect("a second create() call must also succeed, not error or no-op");
            assert!(backend.triangle_pipeline.is_some());
        }
    }
}
