//! Bounded owned guest reads selected by renderer semantics.
//!
//! The renderer derives an exact ordered plan from `TmemLoadSource` journal
//! operations. The guest-memory owner captures only those physical ranges;
//! packet finalization proves the capture still matches the plan one for one
//! before any retained workload exists.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::{
    AccessPurpose, ContentDigest, FastContentDigest, GuestReadPlanIdentity, GuestReadSetIdentity,
    JournalIdentity, OperationId, PhysicalMemoryLayout, PhysicalRange, RdramResource,
    ResourceJournal, ResourceRegion, ValidationError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommandCompletionMoment {
    stream_index: u32,
    command_end_byte_offset: u32,
}

impl CommandCompletionMoment {
    pub const fn new(stream_index: u32, command_end_byte_offset: u32) -> Self {
        Self {
            stream_index,
            command_end_byte_offset,
        }
    }

    pub const fn stream_index(self) -> u32 {
        self.stream_index
    }

    pub const fn command_end_byte_offset(self) -> u32 {
        self.command_end_byte_offset
    }
}

/// When the guest-memory owner must observe one deferred RDRAM read.
///
/// Generic packets retain the historical one-snapshot capture boundary.
/// Sealed raw-DPC plans instead bind a command-completion point that the ABI
/// can resolve against its own physical DPC END timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GuestReadMoment {
    PacketSnapshot,
    CommandCompletion(CommandCompletionMoment),
}

/// One exact journal access bound to the command-completion point that caused
/// its guest read. The plan constructor validates these one-for-one and in
/// journal projection order; an index cannot silently stand in for a
/// different access with the same range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GuestReadCommandMoment {
    access_index: u32,
    operation: OperationId,
    moment: CommandCompletionMoment,
}

impl GuestReadCommandMoment {
    pub const fn new(
        access_index: u32,
        operation: OperationId,
        moment: CommandCompletionMoment,
    ) -> Self {
        Self {
            access_index,
            operation,
            moment,
        }
    }

    pub const fn access_index(self) -> u32 {
        self.access_index
    }

    pub const fn operation(self) -> OperationId {
        self.operation
    }

    pub const fn moment(self) -> CommandCompletionMoment {
        self.moment
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeferredGuestRead {
    access_index: u32,
    operation: OperationId,
    resource: RdramResource,
    range: PhysicalRange,
    moment: GuestReadMoment,
}

impl DeferredGuestRead {
    pub(crate) const fn new(
        access_index: u32,
        operation: OperationId,
        resource: RdramResource,
        range: PhysicalRange,
        moment: GuestReadMoment,
    ) -> Self {
        Self {
            access_index,
            operation,
            resource,
            range,
            moment,
        }
    }

    pub const fn access_index(self) -> u32 {
        self.access_index
    }

    pub const fn operation(self) -> OperationId {
        self.operation
    }

    pub const fn resource(self) -> RdramResource {
        self.resource
    }

    pub const fn range(self) -> PhysicalRange {
        self.range
    }

    pub const fn moment(self) -> GuestReadMoment {
        self.moment
    }
}

/// Renderer-neutral exact read plan. It carries no memory authority and no
/// bytes; its order is the source journal's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredGuestReadPlan {
    memory_layout: PhysicalMemoryLayout,
    journal: JournalIdentity,
    identity: GuestReadPlanIdentity,
    reads: Box<[DeferredGuestRead]>,
    total_bytes: u64,
}

impl DeferredGuestReadPlan {
    pub fn try_from_journal(
        memory_layout: PhysicalMemoryLayout,
        journal: &ResourceJournal,
    ) -> Result<Self, ValidationError> {
        Self::try_from_journal_impl(memory_layout, journal, None)
    }

    pub fn try_from_journal_with_command_moments(
        memory_layout: PhysicalMemoryLayout,
        journal: &ResourceJournal,
        moments: &[GuestReadCommandMoment],
    ) -> Result<Self, ValidationError> {
        Self::try_from_journal_impl(memory_layout, journal, Some(moments))
    }

