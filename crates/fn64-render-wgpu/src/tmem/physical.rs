//! Transactional physical TMEM storage.
//!
//! The public RDP transfer-word shape comes from the SGI *Nintendo 64 RDP
//! Command Summary* and Programming Manual section 13.9. This layer consumes
//! the already-checked M4.2.0 word and fragment plan; it does not recalculate
//! DXT carries, odd-row XOR4 placement, or RGBA32 bank selection. Publication
//! follows fn64's move-only render-IR guest-commit contract. RT64 is not
//! hardware authority for this state engine.

use core::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use fn64_render_ir::{
    AccessPurpose, CompletedWrite, ContentDigest, EffectIdentity, GpuCompleteTicket,
    GuestCommittedTicket, JournalIdentity, QueueIdentity, ResourceAccess, ResourceJournal,
    ResourceJournalLimits, ResourceRegion, ValidationError, WorkloadAdmission, TMEM_BYTES,
};

use crate::raw_dpc::BoundTmemTransfer;

use super::{
    ProposedTmemImageIdentity, TmemByteSource, TmemLoadEpoch, TmemLoadSourceIdentity,
    TmemSnapshotIdentity, TmemTransferPhysicalWord, TmemTransferWord,
};

const TMEM_LEN: usize = TMEM_BYTES as usize;
const PROPOSAL_DOMAIN: &[u8] = b"fn64.render-wgpu.physical-tmem-proposal.v1\0";
static NEXT_PHYSICAL_TMEM_STATE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PHYSICAL_TMEM_TRANSACTION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PHYSICAL_TMEM_LOAD_ID: AtomicU64 = AtomicU64::new(1);

/// Uncaller-chosen identity of one durable physical TMEM allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicalTmemStateIdentity(u64);

impl PhysicalTmemStateIdentity {
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Uncaller-chosen identity of one packet-local physical TMEM transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicalTmemTransactionIdentity(u64);

impl PhysicalTmemTransactionIdentity {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalTmemLoadIdentity(u64);

/// Defined bytes for one accepted complete TMEM word, already placed in its
/// eight physical fragment lanes.
///
/// For a linear word, lanes ascend through its eight-byte range. For a split
/// word, lanes 0..4 ascend through the low-bank fragment and lanes 4..8 ascend
/// through the high-bank fragment. `None` is required for every undefined
/// lane. The crate-private constructor validates the physical lane mask but
/// does not accept or map a captured logical source read: M4.2b's LoadTile
/// and M4.2c's LoadBlock executors must perform the source-to-physical-lane
/// mapping.
///
/// The payload is move-only and bound to the exact uncaller-chosen packet and
/// load transaction that created it. Equal word geometry cannot rebind it to
/// another state, source, submission, destination, or load.
#[derive(Debug, PartialEq, Eq)]
pub struct DefinedPhysicalTmemWordBytes {
    packet_binding: PhysicalTmemBinding,
    load_binding: PhysicalTmemLoadBinding,
    word: TmemTransferWord,
    physical_lanes: [Option<u8>; 8],
}

impl DefinedPhysicalTmemWordBytes {
    fn try_from_physical_lanes(
        packet_binding: PhysicalTmemBinding,
        load_binding: PhysicalTmemLoadBinding,
        word: TmemTransferWord,
        physical_lanes: [Option<u8>; 8],
    ) -> Result<Self, PhysicalTmemError> {
        let expected = physical_defined_lane_mask(word)?;
        let actual = physical_lanes
            .iter()
            .enumerate()
            .fold(0_u8, |mask, (lane, byte)| {
                mask | (u8::from(byte.is_some()) << lane)
            });
        if actual != expected {
            return Err(PhysicalTmemError::PhysicalLaneMaskMismatch {
                index: word.index(),
                expected,
                actual,
            });
        }
        Ok(Self {
            packet_binding,
            load_binding,
            word,
            physical_lanes,
        })
    }

    pub(crate) const fn word(&self) -> TmemTransferWord {
        self.word
    }

    pub(crate) const fn physical_lanes(&self) -> &[Option<u8>; 8] {
        &self.physical_lanes
    }
}

/// Durable renderer-owned 4 KiB TMEM image.
///
/// Invalid bytes retain their underlying storage so an undefined transfer tail
/// cannot manufacture a value. Public observation exposes only valid bytes.
#[derive(Debug, PartialEq, Eq)]
pub struct PhysicalTmemState {
    identity: PhysicalTmemStateIdentity,
    bytes: Box<[u8; TMEM_LEN]>,
    valid: Box<[bool; TMEM_LEN]>,
    last_touched_generation: Box<[u64; TMEM_LEN]>,
    generation: u64,
    last_load_epoch: Option<TmemLoadEpoch>,
}

impl PhysicalTmemState {
    pub fn try_new() -> Result<Self, PhysicalTmemError> {
        let identity = NEXT_PHYSICAL_TMEM_STATE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(PhysicalTmemStateIdentity)
            .map_err(|_| PhysicalTmemError::StateIdentityExhausted)?;
        Ok(Self {
            identity,
            bytes: Box::new([0; TMEM_LEN]),
            valid: Box::new([false; TMEM_LEN]),
            last_touched_generation: Box::new([0; TMEM_LEN]),
            generation: 0,
            last_load_epoch: None,
        })
    }

    pub const fn identity(&self) -> PhysicalTmemStateIdentity {
        self.identity
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn last_load_epoch(&self) -> Option<TmemLoadEpoch> {
        self.last_load_epoch
    }

    /// Returns a byte only when its latest complete-word touch defined it.
    /// Out-of-range and invalid storage are both intentionally unobservable.
    pub fn valid_byte(&self, address: u16) -> Option<u8> {
        let address = usize::from(address);
        (address < TMEM_LEN && self.valid[address]).then(|| self.bytes[address])
    }

    pub fn byte_is_valid(&self, address: u16) -> bool {
        let address = usize::from(address);
        address < TMEM_LEN && self.valid[address]
    }

    pub fn last_touched_generation(&self, address: u16) -> Option<u64> {
        let address = usize::from(address);
        (address < TMEM_LEN).then(|| self.last_touched_generation[address])
    }

    /// Begins the first load in one packet-local physical TMEM transaction.
    /// Later loads are chained from [`PhysicalTmemPacketTransaction`].
    pub fn stage_transfer(
        &self,
        submitted: &fn64_render_ir::SubmittedTicket,
        transfer: &BoundTmemTransfer<'_>,
    ) -> Result<StagedTmemTransaction, PhysicalTmemError> {
        let packet = PhysicalTmemPacketTransaction {
            binding: packet_binding(self, submitted, transfer)?,
            bytes: self.bytes.clone(),
            valid: self.valid.clone(),
            last_touched_generation: self.last_touched_generation.clone(),
            last_load_epoch: self.last_load_epoch,
            projections: Vec::new(),
            effects: Vec::new(),
            expected_destination_accesses: submitted
                .packet()
                .journal()
                .accesses()
                .iter()
                .copied()
                .filter(|access| access.purpose() == AccessPurpose::TmemLoadDestination)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        };
        packet.stage_transfer(submitted, transfer)
    }

    pub fn publication_authority(&mut self) -> PhysicalTmemPublicationAuthority<'_> {
        PhysicalTmemPublicationAuthority { durable: self }
    }

    /// T3 Phase B's sole staging entry point for a load reached through
    /// `fn64_render`'s sealed neutral raw-DPC plan
    /// (`fn64_render::TmemLoadSemantics`), rather than through
    /// this crate's own private decoder (`fn64_render_ir::SubmittedTicket` +
    /// [`crate::raw_dpc::BoundTmemTransfer`], which [`Self::stage_transfer`]
    /// above requires). A production `RenderBackend::execute_raw_dpc`
    /// implementation only ever holds a `BoundSubmittedRawDpc`'s
    /// authority-scoped, nonextracting `execution_view` -- it cannot obtain
    /// a `SubmittedTicket` or `BoundTmemTransfer` to call `stage_transfer`
    /// with, by the production seam's own design (see `docs/DESIGN.md`'s T0
    /// section: "a bare `DecodedTicket`/`SubmittedTicket` never escapes to a
    /// caller"). This method takes the equivalent facts in their neutral
    /// (`fn64-render`-owned) shape instead: the workload/journal/submission
    /// identity a real ABI-side capture would have produced, the queue and
    /// submission ordinal `BoundSubmittedRawDpc::queue()`/`ordinal()`
    /// already expose directly, and one load's complete
    /// `TmemLoadSemantics` plus its exact ordered destination access slice
    /// (which the caller must have already collected from the plan's own
    /// `access()` visitor callback, in journal order, starting at
    /// `load.destination_access_index()` -- exactly the same slice
    /// [`crate::raw_dpc::push_decoded_raw_dpc`]'s `push_tmem_load` pushed
    /// for this load).
    ///
    /// Performs the identical checks [`Self::stage_transfer`]/
    /// [`PhysicalTmemPacketTransaction::stage_transfer`] perform against
    /// their own decoder-typed inputs -- source identity, epoch ordering,
    /// word/access count agreement, and the destination-access/physical-word
    /// coverage cross-check ([`validate_physical_plan`]) -- against these
    /// neutral inputs instead. It does not weaken, skip, or reorder any of
    /// them.
    pub(crate) fn stage_neutral_transfer(
        &self,
        source: TmemLoadSourceIdentity,
        queue: QueueIdentity,
        submission_ordinal: u64,
        transaction_sequence: u64,
        load: &fn64_render::TmemLoadSemantics,
        destination_accesses: &[ResourceAccess],
    ) -> Result<StagedTmemTransaction, PhysicalTmemError> {
        let packet = PhysicalTmemPacketTransaction {
            binding: neutral_packet_binding(
                self,
                source,
                queue,
                submission_ordinal,
                transaction_sequence,
            )?,
            bytes: self.bytes.clone(),
            valid: self.valid.clone(),
            last_touched_generation: self.last_touched_generation.clone(),
            last_load_epoch: self.last_load_epoch,
            projections: Vec::new(),
            effects: Vec::new(),
            // Seeded empty: `PhysicalTmemPacketTransaction::stage_neutral_transfer`
            // below is the sole place that appends `destination_accesses`,
            // called uniformly for the first load (here) and every
            // second-and-later load (`stage_neutral_transfer_next`). Seeding
            // this with `destination_accesses` here too would double-count
            // load one's own destinations once the chained call below
            // appends them again.
            expected_destination_accesses: Box::new([]),
        };
        packet.stage_neutral_transfer(source, load, destination_accesses)
    }
}

/// Exact durable-state and packet lifecycle identity shared by every load in
/// one transaction, its proposed effects, and publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalTmemBinding {
    state: PhysicalTmemStateIdentity,
    transaction: PhysicalTmemTransactionIdentity,
    source: TmemLoadSourceIdentity,
    queue: QueueIdentity,
    submission_ordinal: u64,
    transaction_sequence: u64,
    base_generation: u64,
    next_generation: u64,
    base_last_load_epoch: Option<TmemLoadEpoch>,
}

impl PhysicalTmemBinding {
    pub const fn state(self) -> PhysicalTmemStateIdentity {
        self.state
    }

    pub const fn transaction(self) -> PhysicalTmemTransactionIdentity {
        self.transaction
    }

    pub const fn source(self) -> TmemLoadSourceIdentity {
        self.source
    }

    pub const fn queue(self) -> QueueIdentity {
        self.queue
    }

    pub const fn submission_ordinal(self) -> u64 {
        self.submission_ordinal
    }

    pub const fn transaction_sequence(self) -> u64 {
        self.transaction_sequence
    }

    pub const fn base_generation(self) -> u64 {
        self.base_generation
    }

    pub const fn next_generation(self) -> u64 {
        self.next_generation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalTmemLoadBinding {
    identity: PhysicalTmemLoadIdentity,
    source_access_identity: JournalIdentity,
    source_first_access_index: u32,
    source_access_count: u16,
    destination_access_identity: JournalIdentity,
    destination_first_access_index: u32,
    destination_access_count: u16,
    epoch: TmemLoadEpoch,
}

/// Candidate state between complete loads in one packet. It is move-only: a
/// next load or final sealing consumes it.
pub struct PhysicalTmemPacketTransaction {
    binding: PhysicalTmemBinding,
    bytes: Box<[u8; TMEM_LEN]>,
    valid: Box<[bool; TMEM_LEN]>,
    last_touched_generation: Box<[u64; TMEM_LEN]>,
    last_load_epoch: Option<TmemLoadEpoch>,
    projections: Vec<LoadProjection>,
    effects: Vec<CompletedWrite>,
    expected_destination_accesses: Box<[ResourceAccess]>,
}

impl fmt::Debug for PhysicalTmemPacketTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalTmemPacketTransaction")
            .field("binding", &self.binding)
            .field("last_load_epoch", &self.last_load_epoch)
            .field("completed_loads", &self.projections.len())
            .field("effect_count", &self.effects.len())
            .finish()
    }
}

impl PhysicalTmemPacketTransaction {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.binding
    }

    pub const fn last_load_epoch(&self) -> Option<TmemLoadEpoch> {
        self.last_load_epoch
    }

    pub fn completed_loads(&self) -> usize {
        self.projections.len()
    }

    /// Begins the next accepted load. Any error consumes and rolls back the
    /// whole packet-local candidate.
    pub fn stage_transfer(
        self,
        submitted: &fn64_render_ir::SubmittedTicket,
        transfer: &BoundTmemTransfer<'_>,
    ) -> Result<StagedTmemTransaction, PhysicalTmemError> {
        let load_binding = validate_transfer(
            submitted,
            transfer,
            self.binding.source,
            self.last_load_epoch,
        )?;
        Ok(StagedTmemTransaction {
            packet: self,
            load_binding,
            destination_accesses: transfer.destination_accesses().to_vec().into_boxed_slice(),
            words: transfer.words().to_vec().into_boxed_slice(),
            next_word: 0,
            poisoned: false,
        })
    }

    /// Neutral-plan counterpart to [`Self::stage_transfer`] (the chaining,
    /// second-and-later-load overload): begins the next accepted load
    /// against this already-in-progress packet transaction, exactly as
    /// [`Self::stage_transfer`] chains subsequent decoder-typed loads. See
    /// [`PhysicalTmemState::stage_neutral_transfer`] for why the neutral
    /// entry point exists.
    pub(crate) fn stage_neutral_transfer_next(
        self,
        expected_source: TmemLoadSourceIdentity,
        load: &fn64_render::TmemLoadSemantics,
        destination_accesses: &[ResourceAccess],
    ) -> Result<StagedTmemTransaction, PhysicalTmemError> {
        self.stage_neutral_transfer(expected_source, load, destination_accesses)
    }

    /// Neutral-plan counterpart to [`Self::stage_transfer`]; see
    /// [`PhysicalTmemState::stage_neutral_transfer`] for why this entry
    /// point exists and what it accepts instead of a decoder-typed
    /// `SubmittedTicket`/`BoundTmemTransfer` pair.
    ///
    /// The decoder-typed path (`stage_transfer`) seeds
    /// `expected_destination_accesses` once, from the *whole* submitted
    /// journal, because a `SubmittedTicket` exposes it up front. The
    /// neutral path has no such upfront whole-journal object: each call
    /// only ever carries the destinations for *this one load*
    /// (`stage_and_report` visits the plan's loads one at a time). So this
    /// method must extend the running set here, on every second-and-later
    /// load, or `into_pending`'s coverage check only ever sees load one's
    /// destinations and rejects any multi-load plan.
    fn stage_neutral_transfer(
        mut self,
        expected_source: TmemLoadSourceIdentity,
        load: &fn64_render::TmemLoadSemantics,
        destination_accesses: &[ResourceAccess],
    ) -> Result<StagedTmemTransaction, PhysicalTmemError> {
        let load_binding = neutral_validate_transfer(
            load,
            destination_accesses,
            expected_source,
            self.last_load_epoch,
        )?;
        let words: Vec<TmemTransferWord> = load
            .transfer_words()
            .iter()
            .copied()
            .map(neutral_transfer_word)
            .collect();
        self.expected_destination_accesses = self
            .expected_destination_accesses
            .iter()
            .copied()
            .chain(destination_accesses.iter().copied())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(StagedTmemTransaction {
            packet: self,
            load_binding,
            destination_accesses: destination_accesses.to_vec().into_boxed_slice(),
            words: words.into_boxed_slice(),
            next_word: 0,
            poisoned: false,
        })
    }

