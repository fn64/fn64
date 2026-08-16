//! Lane-neutral MIPS-III execution semantics shared by static and dynamic runners.
//!
//! The `dynamic_mips` fallback lane uses these semantics through the thin
//! `interp` module, while the compact static-micro-op lane calls the same
//! straight-instruction step without gaining dynamic fetch authority.
//!
//! The dynamic lane is an instrumented MIPS-III interpreter that
//! executes one admitted [`CodeBank`] behind the **same** [`BlockExit`] contract
//! the emitted AOT bank runner satisfies.
//!
//! # Why this exists
//!
//! Execution closure (`docs/UNIVERSAL-RUNTIME-PLAN.md`) requires that *every*
//! bank-qualified CPU destination runs. Where a static AOT admission cannot be
//! proven (bytes or targets produced only at runtime), the interpreter runs the
//! same words instrumented instead of faulting. For that fallback to be sound it
//! must be *architecturally indistinguishable* from the AOT lane: given the same
//! bank, entry, budget, [`RecompContext`], and [`Rdram`], it must leave
//! byte-identical `RecompContext` + `Rdram` state and return an identical
//! [`BlockRun`]. That equivalence is the correctness proof, exercised by the
//! differential integration test.
//!
//! # Shared ISA authority
//!
//! Instructions are decoded through [`crate::decoder::decode`] — the *same*
//! decoder the AOT emitter uses. The interpreter therefore never decodes more
//! weakly than the AOT lane: an encoding the decoder does not recognize becomes
//! the architectural reserved-instruction exception here, never a code-generation
//! failure, panic, or silent nop.
//!
//! # Scope (first slice)
//!
//! Covered: the ordinary integer/logical/shift ALU (32- and 64-bit), HI/LO
//! mult/div, every load/store width (including the `^2`/`^3` sub-word swizzle
//! and the LWL/LWR/LDL/LDR/SWL/… unaligned merges via the *same* [`Rdram`]
//! accessors), LL/SC reservations, all conditional branches (taken/not-taken and
//! the branch-likely annulment), J/JAL/JR/JALR with correct delay-slot semantics
//! (delay instruction executes before the transfer commits; a `jr`/`jalr` target
//! is snapshotted before a delay slot that overwrites the source register), the
//! `j self`/`b self` cooperative [`BlockExit::Yield`], deterministic
//! instruction-budget [`BlockExit::Checkpoint`]s that never split a branch/delay
//! pair, canonical 32-bit data translation through recorded TLB entries,
//! precise aligned-memory AdEL/AdES plus TLB refill/invalid/modified faults,
//! an access outside owned backing as a typed [`CpuFaultKind::MemoryFault`]
//! reusing the U4 checked `Rdram` accessors, and `ERET` (a privileged,
//! delay-slot-free transfer, handled the same as `emit_bank_eret` in
//! emit.rs: `RecompContext::exception_return_pc`'s ErrorEPC/ERL-over-EPC/EXL
//! precedence and LLbit clear, resolved as an unconditional
//! [`BlockExit::ResolveTransfer`]).
//!
//! COP1 transfers, memory operations, branches, arithmetic, conditional moves,
//! comparisons, and conversions share the AOT lane's softfloat/FCSR helpers;
//! disabled COP1 use becomes a typed Coprocessor Unusable fault. Their register
//! accesses use the physical-FGR model: FR=0 singles address each physical low
//! word, FR=0 doubles require an even register and join adjacent low words, and
//! FR=1 exposes each full 64-bit FGR. Explicitly OUT are COP2,
//! `SYSCALL`/`BREAK`, conditional traps, 64-bit instruction admission, typed
//! integer-overflow exceptions, and the remaining privileged-CPU surface.
//! Unsupported decoded instructions remain loud [`StepFault::Unsupported`]s.
//! Canonical 32-bit instruction translation is supplied by the one-unit
//! [`crate::fetch::run_mapped_bank`] wrapper, which fetches by physical identity
//! before constructing this interpreter's execution-local virtual view.
//! Modeled 32-bit COP0 moves, the inclusive Random/Wired instruction countdown,
//! and `TLBWI`/`TLBWR`/`TLBR`/`TLBP` share the typed context with the arbitrary-PC
//! AOT lane. The outer `BlockProgram` dispatcher applies the same typed CPU
//! faults to CP0 and selects the guest exception vector in either lane.

#![cfg_attr(
    not(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime")),
    allow(dead_code)
)]

use crate::decoder::{decode, Instruction};
use crate::execution::{
    BankId, BlockExit, BlockRun, CodeCatalog, CpuException, CpuFault, CpuFaultKind, ExecutionKey,
    GuestPc, InstructionBudget,
};
use crate::runtime::{
    DataAccessError, DataAccessKind, FpuException, Rdram, RecompContext, TranslatedDataAddress,
};

/// A hardware-register (KSEG1 MMIO) access the interpreter recognizes as
/// *not* backed RDRAM and routes to a modeled device instead of faulting.
///
/// # Why this is a trait, and why it lives here
///
/// The interpreter (`fn64-cpu-runtime`) must reach the runtime's modeled device
/// state (`fn64-runtime`'s `DeviceFabric`/`MmioSpace`) to give a guest MMIO
/// load a modeled register value and a guest MMIO store a modeled effect. But
/// `fn64-cpu-runtime` must not depend on `fn64-runtime` (the dependency edge runs
/// the other way — `docs/DESIGN.md` §1, and `fn64-runtime/Cargo.toml`'s note on
/// the one-way direction). So the seam is a *port trait* owned here and
/// *implemented* on the runtime side over the SAME peripheral state the AOT/
/// shim lanes use: there is no second device authority, only a typed door
/// through which the interpreter reaches the one that already exists.
///
/// # The load-bearing safety property lives in `None`
///
/// A port is the sole authority on which addresses are its MMIO window. A
/// `read_w`/`write_w` returning [`MmioOutcome::NotMmio`] means "this effective
/// address is not a register I model" — the interpreter then falls through to
/// the ordinary [`Rdram::try_load_w`]/[`Rdram::try_store_w`] accessor, which
/// faults typed if the address is also outside backed RDRAM. An MMIO window
/// therefore never makes an arbitrary out-of-RDRAM address "succeed": only the
/// exact register offsets the port claims are diverted; everything else stays a
/// [`CpuFaultKind::MemoryFault`], exactly as before this seam existed.
pub trait MmioPort {
    /// Word read of the KSEG1 effective address `vaddr` (full 64-bit,
    /// sign-extended, as the guest computed it).
    fn read_w(&mut self, vaddr: u64) -> MmioOutcome<u32>;
    /// Word store of `value` to the KSEG1 effective address `vaddr`.
    fn write_w(&mut self, vaddr: u64, value: u32) -> MmioOutcome<()>;
}

/// One architecturally aligned word address reached through a canonical direct
/// KSEG0/KSEG1 data translation.
///
/// The interpreter constructs this token only after its ordinary alignment and
/// address-translation checks succeed. A cartridge adapter can therefore
/// classify direct CPU cartridge accesses without accepting mapped/TLB or
/// noncanonical aliases. The adapter remains the sole authority for the actual
/// cartridge window; this crate deliberately contains no PI-domain constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlignedDirectWordAddress(u64);

impl AlignedDirectWordAddress {
    /// The canonical zero- or sign-extended effective address selected by
    /// translation.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn from_translated(address: u64) -> Option<Self> {
        let upper = address >> 32;
        let low = address as u32;
        ((address & 3 == 0)
            && (upper == 0 || upper == u32::MAX as u64)
            && (0x8000_0000..0xc000_0000).contains(&low))
        .then_some(Self(address))
    }
}

/// Result of offering a proven direct word load to cartridge storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartridgeReadOutcome {
    /// The address is outside this cartridge adapter's window.
    NotCartridge,
    /// The immutable cartridge supplied the architectural big-endian word.
    Handled(u32),
    /// The address is cartridge-domain but cannot be read (for example, no ROM
    /// is installed or the complete word lies outside its bounded image).
    Fault,
}

/// Result of classifying a proven direct word store against cartridge storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CartridgeStoreOutcome {
    /// The address is outside this cartridge adapter's window.
    NotCartridge,
    /// The address names immutable cartridge storage; the interpreter must
    /// fault without changing ordinary backing or exposing a write capability.
    ReadOnlyFault,
}

/// Read-only direct CPU access to installed cartridge storage.
///
/// There is intentionally no write method. [`Self::classify_store_w`] only
/// identifies a store that must fault, keeping immutable ROM mutation
/// unrepresentable at this seam.
pub trait CartridgeWordPort {
    fn read_w(&mut self, address: AlignedDirectWordAddress) -> CartridgeReadOutcome;
    fn classify_store_w(&mut self, address: AlignedDirectWordAddress) -> CartridgeStoreOutcome;
}

/// Distinct CPU memory authorities composed for one interpreted execution.
///
/// MMIO remains a register-only port. Cartridge storage is optional so the
/// existing no-cartridge and MMIO-only entrypoints retain their exact ordinary
/// RDRAM/fault fallback behavior.
pub struct MemoryPort<'a> {
    mmio: &'a mut dyn MmioPort,
    cartridge: Option<&'a mut dyn CartridgeWordPort>,
}

impl<'a> MemoryPort<'a> {
    pub fn new(mmio: &'a mut dyn MmioPort, cartridge: &'a mut dyn CartridgeWordPort) -> Self {
        Self {
            mmio,
            cartridge: Some(cartridge),
        }
    }

    pub fn mmio_only(mmio: &'a mut dyn MmioPort) -> Self {
        Self {
            mmio,
            cartridge: None,
        }
    }
}

