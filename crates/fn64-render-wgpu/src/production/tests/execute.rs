//! Execution-side unit tests: `execute_raw_dpc`, fills, ordered task
//! batches, resize, staged guest writes, composition and the durable
//! tile/other-mode carry-in snapshots.

use super::*;

#[cfg(feature = "host-gpu-tests")]
#[test]
fn exact_cpu_publication_exposes_one_submission_keyed_visual_snapshot() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&test_render_config()) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!("required host GPU evidence unavailable: {no_adapter:?}")
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    let (_, submission) = publish_one_fill_with_submission(
        &mut backend,
        &mut session,
        whole_target_cpu_triangle_words(),
    );

    let snapshot = backend
        .take_raw_dpc_visual_target_snapshot(submission)
        .expect("a complete CPU color publication has exact visible and coverage state");
    assert_eq!(snapshot.submission(), submission);
    assert_eq!(snapshot.target_address(), FILL_TARGET_ADDRESS);
    assert_eq!(snapshot.target_width(), FILL_TARGET_WIDTH);
    assert_eq!(snapshot.target_height(), 8);
    assert_eq!(
        snapshot.target_format(),
        fn64_render::RawDpcVisualTargetFormatV1::Rgba16
    );
    assert!(!snapshot.target_device_bytes().is_empty());
    assert!(snapshot
        .coverage()
        .iter()
        .all(|coverage| (1..=8).contains(coverage)));
    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(submission),
        Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget),
        "the evidence capability is consuming, so stale target state cannot be reused"
    );
}

#[test]
fn visual_snapshot_submission_mismatch_consumes_the_stale_marker() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, submission) =
        publish_one_fill_with_submission(&mut backend, &mut session, whole_target_fill_words());
    let wrong = planned_submission_identity(partial_width_fill_words());
    assert_ne!(
        wrong, submission,
        "the mismatch fixture needs another identity"
    );

    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(wrong),
        Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::SubmissionMismatch)
    );
    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(submission),
        Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget),
        "a mismatched request cannot leave evidence available to a later caller"
    );
}

#[test]
fn visual_snapshot_refuses_unknown_fill_coverage_by_name() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (_, submission) =
        publish_one_fill_with_submission(&mut backend, &mut session, whole_target_fill_words());

    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(submission),
        Err(
            fn64_render::RawDpcVisualTargetSnapshotRefusal::CoverageUnavailable {
                unknown_cells: (FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT) as usize,
            }
        )
    );
}

#[test]
fn visual_snapshot_refuses_compute_coverage_and_colorless_publications_by_name() {
    let (mut backend, _) = WgpuBackend::try_new().unwrap();
    let submission = planned_submission_identity(whole_target_fill_words());
    backend.last_published_visual_target = Some((
        submission,
        PublishedVisualTargetMarker::ComputeCoverageUnavailable,
    ));
    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(submission),
        Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::ComputeCoverageUnavailable)
    );

    backend.last_published_visual_target =
        Some((submission, PublishedVisualTargetMarker::NoColorTarget));
    assert_eq!(
        backend.take_raw_dpc_visual_target_snapshot(submission),
        Err(fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget)
    );
}

/// **T-7 -- the headline admission test.** A partial-width, three-row
/// fill plans, executes, and reports exactly three staged guest writes,
/// with publication genuinely deferred.
///
/// Every assertion here is one the pre-admission code could not have
/// satisfied: `plan_raw_dpc` used to reject `FillRectangle` outright
/// with `UnadmittedRawDpcCommand`, the journal used to declare zero
/// `RenderTarget` writes, and no staged-write transport existed at all.
///
/// The deferral assertion is the load-bearing one: it proves the
/// deferred-token design actually defers. If `execute_raw_dpc` published
/// eagerly, the guest commit that must precede publication would be
/// running *after* the registry already advanced.
///
/// The whole-target fill that runs first is not incidental setup: a
/// partial rectangle cannot initialize a *fresh* target at all
/// (`PartialNewTargetInitialization` -- the untouched rows would be
/// fabricated zeros), so establishing a real generation 1 is the only
/// honest way to reach the partial-fill path.
#[test]
fn execute_raw_dpc_admits_a_partial_width_fill_end_to_end() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    assert_eq!(
        declared_render_target_writes(partial_width_fill_words()).len(),
        3,
        "a partial-width 11x3 fill declares one RenderTarget write access PER ROW -- a \
         single collapsed range would claim untouched inter-row bytes as written"
    );

    // Establish a real resident generation the partial fill can patch.
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    let generation_before = backend.color_targets().unwrap().residents()[0]
        .generation()
        .get();

    let request = session.plan_request(capture(partial_width_fill_words()));
    let planned = backend.plan_raw_dpc(request).expect(
        "an admitted partial-width fill must plan cleanly, not be rejected as an \
         UnadmittedRawDpcCommand",
    );

    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("an admitted fill must execute cleanly");

    let staged = backend.staged_guest_render_target_writes(submission);
    assert_eq!(
        staged.len(),
        3,
        "the backend must transport exactly the three CompletedWrites its journal declares"
    );
    // 11 pixels x 2 bytes per RGBA16 pixel = 22 bytes per row.
    for (row, write) in staged.iter().enumerate() {
        assert_eq!(
            write.byte_count(),
            22,
            "row {row}'s write must cover only its own 22 bytes"
        );
    }
    assert_eq!(
        staged.iter().map(|write| write.byte_count()).sum::<u32>(),
        66,
        "the three rows total 66 real bytes, never the 86 a collapsed range would span"
    );

    let registry = backend
        .color_targets()
        .expect("the first admitted fill builds the registry");
    assert_eq!(
        registry.residents()[0].generation().get(),
        generation_before,
        "publication must be deferred until publish_raw_dpc -- an advanced generation here \
         would mean the registry moved before the guest commit that must precede it"
    );
    assert!(
        backend.has_pending_fill_publication(),
        "the staged fill is held as a submission-keyed token, not published"
    );

    drop(prepared);
}

#[test]
fn task_batch_privately_chains_a_whole_target_then_partial_fill_before_ordered_publication() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    assert_eq!(
        backend.raw_dpc_task_batch_capability(),
        RawDpcTaskBatchCapability::Transactional
    );

    let requests = vec![
        session.plan_request(capture(whole_target_fill_words())),
        session.plan_request(capture(partial_width_fill_words())),
    ];
    let planned = backend.plan_raw_dpc_task_batch(requests).unwrap();
    let mut bounds = Vec::new();
    let mut submissions = Vec::new();
    for member in planned {
        let bound = finalize_and_submit_pair(&mut session, member).unwrap();
        submissions.push(bound.submission());
        bounds.push(bound);
    }
    task_compute_census::begin_enabled_segment_test();
    let prepared = backend.execute_raw_dpc_task_batch(bounds).unwrap();
    let programs = task_compute_census::finish_enabled_segment_test();
    assert!(
        programs.is_empty(),
        "fill-only task members must not be attributed to a compute program"
    );
    assert_eq!(prepared.len(), 2);
    assert_eq!(
        backend.take_raw_dpc_task_batch_execution_mechanism(),
        fn64_render::RawDpcTaskBatchExecutionMechanism::try_new(2, 0)
    );
    assert_eq!(
        backend.take_raw_dpc_task_batch_execution_mechanism(),
        None,
        "execution evidence is move-once"
    );
    assert!(
        backend.color_targets().unwrap().residents().is_empty(),
        "private color successors must not become durable during batch execution"
    );
    assert_eq!(
        backend
            .staged_guest_render_target_writes(submissions[0])
            .len(),
        1
    );
    assert_eq!(
        backend
            .staged_guest_render_target_writes(submissions[1])
            .len(),
        3,
        "the partial member must seed from the private whole-target successor"
    );

    for (index, member) in prepared.into_iter().enumerate() {
        let writes = backend.staged_guest_render_target_writes(submissions[index]);
        let committed = session
            .commit_guest_render_target_writes(member, writes)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        assert_eq!(
            backend.color_targets().unwrap().residents()[0]
                .generation()
                .get(),
            u64::try_from(index + 1).unwrap()
        );
    }
    assert!(!backend.has_pending_fill_publication());
}

#[test]
fn rejected_task_batch_plan_installs_neither_state_nor_pending_members() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let before = backend.rdp_state.fork_for_decode();

    let mut rejected = partial_width_fill_words();
    rejected.extend([word(FULL_SYNC, 0), 0]);
    let requests = vec![
        session.plan_request(capture(set_other_mode(0, 0x40).to_vec())),
        session.plan_request(capture(rejected)),
    ];
    assert!(backend.plan_raw_dpc_task_batch(requests).is_err());
    assert_eq!(backend.rdp_state, before);
    assert!(backend.pending_raw_dpc_task_batch.is_none());
}

#[test]
fn checkpoint_images_close_cardinality_before_ordered_redemption() {
    for (images, expected, actual) in [(vec![vec![1]], 2, 1), (vec![vec![1], vec![2]], 1, 2)] {
        match ExactCheckpointImages::try_new(images, expected) {
            Err(WgpuRawDpcExecutionError::ComputeRasterCheckpointCount {
                expected: rejected_expected,
                actual: rejected_actual,
            }) => assert_eq!((rejected_expected, rejected_actual), (expected, actual)),
            Err(other) => panic!("unexpected checkpoint cardinality error: {other}"),
            Ok(_) => panic!("mismatched checkpoint cardinality was accepted"),
        }
    }

    let mut exact = ExactCheckpointImages::try_new(vec![vec![1], vec![2]], 2).unwrap();
    assert_eq!(exact.take_next(), vec![1]);
    assert_eq!(exact.take_next(), vec![2]);
    exact.finish();
}

#[test]
fn task_planning_reason_codes_non_triangle_and_mixed_color_cpu_members() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let planned = backend
        .plan_raw_dpc_task_batch(vec![
            session.plan_request(capture(whole_target_fill_words())),
            session.plan_request(capture(fill_then_triangle_words())),
        ])
        .unwrap();
    assert_eq!(planned.len(), 2);
    let pending = backend.pending_raw_dpc_task_batch.as_ref().unwrap();
    assert_eq!(
        pending.members[0].execution,
        PlannedTaskExecution::Cpu(PlannedTaskCpuReason::NoRawTriangle(
            PlannedNoRawTriangleReason::FillOnly,
        ))
    );
    assert_eq!(
        pending.members[1].execution,
        PlannedTaskExecution::Cpu(PlannedTaskCpuReason::MixedFillOrTexrect)
    );
}

#[test]
fn no_triangle_reason_partition_is_exhaustive_and_non_overlapping() {
    let expected = [
        PlannedNoRawTriangleReason::NoOpOnly,
        PlannedNoRawTriangleReason::SyncStateOnly,
        PlannedNoRawTriangleReason::TmemLoadOnly,
        PlannedNoRawTriangleReason::TexrectOnly,
        PlannedNoRawTriangleReason::TexrectAndTmemLoad,
        PlannedNoRawTriangleReason::FillOnly,
        PlannedNoRawTriangleReason::FillAndTmemLoad,
        PlannedNoRawTriangleReason::FillAndTexrect,
        PlannedNoRawTriangleReason::FillTexrectAndTmemLoad,
    ];
    let actual = [
        classify_no_raw_triangle_flags(false, false, false, false),
        classify_no_raw_triangle_flags(false, false, false, true),
        classify_no_raw_triangle_flags(false, false, true, false),
        classify_no_raw_triangle_flags(false, true, false, false),
        classify_no_raw_triangle_flags(false, true, true, false),
        classify_no_raw_triangle_flags(true, false, false, false),
        classify_no_raw_triangle_flags(true, false, true, false),
        classify_no_raw_triangle_flags(true, true, false, false),
        classify_no_raw_triangle_flags(true, true, true, false),
    ];
    assert_eq!(actual, expected);
    let distinct = actual
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), expected.len());
}

#[test]
fn compute_program_attribution_closes_draws_and_segment_members() {
    assert_eq!(
        compute_program_attribution_from_ids([2, 2, 2]),
        ComputeProgramAttribution::Program(2),
    );
    assert_eq!(
        compute_program_attribution_from_ids([0, 2]),
        ComputeProgramAttribution::MixedPrograms,
    );
    assert_eq!(
        compute_program_attribution_from_members([
            ComputeProgramAttribution::Program(2),
            ComputeProgramAttribution::Program(2),
        ]),
        ComputeProgramAttribution::Program(2),
    );
    assert_eq!(
        compute_program_attribution_from_members([
            ComputeProgramAttribution::Program(2),
            ComputeProgramAttribution::MixedPrograms,
        ]),
        ComputeProgramAttribution::MixedPrograms,
    );
    assert_eq!(
        compute_program_attribution_from_members([
            ComputeProgramAttribution::Program(0),
            ComputeProgramAttribution::Program(2),
        ]),
        ComputeProgramAttribution::MixedPrograms,
    );
}

#[test]
fn task_planning_routes_two_coverage_fog_refusals_and_one_admitted_program_exactly() {
    const COVERAGE_FOG_COMBINE_LOW: u32 = 0xfc15_fea3;
    const COVERAGE_FOG_COMBINE_HIGH: u32 = 0xf00f_f23f;
    const COVERAGE_FOG_OTHER_MODE_LOW: u32 = 0x0f0a_7008;
    const HOT_COMBINE_LOW: u32 = 0xfc51_96a3;
    const HOT_COMBINE_HIGH: u32 = 0x112c_fe7f;
    const HOT_OTHER_MODE_HIGH: u32 = 0x0008_acef;
    const HOT_OTHER_MODE_LOW: u32 = 0x0050_41c8;

    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let planned = backend
        .plan_raw_dpc_task_batch(vec![
            session.plan_request(capture(textured_triangle_packet_words(
                COVERAGE_FOG_COMBINE_LOW,
                COVERAGE_FOG_COMBINE_HIGH,
                0x0018_ac8f,
                COVERAGE_FOG_OTHER_MODE_LOW,
            ))),
            session.plan_request(capture(textured_triangle_packet_words(
                COVERAGE_FOG_COMBINE_LOW,
                COVERAGE_FOG_COMBINE_HIGH,
                0x0018_acff,
                COVERAGE_FOG_OTHER_MODE_LOW,
            ))),
            session.plan_request(capture(textured_triangle_packet_words(
                HOT_COMBINE_LOW,
                HOT_COMBINE_HIGH,
                HOT_OTHER_MODE_HIGH,
                HOT_OTHER_MODE_LOW,
            ))),
        ])
        .unwrap();
    assert_eq!(planned.len(), 3);
    let pending = backend.pending_raw_dpc_task_batch.as_ref().unwrap();
    let executions = pending
        .members
        .iter()
        .map(|member| member.execution)
        .collect::<Vec<_>>();
    assert_eq!(
        executions,
        vec![
            PlannedTaskExecution::Cpu(PlannedTaskCpuReason::DefinitelyCpu(
                TaskComputeAdmissionRefusal::CycleType([
                    COVERAGE_FOG_COMBINE_LOW,
                    COVERAGE_FOG_COMBINE_HIGH,
                    0x0018_ac8f,
                    COVERAGE_FOG_OTHER_MODE_LOW,
                ]),
            )),
            PlannedTaskExecution::Cpu(PlannedTaskCpuReason::DefinitelyCpu(
                TaskComputeAdmissionRefusal::CycleType([
                    COVERAGE_FOG_COMBINE_LOW,
                    COVERAGE_FOG_COMBINE_HIGH,
                    0x0018_acff,
                    COVERAGE_FOG_OTHER_MODE_LOW,
                ]),
            )),
            PlannedTaskExecution::ComputeCandidate,
        ],
    );
}

