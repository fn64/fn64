use core::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::{
    journal::hash_region, ContentDigest, EffectIdentity, JournalIdentity, ResourceAccess,
    ValidationError, WorkloadIdentity, WorkloadPacket,
};

static NEXT_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueueIdentity(u64);

impl QueueIdentity {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SubmissionIdentity(ContentDigest);

impl SubmissionIdentity {
    pub const fn digest(self) -> ContentDigest {
        self.0
    }
}

/// Creates three distinct role capabilities for one lifecycle. Queue and
/// ordinal identities are issued inside this crate; callers cannot choose
/// values which make two same-content packets collide.
#[derive(Debug)]
pub struct TicketAuthoritySet {
    authority: u64,
}

impl TicketAuthoritySet {
    pub fn try_new() -> Result<Self, ValidationError> {
        let authority = NEXT_AUTHORITY_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| ValidationError::TicketAuthorityExhausted)?;
        Ok(Self { authority })
    }

    pub fn into_roles(
        self,
    ) -> (
        SubmissionQueue,
        BackendCompletionAuthority,
        GuestCommitAuthority,
    ) {
        (
            SubmissionQueue {
                queue: QueueIdentity(self.authority),
                next_ordinal: 0,
            },
            BackendCompletionAuthority {
                queue: QueueIdentity(self.authority),
            },
            GuestCommitAuthority {
                queue: QueueIdentity(self.authority),
            },
        )
    }
}

/// Sole issuer of queue publication identities for one lifecycle.
#[derive(Debug)]
pub struct SubmissionQueue {
    queue: QueueIdentity,
    next_ordinal: u64,
}

impl SubmissionQueue {
    pub const fn identity(&self) -> QueueIdentity {
        self.queue
    }

    pub fn submit(&mut self, decoded: DecodedTicket) -> Result<SubmittedTicket, ValidationError> {
        let ordinal = self.next_ordinal;
        self.next_ordinal = self.next_ordinal.checked_add(1).ok_or(
            ValidationError::SubmissionOrdinalExhausted {
                queue: self.queue.0,
            },
        )?;
        Ok(issue_submitted_ticket(decoded, self.queue, ordinal))
    }

    /// Fallibly prove ordinal capacity while exclusively borrowing the queue,
    /// without issuing or reserving an ordinal. The returned typestate is the
    /// only route to a later infallible issuance; a capacity failure here
    /// leaves `self` completely unchanged.
    pub fn try_ready_submission(&mut self) -> Result<ReadySubmissionQueue<'_>, ValidationError> {
        if self.next_ordinal.checked_add(1).is_none() {
            return Err(ValidationError::SubmissionOrdinalExhausted {
                queue: self.queue.0,
            });
        }
        Ok(ReadySubmissionQueue { queue: self })
    }
}

/// Nonmutating proof that one more ordinal fits, exclusively borrowing its
/// [`SubmissionQueue`]. Holding this value reserves no ordinal by itself;
/// only [`Self::issue`] advances the queue, and it cannot fail because
/// capacity was already proven at construction and the borrow forbids any
/// intervening mutation.
#[derive(Debug)]
pub struct ReadySubmissionQueue<'queue> {
    queue: &'queue mut SubmissionQueue,
}

impl<'queue> ReadySubmissionQueue<'queue> {
    pub const fn identity(&self) -> QueueIdentity {
        self.queue.queue
    }

    /// Infallibly issue the next ordinal and publish `decoded`. Capacity was
    /// already proven by [`SubmissionQueue::try_ready_submission`]; the
    /// exclusive borrow held since then makes this call the queue's next
    /// state deterministically, so there is no `Result` and no way to
    /// produce a half-bound value.
    pub fn issue(self, decoded: DecodedTicket) -> SubmittedTicket {
        let ordinal = self.queue.next_ordinal;
        self.queue.next_ordinal = ordinal
            .checked_add(1)
            .expect("try_ready_submission proved capacity for this exact ordinal");
        issue_submitted_ticket(decoded, self.queue.queue, ordinal)
    }
}

fn issue_submitted_ticket(
    decoded: DecodedTicket,
    queue: QueueIdentity,
    ordinal: u64,
) -> SubmittedTicket {
    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.submission.v2\0");
    hash.update(decoded.packet.identity().as_bytes());
    hash.update(queue.0.to_be_bytes());
    hash.update(ordinal.to_be_bytes());
    SubmittedTicket {
        packet: decoded.packet,
        queue,
        ordinal,
        identity: SubmissionIdentity(ContentDigest::from_bytes(hash.finalize().into())),
    }
}

/// Owns a decoded packet before queue publication. It intentionally has no
/// submission method; only a [`SubmissionQueue`] can publish it.
///
/// ```compile_fail
/// use fn64_render_ir::DecodedTicket;
/// # fn packet() -> fn64_render_ir::WorkloadPacket { unimplemented!() }
/// let decoded = DecodedTicket::new(packet());
/// decoded.submit(0);
/// ```
#[derive(Debug)]
pub struct DecodedTicket {
    packet: WorkloadPacket,
}

