//! Failure-atomic guest-memory and native-context ownership.

/// Owns a native context while a foreign call is allowed to mutate it.
///
/// Taking the value empties the backend slot. Only a validated precommit or
/// commit path can return it; dropping the lease destroys the context so an
/// unclassified post-start failure cannot influence a later submission.
pub(crate) struct NativeContextLease<'a, T> {
    slot: &'a mut Option<T>,
    context: Option<T>,
}

impl<'a, T> NativeContextLease<'a, T> {
    pub(crate) fn take(slot: &'a mut Option<T>) -> Option<Self> {
        slot.take().map(|context| Self {
            slot,
            context: Some(context),
        })
    }

    #[cfg(feature = "rt64")]
    pub(crate) fn context_mut(&mut self) -> &mut T {
        self.context
            .as_mut()
            .expect("native context lease always owns its context")
    }

    pub(crate) fn restore(mut self) {
        debug_assert!(self.slot.is_none());
        *self.slot = self.context.take();
    }
}

/// Rollback guard for a synchronous native call that can write any RDRAM
/// byte. RT64's queues are joined before the FFI borrow returns, so restoring
/// from `Drop` cannot race a retained foreign alias.
pub(crate) struct NativeRdramRollback<'a> {
    live: &'a mut [u8],
    before: &'a mut Vec<u8>,
    armed: bool,
}

impl<'a> NativeRdramRollback<'a> {
    pub(crate) fn new(live: &'a mut [u8], before: &'a mut Vec<u8>) -> Self {
        before.clear();
        before.extend_from_slice(live);
        Self {
            live,
            before,
            armed: true,
        }
    }

    pub(crate) fn memory_mut(&mut self) -> &mut [u8] {
        self.live
    }

    fn unchanged(&self) -> bool {
        self.live == self.before.as_slice()
    }

    pub(crate) fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for NativeRdramRollback<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.live.copy_from_slice(self.before);
        }
    }
}

/// One rollback boundary for the two guest-memory resources a native HLE
/// task can mutate. The sole `commit` disarms both resources together.
pub(crate) struct NativeTaskMemoryRollback<'a> {
    rdram: NativeRdramRollback<'a>,
    rsp_memory: &'a mut fn64_runtime::RspMemory,
    rsp_before: Option<fn64_runtime::RspMemory>,
    armed: bool,
}

impl<'a> NativeTaskMemoryRollback<'a> {
    pub(crate) fn new(
        rdram: &'a mut [u8],
        rsp_memory: &'a mut fn64_runtime::RspMemory,
        rdram_before: &'a mut Vec<u8>,
    ) -> Self {
        let rsp_before = rsp_memory.clone();
        Self {
            rdram: NativeRdramRollback::new(rdram, rdram_before),
            rsp_memory,
            rsp_before: Some(rsp_before),
            armed: true,
        }
    }

    pub(crate) fn memories_mut(&mut self) -> (&mut [u8], &mut fn64_runtime::RspMemory) {
        (self.rdram.memory_mut(), self.rsp_memory)
    }

    pub(crate) fn unchanged(&self) -> bool {
        self.rdram.unchanged() && Some(&*self.rsp_memory) == self.rsp_before.as_ref()
    }

    pub(crate) fn commit(mut self) {
        // Both flags change without a fallible operation between them. A
        // rejected task therefore restores both resources or neither.
        self.armed = false;
        self.rdram.armed = false;
    }
}

