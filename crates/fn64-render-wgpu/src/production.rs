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
    RawDpcIrCapability, RawDpcPlanRequest, RawDpcSemanticCommandRef, ReadyRawDpcCommitCapsule,
    RenderBackend, RenderConfig, RenderError, TmemLoadSemantics, TmemLoadShape,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CapturedGuestRead, CompletedWrite,
    DecodedTicket, ResourceAccess, ResourceJournal, ResourceJournalLimits, SubmittedTicket,
    TicketAuthoritySet, ValidationError, WorkloadAdmission, WorkloadPacket,
};

use crate::raw_dpc::push_decoded_raw_dpc;
use crate::{
    HeadlessBackend, PhysicalTmemError, PhysicalTmemPacketTransaction, PhysicalTmemState,
    RawDpcDecodeError, RdpState, TmemLoadSourceIdentity, TmemTransferWord,
    TrianglePipelineDeviceOutcome, TrianglePipelineError, TrianglePipelineRenderer,
    UninitializedTrianglePipeline,
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
    /// The real GPU triangle-draw pipeline, populated only by a successful
    /// `RenderBackend::create` call -- never by `try_new`, never lazily on
    /// first draw (`WgpuBackend` production triangle-draw integration card
    /// §1a: eager, synchronous initialization is the owner's explicit
    /// decision, superseding an earlier lazy-init draft). `None` until
    /// `create` succeeds; a caller that never calls `create` never pays
    /// GPU-adapter-negotiation cost, since every existing TMEM-only
    /// caller/test already never calls it.
    triangle_pipeline: Option<Box<TrianglePipelineRenderer>>,
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
    /// renderer or reporting a richly-typed failure. Public and callable
    /// directly (unlike `execute_raw_dpc_inner`, which is a private free
    /// function) so a test can assert on the exact `WgpuCreateError`
    /// variant -- specifically distinguishing a genuine `NoAdapter` from
    /// any other failure -- which `RenderBackend::create`'s own
    /// `Result<(), RenderError>` signature cannot preserve once converted
    /// (`RenderError::Backend`'s `reason` is a plain `String`).
    pub fn create_inner(&mut self, _cfg: &RenderConfig) -> Result<(), WgpuCreateError> {
        let outcome = pollster::block_on(
            UninitializedTrianglePipeline::new(HeadlessBackend::default()).request(),
        )
        .map_err(WgpuCreateError::Request)?;
        match outcome {
            TrianglePipelineDeviceOutcome::Ready(renderer) => {
                self.triangle_pipeline = Some(renderer);
                Ok(())
            }
            TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => {
                Err(WgpuCreateError::NoAdapter(no_adapter))
            }
        }
    }
}

