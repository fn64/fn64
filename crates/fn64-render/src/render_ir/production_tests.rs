use super::*;
use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, DpInterruptState, OperationId,
    PhysicalMemoryLayout, RdramResource, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    TemporalBoundary, TmemRange as IrTmemRange,
};
use std::{cell::RefCell, rc::Rc};

use crate::{OwnedRawDpcCapture, OwnedRawDpcSubmission, RawDpcSource};

/// Build one TMEM-load-shaped, no-FullSync, zero-guest-write plan
/// request/writer pair through the sealed session/authority roles --
/// the exact class of packet T0's frozen scope admits (card v10
/// section 1).
fn planned_fixture(
    session: &RawDpcAbiSession,
    authority: &RawDpcBackendAuthority,
    dram: bool,
) -> (PlannedRawDpcSubmission, ResourceAccess) {
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let source_range = layout.range(0x200, 0x210).unwrap();

    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        if dram {
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: command_range,
            }
        } else {
            ResourceRegion::RspDmem(
                fn64_render_ir::DmemRange::try_new(
                    command_range.start().get(),
                    command_range.end(),
                )
                .unwrap(),
            )
        },
    )
    .unwrap();
    let tmem_source = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Read,
        AccessPurpose::TmemLoadSource,
        ResourceRegion::Rdram {
            resource: RdramResource::Buffer,
            range: source_range,
        },
    )
    .unwrap();
    let tmem_destination = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Write,
        AccessPurpose::TmemLoadDestination,
        ResourceRegion::Tmem(IrTmemRange::try_new(0, 16).unwrap()),
    )
    .unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(4, 0x100).unwrap(),
        vec![command_read, tmem_source, tmem_destination],
    )
    .unwrap();

    let submission = if dram {
        OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0]).unwrap()
    } else {
        OwnedRawDpcSubmission::from_xbus_payload(
            0x100,
            0x108,
            [0xf500_0000u32, 0]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect(),
        )
        .unwrap()
    };
    let capture = OwnedRawDpcCapture::new(
        submission,
        layout,
        7,
        TemporalBoundary::new(11, DpInterruptState::Clear),
    );

    let request = session.plan_request(capture);
    let mut writer = authority.begin_plan(request);
    writer.push_command_decode_access(command_read);
    let location = RawDpcCommandLocation {
        command_index: 0,
        stream_index: 0,
        chunk_index: 0,
        source_address: layout.address(command_range.start().get()).unwrap(),
        source_byte_offset: 0,
        source_byte_len: 8,
        wire_opcode: 0xf5,
    };
    let tile_descriptor = NeutralTileDescriptor {
        format: NeutralImageFormat::Rgba,
        size: NeutralPixelSize::Bits16,
        line_words: 2,
        tmem_word_address: 0,
        palette: 0,
        s_mode: NeutralTileAddressMode::default(),
        mask_s: 0,
        shift_s: 0,
        t_mode: NeutralTileAddressMode::default(),
        mask_t: 0,
        shift_t: 0,
    };
    let source_image = NeutralTextureImage {
        format: NeutralImageFormat::Rgba,
        size: NeutralPixelSize::Bits16,
        width: 4,
        address: layout.address(source_range.start().get()).unwrap(),
    };
    let transfer_words = vec![NeutralTmemTransferWord {
        index: 0,
        logical_source_offset: 0,
        source_access_index: 1,
        source_access_byte_offset: 0,
        defined_source_byte_mask: 0xff,
        defined_destination_byte_mask: 0xff,
        destination_word: 0,
        row_advance: 0,
        odd_row_exchange: false,
        physical: NeutralTmemTransferPhysicalWord::Linear(IrTmemRange::try_new(0, 8).unwrap()),
    }];
    let load = TmemLoadSemantics::new(
        location,
        vec![0xf500_0000, 0],
        TmemLoadEpoch::new(core::num::NonZeroU64::new(1).unwrap()),
        TmemLoadKind::Tile {
            bounds: NeutralTileSize {
                low_s: 0,
                low_t: 0,
                high_s: 16,
                high_t: 16,
            },
        },
        0,
        source_image,
        tile_descriptor,
        vec![tmem_source],
        1,
        tmem_destination,
        2,
        16,
        0,
        1,
        1,
        TmemTransferLayout::Linear,
        transfer_words,
    );
    writer.push_tmem_load(load);
    let planned = writer.finish(journal).unwrap();

    (planned, tmem_destination)
}

/// Build one triangle-only, genuinely zero-write plan: a single
/// command-decode read access (so the journal is non-empty --
/// [`fn64_render_ir::ResourceJournal::try_new`] rejects an empty
/// access list outright) plus one [`RdpTriangleCommand`], which
/// itself pushes zero [`ResourceAccess`] entries (see that type's
/// own doc comment). Unlike [`planned_fixture`], no TMEM
/// source/destination access exists anywhere in this plan's journal
/// -- this is the realistic shape
/// `complete_execution_preserving_physical` exists for.
fn triangle_planned_fixture(
    session: &RawDpcAbiSession,
    authority: &RawDpcBackendAuthority,
) -> PlannedRawDpcSubmission {
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    // 0xc8 (TRI_FILL, `command & 0x3f == 0x08`) is a 32-byte (8-word)
    // command per `raw_rdp_command_width` -- the stream must supply
    // the full width or the preflight's own truncation scan rejects
    // it before this fixture's `writer.finish` is ever reached.
    let command_range = layout.range(0x100, 0x120).unwrap();

    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(4, 0x100).unwrap(),
        vec![command_read],
    )
    .unwrap();

    let words = vec![0xc800_0000, 0, 0, 0, 0, 0, 0, 0];
    let submission = OwnedRawDpcSubmission::from_rdram_words(0x100, 0x120, words.clone()).unwrap();
    let capture = OwnedRawDpcCapture::new(
        submission,
        layout,
        7,
        TemporalBoundary::new(11, DpInterruptState::Clear),
    );

    let request = session.plan_request(capture);
    let mut writer = authority.begin_plan(request);
    writer.push_command_decode_access(command_read);
    let location = RawDpcCommandLocation {
        command_index: 0,
        stream_index: 0,
        chunk_index: 0,
        source_address: layout.address(command_range.start().get()).unwrap(),
        source_byte_offset: 0,
        source_byte_len: 32,
        wire_opcode: 0xc8,
    };
    writer.push_triangle(RdpTriangleCommand {
        location,
        raw_words: words.into_boxed_slice(),
        source: TriangleSource::RawTriangle,
        viewport: None,
        texrect_accesses: None,
        vertices: [NeutralTriangleVertex {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            texcoord: [0.0, 0.0],
        }; 3],
    });
    writer.finish(journal).unwrap()
}

fn matching_guest_read_capture(planned: &PlannedRawDpcSubmission) -> DeferredGuestReadCapture {
    DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                fn64_render_ir::CapturedGuestRead::try_new(
                    *read,
                    vec![0xab; read.range().len() as usize],
                )
                .unwrap()
            })
            .collect(),
    )
}

#[derive(Default)]
struct RecordingVisitor {
    commands: usize,
    accesses: usize,
}

impl ExactRawDpcPlanVisitor for RecordingVisitor {
    fn command(&mut self, _command: RawDpcSemanticCommandRef<'_>) {
        self.commands += 1;
    }

    fn access(&mut self, _access: ResourceAccess) {
        self.accesses += 1;
    }
}

#[test]
fn plan_request_is_stamped_with_the_session_queue_and_paired_authority_accepts_it() {
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
        PhysicalMemoryLayout::try_new(0x100).unwrap(),
        0,
        TemporalBoundary::new(0, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let request_queue = request.queue();
    assert_eq!(request_queue, session.queue.identity());
    // Paired authority accepts it without panicking.
    let _writer = authority.begin_plan(request);
}

#[test]
#[should_panic(expected = "not paired")]
fn unrelated_authority_traps_before_any_plan_field_can_be_written() {
    let (session, _own_authority) = new_raw_dpc_roles().unwrap();
    let (_foreign_session, foreign_authority) = new_raw_dpc_roles().unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
        PhysicalMemoryLayout::try_new(0x100).unwrap(),
        0,
        TemporalBoundary::new(0, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let _writer = foreign_authority.begin_plan(request);
}

#[test]
fn begin_plan_consumes_the_request_so_it_cannot_mint_a_second_writer() {
    // Compile-shape proof, not a runtime assertion: `begin_plan` taking
    // `RawDpcPlanRequest` by value means a second call using the same
    // `request` binding is a move-after-move compile error, not a
    // runtime possibility to test. The `#[test]` here exists so this
    // guarantee has a named, discoverable anchor in the suite; see the
    // `compile_fail` doctest on `RawDpcPlanRequest` for the actual
    // enforcement proof.
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
        PhysicalMemoryLayout::try_new(0x100).unwrap(),
        0,
        TemporalBoundary::new(0, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let _writer = authority.begin_plan(request);
}

#[test]
fn writer_finish_produces_a_plan_with_every_pushed_command_and_access() {
    // Item 6: `PlannedRawDpcSubmission`/`BoundSubmittedRawDpc`/`BackendPreparedRawDpc`
    // expose no `visit_plan` -- ABI must not get a plan-extraction
    // surface. This test proves the sealed plan's *content* is
    // correct using same-module access to the private `plan` field
    // (the same access level `production::`'s own code has, not a
    // capability ABI or `fn64-render-wgpu` can reach), rather than a
    // public visitor call.
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&session, &authority, true);
    let mut visitor = RecordingVisitor::default();
    planned.plan.visit(&mut visitor);
    assert_eq!(visitor.commands, 1);
    // command_decode + tmem_source + tmem_destination
    assert_eq!(visitor.accesses, 3);
}

/// Item 5 audit: `finish`'s call to `super::preflight_raw_dpc_capture`
/// passes `Vec::new()` only for `full_sync_boundaries` (legitimately
/// empty in this no-FullSync frozen slice), never for the capture's
/// own stream/source binding. Proves the resulting preflight's guest
/// read plan carries the capture's exact memory layout and that the
/// plan's own source identity is the exact submitted command bytes'
/// identity -- neither is silently dropped or replaced with a
/// default. This is also the property the XBUS-mode-change
/// correction on [`CommittedRawDpcOutcome`] depends on: the plan's
/// source identity is the *captured* submission's, not something
/// this test (or the real seam) ever recomputes from live device
/// state.
#[test]
fn finish_preserves_the_capture_memory_layout_and_source_identity_not_a_default() {
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let layout = PhysicalMemoryLayout::try_new(0x2000).unwrap();
    let submission =
        OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0]).unwrap();
    let expected_source_identity = submission.identity();
    let capture = OwnedRawDpcCapture::new(
        submission,
        layout,
        42,
        TemporalBoundary::new(99, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let mut writer = authority.begin_plan(request);
    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: layout.range(0x100, 0x108).unwrap(),
        },
    )
    .unwrap();
    writer.push_command_decode_access(command_read);
    let journal = journal_of(vec![command_read]);
    let planned = writer.finish(journal).unwrap();

    // The preflight's guest read plan carries the capture's exact
    // memory layout -- not `PhysicalMemoryLayout::default()` or an
    // unrelated value a discarded binding would produce.
    assert_eq!(planned.guest_read_plan().memory_layout(), layout);
    // The plan's source identity is the submission's own content
    // hash, not a placeholder -- proving the writer's `finish` bound
    // the plan to the exact bytes/range it was given, not a
    // structurally-similar but distinct capture.
    assert_eq!(planned.plan.source_identity(), expected_source_identity);
}