#[test]
fn task_compute_routes_an_exactly_unadmitted_raw_triangle_through_cpu() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    backend.set_task_compute_raster_enabled(true);
    backend.set_task_cpu_color_batch_enabled(true);
    backend.probes.gpu_triangle_draw_enabled = false;
    backend.probes.project_gpu_tmem = false;

    // The flat primitive-color program is a raw-triangle-only planning
    // candidate, but it is intentionally outside the first compute
    // kernel's exact hottest-state key. Restoring the old coarse-bool
    // behavior makes execution fail here instead of producing CPU bytes.
    let planned = backend
        .plan_raw_dpc_task_batch(vec![
            session.plan_request(capture(flat_triangle_packet_words()))
        ])
        .unwrap();
    assert_eq!(
        backend.pending_raw_dpc_task_batch.as_ref().unwrap().members[0].execution,
        PlannedTaskExecution::Cpu(PlannedTaskCpuReason::DefinitelyCpu(
            TaskComputeAdmissionRefusal::Untextured,
        )),
    );
    let bound =
        finalize_and_submit_pair(&mut session, planned.into_iter().next().unwrap()).unwrap();
    let submission = bound.submission();
    let mut prepared = backend.execute_raw_dpc_task_batch(vec![bound]).unwrap();
    assert_eq!(prepared.len(), 1);
    let writes = backend.staged_guest_render_target_writes(submission);
    assert_eq!(
        writes.len(),
        3,
        "the explicit CPU disposition must retain all three declared triangle rows"
    );
    let committed = session
        .commit_guest_render_target_writes(prepared.pop().unwrap(), writes)
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);
    assert_eq!(
        backend.color_targets().unwrap().residents()[0]
            .generation()
            .get(),
        2,
        "a terminal member's pending image must publish without a private shadow copy"
    );
}

#[test]
fn ordered_cpu_task_batch_publishes_each_sparse_triangle_generation_and_bytes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    backend.set_task_compute_raster_enabled(true);
    backend.probes.gpu_triangle_draw_enabled = false;
    backend.probes.project_gpu_tmem = false;

    let planned = backend
        .plan_raw_dpc_task_batch(vec![
            session.plan_request(capture(flat_triangle_packet_words())),
            session.plan_request(capture(flat_triangle_packet_words())),
        ])
        .unwrap();
    let mut bounds = Vec::new();
    let mut submissions = Vec::new();
    for member in planned {
        let bound = finalize_and_submit_pair(&mut session, member).unwrap();
        submissions.push(bound.submission());
        bounds.push(bound);
    }
    let prepared = backend.execute_raw_dpc_task_batch(bounds).unwrap();
    assert_eq!(prepared.len(), 2);
    assert_eq!(
        backend.color_targets().unwrap().residents()[0]
            .generation()
            .get(),
        1,
        "terminal ordered CPU accumulator stays private until publication"
    );

    let resident = &backend.color_targets().unwrap().residents()[0];
    let key = resident.key();
    let mut copied = resident.device_bytes().device_bytes().to_vec();
    for (index, member) in prepared.into_iter().enumerate() {
        let writes = backend.staged_guest_render_target_writes(submissions[index]);
        let payloads = backend.committed_guest_render_target_bytes(submissions[index]);
        assert_eq!((writes.len(), payloads.len()), (3, 3));
        for (write, payload) in writes.iter().zip(&payloads) {
            assert_eq!(
                CompletedWrite::try_from_bytes(write.access(), payload).unwrap(),
                *write,
                "sparse payload remains bound to its exact operation and digest"
            );
            let fn64_render_ir::ResourceRegion::Rdram { range, .. } = write.access().region()
            else {
                panic!("triangle color write must name RDRAM")
            };
            let start = (range.start().get() - key.address().get()) as usize;
            copied[start..start + payload.len()].copy_from_slice(payload);
        }
        let committed = session
            .commit_guest_render_target_writes(member, writes)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        let resident = &backend.color_targets().unwrap().residents()[0];
        assert_eq!(
            resident.generation().get(),
            u64::try_from(index + 2).unwrap()
        );
        assert_eq!(resident.device_bytes().device_bytes(), copied);
    }
}

#[test]
fn task_compute_routes_a_non_deferred_triangle_completion_through_cpu() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    backend.set_task_compute_raster_enabled(true);
    backend.probes.gpu_triangle_draw_enabled = false;
    backend.probes.project_gpu_tmem = false;

    let planned = backend
        .plan_raw_dpc_task_batch(vec![session.plan_request(capture(triangle_only_words()))])
        .unwrap();
    let bound =
        finalize_and_submit_pair(&mut session, planned.into_iter().next().unwrap()).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc_task_batch(vec![bound]).unwrap();
    assert_eq!(prepared.len(), 1);
    assert!(
        backend
            .staged_guest_render_target_writes(submission)
            .is_empty(),
        "a non-write-declaring triangle must keep its ordinary CPU completion shape"
    );
}

#[cfg(feature = "host-gpu-tests")]
#[test]
fn task_compute_batches_two_raw_triangle_packets_and_publishes_each_generation() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    }) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!("required host GPU evidence unavailable: {no_adapter:?}")
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    load_and_publish_full_tmem_fixture(&mut backend, &mut session);
    let _ = cpu_oracle_sample(backend.physical_tmem(), 16, 16);
    backend.set_task_compute_raster_enabled(true);

    let requests = vec![
        session.plan_request(capture(hot_textured_triangle_packet_words())),
        session.plan_request(capture(hot_textured_triangle_packet_words())),
    ];
    let planned = backend.plan_raw_dpc_task_batch(requests).unwrap();
    let mut bounds = Vec::new();
    let mut submissions = Vec::new();
    for member in planned {
        let bound = finalize_and_submit_pair(&mut session, member).unwrap();
        submissions.push(bound.submission());
        bounds.push(bound);
    }
    let prepared = backend.execute_raw_dpc_task_batch(bounds).unwrap();
    assert_eq!(prepared.len(), 2);
    assert_eq!(
        backend.color_targets().unwrap().residents()[0]
            .generation()
            .get(),
        1,
        "device checkpoints remain private until ordered publication"
    );

    for (index, member) in prepared.into_iter().enumerate() {
        let writes = backend.staged_guest_render_target_writes(submissions[index]);
        assert_eq!(writes.len(), 3);
        let committed = session
            .commit_guest_render_target_writes(member, writes)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        assert_eq!(
            backend.color_targets().unwrap().residents()[0]
                .generation()
                .get(),
            u64::try_from(index + 2).unwrap()
        );
    }

    assert_eq!(
        backend.color_targets().unwrap().residents()[0]
            .generation()
            .get(),
        3,
        "both private GPU checkpoints must publish in task order"
    );
}

#[cfg(feature = "host-gpu-tests")]
#[test]
fn task_compute_keeps_compute_cpu_compute_members_in_generation_order() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    match backend.create_inner(&fn64_render::RenderConfig {
        width: FILL_TARGET_WIDTH,
        height: FILL_TARGET_HEIGHT,
        tv_type: fn64_runtime::TvType::default(),
    }) {
        Ok(()) => {}
        Err(WgpuCreateError::NoAdapter(no_adapter)) => {
            panic!("required host GPU evidence unavailable: {no_adapter:?}")
        }
        Err(other) => panic!("create() failed for an unexpected reason: {other}"),
    }
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    load_and_publish_full_tmem_fixture(&mut backend, &mut session);
    backend.set_task_compute_raster_enabled(true);

    let requests = vec![
        session.plan_request(capture(hot_textured_triangle_packet_words())),
        session.plan_request(capture(flat_triangle_packet_words())),
        session.plan_request(capture(hot_textured_triangle_packet_words())),
    ];
    let planned = backend.plan_raw_dpc_task_batch(requests).unwrap();
    let mut bounds = Vec::new();
    let mut submissions = Vec::new();
    for member in planned {
        let bound = finalize_and_submit_pair(&mut session, member).unwrap();
        submissions.push(bound.submission());
        bounds.push(bound);
    }

    let prepared = backend.execute_raw_dpc_task_batch(bounds).unwrap();
    assert_eq!(prepared.len(), 3);
    for (index, member) in prepared.into_iter().enumerate() {
        let writes = backend.staged_guest_render_target_writes(submissions[index]);
        assert_eq!(writes.len(), 3);
        let committed = session
            .commit_guest_render_target_writes(member, writes)
            .unwrap();
        let mut fabric = admitted_fabric();
        let token = fabric.pending_dpc_submission().unwrap().token;
        let ready = fabric.prepare_dpc_commit(token).unwrap();
        let capsule = session.seal_publication(committed, ready).unwrap();
        backend.publish_raw_dpc(capsule);
        assert_eq!(
            backend.color_targets().unwrap().residents()[0]
                .generation()
                .get(),
            u64::try_from(index + 2).unwrap(),
            "compute/CPU boundaries must not reorder task color generations"
        );
    }
}

/// **T-6:** the full-width branch stays in lockstep with the per-row
/// branch. Same target and same three rows, but `x0 == 0 && x1 + 1 ==
/// width`, so `plan_fill` collapses to exactly one access -- which is
/// legitimate here precisely because a full-width run IS contiguous.
#[test]
fn execute_raw_dpc_collapses_a_full_width_fill_to_one_write() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    assert_eq!(
        declared_render_target_writes(full_width_fill_words()).len(),
        1,
        "a full-width fill's rows ARE contiguous, so one access is the honest declaration"
    );

    // Full-width but only three rows tall, so it is still a partial
    // rectangle for admission purposes -- establish a real generation
    // first, as `PartialNewTargetInitialization` requires.
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let request = session.plan_request(capture(full_width_fill_words()));
    let planned = backend.plan_raw_dpc(request).unwrap();

    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).unwrap();

    let staged = backend.staged_guest_render_target_writes(submission);
    assert_eq!(staged.len(), 1);
    assert_eq!(
        staged[0].byte_count(),
        3 * FILL_TARGET_WIDTH * 2,
        "one access covering three full 16-pixel RGBA16 rows is 96 bytes"
    );

    drop(prepared);
}

/// **T-5:** each row's content digest covers exactly its own 22 bytes,
/// sliced from the full-extent device buffer -- never a digest over the
/// whole 256-byte target, and never over the 86-byte span the three rows
/// collectively occupy.
///
/// Recomputed independently here from `effect_content_digest` over the
/// resident's own published bytes, so a change that started hashing a
/// wider slice would fail rather than merely producing a different
/// opaque value.
#[test]
fn each_fill_row_write_hashes_only_its_own_bytes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let row_ranges = declared_render_target_writes(partial_width_fill_words());
    assert_eq!(row_ranges.len(), 3);
    // The three rows are strided by the image's own row pitch (16
    // pixels x 2 bytes), not packed end to end -- which is exactly why
    // they cannot be collapsed.
    assert_eq!(row_ranges[1].0 - row_ranges[0].0, FILL_TARGET_WIDTH * 2);
    assert_eq!(row_ranges[2].0 - row_ranges[1].0, FILL_TARGET_WIDTH * 2);

    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    // Publish the partial fill so the full-extent device bytes it
    // produced are readable, then verify every staged digest against a
    // slice recomputed from them.
    let staged = publish_one_fill(&mut backend, &mut session, partial_width_fill_words());
    assert_eq!(staged.len(), 3);

    let registry = backend.color_targets().unwrap();
    let resident = &registry.residents()[0];
    let buffer = resident.device_bytes().device_bytes();
    assert_eq!(
        buffer.len() as u32,
        FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
        "the resident's device bytes cover the whole target, unlike any single write"
    );

    let base = resident.key().address().get();
    for (row, (start, len)) in row_ranges.iter().enumerate() {
        let offset = (start - base) as usize;
        let slice = &buffer[offset..offset + *len as usize];
        assert_eq!(
            staged[row].content(),
            fn64_render_ir::effect_content_digest(slice),
            "row {row}'s digest must cover exactly its own {len} bytes"
        );
        assert_ne!(
            staged[row].content(),
            fn64_render_ir::effect_content_digest(buffer),
            "row {row}'s digest must NOT be a digest of the whole target buffer"
        );
    }
}

/// **T-10:** each full plan -> execute -> commit -> seal -> publish
/// cycle advances the resident generation by exactly one -- proving
/// publication is neither skipped nor doubled.
///
/// Generation 1 is the whole-target fill (the only rectangle a fresh
/// target admits); generations 2 and 3 are partial fills patching into
/// it.
#[test]
fn publish_raw_dpc_advances_the_resident_generation_exactly_once() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    assert_eq!(
        backend.color_targets().unwrap().residents()[0]
            .generation()
            .get(),
        1
    );

    for expected_generation in 2..=3u64 {
        let staged = publish_one_fill(&mut backend, &mut session, partial_width_fill_words());
        assert_eq!(staged.len(), 3);

        let registry = backend.color_targets().unwrap();
        assert_eq!(
            registry.residents().len(),
            1,
            "every fill targets the same color image, so there is exactly one resident"
        );
        assert_eq!(
            registry.residents()[0].generation().get(),
            expected_generation,
            "each published fill advances the resident generation by exactly one"
        );
        assert!(
            !backend.has_pending_fill_publication(),
            "the token must be consumed by publication, never left behind"
        );
    }
}

/// **T-9 -- the nonmutation test.** A fill rejected at *execution* time,
/// after `begin_candidate` has already succeeded, must leave the
/// registry byte-identical and leave no staged token behind.
///
/// `Z_CMP` (`OtherMode.low & 0x0010`) is the deliberate lever: it passes
/// every plan-time gate (`plan_fill` checks cycle type, not the
/// Z/framebuffer hazard bits) and is rejected by
/// `require_safe_fill_cycle_bypass` inside `execute_fill_rectangle` --
/// i.e. precisely inside the window the deferred-token design creates,
/// after a candidate exists and before anything is published.
#[test]
fn a_rejected_fill_leaves_the_registry_and_physical_slot_untouched() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // First, establish a real resident generation to be preserved.
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let snapshot_generation = backend.color_targets().unwrap().residents()[0]
        .generation()
        .get();
    let snapshot_bytes = backend.color_targets().unwrap().residents()[0]
        .device_bytes()
        .device_bytes()
        .to_vec();
    let snapshot_physical = backend.physical_tmem().identity();
    assert_eq!(snapshot_generation, 1);

    // Now a second capture whose fill is rejected at execution time.
    let mut hostile = Vec::new();
    hostile.extend(fill_cycle_other_mode(0x0010)); // Z_CMP set
    hostile.extend(set_color_image_rgba16());
    hostile.extend(set_fill_color(0x213c_4d59));
    hostile.extend(fill_rectangle(4, 2, 14, 4));

    let (_, result) = plan_and_execute_fill(&mut backend, &mut session, hostile);
    assert!(
        result.is_err(),
        "a Z_CMP fill-cycle bypass must be rejected loudly at execution, never executed"
    );

    let registry = backend.color_targets().unwrap();
    assert_eq!(registry.residents().len(), 1);
    assert_eq!(
        registry.residents()[0].generation().get(),
        snapshot_generation,
        "a rejected fill must not advance the resident generation"
    );
    assert_eq!(
        registry.residents()[0].device_bytes().device_bytes(),
        snapshot_bytes.as_slice(),
        "a rejected fill must leave the resident's device bytes byte-identical"
    );
    assert_eq!(
        backend.physical_tmem().identity(),
        snapshot_physical,
        "a fill never touches physical TMEM, rejected or not"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a rejected fill must leave no staged token for a later publish to redeem"
    );
}

