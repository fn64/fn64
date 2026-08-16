//! Packet-level sequencing of every executable TMEM load in one decoded
//! raw-DPC command stream.
//!
//! M4.2b's LoadTile ([`super::load_tile`]) and M4.2c's LoadBlock
//! ([`super::load_block`]) executors each drive exactly one checked transfer
//! through the M4.2a physical-TMEM typestate engine. Neither, nor
//! [`crate::raw_dpc`], walks [`DecodedRawDpc::commands()`] and chains
//! executors across every load declared by one packet. This module is that
//! outer loop: it dispatches each ordered `LoadTile`/`LoadBlock` command to
//! its executor, chains the resulting [`PhysicalTmemPacketTransaction`]
//! across the whole packet, and seals it into one
//! [`PendingTmemTransaction`]. `LoadTlut` has a valid, bindable transfer plan
//! since M4.3.1 but no physical executor until M4.3.2, so it is refused
//! loudly as a scope boundary before any load in the packet is staged; any
//! YUV-deferred contract is refused the same way. Rejection is decided by a
//! dedicated validation pass over every ordered command, run to completion
//! before staging begins, so a later TLUT/YUV command can never be preceded
//! by an already-staged earlier Tile/Block load — neither is silently
//! omitted from the sealed transaction's destination coverage either way.
//! RT64 is not hardware authority for this module.

use core::fmt;

use fn64_render_ir::SubmittedTicket;

use crate::raw_dpc::{
    BoundTmemTransfer, DecodedRawDpc, RawDpcCommandKind, TmemLoadSourcePlanError,
};

use super::super::{
    PendingTmemTransaction, PhysicalTmemError, PhysicalTmemPacketTransaction, PhysicalTmemState,
};
use super::{
    prepare_load_block, prepare_load_tile, LoadBlockExecutionError, LoadTileExecutionError,
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
///    a `LoadTlut` command or a `LoadTile`/`LoadBlock` command whose contract
///    turns out to be YUV-deferred rejects the whole packet here. Nothing is
///    staged and no [`PhysicalTmemPacketTransaction`] is constructed during
///    this pass.
/// 2. **Execute.** Only once every command has cleared validation does this
///    pass dispatch each `LoadTile`/`LoadBlock` to its M4.2b/M4.2c executor
///    in decode order, chaining the result into one packet-local
///    transaction. Because pass 1 already proved no TLUT/YUV command exists
///    in this packet, pass 2 can never observe one; [`checked_load`] is the
///    single helper both passes call so that can't drift apart.
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
        let Some((load, is_tile)) = checked_load(command.kind())? else {
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
        packet = Some(if is_tile {
            execute_tile(submitted, &transfer, staged)?
        } else {
            execute_block(submitted, &transfer, staged)?
        });
    }
    let packet = packet.ok_or(TmemPacketExecutionError::NoExecutableLoads)?;
    packet
        .into_pending()
        .map_err(TmemPacketExecutionError::Physical)
}

