//! Transaction-local LoadTLUT execution.
//!
//! Source addressing and destination geometry come from the public SGI
//! *Nintendo 64 RDP Command Summary* Table 9, Programming Manual section
//! 13.8/13.9, and the public libultra `gbi.h` `gDPLoadTLUTCmd` macro shape.
//! The checked M4.3.1/M4.3.1b transfer plan remains the geometry authority:
//! this module binds its single global source access to the exact M4.0
//! packet-owned capture and quadricates each entry's 2 captured source bytes
//! into all 8 physical lanes of its high-bank destination word. RT64 is not
//! hardware authority for this executor.

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

/// One-use LoadTLUT operation prepared from one exact submitted packet's
/// checked transfer and packet-owned guest reads.
///
/// It contains no guest-memory borrow or raw RDRAM slice. Execution consumes
/// it, and an error drops the packet-local staged clone without changing the
/// durable physical TMEM state.
pub struct PreparedLoadTlut {
    source: TmemLoadSourceIdentity,
    queue: QueueIdentity,
    submission_ordinal: u64,
    epoch: TmemLoadEpoch,
    words: Box<[PreparedLoadTlutWord]>,
}

impl fmt::Debug for PreparedLoadTlut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedLoadTlut")
            .field("source", &self.source)
            .field("queue", &self.queue)
            .field("submission_ordinal", &self.submission_ordinal)
            .field("epoch", &self.epoch)
            .field("word_count", &self.words.len())
            .finish()
    }
}

struct PreparedLoadTlutWord {
    transfer: TmemTransferWord,
    physical_lanes: [Option<u8>; 8],
}

/// Transaction-local result of one complete LoadTLUT.
///
/// The packet owner can chain another exact transfer or seal the packet. The
/// ordered physical fragments are descriptors only; backend effects remain
/// owned by M4.2a and are not reported or published here.
pub struct ExecutedLoadTlut {
    packet: PhysicalTmemPacketTransaction,
    ordered_fragments: Box<[TmemTransferPhysicalWord]>,
}

impl fmt::Debug for ExecutedLoadTlut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecutedLoadTlut")
            .field("binding", &self.packet.binding())
            .field("ordered_fragments", &self.ordered_fragments)
            .finish()
    }
}

impl ExecutedLoadTlut {
    pub fn ordered_fragments(&self) -> &[TmemTransferPhysicalWord] {
        &self.ordered_fragments
    }

    pub fn into_packet(self) -> PhysicalTmemPacketTransaction {
        self.packet
    }
}

/// Validates and captures the exact logical bytes for one checked LoadTLUT.
///
/// `submitted.packet().guest_reads()` is used directly so an independently
/// supplied, equal-looking read set cannot be rebound to this submission.
pub fn prepare_load_tlut(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
) -> Result<PreparedLoadTlut, LoadTlutExecutionError> {
    let load = transfer.load();
    let TmemLoadKind::Tlut { .. } = load.kind() else {
        return Err(LoadTlutExecutionError::WrongLoadKind);
    };
    let plan = load
        .transfer_plan()
        .map_err(|_| LoadTlutExecutionError::WrongLoadKind)?;
    let source = plan.source().identity();
    validate_submission(submitted, source)?;
    validate_transfer_shape(transfer)?;
    let reads = ExactLoadTlutGuestReads::bind(
        submitted.packet().guest_reads(),
        transfer.source_accesses(),
        plan.source().first_access_index(),
    )?;

    let mut words = Vec::with_capacity(transfer.words().len());
    for (ordinal, word) in transfer.words().iter().copied().enumerate() {
        validate_word(plan, ordinal, word)?;
        let bytes = reads.bytes_for_word(word)?;
        words.push(PreparedLoadTlutWord {
            transfer: word,
            physical_lanes: map_physical_lanes(word, bytes)?,
        });
    }

    Ok(PreparedLoadTlut {
        source,
        queue: submitted.queue(),
        submission_ordinal: submitted.ordinal(),
        epoch: load.epoch(),
        words: words.into_boxed_slice(),
    })
}

impl PreparedLoadTlut {
    /// Consumes this exact operation and the active M4.2a load transaction.
    pub fn execute(
        self,
        staged: StagedTmemTransaction,
    ) -> Result<ExecutedLoadTlut, LoadTlutExecutionError> {
        self.execute_inner(staged, None)
    }

