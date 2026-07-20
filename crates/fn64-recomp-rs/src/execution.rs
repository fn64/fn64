//! Bank-qualified execution identities and code-image admission.
//!
//! Historical function boundaries are useful decompilation evidence, but they
//! are not an architectural property of the VR4300.  A general translator
//! must be able to resume any aligned instruction in the *currently loaded*
//! code image, including when two overlays occupy the same virtual address.
//! This module establishes that identity for the block runner: every
//! destination is an [`ExecutionKey`] (`BankId`, `GuestPc`), and a
//! [`CodeCatalog`] resolves it without consulting a function symbol table.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

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
    /// Compatibility boundary used by the interpreter fallback for a computed
    /// instruction address that is not word aligned. Generated AOT runners use
    /// the architecturally precise [`Self::Exception`] form instead.
    UnalignedPc,
    UnknownBank,
    UnmappedPc {
        bank_start: u32,
        bank_end: u32,
    },
    /// A bankless virtual-address lookup matched more than one admitted code
    /// image. The first two candidates are ordered by [`BankId`]; the count
    /// preserves the complete ambiguity denominator without allocating in a
    /// CPU fault.
    AmbiguousPc {
        first_candidate: BankId,
        second_candidate: BankId,
        candidate_count: u32,
    },
    /// A guest data access whose effective address is outside the RDRAM bytes
    /// owned by the executing host. This remains distinct from architectural
    /// AdEL/AdES: the latter describes alignment, while this value names the
    /// host admission boundary shared by AOT and `dynamic_mips` lanes.
    MemoryFault {
        addr: u64,
    },
    /// A decoded instruction whose architecture is not yet modeled by the
    /// interpreter fallback. The raw word makes the unsupported frontier loud
    /// and deterministic instead of silently treating it as a nop.
    UnsupportedInstruction {
        word: u32,
    },
    Exception {
        exception: CpuException,
        epc: GuestPc,
        branch_delay: bool,
        instruction_code: u32,
        bad_vaddr: Option<u32>,
        coprocessor: Option<u8>,
    },
}

/// Architecturally defined synchronous exceptions currently produced by the
/// arbitrary-PC lane. Coprocessor and TLB exceptions join this enum as their
/// instruction paths stop using host panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuException {
    AddressErrorLoad,
    AddressErrorStore,
    CoprocessorUnusable,
    Syscall,
    Breakpoint,
    Trap,
    IntegerOverflow,
}

/// One of the VR4300 Cause.IP / Status.IM interrupt lines.
///
/// The N64's MIPS Interface drives [`Self::RCP`] (IP2). Keeping the CPU line
/// typed prevents device-specific MI bits from being confused with the CPU's
/// independently numbered pending bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CpuInterruptLine(u8);

impl CpuInterruptLine {
    pub const SOFTWARE_0: Self = Self(0);
    pub const SOFTWARE_1: Self = Self(1);
    pub const RCP: Self = Self(2);
    pub const CARTRIDGE: Self = Self(3);
    pub const PRE_NMI: Self = Self(4);
    pub const RDB_READ: Self = Self(5);
    pub const RDB_WRITE: Self = Self(6);
    pub const TIMER: Self = Self(7);

    pub const fn cause_bit(self) -> u32 {
        1 << (8 + self.0)
    }

    /// Drive this level-sensitive hardware line into Cause.IP.
    pub fn set_level(self, ctx: &mut RecompContext, asserted: bool) {
        if asserted {
            ctx.cop0_cause |= self.cause_bit();
        } else {
            ctx.cop0_cause &= !self.cause_bit();
        }
    }
}