    fn try_from_journal_impl(
        memory_layout: PhysicalMemoryLayout,
        journal: &ResourceJournal,
        moments: Option<&[GuestReadCommandMoment]>,
    ) -> Result<Self, ValidationError> {
        if !journal.matches_memory_layout(memory_layout) {
            return Err(ValidationError::MemoryLayoutMismatch {
                expected: memory_layout.bytes(),
            });
        }
        if let Some(moments) = moments {
            let expected = journal
                .accesses()
                .iter()
                .filter(|access| access.purpose() == AccessPurpose::TmemLoadSource)
                .count();
            if moments.len() != expected {
                return Err(ValidationError::GuestReadMomentCountMismatch {
                    expected,
                    actual: moments.len(),
                });
            }
        }
        let mut reads = Vec::new();
        let mut total_bytes = 0_u64;
        for (access_index, access) in journal.accesses().iter().copied().enumerate() {
            if access.purpose() != AccessPurpose::TmemLoadSource {
                continue;
            }
            let ResourceRegion::Rdram { resource, range } = access.region() else {
                return Err(ValidationError::DeferredGuestReadUnsupportedRegion {
                    access_index,
                    operation: access.operation().get(),
                });
            };
            total_bytes = total_bytes
                .checked_add(u64::from(range.len()))
                .ok_or(ValidationError::DeclaredResourceBytesOverflow)?;
            let access_index =
                u32::try_from(access_index).map_err(|_| ValidationError::NumericOverflow {
                    field: "deferred guest-read access index",
                })?;
            let moment = if let Some(moments) = moments {
                let binding = &moments[reads.len()];
                if binding.access_index != access_index || binding.operation != access.operation() {
                    return Err(ValidationError::GuestReadMomentDescriptorMismatch {
                        index: reads.len(),
                    });
                }
                GuestReadMoment::CommandCompletion(binding.moment)
            } else {
                GuestReadMoment::PacketSnapshot
            };
            reads.push(DeferredGuestRead::new(
                access_index,
                access.operation(),
                resource,
                range,
                moment,
            ));
        }
        let identity = plan_identity(memory_layout, journal.identity(), &reads);
        Ok(Self {
            memory_layout,
            journal: journal.identity(),
            identity,
            reads: reads.into_boxed_slice(),
            total_bytes,
        })
    }

    pub const fn memory_layout(&self) -> PhysicalMemoryLayout {
        self.memory_layout
    }

    pub const fn journal_identity(&self) -> JournalIdentity {
        self.journal
    }

    pub const fn identity(&self) -> GuestReadPlanIdentity {
        self.identity
    }