/// The result of offering a word access to an [`MmioPort`].
///
/// Deliberately three-valued so the "in the window but the device rejected it"
/// case (an unmodeled register, a misaligned MMIO address, a device fault)
/// stays a *loud typed* outcome rather than collapsing into either a silent nop
/// or a spurious RDRAM fault. The interpreter surfaces [`MmioOutcome::Fault`] as
/// a [`CpuFaultKind::MemoryFault`] naming the faulting address — never a panic
/// or a nop.
#[cfg_attr(not(feature = "dev-interpreter"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmioOutcome<T> {
    /// The address is not in this port's modeled MMIO window; the interpreter
    /// must handle it as ordinary memory (backed RDRAM, or a typed fault).
    NotMmio,
    /// A modeled register access; `T` is the read value (`u32`) or `()` for a
    /// store whose modeled effect has been applied.
    Handled(T),
    /// The address is in the port's window but the device could not service it
    /// (unmodeled register, misaligned, or a device-level fault). Carries the
    /// faulting effective address for a typed [`CpuFaultKind::MemoryFault`].
    Fault { addr: u64 },
}

/// The no-device port: every access is [`MmioOutcome::NotMmio`], so the
/// interpreter behaves *exactly* as it did before the MMIO seam existed (word
/// loads/stores go straight to backed RDRAM, and an out-of-RDRAM address is a
/// typed [`CpuFaultKind::MemoryFault`]). [`run_bank`] uses this, which is why
/// adding the seam is byte-identical for every caller that does not opt in.
pub struct NoMmio;

impl MmioPort for NoMmio {
    fn read_w(&mut self, _vaddr: u64) -> MmioOutcome<u32> {
        MmioOutcome::NotMmio
    }
    fn write_w(&mut self, _vaddr: u64, _value: u32) -> MmioOutcome<()> {
        MmioOutcome::NotMmio
    }
}

/// Why one interpreter step could not execute an instruction whose *encoding*
/// the shared decoder recognizes but whose *architecture* this slice does not
/// yet model. Distinct from a guest [`CpuFault`]: this is a translator/runtime
/// coverage boundary (the AOT lane emits a host `panic!` for the same words),
/// surfaced as a typed value so the interpreter never panics or silently nops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedOp {
    /// The bank-qualified PC of the unsupported instruction.
    pub at: ExecutionKey,
    /// The decoded instruction (its `Debug` names the opcode in diagnostics).
    pub instruction: Instruction,
    /// The raw 32-bit encoding of the unsupported instruction. Kept alongside
    /// the decoded form so a dispatcher lane can surface a typed
    /// [`CpuFaultKind::UnsupportedInstruction`] naming the exact opcode without
    /// re-resolving the word from the catalog.
    pub word: u32,
}

impl UnsupportedOp {
    /// Surface this translator/runtime coverage boundary as the typed guest
    /// [`CpuFault`] the block dispatcher understands. The two are deliberately
    /// distinct types — an [`UnsupportedOp`] is a *lane* limitation, a
    /// [`CpuFault`] is the dispatcher's uniform `BlockExit::Fault` currency — so
    /// the conversion is explicit rather than a `From` that blurs the boundary.
    pub fn into_cpu_fault(self) -> CpuFault {
        CpuFault {
            at: self.at,
            kind: CpuFaultKind::UnsupportedInstruction { word: self.word },
        }
    }
}

/// Outcome of executing exactly one instruction (and, for a control transfer,
/// its delay slot) at a given PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Ordinary instruction retired; continue at the next sequential PC.
    Fallthrough { next: u32, retired: u32 },
    /// A control transfer resolved to a typed [`BlockExit`]. `retired` is the
    /// instruction count that fully retired *in this step* (2 for a committed
    /// branch/delay pair, 0 when a delay-slot fault annuls the branch).
    Exit { exit: BlockExit, retired: u32 },
}

/// A fault raised mid-step: either a guest CPU fault ([`CpuFault`]) or an
/// unsupported-opcode coverage boundary. Both are typed; neither panics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepFault {
    Cpu { fault: CpuFault, attempted: u32 },
    Unsupported(UnsupportedOp),
}

/// Precise architectural location of one straight instruction. Keeping the
/// branch owner with the delay-slot PC prevents a memory helper from
/// accidentally reporting the delay PC as EPC or clearing Cause.BD.
#[derive(Clone, Copy)]
pub(crate) struct FaultSite {
    pc: u32,
    epc: u32,
    branch_delay: bool,
}

impl FaultSite {
    pub(crate) const fn straight(pc: u32) -> Self {
        Self {
            pc,
            epc: pc,
            branch_delay: false,
        }
    }
}

/// Execute one ordinary instruction through the shared architectural semantic
/// kernel. This entry point owns no instruction-fetch or executable-image
/// authority; callers must perform admission and live-word verification before
/// invoking it. Decoding the admitted word here prevents a caller from pairing
/// one raw encoding with another instruction's decoded semantics.
pub(crate) fn execute_straight_word(
    bank: BankId,
    pc: u32,
    word: u32,
    retired_before: u32,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> Result<Step, StepFault> {
    let instruction = decode(word);
    assert!(
        !instruction.has_delay_slot(),
        "execute_straight_word requires an ordinary instruction: bank={bank:?} pc={pc:#010x} word={word:#010x} decoded={instruction:?}"
    );
    let words = [word];
    let source = InstructionUnit {
        bank,
        entry: GuestPc::new(pc),
        words: &words,
    };
    Interp {
        instructions: &source,
        bank,
    }
    .straight(
        FaultSite::straight(pc),
        word,
        instruction,
        ctx,
        mem,
        &mut MemoryPort::mmio_only(&mut NoMmio),
        retired_before,
    )
}

/// Run one admitted immutable bank (identified by `bank` within `catalog`) as
/// the interpreter lane.
///
/// This is the interpreter counterpart of a single [`emit_bank_runner`]-emitted
/// callable: it starts at `entry`, executes decoded instructions against `ctx`
/// and `mem`, and returns the same typed [`BlockRun`] the AOT runner would for
/// the same inputs. Words are fetched through [`CodeCatalog::resolve`], the same
/// admission the AOT program uses, so a hole never becomes executable and a
/// malformed PC faults identically. `entry.bank` must equal `bank`; a mismatch
/// is an [`CpuFaultKind::UnknownBank`] fault, exactly as the AOT runner reports.
///
#[cfg(feature = "dev-interpreter")]
pub fn run_bank(
    catalog: &CodeCatalog,
    bank: BankId,
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> Result<BlockRun, UnsupportedOp> {
    run_bank_with_mmio(catalog, bank, entry, budget, ctx, mem, &mut NoMmio)
}

/// [`run_bank`] with a hardware-register ([`MmioPort`]) door installed.
///
/// Identical to [`run_bank`] except that a word load/store whose effective
/// address the `port` claims as a modeled register is routed to the port
/// instead of backed RDRAM: an interpreted `lw` of a device register gets the
/// port's modeled value, and an interpreted `sw` updates the port's modeled
/// state, through the SAME peripheral authority the AOT/shim lanes use. Every
/// other access — including any address the port does not claim — is handled
/// exactly as [`run_bank`] handles it, so hole-stays-a-fault is untouched:
/// [`run_bank`] itself is just this function with a [`NoMmio`] port.
#[cfg(feature = "dev-interpreter")]
pub fn run_bank_with_mmio(
    catalog: &CodeCatalog,
    bank: BankId,
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    port: &mut dyn MmioPort,
) -> Result<BlockRun, UnsupportedOp> {
    run_bank_with_memory_port(
        catalog,
        bank,
        entry,
        budget,
        ctx,
        mem,
        &mut MemoryPort::mmio_only(port),
    )
}

/// [`run_bank`] with distinct register and cartridge-memory authorities.
#[cfg(feature = "dev-interpreter")]
pub fn run_bank_with_memory_port(
    catalog: &CodeCatalog,
    bank: BankId,
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    port: &mut MemoryPort<'_>,
) -> Result<BlockRun, UnsupportedOp> {
    let interp = Interp {
        instructions: catalog,
        bank,
    };
    interp.run(entry, budget, ctx, mem, port)
}

trait InstructionSource {
    fn resolve(&self, key: ExecutionKey) -> Result<u32, CpuFault>;
}

impl InstructionSource for CodeCatalog {
    fn resolve(&self, key: ExecutionKey) -> Result<u32, CpuFault> {
        CodeCatalog::resolve(self, key).map(|resolved| resolved.word)
    }
}

struct InstructionUnit<'a> {
    bank: BankId,
    entry: GuestPc,
    words: &'a [u32],
}

impl InstructionSource for InstructionUnit<'_> {
    fn resolve(&self, key: ExecutionKey) -> Result<u32, CpuFault> {
        if key.bank != self.bank {
            return Err(CpuFault {
                at: key,
                kind: CpuFaultKind::UnknownBank,
            });
        }
        if !key.pc.is_instruction_aligned() {
            return Err(CpuFault::instruction_address_error(key));
        }
        let index = self
            .words
            .iter()
            .enumerate()
            .find(|(index, _)| {
                self.entry.get().wrapping_add(
                    u32::try_from(*index).expect("instruction unit index exceeds u32") * 4,
                ) == key.pc.get()
            })
            .map(|(index, _)| index);
        index.map(|index| self.words[index]).ok_or(CpuFault {
            at: key,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: self.entry.get(),
                bank_end: self.entry.get().wrapping_add(self.words.len() as u32 * 4),
            },
        })
    }
}