/// Enter an enabled pending interrupt between translated instructions.
///
/// VR4300 User's Manual sections 6.2-6.3 define the gate as Status.IE set,
/// EXL/ERL clear, and a nonempty `Status.IM & Cause.IP`. Interrupts use
/// ExcCode 0 and the BEV-selected general exception vector. The arbitrary-PC
/// dispatcher calls this only at an instruction boundary, so BD is clear and
/// EPC is the instruction that would otherwise execute next.
pub fn enter_pending_interrupt(
    ctx: &mut RecompContext,
    interrupted_pc: GuestPc,
) -> Option<GuestPc> {
    const STATUS_IE: u32 = 1;
    const STATUS_EXL: u32 = 1 << 1;
    const STATUS_ERL: u32 = 1 << 2;
    const STATUS_IM_MASK: u32 = 0xFF << 8;
    const STATUS_BEV: u32 = 1 << 22;
    const CAUSE_IP_MASK: u32 = 0xFF << 8;
    const CAUSE_EXCCODE_MASK: u32 = 0x1F << 2;
    const CAUSE_BD: u32 = 1 << 31;

    let enabled = ctx.cop0_status & STATUS_IE != 0;
    let outside_exception = ctx.cop0_status & (STATUS_EXL | STATUS_ERL) == 0;
    let unmasked = (ctx.cop0_status & STATUS_IM_MASK) & (ctx.cop0_cause & CAUSE_IP_MASK) != 0;
    if !enabled || !outside_exception || !unmasked {
        return None;
    }

    ctx.cop0_epc = interrupted_pc.get();
    ctx.cop0_cause &= !(CAUSE_BD | CAUSE_EXCCODE_MASK);
    ctx.cop0_status |= STATUS_EXL;
    Some(GuestPc::new(if ctx.cop0_status & STATUS_BEV != 0 {
        0xBFC0_0380
    } else {
        0x8000_0180
    }))
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
            CpuFaultKind::AmbiguousPc {
                first_candidate,
                second_candidate,
                candidate_count,
            } => write!(
                f,
                "ambiguous execution PC at {}; {candidate_count} admitted banks match, beginning with {first_candidate} and {second_candidate}",
                self.at
            ),
            CpuFaultKind::MemoryFault { addr } => write!(
                f,
                "guest memory access outside backed RDRAM at {}; effective address {addr:#018X}",
                self.at
            ),
            CpuFaultKind::UnsupportedInstruction { word } => write!(
                f,
                "unsupported instruction {word:#010X} at {}; encoding decodes but its architecture is not modeled by the executing lane",
                self.at
            ),
            CpuFaultKind::Exception {
                exception,
                epc,
                branch_delay,
                instruction_code,
                bad_vaddr,
                coprocessor,
            } => write!(
                f,
                "CPU {exception:?} exception at {}; EPC={epc}, BD={branch_delay}, instruction code={instruction_code:#X}, BadVAddr={bad_vaddr:?}, coprocessor={coprocessor:?}",
                self.at
            ),
        }
    }
}

impl std::error::Error for CpuFault {}

impl CpuException {
    /// VR4300 Cause.ExcCode value (User's Manual, exception-code table).
    pub const fn cause_code(self) -> u32 {
        match self {
            Self::AddressErrorLoad => 4,
            Self::AddressErrorStore => 5,
            Self::Syscall => 8,
            Self::Breakpoint => 9,
            Self::CoprocessorUnusable => 11,
            Self::IntegerOverflow => 12,
            Self::Trap => 13,
        }
    }
}

impl CpuFault {
    /// Construct the AdEL raised when instruction fetch sees a PC that is not
    /// word-aligned. The fetch is not a branch delay instruction: EPC and
    /// BadVAddr both name the requested target and Cause.BD is clear.
    pub const fn instruction_address_error(at: ExecutionKey) -> Self {
        Self {
            at,
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: at.pc,
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(at.pc.get()),
                coprocessor: None,
            },
        }
    }

    /// Apply a synchronous exception to CP0 and return its general exception
    /// vector. VR4300 User's Manual section 6.3 defines EXL, EPC, Cause.BD,
    /// Cause.ExcCode, BadVAddr for address exceptions, and the BEV-selected
    /// general vectors.
    ///
    /// Returns `None` for mapping/dispatcher faults, which are host execution
    /// defects rather than guest architectural exceptions.
    pub fn enter_exception(self, ctx: &mut RecompContext) -> Option<GuestPc> {
        let CpuFaultKind::Exception {
            exception,
            epc,
            branch_delay,
            bad_vaddr,
            coprocessor,
            ..
        } = self.kind
        else {
            return None;
        };
        const STATUS_EXL: u32 = 1 << 1;
        const STATUS_BEV: u32 = 1 << 22;
        const CAUSE_BD: u32 = 1 << 31;
        const CAUSE_CE_MASK: u32 = 0b11 << 28;
        const CAUSE_EXCCODE_MASK: u32 = 0x1F << 2;

        if ctx.cop0_status & STATUS_EXL == 0 {
            ctx.cop0_epc = epc.get();
            if branch_delay {
                ctx.cop0_cause |= CAUSE_BD;
            } else {
                ctx.cop0_cause &= !CAUSE_BD;
            }
        }
        if let Some(bad_vaddr) = bad_vaddr {
            ctx.cop0_badvaddr = bad_vaddr;
        }
        if let Some(coprocessor) = coprocessor {
            assert!(
                coprocessor < 4,
                "Cause.CE coprocessor index exceeds two bits"
            );
            ctx.cop0_cause = (ctx.cop0_cause & !CAUSE_CE_MASK) | (u32::from(coprocessor) << 28);
        }
        ctx.cop0_cause = (ctx.cop0_cause & !CAUSE_EXCCODE_MASK) | (exception.cause_code() << 2);
        ctx.cop0_status |= STATUS_EXL;

        Some(GuestPc::new(if ctx.cop0_status & STATUS_BEV != 0 {
            0xBFC0_0380
        } else {
            0x8000_0180
        }))
    }
}

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
    /// A call target is computed or not statically classified. Unlike a jump,
    /// the resolver may identify this as a host ABI function; `resume` is the
    /// already-executed link address and remains bank-qualified.
    ResolveCall {
        source_bank: BankId,
        target_pc: GuestPc,
        resume: ExecutionKey,
    },
    HostCall {
        vram: GuestPc,
        resume: ExecutionKey,
    },
    Checkpoint(ExecutionKey),
    Yield(ExecutionKey),
    /// The guest thread entry returned through its configured sentinel. This
    /// is distinct from an unmapped-PC fault: live runtimes may only finish a
    /// coroutine when generated code or an explicit return adapter emits this
    /// boundary.
    ThreadReturn,
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
    artifact_identity: Option<ProgramArtifactIdentity>,
}

