use super::*;

/// Host policy for the graphics microcode phase of an admitted `M_GFXTASK`.
///
/// Both policies classify the admitted image first. Boot-overlay tasks execute
/// rspboot through the clean-room RSP interpreter and commit its complete
/// post-DMA machine state; direct IMEM images already enter at the fabric's PC
/// zero. `HleOptimized` then offers that task-entry state to the registered
/// graphics backend, retaining the transactional LLE fallback for an
/// unadmitted digest. `LleAccuracy` instead continues the loaded graphics
/// microcode through the existing interpreter unconditionally and forwards
/// only its raw DPC submissions to the backend.
/// The latter avoids making an HLE decoder's arithmetic part of an accuracy
/// claim; it does not select a different RDP implementation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsTaskExecutionPolicy {
    /// Prefer an exact-digest HLE implementation, falling back transactionally.
    #[default]
    HleOptimized,
    /// Execute every loaded graphics microcode instruction through LLE.
    LleAccuracy,
    /// Execute rspboot, then skip the graphics microcode phase explicitly and
    /// synthesize its DP FullSync completion so the game scheduler can advance.
    /// This exists only to isolate non-graphics subsystem diagnostics; release
    /// evidence rejects it.
    DiagnosticSkip,
}

/// Installed-ROM executor for admitted `M_AUDTASK` microcode.
///
/// `Translated` identifies the exact host artifact but is not an accuracy
/// claim: the callback ABI does not itself prove that artifact corresponds to
/// the task's complete live IMEM image. Fixed-cycle release evidence therefore
/// admits only `LleAccuracy`, which executes that image directly. The explicit
/// diagnostic mode preserves fast render-only probes without letting a skipped
/// synth masquerade as an unconfigured or executed task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioTaskExecutionPolicy {
    #[default]
    Unconfigured,
    Translated {
        artifact_sha256: [u8; 32],
    },
    LleAccuracy,
    DiagnosticSkip,
}

/// Exact registered renderer state frozen for fixed-cycle release evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderEnvironmentEvidenceSnapshot {
    pub backend: fn64_render::RenderBackendEvidence,
    pub execution_policy: GraphicsTaskExecutionPolicy,
}

impl RenderEnvironmentEvidenceSnapshot {
    /// Renderer-owned TV standard from the last successful backend creation.
    /// An unidentified compatibility backend has no release authority.
    pub const fn renderer_tv_type(&self) -> Option<fn64_runtime::TvType> {
        self.backend.tv_type()
    }
}

/// Publish the RSP core's guest-visible RDRAM effects after direct execution.
/// The bytes are already in the live allocation; only recompiler bookkeeping
/// and executable-page invalidation remain.
#[cfg(feature = "recomp-rs")]
pub(crate) fn commit_rsp_rdram_writes(source: RspWriterCommitSourceV1, written: &[(usize, usize)]) {
    if written.is_empty() {
        return;
    }
    record_rsp_writer_commits_v1(source, written);
    for &(start, end) in written {
        fn64_recomp_rs::notify_rsp_execution_or_hle_writeback(start as u32, (end - start) as u32);
    }
    // Raw SP_STATUS and host-call task starts execute inside
    // BlockProgram::dispatch, which still owns the program borrow. The write
    // observer makes the generated runner leave through ExecutableWrite as
    // soon as this MMIO/host call returns; run_block_program then installs the
    // completed generation after releasing that borrow and before dispatching
    // another guest instruction. Processing here would reborrow the live
    // program from inside its own runner.
}

#[cfg(not(feature = "recomp-rs"))]
pub(crate) fn commit_rsp_rdram_writes(_source: RspWriterCommitSourceV1, _written: &[(usize, usize)]) {}

/// Same-task authority required to replace one in-flight interpreter phase.
/// Construction is confined to the live owner check below; publication
/// consumes it only after rechecking that ownership under the final HostState
/// borrow.
#[cfg(test)]
pub(crate) struct VerifiedAudioCommitOwner {
    pub(crate) task_addr: RdramAddr,
    pub(crate) admission_generation: RspTaskAdmissionGeneration,
}

