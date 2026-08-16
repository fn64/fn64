use super::*;

fn synthetic_raw_dpc_ir_ticket(write_end: u32) -> fn64_render::ir::DecodedTicket {
    const COMMAND_START: u32 = 0x100;
    const COLOR_TARGET: u32 = 0x400;
    let commands = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0003, COLOR_TARGET),
        (0xf700_0000, 0xf801_f801),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
    ];
    let words = commands
        .into_iter()
        .flat_map(|(word0, word1)| [word0, word1])
        .collect::<Vec<_>>();
    let command_end = COMMAND_START + u32::try_from(words.len() * 4).unwrap();
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let journal = fn64_render::ir::ResourceJournal::try_new(
        fn64_render::ir::ResourceJournalLimits::try_new(2, 0x100).unwrap(),
        vec![
            fn64_render::ir::ResourceAccess::try_new(
                fn64_render::ir::OperationId::new(0),
                fn64_render::ir::AccessMode::Read,
                fn64_render::ir::AccessPurpose::CommandDecode,
                fn64_render::ir::ResourceRegion::Rdram {
                    resource: fn64_render::ir::RdramResource::RawCommands,
                    range: layout.range(COMMAND_START, command_end).unwrap(),
                },
            )
            .unwrap(),
            fn64_render::ir::ResourceAccess::try_new(
                fn64_render::ir::OperationId::new(1),
                fn64_render::ir::AccessMode::Write,
                fn64_render::ir::AccessPurpose::RenderTarget,
                fn64_render::ir::ResourceRegion::Rdram {
                    resource: fn64_render::ir::RdramResource::ColorFramebuffer,
                    range: layout.range(COLOR_TARGET, write_end).unwrap(),
                },
            )
            .unwrap(),
        ],
    )
    .unwrap();

    fn64_render::decode_raw_dpc_capture(
        layout,
        41,
        fn64_render::OwnedRawDpcSubmission::from_rdram_words(COMMAND_START, command_end, words)
            .unwrap(),
        fn64_render::ir::TemporalBoundary::new(7, fn64_render::ir::DpInterruptState::Clear),
        Vec::new(),
        journal,
    )
    .unwrap()
}

