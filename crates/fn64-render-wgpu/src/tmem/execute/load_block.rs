//! Transaction-local LoadBlock execution.
//!
//! Source addressing and 64-bit transfer geometry come from the public SGI
//! *Nintendo 64 RDP Command Summary*, Tables 1, 3, and 6-10, and Programming
//! Manual section 13.9. The checked M4.2.0 transfer remains the geometry
//! authority: this module binds its global source-access indices to the
//! exact M4.0 packet-owned captures and maps logical bytes into M4.2a's
//! physical lanes, mirroring M4.2b's LoadTile executor
//! ([`super::load_tile`]) rather than re-deriving its own source-binding
//! rules. RT64 is not hardware authority for this executor.

use core::fmt;

use fn64_render_ir::{
    AccessMode, AccessPurpose, OwnedGuestReadSet, QueueIdentity, RdramResource, ResourceAccess,
    ResourceRegion, SubmittedTicket,
};

use crate::raw_dpc::BoundTmemTransfer;

use super::super::{
    PhysicalTmemError, PhysicalTmemPacketTransaction, StagedTmemTransaction, TmemLoadEpoch,
    TmemLoadKind, TmemLoadSourceIdentity, TmemTransferPhysicalWord, TmemTransferWord,
};

/// One-use LoadBlock operation prepared from one exact submitted packet's
/// checked transfer and packet-owned guest reads.
///
/// It contains no guest-memory borrow or raw RDRAM slice. Execution consumes
/// it, and an error drops the packet-local staged clone without changing the
/// durable physical TMEM state.
pub struct PreparedLoadBlock {
    source: TmemLoadSourceIdentity,
    queue: QueueIdentity,
    submission_ordinal: u64,
    epoch: TmemLoadEpoch,
    words: Box<[PreparedLoadBlockWord]>,
}

impl fmt::Debug for PreparedLoadBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedLoadBlock")
            .field("source", &self.source)
            .field("queue", &self.queue)
            .field("submission_ordinal", &self.submission_ordinal)
            .field("epoch", &self.epoch)
            .field("word_count", &self.words.len())
            .finish()
    }
}

struct PreparedLoadBlockWord {
    transfer: TmemTransferWord,
    physical_lanes: [Option<u8>; 8],
}

/// Transaction-local result of one complete LoadBlock.
///
/// The packet owner can chain another exact transfer or seal the packet. The
/// ordered physical fragments are descriptors only; backend effects remain
/// owned by M4.2a and are not reported or published here.
pub struct ExecutedLoadBlock {
    packet: PhysicalTmemPacketTransaction,
    ordered_fragments: Box<[TmemTransferPhysicalWord]>,
}

impl fmt::Debug for ExecutedLoadBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutedLoadBlock")
            .field("binding", &self.packet.binding())
            .field("ordered_fragments", &self.ordered_fragments)
            .finish()
    }
}

impl ExecutedLoadBlock {
    pub fn ordered_fragments(&self) -> &[TmemTransferPhysicalWord] {
        &self.ordered_fragments
    }

    pub fn into_packet(self) -> PhysicalTmemPacketTransaction {
        self.packet
    }
}

/// Validates and captures the exact logical bytes for one checked LoadBlock.
///
/// `submitted.packet().guest_reads()` is used directly so an independently
/// supplied, equal-looking read set cannot be rebound to this submission.
pub fn prepare_load_block(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
) -> Result<PreparedLoadBlock, LoadBlockExecutionError> {
    let load = transfer.load();
    let TmemLoadKind::Block { .. } = load.kind() else {
        return Err(LoadBlockExecutionError::WrongLoadKind);
    };
    let plan = load
        .transfer_plan()
        .map_err(|_| LoadBlockExecutionError::WrongLoadKind)?;
    let source = plan.source().identity();
    validate_submission(submitted, source)?;
    validate_transfer_shape(transfer)?;
    let reads = ExactLoadBlockGuestReads::bind(
        submitted.packet().guest_reads(),
        transfer.source_accesses(),
        plan.source().first_access_index(),
    )?;

    let mut words = Vec::with_capacity(transfer.words().len());
    for (ordinal, word) in transfer.words().iter().copied().enumerate() {
        validate_word(plan, ordinal, word)?;
        let bytes = reads.bytes_for_word(word)?;
        words.push(PreparedLoadBlockWord {
            transfer: word,
            physical_lanes: map_physical_lanes(word, bytes),
        });
    }

    Ok(PreparedLoadBlock {
        source,
        queue: submitted.queue(),
        submission_ordinal: submitted.ordinal(),
        epoch: load.epoch(),
        words: words.into_boxed_slice(),
    })
}

impl PreparedLoadBlock {
    /// Consumes this exact operation and the active M4.2a load transaction.
    pub fn execute(
        self,
        staged: StagedTmemTransaction,
    ) -> Result<ExecutedLoadBlock, LoadBlockExecutionError> {
        self.execute_inner(staged, None)
    }

