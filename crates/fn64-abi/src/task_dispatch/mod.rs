//! RSP/RDP task dispatch: microcode admission, interpreter phase management,
//! RDRAM commit, DPC transactions, audio/render backend wiring, and task
//! lifecycle. Split into concern submodules; items are re-exported here so
//! `crate::task_dispatch::X` and cross-submodule `use super::*` both resolve.

use super::*;
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

mod setup;
mod rsp_lineage;
mod rsp_phase;
mod rsp_commit;
mod lifecycle;

pub use lifecycle::*;
pub use rsp_commit::*;
pub use rsp_lineage::*;
pub use rsp_phase::*;
pub(crate) use setup::*;

#[cfg(test)]
mod tests;
