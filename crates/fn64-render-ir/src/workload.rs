use core::fmt;

use sha2::{Digest, Sha256};

use crate::{
    ContentDigest, DeferredGuestReadCapture, DeferredGuestReadPlan, GuestReadCommandMoment,
    OwnedGuestReadSet, PhysicalMemoryLayout, RawCommandStream, RawStreamKind, RdramResource,
    ResourceJournal, ResourceRegion, ValidationError, WorkloadIdentity,
};

pub const MAX_PACKET_STREAMS: usize = 256;
pub const MAX_PACKET_COMMAND_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PACKET_COMMAND_CHUNKS: usize = 4096;
pub const MAX_PACKET_TIMELINE_EVENTS: usize = 65_536;

/// Exact text/data pair and ordered generation admitted for one semantic task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MicrocodeAdmissionIdentity {
    generation: u64,
    text_sha256: ContentDigest,
    data_bytes: u32,
    data_sha256: ContentDigest,
}

impl MicrocodeAdmissionIdentity {
    pub const fn new(
        generation: u64,
        text_sha256: ContentDigest,
        data_bytes: u32,
        data_sha256: ContentDigest,
    ) -> Self {
        Self {
            generation,
            text_sha256,
            data_bytes,
            data_sha256,
        }
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub const fn text_sha256(self) -> ContentDigest {
        self.text_sha256
    }

    pub const fn data_bytes(self) -> u32 {
        self.data_bytes
    }

    pub const fn data_sha256(self) -> ContentDigest {
        self.data_sha256
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkloadAdmission {
    /// Raw DPC work captured at this monotonic fabric transaction sequence.
    RawDpc {
        transaction_sequence: u64,
    },
    GraphicsTask(MicrocodeAdmissionIdentity),
}

/// Capture-independent packet admission. Construction validates stream
/// bounds, temporal order, journal ownership, and the exact deferred read
/// plan before a memory owner is asked to copy any guest bytes.
pub struct WorkloadPacketPreflight {
    memory_layout: PhysicalMemoryLayout,
    admission: WorkloadAdmission,
    streams: Vec<RawCommandStream>,
    journal: ResourceJournal,
    guest_read_plan: DeferredGuestReadPlan,
    owned_bytes: usize,
    chunk_count: usize,
    timeline_event_count: usize,
}

impl WorkloadPacketPreflight {
    pub fn try_new(
        memory_layout: PhysicalMemoryLayout,
        admission: WorkloadAdmission,
        streams: Vec<RawCommandStream>,
        journal: ResourceJournal,
    ) -> Result<Self, ValidationError> {
        let guest_read_plan = DeferredGuestReadPlan::try_from_journal(memory_layout, &journal)?;
        Self::try_new_with_plan(memory_layout, admission, streams, journal, guest_read_plan)
    }

    /// Construct a raw-command packet whose every deferred RDRAM read is
    /// bound to the exact command-completion point supplied by a sealed
    /// semantic planner. The binding list must be the journal's complete,
    /// ordered `TmemLoadSource` projection; missing, extra, or substituted
    /// descriptors are rejected before a packet preflight exists.
    pub fn try_new_with_guest_read_command_moments(
        memory_layout: PhysicalMemoryLayout,
        admission: WorkloadAdmission,
        streams: Vec<RawCommandStream>,
        journal: ResourceJournal,
        moments: &[GuestReadCommandMoment],
    ) -> Result<Self, ValidationError> {
        let guest_read_plan = DeferredGuestReadPlan::try_from_journal_with_command_moments(
            memory_layout,
            &journal,
            moments,
        )?;
        Self::try_new_with_plan(memory_layout, admission, streams, journal, guest_read_plan)
    }

    fn try_new_with_plan(
        memory_layout: PhysicalMemoryLayout,
        admission: WorkloadAdmission,
        streams: Vec<RawCommandStream>,
        journal: ResourceJournal,
        guest_read_plan: DeferredGuestReadPlan,
    ) -> Result<Self, ValidationError> {
        if streams.is_empty() {
            return Err(ValidationError::EmptyWorkload);
        }
        if streams.len() > MAX_PACKET_STREAMS {
            return Err(ValidationError::TooManyPacketStreams {
                actual: streams.len(),
                maximum: MAX_PACKET_STREAMS,
            });
        }
        if !streams
            .iter()
            .all(|stream| stream.matches_memory_layout(memory_layout))
            || !journal.matches_memory_layout(memory_layout)
        {
            return Err(ValidationError::MemoryLayoutMismatch {
                expected: memory_layout.bytes(),
            });
        }
        if guest_read_plan.memory_layout() != memory_layout
            || guest_read_plan.journal_identity() != journal.identity()
        {
            return Err(ValidationError::GuestReadPlanMismatch);
        }

        let owned_bytes = bounded_sum(
            streams.iter().map(|stream| stream.byte_len() as usize),
            MAX_PACKET_COMMAND_BYTES,
            |actual, maximum| ValidationError::PacketCommandBytesExceeded { actual, maximum },
        )?;
        let chunk_count = bounded_sum(
            streams.iter().map(RawCommandStream::chunk_count),
            MAX_PACKET_COMMAND_CHUNKS,
            |actual, maximum| ValidationError::PacketCommandChunksExceeded { actual, maximum },
        )?;
        let timeline_event_count = bounded_sum(
            streams.iter().map(RawCommandStream::timeline_event_count),
            MAX_PACKET_TIMELINE_EVENTS,
            |actual, maximum| ValidationError::PacketTimelineEventsExceeded { actual, maximum },
        )?;

        validate_global_temporal_order(&streams)?;
        validate_one_to_one_command_reads(&streams, &journal)?;
        Ok(Self {
            memory_layout,
            admission,
            streams,
            journal,
            guest_read_plan,
            owned_bytes,
            chunk_count,
            timeline_event_count,
        })
    }

    pub const fn guest_read_plan(&self) -> &DeferredGuestReadPlan {
        &self.guest_read_plan
    }

    pub fn finalize(
        self,
        guest_read_capture: DeferredGuestReadCapture,
    ) -> Result<WorkloadPacket, ValidationError> {
        let guest_reads =
            OwnedGuestReadSet::try_finalize(self.guest_read_plan, guest_read_capture)?;
        let identity = identity(
            self.memory_layout,
            self.admission,
            &self.streams,
            &self.journal,
            &guest_reads,
        );
        Ok(WorkloadPacket {
            identity,
            memory_layout: self.memory_layout,
            admission: self.admission,
            streams: self.streams.into_boxed_slice(),
            journal: self.journal,
            guest_reads,
            owned_bytes: self.owned_bytes,
            chunk_count: self.chunk_count,
            timeline_event_count: self.timeline_event_count,
        })
    }
}

impl fmt::Debug for WorkloadPacketPreflight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkloadPacketPreflight")
            .field("memory_layout", &self.memory_layout)
            .field("admission", &self.admission)
            .field("stream_count", &self.streams.len())
            .field("journal", &self.journal)
            .field("guest_read_plan", &self.guest_read_plan)
            .field("owned_command_bytes", &self.owned_bytes)
            .field("command_chunk_count", &self.chunk_count)
            .field("timeline_event_count", &self.timeline_event_count)
            .finish()
    }
}

impl WorkloadAdmission {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::RawDpc { .. } => 1,
            Self::GraphicsTask(_) => 2,
        }
    }
}

