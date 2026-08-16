//! GPU-independent semantic ownership for fn64 renderers.
//!
//! This crate owns bounded physical regions, immutable raw-command streams,
//! explicit resource effects, content identities, and the move-only workload
//! lifecycle. It deliberately knows nothing about wgpu, windows, queues,
//! guest schedulers, or foreign renderer handles.
//!
//! Serialized [`WorkloadRecord`] values contain identities and bounded
//! semantic metadata only. Command payloads stay in the owned packet or in a
//! caller-controlled fixture and must be supplied again for replay.
#![forbid(unsafe_code)]

mod address;
mod command;
mod digest;
mod error;
mod journal;
mod record;
mod ticket;
mod workload;

pub use address::{
    DmemRange, PhysicalAddress, PhysicalMemoryLayout, PhysicalRange, TmemRange,
    RDP_PHYSICAL_ADDRESS_BYTES, RSP_DMEM_BYTES, TMEM_BYTES,
};
pub use command::{
    CmdEndOccurrence, DpInterruptObservation, DpInterruptState, DramCommandChunk,
    DramCommandStream, FullSyncBoundary, FullSyncOccurrence, RawCommandStream, RawStreamKind,
    RawTimelineEvent, TemporalBoundary, XbusCommandChunk, XbusCommandStream, MAX_COMMAND_CHUNKS,
    MAX_RAW_STREAM_BYTES,
};
pub use digest::{
    ContentDigest, EffectIdentity, JournalIdentity, RawStreamIdentity, RecordIdentity,
    WorkloadIdentity,
};
pub use error::ValidationError;
pub use journal::{
    AccessMode, AccessPurpose, HostResource, OperationId, RdramResource, ResourceAccess,
    ResourceJournal, ResourceJournalLimits, ResourceRegion, MAX_DECLARED_RESOURCE_BYTES,
    MAX_RESOURCE_ACCESSES,
};
pub use record::{
    RawStreamRecord, WorkloadRecord, MAX_WORKLOAD_RECORD_BYTES, WORKLOAD_RECORD_SCHEMA,
};
pub use ticket::{
    BackendCompletionAuthority, BackendEffectReport, CompletedWrite, DecodedTicket,
    GpuCompleteTicket, GpuCompletionReceipt, GuestCommitAuthority, GuestCommitEffectReport,
    GuestCommitReceipt, GuestCommittedTicket, QueueIdentity, SubmissionIdentity, SubmissionQueue,
    SubmittedTicket, TicketAuthoritySet,
};
pub use workload::{
    MicrocodeAdmissionIdentity, WorkloadAdmission, WorkloadPacket, MAX_PACKET_COMMAND_BYTES,
    MAX_PACKET_COMMAND_CHUNKS, MAX_PACKET_STREAMS, MAX_PACKET_TIMELINE_EVENTS,
};
