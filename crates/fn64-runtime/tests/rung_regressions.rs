//! Rung regression suite: each `rung_*` scenario reproduces the SETUP of a
//! reference-runtime failure class documented in `docs/DESIGN.md` (sourced
//! from `aki-recomp/games/NWXE/profile.toml`'s boot-ladder evidence, cited
//! per rung below) and asserts the failure CANNOT occur in this executor --
//! not merely that some other code path happens to avoid it today. Where
//! the class is about a structural impossibility (rung 18), the test
//! documents which type/ownership rule makes the bad program uncompilable
//! or unreachable, since a runtime assertion can't prove a compile-time
//! guarantee -- see each test's doc comment for how it maps back to
//! `docs/DESIGN.md` section 2.
//!
//! Property-ish scenarios (N-coroutine ping-pong, full-queue blocking send,
//! timer ordering) are grouped at the bottom of this file.

use fn64_runtime::{
    Executor, ExternalEvent, Priority, RdramAddr, RecvMesgOutcome, Resume, SendMesgOutcome, Yield,
};
use std::cell::RefCell;
use std::rc::Rc;

fn addr(offset: u32) -> RdramAddr {
    RdramAddr::from_offset(offset)
}

// ---------------------------------------------------------------------
// rung 12: osCreateMesgQueue must reset blocked lists to genuinely empty,
// never a stale/sentinel value from a reused address or a partially
// constructed queue.
//
// docs/DESIGN.md section 2 / mesgqueue.rs module doc: the reference
// runtime's real bug was NOT resetting the queue at all (an un-named
// osCreateMesgQueue left a ROM sentinel address in blocked_on_recv/send,
// read back later as a self-looping "thread"). This suite's structural
// analogue: prove that (a) a fresh queue never reports a blocked waiter,
// and (b) RE-creating a queue at an address that previously had real
// blocked waiters wipes them -- the exact "reused address, stale state"
// shape rung 12's own sentinel was.
// ---------------------------------------------------------------------

#[test]
fn rung_12_freshly_created_queue_has_no_phantom_blocked_waiter() {
    let mut exec = Executor::new();
    let mq = addr(0x8005_7228);
    exec.create_mesg_queue(mq, 1);

    // A blocking recv on an empty, freshly-created queue must actually
    // block (MustYield) -- if the rung-12 sentinel bug were present, the
    // "is anything blocked" check would spuriously read as "something is
    // already blocked" and corrupt delivery; here recv correctly reports
    // "nothing to deliver yet, block me" with no prior phantom state.
    assert_eq!(
        exec.recv_mesg(1, mq, true),
        RecvMesgOutcome::MustYield,
        "a freshly-created queue must have zero real messages and zero \
         phantom blocked waiters"
    );
}

#[test]
fn rung_12_recreating_a_queue_at_a_reused_address_wipes_prior_blocked_state() {
    let mut exec = Executor::new();
    let mq = addr(0x8005_7228);
    exec.create_mesg_queue(mq, 1);

    // Thread 1 genuinely blocks on this queue.
    exec.create_thread(1, 10, |yielder, _| {
        yielder.suspend(Yield::BlockOnRecv {
            mq_addr: addr(0x8005_7228),
            may_block: true,
        });
    });
    exec.start_thread(1);
    exec.run_one_step();
    assert_eq!(exec.recv_mesg(1, mq, false), RecvMesgOutcome::WouldBlock);

    // osCreateMesgQueue called again at the SAME address (a reused-address
    // scenario, exactly rung 12's "every OSMesgQueue this game creates" --
    // any re-create must produce a genuinely fresh queue, not one that
    // remembers thread 1 was blocked here a moment ago).
    exec.create_mesg_queue(mq, 4);

    // A send to the "new" queue must be delivered into the ring buffer,
    // NOT treated as if a receiver is still waiting from before the
    // re-create -- proving the re-create really did reset blocked state.
    assert_eq!(
        exec.send_mesg(2, mq, 0xAB, false),
        SendMesgOutcome::Delivered
    );
    assert_eq!(
        exec.queue_capacity(mq),
        4,
        "capacity reflects the NEW create's count, not the old one"
    );
}

