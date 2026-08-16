//! Narrow adapters between fn64's existing raw-DPC capture and render IR.
//!
//! This module translates one already validated, owned capture. It neither
//! decides when a DPC transaction commits nor applies bytes to guest memory;
//! those remain ABI/runtime-owner responsibilities.

use fn64_render_ir::{
    AccessMode, CompletedWrite, ContentDigest, DecodedTicket, DmemRange, DramCommandChunk,
    DramCommandStream, EffectIdentity, FullSyncBoundary, GpuCompleteTicket, GuestCommittedTicket,
    QueueIdentity, RawCommandStream, ResourceAccess, ResourceJournal, ResourceRegion,
    SubmissionIdentity, TemporalBoundary, ValidationError, WorkloadAdmission, WorkloadPacket,
    WorkloadRecord, XbusCommandChunk, XbusCommandStream,
};

use crate::{OwnedRawDpcSubmission, RawDpcSource};

/// Convert one exact owned raw-DPC capture into the move-only IR decode state.
///
/// Capture validation has already proved the source range and payload length.
/// Packet construction adds the installed-memory proof, exact command decode,
/// temporal observation, resource journal, and content identity. The result is
/// ephemeral input: callers may derive a content-silent [`WorkloadRecord`] for
/// replay, but durable semantic publication uses
/// [`CommittedSemanticWorkloadRecord`] and architectural observations remain
/// forbidden until the surrounding DPC transaction commits.
pub fn decode_raw_dpc_capture(
    memory_layout: fn64_render_ir::PhysicalMemoryLayout,
    transaction_sequence: u64,
    capture: OwnedRawDpcSubmission,
    cmd_end: TemporalBoundary,
    full_sync_boundaries: Vec<FullSyncBoundary>,
    journal: ResourceJournal,
) -> Result<DecodedTicket, ValidationError> {
    let start = capture.start();
    let end = capture.end();
    let stream = match capture.source() {
        RawDpcSource::Rdram => RawCommandStream::Dram(DramCommandStream::try_new(vec![
            DramCommandChunk::try_new(
                memory_layout.range(start, end)?,
                capture.command_words(),
                cmd_end,
                full_sync_boundaries,
            )?,
        ])?),
        RawDpcSource::XbusDmem => RawCommandStream::Xbus(XbusCommandStream::try_new(vec![
            XbusCommandChunk::try_new(
                DmemRange::try_new(start, end)?,
                capture
                    .xbus_payload()
                    .expect("validated XBUS capture owns its sole byte image")
                    .to_vec(),
                cmd_end,
                full_sync_boundaries,
            )?,
        ])?),
    };
    Ok(DecodedTicket::new(WorkloadPacket::try_new(
        memory_layout,
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        },
        vec![stream],
        journal,
    )?))
}

/// Shared content identity for bytes produced by a renderer and rechecked by
/// the guest-memory owner before copyback.
pub fn ir_effect_content_digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::hash(b"fn64.render.ir-effect-bytes.v1\0", &[bytes])
}

/// Exact ABI-owned live-memory transaction preimage bound to one submitted workload.
///
/// This value is an identity, not guest-memory authority. The ABI owner keeps
/// the exclusive live allocation borrow and compares this complete binding
/// again before it issues a guest receipt or copies a byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrGuestMemoryPreimage {
    queue: QueueIdentity,
    transaction_ordinal: u64,
    submission: SubmissionIdentity,
    submission_ordinal: u64,
    byte_len: u32,
    content: ContentDigest,
}

