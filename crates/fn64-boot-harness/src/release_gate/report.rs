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

    /// Recompute the schema-v30 evidence digest after loading a retained JSON
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

#[derive(Debug)]
pub enum GateError {
    WrongCycle {
        expected: u64,
        observed: u64,
        kind: ArtifactKind,
    },
    DuplicateArtifact(ArtifactKind),
    MissingArtifacts(Vec<ArtifactKind>),
    FutureTraceEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    FutureDeviceTraceEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    FutureSaveOperationEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    FutureControllerOperationEvent {
        gate_cycle: u64,
        event_cycle: u64,
        port: u8,
    },
    FutureExecutionDestinationEvent {
        gate_cycle: u64,
        event_cycle: u64,
    },
    FutureUnsupportedEvent {
        gate_cycle: u64,
        event_cycle: u64,
        operation: String,
    },
    FutureRspRdpObservation {
        gate_cycle: u64,
        event_cycle: u64,
    },
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
    LiveGateNotArmed,
    UnidentifiedNativeProgram,
    FunctionDestinationArtifactMismatch {
        expected: String,
        observed: String,
        vram: u32,
        symbol: String,
    },
    UnidentifiedBlockRunnerArtifact {
        bank: u64,
        pc: u32,
    },
    EmptyExecutionDestinationEvidence(&'static str),
    ExecutionDestinationSourceMismatch(&'static str),
    ExecutionDestinationIntegrityMismatch,
    RspRdpObservationCountOverflow,
    RspRdpObservationIntegrityMismatch,
    InvalidDpcObservationRange {
        source: &'static str,
        start: u32,
        end: u32,
        limit: u32,
    },
    NonCanonicalDpcCounter {
        register: &'static str,
        value: u32,
    },
    InvalidMicrocodeDataObservationRange {
        start: u32,
        bytes: u32,
        limit: u32,
    },
    InvalidRspTaskObservationAddress {
        address: u32,
        limit: u32,
    },
    NonMonotonicRspRdpObservationCycle {
        previous: u64,
        observed: u64,
    },
    NonMonotonicImemGeneration {
        previous: u64,
        observed: u64,
    },
    NonMonotonicImemReplacementGeneration {
        previous: u64,
        observed: u64,
    },
    ConflictingImemGenerationDigest {
        generation: u64,
        previous: String,
        observed: String,
    },
    MissingGraphicsMicrocodeRecognition,
    RomTooSmall {
        bytes: u64,
    },
    RomNotWordAligned {
        bytes: u64,
    },
    RomByteLengthOverflow,
    UnknownRomByteOrder {
        first_word: u32,
    },
    UnknownRomDestinationCode(u8),
    RomRegionDecodeMismatch {
        destination_code: u8,
        stored: ReleaseTvRegion,
        decoded: ReleaseTvRegion,
    },
    RomInputEvidenceMismatch,
    MissingDeviceTvType,
    MissingInstalledRomIdentity,
    InstalledRomIdentityMismatch {
        installed_bytes: u64,
        supplied_bytes: u64,
        installed_sha256: String,
        supplied_sha256: String,
    },
    RomTvTypeMismatch {
        authority: &'static str,
        expected: ReleaseTvStandard,
        observed: ReleaseTvStandard,
    },
    UnidentifiedCartridgeSave,
    UnidentifiedRenderBackend,
    NonAccuracyRenderPolicy,
    NonAccuracyAudioTaskPolicy,
    InvalidWindowsVersionEvidence(&'static str),
    RendererObservationMismatch(&'static str),
    InvalidViBoundary(crate::ViBoundaryError),
    WrongLiveCycle {
        expected: u64,
        observed: u64,
    },
    BoundaryPhysicalRdramUnavailable,
    ReferenceFramebufferOutsideFrozenMemory {
        address: u32,
        bytes: u64,
    },
    ReferenceFramebufferDoesNotMatchFrozenMemory {
        address: u32,
        bytes: u64,
    },
    AudioDigestCaptureNotArmed,
    UnsupportedInstrumentationIdentityMismatch {
        expected_schema: &'static str,
        observed_schema: String,
        expected_sha256: String,
        observed_sha256: String,
    },
    InvalidObservationGeometry(ObservationEvidenceError),
    ArmUnsupportedJournal(io::Error),
    EmptyScenario,
    EmptyPathName,
    DuplicatePath(String),
    DuplicateClosurePath(String),
    UnsupportedReportSchema(String),
    InvalidReportSha256(&'static str),
    InvalidArtifactSet {
        expected: Vec<ArtifactKind>,
        observed: Vec<ArtifactKind>,
    },
    DigestRootIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    ArtifactObservationByteMismatch {
        kind: ArtifactKind,
        expected: u64,
        observed: u64,
    },
    InvalidClosurePath {
        name: String,
        detail: &'static str,
    },
    NonCanonicalClosureOrder {
        previous: String,
        next: String,
    },
    ReportIntegrityMismatch {
        stored: String,
        recomputed: String,
    },
    UndeclaredPath(String),
    EmptyUnsupportedName,
    NoClosurePaths,
    ClosureIncomplete {
        unexercised: Vec<String>,
        unsupported: Vec<String>,
    },
    SerializeReport(serde_json::Error),
    WriteReport(io::Error),
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongCycle {
                expected,
                observed,
                kind,
            } => write!(
                f,
                "{kind:?} captured at guest cycle {observed}, expected {expected}"
            ),
            Self::DuplicateArtifact(kind) => write!(f, "duplicate {kind:?} digest artifact"),
            Self::InvalidObservationGeometry(error) => error.fmt(f),
            Self::BoundaryPhysicalRdramUnavailable => write!(
                f,
                "committed VI boundary has no registered complete physical RDRAM observation"
            ),
            Self::UnsupportedInstrumentationIdentityMismatch {
                expected_schema,
                observed_schema,
                expected_sha256,
                observed_sha256,
            } => write!(
                f,
                "release report unsupported-instrumentation identity mismatch: expected {expected_schema}/{expected_sha256}, observed {observed_schema}/{observed_sha256}"
            ),
            Self::ReferenceFramebufferOutsideFrozenMemory { address, bytes } => write!(
                f,
                "reference framebuffer observation at {address:#010x} for {bytes} bytes lies outside the committed physical RDRAM image"
            ),
            Self::ReferenceFramebufferDoesNotMatchFrozenMemory { address, bytes } => write!(
                f,
                "reference framebuffer observation at {address:#010x} for {bytes} bytes does not match the committed physical RDRAM image"
            ),
            Self::MissingArtifacts(kinds) => write!(f, "missing digest artifacts: {kinds:?}"),
            Self::FutureTraceEvent {
                gate_cycle,
                event_cycle,
            } => write!(
                f,
                "timing trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureDeviceTraceEvent {
                gate_cycle,
                event_cycle,
            } => write!(
                f,
                "device trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureSaveOperationEvent {
                gate_cycle,
                event_cycle,
            } => write!(
                f,
                "save-operation trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureControllerOperationEvent {
                gate_cycle,
                event_cycle,
                port,
            } => write!(
                f,
                "controller-operation trace for port {port} contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureExecutionDestinationEvent {
                gate_cycle,
                event_cycle,
            } => write!(
                f,
                "execution-destination trace contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureUnsupportedEvent {
                gate_cycle,
                event_cycle,
                operation,
            } => write!(
                f,
                "unsupported event {operation:?} contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::FutureRspRdpObservation {
                gate_cycle,
                event_cycle,
            } => write!(
                f,
                "RSP/RDP observation contains guest cycle {event_cycle} after gate cycle {gate_cycle}"
            ),
            Self::LiveGateArmedLate {
                sim_time,
                trace_events,
                device_trace_events,
                save_operation_events,
                controller_operation_events,
                rsp_rdp_observations,
                native_execution_destination_events,
                function_execution_destination_events,
                block_execution_destination_events,
            } => write!(
                f,
                "live release gate armed after execution began: sim_time={sim_time}, \
                 trace_events={trace_events}, device_trace_events={device_trace_events}, \
                 save_operation_events={save_operation_events}, \
                 controller_operation_events={controller_operation_events}, \
                 rsp_rdp_observations={rsp_rdp_observations}, \
                 native_execution_destination_events={native_execution_destination_events}, \
                 function_execution_destination_events={function_execution_destination_events}, \
                 block_execution_destination_events={block_execution_destination_events}"
            ),
            Self::LiveGateNotArmed => write!(f, "live release gate was not armed before boot"),
            Self::UnidentifiedNativeProgram => write!(
                f,
                "live release evidence cannot identify the native recompiled program; commit the VI boundary with ReleaseProgramDescriptor::NativeArchive and the exact linked-archive identity"
            ),
            Self::FunctionDestinationArtifactMismatch {
                expected,
                observed,
                vram,
                symbol,
            } => write!(
                f,
                "typed function destination {symbol:?} at {vram:#010x} belongs to artifact {observed}, expected {expected}"
            ),
            Self::UnidentifiedBlockRunnerArtifact { bank, pc } => write!(
                f,
                "typed block destination bank={bank:#018x}, pc={pc:#010x} was entered without a stable runner artifact identity"
            ),
            Self::EmptyExecutionDestinationEvidence(source) => write!(
                f,
                "identified executable source {source} reached the release boundary without an entered destination"
            ),
            Self::ExecutionDestinationSourceMismatch(detail) => {
                write!(f, "execution-destination source mismatch: {detail}")
            }
            Self::ExecutionDestinationIntegrityMismatch => write!(
                f,
                "execution-destination counts, canonical set, order, or digest are inconsistent"
            ),
            Self::RspRdpObservationCountOverflow => {
                write!(f, "RSP/RDP observation count exceeds u64")
            }
            Self::RspRdpObservationIntegrityMismatch => write!(
                f,
                "RSP/RDP observation count, order, or digest is inconsistent"
            ),
            Self::InvalidDpcObservationRange {
                source,
                start,
                end,
                limit,
            } => write!(
                f,
                "{source} DPC observation range [{start:#010x}, {end:#010x}) must be nonempty, 8-byte aligned, and end at or below {limit:#010x}"
            ),
            Self::NonCanonicalDpcCounter { register, value } => write!(
                f,
                "device evidence {register} value {value:#010x} exceeds the canonical 24-bit DPC counter domain"
            ),
            Self::InvalidMicrocodeDataObservationRange {
                start,
                bytes,
                limit,
            } => write!(
                f,
                "microcode-data observation at {start:#010x} with {bytes:#010x} bytes must be nonempty and fit physical RDRAM ending at {limit:#010x}"
            ),
            Self::InvalidRspTaskObservationAddress { address, limit } => write!(
                f,
                "RSP task observation at {address:#010x} must name a complete 64-byte OSTask header inside physical RDRAM ending at {limit:#010x}"
            ),
            Self::NonMonotonicRspRdpObservationCycle { previous, observed } => write!(
                f,
                "RSP/RDP observation cycle {observed} precedes retained cycle {previous}"
            ),
            Self::NonMonotonicImemGeneration { previous, observed } => write!(
                f,
                "RSP IMEM generation {observed} precedes retained generation {previous}"
            ),
            Self::NonMonotonicImemReplacementGeneration { previous, observed } => write!(
                f,
                "RSP IMEM replacement generation {observed} does not follow retained generation {previous}"
            ),
            Self::ConflictingImemGenerationDigest {
                generation,
                previous,
                observed,
            } => write!(
                f,
                "RSP IMEM generation {generation} names conflicting text digests {previous} and {observed}"
            ),
            Self::MissingGraphicsMicrocodeRecognition => write!(
                f,
                "exercised graphics-task closure lacks an ABI-owned microcode-recognition observation"
            ),
            Self::RomTooSmall { bytes } => write!(
                f,
                "release ROM has {bytes} bytes; the normalized N64 header requires at least {ROM_HEADER_BYTES}"
            ),
            Self::RomNotWordAligned { bytes } => write!(
                f,
                "release ROM has {bytes} bytes; z64/n64/v64 normalization requires a multiple of four"
            ),
            Self::RomByteLengthOverflow => {
                write!(f, "release ROM byte length exceeds the u64 evidence wire")
            }
            Self::UnknownRomByteOrder { first_word } => write!(
                f,
                "release ROM first word {first_word:#010x} is not z64, n64, or v64 byte order"
            ),
            Self::UnknownRomDestinationCode(code) => write!(
                f,
                "release ROM destination code {code:#04x} has no admitted NTSC/PAL/M-PAL/region-free decode"
            ),
            Self::RomRegionDecodeMismatch {
                destination_code,
                stored,
                decoded,
            } => write!(
                f,
                "release ROM destination code {destination_code:#04x} decodes as {decoded:?}, not retained {stored:?}"
            ),
            Self::RomInputEvidenceMismatch => write!(
                f,
                "retained ROM identity/header evidence differs from the supplied input bytes"
            ),
            Self::MissingDeviceTvType => write!(
                f,
                "committed device evidence has no configured TV type for ROM-region certification"
            ),
            Self::MissingInstalledRomIdentity => write!(
                f,
                "committed ABI host evidence has no installed-ROM identity"
            ),
            Self::InstalledRomIdentityMismatch {
                installed_bytes,
                supplied_bytes,
                installed_sha256,
                supplied_sha256,
            } => write!(
                f,
                "supplied release ROM ({supplied_bytes} bytes, {supplied_sha256}) differs from installed ROM ({installed_bytes} bytes, {installed_sha256})"
            ),
            Self::RomTvTypeMismatch {
                authority,
                expected,
                observed,
            } => write!(
                f,
                "{authority} requires TV type {expected:?}, observed {observed:?}"
            ),
            Self::UnidentifiedCartridgeSave => write!(
                f,
                "live release evidence cannot identify cartridge save hardware; use set_cartridge_save or configure_no_cartridge_save before boot"
            ),
            Self::UnidentifiedRenderBackend => write!(
                f,
                "live release evidence cannot identify the registered renderer; its RenderBackend implementation must self-report release_environment"
            ),
            Self::NonAccuracyRenderPolicy => write!(
                f,
                "live release evidence requires GraphicsTaskExecutionPolicy::LleAccuracy"
            ),
            Self::NonAccuracyAudioTaskPolicy => write!(
                f,
                "live release evidence requires AudioTaskExecutionPolicy::LleAccuracy"
            ),
            Self::InvalidWindowsVersionEvidence(detail) => {
                write!(f, "invalid Windows release identity: {detail}")
            }
            Self::RendererObservationMismatch(detail) => {
                write!(
                    f,
                    "frozen renderer evidence disagrees with framebuffer observation: {detail}"
                )
            }
            Self::InvalidViBoundary(error) => {
                write!(f, "invalid committed VI release boundary: {error}")
            }
            Self::WrongLiveCycle { expected, observed } => write!(
                f,
                "live release capture occurred at guest cycle {observed}, expected {expected}"
            ),
            Self::AudioDigestCaptureNotArmed => {
                write!(f, "live release audio digest capture was not armed")
            }
            Self::ArmUnsupportedJournal(error) => {
                write!(f, "arm unsupported-event journal: {error}")
            }
            Self::EmptyScenario => write!(f, "release-gate scenario must not be empty"),
            Self::EmptyPathName => write!(f, "closure path name must not be empty"),
            Self::DuplicatePath(name) => write!(f, "closure path {name:?} declared twice"),
            Self::DuplicateClosurePath(name) => {
                write!(f, "release report contains duplicate closure path {name:?}")
            }
            Self::UnsupportedReportSchema(schema) => {
                write!(f, "unsupported release report schema {schema:?}")
            }
            Self::InvalidReportSha256(field) => {
                write!(f, "release report field {field} is not a SHA-256")
            }
            Self::InvalidArtifactSet { expected, observed } => write!(
                f,
                "fixed-cycle digest artifacts are not the canonical exact set: expected={expected:?}, observed={observed:?}"
            ),
            Self::DigestRootIntegrityMismatch { stored, recomputed } => write!(
                f,
                "fixed-cycle digest root mismatch: stored={stored}, recomputed={recomputed}"
            ),
            Self::ArtifactObservationByteMismatch {
                kind,
                expected,
                observed,
            } => write!(
                f,
                "{kind:?} artifact contains {observed} bytes, expected {expected} from observation geometry"
            ),
            Self::InvalidClosurePath { name, detail } => {
                write!(f, "release closure path {name:?} is inconsistent: {detail}")
            }
            Self::NonCanonicalClosureOrder { previous, next } => write!(
                f,
                "release closure paths are not in strict canonical name order: {previous:?} before {next:?}"
            ),
            Self::ReportIntegrityMismatch { stored, recomputed } => write!(
                f,
                "release report SHA mismatch: stored={stored}, recomputed={recomputed}"
            ),
            Self::UndeclaredPath(name) => {
                write!(f, "closure observation used undeclared path {name:?}")
            }
            Self::EmptyUnsupportedName => write!(f, "unsupported event name must not be empty"),
            Self::NoClosurePaths => write!(f, "release closure declared no paths"),
            Self::ClosureIncomplete {
                unexercised,
                unsupported,
            } => write!(
                f,
                "release closure failed; unexercised={unexercised:?}; unsupported={unsupported:?}"
            ),
            Self::SerializeReport(error) => write!(f, "serialize release report: {error}"),
            Self::WriteReport(error) => write!(f, "write release report: {error}"),
        }
    }
}

impl std::error::Error for GateError {}
