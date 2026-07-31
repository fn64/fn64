//! Dynamic-MIPS adapter over the lane-neutral semantic kernel.

#[cfg(feature = "dev-interpreter")]
pub use crate::semantic::{
    run_bank, run_bank_with_mmio, MmioOutcome, MmioPort, NoMmio, UnsupportedOp,
};

pub(crate) use crate::semantic::run_instruction_unit;