impl IrGuestMemoryPreimage {
    pub fn try_capture(
        queue: QueueIdentity,
        transaction_ordinal: u64,
        submission: SubmissionIdentity,
        submission_ordinal: u64,
        bytes: &[u8],
    ) -> Result<Self, ValidationError> {
        let byte_len =
            u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
                field: "IR guest-memory preimage byte length",
            })?;
        Ok(Self {
            queue,
            transaction_ordinal,
            submission,
            submission_ordinal,
            byte_len,
            content: ContentDigest::hash(b"fn64.render.ir-guest-preimage.v1\0", &[bytes]),
        })
    }

    pub const fn queue(self) -> QueueIdentity {
        self.queue
    }

    pub const fn transaction_ordinal(self) -> u64 {
        self.transaction_ordinal
    }

    pub const fn submission(self) -> SubmissionIdentity {
        self.submission
    }

    pub const fn submission_ordinal(self) -> u64 {
        self.submission_ordinal
    }

    pub const fn byte_len(self) -> u32 {
        self.byte_len
    }

    pub const fn content(self) -> ContentDigest {
        self.content
    }
}

/// Owned immutable guest snapshot supplied by the ABI live-memory owner.
///
/// Capturing this image does not release or replace the owner's exclusive
/// borrow of the matching live allocation.
#[derive(Debug)]
pub struct IrGuestMemorySnapshot {
    preimage: IrGuestMemoryPreimage,
    bytes: Box<[u8]>,
}

impl IrGuestMemorySnapshot {
    pub fn try_capture(
        queue: QueueIdentity,
        transaction_ordinal: u64,
        submission: SubmissionIdentity,
        submission_ordinal: u64,
        bytes: &[u8],
    ) -> Result<Self, ValidationError> {
        Ok(Self {
            preimage: IrGuestMemoryPreimage::try_capture(
                queue,
                transaction_ordinal,
                submission,
                submission_ordinal,
                bytes,
            )?,
            bytes: bytes.to_vec().into_boxed_slice(),
        })
    }

    pub const fn preimage(&self) -> IrGuestMemoryPreimage {
        self.preimage
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Durable semantic publication proven to occur after guest copyback.
///
/// [`WorkloadRecord`] remains content-silent replay data and can be created
/// from any packet. This wrapper is the publication type: its private fields
/// can be populated only from a [`GuestCommittedTicket`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedSemanticWorkloadRecord {
    replay: WorkloadRecord,
    queue: QueueIdentity,
    ordinal: u64,
    submission: SubmissionIdentity,
    backend_effects: EffectIdentity,
    guest_effects: EffectIdentity,
}

impl CommittedSemanticWorkloadRecord {
    pub fn from_committed(ticket: &GuestCommittedTicket) -> Self {
        Self {
            replay: WorkloadRecord::from_packet(ticket.packet()),
            queue: ticket.queue(),
            ordinal: ticket.ordinal(),
            submission: ticket.submission(),
            backend_effects: ticket.backend_effect_identity(),
            guest_effects: ticket.guest_effect_identity(),
        }
    }

    pub const fn replay_record(&self) -> &WorkloadRecord {
        &self.replay
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
}

/// Exact RDRAM bytes staged by a renderer after backend completion.
///
/// This is data, not commit authority. The guest-memory owner must recompute
/// [`Self::completed_write`] and match it to the backend receipt before using
/// [`Self::bytes`] for copyback.
#[derive(Debug, PartialEq, Eq)]
pub struct StagedIrRdramWrite {
    access: ResourceAccess,
    bytes: Box<[u8]>,
}

impl StagedIrRdramWrite {
    pub fn try_new(access: ResourceAccess, bytes: Vec<u8>) -> Result<Self, ValidationError> {
        if !matches!(access.region(), ResourceRegion::Rdram { .. }) {
            return Err(ValidationError::EffectAccessMismatch {
                field: "staged RDRAM write",
                index: 0,
            });
        }
        if !matches!(access.mode(), AccessMode::Write | AccessMode::ReadWrite) {
            return Err(ValidationError::EffectForReadOnlyAccess);
        }
        let actual = u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
            field: "staged RDRAM write byte length",
        })?;
        let expected = access.region().declared_bytes();
        if actual != expected {
            return Err(ValidationError::EffectByteCountMismatch { expected, actual });
        }
        Ok(Self {
            access,
            bytes: bytes.into_boxed_slice(),
        })
    }

