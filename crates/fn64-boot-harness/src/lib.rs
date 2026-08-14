//! Shared boot seam for the headless examples and `fn64-shell`.
//!
//! The generated-C lane presents one process-global section table through
//! `bridge/section_bridge.c`. This crate owns that bridge's Rust callback,
//! batches its per-function records into `fn64-abi` sections, exposes the
//! generated entry point, and allocates the one ABI-visible RDRAM buffer.
//! Game policy remains with each harness: which sections are resident,
//! controller input, save type, rendering, audio, and executor driving.

mod audio_output;
#[cfg(feature = "recomp-rs")]
mod boot_context;
mod certification_profile;
mod controller_input_schedule;
#[cfg(feature = "recomp-rs")]
mod generated_runner_build;
mod observation_evidence;
mod platform_certification;
#[cfg(feature = "recomp-rs")]
mod precompiled_admission;
mod private_fs;
mod private_input_admission;
mod private_release_series;
mod release_gate;
mod release_matrix;
mod release_program_build_receipt;
mod release_run_env;
mod render_evidence;
mod report_series;
mod unsupported_journal;

pub use audio_output::{
    pace_live_audio_output, wire_live_audio_output, LiveAudioOutput, LiveAudioPaceError,
};
#[cfg(feature = "recomp-rs")]
pub use boot_context::{load_boot_context, parse_boot_context, BootContextLoadError};
pub use certification_profile::{
    CertificationProfileError, CertificationProfileIdentity, CertificationRequirement,
    CertificationRequirementClass, CertificationRequirementRef, FullParityV1,
    FULL_PARITY_V1_DEFINITION_SHA256, FULL_PARITY_V1_SCHEMA,
};
pub use controller_input_schedule::{
    attach_controllers_for_driven_ports, parse_controller_input_schedule, ControllerInputPhase,
    ControllerInputSchedule, ControllerInputScheduleError, CONTROLLER_INPUT_SCHEDULE_SCHEMA,
};
#[cfg(feature = "recomp-rs")]
pub use generated_runner_build::{
    build_wm2000_generated_runner_v1, parse_generated_runner_bootstrap_runtime_report_v1,
    parse_generated_runner_cpu_runtime_report_v1,
    parse_generated_runner_host_abi_runtime_report_v1, parse_generated_runner_pi_runtime_report_v1,
    parse_generated_runner_rdp_renderer_runtime_report_v1,
    parse_generated_runner_rsp_runtime_report_v1, parse_generated_runner_si_runtime_report_v1,
    parse_generated_runner_sp_runtime_report_v1,
    run_wm2000_generated_runner_bootstrap_runtime_series_v1,
    run_wm2000_generated_runner_cpu_runtime_series_v1,
    run_wm2000_generated_runner_host_abi_runtime_series_v1,
    run_wm2000_generated_runner_pi_runtime_series_v1,
    run_wm2000_generated_runner_rdp_renderer_runtime_series_v1,
    run_wm2000_generated_runner_rsp_runtime_series_v1,
    run_wm2000_generated_runner_si_runtime_series_v1,
    run_wm2000_generated_runner_sp_runtime_series_v1, BootstrapAttributedWriteV1,
    BootstrapMutationBatchV1, BootstrapWriterChannelV1, BootstrapWriterRuntimePrerequisiteV1,
    BootstrapWriterWatchedRangeV1, CpuWriterRuntimePrerequisiteV1, CpuWriterWatchedRangeV1,
    GeneratedRunnerAdapterRoleV1, GeneratedRunnerBootstrapRuntimeReportV1,
    GeneratedRunnerBootstrapRuntimeSeriesEvidenceV1, GeneratedRunnerBuildError,
    GeneratedRunnerBuildEvidenceV1, GeneratedRunnerBuildIdentityV1,
    GeneratedRunnerCpuRuntimeReportV1, GeneratedRunnerCpuRuntimeSeriesEvidenceV1,
    GeneratedRunnerHostAbiRuntimeReportV1, GeneratedRunnerHostAbiRuntimeSeriesEvidenceV1,
    GeneratedRunnerLinkedIdentityV1, GeneratedRunnerPiRuntimeReportV1,
    GeneratedRunnerPiRuntimeSeriesEvidenceV1, GeneratedRunnerRdpRendererRuntimeReportV1,
    GeneratedRunnerRdpRendererRuntimeSeriesEvidenceV1, GeneratedRunnerRspRuntimeReportV1,
    GeneratedRunnerRspRuntimeSeriesEvidenceV1, GeneratedRunnerSiRuntimeReportV1,
    GeneratedRunnerSiRuntimeSeriesEvidenceV1, GeneratedRunnerSpRuntimeReportV1,
    GeneratedRunnerSpRuntimeSeriesEvidenceV1, GeneratedRunnerWriterAuditBundleEvidenceV1,
    GeneratedRunnerWriterAuditSessionV1, HostAbiWriterRuntimePrerequisiteV1,
    HostAbiWriterWatchedRangeV1, PiWriterRuntimePrerequisiteV1, PiWriterWatchedRangeV1,
    RdpRendererWriterRuntimePrerequisiteV1, RdpRendererWriterWatchedRangeV1,
    RspWriterRuntimePrerequisiteV1, RspWriterWatchedRangeV1, SiWriterRuntimePrerequisiteV1,
    SiWriterWatchedRangeV1, SpWriterRuntimePrerequisiteV1, SpWriterWatchedRangeV1,
    VerifiedGeneratedRunnerBootstrapRuntimeSeriesV1, VerifiedGeneratedRunnerBuildV1,
    VerifiedGeneratedRunnerCpuRuntimeSeriesV1, VerifiedGeneratedRunnerHostAbiRuntimeSeriesV1,
    VerifiedGeneratedRunnerPiRuntimeSeriesV1, VerifiedGeneratedRunnerRdpRendererRuntimeSeriesV1,
    VerifiedGeneratedRunnerRspRuntimeSeriesV1, VerifiedGeneratedRunnerSiRuntimeSeriesV1,
    VerifiedGeneratedRunnerSpRuntimeSeriesV1, VerifiedGeneratedRunnerWriterAuditBundleV1,
    Wm2000ExecutableImageGroupV1, Wm2000GeneratedRunnerBuildInputsV1,
    GENERATED_RUNNER_BOOTSTRAP_RUNTIME_ARGUMENT_V1,
    GENERATED_RUNNER_BOOTSTRAP_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_PREFIX_V1,
    GENERATED_RUNNER_BOOTSTRAP_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_BUILD_IDENTITY_ARGUMENT_V1, GENERATED_RUNNER_BUILD_IDENTITY_PREFIX_V1,
    GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V2, GENERATED_RUNNER_BUILD_IDENTITY_SCHEMA_V3,
    GENERATED_RUNNER_CPU_RUNTIME_ARGUMENT_V1, GENERATED_RUNNER_CPU_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_CPU_RUNTIME_REPORT_PREFIX_V1, GENERATED_RUNNER_CPU_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_HOST_ABI_RUNTIME_ARGUMENT_V1, GENERATED_RUNNER_HOST_ABI_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_PREFIX_V1,
    GENERATED_RUNNER_HOST_ABI_RUNTIME_REPORT_SCHEMA_V1, GENERATED_RUNNER_PI_RUNTIME_ARGUMENT_V1,
    GENERATED_RUNNER_PI_RUNTIME_NONCE_ENV_V1, GENERATED_RUNNER_PI_RUNTIME_REPORT_PREFIX_V1,
    GENERATED_RUNNER_PI_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_RDP_RENDERER_RUNTIME_ARGUMENT_V1,
    GENERATED_RUNNER_RDP_RENDERER_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_PREFIX_V1,
    GENERATED_RUNNER_RDP_RENDERER_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_RSP_RUNTIME_ARGUMENT_V1, GENERATED_RUNNER_RSP_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_RSP_RUNTIME_REPORT_PREFIX_V1, GENERATED_RUNNER_RSP_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_SI_RUNTIME_ARGUMENT_V1, GENERATED_RUNNER_SI_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_SI_RUNTIME_REPORT_PREFIX_V1, GENERATED_RUNNER_SI_RUNTIME_REPORT_SCHEMA_V1,
    GENERATED_RUNNER_SP_RUNTIME_ARGUMENT_V1, GENERATED_RUNNER_SP_RUNTIME_NONCE_ENV_V1,
    GENERATED_RUNNER_SP_RUNTIME_REPORT_PREFIX_V1, GENERATED_RUNNER_SP_RUNTIME_REPORT_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_BOOTSTRAP_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V2, VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V3,
    VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V4, VERIFIED_GENERATED_RUNNER_BUILD_SCHEMA_V5,
    VERIFIED_GENERATED_RUNNER_CPU_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_HOST_ABI_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_PI_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_RDP_RENDERER_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_RSP_SERIES_SCHEMA_V1, VERIFIED_GENERATED_RUNNER_SI_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_SP_SERIES_SCHEMA_V1,
    VERIFIED_GENERATED_RUNNER_WRITER_AUDIT_BUNDLE_SCHEMA_V1, WRITER_AUDIT_BOOTSTRAP_COMPLETED_V1,
    WRITER_AUDIT_CPU_COMPLETED_V1, WRITER_AUDIT_HOST_ABI_COMPLETED_V1,
    WRITER_AUDIT_PI_COMPLETED_V1, WRITER_AUDIT_RDP_RENDERER_COMPLETED_V1,
    WRITER_AUDIT_RSP_COMPLETED_V1, WRITER_AUDIT_SI_COMPLETED_V1, WRITER_AUDIT_SP_COMPLETED_V1,
};
pub use observation_evidence::{
    FramebufferObservationFormat, FramebufferObservationGeometry, FramebufferObservationSource,
    LiveReferenceFramebufferEvidence, LiveReleaseGateObservationExt, MemoryObservationGeometry,
    ObservationEvidenceError, ReleaseObservationGeometry,
};
pub use platform_certification::{
    emit_rt64_platform_child_identity, run_rt64_platform_case_series, PlatformCertificationError,
    PreflightedRt64PlatformCase, Rt64PlatformCase, Rt64PlatformTarget,
    VerifiedRt64PlatformCaseAuthority, VerifiedRt64PlatformCaseSeries,
    RT64_PLATFORM_CHILD_IDENTITY_SCHEMA, VERIFIED_RT64_PLATFORM_CASE_AUTHORITY_SCHEMA,
};
#[cfg(feature = "recomp-rs")]
pub use precompiled_admission::verify_precompiled_words;
pub use private_input_admission::{
    load_private_f3dzex2_characterization_input, PrivateF3dzex2CharacterizationError,
    VerifiedPrivateF3dzex2CharacterizationInput,
};
pub use private_release_series::{
    load_private_release_run_contract, run_private_release_series,
    run_synthetic_native_private_release_series, verify_private_release_series,
    verify_private_release_series_with_runner,
    verify_repository_synthetic_private_release_run_contract, PrivateArtifactIdentity,
    PrivateChildCommand, PrivateEnvironmentEntry, PrivateFileIdentity, PrivateReleaseRunContract,
    PrivateReleaseSeriesError, PrivateReleaseSeriesReceipt, PrivateReleaseSeriesRun,
    VerifiedPrivateReleaseRunContract, VerifiedPrivateReleaseSeries,
    PRIVATE_RELEASE_RUN_CONTRACT_SCHEMA, PRIVATE_RELEASE_SERIES_COUNT,
    PRIVATE_RELEASE_SERIES_RECEIPT_SCHEMA, RELEASE_MICROCODE_DATA_PATH_ENV,
    RELEASE_MICROCODE_TEXT_PATH_ENV, REPOSITORY_SYNTHETIC_NATIVE_RELEASE_SCENARIO,
    REPOSITORY_SYNTHETIC_RELEASE_CYCLE, REPOSITORY_SYNTHETIC_RELEASE_INPUT_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_MANIFEST_BYTES, REPOSITORY_SYNTHETIC_RELEASE_READINESS_BYTES,
    REPOSITORY_SYNTHETIC_RELEASE_SCENARIO,
};
pub use release_gate::{
    operational_state_component_digests_v1, ArtifactDigest, ArtifactKind, ClosureGate, ClosurePath,
    ClosurePathStatus, DeterministicDigest, ExecutionDestinationCountEvidence,
    ExecutionDestinationEventEvidence, ExecutionDestinationEvidence, ExecutionDestinationSource,
    FixedCycleDigestGate, GateError, LiveReleaseGate, OperationalStateComponentDigestsV1,
    ReleaseExecutionDestination, ReleaseGateReport, ReleaseMicrocodeFamily, ReleaseRomByteOrder,
    ReleaseRomClass, ReleaseRomEvidence, ReleaseRomInput, ReleaseTvRegion, ReleaseTvStandard,
    RspRdpEvidence, RspRdpObservationEventEvidence, RspRdpObservationKindEvidence,
    UnsupportedEvent, UnsupportedInstrumentationEvidence, LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS,
    LIVE_MINIMUM_CLOSURE_PATHS, LIVE_SAVE_OPERATION_CLOSURE_PATHS,
    OPERATIONAL_STATE_COMPONENT_DIGEST_SCHEMA_V1,
};
#[cfg(feature = "recomp-rs")]
pub use release_gate::{
    operational_thread_publication_digests_v1, operational_thread_publication_digests_v2,
    OperationalThreadPublicationDigestErrorV1, OperationalThreadPublicationDigestsV1,
    OperationalThreadPublicationDigestsV2, OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V1,
    OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V2,
};
pub use release_matrix::{
    verify_release_matrix, verify_release_matrix_with_platform_series,
    verify_release_matrix_with_private_and_platform_series,
    verify_release_matrix_with_private_series, CertificationRequirementAssignment,
    ControllerFeature, IncompleteReleaseMatrix, MicrocodeFeature, PresentationBoundaryEvidence,
    ProgramFeature, ReleaseMatrixCoverage, ReleaseMatrixError, ReleaseMatrixManifest,
    ReleaseMatrixScenario, ReleaseMatrixVerification, ReleasePlatform, RendererFeature,
    RspRdpMechanismFeature, SaveFeature, VerifiedMatrixScenario, VerifiedReleaseMatrix,
    VerifiedRomClassAuthority, INCOMPLETE_RELEASE_MATRIX_SCHEMA, RELEASE_MATRIX_MAX_SCENARIOS,
    RELEASE_MATRIX_REPORT_COUNT, RELEASE_MATRIX_SCHEMA, VERIFIED_RELEASE_MATRIX_SCHEMA,
    VERIFIED_ROM_CLASS_AUTHORITY_SCHEMA,
};
pub use release_program_build_receipt::{
    materialize_release_program_build_receipt, MaterializedReleaseProgramBuildReceipt,
    NativeArchiveReceiptInput, ReleaseProgramBuildReceiptError, ReleaseProgramBuildReceiptInput,
};
pub use release_run_env::{
    release_run_environment_from_process, release_run_environment_from_process_with_oot_aliases,
    ReleaseRunEnvironment, ReleaseRunEnvironmentError, RELEASE_GATE_CYCLE_ENV, RELEASE_REPORT_ENV,
    RELEASE_ROM_CLASS_ENV, RELEASE_RUN_EVENT_SHA256_ENV,
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

/// Result of advancing an idle headless host to the next authoritative device
/// deadline. A quiescent host may have to service several earlier DMA/RCP
/// completions before the exact VI edge becomes the next event; callers must
/// not infer a committed field from the control-flow branch that selected the
/// deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAdvance {
    DeviceEvent { through_cycle: u64 },
    ViFields { retrace_ticks: std::num::NonZeroU32 },
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
    windows_version: Option<ReleaseWindowsVersionEvidence>,
    render_snapshot: fn64_abi::RenderEnvironmentEvidenceSnapshot,
    fixed_cycle: FrozenFixedCycleObservations,
}

