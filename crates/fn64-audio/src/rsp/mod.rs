//! The clean-room RSP scalar/VU execution framework the audio ucode runs on.
//!
//! This is the FOUNDATION the 47 VU op bodies build on. It replaces BOTH GPL
//! headers the RSPRecomp-generated audio-ucode C `#include`s
//! (`librecomp/rsp.hpp` + `librecomp/rsp_vu_impl.hpp`, `rsp_recomp.cpp` lines
//! 1179-1180) with our own clean-room, spec-derived implementation, per
//! `RSP-VU-ISA.md`. No GPL implementation was read.
//!
//! Modules:
//! - [`dmem`] — the 0x1000-byte RSP DMEM + the `RSP_MEM_*` swizzled accessors
//!   (the `^2`/`^3` byte-lane XOR, same pattern as fn64-runtime's rdram).
//! - [`context`] — [`RspContext`] (scalar regs r1..r31, DMA addrs,
//!   jump/resume, [`RspExitReason`]), matching the generated
//!   `RspExitReason(rdram, ucode_addr)` signature.
//! - [`vu`] — [`VuState`]: the 32×8-lane register file, the 48-bit
//!   accumulator, VCO/VCC/VCE flags, the div latch, element-select, and the
//!   clamp helpers. **This is the API the op impls call.**
//! - [`tables`] — the generated VRCP/VRSQ 512-entry seed ROMs (with
//!   spot-check tests against known hardware entries).
//! - [`ops`] — the op enum + operand-shape descriptors + dispatch skeleton the
//!   ops phase fills in.
//!
//! Everything is portable scalar Rust (`i16`/`i32`/`i64` lanes), no SIMD.

pub mod context;
pub mod dmem;
pub mod ops;
pub mod tables;
pub mod vu;
pub mod vu_ops;

pub use context::{RspContext, RspExitReason};
pub use dmem::{Dmem, DMEM_MASK, DMEM_SIZE};
pub use ops::{dispatch, operand_shape, OpInvocation, OpStatus, OperandRole, OperandShape, VuOp};
pub use tables::{rcp_rom, rcp_seed, rsq_rom, rsq_seed, RCP_ROM_LEN, RSQ_ROM_LEN};
pub use vu::{
    clamp_signed, clamp_unsigned, clamp_unsigned_low, element_select, element_source,
    scalar_source_lane, Accumulator, Flags, Vec8, VuRegs, VuState, LANES, NUM_VREGS,
};