fn synthetic_raw_dpc_packet(
    streams: Vec<(u32, Vec<u32>)>,
    writes: &[(u32, u32)],
) -> fn64_render::ir::DecodedTicket {
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let streams = streams
        .into_iter()
        .enumerate()
        .map(|(index, (start, words))| {
            let end = start + u32::try_from(words.len() * size_of::<u32>()).unwrap();
            fn64_render::ir::RawCommandStream::Dram(
                fn64_render::ir::DramCommandStream::try_new(vec![
                    fn64_render::ir::DramCommandChunk::try_new(
                        layout.range(start, end).unwrap(),
                        words,
                        fn64_render::ir::TemporalBoundary::new(
                            10 + index as u64,
                            fn64_render::ir::DpInterruptState::Clear,
                        ),
                        Vec::new(),
                    )
                    .unwrap(),
                ])
                .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut accesses = streams
        .iter()
        .enumerate()
        .map(|(index, stream)| {
            let (start, end) = stream.source_bounds();
            fn64_render::ir::ResourceAccess::try_new(
                fn64_render::ir::OperationId::new(index as u32),
                fn64_render::ir::AccessMode::Read,
                fn64_render::ir::AccessPurpose::CommandDecode,
                fn64_render::ir::ResourceRegion::Rdram {
                    resource: fn64_render::ir::RdramResource::RawCommands,
                    range: layout.range(start, end).unwrap(),
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    accesses.extend(writes.iter().enumerate().map(|(index, &(start, end))| {
        fn64_render::ir::ResourceAccess::try_new(
            fn64_render::ir::OperationId::new((streams.len() + index) as u32),
            fn64_render::ir::AccessMode::Write,
            fn64_render::ir::AccessPurpose::RenderTarget,
            fn64_render::ir::ResourceRegion::Rdram {
                resource: fn64_render::ir::RdramResource::ColorFramebuffer,
                range: layout.range(start, end).unwrap(),
            },
        )
        .unwrap()
    }));
    let declared_bytes = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum();
    let journal = fn64_render::ir::ResourceJournal::try_new(
        fn64_render::ir::ResourceJournalLimits::try_new(accesses.len(), declared_bytes).unwrap(),
        accesses,
    )
    .unwrap();
    fn64_render::ir::DecodedTicket::new(
        fn64_render::ir::WorkloadPacket::try_new(
            layout,
            fn64_render::ir::WorkloadAdmission::RawDpc {
                transaction_sequence: 77,
            },
            streams,
            journal,
        )
        .unwrap(),
    )
}

fn fill_commands(target: u32, fill_color: u32, trailing_noops: usize) -> Vec<u32> {
    let mut commands = vec![
        0xef00_0000 | (3 << 20),
        0,
        0xff10_0003,
        target,
        0xf700_0000,
        fill_color,
        0xf600_0000 | ((3 * 4) << 12) | 4,
        0,
    ];
    commands.extend(std::iter::repeat_n(0, trailing_noops * 2));
    commands
}

fn synthetic_xbus_ticket() -> fn64_render::ir::DecodedTicket {
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let dmem = fn64_render::ir::DmemRange::try_new(0, 8).unwrap();
    let journal = fn64_render::ir::ResourceJournal::try_new(
        fn64_render::ir::ResourceJournalLimits::try_new(1, 8).unwrap(),
        vec![fn64_render::ir::ResourceAccess::try_new(
            fn64_render::ir::OperationId::new(0),
            fn64_render::ir::AccessMode::Read,
            fn64_render::ir::AccessPurpose::CommandDecode,
            fn64_render::ir::ResourceRegion::RspDmem(dmem),
        )
        .unwrap()],
    )
    .unwrap();
    fn64_render::decode_raw_dpc_capture(
        layout,
        88,
        fn64_render::OwnedRawDpcSubmission::from_xbus_payload(0, 8, vec![0; 8]).unwrap(),
        fn64_render::ir::TemporalBoundary::new(1, fn64_render::ir::DpInterruptState::Clear),
        Vec::new(),
        journal,
    )
    .unwrap()
}

fn synthetic_tmem_write_ticket() -> fn64_render::ir::DecodedTicket {
    let layout = fn64_render::ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let stream = fn64_render::ir::RawCommandStream::Dram(
        fn64_render::ir::DramCommandStream::try_new(vec![
            fn64_render::ir::DramCommandChunk::try_new(
                command_range,
                vec![0, 0],
                fn64_render::ir::TemporalBoundary::new(1, fn64_render::ir::DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap(),
        ])
        .unwrap(),
    );
    let journal = fn64_render::ir::ResourceJournal::try_new(
        fn64_render::ir::ResourceJournalLimits::try_new(2, 16).unwrap(),
        vec![
            fn64_render::ir::ResourceAccess::try_new(
                fn64_render::ir::OperationId::new(0),
                fn64_render::ir::AccessMode::Read,
                fn64_render::ir::AccessPurpose::CommandDecode,
                fn64_render::ir::ResourceRegion::Rdram {
                    resource: fn64_render::ir::RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap(),
            fn64_render::ir::ResourceAccess::try_new(
                fn64_render::ir::OperationId::new(1),
                fn64_render::ir::AccessMode::Write,
                fn64_render::ir::AccessPurpose::TmemLoadDestination,
                fn64_render::ir::ResourceRegion::Tmem(
                    fn64_render::ir::TmemRange::try_new(0, 8).unwrap(),
                ),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    fn64_render::ir::DecodedTicket::new(
        fn64_render::ir::WorkloadPacket::try_new(
            layout,
            fn64_render::ir::WorkloadAdmission::RawDpc {
                transaction_sequence: 89,
            },
            vec![stream],
            journal,
        )
        .unwrap(),
    )
}

fn assert_narrow_scope_rejection(decoded: fn64_render::ir::DecodedTicket, expected_reason: &str) {
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let submitted = submission_owner.submit(decoded).unwrap();
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x7c; 0x1000];
    let before = live_rdram.clone();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let error = renderer_owner.execute(submitted, snapshot, 0).unwrap_err();
    assert!(error.to_string().contains(expected_reason), "{error}");
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
    assert_eq!(
        renderer_owner.backend().last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified
    );
}

#[test]
fn synthetic_raw_dpc_ir_requires_backend_receipt_before_guest_copyback() {
    const COLOR_TARGET: u32 = 0x400;
    const COLOR_END: u32 = COLOR_TARGET + 16;
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let dump_dir = std::env::temp_dir().join(format!(
        "fn64-ir-success-dump-must-not-exist-{}",
        std::process::id()
    ));
    assert!(
        !dump_dir.exists(),
        "stale test diagnostic path {dump_dir:?}"
    );
    let mut backend = fn64_render_reference::ReferenceBackend::new().with_auto_dump(
        &dump_dir,
        "speculative-success",
        1,
    );
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x5a; 0x1000];
    let before = live_rdram.clone();

    let submitted = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(COLOR_END))
        .unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let completed = renderer_owner.execute(submitted, snapshot, 0).unwrap();
    assert_eq!(
        transaction.live_rdram_for_intervening_write_test(),
        before,
        "renderer completion must stage rather than publish guest bytes"
    );
    assert_eq!(completed.ticket().backend_writes().len(), 1);

    let committed = transaction.commit(completed).unwrap();
    let view = fn64_runtime::RdramView::from_storage(&live_rdram);
    for offset in (COLOR_TARGET..COLOR_END).step_by(2) {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(offset)),
            0xf801
        );
    }
    assert_eq!(
        &live_rdram[..COLOR_TARGET as usize],
        &before[..COLOR_TARGET as usize]
    );
    assert_eq!(
        &live_rdram[COLOR_END as usize..],
        &before[COLOR_END as usize..]
    );
    assert_ne!(
        committed.backend_effect_identity(),
        committed.guest_effect_identity()
    );

    // Durable semantic publication belongs after both receipt transitions.
    let retained = fn64_render::CommittedSemanticWorkloadRecord::from_committed(&committed);
    let decoded =
        fn64_render::ir::WorkloadRecord::decode(&retained.replay_record().encode()).unwrap();
    assert_eq!(decoded.workload_identity(), committed.packet().identity());
    assert_eq!(retained.queue(), committed.queue());
    assert!(!dump_dir.exists());
}

#[test]
fn xbus_stream_is_a_loud_first_slice_scope_limit() {
    assert_narrow_scope_rejection(synthetic_xbus_ticket(), "does not stage XBUS streams");
}

#[test]
fn non_rdram_write_is_a_loud_first_slice_scope_limit() {
    assert_narrow_scope_rejection(synthetic_tmem_write_ticket(), "receipts only RDRAM writes");
}

#[test]
fn rejected_raw_dpc_ir_discards_speculation_without_receipt_or_guest_write() {
    const INCOMPLETE_COLOR_DECLARATION_END: u32 = 0x402;
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0xa5; 0x1000];
    let before = live_rdram.clone();

    let submitted = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(
            INCOMPLETE_COLOR_DECLARATION_END,
        ))
        .unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let rejected = renderer_owner
        .execute(submitted, snapshot, 0)
        .expect_err("undeclared renderer effects must reject the whole submission");

    assert!(rejected
        .to_string()
        .contains("wrote undeclared RDRAM byte 0x00000402"));
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
    assert_eq!(
        renderer_owner.backend().last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified,
        "the speculative backend clone escaped rollback"
    );
    // A raw WorkloadRecord may exist as replay data, but no
    // CommittedSemanticWorkloadRecord or architectural raw-DPC observation
    // can be published by this path.
}

#[test]
fn each_overlapping_stream_uses_its_immutable_capture_without_erasing_prior_effects() {
    const FIRST_TARGET_IN_SECOND_STREAM: u32 = 0x140;
    const SECOND_TARGET: u32 = 0x400;
    let decoded = synthetic_raw_dpc_packet(
        vec![
            (
                0x100,
                fill_commands(FIRST_TARGET_IN_SECOND_STREAM, 0xf801_f801, 3),
            ),
            (0x120, fill_commands(SECOND_TARGET, 0x07c1_07c1, 3)),
        ],
        &[
            (
                FIRST_TARGET_IN_SECOND_STREAM,
                FIRST_TARGET_IN_SECOND_STREAM + 16,
            ),
            (SECOND_TARGET, SECOND_TARGET + 16),
        ],
    );
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x5a; 0x1000];
    let submitted = submission_owner.submit(decoded).unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (transaction, snapshot) = captured.transfer_snapshot();
    let completion = renderer_owner.execute(submitted, snapshot, 0).unwrap();
    transaction.commit(completion).unwrap();

    let view = fn64_runtime::RdramView::from_storage(&live_rdram);
    for offset in (FIRST_TARGET_IN_SECOND_STREAM..FIRST_TARGET_IN_SECOND_STREAM + 16).step_by(2) {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(offset)),
            0xf801,
            "later command staging erased an earlier renderer write at {offset:#x}"
        );
    }
    for offset in (SECOND_TARGET..SECOND_TARGET + 16).step_by(2) {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(offset)),
            0x07c1,
            "the overlapping second stream did not execute its owned payload at {offset:#x}"
        );
    }
}

#[test]
fn dropped_completion_publishes_neither_guest_bytes_nor_backend_state() {
    const COLOR_END: u32 = 0x410;
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x6b; 0x1000];
    let before = live_rdram.clone();
    let submitted = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(COLOR_END))
        .unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let completion = renderer_owner.execute(submitted, snapshot, 0).unwrap();
    drop(completion);
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
    assert_eq!(
        renderer_owner.backend().last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified
    );
}

