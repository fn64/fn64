use super::*;

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
        backend: &mut dyn RawDpcBackend,
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

/// Scan an admitted raw-DPC range and park it if it ends inside a command.
///
/// Returns `Some(true)` when the submission was parked (the caller must return
/// without dispatching) and `None` when the range is whole and dispatch should
/// proceed. A malformed opcode still panics, as it always has.
unsafe fn park_incomplete_raw_dpc(
    rdram: *mut u8,
    submission: fn64_runtime::DpcSubmission,
    retained_tail: Vec<u32>,
) -> Option<bool> {
    let start = submission.start;
    let end = submission.end;
    let retained_bytes = u32::try_from(retained_tail.len() * size_of::<u32>())
        .expect("retained raw-DPC tail exceeds u32");
    let newly_exposed_start = start
        .checked_add(retained_bytes)
        .expect("retained raw-DPC tail address overflow");
    assert!(
        newly_exposed_start <= end,
        "retained raw-DPC tail extends past admitted END"
    );

    let mut words = retained_tail;
    match submission.source {
        fn64_runtime::DpcSubmissionSource::Rdram => {
            let real = unsafe { renderer_rdram_slice(rdram) };
            let from = newly_exposed_start as usize;
            let to = end as usize;
            assert!(
                from <= to && from.is_multiple_of(8) && to.is_multiple_of(8),
                "DRAM DPC extension [{newly_exposed_start:#010x}, {end:#010x}) must be ordered \
                 and 8-byte aligned"
            );
            assert!(
                to <= real.len(),
                "DRAM DPC range end {end:#010x} exceeds registered RDRAM length {:#x}",
                real.len()
            );
            words.extend(
                real[from..to]
                    .chunks_exact(4)
                    .map(|word| u32::from_ne_bytes(word.try_into().expect("four RDRAM bytes"))),
            );
        }
        fn64_runtime::DpcSubmissionSource::Dmem => {
            let exposed = with_host(|host| {
                let dmem = host
                    .device_fabric
                    .rsp_memory()
                    .bank(fn64_runtime::RspMemoryBank::Dmem);
                dmem[newly_exposed_start as usize..end as usize]
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().expect("four DMEM bytes")))
                    .collect::<Vec<_>>()
            });
            words.extend(exposed);
        }
    }

    match fn64_render::count_raw_rdp_full_sync_sites(&words)
        .unwrap_or_else(|error| panic!("dispatch_raw_rdp: {error}"))
    {
        fn64_render::RawRdpScan::Complete(_) => None,
        fn64_render::RawRdpScan::Incomplete {
            command_start,
            bytes_required,
            ..
        } => {
            let offset =
                u32::try_from(command_start).expect("stalled raw-DPC command offset exceeds u32");
            let command_start = start
                .checked_add(offset)
                .expect("stalled raw-DPC command address overflow");
            let retained_words =
                words[command_start.saturating_sub(start) as usize / size_of::<u32>()..].to_vec();
            with_host(|host| {
                host.device_fabric.park_dpc_submission(
                    submission.token,
                    command_start,
                    end,
                    bytes_required,
                    retained_words,
                )
            })
            .unwrap_or_else(|error| panic!("parking incomplete raw DPC transaction: {error}"));
            Some(true)
        }
    }
}
/// Submit one fabric-owned CPU DPC transaction to the registered renderer.
/// DRAM reads the registered physical device; XBUS snapshots persistent DMEM
/// at the accepted END boundary. Renderer acceptance commits CURRENT before
/// FullSync can schedule DP completion; rejection cancels the token and
/// records no observation.
pub(crate) unsafe fn dispatch_dpc_submission(
    rdram: *mut u8,
    submission: fn64_runtime::DpcSubmission,
    retained_tail: Vec<u32>,
) {
    let start = submission.start;
    let end = submission.end;

    // **Stall before routing.** The DPC accepts END extensions in 8-byte
    // increments, so a multiword command straddles several END writes.
    // Hardware parks CURRENT at that command's start rather than decoding a
    // truncated stream; this is the raw CPU MMIO counterpart of the
    // coalescing `coalesce_dp_submissions` performs for RSP streams.
    //
    // The retained tail arrives from the fabric rather than being reread:
    // XBUS DMEM can change between END writes, so the bytes admitted with the
    // first END are the bytes that must be decoded.
    if let Some(parked) = unsafe { park_incomplete_raw_dpc(rdram, submission, retained_tail) } {
        debug_assert!(parked, "park_incomplete_raw_dpc reports whether it parked");
        return;
    }
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
        let (transaction, ack) = LiveDpcTransaction::new(submission);
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
            ack,
            None,
        )
        .expect("session_registered was already checked true under the same borrow");
        if full_sync == fn64_render::DpFullSyncStatus::Reached {
            crate::pi::start_live_dp_full_sync().unwrap_or_else(|error| {
                panic!("dispatch_raw_rdp: DP FullSync completion: {error}")
            });
        }
        return;
    }

    let (mut transaction, ack) = LiveDpcTransaction::new(submission);
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
                let full_sync = require_matching_raw_dpc_completion(
                    require_complete_raw_dpc_scan(inspected, "dispatch_raw_rdp"),
                    rendered,
                    "dispatch_raw_rdp",
                );
                transaction.validate_atomic_completion(ack);
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
                dispatch_captured_raw_rdp(
                    rdram,
                    &words,
                    start,
                    end,
                    true,
                    true,
                    &mut transaction,
                    ack,
                )
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
        unsafe { dispatch_dpc_submission(rdram, submission, Vec::new()) };
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
    let (mut transaction, ack) = LiveDpcTransaction::new(submission);
    let (full_sync, observation) = unsafe {
        dispatch_captured_raw_rdp(rdram, &words, start, end, true, true, &mut transaction, ack)
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
    ack: DpcAckGuard,
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
    let full_sync = require_matching_raw_dpc_completion(
        require_complete_raw_dpc_scan(inspected, "dispatch_captured_raw_rdp"),
        rendered,
        "dispatch_captured_raw_rdp",
    );
    transaction.validate_atomic_completion(ack);
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
