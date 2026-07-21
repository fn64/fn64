//! Shared boot seam for the headless examples and `fn64-shell`.
//!
//! The generated-C lane presents one process-global section table through
//! `bridge/section_bridge.c`. This crate owns that bridge's Rust callback,
//! batches its per-function records into `fn64-abi` sections, exposes the
//! generated entry point, and allocates the one ABI-visible RDRAM buffer.
//! Game policy remains with each harness: which sections are resident,
//! controller input, save type, rendering, audio, and executor driving.

mod certification_profile;
mod observation_evidence;
mod private_release_series;
mod release_gate;
mod release_matrix;
mod release_program_build_receipt;
mod release_run_env;
mod render_evidence;
mod report_series;
mod unsupported_journal;

pub use certification_profile::{
    CertificationProfileError, CertificationProfileIdentity, CertificationRequirement,
    CertificationRequirementClass, CertificationRequirementRef, FullParityV1,
    FULL_PARITY_V1_DEFINITION_SHA256, FULL_PARITY_V1_SCHEMA,
};
pub use observation_evidence::{
    FramebufferObservationFormat, FramebufferObservationGeometry, FramebufferObservationSource,
    LiveMemoryEvidence, LiveReferenceFramebufferEvidence, LiveReleaseGateObservationExt,
    MemoryObservationGeometry, ObservationEvidenceError, ReleaseObservationGeometry,
};
pub use private_release_series::{
    load_private_release_run_contract, run_private_release_series, verify_private_release_series,
    verify_repository_synthetic_private_release_run_contract, PrivateArtifactIdentity,
    PrivateChildCommand, PrivateEnvironmentEntry, PrivateFileIdentity, PrivateReleaseRunContract,
    PrivateReleaseSeriesError, PrivateReleaseSeriesReceipt, PrivateReleaseSeriesRun,
    VerifiedPrivateReleaseRunContract, PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA,
    PRIVATE_RELEASE_SERIES_COUNT, PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA,
    REPOSITORY_SYNTHETIC_RELEASE_CYCLE, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
};
pub use release_gate::{
    ArtifactDigest, ArtifactKind, ClosureGate, ClosurePath, ClosurePathStatus, DeterministicDigest,
    ExecutionDestinationCountEvidence, ExecutionDestinationEventEvidence,
    ExecutionDestinationEvidence, ExecutionDestinationSource, FixedCycleDigestGate, GateError,
    LiveReleaseGate, ReleaseExecutionDestination, ReleaseGateReport, ReleaseMicrocodeFamily,
    RspRdpEvidence, RspRdpObservationEventEvidence, RspRdpObservationKindEvidence,
    UnsupportedEvent, LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS, LIVE_MINIMUM_CLOSURE_PATHS,
    LIVE_SAVE_OPERATION_CLOSURE_PATHS,
};
pub use release_matrix::{
    verify_release_matrix, CertificationRequirementAssignment, ControllerFeature,
    IncompleteReleaseMatrix, MicrocodeFeature, PresentationBoundaryEvidence, ProgramFeature,
    ReleaseMatrixCoverage, ReleaseMatrixError, ReleaseMatrixManifest, ReleaseMatrixScenario,
    ReleaseMatrixVerification, ReleasePlatform, RendererFeature, RspRdpMechanismFeature,
    SaveFeature, VerifiedMatrixScenario, VerifiedReleaseMatrix, INCOMPLETE_RELEASE_MATRIX_SCHEMA,
    RELEASE_MATRIX_MAX_SCENARIOS, RELEASE_MATRIX_REPORT_COUNT, RELEASE_MATRIX_SCHEMA,
    VERIFIED_RELEASE_MATRIX_SCHEMA,
};
pub use release_run_env::{
    release_run_environment_from_process, release_run_environment_from_process_with_oot_aliases,
    ReleaseRunEnvironment, ReleaseRunEnvironmentError, RELEASE_GATE_CYCLE_ENV, RELEASE_REPORT_ENV,
    RELEASE_RUN_EVENT_SHA256_ENV,
};
pub use render_evidence::{
    LiveReleaseGateRenderExt, LiveRenderEvidence, RenderCaptureStage, RenderEvidenceError,
    RenderPixelFormat,
};
pub use report_series::{
    verify_release_evidence_series, verify_release_report_series, ReportSeriesError,
    VerifiedReportSeries,
};
pub use unsupported_journal::{
    parse_unsupported_journal, verify_release_report_journal, ParsedUnsupportedJournal,
    ParsedUnsupportedJournalEvent, UnsupportedJournalCompletion, UnsupportedJournalError,
};

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

#[path = "../native_program_identity.rs"]
mod native_program_identity;

pub use native_program_identity::native_program_archives_sha256;

#[cfg(feature = "c-bridge")]
use std::collections::HashMap;

pub use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;

pub use fn64_runtime::TvType;

/// Decision for a headless host that drains guest work between virtual VI
/// fields. The public libultra idle priority is the semantic boundary: one
/// idle-thread turn is observable, while a second consecutive turn means no
/// higher-priority guest work can run until the host injects time or an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainDecision {
    Step,
    AdvanceField,
}

/// Diagnostics-only search for a scheduler quiescence boundary at or after a
/// requested guest-cycle floor. This does not own a release gate or report
/// path; callers must use the returned cycle in a separate release run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuiescentDiscovery {
    floor: u64,
}

impl QuiescentDiscovery {
    pub const fn floor(self) -> u64 {
        self.floor
    }

    pub const fn matches(self, decision: DrainDecision, cycle: u64) -> bool {
        matches!(decision, DrainDecision::AdvanceField) && cycle >= self.floor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuiescentDiscoveryError {
    InvalidFloor(String),
    ConflictsWithReleaseGate,
}

impl std::fmt::Display for QuiescentDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFloor(raw) => write!(
                formatter,
                "OOT_RELEASE_DISCOVER_QUIESCENT_AFTER must be an unsigned guest cycle, got {raw:?}"
            ),
            Self::ConflictsWithReleaseGate => write!(
                formatter,
                "OOT_RELEASE_DISCOVER_QUIESCENT_AFTER cannot be combined with OOT_RELEASE_GATE_CYCLE or OOT_RELEASE_REPORT"
            ),
        }
    }
}

impl std::error::Error for QuiescentDiscoveryError {}

