//! ABI-owned bounded capture for renderer-planned deferred guest reads.
//!
//! This module translates and copies physical RDRAM ranges only. It does not
//! decode RDP commands, select resources, or decide renderer policy.

use fn64_render::ir::{
    CapturedGuestRead, DeferredGuestReadCapture, DeferredGuestReadPlan, ValidationError,
};
use fn64_runtime::{RdramAddr, RdramView};

/// Capture only the exact planned ranges from an ABI-storage-order allocation,
/// returning owned N64-logical-order bytes. The allocation length must equal
/// the plan's installed-memory layout; a larger slice is not silently treated
/// as authority for a narrower or aliased layout.
pub fn capture_deferred_guest_reads_from_storage(
    plan: &DeferredGuestReadPlan,
    storage: &[u8],
) -> Result<DeferredGuestReadCapture, ValidationError> {
    if storage.len() != plan.memory_layout().bytes() as usize {
        return Err(ValidationError::MemoryLayoutMismatch {
            expected: plan.memory_layout().bytes(),
        });
    }
    require_native_word_storage_layout(plan)?;
    let view = RdramView::from_storage(storage);
    let mut captured = Vec::with_capacity(plan.reads().len());
    for read in plan.reads() {
        let mut bytes = vec![0; read.range().len() as usize];
        view.copy_logical_bytes(
            RdramAddr::from_offset(read.range().start().get()),
            &mut bytes,
        );
        captured.push(CapturedGuestRead::try_new(*read, bytes)?);
    }
    Ok(DeferredGuestReadCapture::new(captured))
}

/// Capture against the process's registered physical RDRAM device without a
/// whole-device snapshot. `None` means no process allocation is registered.
///
/// This call is valid only at the same executor/host boundary documented by
/// [`crate::with_registered_physical_rdram_read`]. The returned values own
/// their exact bytes and retain no pointer, borrow, or device capability.
pub fn capture_registered_deferred_guest_reads(
    plan: &DeferredGuestReadPlan,
) -> Option<Result<DeferredGuestReadCapture, ValidationError>> {
    crate::with_registered_physical_rdram_read(|physical| {
        if plan.memory_layout().bytes() as usize != physical.len() {
            return Err(ValidationError::MemoryLayoutMismatch {
                expected: plan.memory_layout().bytes(),
            });
        }
        require_native_word_storage_layout(plan)?;
        let mut captured = Vec::with_capacity(plan.reads().len());
        for read in plan.reads() {
            let mut bytes = Vec::with_capacity(read.range().len() as usize);
            for offset in read.range().start().get()..read.range().end() {
                bytes.push(physical.read_u8(RdramAddr::from_offset(offset)));
            }
            captured.push(CapturedGuestRead::try_new(*read, bytes)?);
        }
        Ok(DeferredGuestReadCapture::new(captured))
    })
}