    pub fn reads(&self) -> &[DeferredGuestRead] {
        &self.reads
    }

    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// One independently owned logical-byte capture. This is data, not guest
/// memory authority; finalization still has to match it to the plan.
#[derive(PartialEq, Eq)]
pub struct CapturedGuestRead {
    read: DeferredGuestRead,
    fast_content: FastContentDigest,
    bytes: Arc<[u8]>,
}

/// One immutable logical-byte payload captured for an exact physical range.
///
/// A task may bind the same physical bytes to several ordered read
/// descriptors. This value computes the content identity once and shares the
/// owned bytes without weakening each descriptor's separate validation.
pub struct CapturedGuestReadPayload {
    range: PhysicalRange,
    fast_content: FastContentDigest,
    bytes: Arc<[u8]>,
}

impl core::fmt::Debug for CapturedGuestReadPayload {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CapturedGuestReadPayload")
            .field("range", &self.range)
            .field("fast_content", &self.fast_content)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl CapturedGuestReadPayload {
    pub fn try_new(read: DeferredGuestRead, bytes: Vec<u8>) -> Result<Self, ValidationError> {
        let actual = captured_byte_count(bytes.len())?;
        if actual != read.range.len() {
            return Err(ValidationError::GuestReadByteCountMismatch {
                index: usize::try_from(read.access_index)
                    .expect("guest-read access index exceeds host indexing"),
                expected: read.range.len(),
                actual,
            });
        }
        let fast_content = guest_read_fast_content_digest(&bytes);
        Ok(Self {
            range: read.range,
            fast_content,
            bytes: bytes.into(),
        })
    }

    pub const fn range(&self) -> PhysicalRange {
        self.range
    }
}

impl core::fmt::Debug for CapturedGuestRead {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CapturedGuestRead")
            .field("read", &self.read)
            .field("fast_content", &self.fast_content)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl CapturedGuestRead {
    /// Capture bytes we just read ourselves, hashing them ONCE.
    ///
    /// This deliberately does not route through
    /// [`Self::try_new_with_digest`]. That constructor exists to verify a
    /// digest claimed by *someone else*, and it does so by recomputing the
    /// digest from the bytes. Handing it a digest we just computed from the
    /// same bytes made the comparison vacuous -- it compared a value to
    /// itself and could never fail -- while hashing every payload twice.
    ///
    /// The length check below is the part of `try_new_with_digest` that is
    /// NOT vacuous here, so it is kept.
    pub fn try_new(read: DeferredGuestRead, bytes: Vec<u8>) -> Result<Self, ValidationError> {
        let payload = CapturedGuestReadPayload::try_new(read, bytes)?;
        Self::try_from_payload(read, &payload)
    }

    pub fn try_from_payload(
        read: DeferredGuestRead,
        payload: &CapturedGuestReadPayload,
    ) -> Result<Self, ValidationError> {
        if read.range != payload.range {
            return Err(ValidationError::GuestReadDescriptorMismatch {
                index: usize::try_from(read.access_index)
                    .expect("guest-read access index exceeds host indexing"),
            });
        }
        Ok(Self {
            read,
            bytes: Arc::clone(&payload.bytes),
            fast_content: payload.fast_content,
        })
    }

    pub fn try_new_with_digest(
        read: DeferredGuestRead,
        bytes: Vec<u8>,
        claimed_content: ContentDigest,
    ) -> Result<Self, ValidationError> {
        let actual = u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
            field: "captured guest-read byte length",
        })?;
        if actual != read.range.len() {
            return Err(ValidationError::GuestReadByteCountMismatch {
                index: usize::try_from(read.access_index)
                    .expect("guest-read access index exceeds host indexing"),
                expected: read.range.len(),
                actual,
            });
        }
        let actual_content = guest_read_content_digest(&bytes);
        if claimed_content != actual_content {
            return Err(ValidationError::GuestReadDigestMismatch {
                index: usize::try_from(read.access_index)
                    .expect("guest-read access index exceeds host indexing"),
            });
        }
        Ok(Self {
            read,
            fast_content: guest_read_fast_content_digest(&bytes),
            bytes: bytes.into(),
        })
    }

    pub const fn read(&self) -> DeferredGuestRead {
        self.read
    }

    pub const fn fast_content(&self) -> FastContentDigest {
        self.fast_content
    }

