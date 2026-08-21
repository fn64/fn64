use super::*;
use fn64_audio::rsp::runtime::RspDpCommandSource;

struct XbusStreamDump {
    directory: std::path::PathBuf,
    skip: u64,
    rdram_index: Option<u64>,
}

struct XbusDiagnostics {
    stream_dump: Option<XbusStreamDump>,
    diff_trace: bool,
}

fn xbus_diagnostics() -> &'static XbusDiagnostics {
    static CONFIG: std::sync::OnceLock<XbusDiagnostics> = std::sync::OnceLock::new();
    CONFIG.get_or_init(|| {
        let stream_dump = std::env::var_os("FN64_XBUS_STREAM_DUMP_DIR").map(|directory| {
            let parse_index = |name: &str, default: Option<u64>| {
                std::env::var(name).ok().map_or(default, |raw| {
                    Some(
                        raw.parse::<u64>()
                            .unwrap_or_else(|_| panic!("{name} must be a u64, got {raw:?}")),
                    )
                })
            };
            XbusStreamDump {
                directory: directory.into(),
                skip: parse_index("FN64_XBUS_STREAM_DUMP_SKIP", Some(0))
                    .expect("XBUS dump skip has a default"),
                rdram_index: parse_index("FN64_XBUS_STREAM_DUMP_RDRAM", None),
            }
        });
        XbusDiagnostics {
            stream_dump,
            diff_trace: std::env::var_os("FN64_XBUS_DIFF_TRACE").is_some(),
        }
    })
}

fn rsp_trace_dpc_words_limit() -> Option<usize> {
    static LIMIT: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("RSP_TRACE_DPC_WORDS").ok().map(|raw| {
            raw.parse::<usize>()
                .unwrap_or_else(|_| panic!("RSP_TRACE_DPC_WORDS must be decimal, got {raw:?}"))
        })
    })
}

pub(crate) fn commit_rsp_memory_state(
    dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    overlays: u64,
    execution_state: fn64_runtime::RspExecutionState,
) {
    with_host(|host| {
        let memory = host.device_fabric.rsp_memory_mut();
        memory
            .write_bytes(
                fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0),
                dmem,
            )
            .expect("RSP DMEM commit failed");
        for _ in 0..overlays {
            memory
                .write_bytes(
                    fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
                    imem,
                )
                .expect("RSP IMEM generation commit failed");
        }
        host.device_fabric
            .commit_complete_rsp_execution_state(execution_state)
            .unwrap_or_else(|error| panic!("RSP interpreter-state commit rejected: {error}"));
    });
}

/// One hardware command stream assembled from consecutive DPC submissions.
///
/// `start..end` is the source range the stream was fetched from, and its
/// length always equals the payload's, for both sources. Downstream capture
/// (`OwnedRawDpcSubmission::from_xbus_payload`,
/// `OwnedRawDpcSubmission::from_rdram_words`) rejects any run where it does
/// not.
pub(crate) struct CoalescedDpRun {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) xbus: bool,
    pub(crate) words: Vec<u32>,
}

