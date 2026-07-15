//! Per-family VU op bodies. Each op group lives in its OWN submodule here (to
//! keep parallel implementation work collision-free); the dispatcher in
//! [`super::ops`] routes each [`super::ops::VuOp`] to the matching body.
//!
//! Submodules:
//! - [`logic`] — the bitwise family `VAND`/`VNAND`/`VOR`/`VNOR`/`VXOR`/`VNXOR`
//!   plus `VNOP` (RSP-VU-ISA.md §6.5).
//! - [`mac`] — the multiply-accumulate family `VMACF`/`VMACQ`/`VMADH`/`VMADM`/
//!   `VMADN`/`VMADL` plus the accumulator reader `VSAR` (RSP-VU-ISA.md §6.2,
//!   §6.9).
//! - [`select`] — the compares/merge/clip family `VLT`/`VEQ`/`VNE`/`VGE`/
//!   `VMRG`/`VCH`/`VCL`/`VCR` plus the VCC/VCO/VCE register-slice accessors
//!   (RSP-VU-ISA.md §6.6–§6.8).

pub mod logic;
pub mod mac;
pub mod mul_hi;
pub mod select;