impl DecodedTicket {
    pub const fn new(packet: WorkloadPacket) -> Self {
        Self { packet }
    }

    pub const fn packet(&self) -> &WorkloadPacket {
        &self.packet
    }
}

/// Sole owner after bounded queue publication and before backend completion.
/// It has no receipt constructor; completion evidence is issued through the
/// separately held [`BackendCompletionAuthority`].
#[derive(Debug)]
pub struct SubmittedTicket {
    packet: WorkloadPacket,
    queue: QueueIdentity,
    ordinal: u64,
    identity: SubmissionIdentity,
}

impl SubmittedTicket {
    pub const fn packet(&self) -> &WorkloadPacket {
        &self.packet
    }

    pub const fn queue(&self) -> QueueIdentity {
        self.queue
    }

    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub const fn identity(&self) -> SubmissionIdentity {
        self.identity
    }

    pub fn gpu_complete(
        self,
        receipt: GpuCompletionReceipt,
    ) -> Result<GpuCompleteTicket, ValidationError> {
        validate_receipt_header(
            ReceiptHeader {
                queue: self.queue,
                workload: self.packet.identity(),
                submission: self.identity,
                journal: self.packet.journal().identity(),
            },
            ReceiptHeader {
                queue: receipt.queue,
                workload: receipt.workload,
                submission: receipt.submission,
                journal: receipt.journal,
            },
        )?;
        if receipt.effects.workload != self.packet.identity() {
            return Err(ValidationError::ReceiptEffectMismatch);
        }
        Ok(GpuCompleteTicket {
            packet: self.packet,
            queue: self.queue,
            ordinal: self.ordinal,
            submission: self.identity,
            effects: receipt.effects,
        })
    }
}

/// One exact mutable effect observed by a backend or guest commit owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletedWrite {
    access: ResourceAccess,
    byte_count: u32,
    content: ContentDigest,
}

/// Canonical identity for exact renderer-produced effect bytes.
///
/// Backends and guest-commit adapters share this domain so the same staged
/// bytes cannot acquire backend-specific identities at the ownership seam.
pub fn effect_content_digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::hash(b"fn64.render.ir-effect-bytes.v1\0", &[bytes])
}

impl CompletedWrite {
    /// Renderer-neutral domain for canonical physical-TMEM effect content.
    ///
    /// Backends must use [`Self::physical_tmem_content_digest`] rather than
    /// introducing an adapter-specific hash domain for the same projection.
    pub const PHYSICAL_TMEM_CONTENT_DOMAIN: &'static [u8] =
        b"fn64.render-ir.physical-tmem-effect-content.v1\0";
    const PHYSICAL_TMEM_UNIFORM_CONTENT_DOMAIN: &'static [u8] =
        b"fn64.render-ir.physical-tmem-effect-content.uniform.v2\0";

    /// Hashes one canonical physical-TMEM effect postimage without lifecycle,
    /// allocation, workload, journal, submission, or access identity.
    ///
    /// `byte_count` is complete declared physical access coverage, including
    /// invalid lanes. The remaining fields are load epoch, invalid-data-zeroed
    /// postimage bytes, one-byte validity flags, and touch generations. A
    /// uniform generation uses the distinct v2 domain and binds the generation
    /// count plus one generation value; a nonuniform generation sequence keeps
    /// the v1 per-byte big-endian encoding. Callers retain structural validation
    /// of equal field lengths and zero normalized bytes for invalid lanes.
    pub fn physical_tmem_content_digest(
        byte_count: u32,
        load_epoch: u64,
        normalized_bytes: &[u8],
        validity: &[u8],
        last_touched_generation: &[u64],
    ) -> ContentDigest {
        if uniform_tmem_digest_enabled()
            && !last_touched_generation.is_empty()
            && last_touched_generation
                .iter()
                .all(|generation| *generation == last_touched_generation[0])
        {
            return physical_tmem_uniform_content_digest(
                byte_count,
                load_epoch,
                normalized_bytes,
                validity,
                last_touched_generation[0],
            );
        }
        let mut hash = physical_tmem_content_hasher(
            byte_count,
            load_epoch,
            normalized_bytes,
            validity,
            last_touched_generation.len(),
        );
        let mut encoded = [0_u8; 512];
        for generations in last_touched_generation.chunks(encoded.len() / 8) {
            for (destination, generation) in
                encoded.chunks_exact_mut(8).zip(generations.iter().copied())
            {
                destination.copy_from_slice(&generation.to_be_bytes());
            }
            hash.update(&encoded[..generations.len() * 8]);
        }
        ContentDigest::from_bytes(hash.finalize().into())
    }