/// Immutable, owned semantic work. Raw command payloads are retained here and
/// never borrow guest memory; debug output deliberately prints identities and
/// counts rather than content. Packets are move-only so a decoded owner cannot
/// publish indistinguishable clones.
///
/// ```compile_fail
/// # fn packet() -> fn64_render_ir::WorkloadPacket { unimplemented!() }
/// let packet = packet();
/// let duplicate = packet.clone();
/// # drop(duplicate);
/// ```
#[derive(PartialEq, Eq)]
pub struct WorkloadPacket {
    identity: WorkloadIdentity,
    memory_layout: PhysicalMemoryLayout,
    admission: WorkloadAdmission,
    streams: Box<[RawCommandStream]>,
    journal: ResourceJournal,
    guest_reads: OwnedGuestReadSet,
    owned_bytes: usize,
    chunk_count: usize,
    timeline_event_count: usize,
}

impl WorkloadPacket {
    pub fn try_new(
        memory_layout: PhysicalMemoryLayout,
        admission: WorkloadAdmission,
        streams: Vec<RawCommandStream>,
        journal: ResourceJournal,
    ) -> Result<Self, ValidationError> {
        WorkloadPacketPreflight::try_new(memory_layout, admission, streams, journal)?
            .finalize(DeferredGuestReadCapture::empty())
    }

