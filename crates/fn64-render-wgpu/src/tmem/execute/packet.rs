//! Packet-level sequencing of every executable TMEM load in one decoded
//! raw-DPC command stream.
//!
//! M4.2b's LoadTile ([`super::load_tile`]), M4.2c's LoadBlock
//! ([`super::load_block`]), and M4.3.2's LoadTLUT ([`super::load_tlut`])
//! executors each drive exactly one checked transfer through the M4.2a
//! physical-TMEM typestate engine. None of them, nor [`crate::raw_dpc`],
//! walks [`DecodedRawDpc::commands()`] and chains executors across every load
//! declared by one packet. This module is that outer loop: it dispatches
//! each ordered `LoadTile`/`LoadBlock`/`LoadTlut` command to its executor,
//! chains the resulting [`PhysicalTmemPacketTransaction`] across the whole
//! packet, and seals it into one [`PendingTmemTransaction`]. A YUV-deferred
//! Tile/Block contract is refused loudly as a scope boundary before any load
//! in the packet is staged. Rejection is decided by a dedicated validation
//! pass over every ordered command, run to completion before staging begins,
//! so a later YUV command can never be preceded by an already-staged earlier
//! Tile/Block/TLUT load — neither is silently omitted from the sealed
//! transaction's destination coverage either way. RT64 is not hardware
//! authority for this module.

use core::fmt;

use fn64_render_ir::SubmittedTicket;

use crate::raw_dpc::{
    BoundTmemTransfer, DecodedRawDpc, RawDpcCommandKind, TmemLoadSourcePlanError,
};

use super::super::{
    PendingTmemTransaction, PhysicalTmemError, PhysicalTmemPacketTransaction, PhysicalTmemState,
};
use super::{
    prepare_load_block, prepare_load_tile, prepare_load_tlut, LoadBlockExecutionError,
    LoadTileExecutionError, LoadTlutExecutionError,
};

/// Sequences every ordered executable TMEM load in `decoded.commands()` into
/// one sealed [`PendingTmemTransaction`].
///
/// Two passes, in order:
///
/// 1. **Validate.** `submitted` is checked to be exactly `decoded.submitted()`
///    (queue, submission ordinal/identity, workload, journal, and
///    memory-layout identity all typed-equal — see
///    [`validate_submitted_ticket`]), then every ordered command is scanned:
///    a `LoadTile`/`LoadBlock` command whose contract turns out to be
///    YUV-deferred rejects the whole packet here. Nothing is staged and no
///    [`PhysicalTmemPacketTransaction`] is constructed during this pass.
/// 2. **Execute.** Only once every command has cleared validation does this
///    pass dispatch each `LoadTile`/`LoadBlock`/`LoadTlut` to its
///    M4.2b/M4.2c/M4.3.2 executor in decode order, chaining the result into
///    one packet-local transaction. Because pass 1 already proved no YUV
///    command exists in this packet, pass 2 can never observe one;
///    [`checked_load`] is the single helper both passes call so that can't
///    drift apart.
///
/// This function never silently omits a declared destination from the sealed
/// transaction. Sealing (via [`PhysicalTmemPacketTransaction::into_pending`](
/// super::super::PhysicalTmemPacketTransaction::into_pending)) still enforces
/// exact destination coverage against the packet journal independently of
/// this loop.
///
/// This function stops at [`PendingTmemTransaction`]; it holds neither
/// `BackendCompletionAuthority` nor `GuestCommitAuthority`. Those remain
/// caller-owned, matching the existing one-use-authority split in
/// `fn64_render_ir::ticket` — the caller assembles the packet-wide
/// `BackendEffectReport` (TMEM proposals from `pending.proposed_effects()`
/// plus any other declared writes from the same packet, never fabricated
/// here) and drives the remaining ticket/publication steps itself.
pub fn execute_ordered_tmem_loads(
    state: &PhysicalTmemState,
    submitted: &SubmittedTicket,
    decoded: &DecodedRawDpc,
) -> Result<PendingTmemTransaction, TmemPacketExecutionError> {
    validate_submitted_ticket(submitted, decoded.submitted())?;
    for command in decoded.commands() {
        checked_load(command.kind())?;
    }

    let mut packet: Option<PhysicalTmemPacketTransaction> = None;
    for command in decoded.commands() {
        let Some((load, load_kind)) = checked_load(command.kind())? else {
            continue;
        };
        let transfer = decoded
            .resource_plan()
            .bind_tmem_transfer(load)
            .map_err(|error| TmemPacketExecutionError::DeferredLoadKind {
                error,
                epoch: load.epoch(),
            })?;
        let staged = match packet.take() {
            Some(packet) => packet.stage_transfer(submitted, &transfer),
            None => state.stage_transfer(submitted, &transfer),
        }
        .map_err(TmemPacketExecutionError::Physical)?;
        packet = Some(match load_kind {
            CheckedLoadKind::Tile => execute_tile(submitted, &transfer, staged)?,
            CheckedLoadKind::Block => execute_block(submitted, &transfer, staged)?,
            CheckedLoadKind::Tlut => execute_tlut(submitted, &transfer, staged)?,
        });
    }
    let packet = packet.ok_or(TmemPacketExecutionError::NoExecutableLoads)?;
    packet
        .into_pending()
        .map_err(TmemPacketExecutionError::Physical)
}

/// Which executor one checked command's load dispatches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckedLoadKind {
    Tile,
    Block,
    Tlut,
}

/// Checked classification of one command's load, shared by both the
/// validation pass and the execution pass so the two can never disagree
/// about which commands are YUV-deferred or executable.
///
/// Returns `Ok(None)` for a command that carries no TMEM load at all (it is
/// simply skipped). Returns `Ok(Some((load, kind)))` for a `LoadTile`,
/// `LoadBlock`, or `LoadTlut` command whose contract is not YUV-deferred.
/// Returns `Err` for a Tile/Block command whose contract is YUV-deferred.
///
/// This helper does not call [`crate::raw_dpc::RawDpcResourcePlan::bind_tmem_transfer`]
/// itself for Tile/Block/TLUT commands — the execution pass still needs the
/// borrowed [`BoundTmemTransfer`] that call produces, and a second,
/// independent lookup here would let the two passes see different contracts
/// for the same command if the resource plan were ever position-dependent.
/// Instead this helper inspects [`crate::TmemLoad::contract`] directly, the
/// same source `bind_tmem_transfer` itself reads to decide the YUV-deferred
/// case — so both passes reject on exactly the same fact. TLUT loads are
/// never YUV-deferred (`decode_load_tlut` requires a 16-bit RGBA source, and
/// [`crate::TmemLoad::new`] always mints a `Transfer` contract, never a
/// `DeferredYuv` one, for a `Tlut` kind), but this helper still checks the
/// contract uniformly rather than special-casing TLUT around that check.
fn checked_load(
    kind: RawDpcCommandKind,
) -> Result<Option<(crate::TmemLoad, CheckedLoadKind)>, TmemPacketExecutionError> {
    let (load, load_kind) = match kind {
        RawDpcCommandKind::LoadTlut(load) => (load, CheckedLoadKind::Tlut),
        RawDpcCommandKind::LoadTile(load) => (load, CheckedLoadKind::Tile),
        RawDpcCommandKind::LoadBlock(load) => (load, CheckedLoadKind::Block),
        _ => return Ok(None),
    };
    if let crate::TmemLoadContract::DeferredYuv { .. } = load.contract() {
        return Err(TmemPacketExecutionError::DeferredLoadKind {
            error: TmemLoadSourcePlanError::YuvExecutionDeferred,
            epoch: load.epoch(),
        });
    }
    Ok(Some((load, load_kind)))
}

/// Validates that `submitted` is exactly `expected` (`decoded.submitted()`)
/// before any scope/no-load exit — queue, submission ordinal, submission
/// identity, workload identity, journal identity, and memory-layout identity
/// all typed-equal via existing getters, the same fields and the same
/// `Display` phrasing `PhysicalTmemError`/`LoadTileExecutionError`/
/// `LoadBlockExecutionError`'s own `SubmissionMismatch { field }` use for an
/// analogous check elsewhere in this crate. A caller-supplied ticket that
/// names a foreign queue or submission is rejected with this named identity
/// error here, never surfacing as
/// [`TmemPacketExecutionError::NoExecutableLoads`].
fn validate_submitted_ticket(
    submitted: &SubmittedTicket,
    expected: &SubmittedTicket,
) -> Result<(), TmemPacketExecutionError> {
    for (matches, field) in [
        (submitted.queue() == expected.queue(), "queue"),
        (
            submitted.ordinal() == expected.ordinal(),
            "submission ordinal",
        ),
        (
            submitted.identity() == expected.identity(),
            "submission identity",
        ),
        (
            submitted.packet().identity() == expected.packet().identity(),
            "workload identity",
        ),
        (
            submitted.packet().journal().identity() == expected.packet().journal().identity(),
            "journal identity",
        ),
        (
            submitted.packet().memory_layout() == expected.packet().memory_layout(),
            "memory layout",
        ),
    ] {
        if !matches {
            return Err(TmemPacketExecutionError::SubmissionMismatch { field });
        }
    }
    Ok(())
}

fn execute_tile(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
    staged: super::super::StagedTmemTransaction,
) -> Result<PhysicalTmemPacketTransaction, TmemPacketExecutionError> {
    let prepared =
        prepare_load_tile(submitted, transfer).map_err(TmemPacketExecutionError::Tile)?;
    prepared
        .execute(staged)
        .map(super::ExecutedLoadTile::into_packet)
        .map_err(TmemPacketExecutionError::Tile)
}