    /// Compute the stable serialized SHA-256 identity on the cold path.
    pub fn content_digest(&self) -> ContentDigest {
        guest_read_content_digest(&self.bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn record_content(&self) -> CapturedGuestReadRecordContent {
        CapturedGuestReadRecordContent {
            fast: self.fast_content,
            bytes: Arc::clone(&self.bytes),
        }
    }
}

fn captured_byte_count(len: usize) -> Result<u32, ValidationError> {
    u32::try_from(len).map_err(|_| ValidationError::NumericOverflow {
        field: "captured guest-read byte length",
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapturedGuestReadRecordContent {
    fast: FastContentDigest,
    bytes: Arc<[u8]>,
}

impl CapturedGuestReadRecordContent {
    pub(crate) const fn fast_content(&self) -> FastContentDigest {
        self.fast
    }

    pub(crate) fn content_digest(&self) -> ContentDigest {
        guest_read_content_digest(&self.bytes)
    }
}

/// Untrusted-but-owned result of one bounded memory-owner capture pass.
/// It is intentionally move-only; only packet finalization can turn it into
/// an admitted read set.
///
/// ```compile_fail
/// use fn64_render_ir::DeferredGuestReadCapture;
/// # fn capture() -> DeferredGuestReadCapture { unimplemented!() }
/// # fn consume(_: DeferredGuestReadCapture) {}
/// let capture = capture();
/// consume(capture);
/// consume(capture);
/// ```
#[derive(PartialEq, Eq)]
pub struct DeferredGuestReadCapture {
    reads: Box<[CapturedGuestRead]>,
}

impl core::fmt::Debug for DeferredGuestReadCapture {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("DeferredGuestReadCapture")
            .field("reads", &self.reads)
            .finish()
    }
}

impl DeferredGuestReadCapture {
    pub fn new(reads: Vec<CapturedGuestRead>) -> Self {
        Self {
            reads: reads.into_boxed_slice(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub fn reads(&self) -> &[CapturedGuestRead] {
        &self.reads
    }
}

/// Packet-owned read set. Its private construction proves exact plan/capture
/// correspondence and the packet can outlive the guest-memory borrow.
#[derive(PartialEq, Eq)]
pub struct OwnedGuestReadSet {
    plan: GuestReadPlanIdentity,
    identity: GuestReadSetIdentity,
    reads: Box<[CapturedGuestRead]>,
    total_bytes: usize,
}

impl OwnedGuestReadSet {
    pub(crate) fn try_finalize(
        plan: DeferredGuestReadPlan,
        capture: DeferredGuestReadCapture,
    ) -> Result<Self, ValidationError> {
        if plan.reads.len() != capture.reads.len() {
            return Err(ValidationError::GuestReadCountMismatch {
                expected: plan.reads.len(),
                actual: capture.reads.len(),
            });
        }
        let mut total_bytes = 0_usize;
        for (index, (expected, actual)) in plan.reads.iter().zip(&capture.reads).enumerate() {
            if expected != &actual.read {
                return Err(ValidationError::GuestReadDescriptorMismatch { index });
            }
            total_bytes = total_bytes.checked_add(actual.bytes.len()).ok_or(
                ValidationError::NumericOverflow {
                    field: "owned guest-read aggregate byte length",
                },
            )?;
        }
        if total_bytes as u64 != plan.total_bytes {
            return Err(ValidationError::GuestReadAggregateByteCountMismatch {
                expected: plan.total_bytes,
                actual: total_bytes as u64,
            });
        }
        let identity = set_identity(plan.identity, plan.journal, &capture.reads);
        Ok(Self {
            plan: plan.identity,
            identity,
            reads: capture.reads,
            total_bytes,
        })
    }

    pub const fn plan_identity(&self) -> GuestReadPlanIdentity {
        self.plan
    }

    pub const fn identity(&self) -> GuestReadSetIdentity {
        self.identity
    }

    pub fn reads(&self) -> &[CapturedGuestRead] {
        &self.reads
    }

    pub const fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

impl core::fmt::Debug for OwnedGuestReadSet {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OwnedGuestReadSet")
            .field("plan", &self.plan)
            .field("identity", &self.identity)
            .field("read_count", &self.reads.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

pub(crate) fn plan_identity(
    memory_layout: PhysicalMemoryLayout,
    journal: JournalIdentity,
    reads: &[DeferredGuestRead],
) -> GuestReadPlanIdentity {
    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.deferred-guest-read-plan.v1\0");
    hash.update(memory_layout.bytes().to_be_bytes());
    hash.update(journal.as_bytes());
    hash.update((reads.len() as u32).to_be_bytes());
    for read in reads {
        hash_read(&mut hash, *read);
    }
    GuestReadPlanIdentity::new(ContentDigest::from_bytes(hash.finalize().into()))
}

pub(crate) fn set_identity(
    plan: GuestReadPlanIdentity,
    journal: JournalIdentity,
    reads: &[CapturedGuestRead],
) -> GuestReadSetIdentity {
    set_identity_from_fast_digests(
        plan,
        journal,
        reads.iter().map(|read| (read.read, read.fast_content())),
    )
}

pub(crate) fn set_identity_from_fast_digests(
    plan: GuestReadPlanIdentity,
    journal: JournalIdentity,
    reads: impl ExactSizeIterator<Item = (DeferredGuestRead, FastContentDigest)>,
) -> GuestReadSetIdentity {
    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.owned-guest-read-set.v1\0");
    hash.update(plan.as_bytes());
    hash.update(journal.as_bytes());
    hash.update((reads.len() as u32).to_be_bytes());
    for read in reads {
        hash_read(&mut hash, read.0);
        hash.update(read.1.as_bytes());
    }
    GuestReadSetIdentity::new(ContentDigest::from_bytes(hash.finalize().into()))
}

pub(crate) fn hash_read(hash: &mut Sha256, read: DeferredGuestRead) {
    hash.update(read.access_index.to_be_bytes());
    hash.update(read.operation.get().to_be_bytes());
    hash.update([read.resource.tag()]);
    hash.update(read.range.layout().bytes().to_be_bytes());
    hash.update(read.range.start().get().to_be_bytes());
    hash.update(read.range.end().to_be_bytes());
    match read.moment {
        // Preserve the pre-epoch identity of generic/non-raw packets. Their
        // historical capture boundary is exactly PacketSnapshot.
        GuestReadMoment::PacketSnapshot => {}
        GuestReadMoment::CommandCompletion(moment) => {
            hash.update(b"\0command-completion\0");
            hash.update(moment.stream_index.to_be_bytes());
            hash.update(moment.command_end_byte_offset.to_be_bytes());
        }
    }
}

fn guest_read_content_digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::hash(b"fn64.render-ir.guest-read-content.v1\0", &[bytes])
}

fn guest_read_fast_content_digest(bytes: &[u8]) -> FastContentDigest {
    FastContentDigest::hash(b"fn64.render-ir.guest-read-content.v1\0", &[bytes])
}

#[cfg(test)]
mod tests {
    use crate::{AccessMode, DmemRange, ResourceAccess, ResourceJournalLimits, TmemRange};

    use super::*;

    fn journal(layout: PhysicalMemoryLayout) -> ResourceJournal {
        ResourceJournal::try_new(
            ResourceJournalLimits::try_new(6, 0x100).unwrap(),
            vec![
                ResourceAccess::try_new(
                    OperationId::new(3),
                    AccessMode::Read,
                    AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: layout.range(0x100, 0x108).unwrap(),
                    },
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(3),
                    AccessMode::Write,
                    AccessPurpose::TmemLoadDestination,
                    ResourceRegion::Tmem(TmemRange::try_new(0, 8).unwrap()),
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(9),
                    AccessMode::Read,
                    AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(0x200, 0x204).unwrap(),
                    },
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn capture(plan: &DeferredGuestReadPlan) -> DeferredGuestReadCapture {
        DeferredGuestReadCapture::new(
            plan.reads()
                .iter()
                .map(|read| {
                    CapturedGuestRead::try_new(
                        *read,
                        vec![read.operation().get() as u8; read.range().len() as usize],
                    )
                    .unwrap()
                })
                .collect(),
        )
    }

    #[test]
    fn plan_is_exactly_the_ordered_tmem_source_projection() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let journal = journal(layout);
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal).unwrap();
        assert_eq!(plan.reads().len(), 2);
        assert_eq!(plan.total_bytes(), 12);
        assert_eq!(plan.reads()[0].access_index(), 0);
        assert_eq!(plan.reads()[0].operation(), OperationId::new(3));
        assert_eq!(plan.reads()[1].access_index(), 2);
        assert_eq!(plan.reads()[1].operation(), OperationId::new(9));
        assert_eq!(plan.reads()[0].moment(), GuestReadMoment::PacketSnapshot);
        assert_eq!(plan.journal_identity(), journal.identity());
    }

    #[test]
    fn command_moments_require_complete_ordered_access_bindings() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let journal = journal(layout);
        let first = GuestReadCommandMoment::new(
            0,
            OperationId::new(3),
            CommandCompletionMoment::new(2, 24),
        );
        let second = GuestReadCommandMoment::new(
            2,
            OperationId::new(9),
            CommandCompletionMoment::new(2, 40),
        );
        let plan = DeferredGuestReadPlan::try_from_journal_with_command_moments(
            layout,
            &journal,
            &[first, second],
        )
        .unwrap();
        assert_eq!(
            plan.reads()[0].moment(),
            GuestReadMoment::CommandCompletion(first.moment())
        );
        assert_eq!(
            DeferredGuestReadPlan::try_from_journal_with_command_moments(
                layout,
                &journal,
                &[first],
            )
            .unwrap_err(),
            ValidationError::GuestReadMomentCountMismatch {
                expected: 2,
                actual: 1,
            }
        );
        assert_eq!(
            DeferredGuestReadPlan::try_from_journal_with_command_moments(
                layout,
                &journal,
                &[second, first],
            )
            .unwrap_err(),
            ValidationError::GuestReadMomentDescriptorMismatch { index: 0 }
        );
    }

    #[test]
    fn command_moment_mutation_changes_plan_capture_and_set_identity() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let journal = journal(layout);
        let bindings = |end| {
            [
                GuestReadCommandMoment::new(
                    0,
                    OperationId::new(3),
                    CommandCompletionMoment::new(1, end),
                ),
                GuestReadCommandMoment::new(
                    2,
                    OperationId::new(9),
                    CommandCompletionMoment::new(1, 40),
                ),
            ]
        };
        let first = DeferredGuestReadPlan::try_from_journal_with_command_moments(
            layout,
            &journal,
            &bindings(24),
        )
        .unwrap();
        let mutated = DeferredGuestReadPlan::try_from_journal_with_command_moments(
            layout,
            &journal,
            &bindings(25),
        )
        .unwrap();
        assert_ne!(first.identity(), mutated.identity());
        let first_capture = capture(&first);
        let mutated_capture = capture(&mutated);
        assert_ne!(first_capture.reads()[0], mutated_capture.reads()[0]);
        let first_set = OwnedGuestReadSet::try_finalize(first, first_capture).unwrap();
        let mutated_set = OwnedGuestReadSet::try_finalize(mutated, mutated_capture).unwrap();
        assert_ne!(first_set.identity(), mutated_set.identity());
    }

    #[test]
    fn one_range_bound_payload_can_back_distinct_read_descriptors() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let first = plan.reads()[0];
        let later = DeferredGuestRead::new(
            5,
            OperationId::new(11),
            RdramResource::ColorFramebuffer,
            first.range(),
            GuestReadMoment::PacketSnapshot,
        );
        let payload = CapturedGuestReadPayload::try_new(first, vec![0x5a; 8]).unwrap();
        let first_capture = CapturedGuestRead::try_from_payload(first, &payload).unwrap();
        let later_capture = CapturedGuestRead::try_from_payload(later, &payload).unwrap();

        assert_ne!(first_capture.read(), later_capture.read());
        assert_eq!(first_capture.fast_content(), later_capture.fast_content());
        assert!(Arc::ptr_eq(&first_capture.bytes, &later_capture.bytes));

        let other_range = DeferredGuestRead::new(
            6,
            OperationId::new(12),
            RdramResource::Buffer,
            layout.range(0x300, 0x308).unwrap(),
            GuestReadMoment::PacketSnapshot,
        );
        assert_eq!(
            CapturedGuestRead::try_from_payload(other_range, &payload).unwrap_err(),
            ValidationError::GuestReadDescriptorMismatch { index: 6 }
        );
    }

    #[test]
    fn non_rdram_tmem_source_is_a_loud_boundary_error() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 8).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(4),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::RspDmem(DmemRange::try_new(0, 8).unwrap()),
            )
            .unwrap()],
        )
        .unwrap();
        assert_eq!(
            DeferredGuestReadPlan::try_from_journal(layout, &journal).unwrap_err(),
            ValidationError::DeferredGuestReadUnsupportedRegion {
                access_index: 0,
                operation: 4,
            }
        );
    }

    #[test]
    fn missing_extra_and_reordered_capture_never_finalize() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();

        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let mut missing = capture(&plan).reads.into_vec();
        missing.pop();
        assert_eq!(
            OwnedGuestReadSet::try_finalize(plan, DeferredGuestReadCapture::new(missing))
                .unwrap_err(),
            ValidationError::GuestReadCountMismatch {
                expected: 2,
                actual: 1,
            }
        );

        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let mut extra = capture(&plan).reads.into_vec();
        extra.push(CapturedGuestRead::try_new(plan.reads()[0], vec![3; 8]).unwrap());
        assert_eq!(
            OwnedGuestReadSet::try_finalize(plan, DeferredGuestReadCapture::new(extra))
                .unwrap_err(),
            ValidationError::GuestReadCountMismatch {
                expected: 2,
                actual: 3,
            }
        );

        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let mut reordered = capture(&plan).reads.into_vec();
        reordered.swap(0, 1);
        assert_eq!(
            OwnedGuestReadSet::try_finalize(plan, DeferredGuestReadCapture::new(reordered))
                .unwrap_err(),
            ValidationError::GuestReadDescriptorMismatch { index: 0 }
        );
    }

