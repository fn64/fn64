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
        let mut hash = Sha256::new();
        hash.update(b"fn64.render-ir.submission.v2\0");
        hash.update(decoded.packet.identity().as_bytes());
        hash.update(self.queue.0.to_be_bytes());
        hash.update(ordinal.to_be_bytes());
        Ok(SubmittedTicket {
            packet: decoded.packet,
            queue: self.queue,
            ordinal,
            identity: SubmissionIdentity(ContentDigest::from_bytes(hash.finalize().into())),
        })
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

    /// Hashes one canonical physical-TMEM effect postimage without lifecycle,
    /// allocation, workload, journal, submission, or access identity.
    ///
    /// `byte_count` is complete declared physical access coverage, including
    /// invalid lanes. The remaining fields are load epoch, invalid-data-zeroed
    /// postimage bytes, one-byte validity flags, and per-byte touch generations
    /// encoded big-endian. Callers retain structural validation of equal field
    /// lengths and zero normalized bytes for invalid lanes.
    pub fn physical_tmem_content_digest(
        byte_count: u32,
        load_epoch: u64,
        normalized_bytes: &[u8],
        validity: &[u8],
        last_touched_generation: &[u64],
    ) -> ContentDigest {
        let byte_count = byte_count.to_be_bytes();
        let load_epoch = load_epoch.to_be_bytes();
        let mut touched = Vec::with_capacity(last_touched_generation.len() * 8);
        for generation in last_touched_generation.iter().copied() {
            touched.extend_from_slice(&generation.to_be_bytes());
        }
        ContentDigest::hash(
            Self::PHYSICAL_TMEM_CONTENT_DOMAIN,
            &[
                &byte_count,
                &load_epoch,
                normalized_bytes,
                validity,
                &touched,
            ],
        )
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

#[derive(Debug, PartialEq, Eq)]
pub struct BackendEffectReport {
    workload: WorkloadIdentity,
    writes: Box<[CompletedWrite]>,
    identity: EffectIdentity,
}

impl BackendEffectReport {
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

/// Backend-held role capability. It can issue completion evidence only for a
/// submission published by its paired queue.
#[derive(Debug)]
pub struct BackendCompletionAuthority {
    queue: QueueIdentity,
}

impl BackendCompletionAuthority {
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
        let backend_b =
            CompletedWrite::physical_tmem_content_digest(4, 7, &normalized, &validity, &touched);

        assert_eq!(
            CompletedWrite::PHYSICAL_TMEM_CONTENT_DOMAIN,
            b"fn64.render-ir.physical-tmem-effect-content.v1\0"
        );
        assert_eq!(backend_a, backend_b);
        assert_eq!(
            backend_a.to_string(),
            "a52cc1514e4e131e59fd4ea3b1e8e0d1c8a65ac9e86878d6a7601646335c1d79"
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