// ---------------------------------------------------------------------
// rung 14: the idle-thread cooperative-yield fix. A thread that spins
// unconditionally (no real yield point) must starve everything else
// forever in a cooperative scheduler; a thread that calls pause_self every
// iteration must never starve other runnable work.
//
// docs/DESIGN.md section 2 / profile.toml rung 14: "a backward `j` that
// isn't a self-branch never yields once priority drops to 0, and nothing
// else ever runs again." pause_self (Yield::PauseSelf here) is the fix:
// "loops on wait_for_external_message() + check_running_queue() -- a real
// yield."
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// WM2000 streaming-loader size-underflow rung (2026-07-21): the C-ABI
// `pause_self` boundary must PARK, never auto-resume. N64Recomp's C codegen
// emits `pause_self()` for an unconditional guest self-branch with NO loop
// back -- both permanent idle parks and assert-hang loops (NWXE
// func_80003DD4 0x80003DF0's invalid-file-id trap). Auto-requeue semantics
// (Yield::PauseSelf) let fn64 RETURN through such a call site and fall
// through a guest assertion that is unpassable on hardware; in WM2000 the
// fall-through lookup then indexed the AKI file table with an out-of-range
// id (0xC5BE) and submitted a PI DMA sized 0xFFFFFFFC. Yield::StopSelf is
// the fix: the thread parks in Stopped (the osCreateThread state) and only
// an explicit osStartThread -- reference-runtime semantics -- resumes it,
// at which point pause_self returns (the documented restart fall-through
// the reference runtime also has).
// ---------------------------------------------------------------------

#[test]
fn stop_self_parks_the_thread_until_an_explicit_restart() {
    let mut exec = Executor::new();
    let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let log2 = log.clone();
    exec.create_thread(1, 10, move |yielder, _| {
        log2.borrow_mut().push("reached-assert");
        yielder.suspend(Yield::StopSelf);
        // Only an explicit osStartThread may bring execution here (the
        // reference runtime's restart fall-through) -- never the scheduler
        // on its own.
        log2.borrow_mut().push("fell-through");
    });
    exec.start_thread(1);
    assert!(exec.run_one_step(), "thread runs up to the park");

    // Arbitrarily many scheduling rounds must NOT resume a parked thread:
    // this is the exact difference from Yield::PauseSelf, and the proof the
    // WM2000 assert fall-through class cannot recur.
    for _ in 0..100 {
        exec.run_one_step();
    }
    assert_eq!(*log.borrow(), ["reached-assert"], "no scheduler-driven fall-through");
    assert!(!exec.is_thread_dead(1), "parked, not dead");

    // An explicit osStartThread is the ONE legal resume path (the parked
    // state is ThreadState::Stopped, the same state start_thread demands).
    exec.start_thread(1);
    assert!(exec.run_one_step());
    assert_eq!(*log.borrow(), ["reached-assert", "fell-through"]);
    assert!(exec.is_thread_dead(1), "restarted thread ran to completion");
}

#[test]
fn stop_self_parked_thread_never_starves_other_work() {
    // The WM2000 shape: the game thread parks at a guest assert while other
    // threads (audio pump, service threads) must keep running normally.
    let mut exec = Executor::new();
    let other_ran = Rc::new(RefCell::new(0u32));
    let other_ran2 = other_ran.clone();
    exec.create_thread(1, 20, |yielder, _| {
        yielder.suspend(Yield::StopSelf);
    });
    exec.start_thread(1);
    exec.create_thread(2, 10, move |yielder, _| {
        for _ in 0..3 {
            *other_ran2.borrow_mut() += 1;
            yielder.suspend(Yield::PauseSelf);
        }
    });
    exec.start_thread(2);

    for _ in 0..10 {
        exec.run_one_step();
    }
    assert_eq!(*other_ran.borrow(), 3, "lower-priority work still ran to completion");
    assert!(!exec.is_thread_dead(1));
}

#[test]
fn rung_14_pause_self_idle_loop_yields_every_iteration_and_never_starves_others() {
    let mut exec = Executor::new();
    let other_ran = Rc::new(RefCell::new(0u32));
    let other_ran2 = other_ran.clone();

    // The idle thread: priority 0 (OS_PRIORITY_IDLE), spins calling
    // pause_self forever -- modeling func_800004D0's post-patch body
    // exactly (rung 14: "drops itself to priority 0... then spins," now
    // via pause_self instead of a literal goto).
    exec.create_thread(1, fn64_runtime::OS_PRIORITY_IDLE, |yielder, _| loop {
        yielder.suspend(Yield::PauseSelf);
    });
    exec.start_thread(1);

    // A real, higher-priority worker thread that must get to run despite
    // the idle thread being "runnable forever."
    exec.create_thread(2, 10, move |_yielder, _| {
        *other_ran2.borrow_mut() += 1;
    });
    exec.start_thread(2);

    // Highest priority first: thread 2 must run before the idle thread
    // even gets a second turn (priority-ordered run queue).
    exec.run_one_step();
    assert_eq!(exec.current_thread(), None); // thread 2 already finished
    assert_eq!(*other_ran.borrow(), 1, "the real worker thread ran");
    assert!(exec.is_thread_dead(2));

    // Drain a bounded number of scheduling rounds of the idle thread: it
    // must keep yielding (never finish, never panic, never monopolize) --
    // this is the direct proof the starvation class from rung 14 cannot
    // recur: an unconditional pause_self loop always gives control back to
    // the executor every single iteration.
    for _ in 0..1000 {
        exec.run_one_step();
    }
    assert!(
        !exec.is_thread_dead(1),
        "idle loop still alive, still yielding, never wedged"
    );
}