#[cfg(test)]
pub(crate) fn verified_audio_commit_owner(
    task_addr: RdramAddr,
    admission_generation: NonZeroU64,
) -> VerifiedAudioCommitOwner {
    let admission_generation = RspTaskAdmissionGeneration(admission_generation);
    let expected_owner = RspInterpreterOwner::task(task_addr.offset(), admission_generation);
    with_host(|host| {
        match host.rsp_interpreter_state {
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } if owner == expected_owner => {
                let lineage = host
                    .rsp_task_lineages
                    .get(&task_addr.offset())
                    .unwrap_or_else(|| {
                        panic!(
                            "verified audio task {:#010x} has no Running task lineage",
                            task_addr.offset()
                        )
                    });
                assert_eq!(
                    lineage.phase,
                    RspTaskLineagePhase::Running,
                    "verified audio task {:#010x} cannot commit lineage phase {:?}",
                    task_addr.offset(),
                    lineage.phase
                );
                assert_eq!(
                    lineage.admission_generation,
                    admission_generation,
                    "verified audio task {:#010x} admission generation {} does not own Running generation {}",
                    task_addr.offset(),
                    admission_generation.get(),
                    lineage.admission_generation.get()
                );
                VerifiedAudioCommitOwner {
                    task_addr,
                    admission_generation,
                }
            }
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } => panic!(
            "verified audio task {:#010x} generation {} cannot commit interpreter state owned by {}",
            task_addr.offset(),
            admission_generation.get(),
            owner.describe()
        ),
        _ => panic!(
            "verified audio task {:#010x} cannot commit without an in-flight interpreter owner",
            task_addr.offset()
        ),
    }
    })
}

#[cfg(test)]
pub(crate) fn verified_rsp_execution_state(
    machine: &fn64_audio::rsp::runtime::RspMachineState,
    pc_low12: u32,
) -> fn64_runtime::RspExecutionState {
    rsp_execution_state_from_architectural(machine.architectural_state(), pc_low12)
}

pub(crate) fn rsp_execution_state_from_architectural(
    state: &fn64_audio::rsp::runtime::RspArchitecturalState,
    pc_low12: u32,
) -> fn64_runtime::RspExecutionState {
    fn64_runtime::RspExecutionState {
        pc: pc_low12,
        sp_status: state.sp_status(),
        sp_semaphore: state.sp_semaphore(),
        sp_dma_mem_addr: fn64_runtime::RspMemAddr::from_register(state.dma_mem_address()),
        sp_dma_dram_addr: RdramAddr::from_offset(state.dma_dram_address() & 0x00ff_ffff),
        sp_dma_read_length: state.dma_read_length(),
        sp_dma_write_length: state.dma_write_length(),
        dpc_start: state.dp_start(),
        dpc_end: state.dp_end(),
        dpc_current: state.dp_current(),
        dpc_status: state.dp_status(),
        dpc_clock: state.dp_clock(),
        dpc_busy: state.dp_busy(),
        dpc_pipe_busy: state.dp_pipe_busy(),
        dpc_tmem_busy: state.dp_tmem_busy(),
    }
}

pub(crate) fn begin_rsp_interpreter_phase(
    owner: RspInterpreterOwner,
    machine: &mut fn64_audio::rsp::runtime::RspMachine<'_>,
) {
    let prior = with_host(|host| {
        match &host.rsp_interpreter_state {
            RspInterpreterStateEvidenceSnapshot::InFlight { owner: prior } => panic!(
                "RSP {} cannot start: {} left a pending interpreter continuation",
                owner.describe(),
                prior.describe()
            ),
            RspInterpreterStateEvidenceSnapshot::Reset => None,
            RspInterpreterStateEvidenceSnapshot::Exact(state)
            | RspInterpreterStateEvidenceSnapshot::HleCompatibility(state) => {
                Some(state.clone())
            }
            RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable { owner: prior } => {
                panic!(
                    "RSP {} cannot start after direct-IMEM HLE {}: terminal scalar/VU state is unavailable",
                    owner.describe(),
                    prior.describe()
                )
            }
        }
        .inspect(|state| {
            assert_eq!(
                state.resume_address(),
                0,
                "RSP {} cannot inherit pending overlay resume address {:#06x}",
                owner.describe(),
                state.resume_address()
            );
            assert!(
                !state.resume_delay(),
                "RSP {} cannot inherit a pending branch-delay continuation",
                owner.describe()
            );
            assert!(
                state.dp_submissions().is_empty(),
                "RSP {} cannot inherit {} uncommitted DPC submission(s)",
                owner.describe(),
                state.dp_submissions().len()
            );
        });
        let prior = std::mem::replace(
            &mut host.rsp_interpreter_state,
            RspInterpreterStateEvidenceSnapshot::InFlight { owner },
        );
        match prior {
            RspInterpreterStateEvidenceSnapshot::Reset => None,
            RspInterpreterStateEvidenceSnapshot::Exact(state)
            | RspInterpreterStateEvidenceSnapshot::HleCompatibility(state) => Some(state),
            RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            | RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable { .. } => {
                unreachable!("invalid prior interpreter state rejected before acquisition")
            }
        }
    });

    if let Some(state) = prior {
        machine.restore_architectural_state(state);
    }
    // CPU MMIO and osSpTaskLoad execute outside the interpreter between task
    // snapshots. The fabric is authoritative for every duplicated SP/DPC
    // latch; scalar, VU, and continuation state remain owned above.
    let fabric = with_host(|host| host.device_fabric.rsp_execution_state());
    machine.overlay_device_execution_state(fabric);
    machine.set_sp_status_raw(
        machine.sp_status() & !(fn64_runtime::SP_STATUS_HALT | fn64_runtime::SP_STATUS_BROKE),
    );
}

