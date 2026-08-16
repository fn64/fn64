use super::*;

pub(super) struct LiveTransferResolver {
    pub(super) live: LiveBlockProgram,
}

impl TransferResolver for LiveTransferResolver {
    fn resolve(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        self.live.resolve_transfer(source_bank, target_pc)
    }

    fn resolve_call(
        &mut self,
        source_bank: BankId,
        target_pc: GuestPc,
        _resume: ExecutionKey,
    ) -> Result<CallResolution, CpuFault> {
        if fn64_cpu_runtime::resolve_host_function(target_pc.get()).is_some() {
            Ok(CallResolution::Host)
        } else {
            self.resolve(source_bank, target_pc)
                .map(CallResolution::Guest)
        }
    }
}

impl LiveBlockProgram {
    pub(super) fn resolve_transfer(
        &self,
        source_bank: BankId,
        target_pc: GuestPc,
    ) -> Result<ExecutionKey, CpuFault> {
        if let Some(catalog) = self.precompiled_generations.borrow().as_ref() {
            if let Ok(key) = catalog.resolve_active(target_pc) {
                return Ok(key);
            }
        }
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.transfer_lookup)(source_bank, target_pc)
    }

    pub(super) fn resolve_entry(&self, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {
        if let Some(catalog) = self.precompiled_generations.borrow().as_ref() {
            if let Ok(key) = catalog.resolve_active(target_pc) {
                return Ok(key);
            }
        }
        if let Some(key) = self
            .executable_regions
            .borrow()
            .iter()
            .find_map(|observed| observed.region.resolve(target_pc))
        {
            return Ok(key);
        }
        (self.entry_lookup)(target_pc)
    }
}

fn canonical_pending_executable_writes() -> Vec<PendingExecutableWriteEvidenceSnapshot> {
    let mut writes = PENDING_EXECUTABLE_WRITES.with(|pending| {
        pending
            .borrow()
            .iter()
            .map(|&(physical_start, len)| {
                assert!(len > 0, "pending executable write has zero length");
                let physical_end = physical_start
                    .checked_add(len)
                    .expect("pending executable write exceeds physical address space");
                PendingExecutableWriteEvidenceSnapshot {
                    physical_start,
                    physical_end,
                }
            })
            .collect::<Vec<_>>()
    });
    writes.sort_unstable_by_key(|write| (write.physical_start, write.physical_end));
    let mut canonical: Vec<PendingExecutableWriteEvidenceSnapshot> = Vec::new();
    for write in writes {
        if let Some(previous) = canonical.last_mut() {
            if write.physical_start <= previous.physical_end {
                previous.physical_end = previous.physical_end.max(write.physical_end);
                continue;
            }
        }
        canonical.push(write);
    }
    canonical
}

/// Capture the installed typed-Rust program without runner, resolver,
/// builder, lookup, or native function-pointer values.
///
/// The legacy function install API remains executable for compatibility, but
/// it is intentionally not evidence-capable: callers must use
/// [`set_entry_lookup_with_artifact_identity`] (or the matching boot helper)
/// before this function will describe a function lane. This prevents section
/// geometry or process-specific pointer bits from impersonating code identity.
pub fn recompiled_program_evidence_snapshot() -> Option<RecompiledProgramEvidenceSnapshot> {
    let (function_lane, block_lane, catalog_lane) = with_host(|host| {
        (
            host.recompiled_lookup.is_some(),
            host.recompiled_program.clone(),
            host.canonical_recompiled_program.clone(),
        )
    });
    assert!(
        usize::from(function_lane)
            + usize::from(block_lane.is_some())
            + usize::from(catalog_lane.is_some())
            <= 1,
        "multiple mutually exclusive recompiled lanes are installed simultaneously"
    );
    if function_lane {
        let identity = FUNCTION_LANE_ARTIFACT_IDENTITY
            .with(std::cell::Cell::get)
            .unwrap_or_else(|| {
                panic!(
                    "function-lane release evidence requires a stable host-provided artifact identity"
                )
            });
        return Some(RecompiledProgramEvidenceSnapshot::Function {
            identity: ProgramIdentityEvidenceSnapshot {
                identity,
                source: ProgramIdentitySource::CallerSupplied,
            },
        });
    }
    if let Some(live) = catalog_lane {
        assert!(
            !live.dynamic_execution_installed(),
            "static recompiled-program evidence is unavailable after dynamic mapped execution is installed"
        );
        return Some(RecompiledProgramEvidenceSnapshot::Block {
            program: live.install.program_evidence().clone(),
            dispatch_artifact_identity: live.install.evidence().dispatch_artifact_identity,
            instruction_budget: live.install.budget().get(),
            executable_regions: Vec::new(),
            pending_executable_writes: if live.generations.is_some() {
                canonical_pending_executable_writes()
            } else {
                Vec::new()
            },
        });
    }
    let live = block_lane?;
    let program = live.program.borrow().evidence_snapshot();
    let dispatch_artifact_identity = live.dispatch_artifact_identity.unwrap_or_else(|| {
        panic!(
            "block-lane release evidence requires a stable host-provided dispatch artifact identity"
        )
    });
    let mut executable_regions = live
        .executable_regions
        .borrow()
        .iter()
        .map(|observed| {
            let active_bank = observed.region.active_bank().unwrap_or_else(|| {
                panic!("observed executable region has no active bank during evidence capture")
            });
            let active_generation = observed
                .next_generation
                .checked_sub(1)
                .expect("observed executable region has no active generation");
            let builder_artifact_identity = observed.builder_artifact_identity.unwrap_or_else(|| {
                panic!(
                    "executable-region release evidence requires a stable host-provided builder artifact identity"
                )
            });
            LiveExecutableRegionEvidenceSnapshot {
                physical_start: observed.physical_start,
                physical_end: observed.physical_end,
                virtual_start: observed.region.start(),
                virtual_end: observed.region.end(),
                active_bank,
                active_generation,
                next_generation: observed.next_generation,
                builder_artifact_identity,
                activation: match observed.activation {
                    ExecutableActivation::EagerPublication => {
                        ExecutableActivationEvidence::EagerPublication
                    }
                    ExecutableActivation::FetchBoundary => {
                        ExecutableActivationEvidence::FetchBoundary
                    }
                },
            }
        })
        .collect::<Vec<_>>();
    executable_regions.sort_unstable_by_key(|region| {
        (
            region.physical_start,
            region.physical_end,
            region.virtual_start,
            region.virtual_end,
        )
    });
    Some(RecompiledProgramEvidenceSnapshot::Block {
        program,
        dispatch_artifact_identity,
        instruction_budget: live.budget.get(),
        executable_regions,
        pending_executable_writes: canonical_pending_executable_writes(),
    })
}

