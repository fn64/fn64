use super::*;

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
    pub(super) queue: SubmissionQueue,
    pub(super) guest: GuestCommitAuthority,
    pub(super) ledger: RetirementLedger,
}

/// Backend-owned role: the paired completion authority. Its `begin_plan`
/// is the sole route to a plan-writing capability, and it rejects an
/// unpaired request's queue identity before any plan field can be
/// written.
#[derive(Debug)]
pub struct RawDpcBackendAuthority {
    pub(super) authority: BackendCompletionAuthority,
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
            slots: vec![Some(initial), None],
            active: 0,
            ready: None,
            batch_ready: VecDeque::new(),
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
/// `retirement_slot` is a private `Arc::clone` of the exact
/// `SubmittedRawDpcRetirement`'s shared slot this ordinal's
/// `complete_execution` call observed -- the same allocation the
/// [`RawDpcRetirementHandle`] the session's own ledger holds also
/// points at. It exists for two reasons: [`RawDpcCoordinator::
/// prepare_publication`] can `Arc::ptr_eq` it against the capsule's own
/// retirement to prove the capsule being published really is the one
/// this exact ready slot was prepared for (not merely a
/// queue/submission coincidence), and a coordinator that never sees a
/// matching `prepare_publication` call for an ordinal can still poll
/// this clone's outcome to notice the abandoned/rejected candidate and
/// reap its slot -- without a public capsule-side accessor a caller
/// could otherwise use to reach the same information from outside this
/// module.
pub(super) struct ReadyPhysicalSlot {
    queue: QueueIdentity,
    pub(super) submission: SubmissionIdentity,
    slot_index: usize,
    retirement_slot: Arc<RetirementSlot>,
}

/// Owns one backend's paired [`RawDpcBackendAuthority`] together with
/// its publication-buffered physical state `P` and the private metadata that
/// binds one specific prepared slot to one specific submission. `P` is
/// the backend's own physical candidate/state value -- a plain owned
/// type, never a callback or trait object -- so this type is generic
/// over it without knowing or caring about its fields.
///
/// Ordinary execution uses two slots; an explicit execution batch adds
/// one immutable successor slot per completion until the batch is
/// published in order. **Why slots, not one `mem::replace`.** `P` is an arbitrary
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
    pub(super) authority: RawDpcBackendAuthority,
    pub(super) slots: Vec<Option<P>>,
    active: usize,
    pub(super) ready: Option<ReadyPhysicalSlot>,
    batch_ready: VecDeque<ReadyPhysicalSlot>,
}

impl<P> RawDpcCoordinator<P> {
    /// The currently-published physical state -- the value the last
    /// successful [`ReadyPublication::commit`] flipped `active` to, or
    /// `into_coordinator`'s `initial` if none has published yet.
    pub fn physical(&self) -> &P {
        self.slots[self.active]
            .as_ref()
            .expect("the active coordinator slot is always occupied")
    }

    /// Forwards to [`RawDpcBackendAuthority::begin_plan`] against this
    /// coordinator's own paired authority.
    pub fn begin_plan(&self, request: RawDpcPlanRequest) -> ExactRawDpcPlanWriter {
        self.authority.begin_plan(request)
    }

