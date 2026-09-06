#![allow(clippy::module_inception)]
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsupportedInstrumentationEvidence {
    pub schema: String,
    pub sha256: String,
}

impl UnsupportedInstrumentationEvidence {
    fn current() -> Self {
        Self {
            schema: fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SCHEMA.to_owned(),
            sha256: hex(&fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SHA256),
        }
    }

    pub(crate) fn verify_current(&self) -> Result<(), GateError> {
        let expected_sha256 = hex(&fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SHA256);
        if self.schema != fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SCHEMA
            || self.sha256 != expected_sha256
        {
            return Err(GateError::UnsupportedInstrumentationIdentityMismatch {
                expected_schema: fn64_runtime::UNSUPPORTED_INSTRUMENTATION_SCHEMA,
                observed_schema: self.schema.clone(),
                expected_sha256,
                observed_sha256: self.sha256.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseGateReport {
    pub schema: String,
    pub scenario: String,
    pub input_sha256: String,
    /// Installed-ROM identity and decoded header evidence. Synthetic mechanism
    /// reports retain `None` and cannot satisfy ROM-class or TV-region rows.
    pub rom: Option<ReleaseRomEvidence>,
    pub digest: DeterministicDigest,
    /// Machine-verifiable source and geometry for the private framebuffer and
    /// complete physical-RDRAM payloads represented by the artifact digests.
    pub observations: ReleaseObservationGeometry,
    /// Platform, controller ports, cartridge save, and renderer state derived
    /// only from owners frozen at the committed VI boundary.
    pub environment: ReleaseEnvironmentEvidence,
    /// Exact entered executable destinations selected from the program-owner
    /// lane frozen at the same committed boundary.
    pub execution_destinations: ExecutionDestinationEvidence,
    /// Exact ABI-owned graphics-microcode recognition, IMEM replacement, and
    /// committed DPC history frozen at the same boundary.
    pub rsp_rdp: RspRdpEvidence,
    /// Exact audited denominator of production unsupported-event
    /// instrumentation compiled into the runtime that produced this report.
    pub unsupported_instrumentation: UnsupportedInstrumentationEvidence,
    pub closure: Vec<ClosurePath>,
    /// SHA-256 over every other semantic report field in an explicit wire
    /// order. Cite this value, rather than the artifact-only digest root, when
    /// comparing ROM/lane/backend/policy scenarios.
    pub report_sha256: String,
}

pub(super) struct ReleaseBoundaryReportEvidence {
    pub(super) rom: Option<ReleaseRomEvidence>,
    pub(super) observations: ReleaseObservationGeometry,
    pub(super) environment: ReleaseEnvironmentEvidence,
    pub(super) execution_destinations: ExecutionDestinationEvidence,
    pub(super) rsp_rdp: RspRdpEvidence,
}

impl ReleaseGateReport {
    pub(super) fn new_with_environment(
        scenario: impl Into<String>,
        input_bytes: &[u8],
        digest: DeterministicDigest,
        boundary: ReleaseBoundaryReportEvidence,
        mut closure: Vec<ClosurePath>,
    ) -> Result<Self, GateError> {
        let ReleaseBoundaryReportEvidence {
            rom,
            observations,
            environment,
            execution_destinations,
            rsp_rdp,
        } = boundary;
        let scenario = scenario.into();
        if scenario.is_empty() {
            return Err(GateError::EmptyScenario);
        }
        validate_rom_input(&rom, input_bytes)?;
        observations
            .validate()
            .map_err(GateError::InvalidObservationGeometry)?;
        validate_environment_evidence(&environment)?;
        validate_rom_environment(&rom, &environment)?;
        validate_environment_observation(&environment, &observations)?;
        execution_destinations.verify_integrity()?;
        validate_execution_destination_cycles(digest.guest_cycle, &execution_destinations)?;
        rsp_rdp.verify_integrity(digest.guest_cycle)?;
        digest.verify_integrity()?;
        validate_artifact_observation_bytes(&digest, &observations)?;
        validate_closure_paths(&closure)?;
        closure.sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(duplicate) = closure
            .windows(2)
            .find(|pair| pair[0].name == pair[1].name)
            .map(|pair| pair[0].name.clone())
        {
            return Err(GateError::DuplicateClosurePath(duplicate));
        }
        validate_rsp_rdp_closure(&closure, &rsp_rdp)?;
        let mut report = Self {
            schema: REPORT_SCHEMA.to_owned(),
            scenario,
            input_sha256: sha256_hex(input_bytes),
            rom,
            digest,
            observations,
            environment,
            execution_destinations,
            rsp_rdp,
            unsupported_instrumentation: UnsupportedInstrumentationEvidence::current(),
            closure,
            report_sha256: String::new(),
        };
        report.report_sha256 = sha256_hex(&encode_report_evidence(&report)?);
        Ok(report)
    }

    #[cfg(test)]
    pub(crate) fn new(
        scenario: impl Into<String>,
        input_bytes: &[u8],
        digest: DeterministicDigest,
        observations: ReleaseObservationGeometry,
        closure: Vec<ClosurePath>,
    ) -> Result<Self, GateError> {
        let environment = test_release_environment(&observations);
        let rsp_rdp = test_rsp_rdp_evidence(digest.guest_cycle, &closure)?;
        Self::new_with_environment(
            scenario,
            input_bytes,
            digest,
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations,
                environment,
                execution_destinations: ExecutionDestinationEvidence::no_program(),
                rsp_rdp,
            },
            closure,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_test_environment(
        scenario: impl Into<String>,
        input_bytes: &[u8],
        digest: DeterministicDigest,
        observations: ReleaseObservationGeometry,
        environment: ReleaseEnvironmentEvidence,
        closure: Vec<ClosurePath>,
    ) -> Result<Self, GateError> {
        let rsp_rdp = test_rsp_rdp_evidence(digest.guest_cycle, &closure)?;
        Self::new_with_environment(
            scenario,
            input_bytes,
            digest,
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations,
                environment,
                execution_destinations: ExecutionDestinationEvidence::no_program(),
                rsp_rdp,
            },
            closure,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_test_environment_and_destinations(
        scenario: impl Into<String>,
        input_bytes: &[u8],
        digest: DeterministicDigest,
        observations: ReleaseObservationGeometry,
        environment: ReleaseEnvironmentEvidence,
        execution_destinations: ExecutionDestinationEvidence,
        closure: Vec<ClosurePath>,
    ) -> Result<Self, GateError> {
        let rsp_rdp = test_rsp_rdp_evidence(digest.guest_cycle, &closure)?;
        Self::new_with_environment(
            scenario,
            input_bytes,
            digest,
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations,
                environment,
                execution_destinations,
                rsp_rdp,
            },
            closure,
        )
    }

    /// Recompute the schema-v34 evidence digest after loading a retained JSON
    /// report. Acceptance always performs this check before inspecting the
    /// closure ledger.
    pub fn verify_integrity(&self) -> Result<(), GateError> {
        if self.schema != REPORT_SCHEMA {
            return Err(GateError::UnsupportedReportSchema(self.schema.clone()));
        }
        self.observations
            .validate()
            .map_err(GateError::InvalidObservationGeometry)?;
        validate_environment_evidence(&self.environment)?;
        validate_rom_environment(&self.rom, &self.environment)?;
        validate_environment_observation(&self.environment, &self.observations)?;
        self.execution_destinations.verify_integrity()?;
        validate_execution_destination_cycles(
            self.digest.guest_cycle,
            &self.execution_destinations,
        )?;
        self.rsp_rdp.verify_integrity(self.digest.guest_cycle)?;
        self.unsupported_instrumentation.verify_current()?;
        self.digest.verify_integrity()?;
        validate_artifact_observation_bytes(&self.digest, &self.observations)?;
        validate_closure_paths(&self.closure)?;
        validate_canonical_closure_order(&self.closure)?;
        validate_rsp_rdp_closure(&self.closure, &self.rsp_rdp)?;
        decode_sha256(&self.input_sha256).ok_or(GateError::InvalidReportSha256("input_sha256"))?;
        decode_sha256(&self.report_sha256)
            .ok_or(GateError::InvalidReportSha256("report_sha256"))?;
        let recomputed = sha256_hex(&encode_report_evidence(self)?);
        if recomputed == self.report_sha256 {
            Ok(())
        } else {
            Err(GateError::ReportIntegrityMismatch {
                stored: self.report_sha256.clone(),
                recomputed,
            })
        }
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> Result<(), GateError> {
        let mut file = File::create(path).map_err(GateError::WriteReport)?;
        serde_json::to_writer_pretty(&mut file, self).map_err(GateError::SerializeReport)?;
        file.write_all(b"\n").map_err(GateError::WriteReport)?;
        file.flush().map_err(GateError::WriteReport)
    }

    /// A release claim requires both coverage and zero unsupported events.
    pub fn require_closed(&self) -> Result<(), GateError> {
        self.verify_integrity()?;
        if self.closure.is_empty() {
            return Err(GateError::NoClosurePaths);
        }
        let unexercised: Vec<_> = self
            .closure
            .iter()
            .filter(|path| matches!(path.status, ClosurePathStatus::Unexercised))
            .map(|path| path.name.clone())
            .collect();
        let unsupported: Vec<_> = self
            .closure
            .iter()
            .flat_map(|path| {
                path.unsupported
                    .iter()
                    .map(move |event| format!("{}:{}", path.name, event.operation))
            })
            .collect();
        if unexercised.is_empty() && unsupported.is_empty() {
            Ok(())
        } else {
            Err(GateError::ClosureIncomplete {
                unexercised,
                unsupported,
            })
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error("{kind:?} captured at guest cycle {observed}, expected {expected}")]
    WrongCycle {
        expected: u64,
        observed: u64,
        kind: ArtifactKind,
    },
    #[error("duplicate {0:?} digest artifact")]
    DuplicateArtifact(ArtifactKind),
    #[error("missing digest artifacts: {0:?}")]
    MissingArtifacts(Vec<ArtifactKind>),
    #[error("timing trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}")]
    FutureTraceEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    #[error("device trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}")]
    FutureDeviceTraceEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    #[error(
        "save-operation trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
    )]
    FutureSaveOperationEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    #[error(
        "controller-operation trace for port {port} contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
    )]
    FutureControllerOperationEvent {
        gate_cycle: u64,
        event_cycle: u64,
        port: u8,
    },
    #[error(
        "execution-destination trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
    )]
    FutureExecutionDestinationEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    #[error(
        "unsupported event {operation:?} contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
    )]
    FutureUnsupportedEvent {
        gate_cycle: u64,
        event_cycle: u64,
        operation: String,
    },
    #[error(
        "RSP/RDP observation contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
    )]
    FutureRspRdpObservation {
        gate_cycle: u64,
        event_cycle: u64,
    },
    #[error(
        "live release gate armed after execution began: sim_time={sim_time}, \
         trace_events={trace_events}, device_trace_events={device_trace_events}, \
         save_operation_events={save_operation_events}, \
         controller_operation_events={controller_operation_events}, \
         rsp_rdp_observations={rsp_rdp_observations}, \
         native_execution_destination_events={native_execution_destination_events}, \
         function_execution_destination_events={function_execution_destination_events}, \
         block_execution_destination_events={block_execution_destination_events}"
    )]
    LiveGateArmedLate {
        sim_time: u64,
        trace_events: usize,
        device_trace_events: usize,
        save_operation_events: usize,
        controller_operation_events: usize,
        rsp_rdp_observations: usize,
        native_execution_destination_events: usize,
        function_execution_destination_events: usize,
        block_execution_destination_events: usize,
    },
    #[error("live release gate was not armed before boot")]
    LiveGateNotArmed,
    #[error(
        "live release evidence cannot identify the native recompiled program; commit the VI boundary with ReleaseProgramDescriptor::NativeArchive and the exact linked-archive identity"
    )]
    UnidentifiedNativeProgram,
    #[error(
        "typed function destination {symbol:?} at {vram:#010x} belongs to artifact {observed}, expected {expected}"
    )]
    FunctionDestinationArtifactMismatch {
        expected: String,
        observed: String,
        vram: u32,
        symbol: String,
    },
    #[error(
        "typed block destination bank={bank:#018x}, pc={pc:#010x} was entered without a stable runner artifact identity"
    )]
    UnidentifiedBlockRunnerArtifact {
        bank: u64,
        pc: u32,
    },
    #[error(
        "identified executable source {0} reached the release boundary without an entered destination"
    )]
    EmptyExecutionDestinationEvidence(&'static str),
    #[error("execution-destination source mismatch: {0}")]
    ExecutionDestinationSourceMismatch(&'static str),
    #[error("execution-destination counts, canonical set, order, or digest are inconsistent")]
    ExecutionDestinationIntegrityMismatch,
    #[error("RSP/RDP observation count exceeds u64")]
    RspRdpObservationCountOverflow,
    #[error("RSP/RDP observation count, order, or digest is inconsistent")]
    RspRdpObservationIntegrityMismatch,
    #[error(
        "{event_source} DPC observation range [{start:#010x}, {end:#010x}) must be nonempty, 8-byte aligned, and end at or below {limit:#010x}"
    )]
    InvalidDpcObservationRange {
        event_source: &'static str,
        start: u32,
        end: u32,
        limit: u32,
    },
    #[error(
        "device evidence {register} value {value:#010x} exceeds the canonical 24-bit DPC counter domain"
    )]
    NonCanonicalDpcCounter {
        register: &'static str,
        value: u32,
    },
    #[error(
        "device evidence MI occurrence {interrupt_source:?} in slot {slot} is inconsistent: {detail}"
    )]
    InvalidMiInterruptOccurrence {
        slot: usize,
        interrupt_source: fn64_runtime::InterruptSource,
        detail: &'static str,
    },
    #[error("release HostKernel interrupt evidence is inconsistent: {0}")]
    InconsistentHostInterruptEvidence(&'static str),
    #[error(
        "microcode-data observation at {start:#010x} with {bytes:#010x} bytes must be nonempty and fit physical RDRAM ending at {limit:#010x}"
    )]
    InvalidMicrocodeDataObservationRange {
        start: u32,
        bytes: u32,
        limit: u32,
    },
    #[error(
        "RSP task observation at {address:#010x} must name a complete 64-byte OSTask header inside physical RDRAM ending at {limit:#010x}"
    )]
    InvalidRspTaskObservationAddress {
        address: u32,
        limit: u32,
    },
    #[error("RSP/RDP observation cycle {observed} precedes retained cycle {previous}")]
    NonMonotonicRspRdpObservationCycle {
        previous: u64,
        observed: u64,
    },
    #[error("RSP IMEM generation {observed} precedes retained generation {previous}")]
    NonMonotonicImemGeneration {
        previous: u64,
        observed: u64,
    },
    #[error(
        "RSP IMEM replacement generation {observed} does not follow retained generation {previous}"
    )]
    NonMonotonicImemReplacementGeneration {
        previous: u64,
        observed: u64,
    },
    #[error(
        "RSP IMEM generation {generation} names conflicting text digests {previous} and {observed}"
    )]
    ConflictingImemGenerationDigest {
        generation: u64,
        previous: String,
        observed: String,
    },
    #[error("exercised graphics-task closure lacks an ABI-owned microcode-recognition observation")]
    MissingGraphicsMicrocodeRecognition,
    #[error(
        "release ROM has {bytes} bytes; the normalized N64 header requires at least {ROM_HEADER_BYTES}"
    )]
    RomTooSmall {
        bytes: u64,
    },
    #[error("release ROM has {bytes} bytes; z64/n64/v64 normalization requires a multiple of four")]
    RomNotWordAligned {
        bytes: u64,
    },
    #[error("release ROM byte length exceeds the u64 evidence wire")]
    RomByteLengthOverflow,
    #[error("release ROM first word {first_word:#010x} is not z64, n64, or v64 byte order")]
    UnknownRomByteOrder {
        first_word: u32,
    },
    #[error("release ROM destination code {0:#04x} has no admitted NTSC/PAL/M-PAL/region-free decode")]
    UnknownRomDestinationCode(u8),
    #[error(
        "release ROM destination code {destination_code:#04x} decodes as {decoded:?}, not retained {stored:?}"
    )]
    RomRegionDecodeMismatch {
        destination_code: u8,
        stored: ReleaseTvRegion,
        decoded: ReleaseTvRegion,
    },
    #[error("retained ROM identity/header evidence differs from the supplied input bytes")]
    RomInputEvidenceMismatch,
    #[error("committed device evidence has no configured TV type for ROM-region certification")]
    MissingDeviceTvType,
    #[error("committed ABI host evidence has no installed-ROM identity")]
    MissingInstalledRomIdentity,
    #[error(
        "supplied release ROM ({supplied_bytes} bytes, {supplied_sha256}) differs from installed ROM ({installed_bytes} bytes, {installed_sha256})"
    )]
    InstalledRomIdentityMismatch {
        installed_bytes: u64,
        supplied_bytes: u64,
        installed_sha256: String,
        supplied_sha256: String,
    },
    #[error("{authority} requires TV type {expected:?}, observed {observed:?}")]
    RomTvTypeMismatch {
        authority: &'static str,
        expected: ReleaseTvStandard,
        observed: ReleaseTvStandard,
    },
    #[error(
        "live release evidence cannot identify cartridge save hardware; use set_cartridge_save or configure_no_cartridge_save before boot"
    )]
    UnidentifiedCartridgeSave,
    #[error(
        "live release evidence cannot identify the registered renderer; its RenderBackend implementation must self-report release_environment"
    )]
    UnidentifiedRenderBackend,
    #[error("live release evidence requires GraphicsTaskExecutionPolicy::LleAccuracy")]
    NonAccuracyRenderPolicy,
    #[error("live release evidence requires AudioTaskExecutionPolicy::LleAccuracy")]
    NonAccuracyAudioTaskPolicy,
    #[error("invalid Windows release identity: {0}")]
    InvalidWindowsVersionEvidence(&'static str),
    #[error("frozen renderer evidence disagrees with framebuffer observation: {0}")]
    RendererObservationMismatch(&'static str),
    #[error("invalid committed VI release boundary: {0}")]
    InvalidViBoundary(crate::ViBoundaryError),
    #[error("live release capture occurred at guest cycle {observed}, expected {expected}")]
    WrongLiveCycle {
        expected: u64,
        observed: u64,
    },
    #[error("committed VI boundary has no registered complete physical RDRAM observation")]
    BoundaryPhysicalRdramUnavailable,
    #[error(
        "reference framebuffer observation at {address:#010x} for {bytes} bytes lies outside the committed physical RDRAM image"
    )]
    ReferenceFramebufferOutsideFrozenMemory {
        address: u32,
        bytes: u64,
    },
    #[error(
        "reference framebuffer observation at {address:#010x} for {bytes} bytes does not match the committed physical RDRAM image"
    )]
    ReferenceFramebufferDoesNotMatchFrozenMemory {
        address: u32,
        bytes: u64,
    },
    #[error("live release audio digest capture was not armed")]
    AudioDigestCaptureNotArmed,
    #[error(
        "release report unsupported-instrumentation identity mismatch: expected {expected_schema}/{expected_sha256}, observed {observed_schema}/{observed_sha256}"
    )]
    UnsupportedInstrumentationIdentityMismatch {
        expected_schema: &'static str,
        observed_schema: String,
        expected_sha256: String,
        observed_sha256: String,
    },
    #[error("{0}")]
    InvalidObservationGeometry(ObservationEvidenceError),
    #[error("arm unsupported-event journal: {0}")]
    ArmUnsupportedJournal(io::Error),
    #[error("release-gate scenario must not be empty")]
    EmptyScenario,
    #[error("closure path name must not be empty")]
    EmptyPathName,
    #[error("closure path {0:?} declared twice")]
    DuplicatePath(String),
    #[error("release report contains duplicate closure path {0:?}")]
    DuplicateClosurePath(String),
    #[error("unsupported release report schema {0:?}")]
    UnsupportedReportSchema(String),
    #[error("release report field {0} is not a SHA-256")]
    InvalidReportSha256(&'static str),
    #[error(
        "fixed-cycle digest artifacts are not the canonical exact set: expected={expected:?}, observed={observed:?}"
    )]
    InvalidArtifactSet {
        expected: Vec<ArtifactKind>,
        observed: Vec<ArtifactKind>,
    },
    #[error("fixed-cycle digest root mismatch: stored={stored}, recomputed={recomputed}")]
    DigestRootIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    #[error(
        "{kind:?} artifact contains {observed} bytes, expected {expected} from observation geometry"
    )]
    ArtifactObservationByteMismatch {
        kind: ArtifactKind,
        expected: u64,
        observed: u64,
    },
    #[error("release closure path {name:?} is inconsistent: {detail}")]
    InvalidClosurePath {
        name: String,
        detail: &'static str,
    },
    #[error(
        "release closure paths are not in strict canonical name order: {previous:?} before {next:?}"
    )]
    NonCanonicalClosureOrder {
        previous: String,
        next: String,
    },
    #[error("release report SHA mismatch: stored={stored}, recomputed={recomputed}")]
    ReportIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    #[error("closure observation used undeclared path {0:?}")]
    UndeclaredPath(String),
    #[error("unsupported event name must not be empty")]
    EmptyUnsupportedName,
    #[error("release closure declared no paths")]
    NoClosurePaths,
    #[error("release closure failed; unexercised={unexercised:?}; unsupported={unsupported:?}")]
    ClosureIncomplete {
        unexercised: Vec<String>,
        unsupported: Vec<String>,
    },
    #[error("serialize release report: {0}")]
    SerializeReport(serde_json::Error),
    #[error("write release report: {0}")]
    WriteReport(io::Error),
}