/// Sole live-runtime authority paired with one speculative whole-audio-task
/// capture. Deliberately non-cloneable: publication will eventually consume
/// this value after rechecking the same task admission generation.
#[allow(dead_code)]
pub(crate) struct AudioWholeTaskOwner {
    pub(crate) task_addr: RdramAddr,
    pub(crate) admission_generation: RspTaskAdmissionGeneration,
}

/// Owned pre-rspboot input paired with the live interpreter owner acquired
/// before any physical-device state is cloned.
#[allow(dead_code)]
pub(crate) struct CapturedAudioWholeTask {
    pub(crate) owner: AudioWholeTaskOwner,
    pub(crate) input: fn64_audio::hle_rspboot::AudioRspbootInput,
}

/// Capture one boot-overlay audio task without executing rspboot or mutating
/// any live device owner other than the persistent interpreter state token.
///
/// Acquiring `InFlight` first closes the same-address reuse interleaving: any
/// later failure intentionally retains that owner, so a second task cannot
/// hide an incomplete speculative phase behind a fresh interpreter snapshot.
#[allow(dead_code)]
pub(crate) unsafe fn capture_audio_whole_task_input(
    rdram: *mut u8,
    task_addr: RdramAddr,
    loaded_header: OsTaskHeader,
) -> CapturedAudioWholeTask {
    let mut scratch_rdram = [];
    let mut machine = fn64_audio::rsp::runtime::RspMachine::new(&mut scratch_rdram);
    begin_rsp_interpreter_phase(task_interpreter_owner(task_addr), &mut machine);
    let initial_machine_state = machine.snapshot_state();
    drop(machine);

    let (owner, registered_rdram, allocation_len, rsp_memory, initial_pc_low12) = with_host(
        |host| {
            let (task_offset, admission_generation) = match host.rsp_interpreter_state {
                RspInterpreterStateEvidenceSnapshot::InFlight {
                    owner:
                        RspInterpreterOwner::Task {
                            offset,
                            admission_generation,
                        },
                } if offset == task_addr.offset() => (offset, admission_generation),
                _ => unreachable!("begin_rsp_interpreter_phase installed this task owner"),
            };
            let lineage = host
                .rsp_task_lineages
                .get(&task_offset)
                .expect("whole-audio capture lost its Running task lineage after acquisition");
            assert_eq!(
                (lineage.admission_generation, lineage.phase),
                (admission_generation, RspTaskLineagePhase::Running),
                "whole-audio capture task {task_offset:#010x} lost its exact Running admission after acquisition"
            );
            (
                AudioWholeTaskOwner {
                    task_addr,
                    admission_generation,
                },
                host.runtime_rdram,
                host.runtime_rdram_len,
                host.device_fabric.rsp_memory().snapshot(),
                host.device_fabric.sp_pc(),
            )
        },
    );

    assert!(
        !rdram.is_null()
            && rdram == registered_rdram
            && allocation_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "whole-audio capture must use the registered complete physical RDRAM allocation"
    );
    // SAFETY: process registration owns this allocation for the runtime and
    // the length check covers exactly the physical device. `InFlight` is only
    // protocol ownership of this RSP task; the physical copy is atomic because
    // this synchronous shim runs with one runnable guest and invokes no host
    // callback between registration validation and the copy.
    let physical_rdram = unsafe {
        std::slice::from_raw_parts(rdram, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE).to_vec()
    };
    let input = fn64_audio::hle_rspboot::AudioRspbootInput::new(
        task_addr,
        loaded_header,
        physical_rdram,
        rsp_memory,
        initial_pc_low12,
        initial_machine_state,
    )
    .unwrap_or_else(|error| panic!("whole-audio preboot capture rejected: {error:?}"));

    CapturedAudioWholeTask { owner, input }
}