    /// Hashes the same canonical physical-TMEM effect preimage when every
    /// projected byte was touched by one generation.
    ///
    /// Under the default v2 encoding, the SHA-256 preimage binds the generation
    /// count and one generation value instead of repeating the same eight bytes
    /// once per projected byte. The v1 control retains that repeated encoding.
    /// Callers still own the structural proof that every projected byte has the
    /// supplied generation.
    pub fn physical_tmem_content_digest_uniform_generation(
        byte_count: u32,
        load_epoch: u64,
        normalized_bytes: &[u8],
        validity: &[u8],
        last_touched_generation: u64,
    ) -> ContentDigest {
        if uniform_tmem_digest_enabled() && !normalized_bytes.is_empty() {
            return physical_tmem_uniform_content_digest(
                byte_count,
                load_epoch,
                normalized_bytes,
                validity,
                last_touched_generation,
            );
        }
        let generation_count = normalized_bytes.len();
        let mut hash = physical_tmem_content_hasher(
            byte_count,
            load_epoch,
            normalized_bytes,
            validity,
            generation_count,
        );
        let generation = last_touched_generation.to_be_bytes();
        let mut encoded = [0_u8; 512];
        for destination in encoded.chunks_exact_mut(8) {
            destination.copy_from_slice(&generation);
        }
        let generations_per_block = encoded.len() / 8;
        let full_blocks = generation_count / generations_per_block;
        for _ in 0..full_blocks {
            hash.update(&encoded);
        }
        let remaining = generation_count % generations_per_block;
        hash.update(&encoded[..remaining * 8]);
        ContentDigest::from_bytes(hash.finalize().into())
    }

    pub fn try_new(
        access: ResourceAccess,
        byte_count: u32,
        content: ContentDigest,
    ) -> Result<Self, ValidationError> {
        if !access.mode().writes() {
            return Err(ValidationError::EffectForReadOnlyAccess);
        }
        let expected = access.region().declared_bytes();
        if byte_count != expected {
            return Err(ValidationError::EffectByteCountMismatch {
                expected,
                actual: byte_count,
            });
        }
        Ok(Self {
            access,
            byte_count,
            content,
        })
    }

    pub fn try_from_bytes(access: ResourceAccess, bytes: &[u8]) -> Result<Self, ValidationError> {
        let byte_count =
            u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
                field: "completed effect byte length",
            })?;
        Self::try_new(access, byte_count, effect_content_digest(bytes))
    }

    pub const fn access(self) -> ResourceAccess {
        self.access
    }

    pub const fn byte_count(self) -> u32 {
        self.byte_count
    }

    pub const fn content(self) -> ContentDigest {
        self.content
    }
}

fn uniform_tmem_digest_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("FN64_TMEM_UNIFORM_DIGEST_V2") {
        Ok(value) if value == "0" => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!("FN64_TMEM_UNIFORM_DIGEST_V2 must be exactly 0 or 1, got {value:?}"),
        Err(std::env::VarError::NotPresent) => true,
        Err(error) => panic!("FN64_TMEM_UNIFORM_DIGEST_V2 is not valid Unicode: {error}"),
    })
}

