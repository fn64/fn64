//! Narrow adapters between fn64's existing raw-DPC capture and render IR.
//!
//! This module translates one already validated, owned capture. It neither
//! decides when a DPC transaction commits nor applies bytes to guest memory;
//! those remain ABI/runtime-owner responsibilities.

use fn64_render_ir::{
    AccessMode, CompletedWrite, ContentDigest, DecodedTicket, DeferredGuestReadCapture,
    DeferredGuestReadPlan, DmemRange, DramCommandChunk, DramCommandStream, EffectIdentity,
    FullSyncBoundary, GpuCompleteTicket, GuestCommittedTicket, QueueIdentity, RawCommandStream,
    ResourceAccess, ResourceJournal, ResourceRegion, SubmissionIdentity, TemporalBoundary,
    ValidationError, WorkloadAdmission, WorkloadPacketPreflight, WorkloadRecord, XbusCommandChunk,
    XbusCommandStream,
};

use crate::{OwnedRawDpcSubmission, RawDpcSource};

pub use production::{
    new_raw_dpc_roles, BackendPreparedRawDpc, BoundSubmittedRawDpc, CommittedRawDpcOutcome,
    ExactRawDpcPlanVisitor, ExactRawDpcPlanWriter, ExactValidatedRawDpcPlan, GuestCommittedRawDpc,
    NeutralColor4, NeutralColorImage, NeutralCombineParams, NeutralFillColor, NeutralImageFormat,
    NeutralOtherMode, NeutralPixelSize, NeutralPrimColor, NeutralPrimDepth, NeutralScissor,
    NeutralTextureImage, NeutralTileAddressMode, NeutralTileDescriptor, NeutralTileSize,
    NeutralTmemTransferPhysicalWord, NeutralTmemTransferWord, NeutralTriangleVertex,
    PlannedRawDpcSubmission, RawDpcAbiSession, RawDpcBackendAuthority, RawDpcCommandLocation,
    RawDpcCoordinator, RawDpcExecutionView, RawDpcIrCapability, RawDpcPlanRequest,
    RawDpcRetirementHandle, RawDpcRetirementStage, RawDpcSemanticCommandRef, RawDpcTerminalOutcome,
    RdpFillRectangleCommand, RdpFullSyncSite, RdpStateCommand, RdpStateIdentity,
    RdpTriangleCommand, ReadyPublication, ReadyRawDpcCommitCapsule, RectViewportPixels,
    TmemLoadEpoch, TmemLoadKind, TmemLoadSemantics, TmemLoadShape, TmemTransferLayout,
    TriangleAccessSpan, TriangleSource,
};

/// Convert one exact owned raw-DPC capture into the move-only IR decode state.
///
/// Capture validation has already proved the source range and payload length.
/// Packet construction adds the installed-memory proof, exact command decode,
/// temporal observation, resource journal, and content identity. The result is
/// ephemeral input: callers may derive a content-silent [`WorkloadRecord`] for
/// replay, but durable semantic publication uses
/// [`CommittedSemanticWorkloadRecord`] and architectural observations remain
/// forbidden until the surrounding DPC transaction commits.
pub fn decode_raw_dpc_capture(
    memory_layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    capture: OwnedRawDpcSubmission,
    cmd_end: TemporalBoundary,
    full_sync_boundaries: Vec<FullSyncBoundary>,
    journal: ResourceJournal,
) -> Result<DecodedTicket, ValidationError> {
    preflight_raw_dpc_capture(
        memory_layout,
        transaction_sequence,
        capture,
        cmd_end,
        full_sync_boundaries,
        journal,
    )?
    .finalize(DeferredGuestReadCapture::empty())
}

/// Packet state before the ABI/memory owner captures renderer-selected guest
/// reads. It owns commands and semantic metadata but is not an admitted
/// packet, ticket, or replay record.
#[derive(Debug)]
pub struct IrRawDpcPacketPreflight {
    packet: WorkloadPacketPreflight,
}

impl IrRawDpcPacketPreflight {
    pub const fn guest_read_plan(&self) -> &DeferredGuestReadPlan {
        self.packet.guest_read_plan()
    }

    /// Consume preflight plus one owned ABI capture. This is the only step
    /// that can produce the retained packet/ticket.
    pub fn finalize(
        self,
        capture: DeferredGuestReadCapture,
    ) -> Result<DecodedTicket, ValidationError> {
        Ok(DecodedTicket::new(self.packet.finalize(capture)?))
    }
}

/// Decode and own command bytes, then derive the exact renderer-neutral guest
/// read plan without retaining or reading guest memory.
pub fn preflight_raw_dpc_capture(
    memory_layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    capture: OwnedRawDpcSubmission,
    cmd_end: TemporalBoundary,
    full_sync_boundaries: Vec<FullSyncBoundary>,
    journal: ResourceJournal,
) -> Result<IrRawDpcPacketPreflight, ValidationError> {
    let start = capture.start();
    let end = capture.end();
    let stream = match capture.source() {
        RawDpcSource::Rdram => RawCommandStream::Dram(DramCommandStream::try_new(vec![
            DramCommandChunk::try_new(
                memory_layout.range(start, end)?,
                capture.command_words(),
                cmd_end,
                full_sync_boundaries,
            )?,
        ])?),
        RawDpcSource::XbusDmem => RawCommandStream::Xbus(XbusCommandStream::try_new(vec![
            XbusCommandChunk::try_new(
                DmemRange::try_new(start, end)?,
                capture
                    .xbus_payload()
                    .expect("validated XBUS capture owns its sole byte image")
                    .to_vec(),
                cmd_end,
                full_sync_boundaries,
            )?,
        ])?),
    };
    let packet = WorkloadPacketPreflight::try_new(
        memory_layout,
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        },
        vec![stream],
        journal,
    )?;
    Ok(IrRawDpcPacketPreflight { packet })
}

/// Shared content identity for bytes produced by a renderer and rechecked by
/// the guest-memory owner before copyback.
pub fn ir_effect_content_digest(bytes: &[u8]) -> ContentDigest {
    fn64_render_ir::effect_content_digest(bytes)
}

/// Exact ABI-owned live-memory transaction preimage bound to one submitted workload.
///
/// This value is an identity, not guest-memory authority. The ABI owner keeps
/// the exclusive live allocation borrow and compares this complete binding
/// again before it issues a guest receipt or copies a byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrGuestMemoryPreimage {
    queue: QueueIdentity,
    transaction_ordinal: u64,
    submission: SubmissionIdentity,
    submission_ordinal: u64,
    byte_len: u32,
    content: ContentDigest,
}

impl IrGuestMemoryPreimage {
    pub fn try_capture(
        queue: QueueIdentity,
        transaction_ordinal: u64,
        submission: SubmissionIdentity,
        submission_ordinal: u64,
        bytes: &[u8],
    ) -> Result<Self, ValidationError> {
        let byte_len =
            u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
                field: "IR guest-memory preimage byte length",
            })?;
        Ok(Self {
            queue,
            transaction_ordinal,
            submission,
            submission_ordinal,
            byte_len,
            content: ContentDigest::hash(b"fn64.render.ir-guest-preimage.v1\0", &[bytes]),
        })
    }

    pub const fn queue(self) -> QueueIdentity {
        self.queue
    }

    pub const fn transaction_ordinal(self) -> u64 {
        self.transaction_ordinal
    }

    pub const fn submission(self) -> SubmissionIdentity {
        self.submission
    }

    pub const fn submission_ordinal(self) -> u64 {
        self.submission_ordinal
    }

    pub const fn byte_len(self) -> u32 {
        self.byte_len
    }

    pub const fn content(self) -> ContentDigest {
        self.content
    }
}

/// Owned immutable guest snapshot supplied by the ABI live-memory owner.
///
/// Capturing this image does not release or replace the owner's exclusive
/// borrow of the matching live allocation.
#[derive(Debug)]
pub struct IrGuestMemorySnapshot {
    preimage: IrGuestMemoryPreimage,
    bytes: Box<[u8]>,
}

impl IrGuestMemorySnapshot {
    pub fn try_capture(
        queue: QueueIdentity,
        transaction_ordinal: u64,
        submission: SubmissionIdentity,
        submission_ordinal: u64,
        bytes: &[u8],
    ) -> Result<Self, ValidationError> {
        Ok(Self {
            preimage: IrGuestMemoryPreimage::try_capture(
                queue,
                transaction_ordinal,
                submission,
                submission_ordinal,
                bytes,
            )?,
            bytes: bytes.to_vec().into_boxed_slice(),
        })
    }

    pub const fn preimage(&self) -> IrGuestMemoryPreimage {
        self.preimage
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Durable semantic publication proven to occur after guest copyback.
///
/// [`WorkloadRecord`] remains content-silent replay data and can be created
/// from any packet. This wrapper is the publication type: its private fields
/// can be populated only from a [`GuestCommittedTicket`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedSemanticWorkloadRecord {
    replay: WorkloadRecord,
    queue: QueueIdentity,
    ordinal: u64,
    submission: SubmissionIdentity,
    backend_effects: EffectIdentity,
    guest_effects: EffectIdentity,
}

impl CommittedSemanticWorkloadRecord {
    pub fn from_committed(ticket: &GuestCommittedTicket) -> Self {
        Self {
            replay: WorkloadRecord::from_packet(ticket.packet()),
            queue: ticket.queue(),
            ordinal: ticket.ordinal(),
            submission: ticket.submission(),
            backend_effects: ticket.backend_effect_identity(),
            guest_effects: ticket.guest_effect_identity(),
        }
    }

    pub const fn replay_record(&self) -> &WorkloadRecord {
        &self.replay
    }

    pub const fn queue(&self) -> QueueIdentity {
        self.queue
    }

    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub const fn submission(&self) -> SubmissionIdentity {
        self.submission
    }

    pub const fn backend_effect_identity(&self) -> EffectIdentity {
        self.backend_effects
    }

    pub const fn guest_effect_identity(&self) -> EffectIdentity {
        self.guest_effects
    }
}

/// Exact RDRAM bytes staged by a renderer after backend completion.
///
/// This is data, not commit authority. The guest-memory owner must recompute
/// [`Self::completed_write`] and match it to the backend receipt before using
/// [`Self::bytes`] for copyback.
#[derive(Debug, PartialEq, Eq)]
pub struct StagedIrRdramWrite {
    access: ResourceAccess,
    bytes: Box<[u8]>,
}

impl StagedIrRdramWrite {
    pub fn try_new(access: ResourceAccess, bytes: Vec<u8>) -> Result<Self, ValidationError> {
        if !matches!(access.region(), ResourceRegion::Rdram { .. }) {
            return Err(ValidationError::EffectAccessMismatch {
                field: "staged RDRAM write",
                index: 0,
            });
        }
        if !matches!(access.mode(), AccessMode::Write | AccessMode::ReadWrite) {
            return Err(ValidationError::EffectForReadOnlyAccess);
        }
        let actual = u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
            field: "staged RDRAM write byte length",
        })?;
        let expected = access.region().declared_bytes();
        if actual != expected {
            return Err(ValidationError::EffectByteCountMismatch { expected, actual });
        }
        Ok(Self {
            access,
            bytes: bytes.into_boxed_slice(),
        })
    }

    pub const fn access(&self) -> ResourceAccess {
        self.access
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn completed_write(&self) -> CompletedWrite {
        CompletedWrite::try_from_bytes(self.access, &self.bytes)
            .expect("staged RDRAM write construction proved its access and exact byte count")
    }
}

/// Receipt-validated backend effects plus exact guest bytes still awaiting
/// the separately owned guest-commit authority.
///
/// Completions are move-only, so one completion cannot be committed twice.
///
/// ```compile_fail
/// use fn64_render::IrRawDpcBackendCompletion;
/// # fn completion() -> IrRawDpcBackendCompletion { unimplemented!() }
/// # fn commit(_: IrRawDpcBackendCompletion) {}
/// let completion = completion();
/// commit(completion);
/// commit(completion);
/// ```
#[derive(Debug)]
pub struct IrRawDpcBackendCompletion {
    ticket: GpuCompleteTicket,
    guest_preimage: IrGuestMemoryPreimage,
    staged_guest_writes: Box<[StagedIrRdramWrite]>,
}

impl IrRawDpcBackendCompletion {
    pub fn try_new(
        ticket: GpuCompleteTicket,
        guest_preimage: IrGuestMemoryPreimage,
        staged_guest_writes: Vec<StagedIrRdramWrite>,
    ) -> Result<Self, ValidationError> {
        let expected = ticket
            .backend_writes()
            .iter()
            .copied()
            .filter(|effect| effect.access().region().is_guest_visible())
            .collect::<Vec<_>>();
        if expected.len() != staged_guest_writes.len() {
            return Err(ValidationError::EffectCountMismatch {
                field: "staged guest write",
                expected: expected.len(),
                actual: staged_guest_writes.len(),
            });
        }
        for (index, (expected, staged)) in expected.iter().zip(&staged_guest_writes).enumerate() {
            if *expected != staged.completed_write() {
                return Err(ValidationError::EffectAccessMismatch {
                    field: "staged guest write",
                    index,
                });
            }
        }
        Ok(Self {
            ticket,
            guest_preimage,
            staged_guest_writes: staged_guest_writes.into_boxed_slice(),
        })
    }

    pub const fn ticket(&self) -> &GpuCompleteTicket {
        &self.ticket
    }

    pub fn staged_guest_writes(&self) -> &[StagedIrRdramWrite] {
        &self.staged_guest_writes
    }

    pub const fn guest_preimage(&self) -> IrGuestMemoryPreimage {
        self.guest_preimage
    }

    pub fn into_parts(
        self,
    ) -> (
        GpuCompleteTicket,
        IrGuestMemoryPreimage,
        Box<[StagedIrRdramWrite]>,
    ) {
        (self.ticket, self.guest_preimage, self.staged_guest_writes)
    }
}

/// T0 neutral production raw-DPC seam: opaque capture/plan/capability/outcome
/// types, the planned -> finalized -> bound typestates, and the exact-once
/// retirement vocabulary described in
/// `docs/RENDER-WGPU-PORT-PLAN.md`'s production-dispatch slice.
///
/// Every sealed type here is `pub` only because it must be nameable at the
/// `fn64-render` <-> `fn64-render-wgpu` trait boundary. Fields, constructors,
/// and destructuring stay private to this module: `production::` is the sole
/// authority that can build or take apart a sealed value. A backend crosses
/// that boundary only through the named consuming, authority-gated
/// transitions -- there is no `Any`, `TypeId`, downcast, or `FnOnce` escape
/// hatch anywhere in this module.
mod production {
    use std::cell::Cell;
    use std::rc::Rc;

    use fn64_render_ir::{
        AccessMode, AccessPurpose, BackendCompletionAuthority, CompletedWrite,
        DeferredGuestReadCapture, DeferredGuestReadPlan, GpuCompleteTicket, GuestCommitAuthority,
        GuestCommitEffectReport, GuestCommitReceipt, GuestCommittedTicket, JournalIdentity,
        QueueIdentity, ResourceAccess, SubmissionIdentity, SubmissionQueue, TicketAuthoritySet,
        TmemRange, ValidationError,
    };

    use crate::RawDpcSubmissionIdentity;

    use super::IrRawDpcPacketPreflight;