pub(crate) fn continue_rsp_interpreter_phase(
    expected_owner: RspInterpreterOwner,
    machine: &mut fn64_audio::rsp::runtime::RspMachine<'_>,
    state: fn64_audio::rsp::runtime::RspMachineState,
) {
    let architectural = state.into_architectural_state();
    with_host(|host| match &host.rsp_interpreter_state {
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } if *owner == expected_owner => {}
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } => panic!(
            "RSP {} cannot continue interpreter state owned by {}",
            expected_owner.describe(),
            owner.describe()
        ),
        RspInterpreterStateEvidenceSnapshot::Reset
        | RspInterpreterStateEvidenceSnapshot::Exact(_)
        | RspInterpreterStateEvidenceSnapshot::HleCompatibility(_)
        | RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable { .. } => panic!(
            "RSP {} has a same-task machine snapshot without an in-flight rspboot owner",
            expected_owner.describe()
        ),
    });
    // rspboot's instruction count contributes to task latency, but is not a
    // hardware register and must not seed the ucode phase's diagnostics.
    machine.restore_architectural_state(architectural);
}

pub(crate) fn commit_rsp_interpreter_phase(
    expected_owner: RspInterpreterOwner,
    state: fn64_audio::rsp::runtime::RspArchitecturalState,
) {
    assert_eq!(
        state.resume_address(),
        0,
        "RSP {} reached a commit boundary with pending overlay resume address {:#06x}",
        expected_owner.describe(),
        state.resume_address()
    );
    assert!(
        !state.resume_delay(),
        "RSP {} reached a commit boundary in a branch-delay continuation",
        expected_owner.describe()
    );
    assert!(
        state.dp_submissions().is_empty(),
        "RSP {} reached a commit boundary with {} uncommitted DPC submission(s)",
        expected_owner.describe(),
        state.dp_submissions().len()
    );
    with_host(|host| match host.rsp_interpreter_state {
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } if owner == expected_owner => {
            host.rsp_interpreter_state = RspInterpreterStateEvidenceSnapshot::Exact(state);
        }
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } => panic!(
            "RSP {} cannot commit interpreter state owned by {}",
            expected_owner.describe(),
            owner.describe()
        ),
        _ => panic!(
            "RSP {} cannot commit without an in-flight interpreter owner",
            expected_owner.describe()
        ),
    });
}

pub(crate) fn commit_rsp_hle_compatibility(
    task_addr: RdramAddr,
    state: Option<fn64_audio::rsp::runtime::RspMachineState>,
) {
    let admission_generation = running_task_admission_generation(task_addr);
    let expected_owner = RspInterpreterOwner::task(task_addr.offset(), admission_generation);
    let Some(state) = state else {
        with_host(|host| {
            match host.rsp_interpreter_state {
            RspInterpreterStateEvidenceSnapshot::InFlight { owner } if owner == expected_owner =>
            {
                host.rsp_interpreter_state =
                    RspInterpreterStateEvidenceSnapshot::HleCompatibilityUnavailable {
                        owner: expected_owner,
                    };
            }
            RspInterpreterStateEvidenceSnapshot::InFlight { owner } => panic!(
                "direct-IMEM HLE task {:#010x} generation {} cannot replace in-flight interpreter owner {}",
                task_addr.offset(),
                admission_generation.get(),
                owner.describe()
            ),
            _ => panic!(
                "direct-IMEM HLE task {:#010x} cannot commit compatibility state without its in-flight owner",
                task_addr.offset()
            ),
        }
        });
        return;
    };
    let state = state.into_hle_compatibility_architectural_state();
    assert!(state.dp_submissions().is_empty());
    with_host(|host| {
        match host.rsp_interpreter_state {
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } if owner == expected_owner =>
        {
            host.rsp_interpreter_state =
                RspInterpreterStateEvidenceSnapshot::HleCompatibility(state);
        }
        RspInterpreterStateEvidenceSnapshot::InFlight { owner } => panic!(
            "RSP HLE task {:#010x} generation {} cannot commit compatibility state owned by {}",
            task_addr.offset(),
            admission_generation.get(),
            owner.describe()
        ),
        _ => panic!(
            "RSP HLE task {:#010x} cannot commit compatibility state without an in-flight rspboot owner",
            task_addr.offset()
        ),
    }
    });
}

#[cfg(test)]
pub(crate) fn apply_verified_audio_rdram_patches(
    storage: &mut [u8],
    patches: &fn64_audio::hle_outcome::CanonicalRdramPatches,
) -> Vec<(usize, usize)> {
    assert_eq!(
        storage.len(),
        fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "verified audio commit requires the exact 8 MiB physical RDRAM device"
    );
    let mut writes = Vec::with_capacity(patches.as_slice().len());
    let mut view = fn64_runtime::RdramViewMut::from_storage(storage);
    for patch in patches.as_slice() {
        let range = patch.range();
        let start = range.start() as usize;
        let end = range.end() as usize;
        assert!(
            end <= view.len(),
            "verified audio RDRAM patch [{start:#x}, {end:#x}) exceeds the physical device"
        );
        view.write_logical_bytes(RdramAddr::from_offset(range.start()), patch.bytes());
        writes.push((start, end));
    }
    writes
}

