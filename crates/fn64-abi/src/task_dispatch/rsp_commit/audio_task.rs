use super::*;

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
    let Some(path) = crate::diag_env::diag_env("FN64_DUMP_AUDIO_TASK") else {
        return;
    };
    let target_index = crate::diag_env::diag_env("FN64_DUMP_AUDIO_TASK_INDEX")
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