/// Fixture for item 2's hostile access-list tests: one writer that
/// pushed exactly `command_read` and one `push_command_decode_access`
/// call (no TMEM load), plus the matching baseline journal. Each
/// hostile test mutates only the journal passed to `finish`, proving
/// `finish` rejects every one-field departure from what the writer
/// actually accumulated.
fn hostile_access_fixture(
    session: &RawDpcAbiSession,
    authority: &RawDpcBackendAuthority,
) -> (ExactRawDpcPlanWriter, ResourceAccess, ResourceAccess) {
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let other_range = layout.range(0x300, 0x308).unwrap();
    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let other_access = ResourceAccess::try_new(
        OperationId::new(9),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: other_range,
        },
    )
    .unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0]).unwrap(),
        layout,
        7,
        TemporalBoundary::new(11, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let mut writer = authority.begin_plan(request);
    writer.push_command_decode_access(command_read);
    (writer, command_read, other_access)
}

fn journal_of(accesses: Vec<ResourceAccess>) -> ResourceJournal {
    ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x100).unwrap(), accesses).unwrap()
}

#[test]
fn finish_rejects_a_journal_missing_an_access_the_writer_pushed() {
    // `ResourceJournal` itself refuses an empty access list, so the
    // reachable "missing" case through `finish` is a nonempty journal
    // with fewer entries than the writer actually pushed. The
    // writer's single pushed access is replaced (not just dropped)
    // to keep the journal nonempty, so this exercises the
    // access-mismatch branch rather than journal construction.
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let (writer, _command_read, other_access) = hostile_access_fixture(&session, &authority);
    let short_journal = journal_of(vec![other_access]);
    assert_eq!(
        writer.finish(short_journal).unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "raw-DPC plan writer accumulated access",
            index: 0,
        }
    );
}

#[test]
fn finish_rejects_a_journal_with_an_extra_access_the_writer_never_pushed() {
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let (writer, command_read, other_access) = hostile_access_fixture(&session, &authority);
    let extra_journal = journal_of(vec![command_read, other_access]);
    assert_eq!(
        writer.finish(extra_journal).unwrap_err(),
        ValidationError::EffectCountMismatch {
            field: "raw-DPC plan writer accumulated access",
            expected: 2,
            actual: 1,
        }
    );
}

#[test]
fn finish_rejects_a_reordered_journal_even_with_the_same_access_set() {
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let second_range = layout.range(0x300, 0x308).unwrap();
    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let second_access = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: second_range,
        },
    )
    .unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xf500_0000, 0]).unwrap(),
        layout,
        7,
        TemporalBoundary::new(11, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    let mut writer = authority.begin_plan(request);
    // Writer pushes command_read then second_access, in that order.
    writer.push_command_decode_access(command_read);
    writer.push_command_decode_access(second_access);
    // Journal has the identical set, reversed.
    let reordered_journal = journal_of(vec![second_access, command_read]);
    assert_eq!(
        writer.finish(reordered_journal).unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "raw-DPC plan writer accumulated access",
            index: 0,
        }
    );
}

#[test]
fn finish_rejects_a_journal_whose_matching_index_access_was_mutated() {
    let (session, authority) = new_raw_dpc_roles().unwrap();
    let (writer, command_read, _other) = hostile_access_fixture(&session, &authority);
    // Same operation/mode/purpose, but a different region: a mutated
    // access at the same index, same count.
    let mutated = ResourceAccess::try_new(
        command_read.operation(),
        command_read.mode(),
        command_read.purpose(),
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: PhysicalMemoryLayout::try_new(0x1000)
                .unwrap()
                .range(0x400, 0x408)
                .unwrap(),
        },
    )
    .unwrap();
    let mutated_journal = journal_of(vec![mutated]);
    assert_eq!(
        writer.finish(mutated_journal).unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "raw-DPC plan writer accumulated access",
            index: 0,
        }
    );
}

#[test]
fn finalize_and_submit_never_exposes_a_bare_ticket_and_records_a_ledger_handle() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    assert_eq!(bound.ordinal(), 0);
    assert_eq!(session.ledger.handles.len(), 1);
    let handle = &session.ledger.handles[0];
    assert_eq!(handle.submission(), bound.submission());
    assert!(handle.outcome().is_none());
}

#[test]
fn bound_dropped_before_backend_prepare_records_exactly_one_rejected() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    drop(bound);
    let handle = &session.ledger.handles[0];
    assert!(matches!(
        handle.outcome(),
        Some(RawDpcTerminalOutcome::Rejected {
            stage: RawDpcRetirementStage::Execute,
            ..
        })
    ));
}

#[test]
fn into_backend_prepared_with_unrelated_authority_traps_before_moving_or_exposing_parts() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let (_, mut foreign_authority) = new_raw_dpc_roles().unwrap();

    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0xab; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bound.into_backend_prepared(&mut foreign_authority, effects)
    }));
    assert!(
        outcome.is_err(),
        "unrelated authority must trap, not silently succeed"
    );
    let handle = &session.ledger.handles[0];
    assert!(matches!(
        handle.outcome(),
        Some(RawDpcTerminalOutcome::Rejected { .. })
    ));
}

#[derive(Default)]
struct RecordingExecutionView {
    plan_commands_seen: usize,
    reads_seen: usize,
    packet_seen: bool,
}

impl RawDpcExecutionView<RecordingVisitor> for RecordingExecutionView {
    fn plan_visited(&mut self, plan_visitor: &mut RecordingVisitor) {
        self.plan_commands_seen = plan_visitor.commands;
    }

    fn captured_reads(&mut self, reads: &[fn64_render_ir::CapturedGuestRead]) {
        self.reads_seen = reads.len();
    }

    fn submitted_packet(&mut self, _packet: &fn64_render_ir::WorkloadPacket) {
        self.packet_seen = true;
    }
}

#[test]
fn execution_view_lends_plan_reads_and_packet_under_the_paired_authority() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let mut plan_visitor = RecordingVisitor::default();
    let mut view = RecordingExecutionView::default();
    bound.execution_view(&authority, &mut plan_visitor, &mut view);

    assert_eq!(view.plan_commands_seen, 1);
    assert_eq!(
        view.reads_seen, 1,
        "the fixture's one TmemLoadSource access"
    );
    assert!(view.packet_seen);
}

#[test]
fn execution_view_with_unrelated_authority_traps_before_lending_anything() {
    let (mut session, _own_authority) = new_raw_dpc_roles().unwrap();
    let (_foreign_session, foreign_authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&session, &_own_authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let mut plan_visitor = RecordingVisitor::default();
    let mut view = RecordingExecutionView::default();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bound.execution_view(&foreign_authority, &mut plan_visitor, &mut view);
    }));
    assert!(
        outcome.is_err(),
        "unrelated authority must trap before lending plan/reads/packet"
    );
    assert_eq!(view.plan_commands_seen, 0);
    assert_eq!(view.reads_seen, 0);
    assert!(!view.packet_seen);
}

#[test]
fn matching_authority_prepares_backend_and_advances_stage() {
    let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0xab; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();

    let prepared = bound
        .into_backend_prepared(&mut authority, effects)
        .unwrap();
    assert_eq!(prepared.stage(), RawDpcRetirementStage::BackendReceipt);
    assert!(session.ledger.handles[0].outcome().is_none());
}

