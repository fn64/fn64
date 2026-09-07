//! Mechanical host-binding discovery for public resident libultra routines.
//!
//! Addresses are outputs, never signatures. The recognizers below describe
//! public ABI behavior in register/field terms: `osCreateMesgQueue` initializes
//! the documented six-word queue, `osCreateThread` initializes the public
//! `OSThread` linkage, identity, state, context, and o32 stack-supplied
//! priority fields, `osEPiStartDma` validates the manager and writes the
//! request type/handle into `OSIoMesg` -- recognized by that ABI behavior so
//! that builds keeping the arguments register-resident and builds spilling
//! every argument to the frame are both matched, `osSendMesg` inserts at `(first +
//! validCount) % msgCount`, and the overlay helper calls the DMA routine in a
//! retry loop before a blocking receive on its stack queue.
//! The RSP task recognizers likewise follow the public task-load, start, yield,
//! and yielded-query register/field behavior. `osSetEventMesg` scales the event
//! selector by the documented eight-byte `OSEventState` stride and stores the
//! queue and message through the resulting entry, between an interrupt disable
//! and its matching restore. Timer discovery follows the public o32
//! `osSetTimer` arguments and `OSTimer` fields, resolving the stack-passed
//! arguments relative to the callee's own frame so that builds which inline the
//! list walk and builds which delegate it are both recognized. Every role must
//! have one unique structural match or discovery fails loudly.
//!
//! Split by concern: [`core`] holds the shared decode primitives and public
//! vocabulary; [`mesg`], [`thread`], [`sp_task`], and [`timer`] hold the
//! per-role structural recognizers; [`io`] holds device-I/O recognizers and
//! the `discover_*` entry points that assemble a title's catalog; [`probe`]
//! resolves each catalog role independently so one unresolved role does not
//! fail the whole discovery; [`external_reference`] is the validated
//! external-symbol fallback; [`flash`] is the flash-cartridge and
//! programmed-IO path. Every name below was `pub` (or already re-exported by
//! external callers) before the split and keeps the same `crate::host_bindings::`
//! path.

mod core;
mod external_reference;
mod flash;
mod io;
mod mesg;
mod probe;
mod sp_task;
mod thread;
mod timer;

pub use core::{
    discover_guest_thread_globals, GuestThreadGlobals, HostBinding, HostBindingDiscoveryError,
    HostBindingSymbol, HostCurrentStatusEffect, HostSpawnedStatusEffect, OsPiCandidateLimitKind,
    OsPiDeviceBasePrerequisite, OsPiStartDmaCandidateClassification,
    OsPiStartDmaCandidateOpenReason, OsPiStartDmaShapeCandidate, FLASH_HOST_SYMBOLS,
    PROGRAMMED_IO_HOST_SYMBOLS, WM_BLOCK_RUNTIME_HOST_SYMBOLS,
};
pub use external_reference::{
    discover_wm_block_runtime_host_bindings_with_external_reference, ExternalSymbolTable,
    ResolutionProvenance, ResolvedHostBinding,
};
pub use flash::{discover_flash_host_bindings, discover_programmed_io_host_bindings};
pub use io::{
    discover_drive_rom_init_host_binding, discover_os_create_thread_host_binding,
    discover_overlay_loader_host_bindings, discover_rsp_task_host_bindings,
    discover_si_device_busy_host_binding, discover_timer_host_bindings,
    discover_wm_block_runtime_host_bindings, DriveRomInitBinding,
};
pub use mesg::classify_os_pi_start_dma_candidate;
pub use probe::{probe_wm_block_runtime_host_bindings, HostBindingProbeOutcome};

// Test-only bridge: `tests.rs` predates the concern split and reaches every
// module's internals through `use super::*` as though it were still one
// file. Bringing them into scope here (rather than rewriting tests.rs's few
// hundred call sites to `super::core::`, `super::mesg::`, ...) keeps the
// split behavior-preserving: no test changed what it exercises or how.
// Plain (non-`pub`) `use`: `tests` is a child module of this one, and a
// child sees everything its parent's namespace holds, `use`d items included.
#[cfg(test)]
use crate::cfg::{Cfg, WordClass};
#[cfg(test)]
use crate::facts::FactDb;
#[cfg(test)]
use core::{
    collapse_overlapping_runs, imm, is_addiu, is_andi, is_beq, is_jr_ra, is_lui, is_lw_at,
    is_move_addu, is_sw, jal_field, op, rs,
};
#[cfg(test)]
use mesg::{
    classify_os_pi_start_dma_candidate_with_limits, is_create_mesg_queue, OsPiShapeLimits,
    DEFAULT_PI_SHAPE_LIMITS,
};
#[cfg(test)]
use sp_task::is_sp_task_load;
#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;