#[cfg(test)]
pub(crate) fn validate_verified_audio_rdram_patches(
    storage_len: usize,
    patches: &fn64_audio::hle_outcome::CanonicalRdramPatches,
) {
    assert_eq!(
        storage_len,
        fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "verified audio commit requires the exact 8 MiB physical RDRAM device"
    );
    for patch in patches.as_slice() {
        let range = patch.range();
        assert!(
            range.end() as usize <= storage_len,
            "verified audio RDRAM patch [{:#x}, {:#x}) exceeds the physical device",
            range.start(),
            range.end()
        );
    }
}

#[cfg(test)]
pub(crate) fn deferred_audio_dpc_batch(
    submissions: Vec<fn64_audio::hle_outcome::DeferredDpcSubmission>,
) -> Option<fn64_render::RawDpcBatch> {
    if submissions.is_empty() {
        return None;
    }
    let submissions = submissions
        .into_iter()
        .map(|submission| match submission.source() {
            fn64_audio::hle_outcome::DpcSubmissionSource::Rdram => {
                fn64_render::OwnedRawDpcSubmission::from_rdram_words(
                    submission.start(),
                    submission.end(),
                    submission.command_words(),
                )
            }
            fn64_audio::hle_outcome::DpcSubmissionSource::Dmem => {
                fn64_render::OwnedRawDpcSubmission::from_xbus_payload(
                    submission.start(),
                    submission.end(),
                    submission
                        .xbus_payload()
                        .expect("verified XBUS DPC submission lost its captured DMEM payload")
                        .to_vec(),
                )
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("verified audio DPC conversion rejected: {error:?}"));
    Some(
        fn64_render::RawDpcBatch::new(submissions)
            .unwrap_or_else(|error| panic!("verified audio DPC batch rejected: {error:?}")),
    )
}

#[cfg(test)]
pub(crate) fn canonical_changed_rdram_ranges(before: &[u8], after: &[u8]) -> Vec<(usize, usize)> {
    assert_eq!(before.len(), after.len());
    let before = fn64_runtime::RdramView::from_storage(before);
    let after = fn64_runtime::RdramView::from_storage(after);
    let mut ranges = Vec::new();
    let mut start = None;
    for offset in 0..before.len() {
        let address = RdramAddr::from_offset(offset as u32);
        let changed = before.read_u8(address) != after.read_u8(address);
        match (start, changed) {
            (None, true) => start = Some(offset),
            (Some(range_start), false) => {
                ranges.push((range_start, offset));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(range_start) = start {
        ranges.push((range_start, before.len()));
    }
    ranges
}

#[cfg(test)]
pub(crate) fn merge_canonical_rdram_write_ranges(
    mut ranges: Vec<(usize, usize)>,
    additional: Vec<(usize, usize)>,
) -> Vec<(usize, usize)> {
    ranges.extend(additional);
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, prior_end)) if start <= *prior_end => {
                *prior_end = (*prior_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) unsafe fn commit_verified_audio_effects(
    rdram: *mut u8,
    task_addr: RdramAddr,
    task_admission_generation: NonZeroU64,
    rdram_patches: fn64_audio::hle_outcome::CanonicalRdramPatches,
    rsp_memory: fn64_runtime::rsp::RspMemorySnapshot,
    machine_state: fn64_audio::rsp::runtime::RspMachineState,
    pc_low12: u32,
    dpc_submissions: Vec<fn64_audio::hle_outcome::DeferredDpcSubmission>,
) -> fn64_render::DpFullSyncStatus {
    let owner = verified_audio_commit_owner(task_addr, task_admission_generation);
    let (registered, allocation_len) =
        with_host(|host| (host.runtime_rdram, host.runtime_rdram_len));
    assert!(
        !rdram.is_null()
            && rdram == registered
            && allocation_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "verified audio commit must target the registered physical RDRAM allocation"
    );
    validate_verified_audio_rdram_patches(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE, &rdram_patches);

    let execution_state = verified_rsp_execution_state(&machine_state, pc_low12);
    if deferred_audio_dpc_batch(dpc_submissions).is_some() {
        let reason = "verified audio DPC publication requires exact per-CMD_END memory, device-timing, interrupt, and FullSync-order authority; the staged-RDRAM renderer seam is diagnostic-only";
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Render,
            "render.raw-dpc-batch.non-certifying",
            reason,
            Some(fn64_runtime::Cycles::new(crate::sim_time())),
            fn64_runtime::UnsupportedDisposition::LoudTrap,
        );
        panic!("{reason}");
    }

    let live =
        unsafe { std::slice::from_raw_parts_mut(rdram, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE) };
    let mut shadow = live.to_vec();
    let verified_writes = apply_verified_audio_rdram_patches(&mut shadow, &rdram_patches);
    let architectural_state = machine_state.into_architectural_state();

    let writes = merge_canonical_rdram_write_ranges(
        verified_writes,
        canonical_changed_rdram_ranges(live, &shadow),
    );
    #[cfg(feature = "recomp-rs")]
    if let Err(reason) = crate::recompiled::preflight_non_executable_host_writes(&writes) {
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Recompiler,
            "recompiler.verified-audio.executable-write",
            &reason,
            Some(fn64_runtime::Cycles::new(crate::sim_time())),
            fn64_runtime::UnsupportedDisposition::LoudTrap,
        );
        panic!("verified audio publication rejected: {reason}");
    }

    #[cfg(feature = "recomp-rs")]
    let catalog_writer = crate::recompiled::begin_catalog_nested_writer(
        live,
        "verified audio RSP-state publication",
    );
    with_host(|host| {
        host.device_fabric
            .preflight_complete_rsp_execution_state(&execution_state)
            .unwrap_or_else(|error| panic!("verified audio RSP-state preflight rejected: {error}"));
        // A later load may reuse this OSTask address with generation N+1
        // after generation N's speculative verification. Rechecking the exact
        // generation and Running lineage in this exclusive HostState borrow
        // prevents stale generation N from publishing any effect.
        let expected_owner =
            RspInterpreterOwner::task(owner.task_addr.offset(), owner.admission_generation);
        match host.rsp_interpreter_state {
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: interpreter_owner,
            } if interpreter_owner == expected_owner => {}
            RspInterpreterStateEvidenceSnapshot::InFlight {
                owner: interpreter_owner,
            } => panic!(
                "verified audio task {:#010x} generation {} lost ownership to {} before publication",
                owner.task_addr.offset(),
                owner.admission_generation.get(),
                interpreter_owner.describe()
            ),
            _ => panic!(
                "verified audio task {:#010x} lost its in-flight owner before publication",
                owner.task_addr.offset()
            ),
        }
        let lineage = host
            .rsp_task_lineages
            .get(&owner.task_addr.offset())
            .unwrap_or_else(|| {
                panic!(
                    "verified audio task {:#010x} lost its Running lineage before publication",
                    owner.task_addr.offset()
                )
            });
        assert_eq!(
            (lineage.admission_generation, lineage.phase),
            (owner.admission_generation, RspTaskLineagePhase::Running),
            "verified audio task {:#010x} lost admission generation {} Running authority before publication",
            owner.task_addr.offset(),
            owner.admission_generation.get()
        );
        host.device_fabric
            .commit_complete_rsp_execution_state(execution_state)
            .expect("exclusive verified-audio device preflight became invalid");
        host.device_fabric.rsp_memory_mut().restore(rsp_memory);
        host.rsp_interpreter_state =
            RspInterpreterStateEvidenceSnapshot::Exact(architectural_state);
        live.copy_from_slice(&shadow);
    });
    #[cfg(feature = "recomp-rs")]
    catalog_writer.commit(live);

    fn64_render::DpFullSyncStatus::NotReached
}

pub(crate) fn rsp_visible_rdram_len(allocation_len: usize) -> usize {
    allocation_len.min(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE)
}

#[cfg(feature = "recomp-rs")]
pub(crate) fn track_rdp_renderer_mutation<R>(rdram: &mut [u8], operation: impl FnOnce(&mut [u8]) -> R) -> R {
    super::recompiled::track_rdp_renderer_mutation(rdram, operation)
}

#[cfg(feature = "recomp-rs")]
pub(crate) fn record_rdp_renderer_publication_v1() {
    super::recompiled::record_rdp_renderer_publication_v1();
}

#[cfg(feature = "recomp-rs")]
pub(crate) fn record_rdp_renderer_rejection_v1() {
    super::recompiled::record_rdp_renderer_rejection_v1();
}

#[cfg(feature = "recomp-rs")]
pub(crate) fn track_rsp_execution_or_hle_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> (R, Vec<u64>) {
    super::recompiled::track_rsp_execution_or_hle_mutation(rdram, operation)
}

#[cfg(not(feature = "recomp-rs"))]
pub(crate) fn track_rdp_renderer_mutation<R>(rdram: &mut [u8], operation: impl FnOnce(&mut [u8]) -> R) -> R {
    operation(rdram)
}

#[cfg(not(feature = "recomp-rs"))]
pub(crate) fn record_rdp_renderer_publication_v1() {}

#[cfg(not(feature = "recomp-rs"))]
pub(crate) fn record_rdp_renderer_rejection_v1() {}

#[cfg(not(feature = "recomp-rs"))]
pub(crate) fn track_rsp_execution_or_hle_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> (R, Vec<u64>) {
    (operation(rdram), Vec::new())
}

/// Expose the renderer and RDP to physical RDRAM, never the host-only MMIO
/// backing appended to the generated-code allocation. Retail commands can
/// address the final byte of the 8 MiB device, so a short registration is a
/// caller error rather than a reason to truncate the hardware-visible span.
pub(crate) unsafe fn renderer_rdram_slice<'a>(rdram: *mut u8) -> &'a mut [u8] {
    let allocation_len = RDRAM_LEN.with(Cell::get);
    assert!(
        allocation_len >= fn64_runtime::rdram::DEFAULT_RDRAM_SIZE,
        "renderer RDRAM allocation length {allocation_len:#x} does not cover the required 8 MiB physical device"
    );
    unsafe { std::slice::from_raw_parts_mut(rdram, fn64_runtime::rdram::DEFAULT_RDRAM_SIZE) }
}

pub(crate) fn rsp_dma_storage_layout(
    allocation_len: usize,
    static_aliases: Vec<std::ops::Range<u32>>,
) -> (Vec<std::ops::Range<usize>>, usize) {
    let physical_len = rsp_visible_rdram_len(allocation_len);
    let mut ranges: Vec<_> = std::iter::once(0..physical_len).collect();
    let mut snapshot_len = physical_len;
    for alias in static_aliases {
        let start = alias.start as usize;
        let end = alias.end as usize;
        assert!(
            start < end && end <= allocation_len,
            "loaded static-overlay RSP alias [{start:#x}, {end:#x}) is invalid for host RDRAM \
             allocation length {allocation_len:#x}"
        );
        ranges.push(start..end);
        snapshot_len = snapshot_len.max(end);
    }
    (ranges, snapshot_len)
}

pub(crate) unsafe fn trace_rsp_rdram_words(rdram: *const u8, rdram_len: usize) {
    let Some(spec) = std::env::var_os("RSP_TRACE_RDRAM_WORDS") else {
        return;
    };
    let spec = spec
        .to_str()
        .unwrap_or_else(|| panic!("RSP_TRACE_RDRAM_WORDS must be UTF-8"));
    let (offset, count) = spec
        .split_once(':')
        .unwrap_or_else(|| panic!("RSP_TRACE_RDRAM_WORDS must be OFFSET:COUNT, got {spec:?}"));
    let offset = usize::from_str_radix(offset.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("RSP_TRACE_RDRAM_WORDS offset must be hexadecimal"));
    let count = count
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("RSP_TRACE_RDRAM_WORDS count must be decimal"));
    let byte_len = count
        .checked_mul(4)
        .expect("RSP_TRACE_RDRAM_WORDS byte length overflow");
    let end = offset
        .checked_add(byte_len)
        .expect("RSP_TRACE_RDRAM_WORDS range overflow");
    assert!(
        end <= rdram_len,
        "RSP_TRACE_RDRAM_WORDS range exceeds host allocation"
    );
    let bytes = unsafe { std::slice::from_raw_parts(rdram.add(offset), byte_len) };
    let words: Vec<_> = bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_ne_bytes(bytes.try_into().expect("four RDRAM bytes")))
        .collect();
    eprintln!("[fn64-abi/rsp] RDRAM {offset:#x} words={words:08x?}");
}

