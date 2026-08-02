//! Experimental shared executor for canonical `static-micro-op` V1/V2 packs.
//!
//! This is a differential-development lane, not `production-aot` authority.
//! Its shared straight-instruction path executes the lane-neutral semantic
//! kernel used by dynamic MIPS execution. Control-pair execution remains the
//! deliberately narrow NOP/ADDIU-delay BEQ/BEQL slice. Live-word verification intentionally uses
//! [`crate::verify_precompiled_instruction_word`], exactly like the current
//! dense verified shards. That helper reads the direct RDRAM view; mapped/TLB
//! instruction-fetch aliases require the physical fetch identity path before
//! this lane can be promoted beyond dense-lane replacement experiments.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::execution::{
    verify_precompiled_instruction_word, BankId, BlockExit, BlockRun, CpuException, CpuFault,
    CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget,
};
use crate::runtime::{Rdram, RecompContext};
use crate::semantic::{execute_straight_word, Step, StepFault};
use crate::static_micro_op::{
    StaticMicroOpRecordErrorV1, StaticMicroOpRecordV1, STATIC_MICRO_OP_HEADER_V1_BYTES,
    STATIC_MICRO_OP_MAGIC_V1, STATIC_MICRO_OP_MAGIC_V2,
    STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1, STATIC_MICRO_OP_PACK_SCHEMA_V1,
    STATIC_MICRO_OP_PACK_SCHEMA_V2, STATIC_MICRO_OP_RECORD_V1_BYTES,
    STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES, STATIC_MICRO_OP_SPAN_HEADER_V2_BYTES,
};

pub const STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V1: &str =
    "fn64.static-micro-op-executor-source.v1";
pub const STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V2: &str =
    "fn64.static-micro-op-executor-source.v2";
pub const STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V3: &str =
    "fn64.static-micro-op-executor-source.v3";
pub const STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V1: &str =
    "fn64.experimental-predecoded-aot-build.v1";
pub const STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V2: &str =
    "fn64.experimental-predecoded-aot-build.v2";
pub const STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V3: &str =
    "fn64.experimental-predecoded-aot-build.v3";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutorSourceReceiptV1 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpExecutorSourceReceiptV1 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V1
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_executor_source_receipt_v1() -> StaticMicroOpExecutorSourceReceiptV1 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        (
            b"src/decoder/dispatch.rs",
            include_bytes!("decoder/dispatch.rs"),
        ),
        (
            b"src/decoder/mod.rs",
            include_bytes!("decoder/mod.rs"),
        ),
        (b"src/execution.rs", include_bytes!("execution.rs")),
        (
            b"src/runtime/fpu_ops.rs",
            include_bytes!("runtime/fpu_ops.rs"),
        ),
        (
            b"src/runtime/host.rs",
            include_bytes!("runtime/host.rs"),
        ),
        (
            b"src/runtime/mod.rs",
            include_bytes!("runtime/mod.rs"),
        ),
        (
            b"src/runtime/tests.rs",
            include_bytes!("runtime/tests.rs"),
        ),
        (
            b"src/static_micro_op.rs",
            include_bytes!("static_micro_op.rs"),
        ),
        (
            b"src/static_micro_op_exec.rs",
            include_bytes!("static_micro_op_exec.rs"),
        ),
        (
            b"src/semantic/mod.rs",
            include_bytes!("semantic/mod.rs"),
        ),
        (
            b"src/semantic/tests.rs",
            include_bytes!("semantic/tests.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-executor-source:v1:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    StaticMicroOpExecutorSourceReceiptV1 {
        source_sha256: hasher.finalize().into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutorSourceReceiptV2 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpExecutorSourceReceiptV2 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V2
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_executor_source_receipt_v2() -> StaticMicroOpExecutorSourceReceiptV2 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        (
            b"src/decoder/dispatch.rs",
            include_bytes!("decoder/dispatch.rs"),
        ),
        (
            b"src/decoder/mod.rs",
            include_bytes!("decoder/mod.rs"),
        ),
        (b"src/execution.rs", include_bytes!("execution.rs")),
        (
            b"src/runtime/fpu_ops.rs",
            include_bytes!("runtime/fpu_ops.rs"),
        ),
        (
            b"src/runtime/host.rs",
            include_bytes!("runtime/host.rs"),
        ),
        (
            b"src/runtime/mod.rs",
            include_bytes!("runtime/mod.rs"),
        ),
        (
            b"src/runtime/tests.rs",
            include_bytes!("runtime/tests.rs"),
        ),
        (
            b"src/static_micro_op.rs",
            include_bytes!("static_micro_op.rs"),
        ),
        (
            b"src/static_micro_op_exec.rs",
            include_bytes!("static_micro_op_exec.rs"),
        ),
        (
            b"src/semantic/mod.rs",
            include_bytes!("semantic/mod.rs"),
        ),
        (
            b"src/semantic/tests.rs",
            include_bytes!("semantic/tests.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-executor-source:v2:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    StaticMicroOpExecutorSourceReceiptV2 {
        source_sha256: hasher.finalize().into(),
    }
}

/// Source-complete identity for the shared static micro-op executor.
///
/// V1 and V2 predate the explicit `fpu.rs` edge. They remain immutable for
/// existing evidence; V3 binds every file that owns decode, execution,
/// floating-point, memory, pack-format, and semantic behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutorSourceReceiptV3 {
    source_sha256: [u8; 32],
}

impl StaticMicroOpExecutorSourceReceiptV3 {
    pub const fn schema(self) -> &'static str {
        STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V3
    }

    pub const fn source_sha256(self) -> [u8; 32] {
        self.source_sha256
    }
}