fn execute_block(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
    staged: super::super::StagedTmemTransaction,
) -> Result<PhysicalTmemPacketTransaction, TmemPacketExecutionError> {
    let prepared =
        prepare_load_block(submitted, transfer).map_err(TmemPacketExecutionError::Block)?;
    prepared
        .execute(staged)
        .map(super::ExecutedLoadBlock::into_packet)
        .map_err(TmemPacketExecutionError::Block)
}

fn execute_tlut(
    submitted: &SubmittedTicket,
    transfer: &BoundTmemTransfer<'_>,
    staged: super::super::StagedTmemTransaction,
) -> Result<PhysicalTmemPacketTransaction, TmemPacketExecutionError> {
    let prepared =
        prepare_load_tlut(submitted, transfer).map_err(TmemPacketExecutionError::Tlut)?;
    prepared
        .execute(staged)
        .map(super::ExecutedLoadTlut::into_packet)
        .map_err(TmemPacketExecutionError::Tlut)
}

#[derive(Debug)]
pub enum TmemPacketExecutionError {
    /// `submitted` is not exactly `decoded.submitted()` — queue, submission
    /// ordinal/identity, workload, journal, or memory-layout identity
    /// disagreed at the named `field`. Checked before any other exit, so a
    /// foreign ticket can never surface as `NoExecutableLoads` instead.
    SubmissionMismatch {
        field: &'static str,
    },
    /// A `LoadTile`/`LoadBlock` command whose contract is YUV-deferred
    /// appeared in the packet. This is a scope boundary, not a bug: physical
    /// execution of YUV pairing is out of scope for this slice (M4.3). The
    /// packet is rejected before any load in it is staged.
    DeferredLoadKind {
        error: TmemLoadSourcePlanError,
        epoch: crate::TmemLoadEpoch,
    },
    /// No `LoadTile`/`LoadBlock`/`LoadTlut` command was present to execute.
    NoExecutableLoads,
    Tile(LoadTileExecutionError),
    Block(LoadBlockExecutionError),
    Tlut(LoadTlutExecutionError),
    Physical(PhysicalTmemError),
}

