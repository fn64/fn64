use super::*;
use fn64_render_ir::{
    AccessPurpose, DpInterruptState, OperationId, PhysicalMemoryLayout, RdramResource,
    ResourceJournalLimits,
};

#[test]
fn validated_capture_becomes_one_move_only_decoded_packet() {
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let range = layout.range(0x100, 0x108).unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(1, 8).unwrap(),
        vec![ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range,
            },
        )
        .unwrap()],
    )
    .unwrap();
    let decoded = decode_raw_dpc_capture(
        layout,
        7,
        OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
        TemporalBoundary::new(11, DpInterruptState::Clear),
        Vec::new(),
        journal,
    )
    .unwrap();

    assert_eq!(decoded.packet().streams().len(), 1);
    assert_eq!(decoded.packet().owned_command_bytes(), 8);
    assert_eq!(
        decoded.packet().admission(),
        WorkloadAdmission::RawDpc {
            transaction_sequence: 7
        }
    );
}

#[test]
fn preflight_rejects_missing_command_ownership_before_exposing_a_read_plan() {
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(1, 8).unwrap(),
        vec![ResourceAccess::try_new(
            OperationId::new(7),
            AccessMode::Read,
            AccessPurpose::TmemLoadSource,
            ResourceRegion::Rdram {
                resource: RdramResource::Buffer,
                range: layout.range(0x200, 0x208).unwrap(),
            },
        )
        .unwrap()],
    )
    .unwrap();
    assert_eq!(
        preflight_raw_dpc_capture(
            layout,
            7,
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0, 0]).unwrap(),
            TemporalBoundary::new(11, DpInterruptState::Clear),
            Vec::new(),
            journal,
        )
        .unwrap_err(),
        ValidationError::MissingCommandReadDeclaration {
            stream: fn64_render_ir::RawStreamKind::Dram,
            start: 0x100,
            end: 0x108,
        }
    );
}