/// Parse diagnostics mode without consulting ambient process state, keeping
/// its conflict and boundary predicates unit-testable.
pub fn parse_quiescent_discovery(
    discovery_floor: Option<&str>,
    release_gate_cycle: Option<&str>,
    release_report_present: bool,
) -> Result<Option<QuiescentDiscovery>, QuiescentDiscoveryError> {
    let Some(raw) = discovery_floor else {
        return Ok(None);
    };
    if release_gate_cycle.is_some() || release_report_present {
        return Err(QuiescentDiscoveryError::ConflictsWithReleaseGate);
    }
    let floor = raw
        .parse::<u64>()
        .map_err(|_| QuiescentDiscoveryError::InvalidFloor(raw.to_owned()))?;
    Ok(Some(QuiescentDiscovery { floor }))
}

/// How execution reached an observed guest cycle. Release presentation
/// evidence is admissible only on the host-advance edge, after device events
/// are committed and before any thread they woke can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseCycleArrival {
    HostAdvanceCommitted,
    InstructionCheckpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationReleaseBoundary {
    cycle: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseViEdgeError {
    NonMonotonic { current: u64, next_vi: u64 },
    GateBeforeNextVi { gate: u64, next_vi: u64 },
}

impl std::fmt::Display for ReleaseViEdgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonMonotonic { current, next_vi } => write!(
                formatter,
                "next VI edge {next_vi} must be later than current guest cycle {current}"
            ),
            Self::GateBeforeNextVi { gate, next_vi } => write!(
                formatter,
                "release gate cycle {gate} is not a scheduled VI edge; next VI edge is {next_vi}"
            ),
        }
    }
}

impl std::error::Error for ReleaseViEdgeError {}

/// Select the device fabric's next VI edge without manufacturing an arbitrary
/// clamped host deadline. A later gate waits through ordinary VI edges; a gate
/// before the authoritative next edge is impossible and fails closed.
pub const fn select_release_vi_edge(
    current: u64,
    next_vi: u64,
    release_gate: Option<u64>,
) -> Result<u64, ReleaseViEdgeError> {
    if next_vi <= current {
        return Err(ReleaseViEdgeError::NonMonotonic { current, next_vi });
    }
    if let Some(gate) = release_gate {
        if gate < next_vi {
            return Err(ReleaseViEdgeError::GateBeforeNextVi { gate, next_vi });
        }
    }
    Ok(next_vi)
}

/// Opaque proof that the host committed the device fabric's exact scheduled VI
/// edge and has not resumed a guest thread since. Device, executor-control,
/// ABI HostState, executable-program, host-platform, and renderer evidence are
/// frozen at that edge, so later host input or configuration cannot silently
/// move the report environment or DeviceState artifact. Public callers can
/// move this value into a live release capture but cannot construct it.
#[derive(Debug)]
pub struct CommittedViBoundary {
    cycle: u64,
    resume_epoch: u64,
    trace_events: usize,
    device_trace_events: usize,
    save_operation_events: usize,
    controller_operation_events: usize,
    rsp_rdp_observations: Vec<fn64_abi::RspRdpObservationEvent>,
    native_execution_destinations: Vec<fn64_abi::NativeExecutionDestinationEvent>,
    #[cfg(feature = "recomp-rs")]
    function_execution_destinations:
        Vec<fn64_abi::recompiled::FunctionExecutionDestinationObservation>,
    #[cfg(feature = "recomp-rs")]
    block_execution_destinations: Vec<fn64_recomp_rs::ExecutionDestinationObservation>,
    device_snapshot: fn64_runtime::DeviceEvidenceSnapshot,
    executor_snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
    host_snapshot: fn64_abi::AbiHostEvidenceSnapshot,
    program_snapshot: ProgramEvidenceSnapshot,
    platform: ReleaseHostPlatform,
    render_snapshot: fn64_abi::RenderEnvironmentEvidenceSnapshot,
}

/// Exact host target on which a fixed-cycle report was produced.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseHostPlatform {
    MacosArm64,
    LinuxX86_64,
    WindowsX86_64,
}

/// Exact controller/accessory identity of one physical N64 port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseControllerPort {
    StandardControllerNoPak,
    StandardControllerControllerPak,
    StandardControllerRumblePak,
    StandardControllerTransferPak,
    VoiceRecognitionUnit,
    Absent,
}

/// Exact cartridge-mounted save hardware at the committed boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseCartridgeSave {
    NoCartridgeSave,
    Eeprom4k,
    Eeprom16k,
    Sram32Kib,
    FlashRam128Kib,
}

/// Graphics-microcode execution policy actually registered with the ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseGraphicsExecutionPolicy {
    HleOptimized,
    LleAccuracy,
}

/// Concrete registered renderer state, self-reported by the owned backend.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseRendererEvidence {
    Reference {
        execution_policy: ReleaseGraphicsExecutionPolicy,
    },
    Rt64 {
        execution_policy: ReleaseGraphicsExecutionPolicy,
        backend_identity: String,
        source_authoritative: bool,
        settings_sha256: String,
        replacement_packs_active: bool,
    },
}

/// Host environment observed from owners frozen at one committed VI edge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEnvironmentEvidence {
    pub platform: ReleaseHostPlatform,
    pub controller_ports: [ReleaseControllerPort; 4],
    pub cartridge_save: ReleaseCartridgeSave,
    pub renderer: ReleaseRendererEvidence,
}

const fn release_host_platform() -> Option<ReleaseHostPlatform> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(ReleaseHostPlatform::MacosArm64)
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(ReleaseHostPlatform::LinuxX86_64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(ReleaseHostPlatform::WindowsX86_64)
    } else {
        None
    }
}

/// Stable, pointer-free identity supplied by the build that owns a native
/// recompiled-program archive.
///
/// The bytes are intentionally opaque to this crate. The supplying build must
/// derive them from the exact linked archive set under a domain-separated
/// canonical wire; a function address or a hash of host pointer values is not
/// an artifact identity. Filesystem paths and timestamps are never fields in
/// release evidence, while any such metadata embedded in an archive remains
/// part of that archive's identity and may honestly cause build-to-build drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeProgramArtifactIdentity([u8; 32]);

impl NativeProgramArtifactIdentity {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn from_hex(value: &str) -> Result<Self, NativeProgramIdentityError> {
        if value.len() != 64 {
            return Err(NativeProgramIdentityError::WrongLength(value.len()));
        }
        let mut bytes = [0u8; 32];
        for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = decode_hex_nibble(chunk[0])
                .ok_or(NativeProgramIdentityError::InvalidHex { index: index * 2 })?;
            let low =
                decode_hex_nibble(chunk[1]).ok_or(NativeProgramIdentityError::InvalidHex {
                    index: index * 2 + 1,
                })?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

const fn decode_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeProgramIdentityError {
    WrongLength(usize),
    InvalidHex { index: usize },
}

impl std::fmt::Display for NativeProgramIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongLength(length) => write!(
                formatter,
                "native program artifact identity has {length} hexadecimal characters; expected 64"
            ),
            Self::InvalidHex { index } => write!(
                formatter,
                "native program artifact identity contains non-lowercase-hex data at character {index}"
            ),
        }
    }
}

