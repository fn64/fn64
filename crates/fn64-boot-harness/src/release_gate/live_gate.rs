#![allow(clippy::module_inception)]
use super::*;

/// Opt-in production seam around [`FixedCycleDigestGate`].
///
/// Arming is allowed only before guest time or trace events exist. The
/// committed VI boundary freezes the ABI's memory/audio/device/trace sources;
/// finishing consumes those owned observations. The boot host supplies only
/// typed presentation evidence, which the reference path cross-checks against
/// the frozen physical-RDRAM image.
pub struct LiveReleaseGate {
    guest_cycle: u64,
    armed: bool,
}

pub(crate) struct LiveObservedArtifacts<'a> {
    pub(crate) framebuffer_artifact_bytes: &'a [u8],
    pub(crate) framebuffer_payload_bytes: usize,
    pub(crate) observations: ReleaseObservationGeometry,
}

impl LiveReleaseGate {
    pub const fn new(guest_cycle: u64) -> Self {
        Self {
            guest_cycle,
            armed: false,
        }
    }

    pub const fn guest_cycle(&self) -> u64 {
        self.guest_cycle
    }

    /// Enable every diagnostic channel before boot. Existing guest time or
    /// trace events are rejected rather than silently entering the digest.
    pub fn arm(&mut self) -> Result<(), GateError> {
        self.arm_inner(None, None)
    }

    /// Arm the gate and a crash-flushed unsupported-event journal. The
    /// journal's armed header without its completion record is explicit early
    /// termination evidence; it must not be interpreted as zero events.
    pub fn arm_with_unsupported_journal(
        &mut self,
        journal_path: impl AsRef<Path>,
        run_event_sha256: &str,
    ) -> Result<(), GateError> {
        self.arm_inner(Some(journal_path.as_ref()), Some(run_event_sha256))
    }

    fn arm_inner(
        &mut self,
        journal_path: Option<&Path>,
        run_event_sha256: Option<&str>,
    ) -> Result<(), GateError> {
        let sim_time = fn64_abi::sim_time();
        let trace_events = fn64_abi::copy_trace().len();
        let device_trace_events = fn64_abi::copy_device_trace().len();
        let save_operation_events = fn64_abi::copy_save_operations().len();
        let controller_operation_events = fn64_abi::copy_controller_operations().len();
        let rsp_rdp_observations = fn64_abi::copy_rsp_rdp_observations().len();
        let native_execution_destination_events =
            fn64_abi::copy_native_execution_destinations().len();
        #[cfg(feature = "recomp-rs")]
        let function_execution_destination_events =
            fn64_abi::recompiled::copy_function_execution_destinations().len();
        #[cfg(not(feature = "recomp-rs"))]
        let function_execution_destination_events = 0;
        #[cfg(feature = "recomp-rs")]
        let block_execution_destination_events =
            fn64_abi::recompiled::copy_block_execution_destinations().len();
        #[cfg(not(feature = "recomp-rs"))]
        let block_execution_destination_events = 0;
        if sim_time != 0
            || trace_events != 0
            || device_trace_events != 0
            || save_operation_events != 0
            || controller_operation_events != 0
            || rsp_rdp_observations != 0
            || native_execution_destination_events != 0
            || function_execution_destination_events != 0
            || block_execution_destination_events != 0
        {
            return Err(GateError::LiveGateArmedLate {
                sim_time,
                trace_events,
                device_trace_events,
                save_operation_events,
                controller_operation_events,
                rsp_rdp_observations,
                native_execution_destination_events,
                function_execution_destination_events,
                block_execution_destination_events,
            });
        }
        match run_event_sha256 {
            Some(run_event_sha256) => fn64_runtime::arm_unsupported_events_with_run_identity(
                journal_path,
                run_event_sha256,
            ),
            None => fn64_runtime::arm_unsupported_events(journal_path),
        }
        .map_err(GateError::ArmUnsupportedJournal)?;
        fn64_abi::set_trace_enabled(true);
        fn64_abi::set_audio_digest_capture(true);
        self.armed = true;
        Ok(())
    }

