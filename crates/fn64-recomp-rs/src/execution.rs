//! Bank-qualified execution identities and code-image admission.
//!
//! Historical function boundaries are useful decompilation evidence, but they
//! are not an architectural property of the VR4300.  A general translator
//! must be able to resume any aligned instruction in the *currently loaded*
//! code image, including when two overlays occupy the same virtual address.
//! This module establishes that identity for the block runner: every
//! destination is an [`ExecutionKey`] (`BankId`, `GuestPc`), and a
//! [`CodeCatalog`] resolves it without consulting a function symbol table.

use std::collections::BTreeMap;
use std::fmt;

use crate::runtime::{Rdram, RecompContext};

/// Stable identity of one admitted code image.
///
/// The producer chooses the value from its bank/image lineage.  It must change
/// when executable bytes at an overlapping virtual address denote a different
/// image or generation; [`CodeCatalog`] rejects reusing an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BankId(u64);

impl BankId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for BankId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bank:{:016X}", self.0)
    }
}

/// A guest virtual program counter.
///
/// Alignment is checked at the execution boundary rather than hidden in this
/// constructor so malformed machine state becomes a typed [`CpuFault`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GuestPc(u32);

impl GuestPc {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn is_instruction_aligned(self) -> bool {
        self.0 & 3 == 0
    }
}

impl fmt::Display for GuestPc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0)
    }
}

/// Complete identity of one CPU execution destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExecutionKey {
    pub bank: BankId,
    pub pc: GuestPc,
}

impl ExecutionKey {
    pub const fn new(bank: BankId, pc: GuestPc) -> Self {
        Self { bank, pc }
    }
}

impl fmt::Display for ExecutionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, pc={})", self.bank, self.pc)
    }
}

/// Why CPU execution could not begin or continue at an [`ExecutionKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuFaultKind {
    UnalignedPc,
    UnknownBank,
    UnmappedPc { bank_start: u32, bank_end: u32 },
}

/// A guest CPU fault with the exact bank-qualified destination that caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuFault {
    pub at: ExecutionKey,
    pub kind: CpuFaultKind,
}

impl fmt::Display for CpuFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CpuFaultKind::UnalignedPc => write!(f, "unaligned execution PC at {}", self.at),
            CpuFaultKind::UnknownBank => write!(f, "unknown executable bank at {}", self.at),
            CpuFaultKind::UnmappedPc {
                bank_start,
                bank_end,
            } => write!(
                f,
                "unmapped execution PC at {}; bank interval is {bank_start:#010X}..{bank_end:#010X}",
                self.at
            ),
        }
    }
}

impl std::error::Error for CpuFault {}

/// Typed boundary between one translated block and its dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockExit {
    /// Destination was proven when the block was translated.
    Transfer(ExecutionKey),
    /// Machine code supplied only a virtual target (for example `jr $t9`).
    /// The active mapping layer must resolve it to exactly one bank-qualified
    /// key before another block may execute.
    ResolveTransfer {
        source_bank: BankId,
        target_pc: GuestPc,
    },
    HostCall {
        vram: GuestPc,
        resume: ExecutionKey,
    },
    Checkpoint(ExecutionKey),
    Yield(ExecutionKey),
    Fault(CpuFault),
}

/// Maximum number of ordinary instructions a runner may execute before it
/// returns a deterministic checkpoint. Two is the minimum because a control
/// transfer and its delay slot are one indivisible dispatch unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstructionBudget(u32);

impl InstructionBudget {
    pub const MIN: u32 = 2;