/// B2 defense-in-depth proof: `commit_zero_guest_writes` rejects a
/// packet whose journal declares a real guest-visible write, even
/// though the normal `ExactRawDpcPlanWriter` path can never build one
/// (that gate already runs earlier). This constructs a
/// `GpuCompleteTicket` directly against a hand-built journal with an
/// `Rdram` write access -- bypassing the plan-writer entirely -- to
/// prove the re-check inside `commit_zero_guest_writes` itself
/// (via `GuestCommitEffectReport::try_new`) is what rejects it, not
/// just the earlier, otherwise-unreachable-in-this-test gate.
#[test]
fn commit_zero_guest_writes_rejects_a_packet_with_a_real_guest_write_even_via_a_hand_built_ticket()
{
    let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let guest_write_range = layout.range(0x200, 0x210).unwrap();
    let command_read = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let guest_write = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Write,
        AccessPurpose::RenderTarget,
        ResourceRegion::Rdram {
            resource: RdramResource::ColorFramebuffer,
            range: guest_write_range,
        },
    )
    .unwrap();
    let journal = journal_of(vec![command_read, guest_write]);
    let stream = fn64_render_ir::RawCommandStream::Dram(
        fn64_render_ir::DramCommandStream::try_new(vec![
            fn64_render_ir::DramCommandChunk::try_new(
                command_range,
                vec![0xf500_0000, 0],
                TemporalBoundary::new(11, DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap(),
        ])
        .unwrap(),
    );
    let packet = fn64_render_ir::WorkloadPacket::try_new(
        layout,
        fn64_render_ir::WorkloadAdmission::RawDpc {
            transaction_sequence: 7,
        },
        vec![stream],
        journal,
    )
    .unwrap();

    let (mut queue, mut backend_authority, _guest) = fn64_render_ir::TicketAuthoritySet::try_new()
        .unwrap()
        .into_roles();
    let submitted = queue
        .submit(fn64_render_ir::DecodedTicket::new(packet))
        .unwrap();
    let write_effect = CompletedWrite::try_new(
        guest_write,
        guest_write_range.len(),
        fn64_render_ir::effect_content_digest(&[0xcd; 16]),
    )
    .unwrap();
    let report = BackendEffectReport::try_new(submitted.packet(), vec![write_effect]).unwrap();
    let receipt = backend_authority.issue(&submitted, report).unwrap();
    let complete = submitted.gpu_complete(receipt).unwrap();

    // Hand-build a `BackendPreparedRawDpc` with a fabricated plan and
    // retirement, since this ticket never went through
    // `ExactRawDpcPlanWriter`. Same-module private-field
    // construction is the only way to reach this state at all --
    // proving the production API itself cannot build one.
    let (dummy_retirement, _handle) = SubmittedRawDpcRetirement::new_pair(complete.submission());
    let dummy_plan = ExactValidatedRawDpcPlan {
        source_identity: crate::RawDpcSubmissionIdentity {
            source: RawDpcSource::Rdram,
            start: 0x100,
            end: 0x108,
            command_sha256: [0; 32],
        },
        journal_identity: complete.packet().journal().identity(),
        commands: Vec::new().into_boxed_slice(),
        accesses: Vec::new().into_boxed_slice(),
    };
    let prepared = BackendPreparedRawDpc {
        plan: dummy_plan,
        complete,
        retirement: dummy_retirement,
    };

    // A session sharing the exact same queue the ticket was issued
    // from, so the queue-pairing assertion inside
    // `commit_zero_guest_writes` passes and only the guest-write
    // re-check can reject this call.
    let mut matching_session = RawDpcAbiSession {
        queue,
        guest: fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles()
            .2,
        ledger: RetirementLedger::default(),
    };
    let outcome = matching_session.commit_zero_guest_writes(prepared);
    assert_eq!(
        outcome.unwrap_err(),
        ValidationError::EffectCountMismatch {
            field: "guest commit access",
            expected: 1,
            actual: 0,
        }
    );
}

/// P3 proof: the retirement's shared slot (`Arc<RetirementSlot>`) is
/// the exact same allocation from `finalize_and_submit`'s handle
/// through the fully committed `GuestCommittedRawDpc`'s own
/// `retirement` field -- not a fresh slot minted at any hop.
/// Same-module private-field access reaches both ends of the chain.
#[test]
fn retirement_slot_identity_is_the_same_allocation_across_every_hop() {
    let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0x66; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let prepared = bound
        .into_backend_prepared(&mut authority, effects)
        .unwrap();
    let ledger_slot = session.ledger.handles[0].slot.clone();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    assert!(
        Arc::ptr_eq(&ledger_slot, &committed.retirement.slot),
        "the fully committed value's retirement must share the exact slot \
         the original finalize_and_submit handle points at, not a fresh one"
    );
}

/// One real, admitted `fn64_runtime::device::DeviceFabric` with a
/// fresh DPC submission pending -- built from the same public
/// fixture recipe `fn64-runtime`'s own `ReadyDpcFabricCommit` tests
/// use (`InMemoryRom`/`FixedPiTiming`/`request_dpc_submission`), so
/// this module's capsule tests seal against the concrete T2 type
/// itself, never a stand-in.
fn admitted_fabric(
) -> fn64_runtime::DeviceFabric<fn64_runtime::rom::InMemoryRom, fn64_runtime::FixedPiTiming> {
    let mut fabric = fn64_runtime::DeviceFabric::new(
        fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
        fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
    );
    fabric
        .request_dpc_submission(fn64_runtime::DpcSubmissionSource::Rdram, 0x100, 0x108)
        .unwrap()
        .expect("fresh fabric is never frozen");
    fabric
}

/// Fake, minimal backend physical state (`P` in
/// [`RawDpcCoordinator<P>`]) for T0's own tests -- T3 supplies the
/// real `wgpu` type; T0 only needs to prove the generic mechanism.
/// Ordinary owned data, no `Drop` impl of its own (so this fixture's
/// tests can freely assert on slot contents without a destructor
/// side effect muddying the picture).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FakePhysical(u32);

/// Build one sealed [`GuestCommittedRawDpc`] plus the
/// [`RawDpcCoordinator<FakePhysical>`] it was prepared through, via
/// the full session/coordinator chain: `finalize_and_submit` ->
/// `coordinator.complete_execution` (which stores `next_physical`
/// into the coordinator's inactive slot) -> `commit_zero_guest_writes`.
fn guest_committed_fixture(
    session: &mut RawDpcAbiSession,
    coordinator: &mut RawDpcCoordinator<FakePhysical>,
    next_physical: FakePhysical,
) -> GuestCommittedRawDpc {
    let (planned, destination) = planned_fixture(session, &coordinator.authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0x99; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let prepared = coordinator
        .complete_execution(bound, effects, next_physical)
        .unwrap();
    session.commit_zero_guest_writes(prepared).unwrap()
}

#[test]
fn seal_publication_advances_to_fabric_prepare() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(1));
    let submission = committed.submission();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();

    let capsule = session.seal_publication(committed, ready).unwrap();
    assert_eq!(capsule.stage(), RawDpcRetirementStage::FabricPrepare);
    assert_eq!(capsule.submission(), submission);
    assert!(session.ledger.handles[0].outcome().is_none());
    // Preparing execution must not itself publish: the coordinator's
    // active slot is still the seed value until `commit()`.
    assert_eq!(*coordinator.physical(), FakePhysical(0));
}

#[test]
fn prepare_publication_commit_flips_active_slot_commits_fabric_and_publishes() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(7));
    let submission = committed.submission();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let publication = coordinator.prepare_publication(capsule);
    assert_eq!(
        publication.capsule.stage(),
        RawDpcRetirementStage::PhysicalPrepare,
        "prepare_publication must advance to PhysicalPrepare before returning, \
         so that stage is observable even if commit is never called"
    );
    let outcome = publication.commit();
    assert_eq!(outcome.submission(), submission);
    assert_eq!(
        *coordinator.physical(),
        FakePhysical(7),
        "commit must flip the coordinator's active slot to the prepared candidate"
    );
    assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
    assert_eq!(
        session.ledger.handles[0].outcome(),
        Some(RawDpcTerminalOutcome::Published)
    );
}

#[test]
fn execution_batch_chains_private_successors_then_publishes_each_submission_in_order() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let (planned_a, destination_a) = planned_fixture(&session, &coordinator.authority, true);
    let capture_a = matching_guest_read_capture(&planned_a);
    let (planned_b, destination_b) = planned_fixture(&session, &coordinator.authority, true);
    let capture_b = matching_guest_read_capture(&planned_b);

    let (prepared_a, prepared_b) = {
        let mut batch = coordinator.begin_execution_batch();

        let bound_a = session.finalize_and_submit(planned_a, capture_a).unwrap();
        let effects_a = BackendEffectReport::try_new(
            bound_a.submitted.packet(),
            vec![CompletedWrite::try_new(
                destination_a,
                16,
                fn64_render_ir::effect_content_digest(&[0xa1; 16]),
            )
            .unwrap()],
        )
        .unwrap();
        let prepared_a = batch
            .complete_execution(bound_a, effects_a, FakePhysical(1))
            .unwrap();
        assert_eq!(batch.physical(), &FakePhysical(1));

        let bound_b = session.finalize_and_submit(planned_b, capture_b).unwrap();
        let effects_b = BackendEffectReport::try_new(
            bound_b.submitted.packet(),
            vec![CompletedWrite::try_new(
                destination_b,
                16,
                fn64_render_ir::effect_content_digest(&[0xb2; 16]),
            )
            .unwrap()],
        )
        .unwrap();
        let prepared_b = batch
            .complete_execution(bound_b, effects_b, FakePhysical(2))
            .unwrap();
        assert_eq!(batch.physical(), &FakePhysical(2));
        batch.finish();
        (prepared_a, prepared_b)
    };

    assert_eq!(
        coordinator.physical(),
        &FakePhysical(0),
        "batch execution must not publish a private successor"
    );
    let committed_a = session.commit_zero_guest_writes(prepared_a).unwrap();
    let committed_b = session.commit_zero_guest_writes(prepared_b).unwrap();

    let mut fabric_a = admitted_fabric();
    let token_a = fabric_a.pending_dpc_submission().unwrap().token;
    let ready_a = fabric_a.prepare_dpc_commit(token_a).unwrap();
    let capsule_a = session.seal_publication(committed_a, ready_a).unwrap();
    coordinator.prepare_publication(capsule_a).commit();
    assert_eq!(coordinator.physical(), &FakePhysical(1));

    let mut fabric_b = admitted_fabric();
    let token_b = fabric_b.pending_dpc_submission().unwrap().token;
    let ready_b = fabric_b.prepare_dpc_commit(token_b).unwrap();
    let capsule_b = session.seal_publication(committed_b, ready_b).unwrap();
    coordinator.prepare_publication(capsule_b).commit();
    assert_eq!(coordinator.physical(), &FakePhysical(2));
    assert_eq!(
        session.ledger.handles[0].outcome(),
        Some(RawDpcTerminalOutcome::Published)
    );
    assert_eq!(
        session.ledger.handles[1].outcome(),
        Some(RawDpcTerminalOutcome::Published)
    );
}

