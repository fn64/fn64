//! `OSThread` lifecycle and the single-runnable-token type.
//!
//! See `docs/DESIGN.md` section 2, recommendation (b): "single executor +
//! stackful coroutines... Only one game thread runs at a time... stops
//! being a discipline every future contributor must maintain across N call
//! sites and becomes physically true." This module provides the two pieces
//! that make that literal:
//!
//! 1. `GameThread` wraps a `corosensei::Coroutine` (a real machine stack,
//!    switched to cooperatively) as the unit libultra calls an `OSThread`.
//! 2. `RunToken` is a zero-sized capability: the only way to call
//!    `GameThread::resume` is to hold one, and `Executor::run_one_step`
//!    (the sole producer of a `RunToken`, see `executor.rs`) never creates
//!    a second one while the first is outstanding -- so "two coroutines'
//!    recompiled code executing concurrently" is not just avoided by
//!    convention, it has no expressible call site: nothing outside
//!    `executor.rs` can construct a `RunToken` at all (private constructor),
//!    and `resume` requires one by value, consumed for the duration of the
//!    call via a borrow that can't be reentered (see its doc comment).

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::trace::ThreadId;

/// Native machine-stack size for each `GameThread`'s coroutine.
/// `corosensei::stack::DefaultStack`'s own `Default` impl (what
/// `Coroutine::new` used before this constant existed) is a hardcoded 1 MiB
/// -- fine for the smaller AKI titles' `RecompiledFuncs` corpora, but OoT's
/// decomp-driven recompile (a much larger, more deeply-nested C call graph;
/// N64Recomp's goto-based codegen keeps every translated function's full
/// MIPS stack frame as native locals, so a deep libultra call chain'S
/// native stack usage is proportionally larger too) blew this 1 MiB budget:
/// `DmaMgr_ThreadEntry` -> `DmaMgr_ProcessRequest` -> `DmaMgr_DmaRomToRam`
/// crashed with a corrupted return address (PC jumping to `0x0`/`0x1`)
/// inside `Executor::run_one_step`'s `resume()` call, the classic signature
/// of a stack overflow that ran past `DefaultStack`'s guard page before the
/// fault was raised (ARM64/macOS does not always fault cleanly on the exact
/// guard-page boundary under heavy stack pressure). 8 MiB matches a
/// generous native-OS-thread default and is cheap (one mmap per
/// `GameThread`, reclaimed on thread death) -- not tuned to a measured
/// high-water mark, just large enough that this failure mode stopped
/// reproducing across repeated OoT boot runs.
const COROUTINE_STACK_SIZE: usize = 8 * 1024 * 1024;

/// libultra priority: `osCreateThread`'s `pri` argument / `osSetThreadPri`.
/// A plain newtype rather than a bare `i32` so a call site can't
/// accidentally compare a raw thread ID against a priority (both would
/// otherwise be interchangeable small integers) -- per `AGENTS.md`'s "types
/// before audits."
pub type Priority = i32;

/// The idle/lowest thread priority libultra reserves (`OS_PRIORITY_IDLE`).
/// Documented in the public libultra manual's Thread Manager section: idle
/// threads run only when nothing else is runnable.
pub const OS_PRIORITY_IDLE: Priority = 0;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThreadState {
    /// Created via `osCreateThread` but not yet `osStartThread`'d. Real
    /// libultra threads are not runnable in this state; fn64 may already have
    /// allocated the host coroutine storage that will carry the guest stack.
    Stopped,
    /// On the run queue, eligible to be resumed (may not be the one
    /// actually running right now -- that's the executor's run queue's
    /// job, see `executor.rs`).
    Runnable,
    /// Currently the one thread holding the `RunToken` and executing.
    Running,
    /// Parked in `osRecvMesg` on an empty queue.
    BlockedOnRecv,
    /// Parked in `osSendMesg` on a full queue.
    BlockedOnSend,
    /// The coroutine's body returned (thread function fell off the end) or
    /// `osDestroyThread` was called.
    Dead,
}

