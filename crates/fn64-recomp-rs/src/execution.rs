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

use crate::fetch::{
    admit_mapped_unit, run_admitted_mapped_unit, MappedAotBlock, MappedAotEvidenceSnapshot,
    PhysicalCodeBank, PhysicalCodeBankEvidenceSnapshot, PhysicalCodeCatalog, PhysicalCodeError,
};
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

/// Identity of one admitted physical instruction word.
///
/// `BankId` is the immutable image/generation evidence; `physical_address`
/// selects the word inside that generation. This is intentionally not an
/// [`ExecutionKey`]: branch arithmetic, link registers, EPC, and Cause.BD use
/// the architectural virtual PC even when two VAs name this same identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstructionWordIdentity {
    pub bank: BankId,
    pub physical_address: u32,
}

impl InstructionWordIdentity {
    pub const fn new(bank: BankId, physical_address: u32) -> Self {
        Self {
            bank,
            physical_address,
        }
    }
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
    /// VA translation succeeded, but that physical word was not admitted in
    /// the selected immutable generation.
    UnmappedPhysicalInstruction {
        physical_address: u32,
    },
    /// A translated AOT unit was entered after its VA-to-physical binding
    /// changed. Retrying stale native code would execute the wrong word, so
    /// this remains a loud generation boundary for the mapping owner to
    /// rebuild and re-resolve.
    StaleInstructionIdentity {
        expected: InstructionWordIdentity,
        actual: InstructionWordIdentity,
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
        bad_vaddr: Option<u64>,
        coprocessor: Option<u8>,
    },
}

