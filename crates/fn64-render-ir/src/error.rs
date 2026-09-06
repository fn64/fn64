use crate::{RawStreamKind, WorkloadIdentity};

/// A rejected semantic boundary. Every variant retains enough identity and
/// context to diagnose the exact input; callers must not convert these into a
/// successful no-op.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("physical memory layout is zero bytes")]
    ZeroMemoryLayout,
    #[error(
        "physical memory layout {bytes:#x} exceeds the RDP-visible {maximum:#x}-byte address space"
    )]
    MemoryLayoutTooLarge {
        bytes: u32,
        maximum: u32,
    },
    #[error("physical address {address:#010x} is outside [0, {upper_bound:#010x})")]
    AddressOutOfBounds {
        address: u32,
        upper_bound: u32,
    },
    #[error("range [{start:#010x}, {end:#010x}) is empty or reversed")]
    EmptyOrReversedRange {
        start: u32,
        end: u32,
    },
    #[error("range [{start:#010x}, {end:#010x}) exceeds [0, {upper_bound:#010x})")]
    RangeOutOfBounds {
        start: u32,
        end: u32,
        upper_bound: u32,
    },
    #[error("range [{start:#010x}, {end:#010x}) is not {alignment}-byte aligned")]
    UnalignedRange {
        start: u32,
        end: u32,
        alignment: u32,
    },
    #[error("owned payload has {actual} bytes; source range requires exactly {expected}")]
    PayloadLength {
        expected: usize,
        actual: usize,
    },
    #[error("{stream:?} raw command stream contains no CMD_END chunks")]
    EmptyCommandStream {
        stream: RawStreamKind,
    },
    #[error("raw command stream has {actual} chunks; hard bound is {maximum}")]
    TooManyCommandChunks {
        actual: usize,
        maximum: usize,
    },
    #[error("raw command stream has {actual} bytes; hard bound is {maximum}")]
    CommandStreamTooLarge {
        actual: usize,
        maximum: usize,
    },
    #[error("raw command chunk sequence is not strictly increasing: {prior} then {next}")]
    NonMonotonicChunkSequence {
        prior: u64,
        next: u64,
    },
    #[error(
        "FullSync observation is not strictly ordered after {prior}: command {full_sync}, interrupt observation {interrupt}"
    )]
    NonMonotonicFullSyncSequence {
        prior: u64,
        full_sync: u64,
        interrupt: u64,
    },
    #[error(
        "FullSync interrupt observation does not begin at the preceding captured interrupt level"
    )]
    DiscontinuousDpInterruptObservation,
    #[error("FullSync completion observation cannot clear an asserted DP interrupt")]
    InvalidDpInterruptTransition,
    #[error(
        "raw command chunk {chunk_index} has decoded FullSync occurrence {occurrence} without temporal observation"
    )]
    MissingFullSyncObservation {
        chunk_index: u32,
        occurrence: usize,
    },
    #[error(
        "raw command chunk {chunk_index} has {actual} FullSync observations but decoded {expected} commands"
    )]
    ExtraFullSyncObservation {
        chunk_index: u32,
        expected: usize,
        actual: usize,
    },
    #[error(
        "raw command chunks are discontiguous: prior CMD_END {prior_end:#010x}, next start {next_start:#010x}"
    )]
    DiscontiguousCommandChunks {
        prior_end: u32,
        next_start: u32,
    },
    #[error(
        "{stream:?} raw RDP opcode {:#04x} (wire {wire_opcode:#04x}) at stream byte {byte_offset:#x} has no admitted public width",
        wire_opcode & 0x3f
    )]
    UnknownRdpOpcode {
        stream: RawStreamKind,
        byte_offset: u32,
        wire_opcode: u8,
    },
    #[error(
        "{stream:?} raw RDP command at stream byte {byte_offset:#x} needs {width} bytes but stream ends at {stream_bytes:#x}"
    )]
    TruncatedRdpCommand {
        stream: RawStreamKind,
        byte_offset: u32,
        width: u32,
        stream_bytes: u32,
    },
    #[error("resource purpose {purpose} rejects access mode {mode}")]
    InvalidAccessMode {
        purpose: &'static str,
        mode: &'static str,
    },
    #[error("resource purpose {purpose} rejects resource class {resource}")]
    InvalidAccessResource {
        purpose: &'static str,
        resource: &'static str,
    },
    #[error("resource journal limit {field} is zero")]
    ZeroJournalLimit {
        field: &'static str,
    },
    #[error("resource journal limit {field}={actual} exceeds hard bound {maximum}")]
    JournalLimitTooLarge {
        field: &'static str,
        actual: u64,
        maximum: u64,
    },
    #[error("workload resource journal is empty")]
    EmptyResourceJournal,
    #[error("resource journal has {actual} accesses; configured bound is {maximum}")]
    TooManyResourceAccesses {
        actual: usize,
        maximum: usize,
    },
    #[error("declared resource byte count overflowed")]
    DeclaredResourceBytesOverflow,
    #[error("resource journal declares {actual} bytes; configured bound is {maximum}")]
    DeclaredResourceBytesExceeded {
        actual: u64,
        maximum: u64,
    },
    #[error("workload contains no raw command streams")]
    EmptyWorkload,
    #[error("workload contains {actual} raw streams; hard bound is {maximum}")]
    TooManyPacketStreams {
        actual: usize,
        maximum: usize,
    },
    #[error(
        "workload range does not retain the packet's exact {expected:#x}-byte installed-memory layout"
    )]
    MemoryLayoutMismatch {
        expected: u32,
    },
    #[error("workload owns {actual} command bytes; aggregate bound is {maximum}")]
    PacketCommandBytesExceeded {
        actual: usize,
        maximum: usize,
    },
    #[error("workload owns {actual} command chunks; aggregate bound is {maximum}")]
    PacketCommandChunksExceeded {
        actual: usize,
        maximum: usize,
    },
    #[error("workload owns {actual} temporal events; aggregate bound is {maximum}")]
    PacketTimelineEventsExceeded {
        actual: usize,
        maximum: usize,
    },
    #[error("packet-global event sequence is not strictly increasing: {prior} then {next}")]
    NonMonotonicPacketEventSequence {
        prior: u64,
        next: u64,
    },
    #[error(
        "workload journal lacks an exact CommandDecode read for {stream:?} [{start:#010x}, {end:#010x})"
    )]
    MissingCommandReadDeclaration {
        stream: RawStreamKind,
        start: u32,
        end: u32,
    },
    #[error(
        "workload journal CommandDecode access {access_index} for {stream:?} [{start:#010x}, {end:#010x}) has no one-to-one stream owner"
    )]
    UnmatchedCommandReadDeclaration {
        access_index: usize,
        stream: RawStreamKind,
        start: u32,
        end: u32,
    },
    #[error(
        "journal TmemLoadSource access {access_index} (operation {operation}) is not an RDRAM range and cannot cross the ABI guest-memory boundary"
    )]
    DeferredGuestReadUnsupportedRegion {
        access_index: usize,
        operation: u32,
    },
    #[error(
        "deferred guest-read command-moment list contains {actual} entries; exact plan requires {expected}"
    )]
    GuestReadMomentCountMismatch {
        expected: usize,
        actual: usize,
    },
    #[error(
        "deferred guest-read command-moment entry {index} does not match the exact ordered journal access/operation"
    )]
    GuestReadMomentDescriptorMismatch {
        index: usize,
    },
    #[error(
        "deferred guest-read storage layout length {bytes} is not aligned to its {alignment}-byte native word mapping"
    )]
    GuestReadStorageLayoutUnaligned {
        bytes: u32,
        alignment: u32,
    },
    #[error(
        "deferred guest-read plan does not belong to the packet's exact memory layout and resource journal"
    )]
    GuestReadPlanMismatch,
    #[error("deferred guest-read capture contains {actual} entries; exact plan requires {expected}")]
    GuestReadCountMismatch {
        expected: usize,
        actual: usize,
    },
    #[error(
        "deferred guest-read capture entry {index} does not match the exact ordered plan operation/range"
    )]
    GuestReadDescriptorMismatch {
        index: usize,
    },
    #[error("deferred guest-read capture entry {index} owns {actual} bytes; exact range requires {expected}")]
    GuestReadByteCountMismatch {
        index: usize,
        expected: u32,
        actual: u32,
    },
    #[error(
        "deferred guest-read capture owns {actual} aggregate bytes; exact plan requires {expected}"
    )]
    GuestReadAggregateByteCountMismatch {
        expected: u64,
        actual: u64,
    },
    #[error("deferred guest-read capture entry {index} content digest does not match its owned bytes")]
    GuestReadDigestMismatch {
        index: usize,
    },
    #[error("workload replay requires an owned deferred guest-read capture with {count} entries")]
    ReplayGuestReadCaptureRequired {
        count: usize,
    },
    #[error(
        "replayed deferred guest-read set does not match the record identity/content digests"
    )]
    ReplayGuestReadSetMismatch,
    #[error("ticket role authority identity space is exhausted")]
    TicketAuthorityExhausted,
    #[error("submission ordinal space is exhausted for queue {queue}")]
    SubmissionOrdinalExhausted {
        queue: u64,
    },
    #[error("receipt was issued by a different lifecycle role authority")]
    ReceiptAuthorityMismatch,
    #[error("receipt effects do not match the active workload or backend effects")]
    ReceiptEffectMismatch,
    #[error("guest memory no longer matches the exact captured transaction preimage")]
    GuestMemoryPreimageMismatch,
    #[error("a completed write cannot name a read-only resource access")]
    EffectForReadOnlyAccess,
    #[error(
        "single guest render-target write requires access mode Write and purpose RenderTarget; supplied write has mode {mode} and purpose {purpose}"
    )]
    GuestRenderTargetWriteShapeMismatch {
        mode: &'static str,
        purpose: &'static str,
    },
    #[error("completed write reports {actual} bytes; declared region requires {expected}")]
    EffectByteCountMismatch {
        expected: u32,
        actual: u32,
    },
    #[error("{field} count is {actual}; exact journal requires {expected}")]
    EffectCountMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{field} {index} does not match the exact ordered journal access/effect")]
    EffectAccessMismatch {
        field: &'static str,
        index: usize,
    },
    #[error("receipt workload {actual} does not match active workload {expected}")]
    ReceiptWorkloadMismatch {
        expected: WorkloadIdentity,
        actual: WorkloadIdentity,
    },
    #[error("completion receipt names a different submission")]
    ReceiptSubmissionMismatch,
    #[error("receipt resource journal identity does not match the active workload")]
    ReceiptJournalMismatch,
    #[error("workload record magic is not fn64.render-ir.record.v3")]
    RecordMagic,
    #[error("workload record version {actual} is unsupported")]
    RecordVersion {
        actual: u16,
    },
    #[error("workload record ended while decoding {field}")]
    RecordTruncated {
        field: &'static str,
    },
    #[error("workload record {field} has invalid tag {tag}")]
    RecordInvalidTag {
        field: &'static str,
        tag: u8,
    },
    #[error("workload record {field} is invalid: {reason}")]
    RecordInvalidField {
        field: &'static str,
        reason: String,
    },
    #[error("workload record has {bytes} trailing bytes")]
    RecordTrailingBytes {
        bytes: usize,
    },
    #[error("workload record SHA-256 integrity digest does not match its body")]
    RecordIntegrityMismatch,
    #[error("workload record has {actual} bytes; metadata bound is {maximum}")]
    RecordTooLarge {
        actual: usize,
        maximum: usize,
    },
    #[error("replayed workload identity {actual} does not match record identity {expected}")]
    RecordIdentityMismatch {
        expected: WorkloadIdentity,
        actual: WorkloadIdentity,
    },
    #[error("replay supplied {actual} streams; record requires {expected}")]
    ReplayStreamCount {
        expected: usize,
        actual: usize,
    },
    #[error("replay stream {index} does not match its recorded source/content identity")]
    ReplayStreamMismatch {
        index: usize,
    },
    #[error("numeric field {field} overflowed its canonical representation")]
    NumericOverflow {
        field: &'static str,
    },
}


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