/// What a coroutine's body yields back to the executor at a suspend point.
/// This is the *entire* vocabulary of ways a `GameThread` can give up the
/// CPU -- see `docs/DESIGN.md` section 2's yield-site inventory
/// (`pause_self`, blocking `osRecvMesg`/`osSendMesg`). Because
/// `corosensei::Yielder::suspend` requires a value of this exact type, a
/// coroutine body cannot yield "silently" or with an unrecognized reason:
/// the type system enumerates every legal suspend shape.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Yield {
    /// `pause_self` -- a voluntary cooperative yield with no blocking
    /// condition; the thread is immediately runnable again next scheduling
    /// round (rung 14's idle-loop fix: this is what an unconditional spin
    /// MUST call instead of looping forever without ever yielding).
    ///
    /// Correct only for translated callers that explicitly loop back around
    /// the yield. A generated-C `pause_self` call has no such continuation
    /// and must use [`Yield::StopSelf`].
    PauseSelf,
    /// Park the current thread in `Stopped` until another thread explicitly
    /// starts it. This is the generated-C `pause_self` boundary: returning to
    /// the statement after the call would turn a guest assert/idle park into
    /// impossible fall-through execution.
    StopSelf,
    /// A translated block exhausted its deterministic instruction slice.
    /// The executor charges these guest instructions to virtual time only
    /// after the coroutine has suspended, when its exclusive `&mut Executor`
    /// is available again. Device deadlines are therefore serviced before
    /// this thread (or any other) can execute the next block.
    InstructionCheckpoint { instructions: u32 },
    /// `osRecvMesg` on the named queue. `may_block` is `flag ==
    /// OS_MESG_BLOCK`: when false (`OS_MESG_NOBLOCK`), the executor's
    /// yield handler never parks this coroutine on the blocked list --
    /// only ever resumes it the very next scheduling round, with either
    /// `Resume::Delivered` or `Resume::WouldBlock`. This is deliberately a
    /// suspend point EVEN for the non-blocking case (rather than the ABI
    /// layer pre-checking `Executor` state itself before deciding whether
    /// to yield) -- see `fn64-abi`'s module doc for the reentrancy bug this
    /// closes: a coroutine body must never call back into the executor
    /// directly, since it is already running on the stack the executor's
    /// own `&mut self` borrow is holding open.
    BlockOnRecv {
        mq_addr: crate::RdramAddr,
        may_block: bool,
    },
    /// `osSendMesg` on the named queue, carrying the message being sent.
    /// Same `may_block` semantics as `BlockOnRecv`. `jam` selects the
    /// front-insert path (`osJamMesg`, jammesg.c) instead of the tail-insert
    /// `osSendMesg` -- both share the identical blocked-receiver hand-off and
    /// full-queue blocking behavior, differing only in where a delivered
    /// message lands in the ring.
    BlockOnSend {
        mq_addr: crate::RdramAddr,
        msg: crate::Mesg,
        may_block: bool,
        jam: bool,
    },
}

/// What the executor resumes a coroutine WITH. `Wake` carries the delivered
/// message for a thread that was blocked on `osRecvMesg`, so the coroutine
/// body can return it from the `osRecvMesg_recomp` call site it suspended
/// inside -- see `executor.rs`'s resume logic.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Resume {
    /// First resume after `osStartThread` -- no prior yield to resume from.
    Start,
    /// Resumed after a plain `pause_self` or translated-instruction
    /// checkpoint scheduling round; nothing to hand back.
    Continue,
    /// Resumed because a blocked recv was delivered a message (or a
    /// non-blocking recv attempt succeeded immediately).
    Delivered(crate::Mesg),
    /// Resumed because a blocked send's target queue freed a slot (or a
    /// non-blocking send attempt succeeded immediately); the send itself
    /// already happened (see `executor.rs`), so there is nothing further
    /// for the coroutine to do at this suspend point beyond returning
    /// control to its caller.
    SendUnblocked,
    /// Resumed from a `may_block: false` `BlockOnRecv`/`BlockOnSend` that
    /// could not be delivered immediately -- the libultra `OS_MESG_NOBLOCK`
    /// outcome. The coroutine was never parked on any blocked list.
    WouldBlock,
}

/// The capability required to resume a `GameThread`. Constructible only
/// from `executor.rs` (private-to-crate constructor with no public
/// equivalent), and `resume` takes it by value and does not hand it back --
/// so the only way to obtain a second `RunToken` while resuming a coroutine
/// is for `executor.rs` itself to call `RunToken::issue()` again, which it
/// structurally cannot do until the current `resume` call has returned
/// (Rust's own call-stack discipline: `issue()` and the `resume()` it feeds
/// are sequential statements in the executor's single-threaded run loop,
/// never reentrant, since nothing in this crate spawns a second OS thread).
/// This is the "single-runnable-token type making concurrent game threads a
/// compile error" the task calls for: no function signature in this crate
/// outside `executor.rs` accepts anything that could produce a `RunToken`,
/// so a hypothetical second call site attempting to resume a second
/// `GameThread` while the first is on the stack has no token to pass and
/// fails to compile, not just "would be a bug if someone did it."
pub struct RunToken(());