/// Architecturally defined synchronous exceptions currently produced by the
/// arbitrary-PC lane. Coprocessor and TLB exceptions join this enum as their
/// instruction paths stop using host panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuException {
    TlbModified,
    TlbRefillLoad,
    TlbRefillStore,
    XTlbRefillLoad,
    XTlbRefillStore,
    TlbInvalidLoad,
    TlbInvalidStore,
    AddressErrorLoad,
    AddressErrorStore,
    CoprocessorUnusable,
    Syscall,
    Breakpoint,
    Trap,
    IntegerOverflow,
    /// An enabled COP1 (FPU) IEEE exception. The VR4300 raises ExcCode 15 (FPE)
    /// through the general exception vector when an arithmetic/conversion op sets
    /// an FCSR Cause bit whose matching Enable bit is set. Unlike
    /// [`Self::CoprocessorUnusable`] (ExcCode 11) it does NOT set Cause.CE — FPE
    /// is a normal general exception, and the handler reads FCSR.Cause to learn
    /// which IEEE condition trapped.
    FloatingPoint,
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
            CpuFaultKind::UnmappedPhysicalInstruction { physical_address } => write!(
                f,
                "physical instruction word {physical_address:#010X} is not admitted at {}",
                self.at
            ),
            CpuFaultKind::StaleInstructionIdentity { expected, actual } => write!(
                f,
                "stale translated instruction at {}: expected {}:{:#010X}, fetched {}:{:#010X}",
                self.at,
                expected.bank,
                expected.physical_address,
                actual.bank,
                actual.physical_address
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
            Self::TlbModified => 1,
            Self::TlbRefillLoad | Self::XTlbRefillLoad | Self::TlbInvalidLoad => 2,
            Self::TlbRefillStore | Self::XTlbRefillStore | Self::TlbInvalidStore => 3,
            Self::AddressErrorLoad => 4,
            Self::AddressErrorStore => 5,
            Self::Syscall => 8,
            Self::Breakpoint => 9,
            Self::CoprocessorUnusable => 11,
            Self::IntegerOverflow => 12,
            Self::Trap => 13,
            Self::FloatingPoint => 15,
        }
    }

    const fn is_tlb_exception(self) -> bool {
        matches!(
            self,
            Self::TlbModified
                | Self::TlbRefillLoad
                | Self::TlbRefillStore
                | Self::XTlbRefillLoad
                | Self::XTlbRefillStore
                | Self::TlbInvalidLoad
                | Self::TlbInvalidStore
        )
    }

    const fn is_tlb_refill(self) -> bool {
        matches!(
            self,
            Self::TlbRefillLoad
                | Self::TlbRefillStore
                | Self::XTlbRefillLoad
                | Self::XTlbRefillStore
        )
    }

    const fn is_xtlb_refill(self) -> bool {
        matches!(self, Self::XTlbRefillLoad | Self::XTlbRefillStore)
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
                bad_vaddr: Some(at.pc.get() as u64),
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

        let was_exl = ctx.cop0_status & STATUS_EXL != 0;
        if !was_exl {
            ctx.cop0_epc = epc.get();
            if branch_delay {
                ctx.cop0_cause |= CAUSE_BD;
            } else {
                ctx.cop0_cause &= !CAUSE_BD;
            }
        }
        if let Some(bad_vaddr) = bad_vaddr {
            ctx.cop0_badvaddr = bad_vaddr;
            if exception.is_tlb_exception() {
                // VR4300 User's Manual TLB exception processing: Context gets
                // VA[31:13] as BadVPN2, XContext gets Region plus VA[39:13],
                // and EntryHi gets Region/VPN2. Both context registers retain
                // their software-owned PTEBase and EntryHi retains ASID.
                let low = bad_vaddr as u32;
                ctx.cop0_context = (ctx.cop0_context & 0xff80_0000) | ((low >> 9) & 0x007f_fff0);
                ctx.cop0_xcontext = (ctx.cop0_xcontext & 0xffff_fffe_0000_0000)
                    | ((bad_vaddr >> 31) & 0x0000_0001_8000_0000)
                    | ((bad_vaddr >> 9) & 0x0000_0000_7fff_fff0);
                ctx.cop0_entry_hi =
                    (bad_vaddr & 0xc000_00ff_ffff_e000) | (ctx.cop0_entry_hi & 0xff);
            }
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

        let refill_vector = exception.is_tlb_refill() && !was_exl;
        let extended_refill_vector = exception.is_xtlb_refill() && !was_exl;
        Some(GuestPc::new(if ctx.cop0_status & STATUS_BEV != 0 {
            if extended_refill_vector {
                0xBFC0_0280
            } else if refill_vector {
                0xBFC0_0200
            } else {
                0xBFC0_0380
            }
        } else if extended_refill_vector {
            0x8000_0080
        } else if refill_vector {
            0x8000_0000
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
    /// A committed store changed the active executable image. The owner must
    /// publish the replacement generation before resolving `resume` again.
    ExecutableWrite {
        source_bank: BankId,
        resume: ExecutionKey,
    },
    /// An executable-changing store occurred in the delay slot of a call whose
    /// target still needs guest-versus-host classification. A dispatcher must
    /// resolve that classification without entering either target first.
    ExecutableWriteResolveCall {
        source_bank: BankId,
        target_pc: GuestPc,
        resume: ExecutionKey,
    },
    /// A delay-slot store changed executable bytes before the selected target
    /// raised an architectural fetch fault. Exception state must be applied,
    /// but its handler may not execute until the replacement generation is
    /// visible.
    ExecutableWriteFault(CpuFault),
    Checkpoint(ExecutionKey),
    Yield(ExecutionKey),
    /// The guest thread entry returned through its configured sentinel. This
    /// is distinct from an unmapped-PC fault: live runtimes may only finish a
    /// coroutine when generated code or an explicit return adapter emits this
    /// boundary.
    ThreadReturn,
    Fault(CpuFault),
}

/// Drain a request left by a store in a control transfer's delay slot and
/// preserve the selected continuation without entering it.
///
/// Straight-line runners consume the request at `PC + 4` so they can stop
/// before their own loop advances. Control transfers already return a typed
/// exit after the indivisible branch/delay pair; this conversion makes direct
/// runner invocation just as leak-free as dispatcher-driven invocation.
pub fn finalize_executable_write_exit(source_bank: BankId, exit: BlockExit) -> BlockExit {
    if !crate::runtime::take_executable_write_boundary() {
        return exit;
    }
    match exit {
        BlockExit::Transfer(next) => BlockExit::ExecutableWrite {
            source_bank,
            resume: next,
        },
        BlockExit::ResolveTransfer {
            source_bank,
            target_pc,
        } => BlockExit::ExecutableWrite {
            source_bank,
            resume: ExecutionKey::new(source_bank, target_pc),
        },
        BlockExit::ResolveCall {
            source_bank,
            target_pc,
            resume,
        } => BlockExit::ExecutableWriteResolveCall {
            source_bank,
            target_pc,
            resume,
        },
        BlockExit::Fault(fault) => BlockExit::ExecutableWriteFault(fault),
        // Each of these already returns to the host owner before another guest
        // instruction can execute. Draining the request prevents it from
        // contaminating a later direct runner invocation; the host processes
        // committed executable writes at every such outer boundary.
        outer => outer,
    }
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

    pub const fn callable(self) -> GeneratedBankFn {
        self.run
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
        let run = BlockRun::new(
            finalize_executable_write_exit(entry.bank, run.exit),
            run.instructions,
        );
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
                    | BlockExit::ExecutableWrite { .. }
                    | BlockExit::ExecutableWriteResolveCall { .. }
                    | BlockExit::ExecutableWriteFault(_)
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
            BlockExit::ExecutableWrite {
                source_bank,
                resume,
            } => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWrite {
                        source_bank,
                        resume,
                    },
                    instructions,
                    blocks,
                });
            }
            BlockExit::ExecutableWriteResolveCall {
                source_bank,
                target_pc,
                resume,
            } => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWriteResolveCall {
                        source_bank,
                        target_pc,
                        resume,
                    },
                    instructions,
                    blocks,
                });
            }
            BlockExit::ExecutableWriteFault(fault) => {
                return Ok(DispatchRun {
                    exit: BlockExit::ExecutableWriteFault(fault),
                    instructions,
                    blocks,
                });
            }
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
/// Virtual and physical banks, spans, and mapped AOT entries are sorted by
/// their typed identities/addresses. Instruction word order is architectural
/// and is retained verbatim. Generated runner pointers are deliberately
/// absent, but each generated unit retains its stable artifact identity: the
/// words alone cannot prove two native callables implement the same semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockProgramEvidenceSnapshot {
    pub identity: ProgramIdentityEvidenceSnapshot,
    pub banks: Vec<CodeBankEvidenceSnapshot>,
    pub physical_banks: Vec<PhysicalCodeBankEvidenceSnapshot>,
    pub mapped_aot: Vec<MappedAotEvidenceSnapshot>,
}