/// Group consecutive DPC submissions into hardware command streams.
///
/// Consecutive END extensions against a single unmoved START are one stream:
/// F3DEX xbus 2.08 extends a run 8 bytes per END write, so a 16-byte command
/// straddles two submissions and per-submission decode would trap on a
/// truncation hardware simply stalls through (`7ef65d54`).
///
/// **Adjacency governs both sources, and for the same reason.** A submission
/// continues the open run only when its `start` equals the run's current
/// `end`; anything else is a new START and therefore a new stream. XBUS was
/// briefly exempted from that test on the theory that a DMEM-sourced range
/// means something weaker than a physical one. It does not: the producer
/// (`fn64_audio::rsp::runtime`'s CP0 `DP_END` handler) derives XBUS bytes from
/// exactly `start & 0x0fff .. end & 0x0fff`, so an XBUS range names its bytes
/// as precisely as an RDRAM range names its words. Measured on WM2000's first
/// graphics task: 365 XBUS submissions form four address-contiguous runs over
/// a DMEM ring at `[0x0ba8, 0x0f20)`, wrapping to the ring base three times.
/// Coalescing across a wrap concatenated all 3400 bytes while `start..end`
/// still described only 752 of them, and the capture correctly refused it
/// (`XbusPayloadLength { expected: 752, actual: 3400 }`). Each of the four runs
/// decodes cleanly on its own against the RDP width table
/// (`fn64_render_ir`'s `raw_rdp_command_width`), so the ring wrap is a real
/// stream boundary and not a straddle the coalescer must bridge.
pub(crate) fn coalesce_dp_submissions(
    submissions: Vec<fn64_audio::rsp::runtime::RspDpSubmission>,
) -> Vec<CoalescedDpRun> {
    let mut runs = Vec::new();
    let mut pending = submissions.into_iter().peekable();
    while let Some(first) = pending.next() {
        let (start, mut end, source) = first.into_parts();
        let (xbus, words) = match source {
            RspDpCommandSource::XbusBytes(mut stream) => {
                while pending
                    .peek()
                    .is_some_and(|submission| submission.is_xbus() && submission.start == end)
                {
                    let next = pending.next().expect("peeked XBUS submission disappeared");
                    let (_, next_end, next_source) = next.into_parts();
                    let RspDpCommandSource::XbusBytes(bytes) = next_source else {
                        unreachable!("XBUS predicate and owned command source diverged")
                    };
                    stream.extend_from_slice(&bytes);
                    end = next_end;
                }
                let words = stream
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().expect("four XBUS bytes")))
                    .collect::<Vec<_>>();
                (true, words)
            }
            RspDpCommandSource::RdramWords(mut words) => {
                while pending
                    .peek()
                    .is_some_and(|submission| !submission.is_xbus() && submission.start == end)
                {
                    let next = pending.next().expect("peeked RDRAM submission disappeared");
                    let (_, next_end, next_source) = next.into_parts();
                    let RspDpCommandSource::RdramWords(next_words) = next_source else {
                        unreachable!("RDRAM predicate and owned command source diverged")
                    };
                    words.extend(next_words);
                    end = next_end;
                }
                (false, words)
            }
        };
        // The invariant every downstream capture depends on, checked at the
        // one site that can violate it rather than at the four that observe
        // it. A coalescing bug shows up here, naming the run, instead of as
        // a length mismatch several layers away.
        assert_eq!(
            u64::from(end).checked_sub(u64::from(start)),
            Some(words.len() as u64 * 4),
            "coalesced DPC run [{start:#010x}, {end:#010x}) (xbus={xbus}) does not describe its \
             own {} command bytes",
            words.len() * 4
        );
        runs.push(CoalescedDpRun {
            start,
            end,
            xbus,
            words,
        });
    }
    runs
}

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
    let debug_dir = std::env::var_os("FN64_RSP_LLE_DEBUG_DIR").map(std::path::PathBuf::from);
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
    let dp_submissions = machine.take_dp_submissions();
    let final_architectural_state = machine.snapshot_architectural_state();
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

    let mut dp_full_sync = fn64_render::DpFullSyncStatus::NotReached;
    let mut dpc_observations = Vec::with_capacity(dp_submissions.len());
    let trace_limit = rsp_trace_dpc_words_limit();
    for CoalescedDpRun {
        start,
        end,
        xbus,
        words,
    } in coalesce_dp_submissions(dp_submissions)
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
            let transaction = LiveDpcTransaction::new(submission);
            let (full_sync, observation) = try_dispatch_raw_dpc_via_session(
                rdram,
                SessionRawDpcSource {
                    submission: owned_submission,
                },
                transaction,
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

        let mut transaction = LiveDpcTransaction::new(submission);
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
            dispatch_captured_raw_rdp(rdram, &words, start, end, xbus, false, &mut transaction)
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

    let mut observations = Vec::new();
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
    observations.extend(dpc_observations);
    record_rsp_rdp_observations(observations);
    commit_rsp_interpreter_phase(owner, final_architectural_state);

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

/// Own one fabric-issued DPC transaction across renderer execution. A backend
/// panic unwinds through this guard and cancels the exact token, so a rejected
/// range cannot remain busy or later advance CURRENT as if it had rendered.
pub(crate) struct LiveDpcTransaction {
    pub(crate) token: Option<u64>,
    pub(crate) acknowledgment: Option<fn64_runtime::DpcScheduledExecution>,
}

impl LiveDpcTransaction {
    pub(crate) fn new(submission: fn64_runtime::DpcSubmission) -> Self {
        with_host(|host| {
            assert_eq!(
                host.device_fabric.pending_dpc_submission(),
                Some(submission),
                "renderer received DPC transaction which the device fabric does not own"
            );
        });
        // Install cancellation ownership before any shared-ack construction.
        // If an admitted fabric range cannot form the compatibility quantum,
        // unwinding this guard restores the exact pre-admission DPC state.
        // The exact-token assertion stays first so a bad caller cannot make
        // this guard cancel some other transaction while unwinding.
        let mut transaction = Self {
            token: Some(submission.token),
            acknowledgment: None,
        };
        let source = submission.source;
        let start = fn64_runtime::DpcCursor::new(source, submission.start)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC start cursor: {error:?}"));
        let end = fn64_runtime::DpcCursor::new(source, submission.end)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC end cursor: {error:?}"));
        // Phase B deliberately assigns no device-time meaning to this one
        // compatibility quantum. Zero is an internal acknowledgment sentinel;
        // production still performs one synchronous atomic backend call.
        let sentinel = fn64_runtime::Cycles::new(0);
        let mut acknowledgment = fn64_runtime::DpcScheduledExecution::new(
            submission,
            sentinel,
            vec![fn64_runtime::DpcQuantumPlan {
                at: sentinel,
                id: fn64_runtime::DpcQuantumId::new(1),
                start,
                end,
            }],
        )
        .unwrap_or_else(|error| {
            panic!("fabric DPC transaction cannot form an atomic ack: {error:?}")
        });
        let fn64_runtime::DpcAdvance::Blocked { at, action } = acknowledgment
            .advance_to(sentinel)
            .unwrap_or_else(|error| panic!("arming atomic DPC acknowledgment: {error:?}"))
        else {
            panic!("atomic DPC acknowledgment passed its sole external-work barrier")
        };
        assert_eq!(at, sentinel);
        assert_eq!(action.transaction, acknowledgment.transaction());
        assert_eq!(action.start, start);
        assert_eq!(action.end, end);
        transaction.acknowledgment = Some(acknowledgment);
        transaction
    }

    /// Validate the compatibility backend's sole atomic completion before
    /// publishing its shadow memory. This carries no timing authority.
    pub(crate) fn validate_atomic_completion(&mut self) {
        let acknowledgment = self
            .acknowledgment
            .as_mut()
            .expect("atomic DPC transaction has no acknowledgment owner");
        let fn64_runtime::DpcScheduledPhase::AwaitingAck(request) = acknowledgment.phase() else {
            panic!("atomic DPC transaction lost its acknowledgment owner before validation")
        };
        acknowledgment
            .acknowledge(fn64_runtime::DpcBackendQuantumAck {
                transaction: request.transaction,
                quantum: request.quantum,
                committed_through: request.end,
                status: fn64_runtime::DpcBackendQuantumStatus::Complete,
            })
            .unwrap_or_else(|error| panic!("validating atomic DPC acknowledgment: {error:?}"));
        assert_eq!(
            acknowledgment.phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC acknowledgment did not consume its sole quantum"
        );
    }

    pub(crate) fn commit(mut self) {
        let token = *self
            .token
            .as_ref()
            .expect("DPC transaction committed twice");
        assert_eq!(
            self.acknowledgment
                .as_ref()
                .expect("atomic DPC transaction has no acknowledgment owner")
                .phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC transaction committed before acknowledgment validation"
        );
        with_host(|host| host.device_fabric.commit_dpc_submission(token))
            .unwrap_or_else(|error| panic!("committing rendered DPC transaction: {error}"));
        self.token.take();
    }

    /// Route this transaction's terminal fabric commit through the
    /// nonmutating-prepare / infallible-consume `ReadyDpcFabricCommit`
    /// typestate (`fn64_runtime::device::fabric_ops`), handing the live
    /// `ReadyDpcFabricCommit` to a caller-supplied closure INSIDE the
    /// `with_host` borrow rather than committing it immediately.
    ///
    /// This is the seam a future T0 capsule-assembly call needs: the v11
    /// migration card's `ReadyRawDpcCommitCapsule` must own the ready fabric
    /// state across its OWN joint physical/fabric publication (device fabric
    /// prepares, wgpu does its fallible physical-readiness work, THEN one
    /// atomic body commits both). A method that prepares and immediately
    /// calls `.commit()` before any capsule exists cannot serve that: there
    /// is nothing left for a capsule to receive. `with_ready_commit` instead
    /// hands the ready value, live, to `f` -- which is where a future caller
    /// builds `ReadyRawDpcCommitCapsule` from it (combined with the
    /// guest-committed wrapper) and either commits or lets it drop-cancel,
    /// all still inside the one `with_host` borrow this fabric requires.
    ///
    /// `f`'s return value `R` is NOT permitted to retain the
    /// `ReadyDpcFabricCommit<'_>` borrow -- `with_host`'s own signature
    /// (`impl FnOnce(&mut HostState) -> R`) already forbids that, so the
    /// compiler enforces it structurally, not by convention.
    ///
    /// **Disarms this transaction's own cancel guard (`self.token`) only
    /// AFTER a `ReadyDpcFabricCommit` has been successfully constructed, not
    /// before.** `LiveDpcTransaction::drop` and `ReadyDpcFabricCommit::drop`
    /// are two independent cancellation paths over the same underlying
    /// fabric state (`LiveDpcTransaction` via `cancel_dpc_submission(token)`
    /// on the fabric as a whole; `ReadyDpcFabricCommit` via direct field
    /// writes to the same `dpc`/`pending_dpc` fields `prepare_dpc_commit`
    /// borrowed). Exactly one of the two must be the live cancellation owner
    /// at every point in this method's body -- disarming too early leaks the
    /// pending fabric transaction; disarming too late double-cancels it:
    ///
    /// - While the acknowledgment-phase check and `prepare_dpc_commit`'s own
    ///   fallible validation are still running, NO `ReadyDpcFabricCommit`
    ///   exists yet, so `LiveDpcTransaction` must remain the armed owner: if
    ///   either fails (an assertion panic, or `prepare_dpc_commit` returning
    ///   `Err`), `LiveDpcTransaction::drop` is what cancels the still-pending
    ///   fabric transaction. `prepare_dpc_commit` itself restores the owned
    ///   `PendingDpc` on any rejection (see its own doc comment), so the
    ///   fabric's `pending_dpc` is exactly as it was when
    ///   `LiveDpcTransaction::drop` runs `cancel_dpc_submission`.
    /// - The token is read (`self.token`), not taken, for this whole
    ///   validate-then-prepare span, so `self` stays armed throughout it.
    /// - Only once `prepare_dpc_commit` has returned `Ok` -- meaning a
    ///   `ReadyDpcFabricCommit` now exists and holds its own independent
    ///   cancellation path -- does this method assign `self.token = None`,
    ///   disarming `LiveDpcTransaction::drop`, immediately before calling
    ///   `f(ready)`. From that point on, `ReadyDpcFabricCommit` is the sole
    ///   cancellation owner: if `f` panics, `ReadyDpcFabricCommit::drop`
    ///   cancels (it unwinds before `LiveDpcTransaction::drop`, which is
    ///   already a no-op by then); `LiveDpcTransaction::drop` cannot also
    ///   fire a second `cancel_dpc_submission` against fabric state
    ///   `ReadyDpcFabricCommit::drop` may have already rolled back or
    ///   cleared, which is what would otherwise panic from inside an unwind
    ///   already in progress and abort the process.
    pub(crate) fn with_ready_commit<R>(
        mut self,
        f: impl FnOnce(fn64_runtime::device::ReadyDpcFabricCommit<'_>) -> R,
    ) -> R {
        let token = self.token.expect("DPC transaction committed twice");
        assert_eq!(
            self.acknowledgment
                .as_ref()
                .expect("atomic DPC transaction has no acknowledgment owner")
                .phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC transaction committed before acknowledgment validation"
        );
        // `self.token` is still `Some` here: `LiveDpcTransaction::drop`
        // remains the armed cancellation owner through every fallible step
        // below, up to and including `prepare_dpc_commit` itself.
        with_host(|host| {
            let ready = host
                .device_fabric
                .prepare_dpc_commit(token)
                .unwrap_or_else(|error| panic!("preparing ready DPC fabric commit: {error}"));
            // A `ReadyDpcFabricCommit` now exists and owns its own
            // cancellation path. Disarm `LiveDpcTransaction` here, and not
            // one line earlier, so there is no window where neither guard is
            // armed, and no OBSERVABLE OR FALLIBLE window where both can
            // act: both guards are briefly simultaneously armed across this
            // one nonpanicking assignment (`ready` already exists the moment
            // this comment is reached), but nothing between `ready`'s
            // construction and this line can panic, return `Result`, invoke
            // a callback, or otherwise give either guard's `Drop` a chance to
            // run -- so the two-armed span has no reachable exit that could
            // let both actually cancel.
            self.token = None;
            f(ready)
        })
    }
}

impl Drop for LiveDpcTransaction {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        with_host(|host| host.device_fabric.cancel_dpc_submission(token))
            .unwrap_or_else(|error| panic!("cancelling rejected DPC transaction: {error}"));
    }
}

/// One captured raw-DPC source ready for the T4 plan/execute/publish
/// production seam: the exact original command words and their exact
/// source range/kind. Preserves the real capture source and ranges --
/// unlike the legacy `dispatch_captured_raw_rdp` staging path, this never
/// manufactures a synthetic RDRAM suffix.
///
/// Every admitted TMEM load's *source* bytes are declared as
/// `RdramResource::Buffer` regardless of `submission.source()`
/// (`crate::raw_dpc::production_adapter`'s push loop, mirroring real RDP
/// hardware: XBUS changes only where the RDP *command words* come from --
/// DMEM vs. DRAM -- never where `LoadBlock`/`LoadTile`/`LoadTLUT` read their
/// texel data, which is always the RDP's 24-bit physical RDRAM address
/// space). So there is exactly one guest-read byte source, live RDRAM, for
/// both producers; no separate DMEM byte source is needed or correct here.
struct SessionRawDpcSource {
    submission: fn64_render::OwnedRawDpcSubmission,
}

/// Attempt the T4 production plan/execute/publish routing for one raw-DPC
/// submission. Returns `None` (never partially attempted) when no
/// `RawDpcAbiSession` is registered, so callers fall back to the legacy
/// atomic `process_rdp_commands` path unconditionally -- required for
/// `Rt64Backend` and any other backend that never implements
/// `plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc`.
///
/// `plan_raw_dpc` (`fn64-render-wgpu`'s `WgpuBackend`) already rejects a
/// `FullSync` command or any command outside the admitted TMEM/state/fill
/// subset as a loud `RenderError`.
///
/// Which guest-commit method runs is decided by what the backend itself
/// says it staged, read back through `staged_guest_render_target_writes`:
///
/// - An empty list takes `commit_zero_guest_writes`, which independently
///   re-rejects any guest-visible write with `EffectCountMismatch`. This is
///   every TMEM-only and triangle-only submission.
/// - A nonempty list takes `commit_guest_render_target_writes`, which
///   re-validates every element's access mode/purpose and then, through
///   `GuestCommitEffectReport::try_new`, its count, order, identity, and
///   content digest against the packet's own guest-write journal. A backend
///   that reported a fabricated list is caught there, not trusted here.
///
/// Neither rejection is caught: both `.unwrap_or_else(|error| panic!(...))`
/// through, matching AGENTS.md's loud-trap rule.
///
/// Taking the nonempty branch DOES modify guest RDRAM, through
/// `copy_committed_guest_writes` and only after the commit above returned
/// `Ok`. This supersedes the earlier nonclaim on this function ("taking the
/// nonempty branch modifies no guest RDRAM byte"), which was true until the
/// copyback landed.
///
/// Nonclaim, unchanged: the zero-write branch modifies nothing, and
/// `CompletedWrite` still carries no bytes -- the payload travels through
/// `RenderBackend::committed_guest_render_target_bytes`, a separate method,
/// and is checked against the committed digest before it is written.
/// A submission this backend cannot admit is a hard stop, not a silent
/// fallback to the legacy path: falling back would let a T4-registered
/// session quietly downgrade capture fidelity for exactly the submissions
/// its own admission rules were built to catch.
fn try_dispatch_raw_dpc_via_session(
    rdram: *mut u8,
    source: SessionRawDpcSource,
    mut transaction: LiveDpcTransaction,
) -> Option<(fn64_render::DpFullSyncStatus, RspRdpObservationKind)> {
    let registered = RAW_DPC_SESSION.with(|cell| cell.borrow().is_some());
    if !registered {
        return None;
    }

    // The live RDRAM allocation is the sole guest-read byte source for both
    // producers -- see `SessionRawDpcSource`'s doc comment -- and also the
    // sole memory-layout proof: XBUS command words are bounded separately
    // (`DmemRange`, the 4 KiB DMEM bank) inside `preflight_raw_dpc_capture`,
    // never through this `memory_layout`.
    let real = unsafe { renderer_rdram_slice(rdram) };
    let memory_layout = fn64_render::ir::PhysicalMemoryLayout::try_new(
        u32::try_from(real.len()).expect("registered RDRAM allocation fits a u32 byte length"),
    )
    .unwrap_or_else(|error| panic!("try_dispatch_raw_dpc_via_session: {error}"));
    // The `cmd_end` interrupt snapshot is a fixed `Clear`. That is exact, not
    // an assumption: the DP interrupt for a raw FullSync is raised inside
    // `DeviceFabric::advance_to`'s `DeviceEvent::Dp` arm, and device
    // advancement cannot run during renderer dispatch, so the line cannot
    // have been raised by this submission at the moment this boundary is
    // built. `transaction_sequence` reuses this exact transaction's own
    // fabric-issued token: real per-submission fabric identity, not a
    // fabricated counter, matching the requirement to preserve the existing
    // fabric token lifecycle through this new path.
    let token = transaction
        .token
        .expect("try_dispatch_raw_dpc_via_session: transaction committed twice");
    let xbus = source.submission.source() == fn64_render::RawDpcSource::XbusDmem;
    let observation_start = source.submission.start();
    let observation_end = source.submission.end();
    let observation_words = source.submission.command_words();
    let cmd_end =
        fn64_render::ir::TemporalBoundary::new(token, fn64_render::ir::DpInterruptState::Clear);

    // Reserve half of the FullSync two-phase contract.
    //
    // `fn64-render-ir` requires exactly one `FullSyncBoundary` per decoded
    // `SYNC_FULL` opcode, so a submission carrying one cannot be planned at
    // all unless this producer supplies it. Count the sites structurally
    // (same stride walk, same six-bit masking as the RDRAM inspector) and,
    // when there are any, prove the sole DP completion slot is free through
    // the nonmutating `preflight_dp_full_sync` BEFORE the backend is entered
    // or any guest byte is read -- which is precisely what that function's
    // own doc says it exists for.
    // **This path receives CLOSED streams only.** A completed RSP task's
    // submissions are coalesced by `coalesce_dp_submissions` before they get
    // here, and a CPU raw-MMIO stream reaches this function only once the
    // fabric has assembled a whole command run -- an incomplete tail is
    // parked in the fabric and never dispatched. So `Incomplete` here means
    // an assembler upstream broke its contract, and it stays a loud panic
    // rather than silently stranding a completed transaction.
    let full_sync_sites = match fn64_render::count_raw_rdp_full_sync_sites(&observation_words)
        .unwrap_or_else(|error| panic!("try_dispatch_raw_dpc_via_session: {error}"))
    {
        fn64_render::RawRdpScan::Complete(sites) => sites,
        fn64_render::RawRdpScan::Incomplete {
            command_start,
            bytes_required,
            bytes_available,
            ..
        } => panic!(
            "try_dispatch_raw_dpc_via_session: a dispatched stream ends inside the command at \
             byte {command_start:#x} ({bytes_available} of {bytes_required} bytes present); \
             incomplete tails must be parked by the fabric, never dispatched"
        ),
    };
    let capture = if full_sync_sites == 0 {
        fn64_render::OwnedRawDpcCapture::new(source.submission, memory_layout, token, cmd_end)
    } else {
        // Interleaving closed exactly as `preflight_raw_dpc_completion`
        // closes it on the legacy path: a prior FullSync may still be
        // pending, and observing an occupied slot here rejects before the
        // backend or RDRAM is touched.
        with_host(|host| {
            host.device_fabric
                .preflight_dp_full_sync(fn64_runtime::Cycles::new(1))
        })
        .unwrap_or_else(|error| {
            panic!("try_dispatch_raw_dpc_via_session: DP FullSync completion: {error}")
        });

        // HONESTY BOUNDARY -- read this before changing either state below.
        //
        // `interrupt_before` is `Clear` because it is genuinely observed:
        // device advancement cannot run during dispatch, so nothing this
        // submission did could have raised the line yet.
        //
        // `interrupt_after` is ALSO `Clear`, and that is the honest value,
        // not a placeholder to be "fixed" later by writing `Asserted` here.
        // A successful `preflight_dp_full_sync` is a RESERVATION: it is
        // nonmutating, it schedules no `DeviceEvent::Dp`, and it raises no
        // interrupt. The interrupt for this submission is raised only when
        // `complete_committed_dpc` calls `start_live_dp_full_sync` and the
        // guest later advances devices past the deadline -- strictly after
        // this capture, this plan, this execution, and this publication have
        // all already happened. There is therefore no point in this flow at
        // which an `Asserted` value could be READ, and writing one would
        // fabricate a guest-visible interrupt edge that never occurred.
        //
        // Delivering a truthful `Asserted` needs the post-commit read-
        // observation and coherence work `docs/RENDER-WGPU-PORT-PLAN.md`'s
        // D7 defers to M9. Until then the decoded site is recorded and the
        // observation is not claimed.
        //
        // Sequences: `cmd_end` owns `token`, so each site's pair must be
        // strictly increasing after it and its own interrupt sequence must
        // exceed its site sequence -- `derive_stream`'s
        // `NonMonotonicFullSyncSequence` check.
        let boundaries = (0..full_sync_sites)
            .map(|ordinal| {
                let ordinal = ordinal as u64;
                fn64_render::ir::FullSyncBoundary::new(
                    token + 1 + ordinal * 2,
                    token + 2 + ordinal * 2,
                    fn64_render::ir::DpInterruptState::Clear,
                    fn64_render::ir::DpInterruptState::Clear,
                )
            })
            .collect();
        fn64_render::OwnedRawDpcCapture::with_full_sync_boundaries(
            source.submission,
            memory_layout,
            token,
            cmd_end,
            boundaries,
        )
    };

    let observation = dpc_observation(xbus, observation_start, observation_end, &observation_words);

    crate::session_phase_census::note_submission();
    let planned = crate::session_phase_census::timed(
        crate::session_phase_census::Phase::Plan,
        || {
            RENDER_BACKEND.with(|backend_cell| {
                RAW_DPC_SESSION.with(|session_cell| {
                    let mut backend = backend_cell.borrow_mut();
                    let backend = backend.as_mut().expect(
                        "try_dispatch_raw_dpc_via_session: no render backend registered",
                    );
                    let session = session_cell.borrow();
                    let session = session.as_ref().expect(
                        "try_dispatch_raw_dpc_via_session: session vanished under this borrow",
                    );
                    let request = session.plan_request(capture);
                    backend
                        .plan_raw_dpc(request)
                        .unwrap_or_else(|error| panic!("plan_raw_dpc: {error}"))
                })
            })
        },
    );

    let guest_capture = fn64_render::ir::DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                let range = read.range();
                let start = range.start().get() as usize;
                let end = range.end() as usize;
                assert!(
                    end <= real.len(),
                    "plan_raw_dpc declared guest read [{start:#x}, {end:#x}) outside \
                     the captured source"
                );
                // **Logical order, not raw storage** -- the same byte-lane
                // authority the committed-write direction below already
                // observes, applied to the read direction.
                //
                // `CapturedGuestRead`'s contract is N64-logical bytes, and the
                // TMEM load executors index the capture linearly with no lane
                // mapping of their own. `real` is a bare pointer slice over
                // ABI storage, where bytes sit under the `^3` map, so a raw
                // `to_vec()` handed the sampler every 32-bit word
                // byte-reversed: "adjacent columns swapped AND each halfword
                // byte-reversed", exactly the symptom this file's own
                // write-back doc records for the outlier raw copy that was
                // fixed there.
                //
                // Command words survived the raw read by accident -- `^3`
                // composed with a little-endian host load cancels for an
                // aligned 32-bit word -- which is why this was invisible in
                // command decode and fatal only for byte-granular texture
                // data.
                //
                // Measured: with the raw copy, an eight-texel RGBA16 parity
                // fixture sampled the raw storage halfwords (`0xc107` where
                // `0xf801` was staged, all eight explained by that one rule)
                // while RT64 read the identical buffer and returned the key.
                // With this, both backends are byte-identical to the key.
                let mut bytes = vec![0; end - start];
                fn64_runtime::RdramView::from_storage(real).copy_logical_bytes(
                    fn64_runtime::RdramAddr::from_offset(range.start().get()),
                    &mut bytes,
                );
                fn64_render::ir::CapturedGuestRead::try_new(*read, bytes)
                    .unwrap_or_else(|error| panic!("CapturedGuestRead::try_new: {error}"))
            })
            .collect(),
    );

    let bound = RAW_DPC_SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let session = session
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
        crate::session_phase_census::timed(
            crate::session_phase_census::Phase::Finalize,
            || {
                session
                    .finalize_and_submit(planned, guest_capture)
                    .unwrap_or_else(|error| panic!("finalize_and_submit: {error}"))
            },
        )
    });

    let prepared = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
        crate::session_phase_census::timed(
            crate::session_phase_census::Phase::Execute,
            || {
                backend
                    .execute_raw_dpc(bound)
                    .unwrap_or_else(|error| panic!("execute_raw_dpc: {error}"))
            },
        )
    });

    // The guest-visible `RenderTarget` writes the backend staged for THIS
    // submission during the `execute_raw_dpc` call just above, read back in
    // its own borrow because `RENDER_BACKEND` and `RAW_DPC_SESSION` are
    // separate `RefCell`s that this function has always borrowed
    // separately. Empty for every TMEM-only and triangle-only submission,
    // which is every submission admitted before FillRectangle was.
    //
    // This list is transport, not authority: whichever commit branch it
    // selects below re-validates it against the packet's own journal and
    // against the backend's already-issued `BackendEffectReport`.
    let staged_writes = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
        backend.staged_guest_render_target_writes(prepared.submission())
    });

    let submission_identity = prepared.submission();
    let commit_writes = staged_writes.clone();
    let committed = RAW_DPC_SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let session = session
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
        crate::session_phase_census::timed(
            crate::session_phase_census::Phase::Commit,
            || {
                if staged_writes.is_empty() {
                    session
                        .commit_zero_guest_writes(prepared)
                        .unwrap_or_else(|error| panic!("commit_zero_guest_writes: {error}"))
                } else {
                    session
                        .commit_guest_render_target_writes(prepared, staged_writes)
                        .unwrap_or_else(|error| {
                            panic!("commit_guest_render_target_writes: {error}")
                        })
                }
            },
        )
    });

    // The RDRAM copyback, and the ONLY place this path writes a guest byte.
    //
    // Strictly after the commit above, never speculatively: reaching this
    // line means `commit_guest_render_target_writes` already re-validated
    // every element's access mode/purpose and then, through
    // `GuestCommitEffectReport::try_new`, its count, order, identity, and
    // content digest against the packet's own guest-write journal. The
    // journal -- not the backend -- is therefore the authority for which
    // ranges may be written, and a backend that reported a fabricated list
    // panicked above rather than reaching here.
    //
    // Supersedes the T-17 nonclaim ("nothing in the FillRectangle admission
    // chain writes guest RDRAM"), deliberately and with its test replaced by
    // ones asserting the new behavior --
    // `tests::raw_dpc_session_integration`'s
    // `an_admitted_whole_target_fill_writes_its_image_into_guest_rdram`,
    // `an_admitted_partial_width_fill_writes_only_its_own_disjoint_rows`,
    // and `an_admitted_odd_origin_fill_writes_target_relative_columns_into_guest_rdram`.
    // `a_rejected_guest_commit_leaves_guest_rdram_untouched` pins the
    // after-the-commit ordering this `if` depends on, and
    // `a_tmem_only_submission_writes_no_guest_target_byte` pins the gate.
    if !commit_writes.is_empty() {
        copy_committed_guest_writes(real, submission_identity, &commit_writes);
    }

    // Mirrors the legacy path's own `transaction.validate_atomic_completion()`
    // call (see `dispatch_dpc_submission`'s `Rdram` arm and
    // `dispatch_captured_raw_rdp`): the compatibility acknowledgment this
    // transaction opened at `LiveDpcTransaction::new` must be driven to
    // `Complete` before `with_ready_commit` will accept it -- required by
    // `with_ready_commit`'s own precondition assertion, independent of
    // which path (legacy or T4 session) produced the completed backend
    // result.
    transaction.validate_atomic_completion();

    // `with_ready_commit` hands the live `ReadyDpcFabricCommit` to this
    // closure INSIDE its one `with_host` borrow (see its own doc comment);
    // `seal_publication`/`publish_raw_dpc` run here, not after, so the fabric
    // token's prepare -> seal -> publish sequence stays exactly as ordered
    // as the legacy path's own prepare-then-commit, just carrying a capsule
    // through the middle instead of committing immediately.
    let outcome = transaction.with_ready_commit(|ready| {
        RAW_DPC_SESSION.with(|session_cell| {
            let mut session = session_cell.borrow_mut();
            let session = session
                .as_mut()
                .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
            let capsule = session
                .seal_publication(committed, ready)
                .unwrap_or_else(|error| panic!("seal_publication: {error}"));
            RENDER_BACKEND.with(|backend_cell| {
                let mut backend = backend_cell.borrow_mut();
                let backend = backend
                    .as_mut()
                    .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
                backend.publish_raw_dpc(capsule)
            })
        })
    });
    let _ = outcome;

    record_rsp_rdp_observations(vec![observation.clone()]);
    record_rdp_renderer_publication_v1();
    // Commit half of the FullSync two-phase contract.
    //
    // `DpFullSyncStatus` keeps its exact existing meaning here -- "the
    // backend reached the opcode" -- which is why no fourth variant was
    // added: this enum is consumed by sticky-OR in five places
    // (`rsp_commit.rs`'s two loops and `advance_one`, `raw_dpc_batch.rs`'s
    // `aggregate_full_sync`, and the reference backend's `imp.rs`), and any
    // new variant would read as "no interrupt" in every `!= Reached` test.
    //
    // Reporting `Reached` routes this submission into the caller's sticky-OR
    // and, eventually, `complete_committed_dpc`'s `start_live_dp_full_sync`
    // -- the mutating commit half that actually schedules the DP event. That
    // is the same commit the legacy path performs for the same command
    // stream; the T4 path no longer silently swallows it.
    //
    // Nonclaim: `Reached` means the opcode was walked and the slot was
    // reserved. It does NOT mean the guest observed a DP interrupt. That
    // claim lives only in a `FullSyncBoundary` whose `interrupt_after` is
    // `Asserted`, and this path supplies `Clear` -- see the honesty boundary
    // comment where the boundaries are built.
    let full_sync = if full_sync_sites == 0 {
        fn64_render::DpFullSyncStatus::NotReached
    } else {
        fn64_render::DpFullSyncStatus::Reached
    };
    Some((full_sync, observation))
}

