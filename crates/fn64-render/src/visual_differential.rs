//! Backend-neutral identity for exact raw-DPC visual checkpoints.
//!
//! This is evidence vocabulary, not a backend observation API. A producer
//! supplies already-owned exact boundary data; this module validates whether
//! those facts can support an exact visual checkpoint and hashes only the
//! canonical semantic fields. Renderer, build, and policy identity belong to
//! the surrounding run manifest and deliberately do not enter this digest.

use fn64_render_ir::{ContentDigest, DeferredGuestRead, DeferredGuestReadPlan, GuestReadMoment};
use sha2::{Digest, Sha256};

use crate::{OwnedRawDpcCapture, RawDpcSource, ViScanoutRegisters};

const CHECKPOINT_DOMAIN: &[u8] = b"fn64.raw-dpc-visual-checkpoint.v1\0";

/// Provenance of the command/member boundary supplied to a checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawDpcVisualCaptureSource {
    /// An owned capture taken at the live transaction's exact `CMD_END`.
    ExactLiveTransaction,
    /// A diagnostic reconstruction, including the staged batch adapter.
    DiagnosticReconstruction,
}

/// Device-native color target encoding at the checkpoint boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawDpcVisualTargetFormatV1 {
    Rgba16,
    Rgba32,
}

impl RawDpcVisualTargetFormatV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::Rgba16 => 1,
            Self::Rgba32 => 2,
        }
    }

    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba16 => 2,
            Self::Rgba32 => 4,
        }
    }
}

/// One ordered exact guest read and its canonical logical-byte SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawDpcVisualGuestReadV1 {
    descriptor: DeferredGuestRead,
    content_sha256: ContentDigest,
}

impl RawDpcVisualGuestReadV1 {
    pub const fn new(descriptor: DeferredGuestRead, content_sha256: ContentDigest) -> Self {
        Self {
            descriptor,
            content_sha256,
        }
    }

    pub const fn descriptor(self) -> DeferredGuestRead {
        self.descriptor
    }

    pub const fn content_sha256(self) -> ContentDigest {
        self.content_sha256
    }
}

/// Complete input needed to determine exact-checkpoint readiness.
#[derive(Clone, Copy)]
pub struct RawDpcVisualCheckpointInputV1<'a> {
    pub task_batch_identity: [u8; 32],
    pub member_ordinal: u32,
    pub capture_source: RawDpcVisualCaptureSource,
    pub capture: &'a OwnedRawDpcCapture,
    pub guest_read_plan: &'a DeferredGuestReadPlan,
    pub guest_reads: &'a [RawDpcVisualGuestReadV1],
    /// Complete live VI register authority is a readiness requirement. VI is
    /// consumed by the later post-VI checkpoint and is not part of this raw
    /// DPC semantic digest.
    pub vi_registers: Option<ViScanoutRegisters>,
    pub target_address: u32,
    pub target_width: u32,
    pub target_height: u32,
    pub target_format: RawDpcVisualTargetFormatV1,
    /// Canonical device-order target bytes, without host format conversion.
    pub target_device_bytes: &'a [u8],
    /// One value per target pixel: zero means unknown, 1..=8 is exact N64
    /// hidden coverage.
    pub coverage: &'a [u8],
    /// Complete physical RDRAM after the checkpoint's copyback has committed.
    pub post_copyback_rdram: &'a [u8],
}

/// Why supplied evidence cannot honestly produce an exact checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawDpcVisualCheckpointRefusal {
    NonExactCaptureSource,
    PristineSnapshotReads,
    MissingCompleteViState,
    GuestReadMemoryLayoutMismatch,
    GuestReadCount {
        expected: usize,
        actual: usize,
    },
    GuestReadDescriptorMismatch {
        index: usize,
    },
    TargetGeometryOverflow,
    TargetRangeOutOfBounds {
        address: u32,
        byte_len: usize,
        memory_bytes: u32,
    },
    TargetByteCount {
        expected: usize,
        actual: usize,
    },
    CoverageByteCount {
        expected: usize,
        actual: usize,
    },
    InvalidCoverageCode {
        index: usize,
        code: u8,
    },
    CoverageUnavailable {
        unknown_cells: usize,
    },
    PostCopybackRdramByteCount {
        expected: usize,
        actual: usize,
    },
}