/// Capture evidence only for the callback-free canonical catalog owner.
/// Legacy function and block installs always return `None` here, even when
/// they can produce the broader compatibility evidence snapshot above.
pub fn catalog_resolver_install_evidence_snapshot() -> Option<CatalogResolverInstallEvidenceV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map(|live| live.install.evidence().clone())
    })
}

pub fn catalog_generation_install_evidence_snapshot() -> Option<CatalogGenerationInstallEvidenceV1>
{
    with_host(|host| {
        host.canonical_recompiled_program.as_ref().and_then(|live| {
            live.generation_evidence_snapshot().map(|generations| {
                CatalogGenerationInstallEvidenceV1 {
                    resolver: live.install.evidence().clone(),
                    generations,
                    bootstrap: live.bootstrap_evidence.clone(),
                    pending_physical_writes: canonical_pending_executable_writes(),
                    mutation_journal: live.mutation_evidence_snapshot(),
                }
            })
        })
    })
}

/// Runtime mutation evidence exists only for the callback-free canonical
/// generation owner. An unsealed snapshot means installation occurred but no
/// guest dispatch has yet established the immutable bootstrap baseline.
pub fn canonical_executable_mutation_journal_evidence_snapshot(
) -> Option<CanonicalExecutableMutationJournalEvidenceV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .and_then(CanonicalLiveBlockProgramV1::mutation_evidence_snapshot)
    })
}

/// Transfer the one move-only bootstrap writer-channel authority minted by
/// the canonical validated boot path. A second take returns `None`; retained
/// evidence cannot be deserialized or replayed into another capability.
pub fn take_validated_bootstrap_writer_channel_receipt_v1(
) -> Option<ValidatedBootstrapWriterChannelReceiptV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .and_then(|live| live.bootstrap_writer_completion.borrow_mut().take())
    })
}

/// Start one fresh CPU instruction-store audit window.
///
/// The runtime must be quiescent before arming. The returned move-only token
/// is bound to the exact canonical program model; beginning a replacement
/// window supersedes the prior token. This clears only ABI-private CPU trace
/// state and cannot be reconstructed from copied observations.
pub fn begin_cpu_writer_runtime_trace_epoch_v1(
) -> Result<Option<CpuWriterRuntimeTraceEpochV1>, CpuWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(CpuWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        live.begin_cpu_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local CPU-store runtime prerequisite.
///
/// At least one post-commit CPU RDRAM store must have crossed the typed write
/// observer after this exact epoch was armed. Successful validation requires
/// a second quiescent boundary and consumes both the live epoch and the sole
/// receipt. It is not selected-build or writer-denominator authority.
pub fn take_validated_cpu_writer_runtime_state_receipt_v1(
    epoch: &CpuWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedCpuWriterRuntimeStateReceiptV1>, CpuWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_cpu_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
        )
    })
}

/// Start one fresh canonical Host ABI writer audit window.
///
/// The runtime must be quiescent and own an ABI-issued stable-shim catalog;
/// compatibility caller pointers fail closed. The move-only token binds the
/// subsequent exact transaction lifecycle to this canonical program model.
pub fn begin_host_abi_writer_runtime_trace_epoch_v1(
) -> Result<Option<HostAbiWriterRuntimeTraceEpochV1>, HostAbiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(HostAbiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        live.begin_host_abi_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local Host ABI writer prerequisite.
///
/// Success requires balanced per-thread LIFO transactions through ABI-issued
/// targets and at least one actual HostAbi executable-journal commit after the
/// exact epoch was armed. A host invocation with no observed write is not
/// promoted into writer authority. This is not denominator completion.
pub fn take_validated_host_abi_writer_runtime_state_receipt_v1(
    epoch: &HostAbiWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedHostAbiWriterRuntimeStateReceiptV1>, HostAbiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_host_abi_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
        )
    })
}

/// Start one fresh ABI-owned RSP writeback audit window.
///
/// The runtime must have no admitted/running/yielded task, in-flight
/// interpreter, retained HLE continuation, or pending SP task. The returned
/// token authenticates interpreter writeback ranges and successful translated
/// audio-HLE executable publications. Rejected HLE journal sequences poison
/// the epoch instead of becoming later success evidence.
pub fn begin_rsp_writer_runtime_trace_epoch_v1(
) -> Result<Option<RspWriterRuntimeTraceEpochV1>, RspWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(RspWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if host.device_fabric.snapshot().sp_busy {
            return Err(RspWriterRuntimeStateErrorV1::PendingDeviceRspTask);
        }
        if host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1()
        {
            return Err(RspWriterRuntimeStateErrorV1::PendingAbiRspWork);
        }
        live.begin_rsp_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local RSP writeback prerequisite.
///
/// Success requires at least one nonempty interpreter range or one translated
/// HLE executable-journal sequence, exact owner generations, a second
/// quiescent boundary, and unchanged sealed watched state. No denominator
/// accepts this receipt directly.
pub fn take_validated_rsp_writer_runtime_state_receipt_v1(
    epoch: &RspWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedRspWriterRuntimeStateReceiptV1>, RspWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let pending_abi_rsp_work = host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1();
        live.take_rsp_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.snapshot().sp_busy,
            pending_abi_rsp_work,
        )
    })
}

/// Start one fresh renderer-publication audit epoch.
///
/// Arming requires a validated production-AOT owner and no live RSP task,
/// DPC transaction, DP completion, renderer continuation, or ABI task owner.
/// The returned token is ABI-local prerequisite authority only.
pub fn begin_rdp_renderer_writer_runtime_trace_epoch_v1(
) -> Result<Option<RdpRendererWriterRuntimeTraceEpochV1>, RdpRendererWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        let device = host.device_fabric.snapshot();
        if device.sp_busy {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceRspTask);
        }
        if device.pending_dpc.is_some() {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpcTransaction);
        }
        if device.dp_busy {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingDeviceDpCompletion);
        }
        if host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1()
        {
            return Err(RdpRendererWriterRuntimeStateErrorV1::PendingAbiRendererWork);
        }
        live.begin_rdp_renderer_writer_runtime_trace_epoch()
    })
}

/// Validate and transfer the ABI-local renderer publication prerequisite.
///
/// Success requires at least one backend-committed publication in the exact
/// epoch, a second quiescent boundary, and complete agreement between traced
/// RDP journal sequences and the canonical watched-byte journal.
pub fn take_validated_rdp_renderer_writer_runtime_state_receipt_v1(
    epoch: &RdpRendererWriterRuntimeTraceEpochV1,
) -> Result<
    Option<ValidatedRdpRendererWriterRuntimeStateReceiptV1>,
    RdpRendererWriterRuntimeStateErrorV1,
> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let device = host.device_fabric.snapshot();
        let pending_abi_renderer_work = host.loaded_rsp_task.is_some()
            || !host.rsp_task_lineages.is_empty()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
            || crate::task_dispatch::hle_rsp_writer_work_pending_v1();
        live.take_rdp_renderer_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            device.sp_busy,
            device.pending_dpc.is_some(),
            device.dp_busy,
            pending_abi_renderer_work,
        )
    })
}