    #[test]
    fn overlapping_or_aliased_descriptor_cannot_replace_the_plan() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let expected = plan.reads()[0];
        let overlapping = DeferredGuestRead::new(
            expected.access_index(),
            expected.operation(),
            expected.resource(),
            layout.range(0x104, 0x10c).unwrap(),
            expected.moment(),
        );
        let mut hostile = capture(&plan).reads.into_vec();
        hostile[0] = CapturedGuestRead::try_new(overlapping, vec![3; 8]).unwrap();
        assert_eq!(
            OwnedGuestReadSet::try_finalize(plan, DeferredGuestReadCapture::new(hostile))
                .unwrap_err(),
            ValidationError::GuestReadDescriptorMismatch { index: 0 }
        );

        assert!(matches!(
            layout.range(0xffc, 0x1004),
            Err(ValidationError::RangeOutOfBounds { .. })
        ));
    }

    #[test]
    fn short_long_and_digest_mutation_fail_before_admission() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let read = plan.reads()[0];
        assert_eq!(
            CapturedGuestRead::try_new(read, vec![0; 7]).unwrap_err(),
            ValidationError::GuestReadByteCountMismatch {
                index: 0,
                expected: 8,
                actual: 7,
            }
        );
        assert_eq!(
            CapturedGuestRead::try_new(read, vec![0; 9]).unwrap_err(),
            ValidationError::GuestReadByteCountMismatch {
                index: 0,
                expected: 8,
                actual: 9,
            }
        );
        assert_eq!(
            CapturedGuestRead::try_new_with_digest(
                read,
                vec![0; 8],
                ContentDigest::from_bytes([0xa5; 32]),
            )
            .unwrap_err(),
            ValidationError::GuestReadDigestMismatch { index: 0 }
        );
    }

    #[test]
    fn capture_uses_fast_runtime_identity_and_retains_stable_serialized_digest() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let read = plan.reads()[0];
        let bytes = vec![0x5a; read.range().len() as usize];
        let capture = CapturedGuestRead::try_new(read, bytes.clone()).unwrap();
        let same = CapturedGuestRead::try_new(read, bytes.clone()).unwrap();
        let different = CapturedGuestRead::try_new(read, vec![0xa5; bytes.len()]).unwrap();

        assert_eq!(capture.fast_content(), same.fast_content());
        assert_ne!(capture.fast_content(), different.fast_content());
        assert_eq!(
            capture.content_digest(),
            ContentDigest::hash(b"fn64.render-ir.guest-read-content.v1\0", &[&bytes])
        );
    }

    #[test]
    fn capture_debug_is_content_silent() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan = DeferredGuestReadPlan::try_from_journal(layout, &journal(layout)).unwrap();
        let capture = capture(&plan);
        let debug = format!("{capture:?}");
        assert!(debug.contains("byte_len"));
        assert!(!debug.contains("[3, 3, 3"));
        assert!(!debug.contains("[9, 9, 9"));
    }
}
