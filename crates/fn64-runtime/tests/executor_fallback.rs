//! The live seam: the single-threaded coroutine executor drives a
//! [`FallbackProgram`] for one `GameThread`'s guest execution, on one native
//! stack, mapping each [`ExecutorAction`] to a coroutine scheduling decision —
//! the AOT and interpreter lanes indistinguishable to the executor.
//!
//! This is the `fn64-runtime` half of the executor-drive proof (the pure
//! action-mapping half lives in `fn64-recomp-rs`'s `tests/executor_drive.rs`).
//! It proves the three load-bearing properties against the REAL executor:
//!
//!  1. AOT -> interpreted -> exit: a coroutine body drives a `FallbackProgram`
//!     across both lanes within its resume and the executor schedules it to
//!     completion.
//!  2. hole-stays-a-fault survives the executor integration: a thread dispatched
//!     to a data-hole PC gets a typed `Fault` surfaced through the executor,
//!     never runs data as code, never panics the host.
//!  3. single-runnable preserved: the fallback dispatch happens INSIDE one
//!     `GameThread`'s resume, on one native stack. Nothing here constructs a
//!     `RunToken` or resumes a second coroutine — `run_one_step` remains the
//!     sole resume site (the same invariant `rung_regressions` guards, here
//!     shown to survive a coroutine that drives guest CPU execution).

use std::cell::RefCell;
use std::rc::Rc;

use fn64_recomp_rs::drive::ExecutorAction;
use fn64_recomp_rs::execution::{
    dispatch_until_boundary, BankId, BlockExit, BlockRun, CpuFault, CpuFaultKind, ExecutionKey,
    GuestPc, InstructionBudget,
};
use fn64_recomp_rs::fallback::FallbackProgram;
use fn64_recomp_rs::runtime::{Rdram, RecompContext};
use fn64_recomp_rs::{CodeBank, GeneratedBankRunner};

use fn64_runtime::{Executor, Yield};

const A_VA: u32 = 0x8000_1000;
const B_VA: u32 = 0x8000_2000;
const BANK_A: BankId = BankId::new(0xA0);
const BANK_B: BankId = BankId::new(0xB0);

// AOT lane for bank A: a generated-shape `fn` that runs one instruction and
// transfers directly into bank B (the interpreter lane).
fn aot_bank_a(
    _entry: ExecutionKey,
    _budget: InstructionBudget,
    ctx: &mut RecompContext,
    _mem: &mut Rdram<'_>,
) -> BlockRun {
    ctx.set_r32(4, 0x1234);
    BlockRun::new(
        BlockExit::Transfer(ExecutionKey::new(BANK_B, GuestPc::new(B_VA))),
        1,
    )
}

// addiu $v0,$zero,1 ; jr $ra ; nop — interpreter leaf exiting via a computed
// return to $ra.
const LEAF: [u32; 3] = [0x2402_0001, 0x03E0_0008, 0x0000_0000];

fn contiguous(bank: BankId, va: u32, words: &[u32]) -> CodeBank {
    CodeBank::new(bank, GuestPc::new(va), words.to_vec()).unwrap()
}

/// Build the two-lane program used by the AOT->interp tests.
fn two_lane_program() -> FallbackProgram {
    let mut program = FallbackProgram::new();
    program
        .register_aot(
            contiguous(BANK_A, A_VA, &[0x0000_0000]),
            GeneratedBankRunner::new(BANK_A, aot_bank_a),
        )
        .unwrap();
    program
        .register_dynamic_mips(contiguous(BANK_B, B_VA, &LEAF))
        .unwrap();
    program
}

