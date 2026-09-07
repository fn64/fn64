use super::*;

/// Run the persistent RSP state from its admitted PC through BREAK, resolving
/// every IMEM overlay generation and forwarding DPC work to the renderer.
/// This is the universal clean-room path for custom/unknown task types.
pub(crate) unsafe fn dispatch_lle_task(
    rdram: *mut u8,
    // `None` is a raw `SP_STATUS` clear-halt kick: the RSP was started directly
    // through MMIO with no `OSTask` behind it.
    task_addr: Option<RdramAddr>,
    recognize_graphics_microcode: bool,
    machine_state: Option<fn64_audio::rsp::runtime::RspMachineState>,
    microcode_data: Option<TaskMicrocodeDataIdentity>,
    authoritative_family: Option<fn64_render::UcodeId>,
    mut guest_task_observation: Option<crate::render_observation::PendingGuestTaskObservation>,
) -> LleTaskResult {
    const CHUNK_STEPS: u64 = 1 << 20;
    const MAX_TASK_STEPS: u64 = 1 << 26;

    // The HLE renderer records its own preflight boundary. Graphics LLE then
    // bypasses that boundary and forwards raw DPC work directly, so record the
    // whole recognized LLE phase here and retain a separate breakdown from
    // the aggregate graphics-phase total.
    let gfx_started = (recognize_graphics_microcode && PHASE_TIMING.with(Cell::get))
        .then(std::time::Instant::now);
    // NON-graphics LLE -- in the WM2000 block lane that is the audio ucode,
    // which runs under `AudioTaskExecutionPolicy::LleAccuracy` and therefore
    // never reaches `dispatch_audio_task` (the only site that feeds
    // `AUDIO_DISPATCH_NS`). Untimed, its cost silently joined "executor self"
    // and read as runtime overhead. Time it here so the phase split accounts
    // for every task the RSP interpreter runs, not just the graphics ones.
    let non_gfx_started = (!recognize_graphics_microcode && PHASE_TIMING.with(Cell::get))
        .then(std::time::Instant::now);
    let mut rsp_execution_ns = 0u64;
    let mut raw_rdp_ns = 0u64;

    let (mut dmem, mut imem, mut pc, imem_generation, rdram_len, static_aliases) =
        with_host(|host| {
            let fabric = &host.device_fabric;
            (
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
                fabric.sp_pc(),
                fabric.rsp_memory().imem_generation(),
                host.runtime_rdram_len,
                host.sections.loaded_static_storage_ranges(),
            )
        });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP LLE task has no registered process RDRAM allocation"
    );
    let owner = match task_addr {
        Some(task_addr) => task_interpreter_owner(task_addr),
        None => acquire_raw_kick_interpreter_owner(),
    };
    let initial_imem = imem;
    unsafe { trace_rsp_rdram_words(rdram, rdram_len) };
    let (dma_ranges, _) = rsp_dma_storage_layout(rdram_len, static_aliases);
    #[cfg(feature = "recomp-rs")]
    let catalog_writer = crate::recompiled::begin_catalog_nested_writer(
        unsafe { std::slice::from_raw_parts(rdram, rdram_len) },
        "RSP LLE execution",
    );
    // Execute directly against the live allocation. RSP stores are bounded by
    // the admitted DMA ranges and logged by the machine for post-run JIT
    // invalidation; no full-allocation snapshot or memcmp is needed.
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.set_dma_rdram_ranges(dma_ranges);
    machine.load_dmem_logical(&dmem);
    if let Some(state) = machine_state {
        continue_rsp_interpreter_phase(owner, &mut machine, state);
    } else {
        begin_rsp_interpreter_phase(owner, &mut machine);
    }
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    let mut replacements = Vec::new();
    let debug_dir =
        crate::diag_env::diag_env("FN64_RSP_LLE_DEBUG_DIR").map(std::path::PathBuf::from);
    const DEBUG_TAIL_STEPS: u64 = 1 << 16;
    const DEBUG_PC_RING: usize = 4096;
    let debug_initial = debug_dir.as_ref().map(|_| (dmem, imem, pc));
    let mut debug_pc_ring = std::collections::VecDeque::with_capacity(DEBUG_PC_RING);
    trace_rsp_dmem_words(&machine.dmem_logical(), overlays, pc);
    loop {
        let words: Vec<u32> = imem
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
            .collect();
        let chunk = if debug_dir.is_some() {
            let tail_start = MAX_TASK_STEPS - DEBUG_TAIL_STEPS;
            if total_steps >= tail_start {
                1
            } else {
                CHUNK_STEPS.min(tail_start - total_steps)
            }
        } else {
            CHUNK_STEPS
        };
        // Arm on EITHER branch. This used to read `gfx_started.map(..)`, which
        // is `None` on the audio branch, so `rsp_execution_ns` stayed 0 there
        // and the `AUDIO_LLE_RSP_NS` accumulation at the bottom of this
        // function (:384) -- already written, already plumbed through to
        // `audio_lle_rsp_ms` -- reported 0.000 on every run since it was added.
        //
        // That dead timer is why "the audio RSP path runs at 8.3 ns/instr"
        // circulated: with no audio measurement to divide by, the figure was
        // built from the GRAPHICS numerator over the COMBINED gfx+audio step
        // count. Same numerator, two denominators -- perf-method rule 2's
        // error in a new costume. This makes the comparison possible.
        //
        // `PHASE_TIMING` gates both `gfx_started` and `non_gfx_started`, so an
        // unset run still takes no clock read.
        let rsp_started =
            (gfx_started.is_some() || non_gfx_started.is_some()).then(std::time::Instant::now);
        let result = fn64_audio::rsp::run_imem(&words, pc, &mut machine, chunk);
        if let Some(started) = rsp_started {
            rsp_execution_ns = rsp_execution_ns.saturating_add(started.elapsed().as_nanos() as u64);
        }
        // Counts BOTH branches. `rsp_started` above is `gfx_started.map(..)`,
        // so the RSP wall-time sub-timer never arms on the audio branch and
        // `audio_lle_rsp_ms` reads 0.000 on every run despite audio LLE being
        // almost entirely interpretation. These counters do not share that
        // gate, and they answer the question wall time cannot: whether the
        // interpreter is expensive per instruction or merely asked to run an
        // enormous number of them (perf-method rule 3 -- count, do not infer).
        crate::dpc_copy_census::note_rsp_chunk(
            recognize_graphics_microcode,
            result.steps,
            words.len() as u64,
        );
        total_steps = total_steps
            .checked_add(result.steps)
            .expect("RSP task step counter overflow");
        if debug_dir.is_some() && chunk == 1 {
            if debug_pc_ring.len() == DEBUG_PC_RING {
                debug_pc_ring.pop_front();
            }
            debug_pc_ring.push_back(result.pc);
        }
        if total_steps > MAX_TASK_STEPS {
            if let (Some(dir), Some((initial_dmem, initial_imem, initial_pc))) =
                (debug_dir.as_ref(), debug_initial.as_ref())
            {
                dump_lle_debug_state(
                    dir,
                    initial_dmem,
                    initial_imem,
                    *initial_pc,
                    &imem,
                    &machine,
                    result.pc,
                    total_steps,
                    overlays,
                    &debug_pc_ring,
                );
            }
            panic!(
                "RSP task exceeded deterministic {MAX_TASK_STEPS}-instruction admission bound at PC {:#06x}",
                result.pc
            );
        }
        pc = result.pc;
        match result.reason {
            fn64_audio::rsp::RspExitReason::Broke => break,
            fn64_audio::rsp::RspExitReason::SwapOverlay => {
                machine.complete_imem_dma(&mut imem);
                overlays = overlays
                    .checked_add(1)
                    .expect("RSP IMEM overlay generation counter overflow");
                replacements.push(PendingImemReplacement {
                    generation: imem_generation
                        .checked_add(overlays)
                        .expect("RSP IMEM generation evidence overflow"),
                    image: imem,
                });
                trace_rsp_dmem_words(&machine.dmem_logical(), overlays, pc);
            }
            fn64_audio::rsp::RspExitReason::StepLimit => {}
            reason => panic!(
                "RSP LLE task stopped at PC {:#06x} after {total_steps} instructions: {reason:?}",
                result.pc
            ),
        }
    }

    dmem = machine.dmem_logical();
    let mut deferred_dpc_history = machine.take_deferred_dpc_history();
    let raw_dp_submission_count = deferred_dpc_history.submissions().len();
    let dp_submissions = deferred_dpc_history.take_submissions();
    let final_architectural_state = machine.snapshot_architectural_state();
    let guest_task_outcome =
        if final_architectural_state.sp_status() & fn64_runtime::SP_STATUS_YIELDED == 0 {
            crate::GuestTaskOutcome::Completed
        } else {
            crate::GuestTaskOutcome::Yielded
        };
    let rdram_writes = machine.take_rdram_writes();
    drop(machine);

    commit_rsp_rdram_writes(
        RspWriterCommitSourceV1::Interpreter { owner },
        &rdram_writes,
    );
    #[cfg(feature = "recomp-rs")]
    catalog_writer.commit(unsafe { std::slice::from_raw_parts(rdram, rdram_len) });
    commit_rsp_memory_state(
        &dmem,
        &imem,
        overlays,
        rsp_execution_state_from_architectural(&final_architectural_state, pc & 0x0fff),
    );
    let committed_generation = with_host(|host| host.device_fabric.rsp_memory().imem_generation());
    assert_eq!(
        committed_generation,
        imem_generation
            .checked_add(overlays)
            .expect("RSP IMEM generation evidence overflow"),
        "RSP transactional IMEM replacement count diverged from the committed fabric generation"
    );

    let mut observations = Vec::new();
    // Resolve backend-owned recognition before any raw-DPC task batch can
    // move that backend to the worker. The observations remain unpublished
    // until the batch completes, so their guest-visible order is unchanged.
    // LOUD: `fn64.rsp-rdp-observations.v2` types `MicrocodeRecognition` and
    // `ImemReplacementCommitted`'s `task_address` as a non-optional u32 under
    // `deny_unknown_fields`, so a raw SP kick's overlays are NOT REPRESENTED in
    // that stream. Emitting nothing is deliberate: any placeholder address would
    // be indistinguishable from a real task at that offset. Representing them
    // needs a v3 wire and a schema bump. DPC observations carry no task address
    // and are emitted for both owners.
    if let Some(task_addr) = task_addr {
        let recognition_data = if recognize_graphics_microcode {
            Some(microcode_data.unwrap_or_else(|| {
                panic!(
                    "RSP graphics task {:#010x} has no task-start microcode-data identity",
                    task_addr.offset()
                )
            }))
        } else {
            None
        };
        if let Some(data) = recognition_data {
            observations.push(RspRdpObservationKind::MicrocodeRecognition {
                task_addr,
                imem_generation,
                text_sha256: imem_sha256(&initial_imem),
                data_addr: data.addr,
                data_size: data.size,
                data_sha256: data.sha256,
                family: identify_microcode_pair(&initial_imem, data, authoritative_family),
            });
        }
        for replacement in replacements {
            // One 4 KiB IMEM digest serves both observations. They described
            // the same `replacement.image` and nothing mutates it between
            // them, so the two hashes were always equal; computing it once
            // makes that identity structural rather than coincidental.
            let text_sha256 = imem_sha256(&replacement.image);
            observations.push(RspRdpObservationKind::ImemReplacementCommitted {
                task_addr,
                imem_generation: replacement.generation,
                text_sha256,
            });
            if let Some(data) = recognition_data {
                observations.push(RspRdpObservationKind::MicrocodeRecognition {
                    task_addr,
                    imem_generation: replacement.generation,
                    text_sha256,
                    data_addr: data.addr,
                    data_size: data.size,
                    data_sha256: data.sha256,
                    family: identify_microcode_pair(&replacement.image, data, authoritative_family),
                });
            }
        }
    } else {
        assert!(
            !recognize_graphics_microcode && microcode_data.is_none(),
            "a raw SP kick has no OSTask and cannot carry microcode-data identity"
        );
    }

    let coalesced_dp_runs = coalesce_dp_submissions(dp_submissions);
    maybe_report_rsp_dpc_task_shape(task_addr, raw_dp_submission_count, &coalesced_dp_runs);
    let mut dp_full_sync = fn64_render::DpFullSyncStatus::NotReached;
    let mut dpc_observations = Vec::with_capacity(coalesced_dp_runs.len());
    let mut pending_raw_dpc_task_batch = None;
    let mut completed_guest_task_observation = None;
    let trace_limit = rsp_trace_dpc_words_limit();
    let task_batch = coalesced_dp_runs.len() > 1
        && raw_dpc_task_batch_enabled()
        && RAW_DPC_SESSION.with(|cell| cell.borrow().is_some())
        && RENDER_BACKEND.with(|cell| {
            cell.borrow().as_ref().is_some_and(|backend| {
                backend
                    .backend("raw_dpc_task_batch_capability")
                    .raw_dpc_task_batch_capability()
                    == fn64_render::RawDpcTaskBatchCapability::Transactional
            })
        });
    validate_temporal_guest_read_route(
        deferred_dpc_history.before_image_count(),
        coalesced_dp_runs.len(),
        RAW_DPC_SESSION.with(|cell| cell.borrow().is_some()),
        task_batch,
    );
    if task_batch {
        if let Some(limit) = trace_limit {
            for run in &coalesced_dp_runs {
                let traced = &run.words[..run.words.len().min(limit)];
                eprintln!(
                    "[fn64-rsp-dpc] range [{:#010x}, {:#010x}) xbus={} words={traced:08x?}",
                    run.start, run.end, run.xbus
                );
            }
        }
        let started = gfx_started.map(|_| std::time::Instant::now());
        let dispatch = dispatch_raw_dpc_task_batch_via_session(
            rdram,
            coalesced_dp_runs,
            &deferred_dpc_history,
            guest_task_observation
                .take()
                .map(|observation| (observation, guest_task_outcome)),
        );
        if let Some(started) = started {
            raw_rdp_ns = raw_rdp_ns.saturating_add(started.elapsed().as_nanos() as u64);
        }
        match dispatch {
            RawDpcTaskBatchDispatch::Complete(
                full_sync,
                batch_observations,
                render_observation,
                guest_task_observation,
            ) => {
                if let Some(observation) = render_observation {
                    crate::render_observation::record_completed(observation.seal(None));
                }
                dp_full_sync = full_sync;
                dpc_observations.extend(batch_observations);
                completed_guest_task_observation = guest_task_observation;
            }
            RawDpcTaskBatchDispatch::Pending(pending) => {
                pending_raw_dpc_task_batch = Some(pending);
            }
        }
    } else {
        for CoalescedDpRun {
            start,
            end,
            xbus,
            words,
            read_epoch_boundaries,
        } in coalesced_dp_runs
        {
            if let Some(limit) = trace_limit {
                let traced = &words[..words.len().min(limit)];
                eprintln!(
                "[fn64-rsp-dpc] range [{start:#010x}, {end:#010x}) xbus={xbus} words={traced:08x?}"
            );
            }
            let source = if xbus {
                fn64_runtime::DpcSubmissionSource::Dmem
            } else {
                fn64_runtime::DpcSubmissionSource::Rdram
            };
            let submission = with_host(|host| {
                host.device_fabric
                    .request_dpc_submission(source, start, end)
            })
            .unwrap_or_else(|error| panic!("RSP DPC submission rejected: {error}"));
            let Some(submission) = submission else {
                continue;
            };
            let rdp_started = gfx_started.map(|_| std::time::Instant::now());

            // T4: the real RSP-driven producer (this loop, not a Dmem-arm
            // surrogate). Same routing decision as `dispatch_dpc_submission`'s
            // top-level guard, made BEFORE `LiveDpcTransaction::new` -- see
            // that function's own comment for why (constructing then dropping
            // a `LiveDpcTransaction` cancels a still-wanted fabric submission).
            // `WgpuBackend`'s raw-DPC seam is a synchronous, non-GPU CPU-side
            // coordinator with no async completion concept, so the "always
            // defer the GPU-completion wait" rationale below (specific to
            // RT64's real async GPU queue) does not apply here: routing
            // through `with_ready_commit` (an immediate, synchronous publish)
            // costs nothing extra this loop did not already pay by calling
            // `dispatch_captured_raw_rdp` with `wait_for_completion: false`
            // against a backend that has no deferred completion to defer.
            let session_registered = RAW_DPC_SESSION.with(|cell| cell.borrow().is_some());
            if session_registered {
                let owned_submission = if xbus {
                    fn64_render::OwnedRawDpcSubmission::from_xbus_payload(
                        start,
                        end,
                        words.iter().flat_map(|word| word.to_be_bytes()).collect(),
                    )
                } else {
                    fn64_render::OwnedRawDpcSubmission::from_rdram_words(start, end, words.clone())
                }
                .unwrap_or_else(|error| {
                    panic!("RSP DPC submission does not admit a T4 capture: {error:?}")
                });
                let (transaction, ack) = LiveDpcTransaction::new(submission);
                let (full_sync, observation) = try_dispatch_raw_dpc_via_session(
                    rdram,
                    SessionRawDpcSource {
                        submission: owned_submission,
                    },
                    transaction,
                    ack,
                    Some((&deferred_dpc_history, &read_epoch_boundaries)),
                )
                .expect("session_registered was already checked true under the same borrow");
                if let Some(started) = rdp_started {
                    raw_rdp_ns = raw_rdp_ns.saturating_add(started.elapsed().as_nanos() as u64);
                }
                dpc_observations.push(observation);
                if full_sync == fn64_render::DpFullSyncStatus::Reached {
                    dp_full_sync = fn64_render::DpFullSyncStatus::Reached;
                }
                continue;
            }

            let (mut transaction, ack) = LiveDpcTransaction::new(submission);
            // Always defer the GPU-completion wait here. RT64's queue is a
            // monotonic counter (`waitId <= workloadId`, rt64_workload_queue.cpp
            // :93), so waiting for a later submission's id also waits for every
            // earlier one -- there is no reordering risk from deferring past
            // this call. Nothing between here and this task's return reads
            // GPU-completed state: `full_sync`/`observation` are decided from
            // the submitted command bytes and the synchronous submit-time
            // status (`FrameStatus`/`DpFullSyncStatus`), not from waiting.
            // `Rt64Backend::present` flushes any outstanding workload before it
            // reads anything, which is the one place downstream that genuinely
            // needs completed state. Measured 2026-08-10 (render-benchmark
            // route, rt64 lane): waiting after every submission, when a task's
            // submissions were already fully merged by the loop above, was
            // costing ~11 ms/field with ZERO submissions actually deferred by
            // an earlier same-task-only version of this change -- the repeated
            // wait cost is paid ACROSS separate RSP tasks in one field
            // (sp_tasks ~2.9/field), which this field-wide (present-flushed)
            // version reaches and the earlier per-task version could not.
            let (full_sync, observation) = unsafe {
                dispatch_captured_raw_rdp(
                    rdram,
                    &words,
                    start,
                    end,
                    xbus,
                    false,
                    &mut transaction,
                    ack,
                )
            };
            if let Some(started) = rdp_started {
                raw_rdp_ns = raw_rdp_ns.saturating_add(started.elapsed().as_nanos() as u64);
            }
            transaction.commit();
            dpc_observations.push(observation);
            if full_sync == fn64_render::DpFullSyncStatus::Reached {
                dp_full_sync = fn64_render::DpFullSyncStatus::Reached;
            }
        }
    }

    observations.extend(dpc_observations);
    if let Some(pending) = pending_raw_dpc_task_batch.as_mut() {
        let mut dpc = core::mem::take(&mut pending.observations);
        observations.append(&mut dpc);
        pending.observations = observations;
    } else {
        record_rsp_rdp_observations(observations);
    }
    commit_rsp_interpreter_phase(owner, final_architectural_state);

    if let Some(observation) = completed_guest_task_observation {
        crate::render_observation::record_completed_guest_task(observation);
    }

    if let Some(observation) = guest_task_observation.take() {
        let rdp_execution = if observation.kind() == crate::GuestTaskKind::Audio {
            crate::GuestTaskRdpExecution::NotApplicable
        } else {
            crate::GuestTaskRdpExecution::Unavailable
        };
        crate::render_observation::record_completed_guest_task(observation.complete(
            guest_task_outcome,
            crate::emulated_now(),
            crate::GuestRspDispatchLane::Interpreted,
            rdp_execution,
            crate::GuestTaskQueueIdentity::NotApplicable,
            crate::RenderBatchHostThread::Emulation,
            None,
        ));
    }

    if let Some(started) = gfx_started {
        let elapsed_ns = started.elapsed().as_nanos() as u64;
        GFX_NS.with(|total| {
            total.set(total.get().saturating_add(elapsed_ns));
        });
        GFX_CALLS.with(|calls| calls.set(calls.get() + 1));
        GFX_LLE_NS.with(|total| total.set(total.get().saturating_add(elapsed_ns)));
        GFX_LLE_CALLS.with(|calls| calls.set(calls.get() + 1));
        GFX_LLE_RSP_NS.with(|total| total.set(total.get().saturating_add(rsp_execution_ns)));
        GFX_LLE_RDP_NS.with(|total| total.set(total.get().saturating_add(raw_rdp_ns)));
    }
    if let Some(started) = non_gfx_started {
        let elapsed_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        AUDIO_LLE_NS.with(|total| total.set(total.get().saturating_add(elapsed_ns)));
        AUDIO_LLE_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
        AUDIO_LLE_RSP_NS.with(|total| total.set(total.get().saturating_add(rsp_execution_ns)));
    }

    LleTaskResult {
        steps: total_steps.max(1),
        dp_full_sync,
        pending_raw_dpc_task_batch,
    }
}