impl fmt::Display for TmemPacketExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmissionMismatch { field } => write!(
                formatter,
                "TMEM packet execution belongs to another submission at {field}"
            ),
            Self::DeferredLoadKind { error, epoch } => write!(
                formatter,
                "TMEM packet load at epoch {} cannot execute physically yet: {error}",
                epoch.get()
            ),
            Self::NoExecutableLoads => formatter
                .write_str("TMEM packet has no executable LoadTile/LoadBlock/LoadTlut command"),
            Self::Tile(error) => error.fmt(formatter),
            Self::Block(error) => error.fmt(formatter),
            Self::Tlut(error) => error.fmt(formatter),
            Self::Physical(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TmemPacketExecutionError {}

impl From<PhysicalTmemError> for TmemPacketExecutionError {
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
    use crate::tmem::{LOAD_BLOCK, LOAD_SYNC, LOAD_TILE, LOAD_TLUT, SET_TEXTURE_IMAGE, SET_TILE};
    use crate::{
        decode_direct_texel, filter_three_nearest_committed_cell, gather_committed_texture_cell,
        read_committed_texel, sample_committed_point, AddressedTmemTexel, ImageFormat,
        PhysicalTexelReadError, PhysicalTmemState, PhysicalTmemStateIdentity, PixelSize,
        PointSampleCoordinates, PointSampleRequest, RawTexel, RdpState, TextureCellCorner,
        TextureCellSampleError, TextureCoordinateS10_5, TextureLutMode, TileAddressMode,
        TileCoordinate, TileDescriptor, TileSize, TmemFirstRowParity, TmemLoadEpoch,
        TmemWordAddress,
    };

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct DurableObservation {
        valid_bytes: Vec<Option<u8>>,
        validity: Vec<bool>,
        touch_generations: Vec<Option<u64>>,
        identity: PhysicalTmemStateIdentity,
        generation: u64,
        epoch: Option<TmemLoadEpoch>,
    }

    fn observe_durable(state: &PhysicalTmemState) -> DurableObservation {
        let addresses = 0_u16..4096;
        DurableObservation {
            valid_bytes: addresses
                .clone()
                .map(|address| state.valid_byte(address))
                .collect(),
            validity: addresses
                .clone()
                .map(|address| state.byte_is_valid(address))
                .collect(),
            touch_generations: addresses
                .map(|address| state.last_touched_generation(address))
                .collect(),
            identity: state.identity(),
            generation: state.generation(),
            epoch: state.last_load_epoch(),
        }
    }

    fn word(opcode: u8, payload: u32) -> u32 {
        u32::from(opcode) << 24 | payload
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

    fn finalize_packet_with_sources(
        words: &[u32],
        accesses: Vec<ResourceAccess>,
        sources: &[(u32, &[u8])],
    ) -> WorkloadPacket {
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
                    let start = read.range().start().get();
                    let end = read.range().end();
                    let bytes = sources
                        .iter()
                        .find_map(|(source_start, bytes)| {
                            let source_end =
                                source_start.checked_add(u32::try_from(bytes.len()).unwrap())?;
                            if start < *source_start || end > source_end {
                                return None;
                            }
                            let first = usize::try_from(start - *source_start).unwrap();
                            let last = usize::try_from(end - *source_start).unwrap();
                            Some(bytes[first..last].to_vec())
                        })
                        .unwrap_or_else(|| (start..end).map(|address| address as u8).collect());
                    CapturedGuestRead::try_new(*read, bytes).unwrap()
                })
                .collect(),
        );
        preflight.finalize(capture).unwrap()
    }

    fn finalize_packet(words: &[u32], accesses: Vec<ResourceAccess>) -> WorkloadPacket {
        finalize_packet_with_sources(words, accesses, &[])
    }

    fn packet_from_words(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> WorkloadPacket {
        packet_from_words_with_sources(words, source_ranges, &[])
    }

    fn packet_from_words_with_sources(
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
        sources: &[(u32, &[u8])],
    ) -> WorkloadPacket {
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
        finalize_packet_with_sources(&words, expected, sources)
    }

    fn fixture(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> Fixture {
        fixture_with_state(words, source_ranges, &RdpState::default())
    }

    fn fixture_with_sources(
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
        sources: &[(u32, &[u8])],
    ) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let packet = packet_from_words_with_sources(words, source_ranges, sources);
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    fn fixture_with_state(
        words: Vec<u32>,
        source_ranges: &[(u32, u32)],
        state: &RdpState,
    ) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let packet = packet_from_words(words, source_ranges);
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, state).unwrap(),
            backend,
            guest,
        }
    }

    /// Single-pass fixture for a command stream whose declared accesses are
    /// already exact on the first attempt (no `TmemLoadDestination` claim to
    /// discover via the two-pass planning probe `fixture`/`packet_from_words`
    /// use) -- e.g. a YUV-deferred load, which journals only a `TmemLoadSource`
    /// access and no destination claim.
    fn single_pass_fixture(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> Fixture {
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        let mut accesses = vec![command_access(layout, byte_count, 0)];
        accesses.extend(
            source_ranges
                .iter()
                .copied()
                .enumerate()
                .map(|(ordinal, (start, end))| {
                    source_access(layout, ordinal as u32 + 1, start, end)
                }),
        );
        let packet = finalize_packet(&words, accesses);
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        }
    }

    // Single LoadTile: SET_TEXTURE_IMAGE, SET_TILE, LOAD_SYNC, LOAD_TILE.
    fn single_tile_words(image_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            image_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
        ];
        (words, vec![(image_address, image_address + 8)])
    }

    // Single LoadBlock: SET_TEXTURE_IMAGE, SET_TILE, LOAD_SYNC, LOAD_BLOCK.
    fn single_block_words(image_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19 | 7),
            image_address,
            word(SET_TILE, 2 << 19 | 2 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ];
        (words, vec![(image_address + 0x14, image_address + 0x24)])
    }

    fn two_tile_words(first_address: u32, second_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            first_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            second_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 1),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
        ];
        (
            words,
            vec![
                (first_address, first_address + 8),
                (second_address, second_address + 8),
            ],
        )
    }

    fn mixed_tile_then_block_words(
        tile_address: u32,
        block_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            tile_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
            word(SET_TEXTURE_IMAGE, 2 << 19 | 7),
            block_address,
            word(SET_TILE, 2 << 19 | 2 << 9),
            7 << 24,
            word(LOAD_SYNC, 1),
            0,
            word(LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ];
        (
            words,
            vec![
                (tile_address, tile_address + 8),
                (block_address + 0x14, block_address + 0x24),
            ],
        )
    }

    // YUV LoadBlock: SET_TEXTURE_IMAGE (format 1 = YUV, size 2 = 16-bit),
    // SET_TILE, LOAD_SYNC, LOAD_BLOCK. Mirrors `raw_dpc::tests`' own
    // `unpaired_yuv` fixture (format/size/dxt/texel-count), adapted to this
    // module's `word(opcode, payload)` shape and tile index 7.
    fn yuv_block_words(image_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 1 << 21 | 2 << 19 | 3),
            image_address,
            word(SET_TILE, 2 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_BLOCK, 1 << 12),
            7 << 24 | 2 << 12,
        ];
        (words, vec![(image_address + 2, image_address + 6)])
    }

    fn tile_then_yuv_block_words(
        tile_address: u32,
        yuv_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let (mut words, mut ranges) = single_tile_words(tile_address);
        let (yuv_words, yuv_ranges) = yuv_block_words(yuv_address);
        words.extend(yuv_words);
        ranges.extend(yuv_ranges);
        (words, ranges)
    }

    // Genuine Tile -> TLUT -> Block: a valid LoadTile, then a valid LoadTLUT
    // (executable since M4.3.2), then a second valid load (LoadBlock). Used
    // by `a_genuine_tile_tlut_block_packet_executes_all_three_in_decode_order`
    // below to prove all three execute in decode order in one packet.
    fn tile_tlut_block_words(
        tile_address: u32,
        tlut_address: u32,
        block_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let (mut words, mut ranges) = single_tile_words(tile_address);
        let (tlut_words, tlut_ranges) = tlut_words(tlut_address);
        words.extend(tlut_words);
        ranges.extend(tlut_ranges);
        words.extend([
            word(SET_TEXTURE_IMAGE, 2 << 19 | 7),
            block_address,
            word(SET_TILE, 2 << 19 | 2 << 9),
            7 << 24,
            word(LOAD_SYNC, 2),
            0,
            word(LOAD_BLOCK, 2 << 12 | 1),
            7 << 24 | 9 << 12 | 0x0800,
        ]);
        ranges.push((block_address + 0x14, block_address + 0x24));
        (words, ranges)
    }

    fn publish(
        state: &mut PhysicalTmemState,
        fixture: Fixture,
        pending: PendingTmemTransaction,
    ) -> crate::CommittedTmemTransaction {
        let Fixture {
            decoded,
            mut backend,
            mut guest,
        } = fixture;
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
            .unwrap()
    }

    fn reader_tile(
        format: ImageFormat,
        size: PixelSize,
        line_words: u16,
        tmem: u16,
        palette: u8,
    ) -> TileDescriptor {
        TileDescriptor::from_wire(
            format,
            size,
            line_words,
            TmemWordAddress::try_new(tmem).unwrap(),
            palette,
            TileAddressMode::default(),
            0,
            0,
            TileAddressMode::default(),
            0,
            0,
        )
    }

    fn reader_size(low_s: u16, low_t: u16, high_s: u16, high_t: u16) -> TileSize {
        TileSize::from_wire(
            TileCoordinate::try_new(low_s).unwrap(),
            TileCoordinate::try_new(low_t).unwrap(),
            TileCoordinate::try_new(high_s).unwrap(),
            TileCoordinate::try_new(high_t).unwrap(),
        )
    }

    fn direct_reader_words() -> (Vec<u32>, Vec<(u32, u32)>) {
        let linear_address = 0x201;
        let rgba32_address = 0x220;
        let odd_rows_address = 0x400;
        (
            vec![
                word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
                linear_address,
                word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9 | 8),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 28 << 12,
                word(SET_TEXTURE_IMAGE, 3 << 19 | 1),
                rgba32_address,
                word(SET_TILE, 3 << 19 | 1 << 9 | 16),
                7 << 24,
                word(LOAD_SYNC, 1),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 4 << 12,
                word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
                odd_rows_address,
                word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9 | 511),
                7 << 24,
                word(LOAD_SYNC, 2),
                0,
                word(LOAD_TILE, 4),
                7 << 24 | 28 << 12 | 8,
            ],
            vec![
                (linear_address, linear_address + 8),
                (rgba32_address, rgba32_address + 8),
                (odd_rows_address + 8, odd_rows_address + 24),
            ],
        )
    }

    fn rgba_cell_words(
        image_address: u32,
        size: PixelSize,
        tmem: u16,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let size_wire = match size {
            PixelSize::Bits16 => 2,
            PixelSize::Bits32 => 3,
            _ => unreachable!("RGBA cell fixture is 16-bit or 32-bit"),
        };
        let byte_count = if size == PixelSize::Bits16 { 8 } else { 16 };
        (
            vec![
                word(SET_TEXTURE_IMAGE, size_wire << 19 | 1),
                image_address,
                word(SET_TILE, size_wire << 19 | 1 << 9 | u32::from(tmem)),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 4 << 12 | 4,
            ],
            vec![(image_address, image_address + byte_count)],
        )
    }

    fn ci_cell_tlut_words(
        index_address: u32,
        index_width: u32,
        index_tmem: u16,
        tlut_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let index_bytes = index_width * 2;
        (
            vec![
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | (index_width - 1)),
                index_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9 | u32::from(index_tmem)),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | ((index_width - 1) * 4) << 12 | 4,
                word(SET_TEXTURE_IMAGE, 2 << 19 | 3),
                tlut_address,
                word(SET_TILE, 2 << 19 | 7 << 9 | 257),
                7 << 24,
                word(LOAD_SYNC, 1),
                0,
                word(LOAD_TLUT, 0),
                7 << 24 | 3 << 14,
            ],
            vec![
                (index_address, index_address + index_bytes),
                (tlut_address, tlut_address + 8),
            ],
        )
    }

    fn publish_sources(
        words: Vec<u32>,
        ranges: &[(u32, u32)],
        sources: &[(u32, &[u8])],
    ) -> PhysicalTmemState {
        let fixture = fixture_with_sources(words, ranges, sources);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);
        state
    }

    fn cell_request(parity: TmemFirstRowParity) -> PointSampleRequest {
        PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(16),
                TextureCoordinateS10_5::from_raw(16),
            ),
            parity,
        )
    }

    fn cell_colors(cell: crate::CommittedTextureCell) -> [[u8; 4]; 4] {
        cell.texels().map(|texel| texel.texel().rgba8888())
    }

    fn ci_and_offset_tlut_reader_words() -> (Vec<u32>, Vec<(u32, u32)>) {
        let ci8_address = 0x128;
        let ci4_address = 0x189;
        let unloaded_index_address = 0x100;
        let tlut_address = 0x300;
        (
            vec![
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | 7),
                ci8_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 28 << 12,
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | 7),
                ci4_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9 | 1),
                7 << 24,
                word(LOAD_SYNC, 1),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 28 << 12,
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | 7),
                unloaded_index_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9 | 2),
                7 << 24,
                word(LOAD_SYNC, 2),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 28 << 12,
                word(SET_TEXTURE_IMAGE, 2 << 19),
                tlut_address,
                word(SET_TILE, 2 << 19 | 7 << 9 | 296),
                7 << 24,
                word(LOAD_SYNC, 3),
                0,
                word(LOAD_TLUT, 0),
                7 << 24 | 29 << 14,
            ],
            vec![
                (ci8_address, ci8_address + 8),
                (ci4_address, ci4_address + 8),
                (unloaded_index_address, unloaded_index_address + 8),
                (tlut_address, tlut_address + 60),
            ],
        )
    }

    fn ci_and_hostile_tlut_word_words(high_texels: u16) -> (Vec<u32>, Vec<(u32, u32)>) {
        let index_address = 0x200;
        let high_address = 0x300;
        (
            vec![
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | 7),
                index_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 28 << 12,
                word(
                    SET_TEXTURE_IMAGE,
                    4 << 21 | 1 << 19 | u32::from(high_texels - 1),
                ),
                high_address,
                word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9 | 256),
                7 << 24,
                word(LOAD_SYNC, 1),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | u32::from((high_texels - 1) * 4) << 12,
            ],
            vec![
                (index_address, index_address + 8),
                (high_address, high_address + u32::from(high_texels)),
            ],
        )
    }

    fn ci_cell_with_hostile_second_tlut_words(high_texels: u16) -> (Vec<u32>, Vec<(u32, u32)>) {
        let index_address = 0x200;
        let canonical_address = 0x300;
        let hostile_address = 0x400;
        (
            vec![
                word(SET_TEXTURE_IMAGE, 2 << 21 | 1 << 19 | 1),
                index_address,
                word(SET_TILE, 2 << 21 | 1 << 19 | 1 << 9),
                7 << 24,
                word(LOAD_SYNC, 0),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | 4 << 12,
                word(SET_TEXTURE_IMAGE, 2 << 19),
                canonical_address,
                word(SET_TILE, 2 << 19 | 7 << 9 | 256),
                7 << 24,
                word(LOAD_SYNC, 1),
                0,
                word(LOAD_TLUT, 0),
                7 << 24,
                word(
                    SET_TEXTURE_IMAGE,
                    4 << 21 | 1 << 19 | u32::from(high_texels - 1),
                ),
                hostile_address,
                word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9 | 257),
                7 << 24,
                word(LOAD_SYNC, 2),
                0,
                word(LOAD_TILE, 0),
                7 << 24 | u32::from((high_texels - 1) * 4) << 12,
            ],
            vec![
                (index_address, index_address + 2),
                (canonical_address, canonical_address + 2),
                (hostile_address, hostile_address + u32::from(high_texels)),
            ],
        )
    }

    #[test]
    fn single_load_tile_packet_reduces_to_the_n_equals_one_case() {
        let (words, ranges) = single_tile_words(0x200);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 1);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 1);
        assert_eq!(state.generation(), 1);
    }

    #[test]
    fn single_load_block_packet_dispatches_to_the_block_executor() {
        let (words, ranges) = single_block_words(0x200);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 1);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 1);
        assert_eq!(state.generation(), 1);
    }

    #[test]
    fn committed_reader_decodes_all_seven_direct_pairs_with_explicit_row_parity() {
        let (words, ranges) = direct_reader_words();
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);

        // Advance the durable snapshot and overwrite only RGBA32's low bank.
        // The resulting texel deliberately combines generation-2 RG with
        // still-valid generation-1 BA; the reader binds the returned color
        // to snapshot generation 2 rather than requiring uniform touch ages.
        let mut decode_predecessor = RdpState::default();
        for _ in 0..3 {
            decode_predecessor.tmem_mut().load_sync().unwrap();
        }
        let later_address = 0x500;
        let later_words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            later_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9 | 16),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
        ];
        let later = fixture_with_state(
            later_words,
            &[(later_address, later_address + 8)],
            &decode_predecessor,
        );
        let pending =
            execute_ordered_tmem_loads(&state, later.decoded.submitted(), &later.decoded).unwrap();
        publish(&mut state, later, pending);
        assert_eq!(state.generation(), 2);
        assert_eq!(state.last_touched_generation(8 * 8), Some(1));
        assert_eq!(state.last_touched_generation(16 * 8), Some(2));
        assert_eq!(state.last_touched_generation(16 * 8 + 0x800), Some(1));

        let addressed = |column| AddressedTmemTexel::new(column, 0, TmemFirstRowParity::Even);
        let cases = [
            (ImageFormat::Rgba, PixelSize::Bits16, 0, 0x0102),
            (ImageFormat::IntensityAlpha, PixelSize::Bits4, 1, 0x1),
            (ImageFormat::IntensityAlpha, PixelSize::Bits8, 1, 0x02),
            (ImageFormat::IntensityAlpha, PixelSize::Bits16, 1, 0x0304),
            (ImageFormat::Intensity, PixelSize::Bits4, 1, 0x1),
            (ImageFormat::Intensity, PixelSize::Bits8, 4, 0x05),
        ];
        for (format, size, column, raw) in cases {
            let actual = read_committed_texel(
                &state,
                reader_tile(format, size, 1, 8, 0),
                addressed(column),
                TextureLutMode::Disabled,
            )
            .unwrap();
            let expected =
                decode_direct_texel(format, RawTexel::try_new(size, raw).unwrap()).unwrap();
            assert_eq!(actual.texel(), expected);
            assert_eq!(actual.snapshot().state(), state.identity());
            assert_eq!(actual.snapshot().generation(), 2);
        }

        let rgba32 = read_committed_texel(
            &state,
            reader_tile(ImageFormat::Rgba, PixelSize::Bits32, 1, 16, 0),
            addressed(0),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(
            rgba32.texel(),
            decode_direct_texel(
                ImageFormat::Rgba,
                RawTexel::try_new(PixelSize::Bits32, 0x0001_2223).unwrap(),
            )
            .unwrap()
        );

        let wrapped_odd_first_row = read_committed_texel(
            &state,
            reader_tile(ImageFormat::Intensity, PixelSize::Bits8, 1, 511, 0),
            AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Odd),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(wrapped_odd_first_row.texel().rgba8888(), [0x08; 4]);
        let wrapped_even_second_row = read_committed_texel(
            &state,
            reader_tile(ImageFormat::Intensity, PixelSize::Bits8, 1, 511, 0),
            AddressedTmemTexel::new(0, 1, TmemFirstRowParity::Odd),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(wrapped_even_second_row.texel().rgba8888(), [0x10; 4]);
    }

    #[test]
    fn committed_point_sampler_preserves_snapshot_and_caller_selected_row_parity() {
        let (words, ranges) = direct_reader_words();
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);

        let tile = reader_tile(ImageFormat::Intensity, PixelSize::Bits8, 1, 511, 0);
        let size = reader_size(0, 0, 28, 4);
        let coordinates = PointSampleCoordinates::new(
            TextureCoordinateS10_5::from_raw(0),
            TextureCoordinateS10_5::from_raw(0),
        );
        let before = observe_durable(&state);

        let even = sample_committed_point(
            &state,
            tile,
            size,
            PointSampleRequest::new(coordinates, TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap();
        let odd = sample_committed_point(
            &state,
            tile,
            size,
            PointSampleRequest::new(coordinates, TmemFirstRowParity::Odd),
            TextureLutMode::Disabled,
        )
        .unwrap();

        assert_eq!(even.texel().rgba8888(), [0x0c; 4]);
        assert_eq!(odd.texel().rgba8888(), [0x08; 4]);
        assert_eq!(even.snapshot(), odd.snapshot());
        assert_eq!(even.snapshot().state(), state.identity());
        assert_eq!(even.snapshot().generation(), before.generation);

        let even_cell = gather_committed_texture_cell(
            &state,
            tile,
            size,
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap();
        let odd_cell = gather_committed_texture_cell(
            &state,
            tile,
            size,
            cell_request(TmemFirstRowParity::Odd),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(
            cell_colors(even_cell),
            [[0x0c; 4], [0x14; 4], [0x0d; 4], [0x15; 4]]
        );
        assert_eq!(
            cell_colors(odd_cell),
            [[0x08; 4], [0x10; 4], [0x09; 4], [0x11; 4]]
        );
        for corner in [
            TextureCellCorner::UpperLeft,
            TextureCellCorner::LowerLeft,
            TextureCellCorner::UpperRight,
            TextureCellCorner::LowerRight,
        ] {
            assert_eq!(
                even_cell.addressed().corner(corner).first_row_parity(),
                TmemFirstRowParity::Even
            );
            assert_eq!(
                odd_cell.addressed().corner(corner).first_row_parity(),
                TmemFirstRowParity::Odd
            );
        }
        assert_eq!(observe_durable(&state), before);
    }

    #[test]
    fn committed_point_sampler_decodes_every_direct_format_with_exact_colors() {
        let (words, ranges) = direct_reader_words();
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);

        let coordinates = |column: u16| {
            PointSampleRequest::new(
                PointSampleCoordinates::new(
                    TextureCoordinateS10_5::from_raw((column * 32) as i16),
                    TextureCoordinateS10_5::from_raw(0),
                ),
                TmemFirstRowParity::Even,
            )
        };
        let cases = [
            (ImageFormat::Rgba, PixelSize::Bits16, 0, [0, 33, 8, 0]),
            (
                ImageFormat::IntensityAlpha,
                PixelSize::Bits4,
                1,
                [0, 0, 0, 255],
            ),
            (
                ImageFormat::IntensityAlpha,
                PixelSize::Bits8,
                1,
                [0, 0, 0, 34],
            ),
            (
                ImageFormat::IntensityAlpha,
                PixelSize::Bits16,
                1,
                [3, 3, 3, 4],
            ),
            (
                ImageFormat::Intensity,
                PixelSize::Bits4,
                1,
                [17, 17, 17, 17],
            ),
            (ImageFormat::Intensity, PixelSize::Bits8, 4, [5, 5, 5, 5]),
        ];
        let before = observe_durable(&state);
        for (format, size, column, expected) in cases {
            let actual = sample_committed_point(
                &state,
                reader_tile(format, size, 1, 8, 0),
                reader_size(0, 0, 28, 0),
                coordinates(column),
                TextureLutMode::Disabled,
            )
            .unwrap();
            assert_eq!(actual.texel().rgba8888(), expected);
            assert_eq!(actual.snapshot().state(), state.identity());
            assert_eq!(actual.snapshot().generation(), before.generation);
        }

        let rgba32 = sample_committed_point(
            &state,
            reader_tile(ImageFormat::Rgba, PixelSize::Bits32, 1, 16, 0),
            reader_size(0, 0, 4, 0),
            coordinates(0),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(rgba32.texel().rgba8888(), [32, 33, 34, 35]);
        assert_eq!(rgba32.snapshot().state(), state.identity());
        assert_eq!(rgba32.snapshot().generation(), before.generation);
        assert_eq!(observe_durable(&state), before);
    }

    #[test]
    fn committed_texture_cell_gathers_literal_rgba16_and_rgba32_corners() {
        let rgba16_source = [0xf8, 0x01, 0x07, 0xc1, 0x00, 0x3f, 0xff, 0xff];
        let (words, ranges) = rgba_cell_words(0x200, PixelSize::Bits16, 32);
        let rgba16 = publish_sources(words, &ranges, &[(0x200, &rgba16_source)]);
        let before = observe_durable(&rgba16);
        let cell = gather_committed_texture_cell(
            &rgba16,
            reader_tile(ImageFormat::Rgba, PixelSize::Bits16, 1, 32, 0),
            reader_size(0, 0, 4, 4),
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(
            cell_colors(cell),
            [
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                [255, 255, 255, 255],
            ]
        );
        for texel in cell.texels() {
            assert_eq!(texel.snapshot().state(), rgba16.identity());
            assert_eq!(texel.snapshot().generation(), before.generation);
        }
        assert_eq!(observe_durable(&rgba16), before);

        let rgba32_source = [
            0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0,
            0xf0, 0xff,
        ];
        let (words, ranges) = rgba_cell_words(0x300, PixelSize::Bits32, 64);
        let rgba32 = publish_sources(words, &ranges, &[(0x300, &rgba32_source)]);
        let before = observe_durable(&rgba32);
        let cell = gather_committed_texture_cell(
            &rgba32,
            reader_tile(ImageFormat::Rgba, PixelSize::Bits32, 1, 64, 0),
            reader_size(0, 0, 4, 4),
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(
            cell_colors(cell),
            [
                [0x10, 0x20, 0x30, 0x40],
                [0x90, 0xa0, 0xb0, 0xc0],
                [0x50, 0x60, 0x70, 0x80],
                [0xd0, 0xe0, 0xf0, 0xff],
            ]
        );
        for texel in cell.texels() {
            assert_eq!(texel.snapshot().state(), rgba32.identity());
            assert_eq!(texel.snapshot().generation(), before.generation);
        }
        assert_eq!(observe_durable(&rgba32), before);
    }

    #[test]
    fn three_nearest_filter_reads_the_sf_plus_tf_equals_scale_boundary_from_real_tmem_bytes() {
        // Reuses `centered_cell_exposes_literal_fractions_corners_and_parity`'s
        // exact addressing (`cell_request` = raw S10.5 16, `reader_size(0, 0,
        // 4, 4)`, zero shift/mask) to produce `sf=16, tf=16`: the formula's
        // `<=` boundary, which the `<=` puts in the lower-left branch.
        let rgba16_source = [0xf8, 0x01, 0x07, 0xc1, 0x00, 0x3f, 0xff, 0xff];
        let (words, ranges) = rgba_cell_words(0x200, PixelSize::Bits16, 32);
        let rgba16 = publish_sources(words, &ranges, &[(0x200, &rgba16_source)]);
        let cell = gather_committed_texture_cell(
            &rgba16,
            reader_tile(ImageFormat::Rgba, PixelSize::Bits16, 1, 32, 0),
            reader_size(0, 0, 4, 4),
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Disabled,
        )
        .unwrap();
        assert_eq!(cell.addressed().fractions().s_five_bit(), 16);
        assert_eq!(cell.addressed().fractions().t_five_bit(), 16);
        assert_eq!(
            cell_colors(cell),
            [
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                [255, 255, 255, 255],
            ]
        );

        // c00=UpperLeft=[255,0,0,255], c10=UpperRight=[0,255,0,255],
        // c01=LowerLeft=[0,0,255,255], sf=tf=16, sf+tf=32<=32 (lower-left
        // branch): R = round((255*32 + 16*(0-255) + 16*(0-255)) / 32)
        //            = round((8160 - 4080 - 4080) / 32) = round(0/32) = 0
        // G = round((0*32 + 16*(255-0) + 16*(0-0)) / 32) = round(4080/32) = 128 (rounds to nearest, .5 up)
        // B = round((0*32 + 16*(0-0) + 16*(255-0)) / 32) = round(4080/32) = 128 (mirror of G)
        // A stays 255 (all four corners agree).
        assert_eq!(
            filter_three_nearest_committed_cell(cell),
            [0, 128, 128, 255]
        );
    }

    #[test]
    fn committed_texture_cell_resolves_each_ci_corner_through_its_tlut_entry() {
        let ci4_indices = [0x12, 0x34];
        let rgba16_entries = [0xf8, 0x01, 0x07, 0xc1, 0x00, 0x3f, 0xff, 0xff];
        let (words, ranges) = ci_cell_tlut_words(0x200, 1, 8, 0x300);
        let ci4 = publish_sources(
            words,
            &ranges,
            &[(0x200, &ci4_indices), (0x300, &rgba16_entries)],
        );
        let before = observe_durable(&ci4);
        let cell = gather_committed_texture_cell(
            &ci4,
            reader_tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1, 8, 0),
            reader_size(0, 0, 4, 4),
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Rgba16,
        )
        .unwrap();
        assert_eq!(
            cell_colors(cell),
            [
                [255, 0, 0, 255],
                [0, 0, 255, 255],
                [0, 255, 0, 255],
                [255, 255, 255, 255],
            ]
        );
        assert!(cell
            .texels()
            .iter()
            .all(|texel| texel.snapshot() == cell.texels()[0].snapshot()));
        assert_eq!(observe_durable(&ci4), before);

        let ci8_indices = [0x01, 0x02, 0x03, 0x04];
        let ia16_entries = [0x10, 0x01, 0x20, 0x40, 0x80, 0xff, 0xff, 0x00];
        let (words, ranges) = ci_cell_tlut_words(0x400, 2, 16, 0x500);
        let ci8 = publish_sources(
            words,
            &ranges,
            &[(0x400, &ci8_indices), (0x500, &ia16_entries)],
        );
        let before = observe_durable(&ci8);
        let cell = gather_committed_texture_cell(
            &ci8,
            reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 16, 0),
            reader_size(0, 0, 4, 4),
            cell_request(TmemFirstRowParity::Even),
            TextureLutMode::Ia16,
        )
        .unwrap();
        assert_eq!(
            cell_colors(cell),
            [
                [16, 16, 16, 1],
                [128, 128, 128, 255],
                [32, 32, 32, 64],
                [255, 255, 255, 0],
            ]
        );
        assert!(cell
            .texels()
            .iter()
            .all(|texel| texel.snapshot() == cell.texels()[0].snapshot()));
        assert_eq!(observe_durable(&ci8), before);
    }

    #[test]
    fn committed_texture_cell_reports_the_first_invalid_semantic_corner() {
        let image_address = 0x200;
        let source = [0x11, 0x22];
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19),
            image_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 1 << 9),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 4,
        ];
        let ranges = [(image_address, image_address + 2)];
        let state = publish_sources(words, &ranges, &[(image_address, &source)]);
        let before = observe_durable(&state);
        assert_eq!(
            gather_committed_texture_cell(
                &state,
                reader_tile(ImageFormat::Intensity, PixelSize::Bits8, 1, 0, 0),
                reader_size(0, 0, 4, 4),
                cell_request(TmemFirstRowParity::Even),
                TextureLutMode::Disabled,
            ),
            Err(TextureCellSampleError::Read {
                corner: TextureCellCorner::UpperRight,
                source: PhysicalTexelReadError::InvalidTexelByte { address: 0x001 },
            })
        );
        assert_eq!(observe_durable(&state), before);
    }

    #[test]
    fn committed_reader_resolves_absolute_offset_tlut_for_ci4_and_ci8() {
        // Programming Manual section 13.8's partial-CI8 pattern: palette
        // indices 40..=69 occupy absolute TMEM words 256+40 through 256+69.
        // Nothing is loaded into the lower TLUT words.
        let (words, ranges) = ci_and_offset_tlut_reader_words();
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);

        let ci4 = reader_tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1, 1, 2);
        let ci8 = reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 0, 15);
        let unloaded_ci8 = reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 2, 0);
        let even = AddressedTmemTexel::new(0, 0, TmemFirstRowParity::Even);
        let odd = AddressedTmemTexel::new(1, 0, TmemFirstRowParity::Even);

        assert_eq!(
            read_committed_texel(&state, ci4, even, TextureLutMode::Disabled)
                .unwrap()
                .texel()
                .rgba8888(),
            [0x28; 4]
        );
        assert_eq!(
            read_committed_texel(&state, ci4, odd, TextureLutMode::Disabled)
                .unwrap()
                .texel()
                .rgba8888(),
            [0x29; 4]
        );
        assert_eq!(
            read_committed_texel(&state, ci8, even, TextureLutMode::Disabled)
                .unwrap()
                .texel()
                .rgba8888(),
            [0x28; 4],
            "CI8 must ignore the tile palette"
        );

        for (tile, addressed, raw_entry) in
            [(ci4, even, 0x0001), (ci4, odd, 0x0203), (ci8, even, 0x0001)]
        {
            for (mode, entry_format) in [
                (TextureLutMode::Rgba16, ImageFormat::Rgba),
                (TextureLutMode::Ia16, ImageFormat::IntensityAlpha),
            ] {
                let actual = read_committed_texel(&state, tile, addressed, mode).unwrap();
                let expected = decode_direct_texel(
                    entry_format,
                    RawTexel::try_new(PixelSize::Bits16, raw_entry).unwrap(),
                )
                .unwrap();
                assert_eq!(actual.texel(), expected);
                assert_eq!(actual.snapshot().state(), state.identity());
                assert_eq!(actual.snapshot().generation(), 1);
            }
        }

        assert_eq!(
            read_committed_texel(&state, unloaded_ci8, even, TextureLutMode::Rgba16),
            Err(PhysicalTexelReadError::IncompleteTlutEntry {
                byte_address: 0x800,
                valid_mask: 0,
            }),
            "an offset partial palette must not be rebased onto lower indices"
        );
    }

    #[test]
    fn committed_point_sampler_resolves_ci4_rgba16_and_ci8_ia16_exactly() {
        let (words, ranges) = ci_and_offset_tlut_reader_words();
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut state = state;
        publish(&mut state, fixture, pending);
        let before = observe_durable(&state);
        let request = PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(0),
                TextureCoordinateS10_5::from_raw(0),
            ),
            TmemFirstRowParity::Even,
        );

        let ci4 = sample_committed_point(
            &state,
            reader_tile(ImageFormat::ColorIndex, PixelSize::Bits4, 1, 1, 2),
            reader_size(0, 0, 4, 0),
            request,
            TextureLutMode::Rgba16,
        )
        .unwrap();
        assert_eq!(ci4.texel().rgba8888(), [0, 0, 0, 255]);
        assert_eq!(ci4.snapshot().state(), state.identity());
        assert_eq!(ci4.snapshot().generation(), before.generation);

        let ci8 = sample_committed_point(
            &state,
            reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 0, 15),
            reader_size(0, 0, 4, 0),
            request,
            TextureLutMode::Ia16,
        )
        .unwrap();
        assert_eq!(ci8.texel().rgba8888(), [0, 0, 0, 1]);
        assert_eq!(ci8.snapshot(), ci4.snapshot());
        assert_eq!(observe_durable(&state), before);
    }

    #[test]
    fn committed_reader_rejects_partial_and_unequal_tlut_words() {
        let ci8 = reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 0, 0);

        let (words, ranges) = ci_and_hostile_tlut_word_words(2);
        let partial_fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending = execute_ordered_tmem_loads(
            &state,
            partial_fixture.decoded.submitted(),
            &partial_fixture.decoded,
        )
        .unwrap();
        let mut partial = state;
        publish(&mut partial, partial_fixture, pending);
        let request = PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(0),
                TextureCoordinateS10_5::from_raw(0),
            ),
            TmemFirstRowParity::Even,
        );
        let size = reader_size(0, 0, 0, 0);
        let partial_before = observe_durable(&partial);
        assert_eq!(
            sample_committed_point(&partial, ci8, size, request, TextureLutMode::Rgba16,),
            Err(crate::PointSampleError::Read(
                PhysicalTexelReadError::IncompleteTlutEntry {
                    byte_address: 0x800,
                    valid_mask: 0x03,
                }
            ))
        );
        assert_eq!(observe_durable(&partial), partial_before);

        let (words, ranges) = ci_and_hostile_tlut_word_words(8);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        let mut unequal = state;
        publish(&mut unequal, fixture, pending);
        let unequal_before = observe_durable(&unequal);
        assert_eq!(
            sample_committed_point(&unequal, ci8, size, request, TextureLutMode::Ia16),
            Err(crate::PointSampleError::Read(
                PhysicalTexelReadError::NonCanonicalTlutEntry {
                    byte_address: 0x800,
                    lanes: [0x0001, 0x0203, 0x0405, 0x0607],
                }
            ))
        );
        assert_eq!(observe_durable(&unequal), unequal_before);
    }

    #[test]
    fn committed_texture_cell_locates_nonfirst_partial_and_unequal_tlut_errors() {
        let indices = [0x00, 0x01];
        let canonical = [0xf8, 0x01];
        let request = PointSampleRequest::new(
            PointSampleCoordinates::new(
                TextureCoordinateS10_5::from_raw(16),
                TextureCoordinateS10_5::from_raw(0),
            ),
            TmemFirstRowParity::Even,
        );
        let tile = reader_tile(ImageFormat::ColorIndex, PixelSize::Bits8, 1, 0, 0);
        let size = reader_size(0, 0, 4, 0);

        let hostile_partial = [0xaa, 0xbb];
        let (words, ranges) = ci_cell_with_hostile_second_tlut_words(2);
        let partial = publish_sources(
            words,
            &ranges,
            &[
                (0x200, &indices),
                (0x300, &canonical),
                (0x400, &hostile_partial),
            ],
        );
        let before = observe_durable(&partial);
        assert_eq!(
            gather_committed_texture_cell(&partial, tile, size, request, TextureLutMode::Rgba16,),
            Err(TextureCellSampleError::Read {
                corner: TextureCellCorner::UpperRight,
                source: PhysicalTexelReadError::IncompleteTlutEntry {
                    byte_address: 0x808,
                    valid_mask: 0x03,
                },
            })
        );
        assert_eq!(observe_durable(&partial), before);

        let hostile_unequal = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let (words, ranges) = ci_cell_with_hostile_second_tlut_words(8);
        let unequal = publish_sources(
            words,
            &ranges,
            &[
                (0x200, &indices),
                (0x300, &canonical),
                (0x400, &hostile_unequal),
            ],
        );
        let before = observe_durable(&unequal);
        assert_eq!(
            gather_committed_texture_cell(&unequal, tile, size, request, TextureLutMode::Ia16),
            Err(TextureCellSampleError::Read {
                corner: TextureCellCorner::UpperRight,
                source: PhysicalTexelReadError::NonCanonicalTlutEntry {
                    byte_address: 0x808,
                    lanes: [0x0001, 0x0203, 0x0405, 0x0607],
                },
            })
        );
        assert_eq!(observe_durable(&unequal), before);
    }

    #[test]
    fn two_load_tile_packet_chains_same_kind_loads_in_order() {
        let (words, ranges) = two_tile_words(0x200, 0x300);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 2);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 2);
    }

    #[test]
    fn mixed_load_tile_and_load_block_packet_dispatches_by_command_kind() {
        let (words, ranges) = mixed_tile_then_block_words(0x200, 0x400);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 2);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 2);
    }

    // TLUT fixture words mirror `raw_dpc::tests`' own known-good
    // `four_bit_yuv_and_tlut_destination_claims_stay_loudly_out_of_scope` TLUT
    // case, updated for M4.3.1's now-strict LoadTLUT admission gate: a
    // 16-bit `SetTextureImage` source (format 0, size 2), a 16-bit
    // destination tile descriptor with `tmem >= 256` (line 7, tmem 256), and
    // a 15-entry TLUT count -- adapted to this module's `word(opcode,
    // payload)` shape (no stream prefix) and this module's tile index 7 for
    // consistency with the other fixtures here.
    fn tlut_words(image_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 2 << 19),
            image_address,
            word(SET_TILE, 2 << 19 | 7 << 9 | 256),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TLUT, 0),
            7 << 24 | 15 << 14,
        ];
        (words, vec![(image_address, image_address + 0x20)])
    }

    #[test]
    fn a_load_tlut_command_alone_dispatches_to_the_tlut_executor() {
        // Since M4.3.2, a `LoadTlut` command is executed like any other
        // executable load: it stages, dispatches to `execute_tlut`, and
        // seals into a completed transaction.
        let (words, ranges) = tlut_words(0x300);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 1);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 1);
        assert_eq!(state.generation(), 1);
        // Every byte of the high-bank destination word (tmem 256, byte
        // address 0x800) must be valid and quadricated -- 2 real source
        // bytes repeated across all four 16-bit lanes.
        let base = 256 * 8;
        for lane in 0..8_u16 {
            assert!(state.byte_is_valid(base + lane));
        }
        let hi = state.valid_byte(base).unwrap();
        let lo = state.valid_byte(base + 1).unwrap();
        for lane in (0..8_u16).step_by(2) {
            assert_eq!(state.valid_byte(base + lane), Some(hi));
            assert_eq!(state.valid_byte(base + lane + 1), Some(lo));
        }
    }

    #[test]
    fn a_genuine_tile_tlut_block_packet_executes_all_three_in_decode_order() {
        // Genuinely Tile -> TLUT -> Block: proves the packet-level outer loop
        // dispatches all three kinds, in decode order, into one chained
        // transaction -- not just TLUT alone.
        let (words, ranges) = tile_tlut_block_words(0x200, 0x300, 0x400);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 3);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 3);
        assert_eq!(state.generation(), 1);
    }

    // A single-word LoadTile targeting the SAME high-bank destination word
    // (tmem 256) that `tlut_words` above also targets, so a Tile-then-TLUT
    // packet exercises a genuine cross-kind overlap on one destination word.
    fn tile_words_at_high_bank(image_address: u32) -> (Vec<u32>, Vec<(u32, u32)>) {
        let words = vec![
            word(SET_TEXTURE_IMAGE, 4 << 21 | 1 << 19 | 7),
            image_address,
            word(SET_TILE, 4 << 21 | 1 << 19 | 7 << 9 | 256),
            7 << 24,
            word(LOAD_SYNC, 0),
            0,
            word(LOAD_TILE, 0),
            7 << 24 | 28 << 12,
        ];
        (words, vec![(image_address, image_address + 8)])
    }

    fn tile_then_tlut_same_word_words(
        tile_address: u32,
        tlut_address: u32,
    ) -> (Vec<u32>, Vec<(u32, u32)>) {
        let (mut words, mut ranges) = tile_words_at_high_bank(tile_address);
        let (tlut_words, tlut_ranges) = tlut_words(tlut_address);
        words.extend(tlut_words);
        ranges.extend(tlut_ranges);
        (words, ranges)
    }

    #[test]
    fn a_tlut_load_overwrites_an_earlier_tile_loads_same_destination_word_byte_for_byte() {
        // Cross-kind overlap: LoadTile writes tmem word 256 first (8 real
        // source bytes, one-for-one), then a LoadTLUT in the same packet
        // targets the SAME word (tmem_base 256). Last-writer-wins must hold
        // across kinds, not just within one kind (`overlapping_loads_publish_
        // only_the_last_loads_defined_lanes` in `load_tlut.rs` already covers
        // same-kind TLUT/TLUT overlap) -- every one of the 8 destination
        // bytes must be the TLUT's quadricated `[hi, lo]` pair, not any of
        // the Tile load's literal bytes.
        // Distinct low bytes so the Tile load's literal content and the
        // TLUT's quadricated content can never coincidentally match at any
        // lane -- the byte-for-byte disproof below stays meaningful.
        let tile_address = 0x210_u32;
        let tlut_address = 0x340_u32;
        let (words, ranges) = tile_then_tlut_same_word_words(tile_address, tlut_address);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded)
                .unwrap();
        assert_eq!(pending.completed_loads(), 2);
        let mut state = state;
        let committed = publish(&mut state, fixture, pending);
        assert_eq!(committed.completed_loads(), 2);
        assert_eq!(state.generation(), 1);

        let base = 256 * 8;
        // The fixture's capture fills every source byte with `address as
        // u8`; the TLUT source is 2 bytes at `tlut_address`.
        let hi = tlut_address as u8;
        let lo = (tlut_address + 1) as u8;
        let expected: [u8; 8] = [hi, lo, hi, lo, hi, lo, hi, lo];
        for (lane, expected_byte) in expected.iter().copied().enumerate() {
            let lane = lane as u16;
            assert!(state.byte_is_valid(base + lane));
            assert_eq!(state.valid_byte(base + lane), Some(expected_byte));
            // None of the Tile load's own literal bytes (`tile_address +
            // lane`) may remain -- the exact literal-byte disproof the
            // review asked for.
            assert_ne!(
                state.valid_byte(base + lane),
                Some((tile_address as u16 + lane) as u8),
                "byte {lane} still shows the overwritten Tile load's literal content"
            );
            assert_eq!(state.last_touched_generation(base + lane), Some(1));
        }
    }

    #[test]
    fn a_tlut_load_before_a_later_yuv_deferred_block_rejects_the_whole_packet_unstaged() {
        // TLUT is now executable, so this specifically proves the pre-stage
        // validation pass still catches a LATER YUV-deferred command even
        // when an earlier TLUT command in the same packet is itself
        // perfectly valid and executable: if the loop staged the TLUT load
        // while walking (regressing to pre-validation-pass behavior) before
        // reaching the YUV-deferred load, durable state would already
        // reflect a staged transaction by the time this call returns.
        let (mut words, mut ranges) = tlut_words(0x300);
        let (yuv_words, yuv_ranges) = yuv_block_words(0x500);
        words.extend(yuv_words);
        ranges.extend(yuv_ranges);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::DeferredLoadKind {
                error: TmemLoadSourcePlanError::YuvExecutionDeferred,
                ..
            })
        ));
        // No load staged means the durable state must be untouched, even
        // though the TLUT load earlier in the packet was itself valid.
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn a_yuv_deferred_load_alone_is_rejected_loudly_before_any_load_stages() {
        // YUV-deferred contracts journal only a `TmemLoadSource` access, no
        // `TmemLoadDestination` claim -- this packet decodes cleanly on the
        // first attempt, so it uses `single_pass_fixture` rather than the
        // two-pass planning-probe `fixture` the Tile/Block/TLUT fixtures need.
        let (words, ranges) = yuv_block_words(0x300);
        let fixture = single_pass_fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::DeferredLoadKind {
                error: TmemLoadSourcePlanError::YuvExecutionDeferred,
                ..
            })
        ));
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn a_valid_tile_before_a_yuv_deferred_block_rejects_the_whole_packet_unstaged() {
        // Point 1's exact regression shape: an earlier command in the packet
        // is a genuinely valid, executable LoadTile. If the outer loop
        // staged it while walking (as it did before the pre-stage
        // validation pass existed) before reaching the later YUV-deferred
        // LoadBlock, durable state would already have observed a staged
        // transaction by the time this call returns its error.
        let (words, ranges) = tile_then_yuv_block_words(0x200, 0x300);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::DeferredLoadKind {
                error: TmemLoadSourcePlanError::YuvExecutionDeferred,
                ..
            })
        ));
        // No load staged means the durable state must be untouched, even
        // though the first (Tile) load in the packet was itself valid.
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn a_packet_with_no_executable_load_is_rejected() {
        // No TmemLoadSource/Destination accesses are declared, so this packet
        // decodes cleanly with an empty journal on the first attempt; it does
        // not need the two-pass planning-probe round trip the other fixtures
        // use to discover their exact expected accesses.
        let words = vec![word(LOAD_SYNC, 0), 0];
        let layout = PhysicalMemoryLayout::try_new(LAYOUT_BYTES).unwrap();
        let byte_count = u32::try_from(words.len() * 4).unwrap();
        let accesses = vec![command_access(layout, byte_count, 0)];
        let packet = finalize_packet(&words, accesses);
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        let fixture = Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
            backend,
            guest,
        };
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::NoExecutableLoads)
        ));
    }

    #[test]
    fn wrong_backend_effect_report_is_rejected_at_bind_gpu() {
        let (words, ranges) = single_tile_words(0x200);
        let primary = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let pending =
            execute_ordered_tmem_loads(&state, primary.decoded.submitted(), &primary.decoded)
                .unwrap();

        // `other` must produce genuinely different captured source bytes from
        // `primary` so their proposed TMEM content actually differs (the
        // fixture's guest-read capture truncates address to `u8`, so e.g.
        // 0x200 and 0x500 alias to the same low byte — 0x211 does not alias
        // 0x200's low byte for any of its 8 read bytes).
        let (other_words, other_ranges) = single_tile_words(0x211);
        let other = fixture(other_words, &other_ranges);
        let other_pending =
            execute_ordered_tmem_loads(&state, other.decoded.submitted(), &other.decoded).unwrap();

        let Fixture {
            decoded,
            mut backend,
            ..
        } = primary;
        let wrong_report = BackendEffectReport::try_new(
            other.decoded.submitted().packet(),
            other_pending.proposed_effects().to_vec(),
        )
        .unwrap();
        // Wrong queue: `backend` belongs to `primary`'s queue, not `other`'s.
        assert!(backend
            .issue(other.decoded.submitted(), wrong_report)
            .is_err());

        // Same access shape, different content: `try_new` accepts it (it only
        // checks access-for-access coverage), but `bind_gpu` must reject the
        // mismatched proposal identity/content at completion time.
        let mismatched_report = BackendEffectReport::try_new(
            decoded.submitted().packet(),
            other_pending.proposed_effects().to_vec(),
        )
        .unwrap();
        let mismatched_receipt = backend
            .issue(decoded.submitted(), mismatched_report)
            .unwrap();
        let submitted = decoded.into_contract_parts().submitted;
        let complete = submitted.gpu_complete(mismatched_receipt).unwrap();
        assert!(pending.bind_gpu(&complete).is_err());
    }

    #[test]
    fn cross_packet_guest_ticket_is_rejected_at_publish() {
        let (words_a, ranges_a) = single_tile_words(0x200);
        let fixture_a = fixture(words_a, &ranges_a);
        let (words_b, ranges_b) = single_tile_words(0x300);
        let fixture_b = fixture(words_b, &ranges_b);

        let mut state = PhysicalTmemState::try_new().unwrap();
        let pending_a =
            execute_ordered_tmem_loads(&state, fixture_a.decoded.submitted(), &fixture_a.decoded)
                .unwrap();

        let Fixture {
            decoded: decoded_a,
            mut backend,
            ..
        } = fixture_a;
        let report_a = BackendEffectReport::try_new(
            decoded_a.submitted().packet(),
            pending_a.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt_a = backend.issue(decoded_a.submitted(), report_a).unwrap();
        let submitted_a = decoded_a.into_contract_parts().submitted;
        let complete_a = submitted_a.gpu_complete(receipt_a).unwrap();
        let gpu_bound_a = pending_a.bind_gpu(&complete_a).unwrap();

        // Build B's full guest ticket independently, then try to publish A's
        // GPU-bound transaction against B's guest commit.
        let pending_b =
            execute_ordered_tmem_loads(&state, fixture_b.decoded.submitted(), &fixture_b.decoded)
                .unwrap();
        let guest_b = {
            let Fixture {
                decoded: decoded_b,
                mut backend,
                mut guest,
            } = fixture_b;
            let report_b = BackendEffectReport::try_new(
                decoded_b.submitted().packet(),
                pending_b.proposed_effects().to_vec(),
            )
            .unwrap();
            let receipt_b = backend.issue(decoded_b.submitted(), report_b).unwrap();
            let submitted_b = decoded_b.into_contract_parts().submitted;
            let complete_b = submitted_b.gpu_complete(receipt_b).unwrap();
            let _gpu_bound_b = pending_b.bind_gpu(&complete_b).unwrap();
            let guest_report_b = GuestCommitEffectReport::try_new(&complete_b, Vec::new()).unwrap();
            let guest_receipt_b = guest.issue(&complete_b, guest_report_b).unwrap();
            complete_b.commit_guest(guest_receipt_b).unwrap()
        };

        let result = state.publication_authority().publish(gpu_bound_a, guest_b);
        assert!(result.is_err());
    }

    // Interleaving closed: A and B independently stage a `PendingTmemTransaction`
    // against the same `PhysicalTmemState` generation via the packet-level
    // outer loop. If B publishes first (advancing to generation N+1), A's
    // later publish attempt against stale generation N must fail rather than
    // overwrite B's committed state.
    #[test]
    fn stale_generation_a_b_interleaving_rejects_the_second_publish() {
        let (words_a, ranges_a) = single_tile_words(0x200);
        let fixture_a = fixture(words_a, &ranges_a);
        let (words_b, ranges_b) = single_tile_words(0x300);
        let fixture_b = fixture(words_b, &ranges_b);

        let mut state = PhysicalTmemState::try_new().unwrap();
        let pending_a =
            execute_ordered_tmem_loads(&state, fixture_a.decoded.submitted(), &fixture_a.decoded)
                .unwrap();
        let pending_b =
            execute_ordered_tmem_loads(&state, fixture_b.decoded.submitted(), &fixture_b.decoded)
                .unwrap();

        let Fixture {
            decoded: decoded_a,
            backend: mut backend_a,
            guest: mut guest_a,
        } = fixture_a;
        let report_a = BackendEffectReport::try_new(
            decoded_a.submitted().packet(),
            pending_a.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt_a = backend_a.issue(decoded_a.submitted(), report_a).unwrap();
        let submitted_a = decoded_a.into_contract_parts().submitted;
        let complete_a = submitted_a.gpu_complete(receipt_a).unwrap();
        let gpu_bound_a = pending_a.bind_gpu(&complete_a).unwrap();
        let guest_report_a = GuestCommitEffectReport::try_new(&complete_a, Vec::new()).unwrap();
        let guest_receipt_a = guest_a.issue(&complete_a, guest_report_a).unwrap();
        let guest_a = complete_a.commit_guest(guest_receipt_a).unwrap();

        let Fixture {
            decoded: decoded_b,
            backend: mut backend_b,
            guest: mut guest_b,
        } = fixture_b;
        let report_b = BackendEffectReport::try_new(
            decoded_b.submitted().packet(),
            pending_b.proposed_effects().to_vec(),
        )
        .unwrap();
        let receipt_b = backend_b.issue(decoded_b.submitted(), report_b).unwrap();
        let submitted_b = decoded_b.into_contract_parts().submitted;
        let complete_b = submitted_b.gpu_complete(receipt_b).unwrap();
        let gpu_bound_b = pending_b.bind_gpu(&complete_b).unwrap();
        let guest_report_b = GuestCommitEffectReport::try_new(&complete_b, Vec::new()).unwrap();
        let guest_receipt_b = guest_b.issue(&complete_b, guest_report_b).unwrap();
        let guest_b = complete_b.commit_guest(guest_receipt_b).unwrap();

        // B publishes first: generation 0 -> 1.
        state
            .publication_authority()
            .publish(gpu_bound_b, guest_b)
            .unwrap();
        assert_eq!(state.generation(), 1);

        // A's candidate was staged against base generation 0, which is now
        // stale; it must fail rather than silently overwrite B.
        let result = state.publication_authority().publish(gpu_bound_a, guest_a);
        assert!(matches!(
            result,
            Err(PhysicalTmemError::StaleBaseGeneration {
                expected: 0,
                actual: 1,
            })
        ));
    }

    #[test]
    fn destination_coverage_mismatch_fires_if_the_loop_skipped_a_declared_load() {
        // `decode_raw_dpc` itself guarantees an exact 1:1 correspondence
        // between a packet's declared `TmemLoadDestination` journal accesses
        // and its decoded `LoadTile`/`LoadBlock` commands (confirmed by
        // reading its `JournalMismatch` self-check) — so a two-load packet
        // that survives decode can never actually be short a destination.
        // `into_pending`'s `DestinationCoverageMismatch` is defense-in-depth
        // against this executor's OWN loop skipping a declared load, not a
        // reachable decode-time fixture. This test proves that defense fires
        // by driving only the FIRST of two decoded loads through the
        // M4.2a state machine directly (bypassing `execute_ordered_tmem_loads`
        // entirely), mirroring exactly what a buggy loop that stopped early
        // would produce.
        let (words, ranges) = two_tile_words(0x200, 0x300);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();

        let first_load = fixture
            .decoded
            .commands()
            .iter()
            .find_map(|command| match command.kind() {
                RawDpcCommandKind::LoadTile(load) => Some(load),
                _ => None,
            })
            .unwrap();
        let transfer = fixture
            .decoded
            .resource_plan()
            .bind_tmem_transfer(first_load)
            .unwrap();
        let staged = state
            .stage_transfer(fixture.decoded.submitted(), &transfer)
            .unwrap();
        let prepared = prepare_load_tile(fixture.decoded.submitted(), &transfer).unwrap();
        let packet = prepared.execute(staged).unwrap().into_packet();

        let result = packet.into_pending();
        assert!(matches!(
            result,
            Err(PhysicalTmemError::DestinationCoverageMismatch {
                expected: 2,
                actual: 1,
            })
        ));
    }

    #[test]
    fn overlapping_tile_then_block_leaves_the_last_writer_visible_in_decode_order() {
        // `single_tile_words` and `single_block_words` both target tile 7
        // with `SET_TILE` tmem = 0 (byte address 0) -- `mixed_tile_then_block_words`
        // chains a Tile load at that destination followed by a Block load at
        // the SAME destination, so their physical TMEM writes genuinely
        // overlap. This proves the outer loop's decode order is also the
        // last-writer order: the packet's final byte 0 must equal what an
        // INDEPENDENT, standalone Block-only load (run against its own fresh
        // state, never touched by the Tile load at all) would have written,
        // not the Tile load's byte.
        let (mixed_words, mixed_ranges) = mixed_tile_then_block_words(0x200, 0x400);
        let mixed_fixture = fixture(mixed_words, &mixed_ranges);
        let mut mixed_state = PhysicalTmemState::try_new().unwrap();
        let mixed_pending = execute_ordered_tmem_loads(
            &mixed_state,
            mixed_fixture.decoded.submitted(),
            &mixed_fixture.decoded,
        )
        .unwrap();
        let mixed_committed = publish(&mut mixed_state, mixed_fixture, mixed_pending);
        assert_eq!(mixed_committed.completed_loads(), 2);

        // Independent oracle: the SAME Block words/address as the mixed
        // packet's second load, executed alone against its own fresh state
        // that the Tile load never touched.
        let (block_words, block_ranges) = single_block_words(0x400);
        let block_fixture = fixture(block_words, &block_ranges);
        let mut block_state = PhysicalTmemState::try_new().unwrap();
        let block_pending = execute_ordered_tmem_loads(
            &block_state,
            block_fixture.decoded.submitted(),
            &block_fixture.decoded,
        )
        .unwrap();
        let block_committed = publish(&mut block_state, block_fixture, block_pending);
        assert_eq!(block_committed.completed_loads(), 1);

        // Both writes cover TMEM byte address 0 (tmem word 0). The mixed
        // packet's final byte there must match the Block-only oracle's byte
        // exactly -- proving the Block load (decoded second) is the visible
        // last writer, not the Tile load (decoded first).
        assert!(mixed_state.byte_is_valid(0));
        assert!(block_state.byte_is_valid(0));
        assert_eq!(mixed_state.valid_byte(0), block_state.valid_byte(0));
    }

    #[test]
    fn foreign_queue_submitted_ticket_is_rejected_before_any_load_stages() {
        // `submitted` and `decoded` must come from the same ticket. Here
        // `submitted` belongs to a completely different `TicketAuthoritySet`
        // (a foreign queue) than the one that produced `decoded`.
        let (words, ranges) = single_tile_words(0x200);
        let primary_fixture = fixture(words, &ranges);
        let (other_words, other_ranges) = single_tile_words(0x500);
        let other_fixture = fixture(other_words, &other_ranges);
        let state = PhysicalTmemState::try_new().unwrap();

        let result = execute_ordered_tmem_loads(
            &state,
            other_fixture.decoded.submitted(),
            &primary_fixture.decoded,
        );
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::SubmissionMismatch { .. })
        ));
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn reordered_same_queue_submission_is_rejected_before_any_load_stages() {
        // Two submissions from the SAME queue but different ordinals (as if
        // a caller reordered or duplicated a ticket from an earlier/later
        // submission in the same queue): `submitted`'s queue identity
        // matches, but its ordinal/submission identity and packet do not,
        // and this must still be rejected as a named identity mismatch
        // rather than silently proceeding on the wrong ticket.
        let (words_a, ranges_a) = single_tile_words(0x200);
        let (words_b, ranges_b) = single_tile_words(0x500);
        let packet_a = packet_from_words(words_a, &ranges_a);
        let packet_b = packet_from_words(words_b, &ranges_b);
        let (mut queue, _backend, _guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        // Same queue, ordinal 0 then ordinal 1.
        let submitted_a = queue.submit(DecodedTicket::new(packet_a)).unwrap();
        let submitted_b = queue.submit(DecodedTicket::new(packet_b)).unwrap();
        let decoded_a = decode_raw_dpc(submitted_a, &RdpState::default()).unwrap();

        let state = PhysicalTmemState::try_new().unwrap();
        // Same queue as `decoded_a`, but `submitted_b` names a different
        // ordinal/submission/packet -- a reordered ticket, not a foreign
        // queue.
        let result = execute_ordered_tmem_loads(&state, &submitted_b, &decoded_a);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::SubmissionMismatch { .. })
        ));
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn duplicate_submission_of_the_same_packet_is_rejected_if_ticket_identity_differs() {
        // Submitting the exact same packet content twice through the same
        // queue mints two DISTINCT `SubmittedTicket`s (different ordinals,
        // hence different `SubmissionIdentity` even though `WorkloadIdentity`
        // matches) -- a duplicate submission of identical bytes is still a
        // foreign ticket relative to the first `decoded`, and must be
        // rejected the same way.
        let (words, ranges) = single_tile_words(0x200);
        let packet_first = packet_from_words(words.clone(), &ranges);
        let packet_second = packet_from_words(words, &ranges);
        let (mut queue, _backend, _guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted_first = queue.submit(DecodedTicket::new(packet_first)).unwrap();
        let submitted_second = queue.submit(DecodedTicket::new(packet_second)).unwrap();
        assert_eq!(
            submitted_first.packet().identity(),
            submitted_second.packet().identity()
        );
        assert_ne!(submitted_first.identity(), submitted_second.identity());
        let decoded_first = decode_raw_dpc(submitted_first, &RdpState::default()).unwrap();

        let state = PhysicalTmemState::try_new().unwrap();
        let result = execute_ordered_tmem_loads(&state, &submitted_second, &decoded_first);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::SubmissionMismatch { .. })
        ));
        assert_eq!(state.generation(), 0);
    }
}