    fn execute_inner(
        self,
        mut staged: StagedTmemTransaction,
        #[cfg_attr(not(test), allow(unused_variables))] fault_after_word: Option<usize>,
    ) -> Result<ExecutedLoadTlut, LoadTlutExecutionError> {
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
                return Err(LoadTlutExecutionError::StagedBindingMismatch { field });
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
            return Err(LoadTlutExecutionError::StagedBindingMismatch {
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
                .map_err(LoadTlutExecutionError::Physical)?;
            staged
                .stage_word(payload)
                .map_err(LoadTlutExecutionError::Physical)?;
            #[cfg(test)]
            {
                completed_words += 1;
                if fault_after_word == Some(completed_words) {
                    return Err(LoadTlutExecutionError::InjectedTestFault { completed_words });
                }
            }
        }
        let packet = staged
            .finish_load()
            .map_err(LoadTlutExecutionError::Physical)?;
        Ok(ExecutedLoadTlut {
            packet,
            ordered_fragments: fragments.into_boxed_slice(),
        })
    }
}

struct ExactLoadTlutGuestReads<'a> {
    reads: &'a OwnedGuestReadSet,
    source_accesses: &'a [ResourceAccess],
    first_access_index: u32,
}

impl<'a> ExactLoadTlutGuestReads<'a> {
    fn bind(
        reads: &'a OwnedGuestReadSet,
        source_accesses: &'a [ResourceAccess],
        first_access_index: u32,
    ) -> Result<Self, LoadTlutExecutionError> {
        if source_accesses.is_empty() {
            return Err(LoadTlutExecutionError::EmptySourcePlan);
        }

        for (ordinal, access) in source_accesses.iter().copied().enumerate() {
            let ordinal =
                u32::try_from(ordinal).map_err(|_| LoadTlutExecutionError::SourceIndexOverflow)?;
            let access_index = first_access_index
                .checked_add(ordinal)
                .ok_or(LoadTlutExecutionError::SourceIndexOverflow)?;
            let captured = reads
                .reads()
                .iter()
                .find(|captured| captured.read().access_index() == access_index)
                .ok_or(LoadTlutExecutionError::MissingGuestRead { access_index })?;
            validate_capture(access_index, access, captured.read())?;
        }
        Ok(Self {
            reads,
            source_accesses,
            first_access_index,
        })
    }

    fn bytes_for_word(&self, word: TmemTransferWord) -> Result<&'a [u8], LoadTlutExecutionError> {
        let relative = word
            .source_access_index()
            .checked_sub(self.first_access_index)
            .ok_or(LoadTlutExecutionError::WordSourceMismatch { word: word.index() })?;
        let access = self
            .source_accesses
            .get(relative as usize)
            .copied()
            .ok_or(LoadTlutExecutionError::WordSourceMismatch { word: word.index() })?;
        let captured = self
            .reads
            .reads()
            .iter()
            .find(|captured| captured.read().access_index() == word.source_access_index())
            .ok_or(LoadTlutExecutionError::MissingGuestRead {
                access_index: word.source_access_index(),
            })?;
        validate_capture(word.source_access_index(), access, captured.read())?;

        let defined = word.defined_source_byte_mask().count_ones() as usize;
        let start = word.source_access_byte_offset() as usize;
        let end = start
            .checked_add(defined)
            .ok_or(LoadTlutExecutionError::SourceIndexOverflow)?;
        captured
            .bytes()
            .get(start..end)
            .ok_or(LoadTlutExecutionError::SourceWordOutOfBounds { word: word.index() })
    }
}

fn validate_capture(
    access_index: u32,
    access: ResourceAccess,
    read: fn64_render_ir::DeferredGuestRead,
) -> Result<(), LoadTlutExecutionError> {
    let ResourceRegion::Rdram { resource, range } = access.region() else {
        return Err(LoadTlutExecutionError::GuestReadDescriptorMismatch { access_index });
    };
    if access.mode() != AccessMode::Read
        || access.purpose() != AccessPurpose::TmemLoadSource
        || resource != RdramResource::Buffer
        || read.access_index() != access_index
        || read.operation() != access.operation()
        || read.resource() != resource
        || read.range() != range
    {
        return Err(LoadTlutExecutionError::GuestReadDescriptorMismatch { access_index });
    }
    Ok(())
}

