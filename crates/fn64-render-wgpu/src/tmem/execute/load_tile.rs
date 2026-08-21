//! Transaction-local LoadTile execution.
//!
//! Source addressing and 64-bit transfer geometry come from the public SGI
//! *Nintendo 64 RDP Command Summary*, Tables 3, 6, and 7, and Programming
//! Manual sections 13.8.3 and 13.9. The checked M4.2.0 transfer remains the
//! geometry authority: this module binds its global source-access indices to
//! the exact M4.0 packet-owned captures and maps logical bytes into M4.2a's
//! physical lanes. RT64 is not hardware authority for this executor.

use core::fmt;

use fn64_render_ir::{
    AccessMode, AccessPurpose, OwnedGuestReadSet, QueueIdentity, RdramResource, ResourceAccess,
    ResourceRegion, SubmittedTicket,
};

use crate::raw_dpc::BoundTmemTransfer;
use crate::state::PixelSize;

use super::super::{
    PhysicalTmemError, PhysicalTmemPacketTransaction, StagedTmemTransaction, TmemLoadEpoch,
    TmemLoadKind, TmemLoadSourceIdentity, TmemTransferLayout, TmemTransferPhysicalWord,
    TmemTransferWord,
};

/// One-use LoadTile operation prepared from one exact submitted packet's
/// checked transfer and packet-owned guest reads.
///
/// It contains no guest-memory borrow or raw RDRAM slice. Execution consumes
/// it, and an error drops the packet-local staged clone without changing the
/// durable physical TMEM state.
pub struct PreparedLoadTile {
    source: TmemLoadSourceIdentity,
    queue: QueueIdentity,
    submission_ordinal: u64,
    epoch: TmemLoadEpoch,
    words: Box<[PreparedLoadTileWord]>,
}

impl fmt::Debug for PreparedLoadTile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedLoadTile")
            .field("source", &self.source)
            .field("queue", &self.queue)
            .field("submission_ordinal", &self.submission_ordinal)
            .field("epoch", &self.epoch)
            .field("word_count", &self.words.len())
            .finish()
    }
}

struct PreparedLoadTileWord {
    transfer: TmemTransferWord,
    physical_lanes: [Option<u8>; 8],
}

/// Transaction-local result of one complete LoadTile.
///
/// The packet owner can chain another exact transfer or seal the packet. The
/// ordered physical fragments are descriptors only; backend effects remain
/// owned by M4.2a and are not reported or published here.
pub struct ExecutedLoadTile {
    packet: PhysicalTmemPacketTransaction,
    ordered_fragments: Box<[TmemTransferPhysicalWord]>,
}

impl fmt::Debug for ExecutedLoadTile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutedLoadTile")
            .field("binding", &self.packet.binding())
            .field("ordered_fragments", &self.ordered_fragments)
            .finish()
    }
}

impl ExecutedLoadTile {
    pub fn ordered_fragments(&self) -> &[TmemTransferPhysicalWord] {
        &self.ordered_fragments
    }

    pub fn into_packet(self) -> PhysicalTmemPacketTransaction {
        self.packet
    }
}

/// Validates and captures the exact logical bytes for one checked LoadTile.
///
/// `submitted.packet().guest_reads()` is used directly so an independently
/// supplied, equal-looking read set cannot be rebound to this submission.
pub fn prepare_load_tile(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
) -> Result<PreparedLoadTile, LoadTileExecutionError> {
    let load = transfer.load();
    let TmemLoadKind::Tile { .. } = load.kind() else {
        return Err(LoadTileExecutionError::WrongLoadKind);
    };
    let plan = load
        .transfer_plan()
        .map_err(|_| LoadTileExecutionError::WrongLoadKind)?;
    let source = plan.source().identity();
    validate_submission(submitted, source)?;
    validate_transfer_shape(transfer)?;
    let reads = ExactLoadTileGuestReads::bind(
        submitted.packet().guest_reads(),
        transfer.source_accesses(),
        plan.source().first_access_index(),
    )?;

    let mut words = Vec::with_capacity(transfer.words().len());
    for (ordinal, word) in transfer.words().iter().copied().enumerate() {
        validate_word(plan, ordinal, word)?;
        let bytes = reads.bytes_for_word(word)?;
        words.push(PreparedLoadTileWord {
            transfer: word,
            physical_lanes: map_physical_lanes(word, bytes),
        });
    }

    Ok(PreparedLoadTile {
        source,
        queue: submitted.queue(),
        submission_ordinal: submitted.ordinal(),
        epoch: load.epoch(),
        words: words.into_boxed_slice(),
    })
}

