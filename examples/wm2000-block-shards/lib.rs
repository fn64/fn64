#![allow(clippy::all, unused, unused_comparisons)]

use fn64_recomp_rs::{
    BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuException, CpuFault,
    CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError,
    Rdram, RecompContext,
};

include!(concat!(env!("OUT_DIR"), "/runner.rs"));
include!(concat!(env!("OUT_DIR"), "/metadata.rs"));

pub fn code_bank() -> CodeBank {
    CodeBank::new(BankId::new(BANK_ID), GuestPc::new(VA_START), WORDS.to_vec())
        .expect("generated dense AOT shard is valid")
}
