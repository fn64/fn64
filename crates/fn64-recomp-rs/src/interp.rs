//! The `dynamic_mips` fallback lane: an instrumented MIPS-III interpreter that
//! executes one admitted [`CodeBank`] behind the **same** [`BlockExit`] contract
//! the emitted AOT bank runner ([`crate::emit::emit_bank_runner`]) satisfies.
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
//! weakly than the AOT lane: an encoding the decoder does not recognize is an
//! [`Instruction::Unknown`], which becomes a typed unsupported fault here, never
//! a panic or silent nop.
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
//! pair, and a guest memory access outside backed RDRAM as a typed
//! [`CpuFaultKind::MemoryFault`] reusing the U4 `Rdram::try_*` accessors.
//!
//! Explicitly OUT (each a loud [`StepFault::Unsupported`] naming the opcode, the
//! same frontier the AOT lane leaves open — see `Still open in U4` in
//! `docs/UNIVERSAL-RUNTIME-PLAN.md`): the entire COP1/FPU environment, COP2,
//! `TLBWR`, TLB translation, `ERET`, `SYSCALL`/`BREAK`, and the conditional trap ops. Modeled
//! 32-bit COP0 moves and `TLBWI`/`TLBR`/`TLBP` share the typed context with the
//! AOT lane. Precise
//! VR4300 exception vectoring, `BadVAddr`/`EPC`/`Cause`, TLB-miss vs.
//! address-error, and alignment (`AdEL`/`AdES`) faulting are likewise absent, as
//! in the AOT lane's first U4 slice.

use crate::decoder::{decode, Instruction};
use crate::execution::{
    BankId, BlockExit, BlockRun, CodeCatalog, CpuFault, CpuFaultKind, ExecutionKey, GuestPc,
    InstructionBudget,
};
use crate::runtime::{Rdram, RecompContext};

/// A hardware-register (KSEG1 MMIO) access the interpreter recognizes as
/// *not* backed RDRAM and routes to a modeled device instead of faulting.
///
/// # Why this is a trait, and why it lives here
///
/// The interpreter (`fn64-recomp-rs`) must reach the runtime's modeled device
/// state (`fn64-runtime`'s `DeviceFabric`/`MmioSpace`) to give a guest MMIO
/// load a modeled register value and a guest MMIO store a modeled effect. But
/// `fn64-recomp-rs` must not depend on `fn64-runtime` (the dependency edge runs
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

/// The result of offering a word access to an [`MmioPort`].
///
/// Deliberately three-valued so the "in the window but the device rejected it"
/// case (an unmodeled register, a misaligned MMIO address, a device fault)
/// stays a *loud typed* outcome rather than collapsing into either a silent nop
/// or a spurious RDRAM fault. The interpreter surfaces [`MmioOutcome::Fault`] as
/// a [`CpuFaultKind::MemoryFault`] naming the faulting address — never a panic
/// or a nop.
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
enum Step {
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
enum StepFault {
    Cpu(CpuFault),
    Unsupported(UnsupportedOp),
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
/// [`emit_bank_runner`]: crate::emit::emit_bank_runner
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
pub fn run_bank_with_mmio(
    catalog: &CodeCatalog,
    bank: BankId,
    entry: ExecutionKey,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    port: &mut dyn MmioPort,
) -> Result<BlockRun, UnsupportedOp> {
    let interp = Interp { catalog, bank };
    interp.run(entry, budget, ctx, mem, port)
}

/// The interpreter bound to one immutable bank inside a catalog.
struct Interp<'a> {
    catalog: &'a CodeCatalog,
    bank: BankId,
}

impl Interp<'_> {
    fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut dyn MmioPort,
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
                // if any instruction already retired this turn and the pair would
                // not fit, checkpoint at the transfer's own PC (no work done yet).
                if executed != 0 && executed + 2 > budget.get() {
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

                match self.control_transfer(pc, instr, delay_pc, delay_word, delay, ctx, mem, port)
                {
                    Ok(Step::Exit { exit, retired }) => {
                        let executed = executed + retired;
                        return Ok(BlockRun::new(exit, executed));
                    }
                    Ok(Step::Fallthrough { .. }) => {
                        unreachable!("a control transfer never falls through")
                    }
                    Err(StepFault::Cpu(fault)) => {
                        // Delay-slot fault annuls the branch: neither the branch
                        // nor the slot retire, matching MemFault::Fault's
                        // `(executed - 2)` accounting in the AOT lane.
                        return Ok(BlockRun::new(BlockExit::Fault(fault), executed));
                    }
                    Err(StepFault::Unsupported(op)) => return Err(op),
                }
            }