impl PreparedLoadTile {
    /// Consumes this exact operation and the active M4.2a load transaction.
    pub fn execute(
        self,
        staged: StagedTmemTransaction,
    ) -> Result<ExecutedLoadTile, LoadTileExecutionError> {
        self.execute_inner(staged, None)
    }

    fn execute_inner(
        self,
        mut staged: StagedTmemTransaction,
        #[cfg_attr(not(test), allow(unused_variables))] fault_after_word: Option<usize>,
    ) -> Result<ExecutedLoadTile, LoadTileExecutionError> {
        let binding = staged.binding();
        for (matches, field) in [
            (binding.source() == self.source, "source identity"),
            (binding.queue() == self.queue, "queue identity"),
            (
                binding.submission_ordinal() == self.submission_ordinal,
                "submission ordinal",
            ),
            (staged.epoch() == self.epoch, "load epoch"),
        ] {
            if !matches {
                return Err(LoadTileExecutionError::StagedBindingMismatch { field });
            }
        }
        if staged.expected_words().len() != self.words.len()
            || staged
                .expected_words()
                .iter()
                .copied()
                .zip(self.words.iter().map(|word| word.transfer))
                .any(|(expected, prepared)| expected != prepared)
        {
            return Err(LoadTileExecutionError::StagedBindingMismatch {
                field: "ordered transfer words",
            });
        }

        let mut fragments = Vec::with_capacity(self.words.len());
        #[cfg(test)]
        let mut completed_words = 0;
        for word in self.words.into_vec() {
            fragments.push(word.transfer.physical());
            let payload = staged
                .physical_word_payload(word.transfer, word.physical_lanes)
                .map_err(LoadTileExecutionError::Physical)?;
            staged
                .stage_word(payload)
                .map_err(LoadTileExecutionError::Physical)?;
            #[cfg(test)]
            {
                completed_words += 1;
                if fault_after_word == Some(completed_words) {
                    return Err(LoadTileExecutionError::InjectedTestFault { completed_words });
                }
            }
        }
        let packet = staged
            .finish_load()
            .map_err(LoadTileExecutionError::Physical)?;
        Ok(ExecutedLoadTile {
            packet,
            ordered_fragments: fragments.into_boxed_slice(),
        })
    }
}

struct ExactLoadTileGuestReads<'a> {
    reads: &'a OwnedGuestReadSet,
    source_accesses: &'a [ResourceAccess],
    first_access_index: u32,
}

impl<'a> ExactLoadTileGuestReads<'a> {
    fn bind(
        reads: &'a OwnedGuestReadSet,
        source_accesses: &'a [ResourceAccess],
        first_access_index: u32,
    ) -> Result<Self, LoadTileExecutionError> {
        if source_accesses.is_empty() {
            return Err(LoadTileExecutionError::EmptySourcePlan);
        }

        for (ordinal, access) in source_accesses.iter().copied().enumerate() {
            let ordinal =
                u32::try_from(ordinal).map_err(|_| LoadTileExecutionError::SourceIndexOverflow)?;
            let access_index = first_access_index
                .checked_add(ordinal)
                .ok_or(LoadTileExecutionError::SourceIndexOverflow)?;
            let captured = reads
                .reads()
                .iter()
                .find(|captured| captured.read().access_index() == access_index)
                .ok_or(LoadTileExecutionError::MissingGuestRead { access_index })?;
            validate_capture(access_index, access, captured.read())?;
        }
        Ok(Self {
            reads,
            source_accesses,
            first_access_index,
        })
    }

    fn bytes_for_word(&self, word: TmemTransferWord) -> Result<&'a [u8], LoadTileExecutionError> {
        let relative = word
            .source_access_index()
            .checked_sub(self.first_access_index)
            .ok_or(LoadTileExecutionError::WordSourceMismatch { word: word.index() })?;
        let access = self
            .source_accesses
            .get(relative as usize)
            .copied()
            .ok_or(LoadTileExecutionError::WordSourceMismatch { word: word.index() })?;
        let captured = self
            .reads
            .reads()
            .iter()
            .find(|captured| captured.read().access_index() == word.source_access_index())
            .ok_or(LoadTileExecutionError::MissingGuestRead {
                access_index: word.source_access_index(),
            })?;
        validate_capture(word.source_access_index(), access, captured.read())?;