impl GeneratedBankRunner {
    /// Construct an executable runner without release-evidence identity.
    ///
    /// This compatibility path runs normally, but a containing
    /// [`BlockProgram`] cannot produce release evidence until every runner was
    /// installed through [`Self::new_with_artifact_identity`].
    pub const fn new(bank: BankId, run: GeneratedBankFn) -> Self {
        Self {
            bank,
            run,
            artifact_identity: None,
        }
    }

    /// Bind a generated callable to the stable build artifact which supplies
    /// its implementation. The identity is not derived from the function
    /// pointer and must describe the actual generated runner artifact.
    pub const fn new_with_artifact_identity(
        bank: BankId,
        run: GeneratedBankFn,
        artifact_identity: ProgramArtifactIdentity,
    ) -> Self {
        Self {
            bank,
            run,
            artifact_identity: Some(artifact_identity),
        }
    }

    pub const fn bank(self) -> BankId {
        self.bank
    }

    pub const fn artifact_identity(self) -> Option<ProgramArtifactIdentity> {
        self.artifact_identity
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

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        self.resolve(source_bank, target_pc)
            .map(CallResolution::Guest)
    }
}

/// Typed result of resolving a call destination. Host functions are not fake
/// executable banks and guest banks are not host function pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallResolution {
    Guest(ExecutionKey),
    Host,
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
                BlockExit::Transfer(_)
                    | BlockExit::ResolveTransfer { .. }
                    | BlockExit::ResolveCall { .. }
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
            BlockExit::ResolveCall {
                source_bank,
                target_pc,
                resume,
            } => match resolver.resolve_call(source_bank, target_pc, resume) {
                Ok(CallResolution::Guest(next)) => entry = next,
                Ok(CallResolution::Host) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::HostCall {
                            vram: target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
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

/// Stable 256-bit identity of the executable artifact installed by a host.
///
/// Function-lane native callables are opaque to safe Rust, so their producer
/// supplies the SHA-256 (or an equally stable 256-bit build identity) of the
/// actual generated artifact. Block programs derive their aggregate identity
/// from the canonical bank image plus each runner's supplied artifact
/// identity. Native addresses are never accepted as artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramArtifactIdentity([u8; 32]);

impl ProgramArtifactIdentity {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Authority behind a program artifact identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramIdentitySource {
    /// The host identified an opaque generated native artifact.
    CallerSupplied,
    /// fn64 hashed the complete canonical block code plus the stable artifact
    /// identity of every generated bank runner.
    CanonicalBlockProgramSha256,
}

/// Identity plus the authority which established it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramIdentityEvidenceSnapshot {
    pub identity: ProgramArtifactIdentity,
    pub source: ProgramIdentitySource,
}

/// Pointer-independent image of one contiguous executable span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpanEvidenceSnapshot {
    pub vram_start: GuestPc,
    pub words: Vec<u32>,
}

/// Pointer-independent image of one immutable sparse code bank.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBankEvidenceSnapshot {
    pub id: BankId,
    pub runner_artifact_identity: ProgramArtifactIdentity,
    pub spans: Vec<CodeSpanEvidenceSnapshot>,
}

/// Complete canonical executable image owned by a [`BlockProgram`].
///
/// Banks and spans are sorted by their typed identities/addresses. Instruction
/// word order is architectural and is retained verbatim. Generated runner
/// pointers are deliberately absent, but each bank retains the stable artifact
/// identity supplied with that runner: the words alone cannot prove two native
/// callables implement the same semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProgramEvidenceSnapshot {
    pub identity: ProgramIdentityEvidenceSnapshot,
    pub banks: Vec<CodeBankEvidenceSnapshot>,
}

