use super::*;

pub(super) fn validate_pi_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, u64, u64, u64, u64, [u8; 32]), PiWriterRuntimeStateErrorV1> {
    #[derive(Clone, Copy)]
    struct ActivePi {
        request: fn64_runtime::PiDmaRequest,
        phase: u8,
    }

    let mut active: Option<ActivePi> = None;
    let mut started = 0u64;
    let mut committed = 0u64;
    let mut busy_cleared = 0u64;
    let mut interrupt_raised = 0u64;
    let mut interrupt_cleared = 0u64;
    // The public begin API rejects an already asserted PI line before it
    // clears retained history, so a fresh epoch has one exact initial state.
    let mut interrupt_asserted = false;
    let mut notifications = 0u64;
    let mut to_rdram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:pi-writer-runtime-transitions:v2");

    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
            }
        }
        previous_order = Some(order);

        let transition = match event.kind {
            fn64_runtime::DeviceTraceKind::PiDmaStarted(request) => {
                if active.is_some() {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                active = Some(ActivePi { request, phase: 0 });
                started = started
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::PiBytesCommitted(request) => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.request != request || current.phase != 0 {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 1;
                committed = committed
                    .checked_add(1)
                    .expect("PI transition count overflow");
                if request.direction == fn64_runtime::DmaDirection::ToRdram {
                    to_rdram_committed = to_rdram_committed
                        .checked_add(1)
                        .expect("PI transition count overflow");
                }
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::PiBusyCleared => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.phase != 1 {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 2;
                busy_cleared = busy_cleared
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((2, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Pi) => {
                let Some(current) = active.as_mut() else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                if current.phase != 2 || interrupt_asserted {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                current.phase = 3;
                interrupt_asserted = true;
                interrupt_raised = interrupt_raised
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((3, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptCleared(
                fn64_runtime::InterruptSource::Pi,
            ) => {
                if !interrupt_asserted || active.is_some_and(|current| current.phase == 3) {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                interrupt_asserted = false;
                interrupt_cleared = interrupt_cleared
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((5, None))
            }
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::PiDmaComplete(completion),
            ) => {
                let Some(current) = active else {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                };
                let completed_request = fn64_runtime::PiDmaRequest {
                    direction: completion.direction,
                    dram_addr: completion.dram_addr,
                    device: completion.device,
                    len: completion.len,
                };
                if current.request != completed_request
                    || (current.phase != 3 && !(current.phase == 2 && interrupt_asserted))
                {
                    return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
                }
                active = None;
                notifications = notifications
                    .checked_add(1)
                    .expect("PI transition count overflow");
                Some((4, Some(completed_request)))
            }
            _ => None,
        };
        if let Some((tag, request)) = transition {
            transitions = transitions
                .checked_add(1)
                .expect("PI transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_pi_request(&mut hasher, request);
            }
        }
    }

    if active.is_some()
        || started != committed
        || started != busy_cleared
        || started != notifications
    {
        return Err(PiWriterRuntimeStateErrorV1::InvalidPiTransitionOrder);
    }
    if transitions == 0 || started == 0 {
        return Err(PiWriterRuntimeStateErrorV1::NoPiTransitions);
    }
    if to_rdram_committed == 0 {
        return Err(PiWriterRuntimeStateErrorV1::NoToRdramCommit);
    }
    Ok((
        started,
        committed,
        busy_cleared,
        interrupt_raised,
        interrupt_cleared,
        notifications,
        to_rdram_committed,
        hasher.finalize().into(),
    ))
}

fn hash_sp_request(hasher: &mut sha2::Sha256, request: fn64_runtime::SpDmaRequest) {
    hasher.update([match request.direction {
        fn64_runtime::SpDmaDirection::RdramToRsp => 0,
        fn64_runtime::SpDmaDirection::RspToRdram => 1,
    }]);
    hasher.update((request.mem_addr.offset() as u32).to_be_bytes());
    hasher.update(request.dram_addr.offset().to_be_bytes());
    hasher.update(request.encoded_len.to_be_bytes());
}

pub(super) fn validate_sp_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, u64, u64, [u8; 32]), SpWriterRuntimeStateErrorV1> {
    let mut active = None;
    let mut queued = None;
    let mut expect_busy_clear = false;
    let mut started = 0u64;
    let mut queued_count = 0u64;
    let mut committed = 0u64;
    let mut busy_cleared = 0u64;
    let mut rsp_to_rdram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:sp-writer-runtime-transitions:v1");

    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
            }
        }
        previous_order = Some(order);

        // A committed active slot promotes its queued request, or publishes
        // DMA idle, inside the same device event transition. No other retained
        // event can interleave between those two records.
        if let Some(expected) = queued {
            if active.is_none()
                && !matches!(
                    event.kind,
                    fn64_runtime::DeviceTraceKind::SpDmaStarted(actual) if actual == expected
                )
            {
                return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
            }
        } else if expect_busy_clear
            && !matches!(event.kind, fn64_runtime::DeviceTraceKind::SpDmaBusyCleared)
        {
            return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
        }

        let transition = match event.kind {
            fn64_runtime::DeviceTraceKind::SpDmaStarted(request) => {
                if active.is_some() || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                if let Some(expected) = queued.take() {
                    if expected != request {
                        return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                    }
                }
                active = Some(request);
                started = started
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaQueued(request) => {
                if active.is_none() || queued.is_some() || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                queued = Some(request);
                queued_count = queued_count
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaBytesCommitted(request) => {
                if active != Some(request) || expect_busy_clear {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                active = None;
                committed = committed
                    .checked_add(1)
                    .expect("SP transition count overflow");
                if request.direction == fn64_runtime::SpDmaDirection::RspToRdram {
                    rsp_to_rdram_committed = rsp_to_rdram_committed
                        .checked_add(1)
                        .expect("SP transition count overflow");
                }
                expect_busy_clear = queued.is_none();
                Some((2, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SpDmaBusyCleared => {
                if !expect_busy_clear || active.is_some() || queued.is_some() {
                    return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
                }
                expect_busy_clear = false;
                busy_cleared = busy_cleared
                    .checked_add(1)
                    .expect("SP transition count overflow");
                Some((3, None))
            }
            _ => None,
        };
        if let Some((tag, request)) = transition {
            transitions = transitions
                .checked_add(1)
                .expect("SP transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_sp_request(&mut hasher, request);
            }
        }
    }
    if active.is_some() || queued.is_some() || expect_busy_clear || started != committed {
        return Err(SpWriterRuntimeStateErrorV1::InvalidSpTransitionOrder);
    }
    if transitions == 0 || started == 0 {
        return Err(SpWriterRuntimeStateErrorV1::NoSpTransitions);
    }
    if rsp_to_rdram_committed == 0 {
        return Err(SpWriterRuntimeStateErrorV1::NoRspToRdramCommit);
    }
    Ok((
        started,
        queued_count,
        committed,
        busy_cleared,
        rsp_to_rdram_committed,
        hasher.finalize().into(),
    ))
}

fn hash_si_request(hasher: &mut sha2::Sha256, request: fn64_runtime::SiDmaRequest) {
    let kind = match request.kind {
        fn64_runtime::SiDmaKind::DramToPif => 0,
        fn64_runtime::SiDmaKind::PifToDram => 1,
        fn64_runtime::SiDmaKind::ControllerQuery => 2,
        fn64_runtime::SiDmaKind::ControllerRead => 3,
    };
    hasher.update([kind]);
    hasher.update(request.dram_addr.offset().to_be_bytes());
}

pub(super) fn validate_si_transition_trace(
    trace: &[fn64_runtime::DeviceTraceEvent],
) -> Result<(u64, u64, u64, [u8; 32]), SiWriterRuntimeStateErrorV1> {
    #[derive(Clone, Copy)]
    struct ActiveSi {
        request: fn64_runtime::SiDmaRequest,
        phase: u8,
    }

    let mut active: Option<ActiveSi> = None;
    let mut started = 0u64;
    let mut committed = 0u64;
    let mut pif_to_dram_committed = 0u64;
    let mut transitions = 0u64;
    let mut previous_order = None;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"fn64:si-writer-runtime-transitions:v1");
    for event in trace {
        let order = (event.at.get(), event.sequence);
        if let Some((previous_cycle, previous_sequence)) = previous_order {
            if order.0 < previous_cycle || order.1 <= previous_sequence {
                return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
            }
        }
        previous_order = Some(order);
        let tag = match event.kind {
            fn64_runtime::DeviceTraceKind::SiDmaStarted(request) => {
                if active.is_some() {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                active = Some(ActiveSi { request, phase: 0 });
                started = started
                    .checked_add(1)
                    .expect("SI transition count overflow");
                Some((0, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SiBytesCommitted(request) => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.request != request || current.phase != 0 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 1;
                committed = committed
                    .checked_add(1)
                    .expect("SI transition count overflow");
                if request.kind == fn64_runtime::SiDmaKind::PifToDram {
                    pif_to_dram_committed = pif_to_dram_committed
                        .checked_add(1)
                        .expect("SI transition count overflow");
                }
                Some((1, Some(request)))
            }
            fn64_runtime::DeviceTraceKind::SiBusyCleared => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.phase != 1 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 2;
                Some((2, None))
            }
            fn64_runtime::DeviceTraceKind::MiInterruptRaised(fn64_runtime::InterruptSource::Si) => {
                let Some(current) = active.as_mut() else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.phase != 2 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                current.phase = 3;
                Some((3, None))
            }
            fn64_runtime::DeviceTraceKind::NotificationReady(
                fn64_runtime::DeviceNotification::SiDmaComplete(request),
            ) => {
                let Some(current) = active else {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                };
                if current.request != request || current.phase != 3 {
                    return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
                }
                active = None;
                Some((4, Some(request)))
            }
            _ => None,
        };
        if let Some((tag, request)) = tag {
            transitions = transitions
                .checked_add(1)
                .expect("SI transition count overflow");
            hasher.update([tag]);
            hasher.update(event.at.get().to_be_bytes());
            hasher.update(event.sequence.to_be_bytes());
            if let Some(request) = request {
                hash_si_request(&mut hasher, request);
            }
        }
    }
    if active.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
    }
    if started == 0 || transitions == 0 {
        return Err(SiWriterRuntimeStateErrorV1::NoSiTransitions);
    }
    if started != committed {
        return Err(SiWriterRuntimeStateErrorV1::InvalidSiTransitionOrder);
    }
    if pif_to_dram_committed == 0 {
        return Err(SiWriterRuntimeStateErrorV1::NoPifToDramCommit);
    }
    Ok((
        started,
        committed,
        pif_to_dram_committed,
        hasher.finalize().into(),
    ))
}

pub(super) fn validate_bootstrap_binding(
    validated: &ValidatedBootstrapRdramV1,
    install: &CatalogGenerationInstallV1,
) -> Result<(), BootstrapImportErrorV1> {
    let evidence = validated.receipt.evidence();
    if evidence.schema != BOOTSTRAP_IMPORT_VALIDATION_SCHEMA_V1 {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch { field: "schema" });
    }
    if evidence.receipt_sha256 != bootstrap_receipt_sha256(evidence) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "receipt_sha256",
        });
    }
    if evidence.resolver_install_sha256 != resolver_install_definition_sha256(&install.resolver) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "resolver_install_sha256",
        });
    }
    if evidence.generation_catalog_sha256 != install.generations.canonical_definition_sha256() {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "generation_catalog_sha256",
        });
    }
    if evidence.initial_entry != install.resolver.entry() {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "initial_entry",
        });
    }
    let ranges = executable_physical_ranges(install);
    if evidence
        .watched_ranges
        .iter()
        .map(|range| (range.physical_start, range.physical_end))
        .ne(ranges.iter().copied())
    {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "watched_ranges",
        });
    }
    if evidence.watched_sha256 != watched_bytes_sha256(&validated.storage, &ranges) {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "watched_sha256",
        });
    }
    validate_initial_entry_image(install, &validated.storage)?;
    let view = fn64_runtime::RdramView::from_storage(&validated.storage);
    let initial_generations = install
        .generations
        .validate_initial_physical_images(|physical| {
            view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
        })
        .map_err(|error| match error {
            fn64_cpu_runtime::InitialGenerationImageErrorV1::UnrecognizedNonzeroByte {
                physical_address,
                actual,
            } => BootstrapImportErrorV1::UnrecognizedInitialGenerationImage {
                physical_address,
                actual,
            },
        })?;
    if evidence.initial_generations != initial_generations {
        return Err(BootstrapImportErrorV1::ReceiptBindingMismatch {
            field: "initial_generations",
        });
    }
    Ok(())
}