        let defined = word.defined_source_byte_mask().count_ones() as usize;
        let start = word.source_access_byte_offset() as usize;
        let end = start
            .checked_add(defined)
            .ok_or(LoadTileExecutionError::SourceIndexOverflow)?;
        captured
            .bytes()
            .get(start..end)
            .ok_or(LoadTileExecutionError::SourceWordOutOfBounds { word: word.index() })
    }
}

fn validate_capture(
    access_index: u32,
    access: ResourceAccess,
    read: fn64_render_ir::DeferredGuestRead,
) -> Result<(), LoadTileExecutionError> {
    let ResourceRegion::Rdram { resource, range } = access.region() else {
        return Err(LoadTileExecutionError::GuestReadDescriptorMismatch { access_index });
    };
    if access.mode() != AccessMode::Read
        || access.purpose() != AccessPurpose::TmemLoadSource
        || resource != RdramResource::Buffer
        || read.access_index() != access_index
        || read.operation() != access.operation()
        || read.resource() != resource
        || read.range() != range
    {
        return Err(LoadTileExecutionError::GuestReadDescriptorMismatch { access_index });
    }
    Ok(())
}

fn validate_submission(
    submitted: &SubmittedTicket,
    source: TmemLoadSourceIdentity,
) -> Result<(), LoadTileExecutionError> {
    let packet = submitted.packet();
    for (matches, field) in [
        (packet.identity() == source.workload(), "workload identity"),
        (
            packet.journal().identity() == source.journal(),
            "journal identity",
        ),
        (
            submitted.identity() == source.submission(),
            "submission identity",
        ),
        (
            packet.memory_layout() == source.memory_layout(),
            "memory layout",
        ),
    ] {
        if !matches {
            return Err(LoadTileExecutionError::SubmissionMismatch { field });
        }
    }
    Ok(())
}

fn validate_transfer_shape(transfer: &BoundTmemTransfer<'_>) -> Result<(), LoadTileExecutionError> {
    let plan = transfer
        .load()
        .transfer_plan()
        .map_err(|_| LoadTileExecutionError::WrongLoadKind)?;
    let expected_layout = match plan.source_image().size() {
        PixelSize::Bits4 => return Err(LoadTileExecutionError::DirectFourBit),
        PixelSize::Bits32 => TmemTransferLayout::SplitBanks64,
        PixelSize::Bits8 | PixelSize::Bits16 => TmemTransferLayout::Linear64,
    };
    if plan.layout() != expected_layout {
        return Err(LoadTileExecutionError::TransferMismatch {
            field: "source-size layout",
        });
    }
    if plan.transfer_words() as usize != transfer.words().len() {
        return Err(LoadTileExecutionError::TransferMismatch {
            field: "word count",
        });
    }
    if plan.source().access_count() as usize != transfer.source_accesses().len() {
        return Err(LoadTileExecutionError::TransferMismatch {
            field: "source access count",
        });
    }
    Ok(())
}

fn validate_word(
    plan: super::super::TmemTransferPlan,
    ordinal: usize,
    word: TmemTransferWord,
) -> Result<(), LoadTileExecutionError> {
    let ordinal =
        u16::try_from(ordinal).map_err(|_| LoadTileExecutionError::SourceIndexOverflow)?;
    let exact = word.index() == ordinal
        && word.logical_source_offset()
            == plan.logical_source_offset(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "logical source offset",
                }
            })?
        && word.defined_source_byte_mask()
            == plan.defined_source_byte_mask(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "defined source mask",
                }
            })?
        && word.destination_word()
            == plan.destination_word(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "destination word",
                }
            })?
        && word.row_advance()
            == plan.row_advance_for_word(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "row advance",
                }
            })?
        && word.odd_row_exchange()
            == plan.word_uses_odd_row_exchange(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "row exchange",
                }
            })?
        && word.physical()
            == plan.physical_word(ordinal).map_err(|_| {
                LoadTileExecutionError::TransferMismatch {
                    field: "physical fragments",
                }
            })?;
    let mask = word.defined_source_byte_mask();
    if !exact || mask == 0 || mask & mask.wrapping_add(1) != 0 {
        return Err(LoadTileExecutionError::TransferMismatch {
            field: "ordered transfer word",
        });
    }
    Ok(())
}

