#![allow(clippy::all, unused, unused_comparisons)]

use fn64_recomp_rs::{
    BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuException, CpuFault,
    CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramError,
    Rdram, RecompContext,
};

include!(concat!(env!("OUT_DIR"), "/runner.rs"));
include!(concat!(env!("OUT_DIR"), "/metadata.rs"));

/// Recover this shard's instruction words from the user's ROM.
///
/// The words are no longer baked into the artifact; `ROM_START`/`ROM_END` are
/// geometry that locates them in the normalized big-endian image the host
/// published at startup. Correctness of the recovered bytes is proven by the
/// caller: `block_program.rs` asserts
/// `code_bank_sha256(&code_bank) == expected.code_sha256` against the digest
/// `build.rs` derived from the ROM at build time. That assertion already
/// existed and is what makes this substitution safe rather than merely
/// smaller -- a wrong or truncated ROM fails there, loudly.
pub fn code_bank() -> CodeBank {
    let words = fn64_recomp_rs::shard_words(ROM_START, ROM_END).unwrap_or_else(|error| {
        panic!("dense AOT shard {BANK_ID:#018X} cannot recover its words from the user's ROM: {error}")
    });
    CodeBank::new(BankId::new(BANK_ID), GuestPc::new(VA_START), words)
        .expect("generated dense AOT shard is valid")
}
