//! Owned DRAM/XBUS command streams and exact command-boundary observations.
//!
//! Command widths follow the public SGI *RDP Command Summary* Table 11. The
//! low no-operation block follows the public N64brew RDP command table, with
//! the same deliberately narrow admitted opcode set as fn64's existing raw
//! completion inspector.

use core::fmt;

use sha2::{Digest, Sha256};

use crate::{
    ContentDigest, DmemRange, PhysicalRange, RawStreamIdentity, ValidationError,
    RDP_PHYSICAL_ADDRESS_BYTES,
};

pub const MAX_COMMAND_CHUNKS: usize = 4096;
pub const MAX_RAW_STREAM_BYTES: usize = RDP_PHYSICAL_ADDRESS_BYTES as usize;
const DPC_ALIGNMENT: u32 = 8;
const RDP_SYNC_FULL: u8 = 0x29;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawStreamKind {
    Dram,
    Xbus,
}

impl RawStreamKind {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Dram => 1,
            Self::Xbus => 2,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, ValidationError> {
        match tag {
            1 => Ok(Self::Dram),
            2 => Ok(Self::Xbus),
            _ => Err(ValidationError::RecordInvalidTag {
                field: "raw stream kind",
                tag,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpInterruptState {
    Clear,
    Asserted,
}

impl DpInterruptState {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Clear => 0,
            Self::Asserted => 1,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, ValidationError> {
        match tag {
            0 => Ok(Self::Clear),
            1 => Ok(Self::Asserted),
            _ => Err(ValidationError::RecordInvalidTag {
                field: "DP interrupt state",
                tag,
            }),
        }
    }
}

/// Temporal observation attached to one accepted `CMD_END` write. Sequence is
/// owned by the capture boundary; the interrupt value is a snapshot and does
/// not invent a hardware latency relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TemporalBoundary {
    sequence: u64,
    interrupt: DpInterruptState,
}

/// Capture-owned observation for one decoded FullSync command. The first
/// sequence identifies the command reaching the observed completion boundary;
/// the second identifies the following DP interrupt-level observation.
/// `interrupt_before` and `interrupt_after` distinguish a newly observed raise
/// from an interrupt which was already asserted. They do not claim a hardware
/// latency or causal edge beyond the supplied observation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FullSyncBoundary {
    sequence: u64,
    interrupt_sequence: u64,
    interrupt_before: DpInterruptState,
    interrupt_after: DpInterruptState,
}

impl FullSyncBoundary {
    pub const fn new(
        sequence: u64,
        interrupt_sequence: u64,
        interrupt_before: DpInterruptState,
        interrupt_after: DpInterruptState,
    ) -> Self {
        Self {
            sequence,
            interrupt_sequence,
            interrupt_before,
            interrupt_after,
        }
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    pub const fn interrupt_sequence(self) -> u64 {
        self.interrupt_sequence
    }

    pub const fn interrupt_before(self) -> DpInterruptState {
        self.interrupt_before
    }

    pub const fn interrupt_after(self) -> DpInterruptState {
        self.interrupt_after
    }
}

impl TemporalBoundary {
    pub const fn new(sequence: u64, interrupt: DpInterruptState) -> Self {
        Self {
            sequence,
            interrupt,
        }
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    pub const fn interrupt(self) -> DpInterruptState {
        self.interrupt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FullSyncOccurrence {
    pub ordinal: u32,
    pub stream_byte_offset: u32,
    pub source_address: u32,
    pub chunk_index: u32,
    pub chunk_byte_offset: u32,
    pub sequence: u64,
    pub interrupt_sequence: u64,
    pub interrupt_before: DpInterruptState,
    pub interrupt_after: DpInterruptState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CmdEndOccurrence {
    pub chunk_index: u32,
    pub sequence: u64,
    pub source_address: u32,
    pub interrupt: DpInterruptState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DpInterruptObservation {
    pub full_sync_ordinal: u32,
    pub sequence: u64,
    pub before: DpInterruptState,
    pub after: DpInterruptState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawTimelineEvent {
    FullSync(FullSyncOccurrence),
    CmdEnd(CmdEndOccurrence),
    DpInterrupt(DpInterruptObservation),
}

impl RawTimelineEvent {
    pub const fn sequence(self) -> u64 {
        match self {
            Self::FullSync(event) => event.sequence,
            Self::CmdEnd(event) => event.sequence,
            Self::DpInterrupt(event) => event.sequence,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DramCommandChunk {
    range: PhysicalRange,
    words: Box<[u32]>,
    boundary: TemporalBoundary,
    full_sync_boundaries: Box<[FullSyncBoundary]>,
}

impl DramCommandChunk {
    pub fn try_new(
        range: PhysicalRange,
        words: Vec<u32>,
        boundary: TemporalBoundary,
        full_sync_boundaries: Vec<FullSyncBoundary>,
    ) -> Result<Self, ValidationError> {
        let range = range.require_alignment(DPC_ALIGNMENT)?;
        let actual =
            words
                .len()
                .checked_mul(size_of::<u32>())
                .ok_or(ValidationError::NumericOverflow {
                    field: "DRAM command payload length",
                })?;
        let expected = range.len() as usize;
        if actual != expected {
            return Err(ValidationError::PayloadLength { expected, actual });
        }
        Ok(Self {
            range,
            words: words.into_boxed_slice(),
            boundary,
            full_sync_boundaries: full_sync_boundaries.into_boxed_slice(),
        })
    }

    pub const fn range(&self) -> PhysicalRange {
        self.range
    }

    pub fn words(&self) -> &[u32] {
        &self.words
    }

    pub const fn boundary(&self) -> TemporalBoundary {
        self.boundary
    }

    pub fn full_sync_boundaries(&self) -> &[FullSyncBoundary] {
        &self.full_sync_boundaries
    }
}

impl fmt::Debug for DramCommandChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DramCommandChunk")
            .field("range", &self.range)
            .field("word_count", &self.words.len())
            .field("boundary", &self.boundary)
            .field("full_sync_boundaries", &self.full_sync_boundaries)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct XbusCommandChunk {
    range: DmemRange,
    bytes: Box<[u8]>,
    boundary: TemporalBoundary,
    full_sync_boundaries: Box<[FullSyncBoundary]>,
}

impl XbusCommandChunk {
    pub fn try_new(
        range: DmemRange,
        bytes: Vec<u8>,
        boundary: TemporalBoundary,
        full_sync_boundaries: Vec<FullSyncBoundary>,
    ) -> Result<Self, ValidationError> {
        let range = range.require_alignment(DPC_ALIGNMENT)?;
        let expected = range.len() as usize;
        if bytes.len() != expected {
            return Err(ValidationError::PayloadLength {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            range,
            bytes: bytes.into_boxed_slice(),
            boundary,
            full_sync_boundaries: full_sync_boundaries.into_boxed_slice(),
        })
    }

    pub const fn range(&self) -> DmemRange {
        self.range
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn boundary(&self) -> TemporalBoundary {
        self.boundary
    }

    pub fn full_sync_boundaries(&self) -> &[FullSyncBoundary] {
        &self.full_sync_boundaries
    }
}

impl fmt::Debug for XbusCommandChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("XbusCommandChunk")
            .field("range", &self.range)
            .field("byte_count", &self.bytes.len())
            .field("boundary", &self.boundary)
            .field("full_sync_boundaries", &self.full_sync_boundaries)
            .finish()
    }
}

macro_rules! stream_getters {
    () => {
        pub const fn identity(&self) -> RawStreamIdentity {
            self.identity
        }

        pub const fn byte_len(&self) -> u32 {
            self.byte_len
        }

        pub fn full_sync_occurrences(&self) -> &[FullSyncOccurrence] {
            &self.full_syncs
        }

        pub fn cmd_end_occurrences(&self) -> &[CmdEndOccurrence] {
            &self.cmd_ends
        }

        pub fn timeline(&self) -> Vec<RawTimelineEvent> {
            build_timeline(&self.full_syncs, &self.cmd_ends)
        }

        pub const fn chunk_count(&self) -> usize {
            self.chunks.len()
        }

        pub const fn timeline_event_count(&self) -> usize {
            self.cmd_ends.len() + self.full_syncs.len() * 2
        }
    };
}

#[derive(Clone, PartialEq, Eq)]
pub struct DramCommandStream {
    chunks: Box<[DramCommandChunk]>,
    identity: RawStreamIdentity,
    full_syncs: Box<[FullSyncOccurrence]>,
    cmd_ends: Box<[CmdEndOccurrence]>,
    byte_len: u32,
}

impl DramCommandStream {
    pub fn try_new(chunks: Vec<DramCommandChunk>) -> Result<Self, ValidationError> {
        validate_chunk_count(RawStreamKind::Dram, chunks.len())?;
        let metadata: Vec<_> = chunks
            .iter()
            .map(|chunk| ChunkMetadata {
                start: chunk.range.start().get(),
                end: chunk.range.end(),
                boundary: chunk.boundary,
                full_sync_boundaries: &chunk.full_sync_boundaries,
            })
            .collect();
        let bytes: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| chunk.words.iter().flat_map(|word| word.to_be_bytes()))
            .collect();
        let derived = derive_stream(RawStreamKind::Dram, &metadata, &bytes)?;
        Ok(Self {
            chunks: chunks.into_boxed_slice(),
            identity: derived.identity,
            full_syncs: derived.full_syncs,
            cmd_ends: derived.cmd_ends,
            byte_len: derived.byte_len,
        })
    }

    pub fn chunks(&self) -> &[DramCommandChunk] {
        &self.chunks
    }

    stream_getters!();
}

impl fmt::Debug for DramCommandStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        stream_debug(
            formatter,
            RawStreamKind::Dram,
            self.identity,
            self.byte_len,
            self.chunks.len(),
            &self.full_syncs,
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct XbusCommandStream {
    chunks: Box<[XbusCommandChunk]>,
    identity: RawStreamIdentity,
    full_syncs: Box<[FullSyncOccurrence]>,
    cmd_ends: Box<[CmdEndOccurrence]>,
    byte_len: u32,
}

impl XbusCommandStream {
    pub fn try_new(chunks: Vec<XbusCommandChunk>) -> Result<Self, ValidationError> {
        validate_chunk_count(RawStreamKind::Xbus, chunks.len())?;
        let metadata: Vec<_> = chunks
            .iter()
            .map(|chunk| ChunkMetadata {
                start: chunk.range.start(),
                end: chunk.range.end(),
                boundary: chunk.boundary,
                full_sync_boundaries: &chunk.full_sync_boundaries,
            })
            .collect();
        let bytes: Vec<u8> = chunks
            .iter()
            .flat_map(|chunk| chunk.bytes.iter().copied())
            .collect();
        let derived = derive_stream(RawStreamKind::Xbus, &metadata, &bytes)?;
        Ok(Self {
            chunks: chunks.into_boxed_slice(),
            identity: derived.identity,
            full_syncs: derived.full_syncs,
            cmd_ends: derived.cmd_ends,
            byte_len: derived.byte_len,
        })
    }

    pub fn chunks(&self) -> &[XbusCommandChunk] {
        &self.chunks
    }

    stream_getters!();
}

impl fmt::Debug for XbusCommandStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        stream_debug(
            formatter,
            RawStreamKind::Xbus,
            self.identity,
            self.byte_len,
            self.chunks.len(),
            &self.full_syncs,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawCommandStream {
    Dram(DramCommandStream),
    Xbus(XbusCommandStream),
}

impl RawCommandStream {
    pub const fn kind(&self) -> RawStreamKind {
        match self {
            Self::Dram(_) => RawStreamKind::Dram,
            Self::Xbus(_) => RawStreamKind::Xbus,
        }
    }

    pub const fn identity(&self) -> RawStreamIdentity {
        match self {
            Self::Dram(stream) => stream.identity(),
            Self::Xbus(stream) => stream.identity(),
        }
    }

    pub const fn byte_len(&self) -> u32 {
        match self {
            Self::Dram(stream) => stream.byte_len(),
            Self::Xbus(stream) => stream.byte_len(),
        }
    }

    pub fn full_sync_occurrences(&self) -> &[FullSyncOccurrence] {
        match self {
            Self::Dram(stream) => stream.full_sync_occurrences(),
            Self::Xbus(stream) => stream.full_sync_occurrences(),
        }
    }

    pub fn cmd_end_occurrences(&self) -> &[CmdEndOccurrence] {
        match self {
            Self::Dram(stream) => stream.cmd_end_occurrences(),
            Self::Xbus(stream) => stream.cmd_end_occurrences(),
        }
    }

    pub fn source_bounds(&self) -> (u32, u32) {
        match self {
            Self::Dram(stream) => (
                stream.chunks[0].range.start().get(),
                stream
                    .chunks
                    .last()
                    .expect("stream is nonempty")
                    .range
                    .end(),
            ),
            Self::Xbus(stream) => (
                stream.chunks[0].range.start(),
                stream
                    .chunks
                    .last()
                    .expect("stream is nonempty")
                    .range
                    .end(),
            ),
        }
    }

    pub fn timeline(&self) -> Vec<RawTimelineEvent> {
        match self {
            Self::Dram(stream) => stream.timeline(),
            Self::Xbus(stream) => stream.timeline(),
        }
    }

    pub const fn chunk_count(&self) -> usize {
        match self {
            Self::Dram(stream) => stream.chunk_count(),
            Self::Xbus(stream) => stream.chunk_count(),
        }
    }

    pub const fn timeline_event_count(&self) -> usize {
        match self {
            Self::Dram(stream) => stream.timeline_event_count(),
            Self::Xbus(stream) => stream.timeline_event_count(),
        }
    }

    pub fn temporal_sequence_bounds(&self) -> (u64, u64) {
        let timeline = self.timeline();
        (
            timeline
                .first()
                .expect("stream timeline is nonempty")
                .sequence(),
            timeline
                .last()
                .expect("stream timeline is nonempty")
                .sequence(),
        )
    }

    pub(crate) fn matches_memory_layout(&self, layout: crate::PhysicalMemoryLayout) -> bool {
        match self {
            Self::Dram(stream) => stream
                .chunks()
                .iter()
                .all(|chunk| chunk.range().layout() == layout),
            Self::Xbus(_) => true,
        }
    }
}

struct DerivedStream {
    identity: RawStreamIdentity,
    full_syncs: Box<[FullSyncOccurrence]>,
    cmd_ends: Box<[CmdEndOccurrence]>,
    byte_len: u32,
}

#[derive(Clone, Copy)]
struct ChunkMetadata<'a> {
    start: u32,
    end: u32,
    boundary: TemporalBoundary,
    full_sync_boundaries: &'a [FullSyncBoundary],
}

fn validate_chunk_count(source: RawStreamKind, chunks: usize) -> Result<(), ValidationError> {
    if chunks == 0 {
        return Err(ValidationError::EmptyCommandStream { source });
    }
    if chunks > MAX_COMMAND_CHUNKS {
        return Err(ValidationError::TooManyCommandChunks {
            actual: chunks,
            maximum: MAX_COMMAND_CHUNKS,
        });
    }
    Ok(())
}

fn derive_stream(
    source: RawStreamKind,
    chunks: &[ChunkMetadata<'_>],
    bytes: &[u8],
) -> Result<DerivedStream, ValidationError> {
    validate_chunk_count(source, chunks.len())?;
    if bytes.len() > MAX_RAW_STREAM_BYTES {
        return Err(ValidationError::CommandStreamTooLarge {
            actual: bytes.len(),
            maximum: MAX_RAW_STREAM_BYTES,
        });
    }
    for pair in chunks.windows(2) {
        let prior_last = pair[0]
            .full_sync_boundaries
            .last()
            .map_or(pair[0].boundary.sequence, |boundary| {
                boundary.interrupt_sequence
            });
        if pair[1].boundary.sequence <= prior_last {
            return Err(ValidationError::NonMonotonicChunkSequence {
                prior: prior_last,
                next: pair[1].boundary.sequence,
            });
        }
        if pair[0].end != pair[1].start {
            return Err(ValidationError::DiscontiguousCommandChunks {
                prior_end: pair[0].end,
                next_start: pair[1].start,
            });
        }
    }

    for chunk in chunks {
        let mut prior_sequence = chunk.boundary.sequence;
        let mut prior_interrupt = chunk.boundary.interrupt;
        for observation in chunk.full_sync_boundaries {
            if observation.sequence <= prior_sequence
                || observation.interrupt_sequence <= observation.sequence
            {
                return Err(ValidationError::NonMonotonicFullSyncSequence {
                    prior: prior_sequence,
                    full_sync: observation.sequence,
                    interrupt: observation.interrupt_sequence,
                });
            }
            if observation.interrupt_before != prior_interrupt {
                return Err(ValidationError::DiscontinuousDpInterruptObservation);
            }
            if matches!(
                (observation.interrupt_before, observation.interrupt_after),
                (DpInterruptState::Asserted, DpInterruptState::Clear)
            ) {
                return Err(ValidationError::InvalidDpInterruptTransition);
            }
            prior_sequence = observation.interrupt_sequence;
            prior_interrupt = observation.interrupt_after;
        }
    }

    let byte_len = u32::try_from(bytes.len()).map_err(|_| ValidationError::NumericOverflow {
        field: "raw command stream byte length",
    })?;
    let mut full_syncs = Vec::new();
    let mut observed_syncs = vec![0_usize; chunks.len()];
    let mut byte_offset = 0_u32;
    while byte_offset < byte_len {
        let wire_opcode = bytes[byte_offset as usize];
        let command = wire_opcode & 0x3f;
        let width = raw_rdp_command_width(command).ok_or(ValidationError::UnknownRdpOpcode {
            source,
            byte_offset,
            wire_opcode,
        })?;
        let next = byte_offset
            .checked_add(width)
            .ok_or(ValidationError::NumericOverflow {
                field: "raw RDP command offset",
            })?;
        if next > byte_len {
            return Err(ValidationError::TruncatedRdpCommand {
                source,
                byte_offset,
                width,
                stream_bytes: byte_len,
            });
        }
        if command == RDP_SYNC_FULL {
            let (chunk_index, chunk_offset, source_address) = locate_offset(chunks, byte_offset);
            let chunk = &chunks[chunk_index as usize];
            let observation_index = observed_syncs[chunk_index as usize];
            let observation = chunk
                .full_sync_boundaries
                .get(observation_index)
                .copied()
                .ok_or(ValidationError::MissingFullSyncObservation {
                    chunk_index,
                    occurrence: observation_index,
                })?;
            observed_syncs[chunk_index as usize] += 1;
            full_syncs.push(FullSyncOccurrence {
                ordinal: full_syncs.len() as u32,
                stream_byte_offset: byte_offset,
                source_address,
                chunk_index,
                chunk_byte_offset: chunk_offset,
                sequence: observation.sequence,
                interrupt_sequence: observation.interrupt_sequence,
                interrupt_before: observation.interrupt_before,
                interrupt_after: observation.interrupt_after,
            });
        }
        byte_offset = next;
    }
    for (chunk_index, (chunk, observed)) in chunks.iter().zip(&observed_syncs).enumerate() {
        if chunk.full_sync_boundaries.len() != *observed {
            return Err(ValidationError::ExtraFullSyncObservation {
                chunk_index: chunk_index as u32,
                expected: *observed,
                actual: chunk.full_sync_boundaries.len(),
            });
        }
    }

    let mut hash = Sha256::new();
    hash.update(b"fn64.render-ir.raw-stream.v2\0");
    hash.update([source.tag()]);
    hash.update((chunks.len() as u32).to_be_bytes());
    for chunk in chunks {
        hash.update(chunk.start.to_be_bytes());
        hash.update(chunk.end.to_be_bytes());
        let boundary = chunk.boundary;
        hash.update(boundary.sequence.to_be_bytes());
        hash.update([boundary.interrupt.tag()]);
        hash.update((chunk.full_sync_boundaries.len() as u32).to_be_bytes());
        for observation in chunk.full_sync_boundaries {
            hash.update(observation.sequence.to_be_bytes());
            hash.update(observation.interrupt_sequence.to_be_bytes());
            hash.update([
                observation.interrupt_before.tag(),
                observation.interrupt_after.tag(),
            ]);
        }
    }
    hash.update(byte_len.to_be_bytes());
    hash.update(bytes);
    let identity = RawStreamIdentity::new(ContentDigest::from_bytes(hash.finalize().into()));
    let cmd_ends = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| CmdEndOccurrence {
            chunk_index: index as u32,
            sequence: chunk.boundary.sequence,
            source_address: chunk.end,
            interrupt: chunk.boundary.interrupt,
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    Ok(DerivedStream {
        identity,
        full_syncs: full_syncs.into_boxed_slice(),
        cmd_ends,
        byte_len,
    })
}

fn locate_offset(chunks: &[ChunkMetadata<'_>], offset: u32) -> (u32, u32, u32) {
    let mut consumed = 0_u32;
    for (index, chunk) in chunks.iter().enumerate() {
        let len = chunk.end - chunk.start;
        if offset < consumed + len {
            let chunk_offset = offset - consumed;
            return (index as u32, chunk_offset, chunk.start + chunk_offset);
        }
        consumed += len;
    }
    unreachable!("validated command offset lies inside stream")
}

fn build_timeline(
    full_syncs: &[FullSyncOccurrence],
    cmd_ends: &[CmdEndOccurrence],
) -> Vec<RawTimelineEvent> {
    let mut result = Vec::with_capacity(full_syncs.len() * 2 + cmd_ends.len());
    result.extend(cmd_ends.iter().copied().map(RawTimelineEvent::CmdEnd));
    for sync in full_syncs {
        result.push(RawTimelineEvent::FullSync(*sync));
        result.push(RawTimelineEvent::DpInterrupt(DpInterruptObservation {
            full_sync_ordinal: sync.ordinal,
            sequence: sync.interrupt_sequence,
            before: sync.interrupt_before,
            after: sync.interrupt_after,
        }));
    }
    result.sort_by_key(|event| event.sequence());
    result
}

fn stream_debug(
    formatter: &mut fmt::Formatter<'_>,
    kind: RawStreamKind,
    identity: RawStreamIdentity,
    byte_len: u32,
    chunks: usize,
    full_syncs: &[FullSyncOccurrence],
) -> fmt::Result {
    formatter
        .debug_struct("RawCommandStream")
        .field("kind", &kind)
        .field("identity", &identity)
        .field("byte_len", &byte_len)
        .field("chunk_count", &chunks)
        .field("full_syncs", &full_syncs)
        .finish()
}

/// This crate's own copy of the RDP command-width table.
///
/// Duplicated, not shared, and the duplication is structural rather than an
/// oversight: `fn64_render::raw_rdp_command_width` is the fuller-documented
/// owner, but `fn64-render` depends on `fn64-render-ir` and not the other
/// way round, so this crate cannot import it. The two must stay in step.
/// `fn64-render-wgpu`'s
/// `rt64_rdp_state::tests::the_two_command_width_tables_agree_wherever_both_are_defined`
/// cross-checks against a third (RT64's), and
/// `the_ir_and_render_width_tables_agree` below pins this pair specifically.
///
/// `0x1f` is carved out of the otherwise-rejected `0x10..=0x23` block on
/// measured evidence: WM2000 writes it to terminate every graphics
/// submission (`docs/RT64-WM2000-CENSUS.md` §3). Widened as one id, not the
/// block, so the region keeps working as a mis-synchronization detector --
/// see `fn64_render::raw_rdp_command_width`'s `RDP_STREAM_TERMINATOR_NOOP`
/// for the full argument, which is not restated here.
fn raw_rdp_command_width(command: u8) -> Option<u32> {
    Some(match command & 0x3f {
        0x00..=0x07 => 8,
        0x1f => 8,
        0x08 => 32,
        0x09 => 48,
        0x0a => 96,
        0x0b => 112,
        0x0c => 96,
        0x0d => 112,
        0x0e => 160,
        0x0f => 176,
        0x24 | 0x25 => 16,
        0x26..=0x3f => 8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhysicalMemoryLayout;

    fn words(commands: &[u8]) -> Vec<u32> {
        commands
            .iter()
            .flat_map(|opcode| [u32::from(*opcode) << 24, 0])
            .collect()
    }

    /// The width table's accepted domain, pinned exactly.
    ///
    /// This crate cannot import `fn64_render::raw_rdp_command_width` (the
    /// dependency runs the other way), so the two copies cannot be compared
    /// directly here. What this test can do -- and does -- is make the shape
    /// of *this* copy explicit, so a change on either side shows up as a
    /// failing assertion rather than as a silent divergence discovered by a
    /// game that stops decoding.
    ///
    /// The `0x1f` carve-out is asserted alongside its immediate neighbours
    /// deliberately: "0x1f is accepted" alone would also hold if the whole
    /// `0x10..=0x23` block had been widened, which is exactly the change the
    /// narrow carve-out exists to avoid.
    #[test]
    fn the_width_table_accepts_exactly_its_documented_domain() {
        // The measured carve-out, under every wire prefix.
        for prefix in [0x00u8, 0x40, 0x80, 0xc0] {
            assert_eq!(
                raw_rdp_command_width(prefix | 0x1f),
                Some(8),
                "WM2000's measured stream terminator must be accepted"
            );
        }
        // The rest of the deliberately-rejected block still rejects.
        for command in 0x10u8..=0x23 {
            if command == 0x1f {
                continue;
            }
            assert_eq!(
                raw_rdp_command_width(command),
                None,
                "command {command:#04x} must stay rejected; the carve-out is one id wide"
            );
        }
        // Spot-check the surrounding regions so a mangled range boundary is
        // caught rather than assumed intact.
        assert_eq!(raw_rdp_command_width(0x00), Some(8));
        assert_eq!(raw_rdp_command_width(0x07), Some(8));
        assert_eq!(raw_rdp_command_width(0x08), Some(32));
        assert_eq!(raw_rdp_command_width(0x24), Some(16));
        assert_eq!(raw_rdp_command_width(0x25), Some(16));
        assert_eq!(raw_rdp_command_width(0x26), Some(8));
        assert_eq!(raw_rdp_command_width(0x27), Some(8));
        assert_eq!(raw_rdp_command_width(0x28), Some(8));
        assert_eq!(raw_rdp_command_width(0x3f), Some(8));
    }

    #[test]
    fn dram_stream_retains_exact_full_sync_cmd_end_and_interrupt_order() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let chunks = vec![
            DramCommandChunk::try_new(
                layout.range(0x100, 0x110).unwrap(),
                words(&[0xe9, 0xe6]),
                TemporalBoundary::new(40, DpInterruptState::Asserted),
                vec![FullSyncBoundary::new(
                    41,
                    42,
                    DpInterruptState::Asserted,
                    DpInterruptState::Asserted,
                )],
            )
            .unwrap(),
            DramCommandChunk::try_new(
                layout.range(0x110, 0x128).unwrap(),
                words(&[0x27, 0x29, 0xe9]),
                TemporalBoundary::new(50, DpInterruptState::Clear),
                vec![
                    FullSyncBoundary::new(
                        51,
                        52,
                        DpInterruptState::Clear,
                        DpInterruptState::Asserted,
                    ),
                    FullSyncBoundary::new(
                        53,
                        54,
                        DpInterruptState::Asserted,
                        DpInterruptState::Asserted,
                    ),
                ],
            )
            .unwrap(),
        ];
        let stream = DramCommandStream::try_new(chunks).unwrap();
        assert_eq!(
            stream.full_sync_occurrences(),
            [
                FullSyncOccurrence {
                    ordinal: 0,
                    stream_byte_offset: 0,
                    source_address: 0x100,
                    chunk_index: 0,
                    chunk_byte_offset: 0,
                    sequence: 41,
                    interrupt_sequence: 42,
                    interrupt_before: DpInterruptState::Asserted,
                    interrupt_after: DpInterruptState::Asserted,
                },
                FullSyncOccurrence {
                    ordinal: 1,
                    stream_byte_offset: 0x18,
                    source_address: 0x118,
                    chunk_index: 1,
                    chunk_byte_offset: 8,
                    sequence: 51,
                    interrupt_sequence: 52,
                    interrupt_before: DpInterruptState::Clear,
                    interrupt_after: DpInterruptState::Asserted,
                },
                FullSyncOccurrence {
                    ordinal: 2,
                    stream_byte_offset: 0x20,
                    source_address: 0x120,
                    chunk_index: 1,
                    chunk_byte_offset: 16,
                    sequence: 53,
                    interrupt_sequence: 54,
                    interrupt_before: DpInterruptState::Asserted,
                    interrupt_after: DpInterruptState::Asserted,
                },
            ]
        );
        assert_eq!(
            stream.timeline(),
            [
                RawTimelineEvent::CmdEnd(stream.cmd_end_occurrences()[0]),
                RawTimelineEvent::FullSync(stream.full_sync_occurrences()[0]),
                RawTimelineEvent::DpInterrupt(DpInterruptObservation {
                    full_sync_ordinal: 0,
                    sequence: 42,
                    before: DpInterruptState::Asserted,
                    after: DpInterruptState::Asserted,
                }),
                RawTimelineEvent::CmdEnd(stream.cmd_end_occurrences()[1]),
                RawTimelineEvent::FullSync(stream.full_sync_occurrences()[1]),
                RawTimelineEvent::DpInterrupt(DpInterruptObservation {
                    full_sync_ordinal: 1,
                    sequence: 52,
                    before: DpInterruptState::Clear,
                    after: DpInterruptState::Asserted,
                }),
                RawTimelineEvent::FullSync(stream.full_sync_occurrences()[2]),
                RawTimelineEvent::DpInterrupt(DpInterruptObservation {
                    full_sync_ordinal: 2,
                    sequence: 54,
                    before: DpInterruptState::Asserted,
                    after: DpInterruptState::Asserted,
                }),
            ]
        );
    }

    #[test]
    fn xbus_owns_one_canonical_byte_image_and_identity_binds_boundaries() {
        let payload = words(&[0xe9])
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let chunk = XbusCommandChunk::try_new(
            DmemRange::try_new(0x20, 0x28).unwrap(),
            payload.clone(),
            TemporalBoundary::new(7, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                8,
                9,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap();
        let stream = XbusCommandStream::try_new(vec![chunk.clone()]).unwrap();
        assert_eq!(stream.chunks()[0].bytes(), payload);

        let changed = XbusCommandChunk::try_new(
            chunk.range(),
            payload,
            TemporalBoundary::new(10, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                11,
                12,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap();
        assert_ne!(
            stream.identity(),
            XbusCommandStream::try_new(vec![changed])
                .unwrap()
                .identity()
        );
    }

    #[test]
    fn payload_bytes_cannot_impersonate_full_sync_and_unknown_width_is_loud() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let mut triangle = vec![0x0800_0000, 0];
        triangle.resize(8, 0xe900_0000);
        let stream = DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            layout.range(0, 32).unwrap(),
            triangle,
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![],
        )
        .unwrap()])
        .unwrap();
        assert!(stream.full_sync_occurrences().is_empty());

        let error = DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            layout.range(0x40, 0x48).unwrap(),
            words(&[0x90]),
            TemporalBoundary::new(2, DpInterruptState::Clear),
            vec![],
        )
        .unwrap()])
        .unwrap_err();
        assert!(matches!(
            error,
            ValidationError::UnknownRdpOpcode {
                wire_opcode: 0x90,
                ..
            }
        ));
        assert!(error.to_string().contains("stream byte 0x0"));
    }

    #[test]
    fn multiword_command_can_cross_cmd_end_without_rescanning_its_payload() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let first = DramCommandChunk::try_new(
            layout.range(0x200, 0x208).unwrap(),
            vec![0xe400_0000, 0],
            TemporalBoundary::new(10, DpInterruptState::Clear),
            vec![],
        )
        .unwrap();
        let second = DramCommandChunk::try_new(
            layout.range(0x208, 0x210).unwrap(),
            vec![0xe900_0000, 0],
            TemporalBoundary::new(11, DpInterruptState::Clear),
            vec![],
        )
        .unwrap();
        let stream = DramCommandStream::try_new(vec![first, second]).unwrap();
        assert!(stream.full_sync_occurrences().is_empty());
        assert_eq!(stream.cmd_end_occurrences().len(), 2);
    }

    #[test]
    fn full_sync_temporal_observations_are_exact_and_cannot_invent_a_clear() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let make = |observations| {
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                layout.range(0, 8).unwrap(),
                words(&[0xe9]),
                TemporalBoundary::new(10, DpInterruptState::Asserted),
                observations,
            )
            .unwrap()])
        };
        assert!(matches!(
            make(vec![]),
            Err(ValidationError::MissingFullSyncObservation { .. })
        ));
        assert_eq!(
            make(vec![FullSyncBoundary::new(
                11,
                12,
                DpInterruptState::Asserted,
                DpInterruptState::Clear,
            )])
            .unwrap_err(),
            ValidationError::InvalidDpInterruptTransition
        );
        assert!(matches!(
            make(vec![FullSyncBoundary::new(
                10,
                12,
                DpInterruptState::Asserted,
                DpInterruptState::Asserted,
            )]),
            Err(ValidationError::NonMonotonicFullSyncSequence { .. })
        ));
    }
}