/// Run one already-admitted mapped instruction unit without imposing a
/// non-wrapping virtual-span geometry on the architectural 32-bit PC.
pub(crate) fn run_instruction_unit_with_memory_port(
    bank: BankId,
    entry: GuestPc,
    words: &[u32],
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    port: &mut MemoryPort<'_>,
) -> Result<BlockRun, UnsupportedOp> {
    assert!(
        matches!(words.len(), 1 | 2),
        "mapped instruction unit must contain one straight word or one branch/delay pair"
    );
    let instructions = InstructionUnit { bank, entry, words };
    let interp = Interp {
        instructions: &instructions,
        bank,
    };
    interp.run(ExecutionKey::new(bank, entry), budget, ctx, mem, port)
}

/// The interpreter bound to one immutable instruction source.
struct Interp<'a> {
    instructions: &'a dyn InstructionSource,
    bank: BankId,
}

impl Interp<'_> {
    fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut MemoryPort<'_>,
    ) -> Result<BlockRun, UnsupportedOp> {
        // Bank/alignment admission mirrors the AOT runner prologue exactly: an
        // unknown bank or unaligned entry PC faults with zero instructions.
        if entry.bank != self.bank {
            return Ok(BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::UnknownBank,
                }),
                0,
            ));
        }
        if !entry.pc.is_instruction_aligned() {
            return Ok(BlockRun::new(
                BlockExit::Fault(CpuFault {
                    at: entry,
                    kind: CpuFaultKind::UnalignedPc,
                }),
                0,
            ));
        }

        let mut pc = entry.pc.get();
        let mut executed = 0u32;

        loop {
            let word = match self.resolve(pc) {
                Ok(word) => word,
                Err(fault) => {
                    return Ok(BlockRun::new(BlockExit::Fault(fault), executed));
                }
            };
            let instr = decode(word);

            if instr.has_delay_slot() {
                // A control transfer and its delay slot are one indivisible
                // dispatch unit. Charge/checkpoint identically to the AOT runner:
                // if the pair would not fit, checkpoint at the transfer's own PC
                // with no work from the pair committed.
                if !budget.can_fit(executed, InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS) {
                    return Ok(BlockRun::new(BlockExit::Checkpoint(self.key(pc)), executed));
                }
                // The delay slot must be admitted; the AOT emitter panics at
                // build time when a bank ends inside a delay slot, so a decoded
                // bank never reaches the interpreter with a missing slot. Read it
                // through the same catalog resolution so a delay slot in a hole
                // faults typed rather than reading out of the bank.
                let delay_pc = pc.wrapping_add(4);
                let (delay_word, delay) = match self.resolve(delay_pc) {
                    Ok(dword) => (dword, decode(dword)),
                    Err(fault) => {
                        // The transfer/delay pair is indivisible: a missing delay
                        // slot annuls the branch; nothing in this pair retires.
                        return Ok(BlockRun::new(BlockExit::Fault(fault), executed));
                    }
                };

                let attempt_runtime_fetch = executed + 2 < budget.get();
                match self.control_transfer(
                    pc,
                    instr,
                    delay_pc,
                    delay_word,
                    delay,
                    attempt_runtime_fetch,
                    ctx,
                    mem,
                    port,
                    executed,
                ) {
                    Ok(Step::Exit { exit, retired }) => {
                        let executed = executed + retired;
                        return Ok(BlockRun::new(
                            crate::execution::finalize_executable_write_exit(self.bank, exit),
                            executed,
                        ));
                    }
                    Ok(Step::Fallthrough { .. }) => {
                        unreachable!("a control transfer never falls through")
                    }
                    Err(StepFault::Cpu { fault, attempted }) => {
                        // Delay-slot fault annuls the branch: neither the branch
                        // nor the slot retire, matching MemFault::Fault's
                        // `(executed - 2)` accounting in the AOT lane.
                        return Ok(BlockRun::new(
                            crate::execution::finalize_executable_write_exit(
                                self.bank,
                                BlockExit::Fault(fault),
                            ),
                            executed + attempted,
                        ));
                    }
                    Err(StepFault::Unsupported(op)) => return Err(op),
                }
            }

            // Ordinary straight-line instruction.
            match self.straight(
                FaultSite {
                    pc,
                    epc: pc,
                    branch_delay: false,
                },
                word,
                instr,
                ctx,
                mem,
                port,
                executed,
            ) {
                Ok(Step::Fallthrough { next, retired }) => {
                    executed += retired;
                    if crate::runtime::take_executable_write_boundary() {
                        return Ok(BlockRun::new(
                            BlockExit::ExecutableWrite {
                                source_bank: self.bank,
                                resume: self.key(next),
                            },
                            executed,
                        ));
                    }
                    if self.contains(next) {
                        if executed >= budget.get() {
                            return Ok(BlockRun::new(
                                BlockExit::Checkpoint(self.key(next)),
                                executed,
                            ));
                        }
                        pc = next;
                    } else {
                        // Fell out of the admitted interval: hand the virtual PC
                        // to the active mapping layer (never guess a bank).
                        return Ok(BlockRun::new(
                            BlockExit::ResolveTransfer {
                                source_bank: self.bank,
                                target_pc: GuestPc::new(next),
                            },
                            executed,
                        ));
                    }
                }
                Ok(Step::Exit { exit, retired }) => {
                    executed += retired;
                    return Ok(BlockRun::new(
                        crate::execution::finalize_executable_write_exit(self.bank, exit),
                        executed,
                    ));
                }
                Err(StepFault::Cpu { fault, attempted }) => {
                    return Ok(BlockRun::new(BlockExit::Fault(fault), executed + attempted));
                }
                Err(StepFault::Unsupported(op)) => return Err(op),
            }
        }
    }

    /// Resolve an aligned in-bank PC to its instruction word, or the typed
    /// fault the AOT runner's `_ =>` arm would raise (an unmapped hole).
    fn resolve(&self, pc: u32) -> Result<u32, CpuFault> {
        self.instructions.resolve(self.key(pc))
    }

    fn key(&self, pc: u32) -> ExecutionKey {
        ExecutionKey::new(self.bank, GuestPc::new(pc))
    }

    /// Whether `target` lands inside this bank's admitted (executable) words.
    /// A bounding-range hole is NOT admitted, mirroring the sparse AOT domain.
    fn contains(&self, target: u32) -> bool {
        self.instructions.resolve(self.key(target)).is_ok()
    }

    /// A statically-known in-bank target is a proven [`BlockExit::Transfer`];
    /// anything else hands the virtual PC to the mapping layer as a
    /// [`BlockExit::ResolveTransfer`]. Never guesses a bank from geometry.
    fn proven_or_resolved(&self, target: u32) -> BlockExit {
        if self.contains(target) {
            BlockExit::Transfer(self.key(target))
        } else {
            BlockExit::ResolveTransfer {
                source_bank: self.bank,
                target_pc: GuestPc::new(target),
            }
        }
    }

    /// Calls always cross the resolver boundary, even when the guest target is
    /// already admitted by this instruction source. The resolver owns the
    /// host-function precedence decision; treating an in-bank call as an
    /// ordinary transfer would let overlapping guest code bypass that policy.
    fn resolved_call(&self, target: u32, resume: u32) -> BlockExit {
        BlockExit::ResolveCall {
            source_bank: self.bank,
            target_pc: GuestPc::new(target),
            resume: self.key(resume),
        }
    }

    /// The runtime (`jr`/`jalr`) transfer resolution. An unaligned computed
    /// target is a separate instruction-fetch attempt after the branch/delay
    /// pair: it checkpoints when the pair exhausts the budget, otherwise it
    /// contributes one retired unit and raises AdEL. An aligned in-bank target
    /// is a proven transfer; any other aligned target is resolved by the owner.
    fn runtime_transfer(
        &self,
        target: u32,
        attempt_fetch: bool,
        resume: Option<u32>,
    ) -> (BlockExit, u32) {
        if target & 3 != 0 {
            let at = self.key(target);
            return if attempt_fetch {
                (BlockExit::Fault(CpuFault::instruction_address_error(at)), 1)
            } else {
                (BlockExit::Checkpoint(at), 0)
            };
        }
        (
            resume.map_or_else(
                || self.proven_or_resolved(target),
                |resume| self.resolved_call(target, resume),
            ),
            0,
        )
    }

    /// Execute a control-transfer instruction and its delay slot, producing the
    /// typed [`BlockExit`]. `retired` is 2 on a committed branch/delay pair; a
    /// delay-slot [`CpuFault`] surfaces as `Err(StepFault::Cpu)` and the caller
    /// charges 0 (the branch is annulled).
    ///
    /// `retired_before` is this turn's retired-instruction count immediately
    /// before `instr` (the caller's `executed`) — forwarded to the delay
    /// slot's `straight` call for `MFC0 $9` interior Count visibility (see
    /// `exec_straight`'s doc comment). The branch/delay pair's own charge is
    /// still applied only as the lump `retired: 2` this function returns —
    /// exactly the AOT lane's `executed += 2` timing — so a delay-slot MFC0
    /// never sees itself or its owning branch as already retired.
    #[allow(clippy::too_many_arguments)]
    fn control_transfer(
        &self,
        pc: u32,
        instr: Instruction,
        delay_pc: u32,
        delay_word: u32,
        delay: Instruction,
        attempt_runtime_fetch: bool,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut MemoryPort<'_>,
        retired_before: u32,
    ) -> Result<Step, StepFault> {
        use Instruction::*;

        if instr.requires_cop0() && !ctx.cop0_usable() {
            return Err(StepFault::Cpu {
                fault: CpuFault {
                    at: self.key(pc),
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::CoprocessorUnusable,
                        epc: GuestPc::new(pc),
                        branch_delay: false,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: Some(0),
                    },
                },
                attempted: 1,
            });
        }

        let fallthrough = delay_pc.wrapping_add(4);
        let target = branch_target(&instr, pc);

        // Random is coupled to explicit arbitrary-PC instruction boundaries.
        // The branch executes before its delay instruction; a taken delay
        // observes the decremented value, while an annulled likely slot still
        // consumes the block lane's second deterministic instruction unit.
        ctx.advance_cop0_random(1);

        // Run the delay slot as an ordinary instruction. It may fault (memory)
        // or be unsupported; either annuls the branch and propagates typed. The
        // delay slot is itself an ordinary instruction, so it too may be an MMIO
        // load/store: it is routed through the same `port`.
        let run_delay = |ctx: &mut RecompContext,
                         mem: &mut Rdram<'_>,
                         port: &mut MemoryPort<'_>|
         -> Result<(), StepFault> {
            match self.straight(
                FaultSite {
                    pc: delay_pc,
                    epc: pc,
                    branch_delay: true,
                },
                delay_word,
                delay,
                ctx,
                mem,
                port,
                retired_before,
            )? {
                Step::Fallthrough { .. } => Ok(()),
                Step::Exit { .. } => {
                    unreachable!("a delay-slot instruction is never itself a control transfer")
                }
            }
        };

        // `j self` / `b self` is the cooperative idle boundary: run the delay
        // slot, then yield at this PC. Mirrors the AOT self-pause arm (which
        // yields rather than looping the host CPU).
        let self_pause = target == Some(pc)
            && (matches!(instr, J { .. }) || matches!(instr, Beq { rs: 0, rt: 0, .. }));
        if self_pause {
            run_delay(ctx, mem, port)?;
            return Ok(Step::Exit {
                exit: BlockExit::Yield(self.key(pc)),
                retired: 2,
            });
        }

        let exit = match instr {
            Jr { rs } => {
                // Snapshot the target BEFORE the delay slot: a delay instruction
                // that writes `rs` must not redirect the already-issued jump.
                let target = ctx.r_u32(rs);
                run_delay(ctx, mem, port)?;
                ctx.record_indirect_transfer(self.bank.get(), pc, rs, target, None);
                if ctx.is_thread_return(target) {
                    return Ok(Step::Exit {
                        exit: BlockExit::ThreadReturn,
                        retired: 2,
                    });
                }
                let (exit, target_fetch) =
                    self.runtime_transfer(target, attempt_runtime_fetch, None);
                return Ok(Step::Exit {
                    exit,
                    retired: 2 + target_fetch,
                });
            }
            Jalr { rd, rs } => {
                let target = ctx.r_u32(rs);
                ctx.set_r32(rd, fallthrough as i32);
                run_delay(ctx, mem, port)?;
                ctx.record_indirect_transfer(self.bank.get(), pc, rs, target, Some(fallthrough));
                if ctx.is_thread_return(target) {
                    return Ok(Step::Exit {
                        exit: BlockExit::ThreadReturn,
                        retired: 2,
                    });
                }
                let (exit, target_fetch) =
                    self.runtime_transfer(target, attempt_runtime_fetch, Some(fallthrough));
                return Ok(Step::Exit {
                    exit,
                    retired: 2 + target_fetch,
                });
            }
            J { .. } => {
                run_delay(ctx, mem, port)?;
                self.proven_or_resolved(target.expect("J has a static target"))
            }
            Jal { .. } => {
                ctx.set_r32(31, fallthrough as i32);
                run_delay(ctx, mem, port)?;
                self.resolved_call(target.expect("JAL has a static target"), fallthrough)
            }
            Bltzal { .. } | Bgezal { .. } | Bltzall { .. } | Bgezall { .. } => {
                // Conditional branch-and-link: the link register is written
                // unconditionally, before the (possibly annulled) delay slot.
                let take = branch_condition(&instr, ctx).expect("link branch has a condition");
                let target = target.expect("link branch has a static target");
                ctx.set_r32(31, fallthrough as i32);
                if instr.is_branch_likely() {
                    // Branch-likely: the delay slot runs ONLY when taken.
                    if take {
                        run_delay(ctx, mem, port)?;
                        self.proven_or_resolved(target)
                    } else {
                        ctx.advance_cop0_random(1);
                        self.proven_or_resolved(fallthrough)
                    }
                } else {
                    run_delay(ctx, mem, port)?;
                    self.proven_or_resolved(if take { target } else { fallthrough })
                }
            }
            _ if instr.is_branch_likely() => {
                let take = branch_condition(&instr, ctx).expect("likely branch has a condition");
                let target = target.expect("likely branch has a static target");
                if take {
                    run_delay(ctx, mem, port)?;
                    self.proven_or_resolved(target)
                } else {
                    ctx.advance_cop0_random(1);
                    self.proven_or_resolved(fallthrough)
                }
            }
            _ => {
                // Ordinary conditional branch: `take` is evaluated BEFORE the
                // delay slot (a delay instruction may overwrite an operand).
                let take = branch_condition(&instr, ctx).expect("branch has a condition");
                let target = target.expect("branch has a static target");
                run_delay(ctx, mem, port)?;
                self.proven_or_resolved(if take { target } else { fallthrough })
            }
        };

        Ok(Step::Exit { exit, retired: 2 })
    }

    /// Execute one ordinary (non-control-transfer) instruction against `ctx` and
    /// `mem`. Returns [`Step::Fallthrough`] with `retired == 1` on success.
    /// Semantics mirror `emit_straight` exactly (the AOT lane is the oracle).
    ///
    /// `retired_before` is the bank-runner turn's retired-instruction count
    /// immediately before `instr` — see `exec_straight`'s doc comment; it
    /// exists only to give an in-block `MFC0 $9` interior Count visibility.
    #[allow(clippy::too_many_arguments)]
    fn straight(
        &self,
        site: FaultSite,
        word: u32,
        instr: Instruction,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut MemoryPort<'_>,
        retired_before: u32,
    ) -> Result<Step, StepFault> {
        let next = site.pc.wrapping_add(4);
        let ok = Ok(Step::Fallthrough { next, retired: 1 });
        let mem_fault = |error: DataAccessError| {
            let attempted = u32::from(error.is_architectural_exception())
                * if site.branch_delay { 2 } else { 1 };
            StepFault::Cpu {
                fault: CpuFault {
                    at: self.key(site.pc),
                    kind: error.into_cpu_fault_kind(GuestPc::new(site.epc), site.branch_delay),
                },
                attempted,
            }
        };
        let address_fault = |exception, addr: u64| StepFault::Cpu {
            fault: CpuFault {
                at: self.key(site.pc),
                kind: CpuFaultKind::Exception {
                    exception,
                    epc: GuestPc::new(site.epc),
                    branch_delay: site.branch_delay,
                    instruction_code: 0,
                    bad_vaddr: Some(addr),
                    coprocessor: None,
                },
            },
            attempted: if site.branch_delay { 2 } else { 1 },
        };
        let unsupported = || {
            StepFault::Unsupported(UnsupportedOp {
                at: self.key(site.pc),
                instruction: instr,
                word,
            })
        };
        // SYSCALL/BREAK and a taken conditional trap. `instruction_code` carries
        // the architectural code field so a handler can read it back, matching
        // the AOT lane's trap emission.
        let trap_exception = |exception, code: u32| StepFault::Cpu {
            fault: CpuFault {
                at: self.key(site.pc),
                kind: CpuFaultKind::Exception {
                    exception,
                    epc: GuestPc::new(site.epc),
                    branch_delay: site.branch_delay,
                    instruction_code: code,
                    bad_vaddr: None,
                    coprocessor: None,
                },
            },
            attempted: if site.branch_delay { 2 } else { 1 },
        };
        // An enabled/unmaskable FP exception vectors to ExcCode 15, identical to
        // the AOT bank lane's `emit_bank_fpu_trap`: the destination register and
        // sticky Flags are left unwritten (the `ctx.fpu_*` helper already did
        // that), and the fault carries the precise EPC/BD for this site.
        let fpu_trap = || StepFault::Cpu {
            fault: CpuFault {
                at: self.key(site.pc),
                kind: CpuFaultKind::Exception {
                    exception: CpuException::FloatingPoint,
                    epc: GuestPc::new(site.epc),
                    branch_delay: site.branch_delay,
                    instruction_code: 0,
                    bad_vaddr: None,
                    coprocessor: None,
                },
            },
            attempted: if site.branch_delay { 2 } else { 1 },
        };

        if instr.requires_cop0() && !ctx.cop0_usable() {
            return Err(StepFault::Cpu {
                fault: CpuFault {
                    at: self.key(site.pc),
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::CoprocessorUnusable,
                        epc: GuestPc::new(site.epc),
                        branch_delay: site.branch_delay,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: Some(0),
                    },
                },
                attempted: if site.branch_delay { 2 } else { 1 },
            });
        }

        // Status.CU1 guard: a COP1-visible instruction with coprocessor 1
        // disabled (Status bit 29 clear) is a Coprocessor Unusable exception
        // (ExcCode 11, coprocessor 1), identical to the AOT bank lane's
        // `emit_bank_cop1_guard`. Checked before the op runs so the two lanes
        // agree on the fault vs. execute decision.
        if instr.requires_cop1() && ctx.cop0_status & (1 << 29) == 0 {
            return Err(StepFault::Cpu {
                fault: CpuFault {
                    at: self.key(site.pc),
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::CoprocessorUnusable,
                        epc: GuestPc::new(site.epc),
                        branch_delay: site.branch_delay,
                        instruction_code: 0,
                        bad_vaddr: None,
                        coprocessor: Some(1),
                    },
                },
                attempted: if site.branch_delay { 2 } else { 1 },
            });
        }

        // ERET has no delay slot, but it is a transfer rather than ordinary
        // fallthrough. Its COP0 authority guard above must precede the status
        // transition and LLbit clear performed by `exception_return_pc`.
        if matches!(instr, Instruction::Eret) {
            let target = ctx.exception_return_pc();
            ctx.advance_cop0_random(1);
            return Ok(Step::Exit {
                exit: BlockExit::ResolveTransfer {
                    source_bank: self.bank,
                    target_pc: GuestPc::new(target),
                },
                retired: 1,
            });
        }

        // Synchronous trapping instructions: SYSCALL, BREAK, and the twelve
        // conditional traps. These mirror the AOT lane's trap emission exactly
        // (same ExcCode, same signed/unsigned comparison width, same code
        // field), so an interpreted bank and a generated bank raise the
        // identical architectural exception for the same word. A conditional
        // trap whose condition is false retires as an ordinary no-op.
        if let Some((taken, exception, code)) = classify_trap(instr, ctx) {
            return if taken {
                Err(trap_exception(exception, code))
            } else {
                ok
            };
        }

        // After architectural alignment/TLB checks, precedence is structural:
        // modeled MMIO, then ordinary checked backing (including the legacy raw
        // hook), then immutable cartridge storage for a still-unbacked canonical
        // direct KSEG0/KSEG1 word. A broad cartridge classifier therefore cannot
        // shadow either RDRAM or a register. Cartridge stores expose only a
        // read-only classification outcome, so a claimed external store faults
        // without mutation. Only `Lw`/`Sw` use this deliberately narrow seam.
        if let Some((base, off, alignment, exception)) = aligned_memory_access(instr) {
            let addr = Rdram::eff_addr(ctx.r(base), off);
            if addr & (alignment - 1) != 0 {
                return Err(address_fault(exception, addr));
            }
        }

        match instr {
            Instruction::Unknown { .. } => {
                return Err(trap_exception(CpuException::ReservedInstruction, 0));
            }
            Instruction::Lw { rt, base, off } => {
                let addr = Rdram::eff_addr(ctx.r(base), off);
                let translated = ctx
                    .translate_data_address(addr, DataAccessKind::Load)
                    .map_err(mem_fault)?;
                let direct_word = match translated {
                    TranslatedDataAddress::Direct(address) => {
                        AlignedDirectWordAddress::from_translated(address)
                    }
                    TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                        None
                    }
                };
                let mmio_addr = match translated {
                    TranslatedDataAddress::Direct(address) => Some(address),
                    TranslatedDataAddress::DirectPhysical(physical)
                    | TranslatedDataAddress::Mapped(physical)
                        if physical < 0x2000_0000 =>
                    {
                        Some(0xffff_ffff_a000_0000 | u64::from(physical))
                    }
                    TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                        None
                    }
                };
                match mmio_addr.map_or(MmioOutcome::NotMmio, |address| port.mmio.read_w(address)) {
                    MmioOutcome::Handled(v) => {
                        ctx.set_r32(rt, v as i32);
                        ctx.advance_cop0_random(1);
                        return ok;
                    }
                    MmioOutcome::Fault { .. } => {
                        return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
                    }
                    MmioOutcome::NotMmio => {}
                }
                match mem.try_load_w_translated(ctx, addr) {
                    Ok(value) => {
                        ctx.set_r32(rt, value);
                        ctx.advance_cop0_random(1);
                        return ok;
                    }
                    Err(DataAccessError::Unbacked { .. }) => {}
                    Err(error) => return Err(mem_fault(error)),
                }
                if let (Some(address), Some(cartridge)) =
                    (direct_word, port.cartridge.as_deref_mut())
                {
                    match cartridge.read_w(address) {
                        CartridgeReadOutcome::Handled(value) => {
                            ctx.set_r32(rt, value as i32);
                            ctx.advance_cop0_random(1);
                            return ok;
                        }
                        CartridgeReadOutcome::Fault => {
                            return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
                        }
                        CartridgeReadOutcome::NotCartridge => {}
                    }
                }
                return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
            }
            Instruction::Sw { rt, base, off } => {
                let addr = Rdram::eff_addr(ctx.r(base), off);
                let translated = ctx
                    .translate_data_address(addr, DataAccessKind::Store)
                    .map_err(mem_fault)?;
                let direct_word = match translated {
                    TranslatedDataAddress::Direct(address) => {
                        AlignedDirectWordAddress::from_translated(address)
                    }
                    TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                        None
                    }
                };
                let mmio_addr = match translated {
                    TranslatedDataAddress::Direct(address) => Some(address),
                    TranslatedDataAddress::DirectPhysical(physical)
                    | TranslatedDataAddress::Mapped(physical)
                        if physical < 0x2000_0000 =>
                    {
                        Some(0xffff_ffff_a000_0000 | u64::from(physical))
                    }
                    TranslatedDataAddress::DirectPhysical(_) | TranslatedDataAddress::Mapped(_) => {
                        None
                    }
                };
                match mmio_addr.map_or(MmioOutcome::NotMmio, |address| {
                    port.mmio.write_w(address, ctx.r_u32(rt))
                }) {
                    MmioOutcome::Handled(()) => {
                        ctx.advance_cop0_random(1);
                        return ok;
                    }
                    MmioOutcome::Fault { .. } => {
                        return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
                    }
                    MmioOutcome::NotMmio => {}
                }
                match mem.try_store_w_translated(ctx, addr, ctx.r_u32(rt)) {
                    Ok(()) => {
                        ctx.advance_cop0_random(1);
                        return ok;
                    }
                    Err(DataAccessError::Unbacked { .. }) => {}
                    Err(error) => return Err(mem_fault(error)),
                }
                if let (Some(address), Some(cartridge)) =
                    (direct_word, port.cartridge.as_deref_mut())
                {
                    match cartridge.classify_store_w(address) {
                        CartridgeStoreOutcome::ReadOnlyFault => {
                            return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
                        }
                        CartridgeStoreOutcome::NotCartridge => {}
                    }
                }
                return Err(mem_fault(DataAccessError::Unbacked { vaddr: addr }));
            }
            _ => {}
        }

        exec_straight(
            instr,
            ctx,
            mem,
            &mem_fault,
            &unsupported,
            &fpu_trap,
            retired_before,
        )?;
        ctx.advance_cop0_random(1);
        ok
    }
}