    /// Begin an ordered multi-submission execution lifetime. No ordinary
    /// prepared publication may be outstanding. Old inactive candidates
    /// are reaped here, before any new execution or terminal commit.
    pub fn begin_execution_batch(&mut self) -> RawDpcExecutionBatch<'_, P> {
        assert!(
            self.ready.is_none() && self.batch_ready.is_empty(),
            "begin_execution_batch requires no unpublished completion"
        );
        if self.active != 0 {
            self.slots.swap(0, self.active);
            self.active = 0;
        }
        self.slots.truncate(1);
        self.slots.push(None);
        RawDpcExecutionBatch {
            coordinator: self,
            current_physical: 0,
            ready: VecDeque::new(),
        }
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
        assert!(
            self.batch_ready.is_empty(),
            "ordinary completion cannot overtake a prepared execution batch"
        );
        let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
        let inactive = if self.active == 0 { 1 } else { 0 };
        self.slots[inactive] = Some(next_physical);
        self.ready = Some(ReadyPhysicalSlot {
            queue: self.authority.authority.queue_identity(),
            submission: prepared.submission(),
            slot_index: inactive,
            retirement_slot: Arc::clone(&prepared.retirement.slot),
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
        assert!(
            self.batch_ready.is_empty(),
            "ordinary completion cannot overtake a prepared execution batch"
        );
        let effects =
            fn64_render_ir::BackendEffectReport::try_new(bound.submitted.packet(), Vec::new())?;
        let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
        self.ready = Some(ReadyPhysicalSlot {
            queue: self.authority.authority.queue_identity(),
            submission: prepared.submission(),
            slot_index: self.active,
            retirement_slot: Arc::clone(&prepared.retirement.slot),
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
        assert!(
            self.batch_ready.is_empty(),
            "ordinary completion cannot overtake a prepared execution batch"
        );
        let prepared = bound.into_backend_prepared(&mut self.authority, effects)?;
        self.ready = Some(ReadyPhysicalSlot {
            queue: self.authority.authority.queue_identity(),
            submission: prepared.submission(),
            slot_index: self.active,
            retirement_slot: Arc::clone(&prepared.retirement.slot),
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
            .or_else(|| self.batch_ready.pop_front())
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
            Arc::ptr_eq(&ready.retirement_slot, &capsule.retirement.slot),
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

/// Move-only guard for one ordered backend execution batch. Every
/// completion retains its own queue/submission/retirement identity while
/// the physical successor of submission N becomes the execution input of
/// N+1 before either is published.
pub struct RawDpcExecutionBatch<'coord, P> {
    coordinator: &'coord mut RawDpcCoordinator<P>,
    current_physical: usize,
    ready: VecDeque<ReadyPhysicalSlot>,
}

impl<P> RawDpcExecutionBatch<'_, P> {
    /// Begin an exact plan using the coordinator's paired backend
    /// authority while this batch exclusively owns the coordinator.
    pub fn begin_plan(&self, request: RawDpcPlanRequest) -> ExactRawDpcPlanWriter {
        self.coordinator.authority.begin_plan(request)
    }

    /// Return the physical successor produced by the most recently
    /// completed member, or the published seed before the first member.
    pub fn physical(&self) -> &P {
        self.coordinator.slots[self.current_physical]
            .as_ref()
            .expect("the current batch physical slot is occupied")
    }

    /// Visit one bound member's exact plan through the paired authority.
    pub fn execution_view<PV: ExactRawDpcPlanVisitor, V: RawDpcExecutionView<PV>>(
        &self,
        bound: &BoundSubmittedRawDpc,
        plan_visitor: &mut PV,
        view: &mut V,
    ) {
        bound.execution_view(&self.coordinator.authority, plan_visitor, view);
    }

    /// Record one completed member and retain its physical successor as
    /// both this member's future publication and the next member's input.
    pub fn complete_execution(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: fn64_render_ir::BackendEffectReport,
        next_physical: P,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        let prepared = bound.into_backend_prepared(&mut self.coordinator.authority, effects)?;
        let slot_index = self.coordinator.slots.len();
        self.coordinator.slots.push(Some(next_physical));
        self.current_physical = slot_index;
        self.ready.push_back(ReadyPhysicalSlot {
            queue: self.coordinator.authority.authority.queue_identity(),
            submission: prepared.submission(),
            slot_index,
            retirement_slot: Arc::clone(&prepared.retirement.slot),
        });
        Ok(prepared)
    }

    /// Record one completed member with real effects but no new physical
    /// successor; its publication retains the current batch slot.
    pub fn complete_execution_preserving_physical_with_effects(
        &mut self,
        bound: BoundSubmittedRawDpc,
        effects: fn64_render_ir::BackendEffectReport,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        let prepared = bound.into_backend_prepared(&mut self.coordinator.authority, effects)?;
        self.ready.push_back(ReadyPhysicalSlot {
            queue: self.coordinator.authority.authority.queue_identity(),
            submission: prepared.submission(),
            slot_index: self.current_physical,
            retirement_slot: Arc::clone(&prepared.retirement.slot),
        });
        Ok(prepared)
    }

    /// Record one genuinely zero-write member without constructing or
    /// replacing a physical successor. The packet journal independently
    /// proves the empty effect list before any ready record is retained.
    pub fn complete_execution_preserving_physical(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, ValidationError> {
        let effects =
            fn64_render_ir::BackendEffectReport::try_new(bound.submitted.packet(), Vec::new())?;
        self.complete_execution_preserving_physical_with_effects(bound, effects)
    }

    /// Seal the ordered ready queue into the coordinator. Dropping the
    /// guard instead leaves no publishable metadata.
    pub fn finish(mut self) {
        assert!(
            self.coordinator.batch_ready.is_empty(),
            "an execution batch is already awaiting publication"
        );
        self.coordinator.batch_ready = core::mem::take(&mut self.ready);
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
    ready_index: usize,
    pub(super) capsule: ReadyRawDpcCommitCapsule<'fabric>,
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