/// Start one fresh, typed PI-DMA writer audit epoch.
///
/// The canonical runtime must be quiescent with no active device request,
/// queued ABI completion owner, or previously asserted PI interrupt. A
/// successful arm clears retained device history and binds the returned
/// move-only token to this exact canonical program model. It is not selected-
/// build or writer-denominator authority.
pub fn begin_pi_writer_runtime_trace_epoch_v1(
) -> Result<Option<PiWriterRuntimeTraceEpochV1>, PiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(PiWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        let epoch = live.begin_pi_writer_runtime_trace_epoch(
            host.device_fabric.pending_pi_request().is_some(),
            !host.pending_pi_completions.is_empty(),
            host.device_fabric
                .interrupt_pending(fn64_runtime::InterruptSource::Pi),
        )?;
        if epoch.is_some() {
            host.device_fabric.set_trace_enabled(false);
            host.device_fabric.set_trace_enabled(true);
        }
        Ok(epoch)
    })
}

/// Validate and transfer the ABI-local PI-DMA runtime prerequisite.
///
/// The move-only epoch must come from
/// [`begin_pi_writer_runtime_trace_epoch_v1`] for this exact live program.
/// Successful validation proves a balanced PI lifecycle with at least one
/// committed device-to-RDRAM transfer and consumes the sole receipt. It is
/// not writer-denominator completion.
pub fn take_validated_pi_writer_runtime_state_receipt_v1(
    epoch: &PiWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedPiWriterRuntimeStateReceiptV1>, PiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        live.take_pi_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.pending_pi_request().is_some(),
            !host.pending_pi_completions.is_empty(),
        )
    })
}

/// Validate and transfer the ABI-local SI runtime-state prerequisite once.
///
/// The canonical runtime must be between guest scheduling steps with no SI
/// request, ABI completion owner, executable write, or writer transaction in
/// flight. A failed attempt does not consume the one successful take, so a
/// host may first drain an already accepted SI request. This receipt is not a
/// writer-denominator completion capability and carries no generated-build
/// authority.
pub fn take_validated_si_writer_runtime_state_receipt_v1(
) -> Result<Option<ValidatedSiWriterRuntimeStateReceiptV1>, SiWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let storage = host.owned_runtime_rdram.as_deref().unwrap_or(&[]);
        live.take_si_writer_runtime_state(
            storage,
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.pending_si_request().is_some(),
            host.pending_si_completion.is_some(),
        )
    })
}

/// Start one fresh, typed SP-DMA writer audit epoch.
///
/// The runtime must already be quiescent. This operation discards retained
/// device history, re-enables retention, and returns a move-only token bound
/// to this canonical program model. Unlike the older SI prerequisite, whose
/// selected-child verifier owns trace freshness externally, SP freshness is
/// enforced by the ABI token and cannot be reconstructed from copied events.
pub fn begin_sp_writer_runtime_trace_epoch_v1(
) -> Result<Option<SpWriterRuntimeTraceEpochV1>, SpWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        if live.bootstrap_evidence.is_none()
            || host.owned_runtime_rdram.is_none()
            || host.runtime_rdram_len == 0
        {
            return Err(SpWriterRuntimeStateErrorV1::NotValidatedOwnedBootstrap);
        }
        if host.device_fabric.sp_dma_busy() {
            return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpDma);
        }
        if host.device_fabric.snapshot().sp_busy {
            return Err(SpWriterRuntimeStateErrorV1::PendingDeviceSpTask);
        }
        if host.loaded_rsp_task.is_some()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            )
        {
            return Err(SpWriterRuntimeStateErrorV1::PendingAbiSpWork);
        }
        let epoch = live.begin_sp_writer_runtime_trace_epoch()?;
        if epoch.is_some() {
            host.device_fabric.set_trace_enabled(false);
            host.device_fabric.set_trace_enabled(true);
        }
        Ok(epoch)
    })
}

/// Validate and transfer the ABI-local SP-DMA runtime-state prerequisite.
///
/// The move-only epoch must come from
/// [`begin_sp_writer_runtime_trace_epoch_v1`] for this exact live program.
/// A successful receipt proves a balanced raw SP-DMA lifecycle including at
/// least one RSP-to-RDRAM commit; it is not writer-denominator completion.
pub fn take_validated_sp_writer_runtime_state_receipt_v1(
    epoch: &SpWriterRuntimeTraceEpochV1,
) -> Result<Option<ValidatedSpWriterRuntimeStateReceiptV1>, SpWriterRuntimeStateErrorV1> {
    with_host(|host| {
        let Some(live) = host.canonical_recompiled_program.clone() else {
            return Ok(None);
        };
        let validated_owned_bootstrap = live.bootstrap_evidence.is_some()
            && host.owned_runtime_rdram.is_some()
            && host.runtime_rdram_len != 0;
        let pending_abi_sp_work = host.loaded_rsp_task.is_some()
            || matches!(
                &host.rsp_interpreter_state,
                crate::task_dispatch::RspInterpreterStateEvidenceSnapshot::InFlight { .. }
            );
        live.take_sp_writer_runtime_state(
            epoch,
            host.owned_runtime_rdram.as_deref().unwrap_or(&[]),
            validated_owned_bootstrap,
            host.device_fabric.trace(),
            host.device_fabric.sp_dma_busy(),
            host.device_fabric.snapshot().sp_busy,
            pending_abi_sp_work,
        )
    })
}

/// Copy successfully entered arbitrary-PC destinations in exact runner-entry
/// order. An empty vector means either that no block lane is installed or that
/// its admitted program has not executed; callers select the authoritative
/// interpretation from [`recompiled_program_evidence_snapshot`].
pub fn copy_block_execution_destinations() -> Vec<ExecutionDestinationObservation> {
    let (legacy, catalog) = with_host(|host| {
        (
            host.recompiled_program.clone(),
            host.canonical_recompiled_program.clone(),
        )
    });
    if let Some(live) = catalog {
        return live.install.copy_execution_destinations();
    }
    legacy.map_or_else(Vec::new, |live| {
        live.program.borrow().copy_execution_destinations()
    })
}

/// Return the exact total instruction-budget work charged by all OSThreads
/// executing the currently installed canonical `BlockProgram`.
///
/// The counter is reset on install, includes architectural fault attempts,
/// and excludes synthetic host/legacy-C scheduling charges. Callers sampling
/// it after `run_one_step` observe a global scheduler boundary; it is an
/// operational progress measure, not static coverage or release authority.
pub fn canonical_block_charged_instructions_v1() -> Option<u64> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map(|live| live.canonical_charged_instructions.get())
    })
}