fn map_physical_lanes(word: TmemTransferWord, source: &[u8]) -> [Option<u8>; 8] {
    let mut physical = [None; 8];
    match word.physical() {
        TmemTransferPhysicalWord::Linear(_) => {
            let exchange = usize::from(word.odd_row_exchange()) * 4;
            for (source_lane, byte) in source.iter().copied().enumerate() {
                physical[source_lane ^ exchange] = Some(byte);
            }
        }
        TmemTransferPhysicalWord::SplitBanks { .. } => {
            const SOURCE_TO_PHYSICAL_LANE: [usize; 8] = [0, 1, 4, 5, 2, 3, 6, 7];
            for (source_lane, byte) in source.iter().copied().enumerate() {
                physical[SOURCE_TO_PHYSICAL_LANE[source_lane]] = Some(byte);
            }
        }
    }
    physical
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoadTileExecutionError {
    WrongLoadKind,
    DirectFourBit,
    EmptySourcePlan,
    SourceIndexOverflow,
    SubmissionMismatch {
        field: &'static str,
    },
    StagedBindingMismatch {
        field: &'static str,
    },
    TransferMismatch {
        field: &'static str,
    },
    MissingGuestRead {
        access_index: u32,
    },
    GuestReadDescriptorMismatch {
        access_index: u32,
    },
    WordSourceMismatch {
        word: u16,
    },
    SourceWordOutOfBounds {
        word: u16,
    },
    Physical(PhysicalTmemError),
    #[cfg(test)]
    InjectedTestFault {
        completed_words: usize,
    },
}

impl fmt::Display for LoadTileExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLoadKind => formatter.write_str("LoadTile executor requires a checked LoadTile transfer"),
            Self::DirectFourBit => formatter.write_str("direct four-bit LoadTile execution is not admitted"),
            Self::EmptySourcePlan => formatter.write_str("LoadTile source plan is empty"),
            Self::SourceIndexOverflow => formatter.write_str("LoadTile source index overflows"),
            Self::SubmissionMismatch { field } => write!(formatter, "LoadTile transfer belongs to another submission at {field}"),
            Self::StagedBindingMismatch { field } => write!(formatter, "prepared LoadTile belongs to another staged transaction at {field}"),
            Self::TransferMismatch { field } => write!(formatter, "checked LoadTile transfer differs at {field}"),
            Self::MissingGuestRead { access_index } => write!(formatter, "LoadTile source access {access_index} has no exact owned guest read"),
            Self::GuestReadDescriptorMismatch { access_index } => write!(formatter, "LoadTile source access {access_index} differs from its owned guest-read descriptor"),
            Self::WordSourceMismatch { word } => write!(formatter, "LoadTile word {word} refers outside its exact source-access slice"),
            Self::SourceWordOutOfBounds { word } => write!(formatter, "LoadTile word {word} spills beyond its row-local owned source read"),
            Self::Physical(error) => error.fmt(formatter),
            #[cfg(test)]
            Self::InjectedTestFault { completed_words } => write!(formatter, "injected LoadTile fault after {completed_words} words"),
        }
    }
}

impl std::error::Error for LoadTileExecutionError {}