    /// Capture all live channels at an unconsumed, device-scheduled VI edge,
    /// write the report even when closure is incomplete, and only then enforce
    /// minimum-scenario closure. The opaque boundary prevents another caller
    /// from certifying an instruction-checkpoint or stale post-resume cycle.
    pub(crate) fn capture_and_write_observed(
        self,
        boundary: crate::CommittedViBoundary,
        scenario: impl Into<String>,
        input_bytes: &[u8],
        rom_class: Option<ReleaseRomClass>,
        observed: LiveObservedArtifacts<'_>,
        report_path: impl AsRef<Path>,
    ) -> Result<ReleaseGateReport, GateError> {
        if !self.armed {
            return Err(GateError::LiveGateNotArmed);
        }
        if boundary.cycle() != self.guest_cycle {
            return Err(GateError::WrongLiveCycle {
                expected: self.guest_cycle,
                observed: boundary.cycle(),
            });
        }
        let (
            snapshot,
            executor,
            host,
            program,
            frozen_destinations,
            frozen_rsp_rdp,
            platform,
            windows_version,
            render,
            fixed_cycle,
        ) = boundary
            .into_evidence()
            .map_err(GateError::InvalidViBoundary)?;
        let execution_destinations =
            capture_execution_destinations(&program, frozen_destinations, self.guest_cycle)?;
        let rsp_rdp = capture_rsp_rdp_evidence(frozen_rsp_rdp)?;
        let device_tv_type = snapshot
            .guest
            .tv_type
            .ok_or(GateError::MissingDeviceTvType)?;
        let renderer_tv_type = render
            .renderer_tv_type()
            .ok_or(GateError::UnidentifiedRenderBackend)?;
        if renderer_tv_type != device_tv_type {
            return Err(GateError::RomTvTypeMismatch {
                authority: "renderer create-time configuration",
                expected: device_tv_type.into(),
                observed: renderer_tv_type.into(),
            });
        }
        validate_installed_rom_identity(&host, input_bytes)?;
        let rom = if let Some(class) = rom_class {
            Some(ReleaseRomEvidence::from_bytes(
                input_bytes,
                class,
                device_tv_type,
            )?)
        } else if has_recognized_rom_magic(input_bytes) {
            Some(ReleaseRomEvidence::from_bytes(
                input_bytes,
                ReleaseRomClass::Unclassified,
                device_tv_type,
            )?)
        } else {
            None
        };
        let observed_cycle = fn64_abi::sim_time();
        if observed_cycle != self.guest_cycle {
            return Err(GateError::WrongLiveCycle {
                expected: self.guest_cycle,
                observed: observed_cycle,
            });
        }
        let memory_bytes = require_boundary_physical_rdram(fixed_cycle.physical_rdram_logical)?;
        observed
            .observations
            .validate_payload_lengths(observed.framebuffer_payload_bytes, memory_bytes.len())
            .map_err(GateError::InvalidObservationGeometry)?;
        let environment = environment_from_frozen(platform, windows_version, &host, render)?;
        validate_environment_observation(&environment, &observed.observations)?;
        let audio_bytes = fixed_cycle
            .audio_pcm_s16le
            .ok_or(GateError::AudioDigestCaptureNotArmed)?;
        let trace = fixed_cycle.trace;
        let device_trace = fixed_cycle.device_trace;
        let save_operations = fixed_cycle.save_operations;
        let controller_operations = fixed_cycle.controller_operations;
        let unsupported_events = fixed_cycle.unsupported_events;
        validate_reference_framebuffer_against_memory(
            &observed.observations,
            observed.framebuffer_artifact_bytes,
            &memory_bytes,
        )?;
        if let Some(event) = save_operations
            .iter()
            .find(|event| event.at.get() > observed_cycle)
        {
            return Err(GateError::FutureSaveOperationEvent {
                gate_cycle: observed_cycle,
                event_cycle: event.at.get(),
            });
        }
        validate_controller_operation_cycles(observed_cycle, &controller_operations)?;
        if let Some(event) = unsupported_events.iter().find(|event| {
            event
                .guest_cycle
                .is_some_and(|cycle| cycle.get() > observed_cycle)
        }) {
            return Err(GateError::FutureUnsupportedEvent {
                gate_cycle: observed_cycle,
                event_cycle: event.guest_cycle.expect("matched Some cycle").get(),
                operation: event.operation.clone(),
            });
        }

        let mut digest = FixedCycleDigestGate::new(self.guest_cycle);
        digest.capture(
            observed_cycle,
            ArtifactKind::Framebuffer,
            observed.framebuffer_artifact_bytes,
        )?;
        digest.capture(observed_cycle, ArtifactKind::Audio, &audio_bytes)?;
        digest.capture(observed_cycle, ArtifactKind::Memory, &memory_bytes)?;
        digest.capture_device_snapshot(snapshot, executor, host, program)?;
        digest.capture_live_timing_trace(observed_cycle, &trace, &device_trace)?;

        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: observed.framebuffer_artifact_bytes,
            audio_bytes: &audio_bytes,
            memory_bytes: &memory_bytes,
            trace: &trace,
            device_trace: &device_trace,
            save_operations: &save_operations,
            controller_operations: &controller_operations,
            unsupported_events: &unsupported_events,
        })?;
        let report = ReleaseGateReport::new_with_environment(
            scenario,
            input_bytes,
            digest.finish()?,
            ReleaseBoundaryReportEvidence {
                rom,
                observations: observed.observations,
                environment,
                execution_destinations,
                rsp_rdp,
            },
            closure,
        )?;
        report.write_json(report_path)?;
        fn64_runtime::complete_unsupported_observation(
            fn64_runtime::Cycles::new(observed_cycle),
            &report.report_sha256,
        );
        report.require_closed()?;
        Ok(report)
    }
}