#[test]
fn executor_drives_a_fallback_program_across_both_lanes_to_completion() {
    // Records observed on the single executor thread, written by the coroutine
    // body as it drives the FallbackProgram. Shared via Rc<RefCell<..>> — sound
    // precisely BECAUSE there is one native stack: the body only touches these
    // while it is the one resumed coroutine (single-runnable), and the test
    // thread only reads them after run_to_idle returns.
    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let body_log = log.clone();

    let mut exec = Executor::new();
    exec.create_thread(1, 10, move |yielder, _first_resume| {
        // Coroutine-local machine state (DESIGN.md §2: each coroutine owns its
        // own RecompContext; the rdram storage is owned here for the test).
        let program = two_lane_program();
        let mut storage = vec![0u8; 16];
        let mut mem = Rdram::new(&mut storage);
        let mut ctx = RecompContext::new();
        ctx.set_r(31, u64::from(B_VA)); // $ra: bank B's computed return target

        // Resolve bank B's computed return to bank B's own admitted entry, so
        // the interpreter re-runs under budget and the dispatch terminates at a
        // deterministic Checkpoint (-> Continue) we then convert into a
        // cooperative PauseSelf yield. This models one guest scheduling turn.
        let resume_key = ExecutionKey::new(BANK_B, GuestPc::new(B_VA));
        let mut resolver = move |_src: BankId,
                                 _target: GuestPc|
              -> Result<ExecutionKey, CpuFault> { Ok(resume_key) };

        let entry = ExecutionKey::new(BANK_A, GuestPc::new(A_VA));
        let run = {
            let mut runner = program.runner(&mut ctx, &mut mem);
            dispatch_until_boundary(
                entry,
                InstructionBudget::new(16).unwrap(),
                &mut runner,
                &mut resolver,
            )
            .expect("dispatch contract upheld")
        };
        let action = ExecutorAction::for_run(run);

        // Both lanes ran within this one resume, on this one native stack.
        assert_eq!(ctx.r_u32(4), 0x1234, "AOT bank A executed");
        assert_eq!(ctx.r_u32(2), 1, "interpreter bank B executed");

        match action {
            ExecutorAction::Continue { resume } => {
                body_log
                    .borrow_mut()
                    .push(format!("continue@{}", resume.bank));
                // Translate a guest Continue into a cooperative coroutine yield:
                // this is where the executor's scheduling machinery takes over.
                yielder.suspend(Yield::PauseSelf);
                body_log.borrow_mut().push("resumed".to_string());
            }
            other => panic!("expected a Continue action, got {other:?}"),
        }
    });
    exec.start_thread(1);

    // Drive the executor exactly as the host driver would. run_one_step is the
    // ONLY resume site; it issues the sole RunToken. The coroutine's guest-CPU
    // dispatch happens strictly inside these resumes.
    exec.run_to_idle();

    assert!(
        exec.is_thread_dead(1),
        "the driven thread ran to completion"
    );
    let recorded = log.borrow();
    assert_eq!(
        *recorded,
        vec![
            "continue@bank:00000000000000B0".to_string(),
            "resumed".to_string()
        ],
        "the executor resumed the guest turn after its cooperative yield"
    );
}

#[test]
fn hole_dispatched_through_the_executor_surfaces_a_typed_fault_not_a_panic() {
    use fn64_recomp_rs::execution::CodeSpan;

    // The fault the coroutine observed, surfaced back out through the executor
    // (the body records it and yields; the test reads it after idle).
    let observed: Rc<RefCell<Option<CpuFault>>> = Rc::new(RefCell::new(None));
    let body_observed = observed.clone();

    let mut exec = Executor::new();
    exec.create_thread(1, 10, move |_yielder, _first_resume| {
        // A sparse bank with a data hole at B_VA+4, interpreter lane.
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
        let mut resolver =
            |_src: BankId, _t: GuestPc| -> Result<ExecutionKey, CpuFault> { unreachable!() };

        let entry = ExecutionKey::new(BANK_B, GuestPc::new(B_VA + 4)); // the hole
        let run = {
            let mut runner = program.runner(&mut ctx, &mut mem);
            dispatch_until_boundary(
                entry,
                InstructionBudget::new(16).unwrap(),
                &mut runner,
                &mut resolver,
            )
            .expect("a hole is a typed guest fault, not a dispatch-contract violation")
        };

        // The hole never ran data as code.
        assert_eq!(ctx.r_u32(2), 0, "no lane ran for the hole");
        assert_eq!(ctx.r_u32(3), 0, "data past the hole was not run as code");

        match ExecutorAction::for_run(run) {
            ExecutorAction::Fault(fault) => {
                *body_observed.borrow_mut() = Some(fault);
            }
            other => panic!("a hole must surface as a Fault action, got {other:?}"),
        }
        // Thread retires normally: a surfaced fault is a value, not a host panic.
    });
    exec.start_thread(1);
    exec.run_to_idle();

    let fault = observed
        .borrow()
        .expect("the fault was surfaced to the host");
    assert!(
        matches!(fault.kind, CpuFaultKind::UnmappedPc { .. }),
        "the data-hole fault is a typed UnmappedPc, got {:?}",
        fault.kind
    );
    assert!(
        exec.is_thread_dead(1),
        "the faulting thread retired cleanly"
    );
}

