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

/// Priority captured when a thread joins a libultra message-queue wait list.
pub type WaiterPriority = i32;

/// `__osEnqueueThread` inserts before the first strictly lower-priority
/// waiter and after equal-priority waiters. The head is therefore the
/// highest-priority waiter, with FIFO ordering only among ties.
fn priority_insert_index<T>(
    waiters: &[T],
    priority: impl Fn(&T) -> WaiterPriority,
    incoming: WaiterPriority,
) -> usize {
    waiters
        .iter()
        .position(|waiter| priority(waiter) < incoming)
        .unwrap_or(waiters.len())
}

/// The blocked-list a queue's `blocked_on_recv`/`blocked_on_send` fields
/// represent. Deliberately NOT a raw pointer or sentinel value — see the
/// module doc above for why that shape (the real ROM's own struct layout,
/// mirrored by a raw host pointer) was the rung-12 failure mode.
///
/// Waiters are priority-ordered, not arrival-ordered. This is load-bearing
/// for WM2000: its graphics and higher-priority audio runners can both wait
/// on the shared SP-event queue, and the audio yield handshake must receive
/// the next completion before the lower-priority graphics runner.
#[derive(Default)]
struct BlockedList {
    waiters: Vec<BlockedReceiverEvidenceSnapshot>,
}

impl BlockedList {
    fn push(&mut self, id: CoroutineId, priority: WaiterPriority) {
        let index = priority_insert_index(&self.waiters, |waiter| waiter.priority, priority);
        self.waiters
            .insert(index, BlockedReceiverEvidenceSnapshot { id, priority });
    }

    /// Pop the highest-priority waiter; arrival order breaks priority ties.
    fn pop(&mut self) -> Option<CoroutineId> {
        if self.waiters.is_empty() {
            None
        } else {
            Some(self.waiters.remove(0).id)
        }
    }

    fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }

    fn remove(&mut self, id: CoroutineId) -> usize {
        let before = self.waiters.len();
        self.waiters.retain(|waiter| waiter.id != id);
        before - self.waiters.len()
    }
}

/// Which end of the queue a blocked sender must use once a receiver frees a
/// slot. This is part of the suspended operation, not a property the wake site
/// may reconstruct: `osSendMesg` appends at the tail while `osJamMesg` inserts
/// at the head.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SendPlacement {
    Tail,
    Head,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockedReceiverEvidenceSnapshot {
    pub id: CoroutineId,
    pub priority: WaiterPriority,
}

/// One blocked sender in the exact priority/FIFO-tie order in which a freed
/// queue slot will service it. The placement is part of the suspended operation:
/// omitting it would make `osJamMesg` and `osSendMesg` evidence-identical
/// even though their next successful delivery changes the queue differently.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockedSenderEvidenceSnapshot {
    pub id: CoroutineId,
    pub priority: WaiterPriority,
    pub msg: Mesg,
    pub placement: SendPlacement,
}

/// Pointer-free, future-affecting view of one `OSMesgQueue`.
///
/// `messages` is logical receive order rather than the unused contents of the
/// backing ring. `first` remains explicit because guest code observes it in
/// the mirrored `OSMesgQueue` structure even when two rings would deliver the
/// same logical messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MesgQueueEvidenceSnapshot {
    pub capacity: u64,
    pub first: u64,
    pub messages: Vec<Mesg>,
    pub blocked_receivers: Vec<BlockedReceiverEvidenceSnapshot>,
    pub blocked_senders: Vec<BlockedSenderEvidenceSnapshot>,
}

/// Minimal read-only lifecycle state used when a libultra API requires an
/// exclusively owned message queue. Keeping queued messages and both waiter
/// roles in one value prevents callers from mistaking "validCount == 0" for
/// "nobody else can consume the next post."
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MesgQueueActivity {
    pub capacity: usize,
    pub valid_count: usize,
    pub blocked_receivers: usize,
    pub blocked_senders: usize,
}

impl MesgQueueActivity {
    pub const fn is_exclusively_idle(self) -> bool {
        self.capacity > 0
            && self.valid_count == 0
            && self.blocked_receivers == 0
            && self.blocked_senders == 0
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct BlockedSend {
    id: CoroutineId,
    priority: WaiterPriority,
    msg: Mesg,
    placement: SendPlacement,
}

/// Same priority-ordered shape as `BlockedList`, but for blocked SENDERS: a blocked
/// sender is waiting to hand off a specific `Mesg` payload, not just an
/// identity, so the queue must remember what to actually enqueue once a
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
    waiters: Vec<BlockedSend>,
}

impl BlockedSenderList {
    fn push(
        &mut self,
        id: CoroutineId,
        priority: WaiterPriority,
        msg: Mesg,
        placement: SendPlacement,
    ) {
        let index = priority_insert_index(&self.waiters, |waiter| waiter.priority, priority);
        self.waiters.insert(
            index,
            BlockedSend {
                id,
                priority,
                msg,
                placement,
            },
        );
    }