    /// What this render-ir integration can honestly claim about raw-DPC
    /// execution. `Unsupported` is the only value a default trait method may
    /// report; a real transactional backend overrides it.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum RawDpcIrCapability {
        #[default]
        Unsupported,
        /// Synchronous, no-FullSync, TMEM-only transactional execution -- the
        /// production-dispatch slice's frozen scope (card v10 section 1).
        /// Retained for backends that admit no guest-visible write at all;
        /// superseded for `fn64-render-wgpu`'s `WgpuBackend` by
        /// [`Self::TransactionalTmemFillNoFullSync`].
        TransactionalTmemNoFullSync,
        /// Synchronous, no-FullSync transactional execution admitting TMEM
        /// loads **and** fill-cycle `FillRectangle` color-target writes.
        /// Distinct from [`Self::TransactionalTmemNoFullSync`] because a
        /// caller that reasons "this backend declares zero guest-visible
        /// writes" from the older variant would be wrong about this one --
        /// the distinction is the whole point, not cosmetic.
        ///
        /// Nonclaim: a backend reporting this admits a guest-visible
        /// *journal* write for an admitted fill. It does **not** claim that
        /// any guest RDRAM byte is modified -- the fill executor writes a
        /// backend-owned buffer, and no RDRAM copyback exists in this slice.
        TransactionalTmemFillNoFullSync,
        /// Everything [`Self::TransactionalTmemFillNoFullSync`] admits, plus a
        /// decoded `SYNC_FULL` **site**: the backend walks the opcode, binds
        /// it to the capture's own [`fn64_render_ir::FullSyncBoundary`], and
        /// reserves the sole DP completion slot before it touches anything.
        ///
        /// Added rather than folded into the fill variant for the same reason
        /// that one was added rather than folded into
        /// [`Self::TransactionalTmemNoFullSync`]: a caller that reasons "this
        /// backend rejects every FullSync" from the older variant would be
        /// wrong about this one, and reserving the DP slot is a real
        /// device-fabric interaction the older variants never perform.
        ///
        /// # Nonclaim -- a reservation is not an observation
        ///
        /// This variant claims a *site*, not a *boundary observation*. A
        /// backend reporting it asserts only that:
        ///
        /// - it decoded the `SYNC_FULL` opcode and bound it to a capture-time
        ///   boundary record, and
        /// - `DeviceFabric::preflight_dp_full_sync` proved the sole DP
        ///   completion slot was free -- a nonmutating reserve that raises no
        ///   interrupt and schedules no event.
        ///
        /// It does **not** claim that a DP interrupt was raised, that the
        /// guest observed one, or that any read-side coherence exists. Those
        /// remain `docs/RENDER-WGPU-PORT-PLAN.md`'s D7/M9 work. Concretely:
        /// the DP interrupt for a raw FullSync is raised inside
        /// `DeviceFabric::advance_to`'s `DeviceEvent::Dp` arm, which runs
        /// strictly *after* the whole capture/plan/execute/commit/publish
        /// sequence -- so at the moment a capture's boundary must already
        /// exist, `interrupt_after == Asserted` is not observable by
        /// construction, and a backend reporting this variant supplies
        /// [`fn64_render_ir::DpInterruptState::Clear`] for it.
        ///
        /// A future backend that genuinely observes the boundary is
        /// distinguished not by a new capability variant but by the
        /// `FullSyncBoundary` it supplies carrying
        /// `interrupt_after == Asserted`. That is deliberately the *only*
        /// place the observation claim lives, so it cannot be inferred from a
        /// capability enum that a reserving backend also reports.
        TransactionalTmemFillFullSyncSiteOnly,
    }

    /// The stage an in-flight submission's retirement was last known to
    /// occupy. Every value here is a possible `Rejected { stage, .. }` site;
    /// `PhysicalPrepare` is the last stage before an ordinal can publish.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum RawDpcRetirementStage {
        Execute,
        BackendReceipt,
        GuestReceipt,
        FabricPrepare,
        PhysicalPrepare,
    }

    /// Terminal record for one issued submission ordinal. Fixed-size and
    /// `Copy` so the shared slot never allocates on the drop/unwind path.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RawDpcTerminalOutcome {
        Published,
        Rejected {
            stage: RawDpcRetirementStage,
            submission: SubmissionIdentity,
        },
    }

    /// Write-once shared terminal slot. `Cell` is sufficient because every
    /// writer is single-threaded (the executor model `AGENTS.md`/`DESIGN.md`
    /// hold throughout: one runnable guest thread) and every write is a
    /// simple "if empty, set" -- no read-modify-write race is possible.
    #[derive(Debug, Default)]
    struct RetirementSlot {
        outcome: Cell<Option<RawDpcTerminalOutcome>>,
    }

    impl RetirementSlot {
        fn new() -> Rc<Self> {
            Rc::new(Self {
                outcome: Cell::new(None),
            })
        }

        /// Record the terminal outcome if (and only if) the slot is still
        /// empty. A second call after a value is already recorded is a no-op:
        /// exactly one terminal record ever survives per ordinal.
        fn record_if_empty(&self, outcome: RawDpcTerminalOutcome) {
            if self.outcome.get().is_none() {
                self.outcome.set(Some(outcome));
            }
        }

        fn get(&self) -> Option<RawDpcTerminalOutcome> {
            self.outcome.get()
        }
    }

    /// ABI-ledger-visible handle for diagnostics. Reading it never mutates
    /// and never blocks on the armed owner.
    #[derive(Clone, Debug)]
    pub struct RawDpcRetirementHandle {
        slot: Rc<RetirementSlot>,
        submission: SubmissionIdentity,
    }

    impl RawDpcRetirementHandle {
        pub const fn submission(&self) -> SubmissionIdentity {
            self.submission
        }

        pub fn outcome(&self) -> Option<RawDpcTerminalOutcome> {
            self.slot.get()
        }
    }

    /// Session-owned ledger of every issued ordinal's diagnostic handle.
    /// Purely additive bookkeeping; it never gates a transition and exposes
    /// no plan/ticket field.
    ///
    /// **Scope (P1).** "Exactly one armed retirement owner" is a guarantee
    /// about submissions that entered through `RawDpcAbiSession`, not a
    /// universal invariant over every `SubmittedTicket` in the process.
    /// `fn64-render-ir`'s own public, admission-agnostic
    /// `DecodedTicket::new` and `TicketAuthoritySet::submit`/
    /// `SubmissionQueue::submit` remain callable outside this session
    /// entirely (v11's own "legacy general render-IR queue APIs may remain
    /// public" clause), and a raw-DPC-admitted `SubmittedTicket` minted that
    /// way, then decoded through the legacy `fn64-render-wgpu::decode_raw_dpc`,
    /// is intentionally outside this ledger and never arms a
    /// `SubmittedRawDpcRetirement` at all -- exactly as it was under v10.
    /// This ledger, and the exact-once-terminal-record guarantee it
    /// diagnostically mirrors, only ever describes ordinals this session's
    /// own `finalize_and_submit` issued.
    #[derive(Debug, Default)]
    struct RetirementLedger {
        handles: Vec<RawDpcRetirementHandle>,
    }

    impl RetirementLedger {
        fn record(&mut self, handle: RawDpcRetirementHandle) {
            self.handles.push(handle);
        }
    }

    /// Armed owner of one issued ordinal's terminal record. Every post-submit
    /// typestate carries this by value. Its `Drop` performs only "if empty,
    /// set `Rejected`" against the pre-created shared slot: no allocation, no
    /// `RefCell` borrow, and no panic is possible during unwind.
    ///
    /// The terminal is internally `Option`/disarmed by
    /// [`Self::disarm_published`], so a successful publication's prior
    /// destructor run cannot also append a rejection: disarming happens
    /// before the value carrying this owner is dropped, and the flag itself
    /// is a plain `bool`, not a takeable field, so there is no double-take to
    /// guard against.
    ///
    /// **One concrete owner across every consuming wrapper.** This type has
    /// no `Clone` impl, so the only way a later typestate can obtain a
    /// `SubmittedRawDpcRetirement` is to receive the exact one an earlier
    /// typestate owned, moved out of its consumed struct literal --
    /// `BoundSubmittedRawDpc { retirement, .. }` -> `into_backend_prepared` moves it
    /// into `BackendPreparedRawDpc { retirement: self.retirement, .. }` ->
    /// `commit_zero_guest_writes` moves it into
    /// `GuestCommittedRawDpc { retirement, .. }` (via
    /// `let retirement = prepared.retirement;`, not a fresh
    /// `SubmittedRawDpcRetirement::new_pair`). The eventual capsule stage
    /// (T2/T3, once buildable) continues that same move chain. No code path
    /// in this module constructs a second `SubmittedRawDpcRetirement` for an
    /// already-issued ordinal, and none uses `mem::forget`/`ManuallyDrop` to
    /// suppress this type's own `Drop` -- the source-shape sweep test below
    /// also greps for that. Consequently the shared `Rc<RetirementSlot>`
    /// this type wraps is the *same* allocation from issuance through
    /// terminal record, and exactly one destructor (the last typestate still
    /// holding this value when it is dropped, or the disarming
    /// `publish`-equivalent) can ever write to it.
    #[derive(Debug)]
    struct SubmittedRawDpcRetirement {
        slot: Rc<RetirementSlot>,
        submission: SubmissionIdentity,
        stage: RawDpcRetirementStage,
        armed: bool,
    }

    impl SubmittedRawDpcRetirement {
        /// Arm a fresh retirement plus the ABI-ledger diagnostic handle that
        /// shares its terminal slot.
        fn new_pair(submission: SubmissionIdentity) -> (Self, RawDpcRetirementHandle) {
            let slot = RetirementSlot::new();
            let handle = RawDpcRetirementHandle {
                slot: Rc::clone(&slot),
                submission,
            };
            let retirement = Self {
                slot,
                submission,
                stage: RawDpcRetirementStage::Execute,
                armed: true,
            };
            (retirement, handle)
        }

        const fn stage(&self) -> RawDpcRetirementStage {
            self.stage
        }

        /// Advance the recorded stage. This never touches the shared slot; it
        /// only changes what a later `Drop`-time rejection would report.
        fn advance_stage(&mut self, stage: RawDpcRetirementStage) {
            self.stage = stage;
        }

        /// Disarm this owner as `Published`. Only
        /// [`ReadyRawDpcCommitCapsule::commit`]'s successful terminal
        /// publication calls this; every other destruction path (`Err`,
        /// early explicit drop, or unwind) leaves `armed` set and lets `Drop`
        /// record `Rejected` at the last-advanced stage.
        fn disarm_published(mut self) {
            self.slot.record_if_empty(RawDpcTerminalOutcome::Published);
            self.armed = false;
        }
    }

    impl Drop for SubmittedRawDpcRetirement {
        fn drop(&mut self) {
            if self.armed {
                self.slot.record_if_empty(RawDpcTerminalOutcome::Rejected {
                    stage: self.stage,
                    submission: self.submission,
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Neutral DTO vocabulary
    // ------------------------------------------------------------------

    /// Neutral mirror of the public `G_IM_FMT` texel-format field (SGI *RDP
    /// Command Summary* Table 6). Concrete and `Copy`; carries no
    /// `fn64-render-wgpu`-private type.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum NeutralImageFormat {
        Rgba,
        Yuv,
        ColorIndex,
        IntensityAlpha,
        Intensity,
    }

    /// Neutral mirror of the public `G_IM_SIZ` texel-size field.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum NeutralPixelSize {
        Bits4,
        Bits8,
        Bits16,
        Bits32,
    }

    /// Neutral mirror of one tile's public S/T address-mode bits (mirror,
    /// clamp) from `SetTile`.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct NeutralTileAddressMode {
        pub mirror: bool,
        pub clamp: bool,
    }

    /// Neutral, complete mirror of one `SetTile` command's staged fields:
    /// format/size, TMEM word address, line stride, palette, and both axes'
    /// address mode/mask/shift. T3 needs every field here to execute a load
    /// against the tile state a real decoder already staged; none of it can
    /// be recovered from `ResourceAccess` identity alone.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralTileDescriptor {
        pub format: NeutralImageFormat,
        pub size: NeutralPixelSize,
        pub line_words: u16,
        pub tmem_word_address: u16,
        pub palette: u8,
        pub s_mode: NeutralTileAddressMode,
        pub mask_s: u8,
        pub shift_s: u8,
        pub t_mode: NeutralTileAddressMode,
        pub mask_t: u8,
        pub shift_t: u8,
    }

    /// Neutral mirror of one `SetTileSize` command's S/T bounds, in the
    /// public 10.2 fixed-point raw field encoding.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralTileSize {
        pub low_s: u16,
        pub low_t: u16,
        pub high_s: u16,
        pub high_t: u16,
    }

    /// Neutral mirror of `SetTextureImage`'s staged source description.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralTextureImage {
        pub format: NeutralImageFormat,
        pub size: NeutralPixelSize,
        pub width: u16,
        pub address: fn64_render_ir::PhysicalAddress,
    }

    /// Opaque monotonic TMEM load-sync epoch. Mirrors
    /// `fn64-render-wgpu`'s private `TmemLoadEpoch`: staged by `SyncLoad`,
    /// bound to every load that follows it, so T3 can reject a load whose
    /// epoch predates the physical state's own generation without rereading
    /// command bytes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TmemLoadEpoch(core::num::NonZeroU64);

    impl TmemLoadEpoch {
        pub const fn new(epoch: core::num::NonZeroU64) -> Self {
            Self(epoch)
        }

        pub const fn get(self) -> u64 {
            self.0.get()
        }
    }

    /// Which public opcode produced one [`TmemLoadSemantics`] value, together
    /// with that opcode's exact addressing geometry. Distinct geometry per
    /// kind (LoadBlock's DXT accumulator vs. LoadTile/LoadTLUT's S/T bounds)
    /// cannot be recovered from a shared bare `TileSize`, so each variant
    /// carries its own real fields instead of a lossy common shape.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum TmemLoadKind {
        Block {
            source_s: u16,
            source_t: u16,
            high_s: u16,
            dxt: u16,
        },
        Tile {
            bounds: NeutralTileSize,
        },
        /// Reserved for M4.3.2; a plan admits this only once that
        /// prerequisite lands (card v10 section 1/8).
        Tlut {
            bounds: NeutralTileSize,
            entries: core::num::NonZeroU16,
        },
    }

    /// Neutral mirror of `fn64-render-wgpu`'s private `TmemTransferLayout`:
    /// which physical addressing rule this load's transfer words follow.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum TmemTransferLayout {
        Linear,
        OddRowBankSwap,
    }

    /// One physical TMEM destination for a transfer word: either a single
    /// linear range, or the split low/high-bank pair the public odd-row
    /// exchange rule produces. Concrete, not a downstream type.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum NeutralTmemTransferPhysicalWord {
        Linear(TmemRange),
        SplitBanks { low: TmemRange, high: TmemRange },
    }

    /// One complete, already-computed 64-bit TMEM transfer word: exactly the
    /// materialized fact set a physical executor (T3) needs per word, so it
    /// never rereads raw command bytes or recomputes tile geometry from a
    /// bare resource access. Mirrors `fn64-render-wgpu`'s private
    /// `TmemTransferWord` field-for-field.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralTmemTransferWord {
        pub index: u16,
        pub logical_source_offset: u32,
        pub source_access_index: u32,
        pub source_access_byte_offset: u32,
        pub defined_source_byte_mask: u8,
        pub defined_destination_byte_mask: u8,
        pub destination_word: u16,
        pub row_advance: u16,
        pub odd_row_exchange: bool,
        pub physical: NeutralTmemTransferPhysicalWord,
    }

    /// Exact location of the raw command word(s) one semantic command was
    /// decoded from: its ordinal position within the plan
    /// (`command_index`), which stream/chunk it came from, the exact
    /// physical/DMEM source address the command bytes were read from
    /// (`source_address` -- distinct from `source_byte_offset`, which is
    /// relative to the owning chunk, not the address space), the source's
    /// byte offset/length within that chunk, and the wire opcode byte. T3
    /// needs every field here to bind a physical effect back to the exact
    /// command that caused it, in its exact decode-order position, without
    /// rereading the owning `ExactValidatedRawDpcPlan`'s source bytes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct RawDpcCommandLocation {
        pub command_index: u32,
        pub stream_index: u32,
        pub chunk_index: u32,
        pub source_address: fn64_render_ir::PhysicalAddress,
        pub source_byte_offset: u32,
        pub source_byte_len: u32,
        pub wire_opcode: u8,
    }

    /// Complete neutral semantics for one TMEM load command (`LoadBlock`,
    /// `LoadTile`, or `LoadTLUT`): staged tile/source-image state, the exact
    /// opcode-specific addressing geometry, the load-sync epoch it was bound
    /// under, the command's own raw wire words, exact source-byte
    /// accounting (the complete ordered source-access run, which a
    /// partial-width `LoadTile` splits one-per-row), an explicit index into
    /// the owning plan's access list for the destination access, transfer
    /// layout, and the full ordered
    /// transfer-word set a real decoder (T1) already computed. This is the
    /// complete neutral semantic/load representation the execution seam
    /// needs -- every field T3's physical executor reads is here,
    /// materialized, so crossing into `fn64-render`'s neutral plan cannot
    /// force a redecode or a weakened generation/epoch/state check.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TmemLoadSemantics {
        location: RawDpcCommandLocation,
        raw_words: Box<[u32]>,
        epoch: TmemLoadEpoch,
        kind: TmemLoadKind,
        tile_index: u8,
        source_image: NeutralTextureImage,
        tile_descriptor: NeutralTileDescriptor,
        sources: Box<[ResourceAccess]>,
        source_access_index: u32,
        destination: ResourceAccess,
        destination_access_index: u32,
        logical_source_bytes: u32,
        undefined_padding_bytes: u32,
        words_per_row: u16,
        row_count: u16,
        layout: TmemTransferLayout,
        transfer_words: Box<[NeutralTmemTransferWord]>,
    }

    impl TmemLoadSemantics {
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            location: RawDpcCommandLocation,
            raw_words: Vec<u32>,
            epoch: TmemLoadEpoch,
            kind: TmemLoadKind,
            tile_index: u8,
            source_image: NeutralTextureImage,
            tile_descriptor: NeutralTileDescriptor,
            sources: Vec<ResourceAccess>,
            source_access_index: u32,
            destination: ResourceAccess,
            destination_access_index: u32,
            logical_source_bytes: u32,
            undefined_padding_bytes: u32,
            words_per_row: u16,
            row_count: u16,
            layout: TmemTransferLayout,
            transfer_words: Vec<NeutralTmemTransferWord>,
        ) -> Self {
            assert!(
                !sources.is_empty(),
                "a TMEM load always reads at least one source access"
            );
            Self {
                location,
                raw_words: raw_words.into_boxed_slice(),
                epoch,
                kind,
                tile_index,
                source_image,
                tile_descriptor,
                sources: sources.into_boxed_slice(),
                source_access_index,
                destination,
                destination_access_index,
                logical_source_bytes,
                undefined_padding_bytes,
                words_per_row,
                row_count,
                layout,
                transfer_words: transfer_words.into_boxed_slice(),
            }
        }

        pub const fn location(&self) -> RawDpcCommandLocation {
            self.location
        }

        pub fn raw_words(&self) -> &[u32] {
            &self.raw_words
        }

        pub const fn epoch(&self) -> TmemLoadEpoch {
            self.epoch
        }

        pub const fn kind(&self) -> TmemLoadKind {
            self.kind
        }

        pub const fn shape(&self) -> TmemLoadShape {
            match self.kind {
                TmemLoadKind::Block { .. } => TmemLoadShape::Block,
                TmemLoadKind::Tile { .. } => TmemLoadShape::Tile,
                TmemLoadKind::Tlut { .. } => TmemLoadShape::Tlut,
            }
        }

        pub const fn tile_index(&self) -> u8 {
            self.tile_index
        }

        pub const fn source_image(&self) -> NeutralTextureImage {
            self.source_image
        }

        pub const fn tile_descriptor(&self) -> NeutralTileDescriptor {
            self.tile_descriptor
        }

        /// Every [`ResourceAccess`] this load reads from, in the exact
        /// journal order the decoder produced them, occupying the owning
        /// plan's access list contiguously starting at
        /// [`Self::source_access_index`].
        ///
        /// This is a slice, not a single access, precisely because a
        /// partial-width `LoadTile` declares **one source read per row**
        /// (`tmem::wire::decode_load_tile`'s `(low_t..=high_t)` arm): a
        /// 49-row sub-rectangle of a wider texture is 49 disjoint RDRAM
        /// reads, not one contiguous span. Only a load whose source columns
        /// cover the full texture-image width collapses to a single range.
        /// There is no "collapse to one access" path and there must never
        /// be one -- a collapsed range would claim the untouched
        /// inter-row bytes as read, and `transfer_words[].source_access_index`
        /// already binds each transfer word to the exact row it came from.
        pub fn sources(&self) -> &[ResourceAccess] {
            &self.sources
        }

        /// The load's **first** source access -- the one at
        /// [`Self::source_access_index`]. Callers that must account for
        /// every byte the load reads want [`Self::sources`]; this names
        /// only the first fragment, exactly as [`Self::destination`] names
        /// only the first destination fragment.
        pub fn source(&self) -> ResourceAccess {
            self.sources[0]
        }

        /// Index of [`Self::sources`]`[0]` within the owning plan's exact
        /// ordered access list -- lets T3 correlate this load's source run
        /// without re-deriving which journal entries it came from. The run
        /// is contiguous, so fragment `i` sits at
        /// `source_access_index() + i`.
        pub const fn source_access_index(&self) -> u32 {
            self.source_access_index
        }

        pub const fn destination(&self) -> ResourceAccess {
            self.destination
        }

        /// Index of [`Self::destination`] within the owning plan's exact
        /// ordered access list -- the explicit destination access index T3
        /// needs to bind a physical write back to the plan without
        /// re-deriving it.
        pub const fn destination_access_index(&self) -> u32 {
            self.destination_access_index
        }

        pub const fn logical_source_bytes(&self) -> u32 {
            self.logical_source_bytes
        }

        pub const fn undefined_padding_bytes(&self) -> u32 {
            self.undefined_padding_bytes
        }

        pub const fn words_per_row(&self) -> u16 {
            self.words_per_row
        }

        pub const fn row_count(&self) -> u16 {
            self.row_count
        }

        pub const fn layout(&self) -> TmemTransferLayout {
            self.layout
        }

        pub fn transfer_words(&self) -> &[NeutralTmemTransferWord] {
            &self.transfer_words
        }
    }

    /// Which public opcode produced one [`TmemLoadSemantics`] value. Kept as
    /// a cheap discriminant alongside [`TmemLoadKind`] (which carries the
    /// exact geometry) so callers that only need the opcode class do not
    /// have to match the full geometry enum.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum TmemLoadShape {
        Block,
        Tile,
        Tlut,
    }

    /// Content identity for one neutral tile/texture-image/epoch/RDP-state
    /// value, so a state transition can name what it superseded and what it
    /// established without T3 having to reread or re-derive either snapshot
    /// from raw bytes. Distinct hash domains per state kind keep a
    /// `SetTile` identity from ever colliding with a `SetTextureImage`
    /// identity for coincidentally identical bit patterns.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct RdpStateIdentity(fn64_render_ir::ContentDigest);

    impl RdpStateIdentity {
        pub fn of_tile_descriptor(tile_index: u8, descriptor: NeutralTileDescriptor) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.tmem-state-tile-descriptor.v1\0",
                &[&[tile_index], &descriptor_bytes(descriptor)],
            ))
        }

        pub fn of_tile_size(tile_index: u8, size: NeutralTileSize) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.tmem-state-tile-size.v1\0",
                &[
                    &[tile_index],
                    &size.low_s.to_be_bytes(),
                    &size.low_t.to_be_bytes(),
                    &size.high_s.to_be_bytes(),
                    &size.high_t.to_be_bytes(),
                ],
            ))
        }

        pub fn of_texture_image(image: NeutralTextureImage) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.tmem-state-texture-image.v1\0",
                &[&texture_image_bytes(image)],
            ))
        }

        pub fn of_other_mode(value: NeutralOtherMode) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-other-mode.v1\0",
                &[&other_mode_bytes(value)],
            ))
        }

        pub fn of_color_image(value: NeutralColorImage) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-color-image.v1\0",
                &[&color_image_bytes(value)],
            ))
        }

        pub fn of_fill_color(value: NeutralFillColor) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-fill-color.v1\0",
                &[&fill_color_bytes(value)],
            ))
        }

        pub fn of_env_color(value: NeutralColor4) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-env-color.v1\0",
                &[&color4_bytes(value)],
            ))
        }

        pub fn of_prim_color(value: NeutralPrimColor) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-prim-color.v1\0",
                &[&prim_color_bytes(value)],
            ))
        }

        pub fn of_blend_color(value: NeutralColor4) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-blend-color.v1\0",
                &[&color4_bytes(value)],
            ))
        }

        pub fn of_fog_color(value: NeutralColor4) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-fog-color.v1\0",
                &[&color4_bytes(value)],
            ))
        }

        pub fn of_prim_depth(value: NeutralPrimDepth) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-prim-depth.v1\0",
                &[&prim_depth_bytes(value)],
            ))
        }

        pub fn of_combine(value: NeutralCombineParams) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-combine.v1\0",
                &[&combine_bytes(value)],
            ))
        }

        /// Identity of one `SetScissor`'s tracked rect. Its own domain tag
        /// (`rdp-state-scissor.v1`) keeps it disjoint from every other
        /// state slot's identity space, exactly as [`Self::of_fog_color`]
        /// and its siblings do.
        pub fn of_scissor(value: NeutralScissor) -> Self {
            Self(fn64_render_ir::ContentDigest::hash(
                b"fn64.render.rdp-state-scissor.v1\0",
                &[&scissor_bytes(value)],
            ))
        }

        pub const fn digest(self) -> fn64_render_ir::ContentDigest {
            self.0
        }
    }

    fn descriptor_bytes(descriptor: NeutralTileDescriptor) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16);
        bytes.push(descriptor.format as u8);
        bytes.push(descriptor.size as u8);
        bytes.extend_from_slice(&descriptor.line_words.to_be_bytes());
        bytes.extend_from_slice(&descriptor.tmem_word_address.to_be_bytes());
        bytes.push(descriptor.palette);
        bytes.push(descriptor.s_mode.mirror as u8);
        bytes.push(descriptor.s_mode.clamp as u8);
        bytes.push(descriptor.mask_s);
        bytes.push(descriptor.shift_s);
        bytes.push(descriptor.t_mode.mirror as u8);
        bytes.push(descriptor.t_mode.clamp as u8);
        bytes.push(descriptor.mask_t);
        bytes.push(descriptor.shift_t);
        bytes
    }

    fn texture_image_bytes(image: NeutralTextureImage) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.push(image.format as u8);
        bytes.push(image.size as u8);
        bytes.extend_from_slice(&image.width.to_be_bytes());
        bytes.extend_from_slice(&image.address.get().to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetOtherMode`'s staged pure-state value.
    ///
    /// Kept as the raw `high`/`low` wire pair, matching
    /// `crate::state::OtherMode`'s own internal representation, rather than
    /// decomposed into its ~20 derived fields: every one of those fields is
    /// already a cheap computed accessor on `OtherMode` (`cycle_type`,
    /// `texture_lut_mode`, `blender_cycle_1`, etc.), so decomposing here
    /// would duplicate that bit-math in a second place for no reader this
    /// admission-only card serves. A future consumer needing named fields can
    /// call `OtherMode`'s own accessors after reconstructing it from
    /// `high`/`low`. Open, non-blocking per this card's Section 2d.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralOtherMode {
        pub high: u32,
        pub low: u32,
    }

    fn other_mode_bytes(value: NeutralOtherMode) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.extend_from_slice(&value.high.to_be_bytes());
        bytes.extend_from_slice(&value.low.to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetColorImage`'s staged pure-state value.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralColorImage {
        pub format: NeutralImageFormat,
        pub size: NeutralPixelSize,
        pub width: u32,
        pub address: fn64_render_ir::PhysicalAddress,
    }

    fn color_image_bytes(value: NeutralColorImage) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(10);
        bytes.push(value.format as u8);
        bytes.push(value.size as u8);
        bytes.extend_from_slice(&value.width.to_be_bytes());
        bytes.extend_from_slice(&value.address.get().to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetFillColor`'s staged raw 32-bit wire value.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralFillColor {
        pub value: u32,
    }

    fn fill_color_bytes(value: NeutralFillColor) -> Vec<u8> {
        value.value.to_be_bytes().to_vec()
    }

    /// Neutral mirror of one fragment constant-register RGBA color, shared by
    /// `SetEnvColor`/`SetBlendColor`/`SetFogColor` -- all three decode via
    /// the identical `Color4::from_wire(w1)` (card Section 2d).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralColor4 {
        pub value: u32,
    }

    fn color4_bytes(value: NeutralColor4) -> Vec<u8> {
        value.value.to_be_bytes().to_vec()
    }

    /// Neutral mirror of `SetPrimColor`'s staged LOD-fraction/LOD-min/color
    /// fields.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralPrimColor {
        pub lod_frac: u8,
        pub lod_min: u8,
        pub color: u32,
    }

    fn prim_color_bytes(value: NeutralPrimColor) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(6);
        bytes.push(value.lod_frac);
        bytes.push(value.lod_min);
        bytes.extend_from_slice(&value.color.to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetPrimDepth`'s staged masked depth/delta-Z fields.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralPrimDepth {
        pub z: u16,
        pub dz: u16,
    }

    fn prim_depth_bytes(value: NeutralPrimDepth) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4);
        bytes.extend_from_slice(&value.z.to_be_bytes());
        bytes.extend_from_slice(&value.dz.to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetCombine`'s staged raw low/high wire words.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralCombineParams {
        pub low: u32,
        pub high: u32,
    }

    fn combine_bytes(value: NeutralCombineParams) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8);
        bytes.extend_from_slice(&value.low.to_be_bytes());
        bytes.extend_from_slice(&value.high.to_be_bytes());
        bytes
    }

    /// Neutral mirror of `SetScissor`'s decoded operands (RDP opcode `0x2d`),
    /// field-for-field as RT64's `setScissor` decode reads them: a 2-bit
    /// `mode` plus four 12-bit fixed-point coordinates (10 integer bits, 2
    /// fractional -- the same `<< 2` scale `FillRectangle`/`TexRect` use), all
    /// zero-extended and therefore never negative.
    ///
    /// **Tracked state only.** This carrier exists so a stream containing
    /// `SetScissor` is admitted rather than rejected; nothing in the raster
    /// path reads it. It is deliberately *not* mirrored into
    /// `RdpState`/`RdpStateDelta` the way the nine applied pure-state
    /// commands are, because that is precisely the channel a draw would use
    /// to consult it. Actually clipping to this rect is separate, later work.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct NeutralScissor {
        pub mode: u8,
        pub upper_left_x: u16,
        pub upper_left_y: u16,
        pub lower_right_x: u16,
        pub lower_right_y: u16,
    }

    fn scissor_bytes(value: NeutralScissor) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(9);
        bytes.push(value.mode);
        bytes.extend_from_slice(&value.upper_left_x.to_be_bytes());
        bytes.extend_from_slice(&value.upper_left_y.to_be_bytes());
        bytes.extend_from_slice(&value.lower_right_x.to_be_bytes());
        bytes.extend_from_slice(&value.lower_right_y.to_be_bytes());
        bytes
    }

    /// Neutral payload for one staged, resource-access-free state command
    /// (`SetTile`, `SetTileSize`, `SetTextureImage`, `SyncLoad`, plus the
    /// nine pure-RDP-state commands admitted alongside them: `SetOtherMode`,
    /// `SetColorImage`, `SetFillColor`, `SetEnvColor`, `SetPrimColor`,
    /// `SetBlendColor`, `SetFogColor`, `SetPrimDepth`, `SetCombine`, plus
    /// the tracked-only `SetScissor`) that a following load or draw command
    /// depends on. T3 needs these fields to reconstruct tile/RDP state
    /// without rereading command bytes.
    ///
    /// Every variant except `SyncLoad` carries `raw_words` (the command's own
    /// wire words) and an ordered `before`/`after` [`RdpStateIdentity`] pair:
    /// `before` is `None` only for the first state command touching that
    /// slot in a plan (there is no prior state to identify); `after` is
    /// always the identity of the value this command just staged. `SyncLoad`
    /// instead carries `input_epoch`/`output_epoch` -- the epoch this
    /// command superseded (`None` only for a plan's first `SyncLoad`) and
    /// the new epoch it established -- since a load-sync boundary has no
    /// tile/image value of its own to hash. The nine pure-state commands
    /// each occupy one single global slot in `RdpState`/`RdpStateDelta`
    /// (`Option<T>`, not a per-tile array), so `before` threads exactly the
    /// way `SetTextureImage`'s single-slot `texture_image` field already
    /// does, not the 8-slot tile arrays.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum RdpStateCommand {
        SetTile {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            tile_index: u8,
            descriptor: NeutralTileDescriptor,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetTileSize {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            tile_index: u8,
            size: NeutralTileSize,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetTextureImage {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            image: NeutralTextureImage,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SyncLoad {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            input_epoch: Option<TmemLoadEpoch>,
            output_epoch: TmemLoadEpoch,
        },
        SetOtherMode {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            other_mode: NeutralOtherMode,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetColorImage {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            image: NeutralColorImage,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetFillColor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            color: NeutralFillColor,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetEnvColor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            color: NeutralColor4,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetPrimColor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            color: NeutralPrimColor,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetBlendColor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            color: NeutralColor4,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetFogColor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            color: NeutralColor4,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetPrimDepth {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            depth: NeutralPrimDepth,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        SetCombine {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            combine: NeutralCombineParams,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
        /// RDP opcode `0x2d`, admitted as **tracked state only**: the rect
        /// is carried here so a stream containing `SetScissor` parses and
        /// plans instead of dying at `UnsupportedCommand`, but no draw,
        /// clip, or bounds computation reads it. Unlike the nine applied
        /// pure-state commands above, this one's value is deliberately
        /// absent from `RdpState`/`RdpStateDelta`, so there is no channel
        /// through which the raster path could consult it even by accident.
        /// It still threads `before`/`after` over its own single global
        /// slot exactly like its siblings, so admitting it later (as
        /// applied state) is an additive change rather than a reshape.
        SetScissor {
            location: RawDpcCommandLocation,
            raw_words: Box<[u32]>,
            scissor: NeutralScissor,
            before: Option<RdpStateIdentity>,
            after: RdpStateIdentity,
        },
    }

    /// Neutral mirror of one decoded triangle vertex (RT64's
    /// `posWorkBuffer`/`colorWorkBuffer`/`texcoordWorkBuffer` write for one
    /// triangle vertex), field-for-field identical to T1's private
    /// `TriangleVertex` shape -- this crate cannot name that wgpu-crate type
    /// directly (`fn64-render-wgpu` depends on `fn64-render`, not the
    /// reverse), so this is a plain data mirror, not a reinterpretation.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct NeutralTriangleVertex {
        pub x: f32,
        pub y: f32,
        pub z: f32,
        pub w: f32,
        pub color: [f32; 4],
        pub texcoord: [f32; 2],
    }

    /// Neutral carrier for one admitted `RawTriangle` draw command: the
    /// command's own raw wire words (variable-width, per
    /// `raw_rdp_command_width`/`triangle_word_count` -- unlike every
    /// `RdpStateCommand` variant's fixed 2-word shape) and the three already-
    /// decoded triangle vertices, in RT64's exact `workBufferIndex + 0/1/2`
    /// order. Deliberately has **no** `before`/`after` [`RdpStateIdentity`]
    /// fields: a triangle is a draw event, not a value that persists in one
    /// global slot and gets overwritten the way the nine pure-state
    /// commands do, and it pushes zero [`ResourceAccess`] entries into the
    /// owning plan, so [`ExactRawDpcPlanWriter::finish`]'s access-ordering
    /// contract (which has zero coupling to the writer's pushed commands) is
    /// trivially satisfied without one. `raw_words` is kept anyway, matching
    /// this crate's characterization-first convention of never discarding
    /// the raw bytes a command carried.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RdpTriangleCommand {
        pub location: RawDpcCommandLocation,
        pub raw_words: Box<[u32]>,
        pub vertices: [NeutralTriangleVertex; 3],
        pub source: TriangleSource,
        pub viewport: Option<RectViewportPixels>,
        /// The exact ordered `RenderTarget` write-access span the decoder
        /// declared for the originating `TextureRectangle` command, or
        /// `None`.
        ///
        /// `None` for every `TriangleSource::RawTriangle` -- a raw triangle
        /// pushes zero accesses, as this type's own doc states -- and also
        /// `None` for a `TextureRectangle` whose destination was not
        /// provable at decode time (no staged `SetColorImage`, an
        /// unsupported color format, or a fractional or reversed rectangle;
        /// see the wgpu decoder's `plan_texture_rectangle`). A texrect
        /// that declared no write is not a silent no-op: it still rasters
        /// through the triangle path, it simply has no `ColorFramebuffer`
        /// range for a CPU-side executor to compose into.
        ///
        /// Carried for the same reason [`RdpFillRectangleCommand`] carries
        /// its own pair: so a visitor can locate the accesses this command
        /// declared **without re-deriving them** from the rectangle's
        /// geometry, which is exactly the second-independent-derivation
        /// drift `ExactRawDpcPlanWriter::finish`'s access-for-access check
        /// exists to catch.
        ///
        /// One texture rectangle is admitted as two triangles, and **both
        /// halves carry the identical span** -- it describes the
        /// originating wire command, not either half's own share of it.
        /// A consumer counting declared writes must therefore attribute the
        /// span once per originating command, never once per triangle.
        pub texrect_accesses: Option<TriangleAccessSpan>,
    }

    /// One `TextureRectangle`-sourced triangle's originating command's
    /// declared `RenderTarget` write-access span, in the owning plan's
    /// ordered access list.
    ///
    /// Field-for-field the same pair [`RdpFillRectangleCommand`] carries
    /// (`first_access_index`/`access_count`), named as a struct here because
    /// the whole pair is optional on a triangle where it is mandatory on a
    /// fill -- `Option<TriangleAccessSpan>` makes "declared nothing"
    /// unrepresentable as "declared zero accesses starting at zero".
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TriangleAccessSpan {
        /// Index into the owning plan's ordered access list of the
        /// originating command's first `RenderTarget` write access.
        pub first_access_index: u32,
        /// How many consecutive accesses starting at `first_access_index`
        /// belong to it. `1` for a full-image-width rectangle, the
        /// rectangle's covered pixel-row count otherwise.
        pub access_count: u32,
    }

    /// Neutral carrier for one admitted fill-cycle `FillRectangle` (RDP
    /// opcode 0x36). Carries the decoded wire rectangle plus the exact
    /// ordered [`ResourceAccess`] span the decoder declared for it -- one
    /// access for a full-image-width fill, one **per row** otherwise,
    /// because a partial-width rectangle's rows occupy disjoint,
    /// width-strided RDRAM ranges and one collapsed range would declare
    /// untouched bytes as written.
    ///
    /// Unlike [`RdpTriangleCommand`] (which pushes zero accesses) this
    /// command pushes N, so it carries `first_access_index`/`access_count`
    /// exactly the way [`TmemLoadSemantics`] carries its own
    /// destination-access span -- letting a visitor locate this fill's
    /// accesses in the owning plan without re-deriving them.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RdpFillRectangleCommand {
        pub location: RawDpcCommandLocation,
        pub raw_words: Box<[u32]>,
        /// Raw 12-bit fixed-point wire fields, exactly as the decoder read
        /// them -- 10 integer bits plus 2 fractional bits. Deliberately
        /// **not** pre-divided by 4: the fill executor performs that
        /// conversion itself and rejects a nonzero fraction rather than
        /// truncating, so passing whole pixels here would silently discard
        /// the evidence that rejection is built on.
        pub upper_left_x: u16,
        pub upper_left_y: u16,
        pub lower_right_x: u16,
        pub lower_right_y: u16,
        /// The color image this fill targets, as staged by the preceding
        /// `SetColorImage`. Duplicated onto the command (rather than left
        /// for the visitor to track) so the execution-time color-target
        /// identity is derived from the same value plan time used.
        pub color_image: NeutralColorImage,
        /// The staged `SetFillColor` wire value. Required and present in
        /// Fill cycle; irrelevant to one-/two-cycle rectangles, whose color
        /// comes from the combiner, so those commands preserve its absence.
        pub fill_color: Option<NeutralFillColor>,
        /// Index into the owning plan's ordered access list of this
        /// command's first `RenderTarget` write access.
        pub first_access_index: u32,
        /// How many consecutive accesses starting at `first_access_index`
        /// belong to this fill. `1` for a full-width fill, the rectangle's
        /// pixel height otherwise.
        pub access_count: u32,
        /// Index into the owning plan's ordered access list of this fill's
        /// colour-image SEED read, or `None` when the fill covers the whole
        /// target and needs no seed.
        ///
        /// A partial fill patches into pixels it does not itself write, and
        /// those pixels must carry their real guest value rather than a
        /// fabricated zero -- the same thing `fn64-render-reference` gets by
        /// seeding its target from RDRAM before every raw-RDP task
        /// (`backend/imp.rs:440-447`). The declaring backend records which
        /// declared read carries those bytes; `None` is a positive statement
        /// that none is needed, not an absence of information.
        pub seed_access_index: Option<u32>,
        pub before: Option<RdpStateIdentity>,
        pub after: RdpStateIdentity,
    }

    /// Neutral carrier for one decoded `SYNC_FULL` **site** (RDP opcode 0x29).
    ///
    /// # This is a site, not a boundary observation
    ///
    /// The name is deliberate. A `RdpFullSyncSite` records that the backend
    /// walked a `SYNC_FULL` opcode at a known stream position and that the
    /// sole DP completion slot was proved free before anything was touched.
    /// It records nothing about whether a DP interrupt was subsequently
    /// raised or observed.
    ///
    /// The observation, when a producer can honestly make one, lives in the
    /// capture's own [`fn64_render_ir::FullSyncBoundary`] -- reachable from
    /// [`Self::boundary`] -- and specifically in its
    /// `interrupt_after == Asserted`. Nothing on this struct duplicates or
    /// summarizes that bit, because a second copy is a second thing to get
    /// out of sync with the first.
    ///
    /// Pushes zero [`ResourceAccess`] entries: a sync reads and writes no
    /// resource. That is not a simplification -- `SYNC_FULL`'s effect is on
    /// the RDP pipeline and the DP interrupt line, neither of which is a
    /// journaled resource region.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RdpFullSyncSite {
        pub location: RawDpcCommandLocation,
        pub raw_words: Box<[u32]>,
        /// Zero-based index of this site among the decoded stream's
        /// `SYNC_FULL` occurrences, matching
        /// [`fn64_render_ir::FullSyncOccurrence::ordinal`].
        pub ordinal: u32,
        /// The capture-time boundary record this site was bound to during
        /// decode, carried verbatim.
        ///
        /// `interrupt_after` is the *only* place an observation claim can
        /// live. A backend that merely reserved the DP slot supplies
        /// [`fn64_render_ir::DpInterruptState::Clear`] here; reading
        /// `Asserted` off this field is the sole way a consumer may conclude
        /// the interrupt was observed.
        pub boundary: fn64_render_ir::FullSyncBoundary,
        /// Whether the sole DP completion slot was proved free for this site
        /// before the backend touched anything.
        ///
        /// Nonclaim: `true` means a nonmutating reserve succeeded. It does
        /// **not** mean a DP event was scheduled, an interrupt was raised, or
        /// the guest observed one.
        pub dp_slot_reserved: bool,
    }

    /// Which wire command admitted this triangle: a genuine `RawTriangle`
    /// (0xC8-0xCF family) versus one synthesized from a `TextureRectangle`/
    /// `TextureRectangleFlip` (0x24/0x25) two-triangle expansion. Constructed
    /// only at the two admission sites in `production_adapter.rs` -- see
    /// that file's `RawTriangle`/`TextureRectangle` match arms.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TriangleSource {
        RawTriangle,
        TextureRectangle,
    }

    /// Pixel-space `left`/`top`/`right`/`bottom` bounds of a `TextureRectangle`
    /// draw, RT64's `FixedRect`-equivalent (`rt64_rdp.cpp:1232`). `None` on
    /// [`RdpTriangleCommand`] for `TriangleSource::RawTriangle`; `Some` only
    /// for `TriangleSource::TextureRectangle`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct RectViewportPixels {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    /// One neutral, borrowed semantic view of a decoded raw-DPC command.
    /// `#[non_exhaustive]` because T1's private wgpu decoder is the sole
    /// producer of the owning [`ExactValidatedRawDpcPlan`] and may need to
    /// widen this set (still bounded to the frozen TMEM-only scope) without
    /// an unrelated crate boundary break.
    #[derive(Clone, Copy, Debug)]
    #[non_exhaustive]
    pub enum RawDpcSemanticCommandRef<'plan> {
        /// `LoadBlock`/`LoadTile`/`LoadTLUT`: the complete materialized load
        /// semantics a physical executor needs, borrowed from the owning
        /// plan. [`TmemLoadSemantics::shape`] distinguishes which opcode
        /// produced it.
        TmemLoad(&'plan TmemLoadSemantics),
        /// A supported state/sync command carrying its own staged fields but
        /// no resource access -- required context for the load commands
        /// above.
        State(&'plan RdpStateCommand),
        /// One admitted `RawTriangle` draw command -- geometry only, no
        /// resource access and no before/after identity (see
        /// [`RdpTriangleCommand`]'s own doc for why).
        Triangle(&'plan RdpTriangleCommand),
        /// One admitted fill-cycle `FillRectangle` -- unlike every sibling
        /// here, this command declares N guest-visible `RenderTarget` write
        /// accesses (see [`RdpFillRectangleCommand`]).
        FillRectangle(&'plan RdpFillRectangleCommand),
        /// One decoded `SYNC_FULL` site. Declares zero resource accesses and,
        /// on its own, no DP-interrupt observation -- see
        /// [`RdpFullSyncSite`].
        FullSyncSite(&'plan RdpFullSyncSite),
    }

    /// Borrowed, nonextracting visitor over one validated plan's semantic
    /// commands and resource accesses. Implementors receive read-only views;
    /// nothing here can move a field out of the plan or reconstruct a
    /// constructor for it.
    pub trait ExactRawDpcPlanVisitor {
        fn command(&mut self, command: RawDpcSemanticCommandRef<'_>);
        fn access(&mut self, access: ResourceAccess);
    }

    /// Genuine `fn64-render`-owned neutral concrete representation of one
    /// validated raw-DPC plan. This is not type erasure: every field is a
    /// concrete fn64-render-ir/fn64-render value, never `Any`, `TypeId`, a
    /// downstream private type, or a downcast hook.
    ///
    /// Public access is nonextracting: the identity/count getters return
    /// `Copy` facts, and [`Self::visit`] lends command/access views through
    /// [`ExactRawDpcPlanVisitor`] without moving a field or exposing a
    /// constructor. There is no public constructor: the only route to one is
    /// [`ExactRawDpcPlanWriter`], reachable only through
    /// [`RawDpcBackendAuthority::begin_plan`] with the exact paired queue
    /// identity already checked.
    #[derive(Debug)]
    pub struct ExactValidatedRawDpcPlan {
        source_identity: RawDpcSubmissionIdentity,
        journal_identity: JournalIdentity,
        commands: Box<[OwnedSemanticCommand]>,
        accesses: Box<[ResourceAccess]>,
    }

    /// Owned storage backing one borrowed [`RawDpcSemanticCommandRef`]. Kept
    /// private: only [`ExactValidatedRawDpcPlan::visit`] ever turns this back
    /// into a borrowed view.
    #[derive(Clone, Debug)]
    enum OwnedSemanticCommand {
        TmemLoad(TmemLoadSemantics),
        State(RdpStateCommand),
        Triangle(RdpTriangleCommand),
        FillRectangle(RdpFillRectangleCommand),
        FullSyncSite(RdpFullSyncSite),
    }

    impl OwnedSemanticCommand {
        fn as_ref(&self) -> RawDpcSemanticCommandRef<'_> {
            match self {
                Self::TmemLoad(semantics) => RawDpcSemanticCommandRef::TmemLoad(semantics),
                Self::State(state) => RawDpcSemanticCommandRef::State(state),
                Self::Triangle(triangle) => RawDpcSemanticCommandRef::Triangle(triangle),
                Self::FillRectangle(fill) => RawDpcSemanticCommandRef::FillRectangle(fill),
                Self::FullSyncSite(site) => RawDpcSemanticCommandRef::FullSyncSite(site),
            }
        }
    }

    impl ExactValidatedRawDpcPlan {
        pub const fn source_identity(&self) -> RawDpcSubmissionIdentity {
            self.source_identity
        }

        pub fn command_count(&self) -> usize {
            self.commands.len()
        }

        pub const fn journal_identity(&self) -> JournalIdentity {
            self.journal_identity
        }

        /// Lend every semantic command, then every resource access, to
        /// `visitor`. Order matches construction order; no field is moved.
        /// Generic over `V: ExactRawDpcPlanVisitor`, monomorphized per
        /// concrete visitor type at every call site -- no `dyn` here, so
        /// this call is never itself a vtable dispatch (only the trait-
        /// object entry into `RenderBackend`'s own methods is; see the sole-
        /// dynamic-dispatch documentation on that trait).
        pub fn visit<V: ExactRawDpcPlanVisitor>(&self, visitor: &mut V) {
            for command in &self.commands {
                visitor.command(command.as_ref());
            }
            for access in self.accesses.iter().copied() {
                visitor.access(access);
            }
        }
    }

    // ------------------------------------------------------------------
    // Sealed role split, request, and cross-crate plan-writer seam
    // ------------------------------------------------------------------

    /// ABI-owned role: the submission queue, the guest-commit authority, and
    /// a diagnostic retirement ledger for one raw-DPC production lifecycle.
    /// Its `finalize_and_submit` owns queue readiness and issuance
    /// internally, so a bare `DecodedTicket`/`SubmittedTicket` never escapes
    /// to a caller through this type.
    #[derive(Debug)]
    pub struct RawDpcAbiSession {
        queue: SubmissionQueue,
        guest: GuestCommitAuthority,
        ledger: RetirementLedger,
    }

    /// Backend-owned role: the paired completion authority. Its `begin_plan`
    /// is the sole route to a plan-writing capability, and it rejects an
    /// unpaired request's queue identity before any plan field can be
    /// written.
    #[derive(Debug)]
    pub struct RawDpcBackendAuthority {
        authority: BackendCompletionAuthority,
    }

    impl RawDpcBackendAuthority {
        /// Consume this authority into a [`RawDpcCoordinator`] double-
        /// buffered over the backend's own physical state type `P`. This is
        /// the sole route to a coordinator; a backend obtains one exactly
        /// once, at construction, and drives every raw-DPC submission's
        /// execution/publication through it from then on. `initial` seeds
        /// the active slot before any submission has ever completed
        /// execution -- `physical()` is never `None`-shaped.
        pub fn into_coordinator<P>(self, initial: P) -> RawDpcCoordinator<P> {
            RawDpcCoordinator {
                authority: self,
                slots: [Some(initial), None],
                active: 0,
                ready: None,
            }
        }
    }

    /// Private ready-publication metadata a coordinator records at
    /// [`RawDpcCoordinator::complete_execution`] time and consumes at
    /// [`RawDpcCoordinator::prepare_publication`] time. Never leaves this
    /// module: there is no public constructor, getter, or `Clone`/`Copy` --
    /// a caller cannot fabricate one, echo one back, or hold one past the
    /// coordinator that recorded it.
    ///
    /// `retirement_slot` is a private `Rc::clone` of the exact
    /// `SubmittedRawDpcRetirement`'s shared slot this ordinal's
    /// `complete_execution` call observed -- the same allocation the
    /// [`RawDpcRetirementHandle`] the session's own ledger holds also
    /// points at. It exists for two reasons: [`RawDpcCoordinator::
    /// prepare_publication`] can `Rc::ptr_eq` it against the capsule's own
    /// retirement to prove the capsule being published really is the one
    /// this exact ready slot was prepared for (not merely a
    /// queue/submission coincidence), and a coordinator that never sees a
    /// matching `prepare_publication` call for an ordinal can still poll
    /// this clone's outcome to notice the abandoned/rejected candidate and
    /// reap its slot -- without a public capsule-side accessor a caller
    /// could otherwise use to reach the same information from outside this
    /// module.
    struct ReadyPhysicalSlot {
        queue: QueueIdentity,
        submission: SubmissionIdentity,
        slot_index: u8,
        retirement_slot: Rc<RetirementSlot>,
    }

    /// Owns one backend's paired [`RawDpcBackendAuthority`] together with
    /// its double-buffered physical state `P` and the private metadata that
    /// binds one specific prepared slot to one specific submission. `P` is
    /// the backend's own physical candidate/state value -- a plain owned
    /// type, never a callback or trait object -- so this type is generic
    /// over it without knowing or caring about its fields.
    ///
    /// **Why two slots, not one `mem::replace`.** `P` is an arbitrary
    /// backend type whose `Drop` this module does not control; replacing
    /// the *active* slot in place would run the old active `P`'s `Drop`
    /// exactly at the moment a fresh candidate becomes current, meaning an
    /// arbitrary (and, to this module, unauditable) destructor could panic
    /// on the same instruction that performs the durable physical
    /// publication -- precisely the "no Drop of P after the flip" property
    /// [`ReadyPublication::commit`] must hold. With two slots, the inactive
    /// slot is overwritten -- and whatever `P` used to live there is
    /// dropped -- entirely inside [`Self::complete_execution`], a fallible
    /// step that runs *before* any publication exists, never inside
    /// `commit`'s straight-line body. `commit` only ever flips `active`,
    /// an integer index; it drops nothing.
    pub struct RawDpcCoordinator<P> {
        authority: RawDpcBackendAuthority,
        slots: [Option<P>; 2],
        active: u8,
        ready: Option<ReadyPhysicalSlot>,
    }

    impl<P> RawDpcCoordinator<P> {
        /// The currently-published physical state -- the value the last
        /// successful [`ReadyPublication::commit`] flipped `active` to, or
        /// `into_coordinator`'s `initial` if none has published yet.
        pub fn physical(&self) -> &P {
            self.slots[self.active as usize]
                .as_ref()
                .expect("the active coordinator slot is always occupied")
        }

        /// Forwards to [`RawDpcBackendAuthority::begin_plan`] against this
        /// coordinator's own paired authority.
        pub fn begin_plan(&self, request: RawDpcPlanRequest) -> ExactRawDpcPlanWriter {
            self.authority.begin_plan(request)
        }

        /// Forwards to [`BoundSubmittedRawDpc::execution_view`] against this
        /// coordinator's own paired authority.
        pub fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
            &self,
            bound: &BoundSubmittedRawDpc,
            plan_visitor: &mut PV,
            view: &mut V,
        ) {
            bound.execution_view(&self.authority, plan_visitor, view);
        }

        /// Complete one bound submission's backend-side execution: validate
        /// the paired authority/queue exactly as
        /// [`BoundSubmittedRawDpc::into_backend_prepared`] always has, issue
        /// the receipted [`GpuCompleteTicket`], and -- *before* returning --
        /// overwrite this coordinator's currently-*inactive* slot with
        /// `next_physical`. This is the one place a candidate `P` (and
        /// whatever `P` previously occupied that slot, now dropped) enters
        /// or leaves the coordinator; it happens entirely inside this
        /// fallible, ordinary-Rust method, never inside
        /// [`ReadyPublication::commit`]'s straight-line body. Records the
        /// private `(queue, submission, inactive slot index)` triple this
        /// ordinal's eventual [`Self::prepare_publication`] call must match
        /// exactly.
        pub fn complete_execution(
            &mut self,
            bound: BoundSubmittedRawDpc,
            effects: fn64_render_ir::BackendEffectReport,
            next_physical: P,
        ) -> Result<BackendPreparedRawDpc, ValidationError> {
            let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
            let inactive = 1 - self.active;
            self.slots[inactive as usize] = Some(next_physical);
            self.ready = Some(ReadyPhysicalSlot {
                queue: self.authority.authority.queue_identity(),
                submission: prepared.submission(),
                slot_index: inactive,
                retirement_slot: Rc::clone(&prepared.retirement.slot),
            });
            Ok(prepared)
        }

        /// Complete one bound submission's backend-side execution exactly
        /// like [`Self::complete_execution`] -- same authority/queue
        /// validation, via the same [`BoundSubmittedRawDpc::
        /// into_backend_prepared`] call -- but for a submission whose
        /// effects never touch physical state at all (a triangle-only raw-
        /// DPC packet has nothing to load into or publish out of a `P`).
        /// Records the ready-publication metadata against the *currently
        /// active* slot index, not the inactive one `complete_execution`
        /// uses, and never reads, writes, or constructs a `P`: neither slot
        /// in `self.slots` is touched. `prepare_publication`/`commit` need
        /// no knowledge of which of the two methods produced a given ready
        /// record -- `commit` flipping `active` to its own already-active
        /// value is simply a no-op write.
        ///
        /// Matches [`Self::complete_execution`]'s own replacement semantics
        /// exactly: like that method, this one unconditionally overwrites
        /// any prior `self.ready`, with no busy-gate or rejection if an
        /// earlier ready-but-unpublished completion is still outstanding.
        ///
        /// Takes no caller-supplied `effects`: a generic coordinator cannot
        /// mechanically prove an arbitrary [`fn64_render_ir::
        /// BackendEffectReport`] describes zero physical writes, so it must
        /// never accept one. Instead this method constructs the report
        /// itself with an explicitly empty write list --
        /// [`fn64_render_ir::BackendEffectReport::try_new`] validates that
        /// empty list against `bound`'s own packet journal, so a submission
        /// whose plan actually declares any write access is rejected here,
        /// structurally, before `self.ready` is ever touched.
        pub fn complete_execution_preserving_physical(
            &mut self,
            bound: BoundSubmittedRawDpc,
        ) -> Result<BackendPreparedRawDpc, ValidationError> {
            let effects =
                fn64_render_ir::BackendEffectReport::try_new(bound.submitted.packet(), Vec::new())?;
            let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
            self.ready = Some(ReadyPhysicalSlot {
                queue: self.authority.authority.queue_identity(),
                submission: prepared.submission(),
                slot_index: self.active,
                retirement_slot: Rc::clone(&prepared.retirement.slot),
            });
            Ok(prepared)
        }

        /// Complete one bound submission that produced real guest-visible
        /// writes but no physical-state successor -- a `FillRectangle`
        /// color-target write with no TMEM load. Identical to
        /// [`Self::complete_execution_preserving_physical`] in every
        /// physical-state respect (records against the *currently active*
        /// slot index, never reads, writes, or constructs a `P`, same
        /// `into_backend_prepared` authority/queue validation), and
        /// identical to [`Self::complete_execution`] in accepting a
        /// caller-supplied [`fn64_render_ir::BackendEffectReport`].
        ///
        /// Why accepting `effects` here is safe where
        /// `complete_execution_preserving_physical` deliberately refuses to:
        /// that method refuses because it must *prove* zero writes, and a
        /// generic coordinator cannot inspect an arbitrary report to do so.
        /// This method makes no zero-write claim at all -- the report was
        /// already validated against the packet's own journal by
        /// [`fn64_render_ir::BackendEffectReport::try_new`], which is the
        /// same proof [`Self::complete_execution`] relies on. What this
        /// method additionally does *not* claim is anything about `P`: it
        /// never touches either slot.
        pub fn complete_execution_preserving_physical_with_effects(
            &mut self,
            bound: BoundSubmittedRawDpc,
            effects: fn64_render_ir::BackendEffectReport,
        ) -> Result<BackendPreparedRawDpc, ValidationError> {
            let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
            self.ready = Some(ReadyPhysicalSlot {
                queue: self.authority.authority.queue_identity(),
                submission: prepared.submission(),
                slot_index: self.active,
                retirement_slot: Rc::clone(&prepared.retirement.slot),
            });
            Ok(prepared)
        }

        /// Validate every queue/submission/ready-slot fact this coordinator
        /// privately recorded at [`Self::complete_execution`] time against
        /// `capsule`, *before* any physical publication exists as a value --
        /// this is the method v11 describes as validating "authority, queue,
        /// submission, proposal, and ready-slot identity while durable state
        /// is unchanged." A mismatch (no matching `complete_execution` call
        /// for this capsule's submission, a foreign queue, or -- the
        /// strongest check -- a retirement slot that is not the exact same
        /// `Rc` allocation this coordinator privately cloned at
        /// `complete_execution` time) traps loudly here; nothing about
        /// `commit`'s later straight-line body can ever observe or reject a
        /// mismatch, because none can exist by the time a
        /// [`ReadyPublication`] is returned.
        pub fn prepare_publication<'coord, 'fabric>(
            &'coord mut self,
            mut capsule: ReadyRawDpcCommitCapsule<'fabric>,
        ) -> ReadyPublication<'coord, 'fabric, P> {
            let ready = self
                .ready
                .take()
                .expect("prepare_publication requires a prior complete_execution for this ordinal");
            assert_eq!(
                ready.queue,
                capsule.committed.queue(),
                "prepared physical slot's queue does not match this capsule's queue"
            );
            assert_eq!(
                ready.submission,
                capsule.committed.submission(),
                "prepared physical slot was recorded for a different submission"
            );
            assert!(
                Rc::ptr_eq(&ready.retirement_slot, &capsule.retirement.slot),
                "prepared physical slot's retirement is not the same allocation as this \
                 capsule's own retirement -- this capsule was not the one complete_execution \
                 prepared this ready slot for"
            );
            // Every check above has passed: this ordinal is genuinely ready
            // for its physical publication. Advance to `PhysicalPrepare`
            // here, before `ReadyPublication` is returned, so that stage is
            // observable the instant a caller holds one -- `commit` itself
            // performs no further stage advance, only the unconditional
            // `Published` write.
            capsule
                .retirement
                .advance_stage(RawDpcRetirementStage::PhysicalPrepare);
            ReadyPublication {
                coordinator: self,
                ready_index: ready.slot_index,
                capsule,
            }
        }
    }

    /// Move-only, `#[must_use]` terminal publication: the sole value
    /// [`RawDpcCoordinator::prepare_publication`] produces, and the sole
    /// route to [`CommittedRawDpcOutcome`]. By the time one exists, every
    /// queue/submission/ready-slot check has already passed -- there is
    /// nothing left for [`Self::commit`] to validate, reject, or panic on
    /// except the unconditional, straight-line publication itself.
    ///
    /// Dropping an unconsumed value cancels: the inner
    /// [`fn64_runtime::device::ReadyDpcFabricCommit`] rolls back via its own
    /// armed-gated `Drop`, and the capsule's own retirement records exactly
    /// one `Rejected` at `FabricPrepare` -- `active` is never touched,
    /// because `Drop` runs no code of this type's own at all (it borrows,
    /// rather than owns, the coordinator, so there is nothing here to flip
    /// back).
    #[must_use = "an unused ReadyPublication cancels its capsule on drop"]
    pub struct ReadyPublication<'coord, 'fabric, P> {
        coordinator: &'coord mut RawDpcCoordinator<P>,
        ready_index: u8,
        capsule: ReadyRawDpcCommitCapsule<'fabric>,
    }

    impl<'coord, 'fabric, P> ReadyPublication<'coord, 'fabric, P> {
        /// The sole terminal step: flip the coordinator's `active` index to
        /// the already-prepared slot (the first, and only, durable physical
        /// move), commit the concrete fabric transition, and
        /// unconditionally record `Published`. No callback, trait object,
        /// allocation, lookup, `assert`, `Result`, or `Drop` of `P` runs
        /// after the flip -- the flip is a `u8` write, `fabric.commit()` is
        /// T2's own fixed infallible body, and disarming retirement is a
        /// `Cell` write, in that order, with nothing between them that can
        /// fail.
        pub fn commit(self) -> CommittedRawDpcOutcome {
            self.coordinator.active = self.ready_index;
            let ReadyRawDpcCommitCapsule {
                committed,
                fabric,
                retirement,
                ..
            } = self.capsule;
            let submission = committed.submission();
            fabric.commit();
            retirement.disarm_published();
            CommittedRawDpcOutcome { submission }
        }
    }

    /// Split one fresh [`TicketAuthoritySet`] into the ABI session and
    /// backend authority roles this production seam uses. The third role
    /// ([`fn64_render_ir::GuestCommitAuthority`]) lives inside the session;
    /// nothing outside this module can reach it independently.
    pub fn new_raw_dpc_roles() -> Result<(RawDpcAbiSession, RawDpcBackendAuthority), ValidationError>
    {
        let (queue, authority, guest) = TicketAuthoritySet::try_new()?.into_roles();
        Ok((
            RawDpcAbiSession {
                queue,
                guest,
                ledger: RetirementLedger::default(),
            },
            RawDpcBackendAuthority { authority },
        ))
    }

    impl RawDpcAbiSession {
        /// Stamp one owned capture with this session's queue identity,
        /// producing the request a backend can turn into a plan. This is the
        /// sole way to obtain a [`RawDpcPlanRequest`]; nothing else can
        /// fabricate one with an unrelated queue identity.
        pub fn plan_request(&self, capture: crate::OwnedRawDpcCapture) -> RawDpcPlanRequest {
            RawDpcPlanRequest {
                capture,
                queue: self.queue.identity(),
            }
        }

        /// Consume a preflighted, plan-carrying submission plus the ABI's
        /// captured guest reads and issue it, entirely inside this session:
        /// queue readiness, ticket issuance, and retirement arming never
        /// expose a `DecodedTicket`/`SubmittedTicket` to the caller. Records
        /// the new retirement's diagnostic handle in this session's ledger
        /// and returns a copy of it alongside the sealed bound value.
        /// Returns only the sealed [`BoundSubmittedRawDpc`] -- never a cloned
        /// [`RawDpcRetirementHandle`]. The ledger stays entirely
        /// session-private: this method still records a handle into
        /// `self.ledger` for the session's own diagnostic bookkeeping, but
        /// does not hand a second copy of it to the caller. A caller that
        /// needs to observe an ordinal's terminal outcome does so through
        /// the session itself, not a handle it was given at submit time.
        pub fn finalize_and_submit(
            &mut self,
            planned: PlannedRawDpcSubmission,
            capture: DeferredGuestReadCapture,
        ) -> Result<BoundSubmittedRawDpc, ValidationError> {
            let decoded = planned.preflight.finalize(capture)?;
            let ready = self.queue.try_ready_submission()?;
            let submitted = ready.issue(decoded);
            let (retirement, handle) = SubmittedRawDpcRetirement::new_pair(submitted.identity());
            self.ledger.record(handle);
            Ok(BoundSubmittedRawDpc {
                plan: planned.plan,
                submission_identity: submitted.identity(),
                queue: submitted.queue(),
                ordinal: submitted.ordinal(),
                submitted,
                retirement,
            })
        }

        /// Advance one backend-prepared submission through zero
        /// guest-visible writes -- one of this slice's two admitted
        /// guest-commit shapes, the other being exactly one
        /// `RenderTarget`-purpose write via
        /// [`Self::commit_single_guest_render_target_write`] (FillRectangle
        /// guest-write publication design card) -- into the
        /// sealed [`GuestCommittedRawDpc`], moving plan, retirement, and
        /// ticket together. Consumes `prepared` (not a bare
        /// `GpuCompleteTicket`) and returns the sealed wrapper (not a bare
        /// `GuestCommittedTicket`), so no raw ticket crosses this call in
        /// either direction.
        ///
        /// Traps if the prepared value's queue does not match this session's
        /// own queue. Before issuing the guest-commit receipt, this method
        /// hands `GuestCommitEffectReport::try_new` an empty write list
        /// against `prepared`'s own completed ticket -- that constructor
        /// re-derives `packet.journal().guest_write_accesses()` from the
        /// ticket's own packet and requires the supplied list's length to
        /// match it exactly (`fn64-render-ir`'s `validate_effects`), so a
        /// packet whose journal actually declares any guest-visible write
        /// fails loudly here with `EffectCountMismatch`, regardless of what
        /// this method is named or what an earlier decode-time check already
        /// concluded. v10 §3 already rejects guest-visible writes at plan
        /// time, so this call is deliberately redundant with that earlier
        /// gate -- defense in depth, not the sole enforcement point -- but it
        /// makes "zero guest writes" this method's own re-checked fact
        /// rather than an inherited assumption from the plan/journal it
        /// never re-reads.
        pub fn commit_zero_guest_writes(
            &mut self,
            prepared: BackendPreparedRawDpc,
        ) -> Result<GuestCommittedRawDpc, ValidationError> {
            assert!(
                prepared.complete.queue() == self.queue.identity(),
                "BackendPreparedRawDpc does not belong to this session's queue"
            );
            let effects = GuestCommitEffectReport::try_new(&prepared.complete, Vec::new())?;
            let receipt: GuestCommitReceipt = self.guest.issue(&prepared.complete, effects)?;
            let committed = prepared.complete.commit_guest(receipt)?;
            let mut retirement = prepared.retirement;
            retirement.advance_stage(RawDpcRetirementStage::GuestReceipt);
            Ok(GuestCommittedRawDpc {
                plan: prepared.plan,
                committed,
                retirement,
            })
        }

        /// Advance one backend-prepared submission whose journal declares
        /// exactly one guest-visible `RenderTarget` write into the sealed
        /// [`GuestCommittedRawDpc`]. A one-element convenience wrapper over
        /// [`Self::commit_guest_render_target_writes`], retained because it
        /// is the shape every existing caller and test already uses; it adds
        /// no check of its own and cannot diverge from the N-write path.
        ///
        /// `write` must be the exact [`CompletedWrite`] the backend already
        /// staged and reported inside `prepared`'s own `BackendEffectReport`
        /// (checked structurally by `GuestCommitEffectReport::try_new`'s
        /// content-digest equality check -- supplying any other content,
        /// byte count, or access fails loudly, it is not merely re-trusted).
        pub fn commit_single_guest_render_target_write(
            &mut self,
            prepared: BackendPreparedRawDpc,
            write: CompletedWrite,
        ) -> Result<GuestCommittedRawDpc, ValidationError> {
            self.commit_guest_render_target_writes(prepared, vec![write])
        }

        /// Advance one backend-prepared submission whose journal declares
        /// N guest-visible `RenderTarget` writes -- the general
        /// FillRectangle color-target commit path, and the N-write
        /// counterpart to [`Self::commit_zero_guest_writes`] -- into the
        /// sealed [`GuestCommittedRawDpc`], moving plan, retirement, and
        /// ticket together, exactly like `commit_zero_guest_writes`.
        ///
        /// **Why N and not one.** `fn64-render-wgpu`'s `plan_fill` pushes
        /// one guest-write journal access *per row* for a partial-width
        /// fill, because a partial-width rectangle's rows occupy disjoint,
        /// width-strided RDRAM ranges. Collapsing them into a single range
        /// would declare untouched inter-row bytes as written -- an 11x3
        /// fill into a 320px RGBA16 image writes 66 bytes but spans 1302,
        /// so ~95% of a collapsed claim would be false. The guest-write
        /// journal is exactly what this commit contract verifies, so it
        /// must not lie.
        ///
        /// `writes` must be the exact ordered [`CompletedWrite`] list the
        /// backend already staged and reported inside `prepared`'s own
        /// `BackendEffectReport`. Order is load-bearing:
        /// `GuestCommitEffectReport::try_new` zips this list against the
        /// packet's own `guest_write_accesses()` element for element, and
        /// both filters preserve journal order, so a reordered list is a
        /// loud `EffectAccessMismatch`, never a silently accepted permutation.
        ///
        /// An empty `writes` is a legal input and deliberately not gated
        /// here: `validate_effects` rejects it against a nonempty journal
        /// and accepts it against an empty one, which is exactly
        /// [`Self::commit_zero_guest_writes`]'s behavior. A redundant gate
        /// would be a second, independently-maintained rejection path for
        /// the same fact.
        ///
        /// Traps if the prepared value's queue does not match this
        /// session's own queue, identically to `commit_zero_guest_writes`.
        /// Before issuing the guest-commit receipt, this method first
        /// checks **every** element's own access mode/purpose against the
        /// single admitted shape (`AccessMode::Write`,
        /// `AccessPurpose::RenderTarget`) -- a structural pre-check, not the
        /// sole enforcement; `GuestCommitEffectReport::try_new`'s own
        /// region-purpose-blind count/identity/content check (mirroring
        /// `commit_zero_guest_writes`'s own re-checked-fact pattern) still
        /// runs regardless and is what actually proves these writes are the
        /// ones the backend staged.
        ///
        /// Nonclaim: committing here modifies no guest RDRAM byte. A
        /// [`CompletedWrite`] is a range plus a content digest, not bytes in
        /// motion; the RDRAM copyback is a separate, deferred slice.
        pub fn commit_guest_render_target_writes(
            &mut self,
            prepared: BackendPreparedRawDpc,
            writes: Vec<CompletedWrite>,
        ) -> Result<GuestCommittedRawDpc, ValidationError> {
            assert!(
                prepared.complete.queue() == self.queue.identity(),
                "BackendPreparedRawDpc does not belong to this session's queue"
            );
            for write in &writes {
                if write.access().mode() != AccessMode::Write
                    || write.access().purpose() != AccessPurpose::RenderTarget
                {
                    return Err(ValidationError::GuestRenderTargetWriteShapeMismatch {
                        mode: access_mode_name(write.access().mode()),
                        purpose: access_purpose_name(write.access().purpose()),
                    });
                }
            }
            let effects = GuestCommitEffectReport::try_new(&prepared.complete, writes)?;
            let receipt: GuestCommitReceipt = self.guest.issue(&prepared.complete, effects)?;
            let committed = prepared.complete.commit_guest(receipt)?;
            let mut retirement = prepared.retirement;
            retirement.advance_stage(RawDpcRetirementStage::GuestReceipt);
            Ok(GuestCommittedRawDpc {
                plan: prepared.plan,
                committed,
                retirement,
            })
        }

        /// Seal a guest-committed submission against the concrete,
        /// backend-retained [`fn64_runtime::device::ReadyDpcFabricCommit`]
        /// into the terminal [`ReadyRawDpcCommitCapsule`] -- the sole route
        /// to a publishable capsule -- exact v11 signature
        /// (`Result<ReadyRawDpcCommitCapsule<'a>, ValidationError>`; the
        /// `Result` covers the one caller-bug case this session alone can
        /// detect, and leaves room for a future fallible check -- e.g. T1's
        /// proposal/generation identity -- without another signature break).
        ///
        /// This is the ABI-side half of v11's split seal/unseal shape: it
        /// validates only what this session alone owns (`committed`'s queue
        /// against this session's own queue). It does **not** validate
        /// authority, physical-slot, or proposal identity -- this session has
        /// no view of the backend's own physical state at all, by design
        /// (see [`RawDpcCoordinator`]); that full validation happens inside
        /// [`RawDpcCoordinator::prepare_publication`], the sole place
        /// backend-owned physical/ready-slot state is available, immediately
        /// before a [`ReadyPublication`] can exist.
        pub fn seal_publication<'fabric>(
            &mut self,
            committed: GuestCommittedRawDpc,
            fabric: fn64_runtime::device::ReadyDpcFabricCommit<'fabric>,
        ) -> Result<ReadyRawDpcCommitCapsule<'fabric>, ValidationError> {
            assert!(
                committed.committed.queue() == self.queue.identity(),
                "GuestCommittedRawDpc does not belong to this session's queue"
            );
            let mut retirement = committed.retirement;
            retirement.advance_stage(RawDpcRetirementStage::FabricPrepare);
            Ok(ReadyRawDpcCommitCapsule {
                plan: committed.plan,
                committed: committed.committed,
                fabric,
                retirement,
            })
        }
    }

    /// Stable diagnostic name for a [`ValidationError::
    /// GuestRenderTargetWriteShapeMismatch`] field. `AccessMode::name` is
    /// crate-private to `fn64-render-ir`, so this mirrors it locally rather
    /// than widening that visibility for one error-formatting call site.
    const fn access_mode_name(mode: AccessMode) -> &'static str {
        match mode {
            AccessMode::Read => "Read",
            AccessMode::Write => "Write",
            AccessMode::ReadWrite => "ReadWrite",
        }
    }

    /// Stable diagnostic name for a [`ValidationError::
    /// GuestRenderTargetWriteShapeMismatch`] field. `AccessPurpose::name` is
    /// crate-private to `fn64-render-ir`, so this mirrors it locally rather
    /// than widening that visibility for one error-formatting call site.
    const fn access_purpose_name(purpose: AccessPurpose) -> &'static str {
        match purpose {
            AccessPurpose::CommandDecode => "CommandDecode",
            AccessPurpose::UploadSource => "UploadSource",
            AccessPurpose::TmemLoadSource => "TmemLoadSource",
            AccessPurpose::TmemLoadDestination => "TmemLoadDestination",
            AccessPurpose::RenderTarget => "RenderTarget",
            AccessPurpose::DepthTarget => "DepthTarget",
            AccessPurpose::CopySource => "CopySource",
            AccessPurpose::CopyDestination => "CopyDestination",
            AccessPurpose::ReinterpretSource => "ReinterpretSource",
            AccessPurpose::ReinterpretDestination => "ReinterpretDestination",
            AccessPurpose::ViScanout => "ViScanout",
            AccessPurpose::CaptureSource => "CaptureSource",
            AccessPurpose::CaptureDestination => "CaptureDestination",
            AccessPurpose::GuestReadbackSource => "GuestReadbackSource",
            AccessPurpose::GuestReadbackDestination => "GuestReadbackDestination",
        }
    }

    /// Stamped request: one owned capture bound to the ABI session's queue
    /// identity. Only [`RawDpcBackendAuthority::begin_plan`] can consume it,
    /// and only after checking that stamp against its own authority's queue
    /// identity. `begin_plan` takes this type by value, so the same request
    /// cannot mint two writers or two plans.
    ///
    /// ```compile_fail
    /// use fn64_render::{new_raw_dpc_roles, OwnedRawDpcCapture};
    /// # fn capture() -> OwnedRawDpcCapture { unimplemented!() }
    /// let (session, authority) = new_raw_dpc_roles().unwrap();
    /// let request = session.plan_request(capture());
    /// let _first = authority.begin_plan(request);
    /// // `request` was moved into the first `begin_plan` call; reusing it
    /// // here to mint a second writer for the same stamped request is a
    /// // move-after-move compile error, not a runtime check.
    /// let _second = authority.begin_plan(request);
    /// ```
    #[derive(Debug)]
    pub struct RawDpcPlanRequest {
        capture: crate::OwnedRawDpcCapture,
        queue: QueueIdentity,
    }

    impl RawDpcPlanRequest {
        pub const fn queue(&self) -> QueueIdentity {
            self.queue
        }

        pub const fn capture(&self) -> &crate::OwnedRawDpcCapture {
            &self.capture
        }
    }

    impl RawDpcBackendAuthority {
        /// Sole route to a plan-writing capability. Consumes `request` by
        /// value -- the stamped queue/capture state moves into the returned
        /// writer, so the same request cannot be reused to mint a second
        /// writer or a second plan. Traps immediately, before any plan field
        /// can be written, if `request` was not stamped by this authority's
        /// paired queue -- an unrelated session's request can never reach
        /// [`ExactRawDpcPlanWriter`].
        pub fn begin_plan(&self, request: RawDpcPlanRequest) -> ExactRawDpcPlanWriter {
            assert!(
                self.authority.queue_identity() == request.queue,
                "RawDpcPlanRequest is not paired with this backend authority's queue"
            );
            ExactRawDpcPlanWriter {
                capture: request.capture,
                commands: Vec::new(),
                accesses: Vec::new(),
            }
        }
    }

    /// Private-field, `fn64-render-wgpu`-facing plan-building handle. Its
    /// existence already proves the exact paired-queue check
    /// [`RawDpcBackendAuthority::begin_plan`] performed; every push method is
    /// infallible with respect to that pairing, and [`Self::finish`] is the
    /// sole route to a sealed [`PlannedRawDpcSubmission`]. There is no public bare
    /// constructor for either the plan or the planned submission: `finish`
    /// derives `source_identity` from the writer's own owned capture and
    /// `journal_identity` from the journal actually used to build `journal`,
    /// so the resulting plan and the preflight it is sealed together with
    /// can never disagree about which capture or journal they describe --
    /// there is no separate identity parameter a caller could mismatch. The
    /// writer owns its capture outright (moved out of the consumed request by
    /// [`RawDpcBackendAuthority::begin_plan`]), not a borrow of it, so a
    /// second writer for the same request cannot exist even transiently.
    #[derive(Debug)]
    pub struct ExactRawDpcPlanWriter {
        capture: crate::OwnedRawDpcCapture,
        commands: Vec<OwnedSemanticCommand>,
        accesses: Vec<ResourceAccess>,
    }

    impl ExactRawDpcPlanWriter {
        pub const fn capture(&self) -> &crate::OwnedRawDpcCapture {
            &self.capture
        }

        /// How many [`ResourceAccess`] entries this writer has pushed so
        /// far -- i.e. the index the *next* pushed access will occupy.
        ///
        /// Exists so a caller about to push a run of accesses can record
        /// its own `first_access_index` from the writer's own state rather
        /// than tracking a parallel counter that could drift from it. Read
        /// immediately before the matching push; reading it after would
        /// name the run's end, not its start.
        pub fn access_count(&self) -> u32 {
            self.accesses.len() as u32
        }

        /// Pushes one admitted TMEM load and **every** source
        /// [`ResourceAccess`] it declares followed by its *first*
        /// destination access, in the decoder's exact journal order.
        ///
        /// The decoder emits a load's accesses as one contiguous run of
        /// `N` `TmemLoadSource` reads followed by `M` `TmemLoadDestination`
        /// writes (`tmem::wire::source_accesses` builds the source run,
        /// then `tmem::wire::transfer_plan` appends the destination run to
        /// the same vector). `N` is greater than one for every
        /// partial-width `LoadTile` -- one read per source row -- so this
        /// method pushes `load.sources()` whole rather than a single
        /// access. The caller pushes the remaining `M - 1` destination
        /// fragments with [`Self::push_command_decode_access`] immediately
        /// after, keeping `self.accesses` position-for-position identical
        /// to the journal, which is exactly what [`Self::finish`]'s
        /// element-wise check requires.
        ///
        /// Source-before-destination is load-bearing, not cosmetic:
        /// `fn64_render_ir::validate_effects` compares a backend's writes
        /// against the journal by position under an equal-length
        /// precondition, so pushing the destination before the sources
        /// would mis-commit guest memory rather than fail loudly.
        ///
        /// # Panics
        ///
        /// Panics if `load` declares no source access. A TMEM load always
        /// reads at least one range; a load with an empty source run is a
        /// decoder bug, not a no-op to be admitted quietly. The type
        /// cannot express it either -- `sources` is validated nonempty at
        /// construction.
        pub fn push_tmem_load(&mut self, load: TmemLoadSemantics) {
            assert!(
                !load.sources.is_empty(),
                "a TMEM load always reads at least one source access"
            );
            self.accesses.extend_from_slice(&load.sources);
            self.accesses.push(load.destination);
            self.commands.push(OwnedSemanticCommand::TmemLoad(load));
        }

        pub fn push_state(&mut self, state: RdpStateCommand) {
            self.commands.push(OwnedSemanticCommand::State(state));
        }

        /// Pushes zero [`ResourceAccess`] entries -- see
        /// [`RdpTriangleCommand`]'s own doc for why a triangle draw command
        /// carries no resource access and no before/after identity.
        pub fn push_triangle(&mut self, triangle: RdpTriangleCommand) {
            self.commands.push(OwnedSemanticCommand::Triangle(triangle));
        }

        pub fn push_command_decode_access(&mut self, access: ResourceAccess) {
            self.accesses.push(access);
        }

        /// Pushes the [`ResourceAccess`] entries one admitted
        /// `TextureRectangle` declares for its destination rectangle, in the
        /// exact order the decoder produced them.
        ///
        /// A texrect is admitted as two [`RdpTriangleCommand`]s (RT64's own
        /// two-triangle decomposition of one rectangle), and `push_triangle`
        /// pushes no access. This method carries the rectangle's *destination
        /// writes* -- which the decoder's `plan_texture_rectangle` derived
        /// from the same rasterized extent -- so the journal has a write to
        /// order the texrect against, exactly as a fill's does.
        ///
        /// Call this **before** the two `push_triangle` calls for the same
        /// rectangle, so `self.accesses` stays in decoder order: the decoder
        /// pushes a texrect's accesses at the point it decodes the command,
        /// and `finish` compares the two lists position by position.
        ///
        /// Unlike [`Self::push_fill_rectangle`], an empty slice is accepted
        /// and pushes nothing. A texrect legitimately declares no write when
        /// its destination is not provable at decode time (no staged
        /// `SetColorImage`, a flip, an off-image or degenerate extent -- see
        /// `plan_texture_rectangle`'s own contract); that is the pre-existing
        /// behavior for every texrect, not an admitted fill's "declares no
        /// write" decoder bug.
        pub fn push_texture_rectangle_accesses(&mut self, accesses: &[ResourceAccess]) {
            self.accesses.extend_from_slice(accesses);
        }

        /// Pushes one admitted `FillRectangle` and **every**
        /// [`ResourceAccess`] it declares, in the exact order the decoder
        /// produced them.
        ///
        /// `accesses` is a slice, not a single access, precisely because a
        /// partial-width fill declares one access per row.
        /// [`Self::finish`]'s access-for-access check then lines up
        /// automatically: this method pushes exactly the same N accesses
        /// into `self.accesses` that the decoder pushed into the journal, in
        /// the same order, so the count check and the element-wise identity
        /// check both pass with no per-command special-casing. There is no
        /// "collapse to one access" path and there must never be one -- a
        /// collapsed range would claim untouched inter-row bytes as written.
        ///
        /// The caller must hand over the *decoder's own* access slice rather
        /// than re-deriving it. Two independent derivations of the same
        /// access list is exactly the divergence `finish`'s check exists to
        /// catch, and would turn a sealed guarantee into a runtime coin flip.
        ///
        /// # Panics
        ///
        /// Panics if `accesses` is empty (an admitted fill that declares no
        /// write is a decoder bug, not a no-op to be admitted quietly), or
        /// if its length disagrees with `fill.access_count`.
        pub fn push_fill_rectangle(
            &mut self,
            fill: RdpFillRectangleCommand,
            accesses: &[ResourceAccess],
        ) {
            assert!(
                !accesses.is_empty(),
                "an admitted FillRectangle always declares at least one RenderTarget write access"
            );
            assert_eq!(
                accesses.len() as u32,
                fill.access_count,
                "FillRectangle command's declared access_count disagrees with the access slice \
                 being pushed for it"
            );
            self.accesses.extend_from_slice(accesses);
            self.commands
                .push(OwnedSemanticCommand::FillRectangle(fill));
        }

        /// Pushes one decoded `SYNC_FULL` site. Pushes zero
        /// [`ResourceAccess`] entries -- a sync journals no resource region.
        ///
        /// The `site.boundary` a caller supplies must be the one the decoder
        /// bound during decode (`RawDpcCommandKind::FullSync`'s own
        /// [`fn64_render_ir::FullSyncOccurrence`]), which the decoder in turn
        /// derived from this capture's own boundary list. Re-deriving it here
        /// would let plan state and capture state disagree about the same
        /// site.
        ///
        /// # Panics
        ///
        /// Panics if `site.dp_slot_reserved` is `false`. A site reaching the
        /// plan without its DP completion slot proved free means the reserve
        /// half was skipped, which is a caller bug: the whole point of
        /// `DeviceFabric::preflight_dp_full_sync` being nonmutating is that a
        /// backend can be rejected *before* it observes or changes anything,
        /// and a plan that records the site anyway would have discarded that
        /// rejection.
        ///
        /// # What this method deliberately does *not* check
        ///
        /// It does not validate `site.boundary.interrupt_after()`. Whether an
        /// `Asserted` value there is an honest observation or a fabrication
        /// depends on *how the producer obtained it*, which is not visible
        /// from a boundary value -- both cases are the same two bytes. That
        /// obligation therefore lives at the one place it is knowable, in
        /// [`crate::OwnedRawDpcCapture::with_full_sync_boundaries`]'s
        /// contract, and pretending to re-check it here would be theater.
        pub fn push_full_sync_site(&mut self, site: RdpFullSyncSite) {
            assert!(
                site.dp_slot_reserved,
                "a decoded FullSync site reached the plan without its DP completion slot \
                 reserved -- the nonmutating preflight_dp_full_sync reserve half was skipped"
            );
            self.commands.push(OwnedSemanticCommand::FullSyncSite(site));
        }

        /// Finish this writer into the sealed [`PlannedRawDpcSubmission`]. `journal`
        /// must be the exact journal the caller is about to hand to
        /// [`super::preflight_raw_dpc_capture`] for this same `capture` --
        /// this method builds that preflight itself (via `self.capture`, the
        /// same capture [`RawDpcBackendAuthority::begin_plan`] already bound
        /// to the paired queue) and derives both `source_identity` and
        /// `journal_identity` from that one preflight, so the plan and the
        /// preflight it is sealed with cannot describe two different
        /// captures or journals.
        ///
        /// Before building the plan, proves that every access this writer
        /// accumulated via `push_tmem_load`/`push_command_decode_access`
        /// equals `journal`'s own ordered access list one for one -- same
        /// count, same order, same `ResourceAccess` identity (operation,
        /// mode, purpose, region). A decoder that pushed a different access
        /// set than the journal it hands to preflight would otherwise let
        /// the sealed plan's `visit()` output silently diverge from the
        /// packet the journal actually admitted; this makes that divergence
        /// a loud `Err` here instead.
        pub fn finish(
            self,
            journal: fn64_render_ir::ResourceJournal,
        ) -> Result<PlannedRawDpcSubmission, ValidationError> {
            let journal_accesses = journal.accesses();
            if self.accesses.len() != journal_accesses.len() {
                return Err(ValidationError::EffectCountMismatch {
                    field: "raw-DPC plan writer accumulated access",
                    expected: journal_accesses.len(),
                    actual: self.accesses.len(),
                });
            }
            for (index, (pushed, journaled)) in
                self.accesses.iter().zip(journal_accesses).enumerate()
            {
                if pushed != journaled {
                    return Err(ValidationError::EffectAccessMismatch {
                        field: "raw-DPC plan writer accumulated access",
                        index,
                    });
                }
            }
            let submission = self.capture.submission();
            let source_identity = submission.identity();
            let journal_identity = journal.identity();
            let preflight = super::preflight_raw_dpc_capture(
                self.capture.memory_layout(),
                self.capture.transaction_sequence(),
                submission.clone(),
                self.capture.cmd_end(),
                // The capture's own boundary list, never a fresh `Vec::new()`.
                // Substituting an empty list here would make any capture whose
                // payload contains a `SYNC_FULL` opcode fail derivation with
                // `MissingFullSyncObservation` no matter what its producer
                // supplied -- which is exactly why FullSync could not be
                // planned before this field existed. Still empty, exactly, for
                // every capture built through `OwnedRawDpcCapture::new`.
                self.capture.full_sync_boundaries().to_vec(),
                journal,
            )?;
            let plan = ExactValidatedRawDpcPlan {
                source_identity,
                journal_identity,
                commands: self.commands.into_boxed_slice(),
                accesses: self.accesses.into_boxed_slice(),
            };
            Ok(PlannedRawDpcSubmission { preflight, plan })
        }
    }

    /// Sealed: no public fields, no public constructor, no getter that
    /// exposes the owned plan or preflight. The only route to one is
    /// [`ExactRawDpcPlanWriter::finish`]. The only route onward is
    /// [`RawDpcAbiSession::finalize_and_submit`], which consumes it and
    /// visits its plan's commands only through
    /// [`ExactValidatedRawDpcPlan::visit`] on the resulting [`BoundSubmittedRawDpc`].
    ///
    /// ```compile_fail
    /// use fn64_render::PlannedRawDpcSubmission;
    /// # fn planned() -> PlannedRawDpcSubmission { unimplemented!() }
    /// let planned = planned();
    /// // `plan` is a private field with no getter.
    /// let _ = planned.plan;
    /// ```
    ///
    /// ```compile_fail
    /// use fn64_render::PlannedRawDpcSubmission;
    /// # fn preflight() -> fn64_render::IrRawDpcPacketPreflight { unimplemented!() }
    /// # fn plan() -> fn64_render::ExactValidatedRawDpcPlan { unimplemented!() }
    /// // There is no public constructor: a foreign preflight/plan pair
    /// // cannot be forged into a `PlannedRawDpcSubmission`.
    /// let _ = PlannedRawDpcSubmission::new(preflight(), plan());
    /// ```
    #[derive(Debug)]
    pub struct PlannedRawDpcSubmission {
        preflight: IrRawDpcPacketPreflight,
        plan: ExactValidatedRawDpcPlan,
    }

    impl PlannedRawDpcSubmission {
        pub const fn guest_read_plan(&self) -> &DeferredGuestReadPlan {
            self.preflight.guest_read_plan()
        }
    }

    /// Sealed transport: publicly nameable only because it crosses the
    /// object-safe backend trait, but fields, constructors, destructuring,
    /// and ticket extraction are private to this module. Its only route
    /// onward is [`crate::RenderBackend::execute_raw_dpc`]'s implementation,
    /// via [`Self::into_backend_prepared`], which requires the exact paired
    /// [`RawDpcBackendAuthority`].
    ///
    /// ```compile_fail
    /// use fn64_render::BoundSubmittedRawDpc;
    /// # fn bound() -> BoundSubmittedRawDpc { unimplemented!() }
    /// let bound = bound();
    /// // `submitted` is a private field: no getter returns the bare ticket.
    /// let _ = bound.submitted;
    /// ```
    #[derive(Debug)]
    pub struct BoundSubmittedRawDpc {
        plan: ExactValidatedRawDpcPlan,
        submitted: fn64_render_ir::SubmittedTicket,
        submission_identity: SubmissionIdentity,
        queue: QueueIdentity,
        ordinal: u64,
        retirement: SubmittedRawDpcRetirement,
    }

    impl BoundSubmittedRawDpc {
        pub const fn queue(&self) -> QueueIdentity {
            self.queue
        }

        pub const fn ordinal(&self) -> u64 {
            self.ordinal
        }

        pub const fn submission(&self) -> SubmissionIdentity {
            self.submission_identity
        }

        /// Authority-scoped, nonextracting execution view. Validates the
        /// exact paired authority queue identity before lending anything (a
        /// mismatch loudly traps, `self` remains untouched and sealed); on
        /// success, calls `view`'s methods with the complete neutral plan
        /// (via [`ExactValidatedRawDpcPlan::visit`]), the finalized captured
        /// guest reads, and the submitted [`fn64_render_ir::WorkloadPacket`]
        /// -- everything T3 needs to compute a [`fn64_render_ir::BackendEffectReport`]
        /// -- without ever handing out the sealed [`fn64_render_ir::SubmittedTicket`]
        /// itself or any other ticket. Generic over `V`/`PV`, monomorphized
        /// per call site: no `dyn` dispatch here.
        pub fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
            &self,
            authority: &RawDpcBackendAuthority,
            plan_visitor: &mut PV,
            view: &mut V,
        ) {
            assert!(
                authority.authority.queue_identity() == self.queue,
                "RawDpcBackendAuthority is not paired with this submission's queue"
            );
            self.plan.visit(plan_visitor);
            view.plan_visited(plan_visitor);
            view.captured_reads(self.submitted.packet().guest_reads().reads());
            view.submitted_packet(self.submitted.packet());
        }

        /// Sole unseal route. Validates the exact paired authority queue
        /// identity before moving any field. A mismatch loudly traps; the
        /// still-sealed `self` then drops normally (recording exactly one
        /// `Rejected` and exposing no parts). Consumes the submitted ticket
        /// internally into a receipted [`GpuCompleteTicket`] using the exact
        /// paired authority's own `issue`, so the resulting
        /// [`GpuCompleteTicket`] can only ever be the one this exact
        /// submission produced -- there is no route for an independently
        /// supplied ticket to enter here. Carries no physical-state field or
        /// identity of any kind: the backend's own physical candidate lives
        /// entirely in [`RawDpcCoordinator`]'s double-buffered slots, and
        /// [`RawDpcCoordinator::complete_execution`] (the sole caller of this
        /// method) is what actually stores it there.
        pub fn into_backend_prepared(
            mut self,
            authority: &mut RawDpcBackendAuthority,
            effects: fn64_render_ir::BackendEffectReport,
        ) -> Result<BackendPreparedRawDpc, ValidationError> {
            assert!(
                authority.authority.queue_identity() == self.queue,
                "RawDpcBackendAuthority is not paired with this submission's queue"
            );
            let receipt = authority.authority.issue(&self.submitted, effects)?;
            let complete = self.submitted.gpu_complete(receipt)?;
            self.retirement
                .advance_stage(RawDpcRetirementStage::BackendReceipt);
            Ok(BackendPreparedRawDpc {
                plan: self.plan,
                complete,
                retirement: self.retirement,
            })
        }
    }

    /// Nonextracting, statically-dispatched callback set for
    /// [`BoundSubmittedRawDpc::execution_view`]. Every method receives only borrowed,
    /// neutral (`fn64-render-ir`/`fn64-render`) data -- never a ticket, never
    /// an owned field a caller could retain past the borrow. `PV` is the
    /// caller's own [`ExactRawDpcPlanVisitor`] implementation, threaded
    /// through so `plan_visited` can inspect what it already accumulated
    /// without `execution_view` needing to know its concrete type beyond the
    /// generic bound.
    pub trait RawDpcExecutionView<PV: ExactRawDpcPlanVisitor> {
        /// Called after the complete neutral plan has been lent to
        /// `plan_visitor` (see [`ExactValidatedRawDpcPlan::visit`]).
        fn plan_visited(&mut self, plan_visitor: &mut PV);

        /// The finalized, exact ordered captured guest reads this
        /// submission's plan required -- the same data
        /// [`RawDpcAbiSession::finalize_and_submit`] validated against the
        /// plan's own guest-read plan before a `BoundSubmittedRawDpc` could exist.
        fn captured_reads(&mut self, reads: &[fn64_render_ir::CapturedGuestRead]);

        /// The submitted [`fn64_render_ir::WorkloadPacket`] itself -- enough
        /// to construct a [`fn64_render_ir::BackendEffectReport`] against its
        /// own journal, without ever seeing the sealed
        /// [`fn64_render_ir::SubmittedTicket`] that owns it.
        fn submitted_packet(&mut self, packet: &fn64_render_ir::WorkloadPacket);
    }

    /// Public only to cross `fn64-render` -> `fn64-render-wgpu`. Move-only;
    /// fields, constructors, destructuring, and getters are private. Owns
    /// the receipted [`GpuCompleteTicket`] and armed retirement, ready for
    /// [`RawDpcAbiSession::commit_zero_guest_writes`]. Carries no physical-
    /// state field or identity: the backend's own physical candidate this
    /// submission prepared lives in [`RawDpcCoordinator`]'s double-buffered
    /// slots, keyed by the coordinator's own private ready-slot metadata,
    /// not by anything this type exposes.
    #[derive(Debug)]
    pub struct BackendPreparedRawDpc {
        plan: ExactValidatedRawDpcPlan,
        complete: GpuCompleteTicket,
        retirement: SubmittedRawDpcRetirement,
    }

    impl BackendPreparedRawDpc {
        pub const fn stage(&self) -> RawDpcRetirementStage {
            self.retirement.stage()
        }

        pub fn submission(&self) -> SubmissionIdentity {
            self.complete.submission()
        }
    }

    /// Public only to cross `fn64-render` -> `fn64-render-wgpu`/ABI. Move-only;
    /// fields, constructors, destructuring, and getters are private. Owns
    /// the plan, receipted [`GuestCommittedTicket`], and armed retirement
    /// together -- the sole route [`RawDpcAbiSession::commit_zero_guest_writes`]
    /// produces, and the sole input [`RawDpcAbiSession::seal_publication`]
    /// consumes. There is no bare `GuestCommittedTicket` getter: a caller
    /// can observe only `stage()`/`submission()`. Carries no physical-state
    /// field or identity, exactly like [`BackendPreparedRawDpc`].
    #[derive(Debug)]
    pub struct GuestCommittedRawDpc {
        plan: ExactValidatedRawDpcPlan,
        committed: GuestCommittedTicket,
        retirement: SubmittedRawDpcRetirement,
    }

    impl GuestCommittedRawDpc {
        pub const fn stage(&self) -> RawDpcRetirementStage {
            self.retirement.stage()
        }

        pub fn submission(&self) -> SubmissionIdentity {
            self.committed.submission()
        }
    }

    /// Move-only, `#[must_use]` terminal capsule: the sole publishable value
    /// [`RawDpcAbiSession::seal_publication`] produces. Its sole consumer is
    /// [`RawDpcCoordinator::prepare_publication`] -- there is no public
    /// terminal method on this type itself; publication requires the
    /// backend's own coordinator, not just this capsule. Lifetime-bound to
    /// the backend-retained [`fn64_runtime::device::ReadyDpcFabricCommit`] it
    /// wraps, exactly like that inner type. Fields are private; nothing
    /// outside this module can extract the plan, ticket, or fabric commit
    /// short of the public, narrow accessors below. There is no
    /// terminal-observation handle accessor either: an earlier draft
    /// exposed a public `retirement_handle()` returning a clone of the
    /// diagnostic [`RawDpcRetirementHandle`], but that let any caller reach
    /// the same observation surface a coordinator needs privately for
    /// abandoned-candidate reaping, without going through
    /// [`RawDpcCoordinator::prepare_publication`]'s own checks. The
    /// coordinator now keeps its own private `Rc::clone` of the same
    /// retirement slot (see [`ReadyPhysicalSlot`]) instead.
    ///
    /// **Drop is cancellation, at both layers.** An unconsumed capsule's
    /// `Drop` runs the inner [`fn64_runtime::device::ReadyDpcFabricCommit`]'s
    /// own armed-gated `Drop` (rolling back the DPC registers it prepared)
    /// and this capsule's own [`SubmittedRawDpcRetirement`]'s `Drop`
    /// (recording `Rejected` at `FabricPrepare`, the stage
    /// [`RawDpcAbiSession::seal_publication`] leaves it at -- not
    /// `PhysicalPrepare`, which only [`ReadyPublication::commit`] reaches,
    /// and only on success) -- exactly the same "unconsumed means cancelled"
    /// contract every earlier typestate in this chain already holds, with no
    /// capsule-specific `Drop` impl needed. The same holds for a
    /// [`ReadyPublication`] wrapping this capsule that is itself dropped
    /// before `commit`: it borrows, rather than owns, its coordinator, so
    /// dropping it runs no code of its own -- the capsule's own `Drop`
    /// (described above) is what actually cancels.
    #[must_use = "an unconsumed ReadyRawDpcCommitCapsule cancels its DPC commit on drop"]
    #[derive(Debug)]
    pub struct ReadyRawDpcCommitCapsule<'fabric> {
        plan: ExactValidatedRawDpcPlan,
        committed: GuestCommittedTicket,
        fabric: fn64_runtime::device::ReadyDpcFabricCommit<'fabric>,
        retirement: SubmittedRawDpcRetirement,
    }

    impl<'fabric> ReadyRawDpcCommitCapsule<'fabric> {
        /// Lend this capsule's finalized captured reads, submitted packet,
        /// and complete neutral plan to a paired, statically-dispatched
        /// [`RawDpcExecutionView`] -- the same nonextracting shape
        /// [`BoundSubmittedRawDpc::execution_view`] already provides, at the capsule's
        /// own terminal stage. Traps if `authority` is not paired with this
        /// session's queue, exactly like every other production entry point.
        pub fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
            &self,
            authority: &RawDpcBackendAuthority,
            plan_visitor: &mut PV,
            view: &mut V,
        ) {
            assert!(
                authority.authority.queue_identity() == self.committed.queue(),
                "RawDpcBackendAuthority is not paired with this submission's queue"
            );
            self.plan.visit(plan_visitor);
            view.plan_visited(plan_visitor);
            view.captured_reads(self.committed.packet().guest_reads().reads());
            view.submitted_packet(self.committed.packet());
        }

        pub const fn stage(&self) -> RawDpcRetirementStage {
            self.retirement.stage()
        }

        pub fn submission(&self) -> SubmissionIdentity {
            self.committed.submission()
        }

        pub const fn queue(&self) -> QueueIdentity {
            self.committed.queue()
        }
    }

    /// Semantic terminal evidence for one published raw-DPC submission: it
    /// carries only the identity, never plan/ticket/fabric/physical-state
    /// fields. Produced solely by [`ReadyPublication::commit`]; the type
    /// stays frozen so a caller's own success-path bookkeeping never needs to
    /// widen past one [`SubmissionIdentity`].
    ///
    /// **Live-XBUS-equality is not part of publication readiness.** A public
    /// STATUS-register mode command may legitimately change XBUS source
    /// selection while this submission's DPC transaction is already admitted
    /// and in flight; `fn64-runtime`'s `commit_dpc_submission` deliberately
    /// ignores/preserves that mode bit and clears only the admission-owned
    /// END_VALID/DMA_BUSY/CMD_BUSY status, retaining the pending
    /// submission's originally *captured* source rather than re-reading live
    /// XBUS state. T0's neutral plan is bound to that same captured source
    /// ([`ExactValidatedRawDpcPlan::source_identity`],
    /// [`RawDpcCommandLocation`]'s stream/chunk offsets) for exactly this
    /// reason: [`RawDpcCoordinator::prepare_publication`], which validates
    /// authority/queue/submission/ready-slot identity, compares only against
    /// this plan's captured source and its own private ready-slot metadata --
    /// never against whatever XBUS is live at publish time. A capsule design
    /// that added a live-XBUS-equality gate would be wrong, not merely
    /// stricter -- it would reject an otherwise-valid, already-admitted
    /// submission whose source register a later, unrelated STATUS write
    /// legitimately changed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct CommittedRawDpcOutcome {
        submission: SubmissionIdentity,
    }

    impl CommittedRawDpcOutcome {
        pub const fn submission(self) -> SubmissionIdentity {
            self.submission
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fn64_render_ir::{
            AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, DpInterruptState,
            OperationId, PhysicalMemoryLayout, RdramResource, ResourceJournal,
            ResourceJournalLimits, ResourceRegion, TemporalBoundary, TmemRange as IrTmemRange,
        };
        use std::cell::RefCell;

        use crate::{OwnedRawDpcCapture, OwnedRawDpcSubmission, RawDpcSource};

        /// Build one TMEM-load-shaped, no-FullSync, zero-guest-write plan
        /// request/writer pair through the sealed session/authority roles --
        /// the exact class of packet T0's frozen scope admits (card v10
        /// section 1).
        fn planned_fixture(
            session: &RawDpcAbiSession,
            authority: &RawDpcBackendAuthority,
            dram: bool,
        ) -> (PlannedRawDpcSubmission, ResourceAccess) {
            let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let source_range = layout.range(0x200, 0x210).unwrap();

            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                if dram {
                    ResourceRegion::Rdram {
                        resource: RdramResource::RawCommands,
                        range: command_range,
                    }
                } else {
                    ResourceRegion::RspDmem(
                        fn64_render_ir::DmemRange::try_new(
                            command_range.start().get(),
                            command_range.end(),
                        )
                        .unwrap(),
                    )
                },
            )
            .unwrap();
            let tmem_source = ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: source_range,
                },
            )
            .unwrap();
            let tmem_destination = ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::TmemLoadDestination,
                ResourceRegion::Tmem(IrTmemRange::try_new(0, 16).unwrap()),
            )
            .unwrap();
            let journal = ResourceJournal::try_new(
                ResourceJournalLimits::try_new(4, 0x100).unwrap(),
                vec![command_read, tmem_source, tmem_destination],
            )
            .unwrap();

            let submission = if dram {
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0]).unwrap()
            } else {
                OwnedRawDpcSubmission::from_xbus_payload(
                    0x100,
                    0x108,
                    [0xf500_0000u32, 0]
                        .into_iter()
                        .flat_map(u32::to_be_bytes)
                        .collect(),
                )
                .unwrap()
            };
            let capture = OwnedRawDpcCapture::new(
                submission,
                layout,
                7,
                TemporalBoundary::new(11, DpInterruptState::Clear),
            );

            let request = session.plan_request(capture);
            let mut writer = authority.begin_plan(request);
            writer.push_command_decode_access(command_read);
            let location = RawDpcCommandLocation {
                command_index: 0,
                stream_index: 0,
                chunk_index: 0,
                source_address: layout.address(command_range.start().get()).unwrap(),
                source_byte_offset: 0,
                source_byte_len: 8,
                wire_opcode: 0xf5,
            };
            let tile_descriptor = NeutralTileDescriptor {
                format: NeutralImageFormat::Rgba,
                size: NeutralPixelSize::Bits16,
                line_words: 2,
                tmem_word_address: 0,
                palette: 0,
                s_mode: NeutralTileAddressMode::default(),
                mask_s: 0,
                shift_s: 0,
                t_mode: NeutralTileAddressMode::default(),
                mask_t: 0,
                shift_t: 0,
            };
            let source_image = NeutralTextureImage {
                format: NeutralImageFormat::Rgba,
                size: NeutralPixelSize::Bits16,
                width: 4,
                address: layout.address(source_range.start().get()).unwrap(),
            };
            let transfer_words = vec![NeutralTmemTransferWord {
                index: 0,
                logical_source_offset: 0,
                source_access_index: 1,
                source_access_byte_offset: 0,
                defined_source_byte_mask: 0xff,
                defined_destination_byte_mask: 0xff,
                destination_word: 0,
                row_advance: 0,
                odd_row_exchange: false,
                physical: NeutralTmemTransferPhysicalWord::Linear(
                    IrTmemRange::try_new(0, 8).unwrap(),
                ),
            }];
            let load = TmemLoadSemantics::new(
                location,
                vec![0xf500_0000, 0],
                TmemLoadEpoch::new(core::num::NonZeroU64::new(1).unwrap()),
                TmemLoadKind::Tile {
                    bounds: NeutralTileSize {
                        low_s: 0,
                        low_t: 0,
                        high_s: 16,
                        high_t: 16,
                    },
                },
                0,
                source_image,
                tile_descriptor,
                vec![tmem_source],
                1,
                tmem_destination,
                2,
                16,
                0,
                1,
                1,
                TmemTransferLayout::Linear,
                transfer_words,
            );
            writer.push_tmem_load(load);
            let planned = writer.finish(journal).unwrap();

            (planned, tmem_destination)
        }

        /// Build one triangle-only, genuinely zero-write plan: a single
        /// command-decode read access (so the journal is non-empty --
        /// [`fn64_render_ir::ResourceJournal::try_new`] rejects an empty
        /// access list outright) plus one [`RdpTriangleCommand`], which
        /// itself pushes zero [`ResourceAccess`] entries (see that type's
        /// own doc comment). Unlike [`planned_fixture`], no TMEM
        /// source/destination access exists anywhere in this plan's journal
        /// -- this is the realistic shape
        /// `complete_execution_preserving_physical` exists for.
        fn triangle_planned_fixture(
            session: &RawDpcAbiSession,
            authority: &RawDpcBackendAuthority,
        ) -> PlannedRawDpcSubmission {
            let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
            // 0xc8 (TRI_FILL, `command & 0x3f == 0x08`) is a 32-byte (8-word)
            // command per `raw_rdp_command_width` -- the stream must supply
            // the full width or the preflight's own truncation scan rejects
            // it before this fixture's `writer.finish` is ever reached.
            let command_range = layout.range(0x100, 0x120).unwrap();

            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let journal = ResourceJournal::try_new(
                ResourceJournalLimits::try_new(4, 0x100).unwrap(),
                vec![command_read],
            )
            .unwrap();

            let words = vec![0xc800_0000, 0, 0, 0, 0, 0, 0, 0];
            let submission =
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x120, words.clone()).unwrap();
            let capture = OwnedRawDpcCapture::new(
                submission,
                layout,
                7,
                TemporalBoundary::new(11, DpInterruptState::Clear),
            );

            let request = session.plan_request(capture);
            let mut writer = authority.begin_plan(request);
            writer.push_command_decode_access(command_read);
            let location = RawDpcCommandLocation {
                command_index: 0,
                stream_index: 0,
                chunk_index: 0,
                source_address: layout.address(command_range.start().get()).unwrap(),
                source_byte_offset: 0,
                source_byte_len: 32,
                wire_opcode: 0xc8,
            };
            writer.push_triangle(RdpTriangleCommand {
                location,
                raw_words: words.into_boxed_slice(),
                source: TriangleSource::RawTriangle,
                viewport: None,
                texrect_accesses: None,
                vertices: [NeutralTriangleVertex {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    texcoord: [0.0, 0.0],
                }; 3],
            });
            writer.finish(journal).unwrap()
        }

        fn matching_guest_read_capture(
            planned: &PlannedRawDpcSubmission,
        ) -> DeferredGuestReadCapture {
            DeferredGuestReadCapture::new(
                planned
                    .guest_read_plan()
                    .reads()
                    .iter()
                    .map(|read| {
                        fn64_render_ir::CapturedGuestRead::try_new(
                            *read,
                            vec![0xab; read.range().len() as usize],
                        )
                        .unwrap()
                    })
                    .collect(),
            )
        }

        #[derive(Default)]
        struct RecordingVisitor {
            commands: usize,
            accesses: usize,
        }

        impl ExactRawDpcPlanVisitor for RecordingVisitor {
            fn command(&mut self, _command: RawDpcSemanticCommandRef<'_>) {
                self.commands += 1;
            }

            fn access(&mut self, _access: ResourceAccess) {
                self.accesses += 1;
            }
        }

        #[test]
        fn plan_request_is_stamped_with_the_session_queue_and_paired_authority_accepts_it() {
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
                PhysicalMemoryLayout::try_new(0x100).unwrap(),
                0,
                TemporalBoundary::new(0, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let request_queue = request.queue();
            assert_eq!(request_queue, session.queue.identity());
            // Paired authority accepts it without panicking.
            let _writer = authority.begin_plan(request);
        }

        #[test]
        #[should_panic(expected = "not paired")]
        fn unrelated_authority_traps_before_any_plan_field_can_be_written() {
            let (session, _own_authority) = new_raw_dpc_roles().unwrap();
            let (_foreign_session, foreign_authority) = new_raw_dpc_roles().unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
                PhysicalMemoryLayout::try_new(0x100).unwrap(),
                0,
                TemporalBoundary::new(0, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let _writer = foreign_authority.begin_plan(request);
        }

        #[test]
        fn begin_plan_consumes_the_request_so_it_cannot_mint_a_second_writer() {
            // Compile-shape proof, not a runtime assertion: `begin_plan` taking
            // `RawDpcPlanRequest` by value means a second call using the same
            // `request` binding is a move-after-move compile error, not a
            // runtime possibility to test. The `#[test]` here exists so this
            // guarantee has a named, discoverable anchor in the suite; see the
            // `compile_fail` doctest on `RawDpcPlanRequest` for the actual
            // enforcement proof.
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
                PhysicalMemoryLayout::try_new(0x100).unwrap(),
                0,
                TemporalBoundary::new(0, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let _writer = authority.begin_plan(request);
        }

        #[test]
        fn writer_finish_produces_a_plan_with_every_pushed_command_and_access() {
            // Item 6: `PlannedRawDpcSubmission`/`BoundSubmittedRawDpc`/`BackendPreparedRawDpc`
            // expose no `visit_plan` -- ABI must not get a plan-extraction
            // surface. This test proves the sealed plan's *content* is
            // correct using same-module access to the private `plan` field
            // (the same access level `production::`'s own code has, not a
            // capability ABI or `fn64-render-wgpu` can reach), rather than a
            // public visitor call.
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&session, &authority, true);
            let mut visitor = RecordingVisitor::default();
            planned.plan.visit(&mut visitor);
            assert_eq!(visitor.commands, 1);
            // command_decode + tmem_source + tmem_destination
            assert_eq!(visitor.accesses, 3);
        }

        /// Item 5 audit: `finish`'s call to `super::preflight_raw_dpc_capture`
        /// passes `Vec::new()` only for `full_sync_boundaries` (legitimately
        /// empty in this no-FullSync frozen slice), never for the capture's
        /// own stream/source binding. Proves the resulting preflight's guest
        /// read plan carries the capture's exact memory layout and that the
        /// plan's own source identity is the exact submitted command bytes'
        /// identity -- neither is silently dropped or replaced with a
        /// default. This is also the property the XBUS-mode-change
        /// correction on [`CommittedRawDpcOutcome`] depends on: the plan's
        /// source identity is the *captured* submission's, not something
        /// this test (or the real seam) ever recomputes from live device
        /// state.
        #[test]
        fn finish_preserves_the_capture_memory_layout_and_source_identity_not_a_default() {
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let layout = PhysicalMemoryLayout::try_new(0x2000).unwrap();
            let submission =
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0])
                    .unwrap();
            let expected_source_identity = submission.identity();
            let capture = OwnedRawDpcCapture::new(
                submission,
                layout,
                42,
                TemporalBoundary::new(99, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let mut writer = authority.begin_plan(request);
            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: layout.range(0x100, 0x108).unwrap(),
                },
            )
            .unwrap();
            writer.push_command_decode_access(command_read);
            let journal = journal_of(vec![command_read]);
            let planned = writer.finish(journal).unwrap();

            // The preflight's guest read plan carries the capture's exact
            // memory layout -- not `PhysicalMemoryLayout::default()` or an
            // unrelated value a discarded binding would produce.
            assert_eq!(planned.guest_read_plan().memory_layout(), layout);
            // The plan's source identity is the submission's own content
            // hash, not a placeholder -- proving the writer's `finish` bound
            // the plan to the exact bytes/range it was given, not a
            // structurally-similar but distinct capture.
            assert_eq!(planned.plan.source_identity(), expected_source_identity);
        }

        /// Fixture for item 2's hostile access-list tests: one writer that
        /// pushed exactly `command_read` and one `push_command_decode_access`
        /// call (no TMEM load), plus the matching baseline journal. Each
        /// hostile test mutates only the journal passed to `finish`, proving
        /// `finish` rejects every one-field departure from what the writer
        /// actually accumulated.
        fn hostile_access_fixture(
            session: &RawDpcAbiSession,
            authority: &RawDpcBackendAuthority,
        ) -> (ExactRawDpcPlanWriter, ResourceAccess, ResourceAccess) {
            let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let other_range = layout.range(0x300, 0x308).unwrap();
            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let other_access = ResourceAccess::try_new(
                OperationId::new(9),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: other_range,
                },
            )
            .unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0])
                    .unwrap(),
                layout,
                7,
                TemporalBoundary::new(11, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let mut writer = authority.begin_plan(request);
            writer.push_command_decode_access(command_read);
            (writer, command_read, other_access)
        }

        fn journal_of(accesses: Vec<ResourceAccess>) -> ResourceJournal {
            ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x100).unwrap(), accesses)
                .unwrap()
        }

        #[test]
        fn finish_rejects_a_journal_missing_an_access_the_writer_pushed() {
            // `ResourceJournal` itself refuses an empty access list, so the
            // reachable "missing" case through `finish` is a nonempty journal
            // with fewer entries than the writer actually pushed. The
            // writer's single pushed access is replaced (not just dropped)
            // to keep the journal nonempty, so this exercises the
            // access-mismatch branch rather than journal construction.
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let (writer, _command_read, other_access) =
                hostile_access_fixture(&session, &authority);
            let short_journal = journal_of(vec![other_access]);
            assert_eq!(
                writer.finish(short_journal).unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "raw-DPC plan writer accumulated access",
                    index: 0,
                }
            );
        }

        #[test]
        fn finish_rejects_a_journal_with_an_extra_access_the_writer_never_pushed() {
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let (writer, command_read, other_access) = hostile_access_fixture(&session, &authority);
            let extra_journal = journal_of(vec![command_read, other_access]);
            assert_eq!(
                writer.finish(extra_journal).unwrap_err(),
                ValidationError::EffectCountMismatch {
                    field: "raw-DPC plan writer accumulated access",
                    expected: 2,
                    actual: 1,
                }
            );
        }

        #[test]
        fn finish_rejects_a_reordered_journal_even_with_the_same_access_set() {
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let second_range = layout.range(0x300, 0x308).unwrap();
            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let second_access = ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: second_range,
                },
            )
            .unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0])
                    .unwrap(),
                layout,
                7,
                TemporalBoundary::new(11, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            let mut writer = authority.begin_plan(request);
            // Writer pushes command_read then second_access, in that order.
            writer.push_command_decode_access(command_read);
            writer.push_command_decode_access(second_access);
            // Journal has the identical set, reversed.
            let reordered_journal = journal_of(vec![second_access, command_read]);
            assert_eq!(
                writer.finish(reordered_journal).unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "raw-DPC plan writer accumulated access",
                    index: 0,
                }
            );
        }

        #[test]
        fn finish_rejects_a_journal_whose_matching_index_access_was_mutated() {
            let (session, authority) = new_raw_dpc_roles().unwrap();
            let (writer, command_read, _other) = hostile_access_fixture(&session, &authority);
            // Same operation/mode/purpose, but a different region: a mutated
            // access at the same index, same count.
            let mutated = ResourceAccess::try_new(
                command_read.operation(),
                command_read.mode(),
                command_read.purpose(),
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: PhysicalMemoryLayout::try_new(0x1000)
                        .unwrap()
                        .range(0x400, 0x408)
                        .unwrap(),
                },
            )
            .unwrap();
            let mutated_journal = journal_of(vec![mutated]);
            assert_eq!(
                writer.finish(mutated_journal).unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "raw-DPC plan writer accumulated access",
                    index: 0,
                }
            );
        }

        #[test]
        fn finalize_and_submit_never_exposes_a_bare_ticket_and_records_a_ledger_handle() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            assert_eq!(bound.ordinal(), 0);
            assert_eq!(session.ledger.handles.len(), 1);
            let handle = &session.ledger.handles[0];
            assert_eq!(handle.submission(), bound.submission());
            assert!(handle.outcome().is_none());
        }

        #[test]
        fn bound_dropped_before_backend_prepare_records_exactly_one_rejected() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            drop(bound);
            let handle = &session.ledger.handles[0];
            assert!(matches!(
                handle.outcome(),
                Some(RawDpcTerminalOutcome::Rejected {
                    stage: RawDpcRetirementStage::Execute,
                    ..
                })
            ));
        }

        #[test]
        fn into_backend_prepared_with_unrelated_authority_traps_before_moving_or_exposing_parts() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            let (_, mut foreign_authority) = new_raw_dpc_roles().unwrap();

            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0xab; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                bound.into_backend_prepared(&mut foreign_authority, effects)
            }));
            assert!(
                outcome.is_err(),
                "unrelated authority must trap, not silently succeed"
            );
            let handle = &session.ledger.handles[0];
            assert!(matches!(
                handle.outcome(),
                Some(RawDpcTerminalOutcome::Rejected { .. })
            ));
        }

        #[derive(Default)]
        struct RecordingExecutionView {
            plan_commands_seen: usize,
            reads_seen: usize,
            packet_seen: bool,
        }

        impl RawDpcExecutionView<RecordingVisitor> for RecordingExecutionView {
            fn plan_visited(&mut self, plan_visitor: &mut RecordingVisitor) {
                self.plan_commands_seen = plan_visitor.commands;
            }

            fn captured_reads(&mut self, reads: &[fn64_render_ir::CapturedGuestRead]) {
                self.reads_seen = reads.len();
            }

            fn submitted_packet(&mut self, _packet: &fn64_render_ir::WorkloadPacket) {
                self.packet_seen = true;
            }
        }

        #[test]
        fn execution_view_lends_plan_reads_and_packet_under_the_paired_authority() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let mut plan_visitor = RecordingVisitor::default();
            let mut view = RecordingExecutionView::default();
            bound.execution_view(&authority, &mut plan_visitor, &mut view);

            assert_eq!(view.plan_commands_seen, 1);
            assert_eq!(
                view.reads_seen, 1,
                "the fixture's one TmemLoadSource access"
            );
            assert!(view.packet_seen);
        }

        #[test]
        fn execution_view_with_unrelated_authority_traps_before_lending_anything() {
            let (mut session, _own_authority) = new_raw_dpc_roles().unwrap();
            let (_foreign_session, foreign_authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&session, &_own_authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let mut plan_visitor = RecordingVisitor::default();
            let mut view = RecordingExecutionView::default();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                bound.execution_view(&foreign_authority, &mut plan_visitor, &mut view);
            }));
            assert!(
                outcome.is_err(),
                "unrelated authority must trap before lending plan/reads/packet"
            );
            assert_eq!(view.plan_commands_seen, 0);
            assert_eq!(view.reads_seen, 0);
            assert!(!view.packet_seen);
        }

        #[test]
        fn matching_authority_prepares_backend_and_advances_stage() {
            let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0xab; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();

            let prepared = bound
                .into_backend_prepared(&mut authority, effects)
                .unwrap();
            assert_eq!(prepared.stage(), RawDpcRetirementStage::BackendReceipt);
            assert!(session.ledger.handles[0].outcome().is_none());
        }

        /// B2 defense-in-depth proof: `commit_zero_guest_writes` rejects a
        /// packet whose journal declares a real guest-visible write, even
        /// though the normal `ExactRawDpcPlanWriter` path can never build one
        /// (that gate already runs earlier). This constructs a
        /// `GpuCompleteTicket` directly against a hand-built journal with an
        /// `Rdram` write access -- bypassing the plan-writer entirely -- to
        /// prove the re-check inside `commit_zero_guest_writes` itself
        /// (via `GuestCommitEffectReport::try_new`) is what rejects it, not
        /// just the earlier, otherwise-unreachable-in-this-test gate.
        #[test]
        fn commit_zero_guest_writes_rejects_a_packet_with_a_real_guest_write_even_via_a_hand_built_ticket(
        ) {
            let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let guest_write_range = layout.range(0x200, 0x210).unwrap();
            let command_read = ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let guest_write = ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range: guest_write_range,
                },
            )
            .unwrap();
            let journal = journal_of(vec![command_read, guest_write]);
            let stream = fn64_render_ir::RawCommandStream::Dram(
                fn64_render_ir::DramCommandStream::try_new(vec![
                    fn64_render_ir::DramCommandChunk::try_new(
                        command_range,
                        vec![0xf500_0000, 0],
                        TemporalBoundary::new(11, DpInterruptState::Clear),
                        Vec::new(),
                    )
                    .unwrap(),
                ])
                .unwrap(),
            );
            let packet = fn64_render_ir::WorkloadPacket::try_new(
                layout,
                fn64_render_ir::WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap();

            let (mut queue, mut backend_authority, _guest) =
                fn64_render_ir::TicketAuthoritySet::try_new()
                    .unwrap()
                    .into_roles();
            let submitted = queue
                .submit(fn64_render_ir::DecodedTicket::new(packet))
                .unwrap();
            let write_effect = CompletedWrite::try_new(
                guest_write,
                guest_write_range.len(),
                fn64_render_ir::effect_content_digest(&[0xcd; 16]),
            )
            .unwrap();
            let report =
                BackendEffectReport::try_new(submitted.packet(), vec![write_effect]).unwrap();
            let receipt = backend_authority.issue(&submitted, report).unwrap();
            let complete = submitted.gpu_complete(receipt).unwrap();

            // Hand-build a `BackendPreparedRawDpc` with a fabricated plan and
            // retirement, since this ticket never went through
            // `ExactRawDpcPlanWriter`. Same-module private-field
            // construction is the only way to reach this state at all --
            // proving the production API itself cannot build one.
            let (dummy_retirement, _handle) =
                SubmittedRawDpcRetirement::new_pair(complete.submission());
            let dummy_plan = ExactValidatedRawDpcPlan {
                source_identity: crate::RawDpcSubmissionIdentity {
                    source: RawDpcSource::Rdram,
                    start: 0x100,
                    end: 0x108,
                    command_sha256: [0; 32],
                },
                journal_identity: complete.packet().journal().identity(),
                commands: Vec::new().into_boxed_slice(),
                accesses: Vec::new().into_boxed_slice(),
            };
            let prepared = BackendPreparedRawDpc {
                plan: dummy_plan,
                complete,
                retirement: dummy_retirement,
            };

            // A session sharing the exact same queue the ticket was issued
            // from, so the queue-pairing assertion inside
            // `commit_zero_guest_writes` passes and only the guest-write
            // re-check can reject this call.
            let mut matching_session = RawDpcAbiSession {
                queue,
                guest: fn64_render_ir::TicketAuthoritySet::try_new()
                    .unwrap()
                    .into_roles()
                    .2,
                ledger: RetirementLedger::default(),
            };
            let outcome = matching_session.commit_zero_guest_writes(prepared);
            assert_eq!(
                outcome.unwrap_err(),
                ValidationError::EffectCountMismatch {
                    field: "guest commit access",
                    expected: 1,
                    actual: 0,
                }
            );
        }

        /// P3 proof: the retirement's shared slot (`Rc<RetirementSlot>`) is
        /// the exact same allocation from `finalize_and_submit`'s handle
        /// through the fully committed `GuestCommittedRawDpc`'s own
        /// `retirement` field -- not a fresh slot minted at any hop.
        /// Same-module private-field access reaches both ends of the chain.
        #[test]
        fn retirement_slot_identity_is_the_same_allocation_across_every_hop() {
            let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0x66; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let prepared = bound
                .into_backend_prepared(&mut authority, effects)
                .unwrap();
            let ledger_slot = session.ledger.handles[0].slot.clone();
            let committed = session.commit_zero_guest_writes(prepared).unwrap();

            assert!(
                Rc::ptr_eq(&ledger_slot, &committed.retirement.slot),
                "the fully committed value's retirement must share the exact slot \
                 the original finalize_and_submit handle points at, not a fresh one"
            );
        }

        /// One real, admitted `fn64_runtime::device::DeviceFabric` with a
        /// fresh DPC submission pending -- built from the same public
        /// fixture recipe `fn64-runtime`'s own `ReadyDpcFabricCommit` tests
        /// use (`InMemoryRom`/`FixedPiTiming`/`request_dpc_submission`), so
        /// this module's capsule tests seal against the concrete T2 type
        /// itself, never a stand-in.
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

        /// Fake, minimal backend physical state (`P` in
        /// [`RawDpcCoordinator<P>`]) for T0's own tests -- T3 supplies the
        /// real `wgpu` type; T0 only needs to prove the generic mechanism.
        /// Ordinary owned data, no `Drop` impl of its own (so this fixture's
        /// tests can freely assert on slot contents without a destructor
        /// side effect muddying the picture).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct FakePhysical(u32);

        /// Build one sealed [`GuestCommittedRawDpc`] plus the
        /// [`RawDpcCoordinator<FakePhysical>`] it was prepared through, via
        /// the full session/coordinator chain: `finalize_and_submit` ->
        /// `coordinator.complete_execution` (which stores `next_physical`
        /// into the coordinator's inactive slot) -> `commit_zero_guest_writes`.
        fn guest_committed_fixture(
            session: &mut RawDpcAbiSession,
            coordinator: &mut RawDpcCoordinator<FakePhysical>,
            next_physical: FakePhysical,
        ) -> GuestCommittedRawDpc {
            let (planned, destination) = planned_fixture(session, &coordinator.authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0x99; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let prepared = coordinator
                .complete_execution(bound, effects, next_physical)
                .unwrap();
            session.commit_zero_guest_writes(prepared).unwrap()
        }

        #[test]
        fn seal_publication_advances_to_fabric_prepare() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(1));
            let submission = committed.submission();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();

            let capsule = session.seal_publication(committed, ready).unwrap();
            assert_eq!(capsule.stage(), RawDpcRetirementStage::FabricPrepare);
            assert_eq!(capsule.submission(), submission);
            assert!(session.ledger.handles[0].outcome().is_none());
            // Preparing execution must not itself publish: the coordinator's
            // active slot is still the seed value until `commit()`.
            assert_eq!(*coordinator.physical(), FakePhysical(0));
        }

        #[test]
        fn prepare_publication_commit_flips_active_slot_commits_fabric_and_publishes() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(7));
            let submission = committed.submission();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            let publication = coordinator.prepare_publication(capsule);
            assert_eq!(
                publication.capsule.stage(),
                RawDpcRetirementStage::PhysicalPrepare,
                "prepare_publication must advance to PhysicalPrepare before returning, \
                 so that stage is observable even if commit is never called"
            );
            let outcome = publication.commit();
            assert_eq!(outcome.submission(), submission);
            assert_eq!(
                *coordinator.physical(),
                FakePhysical(7),
                "commit must flip the coordinator's active slot to the prepared candidate"
            );
            assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
            assert_eq!(
                session.ledger.handles[0].outcome(),
                Some(RawDpcTerminalOutcome::Published)
            );
        }

        /// Physical state that records, via a shared counter, exactly when
        /// its `Drop` runs -- so this test can prove *where* an old
        /// inactive-slot `P` is actually dropped, not just that it
        /// eventually is.
        struct DroppableFakePhysical {
            id: u32,
            drops: Rc<RefCell<Vec<u32>>>,
        }

        impl Drop for DroppableFakePhysical {
            fn drop(&mut self) {
                self.drops.borrow_mut().push(self.id);
            }
        }

        /// P5 proof: an old inactive-slot `P` is dropped entirely inside
        /// `complete_execution` -- a fallible, ordinary step that runs
        /// *before* any `ReadyPublication` exists -- never inside
        /// `ReadyPublication::commit`'s straight-line body. This is the
        /// concrete reason `RawDpcCoordinator` double-buffers instead of
        /// `mem::replace`-ing the *active* slot: replacing active in place
        /// would run the old active `P`'s `Drop` at the exact moment a new
        /// candidate becomes current, inside what must otherwise be a
        /// Drop-free commit.
        #[test]
        fn old_inactive_slot_physical_state_drops_during_complete_execution_not_during_commit() {
            let drops = Rc::new(RefCell::new(Vec::new()));
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(DroppableFakePhysical {
                id: 0,
                drops: Rc::clone(&drops),
            });

            // First submission: `complete_execution` stores id=1 into the
            // (empty) inactive slot. Nothing has dropped yet -- the slot was
            // `None`.
            let (planned, destination) = planned_fixture(&session, &coordinator.authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0x11; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let _first_prepared = coordinator
                .complete_execution(
                    bound,
                    effects,
                    DroppableFakePhysical {
                        id: 1,
                        drops: Rc::clone(&drops),
                    },
                )
                .unwrap();
            assert!(
                drops.borrow().is_empty(),
                "storing into a previously-empty inactive slot drops nothing"
            );

            // Second submission, same never-flipped coordinator:
            // `complete_execution` overwrites that same inactive slot again,
            // dropping id=1 right here -- before any capsule/ReadyPublication
            // for *this* ordinal exists, and long before either ordinal's
            // eventual `commit`.
            let (planned, destination) = planned_fixture(&session, &coordinator.authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0x22; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let _second_prepared = coordinator
                .complete_execution(
                    bound,
                    effects,
                    DroppableFakePhysical {
                        id: 2,
                        drops: Rc::clone(&drops),
                    },
                )
                .unwrap();
            assert_eq!(
                *drops.borrow(),
                vec![1],
                "the superseded id=1 candidate must drop exactly here, inside \
                 complete_execution -- not deferred to a later commit"
            );
        }

        /// Build one bound submission ready to hand to
        /// `complete_execution_preserving_physical`, without going through
        /// `guest_committed_fixture` (which always calls the ordinary
        /// `complete_execution` and requires a `next_physical`).
        /// A genuinely zero-write, triangle-only bound submission -- the
        /// exact class of packet [`RawDpcCoordinator::
        /// complete_execution_preserving_physical`] exists for. Built from
        /// [`triangle_planned_fixture`], never [`planned_fixture`] (whose
        /// plan always carries a real TMEM destination write access).
        fn bound_submission_fixture(
            session: &mut RawDpcAbiSession,
            authority: &RawDpcBackendAuthority,
        ) -> BoundSubmittedRawDpc {
            let planned = triangle_planned_fixture(session, authority);
            let capture = matching_guest_read_capture(&planned);
            session.finalize_and_submit(planned, capture).unwrap()
        }

        #[test]
        fn preserving_completion_never_clones_touches_or_replaces_either_physical_slot() {
            let drops = Rc::new(RefCell::new(Vec::new()));
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(DroppableFakePhysical {
                id: 0,
                drops: Rc::clone(&drops),
            });

            let bound = bound_submission_fixture(&mut session, &coordinator.authority);
            let _prepared = coordinator
                .complete_execution_preserving_physical(bound)
                .unwrap();

            assert_eq!(
                coordinator.physical().id,
                0,
                "the active slot's physical identity must survive a preserving completion \
                 unchanged"
            );
            assert!(
                coordinator.slots[1].is_none(),
                "the inactive slot must remain untouched -- no successor is produced or \
                 consumed by a preserving completion"
            );
            assert!(
                drops.borrow().is_empty(),
                "no P is ever cloned, dropped, or otherwise touched by a preserving completion"
            );
        }

        #[test]
        fn preserving_completion_leaves_an_occupied_inactive_slot_untouched() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            // Occupy the inactive slot via an ordinary completion first, so
            // there is real content there to prove untouched.
            let _first = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(42));
            assert_eq!(coordinator.slots[1], Some(FakePhysical(42)));

            let bound = bound_submission_fixture(&mut session, &coordinator.authority);
            let _prepared = coordinator
                .complete_execution_preserving_physical(bound)
                .unwrap();

            assert_eq!(
                coordinator.slots[1],
                Some(FakePhysical(42)),
                "a preserving completion must not read, overwrite, or clear the inactive slot"
            );
            assert_eq!(coordinator.physical(), &FakePhysical(0));
        }

        #[test]
        fn a_second_preserving_completion_replaces_the_first_readys_metadata() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));

            let bound_a = bound_submission_fixture(&mut session, &coordinator.authority);
            let prepared_a = coordinator
                .complete_execution_preserving_physical(bound_a)
                .unwrap();
            let submission_a = prepared_a.submission();
            let ready_after_a = coordinator.ready.as_ref().unwrap();
            assert_eq!(ready_after_a.submission, submission_a);

            let bound_b = bound_submission_fixture(&mut session, &coordinator.authority);
            let prepared_b = coordinator
                .complete_execution_preserving_physical(bound_b)
                .unwrap();
            let submission_b = prepared_b.submission();
            assert_ne!(submission_a, submission_b);

            let ready_after_b = coordinator.ready.as_ref().unwrap();
            assert_eq!(
                ready_after_b.submission, submission_b,
                "a second preserving completion must cleanly replace the first's ready \
                 metadata, matching complete_execution's own unconditional overwrite \
                 semantics -- not be rejected as \"busy\""
            );

            // Dropping the first prepared capsule (abandoned, never
            // published) must not retroactively poison the second's -- each
            // ordinal owns its own retirement slot.
            drop(prepared_a);
            drop(prepared_b);
        }

        #[test]
        fn dropping_an_abandoned_preserving_completion_does_not_validate_a_later_unrelated_capsule()
        {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));

            let bound_a = bound_submission_fixture(&mut session, &coordinator.authority);
            let prepared_a = coordinator
                .complete_execution_preserving_physical(bound_a)
                .unwrap();
            // Abandon it: dropped without ever reaching
            // prepare_publication/commit. Its retirement records `Rejected`
            // at `BackendReceipt` in its own shared slot.
            drop(prepared_a);

            // A later, unrelated capsule must still be independently
            // provable: complete a fresh ordinal and carry it all the way to
            // a real publication.
            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(3));
            let submission_b = committed.submission();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            let publication = coordinator.prepare_publication(capsule);
            let outcome = publication.commit();
            assert_eq!(
                outcome.submission(),
                submission_b,
                "the later capsule must publish as itself, not be confused with the \
                 abandoned preserving completion's identity"
            );
            assert_eq!(coordinator.physical(), &FakePhysical(3));
        }

        #[test]
        fn preserving_completion_rejects_a_foreign_authority_before_recording_a_ready_slot() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let coordinator = authority.into_coordinator(FakePhysical(0));
            let bound = bound_submission_fixture(&mut session, &coordinator.authority);

            let (_, foreign_authority) = new_raw_dpc_roles().unwrap();
            // Drive the validation-failing call through a genuinely
            // *foreign* coordinator instead, so the original `coordinator`
            // (and its `self.ready`) is provably untouched by the rejected
            // attempt.
            let mut foreign_coordinator = foreign_authority.into_coordinator(FakePhysical(0));
            let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                foreign_coordinator.complete_execution_preserving_physical(bound)
            }));
            assert!(
                trapped.is_err(),
                "a bound submission paired with an unrelated authority must trap before any \
                 ReadyPhysicalSlot is recorded"
            );
            assert!(
                foreign_coordinator.ready.is_none(),
                "a validation failure must never record a ready slot"
            );
            assert!(coordinator.ready.is_none());
        }

        #[test]
        fn replacement_preserve_replacement_leaves_the_coordinator_able_to_publish_a_real_successor(
        ) {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));

            // 1. A normal complete_execution with a real successor.
            let committed_1 =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(11));
            let mut fabric_1 = admitted_fabric();
            let token_1 = fabric_1.pending_dpc_submission().unwrap().token;
            let ready_1 = fabric_1.prepare_dpc_commit(token_1).unwrap();
            let capsule_1 = session.seal_publication(committed_1, ready_1).unwrap();
            coordinator.prepare_publication(capsule_1).commit();
            assert_eq!(coordinator.physical(), &FakePhysical(11));

            // 2. A preserving completion in between -- must not disturb
            // either slot or block a future real completion.
            let bound_mid = bound_submission_fixture(&mut session, &coordinator.authority);
            let _prepared_mid = coordinator
                .complete_execution_preserving_physical(bound_mid)
                .unwrap();
            assert_eq!(coordinator.physical(), &FakePhysical(11));
            assert_eq!(
                coordinator.slots[0],
                Some(FakePhysical(0)),
                "the preserving completion must not touch the (now-inactive) slot 0, still \
                 holding the pre-flip seed value"
            );

            // 3. Another normal complete_execution with a real successor --
            // must cleanly replace the preserving call's ready metadata and
            // publish correctly afterward.
            let committed_3 =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(22));
            let submission_3 = committed_3.submission();
            let mut fabric_3 = admitted_fabric();
            let token_3 = fabric_3.pending_dpc_submission().unwrap().token;
            let ready_3 = fabric_3.prepare_dpc_commit(token_3).unwrap();
            let capsule_3 = session.seal_publication(committed_3, ready_3).unwrap();
            let outcome_3 = coordinator.prepare_publication(capsule_3).commit();

            assert_eq!(outcome_3.submission(), submission_3);
            assert_eq!(coordinator.physical(), &FakePhysical(22));
        }

        #[test]
        fn preserving_completion_rejects_a_write_bearing_packet_before_recording_a_ready_slot() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let coordinator = authority.into_coordinator(FakePhysical(0));
            // `planned_fixture`'s plan carries a real TMEM destination write
            // access -- the exact shape `complete_execution_preserving_physical`
            // must refuse, since it builds its own effects report with an
            // explicitly empty write list and lets
            // `BackendEffectReport::try_new`'s journal-vs-writes length check
            // reject any mismatch.
            let (planned, _destination) = planned_fixture(&session, &coordinator.authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let mut coordinator = coordinator;
            let outcome = coordinator.complete_execution_preserving_physical(bound);
            assert!(
                matches!(outcome, Err(ValidationError::EffectCountMismatch { .. })),
                "a write-bearing packet must be rejected before any ReadyPhysicalSlot is \
                 recorded, not silently accepted with a fabricated empty-writes report"
            );
            assert!(
                coordinator.ready.is_none(),
                "a rejected preserving completion must never record a ready slot"
            );
        }

        #[test]
        fn preserving_completion_publishes_end_to_end_preserving_the_exact_physical_identity() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(5));

            let bound = bound_submission_fixture(&mut session, &coordinator.authority);
            let prepared = coordinator
                .complete_execution_preserving_physical(bound)
                .unwrap();
            let committed = session.commit_zero_guest_writes(prepared).unwrap();
            let submission = committed.submission();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            let publication = coordinator.prepare_publication(capsule);
            let outcome = publication.commit();

            assert_eq!(outcome.submission(), submission);
            assert_eq!(
                coordinator.physical(),
                &FakePhysical(5),
                "the exact pre-existing physical identity must survive a preserving \
                 completion's full publish cycle unchanged -- no successor was ever produced"
            );
            assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
            assert_eq!(
                session.ledger.handles[0].outcome(),
                Some(RawDpcTerminalOutcome::Published)
            );
        }

        #[test]
        fn dropping_an_unconsumed_ready_publication_after_a_preserving_completion_rejects_without_flipping_active(
        ) {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(5));

            let bound = bound_submission_fixture(&mut session, &coordinator.authority);
            let prepared = coordinator
                .complete_execution_preserving_physical(bound)
                .unwrap();
            let committed = session.commit_zero_guest_writes(prepared).unwrap();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            // Mirrors the normal complete_execution path's own
            // dropping_an_unconsumed_ready_publication_... coverage below:
            // an unconsumed ReadyPublication for a *preserving* completion
            // must roll back the fabric and reject the retirement exactly
            // the same way, and must never flip `active` -- there is no
            // successor slot to flip to in the first place.
            drop(coordinator.prepare_publication(capsule));

            assert!(fabric.pending_dpc_submission().is_none());
            assert!(matches!(
                session.ledger.handles[0].outcome(),
                Some(RawDpcTerminalOutcome::Rejected {
                    stage: RawDpcRetirementStage::PhysicalPrepare,
                    ..
                })
            ));
            assert_eq!(
                coordinator.physical(),
                &FakePhysical(5),
                "a dropped, unconsumed ReadyPublication for a preserving completion must \
                 never flip the active slot"
            );
        }

        #[test]
        fn dropping_an_unconsumed_ready_publication_cancels_the_fabric_commit_and_rejects_the_retirement_without_flipping_active(
        ) {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(9));

            // `prepare_dpc_commit`'s rollback restores the *pre-admission*
            // register image (see `fn64-runtime`'s own
            // `dropping_unconsumed_ready_commit_cancels_without_mutation`),
            // so the comparison baseline is a pristine fabric, not one with
            // `request_dpc_submission` already applied.
            let pristine = fn64_runtime::DeviceFabric::new(
                fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
                fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
            )
            .rsp_execution_state();

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            drop(coordinator.prepare_publication(capsule));

            assert_eq!(fabric.rsp_execution_state(), pristine);
            assert!(fabric.pending_dpc_submission().is_none());
            assert!(matches!(
                session.ledger.handles[0].outcome(),
                Some(RawDpcTerminalOutcome::Rejected {
                    stage: RawDpcRetirementStage::PhysicalPrepare,
                    ..
                })
            ));
            assert_eq!(
                *coordinator.physical(),
                FakePhysical(0),
                "a dropped, unconsumed ReadyPublication must never flip the active slot"
            );
        }

        #[test]
        fn prepare_publication_traps_when_capsule_submission_has_no_matching_complete_execution() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            let (planned, _destination) = planned_fixture(&session, &coordinator.authority, true);
            let capture = matching_guest_read_capture(&planned);
            // Bind a `GuestCommittedRawDpc` without ever calling
            // `complete_execution`/`commit_zero_guest_writes` through this
            // coordinator, by hand-building the full chain against a
            // *different* session's queue -- proving `prepare_publication`
            // rejects a capsule with no corresponding ready-slot record
            // rather than silently accepting it.
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            drop(bound);

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();

            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(2));
            let capsule = session.seal_publication(committed, ready).unwrap();
            // Consume the one legitimate ready-slot record so a second
            // `prepare_publication` call has none left to match.
            let _ = coordinator.ready.take();

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                coordinator.prepare_publication(capsule)
            }));
            assert!(
                outcome.is_err(),
                "prepare_publication must trap when no complete_execution recorded a ready slot"
            );
        }

        #[test]
        fn capsule_execution_view_lends_plan_reads_and_packet_under_the_paired_authority() {
            let (mut session, authority) = new_raw_dpc_roles().unwrap();
            let mut coordinator = authority.into_coordinator(FakePhysical(0));
            let committed =
                guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(3));

            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = session.seal_publication(committed, ready).unwrap();

            let mut plan_visitor = RecordingVisitor::default();
            let mut view = RecordingExecutionView::default();
            capsule.execution_view(&coordinator.authority, &mut plan_visitor, &mut view);

            assert_eq!(view.plan_commands_seen, 1);
            assert_eq!(
                view.reads_seen, 1,
                "the fixture's one TmemLoadSource access"
            );
            assert!(view.packet_seen);
        }

        #[test]
        fn commit_zero_guest_writes_commits_the_owning_session_ticket() {
            let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0xab; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let prepared = bound
                .into_backend_prepared(&mut authority, effects)
                .unwrap();

            let committed = session.commit_zero_guest_writes(prepared).unwrap();
            assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
        }

        #[test]
        fn commit_zero_guest_writes_rejects_a_foreign_session_ticket() {
            let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();

            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0xab; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let prepared = bound
                .into_backend_prepared(&mut authority, effects)
                .unwrap();

            let (mut foreign_session, _) = new_raw_dpc_roles().unwrap();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                foreign_session.commit_zero_guest_writes(prepared)
            }));
            assert!(
                outcome.is_err(),
                "a completed ticket from a different session's queue must trap"
            );
        }

        /// Source-shape sweep: this seam forbids `Any`/`TypeId`/downcast/
        /// `FnOnce`-callback machinery anywhere in its production types. A
        /// textual grep is a blunt tool, but it is exactly the class of
        /// regression review can silently miss (a new field slipping in as
        /// `Box<dyn Any>` or a generic trait object standing in for a
        /// concrete authority), so this fails loudly and cheaply on the next
        /// such attempt instead of relying on someone noticing in diff
        /// review.
        #[test]
        fn trait_defaults_reject_planning_execution_and_publication_by_name_or_panic() {
            struct NoOpBackend;

            impl crate::RenderBackend for NoOpBackend {
                fn create(&mut self, _cfg: &crate::RenderConfig) -> Result<(), crate::RenderError> {
                    Ok(())
                }

                fn observe_non_rdp_write16(
                    &mut self,
                    _write: crate::NonRdpWrite16,
                ) -> crate::NonRdpWrite16Disposition {
                    crate::NonRdpWrite16Disposition::NoRustHiddenSidecar
                }

                fn process_task(
                    &mut self,
                    _rdram: &mut [u8],
                    _rsp_memory: &mut fn64_runtime::RspMemory,
                    _task: &crate::OsTask,
                    _output_addr: u32,
                ) -> Result<crate::FrameStatus, crate::RenderError> {
                    Err(crate::RenderError::UnsupportedUcode { ucode_addr: 0 })
                }

                fn present(
                    &mut self,
                    _request: crate::PresentRequest<'_>,
                ) -> Result<(), crate::RenderError> {
                    Ok(())
                }

                fn resize(&mut self, _w: u32, _h: u32) {}

                fn supported_ucodes(&self) -> &[crate::UcodeId] {
                    &[]
                }
            }

            let mut backend: Box<dyn crate::RenderBackend> = Box::new(NoOpBackend);
            assert_eq!(
                backend.raw_dpc_ir_capability(),
                RawDpcIrCapability::Unsupported
            );

            let (session, _authority) = new_raw_dpc_roles().unwrap();
            let layout = PhysicalMemoryLayout::try_new(0x100).unwrap();
            let capture = OwnedRawDpcCapture::new(
                OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
                layout,
                0,
                TemporalBoundary::new(0, DpInterruptState::Clear),
            );
            let request = session.plan_request(capture);
            assert!(matches!(
                backend.plan_raw_dpc(request),
                Err(crate::RenderError::Backend {
                    backend: "render/raw-dpc-plan",
                    ..
                })
            ));

            let (mut real_session, real_authority) = new_raw_dpc_roles().unwrap();
            let (planned, _destination) = planned_fixture(&real_session, &real_authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = real_session.finalize_and_submit(planned, capture).unwrap();
            assert!(matches!(
                backend.execute_raw_dpc(bound),
                Err(crate::RenderError::Backend {
                    backend: "render/raw-dpc-execute",
                    ..
                })
            ));

            // `publish_raw_dpc` has no `Result` in its v11 signature, so its
            // default cannot report "unsupported" the way the other two do;
            // it panics instead (see the trait's doc comment). `NoOpBackend`
            // above -- a `dyn RenderBackend` implementor that overrides none
            // of the four raw-DPC methods -- already proves the trait stays
            // object-safe with `publish_raw_dpc` present (this test compiles
            // and `Box::new(NoOpBackend)` coerces to `Box<dyn RenderBackend>`
            // at all). This block proves the *panicking* half of that
            // default specifically, against a fully sealed capsule built
            // through the real session chain.
            let (mut publish_session, publish_authority) = new_raw_dpc_roles().unwrap();
            let mut publish_coordinator = publish_authority.into_coordinator(FakePhysical(0));
            let committed = guest_committed_fixture(
                &mut publish_session,
                &mut publish_coordinator,
                FakePhysical(1),
            );
            let mut fabric = admitted_fabric();
            let token = fabric.pending_dpc_submission().unwrap().token;
            let ready = fabric.prepare_dpc_commit(token).unwrap();
            let capsule = publish_session.seal_publication(committed, ready).unwrap();

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.publish_raw_dpc(capsule)
            }));
            assert!(
                outcome.is_err(),
                "the unoverridden publish_raw_dpc default must panic, not silently \
                 fabricate a CommittedRawDpcOutcome"
            );
        }

        #[test]
        fn production_module_source_contains_no_type_erasure_or_callback_escape_hatch() {
            let source = include_str!("render_ir.rs");
            let production_start = source
                .find("mod production {")
                .expect("this module's own source must contain its own `mod production` marker");
            let tests_start = source[production_start..]
                .find("#[cfg(test)]\n    mod tests {")
                .map(|offset| production_start + offset)
                .unwrap_or(source.len());
            let code_only: String = source[production_start..tests_start]
                .lines()
                .filter(|line| {
                    let trimmed = line.trim_start();
                    !(trimmed.starts_with("///")
                        || trimmed.starts_with("//!")
                        || trimmed.starts_with("//"))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let any_path = ["std", "any", "Any"].join("::");
            let core_any_path = ["core", "any", "Any"].join("::");
            for forbidden in [
                any_path.as_str(),
                core_any_path.as_str(),
                "TypeId",
                "dyn FnOnce",
                "mem::forget",
                "ManuallyDrop",
            ] {
                assert!(
                    !code_only.contains(forbidden),
                    "production module code (excluding comments/tests) must not contain {forbidden:?}"
                );
            }
        }

        /// Proof that [`ReadyRawDpcCommitCapsule`] alone can never reach
        /// `Published`/`CommittedRawDpcOutcome` -- it has no `commit` and no
        /// other method returning that type -- and that
        /// [`ReadyPublication::commit`] is the sole method anywhere that
        /// does. A textual sweep over both `impl` blocks, not just an
        /// absence check: this fails loudly if a future edit reintroduces a
        /// `pub fn commit` on the capsule itself, or any other
        /// `CommittedRawDpcOutcome`-returning method beside
        /// `ReadyPublication::commit`.
        #[test]
        fn capsule_exposes_no_fabric_only_terminal_route_and_ready_publication_commit_is_the_sole_publish_path(
        ) {
            let source = include_str!("render_ir.rs");

            let capsule_impl_start = source
                .find("impl<'fabric> ReadyRawDpcCommitCapsule<'fabric> {")
                .expect("capsule impl block must exist with this exact signature");
            let capsule_impl_body = &source[capsule_impl_start..];
            let capsule_impl_end = capsule_impl_body
                .find("\n    /// Semantic terminal evidence for one published raw-DPC submission")
                .expect("capsule impl block must be immediately followed by CommittedRawDpcOutcome's doc comment");
            let capsule_impl_body = &capsule_impl_body[..capsule_impl_end];

            assert!(
                !capsule_impl_body.contains("pub fn commit("),
                "ReadyRawDpcCommitCapsule must not expose a bare public `commit`"
            );
            assert_eq!(
                capsule_impl_body
                    .matches("-> CommittedRawDpcOutcome")
                    .count(),
                0,
                "ReadyRawDpcCommitCapsule alone must never be able to reach \
                 CommittedRawDpcOutcome -- publication requires a RawDpcCoordinator"
            );

            let publication_impl_start = source
                .find("impl<'coord, 'fabric, P> ReadyPublication<'coord, 'fabric, P> {")
                .expect("ReadyPublication impl block must exist with this exact signature");
            let publication_impl_body = &source[publication_impl_start..];
            let publication_impl_end = publication_impl_body
                .find("\n    /// Move-only, `#[must_use]` terminal capsule")
                .expect("ReadyPublication impl block must be immediately followed by ReadyRawDpcCommitCapsule's doc comment");
            let publication_impl_body = &publication_impl_body[..publication_impl_end];

            assert_eq!(
                publication_impl_body
                    .matches("-> CommittedRawDpcOutcome")
                    .count(),
                1,
                "exactly one method on ReadyPublication may return CommittedRawDpcOutcome"
            );
            assert!(
                publication_impl_body.contains("pub fn commit("),
                "the one CommittedRawDpcOutcome-returning method must be ReadyPublication::commit"
            );
        }

        /// Shared fixture for `commit_single_guest_render_target_write`'s
        /// test suite: a hand-built `BackendPreparedRawDpc` whose journal
        /// declares exactly one command-decode read plus one
        /// `RenderTarget`/`ColorFramebuffer` write, with a matching
        /// `BackendEffectReport` already issued for that write. Mirrors
        /// `commit_zero_guest_writes_rejects_a_packet_with_a_real_guest_write_even_via_a_hand_built_ticket`'s
        /// hand-built-ticket recipe (same-module private-field construction
        /// is the only way to reach this packet shape at all, since the
        /// normal `ExactRawDpcPlanWriter` path cannot build a FillRectangle
        /// write yet).
        fn render_target_write_fixture(
            queue: SubmissionQueue,
            mut backend_authority: BackendCompletionAuthority,
            guest: GuestCommitAuthority,
            content: fn64_render_ir::ContentDigest,
        ) -> (
            SubmissionQueue,
            BackendCompletionAuthority,
            GuestCommitAuthority,
            BackendPreparedRawDpc,
            ResourceAccess,
        ) {
            let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let guest_write_range = layout.range(0x200, 0x210).unwrap();
            let command_read = ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let guest_write = ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                    range: guest_write_range,
                },
            )
            .unwrap();
            let journal = fn64_render_ir::ResourceJournal::try_new(
                fn64_render_ir::ResourceJournalLimits::try_new(4, 0x100).unwrap(),
                vec![command_read, guest_write],
            )
            .unwrap();
            let stream = fn64_render_ir::RawCommandStream::Dram(
                fn64_render_ir::DramCommandStream::try_new(vec![
                    fn64_render_ir::DramCommandChunk::try_new(
                        command_range,
                        vec![0xf500_0000, 0],
                        fn64_render_ir::TemporalBoundary::new(
                            11,
                            fn64_render_ir::DpInterruptState::Clear,
                        ),
                        Vec::new(),
                    )
                    .unwrap(),
                ])
                .unwrap(),
            );
            let packet = fn64_render_ir::WorkloadPacket::try_new(
                layout,
                fn64_render_ir::WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap();

            let mut queue = queue;
            let submitted = queue
                .submit(fn64_render_ir::DecodedTicket::new(packet))
                .unwrap();
            let write_effect =
                CompletedWrite::try_new(guest_write, guest_write_range.len(), content).unwrap();
            let report =
                BackendEffectReport::try_new(submitted.packet(), vec![write_effect]).unwrap();
            let receipt = backend_authority.issue(&submitted, report).unwrap();
            let complete = submitted.gpu_complete(receipt).unwrap();

            let (dummy_retirement, _handle) =
                SubmittedRawDpcRetirement::new_pair(complete.submission());
            let dummy_plan = ExactValidatedRawDpcPlan {
                source_identity: crate::RawDpcSubmissionIdentity {
                    source: RawDpcSource::Rdram,
                    start: 0x100,
                    end: 0x108,
                    command_sha256: [0; 32],
                },
                journal_identity: complete.packet().journal().identity(),
                commands: Vec::new().into_boxed_slice(),
                accesses: Vec::new().into_boxed_slice(),
            };
            let prepared = BackendPreparedRawDpc {
                plan: dummy_plan,
                complete,
                retirement: dummy_retirement,
            };
            (queue, backend_authority, guest, prepared, guest_write)
        }

        /// A session sharing the exact queue identity `queue`/`guest` were
        /// minted with (the same `TicketAuthoritySet::into_roles()` call),
        /// so both `commit_single_guest_render_target_write`'s own
        /// queue-ownership assertion and `GuestCommitAuthority::issue`'s own
        /// authority check pass, and only the guest-commit re-check under
        /// test can reject the call.
        fn matching_session_for(
            queue: SubmissionQueue,
            guest: GuestCommitAuthority,
        ) -> RawDpcAbiSession {
            RawDpcAbiSession {
                queue,
                guest,
                ledger: RetirementLedger::default(),
            }
        }

        /// Test 1 (design card section 13): happy path -- a packet whose
        /// journal declares exactly one `RenderTarget`/`ColorFramebuffer`
        /// write, committed with the matching `CompletedWrite`, succeeds
        /// and reaches `RawDpcRetirementStage::GuestReceipt`.
        #[test]
        fn commit_single_guest_render_target_write_commits_the_matching_write() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let content = fn64_render_ir::effect_content_digest(&[0xcd; 16]);
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);

            let mut session = matching_session_for(queue, guest);
            let write = CompletedWrite::try_new(
                guest_write,
                guest_write.region().declared_bytes(),
                content,
            )
            .unwrap();
            let committed = session
                .commit_single_guest_render_target_write(prepared, write)
                .unwrap();
            assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
        }

        /// Test 2 (design card section 13): the *old* `commit_zero_guest_writes`
        /// still rejects this same packet with `EffectCountMismatch`
        /// (mirrors the pre-existing hand-built-ticket proof), while the
        /// *new* method succeeds on the same packet shape -- proving the
        /// two methods are each correct for their own respective packet
        /// shapes, not that one silently subsumes the other.
        #[test]
        fn old_zero_write_method_rejects_and_new_method_accepts_the_same_render_target_packet() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let content = fn64_render_ir::effect_content_digest(&[0xce; 16]);
            let (queue, _backend_authority, guest, prepared, _guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);

            let mut zero_write_session = matching_session_for(queue, guest);
            assert_eq!(
                zero_write_session
                    .commit_zero_guest_writes(prepared)
                    .unwrap_err(),
                ValidationError::EffectCountMismatch {
                    field: "guest commit access",
                    expected: 1,
                    actual: 0,
                }
            );

            // Rebuild an equivalent prepared ticket (the prior one was
            // consumed by the rejected call above) to prove the new method
            // succeeds on this exact packet shape.
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);
            let mut new_method_session = matching_session_for(queue, guest);
            let write = CompletedWrite::try_new(
                guest_write,
                guest_write.region().declared_bytes(),
                content,
            )
            .unwrap();
            assert_eq!(
                new_method_session
                    .commit_single_guest_render_target_write(prepared, write)
                    .unwrap()
                    .stage(),
                RawDpcRetirementStage::GuestReceipt
            );
        }

        /// Test 3 (design card section 13): a TMEM-only packet (zero
        /// guest-visible writes declared) supplied to the *new* method with
        /// one fabricated `CompletedWrite` anyway must fail with
        /// `EffectCountMismatch` (expected 0, actual 1) -- the new method
        /// cannot smuggle a write past a packet that never declared one.
        #[test]
        fn commit_single_guest_render_target_write_rejects_a_packet_declaring_zero_guest_writes() {
            let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
            let (planned, destination) = planned_fixture(&session, &authority, true);
            let capture = matching_guest_read_capture(&planned);
            let bound = session.finalize_and_submit(planned, capture).unwrap();
            let write = CompletedWrite::try_new(
                destination,
                16,
                fn64_render_ir::effect_content_digest(&[0x66; 16]),
            )
            .unwrap();
            let effects =
                BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
            let prepared = bound
                .into_backend_prepared(&mut authority, effects)
                .unwrap();

            // The fabricated write must itself be `Write`/`RenderTarget`-
            // shaped so it clears `commit_single_guest_render_target_write`'s
            // own structural pre-check and reaches
            // `GuestCommitEffectReport::try_new`'s count check -- proving
            // *that* check (not the pre-check) is what rejects a
            // TMEM-only packet's extra write.
            let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let fabricated_range = layout.range(0x400, 0x410).unwrap();
            let fabricated_access = ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(9),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                    range: fabricated_range,
                },
            )
            .unwrap();
            let fabricated_write = CompletedWrite::try_new(
                fabricated_access,
                fabricated_range.len(),
                fn64_render_ir::effect_content_digest(&[0x77; 16]),
            )
            .unwrap();
            assert_eq!(
                session
                    .commit_single_guest_render_target_write(prepared, fabricated_write)
                    .unwrap_err(),
                ValidationError::EffectCountMismatch {
                    field: "guest commit access",
                    expected: 0,
                    actual: 1,
                }
            );
        }

        /// Test 4 (design card section 13): a `CompletedWrite` whose access
        /// does not match the journal's declared write access (here, a
        /// different `ColorTargetKey`/range entirely -- otherwise
        /// plausible) is rejected with `EffectAccessMismatch`.
        #[test]
        fn commit_single_guest_render_target_write_rejects_a_mismatched_access() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let content = fn64_render_ir::effect_content_digest(&[0x11; 16]);
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);
            let mut session = matching_session_for(queue, guest);

            let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let other_range = layout.range(0x300, 0x310).unwrap();
            let other_access = ResourceAccess::try_new(
                guest_write.operation(),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                    range: other_range,
                },
            )
            .unwrap();
            let mismatched_write =
                CompletedWrite::try_new(other_access, other_range.len(), content).unwrap();
            assert_eq!(
                session
                    .commit_single_guest_render_target_write(prepared, mismatched_write)
                    .unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "guest commit access",
                    index: 0,
                }
            );
        }

        /// Test 5 (design card section 13): the single most important new
        /// test -- a `CompletedWrite` with the *correct* access but a
        /// *different* content digest than the one the backend already
        /// staged in its `BackendEffectReport` is rejected. This proves the
        /// new method cannot publish bytes other than what the backend
        /// actually staged.
        #[test]
        fn commit_single_guest_render_target_write_rejects_a_content_digest_mismatch() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let staged_content = fn64_render_ir::effect_content_digest(&[0x22; 16]);
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, staged_content);
            let mut session = matching_session_for(queue, guest);

            let different_content = fn64_render_ir::effect_content_digest(&[0x33; 16]);
            let corrupted_write = CompletedWrite::try_new(
                guest_write,
                guest_write.region().declared_bytes(),
                different_content,
            )
            .unwrap();
            assert_eq!(
                session
                    .commit_single_guest_render_target_write(prepared, corrupted_write)
                    .unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "guest commit effect",
                    index: 0,
                }
            );
        }

        /// Test 6 (design card section 13): a `CompletedWrite` proven
        /// against one submission's ticket must not be acceptable to a
        /// *different* submission's ticket, even on the exact same session
        /// queue (same-session, cross-submission -- the new case this
        /// design introduces; the cross-*session* variant is already
        /// covered by `commit_zero_guest_writes_rejects_a_foreign_session_ticket`).
        #[test]
        fn commit_single_guest_render_target_write_rejects_a_same_session_cross_submission_write() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let content_a = fn64_render_ir::effect_content_digest(&[0x44; 16]);
            let (queue, backend_authority, guest, _prepared_a, guest_write_a) =
                render_target_write_fixture(queue, backend_authority, guest, content_a);
            let write_a = CompletedWrite::try_new(
                guest_write_a,
                guest_write_a.region().declared_bytes(),
                content_a,
            )
            .unwrap();

            let content_b = fn64_render_ir::effect_content_digest(&[0x55; 16]);
            let (queue, _backend_authority, guest, prepared_b, _guest_write_b) =
                render_target_write_fixture(queue, backend_authority, guest, content_b);

            let mut session = matching_session_for(queue, guest);
            let outcome = session.commit_single_guest_render_target_write(prepared_b, write_a);
            assert!(
                outcome.is_err(),
                "a CompletedWrite proven against submission A's ticket must not be \
                 accepted against submission B's ticket, even from the same session queue"
            );
        }

        /// `render_target_write_fixture`, widened to **three** disjoint
        /// `RenderTarget`/`ColorFramebuffer` write accesses -- the shape a
        /// partial-width FillRectangle produces, one access per row, whose
        /// rows occupy disjoint width-strided RDRAM ranges.
        ///
        /// The three ranges are deliberately non-contiguous (0x200, 0x240,
        /// 0x280, each 0x10 long): a contiguous trio would be
        /// indistinguishable from one collapsed range, which is exactly the
        /// false claim the N-write design exists to prevent.
        #[allow(clippy::type_complexity)]
        fn three_row_render_target_write_fixture(
            queue: SubmissionQueue,
            mut backend_authority: BackendCompletionAuthority,
            guest: GuestCommitAuthority,
            contents: [fn64_render_ir::ContentDigest; 3],
        ) -> (
            SubmissionQueue,
            BackendCompletionAuthority,
            GuestCommitAuthority,
            BackendPreparedRawDpc,
            [ResourceAccess; 3],
        ) {
            let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
            let command_range = layout.range(0x100, 0x108).unwrap();
            let command_read = ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap();
            let row_starts = [0x200u32, 0x240, 0x280];
            let row_accesses: [ResourceAccess; 3] = core::array::from_fn(|row| {
                let start = row_starts[row];
                ResourceAccess::try_new(
                    fn64_render_ir::OperationId::new(1 + row as u32),
                    AccessMode::Write,
                    AccessPurpose::RenderTarget,
                    fn64_render_ir::ResourceRegion::Rdram {
                        resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                        range: layout.range(start, start + 0x10).unwrap(),
                    },
                )
                .unwrap()
            });
            let journal = fn64_render_ir::ResourceJournal::try_new(
                fn64_render_ir::ResourceJournalLimits::try_new(8, 0x200).unwrap(),
                vec![
                    command_read,
                    row_accesses[0],
                    row_accesses[1],
                    row_accesses[2],
                ],
            )
            .unwrap();
            let stream = fn64_render_ir::RawCommandStream::Dram(
                fn64_render_ir::DramCommandStream::try_new(vec![
                    fn64_render_ir::DramCommandChunk::try_new(
                        command_range,
                        vec![0xf500_0000, 0],
                        fn64_render_ir::TemporalBoundary::new(
                            11,
                            fn64_render_ir::DpInterruptState::Clear,
                        ),
                        Vec::new(),
                    )
                    .unwrap(),
                ])
                .unwrap(),
            );
            let packet = fn64_render_ir::WorkloadPacket::try_new(
                layout,
                fn64_render_ir::WorkloadAdmission::RawDpc {
                    transaction_sequence: 7,
                },
                vec![stream],
                journal,
            )
            .unwrap();

            let mut queue = queue;
            let submitted = queue
                .submit(fn64_render_ir::DecodedTicket::new(packet))
                .unwrap();
            let write_effects: Vec<CompletedWrite> = (0..3)
                .map(|row| {
                    CompletedWrite::try_new(
                        row_accesses[row],
                        row_accesses[row].region().declared_bytes(),
                        contents[row],
                    )
                    .unwrap()
                })
                .collect();
            let report = BackendEffectReport::try_new(submitted.packet(), write_effects).unwrap();
            let receipt = backend_authority.issue(&submitted, report).unwrap();
            let complete = submitted.gpu_complete(receipt).unwrap();

            let (dummy_retirement, _handle) =
                SubmittedRawDpcRetirement::new_pair(complete.submission());
            let dummy_plan = ExactValidatedRawDpcPlan {
                source_identity: crate::RawDpcSubmissionIdentity {
                    source: RawDpcSource::Rdram,
                    start: 0x100,
                    end: 0x108,
                    command_sha256: [0; 32],
                },
                journal_identity: complete.packet().journal().identity(),
                commands: Vec::new().into_boxed_slice(),
                accesses: Vec::new().into_boxed_slice(),
            };
            let prepared = BackendPreparedRawDpc {
                plan: dummy_plan,
                complete,
                retirement: dummy_retirement,
            };
            (queue, backend_authority, guest, prepared, row_accesses)
        }

        fn three_row_contents() -> [fn64_render_ir::ContentDigest; 3] {
            [
                fn64_render_ir::effect_content_digest(&[0xa0; 16]),
                fn64_render_ir::effect_content_digest(&[0xa1; 16]),
                fn64_render_ir::effect_content_digest(&[0xa2; 16]),
            ]
        }

        fn three_row_writes(
            accesses: [ResourceAccess; 3],
            contents: [fn64_render_ir::ContentDigest; 3],
        ) -> Vec<CompletedWrite> {
            (0..3)
                .map(|row| {
                    CompletedWrite::try_new(
                        accesses[row],
                        accesses[row].region().declared_bytes(),
                        contents[row],
                    )
                    .unwrap()
                })
                .collect()
        }

        /// T-1: the N-write proof at the IR seam. A packet declaring three
        /// disjoint `RenderTarget` writes commits cleanly when all three
        /// matching `CompletedWrite`s are supplied, and reaches
        /// `GuestReceipt`. This is the shape a partial-width FillRectangle
        /// produces and the shape the old one-write-only signature could
        /// not express at all.
        #[test]
        fn commit_guest_render_target_writes_commits_three_row_writes() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let contents = three_row_contents();
            let (queue, _backend_authority, guest, prepared, accesses) =
                three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

            let mut session = matching_session_for(queue, guest);
            let committed = session
                .commit_guest_render_target_writes(prepared, three_row_writes(accesses, contents))
                .unwrap();
            assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
        }

        /// T-2: dropping one row is a loud count mismatch, not a partial
        /// commit. A committed subset would silently claim the packet wrote
        /// less than its own journal declares.
        #[test]
        fn commit_guest_render_target_writes_rejects_a_dropped_row() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let contents = three_row_contents();
            let (queue, _backend_authority, guest, prepared, accesses) =
                three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

            let mut session = matching_session_for(queue, guest);
            let mut writes = three_row_writes(accesses, contents);
            writes.pop();
            assert_eq!(
                session
                    .commit_guest_render_target_writes(prepared, writes)
                    .unwrap_err(),
                ValidationError::EffectCountMismatch {
                    field: "guest commit access",
                    expected: 3,
                    actual: 2,
                }
            );
        }

        /// T-3: supplying all three rows in the wrong order is rejected --
        /// proving journal order is load-bearing, not incidental. Rows 0/2/1
        /// carry the right accesses and the right digests; only their
        /// sequence differs.
        #[test]
        fn commit_guest_render_target_writes_rejects_reordered_rows() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let contents = three_row_contents();
            let (queue, _backend_authority, guest, prepared, accesses) =
                three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

            let mut session = matching_session_for(queue, guest);
            let writes = three_row_writes(accesses, contents);
            let reordered = vec![writes[0], writes[2], writes[1]];
            assert_eq!(
                session
                    .commit_guest_render_target_writes(prepared, reordered)
                    .unwrap_err(),
                ValidationError::EffectAccessMismatch {
                    field: "guest commit access",
                    index: 1,
                }
            );
        }

        /// T-4: the one-write convenience wrapper and a one-element N-write
        /// call are the same operation. Asserted on the sealed
        /// `guest_effect_identity()`, so a future divergence in either
        /// body -- an extra check, a different report shape -- fails here
        /// rather than silently producing two different receipts for the
        /// same staged write.
        #[test]
        fn commit_single_guest_render_target_write_still_delegates() {
            let content = fn64_render_ir::effect_content_digest(&[0xbe; 16]);

            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);
            let write = CompletedWrite::try_new(
                guest_write,
                guest_write.region().declared_bytes(),
                content,
            )
            .unwrap();
            let mut single_session = matching_session_for(queue, guest);
            let via_single = single_session
                .commit_single_guest_render_target_write(prepared, write)
                .unwrap();

            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);
            let write = CompletedWrite::try_new(
                guest_write,
                guest_write.region().declared_bytes(),
                content,
            )
            .unwrap();
            let mut plural_session = matching_session_for(queue, guest);
            let via_plural = plural_session
                .commit_guest_render_target_writes(prepared, vec![write])
                .unwrap();

            assert_eq!(
                via_single.committed.guest_effect_identity(),
                via_plural.committed.guest_effect_identity(),
                "the single-write wrapper and a one-element N-write call must produce the \
                 identical sealed guest-effect identity"
            );
        }

        /// Hostile: a non-`RenderTarget` access anywhere in the list --
        /// not merely at index 0 -- is rejected by the shape pre-check. The
        /// old single-write method could only ever check one element, so
        /// "every element is checked" is a genuinely new obligation the
        /// N-write body has to meet.
        #[test]
        fn commit_guest_render_target_writes_rejects_a_non_render_target_write_in_any_position() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let contents = three_row_contents();
            let (queue, _backend_authority, guest, prepared, accesses) =
                three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

            let mut session = matching_session_for(queue, guest);
            let mut writes = three_row_writes(accesses, contents);
            // Rewrite the LAST element's purpose, so a body that only
            // inspected `writes[0]` would wrongly accept this call.
            // `CopyDestination` is chosen because it is a genuinely
            // constructible write purpose for a `ColorFramebuffer` region
            // (unlike, say, `TmemLoadDestination`, which `ResourceAccess::
            // try_new` refuses outright) -- so this test exercises the
            // session's own shape pre-check, not the journal's constructor.
            let wrong_purpose = ResourceAccess::try_new(
                accesses[2].operation(),
                AccessMode::Write,
                AccessPurpose::CopyDestination,
                accesses[2].region(),
            )
            .unwrap();
            writes[2] = CompletedWrite::try_new(
                wrong_purpose,
                wrong_purpose.region().declared_bytes(),
                contents[2],
            )
            .unwrap();
            assert_eq!(
                session
                    .commit_guest_render_target_writes(prepared, writes)
                    .unwrap_err(),
                ValidationError::GuestRenderTargetWriteShapeMismatch {
                    mode: "Write",
                    purpose: "CopyDestination",
                }
            );
        }

        /// Test 7 (design card section 13): a `CompletedWrite` whose access
        /// mode/purpose does not match the single admitted shape (`Write`,
        /// `RenderTarget`) is rejected by the method's own structural
        /// pre-check before `GuestCommitEffectReport::try_new` is ever
        /// called -- assert the specific error, not merely "some `Err`".
        #[test]
        fn commit_single_guest_render_target_write_rejects_wrong_mode_or_purpose_at_the_precheck() {
            let (queue, backend_authority, guest) =
                TicketAuthoritySet::try_new().unwrap().into_roles();
            let content = fn64_render_ir::effect_content_digest(&[0x88; 16]);
            let (queue, _backend_authority, guest, prepared, guest_write) =
                render_target_write_fixture(queue, backend_authority, guest, content);
            let mut session = matching_session_for(queue, guest);

            let tmem_destination_range = fn64_render_ir::TmemRange::try_new(0, 16).unwrap();
            let wrong_purpose_access = ResourceAccess::try_new(
                guest_write.operation(),
                AccessMode::Write,
                AccessPurpose::TmemLoadDestination,
                fn64_render_ir::ResourceRegion::Tmem(tmem_destination_range),
            )
            .unwrap();
            let wrong_purpose_write = CompletedWrite::try_new(
                wrong_purpose_access,
                tmem_destination_range.len(),
                content,
            )
            .unwrap();
            assert_eq!(
                session
                    .commit_single_guest_render_target_write(prepared, wrong_purpose_write)
                    .unwrap_err(),
                ValidationError::GuestRenderTargetWriteShapeMismatch {
                    mode: "Write",
                    purpose: "TmemLoadDestination",
                }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{
        AccessPurpose, DpInterruptState, OperationId, PhysicalMemoryLayout, RdramResource,
        ResourceJournalLimits,
    };

    #[test]
    fn validated_capture_becomes_one_move_only_decoded_packet() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let range = layout.range(0x100, 0x108).unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 8).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range,
                },
            )
            .unwrap()],
        )
        .unwrap();
        let decoded = decode_raw_dpc_capture(
            layout,
            7,
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
            TemporalBoundary::new(11, DpInterruptState::Clear),
            Vec::new(),
            journal,
        )
        .unwrap();

        assert_eq!(decoded.packet().streams().len(), 1);
        assert_eq!(decoded.packet().owned_command_bytes(), 8);
        assert_eq!(
            decoded.packet().admission(),
            WorkloadAdmission::RawDpc {
                transaction_sequence: 7
            }
        );
    }

    #[test]
    fn preflight_rejects_missing_command_ownership_before_exposing_a_read_plan() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 8).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(7),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: layout.range(0x200, 0x208).unwrap(),
                },
            )
            .unwrap()],
        )
        .unwrap();
        assert_eq!(
            preflight_raw_dpc_capture(
                layout,
                7,
                OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
                TemporalBoundary::new(11, DpInterruptState::Clear),
                Vec::new(),
                journal,
            )
            .unwrap_err(),
            ValidationError::MissingCommandReadDeclaration {
                source: fn64_render_ir::RawStreamKind::Dram,
                start: 0x100,
                end: 0x108,
            }
        );
    }
}
