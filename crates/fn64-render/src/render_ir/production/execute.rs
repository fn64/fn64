use super::*;

/// Split one fresh [`TicketAuthoritySet`] into the ABI session and
/// backend authority roles this production seam uses. The third role
/// ([`fn64_render_ir::GuestCommitAuthority`]) lives inside the session;
/// nothing outside this module can reach it independently.
pub fn new_raw_dpc_roles() -> Result<(RawDpcAbiSession, RawDpcBackendAuthority), ValidationError> {
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
            guest_read_moments: Vec::new(),
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
    guest_read_moments: Vec<PendingGuestReadCommandMoment>,
}

#[derive(Clone, Copy, Debug)]
struct PendingGuestReadCommandMoment {
    access_index: u32,
    operation: fn64_render_ir::OperationId,
    location: RawDpcCommandLocation,
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
            !load.sources().is_empty(),
            "a TMEM load always reads at least one source access"
        );
        let location = load.location();
        for access in load.sources().iter().copied() {
            self.push_access_at_command(access, location);
        }
        self.accesses.push(load.destination());
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

    /// Push one access whose guest-read moment is the completion of `location`.
    pub fn push_command_access_at(
        &mut self,
        access: ResourceAccess,
        location: RawDpcCommandLocation,
    ) {
        self.push_access_at_command(access, location);
    }

    fn push_access_at_command(&mut self, access: ResourceAccess, location: RawDpcCommandLocation) {
        if access.purpose() == AccessPurpose::TmemLoadSource {
            self.guest_read_moments.push(PendingGuestReadCommandMoment {
                access_index: u32::try_from(self.accesses.len())
                    .expect("bounded raw-DPC access list fits u32"),
                operation: access.operation(),
                location,
            });
        }
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
    pub fn push_texture_rectangle_accesses(
        &mut self,
        accesses: &[ResourceAccess],
        location: RawDpcCommandLocation,
    ) {
        for access in accesses.iter().copied() {
            self.push_access_at_command(access, location);
        }
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
        for (index, (pushed, journaled)) in self.accesses.iter().zip(journal_accesses).enumerate() {
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
        let guest_read_moments = self
            .guest_read_moments
            .iter()
            .map(|binding| {
                let command_end_byte_offset = binding
                    .location
                    .source_byte_offset
                    .checked_add(binding.location.source_byte_len)
                    .ok_or(ValidationError::NumericOverflow {
                        field: "raw-DPC command-completion byte offset",
                    })?;
                Ok(GuestReadCommandMoment::new(
                    binding.access_index,
                    binding.operation,
                    CommandCompletionMoment::new(
                        binding.location.stream_index,
                        command_end_byte_offset,
                    ),
                ))
            })
            .collect::<Result<Vec<_>, ValidationError>>()?;
        let preflight = preflight_raw_dpc_capture_with_guest_read_command_moments(
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
            &guest_read_moments,
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