/// **T-11:** dropping the sealed capsule instead of publishing leaves
/// the registry at its prior generation -- the cancellation path, which
/// the deferred token makes reachable for color targets too.
#[test]
fn dropping_the_capsule_before_publish_leaves_the_registry_untouched() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());
    let generation_before = backend.color_targets().unwrap().residents()[0]
        .generation()
        .get();

    let request = session.plan_request(capture(partial_width_fill_words()));
    let planned = backend.plan_raw_dpc(request).unwrap();
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).unwrap();
    let staged = backend.staged_guest_render_target_writes(submission);
    let committed = session
        .commit_guest_render_target_writes(prepared, staged)
        .unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    assert!(
        backend.has_pending_fill_publication(),
        "the token is held between execute and publish -- that window is the design"
    );
    drop(capsule);

    assert_eq!(
        backend
            .color_targets()
            .expect("the registry was built during execution")
            .residents()[0]
            .generation()
            .get(),
        generation_before,
        "a dropped capsule publishes nothing, so the registry stays at its prior generation"
    );
}

/// **T-13:** the split-arm regression proof. `FullSync` must still be
/// rejected loudly now that it no longer shares a match arm with
/// `FillRectangle`.
#[test]
fn plan_raw_dpc_still_rejects_a_full_sync_command_after_the_arm_split() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let mut words = partial_width_fill_words();
    words.extend([word(FULL_SYNC, 0), 0]);

    let request = session.plan_request(capture(words));
    assert!(
        backend.plan_raw_dpc(request).is_err(),
        "admitting FillRectangle must not have admitted FullSync alongside it"
    );
}

/// Fill and combined cycles plan; Copy is refused by the public-result
/// contract rather than silently admitted as a zero-write command.
#[test]
fn plan_raw_dpc_admits_combined_fill_rectangles_and_refuses_copy_cycle() {
    let plans = |cycle_bits: u32| {
        let (mut backend, session) = WgpuBackend::try_new().unwrap();
        let mut words = Vec::new();
        words.extend([word(SET_OTHER_MODE, cycle_bits << 20), 0]);
        words.extend(set_color_image_rgba16());
        words.extend(set_fill_color(0x213c_4d59));
        words.extend(fill_rectangle(4, 2, 14, 4));

        let request = session.plan_request(capture(words));
        backend.plan_raw_dpc(request).is_ok()
    };

    // Control: Fill cycle, the case that always planned, still does --
    // so a change that broke planning outright cannot pass this test by
    // making every arm behave identically.
    assert!(plans(3), "a Fill-cycle FillRectangle must still plan");
    for (name, cycle_bits) in [("OneCycle", 0u32), ("TwoCycle", 1)] {
        assert!(
            plans(cycle_bits),
            "{name}: combined FillRectangle must plan"
        );
    }
    assert!(
        !plans(2),
        "Copy-cycle G_FILLRECT has no guaranteed public result and must be refused"
    );
}

/// **T-15:** `plan_fill`'s fractional-edge gate must survive the
/// admission change. A coordinate with nonzero low two bits is a
/// quarter-pixel edge this slice does not execute.
#[test]
fn plan_raw_dpc_rejects_a_fractional_edge_fill_rectangle() {
    let (mut backend, session) = WgpuBackend::try_new().unwrap();
    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    // Same rectangle as the headline fixture, but with y1's
    // quarter-pixel fraction set.
    words.extend([
        word(FILL_RECTANGLE, ((14u32 << 2) << 12) | (4u32 << 2) | 1),
        ((4u32 << 2) << 12) | (2u32 << 2),
    ]);

    let request = session.plan_request(capture(words));
    assert!(
        backend.plan_raw_dpc(request).is_err(),
        "a fractional edge must be rejected, never truncated to whole pixels"
    );
}

/// **RETARGET.** This was
/// `execute_raw_dpc_rejects_a_mixed_fill_and_triangle_packet`, which
/// asserted `MixedFillAndTrianglePacket` for a packet carrying an
/// admitted fill and an admitted raw triangle. It pinned a shape gate,
/// and the shape gate's stated reason stopped being true.
///
/// The reason it gave was "the fill is executed CPU-side into an owned
/// buffer while `draw_admitted_triangles` rasterizes into a GPU colour
/// attachment that never composes back". That described the file at the
/// commit that wrote it. It does not describe it now: a flat non-Fill-
/// cycle raw triangle that declares a write is executed by
/// `targets/raw_triangle.rs`'s CPU rasterizer -- "producing the same
/// `CompletedColorTargetWrite` the fill and texrect executors produce"
/// -- and `stage_color_commands` composes it with the fill into ONE
/// accumulation buffer in the packet's own stream order.
///
/// WM2000 forced the re-read: at VI swap 2873 it issues 60 fill-cycle
/// FillRectangles and one raw triangle whose declared span is five
/// per-scanline `RenderTarget` accesses (see
/// `docs/WM2000-FILL-TRIANGLE-EVIDENCE.txt`). The gate cost the ROM all
/// 60 guest-visible fills.
///
/// What is asserted here is not "no longer refused" -- that alone would
/// be satisfied by a backend that silently dropped one half, which is
/// exactly the failure the old gate feared. Both halves' PIXELS are
/// read back out of the published resident and checked:
///
/// 1. Every pixel inside the triangle carries the triangle's own
///    primitive colour, hand-derived from the wire as
///    `TRIANGLE_PRIM_RGBA16`.
/// 2. Every pixel outside it differs from that colour -- so the fill
///    underneath was not blanked by a triangle output that replaced the
///    buffer instead of composing into it.
///
/// FAILS BEFORE this change: `execute_raw_dpc` returns
/// `MixedFillAndTrianglePacket` and there is nothing to read back.
#[test]
fn a_fill_and_a_raw_triangle_in_one_packet_both_reach_the_published_pixels() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // The whole-target fill and the flat triangle IN ONE PACKET. The
    // fill runs first (command order), establishing generation 1
    // honestly, and the triangle then patches into the buffer the fill
    // just produced -- the single accumulation seam under test.
    //
    // The triangle half is `flat_triangle_packet_words` minus its own
    // `set_color_image_rgba16` (the fill already staged the identical
    // image; a second one would be a different fact under test).
    let (low, high) =
        crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_PRIMITIVE);
    let mut words = whole_target_fill_words();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(low, high));
    words.extend(set_prim_color(0, 0, TRIANGLE_PRIM_WIRE));
    words.extend(flat_triangle_in_target_words());

    let planned = plan_with_no_reads(&mut backend, &session, words);
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = match backend.execute_raw_dpc(bound) {
        Ok(prepared) => prepared,
        // The GPU raster runs after this card's CPU staging and refuses
        // on an adapterless host. That says nothing about the guest
        // bytes -- but it must never be the removed shape gate, which
        // no longer exists to be hit.
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("TriangleDrawBeforeCreate")
                    || message.contains("no GPU adapter"),
                "the only tolerated failure is the adapterless GPU raster path, got: {error}"
            );
            return;
        }
    };

    let staged = backend.staged_guest_render_target_writes(submission);
    // The fill declares one whole-target write; the triangle declares
    // one per covered scanline (rows 0, 1, 2). Four in journal order --
    // and the count is the discriminating half: three would mean the
    // fill was dropped, one would mean the triangle was.
    assert_eq!(
        staged.len(),
        4,
        "one whole-target fill write plus the triangle's three per-row writes; got {staged:?}"
    );

    let committed = session
        .commit_guest_render_target_writes(prepared, staged)
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);

    let registry = backend
        .color_targets()
        .expect("the composed packet published a resident");
    let resident = registry
        .residents()
        .iter()
        .find(|resident| resident.key().address().get() == FILL_TARGET_ADDRESS)
        .expect("the target is resident");
    let bytes = resident.device_bytes().device_bytes();
    for y in 0..FILL_TARGET_HEIGHT as usize {
        for x in 0..FILL_TARGET_WIDTH as usize {
            let offset = (y * FILL_TARGET_WIDTH as usize + x) * 2;
            let pixel = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            let inside = y < 3 && (2..6).contains(&x);
            if inside {
                assert_eq!(
                    pixel, TRIANGLE_PRIM_RGBA16,
                    "pixel ({x},{y}) is inside the triangle, so the triangle half of the \
                     composition must have reached the published buffer"
                );
            } else {
                // Hand-derived from the fill's own wire colour, not read
                // back from the buffer: an assert_ne against the
                // triangle colour alone would pass for a buffer of
                // zeros, i.e. for a fill that never ran.
                assert_eq!(
                    pixel,
                    expected_fill_halfword(COMPOSED_FILL_COLOR, x as u32),
                    "pixel ({x},{y}) is outside the triangle, so it must still carry the \
                     fill's own colour -- a triangle output that replaced the buffer rather \
                     than composing into it would blank it"
                );
            }
        }
    }
}

/// The new refusal did not over-reject: a fill with no triangle beside
/// it still executes and still stages its token. Without this, the
/// check above could have been written as "any packet with a fill" and
/// nothing in this module would have noticed.
#[test]
fn a_fill_only_packet_still_executes_after_the_triangle_refusal() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let (_, result) =
        plan_and_execute_fill(&mut backend, &mut session, whole_target_fill_words());
    result.expect("a fill-only packet must still execute -- the new check is fill+triangle");
    assert!(
        backend.has_pending_fill_publication(),
        "a fill-only packet must still stage its deferred publication token"
    );
}

/// Intermediate command completions are consumed to seed their successor,
/// while the final completion is retained for admission and publication.
/// This adapterless fixture observes bytes owned by every command, so
/// dropping, reordering, or failing to transfer either intermediate
/// completion changes the published image.
#[test]
fn three_fills_transfer_intermediate_ownership_and_publish_in_order() {
    const BASE: u32 = 0x0842_1085;
    const LEFT: u32 = 0x213c_4d59;
    const RIGHT: u32 = 0x6319_7bdf;

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, three_fill_words());

    let resident = published_target_bytes(&backend);
    for y in 0..FILL_TARGET_HEIGHT {
        for x in 0..FILL_TARGET_WIDTH {
            let offset = ((y * FILL_TARGET_WIDTH + x) * 2) as usize;
            let actual = u16::from_be_bytes([resident[offset], resident[offset + 1]]);
            let color = if (2..=5).contains(&x) && (2..=5).contains(&y) {
                LEFT
            } else if (10..=13).contains(&x) && (1..=6).contains(&y) {
                RIGHT
            } else {
                BASE
            };
            assert_eq!(
                actual,
                expected_fill_halfword(color, x),
                "pixel ({x}, {y}) must retain its latest command owner"
            );
        }
    }
}

/// A triangle-only packet reaches `draw_admitted_triangles` rather
/// than being caught by anything in `stage_and_report`.
///
/// This was the mirror of the fill+triangle shape gate, and asserted
/// `assert_ne!(reason, MixedFillAndTrianglePacket)`. That gate is gone
/// (see `a_fill_and_a_raw_triangle_in_one_packet_both_reach_the_
/// published_pixels`), so the negative half would now be vacuous and is
/// removed. The positive half is what always carried the claim and is
/// kept unchanged.
///
/// The draw itself needs a real adapter, so on an adapterless host the
/// packet's execution ends in `TriangleDrawBeforeCreate`. That is the
/// *evidence* this test wants, not a limitation of it: reaching that
/// error proves `stage_and_report` admitted the plan and
/// `execute_raw_dpc` went on to attempt the draw. (The full real-GPU
/// success path for a
/// triangle-only plan is covered under `host-gpu-tests` by
/// `triangle_only_plan_completes_via_preserving_physical_and_never_
/// flips_the_slot`.)
#[test]
fn a_triangle_only_packet_reaches_the_draw_rather_than_a_stage_and_report_refusal() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    let planned = plan_with_no_reads(&mut backend, &session, triangle_only_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    match backend.execute_raw_dpc(bound) {
        // A host that DOES have an adapter AND has had create() called
        // on it draws the triangle and succeeds; this test calls no
        // create(), so on every host today it takes the Err arm below.
        // The arm is kept rather than made `unreachable!()` because the
        // claim under test is "went PAST stage_and_report", which a
        // success satisfies just as well as the draw's own error.
        Ok(_) => {}
        Err(RenderError::Backend { reason, .. }) => {
            assert_eq!(
                reason,
                WgpuRawDpcExecutionError::TriangleDrawBeforeCreate.to_string(),
                "on an adapterless host the only expected outcome is the draw's own \
                 TriangleDrawBeforeCreate, reached by going PAST stage_and_report"
            );
        }
        Err(other) => panic!("expected either success or the draw's own error, got {other:?}"),
    }
}

/// **The sync-only packet: the shape WM2000 aborts this backend on.**
///
/// Measured on the real ROM through the all-Rust lane
/// (`FN64_RECOMP=rs`, `FN64_RENDER=wgpu`), the packet that reached
/// `NoCompletedLoads` is exactly one wire command --
/// `wire_opcode = 0xE9` (`G_RDPFULLSYNC`), raw words
/// `[0xE9000000, 0x07000000]` -- with **zero** loads, triangles,
/// texrects and fills, and a single `ResourceAccess`:
/// `Read`/`CommandDecode` over the 8 `RspDmem` bytes of the sync
/// command itself. Its site carried `dp_slot_reserved: true` and
/// `interrupt_after: Clear`, so it planned cleanly and was
/// deliberately admitted -- and then refused at execution.
///
/// A sync-only packet has nothing to *raster*, which is what the
/// refusal's doc meant, but that is not the same as having nothing to
/// *do*: `SYNC_FULL`'s whole effect is on the RDP pipeline and the DP
/// interrupt line, and this backend's own `PlanCollector` already says
/// so at the `FullSyncSite` arm -- "collected, not executed ... the
/// site is retained so the executed plan still accounts for every
/// command the plan carried". Refusing the packet drops the very
/// command that arm went out of its way to retain.
///
/// The completion route is not a widening of the refusal. A sync
/// declares zero `ResourceAccess` writes by construction
/// (`RdpFullSyncSite`'s own doc: "Pushes zero `ResourceAccess`
/// entries: a sync reads and writes no resource"), so
/// `complete_execution_preserving_physical` -- which builds its own
/// explicitly-empty write list and lets
/// `BackendEffectReport::try_new` check it against the packet's real
/// journal -- *proves* the zero-write property here rather than
/// assuming it. A packet that secretly declared a write is still
/// rejected there with `EffectCountMismatch`, independently of this
/// branch.
///
/// Hand-derived, not captured: `word(FULL_SYNC, 0)` is
/// `0x29 << 24 == 0x29000000`, the RDP-side `SYNC_FULL` opcode this
/// module's decoder reads (the 0xE9 seen on the wire is the same
/// command in the ABI's own command-byte space).
#[test]
fn a_sync_only_packet_executes_instead_of_being_refused_as_having_zero_loads() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    let request = session.plan_request(full_sync_capture(sync_only_words()));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("a reserved sync-only capture must plan cleanly");
    assert!(
        planned.guest_read_plan().reads().is_empty(),
        "a sync-only plan declares no TmemLoadSource reads"
    );
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let initial_identity = backend.physical_tmem().identity();

    let prepared = backend.execute_raw_dpc(bound).expect(
        "a sync-only packet must execute: it declares zero writes and zero raster work, and              refusing it aborts the real WM2000 boot",
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a sync-only packet stages no color-target write, so it must leave no redeemable              fill token"
    );
    let committed = session.commit_zero_guest_writes(prepared).unwrap();

    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();

    let outcome = backend.publish_raw_dpc(capsule);
    assert_eq!(outcome.submission(), submission);
    assert_eq!(
        backend.physical_tmem().identity(),
        initial_identity,
        "a sync touches no TMEM, so it has no successor to flip to -- an identity change              would mean complete_execution (the flipping route) was used instead of the              preserving one"
    );
}

