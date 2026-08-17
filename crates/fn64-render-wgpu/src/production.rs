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
//! **Guest-write nonclaim.** An admitted `FillRectangle` declares
//! guest-visible `RenderTarget` *journal* writes and commits them through
//! `RawDpcAbiSession::commit_guest_render_target_writes`. Nothing in that
//! chain modifies guest RDRAM: `execute_fill_rectangle` produces an owned
//! `Vec<u8>`, `ResidentPublication::publish` writes into a backend-local
//! `Vec`, and a `CompletedWrite` is a range plus a content digest, not
//! bytes in motion. The RDRAM copyback is a separate, deferred slice, and
//! no code here may be described as "publishing to guest memory".

use fn64_render::{
    BackendPreparedRawDpc, BoundSubmittedRawDpc, CommittedRawDpcOutcome, ExactRawDpcPlanVisitor,
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcCoordinator, RawDpcExecutionView,
    RawDpcIrCapability, RawDpcPlanRequest, RawDpcSemanticCommandRef, RdpStateCommand,
    RdpTriangleCommand, ReadyRawDpcCommitCapsule, RenderBackend, RenderConfig, RenderError,
    TmemLoadSemantics, TmemLoadShape, TriangleSource,
};
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CapturedGuestRead, CompletedWrite,
    DecodedTicket, ResourceAccess, ResourceJournal, ResourceJournalLimits, SubmittedTicket,
    TicketAuthoritySet, ValidationError, WorkloadAdmission, WorkloadPacket,
};