/// Raw observation channels copied while the committed VI edge owns the
/// single-threaded guest/device boundary. Production report construction
/// consumes these bytes; a boot host cannot substitute a later memory image
/// or extend an audio/trace history after the edge.
#[derive(Debug, Clone)]
pub(crate) struct FrozenFixedCycleObservations {
    pub(crate) physical_rdram_logical: Option<Vec<u8>>,
    pub(crate) audio_pcm_s16le: Option<Vec<u8>>,
    pub(crate) trace: Vec<fn64_runtime::TraceEvent>,
    pub(crate) device_trace: Vec<fn64_runtime::DeviceTraceEvent>,
    pub(crate) save_operations: Vec<fn64_runtime::SaveOperationEvent>,
    pub(crate) controller_operations: Vec<fn64_runtime::ControllerOperationEvent>,
    pub(crate) unsupported_events: Vec<fn64_runtime::UnsupportedEvent>,
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

/// Marketing-family classification derived from the native Windows kernel
/// build, never from a caller-supplied product label.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseWindowsFamily {
    Windows10,
    Windows11,
}

/// Product class returned by the native Windows version query.
///
/// Server builds intentionally have no representable release-evidence value.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseWindowsProductType {
    Workstation,
}

/// Exact native Windows version bound to one committed release boundary.
///
/// Host-source provenance: Microsoft Learn's public `OSVERSIONINFOEXW` and
/// `RtlGetVersion` documentation define the native layout/query and workstation
/// product type; the Windows release-health tables identify Windows 10 RTM as
/// build 10240 and Windows 11 21H2 as build 22000; the OEM deployment guide
/// identifies `HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\UBR` as
/// the Windows build revision.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(deny_unknown_fields)]
pub struct ReleaseWindowsVersionEvidence {
    pub family: ReleaseWindowsFamily,
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    pub update_build_revision: u32,
    pub product_type: ReleaseWindowsProductType,
}