pub fn static_micro_op_executor_source_receipt_v3() -> StaticMicroOpExecutorSourceReceiptV3 {
    let sources: &[(&[u8], &[u8])] = &[
        (b"Cargo.toml", include_bytes!("../Cargo.toml")),
        (b"src/lib.rs", include_bytes!("lib.rs")),
        (
            b"src/decoder/dispatch.rs",
            include_bytes!("decoder/dispatch.rs"),
        ),
        (
            b"src/decoder/mod.rs",
            include_bytes!("decoder/mod.rs"),
        ),
        (b"src/execution.rs", include_bytes!("execution.rs")),
        (
            b"src/runtime/fpu_ops.rs",
            include_bytes!("runtime/fpu_ops.rs"),
        ),
        (
            b"src/runtime/host.rs",
            include_bytes!("runtime/host.rs"),
        ),
        (
            b"src/runtime/mod.rs",
            include_bytes!("runtime/mod.rs"),
        ),
        (
            b"src/runtime/tests.rs",
            include_bytes!("runtime/tests.rs"),
        ),
        (b"src/fpu.rs", include_bytes!("fpu.rs")),
        (
            b"src/static_micro_op.rs",
            include_bytes!("static_micro_op.rs"),
        ),
        (
            b"src/static_micro_op_exec.rs",
            include_bytes!("static_micro_op_exec.rs"),
        ),
        (
            b"src/semantic/mod.rs",
            include_bytes!("semantic/mod.rs"),
        ),
        (
            b"src/semantic/tests.rs",
            include_bytes!("semantic/tests.rs"),
        ),
    ];
    let mut hasher = Sha256::new();
    hasher.update(b"fn64:static-micro-op-executor-source:v3:");
    for (label, source) in sources {
        hasher.update((label.len() as u64).to_be_bytes());
        hasher.update(label);
        hasher.update((source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    StaticMicroOpExecutorSourceReceiptV3 {
        source_sha256: hasher.finalize().into(),
    }
}

/// Linked-lane receipt. The false production bit is intentional and remains
/// false even when this crate is also compiled with `production-aot` for dense
/// runners: this partial executor cannot issue that authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutionBuildReceiptV1 {
    pub schema: &'static str,
    pub experimental_predecoded_aot: bool,
    pub production_authority: bool,
    pub executor_source: StaticMicroOpExecutorSourceReceiptV1,
}

pub fn static_micro_op_execution_build_receipt_v1() -> StaticMicroOpExecutionBuildReceiptV1 {
    StaticMicroOpExecutionBuildReceiptV1 {
        schema: STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V1,
        experimental_predecoded_aot: true,
        production_authority: false,
        executor_source: static_micro_op_executor_source_receipt_v1(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutionBuildReceiptV2 {
    pub schema: &'static str,
    pub experimental_predecoded_aot: bool,
    pub production_authority: bool,
    pub executor_source: StaticMicroOpExecutorSourceReceiptV2,
}

pub fn static_micro_op_execution_build_receipt_v2() -> StaticMicroOpExecutionBuildReceiptV2 {
    StaticMicroOpExecutionBuildReceiptV2 {
        schema: STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V2,
        experimental_predecoded_aot: true,
        production_authority: false,
        executor_source: static_micro_op_executor_source_receipt_v2(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticMicroOpExecutionBuildReceiptV3 {
    pub schema: &'static str,
    pub experimental_predecoded_aot: bool,
    pub production_authority: bool,
    pub executor_source: StaticMicroOpExecutorSourceReceiptV3,
}

pub fn static_micro_op_execution_build_receipt_v3() -> StaticMicroOpExecutionBuildReceiptV3 {
    StaticMicroOpExecutionBuildReceiptV3 {
        schema: STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V3,
        experimental_predecoded_aot: true,
        production_authority: false,
        executor_source: static_micro_op_executor_source_receipt_v3(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AdmittedSpan {
    bank: BankId,
    vram: u32,
    end: u32,
    records: Vec<StaticMicroOpRecordV1>,
    delay_lookahead: Option<StaticMicroOpRecordV1>,
}

/// Runtime-owned, fully validated view of one canonical pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedStaticMicroOpProgramV1 {
    bytes: Vec<u8>,
    spans: Vec<AdmittedSpan>,
    instruction_count: u64,
    body_sha256: [u8; 32],
}

impl AdmittedStaticMicroOpProgramV1 {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StaticMicroOpPackErrorV1> {
        parse_static_micro_op_pack_v1(bytes)
    }

    pub const fn schema(&self) -> &'static str {
        STATIC_MICRO_OP_PACK_SCHEMA_V1
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn span_count(&self) -> u32 {
        self.spans.len() as u32
    }

    pub const fn instruction_count(&self) -> u64 {
        self.instruction_count
    }

    pub const fn body_sha256(&self) -> [u8; 32] {
        self.body_sha256
    }

    /// Execute the deliberately narrow experimental subset from any admitted
    /// aligned PC. This direct-RDRAM live check has the mapped/TLB limitation
    /// stated in the module documentation.
    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let finish = |exit, instructions| {
            BlockRun::new(
                crate::finalize_executable_write_exit(entry.bank, exit),
                instructions,
            )
        };
        let Some((bank_start, bank_end)) = self.bank_bounds(entry.bank) else {
            return finish(
                BlockExit::Fault(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::UnknownBank,
                }),
                0,
            );
        };
        if !entry.pc.is_instruction_aligned() {
            return finish(
                BlockExit::Fault(CpuFault::instruction_address_error(entry)),
                1,
            );
        }

        let mut pc = entry.pc.get();
        let mut executed = 0u32;
        loop {
            let key = ExecutionKey::new(entry.bank, GuestPc::new(pc));
            let Some(record) = self.record(entry.bank, pc) else {
                return finish(
                    BlockExit::Fault(CpuFault {
                        at: key,
                        kind: CpuFaultKind::UnmappedPc {
                            bank_start,
                            bank_end,
                        },
                    }),
                    executed,
                );
            };
            if let Err(miss) = verify_precompiled_instruction_word(
                entry.bank,
                GuestPc::new(pc),
                record.expected_raw_word,
                mem,
            ) {
                return finish(BlockExit::ImageChanged { at: key, miss }, executed);
            }

            match record.opcode {
                75 | 85 => {
                    let take = branch_equal(record.expected_raw_word, ctx);
                    let delay_pc = pc.wrapping_add(4);
                    let delay = self
                        .delay_record(entry.bank, pc)
                        .expect("admission guarantees every control delay record");
                    if record.opcode != 85 || take {
                        if let Err(miss) = verify_precompiled_instruction_word(
                            entry.bank,
                            GuestPc::new(delay_pc),
                            delay.expected_raw_word,
                            mem,
                        ) {
                            return finish(BlockExit::ImageChanged { at: key, miss }, executed);
                        }
                    }
                    if !budget.can_fit(executed, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS) {
                        return finish(BlockExit::Checkpoint(key), executed);
                    }
                    executed += 2;
                    ctx.advance_cop0_random(1);
                    if record.opcode != 85 || take {
                        if let Some(fault) = execute_delay(entry.bank, pc, delay_pc, delay, ctx) {
                            return finish(BlockExit::Fault(fault), executed);
                        }
                        ctx.advance_cop0_random(1);
                    } else {
                        ctx.advance_cop0_random(1);
                    }
                    let target = branch_target(pc, record.expected_raw_word);
                    if record.opcode == 75 && take && target == pc {
                        return finish(BlockExit::Yield(key), executed);
                    }
                    let next = if take { target } else { pc.wrapping_add(8) };
                    return finish(self.proven_or_resolved(entry.bank, next), executed);
                }
                _ if record.has_delay_slot() => {
                    return finish(
                        BlockExit::Fault(CpuFault {
                            at: key,
                            kind: CpuFaultKind::UnsupportedInstruction {
                                word: record.expected_raw_word,
                            },
                        }),
                        executed + 1,
                    )
                }
                _ => match execute_straight_word(
                    entry.bank,
                    pc,
                    record.expected_raw_word,
                    executed,
                    ctx,
                    mem,
                ) {
                    Ok(Step::Fallthrough { next, retired }) => {
                        debug_assert_eq!(next, pc.wrapping_add(4));
                        executed += retired;
                    }
                    Ok(Step::Exit { exit, retired }) => {
                        return BlockRun::new(
                            crate::finalize_executable_write_exit(entry.bank, exit),
                            executed + retired,
                        );
                    }
                    Err(StepFault::Cpu { fault, attempted }) => {
                        return BlockRun::new(
                            crate::finalize_executable_write_exit(
                                entry.bank,
                                BlockExit::Fault(fault),
                            ),
                            executed + attempted,
                        );
                    }
                    Err(StepFault::Unsupported(op)) => {
                        return BlockRun::new(
                            crate::finalize_executable_write_exit(
                                entry.bank,
                                BlockExit::Fault(op.into_cpu_fault()),
                            ),
                            executed + 1,
                        );
                    }
                },
            }

            let next = pc.wrapping_add(4);
            let may_continue = self.record(entry.bank, next).is_some();
            if let Some(exit) = crate::post_straight_instruction_exit(
                entry.bank,
                GuestPc::new(next),
                executed,
                budget,
                may_continue,
            ) {
                return finish(exit, executed);
            }
            if may_continue {
                pc = next;
            } else {
                return finish(
                    BlockExit::ResolveTransfer {
                        source_bank: entry.bank,
                        target_pc: GuestPc::new(next),
                    },
                    executed,
                );
            }
        }
    }

    fn record(&self, bank: BankId, pc: u32) -> Option<StaticMicroOpRecordV1> {
        let index = self
            .spans
            .partition_point(|span| (span.bank, span.vram) <= (bank, pc))
            .checked_sub(1)?;
        let span = &self.spans[index];
        if span.bank != bank || pc >= span.end || !pc.is_multiple_of(4) {
            return None;
        }
        span.records.get(((pc - span.vram) / 4) as usize).copied()
    }

    fn delay_record(&self, bank: BankId, control_pc: u32) -> Option<StaticMicroOpRecordV1> {
        let delay_pc = control_pc.checked_add(4)?;
        self.record(bank, delay_pc).or_else(|| {
            self.spans
                .iter()
                .find(|span| span.bank == bank && span.end == delay_pc)
                .and_then(|span| span.delay_lookahead)
        })
    }

    fn bank_bounds(&self, bank: BankId) -> Option<(u32, u32)> {
        let mut spans = self.spans.iter().filter(|span| span.bank == bank);
        let first = spans.next()?;
        let mut end = first.end;
        for span in spans {
            end = span.end;
        }
        Some((first.vram, end))
    }

    fn proven_or_resolved(&self, bank: BankId, target: u32) -> BlockExit {
        if self.record(bank, target).is_some() {
            BlockExit::Transfer(ExecutionKey::new(bank, GuestPc::new(target)))
        } else {
            BlockExit::ResolveTransfer {
                source_bank: bank,
                target_pc: GuestPc::new(target),
            }
        }
    }
}

/// V2 program with optional per-span delay-only lookahead records. The
/// lookahead is never returned by owned-PC resolution and therefore cannot be
/// entered directly or enlarge the program's ownership identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedStaticMicroOpProgramV2 {
    core: AdmittedStaticMicroOpProgramV1,
}

impl AdmittedStaticMicroOpProgramV2 {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StaticMicroOpPackErrorV1> {
        parse_static_micro_op_pack_v2(bytes)
    }

    pub const fn schema(&self) -> &'static str {
        STATIC_MICRO_OP_PACK_SCHEMA_V2
    }

    pub fn bytes(&self) -> &[u8] {
        self.core.bytes()
    }

    pub fn span_count(&self) -> u32 {
        self.core.span_count()
    }

    pub const fn instruction_count(&self) -> u64 {
        self.core.instruction_count()
    }

    pub const fn body_sha256(&self) -> [u8; 32] {
        self.core.body_sha256()
    }

    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        self.core.run(entry, budget, ctx, mem)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticMicroOpPackErrorV1 {
    EmptySpan {
        bank: BankId,
        vram: u32,
    },
    UnalignedStart {
        bank: BankId,
        vram: u32,
    },
    AddressOverflow {
        bank: BankId,
        vram: u32,
    },
    OutOfOrder {
        previous_bank: BankId,
        previous_vram: u32,
        bank: BankId,
        vram: u32,
    },
    Overlap {
        bank: BankId,
        previous_end: u32,
        vram: u32,
    },
    CountOverflow,
    CountMismatch {
        header: u64,
        observed: u64,
    },
    InvalidMagic,
    Truncated,
    TrailingBytes,
    DigestMismatch,
    InvalidRecord {
        span_index: u32,
        word_index: u32,
        source: StaticMicroOpRecordErrorV1,
    },
    InvalidLookaheadTag {
        span_index: u32,
        actual: u8,
    },
    UnexpectedDelayLookahead {
        bank: BankId,
        pc: u32,
    },
    MissingDelaySlot {
        bank: BankId,
        pc: u32,
    },
}

impl fmt::Display for StaticMicroOpPackErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid static-micro-op.v1 artifact: {self:?}")
    }
}

impl std::error::Error for StaticMicroOpPackErrorV1 {}

fn parse_static_micro_op_pack_v1(
    bytes: &[u8],
) -> Result<AdmittedStaticMicroOpProgramV1, StaticMicroOpPackErrorV1> {
    if bytes.len() < STATIC_MICRO_OP_HEADER_V1_BYTES {
        return Err(StaticMicroOpPackErrorV1::Truncated);
    }
    if &bytes[..8] != STATIC_MICRO_OP_MAGIC_V1 {
        return Err(StaticMicroOpPackErrorV1::InvalidMagic);
    }
    let span_count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    let instruction_count = u64::from_be_bytes(bytes[12..20].try_into().unwrap());
    let body_sha256: [u8; 32] = bytes[20..52].try_into().unwrap();
    let body = &bytes[STATIC_MICRO_OP_HEADER_V1_BYTES..];
    if <[u8; 32]>::from(Sha256::digest(body)) != body_sha256 {
        return Err(StaticMicroOpPackErrorV1::DigestMismatch);
    }

    let mut cursor = 0usize;
    let mut observed = 0u64;
    let mut previous: Option<(BankId, u32, u32)> = None;
    let mut spans = Vec::with_capacity(span_count as usize);
    for span_index in 0..span_count {
        let header = take(body, &mut cursor, STATIC_MICRO_OP_SPAN_HEADER_V1_BYTES)?;
        let bank = BankId::new(u64::from_be_bytes(header[0..8].try_into().unwrap()));
        let vram = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let word_count = u32::from_be_bytes(header[12..16].try_into().unwrap());
        if word_count == 0 {
            return Err(StaticMicroOpPackErrorV1::EmptySpan { bank, vram });
        }
        let byte_len = word_count
            .checked_mul(4)
            .ok_or(StaticMicroOpPackErrorV1::AddressOverflow { bank, vram })?;
        let end = vram
            .checked_add(byte_len)
            .ok_or(StaticMicroOpPackErrorV1::AddressOverflow { bank, vram })?;
        validate_geometry(bank, vram, end, previous)?;
        let record_bytes = usize::try_from(word_count)
            .ok()
            .and_then(|count| count.checked_mul(STATIC_MICRO_OP_RECORD_V1_BYTES))
            .ok_or(StaticMicroOpPackErrorV1::CountOverflow)?;
        let encoded = take(body, &mut cursor, record_bytes)?;
        let mut records = Vec::with_capacity(word_count as usize);
        for (word_index, record) in encoded
            .chunks_exact(STATIC_MICRO_OP_RECORD_V1_BYTES)
            .enumerate()
        {
            records.push(
                StaticMicroOpRecordV1::from_bytes(record.try_into().unwrap()).map_err(
                    |source| StaticMicroOpPackErrorV1::InvalidRecord {
                        span_index,
                        word_index: word_index as u32,
                        source,
                    },
                )?,
            );
        }
        observed = observed
            .checked_add(u64::from(word_count))
            .ok_or(StaticMicroOpPackErrorV1::CountOverflow)?;
        spans.push(AdmittedSpan {
            bank,
            vram,
            end,
            records,
            delay_lookahead: None,
        });
        previous = Some((bank, vram, end));
    }
    if cursor != body.len() {
        return Err(StaticMicroOpPackErrorV1::TrailingBytes);
    }
    if observed != instruction_count {
        return Err(StaticMicroOpPackErrorV1::CountMismatch {
            header: instruction_count,
            observed,
        });
    }

    let program = AdmittedStaticMicroOpProgramV1 {
        bytes: bytes.to_vec(),
        spans,
        instruction_count,
        body_sha256,
    };
    for span in &program.spans {
        for (index, record) in span.records.iter().copied().enumerate() {
            if !record.has_delay_slot() {
                continue;
            }
            let pc = span.vram + index as u32 * 4;
            pc.checked_add(4)
                .ok_or(StaticMicroOpPackErrorV1::MissingDelaySlot {
                    bank: span.bank,
                    pc,
                })?;
            program.delay_record(span.bank, pc).ok_or(
                StaticMicroOpPackErrorV1::MissingDelaySlot {
                    bank: span.bank,
                    pc,
                },
            )?;
        }
    }
    Ok(program)
}

fn parse_static_micro_op_pack_v2(
    bytes: &[u8],
) -> Result<AdmittedStaticMicroOpProgramV2, StaticMicroOpPackErrorV1> {
    if bytes.len() < STATIC_MICRO_OP_HEADER_V1_BYTES {
        return Err(StaticMicroOpPackErrorV1::Truncated);
    }
    if &bytes[..8] != STATIC_MICRO_OP_MAGIC_V2 {
        return Err(StaticMicroOpPackErrorV1::InvalidMagic);
    }
    let span_count = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    let instruction_count = u64::from_be_bytes(bytes[12..20].try_into().unwrap());
    let body_sha256: [u8; 32] = bytes[20..52].try_into().unwrap();
    let body = &bytes[STATIC_MICRO_OP_HEADER_V1_BYTES..];
    if <[u8; 32]>::from(Sha256::digest(body)) != body_sha256 {
        return Err(StaticMicroOpPackErrorV1::DigestMismatch);
    }

    let mut cursor = 0usize;
    let mut observed = 0u64;
    let mut previous: Option<(BankId, u32, u32)> = None;
    let mut spans = Vec::with_capacity(span_count as usize);
    for span_index in 0..span_count {
        let header = take(body, &mut cursor, STATIC_MICRO_OP_SPAN_HEADER_V2_BYTES)?;
        let bank = BankId::new(u64::from_be_bytes(header[0..8].try_into().unwrap()));
        let vram = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let word_count = u32::from_be_bytes(header[12..16].try_into().unwrap());
        let lookahead_tag = header[16];
        if lookahead_tag > 1 {
            return Err(StaticMicroOpPackErrorV1::InvalidLookaheadTag {
                span_index,
                actual: lookahead_tag,
            });
        }
        if word_count == 0 {
            return Err(StaticMicroOpPackErrorV1::EmptySpan { bank, vram });
        }
        let byte_len = word_count
            .checked_mul(4)
            .ok_or(StaticMicroOpPackErrorV1::AddressOverflow { bank, vram })?;
        let end = vram
            .checked_add(byte_len)
            .ok_or(StaticMicroOpPackErrorV1::AddressOverflow { bank, vram })?;
        validate_geometry(bank, vram, end, previous)?;
        let encoded_count = usize::try_from(word_count)
            .ok()
            .and_then(|count| count.checked_add(usize::from(lookahead_tag)))
            .ok_or(StaticMicroOpPackErrorV1::CountOverflow)?;
        let encoded = take(
            body,
            &mut cursor,
            encoded_count
                .checked_mul(STATIC_MICRO_OP_RECORD_V1_BYTES)
                .ok_or(StaticMicroOpPackErrorV1::CountOverflow)?,
        )?;
        let mut decoded = Vec::with_capacity(encoded_count);
        for (word_index, record) in encoded
            .chunks_exact(STATIC_MICRO_OP_RECORD_V1_BYTES)
            .enumerate()
        {
            decoded.push(
                StaticMicroOpRecordV1::from_bytes(record.try_into().unwrap()).map_err(
                    |source| StaticMicroOpPackErrorV1::InvalidRecord {
                        span_index,
                        word_index: word_index as u32,
                        source,
                    },
                )?,
            );
        }
        let delay_lookahead = (lookahead_tag == 1)
            .then(|| decoded.pop().expect("lookahead tag has one encoded record"));
        observed = observed
            .checked_add(u64::from(word_count))
            .ok_or(StaticMicroOpPackErrorV1::CountOverflow)?;
        spans.push(AdmittedSpan {
            bank,
            vram,
            end,
            records: decoded,
            delay_lookahead,
        });
        previous = Some((bank, vram, end));
    }
    if cursor != body.len() {
        return Err(StaticMicroOpPackErrorV1::TrailingBytes);
    }
    if observed != instruction_count {
        return Err(StaticMicroOpPackErrorV1::CountMismatch {
            header: instruction_count,
            observed,
        });
    }

    let core = AdmittedStaticMicroOpProgramV1 {
        bytes: bytes.to_vec(),
        spans,
        instruction_count,
        body_sha256,
    };
    for span in &core.spans {
        if span.delay_lookahead.is_some() {
            let final_pc = span.end - 4;
            let final_record = span.records.last().copied().expect("span is nonempty");
            if !final_record.has_delay_slot() || core.record(span.bank, span.end).is_some() {
                return Err(StaticMicroOpPackErrorV1::UnexpectedDelayLookahead {
                    bank: span.bank,
                    pc: final_pc,
                });
            }
        }
        for (index, record) in span.records.iter().copied().enumerate() {
            if !record.has_delay_slot() {
                continue;
            }
            let pc = span.vram + index as u32 * 4;
            core.delay_record(span.bank, pc)
                .ok_or(StaticMicroOpPackErrorV1::MissingDelaySlot {
                    bank: span.bank,
                    pc,
                })?;
        }
    }
    Ok(AdmittedStaticMicroOpProgramV2 { core })
}

fn validate_geometry(
    bank: BankId,
    vram: u32,
    end: u32,
    previous: Option<(BankId, u32, u32)>,
) -> Result<(), StaticMicroOpPackErrorV1> {
    if !vram.is_multiple_of(4) {
        return Err(StaticMicroOpPackErrorV1::UnalignedStart { bank, vram });
    }
    if let Some((previous_bank, previous_vram, previous_end)) = previous {
        if (bank, vram) < (previous_bank, previous_vram) {
            return Err(StaticMicroOpPackErrorV1::OutOfOrder {
                previous_bank,
                previous_vram,
                bank,
                vram,
            });
        }
        if bank == previous_bank && vram < previous_end {
            return Err(StaticMicroOpPackErrorV1::Overlap {
                bank,
                previous_end,
                vram,
            });
        }
    }
    debug_assert!(end >= vram);
    Ok(())
}

fn take<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    len: usize,
) -> Result<&'a [u8], StaticMicroOpPackErrorV1> {
    let end = cursor
        .checked_add(len)
        .ok_or(StaticMicroOpPackErrorV1::Truncated)?;
    let result = bytes
        .get(*cursor..end)
        .ok_or(StaticMicroOpPackErrorV1::Truncated)?;
    *cursor = end;
    Ok(result)
}

fn execute_addiu(word: u32, ctx: &mut RecompContext) {
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let immediate = word as u16 as i16 as i32 as u32;
    ctx.set_r32(rt, ctx.r_u32(rs).wrapping_add(immediate) as i32);
}

fn branch_equal(word: u32, ctx: &RecompContext) -> bool {
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    ctx.r(rs) == ctx.r(rt)
}

fn branch_target(pc: u32, word: u32) -> u32 {
    let displacement = ((word as u16 as i16 as i32) << 2) as u32;
    pc.wrapping_add(4).wrapping_add(displacement)
}

fn execute_delay(
    bank: BankId,
    branch_pc: u32,
    delay_pc: u32,
    record: StaticMicroOpRecordV1,
    ctx: &mut RecompContext,
) -> Option<CpuFault> {
    match record.opcode {
        0 => None,
        44 => {
            execute_addiu(record.expected_raw_word, ctx);
            None
        }
        STATIC_MICRO_OP_OPCODE_RESERVED_INSTRUCTION_V1 => {
            Some(reserved_instruction_fault(bank, delay_pc, branch_pc, true))
        }
        _ => Some(CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(delay_pc)),
            kind: CpuFaultKind::UnsupportedInstruction {
                word: record.expected_raw_word,
            },
        }),
    }
}

fn reserved_instruction_fault(bank: BankId, at: u32, epc: u32, branch_delay: bool) -> CpuFault {
    CpuFault {
        at: ExecutionKey::new(bank, GuestPc::new(at)),
        kind: CpuFaultKind::Exception {
            exception: CpuException::ReservedInstruction,
            epc: GuestPc::new(epc),
            branch_delay,
            instruction_code: 0,
            bad_vaddr: None,
            coprocessor: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_receipts_are_nonzero_and_cannot_claim_production() {
        let source = static_micro_op_executor_source_receipt_v1();
        assert_eq!(source.schema(), STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V1);
        assert_ne!(source.source_sha256(), [0; 32]);
        assert_eq!(source, static_micro_op_executor_source_receipt_v1());

        let build = static_micro_op_execution_build_receipt_v1();
        assert_eq!(build.schema, STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V1);
        assert!(build.experimental_predecoded_aot);
        assert!(!build.production_authority);
        assert_eq!(build.executor_source, source);

        let source_v2 = static_micro_op_executor_source_receipt_v2();
        assert_eq!(
            source_v2.schema(),
            STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V2
        );
        assert_ne!(source_v2.source_sha256(), [0; 32]);
        let build_v2 = static_micro_op_execution_build_receipt_v2();
        assert_eq!(build_v2.schema, STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V2);
        assert!(build_v2.experimental_predecoded_aot);
        assert!(!build_v2.production_authority);
        assert_eq!(build_v2.executor_source, source_v2);

        let source_v3 = static_micro_op_executor_source_receipt_v3();
        assert_eq!(
            source_v3.schema(),
            STATIC_MICRO_OP_EXECUTOR_SOURCE_SCHEMA_V3
        );
        assert_ne!(source_v3.source_sha256(), [0; 32]);
        assert_ne!(source_v3.source_sha256(), source_v2.source_sha256());
        assert_eq!(source_v3, static_micro_op_executor_source_receipt_v3());
        let build_v3 = static_micro_op_execution_build_receipt_v3();
        assert_eq!(build_v3.schema, STATIC_MICRO_OP_EXECUTION_BUILD_SCHEMA_V3);
        assert!(build_v3.experimental_predecoded_aot);
        assert!(!build_v3.production_authority);
        assert_eq!(build_v3.executor_source, source_v3);
    }
}