    /// Finalize one packet after the guest-memory owner captured the exact
    /// renderer-selected deferred read plan. No retained packet exists on a
    /// plan, ordering, range, byte-count, or digest mismatch.
    pub fn try_new_with_guest_reads(
        memory_layout: PhysicalMemoryLayout,
        admission: WorkloadAdmission,
        streams: Vec<RawCommandStream>,
        journal: ResourceJournal,
        guest_read_plan: DeferredGuestReadPlan,
        guest_read_capture: DeferredGuestReadCapture,
    ) -> Result<Self, ValidationError> {
        WorkloadPacketPreflight::try_new_with_plan(
            memory_layout,
            admission,
            streams,
            journal,
            guest_read_plan,
        )?
        .finalize(guest_read_capture)
    }

    pub const fn identity(&self) -> WorkloadIdentity {
        self.identity
    }

    pub const fn memory_layout(&self) -> PhysicalMemoryLayout {
        self.memory_layout
    }

    pub const fn admission(&self) -> WorkloadAdmission {
        self.admission
    }

    pub fn streams(&self) -> &[RawCommandStream] {
        &self.streams
    }

    /// Complete packet-global CMD_END, FullSync, and interrupt-observation
    /// order. Construction proves that stream timelines do not overlap or
    /// reuse a capture sequence.
    pub fn timeline(&self) -> Vec<crate::RawTimelineEvent> {
        self.streams
            .iter()
            .flat_map(RawCommandStream::timeline)
            .collect()
    }

    pub const fn journal(&self) -> &ResourceJournal {
        &self.journal
    }

    pub const fn guest_reads(&self) -> &OwnedGuestReadSet {
        &self.guest_reads
    }

    pub const fn owned_guest_read_bytes(&self) -> usize {
        self.guest_reads.total_bytes()
    }

    pub const fn owned_command_bytes(&self) -> usize {
        self.owned_bytes
    }

    pub const fn command_chunk_count(&self) -> usize {
        self.chunk_count
    }

    pub const fn timeline_event_count(&self) -> usize {
        self.timeline_event_count
    }
}

impl fmt::Debug for WorkloadPacket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkloadPacket")
            .field("identity", &self.identity)
            .field("memory_layout", &self.memory_layout)
            .field("admission", &self.admission)
            .field("stream_count", &self.streams.len())
            .field(
                "stream_identities",
                &self
                    .streams
                    .iter()
                    .map(RawCommandStream::identity)
                    .collect::<Vec<_>>(),
            )
            .field("journal", &self.journal)
            .field("guest_reads", &self.guest_reads)
            .field("owned_command_bytes", &self.owned_bytes)
            .field("command_chunk_count", &self.chunk_count)
            .field("timeline_event_count", &self.timeline_event_count)
            .finish()
    }
}

