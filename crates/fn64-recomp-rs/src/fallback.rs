//! The interpreter-backed execution *fallback* wired behind the block
//! dispatcher's [`BlockExit`] contract.
//!
//! # What this adds
//!
//! [`BlockProgram`](crate::execution::BlockProgram) pairs an admitted
//! [`CodeBank`] with a generated (AOT) runner and *panics* if an admitted bank
//! has no runner. Execution closure (`docs/UNIVERSAL-RUNTIME-PLAN.md`) requires
//! that every admitted CPU destination runs even before a static AOT runner
//! exists for it. [`FallbackProgram`] is that mechanism: it registers banks in
//! two lanes —
//!
//! - **AOT** ([`EvidenceClass::BlockAot`]): a generated
//!   [`GeneratedBankRunner`], executed exactly as `BlockProgram` does today; and
//! - **`dynamic_mips`** ([`EvidenceClass::DynamicMips`]): a bank admitted as
//!   code but with no generated runner, executed by the [`crate::interp`]
//!   MIPS-III interpreter against the *same* [`RecompContext`] + [`Rdram`].
//!
//! Because both lanes return the same [`BlockRun`]/[`BlockExit`], the dispatcher
//! ([`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)) does
//! not care which produced a given turn.
//!
//! # The load-bearing safety property: a hole stays a fault
//!
//! [`CodeCatalog::resolve`] admission runs **in front of** either lane, exactly
//! as it front-runs the generated runner in `BlockProgram::run`. An
//! [`CpuFaultKind::UnmappedPc`] (a real sparse-bank gap), an
//! [`CpuFaultKind::UnknownBank`], or an [`CpuFaultKind::UnalignedPc`] still
//! faults typed with the fallback installed. The interpreter fallback applies
//! *only* to bytes that are themselves admitted code but lack a generated
//! runner — never to an unmapped or data address. A fallback that runs data as
//! code would be worse than a fault, so this admission check is not optional.
//!
//! # `dynamic_mips` is never silent
//!
//! An unsupported instruction (the FPU/COP0/COP2/TLB/exception frontier) does
//! not panic or nop in the interpreter; it returns a typed
//! [`CpuFaultKind::UnsupportedInstruction`] naming the opcode, surfaced through
//! the same `BlockExit::Fault` the dispatcher already handles. The evidence
//! class of every registered bank is recorded and distinguishable
//! ([`FallbackProgram::evidence_class`]).

use std::collections::BTreeMap;

use crate::execution::{
    BlockExit, BlockProgram, BlockRun, BlockRunner, CodeBank, ExecutionKey, GeneratedBankRunner,
    InstructionBudget, ProgramError,
};
use crate::interp::{run_bank_with_mmio, MmioPort, NoMmio};
use crate::runtime::{Rdram, RecompContext};

pub use crate::execution::BankId;

/// How the bytes of an admitted bank are executed. Recorded per bank so a
/// `dynamic_mips` fallback is never silent — it is a distinguishable, queryable
/// execution class distinct from the AOT lanes (`docs/UNIVERSAL-RUNTIME-PLAN.md`
/// §3.2 "evidence class: exact AOT, block AOT, dynamic MIPS, or unsupported").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceClass {
    /// A whole-function AOT body proven against the N64Recomp oracle. Not
    /// registered by this program (its lane is the historical function runner),
    /// but named here so the taxonomy is complete and honest.
    ExactAot,
    /// A generated bank/basic-block runner emitted by
    /// [`emit_bank_runner`](crate::emit::emit_bank_runner).
    BlockAot,
    /// The interpreter fallback: admitted code with no generated runner, run by
    /// the [`crate::interp`] MIPS-III interpreter behind the same contract.
    DynamicMips,
}

/// One installed execution lane for a bank.
enum Lane {
    /// Keep the evolved runner wrapper inside the primary AOT ownership
    /// mechanism so its artifact identity is not discarded as a bare callable.
    Aot(BlockProgram),
    DynamicMips,
}

impl Lane {
    fn evidence_class(&self) -> EvidenceClass {
        match self {
            Lane::Aot(_) => EvidenceClass::BlockAot,
            Lane::DynamicMips => EvidenceClass::DynamicMips,
        }
    }
}