pub(super) fn validate_bootstrap_writer_completion_state(
    program_model_sha256: [u8; 32],
    bootstrap: &BootstrapOrImportValidationEvidenceV1,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
) -> Result<ValidatedBootstrapWriterChannelReceiptV1, BootstrapWriterChannelCompletionErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(BootstrapWriterChannelCompletionErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(BootstrapWriterChannelCompletionErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => {
            return Err(BootstrapWriterChannelCompletionErrorV1::PendingPhysicalWrites)
        }
        Some(PendingWriteViolation::Attributed) => {
            return Err(BootstrapWriterChannelCompletionErrorV1::PendingAttributedWrites)
        }
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(BootstrapWriterChannelCompletionErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(BootstrapWriterChannelCompletionErrorV1::ActiveChildTransaction);
    }
    if state.next_transaction_id != 0 || state.next_child_transaction_id != 0 {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedTransactionCounters);
    }
    if state.entries.len() != 1 || state.next_sequence != 1 {
        return Err(
            BootstrapWriterChannelCompletionErrorV1::MissingOrExtraJournalEntries {
                actual: state.entries.len(),
            },
        );
    }

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    if watched_ranges != bootstrap.watched_ranges {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedJournalEntry);
    }
    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(BootstrapWriterChannelCompletionErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(BootstrapWriterChannelCompletionErrorV1::CurrentWatchedBytesMismatch);
    }

    let zero_snapshot = state
        .watched
        .iter()
        .map(|range| vec![0; range.expected.len()])
        .collect::<Vec<_>>();
    let before_sha256 = state.digest_snapshot(&zero_snapshot);
    let initial_root =
        canonical_mutation_initial_root(before_sha256, watched_ranges.iter().copied());
    let expected_declarations = state.clipped_declarations(
        &bootstrap
            .publications
            .iter()
            .map(|publication| GuestWriteEvent::Range {
                channel: WriterChannel::BootstrapOrImport,
                physical_offset: publication.physical_start,
                len: publication.physical_end - publication.physical_start,
            })
            .collect::<Vec<_>>(),
    );
    let mut expected_changed_ranges = Vec::new();
    for (range, current) in state.watched.iter().zip(&snapshot) {
        let mut index = 0;
        while index < current.len() {
            if current[index] == 0 {
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < current.len() && current[index] != 0 {
                index += 1;
            }
            expected_changed_ranges.push(PendingExecutableWriteEvidenceSnapshot {
                physical_start: range.physical_start + start as u32,
                physical_end: range.physical_start + index as u32,
            });
        }
    }
    let entry = &state.entries[0];
    if entry.sequence != 0
        || entry.declared_writes != expected_declarations
        || entry.changed_ranges != expected_changed_ranges
        || entry.before_sha256 != before_sha256
        || entry.after_sha256 != final_watched_sha256
        || !entry.invalidated_generations.is_empty()
        || entry.journal_root_sha256 != canonical_mutation_entry_root(initial_root, entry)
        || state.journal_root_sha256 != entry.journal_root_sha256
    {
        return Err(BootstrapWriterChannelCompletionErrorV1::UnexpectedJournalEntry);
    }

    let mut evidence = BootstrapWriterChannelCompletionEvidenceV1 {
        schema: BOOTSTRAP_WRITER_CHANNEL_COMPLETION_SCHEMA_V1.to_string(),
        program_model_sha256,
        bootstrap_receipt_sha256: bootstrap.receipt_sha256,
        rom_sha256: bootstrap.rom_sha256,
        resolver_install_sha256: bootstrap.resolver_install_sha256,
        generation_catalog_sha256: bootstrap.generation_catalog_sha256,
        watched_ranges,
        bootstrap_watched_sha256: bootstrap.watched_sha256,
        initial_generations: bootstrap.initial_generations.clone(),
        journal_entry: entry.clone(),
        final_watched_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = bootstrap_writer_channel_completion_receipt_sha256(&evidence);
    let receipt = ValidatedBootstrapWriterChannelReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(BootstrapWriterChannelCompletionErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

pub(super) fn validate_cpu_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), CpuWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(CpuWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(CpuWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(CpuWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(CpuWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(CpuWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(CpuWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_cpu_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[(u32, u32)],
) -> Result<ValidatedCpuWriterRuntimeStateReceiptV1, CpuWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(CpuWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(CpuWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(CpuWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_cpu_writer_quiescence(state)?;
    if trace.is_empty() {
        return Err(CpuWriterRuntimeStateErrorV1::NoCpuStores);
    }
    if trace.iter().any(|&(start, len)| {
        len == 0
            || start
                .checked_add(len)
                .is_none_or(|end| end > fn64_cpu_runtime::RDRAM_LEN as u32)
    }) {
        return Err(CpuWriterRuntimeStateErrorV1::InvalidCpuStoreRange);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(CpuWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(CpuWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut trace_hasher = sha2::Sha256::new();
    trace_hasher.update(b"fn64:cpu-instruction-store-trace:v1");
    trace_hasher.update(trace_epoch_id.to_be_bytes());
    trace_hasher.update((trace.len() as u64).to_be_bytes());
    for &(physical_start, len) in trace {
        trace_hasher.update(physical_start.to_be_bytes());
        trace_hasher.update(len.to_be_bytes());
    }
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = CpuWriterRuntimeStateEvidenceV1 {
        schema: CPU_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("CPU runtime-state journal entry count exceeds u64"),
        cpu_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::CpuInstructionStore)
                .count(),
        )
        .expect("CPU runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        cpu_store_count: u64::try_from(trace.len()).expect("CPU store trace exceeds u64"),
        cpu_store_trace_sha256: trace_hasher.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = cpu_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedCpuWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(CpuWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

pub(super) fn validate_pi_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), PiWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(PiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(PiWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(PiWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(PiWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(PiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(PiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_pi_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_pi: bool,
    pending_abi_pi: bool,
) -> Result<ValidatedPiWriterRuntimeStateReceiptV1, PiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(PiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(PiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(PiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_pi_writer_quiescence(state)?;
    if pending_device_pi {
        return Err(PiWriterRuntimeStateErrorV1::PendingDevicePi);
    }
    if pending_abi_pi {
        return Err(PiWriterRuntimeStateErrorV1::PendingAbiPi);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(PiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(PiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let (
        pi_started,
        pi_committed,
        pi_busy_cleared,
        pi_interrupt_raised,
        pi_interrupt_cleared,
        pi_notifications,
        pi_to_rdram_committed,
        pi_transition_sha256,
    ) = validate_pi_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = PiWriterRuntimeStateEvidenceV1 {
        schema: PI_WRITER_RUNTIME_STATE_SCHEMA_V2.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("PI runtime-state journal entry count exceeds u64"),
        pi_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::PiDma)
                .count(),
        )
        .expect("PI runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        pi_started,
        pi_committed,
        pi_busy_cleared,
        pi_interrupt_raised,
        pi_interrupt_cleared,
        pi_notifications,
        pi_to_rdram_committed,
        pi_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = pi_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedPiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(PiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_si_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_si: bool,
    pending_abi_si: bool,
) -> Result<ValidatedSiWriterRuntimeStateReceiptV1, SiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(SiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(SiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(SiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(SiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(SiWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(SiWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(SiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(SiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    if pending_device_si {
        return Err(SiWriterRuntimeStateErrorV1::PendingDeviceSi);
    }
    if pending_abi_si {
        return Err(SiWriterRuntimeStateErrorV1::PendingAbiSi);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(SiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(SiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let (si_started, si_committed, si_pif_to_dram_committed, si_transition_sha256) =
        validate_si_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = SiWriterRuntimeStateEvidenceV1 {
        schema: SI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("SI runtime-state journal entry count exceeds u64"),
        si_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::SiDma)
                .count(),
        )
        .expect("SI runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        si_started,
        si_committed,
        si_pif_to_dram_committed,
        si_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = si_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedSiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(SiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_sp_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &[fn64_runtime::DeviceTraceEvent],
    pending_device_sp_dma: bool,
    pending_device_sp_task: bool,
    pending_abi_sp_work: bool,
) -> Result<ValidatedSpWriterRuntimeStateReceiptV1, SpWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(SpWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(SpWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(SpWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(SpWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(SpWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(SpWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(SpWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(SpWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(SpWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    if pending_device_sp_dma {
        return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpDma);
    }
    if pending_device_sp_task {
        return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpTask);
    }
    if pending_abi_sp_work {
        return Err(SpWriterRuntimeStateErrorV1::PendingAbiSpWork);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(SpWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(SpWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let (
        sp_started,
        sp_queued,
        sp_committed,
        sp_busy_cleared,
        sp_rsp_to_rdram_committed,
        sp_transition_sha256,
    ) = validate_sp_transition_trace(trace)?;
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = SpWriterRuntimeStateEvidenceV1 {
        schema: SP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("SP runtime-state journal entry count exceeds u64"),
        sp_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| declaration.channel == WriterChannel::SpDma)
                .count(),
        )
        .expect("SP runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        sp_started,
        sp_queued,
        sp_committed,
        sp_busy_cleared,
        sp_rsp_to_rdram_committed,
        sp_transition_sha256,
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = sp_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedSpWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(SpWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

pub(super) fn validate_host_abi_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), HostAbiWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(HostAbiWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(HostAbiWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(HostAbiWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(HostAbiWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(HostAbiWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(HostAbiWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_host_abi_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog: Option<&AbiHostFunctionCatalogEvidenceV1>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: Option<&HostAbiWriterTraceV1>,
) -> Result<ValidatedHostAbiWriterRuntimeStateReceiptV1, HostAbiWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog) = abi_host_catalog else {
        return Err(HostAbiWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(HostAbiWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    let Some(trace) = trace else {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    if trace.epoch_id != trace_epoch_id {
        return Err(HostAbiWriterRuntimeStateErrorV1::TraceEpochMismatch);
    }
    validate_host_abi_writer_quiescence(state)?;

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(HostAbiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(HostAbiWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut stacks = BTreeMap::<ThreadId, Vec<u64>>::new();
    let mut seen_transactions = BTreeMap::<u64, ()>::new();
    let mut traced_sequences = Vec::new();
    let mut transactions_started = 0u64;
    let mut transactions_finished = 0u64;
    let mut ordering_boundaries = 0u64;
    let mut lifecycle = sha2::Sha256::new();
    lifecycle.update(b"fn64:host-abi-writer-lifecycle:v1");
    lifecycle.update(trace.epoch_id.to_be_bytes());
    lifecycle.update(trace.initial_journal_entry_count.to_be_bytes());
    lifecycle.update((trace.events.len() as u64).to_be_bytes());
    for event in &trace.events {
        match event {
            HostAbiWriterTraceEventV1::Started(frame) => {
                let target_is_abi_writer = abi_host_catalog.bindings.iter().any(|binding| {
                    binding.target_pc == frame.target.get()
                        && binding.writer_effects.contains(&WriterChannel::HostAbi)
                });
                if !target_is_abi_writer
                    || seen_transactions.insert(frame.transaction_id, ()).is_some()
                {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                stacks
                    .entry(frame.thread)
                    .or_default()
                    .push(frame.transaction_id);
                transactions_started = transactions_started
                    .checked_add(1)
                    .expect("Host ABI transaction count exceeds u64");
                lifecycle.update([0]);
                lifecycle.update(frame.transaction_id.to_be_bytes());
                lifecycle.update(frame.thread.to_be_bytes());
                lifecycle.update(frame.target.get().to_be_bytes());
                lifecycle.update(frame.resume.bank.get().to_be_bytes());
                lifecycle.update(frame.resume.pc.get().to_be_bytes());
            }
            HostAbiWriterTraceEventV1::Boundary {
                transaction_id,
                thread,
                journal_sequences,
            } => {
                if stacks.get(thread).and_then(|stack| stack.last()).copied()
                    != Some(*transaction_id)
                {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                ordering_boundaries = ordering_boundaries
                    .checked_add(1)
                    .expect("Host ABI ordering-boundary count exceeds u64");
                lifecycle.update([1]);
                lifecycle.update(transaction_id.to_be_bytes());
                lifecycle.update(thread.to_be_bytes());
                lifecycle.update((journal_sequences.len() as u64).to_be_bytes());
                for sequence in journal_sequences {
                    let Ok(index) = usize::try_from(*sequence) else {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    };
                    let Some(entry) = state.entries.get(index) else {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    };
                    if entry.sequence != *sequence
                        || entry
                            .declared_writes
                            .iter()
                            .any(|declaration| declaration.channel != WriterChannel::HostAbi)
                    {
                        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                    }
                    traced_sequences.push(*sequence);
                    lifecycle.update(sequence.to_be_bytes());
                }
            }
            HostAbiWriterTraceEventV1::Finished {
                transaction_id,
                thread,
            } => {
                let Some(stack) = stacks.get_mut(thread) else {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                };
                if stack.pop() != Some(*transaction_id) {
                    return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
                }
                if stack.is_empty() {
                    stacks.remove(thread);
                }
                transactions_finished = transactions_finished
                    .checked_add(1)
                    .expect("Host ABI transaction count exceeds u64");
                lifecycle.update([2]);
                lifecycle.update(transaction_id.to_be_bytes());
                lifecycle.update(thread.to_be_bytes());
            }
        }
    }
    if transactions_started == 0 {
        return Err(HostAbiWriterRuntimeStateErrorV1::NoHostAbiTransactions);
    }
    if !stacks.is_empty() || transactions_finished != transactions_started {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }

    let Ok(initial_index) = usize::try_from(trace.initial_journal_entry_count) else {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    };
    if initial_index > state.entries.len() {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }
    let expected_sequences = state.entries[initial_index..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::HostAbi)
        })
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    if traced_sequences != expected_sequences {
        return Err(HostAbiWriterRuntimeStateErrorV1::InvalidHostAbiLifecycle);
    }
    if expected_sequences.is_empty() {
        return Err(HostAbiWriterRuntimeStateErrorV1::NoHostAbiWriteCommit);
    }
    let host_abi_journal_declaration_count = state.entries[initial_index..]
        .iter()
        .flat_map(|entry| &entry.declared_writes)
        .filter(|declaration| declaration.channel == WriterChannel::HostAbi)
        .count();

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = HostAbiWriterRuntimeStateEvidenceV1 {
        schema: HOST_ABI_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256: abi_host_catalog.receipt_sha256,
        build_receipt,
        trace_epoch_id,
        initial_journal_entry_count: trace.initial_journal_entry_count,
        final_journal_entry_count: u64::try_from(state.entries.len())
            .expect("Host ABI final journal entry count exceeds u64"),
        watched_ranges,
        host_abi_journal_entry_count: u64::try_from(expected_sequences.len())
            .expect("Host ABI journal entry count exceeds u64"),
        host_abi_journal_declaration_count: u64::try_from(host_abi_journal_declaration_count)
            .expect("Host ABI journal declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        transactions_started,
        transactions_finished,
        ordering_boundaries,
        lifecycle_sha256: lifecycle.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = host_abi_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedHostAbiWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(HostAbiWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

pub(super) fn validate_rsp_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), RspWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(RspWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(RspWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(RspWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(RspWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(RspWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(RspWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_rsp_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    trace_epoch_id: Option<u64>,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &crate::task_dispatch::RspWriterTraceSnapshotV1,
    pending_device_rsp_task: bool,
    pending_abi_rsp_work: bool,
) -> Result<ValidatedRspWriterRuntimeStateReceiptV1, RspWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(RspWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(RspWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    let Some(trace_epoch_id) = trace_epoch_id else {
        return Err(RspWriterRuntimeStateErrorV1::TraceEpochNotArmed);
    };
    validate_rsp_writer_quiescence(state)?;
    if pending_device_rsp_task {
        return Err(RspWriterRuntimeStateErrorV1::PendingDeviceRspTask);
    }
    if pending_abi_rsp_work {
        return Err(RspWriterRuntimeStateErrorV1::PendingAbiRspWork);
    }
    if !trace.rejected_journal_sequences.is_empty() {
        return Err(RspWriterRuntimeStateErrorV1::RejectedRspExecutableMutation);
    }
    if trace.commits.is_empty()
        && !trace
            .hle_publications
            .iter()
            .any(|publication| !publication.journal_sequences.is_empty())
    {
        return Err(RspWriterRuntimeStateErrorV1::NoRspWritebacks);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(RspWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(RspWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let mut interpreter_writeback_count = 0u64;
    let mut translated_audio_hle_publication_count = 0u64;
    let mut trace_hasher = sha2::Sha256::new();
    trace_hasher.update(b"fn64:rsp-execution-writeback-trace:v1");
    trace_hasher.update(trace_epoch_id.to_be_bytes());
    trace_hasher.update((trace.commits.len() as u64).to_be_bytes());
    for observation in &trace.commits {
        if observation.physical_start >= observation.physical_end
            || observation.physical_end > fn64_cpu_runtime::RDRAM_LEN as u32
        {
            return Err(RspWriterRuntimeStateErrorV1::InvalidRspWritebackRange);
        }
        let owner = match observation.source {
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { owner } => {
                interpreter_writeback_count = interpreter_writeback_count
                    .checked_add(1)
                    .expect("RSP interpreter writeback count overflow");
                trace_hasher.update([0]);
                owner
            }
            crate::task_dispatch::RspWriterCommitSourceV1::TranslatedAudioHle { .. } => {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspWritebackRange);
            }
        };
        match owner.task_offset() {
            Some(offset) => {
                trace_hasher.update([0]);
                trace_hasher.update(offset.to_be_bytes());
            }
            None => trace_hasher.update([1]),
        }
        trace_hasher.update(owner.admission_generation().get().to_be_bytes());
        trace_hasher.update(observation.physical_start.to_be_bytes());
        trace_hasher.update(observation.physical_end.to_be_bytes());
    }
    trace_hasher.update((trace.hle_publications.len() as u64).to_be_bytes());
    let mut claimed_hle_sequences = std::collections::BTreeSet::new();
    for publication in &trace.hle_publications {
        let owner = match publication.source {
            crate::task_dispatch::RspWriterCommitSourceV1::TranslatedAudioHle { owner } => owner,
            crate::task_dispatch::RspWriterCommitSourceV1::Interpreter { .. } => {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            }
        };
        translated_audio_hle_publication_count = translated_audio_hle_publication_count
            .checked_add(1)
            .expect("translated audio-HLE publication count overflow");
        trace_hasher.update([1]);
        match owner.task_offset() {
            Some(offset) => {
                trace_hasher.update([0]);
                trace_hasher.update(offset.to_be_bytes());
            }
            None => trace_hasher.update([1]),
        }
        trace_hasher.update(owner.admission_generation().get().to_be_bytes());
        trace_hasher.update((publication.journal_sequences.len() as u64).to_be_bytes());
        for &sequence in &publication.journal_sequences {
            if !claimed_hle_sequences.insert(sequence) {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            }
            let Some(entry) = state
                .entries
                .iter()
                .find(|entry| entry.sequence == sequence)
            else {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            };
            if !entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RspExecutionOrHleWriteback)
            {
                return Err(RspWriterRuntimeStateErrorV1::InvalidRspHlePublication);
            }
            trace_hasher.update(sequence.to_be_bytes());
        }
    }

    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect::<Vec<_>>();
    let mut evidence = RspWriterRuntimeStateEvidenceV1 {
        schema: RSP_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id,
        watched_ranges,
        journal_entry_count: u64::try_from(state.entries.len())
            .expect("RSP runtime-state journal entry count exceeds u64"),
        rsp_journal_declaration_count: u64::try_from(
            state
                .entries
                .iter()
                .flat_map(|entry| &entry.declared_writes)
                .filter(|declaration| {
                    declaration.channel == WriterChannel::RspExecutionOrHleWriteback
                })
                .count(),
        )
        .expect("RSP runtime-state declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        interpreter_writeback_count,
        translated_audio_hle_publication_count,
        writeback_range_count: u64::try_from(trace.commits.len())
            .expect("RSP writeback trace exceeds u64"),
        writeback_trace_sha256: trace_hasher.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = rsp_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedRspWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(RspWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}

pub(super) fn validate_rdp_renderer_writer_quiescence(
    state: &CanonicalExecutableMutationStateV1,
) -> Result<(), RdpRendererWriterRuntimeStateErrorV1> {
    if !state.sealed || state.expected_sha256.is_none() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::Unsealed);
    }
    if state.poison.is_some() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::Poisoned);
    }
    match pending_executable_write_violation() {
        Some(PendingWriteViolation::Physical) => return Err(RdpRendererWriterRuntimeStateErrorV1::PendingPhysicalWrites),
        Some(PendingWriteViolation::Attributed) => return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAttributedWrites),
        None => {}
    }
    if !state.host_transactions.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::OpenHostTransactions);
    }
    if state.active_child_transaction.is_some() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::ActiveChildTransaction);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_rdp_renderer_writer_runtime_state_v1(
    program_model_sha256: [u8; 32],
    resolver_install_sha256: [u8; 32],
    abi_host_catalog_receipt_sha256: Option<[u8; 32]>,
    build_receipt: StaticExecutionBuildReceipt,
    validated_owned_bootstrap: bool,
    epoch: &RdpRendererWriterRuntimeTraceEpochV1,
    storage: &[u8],
    state: &CanonicalExecutableMutationStateV1,
    trace: &RdpRendererWriterTraceV1,
    pending_device_rsp_task: bool,
    pending_device_dpc_transaction: bool,
    pending_device_dp_completion: bool,
    pending_abi_renderer_work: bool,
) -> Result<ValidatedRdpRendererWriterRuntimeStateReceiptV1, RdpRendererWriterRuntimeStateErrorV1> {
    if !validated_owned_bootstrap {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
    }
    let Some(abi_host_catalog_receipt_sha256) = abi_host_catalog_receipt_sha256 else {
        return Err(RdpRendererWriterRuntimeStateErrorV1::MissingAbiHostCatalogAuthority);
    };
    if !catalog_resolver_feature_lane_eligible(build_receipt) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NonProductionAotBuild);
    }
    if trace.epoch_id != epoch.epoch_id
        || trace.program_model_sha256 != program_model_sha256
        || epoch.program_model_sha256 != program_model_sha256
    {
        return Err(RdpRendererWriterRuntimeStateErrorV1::TraceEpochMismatch);
    }
    validate_rdp_renderer_writer_quiescence(state)?;
    if pending_device_rsp_task {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask);
    }
    if pending_device_dpc_transaction {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction);
    }
    if pending_device_dp_completion {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion);
    }
    if pending_abi_renderer_work {
        return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork);
    }
    if !trace.rejected_journal_sequences.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    if trace.publications.is_empty() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::NoRendererPublications);
    }

    let view = fn64_runtime::RdramView::from_storage(storage);
    let snapshot = state
        .read_snapshot_from_view(&view);
    if state
        .watched
        .iter()
        .zip(&snapshot)
        .any(|(range, current)| range.expected != *current)
    {
        return Err(RdpRendererWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }
    let final_watched_sha256 = state.digest_snapshot(&snapshot);
    if state.expected_sha256 != Some(final_watched_sha256) {
        return Err(RdpRendererWriterRuntimeStateErrorV1::CurrentWatchedBytesMismatch);
    }

    let Ok(initial_index) = usize::try_from(trace.initial_journal_entry_count) else {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    };
    if initial_index > state.entries.len() || trace.next_journal_entry_index > state.entries.len() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    let traced_sequences = trace
        .publications
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let expected_sequences = state.entries[initial_index..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RdpRenderer)
        })
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    if traced_sequences != expected_sequences {
        return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
    }
    for sequence in &traced_sequences {
        let Ok(index) = usize::try_from(*sequence) else {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        };
        let Some(entry) = state.entries.get(index) else {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        };
        if entry.sequence != *sequence
            || entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel != WriterChannel::RdpRenderer)
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::InvalidRendererPublicationTrace);
        }
    }

    let mut publication_trace = sha2::Sha256::new();
    publication_trace.update(b"fn64:rdp-renderer-publication-trace:v1");
    publication_trace.update(trace.epoch_id.to_be_bytes());
    publication_trace.update(trace.initial_journal_entry_count.to_be_bytes());
    publication_trace.update((trace.publications.len() as u64).to_be_bytes());
    for sequences in &trace.publications {
        publication_trace.update((sequences.len() as u64).to_be_bytes());
        for sequence in sequences {
            publication_trace.update(sequence.to_be_bytes());
        }
    }
    let rdp_renderer_journal_declaration_count = state.entries[initial_index..]
        .iter()
        .flat_map(|entry| &entry.declared_writes)
        .filter(|declaration| declaration.channel == WriterChannel::RdpRenderer)
        .count();
    let watched_ranges = state
        .watched
        .iter()
        .map(|range| PendingExecutableWriteEvidenceSnapshot {
            physical_start: range.physical_start,
            physical_end: range.physical_end,
        })
        .collect();
    let mut evidence = RdpRendererWriterRuntimeStateEvidenceV1 {
        schema: RDP_RENDERER_WRITER_RUNTIME_STATE_SCHEMA_V1.to_string(),
        program_model_sha256,
        resolver_install_sha256,
        abi_host_catalog_receipt_sha256,
        build_receipt,
        trace_epoch_id: trace.epoch_id,
        initial_journal_entry_count: trace.initial_journal_entry_count,
        final_journal_entry_count: u64::try_from(state.entries.len())
            .expect("RDP renderer final journal entry count exceeds u64"),
        watched_ranges,
        rdp_renderer_journal_entry_count: u64::try_from(expected_sequences.len())
            .expect("RDP renderer journal entry count exceeds u64"),
        rdp_renderer_journal_declaration_count: u64::try_from(
            rdp_renderer_journal_declaration_count,
        )
        .expect("RDP renderer journal declaration count exceeds u64"),
        journal_root_sha256: state.journal_root_sha256,
        final_watched_sha256,
        renderer_publication_count: u64::try_from(trace.publications.len())
            .expect("RDP renderer publication count exceeds u64"),
        publication_trace_sha256: publication_trace.finalize().into(),
        receipt_sha256: [0; 32],
    };
    evidence.receipt_sha256 = rdp_renderer_writer_runtime_state_receipt_sha256(&evidence);
    let receipt = ValidatedRdpRendererWriterRuntimeStateReceiptV1 { evidence };
    if !receipt.has_valid_evidence_hash() {
        return Err(RdpRendererWriterRuntimeStateErrorV1::ReceiptHashMismatch);
    }
    Ok(receipt)
}