/// **Positive control for the fixture above.** Without this, the
/// sync-only test could pass against a plan that silently carried a
/// load, a triangle or a fill, and would then be proving nothing about
/// the refused shape at all. Pins every count the real measurement
/// reported, plus the one access being a `Read` rather than a write.
///
/// Measured through the real decoder via `ExecutionCollector`, exactly
/// as `plan_of` does -- not re-derived from the wire words, since a
/// second parser here could agree with the fixture while disagreeing
/// with what execution actually sees.
#[test]
fn the_sync_only_fixture_really_is_one_sync_command_with_no_executable_work() {
    let plan = plan_of_no_reads(sync_only_words());

    assert_eq!(plan.full_sync_sites.len(), 1, "exactly one SYNC_FULL site");
    assert!(plan.loads.is_empty(), "no TMEM loads");
    assert!(plan.triangles.is_empty(), "no admitted triangles");
    assert!(plan.texrect_commands.is_empty(), "no texrects");
    assert!(plan.fills.is_empty(), "no fills");
    assert_eq!(
        plan.next_command_index, 1,
        "the packet is exactly one wire command"
    );
    assert!(
        !plan.accesses.is_empty(),
        "the sync's own command-decode read must be declared, or the access assertion below              is vacuous"
    );
    assert!(
        plan.accesses
            .iter()
            .all(|access| access.mode() == fn64_render_ir::AccessMode::Read),
        "a sync declares no write access -- only its own command-decode read"
    );
}

/// **The arm the sync fix deliberately KEPT, pinned so it cannot be
/// widened away.**
///
/// Admitting the sync-only packet above narrowed `NoCompletedLoads` to
/// "no load, no triangle, AND no sync". Without this test, deleting the
/// refusal outright -- routing every load-free plan to
/// `NoPhysicalSuccessor` -- passes the whole suite, so nothing would
/// distinguish the correct narrowing from simply dropping the guard.
/// (Measured: that exact mutant survives the suite without this test.)
///
/// The fixture is a packet of `SetOtherMode`/`SetCombine` and nothing
/// else: pure durable RDP register writes, which `PlanCollector` folds
/// into `draw.other_mode`/`draw.combine` and pushes onto no
/// command list at all. It therefore carries zero loads, zero
/// triangles, zero texrects, zero fills and zero `SYNC_FULL` sites --
/// the one shape that genuinely has no command whose completion this
/// backend could account for, and the only shape this refusal still
/// names.
#[test]
fn a_plan_with_no_load_no_triangle_and_no_sync_is_still_refused_by_name() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    let planned = plan_with_no_reads(&mut backend, &session, state_only_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    match backend.execute_raw_dpc(bound) {
        Err(RenderError::Backend { reason, .. }) => assert_eq!(
            reason,
            WgpuRawDpcExecutionError::NoCompletedLoads.to_string(),
            "a plan carrying only durable register writes has no completable command;                  admitting it would mean the sync fix widened the refusal away instead of                  narrowing it"
        ),
        other => panic!(
            "a plan with no load, no triangle and no sync must be refused by name, got                  {other:?}"
        ),
    }
}

/// **Positive control for the refusal fixture above.** Proves the
/// state-only packet really carries none of the three completable
/// command kinds -- otherwise the refusal it asserts could be firing
/// for some other reason entirely.
#[test]
fn the_state_only_fixture_really_carries_no_completable_command() {
    let plan = plan_of_no_reads(state_only_words());

    assert!(plan.loads.is_empty(), "no TMEM loads");
    assert!(plan.triangles.is_empty(), "no admitted triangles");
    assert!(plan.texrect_commands.is_empty(), "no texrects");
    assert!(plan.fills.is_empty(), "no fills");
    assert!(plan.full_sync_sites.is_empty(), "no SYNC_FULL sites");
    assert_eq!(
        plan.next_command_index, 2,
        "the packet is exactly two wire commands -- SetOtherMode and SetCombine -- so the              emptiness asserted above is emptiness of COMPLETABLE work, not an empty stream"
    );
    assert!(
        plan.draw.other_mode.is_some() && plan.draw.combine.is_some(),
        "both register writes must have been folded into durable state, or the fixture is              not the shape this test claims"
    );
}

/// Ordering: a submission whose triangle draw FAILS must leave no
/// redeemable fill token behind.
///
/// `execute_raw_dpc` used to store `pending_fill_publication` before
/// calling `draw_admitted_triangles`, so a draw failure returned `Err`
/// with the token already on the backend -- a later `publish_raw_dpc`
/// could then redeem a fill from a submission that never completed.
///
/// Inducing the failure needs a submission that carries BOTH a fill (to
/// produce a token) and a triangle draw that fails -- and the new
/// refusal above now rejects exactly that packet before either happens.
/// So this drives the two halves of `execute_raw_dpc` directly, in its
/// own order: `execute_raw_dpc_inner` on a fill-only packet yields a
/// real token, then `draw_admitted_triangles` is called with a triangle
/// whose plan state never resolved. Both halves are the production
/// functions, not stand-ins; only their sequencing is reproduced here.
///
/// The chosen failure is `MissingTriangleDrawState::NoCombine`, not the
/// review's `TriangleDrawBeforeCreate`: this host has a real Metal
/// adapter, so `configure_fill_target_height`'s `create_inner` succeeds
/// and the pipeline IS present. `NoCombine` fails inside the same
/// function on any host, adapter or not, and is the same
/// `execute_raw_dpc` error path -- it is `draw_admitted_triangles`
/// returning `Err` that this test is about, not which `Err`.
#[test]
fn a_failed_triangle_draw_leaves_no_redeemable_fill_token() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let planned = plan_with_no_reads(&mut backend, &session, whole_target_fill_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    let (_prepared, _triangles, pending, _draw_tmem, _probe, _replacement) =
        execute_raw_dpc_inner(
            &mut backend.coordinator,
            bound,
            backend
                .raw_dpc_carry_in_before_last_plan
                .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
            &mut backend.color_targets,
            backend.configured_target_extent,
            true,
            false,
            false,
            None,
            None,
            None,
            None,
        )
        .expect("the fill half must stage a real token");
    assert!(
        pending.is_some(),
        "this fixture must actually produce a token, or the ordering claim is vacuous"
    );

    // The draw half, on the same backend the fill just staged against.
    let draw = backend.draw_admitted_triangles(
        vec![Err(MissingTriangleDrawState::NoCombine {
            triangle_index: 0,
        })],
        None,
        true,
    );
    assert!(
        matches!(
            draw,
            Err(WgpuRawDpcExecutionError::MissingTriangleDrawState(_))
        ),
        "expected the draw to fail, got {draw:?}"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "the token must not be on the backend when the triangle draw fails -- the store \
         belongs AFTER the draw, not before it"
    );

    // The runtime half above proves the two production functions
    // compose correctly in this order, but it calls them itself -- it
    // cannot notice `execute_raw_dpc` reverting to the OLD order. So
    // the ordering `execute_raw_dpc` actually uses is pinned at the
    // source level too, the same way
    // `publish_raw_dpc_source_is_exactly_prepare_publication_then_commit`
    // pins its own body's shape.
    let source = include_str!("../../production/state.rs");
    let body_start = source
        .find("fn execute_raw_dpc(")
        .expect("execute_raw_dpc must exist in this file");
    let body_end = source[body_start..]
        .find("\n    }\n")
        .expect("execute_raw_dpc must have a closing brace")
        + body_start;
    let body = &source[body_start..body_end];

    let draw_at = body
        .find("self.draw_admitted_triangles(")
        .expect("execute_raw_dpc must still call draw_admitted_triangles");
    let store_at = body
        .find("self.pending_fill_publication = pending;")
        .expect("execute_raw_dpc must still store the pending token");
    assert!(
        draw_at < store_at,
        "execute_raw_dpc must call draw_admitted_triangles BEFORE storing \
         pending_fill_publication -- storing first leaves a redeemable token on the backend \
         when the draw fails and the call returns Err"
    );
    assert_eq!(
        body.matches("self.pending_fill_publication = pending;")
            .count(),
        1,
        "exactly one store site, or the ordering above says nothing about the other"
    );
}

/// A mixed TMEM-load-plus-fill packet composes, and stages exactly the
/// writes its fill declared.
///
/// **Retargeted, and the old assertion was measuring something else.**
/// This asserted that such a packet is REFUSED, on the stated reasoning
/// that "this slice does not implement the journal-order merge". That
/// reasoning expired: `stage_color_commands` composes fills, texrects
/// and raw triangles in the decoder's own stream order, which is
/// exactly that merge, and `docs/RT64-GUARD-AUDIT.md` records
/// `MixedFillAndTrianglePacket` being removed for the same reason.
///
/// What actually produced the `Err` was incidental:
/// `fill_rectangle(4, 2, 14, 4)` covers columns 1..=3 of a 16-wide
/// target, so it was a partial fill of a brand-new target and tripped
/// `PartialNewTargetInitialization` -- a guard about seeding, not about
/// mixing. Now that a partial fill carries a colour-image seed, the
/// packet completes, and asserting a refusal here would be asserting
/// the seeding bug.
///
/// So this pins the property the fixture can still honestly show: the
/// mixed packet composes, and the writes it stages are its fill's own
/// declared rows -- never another source's, and never none.
#[test]
fn a_mixed_tmem_and_fill_packet_composes_and_stages_its_own_writes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let mut words = one_load_block_words();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x213c_4d59));
    words.extend(fill_rectangle(4, 2, 14, 4));

    let (planned, source_bytes) = plan_with_deterministic_reads(&mut backend, &session, words);
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    backend
        .execute_raw_dpc(bound)
        .expect("a mixed TMEM+fill packet composes in journal order");
    assert!(
        backend.has_pending_fill_publication(),
        "a composed mixed packet must stage its fill's publication"
    );
    // Hand-derived from the wire. `fill_rectangle` takes WHOLE pixels
    // and shifts them left by two itself, so (4,2)..(14,4) is columns
    // 4..=14 and rows 2..=4 of a 16-wide RGBA16 target -- 11 columns,
    // 3 rows. `x0 != 0`, so `plan_render_target_rows` takes its per-row
    // branch and declares one access PER ROW rather than collapsing to
    // one contiguous run: 3 writes of 11 pixels x 2 bytes = 22 bytes.
    //
    // (First written as quarter-pixels, which gave 2 rows of 6 bytes
    // and failed -- the helper's own units are the fact to read, not
    // the wire encoding it produces.)
    let staged = backend.staged_guest_render_target_writes(submission);
    assert_eq!(staged.len(), 3, "one declared write per covered row");
    for write in &staged {
        assert_eq!(
            write.byte_count(),
            22,
            "eleven columns of RGBA16 is twenty-two bytes per row"
        );
    }
}

/// Hostile: a submission mismatch yields an EMPTY staged-write list, not
/// another submission's writes. That empty list then drives the caller
/// into the zero-write commit branch, which fails loudly against the
/// packet's own nonempty guest-write journal -- a loud rejection rather
/// than a quiet wrong publish.
#[test]
fn staged_guest_render_target_writes_returns_empty_for_a_foreign_submission() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    publish_one_fill(&mut backend, &mut session, whole_target_fill_words());

    let request = session.plan_request(capture(partial_width_fill_words()));
    let planned = backend.plan_raw_dpc(request).unwrap();
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).unwrap();

    assert_eq!(
        backend.staged_guest_render_target_writes(submission).len(),
        3
    );

    // A different submission's identity, taken from a second plan on the
    // same session.
    let other_request = session.plan_request(capture(full_width_fill_words()));
    let other_planned = backend.plan_raw_dpc(other_request).unwrap();
    let other_bound = finalize_and_submit_pair(&mut session, other_planned).unwrap();
    let other_submission = other_bound.submission();
    assert_ne!(other_submission, submission);
    assert!(
        backend
            .staged_guest_render_target_writes(other_submission)
            .is_empty(),
        "a submission this backend staged no write for must report an empty list, never \
         another submission's writes"
    );

    drop(prepared);
    drop(other_bound);
}

/// Regression: a TMEM-only submission still reports no staged guest
/// writes at all, so the existing zero-write commit path is undisturbed.
#[test]
fn tmem_only_submissions_stage_no_guest_render_target_writes() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    let submission = bound.submission();
    let prepared = backend.execute_raw_dpc(bound).unwrap();

    assert!(
        backend
            .staged_guest_render_target_writes(submission)
            .is_empty(),
        "a TMEM-only submission stages no color-target write"
    );
    assert!(backend.color_targets().is_none());
    session.commit_zero_guest_writes(prepared).unwrap();
}

/// Hostile: an admitted fill reaching execution with no prior `create`
/// call is rejected loudly. The RDP's `SetColorImage` carries no height
/// field, so this backend has no honest way to size the color target --
/// and inventing one would fabricate the target's identity and range.
#[test]
fn a_fill_before_any_create_is_rejected_rather_than_given_an_invented_height() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    // Deliberately NO configure_fill_target_height call.
    let (_, result) =
        plan_and_execute_fill(&mut backend, &mut session, partial_width_fill_words());
    assert!(
        result.is_err(),
        "with no host-configured height, an admitted fill must be rejected, not sized by \
         a fabricated default"
    );
    assert!(!backend.has_pending_fill_publication());
}

/// Positive: `RenderBackend::create` is a no-op on `WgpuBackend`'s
/// existing TMEM-only tests -- none of them call `create` at all, so
/// this backend's whole TMEM-only test surface above is completely
/// unaffected by `create`'s new eager triangle-pipeline
/// initialization. This test only proves `create` itself has not
/// broken compilation/basic construction on a backend that never
/// touches a triangle -- it deliberately does NOT call `create` before
/// exercising the existing TMEM-only path, matching every other test
/// in this module.
#[test]
fn tmem_only_path_never_calls_create_and_is_unaffected_by_its_existence() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    // No backend.create(...) call here, deliberately -- mirrors every
    // other TMEM-only test in this module.
    let (planned, source_bytes) =
        plan_with_deterministic_reads(&mut backend, &session, one_load_block_words());
    let guest_capture = guest_read_capture(&planned, &source_bytes);
    let bound = session.finalize_and_submit(planned, guest_capture).unwrap();
    backend
        .execute_raw_dpc(bound)
        .expect("TMEM-only execution must succeed without ever calling create()");
}

/// A resize BEFORE any `create` records the host-configured extent and
/// leaves `triangle_target_extent` `None`.
///
/// The pairing invariant (§1a/§1e) is that `triangle_pipeline` and
/// `triangle_target_extent` are `Some` together or `None` together. A
/// resize that populated `triangle_target_extent` on a backend with no
/// pipeline would break it, and `draw_admitted_triangles` reads the two
/// through separate `ok_or`s -- so the broken state would not be caught
/// there, it would just draw at an extent no device was ever requested
/// for. `configured_target_extent` is a different field with a different
/// rule: it is deliberately adapter-independent (see `create_inner`), so
/// it IS written here.
#[test]
fn resize_before_create_records_only_the_adapter_independent_extent() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    assert_eq!(backend.configured_target_extent, None);
    assert_eq!(backend.triangle_target_extent, None);

    backend.resize(320, 240);

    assert_eq!(
        backend.configured_target_extent,
        Some(TriangleTargetExtent {
            width: 320,
            height: 240,
        }),
        "the CPU-side fill path's color-image height must follow a resize even with no adapter"
    );
    assert_eq!(
        backend.triangle_target_extent, None,
        "a resize must never populate a triangle extent with no pipeline behind it"
    );
    assert!(
        backend.triangle_pipeline.is_none(),
        "a resize must not request a device -- that is create()'s job, not this one's"
    );
}