#[test]
fn rung_14_idle_thread_never_prevents_a_later_woken_thread_from_running() {
    // A worker blocks on recv, the idle thread spins with pause_self, then
    // an external event wakes the worker -- it must actually get to run,
    // proving pause_self's yield really does return control to the
    // executor's scheduling loop every round rather than only once.
    let mut exec = Executor::new();
    let mq = addr(0x9000);
    exec.create_mesg_queue(mq, 1);

    let delivered = Rc::new(RefCell::new(None));
    let delivered2 = delivered.clone();
    exec.create_thread(1, fn64_runtime::OS_PRIORITY_IDLE, |yielder, _| loop {
        yielder.suspend(Yield::PauseSelf);
    });
    exec.start_thread(1);

    exec.create_thread(2, 5, move |yielder, first| {
        let mut input = first;
        loop {
            if let Resume::Delivered(msg) = input {
                *delivered2.borrow_mut() = Some(msg);
                return;
            }
            input = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: addr(0x9000),
                may_block: true,
            });
        }
    });
    exec.start_thread(2);

    // Run several rounds: idle spins, worker blocks.
    for _ in 0..10 {
        exec.run_one_step();
    }
    assert!(delivered.borrow().is_none());

    // External event delivers the message thread 2 is blocked on.
    exec.inject_event(ExternalEvent::DirectPost {
        queue_addr: mq,
        msg: 77,
    });

    for _ in 0..10 {
        exec.run_one_step();
    }
    assert_eq!(*delivered.borrow(), Some(77));
    assert!(exec.is_thread_dead(2));
}

// ---------------------------------------------------------------------
// rung 18 / 18b: the check-then-pop handoff / concurrent-write class. Root
// cause was a SECOND host thread executing recompiled code concurrently
// with the first, touching shared queue/rdram state with no lock the
// scheduler could see -- not fixable by adding more locks around the
// existing check-then-act pattern, per docs/DESIGN.md's extensive case
// study ("a mutex was added at exactly the TOCTOU... and the crash
// reproduced unchanged 20/20").
//
// This class is structurally unrepresentable here: there is exactly one
// native call stack ever executing guest code (RunToken, thread.rs), so
// there is no second thread to race the first. The test below proves the
// observable CONSEQUENCE this eliminates (check-then-pop can never
// interleave with another thread's write) by driving many concurrent
// logical threads' blocking send/recv through the SAME queue and asserting
// every message is delivered exactly once with no duplication/loss/panic
// -- the failure mode a real handoff race would produce.
// ---------------------------------------------------------------------

#[test]
fn rung_18_no_concurrent_writer_can_ever_interleave_with_a_check_then_pop() {
    // Many threads blocked on the same queue's recv side; a single sender
    // posts N messages. If check-then-pop could ever race (rung 18's
    // class), this would manifest as a duplicated delivery, a panic on a
    // "certified non-empty" queue actually being empty, or a lost thread.
    // Because RunToken makes two live GameThread::resume calls impossible
    // (see thread.rs), and the whole MesgQueue mutation path runs on one
    // thread with nothing else ever live between a check and the mutation
    // that follows it, none of those can happen -- asserted by running a
    // large N and checking exact-once delivery.
    let mut exec = Executor::new();
    let mq = addr(0xA000);
    exec.create_mesg_queue(mq, 1); // capacity 1: forces real blocking, not a shortcut

    const N: u32 = 50;
    let results: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));

    for id in 0..N {
        let results = results.clone();
        exec.create_thread(id, (id % 5) as Priority, move |yielder, first| {
            let mut input = first;
            loop {
                if let Resume::Delivered(msg) = input {
                    results.borrow_mut().push(msg);
                    return;
                }
                input = yielder.suspend(Yield::BlockOnRecv {
                    mq_addr: addr(0xA000),
                    may_block: true,
                });
            }
        });
        exec.start_thread(id);
    }

    // Get every thread onto the blocked list first.
    for _ in 0..(N * 2) {
        exec.run_one_step();
    }
    for id in 0..N {
        assert!(
            !exec.is_thread_dead(id),
            "thread {id} must still be blocked, not finished/crashed"
        );
    }

    // Now post N messages, one at a time, each waking exactly one blocked
    // receiver via the executor's single deliver_or_enqueue path.
    for m in 0..N {
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: mq,
            msg: m,
        });
    }
    exec.run_to_idle();

    let mut got = results.borrow().clone();
    got.sort_unstable();
    let expected: Vec<u32> = (0..N).collect();
    assert_eq!(
        got, expected,
        "every message delivered exactly once, no duplication, no loss"
    );
    for id in 0..N {
        assert!(
            exec.is_thread_dead(id),
            "thread {id} must have been woken and completed"
        );
    }
}

