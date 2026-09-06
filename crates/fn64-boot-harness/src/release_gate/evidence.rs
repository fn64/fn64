#![allow(clippy::module_inception)]
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionDestinationSource {
    NoProgram,
    NativeArchive {
        artifact_sha256: String,
    },
    TypedObservedFunctionProgram {
        artifact_sha256: String,
    },
    TypedBlockProgram {
        program_sha256: String,
        dispatch_artifact_sha256: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "lane", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseExecutionDestination {
    Native {
        section_index: u32,
        function_offset: u32,
        link_vram: u32,
    },
    TypedFunction {
        vram: u32,
        symbol: String,
    },
    TypedBlock {
        bank: u64,
        pc: u32,
        runner_artifact_sha256: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionDestinationEventEvidence {
    /// Native function entries carry their exact guest cycle. Arbitrary-PC
    /// runners have instruction-order authority but no independent cycle
    /// stamp, so their value is `None` rather than a fabricated timestamp.
    pub guest_cycle: Option<u64>,
    pub destination: ReleaseExecutionDestination,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionDestinationCountEvidence {
    pub destination: ReleaseExecutionDestination,
    pub observations: u64,
}

/// Exact execution history plus a redundant canonical summary which retained
/// reports and representative matrices revalidate independently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionDestinationEvidence {
    pub source: ExecutionDestinationSource,
    pub total_observations: u64,
    pub unique_destinations: u64,
    pub ordered_sha256: String,
    pub unique_sha256: String,
    pub ordered: Vec<ExecutionDestinationEventEvidence>,
    pub unique: Vec<ExecutionDestinationCountEvidence>,
}

/// Exact public graphics-microcode identity returned by the registered
/// backend's digest catalog. This is recognition evidence only: release
/// reports still require the ROM's instructions to execute through LLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseMicrocodeFamily {
    Fast3d,
    F3dex,
    F3dlx,
    F3dlxRej,
    F3dex2,
    F3dex2NoN,
    F3dex2Rej,
    F3dlx2Rej,
    /// Named by the shared renderer seam but excluded from the public
    /// certification denominator because its complete wire is unpublished.
    F3dzex2,
    S2dex,
    S2dex2,
    L3dex,
    L3dex2,
    /// A backend-specific identity is retained but never satisfies a public
    /// family requirement.
    Other {
        id: u32,
    },
}

impl ReleaseMicrocodeFamily {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::Fast3d => 0,
            Self::F3dex => 1,
            Self::F3dlx => 2,
            Self::F3dlxRej => 3,
            Self::F3dex2 => 4,
            Self::F3dex2NoN => 5,
            Self::F3dex2Rej => 6,
            Self::F3dlx2Rej => 7,
            Self::F3dzex2 => 8,
            Self::S2dex => 9,
            Self::S2dex2 => 10,
            Self::L3dex => 11,
            Self::L3dex2 => 12,
            Self::Other { .. } => 13,
        }
    }

    pub(super) fn encode(self, out: &mut Vec<u8>) {
        out.push(self.tag());
        if let Self::Other { id } = self {
            push_u32(out, id);
        }
    }
}

/// One committed observation at the ABI-owned RSP/RDP execution boundary.
/// The vector order is fn64 commit order; it is not presented as a claim
/// about undocumented silicon interleaving.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RspRdpObservationKindEvidence {
    MicrocodeRecognition {
        task_address: u32,
        imem_generation: u64,
        text_sha256: String,
        /// Physical RDRAM address and exact logical byte identity of the
        /// original task microcode-data image. Yielded resumes retain the
        /// initial task identity rather than certifying rewritten yield state.
        data_address: u32,
        data_bytes: u32,
        data_sha256: String,
        family: Option<ReleaseMicrocodeFamily>,
    },
    DramDpcCommitted {
        start: u32,
        end: u32,
        command_sha256: String,
    },
    XbusDpcCommitted {
        start: u32,
        end: u32,
        command_sha256: String,
    },
    ImemReplacementCommitted {
        task_address: u32,
        imem_generation: u64,
        text_sha256: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RspRdpObservationEventEvidence {
    pub guest_cycle: u64,
    pub observation: RspRdpObservationKindEvidence,
}

/// Canonical ordered RSP/RDP history frozen at the committed VI boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RspRdpEvidence {
    pub total_observations: u64,
    pub ordered_sha256: String,
    pub ordered: Vec<RspRdpObservationEventEvidence>,
}

impl RspRdpEvidence {
    pub(crate) fn from_ordered(
        ordered: Vec<RspRdpObservationEventEvidence>,
    ) -> Result<Self, GateError> {
        let total_observations =
            u64::try_from(ordered.len()).map_err(|_| GateError::RspRdpObservationCountOverflow)?;
        let ordered_sha256 = sha256_hex(&encode_rsp_rdp_observations(&ordered)?);
        Ok(Self {
            total_observations,
            ordered_sha256,
            ordered,
        })
    }

    pub fn verify_integrity(&self, gate_cycle: u64) -> Result<(), GateError> {
        if self.total_observations != self.ordered.len() as u64 {
            return Err(GateError::RspRdpObservationIntegrityMismatch);
        }
        decode_sha256(&self.ordered_sha256)
            .ok_or(GateError::InvalidReportSha256("rsp_rdp.ordered_sha256"))?;
        validate_rsp_rdp_observations(gate_cycle, &self.ordered)?;
        let recomputed = sha256_hex(&encode_rsp_rdp_observations(&self.ordered)?);
        if self.ordered_sha256 != recomputed {
            return Err(GateError::RspRdpObservationIntegrityMismatch);
        }
        Ok(())
    }
}

/// One mandatory observable in the fixed-cycle digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Framebuffer,
    Audio,
    Memory,
    DeviceState,
    TimingTrace,
}