    pub const fn new(value: u32) -> Option<Self> {
        if value >= Self::MIN {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Result of one block-runner turn, including deterministic guest work for
/// the clock/device layer to charge before following the exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRun {
    pub exit: BlockExit,
    pub instructions: u32,
}

impl BlockRun {
    pub const fn new(exit: BlockExit, instructions: u32) -> Self {
        Self { exit, instructions }
    }
}

/// One installed bank/basic-block execution lane.
///
/// The trait keeps the dispatcher independent of how a block was produced:
/// generated Rust, a future dynamic translator, and an instrumented
/// interpreter can all satisfy the same contract.
pub trait BlockRunner {
    fn run(&mut self, entry: ExecutionKey, budget: InstructionBudget) -> BlockRun;
}

/// Callable shape emitted for one immutable sparse bank.
pub type GeneratedBankFn = for<'ctx, 'view, 'rdram> fn(
    ExecutionKey,
    InstructionBudget,
    &'ctx mut RecompContext,
    &'view mut Rdram<'rdram>,
) -> BlockRun;

/// A generated callable bound to the bank identity embedded in its body.
#[derive(Clone, Copy)]
pub struct GeneratedBankRunner {
    bank: BankId,
    run: GeneratedBankFn,
}

impl GeneratedBankRunner {
    pub const fn new(bank: BankId, run: GeneratedBankFn) -> Self {
        Self { bank, run }
    }

    pub const fn bank(self) -> BankId {
        self.bank
    }
}

impl<F> BlockRunner for F
where
    F: FnMut(ExecutionKey, InstructionBudget) -> BlockRun,
{
    fn run(&mut self, entry: ExecutionKey, budget: InstructionBudget) -> BlockRun {
        self(entry, budget)
    }
}

/// Resolves a machine-computed virtual target against the currently active
/// executable mapping. A virtual PC alone is never enough to choose between
/// overlapping banks.
pub trait TransferResolver {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault>;
}

impl<F> TransferResolver for F
where
    F: FnMut(BankId, GuestPc) -> Result<ExecutionKey, CpuFault>,
{
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self(source_bank, target_pc)
    }
}

/// Work completed by [`dispatch_until_boundary`] before a device/scheduler
/// boundary, host call, yield, or fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchRun {
    pub exit: BlockExit,
    pub instructions: u32,
    pub blocks: u32,
}

/// Violation of the generated/dynamic runner contract.
///
/// These are host translation defects, not guest CPU exceptions, so they are
/// kept distinct from [`CpuFault`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    ContinuingExitWithoutProgress {
        at: ExecutionKey,
        exit: BlockExit,
    },
    RunnerExceededBudget {
        at: ExecutionKey,
        budget: InstructionBudget,
        actual: u32,
    },
    InstructionCountOverflow,
    BlockCountOverflow,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ContinuingExitWithoutProgress { at, exit } => {
                write!(f, "block runner made no progress at {at}: {exit:?}")
            }
            Self::RunnerExceededBudget { at, budget, actual } => write!(
                f,
                "block runner at {at} executed {actual} instructions with budget {}",
                budget.get()
            ),
            Self::InstructionCountOverflow => write!(f, "dispatch instruction count overflow"),
            Self::BlockCountOverflow => write!(f, "dispatch block count overflow"),
        }
    }
}

impl std::error::Error for DispatchError {}

/// Follow translated block exits until guest execution must return to the
/// device/scheduler layer.
///
/// A total budget is enforced across direct and computed transfers. If fewer
/// than two instructions remain after a transfer, the dispatcher checkpoints
/// at the destination instead of asking a runner to split a branch/delay
/// pair. Resolver failures become ordinary typed CPU-fault exits with all work
/// already completed preserved in the result.
pub fn dispatch_until_boundary<R, V>(
    mut entry: ExecutionKey,
    budget: InstructionBudget,
    runner: &mut R,
    resolver: &mut V,
) -> Result<DispatchRun, DispatchError>
where
    R: BlockRunner,
    V: TransferResolver,
{
    let mut instructions = 0u32;
    let mut blocks = 0u32;

    loop {
        let remaining = budget.get() - instructions;
        if remaining < InstructionBudget::MIN {
            return Ok(DispatchRun {
                exit: BlockExit::Checkpoint(entry),
                instructions,
                blocks,
            });
        }
        let turn_budget = InstructionBudget::new(remaining)
            .expect("remaining budget was checked against InstructionBudget::MIN");
        let run = runner.run(entry, turn_budget);
        if run.instructions > remaining {
            return Err(DispatchError::RunnerExceededBudget {
                at: entry,
                budget: turn_budget,
                actual: run.instructions,
            });
        }
        if run.instructions == 0
            && matches!(
                run.exit,
                BlockExit::Transfer(_) | BlockExit::ResolveTransfer { .. }
            )
        {
            return Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: run.exit,
            });
        }
        instructions = instructions
            .checked_add(run.instructions)
            .ok_or(DispatchError::InstructionCountOverflow)?;
        blocks = blocks
            .checked_add(1)
            .ok_or(DispatchError::BlockCountOverflow)?;

        match run.exit {
            BlockExit::Transfer(next) => entry = next,
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc,
            } => match resolver.resolve(source_bank, target_pc) {
                Ok(next) => entry = next,
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            },
            exit => {
                return Ok(DispatchRun {
                    exit,
                    instructions,
                    blocks,
                });
            }
        }
    }
}