    /// Seals all completed loads and exposes their immutable proposed effects.
    /// Sealing requires exact access-for-access coverage of every packet
    /// journal `TmemLoadDestination` write in journal order.
    pub fn into_pending(self) -> Result<PendingTmemTransaction, PhysicalTmemError> {
        if self.projections.is_empty() {
            return Err(PhysicalTmemError::NoCompletedLoads);
        }
        if self.effects.len() != self.expected_destination_accesses.len()
            || self
                .effects
                .iter()
                .zip(&self.expected_destination_accesses)
                .any(|(effect, expected)| effect.access() != *expected)
        {
            return Err(PhysicalTmemError::DestinationCoverageMismatch {
                expected: self.expected_destination_accesses.len(),
                actual: self.effects.len(),
            });
        }
        let proposal_identity = proposal_identity(
            self.binding,
            self.last_load_epoch,
            &self.projections,
            &self.effects,
        );
        Ok(PendingTmemTransaction {
            binding: self.binding,
            bytes: self.bytes,
            valid: self.valid,
            last_touched_generation: self.last_touched_generation,
            last_load_epoch: self.last_load_epoch,
            projections: self.projections.into_boxed_slice(),
            effects: self.effects.into_boxed_slice(),
            proposal_identity,
        })
    }
}

/// One in-progress complete-word load inside a packet transaction.
pub struct StagedTmemTransaction {
    packet: PhysicalTmemPacketTransaction,
    load_binding: PhysicalTmemLoadBinding,
    destination_accesses: Box<[ResourceAccess]>,
    words: Box<[TmemTransferWord]>,
    next_word: usize,
    poisoned: bool,
}

impl fmt::Debug for StagedTmemTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StagedTmemTransaction")
            .field("binding", &self.packet.binding)
            .field("epoch", &self.load_binding.epoch)
            .field("word_count", &self.words.len())
            .field("next_word", &self.next_word)
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

impl StagedTmemTransaction {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.packet.binding
    }

    pub const fn epoch(&self) -> TmemLoadEpoch {
        self.load_binding.epoch
    }

    pub fn expected_words(&self) -> &[TmemTransferWord] {
        &self.words
    }

    /// Asserts bytes that M4.2b's LoadTile, M4.2c's LoadBlock, or M4.3.2's
    /// LoadTLUT executor has already arranged into this active load's
    /// physical fragment lanes. The returned payload cannot be rebound to an
    /// equal-looking word in another load or packet transaction.
    pub(crate) fn physical_word_payload(
        &self,
        word: TmemTransferWord,
        physical_lanes: [Option<u8>; 8],
    ) -> Result<DefinedPhysicalTmemWordBytes, PhysicalTmemError> {
        DefinedPhysicalTmemWordBytes::try_from_physical_lanes(
            self.packet.binding,
            self.load_binding,
            word,
            physical_lanes,
        )
    }

    /// Stages one opaque physical-lane payload. Undefined lanes take no byte
    /// input: their prior backing is preserved, validity is cleared, and touch
    /// generation advances.
    pub fn stage_word(
        &mut self,
        physical_bytes: DefinedPhysicalTmemWordBytes,
    ) -> Result<(), PhysicalTmemError> {
        if self.poisoned {
            return Err(PhysicalTmemError::PoisonedTransaction);
        }
        let result = self.stage_word_inner(physical_bytes);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn stage_word_inner(
        &mut self,
        physical_bytes: DefinedPhysicalTmemWordBytes,
    ) -> Result<(), PhysicalTmemError> {
        validate_payload_binding(
            physical_bytes.packet_binding,
            physical_bytes.load_binding,
            self.packet.binding,
            self.load_binding,
        )?;
        let word = physical_bytes.word();
        if usize::from(word.index()) < self.next_word {
            return Err(PhysicalTmemError::DuplicateWord {
                index: word.index(),
            });
        }
        let expected =
            self.words
                .get(self.next_word)
                .copied()
                .ok_or(PhysicalTmemError::ExtraWord {
                    index: word.index(),
                })?;
        if word.physical() != expected.physical() {
            return Err(PhysicalTmemError::FragmentMismatch {
                index: expected.index(),
            });
        }
        if word != expected {
            return Err(PhysicalTmemError::WordPlanMismatch {
                expected: expected.index(),
                actual: word.index(),
            });
        }

        let lanes = fragment_lanes(expected.physical())?;
        let physical_lanes = physical_bytes.physical_lanes();
        for (lane, address) in lanes.into_iter().enumerate() {
            self.packet.last_touched_generation[address] = self.packet.binding.next_generation;
            if let Some(byte) = physical_lanes[lane] {
                self.packet.bytes[address] = byte;
                self.packet.valid[address] = true;
            } else {
                self.packet.valid[address] = false;
            }
        }
        self.next_word += 1;
        Ok(())
    }

    /// Completes this load, snapshots its canonical postimage effects, and
    /// returns ownership ready for the next load or final sealing.
    pub fn finish_load(mut self) -> Result<PhysicalTmemPacketTransaction, PhysicalTmemError> {
        if self.poisoned {
            return Err(PhysicalTmemError::PoisonedTransaction);
        }
        if self.next_word != self.words.len() {
            return Err(PhysicalTmemError::IncompleteTransfer {
                expected: self.words.len(),
                actual: self.next_word,
            });
        }
        let (projection, effects) = project_load(
            self.load_binding,
            &self.packet.bytes,
            &self.packet.valid,
            &self.packet.last_touched_generation,
            &self.destination_accesses,
        )?;
        self.packet.last_load_epoch = Some(self.load_binding.epoch);
        self.packet.effects.extend(effects);
        self.packet.projections.push(projection);
        Ok(self.packet)
    }
}

// M4.2b's LoadTile, M4.2c's LoadBlock, and M4.3.2's LoadTLUT executors are
// the production callers of this crate-private assertion seam. Retain its
// exact type in ordinary builds without making raw physical-byte assertion
// reachable to downstream crates beyond those owners.
type PhysicalWordPayloadMint = fn(
    &StagedTmemTransaction,
    TmemTransferWord,
    [Option<u8>; 8],
) -> Result<DefinedPhysicalTmemWordBytes, PhysicalTmemError>;
const _: PhysicalWordPayloadMint = StagedTmemTransaction::physical_word_payload;

/// Complete packet transaction awaiting an exact GPU effect report.
pub struct PendingTmemTransaction {
    binding: PhysicalTmemBinding,
    bytes: Box<[u8; TMEM_LEN]>,
    valid: Box<[bool; TMEM_LEN]>,
    last_touched_generation: Box<[u64; TMEM_LEN]>,
    last_load_epoch: Option<TmemLoadEpoch>,
    projections: Box<[LoadProjection]>,
    effects: Box<[CompletedWrite]>,
    proposal_identity: ContentDigest,
}

impl fmt::Debug for PendingTmemTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingTmemTransaction")
            .field("binding", &self.binding)
            .field("completed_loads", &self.projections.len())
            .field("effect_count", &self.effects.len())
            .field("proposal_identity", &self.proposal_identity)
            .finish()
    }
}

impl PendingTmemTransaction {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.binding
    }

    pub const fn last_load_epoch(&self) -> Option<TmemLoadEpoch> {
        self.last_load_epoch
    }

    pub fn completed_loads(&self) -> usize {
        self.projections.len()
    }

    /// Exact proposed TMEM effects for later backend-report aggregation.
    pub fn proposed_effects(&self) -> &[CompletedWrite] {
        &self.effects
    }

    /// A read-only view of this transaction's **proposed post-image** --
    /// the exact `bytes`/`valid` arrays a successful publication would
    /// install -- usable as a [`super::TmemByteSource`] before any
    /// publication exists.
    ///
    /// This is the seam a texture rectangle in the *same packet* as its own
    /// `LoadBlock`/`LoadTile`/`LoadTLUT` needs. Such a texrect must sample
    /// texels its own packet loaded, and those texels exist only in this
    /// post-image until `into_physical_successor`/`publish` runs -- which
    /// happens strictly *after* staging, i.e. after the point the texrect
    /// has to produce its pixels. Census-measured, this is not a corner
    /// case but the only case: of WM2000's 219 decode entries, 86 carry
    /// both a `G_TEXRECT` and a TMEM load and **zero** carry a texrect
    /// without a load in the same entry, so "sample a prior packet's
    /// committed TMEM" is a shape that never occurs.
    ///
    /// ## What this does not relax
    ///
    /// Three things, each deliberately preserved:
    ///
    /// 1. **No publication.** This borrows; it cannot commit, cannot
    ///    advance a generation, and cannot be converted into a
    ///    `PhysicalTmemState`. `into_physical_successor` and
    ///    `PhysicalTmemPublicationAuthority::publish` remain the only two
    ///    routes to a durable post-image, with every base-state,
    ///    generation, epoch, effect-report and `validate_proposal` check
    ///    they already ran, unchanged and in the same order.
    /// 2. **No forged snapshot identity.** Reads through this view answer
    ///    with `TmemSnapshotIdentity::Proposed`, never `Committed`. A
    ///    pending post-image genuinely has no durable `(state, generation)`
    ///    pair: `binding.state` names the *base* state and
    ///    `binding.next_generation` names a generation that will not exist
    ///    if publication is rejected. Minting a
    ///    `PhysicalTmemSnapshotIdentity` from that pair is precisely the
    ///    forgery the committed/pending split prevents, so the type system
    ///    prevents it instead of a convention.
    /// 3. **No effect-report participation.** Reading is not a write.
    ///    Nothing observed here enters `proposed_effects`, so
    ///    `validate_proposal`'s recomputation and
    ///    `validate_backend_effects`' supersequence walk see exactly what
    ///    they saw before this method existed.
    ///
    /// The proposal digest the view reports is this transaction's own
    /// `proposal_identity`, so a recorded pending read names the exact
    /// proposal content it observed -- and `validate_proposal` recomputes
    /// that digest at both publication routes, so a read cannot be
    /// attributed to a proposal that has since changed.
    pub fn pending_image(&self) -> PendingTmemImage<'_> {
        PendingTmemImage { pending: self }
    }

    pub const fn proposal_identity(&self) -> ContentDigest {
        self.proposal_identity
    }

    /// Binds the candidate to the uncaller-chosen identity of the exact backend
    /// report that contains its proposed TMEM writes. Other declared backend
    /// writes may appear around the TMEM writes.
    pub fn bind_gpu(
        self,
        complete: &GpuCompleteTicket,
    ) -> Result<GpuBoundTmemTransaction, PhysicalTmemError> {
        validate_proposal(&self)?;
        validate_gpu(complete, self.binding, &self.effects)?;
        Ok(GpuBoundTmemTransaction {
            pending: self,
            backend_effect_identity: complete.backend_effect_identity(),
        })
    }

    /// Produces a complete, inactive [`PhysicalTmemState`] successor to
    /// `base` for [`fn64_render::RawDpcCoordinator::complete_execution`]'s
    /// `next_physical` slot -- without touching `base` itself. `base` is the
    /// coordinator's currently-*active* `P`; the returned value is a fresh,
    /// independent state carrying this transaction's postimage, exactly as
    /// [`PhysicalTmemPublicationAuthority::publish`] would durably become,
    /// but never written into `base` and never exposed until a later
    /// `commit` flips a coordinator's active slot to it.
    ///
    /// Runs the identical three base-state checks `publish` runs, in the
    /// same order, against the same fields
    /// (`CrossStatePublication`/`StaleBaseGeneration`/`StaleLoadEpoch`),
    /// then an exact match between `effects`' declared writes and this
    /// transaction's own proposed effects -- same access, same order, no
    /// extra or missing write (`BackendEffectMismatch`) -- then
    /// self-consistency (`validate_proposal`) last, exactly as `publish`
    /// orders its own final check. No `GuestCommittedTicket`/
    /// `GpuCompleteTicket` receipt is consulted or required: this method
    /// exists to hand a backend a durable-shaped candidate before any guest
    /// commit exists, not to publish one.
    pub fn into_physical_successor(
        self,
        base: &PhysicalTmemState,
        effects: &fn64_render_ir::BackendEffectReport,
    ) -> Result<PhysicalTmemState, PhysicalTmemError> {
        if base.identity != self.binding.state {
            return Err(PhysicalTmemError::CrossStatePublication {
                expected: self.binding.state,
                actual: base.identity,
            });
        }
        if base.generation != self.binding.base_generation {
            return Err(PhysicalTmemError::StaleBaseGeneration {
                expected: self.binding.base_generation,
                actual: base.generation,
            });
        }
        if base.last_load_epoch != self.binding.base_last_load_epoch {
            return Err(PhysicalTmemError::StaleLoadEpoch {
                expected: self.binding.base_last_load_epoch,
                actual: base.last_load_epoch,
            });
        }
        validate_backend_effects(effects.writes(), &self.effects)?;
        validate_proposal(&self)?;

        let identity = NEXT_PHYSICAL_TMEM_STATE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(PhysicalTmemStateIdentity)
            .map_err(|_| PhysicalTmemError::StateIdentityExhausted)?;
        Ok(PhysicalTmemState {
            identity,
            bytes: self.bytes,
            valid: self.valid,
            last_touched_generation: self.last_touched_generation,
            generation: self.binding.next_generation,
            last_load_epoch: self.last_load_epoch,
        })
    }
}

/// Borrowed read-only view of a [`PendingTmemTransaction`]'s proposed
/// post-image, as a [`TmemByteSource`].
///
/// Created only by [`PendingTmemTransaction::pending_image`], whose doc
/// states what this view does and does not relax. It holds a shared borrow
/// of the transaction, so the transaction cannot be published, converted
/// into a successor, or otherwise consumed while any read is outstanding --
/// the "read the post-image, then publish it" ordering is enforced by the
/// borrow checker rather than by review.
#[derive(Debug)]
pub struct PendingTmemImage<'pending> {
    pending: &'pending PendingTmemTransaction,
}

impl PendingTmemImage<'_> {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.pending.binding
    }

    /// This view's own identity, the same value its reads report.
    pub const fn identity(&self) -> ProposedTmemImageIdentity {
        ProposedTmemImageIdentity::new(
            self.pending.proposal_identity,
            self.pending.binding.state,
            self.pending.binding.transaction,
            self.pending.binding.next_generation,
        )
    }
}

impl TmemByteSource for PendingTmemImage<'_> {
    fn snapshot(&self) -> TmemSnapshotIdentity {
        TmemSnapshotIdentity::Proposed(self.identity())
    }

    /// Answers from the transaction's own staged post-image arrays -- the
    /// exact `bytes`/`valid` a publication would install -- with the
    /// identical validity gate and identical out-of-range handling
    /// `PhysicalTmemState::valid_byte` applies. The two implementations
    /// agree byte-for-byte on the same arrays by construction; only the
    /// snapshot identity differs.
    fn valid_byte(&self, address: u16) -> Option<u8> {
        let address = usize::from(address);
        (address < TMEM_LEN && self.pending.valid[address]).then(|| self.pending.bytes[address])
    }
}

/// Move-only candidate bound to the exact backend report that completed it.
pub struct GpuBoundTmemTransaction {
    pending: PendingTmemTransaction,
    backend_effect_identity: EffectIdentity,
}

impl fmt::Debug for GpuBoundTmemTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuBoundTmemTransaction")
            .field("binding", &self.pending.binding)
            .field("proposal_identity", &self.pending.proposal_identity)
            .field("backend_effect_identity", &self.backend_effect_identity)
            .finish()
    }
}

impl GpuBoundTmemTransaction {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.pending.binding
    }

    pub fn proposed_effects(&self) -> &[CompletedWrite] {
        &self.pending.effects
    }

    pub const fn proposal_identity(&self) -> ContentDigest {
        self.pending.proposal_identity
    }

    pub const fn backend_effect_identity(&self) -> EffectIdentity {
        self.backend_effect_identity
    }
}

/// Exclusive durable-publication capability. It issues no guest/backend
/// receipt; it only validates an already-issued exact guest ticket.
pub struct PhysicalTmemPublicationAuthority<'state> {
    durable: &'state mut PhysicalTmemState,
}

impl fmt::Debug for PhysicalTmemPublicationAuthority<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalTmemPublicationAuthority")
            .field("state", &self.durable.identity)
            .field("generation", &self.durable.generation)
            .finish_non_exhaustive()
    }
}

