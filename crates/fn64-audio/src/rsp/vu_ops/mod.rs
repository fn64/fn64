//! Per-family VU op bodies. Each op group lives in its OWN submodule here (to
//! keep parallel implementation work collision-free); the dispatcher in
//! [`super::ops`] routes each [`super::ops::VuOp`] to the matching body.
//!
//! Submodules:
//! - [`logic`] — the bitwise family `VAND`/`VNAND`/`VOR`/`VNOR`/`VXOR`/`VNXOR`
//!   plus `VNOP` (RSP-VU-ISA.md §6.5).

pub mod logic;