/// Physical state that records, via a shared counter, exactly when
/// its `Drop` runs -- so this test can prove *where* an old
/// inactive-slot `P` is actually dropped, not just that it
/// eventually is.
struct DroppableFakePhysical {
    id: u32,
    drops: Rc<RefCell<Vec<u32>>>,
}

impl Drop for DroppableFakePhysical {
    fn drop(&mut self) {
        self.drops.borrow_mut().push(self.id);
    }
}

/// P5 proof: an old inactive-slot `P` is dropped entirely inside
/// `complete_execution` -- a fallible, ordinary step that runs
/// *before* any `ReadyPublication` exists -- never inside
/// `ReadyPublication::commit`'s straight-line body. This is the
/// concrete reason `RawDpcCoordinator` double-buffers instead of
/// `mem::replace`-ing the *active* slot: replacing active in place
/// would run the old active `P`'s `Drop` at the exact moment a new
/// candidate becomes current, inside what must otherwise be a
/// Drop-free commit.
#[test]
fn old_inactive_slot_physical_state_drops_during_complete_execution_not_during_commit() {
    let drops = Rc::new(RefCell::new(Vec::new()));
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(DroppableFakePhysical {
        id: 0,
        drops: Rc::clone(&drops),
    });

    // First submission: `complete_execution` stores id=1 into the
    // (empty) inactive slot. Nothing has dropped yet -- the slot was
    // `None`.
    let (planned, destination) = planned_fixture(&session, &coordinator.authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0x11; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let _first_prepared = coordinator
        .complete_execution(
            bound,
            effects,
            DroppableFakePhysical {
                id: 1,
                drops: Rc::clone(&drops),
            },
        )
        .unwrap();
    assert!(
        drops.borrow().is_empty(),
        "storing into a previously-empty inactive slot drops nothing"
    );

    // Second submission, same never-flipped coordinator:
    // `complete_execution` overwrites that same inactive slot again,
    // dropping id=1 right here -- before any capsule/ReadyPublication
    // for *this* ordinal exists, and long before either ordinal's
    // eventual `commit`.
    let (planned, destination) = planned_fixture(&session, &coordinator.authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0x22; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let _second_prepared = coordinator
        .complete_execution(
            bound,
            effects,
            DroppableFakePhysical {
                id: 2,
                drops: Rc::clone(&drops),
            },
        )
        .unwrap();
    assert_eq!(
        *drops.borrow(),
        vec![1],
        "the superseded id=1 candidate must drop exactly here, inside \
         complete_execution -- not deferred to a later commit"
    );
}

/// Build one bound submission ready to hand to
/// `complete_execution_preserving_physical`, without going through
/// `guest_committed_fixture` (which always calls the ordinary
/// `complete_execution` and requires a `next_physical`).
/// A genuinely zero-write, triangle-only bound submission -- the
/// exact class of packet [`RawDpcCoordinator::
/// complete_execution_preserving_physical`] exists for. Built from
/// [`triangle_planned_fixture`], never [`planned_fixture`] (whose
/// plan always carries a real TMEM destination write access).
fn bound_submission_fixture(
    session: &mut RawDpcAbiSession,
    authority: &RawDpcBackendAuthority,
) -> BoundSubmittedRawDpc {
    let planned = triangle_planned_fixture(session, authority);
    let capture = matching_guest_read_capture(&planned);
    session.finalize_and_submit(planned, capture).unwrap()
}

#[test]
fn preserving_completion_never_clones_touches_or_replaces_either_physical_slot() {
    let drops = Rc::new(RefCell::new(Vec::new()));
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(DroppableFakePhysical {
        id: 0,
        drops: Rc::clone(&drops),
    });

    let bound = bound_submission_fixture(&mut session, &coordinator.authority);
    let _prepared = coordinator
        .complete_execution_preserving_physical(bound)
        .unwrap();

    assert_eq!(
        coordinator.physical().id,
        0,
        "the active slot's physical identity must survive a preserving completion \
         unchanged"
    );
    assert!(
        coordinator.slots[1].is_none(),
        "the inactive slot must remain untouched -- no successor is produced or \
         consumed by a preserving completion"
    );
    assert!(
        drops.borrow().is_empty(),
        "no P is ever cloned, dropped, or otherwise touched by a preserving completion"
    );
}

#[test]
fn preserving_completion_leaves_an_occupied_inactive_slot_untouched() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    // Occupy the inactive slot via an ordinary completion first, so
    // there is real content there to prove untouched.
    let _first = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(42));
    assert_eq!(coordinator.slots[1], Some(FakePhysical(42)));

    let bound = bound_submission_fixture(&mut session, &coordinator.authority);
    let _prepared = coordinator
        .complete_execution_preserving_physical(bound)
        .unwrap();

    assert_eq!(
        coordinator.slots[1],
        Some(FakePhysical(42)),
        "a preserving completion must not read, overwrite, or clear the inactive slot"
    );
    assert_eq!(coordinator.physical(), &FakePhysical(0));
}

#[test]
fn a_second_preserving_completion_replaces_the_first_readys_metadata() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));

    let bound_a = bound_submission_fixture(&mut session, &coordinator.authority);
    let prepared_a = coordinator
        .complete_execution_preserving_physical(bound_a)
        .unwrap();
    let submission_a = prepared_a.submission();
    let ready_after_a = coordinator.ready.as_ref().unwrap();
    assert_eq!(ready_after_a.submission, submission_a);

    let bound_b = bound_submission_fixture(&mut session, &coordinator.authority);
    let prepared_b = coordinator
        .complete_execution_preserving_physical(bound_b)
        .unwrap();
    let submission_b = prepared_b.submission();
    assert_ne!(submission_a, submission_b);

    let ready_after_b = coordinator.ready.as_ref().unwrap();
    assert_eq!(
        ready_after_b.submission, submission_b,
        "a second preserving completion must cleanly replace the first's ready \
         metadata, matching complete_execution's own unconditional overwrite \
         semantics -- not be rejected as \"busy\""
    );

    // Dropping the first prepared capsule (abandoned, never
    // published) must not retroactively poison the second's -- each
    // ordinal owns its own retirement slot.
    drop(prepared_a);
    drop(prepared_b);
}

#[test]
fn dropping_an_abandoned_preserving_completion_does_not_validate_a_later_unrelated_capsule() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));

    let bound_a = bound_submission_fixture(&mut session, &coordinator.authority);
    let prepared_a = coordinator
        .complete_execution_preserving_physical(bound_a)
        .unwrap();
    // Abandon it: dropped without ever reaching
    // prepare_publication/commit. Its retirement records `Rejected`
    // at `BackendReceipt` in its own shared slot.
    drop(prepared_a);

    // A later, unrelated capsule must still be independently
    // provable: complete a fresh ordinal and carry it all the way to
    // a real publication.
    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(3));
    let submission_b = committed.submission();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let publication = coordinator.prepare_publication(capsule);
    let outcome = publication.commit();
    assert_eq!(
        outcome.submission(),
        submission_b,
        "the later capsule must publish as itself, not be confused with the \
         abandoned preserving completion's identity"
    );
    assert_eq!(coordinator.physical(), &FakePhysical(3));
}

#[test]
fn preserving_completion_rejects_a_foreign_authority_before_recording_a_ready_slot() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let coordinator = authority.into_coordinator(FakePhysical(0));
    let bound = bound_submission_fixture(&mut session, &coordinator.authority);

    let (_, foreign_authority) = new_raw_dpc_roles().unwrap();
    // Drive the validation-failing call through a genuinely
    // *foreign* coordinator instead, so the original `coordinator`
    // (and its `self.ready`) is provably untouched by the rejected
    // attempt.
    let mut foreign_coordinator = foreign_authority.into_coordinator(FakePhysical(0));
    let trapped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        foreign_coordinator.complete_execution_preserving_physical(bound)
    }));
    assert!(
        trapped.is_err(),
        "a bound submission paired with an unrelated authority must trap before any \
         ReadyPhysicalSlot is recorded"
    );
    assert!(
        foreign_coordinator.ready.is_none(),
        "a validation failure must never record a ready slot"
    );
    assert!(coordinator.ready.is_none());
}