/// Copy one already-committed submission's guest render-target writes into
/// live RDRAM, and nothing else.
///
/// Called only from `try_dispatch_raw_dpc_via_session`, and only after
/// `commit_guest_render_target_writes` returned `Ok`. `writes` is that exact
/// committed list, so every range here has already been validated against
/// the packet's own guest-write journal by
/// `GuestCommitEffectReport::try_new`.
///
/// **The copy is self-checking.** Each write's committed `ContentDigest` is
/// re-derived from the bytes the backend hands over, in the same
/// `ir_effect_content_digest` domain, and a mismatch panics BEFORE any byte
/// is written. A backend whose byte transport disagrees with the digest it
/// already committed is a defect that must be loud, not one that silently
/// scribbles a wrong rectangle into guest memory. The digest is the
/// authority; the bytes are the payload it vouched for.
///
/// **Exactly the committed ranges, no more.** Each `CompletedWrite` is
/// copied at its own `ResourceRegion::Rdram` range and nowhere else. A
/// partial-width fill declares N *disjoint* per-row ranges strided by the
/// color image's width (`fn64-render-wgpu`'s `raw_dpc::plan_fill` collapses
/// to a single range only when the rectangle spans the full image width), so
/// this loop writes N separate spans and never the gaps between them.
/// Collapsing them into one span would claim far more bytes than the fill
/// wrote.
///
/// **Byte-lane mapping: the payload is LOGICAL, the storage is PHYSICAL.**
/// The backend hands over guest-order bytes -- `targets/fill.rs`'s
/// `write_pixel` emits `packed.to_be_bytes()`, big-endian as the RDP writes
/// them -- while this crate's RDRAM allocation is N64Recomp native-word
/// storage, where a logical byte at offset `o` lives at `o ^ 3`
/// (`crates/fn64-runtime/src/rdram.rs`'s module doc, transcribed from
/// `recomp.h`'s `MEM_B`/`MEM_H`). So the copy goes through
/// `RdramViewMut::write_logical_bytes`, which owns that one mapping.
///
/// This was a `copy_from_slice` into the raw allocation and that was WRONG,
/// measured not argued: the VI reads the same memory through
/// `PhysicalRdramRead::read_u16`'s `^2` lane XOR, so a raw-copied fill
/// presented with adjacent columns swapped AND each halfword byte-reversed.
/// The lane-mapped convention is the established one -- the reference
/// backend's own RDP writeback uses `view.write_u16`
/// (`crates/fn64-render-reference/src/backend/framebuffer_io.rs:188`) and
/// `vi_scanout.rs`'s "Byte-lane authority" section names it as the single
/// authority -- and the raw copy here was the outlier. The two legacy
/// copybacks in this file stay raw and are NOT the same case: they round-trip
/// `real` through a whole-RDRAM `image`, so their bytes are already physical.
///
/// **Byte granularity, not halfword.** `write_logical_bytes` maps one byte at
/// a time (`^3`), so it is correct for an arbitrary `CompletedWrite` range
/// with no alignment or even-length precondition. A `write_u16` loop would
/// need both and a committed range guarantees neither -- it is a byte range
/// whose seam is byte-typed, even though a fill's rows happen to be RGBA16.
///
/// **What the digest covers: the PAYLOAD, not the memory image.** The
/// `ir_effect_content_digest` re-check below hashes the backend's logical
/// bytes exactly as handed over, before any lane mapping. That is the right
/// domain and not an oversight: the digest is the backend's own commitment
/// about the content it rendered, and the backend has no opinion about host
/// storage layout. Hashing the post-mapping image would compare a value the
/// backend never computed, and would make the self-check pass or fail on
/// this crate's storage convention rather than on byte transport integrity.
///
/// Writes go through `track_rdp_renderer_mutation` for the same reason the
/// legacy `dispatch_captured_raw_rdp` path does: a guest-visible renderer
/// write must reach the write-barrier journal, not bypass it. The tracker is
/// handed the WHOLE `real` allocation, not the destination subslice: it
/// snapshots and diffs watched ranges by absolute physical offset
/// (`recompiled/snapshots.rs`'s `track_catalog_nested_mutation` reads through
/// `RdramView::read_u8(RdramAddr::from_offset(physical))`), so a subslice
/// would have made every watched offset name the wrong byte.
fn copy_committed_guest_writes(
    real: &mut [u8],
    submission: fn64_render::ir::SubmissionIdentity,
    writes: &[fn64_render::ir::CompletedWrite],
) {
    let payloads = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("copy_committed_guest_writes: no render backend registered");
        backend.committed_guest_render_target_bytes(submission)
    });

    assert_eq!(
        payloads.len(),
        writes.len(),
        "the backend committed {} guest render-target write(s) but produced bytes for {} -- \
         a committed write with no bytes behind it is a backend defect, never a reason to \
         copy a partial rectangle",
        writes.len(),
        payloads.len()
    );

    // Every payload is validated against its own committed write BEFORE the
    // first byte is copied, so a mismatch in the last write cannot leave the
    // earlier ones already applied.
    //
    // The digest assertion below is deliberately kept even though deleting
    // it leaves every test's FINAL RDRAM state unchanged -- measured, by
    // mutation, not assumed. Corrupting one halfword in the backend's byte
    // transport (`committed_guest_render_target_bytes`) trips this assertion
    // and no guest byte is written. Delete the assertion and the same
    // corruption reaches guest memory, where it is caught only afterwards by
    // a test's own pixel comparison. The two mutants are equivalent in
    // outcome and NOT equivalent in blast radius: one is a loud trap before
    // the write, the other is silent guest-memory corruption that happens to
    // be observed downstream. AGENTS.md's loud-trap rule decides that
    // tie -- this is the guard, not a redundant check.
    for (index, (write, bytes)) in writes.iter().zip(payloads.iter()).enumerate() {
        assert_eq!(
            bytes.len() as u32,
            write.byte_count(),
            "committed guest write #{index} declares {} byte(s) but its payload is {}",
            write.byte_count(),
            bytes.len()
        );
        assert_eq!(
            fn64_render::ir_effect_content_digest(bytes),
            write.content(),
            "committed guest write #{index}'s payload does not hash to the ContentDigest the \
             backend already committed for it"
        );
    }

    for (write, bytes) in writes.iter().zip(payloads.iter()) {
        let fn64_render::ir::ResourceRegion::Rdram { range, .. } = write.access().region() else {
            panic!(
                "a committed guest render-target write must name an RDRAM region; \
                 commit_guest_render_target_writes admitted a write that does not"
            );
        };
        let start = range.start().get() as usize;
        let end = range.end() as usize;
        assert!(
            start <= end && end <= real.len(),
            "committed guest write range [{start:#x}, {end:#x}) is outside the registered \
             RDRAM allocation of {:#x} bytes",
            real.len()
        );
        assert_eq!(
            end - start,
            bytes.len(),
            "committed guest write range [{start:#x}, {end:#x}) spans {} byte(s) but its \
             payload is {}",
            end - start,
            bytes.len()
        );
        let addr = fn64_runtime::RdramAddr::from_offset(
            u32::try_from(start).expect("committed guest write offset exceeds u32"),
        );
        track_rdp_renderer_mutation(real, |real| {
            fn64_runtime::RdramViewMut::from_storage(real).write_logical_bytes(addr, bytes);
        });
    }
}