/// Named, loud rejection for one `RenderBackend::create` call -- kept
/// distinct from `WgpuRawDpcExecutionError`/`WgpuBackendConstructionError`
/// because this is specifically the triangle-pipeline device-request
/// failure surface. Public so a caller/test can distinguish `NoAdapter`
/// (no exotic device failure, just no matching adapter on this host) from
/// `Request` (a genuine `TrianglePipelineError` -- adapter/device request
/// rejected, or the pipeline prewarm itself reported a device error).
#[derive(Debug)]
pub enum WgpuCreateError {
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
/// `State` commands are not retained here), plus every access, exactly as
/// [`fn64_render::ExactValidatedRawDpcPlan::visit`] lends them through
/// [`BoundSubmittedRawDpc::execution_view`]/
/// [`RawDpcCoordinator::execution_view`] -- nonextracting, borrowed for the
/// duration of one `execution_view` call only. This is the sole route
/// `execute_raw_dpc` uses to reach plan contents; it never widens access to
/// a bare ticket. `State` commands (`SetTile`/`SetTileSize`/
/// `SetTextureImage`/`SyncLoad`) carry no resource access of their own and
/// no field this executor reads -- `TmemLoadSemantics` already carries its
/// own staged `source_image`/`tile_descriptor`/`epoch` directly -- so they
/// are counted for `command_index` continuity but not stored.
#[derive(Default)]
struct PlanCollector {
    loads: Vec<(u32, TmemLoadSemantics)>,
    accesses: Vec<ResourceAccess>,
    next_command_index: u32,
}

impl ExactRawDpcPlanVisitor for PlanCollector {
    fn command(&mut self, command: RawDpcSemanticCommandRef<'_>) {
        let command_index = self.next_command_index;
        self.next_command_index += 1;
        match command {
            RawDpcSemanticCommandRef::TmemLoad(load) => {
                self.loads.push((command_index, load.clone()));
            }
            RawDpcSemanticCommandRef::State(_) => {}
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
    outcome: Option<Result<(BackendEffectReport, PhysicalTmemState), WgpuRawDpcExecutionError>>,
}

impl RawDpcExecutionView<PlanCollector> for ExecutionCollector<'_> {
    fn plan_visited(&mut self, plan_visitor: &mut PlanCollector) {
        self.plan = core::mem::take(plan_visitor);
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
    /// A plan with zero TMEM loads reached execution -- v11's admitted
    /// TMEM/state subset always requires at least one, exactly like
    /// `PhysicalTmemPacketTransaction::into_pending`'s own
    /// `NoCompletedLoads` rejection.
    NoCompletedLoads,
    Physical(PhysicalTmemError),
    Effect(ValidationError),
    Coordinator(ValidationError),
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
    /// Eagerly, synchronously requests the real GPU triangle-draw pipeline
    /// (`WgpuBackend` production triangle-draw integration card §1a): blocks
    /// once on `UninitializedTrianglePipeline::request()` via
    /// `pollster::block_on`. A repeated call is an explicit full reset, not
    /// a no-op and not an error -- it re-requests a device from scratch and
    /// replaces `triangle_pipeline`, dropping the previous renderer's
    /// device/queue/pipeline. `_cfg` is accepted (matching the trait
    /// signature every other backend implements) but unused here: this
    /// backend's fixed-fixture triangle pipeline does not size itself off
    /// `RenderConfig`'s `width`/`height`/`tv_type` (per-draw extent is
    /// supplied per call to `submit_admitted_triangle`, not fixed at
    /// `create` time). Thin wrapper over `Self::create_inner`, mirroring
    /// `execute_raw_dpc`/`execute_raw_dpc_inner`'s existing split in this
    /// file -- `create_inner` returns the richly-typed `WgpuCreateError` a
    /// test can assert on specifically (e.g. distinguishing a genuine
    /// `NoAdapter` from any other failure), which `RenderError::Backend`'s
    /// plain `String` reason cannot preserve once converted.
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
        execute_raw_dpc_inner(&mut self.coordinator, bound).map_err(RenderError::from)
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
fn execute_raw_dpc_inner(
    coordinator: &mut RawDpcCoordinator<PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
) -> Result<BackendPreparedRawDpc, WgpuRawDpcExecutionError> {
    let mut plan_visitor = PlanCollector::default();
    let mut view = ExecutionCollector {
        physical: coordinator.physical(),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::default(),
        reads: Vec::new(),
        outcome: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    let _ = plan_visitor; // plan contents were moved into `view.plan` by `plan_visited`

    let (effects, next_physical) = view
        .outcome
        .expect("execution_view always calls submitted_packet exactly once")?;

    coordinator
        .complete_execution(bound, effects, next_physical)
        .map_err(WgpuRawDpcExecutionError::Coordinator)
}

/// The pipeline `submitted_packet` runs once `&WorkloadPacket` is in scope:
/// stage every ordered TMEM load into one packet-local transaction via
/// `PhysicalTmemState::stage_neutral_transfer` (T3 Phase B's own neutral
/// counterpart to the decoder-typed `stage_transfer`), seal it into a
/// `PendingTmemTransaction`, compute the exact `BackendEffectReport` from
/// its own proposed effects, and derive this transaction's
/// `into_physical_successor` (T3 Phase A) candidate.
fn stage_and_report(
    collector: &ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<(BackendEffectReport, PhysicalTmemState), WgpuRawDpcExecutionError> {
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

    let packet_transaction =
        packet_transaction.ok_or(WgpuRawDpcExecutionError::NoCompletedLoads)?;
    let pending = packet_transaction
        .into_pending()
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    let writes: Vec<CompletedWrite> = pending.proposed_effects().to_vec();
    let effects =
        BackendEffectReport::try_new(packet, writes).map_err(WgpuRawDpcExecutionError::Effect)?;

    let next_physical = pending
        .into_physical_successor(collector.physical, &effects)
        .map_err(WgpuRawDpcExecutionError::Physical)?;

    Ok((effects, next_physical))
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

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
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
        /// `WgpuBackend::create` stores a real `TrianglePipelineRenderer`.
        /// `create_inner` (not `create`) is called directly so a
        /// `NoAdapter` outcome on a genuinely headless CI host is
        /// reportable as a named, non-panicking skip rather than an
        /// opaque `RenderError::Backend` string match.
        #[test]
        fn create_requests_a_real_adapter_and_stores_the_triangle_pipeline() {
            let (mut backend, _session) = WgpuBackend::try_new().unwrap();
            match backend.create_inner(&test_render_config()) {
                Ok(()) => {
                    assert!(
                        backend.triangle_pipeline.is_some(),
                        "a successful create() must store a real TrianglePipelineRenderer"
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