impl std::error::Error for NativeProgramIdentityError {}

/// Host declaration of the executable-program class committed at a VI edge.
/// `NoProgram` is reserved for synthetic fixtures or hosts that truly execute
/// no recompiled program. Native/C release hosts must supply the exact identity
/// of their linked generated-code archive set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseProgramDescriptor {
    NoProgram,
    NativeArchive(NativeProgramArtifactIdentity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Program-owner contribution to a fixed-cycle DeviceState artifact.
pub enum ProgramEvidenceSnapshot {
    NoProgram,
    UnidentifiedNativeProgram,
    IdentifiedNativeArchive(NativeProgramArtifactIdentity),
    #[cfg(feature = "recomp-rs")]
    TypedRust(fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot),
}

/// Append-only execution histories frozen with the committed VI boundary.
/// Program evidence selects exactly one authoritative stream at report time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenExecutionDestinations {
    pub(crate) native: Vec<fn64_abi::NativeExecutionDestinationEvent>,
    #[cfg(feature = "recomp-rs")]
    pub(crate) function: Vec<fn64_abi::recompiled::FunctionExecutionDestinationObservation>,
    #[cfg(feature = "recomp-rs")]
    pub(crate) block: Vec<fn64_recomp_rs::ExecutionDestinationObservation>,
}

#[cfg(feature = "recomp-rs")]
fn copy_function_destinations_for_program(
    program: &ProgramEvidenceSnapshot,
) -> Vec<fn64_abi::recompiled::FunctionExecutionDestinationObservation> {
    if matches!(
        program,
        ProgramEvidenceSnapshot::TypedRust(
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Function { .. }
        )
    ) {
        // The ABI copy is also the observation-schema capability check. An
        // identity-only legacy install traps here instead of yielding release
        // evidence from a potentially incomplete entry stream.
        fn64_abi::recompiled::copy_function_execution_destinations()
    } else {
        Vec::new()
    }
}

type CommittedEvidence = (
    fn64_runtime::DeviceEvidenceSnapshot,
    fn64_runtime::ExecutorControlEvidenceSnapshot,
    fn64_abi::AbiHostEvidenceSnapshot,
    ProgramEvidenceSnapshot,
    FrozenExecutionDestinations,
    Vec<fn64_abi::RspRdpObservationEvent>,
    ReleaseHostPlatform,
    fn64_abi::RenderEnvironmentEvidenceSnapshot,
);

fn capture_program_evidence(
    descriptor: Option<ReleaseProgramDescriptor>,
) -> ProgramEvidenceSnapshot {
    #[cfg(feature = "recomp-rs")]
    {
        if let Some(program) = fn64_abi::recompiled::recompiled_program_evidence_snapshot() {
            assert!(
                !matches!(descriptor, Some(ReleaseProgramDescriptor::NativeArchive(_))),
                "release program descriptor names a native archive while a typed-Rust recompiled program is installed"
            );
            return ProgramEvidenceSnapshot::TypedRust(program);
        }
    }
    match descriptor {
        None => ProgramEvidenceSnapshot::UnidentifiedNativeProgram,
        Some(ReleaseProgramDescriptor::NoProgram) => ProgramEvidenceSnapshot::NoProgram,
        Some(ReleaseProgramDescriptor::NativeArchive(identity)) => {
            ProgramEvidenceSnapshot::IdentifiedNativeArchive(identity)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViBoundaryError {
    ViNotScheduled,
    WrongScheduledCycle {
        expected: u64,
        scheduled: u64,
    },
    NonMonotonic {
        current: u64,
        scheduled: u64,
    },
    WrongCommittedCycle {
        expected: u64,
        observed: u64,
    },
    MissingViInterrupt {
        cycle: u64,
    },
    GuestStateAdvanced,
    UnsupportedReleasePlatform {
        os: &'static str,
        arch: &'static str,
    },
}

impl std::fmt::Display for ViBoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ViNotScheduled => write!(formatter, "device fabric has no scheduled VI edge"),
            Self::WrongScheduledCycle {
                expected,
                scheduled,
            } => write!(
                formatter,
                "requested VI edge {expected} does not match scheduled edge {scheduled}"
            ),
            Self::NonMonotonic { current, scheduled } => write!(
                formatter,
                "scheduled VI edge {scheduled} must be later than current cycle {current}"
            ),
            Self::WrongCommittedCycle { expected, observed } => write!(
                formatter,
                "VI edge commit reached cycle {observed}, expected {expected}"
            ),
            Self::MissingViInterrupt { cycle } => write!(
                formatter,
                "device trace contains no VI interrupt committed at cycle {cycle}"
            ),
            Self::GuestStateAdvanced => write!(
                formatter,
                "guest, device, or save trace advanced after the committed VI boundary"
            ),
            Self::UnsupportedReleasePlatform { os, arch } => write!(
                formatter,
                "fixed-cycle release evidence has no closed platform identity for {os}/{arch}"
            ),
        }
    }
}

impl std::error::Error for ViBoundaryError {}

pub fn commit_scheduled_vi_boundary(
    expected_cycle: u64,
) -> Result<CommittedViBoundary, ViBoundaryError> {
    commit_scheduled_vi_boundary_inner(expected_cycle, None)
}

/// Commit an exact VI edge with an explicit native/no-program declaration.
///
/// Native/C release hosts must use `NativeArchive`; `NoProgram` is an explicit
/// assertion that no recompiled executable participates in the scenario. The
/// legacy [`commit_scheduled_vi_boundary`] API remains available for ordinary
/// observation, but its unidentified-native marker is rejected by live release
/// capture.
pub fn commit_scheduled_vi_boundary_with_program(
    expected_cycle: u64,
    descriptor: ReleaseProgramDescriptor,
) -> Result<CommittedViBoundary, ViBoundaryError> {
    commit_scheduled_vi_boundary_inner(expected_cycle, Some(descriptor))
}