#[test]
fn completion_rejects_same_content_image_substitution_before_copyback() {
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut image_a = vec![0x37; 0x1000];
    let mut image_b = image_a.clone();
    let before_b = image_b.clone();
    let submitted_a = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();
    let submitted_b = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();

    let completion = {
        let captured_a = guest_owner.begin(&mut image_a, &submitted_a).unwrap();
        let (_transaction_a, snapshot_a) = captured_a.transfer_snapshot();
        renderer_owner.execute(submitted_a, snapshot_a, 0).unwrap()
    };

    let captured_b = guest_owner.begin(&mut image_b, &submitted_b).unwrap();
    let (transaction_b, snapshot_b) = captured_b.transfer_snapshot();
    drop(snapshot_b);
    assert_eq!(
        transaction_b.commit(completion).unwrap_err(),
        fn64_render::ir::ValidationError::GuestMemoryPreimageMismatch
    );
    assert_eq!(image_b, before_b);
}

#[test]
fn intervening_live_memory_mutation_rejects_before_ticket_or_render_write() {
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x48; 0x1000];
    let submitted = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let completion = renderer_owner.execute(submitted, snapshot, 0).unwrap();
    transaction.live_rdram_for_intervening_write_test()[0x20] ^= 0xff;
    assert_eq!(
        transaction.commit(completion).unwrap_err(),
        fn64_render::ir::ValidationError::GuestMemoryPreimageMismatch
    );
    assert!(live_rdram[0x400..0x410].iter().all(|&byte| byte == 0x48));
}

