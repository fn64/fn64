//! The seam by which a single-threaded coroutine executor drives one thread's
//! guest execution through the [`FallbackProgram`](crate::fallback::FallbackProgram)
//! fallback lane and gets back a **scheduling decision** it understands.
//!
//! # What this adds, and why it is only a *type seam*
//!
//! [`FallbackProgram`](crate::fallback::FallbackProgram) already runs one turn
//! of guest code — AOT where a generated runner is admitted, the
//! [`crate::interp`] interpreter where a bank is admitted-but-runner-less —
//! behind the uniform [`BlockExit`] contract, and
//! [`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)
//! already chains those turns until guest execution must hand control back
//! (`docs/UNIVERSAL-RUNTIME-PLAN.md`). What was missing was the last mapping:
//! *given the boundary a dispatch reached, what should the executor DO with the
//! thread* — resume it again, yield, deliver a fault, or retire it.
//!
//! [`ExecutorAction`] is that decision, and [`ExecutorAction::for_boundary`] is
//! the total function from a [`BlockExit`] to it. It is deliberately pure and
//! lane-agnostic: it takes only the *exit variant*, never a hint of whether an
//! AOT runner or the interpreter produced it, so **an AOT turn and an
//! interpreted turn that reach the same `BlockExit` map to the same action** —
//! the third load-bearing property. The mapping cannot even observe the lane;
//! that indistinguishability is enforced by the type, not by review.
//!
//! # The single-runnable invariant this seam must not break
//!
//! `docs/DESIGN.md` §2 (recommendation (b)): exactly one native call stack is
//! ever live in guest code, because exactly one coroutine is ever resumed at a
//! time, gated by the non-`Copy`, privately-constructed `RunToken` that only
//! the executor's run loop can mint. This seam adds **no** new way to enter
//! guest code: it never resumes a coroutine, never spawns a thread, never
//! constructs a `RunToken`. A coroutine body, already running on its own stack
//! inside one `resume`, calls [`dispatch_until_boundary`] (synchronous, on that
//! same stack) and then consults [`ExecutorAction::for_boundary`] to decide how
//! to yield. Driving a `FallbackProgram` is therefore *inside* one thread's
//! resume — it cannot manufacture a second runnable guest stack, because it
//! holds nothing that could resume a second coroutine. That is a type-level
//! argument, not a discipline: this module has no access to a `RunToken` and no
//! dependency on the executor at all.
//!
//! # Hole-stays-a-fault survives the seam
//!
//! Admission is resolved by [`FallbackProgram::run`] *before* either lane runs,
//! so an unmapped/data-hole/unaligned PC is already a typed
//! [`BlockExit::Fault`] with zero instructions before it ever reaches
//! [`dispatch_until_boundary`], which propagates it unchanged. This module maps
//! that `Fault` to [`ExecutorAction::Fault`] — a terminal, thread-retiring
//! decision that never loops back into guest code. Data is never run as code,
//! and no path here panics on a fault: the fault is a value the executor acts
//! on.

use crate::execution::{BlockExit, CpuFault, DispatchRun, ExecutionKey, GuestPc};

/// The scheduling decision a single-threaded executor should carry out for a
/// guest thread after one [`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)
/// turn driven through the fallback lane.
///
/// This is the fallback lane's projection onto the executor's own vocabulary of
/// thread outcomes (resume / yield / retire) — the analogue, for *guest CPU
/// execution*, of the `Yield`/`Resume` shapes the coroutine layer already
/// enumerates. It is intentionally small and closed: every [`BlockExit`] maps
/// to exactly one variant, so a driving executor handles guest execution with a
/// single exhaustive `match` and no residual "what else could it be" path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorAction {
    /// Guest execution reached a clean re-entry point (a proven [`Transfer`],
    /// a deterministic [`Checkpoint`], or a resolved computed transfer that the
    /// dispatcher already followed to a bank-qualified key). The executor may
    /// resume this thread's guest execution at `resume` on its next turn
    /// without any external condition being satisfied first. Whether it does so
    /// immediately or first lets a higher-priority thread run is the executor's
    /// existing scheduling policy — this action does not force either.
    ///
    /// [`Transfer`]: BlockExit::Transfer
    /// [`Checkpoint`]: BlockExit::Checkpoint
    Continue { resume: ExecutionKey },
    /// The guest cooperatively yielded ([`BlockExit::Yield`], e.g. a `j self`
    /// idle spin). The executor should give up the CPU for this thread and let
    /// the run queue pick the next one; the thread is immediately runnable
    /// again and resumes guest execution at `resume`. This is the guest-CPU
    /// analogue of the coroutine layer's `Yield::PauseSelf`.
    Yield { resume: ExecutionKey },
    /// The guest made a host/environment call ([`BlockExit::HostCall`]): control
    /// must leave guest code to run the named host routine, after which guest
    /// execution resumes at `resume`. The `vram` names the call target for the
    /// host dispatcher. Modeling the actual host-call effect (running the shim,
    /// charging its cost) is out of this seam's scope — device fabric and
    /// guest-cycle accounting remain open (`docs/UNIVERSAL-RUNTIME-PLAN.md`
    /// U2+); this action only names that the boundary was reached.
    HostCall { vram: GuestPc, resume: ExecutionKey },
    /// A computed transfer whose target the active mapping layer must resolve to
    /// exactly one bank-qualified [`ExecutionKey`] before another block may run
    /// ([`BlockExit::ResolveTransfer`]). Reached only when
    /// [`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)
    /// was driven *without* a resolver, or a resolver deliberately deferred;
    /// with a resolver installed the dispatcher follows the transfer itself and
    /// this variant does not occur. Carrying `source_bank`/`target_pc`
    /// unchanged lets the executor resolve it against the currently-active
    /// overlay set (open: real overlay admission, U2+).
    Resolve {
        source_bank: crate::execution::BankId,
        target_pc: GuestPc,
    },
    /// A computed call whose target still needs the active mapping/host ABI
    /// resolver. The already-executed link destination remains bank-qualified.
    ResolveCall {
        source_bank: crate::execution::BankId,
        target_pc: GuestPc,
        resume: ExecutionKey,
    },
    /// The guest entry returned through its explicit thread-return sentinel.
    /// This is a clean retirement, distinct from a CPU fault or unmapped PC.
    ThreadReturn,
    /// A typed guest CPU fault surfaced through the fallback lane
    /// ([`BlockExit::Fault`]): an unmapped/data-hole/unaligned PC, an unknown
    /// bank, a memory fault, or an interpreter coverage-boundary opcode. This is
    /// terminal for the turn and never loops back into guest code — the executor
    /// surfaces it (retiring or faulting the thread) rather than running data as
    /// code. A hole stays a fault, all the way through the executor seam.
    Fault(CpuFault),
}

