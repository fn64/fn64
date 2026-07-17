//! `fn64-diff`: the first-divergence comparator (§4's comparator lane).
//!
//! Given fn64's own claimed register state at a checkpoint PC and a
//! reference runtime's ground-truth state at that same PC, report the FIRST
//! field that disagrees -- not a fuzzy or aggregate score. That is this
//! crate's whole job. It is pure: it acquires nothing, spawns nothing, and
//! parses no file formats. Feeding it is the caller's problem.
//!
//! ## What used to be here, and why it is gone (do not re-add it)
//!
//! This crate once also carried a subprocess client for the *faki-tools*
//! `oracle` CLI and a parser for **mupen64plus**'s `.m64p` savestate format,
//! to drive an instruction-exact "state transplant" path: load a reference
//! savestate, transplant its RDRAM+registers into a fresh executor, resume at
//! the saved PC, and lockstep forward against the reference.
//!
//! Both were removed 2026-07-17:
//!
//! - The oracle client was a client for **another project's command line**.
//!   fn64's build graph does not carry those -- see `docs/DESIGN.md` §1.0,
//!   "fn64 owns its toolchain" ("a path to another project is not a
//!   dependency mechanism").
//! - The savestate parser fed a path that **cannot work**. Instruction-exact
//!   transplant is not representable against a recompiler-shaped runtime:
//!   `SectionRegistry::resolve` (`fn64-runtime/src/overlay.rs`) matches only
//!   EXACT function-entry offsets by design, and a snapshot's PC lands
//!   mid-function essentially always. The full finding, with the two things
//!   that would be required to lift the wall, is recorded in
//!   `docs/DESIGN.md` §1.0. **Read it before concluding a savestate parser is
//!   a missing feature here.** It is not a gap; it is a wall.
//!
//! The consequence the comparator is built around: the unit of comparison can
//! only be a **checkpoint PC reached by whole-function execution**, never a
//! single MIPS instruction. [`lockstep`] says so in its own types.
//!
//! The historical end-to-end run those modules produced is preserved in
//! `docs/2026-07-14-first-divergence-report.md` (cited from
//! `fn64-abi/src/mesgqueue.rs` as the provenance of a real coroutine-context
//! -corruption bug).

pub mod lockstep;

pub use lockstep::{
    compare_checkpoint, CheckpointResult, FieldDiff, Fn64Checkpoint, LockstepReport,
    RegisterSnapshot, GPR_NAMES,
};