fn commit_scheduled_vi_boundary_inner(
    expected_cycle: u64,
    descriptor: Option<ReleaseProgramDescriptor>,
) -> Result<CommittedViBoundary, ViBoundaryError> {
    let platform = release_host_platform().ok_or(ViBoundaryError::UnsupportedReleasePlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;
    let current = fn64_abi::sim_time();
    let scheduled = fn64_abi::next_vi_deadline().ok_or(ViBoundaryError::ViNotScheduled)?;
    if scheduled != expected_cycle {
        return Err(ViBoundaryError::WrongScheduledCycle {
            expected: expected_cycle,
            scheduled,
        });
    }
    if scheduled <= current {
        return Err(ViBoundaryError::NonMonotonic { current, scheduled });
    }
    let device_start = fn64_abi::copy_device_trace().len();
    fn64_abi::advance_virtual_time(scheduled);
    let observed = fn64_abi::sim_time();
    if observed != scheduled {
        return Err(ViBoundaryError::WrongCommittedCycle {
            expected: scheduled,
            observed,
        });
    }
    let device_trace = fn64_abi::copy_device_trace();
    let committed_vi = device_trace[device_start..].iter().any(|event| {
        event.at.get() == scheduled
            && matches!(event.kind, fn64_runtime::DeviceTraceKind::ViInterrupt)
    });
    if !committed_vi {
        return Err(ViBoundaryError::MissingViInterrupt { cycle: scheduled });
    }
    let program_snapshot = capture_program_evidence(descriptor);
    #[cfg(feature = "recomp-rs")]
    let function_execution_destinations = copy_function_destinations_for_program(&program_snapshot);
    Ok(CommittedViBoundary {
        cycle: scheduled,
        resume_epoch: fn64_abi::executor_resume_epoch(),
        trace_events: fn64_abi::copy_trace().len(),
        device_trace_events: device_trace.len(),
        save_operation_events: fn64_abi::copy_save_operations().len(),
        controller_operation_events: fn64_abi::copy_controller_operations().len(),
        rsp_rdp_observations: fn64_abi::copy_rsp_rdp_observations(),
        native_execution_destinations: fn64_abi::copy_native_execution_destinations(),
        #[cfg(feature = "recomp-rs")]
        function_execution_destinations,
        #[cfg(feature = "recomp-rs")]
        block_execution_destinations: fn64_abi::recompiled::copy_block_execution_destinations(),
        device_snapshot: fn64_abi::device_evidence_snapshot(),
        executor_snapshot: fn64_abi::executor_control_evidence_snapshot(),
        host_snapshot: fn64_abi::host_evidence_snapshot(),
        program_snapshot,
        platform,
        render_snapshot: fn64_abi::render_environment_evidence_snapshot(),
    })
}

impl CommittedViBoundary {
    pub const fn cycle(&self) -> u64 {
        self.cycle
    }

    pub(crate) fn validate_unconsumed(&self) -> Result<(), ViBoundaryError> {
        let unchanged = fn64_abi::sim_time() == self.cycle
            && fn64_abi::executor_resume_epoch() == self.resume_epoch
            && fn64_abi::copy_trace().len() == self.trace_events
            && fn64_abi::copy_device_trace().len() == self.device_trace_events
            && fn64_abi::copy_save_operations().len() == self.save_operation_events
            && fn64_abi::copy_controller_operations().len() == self.controller_operation_events
            && fn64_abi::copy_rsp_rdp_observations() == self.rsp_rdp_observations
            && fn64_abi::copy_native_execution_destinations() == self.native_execution_destinations
            && {
                #[cfg(feature = "recomp-rs")]
                {
                    copy_function_destinations_for_program(&self.program_snapshot)
                        == self.function_execution_destinations
                        && fn64_abi::recompiled::copy_block_execution_destinations()
                            == self.block_execution_destinations
                }
                #[cfg(not(feature = "recomp-rs"))]
                {
                    true
                }
            };
        if unchanged {
            Ok(())
        } else {
            Err(ViBoundaryError::GuestStateAdvanced)
        }
    }

    pub(crate) fn into_evidence(self) -> Result<CommittedEvidence, ViBoundaryError> {
        self.validate_unconsumed()?;
        Ok((
            self.device_snapshot,
            self.executor_snapshot,
            self.host_snapshot,
            self.program_snapshot,
            FrozenExecutionDestinations {
                native: self.native_execution_destinations,
                #[cfg(feature = "recomp-rs")]
                function: self.function_execution_destinations,
                #[cfg(feature = "recomp-rs")]
                block: self.block_execution_destinations,
            },
            self.rsp_rdp_observations,
            self.platform,
            self.render_snapshot,
        ))
    }

    #[cfg(test)]
    pub(crate) fn synthetic_for_test(cycle: u64) -> Self {
        Self {
            cycle,
            resume_epoch: fn64_abi::executor_resume_epoch(),
            trace_events: fn64_abi::copy_trace().len(),
            device_trace_events: fn64_abi::copy_device_trace().len(),
            save_operation_events: fn64_abi::copy_save_operations().len(),
            controller_operation_events: fn64_abi::copy_controller_operations().len(),
            rsp_rdp_observations: fn64_abi::copy_rsp_rdp_observations(),
            native_execution_destinations: fn64_abi::copy_native_execution_destinations(),
            #[cfg(feature = "recomp-rs")]
            function_execution_destinations: Vec::new(),
            #[cfg(feature = "recomp-rs")]
            block_execution_destinations: fn64_abi::recompiled::copy_block_execution_destinations(),
            device_snapshot: fn64_abi::device_evidence_snapshot(),
            executor_snapshot: fn64_abi::executor_control_evidence_snapshot(),
            host_snapshot: fn64_abi::host_evidence_snapshot(),
            program_snapshot: capture_program_evidence(Some(ReleaseProgramDescriptor::NoProgram)),
            platform: release_host_platform().expect("test platform must be release-supported"),
            render_snapshot: fn64_abi::render_environment_evidence_snapshot(),
        }
    }
}

impl PresentationReleaseBoundary {
    pub const fn new(cycle: u64) -> Self {
        Self { cycle }
    }

    pub const fn matches(self, arrival: ReleaseCycleArrival, observed_cycle: u64) -> bool {
        matches!(arrival, ReleaseCycleArrival::HostAdvanceCommitted) && observed_cycle == self.cycle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationDiscovery {
    floor: u64,
}

impl PresentationDiscovery {
    pub const fn floor(self) -> u64 {
        self.floor
    }

    pub const fn matches(
        self,
        arrival: ReleaseCycleArrival,
        host_cycle: u64,
        presentation_cycle: u64,
    ) -> bool {
        matches!(arrival, ReleaseCycleArrival::HostAdvanceCommitted)
            && host_cycle >= self.floor
            && presentation_cycle == host_cycle
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationDiscoveryError {
    InvalidFloor(String),
    ConflictsWithReleaseMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseEnvError {
    name: &'static str,
}

impl std::fmt::Display for ReleaseEnvError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} is present but is not valid Unicode",
            self.name
        )
    }
}

impl std::error::Error for ReleaseEnvError {}

pub fn parse_release_env_value(
    name: &'static str,
    value: Option<std::ffi::OsString>,
) -> Result<Option<String>, ReleaseEnvError> {
    value
        .map(|raw| raw.into_string().map_err(|_| ReleaseEnvError { name }))
        .transpose()
}

impl std::fmt::Display for PresentationDiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFloor(raw) => write!(
                formatter,
                "OOT_RELEASE_DISCOVER_PRESENTATION_AFTER must be an unsigned guest cycle, got {raw:?}"
            ),
            Self::ConflictsWithReleaseMode => write!(
                formatter,
                "OOT_RELEASE_DISCOVER_PRESENTATION_AFTER cannot be combined with quiescence discovery, OOT_RELEASE_GATE_CYCLE, or OOT_RELEASE_REPORT"
            ),
        }
    }
}