/// One owned, contiguous executable span within a bank.
///
/// Construction binds the span to its bank identity and proves nonempty,
/// aligned, non-overflowing geometry. Cross-span ordering and overlap are
/// validated by [`CodeBank::from_spans`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    bank: BankId,
    vram_start: GuestPc,
    words: Vec<u32>,
}

impl CodeSpan {
    pub fn new(bank: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        if !vram_start.is_instruction_aligned() {
            return Err(BankError::UnalignedStart {
                bank,
                start: vram_start,
            });
        }
        if words.is_empty() {
            return Err(BankError::Empty { bank });
        }
        let byte_len = u32::try_from(words.len())
            .ok()
            .and_then(|len| len.checked_mul(4))
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        vram_start
            .get()
            .checked_add(byte_len)
            .ok_or(BankError::AddressOverflow {
                bank,
                start: vram_start,
            })?;
        Ok(Self {
            bank,
            vram_start,
            words,
        })
    }

    pub const fn bank(&self) -> BankId {
        self.bank
    }

    pub const fn vram_start(&self) -> GuestPc {
        self.vram_start
    }

    pub fn vram_end(&self) -> GuestPc {
        GuestPc::new(self.vram_start.get() + self.words.len() as u32 * 4)
    }

    pub fn instruction_count(&self) -> usize {
        self.words.len()
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let offset = pc.get().checked_sub(self.vram_start.get())?;
        self.words.get((offset / 4) as usize).copied()
    }
}

/// One immutable sparse executable image admitted to the block translator.
///
/// A bank owns sorted, disjoint [`CodeSpan`] values. Its lowest/highest
/// addresses are diagnostic bounds only; addresses in holes never resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBank {
    id: BankId,
    spans: Vec<CodeSpan>,
}

impl CodeBank {
    /// Convenience constructor for a single contiguous executable span.
    pub fn new(id: BankId, vram_start: GuestPc, words: Vec<u32>) -> Result<Self, BankError> {
        Self::from_spans(id, vec![CodeSpan::new(id, vram_start, words)?])
    }

    /// Admit sorted, disjoint executable spans under one immutable identity.
    pub fn from_spans(id: BankId, mut spans: Vec<CodeSpan>) -> Result<Self, BankError> {
        if spans.is_empty() {
            return Err(BankError::Empty { bank: id });
        }
        for span in &spans {
            if span.bank() != id {
                return Err(BankError::SpanBankMismatch {
                    bank: id,
                    span_bank: span.bank(),
                    start: span.vram_start(),
                });
            }
        }
        spans.sort_by_key(CodeSpan::vram_start);
        for pair in spans.windows(2) {
            let left_end = pair[0].vram_end();
            let right_start = pair[1].vram_start();
            if right_start < left_end {
                return Err(BankError::OverlappingSpans {
                    bank: id,
                    left_end,
                    right_start,
                });
            }
        }
        Ok(Self { id, spans })
    }

    pub const fn id(&self) -> BankId {
        self.id
    }

    pub fn vram_start(&self) -> GuestPc {
        self.spans[0].vram_start()
    }

    pub fn vram_end(&self) -> GuestPc {
        self.spans
            .last()
            .expect("CodeBank construction requires a span")
            .vram_end()
    }