#[test]
fn replacement_preserve_replacement_leaves_the_coordinator_able_to_publish_a_real_successor() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));

    // 1. A normal complete_execution with a real successor.
    let committed_1 = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(11));
    let mut fabric_1 = admitted_fabric();
    let token_1 = fabric_1.pending_dpc_submission().unwrap().token;
    let ready_1 = fabric_1.prepare_dpc_commit(token_1).unwrap();
    let capsule_1 = session.seal_publication(committed_1, ready_1).unwrap();
    coordinator.prepare_publication(capsule_1).commit();
    assert_eq!(coordinator.physical(), &FakePhysical(11));

    // 2. A preserving completion in between -- must not disturb
    // either slot or block a future real completion.
    let bound_mid = bound_submission_fixture(&mut session, &coordinator.authority);
    let _prepared_mid = coordinator
        .complete_execution_preserving_physical(bound_mid)
        .unwrap();
    assert_eq!(coordinator.physical(), &FakePhysical(11));
    assert_eq!(
        coordinator.slots[0],
        Some(FakePhysical(0)),
        "the preserving completion must not touch the (now-inactive) slot 0, still \
         holding the pre-flip seed value"
    );

    // 3. Another normal complete_execution with a real successor --
    // must cleanly replace the preserving call's ready metadata and
    // publish correctly afterward.
    let committed_3 = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(22));
    let submission_3 = committed_3.submission();
    let mut fabric_3 = admitted_fabric();
    let token_3 = fabric_3.pending_dpc_submission().unwrap().token;
    let ready_3 = fabric_3.prepare_dpc_commit(token_3).unwrap();
    let capsule_3 = session.seal_publication(committed_3, ready_3).unwrap();
    let outcome_3 = coordinator.prepare_publication(capsule_3).commit();

    assert_eq!(outcome_3.submission(), submission_3);
    assert_eq!(coordinator.physical(), &FakePhysical(22));
}

#[test]
fn preserving_completion_rejects_a_write_bearing_packet_before_recording_a_ready_slot() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let coordinator = authority.into_coordinator(FakePhysical(0));
    // `planned_fixture`'s plan carries a real TMEM destination write
    // access -- the exact shape `complete_execution_preserving_physical`
    // must refuse, since it builds its own effects report with an
    // explicitly empty write list and lets
    // `BackendEffectReport::try_new`'s journal-vs-writes length check
    // reject any mismatch.
    let (planned, _destination) = planned_fixture(&session, &coordinator.authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let mut coordinator = coordinator;
    let outcome = coordinator.complete_execution_preserving_physical(bound);
    assert!(
        matches!(outcome, Err(ValidationError::EffectCountMismatch { .. })),
        "a write-bearing packet must be rejected before any ReadyPhysicalSlot is \
         recorded, not silently accepted with a fabricated empty-writes report"
    );
    assert!(
        coordinator.ready.is_none(),
        "a rejected preserving completion must never record a ready slot"
    );
}

#[test]
fn preserving_completion_publishes_end_to_end_preserving_the_exact_physical_identity() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(5));

    let bound = bound_submission_fixture(&mut session, &coordinator.authority);
    let prepared = coordinator
        .complete_execution_preserving_physical(bound)
        .unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    let submission = committed.submission();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let publication = coordinator.prepare_publication(capsule);
    let outcome = publication.commit();

    assert_eq!(outcome.submission(), submission);
    assert_eq!(
        coordinator.physical(),
        &FakePhysical(5),
        "the exact pre-existing physical identity must survive a preserving \
         completion's full publish cycle unchanged -- no successor was ever produced"
    );
    assert_eq!(fabric.rsp_execution_state().dpc_current, 0x108);
    assert_eq!(
        session.ledger.handles[0].outcome(),
        Some(RawDpcTerminalOutcome::Published)
    );
}

#[test]
fn dropping_an_unconsumed_ready_publication_after_a_preserving_completion_rejects_without_flipping_active(
) {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(5));

    let bound = bound_submission_fixture(&mut session, &coordinator.authority);
    let prepared = coordinator
        .complete_execution_preserving_physical(bound)
        .unwrap();
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    // Mirrors the normal complete_execution path's own
    // dropping_an_unconsumed_ready_publication_... coverage below:
    // an unconsumed ReadyPublication for a *preserving* completion
    // must roll back the fabric and reject the retirement exactly
    // the same way, and must never flip `active` -- there is no
    // successor slot to flip to in the first place.
    drop(coordinator.prepare_publication(capsule));

    assert!(fabric.pending_dpc_submission().is_none());
    assert!(matches!(
        session.ledger.handles[0].outcome(),
        Some(RawDpcTerminalOutcome::Rejected {
            stage: RawDpcRetirementStage::PhysicalPrepare,
            ..
        })
    ));
    assert_eq!(
        coordinator.physical(),
        &FakePhysical(5),
        "a dropped, unconsumed ReadyPublication for a preserving completion must \
         never flip the active slot"
    );
}

#[test]
fn dropping_an_unconsumed_ready_publication_cancels_the_fabric_commit_and_rejects_the_retirement_without_flipping_active(
) {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(9));

    // `prepare_dpc_commit`'s rollback restores the *pre-admission*
    // register image (see `fn64-runtime`'s own
    // `dropping_unconsumed_ready_commit_cancels_without_mutation`),
    // so the comparison baseline is a pristine fabric, not one with
    // `request_dpc_submission` already applied.
    let pristine = fn64_runtime::DeviceFabric::new(
        fn64_runtime::rom::PiDma::new(fn64_runtime::rom::InMemoryRom::new(Vec::new())),
        fn64_runtime::FixedPiTiming(fn64_runtime::Cycles::new(0)),
    )
    .rsp_execution_state();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    drop(coordinator.prepare_publication(capsule));

    assert_eq!(fabric.rsp_execution_state(), pristine);
    assert!(fabric.pending_dpc_submission().is_none());
    assert!(matches!(
        session.ledger.handles[0].outcome(),
        Some(RawDpcTerminalOutcome::Rejected {
            stage: RawDpcRetirementStage::PhysicalPrepare,
            ..
        })
    ));
    assert_eq!(
        *coordinator.physical(),
        FakePhysical(0),
        "a dropped, unconsumed ReadyPublication must never flip the active slot"
    );
}

#[test]
fn prepare_publication_traps_when_capsule_submission_has_no_matching_complete_execution() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let (planned, _destination) = planned_fixture(&session, &coordinator.authority, true);
    let capture = matching_guest_read_capture(&planned);
    // Bind a `GuestCommittedRawDpc` without ever calling
    // `complete_execution`/`commit_zero_guest_writes` through this
    // coordinator, by hand-building the full chain against a
    // *different* session's queue -- proving `prepare_publication`
    // rejects a capsule with no corresponding ready-slot record
    // rather than silently accepting it.
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    drop(bound);

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();

    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(2));
    let capsule = session.seal_publication(committed, ready).unwrap();
    // Consume the one legitimate ready-slot record so a second
    // `prepare_publication` call has none left to match.
    let _ = coordinator.ready.take();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        coordinator.prepare_publication(capsule)
    }));
    assert!(
        outcome.is_err(),
        "prepare_publication must trap when no complete_execution recorded a ready slot"
    );
}

#[test]
fn capsule_execution_view_lends_plan_reads_and_packet_under_the_paired_authority() {
    let (mut session, authority) = new_raw_dpc_roles().unwrap();
    let mut coordinator = authority.into_coordinator(FakePhysical(0));
    let committed = guest_committed_fixture(&mut session, &mut coordinator, FakePhysical(3));

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let mut plan_visitor = RecordingVisitor::default();
    let mut view = RecordingExecutionView::default();
    capsule.execution_view(&coordinator.authority, &mut plan_visitor, &mut view);

    assert_eq!(view.plan_commands_seen, 1);
    assert_eq!(
        view.reads_seen, 1,
        "the fixture's one TmemLoadSource access"
    );
    assert!(view.packet_seen);
}

#[test]
fn commit_zero_guest_writes_commits_the_owning_session_ticket() {
    let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0xab; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let prepared = bound
        .into_backend_prepared(&mut authority, effects)
        .unwrap();

    let committed = session.commit_zero_guest_writes(prepared).unwrap();
    assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
}

#[test]
fn commit_zero_guest_writes_rejects_a_foreign_session_ticket() {
    let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();

    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0xab; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let prepared = bound
        .into_backend_prepared(&mut authority, effects)
        .unwrap();

    let (mut foreign_session, _) = new_raw_dpc_roles().unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        foreign_session.commit_zero_guest_writes(prepared)
    }));
    assert!(
        outcome.is_err(),
        "a completed ticket from a different session's queue must trap"
    );
}