fn physical_tmem_uniform_content_digest(
    byte_count: u32,
    load_epoch: u64,
    normalized_bytes: &[u8],
    validity: &[u8],
    generation: u64,
) -> ContentDigest {
    let mut hash = Sha256::new();
    hash.update(CompletedWrite::PHYSICAL_TMEM_UNIFORM_CONTENT_DOMAIN);
    let byte_count = byte_count.to_be_bytes();
    let load_epoch = load_epoch.to_be_bytes();
    let generation_count = (normalized_bytes.len() as u64).to_be_bytes();
    let generation = generation.to_be_bytes();
    for field in [
        byte_count.as_slice(),
        load_epoch.as_slice(),
        normalized_bytes,
        validity,
        generation_count.as_slice(),
        generation.as_slice(),
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    ContentDigest::from_bytes(hash.finalize().into())
}

fn physical_tmem_content_hasher(
    byte_count: u32,
    load_epoch: u64,
    normalized_bytes: &[u8],
    validity: &[u8],
    generation_count: usize,
) -> Sha256 {
    let mut hash = Sha256::new();
    hash.update(CompletedWrite::PHYSICAL_TMEM_CONTENT_DOMAIN);
    let byte_count = byte_count.to_be_bytes();
    let load_epoch = load_epoch.to_be_bytes();
    for field in [
        byte_count.as_slice(),
        load_epoch.as_slice(),
        normalized_bytes,
        validity,
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    let generation_bytes = generation_count
        .checked_mul(8)
        .expect("physical TMEM generation field length overflow");
    hash.update((generation_bytes as u64).to_be_bytes());
    hash
}

#[derive(Debug, PartialEq, Eq)]
pub struct BackendEffectReport {
    workload: WorkloadIdentity,
    writes: Box<[CompletedWrite]>,
    identity: EffectIdentity,
}

impl BackendEffectReport {
    /// Retain this packet's exact write contract while execution is deferred.
    ///
    /// The returned capability is move-only and contains no packet payload or
    /// guest bytes. It exists for transaction-scoped backends which inspect a
    /// packet while it is lent through an execution view, submit several
    /// packets together, and only then receive the completed bytes needed to
    /// construct [`CompletedWrite`] values. Completion still runs the same
    /// access-for-access validation as [`Self::try_new`].
    pub fn defer(packet: &WorkloadPacket) -> DeferredBackendEffectReport {
        DeferredBackendEffectReport {
            workload: packet.identity(),
            expected_writes: packet.journal().write_accesses().collect(),
        }
    }

    pub fn try_new(
        packet: &WorkloadPacket,
        writes: Vec<CompletedWrite>,
    ) -> Result<Self, ValidationError> {
        validate_effects(packet.journal().write_accesses(), &writes, "backend effect")?;
        Ok(Self {
            workload: packet.identity(),
            identity: hash_effects(b"fn64.render-ir.backend-effects.v1\0", &writes),
            writes: writes.into_boxed_slice(),
        })
    }

    pub fn writes(&self) -> &[CompletedWrite] {
        &self.writes
    }

    pub const fn identity(&self) -> EffectIdentity {
        self.identity
    }
}

/// Move-only authority to complete one packet's backend effect report after
/// an asynchronous or batched executor has produced its exact writes.
///
/// It deliberately retains only the packet identity and its ordered write
/// accesses. Command streams, guest reads, and publication authority remain
/// owned by their existing lifecycle roles.
#[derive(Debug, PartialEq, Eq)]
pub struct DeferredBackendEffectReport {
    workload: WorkloadIdentity,
    expected_writes: Box<[ResourceAccess]>,
}

impl DeferredBackendEffectReport {
    pub fn complete(
        self,
        writes: Vec<CompletedWrite>,
    ) -> Result<BackendEffectReport, ValidationError> {
        validate_effects(
            self.expected_writes.iter().copied(),
            &writes,
            "backend effect",
        )?;
        Ok(BackendEffectReport {
            workload: self.workload,
            identity: hash_effects(b"fn64.render-ir.backend-effects.v1\0", &writes),
            writes: writes.into_boxed_slice(),
        })
    }

    pub const fn workload(&self) -> WorkloadIdentity {
        self.workload
    }

    pub fn expected_writes(&self) -> &[ResourceAccess] {
        &self.expected_writes
    }
}

/// Backend-held role capability. It can issue completion evidence only for a
/// submission published by its paired queue.
#[derive(Debug)]
pub struct BackendCompletionAuthority {
    queue: QueueIdentity,
}

impl BackendCompletionAuthority {
    /// Queue identity this role is authorized to complete.
    ///
    /// Exposes an identity for exact pairing checks at a sealed unseal
    /// boundary, not receipt-issuing authority.
    pub const fn queue_identity(&self) -> QueueIdentity {
        self.queue
    }

    pub fn issue(
        &mut self,
        submitted: &SubmittedTicket,
        effects: BackendEffectReport,
    ) -> Result<GpuCompletionReceipt, ValidationError> {
        if submitted.queue != self.queue {
            return Err(ValidationError::ReceiptAuthorityMismatch);
        }
        if effects.workload != submitted.packet.identity() {
            return Err(ValidationError::ReceiptEffectMismatch);
        }
        Ok(GpuCompletionReceipt {
            queue: self.queue,
            workload: submitted.packet.identity(),
            submission: submitted.identity,
            journal: submitted.packet.journal().identity(),
            effects,
        })
    }
}

/// Completion receipts are move-only.
///
/// ```compile_fail
/// use fn64_render_ir::GpuCompletionReceipt;
/// # fn receipt() -> GpuCompletionReceipt { unimplemented!() }
/// let receipt = receipt();
/// let copied = receipt.clone();
/// # drop(copied);
/// ```
#[derive(Debug)]
pub struct GpuCompletionReceipt {
    queue: QueueIdentity,
    workload: WorkloadIdentity,
    submission: SubmissionIdentity,
    journal: JournalIdentity,
    effects: BackendEffectReport,
}

/// Sole owner after all declared backend writes complete and before
/// guest-visible writes are committed.
#[derive(Debug)]
pub struct GpuCompleteTicket {
    packet: WorkloadPacket,
    queue: QueueIdentity,
    ordinal: u64,
    submission: SubmissionIdentity,
    effects: BackendEffectReport,
}

impl GpuCompleteTicket {
    pub const fn packet(&self) -> &WorkloadPacket {
        &self.packet
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
        self.effects.identity
    }

    pub fn backend_writes(&self) -> &[CompletedWrite] {
        &self.effects.writes
    }

    pub fn commit_guest(
        self,
        receipt: GuestCommitReceipt,
    ) -> Result<GuestCommittedTicket, ValidationError> {
        validate_receipt_header(
            ReceiptHeader {
                queue: self.queue,
                workload: self.packet.identity(),
                submission: self.submission,
                journal: self.packet.journal().guest_write_identity(),
            },
            ReceiptHeader {
                queue: receipt.queue,
                workload: receipt.workload,
                submission: receipt.submission,
                journal: receipt.guest_writes,
            },
        )?;
        if receipt.backend_effects != self.effects.identity
            || receipt.effects.workload != self.packet.identity()
        {
            return Err(ValidationError::ReceiptEffectMismatch);
        }
        Ok(GuestCommittedTicket {
            packet: self.packet,
            queue: self.queue,
            ordinal: self.ordinal,
            submission: self.submission,
            backend_effects: self.effects.identity,
            guest_effects: receipt.effects.identity,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct GuestCommitEffectReport {
    workload: WorkloadIdentity,
    writes: Box<[CompletedWrite]>,
    identity: EffectIdentity,
}

impl GuestCommitEffectReport {
    pub fn try_new(
        ticket: &GpuCompleteTicket,
        writes: Vec<CompletedWrite>,
    ) -> Result<Self, ValidationError> {
        validate_effects(
            ticket.packet.journal().guest_write_accesses(),
            &writes,
            "guest commit access",
        )?;
        let expected = ticket
            .effects
            .writes
            .iter()
            .copied()
            .filter(|effect| effect.access.region().is_guest_visible());
        validate_completed_effects(expected, &writes, "guest commit effect")?;
        Ok(Self {
            workload: ticket.packet.identity(),
            identity: hash_effects(b"fn64.render-ir.guest-commit-effects.v1\0", &writes),
            writes: writes.into_boxed_slice(),
        })
    }

    pub fn writes(&self) -> &[CompletedWrite] {
        &self.writes
    }

    pub const fn identity(&self) -> EffectIdentity {
        self.identity
    }
}

/// Guest-memory-owner capability. It is distinct from backend completion and
/// can issue a receipt only after exact backend-produced guest effects are
/// supplied in journal order.
#[derive(Debug)]
pub struct GuestCommitAuthority {
    queue: QueueIdentity,
}

impl GuestCommitAuthority {
    /// Queue identity this role is authorized to complete.
    ///
    /// Integration owners use this value to bind a live-memory transaction
    /// to the same lifecycle before any renderer work begins. It exposes an
    /// identity, not receipt-issuing authority.
    pub const fn queue_identity(&self) -> QueueIdentity {
        self.queue
    }

    pub fn issue(
        &mut self,
        complete: &GpuCompleteTicket,
        effects: GuestCommitEffectReport,
    ) -> Result<GuestCommitReceipt, ValidationError> {
        if complete.queue != self.queue {
            return Err(ValidationError::ReceiptAuthorityMismatch);
        }
        if effects.workload != complete.packet.identity() {
            return Err(ValidationError::ReceiptEffectMismatch);
        }
        Ok(GuestCommitReceipt {
            queue: self.queue,
            workload: complete.packet.identity(),
            submission: complete.submission,
            guest_writes: complete.packet.journal().guest_write_identity(),
            backend_effects: complete.effects.identity,
            effects,
        })
    }
}

#[derive(Debug)]
pub struct GuestCommitReceipt {
    queue: QueueIdentity,
    workload: WorkloadIdentity,
    submission: SubmissionIdentity,
    guest_writes: JournalIdentity,
    backend_effects: EffectIdentity,
    effects: GuestCommitEffectReport,
}

#[derive(Debug)]
pub struct GuestCommittedTicket {
    packet: WorkloadPacket,
    queue: QueueIdentity,
    ordinal: u64,
    submission: SubmissionIdentity,
    backend_effects: EffectIdentity,
    guest_effects: EffectIdentity,
}

impl GuestCommittedTicket {
    pub const fn packet(&self) -> &WorkloadPacket {
        &self.packet
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

    pub fn into_packet(self) -> WorkloadPacket {
        self.packet
    }
}

#[derive(Clone, Copy)]
struct ReceiptHeader {
    queue: QueueIdentity,
    workload: WorkloadIdentity,
    submission: SubmissionIdentity,
    journal: JournalIdentity,
}

fn validate_receipt_header(
    expected: ReceiptHeader,
    actual: ReceiptHeader,
) -> Result<(), ValidationError> {
    if actual.queue != expected.queue {
        return Err(ValidationError::ReceiptAuthorityMismatch);
    }
    if actual.workload != expected.workload {
        return Err(ValidationError::ReceiptWorkloadMismatch {
            expected: expected.workload,
            actual: actual.workload,
        });
    }
    if actual.submission != expected.submission {
        return Err(ValidationError::ReceiptSubmissionMismatch);
    }
    if actual.journal != expected.journal {
        return Err(ValidationError::ReceiptJournalMismatch);
    }
    Ok(())
}

fn validate_effects(
    expected: impl Iterator<Item = ResourceAccess>,
    actual: &[CompletedWrite],
    field: &'static str,
) -> Result<(), ValidationError> {
    let expected = expected.collect::<Vec<_>>();
    if expected.len() != actual.len() {
        return Err(ValidationError::EffectCountMismatch {
            field,
            expected: expected.len(),
            actual: actual.len(),
        });
    }
    for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
        if *expected != actual.access {
            return Err(ValidationError::EffectAccessMismatch { field, index });
        }
    }
    Ok(())
}

fn validate_completed_effects(
    expected: impl Iterator<Item = CompletedWrite>,
    actual: &[CompletedWrite],
    field: &'static str,
) -> Result<(), ValidationError> {
    let expected = expected.collect::<Vec<_>>();
    if expected.len() != actual.len() {
        return Err(ValidationError::EffectCountMismatch {
            field,
            expected: expected.len(),
            actual: actual.len(),
        });
    }
    for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
        if expected != actual {
            return Err(ValidationError::EffectAccessMismatch { field, index });
        }
    }
    Ok(())
}

fn hash_effects(domain: &[u8], writes: &[CompletedWrite]) -> EffectIdentity {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update((writes.len() as u32).to_be_bytes());
    for effect in writes {
        hash.update(effect.access.operation().get().to_be_bytes());
        hash.update([effect.access.mode().tag(), effect.access.purpose().tag()]);
        hash_region(&mut hash, effect.access.region());
        hash.update(effect.byte_count.to_be_bytes());
        hash.update(effect.content.as_ref());
    }
    EffectIdentity::new(ContentDigest::from_bytes(hash.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::tests_support::packet;
    use crate::{
        AccessMode, AccessPurpose, HostResource, OperationId, ResourceJournal,
        ResourceJournalLimits, ResourceRegion, TmemRange,
    };
    use core::num::NonZeroU32;

    fn effects(packet: &WorkloadPacket, fill: u8) -> Vec<CompletedWrite> {
        packet
            .journal()
            .write_accesses()
            .map(|access| {
                CompletedWrite::try_new(
                    access,
                    access.region().declared_bytes(),
                    ContentDigest::hash(b"test-effect", &[&[fill]]),
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn distinct_roles_advance_effect_bound_state_order() {
        let (mut queue, mut backend, mut commit) =
            TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet(0xe9, 1))).unwrap();
        let backend_effects =
            BackendEffectReport::try_new(submitted.packet(), effects(submitted.packet(), 0x5a))
                .unwrap();
        let gpu_receipt = backend.issue(&submitted, backend_effects).unwrap();
        let complete = submitted.gpu_complete(gpu_receipt).unwrap();
        assert_eq!(complete.queue(), queue.identity());
        assert_eq!(complete.ordinal(), 0);
        let completion_submission = complete.submission();
        let guest_writes = complete
            .backend_writes()
            .iter()
            .copied()
            .filter(|effect| effect.access().region().is_guest_visible())
            .collect::<Vec<_>>();
        let guest_effects =
            GuestCommitEffectReport::try_new(&complete, guest_writes.clone()).unwrap();
        let (_, _, mut wrong_commit) = TicketAuthoritySet::try_new().unwrap().into_roles();
        assert_eq!(
            wrong_commit.issue(&complete, guest_effects).unwrap_err(),
            ValidationError::ReceiptAuthorityMismatch
        );
        let guest_effects = GuestCommitEffectReport::try_new(&complete, guest_writes).unwrap();
        let guest_receipt = commit.issue(&complete, guest_effects).unwrap();
        let committed = complete.commit_guest(guest_receipt).unwrap();
        assert_eq!(committed.queue(), queue.identity());
        assert_eq!(committed.ordinal(), 0);
        assert_eq!(committed.submission(), completion_submission);
        assert_ne!(
            committed.backend_effect_identity(),
            committed.guest_effect_identity()
        );
    }

    #[test]
    fn ready_submission_queue_checks_capacity_without_mutating_and_then_issues_infallibly() {
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let identity_before = queue.identity();
        let ready = queue.try_ready_submission().unwrap();
        assert_eq!(ready.identity(), identity_before);
        // Holding the ready check does not advance the queue's own ordinal.
        let submitted = ready.issue(DecodedTicket::new(packet(0xe9, 1)));
        assert_eq!(submitted.ordinal(), 0);
        assert_eq!(submitted.queue(), identity_before);

        let ready = queue.try_ready_submission().unwrap();
        let submitted = ready.issue(DecodedTicket::new(packet(0xe9, 1)));
        assert_eq!(submitted.ordinal(), 1);
    }

    #[test]
    fn ready_submission_queue_exhaustion_is_reported_before_any_ordinal_is_reserved() {
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        // Drive the queue to the last valid ordinal without exhausting it.
        queue.next_ordinal = u64::MAX;
        assert_eq!(
            queue.try_ready_submission().unwrap_err(),
            ValidationError::SubmissionOrdinalExhausted {
                queue: queue.identity().get(),
            }
        );
        // The failed capacity check did not mutate the queue: it can still
        // report the identical exhaustion on a later, independent call.
        assert_eq!(
            queue.try_ready_submission().unwrap_err(),
            ValidationError::SubmissionOrdinalExhausted {
                queue: queue.identity().get(),
            }
        );
    }

    #[test]
    fn same_content_submissions_receive_distinct_uncaller_chosen_identities() {
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let a = queue.submit(DecodedTicket::new(packet(0xe9, 1))).unwrap();
        let b = queue.submit(DecodedTicket::new(packet(0xe9, 1))).unwrap();
        assert_ne!(a.identity(), b.identity());
        assert_eq!(a.ordinal(), 0);
        assert_eq!(b.ordinal(), 1);
    }

    #[test]
    fn authority_from_another_queue_is_rejected() {
        let (mut queue_a, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let (_, mut backend_b, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue_a.submit(DecodedTicket::new(packet(0xe9, 1))).unwrap();
        let report =
            BackendEffectReport::try_new(submitted.packet(), effects(submitted.packet(), 1))
                .unwrap();
        assert_eq!(
            backend_b.issue(&submitted, report).unwrap_err(),
            ValidationError::ReceiptAuthorityMismatch
        );
    }

    #[test]
    fn missing_or_changed_effect_is_rejected_before_receipt() {
        let packet = packet(0xe9, 1);
        assert!(matches!(
            BackendEffectReport::try_new(&packet, Vec::new()),
            Err(ValidationError::EffectCountMismatch { .. })
        ));
        let mut actual = effects(&packet, 1);
        actual[0] = CompletedWrite::try_new(
            actual[0].access(),
            actual[0].byte_count(),
            ContentDigest::hash(b"different", &[]),
        )
        .unwrap();
        let report = BackendEffectReport::try_new(&packet, actual).unwrap();
        assert_ne!(
            report.identity(),
            BackendEffectReport::try_new(&packet, effects(&packet, 1))
                .unwrap()
                .identity()
        );
    }

    #[test]
    fn deferred_backend_effect_retains_the_exact_packet_write_contract() {
        let packet = packet(0xe9, 1);
        let expected_workload = packet.identity();
        let expected_access = packet.journal().write_accesses().next().unwrap();
        let deferred = BackendEffectReport::defer(&packet);

        assert_eq!(deferred.workload(), expected_workload);
        assert_eq!(deferred.expected_writes(), &[expected_access]);
        assert!(matches!(
            deferred.complete(Vec::new()),
            Err(ValidationError::EffectCountMismatch { .. })
        ));

        let deferred = BackendEffectReport::defer(&packet);
        let direct = BackendEffectReport::try_new(&packet, effects(&packet, 1)).unwrap();
        let completed = deferred.complete(effects(&packet, 1)).unwrap();
        assert_eq!(completed.identity(), direct.identity());
        assert_eq!(completed.writes(), direct.writes());
    }

    #[test]
    fn exact_effect_bytes_have_one_canonical_identity() {
        let packet = packet(0xe9, 1);
        let access = packet.journal().write_accesses().next().unwrap();
        let bytes = vec![0x21; access.region().declared_bytes() as usize];
        let effect = CompletedWrite::try_from_bytes(access, &bytes).unwrap();
        assert_eq!(effect.byte_count(), bytes.len() as u32);
        assert_eq!(effect.content(), effect_content_digest(&bytes));

        let short = &bytes[..bytes.len() - 1];
        assert!(matches!(
            CompletedWrite::try_from_bytes(access, short),
            Err(ValidationError::EffectByteCountMismatch { .. })
        ));
    }

    #[test]
    fn physical_tmem_effect_content_has_one_renderer_neutral_frozen_identity() {
        let normalized = [0x10, 0x11, 0, 0];
        let validity = [1, 1, 0, 0];
        let touched = [9, 9, 9, 9];
        let backend_a =
            CompletedWrite::physical_tmem_content_digest(4, 7, &normalized, &validity, &touched);
        let backend_b = CompletedWrite::physical_tmem_content_digest_uniform_generation(
            4,
            7,
            &normalized,
            &validity,
            9,
        );

        assert_eq!(
            CompletedWrite::PHYSICAL_TMEM_UNIFORM_CONTENT_DOMAIN,
            b"fn64.render-ir.physical-tmem-effect-content.uniform.v2\0"
        );
        assert_eq!(backend_a, backend_b);
        assert_eq!(
            backend_a.to_string(),
            "a3ccf2cedce557e41b997ca65e4343ac5355cc8db31d39ba4eecee8d58ac5fcf"
        );
    }

    #[test]
    fn uniform_physical_tmem_generation_matches_slice_encoding_at_boundaries() {
        for (len, generation) in [
            (1_usize, 1_u64),
            (8, u64::MAX),
            (64, 0x0102_0304_0506_0708),
            (65, 9),
            (1_664, 17),
            (4_096, 23),
        ] {
            let normalized = (0..len)
                .map(|index| if index % 5 == 0 { 0 } else { index as u8 })
                .collect::<Vec<_>>();
            let validity = (0..len)
                .map(|index| u8::from(index % 5 != 0))
                .collect::<Vec<_>>();
            let touched = vec![generation; len];
            assert_eq!(
                CompletedWrite::physical_tmem_content_digest(
                    len as u32,
                    7,
                    &normalized,
                    &validity,
                    &touched,
                ),
                CompletedWrite::physical_tmem_content_digest_uniform_generation(
                    len as u32,
                    7,
                    &normalized,
                    &validity,
                    generation,
                )
            );
        }
    }

    #[test]
    fn uniform_physical_tmem_generation_does_not_collapse_nonuniform_input() {
        let normalized = [0x10, 0x11, 0x12, 0x13];
        let validity = [1, 1, 1, 1];
        let nonuniform = [9, 9, 10, 9];
        assert_ne!(
            CompletedWrite::physical_tmem_content_digest(4, 7, &normalized, &validity, &nonuniform,),
            CompletedWrite::physical_tmem_content_digest_uniform_generation(
                4,
                7,
                &normalized,
                &validity,
                9,
            )
        );
    }

    #[test]
    fn compressed_uniform_physical_tmem_identity_binds_every_canonical_field() {
        let normalized = [0x10, 0x11, 0, 0];
        let validity = [1, 1, 0, 0];
        let baseline = physical_tmem_uniform_content_digest(4, 7, &normalized, &validity, 9);

        assert_eq!(
            CompletedWrite::PHYSICAL_TMEM_UNIFORM_CONTENT_DOMAIN,
            b"fn64.render-ir.physical-tmem-effect-content.uniform.v2\0"
        );
        assert_eq!(
            baseline,
            physical_tmem_uniform_content_digest(4, 7, &normalized, &validity, 9)
        );
        assert_ne!(
            baseline,
            physical_tmem_uniform_content_digest(4, 8, &normalized, &validity, 9)
        );
        assert_ne!(
            baseline,
            physical_tmem_uniform_content_digest(4, 7, &[0x10, 0x12, 0, 0], &validity, 9)
        );
        assert_ne!(
            baseline,
            physical_tmem_uniform_content_digest(4, 7, &normalized, &[1, 0, 0, 0], 9)
        );
        assert_ne!(
            baseline,
            physical_tmem_uniform_content_digest(4, 7, &normalized, &validity, 10)
        );
    }

    #[test]
    fn backend_covers_every_write_while_guest_commit_covers_every_guest_write() {
        let base = packet(0xe6, 1);
        let mut accesses = base.journal().accesses().to_vec();
        accesses.push(
            ResourceAccess::try_new(
                OperationId::new(2),
                AccessMode::Write,
                AccessPurpose::TmemLoadDestination,
                ResourceRegion::Tmem(TmemRange::try_new(0, 8).unwrap()),
            )
            .unwrap(),
        );
        accesses.push(
            ResourceAccess::try_new(
                OperationId::new(3),
                AccessMode::Write,
                AccessPurpose::CaptureDestination,
                ResourceRegion::Host(HostResource::capture(7, NonZeroU32::new(8).unwrap())),
            )
            .unwrap(),
        );
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x200).unwrap(), accesses)
                .unwrap();
        let packet = WorkloadPacket::try_new(
            base.memory_layout(),
            base.admission(),
            base.streams().to_vec(),
            journal,
        )
        .unwrap();

        let all_writes = effects(&packet, 4);
        assert_eq!(all_writes.len(), 3);
        assert!(matches!(
            BackendEffectReport::try_new(&packet, all_writes[..2].to_vec()),
            Err(ValidationError::EffectCountMismatch { .. })
        ));

        let (mut queue, mut backend, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        let report = BackendEffectReport::try_new(submitted.packet(), all_writes).unwrap();
        let receipt = backend.issue(&submitted, report).unwrap();
        let complete = submitted.gpu_complete(receipt).unwrap();
        let guest_writes = complete
            .backend_writes()
            .iter()
            .copied()
            .filter(|effect| effect.access().region().is_guest_visible())
            .collect::<Vec<_>>();
        assert_eq!(guest_writes.len(), 1);
        assert_eq!(
            GuestCommitEffectReport::try_new(&complete, Vec::new()).unwrap_err(),
            ValidationError::EffectCountMismatch {
                field: "guest commit access",
                expected: 1,
                actual: 0,
            }
        );
        assert!(GuestCommitEffectReport::try_new(&complete, guest_writes).is_ok());
    }
}