impl ReleaseWindowsVersionEvidence {
    pub const WINDOWS_11_FIRST_BUILD: u32 = 22_000;
    pub const WINDOWS_10_FIRST_BUILD: u32 = 10_240;

    /// Classify native workstation version components. Windows Server and
    /// compatibility-layer product labels are rejected before this boundary.
    pub fn from_native_workstation(
        major: u32,
        minor: u32,
        build: u32,
        update_build_revision: u32,
    ) -> Result<Self, &'static str> {
        if major != 10 || minor != 0 {
            return Err("Windows release evidence requires native version 10.0");
        }
        if build < Self::WINDOWS_10_FIRST_BUILD {
            return Err("Windows release evidence predates Windows 10 build 10240");
        }
        let family = if build >= Self::WINDOWS_11_FIRST_BUILD {
            ReleaseWindowsFamily::Windows11
        } else {
            ReleaseWindowsFamily::Windows10
        };
        Ok(Self {
            family,
            major,
            minor,
            build,
            update_build_revision,
            product_type: ReleaseWindowsProductType::Workstation,
        })
    }

    pub fn verify(self) -> Result<(), &'static str> {
        if Self::from_native_workstation(
            self.major,
            self.minor,
            self.build,
            self.update_build_revision,
        ) == Ok(self)
        {
            Ok(())
        } else {
            Err("Windows family does not match its native version/build evidence")
        }
    }
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