impl RunToken {
    /// Restricted to `executor.rs`'s single run loop -- see the type's own
    /// doc comment for why this is the entire enforcement mechanism.
    pub(crate) fn issue() -> Self {
        RunToken(())
    }
}

type ThreadCoroutine = Coroutine<Resume, Yield, ()>;

/// An `OSThread`, wrapping a stackful coroutine. See module doc.
pub struct GameThread {
    pub id: ThreadId,
    pub priority: Priority,
    state: ThreadState,
    started: bool,
    coroutine: Option<ThreadCoroutine>,
}

impl GameThread {
    /// `osCreateThread(t, id, entry, arg, stack_top, pri)`. The coroutine
    /// closure is the thread's `entry(arg)` body; `body` here stands in for
    /// that call (an `fn64-abi` shim wraps the real recompiled entry point
    /// per `docs/DESIGN.md` section 1's "dumb adapter" split). The
    /// coroutine is created but not started -- matching real
    /// `osCreateThread`, which does not itself put a thread on any run
    /// queue (`osStartThread` does), reflected here as `ThreadState::
    /// Stopped` and no `resume()` call yet.
    pub fn new(
        id: ThreadId,
        priority: Priority,
        body: impl FnOnce(&Yielder<Resume, Yield>, Resume) + 'static,
    ) -> Self {
        GameThread {
            id,
            priority,
            state: ThreadState::Stopped,
            started: false,
            coroutine: Some(Coroutine::with_stack(
                DefaultStack::new(COROUTINE_STACK_SIZE)
                    .expect("failed to allocate GameThread coroutine stack"),
                move |yielder, first_input| {
                    body(yielder, first_input);
                },
            )),
        }
    }

    pub fn state(&self) -> ThreadState {
        self.state
    }

    pub fn set_state(&mut self, state: ThreadState) {
        self.state = state;
    }

    pub fn is_runnable(&self) -> bool {
        matches!(self.state, ThreadState::Runnable)
    }

    pub fn is_dead(&self) -> bool {
        matches!(self.state, ThreadState::Dead)
    }

    pub fn has_started(&self) -> bool {
        self.started
    }

    /// Resume this thread's coroutine. Requires a `RunToken` -- see that
    /// type's doc comment for the compile-time guarantee this establishes.
    /// The token is consumed for the duration of this call by Rust's
    /// ordinary move semantics (it's passed by value and dropped when this
    /// function returns), so nothing on this call's own stack can present a
    /// second token to a nested `resume` call -- there is no code path in
    /// this crate that would even attempt it, since `executor.rs` is the
    /// only place a token is issued, and it issues exactly one before
    /// calling this function and does not call `issue()` again until this
    /// call has returned.
    pub fn resume(&mut self, _token: RunToken, input: Resume) -> CoroutineResult<Yield, ()> {
        if self.started {
            assert_ne!(
                input,
                Resume::Start,
                "Resume::Start may only be delivered on a GameThread's first resume"
            );
        } else {
            assert_eq!(
                input,
                Resume::Start,
                "a GameThread's first resume must use Resume::Start"
            );
            self.started = true;
        }
        let coroutine = self
            .coroutine
            .as_mut()
            .expect("resume() called on a GameThread with no coroutine (already dead)");
        let result = coroutine.resume(input);
        if matches!(result, CoroutineResult::Return(())) {
            self.state = ThreadState::Dead;
        }
        result
    }