impl ArtifactKind {
    pub(super) const ALL: [Self; 5] = [
        Self::Framebuffer,
        Self::Audio,
        Self::Memory,
        Self::DeviceState,
        Self::TimingTrace,
    ];

    pub(super) const fn tag(self) -> &'static [u8] {
        match self {
            Self::Framebuffer => b"framebuffer",
            Self::Audio => b"audio",
            Self::Memory => b"memory",
            Self::DeviceState => b"device_state",
            Self::TimingTrace => b"timing_trace",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDigest {
    pub kind: ArtifactKind,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeterministicDigest {
    pub guest_cycle: u64,
    pub artifacts: Vec<ArtifactDigest>,
    pub root_sha256: String,
}

/// Operational-only component identities for localizing deterministic A/B
/// divergence. Device bytes preserve the historical DeviceState-v19 shape;
/// executor and ABI-host bytes likewise retain their v1-era shapes. These
/// digests carry no release-gate authority and omit program evidence
/// deliberately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationalStateComponentDigestsV1 {
    pub device_sha256: [u8; 32],
    pub executor_sha256: [u8; 32],
    pub abi_host_sha256: [u8; 32],
}

pub const OPERATIONAL_STATE_COMPONENT_DIGEST_SCHEMA_V1: &str =
    "fn64.operational-state-component-digests.v1";

/// Operational identities for the latest canonical guest-thread
/// publications. CPU and continuation bytes are domain-separated so a
/// divergence can be localized without granting either digest release-gate
/// authority.
#[cfg(feature = "recomp-rs")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationalThreadPublicationDigestsV1 {
    pub cpu_sha256: [u8; 32],
    pub continuation_sha256: [u8; 32],
    pub publication_count: u64,
    pub exact_count: u64,
    /// Total non-comparable publication count across every opaque variant.
    pub opaque_count: u64,
    pub opaque_host_count: u64,
    pub parked_fault_count: u64,
    pub returned_count: u64,
}

#[cfg(feature = "recomp-rs")]
pub const OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V1: &str =
    "fn64.operational-thread-publication-digests.v1";

/// Partition-invariant operational identities for canonical guest-thread
/// publications. Unlike v1, the continuation digest does not bind the most
/// recent dispatch slice's charge. It still binds cumulative canonical
/// charge and every resumable continuation field.
#[cfg(feature = "recomp-rs")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationalThreadPublicationDigestsV2 {
    pub cpu_sha256: [u8; 32],
    pub continuation_sha256: [u8; 32],
    pub publication_count: u64,
    pub exact_count: u64,
    /// Total non-comparable publication count across every opaque variant.
    pub opaque_count: u64,
    pub opaque_host_count: u64,
    pub parked_fault_count: u64,
    pub returned_count: u64,
}

#[cfg(feature = "recomp-rs")]
pub const OPERATIONAL_THREAD_PUBLICATION_DIGEST_SCHEMA_V2: &str =
    "fn64.operational-thread-publication-digests.v2";