#[test]
fn same_queue_snapshot_cannot_cross_to_another_submission() {
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let submitted_a = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();
    let submitted_b = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x49; 0x1000];
    let before = live_rdram.clone();
    let captured = guest_owner.begin(&mut live_rdram, &submitted_a).unwrap();
    let (mut transaction, snapshot_a) = captured.transfer_snapshot();

    let error = renderer_owner
        .execute(submitted_b, snapshot_a, 0)
        .unwrap_err();
    assert!(
        error.to_string().contains("different submitted workload"),
        "{error}"
    );
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
    assert_eq!(
        renderer_owner.backend().last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified
    );
    drop(submitted_a);
}

#[test]
fn wrong_backend_authority_rejects_without_guest_or_template_state() {
    let (queue, _paired_backend, guest_authority) = fn64_render::ir::TicketAuthoritySet::try_new()
        .unwrap()
        .into_roles();
    let (_, wrong_backend, _) = fn64_render::ir::TicketAuthoritySet::try_new()
        .unwrap()
        .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let mut backend = fn64_render_reference::ReferenceBackend::new();
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, wrong_backend);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x59; 0x1000];
    let before = live_rdram.clone();
    let submitted = submission_owner
        .submit(synthetic_raw_dpc_ir_ticket(0x410))
        .unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let error = renderer_owner.execute(submitted, snapshot, 0).unwrap_err();
    assert!(error
        .to_string()
        .contains("different lifecycle role authority"));
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
    assert_eq!(
        renderer_owner.backend().last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified
    );
}