impl Drop for NativeTaskMemoryRollback<'_> {
    fn drop(&mut self) {
        if self.armed {
            *self.rsp_memory = self
                .rsp_before
                .take()
                .expect("armed native task rollback retains its RSP preimage");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_memory_rollback_restores_rdram_rsp_and_imem_generation() {
        let mut rdram = vec![0x11; 64];
        let rdram_before = rdram.clone();
        let mut rsp = fn64_runtime::RspMemory::new();
        rsp.write_bytes(fn64_runtime::RspMemAddr::from_register(0), &[0x22; 16])
            .unwrap();
        rsp.write_bytes(fn64_runtime::RspMemAddr::from_register(0x1000), &[0x33; 16])
            .unwrap();
        let rsp_before = rsp.clone();
        let generation_before = rsp.imem_generation();
        let mut rdram_snapshot = Vec::new();

        {
            let mut transaction =
                NativeTaskMemoryRollback::new(&mut rdram, &mut rsp, &mut rdram_snapshot);
            assert!(transaction.unchanged());
            let (native_rdram, native_rsp) = transaction.memories_mut();
            native_rdram[3] = 0xa5;
            native_rdram[61] = 0x5a;
            native_rsp
                .write_bytes(fn64_runtime::RspMemAddr::from_register(4), &[0x44; 8])
                .unwrap();
            native_rsp
                .write_bytes(fn64_runtime::RspMemAddr::from_register(0x1004), &[0x55; 8])
                .unwrap();
            assert!(!transaction.unchanged());
        }

        assert_eq!(rdram, rdram_before);
        assert_eq!(rsp, rsp_before);
        assert_eq!(rsp.imem_generation(), generation_before);
    }

    #[test]
    fn task_memory_commit_publishes_rdram_and_both_rsp_banks_once() {
        let mut rdram = vec![0x11; 64];
        let mut rsp = fn64_runtime::RspMemory::new();
        let generation_before = rsp.imem_generation();
        let mut rdram_snapshot = Vec::new();

        let mut transaction =
            NativeTaskMemoryRollback::new(&mut rdram, &mut rsp, &mut rdram_snapshot);
        let (native_rdram, native_rsp) = transaction.memories_mut();
        native_rdram[7] = 0xa5;
        native_rsp
            .write_bytes(fn64_runtime::RspMemAddr::from_register(8), &[0x44; 8])
            .unwrap();
        native_rsp
            .write_bytes(fn64_runtime::RspMemAddr::from_register(0x1008), &[0x55; 8])
            .unwrap();
        transaction.commit();

        assert_eq!(rdram[7], 0xa5);
        assert_eq!(
            rsp.read_bytes(fn64_runtime::RspMemAddr::from_register(8), 8)
                .unwrap(),
            [0x44; 8]
        );
        assert_eq!(
            rsp.read_bytes(fn64_runtime::RspMemAddr::from_register(0x1008), 8)
                .unwrap(),
            [0x55; 8]
        );
        assert_eq!(rsp.imem_generation(), generation_before + 1);
    }

    #[test]
    fn context_lease_destroys_on_rejection_and_restores_only_explicitly() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct DropProbe(Rc<Cell<u32>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let drops = Rc::new(Cell::new(0));
        let mut rejected = Some(DropProbe(Rc::clone(&drops)));
        drop(NativeContextLease::take(&mut rejected).unwrap());
        assert!(rejected.is_none());
        assert_eq!(drops.get(), 1);

        let mut accepted = Some(DropProbe(Rc::clone(&drops)));
        NativeContextLease::take(&mut accepted).unwrap().restore();
        assert!(accepted.is_some());
        assert_eq!(drops.get(), 1);
        drop(accepted);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn raw_rdp_memory_rollback_and_commit_are_explicit() {
        let mut rejected = vec![0x12; 32];
        let before = rejected.clone();
        let mut rejected_snapshot = Vec::new();
        {
            let mut transaction = NativeRdramRollback::new(&mut rejected, &mut rejected_snapshot);
            transaction.memory_mut()[17] = 0xef;
        }
        assert_eq!(rejected, before);

        let mut accepted = vec![0x12; 32];
        let mut accepted_snapshot = Vec::new();
        let mut transaction = NativeRdramRollback::new(&mut accepted, &mut accepted_snapshot);
        transaction.memory_mut()[17] = 0xef;
        transaction.commit();
        assert_eq!(accepted[17], 0xef);
    }

    #[test]
    fn full_rdram_preimage_allocation_is_reused_between_calls() {
        let mut memory = vec![0x61; 4096];
        let mut snapshot = Vec::new();
        NativeRdramRollback::new(&mut memory, &mut snapshot).commit();
        let allocation = snapshot.as_ptr();
        let capacity = snapshot.capacity();

        memory.fill(0x72);
        NativeRdramRollback::new(&mut memory, &mut snapshot).commit();

        assert_eq!(snapshot.as_ptr(), allocation);
        assert_eq!(snapshot.capacity(), capacity);
        assert_eq!(snapshot, memory);
    }
}