impl std::error::Error for PresentationDiscoveryError {}

pub fn parse_presentation_discovery(
    discovery_floor: Option<&str>,
    quiescent_discovery_present: bool,
    release_gate_cycle: Option<&str>,
    release_report_present: bool,
) -> Result<Option<PresentationDiscovery>, PresentationDiscoveryError> {
    let Some(raw) = discovery_floor else {
        return Ok(None);
    };
    if quiescent_discovery_present || release_gate_cycle.is_some() || release_report_present {
        return Err(PresentationDiscoveryError::ConflictsWithReleaseMode);
    }
    let floor = raw
        .parse::<u64>()
        .map_err(|_| PresentationDiscoveryError::InvalidFloor(raw.to_owned()))?;
    Ok(Some(PresentationDiscovery { floor }))
}

/// Track one scheduling drain without coupling virtual time to recompilation
/// yield granularity.
#[derive(Debug, Default)]
pub struct GuestDrain {
    ran_idle_thread: bool,
}

impl GuestDrain {
    pub fn before_step(&self, next_priority: Option<fn64_runtime::Priority>) -> DrainDecision {
        match next_priority {
            None => DrainDecision::AdvanceField,
            Some(fn64_runtime::OS_PRIORITY_IDLE) if self.ran_idle_thread => {
                DrainDecision::AdvanceField
            }
            Some(_) => DrainDecision::Step,
        }
    }

    pub fn record_step(&mut self, priority: fn64_runtime::Priority) {
        if priority == fn64_runtime::OS_PRIORITY_IDLE {
            self.ran_idle_thread = true;
        }
    }

    pub fn begin_field(&mut self) {
        self.ran_idle_thread = false;
    }
}

const OS_TV_TYPE: fn64_runtime::RdramAddr = fn64_runtime::RdramAddr::from_offset(0x300);

/// Length required by generated `MEM_*` accesses during boot.
pub const fn rdram_len() -> usize {
    let mmio_end = fn64_runtime::RDRAM_MMIO_WINDOW_END as usize;
    if DEFAULT_RDRAM_SIZE > mmio_end {
        DEFAULT_RDRAM_SIZE
    } else {
        mmio_end
    }
}

/// Allocate the process's single RDRAM buffer, including the sparse raw
/// MMIO/non-RDRAM window generated code can address directly, and seed the IPL-owned boot
/// globals that libultra/game initialization reads before any shim runs.
pub fn new_rdram(tv_type: TvType) -> Vec<u8> {
    fn64_abi::configure_tv_type(tv_type);
    let mut rdram = vec![0; rdram_len()];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u32(OS_TV_TYPE, tv_type as u32);
    rdram
}

/// Reproduce the IPL3 cartridge DMA that establishes the initial executable
/// image before `recomp_entrypoint` runs: one MiB from ROM `0x1000` to RDRAM
/// `0x400`. Translated CPU bodies do not need their own code bytes, but real
/// consumers such as `osSpTaskLoad` DMA rspboot from this resident image.
pub fn seed_ipl3_image(rdram: &mut [u8], rom: &[u8]) {
    const ROM_START: usize = 0x1000;
    const RDRAM_START: u32 = 0x400;
    const COPY_LEN: usize = 0x10_0000;
    let rom_end = ROM_START + COPY_LEN;
    let rdram_end = RDRAM_START as usize + COPY_LEN;
    assert!(
        rom.len() >= rom_end,
        "IPL3 initial DMA needs ROM bytes through {rom_end:#x}, got {:#x}",
        rom.len()
    );
    assert!(
        rdram.len() >= rdram_end,
        "IPL3 initial DMA needs RDRAM bytes through {rdram_end:#x}, got {:#x}",
        rdram.len()
    );
    fn64_runtime::RdramViewMut::from_storage(rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(RDRAM_START),
        &rom[ROM_START..rom_end],
    );
}

/// Copy game-declared always-resident images from their ROM ranges into their
/// static RDRAM ranges. Harness policy chooses which registered sections are
/// resident; this function only enforces and performs that geometry.
pub fn seed_resident_sections(rdram: &mut [u8], rom: &[u8], sections: &[(u32, u32, u32)]) {
    let mut view = fn64_runtime::RdramViewMut::from_storage(rdram);
    for &(rom_addr, ram_addr, size) in sections {
        assert!(
            size != 0,
            "resident section at ROM {rom_addr:#010x} is empty"
        );
        let rom_start = rom_addr as usize;
        let rom_end = rom_start
            .checked_add(size as usize)
            .expect("resident section ROM range overflow");
        assert!(
            rom_end <= rom.len(),
            "resident section ROM range [{rom_start:#x}, {rom_end:#x}) exceeds ROM length {:#x}",
            rom.len()
        );
        let rdram_addr = fn64_runtime::RdramAddr::from_gpr(u64::from(ram_addr));
        let rdram_end = rdram_addr.offset() as usize + size as usize;
        assert!(
            rdram_end <= view.len(),
            "resident section RDRAM range [{:#x}, {rdram_end:#x}) exceeds allocation {:#x}",
            rdram_addr.offset(),
            view.len()
        );
        view.write_logical_bytes(rdram_addr, &rom[rom_start..rom_end]);
    }
}

#[cfg(feature = "c-bridge")]
#[allow(improper_ctypes)]
extern "C" {
    fn fn64_bridge_register_all_sections();
    fn fn64_bridge_num_sections() -> usize;
    fn recomp_entrypoint(rdram: *mut u8, ctx: *mut fn64_abi::RecompContext);
}

/// One section registered from the generated `section_table[]`.
#[cfg(feature = "c-bridge")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredSection {
    pub source_index: usize,
    pub registry_index: fn64_runtime::SectionIndex,
    pub rom_addr: u32,
    pub ram_addr: u32,
    pub size: u32,
    pub function_count: usize,
}

/// Result of walking and registering one linked generated section table.
#[cfg(feature = "c-bridge")]
#[derive(Debug)]
pub struct SectionRegistration {
    reported_count: usize,
    sections: Vec<RegisteredSection>,
}