/// Source-shape sweep: this seam forbids `Any`/`TypeId`/downcast/
/// `FnOnce`-callback machinery anywhere in its production types. A
/// textual grep is a blunt tool, but it is exactly the class of
/// regression review can silently miss (a new field slipping in as
/// `Box<dyn Any>` or a generic trait object standing in for a
/// concrete authority), so this fails loudly and cheaply on the next
/// such attempt instead of relying on someone noticing in diff
/// review.
#[test]
fn trait_defaults_reject_planning_execution_and_publication_by_name_or_panic() {
    struct NoOpBackend;

    impl crate::RenderBackend for NoOpBackend {
        fn create(&mut self, _cfg: &crate::RenderConfig) -> Result<(), crate::RenderError> {
            Ok(())
        }

        fn observe_non_rdp_write16(
            &mut self,
            _write: crate::NonRdpWrite16,
        ) -> crate::NonRdpWrite16Disposition {
            crate::NonRdpWrite16Disposition::NoRustHiddenSidecar
        }

        fn process_task(
            &mut self,
            _rdram: &mut [u8],
            _rsp_memory: &mut fn64_runtime::RspMemory,
            _task: &crate::OsTask,
            _output_addr: u32,
        ) -> Result<crate::FrameStatus, crate::RenderError> {
            Err(crate::RenderError::UnsupportedUcode {
                ucode_addr: fn64_runtime::RdramAddr::from_offset(0),
            })
        }

        fn present(
            &mut self,
            _request: crate::PresentRequest<'_>,
        ) -> Result<(), crate::RenderError> {
            Ok(())
        }

        fn resize(&mut self, _w: u32, _h: u32) {}

        fn supported_ucodes(&self) -> &[crate::UcodeId] {
            &[]
        }
    }

    impl crate::RawDpcBackend for NoOpBackend {}

    impl crate::SettingsSink for NoOpBackend {}

    let mut backend: Box<dyn crate::FullBackend> = Box::new(NoOpBackend);
    assert_eq!(
        backend.raw_dpc_ir_capability(),
        RawDpcIrCapability::Unsupported
    );

    let (session, _authority) = new_raw_dpc_roles().unwrap();
    let layout = PhysicalMemoryLayout::try_new(0x100).unwrap();
    let capture = OwnedRawDpcCapture::new(
        OwnedRawDpcSubmission::from_rdram_words(0, 8, vec![0, 0]).unwrap(),
        layout,
        0,
        TemporalBoundary::new(0, DpInterruptState::Clear),
    );
    let request = session.plan_request(capture);
    assert!(matches!(
        backend.plan_raw_dpc(request),
        Err(crate::RenderError::Backend {
            backend: "render/raw-dpc-plan",
            ..
        })
    ));

    let (mut real_session, real_authority) = new_raw_dpc_roles().unwrap();
    let (planned, _destination) = planned_fixture(&real_session, &real_authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = real_session.finalize_and_submit(planned, capture).unwrap();
    assert!(matches!(
        backend.execute_raw_dpc(bound),
        Err(crate::RenderError::Backend {
            backend: "render/raw-dpc-execute",
            ..
        })
    ));

    // `publish_raw_dpc` has no `Result` in its v11 signature, so its
    // default cannot report "unsupported" the way the other two do;
    // it panics instead (see the trait's doc comment). `NoOpBackend`
    // above -- a `dyn RenderBackend` implementor that overrides none
    // of the four raw-DPC methods -- already proves the trait stays
    // object-safe with `publish_raw_dpc` present (this test compiles
    // and `Box::new(NoOpBackend)` coerces to `Box<dyn FullBackend>`
    // at all). This block proves the *panicking* half of that
    // default specifically, against a fully sealed capsule built
    // through the real session chain.
    let (mut publish_session, publish_authority) = new_raw_dpc_roles().unwrap();
    let mut publish_coordinator = publish_authority.into_coordinator(FakePhysical(0));
    let committed = guest_committed_fixture(
        &mut publish_session,
        &mut publish_coordinator,
        FakePhysical(1),
    );
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = publish_session.seal_publication(committed, ready).unwrap();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        backend.publish_raw_dpc(capsule)
    }));
    assert!(
        outcome.is_err(),
        "the unoverridden publish_raw_dpc default must panic, not silently \
         fabricate a CommittedRawDpcOutcome"
    );
}

#[test]
fn production_module_source_contains_no_type_erasure_or_callback_escape_hatch() {
    let source = include_str!("mod.rs");
    let production_start = source
        .find("mod production {")
        .expect("this module's own source must contain its own `mod production` marker");
    let tests_start = source[production_start..]
        .find("#[cfg(test)]\n    mod tests {")
        .map(|offset| production_start + offset)
        .unwrap_or(source.len());
    let code_only: String = source[production_start..tests_start]
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("///") || trimmed.starts_with("//!") || trimmed.starts_with("//"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let any_path = ["std", "any", "Any"].join("::");
    let core_any_path = ["core", "any", "Any"].join("::");
    for forbidden in [
        any_path.as_str(),
        core_any_path.as_str(),
        "TypeId",
        "dyn FnOnce",
        "mem::forget",
        "ManuallyDrop",
    ] {
        assert!(
            !code_only.contains(forbidden),
            "production module code (excluding comments/tests) must not contain {forbidden:?}"
        );
    }
}

/// Proof that [`ReadyRawDpcCommitCapsule`] alone can never reach
/// `Published`/`CommittedRawDpcOutcome` -- it has no `commit` and no
/// other method returning that type -- and that
/// [`ReadyPublication::commit`] is the sole method anywhere that
/// does. A textual sweep over both `impl` blocks, not just an
/// absence check: this fails loudly if a future edit reintroduces a
/// `pub fn commit` on the capsule itself, or any other
/// `CommittedRawDpcOutcome`-returning method beside
/// `ReadyPublication::commit`.
#[test]
fn capsule_exposes_no_fabric_only_terminal_route_and_ready_publication_commit_is_the_sole_publish_path(
) {
    let source = include_str!("mod.rs");

    let capsule_impl_start = source
        .find("impl<'fabric> ReadyRawDpcCommitCapsule<'fabric> {")
        .expect("capsule impl block must exist with this exact signature");
    let capsule_impl_body = &source[capsule_impl_start..];
    let capsule_impl_end = capsule_impl_body
        .find("\n    /// Semantic terminal evidence for one published raw-DPC submission")
        .expect("capsule impl block must be immediately followed by CommittedRawDpcOutcome's doc comment");
    let capsule_impl_body = &capsule_impl_body[..capsule_impl_end];

    assert!(
        !capsule_impl_body.contains("pub fn commit("),
        "ReadyRawDpcCommitCapsule must not expose a bare public `commit`"
    );
    assert_eq!(
        capsule_impl_body
            .matches("-> CommittedRawDpcOutcome")
            .count(),
        0,
        "ReadyRawDpcCommitCapsule alone must never be able to reach \
         CommittedRawDpcOutcome -- publication requires a RawDpcCoordinator"
    );

    let publication_impl_start = source
        .find("impl<'coord, 'fabric, P> ReadyPublication<'coord, 'fabric, P> {")
        .expect("ReadyPublication impl block must exist with this exact signature");
    let publication_impl_body = &source[publication_impl_start..];
    let publication_impl_end = publication_impl_body
        .find("\n    /// Move-only, `#[must_use]` terminal capsule")
        .expect("ReadyPublication impl block must be immediately followed by ReadyRawDpcCommitCapsule's doc comment");
    let publication_impl_body = &publication_impl_body[..publication_impl_end];

    assert_eq!(
        publication_impl_body
            .matches("-> CommittedRawDpcOutcome")
            .count(),
        1,
        "exactly one method on ReadyPublication may return CommittedRawDpcOutcome"
    );
    assert!(
        publication_impl_body.contains("pub fn commit("),
        "the one CommittedRawDpcOutcome-returning method must be ReadyPublication::commit"
    );
}

/// Shared fixture for `commit_single_guest_render_target_write`'s
/// test suite: a hand-built `BackendPreparedRawDpc` whose journal
/// declares exactly one command-decode read plus one
/// `RenderTarget`/`ColorFramebuffer` write, with a matching
/// `BackendEffectReport` already issued for that write. Mirrors
/// `commit_zero_guest_writes_rejects_a_packet_with_a_real_guest_write_even_via_a_hand_built_ticket`'s
/// hand-built-ticket recipe (same-module private-field construction
/// is the only way to reach this packet shape at all, since the
/// normal `ExactRawDpcPlanWriter` path cannot build a FillRectangle
/// write yet).
fn render_target_write_fixture(
    queue: SubmissionQueue,
    mut backend_authority: BackendCompletionAuthority,
    guest: GuestCommitAuthority,
    content: fn64_render_ir::ContentDigest,
) -> (
    SubmissionQueue,
    BackendCompletionAuthority,
    GuestCommitAuthority,
    BackendPreparedRawDpc,
    ResourceAccess,
) {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let guest_write_range = layout.range(0x200, 0x210).unwrap();
    let command_read = ResourceAccess::try_new(
        fn64_render_ir::OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let guest_write = ResourceAccess::try_new(
        fn64_render_ir::OperationId::new(1),
        AccessMode::Write,
        AccessPurpose::RenderTarget,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::ColorFramebuffer,
            range: guest_write_range,
        },
    )
    .unwrap();
    let journal = fn64_render_ir::ResourceJournal::try_new(
        fn64_render_ir::ResourceJournalLimits::try_new(4, 0x100).unwrap(),
        vec![command_read, guest_write],
    )
    .unwrap();
    let stream = fn64_render_ir::RawCommandStream::Dram(
        fn64_render_ir::DramCommandStream::try_new(vec![
            fn64_render_ir::DramCommandChunk::try_new(
                command_range,
                vec![0xf500_0000, 0],
                fn64_render_ir::TemporalBoundary::new(11, fn64_render_ir::DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap(),
        ])
        .unwrap(),
    );
    let packet = fn64_render_ir::WorkloadPacket::try_new(
        layout,
        fn64_render_ir::WorkloadAdmission::RawDpc {
            transaction_sequence: 7,
        },
        vec![stream],
        journal,
    )
    .unwrap();

    let mut queue = queue;
    let submitted = queue
        .submit(fn64_render_ir::DecodedTicket::new(packet))
        .unwrap();
    let write_effect =
        CompletedWrite::try_new(guest_write, guest_write_range.len(), content).unwrap();
    let report = BackendEffectReport::try_new(submitted.packet(), vec![write_effect]).unwrap();
    let receipt = backend_authority.issue(&submitted, report).unwrap();
    let complete = submitted.gpu_complete(receipt).unwrap();

    let (dummy_retirement, _handle) = SubmittedRawDpcRetirement::new_pair(complete.submission());
    let dummy_plan = ExactValidatedRawDpcPlan {
        source_identity: crate::RawDpcSubmissionIdentity {
            source: RawDpcSource::Rdram,
            start: 0x100,
            end: 0x108,
            command_sha256: [0; 32],
        },
        journal_identity: complete.packet().journal().identity(),
        commands: Vec::new().into_boxed_slice(),
        accesses: Vec::new().into_boxed_slice(),
    };
    let prepared = BackendPreparedRawDpc {
        plan: dummy_plan,
        complete,
        retirement: dummy_retirement,
    };
    (queue, backend_authority, guest, prepared, guest_write)
}

/// A session sharing the exact queue identity `queue`/`guest` were
/// minted with (the same `TicketAuthoritySet::into_roles()` call),
/// so both `commit_single_guest_render_target_write`'s own
/// queue-ownership assertion and `GuestCommitAuthority::issue`'s own
/// authority check pass, and only the guest-commit re-check under
/// test can reject the call.
fn matching_session_for(queue: SubmissionQueue, guest: GuestCommitAuthority) -> RawDpcAbiSession {
    RawDpcAbiSession {
        queue,
        guest,
        ledger: RetirementLedger::default(),
    }
}

/// Test 1 (design card section 13): happy path -- a packet whose
/// journal declares exactly one `RenderTarget`/`ColorFramebuffer`
/// write, committed with the matching `CompletedWrite`, succeeds
/// and reaches `RawDpcRetirementStage::GuestReceipt`.
#[test]
fn commit_single_guest_render_target_write_commits_the_matching_write() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let content = fn64_render_ir::effect_content_digest(&[0xcd; 16]);
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);

    let mut session = matching_session_for(queue, guest);
    let write =
        CompletedWrite::try_new(guest_write, guest_write.region().declared_bytes(), content)
            .unwrap();
    let committed = session
        .commit_single_guest_render_target_write(prepared, write)
        .unwrap();
    assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
}

/// Test 2 (design card section 13): the *old* `commit_zero_guest_writes`
/// still rejects this same packet with `EffectCountMismatch`
/// (mirrors the pre-existing hand-built-ticket proof), while the
/// *new* method succeeds on the same packet shape -- proving the
/// two methods are each correct for their own respective packet
/// shapes, not that one silently subsumes the other.
#[test]
fn old_zero_write_method_rejects_and_new_method_accepts_the_same_render_target_packet() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let content = fn64_render_ir::effect_content_digest(&[0xce; 16]);
    let (queue, _backend_authority, guest, prepared, _guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);

    let mut zero_write_session = matching_session_for(queue, guest);
    assert_eq!(
        zero_write_session
            .commit_zero_guest_writes(prepared)
            .unwrap_err(),
        ValidationError::EffectCountMismatch {
            field: "guest commit access",
            expected: 1,
            actual: 0,
        }
    );

    // Rebuild an equivalent prepared ticket (the prior one was
    // consumed by the rejected call above) to prove the new method
    // succeeds on this exact packet shape.
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);
    let mut new_method_session = matching_session_for(queue, guest);
    let write =
        CompletedWrite::try_new(guest_write, guest_write.region().declared_bytes(), content)
            .unwrap();
    assert_eq!(
        new_method_session
            .commit_single_guest_render_target_write(prepared, write)
            .unwrap()
            .stage(),
        RawDpcRetirementStage::GuestReceipt
    );
}