/// One successfully entered bank-qualified guest execution destination.
///
/// The bank identity names the immutable code-image generation, while the
/// optional runner identity names the generated native artifact that was
/// actually entered. `None` is retained for the compatibility
/// [`GeneratedBankRunner::new`] path and the mapped-interpreter fallback;
/// neither may be promoted to release evidence without a typed artifact
/// authority.
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
    PhysicalCode(PhysicalCodeError),
    DuplicateMappedEntry {
        entry: ExecutionKey,
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
            Self::PhysicalCode(error) => error.fmt(f),
            Self::DuplicateMappedEntry { entry } => {
                write!(f, "block program already contains mapped AOT entry {entry}")
            }
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
    physical_code: PhysicalCodeCatalog,
    mapped_aot: BTreeMap<ExecutionKey, MappedAotBlock>,
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
        if self.code.bank(code_bank).is_some()
            || self.runners.contains_key(&code_bank)
            || self.physical_code.contains_bank(code_bank)
        {
            return Err(ProgramError::DuplicateBank { bank: code_bank });
        }
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.runners
            .insert(code_bank, (runner.run, runner.artifact_identity));
        Ok(())
    }

    /// Admit one immutable physical code generation for canonical 32-bit
    /// mapped fetch. Every aligned VA resolved to this `BankId` can execute
    /// immediately through the interpreter fallback; registered mapped AOT
    /// units override individual entries without changing the fetch contract.
    pub fn register_physical_code(&mut self, code: PhysicalCodeBank) -> Result<(), ProgramError> {
        let bank = code.id();
        if self.code.bank(bank).is_some() || self.runners.contains_key(&bank) {
            return Err(ProgramError::DuplicateBank { bank });
        }
        self.physical_code
            .register(code)
            .map_err(ProgramError::PhysicalCode)
    }

    /// Install one fetch-bound generated unit into the main program runner.
    /// The containing physical generation must already be registered so no
    /// optional side catalog can become a second execution authority.
    pub fn register_mapped_aot(&mut self, block: MappedAotBlock) -> Result<(), ProgramError> {
        let entry = ExecutionKey::new(block.bank(), block.entry());
        assert!(
            self.physical_code.contains_bank(block.bank()),
            "mapped AOT entry {entry} has no admitted physical code generation"
        );
        if self.mapped_aot.contains_key(&entry) {
            return Err(ProgramError::DuplicateMappedEntry { entry });
        }
        self.mapped_aot.insert(entry, block);
        Ok(())
    }

    pub fn code(&self) -> &CodeCatalog {
        &self.code
    }

    pub fn physical_code(&self) -> &PhysicalCodeCatalog {
        &self.physical_code
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
    /// Catalog maps sort bank/AOT identities and bank construction sorts
    /// spans, so equivalent registration order produces byte-identical
    /// evidence. The domain-separated SHA-256 covers every virtual and
    /// physical bank identity, span address, length, instruction word, mapped
    /// entry, translated instruction identity, and runner artifact identity,
    /// all encoded big-endian. Code words alone are insufficient because
    /// registration accepts independently generated native runners.
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
        let physical_banks = self.physical_code.evidence_snapshot();
        let mapped_aot = self
            .mapped_aot
            .values()
            .map(MappedAotBlock::evidence_snapshot)
            .collect::<Vec<_>>();
        let mut hasher = Sha256::new();
        if physical_banks.is_empty() {
            hasher.update(b"fn64.block-program.identity.v1\0");
        } else {
            hasher.update(b"fn64.block-program.identity.v2\0");
        }
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
        if !physical_banks.is_empty() {
            hasher.update(
                u64::try_from(physical_banks.len())
                    .expect("physical block-program bank count exceeds identity wire")
                    .to_be_bytes(),
            );
            for bank in &physical_banks {
                hasher.update(bank.id.get().to_be_bytes());
                hasher.update(
                    u64::try_from(bank.spans.len())
                        .expect("physical block-program span count exceeds identity wire")
                        .to_be_bytes(),
                );
                for span in &bank.spans {
                    hasher.update(span.physical_start.to_be_bytes());
                    hasher.update(
                        u64::try_from(span.words.len())
                            .expect("physical block-program word count exceeds identity wire")
                            .to_be_bytes(),
                    );
                    for word in &span.words {
                        hasher.update(word.to_be_bytes());
                    }
                }
            }
            hasher.update(
                u64::try_from(mapped_aot.len())
                    .expect("mapped AOT unit count exceeds identity wire")
                    .to_be_bytes(),
            );
            for unit in &mapped_aot {
                hasher.update(unit.entry.bank.get().to_be_bytes());
                hasher.update(unit.entry.pc.get().to_be_bytes());
                hasher.update(unit.runner_artifact_identity.bytes());
                hasher.update(
                    u64::try_from(unit.instructions.len())
                        .expect("mapped AOT instruction count exceeds identity wire")
                        .to_be_bytes(),
                );
                for instruction in &unit.instructions {
                    hasher.update(instruction.bank.get().to_be_bytes());
                    hasher.update(instruction.physical_address.to_be_bytes());
                }
                hasher.update(
                    u64::try_from(unit.expected_words.len())
                        .expect("mapped AOT expected-word count exceeds identity wire")
                        .to_be_bytes(),
                );
                for word in &unit.expected_words {
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
            physical_banks,
            mapped_aot,
        }
    }

    /// Atomically retire one immutable code generation and its callable.
    /// Returning `false` means neither half existed; a one-sided presence is
    /// an internal invariant violation rather than a recoverable stale state.
    pub fn unregister(&mut self, bank: BankId) -> bool {
        if let Some(_physical) = self.physical_code.unregister(bank) {
            self.mapped_aot.retain(|entry, _| entry.bank != bank);
            return true;
        }
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
        if self.physical_code.contains_bank(entry.bank) {
            if let Some(block) = self.mapped_aot.get(&entry) {
                if let Err(run) = block.preflight(&self.physical_code, ctx) {
                    return run;
                }
                self.execution_destinations
                    .borrow_mut()
                    .push(ExecutionDestinationObservation {
                        destination: entry,
                        runner_artifact_identity: block.runner_artifact_identity(),
                    });
                return block.run_preflighted(budget, ctx, mem);
            }
            let unit = match admit_mapped_unit(&self.physical_code, entry.bank, entry.pc, ctx) {
                Ok(unit) => unit,
                Err(run) => return run,
            };
            self.execution_destinations
                .borrow_mut()
                .push(ExecutionDestinationObservation {
                    destination: entry,
                    runner_artifact_identity: None,
                });
            return run_admitted_mapped_unit(unit, budget, ctx, mem).unwrap_or_else(
                |unsupported| BlockRun::new(BlockExit::Fault(unsupported.into_cpu_fault()), 0),
            );
        }
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
            let run = BlockRun::new(
                finalize_executable_write_exit(entry.bank, run.exit),
                run.instructions,
            );
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
                        | BlockExit::ExecutableWrite { .. }
                        | BlockExit::ExecutableWriteResolveCall { .. }
                        | BlockExit::ExecutableWriteFault(_)
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
                BlockExit::ExecutableWrite {
                    source_bank,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWrite {
                            source_bank,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteResolveCall {
                    source_bank,
                    target_pc,
                    resume,
                } => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteResolveCall {
                            source_bank,
                            target_pc,
                            resume,
                        },
                        instructions,
                        blocks,
                    });
                }
                BlockExit::ExecutableWriteFault(fault) => {
                    return Ok(DispatchRun {
                        exit: BlockExit::ExecutableWriteFault(fault),
                        instructions,
                        blocks,
                    });
                }
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
    fn floating_point_exception_enters_general_vector_with_exc_code_15() {
        let bank = BankId::new(0xF1);
        let mut ctx = RecompContext::new();
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_1804)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::FloatingPoint,
                epc: GuestPc::new(0x8000_1800),
                branch_delay: true,
                instruction_code: 0,
                bad_vaddr: None,
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_epc, 0x8000_1800);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 15);
        assert_ne!(ctx.cop0_cause & (1 << 31), 0);
        assert_ne!(ctx.cop0_status & (1 << 1), 0);
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
    fn tlb_refill_commits_translation_registers_and_selects_refill_vector() {
        let bank = BankId::new(0x71);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_entry_hi = 0x0000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0x1234_5678),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0000))
        );
        assert_eq!(ctx.cop0_badvaddr, 0x1234_5678);
        assert_eq!(ctx.cop0_context, 0xab89_1a20);
        assert_eq!(ctx.cop0_entry_hi, 0x1234_4042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0200))
        );
    }

    #[test]
    fn xtlb_refill_commits_full_translation_state_and_selects_extended_vector() {
        const BAD_VADDR: u64 = 0x4000_0088_7654_2040;
        let bank = BankId::new(0x73);
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::XTlbRefillLoad,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xab80_0000;
        ctx.cop0_xcontext = 0x1234_5678_0000_0000;
        ctx.cop0_entry_hi = 0x51;
        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0080))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context & 0xff80_0000, 0xab80_0000);
        assert_eq!(
            ctx.cop0_context & 0x007f_fff0,
            ((BAD_VADDR as u32) >> 9) & 0x007f_fff0
        );
        assert_eq!(
            ctx.cop0_xcontext & 0xffff_fffe_0000_0000,
            0x1234_5678_0000_0000 & 0xffff_fffe_0000_0000
        );
        assert_eq!((ctx.cop0_xcontext >> 31) & 0b11, BAD_VADDR >> 62);
        assert_eq!(
            (ctx.cop0_xcontext >> 4) & 0x07ff_ffff,
            (BAD_VADDR >> 13) & 0x07ff_ffff
        );
        assert_eq!(
            ctx.cop0_entry_hi,
            (BAD_VADDR & 0xc000_00ff_ffff_e000) | 0x51
        );
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 2);

        let mut bev_ctx = RecompContext::new();
        bev_ctx.cop0_status = 1 << 22;
        assert_eq!(
            fault.enter_exception(&mut bev_ctx),
            Some(GuestPc::new(0xbfc0_0280))
        );

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        assert_eq!(
            fault.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, BAD_VADDR);
    }

    #[test]
    fn extended_address_error_retains_full_badvaddr_without_tlb_state_updates() {
        const BAD_VADDR: u64 = 0x9000_0001_0000_0040;
        let bank = BankId::new(0x74);
        let mut ctx = RecompContext::new();
        ctx.cop0_context = 0xabcd_1234;
        ctx.cop0_xcontext = 0x1234_5678_9abc_def0;
        ctx.cop0_entry_hi = 0x4000_0042;
        let fault = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_4000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorStore,
                epc: GuestPc::new(0x8000_4000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(BAD_VADDR),
                coprocessor: None,
            },
        };

        assert_eq!(
            fault.enter_exception(&mut ctx),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(ctx.cop0_badvaddr, BAD_VADDR);
        assert_eq!(ctx.cop0_context, 0xabcd_1234);
        assert_eq!(ctx.cop0_xcontext, 0x1234_5678_9abc_def0);
        assert_eq!(ctx.cop0_entry_hi, 0x4000_0042);
        assert_eq!((ctx.cop0_cause >> 2) & 0x1f, 5);
    }

    #[test]
    fn invalid_modified_and_nested_refill_use_the_common_vector() {
        let bank = BankId::new(0x72);
        for (exception, expected_code) in [
            (CpuException::TlbInvalidStore, 3),
            (CpuException::TlbModified, 1),
        ] {
            let mut ctx = RecompContext::new();
            let fault = CpuFault {
                at: ExecutionKey::new(bank, GuestPc::new(0x8000_5000)),
                kind: CpuFaultKind::Exception {
                    exception,
                    epc: GuestPc::new(0x8000_5000),
                    branch_delay: false,
                    instruction_code: 0,
                    bad_vaddr: Some(0x0040_0000),
                    coprocessor: None,
                },
            };
            assert_eq!(
                fault.enter_exception(&mut ctx),
                Some(GuestPc::new(0x8000_0180))
            );
            assert_eq!((ctx.cop0_cause >> 2) & 0x1f, expected_code);
        }

        let mut nested = RecompContext::new();
        nested.cop0_status = 1 << 1;
        nested.cop0_epc = 0x8000_1234;
        let refill = CpuFault {
            at: ExecutionKey::new(bank, GuestPc::new(0x8000_6000)),
            kind: CpuFaultKind::Exception {
                exception: CpuException::TlbRefillStore,
                epc: GuestPc::new(0x8000_6000),
                branch_delay: false,
                instruction_code: 0,
                bad_vaddr: Some(0xc001_2345),
                coprocessor: None,
            },
        };
        assert_eq!(
            refill.enter_exception(&mut nested),
            Some(GuestPc::new(0x8000_0180))
        );
        assert_eq!(nested.cop0_epc, 0x8000_1234);
        assert_eq!(nested.cop0_badvaddr, 0xc001_2345);
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

    fn instruction_entry_lo(physical_page: u32, valid: bool) -> u32 {
        ((physical_page >> 6) & 0x03ff_ffc0) | 1 | ((valid as u32) << 1) | (1 << 2)
    }

    fn map_instruction_pair(
        ctx: &mut RecompContext,
        virtual_pair: u32,
        even_physical: u32,
        odd_physical: u32,
        odd_valid: bool,
    ) {
        ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: u64::from(virtual_pair & 0xffff_e000),
            entry_lo0: instruction_entry_lo(even_physical, true),
            entry_lo1: instruction_entry_lo(odd_physical, odd_valid),
        };
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

    fn zero_progress_executable_write_runner(
        entry: ExecutionKey,
        _budget: InstructionBudget,
        _ctx: &mut RecompContext,
        _mem: &mut Rdram<'_>,
    ) -> BlockRun {
        let resume = ExecutionKey::new(entry.bank, GuestPc::new(VA.get() + 12));
        let exit = match entry.pc.get() - VA.get() {
            0 => BlockExit::ExecutableWrite {
                source_bank: entry.bank,
                resume,
            },
            4 => BlockExit::ExecutableWriteResolveCall {
                source_bank: entry.bank,
                target_pc: GuestPc::new(0x8000_2000),
                resume,
            },
            8 => BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(entry)),
            _ => unreachable!("test runner received an unexpected entry"),
        };
        BlockRun::new(exit, 0)
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

    fn mapped_observation_bank(bank: BankId) -> PhysicalCodeBank {
        PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0000_0040, vec![0x4022_4800]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![0x1000_0001]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![0x2402_0002]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0030_0000, vec![0x2403_0003]).unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn mapped_fetch_failures_do_not_record_an_entered_destination() {
        let bank = BankId::new(0x505);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();

        let mut misaligned_ctx = RecompContext::new();
        let misaligned = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x8000_0042)),
            budget,
            &mut misaligned_ctx,
            &mut mem,
        );
        assert!(matches!(
            misaligned.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    ..
                },
                ..
            })
        ));

        let mut refill_ctx = RecompContext::new();
        let refill = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0060_0000)),
            budget,
            &mut refill_ctx,
            &mut mem,
        );
        assert!(matches!(
            refill.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbRefillLoad,
                    ..
                },
                ..
            })
        ));

        let mut unmapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut unmapped_ctx,
            0x0080_0000,
            0x0040_0000,
            0x0040_1000,
            true,
        );
        let unmapped = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0080_0000)),
            budget,
            &mut unmapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            unmapped.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPhysicalInstruction { .. },
                ..
            })
        ));

        let mut delay_ctx = RecompContext::new();
        map_instruction_pair(&mut delay_ctx, 0x0040_0000, 0x0010_0000, 0x0030_0000, false);
        let delay = program.run(
            ExecutionKey::new(bank, GuestPc::new(0x0040_0ffc)),
            budget,
            &mut delay_ctx,
            &mut mem,
        );
        assert!(matches!(
            delay.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    branch_delay: true,
                    ..
                },
                ..
            })
        ));
        assert!(program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_history_records_only_admitted_units_with_honest_lane_identity() {
        let bank = BankId::new(0x506);
        let mut program = BlockProgram::new();
        program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let aot_artifact = ProgramArtifactIdentity::new([0x56; 32]);
        let direct_aot_entry = GuestPc::new(0x8010_0000);
        let aot = MappedAotBlock::new(
            program.physical_code(),
            &RecompContext::new(),
            bank,
            direct_aot_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        program.register_mapped_aot(aot).unwrap();

        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);
        let budget = InstructionBudget::new(2).unwrap();
        let mut ctx = RecompContext::new();
        let interpreted_entry = GuestPc::new(0x8000_0040);
        let interpreted = program.run(
            ExecutionKey::new(bank, interpreted_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert!(matches!(interpreted.exit, BlockExit::Fault(_)));
        program.run(
            ExecutionKey::new(bank, direct_aot_entry),
            budget,
            &mut ctx,
            &mut mem,
        );
        assert_eq!(
            program.copy_execution_destinations(),
            vec![
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, interpreted_entry),
                    runner_artifact_identity: None,
                },
                ExecutionDestinationObservation {
                    destination: ExecutionKey::new(bank, direct_aot_entry),
                    runner_artifact_identity: Some(aot_artifact),
                },
            ]
        );

        let mut stale_program = BlockProgram::new();
        stale_program
            .register_physical_code(mapped_observation_bank(bank))
            .unwrap();
        let stale_entry = GuestPc::new(0x0080_0000);
        let mut original_ctx = RecompContext::new();
        map_instruction_pair(
            &mut original_ctx,
            stale_entry.get(),
            0x0010_0000,
            0x0030_0000,
            true,
        );
        let stale = MappedAotBlock::new(
            stale_program.physical_code(),
            &original_ctx,
            bank,
            stale_entry,
            &[0x2402_0001],
            GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, aot_artifact),
        )
        .unwrap();
        stale_program.register_mapped_aot(stale).unwrap();
        let mut remapped_ctx = RecompContext::new();
        map_instruction_pair(
            &mut remapped_ctx,
            stale_entry.get(),
            0x0020_0000,
            0x0030_0000,
            true,
        );
        let stale_run = stale_program.run(
            ExecutionKey::new(bank, stale_entry),
            budget,
            &mut remapped_ctx,
            &mut mem,
        );
        assert!(matches!(
            stale_run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(stale_program.copy_execution_destinations().is_empty());
    }

    #[test]
    fn mapped_wraparound_delay_fetch_is_precise_and_records_only_after_admission() {
        let bank = BankId::new(0x507);
        let branch_word = 0x1000_0001;
        let delay_word = 0x2442_0005;
        let physical = PhysicalCodeBank::from_spans(
            bank,
            vec![
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0010_0ffc, vec![branch_word]).unwrap(),
                crate::fetch::PhysicalCodeSpan::new(bank, 0x0020_0000, vec![delay_word]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();
        let entry = GuestPc::new(0xffff_fffc);
        let budget = InstructionBudget::new(2).unwrap();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        let mut invalid_ctx = RecompContext::new();
        for tlb in &mut invalid_ctx.tlb_entries {
            tlb.entry_hi = 0x0040_0000;
        }
        invalid_ctx.tlb_entries[0] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0xffff_e000,
            entry_lo0: instruction_entry_lo(0x0010_0000, true),
            entry_lo1: instruction_entry_lo(0x0010_0000, true),
        };
        invalid_ctx.tlb_entries[1] = crate::runtime::TlbEntryRaw {
            page_mask: 0,
            entry_hi: 0,
            entry_lo0: instruction_entry_lo(0x0020_0000, false),
            entry_lo1: instruction_entry_lo(0x0020_1000, false),
        };
        let invalid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut invalid_ctx,
            &mut mem,
        );
        assert!(matches!(
            invalid.exit,
            BlockExit::Fault(CpuFault {
                at: ExecutionKey {
                    pc: GuestPc(0),
                    ..
                },
                kind: CpuFaultKind::Exception {
                    exception: CpuException::TlbInvalidLoad,
                    epc,
                    branch_delay: true,
                    bad_vaddr: Some(0),
                    ..
                },
            }) if epc == entry
        ));
        assert!(program.copy_execution_destinations().is_empty());

        let mut valid_ctx = invalid_ctx;
        valid_ctx.tlb_entries[1].entry_lo0 = instruction_entry_lo(0x0020_0000, true);
        let valid = program.run(
            ExecutionKey::new(bank, entry),
            budget,
            &mut valid_ctx,
            &mut mem,
        );
        assert_eq!(valid.instructions, 2);
        assert_eq!(
            valid.exit,
            BlockExit::ResolveTransfer {
                source_bank: bank,
                target_pc: GuestPc::new(4),
            }
        );
        assert_eq!(valid_ctx.r_u32(2), 5);
        assert_eq!(
            program.copy_execution_destinations(),
            vec![ExecutionDestinationObservation {
                destination: ExecutionKey::new(bank, entry),
                runner_artifact_identity: None,
            }]
        );
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

    fn mapped_evidence_snapshot(
        bank: BankId,
        spans: &[(u32, u32)],
        mappings: &[(GuestPc, u32, u32, ProgramArtifactIdentity)],
    ) -> BlockProgramEvidenceSnapshot {
        let physical = PhysicalCodeBank::from_spans(
            bank,
            spans
                .iter()
                .map(|&(physical_start, word)| {
                    crate::fetch::PhysicalCodeSpan::new(bank, physical_start, vec![word]).unwrap()
                })
                .collect(),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program.register_physical_code(physical).unwrap();

        let mut ctx = RecompContext::new();
        for (index, &(entry, physical_address, word, artifact)) in mappings.iter().enumerate() {
            assert_eq!(entry.get() & 0x1fff, 0);
            assert_eq!(physical_address & 0xfff, 0);
            ctx.tlb_entries[index] = crate::runtime::TlbEntryRaw {
                page_mask: 0,
                entry_hi: u64::from(entry.get() & 0xffff_e000),
                entry_lo0: ((physical_address >> 6) & 0x03ff_ffc0) | 0x7,
                entry_lo1: 0,
            };
            let block = MappedAotBlock::new(
                program.physical_code(),
                &ctx,
                bank,
                entry,
                &[word],
                GeneratedBankRunner::new_with_artifact_identity(bank, first_runner, artifact),
            )
            .unwrap();
            program.register_mapped_aot(block).unwrap();
        }
        program.evidence_snapshot()
    }

    #[test]
    fn mapped_block_program_evidence_is_canonical_across_registration_order() {
        let bank = BankId::new(0x51);
        let first_entry = GuestPc::new(0x0040_0000);
        let second_entry = GuestPc::new(0x0040_2000);
        let first_word = 0x2402_0001;
        let second_word = 0x2403_0002;
        let first_artifact = ProgramArtifactIdentity::new([0x11; 32]);
        let second_artifact = ProgramArtifactIdentity::new([0x22; 32]);
        let forward = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, first_word), (0x0020_0000, second_word)],
            &[
                (first_entry, 0x0010_0000, first_word, first_artifact),
                (second_entry, 0x0020_0000, second_word, second_artifact),
            ],
        );
        let reverse = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, second_word), (0x0010_0000, first_word)],
            &[
                (second_entry, 0x0020_0000, second_word, second_artifact),
                (first_entry, 0x0010_0000, first_word, first_artifact),
            ],
        );

        assert_eq!(forward, reverse);
        assert_eq!(forward.physical_banks.len(), 1);
        assert_eq!(forward.mapped_aot.len(), 2);
    }

    #[test]
    fn mapped_block_program_identity_binds_physical_and_aot_identity_families() {
        let bank = BankId::new(0x61);
        let entry = GuestPc::new(0x0040_0000);
        let word = 0x2402_0001;
        let artifact = ProgramArtifactIdentity::new([0x33; 32]);
        let baseline = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_bank = mapped_evidence_snapshot(
            BankId::new(0x62),
            &[(0x0010_0000, word)],
            &[(entry, 0x0010_0000, word, artifact)],
        );
        let changed_physical_address = mapped_evidence_snapshot(
            bank,
            &[(0x0020_0000, word)],
            &[(entry, 0x0020_0000, word, artifact)],
        );
        let changed_entry = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(GuestPc::new(0x0040_2000), 0x0010_0000, word, artifact)],
        );
        let changed_word = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word + 1)],
            &[(entry, 0x0010_0000, word + 1, artifact)],
        );
        let changed_artifact = mapped_evidence_snapshot(
            bank,
            &[(0x0010_0000, word)],
            &[(
                entry,
                0x0010_0000,
                word,
                ProgramArtifactIdentity::new([0x44; 32]),
            )],
        );

        for changed in [
            &changed_bank,
            &changed_physical_address,
            &changed_entry,
            &changed_word,
            &changed_artifact,
        ] {
            assert_ne!(baseline, *changed);
            assert_ne!(baseline.identity.identity, changed.identity.identity);
        }
        assert_eq!(baseline.mapped_aot[0].entry.pc, entry);
        assert_eq!(
            baseline.mapped_aot[0].instructions,
            vec![InstructionWordIdentity::new(bank, 0x0010_0000)]
        );
        assert_eq!(baseline.mapped_aot[0].expected_words, vec![word]);
    }

    fn cross_catalog_mapped_program(compiled_word: u32) -> BlockProgram {
        let bank = BankId::new(0x63);
        let entry = GuestPc::new(0x8010_0000);
        let mut compilation_catalog = PhysicalCodeCatalog::new();
        compilation_catalog
            .register(PhysicalCodeBank::new(bank, 0x0010_0000, vec![compiled_word]).unwrap())
            .unwrap();
        let block = MappedAotBlock::new(
            &compilation_catalog,
            &RecompContext::new(),
            bank,
            entry,
            &[compiled_word],
            GeneratedBankRunner::new_with_artifact_identity(
                bank,
                first_runner,
                ProgramArtifactIdentity::new([0x63; 32]),
            ),
        )
        .unwrap();
        let mut program = BlockProgram::new();
        program
            .register_physical_code(
                PhysicalCodeBank::new(bank, 0x0010_0000, vec![0x2402_0001]).unwrap(),
            )
            .unwrap();
        program.register_mapped_aot(block).unwrap();
        program
    }

    #[test]
    fn mapped_aot_evidence_binds_future_preflight_expected_words() {
        let valid = cross_catalog_mapped_program(0x2402_0001);
        let stale = cross_catalog_mapped_program(0x2402_0002);
        let valid_snapshot = valid.evidence_snapshot();
        let stale_snapshot = stale.evidence_snapshot();
        assert_eq!(valid_snapshot.physical_banks, stale_snapshot.physical_banks);
        assert_eq!(
            valid_snapshot.mapped_aot[0].instructions,
            stale_snapshot.mapped_aot[0].instructions
        );
        assert_ne!(
            valid_snapshot.mapped_aot[0].expected_words,
            stale_snapshot.mapped_aot[0].expected_words
        );
        assert_ne!(
            valid_snapshot.identity.identity,
            stale_snapshot.identity.identity
        );

        let entry = ExecutionKey::new(BankId::new(0x63), GuestPc::new(0x8010_0000));
        let budget = InstructionBudget::new(2).unwrap();
        let mut valid_ctx = RecompContext::new();
        let mut stale_ctx = RecompContext::new();
        let mut valid_storage = [];
        let mut stale_storage = [];
        assert!(!matches!(
            valid
                .run(
                    entry,
                    budget,
                    &mut valid_ctx,
                    &mut Rdram::new(&mut valid_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
        assert!(matches!(
            stale
                .run(
                    entry,
                    budget,
                    &mut stale_ctx,
                    &mut Rdram::new(&mut stale_storage),
                )
                .exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::StaleInstructionIdentity { .. },
                ..
            })
        ));
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

    #[test]
    fn both_dispatchers_reject_zero_progress_executable_write_exits() {
        let bank_id = BankId::new(0x71);
        let budget = InstructionBudget::new(2).unwrap();
        let resume = ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 12));
        let entries_and_exits = [
            (
                ExecutionKey::new(bank_id, VA),
                BlockExit::ExecutableWrite {
                    source_bank: bank_id,
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 4)),
                BlockExit::ExecutableWriteResolveCall {
                    source_bank: bank_id,
                    target_pc: GuestPc::new(0x8000_2000),
                    resume,
                },
            ),
            (
                ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                BlockExit::ExecutableWriteFault(CpuFault::instruction_address_error(
                    ExecutionKey::new(bank_id, GuestPc::new(VA.get() + 8)),
                )),
            ),
        ];

        let mut program = BlockProgram::new();
        program
            .register(
                CodeBank::new(bank_id, VA, vec![0; 3]).unwrap(),
                GeneratedBankRunner::new(bank_id, zero_progress_executable_write_runner),
            )
            .unwrap();
        let mut ctx = RecompContext::new();
        let mut storage = [];
        let mut mem = Rdram::new(&mut storage);

        for (entry, exit) in entries_and_exits {
            let mut runner = move |_entry, _budget| BlockRun::new(exit, 0);
            let mut resolver = |_source_bank, _target_pc| unreachable!();
            assert_eq!(
                dispatch_until_boundary(entry, budget, &mut runner, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
            assert_eq!(
                program.dispatch(entry, budget, &mut ctx, &mut mem, &mut resolver),
                Err(DispatchError::ContinuingExitWithoutProgress { at: entry, exit })
            );
        }
    }

    #[test]
    fn executable_write_boundary_preserves_cross_bank_source_lineage() {
        fn changed(_: crate::runtime::GuestWriteEvent) -> crate::runtime::GuestWriteBoundary {
            crate::runtime::GuestWriteBoundary::ExecutableChanged
        }

        let source = BankId::new(0xA);
        let target = ExecutionKey::new(BankId::new(0xC), GuestPc::new(0x8000_4000));
        crate::runtime::set_guest_write_boundary_observer(Some(changed));
        crate::runtime::notify_guest_write(0x20, 4);
        assert_eq!(
            finalize_executable_write_exit(source, BlockExit::Transfer(target)),
            BlockExit::ExecutableWrite {
                source_bank: source,
                resume: target,
            }
        );
        assert!(!crate::runtime::take_executable_write_boundary());
        crate::runtime::set_guest_write_boundary_observer(None);
    }

    #[test]
    fn executable_write_special_continuations_escape_dispatch_unresolved() {
        let source = BankId::new(0xA);
        let target = GuestPc::new(0x8000_5000);
        let resume = ExecutionKey::new(source, GuestPc::new(0x8000_1008));
        let call = BlockExit::ExecutableWriteResolveCall {
            source_bank: source,
            target_pc: target,
            resume,
        };
        let mut call_runner = move |_entry, _budget| BlockRun::new(call, 2);
        let mut resolver = |_source_bank, _target_pc| -> Result<ExecutionKey, CpuFault> {
            panic!("executable-write continuation resolved before owner rebuild")
        };
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut call_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: call,
                instructions: 2,
                blocks: 1,
            }
        );

        let fault = CpuFault::instruction_address_error(ExecutionKey::new(
            source,
            GuestPc::new(0x8000_2002),
        ));
        let mut fault_runner =
            move |_entry, _budget| BlockRun::new(BlockExit::ExecutableWriteFault(fault), 3);
        assert_eq!(
            dispatch_until_boundary(
                ExecutionKey::new(source, VA),
                InstructionBudget::new(4).unwrap(),
                &mut fault_runner,
                &mut resolver,
            )
            .unwrap(),
            DispatchRun {
                exit: BlockExit::ExecutableWriteFault(fault),
                instructions: 3,
                blocks: 1,
            }
        );
    }
}