fn validate_submission(
    submitted: &SubmittedTicket,
    source: TmemLoadSourceIdentity,
) -> Result<(), LoadTlutExecutionError> {
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
            return Err(LoadTlutExecutionError::SubmissionMismatch { field });
        }
    }
    Ok(())
}

fn validate_transfer_shape(transfer: &BoundTmemTransfer<'_>) -> Result<(), LoadTlutExecutionError> {
    let plan = transfer
        .load()
        .transfer_plan()
        .map_err(|_| LoadTlutExecutionError::WrongLoadKind)?;
    if plan.transfer_words() as usize != transfer.words().len() {
        return Err(LoadTlutExecutionError::TransferMismatch {
            field: "word count",
        });
    }
    if plan.source().access_count() as usize != transfer.source_accesses().len() {
        return Err(LoadTlutExecutionError::TransferMismatch {
            field: "source access count",
        });
    }
    Ok(())
}

fn validate_word(
    plan: super::super::TmemTransferPlan,
    ordinal: usize,
    word: TmemTransferWord,
) -> Result<(), LoadTlutExecutionError> {
    let ordinal =
        u16::try_from(ordinal).map_err(|_| LoadTlutExecutionError::SourceIndexOverflow)?;
    let exact = word.index() == ordinal
        && word.logical_source_offset()
            == plan.logical_source_offset(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "logical source offset",
                }
            })?
        && word.defined_source_byte_mask()
            == plan.defined_source_byte_mask(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "defined source mask",
                }
            })?
        && word.defined_destination_byte_mask()
            == plan.defined_destination_byte_mask(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "defined destination mask",
                }
            })?
        && word.destination_word()
            == plan.destination_word(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "destination word",
                }
            })?
        && word.row_advance()
            == plan.row_advance_for_word(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "row advance",
                }
            })?
        && !word.odd_row_exchange()
        && word.odd_row_exchange()
            == plan.word_uses_odd_row_exchange(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "row exchange",
                }
            })?
        && word.physical()
            == plan.physical_word(ordinal).map_err(|_| {
                LoadTlutExecutionError::TransferMismatch {
                    field: "physical fragments",
                }
            })?
        && matches!(word.physical(), TmemTransferPhysicalWord::Linear(_));
    let source_mask = word.defined_source_byte_mask();
    let destination_mask = word.defined_destination_byte_mask();
    if !exact || source_mask != 0x03 || destination_mask != 0xff {
        return Err(LoadTlutExecutionError::TransferMismatch {
            field: "ordered transfer word",
        });
    }
    Ok(())
}

/// Quadricates one TLUT entry's 2 captured source bytes into all four 16-bit
/// lanes of the destination word's linear physical layout: `[hi, lo, hi, lo,
/// hi, lo, hi, lo]`. LoadTLUT never uses split-bank physical layout or
/// odd-row exchange (`validate_word` above already proves both facts for
/// this word before this function runs), so there is exactly one physical
/// shape to map into.
pub(crate) fn map_physical_lanes(
    word: TmemTransferWord,
    source: &[u8],
) -> Result<[Option<u8>; 8], LoadTlutExecutionError> {
    let [hi, lo] = source else {
        return Err(LoadTlutExecutionError::SourceWordOutOfBounds { word: word.index() });
    };
    Ok([
        Some(*hi),
        Some(*lo),
        Some(*hi),
        Some(*lo),
        Some(*hi),
        Some(*lo),
        Some(*hi),
        Some(*lo),
    ])
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoadTlutExecutionError {
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

impl fmt::Display for LoadTlutExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLoadKind => formatter.write_str("LoadTLUT executor requires a checked LoadTLUT transfer"),
            Self::EmptySourcePlan => formatter.write_str("LoadTLUT source plan is empty"),
            Self::SourceIndexOverflow => formatter.write_str("LoadTLUT source index overflows"),
            Self::SubmissionMismatch { field } => write!(formatter, "LoadTLUT transfer belongs to another submission at {field}"),
            Self::StagedBindingMismatch { field } => write!(formatter, "prepared LoadTLUT belongs to another staged transaction at {field}"),
            Self::TransferMismatch { field } => write!(formatter, "checked LoadTLUT transfer differs at {field}"),
            Self::MissingGuestRead { access_index } => write!(formatter, "LoadTLUT source access {access_index} has no exact owned guest read"),
            Self::GuestReadDescriptorMismatch { access_index } => write!(formatter, "LoadTLUT source access {access_index} differs from its owned guest-read descriptor"),
            Self::WordSourceMismatch { word } => write!(formatter, "LoadTLUT word {word} refers outside its exact source-access slice"),
            Self::SourceWordOutOfBounds { word } => write!(formatter, "LoadTLUT word {word} does not capture exactly 2 quadricated source bytes"),
            Self::Physical(error) => error.fmt(formatter),
            #[cfg(test)]
            Self::InjectedTestFault { completed_words } => write!(formatter, "injected LoadTLUT fault after {completed_words} words"),
        }
    }
}