pub(crate) fn trace_rsp_dmem_words(dmem: &[u8], overlay: u64, pc: u32) {
    let Some(spec) = std::env::var_os("RSP_TRACE_DMEM_WORDS") else {
        return;
    };
    let spec = spec
        .to_str()
        .unwrap_or_else(|| panic!("RSP_TRACE_DMEM_WORDS must be UTF-8"));
    let (offset, count) = spec
        .split_once(':')
        .unwrap_or_else(|| panic!("RSP_TRACE_DMEM_WORDS must be OFFSET:COUNT, got {spec:?}"));
    let offset = usize::from_str_radix(offset.trim_start_matches("0x"), 16)
        .unwrap_or_else(|_| panic!("RSP_TRACE_DMEM_WORDS offset must be hexadecimal"));
    let count = count
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("RSP_TRACE_DMEM_WORDS count must be decimal"));
    let byte_len = count
        .checked_mul(4)
        .expect("RSP_TRACE_DMEM_WORDS byte length overflow");
    let end = offset
        .checked_add(byte_len)
        .expect("RSP_TRACE_DMEM_WORDS range overflow");
    assert!(end <= dmem.len(), "RSP_TRACE_DMEM_WORDS range exceeds DMEM");
    let words: Vec<_> = dmem[offset..end]
        .chunks_exact(4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four DMEM bytes")))
        .collect();
    eprintln!("[fn64-abi/rsp] overlay={overlay} pc={pc:#06x} DMEM {offset:#x} words={words:08x?}");
}

