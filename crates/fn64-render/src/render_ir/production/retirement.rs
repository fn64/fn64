use super::*;

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

/// Write-once shared terminal slot. The renderer may execute an owned
/// submission on a worker while the ABI retains its diagnostic handle,
/// so terminal publication uses one lock-free compare/exchange. Guest
/// execution remains single-threaded; this atomic closes only the exact
/// worker-drop versus ABI-observation interleaving.
#[derive(Debug)]
pub(super) struct RetirementSlot {
    submission: SubmissionIdentity,
    state: AtomicU8,
}

impl RetirementSlot {
    const EMPTY: u8 = 0;
    const PUBLISHED: u8 = 1;
    const REJECTED_EXECUTE: u8 = 2;
    const REJECTED_BACKEND_RECEIPT: u8 = 3;
    const REJECTED_GUEST_RECEIPT: u8 = 4;
    const REJECTED_FABRIC_PREPARE: u8 = 5;
    const REJECTED_PHYSICAL_PREPARE: u8 = 6;
    const FOREIGN_SUBMISSION: u8 = 7;

    fn new(submission: SubmissionIdentity) -> Arc<Self> {
        Arc::new(Self {
            submission,
            state: AtomicU8::new(Self::EMPTY),
        })
    }

    /// Record the terminal outcome if (and only if) the slot is still
    /// empty. A second call after a value is already recorded is a no-op:
    /// exactly one terminal record ever survives per ordinal.
    fn record_if_empty(&self, outcome: RawDpcTerminalOutcome) {
        let state = match outcome {
            RawDpcTerminalOutcome::Published => Self::PUBLISHED,
            RawDpcTerminalOutcome::Rejected { stage, submission } => {
                if submission != self.submission {
                    Self::FOREIGN_SUBMISSION
                } else {
                    match stage {
                        RawDpcRetirementStage::Execute => Self::REJECTED_EXECUTE,
                        RawDpcRetirementStage::BackendReceipt => Self::REJECTED_BACKEND_RECEIPT,
                        RawDpcRetirementStage::GuestReceipt => Self::REJECTED_GUEST_RECEIPT,
                        RawDpcRetirementStage::FabricPrepare => Self::REJECTED_FABRIC_PREPARE,
                        RawDpcRetirementStage::PhysicalPrepare => Self::REJECTED_PHYSICAL_PREPARE,
                    }
                }
            }
        };
        let _ =
            self.state
                .compare_exchange(Self::EMPTY, state, Ordering::AcqRel, Ordering::Acquire);
    }

    fn get(&self) -> Option<RawDpcTerminalOutcome> {
        let rejected = |stage| RawDpcTerminalOutcome::Rejected {
            stage,
            submission: self.submission,
        };
        match self.state.load(Ordering::Acquire) {
            Self::EMPTY => None,
            Self::PUBLISHED => Some(RawDpcTerminalOutcome::Published),
            Self::REJECTED_EXECUTE => Some(rejected(RawDpcRetirementStage::Execute)),
            Self::REJECTED_BACKEND_RECEIPT => Some(rejected(RawDpcRetirementStage::BackendReceipt)),
            Self::REJECTED_GUEST_RECEIPT => Some(rejected(RawDpcRetirementStage::GuestReceipt)),
            Self::REJECTED_FABRIC_PREPARE => Some(rejected(RawDpcRetirementStage::FabricPrepare)),
            Self::REJECTED_PHYSICAL_PREPARE => {
                Some(rejected(RawDpcRetirementStage::PhysicalPrepare))
            }
            Self::FOREIGN_SUBMISSION => panic!(
                "retirement terminal record named a foreign submission for {:?}",
                self.submission
            ),
            state => panic!("invalid retirement terminal state {state}"),
        }
    }
}

/// ABI-ledger-visible handle for diagnostics. Reading it never mutates
/// and never blocks on the armed owner.
#[derive(Clone, Debug)]
pub struct RawDpcRetirementHandle {
    pub(super) slot: Arc<RetirementSlot>,
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
pub(super) struct RetirementLedger {
    pub(super) handles: Vec<RawDpcRetirementHandle>,
}

impl RetirementLedger {
    pub(super) fn record(&mut self, handle: RawDpcRetirementHandle) {
        self.handles.push(handle);
    }
}

/// Armed owner of one issued ordinal's terminal record. Every post-submit
/// typestate carries this by value. Its `Drop` performs only "if empty,
/// set `Rejected`" against the pre-created shared slot: no allocation, no
/// lock acquisition, and no panic is possible during unwind.
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
/// also greps for that. Consequently the shared `Arc<RetirementSlot>`
/// this type wraps is the *same* allocation from issuance through
/// terminal record, and exactly one destructor (the last typestate still
/// holding this value when it is dropped, or the disarming
/// `publish`-equivalent) can ever write to it.
#[derive(Debug)]
pub(super) struct SubmittedRawDpcRetirement {
    pub(super) slot: Arc<RetirementSlot>,
    submission: SubmissionIdentity,
    stage: RawDpcRetirementStage,
    armed: bool,
}

impl SubmittedRawDpcRetirement {
    /// Arm a fresh retirement plus the ABI-ledger diagnostic handle that
    /// shares its terminal slot.
    pub(super) fn new_pair(submission: SubmissionIdentity) -> (Self, RawDpcRetirementHandle) {
        let slot = RetirementSlot::new(submission);
        let handle = RawDpcRetirementHandle {
            slot: Arc::clone(&slot),
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

    pub(super) const fn stage(&self) -> RawDpcRetirementStage {
        self.stage
    }

    /// Advance the recorded stage. This never touches the shared slot; it
    /// only changes what a later `Drop`-time rejection would report.
    pub(super) fn advance_stage(&mut self, stage: RawDpcRetirementStage) {
        self.stage = stage;
    }

    /// Disarm this owner as `Published`. Only
    /// [`ReadyRawDpcCommitCapsule::commit`]'s successful terminal
    /// publication calls this; every other destruction path (`Err`,
    /// early explicit drop, or unwind) leaves `armed` set and lets `Drop`
    /// record `Rejected` at the last-advanced stage.
    pub(super) fn disarm_published(mut self) {
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