#[test]
fn speculative_rejection_does_not_publish_global_observations_or_files() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let invalid_target_commands = vec![
        0xef00_0000 | (3 << 20),
        0,
        0xff00_0003,
        0x800,
        0xf600_0000,
        0,
    ];
    let decoded =
        synthetic_raw_dpc_packet(vec![(0x100, invalid_target_commands)], &[(0x800, 0x810)]);
    let (queue, backend_authority, guest_authority) =
        fn64_render::ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
    let mut submission_owner = RawDpcIrSubmissionOwner::new(queue);
    let dump_dir = std::env::temp_dir().join(format!(
        "fn64-ir-speculative-dump-must-not-exist-{}",
        std::process::id()
    ));
    assert!(
        !dump_dir.exists(),
        "stale test diagnostic path {dump_dir:?}"
    );
    let mut backend =
        fn64_render_reference::ReferenceBackend::new().with_auto_dump(&dump_dir, "speculative", 1);
    backend
        .create(&fn64_render::RenderConfig::ntsc(4, 2))
        .unwrap();
    let mut renderer_owner =
        fn64_render_reference::ReferenceIrRawDpcAdapter::new(backend, backend_authority);
    let mut guest_owner = RawDpcIrGuestCommitOwner::new(guest_authority);
    let mut live_rdram = vec![0x6a; 0x1000];
    let before = live_rdram.clone();
    let submitted = submission_owner.submit(decoded).unwrap();
    let captured = guest_owner.begin(&mut live_rdram, &submitted).unwrap();
    let (mut transaction, snapshot) = captured.transfer_snapshot();
    let error = renderer_owner.execute(submitted, snapshot, 0).unwrap_err();
    assert!(error.to_string().contains("format=0 size=0"), "{error}");
    assert!(fn64_runtime::copy_unsupported_events().is_empty());
    assert!(!dump_dir.exists());
    assert_eq!(transaction.live_rdram_for_intervening_write_test(), before);
}

// -- LiveDpcTransaction -> ReadyDpcFabricCommit typestate --------------------

