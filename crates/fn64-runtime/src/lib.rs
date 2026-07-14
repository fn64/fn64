//! fn64-runtime: the core, pure-Rust half of fn64.
//!
//! See `docs/DESIGN.md` (workspace root) sections 1-3 for the architecture
//! this crate implements: the rdram ownership model, the `RdramAddr`
//! translation newtype, and `OSMesgQueue` semantics. This crate has zero
//! knowledge of `fn64-abi`'s extern "C" surface or `fn64-rt64`'s C++
//! interop — it is deliberately the independently-testable core.
//!
//! Design provenance for every non-obvious semantic choice below is cited
//! inline; see `docs/DESIGN.md` section 6 for the full provenance table.

pub mod mesgqueue;
pub mod rdram;

pub use mesgqueue::{Mesg, MesgQueue, RecvResult, SendResult};
pub use rdram::{Rdram, RdramAddr};
