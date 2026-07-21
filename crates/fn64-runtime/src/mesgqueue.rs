//! `OSMesgQueue` semantics. See `docs/DESIGN.md` section 2's "OSMesgQueue
//! semantics, designed from the libultra manual + rung evidence" writeup.
//!
//! Design constraint this module exists to satisfy (cited in full in
//! `docs/DESIGN.md`): `aki-recomp/games/NWXE/profile.toml` rung 12 found
//! that a queue whose `blocked_on_recv`/`blocked_on_send` fields could hold
//! anything other than "genuinely empty" on construction corrupted the
//! reference runtime's scheduler by treating a stale sentinel as a real
//! blocked thread. This module's `MesgQueue::new` is the only constructor,
//! and there is no raw-field-write path from outside this module at all —
//! the empty-blocked-list invariant is enforced by having no other way to
//! produce a `MesgQueue`, not by convention at each call site.
//!
//! A `MesgQueue`'s blocked lists are owned exclusively by whatever executor
//! integration drives coroutine resume order (see `docs/DESIGN.md` section
//! 2's recommended stackful-coroutine model) — this crate models the list
//! itself and the send/recv state machine; it does not itself schedule
//! coroutines.

/// A opaque identifier for whatever unit of execution can be blocked on a
/// queue (an `OSThread`'s coroutine, in the recommended design). Kept
/// abstract here so this module has zero dependency on the executor.
/// Aliased to `trace::ThreadId` (same underlying type) so a `TraceEvent`'s
/// `QueueOp { thread, .. }` and a `BlockedList` entry are provably the same
/// identifier space, not two `u32`s that happen to match by convention.
pub type CoroutineId = crate::trace::ThreadId;

/// One in-flight message. libultra's `OSMesg` is a `void*`-sized opaque
/// payload; modeled here as the same width without attaching game-specific
/// meaning.
pub type Mesg = u32;

/// A blocked waiter's thread priority, captured at the moment it parks.
/// Same type as the executor's `thread::Priority` (`i32`), kept as a plain
/// alias so this module stays executor-independent.
pub type WaiterPriority = i32;

/// The blocked-list a queue's `blocked_on_recv`/`blocked_on_send` fields
/// represent. Deliberately NOT a raw pointer or sentinel value — see the
/// module doc above for why that shape (the real ROM's own struct layout,
/// mirrored by a raw host pointer) was the rung-12 failure mode.
///
/// PRIORITY-ordered, not arrival-ordered (2026-07-21 correction): libultra
/// parks a blocking `osRecvMesg`/`osSendMesg` caller with
/// `__osEnqueueThread(&mq->mtqueue, thread)`, which inserts sorted by
/// thread priority (descending, AFTER existing equal-priority threads),
/// and `osSendMesg`/`osRecvMesg` wake with `__osPopThread` = "take the
/// head" = the highest-priority waiter. Arrival order only breaks ties.
/// WM2000 (NWXE) boot deadlocks without this: its gfx runner (guest thread
/// 17, pri 0x64) and audio runner (thread 18, pri 0x6E) both block on ONE
/// shared OS_EVENT_SP queue (rdram 0x52320), and the audio runner's
/// yield-handshake (funcs_0.c `func_80001024`: osSpTaskYield, then recv the
/// yield-done, kick audio, then recv the audio-done) is only correct
/// because every SP-done that arrives while BOTH are parked wakes the
/// HIGHER-priority audio runner first. Arrival-FIFO handed the audio-done
/// to the longer-parked gfx runner instead — the grant crossover that froze
/// boot at 3 gfx frames.
#[derive(Default)]
struct BlockedList {
    waiters: Vec<(CoroutineId, WaiterPriority)>,
}

/// `__osEnqueueThread`'s insertion rule: before the first strictly-lower-
/// priority entry, after all equal-or-higher ones (stable FIFO among
/// equals). Shared by both blocked lists.
fn priority_insert_index<T>(
    waiters: &[T],
    pri_of: impl Fn(&T) -> WaiterPriority,
    pri: WaiterPriority,
) -> usize {
    waiters
        .iter()
        .position(|w| pri_of(w) < pri)
        .unwrap_or(waiters.len())
}