/// Test 3 (design card section 13): a TMEM-only packet (zero
/// guest-visible writes declared) supplied to the *new* method with
/// one fabricated `CompletedWrite` anyway must fail with
/// `EffectCountMismatch` (expected 0, actual 1) -- the new method
/// cannot smuggle a write past a packet that never declared one.
#[test]
fn commit_single_guest_render_target_write_rejects_a_packet_declaring_zero_guest_writes() {
    let (mut session, mut authority) = new_raw_dpc_roles().unwrap();
    let (planned, destination) = planned_fixture(&session, &authority, true);
    let capture = matching_guest_read_capture(&planned);
    let bound = session.finalize_and_submit(planned, capture).unwrap();
    let write = CompletedWrite::try_new(
        destination,
        16,
        fn64_render_ir::effect_content_digest(&[0x66; 16]),
    )
    .unwrap();
    let effects = BackendEffectReport::try_new(bound.submitted.packet(), vec![write]).unwrap();
    let prepared = bound
        .into_backend_prepared(&mut authority, effects)
        .unwrap();

    // The fabricated write must itself be `Write`/`RenderTarget`-
    // shaped so it clears `commit_single_guest_render_target_write`'s
    // own structural pre-check and reaches
    // `GuestCommitEffectReport::try_new`'s count check -- proving
    // *that* check (not the pre-check) is what rejects a
    // TMEM-only packet's extra write.
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let fabricated_range = layout.range(0x400, 0x410).unwrap();
    let fabricated_access = ResourceAccess::try_new(
        fn64_render_ir::OperationId::new(9),
        AccessMode::Write,
        AccessPurpose::RenderTarget,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::ColorFramebuffer,
            range: fabricated_range,
        },
    )
    .unwrap();
    let fabricated_write = CompletedWrite::try_new(
        fabricated_access,
        fabricated_range.len(),
        fn64_render_ir::effect_content_digest(&[0x77; 16]),
    )
    .unwrap();
    assert_eq!(
        session
            .commit_single_guest_render_target_write(prepared, fabricated_write)
            .unwrap_err(),
        ValidationError::EffectCountMismatch {
            field: "guest commit access",
            expected: 0,
            actual: 1,
        }
    );
}

/// Test 4 (design card section 13): a `CompletedWrite` whose access
/// does not match the journal's declared write access (here, a
/// different `ColorTargetKey`/range entirely -- otherwise
/// plausible) is rejected with `EffectAccessMismatch`.
#[test]
fn commit_single_guest_render_target_write_rejects_a_mismatched_access() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let content = fn64_render_ir::effect_content_digest(&[0x11; 16]);
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);
    let mut session = matching_session_for(queue, guest);

    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let other_range = layout.range(0x300, 0x310).unwrap();
    let other_access = ResourceAccess::try_new(
        guest_write.operation(),
        AccessMode::Write,
        AccessPurpose::RenderTarget,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::ColorFramebuffer,
            range: other_range,
        },
    )
    .unwrap();
    let mismatched_write =
        CompletedWrite::try_new(other_access, other_range.len(), content).unwrap();
    assert_eq!(
        session
            .commit_single_guest_render_target_write(prepared, mismatched_write)
            .unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "guest commit access",
            index: 0,
        }
    );
}

/// Test 5 (design card section 13): the single most important new
/// test -- a `CompletedWrite` with the *correct* access but a
/// *different* content digest than the one the backend already
/// staged in its `BackendEffectReport` is rejected. This proves the
/// new method cannot publish bytes other than what the backend
/// actually staged.
#[test]
fn commit_single_guest_render_target_write_rejects_a_content_digest_mismatch() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let staged_content = fn64_render_ir::effect_content_digest(&[0x22; 16]);
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, staged_content);
    let mut session = matching_session_for(queue, guest);

    let different_content = fn64_render_ir::effect_content_digest(&[0x33; 16]);
    let corrupted_write = CompletedWrite::try_new(
        guest_write,
        guest_write.region().declared_bytes(),
        different_content,
    )
    .unwrap();
    assert_eq!(
        session
            .commit_single_guest_render_target_write(prepared, corrupted_write)
            .unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "guest commit effect",
            index: 0,
        }
    );
}

/// Test 6 (design card section 13): a `CompletedWrite` proven
/// against one submission's ticket must not be acceptable to a
/// *different* submission's ticket, even on the exact same session
/// queue (same-session, cross-submission -- the new case this
/// design introduces; the cross-*session* variant is already
/// covered by `commit_zero_guest_writes_rejects_a_foreign_session_ticket`).
#[test]
fn commit_single_guest_render_target_write_rejects_a_same_session_cross_submission_write() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let content_a = fn64_render_ir::effect_content_digest(&[0x44; 16]);
    let (queue, backend_authority, guest, _prepared_a, guest_write_a) =
        render_target_write_fixture(queue, backend_authority, guest, content_a);
    let write_a = CompletedWrite::try_new(
        guest_write_a,
        guest_write_a.region().declared_bytes(),
        content_a,
    )
    .unwrap();

    let content_b = fn64_render_ir::effect_content_digest(&[0x55; 16]);
    let (queue, _backend_authority, guest, prepared_b, _guest_write_b) =
        render_target_write_fixture(queue, backend_authority, guest, content_b);

    let mut session = matching_session_for(queue, guest);
    let outcome = session.commit_single_guest_render_target_write(prepared_b, write_a);
    assert!(
        outcome.is_err(),
        "a CompletedWrite proven against submission A's ticket must not be \
         accepted against submission B's ticket, even from the same session queue"
    );
}

