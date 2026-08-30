//! Boot-selected ownership of libultra interrupt delivery.
//!
//! A host-kernel boot translates a committed hardware interrupt into the
//! registered host-side `OS_EVENT_*` message path. A guest-kernel boot enters
//! the guest exception handler, which owns that message delivery instead.
//! Those routes cannot be active together: one move-only value selects the
//! owner when the executor is constructed, and there is no replacement API.
//!
//! This module establishes the ownership type only. Production still creates
//! [`HostKernel`] exclusively, and the release-gate encoder does not yet bind
//! the selection. Guest admission, interrupt-route branching, and release
//! evidence must land together so an evidence label cannot precede behavior.

/// Host libultra services own interrupt-to-message delivery.
///
/// The private field prevents code outside this module from minting a second
/// authority independently of [`KernelAuthority`].
#[derive(Debug, PartialEq, Eq)]
pub struct HostKernel {
    _private: (),
}

/// Guest exception code owns interrupt-to-message delivery.
///
/// This type is modeled for the block-runner migration, but no public
/// constructor admits it to production yet.
#[derive(Debug, PartialEq, Eq)]
pub struct GuestKernel {
    _private: (),
}

/// Move-only boot selection for the one kernel that owns interrupt delivery.
///
/// The payload types have no public constructors, so callers cannot forge a
/// variant directly. This type deliberately implements neither `Copy` nor
/// `Clone`: the executor consumes the choice and retains it for its lifetime.
#[derive(Debug, PartialEq, Eq)]
pub enum KernelAuthority {
    HostKernel(HostKernel),
    GuestKernel(GuestKernel),
}

/// Pointer-free evidence for the executor's immutable kernel selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KernelAuthorityEvidenceSnapshot {
    HostKernel,
    GuestKernel,
}

impl KernelAuthority {
    pub(super) const fn host_kernel() -> Self {
        Self::HostKernel(HostKernel { _private: () })
    }

    #[cfg(test)]
    pub(super) const fn guest_kernel() -> Self {
        Self::GuestKernel(GuestKernel { _private: () })
    }

    pub const fn evidence_snapshot(&self) -> KernelAuthorityEvidenceSnapshot {
        match self {
            Self::HostKernel(_) => KernelAuthorityEvidenceSnapshot::HostKernel,
            Self::GuestKernel(_) => KernelAuthorityEvidenceSnapshot::GuestKernel,
        }
    }
}

impl Default for KernelAuthority {
    fn default() -> Self {
        Self::host_kernel()
    }
}