impl PhysicalTmemPublicationAuthority<'_> {
    pub fn publish(
        &mut self,
        gpu_bound: GpuBoundTmemTransaction,
        guest: GuestCommittedTicket,
    ) -> Result<CommittedTmemTransaction, PhysicalTmemError> {
        let backend_effect_identity = gpu_bound.backend_effect_identity;
        let pending = gpu_bound.pending;
        if self.durable.identity != pending.binding.state {
            return Err(PhysicalTmemError::CrossStatePublication {
                expected: pending.binding.state,
                actual: self.durable.identity,
            });
        }
        // Interleaving closed: A and B may both stage from generation N. If B
        // publishes N+1 first, A must fail here instead of overwriting B with
        // a candidate derived from the stale N state.
        if self.durable.generation != pending.binding.base_generation {
            return Err(PhysicalTmemError::StaleBaseGeneration {
                expected: pending.binding.base_generation,
                actual: self.durable.generation,
            });
        }
        if self.durable.last_load_epoch != pending.binding.base_last_load_epoch {
            return Err(PhysicalTmemError::StaleLoadEpoch {
                expected: pending.binding.base_last_load_epoch,
                actual: self.durable.last_load_epoch,
            });
        }
        validate_guest(&guest, pending.binding, &pending.projections)?;
        if guest.backend_effect_identity() != backend_effect_identity {
            return Err(PhysicalTmemError::GuestCommitMismatch {
                field: "backend effect identity",
            });
        }
        validate_proposal(&pending)?;

        self.durable.bytes = pending.bytes;
        self.durable.valid = pending.valid;
        self.durable.last_touched_generation = pending.last_touched_generation;
        self.durable.generation = pending.binding.next_generation;
        self.durable.last_load_epoch = pending.last_load_epoch;

        Ok(CommittedTmemTransaction {
            guest,
            binding: pending.binding,
            effects: pending.effects,
            proposal_identity: pending.proposal_identity,
            load_count: pending.projections.len(),
            last_load_epoch: pending.last_load_epoch,
        })
    }
}

/// Successfully published physical TMEM packet transaction.
pub struct CommittedTmemTransaction {
    guest: GuestCommittedTicket,
    binding: PhysicalTmemBinding,
    effects: Box<[CompletedWrite]>,
    proposal_identity: ContentDigest,
    load_count: usize,
    last_load_epoch: Option<TmemLoadEpoch>,
}

impl fmt::Debug for CommittedTmemTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommittedTmemTransaction")
            .field("binding", &self.binding)
            .field("load_count", &self.load_count)
            .field("last_load_epoch", &self.last_load_epoch)
            .field("proposal_identity", &self.proposal_identity)
            .finish_non_exhaustive()
    }
}

impl CommittedTmemTransaction {
    pub const fn binding(&self) -> PhysicalTmemBinding {
        self.binding
    }

    pub fn completed_effects(&self) -> &[CompletedWrite] {
        &self.effects
    }

    pub const fn proposal_identity(&self) -> ContentDigest {
        self.proposal_identity
    }

    pub const fn completed_loads(&self) -> usize {
        self.load_count
    }

    pub const fn last_load_epoch(&self) -> Option<TmemLoadEpoch> {
        self.last_load_epoch
    }