/// Bound each subsequent canonical dispatch slice so the process-wide charged
/// instruction counter can stop at one exact operational checkpoint. A final
/// straight instruction may use a one-instruction slice; a branch and delay
/// slot remain indivisible and fail loudly if only one instruction remains.
/// This is scheduling evidence control, not guest state or static execution
/// authority. Clearing the limit restores the install's immutable default
/// slice budget.
pub fn set_canonical_block_instruction_limit_v1(limit: Option<u64>) {
    with_host(|host| {
        let live = host
            .canonical_recompiled_program
            .as_ref()
            .unwrap_or_else(|| panic!("canonical instruction limit requires an installed catalog"));
        if let Some(limit) = limit {
            assert!(
                live.canonical_instruction_limit.get().is_none(),
                "canonical instruction limit is already armed"
            );
            let charged = live.canonical_charged_instructions.get();
            assert!(
                limit > charged,
                "canonical instruction limit {limit} must exceed already charged work {charged}"
            );
        }
        live.canonical_instruction_limit.set(limit);
    });
}

/// Copy each canonical thread's latest pointer-free publication in thread-ID
/// order. This is observational state only: the copy does not establish that
/// every thread is quiescent or that a complete runtime state was captured.
pub fn copy_canonical_thread_publications_v1() -> Vec<CanonicalThreadPublicationV1> {
    with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .map_or_else(Vec::new, |live| {
                live.thread_publications
                    .borrow()
                    .values()
                    .cloned()
                    .collect()
            })
    })
}

/// Copy bounded source-bound dynamic execution hotness in full-identity order.
/// Saturation is retained explicitly in the dropped counters. This is
/// operational promotion input only; it is never merged into static
/// destination evidence or writer/release authority.
#[cfg(feature = "dynamic-mapped-runtime")]
pub fn copy_dynamic_mapped_execution_telemetry_v1() -> DynamicMappedExecutionTelemetryV1 {
    let live = with_host(|host| host.canonical_recompiled_program.clone())
        .expect("dynamic execution telemetry requires a canonical catalog install");
    assert!(
        live.dynamic_execution_installed(),
        "dynamic execution telemetry requires an enabled dynamic catalog"
    );
    let mutation = live.mutation_state.as_ref().map(|state| {
        let state = state.borrow();
        (state.journal_root_sha256, state.entries.len() as u64)
    });
    let aggregates = live
        .dynamic_execution_aggregates
        .borrow()
        .values()
        .cloned()
        .collect();
    DynamicMappedExecutionTelemetryV1 {
        resolver_install_sha256: resolver_install_definition_sha256(&live.install),
        program_identity: live.install.evidence().program_identity,
        dynamic_source_sha256: fn64_cpu_runtime::dynamic_mapped_execution_build_receipt_v1()
            .source_sha256(),
        rom_sha256: live
            .bootstrap_evidence
            .as_ref()
            .map(|evidence| evidence.rom_sha256),
        bootstrap_receipt_sha256: live
            .bootstrap_evidence
            .as_ref()
            .map(|evidence| evidence.receipt_sha256),
        mutation_journal_root_sha256: mutation.map(|(root, _)| root),
        mutation_journal_entry_count: mutation.map_or(0, |(_, count)| count),
        aggregates,
        aggregate_capacity: DYNAMIC_EXECUTION_AGGREGATE_CAPACITY as u64,
        attempted_entries_per_aggregate_capacity: DYNAMIC_ATTEMPTED_ENTRIES_PER_AGGREGATE_CAPACITY
            as u64,
        dropped_identity_activations: live.dynamic_dropped_identity_activations.get(),
        dropped_identity_charged_instructions: live
            .dynamic_dropped_identity_charged_instructions
            .get(),
        dropped_identity_unsupported_exits: live.dynamic_dropped_identity_unsupported_exits.get(),
        dropped_attempted_entry_activations: live.dynamic_dropped_attempted_entry_activations.get(),
        dropped_attempted_entry_charged_instructions: live
            .dynamic_dropped_attempted_entry_charged_instructions
            .get(),
        dropped_attempted_entry_unsupported_exits: live
            .dynamic_dropped_attempted_entry_unsupported_exits
            .get(),
    }
}

pub fn copy_block_host_boundaries() -> Vec<BlockHostBoundaryObservation> {
    BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow().iter().copied().collect())
}

/// Bound diagnostic host-boundary history. `None` restores complete history,
/// which is the default required by certification evidence.
pub fn set_block_host_boundary_history_limit(limit: Option<NonZeroUsize>) {
    BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|installed| installed.set(limit));
    if let Some(limit) = limit {
        BLOCK_HOST_BOUNDARIES.with(|boundaries| {
            let mut boundaries = boundaries.borrow_mut();
            while boundaries.len() > limit.get() {
                boundaries.pop_front();
            }
        });
    }
}

/// Enable or suppress diagnostic host-boundary history. Complete history is
/// enabled by default; suppressing it also clears any retained entries.
pub fn set_block_host_boundary_history_enabled(enabled: bool) {
    BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(|installed| installed.set(enabled));
    if !enabled {
        BLOCK_HOST_BOUNDARIES.with(|boundaries| boundaries.borrow_mut().clear());
    }
}

fn observe_block_host_boundary(
    phase: BlockHostBoundaryPhase,
    target: GuestPc,
    resume: ExecutionKey,
    ctx: &RsContext,
) {
    if !BLOCK_HOST_BOUNDARY_HISTORY_ENABLED.with(Cell::get) {
        return;
    }
    BLOCK_HOST_BOUNDARIES.with(|boundaries| {
        let mut boundaries = boundaries.borrow_mut();
        boundaries.push_back(BlockHostBoundaryObservation {
            at: fn64_runtime::Cycles::new(crate::sim_time()),
            thread: crate::current_thread_id("block host-boundary observation"),
            phase,
            target,
            resume,
            gprs: ctx.gprs(),
            hi: ctx.hi,
            lo: ctx.lo,
            cop0_count: ctx.cop0_count,
            cop0_compare: ctx.cop0_compare,
            cop0_status: ctx.cop0_status,
            cop0_cause: ctx.cop0_cause,
            cop0_epc: ctx.cop0_epc,
        });
        BLOCK_HOST_BOUNDARY_HISTORY_LIMIT.with(|limit| {
            if let Some(limit) = limit.get() {
                while boundaries.len() > limit.get() {
                    boundaries.pop_front();
                }
            }
        });
    });
}

pub(super) fn invoke_observed_block_host(
    target: GuestPc,
    resume: ExecutionKey,
    host: RecompFunc,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    observe_block_host_boundary(BlockHostBoundaryPhase::Enter, target, resume, ctx);
    host(ctx, mem);
    observe_block_host_boundary(BlockHostBoundaryPhase::Exit, target, resume, ctx);
}

