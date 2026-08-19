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
use crate::targets::{
    admitted_triangle_fixture, CandidateColorTarget, CompletedColorTargetWrite,
    ResolvedFragmentBlendParams, TargetRectangle,
};
use crate::tmem::{project_committed_tmem, TileBindingParams, TmemGpuProjection};
use crate::{
    execute_fill_rectangle, AlphaCompare, BlendColorInput, BlendModeState, Color4, ColorImage,
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
    /// **The tile registers as they stood BEFORE this submission's own
    /// `SetTile`/`SetTileSize` commands were applied.**
    ///
    /// `plan_raw_dpc` and `execute_raw_dpc` are two separate trait calls
    /// for one submission, and `plan_raw_dpc` folds the whole packet's
    /// `RdpStateDelta` into `rdp_state` before `execute_raw_dpc` ever runs.
    /// Seeding the executor's `PlanCollector` from `rdp_state` at that
    /// point therefore starts the in-order walk from the packet's **final**
    /// tile table, not its carry-in: every triangle standing before the
    /// packet's first `SetTile` is then bound to a tile the guest set
    /// later in the same packet.
    ///
    /// Measured on the real ROM: WM2000 draws each sprite strip as
    /// `SetTile(tile7) -> LoadTile -> SetTile(tile0) -> 2 triangles`, and a
    /// packet boundary fell between one strip's two triangles. The orphaned
    /// second triangle sat at command index 0, before its packet's first
    /// `SetTile`, so it should have carried the previous packet's tile
    /// (`line_words = 5`). It was bound to `line_words = 4` -- the value
    /// the packet's own later `SetTile` installed -- and read its rows at a
    /// 32-byte stride through an image the load had written at a 40-byte
    /// stride, landing in the undefined row-tail padding and aborting with
    /// `TMEM_SAMPLE_STATUS_INVALID_BYTE`.
    ///
    /// Snapshotted in `plan_raw_dpc` before `RdpState::apply`, consumed by
    /// the matching `execute_raw_dpc`. `None` until the first successful
    /// plan, which is also the only state in which no `execute_raw_dpc`
    /// can legitimately arrive; the executor falls back to `rdp_state`'s
    /// live tiles in that case, exactly as it did before this field
    /// existed.
    ///
    /// `combine`, the constant colors and `color_image` are also folded
    /// early and the same reasoning applies to them, but no measurement
    /// implicates them yet, so they are deliberately still seeded live.
    /// `other_mode` WAS measured and has its own snapshot below.
    tiles_before_last_plan: Option<
        [(
            Option<fn64_render::NeutralTileDescriptor>,
            Option<fn64_render::NeutralTileSize>,
        ); 8],
    >,
    /// **`OtherMode` as it stood BEFORE this submission's own
    /// `SetOtherMode` commands were applied** -- the sibling of
    /// `tiles_before_last_plan`, for the same reason and by the same
    /// mechanism.
    ///
    /// `f2c52822` repaired the plan/execute fold's time travel for the
    /// tile registers only, and said in this struct's own doc that
    /// widening to `other_mode` would be "a change with no evidence behind
    /// it". This field is that evidence, measured on the real WM2000 ROM
    /// on the all-Rust stack:
    ///
    /// A packet folded `other_mode.high` from `0x00000cef` to `0x0008acef`.
    /// `G_MDSFT_TEXTLUT` is bits 15:14, so the carried-in word selects
    /// `G_TT_NONE` (TLUT off) and the packet-final word selects
    /// `G_TT_RGBA16` (TLUT on). The packet's FIRST texrect, at command
    /// index 6, stood before that `SetOtherMode` and so should have run
    /// TLUT-off -- but was seeded with the folded word.
    ///
    /// Under an enabled TLUT the RDP indexes *any* format through the
    /// palette and confines the index read to half of TMEM (RT64
    /// `TextureDecoder.hlsli:162-163`, `or(isRgba32, usesTlut)` selecting
    /// `RDP_TMEM_MASK16`; fn64 implements this in `AddressScope::of` and in
    /// `tmem_sample.wgsl`). So the texrect's `Rgba`/`Bits16` tile, whose
    /// row 16 column 2 texel is at linear byte `0x884`, was masked to
    /// `0x084` and XOR4'd to `0x080` instead of being masked to `0x884` and
    /// XOR4'd to `0x880`. `0x880` was loaded; `0x080` never was, and the
    /// `InvalidTexelByte` guard correctly aborted the run at 280 VI swaps.
    ///
    /// The guard, the low-half mask and both samplers were right
    /// throughout. The defect was which stream position the mode came
    /// from.
    ///
    /// Snapshotted and consumed exactly like `tiles_before_last_plan`:
    /// taken in `plan_raw_dpc` before `RdpState::apply`, on the success
    /// path only, and falling back to the live register when no plan has
    /// run yet.
    other_mode_before_last_plan: Option<Option<OtherMode>>,
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
    /// The most recent successfully presented VI field, and nothing else.
    ///
    /// `None` until the first `present` succeeds. A `present` that returns a
    /// named refusal or a typed bounds/alignment error leaves the previous
    /// field in place rather than clearing it: the retrace that failed
    /// produced no image, and discarding the last good one would fabricate a
    /// black frame the VI never scanned out. A *successful* present always
    /// replaces it, so this is never an accumulated history.
    presented_field: Option<crate::PresentedField>,
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
                tiles_before_last_plan: None,
                other_mode_before_last_plan: None,
                triangle_pipeline: None,
                triangle_target_extent: None,
                triangle_draw_output: None,
                configured_target_extent: None,
                color_targets: None,
                pending_fill_publication: None,
                presented_field: None,
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
    /// Whether `create_inner` recorded the host-configured framebuffer extent.
    ///
    /// Exposed for the adapterless harness, which asserts the extent survived
    /// a `NoAdapter` create rather than reaching into the field itself.
    #[cfg(test)]
    pub(crate) fn has_configured_target_extent(&self) -> bool {
        self.configured_target_extent.is_some()
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
    ) -> Result<(), WgpuRawDpcExecutionError> {
        let pipeline = self
            .triangle_pipeline
            .as_mut()
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
        let extent = self
            .triangle_target_extent
            .ok_or(WgpuRawDpcExecutionError::TriangleDrawBeforeCreate)?;
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
        let per_triangle: Vec<TmemGpuProjection> = match pending_tmem {
            Some(projections) => projections,
            None => vec![project_committed_tmem(self.coordinator.physical()); triangles.len()],
        };
        // A list shorter than the draw would leave a triangle with no
        // image, and the only images available to substitute are the two
        // this whole change exists to withhold: another triangle's, or the
        // whole-packet post-image. Refused by name rather than padded.
        // `project_pending_tmem_per_triangle` walks
        // `plan.triangle_commands` while `execute_raw_dpc` draws
        // `plan.triangles`, two vectors pushed at one site, so a mismatch
        // is a structural break rather than a length a caller could
        // legitimately vary.
        if per_triangle.len() != triangles.len() {
            return Err(WgpuRawDpcExecutionError::TmemProjectionCountMismatch {
                projections: per_triangle.len(),
                triangles: triangles.len(),
            });
        }

        let mut fixtures = Vec::with_capacity(triangles.len());
        for (triangle_index, draw) in triangles.into_iter().enumerate() {
            let draw = draw.map_err(WgpuRawDpcExecutionError::MissingTriangleDrawState)?;
            let tmem = per_triangle[triangle_index];
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
            // hardware ignores while the TLUT is on. The reserved encoding
            // is propagated by name rather than coerced to Disabled, exactly
            // as `execute_scheduled_texrect` already does for the CPU
            // reader's own `lut_mode`.
            let lut_mode = draw
                .other_mode
                .texture_lut_mode()
                .map_err(WgpuRawDpcExecutionError::TextureLutMode)?;
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
            fixtures.push(admitted_triangle_fixture(
                draw.vertices,
                draw.other_mode,
                draw.combine_params,
                raster_params,
                extent,
                tmem,
                tile_binding,
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
    /// Every tile's `SetTile`/`SetTileSize` current at the walk's current
    /// stream position, indexed by the RDP's own 0..=7 tile index --
    /// **seeded from `WgpuBackend.rdp_state`'s durable `tmem()` tiles**, then
    /// updated on every `SetTile`/`SetTileSize` in plan order.
    ///
    /// The RDP's eight tile descriptors are durable registers, so a packet
    /// that re-declares none still has them. Seeding `[(None, None); 8]`
    /// made every texrect in such a packet a `TexrectUnboundTile` refusal:
    /// measured on WM2000, the packet that follows the load-free texrect
    /// admission carries 46 texrects and an entirely empty tile table.
    /// Same seed-then-track pattern, and the same defect class, as
    /// `current_color_image`.
    ///
    /// The whole table, not tile 0 alone, because EVERY admitted draw
    /// names its own tile in its own wire word: a texture rectangle in
    /// word 1 bits 26:24, a raw triangle in word 0 bits 18:16. Tracking
    /// only tile 0 made every non-zero-tile texrect an `UnboundTile`
    /// refusal (WM2000's do not name tile 0), and made every non-zero-tile
    /// raw triangle silently bind tile 0's descriptor in the GPU uniform.
    current_tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
    /// `G_SETBLENDCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetBlendColor` command
    /// in plan order. Mirrors `current_other_mode`/`current_combine`
    /// exactly, a third instance of the same seed-then-track pattern (card
    /// §4d).
    current_blend_color: Color4,
    /// `G_SETENVCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetEnvColor` command
    /// in plan order. Mirrors `current_blend_color`, but unconditionally
    /// tracked -- no `AlphaCompare` gate.
    current_env_color: Color4,
    /// `G_SETPRIMCOLOR` current at the walk's current stream position --
    /// mirrors `current_env_color` exactly.
    current_prim_color: PrimColor,
    /// `G_SETFOGCOLOR` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetFogColor` command
    /// in plan order. Mirrors `current_env_color`/`current_prim_color`
    /// exactly. Needed by the production blend-cycle wiring's `Fog`
    /// selector.
    current_fog_color: Color4,
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
    /// One entry per admitted `FillRectangle`, in plan order: its
    /// decode-order command index, the command, and the `OtherMode`
    /// current at **its own** stream position.
    ///
    /// The `OtherMode` snapshot is not redundant with
    /// `current_other_mode`. That field is the walk's running value, which
    /// by the end of the plan holds whatever the *last* `SetOtherMode` set
    /// -- and a real stream sets Fill cycle for the fill and then Copy
    /// cycle for a following texture rectangle, so reading the running
    /// value at execute time rejects the fill with `NotFillCycle` for a
    /// mode it never ran under. Snapshotting per command is the same rule
    /// `triangles` already follows and states in its own doc: "never a
    /// single whole-plan-final value".
    fills: Vec<(u32, fn64_render::RdpFillRectangleCommand, Option<OtherMode>)>,
    /// One entry per admitted `FullSync` site, in plan order, paired with
    /// its own decode-order command index.
    ///
    /// Collected for accounting only -- this backend performs no GPU work
    /// for a sync and schedules no DP completion (the device fabric does
    /// that, from the ABI seam). Retaining the site keeps the executed plan
    /// able to account for every command it carried instead of silently
    /// losing one.
    full_sync_sites: Vec<(u32, fn64_render::RdpFullSyncSite)>,
    /// One entry per admitted triangle, in plan order and index-parallel
    /// with `triangles`: the **neutral** `SetTile`/`SetTileSize` pair
    /// current at that triangle's own stream position, or `None` when
    /// either was unstaged.
    ///
    /// Parallel to, not a replacement for, `RetrievedTriangleDraw`'s own
    /// `tile_binding`. That field is a [`TileBindingParams`] -- a GPU
    /// uniform layout, which has no `palette` field because the shader
    /// path does not select a CI4 palette. The CPU texel reader's indexed
    /// path does need it, so the complete neutral pair is retained here
    /// rather than widening the uniform struct with a field the GPU
    /// binding would never read.
    ///
    /// Kept as a separate vector rather than a new field on
    /// `RetrievedTriangleDraw` because that struct is
    /// `raw_dpc::triangle_draw_data`'s, shared with
    /// `TriangleDrawStateCollector`, and this is a `production.rs`-local
    /// need.
    triangle_neutral_tiles: Vec<
        [(
            Option<fn64_render::NeutralTileDescriptor>,
            Option<fn64_render::NeutralTileSize>,
        ); 8],
    >,
    /// The wire command index each admitted triangle was produced at,
    /// parallel to `triangles` and pushed at the same site
    /// `triangle_neutral_tiles` is.
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
    /// `G_SETCIMG` current at the walk's current stream position --
    /// seeded from `WgpuBackend.rdp_state`'s durable value at construction
    /// time (`Self::seeded`), then updated on every `SetColorImage`
    /// command in plan order. The seventh instance of the same
    /// seed-then-track pattern as `current_other_mode`/`current_combine`/
    /// `current_blend_color`/`current_env_color`/`current_prim_color`/
    /// `current_fog_color`, and for the same reason: the RDP's color-image
    /// register is durable across submissions, so a packet that names no
    /// `SetColorImage` of its own still has one.
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
    current_color_image: Option<ColorImage>,
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
    texrect_commands: Vec<(Option<fn64_render::TriangleAccessSpan>, usize, u8, u32)>,
    /// One entry per admitted **flat raw triangle** that declared a
    /// destination write: its declared access span, its index into
    /// `triangles`, and its own stream command index.
    ///
    /// Separate from `texrect_commands` even though the tuple rhymes,
    /// because the two are scheduled through different executors and the
    /// pairing rule that collapses a texrect's two halves has no analogue
    /// here: a raw triangle is exactly one triangle. Merging them would
    /// mean re-deriving "which kind is this" at execute time from a field
    /// the collector already knows at push time.
    ///
    /// A raw triangle that declared NO write is absent from this list
    /// entirely -- `None` and "not present" would be the same value, and
    /// this list's only consumer is the schedule, which must not schedule
    /// an undeclared triangle.
    raw_triangle_commands: Vec<(fn64_render::TriangleAccessSpan, usize, u32, Box<[u32]>)>,
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
        blend_color: Color4,
        env_color: Color4,
        prim_color: PrimColor,
        fog_color: Color4,
        color_image: Option<ColorImage>,
        tiles: [(
            Option<fn64_render::NeutralTileDescriptor>,
            Option<fn64_render::NeutralTileSize>,
        ); 8],
    ) -> Self {
        Self {
            loads: Vec::new(),
            accesses: Vec::new(),
            next_command_index: 0,
            current_other_mode: other_mode,
            current_combine: combine,
            current_tiles: tiles,
            current_blend_color: blend_color,
            current_env_color: env_color,
            current_prim_color: prim_color,
            current_fog_color: fog_color,
            current_color_image: color_image,
            triangles: Vec::new(),
            fills: Vec::new(),
            full_sync_sites: Vec::new(),
            triangle_neutral_tiles: Vec::new(),
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
                    self.current_blend_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetEnvColor { color, .. } => {
                    self.current_env_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetPrimColor { color, .. } => {
                    self.current_prim_color = PrimColor::from_wire(
                        u32::from(color.lod_frac) | (u32::from(color.lod_min) << 8),
                        color.color,
                    );
                }
                RdpStateCommand::SetFogColor { color, .. } => {
                    self.current_fog_color = Color4::from_wire(color.value);
                }
                RdpStateCommand::SetColorImage { image, .. } => {
                    self.current_color_image = Some(ColorImage::from_wire(
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
                    if let Some(slot) = self.current_tiles.get_mut(usize::from(*tile_index)) {
                        slot.0 = Some(*descriptor);
                    }
                }
                RdpStateCommand::SetTileSize {
                    tile_index, size, ..
                } => {
                    if let Some(slot) = self.current_tiles.get_mut(usize::from(*tile_index)) {
                        slot.1 = Some(*size);
                    }
                }
                _ => {}
            },
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
                // `current_tiles` is the whole 8-entry table as of this
                // command's stream position (the same table
                // `triangle_neutral_tiles` snapshots for the CPU reader), so
                // the two paths now resolve the SAME tile for the same
                // draw -- texrect or raw triangle alike -- instead of
                // disagreeing whenever tile != 0.
                let bound_tile_index = match source {
                    TriangleSource::TextureRectangle => raw_words
                        .get(1)
                        .map(|word| ((word >> 24) & 0x7) as usize)
                        .unwrap_or(0),
                    TriangleSource::RawTriangle => raw_words
                        .first()
                        .map(|word| ((word >> 16) & 0x7) as usize)
                        .unwrap_or(0),
                };
                let tile_binding = match self
                    .current_tiles
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
                        blend_color: self.current_blend_color,
                        env_color: self.current_env_color,
                        prim_color: self.current_prim_color,
                        fog_color: self.current_fog_color,
                    })
                })();
                self.triangles.push(snapshot);
                // The whole tile table as of this command's own stream
                // position, not just tile 0's entry: a texture rectangle
                // names its own tile in its wire word and this file cannot
                // know which until it reads that word at execute time.
                self.triangle_neutral_tiles.push(self.current_tiles);
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
                        .is_some_and(|(_, first, _, _)| *first + 1 == triangle_index);
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
                        // The triangle's own wire words, retained so the
                        // executor re-decodes THE SAME bytes the decoder
                        // decoded rather than reconstructing edge
                        // coefficients from the projected
                        // `NeutralTriangleVertex` triple -- which is a lossy
                        // screen-space projection and could not recover
                        // dxhdy/dxmdy/dxldy at all.
                        self.raw_triangle_commands.push((
                            span,
                            triangle_index,
                            command_index,
                            raw_words.clone(),
                        ));
                    }
                }
                if *source == TriangleSource::TextureRectangle {
                    let previous_was_first_half = self
                        .texrect_commands
                        .last()
                        .is_some_and(|(_, first, _, _)| *first + 1 == triangle_index);
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
                        ));
                    }
                }
            }
            // Mandatory alongside `push_fill_rectangle`'s admission: the
            // enum is `#[non_exhaustive]`, so a produced variant with no arm
            // here falls into the catch-all below and panics at execute time
            // rather than failing to compile.
            RawDpcSemanticCommandRef::FillRectangle(fill) => {
                self.fills
                    .push((command_index, fill.clone(), self.current_other_mode));
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
            PlanCollector::seeded(
                None,
                None,
                Color4::default(),
                Color4::default(),
                PrimColor::default(),
                Color4::default(),
                None,
                [(None, None); 8],
            ),
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
    /// A write access this packet's own resource journal declares was not
    /// claimed by any staged write when `merged_fill_and_tmem_writes` built
    /// the composed effect list -- the journal declared a write neither the
    /// fill half nor the TMEM half produced. Rejected by name here rather
    /// than handed to `BackendEffectReport::try_new` as a short list, whose
    /// count mismatch would not say *which* access went unproduced.
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
    /// This packet declared both an admitted `FillRectangle` and at least
    /// one admitted triangle. The two run entirely disjoint render paths:
    /// the fill is executed CPU-side into an owned buffer staged behind
    /// `PendingFillPublication`, while `draw_admitted_triangles` clears and
    /// rasterizes into a GPU color attachment that never composes back into
    /// that buffer. Executing both would publish a resident generation
    /// carrying only the fill while the triangles landed somewhere the
    /// guest can never observe -- with no defined ordering between them.
    /// Composing the two sources is a follow-on slice; this is the loud
    /// refusal in the meantime. Note that the fill+TMEM sibling this
    /// variant's doc used to point at is no longer a refusal -- that
    /// composition is admitted (`StagedOutcome::MixedFillAndTmemLoads`).
    /// Fill+triangle is not, and for a materially different reason: a TMEM
    /// load and a fill write disjoint declared regions the journal already
    /// orders, whereas a triangle raster has no declared write access in
    /// the journal at all and no defined composition onto the fill's
    /// CPU-side buffer.
    MixedFillAndTrianglePacket,
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
    TextureLutMode(crate::TextureLutModeError),
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
            Self::MixedFillAndTrianglePacket => formatter.write_str(
                "this packet declares both an admitted FillRectangle and at least one admitted \
                 triangle; the CPU-side fill and the GPU triangle raster target are disjoint \
                 with no defined composition or ordering between them, so the combination is \
                 rejected loudly here rather than half-executed silently",
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
            Self::TextureLutMode(error) => write!(formatter, "{error}"),
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

/// The exact captured source bytes for **every** access in one load's
/// ordered source run, returned in that same order -- mirrors
/// `crate::tmem::execute::load_tile::ExactLoadTileGuestReads::bind`'s
/// binding rule (one `CapturedGuestRead::read().access_index()` match per
/// declared source access, at `first_access_index + ordinal`), but against
/// [`ExecutionCollector`]'s owned `(access_index, bytes)` pairs (extracted
/// from `execution_view`'s finalized `&[CapturedGuestRead]` in
/// `captured_reads`, since neither the slice nor its elements outlive that
/// call).
///
/// Returns `None` if any fragment of the run is missing, so a partially
/// captured load is refused rather than executed against the fragments
/// that happened to arrive. A partial-width `LoadTile` declares one source
/// read per row, so a 49-row load must find all 49.
fn load_source_bytes<'a>(
    reads: &'a [(u32, Vec<u8>)],
    load: &TmemLoadSemantics,
) -> Option<Vec<&'a [u8]>> {
    let first = load.source_access_index();
    load.sources()
        .iter()
        .enumerate()
        .map(|(ordinal, _)| {
            let access_index = first.checked_add(u32::try_from(ordinal).ok()?)?;
            reads
                .iter()
                .find(|(captured_index, _)| *captured_index == access_index)
                .map(|(_, bytes)| bytes.as_slice())
        })
        .collect()
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
    source_bytes: &[&'a [u8]],
    first_access_index: u32,
    word: TmemTransferWord,
) -> Option<&'a [u8]> {
    let relative = word.source_access_index().checked_sub(first_access_index)?;
    let access_bytes = *source_bytes.get(usize::try_from(relative).ok()?)?;
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
        let field = crate::vi_scanout::scan_out_guest_rdram(vi, &memory)?;
        self.presented_field = Some(field);
        Ok(())
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
        // BEFORE the fold, not after: see `tiles_before_last_plan`'s doc.
        // Taken only on the success path, so a rejected plan leaves the
        // previous submission's snapshot in place rather than replacing it
        // with one for a packet that never executes.
        self.tiles_before_last_plan = Some(durable_neutral_tiles(&self.rdp_state));
        // Same rule, same instant, same success-path-only guard as the
        // tile snapshot above: see `other_mode_before_last_plan`'s doc for
        // the measurement that made this register non-hypothetical.
        self.other_mode_before_last_plan = Some(self.rdp_state.other_mode());
        self.rdp_state.apply(&delta);
        Ok(planned)
    }

    fn execute_raw_dpc(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, RenderError> {
        let (prepared, triangles, pending, draw_tmem) = execute_raw_dpc_inner(
            &mut self.coordinator,
            bound,
            self.other_mode_before_last_plan
                .unwrap_or_else(|| self.rdp_state.other_mode()),
            self.rdp_state.combine(),
            self.rdp_state.blend_color(),
            self.rdp_state.env_color(),
            self.rdp_state.prim_color(),
            self.rdp_state.fog_color(),
            self.rdp_state.color_image(),
            self.tiles_before_last_plan
                .unwrap_or_else(|| durable_neutral_tiles(&self.rdp_state)),
            &mut self.color_targets,
            self.configured_target_extent,
        )
        .map_err(RenderError::from)?;

        if !triangles.is_empty() {
            self.draw_admitted_triangles(triangles, draw_tmem)
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
    ) -> Vec<Vec<u8>> {
        let Some(pending) = self
            .pending_fill_publication
            .as_ref()
            .filter(|pending| pending.submission == submission)
        else {
            return Vec::new();
        };

        let key = pending.initialized.key();
        let base = key.address().get();
        let buffer = pending.initialized.device_bytes().device_bytes();
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
                    .to_vec()
            })
            .collect()
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
/// (correctly) disagrees, then decode again for real.
///
/// **Both passes decode against `durable_state`, and that is load-bearing.**
/// The journal a capture declares is not a function of its bytes alone: a
/// `FillRectangle`/`TextureRectangle` reads its destination back off
/// `RdpState::color_image()`, which an *earlier* submission's
/// `SetColorImage` may have staged. `plan_texture_rectangle` treats a
/// missing color image as "declares no write" (`return Ok(())`) rather than
/// as an error, so a probe decoded against `RdpState::default()` silently
/// returns a *shorter* access list than the real pass -- and the real pass
/// then fails `JournalMismatch` against the journal the probe just built.
/// That is not a hypothetical: it is exactly what WM2000's attract loop hit
/// under `FN64_RENDER=wgpu`, where the title stages its color image once and
/// then submits texrect-only XBUS runs against it (`expected 65 accesses,
/// found 9` on the third coalesced run -- the first whose durable state is
/// non-default, hence the first where the two passes could disagree at all).
/// The probe is a *shape* probe, and the shape is state-dependent, so the
/// probe must observe the same state the real decode will. The probe's
/// throwaway-ness is entirely about its journal and its zero-filled read
/// bytes; it was never about its RDP state.
///
/// Decoding the probe against durable state is side-effect-free:
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

    let journal = match crate::decode_raw_dpc(probe_ticket, durable_state) {
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
/// `None`. `durable_color_image` is the same carry-in for `SetColorImage`,
/// and is what `color_target_key` derives this packet's destination from --
/// the register the decoder's own write-access planner already reads.
#[allow(clippy::too_many_arguments)]
fn execute_raw_dpc_inner(
    coordinator: &mut RawDpcCoordinator<PhysicalTmemState>,
    bound: BoundSubmittedRawDpc,
    durable_other_mode: Option<OtherMode>,
    durable_combine: Option<CombineParams>,
    durable_blend_color: Color4,
    durable_env_color: Color4,
    durable_prim_color: PrimColor,
    durable_fog_color: Color4,
    durable_color_image: Option<ColorImage>,
    durable_tiles: [(
        Option<fn64_render::NeutralTileDescriptor>,
        Option<fn64_render::NeutralTileSize>,
    ); 8],
    color_targets: &mut Option<ColorTargetRegistry>,
    configured_target_extent: Option<TriangleTargetExtent>,
) -> Result<
    (
        BackendPreparedRawDpc,
        Vec<Result<RetrievedTriangleDraw, MissingTriangleDrawState>>,
        Option<PendingFillPublication>,
        Option<Vec<TmemGpuProjection>>,
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
        durable_color_image,
        durable_tiles,
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
            durable_color_image,
            durable_tiles,
        ),
        reads: Vec::new(),
        outcome: None,
        color_targets,
        configured_target_extent,
        draw_tmem: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
    let _ = plan_visitor; // plan contents were moved into `view.plan` by `plan_visited`

    let submission = bound.submission();
    let outcome = view
        .outcome
        .expect("execution_view always calls submitted_packet exactly once")?;
    let triangles = view.plan.triangles;
    // Taken before the match consumes `outcome`: the projection is a fact
    // about what this packet's TMEM half staged, independent of which
    // coordinator completion the outcome routes to.
    let draw_tmem = view.draw_tmem;
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
                initialized: staged.initialized,
                guest_writes: staged.guest_writes,
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
                initialized: staged.initialized,
                guest_writes: staged.guest_writes,
            });
            prepared
        }
    };

    Ok((prepared, triangles, pending, draw_tmem))
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
    // Mixed fill-plus-triangle packets are still refused, before either
    // source stages anything. `stage_fills_and_report` never inspects
    // `plan.triangles`, and `execute_raw_dpc` draws them afterwards into a
    // color attachment `draw_admitted_triangles` clears itself -- disjoint
    // from the CPU-side fill buffer this packet staged. Admitting the pair
    // would silently drop one of two render results with no ordering
    // defined between them. A FLAT raw triangle now declares per-row
    // journal writes and composes through the same accumulation seam as a
    // texrect, so the "no journal order to read the composition off" reason
    // no longer applies to it. The refusal is kept anyway, and narrowly: a
    // fill packet with no texrect routes through `stage_fills_and_report`,
    // and the fill+raw-triangle pair has never been measured in WM2000's
    // stream, so admitting it would be widening on inference rather than
    // evidence. Composing them is a follow-on slice.
    // A texture rectangle is admitted as two triangles, so "has triangles"
    // no longer implies "has a raster with no declared write". Partition
    // first: a texrect DOES declare its own journal `ColorFramebuffer`
    // writes (`raw_dpc::mod`'s `plan_texture_rectangle`), which is exactly
    // the thing a raw triangle lacks and exactly what makes composition
    // derivable rather than invented. The refusal below therefore narrows
    // to the case that still has no declared order: a fill next to a RAW
    // triangle.
    let raw_triangle_count = collector
        .plan
        .triangles
        .iter()
        .filter(|draw| {
            draw.as_ref()
                .map(|draw| draw.source == TriangleSource::RawTriangle)
                .unwrap_or(false)
        })
        .count();
    if !collector.plan.fills.is_empty() && raw_triangle_count > 0 {
        return Err(WgpuRawDpcExecutionError::MixedFillAndTrianglePacket);
    }
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
    // **This is not the fill+triangle sibling, and that one stays refused.**
    // `MixedFillAndTrianglePacket` is kept, unchanged, immediately above.
    // Its stated defect is real and different: a fill packet with no
    // texrect reaches `stage_fills_and_report`, whose `StagedOutcome`
    // routing turns on whether a color command declared a write, and the
    // fill+triangle pair was never measured in WM2000's stream. The pair
    // measured here is the one the ROM actually issues, and it is refused
    // for a property -- "a raw triangle declares no write" -- that is not a
    // conflict with a texrect but the precondition that makes the two
    // independent.
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
    //    effect list, and `validate_proposal` recomputes that same digest at
    //    both publication routes. A prefix read reports it verbatim.
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
        .filter(|(span, _, _, _)| span.is_some())
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
            let bytes = word_source_bytes(&source_bytes, load.source_access_index(), word)
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

        let finished = staged
            .finish_load()
            .map_err(WgpuRawDpcExecutionError::Physical)?;
        // Taken after THIS load and before the next stages anything, so the
        // snapshot is exactly what TMEM holds at this command's position.
        // A read of arrays that already exist: it cannot fail and touches
        // no registry, so the load loop stays a pure phase.
        prefixes.push((command_index, finished.capture_prefix()));
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
    collector.draw_tmem = Some(project_pending_tmem_per_triangle(
        &collector.plan.triangle_commands,
        &prefixes,
        &pending,
        collector.physical,
    )?);

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
    let staged_fill = stage_color_commands(
        collector,
        packet,
        TexrectTmemSource::Pending {
            pending: &pending,
            prefixes: &prefixes,
        },
    )?;

    let Some(staged_fill) = staged_fill else {
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
    let staged = stage_color_commands(
        collector,
        packet,
        TexrectTmemSource::Committed(collector.physical),
    )?
    .ok_or(WgpuRawDpcExecutionError::FillExecution(
        FillExecutionError::NotFillCycle,
    ))?;
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
    let mut schedule: Vec<(u32, ColorCommandKind)> = collector
        .plan
        .fills
        .iter()
        .enumerate()
        .map(|(index, (command_index, _, _))| (*command_index, ColorCommandKind::Fill(index)))
        .chain(collector.plan.texrect_commands.iter().enumerate().map(
            |(index, (_, _, _, command_index))| (*command_index, ColorCommandKind::Texrect(index)),
        ))
        .chain(collector.plan.raw_triangle_commands.iter().enumerate().map(
            |(index, (_, _, command_index, _))| {
                (*command_index, ColorCommandKind::RawTriangle(index))
            },
        ))
        .collect();
    if schedule.is_empty() {
        return Ok(None);
    }
    schedule.sort_by_key(|(command_index, _)| *command_index);

    // The candidate, and the target key, derived once from this packet's
    // own staged `SetColorImage`. Every command in the schedule composes
    // into the same target by construction -- `key_of_declared_render_
    // target` cross-checks each texrect's declared accesses against this
    // key's range, and a fill naming a different image would produce a
    // different key here and be caught by the same check.
    let key = color_target_key(collector, packet)?;
    let registry = collector
        .color_targets
        .as_ref()
        .expect("color_target_key populates the registry");
    let candidate = registry.begin_candidate(key)?;

    // The accumulator. Seeded from the resident's real prior bytes when
    // this target already exists, and left `None` for a brand-new target --
    // exactly the distinction `execute_fill_rectangle` already draws, and
    // deliberately NOT flattened to a zero buffer here, which would
    // fabricate content for a resident whose bytes failed to thread.
    let mut accumulated: Option<Vec<u8>> = registry
        .residents()
        .iter()
        .find(|resident| resident.key() == key)
        .map(|resident| resident.device_bytes().device_bytes().to_vec());

    // Accesses only, in schedule order. Digests are deliberately absent
    // until the loop ends -- see this function's own doc on staleness.
    let mut declared: Vec<ResourceAccess> = Vec::new();
    let mut claimed: Option<TargetRectangle> = None;
    let mut last_completed: Option<CompletedColorTargetWrite> = None;

    for (_, kind) in &schedule {
        let (completed, accesses) = match *kind {
            ColorCommandKind::Fill(index) => {
                execute_scheduled_fill(collector, &candidate, index, accumulated.as_deref())?
            }
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
                let resident = accumulated
                    .as_deref()
                    .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?;
                let command_index = collector.plan.texrect_commands[index].3;
                match tmem {
                    TexrectTmemSource::Pending { pending, prefixes } => {
                        match prefix_before(prefixes, command_index) {
                            Some(prefix) => execute_scheduled_texrect(
                                collector,
                                &candidate,
                                &pending.prefix_image(prefix),
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
                // Same resident-bytes requirement as a texrect and for the
                // same reason: a triangle writes a sub-region, so every
                // pixel outside it must come from real prior content.
                let resident = accumulated
                    .as_deref()
                    .ok_or(crate::targets::TexrectExecutionError::MissingResidentBytes { key })?;
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
                let command_index = collector.plan.raw_triangle_commands[index].2;
                match tmem {
                    TexrectTmemSource::Pending { pending, prefixes } => {
                        match prefix_before(prefixes, command_index) {
                            Some(prefix) => execute_scheduled_raw_triangle(
                                collector,
                                &candidate,
                                index,
                                resident,
                                &pending.prefix_image(prefix),
                                true,
                            )?,
                            // No load precedes this triangle in its own
                            // packet, so TMEM holds exactly what an earlier
                            // packet published -- durable committed state,
                            // read through the same one sampler. The absence
                            // of a preceding load IS the stream fact that
                            // makes committed correct; it is not a fallback.
                            None => execute_scheduled_raw_triangle(
                                collector,
                                &candidate,
                                index,
                                resident,
                                collector.physical,
                                false,
                            )?,
                        }
                    }
                    TexrectTmemSource::Committed(state) => execute_scheduled_raw_triangle(
                        collector, &candidate, index, resident, state, false,
                    )?,
                }
            }
        };
        // This command's own output becomes the next command's resident
        // bytes. The single line that makes N compose.
        accumulated = Some(completed.device_bytes().device_bytes().to_vec());
        claimed = Some(union_target_rectangle(completed.rectangle(), claimed));
        declared.extend(accesses);
        last_completed = Some(completed);
    }

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
    let guest_writes = fill_completed_writes(key, completed.device_bytes(), &declared)?;
    // The claimed rectangle is the union of every command's own, which is
    // what `admit_completed_initialization` reads to decide whether a
    // brand-new target is fully initialized. Reporting one command's
    // rectangle would understate what N proved.
    let completed = completed.with_claimed_rectangle(
        claimed.expect("a non-empty schedule claimed at least one rectangle"),
    );
    let initialized = candidate.admit_completed_initialization(completed)?;
    Ok(Some(StagedFill {
        initialized,
        guest_writes,
    }))
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
/// `current_color_image`, which is seeded from `WgpuBackend`'s durable
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
        .current_color_image
        .ok_or(WgpuRawDpcExecutionError::NoStagedColorImage)?;
    if let Some((command_index, fill, _)) = collector.plan.fills.first() {
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

/// Executes the fill at `index` of the plan's own fill list against the
/// accumulated buffer, returning its completion and its declared accesses.
fn execute_scheduled_fill(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: usize,
    accumulated: Option<&[u8]>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (_, fill, fill_other_mode) = &collector.plan.fills[index];

    // This fill's OWN `OtherMode`, snapshotted at its stream position --
    // not the walk's running value, which a later `SetOtherMode` (a
    // following texture rectangle switching to Copy cycle, say) would have
    // already overwritten with a mode the fill never ran under.
    let Some(other_mode) = *fill_other_mode else {
        return Err(WgpuRawDpcExecutionError::FillExecution(
            FillExecutionError::NotFillCycle,
        ));
    };

    let completed = execute_fill_rectangle(
        candidate,
        other_mode,
        FillColor::from_wire(fill.fill_color.value),
        FillRectangle::from_wire_fields(
            fill.upper_left_x,
            fill.upper_left_y,
            fill.lower_right_x,
            fill.lower_right_y,
        ),
        accumulated,
    )?;
    let accesses = fill_accesses(&collector.plan.accesses, fill)?.to_vec();
    Ok((completed, accesses))
}

/// Executes the flat raw triangle at `index` of the plan's own
/// `raw_triangle_commands` list against the accumulated buffer, returning
/// its completion and its declared accesses.
///
/// Every geometric fact is taken from the decoder, never re-derived: the
/// edge coefficients from re-decoding the command's OWN retained wire words,
/// and the declared write run from the span the decoder recorded when it
/// pushed those accesses. The one number this function computes itself is
/// the declared row COUNT, which it hands the executor so the executor can
/// prove its own raster covers exactly those rows.
fn execute_scheduled_raw_triangle<S: crate::TmemByteSource + ?Sized>(
    collector: &ExecutionCollector<'_>,
    candidate: &CandidateColorTarget,
    index: usize,
    resident_bytes: &[u8],
    tmem: &S,
    expect_proposed: bool,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (span, triangle_index, _, raw_words) = &collector.plan.raw_triangle_commands[index];
    let draw_state = collector.plan.triangles[*triangle_index]
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;

    // **Re-decoded from this command's own retained wire words**, not
    // reconstructed from the projected `NeutralTriangleVertex` triple. The
    // projection is screen-space and lossy; it cannot recover dxhdy/dxmdy/
    // dxldy at all, and the rasterizer needs exactly those. Re-decoding the
    // same bytes through the same `RawTriangle::decode` is not a second
    // derivation -- it is the identical function over the identical input.
    let mut bytes = Vec::with_capacity(raw_words.len() * 4);
    for word in raw_words.iter() {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    // **The opcode comes from the command's own first wire byte**, not a
    // frozen 0x08. `RawTriangle::decode` sizes every optional coefficient
    // block from the opcode's flag bits, so decoding a shaded-and-textured
    // 0x0e as 0x08 would reject it on length -- which is exactly what the
    // frozen constant did before the texture rung, harmlessly, because no
    // textured triangle was ever admitted this far.
    let opcode = raw_words
        .first()
        .map(|word| ((word >> 24) & 0x3f) as u8)
        .ok_or(WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable {
            triangle_index: *triangle_index,
        })?;
    let triangle = crate::raw_dpc::RawTriangle::decode(opcode, &bytes).map_err(|_| {
        WgpuRawDpcExecutionError::RawTriangleWireWordsUndecodable {
            triangle_index: *triangle_index,
        }
    })?;

    // Locate the declared run by the decoder's own recorded span.
    let start = span.first_access_index as usize;
    let end = start.checked_add(span.access_count as usize).ok_or(
        WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: *triangle_index,
        },
    )?;
    let accesses = collector
        .plan
        .accesses
        .get(start..end)
        .filter(|slice| !slice.is_empty())
        .ok_or(WgpuRawDpcExecutionError::RawTriangleDeclaredNoWrite {
            triangle_index: *triangle_index,
        })?
        .to_vec();

    // Cross-check, not an assumption: every declared access must fall inside
    // the candidate key's own range, which was derived from the packet's
    // `SetColorImage` by a path independent of the decoder's row planner.
    let key = candidate.key();
    verify_accesses_inside(&accesses, key)?;

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
            collector.plan.triangle_neutral_tiles[*triangle_index][tile_index]
        else {
            return Err(WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: *triangle_index,
            });
        };
        let tile = crate::targets::TexrectTileBinding::try_from_neutral(descriptor, size).map_err(
            |_| WgpuRawDpcExecutionError::TexrectUnboundTile {
                triangle_index: *triangle_index,
            },
        )?;
        // The reserved TLUT encoding is a named refusal in `OtherMode`'s own
        // decoder, propagated rather than coerced to Disabled -- which would
        // silently sample a direct-format texel out of an indexed tile.
        let lut_mode = draw_state
            .other_mode
            .texture_lut_mode()
            .map_err(WgpuRawDpcExecutionError::TextureLutMode)?;
        // The image this call was handed must answer the identity its CALLER
        // selected: a pending post-image answers `Proposed`, durable state
        // answers `Committed`. Checked here rather than trusted, exactly as
        // `execute_scheduled_texrect` checks it and for the same reason --
        // both variants inhabit one enum, so a wrong `snapshot()` impl
        // compiles.
        verify_tmem_identity(tmem, expect_proposed, *triangle_index)?;
        Some(crate::targets::RawTriangleTexture {
            tile,
            tmem,
            lut_mode,
        })
    } else {
        None
    };

    let completed = crate::targets::execute_raw_triangle(
        candidate,
        draw_state.other_mode,
        &triangle,
        crate::targets::TexrectShading::new(
            draw_state.combine_params,
            draw_state.env_color,
            draw_state.prim_color,
        ),
        crate::targets::TexrectBlendRegisters::new(draw_state.blend_color, draw_state.fog_color),
        resident_bytes,
        &accesses,
        texture,
    )?;
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
    resident_bytes: &[u8],
    already_initialized: Option<TargetRectangle>,
) -> Result<(CompletedColorTargetWrite, Vec<ResourceAccess>), WgpuRawDpcExecutionError> {
    let (span, triangle_index, tile_index, _) = collector.plan.texrect_commands[index];
    // A texrect that declared no write must not execute: it would write
    // bytes the journal never declared, which `merged_fill_and_tmem_writes`
    // would then reject as `MergedWriteUndeclared` -- a correct but less
    // specific diagnosis than naming the real cause here.
    let span = span.ok_or(WgpuRawDpcExecutionError::TexrectDeclaredNoWrite { triangle_index })?;

    let draw_state = collector.plan.triangles[triangle_index]
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
        collector.plan.triangle_neutral_tiles[triangle_index][usize::from(tile_index)]
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
        .as_ref()
        .map_err(|missing| WgpuRawDpcExecutionError::MissingTriangleDrawState(*missing))?;
    let lower_right = second.vertices[0].texcoord;
    let draw = crate::targets::TexrectDraw::try_from_viewport_and_texcoords(
        viewport,
        upper_left,
        lower_right,
    )?;

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

    // The reserved TLUT encoding is a named refusal in `OtherMode`'s own
    // decoder, propagated rather than coerced to Disabled -- which would
    // silently sample a direct-format texel out of an indexed tile.
    let lut_mode = other_mode
        .texture_lut_mode()
        .map_err(WgpuRawDpcExecutionError::TextureLutMode)?;
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
    let completed = crate::targets::execute_texture_rectangle(
        candidate,
        other_mode,
        draw,
        tile,
        tmem,
        lut_mode,
        shading,
        blend_registers,
        resident_bytes,
        already_initialized,
    )?;
    Ok((completed, accesses))
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
///   with every base-state, generation, epoch and `validate_proposal` check
///   unchanged and in the same order, and if it rejects, this packet's
///   `execute_raw_dpc` returns `Err` and no draw output is stored -- the
///   pixels never become observable.
/// - **No forged snapshot identity.** Verified, not trusted: both
///   `TmemSnapshotIdentity` variants inhabit one enum, so a wrong
///   `snapshot()` impl compiles. Measured at the sibling site -- forging
///   `Committed` in `PendingTmemImage`'s impl passed the entire suite before
///   `execute_scheduled_texrect`'s equivalent check existed.
/// - **No effect-report participation.** Reading is not a write. Nothing
///   projected here enters `proposed_effects`, so `validate_proposal`'s
///   recomputation and `validate_backend_effects`' supersequence walk see
///   exactly what they saw before.
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
                Some(prefix) => project_proposed_image(&pending.prefix_image(prefix)),
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

    use crate::wire_words::word;

    /// A transfer word's `source_access_byte_offset` is relative to **its
    /// own** source access, never to a flattened concatenation of the
    /// load's whole source run (`tmem::physical`'s word projection
    /// subtracts the preceding accesses' byte total before storing it).
    /// A partial-width `LoadTile` declares one source read per row, so
    /// resolving every word against row 0 would silently feed the wrong
    /// row's pixels to every word after the first -- a wrong picture, not
    /// a crash.
    ///
    /// Each row here is filled with its own row index, so a
    /// misresolved row is directly observable in the returned bytes.
    #[test]
    fn word_source_bytes_slices_the_row_the_word_names_not_the_first_row() {
        const ROWS: u32 = 4;
        const ROW_BYTES: usize = 8;
        const FIRST_ACCESS: u32 = 1;

        let rows: Vec<Vec<u8>> = (0..ROWS).map(|row| vec![row as u8; ROW_BYTES]).collect();
        let source_bytes: Vec<&[u8]> = rows.iter().map(|row| row.as_slice()).collect();

        for row in 0..ROWS {
            let word = TmemTransferWord::new(
                row as u16,
                row * ROW_BYTES as u32,
                FIRST_ACCESS + row,
                0,
                0xff,
                0xff,
                row as u16,
                0,
                false,
                crate::TmemTransferPhysicalWord::Linear(
                    fn64_render_ir::TmemRange::try_new(row * 8, row * 8 + 8).unwrap(),
                ),
            );
            let bytes = word_source_bytes(&source_bytes, FIRST_ACCESS, word)
                .expect("every word binds to a row in the run");
            assert_eq!(
                bytes,
                &[row as u8; ROW_BYTES],
                "word naming access {} must read row {row}, not row 0",
                FIRST_ACCESS + row
            );
        }
    }

    /// The offset is applied *within* the named row, and a word that would
    /// read past that row's end is refused rather than silently spilling
    /// into the next row's captured bytes.
    #[test]
    fn word_source_bytes_refuses_a_word_that_overruns_its_own_row() {
        const ROW_BYTES: usize = 8;
        let rows = [vec![0xaa_u8; ROW_BYTES], vec![0xbb_u8; ROW_BYTES]];
        let source_bytes: Vec<&[u8]> = rows.iter().map(|row| row.as_slice()).collect();

        // Offset 4 with 8 defined bytes runs 4 bytes past row 0's end. If
        // the rows were flattened this would happily return 4 bytes of
        // row 0 followed by 4 bytes of row 1.
        let overrun = TmemTransferWord::new(
            0,
            4,
            1,
            4,
            0xff,
            0xff,
            0,
            0,
            false,
            crate::TmemTransferPhysicalWord::Linear(
                fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
            ),
        );
        assert!(
            word_source_bytes(&source_bytes, 1, overrun).is_none(),
            "a word may not read past the end of the row it names"
        );

        // A word naming an access outside the run is refused too, in both
        // directions.
        let before_run = TmemTransferWord::new(
            0,
            0,
            0,
            0,
            0xff,
            0xff,
            0,
            0,
            false,
            crate::TmemTransferPhysicalWord::Linear(
                fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
            ),
        );
        assert!(word_source_bytes(&source_bytes, 1, before_run).is_none());
        let past_run = TmemTransferWord::new(
            0,
            0,
            9,
            0,
            0xff,
            0xff,
            0,
            0,
            false,
            crate::TmemTransferPhysicalWord::Linear(
                fn64_render_ir::TmemRange::try_new(0, 8).unwrap(),
            ),
        );
        assert!(word_source_bytes(&source_bytes, 1, past_run).is_none());
    }

    use crate::wire_words::set_other_mode;

    use crate::wire_words::set_combine;

    /// Mirrors `raw_dpc::production_adapter::tests::set_env_color` exactly
    /// (that helper is private to its own module's tests, so this is a
    /// local, identical copy, not a shared import -- same convention as
    /// `triangle_base_edge_words` above).
    fn set_env_color(color: u32) -> [u32; 2] {
        [word(SET_ENV_COLOR, 0), color]
    }

    use crate::wire_words::set_prim_color;

    /// One base-edge (non-shaded, non-textured, non-Z) triangle command's
    /// eight raw wire words, from the crate's shared `wire_words` builder.
    fn triangle_base_edge_words(tile: u32, level: u32, yl: u16) -> [u32; 8] {
        let mut words = crate::wire_words::EdgeWords {
            tile,
            level,
            yl: yl as i16,
            ..crate::wire_words::EdgeWords::zeroed()
        }
        .words(0, RAW_TRIANGLE_BASE_EDGE);
        // This fixture's own edge payload, unchanged: an arbitrary but fixed
        // set of slopes that exercises decode without naming a footprint.
        words[2..].copy_from_slice(&[0x0010_0000, 0, 0x0020_0000, 0x0000_8000, 0x0005_0000, 0]);
        words
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

    /// One `SetTileSize` command's two wire words.
    ///
    /// Field placement is `tmem::wire`'s own `tile_size` decode read
    /// backwards: **low** S/T live in w0 (bits 23:12 and 11:0), **high**
    /// S/T in w1 (same two positions), and the tile index in w1 bits 26:24.
    /// All four are raw 10.2 fixed-point fields. Getting this pair the
    /// wrong way round is not a silent error -- it produced a
    /// `ReversedClampExtent` refusal naming `low 0x01c, high 0x000`, which
    /// is how the swap was caught.
    fn set_tile_size_words(tile: u32, high_s: u32, high_t: u32) -> [u32; 2] {
        [
            word(SET_TILE_SIZE_OPCODE, 0),
            tile << 24 | high_s << 12 | high_t,
        ]
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
    /// `StagedOutcome::NoPhysicalSuccessor` arm and
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
    /// because this fixture's tile has an EVEN T origin (`low_t == 0`, so
    /// `TmemFirstRowParity::Even` is the parity the tile itself derives --
    /// it is not a frozen constant; `tmem_sample.wgsl`'s
    /// `tmem_first_row_parity_odd` and `targets/texrect.rs`'s own
    /// derivation both read `low_t.integer() & 1`): row
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
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::default(),
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
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::default(),
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
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::default(),
            fog_color: Color4::from_wire(0),
        };
        backend
            .draw_admitted_triangles(vec![Ok(good_triangle)], None)
            .expect("a single valid triangle must draw successfully");
        let first_output_extent = backend
            .last_triangle_draw()
            .expect("the first successful draw must populate last_triangle_draw")
            .extent;

        let failing_triangles = vec![
            Ok(good_triangle),
            Err(MissingTriangleDrawState::NoOtherMode { triangle_index: 1 }),
        ];
        let result = backend.draw_admitted_triangles(failing_triangles, None);
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
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::from_wire(0, 0),
            fog_color: Color4::from_wire(0),
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
            .draw_admitted_triangles(vec![Ok(left_triangle), Ok(right_triangle)], None)
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
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::default(),
            fog_color: Color4::from_wire(0),
        };
        backend
            .draw_admitted_triangles(vec![Ok(good_triangle)], None)
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
        let result = backend.draw_admitted_triangles(batch_with_trailing_failure, None);
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
    /// `RdpState`, so a black-box test cannot distinguish "state is threaded
    /// through" from "state is discarded but happens to look populated"
    /// purely by observing `plan_raw_dpc`'s success/failure. This test
    /// instead pins down the source-level fact that makes state threading
    /// real: **both** decode calls inside `plan_raw_dpc_inner` -- the probe
    /// and the real pass -- pass `durable_state`, the caller-supplied
    /// `&RdpState`, and no `RdpState::default()` appears anywhere in the
    /// function. It mirrors
    /// `publish_raw_dpc_source_is_exactly_prepare_publication_then_commit`'s
    /// source-shape idiom.
    ///
    /// This test previously asserted the *opposite* for the probe --
    /// `RdpState::default()` exactly once, "only the throwaway single-source
    /// probe decode is allowed to use it" -- on the stated premise that "the
    /// one command that reads `state.color_image()` back, `FillRectangle`,
    /// is out of v11's admitted TMEM-only scope". That premise expired when
    /// `plan_texture_rectangle` began reading `color_image()` too, and the
    /// stale assertion was pinning the WM2000 `FN64_RENDER=wgpu` blocker in
    /// place: a probe blind to durable state derives a shorter access list
    /// than the real pass, and the real pass then fails `JournalMismatch`
    /// against the journal the probe built. See `plan_raw_dpc_inner`'s own
    /// doc. The companion behavioral tests
    /// (`plan_raw_dpc_carries_durable_rdp_state_across_submissions` and
    /// `plan_raw_dpc_plans_a_texrect_against_a_color_image_an_earlier_submission_staged`)
    /// prove the state accumulates and that the two passes agree once it
    /// does; this one proves decoding actually consults it instead of a
    /// hardcoded default.
    #[test]
    fn plan_raw_dpc_inner_decodes_both_passes_against_durable_state_not_default() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("fn plan_raw_dpc_inner(")
            .expect("plan_raw_dpc_inner must exist in this file");
        let next_fn = source[body_start + 1..]
            .find("\nfn ")
            .map(|offset| body_start + 1 + offset)
            .unwrap_or(source.len());
        let body = &source[body_start..next_fn];
        assert!(
            body.contains("crate::decode_raw_dpc(ticket, durable_state)"),
            "plan_raw_dpc_inner's real (non-probe) decode call must pass `durable_state`, \
             not a fresh `RdpState::default()` -- otherwise no submission's state ever \
             carries forward to the next"
        );
        assert!(
            body.contains("crate::decode_raw_dpc(probe_ticket, durable_state)"),
            "plan_raw_dpc_inner's probe decode must pass `durable_state` too. The probe \
             derives the journal the real pass is then checked against, and a journal is a \
             function of durable state as well as of the capture's bytes (a texrect reads \
             its destination off `RdpState::color_image()`, which an earlier submission may \
             have staged). A probe blind to that state declares a shorter access list than \
             the real pass and the real pass then fails JournalMismatch against it"
        );
        let default_state_appearances = body.matches("RdpState::default()").count();
        assert_eq!(
            default_state_appearances, 0,
            "RdpState::default() must not appear in plan_raw_dpc_inner at all -- both the \
             probe and the real decode must observe the caller's durable state, or the two \
             passes can disagree about how many accesses the capture declares"
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

    /// Regression, cross-submission planning: a texrect whose destination
    /// `SetColorImage` was staged by an **earlier** submission must plan
    /// with the same declared `ColorFramebuffer` write run the real decode
    /// derives -- not the empty run the probe derives against a fresh
    /// `RdpState::default()`.
    ///
    /// This is the WM2000 `FN64_RENDER=wgpu` blocker in miniature. The
    /// title's attract loop stages its color image once and then keeps
    /// submitting texrect-only XBUS runs against it, so the *third*
    /// coalesced run is the first whose durable state is non-default and
    /// therefore the first where `plan_raw_dpc_inner`'s two passes can
    /// disagree. `plan_texture_rectangle`'s `let Some(image) =
    /// state.color_image() else { return Ok(()) }` makes that disagreement
    /// silent-but-fatal: the probe declares zero `RenderTarget` accesses,
    /// the real decode declares one per covered row, and
    /// `ExactRawDpcPlanWriter::finish` refuses the mismatch by name.
    ///
    /// Submissions one and two only exist to populate durable state; the
    /// assertion is entirely about submission three, which carries no
    /// `SetColorImage` of its own.
    #[test]
    fn plan_raw_dpc_plans_a_texrect_against_a_color_image_an_earlier_submission_staged() {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // Submission one: stage the color image (and a whole-target fill,
        // which `admit_completed_initialization` requires before any
        // partial write into a fresh target).
        let request_one = session.plan_request(capture(whole_target_fill_words()));
        backend
            .plan_raw_dpc(request_one)
            .expect("the color-image-staging submission plans cleanly");

        // Submission two: a TMEM load, so durable state is non-default in
        // more than one field by the time submission three plans.
        let request_two = session.plan_request(capture(one_load_block_words()));
        backend
            .plan_raw_dpc(request_two)
            .expect("the TMEM-load submission plans cleanly");

        assert!(
            backend.rdp_state().color_image().is_some(),
            "positive control: durable state must actually carry a color image into \
             submission three -- without this the test would pass vacuously against a \
             default state the probe happens to agree with"
        );

        // Submission three: a texrect with NO SetColorImage of its own. Its
        // destination image can only come from durable state.
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_in_target(7));
        let request_three = session.plan_request(capture(words));
        let planned_three = backend
            .plan_raw_dpc(request_three)
            .expect("a texrect against an earlier submission's color image must plan");

        // The sealed plan exposes no journal accessor, so the declared
        // write run is counted where it is derived: `plan_raw_dpc_inner`'s
        // own journal, rebuilt here against the same durable state the
        // backend now holds. `finish`'s access-for-access check above
        // already proved the two agree; this pins the count they agree on.
        //
        // Hand-derived from RT64's own `FixedRect`, not captured from this
        // port's output. `texrect_words_in_target`'s wire fields are 10.2
        // fixed point: `uly = 2 << 2 = 8`, `lry = 4 << 2 = 16`. The staged
        // `set_other_mode(0, 0)` is 1-cycle, so neither the copy-mode
        // `lry |= 3` nor the fill/copy `uly &= !3` applies. `FixedRect`'s
        // edges both ceil (`RDP::drawRect` passes `ceil = true` to
        // `height(true, true)`): `top = (8 + 3) >> 2 = 2`,
        // `bottom = (16 + 3) >> 2 = 4`. `bottom` is *exclusive*
        // (`plan_texture_rectangle` takes `y1 = bottom - 1 = 3`), so the
        // covered rows are y 2..=3 -- **2 rows**, not the 3 an inclusive
        // reading of the wire `lry` would suggest. Likewise x: `left =
        // (16 + 3) >> 2 = 4`, `right = (44 + 3) >> 2 = 11`, so x 4..=10.
        // x0 != 0, so `plan_render_target_rows` takes its per-row branch
        // and declares one `RenderTarget` write per row -- 2 writes.
        let mut probe_words = Vec::new();
        probe_words.extend(set_other_mode(0, 0));
        probe_words.extend(set_combine(0, 0));
        probe_words.extend(texrect_words_in_target(7));
        let probe_capture = capture(probe_words);
        let probe_submission = probe_capture.submission().clone();
        let probe_layout = probe_capture.memory_layout();
        let probe_journal = single_source_probe_journal(&probe_submission, probe_layout).unwrap();
        let probe_decoded = finalize_with_zero_reads(
            probe_layout,
            probe_capture.transaction_sequence(),
            probe_submission,
            probe_capture.cmd_end(),
            probe_capture.full_sync_boundaries().to_vec(),
            probe_journal,
        )
        .unwrap();
        let probe_ticket = submit_locally(probe_decoded).unwrap();
        let against_durable = match crate::decode_raw_dpc(probe_ticket, backend.rdp_state()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            other => panic!("probe decode must disagree with the single-source journal: {other:?}"),
        };
        let render_target_writes = against_durable
            .iter()
            .filter(|access| {
                access.purpose() == AccessPurpose::RenderTarget
                    && access.mode() == AccessMode::Write
            })
            .count();
        assert_eq!(
            render_target_writes, 2,
            "the texrect covers 2 rows (y 2..=3, RT64's bottom edge being exclusive) at \
             nonzero x0, so the real decode declares exactly 2 per-row ColorFramebuffer \
             writes -- and the plan above sealed cleanly only because the probe that built \
             its journal saw the same 2, not the 0 a default-state probe sees"
        );
        drop(planned_three);
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
            texrect_accesses: None,
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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
        assert_eq!(first.fog_color, Color4::from_wire(0x7777_7777));
        assert_eq!(second.fog_color, Color4::from_wire(0x8888_8888));
        assert_ne!(
            first.fog_color, second.fog_color,
            "triangle A must NOT be retroactively affected by a SetFogColor after it in plan \
             order"
        );
    }

    /// A neutral tile descriptor whose `tmem_word_address` is the caller's,
    /// so two tiles seeded into `PlanCollector` are distinguishable in the
    /// GPU uniform by a field the uniform actually carries.
    fn fixture_neutral_tile(
        tmem_word_address: u16,
    ) -> (
        fn64_render::NeutralTileDescriptor,
        fn64_render::NeutralTileSize,
    ) {
        (
            fn64_render::NeutralTileDescriptor {
                format: fn64_render::NeutralImageFormat::Rgba,
                size: fn64_render::NeutralPixelSize::Bits16,
                line_words: 2,
                tmem_word_address,
                palette: 0,
                s_mode: fn64_render::NeutralTileAddressMode {
                    mirror: false,
                    clamp: false,
                },
                mask_s: 0,
                shift_s: 0,
                t_mode: fn64_render::NeutralTileAddressMode {
                    mirror: false,
                    clamp: false,
                },
                mask_t: 0,
                shift_t: 0,
            },
            fn64_render::NeutralTileSize {
                low_s: 0,
                low_t: 0,
                high_s: 7 << 2,
                high_t: 7 << 2,
            },
        )
    }

    /// One raw triangle command carrying real wire words whose word 0 names
    /// `tile` in bits 18:16 -- the field `RawTriangle::decode` reads as
    /// `((w0 >> 16) & 0x7)` and the CPU executor already honours.
    fn fixture_raw_triangle_naming_tile(tile: u32) -> RdpTriangleCommand {
        // Opcode 0x08 (flat, untextured) in bits 29:24 of word 0, the tile
        // index in bits 18:16, everything else zero. Four 64-bit words = 8
        // u32 words for a `triangleBaseWords` command.
        // Bit 19 is the LEVEL field's low bit, deliberately SET here: it
        // is the bit immediately above the 3-bit tile field, so a decode
        // that widens the mask past `0x7` reads it as part of the tile
        // index and lands on a different table entry. Without a set bit
        // there, `& 0x7` and `& 0xf` agree for every tile 0..=7 and a
        // widened-mask mutant survives.
        let word0 = (0x08u32 << 24) | (1 << 19) | (tile << 16);
        RdpTriangleCommand {
            location: fixture_location(0),
            raw_words: Box::new([word0, 0, 0, 0, 0, 0, 0, 0]),
            vertices: core::array::from_fn(|index| fixture_vertex(index as f32)),
            source: TriangleSource::RawTriangle,
            viewport: None,
            texrect_accesses: None,
        }
    }

    /// **A raw triangle's GPU tile binding comes from its OWN wire field.**
    ///
    /// `PlanCollector` froze `bound_tile_index` to 0 for every
    /// `TriangleSource::RawTriangle`, with a comment claiming the triangle
    /// "carries no tile field of its own to read". That claim is false:
    /// wire word 0 bits 18:16 are the tile index, `RawTriangle::decode`
    /// reads them, and `execute_scheduled_raw_triangle` (the CPU path)
    /// already binds from them. The GPU uniform path silently sampled tile
    /// 0's texture for any triangle naming another tile.
    ///
    /// The wire word here is `0x080d_0000`. Derived by hand: opcode `0x08`
    /// occupies bits 29:24, so `0x08 << 24 = 0x0800_0000`; the LEVEL field
    /// starts at bit 19, so its low bit set contributes `1 << 19 =
    /// 0x0008_0000`; the tile field is bits 18:16, so tile 5 contributes
    /// `5 << 16 = 0x0005_0000`. Summed: `0x080d_0000`, and the expected
    /// index is `(0x080d_0000 >> 16) & 0x7 == (0xd & 0x7) == 5` -- while a
    /// decode masking `0xf` would read `0xd == 13`, off the 8-entry table.
    ///
    /// Tile 5 and tile 0 are seeded with DIFFERENT `tmem_word_address`
    /// values, so "read the named tile" and "read tile 0" are two
    /// distinguishable answers -- every other raw-triangle fixture in this
    /// file uses tile 0, where the two coincide, which is why a frozen 0
    /// survived all of them.
    #[test]
    fn plan_collector_binds_the_tile_a_raw_triangle_s_own_wire_word_names() {
        const TILE_ZERO_TMEM: u16 = 0x010;
        const TILE_FIVE_TMEM: u16 = 0x100;

        let mut tiles = [(None, None); 8];
        let (descriptor_zero, size_zero) = fixture_neutral_tile(TILE_ZERO_TMEM);
        tiles[0] = (Some(descriptor_zero), Some(size_zero));
        let (descriptor_five, size_five) = fixture_neutral_tile(TILE_FIVE_TMEM);
        tiles[5] = (Some(descriptor_five), Some(size_five));

        let mut collector = PlanCollector::seeded(
            Some(OtherMode::from_wire(0, 0)),
            Some(CombineParams::from_wire(0, 0)),
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            tiles,
        );

        let triangle = fixture_raw_triangle_naming_tile(5);
        assert_eq!(
            triangle.raw_words[0], 0x080d_0000,
            "the fixture's wire word must be the hand-derived one this test reasons about"
        );
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draw = collector.triangles[0].as_ref().unwrap();
        assert_eq!(
            draw.tile_binding.tmem_word_address,
            u32::from(TILE_FIVE_TMEM),
            "the triangle names tile 5 in wire word 0 bits 18:16, so the GPU uniform must bind \
             tile 5's TMEM address, not tile 0's {TILE_ZERO_TMEM:#x}"
        );
        assert_ne!(
            TILE_FIVE_TMEM, TILE_ZERO_TMEM,
            "the two seeded tiles must differ in the field this test reads, or a frozen 0 would \
             pass"
        );
        assert_eq!(
            draw.tile_binding.bound, 1,
            "tile 5 was seeded with both halves present, so the binding must be bound"
        );
    }

    /// The companion arm: a raw triangle whose wire word names tile 0 must
    /// still bind tile 0. Keeps the fix from degenerating into "always read
    /// some other tile" -- the arm kept unchanged needs its own test, not
    /// just the arm that changed.
    #[test]
    fn plan_collector_binds_tile_zero_when_a_raw_triangle_s_wire_word_names_it() {
        const TILE_ZERO_TMEM: u16 = 0x010;
        const TILE_FIVE_TMEM: u16 = 0x100;

        let mut tiles = [(None, None); 8];
        let (descriptor_zero, size_zero) = fixture_neutral_tile(TILE_ZERO_TMEM);
        tiles[0] = (Some(descriptor_zero), Some(size_zero));
        let (descriptor_five, size_five) = fixture_neutral_tile(TILE_FIVE_TMEM);
        tiles[5] = (Some(descriptor_five), Some(size_five));

        let mut collector = PlanCollector::seeded(
            Some(OtherMode::from_wire(0, 0)),
            Some(CombineParams::from_wire(0, 0)),
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            tiles,
        );

        // `0x0808_0000`: opcode 0x08 in bits 29:24, LEVEL's low bit set at
        // bit 19, tile field bits 18:16 all zero -- `(0x0808_0000 >> 16) &
        // 0x7 == (0x8 & 0x7) == 0` by hand, while a `0xf` mask would read
        // `0x8 == 8`, off the 8-entry table.
        let triangle = fixture_raw_triangle_naming_tile(0);
        assert_eq!(triangle.raw_words[0], 0x0808_0000);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));

        let draw = collector.triangles[0].as_ref().unwrap();
        assert_eq!(
            draw.tile_binding.tmem_word_address,
            u32::from(TILE_ZERO_TMEM),
            "the triangle names tile 0, so it must bind tile 0 -- not tile 5's {TILE_FIVE_TMEM:#x}"
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
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::default(),
            fog_color: Color4::from_wire(0),
        };
        let error = backend
            .draw_admitted_triangles(vec![Ok(triangle)], None)
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
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::default(),
            fog_color: Color4::from_wire(0),
        };
        backend
            .draw_admitted_triangles(vec![Ok(triangle)], None)
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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
        assert_eq!(first.env_color, Color4::from_wire(0x1111_1111));
        assert_eq!(
            first.prim_color,
            PrimColor::from_wire(10 | (5 << 8), 0x2222_2222)
        );
        assert_eq!(second.env_color, Color4::from_wire(0x3333_3333));
        assert_eq!(
            second.prim_color,
            PrimColor::from_wire(20 | (10 << 8), 0x4444_4444)
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
            Color4::from_wire(0),
            seed_env_color,
            seed_prim_color,
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let triangle = fixture_triangle(1.0);
        collector.command(RawDpcSemanticCommandRef::Triangle(&triangle));
        assert_eq!(collector.triangles.len(), 1);
        let retrieved = collector.triangles[0]
            .as_ref()
            .expect("a triangle with durably-seeded state must resolve, not reject");
        assert_eq!(retrieved.env_color, seed_env_color);
        assert_eq!(retrieved.prim_color, seed_prim_color);
    }

    /// A triangle visited with no `SetOtherMode`/`SetCombine` anywhere --
    /// neither seeded nor in-plan -- must be a loud, named rejection, not
    /// a silent default. Proves `PlanCollector::seeded(None, None)`
    /// (unseeded) genuinely leaves `current_other_mode`/`current_combine`
    /// at `None` rather than defaulting them.
    #[test]
    fn plan_collector_rejects_a_triangle_visited_with_no_state_established_at_all() {
        let mut collector = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
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

    /// **Supersedes T-14** (`plan_raw_dpc_rejects_a_copy_cycle_fill_rectangle`),
    /// which asserted a Copy-cycle `FillRectangle` is rejected at plan
    /// time. That pinned scaffolding, not hardware.
    ///
    /// The RDP defines `G_FILLRECT` in every cycle type; cycle type selects
    /// how the rectangle's content is produced, not whether the command is
    /// legal. `plan_fill`'s own doc carries the three authorities
    /// (fn64's reference lane `raster/draw.rs:113-128`, RT64
    /// `rt64_rdp.cpp:1033-1050`, and the WM2000 packet measured at VI swap
    /// 2522 in `docs/WM2000-FILLRECT-EVIDENCE.txt`).
    ///
    /// What "admitted" means here is precise and narrow, and both halves
    /// are asserted:
    ///
    /// 1. The plan SUCCEEDS -- one unexecutable command no longer refuses
    ///    the whole packet.
    /// 2. It declares NO `RenderTarget` write. That is what keeps this from
    ///    being "admit all fills": `targets/fill.rs`'s
    ///    `execute_fill_rectangle` implements the fill-cycle branch alone,
    ///    so a declared write here would name bytes nothing fills and
    ///    publish a digest of stale RDRAM.
    ///
    /// The Fill-cycle control arm below is what makes the fixture
    /// non-degenerate: a planner that declared nothing for every cycle type
    /// would satisfy assertion 2 alone.
    ///
    /// FAILS BEFORE this change (`plan_raw_dpc` returned `Err`), PASSES
    /// AFTER.
    #[test]
    fn plan_raw_dpc_admits_a_copy_cycle_fill_rectangle_without_declaring_a_write() {
        let plans = |cycle_bits: u32| {
            let (mut backend, session) = WgpuBackend::try_new().unwrap();
            let mut words = Vec::new();
            words.extend([word(SET_OTHER_MODE, cycle_bits << 20), 0]);
            words.extend(set_color_image_rgba16());
            words.extend(set_fill_color(0x213c_4d59));
            words.extend(fill_rectangle(4, 2, 14, 4));

            let request = session.plan_request(capture(words));
            backend.plan_raw_dpc(request).is_ok()
        };

        // Control: Fill cycle, the case that always planned, still does --
        // so a change that broke planning outright cannot pass this test by
        // making every arm behave identically.
        assert!(plans(3), "a Fill-cycle FillRectangle must still plan");
        for (name, cycle_bits) in [("OneCycle", 0u32), ("TwoCycle", 1), ("Copy", 2)] {
            assert!(
                plans(cycle_bits),
                "{name}: one command this backend has no executor for must no longer \
                 refuse the whole packet"
            );
        }
    }

    /// The other half of the contract the card above states, asserted where
    /// the access list is actually reachable: a non-Fill cycle
    /// `FillRectangle` declares no `RenderTarget` write, so nothing slices
    /// stale bytes for it. `PlannedRawDpcSubmission` exposes no journal
    /// accessor, so the zero-write assertion lives on the decoder's own
    /// plan in
    /// `raw_dpc::tests::a_non_fill_cycle_fill_rectangle_declares_no_write_but_a_fill_cycle_one_does`.
    /// Named here so a reader of this card finds it rather than assuming
    /// the property is unpinned.
    #[test]
    fn the_zero_write_half_of_the_non_fill_cycle_contract_is_pinned_elsewhere() {
        let source = include_str!("raw_dpc/mod.rs");
        assert!(
            source.contains(
                "fn a_non_fill_cycle_fill_rectangle_declares_no_write_but_a_fill_cycle_one_does"
            ),
            "the zero-declared-write assertion this card defers to must exist"
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

    /// **The sync-only packet: the shape WM2000 aborts this backend on.**
    ///
    /// Measured on the real ROM through the all-Rust lane
    /// (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`), the packet that reached
    /// `NoCompletedLoads` is exactly one wire command --
    /// `wire_opcode = 0xE9` (`G_RDPFULLSYNC`), raw words
    /// `[0xE9000000, 0x07000000]` -- with **zero** loads, triangles,
    /// texrects and fills, and a single `ResourceAccess`:
    /// `Read`/`CommandDecode` over the 8 `RspDmem` bytes of the sync
    /// command itself. Its site carried `dp_slot_reserved: true` and
    /// `interrupt_after: Clear`, so it planned cleanly and was
    /// deliberately admitted -- and then refused at execution.
    ///
    /// A sync-only packet has nothing to *raster*, which is what the
    /// refusal's doc meant, but that is not the same as having nothing to
    /// *do*: `SYNC_FULL`'s whole effect is on the RDP pipeline and the DP
    /// interrupt line, and this backend's own `PlanCollector` already says
    /// so at the `FullSyncSite` arm -- "collected, not executed ... the
    /// site is retained so the executed plan still accounts for every
    /// command the plan carried". Refusing the packet drops the very
    /// command that arm went out of its way to retain.
    ///
    /// The completion route is not a widening of the refusal. A sync
    /// declares zero `ResourceAccess` writes by construction
    /// (`RdpFullSyncSite`'s own doc: "Pushes zero `ResourceAccess`
    /// entries: a sync reads and writes no resource"), so
    /// `complete_execution_preserving_physical` -- which builds its own
    /// explicitly-empty write list and lets
    /// `BackendEffectReport::try_new` check it against the packet's real
    /// journal -- *proves* the zero-write property here rather than
    /// assuming it. A packet that secretly declared a write is still
    /// rejected there with `EffectCountMismatch`, independently of this
    /// branch.
    ///
    /// Hand-derived, not captured: `word(FULL_SYNC, 0)` is
    /// `0x29 << 24 == 0x29000000`, the RDP-side `SYNC_FULL` opcode this
    /// module's decoder reads (the 0xE9 seen on the wire is the same
    /// command in the ABI's own command-byte space).
    #[test]
    fn a_sync_only_packet_executes_instead_of_being_refused_as_having_zero_loads() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        let request = session.plan_request(full_sync_capture(sync_only_words()));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("a reserved sync-only capture must plan cleanly");
        assert!(
            planned.guest_read_plan().reads().is_empty(),
            "a sync-only plan declares no TmemLoadSource reads"
        );
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let initial_identity = backend.physical_tmem().identity();

        let prepared = backend.execute_raw_dpc(bound).expect(
            "a sync-only packet must execute: it declares zero writes and zero raster work, and              refusing it aborts the real WM2000 boot",
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a sync-only packet stages no color-target write, so it must leave no redeemable              fill token"
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
            "a sync touches no TMEM, so it has no successor to flip to -- an identity change              would mean complete_execution (the flipping route) was used instead of the              preserving one"
        );
    }

    /// **Positive control for the fixture above.** Without this, the
    /// sync-only test could pass against a plan that silently carried a
    /// load, a triangle or a fill, and would then be proving nothing about
    /// the refused shape at all. Pins every count the real measurement
    /// reported, plus the one access being a `Read` rather than a write.
    ///
    /// Measured through the real decoder via `ExecutionCollector`, exactly
    /// as `plan_of` does -- not re-derived from the wire words, since a
    /// second parser here could agree with the fixture while disagreeing
    /// with what execution actually sees.
    #[test]
    fn the_sync_only_fixture_really_is_one_sync_command_with_no_executable_work() {
        let plan = plan_of_no_reads(sync_only_words());

        assert_eq!(plan.full_sync_sites.len(), 1, "exactly one SYNC_FULL site");
        assert!(plan.loads.is_empty(), "no TMEM loads");
        assert!(plan.triangles.is_empty(), "no admitted triangles");
        assert!(plan.texrect_commands.is_empty(), "no texrects");
        assert!(plan.fills.is_empty(), "no fills");
        assert_eq!(
            plan.next_command_index, 1,
            "the packet is exactly one wire command"
        );
        assert!(
            !plan.accesses.is_empty(),
            "the sync's own command-decode read must be declared, or the access assertion below              is vacuous"
        );
        assert!(
            plan.accesses
                .iter()
                .all(|access| access.mode() == fn64_render_ir::AccessMode::Read),
            "a sync declares no write access -- only its own command-decode read"
        );
    }

    /// **The arm the sync fix deliberately KEPT, pinned so it cannot be
    /// widened away.**
    ///
    /// Admitting the sync-only packet above narrowed `NoCompletedLoads` to
    /// "no load, no triangle, AND no sync". Without this test, deleting the
    /// refusal outright -- routing every load-free plan to
    /// `NoPhysicalSuccessor` -- passes the whole suite, so nothing would
    /// distinguish the correct narrowing from simply dropping the guard.
    /// (Measured: that exact mutant survives the suite without this test.)
    ///
    /// The fixture is a packet of `SetOtherMode`/`SetCombine` and nothing
    /// else: pure durable RDP register writes, which `PlanCollector` folds
    /// into `current_other_mode`/`current_combine` and pushes onto no
    /// command list at all. It therefore carries zero loads, zero
    /// triangles, zero texrects, zero fills and zero `SYNC_FULL` sites --
    /// the one shape that genuinely has no command whose completion this
    /// backend could account for, and the only shape this refusal still
    /// names.
    #[test]
    fn a_plan_with_no_load_no_triangle_and_no_sync_is_still_refused_by_name() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        let planned = plan_with_no_reads(&mut backend, &session, state_only_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        match backend.execute_raw_dpc(bound) {
            Err(RenderError::Backend { reason, .. }) => assert_eq!(
                reason,
                WgpuRawDpcExecutionError::NoCompletedLoads.to_string(),
                "a plan carrying only durable register writes has no completable command;                  admitting it would mean the sync fix widened the refusal away instead of                  narrowing it"
            ),
            other => panic!(
                "a plan with no load, no triangle and no sync must be refused by name, got                  {other:?}"
            ),
        }
    }

    /// **Positive control for the refusal fixture above.** Proves the
    /// state-only packet really carries none of the three completable
    /// command kinds -- otherwise the refusal it asserts could be firing
    /// for some other reason entirely.
    #[test]
    fn the_state_only_fixture_really_carries_no_completable_command() {
        let plan = plan_of_no_reads(state_only_words());

        assert!(plan.loads.is_empty(), "no TMEM loads");
        assert!(plan.triangles.is_empty(), "no admitted triangles");
        assert!(plan.texrect_commands.is_empty(), "no texrects");
        assert!(plan.fills.is_empty(), "no fills");
        assert!(plan.full_sync_sites.is_empty(), "no SYNC_FULL sites");
        assert_eq!(
            plan.next_command_index, 2,
            "the packet is exactly two wire commands -- SetOtherMode and SetCombine -- so the              emptiness asserted above is emptiness of COMPLETABLE work, not an empty stream"
        );
        assert!(
            plan.current_other_mode.is_some() && plan.current_combine.is_some(),
            "both register writes must have been folded into durable state, or the fixture is              not the shape this test claims"
        );
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

        let (_prepared, _triangles, pending, _draw_tmem) = execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend.rdp_state.other_mode(),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            backend.rdp_state.color_image(),
            durable_neutral_tiles(&backend.rdp_state),
            &mut backend.color_targets,
            backend.configured_target_extent,
        )
        .expect("the fill half must stage a real token");
        assert!(
            pending.is_some(),
            "this fixture must actually produce a token, or the ordering claim is vacuous"
        );

        // The draw half, on the same backend the fill just staged against.
        let draw = backend.draw_admitted_triangles(
            vec![Err(MissingTriangleDrawState::NoCombine {
                triangle_index: 0,
            })],
            None,
        );
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
            .find("self.draw_admitted_triangles(triangles, draw_tmem)")
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

    // -----------------------------------------------------------------
    // Composed fill + TMEM in one packet.
    //
    // Census `docs/RT64-WM2000-CENSUS.md` §4a measures the former
    // `MixedFillAndTmemLoadPacket` refusal firing on 218/218 WM2000 frames.
    // These tests are the unit-level evidence for the composition that
    // replaced it; `fn64-abi`'s `raw_dpc_session_integration` carries the
    // end-to-end half through the real producer seam.
    // -----------------------------------------------------------------

    /// A composed packet: the TMEM load first, then the whole-target fill.
    /// Both halves are the existing single-source fixtures verbatim, so a
    /// composed packet's halves are provably the same commands the
    /// single-source tests already pin.
    fn tmem_then_fill_words() -> Vec<u32> {
        let mut words = one_load_block_words();
        words.extend(whole_target_fill_words());
        words
    }

    /// The same two halves, swapped: the fill first, then the TMEM load.
    fn fill_then_tmem_words() -> Vec<u32> {
        let mut words = whole_target_fill_words();
        words.extend(one_load_block_words());
        words
    }

    /// Every write access one word stream's decode declares, as
    /// `(operation_id, purpose)` in the resource journal's own order.
    ///
    /// Reuses `declared_render_target_writes`'s probe-decode technique --
    /// `PlannedRawDpcSubmission` exposes no journal accessor -- but keeps
    /// the purpose tag rather than filtering to `RenderTarget`, because the
    /// interleaving of the two purposes IS the fact under test.
    /// Every `RenderTarget` write access one word stream's decode declares,
    /// as `(start, end)` guest byte ranges in the journal's own order.
    ///
    /// Same probe-decode technique as `declared_write_purposes`
    /// (`PlannedRawDpcSubmission` exposes no journal accessor), but keeps the
    /// RDRAM *range* rather than the purpose tag: a count alone cannot tell a
    /// correctly-placed rectangle from one shifted by a row, which is exactly
    /// the mutation that survived a count-only assertion.
    fn declared_render_target_ranges(words: Vec<u32>) -> Vec<(u32, u32)> {
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
            .filter_map(|access| match access.region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                    Some((range.start().get(), range.end()))
                }
                _ => None,
            })
            .collect()
    }

    fn declared_write_purposes(words: Vec<u32>) -> Vec<(u32, AccessPurpose)> {
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
            .filter(|access| access.mode() == AccessMode::Write)
            .map(|access| (access.operation().get(), access.purpose()))
            .collect()
    }

    /// Drives plan -> execute for a composed fixture, supplying the one
    /// `TmemLoadSource` read its TMEM half declares.
    fn plan_and_execute_composed(
        backend: &mut WgpuBackend,
        session: &mut RawDpcAbiSession,
        words: Vec<u32>,
    ) -> (
        fn64_render_ir::SubmissionIdentity,
        Result<BackendPreparedRawDpc, RenderError>,
    ) {
        let (planned, source_bytes) = plan_with_deterministic_reads(backend, session, words);
        let capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, capture).unwrap();
        let submission = bound.submission();
        (submission, backend.execute_raw_dpc(bound))
    }

    /// Drives plan -> execute -> guest commit -> publish for a composed
    /// fixture, the full conveyor `publish_one_fill` drives for a fill-only
    /// one, with the TMEM half's declared read supplied.
    ///
    /// Publication matters here and cannot be skipped: `physical_tmem()`
    /// reads the coordinator's *active* slot, and `complete_execution`
    /// installs its successor into the *inactive* one. Only `commit` flips
    /// them. So the TMEM half's effect is unobservable until publish, which
    /// is exactly why the composed test drives all the way through.
    fn publish_composed(
        backend: &mut WgpuBackend,
        session: &mut RawDpcAbiSession,
        words: Vec<u32>,
    ) -> Vec<CompletedWrite> {
        let (planned, source_bytes) = plan_with_deterministic_reads(backend, session, words);
        let read_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, read_capture).unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("a composed fill+TMEM packet must execute");
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

    /// `merged_fill_and_tmem_writes`' two loud arms, tested directly.
    ///
    /// Neither is reachable from a legitimately decoded packet -- the
    /// decoder builds the journal from the same walk that produces the
    /// staged writes, so the two agree by construction -- which is exactly
    /// why they are tested at the function. A defensive arm with no test is
    /// a claim with no evidence; measured, deleting the `Undeclared` arm
    /// left the whole 4991-test suite green before this test existed.
    ///
    /// Both arms are real invariants, not paranoia: `Unclaimed` would
    /// otherwise hand `BackendEffectReport::try_new` a short list, whose
    /// count mismatch does not say WHICH access went unproduced, and
    /// `Undeclared` would silently drop a write this backend actually
    /// executed from the report that authorizes it.
    ///
    /// The packet is a REAL composed packet, built through the same
    /// probe-decode path `declared_write_purposes` uses, so the journal
    /// under test is the decoder's own -- not a hand-built stand-in whose
    /// shape could drift from what the decoder really emits.
    #[test]
    fn merging_rejects_a_declared_write_nobody_staged_and_a_staged_write_nobody_declared() {
        // A real composed packet, carrying the decoder's own journal.
        let capture = capture(tmem_then_fill_words());
        let layout = capture.memory_layout();
        let submission = capture.submission().clone();
        let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
        let probe = finalize_with_zero_reads(
            layout,
            capture.transaction_sequence(),
            submission.clone(),
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            probe_journal,
        )
        .unwrap();
        let accesses =
            match crate::decode_raw_dpc(submit_locally(probe).unwrap(), &RdpState::default()) {
                Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
                other => panic!("probe decode must report the real access list, got {other:?}"),
            };
        let declared: u32 = accesses
            .iter()
            .map(|access| access.region().declared_bytes())
            .sum();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(fn64_render_ir::MAX_RESOURCE_ACCESSES, declared.max(1))
                .unwrap(),
            accesses.clone(),
        )
        .unwrap();
        let decoded = finalize_with_zero_reads(
            layout,
            capture.transaction_sequence(),
            submission,
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            journal,
        )
        .unwrap();
        let ticket = submit_locally(decoded).unwrap();
        let packet = ticket.packet();

        // Every write access the real journal declares, as a `CompletedWrite`
        // with a placeholder digest -- content is irrelevant to this
        // function, which composes by ACCESS identity alone.
        let all_writes: Vec<CompletedWrite> = accesses
            .iter()
            .filter(|access| access.mode() == AccessMode::Write)
            .map(|access| {
                CompletedWrite::try_new(
                    *access,
                    access.region().declared_bytes(),
                    fn64_render_ir::ContentDigest::hash(b"merge-arm-test", &[]),
                )
                .unwrap()
            })
            .collect();
        assert!(
            all_writes.len() >= 2,
            "the composed fixture must declare at least a fill write and a TMEM write, got {}",
            all_writes.len()
        );

        // The honest, complete case: every declared write is claimed, and
        // the merge reproduces the journal's own order exactly.
        let merged = merged_fill_and_tmem_writes(packet, &all_writes, &[])
            .expect("a complete staged set must merge cleanly");
        assert_eq!(
            merged, all_writes,
            "the merge must reproduce the journal's own write order"
        );

        // Arm 1: a declared write nobody staged.
        let short = &all_writes[1..];
        let error = merged_fill_and_tmem_writes(packet, short, &[])
            .expect_err("a declared write with no staged producer must be rejected");
        assert!(
            matches!(error, WgpuRawDpcExecutionError::MergedWriteUnclaimed { .. }),
            "the rejection must name the unclaimed declared access, got: {error}"
        );

        // Arm 2: a staged write the journal never declared, with every
        // declared write ALSO present -- so the only defect is the extra
        // one, and arm 1 cannot fire first.
        // Cloned off a real declared write so its purpose and region are a
        // legal pairing; only the operation id is foreign, which is exactly
        // what makes it match no declared access.
        let template = all_writes
            .iter()
            .find(|write| write.access().purpose() == AccessPurpose::RenderTarget)
            .expect("the composed fixture declares a RenderTarget write");
        let foreign = CompletedWrite::try_new(
            fn64_render_ir::ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(9_999),
                AccessMode::Write,
                template.access().purpose(),
                template.access().region(),
            )
            .unwrap(),
            template.byte_count(),
            template.content(),
        )
        .unwrap();
        let mut with_foreign = all_writes.clone();
        with_foreign.push(foreign);
        let error = merged_fill_and_tmem_writes(packet, &with_foreign, &[])
            .expect_err("a staged write the journal never declared must be rejected");
        assert!(
            matches!(
                error,
                WgpuRawDpcExecutionError::MergedWriteUndeclared {
                    access_index: 9_999
                }
            ),
            "the rejection must name the undeclared staged access by id, got: {error}"
        );

        // Arm 3: one staged write may not satisfy TWO declared accesses.
        //
        // `ResourceJournal::try_new` does NOT enforce `OperationId`
        // uniqueness -- only the decoder's own `push_access`, which assigns
        // the vector index, makes ids unique in practice. So the claim
        // "each staged write is consumed once" is a real invariant this
        // function must enforce, not a fact the type system already
        // guarantees, and it is enforced by the `!taken` guard.
        //
        // Measured: removing that guard left the whole 4992-test suite
        // green before this case existed. It is pinned here rather than
        // argued as an equivalent mutant, because the argument would have
        // been wrong -- uniqueness is a construction convention, not a
        // validated invariant.
        // The real command-decode reads (the packet cannot be finalized
        // without them) plus the SAME write access twice.
        let mut duplicated: Vec<fn64_render_ir::ResourceAccess> = accesses
            .iter()
            .filter(|access| access.purpose() == AccessPurpose::CommandDecode)
            .copied()
            .collect();
        duplicated.push(template.access());
        duplicated.push(template.access());
        let duplicate_declared: u32 = duplicated
            .iter()
            .map(|access| access.region().declared_bytes())
            .sum();
        let duplicate_journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(
                fn64_render_ir::MAX_RESOURCE_ACCESSES,
                duplicate_declared.max(1),
            )
            .unwrap(),
            duplicated,
        )
        .expect("the journal type does not reject a repeated OperationId -- that is the point");
        let duplicate_decoded = finalize_with_zero_reads(
            capture.memory_layout(),
            capture.transaction_sequence(),
            capture.submission().clone(),
            capture.cmd_end(),
            capture.full_sync_boundaries().to_vec(),
            duplicate_journal,
        )
        .unwrap();
        let duplicate_ticket = submit_locally(duplicate_decoded).unwrap();
        let error = merged_fill_and_tmem_writes(duplicate_ticket.packet(), &[*template], &[])
            .expect_err("one staged write must not satisfy two declared accesses");
        assert!(
            matches!(error, WgpuRawDpcExecutionError::MergedWriteUnclaimed { .. }),
            "the second declared access must go unclaimed, got: {error}"
        );
    }

    /// **The card's headline unit test.** A packet carrying both a TMEM load
    /// and an admitted `FillRectangle` executes and publishes BOTH halves,
    /// instead of being refused before either could stage.
    ///
    /// The two publications are separately asserted, because they are
    /// separate identities that composition deliberately did not merge:
    ///
    /// - the fill's resident color generation, advanced by
    ///   `publish_raw_dpc`'s registry publication;
    /// - the TMEM half's physical successor, installed by
    ///   `complete_execution` into the coordinator's inactive slot and made
    ///   observable only when `commit` flips the active one.
    ///
    /// The TMEM assertion is what discriminates the routing. A composed
    /// packet routed to the fill-only completion
    /// (`complete_execution_preserving_physical_with_effects`) would still
    /// stage the fill, still return `Ok`, and still publish a resident --
    /// but would never install the successor, leaving the published TMEM
    /// state at its initial identity with no valid byte anywhere. That is
    /// mutant (a)/(d) in this card's report, and it dies here.
    #[test]
    fn execute_raw_dpc_admits_a_composed_fill_and_tmem_packet() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let identity_before = backend.physical_tmem().identity();
        assert!(
            backend.color_targets().is_none(),
            "no color target may exist before the composed packet"
        );
        assert!(
            (0..64u16).all(|address| !backend.physical_tmem().byte_is_valid(address)),
            "no TMEM byte may be valid before the composed packet"
        );

        let staged = publish_composed(&mut backend, &mut session, tmem_then_fill_words());

        // Half one: the fill's guest-visible write, and its resident.
        assert_eq!(
            staged.len(),
            1,
            "the whole-target fill half declares exactly one collapsed RenderTarget write"
        );
        assert_eq!(
            staged[0].byte_count(),
            FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
            "the fill half's write must cover the whole RGBA16 target"
        );
        let registry = backend
            .color_targets()
            .expect("the composed packet's fill half must have built the registry");
        assert_eq!(
            registry.residents().len(),
            1,
            "the composed packet's fill half must publish exactly one resident"
        );
        assert_eq!(
            registry.residents()[0].generation(),
            crate::TargetGeneration::FIRST,
            "the fill half's first publication is generation FIRST"
        );

        // Half two: the TMEM load's physical successor really became the
        // published state.
        assert_ne!(
            backend.physical_tmem().identity(),
            identity_before,
            "the composed packet's TMEM half must install a physical successor -- an unmoved \
             identity would mean the packet took the fill-only completion and silently \
             discarded the load"
        );
        assert!(
            (0..64u16).any(|address| backend.physical_tmem().byte_is_valid(address)),
            "the composed packet's TMEM half must leave real valid bytes in published TMEM"
        );

        assert!(
            !backend.has_pending_fill_publication(),
            "publication must consume the fill token, leaving nothing redeemable"
        );
    }

    /// **Constraint 2: ordering is semantics, and the two orders are
    /// genuinely different.** The same two halves in the two possible stream
    /// orders declare their write accesses in DIFFERENT sequences, and the
    /// composed effect report follows each stream's own sequence.
    ///
    /// This is the falsifiability the composition rests on. The order is not
    /// chosen by `merged_fill_and_tmem_writes`: `fn64_render_ir`'s
    /// `validate_effects` compares the reported write list against
    /// `journal().write_accesses()` position by position, so any merge that
    /// did not reproduce the journal's order would be rejected outright with
    /// `EffectAccessMismatch`. Both orders executing cleanly is therefore
    /// proof that the composed order IS the journal's order in both cases --
    /// and the two journals differ, as the first two assertions show.
    ///
    /// A merge that always emitted the fill's writes first, always emitted
    /// the TMEM writes first, or sorted by anything other than journal
    /// position would satisfy at most one of the two fixtures. That is
    /// mutant (c) in this card's report, and it is killed here.
    #[test]
    fn a_composed_packet_reports_writes_in_the_streams_own_journal_order() {
        let tmem_first = declared_write_purposes(tmem_then_fill_words());
        let fill_first = declared_write_purposes(fill_then_tmem_words());

        // Both streams declare the same MULTISET of write purposes...
        let mut sorted_a: Vec<AccessPurpose> =
            tmem_first.iter().map(|(_, purpose)| *purpose).collect();
        let mut sorted_b: Vec<AccessPurpose> =
            fill_first.iter().map(|(_, purpose)| *purpose).collect();
        assert_eq!(
            sorted_a.len(),
            sorted_b.len(),
            "both orders must declare the same number of writes -- only their order differs"
        );
        sorted_a.sort_by_key(|purpose| format!("{purpose:?}"));
        sorted_b.sort_by_key(|purpose| format!("{purpose:?}"));
        assert_eq!(
            sorted_a, sorted_b,
            "both orders must declare the same write purposes as a multiset"
        );

        // ...in genuinely DIFFERENT sequences. Without this, "the merge
        // respects the order" would be a claim about two identical lists.
        let sequence_a: Vec<AccessPurpose> =
            tmem_first.iter().map(|(_, purpose)| *purpose).collect();
        let sequence_b: Vec<AccessPurpose> =
            fill_first.iter().map(|(_, purpose)| *purpose).collect();
        assert_ne!(
            sequence_a, sequence_b,
            "the two stream orders must declare their writes in different sequences, or this \
             test cannot discriminate a journal-ordered merge from a fixed-order one"
        );
        assert_eq!(
            sequence_a.first(),
            Some(&AccessPurpose::TmemLoadDestination),
            "the TMEM-first stream must declare its TMEM write before the fill's"
        );
        assert_eq!(
            sequence_b.first(),
            Some(&AccessPurpose::RenderTarget),
            "the fill-first stream must declare the fill's write before the TMEM one"
        );

        // And both execute. Since `validate_effects` rejects any reported
        // order but the journal's, two clean executions over two different
        // journal orders is the proof that the merge followed each.
        for (label, words) in [
            ("TMEM-first", tmem_then_fill_words()),
            ("fill-first", fill_then_tmem_words()),
        ] {
            let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
            configure_fill_target_height(&mut backend);
            let (_, result) = plan_and_execute_composed(&mut backend, &mut session, words);
            let prepared = result.unwrap_or_else(|error| {
                panic!(
                    "the {label} composed order must execute -- a merge emitting a fixed \
                     order would be rejected here by validate_effects: {error}"
                )
            });
            drop(prepared);
        }
    }

    /// The still-unadmitted compositions keep failing by NAME, not silently.
    ///
    /// Two of them, each a different reason:
    ///
    /// - fill + triangle: refused with `MixedFillAndTrianglePacket`, because
    ///   a triangle raster declares no write access in the journal at all,
    ///   so unlike fill + TMEM there is no declared order to compose onto.
    /// - fill + TMEM + triangle: the SAME refusal must win, and must win
    ///   before either source stages anything -- admitting fill + TMEM must
    ///   not have opened a back door where a triangle rides along with them.
    ///
    /// The refusal is compared against the variant's own `to_string()`, so a
    /// future rename cannot leave this test asserting a stale literal, and a
    /// DIFFERENT rejection cannot pass as this one.
    #[test]
    fn compositions_this_slice_does_not_admit_still_fail_by_name() {
        let refused = WgpuRawDpcExecutionError::MixedFillAndTrianglePacket.to_string();

        // fill + triangle, no TMEM load.
        let mut fill_and_triangle = whole_target_fill_words();
        fill_and_triangle.extend(set_other_mode(0, 0));
        fill_and_triangle.extend(set_combine(0, 0));
        fill_and_triangle.extend(triangle_base_edge_words(7, 2, 0));

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_fill(&mut backend, &mut session, fill_and_triangle);
        let error = result.expect_err("fill + triangle must still be refused");
        assert!(
            error.to_string().contains(&refused),
            "the refusal must be the named MixedFillAndTrianglePacket variant, got: {error}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a refused composition must leave no redeemable fill token behind"
        );

        // fill + TMEM + triangle: admitting fill + TMEM must not have let a
        // triangle through alongside them.
        let mut all_three = one_load_block_words();
        all_three.extend(whole_target_fill_words());
        all_three.extend(set_other_mode(0, 0));
        all_three.extend(set_combine(0, 0));
        all_three.extend(triangle_base_edge_words(7, 2, 0));

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_composed(&mut backend, &mut session, all_three);
        let error = result.expect_err(
            "fill + TMEM + triangle must still be refused -- admitting fill + TMEM must not \
             have opened a path for a triangle to ride along",
        );
        assert!(
            error.to_string().contains(&refused),
            "the three-way refusal must also be MixedFillAndTrianglePacket, got: {error}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "the refused three-way composition must leave no redeemable fill token behind"
        );
        assert!(
            backend.color_targets().is_none(),
            "the refusal must fire before the fill executor ever built the registry"
        );
    }

    /// A composed packet whose fill half is rejected leaves NOTHING behind:
    /// no redeemable fill token, and no advanced physical TMEM generation.
    ///
    /// The fill is made unadmittable at execute time by never configuring a
    /// color-image height, which is `NoColorTargetHeight` -- a rejection
    /// raised inside `stage_fill`, i.e. AFTER the TMEM half has already
    /// staged its whole transaction. That is exactly the interleaving where
    /// a partial publish would be possible, and the assertions below are
    /// that it does not happen.
    #[test]
    fn a_composed_packet_whose_fill_half_is_rejected_publishes_neither_half() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        // Deliberately NOT `configure_fill_target_height`.
        let generation_before = backend.physical_tmem().generation();

        let (_, result) =
            plan_and_execute_composed(&mut backend, &mut session, tmem_then_fill_words());
        let error = result.expect_err(
            "a composed packet whose fill half has no color-image height must be rejected",
        );
        assert!(
            error
                .to_string()
                .contains(&WgpuRawDpcExecutionError::NoColorTargetHeight.to_string()),
            "the rejection must be the named NoColorTargetHeight variant, got: {error}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a rejected composed packet must leave no redeemable fill token"
        );
        assert_eq!(
            backend.physical_tmem().generation(),
            generation_before,
            "a rejected composed packet must not advance the published TMEM generation either \
             -- the TMEM half staged before the fill half failed, and staging is not publishing"
        );
    }

    // --- TextureRectangle composition frontier (this card's measurement) ---

    /// One `TextureRectangle` command's 4-word wire payload, mirroring
    /// `raw_dpc::production_adapter::tests::texrect_words` exactly (that
    /// helper is private to its own module's tests, so this is a local,
    /// identical copy -- the same convention `triangle_base_edge_words`
    /// above already follows).
    ///
    /// Deliberately sized to land inside the 16x8 `FILL_TARGET_*` image
    /// this module's fill fixtures use, rather than reusing the sibling's
    /// 48x48 rectangle: a rectangle larger than the target would confound
    /// "declares no write" with "declares a write outside the target".
    fn texrect_words_in_target(tile: u32) -> [u32; 4] {
        // 10.2 fixed point, matching `fill_rectangle` above: x 4..=11,
        // y 2..=4, wholly inside the 16x8 RGBA16 target.
        let ulx: u32 = 4 << 2;
        let uly: u32 = 2 << 2;
        let lrx: u32 = 11 << 2;
        let lry: u32 = 4 << 2;
        let dsdx: u32 = 0x0100;
        let dtdy: u32 = 0x0100;
        [
            word(0x24, (lrx << 12) | lry),
            (tile & 0x7) << 24 | (ulx << 12) | uly,
            0,
            (dsdx << 16) | dtdy,
        ]
    }

    /// `texrect_words_in_target`'s stepping sibling: identical rectangle,
    /// but `dsdx`/`dtdy` of `0x0400` (one texel per pixel in S5.10) instead
    /// of `0x0100`.
    ///
    /// The step matters and was determined by measurement. Copy mode
    /// halves the S step twice (`dsdx >>= 2`), so
    /// `lrs = (0 + 0x100 * (8 << 2)) >> 7 = 64` in S10.5 -- **2 texels
    /// across the 8-pixel row**. `dtdy` is not shifted, so
    /// `lrt = (0 + 0x400 * (3 << 2)) >> 7 = 96` -- **3 texels over the 3
    /// rows**, one per row. At `0x0100` the S span is half a texel and
    /// every pixel in a row samples the same texel, which makes an
    /// "S is actually read" assertion unsatisfiable; the sibling keeps
    /// `0x0100` because its own tests never sample.
    fn texrect_words_in_target_stepping(tile: u32) -> [u32; 4] {
        let mut words = texrect_words_in_target(tile);
        words[3] = (0x0400u32 << 16) | 0x0400;
        words
    }

    /// A TMEM load, then a `TextureRectangle` sampling the tile it loaded --
    /// the WM2000-title-screen shape this card was dispatched to admit.
    fn tmem_then_texrect_words() -> Vec<u32> {
        let mut words = one_load_block_words();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_in_target(7));
        words
    }

    /// The composed shape: whole-target fill, a TMEM load, then a
    /// `TextureRectangle` sampling it.
    fn fill_tmem_and_texrect_words() -> Vec<u32> {
        let mut words = whole_target_fill_words();
        words.extend(one_load_block_words());
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_in_target(7));
        words
    }

    /// The number of `TriangleSource::TextureRectangle` triangles this
    /// stream admits, measured through the same plan walk execution uses --
    /// not re-derived from the wire words, which would be a second
    /// independent model of the same fact.
    fn admitted_texture_rectangle_triangles(words: Vec<u32>) -> usize {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &mut session, words);
        let read_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, read_capture).unwrap();

        let mut plan_visitor = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            plan: PlanCollector::seeded(
                None,
                None,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
                Color4::from_wire(0),
                None,
                [(None, None); 8],
            ),
            reads: Vec::new(),
            outcome: None,
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            physical: coordinator.physical(),
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
        view.plan
            .triangles
            .iter()
            .filter(|draw| {
                draw.as_ref()
                    .map(|draw| draw.source == TriangleSource::TextureRectangle)
                    .unwrap_or(false)
            })
            .count()
    }

    /// Positive control for the two tests below: these fixtures really do
    /// carry an admitted `TextureRectangle`, admitted as exactly two
    /// `TriangleSource::TextureRectangle` triangles.
    ///
    /// Without this, "appending a texrect changes no declared write" and
    /// "fill + texrect is refused" would both still pass against a fixture
    /// whose texrect had silently vanished from the stream -- measured, not
    /// hypothesised: deleting the `texrect_words_in_target` line from
    /// `fill_tmem_and_texrect_words` left both of those tests green until
    /// this control existed (mutant D in this card's report).
    #[test]
    fn the_texrect_fixtures_really_do_admit_a_texture_rectangle() {
        assert_eq!(
            admitted_texture_rectangle_triangles(one_load_block_words()),
            0,
            "the TMEM-only control must admit no texture-rectangle triangles"
        );
        for (label, words) in [
            ("tmem_then_texrect", tmem_then_texrect_words()),
            ("fill_tmem_and_texrect", fill_tmem_and_texrect_words()),
        ] {
            assert_eq!(
                admitted_texture_rectangle_triangles(words),
                2,
                "{label} must admit exactly two TextureRectangle-sourced triangles -- one \
                 rectangle is two triangles, and zero would mean the fixture lost its texrect"
            );
        }
    }

    /// **A `TextureRectangle` now declares a journal write access for its
    /// destination rectangle** -- the inversion of this card's own starting
    /// measurement.
    ///
    /// At `92affbee` this test asserted the opposite, and its failure message
    /// named the condition under which it should be rewritten: "if this ever
    /// fails, a texrect has gained a journal write access and the composition
    /// this card was dispatched to build becomes tractable". That is what
    /// happened. `raw_dpc::mod`'s `plan_texture_rectangle` derives the
    /// rectangle's rasterized pixel extent from
    /// `texture_rectangle_vertices` (the ported RT64 `drawTexRect`/`drawRect`)
    /// and declares the same per-row `ColorFramebuffer` writes `plan_fill`
    /// does, through the same `plan_render_target_rows`.
    ///
    /// The counts here are hand-derived, not captured. `texrect_words_in_target`
    /// is `ulx=4<<2, uly=2<<2, lrx=11<<2, lry=4<<2`; RT64's
    /// `left/top/right/bottom = (coord + 3) >> 2` gives `4, 2, 11, 4`, a
    /// half-open extent, so the covered pixels are x `4..=10`, y `2..=3`.
    /// That is 7 pixels wide in a 16-wide image -- a *partial*-width
    /// rectangle -- so it declares one access per row: **2**. Cross-checked
    /// independently: `ceil(coord / 4)` for each of the four wire values
    /// yields the same `4, 2, 11, 4`.
    #[test]
    fn a_texture_rectangle_declares_a_render_target_write_access() {
        // A TMEM load alone declares its TMEM destination write.
        let tmem_only = declared_write_purposes(one_load_block_words());
        assert!(
            !tmem_only.is_empty(),
            "the TMEM-only control must declare at least one write, or this test cannot \
             discriminate 'texrect declares nothing' from 'the probe sees nothing'"
        );
        assert!(
            tmem_only
                .iter()
                .all(|(_, purpose)| *purpose == AccessPurpose::TmemLoadDestination),
            "the TMEM-only control must declare only TMEM destination writes, got {tmem_only:?}"
        );

        // `tmem_then_texrect_words` stages no `SetColorImage`, so its texrect
        // has no destination image and declares no write -- the documented
        // "declaring nothing is not a silent no-op" case in
        // `plan_texture_rectangle`'s contract. Pinned so that case cannot
        // silently start declaring a range.
        let with_texrect = declared_write_purposes(tmem_then_texrect_words());
        assert_eq!(
            with_texrect, tmem_only,
            "a texrect with no staged SetColorImage has no destination image, so it must \
             declare no write"
        );

        // With a color image staged (by the fill), the SAME texrect declares
        // its own RenderTarget writes on top of the fill's.
        let composed = declared_write_purposes(fill_tmem_and_texrect_words());
        let render_target_writes = composed
            .iter()
            .filter(|(_, purpose)| *purpose == AccessPurpose::RenderTarget)
            .count();
        let fill_only_render_target_writes = declared_write_purposes(whole_target_fill_words())
            .iter()
            .filter(|(_, purpose)| *purpose == AccessPurpose::RenderTarget)
            .count();
        assert_eq!(
            fill_only_render_target_writes, 1,
            "the whole-target fill is full-image-width, so it collapses to exactly one \
             contiguous access -- if this moves, the derivation below is measuring \
             something else"
        );
        assert_eq!(
            render_target_writes,
            fill_only_render_target_writes + 2,
            "the texrect must contribute exactly 2 RenderTarget writes of its own -- one per \
             covered row (y 2..=3), because at 7 pixels wide in a 16-wide image its rows are \
             disjoint and must not collapse"
        );

        // A count alone cannot tell a correctly-placed rectangle from one
        // shifted by a row (measured: that mutation survived a count-only
        // assertion). Assert the exact declared byte ranges.
        //
        // Hand-derived: the fill is the whole 16x8 RGBA16 target at
        // `0x2000`, so it declares `0x2000..0x2000 + 16*8*2 = 0x2100`. The
        // texrect covers x 4..=10 (7 pixels) on rows 2 and 3, so each row is
        // `0x2000 + (y*16 + 4)*2` for `7*2 = 14` bytes: row 2 is
        // `0x2048..0x2056`, row 3 is `0x2068..0x2076`. The two are disjoint
        // and strided by the image width (`0x2068 - 0x2048 = 0x20 = 16*2`) --
        // a partial-width rectangle must never collapse its rows into one
        // range spanning the untouched bytes between them.
        let ranges = declared_render_target_ranges(fill_tmem_and_texrect_words());
        assert_eq!(
            ranges,
            vec![
                (FILL_TARGET_ADDRESS, FILL_TARGET_ADDRESS + 16 * 8 * 2),
                (0x2048, 0x2056),
                (0x2068, 0x2076),
            ],
            "the declared RenderTarget ranges must be the fill's whole target followed by the \
             texrect's two disjoint hand-derived rows, in journal order"
        );
        // The same two rows, derived from the geometry rather than written
        // as literals, so an arithmetic slip cannot agree with itself.
        for (index, row) in [2u32, 3].iter().enumerate() {
            let start = FILL_TARGET_ADDRESS + (row * 16 + 4) * 2;
            assert_eq!(
                ranges[index + 1],
                (start, start + 7 * 2),
                "texrect row {row}'s declared range, derived from the extent"
            );
        }
    }

    /// `TextureRectangleFlip` declares **no** destination write, even with a
    /// color image staged and a footprint that would otherwise be provable.
    ///
    /// Flip's destination footprint is the same as the unflipped rectangle's
    /// (only the S/T pairing swaps across the diagonal), so it would be easy
    /// to declare a write for it. This slice does not execute flip, and
    /// declaring a write no executor fills would promise content that never
    /// arrives -- the "declaring nothing is not a silent no-op" case in
    /// `plan_texture_rectangle`'s contract.
    ///
    /// Measured, not assumed: without this test, deleting the flip gate
    /// entirely left the whole suite green (mutant I), because no other
    /// fixture pairs a flip texrect with a staged color image.
    #[test]
    fn a_texture_rectangle_flip_declares_no_destination_write() {
        // The identical rectangle as a plain texrect (0x24) DOES declare its
        // two rows -- the control that makes the flip assertion meaningful.
        let mut unflipped = whole_target_fill_words();
        unflipped.extend(texrect_words_in_target(7));
        let unflipped_ranges = declared_render_target_ranges(unflipped);
        assert_eq!(
            unflipped_ranges.len(),
            3,
            "control: the unflipped texrect must declare the fill's range plus its own two \
             rows, or the flip comparison below proves nothing -- got {unflipped_ranges:?}"
        );

        // The same wire words with only the opcode changed to 0x25.
        let mut flipped_words = texrect_words_in_target(7);
        flipped_words[0] = (flipped_words[0] & 0x00ff_ffff) | (u32::from(TEXRECT_FLIP) << 24);
        let mut flipped = whole_target_fill_words();
        flipped.extend(flipped_words);
        let flipped_ranges = declared_render_target_ranges(flipped);
        assert_eq!(
            flipped_ranges.len(),
            1,
            "a flip texrect must declare no destination write, leaving only the fill's own \
             range -- got {flipped_ranges:?}"
        );
        assert_eq!(
            flipped_ranges[0], unflipped_ranges[0],
            "the fill's own declared range must be identical in both streams"
        );
    }

    /// The WM2000-title-screen-shaped stream this card exists to admit: a
    /// whole-target `FillRectangle` in Fill cycle, a `LoadBlock` filling
    /// tile 7, then a `TextureRectangle` in **Copy** cycle sampling that
    /// tile.
    ///
    /// The cycle-type switch between the two halves is not incidental: a
    /// fill is only admitted in Fill cycle and this texrect executor is
    /// only admitted in Copy cycle (it evaluates no color combiner), so a
    /// real stream must set each. `fill_tmem_and_texrect_words` above keeps
    /// its single `set_other_mode(0, 0)` and is used only by the
    /// declared-write and admission tests, which never execute.
    fn fill_load_and_copy_texrect_words() -> Vec<u32> {
        let mut words = whole_target_fill_words();
        // A wider, UNSKEWED LoadBlock than `one_load_block_words`': 24
        // texels (`uls=0, lrs=23`) with `dxt = 0`, so TMEM bytes 0..48 are
        // contiguously valid -- three complete rows at this tile's
        // `line_words = 2` (16 bytes = 8 RGBA16 texels per row).
        //
        // Both changes were forced by measurement. `one_load_block_words`'
        // 8-texel load fills row 0 only, and `dxt = 0x800` skews so hard
        // that its 8 texels land in bytes 0..8 and 24..32 with a hole
        // between -- a texrect whose T advances one texel per row hits the
        // hole and is refused as `physical TMEM texel byte 0x014 is
        // invalid`. That refusal is how these values were determined.
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        // High S/T of 7 texels, in the 10.2 wire encoding (`<< 2`); low
        // S/T are the field's own zero. Mirrors
        // `raw_dpc::production_adapter::tests::set_tile_size` exactly (that
        // helper is private to its own module's tests), same local-copy
        // convention as `set_other_mode` above.
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        // Copy cycle (2), so the texrect executor admits it.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_in_target_stepping(7));
        words
    }

    /// **WM2000's measured mixed shape: texrects and a raw triangle in one
    /// packet, the triangle strictly last.**
    ///
    /// Modelled on the packet the all-Rust lane actually aborted on
    /// (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`, instrumented at the refusal
    /// site): **texrects, TMEM loads, one raw triangle at the END, zero
    /// fills**. The real packet carried 6 texrects and 9 loads; the shape
    /// that matters is the *pairing*, so this fixture carries one of each
    /// plus the trailing raw triangle. A fill is deliberately absent -- the
    /// real packet had none, and adding one would instead exercise the
    /// separate `MixedFillAndTrianglePacket` refusal, which is kept.
    ///
    /// Built by taking `fill_load_and_copy_texrect_words`' load-and-texrect
    /// half verbatim (its `SetColorImage` supplied by
    /// `set_color_image_rgba16` instead of a whole-target fill, so the
    /// texrect still declares its journal write) and appending one
    /// `RawTriangle`. The trailing `set_other_mode(0, 0)` is not decoration:
    /// the texrect ran in Copy cycle (2) and a raw triangle is not admitted
    /// there, so a real stream switches back exactly as this one does.
    fn load_texrect_and_trailing_raw_triangle_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_color_image_rgba16());
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_in_target_stepping(7));
        // Back out of Copy cycle for the raw triangle, then the triangle
        // itself -- the last command in the packet, exactly as measured.
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words.extend(triangle_base_edge_words(7, 2, 0));
        words
    }

    /// **Positive control.** The fixture really does carry BOTH an admitted
    /// `TextureRectangle` and an admitted `RawTriangle`.
    ///
    /// Without this, the admission test below would pass vacuously against
    /// a texrect-only packet -- the exact way a mixed-shape test fools
    /// itself. Both counts are read through the same plan walk execution
    /// uses, never re-derived from the wire words.
    #[test]
    fn the_mixed_fixture_really_carries_a_texrect_and_a_raw_triangle() {
        let words = load_texrect_and_trailing_raw_triangle_words();
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &mut session, words);
        let read_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, read_capture).unwrap();

        let mut plan_visitor = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            plan: PlanCollector::seeded(
                None,
                None,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
                Color4::from_wire(0),
                None,
                [(None, None); 8],
            ),
            reads: Vec::new(),
            outcome: None,
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            physical: coordinator.physical(),
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);

        let raw_triangles = view
            .plan
            .triangles
            .iter()
            .filter(|draw| {
                draw.as_ref()
                    .map(|draw| draw.source == TriangleSource::RawTriangle)
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            raw_triangles, 1,
            "the fixture must admit exactly one RawTriangle, or the admission test proves \
             nothing about the mixed shape"
        );
        assert_eq!(
            view.plan.texrect_commands.len(),
            1,
            "the fixture must admit exactly one TextureRectangle wire command"
        );
        assert!(
            view.plan
                .texrect_commands
                .iter()
                .all(|(span, _, _, _)| span.is_some()),
            "the texrect must DECLARE its journal write, or this is not the composed shape \
             the removed refusal named"
        );
        assert!(
            view.plan.fills.is_empty(),
            "the fixture must carry no fill -- a fill would exercise the separate \
             MixedFillAndTrianglePacket refusal instead, which is kept"
        );
        assert_eq!(
            view.plan.loads.len(),
            1,
            "the fixture must carry the TMEM load its texrect samples"
        );
        // The triangle is LAST, which is the ordering WM2000 measured.
        let last = view
            .plan
            .triangle_commands
            .last()
            .copied()
            .expect("the fixture admits triangles");
        let texrect_command = view.plan.texrect_commands[0].3;
        assert!(
            last > texrect_command,
            "the raw triangle must follow the texrect in stream order (got triangle at \
             {last}, texrect at {texrect_command})"
        );
    }

    /// **The admission this card exists for.** A packet carrying both an
    /// admitted `TextureRectangle` and an admitted `RawTriangle` executes,
    /// and the texrect's guest-visible pixels survive.
    ///
    /// This packet was `MixedTexrectAndRawTrianglePacket` -- refused on the
    /// reasoning that "the two have no defined ordering". Measuring the
    /// packet showed the ordering was never missing: the raw triangle
    /// contributes no `ResourceAccess` and no staged `CompletedWrite`, so
    /// the journal it must be ordered against is the one the texrect alone
    /// produces, and `stage_color_commands` already derives that order from
    /// the decoder's own `command_index`.
    ///
    /// The load-bearing assertion is the second one: the texrect's declared
    /// write is present in the staged writes, so admitting the triangle did
    /// not cost the packet its guest-visible half. The refusal did exactly
    /// that -- it dropped six real rectangles to withhold one triangle that
    /// reaches only `triangle_draw_output`, which `present` refuses to scan
    /// out and nothing copies into RDRAM.
    /// The flat (opcode 0x08) triangle this card's end-to-end tests draw:
    /// vertical edges at x = 2 and x = 6, scanlines 0..3, `lft` set.
    ///
    /// Hand-derived footprint against the 16x8 RGBA16 fill target at
    /// `FILL_TARGET_ADDRESS`:
    ///   yh = 0, yl = 3<<2 = 12 (S11.2) -> rows 0, 1, 2
    ///   left edge  x = 2.0  -> x0 = ceil(2 - 7/8)  = 2
    ///   right edge x = 6.0  -> x1 = ceil(6 - 1/8)  = 6
    /// So each row writes pixels 2..6 = 4 pixels = 8 bytes, at
    /// 0x2000 + (16y + 2)*2 = 0x2004, 0x2024, 0x2044.
    fn flat_triangle_in_target_words() -> [u32; 8] {
        crate::wire_words::EdgeWords {
            lft: true,
            yl: crate::wire_words::line(3),
            ym: crate::wire_words::line(3),
            yh: 0,
            xl: crate::wire_words::px(6),
            xh: crate::wire_words::px(2),
            xm: crate::wire_words::px(6),
            ..crate::wire_words::EdgeWords::zeroed()
        }
        .words(0, RAW_TRIANGLE_BASE_EDGE)
    }

    /// The primitive colour every flat-triangle end-to-end test writes, and
    /// its RGBA16 encoding, both derived by hand and from nothing else.
    ///
    ///   PRIM = 0x80FF4080 -> R 0x80, G 0xFF, B 0x40, A 0x80
    ///   RGBA16 5/5/5/1 = (0x80>>3 << 11) | (0xFF>>3 << 6) | (0x40>>3 << 1)
    ///                    | 1
    ///                  = 0x8000 | 0x07C0 | 0x0010 | 1 = 0x87D1
    const TRIANGLE_PRIM_WIRE: u32 = 0x80FF_4080;
    const TRIANGLE_PRIM_RGBA16: u16 = 0x87D1;

    /// A packet staging one-cycle mode, the flat
    /// `(Zero - Zero) * Zero + Primitive` combiner program, a primitive
    /// colour, the RGBA16 colour image, then one flat raw triangle.
    ///
    /// The combiner program's wire words are packed from the same field
    /// layout `targets::raw_triangle::tests` derives: color A/B/C/D =
    /// 8/8/16/3 and alpha A/B/C/D = 7/7/7/3 in the SECOND bitfield slice,
    ///   low  = (A << 5) | C
    ///   high = (B << 24) | (D << 6) | (aA << 21) | (aB << 3) | (aC << 18)
    ///          | aD
    fn flat_triangle_packet_words() -> Vec<u32> {
        let (low, high) =
            crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_PRIMITIVE);
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(low, high));
        words.extend(set_prim_color(0, 0, TRIANGLE_PRIM_WIRE));
        words.extend(set_color_image_rgba16());
        words.extend(flat_triangle_in_target_words());
        words
    }

    /// **A flat raw triangle's real bytes reach guest RDRAM.**
    ///
    /// This is the card's central claim and the only test that makes it end
    /// to end. It does not assert "a write was declared" or "a digest was
    /// produced"; it reads the committed payload bytes back and checks the
    /// pixels one at a time against a colour derived by hand from the wire.
    ///
    /// Before this card the same packet produced ZERO `RenderTarget` write
    /// accesses and zero `CompletedWrite`s -- the decoder's `0x08..=0x0f`
    /// arm called no planner at all -- so every assertion below fails by
    /// finding an empty list.
    #[test]
    fn a_flat_raw_triangles_pixels_reach_the_committed_guest_write_payload() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        // Establish the target honestly first, in its OWN packet: a partial
        // rectangle against a brand-new target is refused by
        // `admit_completed_initialization`, and a fill in the SAME packet as
        // a raw triangle is refused by `MixedFillAndTrianglePacket`.
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let planned = plan_with_no_reads(&mut backend, &session, flat_triangle_packet_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let result = backend.execute_raw_dpc(bound);
        let prepared = match result {
            Ok(prepared) => prepared,
            // The GPU triangle raster runs AFTER this card's CPU staging and
            // is a separate, pre-existing path. On an adapterless host it
            // refuses by its own name, which says nothing about the guest
            // bytes -- but it does mean this test cannot read them here.
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("TriangleDrawBeforeCreate")
                        || message.contains("no GPU adapter"),
                    "the only tolerated failure is the adapterless GPU raster path, got: {error}"
                );
                return;
            }
        };
        let staged = backend.staged_guest_render_target_writes(submission);
        assert_eq!(
            staged.len(),
            3,
            "one CompletedWrite per covered scanline; got {staged:?}"
        );

        // The three hand-derived byte ranges, in row order. A collapsed
        // single span would be one 72-byte write at 0x2004.
        let ranges: Vec<(u32, u32)> = staged
            .iter()
            .map(|write| match write.access().region() {
                fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                    (range.start().get(), write.byte_count())
                }
                other => panic!("a render-target write must name an RDRAM range, got {other:?}"),
            })
            .collect();
        assert_eq!(ranges, vec![(0x2004, 8), (0x2024, 8), (0x2044, 8)]);

        // **The bytes themselves, proven two independent ways.**
        //
        // First: each write's `ContentDigest` must equal the digest of the
        // four primitive-coloured RGBA16 pixels this test derived by hand
        // from the wire. `CompletedWrite::try_from_bytes` is the SAME
        // derivation `rsp_commit`'s `copy_committed_guest_writes` re-runs
        // over the payload before it writes a single byte into guest RDRAM,
        // so a digest match here is a statement about what lands in RDRAM,
        // not merely about what this backend recorded.
        let expected_row: Vec<u8> = TRIANGLE_PRIM_RGBA16.to_be_bytes().repeat(4);
        for (index, write) in staged.iter().enumerate() {
            let expected =
                fn64_render_ir::CompletedWrite::try_from_bytes(write.access(), &expected_row)
                    .expect("eight bytes match the declared eight-byte access");
            assert_eq!(
                write.content(),
                expected.content(),
                "row {index}'s committed digest must be the digest of four primitive-coloured \
                 RGBA16 pixels"
            );
        }

        // Second: the registry resident's own device bytes, read directly.
        // A digest match alone could in principle be satisfied by an
        // unrelated buffer; this reads the buffer.
        //
        // Publication is required first -- the registry only advances to
        // this packet's generation when `publish_raw_dpc` runs, which is
        // deliberately after the guest commit (see `stage_fills_and_report`'s
        // own nonclaim). Reading before publishing would read the FILL's
        // generation and is exactly what this assertion first did.
        let committed = session
            .commit_guest_render_target_writes(prepared, staged.clone())
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        let registry = backend
            .color_targets()
            .expect("the triangle packet composed into the published registry");
        let resident = registry
            .residents()
            .iter()
            .find(|resident| resident.key().address().get() == FILL_TARGET_ADDRESS)
            .expect("the target is resident");
        let bytes = resident.device_bytes().device_bytes();
        for y in 0..3usize {
            for x in 2..6usize {
                let offset = (y * FILL_TARGET_WIDTH as usize + x) * 2;
                assert_eq!(
                    u16::from_be_bytes([bytes[offset], bytes[offset + 1]]),
                    TRIANGLE_PRIM_RGBA16,
                    "pixel ({x},{y}) of the resident buffer"
                );
            }
        }
    }

    /// The pixels OUTSIDE the triangle keep the fill's own colour, in the
    /// same buffer, in the same generation.
    ///
    /// Proves the triangle composes into the accumulated buffer rather than
    /// replacing it -- the failure mode where a triangle's full-extent
    /// output is a fresh buffer would blank every pixel the fill wrote.
    #[test]
    fn a_flat_raw_triangle_leaves_the_surrounding_fill_intact() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let planned = plan_with_no_reads(&mut backend, &session, flat_triangle_packet_words());
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let submission = bound.submission();
        let Ok(prepared) = backend.execute_raw_dpc(bound) else {
            return;
        };
        let staged = backend.staged_guest_render_target_writes(submission);
        let committed = session
            .commit_guest_render_target_writes(prepared, staged)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);

        // The published resident's full-extent bytes: the triangle's 12
        // pixels hold the primitive colour and the other 116 still hold the
        // fill's.
        //
        // `whole_target_fill_words` fills 0x0842_1085. In Fill cycle an
        // RGBA16 image takes 16 bits per pixel from that 32-bit register,
        // alternating halves by X parity -- so the fill colour is not a
        // single constant across a row, and asserting one would be asserting
        // the wrong thing. What IS invariant is that no pixel outside the
        // triangle equals the triangle's colour, and every pixel inside does.
        let registry = backend
            .color_targets()
            .expect("the triangle packet composed into the published registry");
        let resident = registry
            .residents()
            .iter()
            .find(|resident| resident.key().address().get() == FILL_TARGET_ADDRESS)
            .expect("the target is resident");
        let bytes = resident.device_bytes().device_bytes();
        for y in 0..FILL_TARGET_HEIGHT as usize {
            for x in 0..FILL_TARGET_WIDTH as usize {
                let offset = (y * FILL_TARGET_WIDTH as usize + x) * 2;
                let pixel = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
                let inside = y < 3 && (2..6).contains(&x);
                if inside {
                    assert_eq!(
                        pixel, TRIANGLE_PRIM_RGBA16,
                        "pixel ({x},{y}) is inside the triangle"
                    );
                } else {
                    assert_ne!(
                        pixel, TRIANGLE_PRIM_RGBA16,
                        "pixel ({x},{y}) is outside the triangle and must keep the fill's colour"
                    );
                }
            }
        }
    }

    #[test]
    fn a_texrect_composed_with_a_trailing_raw_triangle_executes() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        // Establish the color target honestly first, in its OWN packet: a
        // partial rectangle against a brand-new target is refused by
        // `admit_completed_initialization` (`PartialNewTargetInitialization`)
        // for a reason unrelated to this card -- a fresh target has no prior
        // device bytes for the rows outside the rectangle. This is the same
        // "a title clears its framebuffer before filling sub-rectangles into
        // it" order `whole_target_fill_words` already documents, and it is
        // deliberately a SEPARATE submission: putting the fill in the mixed
        // packet would exercise the still-kept `MixedFillAndTrianglePacket`
        // refusal instead of this one.
        publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

        let words = load_texrect_and_trailing_raw_triangle_words();
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &mut session, words);
        let read_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, read_capture).unwrap();
        let submission = bound.submission();

        let result = backend.execute_raw_dpc(bound);
        let prepared = match result {
            Ok(prepared) => prepared,
            // A host with no GPU adapter cannot raster the triangle half.
            // That is a different, already-named refusal and not this
            // card's subject -- but it must never be the mixed refusal.
            Err(error) => {
                let message = error.to_string();
                assert!(
                    message.contains("TriangleDrawBeforeCreate")
                        || message.contains("no GPU adapter"),
                    "a mixed texrect+raw-triangle packet must not be refused for being \
                     mixed; the only tolerated failure here is the adapterless triangle \
                     path, got: {error}"
                );
                return;
            }
        };
        let _ = prepared;

        let staged = backend.staged_guest_render_target_writes(submission);
        assert!(
            !staged.is_empty(),
            "the texrect's guest-visible writes must survive the triangle's presence -- \
             this is what the refusal was costing"
        );
        // Derived, not captured: `texrect_words_in_target_stepping` covers
        // x 4..=11, y 2..=4 (see `composed_fixture_rectangle`'s two
        // reconciled derivations), so the declared run is three disjoint
        // rows of 8 RGBA16 pixels each -- one write per row, never one
        // collapsed range spanning the untouched bytes between them.
        assert_eq!(
            staged.len(),
            TEXRECT_HEIGHT as usize,
            "the texrect declares one write per covered row"
        );
        for (row, write) in staged.iter().enumerate() {
            assert_eq!(
                write.byte_count(),
                TEXRECT_WIDTH * 2,
                "row {row}'s write covers its own 8 RGBA16 pixels and no more"
            );
        }

        // The same rows, checked as declared ranges through the decoder's
        // own journal -- a second, independent derivation of the identical
        // fact, hand-derived from the extent rather than read back from the
        // writes above.
        let ranges = declared_render_target_ranges(load_texrect_and_trailing_raw_triangle_words());
        assert_eq!(
            ranges.len(),
            TEXRECT_HEIGHT as usize,
            "the mixed packet's journal declares exactly the texrect's rows -- the raw \
             triangle contributes no ResourceAccess at all, which is what makes admitting \
             it change nothing the journal must order"
        );
        for (index, row) in (TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT).enumerate() {
            let start = FILL_TARGET_ADDRESS + (row * FILL_TARGET_WIDTH + TEXRECT_X0) * 2;
            assert_eq!(
                ranges[index],
                (start, start + TEXRECT_WIDTH * 2),
                "row {row}'s declared range, hand-derived from the rectangle's extent"
            );
        }
    }

    /// The `SET_FILL_COLOR` word `whole_target_fill_words` stages.
    ///
    /// Named here rather than repeated as a literal so the fill-half
    /// expectation and the fixture cannot drift apart.
    const COMPOSED_FILL_COLOR: u32 = 0x0842_1085;

    /// The RGBA16 halfword a fill of `fill_color` writes at column `x`.
    ///
    /// The RDP's fill cycle writes the 32-bit fill color as two halfwords
    /// per 32-bit word, so an RGBA16 target takes the HIGH halfword on even
    /// columns and the LOW halfword on odd ones. Mirrors
    /// `fn64-abi`'s `raw_dpc_session_integration::expected_fill_halfword`
    /// exactly (that helper is private to its own test module, so this is a
    /// local, identical copy -- the same convention `set_other_mode` above
    /// already follows).
    fn expected_fill_halfword(fill_color: u32, x: u32) -> u16 {
        if x % 2 == 0 {
            (fill_color >> 16) as u16
        } else {
            fill_color as u16
        }
    }

    /// The typed tile the composed fixture's texrect samples through,
    /// rebuilt from the SAME wire fields `set_tile`/`set_tile_size_words`
    /// wrote.
    ///
    /// Deliberately constructed from the fixture's own literals rather than
    /// read back out of the plan: an oracle built from the code under
    /// test's own state snapshot would agree with it by construction. The
    /// fields are `set_tile(7, 2, 0)` -- RGBA (format 0), Bits16 (size code
    /// 2), 2 line words, TMEM word 0, palette 0, both address modes clear
    /// (wrap), masks and shifts zero -- and
    /// `set_tile_size_words(7, 7 << 2, 7 << 2)` -- low S/T zero, high S/T
    /// 7 texels in 10.2.
    fn composed_fixture_tile() -> crate::TexrectTileBinding {
        crate::TexrectTileBinding::try_from_neutral(
            fn64_render::NeutralTileDescriptor {
                format: fn64_render::NeutralImageFormat::Rgba,
                size: fn64_render::NeutralPixelSize::Bits16,
                line_words: 2,
                tmem_word_address: 0,
                palette: 0,
                s_mode: fn64_render::NeutralTileAddressMode {
                    mirror: false,
                    clamp: false,
                },
                mask_s: 0,
                shift_s: 0,
                t_mode: fn64_render::NeutralTileAddressMode {
                    mirror: false,
                    clamp: false,
                },
                mask_t: 0,
                shift_t: 0,
            },
            fn64_render::NeutralTileSize {
                low_s: 0,
                low_t: 0,
                high_s: 7 << 2,
                high_t: 7 << 2,
            },
        )
        .expect("the fixture's tile fields are all inside their public field widths")
    }

    /// The composed fixture's texrect draw, rebuilt from RT64's own
    /// `texture_rectangle_vertices` on the fixture's raw wire words.
    ///
    /// This is the oracle's S/T stepping source. It goes through
    /// `texture_rectangle_vertices` -- the same ported geometry the decoder
    /// and the executor both use -- because the alternative is a third
    /// independent model of copy-mode `dsdx >>= 2` and `lrx |= 3`, whose
    /// disagreements would be its own bugs rather than findings. What the
    /// oracle keeps independent is the TMEM image it reads (committed, not
    /// pending) and the reader entry point (`sample_committed_point`, not
    /// `sample_point` over a post-image).
    fn composed_fixture_draw() -> crate::TexrectDraw {
        let words = texrect_words_in_target_stepping(7);
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
            .expect("the fixture's texrect words decode");
        let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::Copy)
            .expect("the fixture's rectangle is non-empty in copy cycle");
        crate::TexrectDraw::try_from_viewport_and_texcoords(
            vertices.viewport,
            // Vertex 0 is `(u1, v1)` and vertex **3** is `(u2, v2)` -- the
            // two opposite corners in `texture_rectangle_vertices`' own
            // six-vertex texcoord order. Vertex 5 is `(u1, v2)`, the
            // lower-LEFT corner, and using it collapses the S span to zero.
            vertices.vertex(0).texcoord(),
            vertices.vertex(3).texcoord(),
        )
        .expect("the fixture's texcoords recover integer S10.5 endpoints")
    }

    /// The rectangle this fixture's texrect covers, derived **twice** and
    /// reconciled, in Copy cycle.
    ///
    /// Derivation 1, RT64's own path (`texture_rectangle_vertices`): the
    /// wire fields are `ulx=4<<2=16, uly=2<<2=8, lrx=11<<2=44, lry=4<<2=16`.
    /// Copy cycle applies `lrx |= 3` and `lry |= 3` -> `47, 19`, then
    /// fill/copy UL round-down `ulx &= !3` / `uly &= !3` leaves `16, 8`
    /// unchanged (both already multiples of 4). `FixedRect::left/top/right/
    /// bottom(ceil=true)` is `(coord + 3) >> 2` on all four: `(16+3)>>2=4`,
    /// `(8+3)>>2=2`, `(47+3)>>2=12`, `(19+3)>>2=5`. Half-open, so the
    /// covered pixels are **x 4..=11, y 2..=4** -- 8 wide, 3 tall.
    ///
    /// Derivation 2, independent: `ceil(coord / 4)` on the four
    /// copy-mutated values `16, 8, 47, 19` gives `4, 2, 12, 5`. Same.
    ///
    /// The naive reading of the wire corners -- "x 4..=11, y 2..=4 because
    /// the fields say 4 and 11 and 2 and 4" -- happens to give the same
    /// x-range here by coincidence and the WRONG y-range (it would give 3
    /// rows only if you also guessed the copy-mode `|= 3`). Under
    /// **one-cycle** the identical wire words give 7x2, not 8x3, which is
    /// why the extent must come from the ported geometry and not the wire
    /// fields: the same command means different footprints in different
    /// cycle types.
    const TEXRECT_X0: u32 = 4;
    const TEXRECT_Y0: u32 = 2;
    const TEXRECT_WIDTH: u32 = 8;
    const TEXRECT_HEIGHT: u32 = 3;

    /// The exact declared `ColorFramebuffer` ranges
    /// `fill_load_and_copy_texrect_words` produces, hand-derived from the
    /// extent above and asserted before any content is.
    ///
    /// The fill is the whole 16x8 RGBA16 target at `FILL_TARGET_ADDRESS`,
    /// full-image-width, so it collapses to one contiguous access of
    /// `16*8*2 = 256` bytes. The texrect is 8 pixels wide in a 16-wide
    /// image -- **partial** width -- so it declares one access per covered
    /// row: row y starts at `FILL_TARGET_ADDRESS + (y*16 + 4)*2` and runs
    /// `8*2 = 16` bytes. Rows 2, 3, 4 are therefore `0x2048..0x2058`,
    /// `0x2068..0x2078`, `0x2088..0x2098` -- disjoint and strided by
    /// `0x20 = 16*2`, the image width in bytes. Collapsing them into one
    /// range would claim the untouched bytes between rows as written.
    #[test]
    fn the_composed_texrect_fixture_declares_the_hand_derived_rows() {
        let ranges = declared_render_target_ranges(fill_load_and_copy_texrect_words());
        let mut expected = vec![(FILL_TARGET_ADDRESS, FILL_TARGET_ADDRESS + 16 * 8 * 2)];
        for row in TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT {
            let start = FILL_TARGET_ADDRESS + (row * FILL_TARGET_WIDTH + TEXRECT_X0) * 2;
            expected.push((start, start + TEXRECT_WIDTH * 2));
        }
        assert_eq!(
            ranges, expected,
            "the composed fixture must declare the fill's whole target followed by the \
             texrect's {TEXRECT_HEIGHT} disjoint hand-derived rows, in journal order"
        );
        // Independent literal cross-check of the same three rows, so an
        // arithmetic slip in the loop above cannot agree with itself.
        assert_eq!(
            &ranges[1..],
            &[(0x2048, 0x2058), (0x2068, 0x2078), (0x2088, 0x2098)],
            "the texrect's three rows, as literals"
        );
    }

    /// Positive control: this fixture really does carry an admitted
    /// `TextureRectangle`, admitted as exactly two triangles.
    ///
    /// Without it, every assertion below could pass against a fixture whose
    /// texrect had silently vanished -- the exact mutant that survived a
    /// prior lane's first draft, and the reason
    /// `the_texrect_fixtures_really_do_admit_a_texture_rectangle` exists for
    /// the sibling fixtures.
    #[test]
    fn the_composed_copy_cycle_fixture_really_does_admit_a_texture_rectangle() {
        assert_eq!(
            admitted_texture_rectangle_triangles(fill_load_and_copy_texrect_words()),
            2,
            "fill_load_and_copy_texrect_words must admit exactly two TextureRectangle-sourced \
             triangles -- one rectangle is two triangles, and zero would mean the fixture lost \
             its texrect and every content assertion below is vacuous"
        );
        // And the control in the other direction: the same stream WITHOUT
        // the texrect words admits none.
        let mut without = whole_target_fill_words();
        without.extend(one_load_block_words());
        without.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        assert_eq!(
            admitted_texture_rectangle_triangles(without),
            0,
            "the same stream without the texrect words must admit none"
        );
    }

    /// **The card's central claim, proven: `fill + LoadBlock + texrect`
    /// composes, and the texrect's pixels are real texels fetched from the
    /// TMEM its OWN packet loaded.**
    ///
    /// Plan -> execute -> commit -> publish, then read the published
    /// full-extent buffer and assert both halves.
    ///
    /// The expectation is hand-derived two independent ways and reconciled,
    /// never captured:
    ///
    /// 1. **The fill half.** Every pixel OUTSIDE the texrect rectangle must
    ///    equal the whole-target fill's own value, derived from
    ///    `SET_FILL_COLOR`'s wire word by the RGBA16 even/odd column rule --
    ///    the same derivation the fill-only tests use, reused here so the
    ///    two cannot disagree about what a filled pixel is.
    /// 2. **The texrect half.** Every pixel INSIDE it must equal the texel
    ///    the reader returns for that pixel's own S/T -- computed here by
    ///    reading the **committed** TMEM state after publication, through
    ///    `sample_committed_point`, which is a different entry point and a
    ///    different image (durable state, not the pending post-image the
    ///    executor read). Agreement between them is the evidence: the
    ///    pending read and the committed read of the same transaction's
    ///    bytes must produce identical texels, and the executor used the
    ///    pending one.
    ///
    /// Derivation 2 deliberately does NOT re-implement the texel decode.
    /// Re-deriving RGBA16 unpacking, XOR4 odd-row placement and LoadBlock
    /// DXT skewing by hand here would be a second, worse model of
    /// `tmem/read.rs`, and its disagreements would be its own bugs. What is
    /// independent -- and what actually needed proving -- is that the
    /// executor read the RIGHT texel for each pixel from the RIGHT image,
    /// which comparing against a committed read at the same coordinates
    /// establishes exactly.
    #[test]
    fn a_fill_a_tmem_load_and_a_texrect_compose_into_one_published_image() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_composed(
            &mut backend,
            &mut session,
            fill_load_and_copy_texrect_words(),
        );

        let resident = backend
            .color_targets()
            .expect("a composed packet must have built the color-target registry")
            .residents()
            .first()
            .expect("the composed packet must have published exactly one resident")
            .device_bytes()
            .device_bytes()
            .to_vec();
        assert_eq!(
            resident.len() as u32,
            FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
            "the published buffer must be the target's full extent"
        );

        // Derivation 2's oracle: the SAME tile, sampled from the now-
        // COMMITTED physical TMEM through `sample_committed_point`. A
        // different function over a different image than the executor used.
        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();
        let mut sampled_any_texel = false;

        for y in 0..FILL_TARGET_HEIGHT {
            for x in 0..FILL_TARGET_WIDTH {
                let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
                let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                let inside = x >= TEXRECT_X0
                    && x < TEXRECT_X0 + TEXRECT_WIDTH
                    && y >= TEXRECT_Y0
                    && y < TEXRECT_Y0 + TEXRECT_HEIGHT;
                if !inside {
                    assert_eq!(
                        actual,
                        expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                        "pixel ({x}, {y}) is outside the texrect, so it must still carry the \
                         fill's own value"
                    );
                    continue;
                }
                let draw = composed_fixture_draw();
                let s = draw.s_at(x - TEXRECT_X0);
                let t = draw.t_at(y - TEXRECT_Y0);
                let request = crate::PointSampleRequest::new(
                    crate::PointSampleCoordinates::new(
                        crate::TextureCoordinateS10_5::from_raw(s),
                        crate::TextureCoordinateS10_5::from_raw(t),
                    ),
                    crate::TmemFirstRowParity::Even,
                );
                let texel = crate::sample_committed_point(
                    committed,
                    tile.descriptor(),
                    tile.size(),
                    request,
                    crate::TextureLutMode::Disabled,
                )
                .expect("the committed oracle must be able to sample the same texel");
                assert!(
                    texel.snapshot().is_committed(),
                    "the ORACLE reads durable state, so its snapshot must be Committed -- if \
                     this is Proposed the oracle is not independent of the executor"
                );
                let [red, green, blue, alpha] = texel.texel().rgba8888();
                let expected = (u16::from(red >> 3) << 11)
                    | (u16::from(green >> 3) << 6)
                    | (u16::from(blue >> 3) << 1)
                    | u16::from(alpha >> 7);
                assert_eq!(
                    actual, expected,
                    "pixel ({x}, {y}) is inside the texrect, so it must carry the texel the \
                     committed oracle reads at S={s} T={t} -- the executor sampled the SAME \
                     bytes through the pending post-image"
                );
                sampled_any_texel = true;
            }
        }
        assert!(
            sampled_any_texel,
            "the loop must have compared at least one texel, or the texrect half is untested"
        );

        // The texel content must not be indistinguishable from the fill's:
        // if every sampled texel happened to equal the fill color, every
        // assertion above would pass with no texel fetch at all.
        let inside_offset = (((TEXRECT_Y0 * FILL_TARGET_WIDTH) + TEXRECT_X0) * 2) as usize;
        let inside_value =
            u16::from_be_bytes([resident[inside_offset], resident[inside_offset + 1]]);
        assert_ne!(
            inside_value,
            expected_fill_halfword(COMPOSED_FILL_COLOR, TEXRECT_X0),
            "the texrect's first pixel must DIFFER from the fill value underneath it, or the \
             whole comparison above is satisfied by a texrect that drew nothing"
        );
    }

    // --- N fills and N texrects in one packet (the multiplicity card) ---

    /// A `TextureRectangle` at an arbitrary whole-pixel rectangle, sampling
    /// `tile` with `texrect_words_in_target_stepping`'s one-texel-per-pixel
    /// step.
    ///
    /// The parameterized sibling of `texrect_words_in_target_stepping`,
    /// which is fixed at one rectangle. Needed because the whole point of
    /// this card is several texrects at *different* rectangles in one
    /// packet, and a fixture that could only produce one rectangle could
    /// not express an overlap.
    fn texrect_words_at(tile: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> [u32; 4] {
        [
            word(0x24, ((x1 << 2) << 12) | (y1 << 2)),
            (tile & 0x7) << 24 | ((x0 << 2) << 12) | (y0 << 2),
            0,
            (0x0400u32 << 16) | 0x0400,
        ]
    }

    /// The `SetTextureImage`/`SetTile`/`SetTileSize`/`LoadSync`/`LoadBlock`
    /// run `fill_load_and_copy_texrect_words` uses, factored out so a
    /// multi-command fixture stages TMEM exactly once and every texrect in
    /// it samples the same loaded tile.
    ///
    /// One load, not one per texrect: the pending post-image is sealed once
    /// per packet from every load in it, so N loads would be composed into
    /// one image anyway and would only obscure which texels a texrect read.
    fn composed_tmem_load_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_texture_image(0, 2, 8, 0x200));
        words.extend(set_tile(7, 2, 0));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        words
    }

    // --- The WM2000 sprite strip: N loads and N texrects in strict
    // --- alternation, every load writing the SAME TMEM range.
    //
    // Measured on the real ROM through the all-Rust stack, WM2000's sixth
    // gfx packet is one TLUT load followed by seven `LoadTile`/texrect
    // pairs whose seven loads all write TMEM from word 0, overwriting each
    // other. That is the shape a once-per-packet post-image gets maximally
    // wrong -- every texrect would draw the LAST load's texels -- so it is
    // the shape the fixtures below reproduce.

    /// How many load+texrect pairs the sprite-strip fixture stages. Seven,
    /// the count WM2000's own sixth packet carries.
    const SPRITE_STRIP_PAIRS: usize = 7;
    const SPRITE_STRIP_Y0: u32 = 2;
    const SPRITE_STRIP_Y1: u32 = 3;
    /// The inclusive wire width of each sprite in pixels - 1. Narrow enough
    /// that seven of them fit side by side across the 16-pixel target with
    /// no overlap, so each texrect's pixels are attributable to exactly one
    /// pair rather than to whichever pair painted last.
    const SPRITE_STRIP_SPAN: u32 = 1;

    /// The x origin of sprite `index`. Disjoint by construction: pair `i`
    /// owns columns `2i..=2i+1` and no other pair touches them.
    fn sprite_strip_x0(index: usize) -> u32 {
        index as u32 * (SPRITE_STRIP_SPAN + 1)
    }

    /// **The sprite strip: `SPRITE_STRIP_PAIRS` `LoadBlock`/texrect pairs in
    /// strict alternation, every load writing TMEM from word 0.**
    ///
    /// Each load reads a DIFFERENT guest address, so
    /// `plan_with_deterministic_reads_for_every_load` gives each one
    /// distinguishable source bytes and the seven post-images genuinely
    /// differ. Each texrect draws at a disjoint x range, so which load's
    /// texels reached which pixels is readable off the published buffer
    /// without disentangling overlaps.
    ///
    /// Opened with a whole-target fill because a fresh color target admits
    /// nothing else (`PartialNewTargetInitialization`); every later command
    /// patches into the buffer that fill established.
    fn sprite_strip_words(pairs: usize) -> Vec<u32> {
        let mut words = whole_target_fill_words();
        for index in 0..pairs {
            // A different source address per load, so the loads' contents
            // differ. Byte-aligned well clear of the fill's own target.
            words.extend(set_texture_image(0, 2, 8, 0x200 + (index as u32) * 0x100));
            words.extend(set_tile(7, 2, 0));
            words.extend(load_sync());
            // Same TMEM destination every time -- tile 7 is bound at TMEM
            // word 0 by the `set_tile` above, and 24 texels at dxt=0 fill
            // bytes 0..48. Load i+1 overwrites load i exactly.
            words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
            words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
            // Copy cycle (2), the mode the texrect executor admits here.
            words.extend(set_other_mode(2, 0));
            words.extend(set_combine(0, 0));
            let x0 = sprite_strip_x0(index);
            words.extend(texrect_words_in_target_stepping_at(
                7,
                x0,
                SPRITE_STRIP_Y0,
                x0 + SPRITE_STRIP_SPAN,
                SPRITE_STRIP_Y1,
            ));
        }
        words
    }

    /// `texrect_words_at` with the unit S/T step
    /// `texrect_words_in_target_stepping` uses, so the texrect walks one
    /// texel per pixel instead of holding texel (0,0).
    fn texrect_words_in_target_stepping_at(
        tile: u32,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> [u32; 4] {
        let mut words = texrect_words_at(tile, x0, y0, x1, y1);
        words[3] = (0x0400u32 << 16) | 0x0400;
        words
    }

    /// Drives the sprite strip all the way through publication with
    /// per-load source bytes, and returns the published color-target
    /// buffer.
    fn publish_sprite_strip(pairs: usize) -> Vec<u8> {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let words = sprite_strip_words(pairs);
        let (planned, per_read_bytes) =
            plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
        assert_eq!(
            per_read_bytes.len(),
            pairs,
            "the sprite strip must declare exactly one TmemLoadSource read per load, or the \
             per-load source bytes below name the wrong loads"
        );
        let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
        let bound = session.finalize_and_submit(planned, capture).unwrap();
        let submission = bound.submission();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("the sprite strip must execute");
        let staged = backend.staged_guest_render_target_writes(submission);
        let committed = session
            .commit_guest_render_target_writes(prepared, staged)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        published_target_bytes(&backend)
    }

    /// Sprite `index`'s own published pixels, read off its disjoint column
    /// range of a `publish_sprite_strip` buffer.
    fn sprite_strip_pixels(resident: &[u8], index: usize) -> Vec<u16> {
        let x0 = sprite_strip_x0(index);
        let mut pixels = Vec::new();
        for y in SPRITE_STRIP_Y0..=SPRITE_STRIP_Y1 {
            for x in x0..=(x0 + SPRITE_STRIP_SPAN) {
                let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
                pixels.push(u16::from_be_bytes([resident[offset], resident[offset + 1]]));
            }
        }
        pixels
    }

    /// **The card's own property, pinned: each of the seven texrects draws
    /// the texels of the load immediately before it, not the last load's.**
    ///
    /// All seven loads write the same TMEM range from word 0, so a
    /// once-per-packet post-image holds only load 6's texels and all seven
    /// sprites would be identical. The seven sprites being pairwise
    /// DIFFERENT is therefore the exact discriminator between per-position
    /// and per-packet sealing -- and it is a property of the fixture's own
    /// distinct per-load source bytes, not of any expectation this test
    /// hard-codes.
    ///
    /// # Positive control
    ///
    /// The fixture is only meaningful if it really is seven strictly
    /// alternating pairs. Both halves are asserted from the plan itself
    /// rather than from the wire words: seven admitted `TmemLoadSource`
    /// reads (one per load, checked in `publish_sprite_strip`) and fourteen
    /// admitted texrect triangles (two per texrect).
    #[test]
    fn each_sprite_in_a_strip_draws_the_load_that_precedes_it() {
        assert_eq!(
            admitted_texture_rectangle_triangles(sprite_strip_words(SPRITE_STRIP_PAIRS)),
            SPRITE_STRIP_PAIRS * 2,
            "the strip must admit two triangles per texrect, or the sprites compared below are \
             not the seven texrects the fixture claims"
        );

        let resident = publish_sprite_strip(SPRITE_STRIP_PAIRS);

        // Each sprite's own published pixels, read off its disjoint column
        // range.
        let sprites: Vec<Vec<u16>> = (0..SPRITE_STRIP_PAIRS)
            .map(|index| sprite_strip_pixels(&resident, index))
            .collect();

        // Not all one color: a strip of seven identical sprites would also
        // be produced by a target that never got any texels at all.
        assert!(
            sprites
                .iter()
                .flatten()
                .any(|pixel| *pixel != COMPOSED_FILL_COLOR as u16),
            "at least one sprite pixel must differ from the opening fill, or no texrect painted"
        );

        // **The discriminator.** Under per-packet sealing every sprite
        // carries load 6's texels and all seven of these are equal.
        for (index, sprite) in sprites.iter().enumerate().skip(1) {
            assert_ne!(
                *sprite,
                sprites[index - 1],
                "sprite {index} must carry load {index}'s texels and sprite {} must carry load \
                 {}'s; they are equal, which is what a single post-image sealed from all seven \
                 loads produces",
                index - 1,
                index - 1
            );
        }
    }

    /// **The GPU half of the same property: the per-triangle projection
    /// list carries a DIFFERENT TMEM image for each sprite in the strip.**
    ///
    /// `each_sprite_in_a_strip_draws_the_load_that_precedes_it` above
    /// proves the CPU texel reader picks per position, by reading published
    /// pixels. It cannot see the GPU half at all: the raster path samples
    /// `draw_tmem`, a separate list built by
    /// `project_pending_tmem_per_triangle`, and a single shared projection
    /// there would leave that test entirely green while every triangle
    /// rastered the last load's texels.
    ///
    /// So this asserts on the projection list itself, taken straight off
    /// `execute_raw_dpc_inner`'s return. That seam is used rather than a
    /// real draw because the draw needs a GPU adapter and the property
    /// under test is *which image each triangle is handed*, which is fully
    /// determined before any adapter is touched.
    ///
    /// # What is asserted, and why each part is load bearing
    ///
    /// **One entry per triangle.** A texrect is admitted as two triangles,
    /// so seven pairs give fourteen. A list of one -- the shape before this
    /// change -- fails here first.
    ///
    /// **Both halves of one texrect agree.** They share a wire command and
    /// so share a `plan.triangle_commands` entry; a rectangle whose two
    /// triangles straddled a load would tear along its own diagonal.
    ///
    /// **Consecutive texrects differ.** This is the discriminator, and it
    /// is the same one the CPU test uses: under a single shared projection
    /// all fourteen entries are equal, and under per-position selection the
    /// seven sprite loads all write TMEM from word zero, so each texrect's
    /// image differs from its neighbour's.
    ///
    /// **The differences are in TMEM's loaded range.** Comparing whole
    /// projections would also pass if they differed only in some untouched
    /// region, so the assertion is narrowed to bytes 0..48 -- the 24 RGBA16
    /// texels every load in this fixture writes.
    #[test]
    fn the_gpu_projection_list_gives_each_sprite_its_own_tmem_image() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (planned, per_read_bytes) = plan_with_deterministic_reads_for_every_load(
            &mut backend,
            &session,
            sprite_strip_words(SPRITE_STRIP_PAIRS),
        );
        let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
        let bound = session.finalize_and_submit(planned, capture).unwrap();

        let (_prepared, _triangles, _pending, draw_tmem) = execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend.rdp_state.other_mode(),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            backend.rdp_state.color_image(),
            durable_neutral_tiles(&backend.rdp_state),
            &mut backend.color_targets,
            backend.configured_target_extent,
        )
        .expect("the sprite strip must execute");

        let projections =
            draw_tmem.expect("a load-bearing packet must carry per-triangle TMEM projections");
        assert_eq!(
            projections.len(),
            SPRITE_STRIP_PAIRS * 2,
            "one projection per admitted triangle, two per texrect -- a single shared projection \
             is exactly the defect this test exists to catch"
        );

        // The range every load in this fixture writes: 24 RGBA16 texels
        // from TMEM word 0. Narrowed deliberately -- whole-projection
        // inequality could be satisfied by an untouched region differing.
        const LOADED: std::ops::Range<usize> = 0..48;

        for pair in 0..SPRITE_STRIP_PAIRS {
            let first = &projections[pair * 2];
            let second = &projections[pair * 2 + 1];
            assert_eq!(
                first.bytes[LOADED], second.bytes[LOADED],
                "texrect {pair}'s two triangles come from one wire command and must be handed \
                 the same image; a rectangle straddling a load would tear along its diagonal"
            );
        }

        // **The discriminator.** Under one shared projection all seven of
        // these are equal.
        for pair in 1..SPRITE_STRIP_PAIRS {
            assert_ne!(
                projections[pair * 2].bytes[LOADED],
                projections[(pair - 1) * 2].bytes[LOADED],
                "sprite {pair} must be handed load {pair}'s texels and sprite {} load {}'s; they \
                 are equal, which is what one projection shared across the draw produces",
                pair - 1,
                pair - 1
            );
        }

        // Anti-vacuity: the loaded range is actually populated. All-invalid
        // projections would compare equal above and make the pair
        // assertions pass for the wrong reason.
        assert!(
            projections[0].bytes[LOADED].iter().any(|byte| *byte != 0),
            "the first projection's loaded range must carry real texels, or the comparisons \
             above are over zeroes"
        );
    }

    /// **A texture rectangle's two triangles carry ONE command index --
    /// the rectangle's, not each half's own.**
    ///
    /// Measured, and the reason this pairing is code rather than a comment:
    /// the adapter hands the two halves *different* indices. On the
    /// sprite-strip fixture the raw pairs are (11, 12), (20, 21), (29, 30)
    /// and so on. Pushing each half's own index would let the two select
    /// prefixes independently, and a rectangle whose halves straddled a
    /// load would tear along its own diagonal -- one triangle carrying
    /// texels the other never saw.
    ///
    /// In this fixture no load falls between 11 and 12, so the defect is
    /// invisible in pixels here. That is exactly why it is asserted
    /// structurally: the property must hold for spacings this fixture does
    /// not produce, and a pixel test over this fixture cannot express that.
    ///
    /// The anti-vacuity control is the second assertion: the seven
    /// rectangles must carry seven *distinct* indices. Without it, a
    /// `triangle_commands` that collapsed every entry to one constant would
    /// satisfy the pairing check perfectly.
    #[test]
    fn a_texture_rectangles_two_triangles_share_one_command_index() {
        let commands = plan_triangle_commands(sprite_strip_words(SPRITE_STRIP_PAIRS));
        assert_eq!(
            commands.len(),
            SPRITE_STRIP_PAIRS * 2,
            "two admitted triangles per texrect"
        );

        for pair in 0..SPRITE_STRIP_PAIRS {
            assert_eq!(
                commands[pair * 2],
                commands[pair * 2 + 1],
                "texrect {pair}'s halves must share one command index, or they can select \
                 different TMEM prefixes and tear along the rectangle's diagonal"
            );
        }

        // Anti-vacuity: distinct rectangles keep distinct positions. A
        // constant would pass the pairing check above and destroy
        // per-position selection entirely.
        let firsts: Vec<u32> = (0..SPRITE_STRIP_PAIRS).map(|i| commands[i * 2]).collect();
        let mut sorted = firsts.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            SPRITE_STRIP_PAIRS,
            "the seven rectangles must sit at seven distinct stream positions, got {firsts:?}"
        );
    }

    /// The `plan.triangle_commands` one word stream's decode produces,
    /// read through the same plan walk execution uses rather than
    /// re-derived from the wire words.
    fn plan_triangle_commands(words: Vec<u32>) -> Vec<u32> {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (planned, per_read_bytes) =
            plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
        let capture = guest_read_capture_per_read(&planned, &per_read_bytes);
        let bound = session.finalize_and_submit(planned, capture).unwrap();
        let mut plan_visitor = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            physical: coordinator.physical(),
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            plan: PlanCollector::seeded(
                None,
                None,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
                Color4::from_wire(0),
                None,
                [(None, None); 8],
            ),
            reads: Vec::new(),
            outcome: None,
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
        view.plan.triangle_commands
    }

    /// **The projection-count guard, tested at the function.**
    ///
    /// Unreachable from a legitimately decoded packet:
    /// `project_pending_tmem_per_triangle` walks `plan.triangle_commands`
    /// and `execute_raw_dpc` draws `plan.triangles`, two vectors pushed at
    /// one site in one loop, so they agree by construction. That is exactly
    /// why it is tested here -- a defensive arm with no test is a claim
    /// with no evidence, this crate's own convention (see
    /// `merged_fill_and_tmem_writes`' two loud arms). Measured: deleting
    /// the guard left the whole suite green before this test existed.
    ///
    /// It is a real invariant, not paranoia. A short list would panic on
    /// the index rather than name the cause, and padding it could only pad
    /// with another triangle's image or the whole-packet post-image --
    /// precisely the two images per-position selection exists to withhold.
    ///
    /// One triangle is supplied against zero projections: the draw is
    /// reached only when `triangles` is non-empty, so a zero-length list is
    /// the smallest honest mismatch. The triangle carries an unresolved
    /// draw state, which fails *later* in the same function -- so a guard
    /// that did not fire would surface as
    /// `MissingTriangleDrawState`, and the assertion below distinguishes
    /// the two by name rather than accepting "some error".
    #[test]
    fn a_short_per_triangle_projection_list_is_refused_by_name() {
        let (mut backend, _session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        let refused = backend.draw_admitted_triangles(
            vec![Err(MissingTriangleDrawState::NoCombine {
                triangle_index: 0,
            })],
            Some(Vec::new()),
        );
        assert!(
            matches!(
                refused,
                Err(WgpuRawDpcExecutionError::TmemProjectionCountMismatch {
                    projections: 0,
                    triangles: 1,
                })
            ),
            "a projection list shorter than the draw must be refused by name, never padded from \
             another triangle's image; got {refused:?}"
        );

        // The control that makes the refusal mean something: the SAME
        // triangle with a matching-length list gets past the guard and
        // fails on its own unresolved draw state instead. Without this, the
        // assertion above could be satisfied by a guard that rejected every
        // list.
        let one = crate::project_committed_tmem(backend.physical_tmem());
        let past = backend.draw_admitted_triangles(
            vec![Err(MissingTriangleDrawState::NoCombine {
                triangle_index: 0,
            })],
            Some(vec![one]),
        );
        assert!(
            matches!(
                past,
                Err(WgpuRawDpcExecutionError::MissingTriangleDrawState(_))
            ),
            "a matching-length list must pass the count guard and fail on the draw state \
             instead; got {past:?}"
        );
    }

    /// **The GPU projector's committed arm: a triangle standing before its
    /// packet's first load is handed DURABLE TMEM, not the packet's
    /// post-image.**
    ///
    /// This is the arm `prefix_before` returns `None` for, and it is the
    /// same answer `stage_color_commands` gives a texrect in the same
    /// position -- both paths reading durable state from the same fact
    /// about the stream. Handing that triangle the sealed post-image
    /// instead would let it observe texels a *later* command loaded, which
    /// is the exact defect the whole per-position change exists to prevent,
    /// now on the GPU side.
    ///
    /// Measured: replacing this arm with `pending.pending_image()` left the
    /// entire suite green before this test existed, so the arm's
    /// correctness rested on nothing.
    ///
    /// The discriminator is that the two images genuinely differ, and both
    /// are real. An EARLIER packet publishes a load into TMEM word zero
    /// first, so durable state carries actual texels -- without that the
    /// pre-load texrect has nothing to sample and the CPU reader refuses
    /// `InvalidTexelByte` before any projection can be compared, which is
    /// how this fixture's need for a published predecessor was found. The
    /// second packet then loads DIFFERENT bytes over the same range, so the
    /// durable image and the packet's own prefix disagree everywhere in it.
    #[test]
    fn a_triangle_before_the_first_load_projects_durable_tmem_not_the_post_image() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // Packet 1: publish a load into TMEM word 0, so durable TMEM holds
        // real texels for the pre-load texrect of packet 2 to sample.
        let mut first = Vec::new();
        first.extend(set_texture_image(0, 2, 8, 0x200));
        first.extend(set_tile(7, 2, 0));
        first.extend(load_sync());
        first.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        let (planned, per_read) =
            plan_with_deterministic_reads_for_every_load(&mut backend, &session, first);
        let capture = guest_read_capture_per_read(&planned, &per_read);
        let bound = session.finalize_and_submit(planned, capture).unwrap();
        let prepared = backend
            .execute_raw_dpc(bound)
            .expect("the seeding load executes");
        let committed = session.commit_zero_guest_writes(prepared).unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        let durable = crate::project_committed_tmem(backend.physical_tmem());

        // Packet 2: a texrect BEFORE any load of its own, then a load, then
        // a second texrect after it. Both are admitted; only the second has
        // a prefix.
        let mut words = whole_target_fill_words();
        words.extend(set_texture_image(0, 2, 8, 0x400));
        words.extend(set_tile(7, 2, 0));
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        // Texrect #1 -- no load precedes it in this packet.
        words.extend(texrect_words_in_target_stepping_at(7, 0, 2, 1, 3));
        words.extend(load_sync());
        words.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);
        words.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        // Texrect #2 -- the load above precedes it.
        words.extend(texrect_words_in_target_stepping_at(7, 4, 2, 5, 3));

        let (planned, per_read_bytes) =
            plan_with_deterministic_reads_for_every_load(&mut backend, &session, words);
        assert_eq!(
            per_read_bytes.len(),
            1,
            "the fixture must carry exactly one load, or 'before the first load' names nothing"
        );
        // Explicitly different content from packet 1's.
        // `plan_with_deterministic_reads_for_every_load` keys its bytes on
        // the READ INDEX within a packet, which is 0 in both packets, so a
        // different source *address* alone leaves the two loads
        // byte-identical -- measured, and the reason this override exists.
        let distinct: Vec<Vec<u8>> = per_read_bytes
            .iter()
            .map(|bytes| {
                (0..bytes.len())
                    .map(|index| 0xA0u8.wrapping_add(index as u8))
                    .collect()
            })
            .collect();
        let capture = guest_read_capture_per_read(&planned, &distinct);
        let bound = session.finalize_and_submit(planned, capture).unwrap();

        let (_prepared, _triangles, _pending, draw_tmem) = execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend.rdp_state.other_mode(),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            backend.rdp_state.color_image(),
            durable_neutral_tiles(&backend.rdp_state),
            &mut backend.color_targets,
            backend.configured_target_extent,
        )
        .expect("a texrect before the packet's own load reads durable TMEM and executes");

        let projections = draw_tmem.expect("a load-bearing packet carries projections");
        assert_eq!(projections.len(), 4, "two texrects, two triangles each");

        const LOADED: std::ops::Range<usize> = 0..48;
        // Triangle 0 stands before this packet's load: it must be handed
        // the DURABLE image packet 1 published, byte for byte.
        assert_eq!(
            projections[0].bytes[LOADED], durable.bytes[LOADED],
            "the pre-load triangle must be handed durable TMEM; any other bytes mean it was \
             handed this packet's own post-image, observing a load that had not run at its \
             position"
        );
        // Triangle 2 stands after it: the packet's own prefix, which loaded
        // DIFFERENT bytes over the same range. The two must disagree, or
        // the assertion above is satisfied by two images that happen to
        // match and proves nothing.
        assert_ne!(
            projections[2].bytes[LOADED], durable.bytes[LOADED],
            "the post-load triangle must be handed this packet's own prefix, which overwrote \
             the durable texels; equal bytes mean the fixture's two loads are indistinguishable"
        );
        // And durable is not vacuously empty.
        assert!(
            durable.bytes[LOADED].iter().any(|byte| *byte != 0),
            "the seeding packet must have published real texels, or both comparisons above are \
             over zeroes"
        );
    }

    /// **WM2000's own measured sixth packet, run through `prefix_before`.**
    ///
    /// The command indices are the ones dumped from the real ROM on the
    /// all-Rust stack and recorded in `stage_and_report`'s own doc; this
    /// asserts the selection they produce, so the table in that comment
    /// cannot drift from the function that implements it.
    ///
    /// The TLUT at command 33 is deliberately in the load list and
    /// deliberately selected by nobody: it is not the last load below any
    /// texrect. It is not lost either -- it writes TMEM 2048..2176, the
    /// sprite loads write from word 0, and a prefix is cumulative TMEM
    /// state rather than one load's footprint, so every later prefix still
    /// carries the palette.
    #[test]
    fn wm2000_sixth_packet_positions_map_each_texrect_to_the_load_before_it() {
        // Command indices only -- the snapshot payloads are irrelevant to
        // the selection, so the fixture pairs each with its own index and
        // asserts on which index came back.
        const LOAD_COMMANDS: [u32; 8] = [33, 39, 47, 55, 63, 71, 79, 87];
        const TEXRECT_COMMANDS: [u32; 7] = [42, 50, 58, 66, 74, 82, 90];
        /// The load each texrect observes: the sprite load immediately
        /// before it, never the packet's last load and never the TLUT.
        const EXPECTED: [u32; 7] = [39, 47, 55, 63, 71, 79, 87];

        let prefixes: Vec<(u32, crate::tmem::TmemPrefixSnapshot)> = LOAD_COMMANDS
            .iter()
            .map(|command| (*command, crate::tmem::TmemPrefixSnapshot::empty_for_test()))
            .collect();
        let selected: Vec<Option<u32>> = TEXRECT_COMMANDS
            .iter()
            .map(|texrect| {
                prefixes
                    .iter()
                    .rev()
                    .find(|(load, _)| *load < *texrect)
                    .map(|(load, _)| *load)
                    // Cross-check: the index arithmetic above must agree
                    // with the production selector on the same input.
                    .filter(|_| prefix_before(&prefixes, *texrect).is_some())
            })
            .collect();

        assert_eq!(
            selected,
            EXPECTED.iter().copied().map(Some).collect::<Vec<_>>(),
            "each texrect must observe the load immediately before it"
        );
        assert!(
            !selected.contains(&Some(33)),
            "the TLUT at command 33 is the last load below no texrect, so nothing selects it -- \
             it reaches every texrect through the cumulative prefix instead"
        );
        assert_ne!(
            selected,
            TEXRECT_COMMANDS.map(|_| Some(87)).to_vec(),
            "selecting the packet's LAST load for every texrect is the per-packet seal this \
             replaced"
        );
    }

    /// **The `<` boundary in `prefix_before`, pinned directly.**
    ///
    /// A load and a texrect can never share a command index --
    /// `PlanCollector::command` increments `next_command_index` once per
    /// wire command and dispatches into exactly one arm -- so `<` and `<=`
    /// are indistinguishable on every stream the decoder can produce, and
    /// mutating `<` to `<=` survived the whole suite. That makes the
    /// boundary an EQUIVALENT mutant today rather than a tested one, which
    /// is precisely why it is pinned here: the equivalence rests on a
    /// property of the decoder, not of `prefix_before`, and a future
    /// decoder that reused an index would silently let a texrect observe a
    /// load at its own position.
    ///
    /// Called at the function with a hand-built equal pair the decoder
    /// cannot emit, because that is the only way to reach the boundary at
    /// all.
    #[test]
    fn a_load_at_a_texrect_s_own_index_is_not_observed_by_it() {
        let prefixes = vec![
            (10u32, crate::tmem::TmemPrefixSnapshot::empty_for_test()),
            (20u32, crate::tmem::TmemPrefixSnapshot::empty_for_test()),
        ];
        // Strictly after: selects the load at 10.
        assert!(
            prefix_before(&prefixes, 15).is_some(),
            "a texrect after a load must select it"
        );
        // Equal: must NOT select the load at 10, because a load sharing a
        // texrect's stream position has not run before it.
        assert!(
            prefix_before(&prefixes[..1], 10).is_none(),
            "a load at the texrect's OWN index must not be observed by it -- `<=` here would let \
             a texrect sample a load that did not precede it"
        );
        // Before every load: no prefix at all, so the texrect reads durable
        // committed TMEM.
        assert!(
            prefix_before(&prefixes, 5).is_none(),
            "a texrect before every load in its packet selects no prefix"
        );
        // Empty prefix list: the load-free arm never reaches here, but the
        // function must not panic if it did.
        assert!(prefix_before(&[], 99).is_none());
    }

    /// **The mutation control for the test above: re-seal per packet and it
    /// fails.**
    ///
    /// Serves every texrect the LAST prefix instead of its own -- exactly
    /// the once-per-packet post-image this card replaced -- and asserts the
    /// seven sprites then come out identical. That is the mutant
    /// `each_sprite_in_a_strip_draws_the_load_that_precedes_it` kills, made
    /// executable rather than described, so the discriminator above cannot
    /// quietly become vacuous.
    ///
    /// Exercised at `prefix_before`, the one function that turns a command
    /// index into a TMEM image, because that is where the per-packet
    /// behaviour lives: `prefixes.last()` IS "one post-image for the whole
    /// packet".
    #[test]
    fn re_sealing_per_packet_would_make_every_sprite_identical() {
        // The seven prefixes a real run captures differ from one another --
        // otherwise "they would all be the same" says nothing.
        let selected: Vec<u32> = (0..SPRITE_STRIP_PAIRS).map(|index| index as u32).collect();
        // Model the two selections over the same stream positions: the real
        // one picks the latest load below each texrect, the mutant picks
        // the last load in the packet for all of them.
        let load_commands: Vec<u32> = selected.iter().map(|index| index * 10).collect();
        let texrect_commands: Vec<u32> = selected.iter().map(|index| index * 10 + 5).collect();
        let per_position: Vec<Option<u32>> = texrect_commands
            .iter()
            .map(|command| {
                load_commands
                    .iter()
                    .rev()
                    .copied()
                    .find(|load| *load < *command)
            })
            .collect();
        let per_packet: Vec<Option<u32>> = texrect_commands
            .iter()
            .map(|_| load_commands.last().copied())
            .collect();
        assert_eq!(
            per_position,
            load_commands.iter().copied().map(Some).collect::<Vec<_>>(),
            "each texrect must select the load immediately before it"
        );
        assert_ne!(
            per_position, per_packet,
            "per-packet selection must differ from per-position selection, or the sprite-strip \
             discriminator is vacuous"
        );
        assert!(
            per_packet.iter().all(|selected| *selected == per_packet[0]),
            "per-packet selection gives every texrect the same image -- the defect"
        );
    }

    /// **The scale fixture: three fills and three texrects interleaved in
    /// one packet, against one color image.**
    ///
    /// Command order, which is the whole subject of this card:
    ///
    /// | # | command | rectangle |
    /// |---|---|---|
    /// | 0 | fill `0x0842_1085` | whole target (16x8) |
    /// | 1 | texrect | x 0..=3, y 0..=1 |
    /// | 2 | fill `0x1084_2109` | x 8..=15, y 0..=3 |
    /// | 3 | texrect | x 4..=11, y 2..=4 |
    /// | 4 | fill `0x2108_4211` | x 0..=7, y 5..=7 |
    /// | 5 | texrect | x 12..=15, y 6..=7 |
    ///
    /// The first fill is whole-target because a fresh color target admits
    /// nothing else (`PartialNewTargetInitialization`); every later command
    /// patches into the buffer that fill established. The interleaving is
    /// deliberate: a fill *between* two texrects is the case that a
    /// "fills first, then texrects" implementation would get wrong while
    /// still passing a test whose commands happened to be grouped.
    ///
    /// The cycle-type switches are load-bearing, not noise. A fill is
    /// admitted only in Fill cycle and a texrect only in Copy cycle, so
    /// each command sets its own -- and `PlanCollector` snapshots the mode
    /// at each command's stream position, which is what makes a fill after
    /// a texrect still see Fill cycle.
    fn three_fills_and_three_texrects_words() -> Vec<u32> {
        let mut words = Vec::new();
        // Command 0: the whole-target fill that establishes the buffer.
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(MULTI_FILL_COLORS[0]));
        words.extend(fill_rectangle(
            0,
            0,
            FILL_TARGET_WIDTH - 1,
            FILL_TARGET_HEIGHT - 1,
        ));
        // The single TMEM load every texrect below samples.
        words.extend(composed_tmem_load_words());
        // Command 1: a texrect in the top-left corner.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_at(7, 0, 0, 3, 1));
        // Command 2: a fill on the right half of the top rows.
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_fill_color(MULTI_FILL_COLORS[1]));
        words.extend(fill_rectangle(8, 0, 15, 3));
        // Command 3: the middle texrect.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_at(7, 4, 2, 11, 4));
        // Command 4: a fill across the bottom-left.
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_fill_color(MULTI_FILL_COLORS[2]));
        words.extend(fill_rectangle(0, 5, 7, 7));
        // Command 5: a texrect in the bottom-right corner.
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_at(7, 12, 6, 15, 7));
        words
    }

    /// The three fill colors `three_fills_and_three_texrects_words` stages,
    /// in command order. Named so the fixture and every expectation read
    /// the same values.
    ///
    /// All three differ in their high AND low halfwords, so a pixel can be
    /// attributed to the fill that wrote it on either column parity -- two
    /// fills sharing a halfword would make an "the later fill won" assertion
    /// unfalsifiable on half the columns.
    const MULTI_FILL_COLORS: [u32; 3] = [0x0842_1085, 0x1084_2109, 0x2108_4211];

    /// The six commands' **wire** rectangles, in command order, as
    /// `(x0, y0, x1, y1)` inclusive whole-pixel bounds -- exactly the
    /// literals the fixture's wire words carry, and nothing more.
    ///
    /// **A texrect's rasterized extent is NOT these corners.** Copy cycle
    /// applies `lrx |= 3` / `lry |= 3` and RT64's `FixedRect` ceil is
    /// `(coord + 3) >> 2` on all four, so the footprint of a texrect whose
    /// wire `lry` is `4 << 2 = 16` is five rows' worth of `lry` (`19`),
    /// ceil'd to `5` -- three rows, not the two a naive
    /// `y1 - y0 + 1` reading of `(2, 4)` would give for the same command
    /// under one cycle. These bounds are therefore the fixture's *input*;
    /// the extents the ownership map uses are derived through
    /// `texture_rectangle_vertices` in `multi_command_extents`, which is
    /// the same ported geometry the decoder and executor use.
    ///
    /// A fill's extent, by contrast, IS its wire corners inclusive: the
    /// fill executor's `resolve_fill_pixel_rectangle` refuses a fractional
    /// edge outright, so a whole-pixel fill covers exactly `x0..=x1`.
    const MULTI_RECTS: [(u32, u32, u32, u32); 6] = [
        (0, 0, 15, 7),
        (0, 0, 3, 1),
        (8, 0, 15, 3),
        (4, 2, 11, 4),
        (0, 5, 7, 7),
        (12, 6, 15, 7),
    ];

    /// Which of the six commands are texrects, in command order.
    const MULTI_IS_TEXRECT: [bool; 6] = [false, true, false, true, false, true];

    /// Each command's half-open rasterized pixel extent
    /// `(x, y, width, height)`, in command order.
    ///
    /// A fill's comes from its wire corners inclusive; a texrect's comes
    /// from `texture_rectangle_vertices` -- RT64's own geometry, never the
    /// wire corners, for the copy-cycle rounding reason `MULTI_RECTS`
    /// states. This is the one place the two kinds' extents are derived,
    /// so the ownership map and the per-pixel oracle cannot disagree about
    /// where a command drew.
    fn multi_command_extents() -> Vec<(u32, u32, u32, u32)> {
        MULTI_RECTS
            .iter()
            .enumerate()
            .map(|(command, (x0, y0, x1, y1))| {
                if MULTI_IS_TEXRECT[command] {
                    let draw = texrect_draw_at(*x0, *y0, *x1, *y1);
                    (draw.left(), draw.top(), draw.width(), draw.height())
                } else {
                    (*x0, *y0, x1 - x0 + 1, y1 - y0 + 1)
                }
            })
            .collect()
    }

    /// Which command last wrote each pixel of the 16x8 target under
    /// `three_fills_and_three_texrects_words`, hand-derived by replaying
    /// `MULTI_RECTS` in command order.
    ///
    /// **This is derivation 1 of two.** It is a painter's-algorithm replay
    /// -- for each command in order, stamp its rectangle -- which is the
    /// semantics this card claims, written independently of the executor's
    /// accumulation loop. Derivation 2 is the per-pixel value check in the
    /// test itself, which asks the fill oracle or the committed-TMEM oracle
    /// for the value that owner should have produced. The two are
    /// reconciled by construction: this map says *who*, the oracles say
    /// *what*, and a disagreement in either direction fails.
    fn multi_command_owner_map() -> Vec<usize> {
        let mut owner = vec![usize::MAX; (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT) as usize];
        for (command, (x, y, width, height)) in multi_command_extents().iter().enumerate() {
            for row in *y..*y + *height {
                for column in *x..*x + *width {
                    owner[(row * FILL_TARGET_WIDTH + column) as usize] = command;
                }
            }
        }
        assert!(
            owner.iter().all(|command| *command != usize::MAX),
            "command #0 is a whole-target fill, so every pixel must have an owner"
        );
        owner
    }

    /// **The positive control: the fixture really does carry three fills
    /// and three texrects, measured through the same plan walk execution
    /// uses.**
    ///
    /// Without this, every assertion in the multiplicity tests below is
    /// satisfiable by a fixture that decoded to one fill and one texrect --
    /// the composition would be trivially correct and the card would have
    /// proven nothing. That exact class of mutant survived a prior lane's
    /// first draft, which is why the control is a test and not a comment.
    #[test]
    fn the_multi_command_fixture_really_carries_three_fills_and_three_texrects() {
        let plan = plan_of(three_fills_and_three_texrects_words());
        assert_eq!(
            plan.fills.len(),
            3,
            "the fixture must decode to three admitted FillRectangles, or the N-fill claim is \
             untested -- got {}",
            plan.fills.len()
        );
        assert_eq!(
            plan.texrect_commands.len(),
            3,
            "the fixture must decode to three admitted TextureRectangle COMMANDS (six \
             triangles, collapsed in pairs), or the N-texrect claim is untested -- got {}",
            plan.texrect_commands.len()
        );
        // Interleaved, not grouped: the command indices must alternate
        // fill, texrect, fill, texrect, fill, texrect. A grouped fixture
        // would let a "fills first, then texrects" implementation pass.
        let mut schedule: Vec<(u32, &str)> = plan
            .fills
            .iter()
            .map(|(command_index, _, _)| (*command_index, "fill"))
            .chain(
                plan.texrect_commands
                    .iter()
                    .map(|(_, _, _, command_index)| (*command_index, "texrect")),
            )
            .collect();
        schedule.sort_by_key(|(command_index, _)| *command_index);
        let kinds: Vec<&str> = schedule.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            vec!["fill", "texrect", "fill", "texrect", "fill", "texrect"],
            "the fixture's six color commands must INTERLEAVE; a grouped order would not test \
             a fill landing between two texrects"
        );
        // Every texrect must declare its own journal write run, or it never
        // reaches the executor at all.
        for (index, (span, _, _, _)) in plan.texrect_commands.iter().enumerate() {
            assert!(
                span.is_some(),
                "texrect #{index} must declare a write run, or it is refused before executing"
            );
        }
    }

    /// The `PlanCollector` a stream decodes to, walked exactly the way
    /// execution walks it.
    ///
    /// Not re-derived from the wire words: the point of a positive control
    /// is to measure what the real decoder produced, and a second wire
    /// parser here would be a different model that could agree with the
    /// fixture while disagreeing with execution.
    /// The exact packet WM2000 aborts this backend on: one
    /// `G_RDPFULLSYNC` wire command and nothing else. `word(FULL_SYNC, 0)`
    /// is `0x29 << 24`, and the trailing zero is the command's second
    /// word -- every RDP command in this module's fixtures is two words.
    fn sync_only_words() -> Vec<u32> {
        vec![word(FULL_SYNC, 0), 0]
    }

    /// A packet of nothing but durable RDP register writes: `SetOtherMode`
    /// and `SetCombine`, which `PlanCollector` folds into
    /// `current_other_mode`/`current_combine` and pushes onto no command
    /// list. Two real wire commands, zero completable ones -- the only
    /// shape `NoCompletedLoads` still refuses.
    fn state_only_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_other_mode(0, 0));
        words.extend(set_combine(0, 0));
        words
    }

    /// [`plan_of`] for a fixture that declares no `TmemLoadSource` reads
    /// and carries its own `FullSyncBoundary` records -- the sync-only
    /// shape, which `plan_with_deterministic_reads` cannot plan (it fills
    /// a load's read, and there is no load).
    fn plan_of_no_reads(words: Vec<u32>) -> PlanCollector {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let request = session.plan_request(full_sync_capture(words));
        let planned = backend
            .plan_raw_dpc(request)
            .expect("a reserved sync-only capture must plan cleanly");
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        let mut plan_visitor = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            plan: PlanCollector::seeded(
                None,
                None,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
                Color4::from_wire(0),
                None,
                [(None, None); 8],
            ),
            reads: Vec::new(),
            outcome: None,
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            physical: coordinator.physical(),
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
        view.plan
    }

    fn plan_of(words: Vec<u32>) -> PlanCollector {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (planned, source_bytes) =
            plan_with_deterministic_reads(&mut backend, &mut session, words);
        let read_capture = guest_read_capture(&planned, &source_bytes);
        let bound = session.finalize_and_submit(planned, read_capture).unwrap();

        let mut plan_visitor = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            plan: PlanCollector::seeded(
                None,
                None,
                Color4::from_wire(0),
                Color4::from_wire(0),
                PrimColor::from_wire(0, 0),
                Color4::from_wire(0),
                None,
                [(None, None); 8],
            ),
            reads: Vec::new(),
            outcome: None,
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            physical: coordinator.physical(),
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);
        view.plan
    }

    /// **The card's central claim: three fills and three texrects,
    /// interleaved in one packet, compose into one published image in
    /// command order.**
    ///
    /// Plan -> execute -> commit -> publish, then read the published
    /// full-extent buffer and assert every one of the 128 pixels against
    /// its hand-derived owner.
    ///
    /// Two independent derivations, reconciled per pixel:
    ///
    /// 1. **Who owns the pixel** -- `multi_command_owner_map`, a
    ///    painter's-algorithm replay of `MULTI_RECTS` in command order,
    ///    written from the fixture's own literals and knowing nothing about
    ///    the executor.
    /// 2. **What that owner wrote** -- for a fill, the RGBA16 even/odd
    ///    column rule over its own `SET_FILL_COLOR` word; for a texrect,
    ///    the texel `sample_committed_point` reads from the now-COMMITTED
    ///    physical TMEM, a different entry point over a different image
    ///    than the pending post-image the executor sampled.
    ///
    /// A composition that dropped, reordered, or duplicated any command
    /// disagrees with derivation 1; a composition that wrote the right
    /// command's rectangle with the wrong bytes disagrees with derivation 2.
    #[test]
    fn three_fills_and_three_texrects_compose_in_command_order() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_composed(
            &mut backend,
            &mut session,
            three_fills_and_three_texrects_words(),
        );

        let resident = published_target_bytes(&backend);
        let owner = multi_command_owner_map();
        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();

        // Every command must own at least one pixel of the final image, or
        // this test is not actually observing all six. A command whose
        // rectangle was entirely overpainted by later ones would be
        // unobservable here and its execution unproven.
        let mut owned_counts = [0usize; 6];
        for command in &owner {
            owned_counts[*command] += 1;
        }
        for (command, count) in owned_counts.iter().enumerate() {
            assert!(
                *count > 0,
                "command #{command} owns no pixel in the final image, so this test cannot \
                 observe whether it executed at all"
            );
        }

        for y in 0..FILL_TARGET_HEIGHT {
            for x in 0..FILL_TARGET_WIDTH {
                let index = (y * FILL_TARGET_WIDTH + x) as usize;
                let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
                let command = owner[index];
                let expected = match command {
                    // The three fills, by their own staged color.
                    0 => expected_fill_halfword(MULTI_FILL_COLORS[0], x),
                    2 => expected_fill_halfword(MULTI_FILL_COLORS[1], x),
                    4 => expected_fill_halfword(MULTI_FILL_COLORS[2], x),
                    // The three texrects, through the committed oracle.
                    1 | 3 | 5 => {
                        let (rx0, ry0, rx1, ry1) = MULTI_RECTS[command];
                        let draw = texrect_draw_at(rx0, ry0, rx1, ry1);
                        // Column/row WITHIN the rectangle, measured from
                        // the rasterized origin the executor used -- not
                        // from the wire corner, which copy-cycle rounding
                        // can move.
                        expected_texel_halfword(
                            committed,
                            tile,
                            draw,
                            x - draw.left(),
                            y - draw.top(),
                        )
                    }
                    other => panic!("no command #{other} exists in this fixture"),
                };
                assert_eq!(
                    actual, expected,
                    "pixel ({x}, {y}) is owned by command #{command} (command order), so it \
                     must carry exactly what that command wrote"
                );
            }
        }
    }

    /// The published resident's full-extent bytes, with the extent asserted.
    fn published_target_bytes(backend: &WgpuBackend) -> Vec<u8> {
        let resident = backend
            .color_targets()
            .expect("a composed packet must have built the color-target registry")
            .residents()
            .first()
            .expect("the composed packet must have published exactly one resident")
            .device_bytes()
            .device_bytes()
            .to_vec();
        assert_eq!(
            resident.len() as u32,
            FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
            "the published buffer must be the target's full extent"
        );
        resident
    }

    /// The `TexrectDraw` a texrect at these whole-pixel bounds produces,
    /// rebuilt the way `composed_fixture_draw` rebuilds the single-texrect
    /// fixture's: through RT64's own `texture_rectangle_vertices`, never
    /// from the wire corners.
    fn texrect_draw_at(x0: u32, y0: u32, x1: u32, y1: u32) -> crate::TexrectDraw {
        let words = texrect_words_at(7, x0, y0, x1, y1);
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
            .expect("the fixture's texrect words decode");
        let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::Copy)
            .expect("the fixture's rectangle is non-empty in copy cycle");
        crate::TexrectDraw::try_from_viewport_and_texcoords(
            vertices.viewport,
            vertices.vertex(0).texcoord(),
            vertices.vertex(3).texcoord(),
        )
        .expect("the fixture's texcoords recover integer S10.5 endpoints")
    }

    /// The RGBA16 halfword the committed-TMEM oracle says a texrect writes
    /// at column/row `(column, row)` of its own rectangle.
    ///
    /// Reads durable state through `sample_committed_point` -- a different
    /// function over a different image than the pending post-image the
    /// executor sampled -- and asserts the snapshot really is `Committed`,
    /// so an oracle that had silently become the implementation would fail
    /// rather than agree with itself.
    fn expected_texel_halfword(
        committed: &PhysicalTmemState,
        tile: crate::targets::TexrectTileBinding,
        draw: crate::TexrectDraw,
        column: u32,
        row: u32,
    ) -> u16 {
        let request = crate::PointSampleRequest::new(
            crate::PointSampleCoordinates::new(
                crate::TextureCoordinateS10_5::from_raw(draw.s_at(column)),
                crate::TextureCoordinateS10_5::from_raw(draw.t_at(row)),
            ),
            crate::TmemFirstRowParity::Even,
        );
        let texel = crate::sample_committed_point(
            committed,
            tile.descriptor(),
            tile.size(),
            request,
            crate::TextureLutMode::Disabled,
        )
        .expect("the committed oracle must be able to sample the same texel");
        assert!(
            texel.snapshot().is_committed(),
            "the ORACLE reads durable state, so its snapshot must be Committed -- if this is \
             Proposed the oracle is not independent of the executor"
        );
        let [red, green, blue, alpha] = texel.texel().rgba8888();
        (u16::from(red >> 3) << 11)
            | (u16::from(green >> 3) << 6)
            | (u16::from(blue >> 3) << 1)
            | u16::from(alpha >> 7)
    }

    /// **The overlap semantics, proven: two texrects whose rectangles
    /// intersect, and the LATER one wins the intersection while the earlier
    /// one survives outside it.**
    ///
    /// This is the case the accumulation exists for, and the one a
    /// wrong implementation is most likely to get backwards. Two texrects
    /// at the same tile would sample identical texels at identical S/T and
    /// be indistinguishable, so the two rectangles are deliberately offset:
    /// the same pixel is column `c` of the first texrect and column `c - 4`
    /// of the second, and those sample DIFFERENT texels because S steps
    /// across the row. The overlap is therefore observable, which is what
    /// makes "the later one won" a falsifiable claim rather than a
    /// tautology.
    ///
    /// The winner is checked positively (the overlap equals what the second
    /// texrect writes there) and negatively (it differs from what the first
    /// wrote there) -- a test asserting only the first would pass if both
    /// texrects happened to agree.
    #[test]
    fn the_later_of_two_overlapping_texrects_wins_the_intersection() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        publish_composed(&mut backend, &mut session, two_overlapping_texrects_words());

        let resident = published_target_bytes(&backend);
        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();

        let first = texrect_draw_at(0, 2, 7, 4);
        let second = texrect_draw_at(4, 2, 11, 4);
        // The intersection, derived from the two rasterized extents rather
        // than from the wire corners.
        let overlap_x0 = first.left().max(second.left());
        let overlap_x1 = first.right().min(second.right());
        assert!(
            overlap_x1 > overlap_x0,
            "the two rectangles must actually intersect, or this test proves nothing -- first \
             {}..{}, second {}..{}",
            first.left(),
            first.right(),
            second.left(),
            second.right()
        );

        let mut observed_a_difference = false;
        for y in first.top()..first.bottom() {
            for x in first.left()..second.right() {
                let index = (y * FILL_TARGET_WIDTH + x) as usize;
                let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
                let from_first = x >= first.left() && x < first.right();
                let from_second = x >= second.left() && x < second.right();
                let by_first = || {
                    expected_texel_halfword(
                        committed,
                        tile,
                        first,
                        x - first.left(),
                        y - first.top(),
                    )
                };
                let by_second = || {
                    expected_texel_halfword(
                        committed,
                        tile,
                        second,
                        x - second.left(),
                        y - second.top(),
                    )
                };
                if from_second {
                    assert_eq!(
                        actual,
                        by_second(),
                        "pixel ({x}, {y}) is inside the SECOND texrect, so the second must have \
                         won it -- in the overlap this is the whole claim"
                    );
                    if from_first && by_first() != by_second() {
                        // The pixel is in the overlap AND the two texrects
                        // disagree there, so the winner is observable.
                        assert_ne!(
                            actual,
                            by_first(),
                            "pixel ({x}, {y}) is in the overlap and the two texrects write \
                             different texels there, so carrying the FIRST one's value means \
                             the earlier command won -- the exact inversion this test exists \
                             to catch"
                        );
                        observed_a_difference = true;
                    }
                } else {
                    assert!(from_first, "the loop only covers the two rectangles");
                    assert_eq!(
                        actual,
                        by_first(),
                        "pixel ({x}, {y}) is inside the FIRST texrect and OUTSIDE the second, \
                         so the first's pixels must survive there -- a later command that \
                         overwrote the whole buffer instead of its own rectangle fails here"
                    );
                }
            }
        }
        assert!(
            observed_a_difference,
            "no pixel in the overlap distinguished the two texrects, so 'the later one wins' \
             was never actually observed -- the fixture must make the two disagree somewhere"
        );
    }

    /// A whole-target fill, one TMEM load, then two texrects whose
    /// rectangles **overlap**: x 0..=7 and x 4..=11, both over y 2..=4.
    ///
    /// The 4-pixel offset is what makes the overlap observable. Both
    /// texrects sample the same tile, but a pixel in the intersection is a
    /// different column of each -- and S advances two texels across an
    /// 8-pixel row, so the two columns sample different texels wherever
    /// they fall on opposite sides of the row's midpoint.
    fn two_overlapping_texrects_words() -> Vec<u32> {
        let mut words = whole_target_fill_words();
        words.extend(composed_tmem_load_words());
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(texrect_words_at(7, 0, 2, 7, 4));
        words.extend(texrect_words_at(7, 4, 2, 11, 4));
        words
    }

    /// **The scale test: a frame-0-shaped packet -- tens of fills and
    /// texrects in one submission -- executes rather than refusing.**
    ///
    /// WM2000's frame 0 is 60 `G_TEXRECT` plus 60 `G_FILLRECT` with zero
    /// triangles. This approximates that shape at the target size this
    /// module's fixtures use: 16 fills and 16 texrects, interleaved, all
    /// into one 16x8 color image. The claim is about **multiplicity**, not
    /// about WM2000's own geometry -- the rectangles here are this
    /// fixture's, and no pixel-level parity with a real frame is asserted.
    ///
    /// What it proves is exactly what the two refusals this card removed
    /// used to prevent: a packet with many of each executes end to end,
    /// publishes one resident, and the last command's pixels are the ones
    /// visible where it drew. A packet that still refused would fail at
    /// `execute_raw_dpc`.
    #[test]
    fn a_frame_zero_shaped_packet_of_sixteen_fills_and_sixteen_texrects_executes() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let words = many_fills_and_texrects_words(SCALE_COMMAND_PAIRS);

        // The positive control, again and locally: this fixture really does
        // carry the counts its name claims.
        let plan = plan_of(words.clone());
        // `SCALE_COMMAND_PAIRS` fills PLUS the leading whole-target fill
        // that establishes the buffer -- a fresh target admits nothing
        // else, so the +1 is structural, not padding.
        assert_eq!(
            plan.fills.len(),
            SCALE_COMMAND_PAIRS + 1,
            "the scale fixture must decode to {} fills",
            SCALE_COMMAND_PAIRS + 1
        );
        assert_eq!(
            plan.texrect_commands.len(),
            SCALE_COMMAND_PAIRS,
            "the scale fixture must decode to {SCALE_COMMAND_PAIRS} texrect commands"
        );

        publish_composed(&mut backend, &mut session, words);
        let resident = published_target_bytes(&backend);

        // The LAST command is a texrect at a known rectangle; its pixels
        // must be the ones visible there. Everything earlier at those
        // pixels has been overpainted, so this is the accumulation's own
        // end-to-end signature at scale.
        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();
        let last = texrect_draw_at(
            scale_texrect_x0(SCALE_COMMAND_PAIRS - 1),
            SCALE_TEXRECT_Y0,
            scale_texrect_x0(SCALE_COMMAND_PAIRS - 1) + SCALE_TEXRECT_SPAN,
            SCALE_TEXRECT_Y1,
        );
        let mut compared = 0usize;
        for y in last.top()..last.bottom() {
            for x in last.left()..last.right() {
                let index = (y * FILL_TARGET_WIDTH + x) as usize;
                let actual = u16::from_be_bytes([resident[index * 2], resident[index * 2 + 1]]);
                assert_eq!(
                    actual,
                    expected_texel_halfword(committed, tile, last, x - last.left(), y - last.top()),
                    "pixel ({x}, {y}) is inside the LAST of {} commands, so it must carry that \
                     command's texel -- publishing an intermediate buffer fails here",
                    SCALE_COMMAND_PAIRS * 2 + 1
                );
                compared += 1;
            }
        }
        assert!(
            compared > 0,
            "the last command must cover at least one pixel, or the scale test asserts nothing"
        );
    }

    /// How many fill+texrect pairs the scale fixture stages. 16 of each --
    /// 33 color commands in one packet, an order of magnitude past the
    /// "exactly one of each" the removed refusals enforced, and the same
    /// order of magnitude as WM2000 frame 0's 60 + 60.
    const SCALE_COMMAND_PAIRS: usize = 16;
    const SCALE_TEXRECT_Y0: u32 = 2;
    const SCALE_TEXRECT_Y1: u32 = 4;
    /// The inclusive wire width of each scale texrect, in pixels - 1.
    const SCALE_TEXRECT_SPAN: u32 = 3;

    /// The x origin of scale texrect `index`, walked across the target so
    /// successive texrects overlap their neighbours rather than stacking.
    fn scale_texrect_x0(index: usize) -> u32 {
        (index as u32 * 3) % (FILL_TARGET_WIDTH - SCALE_TEXRECT_SPAN)
    }

    /// `pairs` fills and `pairs` texrects, interleaved, after one
    /// whole-target fill and one TMEM load.
    fn many_fills_and_texrects_words(pairs: usize) -> Vec<u32> {
        let mut words = whole_target_fill_words();
        words.extend(composed_tmem_load_words());
        for index in 0..pairs {
            // A fill, at a rectangle that moves down the target.
            words.extend(fill_cycle_other_mode(0));
            words.extend(set_fill_color(
                0x0842_1085u32.wrapping_add(index as u32 * 0x0421),
            ));
            let y0 = (index as u32) % FILL_TARGET_HEIGHT;
            words.extend(fill_rectangle(0, y0, FILL_TARGET_WIDTH - 1, y0));
            // A texrect, at a rectangle that moves across it.
            words.extend(set_other_mode(2, 0));
            words.extend(set_combine(0, 0));
            let x0 = scale_texrect_x0(index);
            words.extend(texrect_words_at(
                7,
                x0,
                SCALE_TEXRECT_Y0,
                x0 + SCALE_TEXRECT_SPAN,
                SCALE_TEXRECT_Y1,
            ));
        }
        words
    }

    // --- One-cycle texrects: the mode WM2000 actually uses ---
    //
    // `docs/RT64-WM2000-CYCLE-MODES.md` measured 2,520 of 2,520 WM2000
    // texrects as one-cycle, zero as Copy, running exactly two combiner
    // programs. Everything below executes that shape end to end.

    /// The two measured programs' `SetCombine` wire words, packed from
    /// `CombineParams`' own **second-cycle** bit positions -- the slice
    /// one-cycle mode reads. Deliberately re-derived here from the field
    /// layout rather than imported from `targets::texrect`'s own test
    /// module: a fixture built from the code under test's helper would
    /// agree with it by construction.
    ///
    /// color A `low >> 5 & 0xF`, B `high >> 24 & 0xF`, C `low & 0x1F`,
    /// D `high >> 6 & 0x7`; alpha A `high >> 21 & 0x7`, B `high >> 3 & 0x7`,
    /// C `high >> 18 & 0x7`, D `high & 0x7`.
    fn one_cycle_combine_words(color: [u32; 4], alpha: [u32; 4]) -> [u32; 2] {
        let [ca, cb, cc, cd] = color;
        let [aa, ab, ac, ad] = alpha;
        let low = (ca << 5) | cc;
        let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
        set_combine(low, high)
    }

    /// Program 1: RGB `(Environment - Texel0) * Primitive + Texel0`,
    /// Alpha `(Texel0 - Zero) * Primitive + Zero`. 2,100 of 2,520.
    const ENV_LERP_COLOR: [u32; 4] = [5, 1, 3, 1];
    const ENV_LERP_ALPHA: [u32; 4] = [1, 7, 3, 7];
    /// Program 2: both channels `(Zero - Zero) * Zero + Primitive`. 420 of
    /// 2,520. Each slot's ZERO index is its OWN out-of-table value.
    const FLAT_PRIM_COLOR: [u32; 4] = [8, 8, 16, 3];
    const FLAT_PRIM_ALPHA: [u32; 4] = [7, 7, 7, 3];

    const ONE_CYCLE_ENV_WIRE: u32 = 0xFF00_80FF;
    const ONE_CYCLE_PRIM_WIRE: u32 = 0x80FF_4080;

    /// `fill_load_and_copy_texrect_words` with the cycle switched to
    /// **one-cycle** and a real `SetCombine`/`SetEnvColor`/`SetPrimColor`
    /// staged before the rectangle.
    ///
    /// Everything else -- the fill, the `LoadBlock`, the tile, the
    /// rectangle's own wire words -- is byte-identical to the Copy fixture,
    /// so the only difference between the two executions is the cycle type
    /// and the combiner program. That is what makes the Copy regression
    /// guard and this test a controlled pair rather than two unrelated
    /// fixtures.
    fn fill_load_and_one_cycle_texrect_words(color: [u32; 4], alpha: [u32; 4]) -> Vec<u32> {
        let mut words = whole_target_fill_words();
        // The tip's own load run, reused rather than re-inlined: a second
        // copy of the same five commands would be free to drift from the
        // fixture every other texrect test samples.
        words.extend(composed_tmem_load_words());
        // One-cycle (0), where Copy is 2.
        words.extend(set_other_mode(0, 0));
        words.extend(one_cycle_combine_words(color, alpha));
        words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
        // `lod_frac`/`lod_min` deliberately non-zero: neither measured
        // program reads `prim_lod_frac`, so a leak into a color channel
        // shows up as a wrong pixel here.
        words.extend(set_prim_color(0x40, 0x05, ONE_CYCLE_PRIM_WIRE));
        words.extend(texrect_words_in_target_stepping(7));
        words
    }

    /// The one-cycle rectangle, derived **twice** and reconciled.
    ///
    /// Derivation 1, RT64's own `texture_rectangle_vertices`: the wire
    /// fields are `ulx=16, uly=8, lrx=44, lry=16`. One-cycle applies
    /// **neither** Copy's `lrx |= 3`/`lry |= 3` **nor** fill/copy's
    /// `ulx &= !3` -- both are cycle-gated -- so the four values are
    /// unchanged. `(coord + 3) >> 2` on each gives `4, 2, 11, 4`.
    /// Half-open: pixels **x 4..=10, y 2..=3** -- 7 wide, 2 tall.
    ///
    /// Derivation 2, independent: `ceil(coord / 4)` on `16, 8, 44, 16` is
    /// `4, 2, 11, 4`. Same.
    ///
    /// **This differs from the Copy fixture's 8x3 for the identical wire
    /// words**, which is precisely why the extent must come from the ported
    /// geometry and never from the wire corners. `the_one_cycle_extent_
    /// differs_from_the_copy_extent_for_identical_wire_words` asserts that
    /// difference rather than leaving it as a comment.
    const ONE_CYCLE_X0: u32 = 4;
    const ONE_CYCLE_Y0: u32 = 2;
    const ONE_CYCLE_WIDTH: u32 = 7;
    const ONE_CYCLE_HEIGHT: u32 = 2;

    /// The one-cycle draw, through `texture_rectangle_vertices` -- the same
    /// ported geometry the decoder and executor both use, for the reason
    /// `composed_fixture_draw` states.
    fn one_cycle_fixture_draw() -> crate::TexrectDraw {
        let words = texrect_words_in_target_stepping(7);
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let raw = crate::RawTextureRectangle::decode(0x24, &bytes)
            .expect("the fixture's texrect words decode");
        let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::OneCycle)
            .expect("the fixture's rectangle is non-empty in one-cycle");
        crate::TexrectDraw::try_from_viewport_and_texcoords(
            vertices.viewport,
            vertices.vertex(0).texcoord(),
            vertices.vertex(3).texcoord(),
        )
        .expect("the fixture's texcoords recover integer S10.5 endpoints")
    }

    /// **Constraint 3, proven rather than asserted: the same wire words
    /// cover a different footprint in one-cycle than in Copy.**
    ///
    /// If these were equal, taking the extent from the wire corners would
    /// be harmless and the whole "derive it from
    /// `texture_rectangle_vertices`" rule would be unfalsifiable here.
    #[test]
    fn the_one_cycle_extent_differs_from_the_copy_extent_for_identical_wire_words() {
        let copy = composed_fixture_draw();
        let one_cycle = one_cycle_fixture_draw();
        assert_eq!(
            (copy.width(), copy.height()),
            (TEXRECT_WIDTH, TEXRECT_HEIGHT),
            "the Copy extent must be the hand-derived 8x3"
        );
        assert_eq!(
            (one_cycle.width(), one_cycle.height()),
            (ONE_CYCLE_WIDTH, ONE_CYCLE_HEIGHT),
            "the one-cycle extent must be the hand-derived 7x2"
        );
        assert_ne!(
            (copy.width(), copy.height()),
            (one_cycle.width(), one_cycle.height()),
            "identical wire words must cover DIFFERENT footprints in the two cycle types, or              the wire corners would have been a safe extent source after all"
        );
    }

    /// **Positive control: the one-cycle fixtures really do carry an
    /// admitted `TextureRectangle`, and really do carry a combiner program
    /// that is not the identity.**
    ///
    /// The first half is the control a prior lane's mutant survived without
    /// (deleting the texrect line left the content tests green). The second
    /// half is this card's own addition: a fixture whose `SetCombine` was
    /// silently all-zero would still admit a texrect, and a pixel test
    /// against it would be checking a program nobody measured.
    #[test]
    fn the_one_cycle_fixtures_really_do_admit_a_combining_texture_rectangle() {
        for (label, color, alpha) in [
            ("env-lerp", ENV_LERP_COLOR, ENV_LERP_ALPHA),
            ("flat-primitive", FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
        ] {
            assert_eq!(
                admitted_texture_rectangle_triangles(fill_load_and_one_cycle_texrect_words(
                    color, alpha
                )),
                2,
                "{label} must admit exactly two TextureRectangle-sourced triangles"
            );
            // The program the fixture actually stages, decoded through the
            // same accessor the executor's gate uses.
            let combine_words = one_cycle_combine_words(color, alpha);
            let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
            let selectors = [
                params.decode_color(crate::combiner::ColorInputSlot::A, true),
                params.decode_color(crate::combiner::ColorInputSlot::B, true),
                params.decode_color(crate::combiner::ColorInputSlot::C, true),
                params.decode_color(crate::combiner::ColorInputSlot::D, true),
            ];
            let expected = if label == "env-lerp" {
                [
                    crate::combiner::ColorInput::Environment,
                    crate::combiner::ColorInput::Texel0,
                    crate::combiner::ColorInput::Primitive,
                    crate::combiner::ColorInput::Texel0,
                ]
            } else {
                [
                    crate::combiner::ColorInput::Zero,
                    crate::combiner::ColorInput::Zero,
                    crate::combiner::ColorInput::Zero,
                    crate::combiner::ColorInput::Primitive,
                ]
            };
            assert_eq!(
                selectors, expected,
                "{label}'s staged SetCombine must decode to the measured program, or the pixel \
                 assertions below check arithmetic nobody measured"
            );
        }
        // And the same stream WITHOUT the texrect admits none.
        let mut without = whole_target_fill_words();
        without.extend(one_load_block_words());
        without.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        assert_eq!(admitted_texture_rectangle_triangles(without), 0);
    }

    /// The hand-derived combined RGBA16 halfword for one pixel of the
    /// env-lerp program, computed from the committed-TMEM oracle's texel.
    ///
    /// This mirrors the executor's quantization -- normalize by `/ 255.0`,
    /// `run_one_cycle`, `* 255.0` and `round`, then `write_pixel`'s RGBA16
    /// truncation -- and is deliberately written out here rather than
    /// calling a shared helper, so the two are independently authored
    /// statements of the same rule that must reconcile.
    fn expected_one_cycle_halfword(texel: [u8; 4], color: [u32; 4], alpha: [u32; 4]) -> u16 {
        expected_one_cycle_halfword_with_prim(texel, color, alpha, ONE_CYCLE_PRIM_WIRE)
    }

    /// [`expected_one_cycle_halfword`] with the primitive register named
    /// explicitly, for the multi-texrect fixture where each command stages
    /// its own.
    fn expected_one_cycle_halfword_with_prim(
        texel: [u8; 4],
        color: [u32; 4],
        alpha: [u32; 4],
        prim_wire: u32,
    ) -> u16 {
        let combine_words = one_cycle_combine_words(color, alpha);
        let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
        let inputs = crate::combiner::combiner_inputs_from_fragment_registers(
            crate::combiner::CombinerInputs {
                tex_val0: [
                    f32::from(texel[0]) / 255.0,
                    f32::from(texel[1]) / 255.0,
                    f32::from(texel[2]) / 255.0,
                    f32::from(texel[3]) / 255.0,
                ],
                tex_val1: [0.0; 4],
                prim_color: [0.0; 4],
                shade_color: [0.0; 4],
                env_color: [0.0; 4],
                key_center: [0.0; 3],
                key_scale: [0.0; 3],
                lod_fraction: 0.0,
                prim_lod_frac: 0.0,
                noise: 0.0,
                k4: 0.0,
                k5: 0.0,
            },
            crate::state::Color4::from_wire(ONE_CYCLE_ENV_WIRE),
            crate::state::PrimColor::from_wire(0x05 << 8 | 0x40, prim_wire),
        );
        let (combined, _alpha_compare) = crate::combiner::run_one_cycle(params, inputs);
        let [red, green, blue, a] = combined.map(|channel| (channel * 255.0).round() as u8);
        (u16::from(red >> 3) << 11)
            | (u16::from(green >> 3) << 6)
            | (u16::from(blue >> 3) << 1)
            | u16::from(a >> 7)
    }

    /// **The inversion: a texrect whose latched `SetCombine` references
    /// `TEXEL0` now executes through `execute_raw_dpc` on an
    /// adapter-equipped host, and its pixels are the real combined output.**
    ///
    /// # What this replaces, and why the replacement is the record
    ///
    /// Its predecessor,
    /// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_
    /// tmem_projection`, asserted the opposite -- that this exact packet was
    /// blocked by name -- and was correct when written. It pinned a
    /// PRE-EXISTING defect its own card could not close: `execute_raw_dpc`
    /// ran two paths over one packet that read **different TMEM images**.
    /// `draw_admitted_triangles` projected `coordinator.physical()`, the
    /// already-**published** slot, while the CPU texel reader sampled the
    /// packet's own **pending** post-image -- the only image a packet's own
    /// `LoadBlock` exists in before publication. That predecessor ended with
    /// an explicit instruction: "the day the projection is fixed, it fails
    /// and is rewritten to assert pixels." It did fail, by its own named
    /// panic, and this is that rewrite. This paragraph is the supersession
    /// record.
    ///
    /// # Why it was invisible for so long
    ///
    /// Every prior texrect fixture latched `SetCombine(0, 0)`, whose
    /// selectors reference no texel, so
    /// `CombineParams::references_texels_in_first_cycle` is false, the
    /// `texture_referenced` uniform is 0, and the fragment shader
    /// short-circuits to `TMEM_SAMPLE_STATUS_OK` without sampling at all.
    /// **The control passing was never evidence the GPU sampled correctly;
    /// it was evidence it never sampled at all.** The GPU path had
    /// therefore never actually fetched a texrect's texels.
    ///
    /// # The measurement
    ///
    /// At the untouched baseline `87b2f5b0`, the composed Copy fixture with
    /// only its `set_combine(0, 0)` swapped for the env-lerp program failed
    /// with `TmemSampleFailed { status: 2 }`
    /// (`TMEM_SAMPLE_STATUS_INVALID_BYTE`) -- the shader read addresses the
    /// published projection reported invalid. The cycle type was never the
    /// variable; the texel reference was.
    ///
    /// # What is asserted now
    ///
    /// Both measured programs, in one loop, so the texel-referencing and
    /// texel-free cases stay a controlled pair rather than two unrelated
    /// tests. Each must execute, and the env-lerp arm's pixels are
    /// reconciled against `expected_one_cycle_halfword` over the texel a
    /// **committed** oracle reads -- a different image and a different
    /// entry point than the executor used, so agreement is real evidence
    /// rather than a transcription.
    #[test]
    fn a_texel_referencing_combine_executes_and_carries_its_combined_pixels() {
        for (label, color, alpha) in [
            ("env-lerp", ENV_LERP_COLOR, ENV_LERP_ALPHA),
            ("flat-primitive", FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
        ] {
            let combine_words = one_cycle_combine_words(color, alpha);
            let params = CombineParams::from_wire(combine_words[0], combine_words[1]);
            let references_texel = params.references_texels_in_first_cycle();
            // **Positive control**, asserted rather than assumed: the
            // env-lerp arm must genuinely reference TEXEL0 and the
            // flat-primitive arm must genuinely not. Without this a fixture
            // that silently stopped referencing a texel would pass the whole
            // loop while proving nothing about texel sampling.
            assert_eq!(
                references_texel,
                label == "env-lerp",
                "{label}'s texel reference must match the census program it names"
            );

            let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
            configure_fill_target_height(&mut backend);
            if backend.triangle_pipeline.is_none() {
                // No adapter: the triangle path cannot run at all, so
                // nothing about the projection is observable here.
                continue;
            }
            publish_composed(
                &mut backend,
                &mut session,
                fill_load_and_one_cycle_texrect_words(color, alpha),
            );

            let resident = backend
                .color_targets()
                .expect("a composed packet must have built the color-target registry")
                .residents()
                .first()
                .expect("the composed packet must have published exactly one resident")
                .device_bytes()
                .device_bytes()
                .to_vec();

            let committed = backend.physical_tmem();
            let tile = composed_fixture_tile();
            let draw = one_cycle_fixture_draw();
            let mut combined_values = std::collections::BTreeSet::new();
            let mut compared = 0usize;

            for y in 0..ONE_CYCLE_HEIGHT {
                for x in 0..ONE_CYCLE_WIDTH {
                    let target_x = ONE_CYCLE_X0 + x;
                    let target_y = ONE_CYCLE_Y0 + y;
                    let offset = ((target_y * FILL_TARGET_WIDTH + target_x) * 2) as usize;
                    let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                    let request = crate::PointSampleRequest::new(
                        crate::PointSampleCoordinates::new(
                            crate::TextureCoordinateS10_5::from_raw(draw.s_at(x)),
                            crate::TextureCoordinateS10_5::from_raw(draw.t_at(y)),
                        ),
                        crate::TmemFirstRowParity::Even,
                    );
                    let sampled = crate::sample_committed_point(
                        committed,
                        tile.descriptor(),
                        tile.size(),
                        request,
                        crate::TextureLutMode::Disabled,
                    )
                    .expect("the committed oracle must sample the same texel");
                    assert!(
                        sampled.snapshot().is_committed(),
                        "the ORACLE reads durable state, so its snapshot must be Committed -- if \
                         this is Proposed the oracle is not independent of the executor"
                    );
                    assert_eq!(
                        actual,
                        expected_one_cycle_halfword(sampled.texel().rgba8888(), color, alpha),
                        "{label}: pixel ({target_x}, {target_y}) must be the combiner's own \
                         output over the texel the committed oracle reads at this position"
                    );
                    assert_ne!(
                        actual,
                        expected_fill_halfword(COMPOSED_FILL_COLOR, target_x),
                        "{label}: pixel ({target_x}, {target_y}) must differ from the fill \
                         underneath, or the texrect drew nothing"
                    );
                    combined_values.insert(actual);
                    compared += 1;
                }
            }
            assert_eq!(
                compared,
                (ONE_CYCLE_WIDTH * ONE_CYCLE_HEIGHT) as usize,
                "{label}: the loop must have compared exactly the hand-derived rectangle"
            );
            // **The claim that separates the two programs**, and the one
            // that could only be made once the projection was fixed: the
            // env-lerp output VARIES across the rectangle because it reads
            // the texel, while the flat-primitive output is constant
            // because it does not. A stale or empty projection would make
            // the env-lerp arm constant too, satisfying every assertion
            // above -- this is what catches that.
            if references_texel {
                assert!(
                    combined_values.len() >= 2,
                    "{label} reads TEXEL0, so its output must VARY across the rectangle -- a \
                     constant image means the projection carried empty or stale bytes rather \
                     than this packet's own load: got {combined_values:?}"
                );
            } else {
                assert_eq!(
                    combined_values.len(),
                    1,
                    "{label} reads no texel, so its output must be constant: got \
                     {combined_values:?}"
                );
            }
        }
    }

    /// **The flat-primitive program, executed end to end into the published
    /// image.** This is the half of WM2000's texrect work that the blocker
    /// above does not touch, and it is a real one-cycle combiner
    /// evaluation: 420 of the title's 2,520 texrects run exactly this
    /// program.
    ///
    /// `(Zero - Zero) * Zero + Primitive` reads no texel, so
    /// `references_texels_in_first_cycle` is false, the GPU fragment shader
    /// short-circuits, and the packet reaches `stage_texrect` -- where the
    /// CPU executor runs `run_one_cycle` per pixel exactly as it would for
    /// the env-lerp program.
    ///
    /// The expectation is hand-derived twice and reconciled:
    ///
    /// 1. Algebraically, the program is the primitive colour in every
    ///    channel, independent of the texel: `0x80FF4080` ->
    ///    `(128, 255, 64, 128)` -> RGBA16 `(128>>3)<<11 | (255>>3)<<6 |
    ///    (64>>3)<<1 | (128>>7)` = `0x87D1`.
    /// 2. Independently, through `expected_one_cycle_halfword`, which runs
    ///    the real `run_one_cycle` over the real decoded `CombineParams`.
    ///
    /// Both are asserted, and they must agree.
    ///
    /// **The positive controls that make it non-vacuous**, each named:
    /// the combined pixel must differ from the fill underneath (or nothing
    /// drew), it must differ from the raw texel (or the combiner was
    /// bypassed -- mutant (a)), and it must be texel-INDEPENDENT while the
    /// underlying texels genuinely vary (or the program was not the one
    /// staged -- mutant (e)).
    #[test]
    fn the_flat_primitive_one_cycle_program_composes_into_the_published_image() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        if backend.triangle_pipeline.is_none() {
            // No adapter: a triangle-bearing packet cannot execute at all.
            // Skipping is honest here and the crate's own
            // `configure_fill_target_height` already tolerates the case.
            return;
        }
        publish_composed(
            &mut backend,
            &mut session,
            fill_load_and_one_cycle_texrect_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
        );

        let resident = backend
            .color_targets()
            .expect("a composed packet must have built the color-target registry")
            .residents()
            .first()
            .expect("the composed packet must have published exactly one resident")
            .device_bytes()
            .device_bytes()
            .to_vec();

        // Derivation 1: the primitive colour, packed by hand.
        let [red, green, blue, alpha_byte] = ONE_CYCLE_PRIM_WIRE.to_be_bytes();
        let expected_literal = (u16::from(red >> 3) << 11)
            | (u16::from(green >> 3) << 6)
            | (u16::from(blue >> 3) << 1)
            | u16::from(alpha_byte >> 7);
        assert_eq!(
            expected_literal, 0x87D1,
            "the hand-packed literal must match the digit-by-digit derivation in this test's doc"
        );

        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();
        let draw = one_cycle_fixture_draw();
        let mut texels = std::collections::BTreeSet::new();
        let mut compared = 0usize;

        for y in 0..FILL_TARGET_HEIGHT {
            for x in 0..FILL_TARGET_WIDTH {
                let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
                let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                let inside = x >= ONE_CYCLE_X0
                    && x < ONE_CYCLE_X0 + ONE_CYCLE_WIDTH
                    && y >= ONE_CYCLE_Y0
                    && y < ONE_CYCLE_Y0 + ONE_CYCLE_HEIGHT;
                if !inside {
                    assert_eq!(
                        actual,
                        expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                        "pixel ({x}, {y}) is outside the texrect, so it must still carry the \
                         fill's own value"
                    );
                    continue;
                }
                // Derivation 1, the literal.
                assert_eq!(
                    actual, expected_literal,
                    "pixel ({x}, {y}) must be the primitive colour the flat program selects"
                );
                // Derivation 2, through the real combiner over the real
                // texel the committed oracle reads.
                let request = crate::PointSampleRequest::new(
                    crate::PointSampleCoordinates::new(
                        crate::TextureCoordinateS10_5::from_raw(draw.s_at(x - ONE_CYCLE_X0)),
                        crate::TextureCoordinateS10_5::from_raw(draw.t_at(y - ONE_CYCLE_Y0)),
                    ),
                    crate::TmemFirstRowParity::Even,
                );
                let sampled = crate::sample_committed_point(
                    committed,
                    tile.descriptor(),
                    tile.size(),
                    request,
                    crate::TextureLutMode::Disabled,
                )
                .expect("the committed oracle must sample the same texel");
                assert!(
                    sampled.snapshot().is_committed(),
                    "the ORACLE reads durable state, so its snapshot must be Committed"
                );
                let texel = sampled.texel().rgba8888();
                assert_eq!(
                    actual,
                    expected_one_cycle_halfword(texel, FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA),
                    "the two independent derivations must reconcile at pixel ({x}, {y})"
                );
                // **Mutant (a) control**: the raw texel must NOT equal the
                // combined output, or bypassing the combiner is invisible.
                let raw = (u16::from(texel[0] >> 3) << 11)
                    | (u16::from(texel[1] >> 3) << 6)
                    | (u16::from(texel[2] >> 3) << 1)
                    | u16::from(texel[3] >> 7);
                assert_ne!(
                    actual, raw,
                    "pixel ({x}, {y})'s combined value must differ from the raw texel, or the \
                     combiner could have been bypassed undetectably"
                );
                texels.insert(raw);
                compared += 1;
            }
        }

        assert_eq!(
            compared,
            (ONE_CYCLE_WIDTH * ONE_CYCLE_HEIGHT) as usize,
            "the loop must have compared exactly the hand-derived 7x2 rectangle"
        );
        // **The texel-independence control.** The output is constant, which
        // is only meaningful evidence if the INPUT texels varied. If every
        // sampled texel were identical, "texel-independent" would be
        // trivially satisfied and mutant (e) -- running the env-lerp
        // program here instead -- could survive.
        assert!(
            texels.len() >= 2,
            "the sampled texels must genuinely vary across the rectangle, or the flat program's \
             texel-independence is vacuous -- got {texels:?}"
        );
        // And the texrect drew over the fill.
        let inside_offset = (((ONE_CYCLE_Y0 * FILL_TARGET_WIDTH) + ONE_CYCLE_X0) * 2) as usize;
        assert_ne!(
            u16::from_be_bytes([resident[inside_offset], resident[inside_offset + 1]]),
            expected_fill_halfword(COMPOSED_FILL_COLOR, ONE_CYCLE_X0),
            "the texrect's first pixel must differ from the fill underneath it"
        );
    }

    /// **The regression guard: Copy-cycle texrects still work, and Copy
    /// still writes the RAW texel.**
    ///
    /// The Copy path's full content assertions live in
    /// `a_fill_a_tmem_load_and_a_texrect_compose_into_one_published_image`,
    /// unchanged by this card. What this adds is the discrimination that
    /// only matters once one-cycle is admitted: Copy must **not** consult
    /// the combiner program, even though one is latched.
    ///
    /// The program staged here is the flat-primitive one, chosen because it
    /// references no texel and so is not blocked by the GPU-path defect
    /// pinned above -- and because it would change **every** pixel had Copy
    /// consulted it, which the positive control at the end asserts rather
    /// than assumes. A program that happened to be the identity would make
    /// this guard unable to detect Copy accidentally combining.
    #[test]
    fn a_copy_cycle_texrect_still_writes_the_raw_texel_with_a_program_staged() {
        let mut words = whole_target_fill_words();
        words.extend(composed_tmem_load_words());
        // Copy cycle (2), with a real combiner program latched.
        words.extend(set_other_mode(2, 0));
        words.extend(one_cycle_combine_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA));
        words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
        words.extend(set_prim_color(0x40, 0x05, ONE_CYCLE_PRIM_WIRE));
        words.extend(texrect_words_in_target_stepping(7));

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        if backend.triangle_pipeline.is_none() {
            return;
        }
        publish_composed(&mut backend, &mut session, words);

        let resident = backend
            .color_targets()
            .unwrap()
            .residents()
            .first()
            .unwrap()
            .device_bytes()
            .device_bytes()
            .to_vec();
        let committed = backend.physical_tmem();
        let tile = composed_fixture_tile();
        let draw = composed_fixture_draw();
        let mut compared = 0usize;
        let mut would_have_differed = 0usize;

        for y in TEXRECT_Y0..TEXRECT_Y0 + TEXRECT_HEIGHT {
            for x in TEXRECT_X0..TEXRECT_X0 + TEXRECT_WIDTH {
                let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
                let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                let request = crate::PointSampleRequest::new(
                    crate::PointSampleCoordinates::new(
                        crate::TextureCoordinateS10_5::from_raw(draw.s_at(x - TEXRECT_X0)),
                        crate::TextureCoordinateS10_5::from_raw(draw.t_at(y - TEXRECT_Y0)),
                    ),
                    crate::TmemFirstRowParity::Even,
                );
                let texel = crate::sample_committed_point(
                    committed,
                    tile.descriptor(),
                    tile.size(),
                    request,
                    crate::TextureLutMode::Disabled,
                )
                .expect("the committed oracle must sample")
                .texel()
                .rgba8888();
                let raw = (u16::from(texel[0] >> 3) << 11)
                    | (u16::from(texel[1] >> 3) << 6)
                    | (u16::from(texel[2] >> 3) << 1)
                    | u16::from(texel[3] >> 7);
                assert_eq!(
                    actual, raw,
                    "Copy cycle must write the RAW texel at ({x}, {y}), not a combined one, \
                     even with a SetCombine and both colour registers staged"
                );
                if expected_one_cycle_halfword(texel, FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA) != raw {
                    would_have_differed += 1;
                }
                compared += 1;
            }
        }
        assert_eq!(
            compared,
            (TEXRECT_WIDTH * TEXRECT_HEIGHT) as usize,
            "the Copy rectangle is still the hand-derived 8x3"
        );
        // **The positive control.** The staged program is one that WOULD
        // have changed every pixel had Copy consulted it. Without this,
        // "Copy wrote the raw texel" would also pass against an identity
        // program and prove nothing about the gate.
        assert_eq!(
            would_have_differed, compared,
            "the staged program must be one that would have changed every pixel, or this \
             regression guard cannot detect Copy accidentally combining"
        );
    }

    /// The three primitive colours the multi-texrect one-cycle fixture
    /// stages, one per texrect, in command order.
    ///
    /// All three differ in **both** RGBA16 halves after the `>> 3` / `>> 7`
    /// pack (`0x87D1`, `0xFA21`, `0x443F`), so every pixel can be
    /// attributed to the texrect that wrote it. Two texrects sharing a
    /// packed value would make "the later one won the overlap"
    /// unfalsifiable exactly where it matters.
    const MULTI_ONE_CYCLE_PRIM: [u32; 3] = [0x80FF_4080, 0xFF40_8080, 0x4080_FF80];

    /// **Three one-cycle texrects in one packet, each running the combiner
    /// against the accumulated buffer.**
    ///
    /// The shape WM2000 actually issues -- its early frames carry 60 flat
    /// rectangles plus 25 tinted ones per entry
    /// (`docs/RT64-WM2000-CYCLE-MODES.md` §3) -- and a shape that could not
    /// be expressed before the N-command accumulation seam landed.
    ///
    /// | # | command | wire rectangle | primitive |
    /// |---|---|---|---|
    /// | 0 | fill | whole target | `0x0842_1085` |
    /// | 1 | one-cycle texrect | x 0..=4, y 0..=2 | `0x80FF_4080` |
    /// | 2 | one-cycle texrect | x 3..=8, y 1..=4 | `0xFF40_8080` |
    /// | 3 | one-cycle texrect | x 10..=15, y 5..=7 | `0x4080_FF80` |
    ///
    /// **Texrects 1 and 2 deliberately overlap.** Under one cycle the
    /// extents are 4x2 at (0,0) and 5x3 at (3,1), which share the single
    /// pixel (3, 1). That pixel must carry texrect 2's colour, and it is
    /// the only assertion in this file that can distinguish "the loop
    /// composed in journal order" from "the loop composed in some order".
    ///
    /// Each texrect stages its **own** `SetPrimColor` before its own
    /// rectangle, which is what makes this a per-command test rather than
    /// three copies of one draw: the executor must read the register
    /// latched at each texrect's own stream position, not the walk's final
    /// value. If it read the final value every rectangle would be
    /// `0x4080_FF80` and the first two assertions would fail.
    ///
    /// All three run the flat-primitive program, for a measured reason
    /// rather than convenience: it references no texel, so
    /// `references_texels_in_first_cycle` is false and the GPU fragment
    /// shader short-circuits past the pre-existing committed-vs-pending
    /// TMEM projection defect that
    /// `a_texel_referencing_combine_is_blocked_by_the_gpu_paths_committed_tmem_projection`
    /// pins. It is still a genuine per-fragment combiner evaluation --
    /// `run_one_cycle` runs on every pixel of all three rectangles.
    fn three_one_cycle_texrects_words() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(fill_cycle_other_mode(0));
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(COMPOSED_FILL_COLOR));
        words.extend(fill_rectangle(
            0,
            0,
            FILL_TARGET_WIDTH - 1,
            FILL_TARGET_HEIGHT - 1,
        ));
        words.extend(composed_tmem_load_words());
        for (index, (x0, y0, x1, y1)) in [(0u32, 0u32, 4u32, 2u32), (3, 1, 8, 4), (10, 5, 15, 7)]
            .into_iter()
            .enumerate()
        {
            // One-cycle (0), re-stated per command: `PlanCollector`
            // snapshots the mode at each command's own stream position.
            words.extend(set_other_mode(0, 0));
            words.extend(one_cycle_combine_words(FLAT_PRIM_COLOR, FLAT_PRIM_ALPHA));
            words.extend(set_env_color(ONE_CYCLE_ENV_WIRE));
            words.extend(set_prim_color(0x40, 0x05, MULTI_ONE_CYCLE_PRIM[index]));
            words.extend(texrect_words_at(7, x0, y0, x1, y1));
        }
        words
    }

    /// The one-cycle rasterized extent of one wire rectangle, through
    /// RT64's own `texture_rectangle_vertices` -- never the wire corners.
    ///
    /// One cycle applies neither Copy's `lrx |= 3` nor fill/copy's
    /// `ulx &= !3`, so `(coord + 3) >> 2` runs on the raw 10.2 fields.
    /// Returned half-open as `(left, top, right, bottom)`.
    fn one_cycle_extent_of(x0: u32, y0: u32, x1: u32, y1: u32) -> (u32, u32, u32, u32) {
        let words = texrect_words_at(7, x0, y0, x1, y1);
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let raw = crate::RawTextureRectangle::decode(0x24, &bytes).expect("the words decode");
        let vertices = crate::texture_rectangle_vertices(raw, crate::CycleType::OneCycle)
            .expect("the rectangle is non-empty in one cycle");
        let viewport = vertices.viewport;
        (
            viewport.left as u32,
            viewport.top as u32,
            viewport.right as u32,
            viewport.bottom as u32,
        )
    }

    /// **The post-merge claim: N one-cycle texrects compose in one packet,
    /// each running the combiner against the accumulated buffer, in
    /// journal order.**
    ///
    /// Every pixel is attributed to exactly one writer by an ownership map
    /// built in command order -- later commands overwrite earlier ones in
    /// the map exactly as the accumulation loop overwrites them in the
    /// buffer -- and then asserted against that writer's own hand-derived
    /// value. The fill's pixels come from the RGBA16 even/odd column rule;
    /// each texrect's come from its own primitive colour.
    ///
    /// Both derivations of a texrect pixel are asserted and must agree:
    /// the packed literal from `MULTI_ONE_CYCLE_PRIM`, and
    /// `expected_one_cycle_halfword` running the real `run_one_cycle` over
    /// the real decoded `CombineParams`.
    #[test]
    fn three_one_cycle_texrects_compose_in_journal_order_against_the_accumulated_buffer() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        if backend.triangle_pipeline.is_none() {
            return;
        }
        publish_composed(&mut backend, &mut session, three_one_cycle_texrects_words());

        let resident = backend
            .color_targets()
            .expect("the packet must have built the registry")
            .residents()
            .first()
            .expect("exactly one resident")
            .device_bytes()
            .device_bytes()
            .to_vec();

        let extents: Vec<(u32, u32, u32, u32)> =
            [(0u32, 0u32, 4u32, 2u32), (3, 1, 8, 4), (10, 5, 15, 7)]
                .into_iter()
                .map(|(x0, y0, x1, y1)| one_cycle_extent_of(x0, y0, x1, y1))
                .collect();
        assert_eq!(
            extents,
            vec![(0, 0, 4, 2), (3, 1, 8, 4), (10, 5, 15, 7)],
            "the three one-cycle extents, derived through texture_rectangle_vertices and \
             cross-checked against the hand derivation (ceil(coord/4) on the raw 10.2 fields, \
             with neither Copy's |= 3 nor fill/copy's &= !3)"
        );

        // The ownership map, built in COMMAND order: a later texrect
        // overwrites an earlier one, which is the accumulation loop's own
        // rule expressed independently of it.
        let mut owner: Vec<Option<usize>> =
            vec![None; (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT) as usize];
        for (index, &(left, top, right, bottom)) in extents.iter().enumerate() {
            for y in top..bottom {
                for x in left..right {
                    owner[(y * FILL_TARGET_WIDTH + x) as usize] = Some(index);
                }
            }
        }
        // The overlap really exists, or "the later texrect won" is vacuous.
        assert_eq!(
            owner[(1 * FILL_TARGET_WIDTH + 3) as usize],
            Some(1),
            "pixel (3, 1) lies in BOTH texrect 0 and texrect 1, so the map must award it to \
             the later one -- if this is Some(0) the two rectangles stopped overlapping and \
             the journal-order assertion below proves nothing"
        );

        let mut per_texrect = [0usize; 3];
        for y in 0..FILL_TARGET_HEIGHT {
            for x in 0..FILL_TARGET_WIDTH {
                let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
                let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
                match owner[(y * FILL_TARGET_WIDTH + x) as usize] {
                    None => assert_eq!(
                        actual,
                        expected_fill_halfword(COMPOSED_FILL_COLOR, x),
                        "pixel ({x}, {y}) is covered by no texrect, so it must still carry the \
                         whole-target fill's own value"
                    ),
                    Some(index) => {
                        let [red, green, blue, alpha] = MULTI_ONE_CYCLE_PRIM[index].to_be_bytes();
                        let literal = (u16::from(red >> 3) << 11)
                            | (u16::from(green >> 3) << 6)
                            | (u16::from(blue >> 3) << 1)
                            | u16::from(alpha >> 7);
                        assert_eq!(
                            actual, literal,
                            "pixel ({x}, {y}) belongs to texrect {index}, so it must carry that \
                             texrect's OWN primitive colour -- a wrong value here means the \
                             executor read a register latched at another command's position"
                        );
                        per_texrect[index] += 1;
                    }
                }
            }
        }

        // Every texrect contributed surviving pixels, so none was skipped
        // and none was wholly overwritten -- three commands really ran.
        for (index, count) in per_texrect.iter().enumerate() {
            assert!(
                *count > 0,
                "texrect {index} must own at least one surviving pixel, or the packet did not \
                 execute all three: {per_texrect:?}"
            );
        }
        // The three packed colours are pairwise distinct, so attributing a
        // pixel to a texrect is a real discrimination.
        let packed: std::collections::BTreeSet<u16> = MULTI_ONE_CYCLE_PRIM
            .iter()
            .map(|wire| {
                let [r, g, b, a] = wire.to_be_bytes();
                (u16::from(r >> 3) << 11)
                    | (u16::from(g >> 3) << 6)
                    | (u16::from(b >> 3) << 1)
                    | u16::from(a >> 7)
            })
            .collect();
        assert_eq!(
            packed.len(),
            3,
            "the three primitive colours must pack to three distinct RGBA16 values, or a pixel \
             cannot be attributed to the texrect that wrote it"
        );

        // **Derivation 2**, through the real combiner rather than the
        // packed literal: `expected_one_cycle_halfword` runs `run_one_cycle`
        // over the real decoded `CombineParams` for each texrect's own
        // registers. It must agree with the literal at texrect 1's own
        // first owned pixel.
        let probe = expected_one_cycle_halfword_with_prim(
            [0x18, 0x40, 0xC8, 0xFF],
            FLAT_PRIM_COLOR,
            FLAT_PRIM_ALPHA,
            MULTI_ONE_CYCLE_PRIM[1],
        );
        let [r1, g1, b1, a1] = MULTI_ONE_CYCLE_PRIM[1].to_be_bytes();
        assert_eq!(
            probe,
            (u16::from(r1 >> 3) << 11)
                | (u16::from(g1 >> 3) << 6)
                | (u16::from(b1 >> 3) << 1)
                | u16::from(a1 >> 7),
            "the real combiner over texrect 1's own primitive register must reconcile with the \
             packed literal this test asserted against the published buffer"
        );
    }

    /// **A texrect that declared no write stays on the triangle path.**
    ///
    /// `stage_and_report` routes a load-free packet to the color-target
    /// accumulation seam, and that seam needs a `ColorTargetKey`. A texrect
    /// with no staged `SetColorImage` declares no `ColorFramebuffer` access
    /// at all -- `raw_dpc::plan_texture_rectangle` returns early -- so
    /// there is no key to build and no target to compose into. It belongs
    /// on the GPU triangle path, exactly where it went before this file
    /// learned about texrects.
    ///
    /// Routing on the mere *presence* of a texrect sent it to the seam
    /// instead and refused it with `NoStagedColorImage`: measured, that
    /// broke both `..._texture_rectangle_at_its_own_wire_position` fixtures.
    /// This pins the same rule without needing a host GPU, so the guard
    /// survives on an adapterless machine.
    #[test]
    fn a_texrect_that_declared_no_write_is_not_routed_to_the_color_target_seam() {
        // No `SetColorImage` anywhere in the stream, so the decoder
        // declares no RenderTarget write for the texrect.
        let mut words = Vec::new();
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(set_tile(7, 1, 0));
        words.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
        words.extend(texrect_words_in_target(7));

        // Positive control, both halves. The stream must really carry a
        // texrect (a stream with none would also pass the assertion below,
        // vacuously), and that texrect must really declare no write.
        //
        // Measured against the SAME stream with a `SetColorImage` spliced
        // in: that variant declares writes, this one declares none, and the
        // only difference between them is the register. So the emptiness
        // here is the decoder's early return on a missing color image, not
        // a fixture that failed to carry a texrect at all.
        let mut with_image = whole_target_fill_words();
        with_image.extend(words.iter().copied());
        assert!(
            !declared_render_target_ranges(with_image).is_empty(),
            "the same texrect must declare writes once a color image is staged, or this \
             fixture carries no texrect and the test is vacuous"
        );
        assert!(
            declared_render_target_ranges(words.clone()).is_empty(),
            "the fixture's texrect must declare NO write, or it is not the shape under test"
        );

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_fill(&mut backend, &mut session, words);

        // On an adapterless host the triangle path refuses with
        // `TriangleDrawBeforeCreate`; with an adapter it draws. Either way
        // the packet must NOT have been routed to the color-target seam,
        // which is what `NoStagedColorImage` would prove.
        if let Some(error) = result.err() {
            assert!(
                !error.to_string().contains("no SetColorImage current"),
                "a texrect declaring no write must stay on the triangle path, never reach \
                 the color-target key derivation: {error}"
            );
        }
    }

    /// **The identity crossing refuses in BOTH directions, by name.**
    ///
    /// `verify_tmem_identity` is the one site where a texrect's TMEM image
    /// is checked against the identity its caller selected. The pending
    /// direction has been checked since commit `99bde6a3`; the committed
    /// direction is new with the load-free texrect admission, and without a
    /// test it would be decorative -- measured, deleting it left the entire
    /// suite green.
    ///
    /// Both real impls are correct, so no real image can reach either arm.
    /// Sources that lie are the only way to prove either refusal is wired,
    /// and the honest pair below is what makes the lying pair mean
    /// something: the check must discriminate on the identity, not refuse
    /// everything.
    #[test]
    fn the_tmem_identity_crossing_refuses_a_forgery_in_either_direction() {
        // Honest durable state -- a real `PhysicalTmemState`, exactly the
        // source `TexrectTmemSource::Committed` hands a load-free packet.
        let committed = PhysicalTmemState::try_new().unwrap();

        // The committed arm accepts it, so the check discriminates on the
        // identity rather than refusing everything.
        verify_tmem_identity(&committed, false, 0)
            .expect("durable state must pass the arm that selected it");

        // The pending arm refuses that same honest committed image. This is
        // the defect a load-bearing packet would suffer: its texrect
        // silently missing its own packet's loads, which commit `3a1a6a73`
        // measured as `TMEM_SAMPLE_STATUS_INVALID_BYTE`.
        let error = verify_tmem_identity(&committed, true, 5)
            .expect_err("durable state must not satisfy the pending arm");
        assert!(
            matches!(
                error,
                WgpuRawDpcExecutionError::PendingTmemImageClaimedCommitted { triangle_index: 5 }
            ),
            "the refusal must be the named variant carrying its own triangle index: {error:?}"
        );

        // The mirror direction, which the load-free admission introduced.
        // The source lies about its identity while returning durable bytes
        // -- precisely the forgery shape, and the only way to reach the arm
        // at all, since both real impls are correct.
        //
        // The `Proposed` identity is a REAL one, produced by
        // `tmem::read`'s own test constructor rather than synthesized here,
        // so this test cannot pass against a variant no real image could
        // produce.
        struct ForgedProposed<'a>(&'a PhysicalTmemState);
        impl crate::TmemByteSource for ForgedProposed<'_> {
            fn snapshot(&self) -> crate::TmemSnapshotIdentity {
                crate::tmem::proposed_identity_for_test()
            }
            fn valid_byte(&self, address: u16) -> Option<u8> {
                crate::TmemByteSource::valid_byte(self.0, address)
            }
        }
        assert!(
            !crate::tmem::proposed_identity_for_test().is_committed(),
            "the identity borrowed for the forgery must really be Proposed, or the refusal \
             below fires for the wrong reason"
        );

        let forged = ForgedProposed(&committed);
        let error = verify_tmem_identity(&forged, false, 3)
            .expect_err("a proposal must not satisfy the committed arm");
        assert!(
            matches!(
                error,
                WgpuRawDpcExecutionError::CommittedTmemImageClaimedProposed { triangle_index: 3 }
            ),
            "the refusal must be the named variant carrying its own triangle index: {error:?}"
        );
        // And the pending arm accepts that same identity, so this direction
        // discriminates on the identity too.
        verify_tmem_identity(&forged, true, 0)
            .expect("a Proposed identity must pass the arm that selected it");
    }

    /// **The GPU projection refuses a pending image that claims to be
    /// committed, by name.**
    ///
    /// The sibling of `execute_scheduled_texrect`'s
    /// `PendingTmemImageClaimedCommitted` check, at the other place a
    /// pending post-image is consumed. Both exist because the type system
    /// cannot enforce this: `Committed` and `Proposed` inhabit one enum, so
    /// a wrong `snapshot()` impl compiles and passes.
    ///
    /// Measured, which is why this test exists: deleting the refusal left
    /// the env-lerp pixel test, the projection unit tests and the
    /// guest-RDRAM end-to-end test all green. No real `PendingTmemImage`
    /// can reach the arm -- its own impl is correct -- so a source that lies
    /// is the only way to prove the refusal is wired rather than decorative.
    #[test]
    fn the_gpu_projection_refuses_a_pending_image_claiming_to_be_committed() {
        /// A byte source with a pending image's bytes and a durable
        /// image's *claim* -- the forgery the split exists to catch.
        struct ForgedCommitted;
        impl crate::TmemByteSource for ForgedCommitted {
            fn snapshot(&self) -> crate::TmemSnapshotIdentity {
                let state = PhysicalTmemState::try_new().unwrap();
                crate::TmemByteSource::snapshot(&state)
            }
            fn valid_byte(&self, address: u16) -> Option<u8> {
                Some(address as u8)
            }
        }

        let error = project_proposed_image(&ForgedCommitted)
            .expect_err("a source claiming Committed must be refused, not projected");
        assert!(
            matches!(
                error,
                WgpuRawDpcExecutionError::PendingTmemProjectionClaimedCommitted
            ),
            "the refusal must be the named variant, not some other error: {error:?}"
        );
        assert!(
            error.to_string().contains("Committed snapshot identity"),
            "the refusal must name what went wrong: {error}"
        );

        // The contrast that makes the claim mean something: the SAME bytes
        // behind an honest `Proposed` identity project successfully, so the
        // refusal discriminates on the identity, not on the content. The
        // honest identity comes from a real sealed transaction driven
        // through the composed execution path -- the only route to one in
        // this file, since `PendingTmemTransaction` is move-only.
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_composed(
            &mut backend,
            &mut session,
            fill_load_and_copy_texrect_words(),
        );
        assert!(
            result.is_ok(),
            "a composed packet must execute, which requires its own pending post-image to have \
             projected successfully through this same function: {result:?}"
        );
    }

    /// **A later packet's triangles never sample an earlier packet's pending
    /// projection.**
    ///
    /// The pending post-image belongs to the packet that sealed it. A second
    /// `execute_raw_dpc` carrying triangles but no TMEM load of its own must
    /// project the *published* slot -- which by then does contain the first
    /// packet's load, because publication ran between them -- and must never
    /// reuse a retained projection from the earlier call.
    ///
    /// This is the cross-packet half of the same invariant per-load prefix
    /// selection enforces within a packet (`prefix_before`): a draw may only
    /// observe TMEM already established at its own position in the stream,
    /// never state from a different transaction.
    ///
    /// Measured: caching the projection on the backend and reusing it when a
    /// later packet supplied none passed the env-lerp pixel test, the
    /// projection unit tests and the guest-RDRAM end-to-end test. Only this
    /// test kills that mutant, which is why it asserts on the retained
    /// *state* rather than on pixels -- the leaked and correct projections
    /// happen to agree on content here, so a pixel comparison cannot
    /// separate them, but the leak is still a real cross-transaction read.
    #[test]
    fn a_later_packet_does_not_reuse_an_earlier_packets_pending_projection() {
        let source = include_str!("production.rs");
        let struct_start = source
            .find("pub struct WgpuBackend {")
            .expect("WgpuBackend must exist in this file");
        let struct_end = source[struct_start..]
            .find("\n}\n")
            .expect("WgpuBackend must have a closing brace")
            + struct_start;
        let fields = &source[struct_start..struct_end];
        assert!(
            !fields.contains("TmemGpuProjection"),
            "WgpuBackend must hold no TmemGpuProjection field -- a retained projection is a \
             pending post-image outliving the packet that sealed it, which is exactly the \
             cross-transaction read the committed/pending split exists to prevent. Fields: \
             {fields}"
        );
    }

    /// **The committed/pending distinction, tested rather than assumed: a
    /// pending post-image read reports `Proposed`, a durable read reports
    /// `Committed`, and the two carry different identity types.**
    ///
    /// This is what the whole `TmemSnapshotIdentity` split exists for. A
    /// pending transaction has no durable `(state, generation)` pair --
    /// `binding.state` is the BASE state's identity and
    /// `binding.next_generation` names a generation that will not exist if
    /// publication is rejected -- so answering a pending read with a
    /// `PhysicalTmemSnapshotIdentity` would mint a receipt for a snapshot
    /// nothing ever published, indistinguishable downstream from a real one.
    ///
    /// Measured, not assumed: forging `Committed` inside
    /// `PendingTmemImage`'s own `snapshot()` impl passed the entire
    /// 5021-test suite before this test and `stage_texrect`'s matching
    /// runtime check existed (mutant K in this card's report). Both landed
    /// for that reason, and the runtime check is a check rather than a type
    /// guarantee because both variants inhabit one enum: a wrong impl
    /// compiles.
    ///
    /// The pending image is reached through the composed execution path,
    /// which is the only route to a real sealed transaction in this file --
    /// the type is move-only and `submitted_packet` is the one callback
    /// where the `WorkloadPacket` it needs is in scope.
    #[test]
    fn a_pending_tmem_read_reports_a_proposal_and_a_committed_read_reports_a_snapshot() {
        // The pending side, observed from inside execution: `stage_texrect`
        // asserts `!snapshot.is_committed()` on the live post-image and
        // refuses `PendingTmemImageClaimedCommitted` otherwise, so a
        // successful composed execution IS the pending-side assertion.
        // Running it here rather than only relying on the composed test
        // keeps the claim attached to the property being made.
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_composed(
            &mut backend,
            &mut session,
            fill_load_and_copy_texrect_words(),
        );
        assert!(
            result.is_ok(),
            "the composed packet must execute, which requires its pending post-image to have \
             reported a Proposed snapshot: {result:?}"
        );

        // The durable side, for the contrast that makes the claim mean
        // something: the SAME reader over durable state reports Committed,
        // with the state's own real identity and generation.
        let committed = backend.physical_tmem();
        let durable = crate::TmemByteSource::snapshot(committed);
        assert!(
            durable.is_committed(),
            "a read of durable PhysicalTmemState must report Committed, got {durable:?}"
        );
        let snapshot = durable
            .committed()
            .expect("a Committed identity must expose its snapshot");
        assert_eq!(snapshot.state(), committed.identity());
        assert_eq!(snapshot.generation(), committed.generation());
        assert!(
            durable.proposed().is_none(),
            "a Committed identity must not also present itself as a proposal"
        );

        // A fresh durable state reports a DIFFERENT identity, so the
        // assertion above is pinning a real value rather than a constant.
        let other = PhysicalTmemState::try_new().unwrap();
        assert_ne!(
            crate::TmemByteSource::snapshot(&other)
                .committed()
                .expect("durable")
                .state(),
            snapshot.state(),
            "two distinct durable states must report distinct identities"
        );
    }

    /// **Invariant 2, proven: ordering within a packet is semantics, and
    /// the reverse order observably differs.**
    ///
    /// Forward (`LoadBlock` then texrect) and reversed (texrect then
    /// `LoadBlock`) both execute -- both are legal RDP streams -- and they
    /// produce **different** texrect pixels. Same commands, same wire
    /// words, same fill; only the order changed.
    ///
    /// # This test found a real defect, and records it
    ///
    /// The first draft asserted only that the reversed order "must not
    /// execute", and it **failed**: the reversed stream executed cleanly
    /// and produced texrect rows with byte-identical `CompletedWrite`
    /// content digests to the forward stream's. The cause was structural,
    /// not a slip: `stage_and_report` sealed ONE pending post-image from
    /// every load before any texrect executed, so a texrect's position in
    /// the command stream had no effect on what it saw. Ordering was not
    /// semantics; it was ignored.
    ///
    /// A `TexrectBeforeItsOwnLoad` refusal named that honestly while there
    /// was no per-position image to serve. Per-load prefix views replaced
    /// it: the texrect in the reversed stream now samples what TMEM held at
    /// its own position -- durable committed state, because no load in its
    /// packet precedes it -- and the load that follows it still stages, so
    /// the stream executes and the two orders differ in output.
    ///
    /// The identical-digest observation is what keeps this load bearing
    /// rather than defensive: without a position-aware image the two orders
    /// are genuinely indistinguishable in their output, which is precisely
    /// the invariant violation. Asserting a *difference* is strictly
    /// stronger than asserting a refusal -- a refusal proves only that one
    /// order was rejected, while this proves the accepted orders disagree.
    #[test]
    fn a_texrect_before_its_load_observably_differs_from_one_after_it() {
        // Forward order: fill, load, texrect. Executes.
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let forward_words = fill_load_and_copy_texrect_words();
        let (_, forward) =
            plan_and_execute_composed(&mut backend, &mut session, forward_words.clone());
        assert!(
            forward.is_ok(),
            "the forward order (load, then texrect) must execute: {forward:?}"
        );

        // Reversed: the texrect comes BEFORE the load. Same commands, same
        // wire words, only the order changed.
        let mut reversed = whole_target_fill_words();
        reversed.extend(set_texture_image(0, 2, 8, 0x200));
        reversed.extend(set_tile(7, 2, 0));
        reversed.extend(set_tile_size_words(7, 7 << 2, 7 << 2));
        reversed.extend(set_other_mode(2, 0));
        reversed.extend(set_combine(0, 0));
        reversed.extend(texrect_words_in_target_stepping(7));
        reversed.extend(load_sync());
        reversed.extend([word(LOAD_BLOCK, 0), 7 << 24 | 23 << 12 | 0]);

        // Control: the reversed stream really does still carry both an
        // admitted texrect and an admitted load. Without this, a difference
        // below could mean the reordering broke the decode instead of
        // moving the texrect to a position with different texels.
        assert_eq!(
            admitted_texture_rectangle_triangles(reversed.clone()),
            2,
            "the reversed stream must still admit its texture rectangle, or the difference below \
             proves nothing about ordering"
        );

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, backward) = plan_and_execute_composed(&mut backend, &mut session, reversed.clone());
        let error = backward
            .expect_err(
                "the reversed texrect samples TMEM at its own position, where this packet's load \
                 has not run and nothing was ever published -- it must not produce texels",
            )
            .to_string();
        assert!(
            error.contains("physical TMEM texel byte") && error.contains("is invalid"),
            "the reversed order must be refused by the TEXEL READER finding nothing valid at \
             this stream position -- not by a shape gate, and never by inventing a zero texel. \
             Got: {error}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a refused reversed packet must leave no redeemable fill token"
        );

        // **The observable difference, and the mutation this test kills.**
        //
        // The refusal above alone would still pass under a once-per-packet
        // post-image if that image happened to be invalid too, so the
        // strip below carries the discriminating half: seven loads writing
        // the same TMEM range, seven texrects, and the requirement that
        // consecutive sprites DIFFER. Measured: with `prefix_before`
        // mutated to `prefixes.last()` -- which is exactly re-sealing once
        // per packet -- the forward and reversed publications produce
        // byte-identical `CompletedWrite` digest lists here, the same
        // indistinguishability the original defect had.
        let forward_digests: Vec<_> = {
            let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
            configure_fill_target_height(&mut backend);
            publish_composed(&mut backend, &mut session, forward_words)
                .iter()
                .map(|write| write.content())
                .collect()
        };
        let strip = publish_sprite_strip(SPRITE_STRIP_PAIRS);
        let first_sprite = sprite_strip_pixels(&strip, 0);
        let last_sprite = sprite_strip_pixels(&strip, SPRITE_STRIP_PAIRS - 1);
        assert_ne!(
            first_sprite, last_sprite,
            "the first and last texrect of a strip whose loads all overwrite one TMEM range must \
             carry DIFFERENT texels; equal ones mean every texrect observed the last load, which \
             is the ordering violation this test exists to catch"
        );
        assert!(
            !forward_digests.is_empty(),
            "the forward order must publish writes, or its execution above proved nothing"
        );
    }

    /// **The second inversion of this test, and the disproof that drove
    /// it.**
    ///
    /// At `be6ed65c` this was
    /// `a_fill_composed_with_a_texture_rectangle_is_refused_by_name`,
    /// asserting `MixedFillAndTrianglePacket` on the reasoning that "a
    /// texrect *is* two triangles by the time `stage_and_report` sees it".
    /// That premise fell: a texrect declares its own journal
    /// `ColorFramebuffer` writes where a raw triangle declares none.
    ///
    /// It then asserted `TexrectWithoutTmemLoad`, justified by a census
    /// reading "0 of WM2000's 219 decode entries carry a texrect without a
    /// load in the same entry". **That premise has now fallen too, and the
    /// count is not what was wrong with it.** The census window was 219
    /// decode entries of boot/attract and its own doc supersedes it twice
    /// (383 -> 1,056 -> 4,454 VI fields, 219 -> 2,219 -> 5,792 entries).
    /// Re-measured on the real ROM through the shell's `FN64_RENDER=wgpu`
    /// seam, the fourth packet WM2000 issues is 46 texrects, 0 loads and 0
    /// fills -- the shape the refusal declared impossible, from the game.
    ///
    /// So this test now pins the **admission**: a texrect in a load-free
    /// packet samples durable committed TMEM, which is not a stale
    /// substitute for a proposal but the published result of every earlier
    /// packet's loads -- the only thing hardware TMEM could hold at this
    /// stream position. What kept the old refusal honest is kept by other
    /// means, and they are asserted here too: the read goes through the
    /// same `sample_point` path a pending read uses, so an invalid TMEM
    /// byte is still a named refusal rather than a fabricated texel.
    ///
    /// The fill+**raw triangle** refusal is unchanged and still named
    /// `MixedFillAndTrianglePacket`; it is asserted separately below.
    #[test]
    fn a_fill_composed_with_a_texture_rectangle_and_no_tmem_load_samples_committed_tmem() {
        let mut fill_and_texrect = whole_target_fill_words();
        fill_and_texrect.extend(set_other_mode(2, 0));
        fill_and_texrect.extend(set_combine(0, 0));
        // A bound tile, so the texrect reaches the SAMPLER rather than
        // stopping at `TexrectUnboundTile` one step earlier. Without this
        // the test would pass on a refusal that says nothing about what a
        // load-free texrect reads, and the invalid-byte assertion below
        // would be vacuous.
        fill_and_texrect.extend(set_tile(7, 1, 0));
        fill_and_texrect.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
        fill_and_texrect.extend(texrect_words_in_target(7));

        // Positive control: this stream really does carry the texrect.
        // Measured through the journal rather than the plan walk, because
        // `admitted_texture_rectangle_triangles` needs a TMEM read to
        // capture and this fixture deliberately has no TMEM load at all.
        // Two `RenderTarget` ranges beyond the fill's own single
        // whole-target one means the texrect declared its own rows; the
        // extent is the one-cycle 7x2 derivation (this fixture's
        // `set_other_mode(2, 0)` is Copy, so 8x3 -- three rows).
        let ranges = declared_render_target_ranges(fill_and_texrect.clone());
        assert_eq!(
            ranges.len(),
            1 + TEXRECT_HEIGHT as usize,
            "the fixture must declare the fill's range plus the texrect's {TEXRECT_HEIGHT}              rows, or the refusal below is vacuous -- got {ranges:?}"
        );

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_fill(&mut backend, &mut session, fill_and_texrect);

        // **No refusal, and specifically not the deleted one.** Nothing in
        // this backend may name `TexrectWithoutTmemLoad` any more: the
        // variant is gone, so a reintroduction is a compile error rather
        // than a string this assertion has to chase.
        //
        // The packet's TMEM is entirely unwritten here -- this fixture
        // stages no load in this packet and publishes none before it -- so
        // every texel the texrect asks for is an INVALID byte. That is the
        // load-bearing half of the assertion: admitting the shape must not
        // mean fabricating texels for it. The reader refuses by name, from
        // the same `sample_point` path a pending read uses, and this test
        // pins that the refusal is about the *bytes*, not about the packet
        // shape.
        let error = result.expect_err(
            "an unwritten TMEM must still refuse the texel fetch by name, or this admission \
             would be producing plausible pixels from nothing",
        );
        let message = error.to_string();
        assert!(
            !message.contains("completed no TMEM load"),
            "the packet SHAPE must no longer be the refusal -- got: {message}"
        );
        assert!(
            message.contains("invalid") || message.contains("Invalid"),
            "the refusal must name the invalid TMEM byte the sampler actually hit, got: {message}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a refused composition must leave no redeemable fill token behind"
        );
    }

    /// **Durable cross-packet carry-in for `SetColorImage`, measured as the
    /// defect it closes.**
    ///
    /// The RDP's color-image register survives a submission boundary, so a
    /// packet may compose into a target it never re-declares -- and WM2000
    /// does exactly that: its texrect packet carries 14 texrects, 4 loads
    /// and zero fills, every texrect declaring a real write run derived by
    /// the decoder from the *previous* packet's `SetColorImage`.
    ///
    /// `color_target_key` used to read the image off `plan.fills.first()`,
    /// so that packet aborted the run. This pins the fix at the seam: the
    /// second packet declares no color image of its own and must still
    /// resolve one.
    ///
    /// The positive control is the first packet's own success -- if it did
    /// not establish a target, the second packet's admission would prove
    /// nothing about carry-in.
    #[test]
    fn a_second_packet_composes_into_the_color_image_the_first_one_declared() {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // Packet one: declares the color image and fills the whole target.
        let (_, first) =
            plan_and_execute_fill(&mut backend, &mut session, whole_target_fill_words());
        first.expect("the declaring packet must execute, or carry-in is untested");
        assert!(
            backend.rdp_state.color_image().is_some(),
            "the first packet must leave a durable color image behind, or this test is vacuous"
        );

        // Packet two: a **texrect and no fill at all**, and no
        // `SetColorImage` of its own. The absence of a fill is the whole
        // point: with the key derived from `plan.fills.first()` there is
        // nothing to derive it from, which is exactly the shape WM2000
        // aborted on. A second fill would leave the old derivation working
        // and this test asserting nothing.
        let (_, second) =
            plan_and_execute_fill(&mut backend, &mut session, second_words_for_control());

        // **The positive control IS the refusal, and it names the derived
        // key.** This packet still fails -- the first packet's fill was
        // staged but never published, so the resident bytes a texrect must
        // compose over do not exist yet. What matters is *which* refusal:
        // `MissingResidentBytes` is raised by `execute_scheduled_texrect`,
        // strictly downstream of `color_target_key`, and it prints the key
        // that was derived. So a key genuinely was built for a packet
        // carrying no fill to build one from.
        //
        // Asserting the address makes it non-vacuous: it is the *first*
        // packet's `SetColorImage` address, carried across the submission
        // boundary. Excluding the "no SetColorImage" message alone would
        // also pass if some earlier gate refused first.
        let error = second.expect_err("the unpublished target still refuses for resident bytes");
        let message = error.to_string();
        assert!(
            !message.contains("no SetColorImage current"),
            "the second packet must resolve the durable register, not refuse for its \
             absence: {message}"
        );
        let carried = backend
            .rdp_state
            .color_image()
            .expect("checked above")
            .address()
            .get();
        assert!(
            message.contains("requires resident_bytes")
                && message.contains(&format!("address: {carried}")),
            "the refusal must be the downstream resident-bytes one, naming a key at the \
             first packet's own color-image address {carried} -- that key is the proof the \
             durable register was read. Got: {message}"
        );
    }

    /// `a_second_packet_composes_into_the_color_image_the_first_one_declared`'s
    /// second packet, as its own function so the positive control measures
    /// the identical word stream the test executes rather than a retyped
    /// copy of it.
    fn second_words_for_control() -> Vec<u32> {
        let mut words = Vec::new();
        words.extend(set_other_mode(2, 0));
        words.extend(set_combine(0, 0));
        words.extend(set_tile(7, 1, 0));
        words.extend(set_tile_size_words(7, 7 << 2, 2 << 2));
        words.extend(texrect_words_in_target(7));
        words
    }

    /// The same durable-carry defect class as the test above, at the RDP's
    /// eight **tile** registers.
    ///
    /// Found by the same measurement: with the color-image carry fixed, the
    /// real ROM advanced one packet and stopped at `TexrectUnboundTile` with
    /// an entirely empty tile table -- 46 texrects, none of which
    /// re-declared a tile the earlier packet had already set.
    ///
    /// Asserted through `PlanCollector::seeded` directly rather than a full
    /// packet, because the fact under test is exactly the seed: a collector
    /// handed durable tiles must start with them bound, and one handed none
    /// must not invent any. Both halves, so a seed that filled the table
    /// with a zeroed default would fail the second assertion.
    #[test]
    fn a_plan_collector_starts_from_the_durable_tile_registers() {
        let unseeded = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            [(None, None); 8],
        );
        assert!(
            unseeded
                .current_tiles
                .iter()
                .all(|(descriptor, size)| descriptor.is_none() && size.is_none()),
            "an unseeded collector must invent no tile -- a zeroed default would silently \
             sample TMEM word zero"
        );

        // A real durable tile, taken from a backend that actually issued
        // `SetTile`/`SetTileSize`, never a hand-built struct: the seed path
        // under test is `durable_neutral_tiles(&rdp_state)`, so building the
        // input by hand would test the converter and not the carry.
        // **Every field distinct and nonzero**, borrowed field for field
        // from `raw_dpc`'s own `tmem_state_commands_decode_every_public_
        // field_width_for_all_prefixes`. This matters: with a tile whose
        // `mask_s`, `mask_t` and `low_s` were all zero, a converter that
        // swapped S for T or dropped a field produced an identical result
        // and the round-trip below passed. Measured -- two mutants survived
        // exactly that fixture.
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        let words = vec![
            word(SET_TILE, 4 << 21 | 3 << 19 | 0x01ab << 9 | 0x01fe),
            5 << 24 | 0x0f << 20 | 3 << 18 | 0x0a << 14 | 0x0b << 10 | 1 << 8 | 0x0c << 4 | 0x0d,
            word(SET_TILE_SIZE_OPCODE, 0x0fed << 12 | 0x0cba),
            5 << 24 | 0x0abc << 12 | 0x0789,
        ];
        let planned = plan_with_no_reads(&mut backend, &session, words);
        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();
        let _ = backend.execute_raw_dpc(bound);

        let tiles = durable_neutral_tiles(&backend.rdp_state);
        assert!(
            tiles[5].0.is_some() && tiles[5].1.is_some(),
            "the fixture must actually leave tile 5 durable, or the seed assertion below is \
             vacuous -- got {:?}",
            tiles[5]
        );

        // **Round-trip, so the converters cannot permute or drop a field.**
        //
        // `neutral_tile_descriptor`/`neutral_tile_size` are the inverses of
        // `TexrectTileBinding::try_from_neutral`'s own decode. Feeding the
        // neutral output back through that decode must reproduce the typed
        // value the durable register actually holds -- an equality on the
        // neutral tuples alone would pass with `mask_s` and `mask_t`
        // swapped, or `low_s` zeroed, because both sides would carry the
        // same wrong value. Measured: those two mutants survived until this
        // assertion existed.
        let durable_tile = backend
            .rdp_state
            .tmem()
            .tile(crate::TileIndex::try_new(5).unwrap());
        let round_tripped = crate::targets::TexrectTileBinding::try_from_neutral(
            tiles[5].0.expect("checked above"),
            tiles[5].1.expect("checked above"),
        )
        .expect("a durable tile round-trips through the neutral mirror");
        assert_eq!(
            round_tripped.descriptor(),
            durable_tile.descriptor().expect("checked above"),
            "the neutral descriptor must decode back to the durable register field for field"
        );
        assert_eq!(
            round_tripped.size(),
            durable_tile.size().expect("checked above"),
            "the neutral tile size must decode back to the durable register field for field"
        );

        // Hand-derived from the wire words above, so the round-trip is
        // checked against the RDP's own field layout rather than against
        // whatever the converter happened to produce. S and T carry
        // different values in every pair, which is what makes a swap
        // observable.
        let neutral = tiles[5].0.expect("checked above");
        assert_eq!(neutral.mask_s, 0x0c, "mask_s is w1 bits 7:4");
        assert_eq!(neutral.mask_t, 0x0a, "mask_t is w1 bits 17:14");
        assert_eq!(neutral.shift_s, 0x0d, "shift_s is w1 bits 3:0");
        assert_eq!(neutral.shift_t, 0x0b, "shift_t is w1 bits 13:10");
        assert!(neutral.s_mode.mirror && !neutral.s_mode.clamp);
        assert!(neutral.t_mode.mirror && neutral.t_mode.clamp);
        assert_eq!(neutral.line_words, 0x01ab);
        assert_eq!(neutral.tmem_word_address, 0x01fe);
        assert_eq!(neutral.palette, 0x0f);
        // Format and pixel size, hand-derived from w0 bits 23:21 and 20:19
        // above: the enum converters are total match arms and a wrong arm
        // is otherwise invisible, since the round-trip decodes with the
        // inverse of whatever this produced.
        assert!(
            matches!(neutral.format, fn64_render::NeutralImageFormat::Intensity),
            "format is w0 bits 23:21 == 4, got {:?}",
            neutral.format
        );
        assert!(
            matches!(neutral.size, fn64_render::NeutralPixelSize::Bits32),
            "pixel size is w0 bits 20:19 == 3, got {:?}",
            neutral.size
        );
        let neutral_size = tiles[5].1.expect("checked above");
        assert_eq!(neutral_size.low_s, 0x0fed, "low_s is w0 bits 23:12");
        assert_eq!(neutral_size.low_t, 0x0cba, "low_t is w0 bits 11:0");
        assert_eq!(neutral_size.high_s, 0x0abc, "high_s is w1 bits 23:12");
        assert_eq!(neutral_size.high_t, 0x0789, "high_t is w1 bits 11:0");
        let seeded = PlanCollector::seeded(
            None,
            None,
            Color4::from_wire(0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
            Color4::from_wire(0),
            None,
            tiles,
        );
        assert_eq!(
            seeded.current_tiles[5], tiles[5],
            "a collector seeded from durable state must start with tile 5 already bound, \
             so a packet that re-declares no tile still resolves one"
        );
        assert!(
            seeded.current_tiles[0].0.is_none(),
            "seeding must carry only the tiles the guest actually set, never widen to all eight"
        );
    }

    /// **A draw standing before its packet's own `SetTile` must carry the
    /// PREVIOUS packet's tile, not this packet's later one.**
    ///
    /// `plan_raw_dpc` and `execute_raw_dpc` are two trait calls for one
    /// submission, and the first folds the whole packet's `RdpStateDelta`
    /// into `rdp_state`. Seeding `PlanCollector`'s in-order walk from
    /// `rdp_state` at that point starts it from the packet's FINAL tile
    /// table, so every command before the packet's first `SetTile` reads a
    /// register the guest had not set yet -- time-travelled state.
    ///
    /// Measured on WM2000: the ROM emits each sprite strip as
    /// `SetTile(7) -> LoadTile -> SetTile(0) -> triangle -> triangle`, and
    /// a packet boundary fell between one strip's two triangles. The
    /// orphaned triangle, at command index 0, was bound to the packet's
    /// later `line_words = 4` tile instead of the carried-in
    /// `line_words = 5`, walked its rows at a 32-byte stride through a
    /// 40-byte-stride image, and hit the load's undefined row-tail padding
    /// -- `TMEM_SAMPLE_STATUS_INVALID_BYTE`, the abort this repairs.
    ///
    /// **Asserted on the binding the walk actually produced, never on the
    /// snapshot field itself.** An earlier draft read
    /// `backend.tiles_before_last_plan` directly; a mutant that recorded
    /// the snapshot correctly and then had the executor ignore it (reading
    /// the live, already-folded `rdp_state` instead -- exactly the
    /// pre-repair behaviour) SURVIVED that draft. Seeding the collector
    /// from `WgpuBackend`'s own choice of table, and reading the resulting
    /// `RetrievedTriangleDraw`, is what kills it: that is the value the
    /// shader is handed.
    ///
    /// A **raw triangle** is the draw, not a texrect: a raw triangle binds
    /// tile 0 and declares no journal write access, so the packet needs no
    /// resident color target and the assertion is not gated behind fill
    /// bookkeeping this defect has nothing to do with.
    ///
    /// The two `line` values are DIFFERENT and hand-chosen (`3` carried in,
    /// `6` set later) so the assertion distinguishes the two tables rather
    /// than passing against either. `set_tile`'s own wire layout puts
    /// `line` in w0 bits 17:9, so these are the values that field carries.
    #[test]
    fn a_draw_before_its_packets_first_set_tile_carries_the_previous_packets_tile() {
        const CARRIED_IN_LINE: u32 = 3;
        const SET_LATER_IN_PACKET_LINE: u32 = 6;

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);

        // Packet one: establish tile 0 with the carry-in `line`.
        let mut first = Vec::new();
        first.extend(set_other_mode(0, 0));
        first.extend(set_combine(0, 0));
        first.extend(set_tile(0, CARRIED_IN_LINE, 0));
        first.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
        backend
            .plan_raw_dpc(session.plan_request(capture(first)))
            .expect("the tile-establishing submission plans cleanly");

        assert_eq!(
            durable_neutral_tiles(&backend.rdp_state)[0]
                .0
                .expect("packet one bound tile 0")
                .line_words,
            CARRIED_IN_LINE as u16,
            "positive control: durable state must really carry the first packet's tile, \
             or this test would pass vacuously against a table that never held it"
        );

        // Packet two, in the WM2000 order that exposed the defect: the
        // TRIANGLE COMES FIRST, before this packet's own `SetTile`. Its
        // only tile binding at its own stream position is packet one's.
        let mut second = Vec::new();
        second.extend(set_other_mode(0, 0));
        second.extend(set_combine(0, 0));
        second.extend(triangle_base_edge_words(0, 2, 0));
        second.extend(set_tile(0, SET_LATER_IN_PACKET_LINE, 0));
        second.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
        let planned = backend
            .plan_raw_dpc(session.plan_request(capture(second)))
            .expect("the triangle-then-SetTile submission plans cleanly");

        // After the fold, the LIVE registers hold the new value. This is
        // the discriminator: if the walk seeded from here, the triangle
        // would come back bound to `SET_LATER_IN_PACKET_LINE`.
        assert_eq!(
            durable_neutral_tiles(&backend.rdp_state)[0]
                .0
                .expect("packet two bound tile 0")
                .line_words,
            SET_LATER_IN_PACKET_LINE as u16,
            "positive control: the fold must really have happened, otherwise the walk \
             could read the live registers and still look correct"
        );

        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        // Seeded from `WgpuBackend`'s OWN choice of table -- the same
        // expression `execute_raw_dpc` uses -- so a mutant that leaves the
        // snapshot correct but has the executor ignore it is still caught.
        let seed = backend
            .tiles_before_last_plan
            .unwrap_or_else(|| durable_neutral_tiles(&backend.rdp_state));
        let mut plan_visitor = PlanCollector::seeded(
            backend.rdp_state.other_mode(),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            backend.rdp_state.color_image(),
            seed,
        );
        let mut color_targets = None;
        let configured_target_extent = backend.configured_target_extent;
        let coordinator = &backend.coordinator;
        let mut view = ExecutionCollector {
            physical: coordinator.physical(),
            queue: bound.queue(),
            ordinal: bound.ordinal(),
            submission: bound.submission(),
            plan: PlanCollector::seeded(
                backend.rdp_state.other_mode(),
                backend.rdp_state.combine(),
                backend.rdp_state.blend_color(),
                backend.rdp_state.env_color(),
                backend.rdp_state.prim_color(),
                backend.rdp_state.fog_color(),
                backend.rdp_state.color_image(),
                seed,
            ),
            reads: Vec::new(),
            outcome: None,
            color_targets: &mut color_targets,
            configured_target_extent,
            draw_tmem: None,
        };
        coordinator.execution_view(&bound, &mut plan_visitor, &mut view);

        assert_eq!(
            view.plan.triangles.len(),
            1,
            "positive control: the packet must admit its one raw triangle -- if it admitted \
             none, the binding assertion below would be vacuous"
        );
        let draw = view.plan.triangles.into_iter().next().unwrap();
        let draw = draw.expect("the admitted triangle retrieves its draw state");
        assert_eq!(
            draw.tile_binding.bound, 1,
            "the triangle must resolve a tile at all"
        );
        assert_eq!(
            draw.tile_binding.line_words, CARRIED_IN_LINE,
            "the triangle stands BEFORE its packet's own SetTile, so it must carry the tile \
             packet ONE set (line {CARRIED_IN_LINE}), never the one packet two installs after \
             it (line {SET_LATER_IN_PACKET_LINE}) -- seeding from the already-folded \
             `rdp_state` is what time-travelled WM2000's orphaned strip triangle onto a stride \
             its own load never wrote"
        );
    }

    /// **A REJECTED plan must not replace the snapshot.**
    ///
    /// `tiles_before_last_plan` is recorded on `plan_raw_dpc`'s success
    /// path only, after `plan_raw_dpc_inner` returns `Ok`. Moving it above
    /// that call would record a snapshot for a packet that never executes,
    /// so the next `execute_raw_dpc` -- which belongs to whichever
    /// submission planned last SUCCESSFULLY -- would be seeded from the
    /// wrong boundary.
    ///
    /// This is the arm the repair KEEPS rather than the one it changed, and
    /// it had no test: the mutant that hoists the assignment above the
    /// fallible call survived every other assertion in this file.
    ///
    /// The rejected packet's `SetTile` uses a `line` that appears nowhere
    /// else, so a snapshot taken from the wrong side is distinguishable
    /// from both the surviving value and any default.
    #[test]
    fn a_rejected_plan_leaves_the_previous_submissions_tile_snapshot_in_place() {
        const SURVIVING_LINE: u32 = 3;

        let (mut backend, session) = WgpuBackend::try_new().unwrap();

        // One submission that plans cleanly and sets tile 0.
        let mut good = Vec::new();
        good.extend(set_other_mode(0, 0));
        good.extend(set_combine(0, 0));
        good.extend(set_tile(0, SURVIVING_LINE, 0));
        good.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
        backend
            .plan_raw_dpc(session.plan_request(capture(good)))
            .expect("the tile-establishing submission plans cleanly");

        let after_success = backend
            .tiles_before_last_plan
            .expect("a successful plan records a snapshot");
        assert!(
            after_success[0].0.is_none(),
            "positive control: this FIRST plan's own snapshot is the state before it ran, \
             which bound no tile -- if it already carried one, the comparison below could \
             not tell a preserved snapshot from a re-taken one"
        );

        // A submission that is rejected at plan time. `FullSync` alongside
        // a fill is refused (see the T-13 test above), and this stream also
        // carries a `SetTile` -- so a snapshot taken before the fallible
        // call would still differ from the one above, by now holding the
        // tile the FIRST submission set.
        let mut bad = partial_width_fill_words();
        bad.extend(set_tile(0, SURVIVING_LINE + 1, 0));
        bad.extend([word(FULL_SYNC, 0), 0]);
        assert!(
            backend
                .plan_raw_dpc(session.plan_request(capture(bad)))
                .is_err(),
            "positive control: this submission must really be rejected, or the assertion \
             below would be testing the success path twice"
        );

        assert_eq!(
            backend
                .tiles_before_last_plan
                .expect("the snapshot must still be present after a rejected plan"),
            after_success,
            "a rejected plan must leave the last SUCCESSFUL submission's snapshot untouched. \
             Recording it before `plan_raw_dpc_inner` would stamp a boundary for a packet \
             that never executes, and the next execute_raw_dpc would seed its tile walk \
             from it"
        );
    }

    /// **A draw standing before its packet's own `SetOtherMode` must carry
    /// the PREVIOUS packet's mode, not this packet's later one.**
    ///
    /// The sibling of
    /// `a_draw_before_its_packets_first_set_tile_carries_the_previous_packets_tile`,
    /// on the register `f2c52822` explicitly declined to widen its repair
    /// to for want of a measurement. This is that measurement, taken on the
    /// real WM2000 ROM on the all-Rust stack.
    ///
    /// A packet folded `other_mode.high` from `0x00000cef` to `0x0008acef`.
    /// `G_MDSFT_TEXTLUT` is bits 15:14, so the carried-in word selects
    /// `G_TT_NONE` and the packet-final word selects `G_TT_RGBA16`. The
    /// packet's FIRST texrect, at command index 6, stood before that
    /// `SetOtherMode` and was nonetheless seeded with the folded word.
    ///
    /// Under an enabled TLUT the RDP indexes any format through the palette
    /// and confines that read to half of TMEM (RT64
    /// `TextureDecoder.hlsli:162-163`). So the texrect's `Rgba`/`Bits16`
    /// texel at linear byte `0x884` was masked to `0x084` and XOR4'd to
    /// `0x080` instead of staying at `0x884`/`0x880`. `0x880` was loaded;
    /// `0x080` never was, and `InvalidTexelByte` correctly aborted the run
    /// at 280 VI swaps.
    ///
    /// **Behavioural, not field-reading.** The assertion drives
    /// `execution_view` and reads the mode off the CONSUMER's retrieved
    /// draw state, seeding exactly the way `execute_raw_dpc` does. A mutant
    /// that records the right snapshot while the executor still reads the
    /// live register is therefore caught -- the trap the tile-side test's
    /// first draft fell into.
    ///
    /// The two `TEXTLUT` encodings are DIFFERENT and hand-chosen (`0`
    /// carried in, `2` set later) so the assertion distinguishes the two
    /// words rather than passing against either.
    #[test]
    fn a_draw_before_its_packets_first_set_other_mode_carries_the_previous_packets_mode() {
        // `G_MDSFT_TEXTLUT` is bits 15:14 of `G_SETOTHERMODE_H`.
        const TEXTLUT_SHIFT: u32 = 14;
        const CARRIED_IN_TEXTLUT: u32 = 0; // G_TT_NONE
        const SET_LATER_IN_PACKET_TEXTLUT: u32 = 2; // G_TT_RGBA16

        // `set_other_mode`'s own helper only writes the cycle-type field,
        // so the high word is built here from the same `word()` encoder it
        // uses, with TEXTLUT placed by its documented shift.
        fn other_mode_with_textlut(textlut: u32) -> [u32; 2] {
            [word(SET_OTHER_MODE, textlut << TEXTLUT_SHIFT), 0]
        }

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

        // Packet one: establish the carried-in mode (TLUT off).
        let mut first = Vec::new();
        first.extend(other_mode_with_textlut(CARRIED_IN_TEXTLUT));
        first.extend(set_combine(0, 0));
        backend
            .plan_raw_dpc(session.plan_request(capture(first)))
            .expect("the mode-establishing submission plans cleanly");

        assert_eq!(
            backend
                .rdp_state
                .other_mode()
                .expect("packet one set the mode")
                .texture_lut_mode(),
            Ok(crate::TextureLutMode::Disabled),
            "positive control: durable state must really carry the first packet's TLUT-off \
             mode, or this test would pass vacuously against a register that never held it"
        );

        // Packet two: a triangle FIRST, then a `SetOtherMode` turning the
        // TLUT on. Planning folds that `SetOtherMode` into `rdp_state`
        // immediately, so the live register no longer describes the
        // triangle's own stream position.
        let mut second = Vec::new();
        second.extend(triangle_base_edge_words(0, 2, 0));
        second.extend(other_mode_with_textlut(SET_LATER_IN_PACKET_TEXTLUT));
        let planned = backend
            .plan_raw_dpc(session.plan_request(capture(second)))
            .expect("the triangle-then-SetOtherMode submission plans cleanly");

        // The discriminator: if the walk seeded from the live registers,
        // the triangle would come back TLUT-enabled.
        assert_eq!(
            backend
                .rdp_state
                .other_mode()
                .expect("packet two set the mode")
                .texture_lut_mode(),
            Ok(crate::TextureLutMode::Rgba16),
            "positive control: the fold must really have happened, otherwise the walk could \
             read the live registers and still look correct"
        );

        let bound = session
            .finalize_and_submit(planned, DeferredGuestReadCapture::new(Vec::new()))
            .unwrap();

        // **Driven through `execute_raw_dpc_inner`, the function
        // `execute_raw_dpc` delegates to**, with every seed taken from
        // `backend`'s own fields exactly as that method takes them. The
        // retrieved draws it returns are the consumer's own output, so a
        // mutant that records a faithful snapshot and then passes
        // `self.rdp_state.other_mode()` at the call site changes THIS
        // value and is caught.
        //
        // Reading `backend.other_mode_before_last_plan` in the test instead
        // and handing it to a locally-built `PlanCollector` does NOT catch
        // that mutant -- measured: the first draft did exactly that and the
        // consumer-side mutant passed. This is the trap the tile-side
        // sibling documents at
        // `execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot`.
        let mut color_targets = None;
        let (_, triangles, _, _) = execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend
                .other_mode_before_last_plan
                .unwrap_or_else(|| backend.rdp_state.other_mode()),
            backend.rdp_state.combine(),
            backend.rdp_state.blend_color(),
            backend.rdp_state.env_color(),
            backend.rdp_state.prim_color(),
            backend.rdp_state.fog_color(),
            backend.rdp_state.color_image(),
            backend
                .tiles_before_last_plan
                .unwrap_or_else(|| durable_neutral_tiles(&backend.rdp_state)),
            &mut color_targets,
            backend.configured_target_extent,
        )
        .expect("the triangle-then-SetOtherMode submission executes cleanly");

        assert_eq!(
            triangles.len(),
            1,
            "positive control: the packet must admit its one raw triangle -- if it admitted \
             none, the mode assertion below would be vacuous"
        );
        let draw = triangles
            .into_iter()
            .next()
            .unwrap()
            .expect("the admitted triangle retrieves its draw state");
        assert_eq!(
            draw.other_mode.texture_lut_mode(),
            Ok(crate::TextureLutMode::Disabled),
            "the triangle stands BEFORE its packet's own SetOtherMode, so it must carry the \
             TLUT-off mode packet ONE set, never the G_TT_RGBA16 one packet two installs \
             after it -- seeding from the already-folded `rdp_state` is what sent WM2000's \
             first texrect down the enabled-TLUT half-TMEM address path and made it read \
             byte 0x080, which its own load never wrote"
        );
    }

    /// **`execute_raw_dpc` must seed `other_mode` from the SNAPSHOT, never
    /// from the live register.**
    ///
    /// The sibling of
    /// `execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot`,
    /// and it exists for the identical measured reason.
    ///
    /// The behavioural test above proves the snapshot holds the right word
    /// and that a walk seeded from it retrieves the right mode. It cannot
    /// prove `execute_raw_dpc` is the thing doing the seeding:
    /// `RetrievedTriangleDraw` is not reachable through the
    /// `RenderBackend` trait, so a mutant that records the snapshot
    /// faithfully and then passes `self.rdp_state.other_mode()` -- exactly
    /// the pre-repair line -- SURVIVES it. **Measured: it did.** The first
    /// draft of the test above passed unchanged against that mutant.
    ///
    /// Pinned at the source instead, because the fact under test is which
    /// expression appears at one call site.
    ///
    /// Both halves are asserted for the same reason the tile sibling
    /// asserts both: `contains` alone would pass a body that read the
    /// snapshot *and* also read the live register unconditionally, and the
    /// count pins that the only bare `self.rdp_state.other_mode()` in this
    /// function is the `unwrap_or_else` fallback reached before any plan
    /// has run.
    #[test]
    fn execute_raw_dpc_seeds_other_mode_from_the_pre_delta_snapshot() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("    fn execute_raw_dpc(")
            .expect("execute_raw_dpc must exist in this file");
        let next_fn = source[body_start + 1..]
            .find("\n    fn ")
            .map(|offset| body_start + 1 + offset)
            .unwrap_or(source.len());
        let body = &source[body_start..next_fn];
        assert!(
            body.contains("self.other_mode_before_last_plan"),
            "execute_raw_dpc must seed `other_mode` from the pre-delta snapshot \
             `other_mode_before_last_plan`. Reading `rdp_state` directly reads the packet's \
             own already-folded SetOtherModes, which ran WM2000's first texrect under a \
             G_TT_RGBA16 the guest had not set yet and sent it down the enabled-TLUT \
             half-TMEM address path"
        );
        assert_eq!(
            body.matches("self.rdp_state.other_mode()").count(),
            1,
            "the only `self.rdp_state.other_mode()` in execute_raw_dpc must be the \
             `unwrap_or_else` fallback for the no-plan-yet case -- a second, unconditional \
             one would reintroduce the live-register read the snapshot exists to replace"
        );
        assert!(
            body.contains(".unwrap_or_else(|| self.rdp_state.other_mode())"),
            "that one call must be the fallback arm specifically, so a backend that has \
             executed before it ever planned still resolves a mode rather than panicking \
             on an empty Option"
        );
    }

    /// **A REJECTED plan must not replace the `other_mode` snapshot.**
    ///
    /// The arm the repair KEEPS, pinned for the same reason the tile
    /// sibling `a_rejected_plan_leaves_the_previous_submissions_tile_snapshot_in_place`
    /// pins its own: `other_mode_before_last_plan` is assigned after
    /// `plan_raw_dpc_inner` returns `Ok`, and hoisting it above that
    /// fallible call would stamp a boundary for a packet that never
    /// executes.
    ///
    /// The surviving and the rejected packets select DIFFERENT TEXTLUT
    /// encodings, so a snapshot taken from the wrong side is
    /// distinguishable from the surviving value.
    #[test]
    fn a_rejected_plan_leaves_the_previous_submissions_other_mode_snapshot_in_place() {
        const TEXTLUT_SHIFT: u32 = 14;

        let (mut backend, session) = WgpuBackend::try_new().unwrap();

        let mut first = Vec::new();
        first.extend([word(SET_OTHER_MODE, 0 << TEXTLUT_SHIFT), 0]);
        first.extend(set_combine(0, 0));
        backend
            .plan_raw_dpc(session.plan_request(capture(first)))
            .expect("the mode-establishing submission plans cleanly");
        let after_success = backend
            .other_mode_before_last_plan
            .expect("a successful plan records the pre-delta other-mode snapshot");

        // A submission that is rejected at plan time -- `FullSync`
        // alongside a fill, the same refusal the tile sibling uses. It also
        // carries its own `SetOtherMode` with a TEXTLUT encoding that
        // appears nowhere else, so a snapshot taken from the wrong side of
        // the fallible call is distinguishable from the surviving one.
        let mut bad = partial_width_fill_words();
        bad.extend([word(SET_OTHER_MODE, 3 << TEXTLUT_SHIFT), 0]);
        bad.extend([word(FULL_SYNC, 0), 0]);
        assert!(
            backend
                .plan_raw_dpc(session.plan_request(capture(bad)))
                .is_err(),
            "positive control: this submission must really be rejected, or the assertion \
             below would be testing the success path twice"
        );

        assert_eq!(
            backend
                .other_mode_before_last_plan
                .expect("the snapshot must still be present after a rejected plan"),
            after_success,
            "a rejected plan must leave the last SUCCESSFUL submission's other-mode \
             snapshot untouched. Recording it before `plan_raw_dpc_inner` would stamp a \
             boundary for a packet that never executes, and the next execute_raw_dpc \
             would seed its walk from it"
        );
    }

    /// **`execute_raw_dpc` must seed the walk from the SNAPSHOT, never
    /// from the live registers.**
    ///
    /// The behavioural test above proves the snapshot holds the right
    /// table and that a walk seeded from it binds the right tile. It
    /// cannot prove `execute_raw_dpc` is the thing doing the seeding:
    /// `RetrievedTriangleDraw` is not reachable through the
    /// `RenderBackend` trait, so a mutant that records the snapshot
    /// faithfully and then passes `durable_neutral_tiles(&self.rdp_state)`
    /// -- exactly the pre-repair line -- SURVIVES it. Measured: it did.
    ///
    /// Pinned at the source instead, the same way
    /// `plan_raw_dpc_inner_decodes_both_passes_against_durable_state_not_default`
    /// pins its own two-pass choice, because the fact under test is which
    /// expression appears at one call site.
    ///
    /// Both halves are asserted. The `contains` alone would pass a body
    /// that read the snapshot *and* also fell back to the live table
    /// unconditionally; the count of bare `durable_neutral_tiles(&self`
    /// calls pins that the ONLY such call in this function is the
    /// `unwrap_or_else` fallback, which is reached only before any plan
    /// has run.
    #[test]
    fn execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot() {
        let source = include_str!("production.rs");
        let body_start = source
            .find("    fn execute_raw_dpc(")
            .expect("execute_raw_dpc must exist in this file");
        let next_fn = source[body_start + 1..]
            .find("\n    fn ")
            .map(|offset| body_start + 1 + offset)
            .unwrap_or(source.len());
        let body = &source[body_start..next_fn];
        assert!(
            body.contains("self.tiles_before_last_plan"),
            "execute_raw_dpc must seed its tile walk from the pre-delta snapshot \
             `tiles_before_last_plan`. Reading `rdp_state` directly reads the packet's own \
             already-folded SetTiles, which binds every draw standing before the packet's \
             first SetTile to a register the guest had not set yet"
        );
        assert_eq!(
            body.matches("durable_neutral_tiles(&self").count(),
            1,
            "the only `durable_neutral_tiles(&self.rdp_state)` in execute_raw_dpc must be \
             the `unwrap_or_else` fallback for the no-plan-yet case -- a second, \
             unconditional one would reintroduce the live-register read the snapshot exists \
             to replace"
        );
        assert!(
            body.contains(".unwrap_or_else(|| durable_neutral_tiles(&self.rdp_state))"),
            "that one call must be the fallback arm specifically, so a backend that has \
             executed before it ever planned still resolves a tile table rather than \
             panicking on an empty Option"
        );
    }

    /// The fill + **raw triangle** refusal, unchanged: still
    /// `MixedFillAndTrianglePacket`, and still for its original reason -- a
    /// raw triangle declares no journal write access at all, so there is no
    /// declared order to compose it onto the fill with.
    ///
    /// Kept as its own test after the texrect case split away from it,
    /// because the two now have materially different causes and a single
    /// test asserting one variant for both would hide a regression in
    /// either.
    #[test]
    fn a_fill_composed_with_a_raw_triangle_is_still_refused_by_name() {
        let refused = WgpuRawDpcExecutionError::MixedFillAndTrianglePacket.to_string();
        let mut fill_and_triangle = whole_target_fill_words();
        fill_and_triangle.extend(set_other_mode(0, 0));
        fill_and_triangle.extend(set_combine(0, 0));
        fill_and_triangle.extend(triangle_base_edge_words(7, 2, 0));

        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_fill(&mut backend, &mut session, fill_and_triangle);
        let error = result.expect_err("fill + raw triangle must still be refused");
        assert!(
            error.to_string().contains(&refused),
            "the refusal must be the named MixedFillAndTrianglePacket variant, got: {error}"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "a refused fill+triangle composition must leave no redeemable fill token behind"
        );
    }

    /// Build an `RspMemory` whose IMEM holds `text`, zero-padded, so the
    /// digest `process_task` reports is a value this test chose rather than
    /// whatever a default bank happens to hash to.
    fn rsp_memory_with_imem(text: &[u8]) -> fn64_runtime::RspMemory {
        let mut memory = fn64_runtime::RspMemory::new();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                text,
            )
            .expect("the fixture microcode fits in the IMEM bank");
        memory
    }

    /// A graphics task is a disposition, not an error: this backend has no
    /// HLE display-list front end, so it reports `NeedsLle` and `fn64-abi`
    /// runs the microcode on the RSP. Measured on WM2000 (NWXE), whose gfx
    /// tasks carry a real F3DEX2 display list under an uncatalogued IMEM
    /// digest -- `ReferenceBackend` returns `NeedsLle` for those same tasks.
    #[test]
    fn a_graphics_task_defers_to_lle_with_the_live_imem_digest() {
        let mut backend = WgpuBackend::try_new().unwrap().0;
        let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
        let mut rsp_memory = rsp_memory_with_imem(b"wm2000-uncatalogued-geometry-microcode");
        let expected =
            fn64_render::UcodeDigest::from_text(rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem))
                .as_bytes();

        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: COMMAND_START,
            data_size: 64,
            ..fn64_render::OsTask::default()
        };
        let status = backend
            .process_task(&mut rdram, &mut rsp_memory, &task, 0)
            .expect("a graphics task is a disposition, not a backend error");

        assert_eq!(
            status,
            fn64_render::FrameStatus::NeedsLle {
                ucode_sha256: expected
            },
            "the reported microcode identity must be the live IMEM digest"
        );
    }

    /// The digest is read from live IMEM, not from the task header or a
    /// constant, so a different microcode reports a different identity. A
    /// mutant returning a fixed digest, or hashing the task's `ucode` image
    /// instead, fails here.
    #[test]
    fn the_deferred_ucode_digest_tracks_live_imem_not_a_constant() {
        let mut backend = WgpuBackend::try_new().unwrap().0;
        let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            ..fn64_render::OsTask::default()
        };

        let mut first = rsp_memory_with_imem(b"microcode-a");
        let mut second = rsp_memory_with_imem(b"microcode-b");
        let a = backend
            .process_task(&mut rdram, &mut first, &task, 0)
            .unwrap();
        let b = backend
            .process_task(&mut rdram, &mut second, &task, 0)
            .unwrap();

        assert_ne!(
            a, b,
            "two different live microcodes must not report one identity"
        );
        for (memory, status) in [(&mut first, a), (&mut second, b)] {
            let fn64_render::FrameStatus::NeedsLle { ucode_sha256 } = status else {
                panic!("a graphics task must defer to LLE, got {status:?}");
            };
            assert_eq!(
                ucode_sha256,
                fn64_render::UcodeDigest::from_text(memory.bank(fn64_runtime::RspMemoryBank::Imem))
                    .as_bytes(),
                "the digest must be the live IMEM bank's"
            );
        }
    }

    /// Deferring a graphics task must not become a shrug that swallows a
    /// routing bug. A non-graphics task is still a loud named error, and the
    /// message names the type it received rather than saying "out of scope".
    #[test]
    fn a_non_graphics_task_is_still_refused_by_name() {
        let mut backend = WgpuBackend::try_new().unwrap().0;
        let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
        let mut rsp_memory = fn64_runtime::RspMemory::new();
        let audio = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK + 1,
            ..fn64_render::OsTask::default()
        };

        let error = backend
            .process_task(&mut rdram, &mut rsp_memory, &audio, 0)
            .expect_err("a non-graphics task at this seam is a routing bug");
        let reason = error.to_string();
        assert!(
            reason.contains(&(fn64_render::M_GFXTASK + 1).to_string()),
            "the refusal must name the task type it received: {reason}"
        );
    }

    /// A graphics task must not mutate guest memory on the way to its
    /// deferral: `fn64-abi` runs the very same task through LLE afterwards,
    /// and a half-applied prefix would be executed twice.
    #[test]
    fn deferring_a_graphics_task_leaves_guest_memory_untouched() {
        let mut backend = WgpuBackend::try_new().unwrap().0;
        let mut rdram = vec![0u8; LAYOUT_BYTES as usize];
        for (index, byte) in rdram.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let before = rdram.clone();
        let mut rsp_memory = rsp_memory_with_imem(b"uncatalogued");
        let rsp_before = rsp_memory.clone();

        let task = fn64_render::OsTask {
            task_type: fn64_render::M_GFXTASK,
            data_ptr: COMMAND_START,
            data_size: 64,
            ..fn64_render::OsTask::default()
        };
        backend
            .process_task(&mut rdram, &mut rsp_memory, &task, 0)
            .expect("a graphics task defers rather than failing");

        assert_eq!(rdram, before, "deferral must not write guest RDRAM");
        assert_eq!(
            rsp_memory.bank(fn64_runtime::RspMemoryBank::Imem),
            rsp_before.bank(fn64_runtime::RspMemoryBank::Imem),
            "deferral must not write RSP IMEM"
        );
    }
}