pub(super) fn require_boundary_physical_rdram(memory_bytes: Option<Vec<u8>>) -> Result<Vec<u8>, GateError> {
    memory_bytes.ok_or(GateError::BoundaryPhysicalRdramUnavailable)
}

pub(super) fn validate_reference_framebuffer_against_memory(
    observations: &ReleaseObservationGeometry,
    framebuffer_bytes: &[u8],
    memory_bytes: &[u8],
) -> Result<(), GateError> {
    let FramebufferObservationSource::PhysicalRdram { address } = &observations.framebuffer.source
    else {
        return Ok(());
    };
    let address = *address;
    let start = usize::try_from(address).expect("physical framebuffer address exceeds usize");
    let end = start.checked_add(framebuffer_bytes.len()).ok_or(
        GateError::ReferenceFramebufferOutsideFrozenMemory {
            address,
            bytes: framebuffer_bytes.len() as u64,
        },
    )?;
    let frozen =
        memory_bytes
            .get(start..end)
            .ok_or(GateError::ReferenceFramebufferOutsideFrozenMemory {
                address,
                bytes: framebuffer_bytes.len() as u64,
            })?;
    if frozen != framebuffer_bytes {
        return Err(GateError::ReferenceFramebufferDoesNotMatchFrozenMemory {
            address,
            bytes: framebuffer_bytes.len() as u64,
        });
    }
    Ok(())
}

pub(super) fn validate_controller_operation_cycles(
    gate_cycle: u64,
    operations: &[ControllerOperationEvent],
) -> Result<(), GateError> {
    if let Some(event) = operations.iter().find(|event| event.at.get() > gate_cycle) {
        Err(GateError::FutureControllerOperationEvent {
            gate_cycle,
            event_cycle: event.at.get(),
            port: event.port,
        })
    } else {
        Ok(())
    }
}

pub(super) fn capture_rsp_rdp_evidence(
    frozen: Vec<fn64_abi::RspRdpObservationEvent>,
) -> Result<RspRdpEvidence, GateError> {
    let ordered = frozen
        .into_iter()
        .map(|event| RspRdpObservationEventEvidence {
            guest_cycle: event.at.get(),
            observation: match event.kind {
                fn64_abi::RspRdpObservationKind::MicrocodeRecognition {
                    task_addr,
                    imem_generation,
                    text_sha256,
                    data_addr,
                    data_size,
                    data_sha256,
                    family,
                } => RspRdpObservationKindEvidence::MicrocodeRecognition {
                    task_address: task_addr.offset(),
                    imem_generation,
                    text_sha256: hex(&text_sha256),
                    data_address: data_addr.offset(),
                    data_bytes: data_size,
                    data_sha256: hex(&data_sha256),
                    family: family.map(release_microcode_family),
                },
                fn64_abi::RspRdpObservationKind::DramDpcCommitted {
                    start,
                    end,
                    command_sha256,
                } => RspRdpObservationKindEvidence::DramDpcCommitted {
                    start,
                    end,
                    command_sha256: hex(&command_sha256),
                },
                fn64_abi::RspRdpObservationKind::XbusDpcCommitted {
                    start,
                    end,
                    command_sha256,
                } => RspRdpObservationKindEvidence::XbusDpcCommitted {
                    start,
                    end,
                    command_sha256: hex(&command_sha256),
                },
                fn64_abi::RspRdpObservationKind::ImemReplacementCommitted {
                    task_addr,
                    imem_generation,
                    text_sha256,
                } => RspRdpObservationKindEvidence::ImemReplacementCommitted {
                    task_address: task_addr.offset(),
                    imem_generation,
                    text_sha256: hex(&text_sha256),
                },
            },
        })
        .collect();
    RspRdpEvidence::from_ordered(ordered)
}

