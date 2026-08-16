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