            // Ordinary straight-line instruction.
            match self.straight(pc, word, instr, ctx, mem, port) {
                Ok(Step::Fallthrough { next, retired }) => {
                    executed += retired;
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
                Ok(Step::Exit { .. }) => {
                    unreachable!("a straight-line instruction never produces a transfer exit")
                }
                Err(StepFault::Cpu(fault)) => {
                    return Ok(BlockRun::new(BlockExit::Fault(fault), executed));
                }
                Err(StepFault::Unsupported(op)) => return Err(op),
            }
        }
    }

    /// Resolve an aligned in-bank PC to its instruction word, or the typed
    /// fault the AOT runner's `_ =>` arm would raise (an unmapped hole).
    fn resolve(&self, pc: u32) -> Result<u32, CpuFault> {
        self.catalog
            .resolve(self.key(pc))
            .map(|resolved| resolved.word)
    }

    fn key(&self, pc: u32) -> ExecutionKey {
        ExecutionKey::new(self.bank, GuestPc::new(pc))
    }

    /// Whether `target` lands inside this bank's admitted (executable) words.
    /// A bounding-range hole is NOT admitted, mirroring the sparse AOT domain.
    fn contains(&self, target: u32) -> bool {
        self.catalog.resolve(self.key(target)).is_ok()
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

    /// The runtime (`jr`/`jalr`) transfer resolution: an unaligned computed
    /// target is a typed [`CpuFaultKind::UnalignedPc`]; an in-bank target is a
    /// proven transfer; otherwise a resolve transfer. Mirrors the AOT
    /// `emit_runtime_transfer` sequence exactly.
    fn runtime_transfer(&self, target: u32) -> BlockExit {
        if target & 3 != 0 {
            return BlockExit::Fault(CpuFault {
                at: self.key(target),
                kind: CpuFaultKind::UnalignedPc,
            });
        }
        self.proven_or_resolved(target)
    }

    /// Execute a control-transfer instruction and its delay slot, producing the
    /// typed [`BlockExit`]. `retired` is 2 on a committed branch/delay pair; a
    /// delay-slot [`CpuFault`] surfaces as `Err(StepFault::Cpu)` and the caller
    /// charges 0 (the branch is annulled).
    #[allow(clippy::too_many_arguments)]
    fn control_transfer(
        &self,
        pc: u32,
        instr: Instruction,
        delay_pc: u32,
        delay_word: u32,
        delay: Instruction,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut dyn MmioPort,
    ) -> Result<Step, StepFault> {
        use Instruction::*;

        let fallthrough = delay_pc.wrapping_add(4);
        let target = branch_target(&instr, pc);

        // Run the delay slot as an ordinary instruction. It may fault (memory)
        // or be unsupported; either annuls the branch and propagates typed. The
        // delay slot is itself an ordinary instruction, so it too may be an MMIO
        // load/store: it is routed through the same `port`.
        let run_delay = |ctx: &mut RecompContext,
                         mem: &mut Rdram<'_>,
                         port: &mut dyn MmioPort|
         -> Result<(), StepFault> {
            match self.straight(delay_pc, delay_word, delay, ctx, mem, port)? {
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
                self.runtime_transfer(target)
            }
            Jalr { rd, rs } => {
                let target = ctx.r_u32(rs);
                ctx.set_r32(rd, fallthrough as i32);
                run_delay(ctx, mem, port)?;
                self.runtime_transfer(target)
            }
            J { .. } => {
                run_delay(ctx, mem, port)?;
                self.proven_or_resolved(target.expect("J has a static target"))
            }
            Jal { .. } => {
                ctx.set_r32(31, fallthrough as i32);
                run_delay(ctx, mem, port)?;
                self.proven_or_resolved(target.expect("JAL has a static target"))
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
    fn straight(
        &self,
        pc: u32,
        word: u32,
        instr: Instruction,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut dyn MmioPort,
    ) -> Result<Step, StepFault> {
        let next = pc.wrapping_add(4);
        let ok = Ok(Step::Fallthrough { next, retired: 1 });
        let mem_fault = |addr: u64| {
            StepFault::Cpu(CpuFault {
                at: self.key(pc),
                kind: CpuFaultKind::MemoryFault { addr },
            })
        };
        let unsupported = || {
            StepFault::Unsupported(UnsupportedOp {
                at: self.key(pc),
                instruction: instr,
                word,
            })
        };

        // The MMIO seam: a word load/store is offered to the device `port`
        // FIRST. The port is the sole authority on which effective addresses are
        // modeled registers (`MmioOutcome::NotMmio` for everything else), so this
        // diverts ONLY register accesses to the modeled device and leaves every
        // other address — backed RDRAM or an out-of-RDRAM hole — to
        // `exec_straight`'s `try_*` accessor, unchanged. That is why the seam
        // cannot turn an arbitrary out-of-RDRAM address into a success: the port
        // says `NotMmio` and the address still faults typed. Only `Lw`/`Sw` are
        // routed here (the one proven device-word interaction); every other
        // width/op stays on the plain RDRAM path — a deliberately narrow first
        // slice (`docs/UNIVERSAL-RUNTIME-PLAN.md` U2).
        match instr {
            Instruction::Lw { rt, base, off } => {
                let addr = Rdram::eff_addr(ctx.r(base), off);
                match port.read_w(addr) {
                    MmioOutcome::Handled(v) => {
                        ctx.set_r32(rt, v as i32);
                        return ok;
                    }
                    MmioOutcome::Fault { addr } => return Err(mem_fault(addr)),
                    MmioOutcome::NotMmio => {}
                }
            }
            Instruction::Sw { rt, base, off } => {
                let addr = Rdram::eff_addr(ctx.r(base), off);
                match port.write_w(addr, ctx.r_u32(rt)) {
                    MmioOutcome::Handled(()) => return ok,
                    MmioOutcome::Fault { addr } => return Err(mem_fault(addr)),
                    MmioOutcome::NotMmio => {}
                }
            }
            _ => {}
        }

        exec_straight(instr, ctx, mem, &mem_fault, &unsupported)?;
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

/// Execute one straight-line instruction, driving `ctx`/`mem` through the SAME
/// typed accessors the AOT emitter open-codes. `mem_fault(addr)` builds the
/// typed fault for an out-of-bounds effective address; `unsupported()` builds
/// the typed coverage boundary for an op this slice does not model. Both are
/// returned as `Err` so no path panics or silently nops.
///
/// Every arithmetic/logical/shift/memory arm here is the executable twin of an
/// `emit_straight` arm; the differential test is the proof they agree.
fn exec_straight(
    instr: Instruction,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    mem_fault: &dyn Fn(u64) -> StepFault,
    unsupported: &dyn Fn() -> StepFault,
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
            let v = mem.try_load_w(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }
        Lwu { rt, base, off } => {
            let v = mem.try_load_w(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r(rt, v as u32 as u64);
        }
        Ll { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = mem.try_load_w(a).map_err(mem_fault)?;
            ctx.set_r32(rt, v);
            ctx.set_ll_reservation(a, 4);
        }
        Lh { rt, base, off } => {
            let v = mem.try_load_h(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r32(rt, v as i32);
        }
        Lhu { rt, base, off } => {
            let v = mem.try_load_hu(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r(rt, v as u64);
        }
        Lb { rt, base, off } => {
            let v = mem.try_load_b(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r32(rt, v as i32);
        }
        Lbu { rt, base, off } => {
            let v = mem.try_load_bu(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r(rt, v as u64);
        }
        Lwl { rt, base, off } => {
            let v = mem
                .try_load_wl(ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }
        Lwr { rt, base, off } => {
            let v = mem
                .try_load_wr(ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r32(rt, v);
        }

        // --- Stores ---
        Sw { rt, base, off } => {
            mem.try_store_w(eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Sh { rt, base, off } => {
            mem.try_store_h(eff(ctx, base, off), ctx.r_u32(rt) as u16)
                .map_err(mem_fault)?;
        }
        Sb { rt, base, off } => {
            mem.try_store_b(eff(ctx, base, off), ctx.r_u32(rt) as u8)
                .map_err(mem_fault)?;
        }
        Swl { rt, base, off } => {
            mem.try_store_wl(eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Swr { rt, base, off } => {
            mem.try_store_wr(eff(ctx, base, off), ctx.r_u32(rt))
                .map_err(mem_fault)?;
        }
        Sc { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = ctx.r_u32(rt);
            if ctx.take_ll_reservation(a, 4) {
                mem.try_store_w(a, v).map_err(mem_fault)?;
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
            let v = mem.try_load_d(eff(ctx, base, off)).map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }
        Lld { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = mem.try_load_d(a).map_err(mem_fault)?;
            ctx.set_r(rt, v);
            ctx.set_ll_reservation(a, 8);
        }
        Ldl { rt, base, off } => {
            let v = mem
                .try_load_dl(ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }
        Ldr { rt, base, off } => {
            let v = mem
                .try_load_dr(ctx.r(rt), eff(ctx, base, off))
                .map_err(mem_fault)?;
            ctx.set_r(rt, v);
        }

        // --- Doubleword stores ---
        Sd { rt, base, off } => {
            mem.try_store_d(eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Sdl { rt, base, off } => {
            mem.try_store_dl(eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Sdr { rt, base, off } => {
            mem.try_store_dr(eff(ctx, base, off), ctx.r_u64(rt))
                .map_err(mem_fault)?;
        }
        Scd { rt, base, off } => {
            let a = eff(ctx, base, off);
            let v = ctx.r_u64(rt);
            if ctx.take_ll_reservation(a, 8) {
                mem.try_store_d(a, v).map_err(mem_fault)?;
                ctx.set_r(rt, 1);
            } else {
                ctx.set_r(rt, 0);
            }
        }

        // --- Modeled COP0/TLB management ---
        Mfc0 { rt, cop0d } => match cop0d {
            0 | 2 | 3 | 5 | 6 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30 => {
                ctx.set_r32(rt, ctx.read_cop0(cop0d) as i32);
            }
            _ => return Err(unsupported()),
        },
        Mtc0 { rt, cop0d } => match cop0d {
            0 | 2 | 3 | 5 | 6 | 9 | 10 | 11 | 12 | 13 | 14 | 18 | 19 | 30 => {
                ctx.write_cop0(cop0d, ctx.r_u32(rt));
            }
            _ => return Err(unsupported()),
        },
        Tlbwi => ctx.tlbwi_record(),
        Tlbr => ctx.tlbr_read(),
        Tlbp => ctx.tlbp_probe(),

        // --- Cache / sync: no-ops on a coherent host rdram (as the AOT lane) ---
        Cache { .. } | Sync => {}

        // ================================================================
        // Out of scope for this slice — a loud typed unsupported fault naming
        // the opcode, mirroring the AOT lane's host `panic!` for the same
        // words. FPU/COP0/COP2/TLB/exceptions are the named next frontier
        // (docs/UNIVERSAL-RUNTIME-PLAN.md, U4). Nothing here is a silent nop.
        // ================================================================
        _ => return Err(unsupported()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{CodeBank, CodeSpan};

    const BANK: BankId = BankId::new(0x42);
    const VA: u32 = 0x8000_1000;

    fn catalog_of(words: &[u32]) -> CodeCatalog {
        let bank = CodeBank::new(BANK, GuestPc::new(VA), words.to_vec()).unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        catalog
    }

    fn run(
        catalog: &CodeCatalog,
        pc: u32,
        budget: u32,
        ctx: &mut RecompContext,
    ) -> Result<BlockRun, UnsupportedOp> {
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        run_bank(
            catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(pc)),
            InstructionBudget::new(budget).unwrap(),
            ctx,
            &mut mem,
        )
    }

    #[test]
    fn unknown_bank_and_unaligned_entry_fault_with_zero_work() {
        // addiu $v0,$zero,1 ; jr $ra ; nop
        let catalog = catalog_of(&[0x2402_0001, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();

        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let wrong = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BankId::new(0x99), GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        assert!(matches!(
            wrong.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnknownBank,
                ..
            })
        ));
        assert_eq!(wrong.instructions, 0);

        let unaligned = run(&catalog, VA + 2, 8, &mut ctx).unwrap();
        assert!(matches!(
            unaligned.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnalignedPc,
                ..
            })
        ));
        assert_eq!(unaligned.instructions, 0);
        assert_eq!(
            ctx.r(2),
            0,
            "faulting entry must not execute any instruction"
        );
    }

    #[test]
    fn unsupported_opcode_is_a_loud_typed_fault_not_a_panic_or_nop() {
        // TLBWR depends on the per-instruction Random countdown and remains
        // outside this slice: decoded, then a typed unsupported fault naming
        // the op, exactly where the AOT lane traps.
        let tlbwr = 0x4200_0006;
        let catalog = catalog_of(&[tlbwr, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let err = run(&catalog, VA, 8, &mut ctx).unwrap_err();
        assert_eq!(err.at, ExecutionKey::new(BANK, GuestPc::new(VA)));
        assert_eq!(err.instruction, Instruction::Tlbwr);
    }

    #[test]
    fn modeled_cop0_and_indexed_tlb_management_execute_in_the_interpreter() {
        let mtc0 = |rt: u32, rd: u32| 0x4080_0000 | (rt << 16) | (rd << 11);
        let mfc0 = |rt: u32, rd: u32| 0x4000_0000 | (rt << 16) | (rd << 11);
        let words = [
            mtc0(2, 10), // EntryHi
            mtc0(3, 2),  // EntryLo0
            mtc0(4, 3),  // EntryLo1
            mtc0(5, 5),  // PageMask
            mtc0(6, 0),  // Index
            0x4200_0002, // TLBWI
            mtc0(7, 10), // probe EntryHi
            0x4200_0008, // TLBP
            mfc0(8, 0),  // matched Index
            0x4200_0001, // TLBR
            mfc0(9, 10), // reloaded EntryHi
            0x03e0_0008, // jr $ra
            0,
        ];
        let catalog = catalog_of(&words);
        let mut ctx = RecompContext::new();
        ctx.set_r(2, 0x1234_400a);
        ctx.set_r(3, 0x0000_0046);
        ctx.set_r(4, 0x0000_0086);
        ctx.set_r(5, 0x0000_6000);
        ctx.set_r(6, 7);
        ctx.set_r(7, 0x1234_200a);

        let result = run(&catalog, VA, words.len() as u32, &mut ctx).unwrap();
        assert_eq!(result.instructions, words.len() as u32);
        assert_eq!(ctx.r_u32(8), 7);
        assert_eq!(ctx.r_u32(9), 0x1234_400a);
        assert_eq!(ctx.cop0_page_mask, 0x0000_6000);
        assert_eq!(ctx.cop0_entry_lo0, 0x0000_0046);
        assert_eq!(ctx.cop0_entry_lo1, 0x0000_0086);
    }

    #[test]
    fn memory_fault_reports_effective_address_and_excludes_the_faulting_op() {
        // lui $t0,0x8000 ; sw $v0,0x40($t0) (offset 0x40 outside 16-byte rdram)
        let catalog = catalog_of(&[0x3C08_8000, 0xAD02_0040, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let run = run_bank(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::MemoryFault { addr },
            }) => {
                assert_eq!(at, ExecutionKey::new(BANK, GuestPc::new(VA + 4)));
                assert_eq!(addr, 0xFFFF_FFFF_8000_0040);
            }
            other => panic!("expected typed MemoryFault, got {other:?}"),
        }
        // Only the LUI retired; the faulting SW is excluded.
        assert_eq!(run.instructions, 1);
    }

    #[test]
    fn budget_checkpoints_before_a_branch_delay_pair_without_splitting_it() {
        // Two straight ops, then a branch: a 2-instruction budget must stop at
        // the branch's PC with the pair uncharged.
        let catalog = catalog_of(&[
            0x2402_0001, // addiu $v0,$zero,1
            0x2442_0002, // addiu $v0,$v0,2
            0x1042_0001, // beq $v0,$v0,+1
            0x2404_0007, // addiu $a0,$zero,7 (delay)
            0x03E0_0008, // jr $ra
            0x0000_0000, // nop
        ]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 2, &mut ctx).unwrap();
        assert_eq!(run.instructions, 2);
        assert_eq!(
            run.exit,
            BlockExit::Checkpoint(ExecutionKey::new(BANK, GuestPc::new(VA + 8)))
        );
        assert_eq!(
            ctx.r(2),
            3,
            "the two straight ops retired before the checkpoint"
        );
        assert_eq!(ctx.r(4), 0, "the delay slot must not have run");
    }

    #[test]
    fn jr_snapshots_target_before_a_delay_slot_that_overwrites_the_source() {
        // jr $t0 ; addiu $t0,$zero,0x1234 (delay overwrites $t0)
        let catalog = catalog_of(&[0x0100_0008, 0x2408_1234]);
        let mut ctx = RecompContext::new();
        ctx.set_r(8, 0x8000_2000);
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(ctx.r_u32(8), 0x1234, "the delay slot ran");
        assert_eq!(
            run.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_2000),
            },
            "the transfer used the pre-delay snapshot"
        );
        assert_eq!(run.instructions, 2);
    }

    #[test]
    fn falling_out_of_the_bank_hands_the_virtual_pc_to_the_mapping_layer() {
        // A single straight op whose fallthrough is outside the admitted bank.
        let catalog = catalog_of(&[0x2402_0001]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(
            run.exit,
            BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(VA + 4),
            }
        );
        assert_eq!(run.instructions, 1);
    }

    #[test]
    fn a_data_hole_between_spans_is_never_executed() {
        // Two disjoint spans with a hole at VA+4; entering the hole faults typed.
        let bank = CodeBank::from_spans(
            BANK,
            vec![
                CodeSpan::new(BANK, GuestPc::new(VA), vec![0x2402_0001]).unwrap(),
                CodeSpan::new(BANK, GuestPc::new(VA + 8), vec![0x2403_0002]).unwrap(),
            ],
        )
        .unwrap();
        let mut catalog = CodeCatalog::new();
        catalog.register(bank).unwrap();
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA + 4, 8, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        assert_eq!(run.instructions, 0);
    }

    #[test]
    fn self_loop_runs_its_delay_slot_then_yields() {
        // beq $zero,$zero,self ; addiu $a0,$zero,7 (delay)
        let catalog = catalog_of(&[0x1000_FFFF, 0x2404_0007]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert_eq!(
            run.exit,
            BlockExit::Yield(ExecutionKey::new(BANK, GuestPc::new(VA)))
        );
        assert_eq!(run.instructions, 2);
        assert_eq!(
            ctx.r(4),
            7,
            "the self-loop delay slot runs before the yield"
        );
    }

    // -- MMIO seam (interpreter side) --------------------------------------
    //
    // A minimal in-crate mock port standing in for the runtime's real device
    // model: it owns ONE modeled register value and claims exactly ONE KSEG1
    // window (`0xFFFF_FFFF_A460_0000..A460_1000`, the PI block). Everything
    // outside that window is `NotMmio`, so it exercises the load-bearing
    // property that an MMIO window does not make arbitrary addresses succeed.
    // The runtime-side integration test (`fn64-runtime/tests/`) proves the SAME
    // seam against the crate's actual `DeviceFabric`/`MmioSpace` state.
    struct MockPiPort {
        /// The one modeled register's value (a PI_STATUS-like word).
        reg: u32,
        /// Reads/writes observed, for asserting the port was actually hit.
        reads: u32,
        writes: u32,
    }

    // The single register this mock models: PI_STATUS at KSEG1 0xA460_0010,
    // sign-extended to the 64-bit effective address the guest computes.
    const PI_STATUS_VADDR: u64 = 0xFFFF_FFFF_A460_0010;
    const PI_WINDOW_LO: u64 = 0xFFFF_FFFF_A460_0000;
    const PI_WINDOW_HI: u64 = 0xFFFF_FFFF_A460_1000;

    impl MmioPort for MockPiPort {
        fn read_w(&mut self, vaddr: u64) -> MmioOutcome<u32> {
            if !(PI_WINDOW_LO..PI_WINDOW_HI).contains(&vaddr) {
                return MmioOutcome::NotMmio;
            }
            if vaddr == PI_STATUS_VADDR {
                self.reads += 1;
                MmioOutcome::Handled(self.reg)
            } else {
                // In-window but unmodeled register: a loud typed fault, never a
                // silent 0 (mirrors MmioSpace::read_w's panic-to-fault stance).
                MmioOutcome::Fault { addr: vaddr }
            }
        }
        fn write_w(&mut self, vaddr: u64, value: u32) -> MmioOutcome<()> {
            if !(PI_WINDOW_LO..PI_WINDOW_HI).contains(&vaddr) {
                return MmioOutcome::NotMmio;
            }
            if vaddr == PI_STATUS_VADDR {
                self.writes += 1;
                self.reg = value;
                MmioOutcome::Handled(())
            } else {
                MmioOutcome::Fault { addr: vaddr }
            }
        }
    }

    #[test]
    fn interpreted_mmio_load_gets_the_modeled_register_value() {
        // lui $t0,0xA460 ; lw $v0,0x10($t0) ; jr $ra ; nop
        // The lw's effective address is the modeled PI_STATUS; the interpreter
        // must return the port's value, not read RDRAM (which would fault).
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0010, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0xDEAD_BEEF,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        assert_eq!(port.reads, 1, "the modeled register was read once");
        // Word register value sign-extends into the GPR exactly as a real LW.
        assert_eq!(ctx.r(2), 0xFFFF_FFFF_DEAD_BEEF);
        assert!(matches!(run.exit, BlockExit::ResolveTransfer { .. }));
    }

    #[test]
    fn interpreted_mmio_store_updates_the_modeled_register_state() {
        // lui $t0,0xA460 ; ori $v0,$zero,0 ; sw $v0,0x10($t0) ; jr $ra ; nop
        // A store of 0 to PI_STATUS updates the modeled state through the port.
        let catalog = catalog_of(&[
            0x3C08_A460, // lui $t0,0xA460
            0x3402_0000, // ori $v0,$zero,0
            0xAD02_0010, // sw $v0,0x10($t0)
            0x03E0_0008, // jr $ra
            0x0000_0000, // nop
        ]);
        let mut port = MockPiPort {
            reg: 0b11, // busy+error set, as after a DMA start
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 64];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        assert_eq!(port.writes, 1, "the modeled register was written once");
        assert_eq!(port.reg, 0, "the store updated modeled device state");
    }

    #[test]
    fn a_non_mmio_out_of_rdram_load_still_faults_typed_with_a_port_present() {
        // The load-bearing safety property: an MMIO window present must NOT make
        // an arbitrary out-of-RDRAM address succeed. lui $t0,0x8000 ; lw
        // $v0,0x40($t0) reads 0x8000_0040 — outside the 16-byte rdram AND
        // outside the port's PI window — so it must be a typed MemoryFault, the
        // same as with no port at all.
        let catalog = catalog_of(&[0x3C08_8000, 0x8D02_0040, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0xDEAD_BEEF,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { addr },
                ..
            }) => assert_eq!(addr, 0xFFFF_FFFF_8000_0040),
            other => panic!("expected typed MemoryFault, got {other:?}"),
        }
        assert_eq!(port.reads, 0, "the port was not consulted-as-handled");
        assert_eq!(ctx.r(2), 0, "the faulting load wrote no register");
    }

    #[test]
    fn an_in_window_unmodeled_register_is_a_typed_fault_not_a_nop() {
        // A load in the PI window but at an unmodeled offset (0x14) is a typed
        // MemoryFault (the port's Fault outcome), never a silent success.
        // lui $t0,0xA460 ; lw $v0,0x14($t0)
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0014, 0x03E0_0008, 0x0000_0000]);
        let mut port = MockPiPort {
            reg: 0,
            reads: 0,
            writes: 0,
        };
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = run_bank_with_mmio(
            &catalog,
            BANK,
            ExecutionKey::new(BANK, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
            &mut port,
        )
        .unwrap();
        match run.exit {
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { addr },
                ..
            }) => assert_eq!(addr, 0xFFFF_FFFF_A460_0014),
            other => panic!("expected typed MemoryFault for unmodeled register, got {other:?}"),
        }
    }

    #[test]
    fn run_bank_default_no_mmio_still_faults_an_mmio_address() {
        // Without a port (plain run_bank), an MMIO-window load is just an
        // out-of-RDRAM MemoryFault — proving the seam is opt-in and the default
        // path is byte-identical to before it existed.
        let catalog = catalog_of(&[0x3C08_A460, 0x8D02_0010, 0x03E0_0008, 0x0000_0000]);
        let mut ctx = RecompContext::new();
        let run = run(&catalog, VA, 8, &mut ctx).unwrap();
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::MemoryFault { .. },
                ..
            })
        ));
    }
}