/// Canonical semantic identity of one exact raw-DPC visual checkpoint.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RawDpcVisualCheckpointV1(ContentDigest);

impl RawDpcVisualCheckpointV1 {
    pub const fn digest(self) -> ContentDigest {
        self.0
    }

    pub const fn as_bytes(self) -> [u8; 32] {
        self.0.as_bytes()
    }
}

impl core::fmt::Display for RawDpcVisualCheckpointV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl core::fmt::Debug for RawDpcVisualCheckpointV1 {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "RawDpcVisualCheckpointV1({})", self.0)
    }
}

/// Validate exact readiness and derive the canonical v1 checkpoint identity.
pub fn raw_dpc_visual_checkpoint_v1(
    input: RawDpcVisualCheckpointInputV1<'_>,
) -> Result<RawDpcVisualCheckpointV1, RawDpcVisualCheckpointRefusal> {
    if input.capture_source != RawDpcVisualCaptureSource::ExactLiveTransaction {
        return Err(RawDpcVisualCheckpointRefusal::NonExactCaptureSource);
    }
    if input.guest_read_plan.memory_layout() != input.capture.memory_layout() {
        return Err(RawDpcVisualCheckpointRefusal::GuestReadMemoryLayoutMismatch);
    }
    if input.guest_read_plan.reads().len() != input.guest_reads.len() {
        return Err(RawDpcVisualCheckpointRefusal::GuestReadCount {
            expected: input.guest_read_plan.reads().len(),
            actual: input.guest_reads.len(),
        });
    }
    if let Some(index) = input
        .guest_read_plan
        .reads()
        .iter()
        .zip(input.guest_reads)
        .position(|(expected, actual)| expected != &actual.descriptor)
    {
        return Err(RawDpcVisualCheckpointRefusal::GuestReadDescriptorMismatch { index });
    }
    if input
        .guest_reads
        .iter()
        .any(|read| matches!(read.descriptor.moment(), GuestReadMoment::PacketSnapshot))
    {
        return Err(RawDpcVisualCheckpointRefusal::PristineSnapshotReads);
    }
    if input.vi_registers.is_none() {
        return Err(RawDpcVisualCheckpointRefusal::MissingCompleteViState);
    }

    let pixel_count = usize::try_from(input.target_width)
        .ok()
        .and_then(|width| {
            usize::try_from(input.target_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(RawDpcVisualCheckpointRefusal::TargetGeometryOverflow)?;
    let target_byte_count = pixel_count
        .checked_mul(input.target_format.bytes_per_pixel())
        .ok_or(RawDpcVisualCheckpointRefusal::TargetGeometryOverflow)?;
    if input.target_device_bytes.len() != target_byte_count {
        return Err(RawDpcVisualCheckpointRefusal::TargetByteCount {
            expected: target_byte_count,
            actual: input.target_device_bytes.len(),
        });
    }
    let target_end = usize::try_from(input.target_address)
        .ok()
        .and_then(|address| address.checked_add(target_byte_count))
        .ok_or(RawDpcVisualCheckpointRefusal::TargetGeometryOverflow)?;
    if target_end > input.capture.memory_layout().bytes() as usize {
        return Err(RawDpcVisualCheckpointRefusal::TargetRangeOutOfBounds {
            address: input.target_address,
            byte_len: target_byte_count,
            memory_bytes: input.capture.memory_layout().bytes(),
        });
    }
    if input.coverage.len() != pixel_count {
        return Err(RawDpcVisualCheckpointRefusal::CoverageByteCount {
            expected: pixel_count,
            actual: input.coverage.len(),
        });
    }
    if let Some((index, code)) = input
        .coverage
        .iter()
        .copied()
        .enumerate()
        .find(|(_, code)| *code > 8)
    {
        return Err(RawDpcVisualCheckpointRefusal::InvalidCoverageCode { index, code });
    }
    let unknown_cells = input.coverage.iter().filter(|code| **code == 0).count();
    if unknown_cells != 0 {
        return Err(RawDpcVisualCheckpointRefusal::CoverageUnavailable { unknown_cells });
    }
    let memory_bytes = input.capture.memory_layout().bytes() as usize;
    if input.post_copyback_rdram.len() != memory_bytes {
        return Err(RawDpcVisualCheckpointRefusal::PostCopybackRdramByteCount {
            expected: memory_bytes,
            actual: input.post_copyback_rdram.len(),
        });
    }

    let submission = input.capture.submission().identity();
    let mut hash = Sha256::new();
    hash.update(CHECKPOINT_DOMAIN);
    hash.update(input.task_batch_identity);
    hash.update(input.member_ordinal.to_be_bytes());
    hash.update([match submission.source {
        RawDpcSource::Rdram => 1,
        RawDpcSource::XbusDmem => 2,
    }]);
    hash.update(submission.start.to_be_bytes());
    hash.update(submission.end.to_be_bytes());
    hash.update(submission.command_sha256);
    hash.update(input.capture.memory_layout().bytes().to_be_bytes());
    hash.update(input.capture.transaction_sequence().to_be_bytes());
    hash.update(input.capture.cmd_end().sequence().to_be_bytes());
    hash.update([match input.capture.cmd_end().interrupt() {
        fn64_render_ir::DpInterruptState::Clear => 0,
        fn64_render_ir::DpInterruptState::Asserted => 1,
    }]);
    hash.update(input.guest_read_plan.identity().as_bytes());
    hash.update((input.guest_reads.len() as u32).to_be_bytes());
    for read in input.guest_reads {
        let descriptor = read.descriptor;
        hash.update(descriptor.access_index().to_be_bytes());
        hash.update(descriptor.operation().get().to_be_bytes());
        hash.update(descriptor.range().start().get().to_be_bytes());
        hash.update(descriptor.range().end().to_be_bytes());
        match descriptor.moment() {
            GuestReadMoment::PacketSnapshot => unreachable!("refused before hashing"),
            GuestReadMoment::CommandCompletion(moment) => {
                hash.update([1]);
                hash.update(moment.stream_index().to_be_bytes());
                hash.update(moment.command_end_byte_offset().to_be_bytes());
            }
        }
        hash.update(read.content_sha256.as_bytes());
    }
    hash.update(input.target_address.to_be_bytes());
    hash.update(input.target_width.to_be_bytes());
    hash.update(input.target_height.to_be_bytes());
    hash.update([input.target_format.tag()]);
    hash.update(Sha256::digest(input.target_device_bytes));
    hash.update(Sha256::digest(input.coverage));
    hash.update((unknown_cells as u64).to_be_bytes());
    hash.update(Sha256::digest(input.post_copyback_rdram));
    Ok(RawDpcVisualCheckpointV1(ContentDigest::from_bytes(
        hash.finalize().into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{
        AccessMode, AccessPurpose, CommandCompletionMoment, DpInterruptState,
        GuestReadCommandMoment, OperationId, PhysicalMemoryLayout, RdramResource, ResourceAccess,
        ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
    };

    fn fixture_plan(layout: PhysicalMemoryLayout) -> fn64_render_ir::DeferredGuestReadPlan {
        fixture_plan_with(layout, 7, 0x40, 0, 8, false, RdramResource::Buffer)
    }

    fn fixture_plan_with(
        layout: PhysicalMemoryLayout,
        operation_id: u32,
        range_start: u32,
        stream_index: u32,
        command_end_byte_offset: u32,
        prefix_access: bool,
        resource: RdramResource,
    ) -> fn64_render_ir::DeferredGuestReadPlan {
        let operation = OperationId::new(operation_id);
        let mut accesses = Vec::new();
        if prefix_access {
            accesses.push(
                ResourceAccess::try_new(
                    OperationId::new(1),
                    AccessMode::Read,
                    AccessPurpose::CommandDecode,
                    ResourceRegion::Rdram {
                        resource: RdramResource::RawCommands,
                        range: layout.range(0x20, 0x24).unwrap(),
                    },
                )
                .unwrap(),
            );
        }
        accesses.push(
            ResourceAccess::try_new(
                operation,
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource,
                    range: layout.range(range_start, range_start + 4).unwrap(),
                },
            )
            .unwrap(),
        );
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(2, 16).unwrap(), accesses)
                .unwrap();
        fn64_render_ir::DeferredGuestReadPlan::try_from_journal_with_command_moments(
            layout,
            &journal,
            &[GuestReadCommandMoment::new(
                u32::from(prefix_access),
                operation,
                CommandCompletionMoment::new(stream_index, command_end_byte_offset),
            )],
        )
        .unwrap()
    }

    fn capture(layout: PhysicalMemoryLayout, command_word: u32) -> OwnedRawDpcCapture {
        capture_at(layout, 0x100, command_word, 11, 12, DpInterruptState::Clear)
    }

    fn capture_at(
        layout: PhysicalMemoryLayout,
        start: u32,
        command_word: u32,
        transaction_sequence: u64,
        cmd_end_sequence: u64,
        interrupt: DpInterruptState,
    ) -> OwnedRawDpcCapture {
        let submission =
            crate::OwnedRawDpcSubmission::from_rdram_words(start, start + 8, vec![command_word, 0])
                .unwrap();
        OwnedRawDpcCapture::new(
            submission,
            layout,
            transaction_sequence,
            TemporalBoundary::new(cmd_end_sequence, interrupt),
        )
    }

    fn vi() -> ViScanoutRegisters {
        ViScanoutRegisters::from_words([0; ViScanoutRegisters::WORD_COUNT])
    }

    fn checkpoint(
        task_batch_identity: [u8; 32],
        member_ordinal: u32,
        capture: &OwnedRawDpcCapture,
        plan: &fn64_render_ir::DeferredGuestReadPlan,
        reads: &[RawDpcVisualGuestReadV1],
        target_address: u32,
        target_width: u32,
        target_height: u32,
        target_format: RawDpcVisualTargetFormatV1,
        target: &[u8],
        coverage: &[u8],
        post: &[u8],
    ) -> Result<RawDpcVisualCheckpointV1, RawDpcVisualCheckpointRefusal> {
        raw_dpc_visual_checkpoint_v1(RawDpcVisualCheckpointInputV1 {
            task_batch_identity,
            member_ordinal,
            capture_source: RawDpcVisualCaptureSource::ExactLiveTransaction,
            capture,
            guest_read_plan: plan,
            guest_reads: reads,
            vi_registers: Some(vi()),
            target_address,
            target_width,
            target_height,
            target_format,
            target_device_bytes: target,
            coverage,
            post_copyback_rdram: post,
        })
    }

    #[test]
    fn canonical_hash_binds_every_raw_checkpoint_field() {
        let layout = PhysicalMemoryLayout::try_new(0x200).unwrap();
        let plan = fixture_plan(layout);
        let capture_a = capture(layout, 0xe6_000000);
        let capture_b = capture(layout, 0xe7_000000);
        let read =
            RawDpcVisualGuestReadV1::new(plan.reads()[0], ContentDigest::hash(b"read", &[b"a"]));
        let other_read =
            RawDpcVisualGuestReadV1::new(plan.reads()[0], ContentDigest::hash(b"read", &[b"b"]));
        let target = [1, 2, 3, 4, 5, 6, 7, 8];
        let other_target = [8, 2, 3, 4, 5, 6, 7, 8];
        let coverage = [1, 2, 3, 4];
        let other_coverage = [1, 2, 3, 5];
        let post = vec![0x55; layout.bytes() as usize];
        let mut other_post = post.clone();
        other_post[0] ^= 1;
        let base = checkpoint(
            [1; 32],
            2,
            &capture_a,
            &plan,
            &[read],
            0x80,
            2,
            2,
            RawDpcVisualTargetFormatV1::Rgba16,
            &target,
            &coverage,
            &post,
        )
        .unwrap();

        let variants = [
            checkpoint(
                [2; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                3,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_b,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[other_read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x88,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                1,
                4,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                1,
                RawDpcVisualTargetFormatV1::Rgba32,
                &target,
                &coverage[..2],
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &other_target,
                &coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &other_coverage,
                &post,
            )
            .unwrap(),
            checkpoint(
                [1; 32],
                2,
                &capture_a,
                &plan,
                &[read],
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &other_post,
            )
            .unwrap(),
        ];
        assert!(variants.into_iter().all(|variant| variant != base));
    }

    #[test]
    fn canonical_hash_separately_binds_capture_source_range_layout_and_timeline() {
        let layout = PhysicalMemoryLayout::try_new(0x200).unwrap();
        let wider_layout = PhysicalMemoryLayout::try_new(0x208).unwrap();
        let plan = fixture_plan(layout);
        let wider_plan = fixture_plan(wider_layout);
        let base_capture = capture(layout, 0xe6_000000);
        let changed_captures = [
            capture_at(layout, 0x108, 0xe6_000000, 11, 12, DpInterruptState::Clear),
            capture_at(layout, 0x100, 0xe6_000000, 12, 12, DpInterruptState::Clear),
            capture_at(layout, 0x100, 0xe6_000000, 11, 13, DpInterruptState::Clear),
            capture_at(
                layout,
                0x100,
                0xe6_000000,
                11,
                12,
                DpInterruptState::Asserted,
            ),
        ];
        let xbus = OwnedRawDpcCapture::new(
            crate::OwnedRawDpcSubmission::from_xbus_payload(
                0x100,
                0x108,
                vec![0xe6, 0, 0, 0, 0, 0, 0, 0],
            )
            .unwrap(),
            layout,
            11,
            TemporalBoundary::new(12, DpInterruptState::Clear),
        );
        let digest = ContentDigest::hash(b"read", &[b"a"]);
        let read = [RawDpcVisualGuestReadV1::new(plan.reads()[0], digest)];
        let wider_read = [RawDpcVisualGuestReadV1::new(wider_plan.reads()[0], digest)];
        let target = [0; 8];
        let coverage = [1; 4];
        let post = vec![0; layout.bytes() as usize];
        let wider_post = vec![0; wider_layout.bytes() as usize];
        let base = checkpoint(
            [1; 32],
            0,
            &base_capture,
            &plan,
            &read,
            0x80,
            2,
            2,
            RawDpcVisualTargetFormatV1::Rgba16,
            &target,
            &coverage,
            &post,
        )
        .unwrap();
        let mut variants: Vec<_> = changed_captures
            .iter()
            .chain([&xbus])
            .map(|changed| {
                checkpoint(
                    [1; 32],
                    0,
                    changed,
                    &plan,
                    &read,
                    0x80,
                    2,
                    2,
                    RawDpcVisualTargetFormatV1::Rgba16,
                    &target,
                    &coverage,
                    &post,
                )
                .unwrap()
            })
            .collect();
        let wider_capture = capture(wider_layout, 0xe6_000000);
        variants.push(
            checkpoint(
                [1; 32],
                0,
                &wider_capture,
                &wider_plan,
                &wider_read,
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &wider_post,
            )
            .unwrap(),
        );
        assert!(variants.into_iter().all(|variant| variant != base));
    }

    #[test]
    fn canonical_hash_binds_plan_identity_and_each_ordered_read_descriptor_field() {
        let layout = PhysicalMemoryLayout::try_new(0x200).unwrap();
        let plans = [
            fixture_plan_with(layout, 8, 0x40, 0, 8, false, RdramResource::Buffer),
            fixture_plan_with(layout, 7, 0x44, 0, 8, false, RdramResource::Buffer),
            fixture_plan_with(layout, 7, 0x40, 1, 8, false, RdramResource::Buffer),
            fixture_plan_with(layout, 7, 0x40, 0, 12, false, RdramResource::Buffer),
            fixture_plan_with(layout, 7, 0x40, 0, 8, true, RdramResource::Buffer),
            fixture_plan_with(
                layout,
                7,
                0x40,
                0,
                8,
                false,
                RdramResource::ColorFramebuffer,
            ),
        ];
        let base_plan = fixture_plan(layout);
        let capture = capture(layout, 0xe6_000000);
        let digest = ContentDigest::hash(b"read", &[b"same"]);
        let target = [0; 8];
        let coverage = [1; 4];
        let post = vec![0; layout.bytes() as usize];
        let base_read = [RawDpcVisualGuestReadV1::new(base_plan.reads()[0], digest)];
        let base = checkpoint(
            [1; 32],
            0,
            &capture,
            &base_plan,
            &base_read,
            0x80,
            2,
            2,
            RawDpcVisualTargetFormatV1::Rgba16,
            &target,
            &coverage,
            &post,
        )
        .unwrap();
        for plan in plans {
            let read = [RawDpcVisualGuestReadV1::new(plan.reads()[0], digest)];
            let changed = checkpoint(
                [1; 32],
                0,
                &capture,
                &plan,
                &read,
                0x80,
                2,
                2,
                RawDpcVisualTargetFormatV1::Rgba16,
                &target,
                &coverage,
                &post,
            )
            .unwrap();
            assert_ne!(changed, base);
        }
    }

    #[test]
    fn exact_readiness_refuses_nonexact_pristine_missing_vi_and_bad_coverage() {
        let layout = PhysicalMemoryLayout::try_new(0x200).unwrap();
        let exact_plan = fixture_plan(layout);
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 16).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(7),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: layout.range(0x40, 0x44).unwrap(),
                },
            )
            .unwrap()],
        )
        .unwrap();
        let pristine_plan =
            fn64_render_ir::DeferredGuestReadPlan::try_from_journal(layout, &journal).unwrap();
        let capture = capture(layout, 0xe6_000000);
        let digest = ContentDigest::hash(b"same", &[b"bytes"]);
        let exact_read = [RawDpcVisualGuestReadV1::new(exact_plan.reads()[0], digest)];
        let pristine_read = [RawDpcVisualGuestReadV1::new(
            pristine_plan.reads()[0],
            digest,
        )];
        let target = [0; 8];
        let coverage = [1, 2, 3, 4];
        let post = vec![0; layout.bytes() as usize];
        let mut input = RawDpcVisualCheckpointInputV1 {
            task_batch_identity: [1; 32],
            member_ordinal: 0,
            capture_source: RawDpcVisualCaptureSource::DiagnosticReconstruction,
            capture: &capture,
            guest_read_plan: &exact_plan,
            guest_reads: &exact_read,
            vi_registers: Some(vi()),
            target_address: 0x80,
            target_width: 2,
            target_height: 2,
            target_format: RawDpcVisualTargetFormatV1::Rgba16,
            target_device_bytes: &target,
            coverage: &coverage,
            post_copyback_rdram: &post,
        };
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::NonExactCaptureSource)
        );

        input.capture_source = RawDpcVisualCaptureSource::ExactLiveTransaction;
        input.guest_read_plan = &pristine_plan;
        input.guest_reads = &pristine_read;
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::PristineSnapshotReads)
        );

        input.guest_read_plan = &exact_plan;
        input.guest_reads = &exact_read;
        input.vi_registers = None;
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::MissingCompleteViState)
        );

        input.vi_registers = Some(vi());
        input.coverage = &[1, 2, 3];
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::CoverageByteCount {
                expected: 4,
                actual: 3
            })
        );
        input.coverage = &[1, 2, 9, 4];
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::InvalidCoverageCode { index: 2, code: 9 })
        );
        input.coverage = &[1, 0, 0, 4];
        assert_eq!(
            raw_dpc_visual_checkpoint_v1(input),
            Err(RawDpcVisualCheckpointRefusal::CoverageUnavailable { unknown_cells: 2 })
        );
    }
}