/// Own the ABI side of an explicitly scheduled raw-DPC renderer transaction.
///
/// The renderer receives only a shadow image. Once it has been called, any
/// error or malformed success poisons the schedule: the backend may already
/// have consumed its private continuation, so retrying the same request would
/// duplicate work. A valid acknowledgment publishes schedule state, shadow
/// memory, continuation, and cumulative FullSync evidence as one transition.
#[cfg(test)]
pub(crate) struct ScheduledRawDpcTransaction {
    pub(crate) execution: fn64_runtime::DpcScheduledExecution,
    pub(crate) continuation: Option<fn64_render::RenderRawDpcContinuation>,
    pub(crate) full_sync: fn64_render::DpFullSyncStatus,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum ScheduledRawDpcError {
    Schedule(fn64_runtime::DpcScheduleError),
    Backend(fn64_render::RenderError),
    UnidentifiedFullSync,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScheduledRawDpcAdvance {
    Reached {
        at: fn64_runtime::Cycles,
    },
    Committed {
        at: fn64_runtime::Cycles,
        phase: fn64_runtime::DpcScheduledPhase,
        full_sync: fn64_render::DpFullSyncStatus,
    },
}

#[cfg(test)]
impl ScheduledRawDpcTransaction {
    pub(crate) fn new(execution: fn64_runtime::DpcScheduledExecution) -> Self {
        Self {
            execution,
            continuation: None,
            full_sync: fn64_render::DpFullSyncStatus::NotReached,
        }
    }

    pub(crate) fn advance_one(
        &mut self,
        requested: fn64_runtime::Cycles,
        backend: &mut dyn RenderBackend,
        rdram: &mut [u8],
        output_addr: u32,
    ) -> Result<ScheduledRawDpcAdvance, ScheduledRawDpcError> {
        let (at, action) = match self
            .execution
            .advance_to(requested)
            .map_err(ScheduledRawDpcError::Schedule)?
        {
            fn64_runtime::DpcAdvance::Reached { at } => {
                return Ok(ScheduledRawDpcAdvance::Reached { at });
            }
            fn64_runtime::DpcAdvance::Blocked { at, action } => (at, action),
        };

        let step = self.continuation.take().map_or(
            fn64_render::RawDpcStep::Start,
            fn64_render::RawDpcStep::Resume,
        );
        let mut shadow = rdram.to_vec();
        let ack = match backend.process_rdp_command_chunk(
            &mut shadow,
            fn64_render::RawDpcQuantum {
                request: action,
                output_addr,
            },
            step,
        ) {
            Ok(ack) => ack,
            Err(error) => {
                self.execution.poison();
                return Err(ScheduledRawDpcError::Backend(error));
            }
        };
        if ack.full_sync == fn64_render::DpFullSyncStatus::Unidentified {
            self.execution.poison();
            return Err(ScheduledRawDpcError::UnidentifiedFullSync);
        }
        let (status, continuation) = match ack.status {
            fn64_render::RawDpcChunkStatus::Continue(token) => {
                (fn64_runtime::DpcBackendQuantumStatus::Continue, Some(token))
            }
            fn64_render::RawDpcChunkStatus::Complete => {
                (fn64_runtime::DpcBackendQuantumStatus::Complete, None)
            }
        };
        if let Err(error) = self
            .execution
            .acknowledge(fn64_runtime::DpcBackendQuantumAck {
                transaction: ack.transaction,
                quantum: ack.quantum,
                committed_through: ack.committed_through,
                status,
            })
        {
            self.execution.poison();
            return Err(ScheduledRawDpcError::Schedule(error));
        }

        // No fallible work follows this point. These fields form the ABI's
        // publication boundary for the already-validated backend result.
        rdram.copy_from_slice(&shadow);
        self.continuation = continuation;
        if ack.full_sync == fn64_render::DpFullSyncStatus::Reached {
            self.full_sync = fn64_render::DpFullSyncStatus::Reached;
        }
        Ok(ScheduledRawDpcAdvance::Committed {
            at,
            phase: self.execution.phase(),
            full_sync: self.full_sync,
        })
    }

    pub(crate) fn phase(&self) -> fn64_runtime::DpcScheduledPhase {
        self.execution.phase()
    }

    pub(crate) fn cursor(&self) -> fn64_runtime::DpcCursor {
        self.execution.cursor()
    }

    pub(crate) fn continuation(&self) -> Option<fn64_render::RenderRawDpcContinuation> {
        self.continuation
    }

    pub(crate) fn full_sync(&self) -> fn64_render::DpFullSyncStatus {
        self.full_sync
    }
}

pub(crate) fn complete_committed_dpc(
    transaction: LiveDpcTransaction,
    full_sync: fn64_render::DpFullSyncStatus,
    observation: RspRdpObservationKind,
    operation: &'static str,
) {
    transaction.commit();
    record_rsp_rdp_observations(vec![observation]);
    record_rdp_renderer_publication_v1();
    if full_sync == fn64_render::DpFullSyncStatus::Reached {
        crate::pi::start_live_dp_full_sync()
            .unwrap_or_else(|error| panic!("{operation}: DP FullSync completion: {error}"));
    }
}


/// A closed capture must not end inside a command.
///
/// The legacy/staged dispatch paths hold a stream that is already fully
/// assembled -- there is no later END write to expose more bytes -- so an
/// incomplete tail means an upstream assembler broke its contract. Only the
/// raw CPU MMIO ingress may park a tail, and it does so in the fabric before
/// ever reaching these paths.
fn require_complete_raw_dpc_scan(
    scanned: fn64_render::RawRdpScan<fn64_render::DpFullSyncStatus, u32>,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    match scanned {
        fn64_render::RawRdpScan::Complete(status) => status,
        fn64_render::RawRdpScan::Incomplete {
            command_start,
            bytes_required,
            bytes_available,
            ..
        } => panic!(
            "{operation}: closed capture ends inside the command at {command_start:#010x} \
             ({bytes_available} of {bytes_required} bytes present); an extendable stream must \
             be parked by the fabric, never dispatched"
        ),
    }
}
/// Returns the scan outcome so the CALLER applies its ingress policy.
///
/// A truncated tail is not an error: hardware stalls CURRENT at that
/// command's start until a later END exposes the rest. Callers holding a
/// CLOSED capture (a coalesced RSP stream, a batch replay) must treat
/// `Incomplete` as a hard failure; only raw CPU MMIO ingress may park it.
/// A malformed opcode still panics here, as it always did.
pub(crate) fn preflight_raw_dpc_completion(
    image: &[u8],
    start: u32,
    end: u32,
    operation: &'static str,
) -> fn64_render::RawRdpScan<fn64_render::DpFullSyncStatus, u32> {
    let scanned = fn64_render::inspect_raw_rdp_full_sync(image, start, end)
        .unwrap_or_else(|error| panic!("{operation}: {error}"));
    let inspected = match scanned {
        fn64_render::RawRdpScan::Complete(status) => status,
        // The prefix's completion result still stands, but no FullSync slot
        // is reserved for a stream that has not finished arriving.
        fn64_render::RawRdpScan::Incomplete { .. } => return scanned,
    };
    if inspected == fn64_render::DpFullSyncStatus::Reached {
        // Interleaving closed here: a prior FullSync may remain pending while
        // the guest submits another DPC range. Device advancement and guest
        // callbacks cannot run during renderer dispatch, so observing an
        // empty slot here reserves it through the later synchronous commit;
        // observing an occupied slot rejects before backend or RDRAM mutation.
        with_host(|host| {
            host.device_fabric
                .preflight_dp_full_sync(fn64_runtime::Cycles::new(1))
        })
        .unwrap_or_else(|error| panic!("{operation}: DP FullSync completion: {error}"));
    }
    scanned
}

pub(crate) fn require_matching_raw_dpc_completion(
    inspected: fn64_render::DpFullSyncStatus,
    rendered: fn64_render::DpFullSyncStatus,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    assert_eq!(
        rendered, inspected,
        "{operation}: renderer FullSync evidence disagrees with the submitted raw RDP command stream"
    );
    rendered
}

/// Submit one fabric-owned CPU DPC transaction to the registered renderer.
/// DRAM reads the registered physical device; XBUS snapshots persistent DMEM
/// at the accepted END boundary. Renderer acceptance commits CURRENT before
/// FullSync can schedule DP completion; rejection cancels the token and
/// records no observation.
pub(crate) unsafe fn dispatch_dpc_submission(
    rdram: *mut u8,
    submission: fn64_runtime::DpcSubmission,
) {
    let start = submission.start;
    let end = submission.end;
    // T4 routing decision, made BEFORE `LiveDpcTransaction::new` ever runs:
    // whether a production raw-DPC session is registered is read once, up
    // front, so there is exactly one fabric-owning `LiveDpcTransaction` for
    // this submission either way -- never one constructed, probed, dropped
    // (which would cancel the still-wanted fabric submission), and rebuilt.
    let session_registered = RAW_DPC_SESSION.with(|cell| cell.borrow().is_some());
    if session_registered {
        let owned_submission = match submission.source {
            fn64_runtime::DpcSubmissionSource::Rdram => {
                let real_len = unsafe { renderer_rdram_slice(rdram) }.len();
                let start_usize = start as usize;
                let end_usize = end as usize;
                assert!(
                    start_usize < end_usize
                        && start_usize.is_multiple_of(8)
                        && end_usize.is_multiple_of(8),
                    "DRAM DPC range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
                );
                assert!(
                    end_usize <= real_len,
                    "DRAM DPC range end {end:#010x} exceeds registered RDRAM length {real_len:#x}"
                );
                let words = unsafe { renderer_rdram_slice(rdram) }[start_usize..end_usize]
                    .chunks_exact(4)
                    .map(|word| u32::from_ne_bytes(word.try_into().expect("four RDRAM bytes")))
                    .collect::<Vec<_>>();
                fn64_render::OwnedRawDpcSubmission::from_rdram_words(start, end, words)
                    .unwrap_or_else(|error| {
                        panic!(
                            "dispatch_raw_rdp: fabric-admitted DRAM range does not admit a T4 \
                             capture: {error:?}"
                        )
                    })
            }
            fn64_runtime::DpcSubmissionSource::Dmem => {
                let dmem = with_host(|host| {
                    *host
                        .device_fabric
                        .rsp_memory()
                        .bank(fn64_runtime::RspMemoryBank::Dmem)
                });
                let payload = dmem[start as usize..end as usize].to_vec();
                fn64_render::OwnedRawDpcSubmission::from_xbus_payload(start, end, payload)
                    .unwrap_or_else(|error| {
                        panic!(
                            "dispatch_raw_rdp_xbus: fabric-admitted XBUS range does not admit a \
                             T4 capture: {error:?}"
                        )
                    })
            }
        };
        let transaction = LiveDpcTransaction::new(submission);
        // `try_dispatch_raw_dpc_via_session` already commits the fabric
        // transaction (through `with_ready_commit`/`publish_raw_dpc`) and
        // records observations internally, exactly like
        // `complete_committed_dpc` does for the legacy path below.
        //
        // The DP completion is NOT already handled, though, and must be
        // driven here. `complete_committed_dpc` is what calls
        // `start_live_dp_full_sync` for the legacy path, and this branch
        // deliberately does not go through it. Before FullSync was admitted
        // this cost nothing because the session path could only ever report
        // `NotReached`; now that it can report `Reached`, discarding the
        // status would silently swallow the guest's DP interrupt for exactly
        // the submissions the site admission exists to handle.
        //
        // This is the mutating commit half of the two-phase contract. Its
        // reserve half already ran inside the call below, before the backend
        // was entered -- so this scheduling cannot fail for a slot reason
        // that a nonmutating check would have caught earlier.
        let (full_sync, _observation) = try_dispatch_raw_dpc_via_session(
            rdram,
            SessionRawDpcSource {
                submission: owned_submission,
            },
            transaction,
        )
        .expect("session_registered was already checked true under the same borrow");
        if full_sync == fn64_render::DpFullSyncStatus::Reached {
            crate::pi::start_live_dp_full_sync().unwrap_or_else(|error| {
                panic!("dispatch_raw_rdp: DP FullSync completion: {error}")
            });
        }
        return;
    }

    let mut transaction = LiveDpcTransaction::new(submission);
    let (full_sync, observation, operation) = match submission.source {
        fn64_runtime::DpcSubmissionSource::Rdram => {
            let (words, full_sync) = {
                let real = unsafe { renderer_rdram_slice(rdram) };
                let start_usize = start as usize;
                let end_usize = end as usize;
                assert!(
                    start_usize < end_usize
                        && start_usize.is_multiple_of(8)
                        && end_usize.is_multiple_of(8),
                    "DRAM DPC range [{start:#010x}, {end:#010x}) must be nonempty and 8-byte aligned"
                );
                assert!(
                    end_usize <= real.len(),
                    "DRAM DPC range end {end:#010x} exceeds registered RDRAM length {:#x}",
                    real.len()
                );
                let words = real[start_usize..end_usize]
                    .chunks_exact(4)
                    .map(|word| u32::from_ne_bytes(word.try_into().expect("four RDRAM bytes")))
                    .collect::<Vec<_>>();
                let mut image = real.to_vec();
                let inspected =
                    preflight_raw_dpc_completion(&image, start, end, "dispatch_raw_rdp");
                let result = with_render_backend("dispatch_raw_rdp", |backend| {
                    // `true`: this call site is single-shot per invocation,
                    // not the coalesced-submission loop in
                    // `dispatch_captured_raw_rdp` below, which is the one
                    // this session measured and verified safe to defer.
                    // Preserve existing (always-wait) behavior here.
                    let status = backend.process_rdp_commands(
                        &mut image,
                        start,
                        end,
                        render_output_addr(),
                        true,
                    )?;
                    Ok(RenderDispatchResult {
                        status,
                        dp_full_sync: backend.last_dp_full_sync(),
                    })
                });
                let rendered = require_committed_full_sync_evidence(result, "dispatch_raw_rdp");
                let full_sync =
                    require_matching_raw_dpc_completion(
                        require_complete_raw_dpc_scan(inspected, "dispatch_raw_rdp"),
                        rendered,
                        "dispatch_raw_rdp",
                    );
                transaction.validate_atomic_completion();
                track_rdp_renderer_mutation(real, |real| real.copy_from_slice(&image));
                (words, full_sync)
            };
            (
                full_sync,
                dpc_observation(false, start, end, &words),
                "dispatch_raw_rdp",
            )
        }
        fn64_runtime::DpcSubmissionSource::Dmem => {
            let dmem = with_host(|host| {
                *host
                    .device_fabric
                    .rsp_memory()
                    .bank(fn64_runtime::RspMemoryBank::Dmem)
            });
            let words = dmem[start as usize..end as usize]
                .chunks_exact(4)
                .map(|word| u32::from_be_bytes(word.try_into().expect("four DMEM bytes")))
                .collect::<Vec<_>>();
            let (full_sync, observation) = unsafe {
                dispatch_captured_raw_rdp(rdram, &words, start, end, true, true, &mut transaction)
            };
            (full_sync, observation, "dispatch_raw_rdp_xbus")
        }
    };
    complete_committed_dpc(transaction, full_sync, observation, operation);
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) unsafe fn dispatch_raw_rdp(rdram: *mut u8, start: u32, end: u32) {
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Rdram,
            start,
            end,
        )
    })
    .unwrap_or_else(|error| panic!("dispatch_raw_rdp: DPC submission rejected: {error}"));
    if let Some(submission) = submission {
        unsafe { dispatch_dpc_submission(rdram, submission) };
    }
}