fn identity(
    memory_layout: PhysicalMemoryLayout,
    admission: WorkloadAdmission,
    streams: &[RawCommandStream],
    journal: &ResourceJournal,
    guest_reads: &OwnedGuestReadSet,
) -> WorkloadIdentity {
    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.workload.v3\0");
    hash.update(memory_layout.bytes().to_be_bytes());
    hash.update([admission.tag()]);
    match admission {
        WorkloadAdmission::RawDpc {
            transaction_sequence,
        } => hash.update(transaction_sequence.to_be_bytes()),
        WorkloadAdmission::GraphicsTask(identity) => {
            hash.update(identity.generation.to_be_bytes());
            hash.update(identity.text_sha256.as_ref());
            hash.update(identity.data_bytes.to_be_bytes());
            hash.update(identity.data_sha256.as_ref());
        }
    }
    hash.update((streams.len() as u32).to_be_bytes());
    for stream in streams {
        hash.update([stream.kind().tag()]);
        hash.update(stream.identity().as_bytes());
        hash.update(stream.byte_len().to_be_bytes());
        hash.update((stream.full_sync_occurrences().len() as u32).to_be_bytes());
    }
    hash.update(journal.identity().as_bytes());
    hash.update(guest_reads.plan_identity().as_bytes());
    hash.update(guest_reads.identity().as_bytes());
    WorkloadIdentity::new(ContentDigest::from_bytes(hash.finalize().into()))
}

fn bounded_sum(
    values: impl Iterator<Item = usize>,
    maximum: usize,
    error: impl FnOnce(usize, usize) -> ValidationError + Copy,
) -> Result<usize, ValidationError> {
    let mut total = 0_usize;
    for value in values {
        total = total
            .checked_add(value)
            .ok_or_else(|| error(usize::MAX, maximum))?;
        if total > maximum {
            return Err(error(total, maximum));
        }
    }
    Ok(total)
}

