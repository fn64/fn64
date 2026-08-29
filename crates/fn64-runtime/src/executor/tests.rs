    use super::*;

    fn read_i32(buf: &[u8], off: usize) -> i32 {
        i32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    #[test]
    fn peek_next_priority_distinguishes_work_from_the_idle_thread() {
        let mut exec = Executor::new();
        exec.create_thread(1, crate::thread::OS_PRIORITY_IDLE, |_yielder, _resume| {});
        exec.create_thread(2, 10, |_yielder, _resume| {});
        exec.start_thread(1);
        exec.start_thread(2);

        assert_eq!(exec.peek_next_priority(), Some(10));
        assert!(exec.run_one_step());
        assert_eq!(
            exec.peek_next_priority(),
            Some(crate::thread::OS_PRIORITY_IDLE)
        );
    }

    /// Regression: the guest's rdram `OSMesgQueue` struct
    /// (`validCount`@0x08, `first`@0x0C, `msgCount`@0x10) MUST be kept in
    /// sync with the executor's authoritative `MesgQueue` after creation and
    /// every mutation. Guest code reads those fields DIRECTLY via
    /// `MQ_GET_COUNT`/`MQ_IS_FULL` (e.g. `IrqMgr_SendMesgToClients`'s
    /// `MQ_IS_FULL(client->queue)` gate). Before this fix, the struct stayed
    /// zero-initialized, so `MQ_IS_FULL` = `0 >= 0` = ALWAYS TRUE, silently
    /// dropping every VI-retrace forward to the OoT scheduler and freezing
    /// boot at exactly 1 framebuffer swap.
    ///
    /// Distinguishable values chosen so a regression can't pass by accident:
    /// capacity 5 (not 0/1), and a partial fill of 3 (not 0, not full).
    #[test]
    fn queue_struct_mirrored_into_rdram_on_create_and_send() {
        // A queue at a byte offset with room for the 0x18-byte struct.
        const Q_OFF: u32 = 0x1000;
        const CAPACITY: usize = 5;
        let mut rdram = vec![0u8; 0x2000];

        let mut exec = Executor::new();
        unsafe { exec.set_rdram_base(rdram.as_mut_ptr()) };

        let q = RdramAddr::from_offset(Q_OFF);
        exec.create_mesg_queue(q, CAPACITY);

        // After creation: validCount==0, first==0, msgCount==capacity.
        let base = Q_OFF as usize;
        assert_eq!(read_i32(&rdram, base + 0x08), 0, "validCount after create");
        assert_eq!(read_i32(&rdram, base + 0x0C), 0, "first after create");
        assert_eq!(
            read_i32(&rdram, base + 0x10),
            CAPACITY as i32,
            "msgCount after create MUST equal capacity, else MQ_IS_FULL reads garbage"
        );

        // Post three messages via the external/timer path (no blocked
        // receiver), then confirm validCount tracked each enqueue -- 3, a
        // value distinct from 0 (empty) and 5 (full).
        for msg in [0x11u32, 0x22, 0x33] {
            exec.inject_event(ExternalEvent::DirectPost { queue_addr: q, msg });
        }
        assert_eq!(
            read_i32(&rdram, base + 0x08),
            3,
            "validCount MUST mirror the 3 enqueued messages (so MQ_GET_COUNT/MQ_IS_FULL are correct)"
        );
        assert_eq!(
            read_i32(&rdram, base + 0x10),
            CAPACITY as i32,
            "msgCount stays capacity"
        );
        // MQ_IS_FULL semantics the guest computes: validCount >= msgCount.
        assert!(
            read_i32(&rdram, base + 0x08) < read_i32(&rdram, base + 0x10),
            "a partially-filled queue MUST NOT read as full (the bug: it always did)"
        );
    }

    /// Regression: a queue the guest NEVER passed to `osCreateMesgQueue` (a
    /// bzero'd `OSMesgQueue` struct used directly) must behave as a real
    /// zero-capacity queue: NOBLOCK send finds it full (dropped), NOBLOCK recv
    /// finds it empty (would-block) -- BOTH returning the -1 the guest relies
    /// on. OoT's audio driver depends on exactly this for
    /// `gAudioCtx.asyncLoadUnkMediumQueue`, which the decomp never creates and
    /// only ever NOBLOCK-sends/recvs (`audio/internal/load.c:1652,1717-1718`).
    ///
    /// Fail-against-bug: before the `queue_mut` lazy-zero-capacity fix, the
    /// FIRST touch of such a queue PANICKED ("used before osCreateMesgQueue"),
    /// aborting the whole boot at ~VI swap 2 the moment the (newly un-stubbed)
    /// audio load path ran. This test would have panicked instead of asserting.
    #[test]
    fn untracked_queue_behaves_as_zero_capacity_not_a_panic() {
        let mut exec = Executor::new();
        // A queue address that was NEVER created via osCreateMesgQueue.
        let q = RdramAddr::from_offset(0x4321);

        // NOBLOCK send: a zero-capacity queue is always full -> dropped, not a
        // panic, not a fake "delivered".
        assert_eq!(
            exec.send_mesg(0, q, 0xDEAD, /* blocking */ false),
            SendMesgOutcome::DroppedWouldBlock,
            "NOBLOCK send to an untracked (bzero'd) queue must report full/dropped (guest -1)"
        );

        // NOBLOCK recv: a zero-capacity queue is always empty -> would-block.
        assert_eq!(
            exec.recv_mesg(0, q, /* blocking */ false),
            RecvMesgOutcome::WouldBlock,
            "NOBLOCK recv from an untracked (bzero'd) queue must report empty (guest -1)"
        );

        // The lazy install must be genuinely zero-capacity, so it can never
        // silently accept a message a real bzero'd queue would have rejected.
        assert_eq!(
            exec.queue_capacity(q),
            0,
            "untracked queue must be zero-capacity"
        );
    }

    /// Without a registered rdram base (unit-test executors that never boot a
    /// real rdram), the mirror is a safe no-op -- never a null deref.
    #[test]
    fn mirror_is_noop_without_rdram_base() {
        let mut exec = Executor::new();
        let q = RdramAddr::from_offset(0x2000);
        exec.create_mesg_queue(q, 4); // must not panic / deref null
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: q,
            msg: 1,
        });
        assert_eq!(exec.queue_capacity(q), 4);
    }

    #[test]
    fn cp0_count_runs_at_half_cpu_rate_and_compare_latches_ip7() {
        let mut exec = Executor::new();
        exec.set_cp0_count(0xFFFF_FFFE);
        exec.write_cp0_compare(0);

        exec.advance_time(1);
        assert_eq!(exec.cp0_count(), 0xFFFF_FFFE);
        assert_eq!(exec.cp0_count_phase(), 1);
        assert!(!exec.cp0_timer_pending());
        exec.advance_time(2);
        assert_eq!(exec.cp0_count(), 0xFFFF_FFFF);
        assert_eq!(exec.cp0_count_phase(), 0);
        assert!(!exec.cp0_timer_pending());
        exec.advance_time(4);
        assert_eq!(exec.cp0_count(), 0);
        assert_eq!(exec.cp0_count_phase(), 0);
        assert!(exec.cp0_timer_pending());

        exec.write_cp0_compare(0x1234_5678);
        assert_eq!(exec.cp0_compare(), 0x1234_5678);
        assert!(!exec.cp0_timer_pending());
    }

    #[test]
    fn boot_clock_restore_retains_captured_compare_latch() {
        let mut exec = Executor::new();
        exec.restore_cp0_clock(0x1234_5678, 0x9abc_def0, true);
        assert_eq!(exec.cp0_count(), 0x1234_5678);
        assert_eq!(exec.cp0_compare(), 0x9abc_def0);
        assert!(exec.cp0_timer_pending());
    }

    #[test]
    fn split_cp0_count_advances_preserve_the_odd_cycle_phase() {
        let mut split = Executor::new();
        split.advance_time(1);
        split.advance_time(3);
        split.advance_time(7);

        let mut combined = Executor::new();
        combined.advance_time(7);
        assert_eq!(split.cp0_count(), combined.cp0_count());
        assert_eq!(split.cp0_count(), 3);
    }

    #[test]
    fn executor_delivers_start_once_then_continue_after_pause() {
        let inputs = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed = inputs.clone();
        let mut exec = Executor::new();
        exec.create_thread(1, 1, move |yielder, first| {
            observed.borrow_mut().push(first);
            let resumed = yielder.suspend(Yield::PauseSelf);
            observed.borrow_mut().push(resumed);
        });
        exec.start_thread(1);

        assert!(exec.run_one_step());
        assert_eq!(&*inputs.borrow(), &[Resume::Start]);
        assert!(exec.run_one_step());
        assert_eq!(&*inputs.borrow(), &[Resume::Start, Resume::Continue]);
        assert!(exec.is_thread_dead(1));
    }

    /// Exact interleaving regression: A blocks receiving, B destroys A, then
    /// a host event posts to the queue. The post must enqueue normally; it
    /// must not pop A's stale waiter id and revive the destroyed coroutine.
    #[test]
    fn destroyed_blocked_receiver_cannot_be_revived_by_a_later_post() {
        let mut exec = Executor::new();
        let queue = RdramAddr::from_offset(0x3000);
        exec.create_mesg_queue(queue, 1);
        exec.create_thread(1, 1, move |yielder, _| {
            let _ = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: queue,
                may_block: true,
            });
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());

        exec.destroy_thread(1);
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0xABCD,
        });

        assert!(exec.is_thread_dead(1));
        assert_eq!(exec.peek_next_thread(), None);
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::Delivered(0xABCD)
        );
    }

    /// Exact interleaving regression: A blocks jamming into a full queue, B
    /// stops A, then B receives and frees a slot. The receive must not replay
    /// A's removed blocked operation or make A runnable again.
    #[test]
    fn stopped_blocked_sender_cannot_be_revived_when_space_frees() {
        let mut exec = Executor::new();
        let queue = RdramAddr::from_offset(0x4000);
        exec.create_mesg_queue(queue, 1);
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0x1111,
        });
        exec.create_thread(1, 1, move |yielder, _| {
            let _ = yielder.suspend(Yield::BlockOnSend {
                mq_addr: queue,
                msg: 0x2222,
                may_block: true,
                jam: true,
            });
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());

        exec.stop_thread(1);
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::Delivered(0x1111)
        );
        assert_eq!(
            exec.recv_mesg(99, queue, false),
            RecvMesgOutcome::WouldBlock
        );
        assert_eq!(exec.peek_next_thread(), None);
        assert!(!exec.is_thread_dead(1));
    }

    #[test]
    fn control_evidence_canonicalizes_hash_owned_insertion_order() {
        fn build(reversed: bool) -> Executor {
            let mut exec = Executor::new();
            let thread_ids = if reversed { [9, 3] } else { [3, 9] };
            for id in thread_ids {
                exec.create_thread(id, id as Priority, |_yielder, _resume| {});
            }

            let queue_offsets = if reversed {
                [0x2200, 0x1100]
            } else {
                [0x1100, 0x2200]
            };
            for offset in queue_offsets {
                let queue = RdramAddr::from_offset(offset);
                exec.create_mesg_queue(queue, 3);
                exec.inject_event(ExternalEvent::DirectPost {
                    queue_addr: queue,
                    msg: offset,
                });
            }

            let events = if reversed {
                [(8, 0x2200, 0x88), (2, 0x1100, 0x22)]
            } else {
                [(2, 0x1100, 0x22), (8, 0x2200, 0x88)]
            };
            for (event, queue, msg) in events {
                exec.set_event_mesg(event, RdramAddr::from_offset(queue), msg);
            }
            exec
        }

        let snapshot = build(false).control_evidence_snapshot();
        assert_eq!(snapshot, build(true).control_evidence_snapshot());
        assert_eq!(
            snapshot
                .threads
                .iter()
                .map(|thread| thread.id)
                .collect::<Vec<_>>(),
            vec![3, 9]
        );
        assert_eq!(
            snapshot
                .queues
                .iter()
                .map(|queue| queue.address.offset())
                .collect::<Vec<_>>(),
            vec![0x1100, 0x2200]
        );
        assert_eq!(
            snapshot
                .event_table
                .iter()
                .map(|registration| registration.event)
                .collect::<Vec<_>>(),
            vec![2, 8]
        );
    }

    #[test]
    fn control_evidence_preserves_exact_equal_priority_run_order() {
        fn build(order: [ThreadId; 2]) -> ExecutorControlEvidenceSnapshot {
            let mut exec = Executor::new();
            exec.create_thread(1, 10, |_yielder, _resume| {});
            exec.create_thread(2, 10, |_yielder, _resume| {});
            exec.start_thread(order[0]);
            exec.start_thread(order[1]);
            exec.control_evidence_snapshot()
        }

        let first = build([1, 2]);
        let reversed = build([2, 1]);
        assert_eq!(first.threads, reversed.threads);
        assert_eq!(first.pending_resumes, reversed.pending_resumes);
        assert_eq!(
            first
                .pending_resumes
                .iter()
                .map(|pending| pending.thread)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_ne!(first.run_queue, reversed.run_queue);
    }

    #[test]
    fn control_evidence_is_pointer_independent_and_nonmutating() {
        let mut first_rdram = vec![0u8; 0x4000];
        let mut second_rdram = vec![0u8; 0x4000];
        assert_ne!(first_rdram.as_mut_ptr(), second_rdram.as_mut_ptr());

        let mut first = Executor::new();
        let mut second = Executor::new();
        unsafe {
            first.set_rdram_base_with_len(first_rdram.as_mut_ptr(), first_rdram.len());
            second.set_rdram_base_with_len(second_rdram.as_mut_ptr(), second_rdram.len());
        }
        first.create_thread(4, 7, |_yielder, _resume| {});
        second.create_thread(4, 7, |_yielder, _resume| {});

        let snapshot = first.control_evidence_snapshot();
        assert_eq!(snapshot, second.control_evidence_snapshot());
        assert_eq!(first.control_evidence_snapshot(), snapshot);
        assert_eq!(first.peek_next_thread(), None);
    }

    #[test]
    fn control_evidence_detects_each_owner_family_perturbation() {
        let baseline = Executor::new().control_evidence_snapshot();

        let mut rdram_bytes = vec![0u8; 32];
        let mut rdram = Executor::new();
        unsafe { rdram.set_rdram_base_with_len(rdram_bytes.as_mut_ptr(), rdram_bytes.len()) };
        assert_ne!(baseline.rdram, rdram.control_evidence_snapshot().rdram);

        let mut thread = Executor::new();
        thread.create_thread(1, 3, |_yielder, _resume| {});
        assert_ne!(baseline.threads, thread.control_evidence_snapshot().threads);

        let mut queue = Executor::new();
        queue.create_mesg_queue(RdramAddr::from_offset(0x100), 2);
        assert_ne!(baseline.queues, queue.control_evidence_snapshot().queues);

        let mut timer = Executor::new();
        timer.set_timer(7, 0, RdramAddr::from_offset(0x100), 5, 1);
        assert_ne!(baseline.timers, timer.control_evidence_snapshot().timers);

        let mut event = Executor::new();
        event.set_event_mesg(7, RdramAddr::from_offset(0x100), 5);
        assert_ne!(
            baseline.event_table,
            event.control_evidence_snapshot().event_table
        );

        let mut clock = Executor::new();
        clock.write_cp0_compare(1);
        clock.advance_time(3);
        let clock = clock.control_evidence_snapshot();
        assert_ne!(baseline.sim_time, clock.sim_time);
        assert_ne!(baseline.cp0_count, clock.cp0_count);
        assert_ne!(baseline.cp0_count_phase, clock.cp0_count_phase);
        assert_ne!(baseline.cp0_compare, clock.cp0_compare);
        assert_ne!(baseline.cp0_timer_pending, clock.cp0_timer_pending);

        let mut active = Executor::new();
        active.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        active.start_thread(1);
        assert!(active.run_one_step());
        active.run_queue.clear();
        active
            .threads
            .get_mut(&1)
            .expect("thread exists")
            .set_state(ThreadState::Running);
        active.running = Some(1);
        assert_eq!(
            active.control_evidence_snapshot().running,
            ExecutorRunningEvidenceSnapshot::Active(1)
        );

        let mut pending = Executor::new();
        pending.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        pending.start_thread(1);
        assert!(pending.run_one_step());
        let without_pending = pending.control_evidence_snapshot();
        pending.pending_resume.insert(1, Resume::WouldBlock);
        let with_pending = pending.control_evidence_snapshot();
        assert_eq!(without_pending.threads, with_pending.threads);
        assert_eq!(without_pending.run_queue, with_pending.run_queue);
        assert_ne!(
            without_pending.pending_resumes,
            with_pending.pending_resumes
        );
    }

    #[test]
    fn control_evidence_rejects_corrupt_cross_owner_relationships() {
    let mut pending_time = Executor::new();
    pending_time.pending_time_target = Some(crate::EmulatedInstant::new(17));
    assert_eq!(
        pending_time.validate_control_evidence_invariants(),
        Err(ExecutorControlInvariantError::UncommittedTimeTarget(
            crate::EmulatedInstant::new(17)
        ))
    );

        let mut duplicate = Executor::new();
        duplicate.create_thread(1, 1, |_yielder, _resume| {});
        duplicate.start_thread(1);
        duplicate.run_queue.push(1);
        assert_eq!(
            duplicate.validate_control_evidence_invariants(),
            Err(ExecutorControlInvariantError::DuplicateRunQueueThread(1))
        );

        let mut wrong_waiter_state = Executor::new();
        let queue = RdramAddr::from_offset(0x100);
        wrong_waiter_state.create_mesg_queue(queue, 1);
        wrong_waiter_state.create_thread(2, 1, |_yielder, _resume| {});
        wrong_waiter_state.queue_mut(queue).block_receiver(2, 1);
        assert_eq!(
            wrong_waiter_state.validate_control_evidence_invariants(),
            Err(ExecutorControlInvariantError::ReceiverWaiterStateMismatch(
                2,
                ThreadState::Stopped
            ))
        );

        let mut stale_resume = Executor::new();
        stale_resume.create_thread(3, 1, |_yielder, _resume| {});
        stale_resume.pending_resume.insert(3, Resume::Continue);
        assert_eq!(
            stale_resume.validate_control_evidence_invariants(),
            Err(
                ExecutorControlInvariantError::PendingResumeThreadNotRunnable(
                    3,
                    ThreadState::Stopped
                )
            )
        );
    }

    #[test]
    fn stopping_a_woken_thread_discards_its_stale_pending_resume() {
        let observed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed_by_thread = observed.clone();
        let queue = RdramAddr::from_offset(0x100);
        let mut exec = Executor::new();
        exec.create_mesg_queue(queue, 1);
        exec.create_thread(1, 1, move |yielder, first| {
            observed_by_thread.borrow_mut().push(first);
            let resumed = yielder.suspend(Yield::BlockOnRecv {
                mq_addr: queue,
                may_block: true,
            });
            observed_by_thread.borrow_mut().push(resumed);
        });
        exec.start_thread(1);
        assert!(exec.run_one_step());
        exec.inject_event(ExternalEvent::DirectPost {
            queue_addr: queue,
            msg: 0xCAFE,
        });

        exec.stop_thread(1);
        exec.start_thread(1);
        assert!(exec.run_one_step());
        assert_eq!(&*observed.borrow(), &[Resume::Start, Resume::Continue]);
    }

    #[test]
    fn opaque_native_continuations_are_intentionally_evidence_equal() {
        let mut yields_again = Executor::new();
        yields_again.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
            yielder.suspend(Yield::PauseSelf);
        });
        yields_again.start_thread(1);
        assert!(yields_again.run_one_step());

        let mut returns_next = Executor::new();
        returns_next.create_thread(1, 1, |yielder, _resume| {
            yielder.suspend(Yield::PauseSelf);
        });
        returns_next.start_thread(1);
        assert!(returns_next.run_one_step());

        assert_eq!(
            yields_again.control_evidence_snapshot(),
            returns_next.control_evidence_snapshot(),
            "native continuation differences are outside this evidence projection"
        );

        assert!(yields_again.run_one_step());
        assert!(returns_next.run_one_step());
        assert_ne!(
            yields_again.control_evidence_snapshot(),
            returns_next.control_evidence_snapshot(),
            "the next scheduling step exposes the intentionally opaque difference"
        );
    }

    #[test]
    fn process_exit_rejects_an_active_run_token_owner() {
        let mut exec = Executor::new();
        exec.running = Some(77);
        let panic =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exec.prepare_process_exit()));
        assert!(panic.is_err());
        exec.running = None;
    }