pub(super) const fn release_microcode_family(family: fn64_abi::UcodeId) -> ReleaseMicrocodeFamily {
    match family {
        fn64_abi::UcodeId::Fast3d => ReleaseMicrocodeFamily::Fast3d,
        fn64_abi::UcodeId::F3dex => ReleaseMicrocodeFamily::F3dex,
        fn64_abi::UcodeId::F3dlx => ReleaseMicrocodeFamily::F3dlx,
        fn64_abi::UcodeId::F3dlxRej => ReleaseMicrocodeFamily::F3dlxRej,
        fn64_abi::UcodeId::F3dex2 => ReleaseMicrocodeFamily::F3dex2,
        fn64_abi::UcodeId::F3dex2NoN => ReleaseMicrocodeFamily::F3dex2NoN,
        fn64_abi::UcodeId::F3dex2Rej => ReleaseMicrocodeFamily::F3dex2Rej,
        fn64_abi::UcodeId::F3dlx2Rej => ReleaseMicrocodeFamily::F3dlx2Rej,
        fn64_abi::UcodeId::F3dzex2 => ReleaseMicrocodeFamily::F3dzex2,
        fn64_abi::UcodeId::S2dex => ReleaseMicrocodeFamily::S2dex,
        fn64_abi::UcodeId::S2dex2 => ReleaseMicrocodeFamily::S2dex2,
        fn64_abi::UcodeId::L3dex => ReleaseMicrocodeFamily::L3dex,
        fn64_abi::UcodeId::L3dex2 => ReleaseMicrocodeFamily::L3dex2,
        fn64_abi::UcodeId::Other(id) => ReleaseMicrocodeFamily::Other { id },
    }
}

pub(super) fn capture_execution_destinations(
    program: &crate::ProgramEvidenceSnapshot,
    frozen: crate::FrozenExecutionDestinations,
    gate_cycle: u64,
) -> Result<ExecutionDestinationEvidence, GateError> {
    #[cfg(feature = "recomp-rs")]
    let function_is_empty = frozen.function.is_empty();
    #[cfg(not(feature = "recomp-rs"))]
    let function_is_empty = true;
    #[cfg(feature = "recomp-rs")]
    let block_is_empty = frozen.block.is_empty();
    #[cfg(not(feature = "recomp-rs"))]
    let block_is_empty = true;

    let (source, ordered) = match program {
        crate::ProgramEvidenceSnapshot::NoProgram => {
            if !frozen.native.is_empty() || !function_is_empty || !block_is_empty {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "NoProgram boundary contains entered executable destinations",
                ));
            }
            (ExecutionDestinationSource::NoProgram, Vec::new())
        }
        crate::ProgramEvidenceSnapshot::UnidentifiedNativeProgram => {
            return Err(GateError::UnidentifiedNativeProgram);
        }
        crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(identity) => {
            if !function_is_empty || !block_is_empty {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "native archive boundary contains typed-Rust destinations",
                ));
            }
            let mut ordered = Vec::with_capacity(frozen.native.len());
            for event in frozen.native {
                if event.at.get() > gate_cycle {
                    return Err(GateError::FutureExecutionDestinationEvent {
                        gate_cycle,
                        event_cycle: event.at.get(),
                    });
                }
                ordered.push(ExecutionDestinationEventEvidence {
                    guest_cycle: Some(event.at.get()),
                    destination: ReleaseExecutionDestination::Native {
                        section_index: event.destination.section_index,
                        function_offset: event.destination.function_offset,
                        link_vram: event.destination.link_vram,
                    },
                });
            }
            if ordered.is_empty() {
                return Err(GateError::EmptyExecutionDestinationEvidence(
                    "native_archive",
                ));
            }
            (
                ExecutionDestinationSource::NativeArchive {
                    artifact_sha256: hex(&identity.bytes()),
                },
                ordered,
            )
        }
        #[cfg(feature = "recomp-rs")]
        crate::ProgramEvidenceSnapshot::TypedRust(
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Function { identity },
        ) => {
            if !frozen.native.is_empty() || !block_is_empty {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "typed observed-function boundary contains another lane's destinations",
                ));
            }
            let mut ordered = Vec::with_capacity(frozen.function.len());
            for event in frozen.function {
                if event.artifact_identity != identity.identity {
                    return Err(GateError::FunctionDestinationArtifactMismatch {
                        expected: hex(&identity.identity.bytes()),
                        observed: hex(&event.artifact_identity.bytes()),
                        vram: event.function.vram,
                        symbol: event.function.symbol.to_owned(),
                    });
                }
                if event.at.get() > gate_cycle {
                    return Err(GateError::FutureExecutionDestinationEvent {
                        gate_cycle,
                        event_cycle: event.at.get(),
                    });
                }
                if event.function.symbol.is_empty() {
                    return Err(GateError::ExecutionDestinationSourceMismatch(
                        "typed observed-function destination has an empty symbol",
                    ));
                }
                ordered.push(ExecutionDestinationEventEvidence {
                    guest_cycle: Some(event.at.get()),
                    destination: ReleaseExecutionDestination::TypedFunction {
                        vram: event.function.vram,
                        symbol: event.function.symbol.to_owned(),
                    },
                });
            }
            if ordered.is_empty() {
                return Err(GateError::EmptyExecutionDestinationEvidence(
                    "typed_observed_function_program",
                ));
            }
            (
                ExecutionDestinationSource::TypedObservedFunctionProgram {
                    artifact_sha256: hex(&identity.identity.bytes()),
                },
                ordered,
            )
        }
        #[cfg(feature = "recomp-rs")]
        crate::ProgramEvidenceSnapshot::TypedRust(
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Block {
                program,
                dispatch_artifact_identity,
                ..
            },
        ) => {
            if !frozen.native.is_empty() || !function_is_empty {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "typed-block boundary contains another lane's destinations",
                ));
            }
            let mut ordered = Vec::with_capacity(frozen.block.len());
            for event in frozen.block {
                let runner_artifact_identity = event.runner_artifact_identity.ok_or(
                    GateError::UnidentifiedBlockRunnerArtifact {
                        bank: event.destination.bank.get(),
                        pc: event.destination.pc.get(),
                    },
                )?;
                ordered.push(ExecutionDestinationEventEvidence {
                    guest_cycle: None,
                    destination: ReleaseExecutionDestination::TypedBlock {
                        bank: event.destination.bank.get(),
                        pc: event.destination.pc.get(),
                        runner_artifact_sha256: hex(&runner_artifact_identity.bytes()),
                    },
                });
            }
            if ordered.is_empty() {
                return Err(GateError::EmptyExecutionDestinationEvidence(
                    "typed_block_program",
                ));
            }
            (
                ExecutionDestinationSource::TypedBlockProgram {
                    program_sha256: hex(&program.identity.identity.bytes()),
                    dispatch_artifact_sha256: hex(&dispatch_artifact_identity.bytes()),
                },
                ordered,
            )
        }
    };
    ExecutionDestinationEvidence::from_ordered(source, ordered)
}