#[cfg(feature = "recomp-rs")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationalThreadPublicationDigestErrorV1 {
    NonStrictThreadOrder {
        index: usize,
        previous: fn64_runtime::ThreadId,
        current: fn64_runtime::ThreadId,
    },
    IncoherentPreparedContinuation {
        thread: fn64_runtime::ThreadId,
    },
    InvalidExactCheckpointCharge {
        thread: fn64_runtime::ThreadId,
    },
    PendingCop0TimingWrite {
        thread: fn64_runtime::ThreadId,
    },
    ParkedFaultIsNotArchitecturalException {
        thread: fn64_runtime::ThreadId,
    },
}

#[cfg(feature = "recomp-rs")]
impl fmt::Display for OperationalThreadPublicationDigestErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonStrictThreadOrder {
                index,
                previous,
                current,
            } => write!(
                formatter,
                "canonical thread publications are not in strict ThreadId order at index {index}: {previous} then {current}"
            ),
            Self::IncoherentPreparedContinuation { thread } => write!(
                formatter,
                "canonical thread {thread} prepared continuation does not match its pending exit"
            ),
            Self::InvalidExactCheckpointCharge { thread } => write!(
                formatter,
                "canonical thread {thread} exact checkpoint has an impossible instruction charge"
            ),
            Self::PendingCop0TimingWrite { thread } => write!(
                formatter,
                "canonical thread {thread} publication retains a pending COP0 Count/Compare write"
            ),
            Self::ParkedFaultIsNotArchitecturalException { thread } => write!(
                formatter,
                "canonical thread {thread} parked-fault publication is not an architectural exception"
            ),
        }
    }
}

#[cfg(feature = "recomp-rs")]
impl std::error::Error for OperationalThreadPublicationDigestErrorV1 {}

/// Builder that rejects samples from any cycle other than its declared gate.
pub struct FixedCycleDigestGate {
    pub(super) guest_cycle: u64,
    pub(super) artifacts: BTreeMap<ArtifactKind, ArtifactDigest>,
}

/// Canonical paths a minimum live scenario report must actually exercise.
///
/// These are derived from captured artifacts and typed trace events by
/// [`LiveReleaseGate`], never asserted unconditionally by a boot host.
/// They are intentionally not a claim that every device or runtime behavior
/// was covered. Device DMA paths require the corresponding fabric-owned byte
/// commit/completion event; a generic executor event or shim call is not
/// evidence for any of them.
pub const LIVE_MINIMUM_CLOSURE_PATHS: [&str; 12] = [
    "cpu.thread-switch",
    "os.message-queue",
    "device.pi-dma-commit",
    "device.si-dma-commit",
    "device.ai-dma-complete",
    "device.sp-task-load-commit",
    "rsp.graphics-task",
    "rsp.audio-task",
    "vi.framebuffer",
    "ai.pcm",
    "memory.rdram",
    "execution.unsupported-event-source",
];

/// Optional live save-operation paths. A path is present only after at least
/// one successful authoritative operation for that device reaches the gate.
pub const LIVE_SAVE_OPERATION_CLOSURE_PATHS: [(SaveType, &str); 5] = [
    (SaveType::Eeprom4k, "save.eeprom-4k-operation"),
    (SaveType::Eeprom16k, "save.eeprom-16k-operation"),
    (SaveType::SramBanked, "save.sram-operation"),
    (SaveType::FlashRam, "save.flashram-operation"),
    (SaveType::ControllerPak, "save.pfs-operation"),
];

/// Optional controller/accessory paths. Merely configuring a port or probing
/// an accessory never creates one: a successful guest-visible operation must
/// reach the authoritative ABI or raw Joybus boundary.
pub const LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS: [(ControllerOperationDevice, &str); 4] = [
    (
        ControllerOperationDevice::StandardController,
        "controller.standard-input-read",
    ),
    (
        ControllerOperationDevice::RumblePak,
        "controller.rumble-operation",
    ),
    (
        ControllerOperationDevice::TransferPak,
        "controller.transfer-pak-operation",
    ),
    (
        ControllerOperationDevice::VoiceRecognitionUnit,
        "controller.voice-operation",
    ),
];


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedEvent {
    pub subsystem: String,
    pub operation: String,
    pub context: String,
    pub guest_cycle: Option<u64>,
    pub disposition: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosurePathStatus {
    Unexercised,
    ExercisedZeroUnsupported,
    ExercisedUnsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosurePath {
    pub name: String,
    pub observations: u64,
    pub status: ClosurePathStatus,
    pub unsupported: Vec<UnsupportedEvent>,
}