use crate::raw_dpc::push_decoded_raw_dpc;
use crate::targets::{admitted_triangle_fixture, ResolvedFragmentBlendParams};
use crate::tmem::{project_committed_tmem, TileBindingParams};
use crate::{
    execute_fill_rectangle, AlphaCompare, BlendColorInput, BlendModeState, Color4,
    ColorTargetExtent, ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, CombineParams,
    FillColor, FillExecutionError, FillRectangle, HeadlessBackend, InitializedCandidateColorTarget,
    MissingTriangleDrawState, OtherMode, PhysicalTmemError, PhysicalTmemPacketTransaction,
    PhysicalTmemState, PrimColor, RawDpcDecodeError, RdpState, ResolvedBlendCycle,
    RetrievedTriangleDraw, TargetError, TmemLoadSourceIdentity, TmemTransferWord,
    TriangleDrawOutput, TrianglePipelineDeviceOutcome, TrianglePipelineError,
    TrianglePipelineRenderer, TriangleRasterParams, TriangleTargetExtent,
    UninitializedTrianglePipeline, TMEM_SAMPLE_STATUS_OK,
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
    initialized: InitializedCandidateColorTarget,
    /// The exact N `CompletedWrite`s this fill contributed to the
    /// submission's `BackendEffectReport`, in journal order.
    guest_writes: Vec<CompletedWrite>,
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
                configured_target_extent: None,
                color_targets: None,
                pending_fill_publication: None,
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
        // Recorded before the device request, unlike `triangle_target_extent`
        // below: an admitted `FillRectangle` is executed entirely CPU-side
        // and needs only this host-configured height, so a host with no GPU
        // adapter must still be able to execute one. See
        // `configured_target_extent`'s own doc for the nonclaim this carries.
        self.configured_target_extent = Some(TriangleTargetExtent {
            width: cfg.width,
            height: cfg.height,
        });
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
    ) -> Result<(), WgpuRawDpcExecutionError> {
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let extent = self
            .triangle_target_extent
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        // Published committed-TMEM textured-draw card §2: the committed
        // physical TMEM byte image this draw samples against, projected
        // once per `draw_admitted_triangles` call (not once per triangle --
        // every triangle in one call shares the same committed-TMEM
        // snapshot, since no TMEM commit happens between triangles within a
        // single `execute_raw_dpc` call).
        let tmem = project_committed_tmem(self.coordinator.physical());

        let mut fixtures = Vec::with_capacity(triangles.len());
        for (triangle_index, draw) in triangles.into_iter().enumerate() {
            let draw = draw.map_err(WgpuRawDpcExecutionError::MissingTriangleDrawState)?;
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
            let blend_mode = BlendModeState {
                other_mode: draw.other_mode,
                blend_color_register: draw.blend_color.map_or([0u8; 4], |color| color.rgba8()),
                fog_color: draw.fog_color.map_or([0u8; 4], |color| color.rgba8()),
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
            fixtures.push(admitted_triangle_fixture(
                draw.vertices,
                draw.other_mode,
                draw.combine_params,
                raster_params,
                extent,
                tmem,
                draw.tile_binding,
                draw.blend_color,
                draw.env_color,
                draw.prim_color,
                blend_params,
                draw.source == TriangleSource::TextureRectangle,
            ));
        }

        if fixtures.is_empty() {
            return Ok(());
        }

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
            return Err(WgpuRawDpcExecutionError::TmemSampleFailed { status });
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
    /// Tile 0's binding current at the walk's current stream position
    /// (published committed-TMEM textured-draw card §2: "extend
    /// `PlanCollector`... to snapshot the current `TmemState` tile
    /// bindings onto each triangle"). Tile 0 is the RDP's default bound
    /// texture tile for a standard triangle draw -- `RdpTriangleCommand`
    /// carries no tile index of its own. Mirrors
    /// `raw_dpc::triangle_draw_data::TriangleDrawStateCollector`'s own
    /// identical fields exactly (this struct's own module doc already
    /// states this file duplicates that collector's behavior).
    current_tile0_descriptor: Option<fn64_render::NeutralTileDescriptor>,
    current_tile0_size: Option<fn64_render::NeutralTileSize>,
    /// `G_SETBLENDCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetBlendColor` command
    /// in plan order. Mirrors `current_other_mode`/`current_combine`
    /// exactly, a third instance of the same seed-then-track pattern (card
    /// §4d).
    current_blend_color: Option<Color4>,
    /// `G_SETENVCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetEnvColor` command
    /// in plan order. Mirrors `current_blend_color`, but unconditionally
    /// tracked -- no `AlphaCompare` gate.
    current_env_color: Option<Color4>,
    /// `G_SETPRIMCOLOR` current at the walk's current stream position --
    /// mirrors `current_env_color` exactly.
    current_prim_color: Option<PrimColor>,
    /// `G_SETFOGCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetFogColor` command
    /// in plan order. Mirrors `current_env_color`/`current_prim_color`
    /// exactly. Needed by the production blend-cycle wiring's `Fog`
    /// selector.
    current_fog_color: Option<Color4>,
    /// One entry per admitted `Triangle` command, in plan order. `Err`
    /// names exactly which state (`OtherMode` or `CombineParams`) was
    /// still unset at that triangle's own stream position -- never a
    /// silent default, matching `TriangleDrawStateCollector`'s own
    /// documented absence handling.
    triangles: Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
    /// One entry per admitted `FillRectangle` command, in plan order,
    /// paired with its own decode-order command index. Unlike `triangles`,
    /// no draw-state snapshot is taken here: a fill carries its own
    /// `color_image`/`fill_color` on the command itself (see
    /// `RdpFillRectangleCommand`), so nothing needs to be tracked across
    /// the walk for it.
    fills: Vec<(u32, fn64_render::RdpFillRectangleCommand)>,
    /// One entry per admitted `FullSync` site, in plan order, paired with
    /// its own decode-order command index.
    ///
    /// Collected for accounting only -- this backend performs no GPU work
    /// for a sync and schedules no DP completion (the device fabric does
    /// that, from the ABI seam). Retaining the site keeps the executed plan
    /// able to account for every command it carried instead of silently
    /// losing one.
    full_sync_sites: Vec<(u32, fn64_render::RdpFullSyncSite)>,
}

impl PlanCollector {
    /// Seeds `current_other_mode`/`current_combine`/`current_blend_color`
    /// from `WgpuBackend`'s own durable `rdp_state` instead of `None` -- a
    /// real constructor parameter, never a synthetic plan-stream entry.
    /// This is the draw-state-*retrieval* half of durable cross-submission
    /// carry-in; the admission-time half (`push_decoded_raw_dpc`'s own
    /// `TriangleBeforeAnyOtherMode` gate) already seeds identically from
    /// the same `rdp_state`, via `decode_raw_dpc`'s existing
    /// `durable_state` parameter -- see this card's own design notes for
    /// why neither half needed a signature change to close this gap.
    fn seeded(
        other_mode: Option<OtherMode>,
        combine: Option<CombineParams>,
        blend_color: Option<Color4>,
        env_color: Option<Color4>,
        prim_color: Option<PrimColor>,
        fog_color: Option<Color4>,
    ) -> Self {
        Self {
            loads: Vec::new(),
            accesses: Vec::new(),
            next_command_index: 0,
            current_other_mode: other_mode,
            current_combine: combine,
            current_tile0_descriptor: None,
            current_tile0_size: None,
            current_blend_color: blend_color,
            current_env_color: env_color,
            current_prim_color: prim_color,
            current_fog_color: fog_color,
            triangles: Vec::new(),
            fills: Vec::new(),
            full_sync_sites: Vec::new(),
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
                RdpStateCommand::SetBlendColor { color, .. } => {
                    self.current_blend_color = Some(Color4::from_wire(color.value));
                }
                RdpStateCommand::SetEnvColor { color, .. } => {
                    self.current_env_color = Some(Color4::from_wire(color.value));
                }
                RdpStateCommand::SetPrimColor { color, .. } => {
                    self.current_prim_color = Some(PrimColor::from_wire(
                        u32::from(color.lod_frac) | (u32::from(color.lod_min) << 8),
                        color.color,
                    ));
                }
                RdpStateCommand::SetFogColor { color, .. } => {
                    self.current_fog_color = Some(Color4::from_wire(color.value));
                }
                RdpStateCommand::SetTile {
                    tile_index,
                    descriptor,
                    ..
                } if *tile_index == 0 => {
                    self.current_tile0_descriptor = Some(*descriptor);
                }
                RdpStateCommand::SetTileSize {
                    tile_index, size, ..
                } if *tile_index == 0 => {
                    self.current_tile0_size = Some(*size);
                }
                _ => {}
            },
            RawDpcSemanticCommandRef::Triangle(RdpTriangleCommand {
                vertices,
                source,
                viewport,
                ..
            }) => {
                let triangle_index = self.triangles.len();
                let tile_binding = match (self.current_tile0_descriptor, self.current_tile0_size) {
                    (Some(descriptor), Some(size)) => {
                        TileBindingParams::from_neutral(descriptor, size)
                    }
                    _ => TileBindingParams::unbound(),
                };
                let snapshot = (|| {
                    let other_mode = self
                        .current_other_mode
                        .ok_or(MissingTriangleDrawState::NoOtherMode { triangle_index })?;
                    let combine_params = self
                        .current_combine
                        .ok_or(MissingTriangleDrawState::NoCombine { triangle_index })?;
                    // Retrieval-time admission gate (card §4a), duplicated
                    // from `TriangleDrawStateCollector` per this struct's
                    // own module doc: `Reserved`/`Dither` never reach
                    // `submit_admitted_triangle` -- loud, named panics here,
                    // not a silent None/Threshold coercion.
                    match other_mode.alpha_compare() {
                        AlphaCompare::Reserved => panic!(
                            "triangle #{triangle_index} (plan order) selected reserved G_AC \
                             alpha-compare mode 2"
                        ),
                        AlphaCompare::Dither => panic!(
                            "triangle #{triangle_index} (plan order) selected G_AC_DITHER \
                             alpha-compare, which has no fragment-callable RT64 PRNG binding in \
                             this pipeline (no frame-count uniform exists to seed it honestly; \
                             see fn64-alpha-compare-production-card.md \u{a7}2)"
                        ),
                        AlphaCompare::Threshold => {
                            self.current_blend_color
                                .ok_or(MissingTriangleDrawState::NoBlendColor { triangle_index })?;
                        }
                        AlphaCompare::None => {}
                    };
                    Ok(RetrievedTriangleDraw {
                        vertices: *vertices,
                        source: *source,
                        viewport: *viewport,
                        other_mode,
                        combine_params,
                        tile_binding,
                        blend_color: self.current_blend_color,
                        env_color: self.current_env_color,
                        prim_color: self.current_prim_color,
                        fog_color: self.current_fog_color,
                    })
                })();
                self.triangles.push(snapshot);
            }
            // Mandatory alongside `push_fill_rectangle`'s admission: the
            // enum is `#[non_exhaustive]`, so a produced variant with no arm
            // here falls into the catch-all below and panics at execute time
            // rather than failing to compile.
            RawDpcSemanticCommandRef::FillRectangle(fill) => {
                self.fills.push((command_index, fill.clone()));
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
struct ExecutionCollector<'coord> {
    physical: &'coord PhysicalTmemState,
    queue: fn64_render_ir::QueueIdentity,
    ordinal: u64,
    submission: fn64_render_ir::SubmissionIdentity,
    plan: PlanCollector,
    reads: Vec<(u32, Vec<u8>)>,
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
    /// This plan staged at least one guest-visible color-target write and
    /// completed zero TMEM loads. Structurally distinct from both siblings:
    /// unlike `TriangleOnly` it carries a nonempty `BackendEffectReport` (so
    /// `complete_execution_preserving_physical`, which builds its own empty
    /// one, is not a legal destination), and unlike `TmemLoads` it offers no
    /// `PhysicalTmemState` successor -- a color-target write does not touch
    /// physical TMEM at all.
    ///
    /// Carries the staged fill token out of `stage_and_report` so
    /// `execute_raw_dpc_inner` can hand it to the backend only after the
    /// coordinator accepted the completion.
    GuestWritesOnly(BackendEffectReport, StagedFill),
}

/// One fill's execution result, staged inside `stage_and_report` and moved
/// out through [`StagedOutcome::GuestWritesOnly`]. Becomes a
/// [`PendingFillPublication`] once `execute_raw_dpc_inner` knows which
/// submission it belongs to and the coordinator has accepted the completion.
struct StagedFill {
    initialized: InitializedCandidateColorTarget,
    guest_writes: Vec<CompletedWrite>,
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
            PlanCollector::seeded(None, None, None, None, None, None),
        );
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
    /// At least one fragment of an admitted triangle's draw reported a
    /// non-`TMEM_SAMPLE_STATUS_OK` status through `tmem_sample.wgsl`'s
    /// observable shader-failure-status channel (published committed-TMEM
    /// textured-draw card, audit repair) -- a missing tile binding, a
    /// reversed clamp extent, an invalid (never-loaded) TMEM byte in the
    /// triangle's UV footprint, or an unsupported production format.
    /// `status` is the first such code found, in row-major fragment order;
    /// never silently accepted as a successful textured draw.
    TmemSampleFailed {
        status: u32,
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
    /// The fill executor itself rejected the rectangle -- non-Fill cycle, a
    /// Z/framebuffer-consumer bypass hazard, a fractional edge, or missing
    /// resident bytes.
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
    /// This packet declared both TMEM loads and an admitted
    /// `FillRectangle`. The merged `BackendEffectReport` write list would
    /// have to interleave both sources in the plan's own journal order, and
    /// no in-tree fixture produces such a stream today -- so rather than
    /// ship an untested merge, this slice rejects the combination loudly.
    /// Admitting it is a follow-on slice, not a silent reorder.
    MixedFillAndTmemLoadPacket,
    /// This packet declared both an admitted `FillRectangle` and at least
    /// one admitted triangle. The two run entirely disjoint render paths:
    /// the fill is executed CPU-side into an owned buffer staged behind
    /// `PendingFillPublication`, while `draw_admitted_triangles` clears and
    /// rasterizes into a GPU color attachment that never composes back into
    /// that buffer. Executing both would publish a resident generation
    /// carrying only the fill while the triangles landed somewhere the
    /// guest can never observe -- with no defined ordering between them.
    /// Composing the two sources is a follow-on slice; this is the loud
    /// refusal in the meantime, exactly as with
    /// [`Self::MixedFillAndTmemLoadPacket`].
    MixedFillAndTrianglePacket,
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
            Self::TmemSampleFailed { status } => write!(
                formatter,
                "a triangle draw's fragment shader reported a non-OK tmem_sample.wgsl status: \
                 {status}"
            ),
            Self::BlendRequiresFramebuffer { triangle_index } => write!(
                formatter,
                "triangle #{triangle_index} (plan order) selected a blend-cycle input that reads \
                 the framebuffer alpha (coverage count); this crate does not yet implement \
                 framebuffer-alpha-dependent blending"
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
            Self::MixedFillAndTmemLoadPacket => formatter.write_str(
                "this packet declares both TMEM loads and an admitted FillRectangle; merging \
                 both write sources into the plan's own journal order is a follow-on slice, \
                 rejected loudly here rather than reordered silently",
            ),
            Self::MixedFillAndTrianglePacket => formatter.write_str(
                "this packet declares both an admitted FillRectangle and at least one admitted \
                 triangle; the CPU-side fill and the GPU triangle raster target are disjoint \
                 with no defined composition or ordering between them, so the combination is \
                 rejected loudly here rather than half-executed silently",
            ),
        }
    }
}

impl From<TargetError> for WgpuRawDpcExecutionError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}

impl From<FillExecutionError> for WgpuRawDpcExecutionError {
    fn from(error: FillExecutionError) -> Self {
        Self::FillExecution(error)
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
        if let Some(triangle_target_extent) = self.triangle_target_extent.as_mut() {
            *triangle_target_extent = extent;
        }
    }

    fn supported_ucodes(&self) -> &[fn64_render::UcodeId] {
        &[]
    }

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
        let (prepared, triangles, pending) = execute_raw_dpc_inner(
            &mut self.coordinator,
            bound,
            self.rdp_state.other_mode(),
            self.rdp_state.combine(),
            self.rdp_state.blend_color(),
            self.rdp_state.env_color(),
            self.rdp_state.prim_color(),
            self.rdp_state.fog_color(),
            &mut self.color_targets,
            self.configured_target_extent,
        )
        .map_err(RenderError::from)?;

        if !triangles.is_empty() {
            self.draw_admitted_triangles(triangles)
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
            .map(|pending| pending.guest_writes.clone())
            .unwrap_or_default()
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
        let pending = self.pending_fill_publication.take();
        let submission = publication.submission();
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

        if let Some(pending) = pending {
            assert_eq!(
                pending.submission, submission,
                "publish_raw_dpc received a capsule for a different submission than the one \
                 execute_raw_dpc staged a color-target write for"
            );
            let registry = self
                .color_targets
                .as_mut()
                .expect("a staged fill publication implies the registry was built");
            registry
                .prepare_publication(pending.initialized)
                .unwrap_or_else(|error| {
                    panic!("color-target publication rejected after guest commit: {error}")
                })
                .publish();
        }
        outcome
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
        capture.full_sync_boundaries().to_vec(),
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
        capture.full_sync_boundaries().to_vec(),
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
///
/// `full_sync_boundaries` is NOT zero-filled the way the read bytes are, and
/// must be the originating capture's own list. Stream derivation requires one
/// boundary per decoded `SYNC_FULL` opcode, so an empty list here would fail
/// both internal decode passes with `MissingFullSyncObservation` for any
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
/// `durable_other_mode`/`durable_combine`/`durable_blend_color`/
/// `durable_env_color`/`durable_prim_color`/`durable_fog_color` are
/// `WgpuBackend.rdp_state`'s own current values, passed in by the trait
/// method (which has `self`) since this is a free function taking only
/// `coordinator` -- they seed `PlanCollector`'s walk (`PlanCollector::seeded`)
/// so a triangle in this submission with no `SetOtherMode`/`SetCombine`/
/// `SetBlendColor`/`SetEnvColor`/`SetPrimColor`/`SetFogColor` of its own
/// still resolves its draw state from durable cross-submission carry-in, not
/// `None`.
#[allow(clippy::too_many_arguments)]
fn execute_raw_dpc_inner(
    coordinator: &mut RawDpcCoordinator<PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
    durable_other_mode: Option<OtherMode>,
    durable_combine: Option<CombineParams>,
    durable_blend_color: Option<Color4>,
    durable_env_color: Option<Color4>,
    durable_prim_color: Option<PrimColor>,
    durable_fog_color: Option<Color4>,
    color_targets: &mut Option<ColorTargetRegistry>,
    configured_target_extent: Option<TriangleTargetExtent>,
) -> Result<
    (
        BackendPreparedRawDpc,
        Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        Option<PendingFillPublication>,
    ),
    WgpuRawDpcExecutionError,
> {
    let mut plan_visitor = PlanCollector::seeded(
        durable_other_mode,
        durable_combine,
        durable_blend_color,
        durable_env_color,
        durable_prim_color,
        durable_fog_color,
    );
    let mut view = ExecutionCollector {
        physical: coordinator.physical(),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::seeded(
            durable_other_mode,
            durable_combine,
            durable_blend_color,
            durable_env_color,
            durable_prim_color,
            durable_fog_color,
        ),
        reads: Vec::new(),
        outcome: None,
        color_targets,
        configured_target_extent,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    let _ = plan_visitor; // plan contents were moved into `view.plan` by `plan_visited`

    let submission = bound.submission();
    let outcome = view
        .outcome
        .expect("execution_view always calls submitted_packet exactly once")?;
    let triangles = view.plan.triangles;
    let mut pending = None;

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
                initialized: staged.initialized,
                guest_writes: staged.guest_writes,
            });
            prepared
        }
    };

    Ok((prepared, triangles, pending))
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
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<StagedOutcome, WgpuRawDpcExecutionError> {
    // Mixed TMEM-load-plus-fill packets are rejected before either source
    // stages anything: merging both write lists into the plan's own
    // journal-ordered write-access sequence is a follow-on slice, and
    // shipping an untested merge would be worse than a loud, named refusal.
    if !collector.plan.fills.is_empty() && !collector.plan.loads.is_empty() {
        return Err(WgpuRawDpcExecutionError::MixedFillAndTmemLoadPacket);
    }
    // Mixed fill-plus-triangle packets are refused at the same point, in
    // the same shape, and for the same reason. `stage_fills_and_report`
    // never inspects `plan.triangles`, and `execute_raw_dpc` draws them
    // afterwards into a color attachment `draw_admitted_triangles` clears
    // itself -- disjoint from the CPU-side fill buffer this packet staged.
    // Admitting the pair would silently drop one of two render results
    // with no ordering defined between them. Composing them is a follow-on
    // slice; refusing by name is the honest answer until then.
    if !collector.plan.fills.is_empty() && !collector.plan.triangles.is_empty() {
        return Err(WgpuRawDpcExecutionError::MixedFillAndTrianglePacket);
    }
    if !collector.plan.fills.is_empty() {
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
/// Exactly one fill per packet is admitted in this slice: a second fill
/// would need its own candidate against a registry the first has not
/// published into yet, so its `predecessor` would be stale by construction.
/// That is a loud rejection, not a silent overwrite.
///
/// Nonclaim: nothing here writes guest RDRAM. `execute_fill_rectangle`
/// produces an owned `Vec<u8>`, and the `CompletedWrite`s are ranges plus
/// content digests, not bytes in motion.
fn stage_fills_and_report(
    collector: &mut ExecutionCollector<'_>,
    packet: &WorkloadPacket,
) -> Result<StagedOutcome, WgpuRawDpcExecutionError> {
    if collector.plan.fills.len() > 1 {
        return Err(WgpuRawDpcExecutionError::MixedFillAndTmemLoadPacket);
    }
    let (_, fill) = collector.plan.fills[0].clone();

    // The `OtherMode` current at this fill's own stream position, tracked by
    // `PlanCollector`'s walk exactly the way a triangle's is. `plan_fill`
    // already refused to admit a fill without a staged fill-cycle
    // `OtherMode`, so `None` here would mean the plan and the walk
    // disagree -- rejected loudly, never defaulted to wire zero (which
    // decodes as Fill cycle with no hazard bits and would silently execute
    // a rectangle the RDP never asked for).
    let Some(other_mode) = collector.plan.current_other_mode else {
        return Err(WgpuRawDpcExecutionError::FillExecution(
            FillExecutionError::NotFillCycle,
        ));
    };

    let Some(extent) = collector.configured_target_extent else {
        return Err(WgpuRawDpcExecutionError::NoColorTargetHeight);
    };

    let format = ColorTargetFormat::try_from_rdp(
        image_format(fill.color_image.format),
        pixel_size(fill.color_image.size),
    )?;
    let key = ColorTargetKey::try_new(
        fill.color_image.address,
        ColorTargetExtent::try_new(fill.color_image.width, extent.height)?,
        format,
    )?;

    // Built lazily, from this capture's own layout: neither `try_new` nor
    // `create` has one to build it from, and inventing one would be a
    // fabricated fact. A later capture whose layout differs is rejected by
    // `begin_candidate`'s own `MemoryLayoutMismatch` check below, never by
    // silently rebuilding and dropping every resident generation.
    if collector.color_targets.is_none() {
        *collector.color_targets = Some(ColorTargetRegistry::try_new(
            packet.memory_layout(),
            COLOR_TARGET_REGISTRY_CAPACITY,
        )?);
    }
    let registry = collector
        .color_targets
        .as_mut()
        .expect("just populated above");

    let candidate = registry.begin_candidate(key)?;
    let resident_bytes = registry
        .residents()
        .iter()
        .find(|resident| resident.key() == key)
        .map(|resident| resident.device_bytes().device_bytes());

    let completed = execute_fill_rectangle(
        &candidate,
        other_mode,
        FillColor::from_wire(fill.fill_color.value),
        FillRectangle::from_wire_fields(
            fill.upper_left_x,
            fill.upper_left_y,
            fill.lower_right_x,
            fill.lower_right_y,
        ),
        resident_bytes,
    )?;

    let accesses = fill_accesses(&collector.plan.accesses, &fill)?;
    let guest_writes = fill_completed_writes(key, completed.device_bytes(), accesses)?;
    let initialized = candidate.admit_completed_initialization(completed)?;

    let effects = BackendEffectReport::try_new(packet, guest_writes.clone())
        .map_err(WgpuRawDpcExecutionError::Effect)?;

    Ok(StagedOutcome::GuestWritesOnly(
        effects,
        StagedFill {
            initialized,
            guest_writes,
        },
    ))
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

fn pixel_size(size: fn64_render::NeutralPixelSize) -> crate::PixelSize {
    match size {
        fn64_render::NeutralPixelSize::Bits4 => crate::PixelSize::Bits4,
        fn64_render::NeutralPixelSize::Bits8 => crate::PixelSize::Bits8,
        fn64_render::NeutralPixelSize::Bits16 => crate::PixelSize::Bits16,
        fn64_render::NeutralPixelSize::Bits32 => crate::PixelSize::Bits32,
    }
}

#[cfg(test)]
mod tests {
    use fn64_render::OwnedRawDpcSubmission;
    use fn64_render_ir::{
        CapturedGuestRead, DeferredGuestReadCapture, DpInterruptState, TemporalBoundary,
    };

    use crate::{
        BlendBInput, BlenderCycle, ImageFormat, PixelSize, TileAddressMode, TileCoordinate,
        TileDescriptor, TileSize, TmemWordAddress,
    };

    use super::*;

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    const SET_TEXTURE_IMAGE: u8 = 0x3d;
    const SET_TILE: u8 = 0x35;
    const SET_TILE_SIZE_OPCODE: u8 = 0x32;
    const LOAD_SYNC: u8 = 0x26;
    const LOAD_BLOCK: u8 = 0x33;
    const FULL_SYNC: u8 = 0x29;
    const SET_OTHER_MODE: u8 = 0x2f;
    const SET_COMBINE: u8 = 0x3c;
    const SET_ENV_COLOR: u8 = 0x3b;
    const SET_PRIM_COLOR: u8 = 0x3a;
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

    /// Mirrors `raw_dpc::production_adapter::tests::set_env_color` exactly
    /// (that helper is private to its own module's tests, so this is a
    /// local, identical copy, not a shared import -- same convention as
    /// `triangle_base_edge_words` above).
    fn set_env_color(color: u32) -> [u32; 2] {
        [word(SET_ENV_COLOR, 0), color]
    }

    /// Mirrors `raw_dpc::production_adapter::tests::set_prim_color` exactly,
    /// same local-copy convention.
    fn set_prim_color(lod_frac: u32, lod_min: u32, color: u32) -> [u32; 2] {
        [word(SET_PRIM_COLOR, lod_min << 8 | lod_frac), color]
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

    /// Same fixture shape as `capture`, but carrying one `FullSyncBoundary`
    /// per `SYNC_FULL` opcode in `words` -- what a producer that took the
    /// nonmutating `preflight_dp_full_sync` reserve half supplies.
    ///
    /// Both interrupt states are `Clear`. That mirrors the real ABI producer
    /// exactly: a reservation observes no interrupt, and the device fabric
    /// raises the DP line only on a later `advance_to`, strictly after this
    /// capture would have been built.
    fn full_sync_capture(words: Vec<u32>) -> fn64_render::OwnedRawDpcCapture {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
        let sites = fn64_render::count_raw_rdp_full_sync_sites(&words).unwrap();
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, end, words.clone()).unwrap();
        let boundaries = (0..sites as u64)
            .map(|ordinal| {
                fn64_render_ir::FullSyncBoundary::new(
                    2 + ordinal * 2,
                    3 + ordinal * 2,
                    DpInterruptState::Clear,
                    DpInterruptState::Clear,
                )
            })
            .collect();
        fn64_render::OwnedRawDpcCapture::with_full_sync_boundaries(
            submission,
            layout,
            7,
            TemporalBoundary::new(1, DpInterruptState::Clear),
            boundaries,
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

        // PRIMITIVE-passthrough SetCombine: (A-B)*C+D collapses to D, and D
        // now decodes to PRIMITIVE (index 3, `color_input_d`) instead of
        // SHADE -- this genuinely exercises Slice B's new
        // `fragment_material_params` uniform rather than continuing to
        // collapse to a SHADE-only formula where env/prim would silently
        // not matter (production-combiner-slice-b-card §6 step 2).
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 3;
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

        // Real SetEnvColor/SetPrimColor wire commands (card §6 step 1),
        // pushed before the triangle so Slice A's command-time capture
        // (`RetrievedTriangleDraw.env_color`/`.prim_color`) resolves them.
        let env_color_wire: u32 = 0x1122_33AA;
        let prim_lod_frac_wire: u32 = 0x40;
        let prim_lod_min_wire: u32 = 0x05;
        let prim_color_wire: u32 = 0x4455_66BB;

        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(low, high));
        words.extend(set_env_color(env_color_wire));
        words.extend(set_prim_color(
            prim_lod_frac_wire,
            prim_lod_min_wire,
            prim_color_wire,
        ));
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
        let base_inputs = crate::combiner::CombinerInputs {
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
        // Real env_color/prim_color values (via
        // combiner_inputs_from_fragment_registers, the exact Rust-side
        // machinery Slice B's uniform mirrors) instead of hardcoded zero --
        // proves the expected value matches what the production path now
        // actually computes.
        let inputs = crate::combiner::combiner_inputs_from_fragment_registers(
            base_inputs,
            crate::state::Color4::from_wire(env_color_wire),
            crate::state::PrimColor::from_wire(
                prim_lod_min_wire << 8 | prim_lod_frac_wire,
                prim_color_wire,
            ),
        );
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
        // D now decodes to PRIMITIVE (not SHADE), so the expected color's
        // RGB channels come from prim_color_wire's real bytes; alpha_d is
        // still SHADE(4), so alpha comes from the triangle's own shade
        // alpha (triangle_color_255[3] == 255, normalized to 1.0).
        let prim_color_rgba8 = prim_color_wire.to_be_bytes();
        assert_eq!(
            expected_u8,
            [
                prim_color_rgba8[0],
                prim_color_rgba8[1],
                prim_color_rgba8[2],
                triangle_color_255[3] as u8,
            ]
        );
    }

    const TEXRECT: u8 = 0x24;
    const TEXRECT_FLIP: u8 = 0x25;

    /// One `TextureRectangle`/`TextureRectangleFlip` command's 4-word wire
    /// payload -- same bit layout as `raw_dpc::production_adapter`'s own
    /// `texrect_words`, but this fixture's `ulx=8, uly=8, lrx=24, lry=24`
    /// (2.0/2.0/6.0/6.0px, `.2` fixed point) places a 4x4-pixel rectangle
    /// entirely inside `test_render_config`'s 8x8 target, at `[2, 6) x
    /// [2, 6)`, unlike `production_adapter.rs`'s own fixture (which targets
    /// a much larger, offscreen-for-8x8 render target). `dsdx=dtdy=0`
    /// (constant `uls=ult=0` texcoord for every vertex) keeps every covered
    /// fragment's sample well inside the 2x2 tile's interior, including the
    /// 3-nearest filter's `+1` neighbor read -- this fixture's job is to
    /// prove the rectangle's real pixel POSITION, not to exercise a UV
    /// gradient (`required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`
    /// already covers gradient/interpolation correctness for a `RawTriangle`).
    fn texrect_words(opcode: u8, tile: u32) -> [u32; 4] {
        let ulx: u32 = 8;
        let uly: u32 = 8;
        let lrx: u32 = 24;
        let lry: u32 = 24;
        let uls: u32 = 0;
        let ult: u32 = 0;
        let dsdx: u32 = 0x0000;
        let dtdy: u32 = 0x0000;
        [
            word(opcode, (lrx << 12) | lry),
            (tile & 0x7) << 24 | (ulx << 12) | uly,
            (uls << 16) | ult,
            (dsdx << 16) | dtdy,
        ]
    }

    /// Loads this module's frozen 2x2 RGBA16 texel fixture, commits, and
    /// publishes it, exactly like
    /// `required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`'s
    /// own load-then-draw split: `project_committed_tmem` only reflects the
    /// coordinator's ACTIVE (already-published) physical slot, never a load
    /// still pending within the same `execute_raw_dpc` call -- so a
    /// texture-sampling draw must be a SEPARATE, later `execute_raw_dpc`
    /// from its own load, not batched into one command stream with it.
    fn load_and_publish_fixture_texture(backend: &mut WgpuBackend, session: &mut RawDpcAbiSession) {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(load_sync());
        let source_bytes = fixture_load_block_source_bytes();
        words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

        let (planned, _unused_deterministic_bytes) =
            plan_with_deterministic_reads(backend, session, words);
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture's TMEM-only load stays inside the admitted subset");
        let committed = session.commit_zero_guest_writes(prepared).unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
    }

    /// The real end-to-end test (texture-rectangle placement card §3): a
    /// real decoded capture containing `SetOtherMode`/`SetCombine`/one
    /// `TextureRectangle` (opcode `0x24`), sampling this module's already-
    /// committed fixture texture, pushed through the actual production
    /// entry points (`WgpuBackend::create`/`plan_raw_dpc`/
    /// `execute_raw_dpc`), asserted against real GPU-observed pixel output
    /// at the pixel range `[left, right) x [top, bottom)` this rectangle's
    /// own `ulx`/`uly`/`lrx`/`lry` place it at -- genuinely wire-position-
    /// faithful, not a fixed-corner artifact (the gap this card's own §0
    /// closes).
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position() {
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
        load_and_publish_fixture_texture(&mut backend, &mut session);

        // Re-declare the tile binding: tile-binding state does not persist
        // across separate `execute_raw_dpc` calls (`PlanCollector` is fresh
        // per plan) -- only `project_committed_tmem`'s underlying physical
        // TMEM bytes persist, via publish.
        let mut words = Vec::new();
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(set_other_mode(0, 0));
        // TEXEL0-passthrough SetCombine, same idiom as
        // `required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle`.
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 1; // TEXEL0
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 1; // TEXEL0
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        words.extend(set_combine(low, high));
        words.extend(texrect_words(TEXRECT, 0));

        let planned = plan_with_no_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &[]);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        backend
            .execute_raw_dpc(bound)
            .expect("fixture stays inside the admitted state+rect subset");

        let output = backend
            .last_triangle_draw()
            .expect("a successful rect-bearing execute_raw_dpc must populate last_triangle_draw");

        // `texrect_words`' own fixture: [left, right) x [top, bottom) ==
        // [2, 6) x [2, 6) in this 8x8 target. Every covered pixel samples
        // TEXEL0 -- a real (non-uniform) texture read, so this only proves
        // real position, not that every pixel has the same color; the
        // uncovered corner proves the rectangle did NOT cover the whole
        // target (a fixed-NDC-corner bug would cover all 64 pixels).
        let width = output.extent.width;
        let covered_pixel_index = (2 * width + 2) as usize * 4;
        let covered = [
            output.color_rgba8[covered_pixel_index],
            output.color_rgba8[covered_pixel_index + 1],
            output.color_rgba8[covered_pixel_index + 2],
            output.color_rgba8[covered_pixel_index + 3],
        ];
        assert_ne!(
            covered,
            [0, 0, 0, 0],
            "pixel (2,2) is inside [2,6)x[2,6) and must be covered by the real rectangle position"
        );
        let outside_pixel_index = 0usize;
        let outside = [
            output.color_rgba8[outside_pixel_index],
            output.color_rgba8[outside_pixel_index + 1],
            output.color_rgba8[outside_pixel_index + 2],
            output.color_rgba8[outside_pixel_index + 3],
        ];
        assert_eq!(
            outside,
            [0, 0, 0, 0],
            "pixel (0,0) is outside [2,6)x[2,6) and must stay the Clear color -- a fixed-NDC-\
             corner bug would cover the whole 8x8 target and fail this assertion"
        );
    }

    /// Flip sibling of
    /// `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`
    /// (texture-rectangle placement card §3 item 3): `TextureRectangleFlip`
    /// (opcode `0x25`) places the SAME rectangle at the SAME pixel range --
    /// flip only transposes UV pairing (`texture_rectangle.rs`'s own
    /// module doc), never position -- proving flip ordering survives all
    /// the way to real pixel coverage, not just the CPU-side vertex/texcoord
    /// unit tests `raw_dpc::texture_rectangle`/`production_adapter` already
    /// cover.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn wgpu_backend_draws_a_real_texture_rectangle_flip_at_the_same_wire_position() {
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
        load_and_publish_fixture_texture(&mut backend, &mut session);

        // Re-declare the tile binding: see the non-flip sibling's own
        // comment for why this is required per-plan, not durable.
        let mut words = Vec::new();
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(set_other_mode(0, 0));
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 1; // TEXEL0
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 1; // TEXEL0
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        words.extend(set_combine(low, high));
        words.extend(texrect_words(TEXRECT_FLIP, 0));

        let planned = plan_with_no_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &[]);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        backend
            .execute_raw_dpc(bound)
            .expect("fixture stays inside the admitted state+rect subset");

        let output = backend
            .last_triangle_draw()
            .expect("a successful rect-bearing execute_raw_dpc must populate last_triangle_draw");

        // Same [2,6)x[2,6) placement as the non-flip sibling -- flip must
        // not move the rectangle.
        let width = output.extent.width;
        let covered_pixel_index = (2 * width + 2) as usize * 4;
        let covered = [
            output.color_rgba8[covered_pixel_index],
            output.color_rgba8[covered_pixel_index + 1],
            output.color_rgba8[covered_pixel_index + 2],
            output.color_rgba8[covered_pixel_index + 3],
        ];
        assert_ne!(
            covered,
            [0, 0, 0, 0],
            "flip must not change the rectangle's covered pixel range"
        );
        let outside_pixel_index = 0usize;
        let outside = [
            output.color_rgba8[outside_pixel_index],
            output.color_rgba8[outside_pixel_index + 1],
            output.color_rgba8[outside_pixel_index + 2],
            output.color_rgba8[outside_pixel_index + 3],
        ];
        assert_eq!(
            outside,
            [0, 0, 0, 0],
            "flip must not change the rectangle's covered pixel range"
        );
    }

    /// Published committed-TMEM textured-draw card §4: the frozen literal
    /// texel values (four fully-saturated primary/neutral colors, one per
    /// corner), corrected against this crate's own real
    /// `LoadBlock`/tile-addressing implementation for the *source image
    /// width and byte layout*, rather than copied blind from the card's
    /// prose. See this function's own doc below for the one correction.
    ///
    /// RGBA16, 2x2 TILE (the card's own frozen extent). Texel `(0,0)` = red
    /// `0xF801` -> RGBA8 `(255,0,0,255)`; `(1,0)` = green `0x07C1` ->
    /// `(0,255,0,255)`; `(0,1)` = blue `0x003F` -> `(0,0,255,255)`; `(1,1)`
    /// = white `0xFFFF` -> `(255,255,255,255)`.
    ///
    /// **Source-image-width correction (this slice's own verification, not
    /// a copy of the card's literal byte string):** `tmem/read.rs`'s
    /// `linear_byte_address` computes each row's TMEM start as `row *
    /// tile.line_words() * 8` -- always a whole-8-byte-word multiple.
    /// `tmem/wire.rs`'s `transfer_shape` for `LoadBlock` transfers exactly
    /// `source.total_bytes()` bytes as ONE flat linear run (`dxt=0` mode,
    /// no row-interleave), so if the card's own literal 2-texel-wide
    /// SOURCE IMAGE were used, row 1 (texels `(0,1)`/`(1,1)`) would land at
    /// source/TMEM byte 4 -- not a whole-word multiple, so no `line_words`
    /// value can make the READ side find it there (`line_words=1` looks
    /// for row 1 at byte 8; `line_words=0` aliases every row to byte 0).
    /// This fixture instead uses a 4-texel-wide SOURCE IMAGE (so one row is
    /// naturally exactly one 8-byte TMEM word: `4 texels * 2 bytes = 8
    /// bytes`), `LoadBlock`s the top 2 rows x 4 columns (8 texels, still
    /// one linear `dxt=0` transfer, still admitted), and the 2x2 TILE's own
    /// `SetTile`/`SetTileSize` addresses only that image's left 2x2
    /// sub-region (columns 0-1, `mask_s`/`mask_t` left at 0 so clamp mode
    /// bounds the tile exactly to `high.integer()-low.integer()+1 == 2`).
    /// Columns 2-3 of each row are filler, never addressed by this tile.
    /// `line_words=1` (one whole word/row) now correctly finds row 1 at
    /// byte 8, matching `LoadBlock`'s own real linear placement. The three
    /// assertion points' expected colors are computed by literally calling
    /// this crate's own `address_texture_cell`/`gather_committed_texture_cell`/
    /// `filter_three_nearest_committed_cell` chain against this corrected
    /// layout -- not hand-derived arithmetic copied from the card, which is
    /// exactly the kind of mismatch this verification step exists to catch.
    const FIXTURE_TMEM_WORD_ADDRESS: u16 = 0;
    const FIXTURE_LINE_WORDS: u16 = 1;
    const FIXTURE_SOURCE_IMAGE_WIDTH: u32 = 4;

    /// **Odd-row XOR4 correction (this slice's own verification against
    /// the real read path, not assumed from the card's prose):**
    /// `LoadBlock` writes its whole transfer as ONE linear run
    /// (`tmem/wire.rs`'s `transfer_shape` `Block` arm always reports
    /// `row_count = 1`, so its own `odd_row_exchange` never fires) --
    /// hardware treats a block load as texel-address-agnostic bytes, not
    /// discrete tile rows. But the READ side (`tmem_rgba16_texel_address`/
    /// `tmem/read.rs`'s `linear_byte_address`+`odd_row_exchange`) DOES
    /// apply the XOR4 swap to any texel whose TILE-relative row is odd,
    /// under this slice's frozen `TmemFirstRowParity::Even` (card §6): row
    /// 1 (odd) XORs its computed address by 4. Since the write never
    /// exchanged but the read always will for row 1, this fixture's source
    /// bytes for row-1 texels must be placed at their POST-XOR4 TMEM
    /// offsets directly: texel (0,1) reads from address `8 XOR 4 = 12`,
    /// texel (1,1) reads from address `10 XOR 4 = 14`. Bytes 8-11 (row 1's
    /// un-exchanged half) are filler, never read by this tile's own two
    /// column addresses under the exchange.
    fn fixture_load_block_source_bytes() -> Vec<u8> {
        vec![
            0xf8, 0x01, // (0,0) red -- row 0 (even), no exchange
            0x07, 0xc1, // (1,0) green -- row 0 (even), no exchange
            0x00, 0x00, // (2,0) filler, never addressed by the 2x2 tile
            0x00, 0x00, // (3,0) filler
            0x00, 0x00, // byte 8-9: row-1 UN-exchanged half, never read
            0x00, 0x00, // byte 10-11: row-1 UN-exchanged half, never read
            0x00, 0x3f, // byte 12-13: (0,1) blue's real post-XOR4 address
            0xff, 0xff, // byte 14-15: (1,1) white's real post-XOR4 address
        ]
    }

    fn fixture_tile_descriptor() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::Rgba,
            PixelSize::Bits16,
            FIXTURE_LINE_WORDS,
            TmemWordAddress::try_new(FIXTURE_TMEM_WORD_ADDRESS).unwrap(),
            0,
            TileAddressMode::default(),
            0,
            0,
            TileAddressMode::default(),
            0,
            0,
        )
    }

    fn fixture_tile_size() -> TileSize {
        // S10.2 raw units: `TileCoordinate::integer() = raw >> 2`, so
        // `high - low + 1 == 2` texels wide/tall needs `high.integer() ==
        // 1` (raw `4`) with `low.integer() == 0` (raw `0`).
        TileSize::from_wire(
            TileCoordinate::try_new(0).unwrap(),
            TileCoordinate::try_new(0).unwrap(),
            TileCoordinate::try_new(4).unwrap(),
            TileCoordinate::try_new(4).unwrap(),
        )
    }

    /// CPU oracle side of the differential: the real
    /// `address_texture_cell`/`gather_committed_texture_cell`/
    /// `filter_three_nearest_committed_cell` chain, invoked directly with
    /// no GPU involved (card §4/§7 requirement).
    fn cpu_oracle_sample(physical: &PhysicalTmemState, raw_s: i16, raw_t: i16) -> [u8; 4] {
        cpu_oracle_sample_with_tile(
            physical,
            fixture_tile_descriptor(),
            fixture_tile_size(),
            raw_s,
            raw_t,
        )
    }

    /// Same CPU oracle chain as `cpu_oracle_sample`, parameterized over the
    /// tile descriptor/size -- used by the negative-coordinate repair test
    /// below, which needs a wrap-addressed (not clamp-addressed) tile: under
    /// this crate's frozen clamp fixture (`fixture_tile_descriptor`'s own
    /// `mask_s`/`mask_t == 0`, which forces `clamps = true` unconditionally
    /// per `address_axis_texel`), any negative `base_texel` clamps to column/
    /// row 0 on BOTH the correct-floor and buggy-truncate paths, and the
    /// resulting blended color is provably identical either way (the clamp
    /// formula's two branches agree exactly at that boundary) -- so a clamp
    /// fixture cannot discriminate floor from truncation for a negative
    /// coordinate. A `mask=1` wrap tile (non-clamp, non-mirror) instead
    /// addresses each axis by parity (`coordinate & 1`), so a negative
    /// `base_texel` of differing parity under floor vs. truncation selects
    /// genuinely different corners, not a saturated boundary.
    fn cpu_oracle_sample_with_tile(
        physical: &PhysicalTmemState,
        tile: TileDescriptor,
        size: TileSize,
        raw_s: i16,
        raw_t: i16,
    ) -> [u8; 4] {
        let request = crate::PointSampleRequest::new(
            crate::PointSampleCoordinates::new(
                crate::TextureCoordinateS10_5::from_raw(raw_s),
                crate::TextureCoordinateS10_5::from_raw(raw_t),
            ),
            crate::TmemFirstRowParity::Even,
        );
        let cell = crate::gather_committed_texture_cell(
            physical,
            tile,
            size,
            request,
            crate::TextureLutMode::Disabled,
        )
        .expect("fixture's assertion points stay inside the addressed footprint");
        crate::filter_three_nearest_committed_cell(cell)
    }

    /// Wrap-mode (`mask=1`, non-clamp, non-mirror) sibling of
    /// `fixture_tile_descriptor` over the exact same committed 2x2 RGBA16
    /// texel layout -- see `cpu_oracle_sample_with_tile`'s doc for why
    /// wrap (not clamp) addressing is required to discriminate floor from
    /// truncation at a negative coordinate.
    fn fixture_wrap_tile_descriptor() -> TileDescriptor {
        TileDescriptor::from_wire(
            ImageFormat::Rgba,
            PixelSize::Bits16,
            FIXTURE_LINE_WORDS,
            TmemWordAddress::try_new(FIXTURE_TMEM_WORD_ADDRESS).unwrap(),
            0,
            TileAddressMode::from_wire(0), // t: mirror=false, clamp=false
            1,                             // mask_t = 1 (2-texel wrap period)
            0,
            TileAddressMode::from_wire(0), // s: mirror=false, clamp=false
            1,                             // mask_s = 1 (2-texel wrap period)
            0,
        )
    }

    /// CPU-only half of the differential (card §4/§7 requirement "(a) the
    /// CPU oracle chain ... invoked directly in a `#[cfg(test)]` unit test
    /// with no GPU involved"): runs this fixture's real production
    /// TMEM-only load through `WgpuBackend::execute_raw_dpc` (no GPU
    /// adapter required -- `execute_raw_dpc` only reaches the triangle
    /// pipeline when a plan admits a `Triangle` command, which a TMEM-only
    /// plan never does), then asserts hand-derivable properties of
    /// `cpu_oracle_sample`'s output directly against
    /// `address_texture_cell`/`gather_committed_texture_cell`/
    /// `filter_three_nearest_committed_cell`. Exact texel-center point
    /// `(16,16)` has no filtering ambiguity (three-nearest weighting at an
    /// exact-center coordinate reduces to selecting that corner at full
    /// weight) and must equal pure red -- this is the load-bearing
    /// assertion the GPU-side differential below reuses as its own known-
    /// good anchor.
    #[test]
    fn required_cpu_tmem_oracle_matches_hand_derived_texel_colors() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(load_sync());
        let source_bytes = fixture_load_block_source_bytes();
        words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

        let (planned, _unused_deterministic_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture's TMEM-only load stays inside the admitted subset");
        // `physical_tmem()` reflects the coordinator's ACTIVE physical
        // slot, which only flips at publish (see
        // `plan_execute_publish_completes_and_flips_active_physical_slot`'s
        // own doc/assertion) -- `execute_raw_dpc` alone stages the
        // candidate but does not publish it.
        let committed = session.commit_zero_guest_writes(prepared).unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);

        // Addressing convention (this slice's own verification against
        // `relative_axis_coordinate`/`address_axis_texel`/
        // `filter_three_nearest`, corrected from an initial wrong
        // assumption that raw=16 was a texel's own unambiguous center):
        // `base_texel = raw.div_euclid(32)`, `fraction = raw.rem_euclid(32)`,
        // and `filter_three_nearest`'s weight is 100% on corner `(s0,t0)`
        // only when `fraction == 0` on both axes (`sf=tf=0` collapses
        // `value = c00*32` exactly). So a texel's own unambiguous center is
        // at raw = `base_texel * 32`, NOT `base_texel*32 + 16` -- `+16`
        // instead lands exactly halfway toward the NEXT texel, which is
        // this fixture's genuinely-blended "tile center" case below.
        //
        // Exact address of texel (0,0): red, no filtering ambiguity
        // (sf=tf=0, base_texel=(0,0)).
        assert_eq!(
            cpu_oracle_sample(backend.physical_tmem(), 0, 0),
            [255, 0, 0, 255]
        );
        // Exact address of texel (1,0): green (base_texel=(1,0)).
        assert_eq!(
            cpu_oracle_sample(backend.physical_tmem(), 32, 0),
            [0, 255, 0, 255]
        );
        // Exact address of texel (0,1): blue (base_texel=(0,1)).
        assert_eq!(
            cpu_oracle_sample(backend.physical_tmem(), 0, 32),
            [0, 0, 255, 255]
        );
        // Exact address of texel (1,1): white (base_texel=(1,1), clamped
        // to the tile's own [0,1] dimension on each axis).
        assert_eq!(
            cpu_oracle_sample(backend.physical_tmem(), 32, 32),
            [255, 255, 255, 255]
        );
        // Genuine four-corner blend (card's own "tile's geometric center"
        // intent): raw=(16,16), `base_texel=(0,0)`, `sf=tf=16` (halfway
        // toward `s1`/`t1`) -- `filter_three_nearest`'s `sf+tf<=32` branch
        // gives `value = c00*32 + 16*(c10-c00) + 16*(c01-c00)` per channel,
        // hand-substituted here directly from red `(255,0,0)`, green
        // `(0,255,0)`, blue `(0,0,255)` (the `c11`/white corner does not
        // enter this branch at all, since `sf+tf=32<=32`):
        // R: 255*32 + 16*(0-255) + 16*(0-255) = 8160 - 4080 - 4080 = 0
        // G: 0*32 + 16*(255-0) + 16*(0-0) = 4080 -> round((4080+16)/32) = 128
        // B: 0*32 + 16*(0-0) + 16*(255-0) = 4080 -> 128
        // A: 255*32 + 16*(255-255) + 16*(255-255) = 8160 -> 255
        let tile_center = cpu_oracle_sample(backend.physical_tmem(), 16, 16);
        assert_eq!(tile_center, [0, 128, 128, 255]);

        // Negative-coordinate floor-vs-truncation repair (independent
        // adversarial-review finding): `triangle_pipeline_fragment.wgsl`
        // used to compute `i32(uv.x)` directly on the interpolated `f32` raw
        // S10.5 coordinate, which truncates toward zero instead of flooring
        // toward negative infinity -- disagreeing with this CPU oracle's
        // (and `tmem_sample.wgsl`'s own `relative_axis_coordinate` port's)
        // `div_euclid`/`rem_euclid` floor convention for any negative
        // coordinate. This fixture's own clamp-addressed tile
        // (`fixture_tile_descriptor`, `mask_s = mask_t = 0`) cannot expose
        // that bug: `address_axis_texel` clamps unconditionally when
        // `mask == 0`, so `base_texel = -1` (the correct floor of raw `-1`)
        // and `base_texel = 0` (`i32`-truncated raw `0`, the wrong result if
        // truncation were used instead of floor) both clamp `s0`/`s1` to the
        // SAME column pair regardless -- the two address paths are
        // mathematically indistinguishable in the final blended color at
        // that boundary. `fixture_wrap_tile_descriptor` instead uses
        // `mask_s = mask_t = 1` (non-clamp, non-mirror wrap addressing over
        // the SAME committed 2x2 texel layout): a wrap-addressed axis
        // selects by parity (`coordinate & 1`), so floor(-1) (odd) and
        // trunc(0) (even) address genuinely different, non-collapsing
        // corners.
        //
        // At raw S10.5 point (s=-1, t=0): `relative_axis_coordinate` gives
        // `base_s = (-1).div_euclid(32) = -1`, `frac_s =
        // (-1).rem_euclid(32) = 31`; `base_t = 0`, `frac_t = 0`. Wrap
        // addressing (`mask=1`, period 2): `s0 = (-1) & 1 = 1`, `s1 = 0 & 1
        // = 0`, `t0 = 0 & 1 = 0`, `t1 = 1 & 1 = 1`. Corners: `c00 =
        // color(s0=1,t0=0)` = green `(0,255,0,255)`, `c10 =
        // color(s1=0,t0=0)` = red `(255,0,0,255)`, `c01 =
        // color(s0=1,t1=1)` = white `(255,255,255,255)`. `sf=31, tf=0,
        // sf+tf=31<=32` selects `filter_three_nearest`'s first branch:
        // `value = c00*32 + 31*(c10-c00) + 0*(c01-c00)` per channel:
        // R: 0*32 + 31*(255-0) + 0 = 7905 -> round((7905+16)/32) = 247
        // G: 255*32 + 31*(0-255) + 0 = 8160-7905 = 255 -> round((255+16)/32) = 8
        // B: 0*32 + 31*(0-0) + 0 = 0 -> 0
        // A: 255*32 + 31*(255-255) + 0 = 8160 -> 255
        // This CPU oracle result (the ONLY correct answer, since this
        // module's own `TextureCoordinateS10_5`/`address_axis_texel` chain
        // has always used `div_euclid`/`rem_euclid`, never truncation) is
        // asserted exactly below, with zero tolerance. This is the
        // discriminating value the GPU-side differential test also samples,
        // where the pre-repair `i32(uv.x)` bug would instead have addressed
        // `base_texel = 0` (truncating -1.0's neighborhood toward zero) and
        // produced pure red `(255,0,0,255)` -- a different, wrong result
        // this exact assertion rules out.
        let negative_coordinate = cpu_oracle_sample_with_tile(
            backend.physical_tmem(),
            fixture_wrap_tile_descriptor(),
            fixture_tile_size(),
            -1,
            0,
        );
        assert_eq!(negative_coordinate, [247, 8, 0, 255]);
    }

    /// Published committed-TMEM textured-draw card §4/§7 (mandatory exit
    /// gate): the new fragment-callable WGSL TMEM addressing/filter chain
    /// (`shaders/tmem_sample.wgsl`, wired through
    /// `triangle_pipeline_fragment.wgsl`'s `fs_main`) must agree with the
    /// CPU oracle chain, both computed independently -- CPU side via
    /// `cpu_oracle_sample` above (no GPU, see
    /// `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`), GPU
    /// side through the real fragment pipeline on a host-GPU adapter
    /// (`WgpuBackend::execute_raw_dpc` for the TMEM commit,
    /// `TrianglePipelineRenderer::submit_admitted_triangle` -- reached via
    /// the `#[cfg(test)]`-only `triangle_pipeline_for_test` accessor -- for
    /// the textured triangle draw itself). UVs are chosen so the
    /// rasterizer's own per-fragment interpolation (not a per-triangle
    /// constant) produces two genuinely different sample points, each
    /// algebraically derived below from the real vertex UVs and the actual
    /// pixel-center sampling convention, not copied magic numbers.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle() {
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

        // Real production TMEM-load path: SetTextureImage/SetTile(0)/
        // SetTileSize(0)/LoadSync/LoadBlock, admitted and executed through
        // `WgpuBackend::execute_raw_dpc` exactly like every other TMEM-load
        // test in this module. SetTextureImage's own width is the SOURCE
        // IMAGE'S width (4 texels, see this test's own doc above) -- not
        // the 2x2 TILE's width, which `SetTileSize` below states
        // separately.
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        // SetTileSize: low_s=0, low_t=0, high_s=4 (raw S10.2; `integer() ==
        // 1`), high_t=4 -- a 2x2-texel tile (`high.integer() -
        // low.integer() + 1 == 2` on each axis), matching
        // `fixture_tile_size()` exactly.
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(load_sync());
        let source_bytes = fixture_load_block_source_bytes();
        // LoadBlock w0: source_s=0, source_t=0 (top-left of the 4-wide
        // source image). w1: tile=0, high_s=7 (eight texels, 0..=7
        // inclusive, spanning both rows of the 4-wide image --
        // `decode_load_block`'s `texels = high_s - source_s + 1 == 8`),
        // dxt=0 (pure-linear mode: no row-interleave -- correct here
        // because the source image's own natural row width, 4 texels * 2
        // bytes = 8 bytes, is already exactly one TMEM word, so a flat
        // linear copy already lands row 1 at the same word-aligned offset
        // `line_words=1`'s read-side formula expects).
        words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

        let (planned, _unused_deterministic_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture's TMEM-only load stays inside the admitted subset");
        // Unlike a bare `execute_raw_dpc` discard, this test needs the
        // TMEM write to be durably visible: `physical_tmem()` only reflects
        // the coordinator's ACTIVE slot after publish (see
        // `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
        // doc) -- skipping commit/seal/publish here left stale/invalid TMEM
        // active, which is what real-Metal execution caught
        // (`InvalidTexelByte` at the fixture's own addressed footprint).
        let committed = session.commit_zero_guest_writes(prepared).unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);

        let tmem = project_committed_tmem(backend.physical_tmem());
        let tile_binding = TileBindingParams::bound(fixture_tile_descriptor(), fixture_tile_size());

        // Screen geometry matches `covering_triangle_fixture`'s own right
        // triangle exactly: vertex0=(0,0), vertex1=(8,0), vertex2=(0,8),
        // all w=1 (perspective divide is a no-op) -- so the rasterizer's
        // barycentric UV interpolation at pixel-center `(px, py) = (x+0.5,
        // y+0.5)` is the plain affine formula `uv = uv0 + wx*(uv1-uv0) +
        // wy*(uv2-uv0)`, `wx = px/8`, `wy = py/8`, matching this file's own
        // `expected_interpolated_rgba8` color-interpolation precedent
        // (`targets/triangle_pipeline/tests.rs`) applied to UV instead of
        // color. UV0=(16,16), UV1=(112,16), UV2=(16,112): a genuinely
        // varying UV gradient (not constant) sweeping well past the 2x2
        // tile's own 64-unit extent, so pixel-center interpolation lands at
        // real, hand-computed points inside the tile -- computed
        // algebraically below, not guessed or copied from the card.
        let uv0 = (16.0f32, 16.0f32);
        let uv1 = (112.0f32, 16.0f32);
        let uv2 = (16.0f32, 112.0f32);
        let interpolated_uv = |x: u32, y: u32| -> (i16, i16) {
            let wx = (x as f32 + 0.5) / 8.0;
            let wy = (y as f32 + 0.5) / 8.0;
            let s = uv0.0 + wx * (uv1.0 - uv0.0) + wy * (uv2.0 - uv0.0);
            let t = uv0.1 + wx * (uv1.1 - uv0.1) + wy * (uv2.1 - uv0.1);
            (s.round() as i16, t.round() as i16)
        };
        // Pixel (0,0)'s center (px=py=0.5, wx=wy=0.0625): s=t=16+0.0625*96=22.
        let pixel_a = (0u32, 0u32);
        // Pixel (2,2)'s center (px=py=2.5, wx=wy=0.3125): s=t=16+0.3125*96=46.
        let pixel_b = (2u32, 2u32);
        let (a_s, a_t) = interpolated_uv(pixel_a.0, pixel_a.1);
        let (b_s, b_t) = interpolated_uv(pixel_b.0, pixel_b.1);
        assert_eq!((a_s, a_t), (22, 22));
        assert_eq!((b_s, b_t), (46, 46));

        let assertion_points: [(i16, i16); 2] = [(a_s, a_t), (b_s, b_t)];
        let expected: Vec<[u8; 4]> = assertion_points
            .iter()
            .map(|&(s, t)| cpu_oracle_sample(backend.physical_tmem(), s, t))
            .collect();

        let vertices = [
            fn64_render::NeutralTriangleVertex {
                x: 0.0,
                y: 0.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv0.0, uv0.1],
            },
            fn64_render::NeutralTriangleVertex {
                x: 8.0,
                y: 0.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv1.0, uv1.1],
            },
            fn64_render::NeutralTriangleVertex {
                x: 0.0,
                y: 8.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv2.0, uv2.1],
            },
        ];
        // TEXEL0 passthrough SetCombine: (A-B)*C+D with A=TEXEL0(1), B=0
        // (COMBINED), C=ONE-equivalent... instead use the simplest faithful
        // identity available in the common table: D=TEXEL0 with A=B=
        // COMBINED(0) (zeroing the (A-B)*C term), matching this file's own
        // established SHADE-passthrough idiom but selecting TEXEL0 (index 1)
        // for D instead of SHADE (index 4).
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 1; // TEXEL0
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 1; // TEXEL0
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        let combine_params = CombineParams::from_wire(low, high);

        let renderer = backend.triangle_pipeline_for_test();
        let raster_params = TriangleRasterParams {
            resolution: [8.0, 8.0],
            screen_scale: [1.0, 1.0],
            screen_offset: [0.0, 0.0],
        };
        let output = renderer
            .submit_admitted_triangle(
                vertices,
                OtherMode::from_wire(0, 0),
                combine_params,
                raster_params,
                TriangleTargetExtent {
                    width: 8,
                    height: 8,
                },
                tmem,
                tile_binding,
                None,
                None,
                None,
                ResolvedFragmentBlendParams::NO_OP,
                false,
            )
            .expect("textured triangle draw must submit cleanly")
            .complete()
            .expect("textured triangle draw must complete cleanly");

        assert!(
            output
                .tmem_sample_status
                .iter()
                .all(|&status| status == TMEM_SAMPLE_STATUS_OK),
            "every fragment's tmem_sample.wgsl status must be OK for this fixture"
        );

        let observed_at = |x: u32, y: u32| -> [u8; 4] {
            let index = (y as usize * 8 + x as usize) * 4;
            [
                output.color_rgba8[index],
                output.color_rgba8[index + 1],
                output.color_rgba8[index + 2],
                output.color_rgba8[index + 3],
            ]
        };
        // Both assertion points: the real GPU fragment output at each
        // pixel, sourced through the actual per-fragment
        // `sample_committed_rgba16_three_nearest_bound` WGSL call, compared
        // against the CPU oracle chain computed above at the SAME
        // algebraically-derived interpolated UV -- the card's mandatory
        // CPU-vs-WGSL differential (§4/§7), both sides independent.
        assert_close_rgba8_channels(observed_at(pixel_a.0, pixel_a.1), expected[0], 2);
        assert_close_rgba8_channels(observed_at(pixel_b.0, pixel_b.1), expected[1], 2);
    }

    #[cfg(feature = "host-gpu-tests")]
    fn assert_close_rgba8_channels(observed: [u8; 4], expected: [u8; 4], tolerance: i32) {
        for channel in 0..4 {
            let diff = i32::from(observed[channel]) - i32::from(expected[channel]);
            assert!(
                diff.abs() <= tolerance,
                "channel {channel}: observed={observed:?} expected={expected:?} \
                 tolerance={tolerance}"
            );
        }
    }

    /// Negative-coordinate floor-vs-truncation repair, GPU half (independent
    /// adversarial-review finding; CPU half is
    /// `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
    /// `negative_coordinate` assertion -- see that assertion's doc comment
    /// for the full discrimination argument and the wrap-vs-clamp addressing
    /// reasoning). `triangle_pipeline_fragment.wgsl` used to compute
    /// `i32(uv.x)`/`i32(uv.y)` directly on the interpolated `f32` raw S10.5
    /// coordinate -- truncating toward zero -- before calling
    /// `sample_committed_rgba16_three_nearest_bound`. This test's geometry
    /// makes the rasterizer's own per-fragment interpolation land exactly on
    /// a fractional NEGATIVE raw S coordinate at pixel (0,0), so a real GPU
    /// run exercises the actual bug site (the fragment shader's own
    /// `f32`->`i32` conversion of an interpolated value), not just the
    /// integer-only WGSL addressing chain downstream of it -- unlike the
    /// CPU-only half above, which starts from an already-integer raw
    /// coordinate and cannot observe this specific conversion-site defect by
    /// itself.
    ///
    /// This fixture uses `fixture_wrap_tile_descriptor` (mask=1 wrap, not
    /// `fixture_tile_descriptor`'s clamp) over the SAME committed 2x2 RGBA16
    /// texel layout -- required because the clamp fixture cannot expose this
    /// bug at all (see that function's own doc). Vertex UVs: `uv0=(-1,0)`,
    /// `uv1=(-1,0)` (S constant across X, so the X-gradient term vanishes),
    /// `uv2=(7,0)` (S varies across Y only). At pixel (0,0)'s center
    /// (`wx=wy=1/16`): `s = -1 + (1/16)*(-1-(-1)) + (1/16)*(7-(-1)) = -1 +
    /// 0 + 0.5 = -0.5` exactly (`0.0625*8 = 0.5` has an exact `f32`
    /// representation, no rounding drift); `t = 0` exactly (T is literally
    /// constant `0` across all three vertices -- this test isolates the
    /// negative-S repair alone; T's own varying-UV requirement is already
    /// covered by `required_host_textured_triangle_wgsl_sampling_matches_
    /// the_cpu_tmem_oracle`, unmodified by this test). `floor(-0.5) = -1`
    /// (correct) vs. `i32(-0.5) = 0` truncated toward zero (the pre-repair
    /// bug) -- see the CPU-side assertion's doc for the full corner/
    /// fraction arithmetic this produces under wrap addressing, and why the
    /// two paths' final blended colors provably differ, not just their
    /// intermediate fractions.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn required_host_negative_uv_floors_toward_negative_infinity_not_truncation() {
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

        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, FIXTURE_SOURCE_IMAGE_WIDTH, 0x200));
        words.extend(set_tile(
            0,
            FIXTURE_LINE_WORDS as u32,
            FIXTURE_TMEM_WORD_ADDRESS as u32,
        ));
        words.extend([word(SET_TILE_SIZE_OPCODE, 0), 4u32 << 12 | 4u32]);
        words.extend(load_sync());
        let source_bytes = fixture_load_block_source_bytes();
        words.extend([word(LOAD_BLOCK, 0), 7u32 << 12]);

        let (planned, _unused_deterministic_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture's TMEM-only load stays inside the admitted subset");
        // Unlike a bare `execute_raw_dpc` discard, this test needs the
        // TMEM write to be durably visible: `physical_tmem()` only reflects
        // the coordinator's ACTIVE slot after publish (see
        // `required_cpu_tmem_oracle_matches_hand_derived_texel_colors`'s own
        // doc) -- skipping commit/seal/publish here left stale/invalid TMEM
        // active, which is what real-Metal execution caught
        // (`InvalidTexelByte` at the fixture's own addressed footprint).
        let committed = session.commit_zero_guest_writes(prepared).unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        let outcome = backend.publish_raw_dpc(capsule);
        assert_eq!(outcome.submission(), submission);

        let tmem = project_committed_tmem(backend.physical_tmem());
        // Wrap tile (mask=1), not the clamp fixture -- see this test's own
        // doc for why clamp addressing cannot expose the floor-vs-truncation
        // bug.
        let tile_binding =
            TileBindingParams::bound(fixture_wrap_tile_descriptor(), fixture_tile_size());

        let uv0 = (-1.0f32, 0.0f32);
        let uv1 = (-1.0f32, 0.0f32);
        let uv2 = (7.0f32, 0.0f32);
        // Pixel (0,0)'s center (px=py=0.5, wx=wy=0.0625): s = -1 +
        // 0.0625*(-1-(-1)) + 0.0625*(7-(-1)) = -0.5, t = 0.
        let (expected_s, expected_t): (i16, i16) = (-1, 0);
        let expected = cpu_oracle_sample_with_tile(
            backend.physical_tmem(),
            fixture_wrap_tile_descriptor(),
            fixture_tile_size(),
            expected_s,
            expected_t,
        );
        // Cross-check against the CPU-only test's own independently
        // hand-derived literal, not just self-consistency with
        // `cpu_oracle_sample_with_tile`.
        assert_eq!(expected, [247, 8, 0, 255]);

        let vertices = [
            fn64_render::NeutralTriangleVertex {
                x: 0.0,
                y: 0.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv0.0, uv0.1],
            },
            fn64_render::NeutralTriangleVertex {
                x: 8.0,
                y: 0.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv1.0, uv1.1],
            },
            fn64_render::NeutralTriangleVertex {
                x: 0.0,
                y: 8.0,
                z: 0.5,
                w: 1.0,
                color: [0.0, 0.0, 0.0, 1.0],
                texcoord: [uv2.0, uv2.1],
            },
        ];
        // Same TEXEL0-passthrough SetCombine as the other GPU differential
        // test.
        let color_a: u32 = 0;
        let color_b: u32 = 0;
        let color_c: u32 = 0;
        let color_d: u32 = 1; // TEXEL0
        let alpha_a: u32 = 0;
        let alpha_b: u32 = 0;
        let alpha_c: u32 = 1;
        let alpha_d: u32 = 1; // TEXEL0
        let low = (color_a << 5) | color_c;
        let high = (color_b << 24)
            | (color_d << 6)
            | (alpha_a << 21)
            | (alpha_b << 3)
            | (alpha_c << 18)
            | alpha_d;
        let combine_params = CombineParams::from_wire(low, high);

        let renderer = backend.triangle_pipeline_for_test();
        let raster_params = TriangleRasterParams {
            resolution: [8.0, 8.0],
            screen_scale: [1.0, 1.0],
            screen_offset: [0.0, 0.0],
        };
        let output = renderer
            .submit_admitted_triangle(
                vertices,
                OtherMode::from_wire(0, 0),
                combine_params,
                raster_params,
                TriangleTargetExtent {
                    width: 8,
                    height: 8,
                },
                tmem,
                tile_binding,
                None,
                None,
                None,
                ResolvedFragmentBlendParams::NO_OP,
                false,
            )
            .expect("textured triangle draw must submit cleanly")
            .complete()
            .expect("textured triangle draw must complete cleanly");

        assert!(
            output
                .tmem_sample_status
                .iter()
                .all(|&status| status == TMEM_SAMPLE_STATUS_OK),
            "every fragment's tmem_sample.wgsl status must be OK for this fixture"
        );

        let observed = [
            output.color_rgba8[0],
            output.color_rgba8[1],
            output.color_rgba8[2],
            output.color_rgba8[3],
        ];
        // Exact agreement required, not the ±2 tolerance the other GPU
        // differential test uses: the pre-repair truncation bug's wrong
        // answer at this exact point (`[255, 0, 0, 255]`, pure red -- see
        // the CPU-side assertion's doc) differs from the correct floored
        // answer (`[247, 8, 0, 255]`) by up to 8 in the green channel, well
        // outside a ±2 tolerance, but this assertion holds GPU float
        // interpolation to the CPU oracle's own exact integer result with
        // zero slack -- this specific point was chosen so the interpolated
        // `f32` UV (`-0.5`) has an exact binary representation (no rounding
        // drift into the fixed-point filter math), so exact agreement is
        // the correct bar here, not a concession to interpolation noise.
        assert_eq!(observed, expected);
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
            source: TriangleSource::RawTriangle,
            viewport: None,
            other_mode: OtherMode::from_wire(0, 0),
            combine_params: CombineParams::from_wire(0, 0),
            tile_binding: TileBindingParams::unbound(),
            blend_color: None,
            env_color: None,
            prim_color: None,
            fog_color: None,
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

    /// One flat-shaded, non-textured, non-Z `RawTriangle` covering exactly
    /// the left half (`x` in `[0, width/2)`) of an 8x8 target, built from
    /// literal `NeutralTriangleVertex` positions (raw RDP screen-pixel
    /// space, matching `shaders/triangle_pipeline_vertex.wgsl`'s own
    /// module doc) rather than a wire-decoded fixture -- this test only
    /// needs two draws with disjoint, independently-checkable pixel
    /// coverage, not a real command-stream decode.
    fn half_covering_triangle(left: f32, right: f32, shade: f32) -> RetrievedTriangleDraw {
        RetrievedTriangleDraw {
            vertices: [
                fn64_render::NeutralTriangleVertex {
                    x: left,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                    color: [shade, shade, shade, 1.0],
                    texcoord: [0.0, 0.0],
                },
                fn64_render::NeutralTriangleVertex {
                    x: right,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                    color: [shade, shade, shade, 1.0],
                    texcoord: [0.0, 0.0],
                },
                fn64_render::NeutralTriangleVertex {
                    x: (left + right) / 2.0,
                    y: 8.0,
                    z: 0.0,
                    w: 1.0,
                    color: [shade, shade, shade, 1.0],
                    texcoord: [0.0, 0.0],
                },
            ],
            source: TriangleSource::RawTriangle,
            viewport: None,
            other_mode: OtherMode::from_wire(0, 0),
            // SHADE passthrough: `run_one_cycle` always evaluates the
            // second-cycle bit positions (`color_combiner.wgsl`'s
            // `run_one_cycle` hardcodes `second_cycle = true`), so this
            // uses the same second-cycle color_d/alpha_d=SHADE encoding as
            // `targets::triangle_pipeline::tests::shade_passthrough_combine_params`
            // -- color_a=color_b=0 (COMBINED) makes `(A-B)*C` zero, so
            // `(A-B)*C+D` collapses to D (SHADE), and this triangle's own
            // per-vertex `color` is what reaches the fragment, not the
            // all-zero default `CombineParams::from_wire(0, 0)` would
            // otherwise produce (transparent black everywhere,
            // indistinguishable from an uncovered/cleared pixel).
            combine_params: CombineParams::from_wire(0, (4 << 6) | 4),
            tile_binding: TileBindingParams::unbound(),
            blend_color: None,
            env_color: None,
            prim_color: None,
            fog_color: None,
        }
    }

    /// Hostile regression for the clear-per-draw batching defect this card
    /// fixes: two ordinary `RawTriangle`s with disjoint pixel coverage
    /// (left half / right half of an 8x8 target), submitted together in
    /// one `draw_admitted_triangles` call, must BOTH be visible in the
    /// single resulting `last_triangle_draw()` output. Before this fix,
    /// `draw_admitted_triangles` called `submit_admitted_triangle` once per
    /// triangle, and each call's own `submit_triangles(&[fixture])`
    /// re-cleared the shared target -- so only the second (last) triangle
    /// would ever survive, and the first triangle's left half would read
    /// back as the Clear color even though it drew without error. This is
    /// the same underlying defect a `TextureRectangle`'s two-triangle
    /// admission exposes (see
    /// `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position`),
    /// isolated here for a plain two-`RawTriangle` sequence with no rect
    /// involvement at all -- proving the fix is general to
    /// `draw_admitted_triangles`'s batching, not specific to `is_rect`.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn two_ordinary_triangles_in_one_call_both_survive_into_one_output() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("create() must succeed on a real adapter");

        let left_triangle = half_covering_triangle(0.0, 4.0, 1.0);
        let right_triangle = half_covering_triangle(4.0, 8.0, 1.0);
        backend
            .draw_admitted_triangles(vec![Ok(left_triangle), Ok(right_triangle)])
            .expect("two well-formed triangles in one call must draw successfully");

        let output = backend
            .last_triangle_draw()
            .expect("a successful draw_admitted_triangles call must populate last_triangle_draw");
        let width = output.extent.width;
        let pixel = |x: u32, y: u32| {
            let index = (y * width + x) as usize * 4;
            [
                output.color_rgba8[index],
                output.color_rgba8[index + 1],
                output.color_rgba8[index + 2],
                output.color_rgba8[index + 3],
            ]
        };
        assert_ne!(
            pixel(1, 4),
            [0, 0, 0, 0],
            "the left triangle's own half must be covered -- if the second draw re-cleared \
             the target, this pixel would still read back as the Clear color"
        );
        assert_ne!(
            pixel(6, 4),
            [0, 0, 0, 0],
            "the right (later) triangle's own half must also be covered"
        );
    }

    /// Same hostile shape as
    /// `a_failed_triangle_draw_leaves_the_prior_successful_output_untouched`,
    /// but with a real two-triangle batch preceding the invalid draw:
    /// proves batch submission failure atomicity holds even once
    /// `draw_admitted_triangles` collects multiple fixtures before
    /// submitting -- an invalid THIRD draw appended to an otherwise-valid
    /// two-triangle batch must fail the whole call and leave the prior
    /// output completely untouched, not partially apply the first two
    /// triangles.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn an_invalid_draw_after_two_valid_triangles_preserves_the_prior_output() {
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
            source: TriangleSource::RawTriangle,
            viewport: None,
            other_mode: OtherMode::from_wire(0, 0),
            combine_params: CombineParams::from_wire(0, 0),
            tile_binding: TileBindingParams::unbound(),
            blend_color: None,
            env_color: None,
            prim_color: None,
            fog_color: None,
        };
        backend
            .draw_admitted_triangles(vec![Ok(good_triangle)])
            .expect("a single valid triangle must draw successfully");
        let prior_color = backend
            .last_triangle_draw()
            .expect("the first successful draw must populate last_triangle_draw")
            .color_rgba8
            .clone();

        let batch_with_trailing_failure = vec![
            Ok(half_covering_triangle(0.0, 4.0, 1.0)),
            Ok(half_covering_triangle(4.0, 8.0, 1.0)),
            Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 2 }),
        ];
        let result = backend.draw_admitted_triangles(batch_with_trailing_failure);
        assert!(
            result.is_err(),
            "a batch whose last entry is a MissingTriangleDrawState must fail as a whole, even \
             though the two preceding entries were individually valid"
        );

        let output_after_failure = backend
            .last_triangle_draw()
            .expect("the prior successful output must still be present after a later failure");
        assert_eq!(
            output_after_failure.color_rgba8, prior_color,
            "a batch that fails during mapping must never submit any of its fixtures, leaving \
             last_triangle_draw() byte-identical to the value before the failed call"
        );
    }

    /// Positive: `raw_dpc_ir_capability` reports the real TMEM-plus-fill-
    /// plus-FullSync-site capability, not the trait's `Unsupported` default
    /// and not either older value -- a caller must be able to tell this
    /// backend apart from a non-raw-DPC-capable one, from one that admits no
    /// guest-visible write, and from one that rejects every FullSync,
    /// without attempting a submission.
    #[test]
    fn raw_dpc_ir_capability_reports_transactional_tmem_fill_full_sync_site_only() {
        let (backend, _session) = WgpuBackend::try_new().unwrap();
        assert_eq!(
            backend.raw_dpc_ir_capability(),
            RawDpcIrCapability::TransactionalTmemFillFullSyncSiteOnly
        );
        assert_ne!(
            backend.raw_dpc_ir_capability(),
            RawDpcIrCapability::TransactionalTmemNoFullSync,
            "the older TMEM-only value would tell a caller this backend declares zero \
             guest-visible writes, which is no longer true"
        );
        assert_ne!(
            backend.raw_dpc_ir_capability(),
            RawDpcIrCapability::TransactionalTmemFillNoFullSync,
            "the fill-only value would tell a caller this backend rejects every FullSync, \
             which is no longer true"
        );
    }

    /// Hostile: a FullSync whose capture carries no boundary record -- a
    /// producer that never took the reserve half -- must be surfaced as a
    /// loud `RenderError`, never a silently truncated plan and never an
    /// admitted site.
    #[test]
    fn plan_raw_dpc_rejects_an_unreserved_full_sync_command_loudly() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = one_load_block_words();
        words.extend([word(FULL_SYNC, 0), 0]);

        // `capture` builds through `OwnedRawDpcCapture::new`, so its boundary
        // list is empty.
        let request = session.plan_request(capture(words));
        let result = backend.plan_raw_dpc(request);
        assert!(
            result.is_err(),
            "an unreserved FullSync must be rejected, not silently admitted into the plan"
        );
    }

    /// Positive: the same stream, with the boundary record a reserving
    /// producer supplies, plans cleanly -- FullSync is no longer blanket-
    /// rejected at the production seam.
    ///
    /// The boundary supplied here is exactly what
    /// `try_dispatch_raw_dpc_via_session` supplies in production: both
    /// interrupt states `Clear`, because reserving the DP completion slot
    /// observes no interrupt. This test therefore also pins the nonclaim --
    /// admission does not require, and does not produce, an `Asserted`
    /// value.
    #[test]
    fn plan_raw_dpc_admits_a_reserved_full_sync_site() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = one_load_block_words();
        words.extend([word(FULL_SYNC, 0), 0]);

        let request = session.plan_request(full_sync_capture(words));
        let planned = backend.plan_raw_dpc(request);
        assert!(
            planned.is_ok(),
            "a FullSync site whose capture carries its boundary must plan cleanly: {:?}",
            planned.err()
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
    /// a raw-DPC capability with no source-kind carve-out.
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
    /// `fn64-render-wgpu` side: `publish_raw_dpc`'s body reaches
    /// `Published` through exactly that one unaltered expression -- no
    /// fabric-only path exists that could reach `Published` without also
    /// flipping this backend's own physical slot.
    ///
    /// The body is no longer a single statement: the deferred color-target
    /// publication takes its submission-keyed token before the commit and
    /// redeems it after. That addition is deliberately held to the same
    /// invariant, and this test now proves both halves of it -- the
    /// terminal expression is still character-for-character intact, and
    /// the token `take` that precedes it does not touch the capsule, the
    /// coordinator, or the fabric.
    #[test]
    fn publish_raw_dpc_source_is_exactly_prepare_publication_then_commit() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("fn publish_raw_dpc(")
            .expect("publish_raw_dpc must exist in this file");
        let body_end = source[body_start..]
            .find("\n    }\n")
            .expect("publish_raw_dpc must have a closing brace")
            + body_start;
        let body = &source[body_start..body_end];

        assert!(
            body.contains("self.coordinator.prepare_publication(publication).commit()"),
            "publish_raw_dpc must still reach Published through exactly \
             `self.coordinator.prepare_publication(publication).commit()` -- \
             one non-Result, callback-free terminal path"
        );
        assert_eq!(
            body.matches("prepare_publication(publication)").count(),
            1,
            "publish_raw_dpc must call the coordinator's prepare_publication exactly once"
        );
        assert_eq!(
            body.matches(".commit()").count(),
            1,
            "publish_raw_dpc must reach exactly one terminal commit"
        );

        // Nothing between obtaining the capsule and committing it may read
        // or alter the capsule, the coordinator, or the fabric. The only
        // statements permitted before the commit are the submission-keyed
        // token take and the capsule's own submission read -- neither of
        // which can change what is published.
        let before_commit = &body[..body
            .find("self.coordinator.prepare_publication(publication).commit()")
            .expect("checked above")];
        let executable_before: Vec<&str> = before_commit
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("//"))
            .filter(|line| {
                !line.starts_with("fn publish_raw_dpc(")
                    && !line.starts_with("&mut self,")
                    && !line.starts_with("publication:")
                    && !line.starts_with(") ->")
            })
            .collect();
        assert_eq!(
            executable_before,
            vec![
                "let pending = self.pending_fill_publication.take();",
                "let submission = publication.submission();",
                "let outcome =",
            ],
            "no step other than the submission-keyed fill token take and the capsule's own \
             submission read may run before publish_raw_dpc's terminal commit"
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
            source: TriangleSource::RawTriangle,
            viewport: None,
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

    fn fixture_set_env_color(value: u32) -> RdpStateCommand {
        RdpStateCommand::SetEnvColor {
            location: fixture_location(0),
            raw_words: Box::new([0]),
            color: fn64_render::NeutralColor4 { value },
            before: None,
            after: fn64_render::RdpStateIdentity::of_env_color(fn64_render::NeutralColor4 {
                value,
            }),
        }
    }

    fn fixture_set_prim_color(lod_frac: u8, lod_min: u8, color: u32) -> RdpStateCommand {
        let neutral = fn64_render::NeutralPrimColor {
            lod_frac,
            lod_min,
            color,
        };
        RdpStateCommand::SetPrimColor {
            location: fixture_location(0),
            raw_words: Box::new([0, 0]),
            color: neutral,
            before: None,
            after: fn64_render::RdpStateIdentity::of_prim_color(neutral),
        }
    }

    fn fixture_set_fog_color(value: u32) -> RdpStateCommand {
        RdpStateCommand::SetFogColor {
            location: fixture_location(0),
            raw_words: Box::new([0]),
            color: fn64_render::NeutralColor4 { value },
            before: None,
            after: fn64_render::RdpStateIdentity::of_fog_color(fn64_render::NeutralColor4 {
                value,
            }),
        }
    }

    /// Production blend wiring slice 1: `SetFogColor(A)` -> triangle A ->
    /// `SetFogColor(B)` -> triangle B must collect two distinct snapshots
    /// through `PlanCollector`, mirroring `plan_collector_snapshots_
    /// distinct_env_and_prim_colors_through_a_and_b_triangles` below exactly
    /// for the new `current_fog_color` field.
    #[test]
    fn plan_collector_snapshots_distinct_fog_colors_through_a_and_b_triangles() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let seed_combine = CombineParams::from_wire(0, 0);
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(seed_combine),
            None,
            None,
            None,
            None,
        );

        let fog_a = fixture_set_fog_color(0x7777_7777);
        collector.command(RawDpcSemanticCommandRef::State(&fog_a));
        let triangle_a = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));

        let fog_b = fixture_set_fog_color(0x8888_8888);
        collector.command(RawDpcSemanticCommandRef::State(&fog_b));
        let triangle_b = fixture_triangle(10.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

        assert_eq!(collector.triangles.len(), 2);
        let first = collector.triangles[0].as_ref().unwrap();
        let second = collector.triangles[1].as_ref().unwrap();
        assert_eq!(first.fog_color, Some(Color4::from_wire(0x7777_7777)));
        assert_eq!(second.fog_color, Some(Color4::from_wire(0x8888_8888)));
        assert_ne!(
            first.fog_color, second.fog_color,
            "triangle A must NOT be retroactively affected by a SetFogColor after it in plan \
             order"
        );
    }

    /// Framebuffer-blend admission split (Slice B): a triangle whose
    /// resolved blend cycle selects `BlendBInput::FramebufferAlpha` on an
    /// active cycle must still be rejected before GPU submission with a
    /// named `BlendRequiresFramebuffer` error -- the coverage-count half of
    /// the framebuffer-memory dependency, which this crate still does not
    /// implement. A plain `BlendColorInput::Framebuffer` selector on `P`/`M`
    /// alone (the destination-*color* half) is no longer this test's
    /// fixture, since that subset is now admitted and rendered -- see
    /// `draw_admitted_triangles_admits_a_framebuffer_color_only_blend_cycle`
    /// below for that coverage. Mirrors `a_failed_triangle_draw_leaves_the_
    /// prior_successful_output_untouched`'s own `create_inner`/
    /// `RetrievedTriangleDraw` literal fixture pattern exactly.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn draw_admitted_triangles_rejects_a_blend_cycle_that_reads_the_framebuffer() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("create() must succeed on a real adapter");

        // OneCycle (high bits 20:21 == 0, the default) with cycle 1's `B`
        // selector (`blender_cycle_1().alpha_b`, low bits 18:19) = 1
        // (FramebufferAlpha, `BlendBInput::from_wire`): the coverage-count
        // sub-case this crate still does not implement.
        let framebuffer_alpha_blend_other_mode = OtherMode::from_wire(0, 1 << 18);
        let triangle = RetrievedTriangleDraw {
            vertices: [
                fixture_vertex(0.0),
                fixture_vertex(1.0),
                fixture_vertex(2.0),
            ],
            source: TriangleSource::RawTriangle,
            viewport: None,
            other_mode: framebuffer_alpha_blend_other_mode,
            combine_params: CombineParams::from_wire(0, 0),
            tile_binding: TileBindingParams::unbound(),
            blend_color: None,
            env_color: None,
            prim_color: None,
            fog_color: None,
        };
        let error = backend
            .draw_admitted_triangles(vec![Ok(triangle)])
            .expect_err(
                "a framebuffer-alpha-dependent blend cycle must be rejected before submission",
            );
        assert!(
            matches!(
                error,
                WgpuRawDpcExecutionError::BlendRequiresFramebuffer { triangle_index: 0 }
            ),
            "unexpected error variant: {error:?}"
        );
    }

    /// `ResolvedBlendCycle::requires_framebuffer_alpha` table: `true` exactly
    /// when `alpha_b` (`B` selector) decodes to `FramebufferAlpha` (wire
    /// value `1`), independent of `color_a`/`color_b` (`P`/`M`) -- composed
    /// directly against `BlendBInput::from_wire`'s own decode, no new
    /// arithmetic oracle.
    #[test]
    fn requires_framebuffer_alpha_matches_only_the_b_selector() {
        for color_a in 0u8..4 {
            for color_b in 0u8..4 {
                for alpha_b in 0u8..4 {
                    let cycle = ResolvedBlendCycle::from_wire(BlenderCycle {
                        color_a,
                        alpha_a: 0,
                        color_b,
                        alpha_b,
                    });
                    let expected = BlendBInput::from_wire(alpha_b) == BlendBInput::FramebufferAlpha;
                    assert_eq!(
                        cycle.requires_framebuffer_alpha(),
                        expected,
                        "color_a={color_a} color_b={color_b} alpha_b={alpha_b}"
                    );
                }
            }
        }
    }

    /// Admission split (Slice B): a triangle whose resolved blend cycle
    /// selects `BlendColorInput::Framebuffer` on `P` (color only, no
    /// `FramebufferAlpha`) is now admitted -- not rejected -- and the
    /// resulting fixture's `blend_params.reads_framebuffer_color` is `true`.
    #[cfg(feature = "host-gpu-tests")]
    #[test]
    fn draw_admitted_triangles_admits_a_framebuffer_color_only_blend_cycle() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        backend
            .create_inner(&test_render_config())
            .expect("create() must succeed on a real adapter");

        // OneCycle (high bits 20:21 == 0, the default) with cycle 1's `P`
        // selector (`blender_cycle_1().color_a`, low bits 30:31) = 1
        // (Framebuffer, `BlendColorInput::from_wire`), `B` selector left at
        // its default (`0` = `OneMinusA`, not `FramebufferAlpha`) -- the
        // destination-color-only subset this card admits.
        let framebuffer_color_only_other_mode = OtherMode::from_wire(0, 1 << 30);
        let triangle = RetrievedTriangleDraw {
            vertices: [
                fixture_vertex(0.0),
                fixture_vertex(1.0),
                fixture_vertex(2.0),
            ],
            source: TriangleSource::RawTriangle,
            viewport: None,
            other_mode: framebuffer_color_only_other_mode,
            combine_params: CombineParams::from_wire(0, 0),
            tile_binding: TileBindingParams::unbound(),
            blend_color: None,
            env_color: None,
            prim_color: None,
            fog_color: None,
        };
        backend
            .draw_admitted_triangles(vec![Ok(triangle)])
            .expect("a color-only framebuffer blend cycle must be admitted, not rejected");
    }

    /// Command-time capture seam (card): `SetEnvColor(A)`/`SetPrimColor(A)`
    /// -> triangle A -> `SetEnvColor(B)`/`SetPrimColor(B)` -> triangle B
    /// must collect two distinct snapshots through `PlanCollector`,
    /// mirroring `plan_collector_snapshots_each_triangle_at_its_own_
    /// stream_position_not_the_final_value` above and
    /// `triangle_draw_data.rs`'s identical `TriangleDrawStateCollector`
    /// characterization test for the same new fields.
    #[test]
    fn plan_collector_snapshots_distinct_env_and_prim_colors_through_a_and_b_triangles() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let seed_combine = CombineParams::from_wire(0, 0);
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(seed_combine),
            None,
            None,
            None,
            None,
        );

        let env_a = fixture_set_env_color(0x1111_1111);
        let prim_a = fixture_set_prim_color(10, 5, 0x2222_2222);
        collector.command(RawDpcSemanticCommandRef::State(&env_a));
        collector.command(RawDpcSemanticCommandRef::State(&prim_a));
        let triangle_a = fixture_triangle(0.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_a));

        let env_b = fixture_set_env_color(0x3333_3333);
        let prim_b = fixture_set_prim_color(20, 10, 0x4444_4444);
        collector.command(RawDpcSemanticCommandRef::State(&env_b));
        collector.command(RawDpcSemanticCommandRef::State(&prim_b));
        let triangle_b = fixture_triangle(10.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle_b));

        assert_eq!(collector.triangles.len(), 2);
        let first = collector.triangles[0].as_ref().unwrap();
        let second = collector.triangles[1].as_ref().unwrap();
        assert_eq!(first.env_color, Some(Color4::from_wire(0x1111_1111)));
        assert_eq!(
            first.prim_color,
            Some(PrimColor::from_wire(10 | (5 << 8), 0x2222_2222))
        );
        assert_eq!(second.env_color, Some(Color4::from_wire(0x3333_3333)));
        assert_eq!(
            second.prim_color,
            Some(PrimColor::from_wire(20 | (10 << 8), 0x4444_4444))
        );
        assert_ne!(
            first.env_color, second.env_color,
            "triangle A must NOT be retroactively affected by a SetEnvColor after it in plan \
             order"
        );
        assert_ne!(
            first.prim_color, second.prim_color,
            "triangle A must NOT be retroactively affected by a SetPrimColor after it in plan \
             order"
        );
    }

    /// Durable cross-submission seed behavior for `env_color`/`prim_color`:
    /// a triangle with no in-plan `SetEnvColor`/`SetPrimColor` of its own
    /// still resolves those fields from `PlanCollector::seeded`'s durable
    /// value, exactly mirroring `plan_collector_seeded_resolves_a_triangle_
    /// with_no_in_plan_state_of_its_own` above for `other_mode`/`combine`.
    #[test]
    fn plan_collector_seeded_env_and_prim_color_resolve_a_triangle_with_no_in_plan_state() {
        let seed_other_mode = OtherMode::from_wire(0, 0);
        let seed_combine = CombineParams::from_wire(0, 0);
        let seed_env_color = Color4::from_wire(0x5555_5555);
        let seed_prim_color = PrimColor::from_wire(15 | (7 << 8), 0x6666_6666);
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(seed_combine),
            None,
            Some(seed_env_color),
            Some(seed_prim_color),
            None,
        );
        let triangle = fixture_triangle(1.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        assert_eq!(collector.triangles.len(), 1);
        let retrieved = collector.triangles[0]
            .as_ref()
            .expect("a triangle with durably-seeded state must resolve, not reject");
        assert_eq!(retrieved.env_color, Some(seed_env_color));
        assert_eq!(retrieved.prim_color, Some(seed_prim_color));
    }

    /// A triangle visited with no `SetOtherMode`/`SetCombine` anywhere --
    /// neither seeded nor in-plan -- must be a loud, named rejection, not
    /// a silent default. Proves `PlanCollector::seeded(None, None)`
    /// (unseeded) genuinely leaves `current_other_mode`/`current_combine`
    /// at `None` rather than defaulting them.
    #[test]
    fn plan_collector_rejects_a_triangle_visited_with_no_state_established_at_all() {
        let mut collector = PlanCollector::seeded(None, None, None, None, None, None);
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
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(seed_combine),
            None,
            None,
            None,
            None,
        );
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
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(seed_combine),
            None,
            None,
            None,
            None,
        );

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
        let mut collector = PlanCollector::seeded(
            Some(seed_other_mode),
            Some(CombineParams::from_wire(0, 0)),
            None,
            None,
            None,
            None,
        );

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
            None,
            None,
            None,
            None,
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

    /// The color-image height every fill fixture in this module configures.
    const FILL_TARGET_HEIGHT: u32 = 8;

    /// The color-image width every fill fixture in this module stages.
    const FILL_TARGET_WIDTH: u32 = 16;

    /// The physical address every fill fixture's `SetColorImage` names.
    /// Chosen clear of `COMMAND_START` (0x1000) so the target's byte range
    /// never overlaps the command stream, and inside `LAYOUT_BYTES`
    /// (0x4000) so `plan_fill`'s installed-RDRAM check passes.
    const FILL_TARGET_ADDRESS: u32 = 0x2000;

    const SET_COLOR_IMAGE: u8 = 0x3f;
    const SET_FILL_COLOR: u8 = 0x37;
    const FILL_RECTANGLE: u8 = 0x36;

    /// Records the host-configured framebuffer extent without requiring a
    /// GPU adapter.
    ///
    /// `create_inner` stores `configured_target_extent` *before* it requests
    /// a device (see its own comment), precisely so an admitted
    /// `FillRectangle` -- a CPU-side executor with no adapter dependency --
    /// can execute on an adapterless host. A `NoAdapter` result is therefore
    /// expected and ignored here; any *other* create failure still panics,
    /// because that would mean the extent was not recorded for the reason
    /// this helper assumes.
    fn configure_fill_target_height(backend: &mut WgpuBackend) {
        match backend.create_inner(&fn64_render::RenderConfig {
            width: FILL_TARGET_WIDTH,
            height: FILL_TARGET_HEIGHT,
            tv_type: fn64_runtime::TvType::default(),
        }) {
            Ok(()) | Err(WgpuCreateError::NoAdapter(_)) => {}
            Err(other) => panic!("create_inner failed for an unexpected reason: {other}"),
        }
        assert!(
            backend.configured_target_extent.is_some(),
            "create_inner must record the host-configured extent even with no GPU adapter"
        );
    }

    /// `SetOtherMode` staging Fill cycle (`cycle_type == 3`) with no
    /// Z-compare/Z-update/image-read bit set -- the only `OtherMode`
    /// `execute_fill_rectangle` admits (`require_safe_fill_cycle_bypass`).
    fn fill_cycle_other_mode(low: u32) -> [u32; 2] {
        [word(SET_OTHER_MODE, 3 << 20), low]
    }

    /// `SetColorImage` staging an RGBA16 image of `FILL_TARGET_WIDTH` at
    /// `FILL_TARGET_ADDRESS`. Wire `format` is 0 (`Rgba`), wire `size` is 2
    /// (`Bits16`), and the wire `width` field is width-1 (the decoder adds
    /// one back). `FILL_TARGET_ADDRESS` is 64-byte aligned, which
    /// `SetColorImage`'s own decode requires.
    fn set_color_image_rgba16() -> [u32; 2] {
        [
            word(SET_COLOR_IMAGE, 2 << 19 | (FILL_TARGET_WIDTH - 1)),
            FILL_TARGET_ADDRESS,
        ]
    }

    fn set_fill_color(value: u32) -> [u32; 2] {
        [word(SET_FILL_COLOR, 0), value]
    }

    /// One `FillRectangle` at whole-pixel coordinates. The wire fields are
    /// 10.2 fixed point, so each coordinate is shifted left by 2.
    fn fill_rectangle(x0: u32, y0: u32, x1: u32, y1: u32) -> [u32; 2] {
        [
            word(FILL_RECTANGLE, ((x1 << 2) << 12) | (y1 << 2)),
            ((x0 << 2) << 12) | (y0 << 2),
        ]
    }

    /// The headline fixture: a partial-width, three-row fill.
    ///
    /// `x0 = 4` is deliberately nonzero, so `plan_fill` takes its per-row
    /// branch (`x0 == 0 && x1 + 1 == width` is false) and declares **three**
    /// disjoint, width-strided write accesses rather than one collapsed
    /// range. 11 pixels wide (x 4..=14) x 3 rows (y 2..=4) in an RGBA16
    /// image: 22 bytes per row, 66 bytes total, spanning 22 + 2*32 = 86
    /// bytes -- so a single collapsed range would falsely claim 20 untouched
    /// inter-row bytes as written.
    fn partial_width_fill_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        words.extend(fill_rectangle(4, 2, 14, 4));
        words
    }

    /// Same target and rectangle height, but spanning the image's full
    /// width -- so `plan_fill` takes its `planned_rows == 1` branch and
    /// declares exactly one contiguous access.
    fn full_width_fill_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        words.extend(fill_rectangle(0, 2, FILL_TARGET_WIDTH - 1, 4));
        words
    }

    /// A whole-target fill: every pixel of the 16x8 image.
    ///
    /// Required as the *first* fill against a fresh color target.
    /// `CandidateColorTarget::admit_completed_initialization` rejects a
    /// partial rectangle on a target with no predecessor
    /// (`PartialNewTargetInitialization`), because a brand-new target has no
    /// prior device-byte content for the untouched rows and admitting one
    /// would publish fabricated zeros as if they were real content. Filling
    /// the whole target first establishes generation 1 honestly; a
    /// subsequent partial fill then patches into that real buffer.
    ///
    /// This is also the real-world order: a title clears its framebuffer
    /// before filling sub-rectangles into it.
    fn whole_target_fill_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x0842_1085));
        words.extend(fill_rectangle(
            0,
            0,
            FILL_TARGET_WIDTH - 1,
            FILL_TARGET_HEIGHT - 1,
        ));
        words
    }

    /// Runs one fill capture all the way through plan -> execute -> commit
    /// -> seal -> publish, returning the staged writes it committed.
    fn publish_one_fill(
        backend: &mut WgpuBackend,
        session: &mut RawDpcAbiSession,
        words: Vec<u32>,
    ) -> Vec<CompletedWrite> {
        let request = session.plan_request(capture(words));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("fixture plans cleanly");
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture executes cleanly");
        let staged = backend.staged_guest_render_target_writes(submission);
        let committed = session
            .commit_guest_render_target_writes(prepared, staged.clone())
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        staged
    }

    /// The ordered `RenderTarget` write accesses one word stream's decode
    /// declares in its own resource journal.
    ///
    /// Read from `crate::decode_raw_dpc`'s resource plan -- the same list
    /// `plan_fill` pushed and the same list `ExactRawDpcPlanWriter::finish`
    /// proves the sealed plan equals one for one. `PlannedRawDpcSubmission`
    /// exposes no journal accessor of its own, so this re-decodes the same
    /// capture rather than reaching into a sealed value.
    fn declared_render_target_writes(words: Vec<u32>) -> Vec<(u32, u32)> {
        let capture = capture(words);
        let layout = capture.memory_layout();
        let submission = capture.submission().clone();
        let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
        let decoded = finalize_with_zero_reads(
            layout,
            capture.transaction_sequence(),
            submission.clone(),
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            probe_journal,
        )
        .unwrap();
        let ticket = submit_locally(decoded).unwrap();
        let accesses = match crate::decode_raw_dpc(ticket, &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            other => panic!("probe decode must report the real access list, got {other:?}"),
        };
        accesses
            .iter()
            .filter(|access| {
                access.mode() == AccessMode::Write
                    && access.purpose() == AccessPurpose::RenderTarget
            })
            .map(|access| match access.region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                    (range.start().get(), range.len())
                }
                other => panic!("a fill access is always an RDRAM region, got {other:?}"),
            })
            .collect()
    }

    /// Drives plan -> execute for a fill fixture, which declares zero
    /// `TmemLoadSource` reads.
    fn plan_and_execute_fill(
        backend: &mut WgpuBackend,
        session: &mut RawDpcAbiSession,
        words: Vec<u32>,
    ) -> (
        fn64_render_ir::SubmissionIdentity,
        Result<BackendPreparedRawDpc, RenderError>,
    ) {
        let planned = plan_with_no_reads(backend, session, words);
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        (submission, backend.execute_raw_dpc(bound))
    }

    /// **T-7 -- the headline admission test.** A partial-width, three-row
    /// fill plans, executes, and reports exactly three staged guest writes,
    /// with publication genuinely deferred.
    ///
    /// Every assertion here is one the pre-admission code could not have
    /// satisfied: `plan_raw_dpc` used to reject `FillRectangle` outright
    /// with `UnadmittedRawDpcCommand`, the journal used to declare zero
    /// `RenderTarget` writes, and no staged-write transport existed at all.
    ///
    /// The deferral assertion is the load-bearing one: it proves the
    /// deferred-token design actually defers. If `execute_raw_dpc` published
    /// eagerly, the guest commit that must precede publication would be
    /// running *after* the registry already advanced.
    ///
    /// The whole-target fill that runs first is not incidental setup: a
    /// partial rectangle cannot initialize a *fresh* target at all
    /// (`PartialNewTargetInitialization` -- the untouched rows would be
    /// fabricated zeros), so establishing a real generation 1 is the only
    /// honest way to reach the partial-fill path.
    #[test]
    fn execute_raw_dpc_admits_a_partial_width_fill_end_to_end() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        assert_eq!(
            declared_render_target_writes(partial_width_fill_words()).len(),
            3,
            "a partial-width 11x3 fill declares one RenderTarget write access PER ROW -- a \
             single collapsed range would claim untouched inter-row bytes as written"
        );

        // Establish a real resident generation the partial fill can patch.
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
        let generation_before = backend.color_targets().unwrap().residents()[0]
            .generation()
            .get();

        let request = session.plan_request(capture(partial_width_fill_words()));
        let planned = backend.plan_raw_dpc(request).expect(
            "an admitted partial-width fill must plan cleanly, not be rejected as an \
             UnadmittedRawDpcCommand",
        );

        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("an admitted fill must execute cleanly");

        let staged = backend.staged_guest_render_target_writes(submission);
        assert_eq!(
            staged.len(),
            3,
            "the backend must transport exactly the three CompletedWrites its journal declares"
        );
        // 11 pixels x 2 bytes per RGBA16 pixel = 22 bytes per row.
        for (row, write) in staged.iter().enumerate() {
            assert_eq!(
                write.byte_count(),
                22,
                "row {row}'s write must cover only its own 22 bytes"
            );
        }
        assert_eq!(
            staged.iter().map(|write| write.byte_count()).sum::<u32>(),
            66,
            "the three rows total 66 real bytes, never the 86 a collapsed range would span"
        );

        let registry = backend
            .color_targets()
            .expect("the first admitted fill builds the registry");
        assert_eq!(
            registry.residents()[0].generation().get(),
            generation_before,
            "publication must be deferred until publish_raw_dpc -- an advanced generation here \
             would mean the registry moved before the guest commit that must precede it"
        );
        assert!(
            backend.has_pending_fill_publication(),
            "the staged fill is held as a submission-keyed token, not published"
        );

        drop(prepared);
    }

    /// **T-6:** the full-width branch stays in lockstep with the per-row
    /// branch. Same target and same three rows, but `x0 == 0 && x1 + 1 ==
    /// width`, so `plan_fill` collapses to exactly one access -- which is
    /// legitimate here precisely because a full-width run IS contiguous.
    #[test]
    fn execute_raw_dpc_collapses_a_full_width_fill_to_one_write() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        assert_eq!(
            declared_render_target_writes(full_width_fill_words()).len(),
            1,
            "a full-width fill's rows ARE contiguous, so one access is the honest declaration"
        );

        // Full-width but only three rows tall, so it is still a partial
        // rectangle for admission purposes -- establish a real generation
        // first, as `PartialNewTargetInitialization` requires.
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let request = session.plan_request(capture(full_width_fill_words()));
        let planned = backend.plan_raw_dpc(request).unwrap();

        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend.execute_raw_dpc(bound).unwrap();

        let staged = backend.staged_guest_render_target_writes(submission);
        assert_eq!(staged.len(), 1);
        assert_eq!(
            staged[0].byte_count(),
            3 * FILL_TARGET_WIDTH * 2,
            "one access covering three full 16-pixel RGBA16 rows is 96 bytes"
        );

        drop(prepared);
    }

    /// **T-5:** each row's content digest covers exactly its own 22 bytes,
    /// sliced from the full-extent device buffer -- never a digest over the
    /// whole 256-byte target, and never over the 86-byte span the three rows
    /// collectively occupy.
    ///
    /// Recomputed independently here from `effect_content_digest` over the
    /// resident's own published bytes, so a change that started hashing a
    /// wider slice would fail rather than merely producing a different
    /// opaque value.
    #[test]
    fn each_fill_row_write_hashes_only_its_own_bytes() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let row_ranges = declared_render_target_writes(partial_width_fill_words());
        assert_eq!(row_ranges.len(), 3);
        // The three rows are strided by the image's own row pitch (16
        // pixels x 2 bytes), not packed end to end -- which is exactly why
        // they cannot be collapsed.
        assert_eq!(row_ranges[1].0 - row_ranges[0].0, FILL_TARGET_WIDTH * 2);
        assert_eq!(row_ranges[2].0 - row_ranges[1].0, FILL_TARGET_WIDTH * 2);

        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
        // Publish the partial fill so the full-extent device bytes it
        // produced are readable, then verify every staged digest against a
        // slice recomputed from them.
        let staged = publish_one_fill(&mut backend, &mut session, partial_width_fill_words());
        assert_eq!(staged.len(), 3);

        let registry = backend.color_targets().unwrap();
        let resident = &registry.residents()[0];
        let buffer = resident.device_bytes().device_bytes();
        assert_eq!(
            buffer.len() as u32,
            FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
            "the resident's device bytes cover the whole target, unlike any single write"
        );

        let base = resident.key().address().get();
        for (row, (start, len)) in row_ranges.iter().enumerate() {
            let offset = (start - base) as usize;
            let slice = &buffer[offset..offset + *len as usize];
            assert_eq!(
                staged[row].content(),
                fn64_render_ir::effect_content_digest(slice),
                "row {row}'s digest must cover exactly its own {len} bytes"
            );
            assert_ne!(
                staged[row].content(),
                fn64_render_ir::effect_content_digest(buffer),
                "row {row}'s digest must NOT be a digest of the whole target buffer"
            );
        }
    }

    /// **T-10:** each full plan -> execute -> commit -> seal -> publish
    /// cycle advances the resident generation by exactly one -- proving
    /// publication is neither skipped nor doubled.
    ///
    /// Generation 1 is the whole-target fill (the only rectangle a fresh
    /// target admits); generations 2 and 3 are partial fills patching into
    /// it.
    #[test]
    fn publish_raw_dpc_advances_the_resident_generation_exactly_once() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
        assert_eq!(
            backend.color_targets().unwrap().residents()[0]
                .generation()
                .get(),
            1
        );

        for expected_generation in 2..=3u64 {
            let staged = publish_one_fill(&mut backend, &mut session, partial_width_fill_words());
            assert_eq!(staged.len(), 3);

            let registry = backend.color_targets().unwrap();
            assert_eq!(
                registry.residents().len(),
                1,
                "every fill targets the same color image, so there is exactly one resident"
            );
            assert_eq!(
                registry.residents()[0].generation().get(),
                expected_generation,
                "each published fill advances the resident generation by exactly one"
            );
            assert!(
                !backend.has_pending_fill_publication(),
                "the token must be consumed by publication, never left behind"
            );
        }
    }

    /// **T-9 -- the nonmutation test.** A fill rejected at *execution* time,
    /// after `begin_candidate` has already succeeded, must leave the
    /// registry byte-identical and leave no staged token behind.
    ///
    /// `Z_CMP` (`OtherMode.low & 0x0010`) is the deliberate lever: it passes
    /// every plan-time gate (`plan_fill` checks cycle type, not the
    /// Z/framebuffer hazard bits) and is rejected by
    /// `require_safe_fill_cycle_bypass` inside `execute_fill_rectangle` --
    /// i.e. precisely inside the window the deferred-token design creates,
    /// after a candidate exists and before anything is published.
    #[test]
    fn a_rejected_fill_leaves_the_registry_and_physical_slot_untouched() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // First, establish a real resident generation to be preserved.
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let snapshot_generation = backend.color_targets().unwrap().residents()[0]
            .generation()
            .get();
        let snapshot_bytes = backend.color_targets().unwrap().residents()[0]
            .device_bytes()
            .device_bytes()
            .to_vec();
        let snapshot_physical = backend.physical_tmem().identity();
        assert_eq!(snapshot_generation, 1);

        // Now a second capture whose fill is rejected at execution time.
        let mut hostile = Vec::new();
        hostile.extend(fill_cycle_other_mode(0x0010)); // Z_CMP set
        hostile.extend(set_color_image_rgba16());
        hostile.extend(set_fill_color(0x213c_4d59));
        hostile.extend(fill_rectangle(4, 2, 14, 4));

        let (_, result) = plan_and_execute_fill(&mut backend, &mut session, hostile);
        assert!(
            result.is_err(),
            "a Z_CMP fill-cycle bypass must be rejected loudly at execution, never executed"
        );

        let registry = backend.color_targets().unwrap();
        assert_eq!(registry.residents().len(), 1);
        assert_eq!(
            registry.residents()[0].generation().get(),
            snapshot_generation,
            "a rejected fill must not advance the resident generation"
        );
        assert_eq!(
            registry.residents()[0].device_bytes().device_bytes(),
            snapshot_bytes.as_slice(),
            "a rejected fill must leave the resident's device bytes byte-identical"
        );
        assert_eq!(
            backend.physical_tmem().identity(),
            snapshot_physical,
            "a fill never touches physical TMEM, rejected or not"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a rejected fill must leave no staged token for a later publish to redeem"
        );
    }

    /// **T-11:** dropping the sealed capsule instead of publishing leaves
    /// the registry at its prior generation -- the cancellation path, which
    /// the deferred token makes reachable for color targets too.
    #[test]
    fn dropping_the_capsule_before_publish_leaves_the_registry_untouched() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
        let generation_before = backend.color_targets().unwrap().residents()[0]
            .generation()
            .get();

        let request = session.plan_request(capture(partial_width_fill_words()));
        let planned = backend.plan_raw_dpc(request).unwrap();
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend.execute_raw_dpc(bound).unwrap();
        let staged = backend.staged_guest_render_target_writes(submission);
        let committed = session
            .commit_guest_render_target_writes(prepared, staged)
            .unwrap();

        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        assert!(
            backend.has_pending_fill_publication(),
            "the token is held between execute and publish -- that window is the design"
        );
        drop(capsule);

        assert_eq!(
            backend
                .color_targets()
                .expect("the registry was built during execution")
                .residents()[0]
                .generation()
                .get(),
            generation_before,
            "a dropped capsule publishes nothing, so the registry stays at its prior generation"
        );
    }

    /// **T-13:** the split-arm regression proof. `FullSync` must still be
    /// rejected loudly now that it no longer shares a match arm with
    /// `FillRectangle`.
    #[test]
    fn plan_raw_dpc_still_rejects_a_full_sync_command_after_the_arm_split() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = partial_width_fill_words();
        words.extend([word(FULL_SYNC, 0), 0]);

        let request = session.plan_request(capture(words));
        assert!(
            backend.plan_raw_dpc(request).is_err(),
            "admitting FillRectangle must not have admitted FullSync alongside it"
        );
    }

    /// **T-14:** admitting fills did not become "admit all fills". A
    /// `FillRectangle` under Copy cycle (`cycle_type == 2`) is still
    /// rejected at plan time.
    #[test]
    fn plan_raw_dpc_rejects_a_copy_cycle_fill_rectangle() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = Vec::new();
        words.extend([word(SET_OTHER_MODE, 2 << 20), 0]);
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        words.extend(fill_rectangle(4, 2, 14, 4));

        let request = session.plan_request(capture(words));
        assert!(
            backend.plan_raw_dpc(request).is_err(),
            "only fill-cycle FillRectangles are admitted"
        );
    }

    /// **T-15:** `plan_fill`'s fractional-edge gate must survive the
    /// admission change. A coordinate with nonzero low two bits is a
    /// quarter-pixel edge this slice does not execute.
    #[test]
    fn plan_raw_dpc_rejects_a_fractional_edge_fill_rectangle() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        // Same rectangle as the headline fixture, but with y1's
        // quarter-pixel fraction set.
        words.extend([
            word(FILL_RECTANGLE, ((14u32 << 2) << 12) | (4u32 << 2) | 1),
            ((4u32 << 2) << 12) | (2u32 << 2),
        ]);

        let request = session.plan_request(capture(words));
        assert!(
            backend.plan_raw_dpc(request).is_err(),
            "a fractional edge must be rejected, never truncated to whole pixels"
        );
    }

    /// A `FillRectangle` followed by a `RawTriangle` in one packet: the
    /// ordinary N64 idiom of clearing a framebuffer and then drawing into
    /// it. Both halves plan cleanly on their own, so nothing upstream
    /// refuses this; the refusal has to live at execution.
    ///
    /// `set_combine` is required before the triangle -- `PlanCollector`
    /// rejects a triangle visited with no combiner state established
    /// (see `plan_collector_rejects_a_triangle_visited_with_no_state_
    /// established_at_all`). `set_other_mode` is deliberately NOT re-issued
    /// after the fill: reverting to a non-Fill cycle would be a second,
    /// unrelated reason for the packet to be interesting, and the fill's
    /// own Fill-cycle `OtherMode` is what `plan_fill` admitted against.
    fn fill_then_triangle_words() -> Vec<u32> {
        let mut words = whole_target_fill_words();
        words.extend(set_combine(0, 0));
        words.extend(triangle_base_edge_words(7, 2, 0));
        words
    }

    /// Hostile, and the reason this refusal exists: a packet carrying both
    /// an admitted fill and an admitted triangle is rejected by name.
    ///
    /// Before this check, `stage_and_report` routed straight into
    /// `stage_fills_and_report` -- which never inspects `plan.triangles` --
    /// and `execute_raw_dpc` then drew those triangles into a color
    /// attachment `draw_admitted_triangles` clears itself, disjoint from
    /// the CPU-side fill buffer the same packet had just staged. The
    /// renderer reported success while one of the two render results went
    /// nowhere the guest could observe, with no ordering between them.
    ///
    /// The assertion is against the named variant's own `Display` text, not
    /// a substring: `RenderBackend::execute_raw_dpc` converts the typed
    /// `WgpuRawDpcExecutionError` into `RenderError::Backend`'s string, so
    /// comparing to `MixedFillAndTrianglePacket.to_string()` is how this
    /// module pins the *specific* refusal rather than merely "some error".
    ///
    /// The whole-target fill runs first for the same reason every other
    /// first-fill fixture here does: a partial rectangle cannot honestly
    /// initialize a fresh target.
    #[test]
    fn execute_raw_dpc_rejects_a_mixed_fill_and_triangle_packet() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let planned = plan_with_no_reads(&mut backend, &session, fill_then_triangle_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        let error = backend
            .execute_raw_dpc(bound)
            .expect_err("a fill+triangle packet must be refused, never half-executed");
        match error {
            RenderError::Backend { reason, .. } => assert_eq!(
                reason,
                WgpuRawDpcExecutionError::MixedFillAndTrianglePacket.to_string(),
                "the refusal must be the named MixedFillAndTrianglePacket variant, not some \
                 other error that happens to also reject this packet"
            ),
            other => panic!("expected a backend rejection, got {other:?}"),
        }
        assert!(
            !backend.has_pending_fill_publication(),
            "a refused fill+triangle packet must stage no redeemable fill token"
        );
    }

    /// The new refusal did not over-reject: a fill with no triangle beside
    /// it still executes and still stages its token. Without this, the
    /// check above could have been written as "any packet with a fill" and
    /// nothing in this module would have noticed.
    #[test]
    fn a_fill_only_packet_still_executes_after_the_triangle_refusal() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let (_, result) =
            plan_and_execute_fill(&mut backend, &mut session, whole_target_fill_words());
        result.expect("a fill-only packet must still execute -- the new check is fill+triangle");
        assert!(
            backend.has_pending_fill_publication(),
            "a fill-only packet must still stage its deferred publication token"
        );
    }

    /// The mirror of the test above, from the triangle side: a triangle
    /// with no fill beside it still reaches `draw_admitted_triangles`
    /// rather than being caught by the new check.
    ///
    /// The draw itself needs a real adapter, so on an adapterless host the
    /// packet's execution ends in `TriangleDrawBeforeCreate`. That is the
    /// *evidence* this test wants, not a limitation of it: reaching that
    /// error proves `stage_and_report` admitted the plan and
    /// `execute_raw_dpc` went on to attempt the draw. Being caught by
    /// `MixedFillAndTrianglePacket` instead would mean the new check fires
    /// on triangles alone. (The full real-GPU success path for a
    /// triangle-only plan is covered under `host-gpu-tests` by
    /// `triangle_only_plan_completes_via_preserving_physical_and_never_
    /// flips_the_slot`.)
    #[test]
    fn a_triangle_only_packet_still_reaches_the_draw_after_the_fill_refusal() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        let planned = plan_with_no_reads(&mut backend, &session, triangle_only_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        let refused = WgpuRawDpcExecutionError::MixedFillAndTrianglePacket.to_string();
        match backend.execute_raw_dpc(bound) {
            // A host that DOES have an adapter AND has had create() called
            // on it draws the triangle and succeeds; this test calls no
            // create(), so on every host today it takes the Err arm below.
            // The arm is kept rather than made `unreachable!()` because the
            // claim under test is "not caught by the fill+triangle refusal",
            // which a success satisfies just as well as the draw's own error.
            Ok(_) => {}
            Err(RenderError::Backend { reason, .. }) => {
                assert_ne!(
                    reason, refused,
                    "a triangle with no fill beside it must never hit the fill+triangle refusal"
                );
                assert_eq!(
                    reason,
                    WgpuRawDpcExecutionError::TriangleDrawBeforeCreate.to_string(),
                    "on an adapterless host the only expected outcome is the draw's own \
                     TriangleDrawBeforeCreate, reached by going PAST stage_and_report"
                );
            }
            Err(other) => panic!("expected either success or the draw's own error, got {other:?}"),
        }
    }

    /// Ordering: a submission whose triangle draw FAILS must leave no
    /// redeemable fill token behind.
    ///
    /// `execute_raw_dpc` used to store `pending_fill_publication` before
    /// calling `draw_admitted_triangles`, so a draw failure returned `Err`
    /// with the token already on the backend -- a later `publish_raw_dpc`
    /// could then redeem a fill from a submission that never completed.
    ///
    /// Inducing the failure needs a submission that carries BOTH a fill (to
    /// produce a token) and a triangle draw that fails -- and the new
    /// refusal above now rejects exactly that packet before either happens.
    /// So this drives the two halves of `execute_raw_dpc` directly, in its
    /// own order: `execute_raw_dpc_inner` on a fill-only packet yields a
    /// real token, then `draw_admitted_triangles` is called with a triangle
    /// whose plan state never resolved. Both halves are the production
    /// functions, not stand-ins; only their sequencing is reproduced here.
    ///
    /// The chosen failure is `MissingTriangleDrawState::NoCombine`, not the
    /// review's `TriangleDrawBeforeCreate`: this host has a real Metal
    /// adapter, so `configure_fill_target_height`'s `create_inner` succeeds
    /// and the pipeline IS present. `NoCombine` fails inside the same
    /// function on any host, adapter or not, and is the same
    /// `execute_raw_dpc` error path -- it is `draw_admitted_triangles`
    /// returning `Err` that this test is about, not which `Err`.
    #[test]
    fn a_failed_triangle_draw_leaves_no_redeemable_fill_token() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let planned = plan_with_no_reads(&mut backend, &session, whole_target_fill_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        let (_prepared, _triangles, pending) = execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend.rdp_state.other_mode(),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            &mut backend.color_targets,
            backend.configured_target_extent,
        )
        .expect("the fill half must stage a real token");
        assert!(
            pending.is_some(),
            "this fixture must actually produce a token, or the ordering claim is vacuous"
        );

        // The draw half, on the same backend the fill just staged against.
        let draw =
            backend.draw_admitted_triangles(vec![Err(MissingTriangleDrawState::NoCombine {
                triangle_index: 0,
            })]);
        assert!(
            matches!(
                draw,
                Err(WgpuRawDpcExecutionError::MissingTriangleDrawState(_))
            ),
            "expected the draw to fail, got {draw:?}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "the token must not be on the backend when the triangle draw fails -- the store \
             belongs AFTER the draw, not before it"
        );

        // The runtime half above proves the two production functions
        // compose correctly in this order, but it calls them itself -- it
        // cannot notice `execute_raw_dpc` reverting to the OLD order. So
        // the ordering `execute_raw_dpc` actually uses is pinned at the
        // source level too, the same way
        // `publish_raw_dpc_source_is_exactly_prepare_publication_then_commit`
        // pins its own body's shape.
        let source = include_str!("production.rs");
        let body_start = source
            .find("fn execute_raw_dpc(")
            .expect("execute_raw_dpc must exist in this file");
        let body_end = source[body_start..]
            .find("\n    }\n")
            .expect("execute_raw_dpc must have a closing brace")
            + body_start;
        let body = &source[body_start..body_end];

        let draw_at = body
            .find("self.draw_admitted_triangles(triangles)")
            .expect("execute_raw_dpc must still call draw_admitted_triangles");
        let store_at = body
            .find("self.pending_fill_publication = pending;")
            .expect("execute_raw_dpc must still store the pending token");
        assert!(
            draw_at < store_at,
            "execute_raw_dpc must call draw_admitted_triangles BEFORE storing \
             pending_fill_publication -- storing first leaves a redeemable token on the backend \
             when the draw fails and the call returns Err"
        );
        assert_eq!(
            body.matches("self.pending_fill_publication = pending;")
                .count(),
            1,
            "exactly one store site, or the ordering above says nothing about the other"
        );
    }

    /// Hostile: a mixed TMEM-load-plus-fill packet is rejected loudly at
    /// execution rather than silently reordering two write sources into one
    /// effect report. This slice does not implement the journal-order merge;
    /// the refusal is named, not implicit.
    #[test]
    fn execute_raw_dpc_rejects_a_mixed_tmem_and_fill_packet() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let mut words = one_load_block_words();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        words.extend(fill_rectangle(4, 2, 14, 4));

        let (planned, source_bytes) = plan_with_deterministic_reads(&mut backend, &session, words);
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        assert!(
            backend.execute_raw_dpc(bound).is_err(),
            "a mixed TMEM+fill packet must be rejected, never merged in an unverified order"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a rejected mixed packet must stage nothing"
        );
    }

    /// Hostile: a submission mismatch yields an EMPTY staged-write list, not
    /// another submission's writes. That empty list then drives the caller
    /// into the zero-write commit branch, which fails loudly against the
    /// packet's own nonempty guest-write journal -- a loud rejection rather
    /// than a quiet wrong publish.
    #[test]
    fn staged_guest_render_target_writes_returns_empty_for_a_foreign_submission() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let request = session.plan_request(capture(partial_width_fill_words()));
        let planned = backend.plan_raw_dpc(request).unwrap();
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend.execute_raw_dpc(bound).unwrap();

        assert_eq!(
            backend.staged_guest_render_target_writes(submission).len(),
            3
        );

        // A different submission's identity, taken from a second plan on the
        // same session.
        let other_request = session.plan_request(capture(full_width_fill_words()));
        let other_planned = backend.plan_raw_dpc(other_request).unwrap();
        let other_bound = session
            .finalize_and_submit(other_planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let other_submission = other_bound.submission();
        assert_ne!(other_submission, submission);
        assert!(
            backend
                .staged_guest_render_target_writes(other_submission)
                .is_empty(),
            "a submission this backend staged no write for must report an empty list, never \
             another submission's writes"
        );

        drop(prepared);
        drop(other_bound);
    }

    /// Regression: a TMEM-only submission still reports no staged guest
    /// writes at all, so the existing zero-write commit path is undisturbed.
    #[test]
    fn tmem_only_submissions_stage_no_guest_render_target_writes() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
        let guest_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend.execute_raw_dpc(bound).unwrap();

        assert!(
            backend
                .staged_guest_render_target_writes(submission)
                .is_empty(),
            "a TMEM-only submission stages no color-target write"
        );
        assert!(backend.color_targets().is_none());
        session.commit_zero_guest_writes(prepared).unwrap();
    }

    /// Hostile: an admitted fill reaching execution with no prior `create`
    /// call is rejected loudly. The RDP's `SetColorImage` carries no height
    /// field, so this backend has no honest way to size the color target --
    /// and inventing one would fabricate the target's identity and range.
    #[test]
    fn a_fill_before_any_create_is_rejected_rather_than_given_an_invented_height() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        // Deliberately NO configure_fill_target_height call.
        let (_, result) =
            plan_and_execute_fill(&mut backend, &mut session, partial_width_fill_words());
        assert!(
            result.is_err(),
            "with no host-configured height, an admitted fill must be rejected, not sized by \
             a fabricated default"
        );
        assert!(!backend.has_pending_fill_publication());
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

    /// A resize BEFORE any `create` records the host-configured extent and
    /// leaves `triangle_target_extent` `None`.
    ///
    /// The pairing invariant (§1a/§1e) is that `triangle_pipeline` and
    /// `triangle_target_extent` are `Some` together or `None` together. A
    /// resize that populated `triangle_target_extent` on a backend with no
    /// pipeline would break it, and `draw_admitted_triangles` reads the two
    /// through separate `ok_or`s -- so the broken state would not be caught
    /// there, it would just draw at an extent no device was ever requested
    /// for. `configured_target_extent` is a different field with a different
    /// rule: it is deliberately adapter-independent (see `create_inner`), so
    /// it IS written here.
    #[test]
    fn resize_before_create_records_only_the_adapter_independent_extent() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        assert_eq!(backend.configured_target_extent, None);
        assert_eq!(backend.triangle_target_extent, None);

        backend.resize(320, 240);

        assert_eq!(
            backend.configured_target_extent,
            Some(TriangleTargetExtent {
                width: 320,
                height: 240,
            }),
            "the CPU-side fill path's color-image height must follow a resize even with no adapter"
        );
        assert_eq!(
            backend.triangle_target_extent, None,
            "a resize must never populate a triangle extent with no pipeline behind it"
        );
        assert!(
            backend.triangle_pipeline.is_none(),
            "a resize must not request a device -- that is create()'s job, not this one's"
        );
    }

    /// A resize AFTER a successful create updates both extents together and
    /// keeps the live pipeline.
    ///
    /// `create_inner` here is allowed to report `NoAdapter` (this test must
    /// run on the default, adapterless configuration too), so the pipeline
    /// half is asserted conditionally on whether one was actually obtained
    /// -- what is unconditional is the pairing: whichever way create went,
    /// `triangle_target_extent.is_some()` still equals
    /// `triangle_pipeline.is_some()` after the resize, and the extent that
    /// exists is the new one, never the create-time one.
    #[test]
    fn resize_after_create_updates_both_extents_and_keeps_the_pipeline() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let had_pipeline = backend.triangle_pipeline.is_some();
        assert_eq!(
            backend.triangle_target_extent.is_some(),
            had_pipeline,
            "create_inner's own pairing invariant must hold before the resize is judged"
        );

        backend.resize(640, 480);

        let resized = TriangleTargetExtent {
            width: 640,
            height: 480,
        };
        assert_eq!(
            backend.configured_target_extent,
            Some(resized),
            "the fill path's extent must be the resized one, not create()'s"
        );
        assert_eq!(
            backend.triangle_pipeline.is_some(),
            had_pipeline,
            "a resize must not drop or re-request the device -- nothing this backend owns is \
             sized at create time"
        );
        assert_eq!(
            backend.triangle_target_extent.is_some(),
            had_pipeline,
            "the pipeline/extent pairing must survive a resize in both directions"
        );
        if had_pipeline {
            assert_eq!(
                backend.triangle_target_extent,
                Some(resized),
                "a live pipeline's raster extent must follow the resize; the per-submission \
                 attachments are built from it"
            );
        }
    }

    /// A resize to the SAME dimensions is a real write of the same value,
    /// not a special-cased early return.
    ///
    /// There is deliberately no `if new == old { return }` guard: this
    /// method allocates nothing and touches no device, so an equality check
    /// would only add a branch whose "skip" arm is indistinguishable from
    /// the silent no-op this whole change removes. The observable
    /// requirement is idempotence, which is what this asserts.
    ///
    /// A same-dimensions resize is trivially satisfied by a no-op, so this
    /// deliberately resizes AWAY first and then back: that makes the return
    /// leg a real write whose result a no-op cannot produce, and only then
    /// is repeating it asserted to be a fixed point.
    #[test]
    fn resize_to_the_same_dimensions_is_idempotent() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let after_create = backend.configured_target_extent;
        let pipeline_extent_after_create = backend.triangle_target_extent;

        // Away, so the return leg below cannot be satisfied by doing nothing.
        backend.resize(FILL_TARGET_WIDTH * 3, FILL_TARGET_HEIGHT * 3);
        assert_ne!(
            backend.configured_target_extent, after_create,
            "the intermediate resize must genuinely move the extent"
        );

        backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT);
        assert_eq!(
            backend.configured_target_extent, after_create,
            "resizing back to create()'s own dimensions must restore exactly that extent"
        );
        assert_eq!(
            backend.triangle_target_extent, pipeline_extent_after_create,
            "same for the triangle extent"
        );

        backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT);
        assert_eq!(
            backend.configured_target_extent, after_create,
            "and a repeated identical resize must still be a fixed point"
        );
        assert_eq!(
            backend.triangle_target_extent, pipeline_extent_after_create,
            "the triangle extent must be a fixed point under repetition too"
        );
    }

    /// A resize to zero is RECORDED, not clamped and not ignored, and the
    /// zero then surfaces as a named rejection at the point of use.
    ///
    /// This is the honest reading of the trait's own contract ("a backend
    /// that cannot honor a resize should surface that at the next
    /// `process_task`/`present` call ... not here"): `resize` is infallible,
    /// so the refusal has to live downstream, and it already does --
    /// `ColorTargetExtent::try_new` rejects a zero height with
    /// `TargetError::ZeroExtent`. Clamping to 1 would invent a target
    /// geometry the host never asked for and publish a resident whose byte
    /// range is wrong; ignoring the call would be the silent no-op this
    /// change exists to delete. Asserting the *named* error, not merely
    /// "some error", is the point.
    #[test]
    fn resize_to_zero_is_recorded_and_rejected_by_name_at_the_fill() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        backend.resize(FILL_TARGET_WIDTH, 0);
        assert_eq!(
            backend.configured_target_extent,
            Some(TriangleTargetExtent {
                width: FILL_TARGET_WIDTH,
                height: 0,
            }),
            "a zero extent must be recorded verbatim -- never clamped to 1, never dropped"
        );

        let planned = plan_with_no_reads(&mut backend, &session, whole_target_fill_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let error = backend
            .execute_raw_dpc(bound)
            .expect_err("a fill against a zero-height target must not execute");
        let RenderError::Backend { reason, .. } = error else {
            panic!("expected a backend-scoped rejection, got {error:?}");
        };
        assert_eq!(
            reason,
            WgpuRawDpcExecutionError::Target(TargetError::ZeroExtent {
                width: FILL_TARGET_WIDTH,
                height: 0,
            })
            .to_string(),
            "the zero must surface as the color target's own named ZeroExtent, not as a generic \
             failure and not as a successful fill of a fabricated one-row target"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a rejected fill must stage no redeemable token"
        );
    }

    /// A resize between `execute_raw_dpc` and `publish_raw_dpc` must NOT
    /// disturb the outstanding fill token, and the fill must still publish
    /// at the extent it actually executed against.
    ///
    /// Why keeping it is correct rather than merely convenient: the token's
    /// `InitializedCandidateColorTarget` sealed its own `ColorTargetKey`
    /// (address, extent, byte range) when the fill ran, and
    /// `ColorTargetRegistry::prepare_publication` reads only that captured
    /// key -- it never re-derives one from the backend's current
    /// `configured_target_extent`. So a resize structurally cannot retarget
    /// an already-executed fill; the invariant is in the type, not in a
    /// guard inside `resize`. Dropping the token would instead throw away a
    /// completed submission's guest-write report and fail it with
    /// `EffectCountMismatch` for a window resize it had nothing to do with.
    ///
    /// The resize here is deliberately to a DIFFERENT height than the fill
    /// executed at, so a hypothetical re-derivation would produce a
    /// different key and be caught.
    #[test]
    fn a_resize_between_execute_and_publish_leaves_the_fill_token_redeemable() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let request = session.plan_request(capture(whole_target_fill_words()));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("fixture plans cleanly");
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("fixture executes cleanly");
        assert!(
            backend.has_pending_fill_publication(),
            "this fixture must actually stage a token, or the claim below is vacuous"
        );

        // The window changes size in the middle of the staged-then-publish
        // window -- a different height, so a re-derived key would differ.
        backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT * 2);
        assert_eq!(
            backend.configured_target_extent,
            Some(TriangleTargetExtent {
                width: FILL_TARGET_WIDTH,
                height: FILL_TARGET_HEIGHT * 2,
            }),
            "the resize must genuinely have landed, or this test proves nothing about surviving \
             one"
        );
        assert!(
            backend.has_pending_fill_publication(),
            "a resize must not drop an outstanding fill token"
        );

        let staged = backend.staged_guest_render_target_writes(submission);
        assert!(
            !staged.is_empty(),
            "the staged guest writes must still bind to their own submission after a resize -- \
             an empty list here would drive the caller into the zero-write commit branch and \
             fail a submission that completed correctly"
        );
        let committed = session
            .commit_guest_render_target_writes(prepared, staged.clone())
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);

        assert!(
            !backend.has_pending_fill_publication(),
            "the token must be redeemed by the publish, not left behind"
        );
        let residents = backend
            .color_targets()
            .expect("an executed fill builds the registry")
            .residents();
        assert_eq!(residents.len(), 1, "exactly one resident target was filled");
        assert_eq!(
            residents[0].key().extent().height(),
            FILL_TARGET_HEIGHT,
            "the published resident must carry the extent the fill EXECUTED at, never the \
             post-resize one -- the key is sealed in the token, not re-derived at publish"
        );
    }

    /// The adapterless CPU-side fill path still works after a resize, at the
    /// new geometry.
    ///
    /// `configured_target_extent` exists precisely so an admitted
    /// `FillRectangle` executes with no adapter (see its own doc), and this
    /// change writes that field -- so the hazard is that a resize breaks the
    /// one path the field was created for. This drives a full
    /// plan/execute/commit/seal/publish cycle at a resized height and proves
    /// the resident lands at the NEW extent, which also proves the resize
    /// actually reached the fill path rather than being cosmetic.
    ///
    /// Deliberately not `#[cfg(feature = "host-gpu-tests")]`: the whole
    /// point is the no-adapter case, and this host having a real Metal
    /// adapter must not be what makes it pass. The fill executor is CPU-side
    /// on either host.
    #[test]
    fn the_adapterless_fill_path_still_works_after_a_resize() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // Half the height create() configured: a real geometry change the
        // fill's own ColorTargetKey must pick up.
        let resized_height = FILL_TARGET_HEIGHT / 2;
        backend.resize(FILL_TARGET_WIDTH, resized_height);

        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x0842_1085));
        words.extend(fill_rectangle(
            0,
            0,
            FILL_TARGET_WIDTH - 1,
            resized_height - 1,
        ));
        let staged = publish_one_fill(&mut backend, &mut session, words);
        assert!(
            !staged.is_empty(),
            "an admitted fill must still declare guest writes after a resize"
        );

        let registry = backend
            .color_targets()
            .expect("an executed fill builds the registry");
        let residents = registry.residents();
        assert_eq!(residents.len(), 1, "exactly one resident target was filled");
        assert_eq!(
            residents[0].key().extent().height(),
            resized_height,
            "the fill must have executed against the RESIZED height -- equal to \
             FILL_TARGET_HEIGHT here would mean the resize never reached the fill path"
        );
        assert_eq!(
            residents[0].key().extent().width(),
            FILL_TARGET_WIDTH,
            "width comes from SetColorImage's own wire field, not from the resize"
        );
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