impl BlockedList {
    fn push(&mut self, id: CoroutineId, pri: WaiterPriority) {
        let idx = priority_insert_index(&self.waiters, |w| w.1, pri);
        self.waiters.insert(idx, (id, pri));
    }

    /// Pop the highest-priority waiter (list head; see the type doc for why
    /// this is priority order, with arrival order only among equals).
    fn pop(&mut self) -> Option<CoroutineId> {
        if self.waiters.is_empty() {
            None
        } else {
            Some(self.waiters.remove(0).0)
        }
    }

    fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

/// Same priority-ordered shape as `BlockedList`, but for blocked SENDERS: a
/// blocked sender is waiting to hand off a specific `Mesg` payload, not just
/// an identity, so the queue must remember what to actually enqueue once a
/// slot frees. Fixes a real bug this session's OoT boot run hit: the
/// previous `BlockedList`-based sender queue woke a blocked sender with
/// `Resume::SendUnblocked` (see `executor.rs`'s `try_deliver_recv`) with NO
/// corresponding write of that sender's message into the queue's buffer --
/// silently dropping the message, and leaving `valid_count`/`first`
/// inconsistent with what both threads believed had happened. A later
/// `osRecvMesg` on the same queue could then hand a caller garbage/stale
/// buffer contents believing a real send had occurred -- observed as an
/// eventual wild jump (PC into unmapped memory) deep inside unrelated
/// recompiled code that trusted the delivered `Mesg` as a valid pointer/
/// index.
#[derive(Default)]
struct BlockedSenderList {
    waiters: Vec<(CoroutineId, WaiterPriority, Mesg)>,
}

impl BlockedSenderList {
    fn push(&mut self, id: CoroutineId, pri: WaiterPriority, msg: Mesg) {
        let idx = priority_insert_index(&self.waiters, |w| w.1, pri);
        self.waiters.insert(idx, (id, pri, msg));
    }

    /// Pop the highest-priority blocked sender (see `BlockedList::pop`).
    fn pop(&mut self) -> Option<(CoroutineId, Mesg)> {
        if self.waiters.is_empty() {
            None
        } else {
            let (id, _pri, msg) = self.waiters.remove(0);
            Some((id, msg))
        }
    }

    fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

/// Outcome of `MesgQueue::try_send`. `WouldBlock` carries no coroutine
/// registration itself -- per `docs/DESIGN.md` section 2, the caller (the
/// `fn64-abi` shim, running as the current coroutine) is responsible for
/// calling `MesgQueue::block_sender` and then yielding to the executor;
/// keeping that two-step explicit is what makes "register on the blocked
/// list" and "actually stop running" two ordinary sequential statements on
/// a single executor thread, not a place a second thread could observe an
/// inconsistent in-between state.
#[derive(Debug, PartialEq, Eq)]
pub enum SendResult {
    Delivered,
    WouldBlock,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RecvResult {
    Delivered(Mesg),
    WouldBlock,
}

/// An `OSMesgQueue`. See the module doc for the invariant this type
/// exists to make structurally unbreakable.
pub struct MesgQueue {
    buffer: Box<[Mesg]>,
    valid_count: usize,
    first: usize,
    blocked_on_recv: BlockedList,
    blocked_on_send: BlockedSenderList,
}

impl MesgQueue {
    /// The only constructor. Always produces an empty queue with empty
    /// blocked lists -- this is `osCreateMesgQueue`'s real, load-bearing
    /// reset (rung 12), not an implementation detail.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "osCreateMesgQueue requires count > 0");
        MesgQueue {
            buffer: vec![0; capacity].into_boxed_slice(),
            valid_count: 0,
            first: 0,
            blocked_on_recv: BlockedList::default(),
            blocked_on_send: BlockedSenderList::default(),
        }
    }

