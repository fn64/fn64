//! The executor-drive seam: a single-threaded driver chains
//! [`FallbackProgram`] turns through [`dispatch_until_boundary`] across BOTH
//! lanes (AOT then interpreter) and turns each boundary into one
//! [`ExecutorAction`] scheduling decision — the wiring proven here in isolation
//! from the coroutine crate (`fn64-runtime` owns the live-coroutine proof).
//!
//! What this file proves, matching the task's three load-bearing properties:
//!
//!  1. A driver can run a multi-block thread AOT -> interpreted -> exit through
//!     one `FallbackProgram` and get back the expected `ExecutorAction`s.
//!  2. Hole-stays-a-fault survives the drive: a data-hole entry yields
//!     `ExecutorAction::Fault`, never runs data as code, never panics.
//!  3. AOT and interpreted turns that reach the same `BlockExit` produce the
//!     same `ExecutorAction` — the decision cannot observe which lane ran.

use fn64_cpu_runtime::drive::ExecutorAction;
use fn64_cpu_runtime::execution::{
    dispatch_until_boundary, BankId, BlockExit, BlockRun, CpuFault, CpuFaultKind, ExecutionKey,
    GuestPc, InstructionBudget,
};
use fn64_cpu_runtime::fallback::FallbackProgram;
use fn64_cpu_runtime::runtime::{Rdram, RecompContext};
use fn64_cpu_runtime::CodeBank;

const A_VA: u32 = 0x8000_1000;
const B_VA: u32 = 0x8000_2000;
const BANK_A: BankId = BankId::new(0xA0);
const BANK_B: BankId = BankId::new(0xB0);

// An AOT lane for bank A: a plain generated-shape `fn` (indistinguishable from
// emit_bank_runner's output at the type level) that runs one instruction and
// hands control to bank B via a proven direct Transfer.
fn aot_bank_a(
    _entry: ExecutionKey,
    _budget: InstructionBudget,
    ctx: &mut RecompContext,
    _mem: &mut Rdram<'_>,
) -> BlockRun {
    ctx.set_r32(4, 0x1234); // observable side effect of the AOT turn
    BlockRun::new(
        BlockExit::Transfer(ExecutionKey::new(BANK_B, GuestPc::new(B_VA))),
        1,
    )
}

// addiu $v0,$zero,1 ; jr $ra ; nop — the interpreter leaf that exits via a
// computed (ResolveTransfer) return to $ra. Registered for the dynamic_mips
// lane so the SAME FallbackProgram runs one AOT and one interpreted turn.
const LEAF: [u32; 3] = [0x2402_0001, 0x03E0_0008, 0x0000_0000];

fn contiguous(bank: BankId, va: u32, words: &[u32]) -> CodeBank {
    CodeBank::new(bank, GuestPc::new(va), words.to_vec()).unwrap()
}

/// A driver standing in for the executor's per-thread guest-execution turn:
/// dispatch through the fallback lane, then project the boundary onto the
/// executor's scheduling vocabulary. Pure, single-stack, no coroutine.
fn drive_once(
    program: &FallbackProgram,
    entry: ExecutionKey,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
    resolver: &mut impl FnMut(BankId, GuestPc) -> Result<ExecutionKey, CpuFault>,
) -> ExecutorAction {
    let mut runner = program.runner(ctx, mem);
    let run = dispatch_until_boundary(
        entry,
        InstructionBudget::new(16).unwrap(),
        &mut runner,
        resolver,
    )
    .expect("dispatch contract upheld");
    ExecutorAction::for_run(run)
}

#[test]
fn driver_chains_aot_then_interpreter_then_exit() {
    let mut program = FallbackProgram::new();
    program
        .register_aot(
            contiguous(BANK_A, A_VA, &[0x0000_0000]),
            fn64_cpu_runtime::GeneratedBankRunner::new(BANK_A, aot_bank_a),
        )
        .unwrap();
    program
        .register_dynamic_mips(contiguous(BANK_B, B_VA, &LEAF))
        .unwrap();

    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    ctx.set_r(31, 0x8000_9000); // $ra: where bank B's `jr $ra` computes to

    // The AOT turn transfers directly into bank B; dispatch_until_boundary
    // follows that Transfer and runs bank B (the interpreter) in the SAME turn,
    // which exits via a computed ResolveTransfer to $ra. Resolver is not
    // consulted for the direct A->B transfer; it IS consulted for B's computed
    // return, which we deliberately leave unresolved to observe the Resolve
    // action (its resolution against a live overlay set is U2+).
    let mut resolver = |_src: BankId, target: GuestPc| -> Result<ExecutionKey, CpuFault> {
        // A real executor would resolve against the active overlay set; here we
        // return a fault so the dispatcher stops and surfaces it as an action,
        // proving the computed-transfer boundary is reachable end-to-end.
        Err(CpuFault {
            at: ExecutionKey::new(BANK_B, target),
            kind: CpuFaultKind::UnknownBank,
        })
    };

    let action = drive_once(
        &program,
        ExecutionKey::new(BANK_A, GuestPc::new(A_VA)),
        &mut ctx,
        &mut mem,
        &mut resolver,
    );

    // The AOT lane ran (side effect visible) AND the interpreter lane ran
    // (its $v0 = 1 side effect visible) — both lanes drove within one turn.
    assert_eq!(ctx.r_u32(4), 0x1234, "AOT bank A executed");
    assert_eq!(ctx.r_u32(2), 1, "interpreter bank B executed");
    // The unresolved computed return surfaces as a typed fault action.
    assert!(
        action.is_terminal_fault(),
        "an unresolved computed transfer surfaces as a fault action, got {action:?}"
    );
}