impl ExecutionDestinationEvidence {
    #[cfg(test)]
    pub(crate) fn no_program() -> Self {
        Self::from_ordered(ExecutionDestinationSource::NoProgram, Vec::new())
            .expect("empty no-program execution evidence is canonical")
    }

    pub(crate) fn from_ordered(
        source: ExecutionDestinationSource,
        ordered: Vec<ExecutionDestinationEventEvidence>,
    ) -> Result<Self, GateError> {
        let mut counts = BTreeMap::<ReleaseExecutionDestination, u64>::new();
        for event in &ordered {
            let count = counts.entry(event.destination.clone()).or_default();
            *count = count
                .checked_add(1)
                .expect("execution destination observation count overflow");
        }
        let unique = counts
            .into_iter()
            .map(
                |(destination, observations)| ExecutionDestinationCountEvidence {
                    destination,
                    observations,
                },
            )
            .collect::<Vec<_>>();
        let total_observations = u64::try_from(ordered.len())
            .expect("execution destination history exceeds evidence wire");
        let unique_destinations =
            u64::try_from(unique.len()).expect("unique destination set exceeds evidence wire");
        let ordered_sha256 = sha256_hex(&encode_ordered_execution_destinations(&ordered)?);
        let unique_sha256 = sha256_hex(&encode_unique_execution_destinations(&unique)?);
        Ok(Self {
            source,
            total_observations,
            unique_destinations,
            ordered_sha256,
            unique_sha256,
            ordered,
            unique,
        })
    }

    pub fn verify_integrity(&self) -> Result<(), GateError> {
        validate_execution_destination_source(&self.source, &self.ordered)?;
        let canonical = Self::from_ordered(self.source.clone(), self.ordered.clone())?;
        if *self == canonical {
            Ok(())
        } else {
            Err(GateError::ExecutionDestinationIntegrityMismatch)
        }
    }
}

pub(super) fn validate_execution_destination_cycles(
    gate_cycle: u64,
    evidence: &ExecutionDestinationEvidence,
) -> Result<(), GateError> {
    if let Some(event_cycle) = evidence
        .ordered
        .iter()
        .filter_map(|event| event.guest_cycle)
        .find(|&event_cycle| event_cycle > gate_cycle)
    {
        Err(GateError::FutureExecutionDestinationEvent {
            gate_cycle,
            event_cycle,
        })
    } else {
        Ok(())
    }
}