/// The static (in-bank) branch/jump target, if one exists. Byte-identical to
/// the AOT emitter's `branch_target`: `jr`/`jalr` return `None` (computed).
fn branch_target(instr: &Instruction, vram: u32) -> Option<u32> {
    use Instruction::*;
    let rel = |off: i16| vram.wrapping_add(4).wrapping_add((off as i32 as u32) << 2);
    match *instr {
        Beq { off, .. } | Bne { off, .. } | Beql { off, .. } | Bnel { off, .. } => Some(rel(off)),
        Blez { off, .. } | Bgtz { off, .. } | Blezl { off, .. } | Bgtzl { off, .. } => {
            Some(rel(off))
        }
        Bltz { off, .. } | Bgez { off, .. } | Bltzl { off, .. } | Bgezl { off, .. } => {
            Some(rel(off))
        }
        Bltzal { off, .. } | Bgezal { off, .. } | Bltzall { off, .. } | Bgezall { off, .. } => {
            Some(rel(off))
        }
        Bc1t { off } | Bc1f { off } | Bc1tl { off } | Bc1fl { off } => Some(rel(off)),
        Bc0t { off } | Bc0f { off } | Bc0tl { off } | Bc0fl { off } => Some(rel(off)),
        J { target } | Jal { target } => Some((vram.wrapping_add(4) & 0xF000_0000) | (target << 2)),
        _ => None,
    }
}