    pub fn instruction_count(&self) -> usize {
        self.spans.iter().map(CodeSpan::instruction_count).sum()
    }

    pub fn spans(&self) -> &[CodeSpan] {
        &self.spans
    }

    fn resolve(&self, pc: GuestPc) -> Option<u32> {
        let candidate = self
            .spans
            .partition_point(|span| span.vram_start() <= pc)
            .checked_sub(1)?;
        let span = &self.spans[candidate];
        if pc < span.vram_end() {
            span.resolve(pc)
        } else {
            None
        }
    }
}

/// Failure to admit an executable image into a [`CodeCatalog`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankError {
    Empty {
        bank: BankId,
    },
    UnalignedStart {
        bank: BankId,
        start: GuestPc,
    },
    AddressOverflow {
        bank: BankId,
        start: GuestPc,
    },
    SpanBankMismatch {
        bank: BankId,
        span_bank: BankId,
        start: GuestPc,
    },
    OverlappingSpans {
        bank: BankId,
        left_end: GuestPc,
        right_start: GuestPc,
    },
    DuplicateId {
        bank: BankId,
    },
}

impl fmt::Display for BankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            BankError::Empty { bank } => write!(f, "{bank} has no executable words"),
            BankError::UnalignedStart { bank, start } => {
                write!(f, "{bank} starts at unaligned PC {start}")
            }
            BankError::AddressOverflow { bank, start } => {
                write!(
                    f,
                    "{bank} starting at {start} exceeds the guest address space"
                )
            }
            BankError::SpanBankMismatch {
                bank,
                span_bank,
                start,
            } => write!(
                f,
                "{bank} cannot own span from {span_bank} starting at {start}"
            ),
            BankError::OverlappingSpans {
                bank,
                left_end,
                right_start,
            } => write!(
                f,
                "{bank} has overlapping executable spans at {left_end} and {right_start}"
            ),
            BankError::DuplicateId { bank } => {
                write!(f, "executable identity {bank} is already registered")
            }
        }
    }
}

impl std::error::Error for BankError {}

/// A resolved instruction word and the bank-qualified address that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedInstruction {
    pub key: ExecutionKey,
    pub word: u32,
}

/// Deterministic registry of immutable executable images.
///
/// Banks may overlap in virtual address space.  Only their identities must be
/// unique, which is exactly what prevents an overlay lookup from silently
/// selecting whichever same-VA image happened to be registered last.
#[derive(Clone, Debug, Default)]
pub struct CodeCatalog {
    banks: BTreeMap<BankId, CodeBank>,
}

impl CodeCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, bank: CodeBank) -> Result<(), BankError> {
        let id = bank.id();
        if self.banks.contains_key(&id) {
            return Err(BankError::DuplicateId { bank: id });
        }
        self.banks.insert(id, bank);
        Ok(())
    }

    pub fn bank(&self, id: BankId) -> Option<&CodeBank> {
        self.banks.get(&id)
    }

    pub fn resolve(&self, key: ExecutionKey) -> Result<ResolvedInstruction, CpuFault> {
        if !key.pc.is_instruction_aligned() {
            return Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnalignedPc,
            });
        }
        let bank = self.banks.get(&key.bank).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnknownBank,
        })?;
        let start = bank.vram_start().get();
        let end = bank.vram_end().get();
        let word = bank.resolve(key.pc).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: start,
                bank_end: end,
            },
        })?;
        Ok(ResolvedInstruction { key, word })
    }

    /// Classify an admitted instruction for table-backed dispatch.  Resolution
    /// goes through the same sparse bank catalog as execution, so a data hole
    /// cannot acquire a classification merely because it lies inside a
    /// bounding interval.
    pub fn classify(&self, key: ExecutionKey) -> Result<crate::emit::BankWordKind, CpuFault> {
        let resolved = self.resolve(key)?;
        let instruction = crate::decode(resolved.word);
        Ok(
            if matches!(instruction, crate::decoder::Instruction::Unknown { .. }) {
                crate::emit::BankWordKind::Unknown
            } else if instruction.has_delay_slot() {
                crate::emit::BankWordKind::ControlTransfer
            } else {
                crate::emit::BankWordKind::Straight
            },
        )
    }
}