pub(super) fn validate_execution_destination_source(
    source: &ExecutionDestinationSource,
    ordered: &[ExecutionDestinationEventEvidence],
) -> Result<(), GateError> {
    match source {
        ExecutionDestinationSource::NoProgram => {
            if !ordered.is_empty() {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "NoProgram evidence has an entered destination",
                ));
            }
        }
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            decode_sha256(artifact_sha256).ok_or(GateError::InvalidReportSha256(
                "execution_destinations.source.artifact_sha256",
            ))?;
            if ordered.is_empty()
                || ordered.iter().any(|event| {
                    event.guest_cycle.is_none()
                        || !matches!(
                            event.destination,
                            ReleaseExecutionDestination::Native { .. }
                        )
                })
            {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "native archive requires one or more cycle-stamped native destinations",
                ));
            }
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            decode_sha256(artifact_sha256).ok_or(GateError::InvalidReportSha256(
                "execution_destinations.source.artifact_sha256",
            ))?;
            if ordered.is_empty()
                || ordered.iter().any(|event| {
                    event.guest_cycle.is_none()
                        || !matches!(
                            &event.destination,
                            ReleaseExecutionDestination::TypedFunction { symbol, .. }
                                if !symbol.is_empty()
                        )
                })
            {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "typed observed-function program requires one or more cycle-stamped, named typed-function destinations",
                ));
            }
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            decode_sha256(program_sha256).ok_or(GateError::InvalidReportSha256(
                "execution_destinations.source.program_sha256",
            ))?;
            decode_sha256(dispatch_artifact_sha256).ok_or(GateError::InvalidReportSha256(
                "execution_destinations.source.dispatch_artifact_sha256",
            ))?;
            if ordered.is_empty()
                || ordered.iter().any(|event| {
                    event.guest_cycle.is_some()
                        || !matches!(
                            event.destination,
                            ReleaseExecutionDestination::TypedBlock { .. }
                        )
                })
            {
                return Err(GateError::ExecutionDestinationSourceMismatch(
                    "typed block program requires one or more unstamped typed-block destinations",
                ));
            }
            for event in ordered {
                if let ReleaseExecutionDestination::TypedBlock {
                    runner_artifact_sha256,
                    ..
                } = &event.destination
                {
                    decode_sha256(runner_artifact_sha256).ok_or(GateError::InvalidReportSha256(
                        "execution_destinations.ordered[].runner_artifact_sha256",
                    ))?;
                }
            }
        }
    }
    Ok(())
}

impl FixedCycleDigestGate {
    pub fn new(guest_cycle: u64) -> Self {
        Self {
            guest_cycle,
            artifacts: BTreeMap::new(),
        }
    }

    pub fn capture(
        &mut self,
        observed_cycle: u64,
        kind: ArtifactKind,
        bytes: &[u8],
    ) -> Result<(), GateError> {
        if observed_cycle != self.guest_cycle {
            return Err(GateError::WrongCycle {
                expected: self.guest_cycle,
                observed: observed_cycle,
                kind,
            });
        }
        if self.artifacts.contains_key(&kind) {
            return Err(GateError::DuplicateArtifact(kind));
        }
        self.artifacts.insert(
            kind,
            ArtifactDigest {
                kind,
                bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            },
        );
        Ok(())
    }

    /// Capture the guest-visible device registers in an explicit wire order.
    /// Debug formatting is deliberately excluded from digest evidence.
    pub fn capture_device_snapshot(
        &mut self,
        snapshot: DeviceEvidenceSnapshot,
        executor: fn64_runtime::ExecutorControlEvidenceSnapshot,
        host: fn64_abi::AbiHostEvidenceSnapshot,
        program: crate::ProgramEvidenceSnapshot,
    ) -> Result<(), GateError> {
        let observed_cycle = snapshot.guest.now.get();
        let bytes = try_encode_device_snapshot(snapshot, executor, host, program)?;
        self.capture(observed_cycle, ArtifactKind::DeviceState, &bytes)
    }

    /// Capture the scheduler/device-boundary timing vocabulary in an explicit
    /// wire order, excluding the process-global diagnostic sequence counter.
    pub fn capture_timing_trace(
        &mut self,
        observed_cycle: u64,
        events: &[TraceEvent],
    ) -> Result<(), GateError> {
        if let Some(event) = events.iter().find(|event| event.sim_time > observed_cycle) {
            return Err(GateError::FutureTraceEvent {
                gate_cycle: observed_cycle,
                event_cycle: event.sim_time,
            });
        }
        let bytes = encode_timing_trace(events);
        self.capture(observed_cycle, ArtifactKind::TimingTrace, &bytes)
    }