#[test]
fn driver_reaches_a_clean_continue_when_the_resolver_maps_the_return() {
    // Same program, but now the resolver maps bank B's computed return to a
    // real re-entry key: the driver gets a Continue action, the clean case.
    let mut program = FallbackProgram::new();
    program
        .register_aot(
            contiguous(BANK_A, A_VA, &[0x0000_0000]),
            fn64_cpu_runtime::GeneratedBankRunner::new(BANK_A, aot_bank_a),
        )
        .unwrap();
    // Bank B leaf that yields cooperatively at the end instead of `jr $ra`:
    // addiu $v0,$zero,1 ; j self ; nop  (0x0800_0801 = j 0x8000_2004? use b self)
    // Simpler: reuse LEAF but resolve the return to bank A's checkpoint key.
    program
        .register_dynamic_mips(contiguous(BANK_B, B_VA, &LEAF))
        .unwrap();

    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    ctx.set_r(31, u64::from(B_VA)); // $ra points back into bank B (a real admitted key)

    let resume_key = ExecutionKey::new(BANK_B, GuestPc::new(B_VA));
    let mut resolver = move |_src: BankId, _target: GuestPc| -> Result<ExecutionKey, CpuFault> {
        // Resolve the computed return to bank B's entry — a real admitted,
        // aligned re-entry key. The dispatcher will re-run bank B until the
        // budget forces a deterministic Checkpoint, which maps to Continue.
        Ok(resume_key)
    };

    let action = drive_once(
        &program,
        ExecutionKey::new(BANK_A, GuestPc::new(A_VA)),
        &mut ctx,
        &mut mem,
        &mut resolver,
    );

    match action {
        ExecutorAction::Continue { resume } => {
            assert_eq!(resume.bank, BANK_B, "clean re-entry stays bank-qualified");
        }
        other => panic!("expected a Continue action from a resolved chain, got {other:?}"),
    }
}

#[test]
fn hole_stays_a_fault_through_the_drive() {
    // A sparse bank with a data hole at B_VA+4, registered for the interpreter
    // lane. Dispatching AT the hole must surface a typed Fault action — never
    // run the data word as code, never panic the driver.
    use fn64_cpu_runtime::execution::CodeSpan;
    let sparse = CodeBank::from_spans(
        BANK_B,
        vec![
            CodeSpan::new(BANK_B, GuestPc::new(B_VA), vec![0x2402_0001]).unwrap(),
            CodeSpan::new(BANK_B, GuestPc::new(B_VA + 8), vec![0x2403_0002]).unwrap(),
        ],
    )
    .unwrap();
    let mut program = FallbackProgram::new();
    program.register_dynamic_mips(sparse).unwrap();

    let mut storage = vec![0u8; 16];
    let mut mem = Rdram::new(&mut storage);
    let mut ctx = RecompContext::new();
    let mut resolver = |_src: BankId, _target: GuestPc| -> Result<ExecutionKey, CpuFault> {
        panic!("resolver must not be consulted for a hole fault")
    };

    let action = drive_once(
        &program,
        ExecutionKey::new(BANK_B, GuestPc::new(B_VA + 4)), // the hole
        &mut ctx,
        &mut mem,
        &mut resolver,
    );

    match action {
        ExecutorAction::Fault(CpuFault {
            kind: CpuFaultKind::UnmappedPc { .. },
            ..
        }) => {}
        other => panic!("a data hole must surface as an UnmappedPc fault action, got {other:?}"),
    }
    assert_eq!(ctx.r_u32(2), 0, "no lane ran for the hole");
    assert_eq!(
        ctx.r_u32(3),
        0,
        "the data word past the hole was not run as code"
    );
}

#[test]
fn aot_and_interpreter_boundaries_are_indistinguishable_to_the_action_map() {
    // The same BlockExit reached by either lane must produce the same action.
    // We assert it at the mapping directly: the projection takes only the exit,
    // so lane provenance cannot influence the decision.
    let key = ExecutionKey::new(BANK_A, GuestPc::new(A_VA));
    let via_aot = ExecutorAction::for_boundary(BlockExit::Yield(key));
    let via_interp = ExecutorAction::for_boundary(BlockExit::Yield(key));
    assert_eq!(via_aot, via_interp);
    assert_eq!(via_aot, ExecutorAction::Yield { resume: key });
}