pub(crate) fn lle_debug_task_data(rdram: &[u8], source_addr: u32, source_size: u32) -> Option<Vec<u8>> {
    let addr = RdramAddr::from_offset(source_addr & 0x00ff_ffff);
    let requested_len = (source_size as usize).clamp(0x40, 0x20000);
    let start = addr.offset() as usize;
    let end = start
        .checked_add(requested_len)
        .expect("LLE debug task-data range overflow")
        .min(rdram.len());
    if start >= end {
        return None;
    }

    let mut logical = vec![0; end - start];
    fn64_runtime::RdramView::from_storage(rdram).copy_logical_bytes(addr, &mut logical);
    Some(logical)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dump_lle_debug_state(
    dir: &std::path::Path,
    initial_dmem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    initial_imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    initial_pc: u32,
    imem: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    machine: &fn64_audio::rsp::runtime::RspMachine<'_>,
    abort_pc: u32,
    total_steps: u64,
    overlays: u64,
    pc_ring: &std::collections::VecDeque<u32>,
) {
    use std::fmt::Write as _;
    if let Err(error) = std::fs::create_dir_all(dir) {
        eprintln!("[fn64-abi] LLE debug dump: cannot create {dir:?}: {error}");
        return;
    }
    let write = |name: &str, bytes: &[u8]| {
        if let Err(error) = std::fs::write(dir.join(name), bytes) {
            eprintln!("[fn64-abi] LLE debug dump: cannot write {name}: {error}");
        }
    };
    write("initial_dmem.bin", initial_dmem);
    write("initial_imem.bin", initial_imem);
    write("final_dmem.bin", &machine.dmem_logical());
    write("final_imem.bin", imem);

    let mut state = String::new();
    let _ = writeln!(state, "abort_pc {abort_pc:#06x}");
    let _ = writeln!(state, "initial_pc {initial_pc:#06x}");
    let _ = writeln!(state, "total_steps {total_steps}");
    let _ = writeln!(state, "overlays {overlays}");
    let _ = writeln!(state, "sp_status {:#010x}", machine.sp_status());
    let _ = writeln!(state, "sp_semaphore {}", machine.sp_semaphore_latch());
    let _ = writeln!(
        state,
        "dma_mem_address {:#010x}",
        machine.ctx.dma_mem_address
    );
    let _ = writeln!(
        state,
        "dma_dram_address {:#010x}",
        machine.ctx.dma_dram_address
    );
    for reg in 0..32u8 {
        let _ = writeln!(state, "r{reg} {:#010x}", machine.reg(reg));
    }
    write("state.txt", state.as_bytes());

    let mut ring = String::new();
    for pc in pc_ring {
        let _ = writeln!(ring, "{pc:#06x}");
    }
    write("pc_ring.txt", ring.as_bytes());

    let field = |image: &[u8; fn64_runtime::RSP_MEMORY_BANK_SIZE], offset: usize| {
        u32::from_be_bytes(
            image[0xfc0 + offset..0xfc0 + offset + 4]
                .try_into()
                .expect("four OSTask bytes"),
        )
    };
    let mut header = String::new();
    for (name, offset) in [
        ("type", 0x00),
        ("flags", 0x04),
        ("ucode_boot", 0x08),
        ("ucode_boot_size", 0x0c),
        ("ucode", 0x10),
        ("ucode_size", 0x14),
        ("ucode_data", 0x18),
        ("ucode_data_size", 0x1c),
        ("dram_stack", 0x20),
        ("dram_stack_size", 0x24),
        ("output_buff", 0x28),
        ("output_buff_size", 0x2c),
        ("data_ptr", 0x30),
        ("data_size", 0x34),
        ("yield_data_ptr", 0x38),
        ("yield_data_size", 0x3c),
    ] {
        let _ = writeln!(header, "{name} {:#010x}", field(initial_dmem, offset));
    }
    write("task_header.txt", header.as_bytes());

    if let Some(logical) = lle_debug_task_data(
        machine.rdram,
        field(initial_dmem, 0x30),
        field(initial_dmem, 0x34),
    ) {
        write("task_data_logical.bin", &logical);
    }
    let raw_len = machine
        .rdram
        .len()
        .min(fn64_runtime::rdram::DEFAULT_RDRAM_SIZE);
    write("rdram_raw.bin", &machine.rdram[..raw_len]);
}