/// Evaluate a conditional branch's predicate. Mirrors the AOT `branch_condition`
/// expression: `$zero` reads as 0, the single-operand branches compare the full
/// 64-bit register (`r_s64`), and the equality branches compare full registers.
/// Returns `None` for instructions that carry no condition (the unconditional
/// jumps handled separately by the caller).
fn branch_condition(instr: &Instruction, ctx: &RecompContext) -> Option<bool> {
    use Instruction::*;
    Some(match *instr {
        Beq { rs, rt, .. } | Beql { rs, rt, .. } => ctx.r(rs) == ctx.r(rt),
        Bne { rs, rt, .. } | Bnel { rs, rt, .. } => ctx.r(rs) != ctx.r(rt),
        Blez { rs, .. } | Blezl { rs, .. } => ctx.r_s64(rs) <= 0,
        Bgtz { rs, .. } | Bgtzl { rs, .. } => ctx.r_s64(rs) > 0,
        Bltz { rs, .. } | Bltzl { rs, .. } | Bltzal { rs, .. } | Bltzall { rs, .. } => {
            ctx.r_s64(rs) < 0
        }
        Bgez { rs, .. } | Bgezl { rs, .. } | Bgezal { rs, .. } | Bgezall { rs, .. } => {
            ctx.r_s64(rs) >= 0
        }
        Bc1t { .. } | Bc1tl { .. } => ctx.fpu_cond,
        Bc1f { .. } | Bc1fl { .. } => !ctx.fpu_cond,
        Bc0t { .. } | Bc0tl { .. } => ctx.cop0_cond,
        Bc0f { .. } | Bc0fl { .. } => !ctx.cop0_cond,
        _ => return None,
    })
}

