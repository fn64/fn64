use super::*;

fn write_logical_bytes(storage: &mut [u8], start: u32, bytes: &[u8]) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(storage);
    for (offset, byte) in bytes.iter().copied().enumerate() {
        view.write_u8(
            fn64_runtime::RdramAddr::from_offset(start + u32::try_from(offset).unwrap()),
            byte,
        );
    }
}

fn dma_write_eight(
    machine: &mut fn64_audio::rsp::runtime::RspMachine<'_>,
    dram: u32,
    bytes: [u8; 8],
) {
    let mut dmem = machine.dmem_logical();
    dmem[0x40..0x48].copy_from_slice(&bytes);
    machine.load_dmem_logical(&dmem);
    machine.set_dma_dram(dram);
    machine.set_dma_mem(0x40);
    machine.dma_write(7);
}

fn command_moment_plan(
    layout: fn64_render::ir::PhysicalMemoryLayout,
    range: fn64_render::ir::PhysicalRange,
    command_ends: &[u32],
) -> fn64_render::ir::DeferredGuestReadPlan {
    use fn64_render::ir::{
        AccessMode, AccessPurpose, CommandCompletionMoment, GuestReadCommandMoment, OperationId,
        RdramResource, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    };
    let accesses = command_ends
        .iter()
        .enumerate()
        .map(|(index, _)| {
            ResourceAccess::try_new(
                OperationId::new(u32::try_from(index).unwrap()),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range,
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(
            accesses.len(),
            range.len() * u32::try_from(accesses.len()).unwrap(),
        )
        .unwrap(),
        accesses.clone(),
    )
    .unwrap();
    let moments = accesses
        .iter()
        .zip(command_ends)
        .enumerate()
        .map(|(access_index, (access, command_end))| {
            GuestReadCommandMoment::new(
                u32::try_from(access_index).unwrap(),
                access.operation(),
                CommandCompletionMoment::new(0, *command_end),
            )
        })
        .collect::<Vec<_>>();
    fn64_render::ir::DeferredGuestReadPlan::try_from_journal_with_command_moments(
        layout, &journal, &moments,
    )
    .unwrap()
}

#[test]
fn repeated_physical_range_is_copied_once_and_bound_to_each_descriptor() {
    use fn64_render::ir::{
        AccessMode, AccessPurpose, OperationId, PhysicalMemoryLayout, RdramResource,
        ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    };

    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let range = layout.range(0x100, 0x108).unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(2, 16).unwrap(),
        vec![
            ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range,
                },
            )
            .unwrap(),
            ResourceAccess::try_new(
                OperationId::new(2),
                AccessMode::Read,
                AccessPurpose::TmemLoadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range,
                },
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let plan = fn64_render::ir::DeferredGuestReadPlan::try_from_journal(layout, &journal).unwrap();
    let mut storage = vec![0u8; 0x1000];
    let history = {
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut storage);
        machine.take_deferred_dpc_history()
    };
    let mut arena = TaskGuestReadCaptureArena::new(&storage, &history);
    let capture = arena.capture(&plan, &[]);

    assert_eq!(arena.payloads.len(), 1);
    assert_eq!(capture.reads().len(), 2);
    assert_ne!(capture.reads()[0].read(), capture.reads()[1].read());
    assert!(std::ptr::eq(
        capture.reads()[0].bytes().as_ptr(),
        capture.reads()[1].bytes().as_ptr(),
    ));
}

#[test]
fn two_command_end_moments_capture_a_then_b_from_one_physical_range() {
    const COMMAND_START: u32 = 0x100;
    const DATA: u32 = 0x280;
    const A: [u8; 8] = *b"epoch-A!";
    const B: [u8; 8] = *b"epoch-B!";
    const C: [u8; 8] = *b"epoch-C!";
    let mut storage = vec![0u8; 0x1000];
    write_logical_bytes(&mut storage, DATA, &A);
    let mut history = {
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut storage);
        machine.write_cp0(8, COMMAND_START);
        machine.write_cp0(9, COMMAND_START + 8);
        dma_write_eight(&mut machine, DATA, B);
        machine.write_cp0(9, COMMAND_START + 16);
        dma_write_eight(&mut machine, DATA, C);
        machine.take_deferred_dpc_history()
    };
    let runs = coalesce_dp_submissions(history.take_submissions());
    assert_eq!(runs.len(), 1);
    let boundaries = &runs[0].read_epoch_boundaries;
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(storage.len() as u32).unwrap();
    let range = layout.range(DATA, DATA + 8).unwrap();
    let plan = command_moment_plan(layout, range, &[8, 16]);
    let mut arena = TaskGuestReadCaptureArena::new(&storage, &history);
    let capture = arena.capture(&plan, boundaries);
    assert_eq!(capture.reads()[0].bytes(), A);
    assert_eq!(capture.reads()[1].bytes(), B);
    assert_eq!(arena.payloads.len(), 2);
}

#[test]
fn sixteen_byte_command_straddling_two_ends_uses_the_later_epoch() {
    const COMMAND_START: u32 = 0x100;
    const DATA: u32 = 0x280;
    let mut storage = vec![0u8; 0x1000];
    write_logical_bytes(&mut storage, DATA, b"before!!");
    let mut history = {
        let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut storage);
        machine.write_cp0(8, COMMAND_START);
        machine.write_cp0(9, COMMAND_START + 8);
        dma_write_eight(&mut machine, DATA, *b"during!!");
        machine.write_cp0(9, COMMAND_START + 16);
        dma_write_eight(&mut machine, DATA, *b"after!!!");
        machine.take_deferred_dpc_history()
    };
    let runs = coalesce_dp_submissions(history.take_submissions());
    let boundaries = &runs[0].read_epoch_boundaries;
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(storage.len() as u32).unwrap();
    let range = layout.range(DATA, DATA + 8).unwrap();
    let plan = command_moment_plan(layout, range, &[16]);
    assert_eq!(
        resolve_guest_read_epoch(plan.reads()[0], boundaries, history.current_epoch()),
        boundaries[1].read_epoch
    );
    let mut arena = TaskGuestReadCaptureArena::new(&storage, &history);
    let capture = arena.capture(&plan, boundaries);
    assert_eq!(capture.reads()[0].bytes(), b"during!!");
}

#[test]
fn temporal_multi_run_route_requires_transactional_batching() {
    validate_temporal_guest_read_route(1, 2, true, true);
    assert!(
        std::panic::catch_unwind(|| { validate_temporal_guest_read_route(1, 2, true, false) })
            .is_err()
    );
    assert!(std::panic::catch_unwind(|| {
        validate_temporal_guest_read_route(1, 1, false, false)
    })
    .is_err());
    validate_temporal_guest_read_route(0, 2, false, false);
}
