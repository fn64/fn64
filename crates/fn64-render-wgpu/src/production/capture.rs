use super::*;

/// Execution-view sink that drives the complete TMEM staging pipeline.
/// `RawDpcExecutionView`'s three callbacks fire in a fixed order --
/// `plan_visited`, then `captured_reads`, then `submitted_packet` -- and
/// none of `CapturedGuestRead`, `WorkloadPacket`, or the lent plan itself
/// is `Clone` or outlives the call. Rather than trying to retain borrowed
/// data past `execution_view`'s return (which the sealed API does not
/// allow), this collector accumulates the plan and captured reads in the
/// first two callbacks, then performs the entire stage/finish/effect-report
/// pipeline inside `submitted_packet` -- the one callback where
/// `&WorkloadPacket` (which `BackendEffectReport::try_new` requires) is
/// still in scope. `outcome` carries the result out; `execute_raw_dpc_inner`
/// takes it after `execution_view` returns.
pub(super) struct CapturedGuestReadBytes(Arc<[u8]>);

impl CapturedGuestReadBytes {
    pub(super) fn copied(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[cfg(test)]
    pub(super) fn shares_allocation_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One packet's journal binding to immutable captured bytes. This remains a
/// distinct value even when the task pool shares its payload allocation with
/// another packet's binding.
pub(super) struct CapturedGuestReadBinding {
    pub(super) read: DeferredGuestRead,
    pub(super) bytes: CapturedGuestReadBytes,
}

pub(super) struct IndexedCapturedGuestRead {
    pub(super) access: ResourceAccess,
    pub(super) bytes: CapturedGuestReadBytes,
}

/// Packet-sized, access-indexed authority over finalized captured reads.
///
/// `pending` exists only between `captured_reads` and `submitted_packet` in
/// the sealed execution view. Binding consumes it once, validates every
/// descriptor against the packet's exact journal access, and leaves direct
/// indexing as the only production lookup path.
#[derive(Default)]
pub(super) struct CapturedGuestReadAuthority {
    pub(super) pending: Vec<CapturedGuestReadBinding>,
    pub(super) by_access: Vec<Option<IndexedCapturedGuestRead>>,
}

impl CapturedGuestReadAuthority {
    pub(super) fn clear_and_reserve(&mut self, len: usize) {
        self.pending.clear();
        self.pending.reserve(len);
        self.by_access.clear();
    }

    pub(super) fn push(&mut self, read: DeferredGuestRead, bytes: CapturedGuestReadBytes) {
        self.pending.push(CapturedGuestReadBinding { read, bytes });
    }

    pub(super) fn bind_packet(
        &mut self,
        packet: &WorkloadPacket,
    ) -> Result<(), WgpuRawDpcExecutionError> {
        self.bind_accesses(packet.journal().accesses())
    }

    pub(super) fn bind_accesses(
        &mut self,
        accesses: &[ResourceAccess],
    ) -> Result<(), WgpuRawDpcExecutionError> {
        self.by_access.clear();
        self.by_access.resize_with(accesses.len(), || None);

        for binding in self.pending.drain(..) {
            let access_index = binding.read.access_index();
            let index = usize::try_from(access_index).map_err(|_| {
                WgpuRawDpcExecutionError::CapturedSourceAccessOutOfRange { access_index }
            })?;
            let expected = accesses
                .get(index)
                .copied()
                .ok_or(WgpuRawDpcExecutionError::CapturedSourceAccessOutOfRange { access_index })?;
            let descriptor_matches = binding.read.operation() == expected.operation()
                && expected.mode() == AccessMode::Read
                && expected.purpose() == AccessPurpose::TmemLoadSource
                && matches!(
                    expected.region(),
                    ResourceRegion::Rdram { resource, range }
                        if resource == binding.read.resource() && range == binding.read.range()
                );
            if !descriptor_matches {
                return Err(WgpuRawDpcExecutionError::CapturedSourceAccessMismatch {
                    access_index,
                });
            }
            let slot = &mut self.by_access[index];
            if slot.is_some() {
                return Err(WgpuRawDpcExecutionError::DuplicateCapturedSource { access_index });
            }
            *slot = Some(IndexedCapturedGuestRead {
                access: expected,
                bytes: binding.bytes,
            });
        }

        for (index, access) in accesses.iter().enumerate() {
            if access.purpose() == AccessPurpose::TmemLoadSource && self.by_access[index].is_none()
            {
                return Err(WgpuRawDpcExecutionError::MissingCapturedSourceAccess {
                    access_index: u32::try_from(index)
                        .expect("packet resource-access count exceeds u32"),
                });
            }
        }
        Ok(())
    }

    pub(super) fn bytes(&self, access_index: u32, expected: ResourceAccess) -> Option<&[u8]> {
        let indexed = self
            .by_access
            .get(usize::try_from(access_index).ok()?)?
            .as_ref()?;
        (indexed.access == expected).then(|| indexed.bytes.as_slice())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TaskGuestReadPayloadKey {
    pub(super) range: PhysicalRange,
    pub(super) content: FastContentDigest,
}

/// Task-local ownership of immutable guest-read payloads.
///
/// Each packet keeps its own access-index binding in `ExecutionCollector`;
/// only byte storage for an identical physical range and content identity is
/// shared. The byte comparison is load-bearing: the fast digest selects a
/// small candidate bucket but can never authorize reuse by itself.
#[derive(Default)]
pub(super) struct TaskGuestReadCapturePool {
    pub(super) payloads: HashMap<TaskGuestReadPayloadKey, Vec<Arc<[u8]>>>,
}

impl TaskGuestReadCapturePool {
    pub(super) fn intern(&mut self, captured: &CapturedGuestRead) -> CapturedGuestReadBytes {
        self.intern_parts(
            captured.read().range(),
            captured.fast_content(),
            captured.bytes(),
        )
    }

    pub(super) fn intern_parts(
        &mut self,
        range: PhysicalRange,
        content: FastContentDigest,
        bytes: &[u8],
    ) -> CapturedGuestReadBytes {
        let candidates = self
            .payloads
            .entry(TaskGuestReadPayloadKey { range, content })
            .or_default();
        if let Some(existing) = candidates
            .iter()
            .find(|existing| existing.as_ref() == bytes)
        {
            return CapturedGuestReadBytes(Arc::clone(existing));
        }
        let owned: Arc<[u8]> = Arc::from(bytes);
        candidates.push(Arc::clone(&owned));
        CapturedGuestReadBytes(owned)
    }
}