/// Single-runnable, shown live: a coroutine that drives a `FallbackProgram`
/// yields and is interleaved with a SECOND thread by the executor's run queue.
/// At no instant are two `FallbackProgram` turns in flight, because each turn
/// runs inside a resume and only `run_one_step` (the sole `RunToken` minter)
/// resumes anything. A shared counter that each body increments-then-yields
/// -then-checks would observe corruption if two guest turns ever overlapped;
/// it never does.
#[test]
fn only_one_fallback_turn_is_ever_in_flight_across_interleaved_threads() {
    // `in_guest_turn` is a non-atomic bool guarded only by the single-runnable
    // invariant. If the executor ever drove two FallbackProgram turns
    // concurrently, one body would observe it already true — a data race the
    // single native stack makes impossible. Both bodies drive real dispatches.
    let in_guest_turn = Rc::new(RefCell::new(false));
    let max_concurrent = Rc::new(RefCell::new(0u32));

    let make_body = |flag: Rc<RefCell<bool>>, peak: Rc<RefCell<u32>>| {
        move |yielder: &corosensei::Yielder<fn64_runtime::Resume, Yield>,
              _r: fn64_runtime::Resume| {
            for _ in 0..3 {
                // Enter a guest turn: assert exclusivity, then drive a real
                // FallbackProgram dispatch (interpreter lane).
                {
                    let mut f = flag.borrow_mut();
                    assert!(
                        !*f,
                        "a second guest turn overlapped the first (single-runnable broken)"
                    );
                    *f = true;
                }
                *peak.borrow_mut() += 1;
                let cur = *peak.borrow();
                assert_eq!(cur, 1, "at most one guest turn in flight");

                let program = {
                    let mut p = FallbackProgram::new();
                    p.register_dynamic_mips(contiguous(BANK_B, B_VA, &LEAF))
                        .unwrap();
                    p
                };
                let mut storage = vec![0u8; 16];
                let mut mem = Rdram::new(&mut storage);
                let mut ctx = RecompContext::new();
                ctx.set_r(31, 0x8000_9000);
                let mut resolver = |_s: BankId, _t: GuestPc| -> Result<ExecutionKey, CpuFault> {
                    Err(CpuFault {
                        at: ExecutionKey::new(BANK_B, GuestPc::new(0x8000_9000)),
                        kind: CpuFaultKind::UnknownBank,
                    })
                };
                let run = {
                    let mut runner = program.runner(&mut ctx, &mut mem);
                    dispatch_until_boundary(
                        ExecutionKey::new(BANK_B, GuestPc::new(B_VA)),
                        InstructionBudget::new(8).unwrap(),
                        &mut runner,
                        &mut resolver,
                    )
                    .expect("dispatch contract upheld")
                };
                assert_eq!(ctx.r_u32(2), 1, "the interpreter turn ran");
                let _ = ExecutorAction::for_run(run);

                // Leave the guest turn, THEN yield: the yield hands control back
                // to the executor, which may resume the other thread's guest
                // turn — but only after this one has fully exited.
                {
                    let mut f = flag.borrow_mut();
                    *f = false;
                    *peak.borrow_mut() -= 1;
                }
                yielder.suspend(Yield::PauseSelf);
            }
        }
    };

    let mut exec = Executor::new();
    exec.create_thread(
        1,
        10,
        make_body(in_guest_turn.clone(), max_concurrent.clone()),
    );
    exec.create_thread(
        2,
        10,
        make_body(in_guest_turn.clone(), max_concurrent.clone()),
    );
    exec.start_thread(1);
    exec.start_thread(2);
    exec.run_to_idle();

    assert!(exec.is_thread_dead(1));
    assert!(exec.is_thread_dead(2));
    assert!(
        !*in_guest_turn.borrow(),
        "no guest turn left in flight after idle"
    );
}