impl ExecutorAction {
    /// The total, lane-agnostic mapping from a dispatch boundary to the
    /// executor's scheduling decision.
    ///
    /// Pure by construction: it inspects only the [`BlockExit`] variant, so it
    /// is impossible for it to make a different decision for an AOT turn than
    /// for an interpreted turn that reached the same boundary. That is the
    /// "AOT and interpreted turns are indistinguishable to the executor"
    /// property, discharged at the type level.
    pub fn for_boundary(exit: BlockExit) -> Self {
        match exit {
            BlockExit::Transfer(resume) | BlockExit::Checkpoint(resume) => {
                ExecutorAction::Continue { resume }
            }
            BlockExit::Yield(resume) => ExecutorAction::Yield { resume },
            BlockExit::HostCall { vram, resume } => ExecutorAction::HostCall { vram, resume },
            BlockExit::ResolveTransfer {
                source_bank,
                target_pc,
            } => ExecutorAction::Resolve {
                source_bank,
                target_pc,
            },
            BlockExit::ResolveCall {
                source_bank,
                target_pc,
                resume,
            } => ExecutorAction::ResolveCall {
                source_bank,
                target_pc,
                resume,
            },
            BlockExit::ThreadReturn => ExecutorAction::ThreadReturn,
            BlockExit::Fault(fault) => ExecutorAction::Fault(fault),
        }
    }

    /// The scheduling decision for a completed
    /// [`dispatch_until_boundary`](crate::execution::dispatch_until_boundary)
    /// turn. A convenience over [`ExecutorAction::for_boundary`] that reads the
    /// boundary off the [`DispatchRun`]; the completed guest work
    /// (`run.instructions`, `run.blocks`) is the caller's to charge to the
    /// device/clock layer (open: guest-cycle accounting, U2+).
    pub fn for_run(run: DispatchRun) -> Self {
        Self::for_boundary(run.exit)
    }

    /// Whether this action retires the current guest turn without any
    /// possibility of re-entering guest code on this thread as a direct
    /// consequence — i.e. a [`Fault`](ExecutorAction::Fault). Lets a driving
    /// executor recognize the hole-stays-a-fault terminal case with a named
    /// predicate rather than an open-coded `matches!`.
    pub fn is_terminal_fault(self) -> bool {
        matches!(self, ExecutorAction::Fault(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{BankId, CpuFaultKind};

    const BANK: BankId = BankId::new(0x51);
    const KEY: ExecutionKey = ExecutionKey::new(BANK, GuestPc::new(0x8000_1000));

    #[test]
    fn every_block_exit_maps_to_exactly_one_action() {
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::Transfer(KEY)),
            ExecutorAction::Continue { resume: KEY }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::Checkpoint(KEY)),
            ExecutorAction::Continue { resume: KEY }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::Yield(KEY)),
            ExecutorAction::Yield { resume: KEY }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::HostCall {
                vram: GuestPc::new(0x8000_2000),
                resume: KEY,
            }),
            ExecutorAction::HostCall {
                vram: GuestPc::new(0x8000_2000),
                resume: KEY,
            }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::ResolveTransfer {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_3000),
            }),
            ExecutorAction::Resolve {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_3000),
            }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::ResolveCall {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_4000),
                resume: KEY,
            }),
            ExecutorAction::ResolveCall {
                source_bank: BANK,
                target_pc: GuestPc::new(0x8000_4000),
                resume: KEY,
            }
        );
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::ThreadReturn),
            ExecutorAction::ThreadReturn
        );
        let fault = CpuFault {
            at: KEY,
            kind: CpuFaultKind::UnmappedPc {
                bank_start: 0x8000_1000,
                bank_end: 0x8000_1010,
            },
        };
        assert_eq!(
            ExecutorAction::for_boundary(BlockExit::Fault(fault)),
            ExecutorAction::Fault(fault)
        );
    }

    #[test]
    fn only_a_fault_is_terminal() {
        assert!(ExecutorAction::Fault(CpuFault {
            at: KEY,
            kind: CpuFaultKind::UnknownBank,
        })
        .is_terminal_fault());
        assert!(!ExecutorAction::Continue { resume: KEY }.is_terminal_fault());
        assert!(!ExecutorAction::Yield { resume: KEY }.is_terminal_fault());
        assert!(!ExecutorAction::ThreadReturn.is_terminal_fault());
    }
}
