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
mod guest_read;
mod journal;
mod record;
mod rsp_math;
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
    ContentDigest, EffectIdentity, FastContentDigest, GuestReadPlanIdentity, GuestReadSetIdentity,
    JournalIdentity, RawStreamIdentity, RecordIdentity, WorkloadIdentity,
};
pub use error::ValidationError;
pub use guest_read::{
    CapturedGuestRead, DeferredGuestRead, DeferredGuestReadCapture, DeferredGuestReadPlan,
    OwnedGuestReadSet,
};
pub use journal::{
    AccessMode, AccessPurpose, HostResource, OperationId, RdramResource, ResourceAccess,
    ResourceJournal, ResourceJournalLimits, ResourceRegion, MAX_DECLARED_RESOURCE_BYTES,
    MAX_RESOURCE_ACCESSES,
};
pub use record::{
    RawStreamRecord, WorkloadRecord, MAX_WORKLOAD_RECORD_BYTES, WORKLOAD_RECORD_SCHEMA,
};
pub use rsp_math::{
    compute_attenuation, compute_dir_light, compute_length, compute_n_dot_l, compute_pos_light,
    Mat4, RspFog, RspLight, RspLookAt, RspVertexTestZCb, RspViewport, Vec3, Vec4,
    RSP_LOOKAT_INDEX_ENABLED, RSP_LOOKAT_INDEX_LINEAR, RSP_LOOKAT_INDEX_SHIFT,
};
pub use ticket::{
    effect_content_digest, BackendCompletionAuthority, BackendEffectReport, CompletedWrite,
    DecodedTicket, GpuCompleteTicket, GpuCompletionReceipt, GuestCommitAuthority,
    GuestCommitEffectReport, GuestCommitReceipt, GuestCommittedTicket, QueueIdentity,
    ReadySubmissionQueue, SubmissionIdentity, SubmissionQueue, SubmittedTicket, TicketAuthoritySet,
};
pub use workload::{
    MicrocodeAdmissionIdentity, WorkloadAdmission, WorkloadPacket, WorkloadPacketPreflight,
    MAX_PACKET_COMMAND_BYTES, MAX_PACKET_COMMAND_CHUNKS, MAX_PACKET_STREAMS,
    MAX_PACKET_TIMELINE_EVENTS,
};