/// Classify SYSCALL/BREAK and the twelve conditional traps, mirroring the AOT
/// lane's trap emission. Returns `None` when `instr` is not a trapping op, or
/// `Some((taken, exception, code))` where `taken` is false for a conditional
/// trap whose condition does not hold (it then retires as a no-op). The
/// comparison widths are the architectural ones: signed 64-bit for `TGE`/`TLT`
/// and their immediate forms, unsigned 64-bit for `TGEU`/`TLTU`, and a raw
/// 64-bit equality for `TEQ`/`TNE`; immediates are sign-extended first.
fn classify_trap(instr: Instruction, ctx: &RecompContext) -> Option<(bool, CpuException, u32)> {
    use Instruction::*;
    let s = |reg| ctx.r(reg) as i64;
    let u = |reg| ctx.r(reg);
    let (taken, exception, code) = match instr {
        Syscall { code } => (true, CpuException::Syscall, code),
        Break { code } => (true, CpuException::Breakpoint, code),
        Tge { rs, rt, code } => (s(rs) >= s(rt), CpuException::Trap, u32::from(code)),
        Tgeu { rs, rt, code } => (u(rs) >= u(rt), CpuException::Trap, u32::from(code)),
        Tlt { rs, rt, code } => (s(rs) < s(rt), CpuException::Trap, u32::from(code)),
        Tltu { rs, rt, code } => (u(rs) < u(rt), CpuException::Trap, u32::from(code)),
        Teq { rs, rt, code } => (u(rs) == u(rt), CpuException::Trap, u32::from(code)),
        Tne { rs, rt, code } => (u(rs) != u(rt), CpuException::Trap, u32::from(code)),
        Tgei { rs, imm } => (s(rs) >= i64::from(imm), CpuException::Trap, 0),
        Tgeiu { rs, imm } => (u(rs) >= i64::from(imm) as u64, CpuException::Trap, 0),
        Tlti { rs, imm } => (s(rs) < i64::from(imm), CpuException::Trap, 0),
        Tltiu { rs, imm } => ((u(rs) < i64::from(imm) as u64), CpuException::Trap, 0),
        Teqi { rs, imm } => (s(rs) == i64::from(imm), CpuException::Trap, 0),
        Tnei { rs, imm } => (s(rs) != i64::from(imm), CpuException::Trap, 0),
        _ => return None,
    };
    Some((taken, exception, code))
}

fn aligned_memory_access(instr: Instruction) -> Option<(u8, i16, u64, CpuException)> {
    use Instruction::*;
    Some(match instr {
        Lh { base, off, .. } | Lhu { base, off, .. } => {
            (base, off, 2, CpuException::AddressErrorLoad)
        }
        Lw { base, off, .. } | Lwu { base, off, .. } | Ll { base, off, .. } => {
            (base, off, 4, CpuException::AddressErrorLoad)
        }
        Ld { base, off, .. } | Lld { base, off, .. } => {
            (base, off, 8, CpuException::AddressErrorLoad)
        }
        Sh { base, off, .. } => (base, off, 2, CpuException::AddressErrorStore),
        Sw { base, off, .. } | Sc { base, off, .. } => {
            (base, off, 4, CpuException::AddressErrorStore)
        }
        Sd { base, off, .. } | Scd { base, off, .. } => {
            (base, off, 8, CpuException::AddressErrorStore)
        }
        _ => return None,
    })
}

fn convert_fpu_i32(
    ctx: &mut RecompContext,
    fd: u8,
    fs: u8,
    single: bool,
    mode: Option<u8>,
) -> Result<(), FpuException> {
    let value = if single {
        ctx.try_fpu_to_i32_s(fs, mode)?
    } else {
        ctx.try_fpu_to_i32_d(fs, mode)?
    };
    ctx.set_f_bits(fd, value as u32);
    Ok(())
}

fn convert_fpu_i64(
    ctx: &mut RecompContext,
    fd: u8,
    fs: u8,
    single: bool,
    mode: Option<u8>,
) -> Result<(), FpuException> {
    let value = if single {
        ctx.try_fpu_to_i64_s(fs, mode)?
    } else {
        ctx.try_fpu_to_i64_d(fs, mode)?
    };
    ctx.set_d_bits(fd, value as u64);
    Ok(())
}