fn require_native_word_storage_layout(plan: &DeferredGuestReadPlan) -> Result<(), ValidationError> {
    const NATIVE_WORD_BYTES: u32 = u32::BITS / u8::BITS;
    let bytes = plan.memory_layout().bytes();
    if !bytes.is_multiple_of(NATIVE_WORD_BYTES) {
        return Err(ValidationError::GuestReadStorageLayoutUnaligned {
            bytes,
            alignment: NATIVE_WORD_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fn64_render::ir::{
        AccessMode, AccessPurpose, DpInterruptState, OperationId, PhysicalMemoryLayout,
        RdramResource, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
        TemporalBoundary,
    };
    use fn64_render::{preflight_raw_dpc_capture, OwnedRawDpcSubmission};
    use fn64_runtime::{RdramAddr, RdramViewMut};

    use super::*;

    fn journal(layout: PhysicalMemoryLayout) -> ResourceJournal {
        ResourceJournal::try_new(
            ResourceJournalLimits::try_new(4, 0x40).unwrap(),
            vec![
                ResourceAccess::try_new(
                    OperationId::new(0),
                    AccessMode::Read,
                    AccessPurpose::CommandDecode,
                    ResourceRegion::Rdram {
                        resource: RdramResource::RawCommands,
                        range: layout.range(0x100, 0x108).unwrap(),
                    },
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(7),
                    AccessMode::Read,
                    AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: layout.range(0x201, 0x208).unwrap(),
                    },
                )
                .unwrap(),
                ResourceAccess::try_new(
                    OperationId::new(9),
                    AccessMode::Read,
                    AccessPurpose::TmemLoadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(0x300, 0x305).unwrap(),
                    },
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn abi_capture_finalizes_owned_packet_without_full_rdram_snapshot() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let preflight = preflight_raw_dpc_capture(
            layout,
            11,
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![],
            journal(layout),
        )
        .unwrap();
        assert_eq!(preflight.guest_read_plan().total_bytes(), 12);

        let mut storage = vec![0; layout.bytes() as usize];
        let first = [0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76];
        let second = [0xa0, 0xb1, 0xc2, 0xd3, 0xe4];
        let mut view = RdramViewMut::from_storage(&mut storage);
        view.write_logical_bytes(RdramAddr::from_offset(0x201), &first);
        view.write_logical_bytes(RdramAddr::from_offset(0x300), &second);

        let capture =
            capture_deferred_guest_reads_from_storage(preflight.guest_read_plan(), &storage)
                .unwrap();
        storage.fill(0xff);
        let decoded = preflight.finalize(capture).unwrap();
        assert_eq!(decoded.packet().owned_guest_read_bytes(), 12);
        assert_eq!(decoded.packet().guest_reads().reads()[0].bytes(), first);
        assert_eq!(decoded.packet().guest_reads().reads()[1].bytes(), second);
        assert_eq!(decoded.packet().owned_command_bytes(), 8);

        let record = fn64_render::ir::WorkloadRecord::from_packet(decoded.packet());
        let encoded = record.encode();
        let decoded_record = fn64_render::ir::WorkloadRecord::decode(&encoded).unwrap();
        assert_eq!(decoded_record.guest_read_content_digests().len(), 2);
        assert_eq!(
            decoded_record
                .replay(decoded.packet().streams().to_vec())
                .unwrap_err(),
            ValidationError::ReplayGuestReadCaptureRequired { count: 2 }
        );

        let mut replay_storage = vec![0; layout.bytes() as usize];
        let mut replay_view = RdramViewMut::from_storage(&mut replay_storage);
        replay_view.write_logical_bytes(RdramAddr::from_offset(0x201), &first);
        replay_view.write_logical_bytes(RdramAddr::from_offset(0x300), &second);
        let replay_plan = fn64_render::ir::DeferredGuestReadPlan::try_from_journal(
            layout,
            decoded_record.journal(),
        )
        .unwrap();
        let replay_capture =
            capture_deferred_guest_reads_from_storage(&replay_plan, &replay_storage).unwrap();
        let replayed = decoded_record
            .replay_with_guest_reads(decoded.packet().streams().to_vec(), replay_capture)
            .unwrap();
        assert_eq!(replayed.identity(), decoded.packet().identity());
    }

    #[test]
    fn abi_capture_rejects_layout_alias_instead_of_truncating() {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let plan =
            fn64_render::ir::DeferredGuestReadPlan::try_from_journal(layout, &journal(layout))
                .unwrap();
        assert_eq!(
            capture_deferred_guest_reads_from_storage(&plan, &[0; 0x2000]).unwrap_err(),
            ValidationError::MemoryLayoutMismatch { expected: 0x1000 }
        );
    }

    #[test]
    fn abi_capture_rejects_non_word_sized_storage_before_lane_translation() {
        let layout = PhysicalMemoryLayout::try_new(1).unwrap();
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(1, 1).unwrap(),
            vec![ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: layout.range(0, 1).unwrap(),
                },
            )
            .unwrap()],
        )
        .unwrap();
        let plan =
            fn64_render::ir::DeferredGuestReadPlan::try_from_journal(layout, &journal).unwrap();
        assert_eq!(
            capture_deferred_guest_reads_from_storage(&plan, &[0]).unwrap_err(),
            ValidationError::GuestReadStorageLayoutUnaligned {
                bytes: 1,
                alignment: 4,
            }
        );
    }
}