/// Submit an XBUS DPC range whose command bytes live in persistent RSP DMEM.
/// The renderer seam accepts an RDRAM image, so the command span is staged
/// after the real allocation in a synthetic image. Only the original RDRAM
/// prefix is copied back after rendering, but RDP commands can still address
/// the suffix while executing; this is not exact physical-memory isolation.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) unsafe fn dispatch_raw_rdp_xbus(
    rdram: *mut u8,
    dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    start: u32,
    end: u32,
) {
    let start = start & 0x0fff;
    let end = end & 0x0fff;
    let start_usize = start as usize;
    let end_usize = end as usize;
    assert!(
        start < end && start.is_multiple_of(8) && end.is_multiple_of(8),
        "RSP XBUS DPC range [{start:#05x}, {end:#05x}) must be nonempty and 8-byte aligned"
    );
    assert!(
        end_usize <= dmem.len(),
        "RSP XBUS DPC range end {end:#05x} exceeds 4 KiB DMEM"
    );
    let words = dmem[start_usize..end_usize]
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four DMEM bytes")))
        .collect::<Vec<_>>();
    let submission = with_host(|host| {
        host.device_fabric.request_dpc_submission(
            fn64_runtime::DpcSubmissionSource::Dmem,
            start,
            end,
        )
    })
    .unwrap_or_else(|error| panic!("dispatch_raw_rdp_xbus: DPC submission rejected: {error}"));
    let Some(submission) = submission else {
        return;
    };
    let mut transaction = LiveDpcTransaction::new(submission);
    let (full_sync, observation) = unsafe {
        dispatch_captured_raw_rdp(rdram, &words, start, end, true, true, &mut transaction)
    };
    complete_committed_dpc(transaction, full_sync, observation, "dispatch_raw_rdp_xbus");
}