    /// Pop the highest-priority waiter; arrival order breaks priority ties.
    fn pop(&mut self) -> Option<BlockedSend> {
        if self.waiters.is_empty() {
            None
        } else {
            Some(self.waiters.remove(0))
        }
    }

    fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }

    fn remove(&mut self, id: CoroutineId) -> usize {
        let before = self.waiters.len();
        self.waiters.retain(|waiter| waiter.id != id);
        before - self.waiters.len()
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RemovedWaiterRoles {
    pub receivers: usize,
    pub senders: usize,
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

    /// Produce a read-only evidence projection without exposing the backing
    /// allocation or stale ring slots. Empty zero-capacity queues are handled
    /// explicitly so the logical-order projection never takes modulo zero.
    pub fn evidence_snapshot(&self) -> MesgQueueEvidenceSnapshot {
        let messages = if self.buffer.is_empty() {
            Vec::new()
        } else {
            (0..self.valid_count)
                .map(|logical_index| self.buffer[(self.first + logical_index) % self.buffer.len()])
                .collect()
        };
        MesgQueueEvidenceSnapshot {
            capacity: u64::try_from(self.buffer.len())
                .expect("OSMesgQueue capacity does not fit evidence width"),
            first: u64::try_from(self.first)
                .expect("OSMesgQueue first index does not fit evidence width"),
            messages,
            blocked_receivers: self.blocked_on_recv.waiters.clone(),
            blocked_senders: self
                .blocked_on_send
                .waiters
                .iter()
                .map(|blocked| BlockedSenderEvidenceSnapshot {
                    id: blocked.id,
                    priority: blocked.priority,
                    msg: blocked.msg,
                    placement: blocked.placement,
                })
                .collect(),
        }
    }

    pub fn activity(&self) -> MesgQueueActivity {
        MesgQueueActivity {
            capacity: self.capacity(),
            valid_count: self.valid_count,
            blocked_receivers: self.blocked_on_recv.waiters.len(),
            blocked_senders: self.blocked_on_send.waiters.len(),
        }
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
    /// list.
    pub fn block_sender(
        &mut self,
        id: CoroutineId,
        priority: WaiterPriority,
        msg: Mesg,
        placement: SendPlacement,
    ) {
        self.blocked_on_send.push(id, priority, msg, placement);
    }

    pub fn block_receiver(&mut self, id: CoroutineId, priority: WaiterPriority) {
        self.blocked_on_recv.push(id, priority);
    }

    /// Remove `id` from every waiter role owned by this queue. Thread
    /// lifecycle code calls this for every registered queue before marking a
    /// thread stopped or dead, so a later queue mutation cannot rediscover and
    /// revive an otherwise-unscheduled coroutine.
    pub fn remove_waiter(&mut self, id: CoroutineId) -> RemovedWaiterRoles {
        RemovedWaiterRoles {
            receivers: self.blocked_on_recv.remove(id),
            senders: self.blocked_on_send.remove(id),
        }
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
        let blocked = self.blocked_on_send.pop()?;
        // Interleaving closed here: a blocking osJamMesg suspends while the
        // queue is full, another thread receives one item, then this wake path
        // commits the suspended message. Retaining `placement` in the waiter
        // prevents that handoff from silently changing a head insert into the
        // ordinary tail-send operation.
        let delivered = match blocked.placement {
            SendPlacement::Tail => self.try_send(blocked.msg),
            SendPlacement::Head => self.try_jam(blocked.msg),
        };
        assert_eq!(
            delivered,
            SendResult::Delivered,
            "wake_one_sender: freed slot rejected the blocked sender's message -- queue \
             bookkeeping is inconsistent"
        );
        Some(blocked.id)
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
        q.block_sender(42, 10, 2, SendPlacement::Tail);
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

    #[test]
    fn blocked_waiters_wake_by_priority_with_fifo_ties() {
        let mut queue = MesgQueue::new(1);
        queue.block_receiver(17, 0x64);
        queue.block_receiver(18, 0x6e);
        queue.block_receiver(19, 0x6e);
        assert_eq!(queue.wake_one_receiver(), Some(18));
        assert_eq!(queue.wake_one_receiver(), Some(19));
        assert_eq!(queue.wake_one_receiver(), Some(17));

        assert_eq!(queue.try_send(0xaa), SendResult::Delivered);
        queue.block_sender(40, 5, 0x1111, SendPlacement::Tail);
        queue.block_sender(41, 20, 0x2222, SendPlacement::Head);
        assert_eq!(queue.try_recv(), RecvResult::Delivered(0xaa));
        assert_eq!(queue.wake_one_sender(), Some(41));
        assert_eq!(queue.try_recv(), RecvResult::Delivered(0x2222));
        assert_eq!(queue.wake_one_sender(), Some(40));
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
        q.block_sender(7, 10, 0xBBBB, SendPlacement::Tail);

        // First recv drains the original message and wakes the blocked
        // sender, which must land ITS message (0xBBBB) into the queue.
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xAAAA));
        assert_eq!(q.wake_one_sender(), Some(7));

        // A second recv must now see the previously-blocked sender's real
        // message, not 0 (the old, silently-dropped-message bug) or any
        // other stale value.
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xBBBB));
    }