/// Execute the admitted rspboot until control first reaches bytes loaded by
/// an IMEM DMA, then commit its memory, PC, and SP-status effects before an
/// optimized graphics/audio backend represents the ucode phase.
///
/// The phase boundary comes from the public SGI RSP guide's task protocol:
/// rspboot consumes the task header, loads the selected ucode into IMEM, and
/// starts that ucode. HLE backends consume the public `OSTask` contract, while
/// a transactional LLE fallback receives the complete non-memory machine
/// snapshot alongside the committed memory image.
pub(crate) unsafe fn dispatch_hle_rspboot(rdram: *mut u8, task_addr: RdramAddr) -> HleBootResult {
    const BOOT_CHUNK_STEPS: u64 = 1 << 12;
    const MAX_BOOT_STEPS: u64 = 1 << 20;

    let (mut dmem, mut imem, status, mut pc, imem_generation, rdram_len, static_aliases) =
        with_host(|host| {
            let fabric = &host.device_fabric;
            (
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Dmem),
                *fabric.rsp_memory().bank(fn64_runtime::RspMemoryBank::Imem),
                fabric.sp_status(),
                fabric.sp_pc(),
                fabric.rsp_memory().imem_generation(),
                host.runtime_rdram_len,
                host.sections.loaded_static_storage_ranges(),
            )
        });
    assert!(
        !rdram.is_null() && rdram_len != 0,
        "RSP HLE rspboot has no registered process RDRAM allocation"
    );
    let (dma_ranges, _) = rsp_dma_storage_layout(rdram_len, static_aliases);
    #[cfg(feature = "recomp-rs")]
    let catalog_writer = crate::recompiled::begin_catalog_nested_writer(
        unsafe { std::slice::from_raw_parts(rdram, rdram_len) },
        "RSP HLE rspboot execution",
    );
    let rdram_slice = unsafe { std::slice::from_raw_parts_mut(rdram, rdram_len) };
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(rdram_slice);
    machine.set_dma_rdram_ranges(dma_ranges);
    machine.load_dmem_logical(&dmem);
    begin_rsp_interpreter_phase(task_interpreter_owner(task_addr), &mut machine);
    let mut total_steps = 0u64;
    let mut overlays = 0u64;
    let mut loaded_spans = Vec::new();
    let mut replacements = Vec::new();

    loop {
        let execution_pc = if machine.ctx.resume_address != 0 {
            0x1000 | (machine.ctx.resume_address & 0x0fff)
        } else {
            0x1000 | (pc & 0x0fff)
        };
        if loaded_spans
            .iter()
            .copied()
            .any(|span: fn64_audio::rsp::runtime::ImemDmaSpan| span.contains_pc(execution_pc))
        {
            pc = execution_pc;
            break;
        }

        let words: Vec<u32> = imem
            .chunks_exact(4)
            .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four IMEM bytes")))
            .collect();
        let budget = if loaded_spans.is_empty() {
            BOOT_CHUNK_STEPS
        } else {
            1
        };
        let result = fn64_audio::rsp::run_imem(&words, pc, &mut machine, budget);
        total_steps = total_steps
            .checked_add(result.steps)
            .expect("RSP rspboot step counter overflow");
        let imem_word_0 = u32::from_be_bytes(imem[0..4].try_into().expect("one IMEM word"));
        let task_boot = u32::from_be_bytes(dmem[0x0fc8..0x0fcc].try_into().expect("OSTask boot"));
        let task_boot_size =
            u32::from_be_bytes(dmem[0x0fcc..0x0fd0].try_into().expect("OSTask boot size"));
        let task_ucode = u32::from_be_bytes(dmem[0x0fd0..0x0fd4].try_into().expect("OSTask ucode"));
        assert!(
            total_steps <= MAX_BOOT_STEPS,
            "RSP HLE rspboot exceeded deterministic {MAX_BOOT_STEPS}-instruction bound at PC \
             {:#06x}; IMEM[0]={imem_word_0:#010x}, SP status={status:#010x}, admitted OSTask \
             boot={task_boot:#010x}/size={task_boot_size:#x}, ucode={task_ucode:#010x}",
            result.pc,
        );
        pc = result.pc;
        match result.reason {
            fn64_audio::rsp::RspExitReason::SwapOverlay => {
                loaded_spans.push(machine.pending_imem_dma_span());
                machine.complete_imem_dma(&mut imem);
                overlays = overlays
                    .checked_add(1)
                    .expect("RSP rspboot IMEM generation counter overflow");
                replacements.push(PendingImemReplacement {
                    generation: imem_generation
                        .checked_add(overlays)
                        .expect("RSP rspboot IMEM generation evidence overflow"),
                    image: imem,
                });
            }
            fn64_audio::rsp::RspExitReason::StepLimit => {}
            fn64_audio::rsp::RspExitReason::Broke => panic!(
                "RSP HLE rspboot reached BREAK before entering DMA-loaded ucode at PC {:#06x}",
                result.pc
            ),
            reason => panic!(
                "RSP HLE rspboot stopped at PC {:#06x} after {total_steps} instructions: {reason:?}",
                result.pc
            ),
        }
    }

    dmem = machine.dmem_logical();
    let task = os_task_header_from_words(|field| {
        let start = 0x0fc0 + field;
        u32::from_be_bytes(
            dmem[start..start + 4]
                .try_into()
                .expect("four OSTask DMEM bytes"),
        )
    });
    let dp_submissions = machine.take_dp_submissions();
    let machine_state = machine.snapshot_state();
    let final_architectural_state = machine_state.architectural_state().clone();
    assert!(
        dp_submissions.is_empty(),
        "RSP HLE rspboot submitted {} DPC range(s) before entering ucode",
        dp_submissions.len()
    );
    let rdram_writes = machine.take_rdram_writes();
    drop(machine);

    commit_rsp_rdram_writes(
        RspWriterCommitSourceV1::Interpreter {
            owner: task_interpreter_owner(task_addr),
        },
        &rdram_writes,
    );
    #[cfg(feature = "recomp-rs")]
    catalog_writer.commit(unsafe { std::slice::from_raw_parts(rdram, rdram_len) });
    commit_rsp_memory_state(
        &dmem,
        &imem,
        overlays,
        rsp_execution_state_from_architectural(&final_architectural_state, pc & 0x0fff),
    );
    let committed_generation = with_host(|host| host.device_fabric.rsp_memory().imem_generation());
    assert_eq!(
        committed_generation,
        imem_generation
            .checked_add(overlays)
            .expect("RSP rspboot IMEM generation evidence overflow"),
        "RSP rspboot IMEM replacement count diverged from the committed fabric generation"
    );
    record_rsp_rdp_observations(
        replacements
            .into_iter()
            .map(
                |replacement| RspRdpObservationKind::ImemReplacementCommitted {
                    task_addr,
                    imem_generation: replacement.generation,
                    text_sha256: imem_sha256(&replacement.image),
                },
            )
            .collect(),
    );
    HleBootResult {
        steps: total_steps.max(1),
        task,
        machine_state,
    }
}