/// A resize AFTER a successful create updates both extents together and
/// keeps the live pipeline.
///
/// `create_inner` here is allowed to report `NoAdapter` (this test must
/// run on the default, adapterless configuration too), so the pipeline
/// half is asserted conditionally on whether one was actually obtained
/// -- what is unconditional is the pairing: whichever way create went,
/// `triangle_target_extent.is_some()` still equals
/// `triangle_pipeline.is_some()` after the resize, and the extent that
/// exists is the new one, never the create-time one.
#[test]
fn resize_after_create_updates_both_extents_and_keeps_the_pipeline() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let had_pipeline = backend.triangle_pipeline.is_some();
    assert_eq!(
        backend.triangle_target_extent.is_some(),
        had_pipeline,
        "create_inner's own pairing invariant must hold before the resize is judged"
    );

    backend.resize(640, 480);

    let resized = TriangleTargetExtent {
        width: 640,
        height: 480,
    };
    assert_eq!(
        backend.configured_target_extent,
        Some(resized),
        "the fill path's extent must be the resized one, not create()'s"
    );
    assert_eq!(
        backend.triangle_pipeline.is_some(),
        had_pipeline,
        "a resize must not drop or re-request the device -- nothing this backend owns is \
         sized at create time"
    );
    assert_eq!(
        backend.triangle_target_extent.is_some(),
        had_pipeline,
        "the pipeline/extent pairing must survive a resize in both directions"
    );
    if had_pipeline {
        assert_eq!(
            backend.triangle_target_extent,
            Some(resized),
            "a live pipeline's raster extent must follow the resize; the per-submission \
             attachments are built from it"
        );
    }
}

/// A resize to the SAME dimensions is a real write of the same value,
/// not a special-cased early return.
///
/// There is deliberately no `if new == old { return }` guard: this
/// method allocates nothing and touches no device, so an equality check
/// would only add a branch whose "skip" arm is indistinguishable from
/// the silent no-op this whole change removes. The observable
/// requirement is idempotence, which is what this asserts.
///
/// A same-dimensions resize is trivially satisfied by a no-op, so this
/// deliberately resizes AWAY first and then back: that makes the return
/// leg a real write whose result a no-op cannot produce, and only then
/// is repeating it asserted to be a fixed point.
#[test]
fn resize_to_the_same_dimensions_is_idempotent() {
    let (mut backend, _session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let after_create = backend.configured_target_extent;
    let pipeline_extent_after_create = backend.triangle_target_extent;

    // Away, so the return leg below cannot be satisfied by doing nothing.
    backend.resize(FILL_TARGET_WIDTH * 3, FILL_TARGET_HEIGHT * 3);
    assert_ne!(
        backend.configured_target_extent, after_create,
        "the intermediate resize must genuinely move the extent"
    );

    backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT);
    assert_eq!(
        backend.configured_target_extent, after_create,
        "resizing back to create()'s own dimensions must restore exactly that extent"
    );
    assert_eq!(
        backend.triangle_target_extent, pipeline_extent_after_create,
        "same for the triangle extent"
    );

    backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT);
    assert_eq!(
        backend.configured_target_extent, after_create,
        "and a repeated identical resize must still be a fixed point"
    );
    assert_eq!(
        backend.triangle_target_extent, pipeline_extent_after_create,
        "the triangle extent must be a fixed point under repetition too"
    );
}

/// A resize to zero is RECORDED, not clamped and not ignored, and the
/// zero then surfaces as a named rejection at the point of use.
///
/// This is the honest reading of the trait's own contract ("a backend
/// that cannot honor a resize should surface that at the next
/// `process_task`/`present` call ... not here"): `resize` is infallible,
/// so the refusal has to live downstream, and it already does --
/// `ColorTargetExtent::try_new` rejects a zero height with
/// `TargetError::ZeroExtent`. Clamping to 1 would invent a target
/// geometry the host never asked for and publish a resident whose byte
/// range is wrong; ignoring the call would be the silent no-op this
/// change exists to delete. Asserting the *named* error, not merely
/// "some error", is the point.
#[test]
fn resize_to_zero_is_recorded_and_rejected_by_name_at_the_fill() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    backend.resize(FILL_TARGET_WIDTH, 0);
    assert_eq!(
        backend.configured_target_extent,
        Some(TriangleTargetExtent {
            width: FILL_TARGET_WIDTH,
            height: 0,
        }),
        "a zero extent must be recorded verbatim -- never clamped to 1, never dropped"
    );

    let planned = plan_with_no_reads(&mut backend, &session, whole_target_fill_words());
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let error = backend
        .execute_raw_dpc(bound)
        .expect_err("a fill against a zero-height target must not execute");
    let RenderError::Backend { reason, .. } = error else {
        panic!("expected a backend-scoped rejection, got {error:?}");
    };
    assert_eq!(
        reason,
        WgpuRawDpcExecutionError::Target(TargetError::ZeroExtent {
            width: FILL_TARGET_WIDTH,
            height: 0,
        })
        .to_string(),
        "the zero must surface as the color target's own named ZeroExtent, not as a generic \
         failure and not as a successful fill of a fabricated one-row target"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a rejected fill must stage no redeemable token"
    );
}

/// A resize between `execute_raw_dpc` and `publish_raw_dpc` must NOT
/// disturb the outstanding fill token, and the fill must still publish
/// at the extent it actually executed against.
///
/// Why keeping it is correct rather than merely convenient: the token's
/// `InitializedCandidateColorTarget` sealed its own `ColorTargetKey`
/// (address, extent, byte range) when the fill ran, and
/// `ColorTargetRegistry::prepare_publication` reads only that captured
/// key -- it never re-derives one from the backend's current
/// `configured_target_extent`. So a resize structurally cannot retarget
/// an already-executed fill; the invariant is in the type, not in a
/// guard inside `resize`. Dropping the token would instead throw away a
/// completed submission's guest-write report and fail it with
/// `EffectCountMismatch` for a window resize it had nothing to do with.
///
/// The resize here is deliberately to a DIFFERENT height than the fill
/// executed at, so a hypothetical re-derivation would produce a
/// different key and be caught.
#[test]
fn a_resize_between_execute_and_publish_leaves_the_fill_token_redeemable() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let request = session.plan_request(capture(whole_target_fill_words()));
    let planned = backend
        .plan_raw_dpc(request)
        .expect("fixture plans cleanly");
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let submission = bound.submission();
    let prepared = backend
        .execute_raw_dpc(bound)
        .expect("fixture executes cleanly");
    assert!(
        backend.has_pending_fill_publication(),
        "this fixture must actually stage a token, or the claim below is vacuous"
    );

    // The window changes size in the middle of the staged-then-publish
    // window -- a different height, so a re-derived key would differ.
    backend.resize(FILL_TARGET_WIDTH, FILL_TARGET_HEIGHT * 2);
    assert_eq!(
        backend.configured_target_extent,
        Some(TriangleTargetExtent {
            width: FILL_TARGET_WIDTH,
            height: FILL_TARGET_HEIGHT * 2,
        }),
        "the resize must genuinely have landed, or this test proves nothing about surviving \
         one"
    );
    assert!(
        backend.has_pending_fill_publication(),
        "a resize must not drop an outstanding fill token"
    );

    let staged = backend.staged_guest_render_target_writes(submission);
    assert!(
        !staged.is_empty(),
        "the staged guest writes must still bind to their own submission after a resize -- \
         an empty list here would drive the caller into the zero-write commit branch and \
         fail a submission that completed correctly"
    );
    let committed = session
        .commit_guest_render_target_writes(prepared, staged.clone())
        .unwrap();
    let mut fabric = admitted_fabric();
    let token = fabric.pending_dpc_submission().unwrap().token;
    let ready = fabric.prepare_dpc_commit(token).unwrap();
    let capsule = session.seal_publication(committed, ready).unwrap();
    backend.publish_raw_dpc(capsule);

    assert!(
        !backend.has_pending_fill_publication(),
        "the token must be redeemed by the publish, not left behind"
    );
    let residents = backend
        .color_targets()
        .expect("an executed fill builds the registry")
        .residents();
    assert_eq!(residents.len(), 1, "exactly one resident target was filled");
    assert_eq!(
        residents[0].key().extent().height(),
        FILL_TARGET_HEIGHT,
        "the published resident must carry the extent the fill EXECUTED at, never the \
         post-resize one -- the key is sealed in the token, not re-derived at publish"
    );
}

/// The adapterless CPU-side fill path still works after a resize, at the
/// new geometry.
///
/// `configured_target_extent` exists precisely so an admitted
/// `FillRectangle` executes with no adapter (see its own doc), and this
/// change writes that field -- so the hazard is that a resize breaks the
/// one path the field was created for. This drives a full
/// plan/execute/commit/seal/publish cycle at a resized height and proves
/// the resident lands at the NEW extent, which also proves the resize
/// actually reached the fill path rather than being cosmetic.
///
/// Deliberately not `#[cfg(feature = "host-gpu-tests")]`: the whole
/// point is the no-adapter case, and this host having a real Metal
/// adapter must not be what makes it pass. The fill executor is CPU-side
/// on either host.
#[test]
fn the_adapterless_fill_path_still_works_after_a_resize() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Half the height create() configured: a real geometry change the
    // fill's own ColorTargetKey must pick up.
    let resized_height = FILL_TARGET_HEIGHT / 2;
    backend.resize(FILL_TARGET_WIDTH, resized_height);

    let mut words = Vec::new();
    words.extend(fill_cycle_other_mode(0));
    words.extend(set_color_image_rgba16());
    words.extend(set_fill_color(0x0842_1085));
    words.extend(fill_rectangle(
        0,
        0,
        FILL_TARGET_WIDTH - 1,
        resized_height - 1,
    ));
    let staged = publish_one_fill(&mut backend, &mut session, words);
    assert!(
        !staged.is_empty(),
        "an admitted fill must still declare guest writes after a resize"
    );

    let registry = backend
        .color_targets()
        .expect("an executed fill builds the registry");
    let residents = registry.residents();
    assert_eq!(residents.len(), 1, "exactly one resident target was filled");
    assert_eq!(
        residents[0].key().extent().height(),
        resized_height,
        "the fill must have executed against the RESIZED height -- equal to \
         FILL_TARGET_HEIGHT here would mean the resize never reached the fill path"
    );
    assert_eq!(
        residents[0].key().extent().width(),
        FILL_TARGET_WIDTH,
        "width comes from SetColorImage's own wire field, not from the resize"
    );
}

/// `merged_fill_and_tmem_writes`' two loud arms, tested directly.
///
/// Neither is reachable from a legitimately decoded packet -- the
/// decoder builds the journal from the same walk that produces the
/// staged writes, so the two agree by construction -- which is exactly
/// why they are tested at the function. A defensive arm with no test is
/// a claim with no evidence; measured, deleting the `Undeclared` arm
/// left the whole 4991-test suite green before this test existed.
///
/// Both arms are real invariants, not paranoia: `Unclaimed` would
/// otherwise hand `BackendEffectReport::try_new` a short list, whose
/// count mismatch does not say WHICH access went unproduced, and
/// `Undeclared` would silently drop a write this backend actually
/// executed from the report that authorizes it.
///
/// The packet is a REAL composed packet, built through the same
/// probe-decode path `declared_write_purposes` uses, so the journal
/// under test is the decoder's own -- not a hand-built stand-in whose
/// shape could drift from what the decoder really emits.
#[test]
fn merging_rejects_a_declared_write_nobody_staged_and_a_staged_write_nobody_declared() {
    // A real composed packet, carrying the decoder's own journal.
    let capture = capture(tmem_then_fill_words());
    let layout = capture.memory_layout();
    let submission = capture.submission().clone();
    let probe_journal = single_source_probe_journal(&submission, layout).unwrap();
    let probe = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission.clone(),
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        probe_journal,
    )
    .unwrap();
    let accesses =
        match crate::decode_raw_dpc(submit_locally(probe).unwrap(), &RdpState::default()) {
            Err(RawDpcDecodeError::JournalMismatch { expected, .. }) => expected.into_vec(),
            Ok(decoded) => decoded.resource_plan().accesses().to_vec(),
            Err(error) => {
                panic!("probe decode must report the real access list, got {error:?}")
            }
        };
    let declared: u32 = accesses
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(fn64_render_ir::MAX_RESOURCE_ACCESSES, declared.max(1))
            .unwrap(),
        accesses.clone(),
    )
    .unwrap();
    let decoded = finalize_with_zero_reads(
        layout,
        capture.transaction_sequence(),
        submission,
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        journal,
    )
    .unwrap();
    let ticket = submit_locally(decoded).unwrap();
    let packet = ticket.packet();

    // Every write access the real journal declares, as a `CompletedWrite`
    // with a placeholder digest -- content is irrelevant to this
    // function, which composes by ACCESS identity alone.
    let all_writes: Vec<CompletedWrite> = accesses
        .iter()
        .filter(|access| access.mode() == AccessMode::Write)
        .map(|access| {
            CompletedWrite::try_new(
                *access,
                access.region().declared_bytes(),
                fn64_render_ir::ContentDigest::hash(b"merge-arm-test", &[]),
            )
            .unwrap()
        })
        .collect();
    assert!(
        all_writes.len() >= 2,
        "the composed fixture must declare at least a fill write and a TMEM write, got {}",
        all_writes.len()
    );

    // The honest, complete case: every declared write is claimed, and
    // the merge reproduces the journal's own order exactly.
    let merged = merged_fill_and_tmem_writes(packet, &all_writes, &[])
        .expect("a complete staged set must merge cleanly");
    assert_eq!(
        merged, all_writes,
        "the merge must reproduce the journal's own write order"
    );

    // Arm 1: a declared write nobody staged.
    let short = &all_writes[1..];
    let error = merged_fill_and_tmem_writes(packet, short, &[])
        .expect_err("a declared write with no staged producer must be rejected");
    assert!(
        matches!(error, WgpuRawDpcExecutionError::MergedWriteUnclaimed { .. }),
        "the rejection must name the unclaimed declared access, got: {error}"
    );

    // Arm 2: a staged write the journal never declared, with every
    // declared write ALSO present -- so the only defect is the extra
    // one, and arm 1 cannot fire first.
    // Cloned off a real declared write so its purpose and region are a
    // legal pairing; only the operation id is foreign, which is exactly
    // what makes it match no declared access.
    let template = all_writes
        .iter()
        .find(|write| write.access().purpose() == AccessPurpose::RenderTarget)
        .expect("the composed fixture declares a RenderTarget write");
    let foreign = CompletedWrite::try_new(
        fn64_render_ir::ResourceAccess::try_new(
            fn64_render_ir::OperationId::new(9_999),
            AccessMode::Write,
            template.access().purpose(),
            template.access().region(),
        )
        .unwrap(),
        template.byte_count(),
        template.content(),
    )
    .unwrap();
    let mut with_foreign = all_writes.clone();
    with_foreign.push(foreign);
    let error = merged_fill_and_tmem_writes(packet, &with_foreign, &[])
        .expect_err("a staged write the journal never declared must be rejected");
    assert!(
        matches!(
            error,
            WgpuRawDpcExecutionError::MergedWriteUndeclared {
                access_index: 9_999
            }
        ),
        "the rejection must name the undeclared staged access by id, got: {error}"
    );

    // Arm 3: one staged write may not satisfy TWO declared accesses.
    //
    // `ResourceJournal::try_new` does NOT enforce `OperationId`
    // uniqueness -- only the decoder's own `push_access`, which assigns
    // the vector index, makes ids unique in practice. So the claim
    // "each staged write is consumed once" is a real invariant this
    // function must enforce, not a fact the type system already
    // guarantees, and it is enforced by the `!taken` guard.
    //
    // Measured: removing that guard left the whole 4992-test suite
    // green before this case existed. It is pinned here rather than
    // argued as an equivalent mutant, because the argument would have
    // been wrong -- uniqueness is a construction convention, not a
    // validated invariant.
    // The real command-decode reads (the packet cannot be finalized
    // without them) plus the SAME write access twice.
    let mut duplicated: Vec<fn64_render_ir::ResourceAccess> = accesses
        .iter()
        .filter(|access| access.purpose() == AccessPurpose::CommandDecode)
        .copied()
        .collect();
    duplicated.push(template.access());
    duplicated.push(template.access());
    let duplicate_declared: u32 = duplicated
        .iter()
        .map(|access| access.region().declared_bytes())
        .sum();
    let duplicate_journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(
            fn64_render_ir::MAX_RESOURCE_ACCESSES,
            duplicate_declared.max(1),
        )
        .unwrap(),
        duplicated,
    )
    .expect("the journal type does not reject a repeated OperationId -- that is the point");
    let duplicate_decoded = finalize_with_zero_reads(
        capture.memory_layout(),
        capture.transaction_sequence(),
        capture.submission().clone(),
        capture.cmd_end(),
        capture.full_sync_boundaries().to_vec(),
        duplicate_journal,
    )
    .unwrap();
    let duplicate_ticket = submit_locally(duplicate_decoded).unwrap();
    let error = merged_fill_and_tmem_writes(duplicate_ticket.packet(), &[*template], &[])
        .expect_err("one staged write must not satisfy two declared accesses");
    assert!(
        matches!(error, WgpuRawDpcExecutionError::MergedWriteUnclaimed { .. }),
        "the second declared access must go unclaimed, got: {error}"
    );
}

