use core::fmt;

use crate::{RawStreamKind, WorkloadIdentity};

/// A rejected semantic boundary. Every variant retains enough identity and
/// context to diagnose the exact input; callers must not convert these into a
/// successful no-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    ZeroMemoryLayout,
    MemoryLayoutTooLarge {
        bytes: u32,
        maximum: u32,
    },
    AddressOutOfBounds {
        address: u32,
        upper_bound: u32,
    },
    EmptyOrReversedRange {
        start: u32,
        end: u32,
    },
    RangeOutOfBounds {
        start: u32,
        end: u32,
        upper_bound: u32,
    },
    UnalignedRange {
        start: u32,
        end: u32,
        alignment: u32,
    },
    PayloadLength {
        expected: usize,
        actual: usize,
    },
    EmptyCommandStream {
        source: RawStreamKind,
    },
    TooManyCommandChunks {
        actual: usize,
        maximum: usize,
    },
    CommandStreamTooLarge {
        actual: usize,
        maximum: usize,
    },
    NonMonotonicChunkSequence {
        prior: u64,
        next: u64,
    },
    NonMonotonicFullSyncSequence {
        prior: u64,
        full_sync: u64,
        interrupt: u64,
    },
    DiscontinuousDpInterruptObservation,
    InvalidDpInterruptTransition,
    MissingFullSyncObservation {
        chunk_index: u32,
        occurrence: usize,
    },
    ExtraFullSyncObservation {
        chunk_index: u32,
        expected: usize,
        actual: usize,
    },
    DiscontiguousCommandChunks {
        prior_end: u32,
        next_start: u32,
    },
    UnknownRdpOpcode {
        source: RawStreamKind,
        byte_offset: u32,
        wire_opcode: u8,
    },
    TruncatedRdpCommand {
        source: RawStreamKind,
        byte_offset: u32,
        width: u32,
        stream_bytes: u32,
    },
    InvalidAccessMode {
        purpose: &'static str,
        mode: &'static str,
    },
    InvalidAccessResource {
        purpose: &'static str,
        resource: &'static str,
    },
    ZeroJournalLimit {
        field: &'static str,
    },
    JournalLimitTooLarge {
        field: &'static str,
        actual: u64,
        maximum: u64,
    },
    EmptyResourceJournal,
    TooManyResourceAccesses {
        actual: usize,
        maximum: usize,
    },
    DeclaredResourceBytesOverflow,
    DeclaredResourceBytesExceeded {
        actual: u64,
        maximum: u64,
    },
    EmptyWorkload,
    TooManyPacketStreams {
        actual: usize,
        maximum: usize,
    },
    MemoryLayoutMismatch {
        expected: u32,
    },
    PacketCommandBytesExceeded {
        actual: usize,
        maximum: usize,
    },
    PacketCommandChunksExceeded {
        actual: usize,
        maximum: usize,
    },
    PacketTimelineEventsExceeded {
        actual: usize,
        maximum: usize,
    },
    NonMonotonicPacketEventSequence {
        prior: u64,
        next: u64,
    },
    MissingCommandReadDeclaration {
        source: RawStreamKind,
        start: u32,
        end: u32,
    },
    UnmatchedCommandReadDeclaration {
        access_index: usize,
        source: RawStreamKind,
        start: u32,
        end: u32,
    },
    DeferredGuestReadUnsupportedRegion {
        access_index: usize,
        operation: u32,
    },
    GuestReadStorageLayoutUnaligned {
        bytes: u32,
        alignment: u32,
    },
    GuestReadPlanMismatch,
    GuestReadCountMismatch {
        expected: usize,
        actual: usize,
    },
    GuestReadDescriptorMismatch {
        index: usize,
    },
    GuestReadByteCountMismatch {
        index: usize,
        expected: u32,
        actual: u32,
    },
    GuestReadAggregateByteCountMismatch {
        expected: u64,
        actual: u64,
    },
    GuestReadDigestMismatch {
        index: usize,
    },
    ReplayGuestReadCaptureRequired {
        count: usize,
    },
    ReplayGuestReadSetMismatch,
    TicketAuthorityExhausted,
    SubmissionOrdinalExhausted {
        queue: u64,
    },
    ReceiptAuthorityMismatch,
    ReceiptEffectMismatch,
    GuestMemoryPreimageMismatch,
    EffectForReadOnlyAccess,
    GuestRenderTargetWriteShapeMismatch {
        mode: &'static str,
        purpose: &'static str,
    },
    EffectByteCountMismatch {
        expected: u32,
        actual: u32,
    },
    EffectCountMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    EffectAccessMismatch {
        field: &'static str,
        index: usize,
    },
    ReceiptWorkloadMismatch {
        expected: WorkloadIdentity,
        actual: WorkloadIdentity,
    },
    ReceiptSubmissionMismatch,
    ReceiptJournalMismatch,
    RecordMagic,
    RecordVersion {
        actual: u16,
    },
    RecordTruncated {
        field: &'static str,
    },
    RecordInvalidTag {
        field: &'static str,
        tag: u8,
    },
    RecordInvalidField {
        field: &'static str,
        reason: String,
    },
    RecordTrailingBytes {
        bytes: usize,
    },
    RecordIntegrityMismatch,
    RecordTooLarge {
        actual: usize,
        maximum: usize,
    },
    RecordIdentityMismatch {
        expected: WorkloadIdentity,
        actual: WorkloadIdentity,
    },
    ReplayStreamCount {
        expected: usize,
        actual: usize,
    },
    ReplayStreamMismatch {
        index: usize,
    },
    NumericOverflow {
        field: &'static str,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMemoryLayout => formatter.write_str("physical memory layout is zero bytes"),
            Self::MemoryLayoutTooLarge { bytes, maximum } => write!(
                formatter,
                "physical memory layout {bytes:#x} exceeds the RDP-visible {maximum:#x}-byte address space"
            ),
            Self::AddressOutOfBounds { address, upper_bound } => write!(
                formatter,
                "physical address {address:#010x} is outside [0, {upper_bound:#010x})"
            ),
            Self::EmptyOrReversedRange { start, end } => write!(
                formatter,
                "range [{start:#010x}, {end:#010x}) is empty or reversed"
            ),
            Self::RangeOutOfBounds { start, end, upper_bound } => write!(
                formatter,
                "range [{start:#010x}, {end:#010x}) exceeds [0, {upper_bound:#010x})"
            ),
            Self::UnalignedRange { start, end, alignment } => write!(
                formatter,
                "range [{start:#010x}, {end:#010x}) is not {alignment}-byte aligned"
            ),
            Self::PayloadLength { expected, actual } => write!(
                formatter,
                "owned payload has {actual} bytes; source range requires exactly {expected}"
            ),
            Self::EmptyCommandStream { source } => {
                write!(formatter, "{source:?} raw command stream contains no CMD_END chunks")
            }
            Self::TooManyCommandChunks { actual, maximum } => write!(
                formatter,
                "raw command stream has {actual} chunks; hard bound is {maximum}"
            ),
            Self::CommandStreamTooLarge { actual, maximum } => write!(
                formatter,
                "raw command stream has {actual} bytes; hard bound is {maximum}"
            ),
            Self::NonMonotonicChunkSequence { prior, next } => write!(
                formatter,
                "raw command chunk sequence is not strictly increasing: {prior} then {next}"
            ),
            Self::NonMonotonicFullSyncSequence { prior, full_sync, interrupt } => write!(
                formatter,
                "FullSync observation is not strictly ordered after {prior}: command {full_sync}, interrupt observation {interrupt}"
            ),
            Self::DiscontinuousDpInterruptObservation => formatter.write_str(
                "FullSync interrupt observation does not begin at the preceding captured interrupt level",
            ),
            Self::InvalidDpInterruptTransition => formatter.write_str(
                "FullSync completion observation cannot clear an asserted DP interrupt",
            ),
            Self::MissingFullSyncObservation { chunk_index, occurrence } => write!(
                formatter,
                "raw command chunk {chunk_index} has decoded FullSync occurrence {occurrence} without temporal observation"
            ),
            Self::ExtraFullSyncObservation { chunk_index, expected, actual } => write!(
                formatter,
                "raw command chunk {chunk_index} has {actual} FullSync observations but decoded {expected} commands"
            ),
            Self::DiscontiguousCommandChunks { prior_end, next_start } => write!(
                formatter,
                "raw command chunks are discontiguous: prior CMD_END {prior_end:#010x}, next start {next_start:#010x}"
            ),
            Self::UnknownRdpOpcode { source, byte_offset, wire_opcode } => write!(
                formatter,
                "{source:?} raw RDP opcode {:#04x} (wire {wire_opcode:#04x}) at stream byte {byte_offset:#x} has no admitted public width",
                wire_opcode & 0x3f
            ),
            Self::TruncatedRdpCommand { source, byte_offset, width, stream_bytes } => write!(
                formatter,
                "{source:?} raw RDP command at stream byte {byte_offset:#x} needs {width} bytes but stream ends at {stream_bytes:#x}"
            ),
            Self::InvalidAccessMode { purpose, mode } => {
                write!(formatter, "resource purpose {purpose} rejects access mode {mode}")
            }
            Self::InvalidAccessResource { purpose, resource } => {
                write!(formatter, "resource purpose {purpose} rejects resource class {resource}")
            }
            Self::ZeroJournalLimit { field } => write!(formatter, "resource journal limit {field} is zero"),
            Self::JournalLimitTooLarge { field, actual, maximum } => write!(
                formatter,
                "resource journal limit {field}={actual} exceeds hard bound {maximum}"
            ),
            Self::EmptyResourceJournal => formatter.write_str("workload resource journal is empty"),
            Self::TooManyResourceAccesses { actual, maximum } => write!(
                formatter,
                "resource journal has {actual} accesses; configured bound is {maximum}"
            ),
            Self::DeclaredResourceBytesOverflow => formatter.write_str("declared resource byte count overflowed"),
            Self::DeclaredResourceBytesExceeded { actual, maximum } => write!(
                formatter,
                "resource journal declares {actual} bytes; configured bound is {maximum}"
            ),
            Self::EmptyWorkload => formatter.write_str("workload contains no raw command streams"),
            Self::TooManyPacketStreams { actual, maximum } => write!(
                formatter,
                "workload contains {actual} raw streams; hard bound is {maximum}"
            ),
            Self::MemoryLayoutMismatch { expected } => write!(
                formatter,
                "workload range does not retain the packet's exact {expected:#x}-byte installed-memory layout"
            ),
            Self::PacketCommandBytesExceeded { actual, maximum } => write!(
                formatter,
                "workload owns {actual} command bytes; aggregate bound is {maximum}"
            ),
            Self::PacketCommandChunksExceeded { actual, maximum } => write!(
                formatter,
                "workload owns {actual} command chunks; aggregate bound is {maximum}"
            ),
            Self::PacketTimelineEventsExceeded { actual, maximum } => write!(
                formatter,
                "workload owns {actual} temporal events; aggregate bound is {maximum}"
            ),
            Self::NonMonotonicPacketEventSequence { prior, next } => write!(
                formatter,
                "packet-global event sequence is not strictly increasing: {prior} then {next}"
            ),
            Self::MissingCommandReadDeclaration { source, start, end } => write!(
                formatter,
                "workload journal lacks an exact CommandDecode read for {source:?} [{start:#010x}, {end:#010x})"
            ),
            Self::UnmatchedCommandReadDeclaration { access_index, source, start, end } => write!(
                formatter,
                "workload journal CommandDecode access {access_index} for {source:?} [{start:#010x}, {end:#010x}) has no one-to-one stream owner"
            ),
            Self::DeferredGuestReadUnsupportedRegion { access_index, operation } => write!(
                formatter,
                "journal TmemLoadSource access {access_index} (operation {operation}) is not an RDRAM range and cannot cross the ABI guest-memory boundary"
            ),
            Self::GuestReadStorageLayoutUnaligned { bytes, alignment } => write!(
                formatter,
                "deferred guest-read storage layout length {bytes} is not aligned to its {alignment}-byte native word mapping"
            ),
            Self::GuestReadPlanMismatch => formatter.write_str(
                "deferred guest-read plan does not belong to the packet's exact memory layout and resource journal",
            ),
            Self::GuestReadCountMismatch { expected, actual } => write!(
                formatter,
                "deferred guest-read capture contains {actual} entries; exact plan requires {expected}"
            ),
            Self::GuestReadDescriptorMismatch { index } => write!(
                formatter,
                "deferred guest-read capture entry {index} does not match the exact ordered plan operation/range"
            ),
            Self::GuestReadByteCountMismatch { index, expected, actual } => write!(
                formatter,
                "deferred guest-read capture entry {index} owns {actual} bytes; exact range requires {expected}"
            ),
            Self::GuestReadAggregateByteCountMismatch { expected, actual } => write!(
                formatter,
                "deferred guest-read capture owns {actual} aggregate bytes; exact plan requires {expected}"
            ),
            Self::GuestReadDigestMismatch { index } => write!(
                formatter,
                "deferred guest-read capture entry {index} content digest does not match its owned bytes"
            ),
            Self::ReplayGuestReadCaptureRequired { count } => write!(
                formatter,
                "workload replay requires an owned deferred guest-read capture with {count} entries"
            ),
            Self::ReplayGuestReadSetMismatch => formatter.write_str(
                "replayed deferred guest-read set does not match the record identity/content digests",
            ),
            Self::TicketAuthorityExhausted => formatter.write_str("ticket role authority identity space is exhausted"),
            Self::SubmissionOrdinalExhausted { queue } => write!(formatter, "submission ordinal space is exhausted for queue {queue}"),
            Self::ReceiptAuthorityMismatch => formatter.write_str("receipt was issued by a different lifecycle role authority"),
            Self::ReceiptEffectMismatch => formatter.write_str("receipt effects do not match the active workload or backend effects"),
            Self::GuestMemoryPreimageMismatch => formatter.write_str("guest memory no longer matches the exact captured transaction preimage"),
            Self::EffectForReadOnlyAccess => formatter.write_str("a completed write cannot name a read-only resource access"),
            Self::GuestRenderTargetWriteShapeMismatch { mode, purpose } => write!(
                formatter,
                "single guest render-target write requires access mode Write and purpose RenderTarget; supplied write has mode {mode} and purpose {purpose}"
            ),
            Self::EffectByteCountMismatch { expected, actual } => write!(formatter, "completed write reports {actual} bytes; declared region requires {expected}"),
            Self::EffectCountMismatch { field, expected, actual } => write!(formatter, "{field} count is {actual}; exact journal requires {expected}"),
            Self::EffectAccessMismatch { field, index } => write!(formatter, "{field} {index} does not match the exact ordered journal access/effect"),
            Self::ReceiptWorkloadMismatch { expected, actual } => write!(
                formatter,
                "receipt workload {actual} does not match active workload {expected}"
            ),
            Self::ReceiptSubmissionMismatch => formatter.write_str("completion receipt names a different submission"),
            Self::ReceiptJournalMismatch => formatter.write_str("receipt resource journal identity does not match the active workload"),
            Self::RecordMagic => formatter.write_str("workload record magic is not fn64.render-ir.record.v3"),
            Self::RecordVersion { actual } => write!(formatter, "workload record version {actual} is unsupported"),
            Self::RecordTruncated { field } => write!(formatter, "workload record ended while decoding {field}"),
            Self::RecordInvalidTag { field, tag } => write!(formatter, "workload record {field} has invalid tag {tag}"),
            Self::RecordInvalidField { field, reason } => write!(formatter, "workload record {field} is invalid: {reason}"),
            Self::RecordTrailingBytes { bytes } => write!(formatter, "workload record has {bytes} trailing bytes"),
            Self::RecordIntegrityMismatch => formatter.write_str("workload record SHA-256 integrity digest does not match its body"),
            Self::RecordTooLarge { actual, maximum } => write!(formatter, "workload record has {actual} bytes; metadata bound is {maximum}"),
            Self::RecordIdentityMismatch { expected, actual } => write!(formatter, "replayed workload identity {actual} does not match record identity {expected}"),
            Self::ReplayStreamCount { expected, actual } => write!(formatter, "replay supplied {actual} streams; record requires {expected}"),
            Self::ReplayStreamMismatch { index } => write!(formatter, "replay stream {index} does not match its recorded source/content identity"),
            Self::NumericOverflow { field } => write!(formatter, "numeric field {field} overflowed its canonical representation"),
        }
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::ValidationError;

    #[test]
    fn guest_render_target_write_shape_mismatch_displays_both_offending_fields() {
        let error = ValidationError::GuestRenderTargetWriteShapeMismatch {
            mode: "Read",
            purpose: "TmemLoadDestination",
        };
        assert_eq!(
            error.to_string(),
            "single guest render-target write requires access mode Write and purpose \
             RenderTarget; supplied write has mode Read and purpose TmemLoadDestination"
        );
    }
}