/// `render_target_write_fixture`, widened to **three** disjoint
/// `RenderTarget`/`ColorFramebuffer` write accesses -- the shape a
/// partial-width FillRectangle produces, one access per row, whose
/// rows occupy disjoint width-strided RDRAM ranges.
///
/// The three ranges are deliberately non-contiguous (0x200, 0x240,
/// 0x280, each 0x10 long): a contiguous trio would be
/// indistinguishable from one collapsed range, which is exactly the
/// false claim the N-write design exists to prevent.
#[allow(clippy::type_complexity)]
fn three_row_render_target_write_fixture(
    queue: SubmissionQueue,
    mut backend_authority: BackendCompletionAuthority,
    guest: GuestCommitAuthority,
    contents: [fn64_render_ir::ContentDigest; 3],
) -> (
    SubmissionQueue,
    BackendCompletionAuthority,
    GuestCommitAuthority,
    BackendPreparedRawDpc,
    [ResourceAccess; 3],
) {
    let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
    let command_range = layout.range(0x100, 0x108).unwrap();
    let command_read = ResourceAccess::try_new(
        fn64_render_ir::OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::RawCommands,
            range: command_range,
        },
    )
    .unwrap();
    let row_starts = [0x200u32, 0x240, 0x280];
    let row_accesses: [ResourceAccess; 3] = core::array::from_fn(|row| {
        let start = row_starts[row];
        ResourceAccess::try_new(
            fn64_render_ir::OperationId::new(1 + row as u32),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            fn64_render_ir::ResourceRegion::Rdram {
                resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                range: layout.range(start, start + 0x10).unwrap(),
            },
        )
        .unwrap()
    });
    let journal = fn64_render_ir::ResourceJournal::try_new(
        fn64_render_ir::ResourceJournalLimits::try_new(8, 0x200).unwrap(),
        vec![
            command_read,
            row_accesses[0],
            row_accesses[1],
            row_accesses[2],
        ],
    )
    .unwrap();
    let stream = fn64_render_ir::RawCommandStream::Dram(
        fn64_render_ir::DramCommandStream::try_new(vec![
            fn64_render_ir::DramCommandChunk::try_new(
                command_range,
                vec![0xf500_0000, 0],
                fn64_render_ir::TemporalBoundary::new(11, fn64_render_ir::DpInterruptState::Clear),
                Vec::new(),
            )
            .unwrap(),
        ])
        .unwrap(),
    );
    let packet = fn64_render_ir::WorkloadPacket::try_new(
        layout,
        fn64_render_ir::WorkloadAdmission::RawDpc {
            transaction_sequence: 7,
        },
        vec![stream],
        journal,
    )
    .unwrap();

    let mut queue = queue;
    let submitted = queue
        .submit(fn64_render_ir::DecodedTicket::new(packet))
        .unwrap();
    let write_effects: Vec<CompletedWrite> = (0..3)
        .map(|row| {
            CompletedWrite::try_new(
                row_accesses[row],
                row_accesses[row].region().declared_bytes(),
                contents[row],
            )
            .unwrap()
        })
        .collect();
    let report = BackendEffectReport::try_new(submitted.packet(), write_effects).unwrap();
    let receipt = backend_authority.issue(&submitted, report).unwrap();
    let complete = submitted.gpu_complete(receipt).unwrap();

    let (dummy_retirement, _handle) = SubmittedRawDpcRetirement::new_pair(complete.submission());
    let dummy_plan = ExactValidatedRawDpcPlan {
        source_identity: crate::RawDpcSubmissionIdentity {
            source: RawDpcSource::Rdram,
            start: 0x100,
            end: 0x108,
            command_sha256: [0; 32],
        },
        journal_identity: complete.packet().journal().identity(),
        commands: Vec::new().into_boxed_slice(),
        accesses: Vec::new().into_boxed_slice(),
    };
    let prepared = BackendPreparedRawDpc {
        plan: dummy_plan,
        complete,
        retirement: dummy_retirement,
    };
    (queue, backend_authority, guest, prepared, row_accesses)
}

fn three_row_contents() -> [fn64_render_ir::ContentDigest; 3] {
    [
        fn64_render_ir::effect_content_digest(&[0xa0; 16]),
        fn64_render_ir::effect_content_digest(&[0xa1; 16]),
        fn64_render_ir::effect_content_digest(&[0xa2; 16]),
    ]
}

fn three_row_writes(
    accesses: [ResourceAccess; 3],
    contents: [fn64_render_ir::ContentDigest; 3],
) -> Vec<CompletedWrite> {
    (0..3)
        .map(|row| {
            CompletedWrite::try_new(
                accesses[row],
                accesses[row].region().declared_bytes(),
                contents[row],
            )
            .unwrap()
        })
        .collect()
}

/// T-1: the N-write proof at the IR seam. A packet declaring three
/// disjoint `RenderTarget` writes commits cleanly when all three
/// matching `CompletedWrite`s are supplied, and reaches
/// `GuestReceipt`. This is the shape a partial-width FillRectangle
/// produces and the shape the old one-write-only signature could
/// not express at all.
#[test]
fn commit_guest_render_target_writes_commits_three_row_writes() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let contents = three_row_contents();
    let (queue, _backend_authority, guest, prepared, accesses) =
        three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

    let mut session = matching_session_for(queue, guest);
    let committed = session
        .commit_guest_render_target_writes(prepared, three_row_writes(accesses, contents))
        .unwrap();
    assert_eq!(committed.stage(), RawDpcRetirementStage::GuestReceipt);
}

/// T-2: dropping one row is a loud count mismatch, not a partial
/// commit. A committed subset would silently claim the packet wrote
/// less than its own journal declares.
#[test]
fn commit_guest_render_target_writes_rejects_a_dropped_row() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let contents = three_row_contents();
    let (queue, _backend_authority, guest, prepared, accesses) =
        three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

    let mut session = matching_session_for(queue, guest);
    let mut writes = three_row_writes(accesses, contents);
    writes.pop();
    assert_eq!(
        session
            .commit_guest_render_target_writes(prepared, writes)
            .unwrap_err(),
        ValidationError::EffectCountMismatch {
            field: "guest commit access",
            expected: 3,
            actual: 2,
        }
    );
}

/// T-3: supplying all three rows in the wrong order is rejected --
/// proving journal order is load-bearing, not incidental. Rows 0/2/1
/// carry the right accesses and the right digests; only their
/// sequence differs.
#[test]
fn commit_guest_render_target_writes_rejects_reordered_rows() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let contents = three_row_contents();
    let (queue, _backend_authority, guest, prepared, accesses) =
        three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

    let mut session = matching_session_for(queue, guest);
    let writes = three_row_writes(accesses, contents);
    let reordered = vec![writes[0], writes[2], writes[1]];
    assert_eq!(
        session
            .commit_guest_render_target_writes(prepared, reordered)
            .unwrap_err(),
        ValidationError::EffectAccessMismatch {
            field: "guest commit access",
            index: 1,
        }
    );
}

/// T-4: the one-write convenience wrapper and a one-element N-write
/// call are the same operation. Asserted on the sealed
/// `guest_effect_identity()`, so a future divergence in either
/// body -- an extra check, a different report shape -- fails here
/// rather than silently producing two different receipts for the
/// same staged write.
#[test]
fn commit_single_guest_render_target_write_still_delegates() {
    let content = fn64_render_ir::effect_content_digest(&[0xbe; 16]);

    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);
    let write =
        CompletedWrite::try_new(guest_write, guest_write.region().declared_bytes(), content)
            .unwrap();
    let mut single_session = matching_session_for(queue, guest);
    let via_single = single_session
        .commit_single_guest_render_target_write(prepared, write)
        .unwrap();

    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);
    let write =
        CompletedWrite::try_new(guest_write, guest_write.region().declared_bytes(), content)
            .unwrap();
    let mut plural_session = matching_session_for(queue, guest);
    let via_plural = plural_session
        .commit_guest_render_target_writes(prepared, vec![write])
        .unwrap();

    assert_eq!(
        via_single.committed.guest_effect_identity(),
        via_plural.committed.guest_effect_identity(),
        "the single-write wrapper and a one-element N-write call must produce the \
         identical sealed guest-effect identity"
    );
}

/// Hostile: a non-`RenderTarget` access anywhere in the list --
/// not merely at index 0 -- is rejected by the shape pre-check. The
/// old single-write method could only ever check one element, so
/// "every element is checked" is a genuinely new obligation the
/// N-write body has to meet.
#[test]
fn commit_guest_render_target_writes_rejects_a_non_render_target_write_in_any_position() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let contents = three_row_contents();
    let (queue, _backend_authority, guest, prepared, accesses) =
        three_row_render_target_write_fixture(queue, backend_authority, guest, contents);

    let mut session = matching_session_for(queue, guest);
    let mut writes = three_row_writes(accesses, contents);
    // Rewrite the LAST element's purpose, so a body that only
    // inspected `writes[0]` would wrongly accept this call.
    // `CopyDestination` is chosen because it is a genuinely
    // constructible write purpose for a `ColorFramebuffer` region
    // (unlike, say, `TmemLoadDestination`, which `ResourceAccess::
    // try_new` refuses outright) -- so this test exercises the
    // session's own shape pre-check, not the journal's constructor.
    let wrong_purpose = ResourceAccess::try_new(
        accesses[2].operation(),
        AccessMode::Write,
        AccessPurpose::CopyDestination,
        accesses[2].region(),
    )
    .unwrap();
    writes[2] = CompletedWrite::try_new(
        wrong_purpose,
        wrong_purpose.region().declared_bytes(),
        contents[2],
    )
    .unwrap();
    assert_eq!(
        session
            .commit_guest_render_target_writes(prepared, writes)
            .unwrap_err(),
        ValidationError::GuestRenderTargetWriteShapeMismatch {
            mode: "Write",
            purpose: "CopyDestination",
        }
    );
}

/// Test 7 (design card section 13): a `CompletedWrite` whose access
/// mode/purpose does not match the single admitted shape (`Write`,
/// `RenderTarget`) is rejected by the method's own structural
/// pre-check before `GuestCommitEffectReport::try_new` is ever
/// called -- assert the specific error, not merely "some `Err`".
#[test]
fn commit_single_guest_render_target_write_rejects_wrong_mode_or_purpose_at_the_precheck() {
    let (queue, backend_authority, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let content = fn64_render_ir::effect_content_digest(&[0x88; 16]);
    let (queue, _backend_authority, guest, prepared, guest_write) =
        render_target_write_fixture(queue, backend_authority, guest, content);
    let mut session = matching_session_for(queue, guest);

    let tmem_destination_range = fn64_render_ir::TmemRange::try_new(0, 16).unwrap();
    let wrong_purpose_access = ResourceAccess::try_new(
        guest_write.operation(),
        AccessMode::Write,
        AccessPurpose::TmemLoadDestination,
        fn64_render_ir::ResourceRegion::Tmem(tmem_destination_range),
    )
    .unwrap();
    let wrong_purpose_write =
        CompletedWrite::try_new(wrong_purpose_access, tmem_destination_range.len(), content)
            .unwrap();
    assert_eq!(
        session
            .commit_single_guest_render_target_write(prepared, wrong_purpose_write)
            .unwrap_err(),
        ValidationError::GuestRenderTargetWriteShapeMismatch {
            mode: "Write",
            purpose: "TmemLoadDestination",
        }
    );
}