/// **The card's headline unit test.** A packet carrying both a TMEM load
/// and an admitted `FillRectangle` executes and publishes BOTH halves,
/// instead of being refused before either could stage.
///
/// The two publications are separately asserted, because they are
/// separate identities that composition deliberately did not merge:
///
/// - the fill's resident color generation, advanced by
///   `publish_raw_dpc`'s registry publication;
/// - the TMEM half's physical successor, installed by
///   `complete_execution` into the coordinator's inactive slot and made
///   observable only when `commit` flips the active one.
///
/// The TMEM assertion is what discriminates the routing. A composed
/// packet routed to the fill-only completion
/// (`complete_execution_preserving_physical_with_effects`) would still
/// stage the fill, still return `Ok`, and still publish a resident --
/// but would never install the successor, leaving the published TMEM
/// state at its initial identity with no valid byte anywhere. That is
/// mutant (a)/(d) in this card's report, and it dies here.
#[test]
fn execute_raw_dpc_admits_a_composed_fill_and_tmem_packet() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    let identity_before = backend.physical_tmem().identity();
    assert!(
        backend.color_targets().is_none(),
        "no color target may exist before the composed packet"
    );
    assert!(
        (0..64u16).all(|address| !backend.physical_tmem().byte_is_valid(address)),
        "no TMEM byte may be valid before the composed packet"
    );

    let staged = publish_composed(&mut backend, &mut session, tmem_then_fill_words());

    // Half one: the fill's guest-visible write, and its resident.
    assert_eq!(
        staged.len(),
        1,
        "the whole-target fill half declares exactly one collapsed RenderTarget write"
    );
    assert_eq!(
        staged[0].byte_count(),
        FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
        "the fill half's write must cover the whole RGBA16 target"
    );
    let registry = backend
        .color_targets()
        .expect("the composed packet's fill half must have built the registry");
    assert_eq!(
        registry.residents().len(),
        1,
        "the composed packet's fill half must publish exactly one resident"
    );
    assert_eq!(
        registry.residents()[0].generation(),
        crate::TargetGeneration::FIRST,
        "the fill half's first publication is generation FIRST"
    );

    // Half two: the TMEM load's physical successor really became the
    // published state.
    assert_ne!(
        backend.physical_tmem().identity(),
        identity_before,
        "the composed packet's TMEM half must install a physical successor -- an unmoved \
         identity would mean the packet took the fill-only completion and silently \
         discarded the load"
    );
    assert!(
        (0..64u16).any(|address| backend.physical_tmem().byte_is_valid(address)),
        "the composed packet's TMEM half must leave real valid bytes in published TMEM"
    );

    assert!(
        !backend.has_pending_fill_publication(),
        "publication must consume the fill token, leaving nothing redeemable"
    );
}

/// **Constraint 2: ordering is semantics, and the two orders are
/// genuinely different.** The same two halves in the two possible stream
/// orders declare their write accesses in DIFFERENT sequences, and the
/// composed effect report follows each stream's own sequence.
///
/// This is the falsifiability the composition rests on. The order is not
/// chosen by `merged_fill_and_tmem_writes`: `fn64_render_ir`'s
/// `validate_effects` compares the reported write list against
/// `journal().write_accesses()` position by position, so any merge that
/// did not reproduce the journal's order would be rejected outright with
/// `EffectAccessMismatch`. Both orders executing cleanly is therefore
/// proof that the composed order IS the journal's order in both cases --
/// and the two journals differ, as the first two assertions show.
///
/// A merge that always emitted the fill's writes first, always emitted
/// the TMEM writes first, or sorted by anything other than journal
/// position would satisfy at most one of the two fixtures. That is
/// mutant (c) in this card's report, and it is killed here.
#[test]
fn a_composed_packet_reports_writes_in_the_streams_own_journal_order() {
    let tmem_first = declared_write_purposes(tmem_then_fill_words());
    let fill_first = declared_write_purposes(fill_then_tmem_words());

    // Both streams declare the same MULTISET of write purposes...
    let mut sorted_a: Vec<AccessPurpose> =
        tmem_first.iter().map(|(_, purpose)| *purpose).collect();
    let mut sorted_b: Vec<AccessPurpose> =
        fill_first.iter().map(|(_, purpose)| *purpose).collect();
    assert_eq!(
        sorted_a.len(),
        sorted_b.len(),
        "both orders must declare the same number of writes -- only their order differs"
    );
    sorted_a.sort_by_key(|purpose| format!("{purpose:?}"));
    sorted_b.sort_by_key(|purpose| format!("{purpose:?}"));
    assert_eq!(
        sorted_a, sorted_b,
        "both orders must declare the same write purposes as a multiset"
    );

    // ...in genuinely DIFFERENT sequences. Without this, "the merge
    // respects the order" would be a claim about two identical lists.
    let sequence_a: Vec<AccessPurpose> =
        tmem_first.iter().map(|(_, purpose)| *purpose).collect();
    let sequence_b: Vec<AccessPurpose> =
        fill_first.iter().map(|(_, purpose)| *purpose).collect();
    assert_ne!(
        sequence_a, sequence_b,
        "the two stream orders must declare their writes in different sequences, or this \
         test cannot discriminate a journal-ordered merge from a fixed-order one"
    );
    assert_eq!(
        sequence_a.first(),
        Some(&AccessPurpose::TmemLoadDestination),
        "the TMEM-first stream must declare its TMEM write before the fill's"
    );
    assert_eq!(
        sequence_b.first(),
        Some(&AccessPurpose::RenderTarget),
        "the fill-first stream must declare the fill's write before the TMEM one"
    );

    // And both execute. Since `validate_effects` rejects any reported
    // order but the journal's, two clean executions over two different
    // journal orders is the proof that the merge followed each.
    for (label, words) in [
        ("TMEM-first", tmem_then_fill_words()),
        ("fill-first", fill_then_tmem_words()),
    ] {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (_, result) = plan_and_execute_composed(&mut backend, &mut session, words);
        let prepared = result.unwrap_or_else(|error| {
            panic!(
                "the {label} composed order must execute -- a merge emitting a fixed \
                 order would be rejected here by validate_effects: {error}"
            )
        });
        drop(prepared);
    }
}

/// **RETARGET.** This was
/// `compositions_this_slice_does_not_admit_still_fail_by_name`,
/// asserting `MixedFillAndTrianglePacket` for fill+triangle and for
/// fill+TMEM+triangle. Both are now admitted; see
/// `a_fill_and_a_raw_triangle_in_one_packet_both_reach_the_published_
/// pixels` for why the shape gate's stated reason stopped being true.
///
/// The two shapes are kept, with the assertion inverted to the property
/// the gate was standing in for. A triangle that declares NO write --
/// `triangle_base_edge_words(7, 2, 0)`, whose `yl` of 0 covers no row --
/// must ride along **contributing nothing to the journal**. That is the
/// discriminating claim, and it is the one the old refusal's own
/// justification actually rested on:
///
/// 1. The packet executes rather than being refused for its shape.
/// 2. Its staged writes are EXACTLY the fill's own, byte for byte
///    identical to the writes the same fill produces with no triangle
///    beside it. A triangle that quietly declared or staged anything
///    would change this list, and a fill half that was dropped to make
///    room for the triangle would empty it.
///
/// Assertion 2 is what keeps this from being "the refusal was deleted":
/// it is a strictly stronger statement than "no error was returned",
/// and it fails for every half-execution the old gate feared.
///
/// FAILS BEFORE this change: both halves return
/// `MixedFillAndTrianglePacket`.
#[test]
fn a_write_declaring_nothing_triangle_rides_along_without_touching_the_journal() {
    // The control: the same fill, alone, in its own backend. Every
    // expectation below is this list -- derived from a run of the code
    // under test in a DIFFERENT shape, not from the shape under test.
    let fill_only_writes = {
        let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
        configure_fill_target_height(&mut backend);
        let (submission, result) =
            plan_and_execute_fill(&mut backend, &mut session, whole_target_fill_words());
        result.expect("the fill-only control must execute");
        backend.staged_guest_render_target_writes(submission)
    };
    assert!(
        !fill_only_writes.is_empty(),
        "the control must stage the fill's writes, or every comparison below is vacuous"
    );

    // fill + triangle, no TMEM load.
    let mut fill_and_triangle = whole_target_fill_words();
    fill_and_triangle.extend(set_other_mode(0, 0));
    fill_and_triangle.extend(set_combine(0, 0));
    fill_and_triangle.extend(triangle_base_edge_words(7, 2, 0));

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (submission, result) =
        plan_and_execute_fill(&mut backend, &mut session, fill_and_triangle);
    match result {
        Ok(_) => {
            assert_eq!(
                backend.staged_guest_render_target_writes(submission),
                fill_only_writes,
                "a triangle declaring no write must leave the fill's staged writes exactly \
                 as they are with no triangle present"
            );
        }
        // The GPU raster half refuses on an adapterless host. That is
        // downstream of the staging under test and is not the removed
        // shape gate.
        Err(error) => assert!(
            error.to_string().contains("TriangleDrawBeforeCreate")
                || error.to_string().contains("no GPU adapter"),
            "the only tolerated failure is the adapterless GPU raster path, got: {error}"
        ),
    }

    // fill + TMEM + triangle: the three-source merge, same claim.
    let mut all_three = one_load_block_words();
    all_three.extend(whole_target_fill_words());
    all_three.extend(set_other_mode(0, 0));
    all_three.extend(set_combine(0, 0));
    all_three.extend(triangle_base_edge_words(7, 2, 0));

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (submission, result) = plan_and_execute_composed(&mut backend, &mut session, all_three);
    match result {
        Ok(_) => {
            // Compared on range + byte count + content digest, not on
            // the whole `CompletedWrite`: this packet's journal also
            // declares the TMEM load's destination accesses, which
            // precede the fill's and shift its `OperationId` (measured:
            // 4 here versus 1 in the fill-only control). That index is a
            // position in a longer journal, not a difference in what the
            // fill wrote -- and the digest, which IS what it wrote, must
            // be byte-identical.
            let shape = |writes: &[CompletedWrite]| -> Vec<((u32, u32), u32, String)> {
                writes
                    .iter()
                    .map(|write| {
                        let range = match write.access().region() {
                            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                                (range.start().get(), range.len())
                            }
                            other => panic!("expected an RDRAM range, got {other:?}"),
                        };
                        (range, write.byte_count(), format!("{:?}", write.content()))
                    })
                    .collect()
            };
            let staged = backend.staged_guest_render_target_writes(submission);
            assert_eq!(
                shape(&staged),
                shape(&fill_only_writes),
                "in the three-source merge too, a triangle declaring no write must leave \
                 the fill's render-target ranges and content digests untouched"
            );
        }
        Err(error) => assert!(
            error.to_string().contains("TriangleDrawBeforeCreate")
                || error.to_string().contains("no GPU adapter"),
            "the only tolerated failure is the adapterless GPU raster path, got: {error}"
        ),
    }
}

/// A composed packet whose fill half is rejected leaves NOTHING behind:
/// no redeemable fill token, and no advanced physical TMEM generation.
///
/// The fill is made unadmittable at execute time by never configuring a
/// color-image height, which is `NoColorTargetHeight` -- a rejection
/// raised inside `stage_fill`, i.e. AFTER the TMEM half has already
/// staged its whole transaction. That is exactly the interleaving where
/// a partial publish would be possible, and the assertions below are
/// that it does not happen.
#[test]
fn a_composed_packet_whose_fill_half_is_rejected_publishes_neither_half() {
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    // Deliberately NOT `configure_fill_target_height`.
    let generation_before = backend.physical_tmem().generation();

    let (_, result) =
        plan_and_execute_composed(&mut backend, &mut session, tmem_then_fill_words());
    let error = result.expect_err(
        "a composed packet whose fill half has no color-image height must be rejected",
    );
    assert!(
        error
            .to_string()
            .contains(&WgpuRawDpcExecutionError::NoColorTargetHeight.to_string()),
        "the rejection must be the named NoColorTargetHeight variant, got: {error}"
    );
    assert!(
        !backend.has_pending_fill_publication(),
        "a rejected composed packet must leave no redeemable fill token"
    );
    assert_eq!(
        backend.physical_tmem().generation(),
        generation_before,
        "a rejected composed packet must not advance the published TMEM generation either \
         -- the TMEM half staged before the fill half failed, and staging is not publishing"
    );
}