pub(super) fn invoke_catalog_block_host(
    live: &CanonicalLiveBlockProgramV1,
    target: GuestPc,
    resume: ExecutionKey,
    host: RecompFunc,
    ctx: &mut RsContext,
    mem: &mut Rdram<'_>,
) {
    live.publish_opaque_host(target, resume);
    let transaction = live.begin_host_abi_transaction(target, resume, mem);
    let invocation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        invoke_observed_block_host(target, resume, host, ctx, mem);
    }));
    if let Err(payload) = invocation {
        if let Some(transaction) = transaction {
            live.mutation_state
                .as_ref()
                .expect("host transaction lost canonical mutation state while unwinding")
                .borrow_mut()
                .poison(format!(
                    "host ABI transaction {} for thread {} unwound before commit",
                    transaction.transaction_id, transaction.thread
                ));
        }
        std::panic::resume_unwind(payload);
    }
    live.finish_host_abi_transaction(transaction, mem);
}

/// Copy successfully entered emitted whole-function destinations in exact
/// entry order. An installed legacy function lane fails closed: only the API
/// which consumes the generated artifact's observation-schema marker can make
/// this history authoritative.
pub fn copy_function_execution_destinations() -> Vec<FunctionExecutionDestinationObservation> {
    let function_lane = with_host(|host| host.recompiled_lookup.is_some());
    if !function_lane {
        return Vec::new();
    }
    FUNCTION_LANE_ENTRY_OBSERVATION_SCHEMA.with(|schema| {
        schema.get().unwrap_or_else(|| {
            panic!(
                "function-lane destination evidence requires the generated artifact's entry-observation schema"
            )
        });
    });
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| destinations.borrow().clone())
}

pub(super) fn observe_function_entry(function: TranslatedFunctionIdentity) {
    let artifact_identity = FUNCTION_LANE_ARTIFACT_IDENTITY
        .with(std::cell::Cell::get)
        .unwrap_or_else(|| {
            panic!("observed function entry has no stable generated-artifact identity")
        });
    let at = fn64_runtime::Cycles::new(crate::sim_time());
    FUNCTION_EXECUTION_DESTINATIONS.with(|destinations| {
        destinations
            .borrow_mut()
            .push(FunctionExecutionDestinationObservation {
                at,
                artifact_identity,
                function,
            });
    });
}

pub(super) fn observe_renderer_write(event: GuestWriteEvent) {
    if let GuestWriteEvent::NonRdpWrite16 {
        logical_offset,
        value,
        ..
    } = event
    {
        crate::task_dispatch::observe_non_rdp_write16(logical_offset, value);
    }
}

/// TEMPORARY (mprotect feasibility census, 2026-08-07).
///
/// Counts the DISTINCT 16 KiB host pages of the watched region that guest
/// writes touch between two dispatch boundaries. That count is the number of
/// write faults an `mprotect` write barrier would take per boundary, which is
/// the only unknown in the fault-vs-scan comparison. Enabled by
/// `FN64_MPROTECT_CENSUS=1`; inert otherwise.
pub mod mprotect_census {
    use std::cell::RefCell;

    /// Apple Silicon page size; the granule an `mprotect` barrier would use.
    const PAGE: u32 = 16384;

    thread_local! {
        static PAGES: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    }

    /// (boundaries, total distinct pages, histogram of pages-per-boundary).
    ///
    /// Process-global rather than thread-local so the at-exit report can be
    /// printed from any thread: the counting happens on a coroutine-backed
    /// executor thread whose thread-locals are not torn down at process exit,
    /// so a thread-local total would never be observed.
    static TOTALS: std::sync::Mutex<(u64, u64, Vec<u64>)> =
        std::sync::Mutex::new((0, 0, Vec::new()));

    pub fn enabled() -> bool {
        // Read the environment once. `note_write` runs on every guest store, so
        // a `getenv` here is itself visible in the profile.
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("FN64_MPROTECT_CENSUS").is_some())
    }

    /// Record that `[offset, offset+len)` was written.
    pub(super) fn note_write(offset: u32, len: u32) {
        if !enabled() {
            return;
        }
        let first = offset / PAGE;
        let last = offset.saturating_add(len.saturating_sub(1)) / PAGE;
        PAGES.with(|pages| {
            let mut pages = pages.borrow_mut();
            for page in first..=last {
                if !pages.contains(&page) {
                    pages.push(page);
                }
            }
        });
    }

    /// Close one dispatch boundary and fold its page count into the totals.
    ///
    /// Also arms the at-exit report on first use, so the census needs no edit
    /// to any harness `main` -- notably not
    /// `examples/wm2000-block-boot/src/main.rs`, whose bytes are hashed into
    /// `DISPATCH_SOURCE_SHA256` (`build.rs:794`) and therefore into the
    /// canonical program identity. Printing from here keeps the measured
    /// program byte-identical to the unmeasured one.
    pub fn note_boundary() {
        if !enabled() {
            return;
        }
        arm_report();
        let count = PAGES.with(|pages| {
            let mut pages = pages.borrow_mut();
            let count = pages.len();
            pages.clear();
            count
        });
        {
            let mut totals = TOTALS.lock().expect("mprotect census totals poisoned");
            let (boundaries, total, histogram) = &mut *totals;
            *boundaries += 1;
            *total += count as u64;
            if histogram.len() <= count {
                histogram.resize(count + 1, 0);
            }
            histogram[count] += 1;
        }
    }

    /// Register the at-exit report exactly once.
    ///
    /// `atexit` rather than a `Drop` guard: the counting runs on a
    /// coroutine-backed executor thread that is not joined at process exit, so
    /// no destructor of its would run. The totals are process-global, so the
    /// handler can print them from whichever thread calls `exit`.
    fn arm_report() {
        extern "C" fn at_exit() {
            print!("{}", report());
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        static ARMED: std::sync::Once = std::sync::Once::new();
        ARMED.call_once(|| {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> i32;
            }
            unsafe { atexit(at_exit) };
        });
    }

    /// One line per bucket, plus the mean. Printed by the at-exit hook.
    pub fn report() -> String {
        {
            let totals = TOTALS.lock().expect("mprotect census totals poisoned");
            let (boundaries, total, histogram) = &*totals;
            let mean = if *boundaries == 0 {
                0.0
            } else {
                *total as f64 / *boundaries as f64
            };
            let mut out = format!(
                "[mprotect-census] boundaries={boundaries} distinct_pages_total={total} \
                 mean_pages_per_boundary={mean:.4}\n"
            );
            for (count, hits) in histogram.iter().enumerate() {
                if *hits == 0 {
                    continue;
                }
                let share = 100.0 * *hits as f64 / *boundaries.max(&1) as f64;
                out.push_str(&format!(
                    "[mprotect-census]   {count:>4} page(s): {hits:>10} boundaries ({share:5.2}%)\n"
                ));
            }
            out
        }
    }
}

