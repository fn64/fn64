use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendCompletionAuthority, CommandCompletionMoment, CompletedWrite,
    DeferredGuestReadCapture, DeferredGuestReadPlan, GpuCompleteTicket, GuestCommitAuthority,
    GuestCommitEffectReport, GuestCommitReceipt, GuestCommittedTicket, GuestReadCommandMoment,
    JournalIdentity, QueueIdentity, ResourceAccess, SubmissionIdentity, SubmissionQueue,
    TicketAuthoritySet, TmemRange, ValidationError,
};

use crate::RawDpcSubmissionIdentity;

use super::{preflight_raw_dpc_capture_with_guest_read_command_moments, IrRawDpcPacketPreflight};

mod capsule;
mod commands;
mod execute;
mod neutral;
mod retirement;
mod session;

pub use capsule::*;
pub use commands::*;
pub use execute::*;
pub use neutral::*;
pub use retirement::*;
pub use session::*;

#[cfg(test)]
#[path = "production_tests.rs"]
mod tests;