    pub fn into_guest_ticket(self) -> GuestCommittedTicket {
        self.guest
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PhysicalTmemError {
    StateIdentityExhausted,
    TransactionIdentityExhausted,
    LoadIdentityExhausted,
    DeferredTransfer,
    SubmissionMismatch {
        field: &'static str,
    },
    TransferPlanMismatch {
        field: &'static str,
    },
    DestinationPlanMismatch,
    /// A load's ordered source-access run could not be expressed as a
    /// physical binding -- today, only because its length exceeds `u16`.
    /// Named separately from [`Self::DestinationPlanMismatch`] so a source
    /// side failure is never reported as a destination one; the decoder
    /// already bounds the run by `MAX_RESOURCE_ACCESSES`, so this is a
    /// defence-in-depth refusal rather than a reachable production path.
    SourcePlanMismatch,
    GenerationExhausted,
    EpochNotNewer {
        previous: Option<TmemLoadEpoch>,
        actual: TmemLoadEpoch,
    },
    DuplicateWord {
        index: u16,
    },
    ExtraWord {
        index: u16,
    },
    FragmentMismatch {
        index: u16,
    },
    WordPlanMismatch {
        expected: u16,
        actual: u16,
    },
    PhysicalLaneMaskMismatch {
        index: u16,
        expected: u8,
        actual: u8,
    },
    PhysicalLanePayloadMismatch {
        field: &'static str,
    },
    IncompleteTransfer {
        expected: usize,
        actual: usize,
    },
    NoCompletedLoads,
    DestinationCoverageMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidPhysicalFragment,
    PoisonedTransaction,
    CrossStatePublication {
        expected: PhysicalTmemStateIdentity,
        actual: PhysicalTmemStateIdentity,
    },
    StaleBaseGeneration {
        expected: u64,
        actual: u64,
    },
    StaleLoadEpoch {
        expected: Option<TmemLoadEpoch>,
        actual: Option<TmemLoadEpoch>,
    },
    GuestCommitMismatch {
        field: &'static str,
    },
    GpuCompletionMismatch {
        field: &'static str,
    },
    BackendEffectMismatch {
        field: &'static str,
    },
    ProposalMismatch,
    Ir(ValidationError),
}

impl fmt::Display for PhysicalTmemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateIdentityExhausted => {
                formatter.write_str("physical TMEM state identity authority exhausted")
            }
            Self::TransactionIdentityExhausted => {
                formatter.write_str("physical TMEM transaction identity authority exhausted")
            }
            Self::LoadIdentityExhausted => {
                formatter.write_str("physical TMEM load identity authority exhausted")
            }
            Self::DeferredTransfer => formatter.write_str(
                "physical TMEM state accepts only an exact closed LoadBlock/LoadTile/LoadTLUT transfer plan, never a still-deferred contract",
            ),
            Self::SubmissionMismatch { field } => {
                write!(
                    formatter,
                    "TMEM transfer belongs to another submission at {field}"
                )
            }
            Self::TransferPlanMismatch { field } => {
                write!(formatter, "TMEM transfer plan differs at {field}")
            }
            Self::DestinationPlanMismatch => formatter
                .write_str("TMEM physical fragments differ from the canonical destination plan"),
            Self::SourcePlanMismatch => formatter
                .write_str("TMEM load source-access run is not expressible as a physical binding"),
            Self::GenerationExhausted => formatter.write_str("physical TMEM generation exhausted"),
            Self::EpochNotNewer { previous, actual } => write!(
                formatter,
                "TMEM load epoch {} does not follow transaction epoch {previous:?}",
                actual.get()
            ),
            Self::DuplicateWord { index } => {
                write!(formatter, "TMEM transfer word {index} was staged twice")
            }
            Self::ExtraWord { index } => {
                write!(formatter, "TMEM transfer has unexpected extra word {index}")
            }
            Self::FragmentMismatch { index } => {
                write!(
                    formatter,
                    "TMEM transfer word {index} uses the wrong physical fragment"
                )
            }
            Self::WordPlanMismatch { expected, actual } => write!(
                formatter,
                "TMEM transfer expected word {expected}, found word {actual} from another plan"
            ),
            Self::PhysicalLaneMaskMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "TMEM transfer word {index} needs physical-lane mask {expected:#04x}, found {actual:#04x}"
            ),
            Self::PhysicalLanePayloadMismatch { field } => {
                write!(formatter, "TMEM physical-lane payload differs at {field}")
            }
            Self::IncompleteTransfer { expected, actual } => write!(
                formatter,
                "TMEM transfer staged {actual} of {expected} complete words"
            ),
            Self::NoCompletedLoads => {
                formatter.write_str("TMEM packet transaction has no completed loads")
            }
            Self::DestinationCoverageMismatch { expected, actual } => write!(
                formatter,
                "TMEM packet transaction projected {actual} of {expected} destination accesses"
            ),
            Self::InvalidPhysicalFragment => {
                formatter.write_str("TMEM transfer contains a malformed physical fragment")
            }
            Self::PoisonedTransaction => {
                formatter.write_str("TMEM staging transaction is poisoned")
            }
            Self::CrossStatePublication { expected, actual } => write!(
                formatter,
                "TMEM transaction for state {} cannot publish into state {}",
                expected.get(),
                actual.get()
            ),
            Self::StaleBaseGeneration { expected, actual } => write!(
                formatter,
                "TMEM publication expected base generation {expected}, found {actual}"
            ),
            Self::StaleLoadEpoch { expected, actual } => write!(
                formatter,
                "TMEM publication expected base load epoch {expected:?}, found {actual:?}"
            ),
            Self::GuestCommitMismatch { field } => {
                write!(formatter, "TMEM guest-commit ticket differs at {field}")
            }
            Self::GpuCompletionMismatch { field } => {
                write!(formatter, "TMEM GPU-complete ticket differs at {field}")
            }
            Self::BackendEffectMismatch { field } => {
                write!(formatter, "TMEM backend effect report differs at {field}")
            }
            Self::ProposalMismatch => {
                formatter.write_str("TMEM proposed effects differ at publication")
            }
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PhysicalTmemError {}

impl From<ValidationError> for PhysicalTmemError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LoadProjection {
    binding: PhysicalTmemLoadBinding,
    accesses: Box<[CanonicalTmemEffectProjection]>,
}

#[derive(Debug, PartialEq, Eq)]
struct CanonicalTmemEffectProjection {
    access: ResourceAccess,
    load_epoch: TmemLoadEpoch,
    normalized_bytes: Box<[u8]>,
    validity: Box<[u8]>,
    last_touched_generation: Box<[u64]>,
}

fn packet_binding(
    state: &PhysicalTmemState,
    submitted: &fn64_render_ir::SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
) -> Result<PhysicalTmemBinding, PhysicalTmemError> {
    let load = transfer.load();
    let plan = load
        .transfer_plan()
        .map_err(|_| PhysicalTmemError::DeferredTransfer)?;
    let source = plan.source().identity();
    validate_source_binding(submitted, source)?;
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = submitted.packet().admission()
    else {
        return Err(PhysicalTmemError::SubmissionMismatch {
            field: "raw-DPC admission",
        });
    };
    let next_generation = state
        .generation
        .checked_add(1)
        .ok_or(PhysicalTmemError::GenerationExhausted)?;
    Ok(PhysicalTmemBinding {
        state: state.identity,
        transaction: mint_transaction_identity()?,
        source,
        queue: submitted.queue(),
        submission_ordinal: submitted.ordinal(),
        transaction_sequence,
        base_generation: state.generation,
        next_generation,
        base_last_load_epoch: state.last_load_epoch,
    })
}

fn validate_transfer(
    submitted: &fn64_render_ir::SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
    expected_source: TmemLoadSourceIdentity,
    previous_epoch: Option<TmemLoadEpoch>,
) -> Result<PhysicalTmemLoadBinding, PhysicalTmemError> {
    let load = transfer.load();
    let plan = load
        .transfer_plan()
        .map_err(|_| PhysicalTmemError::DeferredTransfer)?;
    let source = plan.source().identity();
    validate_source_binding(submitted, source)?;
    if source != expected_source {
        return Err(PhysicalTmemError::SubmissionMismatch {
            field: "load source identity",
        });
    }
    if plan.epoch() != load.epoch() {
        return Err(PhysicalTmemError::TransferPlanMismatch {
            field: "load epoch",
        });
    }
    if plan.transfer_words() as usize != transfer.words().len() {
        return Err(PhysicalTmemError::TransferPlanMismatch {
            field: "word count",
        });
    }
    if plan.source().access_count() as usize != transfer.source_accesses().len() {
        return Err(PhysicalTmemError::TransferPlanMismatch {
            field: "source access count",
        });
    }
    if plan.destination().access_count() as usize != transfer.destination_accesses().len() {
        return Err(PhysicalTmemError::TransferPlanMismatch {
            field: "destination access count",
        });
    }
    validate_packet_slice(
        submitted.packet().journal().accesses(),
        plan.source().first_access_index(),
        transfer.source_accesses(),
        "source access slice",
    )?;
    validate_packet_slice(
        submitted.packet().journal().accesses(),
        plan.destination().first_access_index(),
        transfer.destination_accesses(),
        "destination access slice",
    )?;
    validate_physical_plan(transfer.destination_accesses(), transfer.words())?;
    if access_identity(transfer.destination_accesses())?
        != plan.destination().destination_access_identity()
    {
        return Err(PhysicalTmemError::TransferPlanMismatch {
            field: "destination access identity",
        });
    }
    if previous_epoch.is_some_and(|previous| load.epoch().get() <= previous.get()) {
        return Err(PhysicalTmemError::EpochNotNewer {
            previous: previous_epoch,
            actual: load.epoch(),
        });
    }
    Ok(PhysicalTmemLoadBinding {
        identity: mint_load_identity()?,
        source_access_identity: plan.source().source_access_identity(),
        source_first_access_index: plan.source().first_access_index(),
        source_access_count: plan.source().access_count(),
        destination_access_identity: plan.destination().destination_access_identity(),
        destination_first_access_index: plan.destination().first_access_index(),
        destination_access_count: plan.destination().access_count(),
        epoch: load.epoch(),
    })
}

/// Neutral-plan counterpart to [`packet_binding`]; see
/// [`PhysicalTmemState::stage_neutral_transfer`] for why this exists.
/// `transaction_sequence` is the raw-DPC packet's own
/// `WorkloadAdmission::RawDpc { transaction_sequence }` field, which a
/// caller reaches through `RawDpcExecutionView::submitted_packet`'s
/// `&WorkloadPacket` (there is no `admission()` accessor on the neutral
/// `TmemLoadSemantics` itself, exactly as `packet_binding` reads it from
/// `submitted.packet().admission()` rather than from `transfer`).
fn neutral_packet_binding(
    state: &PhysicalTmemState,
    source: TmemLoadSourceIdentity,
    queue: QueueIdentity,
    submission_ordinal: u64,
    transaction_sequence: u64,
) -> Result<PhysicalTmemBinding, PhysicalTmemError> {
    let next_generation = state
        .generation
        .checked_add(1)
        .ok_or(PhysicalTmemError::GenerationExhausted)?;
    Ok(PhysicalTmemBinding {
        state: state.identity,
        transaction: mint_transaction_identity()?,
        source,
        queue,
        submission_ordinal,
        transaction_sequence,
        base_generation: state.generation,
        next_generation,
        base_last_load_epoch: state.last_load_epoch,
    })
}

/// Neutral-plan counterpart to [`validate_transfer`]; see
/// [`PhysicalTmemState::stage_neutral_transfer`] for why this exists.
/// `destination_accesses` is the caller-collected exact ordered slice
/// (see that method's doc comment); this function still cross-checks it
/// against `load`'s own `transfer_words()` physical placement via
/// [`validate_physical_plan`] exactly as the decoder-typed path does via
/// `transfer.destination_accesses()`/`transfer.words()`.
fn neutral_validate_transfer(
    load: &fn64_render::TmemLoadSemantics,
    destination_accesses: &[ResourceAccess],
    // Unlike `validate_transfer`'s `expected_source` (checked against a
    // per-load `TmemLoadSourcePlan::identity()` the decoder path derives
    // independently per load), the neutral path has exactly one source
    // identity per submission -- `stage_neutral_transfer`'s own `source`
    // parameter, threaded through to `neutral_packet_binding` -- and no
    // second, independently derived per-load neutral source identity
    // exists to compare it against; `ExactRawDpcPlanWriter::finish` already
    // proved every pushed command belongs to the one journal this
    // submission's `source` identity was built from before a plan could
    // exist at all (see `docs/DESIGN.md`'s T0 section). Kept as a named
    // parameter (not silently dropped) so a future caller-suppliable
    // per-load neutral source identity, if one is ever added, has an
    // obvious place to plug in a real check.
    _source: TmemLoadSourceIdentity,
    previous_epoch: Option<TmemLoadEpoch>,
) -> Result<PhysicalTmemLoadBinding, PhysicalTmemError> {
    let words: Vec<TmemTransferWord> = load
        .transfer_words()
        .iter()
        .copied()
        .map(neutral_transfer_word)
        .collect();
    validate_physical_plan(destination_accesses, &words)?;
    let epoch = neutral_load_epoch(load.epoch());
    if previous_epoch.is_some_and(|previous| epoch.get() <= previous.get()) {
        return Err(PhysicalTmemError::EpochNotNewer {
            previous: previous_epoch,
            actual: epoch,
        });
    }
    let destination_access_identity = access_identity(destination_accesses)?;
    Ok(PhysicalTmemLoadBinding {
        identity: mint_load_identity()?,
        // Mirrors the decoder-typed path's `plan.source().access_count()` /
        // `source_access_identity()` above: the identity and the count are
        // taken over the load's **whole** ordered source run, not just its
        // first fragment. A partial-width `LoadTile` reads one access per
        // source row, so hard-coding 1 here would hash a 49-row load's
        // source identity over a single row. Both fields feed
        // `proposal_digest`'s per-load projection, so a collapsed count
        // would make a 49-row load's published proposal digest
        // indistinguishable from a one-row load's over the same first row
        // -- pinned by
        // `neutral_source_run_widens_the_binding_count_and_identity`.
        source_access_identity: access_identity(load.sources())?,
        source_first_access_index: load.source_access_index(),
        source_access_count: u16::try_from(load.sources().len())
            .map_err(|_| PhysicalTmemError::SourcePlanMismatch)?,
        destination_access_identity,
        destination_first_access_index: load.destination_access_index(),
        destination_access_count: u16::try_from(destination_accesses.len())
            .map_err(|_| PhysicalTmemError::DestinationPlanMismatch)?,
        epoch,
    })
}

/// Field-for-field conversion from `fn64_render`'s neutral
/// [`fn64_render::NeutralTmemTransferWord`] to this crate's
/// private [`TmemTransferWord`] -- the two are documented mirrors of each
/// other (see `NeutralTmemTransferWord`'s own doc comment in
/// `fn64-render`), so this conversion is a straight field copy, never a
/// recomputation.
fn neutral_transfer_word(word: fn64_render::NeutralTmemTransferWord) -> TmemTransferWord {
    TmemTransferWord::new(
        word.index,
        word.logical_source_offset,
        word.source_access_index,
        word.source_access_byte_offset,
        word.defined_source_byte_mask,
        word.defined_destination_byte_mask,
        word.destination_word,
        word.row_advance,
        word.odd_row_exchange,
        match word.physical {
            fn64_render::NeutralTmemTransferPhysicalWord::Linear(range) => {
                TmemTransferPhysicalWord::Linear(range)
            }
            fn64_render::NeutralTmemTransferPhysicalWord::SplitBanks { low, high } => {
                TmemTransferPhysicalWord::SplitBanks { low, high }
            }
        },
    )
}

fn neutral_load_epoch(epoch: fn64_render::TmemLoadEpoch) -> TmemLoadEpoch {
    TmemLoadEpoch::new(core::num::NonZeroU64::new(epoch.get()).expect("neutral epoch is nonzero"))
}

/// A real minted transaction identity, for `read`'s `#[cfg(test)]`
/// proposal-identity helper. Same mint as a live transaction's, so a test
/// identity is indistinguishable from one a real transaction would carry.
#[cfg(test)]
pub(super) fn next_transaction_identity_for_test() -> PhysicalTmemTransactionIdentity {
    mint_transaction_identity().expect("the test mint is not exhausted")
}

fn mint_transaction_identity() -> Result<PhysicalTmemTransactionIdentity, PhysicalTmemError> {
    NEXT_PHYSICAL_TMEM_TRANSACTION_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map(PhysicalTmemTransactionIdentity)
        .map_err(|_| PhysicalTmemError::TransactionIdentityExhausted)
}

fn mint_load_identity() -> Result<PhysicalTmemLoadIdentity, PhysicalTmemError> {
    NEXT_PHYSICAL_TMEM_LOAD_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map(PhysicalTmemLoadIdentity)
        .map_err(|_| PhysicalTmemError::LoadIdentityExhausted)
}

fn validate_payload_binding(
    actual_packet: PhysicalTmemBinding,
    actual_load: PhysicalTmemLoadBinding,
    expected_packet: PhysicalTmemBinding,
    expected_load: PhysicalTmemLoadBinding,
) -> Result<(), PhysicalTmemError> {
    for (matches, field) in [
        (
            actual_packet.state == expected_packet.state,
            "state identity",
        ),
        (
            actual_packet.source == expected_packet.source,
            "source identity",
        ),
        (
            actual_packet.queue == expected_packet.queue,
            "queue identity",
        ),
        (
            actual_packet.submission_ordinal == expected_packet.submission_ordinal,
            "submission ordinal",
        ),
        (
            actual_packet.transaction_sequence == expected_packet.transaction_sequence,
            "transaction sequence",
        ),
        (
            actual_packet.base_generation == expected_packet.base_generation,
            "base generation",
        ),
        (
            actual_packet.next_generation == expected_packet.next_generation,
            "next generation",
        ),
        (
            actual_packet.base_last_load_epoch == expected_packet.base_last_load_epoch,
            "base load epoch",
        ),
        (
            actual_packet.transaction == expected_packet.transaction,
            "packet transaction identity",
        ),
        (
            actual_load.identity == expected_load.identity,
            "load identity",
        ),
        (
            actual_load.source_access_identity == expected_load.source_access_identity,
            "source access identity",
        ),
        (
            actual_load.source_first_access_index == expected_load.source_first_access_index,
            "source access index",
        ),
        (
            actual_load.source_access_count == expected_load.source_access_count,
            "source access count",
        ),
        (
            actual_load.destination_access_identity == expected_load.destination_access_identity,
            "destination access identity",
        ),
        (
            actual_load.destination_first_access_index
                == expected_load.destination_first_access_index,
            "destination access index",
        ),
        (
            actual_load.destination_access_count == expected_load.destination_access_count,
            "destination access count",
        ),
        (actual_load.epoch == expected_load.epoch, "load epoch"),
    ] {
        if !matches {
            return Err(PhysicalTmemError::PhysicalLanePayloadMismatch { field });
        }
    }
    Ok(())
}

fn validate_source_binding(
    submitted: &fn64_render_ir::SubmittedTicket,
    identity: TmemLoadSourceIdentity,
) -> Result<(), PhysicalTmemError> {
    let packet = submitted.packet();
    for (matches, field) in [
        (
            packet.identity() == identity.workload(),
            "workload identity",
        ),
        (
            packet.journal().identity() == identity.journal(),
            "journal identity",
        ),
        (
            submitted.identity() == identity.submission(),
            "submission identity",
        ),
        (
            packet.memory_layout() == identity.memory_layout(),
            "memory layout",
        ),
    ] {
        if !matches {
            return Err(PhysicalTmemError::SubmissionMismatch { field });
        }
    }
    Ok(())
}

fn validate_packet_slice(
    packet: &[ResourceAccess],
    first: u32,
    expected: &[ResourceAccess],
    field: &'static str,
) -> Result<(), PhysicalTmemError> {
    let start =
        usize::try_from(first).map_err(|_| PhysicalTmemError::TransferPlanMismatch { field })?;
    let end = start
        .checked_add(expected.len())
        .ok_or(PhysicalTmemError::TransferPlanMismatch { field })?;
    if packet.get(start..end) != Some(expected) {
        return Err(PhysicalTmemError::TransferPlanMismatch { field });
    }
    Ok(())
}

fn access_identity(accesses: &[ResourceAccess]) -> Result<JournalIdentity, PhysicalTmemError> {
    let total_bytes = accesses.iter().try_fold(0_u32, |total, access| {
        total.checked_add(access.region().declared_bytes())
    });
    let total_bytes = total_bytes.ok_or(PhysicalTmemError::DestinationPlanMismatch)?;
    let limits = ResourceJournalLimits::try_new(accesses.len(), total_bytes)?;
    Ok(ResourceJournal::try_new(limits, accesses.to_vec())?.identity())
}

fn validate_physical_plan(
    destination_accesses: &[ResourceAccess],
    words: &[TmemTransferWord],
) -> Result<(), PhysicalTmemError> {
    if destination_accesses.is_empty() || words.is_empty() {
        return Err(PhysicalTmemError::DestinationPlanMismatch);
    }
    let mut declared = [false; TMEM_LEN];
    let mut previous_end = 0;
    for (index, access) in destination_accesses.iter().enumerate() {
        let ResourceRegion::Tmem(range) = access.region() else {
            return Err(PhysicalTmemError::DestinationPlanMismatch);
        };
        if index != 0 && range.start() <= previous_end {
            return Err(PhysicalTmemError::DestinationPlanMismatch);
        }
        previous_end = range.end();
        for address in range.start()..range.end() {
            declared[address as usize] = true;
        }
    }

    let mut touched = [false; TMEM_LEN];
    for (index, word) in words.iter().copied().enumerate() {
        if usize::from(word.index()) != index {
            return Err(PhysicalTmemError::DestinationPlanMismatch);
        }
        physical_defined_lane_mask(word)?;
        for address in fragment_lanes(word.physical())? {
            touched[address] = true;
        }
    }
    if touched != declared {
        return Err(PhysicalTmemError::DestinationPlanMismatch);
    }
    Ok(())
}

const fn mask_is_low_prefix(mask: u8) -> bool {
    mask != 0 && (mask & mask.wrapping_add(1)) == 0
}

/// Converts only the accepted word's defined-lane shape. Actual logical
/// source bytes are never inspected or rearranged here; M4.2b's LoadTile and
/// M4.2c's LoadBlock executors own that source-to-physical payload mapping.
///
/// The physical lane mask is derived from `defined_destination_byte_mask`,
/// not `defined_source_byte_mask`: those two coincide for Block/Tile (every
/// defined destination byte is copied one-for-one from its own captured
/// source byte) but diverge for TLUT, where 2 captured source bytes are
/// quadricated into 8 defined destination bytes. Using the source mask here
/// for TLUT would falsely reject its correct 8-lane payload and, if the check
/// were loosened to accept a 2-lane payload instead, would mark 6 of 8 real
/// destination bytes invalid -- contradicting `undefined_padding_bytes() ==
/// 0` for `Tlut`. Linear odd rows exchange their two four-byte halves, so an
/// odd-row two-byte tail occupies mask `0x30` rather than the logical-prefix
/// mask `0x03`. Split-bank lane order is low[0..4] followed by high[0..4], so
/// one four-byte RGBA32 texel occupies mask 0x33.
fn physical_defined_lane_mask(word: TmemTransferWord) -> Result<u8, PhysicalTmemError> {
    let source_mask = word.defined_source_byte_mask();
    let destination_mask = word.defined_destination_byte_mask();
    if !mask_is_low_prefix(source_mask) || !mask_is_low_prefix(destination_mask) {
        return Err(PhysicalTmemError::DestinationPlanMismatch);
    }
    if destination_mask.count_ones() < source_mask.count_ones() {
        return Err(PhysicalTmemError::DestinationPlanMismatch);
    }
    match word.physical() {
        TmemTransferPhysicalWord::Linear(_) => Ok(if word.odd_row_exchange() {
            destination_mask.rotate_left(4)
        } else {
            destination_mask
        }),
        TmemTransferPhysicalWord::SplitBanks { .. } => {
            const SOURCE_TO_PHYSICAL_LANE: [u8; 8] = [0, 1, 4, 5, 2, 3, 6, 7];
            let mut physical_mask = 0_u8;
            for (source_lane, physical_lane) in SOURCE_TO_PHYSICAL_LANE.into_iter().enumerate() {
                if destination_mask & (1 << source_lane) != 0 {
                    physical_mask |= 1 << physical_lane;
                }
            }
            Ok(physical_mask)
        }
    }
}

fn fragment_lanes(physical: TmemTransferPhysicalWord) -> Result<[usize; 8], PhysicalTmemError> {
    match physical {
        TmemTransferPhysicalWord::Linear(range) if range.len() == 8 => {
            let start = range.start() as usize;
            Ok(core::array::from_fn(|lane| start + lane))
        }
        TmemTransferPhysicalWord::SplitBanks { low, high } if low.len() == 4 && high.len() == 4 => {
            Ok(core::array::from_fn(|lane| {
                if lane < 4 {
                    low.start() as usize + lane
                } else {
                    high.start() as usize + lane - 4
                }
            }))
        }
        _ => Err(PhysicalTmemError::InvalidPhysicalFragment),
    }
}

fn project_load(
    load_binding: PhysicalTmemLoadBinding,
    bytes: &[u8; TMEM_LEN],
    valid: &[bool; TMEM_LEN],
    last_touched_generation: &[u64; TMEM_LEN],
    accesses: &[ResourceAccess],
) -> Result<(LoadProjection, Vec<CompletedWrite>), PhysicalTmemError> {
    let mut projections = Vec::with_capacity(accesses.len());
    let mut effects = Vec::with_capacity(accesses.len());
    for access in accesses.iter().copied() {
        let ResourceRegion::Tmem(range) = access.region() else {
            return Err(PhysicalTmemError::DestinationPlanMismatch);
        };
        let start = range.start() as usize;
        let end = range.end() as usize;
        let projection = CanonicalTmemEffectProjection {
            access,
            load_epoch: load_binding.epoch,
            normalized_bytes: (start..end)
                .map(|address| if valid[address] { bytes[address] } else { 0 })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            validity: (start..end)
                .map(|address| u8::from(valid[address]))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            last_touched_generation: last_touched_generation[start..end]
                .to_vec()
                .into_boxed_slice(),
        };
        effects.push(completed_effect(&projection)?);
        projections.push(projection);
    }
    Ok((
        LoadProjection {
            binding: load_binding,
            accesses: projections.into_boxed_slice(),
        },
        effects,
    ))
}

/// The byte count is complete declared physical access coverage, including
/// invalid tail lanes. The content preimage contains only byte count, load
/// epoch, normalized bytes, validity, and per-byte touch generations; access
/// and lifecycle/allocation identities are bound outside `content`. Render-IR
/// owns the frozen renderer-neutral domain and encoding for this preimage.
fn completed_effect(
    projection: &CanonicalTmemEffectProjection,
) -> Result<CompletedWrite, PhysicalTmemError> {
    let ResourceRegion::Tmem(range) = projection.access.region() else {
        return Err(PhysicalTmemError::DestinationPlanMismatch);
    };
    let len = range.len() as usize;
    if projection.normalized_bytes.len() != len
        || projection.validity.len() != len
        || projection.last_touched_generation.len() != len
        || projection.validity.iter().any(|valid| *valid > 1)
        || projection
            .normalized_bytes
            .iter()
            .zip(&projection.validity)
            .any(|(byte, valid)| *valid == 0 && *byte != 0)
    {
        return Err(PhysicalTmemError::ProposalMismatch);
    }
    let byte_count = range.len();
    let digest = CompletedWrite::physical_tmem_content_digest(
        byte_count,
        projection.load_epoch.get(),
        &projection.normalized_bytes,
        &projection.validity,
        &projection.last_touched_generation,
    );
    CompletedWrite::try_new(projection.access, byte_count, digest).map_err(PhysicalTmemError::Ir)
}

fn proposal_identity(
    binding: PhysicalTmemBinding,
    last_load_epoch: Option<TmemLoadEpoch>,
    projections: &[LoadProjection],
    effects: &[CompletedWrite],
) -> ContentDigest {
    let state = binding.state.get().to_be_bytes();
    let transaction = binding.transaction.get().to_be_bytes();
    let queue = binding.queue.get().to_be_bytes();
    let submission_ordinal = binding.submission_ordinal.to_be_bytes();
    let transaction_sequence = binding.transaction_sequence.to_be_bytes();
    let memory_layout = binding.source.memory_layout().bytes().to_be_bytes();
    let base_generation = binding.base_generation.to_be_bytes();
    let next_generation = binding.next_generation.to_be_bytes();
    let base_epoch = binding
        .base_last_load_epoch
        .map_or(0, TmemLoadEpoch::get)
        .to_be_bytes();
    let last_epoch = last_load_epoch.map_or(0, TmemLoadEpoch::get).to_be_bytes();
    let load_count = (projections.len() as u32).to_be_bytes();
    let effect_count = (effects.len() as u32).to_be_bytes();
    let mut load_projection = Vec::with_capacity(projections.len() * 48);
    for load in projections {
        load_projection.extend_from_slice(&load.binding.identity.0.to_be_bytes());
        load_projection.extend_from_slice(load.binding.source_access_identity.as_bytes().as_ref());
        load_projection.extend_from_slice(&load.binding.source_first_access_index.to_be_bytes());
        load_projection.extend_from_slice(&load.binding.source_access_count.to_be_bytes());
        load_projection
            .extend_from_slice(load.binding.destination_access_identity.as_bytes().as_ref());
        load_projection
            .extend_from_slice(&load.binding.destination_first_access_index.to_be_bytes());
        load_projection.extend_from_slice(&load.binding.destination_access_count.to_be_bytes());
        load_projection.extend_from_slice(&load.binding.epoch.get().to_be_bytes());
    }
    let mut effect_projection = Vec::with_capacity(effects.len() * 44);
    for effect in effects {
        effect_projection.extend_from_slice(&effect.access().operation().get().to_be_bytes());
        effect_projection.extend_from_slice(&effect.byte_count().to_be_bytes());
        effect_projection.extend_from_slice(effect.content().as_ref());
    }
    ContentDigest::hash(
        PROPOSAL_DOMAIN,
        &[
            &state,
            &transaction,
            &queue,
            &submission_ordinal,
            &transaction_sequence,
            binding.source.workload().as_bytes().as_ref(),
            binding.source.journal().as_bytes().as_ref(),
            binding.source.submission().digest().as_bytes().as_ref(),
            &memory_layout,
            &base_generation,
            &next_generation,
            &base_epoch,
            &last_epoch,
            &load_count,
            &load_projection,
            &effect_count,
            &effect_projection,
        ],
    )
}

fn validate_proposal(pending: &PendingTmemTransaction) -> Result<(), PhysicalTmemError> {
    let mut recomputed = Vec::new();
    for load in pending.projections.iter() {
        if load.accesses.len() != usize::from(load.binding.destination_access_count) {
            return Err(PhysicalTmemError::ProposalMismatch);
        }
        let accesses = load
            .accesses
            .iter()
            .map(|projection| projection.access)
            .collect::<Vec<_>>();
        if access_identity(&accesses)? != load.binding.destination_access_identity {
            return Err(PhysicalTmemError::ProposalMismatch);
        }
        for projection in load.accesses.iter() {
            if projection.load_epoch != load.binding.epoch {
                return Err(PhysicalTmemError::ProposalMismatch);
            }
            recomputed.push(completed_effect(projection)?);
        }
    }
    if pending.last_load_epoch != pending.projections.last().map(|load| load.binding.epoch)
        || recomputed != pending.effects.as_ref()
        || proposal_identity(
            pending.binding,
            pending.last_load_epoch,
            &pending.projections,
            &recomputed,
        ) != pending.proposal_identity
    {
        return Err(PhysicalTmemError::ProposalMismatch);
    }
    Ok(())
}

fn validate_gpu(
    complete: &GpuCompleteTicket,
    binding: PhysicalTmemBinding,
    proposed: &[CompletedWrite],
) -> Result<(), PhysicalTmemError> {
    let packet = complete.packet();
    for (matches, field) in [
        (complete.queue() == binding.queue, "queue identity"),
        (
            complete.submission() == binding.source.submission(),
            "submission identity",
        ),
        (
            complete.ordinal() == binding.submission_ordinal,
            "submission ordinal",
        ),
        (
            packet.identity() == binding.source.workload(),
            "workload identity",
        ),
        (
            packet.journal().identity() == binding.source.journal(),
            "journal identity",
        ),
        (
            packet.memory_layout() == binding.source.memory_layout(),
            "memory layout",
        ),
        (
            matches!(
                packet.admission(),
                WorkloadAdmission::RawDpc { transaction_sequence }
                    if transaction_sequence == binding.transaction_sequence
            ),
            "transaction sequence",
        ),
    ] {
        if !matches {
            return Err(PhysicalTmemError::GpuCompletionMismatch { field });
        }
    }

    let mut backend_cursor = 0;
    for expected in proposed.iter().copied() {
        let matching = complete.backend_writes()[backend_cursor..]
            .iter()
            .position(|actual| actual.access() == expected.access())
            .map(|offset| backend_cursor + offset)
            .ok_or(PhysicalTmemError::GpuCompletionMismatch {
                field: "missing proposed write",
            })?;
        if complete.backend_writes()[matching] != expected {
            return Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "proposed write content",
            });
        }
        if complete.backend_writes()[matching + 1..]
            .iter()
            .any(|actual| actual.access() == expected.access())
        {
            return Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "duplicate proposed access",
            });
        }
        backend_cursor = matching + 1;
    }
    Ok(())
}