/// Submit command words already captured at the DPC CMD_END boundary through
/// a synthetic staging suffix. Prefix-only copyback prevents the synthetic
/// bytes themselves from entering guest RDRAM, but does not stop commands from
/// reading or targeting the suffix through the RDP's 24-bit address space.
/// Exact native LLE capture therefore still requires a separate command buffer
/// and a physical-memory access bound at the renderer seam.
pub(crate) unsafe fn dispatch_captured_raw_rdp(
    rdram: *mut u8,
    words: &[u32],
    source_start: u32,
    source_end: u32,
    xbus: bool,
    wait_for_completion: bool,
    transaction: &mut LiveDpcTransaction,
) -> (fn64_render::DpFullSyncStatus, RspRdpObservationKind) {
    assert!(
        !words.is_empty() && words.len().is_multiple_of(2),
        "captured RSP DPC submission must contain a nonempty whole number of 64-bit commands"
    );
    if !xbus {
        assert_eq!(
            source_end.checked_sub(source_start),
            Some(u32::try_from(words.len() * 4).expect("captured RSP DPC byte length exceeds u32")),
            "captured DRAM DPC source range does not match the captured command image"
        );
    }
    let real = unsafe { renderer_rdram_slice(rdram) };
    let physical_len = real.len();
    let xbus_diagnostics = xbus_diagnostics();
    if xbus {
        if let Some(dump) = xbus_diagnostics.stream_dump.as_ref() {
            thread_local! {
                static XBUS_DUMP_INDEX: Cell<u64> = const { Cell::new(0) };
            }
            let index = XBUS_DUMP_INDEX.with(|cell| {
                let index = cell.get();
                cell.set(index + 1);
                index
            });
            if index >= dump.skip && index < dump.skip.saturating_add(16) {
                std::fs::create_dir_all(&dump.directory).unwrap_or_else(|error| {
                    panic!("FN64_XBUS_STREAM_DUMP_DIR {:?}: {error}", dump.directory)
                });
                let stream = words
                    .iter()
                    .flat_map(|word| word.to_be_bytes())
                    .collect::<Vec<_>>();
                let path = dump.directory.join(format!("xbus-{index:04}.bin"));
                std::fs::write(&path, stream)
                    .unwrap_or_else(|error| panic!("writing XBUS stream dump {path:?}: {error}"));
                eprintln!(
                    "[fn64-abi] dumped XBUS stream #{index} ({} bytes) to {}",
                    words.len() * 4,
                    path.display()
                );
                if dump.rdram_index == Some(index) {
                    let rdram_path = dump.directory.join(format!("rdram-{index:04}.bin"));
                    std::fs::write(&rdram_path, &*real).unwrap_or_else(|error| {
                        panic!("writing RDRAM dump {rdram_path:?}: {error}")
                    });
                    eprintln!(
                        "[fn64-abi] dumped RDRAM image ({physical_len:#x} bytes) to {}",
                        rdram_path.display()
                    );
                }
            }
        }
    }
    let staging_start = physical_len;
    let staged_end = staging_start + words.len() * 4;
    assert!(
        staged_end <= 0x0100_0000,
        "captured RSP DPC staging range [{staging_start:#010x}, {staged_end:#010x}) exceeds the 24-bit RDP address space"
    );
    crate::dpc_copy_census::note_call();
    let mut image = RAW_DPC_STAGING_SCRATCH.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    crate::dpc_copy_census::timed(
        crate::dpc_copy_census::Phase::Alloc,
        staged_end as u64,
        || image.resize(staged_end, 0),
    );
    crate::dpc_copy_census::timed(
        crate::dpc_copy_census::Phase::CopyIn,
        physical_len as u64,
        || image[..physical_len].copy_from_slice(real),
    );
    for (word_index, value) in words.iter().copied().enumerate() {
        let offset = staging_start + word_index * 4;
        image[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
    }
    let inspected = preflight_raw_dpc_completion(
        &image,
        staging_start as u32,
        staged_end as u32,
        "dispatch_captured_raw_rdp",
    );
    let result = with_render_backend("dispatch_captured_raw_rdp", |backend| {
        let status = backend.process_rdp_commands(
            &mut image,
            staging_start as u32,
            staged_end as u32,
            render_output_addr(),
            wait_for_completion,
        )?;
        Ok(RenderDispatchResult {
            status,
            dp_full_sync: backend.last_dp_full_sync(),
        })
    });
    let rendered = require_committed_full_sync_evidence(result, "dispatch_captured_raw_rdp");
    let full_sync =
        require_matching_raw_dpc_completion(
            require_complete_raw_dpc_scan(inspected, "dispatch_captured_raw_rdp"),
            rendered,
            "dispatch_captured_raw_rdp",
        );
    transaction.validate_atomic_completion();
    if xbus && xbus_diagnostics.diff_trace {
        let mut offset = 0usize;
        while offset < physical_len {
            if image[offset] != real[offset] {
                let start = offset;
                while offset < physical_len && image[offset] != real[offset] {
                    offset += 1;
                }
                eprintln!(
                    "[fn64-abi] XBUS diff: renderer changed rdram [{start:#010x}, {offset:#010x}) ({} bytes)",
                    offset - start
                );
            } else {
                offset += 1;
            }
        }
    }
    // How much of the copyback below is real work. MUST run before it: the
    // copy destroys the very difference being measured. Inert without
    // `FN64_DPC_COPY_CENSUS`, and it copies nothing either way -- it exists
    // to answer, by counting rather than by argument (perf-method rule 3),
    // whether narrowing the copyback can pay at all. See the counters'
    // doc comment for why the 3.4x copy_in/copy_back asymmetry raised the
    // question and why rule 12 requires it be settled before acting.
    crate::dpc_copy_census::note_copy_back_diff(&image[..physical_len], real);
    // Times the copyback only, NOT `track_rdp_renderer_mutation`'s own
    // bookkeeping around it: the closure is the memcpy and the wrapper is
    // mutation-journal work, and conflating them would reproduce exactly the
    // inclusive-as-self-time error this census exists to avoid.
    track_rdp_renderer_mutation(real, |real| {
        crate::dpc_copy_census::timed(
            crate::dpc_copy_census::Phase::CopyBack,
            physical_len as u64,
            || real.copy_from_slice(&image[..physical_len]),
        )
    });
    let observation = dpc_observation(xbus, source_start, source_end, words);
    RAW_DPC_STAGING_SCRATCH.with(|cell| *cell.borrow_mut() = image);
    (full_sync, observation)
}

pub(crate) fn require_committed_full_sync_evidence(
    result: RenderDispatchResult,
    operation: &'static str,
) -> fn64_render::DpFullSyncStatus {
    match result.status {
        fn64_render::FrameStatus::Complete => match result.dp_full_sync {
            fn64_render::DpFullSyncStatus::Unidentified => {
                panic!("{operation}: renderer completed without identifying DP FullSync state")
            }
            status => status,
        },
        fn64_render::FrameStatus::Yielded => {
            panic!("{operation}: raw RDP submission cannot yield as an RSP task")
        }
        fn64_render::FrameStatus::NeedsLle { .. } => {
            panic!("{operation}: raw RDP submission cannot request RSP LLE fallback")
        }
    }
}

/// Dispatch a translated audio task (`M_AUDTASK`) once, at the point the RSP
/// is kicked. `o` is the OSTask's RDRAM offset, which the translated ucode's
/// FFI wrapper uses to seed RSP DMEM[0xFC0]. Missing execution policy and every
/// callback result other than RSP BREAK trap before completion or an
/// execution-qualified task trace can be produced.
///
/// Sample delivery happens later at `osAiSetNextBuffer_recomp`, the public AI
/// DMA boundary where the CPU names the actual finished PCM range. It cannot
/// happen here: OoT's live task has zero `OSTask.output_buff` fields and
/// selects output destinations through `A_SAVEBUFF` commands.
///
/// This runs only from `osSpTaskStartGo_recomp`, the Load+StartGo path OoT's
/// audio driver uses (`AudioMgr_HandleRetrace` -> scheduler -> `Sched_RunTask`
/// -> `osSpTaskLoad`+`osSpTaskStartGo`). A prior version dispatched the ucode
/// only from the later yield-status query, so a normal task never ran while a
/// query could run it twice.
///
/// # Safety
/// `rdram` valid for the call; `o` a valid task-header offset within it and
/// `header` the task header read from that offset.
pub(crate) unsafe fn dispatch_audio_task(
    rdram: *mut u8,
    o: usize,
    header: &OsTaskHeader,
    callback: AudioUcodeFn,
) {
    debug_assert_eq!(header.task_type, M_AUDTASK);
    // Before PHASE_TIMING's thread_local initializer reads the NEW name: a run
    // that set only the old spelling must trap here, not silently proceed.
    assert_no_legacy_env_vars();
    let started = PHASE_TIMING.with(Cell::get).then(std::time::Instant::now);
    // Phase timing measures the translated audio ucode (the per-frame RSP
    // synth, currently unoptimized and the dominant per-swap cost).
    // The shared task dump happens before policy dispatch in
    // `osSpTaskStartGo_recomp`, so translated and LLE tasks expose the same
    // immutable input boundary.
    //
    // The CPU-side audio driver (AudioThread_UpdateImpl) and this task's
    // completion event still run, so the audio-reset handshake that unblocks
    // Play_Init still completes.
    // Safety: translated registration atomically pairs this callback with its
    // process-lifetime evidence identity. `o` is the admitted OSTask offset.
    // Both muts are exercised only under the recomp-rs feature (the closure
    // assigns tracked_hle_publication there); the default build sees them
    // immutable and would warn.
    #[allow(unused_mut)]
    let mut tracked_hle_publication = None;
    #[allow(unused_mut)]
    let mut invoke = || {
        #[cfg(feature = "recomp-rs")]
        if with_host(|host| host.canonical_recompiled_program.is_some()) {
            let owner = task_interpreter_owner(RdramAddr::from_offset(
                u32::try_from(o).expect("translated audio task offset exceeds u32"),
            ));
            let rdram = unsafe { renderer_rdram_slice(rdram) };
            let (reason, journal_sequences) =
                track_rsp_execution_or_hle_mutation(rdram, |rdram| unsafe {
                    callback(rdram.as_mut_ptr(), o as u32)
                });
            tracked_hle_publication = Some((owner, journal_sequences));
            return reason;
        }
        unsafe { callback(rdram, o as u32) }
    };
    let reason = if AUDIO_UCODE_TIMING.with(|c| c.get()) {
        let t = std::time::Instant::now();
        let reason = invoke();
        let ns = t.elapsed().as_nanos() as u64;
        AUDIO_UCODE_NS.with(|c| c.set(c.get() + ns));
        AUDIO_UCODE_CALLS.with(|c| c.set(c.get() + 1));
        reason
    } else {
        invoke()
    };
    if let Some((owner, journal_sequences)) = tracked_hle_publication {
        // The callback may have changed executable backing before returning a
        // non-BREAK reason. Classifying the result before appending a success
        // closes the interleaving where a caught unwind could otherwise let a
        // later audit absorb a speculative callback's journal entry.
        finish_translated_audio_hle_publication_v1(
            RspWriterCommitSourceV1::TranslatedAudioHle { owner },
            journal_sequences,
            reason == 0,
        );
    }
    if reason != 0 {
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Audio,
            "audio.translated-ucode.exit-reason",
            format!("task_offset={o:#010x} exit_reason={reason}"),
            Some(fn64_runtime::Cycles::new(crate::sim_time())),
            fn64_runtime::UnsupportedDisposition::LoudTrap,
        );
        panic!(
            "audio.translated-ucode.exit-reason: task {o:#010x} returned non-BREAK reason {reason}"
        );
    }
    if let Some(started) = started {
        AUDIO_DISPATCH_NS.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            );
        });
        AUDIO_DISPATCH_CALLS.with(|calls| calls.set(calls.get() + 1));
    }
}

