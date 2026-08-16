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

use super::{TmemLoadEpoch, TmemLoadSourceIdentity, TmemTransferPhysicalWord, TmemTransferWord};

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

        #[test]
        #[should_panic(expected = "cannot claim fewer defined destination bytes")]
        fn forged_mismatched_masks_are_rejected_by_the_private_constructor() {
            // Production code cannot mint a `TmemTransferWord` at all except
            // through `raw_dpc::transfer_record`, which always sources both
            // masks from the same `TmemTransferPlan`. This forged combination
            // is reachable only through this crate-private constructor, and
            // its own `debug_assert!` invariant check catches it at mint
            // time -- stronger than a runtime `Result`, and exactly why the
            // check lives in the constructor rather than only in
            // `physical_defined_lane_mask`.
            let range = fn64_render_ir::TmemRange::try_new(0, 8).unwrap();
            // destination mask (0x01) claims fewer defined bytes than source
            // (0x03) -- forbidden for every current load kind.
            let _ = TmemTransferWord::new(
                0,
                0,
                0,
                0,
                0x03,
                0x01,
                0,
                0,
                false,
                TmemTransferPhysicalWord::Linear(range),
            );
        }

        #[test]
        #[should_panic(expected = "must be a nonzero low-bit prefix")]
        fn non_prefix_destination_mask_is_rejected() {
            let range = fn64_render_ir::TmemRange::try_new(0, 8).unwrap();
            // 0x05 (bits 0 and 2) is not a contiguous low-bit prefix.
            let _ = TmemTransferWord::new(
                0,
                0,
                0,
                0,
                0x01,
                0x05,
                0,
                0,
                false,
                TmemTransferPhysicalWord::Linear(range),
            );
        }
    }
}