    fn execute_inner(
        self,
        mut staged: StagedTmemTransaction,
        #[cfg_attr(not(test), allow(unused_variables))] fault_after_word: Option<usize>,
    ) -> Result<ExecutedLoadBlock, LoadBlockExecutionError> {
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
                return Err(LoadBlockExecutionError::StagedBindingMismatch { field });
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
            return Err(LoadBlockExecutionError::StagedBindingMismatch {
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
                .map_err(LoadBlockExecutionError::Physical)?;
            staged
                .stage_word(payload)
                .map_err(LoadBlockExecutionError::Physical)?;
            #[cfg(test)]
            {
                completed_words += 1;
                if fault_after_word == Some(completed_words) {
                    return Err(LoadBlockExecutionError::InjectedTestFault { completed_words });
                }
            }
        }
        let packet = staged
            .finish_load()
            .map_err(LoadBlockExecutionError::Physical)?;
        Ok(ExecutedLoadBlock {
            packet,
            ordered_fragments: fragments.into_boxed_slice(),
        })
    }
}

struct ExactLoadBlockGuestReads<'a> {
    reads: &'a OwnedGuestReadSet,
    source_accesses: &'a [ResourceAccess],
    first_access_index: u32,
}

impl<'a> ExactLoadBlockGuestReads<'a> {
    fn bind(
        reads: &'a OwnedGuestReadSet,
        source_accesses: &'a [ResourceAccess],
        first_access_index: u32,
    ) -> Result<Self, LoadBlockExecutionError> {
        if source_accesses.is_empty() {
            return Err(LoadBlockExecutionError::EmptySourcePlan);
        }

        for (ordinal, access) in source_accesses.iter().copied().enumerate() {
            let ordinal =
                u32::try_from(ordinal).map_err(|_| LoadBlockExecutionError::SourceIndexOverflow)?;
            let access_index = first_access_index
                .checked_add(ordinal)
                .ok_or(LoadBlockExecutionError::SourceIndexOverflow)?;
            let captured = reads
                .reads()
                .iter()
                .find(|captured| captured.read().access_index() == access_index)
                .ok_or(LoadBlockExecutionError::MissingGuestRead { access_index })?;
            validate_capture(access_index, access, captured.read())?;
        }
        Ok(Self {
            reads,
            source_accesses,
            first_access_index,
        })
    }

    fn bytes_for_word(&self, word: TmemTransferWord) -> Result<&'a [u8], LoadBlockExecutionError> {
        let relative = word
            .source_access_index()
            .checked_sub(self.first_access_index)
            .ok_or(LoadBlockExecutionError::WordSourceMismatch { word: word.index() })?;
        let access = self
            .source_accesses
            .get(relative as usize)
            .copied()
            .ok_or(LoadBlockExecutionError::WordSourceMismatch { word: word.index() })?;
        let captured = self
            .reads
            .reads()
            .iter()
            .find(|captured| captured.read().access_index() == word.source_access_index())
            .ok_or(LoadBlockExecutionError::MissingGuestRead {
                access_index: word.source_access_index(),
            })?;
        validate_capture(word.source_access_index(), access, captured.read())?;

        let defined = word.defined_source_byte_mask().count_ones() as usize;
        let start = word.source_access_byte_offset() as usize;
        let end = start
            .checked_add(defined)
            .ok_or(LoadBlockExecutionError::SourceIndexOverflow)?;
        captured
            .bytes()
            .get(start..end)
            .ok_or(LoadBlockExecutionError::SourceWordOutOfBounds { word: word.index() })
    }
}

fn validate_capture(
    access_index: u32,
    access: ResourceAccess,
    read: fn64_render_ir::DeferredGuestRead,
) -> Result<(), LoadBlockExecutionError> {
    let ResourceRegion::Rdram { resource, range } = access.region() else {
        return Err(LoadBlockExecutionError::GuestReadDescriptorMismatch { access_index });
    };
    if access.mode() != AccessMode::Read
        || access.purpose() != AccessPurpose::TmemLoadSource
        || resource != RdramResource::Buffer
        || read.access_index() != access_index
        || read.operation() != access.operation()
        || read.resource() != resource
        || read.range() != range
    {
        return Err(LoadBlockExecutionError::GuestReadDescriptorMismatch { access_index });
    }
    Ok(())
}

fn validate_submission(
    submitted: &SubmittedTicket,
    source: TmemLoadSourceIdentity,
) -> Result<(), LoadBlockExecutionError> {
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
            return Err(LoadBlockExecutionError::SubmissionMismatch { field });
        }
    }
    Ok(())
}

fn validate_transfer_shape(
    transfer: &BoundTmemTransfer<'_>,
) -> Result<(), LoadBlockExecutionError> {
    let plan = transfer
        .load()
        .transfer_plan()
        .map_err(|_| LoadBlockExecutionError::WrongLoadKind)?;
    if plan.transfer_words() as usize != transfer.words().len() {
        return Err(LoadBlockExecutionError::TransferMismatch {
            field: "word count",
        });
    }
    if plan.source().access_count() as usize != transfer.source_accesses().len() {
        return Err(LoadBlockExecutionError::TransferMismatch {
            field: "source access count",
        });
    }
    Ok(())
}