/// Exercise the production translated-audio dispatcher with a canonical task
/// owner. This wrapper exists only so the recompiler authority tests can drive
/// the real callback classification boundary without manufacturing a trace
/// observation directly.
#[cfg(all(test, feature = "recomp-rs"))]
pub(crate) unsafe fn test_dispatch_translated_audio_task_v1(
    task_offset: u32,
    callback: unsafe extern "C" fn(*mut u8, u32) -> u32,
) {
    let task_addr = RdramAddr::from_offset(task_offset);
    let generation = RspTaskAdmissionGeneration::first();
    let owner = RspInterpreterOwner::task(task_offset, generation);
    let (rdram, rdram_len) = with_host(|host| {
        host.rsp_task_lineages.insert(
            task_offset,
            RspTaskLineage {
                admission_generation: generation,
                original_header: OsTaskHeader::default(),
                data_identity: None,
                phase: RspTaskLineagePhase::Running,
            },
        );
        host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::InFlight { owner };
        (host.runtime_rdram, host.runtime_rdram_len)
    });
    assert!(
        !rdram.is_null() && rdram_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "translated-audio test requires canonical physical RDRAM"
    );
    struct TestRdramRegistration(usize);
    impl Drop for TestRdramRegistration {
        fn drop(&mut self) {
            RDRAM_LEN.with(|cell| cell.set(self.0));
        }
    }
    let registration = TestRdramRegistration(RDRAM_LEN.with(|cell| cell.replace(rdram_len)));
    let header = OsTaskHeader {
        task_type: M_AUDTASK,
        ..OsTaskHeader::default()
    };
    unsafe { dispatch_audio_task(rdram, task_offset as usize, &header, callback) };
    commit_rsp_hle_compatibility(task_addr, None);
    retire_running_rsp_task_lineage(task_addr, "translated-audio production-path test");
    drop(registration);
}