    /// Capture executor timing plus typed device-fabric DMA transitions. The
    /// DMA substream retains only device-qualified start/commit/completion
    /// variants plus synchronous SP task-load admission in their original
    /// fabric order; unrelated device events do not fabricate DMA evidence or
    /// perturb this artifact.
    pub fn capture_live_timing_trace(
        &mut self,
        observed_cycle: u64,
        events: &[TraceEvent],
        device_events: &[DeviceTraceEvent],
    ) -> Result<(), GateError> {
        if let Some(event) = events.iter().find(|event| event.sim_time > observed_cycle) {
            return Err(GateError::FutureTraceEvent {
                gate_cycle: observed_cycle,
                event_cycle: event.sim_time,
            });
        }
        if let Some(event) = device_events
            .iter()
            .find(|event| event.at.get() > observed_cycle)
        {
            return Err(GateError::FutureDeviceTraceEvent {
                gate_cycle: observed_cycle,
                event_cycle: event.at.get(),
            });
        }
        let mut bytes = Vec::new();
        push_bytes(&mut bytes, b"fn64.live-timing.v2");
        push_bytes(&mut bytes, &encode_timing_trace(events));
        push_bytes(&mut bytes, &encode_device_dma_trace(device_events));
        self.capture(observed_cycle, ArtifactKind::TimingTrace, &bytes)
    }

    pub fn finish(self) -> Result<DeterministicDigest, GateError> {
        let missing: Vec<_> = ArtifactKind::ALL
            .into_iter()
            .filter(|kind| !self.artifacts.contains_key(kind))
            .collect();
        if !missing.is_empty() {
            return Err(GateError::MissingArtifacts(missing));
        }

        let artifacts: Vec<_> = self.artifacts.into_values().collect();
        let root_sha256 = recompute_digest_root(self.guest_cycle, &artifacts)?;
        Ok(DeterministicDigest {
            guest_cycle: self.guest_cycle,
            artifacts,
            root_sha256,
        })
    }
}

impl DeterministicDigest {
    pub fn verify_integrity(&self) -> Result<(), GateError> {
        let observed: Vec<_> = self
            .artifacts
            .iter()
            .map(|artifact| artifact.kind)
            .collect();
        if observed.as_slice() != ArtifactKind::ALL {
            return Err(GateError::InvalidArtifactSet {
                expected: ArtifactKind::ALL.to_vec(),
                observed,
            });
        }
        decode_sha256(&self.root_sha256)
            .ok_or(GateError::InvalidReportSha256("digest.root_sha256"))?;
        let recomputed = recompute_digest_root(self.guest_cycle, &self.artifacts)?;
        if self.root_sha256 == recomputed {
            Ok(())
        } else {
            Err(GateError::DigestRootIntegrityMismatch {
                stored: self.root_sha256.clone(),
                recomputed,
            })
        }
    }
}


#[derive(Default)]
pub struct ClosureGate {
    paths: BTreeMap<String, ClosurePath>,
}