    /// Exact interleaving regression: a blocking jam observes a full queue,
    /// suspends, and another thread receives one item before the sender wakes.
    /// The delayed commit must still insert at the head; forgetting the typed
    /// placement makes the pre-existing tail message arrive first.
    #[test]
    fn blocked_jam_preserves_head_placement_after_receiver_frees_space() {
        let mut q = MesgQueue::new(2);
        assert_eq!(q.try_send(0xAAAA), SendResult::Delivered);
        assert_eq!(q.try_send(0xBBBB), SendResult::Delivered);
        assert_eq!(q.try_jam(0xCCCC), SendResult::WouldBlock);
        q.block_sender(7, 10, 0xCCCC, SendPlacement::Head);

        assert_eq!(q.try_recv(), RecvResult::Delivered(0xAAAA));
        assert_eq!(q.wake_one_sender(), Some(7));
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xCCCC));
        assert_eq!(q.try_recv(), RecvResult::Delivered(0xBBBB));
    }

    #[test]
    fn remove_waiter_clears_every_role_and_every_duplicate() {
        let mut q = MesgQueue::new(1);
        q.block_receiver(9, 10);
        q.block_receiver(9, 10);
        q.block_sender(9, 10, 1, SendPlacement::Tail);
        q.block_sender(9, 10, 2, SendPlacement::Head);

        assert_eq!(
            q.remove_waiter(9),
            RemovedWaiterRoles {
                receivers: 2,
                senders: 2,
            }
        );
        assert!(!q.has_blocked_receivers());
        assert!(!q.has_blocked_senders());
        assert_eq!(q.wake_one_receiver(), None);
        assert_eq!(q.wake_one_sender(), None);
    }

    #[test]
    fn evidence_snapshot_uses_logical_ring_and_exact_waiter_order() {
        let mut q = MesgQueue::new(3);
        assert_eq!(q.try_send(0x11), SendResult::Delivered);
        assert_eq!(q.try_send(0x22), SendResult::Delivered);
        assert_eq!(q.try_recv(), RecvResult::Delivered(0x11));
        assert_eq!(q.try_send(0x33), SendResult::Delivered);
        q.block_receiver(7, 20);
        q.block_receiver(8, 10);
        q.block_sender(9, 20, 0x44, SendPlacement::Head);
        q.block_sender(10, 10, 0x55, SendPlacement::Tail);

        assert_eq!(
            q.evidence_snapshot(),
            MesgQueueEvidenceSnapshot {
                capacity: 3,
                first: 1,
                messages: vec![0x22, 0x33],
                blocked_receivers: vec![
                    BlockedReceiverEvidenceSnapshot {
                        id: 7,
                        priority: 20
                    },
                    BlockedReceiverEvidenceSnapshot {
                        id: 8,
                        priority: 10
                    },
                ],
                blocked_senders: vec![
                    BlockedSenderEvidenceSnapshot {
                        id: 9,
                        priority: 20,
                        msg: 0x44,
                        placement: SendPlacement::Head,
                    },
                    BlockedSenderEvidenceSnapshot {
                        id: 10,
                        priority: 10,
                        msg: 0x55,
                        placement: SendPlacement::Tail,
                    },
                ],
            }
        );
    }
}