/// Capture one immutable audio-task input before translated or LLE execution.
///
/// The output is private diagnostic material and remains outside git. Keeping
/// this at the common task-kick boundary makes a capture independent of the
/// installed execution policy, which is required for later LLE/HLE replay.
///
/// # Safety
/// `rdram` must cover the registered process RDRAM length, and `task_offset`
/// must name the admitted `OSTask` inside it.
pub(crate) unsafe fn maybe_dump_audio_task_input(rdram: *mut u8, task_offset: usize) {
    let Some(path) = std::env::var_os("FN64_DUMP_AUDIO_TASK") else {
        return;
    };
    let target_index = std::env::var("FN64_DUMP_AUDIO_TASK_INDEX")
        .ok()
        .map(|raw| {
            raw.parse::<u64>().unwrap_or_else(|_| {
                panic!("FN64_DUMP_AUDIO_TASK_INDEX must be a positive integer, got {raw:?}")
            })
        })
        .unwrap_or(1);
    assert!(
        target_index != 0,
        "FN64_DUMP_AUDIO_TASK_INDEX is one-based; got 0"
    );
    AUDIO_TASK_DUMP.with(|dump| {
        let mut state = dump.get();
        state.seen = state.seen.saturating_add(1);
        if !state.dumped && state.seen == target_index {
            let (registered_rdram, registered_len) =
                with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
            assert_eq!(
                registered_rdram, rdram,
                "FN64_DUMP_AUDIO_TASK task pointer does not match registered process RDRAM"
            );
            let rdram_len = fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
            assert!(
                registered_len >= rdram_len,
                "FN64_DUMP_AUDIO_TASK registered process RDRAM is {registered_len:#x} bytes; \
                 physical capture requires {rdram_len:#x}"
            );
            assert!(
                task_offset
                    .checked_add(core::mem::size_of::<OsTaskHeader>())
                    .is_some_and(|end| end <= rdram_len),
                "FN64_DUMP_AUDIO_TASK task offset {task_offset:#x} is outside physical RDRAM"
            );
            let bytes = unsafe { std::slice::from_raw_parts(rdram, rdram_len) };
            let path = std::path::Path::new(&path);
            std::fs::write(path, bytes).unwrap_or_else(|error| {
                panic!("write private audio task capture {path:?}: {error}")
            });
            let meta = format!(
                "task_offset={task_offset}\nrdram_len={rdram_len}\ntask_index={}\n",
                state.seen
            );
            let meta_path = path.with_extension("meta");
            std::fs::write(&meta_path, meta).unwrap_or_else(|error| {
                panic!("write private audio task metadata {meta_path:?}: {error}")
            });
            eprintln!(
                "[fn64-abi] dumped audio task #{} input ({rdram_len} B, task_offset={task_offset}) to {path:?}",
                state.seen
            );
            state.dumped = true;
        }
        dump.set(state);
    });
}

/// `osSpTaskYielded(OSTask *task) -> OSYieldResult` observes the public RSP
/// task-yield handshake after the SP completion event. SIG1/
/// `SP_STATUS_YIELDED` means microcode saved resumable state in the task's
/// yield buffer. In that case libultra prepares the same task for restart by
/// setting `OS_TASK_YIELDED`, replacing its ucode-data pointer and size with
/// the yield-buffer fields, and returning `OS_TASK_YIELDED`. Otherwise the
/// task completed before honoring SIG0 and this returns zero without changing
/// the task.
///
/// This is an observation/preparation call, never a submission point. Running
/// a graphics backend or audio ucode here would execute work a second time
/// after `osSpTaskStartGo_recomp` already completed it.
///
/// # Safety
/// `ctx`/`rdram` must be valid per every other shim's contract in this file.
#[no_mangle]
pub unsafe extern "C" fn osSpTaskYielded_recomp(rdram: *mut u8, ctx: *mut RecompContext) {
    let ctx = unsafe { &mut *ctx };
    let task_addr = RdramAddr::from_gpr(ctx.r4);
    let o = task_addr.offset() as usize;
    if crate::pi::live_sp_status() & fn64_runtime::SP_STATUS_YIELDED == 0 {
        with_host(|host| {
            let retire = host
                .rsp_task_lineages
                .get(&task_addr.offset())
                .is_some_and(|lineage| lineage.phase == RspTaskLineagePhase::Running);
            if retire {
                host.rsp_task_lineages.remove(&task_addr.offset());
            }
        });
        ctx.r2 = 0;
        return;
    }

    let header = unsafe { read_os_task_header(rdram, o) };
    let lineage = with_host(|host| host.rsp_task_lineages.get(&task_addr.offset()).copied())
        .unwrap_or_else(|| {
            panic!(
                "osSpTaskYielded_recomp: yielded RSP task {:#010x} has no started task lineage",
                task_addr.offset()
            )
        });
    assert_eq!(
        lineage.phase,
        RspTaskLineagePhase::Running,
        "osSpTaskYielded_recomp: yielded RSP task {:#010x} cannot authorize a resume from phase {:?}",
        task_addr.offset(),
        lineage.phase,
    );
    let expected = if header.flags & fn64_runtime::OS_TASK_YIELDED != 0 {
        lineage.yielded_header()
    } else {
        lineage.original_header
    };
    assert_eq!(
        header,
        expected,
        "osSpTaskYielded_recomp: task {:#010x} changed after its loaded task admission",
        task_addr.offset()
    );
    unsafe {
        write_os_task_word(rdram, o, 0x04, header.flags | fn64_runtime::OS_TASK_YIELDED);
        write_os_task_word(rdram, o, 0x18, header.yield_data_ptr);
        write_os_task_word(rdram, o, 0x1c, header.yield_data_size);
    }
    with_host(|host| {
        let lineage = host
            .rsp_task_lineages
            .get_mut(&task_addr.offset())
            .expect("yielded task lineage was validated above");
        assert_eq!(
            lineage.phase,
            RspTaskLineagePhase::Running,
            "osSpTaskYielded_recomp: yielded RSP task {:#010x} changed lineage phase during its public rewrite",
            task_addr.offset()
        );
        lineage.phase = RspTaskLineagePhase::ResumeAuthorized;
    });
    ctx.r2 = u64::from(fn64_runtime::OS_TASK_YIELDED);
}
