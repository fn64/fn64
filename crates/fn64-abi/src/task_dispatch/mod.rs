//! RSP/RDP task dispatch: microcode admission, interpreter phase management,
//! RDRAM commit, DPC transactions, audio/render backend wiring, and task
//! lifecycle. Split into concern submodules; items are re-exported here so
//! `crate::task_dispatch::X` and cross-submodule `use super::*` both resolve.
//!
//! # The audio-priority bounded VI join
//!
//! One contract in here is load-bearing enough to name. Under audio-priority
//! presentation ([`set_audio_priority_vi_presentation`]) a VI edge that finds
//! the raw-DPC renderer worker still running does **not** block on it:
//!
//! 1. [`try_advance_async_lle_render_task`] waits at most
//!    `audio_priority_join_budget()` -- the host-installed
//!    [`set_audio_priority_join_budget_ms`], else 3ms -- for the worker,
//!    through `ThreadedRenderBackend::poll_raw_dpc_task_batch_bounded`'s
//!    `recv_timeout`.
//! 2. On timeout it leaves the batch running, increments `VI_JOIN_SKIPS`
//!    (readable as [`audio_priority_vi_join_skips`]), and returns `true`.
//! 3. `present_render_backend` then takes its early return, so
//!    `RenderBackend::present` is never called for that retrace and no new
//!    `PresentedSourceFieldGeneration` is minted: the host keeps displaying
//!    the field it already owns. RDRAM still holds the prior completed frame
//!    (batch results commit only at the join, on this thread), so the skipped
//!    join is a clean frame re-present rather than a tear.
//!
//! Why it exists: blocking the pump at a VI edge stalls guest time itself,
//! including audio production, which starved the output ring during
//! render-heavy scenes (~50k underrun sample slots per second). Hardware has
//! no such coupling -- a slow RDP delays only the game's own DP-completion
//! wait while VI keeps scanning the previous framebuffer.
//!
//! Note that the counter and the re-present live in **different functions**:
//! the increment is in `lifecycle.rs`, the re-present is the early return in
//! `setup.rs`'s `present_render_backend`.
//!
//! The contract is pinned end to end by
//! `tests::raw_dpc_session_integration::audio_priority_join`, whose two tests
//! drive the real threaded worker and the real production retrace path:
//! `a_vi_edge_whose_renderer_has_not_replied_skips_within_budget_and_re_presents`
//! (bounded, one skip, byte-identical field) and
//! `a_vi_edge_whose_renderer_replied_in_budget_joins_with_no_skip_and_presents_the_new_field`
//! (zero skips, a strictly later field generation).

use super::*;
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

mod lifecycle;
mod rsp_commit;
mod rsp_lineage;
mod rsp_phase;
mod setup;

pub use lifecycle::*;
pub use rsp_commit::*;
pub use rsp_lineage::*;
pub use rsp_phase::*;
pub(crate) use setup::*;

#[cfg(test)]
mod tests;