/// A [`BlockProgram`](crate::execution::BlockProgram) extended with an
/// interpreter fallback lane.
///
/// It owns one [`CodeCatalog`](crate::execution::CodeCatalog) — the single
/// admission authority — and a per-bank lane. Registration validates identity
/// and admits the code atomically; [`FallbackProgram::run`] resolves admission
/// before invoking either lane, so a sparse-bank hole can never acquire an
/// execution path from bounding geometry.
#[derive(Default)]
pub struct FallbackProgram {
    code: crate::execution::CodeCatalog,
    lanes: BTreeMap<BankId, Lane>,
}

impl FallbackProgram {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a bank with a generated AOT runner. Identical semantics to
    /// [`BlockProgram::register`](crate::execution::BlockProgram::register): the
    /// runner's embedded [`BankId`] must equal the code's, and neither map may
    /// already contain it. On any error nothing is mutated.
    pub fn register_aot(
        &mut self,
        code: CodeBank,
        runner: GeneratedBankRunner,
    ) -> Result<(), ProgramError> {
        let bank = code.id();
        if runner.bank() != bank {
            return Err(ProgramError::RunnerBankMismatch {
                code_bank: bank,
                runner_bank: runner.bank(),
            });
        }
        if self.contains(bank) {
            return Err(ProgramError::DuplicateBank { bank });
        }
        let mut program = BlockProgram::new();
        program
            .register(code.clone(), runner)
            .expect("runner identity and duplicate admission were checked before registration");
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.lanes.insert(bank, Lane::Aot(program));
        Ok(())
    }

    /// Register a bank for the `dynamic_mips` interpreter fallback: admitted
    /// code with no generated runner. The interpreter executes it behind the
    /// same [`BlockExit`] contract the AOT lane satisfies.
    pub fn register_dynamic_mips(&mut self, code: CodeBank) -> Result<(), ProgramError> {
        let bank = code.id();
        if self.contains(bank) {
            return Err(ProgramError::DuplicateBank { bank });
        }
        self.code
            .register(code)
            .expect("duplicate program bank was checked before catalog registration");
        self.lanes.insert(bank, Lane::DynamicMips);
        Ok(())
    }

    fn contains(&self, bank: BankId) -> bool {
        self.code.bank(bank).is_some() || self.lanes.contains_key(&bank)
    }

    pub fn code(&self) -> &crate::execution::CodeCatalog {
        &self.code
    }

    /// The recorded execution class for a registered bank, or `None` if the
    /// bank is not registered. `dynamic_mips` is never silent: it is queryable.
    pub fn evidence_class(&self, bank: BankId) -> Option<EvidenceClass> {
        self.lanes.get(&bank).map(Lane::evidence_class)
    }

    /// Run one turn at `entry`.
    ///
    /// Admission is checked through [`CodeCatalog::resolve`](crate::execution::CodeCatalog::resolve)
    /// **before** either lane executes: an unaligned PC, an unknown bank, or an
    /// unmapped (sparse-hole/data) address is a typed [`BlockExit::Fault`] with
    /// zero instructions, and no lane runs. Only an admitted-code address
    /// reaches a lane. An interpreter `dynamic_mips` bank that hits an
    /// unsupported instruction returns the typed
    /// [`CpuFaultKind::UnsupportedInstruction`](crate::execution::CpuFaultKind::UnsupportedInstruction)
    /// fault — never a panic or a silent nop.
    pub fn run(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        self.run_with_mmio(entry, budget, ctx, mem, &mut NoMmio)
    }