/// Execute one straight-line instruction, driving `ctx`/`mem` through the SAME
/// typed accessors the AOT emitter open-codes. `mem_fault(addr)` builds the
/// typed fault for an out-of-bounds effective address; `unsupported()` builds
/// the typed coverage boundary for an op this slice does not model. Both are
/// returned as `Err` so no path panics or silently nops.
///
/// Every arithmetic/logical/shift/memory arm here is the executable twin of an
/// `emit_straight` arm; the differential test is the proof they agree.
///
/// `retired_before` is this bank-runner turn's retired-instruction count
/// immediately BEFORE `instr` (never including `instr` itself, and never
/// including an in-flight, not-yet-committed branch/delay pair — see
/// `Interp::run`'s `executed` accumulator, which is exactly this value at
/// every call site). `MFC0 $9` (Count) is the only arm that reads it, to
/// give a mid-block Count read the interior visibility
/// `RecompContext::read_cop0_count_interior` documents; every other arm
/// ignores it, matching the AOT lane exactly (see that method's doc comment
/// for why this cannot double-count against the block-boundary sync).
fn exec_straight(
    instr: Instruction,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    mem_fault: &dyn Fn(DataAccessError) -> StepFault,
    unsupported: &dyn Fn() -> StepFault,
    fpu_trap: &dyn Fn() -> StepFault,
    retired_before: u32,
) -> Result<(), StepFault> {
    use Instruction::*;

    // Effective-address helper, identical to `Rdram::eff_addr(r(base), off)`.
    let eff = |ctx: &RecompContext, base: u8, off: i16| Rdram::eff_addr(ctx.r(base), off);

    match instr {
        Nop => {}

        // --- ALU immediate (32-bit result, sign-extended into the GPR) ---
        Addi { rt, rs, imm } => {
            let v = ctx
                .r_s32(rs)
                .checked_add(imm as i32)
                .expect("MIPS ADDI integer overflow");
            ctx.set_r32(rt, v);
        }
        Addiu { rt, rs, imm } => ctx.set_r32(rt, ctx.r_s32(rs).wrapping_add(imm as i32)),
        Slti { rt, rs, imm } => ctx.set_r(rt, u64::from(ctx.r_s64(rs) < imm as i64)),
        Sltiu { rt, rs, imm } => ctx.set_r(rt, u64::from(ctx.r_u64(rs) < imm as i64 as u64)),
        Andi { rt, rs, imm } => ctx.set_r(rt, ctx.r(rs) & imm as u64),
        Ori { rt, rs, imm } => ctx.set_r(rt, ctx.r(rs) | imm as u64),
        Xori { rt, rs, imm } => ctx.set_r(rt, ctx.r(rs) ^ imm as u64),
        Lui { rt, imm } => ctx.set_r32(rt, ((imm as u32) << 16) as i32),

        // --- ALU register ---
        Add { rd, rs, rt } => {
            let v = ctx
                .r_s32(rs)
                .checked_add(ctx.r_s32(rt))
                .expect("MIPS ADD integer overflow");
            ctx.set_r32(rd, v);
        }
        Addu { rd, rs, rt } => ctx.set_r32(rd, ctx.r_s32(rs).wrapping_add(ctx.r_s32(rt))),
        Sub { rd, rs, rt } => {
            let v = ctx
                .r_s32(rs)
                .checked_sub(ctx.r_s32(rt))
                .expect("MIPS SUB integer overflow");
            ctx.set_r32(rd, v);
        }
        Subu { rd, rs, rt } => ctx.set_r32(rd, ctx.r_s32(rs).wrapping_sub(ctx.r_s32(rt))),
        And { rd, rs, rt } => ctx.set_r(rd, ctx.r(rs) & ctx.r(rt)),
        Or { rd, rs, rt } => ctx.set_r(rd, ctx.r(rs) | ctx.r(rt)),
        Xor { rd, rs, rt } => ctx.set_r(rd, ctx.r(rs) ^ ctx.r(rt)),
        Nor { rd, rs, rt } => ctx.set_r(rd, !(ctx.r(rs) | ctx.r(rt))),
        Slt { rd, rs, rt } => ctx.set_r(rd, u64::from(ctx.r_s64(rs) < ctx.r_s64(rt))),
        Sltu { rd, rs, rt } => ctx.set_r(rd, u64::from(ctx.r_u64(rs) < ctx.r_u64(rt))),

        // --- Shifts (32-bit, sign-extended) ---
        Sll { rd, rt, sa } => ctx.set_r32(rd, (ctx.r_u32(rt) << sa) as i32),
        Srl { rd, rt, sa } => ctx.set_r32(rd, (ctx.r_u32(rt) >> sa) as i32),
        Sra { rd, rt, sa } => ctx.set_r32(rd, ctx.r_s32(rt) >> sa),
        Sllv { rd, rt, rs } => ctx.set_r32(rd, (ctx.r_u32(rt) << (ctx.r_u32(rs) & 31)) as i32),
        Srlv { rd, rt, rs } => ctx.set_r32(rd, (ctx.r_u32(rt) >> (ctx.r_u32(rs) & 31)) as i32),
        Srav { rd, rt, rs } => ctx.set_r32(rd, ctx.r_s32(rt) >> (ctx.r_u32(rs) & 31)),

        // --- Mult/Div (write HI/LO; 32x32 -> 64 sign-extended halves) ---
        Mult { rs, rt } => {
            let p = (ctx.r_s32(rs) as i64) * (ctx.r_s32(rt) as i64);
            ctx.lo = (p as i32) as i64 as u64;
            ctx.hi = ((p >> 32) as i32) as i64 as u64;
        }
        Multu { rs, rt } => {
            let p = (ctx.r_u32(rs) as u64) * (ctx.r_u32(rt) as u64);
            ctx.lo = (p as i32) as i64 as u64;
            ctx.hi = ((p >> 32) as i32) as i64 as u64;
        }
        Div { rs, rt } => ctx.div_s32(ctx.r_s32(rs), ctx.r_s32(rt)),
        Divu { rs, rt } => ctx.div_u32(ctx.r_u32(rs), ctx.r_u32(rt)),
        Mfhi { rd } => ctx.set_r(rd, ctx.hi),
        Mflo { rd } => ctx.set_r(rd, ctx.lo),
        Mthi { rs } => ctx.hi = ctx.r(rs),
        Mtlo { rs } => ctx.lo = ctx.r(rs),

        // --- Loads ---
        Lw { rt, base, off } => {
            let v = mem
                .try_load_w_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }
        Lwu { rt, base, off } => {
            let v = mem
                .try_load_w_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v as u32 as u64);
        }
        Ll { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = mem.try_load_w_translated(ctx, a).map_err(mem_fault)?;
            ctx.set_r32(rt, v);
            ctx.set_ll_reservation(a, 4);
        }
        Lh { rt, base, off } => {
            let v = mem
                .try_load_h_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v as i32);
        }
        Lhu { rt, base, off } => {
            let v = mem
                .try_load_hu_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v as u64);
        }
        Lb { rt, base, off } => {
            let v = mem
                .try_load_b_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v as i32);
        }
        Lbu { rt, base, off } => {
            let v = mem
                .try_load_bu_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v as u64);
        }
        Lwl { rt, base, off } => {
            let v = mem
                .try_load_wl_translated(ctx, ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }
        Lwr { rt, base, off } => {
            let v = mem
                .try_load_wr_translated(ctx, ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }

        // --- Stores ---
        Sw { rt, base, off } => {
            mem.try_store_w_translated(ctx, eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Sh { rt, base, off } => {
            mem.try_store_h_translated(ctx, eff(ctx, base, off), ctx.r_u32(rt) as u16)
                .map_err(mem_fault)?;
        }
        Sb { rt, base, off } => {
            mem.try_store_b_translated(ctx, eff(ctx, base, off), ctx.r_u32(rt) as u8)
                .map_err(mem_fault)?;
        }
        Swl { rt, base, off } => {
            mem.try_store_wl_translated(ctx, eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Swr { rt, base, off } => {
            mem.try_store_wr_translated(ctx, eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Sc { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = ctx.r_u32(rt);
            Rdram::check_store_translation(ctx, a).map_err(mem_fault)?;
            if ctx.take_ll_reservation(a, 4) {
                mem.try_store_w_translated(ctx, a, v).map_err(mem_fault)?;
                ctx.set_r(rt, 1);
            } else {
                ctx.set_r(rt, 0);
            }
        }

        // --- 64-bit doubleword ALU immediate ---
        Daddi { rt, rs, imm } => {
            let v = (ctx.r_u64(rs) as i64)
                .checked_add(imm as i64)
                .expect("MIPS DADDI integer overflow") as u64;
            ctx.set_r(rt, v);
        }
        Daddiu { rt, rs, imm } => ctx.set_r(rt, ctx.r_u64(rs).wrapping_add(imm as i64 as u64)),

        // --- 64-bit doubleword ALU register ---
        Dadd { rd, rs, rt } => {
            let v = ctx
                .r_s64(rs)
                .checked_add(ctx.r_s64(rt))
                .expect("MIPS DADD integer overflow") as u64;
            ctx.set_r(rd, v);
        }
        Daddu { rd, rs, rt } => ctx.set_r(rd, ctx.r_u64(rs).wrapping_add(ctx.r_u64(rt))),
        Dsub { rd, rs, rt } => {
            let v = ctx
                .r_s64(rs)
                .checked_sub(ctx.r_s64(rt))
                .expect("MIPS DSUB integer overflow") as u64;
            ctx.set_r(rd, v);
        }
        Dsubu { rd, rs, rt } => ctx.set_r(rd, ctx.r_u64(rs).wrapping_sub(ctx.r_u64(rt))),

        // --- 64-bit doubleword shifts ---
        Dsll { rd, rt, sa } => ctx.set_r(rd, ctx.r_u64(rt) << sa),
        Dsrl { rd, rt, sa } => ctx.set_r(rd, ctx.r_u64(rt) >> sa),
        Dsra { rd, rt, sa } => ctx.set_r(rd, (ctx.r_s64(rt) >> sa) as u64),
        Dsll32 { rd, rt, sa } => ctx.set_r(rd, ctx.r_u64(rt) << (sa as u32 + 32)),
        Dsrl32 { rd, rt, sa } => ctx.set_r(rd, ctx.r_u64(rt) >> (sa as u32 + 32)),
        Dsra32 { rd, rt, sa } => ctx.set_r(rd, (ctx.r_s64(rt) >> (sa as u32 + 32)) as u64),
        Dsllv { rd, rt, rs } => ctx.set_r(rd, ctx.r_u64(rt) << (ctx.r_u64(rs) & 63)),
        Dsrlv { rd, rt, rs } => ctx.set_r(rd, ctx.r_u64(rt) >> (ctx.r_u64(rs) & 63)),
        Dsrav { rd, rt, rs } => ctx.set_r(rd, (ctx.r_s64(rt) >> (ctx.r_u64(rs) & 63)) as u64),

        // --- 64-bit doubleword mult/div ---
        Dmult { rs, rt } => {
            let p = (ctx.r_s64(rs) as i128) * (ctx.r_s64(rt) as i128);
            ctx.lo = p as u64;
            ctx.hi = (p >> 64) as u64;
        }
        Dmultu { rs, rt } => {
            let p = (ctx.r_u64(rs) as u128) * (ctx.r_u64(rt) as u128);
            ctx.lo = p as u64;
            ctx.hi = (p >> 64) as u64;
        }
        Ddiv { rs, rt } => ctx.div_s64(ctx.r_s64(rs), ctx.r_s64(rt)),
        Ddivu { rs, rt } => ctx.div_u64(ctx.r_u64(rs), ctx.r_u64(rt)),

        // --- Doubleword loads ---
        Ld { rt, base, off } => {
            let v = mem
                .try_load_d_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }
        Lld { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = mem.try_load_d_translated(ctx, a).map_err(mem_fault)?;
            ctx.set_r(rt, v);
            ctx.set_ll_reservation(a, 8);
        }
        Ldl { rt, base, off } => {
            let v = mem
                .try_load_dl_translated(ctx, ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }
        Ldr { rt, base, off } => {
            let v = mem
                .try_load_dr_translated(ctx, ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }

        // --- Doubleword stores ---
        Sd { rt, base, off } => {
            mem.try_store_d_translated(ctx, eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Sdl { rt, base, off } => {
            mem.try_store_dl_translated(ctx, eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Sdr { rt, base, off } => {
            mem.try_store_dr_translated(ctx, eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Scd { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = ctx.r_u64(rt);
            Rdram::check_store_translation(ctx, a).map_err(mem_fault)?;
            if ctx.take_ll_reservation(a, 8) {
                mem.try_store_d_translated(ctx, a, v).map_err(mem_fault)?;
                ctx.set_r(rt, 1);
            } else {
                ctx.set_r(rt, 0);
            }
        }

        // --- Modeled COP0/TLB management ---
        Mfc0 { rt, cop0d } => match cop0d {
            // Count (9): interior visibility — see
            // `RecompContext::read_cop0_count_interior`'s doc comment for
            // the exact boundary-sync contract this must not violate.
            9 => {
                ctx.set_r32(rt, ctx.read_cop0_count_interior(retired_before) as i32);
            }
            0 | 1 | 2 | 3 | 4 | 5 | 6 | 8 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 20 | 30 => {
                ctx.set_r32(rt, ctx.read_cop0(cop0d) as i32);
            }
            _ => return Err(unsupported()),
        },
        Mtc0 { rt, cop0d } => match cop0d {
            0 | 2 | 3 | 4 | 5 | 6 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30 => {
                ctx.write_cop0(cop0d, ctx.r_u32(rt));
            }
            _ => return Err(unsupported()),
        },
        Dmfc0 { rt, cop0d } if matches!(cop0d, 8 | 10 | 20) => {
            ctx.set_r(rt, ctx.read_cop0_64(cop0d));
        }
        Dmtc0 { rt, cop0d } if matches!(cop0d, 10 | 20) => {
            ctx.write_cop0_64(cop0d, ctx.r_u64(rt));
        }
        Tlbwi => ctx.tlbwi_record(),
        Tlbwr => ctx.tlbwr_record(),
        Tlbr => ctx.tlbr_read(),
        Tlbp => ctx.tlbp_probe(),

        // --- Cache / sync: no-ops on a coherent host rdram (as the AOT lane) ---
        Cache { .. } | Sync => {}

        // ================================================================
        // COP1 / FPU. Routed through the SAME typed `RecompContext` accessors
        // and the SAME `crate::fpu` soft-float shim the AOT bank lane emits, so
        // the two lanes produce bit-identical FPU results (FCSR.RM, IEEE flags,
        // canonical NaN, FR-mode register addressing, and the ExcCode-15
        // enabled/unmaskable-exception trap are all shared). Every `ctx.fpu_*`
        // arithmetic helper returns `true` when an enabled/unmaskable FP
        // exception trapped (destination left unwritten); that maps to the
        // `fpu_trap()` ExcCode-15 fault, exactly as `emit_bank_fpu_trap` does.
        // ================================================================

        // GPR <-> FPR moves.
        Mfc1 { rt, fs } => ctx.set_r32(rt, ctx.f_bits(fs) as i32),
        Mtc1 { rt, fs } => ctx.set_f_bits(fs, ctx.r_u32(rt)),
        Dmfc1 { rt, fs } => ctx.set_r(rt, ctx.d_bits(fs)),
        Dmtc1 { rt, fs } => ctx.set_d_bits(fs, ctx.r_u64(rt)),
        Cfc1 { rt, fs } => {
            let v = ctx.read_fcr(fs);
            ctx.set_r32(rt, v as i32);
        }
        Ctc1 { rt, fs } => {
            ctx.write_fcr(fs, ctx.r_u32(rt));
            if ctx.fcsr_exception_pending() {
                return Err(fpu_trap());
            }
        }
        CEqS { fs, ft } => ctx.try_fpu_compare_s(fs, ft, 2).map_err(|_| fpu_trap())?,
        CLtS { fs, ft } => ctx.try_fpu_compare_s(fs, ft, 12).map_err(|_| fpu_trap())?,
        CLeS { fs, ft } => ctx.try_fpu_compare_s(fs, ft, 14).map_err(|_| fpu_trap())?,
        CEqD { fs, ft } => ctx.try_fpu_compare_d(fs, ft, 2).map_err(|_| fpu_trap())?,
        CLtD { fs, ft } => ctx.try_fpu_compare_d(fs, ft, 12).map_err(|_| fpu_trap())?,
        CLeD { fs, ft } => ctx.try_fpu_compare_d(fs, ft, 14).map_err(|_| fpu_trap())?,
        CCondS { fs, ft, cond } => ctx
            .try_fpu_compare_s(fs, ft, cond)
            .map_err(|_| fpu_trap())?,
        CCondD { fs, ft, cond } => ctx
            .try_fpu_compare_d(fs, ft, cond)
            .map_err(|_| fpu_trap())?,
        CvtSW { fd, fs } => {
            let value = ctx.try_cvt_s_w_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_f_bits(fd, value);
        }
        CvtDW { fd, fs } => {
            let value = ctx.try_cvt_d_w_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_d_bits(fd, value);
        }
        CvtSL { fd, fs } => {
            let value = ctx.try_cvt_s_l_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_f_bits(fd, value);
        }
        CvtDL { fd, fs } => {
            let value = ctx.try_cvt_d_l_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_d_bits(fd, value);
        }
        CvtDS { fd, fs } => {
            let value = ctx.try_cvt_d_s_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_d_bits(fd, value);
        }
        CvtSD { fd, fs } => {
            let value = ctx.try_cvt_s_d_bits(fs).map_err(|_| fpu_trap())?;
            ctx.set_f_bits(fd, value);
        }
        CvtWS { fd, fs } => convert_fpu_i32(ctx, fd, fs, true, None).map_err(|_| fpu_trap())?,
        CvtWD { fd, fs } => convert_fpu_i32(ctx, fd, fs, false, None).map_err(|_| fpu_trap())?,
        CvtLS { fd, fs } => convert_fpu_i64(ctx, fd, fs, true, None).map_err(|_| fpu_trap())?,
        CvtLD { fd, fs } => convert_fpu_i64(ctx, fd, fs, false, None).map_err(|_| fpu_trap())?,
        RoundWS { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, true, Some(0)).map_err(|_| fpu_trap())?
        }
        RoundWD { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, false, Some(0)).map_err(|_| fpu_trap())?
        }
        RoundLS { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, true, Some(0)).map_err(|_| fpu_trap())?
        }
        RoundLD { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, false, Some(0)).map_err(|_| fpu_trap())?
        }
        TruncWS { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, true, Some(1)).map_err(|_| fpu_trap())?
        }
        TruncWD { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, false, Some(1)).map_err(|_| fpu_trap())?
        }
        TruncLS { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, true, Some(1)).map_err(|_| fpu_trap())?
        }
        TruncLD { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, false, Some(1)).map_err(|_| fpu_trap())?
        }
        CeilWS { fd, fs } => convert_fpu_i32(ctx, fd, fs, true, Some(2)).map_err(|_| fpu_trap())?,
        CeilWD { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, false, Some(2)).map_err(|_| fpu_trap())?
        }
        CeilLS { fd, fs } => convert_fpu_i64(ctx, fd, fs, true, Some(2)).map_err(|_| fpu_trap())?,
        CeilLD { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, false, Some(2)).map_err(|_| fpu_trap())?
        }
        FloorWS { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, true, Some(3)).map_err(|_| fpu_trap())?
        }
        FloorWD { fd, fs } => {
            convert_fpu_i32(ctx, fd, fs, false, Some(3)).map_err(|_| fpu_trap())?
        }
        FloorLS { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, true, Some(3)).map_err(|_| fpu_trap())?
        }
        FloorLD { fd, fs } => {
            convert_fpu_i64(ctx, fd, fs, false, Some(3)).map_err(|_| fpu_trap())?
        }

        // COP1 loads/stores.
        Lwc1 { ft, base, off } => {
            let v = mem
                .try_load_w_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_f_bits(ft, v as u32);
        }
        Swc1 { ft, base, off } => {
            mem.try_store_w_translated(ctx, eff(ctx, base, off), ctx.f_bits(ft))
                .map_err(mem_fault)?;
        }
        Ldc1 { ft, base, off } => {
            let v = mem
                .try_load_d_translated(ctx, eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_d_bits(ft, v);
        }
        Sdc1 { ft, base, off } => {
            mem.try_store_d_translated(ctx, eff(ctx, base, off), ctx.d_bits(ft))
                .map_err(mem_fault)?;
        }

        // Single-precision arithmetic (shim; may trap ExcCode 15).
        AddS { fd, fs, ft } if ctx.fpu_add_s(fd, fs, ft) => return Err(fpu_trap()),
        AddS { .. } => {}
        SubS { fd, fs, ft } if ctx.fpu_sub_s(fd, fs, ft) => return Err(fpu_trap()),
        SubS { .. } => {}
        MulS { fd, fs, ft } if ctx.fpu_mul_s(fd, fs, ft) => return Err(fpu_trap()),
        MulS { .. } => {}
        DivS { fd, fs, ft } if ctx.fpu_div_s(fd, fs, ft) => return Err(fpu_trap()),
        DivS { .. } => {}
        AbsS { fd, fs } if ctx.fpu_abs_s(fd, fs) => return Err(fpu_trap()),
        AbsS { .. } => {}
        NegS { fd, fs } if ctx.fpu_neg_s(fd, fs) => return Err(fpu_trap()),
        NegS { .. } => {}
        SqrtS { fd, fs } if ctx.fpu_sqrt_s(fd, fs) => return Err(fpu_trap()),
        SqrtS { .. } => {}
        MovS { fd, fs } => ctx.set_f_bits(fd, ctx.f_bits(fs)),
        MovcfS { fd, fs, tf } => ctx.fpu_movcf_s(fd, fs, tf),
        MovzS { fd, fs, rt } => ctx.fpu_movz_s(fd, fs, rt),
        MovnS { fd, fs, rt } => ctx.fpu_movn_s(fd, fs, rt),

        // Double-precision arithmetic (shim; may trap ExcCode 15).
        AddD { fd, fs, ft } if ctx.fpu_add_d(fd, fs, ft) => return Err(fpu_trap()),
        AddD { .. } => {}
        SubD { fd, fs, ft } if ctx.fpu_sub_d(fd, fs, ft) => return Err(fpu_trap()),
        SubD { .. } => {}
        MulD { fd, fs, ft } if ctx.fpu_mul_d(fd, fs, ft) => return Err(fpu_trap()),
        MulD { .. } => {}
        DivD { fd, fs, ft } if ctx.fpu_div_d(fd, fs, ft) => return Err(fpu_trap()),
        DivD { .. } => {}
        AbsD { fd, fs } if ctx.fpu_abs_d(fd, fs) => return Err(fpu_trap()),
        AbsD { .. } => {}
        NegD { fd, fs } if ctx.fpu_neg_d(fd, fs) => return Err(fpu_trap()),
        NegD { .. } => {}
        SqrtD { fd, fs } if ctx.fpu_sqrt_d(fd, fs) => return Err(fpu_trap()),
        SqrtD { .. } => {}
        MovD { fd, fs } => ctx.set_d_bits(fd, ctx.d_bits(fs)),
        MovcfD { fd, fs, tf } => ctx.fpu_movcf_d(fd, fs, tf),
        MovzD { fd, fs, rt } => ctx.fpu_movz_d(fd, fs, rt),
        MovnD { fd, fs, rt } => ctx.fpu_movn_d(fd, fs, rt),

        // Conversions — mirror the AOT emit arms exactly (same casts, same
        // `fpu_to_i32`/`i64` rounding through FCSR.RM or the fixed mode).
        // ================================================================
        // Out of scope for this slice — a loud typed unsupported fault naming
        // the opcode, mirroring the AOT lane's host `panic!` for the same
        // words. COP2/TLB/exceptions are the named next frontier
        // (docs/UNIVERSAL-RUNTIME-PLAN.md, U4). Nothing here is a silent nop.
        // ================================================================
        _ => return Err(unsupported()),
    }
    Ok(())
}

#[cfg(all(test, feature = "dev-interpreter"))]
mod tests;