    /// A zero-capacity queue, modeling a guest `OSMesgQueue` struct that was
    /// only ever `bzero`'d and never passed to `osCreateMesgQueue` (so its
    /// `msgCount`/message buffer are 0/NULL). OoT's audio driver has exactly
    /// one such queue: `gAudioCtx.asyncLoadUnkMediumQueue`, which the decomp
    /// never creates -- it is bzero'd as part of `gAudioCtx` and then only
    /// ever NOBLOCK-sent/recv'd (`audio/internal/load.c:1652,1717,1718`),
    /// relying on both operations returning -1 (full-on-send, empty-on-recv).
    /// A zero-capacity queue reproduces that exactly: `is_full()` and
    /// `is_empty_queue()` are both always true, so `try_send`/`try_recv`
    /// return `WouldBlock` without ever reaching their `% buffer.len()` (which
    /// would panic on a 0-length buffer). See `Executor::queue_mut`, which
    /// lazily installs this on first use of an untracked queue.
    pub fn zero_capacity() -> Self {
        MesgQueue {
            buffer: Vec::new().into_boxed_slice(),
            valid_count: 0,
            first: 0,
            blocked_on_recv: BlockedList::default(),
            blocked_on_send: BlockedSenderList::default(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Current number of queued messages -- mirrored into the guest's rdram
    /// `OSMesgQueue.validCount` (0x08) so guest `MQ_GET_COUNT`/`MQ_IS_FULL`
    /// reads see truth. See `Executor::mirror_queue_to_rdram`.
    pub fn valid_count(&self) -> usize {
        self.valid_count
    }

    /// Ring-buffer head index -- mirrored into `OSMesgQueue.first` (0x0C).
    pub fn first_index(&self) -> usize {
        self.first
    }

    fn is_full(&self) -> bool {
        self.valid_count == self.buffer.len()
    }

    fn is_empty_queue(&self) -> bool {
        self.valid_count == 0
    }

    /// Non-blocking send attempt (`osSendMesg` with `OS_MESG_NOBLOCK`, or
    /// the first half of a blocking `osSendMesg` before the shim decides
    /// whether to yield). Returns `WouldBlock` if the queue is full; the
    /// caller then calls `block_sender` and yields.
    pub fn try_send(&mut self, msg: Mesg) -> SendResult {
        if self.is_full() {
            return SendResult::WouldBlock;
        }
        let idx = (self.first + self.valid_count) % self.buffer.len();
        self.buffer[idx] = msg;
        self.valid_count += 1;
        SendResult::Delivered
    }

    /// Non-blocking front-insert (`osJamMesg`). Inserts at the HEAD of the
    /// ring (next to be received) rather than the tail, matching
    /// libultra `jammesg.c:16-17`: `first = (first + msgCount - 1) % msgCount;
    /// msg[first] = msg`. Returns `WouldBlock` if the queue is full; the
    /// caller then blocks like a blocked sender. Mirrors `try_send`'s shape.
    pub fn try_jam(&mut self, msg: Mesg) -> SendResult {
        if self.is_full() {
            return SendResult::WouldBlock;
        }
        // Decrement-first-mod-capacity: step `first` back one slot (with
        // wraparound) and write there, so this message is the next popped.
        let cap = self.buffer.len();
        self.first = (self.first + cap - 1) % cap;
        self.buffer[self.first] = msg;
        self.valid_count += 1;
        SendResult::Delivered
    }

    /// Non-blocking recv attempt (mirrors `try_send`).
    pub fn try_recv(&mut self) -> RecvResult {
        if self.is_empty_queue() {
            return RecvResult::WouldBlock;
        }
        let msg = self.buffer[self.first];
        self.first = (self.first + 1) % self.buffer.len();
        self.valid_count -= 1;
        RecvResult::Delivered(msg)
    }

    /// Register the current coroutine as blocked waiting to send (queue
    /// was full), remembering the `Mesg` payload it was trying to send --
    /// `try_send` never got to run for this message (the queue was full),
    /// so it must be replayed by `wake_one_sender` once space frees; see
    /// that method's doc comment. Called by the `fn64-abi` shim after
    /// `try_send` returns `WouldBlock`, immediately before yielding to the
    /// executor -- see module doc: registration and yield are sequential
    /// steps on the one executor thread, never two threads racing this
    /// list. `pri` is the blocking thread's priority AT PARK TIME
    /// (`__osEnqueueThread` sorts by it; see `BlockedList`'s doc).
    pub fn block_sender(&mut self, id: CoroutineId, pri: WaiterPriority, msg: Mesg) {
        self.blocked_on_send.push(id, pri, msg);
    }

    /// See `block_sender` for the `pri` contract.
    pub fn block_receiver(&mut self, id: CoroutineId, pri: WaiterPriority) {
        self.blocked_on_recv.push(id, pri);
    }

    /// Called by the executor after a `try_recv` succeeds (freeing a slot),
    /// to find the next blocked sender (if any) and actually deliver its
    /// message into the now-free slot -- this is the real hand-off a
    /// blocked `osSendMesg` was waiting on, not just a wakeup notification.
    /// Returns the woken coroutine's id so the executor can resume it with
    /// `Resume::SendUnblocked`. Panics if the just-freed slot somehow can't
    /// accept the message (`try_send` failing here would mean this
    /// queue's `valid_count`/blocked-list bookkeeping is already
    /// inconsistent -- a real bug, not a legitimate runtime state, since
    /// the caller only invokes this after confirming
    /// `has_blocked_senders()` following a successful `try_recv`).
    pub fn wake_one_sender(&mut self) -> Option<CoroutineId> {
        let (id, msg) = self.blocked_on_send.pop()?;
        assert_eq!(
            self.try_send(msg),
            SendResult::Delivered,
            "wake_one_sender: freed slot rejected the blocked sender's message -- queue \
             bookkeeping is inconsistent"
        );
        Some(id)
    }

    pub fn wake_one_receiver(&mut self) -> Option<CoroutineId> {
        self.blocked_on_recv.pop()
    }

    pub fn has_blocked_senders(&self) -> bool {
        !self.blocked_on_send.is_empty()
    }

    pub fn has_blocked_receivers(&self) -> bool {
        !self.blocked_on_recv.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshly_created_queue_has_no_blocked_waiters() {
        // The rung-12 invariant: construction alone must guarantee this,
        // with no separate reset step a caller could skip.
        let q = MesgQueue::new(4);
        assert!(!q.has_blocked_senders());
        assert!(!q.has_blocked_receivers());
        assert_eq!(q.valid_count, 0);
    }

    #[test]
    fn send_recv_fifo_roundtrip() {
        let mut q = MesgQueue::new(2);
        assert_eq!(q.try_send(10), SendResult::Delivered);
        assert_eq!(q.try_send(20), SendResult::Delivered);
        assert_eq!(q.try_send(30), SendResult::WouldBlock); // full
        assert_eq!(q.try_recv(), RecvResult::Delivered(10));
        assert_eq!(q.try_recv(), RecvResult::Delivered(20));
        assert_eq!(q.try_recv(), RecvResult::WouldBlock); // empty
    }

    #[test]
    fn blocked_sender_woken_after_space_frees() {
        let mut q = MesgQueue::new(1);
        assert_eq!(q.try_send(1), SendResult::Delivered);
        assert_eq!(q.try_send(2), SendResult::WouldBlock);
        q.block_sender(42, 10, 2);
        assert!(q.has_blocked_senders());

        // Executor's job: a recv frees a slot, then wake a blocked sender.
        assert_eq!(q.try_recv(), RecvResult::Delivered(1));
        let woken = q.wake_one_sender();
        assert_eq!(woken, Some(42));
        assert!(!q.has_blocked_senders());
    }

    /// `osJamMesg` front-insert: a jammed message must be received BEFORE
    /// messages already queued (FIFO order is subverted at the head), and
    /// wrap correctly when `first` is at 0. Fails against a back-insert bug.
    #[test]
    fn jam_inserts_at_front_and_wraps() {
        let mut q = MesgQueue::new(3);
        assert_eq!(q.try_send(0x1111), SendResult::Delivered);
        assert_eq!(q.try_send(0x2222), SendResult::Delivered);
        // Jam a high-priority message: it must jump the queue.
        assert_eq!(q.try_jam(0x9999), SendResult::Delivered);
        // Head-of-line: the jammed message comes out first, before 0x1111.
        assert_eq!(q.try_recv(), RecvResult::Delivered(0x9999));
        assert_eq!(q.try_recv(), RecvResult::Delivered(0x1111));
        assert_eq!(q.try_recv(), RecvResult::Delivered(0x2222));
        // Full queue rejects a jam, like try_send.
        let mut full = MesgQueue::new(1);
        assert_eq!(full.try_send(1), SendResult::Delivered);
        assert_eq!(full.try_jam(2), SendResult::WouldBlock);
    }

    /// The WM2000 grant-crossover regression, at the list level: the wake
    /// order of blocked receivers is THREAD PRIORITY (highest first), with
    /// arrival order only breaking ties -- `__osEnqueueThread`/`__osPopThread`
    /// semantics, NOT arrival-FIFO. The deadlock shape: the gfx runner
    /// (pri 0x64) parks first and the audio runner (pri 0x6E) parks second
    /// on the same OS_EVENT_SP queue; the SP-done must wake the audio
    /// runner. Arrival-FIFO woke the gfx runner and froze boot at 3 frames.
    #[test]
    fn blocked_receivers_wake_in_priority_order_not_arrival_order() {
        let mut q = MesgQueue::new(8);
        q.block_receiver(17, 0x64); // gfx runner parks FIRST, lower pri
        q.block_receiver(18, 0x6E); // audio runner parks second, higher pri
        assert_eq!(
            q.wake_one_receiver(),
            Some(18),
            "the higher-priority waiter must be woken first even though it \
             parked later (libultra __osPopThread on a priority-sorted queue)"
        );
        assert_eq!(q.wake_one_receiver(), Some(17));
        assert_eq!(q.wake_one_receiver(), None);
    }

    /// Equal priorities keep arrival order (`__osEnqueueThread` inserts
    /// AFTER existing equal-priority threads), and blocked senders follow
    /// the same priority rule as receivers.
    #[test]
    fn equal_priority_waiters_keep_fifo_and_senders_sort_by_priority_too() {
        let mut q = MesgQueue::new(8);
        q.block_receiver(1, 10);
        q.block_receiver(2, 10);
        q.block_receiver(3, 10);
        assert_eq!(q.wake_one_receiver(), Some(1));
        assert_eq!(q.wake_one_receiver(), Some(2));
        assert_eq!(q.wake_one_receiver(), Some(3));

        let mut full = MesgQueue::new(1);
        assert_eq!(full.try_send(0xAA), SendResult::Delivered);
        full.block_sender(40, 5, 0x1111); // parks first, low pri
        full.block_sender(41, 20, 0x2222); // parks second, high pri
        assert_eq!(full.try_recv(), RecvResult::Delivered(0xAA));
        assert_eq!(
            full.wake_one_sender(),
            Some(41),
            "higher-priority blocked sender must be woken first"
        );
        // Its message really landed; the lower-pri sender is still parked.
        assert_eq!(full.try_recv(), RecvResult::Delivered(0x2222));
        assert_eq!(full.wake_one_sender(), Some(40));
        assert_eq!(full.try_recv(), RecvResult::Delivered(0x1111));
    }

    /// The bug this session's OoT boot run actually hit: a blocked
    /// sender's message must be REALLY delivered into the queue, not just
    /// have its coroutine woken -- a later recv must see it, not stale/
    /// leftover buffer contents.
    #[test]
    fn blocked_sender_message_is_actually_delivered() {
        let mut q = MesgQueue::new(1);
        assert_eq!(q.try_send(0xAAAA), SendResult::Delivered);
        assert_eq!(q.try_send(0xBBBB), SendResult::WouldBlock);
        q.block_sender(7, 10, 0xBBBB);

        // First recv drains the original message and wakes the blocked
        // sender, which must land ITS message (0xBBBB) into the queue.
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xAAAA));
        assert_eq!(q.wake_one_sender(), Some(7));

        // A second recv must now see the previously-blocked sender's real
        // message, not 0 (the old, silently-dropped-message bug) or any
        // other stale value.
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xBBBB));
    }
}