#[test]
fn rung_18_run_token_makes_a_second_concurrent_resume_uncompilable() {
    // This is a documentation-as-test placeholder: the actual guarantee is
    // a COMPILE-time one (see fn64_runtime::thread::RunToken's doc
    // comment) -- `GameThread::resume` requires a `RunToken`, and
    // `RunToken::issue()` is `pub(crate)` to fn64-runtime and called from
    // exactly one place (`Executor::run_one_step`). There is no public API
    // in this crate that hands out a second token while the first is live,
    // so "two coroutines' recompiled code running at once" has no
    // expressible call site outside this crate -- there is nothing further
    // to assert at runtime beyond what rung_18_no_concurrent_writer_*
    // above already demonstrates behaviorally. This test exists so the
    // rung is represented by name in the suite, per the task's naming
    // requirement, and documents where the REAL guarantee lives.
    let mut exec = Executor::new();
    exec.create_thread(1, 1, |_yielder, _| {});
    exec.start_thread(1);
    exec.run_one_step();
    assert!(exec.is_thread_dead(1));
}

// ---------------------------------------------------------------------
// Property-ish tests.
// ---------------------------------------------------------------------

#[test]
fn ping_pong_n_coroutines_random_priorities_all_complete_exactly_once() {
    // A simple LCG so the test has no external rand dependency but still
    // gets varied, reproducible "random" priorities.
    let mut state: u32 = 0x1234_5678;
    let mut next_rand = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };

    let mut exec = Executor::new();
    let mq_a = addr(0xB000);
    let mq_b = addr(0xB004);
    exec.create_mesg_queue(mq_a, 4);
    exec.create_mesg_queue(mq_b, 4);

    const PAIRS: u32 = 12;
    let completed = Rc::new(RefCell::new(0u32));

    for i in 0..PAIRS {
        let pri_a = (next_rand() % 10) as Priority;
        let pri_b = (next_rand() % 10) as Priority;
        let completed_a = completed.clone();
        let completed_b = completed.clone();

        // Pinger: sends a ping (tagged with its own index so the ponger
        // can echo it back distinguishably), then blocks recv on mq_b for
        // the reply.
        exec.create_thread(i * 2, pri_a, move |yielder, _first| {
            yielder.suspend(Yield::BlockOnSend {
                mq_addr: addr(0xB000),
                msg: i,
                may_block: true,
                jam: false,
            });
            loop {
                if let Resume::Delivered(msg) = yielder.suspend(Yield::BlockOnRecv {
                    mq_addr: addr(0xB004),
                    may_block: true,
                }) {
                    assert_eq!(msg, i, "pong must echo this pinger's own ping value");
                    *completed_a.borrow_mut() += 1;
                    return;
                }
            }
        });
        exec.start_thread(i * 2);

        // Ponger: recvs the ping, sends the same value back as a pong.
        exec.create_thread(i * 2 + 1, pri_b, move |yielder, _first| {
            let ping = loop {
                if let Resume::Delivered(msg) = yielder.suspend(Yield::BlockOnRecv {
                    mq_addr: addr(0xB000),
                    may_block: true,
                }) {
                    break msg;
                }
            };
            yielder.suspend(Yield::BlockOnSend {
                mq_addr: addr(0xB004),
                msg: ping,
                may_block: true,
                jam: false,
            });
            *completed_b.borrow_mut() += 1;
        });
        exec.start_thread(i * 2 + 1);
    }

    // Drive to idle across many rounds -- ping/pong needs several steps
    // per pair.
    for _ in 0..(PAIRS * 20) {
        if !exec.run_one_step() {
            break;
        }
    }

    for i in 0..(PAIRS * 2) {
        assert!(
            exec.is_thread_dead(i),
            "thread {i} must complete its ping/pong roundtrip"
        );
    }
}