#[cfg(feature = "c-bridge")]
impl SectionRegistration {
    /// Number of entries reported by the generated `section_table[]`.
    pub fn reported_count(&self) -> usize {
        self.reported_count
    }

    /// Registered sections, ordered by their generated table index.
    pub fn sections(&self) -> &[RegisteredSection] {
        &self.sections
    }

    /// Runtime registry index corresponding to a generated table index.
    pub fn registry_index(&self, source_index: usize) -> Option<fn64_runtime::SectionIndex> {
        self.sections
            .iter()
            .find(|section| section.source_index == source_index)
            .map(|section| section.registry_index)
    }
}

#[cfg(feature = "c-bridge")]
type SectionEntry = (u32, u32, u32, Vec<(u32, u32, fn64_abi::RecompFunc)>);

#[cfg(feature = "c-bridge")]
#[derive(Default)]
struct SectionBuilder {
    sections: HashMap<usize, SectionEntry>,
}

#[cfg(feature = "c-bridge")]
thread_local! {
    static SECTION_BUILDER: std::cell::RefCell<SectionBuilder> =
        std::cell::RefCell::new(SectionBuilder::default());
}

/// Receive one `(section, function)` pair from `bridge/section_bridge.c`.
///
/// The C bridge emits one callback per `FuncEntry`; `fn64-abi` accepts a
/// complete function list per section, so this process-global accumulator is
/// the single adapter between those contracts.
#[cfg(feature = "c-bridge")]
#[no_mangle]
extern "C" fn fn64_register_func(
    section_index: usize,
    rom_addr: u32,
    ram_addr: u32,
    size: u32,
    offset: u32,
    rom_size: u32,
    func: fn64_abi::RecompFunc,
) {
    SECTION_BUILDER.with(|cell| {
        let mut builder = cell.borrow_mut();
        let entry = builder
            .sections
            .entry(section_index)
            .or_insert_with(|| (rom_addr, ram_addr, size, Vec::new()));
        entry.3.push((offset, rom_size, func));
    });
}

/// Walk the linked generated section table and register every section with
/// `fn64-abi` in generated-index order.
///
/// This is safe for harness callers because the bundled C bridge obtains all
/// function pointers from file-scope generated `FuncEntry` definitions,
/// satisfying `fn64_abi::register_section`'s process-lifetime requirement.
#[cfg(feature = "c-bridge")]
pub fn register_linked_sections() -> SectionRegistration {
    SECTION_BUILDER.with(|cell| cell.borrow_mut().sections.clear());

    // SAFETY: these two symbols are defined by the bundled bridge compiled
    // against the linked generated table. Its walk invokes the callback above
    // synchronously and its count is a plain read of generated `num_sections`.
    unsafe { fn64_bridge_register_all_sections() };
    let reported_count = unsafe { fn64_bridge_num_sections() };

    let sections = SECTION_BUILDER.with(|cell| {
        let builder = cell.borrow();
        let mut keys: Vec<_> = builder.sections.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter()
            .map(|source_index| {
                let (rom_addr, ram_addr, size, funcs) = &builder.sections[&source_index];
                // SAFETY: every pointer came directly from a file-scope
                // generated FuncEntry and remains valid for the process.
                let registry_index =
                    unsafe { fn64_abi::register_section(*rom_addr, *ram_addr, *size, funcs) };
                RegisteredSection {
                    source_index,
                    registry_index,
                    rom_addr: *rom_addr,
                    ram_addr: *ram_addr,
                    size: *size,
                    function_count: funcs.len(),
                }
            })
            .collect()
    });

    SectionRegistration {
        reported_count,
        sections,
    }
}