/// Every proposed write must appear in `reported` **in the same relative
/// order** with identical content -- no reorder, no substitution, no missing
/// or duplicated proposed write. `BackendEffectReport` exposes no
/// queue/submission/workload identity to cross-check here; its own `try_new`
/// already proved `reported` is exactly the write set its packet's journal
/// declares, in that journal's own order.
///
/// **Order-preserving subsequence, not whole-list equality.** This method's
/// own doc header already promised that "other declared backend writes may
/// appear around the TMEM writes"; until the mixed fill+TMEM card, this
/// function contradicted that promise with a positional `zip` over equal
/// lengths, and no in-tree caller produced a mixed report to expose the
/// contradiction. A packet interleaving an admitted `FillRectangle`'s
/// `RenderTarget` writes with this transaction's `TmemLoadDestination`
/// writes reports both, in the journal's own order; only the TMEM subset is
/// this transaction's to vouch for, and the fill's writes are neither
/// proposed here nor absent from the report.
///
/// This is exactly the walk `validate_gpu` above already performs against
/// `GpuCompleteTicket::backend_writes` for the same proposed list -- the two
/// now agree on what "the report contains my proposed writes" means, where
/// before they disagreed by the strictness of one `zip`.
///
/// Relaxing the length equality does NOT relax the rejection: a report built
/// for a genuinely different transaction still fails, now by name
/// (`missing proposed write`) rather than by count. Nothing that was
/// rejected becomes accepted -- a superset in journal order is admitted, an
/// omission, a reorder, a substitution and a duplicate are each still a
/// named error.
fn validate_backend_effects(
    reported: &[CompletedWrite],
    proposed: &[CompletedWrite],
) -> Result<(), PhysicalTmemError> {
    if reported.len() < proposed.len() {
        return Err(PhysicalTmemError::BackendEffectMismatch {
            field: "write count",
        });
    }
    let mut cursor = 0;
    for expected in proposed.iter().copied() {
        let matching = reported[cursor..]
            .iter()
            .position(|actual| actual.access() == expected.access())
            .map(|offset| cursor + offset)
            .ok_or(PhysicalTmemError::BackendEffectMismatch {
                field: "missing proposed write",
            })?;
        if reported[matching] != expected {
            return Err(PhysicalTmemError::BackendEffectMismatch {
                field: "write content",
            });
        }
        if reported[matching + 1..]
            .iter()
            .any(|actual| actual.access() == expected.access())
        {
            return Err(PhysicalTmemError::BackendEffectMismatch {
                field: "duplicate proposed access",
            });
        }
        cursor = matching + 1;
    }
    Ok(())
}