/// TEMPORARY (generation-activation census, 2026-08-07).
///
/// Counts physically backed catalog activations per generation, splitting
/// first-time activations from re-activations of an already-live image. A
/// re-activation re-reads and re-hashes the generation's whole image through
/// its backing, so `reactivated * bytes` is the SHA-256 volume the route pays
/// purely to re-prove images it already proved.
///
/// This exists because the WM2000 "entrance hang" profiles as 88%
/// `activate_for_fetch` / 54% raw SHA-256: the question is whether that is one
/// expensive activation or millions of cheap-looking repeats.
///
/// Enabled by `FN64_ACTIVATION_CENSUS=1`; inert otherwise. It installs itself
/// through the public `set_backed_generation_activation_observer_v1` seam and
/// prints from `atexit`, so no harness `main` is edited -- notably not
/// `examples/wm2000-block-boot/src/main.rs`, whose bytes are hashed into the
/// canonical program identity.
pub mod activation_census {
    /// generation id -> (activations, reactivations, unused, retired count)
    static TOTALS: std::sync::Mutex<Option<std::collections::BTreeMap<u64, [u64; 4]>>> =
        std::sync::Mutex::new(None);

    /// (selected generation, requested pc) -> activations. Answers "is one PC
    /// alternating between two generations, or are two PCs each stable?" --
    /// the question that separates a genuine A/B overlay swap from a
    /// retirement artifact.
    #[allow(clippy::type_complexity)]
    static BY_PC: std::sync::Mutex<Option<std::collections::BTreeMap<(u64, u32), u64>>> =
        std::sync::Mutex::new(None);

    /// (activated generation, retired generation) -> count.
    #[allow(clippy::type_complexity)]
    static RETIREMENTS: std::sync::Mutex<Option<std::collections::BTreeMap<(u64, u64), u64>>> =
        std::sync::Mutex::new(None);

    pub fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| std::env::var_os("FN64_ACTIVATION_CENSUS").is_some())
    }

    fn observe(observation: &fn64_cpu_runtime::BackedGenerationActivationObservationV1) {
        let mut guard = TOTALS.lock().expect("activation census poisoned");
        let totals = guard.get_or_insert_with(std::collections::BTreeMap::new);
        let slot = totals.entry(observation.generation.get()).or_insert([0; 4]);
        slot[0] += 1;
        if !observation.newly_activated {
            slot[1] += 1;
        }
        slot[3] += observation.retired.len() as u64;
        drop(guard);

        let mut guard = BY_PC.lock().expect("activation census poisoned");
        let by_pc = guard.get_or_insert_with(std::collections::BTreeMap::new);
        *by_pc
            .entry((
                observation.generation.get(),
                observation.requested_pc.get(),
            ))
            .or_insert(0) += 1;
        drop(guard);

        let mut guard = RETIREMENTS.lock().expect("activation census poisoned");
        let retirements = guard.get_or_insert_with(std::collections::BTreeMap::new);
        for retired in &observation.retired {
            *retirements
                .entry((observation.generation.get(), retired.get()))
                .or_insert(0) += 1;
        }
    }

    /// Install the observer and arm the at-exit report. Idempotent.
    pub fn install() {
        if !enabled() {
            return;
        }
        fn64_cpu_runtime::set_backed_generation_activation_observer_v1(Some(observe));
        extern "C" fn at_exit() {
            print!("{}", report());
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        static ARMED: std::sync::Once = std::sync::Once::new();
        ARMED.call_once(|| {
            extern "C" {
                fn atexit(f: extern "C" fn()) -> i32;
            }
            unsafe { atexit(at_exit) };
        });
    }

    pub fn report() -> String {
        let guard = TOTALS.lock().expect("activation census poisoned");
        let Some(totals) = guard.as_ref() else {
            return String::from("[activation-census] no activations observed\n");
        };
        let mut out = String::new();
        let mut all = 0u64;
        let mut re = 0u64;
        for (generation, [activations, reactivations, _, retired]) in totals {
            all += *activations;
            re += *reactivations;
            out.push_str(&format!(
                "[activation-census] generation={generation} activations={activations} \
                 reactivations={reactivations} retired_others={retired}\n"
            ));
        }
        out.push_str(&format!(
            "[activation-census] total_activations={all} total_reactivations={re}\n"
        ));
        drop(guard);

        let guard = RETIREMENTS.lock().expect("activation census poisoned");
        if let Some(retirements) = guard.as_ref() {
            for ((activated, retired), count) in retirements {
                out.push_str(&format!(
                    "[activation-census] retire activated={activated} retired={retired} \
                     count={count}\n"
                ));
            }
        }
        drop(guard);

        let guard = BY_PC.lock().expect("activation census poisoned");
        if let Some(by_pc) = guard.as_ref() {
            let mut rows = by_pc.iter().collect::<Vec<_>>();
            rows.sort_unstable_by_key(|(_, count)| std::cmp::Reverse(**count));
            for ((generation, pc), count) in rows.into_iter().take(24) {
                out.push_str(&format!(
                    "[activation-census] pc generation={generation} pc=0x{pc:08x} \
                     activations={count}\n"
                ));
            }
        }
        out
    }
}

pub(super) fn record_executable_and_renderer_write(event: GuestWriteEvent) {
    let (offset, len) = event.range();
    mprotect_census::note_write(offset, len);
    if event.channel() == WriterChannel::CpuInstructionStore {
        CPU_INSTRUCTION_STORE_TRACE.with(|trace| {
            if let Some(trace) = trace.borrow_mut().as_mut() {
                trace.events.push((offset, len));
            }
        });
    }
    let end = offset.saturating_add(len);
    let intersects_executable = EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges
            .borrow()
            .iter()
            .any(|&(physical_start, physical_end)| offset < physical_end && end > physical_start)
    });
    PENDING_EXECUTABLE_WRITES.with(|writes| writes.borrow_mut().push((offset, len)));
    if intersects_executable {
        PENDING_ATTRIBUTED_EXECUTABLE_WRITES.with(|writes| writes.borrow_mut().push(event));
    }
    observe_renderer_write(event);
}