    /// [`FallbackProgram::run`] with a hardware-register ([`MmioPort`]) door
    /// installed for the `dynamic_mips` interpreter lane.
    ///
    /// The interpreter routes a word load/store to a modeled register through
    /// `port`, giving an interpreted `lw` of a device register a modeled value
    /// and an interpreted `sw` a modeled effect (see [`crate::interp`]'s
    /// `MmioPort` doc). Admission is still resolved *before* either lane runs, so
    /// hole-stays-a-fault is untouched: an unmapped/data-hole/unaligned PC is a
    /// typed [`BlockExit::Fault`] with zero instructions and no lane — and hence
    /// no `port` — runs.
    ///
    /// The **AOT lane is unchanged**: a generated runner reaches memory through
    /// its own open-coded accessors and is not offered the port here. This is an
    /// interpreter→device seam, not a rerouting of the AOT path; wiring the AOT
    /// lane to the device model is separate, still-open scope
    /// (`docs/UNIVERSAL-RUNTIME-PLAN.md` U2). Passing [`NoMmio`] recovers
    /// [`FallbackProgram::run`] exactly.
    pub fn run_with_mmio(
        &self,
        entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        port: &mut dyn MmioPort,
    ) -> BlockRun {
        if let Err(fault) = self.code.resolve(entry) {
            return BlockRun::new(BlockExit::Fault(fault), 0);
        }
        let lane = self.lanes.get(&entry.bank).unwrap_or_else(|| {
            panic!(
                "fallback program invariant violated: admitted {} has no execution lane",
                entry.bank
            )
        });
        match lane {
            Lane::Aot(program) => program.run(entry, budget, ctx, mem),
            Lane::DynamicMips => {
                match run_bank_with_mmio(&self.code, entry.bank, entry, budget, ctx, mem, port) {
                    Ok(run) => run,
                    // The interpreter's coverage boundary is surfaced as the typed
                    // guest fault the dispatcher already understands, so an AOT and
                    // an interpreted turn are indistinguishable to the dispatcher.
                    Err(op) => BlockRun::new(BlockExit::Fault(op.into_cpu_fault()), 0),
                }
            }
        }
    }

    /// A [`BlockRunner`] view bound to `ctx`/`mem`, so a `FallbackProgram` can
    /// drive [`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)
    /// directly. The borrow keeps the program immutable while the runner holds
    /// the mutable machine state.
    pub fn runner<'a, 'r>(
        &'a self,
        ctx: &'a mut RecompContext,
        mem: &'a mut Rdram<'r>,
    ) -> FallbackRunner<'a, 'r> {
        FallbackRunner {
            program: self,
            ctx,
            mem,
            port: None,
        }
    }

    /// A [`BlockRunner`] view like [`FallbackProgram::runner`] but with a
    /// hardware-register [`MmioPort`] door threaded to the `dynamic_mips`
    /// interpreter lane, so a driven dispatch's interpreted word MMIO accesses
    /// reach the modeled device. The `port` outlives the returned runner (it is
    /// the executor-owned device state the turn mutates), keeping the single
    /// device authority on the runtime side.
    pub fn runner_with_mmio<'a, 'r>(
        &'a self,
        ctx: &'a mut RecompContext,
        mem: &'a mut Rdram<'r>,
        port: &'a mut dyn MmioPort,
    ) -> FallbackRunner<'a, 'r> {
        FallbackRunner {
            program: self,
            ctx,
            mem,
            port: Some(port),
        }
    }
}

/// A [`BlockRunner`] adapter that runs turns of a [`FallbackProgram`] against
/// borrowed machine state, routing the interpreter lane's word MMIO accesses to
/// a [`MmioPort`] when one is installed. Produced by
/// [`FallbackProgram::runner`]/[`FallbackProgram::runner_with_mmio`].
pub struct FallbackRunner<'a, 'r> {
    program: &'a FallbackProgram,
    ctx: &'a mut RecompContext,
    mem: &'a mut Rdram<'r>,
    /// `None` recovers the plain (no-device) runner exactly: turns run with a
    /// transient [`NoMmio`] port, byte-identical to before the seam existed.
    port: Option<&'a mut dyn MmioPort>,
}