/// The same durable-carry defect class as the test above, at the RDP's
/// eight **tile** registers.
///
/// Found by the same measurement: with the color-image carry fixed, the
/// real ROM advanced one packet and stopped at `TexrectUnboundTile` with
/// an entirely empty tile table -- 46 texrects, none of which
/// re-declared a tile the earlier packet had already set.
///
/// Asserted through `PlanCollector::seeded` directly rather than a full
/// packet, because the fact under test is exactly the seed: a collector
/// handed durable tiles must start with them bound, and one handed none
/// must not invent any. Both halves, so a seed that filled the table
/// with a zeroed default would fail the second assertion.
#[test]
fn a_plan_collector_starts_from_the_durable_tile_registers() {
    let unseeded = PlanCollector::seeded(RawDpcCarryIn {
        draw: RdpDrawState {
            other_mode: None,
            combine: None,
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::from_wire(0, 0),
            fog_color: Color4::from_wire(0),
            scissor: None,
            color_image: None,
            tiles: [(None, None); 8],
            prim_depth: None,
        },
    });
    assert!(
        unseeded
            .draw
            .tiles
            .iter()
            .all(|(descriptor, size)| descriptor.is_none() && size.is_none()),
        "an unseeded collector must invent no tile -- a zeroed default would silently \
         sample TMEM word zero"
    );

    // A real durable tile, taken from a backend that actually issued
    // `SetTile`/`SetTileSize`, never a hand-built struct: the seed path
    // under test is `durable_neutral_tiles(&rdp_state)`, so building the
    // input by hand would test the converter and not the carry.
    // **Every field distinct and nonzero**, borrowed field for field
    // from `raw_dpc`'s own `tmem_state_commands_decode_every_public_
    // field_width_for_all_prefixes`. This matters: with a tile whose
    // `mask_s`, `mask_t` and `low_s` were all zero, a converter that
    // swapped S for T or dropped a field produced an identical result
    // and the round-trip below passed. Measured -- two mutants survived
    // exactly that fixture.
    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    let words = vec![
        word(SET_TILE, 4 << 21 | 3 << 19 | 0x01ab << 9 | 0x01fe),
        5 << 24 | 0x0f << 20 | 3 << 18 | 0x0a << 14 | 0x0b << 10 | 1 << 8 | 0x0c << 4 | 0x0d,
        word(SET_TILE_SIZE_OPCODE, 0x0fed << 12 | 0x0cba),
        5 << 24 | 0x0abc << 12 | 0x0789,
    ];
    let planned = plan_with_no_reads(&mut backend, &session, words);
    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();
    let _ = backend.execute_raw_dpc(bound);

    let tiles = durable_neutral_tiles(&backend.rdp_state);
    assert!(
        tiles[5].0.is_some() && tiles[5].1.is_some(),
        "the fixture must actually leave tile 5 durable, or the seed assertion below is \
         vacuous -- got {:?}",
        tiles[5]
    );

    // **Round-trip, so the converters cannot permute or drop a field.**
    //
    // `neutral_tile_descriptor`/`neutral_tile_size` are the inverses of
    // `TexrectTileBinding::try_from_neutral`'s own decode. Feeding the
    // neutral output back through that decode must reproduce the typed
    // value the durable register actually holds -- an equality on the
    // neutral tuples alone would pass with `mask_s` and `mask_t`
    // swapped, or `low_s` zeroed, because both sides would carry the
    // same wrong value. Measured: those two mutants survived until this
    // assertion existed.
    let durable_tile = backend
        .rdp_state
        .tmem()
        .tile(crate::TileIndex::try_new(5).unwrap());
    let round_tripped = crate::targets::TexrectTileBinding::try_from_neutral(
        tiles[5].0.expect("checked above"),
        tiles[5].1.expect("checked above"),
    )
    .expect("a durable tile round-trips through the neutral mirror");
    assert_eq!(
        round_tripped.descriptor(),
        durable_tile.descriptor().expect("checked above"),
        "the neutral descriptor must decode back to the durable register field for field"
    );
    assert_eq!(
        round_tripped.size(),
        durable_tile.size().expect("checked above"),
        "the neutral tile size must decode back to the durable register field for field"
    );

    // Hand-derived from the wire words above, so the round-trip is
    // checked against the RDP's own field layout rather than against
    // whatever the converter happened to produce. S and T carry
    // different values in every pair, which is what makes a swap
    // observable.
    let neutral = tiles[5].0.expect("checked above");
    assert_eq!(neutral.mask_s, 0x0c, "mask_s is w1 bits 7:4");
    assert_eq!(neutral.mask_t, 0x0a, "mask_t is w1 bits 17:14");
    assert_eq!(neutral.shift_s, 0x0d, "shift_s is w1 bits 3:0");
    assert_eq!(neutral.shift_t, 0x0b, "shift_t is w1 bits 13:10");
    assert!(neutral.s_mode.mirror && !neutral.s_mode.clamp);
    assert!(neutral.t_mode.mirror && neutral.t_mode.clamp);
    assert_eq!(neutral.line_words, 0x01ab);
    assert_eq!(neutral.tmem_word_address, 0x01fe);
    assert_eq!(neutral.palette, 0x0f);
    // Format and pixel size, hand-derived from w0 bits 23:21 and 20:19
    // above: the enum converters are total match arms and a wrong arm
    // is otherwise invisible, since the round-trip decodes with the
    // inverse of whatever this produced.
    assert!(
        matches!(neutral.format, fn64_render::NeutralImageFormat::Intensity),
        "format is w0 bits 23:21 == 4, got {:?}",
        neutral.format
    );
    assert!(
        matches!(neutral.size, fn64_render::NeutralPixelSize::Bits32),
        "pixel size is w0 bits 20:19 == 3, got {:?}",
        neutral.size
    );
    let neutral_size = tiles[5].1.expect("checked above");
    assert_eq!(neutral_size.low_s, 0x0fed, "low_s is w0 bits 23:12");
    assert_eq!(neutral_size.low_t, 0x0cba, "low_t is w0 bits 11:0");
    assert_eq!(neutral_size.high_s, 0x0abc, "high_s is w1 bits 23:12");
    assert_eq!(neutral_size.high_t, 0x0789, "high_t is w1 bits 11:0");
    let seeded = PlanCollector::seeded(RawDpcCarryIn {
        draw: RdpDrawState {
            other_mode: None,
            combine: None,
            blend_color: Color4::from_wire(0),
            env_color: Color4::from_wire(0),
            prim_color: PrimColor::from_wire(0, 0),
            fog_color: Color4::from_wire(0),
            scissor: None,
            color_image: None,
            tiles: tiles,
            prim_depth: None,
        },
    });
    assert_eq!(
        seeded.draw.tiles[5], tiles[5],
        "a collector seeded from durable state must start with tile 5 already bound, \
         so a packet that re-declares no tile still resolves one"
    );
    assert!(
        seeded.draw.tiles[0].0.is_none(),
        "seeding must carry only the tiles the guest actually set, never widen to all eight"
    );
}

/// **A draw standing before its packet's own `SetTile` must carry the
/// PREVIOUS packet's tile, not this packet's later one.**
///
/// `plan_raw_dpc` and `execute_raw_dpc` are two trait calls for one
/// submission, and the first folds the whole packet's `RdpStateDelta`
/// into `rdp_state`. Seeding `PlanCollector`'s in-order walk from
/// `rdp_state` at that point starts it from the packet's FINAL tile
/// table, so every command before the packet's first `SetTile` reads a
/// register the guest had not set yet -- time-travelled state.
///
/// Measured on WM2000: the ROM emits each sprite strip as
/// `SetTile(7) -> LoadTile -> SetTile(0) -> triangle -> triangle`, and
/// a packet boundary fell between one strip's two triangles. The
/// orphaned triangle, at command index 0, was bound to the packet's
/// later `line_words = 4` tile instead of the carried-in
/// `line_words = 5`, walked its rows at a 32-byte stride through a
/// 40-byte-stride image, and hit the load's undefined row-tail padding
/// -- `TMEM_SAMPLE_STATUS_INVALID_BYTE`, the abort this repairs.
///
/// **Asserted on the binding the walk actually produced, never on the
/// snapshot field itself.** An earlier draft read
/// `backend.raw_dpc_carry_in_before_last_plan` directly; a mutant that recorded
/// the snapshot correctly and then had the executor ignore it (reading
/// the live, already-folded `rdp_state` instead -- exactly the
/// pre-repair behaviour) SURVIVED that draft. Seeding the collector
/// from `WgpuBackend`'s own choice of table, and reading the resulting
/// `RetrievedTriangleDraw`, is what kills it: that is the value the
/// shader is handed.
///
/// A **raw triangle** is the draw, not a texrect: a raw triangle binds
/// tile 0 and declares no journal write access, so the packet needs no
/// resident color target and the assertion is not gated behind fill
/// bookkeeping this defect has nothing to do with.
///
/// The two `line` values are DIFFERENT and hand-chosen (`3` carried in,
/// `6` set later) so the assertion distinguishes the two tables rather
/// than passing against either. `set_tile`'s own wire layout puts
/// `line` in w0 bits 17:9, so these are the values that field carries.
#[test]
fn a_draw_before_its_packets_first_set_tile_carries_the_previous_packets_tile() {
    const CARRIED_IN_LINE: u32 = 3;
    const SET_LATER_IN_PACKET_LINE: u32 = 6;

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);

    // Packet one: establish tile 0 with the carry-in `line`.
    let mut first = Vec::new();
    first.extend(set_other_mode(0, 0));
    first.extend(set_combine(0, 0));
    first.extend(set_tile(0, CARRIED_IN_LINE, 0));
    first.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
    backend
        .plan_raw_dpc(session.plan_request(capture(first)))
        .expect("the tile-establishing submission plans cleanly");

    assert_eq!(
        durable_neutral_tiles(&backend.rdp_state)[0]
            .0
            .expect("packet one bound tile 0")
            .line_words,
        CARRIED_IN_LINE as u16,
        "positive control: durable state must really carry the first packet's tile, \
         or this test would pass vacuously against a table that never held it"
    );

    // Packet two, in the WM2000 order that exposed the defect: the
    // TRIANGLE COMES FIRST, before this packet's own `SetTile`. Its
    // only tile binding at its own stream position is packet one's.
    let mut second = Vec::new();
    second.extend(set_other_mode(0, 0));
    second.extend(set_combine(0, 0));
    second.extend(triangle_base_edge_words(0, 2, 0));
    second.extend(set_tile(0, SET_LATER_IN_PACKET_LINE, 0));
    second.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
    let planned = backend
        .plan_raw_dpc(session.plan_request(capture(second)))
        .expect("the triangle-then-SetTile submission plans cleanly");

    // After the fold, the LIVE registers hold the new value. This is
    // the discriminator: if the walk seeded from here, the triangle
    // would come back bound to `SET_LATER_IN_PACKET_LINE`.
    assert_eq!(
        durable_neutral_tiles(&backend.rdp_state)[0]
            .0
            .expect("packet two bound tile 0")
            .line_words,
        SET_LATER_IN_PACKET_LINE as u16,
        "positive control: the fold must really have happened, otherwise the walk \
         could read the live registers and still look correct"
    );

    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    // Seeded from `WgpuBackend`'s OWN choice of table -- the same
    // expression `execute_raw_dpc` uses -- so a mutant that leaves the
    // snapshot correct but has the executor ignore it is still caught.
    let seed = backend
        .raw_dpc_carry_in_before_last_plan
        .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state));
    let mut plan_visitor = PlanCollector::seeded(seed);
    let mut color_targets = None;
    let configured_target_extent = backend.configured_target_extent;
    let coordinator = &backend.coordinator;
    let mut view = ExecutionCollector {
        physical: coordinator.physical(),
        queue: bound.queue(),
        ordinal: bound.ordinal(),
        submission: bound.submission(),
        plan: PlanCollector::seeded(seed),
        reads: CapturedGuestReadAuthority::default(),
        task_guest_read_pool: None,
        outcome: None,
        color_targets: &mut color_targets,
        configured_target_extent,
        draw_tmem: None,
        project_gpu_tmem: true,
        collect_compute_probe: false,
        compute_probes: Vec::new(),
        compute_replacement_enabled: false,
        compute_replacement_pipeline: None,
        compute_replacement_receipt: None,
        color_execution_batch: None,
        ordered_cpu_color_batch: None,
        task_cpu_phase_census: None,
        defer_compute_replacement: false,
        deferred_compute: None,
    };
    coordinator.execution_view(&bound, &mut plan_visitor, &mut view);

    assert_eq!(
        view.plan.triangles.len(),
        1,
        "positive control: the packet must admit its one raw triangle -- if it admitted \
         none, the binding assertion below would be vacuous"
    );
    let draw = view.plan.triangles.into_iter().next().unwrap().draw;
    let draw = draw.expect("the admitted triangle retrieves its draw state");
    assert_eq!(
        draw.tile_binding.bound, 1,
        "the triangle must resolve a tile at all"
    );
    assert_eq!(
        draw.tile_binding.line_words, CARRIED_IN_LINE,
        "the triangle stands BEFORE its packet's own SetTile, so it must carry the tile \
         packet ONE set (line {CARRIED_IN_LINE}), never the one packet two installs after \
         it (line {SET_LATER_IN_PACKET_LINE}) -- seeding from the already-folded \
         `rdp_state` is what time-travelled WM2000's orphaned strip triangle onto a stride \
         its own load never wrote"
    );
}

/// **A REJECTED plan must not replace the snapshot.**
///
/// `raw_dpc_carry_in_before_last_plan` is recorded on `plan_raw_dpc`'s success
/// path only, after `plan_raw_dpc_inner` returns `Ok`. Moving it above
/// that call would record a snapshot for a packet that never executes,
/// so the next `execute_raw_dpc` -- which belongs to whichever
/// submission planned last SUCCESSFULLY -- would be seeded from the
/// wrong boundary.
///
/// This is the arm the repair KEEPS rather than the one it changed, and
/// it had no test: the mutant that hoists the assignment above the
/// fallible call survived every other assertion in this file.
///
/// The rejected packet's `SetTile` uses a `line` that appears nowhere
/// else, so a snapshot taken from the wrong side is distinguishable
/// from both the surviving value and any default.
#[test]
fn a_rejected_plan_leaves_the_previous_submissions_tile_snapshot_in_place() {
    const SURVIVING_LINE: u32 = 3;

    let (mut backend, session) = WgpuBackend::try_new().unwrap();

    // One submission that plans cleanly and sets tile 0.
    let mut good = Vec::new();
    good.extend(set_other_mode(0, 0));
    good.extend(set_combine(0, 0));
    good.extend(set_tile(0, SURVIVING_LINE, 0));
    good.extend(set_tile_size_words(0, 7 << 2, 2 << 2));
    backend
        .plan_raw_dpc(session.plan_request(capture(good)))
        .expect("the tile-establishing submission plans cleanly");

    let after_success = backend
        .raw_dpc_carry_in_before_last_plan
        .expect("a successful plan records a snapshot");
    assert!(
        after_success.draw.tiles[0].0.is_none(),
        "positive control: this FIRST plan's own snapshot is the state before it ran, \
         which bound no tile -- if it already carried one, the comparison below could \
         not tell a preserved snapshot from a re-taken one"
    );

    // A submission that is rejected at plan time. `FullSync` alongside
    // a fill is refused (see the T-13 test above), and this stream also
    // carries a `SetTile` -- so a snapshot taken before the fallible
    // call would still differ from the one above, by now holding the
    // tile the FIRST submission set.
    let mut bad = partial_width_fill_words();
    bad.extend(set_tile(0, SURVIVING_LINE + 1, 0));
    bad.extend([word(FULL_SYNC, 0), 0]);
    assert!(
        backend
            .plan_raw_dpc(session.plan_request(capture(bad)))
            .is_err(),
        "positive control: this submission must really be rejected, or the assertion \
         below would be testing the success path twice"
    );

    assert_eq!(
        backend
            .raw_dpc_carry_in_before_last_plan
            .expect("the snapshot must still be present after a rejected plan"),
        after_success,
        "a rejected plan must leave the last SUCCESSFUL submission's snapshot untouched. \
         Recording it before `plan_raw_dpc_inner` would stamp a boundary for a packet \
         that never executes, and the next execute_raw_dpc would seed its tile walk \
         from it"
    );
}