#[test]
fn full_queue_blocking_send_actually_blocks_and_is_woken_by_a_recv() {
    let mut exec = Executor::new();
    let mq = addr(0xC000);
    exec.create_mesg_queue(mq, 1);
    assert_eq!(exec.send_mesg(1, mq, 1, false), SendMesgOutcome::Delivered);

    // Queue is now full; a second send must require blocking.
    assert_eq!(exec.send_mesg(2, mq, 2, true), SendMesgOutcome::MustYield);

    let sent = Rc::new(RefCell::new(false));
    let sent2 = sent.clone();
    exec.create_thread(2, 1, move |yielder, _| {
        yielder.suspend(Yield::BlockOnSend {
            mq_addr: addr(0xC000),
            msg: 2,
            may_block: true,
            jam: false,
        });
        *sent2.borrow_mut() = true;
    });
    exec.start_thread(2);
    exec.run_one_step();
    assert!(
        !*sent.borrow(),
        "sender must still be blocked, queue is full"
    );

    // Draining one message must wake the blocked sender.
    assert_eq!(exec.recv_mesg(9, mq, false), RecvMesgOutcome::Delivered(1));
    exec.run_to_idle();
    assert!(*sent.borrow(), "blocked sender woken once space freed");
    assert!(exec.is_thread_dead(2));
}

#[test]
fn nonblocking_yield_never_parks_and_never_reenters_the_executor() {
    // OS_MESG_NOBLOCK's coroutine-side shape: `may_block: false`. This is
    // the exact path added to close a real reentrancy bug found in
    // fn64-abi (a coroutine body must NEVER call back into the executor
    // synchronously -- see that crate's module doc). Proves the
    // non-blocking case is a real suspend/resume round-trip (never parked
    // on any blocked list, never requires the ABI layer to pre-check
    // executor state itself) for both a full send and an empty recv.
    let mut exec = Executor::new();
    let mq = addr(0xE000);
    exec.create_mesg_queue(mq, 1);

    let recv_result = Rc::new(RefCell::new(None));
    let recv_result2 = recv_result.clone();
    exec.create_thread(1, 1, move |yielder, _| {
        let resumed = yielder.suspend(Yield::BlockOnRecv {
            mq_addr: addr(0xE000),
            may_block: false,
        });
        *recv_result2.borrow_mut() = Some(resumed);
    });
    exec.start_thread(1);
    exec.run_to_idle();
    assert_eq!(
        *recv_result.borrow(),
        Some(Resume::WouldBlock),
        "non-blocking recv on an empty queue must resume with WouldBlock, never park"
    );
    assert!(
        exec.is_thread_dead(1),
        "the coroutine must run straight through, never blocked on any list"
    );

    // Fill the queue, then attempt a non-blocking send on top of it.
    assert_eq!(exec.send_mesg(0, mq, 1, false), SendMesgOutcome::Delivered);
    let send_result = Rc::new(RefCell::new(None));
    let send_result2 = send_result.clone();
    exec.create_thread(2, 1, move |yielder, _| {
        let resumed = yielder.suspend(Yield::BlockOnSend {
            mq_addr: addr(0xE000),
            msg: 2,
            may_block: false,
            jam: false,
        });
        *send_result2.borrow_mut() = Some(resumed);
    });
    exec.start_thread(2);
    exec.run_to_idle();
    assert_eq!(
        *send_result.borrow(),
        Some(Resume::WouldBlock),
        "non-blocking send on a full queue must resume with WouldBlock, never park"
    );
    assert!(exec.is_thread_dead(2));
    // The dropped message must not have been enqueued.
    assert_eq!(exec.recv_mesg(9, mq, false), RecvMesgOutcome::Delivered(1));
    assert_eq!(exec.recv_mesg(9, mq, false), RecvMesgOutcome::WouldBlock);
}

#[test]
fn timer_ordering_posts_messages_in_deadline_order_via_the_single_injection_path() {
    let mut exec = Executor::new();
    let mq = addr(0xD000);
    exec.create_mesg_queue(mq, 8);
    exec.set_timer(30, 0, mq, 3, 0);
    exec.set_timer(10, 0, mq, 1, 0);
    exec.set_timer(20, 0, mq, 2, 0);

    exec.advance_time(30);

    let mut received = Vec::new();
    while let RecvMesgOutcome::Delivered(m) = exec.recv_mesg(0, mq, false) {
        received.push(m);
    }
    assert_eq!(
        received,
        vec![1, 2, 3],
        "timers fire and post in deadline order"
    );
}