impl BlockRunner for FallbackRunner<'_, '_> {
    fn run(&mut self, entry: ExecutionKey, budget: InstructionBudget) -> BlockRun {
        match self.port.as_deref_mut() {
            Some(port) => self
                .program
                .run_with_mmio(entry, budget, self.ctx, self.mem, port),
            None => self
                .program
                .run_with_mmio(entry, budget, self.ctx, self.mem, &mut NoMmio),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{CodeSpan, CpuFault, CpuFaultKind, GuestPc};

    const VA: u32 = 0x8000_1000;

    fn contiguous(id: u64, words: &[u32]) -> CodeBank {
        CodeBank::new(BankId::new(id), GuestPc::new(VA), words.to_vec()).unwrap()
    }

    // addiu $v0,$zero,1 ; jr $ra ; nop — a trivial leaf that exits via
    // ResolveTransfer to $ra. Runs identically in both lanes.
    const LEAF: [u32; 3] = [0x2402_0001, 0x03E0_0008, 0x0000_0000];

    #[test]
    fn dynamic_mips_bank_runs_the_interpreter_and_records_its_class() {
        let id = BankId::new(0x70);
        let mut program = FallbackProgram::new();
        program
            .register_dynamic_mips(contiguous(0x70, &LEAF))
            .unwrap();
        assert_eq!(program.evidence_class(id), Some(EvidenceClass::DynamicMips));

        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(31, 0x8000_9000);
        let run = program.run(
            ExecutionKey::new(id, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert_eq!(ctx.r_u32(2), 1);
        assert_eq!(
            run.exit,
            BlockExit::ResolveTransfer {
                source_bank: id,
                target_pc: GuestPc::new(0x8000_9000),
            }
        );
    }

    #[test]
    fn a_hole_still_faults_typed_with_the_fallback_installed() {
        // Sparse bank with a data hole at VA+4, registered for dynamic_mips.
        let id = BankId::new(0x71);
        let sparse = CodeBank::from_spans(
            id,
            vec![
                CodeSpan::new(id, GuestPc::new(VA), vec![0x2402_0001]).unwrap(),
                CodeSpan::new(id, GuestPc::new(VA + 8), vec![0x2403_0002]).unwrap(),
            ],
        )
        .unwrap();
        let mut program = FallbackProgram::new();
        program.register_dynamic_mips(sparse).unwrap();

        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = program.run(
            ExecutionKey::new(id, GuestPc::new(VA + 4)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        );
        assert!(matches!(
            run.exit,
            BlockExit::Fault(CpuFault {
                kind: CpuFaultKind::UnmappedPc { .. },
                ..
            })
        ));
        assert_eq!(run.instructions, 0);
        assert_eq!(ctx.r_u32(2), 0, "no lane runs for a catalog hole");
    }

    #[test]
    fn unsupported_op_in_the_interpreter_is_a_typed_fault_not_a_panic() {
        // DMFC0 remains outside the modeled privileged-register slice, so it
        // remains a typed fault rather than becoming a host panic or no-op.
        let id = BankId::new(0x72);
        let words = [0x4022_4800, 0x03E0_0008, 0x0000_0000];
        let mut program = FallbackProgram::new();
        program
            .register_dynamic_mips(contiguous(0x72, &words))
            .unwrap();

        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        let run = program.run(
            ExecutionKey::new(id, GuestPc::new(VA)),
            InstructionBudget::new(8).unwrap(),
            &mut ctx,
            &mut mem,
        );
        match run.exit {
            BlockExit::Fault(CpuFault {
                at,
                kind: CpuFaultKind::UnsupportedInstruction { word },
            }) => {
                assert_eq!(at, ExecutionKey::new(id, GuestPc::new(VA)));
                assert_eq!(word, 0x4022_4800);
            }
            other => panic!("expected typed UnsupportedInstruction fault, got {other:?}"),
        }
    }

    #[test]
    fn registration_is_bank_qualified_and_rejects_duplicates_across_lanes() {
        let mut program = FallbackProgram::new();
        program
            .register_dynamic_mips(contiguous(0x73, &LEAF))
            .unwrap();
        // A second registration of the same identity is rejected regardless of
        // lane, and leaves the first lane intact.
        assert_eq!(
            program.register_dynamic_mips(contiguous(0x73, &LEAF)),
            Err(ProgramError::DuplicateBank {
                bank: BankId::new(0x73),
            })
        );
        assert_eq!(
            program.evidence_class(BankId::new(0x73)),
            Some(EvidenceClass::DynamicMips)
        );
        assert_eq!(program.evidence_class(BankId::new(0x99)), None);
    }
}