/// Whether one committed guest write must break the current block.
///
/// This is separate from [`record_executable_and_renderer_write`] above, which
/// feeds `PENDING_ATTRIBUTED_EXECUTABLE_WRITES` and the journal. **Attribution
/// stays wide**: narrowing it is what produced the `events=0 declarations=0`
/// bug documented at the fallback in `track_catalog_nested_mutation` below.
/// Only the boundary narrows here.
///
/// A write is a boundary when it lands in the watched executable region AND
/// some generation backed by those bytes is currently resident. A write to
/// bytes no resident generation backs cannot invalidate a live translation, so
/// the block chains on.
///
/// # Why the un-resident case is safe
///
/// The obvious counterexample -- write bytes while nothing is resident, then
/// activate a generation over them and execute stale code -- cannot happen.
/// `activate_for_fetch_with_digest`
/// (`fn64-cpu-runtime` `generation/mod.rs:771`) computes `live_digest` from LIVE
/// memory and compares it against `expected_sha256` for every containing
/// candidate **unconditionally and before** consulting `self.active`; the
/// `already_active` short-circuit happens after that loop. So a later
/// activation over bytes changed earlier re-digests the changed bytes and
/// returns `AotMiss`/`NoGenerationMatched` instead of activating.
///
/// `guest_write_token` would be the way to cache this, and it has no non-test
/// consumers, so no activation path bypasses the digest.
/// Kill switch restoring the pre-residency behaviour: every watched-region
/// write breaks the block. This is the A/B control the speedup is measured
/// against, and an escape hatch if the predicate is ever suspected.
fn resident_boundary_disabled() -> bool {
    use std::sync::OnceLock;
    static DISABLED: OnceLock<bool> = OnceLock::new();
    // Read once. This is consulted on every watched store, so an uncached
    // `env::var_os` would scan the environment millions of times per route.
    *DISABLED.get_or_init(|| std::env::var_os("FN64_DISABLE_RESIDENT_BOUNDARY").is_some())
}

pub(super) fn classify_live_executable_write(event: GuestWriteEvent) -> GuestWriteBoundary {
    let (start, len) = event.range();
    let end = start.saturating_add(len);
    if !EXECUTABLE_WRITE_RANGES.with(|ranges| {
        ranges
            .borrow()
            .iter()
            .any(|&(physical_start, physical_end)| start < physical_end && end > physical_start)
    }) {
        return GuestWriteBoundary::Continue;
    }
    if resident_boundary_disabled() {
        return GuestWriteBoundary::ExecutableChanged;
    }
    // Unanswerable means "assume resident", because a permissive answer would
    // let stale translated code execute. There are three ways to be
    // unanswerable, and all three must break the block:
    //
    //  - `HOST` is already borrowed. This is REACHED, not theoretical:
    //    `advance_device_time_step` issues device writes from inside its own
    //    `with_host` closure, so `with_host` here would be a nested
    //    `borrow_mut` and a hard abort. Hence `try_with_host`.
    //  - no canonical program is installed.
    //  - the generation catalog is itself already borrowed.
    // Answer the question INSIDE the closure, against a borrow. The previous
    // form cloned the program out of `HOST` first, and
    // `CanonicalLiveBlockProgramV1` is `Clone` but not free: every field is an
    // `Rc` except `bootstrap_evidence`, which is an owned
    // `BootstrapOrImportValidationEvidenceV1` that deep-clones. On a path that
    // runs on EVERY store into the watched region that allocation-and-free pair
    // was 4.17% of total runtime, and the clone was never used for anything but
    // calling one `&self` method.
    //
    // This holds `HOST` while `generations.try_borrow()` runs, which is a
    // NARROWER window than before -- the clone already ran under the same
    // borrow -- and the inner borrow stays `try_borrow`, so a contended catalog
    // still resolves conservatively rather than panicking.
    let resident = crate::try_with_host(|host| {
        host.canonical_recompiled_program
            .as_ref()
            .and_then(|live| live.resident_backing_intersects(start, end))
    })
    .flatten()
    .unwrap_or(true);
    if resident {
        GuestWriteBoundary::ExecutableChanged
    } else {
        GuestWriteBoundary::Continue
    }
}