#[test]
fn with_ready_commit_advances_current_like_direct_commit() {
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let mut transaction = LiveDpcTransaction::new(submission);
    transaction.validate_atomic_completion();
    transaction.with_ready_commit(|ready| ready.commit());

    let after_device = with_host(|host| host.device_fabric.snapshot());
    assert_ne!(
        after_device, before_device,
        "commit must advance CURRENT past the admitted range"
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

#[test]
fn with_ready_commit_succeeds_through_a_real_interleaved_xbus_mode_command() {
    // ABI-level companion to
    // `fn64_runtime::device::fabric_ops::ready_dpc_fabric_commit_tests::
    // prepare_and_commit_survive_an_interleaved_xbus_mode_command`, driven
    // through the actual production seam (`LiveDpcTransaction::with_ready_commit`,
    // not a direct `prepare_dpc_commit` call). After RDRAM admission and the
    // transaction's atomic-completion acknowledgment, the guest issues a
    // real, publicly reachable `write_mmio` STATUS mode command --
    // `DeviceFault`-free, not private-field corruption -- setting
    // `DPC_STATUS_XBUS_DMEM_DMA` while this RDRAM-source submission is still
    // pending. `with_ready_commit` must not treat that as stale/corrupted
    // state: `commit_dpc_submission` never reads the XBUS bit, and
    // `cancel_dpc_submission` is separately proven (device_b.rs's
    // `dpc_status_mode_commands_during_renderer_admission_survive_cancellation`)
    // to preserve exactly this interleaving through cancellation. This test
    // proves the same tolerance holds through the commit path.
    crate::load_rom(Vec::new());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let mut transaction = LiveDpcTransaction::new(submission);
    transaction.validate_atomic_completion();

    // Command `0x02` sets DPC_STATUS_XBUS_DMEM_DMA (bit 0 clears it, bit 1
    // sets it -- see `apply_dpc_status_mode_commands`'s clear/set pairing).
    // `MmioAddr::new(0xA410_000C)` is the real DPC_STATUS MMIO address; this
    // is the exact public write a guest CPU issues, reached the same way
    // production code would reach it.
    let dpc_status_reg = fn64_runtime::MmioAddr::new(0xA410_000C);
    let _ = with_host(|host| host.device_fabric.write_mmio(dpc_status_reg, 0x02))
        .expect("a real STATUS mode command must not itself be rejected");
    assert_eq!(
        with_host(|host| host.device_fabric.read_mmio(dpc_status_reg)).unwrap()
            & fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA,
        fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA,
        "the interleaved mode command must have taken effect before with_ready_commit runs"
    );

    transaction.with_ready_commit(|ready| ready.commit());

    assert_eq!(
        with_host(|host| host.device_fabric.read_mmio(dpc_status_reg)).unwrap()
            & fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA,
        fn64_runtime::DPC_STATUS_XBUS_DMEM_DMA,
        "commit must not silently revert the interleaved mode command"
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

#[test]
fn with_ready_commit_cancels_exactly_once_when_acknowledgment_is_not_yet_complete() {
    // Regression for the leak hazard: `with_ready_commit` must NOT disarm
    // `LiveDpcTransaction`'s own cancel guard until a `ReadyDpcFabricCommit`
    // actually exists. This test calls `with_ready_commit` WITHOUT first
    // calling `validate_atomic_completion`, so the acknowledgment-phase
    // `assert_eq!` inside `with_ready_commit` panics before
    // `prepare_dpc_commit` ever runs -- no `ReadyDpcFabricCommit` is
    // constructed. If `self.token` had already been taken before that
    // assertion (the earlier, buggy ordering), `LiveDpcTransaction::drop`
    // would find `None` and silently no-op, leaking the admitted fabric
    // range as permanently busy with no owner left to cancel it. With the
    // corrected ordering, `self.token` is still `Some` when the assertion
    // panics, so `LiveDpcTransaction::drop` is the one live cancellation
    // owner and rolls the fabric back exactly once.
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let transaction = LiveDpcTransaction::new(submission);
    // Deliberately skip `validate_atomic_completion()`.

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transaction.with_ready_commit(|_ready| {
            panic!("with_ready_commit's own acknowledgment assertion should have already panicked")
        });
    }));
    assert!(
        result.is_err(),
        "the acknowledgment-phase assertion must panic (not silently proceed) with no ack"
    );

    // Exactly one cancellation ran: `LiveDpcTransaction::drop`'s, because no
    // `ReadyDpcFabricCommit` was ever constructed to own a second one.
    assert_eq!(
        with_host(|host| host.device_fabric.snapshot()),
        before_device,
        "the pending fabric transaction must not leak when readiness is never reached"
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

// A third `with_ready_commit` hostile -- "`prepare_dpc_commit` itself
// rejects while `token` still legitimately owns the pending slot, and the
// OUTER `LiveDpcTransaction::drop` then cancels that same still-matching
// slot cleanly" -- was believed to have no ABI-reachable trigger. That
// belief was WRONG once: `prepare_dpc_commit` used to also check that
// `self.dpc.status`'s `DPC_STATUS_XBUS_DMEM_DMA` bit still matched the
// submission's source, and a real, publicly reachable
// `write_mmio(DPC_STATUS_REG, ..)` STATUS mode command -- issued by the
// guest CPU while an RDRAM-source submission was pending -- could flip that
// exact bit and trigger that exact rejection while `token` still legitimately
// owned the slot. Rather than add a hostile proving that rejection cancels
// cleanly, the check itself was removed as a false positive:
// `commit_dpc_submission` never reads the XBUS bit, and
// `cancel_dpc_submission` is separately, deliberately designed to preserve
// (not discard) this exact interleaving -- see
// `dpc_status_mode_commands_during_renderer_admission_survive_cancellation`
// in `fn64-runtime`'s `device/tests/device_b.rs`. The commit-path proof that
// this interleaving now survives cleanly, instead of rejecting, is
// `with_ready_commit_succeeds_through_a_real_interleaved_xbus_mode_command`
// above.
//
// Every OTHER `DeviceFault` `prepare_dpc_commit` can return while `token` no
// longer matches the pending slot (`NoPendingDpcSubmission`,
// token-mismatched `StaleDpcSubmission`) implies the fabric already has
// nothing left for `LiveDpcTransaction::drop`'s own subsequent
// `cancel_dpc_submission(token)` to cancel either -- both panics would then
// be separately honest reports of the same real state, and Rust aborts the
// process when a second panic unwinds through an already-panicking `Drop`
// (general Rust behavior, not specific to this code). The remaining
// rejections that leave `token`'s slot correctly restored and still
// cancellable (`InvalidDpcRange`, rollback-consistency `StaleDpcSubmission`)
// are individually reachability-audited as unreachable through the real
// admission path (see `prepare_dpc_commit`'s own doc comment in
// `fabric_ops.rs` for the audit of each) and are exercised instead at the
// runtime level, where `PendingDpc` can be hand-corrupted directly:
// `fn64_runtime::device::fabric_ops::ready_dpc_fabric_commit_tests::
// prepare_rejects_an_inconsistent_rollback_image_and_leaves_it_cancellable`.

#[test]
fn with_ready_commit_disarms_transaction_before_assemble_so_panic_cancels_exactly_once() {
    // Regression for the double-cancel hazard: `LiveDpcTransaction::drop`
    // and `ReadyDpcFabricCommit::drop` are two independent cancellation
    // paths over the same fabric state. If `LiveDpcTransaction` still owned
    // its token when `assemble` panicked, BOTH would try to cancel --
    // `ReadyDpcFabricCommit::drop` runs first (constructed later, unwinds
    // first) and clears `pending_dpc`, then `LiveDpcTransaction::drop` would
    // call `cancel_dpc_submission` again, find `pending_dpc` already `None`,
    // and panic on `DeviceFault::NoPendingDpcSubmission` from inside an
    // unwind already in progress -- which aborts the process rather than
    // propagating one clean panic (an abort cannot be caught by
    // `catch_unwind`, so if this regressed, this test itself would not
    // report a failure -- it would kill the test process outright).
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let mut transaction = LiveDpcTransaction::new(submission);
    transaction.validate_atomic_completion();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transaction.with_ready_commit(|_ready| {
            panic!("synthetic panic while assembling a capsule from the ready value");
        });
    }));
    assert!(
        result.is_err(),
        "the synthetic panic must propagate as exactly one panic, not an abort"
    );

    // Exactly one cancellation ran (ReadyDpcFabricCommit's, via field
    // writes): registers are back to their pre-admission state, and the
    // pending submission is gone. If LiveDpcTransaction's OWN drop had also
    // fired a second cancel, this process would already have aborted above
    // instead of reaching this assertion.
    assert_eq!(
        with_host(|host| host.device_fabric.snapshot()),
        before_device
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

#[test]
fn with_ready_commit_lets_a_caller_assemble_then_commit_from_inside_the_closure() {
    // The CPS seam's actual purpose: a caller builds something (standing in
    // for a future T0 capsule) from the live `ReadyDpcFabricCommit` and
    // decides what to do with it, all inside one `with_host` borrow, rather
    // than the value being committed before the caller ever sees it.
    crate::load_rom(Vec::new());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let mut transaction = LiveDpcTransaction::new(submission);
    transaction.validate_atomic_completion();

    // `assemble` stands in for a future capsule-construction call: it
    // receives the live `ReadyDpcFabricCommit`, does its own work (here,
    // nothing -- a real caller would combine it with a guest-committed
    // wrapper into a sealed capsule), and only THEN commits it, all inside
    // the one `with_host` borrow `with_ready_commit` provides.
    let assembled_before_commit = std::cell::Cell::new(false);
    transaction.with_ready_commit(|ready| {
        assembled_before_commit.set(true);
        ready.commit();
    });
    assert!(
        assembled_before_commit.get(),
        "assemble must run with a live ReadyDpcFabricCommit before any commit happens"
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

#[test]
fn dropping_live_dpc_transaction_before_with_ready_commit_cancels() {
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    let transaction = LiveDpcTransaction::new(submission);
    // No `commit`/`with_ready_commit` call: this is the early-
    // rejection path, same as an ordinary backend error would take.
    drop(transaction);

    let after_device = with_host(|host| host.device_fabric.snapshot());
    assert_eq!(
        after_device, before_device,
        "an uncommitted transaction's drop must roll back exactly like cancel_dpc_submission"
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

// -- ReadyDpcFabricCommit at the ABI seam: wrong-token / drop / unwind /
// field-isolation, driven through `with_host` exactly as production code
// reaches it (not through the runtime-crate-internal hostile constructor
// `fabric_ops`'s own tests use). These are the tests requested when the
// generic `ReadyDpcFabricCommit<'a, R, T>` was replaced with the concrete,
// field-only `ReadyDpcFabricCommit<'a>` -- they exercise the type from the
// ABI side of the boundary the redesign exists to serve.

#[test]
fn abi_seam_wrong_token_is_rejected_before_any_mutation() {
    crate::load_rom(Vec::new());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    // Baseline captured AFTER admission: admission itself mutates
    // dpc.start/end/current/status, so a pre-admission snapshot would make
    // this assertion compare against the wrong state.
    let after_admission = with_host(|host| host.device_fabric.snapshot());
    let error = with_host(|host| {
        host.device_fabric
            .prepare_dpc_commit(submission.token.wrapping_add(1))
            .err()
    })
    .expect("a mismatched token must be rejected, not silently accepted");
    assert_eq!(
        error,
        fn64_runtime::DeviceFault::StaleDpcSubmission {
            pending_token: submission.token,
            received_token: submission.token.wrapping_add(1),
        }
    );
    // Rejection is nonmutating: the real pending submission is untouched, and
    // a correct-token prepare afterward still succeeds against it.
    assert_eq!(
        with_host(|host| host.device_fabric.snapshot()),
        after_admission
    );
    with_host(|host| {
        host.device_fabric
            .prepare_dpc_commit(submission.token)
            .expect("the real token must still prepare after a wrong-token rejection")
            .commit();
    });
}

#[test]
fn abi_seam_drop_without_commit_cancels_exactly_once() {
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    with_host(|host| {
        let ready = host
            .device_fabric
            .prepare_dpc_commit(submission.token)
            .unwrap();
        drop(ready);
    });
    assert_eq!(
        with_host(|host| host.device_fabric.snapshot()),
        before_device,
        "drop-cancel at the ABI seam must roll back exactly like a direct cancel"
    );
    // Never retried: the pending submission is gone, so the same token
    // cannot prepare a second time.
    let retry = with_host(|host| {
        host.device_fabric
            .prepare_dpc_commit(submission.token)
            .err()
    });
    assert_eq!(
        retry,
        Some(fn64_runtime::DeviceFault::NoPendingDpcSubmission)
    );
}

#[test]
fn abi_seam_panic_mid_with_host_unwinds_through_drop_and_cancels() {
    crate::load_rom(Vec::new());
    let before_device = with_host(|host| host.device_fabric.snapshot());
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_host(|host| {
            let _ready = host
                .device_fabric
                .prepare_dpc_commit(submission.token)
                .unwrap();
            panic!("synthetic panic while a ReadyDpcFabricCommit is live inside with_host");
        });
    }));
    assert!(result.is_err());

    // The panic unwound through `ReadyDpcFabricCommit`'s `Drop` before
    // `with_host`'s own `RefCell::borrow_mut()` guard dropped, so `HOST` is
    // not left borrowed and the fabric shows a clean cancel -- both an
    // unwind-safety property and the retirement-style exact-once-terminal
    // property this typestate is built to guarantee.
    assert_eq!(
        with_host(|host| host.device_fabric.snapshot()),
        before_device
    );
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}

#[test]
fn abi_seam_field_isolation_render_backend_and_host_are_independent_refcells() {
    // The redesign's motivating property, proven at the actual ABI seam: a
    // `ReadyDpcFabricCommit` prepared from inside one `with_host` closure
    // coexists with a completely independent `RENDER_BACKEND` borrow. This
    // was already true before the redesign (the two are separate
    // `thread_local!` `RefCell`s, not fields of the same struct), but the
    // redesign is what makes it possible for the SAME property to hold if a
    // future `fn64-render` sealed capsule wants to hold a `ReadyDpcFabricCommit`
    // while also invoking a `Box<dyn RenderBackend>` method through
    // `RENDER_BACKEND` -- that capsule cannot exist at all if
    // `ReadyDpcFabricCommit` remains generic over `fn64-runtime`'s private
    // `R, T` (see this file's `with_ready_commit` doc comment).
    crate::load_rom(Vec::new());
    set_render_backend(
        Box::new(StatusRenderBackend(fn64_render::FrameStatus::Complete)),
        0x1000,
    );
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            0x100,
            0x180,
        )
    })
    .unwrap()
    .expect("unfrozen DPC submission must publish");
    with_host(|host| {
        let ready = host
            .device_fabric
            .prepare_dpc_commit(submission.token)
            .unwrap();
        // `RENDER_BACKEND` is borrowed here, independently of `HOST`/`ready`,
        // exactly as `with_ready_commit`'s doc comment claims.
        RENDER_BACKEND.with(|cell| {
            let mut backend = cell.borrow_mut();
            let backend = backend.as_mut().expect("backend was just registered");
            backend
                .create(&fn64_render::RenderConfig::ntsc(4, 2))
                .unwrap();
        });
        ready.commit();
    });
    assert!(with_host(|host| host.device_fabric.pending_dpc_submission()).is_none());
}