impl ClosureGate {
    pub fn declare(&mut self, name: impl Into<String>) -> Result<(), GateError> {
        let name = name.into();
        if name.is_empty() {
            return Err(GateError::EmptyPathName);
        }
        if self.paths.contains_key(&name) {
            return Err(GateError::DuplicatePath(name));
        }
        self.paths.insert(
            name.clone(),
            ClosurePath {
                name,
                observations: 0,
                status: ClosurePathStatus::Unexercised,
                unsupported: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn observe_supported(&mut self, name: &str) -> Result<(), GateError> {
        self.observe_supported_count(name, 1)
    }

    fn observe_supported_count(&mut self, name: &str, count: u64) -> Result<(), GateError> {
        assert!(count > 0, "closure observation count must be positive");
        let path = self
            .paths
            .get_mut(name)
            .ok_or_else(|| GateError::UndeclaredPath(name.to_owned()))?;
        path.observations = path
            .observations
            .checked_add(count)
            .expect("closure observation count overflow");
        if path.unsupported.is_empty() {
            path.status = ClosurePathStatus::ExercisedZeroUnsupported;
        }
        Ok(())
    }

    /// Record a named unsupported event. The report can still be serialized;
    /// [`ReleaseGateReport::require_closed`] then fails with every event name.
    pub fn observe_unsupported(
        &mut self,
        path_name: &str,
        subsystem: impl Into<String>,
        operation: impl Into<String>,
        context: impl Into<String>,
        guest_cycle: Option<u64>,
        disposition: impl Into<String>,
    ) -> Result<(), GateError> {
        let path = self
            .paths
            .get_mut(path_name)
            .ok_or_else(|| GateError::UndeclaredPath(path_name.to_owned()))?;
        let operation = operation.into();
        if operation.is_empty() {
            return Err(GateError::EmptyUnsupportedName);
        }
        path.observations += 1;
        path.status = ClosurePathStatus::ExercisedUnsupported;
        path.unsupported.push(UnsupportedEvent {
            subsystem: subsystem.into(),
            operation,
            context: context.into(),
            guest_cycle,
            disposition: disposition.into(),
        });
        Ok(())
    }

    pub fn finish(self) -> Vec<ClosurePath> {
        self.paths.into_values().collect()
    }
}

pub(super) struct LiveClosureInputs<'a> {
    pub(super) framebuffer_bytes: &'a [u8],
    pub(super) audio_bytes: &'a [u8],
    pub(super) memory_bytes: &'a [u8],
    pub(super) trace: &'a [TraceEvent],
    pub(super) device_trace: &'a [DeviceTraceEvent],
    pub(super) save_operations: &'a [SaveOperationEvent],
    pub(super) controller_operations: &'a [ControllerOperationEvent],
    pub(super) unsupported_events: &'a [RuntimeUnsupportedEvent],
}

pub(super) fn derive_live_closure(inputs: LiveClosureInputs<'_>) -> Result<Vec<ClosurePath>, GateError> {
    let LiveClosureInputs {
        framebuffer_bytes,
        audio_bytes,
        memory_bytes,
        trace,
        device_trace,
        save_operations,
        controller_operations,
        unsupported_events,
    } = inputs;
    let mut closure = ClosureGate::default();
    for path in LIVE_MINIMUM_CLOSURE_PATHS {
        closure.declare(path)?;
    }

    if !framebuffer_bytes.is_empty() {
        closure.observe_supported("vi.framebuffer")?;
    }
    if !audio_bytes.is_empty() {
        closure.observe_supported("ai.pcm")?;
    }
    if !memory_bytes.is_empty() {
        closure.observe_supported("memory.rdram")?;
    }

    for event in trace {
        let path = match event.kind {
            TraceKind::ThreadSwitch { .. } => Some("cpu.thread-switch"),
            TraceKind::QueueOp { .. } => Some("os.message-queue"),
            // This legacy comparator vocabulary has no device identity or
            // commit phase, so it cannot satisfy a device-qualified path.
            TraceKind::Dma { .. } => None,
            // TaskSubmit is emitted only after StartGo consumes the admitted
            // task token; SpTaskAdmitted above remains the separate Load proof.
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Graphics,
                ..
            } => Some("rsp.graphics-task"),
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Audio,
                ..
            } => Some("rsp.audio-task"),
            // Registration is configuration, not proof that a queue or
            // device event was exercised.
            TraceKind::EventMesg { .. } => None,
        };
        if let Some(path) = path {
            closure.observe_supported(path)?;
        }
    }
    for event in device_trace {
        let path = match event.kind {
            DeviceTraceKind::PiBytesCommitted(_) => Some("device.pi-dma-commit"),
            DeviceTraceKind::SiBytesCommitted(_) => Some("device.si-dma-commit"),
            DeviceTraceKind::AiDmaComplete(_) => Some("device.ai-dma-complete"),
            // `osSpTaskLoad` is synchronous: this event is recorded only
            // after its task-header and rspboot DMA-and-poll loops committed
            // DMEM/IMEM. It does not claim the separate raw timed SP-DMA path.
            DeviceTraceKind::SpTaskAdmitted { .. } => Some("device.sp-task-load-commit"),
            _ => None,
        };
        if let Some(path) = path {
            closure.observe_supported(path)?;
        }
    }
    for (device, path) in LIVE_SAVE_OPERATION_CLOSURE_PATHS {
        let observations = save_operations
            .iter()
            .filter(|event| event.device == device)
            .count() as u64;
        if observations > 0 {
            closure.declare(path)?;
            closure.observe_supported_count(path, observations)?;
        }
    }
    for (device, path) in LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS {
        let observations = controller_operations
            .iter()
            .filter(|event| event.device == device)
            .count() as u64;
        if observations > 0 {
            closure.declare(path)?;
            closure.observe_supported_count(path, observations)?;
        }
    }
    if unsupported_events.is_empty() {
        closure.observe_supported("execution.unsupported-event-source")?;
    } else {
        for event in unsupported_events {
            closure.observe_unsupported(
                "execution.unsupported-event-source",
                event.subsystem.as_str(),
                &event.operation,
                &event.context,
                event.guest_cycle.map(fn64_runtime::Cycles::get),
                event.disposition.as_str(),
            )?;
        }
    }
    Ok(closure.finish())
}