    pub const fn access(&self) -> ResourceAccess {
        self.access
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn completed_write(&self) -> CompletedWrite {
        CompletedWrite::try_new(
            self.access,
            self.access.region().declared_bytes(),
            ir_effect_content_digest(&self.bytes),
        )
        .expect("staged RDRAM write construction proved its access and exact byte count")
    }
}

/// Receipt-validated backend effects plus exact guest bytes still awaiting
/// the separately owned guest-commit authority.
///
/// Completions are move-only, so one completion cannot be committed twice.
///
/// ```compile_fail
/// use fn64_render::IrRawDpcBackendCompletion;
/// # fn completion() -> IrRawDpcBackendCompletion { unimplemented!() }
/// # fn commit(_: IrRawDpcBackendCompletion) {}
/// let completion = completion();
/// commit(completion);
/// commit(completion);
/// ```
#[derive(Debug)]
pub struct IrRawDpcBackendCompletion {
    ticket: GpuCompleteTicket,
    guest_preimage: IrGuestMemoryPreimage,
    staged_guest_writes: Box<[StagedIrRdramWrite]>,
}

impl IrRawDpcBackendCompletion {
    pub fn try_new(
        ticket: GpuCompleteTicket,
        guest_preimage: IrGuestMemoryPreimage,
        staged_guest_writes: Vec<StagedIrRdramWrite>,
    ) -> Result<Self, ValidationError> {
        let expected = ticket
            .backend_writes()
            .iter()
            .copied()
            .filter(|effect| effect.access().region().is_guest_visible())
            .collect::<Vec<_>>();
        if expected.len() != staged_guest_writes.len() {
            return Err(ValidationError::EffectCountMismatch {
                field: "staged guest write",
                expected: expected.len(),
                actual: staged_guest_writes.len(),
            });
        }
        for (index, (expected, staged)) in expected.iter().zip(&staged_guest_writes).enumerate() {
            if *expected != staged.completed_write() {
                return Err(ValidationError::EffectAccessMismatch {
                    field: "staged guest write",
                    index,
                });
            }
        }
        Ok(Self {
            ticket,
            guest_preimage,
            staged_guest_writes: staged_guest_writes.into_boxed_slice(),
        })
    }

    pub const fn ticket(&self) -> &GpuCompleteTicket {
        &self.ticket
    }

    pub fn staged_guest_writes(&self) -> &[StagedIrRdramWrite] {
        &self.staged_guest_writes
    }

    pub const fn guest_preimage(&self) -> IrGuestMemoryPreimage {
        self.guest_preimage
    }

    pub fn into_parts(
        self,
    ) -> (
        GpuCompleteTicket,
        IrGuestMemoryPreimage,
        Box<[StagedIrRdramWrite]>,
    ) {
        (self.ticket, self.guest_preimage, self.staged_guest_writes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{
        AccessPurpose, DpInterruptState, OperationId, PhysicalMemoryLayout, RdramResource,
        ResourceJournalLimits,
    };

    #[test]
    fn validated_capture_becomes_one_move_only_decoded_packet() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let range = layout.range(0x100, 0x108).unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 8).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range,
                },
            )
            .unwrap()],
        )
        .unwrap();
        let decoded = decode_raw_dpc_capture(
            layout,
            7,
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
            TemporalBoundary::new(11, DpInterruptState::Clear),
            Vec::new(),
            journal,
        )
        .unwrap();

        assert_eq!(decoded.packet().streams().len(), 1);
        assert_eq!(decoded.packet().owned_command_bytes(), 8);
        assert_eq!(
            decoded.packet().admission(),
            WorkloadAdmission::RawDpc {
                transaction_sequence: 7
            }
        );
    }
}