/// Concrete graphics API that produced an RT64 release image.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseGraphicsApi {
    D3d12,
    Vulkan,
    Metal,
}

/// Concrete registered renderer state, self-reported by the owned backend.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseRendererEvidence {
    Reference {
        execution_policy: ReleaseGraphicsExecutionPolicy,
        tv_type: ReleaseTvStandard,
    },
    Rt64 {
        execution_policy: ReleaseGraphicsExecutionPolicy,
        tv_type: ReleaseTvStandard,
        graphics_api: ReleaseGraphicsApi,
        backend_identity: String,
        source_authoritative: bool,
        settings_sha256: String,
        replacement_packs_active: bool,
    },
}

/// Audio-task executor selected before guest task admission.
///
/// The translated identity binds the exact host artifact selected by the
/// owner, but it does not prove that artifact implements the live RSP IMEM
/// image. Release evidence therefore admits only [`Self::LleAccuracy`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseAudioTaskExecutionPolicy {
    Unconfigured,
    Translated { artifact_sha256: String },
    LleAccuracy,
    DiagnosticSkip,
}

impl ReleaseRendererEvidence {
    pub const fn tv_type(&self) -> ReleaseTvStandard {
        match self {
            Self::Reference { tv_type, .. } | Self::Rt64 { tv_type, .. } => *tv_type,
        }
    }
}