/// **A draw standing before its packet's own `SetOtherMode` must carry
/// the PREVIOUS packet's mode, not this packet's later one.**
///
/// The sibling of
/// `a_draw_before_its_packets_first_set_tile_carries_the_previous_packets_tile`,
/// on the register `f2c52822` explicitly declined to widen its repair
/// to for want of a measurement. This is that measurement, taken on the
/// real WM2000 ROM on the all-Rust stack.
///
/// A packet folded `other_mode.high` from `0x00000cef` to `0x0008acef`.
/// `G_MDSFT_TEXTLUT` is bits 15:14, so the carried-in word selects
/// `G_TT_NONE` and the packet-final word selects `G_TT_RGBA16`. The
/// packet's FIRST texrect, at command index 6, stood before that
/// `SetOtherMode` and was nonetheless seeded with the folded word.
///
/// Under an enabled TLUT the RDP indexes any format through the palette
/// and confines that read to half of TMEM (RT64
/// `TextureDecoder.hlsli:162-163`). So the texrect's `Rgba`/`Bits16`
/// texel at linear byte `0x884` was masked to `0x084` and XOR4'd to
/// `0x080` instead of staying at `0x884`/`0x880`. `0x880` was loaded;
/// `0x080` never was, and `InvalidTexelByte` correctly aborted the run
/// at 280 VI swaps.
///
/// **Behavioural, not field-reading.** The assertion drives
/// `execution_view` and reads the mode off the CONSUMER's retrieved
/// draw state, seeding exactly the way `execute_raw_dpc` does. A mutant
/// that records the right snapshot while the executor still reads the
/// live register is therefore caught -- the trap the tile-side test's
/// first draft fell into.
///
/// The two `TEXTLUT` encodings are DIFFERENT and hand-chosen (`0`
/// carried in, `2` set later) so the assertion distinguishes the two
/// words rather than passing against either.
#[test]
fn a_draw_before_its_packets_first_set_other_mode_carries_the_previous_packets_mode() {
    // `G_MDSFT_TEXTLUT` is bits 15:14 of `G_SETOTHERMODE_H`.
    const TEXTLUT_SHIFT: u32 = 14;
    const CARRIED_IN_TEXTLUT: u32 = 0; // G_TT_NONE
    const SET_LATER_IN_PACKET_TEXTLUT: u32 = 2; // G_TT_RGBA16

    // `set_other_mode`'s own helper only writes the cycle-type field,
    // so the high word is built here from the same `word()` encoder it
    // uses, with TEXTLUT placed by its documented shift.
    fn other_mode_with_textlut(textlut: u32) -> [u32; 2] {
        [word(SET_OTHER_MODE, textlut << TEXTLUT_SHIFT), 0]
    }

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();

    // Packet one: establish the carried-in mode (TLUT off).
    let mut first = Vec::new();
    first.extend(other_mode_with_textlut(CARRIED_IN_TEXTLUT));
    first.extend(set_combine(0, 0));
    backend
        .plan_raw_dpc(session.plan_request(capture(first)))
        .expect("the mode-establishing submission plans cleanly");

    assert_eq!(
        backend
            .rdp_state
            .other_mode()
            .expect("packet one set the mode")
            .texture_lut_mode(),
        crate::TextureLutMode::Disabled,
        "positive control: durable state must really carry the first packet's TLUT-off \
         mode, or this test would pass vacuously against a register that never held it"
    );

    // Packet two: a triangle FIRST, then a `SetOtherMode` turning the
    // TLUT on. Planning folds that `SetOtherMode` into `rdp_state`
    // immediately, so the live register no longer describes the
    // triangle's own stream position.
    let mut second = Vec::new();
    second.extend(triangle_base_edge_words(0, 2, 0));
    second.extend(other_mode_with_textlut(SET_LATER_IN_PACKET_TEXTLUT));
    let planned = backend
        .plan_raw_dpc(session.plan_request(capture(second)))
        .expect("the triangle-then-SetOtherMode submission plans cleanly");

    // The discriminator: if the walk seeded from the live registers,
    // the triangle would come back TLUT-enabled.
    assert_eq!(
        backend
            .rdp_state
            .other_mode()
            .expect("packet two set the mode")
            .texture_lut_mode(),
        crate::TextureLutMode::Rgba16,
        "positive control: the fold must really have happened, otherwise the walk could \
         read the live registers and still look correct"
    );

    let bound = finalize_and_submit_pair(&mut session, planned).unwrap();

    // **Driven through `execute_raw_dpc_inner`, the function
    // `execute_raw_dpc` delegates to**, with every seed taken from
    // `backend`'s own fields exactly as that method takes them. The
    // retrieved draws it returns are the consumer's own output, so a
    // mutant that records a faithful snapshot and then passes
    // `self.rdp_state.other_mode()` at the call site changes THIS
    // value and is caught.
    //
    // Reading `backend.raw_dpc_carry_in_before_last_plan` in the test instead
    // and handing it to a locally-built `PlanCollector` does NOT catch
    // that mutant -- measured: the first draft did exactly that and the
    // consumer-side mutant passed. This is the trap the tile-side
    // sibling documents at
    // `execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot`.
    let mut color_targets = None;
    let (_, triangles, _, _, _, _) = execute_raw_dpc_inner(
        &mut backend.coordinator,
        bound,
        backend
            .raw_dpc_carry_in_before_last_plan
            .unwrap_or_else(|| RawDpcCarryIn::capture(&backend.rdp_state)),
        &mut color_targets,
        backend.configured_target_extent,
        true,
        false,
        false,
        None,
        None,
        None,
        None,
    )
    .expect("the triangle-then-SetOtherMode submission executes cleanly");

    assert_eq!(
        triangles.len(),
        1,
        "positive control: the packet must admit its one raw triangle -- if it admitted \
         none, the mode assertion below would be vacuous"
    );
    let draw = triangles
        .into_iter()
        .next()
        .unwrap()
        .expect("the admitted triangle retrieves its draw state");
    assert_eq!(
        draw.other_mode.texture_lut_mode(),
        crate::TextureLutMode::Disabled,
        "the triangle stands BEFORE its packet's own SetOtherMode, so it must carry the \
         TLUT-off mode packet ONE set, never the G_TT_RGBA16 one packet two installs \
         after it -- seeding from the already-folded `rdp_state` is what sent WM2000's \
         first texrect down the enabled-TLUT half-TMEM address path and made it read \
         byte 0x080, which its own load never wrote"
    );
}

/// **`execute_raw_dpc` must seed `other_mode` from the SNAPSHOT, never
/// from the live register.**
///
/// The sibling of
/// `execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot`,
/// and it exists for the identical measured reason.
///
/// The behavioural test above proves the snapshot holds the right word
/// and that a walk seeded from it retrieves the right mode. It cannot
/// prove `execute_raw_dpc` is the thing doing the seeding:
/// `RetrievedTriangleDraw` is not reachable through the
/// `RenderBackend` trait, so a mutant that records the snapshot
/// faithfully and then passes `self.rdp_state.other_mode()` -- exactly
/// the pre-repair line -- SURVIVES it. **Measured: it did.** The first
/// draft of the test above passed unchanged against that mutant.
///
/// Pinned at the source instead, because the fact under test is which
/// expression appears at one call site.
///
/// Both halves are asserted for the same reason the tile sibling
/// asserts both: `contains` alone would pass a body that read the
/// snapshot *and* also read the live register unconditionally, and the
/// count pins that the only bare `self.rdp_state.other_mode()` in this
/// function is the `unwrap_or_else` fallback reached before any plan
/// has run.
#[test]
fn execute_raw_dpc_seeds_other_mode_from_the_pre_delta_snapshot() {
    let source = include_str!("../../production/state.rs");
    let body_start = source
        .find("    fn execute_raw_dpc(")
        .expect("execute_raw_dpc must exist in this file");
    let next_fn = source[body_start + 1..]
        .find("\n    fn ")
        .map(|offset| body_start + 1 + offset)
        .unwrap_or(source.len());
    let body = &source[body_start..next_fn];
    assert!(
        body.contains("self.raw_dpc_carry_in_before_last_plan"),
        "execute_raw_dpc must seed `other_mode` from the pre-delta snapshot \
         `raw_dpc_carry_in_before_last_plan`. Reading `rdp_state` directly reads the packet's \
         own already-folded SetOtherModes, which ran WM2000's first texrect under a \
         G_TT_RGBA16 the guest had not set yet and sent it down the enabled-TLUT \
         half-TMEM address path"
    );
    assert_eq!(
        body.matches("RawDpcCarryIn::capture(&self.rdp_state)")
            .count(),
        1,
        "the only live-state read in execute_raw_dpc must build the complete typed fallback",
    );
}

/// **A REJECTED plan must not replace the `other_mode` snapshot.**
///
/// The arm the repair KEEPS, pinned for the same reason the tile
/// sibling `a_rejected_plan_leaves_the_previous_submissions_tile_snapshot_in_place`
/// pins its own: `raw_dpc_carry_in_before_last_plan` is assigned after
/// `plan_raw_dpc_inner` returns `Ok`, and hoisting it above that
/// fallible call would stamp a boundary for a packet that never
/// executes.
///
/// The surviving and the rejected packets select DIFFERENT TEXTLUT
/// encodings, so a snapshot taken from the wrong side is
/// distinguishable from the surviving value.
#[test]
fn a_rejected_plan_leaves_the_previous_submissions_other_mode_snapshot_in_place() {
    const TEXTLUT_SHIFT: u32 = 14;

    let (mut backend, session) = WgpuBackend::try_new().unwrap();

    let mut first = Vec::new();
    first.extend([word(SET_OTHER_MODE, 0 << TEXTLUT_SHIFT), 0]);
    first.extend(set_combine(0, 0));
    backend
        .plan_raw_dpc(session.plan_request(capture(first)))
        .expect("the mode-establishing submission plans cleanly");
    let after_success = backend
        .raw_dpc_carry_in_before_last_plan
        .expect("a successful plan records the pre-delta other-mode snapshot");

    // A submission that is rejected at plan time -- `FullSync`
    // alongside a fill, the same refusal the tile sibling uses. It also
    // carries its own `SetOtherMode` with a TEXTLUT encoding that
    // appears nowhere else, so a snapshot taken from the wrong side of
    // the fallible call is distinguishable from the surviving one.
    let mut bad = partial_width_fill_words();
    bad.extend([word(SET_OTHER_MODE, 3 << TEXTLUT_SHIFT), 0]);
    bad.extend([word(FULL_SYNC, 0), 0]);
    assert!(
        backend
            .plan_raw_dpc(session.plan_request(capture(bad)))
            .is_err(),
        "positive control: this submission must really be rejected, or the assertion \
         below would be testing the success path twice"
    );

    assert_eq!(
        backend
            .raw_dpc_carry_in_before_last_plan
            .expect("the snapshot must still be present after a rejected plan"),
        after_success,
        "a rejected plan must leave the last SUCCESSFUL submission's other-mode \
         snapshot untouched. Recording it before `plan_raw_dpc_inner` would stamp a \
         boundary for a packet that never executes, and the next execute_raw_dpc \
         would seed its walk from it"
    );
}

/// **`execute_raw_dpc` must seed the walk from the SNAPSHOT, never
/// from the live registers.**
///
/// The behavioural test above proves the snapshot holds the right
/// table and that a walk seeded from it binds the right tile. It
/// cannot prove `execute_raw_dpc` is the thing doing the seeding:
/// `RetrievedTriangleDraw` is not reachable through the
/// `RenderBackend` trait, so a mutant that records the snapshot
/// faithfully and then passes `durable_neutral_tiles(&self.rdp_state)`
/// -- exactly the pre-repair line -- SURVIVES it. Measured: it did.
///
/// Pinned at the source instead, the same way
/// `plan_raw_dpc_inner_decodes_both_passes_against_durable_state_not_default`
/// pins its own two-pass choice, because the fact under test is which
/// expression appears at one call site.
///
/// Both halves are asserted. The `contains` alone would pass a body
/// that read the snapshot *and* also fell back to the live table
/// unconditionally; the count of bare `durable_neutral_tiles(&self`
/// calls pins that the ONLY such call in this function is the
/// `unwrap_or_else` fallback, which is reached only before any plan
/// has run.
#[test]
fn execute_raw_dpc_seeds_the_tile_walk_from_the_pre_delta_snapshot() {
    let source = include_str!("../../production/state.rs");
    let body_start = source
        .find("    fn execute_raw_dpc(")
        .expect("execute_raw_dpc must exist in this file");
    let next_fn = source[body_start + 1..]
        .find("\n    fn ")
        .map(|offset| body_start + 1 + offset)
        .unwrap_or(source.len());
    let body = &source[body_start..next_fn];
    assert!(
        body.contains("self.raw_dpc_carry_in_before_last_plan"),
        "execute_raw_dpc must seed its tile walk from the pre-delta snapshot \
         `raw_dpc_carry_in_before_last_plan`. Reading `rdp_state` directly reads the packet's own \
         already-folded SetTiles, which binds every draw standing before the packet's \
         first SetTile to a register the guest had not set yet"
    );
    assert_eq!(
        body.matches("RawDpcCarryIn::capture(&self.rdp_state)")
            .count(),
        1,
        "the only live-state read in execute_raw_dpc must build the complete typed fallback",
    );
}

/// **RETARGET, and the anti-workaround half of this change.** This was
/// `a_fill_composed_with_a_raw_triangle_is_still_refused_by_name`,
/// asserting `MixedFillAndTrianglePacket`. The gate is gone.
///
/// It is retargeted rather than deleted because the concern it was
/// standing in for is real and must still be enforced: a colour-target
/// command must never write bytes this packet's own journal did not
/// declare, and the journal must never declare bytes no command wrote.
/// The shape gate approximated that per-packet; the invariant is checked
/// per-access, by `merged_fill_and_tmem_writes`
/// (`MergedWriteUnclaimed`/`MergedWriteUndeclared`) and by
/// `fill_completed_writes` (`FillAccessOutsideTarget`). Those are the
/// checks that actually protect this seam, and they are the ones a
/// reader arriving from the removed gate needs pointed at.
///
/// Asserted here on the **fill + declared-write raw triangle** pair --
/// WM2000's own measured shape, and the pair the removed gate refused --
/// through the strongest observable this module has: the journal's
/// declared render-target ranges, hand-derived, compared against the
/// staged writes one for one.
///
/// FAILS BEFORE this change: `plan_and_execute_fill` returns
/// `MixedFillAndTrianglePacket` and nothing is staged.
#[test]
fn a_fill_and_a_declaring_raw_triangle_stage_exactly_the_writes_the_journal_declares() {
    let (low, high) =
        crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_PRIMITIVE);
    let mut words = whole_target_fill_words();
    words.extend(set_other_mode(0, 0));
    words.extend(set_combine(low, high));
    words.extend(set_prim_color(0, 0, TRIANGLE_PRIM_WIRE));
    words.extend(flat_triangle_in_target_words());

    // **The expectation, hand-derived from the wire layout alone.**
    //
    // Deliberately NOT read from `declared_render_target_writes`: that
    // helper re-decodes against `RdpState::default()`, which carries no
    // `color_target_height`, and `plan_raw_triangle` declines to declare
    // a row when the height is unknown. Measured, it therefore reports
    // the fill's access only. That is correct for the state it decodes
    // against and wrong as an oracle for a backend that HAS had
    // `create` called on it, so the expectation is derived here instead.
    //
    // The fill covers the whole 16x8 RGBA16 target at 0x2000: one
    // contiguous 256-byte range (`plan_fill`'s `x0 == 0 && x1 + 1 ==
    // width` collapse). The triangle covers rows 0..3, columns 2..6
    // (see `flat_triangle_in_target_words`' own derivation), so it
    // declares one 8-byte run per scanline at
    // 0x2000 + (16y + 2) * 2 = 0x2004, 0x2024, 0x2044. Four accesses,
    // in stream order -- the fill's first, because it is the earlier
    // wire command.
    let declared = vec![
        (
            FILL_TARGET_ADDRESS,
            FILL_TARGET_WIDTH * FILL_TARGET_HEIGHT * 2,
        ),
        (0x2004, 8),
        (0x2024, 8),
        (0x2044, 8),
    ];

    let (mut backend, mut session) = WgpuBackend::try_new().unwrap();
    configure_fill_target_height(&mut backend);
    let (submission, result) = plan_and_execute_fill(&mut backend, &mut session, words);
    let Ok(_prepared) = result else {
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("TriangleDrawBeforeCreate")
                || error.to_string().contains("no GPU adapter"),
            "the only tolerated failure is the adapterless GPU raster path, got: {error}"
        );
        return;
    };

    // Every declared access is claimed by exactly one staged write, in
    // the journal's own order. `merged_fill_and_tmem_writes` and
    // `BackendEffectReport::try_new` both already refuse any other
    // outcome by name; this asserts the outcome they permit is the one
    // hand-derived above, not merely self-consistent.
    let staged = backend.staged_guest_render_target_writes(submission);
    let staged_ranges: Vec<(u32, u32)> = staged
        .iter()
        .map(|write| match write.access().region() {
            fn64_render_ir::ResourceRegion::Rdram { range, .. } => {
                (range.start().get(), range.len())
            }
            other => panic!("a render-target write must name an RDRAM range, got {other:?}"),
        })
        .collect();
    assert_eq!(
        staged_ranges, declared,
        "the staged writes must be exactly the journal's declared render-target accesses, \
         in the journal's order -- no extra write the journal never declared, and no \
         declared write left unstaged"
    );
}