impl std::error::Error for LoadTlutExecutionError {}

impl From<PhysicalTmemError> for LoadTlutExecutionError {
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
    use crate::tmem::{LOAD_SYNC, LOAD_TLUT, SET_TEXTURE_IMAGE, SET_TILE};
    use crate::{PhysicalTmemState, RdpState};

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    #[derive(Clone, Copy)]
    struct TlutFixtureSpec {
        image_address: u32,
        tile: u32,
        tmem_base: u16,
        entries: u16,
    }

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
    }

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
    }

    fn commands(spec: TlutFixtureSpec) -> Vec<u32> {
        vec![
            word(SET_TEXTURE_IMAGE, 2 << 19),
            spec.image_address,
            word(
                SET_TILE,
                2 << 19 | spec.tile << 9 | u32::from(spec.tmem_base),
            ),
            spec.tile << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TLUT, 0),
            spec.tile << 24 | u32::from(spec.entries - 1) << 14,
        ]
    }

    fn source_range(spec: TlutFixtureSpec) -> (u32, u32) {
        let bytes = u32::from(spec.entries) * 2;
        (spec.image_address, spec.image_address + bytes)
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

    fn packet(spec: TlutFixtureSpec) -> WorkloadPacket {
        packet_from_words(commands(spec), &[source_range(spec)])
    }

    fn fixture(spec: TlutFixtureSpec) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet(spec))).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    fn fixture_from_words(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let packet = packet_from_words(words, source_ranges);
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
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
                RawDpcCommandKind::LoadTlut(load) => Some(load),
                _ => None,
            })
            .expect("fixture must contain LoadTLUT")
    }

    fn prepared_lanes(spec: TlutFixtureSpec) -> Vec<(TmemTransferWord, [Option<u8>; 8])> {
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        prepare_load_tlut(fixture.decoded.submitted(), &transfer)
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

    fn publish(state: &mut PhysicalTmemState, fixture: Fixture, executed: ExecutedLoadTlut) {
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

    // The fixture's `DeferredGuestReadCapture` (see `finalize_packet`) fills
    // each captured guest read with `address as u8` for every byte in its
    // range, so a source starting at `image_address` captures bytes
    // `[image_address as u8, image_address as u8 + 1, ...]` sequentially.
    fn expected_entry_bytes(image_address: u32, entry: u32) -> (u8, u8) {
        let hi = (image_address.wrapping_add(entry * 2)) as u8;
        let lo = (image_address.wrapping_add(entry * 2 + 1)) as u8;
        (hi, lo)
    }

    fn quadricated(hi: u8, lo: u8) -> [Option<u8>; 8] {
        [
            Some(hi),
            Some(lo),
            Some(hi),
            Some(lo),
            Some(hi),
            Some(lo),
            Some(hi),
            Some(lo),
        ]
    }

    #[test]
    fn a_single_minimum_entry_quadricates_into_all_four_lanes() {
        let spec = TlutFixtureSpec {
            image_address: 0x200,
            tile: 7,
            tmem_base: 256,
            entries: 1,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].0.destination_word(), 256);
        assert!(!words[0].0.odd_row_exchange());
        let (hi, lo) = expected_entry_bytes(spec.image_address, 0);
        assert_eq!(words[0].1, quadricated(hi, lo));
    }

    #[test]
    fn a_full_256_entry_load_fills_the_entire_high_bank_in_order() {
        let spec = TlutFixtureSpec {
            image_address: 0x300,
            tile: 7,
            tmem_base: 256,
            entries: 256,
        };
        let words = prepared_lanes(spec);
        assert_eq!(words.len(), 256);
        for (entry, (word, lanes)) in words.iter().enumerate() {
            assert_eq!(word.destination_word(), 256 + entry as u16);
            let (hi, lo) = expected_entry_bytes(spec.image_address, entry as u32);
            assert_eq!(*lanes, quadricated(hi, lo));
        }
    }

    #[test]
    fn a_high_bank_base_of_511_wraps_to_word_zero_across_the_full_tmem_domain() {
        // M4.3.1c (`types.rs`'s `project_tlut_high_bank_word`) wraps the full
        // 512-word TMEM domain: base 511 + entry 1 lands at word 0, matching
        // `write_tlut`/RT64 reference parity, never re-landing in the high
        // bank (256-511).
        let spec = TlutFixtureSpec {
            image_address: 0x300,
            tile: 7,
            tmem_base: 511,
            entries: 2,
        };
        let words = prepared_lanes(spec);
        assert_eq!(
            words
                .iter()
                .map(|(word, _)| word.destination_word())
                .collect::<Vec<_>>(),
            vec![511, 0]
        );
    }

    #[test]
    fn a_high_bank_base_of_511_wraps_to_zero_across_the_full_256_entry_high_bank() {
        // Full-256-entry evidence for the M4.3.1c RT64/reference parity
        // policy: a base-511 LoadTLUT with the maximum 256 entries must land
        // entry 0 at word 511, then wrap through the *full* 512-word TMEM
        // space (511 -> 0 -> 1 -> ... -> 254), never re-landing in the high
        // bank (256-511) for entries 1..=255.
        let spec = TlutFixtureSpec {
            image_address: 0x300,
            tile: 7,
            tmem_base: 511,
            entries: 256,
        };
        let words = prepared_lanes(spec);
        let destinations = words
            .iter()
            .map(|(word, _)| word.destination_word())
            .collect::<Vec<_>>();
        assert_eq!(destinations[0], 511);
        let expected: Vec<u16> = std::iter::once(511u16).chain(0..255).collect();
        assert_eq!(destinations, expected);
        for (entry, (_word, lanes)) in words.iter().enumerate() {
            let (hi, lo) = expected_entry_bytes(spec.image_address, entry as u32);
            assert_eq!(*lanes, quadricated(hi, lo));
        }
    }

    #[test]
    fn big_endian_source_bytes_replicate_hi_lo_hi_lo_across_all_four_lanes() {
        // Byte order: the first captured byte (lower address) lands in every
        // even lane (the 16-bit "hi" position); the second (higher address)
        // lands in every odd lane ("lo") -- big-endian per-entry replication,
        // never byte-swapped.
        let spec = TlutFixtureSpec {
            image_address: 0x200,
            tile: 7,
            tmem_base: 256,
            entries: 1,
        };
        let words = prepared_lanes(spec);
        let hi = 0x200_u32 as u8;
        let lo = 0x201_u32 as u8;
        assert_eq!(
            words[0].1,
            [
                Some(hi),
                Some(lo),
                Some(hi),
                Some(lo),
                Some(hi),
                Some(lo),
                Some(hi),
                Some(lo),
            ]
        );
    }

    #[test]
    fn multiple_entries_map_in_ascending_order_to_consecutive_destination_words() {
        let spec = TlutFixtureSpec {
            image_address: 0x400,
            tile: 7,
            tmem_base: 300,
            entries: 4,
        };
        let words = prepared_lanes(spec);
        assert_eq!(
            words
                .iter()
                .map(|(word, _)| word.destination_word())
                .collect::<Vec<_>>(),
            vec![300, 301, 302, 303]
        );
        for (entry, (word, lanes)) in words.iter().enumerate() {
            assert_eq!(word.index(), entry as u16);
            let (hi, lo) = expected_entry_bytes(spec.image_address, entry as u32);
            assert_eq!(*lanes, quadricated(hi, lo));
        }
    }

    // Two LoadTLUTs in ONE packet, both targeting the same high-bank
    // destination word (tmem 256): the second command's `LOAD_SYNC` bumps
    // this shared `TmemState`'s epoch counter, so both loads chain through
    // one `PhysicalTmemPacketTransaction` with strictly increasing epochs --
    // unlike two independent `fixture()` calls, which would each start a
    // fresh `TmemState` at epoch 1 and collide on `EpochNotNewer`.
    fn two_overlapping_tlut_words(
        first_address: u32,
        second_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19),
            first_address,
            word(SET_TILE, 2 << 19 | 7 << 9 | 256),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TLUT, 0),
            7 << 24,
            word(SET_TEXTURE_IMAGE, 2 << 19),
            second_address,
            word(SET_TILE, 2 << 19 | 7 << 9 | 256),
            7 << 24,
            word(LOAD_SYNC, 1),
            0,
            word(LOAD_TLUT, 0),
            7 << 24,
        ];
        (
            words,
            vec![
                (first_address, first_address + 2),
                (second_address, second_address + 2),
            ],
        )
    }

    fn two_loads(decoded: &crate::DecodedRawDpc) -> [crate::TmemLoad; 2] {
        let mut loads = decoded
            .commands()
            .iter()
            .filter_map(|command| match command.kind() {
                RawDpcCommandKind::LoadTlut(load) => Some(load),
                _ => None,
            });
        let first = loads.next().expect("fixture must contain a first LoadTLUT");
        let second = loads
            .next()
            .expect("fixture must contain a second LoadTLUT");
        [first, second]
    }

    #[test]
    fn overlapping_loads_publish_only_the_last_loads_defined_lanes() {
        // Two LoadTLUTs to the SAME destination word (tmem 256) in one
        // packet: the second load's quadricated bytes must be the ones
        // visible after publish, not the first's.
        let (words, ranges) = two_overlapping_tlut_words(0x200, 0x500);
        let fixture = fixture_from_words(words, &ranges);
        let mut state = PhysicalTmemState::try_new().unwrap();
        let [first_load, second_load] = two_loads(&fixture.decoded);

        let first_transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(first_load)
            .unwrap();
        let first_prepared =
            prepare_load_tlut(fixture.decoded.submitted(), &first_transfer).unwrap();
        let first_staged = state
            .stage_transfer(fixture.decoded.submitted(), &first_transfer)
            .unwrap();
        let first_packet = first_prepared.execute(first_staged).unwrap().into_packet();

        let second_transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(second_load)
            .unwrap();
        let second_prepared =
            prepare_load_tlut(fixture.decoded.submitted(), &second_transfer).unwrap();
        let second_staged = first_packet
            .stage_transfer(fixture.decoded.submitted(), &second_transfer)
            .unwrap();
        let second_executed = second_prepared.execute(second_staged).unwrap();
        publish(&mut state, fixture, second_executed);
        assert_eq!(state.generation(), 1);

        let base = 256 * 8;
        let (hi, lo) = expected_entry_bytes(0x500, 0);
        assert_eq!(state.valid_byte(base), Some(hi));
        assert_eq!(state.valid_byte(base + 1), Some(lo));
        assert_eq!(state.valid_byte(base + 2), Some(hi));
        assert_eq!(state.valid_byte(base + 7), Some(lo));
        for lane in 0..8_u16 {
            assert_eq!(state.last_touched_generation(base + lane), Some(1));
        }
    }

    #[test]
    fn a_single_entry_load_produces_the_independently_computed_full_4096_byte_tmem_state() {
        // Independent oracle: recomputes expected validity and content for
        // EVERY one of TMEM's 4096 bytes by hand (address arithmetic and the
        // fixture's own `address as u8` capture convention only -- no call
        // into `map_physical_lanes`, `quadricated`, or any other production
        // helper), so a bug shared between the executor and a test helper
        // cannot hide here. A single 1-entry LoadTLUT at tmem_base 256 must
        // leave exactly 8 bytes (256*8..256*8+8) valid and quadricated, and
        // every other byte in the full 4 KiB space untouched (invalid, no
        // touch generation).
        let spec = TlutFixtureSpec {
            image_address: 0x9a2,
            tile: 7,
            tmem_base: 256,
            entries: 1,
        };
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        let prepared = prepare_load_tlut(fixture.decoded.submitted(), &transfer).unwrap();
        let mut state = PhysicalTmemState::try_new().unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let executed = prepared.execute(staged).unwrap();
        publish(&mut state, fixture, executed);

        let written_base: u32 = 256 * 8;
        let hi = (spec.image_address) as u8;
        let lo = (spec.image_address + 1) as u8;
        let expected_touched: [u8; 8] = [hi, lo, hi, lo, hi, lo, hi, lo];

        for address in 0..fn64_render_ir::TMEM_BYTES {
            let is_touched = (written_base..written_base + 8).contains(&address);
            if is_touched {
                let lane = (address - written_base) as usize;
                assert!(
                    state.byte_is_valid(address as u16),
                    "byte {address} should be valid"
                );
                assert_eq!(
                    state.valid_byte(address as u16),
                    Some(expected_touched[lane]),
                    "byte {address} content mismatch"
                );
                assert_eq!(
                    state.last_touched_generation(address as u16),
                    Some(1),
                    "byte {address} generation mismatch"
                );
            } else {
                assert!(
                    !state.byte_is_valid(address as u16),
                    "byte {address} should still be invalid"
                );
                // `last_touched_generation` returns `Some(0)` (the initial,
                // never-touched generation) for every in-range address; it
                // only returns `None` out of TMEM's 4096-byte range, which
                // this loop never reaches.
                assert_eq!(
                    state.last_touched_generation(address as u16),
                    Some(0),
                    "byte {address} should still be at the initial untouched generation"
                );
            }
        }
    }

    #[test]
    fn dropping_an_executed_but_unpublished_load_leaves_durable_state_untouched() {
        // A fully-executed LoadTLUT (`ExecutedLoadTlut` -> `PendingTmemTransaction`)
        // that is never carried through `publish` -- e.g. the caller drops it
        // after a GPU/guest-commit failure elsewhere in the pipeline -- must
        // never have touched the durable `PhysicalTmemState` it was staged
        // against. Durable state only changes inside `publication_authority
        // ().publish`, which this test deliberately never calls.
        let spec = TlutFixtureSpec {
            image_address: 0x200,
            tile: 7,
            tmem_base: 256,
            entries: 4,
        };
        let state = PhysicalTmemState::try_new().unwrap();
        let before = snapshot(&state);
        let fixture = fixture(spec);
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&fixture.decoded))
            .unwrap();
        let prepared = prepare_load_tlut(fixture.decoded.submitted(), &transfer).unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let executed = prepared.execute(staged).unwrap();
        let pending = executed.into_packet().into_pending().unwrap();
        assert_eq!(pending.completed_loads(), 1);
        // Dropped here, deliberately unpublished.
        drop(pending);
        assert_eq!(snapshot(&state), before);
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn exact_submission_and_staged_transaction_bindings_reject_rebinding() {
        let spec = TlutFixtureSpec {
            image_address: 0x200,
            tile: 7,
            tmem_base: 256,
            entries: 3,
        };
        let first = fixture(spec);
        let second = fixture(TlutFixtureSpec {
            image_address: 0x300,
            ..spec
        });
        let first_transfer = first
            .decoded
            .resource_plan()
            .bind_tmem_transfer(load(&first.decoded))
            .unwrap();
        assert!(matches!(
            prepare_load_tlut(second.decoded.submitted(), &first_transfer),
            Err(LoadTlutExecutionError::SubmissionMismatch { .. })
        ));

        let prepared = prepare_load_tlut(first.decoded.submitted(), &first_transfer).unwrap();
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
            Err(LoadTlutExecutionError::StagedBindingMismatch { .. })
        ));
    }

    #[test]
    fn a_checked_load_tile_cannot_enter_the_load_tlut_executor() {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            0x200,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(crate::tmem::LOAD_TILE, 0),
            7 << 24 | 28 << 12,
        ];
        let packet = packet_from_words(words, &[(0x200, 0x208)]);
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
            prepare_load_tlut(decoded.submitted(), &transfer),
            Err(LoadTlutExecutionError::WrongLoadKind)
        ));
    }

    #[test]
    fn a_checked_load_block_cannot_enter_the_load_tlut_executor() {
        let image_address = 0x200_u32;
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19 | 7),
            image_address,
            word(SET_TILE, 2 << 19 | 2 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(crate::tmem::LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ];
        let packet = packet_from_words(words, &[(image_address + 0x14, image_address + 0x24)]);
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
            prepare_load_tlut(decoded.submitted(), &transfer),
            Err(LoadTlutExecutionError::WrongLoadKind)
        ));
    }

    #[test]
    fn a_fault_after_each_word_discards_the_packet_clone() {
        let spec = TlutFixtureSpec {
            image_address: 0x200,
            tile: 7,
            tmem_base: 256,
            entries: 3,
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
            let prepared = prepare_load_tlut(fixture.decoded.submitted(), &transfer).unwrap();
            let staged = state
                .stage_transfer(fixture.decoded.submitted(), &transfer)
                .unwrap();
            assert!(matches!(
                prepared.execute_inner(staged, Some(completed_words)),
                Err(LoadTlutExecutionError::InjectedTestFault {
                    completed_words: actual,
                }) if actual == completed_words
            ));
            assert_eq!(snapshot(&state), before);
        }
    }
}