/// Host environment observed from owners frozen at one committed VI edge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEnvironmentEvidence {
    pub platform: ReleaseHostPlatform,
    /// Present exactly on Windows. The report schema rejects both a missing
    /// Windows version and a version attached to a non-Windows platform.
    pub windows_version: Option<ReleaseWindowsVersionEvidence>,
    pub controller_ports: [ReleaseControllerPort; 4],
    pub cartridge_save: ReleaseCartridgeSave,
    pub audio_task_execution: ReleaseAudioTaskExecutionPolicy,
    pub renderer: ReleaseRendererEvidence,
}

pub(crate) const fn release_host_platform() -> Option<ReleaseHostPlatform> {
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

pub(crate) fn release_host_identity(
) -> Result<(ReleaseHostPlatform, Option<ReleaseWindowsVersionEvidence>), ViBoundaryError> {
    let platform = release_host_platform().ok_or(ViBoundaryError::UnsupportedReleasePlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;
    #[cfg(target_os = "windows")]
    {
        let version = windows_native_version()
            .map_err(|detail| ViBoundaryError::UnsupportedWindowsVersion { detail })?;
        Ok((platform, Some(version)))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok((platform, None))
    }
}

#[cfg(test)]
pub(crate) fn test_release_windows_version() -> Option<ReleaseWindowsVersionEvidence> {
    #[cfg(target_os = "windows")]
    {
        Some(
            ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 19_045, 0)
                .expect("fixed test Windows identity is valid"),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_native_version() -> Result<ReleaseWindowsVersionEvidence, String> {
    const VER_NT_WORKSTATION: u8 = 1;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
    const ERROR_SUCCESS: i32 = 0;

    #[repr(C)]
    struct OsVersionInfoExW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        service_pack: [u16; 128],
        service_pack_major: u16,
        service_pack_minor: u16,
        suite_mask: u16,
        product_type: u8,
        reserved: u8,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(version: *mut OsVersionInfoExW) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(module_name: *const u16) -> isize;
        fn GetProcAddress(module: isize, procedure_name: *const u8) -> *const std::ffi::c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            value_type: *mut u32,
            data: *mut std::ffi::c_void,
            data_size: *mut u32,
        ) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let ntdll = wide("ntdll.dll");
    // SAFETY: both arguments are immutable NUL-terminated strings and the
    // returned module handle is inspected only for one exported symbol.
    let ntdll_module = unsafe { GetModuleHandleW(ntdll.as_ptr()) };
    if ntdll_module == 0 {
        return Err("GetModuleHandleW could not resolve ntdll.dll".to_owned());
    }
    // fn64's host policy rejects the conventional Wine marker documented by
    // WineHQ as an ntdll `wine_get_version` export. This is deliberately not a
    // claim that absence of the marker attests every compatibility layer away.
    let wine_symbol = b"wine_get_version\0";
    // SAFETY: `ntdll_module` came from GetModuleHandleW and the procedure name
    // is NUL-terminated for the duration of the lookup.
    if !unsafe { GetProcAddress(ntdll_module, wine_symbol.as_ptr()) }.is_null() {
        return Err(
            "Wine compatibility-layer hosts cannot produce Windows release evidence".into(),
        );
    }

    let mut version = OsVersionInfoExW {
        size: std::mem::size_of::<OsVersionInfoExW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform_id: 0,
        service_pack: [0; 128],
        service_pack_major: 0,
        service_pack_minor: 0,
        suite_mask: 0,
        product_type: 0,
        reserved: 0,
    };
    // SAFETY: `version` has the documented RTL_OSVERSIONINFOEXW layout and
    // remains exclusively borrowed for the duration of the native query.
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 {
        return Err(format!("RtlGetVersion failed with NTSTATUS {status:#010x}"));
    }
    if version.product_type != VER_NT_WORKSTATION {
        return Err(format!(
            "Windows product type {} is not a workstation",
            version.product_type
        ));
    }
    let current_version_key = wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let ubr_name = wide("UBR");
    let mut ubr = 0u32;
    let mut ubr_type = 0u32;
    let mut ubr_size = std::mem::size_of::<u32>() as u32;
    // SAFETY: all pointers name live, correctly sized storage; RegGetValueW
    // receives the fixed machine-wide CurrentVersion key and DWORD-only flag.
    let ubr_status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            current_version_key.as_ptr(),
            ubr_name.as_ptr(),
            RRF_RT_REG_DWORD,
            &mut ubr_type,
            (&mut ubr as *mut u32).cast(),
            &mut ubr_size,
        )
    };
    if ubr_status != ERROR_SUCCESS || ubr_type != 4 || ubr_size != std::mem::size_of::<u32>() as u32
    {
        return Err(format!(
            "Windows CurrentVersion UBR query failed: status={ubr_status}, type={ubr_type}, bytes={ubr_size}"
        ));
    }
    ReleaseWindowsVersionEvidence::from_native_workstation(
        version.major,
        version.minor,
        version.build,
        ubr,
    )
    .map_err(str::to_owned)
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
    Option<ReleaseWindowsVersionEvidence>,
    fn64_abi::RenderEnvironmentEvidenceSnapshot,
    FrozenFixedCycleObservations,
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
    UnsupportedWindowsVersion {
        detail: String,
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
            Self::UnsupportedWindowsVersion { detail } => {
                write!(
                    formatter,
                    "fixed-cycle Windows identity is unsupported: {detail}"
                )
            }
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
    let (platform, windows_version) = release_host_identity()?;
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
    let fixed_cycle = FrozenFixedCycleObservations {
        physical_rdram_logical: fn64_abi::copy_registered_physical_rdram_logical(),
        audio_pcm_s16le: fn64_abi::copy_audio_digest_bytes(),
        trace: fn64_abi::copy_trace(),
        device_trace: device_trace.clone(),
        save_operations: fn64_abi::copy_save_operations(),
        controller_operations: fn64_abi::copy_controller_operations(),
        unsupported_events: fn64_runtime::copy_unsupported_events(),
    };
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
        windows_version,
        render_snapshot: fn64_abi::render_environment_evidence_snapshot(),
        fixed_cycle,
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
            // Count, not contents. This check asks only whether anything
            // advanced since the boundary was committed, which is what every
            // sibling line above asks with `.len()`. The history only grows
            // within a ROM load, so a count detects an advance exactly as a
            // deep compare does -- while a deep compare also CLONES the whole
            // vector on every VI boundary, which is O(n) per field against an
            // n that grows ~2.8/field. That is quadratic over a session and is
            // the shape behind `pump_ms` climbing 12.7 -> 29.1 ms within one
            // windowed run. `into_evidence` below still carries the full
            // ordered sequence, so the release gate is unaffected.
            && fn64_abi::rsp_rdp_observation_count() == self.rsp_rdp_observations.len()
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
            self.windows_version,
            self.render_snapshot,
            self.fixed_cycle,
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
            windows_version: test_release_windows_version(),
            render_snapshot: fn64_abi::render_environment_evidence_snapshot(),
            fixed_cycle: FrozenFixedCycleObservations {
                physical_rdram_logical: fn64_abi::copy_registered_physical_rdram_logical(),
                audio_pcm_s16le: fn64_abi::copy_audio_digest_bytes(),
                trace: fn64_abi::copy_trace(),
                device_trace: fn64_abi::copy_device_trace(),
                save_operations: fn64_abi::copy_save_operations(),
                controller_operations: fn64_abi::copy_controller_operations(),
                unsupported_events: fn64_runtime::copy_unsupported_events(),
            },
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

    /// Advance a quiescent guest to its next exact device event.
    ///
    /// The device fabric owns the VI schedule. In particular, translated
    /// instruction checkpoints can move the shared clock past a host-owned
    /// interval accumulator, and another device deadline can coincide with or
    /// precede the VI edge. Snapshotting [`fn64_abi::next_vi_deadline`] before
    /// the advance makes the returned `ViFields` an observation of that armed
    /// edge rather than a prediction from elapsed time.
    pub fn advance_to_next_device_event(&mut self) -> DeviceAdvance {
        assert_eq!(
            self.before_step(fn64_abi::next_runnable_priority()),
            DrainDecision::AdvanceField,
            "GuestDrain::advance_to_next_device_event requires a quiescent guest"
        );
        let current = fn64_abi::sim_time();
        let next_vi = fn64_abi::next_vi_deadline()
            .expect("GuestDrain::advance_to_next_device_event requires an armed VI field");
        let target = fn64_abi::next_device_deadline()
            .expect("GuestDrain::advance_to_next_device_event: armed VI has no device deadline");
        assert!(
            target <= next_vi,
            "GuestDrain::advance_to_next_device_event: earliest device deadline {target} skips armed VI edge {next_vi}"
        );

        // Instruction checkpoints advance executor time before this idle
        // pump. If the fabric's earliest event is consequently overdue,
        // commit every event through the already-observed executor cycle;
        // asking the shared clock to move back to the raw deadline would be
        // invalid, while advancing only one nominal field would leave older
        // hardware work uncommitted.
        let through_cycle = target.max(current);
        let advance = fn64_abi::advance_virtual_time(through_cycle);
        match advance.vi_retrace_ticks() {
            0 => {
                assert!(
                    through_cycle < next_vi,
                    "GuestDrain::advance_to_next_device_event: armed VI edge {next_vi} committed without a reported retrace"
                );
                assert_eq!(
                    fn64_abi::next_vi_deadline(),
                    Some(next_vi),
                    "GuestDrain::advance_to_next_device_event: non-VI event rescheduled the armed VI edge"
                );
                DeviceAdvance::DeviceEvent { through_cycle }
            }
            ticks => {
                assert!(
                    next_vi <= through_cycle,
                    "GuestDrain::advance_to_next_device_event: {ticks} VI retrace(s) reported before armed edge {next_vi} at catch-up cycle {through_cycle}"
                );
                let following_vi = fn64_abi::next_vi_deadline().expect(
                    "GuestDrain::advance_to_next_device_event: committed VI edge was not rescheduled",
                );
                assert!(
                    following_vi > through_cycle,
                    "GuestDrain::advance_to_next_device_event: committed VI schedule {following_vi} did not advance beyond catch-up cycle {through_cycle}"
                );
                self.begin_field();
                DeviceAdvance::ViFields {
                    retrace_ticks: std::num::NonZeroU32::new(ticks)
                        .expect("positive VI retrace branch"),
                }
            }
        }
    }
}

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
    fn64_runtime::IplBootGlobals::cold(tv_type).install(&mut rdram);
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
mod tests;