/// One successfully entered bank-qualified guest execution destination.
///
/// The bank identity names the immutable code-image generation, while the
/// optional runner identity names the generated native artifact that was
/// actually entered. `None` is retained only for the compatibility
/// [`GeneratedBankRunner::new`] path and must not be promoted to release
/// evidence without the program's separate artifact-authority proof.
/// Historical execution observations are intentionally separate from
/// [`BlockProgramEvidenceSnapshot`]: they describe what happened, not state
/// which can affect future execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionDestinationObservation {
    pub destination: ExecutionKey,
    pub runner_artifact_identity: Option<ProgramArtifactIdentity>,
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

    fn unregister(&mut self, id: BankId) -> Option<CodeBank> {
        self.banks.remove(&id)
    }

    pub fn resolve(&self, key: ExecutionKey) -> Result<ResolvedInstruction, CpuFault> {
        if !key.pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(key));
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
    runners: BTreeMap<BankId, (GeneratedBankFn, Option<ProgramArtifactIdentity>)>,
    execution_destinations: RefCell<Vec<ExecutionDestinationObservation>>,
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
        self.runners
            .insert(code_bank, (runner.run, runner.artifact_identity));
        Ok(())
    }

    pub fn code(&self) -> &CodeCatalog {
        &self.code
    }

    /// Copy the append-only execution history in authoritative entry order.
    ///
    /// Resolution and classification do not append here. An observation is
    /// added only after sparse code admission and runner lookup both succeed.
    pub fn copy_execution_destinations(&self) -> Vec<ExecutionDestinationObservation> {
        self.execution_destinations.borrow().clone()
    }

    /// Start a new observation lifetime without changing executable state.
    pub fn clear_execution_destinations(&mut self) {
        self.execution_destinations.get_mut().clear();
    }

    /// Capture the complete immutable guest-code image without native
    /// callable addresses.
    ///
    /// `CodeCatalog` is a `BTreeMap` and `CodeBank` construction sorts spans,
    /// so equivalent registration order produces byte-identical evidence.
    /// The domain-separated SHA-256 covers every runner artifact identity,
    /// bank identity, span start, length, and instruction word encoded
    /// big-endian. Code words alone are insufficient because registration
    /// accepts independently generated native runners.
    pub fn evidence_snapshot(&self) -> BlockProgramEvidenceSnapshot {
        let banks = self
            .code
            .banks
            .values()
            .map(|bank| {
                let runner_artifact_identity = self
                    .runners
                    .get(&bank.id)
                    .and_then(|(_, identity)| *identity)
                    .unwrap_or_else(|| {
                        panic!(
                            "block-program release evidence requires a stable artifact identity for generated runner {}",
                            bank.id
                        )
                    });
                CodeBankEvidenceSnapshot {
                    id: bank.id,
                    runner_artifact_identity,
                    spans: bank
                        .spans
                        .iter()
                        .map(|span| CodeSpanEvidenceSnapshot {
                            vram_start: span.vram_start,
                            words: span.words.clone(),
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let mut hasher = Sha256::new();
        hasher.update(b"fn64.block-program.identity.v1\0");
        hasher.update(
            u64::try_from(banks.len())
                .expect("block-program bank count exceeds identity wire")
                .to_be_bytes(),
        );
        for bank in &banks {
            hasher.update(bank.id.get().to_be_bytes());
            hasher.update(bank.runner_artifact_identity.bytes());
            hasher.update(
                u64::try_from(bank.spans.len())
                    .expect("block-program span count exceeds identity wire")
                    .to_be_bytes(),
            );
            for span in &bank.spans {
                hasher.update(span.vram_start.get().to_be_bytes());
                hasher.update(
                    u64::try_from(span.words.len())
                        .expect("block-program instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &span.words {
                    hasher.update(word.to_be_bytes());
                }
            }
        }
        BlockProgramEvidenceSnapshot {
            identity: ProgramIdentityEvidenceSnapshot {
                identity: ProgramArtifactIdentity::new(hasher.finalize().into()),
                source: ProgramIdentitySource::CanonicalBlockProgramSha256,
            },
            banks,
        }
    }

    /// Atomically retire one immutable code generation and its callable.
    /// Returning `false` means neither half existed; a one-sided presence is
    /// an internal invariant violation rather than a recoverable stale state.
    pub fn unregister(&mut self, bank: BankId) -> bool {
        let code = self.code.unregister(bank);
        let runner = self.runners.remove(&bank);
        assert_eq!(
            code.is_some(),
            runner.is_some(),
            "block program generation {bank} existed in only one ownership map"
        );
        code.is_some()
    }

    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if let Err(fault) = self.code.resolve(entry) {
            let attempted_fetch = u32::from(matches!(fault.kind, CpuFaultKind::Exception { .. }));
            return BlockRun::new(BlockExit::Fault(fault), attempted_fetch);
        }
        let (run, runner_artifact_identity) =
            self.runners.get(&entry.bank).copied().unwrap_or_else(|| {
                panic!(
                    "block program invariant violated: admitted {} has no generated runner",
                    entry.bank
                )
            });
        self.execution_destinations
            .borrow_mut()
            .push(ExecutionDestinationObservation {
                destination: entry,
                runner_artifact_identity,
            });
        run(entry, budget, ctx, mem)
    }

    /// Run the registered arbitrary-PC program through transfers and
    /// synchronous architectural exception entry until execution reaches a
    /// scheduler/device boundary.
    ///
    /// Exception vectors are virtual addresses, so they go through the same
    /// active-mapping resolver as computed transfers. CP0 state is committed
    /// before vector resolution; a missing vector therefore returns the
    /// resolver's mapping fault without erasing the guest exception state.
    pub fn dispatch<V>(
        &self,
        mut entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        resolver: &mut V,
    ) -> Result<DispatchRun, DispatchError>
    where
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
            let run = self.run(entry, turn_budget, ctx, mem);
            if run.instructions > remaining {
                return Err(DispatchError::RunnerExceededBudget {
                    at: entry,
                    budget: turn_budget,
                    actual: run.instructions,
                });
            }
            let continuing_without_progress = run.instructions == 0
                && matches!(
                    run.exit,
                    BlockExit::Transfer(_)
                        | BlockExit::ResolveTransfer { .. }
                        | BlockExit::ResolveCall { .. }
                        | BlockExit::Fault(CpuFault {
                            kind: CpuFaultKind::Exception { .. },
                            ..
                        })
                );
            if continuing_without_progress {
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

            let resolution = match run.exit {
                BlockExit::Transfer(next) => {
                    entry = next;
                    continue;
                }
                BlockExit::ResolveTransfer {
                    source_bank,
                    target_pc,
                } => resolver.resolve(source_bank, target_pc),
                BlockExit::ResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => match resolver.resolve_call(source_bank, target_pc, resume) {
                    Ok(CallResolution::Guest(next)) => {
                        entry = next;
                        continue;
                    }
                    Ok(CallResolution::Host) => {
                        return Ok(DispatchRun {
                            exit: BlockExit::HostCall {
                                vram: target_pc,
                                resume,
                            },
                            instructions,
                            blocks,
                        });
                    }
                    Err(fault) => Err(fault),
                },
                BlockExit::Fault(fault) => {
                    let Some(vector) = fault.enter_exception(ctx) else {
                        return Ok(DispatchRun {
                            exit: run.exit,
                            instructions,
                            blocks,
                        });
                    };
                    resolver.resolve(fault.at.bank, vector)
                }
                exit => {
                    return Ok(DispatchRun {
                        exit,
                        instructions,
                        blocks,
                    });
                }
            };

            match resolution {
                Ok(next) => entry = next,
                Err(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::Fault(fault),
                        instructions,
                        blocks,
                    });
                }
            }
        }
    }
}

/// Failure to publish a new executable generation into a fixed virtual
/// region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationError {
    RegionMismatch {
        region_start: GuestPc,
        region_end: GuestPc,
        bank_start: GuestPc,
        bank_end: GuestPc,
    },
    Program(ProgramError),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::RegionMismatch {
                region_start,
                region_end,
                bank_start,
                bank_end,
            } => write!(
                f,
                "executable generation [{bank_start}, {bank_end}) does not exactly replace region [{region_start}, {region_end})"
            ),
            Self::Program(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for GenerationError {}

/// One virtual code region with exactly one active immutable generation.
///
/// Installing a replacement removes the old `CodeBank` and generated runner
/// together before publishing the new pair. The region therefore never
/// resolves stale code by virtual address after a successful rewrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutableRegion {
    start: GuestPc,
    end: GuestPc,
    active: Option<BankId>,
}

impl ExecutableRegion {
    pub fn new(start: GuestPc, end: GuestPc) -> Self {
        assert!(start < end, "executable region must be nonempty");
        assert!(
            start.is_instruction_aligned() && end.is_instruction_aligned(),
            "executable region bounds must be instruction-aligned"
        );
        Self {
            start,
            end,
            active: None,
        }
    }

    pub const fn active_bank(self) -> Option<BankId> {
        self.active
    }

    pub const fn start(self) -> GuestPc {
        self.start
    }

    pub const fn end(self) -> GuestPc {
        self.end
    }

    pub fn resolve(self, pc: GuestPc) -> Option<ExecutionKey> {
        if pc < self.start || pc >= self.end {
            return None;
        }
        self.active.map(|bank| ExecutionKey::new(bank, pc))
    }

    pub fn install(
        &mut self,
        program: &mut BlockProgram,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<Option<BankId>, GenerationError> {
        if code.vram_start() != self.start || code.vram_end() != self.end {
            return Err(GenerationError::RegionMismatch {
                region_start: self.start,
                region_end: self.end,
                bank_start: code.vram_start(),
                bank_end: code.vram_end(),
            });
        }
        let bank = code.id();
        if runner.bank() != bank {
            return Err(GenerationError::Program(ProgramError::RunnerBankMismatch {
                code_bank: bank,
                runner_bank: runner.bank(),
            }));
        }
        if program.code().bank(bank).is_some() {
            return Err(GenerationError::Program(ProgramError::DuplicateBank {
                bank,
            }));
        }

        let retired = self.active;
        if let Some(previous) = retired {
            assert!(
                program.unregister(previous),
                "active executable region referenced missing generation {previous}"
            );
        }
        program
            .register(code, runner)
            .map_err(GenerationError::Program)?;
        self.active = Some(bank);
        Ok(retired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronous_exception_entry_sets_epc_bd_exl_cause_and_vector() {
        let bank = BankId::new(7);
        let mut ctx = RecompContext::new();
        ctx.cop0_cause = 0x0000_0100; // preserve an unrelated pending bit
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Breakpoint,
                epc: GuestPc::new(0x8000_1000),
                branch_delay: true,
                instruction_code: 7,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1000);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 9);
        assert_ne!(ctx.cop0_cause & 0x100, 0);
    }

    #[test]
    fn nested_exception_preserves_first_epc_bd_and_bev_selects_boot_vector() {
        let bank = BankId::new(8);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = (1 << 1) | (1 << 22); // EXL + BEV
        ctx.cop0_epc = 0x8000_2000;
        ctx.cop0_cause = 1 << 31;
        let nested = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_3000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::Syscall,
                epc: GuestPc::new(0x8000_3000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            nested.enter_exception(&mut ctx),
            Some(GuestPc::new(0xBFC0_0380))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_2000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 8);
    }

    #[test]
    fn address_exception_commits_badvaddr_and_architectural_cause_code() {
        let bank = BankId::new(9);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0001),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0001);
        assert_eq!(ctx.cop0_epc, 0x8000_4000);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 4);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
    }

    #[test]
    fn nested_address_exception_updates_badvaddr_without_replacing_epc_or_bd() {
        let bank = BankId::new(10);
        let mut ctx = RecompContext::new();
        ctx.cop0_status = 1 << 1;
        ctx.cop0_epc = 0x8000_5000;
        ctx.cop0_cause = 1 << 31;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6004)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: Some(0x8000_0002),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x8000_0002);
        assert_eq!(ctx.cop0_epc, 0x8000_5000);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 5);
    }

    #[test]
    fn coprocessor_unusable_exception_records_cause_ce() {
        let bank = BankId::new(11);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_7000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::CoprocessorUnusable,
                epc: GuestPc::new(0x8000_7000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: Some(1),
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 11);
        assert_eq!((ctx.cop0_cause >> 28) & 0b11, 1);
        assert_eq!(ctx.cop0_epc, 0x8000_7000);
    }

    #[test]
    fn level_sensitive_interrupt_entry_obeys_ie_im_exl_and_erl() {
        let mut ctx = RecompContext::new();
        let interrupted = GuestPc::new(0x8000_1000);
        CpuInterruptLine::RCP.set_level(&mut ctx, true);
        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        ctx.cop0_status = 1 | CpuInterruptLine::RCP.cause_bit();
        ctx.cop0_cause |= (9 << 2) | (1 << 31);
        assert_eq!(
            enter_pending_interrupt(&mut ctx, interrupted),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, interrupted.get());
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1F, 0);
        assert_eq!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);

        assert_eq!(enter_pending_interrupt(&mut ctx, interrupted), None);
        CpuInterruptLine::RCP.set_level(&mut ctx, false);
        assert_eq!(ctx.cop0_cause & CpuInterruptLine::RCP.cause_bit(), 0);
    }

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

    fn observation_transfer_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        match entry {
            key if key == ExecutionKey::new(first_bank, VA) => BlockRun::new(
                BlockExit::Transfer(ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4))),
                1,
            ),
            key if key == ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4)) => {
                BlockRun::new(
                    BlockExit::ResolveTransfer {
                        source_bank: first_bank,
                        target_pc: VA,
                    },
                    1,
                )
            }
            key if key == ExecutionKey::new(second_bank, VA) => {
                BlockRun::new(BlockExit::Yield(key), 1)
            }
            _ => unreachable!("observation runner received unexpected destination {entry}"),
        }
    }

    fn observation_host_call_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let bank = BankId::new(0x503);
        match entry.pc {
            pc if pc == VA => BlockRun::new(
                BlockExit::HostCall {
                    vram: GuestPc::new(0x8000_4000),
                    resume: ExecutionKey::new(bank, GuestPc::new(VA.get() + 4)),
                },
                1,
            ),
            pc if pc == GuestPc::new(VA.get() + 4) => BlockRun::new(BlockExit::Yield(entry), 1),
            _ => unreachable!("host-call runner received unexpected destination {entry}"),
        }
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
    fn executable_region_rewrite_retires_stale_bank_and_runner_atomically() {
        let first = BankId::new(0x101);
        let second = BankId::new(0x102);
        let mut program = BlockProgram::new();
        let mut region = ExecutableRegion::new(VA, GuestPc::new(VA.get() + 4));
        let mut storage = [0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(first, VA, vec![0x2402_0001]).unwrap(),
                    GeneratedBankRunner::new(first, first_runner),
                )
                .unwrap(),
            None
        );
        let first_key = region.resolve(VA).unwrap();
        assert_eq!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .instructions,
            1
        );
        assert_eq!(ctx.r_u32(2), 1);

        assert_eq!(
            region
                .install(
                    &mut program,
                    CodeBank::new(second, VA, vec![0x2402_0002]).unwrap(),
                    GeneratedBankRunner::new(second, second_runner),
                )
                .unwrap(),
            Some(first)
        );
        assert_eq!(region.active_bank(), Some(second));
        assert!(matches!(
            program
                .run(
                    first_key,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        let second_key = region.resolve(VA).unwrap();
        assert_eq!(second_key.bank, second);
        program.run(
            second_key,
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(ctx.r_u32(2), 2);
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
    fn block_program_observes_direct_transferred_and_resolved_entries_in_order() {
        let first_bank = BankId::new(0x501);
        let second_bank = BankId::new(0x502);
        let first_artifact = ProgramArtifactIdentity::new([0x51; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x52; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(first_bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    first_bank,
                    observation_transfer_runner,
                    first_artifact,
                ),
            )
            .unwrap();
        program
            .register(
                CodeBank::new(second_bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    second_bank,
                    observation_transfer_runner,
                    second_artifact,
                ),
            )
            .unwrap();

        let immutable_before = program.evidence_snapshot();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program
            .code()
            .resolve(ExecutionKey::new(first_bank, VA))
            .is_ok());
        assert!(program.copy_execution_destinations().is_empty());

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |source_bank: BankId, target_pc: GuestPc| {
            assert_eq!(source_bank, first_bank);
            assert_eq!(target_pc, VA);
            Ok(ExecutionKey::new(second_bank, target_pc))
        };
        let run = program
            .dispatch(
                ExecutionKey::new(first_bank, VA),
                InstructionBudget::new(6).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(second_bank, VA))
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, VA),
                    runner_artifact_identity: Some(first_artifact),
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(first_bank, GuestPc::new(VA.get() + 4),),
                    runner_artifact_identity: Some(first_artifact),
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(second_bank, VA),
                    runner_artifact_identity: Some(second_artifact),
                },
            ]
        );
        assert_eq!(
            immutable_before,
            program.evidence_snapshot(),
            "historical execution must not enter future-affecting program evidence"
        );
    }

    #[test]
    fn block_program_records_host_resume_only_when_guest_execution_reenters() {
        let bank = BankId::new(0x503);
        let artifact = ProgramArtifactIdentity::new([0x53; 32]);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0, 0]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    bank,
                    observation_host_call_runner,
                    artifact,
                ),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let mut resolver = |_source_bank: BankId, _target_pc: GuestPc| {
            unreachable!("host-call fixture must not resolve a guest transfer")
        };

        let first = program
            .dispatch(
                ExecutionKey::new(bank, VA),
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        let resume = match first.exit {
            BlockExit::HostCall { resume, .. } => resume,
            exit => panic!("expected host call, got {exit:?}"),
        };
        assert_eq!(program.copy_execution_destinations().len(), 1);

        let second = program
            .dispatch(
                resume,
                InstructionBudget::new(4).unwrap(),
                &mut ctx,
                &mut mem,
                &mut resolver,
            )
            .unwrap();
        assert_eq!(second.exit, BlockExit::Yield(resume));
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, VA),
                    runner_artifact_identity: Some(artifact),
                },
                ExecutionDestinationObservation {
                    destination: resume,
                    runner_artifact_identity: Some(artifact),
                },
            ]
        );
    }

    #[test]
    fn block_program_observation_lifetime_is_explicit_and_program_local() {
        let bank = BankId::new(0x504);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(bank, first_runner),
            )
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        program.run(
            ExecutionKey::new(bank, VA),
            InstructionBudget::new(2).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, VA),
                runner_artifact_identity: None,
            }]
        );
        assert!(BlockProgram::new().copy_execution_destinations().is_empty());
        program.clear_execution_destinations();
        assert!(program.copy_execution_destinations().is_empty());
        assert!(program.code().bank(bank).is_some());
    }

    #[test]
    fn block_program_evidence_is_sorted_and_runner_pointer_independent() {
        let first = BankId::new(0x21);
        let second = BankId::new(0x22);
        let artifact = ProgramArtifactIdentity::new([0xA5; 32]);
        let mut forward = BlockProgram::new();
        forward
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, first_runner, artifact),
            )
            .unwrap();
        forward
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, second_runner, artifact),
            )
            .unwrap();

        let mut reverse_with_different_runners = BlockProgram::new();
        reverse_with_different_runners
            .register(
                CodeBank::new(second, GuestPc::new(VA.get() + 0x40), vec![0x3333]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(second, first_runner, artifact),
            )
            .unwrap();
        reverse_with_different_runners
            .register(
                CodeBank::new(first, VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(first, second_runner, artifact),
            )
            .unwrap();

        let snapshot = forward.evidence_snapshot();
        assert_eq!(snapshot, reverse_with_different_runners.evidence_snapshot());
        assert_eq!(
            snapshot.identity.source,
            ProgramIdentitySource::CanonicalBlockProgramSha256
        );
        assert_eq!(
            snapshot
                .banks
                .iter()
                .map(|bank| bank.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
    }

    #[test]
    fn block_program_identity_binds_bank_span_and_instruction_families() {
        fn snapshot(id: BankId, start: GuestPc, words: Vec<u32>) -> BlockProgramEvidenceSnapshot {
            let mut program = BlockProgram::new();
            program
                .register(
                    CodeBank::new(id, start, words).unwrap(),
                    GeneratedBankRunner::new_with_artifact_identity(
                        id,
                        first_runner,
                        ProgramArtifactIdentity::new([0xC3; 32]),
                    ),
                )
                .unwrap();
            program.evidence_snapshot()
        }

        let baseline = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2222]);
        let changed_bank = snapshot(BankId::new(0x32), VA, vec![0x1111, 0x2222]);
        let changed_span = snapshot(
            BankId::new(0x31),
            GuestPc::new(VA.get() + 4),
            vec![0x1111, 0x2222],
        );
        let changed_word = snapshot(BankId::new(0x31), VA, vec![0x1111, 0x2223]);

        for changed in [&changed_bank, &changed_span, &changed_word] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }

        let mut changed_runner_artifact = BlockProgram::new();
        changed_runner_artifact
            .register(
                CodeBank::new(BankId::new(0x31), VA, vec![0x1111, 0x2222]).unwrap(),
                GeneratedBankRunner::new_with_artifact_identity(
                    BankId::new(0x31),
                    first_runner,
                    ProgramArtifactIdentity::new([0x3C; 32]),
                ),
            )
            .unwrap();
        let changed_runner_artifact = changed_runner_artifact.evidence_snapshot();
        assert_ne!(baseline, changed_runner_artifact);
        assert_ne!(
            baseline.identity.identity,
            changed_runner_artifact.identity.identity
        );
    }

    #[test]
    #[should_panic(expected = "stable artifact identity for generated runner")]
    fn block_program_evidence_rejects_unidentified_runner_artifact() {
        let id = BankId::new(0x41);
        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(id, VA, vec![0]).unwrap(),
                GeneratedBankRunner::new(id, first_runner),
            )
            .unwrap();
        let _ = program.evidence_snapshot();
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
        assert!(program.copy_execution_destinations().is_empty());

        let unknown = ExecutionKey::new(BankId::new(0xDEAD), VA);
        assert!(matches!(
            program
                .run(
                    unknown,
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        assert!(program.copy_execution_destinations().is_empty());
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
        assert_eq!(fault, CpuFault::instruction_address_error(unaligned));
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