/// The linked generated C boot entry point.
#[cfg(feature = "c-bridge")]
pub fn c_recomp_entrypoint() -> fn64_abi::RecompFunc {
    recomp_entrypoint
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_synthetic_boundary(cycle: u64) -> Result<CommittedViBoundary, ViBoundaryError> {
        commit_scheduled_vi_boundary_with_program(cycle, ReleaseProgramDescriptor::NoProgram)
    }

    #[test]
    fn rdram_length_covers_physical_memory_and_raw_mmio_window() {
        assert!(rdram_len() >= DEFAULT_RDRAM_SIZE);
        assert!(rdram_len() >= fn64_runtime::RDRAM_MMIO_WINDOW_END as usize);
    }

    #[test]
    fn native_program_identity_parser_is_exact_and_lowercase() {
        let value = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let identity = NativeProgramArtifactIdentity::from_hex(value).unwrap();
        assert_eq!(
            identity.bytes()[0..8],
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]
        );
        assert!(matches!(
            NativeProgramArtifactIdentity::from_hex("00"),
            Err(NativeProgramIdentityError::WrongLength(2))
        ));
        let uppercase = "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(matches!(
            NativeProgramArtifactIdentity::from_hex(uppercase),
            Err(NativeProgramIdentityError::InvalidHex { index: 10 })
        ));
    }

    #[test]
    fn television_standard_is_explicit_boot_state_not_zero_fill_accident() {
        for (tv_type, expected) in [(TvType::Pal, 0), (TvType::Ntsc, 1), (TvType::Mpal, 2)] {
            let rdram = new_rdram(tv_type);
            assert_eq!(
                fn64_runtime::RdramView::from_storage(&rdram).read_u32(OS_TV_TYPE),
                expected
            );
            assert_eq!(fn64_abi::configured_tv_type(), tv_type);
            assert_eq!(
                fn64_abi::vi_field_interval(),
                Some(tv_type.nominal_field_cycles())
            );
        }
    }

    #[test]
    fn ipl3_image_seeding_uses_the_public_rom_and_rdram_ranges() {
        let mut rom = vec![0u8; 0x10_1000];
        rom[0x1000] = 0x12;
        rom[0x10_0fff] = 0x34;
        let mut rdram = vec![0u8; 0x10_0400];

        seed_ipl3_image(&mut rdram, &rom);

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        assert_eq!(
            view.read_u8(fn64_runtime::RdramAddr::from_offset(0x400)),
            0x12
        );
        assert_eq!(
            view.read_u8(fn64_runtime::RdramAddr::from_offset(0x10_03ff)),
            0x34
        );
    }

    #[test]
    fn resident_section_seeding_obeys_registered_geometry() {
        let mut rom = vec![0u8; 0x40];
        rom[0x20..0x24].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        let mut rdram = vec![0u8; 0x80];

        seed_resident_sections(&mut rdram, &rom, &[(0x20, 0x8000_0040, 4)]);

        let mut actual = [0u8; 4];
        fn64_runtime::RdramView::from_storage(&rdram)
            .copy_logical_bytes(fn64_runtime::RdramAddr::from_offset(0x40), &mut actual);
        assert_eq!(actual, [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn guest_drain_uses_idle_quiescence_not_a_resume_count() {
        let mut drain = GuestDrain::default();

        for _ in 0..250 {
            assert_eq!(drain.before_step(Some(10)), DrainDecision::Step);
            drain.record_step(10);
        }
        assert_eq!(drain.before_step(Some(0)), DrainDecision::Step);
        drain.record_step(0);
        assert_eq!(drain.before_step(Some(0)), DrainDecision::AdvanceField);

        drain.begin_field();
        assert_eq!(drain.before_step(Some(0)), DrainDecision::Step);
        assert_eq!(drain.before_step(None), DrainDecision::AdvanceField);
    }

    #[test]
    fn quiescent_discovery_parses_conflicts_and_requires_a_real_boundary() {
        assert_eq!(parse_quiescent_discovery(None, None, false).unwrap(), None);
        assert_eq!(
            parse_quiescent_discovery(Some("nope"), None, false),
            Err(QuiescentDiscoveryError::InvalidFloor("nope".to_owned()))
        );
        assert_eq!(
            parse_quiescent_discovery(Some("10"), Some("20"), false),
            Err(QuiescentDiscoveryError::ConflictsWithReleaseGate)
        );
        assert_eq!(
            parse_quiescent_discovery(Some("10"), None, true),
            Err(QuiescentDiscoveryError::ConflictsWithReleaseGate)
        );

        let discovery = parse_quiescent_discovery(Some("10"), None, false)
            .unwrap()
            .unwrap();
        assert!(!discovery.matches(DrainDecision::AdvanceField, 9));
        assert!(!discovery.matches(DrainDecision::Step, 10));
        assert!(discovery.matches(DrainDecision::AdvanceField, 10));
        assert!(discovery.matches(DrainDecision::AdvanceField, 11));
    }

    #[test]
    fn presentation_boundary_requires_host_advance_and_exact_capture_cycle() {
        let boundary = PresentationReleaseBoundary::new(20);
        assert!(!boundary.matches(ReleaseCycleArrival::InstructionCheckpoint, 20));
        assert!(!boundary.matches(ReleaseCycleArrival::HostAdvanceCommitted, 19));
        assert!(boundary.matches(ReleaseCycleArrival::HostAdvanceCommitted, 20));

        assert_eq!(
            parse_presentation_discovery(Some("bad"), false, None, false),
            Err(PresentationDiscoveryError::InvalidFloor("bad".to_owned()))
        );
        assert_eq!(
            parse_presentation_discovery(Some("10"), true, None, false),
            Err(PresentationDiscoveryError::ConflictsWithReleaseMode)
        );
        let discovery = parse_presentation_discovery(Some("10"), false, None, false)
            .unwrap()
            .unwrap();
        assert!(!discovery.matches(ReleaseCycleArrival::InstructionCheckpoint, 10, 10));
        assert!(!discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 9, 9));
        assert!(!discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 10, 9));
        assert!(discovery.matches(ReleaseCycleArrival::HostAdvanceCommitted, 10, 10));

        assert_eq!(select_release_vi_edge(10, 20, None), Ok(20));
        assert_eq!(select_release_vi_edge(10, 20, Some(30)), Ok(20));
        assert_eq!(select_release_vi_edge(10, 20, Some(20)), Ok(20));
        assert_eq!(
            select_release_vi_edge(10, 10, None),
            Err(ReleaseViEdgeError::NonMonotonic {
                current: 10,
                next_vi: 10
            })
        );
        assert_eq!(
            select_release_vi_edge(10, 20, Some(19)),
            Err(ReleaseViEdgeError::GateBeforeNextVi {
                gate: 19,
                next_vi: 20
            })
        );
    }

    #[test]
    fn committed_vi_boundary_is_exact_and_expires_after_further_execution() {
        unsafe extern "C" fn return_immediately(
            _rdram: *mut u8,
            _ctx: *mut fn64_abi::RecompContext,
        ) {
        }

        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        assert!(matches!(
            commit_scheduled_vi_boundary_with_program(
                scheduled - 1,
                ReleaseProgramDescriptor::NoProgram,
            ),
            Err(ViBoundaryError::WrongScheduledCycle { .. })
        ));

        let boundary = commit_synthetic_boundary(scheduled).unwrap();
        assert_eq!(boundary.cycle(), scheduled);
        assert_eq!(boundary.validate_unconsumed(), Ok(()));
        fn64_abi::set_trace_enabled(false);
        let mut rdram = vec![0u8; 0x100];
        unsafe {
            fn64_abi::boot_thread0(rdram.as_mut_ptr(), rdram.len(), return_immediately, 99, 10);
        }
        assert!(fn64_abi::run_one_step());
        assert_eq!(fn64_abi::sim_time(), scheduled);
        assert_eq!(
            boundary.validate_unconsumed(),
            Err(ViBoundaryError::GuestStateAdvanced)
        );
    }

    #[test]
    fn committed_vi_boundary_freezes_runtime_evidence_at_the_edge() {
        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        let boundary = commit_synthetic_boundary(scheduled).unwrap();
        let edge_device = boundary.device_snapshot.clone();
        let edge_executor = boundary.executor_snapshot.clone();
        let edge_host = boundary.host_snapshot.clone();
        let edge_peripherals = edge_host.runtime_peripherals.clone();

        fn64_abi::set_controller_port_state(
            0,
            fn64_runtime::PortState::StandardControllerRumblePak,
        );
        fn64_abi::set_controller_state(0, 0xa55a, -37, 63);
        let black = edge_peripherals
            .peripherals
            .vi
            .next_blanked
            .is_none_or(|queued| !queued);
        // An all-zero context is a valid integer-only ABI call frame; this
        // shim reads only r4 and ignores the RDRAM pointer.
        let mut context: fn64_abi::RecompContext = unsafe { std::mem::zeroed() };
        context.r4 = u64::from(black);
        unsafe {
            fn64_abi::osViBlack_recomp(std::ptr::null_mut(), &mut context);
        }

        assert_ne!(fn64_abi::peripherals_evidence_snapshot(), edge_peripherals);
        assert_eq!(boundary.validate_unconsumed(), Ok(()));
        let (
            captured_device,
            captured_executor,
            captured_host,
            _captured_program,
            _captured_destinations,
            _captured_rsp_rdp,
            _captured_platform,
            _captured_renderer,
        ) = boundary.into_evidence().unwrap();
        assert_eq!(captured_device, edge_device);
        assert_eq!(captured_executor, edge_executor);
        assert_eq!(captured_host, edge_host);
    }

    #[test]
    fn committed_vi_boundary_expires_after_a_controller_operation() {
        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        fn64_abi::set_controller_port_state(0, fn64_runtime::PortState::StandardControllerNoPak);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        let boundary = commit_synthetic_boundary(scheduled).unwrap();

        let mut rdram = vec![0u8; 4 * 6];
        let mut context: fn64_abi::RecompContext = unsafe { std::mem::zeroed() };
        context.r4 = 0x8000_0000;
        unsafe {
            fn64_abi::osContGetReadData_recomp(rdram.as_mut_ptr(), &mut context);
        }

        assert_eq!(
            boundary.validate_unconsumed(),
            Err(ViBoundaryError::GuestStateAdvanced)
        );
    }

    #[test]
    fn committed_vi_boundary_expires_after_native_destination_entry() {
        unsafe extern "C" fn entered_after_boundary(
            _rdram: *mut u8,
            _ctx: *mut fn64_abi::RecompContext,
        ) {
        }

        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        let boundary = commit_synthetic_boundary(scheduled).unwrap();
        unsafe {
            fn64_abi::register_section(
                0x0010_0000,
                0x8000_2000,
                4,
                &[(0, 4, entered_after_boundary)],
            );
        }
        fn64_abi::fn64_c_recompiled_function_enter(entered_after_boundary);

        assert_eq!(
            boundary.validate_unconsumed(),
            Err(ViBoundaryError::GuestStateAdvanced)
        );
    }

    #[cfg(feature = "recomp-rs")]
    fn observed_function_lookup(_vram: u32) -> fn64_recomp_rs::RecompFunc {
        fn observed_function(
            _ctx: &mut fn64_recomp_rs::RecompContext,
            _rdram: &mut fn64_recomp_rs::Rdram<'_>,
        ) {
        }
        observed_function
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn committed_vi_boundary_freezes_observed_function_destinations() {
        std::thread::spawn(|| {
            use fn64_recomp_rs::{
                ProgramArtifactIdentity, TranslatedFunctionIdentity,
                FUNCTION_ENTRY_OBSERVATION_SCHEMA,
            };

            fn64_abi::load_rom(Vec::new());
            fn64_abi::recompiled::set_entry_lookup_with_execution_observation(
                observed_function_lookup,
                0x100,
                ProgramArtifactIdentity::new([0x5a; 32]),
                FUNCTION_ENTRY_OBSERVATION_SCHEMA,
            );
            fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
                0x8000_1000,
                "entry",
            ));
            fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
            let scheduled = fn64_abi::next_vi_deadline().unwrap();
            let boundary = commit_scheduled_vi_boundary(scheduled).unwrap();
            assert_eq!(boundary.function_execution_destinations.len(), 1);
            assert_eq!(boundary.validate_unconsumed(), Ok(()));

            fn64_recomp_rs::notify_function_entry(TranslatedFunctionIdentity::new(
                0x8000_2000,
                "callee",
            ));
            assert_eq!(
                boundary.validate_unconsumed(),
                Err(ViBoundaryError::GuestStateAdvanced)
            );
        })
        .join()
        .unwrap();
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn committed_vi_boundary_rejects_identity_only_function_lane() {
        std::thread::spawn(|| {
            fn64_abi::load_rom(Vec::new());
            fn64_abi::recompiled::set_entry_lookup_with_artifact_identity(
                observed_function_lookup,
                0x100,
                fn64_recomp_rs::ProgramArtifactIdentity::new([0x5b; 32]),
            );
            fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
            let scheduled = fn64_abi::next_vi_deadline().unwrap();
            let failure = std::panic::catch_unwind(|| commit_scheduled_vi_boundary(scheduled))
                .expect_err("identity-only function lane must fail the observation-schema gate");
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(message.contains("entry-observation schema"));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn live_gate_rejects_expired_boundary_without_writing_a_report() {
        unsafe extern "C" fn return_immediately(
            _rdram: *mut u8,
            _ctx: *mut fn64_abi::RecompContext,
        ) {
        }

        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        let mut gate = LiveReleaseGate::new(scheduled);
        gate.arm().unwrap();
        fn64_abi::set_trace_enabled(false);
        let boundary = commit_synthetic_boundary(scheduled).unwrap();

        let mut rdram = vec![0u8; 0x100];
        unsafe {
            fn64_abi::boot_thread0(rdram.as_mut_ptr(), rdram.len(), return_immediately, 100, 10);
        }
        assert!(fn64_abi::run_one_step());
        assert_eq!(fn64_abi::sim_time(), scheduled);

        let path = std::env::temp_dir().join(format!(
            "fn64-expired-boundary-{}-{scheduled}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let result = gate.capture_and_write_observed(
            boundary,
            "expired-boundary",
            b"input",
            release_gate::LiveObservedArtifacts {
                framebuffer_artifact_bytes: b"framebuffer",
                framebuffer_payload_bytes: 2,
                memory_bytes: b"memory",
                observations: ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            },
            &path,
        );
        assert!(matches!(
            result,
            Err(GateError::InvalidViBoundary(
                ViBoundaryError::GuestStateAdvanced
            ))
        ));
        assert!(!path.exists());
    }

    #[test]
    fn live_gate_rejects_legacy_unidentified_native_boundary() {
        fn64_abi::load_rom(Vec::new());
        fn64_abi::configure_tv_type(fn64_runtime::TvType::Ntsc);
        let scheduled = fn64_abi::next_vi_deadline().unwrap();
        let mut gate = LiveReleaseGate::new(scheduled);
        gate.arm().unwrap();
        let boundary = commit_scheduled_vi_boundary(scheduled).unwrap();
        let path = std::env::temp_dir().join(format!(
            "fn64-unidentified-native-{}-{scheduled}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let result = gate.capture_and_write_observed(
            boundary,
            "unidentified-native",
            b"input",
            release_gate::LiveObservedArtifacts {
                framebuffer_artifact_bytes: b"framebuffer",
                framebuffer_payload_bytes: 2,
                memory_bytes: b"memory",
                observations: ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap(),
            },
            &path,
        );
        assert!(matches!(result, Err(GateError::UnidentifiedNativeProgram)));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn release_env_presence_never_silently_discards_non_unicode_values() {
        use std::os::unix::ffi::OsStringExt as _;

        assert_eq!(
            parse_release_env_value("MODE", Some(std::ffi::OsString::from("10"))).unwrap(),
            Some("10".to_owned())
        );
        assert_eq!(parse_release_env_value("MODE", None).unwrap(), None);
        assert_eq!(
            parse_release_env_value("MODE", Some(std::ffi::OsString::from_vec(vec![0xff]))),
            Err(ReleaseEnvError { name: "MODE" })
        );
    }
}