fn validate_global_temporal_order(streams: &[RawCommandStream]) -> Result<(), ValidationError> {
    let mut prior = None;
    for stream in streams {
        let (first, last) = stream.temporal_sequence_bounds();
        if let Some(prior) = prior {
            if first <= prior {
                return Err(ValidationError::NonMonotonicPacketEventSequence {
                    prior,
                    next: first,
                });
            }
        }
        prior = Some(last);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommandReadKey {
    kind: RawStreamKind,
    start: u32,
    end: u32,
}

fn validate_one_to_one_command_reads(
    streams: &[RawCommandStream],
    journal: &ResourceJournal,
) -> Result<(), ValidationError> {
    let expected = streams
        .iter()
        .map(|stream| {
            let (start, end) = stream.source_bounds();
            CommandReadKey {
                kind: stream.kind(),
                start,
                end,
            }
        })
        .collect::<Vec<_>>();
    let actual = journal
        .command_reads()
        .map(|access| match access.region() {
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range,
            } => CommandReadKey {
                kind: RawStreamKind::Dram,
                start: range.start().get(),
                end: range.end(),
            },
            ResourceRegion::RspDmem(range) => CommandReadKey {
                kind: RawStreamKind::Xbus,
                start: range.start(),
                end: range.end(),
            },
            _ => unreachable!("ResourceAccess construction restricts CommandDecode resources"),
        })
        .collect::<Vec<_>>();

    let mut consumed = vec![false; actual.len()];
    for key in expected {
        let Some(index) = actual.iter().enumerate().find_map(|(index, candidate)| {
            (!consumed[index] && *candidate == key).then_some(index)
        }) else {
            return Err(ValidationError::MissingCommandReadDeclaration {
                source: key.kind,
                start: key.start,
                end: key.end,
            });
        };
        consumed[index] = true;
    }
    if let Some((index, key)) = actual
        .iter()
        .enumerate()
        .find(|(index, _)| !consumed[*index])
    {
        return Err(ValidationError::UnmatchedCommandReadDeclaration {
            access_index: index,
            source: key.kind,
            start: key.start,
            end: key.end,
        });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::{
        AccessMode, AccessPurpose, DpInterruptState, DramCommandChunk, DramCommandStream,
        FullSyncBoundary, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
        ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
        WorkloadAdmission, WorkloadPacket,
    };

    pub(crate) fn packet(opcode: u8, transaction_sequence: u64) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let range = layout.range(0x100, 0x108).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                range,
                vec![u32::from(opcode) << 24, 0],
                TemporalBoundary::new(1, DpInterruptState::Clear),
                ((opcode & 0x3f) == 0x29)
                    .then_some(FullSyncBoundary::new(
                        2,
                        3,
                        DpInterruptState::Clear,
                        DpInterruptState::Asserted,
                    ))
                    .into_iter()
                    .collect(),
            )
            .unwrap()])
            .unwrap(),
        );
        let access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range,
            },
        )
        .unwrap();
        let write = ResourceAccess::try_new(
            OperationId::new(1),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: layout.range(0x200, 0x208).unwrap(),
            },
        )
        .unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(4, 0x100).unwrap(),
            vec![access, write],
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessMode, AccessPurpose, DmemRange, DpInterruptState, DramCommandChunk,
        DramCommandStream, FullSyncBoundary, OperationId, RawCommandStream, ResourceAccess,
        ResourceJournalLimits, ResourceRegion, TemporalBoundary, XbusCommandChunk,
        XbusCommandStream,
    };

    #[test]
    fn packet_requires_exact_source_typed_command_read() {
        let payload = [0xe900_0000u32, 0]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let stream = RawCommandStream::Xbus(
            XbusCommandStream::try_new(vec![XbusCommandChunk::try_new(
                DmemRange::try_new(0, 8).unwrap(),
                payload,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                vec![crate::FullSyncBoundary::new(
                    2,
                    3,
                    DpInterruptState::Clear,
                    DpInterruptState::Asserted,
                )],
            )
            .unwrap()])
            .unwrap(),
        );
        let wrong = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::RspDmem(DmemRange::try_new(8, 16).unwrap()),
        )
        .unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(2, 0x100).unwrap(),
            vec![wrong],
        )
        .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(
                crate::PhysicalMemoryLayout::try_new(0x1000).unwrap(),
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 1
                },
                vec![stream],
                journal
            ),
            Err(ValidationError::MissingCommandReadDeclaration {
                source: crate::RawStreamKind::Xbus,
                ..
            })
        ));
    }

    #[test]
    fn packet_identity_binds_admission_stream_and_journal() {
        let a = tests_support::packet(0xe9, 1);
        let b = tests_support::packet(0xe9, 2);
        let c = tests_support::packet(0xe6, 1);
        assert_ne!(a.identity(), b.identity());
        assert_ne!(a.identity(), c.identity());
    }

    #[test]
    fn packet_rejects_layout_erasure_and_reused_command_declarations() {
        let packet = tests_support::packet(0xe9, 1);
        let streams = packet.streams().to_vec();
        let journal = packet.journal().clone();
        assert_eq!(
            WorkloadPacket::try_new(
                PhysicalMemoryLayout::try_new(0x2000).unwrap(),
                packet.admission(),
                streams.clone(),
                journal.clone(),
            )
            .unwrap_err(),
            ValidationError::MemoryLayoutMismatch { expected: 0x2000 }
        );

        let mut accesses = journal.accesses().to_vec();
        accesses.insert(1, accesses[0]);
        let duplicate =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x100).unwrap(), accesses)
                .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(
                packet.memory_layout(),
                packet.admission(),
                streams,
                duplicate,
            ),
            Err(ValidationError::UnmatchedCommandReadDeclaration { .. })
        ));
    }

    #[test]
    fn packet_rejects_cross_stream_sequence_reuse() {
        let packet = tests_support::packet(0xe9, 1);
        let streams = vec![packet.streams()[0].clone(), packet.streams()[0].clone()];
        let mut accesses = packet.journal().accesses().to_vec();
        accesses.insert(1, accesses[0]);
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x100).unwrap(), accesses)
                .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(packet.memory_layout(), packet.admission(), streams, journal,),
            Err(ValidationError::NonMonotonicPacketEventSequence { .. })
        ));
    }

    #[test]
    fn packet_enforces_aggregate_chunk_and_event_bounds() {
        let layout = PhysicalMemoryLayout::try_new(0x80_000).unwrap();
        let build_nop_stream = |base_sequence: u64| {
            let chunks = (0..(MAX_PACKET_COMMAND_CHUNKS / 2 + 1))
                .map(|index| {
                    let start = index as u32 * 8;
                    DramCommandChunk::try_new(
                        layout.range(start, start + 8).unwrap(),
                        vec![0, 0],
                        TemporalBoundary::new(
                            base_sequence + index as u64,
                            DpInterruptState::Clear,
                        ),
                        vec![],
                    )
                    .unwrap()
                })
                .collect();
            RawCommandStream::Dram(DramCommandStream::try_new(chunks).unwrap())
        };
        let streams = vec![build_nop_stream(1), build_nop_stream(10_000)];
        let command_range = layout
            .range(0, (MAX_PACKET_COMMAND_CHUNKS / 2 + 1) as u32 * 8)
            .unwrap();
        let command_access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: crate::RdramResource::RawCommands,
                range: command_range,
            },
        )
        .unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(2, command_range.len() * 2).unwrap(),
            vec![command_access, command_access],
        )
        .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 1,
                },
                streams,
                journal,
            ),
            Err(ValidationError::PacketCommandChunksExceeded { .. })
        ));

        let sync_count = MAX_PACKET_TIMELINE_EVENTS / 2 + 1;
        let byte_len = sync_count as u32 * 8;
        let boundaries = (0..sync_count)
            .map(|index| {
                FullSyncBoundary::new(
                    2 + index as u64 * 2,
                    3 + index as u64 * 2,
                    if index == 0 {
                        DpInterruptState::Clear
                    } else {
                        DpInterruptState::Asserted
                    },
                    DpInterruptState::Asserted,
                )
            })
            .collect();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                layout.range(0, byte_len).unwrap(),
                [0xe900_0000, 0].repeat(sync_count),
                TemporalBoundary::new(1, DpInterruptState::Clear),
                boundaries,
            )
            .unwrap()])
            .unwrap(),
        );
        let access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: crate::RdramResource::RawCommands,
                range: layout.range(0, byte_len).unwrap(),
            },
        )
        .unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, byte_len).unwrap(),
            vec![access],
        )
        .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 2,
                },
                vec![stream],
                journal,
            ),
            Err(ValidationError::PacketTimelineEventsExceeded { .. })
        ));
    }

    #[test]
    fn packet_enforces_aggregate_owned_byte_bound() {
        let layout = PhysicalMemoryLayout::try_new(crate::RDP_PHYSICAL_ADDRESS_BYTES).unwrap();
        let range = layout.range(0, crate::RDP_PHYSICAL_ADDRESS_BYTES).unwrap();
        let word_count = crate::RDP_PHYSICAL_ADDRESS_BYTES as usize / size_of::<u32>();
        let dram = |sequence| {
            RawCommandStream::Dram(
                DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                    range,
                    vec![0; word_count],
                    TemporalBoundary::new(sequence, DpInterruptState::Clear),
                    vec![],
                )
                .unwrap()])
                .unwrap(),
            )
        };
        let xbus_range = DmemRange::try_new(0, 8).unwrap();
        let xbus = RawCommandStream::Xbus(
            XbusCommandStream::try_new(vec![XbusCommandChunk::try_new(
                xbus_range,
                vec![0; 8],
                TemporalBoundary::new(3, DpInterruptState::Clear),
                vec![],
            )
            .unwrap()])
            .unwrap(),
        );
        let dram_access = ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: crate::RdramResource::RawCommands,
                range,
            },
        )
        .unwrap();
        let xbus_access = ResourceAccess::try_new(
            OperationId::new(1),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::RspDmem(xbus_range),
        )
        .unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(2, (MAX_PACKET_COMMAND_BYTES + 8) as u32).unwrap(),
            vec![dram_access, xbus_access],
        )
        .unwrap();
        assert!(matches!(
            WorkloadPacket::try_new(
                layout,
                WorkloadAdmission::RawDpc {
                    transaction_sequence: 3,
                },
                vec![dram(1), xbus],
                journal,
            ),
            Err(ValidationError::PacketCommandBytesExceeded { .. })
        ));
    }
}