/// Failure to atomically pair admitted code with its generated runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramError {
    RunnerBankMismatch {
        code_bank: BankId,
        runner_bank: BankId,
    },
    DuplicateBank {
        bank: BankId,
    },
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RunnerBankMismatch {
                code_bank,
                runner_bank,
            } => write!(
                f,
                "generated runner for {runner_bank} cannot execute code admitted as {code_bank}"
            ),
            Self::DuplicateBank { bank } => write!(f, "block program already contains {bank}"),
        }
    }
}

impl std::error::Error for ProgramError {}

/// Immutable-code catalog and generated callables registered as one program.
///
/// The maps are private and registration validates both identities before
/// mutating either one. A call is admitted through [`CodeCatalog::resolve`]
/// before the generated function runs, so a broad generated match cannot
/// accidentally make a sparse-bank hole executable.
#[derive(Default)]
pub struct BlockProgram {
    code: CodeCatalog,
    runners: BTreeMap<BankId, GeneratedBankFn>,
}

impl BlockProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<(), ProgramError> {
        let code_bank = code.id();
        if runner.bank != code_bank {
            return Err(ProgramError::RunnerBankMismatch {
                code_bank,
                runner_bank: runner.bank,
            });
        }
        if self.code.bank(code_bank).is_some() || self.runners.contains_key(&code_bank) {
            return Err(ProgramError::DuplicateBank { bank: code_bank });
        }
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.runners.insert(code_bank, runner.run);
        Ok(())
    }

    pub fn code(&self) -> &CodeCatalog {
        &self.code
    }

    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if let Err(fault) = self.code.resolve(entry) {
            return BlockRun::new(BlockExit::Fault(fault), 0);
        }
        let run = self.runners.get(&entry.bank).copied().unwrap_or_else(|| {
            panic!(
                "block program invariant violated: admitted {} has no generated runner",
                entry.bank
            )
        });
        run(entry, budget, ctx, mem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VA: GuestPc = GuestPc::new(0x8000_1000);

    fn bank(id: u64, words: &[u32]) -> CodeBank {
        CodeBank::new(BankId::new(id), VA, words.to_vec()).unwrap()
    }

    fn first_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 1);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    fn second_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        ctx.set_r32(2, 2);
        BlockRun::new(BlockExit::Yield(entry), 1)
    }

    #[test]
    fn resolves_an_interior_instruction_without_a_function_entry() {
        let mut catalog = CodeCatalog::new();
        catalog
            .register(bank(1, &[0x1111, 0x2222, 0x3333]))
            .unwrap();

        let key = ExecutionKey::new(BankId::new(1), GuestPc::new(VA.get() + 4));
        assert_eq!(catalog.resolve(key).unwrap().word, 0x2222);
    }

    #[test]
    fn same_virtual_address_resolves_by_bank_identity() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        catalog.register(bank(2, &[0x2222])).unwrap();

        let first = ExecutionKey::new(BankId::new(1), VA);
        let second = ExecutionKey::new(BankId::new(2), VA);
        assert_eq!(catalog.resolve(first).unwrap().word, 0x1111);
        assert_eq!(catalog.resolve(second).unwrap().word, 0x2222);
    }

    #[test]
    fn sparse_bank_sorts_spans_and_never_resolves_a_bounding_hole() {
        let id = BankId::new(3);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x3333]).unwrap(),
                CodeSpan::new(id, VA, vec![0x1111, 0x2222]).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(bank.vram_start(), VA);
        assert_eq!(bank.vram_end(), GuestPc::new(VA.get() + 0x24));
        assert_eq!(bank.instruction_count(), 3);

        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap()
                .word,
            0x3333
        );
        assert!(matches!(
            catalog
                .resolve(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10)))
                .unwrap_err()
                .kind,
            CpuFaultKind::UnmappedPc { .. }
        ));
    }

    #[test]
    fn sparse_bank_rejects_overlap_and_cross_bank_spans() {
        let id = BankId::new(4);
        let overlap = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1, 2]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 4), vec![3]).unwrap(),
            ],
        );
        assert_eq!(
            overlap,
            Err(BankError::OverlappingSpans {
                bank: id,
                left_end: GuestPc::new(VA.get() + 8),
                right_start: GuestPc::new(VA.get() + 4),
            })
        );

        let other = BankId::new(5);
        assert_eq!(
            CodeBank::from_spans(id, vec![CodeSpan::new(other, VA, vec![1]).unwrap()]),
            Err(BankError::SpanBankMismatch {
                bank: id,
                span_bank: other,
                start: VA,
            })
        );
    }

    #[test]
    fn classify_uses_sparse_admission_and_rejects_holes() {
        let id = BankId::new(6);
        let bank = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![0x2402_0001]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 0x20), vec![0x0100_0008]).unwrap(),
            ],
        )
        .unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        assert_eq!(
            catalog.classify(ExecutionKey::new(id, VA)).unwrap(),
            crate::emit::BankWordKind::Straight
        );
        assert_eq!(
            catalog
                .classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x20)))
                .unwrap(),
            crate::emit::BankWordKind::ControlTransfer
        );
        assert!(matches!(
            catalog.classify(ExecutionKey::new(id, GuestPc::new(VA.get() + 0x10))),
            Err(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
    }

    #[test]
    fn block_program_registration_is_atomic_and_bank_qualified() {
        let first = BankId::new(10);
        let second = BankId::new(11);
        let mut program = BlockProgram::new();
        assert_eq!(
            program.register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(second, first_runner),
            ),
            Err(ProgramError::RunnerBankMismatch {
                code_bank: first,
                runner_bank: second,
            })
        );
        assert!(program.code().bank(first).is_none());

        program
            .register(
                bank(10, &[0x1111]),
                GeneratedBankRunner::new(first, first_runner),
            )
            .unwrap();
        program
            .register(
                bank(11, &[0x2222]),
                GeneratedBankRunner::new(second, second_runner),
            )
            .unwrap();
        assert_eq!(
            program.register(
                bank(10, &[0x3333]),
                GeneratedBankRunner::new(first, first_runner),
            ),
            Err(ProgramError::DuplicateBank { bank: first })
        );

        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let budget = InstructionBudget::new(2).unwrap();
        let first_key = ExecutionKey::new(first, VA);
        let second_key = ExecutionKey::new(second, VA);
        assert_eq!(
            program
                .run(first_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            program
                .run(second_key, budget, &mut ctx, &mut mem)
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 2);
    }

    #[test]
    fn block_program_rejects_holes_before_invoking_runner() {
        let id = BankId::new(12);
        let sparse = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, VA, vec![1]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA.get() + 8), vec![2]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register(sparse, GeneratedBankRunner::new(id, first_runner))
            .unwrap();
        let mut bytes = [];
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let hole = ExecutionKey::new(id, GuestPc::new(VA.get() + 4));
        let run = program.run(hole, InstructionBudget::new(2).unwrap(), &mut ctx, &mut mem);
        assert!(matches!(
            run,
            BlockRun {
                exit: BlockExit::Fault(CpuFault {
                    at,
                    kind: CpuFaultKind::UnmappedPc { .. }
                }),
                instructions: 0,
            } if at == hole
        ));
        assert_eq!(
            ctx.r_u32(2),
            0,
            "runner must not execute for a catalog hole"
        );
    }

    #[test]
    fn transfers_distinguish_proven_and_runtime_resolved_destinations() {
        let destination = ExecutionKey::new(BankId::new(9), GuestPc::new(0x8000_2000));
        assert_eq!(
            BlockExit::Transfer(destination),
            BlockExit::Transfer(destination)
        );

        let indirect = BlockExit::ResolveTransfer {
            source_bank: BankId::new(1),
            target_pc: GuestPc::new(0x8000_2000),
        };
        assert!(matches!(
            indirect,
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc
            } if source_bank == BankId::new(1) && target_pc == GuestPc::new(0x8000_2000)
        ));
    }

    #[test]
    fn instruction_budget_cannot_split_a_branch_delay_pair() {
        assert_eq!(InstructionBudget::new(0), None);
        assert_eq!(InstructionBudget::new(1), None);
        assert_eq!(InstructionBudget::new(2).unwrap().get(), 2);
    }

    #[test]
    fn malformed_destinations_fault_with_bank_and_pc() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(7, &[0])).unwrap();

        let unaligned = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 2));
        let fault = catalog.resolve(unaligned).unwrap_err();
        assert_eq!(fault.kind, CpuFaultKind::UnalignedPc);
        assert!(fault.to_string().contains("bank:0000000000000007"));
        assert!(fault.to_string().contains("0x80001002"));

        let unmapped = ExecutionKey::new(BankId::new(7), GuestPc::new(VA.get() + 4));
        assert!(matches!(
            catalog.resolve(unmapped).unwrap_err().kind,
            CpuFaultKind::UnmappedPc { .. }
        ));

        let unknown = ExecutionKey::new(BankId::new(8), VA);
        assert_eq!(
            catalog.resolve(unknown).unwrap_err().kind,
            CpuFaultKind::UnknownBank
        );
    }

    #[test]
    fn bank_identity_cannot_be_reused_for_new_bytes() {
        let mut catalog = CodeCatalog::new();
        catalog.register(bank(1, &[0x1111])).unwrap();
        assert_eq!(
            catalog.register(bank(1, &[0x2222])),
            Err(BankError::DuplicateId {
                bank: BankId::new(1)
            })
        );
    }

    #[test]
    fn dispatcher_follows_direct_and_resolved_bank_qualified_transfers() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let second = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1010));
        let third = ExecutionKey::new(BankId::new(2), GuestPc::new(0x8000_1010));
        let mut runner = |entry: ExecutionKey, _budget: InstructionBudget| match entry {
            key if key == first => BlockRun::new(BlockExit::Transfer(second), 1),
            key if key == second => BlockRun::new(
                BlockExit::ResolveTransfer {
                    source_bank: second.bank,
                    target_pc: second.pc,
                },
                2,
            ),
            key if key == third => BlockRun::new(BlockExit::Yield(third), 1),
            _ => unreachable!("test runner received an unexpected key"),
        };
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, second.bank);
            assert_eq!(target_pc, second.pc);
            Ok(third)
        };

        assert_eq!(
            dispatch_until_boundary(
                first,
                InstructionBudget::new(6).unwrap(),
                &mut runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::Yield(third),
                instructions: 4,
                blocks: 3,
            }
        );
    }

    #[test]
    fn dispatcher_checkpoints_before_an_indivisible_next_unit() {
        let first = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let next = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1004));
        let mut calls = 0;
        let mut runner = |_entry, _budget| {
            calls += 1;
            BlockRun::new(BlockExit::Transfer(next), 1)
        };
        let mut resolver = |_source_bank, _target_pc| unreachable!();

        let run = dispatch_until_boundary(
            first,
            InstructionBudget::new(2).unwrap(),
            &mut runner,
            &mut resolver,
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            run,
            DispatchRun {
                exit: BlockExit::Checkpoint(next),
                instructions: 1,
                blocks: 1,
            }
        );
    }

    #[test]
    fn dispatcher_rejects_non_progress_and_budget_violations() {
        let entry = ExecutionKey::new(BankId::new(1), GuestPc::new(0x8000_1000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut resolver = |_source_bank, _target_pc| unreachable!();
        let mut stalled = |_entry, _budget| BlockRun::new(BlockExit::Transfer(entry), 0);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut stalled, &mut resolver),
            Err(DispatchError::ContinuingExitWithoutProgress {
                at: entry,
                exit: BlockExit::Transfer(entry),
            })
        );

        let mut excessive = |_entry, _budget| BlockRun::new(BlockExit::Yield(entry), 3);
        assert_eq!(
            dispatch_until_boundary(entry, budget, &mut excessive, &mut resolver),
            Err(DispatchError::RunnerExceededBudget {
                at: entry,
                budget,
                actual: 3,
            })
        );
    }
}