fn validate_word(
    plan: super::super::TmemTransferPlan,
    ordinal: usize,
    word: TmemTransferWord,
) -> Result<(), LoadBlockExecutionError> {
    let ordinal =
        u16::try_from(ordinal).map_err(|_| LoadBlockExecutionError::SourceIndexOverflow)?;
    let exact = word.index() == ordinal
        && word.logical_source_offset()
            == plan.logical_source_offset(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "logical source offset",
                }
            })?
        && word.defined_source_byte_mask()
            == plan.defined_source_byte_mask(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "defined source mask",
                }
            })?
        && word.destination_word()
            == plan.destination_word(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "destination word",
                }
            })?
        && word.row_advance()
            == plan.row_advance_for_word(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "row advance",
                }
            })?
        && word.odd_row_exchange()
            == plan.word_uses_odd_row_exchange(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "row exchange",
                }
            })?
        && word.physical()
            == plan.physical_word(ordinal).map_err(|_| {
                LoadBlockExecutionError::TransferMismatch {
                    field: "physical fragments",
                }
            })?;
    let mask = word.defined_source_byte_mask();
    if !exact || mask == 0 || mask & mask.wrapping_add(1) != 0 {
        return Err(LoadBlockExecutionError::TransferMismatch {
            field: "ordered transfer word",
        });
    }
    Ok(())
}

/// Places this word's defined logical source bytes into their physical
/// fragment lanes.
///
/// For a linear word, odd-row LoadBlock transfers exchange the two 4-byte
/// halves of the 8-byte word (SGI Programming Manual section 13.9's TMEM row
/// interleave applies to every LoadBlock transfer, not only the RGBA32
/// split-bank case): source lane `n` lands in physical lane `n ^ 4` when
/// `word.odd_row_exchange()` is set, matching `physical.rs`'s own
/// `physical_defined_lane_mask` rotation for the same word. The XOR stays
/// inside the same 8-byte word `project_tmem_transfer_word` already unioned,
/// so the frozen M4.2.0 physical range is untouched. For a split-bank
/// (RGBA32) word, source lanes interleave into low/high four-byte banks:
/// lane order `[0,1,4,5,2,3,6,7]` places logical bytes 0-1 and 4-5 into the
/// low bank and 2-3 and 6-7 into the high bank -- the exact split M4.2a's
/// `physical_defined_lane_mask` expects and validates independently. This
/// function does not decide either mapping; it reproduces the one M4.2a
/// already froze, so a mismatch here surfaces as a loud
/// `PhysicalLaneMaskMismatch` rather than a silently wrong texture.
pub(crate) fn map_physical_lanes(word: TmemTransferWord, source: &[u8]) -> [Option<u8>; 8] {
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

/// Loud, source-identified LoadBlock execution failure. Every variant either
/// wraps the M4.2a physical-state authority's own rejection or names a
/// LoadBlock-specific precondition this executor itself enforces before
/// staging.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadBlockExecutionError {
    WrongLoadKind,
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

impl fmt::Display for LoadBlockExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLoadKind => {
                formatter.write_str("LoadBlock executor requires a checked LoadBlock transfer")
            }
            Self::EmptySourcePlan => formatter.write_str("LoadBlock source plan is empty"),
            Self::SourceIndexOverflow => formatter.write_str("LoadBlock source index overflows"),
            Self::SubmissionMismatch { field } => write!(
                formatter,
                "LoadBlock transfer belongs to another submission at {field}"
            ),
            Self::StagedBindingMismatch { field } => write!(
                formatter,
                "prepared LoadBlock belongs to another staged transaction at {field}"
            ),
            Self::TransferMismatch { field } => {
                write!(formatter, "checked LoadBlock transfer differs at {field}")
            }
            Self::MissingGuestRead { access_index } => write!(
                formatter,
                "LoadBlock source access {access_index} has no exact owned guest read"
            ),
            Self::GuestReadDescriptorMismatch { access_index } => write!(
                formatter,
                "LoadBlock source access {access_index} differs from its owned guest-read descriptor"
            ),
            Self::WordSourceMismatch { word } => write!(
                formatter,
                "LoadBlock word {word} refers outside its exact source-access slice"
            ),
            Self::SourceWordOutOfBounds { word } => write!(
                formatter,
                "LoadBlock word {word} spills beyond its owned source read"
            ),
            Self::Physical(error) => error.fmt(formatter),
            #[cfg(test)]
            Self::InjectedTestFault { completed_words } => {
                write!(formatter, "injected LoadBlock fault after {completed_words} words")
            }
        }
    }
}

impl std::error::Error for LoadBlockExecutionError {}