impl From<PhysicalTmemError> for LoadTileExecutionError {
    fn from(error: PhysicalTmemError) -> Self {
        Self::Physical(error)
    }
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        AccessMode, AccessPurpose, BackendCompletionAuthority, BackendEffectReport,
        CapturedGuestRead, DecodedTicket, DeferredGuestReadCapture, DpInterruptState,
        DramCommandChunk, DramCommandStream, GuestCommitAuthority, GuestCommitEffectReport,
        OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource, ResourceAccess,
        ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
        TicketAuthoritySet, WorkloadAdmission, WorkloadPacket, WorkloadPacketPreflight,
        MAX_RESOURCE_ACCESSES,
    };

    use super::*;
    use crate::raw_dpc::{decode_raw_dpc, RawDpcCommandKind, RawDpcDecodeError};
    use crate::tmem::{LOAD_BLOCK, LOAD_SYNC, LOAD_TILE, SET_TEXTURE_IMAGE, SET_TILE};
    use crate::{PhysicalTmemState, RdpState};

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    #[derive(Clone, Copy)]
    struct TileFixtureSpec {
        format: u8,
        size: u8,
        image_width: u16,
        image_address: u32,
        tile_line_words: u16,
        tile_tmem: u16,
        low_s: u16,
        low_t: u16,
        high_s: u16,
        high_t: u16,
    }

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
    }

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    fn commands(spec: TileFixtureSpec) -> Vec<u32> {
        vec![
            word(
                SET_TEXTURE_IMAGE,
                u32::from(spec.format) << 21
                    | u32::from(spec.size) << 19
                    | u32::from(spec.image_width - 1),
            ),
            spec.image_address,
            word(
                SET_TILE,
                u32::from(spec.format) << 21
                    | u32::from(spec.size) << 19
                    | u32::from(spec.tile_line_words) << 9
                    | u32::from(spec.tile_tmem),
            ),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(
                LOAD_TILE,
                u32::from(spec.low_s) << 12 | u32::from(spec.low_t),
            ),
            7 << 24 | u32::from(spec.high_s) << 12 | u32::from(spec.high_t),
        ]
    }

    fn source_ranges(spec: TileFixtureSpec) -> Vec<(u32, u32)> {
        let bytes_per_pixel = match spec.size {
            1 => 1,
            2 => 2,
            3 => 4,
            _ => panic!("fixture needs a directly loadable non-four-bit size"),
        };
        let low_s = u32::from(spec.low_s >> 2);
        let low_t = u32::from(spec.low_t >> 2);
        let high_s = u32::from(spec.high_s >> 2);
        let high_t = u32::from(spec.high_t >> 2);
        let width = high_s - low_s + 1;
        let row_bytes = width * bytes_per_pixel;
        let padded_row_bytes = row_bytes.div_ceil(8) * 8;
        (low_t..=high_t)
            .map(|row| {
                let start = spec.image_address
                    + (row * u32::from(spec.image_width) + low_s) * bytes_per_pixel;
                (start, start + padded_row_bytes)
            })
            .collect()
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

    fn source_access(
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
                    let bytes = (read.range().start().get()..read.range().end())
                        .map(|address| address as u8)
                        .collect();
                    CapturedGuestRead::try_new(*read, bytes).unwrap()
                })
                .collect(),
        );
        preflight.finalize(capture).unwrap()
    }

    fn packet_from_words(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        let mut probe_accesses = vec![command_access(layout, byte_count, 0)];
        probe_accesses.extend(
            source_ranges
                .iter()
                .copied()
                .enumerate()
                .map(|(ordinal, (start, end))| {
                    source_access(layout, ordinal as u32 + 1, start, end)
                }),
        );
        let probe = finalize_packet(&words, probe_accesses);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(probe)).unwrap();
        let expected = match decode_raw_dpc(submitted, &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            other => panic!("planning probe did not request exact effects: {other:?}"),
        };
        finalize_packet(&words, expected)
    }

    fn packet(spec: TileFixtureSpec) -> WorkloadPacket {
        packet_from_words(commands(spec), &source_ranges(spec))
    }

    fn fixture(spec: TileFixtureSpec) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet(spec))).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    fn load(decoded: &crate::DecodedRawDpc) -> crate::TmemLoad {
        decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::LoadTile(load) => Some(load),
                _ => None,
            })
            .expect("fixture must contain LoadTile")
    }

    fn prepared_lanes(spec: TileFixtureSpec) -> Vec<(TmemTransferWord, [Option<u8>; 8])> {
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        prepare_load_tile(fixture.decoded.submitted(), &transfer)
            .unwrap()
            .words
            .into_vec()
            .into_iter()
            .map(|word| (word.transfer, word.physical_lanes))
            .collect()
    }

    fn snapshot(state: &PhysicalTmemState) -> String {
        format!("{state:?}")
    }

    fn publish(state: &mut PhysicalTmemState, fixture: Fixture, executed: ExecutedLoadTile) {
        let Fixture {
            decoded,
            mut backend,
            mut guest,
        } = fixture;
        let pending = executed.into_packet().into_pending().unwrap();
        let report = BackendEffectReport::try_new(
            decoded.submitted().packet(),
            pending.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt = backend.issue(decoded.submitted(), report).unwrap();
        let submitted = decoded.into_contract_parts().submitted;
        let complete = submitted.gpu_complete(receipt).unwrap();
        let gpu_bound = pending.bind_gpu(&complete).unwrap();
        let guest_report = GuestCommitEffectReport::try_new(&complete, Vec::new()).unwrap();
        let guest_receipt = guest.issue(&complete, guest_report).unwrap();
        let guest = complete.commit_guest(guest_receipt).unwrap();
        state
            .publication_authority()
            .publish(gpu_bound, guest)
            .unwrap();
    }

    #[test]
    fn linear_even_and_starting_odd_rows_map_complete_words() {
        let even = TileFixtureSpec {
            format: 4,
            size: 1,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            low_s: 0,
            low_t: 0,
            high_s: 28,
            high_t: 0,
        };
        let even_words = prepared_lanes(even);
        assert_eq!(source_ranges(even), vec![(0x200, 0x208)]);
        assert_eq!(even_words.len(), 1);
        assert_eq!(
            even_words[0].1,
            [
                Some(0x00),
                Some(0x01),
                Some(0x02),
                Some(0x03),
                Some(0x04),
                Some(0x05),
                Some(0x06),
                Some(0x07),
            ]
        );

        // **A load's FIRST row is tile-relative row 0 and never exchanges,
        // whatever the tile's T origin is.** This fixture used to set
        // `low_t: 4` (S10.2, so `.integer()` is 1) and assert that its single
        // word exchanged, which asserted the writer's removed
        // `low_t.integer()` term rather than anything hardware does --
        // angrylion takes `dswap = sst & 1` on a row made tile-relative by
        // `TRELATIVE`, so the first row's parity is 0 (`tex.c:583`,
        // `tcoord.c:998-999`).
        //
        // The origin is kept nonzero so the assertion FAILS if that term is
        // reintroduced, and a genuinely odd row is reached by spanning two
        // rows instead: `high_t` one row past `low_t` makes word 1
        // tile-relative row 1.
        let two_rows = TileFixtureSpec {
            low_t: 4,
            high_t: 8,
            ..even
        };
        let odd_words = prepared_lanes(two_rows);
        assert_eq!(
            source_ranges(two_rows),
            vec![(0x208, 0x210), (0x210, 0x218)]
        );
        assert_eq!(odd_words.len(), 2);
        assert!(
            !odd_words[0].0.odd_row_exchange(),
            "the load's first row is tile-relative row 0"
        );
        assert_eq!(
            odd_words[0].1,
            [
                Some(0x08),
                Some(0x09),
                Some(0x0a),
                Some(0x0b),
                Some(0x0c),
                Some(0x0d),
                Some(0x0e),
                Some(0x0f),
            ]
        );
        assert!(
            odd_words[1].0.odd_row_exchange(),
            "the load's second row is tile-relative row 1"
        );
        assert_eq!(
            odd_words[1].1,
            [
                Some(0x14),
                Some(0x15),
                Some(0x16),
                Some(0x17),
                Some(0x10),
                Some(0x11),
                Some(0x12),
                Some(0x13),
            ]
        );
    }

    /// Each row reads its own padded whole words, including exact adjacent
    /// RDRAM bytes, and retains tile-relative row parity. This follows RT64
    /// `rt64_rdp.cpp:369-397` (`Copy the entire word`).
    #[test]
    fn rgba16_padded_row_words_carry_adjacent_bytes_and_keep_row_parity() {
        let spec = TileFixtureSpec {
            format: 0,
            size: 2,
            image_width: 5,
            image_address: 0x200,
            tile_line_words: 3,
            tile_tmem: 0,
            low_s: 0,
            low_t: 4,
            high_s: 16,
            high_t: 8,
        };
        assert_eq!(source_ranges(spec), vec![(0x20a, 0x21a), (0x214, 0x224)]);
        let words = prepared_lanes(spec);
        assert_eq!(
            words
                .iter()
                .map(|(word, _)| (
                    word.logical_source_offset(),
                    word.defined_source_byte_mask(),
                    word.destination_word(),
                    word.odd_row_exchange(),
                ))
                .collect::<Vec<_>>(),
            // The exchange column is the TILE-RELATIVE row's parity and
            // nothing else: words 0-1 are row 0, words 2-3 are row 1. This
            // used to read `true, true, false, false` -- the exact inverse --
            // because the writer folded in `low_t.integer()`, which is 1 for
            // this fixture's `low_t = 4` (S10.2). That term is not on
            // hardware; see `tmem/read.rs::odd_row_exchange` for the
            // angrylion citation.
            vec![
                (0, 0xff, 0, false),
                (8, 0xff, 1, false),
                (10, 0xff, 3, true),
                (18, 0xff, 4, true),
            ]
        );
        // Word 1 is row 0: unexchanged, so its two defined bytes stay in the
        // logical-prefix lanes 0-1.
        assert_eq!(
            words[1].1,
            [
                Some(0x12), Some(0x13), Some(0x14), Some(0x15),
                Some(0x16), Some(0x17), Some(0x18), Some(0x19),
            ]
        );
        // Word 2 is row 1: a full word, so the exchange swaps its two 4-byte
        // halves.
        assert_eq!(
            words[2].1,
            [
                Some(0x18),
                Some(0x19),
                Some(0x1a),
                Some(0x1b),
                Some(0x14),
                Some(0x15),
                Some(0x16),
                Some(0x17),
            ]
        );
        // Word 3 is row 1 with a two-byte tail: the exchange moves it into
        // the high lanes 4-5.
        assert_eq!(
            words[3].1,
            [
                Some(0x20), Some(0x21), Some(0x22), Some(0x23),
                Some(0x1c), Some(0x1d), Some(0x1e), Some(0x1f),
            ]
        );
    }

    #[test]
    fn linear_wrap_and_rgba32_split_bank_mapping_are_exact() {
        let wrap = TileFixtureSpec {
            format: 0,
            size: 2,
            image_width: 5,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 511,
            low_s: 0,
            low_t: 0,
            high_s: 16,
            high_t: 0,
        };
        let wrapped = prepared_lanes(wrap);
        assert_eq!(
            wrapped
                .iter()
                .map(|(word, _)| word.destination_word())
                .collect::<Vec<_>>(),
            vec![511, 0]
        );

        let rgba32 = TileFixtureSpec {
            format: 0,
            size: 3,
            image_width: 2,
            image_address: 0x300,
            tile_line_words: 1,
            tile_tmem: 255,
            low_s: 0,
            low_t: 0,
            high_s: 4,
            high_t: 0,
        };
        let split = prepared_lanes(rgba32);
        assert_eq!(split.len(), 1);
        assert_eq!(
            split[0].1,
            [
                Some(0x00),
                Some(0x01),
                Some(0x04),
                Some(0x05),
                Some(0x02),
                Some(0x03),
                Some(0x06),
                Some(0x07),
            ]
        );
        assert!(matches!(
            split[0].0.physical(),
            TmemTransferPhysicalWord::SplitBanks { .. }
        ));

        let odd_wrap = TileFixtureSpec {
            low_t: 4,
            high_t: 8,
            ..rgba32
        };
        let split = prepared_lanes(odd_wrap);
        assert_eq!(
            split
                .iter()
                .map(|(word, _)| (
                    word.destination_word(),
                    word.odd_row_exchange(),
                    word.physical(),
                ))
                .collect::<Vec<_>>(),
            // Row 0 does not exchange and row 1 does -- the tile-relative
            // row's parity alone. This used to read `true` then `false`,
            // folding in the writer's removed `low_t.integer()` term (1 for
            // this fixture's S10.2 `low_t = 4`); the bank ranges move with the
            // exchange, so both columns invert together. See
            // `tmem/read.rs::odd_row_exchange`.
            vec![
                (
                    255,
                    false,
                    TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(2040, 2044).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(4088, 4092).unwrap(),
                    },
                ),
                (
                    0,
                    true,
                    TmemTransferPhysicalWord::SplitBanks {
                        low: fn64_render_ir::TmemRange::try_new(4, 8).unwrap(),
                        high: fn64_render_ir::TmemRange::try_new(2052, 2056).unwrap(),
                    },
                ),
            ]
        );
    }

    /// Overlapping destination rows publish the later row's complete padded
    /// word, following RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`).
    #[test]
    fn overlapping_rows_publish_the_last_rows_entire_padded_word() {
        let spec = TileFixtureSpec {
            format: 0,
            size: 2,
            image_width: 1,
            image_address: 0x200,
            tile_line_words: 0,
            tile_tmem: 0,
            low_s: 0,
            low_t: 0,
            high_s: 0,
            high_t: 4,
        };
        let mut state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        let prepared = prepare_load_tile(fixture.decoded.submitted(), &transfer).unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let executed = prepared.execute(staged).unwrap();
        assert_eq!(
            executed.ordered_fragments(),
            transfer
                .words()
                .iter()
                .map(|word| word.physical())
                .collect::<Vec<_>>()
        );
        publish(&mut state, fixture, executed);

        assert_eq!(state.generation(), 1);
        for lane in 0..8_u16 {
            assert!(state.byte_is_valid(lane));
            assert_eq!(state.last_touched_generation(lane), Some(1));
        }
        assert_eq!(
            (0_u16..8).map(|lane| state.valid_byte(lane).unwrap()).collect::<Vec<_>>(),
            vec![0x06, 0x07, 0x08, 0x09, 0x02, 0x03, 0x04, 0x05]
        );
    }

    #[test]
    fn exact_submission_and_staged_transaction_bindings_reject_rebinding() {
        let spec = TileFixtureSpec {
            format: 4,
            size: 1,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            low_s: 1,
            low_t: 1,
            high_s: 29,
            high_t: 1,
        };
        let first = fixture(spec);
        let second = fixture(TileFixtureSpec {
            image_address: 0x300,
            ..spec
        });
        let first_transfer = first
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&first.decoded))
            .unwrap();
        assert!(matches!(
            prepare_load_tile(second.decoded.submitted(), &first_transfer),
            Err(LoadTileExecutionError::SubmissionMismatch { .. })
        ));

        let prepared = prepare_load_tile(first.decoded.submitted(), &first_transfer).unwrap();
        let second_transfer = second
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&second.decoded))
            .unwrap();
        let state = PhysicalTmemState::try_new().unwrap();
        let staged = state
            .stage_transfer(second.decoded.submitted(), &second_transfer)
            .unwrap();
        assert!(matches!(
            prepared.execute(staged),
            Err(LoadTileExecutionError::StagedBindingMismatch { .. })
        ));

        let RawDpcCommandKind::LoadTile(load) = first.decoded.commands()[3].kind() else {
            panic!("expected LoadTile");
        };
        let TmemLoadKind::Tile { bounds } = load.kind() else {
            panic!("expected tile bounds");
        };
        assert_eq!(bounds.low_s().raw(), 1);
        assert_eq!(bounds.low_t().raw(), 1);
        assert_eq!(bounds.high_s().raw(), 29);
        assert_eq!(bounds.high_t().raw(), 1);
    }

    #[test]
    fn a_checked_load_block_cannot_enter_the_load_tile_executor() {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19 | 7),
            0x200,
            word(SET_TILE, 2 << 19 | 2 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ];
        let packet = packet_from_words(words, &[(0x214, 0x224)]);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        let block = decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::LoadBlock(load) => Some(load),
                _ => None,
            })
            .unwrap();
        let transfer = decoded.resource_plan().bind_tmem_transfer(block).unwrap();
        assert!(matches!(
            prepare_load_tile(decoded.submitted(), &transfer),
            Err(LoadTileExecutionError::WrongLoadKind)
        ));
    }

    #[test]
    fn a_fault_after_each_word_discards_the_packet_clone() {
        let spec = TileFixtureSpec {
            format: 0,
            size: 2,
            image_width: 5,
            image_address: 0x200,
            tile_line_words: 3,
            tile_tmem: 0,
            low_s: 0,
            low_t: 4,
            high_s: 16,
            high_t: 8,
        };
        let state = PhysicalTmemState::try_new().unwrap();
        let before = snapshot(&state);
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        for completed_words in 1..=transfer.words().len() {
            let prepared = prepare_load_tile(fixture.decoded.submitted(), &transfer).unwrap();
            let staged = state
                .stage_transfer(fixture.decoded.submitted(), &transfer)
                .unwrap();
            assert!(matches!(
                prepared.execute_inner(staged, Some(completed_words)),
                Err(LoadTileExecutionError::InjectedTestFault {
                    completed_words: actual,
                }) if actual == completed_words
            ));
            assert_eq!(snapshot(&state), before);
        }
    }
}