/// Checked classification of one command's load, shared by both the
/// validation pass and the execution pass so the two can never disagree
/// about which commands are TLUT, YUV-deferred, or executable.
///
/// Returns `Ok(None)` for a command that carries no TMEM load at all (it is
/// simply skipped). Returns `Ok(Some((load, is_tile)))` for a `LoadTile`
/// (`is_tile = true`) or `LoadBlock` (`is_tile = false`) command whose
/// contract is not YUV-deferred. Returns `Err` for a `LoadTlut` command (in
/// or out of scope, see [`TmemPacketExecutionError::TlutExecutorNotLanded`])
/// or a Tile/Block command whose contract is YUV-deferred.
///
/// This helper does not call [`crate::raw_dpc::RawDpcResourcePlan::bind_tmem_transfer`]
/// itself for Tile/Block commands — the execution pass still needs the
/// borrowed [`BoundTmemTransfer`] that call produces, and a second,
/// independent lookup here would let the two passes see different contracts
/// for the same command if the resource plan were ever position-dependent.
/// Instead this helper inspects [`crate::TmemLoad::contract`] directly, the
/// same source `bind_tmem_transfer` itself reads to decide the YUV-deferred
/// case — so both passes reject on exactly the same fact.
fn checked_load(
    kind: RawDpcCommandKind,
) -> Result<Option<(crate::TmemLoad, bool)>, TmemPacketExecutionError> {
    let (load, is_tile) = match kind {
        RawDpcCommandKind::LoadTlut(load) => {
            return Err(TmemPacketExecutionError::TlutExecutorNotLanded {
                epoch: load.epoch(),
            });
        }
        RawDpcCommandKind::LoadTile(load) => (load, true),
        RawDpcCommandKind::LoadBlock(load) => (load, false),
        _ => return Ok(None),
    };
    if let crate::TmemLoadContract::DeferredYuv { .. } = load.contract() {
        return Err(TmemPacketExecutionError::DeferredLoadKind {
            error: TmemLoadSourcePlanError::YuvExecutionDeferred,
            epoch: load.epoch(),
        });
    }
    Ok(Some((load, is_tile)))
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
/// [`TmemPacketExecutionError::TlutExecutorNotLanded`] or
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

#[derive(Debug)]
pub enum TmemPacketExecutionError {
    /// `submitted` is not exactly `decoded.submitted()` — queue, submission
    /// ordinal/identity, workload, journal, or memory-layout identity
    /// disagreed at the named `field`. Checked before any other exit, so a
    /// foreign ticket can never surface as `TlutExecutorNotLanded` or
    /// `NoExecutableLoads` instead.
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
    /// A `LoadTlut` command appeared in the packet. Since M4.3.1,
    /// `RawDpcResourcePlan::bind_tmem_transfer` produces a valid, bindable
    /// `BoundTmemTransfer` for TLUT loads — the destination transfer-plan is
    /// closed — but no physical executor (`prepare_load_tlut`, M4.3.2) exists
    /// yet to actually write TMEM from it. This is a scope boundary, not a
    /// bug and not a decode-time defer: the load is well-formed, this loop
    /// simply refuses to execute it. The packet is rejected before any load
    /// in it is staged.
    TlutExecutorNotLanded {
        epoch: crate::TmemLoadEpoch,
    },
    /// No `LoadTile`/`LoadBlock` command was present to execute.
    NoExecutableLoads,
    Tile(LoadTileExecutionError),
    Block(LoadBlockExecutionError),
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
            Self::TlutExecutorNotLanded { epoch } => write!(
                formatter,
                "TMEM packet LoadTLUT at epoch {} has a valid transfer plan but no physical \
                 executor until M4.3.2",
                epoch.get()
            ),
            Self::NoExecutableLoads => {
                formatter.write_str("TMEM packet has no executable LoadTile/LoadBlock command")
            }
            Self::Tile(error) => error.fmt(formatter),
            Self::Block(error) => error.fmt(formatter),
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
    use crate::{PhysicalTmemState, RdpState};

    const LAYOUT_BYTES: u32 = 0x4000;
    const COMMAND_START: u32 = 0x1000;

    struct Fixture {
        decoded: crate::DecodedRawDpc,
        backend: BackendCompletionAuthority,
        guest: GuestCommitAuthority,
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

    fn fixture(words: Vec<u32>, source_ranges: &[(u32, u32)]) -> Fixture {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let packet = packet_from_words(words, source_ranges);
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        Fixture {
            decoded: decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
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

    // Genuine Tile -> TLUT -> Block: a valid LoadTile, then a LoadTLUT (no
    // executor until M4.3.2), then a second valid load (LoadBlock). Proves
    // TLUT rejects the whole packet regardless of position, including a
    // still-valid executable load declared after it.
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
    fn a_load_tlut_command_is_rejected_loudly_before_any_load_stages() {
        // Since M4.3.1, LoadTLUT decodes a real transfer plan with a
        // journaled `TmemLoadDestination` claim in the high TMEM bank, so
        // this fixture needs the same two-pass planning-probe round trip
        // (`fixture`/`packet_from_words`) as the Tile/Block fixtures, not the
        // single-pass, source-only construction the pre-M4.3.1 test used.
        let (words, ranges) = tlut_words(0x300);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::TlutExecutorNotLanded { .. })
        ));
        // No load staged means the durable state must be untouched.
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn a_load_tlut_between_two_executable_loads_still_rejects_the_whole_packet() {
        // Genuinely Tile -> TLUT -> Block: both the load before and the load
        // after LoadTLUT are individually valid, executable commands. If the
        // pre-stage validation pass (point 1) regressed back to staging
        // while walking, the leading Tile load would already be staged by
        // the time the loop reached LoadTLUT; this proves it never is.
        let (words, ranges) = tile_tlut_block_words(0x200, 0x300, 0x400);
        let fixture = fixture(words, &ranges);
        let state = PhysicalTmemState::try_new().unwrap();
        let result =
            execute_ordered_tmem_loads(&state, fixture.decoded.submitted(), &fixture.decoded);
        assert!(matches!(
            result,
            Err(TmemPacketExecutionError::TlutExecutorNotLanded { .. })
        ));
        // No load staged means the durable state must be untouched, even
        // though both the Tile load before and the Block load after LoadTLUT
        // were themselves individually valid.
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