    /// Detach this thread's native stack at the terminal process-exit boundary.
    ///
    /// A started, unfinished guest coroutine can be suspended inside generated
    /// C and an `extern "C"` ABI shim. `corosensei` normally force-unwinds a
    /// suspended coroutine from `Drop`, but that unwind cannot cross the
    /// non-unwind FFI frames and aborts the process. At process exit the OS is
    /// about to reclaim the stack allocation, so intentionally forgetting
    /// exactly that coroutine is the only operation performed here. Never-
    /// started and completed coroutines still take their ordinary `Drop` path.
    ///
    /// This is crate-private because it is not guest `osDestroyThread`
    /// behavior and must only be reached through the executor's terminal host
    /// shutdown operation.
    pub(crate) fn detach_for_process_exit(&mut self) -> bool {
        let Some(coroutine) = self.coroutine.take() else {
            return false;
        };
        let suspended = coroutine.started() && !coroutine.done();
        if suspended {
            std::mem::forget(coroutine);
        }
        self.state = ThreadState::Dead;
        suspended
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn thread_runs_body_and_dies_on_return() {
        let ran = Rc::new(RefCell::new(false));
        let ran2 = ran.clone();
        let mut t = GameThread::new(1, 10, move |_yielder, _input| {
            *ran2.borrow_mut() = true;
        });
        assert_eq!(t.state(), ThreadState::Stopped);

        let result = t.resume(RunToken::issue(), Resume::Start);
        assert!(matches!(result, CoroutineResult::Return(())));
        assert!(*ran.borrow());
        assert!(t.is_dead());
    }

    #[test]
    fn pause_self_yields_and_can_be_resumed_again() {
        let mut t = GameThread::new(1, 10, |yielder, _input| {
            yielder.suspend(Yield::PauseSelf);
            yielder.suspend(Yield::PauseSelf);
        });

        let r1 = t.resume(RunToken::issue(), Resume::Start);
        assert_eq!(r1, CoroutineResult::Yield(Yield::PauseSelf));
        assert!(!t.is_dead());

        let r2 = t.resume(RunToken::issue(), Resume::Continue);
        assert_eq!(r2, CoroutineResult::Yield(Yield::PauseSelf));

        let r3 = t.resume(RunToken::issue(), Resume::Continue);
        assert!(matches!(r3, CoroutineResult::Return(())));
        assert!(t.is_dead());
    }

    #[test]
    fn blocking_recv_yield_carries_queue_address_and_delivers_message() {
        let received = Rc::new(RefCell::new(None));
        let received2 = received.clone();
        let target = crate::RdramAddr::from_offset(0x1234);
        let mut t = GameThread::new(1, 10, move |yielder, _input| {
            let resumed = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: target,
                may_block: true,
            });
            if let Resume::Delivered(msg) = resumed {
                *received2.borrow_mut() = Some(msg);
            }
        });

        let r1 = t.resume(RunToken::issue(), Resume::Start);
        assert_eq!(
            r1,
            CoroutineResult::Yield(Yield::BlockOnRecv {
                mq_addr: target,
                may_block: true
            })
        );

        let r2 = t.resume(RunToken::issue(), Resume::Delivered(99));
        assert!(matches!(r2, CoroutineResult::Return(())));
        assert_eq!(*received.borrow(), Some(99));
    }

    #[test]
    fn process_exit_detaches_only_a_started_suspended_coroutine() {
        struct DropProbe(Rc<Cell<bool>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let never_started_dropped = Rc::new(Cell::new(false));
        let never_started_probe = DropProbe(never_started_dropped.clone());
        let mut never_started = GameThread::new(1, 10, move |_yielder, _input| {
            let _probe = never_started_probe;
        });
        assert!(!never_started.detach_for_process_exit());
        assert!(never_started.is_dead());
        assert!(never_started_dropped.get());

        let completed_dropped = Rc::new(Cell::new(false));
        let completed_probe = DropProbe(completed_dropped.clone());
        let mut completed = GameThread::new(2, 10, move |_yielder, _input| {
            let _probe = completed_probe;
        });
        assert!(matches!(
            completed.resume(RunToken::issue(), Resume::Start),
            CoroutineResult::Return(())
        ));
        assert!(!completed.detach_for_process_exit());
        assert!(completed_dropped.get());

        let suspended_dropped = Rc::new(Cell::new(false));
        let suspended_probe = DropProbe(suspended_dropped.clone());
        let mut suspended = GameThread::new(3, 10, move |yielder, _input| {
            let _probe = suspended_probe;
            yielder.suspend(Yield::PauseSelf);
        });
        assert_eq!(
            suspended.resume(RunToken::issue(), Resume::Start),
            CoroutineResult::Yield(Yield::PauseSelf)
        );
        assert!(suspended.detach_for_process_exit());
        assert!(suspended.is_dead());
        assert!(
            !suspended_dropped.get(),
            "terminal detach must not unwind stack-owned values across foreign frames"
        );
    }
}
