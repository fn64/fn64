use super::*;

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
    pub(super) preflight: IrRawDpcPacketPreflight,
    pub(super) plan: ExactValidatedRawDpcPlan,
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
    pub(super) plan: ExactValidatedRawDpcPlan,
    pub(super) submitted: fn64_render_ir::SubmittedTicket,
    pub(super) submission_identity: SubmissionIdentity,
    pub(super) queue: QueueIdentity,
    pub(super) ordinal: u64,
    pub(super) retirement: SubmittedRawDpcRetirement,
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
    pub(super) plan: ExactValidatedRawDpcPlan,
    pub(super) complete: GpuCompleteTicket,
    pub(super) retirement: SubmittedRawDpcRetirement,
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
    pub(super) plan: ExactValidatedRawDpcPlan,
    pub(super) committed: GuestCommittedTicket,
    pub(super) retirement: SubmittedRawDpcRetirement,
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
/// coordinator now keeps its own private `Arc::clone` of the same
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
    pub(super) plan: ExactValidatedRawDpcPlan,
    pub(super) committed: GuestCommittedTicket,
    pub(super) fabric: fn64_runtime::device::ReadyDpcFabricCommit<'fabric>,
    pub(super) retirement: SubmittedRawDpcRetirement,
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
    pub(super) submission: SubmissionIdentity,
}

impl CommittedRawDpcOutcome {
    pub const fn submission(self) -> SubmissionIdentity {
        self.submission
    }
}