impl From<PhysicalTmemError> for LoadBlockExecutionError {
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
    struct BlockFixtureSpec {
        format: u8,
        size: u8,
        image_width: u16,
        image_address: u32,
        tile_line_words: u16,
        tile_tmem: u16,
        source_s: u16,
        source_t: u16,
        high_s: u16,
        dxt: u16,
        source_byte_len: u32,
    }

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
    }

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    fn commands(spec: BlockFixtureSpec) -> Vec<u32> {
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
                LOAD_BLOCK,
                u32::from(spec.source_s) << 12 | u32::from(spec.source_t),
            ),
            7 << 24 | u32::from(spec.high_s) << 12 | u32::from(spec.dxt),
        ]
    }

    fn source_ranges(spec: BlockFixtureSpec) -> Vec<(u32, u32)> {
        let start = spec.image_address;
        let padded_byte_len = spec.source_byte_len.div_ceil(8) * 8;
        vec![(start, start + padded_byte_len)]
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

    fn packet(spec: BlockFixtureSpec) -> WorkloadPacket {
        packet_from_words(commands(spec), &source_ranges(spec))
    }

    fn fixture(spec: BlockFixtureSpec) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet(spec))).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    fn load(decoded: &crate::DecodedRawDpc) -> super::super::super::TmemLoad {
        load_at(decoded, 0)
    }

    fn load_at(decoded: &crate::DecodedRawDpc, ordinal: usize) -> super::super::super::TmemLoad {
        let command_index = 3 + ordinal * 2;
        match decoded.commands()[command_index].kind() {
            RawDpcCommandKind::LoadBlock(load) => load,
            other => panic!("expected LoadBlock at command {command_index}, found {other:?}"),
        }
    }

    /// One packet carrying two chained LoadBlocks against the same tile,
    /// each reading a disjoint, non-overlapping texel span so both loads
    /// admit as separate resource accesses in one submission -- mirroring
    /// `tmem::physical`'s own `fixture(2)`/`load(decoded, ordinal)` chained-
    /// transaction test pattern.
    fn two_load_fixture(spec: BlockFixtureSpec) -> Fixture {
        let words = vec![
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
                LOAD_BLOCK,
                u32::from(spec.source_s) << 12 | u32::from(spec.source_t),
            ),
            7 << 24 | u32::from(spec.high_s) << 12 | u32::from(spec.dxt),
            word(LOAD_SYNC, 0),
            0,
            word(
                LOAD_BLOCK,
                u32::from(spec.high_s + 1) << 12 | u32::from(spec.source_t),
            ),
            7 << 24
                | u32::from(spec.high_s + 1 + (spec.high_s - spec.source_s)) << 12
                | u32::from(spec.dxt),
        ];
        let bytes_per_texel: u32 = match spec.size {
            1 => 1,
            2 => 2,
            3 => 4,
            _ => panic!("fixture needs a directly loadable non-four-bit size"),
        };
        let texels = u32::from(spec.high_s - spec.source_s) + 1;
        let span_bytes = texels * bytes_per_texel;
        let first_start = spec.image_address;
        let first_end = first_start + span_bytes;
        let second_start = first_end;
        let second_end = second_start + span_bytes;
        let packet = packet_from_words(
            words,
            &[(first_start, first_end), (second_start, second_end)],
        );
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    fn prepared_lanes(spec: BlockFixtureSpec) -> Vec<(TmemTransferWord, [Option<u8>; 8])> {
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        let prepared = prepare_load_block(fixture.decoded.submitted(), &transfer).unwrap();
        prepared
            .words
            .iter()
            .map(|word| (word.transfer, word.physical_lanes))
            .collect()
    }

    #[test]
    fn linear_even_row_places_source_bytes_in_logical_order() {
        // RGBA16 (Bits16), source_t even (0): no odd-row exchange.
        let spec = BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 0,
            high_s: 3,
            dxt: 0,
            source_byte_len: 8,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 1);
        assert!(!words[0].0.odd_row_exchange());
        assert_eq!(
            words[0].1,
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
    }

    /// A full 8-byte word landing on an ODD tile-relative row swaps its two
    /// 4-byte halves into physical lanes, even though the defined-byte mask
    /// stays `0xff` either way.
    ///
    /// **The odd row is reached with DXT, not with `source_t`.** The row a
    /// LoadBlock word lands on is `(word * dxt) >> 11`, so `dxt = 2048`
    /// advances exactly one row per word and word 1 is row 1. This fixture
    /// used to set `source_t: 1` and assert that word 0 -- row ZERO --
    /// exchanged, which asserted fn64's own removed `source_t` term rather
    /// than anything hardware does. The reference write parity is `dswap =
    /// sst & 1` on a row made tile-relative by `TRELATIVE` (`tex.c:583`,
    /// `tcoord.c:998-999`), and a load's first row is always zero; see
    /// `tmem/read.rs::odd_row_exchange`.
    ///
    /// `source_t` is kept at 1 deliberately, so that the assertion below
    /// FAILS if the removed term is ever reintroduced on either side: with
    /// it back, word 0 would exchange and word 1 would not -- the exact
    /// inverse of what is asserted here.
    #[test]
    fn linear_odd_row_full_word_exchanges_lane_halves() {
        let spec = BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 1,
            high_s: 7,
            dxt: 2048,
            source_byte_len: 16,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 2);
        // Row 0: no exchange. Row 1: exchange.
        assert!(
            !words[0].0.odd_row_exchange(),
            "word 0 is tile-relative row 0"
        );
        assert!(
            words[1].0.odd_row_exchange(),
            "word 1 is tile-relative row 1"
        );
        assert_eq!(words[1].0.defined_source_byte_mask(), 0xff);
        // `source_t = 1` offsets the logical source by one image row
        // (8 texels * 2 bytes = 16 bytes), so the captured bytes start at
        // 0x10; word 1 takes the following eight, 0x18..0x20 = 24..32, and
        // the odd-row exchange swaps their two 4-byte halves.
        assert_eq!(
            words[1].1,
            [
                Some(28),
                Some(29),
                Some(30),
                Some(31),
                Some(24),
                Some(25),
                Some(26),
                Some(27),
            ]
        );
    }

    /// A padded tail word landing on an odd tile-relative row moves the whole
    /// adjacent-RDRAM word through the four-byte lane exchange. This matches
    /// RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`), which copies all
    /// eight bytes even when the logical texels end partway through the word.
    ///
    /// The odd row is reached with `dxt = 2048` (one row per word), so word 1
    /// is tile-relative row 1. This fixture previously relied on `source_t: 1`
    /// making BOTH words exchange, which asserted fn64's own removed
    /// `source_t` term; see `linear_odd_row_full_word_exchanges_lane_halves`
    /// above for the citation. `source_t: 1` is retained so the row-0/row-1
    /// split below inverts if that term is ever reintroduced.
    #[test]
    fn linear_odd_row_padded_tail_exchanges_the_entire_word() {
        let spec = BlockFixtureSpec {
            format: 0,
            size: 1,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 1,
            high_s: 9,
            dxt: 2048,
            source_byte_len: 10,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 2);
        assert!(
            !words[0].0.odd_row_exchange(),
            "word 0 is tile-relative row 0"
        );
        assert!(
            words[1].0.odd_row_exchange(),
            "word 1 is tile-relative row 1"
        );
        // **All eight lanes, because the DMA copies whole 64-bit words.**
        // This asserted `0x03` and a half-`None` tail, which is the model the
        // padded-word fix corrected; the EXCHANGE claim it exists for is
        // unaffected and is asserted below.
        assert_eq!(words[1].0.defined_source_byte_mask(), 0xff);

        // `source_t = 1` offsets the logical source by one image row
        // (8 texels * 1 byte = 8 bytes) from `image_address`, so word 1 reads
        // absolute source bytes 16..24 -- its two logical bytes 16-17 plus the
        // six adjacent RDRAM bytes the padded word carries.
        //
        // **The exchange is what this test is for**: word 1 is tile-relative
        // row 1, so its bytes land in the HIGH half of the destination word
        // (`lane ^ 4`) rather than at the unexchanged prefix. With a full
        // word every lane is defined, and the exchange shows as the byte
        // ORDER: lanes 4..8 carry the word's first four source bytes.
        let lanes = words[1].1;
        assert!(
            lanes.iter().all(Option::is_some),
            "a padded word defines every lane: {lanes:?}"
        );
        assert_eq!(
            [lanes[4], lanes[5], lanes[6], lanes[7]],
            [Some(16), Some(17), Some(18), Some(19)],
            "odd-row exchange puts the word's leading source bytes in the high half"
        );
        assert_eq!(
            [lanes[0], lanes[1], lanes[2], lanes[3]],
            [Some(20), Some(21), Some(22), Some(23)],
            "and its trailing source bytes in the low half"
        );

        // The placement must also satisfy M4.2a's own independently-derived
        // physical lane mask (cross-check, not a second trust of the same
        // table): staging this payload must succeed rather than raise
        // `PhysicalLaneMaskMismatch`.
        let fixture_state = PhysicalTmemState::try_new().unwrap();
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        let prepared = prepare_load_block(fixture.decoded.submitted(), &transfer).unwrap();
        let staged = fixture_state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        assert!(prepared.execute(staged).is_ok());
    }

    #[test]
    fn split_bank_word_places_source_lanes_into_low_and_high_banks() {
        // RGBA32 (Bits32), source_t even: exercises SplitBanks lane
        // interleave, independent of the Linear odd-row path.
        let spec = BlockFixtureSpec {
            format: 0,
            size: 3,
            image_width: 2,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 255,
            source_s: 0,
            source_t: 0,
            high_s: 1,
            dxt: 0,
            source_byte_len: 8,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 1);
        assert!(matches!(
            words[0].0.physical(),
            TmemTransferPhysicalWord::SplitBanks { .. }
        ));
        assert_eq!(
            words[0].1,
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
    }

    fn spec_for_two_word_transfer() -> BlockFixtureSpec {
        BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 511,
            source_s: 0,
            source_t: 0,
            high_s: 4,
            dxt: 0,
            source_byte_len: 10,
        }
    }

    fn publish(
        decoded: crate::DecodedRawDpc,
        mut backend: BackendCompletionAuthority,
        mut guest: GuestCommitAuthority,
        mut state: PhysicalTmemState,
        pending: crate::tmem::PendingTmemTransaction,
    ) -> PhysicalTmemState {
        let report = BackendEffectReport::try_new(
            decoded.submitted().packet(),
            pending.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt = backend.issue(decoded.submitted(), report).unwrap();
        let submitted = decoded.into_contract_parts().submitted;
        let complete = submitted.gpu_complete(receipt).unwrap();
        let gpu_bound = pending.bind_gpu(&complete).unwrap();
        let effects = GuestCommitEffectReport::try_new(&complete, Vec::new()).unwrap();
        let guest_receipt = guest.issue(&complete, effects).unwrap();
        let guest_ticket = complete.commit_guest(guest_receipt).unwrap();
        state
            .publication_authority()
            .publish(gpu_bound, guest_ticket)
            .unwrap();
        state
    }

    /// RT64 `rt64_rdp.cpp:369-397` (`Copy the entire word`) makes a block's
    /// padded tail lanes valid and fills them with the adjacent RDRAM bytes.
    #[test]
    fn padded_tail_bytes_are_staged_valid_with_adjacent_rdram_values() {
        let spec = spec_for_two_word_transfer();
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        assert_eq!(transfer.words().len(), 2);
        assert_eq!(transfer.words()[1].defined_source_byte_mask(), 0xff);
        assert_eq!(transfer.words()[0].destination_word(), 511);
        assert_eq!(transfer.words()[1].destination_word(), 0);

        let state = PhysicalTmemState::try_new().unwrap();
        let prepared = prepare_load_block(fixture.decoded.submitted(), &transfer).unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let executed = prepared.execute(staged).unwrap();
        let pending = executed.into_packet().into_pending().unwrap();
        let state = publish(
            fixture.decoded,
            fixture.backend,
            fixture.guest,
            state,
            pending,
        );

        for address in 0_u16..8 {
            assert!(
                state.byte_is_valid(address),
                "byte {address} should be valid"
            );
            assert_eq!(state.valid_byte(address), Some(8 + address as u8));
        }
    }

    #[test]
    fn a_checked_load_tile_cannot_enter_the_load_block_executor() {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19 | 4),
            0x200,
            word(SET_TILE, 2 << 19 | 3 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 4),
            7 << 24 | 16 << 12 | 8,
        ];
        let packet = packet_from_words(words, &[(0x20a, 0x21e)]);
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        let tile = decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::LoadTile(load) => Some(load),
                _ => None,
            })
            .unwrap();
        let transfer = decoded.resource_plan().bind_tmem_transfer(tile).unwrap();
        assert!(matches!(
            prepare_load_block(decoded.submitted(), &transfer),
            Err(LoadBlockExecutionError::WrongLoadKind)
        ));
    }

    #[test]
    fn exact_submission_and_staged_transaction_bindings_reject_rebinding() {
        let spec = spec_for_two_word_transfer();
        let first = fixture(spec);
        let second = fixture(BlockFixtureSpec {
            image_address: 0x300,
            ..spec
        });
        let first_transfer = first
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&first.decoded))
            .unwrap();
        assert!(matches!(
            prepare_load_block(second.decoded.submitted(), &first_transfer),
            Err(LoadBlockExecutionError::SubmissionMismatch { .. })
        ));

        let prepared = prepare_load_block(first.decoded.submitted(), &first_transfer).unwrap();
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
            Err(LoadBlockExecutionError::StagedBindingMismatch { .. })
        ));
    }

    #[test]
    fn a_fault_after_each_word_discards_the_packet_clone() {
        let spec = spec_for_two_word_transfer();
        let state = PhysicalTmemState::try_new().unwrap();
        let before_generation = state.generation();
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        for completed_words in 1..=transfer.words().len() {
            let prepared = prepare_load_block(fixture.decoded.submitted(), &transfer).unwrap();
            let staged = state
                .stage_transfer(fixture.decoded.submitted(), &transfer)
                .unwrap();
            assert!(matches!(
                prepared.execute_inner(staged, Some(completed_words)),
                Err(LoadBlockExecutionError::InjectedTestFault {
                    completed_words: actual,
                }) if actual == completed_words
            ));
            assert_eq!(state.generation(), before_generation);
            assert!(!state.byte_is_valid(0));
        }
    }

    /// Two chained LoadBlocks in one packet transaction: the first succeeds,
    /// then each of wrong-kind / reordered / duplicate / poisoned second
    /// loads is proven to roll back the *whole* packet (not just its own
    /// load), leaving durable generation and bytes exactly as before either
    /// load began. This exercises `PreparedLoadBlock::execute` chained onto
    /// an already-staged packet -- the LoadBlock-specific wrapper, not just
    /// M4.2a's generic `physical.rs` mechanism.
    #[test]
    fn chained_second_load_rollback_leaves_durable_state_unchanged_on_every_failure_mode() {
        let spec = BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 16,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 0,
            high_s: 3,
            dxt: 0,
            source_byte_len: 0,
        };

        // A wrong-kind second load: a LoadTile-shaped transfer, decoded from
        // an unrelated submission, chained after a successful first
        // LoadBlock. `execute_load_block`-equivalent rejection must happen
        // before the packet is ever touched.
        {
            let state = PhysicalTmemState::try_new().unwrap();
            let both = two_load_fixture(spec);
            let first_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 0))
                .unwrap();
            let prepared = prepare_load_block(both.decoded.submitted(), &first_transfer).unwrap();
            let staged = state
                .stage_transfer(both.decoded.submitted(), &first_transfer)
                .unwrap();
            let executed = prepared.execute(staged).unwrap();
            assert_eq!(executed.ordered_fragments().len(), 1);
            let packet = executed.into_packet();

            let tile_words = vec![
                word(SET_TEXTURE_IMAGE, 2 << 19 | 4),
                0x200,
                word(SET_TILE, 2 << 19 | 3 << 9),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 4),
                7 << 24 | 16 << 12 | 8,
            ];
            let tile_packet = packet_from_words(tile_words, &[(0x20a, 0x21e)]);
            let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
            let tile_submitted = queue.submit(DecodedTicket::new(tile_packet)).unwrap();
            let tile_decoded = decode_raw_dpc(tile_submitted, &RdpState::default()).unwrap();
            let tile_load = tile_decoded
                .commands()
                .iter()
                .find_map(|command| match command.kind() {
                    RawDpcCommandKind::LoadTile(load) => Some(load),
                    _ => None,
                })
                .unwrap();
            let tile_transfer = tile_decoded
                .resource_plan()
                .bind_tmem_transfer(tile_load)
                .unwrap();
            assert!(matches!(
                prepare_load_block(tile_decoded.submitted(), &tile_transfer),
                Err(LoadBlockExecutionError::WrongLoadKind)
            ));
            // Wrong-kind rejection happens before ever touching `packet`;
            // the packet transaction is simply dropped by the caller.
            drop(packet);
            assert_eq!(state.generation(), 0);
            assert_eq!(state.last_load_epoch(), None);
            assert!(!state.byte_is_valid(0));
        }

        // Reordered/duplicate second load: chain the first load's own
        // already-staged word payload onto the packet a second time instead
        // of the second load's own words -- a cross-load payload replay,
        // the same hostile shape `tmem::physical`'s own poisoning tests use.
        {
            let state = PhysicalTmemState::try_new().unwrap();
            let both = two_load_fixture(spec);
            let first_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 0))
                .unwrap();
            let staged = state
                .stage_transfer(both.decoded.submitted(), &first_transfer)
                .unwrap();
            let first_word = staged.expected_words()[0];
            let first_prepared =
                prepare_load_block(both.decoded.submitted(), &first_transfer).unwrap();
            let executed = first_prepared.execute(staged).unwrap();
            let packet = executed.into_packet();

            let second_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 1))
                .unwrap();
            let mut second_staged = packet
                .stage_transfer(both.decoded.submitted(), &second_transfer)
                .unwrap();
            let stale_lanes = [
                Some(0x00),
                Some(0x01),
                Some(0x02),
                Some(0x03),
                Some(0x04),
                Some(0x05),
                Some(0x06),
                Some(0x07),
            ];
            let stale_payload = second_staged
                .physical_word_payload(first_word, stale_lanes)
                .unwrap();
            assert!(second_staged.stage_word(stale_payload).is_err());
            drop(second_staged);
            assert_eq!(state.generation(), 0);
            assert_eq!(state.last_load_epoch(), None);
            assert!(!state.byte_is_valid(0));
        }

        // Poisoned second load: a correctly-*prepared* second `PreparedLoadBlock`
        // is chained onto the first load's packet through the wrapper itself
        // (`execute_inner`, the same one-use operation `execute` calls), with
        // a fault injected after its own first completed word -- proving the
        // wrapper's own rollback-on-failure path, not just that raw
        // `StagedTmemTransaction::stage_word` rejects a hostile payload.
        {
            let state = PhysicalTmemState::try_new().unwrap();
            let both = two_load_fixture(spec);
            let first_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 0))
                .unwrap();
            let first_prepared =
                prepare_load_block(both.decoded.submitted(), &first_transfer).unwrap();
            let staged = state
                .stage_transfer(both.decoded.submitted(), &first_transfer)
                .unwrap();
            let executed = first_prepared.execute(staged).unwrap();
            let packet = executed.into_packet();

            let second_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 1))
                .unwrap();
            let second_prepared =
                prepare_load_block(both.decoded.submitted(), &second_transfer).unwrap();
            let second_staged = packet
                .stage_transfer(both.decoded.submitted(), &second_transfer)
                .unwrap();
            assert_eq!(second_staged.expected_words().len(), 1);
            // Fault injected after the second load's own (and only) word:
            // the wrapper must still surface `InjectedTestFault` and drop
            // the poisoned chained candidate rather than finishing the load.
            assert!(matches!(
                second_prepared.execute_inner(second_staged, Some(1)),
                Err(LoadBlockExecutionError::InjectedTestFault { completed_words: 1 })
            ));
            assert_eq!(state.generation(), 0);
            assert_eq!(state.last_load_epoch(), None);
            assert!(!state.byte_is_valid(0));
        }

        // Binding-mismatch second load: a `PreparedLoadBlock` genuinely
        // prepared against the *first* load's transfer is fed the *second*
        // load's already-staged transaction -- an injected binding failure
        // through the same wrapper, distinct from the fault-injection case
        // above. The consumed chained candidate must drop without touching
        // durable state.
        {
            let state = PhysicalTmemState::try_new().unwrap();
            let both = two_load_fixture(spec);
            let first_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 0))
                .unwrap();
            let first_prepared =
                prepare_load_block(both.decoded.submitted(), &first_transfer).unwrap();
            let staged = state
                .stage_transfer(both.decoded.submitted(), &first_transfer)
                .unwrap();
            let executed = first_prepared.execute(staged).unwrap();
            let packet = executed.into_packet();

            let second_transfer = both
                .decoded
                .resource_plan()
                .bind_tmem_transfer(load_at(&both.decoded, 1))
                .unwrap();
            // Prepared against the *first* load's transfer again, then
            // chained onto the packet's *second*-load staged transaction:
            // the wrapper's own word-shape/epoch binding check must reject
            // this mismatched payload, not a re-derived generic check.
            let mismatched_prepared =
                prepare_load_block(both.decoded.submitted(), &first_transfer).unwrap();
            let second_staged = packet
                .stage_transfer(both.decoded.submitted(), &second_transfer)
                .unwrap();
            assert!(matches!(
                mismatched_prepared.execute(second_staged),
                Err(LoadBlockExecutionError::StagedBindingMismatch { .. })
            ));
            let _ = second_transfer;
            assert_eq!(state.generation(), 0);
            assert_eq!(state.last_load_epoch(), None);
            assert!(!state.byte_is_valid(0));
        }
    }

    /// FAIL-BEFORE / PASS-AFTER for the LoadBlock odd-row XOR4 mismatch.
    ///
    /// The RDP interleaves TMEM rows by XOR-ing the address by 4 bytes on odd
    /// rows. What makes a texel come back correct is not which absolute
    /// address the exchange lands on, but that the WRITER and the READER
    /// compute the same exchange bit for the same row.
    ///
    /// The reference lane guarantees this structurally for LoadBlock. Its loader
    /// (`src/core/n64video/rdp/tex.c:907-937`) ASSIGNS the command's TL into
    /// the tile: `wstate->tile[tilenum].tl = tl`. The write side then makes
    /// its row tile-relative through `tc_pipeline_load`'s
    /// `TRELATIVE(sst1, tile->tl)` (`tcoord.c:998-999`) and takes
    /// `dswap = sst & 1` (`tex.c:583`); the read side takes `t & 1` on the
    /// equally tile-relative row, and `fetch_texel` (`tmem.c:63`) never reads
    /// `tile->tl` at all. One field, both sides, so they cannot disagree.
    ///
    /// fn64 splits them. The writer
    /// (`tmem/types.rs`, `TmemLoadKind::Block`) uses
    /// `(source_t.raw() + advance) & 1`; the reader
    /// (`tmem/read.rs::odd_row_exchange`) uses
    /// `(low_t.integer() & 1) ^ (row & 1)`. Those are different fields AND
    /// different units -- `.raw()` is the plain integer row LoadBlock's TL
    /// carries, `.integer()` is `raw >> 2` of `SetTileSize`'s S10.2 field.
    ///
    /// This fixture uses `source_t = 1`, an ODD block row, against a tile
    /// whose `SetTileSize` was never issued, so the reader's `low_t` is 0 --
    /// EVEN. Writer says exchange, reader says no exchange. Every texel on
    /// that row is then fetched from the wrong 4-byte half of its 64-bit
    /// word.
    ///
    /// The assertion is on the two exchange bits directly rather than on a
    /// decoded colour, because that is the actual invariant: a colour
    /// assertion would also pass if both sides were wrong in the same
    /// direction.
    ///
    /// MUTATION NOTE: `source_t: 1` is load-bearing. At `source_t: 0` the
    /// writer and reader both compute `false` and the test passes under the
    /// bug -- the classic fixture-where-both-answers-coincide. The
    /// `even_block_row` sibling below pins that coincidence deliberately, so
    /// a fix that simply forces the exchange off for every row is caught.
    #[test]
    fn odd_block_row_writer_and_reader_agree_on_the_exchange() {
        let spec = BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 1,
            high_s: 3,
            dxt: 0,
            source_byte_len: 8,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 1, "fixture is a single-word transfer");
        let writer_exchange = words[0].0.odd_row_exchange();

        // The reader's rule, applied to the same word. `row_advance` is the
        // tile-relative row this word lands on, and that row's parity is the
        // whole rule -- see `tmem/read.rs::odd_row_exchange` for the
        // RT64 citation that there is no T-origin term on either side.
        let row = words[0].0.row_advance();
        let reader_exchange = row & 1 != 0;

        assert_eq!(
            writer_exchange, reader_exchange,
            "LoadBlock writer and reader disagree on the odd-row XOR4 for \
             row {row}: writer={writer_exchange}, reader={reader_exchange}. \
             Every texel on this row is fetched from the wrong 4-byte lane."
        );
    }

    /// The coincidence control for the fixture above: at an EVEN block row
    /// the writer and reader agree even under the bug. Kept so that a
    /// "fix" which hardwires the exchange to a constant is caught rather
    /// than credited.
    #[test]
    fn even_block_row_writer_and_reader_agree_on_the_exchange() {
        let spec = BlockFixtureSpec {
            format: 0,
            size: 2,
            image_width: 8,
            image_address: 0x200,
            tile_line_words: 1,
            tile_tmem: 0,
            source_s: 0,
            source_t: 0,
            high_s: 3,
            dxt: 0,
            source_byte_len: 8,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 1);
        let writer_exchange = words[0].0.odd_row_exchange();
        let row = words[0].0.row_advance();
        let reader_exchange = row & 1 != 0;
        assert_eq!(writer_exchange, reader_exchange);
        assert!(
            !writer_exchange,
            "an even block row's first word must not exchange"
        );
    }
}