fn validate_guest(
    guest: &GuestCommittedTicket,
    binding: PhysicalTmemBinding,
    projections: &[LoadProjection],
) -> Result<(), PhysicalTmemError> {
    let packet = guest.packet();
    for (matches, field) in [
        (guest.queue() == binding.queue, "queue identity"),
        (
            guest.submission() == binding.source.submission(),
            "submission identity",
        ),
        (
            guest.ordinal() == binding.submission_ordinal,
            "submission ordinal",
        ),
        (
            packet.identity() == binding.source.workload(),
            "workload identity",
        ),
        (
            packet.journal().identity() == binding.source.journal(),
            "journal identity",
        ),
        (
            packet.memory_layout() == binding.source.memory_layout(),
            "memory layout",
        ),
        (
            matches!(
                packet.admission(),
                WorkloadAdmission::RawDpc { transaction_sequence }
                    if transaction_sequence == binding.transaction_sequence
            ),
            "transaction sequence",
        ),
    ] {
        if !matches {
            return Err(PhysicalTmemError::GuestCommitMismatch { field });
        }
    }
    for load in projections {
        let accesses = load
            .accesses
            .iter()
            .map(|projection| projection.access)
            .collect::<Vec<_>>();
        if accesses.len() != usize::from(load.binding.destination_access_count) {
            return Err(PhysicalTmemError::GuestCommitMismatch {
                field: "destination access count",
            });
        }
        let start = usize::try_from(load.binding.destination_first_access_index).map_err(|_| {
            PhysicalTmemError::GuestCommitMismatch {
                field: "destination access index",
            }
        })?;
        let end =
            start
                .checked_add(accesses.len())
                .ok_or(PhysicalTmemError::GuestCommitMismatch {
                    field: "destination access slice",
                })?;
        if packet.journal().accesses().get(start..end) != Some(accesses.as_slice()) {
            return Err(PhysicalTmemError::GuestCommitMismatch {
                field: "destination access slice",
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        AccessMode, BackendCompletionAuthority, BackendEffectReport, CapturedGuestRead,
        DecodedTicket, DeferredGuestReadCapture, DpInterruptState, DramCommandChunk,
        DramCommandStream, GuestCommitAuthority, GuestCommitEffectReport, OperationId,
        PhysicalMemoryLayout, RawCommandStream, RdramResource, ResourceJournal,
        ResourceJournalLimits, ResourceRegion, SubmissionQueue, TemporalBoundary,
        TicketAuthoritySet, TmemRange, WorkloadPacket, WorkloadPacketPreflight,
        MAX_RESOURCE_ACCESSES,
    };

    use super::super::{LOAD_SYNC, LOAD_TILE, LOAD_TLUT, SET_TEXTURE_IMAGE, SET_TILE};
    use super::*;
    use crate::{decode_raw_dpc, RawDpcCommandKind, RawDpcDecodeError, RdpState};

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
    }

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    /// Builds a neutral [`fn64_render::TmemLoadSemantics`] whose source run
    /// is `source_rows` accesses wide, each reading 8 bytes, with one
    /// transfer word per row bound to its own row.
    fn neutral_load_with_source_rows(
        layout: PhysicalMemoryLayout,
        source_rows: u32,
    ) -> fn64_render::TmemLoadSemantics {
        use fn64_render::{
            NeutralImageFormat, NeutralPixelSize, NeutralTextureImage, NeutralTileAddressMode,
            NeutralTileDescriptor, NeutralTileSize, NeutralTmemTransferPhysicalWord,
            NeutralTmemTransferWord, RawDpcCommandLocation, TmemLoadEpoch, TmemLoadKind,
            TmemTransferLayout,
        };

        // One 8-byte source access per row, spaced 16 bytes apart so the
        // rows are disjoint and non-adjacent -- the real partial-width
        // `LoadTile` shape, which cannot collapse to one range.
        let sources: Vec<ResourceAccess> = (0..source_rows)
            .map(|row| {
                let start = 0x200 + row * 16;
                ResourceAccess::try_new(
                    OperationId::new(1 + row),
                    AccessMode::Read,
                    fn64_render_ir::AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: layout.range(start, start + 8).unwrap(),
                    },
                )
                .unwrap()
            })
            .collect();
        let destination = ResourceAccess::try_new(
            OperationId::new(1 + source_rows),
            AccessMode::Write,
            fn64_render_ir::AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(0, source_rows * 8).unwrap()),
        )
        .unwrap();
        let transfer_words: Vec<NeutralTmemTransferWord> = (0..source_rows)
            .map(|row| NeutralTmemTransferWord {
                index: row as u16,
                logical_source_offset: row * 8,
                source_access_index: 1 + row,
                source_access_byte_offset: 0,
                defined_source_byte_mask: 0xff,
                defined_destination_byte_mask: 0xff,
                destination_word: row as u16,
                row_advance: 0,
                odd_row_exchange: false,
                physical: NeutralTmemTransferPhysicalWord::Linear(
                    TmemRange::try_new(row * 8, row * 8 + 8).unwrap(),
                ),
            })
            .collect();

        fn64_render::TmemLoadSemantics::new(
            RawDpcCommandLocation {
                command_index: 0,
                stream_index: 0,
                chunk_index: 0,
                source_address: layout.address(COMMAND_START).unwrap(),
                source_byte_offset: 0,
                source_byte_len: 8,
                wire_opcode: 0xf4,
            },
            vec![0xf400_0000, 0],
            TmemLoadEpoch::new(core::num::NonZeroU64::new(1).unwrap()),
            TmemLoadKind::Tile {
                bounds: NeutralTileSize {
                    low_s: 0,
                    low_t: 0,
                    high_s: 3,
                    high_t: (source_rows - 1) as u16,
                },
            },
            0,
            NeutralTextureImage {
                format: NeutralImageFormat::Rgba,
                size: NeutralPixelSize::Bits16,
                width: 8,
                address: layout.address(0x200).unwrap(),
            },
            NeutralTileDescriptor {
                format: NeutralImageFormat::Rgba,
                size: NeutralPixelSize::Bits16,
                line_words: 1,
                tmem_word_address: 0,
                palette: 0,
                s_mode: NeutralTileAddressMode::default(),
                mask_s: 0,
                shift_s: 0,
                t_mode: NeutralTileAddressMode::default(),
                mask_t: 0,
                shift_t: 0,
            },
            sources,
            1,
            destination,
            1 + source_rows,
            source_rows * 8,
            0,
            1,
            source_rows as u16,
            TmemTransferLayout::Linear,
            transfer_words,
        )
    }

    /// The neutral path's physical binding must take its source identity
    /// and count over the load's **whole** ordered source run, exactly as
    /// the decoder-typed path takes `plan.source().access_count()`.
    ///
    /// Both fields feed `proposal_digest`'s per-load projection, so
    /// hard-coding the count to 1 would let a many-row load publish the
    /// same proposal digest as a one-row load reading the same first row.
    /// This asserts the count widens AND that the two digests actually
    /// differ -- the second half is what makes the first non-cosmetic.
    #[test]
    fn neutral_source_run_widens_the_binding_count_and_identity() {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let wide = neutral_load_with_source_rows(layout, 8);
        let narrow = neutral_load_with_source_rows(layout, 1);

        assert_eq!(wide.sources().len(), 8);
        assert_eq!(narrow.sources().len(), 1);
        assert_eq!(
            wide.sources()[0],
            narrow.sources()[0],
            "both loads read the same first row, so only the run width can distinguish them"
        );

        let wide_identity = access_identity(wide.sources()).unwrap();
        let narrow_identity = access_identity(narrow.sources()).unwrap();
        assert_ne!(
            wide_identity, narrow_identity,
            "an 8-row source run must not hash to the same identity as its own first row"
        );
        assert_eq!(
            access_identity(core::slice::from_ref(&wide.source())).unwrap(),
            narrow_identity,
            "positive control: hashing only the first fragment IS the collapsed identity, \
             which is exactly the bug this test forbids"
        );

        // Now drive the real binding builder. `neutral_validate_transfer`
        // ignores the identity argument (see its `_source` parameter's own
        // comment), so any well-formed one serves.
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue
            .submit(DecodedTicket::new(planned_packet(1)))
            .unwrap();
        let identity = TmemLoadSourceIdentity::new(
            submitted.packet().identity(),
            submitted.packet().journal().identity(),
            submitted.identity(),
            submitted.packet().memory_layout(),
        );

        let wide_destination = [ResourceAccess::try_new(
            OperationId::new(9),
            AccessMode::Write,
            fn64_render_ir::AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(0, 64).unwrap()),
        )
        .unwrap()];
        let wide_binding =
            neutral_validate_transfer(&wide, &wide_destination, identity, None).unwrap();

        assert_eq!(
            wide_binding.source_access_count, 8,
            "the binding's source_access_count must be the whole run's width, not 1"
        );
        assert_eq!(
            wide_binding.source_access_identity, wide_identity,
            "the binding's source identity must be hashed over the whole run"
        );
        assert_ne!(
            wide_binding.source_access_identity, narrow_identity,
            "a collapsed identity would make an 8-row load indistinguishable from a 1-row one"
        );
        assert_eq!(wide_binding.source_first_access_index, 1);
    }


    fn load_tile_words(load_count: usize) -> Vec<u32> {
        let mut words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19 | 4),
            0x200,
            word(SET_TILE, 2 << 19 | 3 << 9),
            7 << 24,
        ];
        for _ in 0..load_count {
            words.extend([
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 4),
                7 << 24 | 16 << 12 | 8,
            ]);
        }
        words
    }

    fn command_access(
        layout: PhysicalMemoryLayout,
        byte_count: u32,
        operation: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: layout
                    .range(COMMAND_START, COMMAND_START + byte_count)
                    .unwrap(),
            },
        )
        .unwrap()
    }

    fn source_access_range(
        layout: PhysicalMemoryLayout,
        operation: u32,
        start: u32,
        end: u32,
    ) -> ResourceAccess {
        ResourceAccess::try_new(
            OperationId::new(operation),
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: layout.range(start, end).unwrap(),
            },
        )
        .unwrap()
    }

    fn finalize_packet(words: &[u32], accesses: Vec<ResourceAccess>) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                layout
                    .range(COMMAND_START, COMMAND_START + byte_count)
                    .unwrap(),
                words.to_vec(),
                TemporalBoundary::new(1, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap()])
            .unwrap(),
        );
        let declared = accesses
            .iter()
            .map(|access| access.region().declared_bytes())
            .sum::<u32>();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(MAX_RESOURCE_ACCESSES, declared).unwrap(),
            accesses,
        )
        .unwrap();
        let preflight = WorkloadPacketPreflight::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence: 7,
            },
            vec![stream],
            journal,
        )
        .unwrap();
        let capture = DeferredGuestReadCapture::new(
            preflight
                .guest_read_plan()
                .reads()
                .iter()
                .map(|read| {
                    CapturedGuestRead::try_new(
                        *read,
                        vec![read.operation().get() as u8; read.range().len() as usize],
                    )
                    .unwrap()
                })
                .collect(),
        );
        preflight.finalize(capture).unwrap()
    }

    fn planned_packet(load_count: usize) -> WorkloadPacket {
        let words = load_tile_words(load_count);
        let source_ranges = vec![(0x20a, 0x21e); load_count];
        planned_packet_with_sources(words, &source_ranges)
    }

    fn planned_packet_with_sources(
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        let mut probe_accesses = vec![command_access(layout, byte_count, 0)];
        probe_accesses.extend(source_ranges.iter().copied().enumerate().map(
            |(index, (start, end))| source_access_range(layout, index as u32 + 1, start, end),
        ));
        let probe = finalize_packet(&words, probe_accesses);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(probe)).unwrap();
        let expected = match decode_raw_dpc(submitted, &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            other => panic!("planning probe did not request the exact journal: {other:?}"),
        };
        finalize_packet(&words, expected)
    }

    fn fixture(load_count: usize) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        Fixture {
            decoded: decode_with(&mut queue, planned_packet(load_count)),
            backend,
            guest,
        }
    }

    fn decode_with(queue: &mut SubmissionQueue, packet: WorkloadPacket) -> crate::DecodedRawDpc {
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        decode_raw_dpc(submitted, &RdpState::default()).unwrap()
    }

    fn load(decoded: &crate::DecodedRawDpc, ordinal: usize) -> super::super::TmemLoad {
        let command_index = 3 + ordinal * 2;
        match decoded.commands()[command_index].kind() {
            RawDpcCommandKind::LoadTile(load) | RawDpcCommandKind::LoadBlock(load) => load,
            _ => panic!("expected physical TMEM load at command {command_index}"),
        }
    }

    fn finish_words(
        mut staged: StagedTmemTransaction,
        first_byte: u8,
    ) -> PhysicalTmemPacketTransaction {
        let words = staged.expected_words().to_vec();
        for word in words {
            let physical_lanes = defined_physical_lanes(word, first_byte);
            let payload = staged.physical_word_payload(word, physical_lanes).unwrap();
            staged.stage_word(payload).unwrap();
        }
        staged.finish_load().unwrap()
    }

    fn defined_physical_lanes(word: TmemTransferWord, first_byte: u8) -> [Option<u8>; 8] {
        let mask = physical_defined_lane_mask(word).unwrap();
        let mut defined_ordinal = 0_u8;
        core::array::from_fn(|lane| {
            if mask & (1 << lane) == 0 {
                None
            } else {
                let byte = first_byte
                    .wrapping_add(word.index() as u8 * 8)
                    .wrapping_add(defined_ordinal);
                defined_ordinal = defined_ordinal.wrapping_add(1);
                Some(byte)
            }
        })
    }

    fn stage_all(
        state: &PhysicalTmemState,
        decoded: &crate::DecodedRawDpc,
        first_bytes: &[u8],
    ) -> PendingTmemTransaction {
        let first_load = load(decoded, 0);
        let first_transfer = decoded
            .resource_plan()
            .bind_tmem_transfer(first_load)
            .unwrap();
        let first = state
            .stage_transfer(decoded.submitted(), &first_transfer)
            .unwrap();
        let mut packet = finish_words(first, first_bytes[0]);
        for (ordinal, first_byte) in first_bytes.iter().copied().enumerate().skip(1) {
            let next_load = load(decoded, ordinal);
            let next_transfer = decoded
                .resource_plan()
                .bind_tmem_transfer(next_load)
                .unwrap();
            let staged = packet
                .stage_transfer(decoded.submitted(), &next_transfer)
                .unwrap();
            packet = finish_words(staged, first_byte);
        }
        packet.into_pending().unwrap()
    }

    fn gpu_complete(
        decoded: crate::DecodedRawDpc,
        mut backend: BackendCompletionAuthority,
        effects: Vec<CompletedWrite>,
    ) -> GpuCompleteTicket {
        let report = BackendEffectReport::try_new(decoded.submitted().packet(), effects).unwrap();
        let receipt = backend.issue(decoded.submitted(), report).unwrap();
        let submitted = decoded.into_contract_parts().submitted;
        submitted.gpu_complete(receipt).unwrap()
    }

    fn guest_commit(
        complete: GpuCompleteTicket,
        mut guest: GuestCommitAuthority,
    ) -> GuestCommittedTicket {
        let effects = GuestCommitEffectReport::try_new(&complete, Vec::new()).unwrap();
        let receipt = guest.issue(&complete, effects).unwrap();
        complete.commit_guest(receipt).unwrap()
    }

    #[test]
    fn complete_words_publish_validity_epoch_and_preserve_undefined_backing() {
        let mut state = PhysicalTmemState::try_new().unwrap();
        state.bytes.fill(0xaa);
        let fixture = fixture(1);
        let load = load(&fixture.decoded, 0);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load)
            .unwrap();
        let words = transfer.words().to_vec();
        let pending = stage_all(&state, &fixture.decoded, &[0x20]);
        let proposed = pending.proposed_effects().to_vec();

        assert_eq!(state.generation(), 0);
        assert_eq!(state.last_load_epoch(), None);
        assert!(state.valid_byte(0).is_none());

        let complete = gpu_complete(fixture.decoded, fixture.backend, proposed.clone());
        let gpu_bound = pending.bind_gpu(&complete).unwrap();
        let guest = guest_commit(complete, fixture.guest);
        let committed = state
            .publication_authority()
            .publish(gpu_bound, guest)
            .unwrap();

        assert_eq!(state.generation(), 1);
        assert_eq!(state.last_load_epoch(), Some(load.epoch()));
        assert_eq!(committed.completed_effects(), proposed);
        let mut defined_ordinal = 0_u8;
        let mut touched = [false; TMEM_LEN];
        for word in words {
            for (lane, address) in fragment_lanes(word.physical())
                .unwrap()
                .into_iter()
                .enumerate()
            {
                touched[address] = true;
                assert_eq!(state.last_touched_generation[address], 1);
                if physical_defined_lane_mask(word).unwrap() & (1 << lane) != 0 {
                    let expected = 0x20_u8
                        .wrapping_add(word.index() as u8 * 8)
                        .wrapping_add(defined_ordinal);
                    assert_eq!(state.bytes[address], expected);
                    assert_eq!(state.valid_byte(address as u16), Some(expected));
                    defined_ordinal = defined_ordinal.wrapping_add(1);
                } else {
                    assert_eq!(state.bytes[address], 0xaa);
                    assert!(!state.byte_is_valid(address as u16));
                    assert_eq!(state.valid_byte(address as u16), None);
                }
            }
            defined_ordinal = 0;
        }
        let untouched = touched.iter().position(|is_touched| !is_touched).unwrap();
        assert_eq!(state.last_touched_generation[untouched], 0);
        assert_eq!(state.bytes[untouched], 0xaa);
    }

    #[test]
    fn odd_width_rgba32_tail_uses_split_bank_physical_mask() {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 3 << 19),
            0x200,
            word(SET_TILE, 2 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24,
        ];
        let packet = planned_packet_with_sources(words, &[(0x200, 0x204)]);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let decoded = decode_with(&mut queue, packet);
        let load = load(&decoded, 0);
        let transfer = decoded.resource_plan().bind_tmem_transfer(load).unwrap();
        assert_eq!(transfer.words().len(), 1);
        let word = transfer.words()[0];
        assert_eq!(word.defined_source_byte_mask(), 0x0f);
        assert_eq!(physical_defined_lane_mask(word).unwrap(), 0x33);
        assert!(matches!(
            word.physical(),
            TmemTransferPhysicalWord::SplitBanks { .. }
        ));

        let mut state = PhysicalTmemState::try_new().unwrap();
        state.bytes.fill(0xaa);
        let mut staged = state
            .stage_transfer(decoded.submitted(), &transfer)
            .unwrap();
        let logical_prefix = [
            Some(0x10),
            Some(0x11),
            Some(0x12),
            Some(0x13),
            None,
            None,
            None,
            None,
        ];
        assert!(matches!(
            staged.physical_word_payload(word, logical_prefix),
            Err(PhysicalTmemError::PhysicalLaneMaskMismatch {
                expected: 0x33,
                actual: 0x0f,
                ..
            })
        ));
        let physical = [
            Some(0x10),
            Some(0x11),
            None,
            None,
            Some(0x12),
            Some(0x13),
            None,
            None,
        ];
        let payload = staged.physical_word_payload(word, physical).unwrap();
        staged.stage_word(payload).unwrap();
        let candidate = staged.finish_load().unwrap();
        let lanes = fragment_lanes(word.physical()).unwrap();
        for (lane, address) in lanes.into_iter().enumerate() {
            assert_eq!(candidate.last_touched_generation[address], 1);
            match physical[lane] {
                Some(expected) => {
                    assert_eq!(candidate.bytes[address], expected);
                    assert!(candidate.valid[address]);
                }
                None => {
                    assert_eq!(candidate.bytes[address], 0xaa);
                    assert!(!candidate.valid[address]);
                }
            }
        }
        let pending = candidate.into_pending().unwrap();
        assert_eq!(pending.proposed_effects().len(), 2);
        assert!(pending
            .proposed_effects()
            .iter()
            .all(|effect| effect.byte_count() == 4));
    }

    #[test]
    fn linear_partial_tail_masks_follow_even_and_odd_row_lane_exchange() {
        let range = fn64_render_ir::TmemRange::try_new(0, 8).unwrap();
        let even_masks = [0x01, 0x03, 0x07, 0x0f, 0x1f, 0x3f, 0x7f, 0xff];
        let odd_masks = [0x10, 0x30, 0x70, 0xf0, 0xf1, 0xf3, 0xf7, 0xff];
        for (defined_bytes, (even, odd)) in even_masks.into_iter().zip(odd_masks).enumerate() {
            let source_mask = if defined_bytes == 7 {
                u8::MAX
            } else {
                ((1_u16 << (defined_bytes + 1)) - 1) as u8
            };
            for (odd_row_exchange, expected) in [(false, even), (true, odd)] {
                let word = TmemTransferWord::new(
                    0,
                    0,
                    0,
                    0,
                    source_mask,
                    source_mask,
                    0,
                    0,
                    odd_row_exchange,
                    TmemTransferPhysicalWord::Linear(range),
                );
                assert_eq!(physical_defined_lane_mask(word).unwrap(), expected);
            }
        }

        let fixture = fixture(1);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 0))
            .unwrap();
        let odd_tail = transfer.words()[1];
        assert!(odd_tail.odd_row_exchange());
        assert_eq!(odd_tail.defined_source_byte_mask(), 0x03);
        assert_eq!(physical_defined_lane_mask(odd_tail).unwrap(), 0x30);

        let state = PhysicalTmemState::try_new().unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let logical_prefix = [Some(0x10), Some(0x11), None, None, None, None, None, None];
        assert!(matches!(
            staged.physical_word_payload(odd_tail, logical_prefix),
            Err(PhysicalTmemError::PhysicalLaneMaskMismatch {
                expected: 0x30,
                actual: 0x03,
                ..
            })
        ));
    }

    #[test]
    fn overlapping_loads_snapshot_intermediate_effect_and_publish_final_postimage() {
        let mut state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(2);
        let first_word = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 0))
            .unwrap()
            .words()[0];
        let address = fragment_lanes(first_word.physical()).unwrap()[0];
        let pending = stage_all(&state, &fixture.decoded, &[0x10, 0x80]);

        assert_eq!(pending.completed_loads(), 2);
        let first_projection = &pending.projections[0];
        let second_projection = &pending.projections[1];
        let projected_byte = |load: &LoadProjection| {
            load.accesses
                .iter()
                .find_map(|projection| {
                    let ResourceRegion::Tmem(range) = projection.access.region() else {
                        return None;
                    };
                    (range.start() as usize..range.end() as usize)
                        .contains(&address)
                        .then(|| projection.normalized_bytes[address - range.start() as usize])
                })
                .unwrap()
        };
        assert_eq!(projected_byte(first_projection), 0x10);
        assert_eq!(projected_byte(second_projection), 0x80);
        assert_eq!(pending.bytes[address], 0x80);

        let first_accesses = first_projection
            .accesses
            .iter()
            .map(|projection| projection.access)
            .collect::<Vec<_>>();
        let (_, incorrectly_final_effects) = project_load(
            first_projection.binding,
            &pending.bytes,
            &pending.valid,
            &pending.last_touched_generation,
            &first_accesses,
        )
        .unwrap();
        assert_ne!(pending.effects[0], incorrectly_final_effects[0]);

        let proposed = pending.proposed_effects().to_vec();
        let final_epoch = pending.last_load_epoch().unwrap();
        let complete = gpu_complete(fixture.decoded, fixture.backend, proposed);
        let gpu_bound = pending.bind_gpu(&complete).unwrap();
        let guest = guest_commit(complete, fixture.guest);
        state
            .publication_authority()
            .publish(gpu_bound, guest)
            .unwrap();
        assert_eq!(state.valid_byte(address as u16), Some(0x80));
        assert_eq!(state.last_load_epoch(), Some(final_epoch));
        assert_eq!(state.generation(), 1);
    }

    #[test]
    fn skipped_or_rejected_later_load_rolls_back_the_whole_packet() {
        let state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(2);
        let first = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 0))
            .unwrap();
        let packet = finish_words(
            state
                .stage_transfer(fixture.decoded.submitted(), &first)
                .unwrap(),
            0x10,
        );
        assert!(matches!(
            packet.into_pending(),
            Err(PhysicalTmemError::DestinationCoverageMismatch {
                expected: 4,
                actual: 2
            })
        ));

        let first = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 0))
            .unwrap();
        let first_staged = state
            .stage_transfer(fixture.decoded.submitted(), &first)
            .unwrap();
        let first_word = first_staged.expected_words()[0];
        let stale_first_load_payload = first_staged
            .physical_word_payload(first_word, defined_physical_lanes(first_word, 0x10))
            .unwrap();
        let packet = finish_words(first_staged, 0x10);
        let second = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 1))
            .unwrap();
        let mut rejected = packet
            .stage_transfer(fixture.decoded.submitted(), &second)
            .unwrap();
        assert!(matches!(
            rejected.stage_word(stale_first_load_payload),
            Err(PhysicalTmemError::PhysicalLanePayloadMismatch {
                field: "load identity"
            })
        ));
        drop(rejected);
        assert_eq!(state.generation(), 0);
        assert_eq!(state.last_load_epoch(), None);
        assert!(state.valid_byte(0).is_none());
    }

    #[test]
    fn lane_payload_and_word_order_rejections_poison_staging() {
        let state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(1);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded, 0))
            .unwrap();
        let mut staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let words = staged.expected_words().to_vec();
        let mut missing_lane = defined_physical_lanes(words[0], 0);
        missing_lane[7] = None;
        assert!(matches!(
            staged.physical_word_payload(words[0], missing_lane),
            Err(PhysicalTmemError::PhysicalLaneMaskMismatch {
                expected: 0xff,
                actual: 0x7f,
                ..
            })
        ));
        let wrong = staged
            .physical_word_payload(words[2], defined_physical_lanes(words[2], 0))
            .unwrap();
        assert!(matches!(
            staged.stage_word(wrong),
            Err(PhysicalTmemError::FragmentMismatch { .. })
        ));
        let right = staged
            .physical_word_payload(words[0], defined_physical_lanes(words[0], 0))
            .unwrap();
        assert_eq!(
            staged.stage_word(right).unwrap_err(),
            PhysicalTmemError::PoisonedTransaction
        );

        let mut duplicate = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let right = duplicate
            .physical_word_payload(words[0], defined_physical_lanes(words[0], 0))
            .unwrap();
        duplicate.stage_word(right).unwrap();
        let repeated = duplicate
            .physical_word_payload(words[0], defined_physical_lanes(words[0], 0))
            .unwrap();
        assert!(matches!(
            duplicate.stage_word(repeated),
            Err(PhysicalTmemError::DuplicateWord { index: 0 })
        ));
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn physical_lane_payload_rejects_equal_geometry_across_state_and_submission() {
        let state_a = PhysicalTmemState::try_new().unwrap();
        let state_b = PhysicalTmemState::try_new().unwrap();
        let shared = fixture(1);
        let transfer = shared
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&shared.decoded, 0))
            .unwrap();
        let staged_a = state_a
            .stage_transfer(shared.decoded.submitted(), &transfer)
            .unwrap();
        let mut staged_b = state_b
            .stage_transfer(shared.decoded.submitted(), &transfer)
            .unwrap();
        assert_eq!(staged_a.expected_words(), staged_b.expected_words());
        assert_eq!(staged_a.epoch(), staged_b.epoch());
        assert_ne!(
            staged_a.binding().transaction(),
            staged_b.binding().transaction()
        );
        let word = staged_a.expected_words()[0];
        let cross_state = staged_a
            .physical_word_payload(word, defined_physical_lanes(word, 0x10))
            .unwrap();
        assert!(matches!(
            staged_b.stage_word(cross_state),
            Err(PhysicalTmemError::PhysicalLanePayloadMismatch {
                field: "state identity"
            })
        ));

        let state = PhysicalTmemState::try_new().unwrap();
        let submission_a = fixture(1);
        let submission_b = fixture(1);
        let transfer_a = submission_a
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&submission_a.decoded, 0))
            .unwrap();
        let transfer_b = submission_b
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&submission_b.decoded, 0))
            .unwrap();
        let staged_a = state
            .stage_transfer(submission_a.decoded.submitted(), &transfer_a)
            .unwrap();
        let mut staged_b = state
            .stage_transfer(submission_b.decoded.submitted(), &transfer_b)
            .unwrap();
        assert_eq!(staged_a.expected_words(), staged_b.expected_words());
        assert_eq!(staged_a.epoch(), staged_b.epoch());
        let word = staged_a.expected_words()[0];
        let cross_submission = staged_a
            .physical_word_payload(word, defined_physical_lanes(word, 0x20))
            .unwrap();
        assert!(matches!(
            staged_b.stage_word(cross_submission),
            Err(PhysicalTmemError::PhysicalLanePayloadMismatch {
                field: "source identity"
            })
        ));
    }

    #[test]
    fn canonical_projection_hides_invalid_backing_and_hashes_validity_epoch_and_touch() {
        let mut first_state = PhysicalTmemState::try_new().unwrap();
        first_state.bytes.fill(0x11);
        let mut second_state = PhysicalTmemState::try_new().unwrap();
        second_state.bytes.fill(0xee);
        let first_fixture = fixture(1);
        let second_fixture = fixture(1);
        let third_fixture = fixture(1);
        let first = stage_all(&first_state, &first_fixture.decoded, &[0x20]);
        let second = stage_all(&second_state, &second_fixture.decoded, &[0x20]);
        let third = stage_all(&first_state, &third_fixture.decoded, &[0x20]);
        assert_ne!(first.binding.state, second.binding.state);
        assert_ne!(first.binding.source, second.binding.source);
        assert_ne!(first.binding.transaction, third.binding.transaction);
        assert_eq!(first.effects, second.effects);
        assert_eq!(first.effects, third.effects);
        assert_ne!(first.proposal_identity, second.proposal_identity);
        assert_ne!(first.proposal_identity, third.proposal_identity);
        assert!(first
            .projections
            .iter()
            .all(|load| load.accesses.iter().all(|projection| projection
                .normalized_bytes
                .iter()
                .zip(&projection.validity)
                .all(|(byte, valid)| *valid != 0 || *byte == 0))));

        let load_projection = &first.projections[0];
        let projection = &load_projection.accesses[0];
        let baseline = completed_effect(projection).unwrap();
        let byte_count = projection.normalized_bytes.len() as u32;
        assert_eq!(baseline.byte_count(), byte_count);
        assert!(
            projection
                .validity
                .iter()
                .filter(|valid| **valid == 1)
                .count()
                < baseline.byte_count() as usize
        );
        assert_eq!(
            baseline.content(),
            CompletedWrite::physical_tmem_content_digest(
                byte_count,
                projection.load_epoch.get(),
                &projection.normalized_bytes,
                &projection.validity,
                &projection.last_touched_generation,
            )
        );
        let mut changed_validity = CanonicalTmemEffectProjection {
            access: projection.access,
            load_epoch: projection.load_epoch,
            normalized_bytes: projection.normalized_bytes.clone(),
            validity: projection.validity.clone(),
            last_touched_generation: projection.last_touched_generation.clone(),
        };
        let lane = changed_validity
            .validity
            .iter()
            .position(|valid| *valid == 1)
            .unwrap();
        changed_validity.validity[lane] = 0;
        changed_validity.normalized_bytes[lane] = 0;
        assert_ne!(baseline, completed_effect(&changed_validity).unwrap());
        let mut changed_touch = changed_validity;
        changed_touch.last_touched_generation[lane] += 1;
        assert_ne!(
            completed_effect(&changed_touch).unwrap(),
            completed_effect(projection).unwrap()
        );
        let changed_epoch = CanonicalTmemEffectProjection {
            access: projection.access,
            load_epoch: TmemLoadEpoch::new(
                core::num::NonZeroU64::new(projection.load_epoch.get() + 1).unwrap(),
            ),
            normalized_bytes: projection.normalized_bytes.clone(),
            validity: projection.validity.clone(),
            last_touched_generation: projection.last_touched_generation.clone(),
        };
        assert_ne!(baseline, completed_effect(&changed_epoch).unwrap());
    }

    #[test]
    fn gpu_binding_rejects_changed_missing_reordered_and_cross_lifecycle_reports() {
        let state = PhysicalTmemState::try_new().unwrap();
        let changed_fixture = fixture(1);
        let pending = stage_all(&state, &changed_fixture.decoded, &[0x20]);
        let mut changed = pending.proposed_effects().to_vec();
        changed[0] = CompletedWrite::try_new(
            changed[0].access(),
            changed[0].byte_count(),
            ContentDigest::hash(b"hostile-wrong-TMEM-content", &[]),
        )
        .unwrap();
        let complete = gpu_complete(changed_fixture.decoded, changed_fixture.backend, changed);
        assert!(matches!(
            pending.bind_gpu(&complete),
            Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "proposed write content"
            })
        ));

        let exact_fixture = fixture(1);
        let pending = stage_all(&state, &exact_fixture.decoded, &[0x20]);
        let complete = gpu_complete(
            exact_fixture.decoded,
            exact_fixture.backend,
            pending.proposed_effects().to_vec(),
        );
        let mut missing = pending.proposed_effects().to_vec();
        let absent_access = ResourceAccess::try_new(
            OperationId::new(99),
            AccessMode::Write,
            AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(0x100, 0x108).unwrap()),
        )
        .unwrap();
        missing.push(
            CompletedWrite::try_new(
                absent_access,
                8,
                ContentDigest::hash(b"hostile-absent-write", &[]),
            )
            .unwrap(),
        );
        assert!(matches!(
            validate_gpu(&complete, pending.binding, &missing),
            Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "missing proposed write"
            })
        ));
        let mut reordered = pending.proposed_effects().to_vec();
        reordered.reverse();
        assert!(validate_gpu(&complete, pending.binding, &reordered).is_err());

        let other = fixture(1);
        let other_pending = stage_all(&state, &other.decoded, &[0x20]);
        let other_complete = gpu_complete(
            other.decoded,
            other.backend,
            other_pending.proposed_effects().to_vec(),
        );
        assert!(matches!(
            validate_gpu(&other_complete, pending.binding, pending.proposed_effects()),
            Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "queue identity"
            })
        ));
    }

    #[test]
    fn gpu_binding_rejects_same_queue_cross_submission_and_wrong_ordinal() {
        let state = PhysicalTmemState::try_new().unwrap();
        let (mut queue, mut backend, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let decoded_a = decode_with(&mut queue, planned_packet(1));
        let decoded_b = decode_with(&mut queue, planned_packet(1));
        let pending_a = stage_all(&state, &decoded_a, &[0x20]);
        let pending_b = stage_all(&state, &decoded_b, &[0x20]);
        let report = BackendEffectReport::try_new(
            decoded_b.submitted().packet(),
            pending_b.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt = backend.issue(decoded_b.submitted(), report).unwrap();
        let submitted_b = decoded_b.into_contract_parts().submitted;
        let complete_b = submitted_b.gpu_complete(receipt).unwrap();

        assert!(matches!(
            validate_gpu(&complete_b, pending_a.binding, pending_a.proposed_effects()),
            Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "submission identity"
            })
        ));
        let mut wrong_ordinal = pending_b.binding;
        wrong_ordinal.submission_ordinal += 1;
        assert!(matches!(
            validate_gpu(&complete_b, wrong_ordinal, pending_b.proposed_effects()),
            Err(PhysicalTmemError::GpuCompletionMismatch {
                field: "submission ordinal"
            })
        ));
        drop(decoded_a);
    }

    #[test]
    fn state_identity_and_generation_reject_cross_state_and_stale_publication() {
        let mut state_a = PhysicalTmemState::try_new().unwrap();
        let mut state_b = PhysicalTmemState::try_new().unwrap();
        let cross_state_fixture = fixture(1);
        let pending = stage_all(&state_a, &cross_state_fixture.decoded, &[0x20]);
        let complete = gpu_complete(
            cross_state_fixture.decoded,
            cross_state_fixture.backend,
            pending.proposed_effects().to_vec(),
        );
        let gpu_bound = pending.bind_gpu(&complete).unwrap();
        let guest = guest_commit(complete, cross_state_fixture.guest);
        assert!(matches!(
            state_b.publication_authority().publish(gpu_bound, guest),
            Err(PhysicalTmemError::CrossStatePublication { .. })
        ));
        assert_eq!(state_b.generation(), 0);

        let fixture_a = fixture(1);
        let fixture_b = fixture(1);
        let pending_a = stage_all(&state_a, &fixture_a.decoded, &[0x20]);
        let pending_b = stage_all(&state_a, &fixture_b.decoded, &[0x80]);
        let complete_b = gpu_complete(
            fixture_b.decoded,
            fixture_b.backend,
            pending_b.proposed_effects().to_vec(),
        );
        let gpu_b = pending_b.bind_gpu(&complete_b).unwrap();
        let guest_b = guest_commit(complete_b, fixture_b.guest);
        state_a
            .publication_authority()
            .publish(gpu_b, guest_b)
            .unwrap();

        let complete_a = gpu_complete(
            fixture_a.decoded,
            fixture_a.backend,
            pending_a.proposed_effects().to_vec(),
        );
        let gpu_a = pending_a.bind_gpu(&complete_a).unwrap();
        let guest_a = guest_commit(complete_a, fixture_a.guest);
        assert!(matches!(
            state_a.publication_authority().publish(gpu_a, guest_a),
            Err(PhysicalTmemError::StaleBaseGeneration {
                expected: 0,
                actual: 1
            })
        ));
    }

    fn backend_report(
        decoded: &crate::DecodedRawDpc,
        writes: Vec<CompletedWrite>,
    ) -> BackendEffectReport {
        BackendEffectReport::try_new(decoded.submitted().packet(), writes).unwrap()
    }

    #[test]
    fn successor_is_independent_and_matches_publish_postimage() {
        let mut state = PhysicalTmemState::try_new().unwrap();
        state.bytes.fill(0xaa);
        let base_identity = state.identity();
        let fixture = fixture(1);
        let pending = stage_all(&state, &fixture.decoded, &[0x20]);
        let proposed = pending.proposed_effects().to_vec();
        let report = backend_report(&fixture.decoded, proposed.clone());

        let successor = pending
            .into_physical_successor(&state, &report)
            .expect("successor must validate against the untouched base state");

        // Non-mutation: `state` (the coordinator's active slot analogue) is
        // byte-for-byte unchanged -- identity, generation, epoch, and every
        // TMEM byte/validity/touch-generation entry.
        assert_eq!(state.identity(), base_identity);
        assert_eq!(state.generation(), 0);
        assert_eq!(state.last_load_epoch(), None);
        assert!(state.valid_byte(0).is_none());
        assert_eq!(state.bytes.as_ref(), &[0xaa; TMEM_LEN]);

        // The successor is a genuinely distinct state, not an alias.
        assert_ne!(successor.identity(), state.identity());
        assert_eq!(successor.generation(), 1);
        assert!(successor.last_load_epoch().is_some());

        assert_eq!(successor.bytes.len(), TMEM_LEN);
    }

    #[test]
    fn successor_generation_epoch_and_bytes_match_what_publish_would_durably_write() {
        let mut state_direct = PhysicalTmemState::try_new().unwrap();
        let state_via_successor = PhysicalTmemState::try_new().unwrap();

        let fixture_a = fixture(1);
        let pending_a = stage_all(&state_direct, &fixture_a.decoded, &[0x40]);
        let proposed_a = pending_a.proposed_effects().to_vec();
        let complete_a = gpu_complete(fixture_a.decoded, fixture_a.backend, proposed_a.clone());
        let gpu_a = pending_a.bind_gpu(&complete_a).unwrap();
        let guest_a = guest_commit(complete_a, fixture_a.guest);
        state_direct
            .publication_authority()
            .publish(gpu_a, guest_a)
            .unwrap();

        let fixture_b = fixture(1);
        let pending_b = stage_all(&state_via_successor, &fixture_b.decoded, &[0x40]);
        let report_b = backend_report(&fixture_b.decoded, pending_b.proposed_effects().to_vec());
        let successor = pending_b
            .into_physical_successor(&state_via_successor, &report_b)
            .unwrap();

        assert_eq!(successor.generation(), state_direct.generation());
        assert_eq!(successor.last_load_epoch(), state_direct.last_load_epoch());
        assert_eq!(successor.bytes, state_direct.bytes);
        assert_eq!(successor.valid, state_direct.valid);
        assert_eq!(
            successor.last_touched_generation,
            state_direct.last_touched_generation
        );
        // The unpublished base is provably untouched.
        assert_eq!(state_via_successor.generation(), 0);
    }

    #[test]
    fn successor_rejects_cross_state_and_stale_generation() {
        let state_a = PhysicalTmemState::try_new().unwrap();
        let mut state_b = PhysicalTmemState::try_new().unwrap();

        let cross_fixture = fixture(1);
        let pending = stage_all(&state_a, &cross_fixture.decoded, &[0x20]);
        let report = backend_report(&cross_fixture.decoded, pending.proposed_effects().to_vec());
        assert!(matches!(
            pending.into_physical_successor(&state_b, &report),
            Err(PhysicalTmemError::CrossStatePublication { .. })
        ));
        assert_eq!(state_b.generation(), 0);

        let fixture_a = fixture(1);
        let fixture_c = fixture(1);
        let pending_a = stage_all(&state_b, &fixture_a.decoded, &[0x20]);
        let pending_c = stage_all(&state_b, &fixture_c.decoded, &[0x80]);
        let complete_c = gpu_complete(
            fixture_c.decoded,
            fixture_c.backend,
            pending_c.proposed_effects().to_vec(),
        );
        // Publish `pending_c` through the legacy path to advance `state_b`
        // so the stale-generation check below observes a real advance.
        let gpu_bound_c = pending_c.bind_gpu(&complete_c).unwrap();
        let guest_c = guest_commit(complete_c, fixture_c.guest);
        state_b
            .publication_authority()
            .publish(gpu_bound_c, guest_c)
            .unwrap();

        let report_a = backend_report(&fixture_a.decoded, pending_a.proposed_effects().to_vec());
        assert!(matches!(
            pending_a.into_physical_successor(&state_b, &report_a),
            Err(PhysicalTmemError::StaleBaseGeneration {
                expected: 0,
                actual: 1
            })
        ));
    }

    #[test]
    fn successor_rejects_stale_load_epoch_with_matching_generation() {
        // `base_generation`/`base_last_load_epoch` normally advance in
        // lockstep, so a forged binding is the only way to exercise
        // `StaleLoadEpoch` in isolation from `StaleBaseGeneration` -- the
        // same reachability gap `publish`'s own test suite leaves untested.
        let state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(1);
        let mut pending = stage_all(&state, &fixture.decoded, &[0x20]);
        let report = backend_report(&fixture.decoded, pending.proposed_effects().to_vec());
        pending.binding.base_last_load_epoch =
            Some(TmemLoadEpoch::new(core::num::NonZeroU64::new(1).unwrap()));

        assert!(matches!(
            pending.into_physical_successor(&state, &report),
            Err(PhysicalTmemError::StaleLoadEpoch {
                expected: Some(_),
                actual: None,
            })
        ));
        assert_eq!(state.generation(), 0);
    }

    /// The relaxation `validate_backend_effects` took for the mixed
    /// fill+TMEM card, tested directly at the function -- because no
    /// legitimately-constructed `BackendEffectReport` can express these
    /// shapes (`try_new` validates against the packet's own journal first),
    /// and the mixed-packet path that exercises the relaxation lives two
    /// crates up.
    ///
    /// The relaxation is: `reported` may be a strict SUPERSET of `proposed`,
    /// in the journal's own order, so a composed packet's interleaved
    /// `RenderTarget` writes do not read as this transaction's omission.
    /// Every other divergence must still be a named rejection -- these
    /// cases are what proves the weakening was surgical rather than a hole.
    ///
    /// Each case was confirmed to be a real kill: removing the
    /// `missing proposed write` arm left the whole 4990-test suite green
    /// before this test existed (a measured mutation survivor), which is
    /// why it is pinned here.
    #[test]
    fn backend_effects_admit_a_superset_in_order_and_reject_everything_else() {
        fn tmem_write(operation: u32, start: u32, domain: &[u8]) -> CompletedWrite {
            CompletedWrite::try_new(
                ResourceAccess::try_new(
                    OperationId::new(operation),
                    AccessMode::Write,
                    AccessPurpose::TmemLoadDestination,
                    ResourceRegion::Tmem(TmemRange::try_new(start, start + 8).unwrap()),
                )
                .unwrap(),
                8,
                ContentDigest::hash(domain, &[]),
            )
            .unwrap()
        }
        fn render_target_write(operation: u32) -> CompletedWrite {
            let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
            CompletedWrite::try_new(
                ResourceAccess::try_new(
                    OperationId::new(operation),
                    AccessMode::Write,
                    AccessPurpose::RenderTarget,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(0x2000, 0x2010).unwrap(),
                    },
                )
                .unwrap(),
                16,
                ContentDigest::hash(b"fill-bytes", &[]),
            )
            .unwrap()
        }

        let first = tmem_write(1, 0, b"first");
        let second = tmem_write(3, 8, b"second");
        let proposed = vec![first, second];
        let fill = render_target_write(2);

        // Exactly the proposed list: accepted, as before the relaxation.
        assert!(validate_backend_effects(&proposed, &proposed).is_ok());

        // The relaxation itself: a fill's write INTERLEAVED between the two
        // TMEM writes, in journal order. This is the shape a composed
        // fill+TMEM packet reports, and the whole point of the change.
        assert!(
            validate_backend_effects(&[first, fill, second], &proposed).is_ok(),
            "a superset carrying another source's write between two proposed ones, in order, \
             must be admitted -- this is exactly the composed fill+TMEM report"
        );
        // And with the fill's write before and after the pair.
        assert!(validate_backend_effects(&[fill, first, second], &proposed).is_ok());
        assert!(validate_backend_effects(&[first, second, fill], &proposed).is_ok());

        // A MISSING proposed write is still rejected by name. This is the
        // arm whose removal survived the suite before this test existed.
        assert!(matches!(
            validate_backend_effects(&[first, fill], &proposed),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "missing proposed write"
            })
        ));
        assert!(matches!(
            validate_backend_effects(&[fill], &proposed),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "write count"
            })
        ));

        // A REORDER of two proposed writes is still rejected. The walk is
        // order-preserving: once `second` is matched, `first` can no longer
        // be found ahead of the cursor.
        assert!(matches!(
            validate_backend_effects(&[second, first], &proposed),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "missing proposed write"
            })
        ));

        // WRONG CONTENT at a matched access is still rejected.
        let second_wrong = tmem_write(3, 8, b"tampered");
        assert!(matches!(
            validate_backend_effects(&[first, second_wrong], &proposed),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "write content"
            })
        ));

        // A DUPLICATE of a proposed access is still rejected -- the same
        // TMEM range written twice is not a superset, it is ambiguity about
        // which write this transaction vouched for.
        assert!(matches!(
            validate_backend_effects(&[first, second, second], &proposed),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "duplicate proposed access"
            })
        ));
    }

    #[test]
    fn successor_rejects_mismatched_backend_write_content() {
        let state = PhysicalTmemState::try_new().unwrap();
        let changed_fixture = fixture(1);
        let pending = stage_all(&state, &changed_fixture.decoded, &[0x20]);
        let mut changed = pending.proposed_effects().to_vec();
        changed[0] = CompletedWrite::try_new(
            changed[0].access(),
            changed[0].byte_count(),
            ContentDigest::hash(b"hostile-wrong-TMEM-content", &[]),
        )
        .unwrap();
        let report = backend_report(&changed_fixture.decoded, changed);
        assert!(matches!(
            pending.into_physical_successor(&state, &report),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "write content"
            })
        ));
    }

    fn single_write_journal_packet(access: ResourceAccess) -> WorkloadPacket {
        let words = load_tile_words(1);
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        finalize_packet(&words, vec![command_access(layout, byte_count, 0), access])
    }

    #[test]
    fn successor_rejects_a_report_bound_to_a_different_transaction() {
        // `BackendEffectReport::try_new` already proves `writes` is exactly
        // the write set its own packet's journal declares, so a same-packet
        // reorder/omission cannot reach this method as a legitimately
        // constructed report -- that guarantee lives in `fn64-render-ir` and
        // is out of `into_physical_successor`'s scope to re-check. What it
        // must still catch is a report honestly built for a genuinely
        // different transaction, whose declared writes diverge from this
        // pending transaction's own proposed effects.
        let state = PhysicalTmemState::try_new().unwrap();
        let target_fixture = fixture(1);
        let pending = stage_all(&state, &target_fixture.decoded, &[0x20]);

        let foreign_access = ResourceAccess::try_new(
            OperationId::new(41),
            AccessMode::Write,
            AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(0, 8).unwrap()),
        )
        .unwrap();
        let foreign_packet = single_write_journal_packet(foreign_access);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let foreign_submitted = queue.submit(DecodedTicket::new(foreign_packet)).unwrap();
        let foreign_write = CompletedWrite::try_new(
            foreign_access,
            8,
            ContentDigest::hash(b"hostile-foreign-transaction", &[]),
        )
        .unwrap();
        let foreign_report =
            BackendEffectReport::try_new(foreign_submitted.packet(), vec![foreign_write]).unwrap();

        assert!(matches!(
            pending.into_physical_successor(&state, &foreign_report),
            Err(PhysicalTmemError::BackendEffectMismatch {
                field: "write count"
            })
        ));
    }

    #[test]
    fn journal_slice_index_is_not_derived_from_operation_identity() {
        let first = ResourceAccess::try_new(
            OperationId::new(41),
            AccessMode::Write,
            AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(0, 8).unwrap()),
        )
        .unwrap();
        let destination = ResourceAccess::try_new(
            OperationId::new(77),
            AccessMode::Write,
            AccessPurpose::TmemLoadDestination,
            ResourceRegion::Tmem(TmemRange::try_new(16, 24).unwrap()),
        )
        .unwrap();
        let packet = [first, destination];
        assert!(validate_packet_slice(&packet, 1, &[destination], "hostile slice").is_ok());
        assert!(validate_packet_slice(&packet, 77, &[destination], "hostile slice").is_err());
    }

    // M4.3.1b: LoadTLUT's destination-mask goldens/hostiles. TLUT captures 2
    // source bytes per entry (`defined_source_byte_mask` == 0x03) but
    // quadricates them into all 8 destination bytes
    // (`defined_destination_byte_mask` == 0xff); `physical_defined_lane_mask`
    // must key off the destination mask, not the source mask, or it either
    // wrongly rejects the correct 8-lane payload or wrongly accepts a 2-lane
    // payload that would mark 6 real destination bytes invalid.
    mod tlut_destination_mask {
        use super::*;

        fn tlut_words(tmem_base: u32, entries_minus_one: u32) -> Vec<u32> {
            vec![
                word(SET_TEXTURE_IMAGE, 2 << 19),
                0x200,
                word(SET_TILE, 2 << 19 | tmem_base),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TLUT, 0),
                7 << 24 | entries_minus_one << 14,
            ]
        }

        fn tlut_fixture(entries: u16) -> Fixture {
            let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
            let words = tlut_words(256, u32::from(entries) - 1);
            let source_end = 0x200 + u32::from(entries) * 2;
            let packet = planned_packet_with_sources(words, &[(0x200, source_end)]);
            Fixture {
                decoded: decode_with(&mut queue, packet),
                backend,
                guest,
            }
        }

        fn tlut_load(decoded: &crate::DecodedRawDpc) -> super::super::super::TmemLoad {
            match decoded.commands()[3].kind() {
                RawDpcCommandKind::LoadTlut(load) => load,
                other => panic!("expected LoadTLUT, found {other:?}"),
            }
        }

        #[test]
        fn tlut_word_reports_source_0x03_destination_0xff() {
            let fixture = tlut_fixture(1);
            let transfer = fixture
                .decoded
                .resource_plan()
                .bind_tmem_transfer(tlut_load(&fixture.decoded))
                .unwrap();
            assert_eq!(transfer.words().len(), 1);
            let word = transfer.words()[0];
            assert_eq!(word.defined_source_byte_mask(), 0x03);
            assert_eq!(word.defined_destination_byte_mask(), 0xff);
            assert_eq!(physical_defined_lane_mask(word).unwrap(), 0xff);
        }

        #[test]
        fn all_eight_some_tlut_payload_is_accepted() {
            let fixture = tlut_fixture(1);
            let transfer = fixture
                .decoded
                .resource_plan()
                .bind_tmem_transfer(tlut_load(&fixture.decoded))
                .unwrap();
            let word = transfer.words()[0];
            let state = PhysicalTmemState::try_new().unwrap();
            let mut staged = state
                .stage_transfer(fixture.decoded.submitted(), &transfer)
                .unwrap();
            // One quadricated TLUT entry: two real source bytes (0x10, 0x11)
            // repeated across all four 16-bit lanes of the 8-byte word.
            let quadricated = [
                Some(0x10),
                Some(0x11),
                Some(0x10),
                Some(0x11),
                Some(0x10),
                Some(0x11),
                Some(0x10),
                Some(0x11),
            ];
            let payload = staged.physical_word_payload(word, quadricated).unwrap();
            staged.stage_word(payload).unwrap();
            let candidate = staged.finish_load().unwrap();
            let lanes = fragment_lanes(word.physical()).unwrap();
            for (lane, address) in lanes.into_iter().enumerate() {
                assert!(candidate.valid[address]);
                assert_eq!(candidate.bytes[address], quadricated[lane].unwrap());
            }
        }

        #[test]
        fn only_two_some_tlut_payload_is_rejected() {
            let fixture = tlut_fixture(1);
            let transfer = fixture
                .decoded
                .resource_plan()
                .bind_tmem_transfer(tlut_load(&fixture.decoded))
                .unwrap();
            let word = transfer.words()[0];
            let state = PhysicalTmemState::try_new().unwrap();
            let staged = state
                .stage_transfer(fixture.decoded.submitted(), &transfer)
                .unwrap();
            let two_lane = [Some(0x10), Some(0x11), None, None, None, None, None, None];
            assert!(matches!(
                staged.physical_word_payload(word, two_lane),
                Err(PhysicalTmemError::PhysicalLaneMaskMismatch {
                    expected: 0xff,
                    actual: 0x03,
                    ..
                })
            ));
        }

        #[test]
        fn direct_tile_masks_are_unchanged_by_the_destination_mask_split() {
            // Block/Tile: source and destination masks coincide, including
            // the odd-row partial tail and the split-bank RGBA32 case
            // already covered by `linear_partial_tail_masks_follow_even_and_odd_row_lane_exchange`
            // and `odd_width_rgba32_tail_uses_split_bank_physical_mask`. This
            // is a targeted regression that word minting itself (via
            // `raw_dpc::transfer_record`) still produces `source ==
            // destination` for a non-TLUT kind, now that both masks are
            // minted independently.
            let fixture = fixture(1);
            let transfer = fixture
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load(&fixture.decoded, 0))
                .unwrap();
            for word in transfer.words() {
                assert_eq!(
                    word.defined_source_byte_mask(),
                    word.defined_destination_byte_mask()
                );
            }
        }

        // Both hostiles below forge a `TmemTransferWord` whose destination
        // mask violates one of the two minting invariants. Production code
        // cannot reach either shape: `raw_dpc::transfer_record`
        // (`raw_dpc/mod.rs:599`) always sources both masks from the same
        // `TmemTransferPlan`, and `neutral_transfer_word` (`physical.rs:1340`)
        // is a field-for-field copy of an already-validated neutral word.
        //
        // `TmemTransferWord::new` carries a matching pair of `debug_assert!`s
        // (`tmem/types.rs:829-841`), but those are compiled out under
        // `-C debug-assertions=off`, so a `#[should_panic]` test on them
        // would assert a build-profile-dependent property -- precisely what
        // `rt64_common.rs:246-251` declines to do. The invariant itself is
        // *not* profile-dependent: `physical_defined_lane_mask`
        // (`physical.rs:1578-1586`) re-checks both conditions unconditionally
        // and returns `Err(DestinationPlanMismatch)`, and every consumption
        // path runs it -- `validate_physical_plan` (`physical.rs:1547`) over
        // every word of every staged transfer, and
        // `DefinedPhysicalTmemWordBytes::try_from_physical_lanes`
        // (`physical.rs:83`) on every payload. These tests therefore assert
        // the release-surviving `Result` rejection, which holds in both
        // profiles, rather than the debug-only panic.

        #[test]
        fn forged_mismatched_masks_are_rejected_by_the_lane_mask_check() {
            let range = fn64_render_ir::TmemRange::try_new(0, 8).unwrap();
            // destination mask (0x01) claims fewer defined bytes than source
            // (0x03) -- forbidden for every current load kind.
            let forged = forge_word(0x03, 0x01, range);
            assert!(matches!(
                physical_defined_lane_mask(forged),
                Err(PhysicalTmemError::DestinationPlanMismatch)
            ));
        }

        #[test]
        fn non_prefix_destination_mask_is_rejected() {
            let range = fn64_render_ir::TmemRange::try_new(0, 8).unwrap();
            // 0x05 (bits 0 and 2) is not a contiguous low-bit prefix.
            let forged = forge_word(0x01, 0x05, range);
            assert!(matches!(
                physical_defined_lane_mask(forged),
                Err(PhysicalTmemError::DestinationPlanMismatch)
            ));
        }

        /// Mints a deliberately invariant-violating word for the two hostiles
        /// above. Only ever called with mask pairs that the constructor's
        /// `debug_assert!`s reject, so it must not run under debug
        /// assertions; both callers assert on the release-surviving
        /// `physical_defined_lane_mask` rejection instead. The `cfg!` guard
        /// keeps the *test count* identical in both profiles (unlike
        /// `#[cfg(not(debug_assertions))]` on the tests themselves) while
        /// still exercising the real check whenever the constructor would
        /// not abort first.
        fn forge_word(
            source_mask: u8,
            destination_mask: u8,
            range: fn64_render_ir::TmemRange,
        ) -> TmemTransferWord {
            if cfg!(debug_assertions) {
                // Mint a valid word, then overwrite the masks so the
                // constructor's debug-only asserts never observe the forged
                // pair. Field access is legal here: `tests` is a descendant
                // module of the crate that declares `TmemTransferWord`.
                let mut word = TmemTransferWord::new(
                    0,
                    0,
                    0,
                    0,
                    0xff,
                    0xff,
                    0,
                    0,
                    false,
                    TmemTransferPhysicalWord::Linear(range),
                );
                word.forge_masks_for_test(source_mask, destination_mask);
                word
            } else {
                TmemTransferWord::new(
                    0,
                    0,
                    0,
                    0,
                    source_mask,
                    destination_mask,
                    0,
                    0,
                    false,
                    TmemTransferPhysicalWord::Linear(range),
                )
            }
        }
    }
}