/// Run one synchronous renderer publication against live RDRAM and attribute
/// every changed executable byte to the renderer channel before the guest can
/// resume. The snapshot is limited to the sealed ever-admissible backing
/// union; ordinary framebuffer writes outside that union incur no journal
/// storage.
fn track_catalog_nested_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
    notify: impl Fn(u32, u32),
) -> R {
    let transaction = begin_catalog_nested_writer(rdram, "tracked renderer/RSP publication");
    if transaction.is_canonical() {
        let result = operation(rdram);
        transaction.commit_changed_bytes(rdram, notify);
        return result;
    }
    // Watch the ranges the GUARD will later check, not just this thread's
    // registered write ranges.
    //
    // The canonical branch above diffs `mutation_state`'s watched set -- the
    // same set `commit_snapshot` compares against when it decides a byte was
    // undeclared. This branch used `EXECUTABLE_WRITE_RANGES`, a different set,
    // so a renderer write landing on executable bytes outside it was never
    // compared here and never notified. It then surfaced at the next commit as
    // an undeclared mutation with `events=0 declarations=0`, because nothing
    // had declared it -- WM2000 patching a store's immediate at 0x8009b0b0
    // during a graphics task is exactly that case.
    //
    // Falling back to the thread-local set keeps behavior unchanged when no
    // canonical program is installed, which is the only situation it was ever
    // the right answer for.
    let ranges = with_host(|host| host.canonical_recompiled_program.clone())
        .and_then(|live| {
            live.mutation_state
                .as_ref()
                .map(|state| state.borrow().watched_ranges())
        })
        .unwrap_or_else(|| EXECUTABLE_WRITE_RANGES.with(|ranges| ranges.borrow().clone()));
    let before = {
        let view = fn64_runtime::RdramView::from_storage(rdram);
        ranges
            .iter()
            .map(|&(physical_start, physical_end)| {
                assert!(
                    physical_end as usize <= view.len(),
                    "renderer mutation tracker range [{physical_start:#010x}, {physical_end:#010x}) exceeds live RDRAM {:#x}",
                    view.len()
                );
                (physical_start..physical_end)
                    .map(|physical| {
                        view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let result = operation(rdram);
    let view = fn64_runtime::RdramView::from_storage(rdram);
    for (&(physical_start, physical_end), before) in ranges.iter().zip(before) {
        let mut physical = physical_start;
        while physical < physical_end {
            let before_index = (physical - physical_start) as usize;
            if before[before_index] == view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            {
                physical += 1;
                continue;
            }
            let changed_start = physical;
            physical += 1;
            while physical < physical_end
                && before[(physical - physical_start) as usize]
                    != view.read_u8(fn64_runtime::RdramAddr::from_offset(physical))
            {
                physical += 1;
            }
            notify(changed_start, physical - changed_start);
        }
    }
    transaction.commit(rdram);
    result
}

pub(crate) fn track_rdp_renderer_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> R {
    track_catalog_nested_mutation(rdram, operation, fn64_cpu_runtime::notify_rdp_renderer_write)
}

/// Record one renderer operation whose backend contract has crossed a commit
/// boundary. Mutation tracking and this lifecycle mark are deliberately
/// separate: a `NeedsLle` operation does not become a successful publication,
/// and any executable journal sequence it produced invalidates the epoch.
pub(crate) fn record_rdp_renderer_publication_v1() {
    finish_rdp_renderer_operation_v1(true);
}

pub(crate) fn record_rdp_renderer_rejection_v1() {
    finish_rdp_renderer_operation_v1(false);
}

fn finish_rdp_renderer_operation_v1(committed: bool) {
    RDP_RENDERER_WRITER_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(trace) = trace.as_mut() else {
            return;
        };
        let live = with_host(|host| host.canonical_recompiled_program.clone())
            .expect("armed RDP renderer trace lost its canonical program owner");
        assert_eq!(
            live.rdp_renderer_writer_trace_epoch_id.get(),
            Some(trace.epoch_id),
            "RDP renderer publication crossed trace epoch owners"
        );
        assert_eq!(
            live.writer_program_model_sha256, trace.program_model_sha256,
            "RDP renderer publication crossed canonical program models"
        );
        let state = live
            .mutation_state
            .as_ref()
            .expect("armed RDP renderer trace lost its mutation journal")
            .borrow();
        assert!(
            trace.next_journal_entry_index <= state.entries.len(),
            "RDP renderer trace journal cursor exceeds the canonical journal"
        );
        let sequences = state.entries[trace.next_journal_entry_index..]
            .iter()
            .filter(|entry| {
                entry
                    .declared_writes
                    .iter()
                    .any(|declaration| declaration.channel == WriterChannel::RdpRenderer)
            })
            .map(|entry| entry.sequence)
            .collect();
        trace.next_journal_entry_index = state.entries.len();
        if committed {
            trace.publications.push(sequences);
        } else {
            trace.rejected_journal_sequences.extend(sequences);
        }
    });
}

pub(crate) fn track_rsp_execution_or_hle_mutation<R>(
    rdram: &mut [u8],
    operation: impl FnOnce(&mut [u8]) -> R,
) -> (R, Vec<u64>) {
    let live = with_host(|host| host.canonical_recompiled_program.clone())
        .expect("tracked RSP/HLE publication lost its canonical program owner");
    let state = live
        .mutation_state
        .as_ref()
        .expect("tracked RSP/HLE publication lost its mutation journal");
    let initial_entry_count = state.borrow().entries.len();
    let result = track_catalog_nested_mutation(
        rdram,
        operation,
        fn64_cpu_runtime::notify_rsp_execution_or_hle_writeback,
    );
    let journal_sequences = state.borrow().entries[initial_entry_count..]
        .iter()
        .filter(|entry| {
            entry
                .declared_writes
                .iter()
                .any(|declaration| declaration.channel == WriterChannel::RspExecutionOrHleWriteback)
        })
        .map(|entry| entry.sequence)
        .collect();
    (result, journal_sequences)
}

pub(super) fn process_executable_writes(
    live: &LiveBlockProgram,
    mut read_logical_byte: impl FnMut(u32) -> u8,
) -> Vec<BankId> {
    let writes =
        PENDING_EXECUTABLE_WRITES.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if writes.is_empty() {
        return Vec::new();
    }
    let mut regions = live.executable_regions.borrow_mut();
    let deferred = writes
        .iter()
        .flat_map(|&(start, len)| {
            let end = start.saturating_add(len);
            regions.iter().filter_map(move |observed| {
                if observed.activation != ExecutableActivation::FetchBoundary {
                    return None;
                }
                let deferred_start = start.max(observed.physical_start);
                let deferred_end = end.min(observed.physical_end);
                (deferred_start < deferred_end)
                    .then(|| (deferred_start, deferred_end - deferred_start))
            })
        })
        .collect::<Vec<_>>();
    let mut program = live.program.borrow_mut();
    let mut retired = Vec::new();
    for observed in regions.iter_mut() {
        if observed.activation != ExecutableActivation::EagerPublication {
            continue;
        }
        let touched = writes.iter().any(|&(start, len)| {
            let end = start.saturating_add(len);
            start < observed.physical_end && end > observed.physical_start
        });
        if !touched {
            continue;
        }
        let bytes = (observed.physical_start..observed.physical_end)
            .map(&mut read_logical_byte)
            .collect::<Vec<_>>();
        let generation = observed.next_generation;
        let (code, runner) = (observed.builder)(&bytes, generation).unwrap_or_else(|error| {
            panic!(
                "executable rewrite [{:#010x}, {:#010x}) generation {generation} could not be translated: {error}",
                observed.physical_start, observed.physical_end
            )
        });
        if let Some(previous) = observed
            .region
            .install(&mut program, code, runner)
            .unwrap_or_else(|error| panic!("executable generation install failed: {error}"))
        {
            retired.push(previous);
        }
        observed.next_generation = observed
            .next_generation
            .checked_add(1)
            .expect("executable generation counter overflow");
    }
    PENDING_EXECUTABLE_WRITES.with(|pending| pending.borrow_mut().extend(deferred));
    retired
}

pub(super) fn activate_fetch_generation(
    live: &LiveBlockProgram,
    at: ExecutionKey,
    miss: AotMiss,
    mut read_logical_byte: impl FnMut(u32) -> u8,
) -> Result<ExecutionKey, String> {
    if let Some(catalog) = live.precompiled_generations.borrow_mut().as_mut() {
        return catalog
            .activate_for_fetch_with(at.pc, |vaddr| read_logical_byte(vaddr & 0x1fff_ffff))
            .map(|resolution| resolution.entry)
            .map_err(|error| format!("{miss}; closed AOT pack selection failed: {error}"));
    }
    let mut regions = live.executable_regions.borrow_mut();
    let observed = regions
        .iter_mut()
        .find(|observed| {
            observed.activation == ExecutableActivation::FetchBoundary
                && observed.region.start() == miss.va_start
                && observed.region.end().get() == miss.va_start.get() + miss.byte_len
        })
        .ok_or_else(|| format!("{miss}; no fetch-activated region owns the attempted range"))?;
    if observed.region.active_bank() != Some(miss.expected_bank) {
        return Err(format!(
            "{miss}; active generation changed before fetch activation"
        ));
    }
    let bytes = (observed.physical_start..observed.physical_end)
        .map(&mut read_logical_byte)
        .collect::<Vec<_>>();
    let generation = observed.next_generation;
    let (code, runner) = (observed.builder)(&bytes, generation).map_err(|error| {
        format!("{miss}; no precompiled generation matches the completed image: {error}")
    })?;
    observed
        .region
        .install(&mut live.program.borrow_mut(), code, runner)
        .map_err(|error| format!("fetch-activated generation install failed: {error}"))?;
    observed.next_generation = observed
        .next_generation
        .checked_add(1)
        .ok_or_else(|| "fetch-activated generation counter overflow".to_string())?;
    PENDING_EXECUTABLE_WRITES.with(|pending| {
        pending.borrow_mut().retain(|&(start, len)| {
            let end = start.saturating_add(len);
            end <= observed.physical_start || start >= observed.physical_end
        });
    });
    observed
        .region
        .resolve(at.pc)
        .ok_or_else(|| format!("fetch-activated region does not contain retry PC {}", at.pc))
}
