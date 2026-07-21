//! Deterministic end-to-end evidence for a boot host.
//!
//! This module does not decide which game paths matter. A host declares those
//! paths, records whether each ran, and captures all output channels at one
//! exact guest cycle. That keeps a missing observation distinct from a proved
//! zero and prevents a shorter boot from masquerading as release closure.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use fn64_runtime::{
    ControllerOperationDevice, ControllerOperationEvent, DeviceEvidenceSnapshot, DeviceSnapshot,
    DeviceTraceEvent, DeviceTraceKind, DmaDirection, GameBoyMapperEvidenceSnapshot, PendingViFade,
    PortState, QueueOpKind, RdramAddr, SaveOperationEvent, SaveType, ScheduledDeviceEventKind,
    SiDmaKind, SpDmaDirection, SwitchReason, TaskKind, TraceEvent, TraceKind,
    UnsupportedEvent as RuntimeUnsupportedEvent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    FramebufferObservationSource, ObservationEvidenceError, ReleaseCartridgeSave,
    ReleaseControllerPort, ReleaseEnvironmentEvidence, ReleaseGraphicsApi,
    ReleaseGraphicsExecutionPolicy, ReleaseHostPlatform, ReleaseObservationGeometry,
    ReleaseRendererEvidence, ReleaseWindowsFamily, ReleaseWindowsProductType,
    ReleaseWindowsVersionEvidence,
};

pub(crate) const REPORT_SCHEMA: &str = "fn64.release-gate.v20";

/// Provenance class declared for a ROM input. The N64 header does not encode
/// whether otherwise-valid bytes came from a retail cartridge or a public
/// homebrew release, so this value is never inferred from ROM contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRomClass {
    Unclassified,
    RetailCartridge,
    PublicHomebrew,
}

/// One class declaration paired inseparably with the exact ROM bytes it
/// describes. Production callers obtain the class from verified admission;
/// the report builder derives every byte-level identity and header fact.
#[derive(Clone, Copy, Debug)]
pub struct ReleaseRomInput<'a> {
    class: ReleaseRomClass,
    bytes: &'a [u8],
}

impl<'a> ReleaseRomInput<'a> {
    pub const fn new(class: ReleaseRomClass, bytes: &'a [u8]) -> Self {
        Self { class, bytes }
    }

    pub const fn class(self) -> ReleaseRomClass {
        self.class
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}

impl ReleaseRomClass {
    const fn tag(self) -> u8 {
        match self {
            Self::Unclassified => 0,
            Self::RetailCartridge => 1,
            Self::PublicHomebrew => 2,
        }
    }

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified",
            Self::RetailCartridge => "retail_cartridge",
            Self::PublicHomebrew => "public_homebrew",
        }
    }

    pub fn from_wire_name(value: &str) -> Option<Self> {
        match value {
            "unclassified" => Some(Self::Unclassified),
            "retail_cartridge" => Some(Self::RetailCartridge),
            "public_homebrew" => Some(Self::PublicHomebrew),
            _ => None,
        }
    }
}

/// Source byte order normalized before hashing and decoding the N64 header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRomByteOrder {
    Z64,
    N64,
    V64,
}

impl ReleaseRomByteOrder {
    const fn tag(self) -> u8 {
        match self {
            Self::Z64 => 0,
            Self::N64 => 1,
            Self::V64 => 2,
        }
    }
}

/// TV compatibility decoded from the normalized ROM destination code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTvRegion {
    Ntsc,
    Pal,
    Mpal,
    RegionFree,
}

/// Concrete TV standard configured in the device fabric and renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseTvStandard {
    Ntsc,
    Pal,
    Mpal,
}

impl From<fn64_runtime::TvType> for ReleaseTvStandard {
    fn from(value: fn64_runtime::TvType) -> Self {
        match value {
            fn64_runtime::TvType::Ntsc => Self::Ntsc,
            fn64_runtime::TvType::Pal => Self::Pal,
            fn64_runtime::TvType::Mpal => Self::Mpal,
        }
    }
}

impl ReleaseTvStandard {
    pub const fn tv_type(self) -> fn64_runtime::TvType {
        match self {
            Self::Ntsc => fn64_runtime::TvType::Ntsc,
            Self::Pal => fn64_runtime::TvType::Pal,
            Self::Mpal => fn64_runtime::TvType::Mpal,
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::Ntsc => 0,
            Self::Pal => 1,
            Self::Mpal => 2,
        }
    }
}

impl ReleaseTvRegion {
    pub const fn tv_type(self) -> Option<fn64_runtime::TvType> {
        match self {
            Self::Ntsc => Some(fn64_runtime::TvType::Ntsc),
            Self::Pal => Some(fn64_runtime::TvType::Pal),
            Self::Mpal => Some(fn64_runtime::TvType::Mpal),
            Self::RegionFree => None,
        }
    }

    const fn fixed_tv_type(self) -> Option<ReleaseTvStandard> {
        match self {
            Self::Ntsc => Some(ReleaseTvStandard::Ntsc),
            Self::Pal => Some(ReleaseTvStandard::Pal),
            Self::Mpal => Some(ReleaseTvStandard::Mpal),
            Self::RegionFree => None,
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::Ntsc => 0,
            Self::Pal => 1,
            Self::Mpal => 2,
            Self::RegionFree => 3,
        }
    }
}

/// Canonical installed-ROM identity and header-derived TV evidence.
///
/// Header offsets and the z64/n64/v64 normalization follow the public
/// N64brew ROM Header specification. The raw installed identity remains the
/// report's `input_sha256`; this additional digest makes byte-order-equivalent
/// inputs share one canonical big-endian identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRomEvidence {
    pub class: ReleaseRomClass,
    pub source_byte_order: ReleaseRomByteOrder,
    pub byte_len: u64,
    pub canonical_sha256: String,
    pub destination_code: u8,
    pub decoded_tv_region: ReleaseTvRegion,
    pub configured_tv_type: ReleaseTvStandard,
}

impl ReleaseRomEvidence {
    /// Decode the normalized header's fixed TV authority before boot. A
    /// region-free header returns `None`; callers must choose an explicit host
    /// standard and the retained report will record it without crediting a
    /// fixed TV-region requirement.
    pub fn decode_tv_type(rom_bytes: &[u8]) -> Result<Option<fn64_runtime::TvType>, GateError> {
        let (_, canonical) = normalize_rom_bytes(rom_bytes)?;
        Ok(decode_rom_tv_region(canonical[0x3e])?.tv_type())
    }

    pub fn from_bytes(
        rom_bytes: &[u8],
        class: ReleaseRomClass,
        configured_tv_type: fn64_runtime::TvType,
    ) -> Result<Self, GateError> {
        let (source_byte_order, canonical) = normalize_rom_bytes(rom_bytes)?;
        let destination_code = canonical[0x3e];
        let decoded_tv_region = decode_rom_tv_region(destination_code)?;
        let configured_tv_type = ReleaseTvStandard::from(configured_tv_type);
        if let Some(expected) = decoded_tv_region.fixed_tv_type() {
            if configured_tv_type != expected {
                return Err(GateError::RomTvTypeMismatch {
                    authority: "normalized ROM destination code",
                    expected,
                    observed: configured_tv_type,
                });
            }
        }
        Ok(Self {
            class,
            source_byte_order,
            byte_len: u64::try_from(rom_bytes.len())
                .map_err(|_| GateError::RomByteLengthOverflow)?,
            canonical_sha256: sha256_hex(&canonical),
            destination_code,
            decoded_tv_region,
            configured_tv_type,
        })
    }

    fn verify_integrity(&self) -> Result<(), GateError> {
        if self.byte_len < ROM_HEADER_BYTES {
            return Err(GateError::RomTooSmall {
                bytes: self.byte_len,
            });
        }
        if !self.byte_len.is_multiple_of(4) {
            return Err(GateError::RomNotWordAligned {
                bytes: self.byte_len,
            });
        }
        decode_sha256(&self.canonical_sha256)
            .ok_or(GateError::InvalidReportSha256("rom.canonical_sha256"))?;
        let decoded = decode_rom_tv_region(self.destination_code)?;
        if decoded != self.decoded_tv_region {
            return Err(GateError::RomRegionDecodeMismatch {
                destination_code: self.destination_code,
                stored: self.decoded_tv_region,
                decoded,
            });
        }
        if let Some(expected) = decoded.fixed_tv_type() {
            if self.configured_tv_type != expected {
                return Err(GateError::RomTvTypeMismatch {
                    authority: "retained ROM destination code",
                    expected,
                    observed: self.configured_tv_type,
                });
            }
        }
        Ok(())
    }
}

const ROM_HEADER_BYTES: u64 = 0x40;
const MAGIC_Z64: u32 = 0x8037_1240;
const MAGIC_N64: u32 = 0x4012_3780;
const MAGIC_V64: u32 = 0x3780_4012;

fn normalize_rom_bytes(input: &[u8]) -> Result<(ReleaseRomByteOrder, Vec<u8>), GateError> {
    if input.len() < ROM_HEADER_BYTES as usize {
        return Err(GateError::RomTooSmall {
            bytes: input.len() as u64,
        });
    }
    if !input.len().is_multiple_of(4) {
        return Err(GateError::RomNotWordAligned {
            bytes: input.len() as u64,
        });
    }
    let first_word = u32::from_be_bytes(input[..4].try_into().expect("four-byte ROM magic"));
    let source = match first_word {
        MAGIC_Z64 => ReleaseRomByteOrder::Z64,
        MAGIC_N64 => ReleaseRomByteOrder::N64,
        MAGIC_V64 => ReleaseRomByteOrder::V64,
        _ => return Err(GateError::UnknownRomByteOrder { first_word }),
    };
    let canonical = match source {
        ReleaseRomByteOrder::Z64 => input.to_vec(),
        ReleaseRomByteOrder::N64 => input
            .chunks_exact(4)
            .flat_map(|word| [word[3], word[2], word[1], word[0]])
            .collect(),
        ReleaseRomByteOrder::V64 => input
            .chunks_exact(2)
            .flat_map(|pair| [pair[1], pair[0]])
            .collect(),
    };
    Ok((source, canonical))
}

fn has_recognized_rom_magic(input: &[u8]) -> bool {
    input.get(..4).is_some_and(|bytes| {
        matches!(
            u32::from_be_bytes(bytes.try_into().expect("four-byte ROM magic")),
            MAGIC_Z64 | MAGIC_N64 | MAGIC_V64
        )
    })
}

fn validate_installed_rom_identity(
    host: &fn64_abi::AbiHostEvidenceSnapshot,
    input_bytes: &[u8],
) -> Result<(), GateError> {
    let installed = host
        .installed_rom
        .ok_or(GateError::MissingInstalledRomIdentity)?;
    let supplied_bytes =
        u64::try_from(input_bytes.len()).map_err(|_| GateError::RomByteLengthOverflow)?;
    let supplied_sha256: [u8; 32] = Sha256::digest(input_bytes).into();
    if !host.rom_installed
        || installed.byte_len != supplied_bytes
        || installed.sha256 != supplied_sha256
    {
        return Err(GateError::InstalledRomIdentityMismatch {
            installed_bytes: installed.byte_len,
            supplied_bytes,
            installed_sha256: hex(&installed.sha256),
            supplied_sha256: hex(&supplied_sha256),
        });
    }
    Ok(())
}

fn decode_rom_tv_region(destination_code: u8) -> Result<ReleaseTvRegion, GateError> {
    // Public N64brew "ROM Header" destination table. Zero is the common
    // homebrew region-free value; `A` means all destinations.
    match destination_code {
        0 | b'A' => Ok(ReleaseTvRegion::RegionFree),
        b'B' => Ok(ReleaseTvRegion::Mpal),
        b'C' | b'E' | b'G' | b'J' | b'K' | b'N' => Ok(ReleaseTvRegion::Ntsc),
        b'D' | b'F' | b'H' | b'I' | b'L' | b'P' | b'S' | b'U' | b'W' | b'X' | b'Y' | b'Z' => {
            Ok(ReleaseTvRegion::Pal)
        }
        _ => Err(GateError::UnknownRomDestinationCode(destination_code)),
    }
}

fn validate_rom_environment(
    rom: &Option<ReleaseRomEvidence>,
    environment: &ReleaseEnvironmentEvidence,
) -> Result<(), GateError> {
    let Some(rom) = rom else {
        return Ok(());
    };
    rom.verify_integrity()?;
    let renderer_tv_type = environment.renderer.tv_type();
    if renderer_tv_type != rom.configured_tv_type {
        return Err(GateError::RomTvTypeMismatch {
            authority: "retained renderer create-time configuration",
            expected: rom.configured_tv_type,
            observed: renderer_tv_type,
        });
    }
    Ok(())
}

fn validate_rom_input(
    rom: &Option<ReleaseRomEvidence>,
    input_bytes: &[u8],
) -> Result<(), GateError> {
    let Some(rom) = rom else {
        return Ok(());
    };
    let decoded =
        ReleaseRomEvidence::from_bytes(input_bytes, rom.class, rom.configured_tv_type.tv_type())?;
    if &decoded != rom {
        return Err(GateError::RomInputEvidenceMismatch);
    }
    Ok(())
}

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
    const fn tag(self) -> u8 {
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

    fn encode(self, out: &mut Vec<u8>) {
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
    const ALL: [Self; 5] = [
        Self::Framebuffer,
        Self::Audio,
        Self::Memory,
        Self::DeviceState,
        Self::TimingTrace,
    ];

    const fn tag(self) -> &'static [u8] {
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

/// Builder that rejects samples from any cycle other than its declared gate.
pub struct FixedCycleDigestGate {
    guest_cycle: u64,
    artifacts: BTreeMap<ArtifactKind, ArtifactDigest>,
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

/// Opt-in production seam around [`FixedCycleDigestGate`].
///
/// Arming is allowed only before guest time or trace events exist. Finishing
/// samples the ABI's typed device/trace/audio sources itself; the boot host
/// must supply its framebuffer and memory bytes from live backend state. The
/// gate derives the closure ledger from those observations, preventing a host
/// from turning an unconditional declaration into fabricated coverage.
pub struct LiveReleaseGate {
    guest_cycle: u64,
    armed: bool,
}

pub(crate) struct LiveObservedArtifacts<'a> {
    pub(crate) framebuffer_artifact_bytes: &'a [u8],
    pub(crate) framebuffer_payload_bytes: usize,
    pub(crate) memory_bytes: &'a [u8],
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
        observed
            .observations
            .validate_payload_lengths(
                observed.framebuffer_payload_bytes,
                observed.memory_bytes.len(),
            )
            .map_err(GateError::InvalidObservationGeometry)?;
        let environment = environment_from_frozen(platform, windows_version, &host, render)?;
        validate_environment_observation(&environment, &observed.observations)?;
        let audio_bytes =
            fn64_abi::copy_audio_digest_bytes().ok_or(GateError::AudioDigestCaptureNotArmed)?;
        let trace = fn64_abi::copy_trace();
        let device_trace = fn64_abi::copy_device_trace();
        let save_operations = fn64_abi::copy_save_operations();
        let controller_operations = fn64_abi::copy_controller_operations();
        let unsupported_events = fn64_runtime::copy_unsupported_events();
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
        digest.capture(observed_cycle, ArtifactKind::Memory, observed.memory_bytes)?;
        digest.capture_device_snapshot(snapshot, executor, host, program)?;
        digest.capture_live_timing_trace(observed_cycle, &trace, &device_trace)?;

        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: observed.framebuffer_artifact_bytes,
            audio_bytes: &audio_bytes,
            memory_bytes: observed.memory_bytes,
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

fn validate_controller_operation_cycles(
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

fn capture_rsp_rdp_evidence(
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

const fn release_microcode_family(family: fn64_abi::UcodeId) -> ReleaseMicrocodeFamily {
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

fn capture_execution_destinations(
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

fn validate_execution_destination_cycles(
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

fn validate_execution_destination_source(
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
        let bytes = encode_device_snapshot(snapshot, executor, host, program);
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
        push_bytes(&mut bytes, b"fn64.live-timing.v1");
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

struct LiveClosureInputs<'a> {
    framebuffer_bytes: &'a [u8],
    audio_bytes: &'a [u8],
    memory_bytes: &'a [u8],
    trace: &'a [TraceEvent],
    device_trace: &'a [DeviceTraceEvent],
    save_operations: &'a [SaveOperationEvent],
    controller_operations: &'a [ControllerOperationEvent],
    unsupported_events: &'a [RuntimeUnsupportedEvent],
}

fn derive_live_closure(inputs: LiveClosureInputs<'_>) -> Result<Vec<ClosurePath>, GateError> {
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
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Graphics,
                ..
            } => Some("rsp.graphics-task"),
            TraceKind::TaskSubmit {
                task_kind: TaskKind::Audio,
                ..
            } => Some("rsp.audio-task"),
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

fn environment_from_frozen(
    platform: ReleaseHostPlatform,
    windows_version: Option<ReleaseWindowsVersionEvidence>,
    host: &fn64_abi::AbiHostEvidenceSnapshot,
    render: fn64_abi::RenderEnvironmentEvidenceSnapshot,
) -> Result<ReleaseEnvironmentEvidence, GateError> {
    let controller_ports = host
        .runtime_peripherals
        .peripherals
        .pif
        .ports
        .map(|port| match port {
            PortState::StandardControllerNoPak => ReleaseControllerPort::StandardControllerNoPak,
            PortState::StandardControllerControllerPak => {
                ReleaseControllerPort::StandardControllerControllerPak
            }
            PortState::StandardControllerRumblePak => {
                ReleaseControllerPort::StandardControllerRumblePak
            }
            PortState::StandardControllerTransferPak => {
                ReleaseControllerPort::StandardControllerTransferPak
            }
            PortState::VoiceRecognitionUnit => ReleaseControllerPort::VoiceRecognitionUnit,
            PortState::Absent => ReleaseControllerPort::Absent,
        });
    let cartridge_save = match host.cartridge_save {
        fn64_abi::CartridgeSaveEvidenceSnapshot::Unidentified => {
            return Err(GateError::UnidentifiedCartridgeSave);
        }
        fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave => {
            ReleaseCartridgeSave::NoCartridgeSave
        }
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(save_type) => match save_type {
            fn64_abi::CartridgeSaveType::Eeprom4k => ReleaseCartridgeSave::Eeprom4k,
            fn64_abi::CartridgeSaveType::Eeprom16k => ReleaseCartridgeSave::Eeprom16k,
            fn64_abi::CartridgeSaveType::SramBanked => ReleaseCartridgeSave::Sram32Kib,
            fn64_abi::CartridgeSaveType::FlashRam => ReleaseCartridgeSave::FlashRam128Kib,
        },
    };
    let execution_policy = match render.execution_policy {
        fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized => {
            return Err(GateError::NonAccuracyRenderPolicy);
        }
        fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy => {
            ReleaseGraphicsExecutionPolicy::LleAccuracy
        }
    };
    let renderer = match render.backend {
        fn64_abi::RenderBackendEvidence::Unidentified => {
            return Err(GateError::UnidentifiedRenderBackend);
        }
        fn64_abi::RenderBackendEvidence::Reference { tv_type } => {
            ReleaseRendererEvidence::Reference {
                execution_policy,
                tv_type: tv_type.into(),
            }
        }
        fn64_abi::RenderBackendEvidence::Rt64 {
            tv_type,
            backend_identity,
            source_authoritative,
            graphics_api,
            settings_sha256,
            replacement_packs_active,
        } => ReleaseRendererEvidence::Rt64 {
            execution_policy,
            tv_type: tv_type.into(),
            graphics_api: match graphics_api {
                fn64_abi::ActiveRenderGraphicsApi::D3d12 => ReleaseGraphicsApi::D3d12,
                fn64_abi::ActiveRenderGraphicsApi::Vulkan => ReleaseGraphicsApi::Vulkan,
                fn64_abi::ActiveRenderGraphicsApi::Metal => ReleaseGraphicsApi::Metal,
            },
            backend_identity,
            source_authoritative,
            settings_sha256: hex(&settings_sha256),
            replacement_packs_active,
        },
    };
    Ok(ReleaseEnvironmentEvidence {
        platform,
        windows_version,
        controller_ports,
        cartridge_save,
        renderer,
    })
}

fn validate_environment_observation(
    environment: &ReleaseEnvironmentEvidence,
    observations: &ReleaseObservationGeometry,
) -> Result<(), GateError> {
    match (&environment.renderer, &observations.framebuffer.source) {
        (
            ReleaseRendererEvidence::Reference { .. },
            FramebufferObservationSource::PhysicalRdram { .. },
        ) => Ok(()),
        (
            ReleaseRendererEvidence::Rt64 {
                backend_identity,
                source_authoritative,
                settings_sha256,
                ..
            },
            FramebufferObservationSource::PostViSwapchain {
                backend_identity: observed_identity,
                settings_sha256: observed_settings,
                ..
            },
        ) if *source_authoritative
            && backend_identity == observed_identity
            && settings_sha256 == observed_settings =>
        {
            Ok(())
        }
        (ReleaseRendererEvidence::Reference { .. }, _) => {
            Err(GateError::RendererObservationMismatch(
                "Reference backend requires a physical-RDRAM framebuffer",
            ))
        }
        (ReleaseRendererEvidence::Rt64 { .. }, _) => Err(GateError::RendererObservationMismatch(
            "RT64 requires authoritative matching post-VI identity and settings",
        )),
    }
}

fn validate_environment_evidence(
    environment: &ReleaseEnvironmentEvidence,
) -> Result<(), GateError> {
    match (environment.platform, environment.windows_version) {
        (ReleaseHostPlatform::WindowsX86_64, Some(version)) => version
            .verify()
            .map_err(GateError::InvalidWindowsVersionEvidence)?,
        (ReleaseHostPlatform::WindowsX86_64, None) => {
            return Err(GateError::InvalidWindowsVersionEvidence(
                "windows_x86_64 requires exact native build evidence",
            ));
        }
        (_, Some(_)) => {
            return Err(GateError::InvalidWindowsVersionEvidence(
                "non-Windows platform carries Windows version evidence",
            ));
        }
        (_, None) => {}
    }
    match &environment.renderer {
        ReleaseRendererEvidence::Reference {
            execution_policy, ..
        }
        | ReleaseRendererEvidence::Rt64 {
            execution_policy, ..
        } => {
            if *execution_policy != ReleaseGraphicsExecutionPolicy::LleAccuracy {
                return Err(GateError::NonAccuracyRenderPolicy);
            }
        }
    }
    if let ReleaseRendererEvidence::Rt64 {
        graphics_api,
        backend_identity,
        source_authoritative,
        settings_sha256,
        ..
    } = &environment.renderer
    {
        if backend_identity.is_empty() || !*source_authoritative {
            return Err(GateError::RendererObservationMismatch(
                "RT64 backend identity is empty or non-authoritative",
            ));
        }
        decode_sha256(settings_sha256).ok_or(GateError::InvalidReportSha256(
            "environment.renderer.settings_sha256",
        ))?;
        crate::render_evidence::validate_authoritative_rt64_backend_identity(
            backend_identity,
            environment.platform,
            *graphics_api,
        )
        .map_err(|_| {
            GateError::RendererObservationMismatch(
                "RT64 backend identity lacks canonical adapter/source/platform provenance",
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
fn test_release_environment(
    observations: &ReleaseObservationGeometry,
) -> ReleaseEnvironmentEvidence {
    let renderer = match &observations.framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { .. } => ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
        },
        FramebufferObservationSource::PostViSwapchain {
            backend_identity,
            settings_sha256,
            ..
        } => ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: match super::release_host_platform()
                .expect("test platform is release-supported")
            {
                ReleaseHostPlatform::MacosArm64 => ReleaseGraphicsApi::Metal,
                ReleaseHostPlatform::LinuxX86_64 => ReleaseGraphicsApi::Vulkan,
                ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::D3d12,
            },
            backend_identity: backend_identity.clone(),
            source_authoritative: true,
            settings_sha256: settings_sha256.clone(),
            replacement_packs_active: false,
        },
    };
    ReleaseEnvironmentEvidence {
        platform: super::release_host_platform().expect("test platform is release-supported"),
        windows_version: super::test_release_windows_version(),
        controller_ports: [
            ReleaseControllerPort::StandardControllerNoPak,
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
            ReleaseControllerPort::Absent,
        ],
        cartridge_save: ReleaseCartridgeSave::NoCartridgeSave,
        renderer,
    }
}

#[cfg(test)]
fn test_rsp_rdp_evidence(
    guest_cycle: u64,
    closure: &[ClosurePath],
) -> Result<RspRdpEvidence, GateError> {
    let graphics_exercised = closure.iter().any(|path| {
        path.name == "rsp.graphics-task"
            && matches!(
                path.status,
                ClosurePathStatus::ExercisedZeroUnsupported
                    | ClosurePathStatus::ExercisedUnsupported
            )
    });
    let ordered = if graphics_exercised {
        vec![RspRdpObservationEventEvidence {
            guest_cycle,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0,
                imem_generation: 0,
                text_sha256: sha256_hex(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]),
                data_address: 0,
                data_bytes: 1,
                data_sha256: sha256_hex(&[0]),
                family: None,
            },
        }]
    } else {
        Vec::new()
    };
    RspRdpEvidence::from_ordered(ordered)
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
    pub closure: Vec<ClosurePath>,
    /// SHA-256 over every other semantic report field in an explicit wire
    /// order. Cite this value, rather than the artifact-only digest root, when
    /// comparing ROM/lane/backend/policy scenarios.
    pub report_sha256: String,
}

struct ReleaseBoundaryReportEvidence {
    rom: Option<ReleaseRomEvidence>,
    observations: ReleaseObservationGeometry,
    environment: ReleaseEnvironmentEvidence,
    execution_destinations: ExecutionDestinationEvidence,
    rsp_rdp: RspRdpEvidence,
}

impl ReleaseGateReport {
    fn new_with_environment(
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

    /// Recompute the schema-v20 evidence digest after loading a retained JSON
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
    InvalidWindowsVersionEvidence(&'static str),
    RendererObservationMismatch(&'static str),
    InvalidViBoundary(crate::ViBoundaryError),
    WrongLiveCycle {
        expected: u64,
        observed: u64,
    },
    AudioDigestCaptureNotArmed,
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
            Self::InvalidWindowsVersionEvidence(detail) => {
                write!(f, "invalid Windows release identity: {detail}")
            }
            Self::RendererObservationMismatch(detail) => {
                write!(f, "frozen renderer evidence disagrees with framebuffer observation: {detail}")
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

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

pub(crate) fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut out = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        out[index] = (high << 4) | low;
    }
    Some(out)
}

pub(crate) fn recompute_digest_root(
    guest_cycle: u64,
    artifacts: &[ArtifactDigest],
) -> Result<String, GateError> {
    let mut root = Sha256::new();
    root.update(REPORT_SCHEMA.as_bytes());
    root.update(guest_cycle.to_be_bytes());
    for artifact in artifacts {
        root.update(artifact.kind.tag());
        root.update(artifact.bytes.to_be_bytes());
        root.update(
            decode_sha256(&artifact.sha256)
                .ok_or(GateError::InvalidReportSha256("digest.artifacts[].sha256"))?,
        );
    }
    Ok(hex(&root.finalize()))
}

pub(crate) fn validate_artifact_observation_bytes(
    digest: &DeterministicDigest,
    observations: &ReleaseObservationGeometry,
) -> Result<(), GateError> {
    let framebuffer = &digest.artifacts[0];
    let expected_framebuffer = observations
        .expected_framebuffer_artifact_bytes()
        .map_err(GateError::InvalidObservationGeometry)?;
    if framebuffer.bytes != expected_framebuffer {
        return Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Framebuffer,
            expected: expected_framebuffer,
            observed: framebuffer.bytes,
        });
    }
    let memory = &digest.artifacts[2];
    if memory.bytes != observations.memory.payload_bytes {
        return Err(GateError::ArtifactObservationByteMismatch {
            kind: ArtifactKind::Memory,
            expected: observations.memory.payload_bytes,
            observed: memory.bytes,
        });
    }
    Ok(())
}

pub(crate) fn validate_closure_paths(paths: &[ClosurePath]) -> Result<(), GateError> {
    let mut names = std::collections::BTreeSet::new();
    for path in paths {
        if path.name.is_empty() {
            return Err(GateError::EmptyPathName);
        }
        if !names.insert(path.name.as_str()) {
            return Err(GateError::DuplicateClosurePath(path.name.clone()));
        }
        let valid = match path.status {
            ClosurePathStatus::Unexercised => path.observations == 0 && path.unsupported.is_empty(),
            ClosurePathStatus::ExercisedZeroUnsupported => {
                path.observations > 0 && path.unsupported.is_empty()
            }
            ClosurePathStatus::ExercisedUnsupported => {
                path.observations > 0
                    && !path.unsupported.is_empty()
                    && path.observations >= path.unsupported.len() as u64
            }
        };
        if !valid {
            let detail = match path.status {
                ClosurePathStatus::Unexercised => {
                    "unexercised requires zero observations and no unsupported events"
                }
                ClosurePathStatus::ExercisedZeroUnsupported => {
                    "zero-unsupported requires a positive observation count and no unsupported events"
                }
                ClosurePathStatus::ExercisedUnsupported => {
                    "unsupported requires a positive count, at least one event, and count >= event count"
                }
            };
            return Err(GateError::InvalidClosurePath {
                name: path.name.clone(),
                detail,
            });
        }
    }
    Ok(())
}

pub(crate) fn validate_canonical_closure_order(paths: &[ClosurePath]) -> Result<(), GateError> {
    if let Some(pair) = paths.windows(2).find(|pair| pair[0].name >= pair[1].name) {
        return Err(GateError::NonCanonicalClosureOrder {
            previous: pair[0].name.clone(),
            next: pair[1].name.clone(),
        });
    }
    Ok(())
}

fn validate_rsp_rdp_closure(
    paths: &[ClosurePath],
    evidence: &RspRdpEvidence,
) -> Result<(), GateError> {
    let graphics_exercised = paths.iter().any(|path| {
        path.name == "rsp.graphics-task"
            && matches!(
                path.status,
                ClosurePathStatus::ExercisedZeroUnsupported
                    | ClosurePathStatus::ExercisedUnsupported
            )
    });
    let recognition_observed = evidence.ordered.iter().any(|event| {
        matches!(
            event.observation,
            RspRdpObservationKindEvidence::MicrocodeRecognition { .. }
        )
    });
    if graphics_exercised && !recognition_observed {
        return Err(GateError::MissingGraphicsMicrocodeRecognition);
    }
    Ok(())
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn encode_guest_device_snapshot(out: &mut Vec<u8>, snapshot: DeviceSnapshot) {
    push_u64(out, snapshot.now.get());
    for value in [
        snapshot.pi_dram_addr.offset(),
        snapshot.pi_cart_addr,
        snapshot.pi_status,
        snapshot.ai_status,
        snapshot.ai_length,
        snapshot.si_dram_addr.offset(),
        snapshot.si_status,
        snapshot.vi_current,
        snapshot.vi_intr,
        snapshot.vi_v_sync,
    ] {
        push_u32(out, value);
    }
    push_u32(out, snapshot.tv_type.map_or(u32::MAX, |tv| tv as u32));
    push_u64(
        out,
        snapshot
            .vi_field_interval
            .map_or(u64::MAX, |cycles| cycles.get()),
    );
    out.push(snapshot.sp_busy as u8);
    push_u32(out, snapshot.sp_status);
    push_u32(
        out,
        u32::try_from(snapshot.sp_mem_addr.offset()).expect("RSP offset fits u32"),
    );
    push_u32(out, snapshot.sp_dram_addr.offset());
    push_u64(out, snapshot.sp_imem_generation);
    out.push(snapshot.dp_busy as u8);
    for value in [snapshot.mi_pending, snapshot.mi_mask] {
        push_u32(out, value);
    }
    for timing in [snapshot.pi_domain1, snapshot.pi_domain2] {
        out.extend_from_slice(&[
            timing.latency,
            timing.pulse_width,
            timing.page_size,
            timing.release,
        ]);
    }
}

fn encode_pi_request(out: &mut Vec<u8>, request: fn64_runtime::PiDmaRequest) {
    out.push(match request.direction {
        DmaDirection::ToRdram => 0,
        DmaDirection::FromRdram => 1,
    });
    push_u32(out, request.dram_addr.offset());
    push_u32(out, request.cart_addr);
    push_u32(out, request.len);
}

fn encode_ai_request(out: &mut Vec<u8>, request: fn64_runtime::AiDmaRequest) {
    push_u32(out, request.dram_addr.offset());
    push_u32(out, request.len);
    push_u32(out, request.sample_rate_hz);
}

fn encode_si_request(out: &mut Vec<u8>, request: fn64_runtime::SiDmaRequest) {
    out.push(match request.kind {
        SiDmaKind::DramToPif => 0,
        SiDmaKind::PifToDram => 1,
        SiDmaKind::ControllerQuery => 2,
        SiDmaKind::ControllerRead => 3,
    });
    push_u32(out, request.dram_addr.offset());
}

fn encode_sp_dma_request(out: &mut Vec<u8>, request: fn64_runtime::SpDmaRequest) {
    out.push(match request.direction {
        SpDmaDirection::RdramToRsp => 0,
        SpDmaDirection::RspToRdram => 1,
    });
    push_u32(
        out,
        u32::try_from(request.mem_addr.offset()).expect("RSP DMA offset fits u32"),
    );
    push_u32(out, request.dram_addr.offset());
    push_u32(out, request.encoded_len);
}

fn push_option_u32(out: &mut Vec<u8>, value: Option<u32>) {
    match value {
        Some(value) => {
            out.push(1);
            push_u32(out, value);
        }
        None => out.push(0),
    }
}

fn push_option_u16(out: &mut Vec<u8>, value: Option<u16>) {
    match value {
        Some(value) => {
            out.push(1);
            push_u16(out, value);
        }
        None => out.push(0),
    }
}

fn push_option_bool(out: &mut Vec<u8>, value: Option<bool>) {
    out.push(match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    });
}

fn encode_port_state(state: PortState) -> u8 {
    match state {
        PortState::StandardControllerNoPak => 0,
        PortState::StandardControllerControllerPak => 1,
        PortState::StandardControllerRumblePak => 2,
        PortState::StandardControllerTransferPak => 3,
        PortState::VoiceRecognitionUnit => 4,
        PortState::Absent => 5,
    }
}

fn encode_controller_pak(out: &mut Vec<u8>, snapshot: fn64_runtime::ControllerPakEvidenceSnapshot) {
    out.extend_from_slice(&[snapshot.bank_count, snapshot.active_bank]);
    for note in snapshot.notes {
        match note {
            Some(note) => {
                out.push(1);
                push_u16(out, note.key.company_code);
                push_u32(out, note.key.game_code);
                out.extend_from_slice(&note.key.game_name);
                out.extend_from_slice(&note.key.ext_name);
                push_u64(out, note.pages.len() as u64);
                for page in note.pages {
                    push_u16(out, page);
                }
            }
            None => out.push(0),
        }
    }
    push_bytes(out, &snapshot.raw);
}

fn encode_game_boy_mapper(out: &mut Vec<u8>, mapper: GameBoyMapperEvidenceSnapshot) {
    match mapper {
        GameBoyMapperEvidenceSnapshot::RomOnly => out.push(0),
        GameBoyMapperEvidenceSnapshot::Mbc1 {
            ram_enabled,
            rom_low5,
            upper2,
            ram_mode,
        } => out.extend_from_slice(&[1, ram_enabled as u8, rom_low5, upper2, ram_mode as u8]),
        GameBoyMapperEvidenceSnapshot::Mbc2 {
            ram_enabled,
            rom_bank,
        } => out.extend_from_slice(&[2, ram_enabled as u8, rom_bank]),
        GameBoyMapperEvidenceSnapshot::Mbc3 {
            timer_present,
            ram_enabled,
            rom_bank,
            select,
            latch_armed,
            rtc,
            latched_rtc,
            subsecond_cycles,
        } => {
            out.extend_from_slice(&[
                3,
                timer_present as u8,
                ram_enabled as u8,
                rom_bank,
                select,
                latch_armed as u8,
            ]);
            out.extend_from_slice(&rtc);
            out.extend_from_slice(&latched_rtc);
            push_u64(out, subsecond_cycles);
        }
        GameBoyMapperEvidenceSnapshot::Mbc5 {
            ram_enabled,
            rom_bank,
            ram_bank,
            rumble_variant,
        } => {
            out.extend_from_slice(&[4, ram_enabled as u8]);
            push_u16(out, rom_bank);
            out.extend_from_slice(&[ram_bank, rumble_variant as u8]);
        }
    }
}

fn encode_transfer_pak(out: &mut Vec<u8>, snapshot: fn64_runtime::TransferPakEvidenceSnapshot) {
    push_u64(out, snapshot.now.get());
    out.extend_from_slice(&[
        snapshot.enabled as u8,
        snapshot.transfer_bank,
        snapshot.access_mode,
        snapshot.cartridge_pulled as u8,
        snapshot.reset_detected as u8,
    ]);
    match snapshot.cartridge {
        Some(cartridge) => {
            out.push(1);
            push_bytes(out, &cartridge.rom);
            push_bytes(out, &cartridge.ram);
            encode_game_boy_mapper(out, cartridge.mapper);
        }
        None => out.push(0),
    }
}

fn encode_voice_data(out: &mut Vec<u8>, data: fn64_runtime::VoiceData) {
    for value in [
        data.warning,
        data.answer_num,
        data.voice_level,
        data.voice_sn,
        data.voice_time,
    ] {
        push_u16(out, value);
    }
    for value in data.answer.into_iter().chain(data.distance) {
        push_u16(out, value);
    }
}

fn encode_voice_unit(out: &mut Vec<u8>, snapshot: fn64_runtime::VoiceEvidenceSnapshot) {
    out.extend_from_slice(&[snapshot.initialized as u8, snapshot.raw_init_step]);
    match snapshot.expected_words {
        Some(words) => out.extend_from_slice(&[1, words]),
        None => out.push(0),
    }
    push_u64(out, snapshot.words.len() as u64);
    for word in snapshot.words {
        push_bytes(out, &word);
    }
    push_bytes(out, &snapshot.mask);
    out.extend_from_slice(&[snapshot.analog_gain, snapshot.digital_gain, snapshot.status]);
    match snapshot.pending_result {
        Some(data) => {
            out.push(1);
            encode_voice_data(out, data);
        }
        None => out.push(0),
    }
}

fn encode_vi_manager(out: &mut Vec<u8>, snapshot: fn64_runtime::ViEvidenceSnapshot) {
    push_option_u32(out, snapshot.mode_ptr);
    push_option_u32(out, snapshot.next_mode_ptr);
    out.push(snapshot.next_mode_resets_overrides as u8);
    for value in [
        snapshot.special_features,
        snapshot.next_special_features,
        snapshot.x_scale_bits,
        snapshot.y_scale_bits,
        snapshot.next_x_scale_bits,
        snapshot.next_y_scale_bits,
    ] {
        push_option_u32(out, value);
    }
    out.push(snapshot.blanked as u8);
    push_option_bool(out, snapshot.next_blanked);
    push_option_u16(out, snapshot.fade);
    match snapshot.next_fade {
        PendingViFade::Unchanged => out.push(0),
        PendingViFade::Disabled => out.push(1),
        PendingViFade::Factor(factor) => {
            out.push(2);
            push_u16(out, factor);
        }
    }
    out.push(snapshot.repeat_line as u8);
    push_option_bool(out, snapshot.next_repeat_line);
    push_option_u32(out, snapshot.current_framebuffer);
    push_option_u32(out, snapshot.next_framebuffer);
    push_u64(out, snapshot.swap_count);
    match snapshot.retrace_target {
        Some((queue, message)) => {
            out.push(1);
            push_u32(out, queue);
            push_u32(out, message);
        }
        None => out.push(0),
    }
    push_u32(out, snapshot.retrace_count);
    push_u32(out, snapshot.retrace_phase);
}

fn encode_vi_mode(out: &mut Vec<u8>, mode: fn64_abi::PendingViModeEvidenceSnapshot) {
    for register in mode.registers {
        push_u32(out, register);
    }
    for field in mode.fields {
        for register in field {
            push_u32(out, register);
        }
    }
}

fn encode_runtime_peripherals(
    out: &mut Vec<u8>,
    snapshot: fn64_abi::RuntimePeripheralEvidenceSnapshot,
) {
    let peripherals = snapshot.peripherals;
    encode_vi_manager(out, peripherals.vi);
    match peripherals.retrace {
        Some(retrace) => {
            out.push(1);
            push_u64(out, retrace.interval);
            push_u64(out, retrace.next_due);
        }
        None => out.push(0),
    }
    for state in peripherals.pif.ports {
        out.push(encode_port_state(state));
    }
    for input in peripherals.pif.inputs {
        push_u16(out, input.button);
        out.extend_from_slice(&[input.stick_x as u8, input.stick_y as u8]);
    }
    for active in peripherals.pif.rumble_on {
        out.push(active as u8);
    }
    for pak in peripherals.controller_paks {
        match pak {
            Some(pak) => {
                out.push(1);
                encode_controller_pak(out, pak);
            }
            None => out.push(0),
        }
    }
    for pak in peripherals.transfer_paks {
        match pak {
            Some(pak) => {
                out.push(1);
                encode_transfer_pak(out, pak);
            }
            None => out.push(0),
        }
    }
    for voice in peripherals.voice_units {
        match voice {
            Some(voice) => {
                out.push(1);
                encode_voice_unit(out, voice);
            }
            None => out.push(0),
        }
    }

    push_u64(out, snapshot.pending_pi_completions.len() as u64);
    for pending in snapshot.pending_pi_completions {
        encode_pi_request(out, pending.request);
        push_u64(out, pending.rdram_len);
        push_option_u32(out, pending.ret_queue.map(RdramAddr::offset));
        push_u32(out, pending.ret_mesg);
    }
    match snapshot.pending_si_completion {
        Some(pending) => {
            out.push(1);
            encode_si_request(out, pending.request);
            push_u64(out, pending.rdram_len);
        }
        None => out.push(0),
    }
    for mode in [snapshot.vi.pending_mode, snapshot.vi.active_mode] {
        match mode {
            Some(mode) => {
                out.push(1);
                encode_vi_mode(out, mode);
            }
            None => out.push(0),
        }
    }
    for value in [
        snapshot.vi.pending_control,
        snapshot.vi.pending_x_scale_bits,
        snapshot.vi.pending_y_scale_bits,
    ] {
        push_option_u32(out, value);
    }
    push_u32(out, snapshot.vi.active_x_scale_bits);
    push_u32(out, snapshot.vi.active_y_scale_bits);
}

fn encode_resume(out: &mut Vec<u8>, resume: fn64_runtime::Resume) {
    match resume {
        fn64_runtime::Resume::Start => out.push(0),
        fn64_runtime::Resume::Continue => out.push(1),
        fn64_runtime::Resume::Delivered(message) => {
            out.push(2);
            push_u32(out, message);
        }
        fn64_runtime::Resume::SendUnblocked => out.push(3),
        fn64_runtime::Resume::WouldBlock => out.push(4),
    }
}

fn encode_thread_state(state: fn64_runtime::ThreadState) -> u8 {
    match state {
        fn64_runtime::ThreadState::Stopped => 0,
        fn64_runtime::ThreadState::Runnable => 1,
        fn64_runtime::ThreadState::Running => 2,
        fn64_runtime::ThreadState::BlockedOnRecv => 3,
        fn64_runtime::ThreadState::BlockedOnSend => 4,
        fn64_runtime::ThreadState::Dead => 5,
    }
}

fn encode_executor_control(
    out: &mut Vec<u8>,
    snapshot: fn64_runtime::ExecutorControlEvidenceSnapshot,
) {
    match snapshot.rdram {
        fn64_runtime::RdramRegistrationEvidenceSnapshot::Absent => out.push(0),
        fn64_runtime::RdramRegistrationEvidenceSnapshot::LegacyUnbounded => out.push(1),
        fn64_runtime::RdramRegistrationEvidenceSnapshot::Present { len } => {
            out.push(2);
            push_u64(out, len);
        }
    }
    push_u64(out, snapshot.threads.len() as u64);
    for thread in snapshot.threads {
        push_u32(out, thread.id);
        push_u32(out, thread.priority as u32);
        out.push(encode_thread_state(thread.state));
        out.push(thread.started as u8);
    }
    push_u64(out, snapshot.run_queue.len() as u64);
    for thread in snapshot.run_queue {
        push_u32(out, thread);
    }
    push_u64(out, snapshot.pending_resumes.len() as u64);
    for pending in snapshot.pending_resumes {
        push_u32(out, pending.thread);
        encode_resume(out, pending.resume);
    }
    push_u64(out, snapshot.queues.len() as u64);
    for queue in snapshot.queues {
        push_u32(out, queue.address.offset());
        push_u64(out, queue.queue.capacity);
        push_u64(out, queue.queue.first);
        push_u64(out, queue.queue.messages.len() as u64);
        for message in queue.queue.messages {
            push_u32(out, message);
        }
        push_u64(out, queue.queue.blocked_receivers.len() as u64);
        for thread in queue.queue.blocked_receivers {
            push_u32(out, thread);
        }
        push_u64(out, queue.queue.blocked_senders.len() as u64);
        for sender in queue.queue.blocked_senders {
            push_u32(out, sender.id);
            push_u32(out, sender.msg);
            out.push(match sender.placement {
                fn64_runtime::SendPlacement::Tail => 0,
                fn64_runtime::SendPlacement::Head => 1,
            });
        }
    }
    push_u32(out, snapshot.timers.next_id);
    push_u64(out, snapshot.timers.firing_order.len() as u64);
    for timer in snapshot.timers.firing_order {
        push_u32(out, timer.id);
        push_u64(out, timer.deadline);
        push_u64(out, timer.interval);
        push_u32(out, timer.queue_addr.offset());
        push_u32(out, timer.msg);
        push_u32(out, timer.armed_by);
    }
    push_u64(out, snapshot.event_table.len() as u64);
    for registration in snapshot.event_table {
        push_u32(out, registration.event);
        push_u32(out, registration.queue_addr.offset());
        push_u32(out, registration.msg);
    }
    match snapshot.running {
        fn64_runtime::ExecutorRunningEvidenceSnapshot::Quiescent => out.push(0),
        fn64_runtime::ExecutorRunningEvidenceSnapshot::Active(thread) => {
            out.push(1);
            push_u32(out, thread);
        }
    }
    push_u64(out, snapshot.sim_time);
    push_u32(out, snapshot.cp0_count);
    out.push(snapshot.cp0_count_phase);
    push_u32(out, snapshot.cp0_compare);
    out.push(snapshot.cp0_timer_pending as u8);
}

fn encode_section_registry(
    out: &mut Vec<u8>,
    snapshot: fn64_runtime::SectionRegistryEvidenceSnapshot,
) {
    push_u64(out, snapshot.sections.len() as u64);
    for section in snapshot.sections {
        push_u32(out, section.rom_addr);
        push_u32(out, section.ram_addr);
        push_u32(out, section.size);
        push_u64(out, section.funcs.len() as u64);
        for function in section.funcs {
            push_u32(out, function.offset);
            push_u32(out, function.rom_size);
        }
    }
    push_u64(out, snapshot.loaded_sections.len() as u64);
    for section in snapshot.loaded_sections {
        push_u64(
            out,
            u64::try_from(section).expect("section index exceeds evidence wire"),
        );
    }
    push_u64(out, snapshot.runtime_loads.len() as u64);
    for load in snapshot.runtime_loads {
        push_u64(
            out,
            u64::try_from(load.section).expect("section index exceeds evidence wire"),
        );
        push_u32(out, load.load_vram);
    }
    match snapshot.static_mirror {
        Some(mirror) => {
            out.push(1);
            push_u64(
                out,
                u64::try_from(mirror.section).expect("section index exceeds evidence wire"),
            );
            push_u32(out, mirror.next_rom);
            push_u32(out, mirror.next_static_off);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.static_storage_ends.len() as u64);
    for storage in snapshot.static_storage_ends {
        push_u64(
            out,
            u64::try_from(storage.section).expect("section index exceeds evidence wire"),
        );
        push_u32(out, storage.end);
    }
}

fn encode_os_task_header(out: &mut Vec<u8>, header: &fn64_runtime::OsTaskHeader) {
    for word in [
        header.task_type,
        header.flags,
        header.ucode_boot,
        header.ucode_boot_size,
        header.ucode,
        header.ucode_size,
        header.ucode_data,
        header.ucode_data_size,
        header.dram_stack,
        header.dram_stack_size,
        header.output_buff,
        header.output_buff_size,
        header.data_ptr,
        header.data_size,
        header.yield_data_ptr,
        header.yield_data_size,
    ] {
        push_u32(out, word);
    }
}

fn encode_rsp_task_data_identity(
    out: &mut Vec<u8>,
    identity: Option<fn64_abi::RspTaskDataIdentityEvidenceSnapshot>,
) {
    match identity {
        Some(identity) => {
            out.push(1);
            push_u32(out, identity.rdram_offset);
            push_u32(out, identity.byte_len);
            out.extend_from_slice(&identity.sha256);
        }
        None => out.push(0),
    }
}

fn encode_abi_host(out: &mut Vec<u8>, snapshot: fn64_abi::AbiHostEvidenceSnapshot) {
    encode_runtime_peripherals(out, snapshot.runtime_peripherals);
    match snapshot.flash.write_buffer {
        Some(bytes) => {
            out.push(1);
            out.extend_from_slice(&bytes);
        }
        None => out.push(0),
    }
    out.push(snapshot.flash.erase_complete as u8);
    out.push(snapshot.flash.status);
    push_u32(out, snapshot.flash.identity.flash_type);
    push_u32(out, snapshot.flash.identity.flash_maker);
    encode_section_registry(out, snapshot.sections);
    push_u64(out, snapshot.rsp_boot_images.len() as u64);
    for image in snapshot.rsp_boot_images {
        push_u32(out, image.rdram_offset);
        push_bytes(out, &image.bytes);
    }
    match snapshot.loaded_rsp_task {
        Some(task) => {
            out.push(1);
            push_u32(out, task.task_offset);
            encode_os_task_header(out, &task.header);
            encode_rsp_task_data_identity(out, task.resumed_data_identity);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.rsp_task_lineages.len() as u64);
    for lineage in snapshot.rsp_task_lineages {
        push_u32(out, lineage.task_offset);
        encode_os_task_header(out, &lineage.original_header);
        encode_rsp_task_data_identity(out, lineage.data_identity);
        out.push(match lineage.phase {
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::Running => 0,
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized => 1,
            fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded => 2,
        });
    }
    out.push(snapshot.rom_installed as u8);
    match snapshot.installed_rom {
        Some(rom) => {
            out.push(1);
            push_u64(out, rom.byte_len);
            out.extend_from_slice(&rom.sha256);
        }
        None => out.push(0),
    }
    out.push(match snapshot.cartridge_save {
        fn64_abi::CartridgeSaveEvidenceSnapshot::Unidentified => 0,
        fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave => 1,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::Eeprom4k,
        ) => 2,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::Eeprom16k,
        ) => 3,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::SramBanked,
        ) => 4,
        fn64_abi::CartridgeSaveEvidenceSnapshot::Configured(
            fn64_abi::CartridgeSaveType::FlashRam,
        ) => 5,
    });
    push_option_u32(out, snapshot.cart_rom_handle_vram);
    push_option_u32(out, snapshot.flash_handle_vram);
    match snapshot.leo_disk {
        Some(disk) => {
            out.push(1);
            push_u32(out, disk.handle_vram);
            out.extend_from_slice(&[disk.latency, disk.page_size, disk.release, disk.pulse_width]);
        }
        None => out.push(0),
    }
    push_u64(out, snapshot.thread_handles.len() as u64);
    for handle in snapshot.thread_handles {
        push_u32(out, handle.osthread_offset);
        push_u32(out, handle.executor_thread_id);
    }
    push_u64(out, snapshot.thread_guest_ids.len() as u64);
    for guest in snapshot.thread_guest_ids {
        push_u32(out, guest.executor_thread_id);
        push_u32(out, guest.guest_os_id);
    }
    push_u64(out, snapshot.timer_handles.len() as u64);
    for handle in snapshot.timer_handles {
        push_u32(out, handle.ostimer_offset);
        push_u32(out, handle.timer_id);
    }
    push_u32(out, snapshot.next_synthetic_thread_id);
    out.push(snapshot.registered_rdram.present as u8);
    push_u64(out, snapshot.registered_rdram.byte_len);
    out.push(match snapshot.debug_hardware {
        fn64_abi::DebugHardware::None => 0,
        fn64_abi::DebugHardware::Msp => 1,
        fn64_abi::DebugHardware::Kmc => 2,
        fn64_abi::DebugHardware::Isv => 3,
    });
}

#[cfg(feature = "recomp-rs")]
fn encode_program_identity(
    out: &mut Vec<u8>,
    identity: fn64_recomp_rs::ProgramIdentityEvidenceSnapshot,
) {
    out.extend_from_slice(&identity.identity.bytes());
    out.push(match identity.source {
        fn64_recomp_rs::ProgramIdentitySource::CallerSupplied => 0,
        fn64_recomp_rs::ProgramIdentitySource::CanonicalBlockProgramSha256 => 1,
    });
}

fn encode_program(out: &mut Vec<u8>, snapshot: crate::ProgramEvidenceSnapshot) {
    match snapshot {
        crate::ProgramEvidenceSnapshot::NoProgram => out.push(0),
        crate::ProgramEvidenceSnapshot::UnidentifiedNativeProgram => out.push(1),
        crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(identity) => {
            out.push(2);
            out.extend_from_slice(&identity.bytes());
        }
        #[cfg(feature = "recomp-rs")]
        crate::ProgramEvidenceSnapshot::TypedRust(program) => match program {
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Function { identity } => {
                out.push(3);
                encode_program_identity(out, identity);
            }
            fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot::Block {
                program,
                dispatch_artifact_identity,
                instruction_budget,
                executable_regions,
                pending_executable_writes,
            } => {
                out.push(4);
                encode_program_identity(out, program.identity);
                push_u64(out, program.banks.len() as u64);
                for bank in program.banks {
                    push_u64(out, bank.id.get());
                    out.extend_from_slice(&bank.runner_artifact_identity.bytes());
                    push_u64(out, bank.spans.len() as u64);
                    for span in bank.spans {
                        push_u32(out, span.vram_start.get());
                        push_u64(out, span.words.len() as u64);
                        for word in span.words {
                            push_u32(out, word);
                        }
                    }
                }
                out.extend_from_slice(&dispatch_artifact_identity.bytes());
                push_u32(out, instruction_budget);
                push_u64(out, executable_regions.len() as u64);
                for region in executable_regions {
                    push_u32(out, region.physical_start);
                    push_u32(out, region.physical_end);
                    push_u32(out, region.virtual_start.get());
                    push_u32(out, region.virtual_end.get());
                    push_u64(out, region.active_bank.get());
                    push_u64(out, region.active_generation);
                    push_u64(out, region.next_generation);
                    out.extend_from_slice(&region.builder_artifact_identity.bytes());
                }
                push_u64(out, pending_executable_writes.len() as u64);
                for write in pending_executable_writes {
                    push_u32(out, write.physical_start);
                    push_u32(out, write.physical_end);
                }
            }
        },
    }
}

fn encode_device_snapshot(
    snapshot: DeviceEvidenceSnapshot,
    executor: fn64_runtime::ExecutorControlEvidenceSnapshot,
    host: fn64_abi::AbiHostEvidenceSnapshot,
    program: crate::ProgramEvidenceSnapshot,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 * 1024 + snapshot.save_bytes.as_ref().map_or(0, Vec::len));
    out.extend_from_slice(b"fn64.device-evidence.v7\0");
    encode_guest_device_snapshot(&mut out, snapshot.guest);
    push_bytes(&mut out, &snapshot.pi_timing_policy);

    match snapshot.pending_pi {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_pi_request(&mut out, pending.request);
        }
        None => out.push(0),
    }
    match snapshot.current_ai {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_ai_request(&mut out, pending.request);
            push_u64(&mut out, pending.started_at.get());
            push_u64(&mut out, pending.deadline.get());
        }
        None => out.push(0),
    }
    match snapshot.queued_ai {
        Some(request) => {
            out.push(1);
            encode_ai_request(&mut out, request);
        }
        None => out.push(0),
    }
    match snapshot.pending_si {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_si_request(&mut out, pending.request);
        }
        None => out.push(0),
    }
    out.push(snapshot.si_dma_error as u8);
    push_u64(&mut out, snapshot.si_latency.get());
    out.extend_from_slice(&snapshot.pif_ram);

    out.extend_from_slice(&snapshot.rsp_dmem);
    out.extend_from_slice(&snapshot.rsp_imem);
    for value in [snapshot.sp_rd_len, snapshot.sp_wr_len, snapshot.sp_pc] {
        push_u32(&mut out, value);
    }
    out.push(snapshot.sp_semaphore as u8);
    match snapshot.active_sp_dma {
        Some(pending) => {
            out.push(1);
            push_u64(&mut out, pending.token);
            encode_sp_dma_request(&mut out, pending.request);
        }
        None => out.push(0),
    }
    match snapshot.queued_sp_dma {
        Some(request) => {
            out.push(1);
            encode_sp_dma_request(&mut out, request);
        }
        None => out.push(0),
    }
    push_u64(&mut out, snapshot.sp_dma_setup_cycles.get());

    for value in snapshot.vi_registers {
        push_u32(&mut out, value);
    }
    push_u64(&mut out, snapshot.vi_epoch.get());
    for token in [
        snapshot.pending_vi_token,
        snapshot.pending_sp_token,
        snapshot.pending_dp_token,
    ] {
        match token {
            Some(token) => {
                out.push(1);
                push_u64(&mut out, token);
            }
            None => out.push(0),
        }
    }
    push_u64(&mut out, snapshot.scheduled_events.len() as u64);
    for event in snapshot.scheduled_events {
        push_u64(&mut out, event.at.get());
        push_u64(&mut out, event.sequence);
        push_u64(&mut out, event.token);
        out.push(match event.kind {
            ScheduledDeviceEventKind::Pi => 0,
            ScheduledDeviceEventKind::Ai => 1,
            ScheduledDeviceEventKind::Si => 2,
            ScheduledDeviceEventKind::SpDma => 3,
            ScheduledDeviceEventKind::Vi => 4,
            ScheduledDeviceEventKind::Sp => 5,
            ScheduledDeviceEventKind::Dp => 6,
        });
    }
    push_u64(&mut out, snapshot.next_event_sequence);

    match snapshot.save_bytes {
        Some(bytes) => {
            out.push(1);
            push_bytes(&mut out, &bytes);
        }
        None => out.push(0),
    }
    match snapshot.pending_eeprom_write {
        Some(pending) => {
            out.push(1);
            push_u32(&mut out, pending.offset);
            out.extend_from_slice(&pending.data);
            push_u64(&mut out, pending.ready_at.get());
        }
        None => out.push(0),
    }
    encode_executor_control(&mut out, executor);
    encode_abi_host(&mut out, host);
    encode_program(&mut out, program);
    out
}

fn encode_timing_trace(events: &[TraceEvent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(events.len() * 32);
    push_u64(&mut out, events.len() as u64);
    for event in events {
        push_u64(&mut out, event.sim_time);
        match event.kind {
            TraceKind::ThreadSwitch { from, to, reason } => {
                out.push(0);
                push_u32(&mut out, from.unwrap_or(u32::MAX));
                push_u32(&mut out, to);
                out.push(match reason {
                    SwitchReason::PauseSelf => 0,
                    SwitchReason::BlockedOnRecv => 1,
                    SwitchReason::BlockedOnSend => 2,
                    SwitchReason::Woken => 3,
                    SwitchReason::TimerFired => 4,
                    SwitchReason::Scheduled => 5,
                });
            }
            TraceKind::QueueOp { queue, op, thread } => {
                out.push(1);
                push_u32(&mut out, queue.offset());
                out.push(match op {
                    QueueOpKind::Send => 0,
                    QueueOpKind::Recv => 1,
                    QueueOpKind::Block => 2,
                    QueueOpKind::Wake => 3,
                });
                push_u32(&mut out, thread);
            }
            TraceKind::Dma {
                direction,
                dram,
                dev_addr,
                len,
            } => {
                out.push(2);
                out.push(match direction {
                    DmaDirection::ToRdram => 0,
                    DmaDirection::FromRdram => 1,
                });
                push_u32(&mut out, dram.offset());
                push_u32(&mut out, dev_addr);
                push_u32(&mut out, len);
            }
            TraceKind::TaskSubmit { task_kind, ucode } => {
                out.push(3);
                out.push(match task_kind {
                    TaskKind::Graphics => 0,
                    TaskKind::Audio => 1,
                });
                push_u32(&mut out, ucode);
            }
        }
    }
    out
}

fn encode_device_dma_trace(events: &[DeviceTraceEvent]) -> Vec<u8> {
    let dma_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DeviceTraceKind::PiDmaStarted(_)
                    | DeviceTraceKind::PiBytesCommitted(_)
                    | DeviceTraceKind::AiDmaStarted(_)
                    | DeviceTraceKind::AiDmaComplete(_)
                    | DeviceTraceKind::SiDmaStarted(_)
                    | DeviceTraceKind::SiBytesCommitted(_)
                    | DeviceTraceKind::SpDmaStarted(_)
                    | DeviceTraceKind::SpDmaQueued(_)
                    | DeviceTraceKind::SpDmaBytesCommitted(_)
                    | DeviceTraceKind::SpTaskAdmitted { .. }
            )
        })
        .count();
    let mut out = Vec::with_capacity(dma_count * 32);
    push_u64(&mut out, dma_count as u64);
    for event in events {
        let tag = match event.kind {
            DeviceTraceKind::PiDmaStarted(_) => 0,
            DeviceTraceKind::PiBytesCommitted(_) => 1,
            DeviceTraceKind::AiDmaStarted(_) => 2,
            DeviceTraceKind::AiDmaComplete(_) => 3,
            DeviceTraceKind::SiDmaStarted(_) => 4,
            DeviceTraceKind::SiBytesCommitted(_) => 5,
            DeviceTraceKind::SpDmaStarted(_) => 6,
            DeviceTraceKind::SpDmaQueued(_) => 7,
            DeviceTraceKind::SpDmaBytesCommitted(_) => 8,
            DeviceTraceKind::SpTaskAdmitted { .. } => 9,
            _ => continue,
        };
        push_u64(&mut out, event.at.get());
        out.push(tag);
        match event.kind {
            DeviceTraceKind::PiDmaStarted(request) | DeviceTraceKind::PiBytesCommitted(request) => {
                out.push(match request.direction {
                    DmaDirection::ToRdram => 0,
                    DmaDirection::FromRdram => 1,
                });
                push_u32(&mut out, request.dram_addr.offset());
                push_u32(&mut out, request.cart_addr);
                push_u32(&mut out, request.len);
            }
            DeviceTraceKind::AiDmaStarted(request) | DeviceTraceKind::AiDmaComplete(request) => {
                push_u32(&mut out, request.dram_addr.offset());
                push_u32(&mut out, request.len);
                push_u32(&mut out, request.sample_rate_hz);
            }
            DeviceTraceKind::SiDmaStarted(request) | DeviceTraceKind::SiBytesCommitted(request) => {
                out.push(match request.kind {
                    SiDmaKind::DramToPif => 0,
                    SiDmaKind::PifToDram => 1,
                    SiDmaKind::ControllerQuery => 2,
                    SiDmaKind::ControllerRead => 3,
                });
                push_u32(&mut out, request.dram_addr.offset());
            }
            DeviceTraceKind::SpDmaStarted(request)
            | DeviceTraceKind::SpDmaQueued(request)
            | DeviceTraceKind::SpDmaBytesCommitted(request) => {
                out.push(match request.direction {
                    SpDmaDirection::RdramToRsp => 0,
                    SpDmaDirection::RspToRdram => 1,
                });
                push_u32(
                    &mut out,
                    u32::try_from(request.mem_addr.offset()).expect("RSP DMA offset fits u32"),
                );
                push_u32(&mut out, request.dram_addr.offset());
                push_u32(&mut out, request.encoded_len);
            }
            DeviceTraceKind::SpTaskAdmitted { task_addr, header } => {
                push_u32(&mut out, task_addr.offset());
                for value in [
                    header.task_type,
                    header.flags,
                    header.ucode_boot,
                    header.ucode_boot_size,
                    header.ucode,
                    header.ucode_size,
                    header.ucode_data,
                    header.ucode_data_size,
                    header.dram_stack,
                    header.dram_stack_size,
                    header.output_buff,
                    header.output_buff_size,
                    header.data_ptr,
                    header.data_size,
                    header.yield_data_ptr,
                    header.yield_data_size,
                ] {
                    push_u32(&mut out, value);
                }
            }
            _ => unreachable!("device DMA tag and request encoding diverged"),
        }
    }
    out
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    push_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn encode_execution_destination(
    out: &mut Vec<u8>,
    destination: &ReleaseExecutionDestination,
) -> Result<(), GateError> {
    match destination {
        ReleaseExecutionDestination::Native {
            section_index,
            function_offset,
            link_vram,
        } => {
            out.push(0);
            push_u32(out, *section_index);
            push_u32(out, *function_offset);
            push_u32(out, *link_vram);
        }
        ReleaseExecutionDestination::TypedFunction { vram, symbol } => {
            out.push(1);
            push_u32(out, *vram);
            push_bytes(out, symbol.as_bytes());
        }
        ReleaseExecutionDestination::TypedBlock {
            bank,
            pc,
            runner_artifact_sha256,
        } => {
            out.push(2);
            push_u64(out, *bank);
            push_u32(out, *pc);
            out.extend_from_slice(&decode_sha256(runner_artifact_sha256).ok_or(
                GateError::InvalidReportSha256(
                    "execution_destinations.ordered[].runner_artifact_sha256",
                ),
            )?);
        }
    }
    Ok(())
}

fn encode_ordered_execution_destinations(
    ordered: &[ExecutionDestinationEventEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.ordered.v2\0");
    push_u64(&mut out, ordered.len() as u64);
    for event in ordered {
        match event.guest_cycle {
            Some(cycle) => {
                out.push(1);
                push_u64(&mut out, cycle);
            }
            None => out.push(0),
        }
        encode_execution_destination(&mut out, &event.destination)?;
    }
    Ok(out)
}

fn encode_unique_execution_destinations(
    unique: &[ExecutionDestinationCountEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.unique.v2\0");
    push_u64(&mut out, unique.len() as u64);
    for entry in unique {
        encode_execution_destination(&mut out, &entry.destination)?;
        push_u64(&mut out, entry.observations);
    }
    Ok(out)
}

pub(crate) fn encode_execution_destination_evidence(
    evidence: &ExecutionDestinationEvidence,
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.execution-destinations.evidence.v2\0");
    match &evidence.source {
        ExecutionDestinationSource::NoProgram => out.push(0),
        ExecutionDestinationSource::NativeArchive { artifact_sha256 } => {
            out.push(1);
            out.extend_from_slice(&decode_sha256(artifact_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.artifact_sha256"),
            )?);
        }
        ExecutionDestinationSource::TypedBlockProgram {
            program_sha256,
            dispatch_artifact_sha256,
        } => {
            out.push(3);
            out.extend_from_slice(&decode_sha256(program_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.program_sha256"),
            )?);
            out.extend_from_slice(&decode_sha256(dispatch_artifact_sha256).ok_or(
                GateError::InvalidReportSha256(
                    "execution_destinations.source.dispatch_artifact_sha256",
                ),
            )?);
        }
        ExecutionDestinationSource::TypedObservedFunctionProgram { artifact_sha256 } => {
            out.push(2);
            out.extend_from_slice(&decode_sha256(artifact_sha256).ok_or(
                GateError::InvalidReportSha256("execution_destinations.source.artifact_sha256"),
            )?);
        }
    }
    push_u64(&mut out, evidence.total_observations);
    push_u64(&mut out, evidence.unique_destinations);
    out.extend_from_slice(&decode_sha256(&evidence.ordered_sha256).ok_or(
        GateError::InvalidReportSha256("execution_destinations.ordered_sha256"),
    )?);
    out.extend_from_slice(&decode_sha256(&evidence.unique_sha256).ok_or(
        GateError::InvalidReportSha256("execution_destinations.unique_sha256"),
    )?);
    push_bytes(
        &mut out,
        &encode_ordered_execution_destinations(&evidence.ordered)?,
    );
    push_bytes(
        &mut out,
        &encode_unique_execution_destinations(&evidence.unique)?,
    );
    Ok(out)
}

fn validate_rsp_rdp_observations(
    gate_cycle: u64,
    observations: &[RspRdpObservationEventEvidence],
) -> Result<(), GateError> {
    let mut previous_cycle = None;
    let mut previous_imem_generation = None;
    let mut imem_generation_digests = BTreeMap::<u64, &str>::new();
    for event in observations {
        if event.guest_cycle > gate_cycle {
            return Err(GateError::FutureRspRdpObservation {
                gate_cycle,
                event_cycle: event.guest_cycle,
            });
        }
        if previous_cycle.is_some_and(|previous| event.guest_cycle < previous) {
            return Err(GateError::NonMonotonicRspRdpObservationCycle {
                previous: previous_cycle.expect("checked RSP/RDP observation cycle"),
                observed: event.guest_cycle,
            });
        }
        previous_cycle = Some(event.guest_cycle);
        match &event.observation {
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address,
                imem_generation,
                text_sha256,
                data_address,
                data_bytes,
                data_sha256,
                ..
            } => {
                validate_rsp_task_observation_address(*task_address)?;
                decode_sha256(text_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.text_sha256",
                ))?;
                decode_sha256(data_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.data_sha256",
                ))?;
                validate_microcode_data_observation_range(*data_address, *data_bytes)?;
                validate_imem_generation_digest(
                    &mut imem_generation_digests,
                    *imem_generation,
                    text_sha256,
                )?;
                if previous_imem_generation.is_some_and(|previous| *imem_generation < previous) {
                    return Err(GateError::NonMonotonicImemGeneration {
                        previous: previous_imem_generation.expect("checked RSP IMEM generation"),
                        observed: *imem_generation,
                    });
                }
                previous_imem_generation = Some(*imem_generation);
            }
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                validate_dpc_observation_range(
                    *start,
                    *end,
                    crate::DEFAULT_RDRAM_SIZE as u32,
                    "DRAM",
                )?;
                decode_sha256(command_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.command_sha256",
                ))?;
            }
            RspRdpObservationKindEvidence::XbusDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                validate_dpc_observation_range(*start, *end, 0x1000, "XBUS")?;
                decode_sha256(command_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.command_sha256",
                ))?;
            }
            RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address,
                imem_generation,
                text_sha256,
            } => {
                validate_rsp_task_observation_address(*task_address)?;
                decode_sha256(text_sha256).ok_or(GateError::InvalidReportSha256(
                    "rsp_rdp.ordered[].observation.text_sha256",
                ))?;
                validate_imem_generation_digest(
                    &mut imem_generation_digests,
                    *imem_generation,
                    text_sha256,
                )?;
                if previous_imem_generation.is_some_and(|previous| *imem_generation <= previous) {
                    return Err(GateError::NonMonotonicImemReplacementGeneration {
                        previous: previous_imem_generation.expect("checked RSP IMEM generation"),
                        observed: *imem_generation,
                    });
                }
                previous_imem_generation = Some(*imem_generation);
            }
        }
    }
    Ok(())
}

fn validate_imem_generation_digest<'a>(
    generations: &mut BTreeMap<u64, &'a str>,
    generation: u64,
    text_sha256: &'a str,
) -> Result<(), GateError> {
    if let Some(previous) = generations.insert(generation, text_sha256) {
        if previous != text_sha256 {
            return Err(GateError::ConflictingImemGenerationDigest {
                generation,
                previous: previous.to_owned(),
                observed: text_sha256.to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_dpc_observation_range(
    start: u32,
    end: u32,
    limit: u32,
    source: &'static str,
) -> Result<(), GateError> {
    if start >= end || !start.is_multiple_of(8) || !end.is_multiple_of(8) || end > limit {
        return Err(GateError::InvalidDpcObservationRange {
            source,
            start,
            end,
            limit,
        });
    }
    Ok(())
}

fn validate_microcode_data_observation_range(start: u32, bytes: u32) -> Result<(), GateError> {
    let limit = u32::try_from(crate::DEFAULT_RDRAM_SIZE).expect("release RDRAM size fits u32");
    let valid =
        bytes != 0 && start < limit && start.checked_add(bytes).is_some_and(|end| end <= limit);
    if !valid {
        return Err(GateError::InvalidMicrocodeDataObservationRange {
            start,
            bytes,
            limit,
        });
    }
    Ok(())
}

fn validate_rsp_task_observation_address(address: u32) -> Result<(), GateError> {
    const OS_TASK_HEADER_BYTES: u32 = 64;
    let limit = u32::try_from(crate::DEFAULT_RDRAM_SIZE).expect("release RDRAM size fits u32");
    if address
        .checked_add(OS_TASK_HEADER_BYTES)
        .is_none_or(|end| end > limit)
    {
        return Err(GateError::InvalidRspTaskObservationAddress { address, limit });
    }
    Ok(())
}

pub(crate) fn encode_rsp_rdp_observations(
    observations: &[RspRdpObservationEventEvidence],
) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"fn64.rsp-rdp-observations.v2\0");
    push_u64(&mut out, observations.len() as u64);
    for event in observations {
        push_u64(&mut out, event.guest_cycle);
        match &event.observation {
            RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address,
                imem_generation,
                text_sha256,
                data_address,
                data_bytes,
                data_sha256,
                family,
            } => {
                out.push(0);
                push_u32(&mut out, *task_address);
                push_u64(&mut out, *imem_generation);
                out.extend_from_slice(&decode_sha256(text_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.text_sha256"),
                )?);
                push_u32(&mut out, *data_address);
                push_u32(&mut out, *data_bytes);
                out.extend_from_slice(&decode_sha256(data_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.data_sha256"),
                )?);
                match family {
                    Some(family) => {
                        out.push(1);
                        family.encode(&mut out);
                    }
                    None => out.push(0),
                }
            }
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                out.push(1);
                push_u32(&mut out, *start);
                push_u32(&mut out, *end);
                out.extend_from_slice(&decode_sha256(command_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.command_sha256"),
                )?);
            }
            RspRdpObservationKindEvidence::XbusDpcCommitted {
                start,
                end,
                command_sha256,
            } => {
                out.push(2);
                push_u32(&mut out, *start);
                push_u32(&mut out, *end);
                out.extend_from_slice(&decode_sha256(command_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.command_sha256"),
                )?);
            }
            RspRdpObservationKindEvidence::ImemReplacementCommitted {
                task_address,
                imem_generation,
                text_sha256,
            } => {
                out.push(3);
                push_u32(&mut out, *task_address);
                push_u64(&mut out, *imem_generation);
                out.extend_from_slice(&decode_sha256(text_sha256).ok_or(
                    GateError::InvalidReportSha256("rsp_rdp.ordered[].observation.text_sha256"),
                )?);
            }
        }
    }
    Ok(out)
}

/// Encode a complete report without `report_sha256` itself. This is an
/// evidence wire format, so it does not depend on JSON key order or serializer
/// formatting.
pub(crate) fn encode_report_evidence(report: &ReleaseGateReport) -> Result<Vec<u8>, GateError> {
    let mut out = Vec::new();
    push_bytes(&mut out, report.schema.as_bytes());
    push_bytes(&mut out, report.scenario.as_bytes());
    out.extend_from_slice(
        &decode_sha256(&report.input_sha256)
            .ok_or(GateError::InvalidReportSha256("input_sha256"))?,
    );
    match &report.rom {
        Some(rom) => {
            rom.verify_integrity()?;
            out.push(1);
            out.push(rom.class.tag());
            out.push(rom.source_byte_order.tag());
            push_u64(&mut out, rom.byte_len);
            out.extend_from_slice(
                &decode_sha256(&rom.canonical_sha256)
                    .ok_or(GateError::InvalidReportSha256("rom.canonical_sha256"))?,
            );
            out.push(rom.destination_code);
            out.push(rom.decoded_tv_region.tag());
            out.push(rom.configured_tv_type.tag());
        }
        None => out.push(0),
    }
    push_u64(&mut out, report.digest.guest_cycle);
    push_u64(&mut out, report.digest.artifacts.len() as u64);
    for artifact in &report.digest.artifacts {
        push_bytes(&mut out, artifact.kind.tag());
        push_u64(&mut out, artifact.bytes);
        out.extend_from_slice(
            &decode_sha256(&artifact.sha256)
                .ok_or(GateError::InvalidReportSha256("digest.artifacts[].sha256"))?,
        );
    }
    out.extend_from_slice(
        &decode_sha256(&report.digest.root_sha256)
            .ok_or(GateError::InvalidReportSha256("digest.root_sha256"))?,
    );
    let framebuffer = &report.observations.framebuffer;
    match &framebuffer.source {
        FramebufferObservationSource::PhysicalRdram { address } => {
            out.push(0);
            push_u32(&mut out, *address);
        }
        FramebufferObservationSource::PostViSwapchain {
            backend_identity,
            settings_sha256,
            workload_id,
            present_id,
        } => {
            out.push(1);
            push_bytes(&mut out, backend_identity.as_bytes());
            out.extend_from_slice(&decode_sha256(settings_sha256).ok_or(
                GateError::InvalidReportSha256("observations.framebuffer.source.settings_sha256"),
            )?);
            push_u64(&mut out, workload_id.get());
            push_u64(&mut out, *present_id);
        }
    }
    push_u32(&mut out, framebuffer.width);
    push_u32(&mut out, framebuffer.height);
    push_u32(&mut out, framebuffer.row_bytes);
    out.push(framebuffer.format.tag());
    push_u64(&mut out, framebuffer.payload_bytes);
    push_u32(&mut out, report.observations.memory.physical_address);
    push_u64(&mut out, report.observations.memory.payload_bytes);
    out.push(match report.environment.platform {
        ReleaseHostPlatform::MacosArm64 => 0,
        ReleaseHostPlatform::LinuxX86_64 => 1,
        ReleaseHostPlatform::WindowsX86_64 => 2,
    });
    match report.environment.windows_version {
        None => out.push(0),
        Some(version) => {
            out.push(1);
            out.push(match version.family {
                ReleaseWindowsFamily::Windows10 => 0,
                ReleaseWindowsFamily::Windows11 => 1,
            });
            push_u32(&mut out, version.major);
            push_u32(&mut out, version.minor);
            push_u32(&mut out, version.build);
            push_u32(&mut out, version.update_build_revision);
            out.push(match version.product_type {
                ReleaseWindowsProductType::Workstation => 0,
            });
        }
    }
    for port in report.environment.controller_ports {
        out.push(match port {
            ReleaseControllerPort::StandardControllerNoPak => 0,
            ReleaseControllerPort::StandardControllerControllerPak => 1,
            ReleaseControllerPort::StandardControllerRumblePak => 2,
            ReleaseControllerPort::StandardControllerTransferPak => 3,
            ReleaseControllerPort::VoiceRecognitionUnit => 4,
            ReleaseControllerPort::Absent => 5,
        });
    }
    out.push(match report.environment.cartridge_save {
        ReleaseCartridgeSave::NoCartridgeSave => 0,
        ReleaseCartridgeSave::Eeprom4k => 1,
        ReleaseCartridgeSave::Eeprom16k => 2,
        ReleaseCartridgeSave::Sram32Kib => 3,
        ReleaseCartridgeSave::FlashRam128Kib => 4,
    });
    match &report.environment.renderer {
        ReleaseRendererEvidence::Reference {
            execution_policy, ..
        } => {
            out.push(0);
            out.push(encode_graphics_execution_policy(*execution_policy));
            out.push(report.environment.renderer.tv_type().tag());
        }
        ReleaseRendererEvidence::Rt64 {
            execution_policy,
            graphics_api,
            backend_identity,
            source_authoritative,
            settings_sha256,
            replacement_packs_active,
            ..
        } => {
            out.push(1);
            out.push(encode_graphics_execution_policy(*execution_policy));
            out.push(report.environment.renderer.tv_type().tag());
            out.push(encode_graphics_api(*graphics_api));
            push_bytes(&mut out, backend_identity.as_bytes());
            out.push(*source_authoritative as u8);
            out.extend_from_slice(&decode_sha256(settings_sha256).ok_or(
                GateError::InvalidReportSha256("environment.renderer.settings_sha256"),
            )?);
            out.push(*replacement_packs_active as u8);
        }
    }
    push_bytes(
        &mut out,
        &encode_execution_destination_evidence(&report.execution_destinations)?,
    );
    push_bytes(
        &mut out,
        &encode_rsp_rdp_observations(&report.rsp_rdp.ordered)?,
    );
    push_u64(&mut out, report.rsp_rdp.total_observations);
    out.extend_from_slice(
        &decode_sha256(&report.rsp_rdp.ordered_sha256)
            .ok_or(GateError::InvalidReportSha256("rsp_rdp.ordered_sha256"))?,
    );
    push_u64(&mut out, report.closure.len() as u64);
    for path in &report.closure {
        push_bytes(&mut out, path.name.as_bytes());
        push_u64(&mut out, path.observations);
        out.push(match path.status {
            ClosurePathStatus::Unexercised => 0,
            ClosurePathStatus::ExercisedZeroUnsupported => 1,
            ClosurePathStatus::ExercisedUnsupported => 2,
        });
        push_u64(&mut out, path.unsupported.len() as u64);
        for event in &path.unsupported {
            push_bytes(&mut out, event.subsystem.as_bytes());
            push_bytes(&mut out, event.operation.as_bytes());
            push_bytes(&mut out, event.context.as_bytes());
            match event.guest_cycle {
                Some(cycle) => {
                    out.push(1);
                    push_u64(&mut out, cycle);
                }
                None => out.push(0),
            }
            push_bytes(&mut out, event.disposition.as_bytes());
        }
    }
    Ok(out)
}

const fn encode_graphics_execution_policy(policy: ReleaseGraphicsExecutionPolicy) -> u8 {
    match policy {
        ReleaseGraphicsExecutionPolicy::HleOptimized => 0,
        ReleaseGraphicsExecutionPolicy::LleAccuracy => 1,
    }
}

const fn encode_graphics_api(api: ReleaseGraphicsApi) -> u8 {
    match api {
        ReleaseGraphicsApi::D3d12 => 0,
        ReleaseGraphicsApi::Vulkan => 1,
        ReleaseGraphicsApi::Metal => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_runtime::{
        AiDmaRequest, Cycles, DeviceEvidenceSnapshot, DeviceSnapshot, OsTaskHeader, PiDmaRequest,
        PiDomainTiming, RdramAddr, RspMemAddr, SaveOperationKind, SiDmaRequest, TvType,
        RSP_MEMORY_BANK_SIZE,
    };

    fn observations() -> ReleaseObservationGeometry {
        ReleaseObservationGeometry::reference_rdram(0, 1, 1).unwrap()
    }

    fn test_rom(destination_code: u8) -> Vec<u8> {
        let mut rom = vec![0; 0x1000];
        rom[..4].copy_from_slice(&MAGIC_Z64.to_be_bytes());
        rom[0x3b..0x3f].copy_from_slice(&[b'N', b'F', b'6', destination_code]);
        rom
    }

    fn n64_order(canonical: &[u8]) -> Vec<u8> {
        canonical
            .chunks_exact(4)
            .flat_map(|word| [word[3], word[2], word[1], word[0]])
            .collect()
    }

    fn v64_order(canonical: &[u8]) -> Vec<u8> {
        canonical
            .chunks_exact(2)
            .flat_map(|pair| [pair[1], pair[0]])
            .collect()
    }

    #[test]
    fn schema_v20_rom_identity_normalizes_byte_order_and_decodes_every_tv_class() {
        let ntsc = test_rom(b'E');
        let expected =
            ReleaseRomEvidence::from_bytes(&ntsc, ReleaseRomClass::RetailCartridge, TvType::Ntsc)
                .unwrap();
        assert_eq!(expected.source_byte_order, ReleaseRomByteOrder::Z64);
        assert_eq!(expected.decoded_tv_region, ReleaseTvRegion::Ntsc);
        assert_eq!(
            ReleaseRomEvidence::decode_tv_type(&ntsc).unwrap(),
            Some(TvType::Ntsc)
        );

        for (bytes, order) in [
            (n64_order(&ntsc), ReleaseRomByteOrder::N64),
            (v64_order(&ntsc), ReleaseRomByteOrder::V64),
        ] {
            let observed = ReleaseRomEvidence::from_bytes(
                &bytes,
                ReleaseRomClass::RetailCartridge,
                TvType::Ntsc,
            )
            .unwrap();
            assert_eq!(observed.source_byte_order, order);
            assert_eq!(observed.canonical_sha256, expected.canonical_sha256);
        }

        let pal = ReleaseRomEvidence::from_bytes(
            &test_rom(b'P'),
            ReleaseRomClass::PublicHomebrew,
            TvType::Pal,
        )
        .unwrap();
        assert_eq!(pal.decoded_tv_region, ReleaseTvRegion::Pal);
        let mpal = ReleaseRomEvidence::from_bytes(
            &test_rom(b'B'),
            ReleaseRomClass::Unclassified,
            TvType::Mpal,
        )
        .unwrap();
        assert_eq!(mpal.decoded_tv_region, ReleaseTvRegion::Mpal);

        for destination_code in [0, b'A'] {
            let region_free = test_rom(destination_code);
            assert_eq!(
                ReleaseRomEvidence::decode_tv_type(&region_free).unwrap(),
                None
            );
            for tv_type in [TvType::Ntsc, TvType::Pal, TvType::Mpal] {
                assert_eq!(
                    ReleaseRomEvidence::from_bytes(
                        &region_free,
                        ReleaseRomClass::PublicHomebrew,
                        tv_type,
                    )
                    .unwrap()
                    .decoded_tv_region,
                    ReleaseTvRegion::RegionFree
                );
            }
        }
    }

    #[test]
    fn schema_v20_rom_decode_rejects_unknown_or_inconsistent_authority() {
        assert!(matches!(
            ReleaseRomEvidence::from_bytes(
                &test_rom(b'E'),
                ReleaseRomClass::RetailCartridge,
                TvType::Pal,
            ),
            Err(GateError::RomTvTypeMismatch { .. })
        ));
        assert!(matches!(
            ReleaseRomEvidence::decode_tv_type(&test_rom(b'?')),
            Err(GateError::UnknownRomDestinationCode(b'?'))
        ));
        let mut unknown_order = test_rom(b'E');
        unknown_order[..4].fill(0);
        assert!(matches!(
            ReleaseRomEvidence::decode_tv_type(&unknown_order),
            Err(GateError::UnknownRomByteOrder { .. })
        ));
        assert!(matches!(
            ReleaseRomEvidence::decode_tv_type(&[0; 63]),
            Err(GateError::RomTooSmall { bytes: 63 })
        ));
        assert!(matches!(
            ReleaseRomEvidence::decode_tv_type(&[0; 65]),
            Err(GateError::RomNotWordAligned { bytes: 65 })
                | Err(GateError::UnknownRomByteOrder { .. })
        ));
    }

    fn authoritative_rt64_identity_for(graphics_api: ReleaseGraphicsApi) -> String {
        let post_vi_api = match graphics_api {
            ReleaseGraphicsApi::D3d12 => "d3d12-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Vulkan => "vulkan-bgra8-rgba8-unorm",
            ReleaseGraphicsApi::Metal => "metal-bgra8-unorm",
        };
        format!(
            "adapter=fn64-render-rt64/rt64;adapter_sha256={};source=git:{};provenance=git-clean;overlay=test;post_vi_api={post_vi_api}",
            "a".repeat(64),
            "b".repeat(40),
        )
    }

    fn authoritative_rt64_identity() -> String {
        authoritative_rt64_identity_for(current_test_graphics_api())
    }

    fn current_test_graphics_api() -> ReleaseGraphicsApi {
        match crate::release_host_platform().unwrap() {
            ReleaseHostPlatform::MacosArm64 => ReleaseGraphicsApi::Metal,
            ReleaseHostPlatform::LinuxX86_64 => ReleaseGraphicsApi::Vulkan,
            ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::D3d12,
        }
    }

    fn snapshot(cycle: u64) -> DeviceEvidenceSnapshot {
        DeviceEvidenceSnapshot {
            guest: DeviceSnapshot {
                now: Cycles::new(cycle),
                pi_dram_addr: RdramAddr::from_offset(0x100),
                pi_cart_addr: 0x1000_1000,
                pi_status: 1,
                ai_status: 0,
                ai_length: 0x200,
                si_dram_addr: RdramAddr::from_offset(0x200),
                si_status: 0,
                vi_current: 20,
                vi_intr: 2,
                vi_v_sync: 525,
                tv_type: Some(TvType::Ntsc),
                vi_field_interval: Some(Cycles::new(781_250)),
                sp_busy: false,
                sp_status: 1,
                sp_mem_addr: RspMemAddr::from_register(0x40),
                sp_dram_addr: RdramAddr::from_offset(0x300),
                sp_imem_generation: 2,
                dp_busy: false,
                mi_pending: 8,
                mi_mask: 8,
                pi_domain1: PiDomainTiming::default(),
                pi_domain2: PiDomainTiming::default(),
            },
            pi_timing_policy: b"test-policy".to_vec(),
            pending_pi: None,
            current_ai: None,
            queued_ai: None,
            pending_si: None,
            si_dma_error: false,
            si_latency: Cycles::new(1),
            pif_ram: [0; 64],
            rsp_dmem: [0; RSP_MEMORY_BANK_SIZE],
            rsp_imem: [0; RSP_MEMORY_BANK_SIZE],
            sp_rd_len: 0,
            sp_wr_len: 0,
            sp_pc: 0,
            sp_semaphore: false,
            active_sp_dma: None,
            queued_sp_dma: None,
            sp_dma_setup_cycles: Cycles::new(8),
            vi_registers: [0; 14],
            vi_epoch: Cycles::ZERO,
            pending_vi_token: None,
            pending_sp_token: None,
            pending_dp_token: None,
            scheduled_events: Vec::new(),
            next_event_sequence: 0,
            save_bytes: None,
            pending_eeprom_write: None,
        }
    }

    fn peripherals_snapshot() -> fn64_abi::RuntimePeripheralEvidenceSnapshot {
        fn64_abi::RuntimePeripheralEvidenceSnapshot {
            peripherals: fn64_runtime::Peripherals::new().evidence_snapshot(),
            pending_pi_completions: Vec::new(),
            pending_si_completion: None,
            vi: fn64_abi::AbiViEvidenceSnapshot {
                pending_mode: None,
                active_mode: None,
                pending_control: None,
                pending_x_scale_bits: None,
                pending_y_scale_bits: None,
                active_x_scale_bits: 1.0f32.to_bits(),
                active_y_scale_bits: 1.0f32.to_bits(),
            },
        }
    }

    fn executor_snapshot() -> fn64_runtime::ExecutorControlEvidenceSnapshot {
        fn64_runtime::Executor::new().control_evidence_snapshot()
    }

    fn host_snapshot() -> fn64_abi::AbiHostEvidenceSnapshot {
        let mut snapshot = fn64_abi::host_evidence_snapshot();
        snapshot.runtime_peripherals = peripherals_snapshot();
        snapshot
    }

    fn encode_test_device(
        device: DeviceEvidenceSnapshot,
        peripherals: fn64_abi::RuntimePeripheralEvidenceSnapshot,
    ) -> Vec<u8> {
        let mut host = host_snapshot();
        host.runtime_peripherals = peripherals;
        encode_device_snapshot(
            device,
            executor_snapshot(),
            host,
            crate::ProgramEvidenceSnapshot::NoProgram,
        )
    }

    fn complete_digest() -> DeterministicDigest {
        let cycle = 42;
        let mut gate = FixedCycleDigestGate::new(cycle);
        gate.capture(cycle, ArtifactKind::Framebuffer, b"fb")
            .unwrap();
        gate.capture(cycle, ArtifactKind::Audio, b"audio").unwrap();
        gate.capture(
            cycle,
            ArtifactKind::Memory,
            &vec![0; crate::DEFAULT_RDRAM_SIZE],
        )
        .unwrap();
        gate.capture_device_snapshot(
            snapshot(cycle),
            executor_snapshot(),
            host_snapshot(),
            crate::ProgramEvidenceSnapshot::NoProgram,
        )
        .unwrap();
        gate.capture_timing_trace(cycle, &[]).unwrap();
        gate.finish().unwrap()
    }

    fn native_destination_event(
        cycle: u64,
        section_index: u32,
        function_offset: u32,
        link_vram: u32,
    ) -> fn64_abi::NativeExecutionDestinationEvent {
        fn64_abi::NativeExecutionDestinationEvent {
            at: Cycles::new(cycle),
            destination: fn64_abi::NativeExecutionDestination {
                section_index,
                function_offset,
                link_vram,
            },
        }
    }

    #[cfg(feature = "recomp-rs")]
    fn typed_block_program() -> crate::ProgramEvidenceSnapshot {
        use fn64_abi::recompiled::RecompiledProgramEvidenceSnapshot;
        use fn64_recomp_rs::{
            BankId, BlockProgramEvidenceSnapshot, CodeBankEvidenceSnapshot,
            CodeSpanEvidenceSnapshot, GuestPc, ProgramArtifactIdentity,
            ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
        };
        let identity = |byte| ProgramArtifactIdentity::new([byte; 32]);
        crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
            program: BlockProgramEvidenceSnapshot {
                identity: ProgramIdentityEvidenceSnapshot {
                    identity: identity(0x31),
                    source: ProgramIdentitySource::CanonicalBlockProgramSha256,
                },
                banks: vec![CodeBankEvidenceSnapshot {
                    id: BankId::new(0x32),
                    runner_artifact_identity: identity(0x33),
                    spans: vec![CodeSpanEvidenceSnapshot {
                        vram_start: GuestPc::new(0x8000_1000),
                        words: vec![0],
                    }],
                }],
            },
            dispatch_artifact_identity: identity(0x34),
            instruction_budget: 100,
            executable_regions: Vec::new(),
            pending_executable_writes: Vec::new(),
        })
    }

    #[test]
    fn execution_destination_evidence_binds_order_and_collision_safe_unique_counts() {
        let program = crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
            crate::NativeProgramArtifactIdentity::new([0x21; 32]),
        );
        let first = native_destination_event(3, 1, 0x10, 0x8000_1010);
        let collision = native_destination_event(4, 2, 0x20, 0x8000_1010);
        let repeated = native_destination_event(5, 1, 0x10, 0x8000_1010);
        let evidence = capture_execution_destinations(
            &program,
            crate::FrozenExecutionDestinations {
                native: vec![first, collision, repeated],
                #[cfg(feature = "recomp-rs")]
                function: Vec::new(),
                #[cfg(feature = "recomp-rs")]
                block: Vec::new(),
            },
            5,
        )
        .unwrap();
        assert_eq!(evidence.total_observations, 3);
        assert_eq!(evidence.unique_destinations, 2);
        assert_eq!(evidence.unique[0].observations, 2);
        assert_eq!(evidence.unique[1].observations, 1);
        evidence.verify_integrity().unwrap();

        let mut reordered = evidence.clone();
        reordered.ordered.swap(0, 1);
        assert!(matches!(
            reordered.verify_integrity(),
            Err(GateError::ExecutionDestinationIntegrityMismatch)
        ));
        let reordered_canonical =
            ExecutionDestinationEvidence::from_ordered(evidence.source.clone(), reordered.ordered)
                .unwrap();
        assert_ne!(evidence.ordered_sha256, reordered_canonical.ordered_sha256);
        assert_eq!(evidence.unique_sha256, reordered_canonical.unique_sha256);

        let geometry = observations();
        let first_report = ReleaseGateReport::new_with_environment(
            "destination-order",
            b"input",
            complete_digest(),
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations: geometry.clone(),
                environment: test_release_environment(&geometry),
                execution_destinations: evidence.clone(),
                rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
            },
            Vec::new(),
        )
        .unwrap();
        let second_report = ReleaseGateReport::new_with_environment(
            "destination-order",
            b"input",
            complete_digest(),
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations: geometry.clone(),
                environment: test_release_environment(&geometry),
                execution_destinations: reordered_canonical,
                rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
            },
            Vec::new(),
        )
        .unwrap();
        assert_ne!(first_report.report_sha256, second_report.report_sha256);

        let mut mutated = evidence;
        mutated.unique[0].observations += 1;
        assert!(matches!(
            mutated.verify_integrity(),
            Err(GateError::ExecutionDestinationIntegrityMismatch)
        ));
    }

    #[test]
    fn execution_destination_capture_rejects_future_and_cross_lane_evidence() {
        let native = crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
            crate::NativeProgramArtifactIdentity::new([0x22; 32]),
        );
        assert!(matches!(
            capture_execution_destinations(
                &native,
                crate::FrozenExecutionDestinations {
                    native: vec![native_destination_event(6, 1, 0, 0x8000_1000)],
                    #[cfg(feature = "recomp-rs")]
                    function: Vec::new(),
                    #[cfg(feature = "recomp-rs")]
                    block: Vec::new(),
                },
                5,
            ),
            Err(GateError::FutureExecutionDestinationEvent {
                gate_cycle: 5,
                event_cycle: 6,
            })
        ));
        assert!(matches!(
            capture_execution_destinations(
                &crate::ProgramEvidenceSnapshot::NoProgram,
                crate::FrozenExecutionDestinations {
                    native: vec![native_destination_event(0, 1, 0, 0x8000_1000)],
                    #[cfg(feature = "recomp-rs")]
                    function: Vec::new(),
                    #[cfg(feature = "recomp-rs")]
                    block: Vec::new(),
                },
                0,
            ),
            Err(GateError::ExecutionDestinationSourceMismatch(_))
        ));
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn typed_block_destination_requires_runner_identity_and_rejects_native_mix() {
        use fn64_recomp_rs::{
            BankId, ExecutionDestinationObservation, ExecutionKey, GuestPc, ProgramArtifactIdentity,
        };
        let destination = ExecutionKey::new(BankId::new(0x32), GuestPc::new(0x8000_1000));
        let program = typed_block_program();
        assert!(matches!(
            capture_execution_destinations(
                &program,
                crate::FrozenExecutionDestinations {
                    native: Vec::new(),
                    function: Vec::new(),
                    block: vec![ExecutionDestinationObservation {
                        destination,
                        runner_artifact_identity: None,
                    }],
                },
                0,
            ),
            Err(GateError::UnidentifiedBlockRunnerArtifact { .. })
        ));
        assert!(matches!(
            capture_execution_destinations(
                &program,
                crate::FrozenExecutionDestinations {
                    native: vec![native_destination_event(0, 1, 0, 0x8000_1000)],
                    function: Vec::new(),
                    block: vec![ExecutionDestinationObservation {
                        destination,
                        runner_artifact_identity: Some(ProgramArtifactIdentity::new([0x33; 32])),
                    }],
                },
                0,
            ),
            Err(GateError::ExecutionDestinationSourceMismatch(_))
        ));

        let evidence = capture_execution_destinations(
            &program,
            crate::FrozenExecutionDestinations {
                native: Vec::new(),
                function: Vec::new(),
                block: vec![ExecutionDestinationObservation {
                    destination,
                    runner_artifact_identity: Some(ProgramArtifactIdentity::new([0x33; 32])),
                }],
            },
            0,
        )
        .unwrap();
        assert!(matches!(
            evidence.source,
            ExecutionDestinationSource::TypedBlockProgram { .. }
        ));
        evidence.verify_integrity().unwrap();
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn typed_function_destination_binds_identity_cycle_symbol_order_and_counts() {
        use fn64_abi::recompiled::{
            FunctionExecutionDestinationObservation, RecompiledProgramEvidenceSnapshot,
        };
        use fn64_recomp_rs::{
            ProgramArtifactIdentity, ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
            TranslatedFunctionIdentity,
        };

        let identity = ProgramArtifactIdentity::new([0x44; 32]);
        let program = crate::ProgramEvidenceSnapshot::TypedRust(
            RecompiledProgramEvidenceSnapshot::Function {
                identity: ProgramIdentityEvidenceSnapshot {
                    identity,
                    source: ProgramIdentitySource::CallerSupplied,
                },
            },
        );
        let event = |cycle, vram, symbol| FunctionExecutionDestinationObservation {
            at: fn64_runtime::Cycles::new(cycle),
            artifact_identity: identity,
            function: TranslatedFunctionIdentity::new(vram, symbol),
        };
        let frozen = |function| crate::FrozenExecutionDestinations {
            native: Vec::new(),
            function,
            block: Vec::new(),
        };

        let evidence = capture_execution_destinations(
            &program,
            frozen(vec![
                event(3, 0x8000_1000, "entry"),
                event(4, 0x8000_1000, "alias"),
                event(5, 0x8000_1000, "entry"),
            ]),
            5,
        )
        .unwrap();
        assert_eq!(evidence.total_observations, 3);
        assert_eq!(evidence.unique_destinations, 2);
        assert!(matches!(
            evidence.source,
            ExecutionDestinationSource::TypedObservedFunctionProgram { .. }
        ));
        assert_eq!(evidence.unique[0].observations, 1);
        assert_eq!(evidence.unique[1].observations, 2);
        evidence.verify_integrity().unwrap();
        let json = serde_json::to_value(&evidence).unwrap();
        assert_eq!(
            json["source"],
            serde_json::json!({
                "kind": "typed_observed_function_program",
                "artifact_sha256": "44".repeat(32),
            })
        );
        assert_eq!(
            json["ordered"][0],
            serde_json::json!({
                "guest_cycle": 3,
                "destination": {
                    "lane": "typed_function",
                    "vram": 0x8000_1000_u32,
                    "symbol": "entry",
                },
            })
        );

        let reordered = ExecutionDestinationEvidence::from_ordered(
            evidence.source.clone(),
            vec![
                evidence.ordered[1].clone(),
                evidence.ordered[0].clone(),
                evidence.ordered[2].clone(),
            ],
        )
        .unwrap();
        assert_ne!(evidence.ordered_sha256, reordered.ordered_sha256);
        assert_eq!(evidence.unique_sha256, reordered.unique_sha256);
        let mut tampered = evidence.clone();
        if let ReleaseExecutionDestination::TypedFunction { symbol, .. } =
            &mut tampered.ordered[0].destination
        {
            *symbol = "tampered".to_owned();
        }
        assert!(matches!(
            tampered.verify_integrity(),
            Err(GateError::ExecutionDestinationIntegrityMismatch)
        ));

        assert!(matches!(
            capture_execution_destinations(
                &program,
                frozen(vec![event(6, 0x8000_1000, "entry")]),
                5,
            ),
            Err(GateError::FutureExecutionDestinationEvent { .. })
        ));
        let future_retained = ExecutionDestinationEvidence::from_ordered(
            evidence.source.clone(),
            vec![ExecutionDestinationEventEvidence {
                guest_cycle: Some(6),
                destination: ReleaseExecutionDestination::TypedFunction {
                    vram: 0x8000_1000,
                    symbol: "entry".to_owned(),
                },
            }],
        )
        .unwrap();
        assert!(matches!(
            validate_execution_destination_cycles(5, &future_retained),
            Err(GateError::FutureExecutionDestinationEvent { .. })
        ));
        let mut wrong_identity = event(5, 0x8000_1000, "entry");
        wrong_identity.artifact_identity = ProgramArtifactIdentity::new([0x45; 32]);
        assert!(matches!(
            capture_execution_destinations(&program, frozen(vec![wrong_identity]), 5),
            Err(GateError::FunctionDestinationArtifactMismatch { .. })
        ));
        assert!(matches!(
            capture_execution_destinations(&program, frozen(Vec::new()), 5),
            Err(GateError::EmptyExecutionDestinationEvidence(
                "typed_observed_function_program"
            ))
        ));
        assert!(matches!(
            capture_execution_destinations(
                &program,
                crate::FrozenExecutionDestinations {
                    native: vec![native_destination_event(5, 1, 0, 0x8000_1000)],
                    function: vec![event(5, 0x8000_1000, "entry")],
                    block: Vec::new(),
                },
                5,
            ),
            Err(GateError::ExecutionDestinationSourceMismatch(_))
        ));
    }

    unsafe extern "C" fn late_native_destination(
        _rdram: *mut u8,
        _ctx: *mut fn64_abi::RecompContext,
    ) {
    }

    #[test]
    fn live_gate_rejects_native_execution_destination_before_arm() {
        fn64_abi::load_rom(Vec::new());
        unsafe {
            fn64_abi::register_section(
                0x0010_0000,
                0x8000_1000,
                4,
                &[(0, 4, late_native_destination)],
            );
        }
        fn64_abi::fn64_c_recompiled_function_enter(late_native_destination);
        assert!(matches!(
            LiveReleaseGate::new(0).arm(),
            Err(GateError::LiveGateArmedLate {
                native_execution_destination_events: 1,
                ..
            })
        ));
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn live_gate_rejects_function_execution_destination_before_arm() {
        fn lookup(_vram: u32) -> fn64_recomp_rs::RecompFunc {
            fn body(
                _ctx: &mut fn64_recomp_rs::RecompContext,
                _rdram: &mut fn64_recomp_rs::Rdram<'_>,
            ) {
            }
            body
        }

        std::thread::spawn(|| {
            fn64_abi::load_rom(Vec::new());
            fn64_abi::recompiled::set_entry_lookup_with_execution_observation(
                lookup,
                0x100,
                fn64_recomp_rs::ProgramArtifactIdentity::new([0x5c; 32]),
                fn64_recomp_rs::FUNCTION_ENTRY_OBSERVATION_SCHEMA,
            );
            fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(
                0x8000_1000,
                "entry",
            ));
            assert!(matches!(
                LiveReleaseGate::new(0).arm(),
                Err(GateError::LiveGateArmedLate {
                    function_execution_destination_events: 1,
                    ..
                })
            ));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn schema_v20_fixed_cycle_digest_is_stable_and_complete() {
        assert_eq!(complete_digest(), complete_digest());
        assert_eq!(complete_digest().artifacts.len(), 5);
        assert_eq!(
            complete_digest().root_sha256,
            "3001732227bad47cceeefe18ae478d58bbf321ba266f09474324cea269b6d423"
        );
    }

    #[test]
    fn schema_v20_report_wire_binds_rom_identity_class_and_tv_authorities() {
        let input = test_rom(b'E');
        let geometry = observations();
        let rom =
            ReleaseRomEvidence::from_bytes(&input, ReleaseRomClass::RetailCartridge, TvType::Ntsc)
                .unwrap();
        let report = ReleaseGateReport::new_with_environment(
            "rom-wire",
            &input,
            complete_digest(),
            ReleaseBoundaryReportEvidence {
                rom: Some(rom),
                observations: geometry.clone(),
                environment: test_release_environment(&geometry),
                execution_destinations: ExecutionDestinationEvidence::no_program(),
                rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
            },
            Vec::new(),
        )
        .unwrap();
        report.verify_integrity().unwrap();

        let baseline = report.report_sha256.clone();
        let mut changed_class = report.clone();
        changed_class.rom.as_mut().unwrap().class = ReleaseRomClass::PublicHomebrew;
        assert_ne!(
            sha256_hex(&encode_report_evidence(&changed_class).unwrap()),
            baseline
        );

        let mut changed_order = report.clone();
        changed_order.rom.as_mut().unwrap().source_byte_order = ReleaseRomByteOrder::V64;
        assert_ne!(
            sha256_hex(&encode_report_evidence(&changed_order).unwrap()),
            baseline
        );

        let mut changed_identity = report.clone();
        changed_identity.rom.as_mut().unwrap().canonical_sha256 = "ab".repeat(32);
        assert_ne!(
            sha256_hex(&encode_report_evidence(&changed_identity).unwrap()),
            baseline
        );

        let mut changed_destination = report.clone();
        changed_destination.rom.as_mut().unwrap().destination_code = b'P';
        assert!(matches!(
            changed_destination.verify_integrity(),
            Err(GateError::RomRegionDecodeMismatch { .. })
        ));

        let mut changed_region = report.clone();
        changed_region.rom.as_mut().unwrap().decoded_tv_region = ReleaseTvRegion::Pal;
        assert!(matches!(
            changed_region.verify_integrity(),
            Err(GateError::RomRegionDecodeMismatch { .. })
        ));

        let mut changed_renderer_tv = report.clone();
        let ReleaseRendererEvidence::Reference { tv_type, .. } =
            &mut changed_renderer_tv.environment.renderer
        else {
            unreachable!()
        };
        *tv_type = ReleaseTvStandard::Mpal;
        changed_renderer_tv.report_sha256 =
            sha256_hex(&encode_report_evidence(&changed_renderer_tv).unwrap());
        assert!(matches!(
            changed_renderer_tv.verify_integrity(),
            Err(GateError::RomTvTypeMismatch {
                authority: "retained renderer create-time configuration",
                ..
            })
        ));

        let mut mismatched_input = input;
        mismatched_input[0x100] ^= 1;
        assert!(matches!(
            ReleaseGateReport::new_with_environment(
                "rom-wire",
                &mismatched_input,
                report.digest,
                ReleaseBoundaryReportEvidence {
                    rom: report.rom,
                    observations: report.observations,
                    environment: report.environment,
                    execution_destinations: report.execution_destinations,
                    rsp_rdp: report.rsp_rdp,
                },
                Vec::new(),
            ),
            Err(GateError::RomInputEvidenceMismatch)
        ));
    }

    #[test]
    fn device_evidence_wire_binds_every_future_state_family() {
        use fn64_runtime::{
            PendingAiSnapshot, PendingEepromWriteSnapshot, PendingPiSnapshot, PendingSiSnapshot,
            PendingSpDmaSnapshot, ScheduledDeviceEventKind, ScheduledDeviceEventSnapshot,
            SpDmaRequest,
        };

        let baseline = snapshot(42);
        let baseline_sha = sha256_hex(&encode_test_device(
            baseline.clone(),
            peripherals_snapshot(),
        ));
        let mut cases = Vec::new();
        macro_rules! changed {
            ($name:literal, $body:expr) => {{
                let mut value = baseline.clone();
                $body(&mut value);
                cases.push(($name, value));
            }};
        }

        changed!(
            "guest register projection",
            |value: &mut DeviceEvidenceSnapshot| { value.guest.pi_cart_addr ^= 1 }
        );
        changed!("pi domain timing", |value: &mut DeviceEvidenceSnapshot| {
            value.guest.pi_domain2.release ^= 1
        });
        changed!("pi timing policy", |value: &mut DeviceEvidenceSnapshot| {
            value.pi_timing_policy.push(1)
        });
        changed!("pending PI", |value: &mut DeviceEvidenceSnapshot| {
            value.pending_pi = Some(PendingPiSnapshot {
                token: 1,
                request: PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: RdramAddr::from_offset(4),
                    cart_addr: 8,
                    len: 12,
                },
            })
        });
        changed!("current AI", |value: &mut DeviceEvidenceSnapshot| {
            value.current_ai = Some(PendingAiSnapshot {
                token: 2,
                request: AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(16),
                    len: 32,
                    sample_rate_hz: 32_000,
                },
                started_at: Cycles::new(40),
                deadline: Cycles::new(80),
            })
        });
        changed!("queued AI", |value: &mut DeviceEvidenceSnapshot| {
            value.queued_ai = Some(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(48),
                len: 64,
                sample_rate_hz: 44_100,
            })
        });
        changed!("pending SI", |value: &mut DeviceEvidenceSnapshot| {
            value.pending_si = Some(PendingSiSnapshot {
                token: 3,
                request: SiDmaRequest {
                    kind: SiDmaKind::PifToDram,
                    dram_addr: RdramAddr::from_offset(80),
                },
            })
        });
        changed!("SI error", |value: &mut DeviceEvidenceSnapshot| {
            value.si_dma_error = true
        });
        changed!("SI policy", |value: &mut DeviceEvidenceSnapshot| {
            value.si_latency = Cycles::new(2)
        });
        changed!("PIF RAM", |value: &mut DeviceEvidenceSnapshot| {
            value.pif_ram[63] = 1
        });
        changed!("RSP DMEM", |value: &mut DeviceEvidenceSnapshot| {
            value.rsp_dmem[4095] = 1
        });
        changed!("RSP IMEM", |value: &mut DeviceEvidenceSnapshot| {
            value.rsp_imem[4095] = 1
        });
        changed!("SP registers", |value: &mut DeviceEvidenceSnapshot| {
            value.sp_pc = 4
        });
        changed!("SP semaphore", |value: &mut DeviceEvidenceSnapshot| {
            value.sp_semaphore = true
        });
        let sp_request = SpDmaRequest {
            direction: SpDmaDirection::RdramToRsp,
            mem_addr: RspMemAddr::from_register(0x20),
            dram_addr: RdramAddr::from_offset(0x100),
            encoded_len: 7,
        };
        changed!("active SP DMA", |value: &mut DeviceEvidenceSnapshot| {
            value.active_sp_dma = Some(PendingSpDmaSnapshot {
                token: 4,
                request: sp_request,
            })
        });
        changed!("queued SP DMA", |value: &mut DeviceEvidenceSnapshot| {
            value.queued_sp_dma = Some(sp_request)
        });
        changed!("SP DMA policy", |value: &mut DeviceEvidenceSnapshot| {
            value.sp_dma_setup_cycles = Cycles::new(9)
        });
        changed!("VI registers", |value: &mut DeviceEvidenceSnapshot| {
            value.vi_registers[13] = 1
        });
        changed!("VI epoch", |value: &mut DeviceEvidenceSnapshot| {
            value.vi_epoch = Cycles::new(1)
        });
        changed!(
            "pending RCP tokens",
            |value: &mut DeviceEvidenceSnapshot| { value.pending_dp_token = Some(5) }
        );
        changed!(
            "scheduled event order",
            |value: &mut DeviceEvidenceSnapshot| {
                value.scheduled_events.push(ScheduledDeviceEventSnapshot {
                    at: Cycles::new(43),
                    sequence: 5,
                    token: 5,
                    kind: ScheduledDeviceEventKind::Dp,
                })
            }
        );
        changed!(
            "next event sequence",
            |value: &mut DeviceEvidenceSnapshot| { value.next_event_sequence = 6 }
        );
        changed!("save bytes", |value: &mut DeviceEvidenceSnapshot| {
            value.save_bytes = Some(vec![0xff; 512])
        });
        changed!("pending EEPROM", |value: &mut DeviceEvidenceSnapshot| {
            value.pending_eeprom_write = Some(PendingEepromWriteSnapshot {
                offset: 8,
                data: [0x5a; 8],
                ready_at: Cycles::new(100),
            })
        });

        for (name, value) in cases {
            assert_ne!(
                sha256_hex(&encode_test_device(value, peripherals_snapshot())),
                baseline_sha,
                "device evidence omitted {name}"
            );
        }
    }

    #[test]
    fn device_evidence_wire_binds_executor_peripheral_and_manager_state() {
        use fn64_runtime::{
            ContInput, ControllerPak, GameBoyCartridgeEvidenceSnapshot,
            GameBoyMapperEvidenceSnapshot, PfsKey, PfsNoteEvidenceSnapshot, PortState,
            RetraceScheduleEvidenceSnapshot, TransferPakEvidenceSnapshot, VoiceData,
            VoiceEvidenceSnapshot,
        };

        let device = snapshot(42);
        let baseline = peripherals_snapshot();
        let baseline_sha = sha256_hex(&encode_test_device(device.clone(), baseline.clone()));
        let mut cases = Vec::new();
        macro_rules! changed {
            ($name:literal, $body:expr) => {{
                let mut value = baseline.clone();
                $body(&mut value);
                cases.push(($name, value));
            }};
        }

        changed!(
            "controller identity",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.pif.ports[3] = PortState::StandardControllerNoPak;
            }
        );
        changed!(
            "controller input",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.pif.inputs[0] = ContInput {
                    button: 0x8000,
                    stick_x: -12,
                    stick_y: 34,
                };
            }
        );
        changed!(
            "rumble state",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.pif.rumble_on[0] = true;
            }
        );
        changed!(
            "Controller Pak raw image",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                let mut pak = ControllerPak::new().evidence_snapshot();
                pak.raw[31] = 0x5a;
                value.peripherals.controller_paks[2] = Some(pak);
            }
        );
        changed!(
            "Controller Pak bank count",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                let mut pak = ControllerPak::new().evidence_snapshot();
                pak.bank_count = 2;
                value.peripherals.controller_paks[2] = Some(pak);
            }
        );
        changed!(
            "Controller Pak active bank",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                let mut pak = ControllerPak::new().evidence_snapshot();
                pak.active_bank = 1;
                value.peripherals.controller_paks[2] = Some(pak);
            }
        );
        changed!(
            "Controller Pak semantic notes",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                let mut pak = ControllerPak::new().evidence_snapshot();
                pak.notes[0] = Some(PfsNoteEvidenceSnapshot {
                    key: PfsKey {
                        company_code: 0x1234,
                        game_code: 0x5566_7788,
                        game_name: [0x41; 16],
                        ext_name: [0x42; 4],
                    },
                    pages: vec![5, 6],
                });
                value.peripherals.controller_paks[2] = Some(pak);
            }
        );
        changed!(
            "Transfer Pak register state",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.transfer_paks[1] = Some(TransferPakEvidenceSnapshot {
                    now: Cycles::new(42),
                    enabled: true,
                    transfer_bank: 2,
                    access_mode: 1,
                    cartridge: None,
                    cartridge_pulled: true,
                    reset_detected: true,
                });
            }
        );
        changed!(
            "Transfer Pak cartridge and mapper",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.transfer_paks[1] = Some(TransferPakEvidenceSnapshot {
                    now: Cycles::new(42),
                    enabled: false,
                    transfer_bank: 0,
                    access_mode: 0,
                    cartridge: Some(GameBoyCartridgeEvidenceSnapshot {
                        rom: vec![0x11; 0x150],
                        ram: vec![0x22; 32],
                        mapper: GameBoyMapperEvidenceSnapshot::Mbc3 {
                            timer_present: true,
                            ram_enabled: true,
                            rom_bank: 3,
                            select: 0x08,
                            latch_armed: true,
                            rtc: [1, 2, 3, 4, 5],
                            latched_rtc: [6, 7, 8, 9, 10],
                            subsecond_cycles: 99,
                        },
                    }),
                    cartridge_pulled: false,
                    reset_detected: false,
                });
            }
        );
        changed!(
            "VRU dictionary and result",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.voice_units[0] = Some(VoiceEvidenceSnapshot {
                    initialized: true,
                    raw_init_step: 0,
                    expected_words: Some(1),
                    words: vec![b"test".to_vec()],
                    mask: vec![1],
                    analog_gain: 1,
                    digital_gain: 7,
                    status: 7,
                    pending_result: Some(VoiceData {
                        answer_num: 1,
                        answer: [2, 3, 4, 5, 6],
                        ..VoiceData::default()
                    }),
                });
            }
        );
        changed!(
            "VRU raw initialization sequence position",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.voice_units[0] = Some(VoiceEvidenceSnapshot {
                    initialized: false,
                    raw_init_step: 3,
                    expected_words: None,
                    words: Vec::new(),
                    mask: Vec::new(),
                    analog_gain: 0,
                    digital_gain: 0,
                    status: 0,
                    pending_result: None,
                });
            }
        );
        changed!(
            "high-level VI manager",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.vi.next_mode_ptr = Some(0x8000_1000);
                value.peripherals.vi.next_fade = PendingViFade::Factor(0x155);
            }
        );
        changed!(
            "compatibility retrace schedule",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.peripherals.retrace = Some(RetraceScheduleEvidenceSnapshot {
                    interval: 100,
                    next_due: 200,
                });
            }
        );
        changed!(
            "PI manager completion queue",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value
                    .pending_pi_completions
                    .push(fn64_abi::PendingPiCompletionEvidenceSnapshot {
                        request: PiDmaRequest {
                            direction: DmaDirection::ToRdram,
                            dram_addr: RdramAddr::from_offset(4),
                            cart_addr: 8,
                            len: 12,
                        },
                        rdram_len: 8 * 1024 * 1024,
                        ret_queue: Some(RdramAddr::from_offset(16)),
                        ret_mesg: 20,
                    });
            }
        );
        changed!(
            "SI manager completion metadata",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.pending_si_completion = Some(fn64_abi::PendingSiCompletionEvidenceSnapshot {
                    request: SiDmaRequest {
                        kind: SiDmaKind::ControllerRead,
                        dram_addr: RdramAddr::from_offset(24),
                    },
                    rdram_len: 8 * 1024 * 1024,
                });
            }
        );
        changed!(
            "ABI VI mode and scale latches",
            |value: &mut fn64_abi::RuntimePeripheralEvidenceSnapshot| {
                value.vi.pending_mode = Some(fn64_abi::PendingViModeEvidenceSnapshot {
                    registers: [1; 14],
                    fields: [[2; 5], [3; 5]],
                });
                value.vi.pending_control = Some(4);
                value.vi.pending_x_scale_bits = Some(0.5f32.to_bits());
                value.vi.active_y_scale_bits = 0.75f32.to_bits();
            }
        );

        for (name, value) in cases {
            assert_ne!(
                sha256_hex(&encode_test_device(device.clone(), value)),
                baseline_sha,
                "device evidence omitted {name}"
            );
        }
    }

    #[test]
    fn device_state_v7_wire_binds_executor_and_abi_host_families() {
        use fn64_runtime::{
            EventRegistrationEvidenceSnapshot, ExecutorQueueEvidenceSnapshot,
            ExecutorRunningEvidenceSnapshot, MesgQueueEvidenceSnapshot,
            PendingResumeEvidenceSnapshot, RdramRegistrationEvidenceSnapshot,
            SectionEvidenceSnapshot, SectionLoadEvidenceSnapshot, StaticMirrorEvidenceSnapshot,
            StaticStorageEndEvidenceSnapshot, ThreadEvidenceSnapshot,
        };

        let device = snapshot(42);
        let executor = executor_snapshot();
        let host = host_snapshot();
        let baseline = sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host.clone(),
            crate::ProgramEvidenceSnapshot::NoProgram,
        ));

        macro_rules! changed_executor {
            ($name:literal, $body:expr) => {{
                let mut value = executor.clone();
                $body(&mut value);
                assert_ne!(
                    sha256_hex(&encode_device_snapshot(
                        device.clone(),
                        value,
                        host.clone(),
                        crate::ProgramEvidenceSnapshot::NoProgram,
                    )),
                    baseline,
                    "device-state-v7 evidence omitted executor family {}",
                    $name
                );
            }};
        }
        changed_executor!(
            "RDRAM registration",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.rdram = RdramRegistrationEvidenceSnapshot::Present { len: 0x80 };
            }
        );
        changed_executor!(
            "threads",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.threads.push(ThreadEvidenceSnapshot {
                    id: 7,
                    priority: -2,
                    state: fn64_runtime::ThreadState::Dead,
                    started: true,
                });
            }
        );
        changed_executor!(
            "run queue",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.run_queue.push(7);
            }
        );
        changed_executor!(
            "pending resume",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.pending_resumes.push(PendingResumeEvidenceSnapshot {
                    thread: 7,
                    resume: fn64_runtime::Resume::Delivered(0x1234),
                });
            }
        );
        changed_executor!(
            "message queues",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.queues.push(ExecutorQueueEvidenceSnapshot {
                    address: RdramAddr::from_offset(0x100),
                    queue: MesgQueueEvidenceSnapshot {
                        capacity: 2,
                        first: 1,
                        messages: vec![0x55],
                        blocked_receivers: vec![7],
                        blocked_senders: Vec::new(),
                    },
                });
            }
        );
        changed_executor!(
            "timer wheel",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.timers.next_id = 9;
            }
        );
        changed_executor!(
            "event table",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.event_table.push(EventRegistrationEvidenceSnapshot {
                    event: 7,
                    queue_addr: RdramAddr::from_offset(0x100),
                    msg: 0x77,
                });
            }
        );
        changed_executor!(
            "running owner",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.running = ExecutorRunningEvidenceSnapshot::Active(7);
            }
        );
        changed_executor!(
            "virtual and CP0 clocks",
            |value: &mut fn64_runtime::ExecutorControlEvidenceSnapshot| {
                value.sim_time = 42;
                value.cp0_count = 21;
                value.cp0_count_phase = 1;
                value.cp0_compare = 22;
                value.cp0_timer_pending = true;
            }
        );

        macro_rules! changed_host {
            ($name:literal, $body:expr) => {{
                let mut value = host.clone();
                $body(&mut value);
                assert_ne!(
                    sha256_hex(&encode_device_snapshot(
                        device.clone(),
                        executor.clone(),
                        value,
                        crate::ProgramEvidenceSnapshot::NoProgram,
                    )),
                    baseline,
                    "device-state-v7 evidence omitted ABI HostState family {}",
                    $name
                );
            }};
        }
        changed_host!(
            "Flash sequencer",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.flash.status = 0x80;
            }
        );
        changed_host!(
            "section registry",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.sections.sections.push(SectionEvidenceSnapshot {
                    rom_addr: 1,
                    ram_addr: 2,
                    size: 4,
                    funcs: Vec::new(),
                });
                value.sections.loaded_sections.push(0);
                value
                    .sections
                    .runtime_loads
                    .push(SectionLoadEvidenceSnapshot {
                        section: 0,
                        load_vram: 3,
                    });
                value.sections.static_mirror = Some(StaticMirrorEvidenceSnapshot {
                    section: 0,
                    next_rom: 2,
                    next_static_off: 1,
                });
                value
                    .sections
                    .static_storage_ends
                    .push(StaticStorageEndEvidenceSnapshot { section: 0, end: 4 });
            }
        );
        changed_host!(
            "rspboot images",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value
                    .rsp_boot_images
                    .push(fn64_abi::RspBootImageEvidenceSnapshot {
                        rdram_offset: 0x100,
                        bytes: vec![1, 2, 3],
                    });
            }
        );
        changed_host!(
            "loaded RSP task token",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.loaded_rsp_task = Some(fn64_abi::LoadedRspTaskEvidenceSnapshot {
                    task_offset: 0x200,
                    header: fn64_runtime::OsTaskHeader {
                        task_type: fn64_runtime::M_GFXTASK,
                        flags: fn64_runtime::OS_TASK_YIELDED,
                        ucode_boot: 0x1000,
                        ucode_boot_size: 0x80,
                        ucode: 0x2000,
                        ucode_size: 0x1000,
                        ucode_data: 0x3000,
                        ucode_data_size: 0x40,
                        dram_stack: 0x4000,
                        dram_stack_size: 0x20,
                        output_buff: 0x5000,
                        output_buff_size: 0x5004,
                        data_ptr: 0x6000,
                        data_size: 0x18,
                        yield_data_ptr: 0x7000,
                        yield_data_size: 0x80,
                    },
                    resumed_data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                        rdram_offset: 0x3000,
                        byte_len: 0x40,
                        sha256: [0x31; 32],
                    }),
                });
            }
        );
        changed_host!(
            "yielded RSP task lineage",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value
                    .rsp_task_lineages
                    .push(fn64_abi::RspTaskLineageEvidenceSnapshot {
                        task_offset: 0x200,
                        original_header: fn64_runtime::OsTaskHeader {
                            task_type: fn64_runtime::M_GFXTASK,
                            ucode_data: 0x3000,
                            ucode_data_size: 0x40,
                            yield_data_ptr: 0x7000,
                            yield_data_size: 0x80,
                            ..fn64_runtime::OsTaskHeader::default()
                        },
                        data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                            rdram_offset: 0x3000,
                            byte_len: 0x40,
                            sha256: [0x32; 32],
                        }),
                        phase: fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized,
                    });
            }
        );
        changed_host!(
            "installed ROM identity",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.rom_installed = true;
                value.installed_rom = Some(fn64_abi::InstalledRomEvidenceSnapshot {
                    byte_len: 3,
                    sha256: [0x5a; 32],
                });
            }
        );
        changed_host!(
            "cartridge save configuration",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;
            }
        );
        changed_host!(
            "PI handles and Leo configuration",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.cart_rom_handle_vram = Some(0x8000_1000);
                value.flash_handle_vram = Some(0x8000_2000);
                value.leo_disk = Some(fn64_abi::LeoDiskConfig {
                    handle_vram: 0x8000_3000,
                    latency: 1,
                    page_size: 2,
                    release: 3,
                    pulse_width: 4,
                });
            }
        );
        changed_host!(
            "thread and timer handle maps",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value
                    .thread_handles
                    .push(fn64_abi::ThreadHandleEvidenceSnapshot {
                        osthread_offset: 0x100,
                        executor_thread_id: 7,
                    });
                value
                    .thread_guest_ids
                    .push(fn64_abi::ThreadGuestIdEvidenceSnapshot {
                        executor_thread_id: 7,
                        guest_os_id: 8,
                    });
                value
                    .timer_handles
                    .push(fn64_abi::TimerHandleEvidenceSnapshot {
                        ostimer_offset: 0x200,
                        timer_id: 9,
                    });
            }
        );
        changed_host!(
            "synthetic ID and RDRAM registration",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.next_synthetic_thread_id ^= 1;
                value.registered_rdram.present = true;
                value.registered_rdram.byte_len = 0x80;
            }
        );
        changed_host!(
            "debug hardware",
            |value: &mut fn64_abi::AbiHostEvidenceSnapshot| {
                value.debug_hardware = fn64_abi::DebugHardware::Isv;
            }
        );
    }

    #[test]
    fn device_state_v7_wire_distinguishes_rsp_task_lineage_phases() {
        let device = snapshot(42);
        let executor = executor_snapshot();
        let digest = |phase| {
            let mut host = host_snapshot();
            host.rsp_task_lineages
                .push(fn64_abi::RspTaskLineageEvidenceSnapshot {
                    task_offset: 0x200,
                    original_header: fn64_runtime::OsTaskHeader {
                        task_type: fn64_runtime::M_GFXTASK,
                        ucode_data: 0x3000,
                        ucode_data_size: 0x40,
                        ..fn64_runtime::OsTaskHeader::default()
                    },
                    data_identity: Some(fn64_abi::RspTaskDataIdentityEvidenceSnapshot {
                        rdram_offset: 0x3000,
                        byte_len: 0x40,
                        sha256: [0x32; 32],
                    }),
                    phase,
                });
            sha256_hex(&encode_device_snapshot(
                device.clone(),
                executor.clone(),
                host,
                crate::ProgramEvidenceSnapshot::NoProgram,
            ))
        };
        let distinct: std::collections::BTreeSet<_> = [
            digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::Running),
            digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeAuthorized),
            digest(fn64_abi::RspTaskLineagePhaseEvidenceSnapshot::ResumeLoaded),
        ]
        .into_iter()
        .collect();
        assert_eq!(distinct.len(), 3);
    }

    #[test]
    fn device_state_v7_wire_distinguishes_native_program_classes_and_identity() {
        let device = snapshot(42);
        let executor = executor_snapshot();
        let host = host_snapshot();
        let digest = |program| {
            sha256_hex(&encode_device_snapshot(
                device.clone(),
                executor.clone(),
                host.clone(),
                program,
            ))
        };
        let no_program = digest(crate::ProgramEvidenceSnapshot::NoProgram);
        let unidentified = digest(crate::ProgramEvidenceSnapshot::UnidentifiedNativeProgram);
        let native_a = digest(crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
            crate::NativeProgramArtifactIdentity::new([0x41; 32]),
        ));
        let native_b = digest(crate::ProgramEvidenceSnapshot::IdentifiedNativeArchive(
            crate::NativeProgramArtifactIdentity::new([0x42; 32]),
        ));

        let distinct: std::collections::BTreeSet<_> =
            [no_program, unidentified, native_a, native_b]
                .into_iter()
                .collect();
        assert_eq!(distinct.len(), 4);
    }

    #[cfg(feature = "recomp-rs")]
    #[test]
    fn device_state_v7_wire_binds_typed_program_identity_and_dynamic_state() {
        use fn64_abi::recompiled::{
            LiveExecutableRegionEvidenceSnapshot, PendingExecutableWriteEvidenceSnapshot,
            RecompiledProgramEvidenceSnapshot,
        };
        use fn64_recomp_rs::{
            BankId, BlockProgramEvidenceSnapshot, CodeBankEvidenceSnapshot,
            CodeSpanEvidenceSnapshot, GuestPc, ProgramArtifactIdentity,
            ProgramIdentityEvidenceSnapshot, ProgramIdentitySource,
        };

        let identity = |byte| ProgramArtifactIdentity::new([byte; 32]);
        let device = snapshot(42);
        let executor = executor_snapshot();
        let host = host_snapshot();
        let baseline = sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host.clone(),
            crate::ProgramEvidenceSnapshot::NoProgram,
        ));
        let function = crate::ProgramEvidenceSnapshot::TypedRust(
            RecompiledProgramEvidenceSnapshot::Function {
                identity: ProgramIdentityEvidenceSnapshot {
                    identity: identity(1),
                    source: ProgramIdentitySource::CallerSupplied,
                },
            },
        );
        let function_sha = sha256_hex(&encode_device_snapshot(
            device.clone(),
            executor.clone(),
            host.clone(),
            function,
        ));
        assert_ne!(function_sha, baseline);

        let block =
            crate::ProgramEvidenceSnapshot::TypedRust(RecompiledProgramEvidenceSnapshot::Block {
                program: BlockProgramEvidenceSnapshot {
                    identity: ProgramIdentityEvidenceSnapshot {
                        identity: identity(2),
                        source: ProgramIdentitySource::CanonicalBlockProgramSha256,
                    },
                    banks: vec![CodeBankEvidenceSnapshot {
                        id: BankId::new(3),
                        runner_artifact_identity: identity(4),
                        spans: vec![CodeSpanEvidenceSnapshot {
                            vram_start: GuestPc::new(0x8000_1000),
                            words: vec![0x1234_5678],
                        }],
                    }],
                },
                dispatch_artifact_identity: identity(5),
                instruction_budget: 100,
                executable_regions: vec![LiveExecutableRegionEvidenceSnapshot {
                    physical_start: 0x1000,
                    physical_end: 0x2000,
                    virtual_start: GuestPc::new(0x8000_1000),
                    virtual_end: GuestPc::new(0x8000_2000),
                    active_bank: BankId::new(3),
                    active_generation: 6,
                    next_generation: 7,
                    builder_artifact_identity: identity(8),
                }],
                pending_executable_writes: vec![PendingExecutableWriteEvidenceSnapshot {
                    physical_start: 0x1100,
                    physical_end: 0x1200,
                }],
            });
        let block_sha = sha256_hex(&encode_device_snapshot(device, executor, host, block));
        assert_ne!(block_sha, baseline);
        assert_ne!(block_sha, function_sha);
    }

    #[test]
    fn digest_integrity_rejects_stale_root_and_noncanonical_artifact_sets() {
        let valid = complete_digest();

        let mut stale_root = valid.clone();
        stale_root.root_sha256 =
            "f61d68656ce63b773664e4bdf7b19017697cda2232c10f01ca7bde3a9f910705".to_owned();
        assert!(matches!(
            stale_root.verify_integrity(),
            Err(GateError::DigestRootIntegrityMismatch { .. })
        ));

        let mut missing = valid.clone();
        missing.artifacts.pop();
        assert!(matches!(
            missing.verify_integrity(),
            Err(GateError::InvalidArtifactSet { .. })
        ));

        let mut duplicate = valid.clone();
        duplicate.artifacts[1].kind = ArtifactKind::Framebuffer;
        assert!(matches!(
            duplicate.verify_integrity(),
            Err(GateError::InvalidArtifactSet { .. })
        ));

        let mut reordered = valid;
        reordered.artifacts.swap(0, 1);
        assert!(matches!(
            reordered.verify_integrity(),
            Err(GateError::InvalidArtifactSet { .. })
        ));

        let mut noncanonical_sha = complete_digest();
        noncanonical_sha.artifacts[0].sha256.make_ascii_uppercase();
        assert!(matches!(
            noncanonical_sha.verify_integrity(),
            Err(GateError::InvalidReportSha256("digest.artifacts[].sha256"))
        ));
    }

    #[test]
    fn report_rejects_artifact_counts_that_disagree_with_observations() {
        let make_report = |digest| {
            ReleaseGateReport::new(
                "count-binding",
                b"input",
                digest,
                observations(),
                Vec::new(),
            )
        };

        let mut wrong_memory = complete_digest();
        wrong_memory.artifacts[2].bytes -= 1;
        wrong_memory.root_sha256 =
            recompute_digest_root(wrong_memory.guest_cycle, &wrong_memory.artifacts).unwrap();
        assert!(matches!(
            make_report(wrong_memory),
            Err(GateError::ArtifactObservationByteMismatch {
                kind: ArtifactKind::Memory,
                ..
            })
        ));

        let mut wrong_reference_framebuffer = complete_digest();
        wrong_reference_framebuffer.artifacts[0].bytes += 1;
        wrong_reference_framebuffer.root_sha256 = recompute_digest_root(
            wrong_reference_framebuffer.guest_cycle,
            &wrong_reference_framebuffer.artifacts,
        )
        .unwrap();
        assert!(matches!(
            make_report(wrong_reference_framebuffer),
            Err(GateError::ArtifactObservationByteMismatch {
                kind: ArtifactKind::Framebuffer,
                ..
            })
        ));

        let geometry = ReleaseObservationGeometry::post_vi_swapchain(
            authoritative_rt64_identity(),
            "11".repeat(32),
            1,
            1,
            1,
            1,
            4,
            4,
        )
        .unwrap();
        let expected = geometry.expected_framebuffer_artifact_bytes().unwrap();
        let mut digest = complete_digest();
        digest.artifacts[0].bytes = expected - 1;
        digest.root_sha256 = recompute_digest_root(digest.guest_cycle, &digest.artifacts).unwrap();
        assert!(matches!(
            ReleaseGateReport::new("post-vi-count", b"input", digest, geometry, Vec::new()),
            Err(GateError::ArtifactObservationByteMismatch {
                kind: ArtifactKind::Framebuffer,
                ..
            })
        ));
    }

    #[test]
    fn report_rejects_contradictory_closure_states() {
        let base = ClosurePath {
            name: "path".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        };
        let event = UnsupportedEvent {
            subsystem: "test".to_owned(),
            operation: "unsupported".to_owned(),
            context: "fixture".to_owned(),
            guest_cycle: Some(42),
            disposition: "loud_trap".to_owned(),
        };
        let mut invalid = Vec::new();
        invalid.push(ClosurePath {
            observations: 1,
            status: ClosurePathStatus::Unexercised,
            ..base.clone()
        });
        invalid.push(ClosurePath {
            observations: 0,
            ..base.clone()
        });
        invalid.push(ClosurePath {
            status: ClosurePathStatus::ExercisedUnsupported,
            ..base.clone()
        });
        invalid.push(ClosurePath {
            observations: 1,
            status: ClosurePathStatus::ExercisedUnsupported,
            unsupported: vec![event.clone(), event],
            ..base
        });

        for closure in invalid {
            assert!(matches!(
                ReleaseGateReport::new(
                    "closure-invariant",
                    b"input",
                    complete_digest(),
                    observations(),
                    vec![closure],
                ),
                Err(GateError::InvalidClosurePath { .. })
            ));
        }
    }

    #[test]
    fn report_sha_binds_input_scenario_digest_and_canonical_closure() {
        let paths = vec![
            ClosurePath {
                name: "render".to_owned(),
                observations: 2,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            },
            ClosurePath {
                name: "cpu".to_owned(),
                observations: 1,
                status: ClosurePathStatus::ExercisedZeroUnsupported,
                unsupported: Vec::new(),
            },
        ];
        let report = ReleaseGateReport::new(
            "rs-reference-lle",
            b"rom-a",
            complete_digest(),
            observations(),
            paths.clone(),
        )
        .unwrap();
        let reordered = ReleaseGateReport::new(
            "rs-reference-lle",
            b"rom-a",
            complete_digest(),
            observations(),
            paths.into_iter().rev().collect(),
        )
        .unwrap();
        assert_eq!(report.report_sha256, reordered.report_sha256);

        let different_scenario = ReleaseGateReport::new(
            "rs-rt64-lle",
            b"rom-a",
            complete_digest(),
            observations(),
            report.closure.clone(),
        )
        .unwrap();
        let different_input = ReleaseGateReport::new(
            "rs-reference-lle",
            b"rom-b",
            complete_digest(),
            observations(),
            report.closure.clone(),
        )
        .unwrap();
        assert_ne!(report.report_sha256, different_scenario.report_sha256);
        assert_ne!(report.report_sha256, different_input.report_sha256);
        report.verify_integrity().unwrap();

        let mut duplicate_closure = report.clone();
        duplicate_closure
            .closure
            .push(duplicate_closure.closure[0].clone());
        duplicate_closure.report_sha256 =
            sha256_hex(&encode_report_evidence(&duplicate_closure).unwrap());
        assert!(matches!(
            duplicate_closure.verify_integrity(),
            Err(GateError::DuplicateClosurePath(_))
        ));

        let mut empty_closure_name = report.clone();
        empty_closure_name.closure[0].name.clear();
        empty_closure_name.report_sha256 =
            sha256_hex(&encode_report_evidence(&empty_closure_name).unwrap());
        assert!(matches!(
            empty_closure_name.verify_integrity(),
            Err(GateError::EmptyPathName)
        ));

        let mut reordered_closure = report.clone();
        reordered_closure.closure.swap(0, 1);
        reordered_closure.report_sha256 =
            sha256_hex(&encode_report_evidence(&reordered_closure).unwrap());
        assert!(matches!(
            reordered_closure.verify_integrity(),
            Err(GateError::NonCanonicalClosureOrder { .. })
        ));

        let mut relabeled_source = report.clone();
        let FramebufferObservationSource::PhysicalRdram { address } =
            &mut relabeled_source.observations.framebuffer.source
        else {
            unreachable!("test report uses physical RDRAM")
        };
        *address = 2;
        assert!(matches!(
            relabeled_source.verify_integrity(),
            Err(GateError::ReportIntegrityMismatch { .. })
        ));

        let encoded = serde_json::to_vec(&report).unwrap();
        let mut retained: ReleaseGateReport = serde_json::from_slice(&encoded).unwrap();
        retained.scenario.push_str("-mutated");
        assert!(matches!(
            retained.verify_integrity(),
            Err(GateError::ReportIntegrityMismatch { .. })
        ));
        assert!(matches!(
            retained.require_closed(),
            Err(GateError::ReportIntegrityMismatch { .. })
        ));

        let mut stale_schema = report.clone();
        stale_schema.schema = "fn64.release-gate.v19".to_owned();
        assert!(matches!(
            stale_schema.verify_integrity(),
            Err(GateError::UnsupportedReportSchema(schema))
                if schema == "fn64.release-gate.v19"
        ));

        let duplicate = vec![report.closure[0].clone(), report.closure[0].clone()];
        assert!(matches!(
            ReleaseGateReport::new(
                "duplicate",
                b"rom",
                complete_digest(),
                observations(),
                duplicate,
            ),
            Err(GateError::DuplicateClosurePath(_))
        ));
    }

    #[test]
    fn schema_v20_report_wire_binds_every_release_environment_field() {
        let report = ReleaseGateReport::new(
            "environment-wire",
            b"input",
            complete_digest(),
            observations(),
            Vec::new(),
        )
        .unwrap();
        let digest = |value: &ReleaseGateReport| {
            sha256_hex(&encode_report_evidence(value).expect("environment encodes"))
        };
        let baseline = digest(&report);

        for platform in [
            ReleaseHostPlatform::MacosArm64,
            ReleaseHostPlatform::LinuxX86_64,
            ReleaseHostPlatform::WindowsX86_64,
        ] {
            if platform != report.environment.platform {
                let mut changed = report.clone();
                changed.environment.platform = platform;
                assert_ne!(digest(&changed), baseline, "platform tag collided");
            }
        }

        let mut windows = report.clone();
        windows.environment.platform = ReleaseHostPlatform::WindowsX86_64;
        windows.environment.windows_version = Some(
            ReleaseWindowsVersionEvidence::from_native_workstation(10, 0, 22_000, 123).unwrap(),
        );
        validate_environment_evidence(&windows.environment).unwrap();
        let windows_baseline = digest(&windows);
        for mutate in [
            |version: &mut ReleaseWindowsVersionEvidence| version.major = 11,
            |version: &mut ReleaseWindowsVersionEvidence| version.minor = 1,
            |version: &mut ReleaseWindowsVersionEvidence| version.build += 1,
            |version: &mut ReleaseWindowsVersionEvidence| version.update_build_revision += 1,
            |version: &mut ReleaseWindowsVersionEvidence| {
                version.family = ReleaseWindowsFamily::Windows10
            },
        ] {
            let mut changed = windows.clone();
            mutate(changed.environment.windows_version.as_mut().unwrap());
            assert_ne!(
                digest(&changed),
                windows_baseline,
                "Windows identity field collided"
            );
        }
        let mut missing_windows_version = windows.clone();
        missing_windows_version.environment.windows_version = None;
        assert!(matches!(
            validate_environment_evidence(&missing_windows_version.environment),
            Err(GateError::InvalidWindowsVersionEvidence(_))
        ));
        let mut attached_to_macos = windows;
        attached_to_macos.environment.platform = ReleaseHostPlatform::MacosArm64;
        assert!(matches!(
            validate_environment_evidence(&attached_to_macos.environment),
            Err(GateError::InvalidWindowsVersionEvidence(_))
        ));

        let port_states = [
            ReleaseControllerPort::StandardControllerNoPak,
            ReleaseControllerPort::StandardControllerControllerPak,
            ReleaseControllerPort::StandardControllerRumblePak,
            ReleaseControllerPort::StandardControllerTransferPak,
            ReleaseControllerPort::VoiceRecognitionUnit,
            ReleaseControllerPort::Absent,
        ];
        for index in 0..4 {
            for state in port_states {
                if state != report.environment.controller_ports[index] {
                    let mut changed = report.clone();
                    changed.environment.controller_ports[index] = state;
                    assert_ne!(
                        digest(&changed),
                        baseline,
                        "controller port {index} state {state:?} collided"
                    );
                }
            }
        }

        for save in [
            ReleaseCartridgeSave::NoCartridgeSave,
            ReleaseCartridgeSave::Eeprom4k,
            ReleaseCartridgeSave::Eeprom16k,
            ReleaseCartridgeSave::Sram32Kib,
            ReleaseCartridgeSave::FlashRam128Kib,
        ] {
            if save != report.environment.cartridge_save {
                let mut changed = report.clone();
                changed.environment.cartridge_save = save;
                assert_ne!(digest(&changed), baseline, "cartridge save tag collided");
            }
        }

        let mut changed_policy = report.clone();
        changed_policy.environment.renderer = ReleaseRendererEvidence::Reference {
            execution_policy: ReleaseGraphicsExecutionPolicy::HleOptimized,
            tv_type: ReleaseTvStandard::Ntsc,
        };
        assert_ne!(digest(&changed_policy), baseline, "render policy collided");

        let mut changed_tv = report.clone();
        let ReleaseRendererEvidence::Reference { tv_type, .. } =
            &mut changed_tv.environment.renderer
        else {
            unreachable!()
        };
        *tv_type = ReleaseTvStandard::Pal;
        assert_ne!(digest(&changed_tv), baseline, "renderer TV type collided");

        let mut rt64 = report.clone();
        rt64.environment.renderer = ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: current_test_graphics_api(),
            backend_identity: authoritative_rt64_identity(),
            source_authoritative: true,
            settings_sha256: "11".repeat(32),
            replacement_packs_active: false,
        };
        let rt64_baseline = digest(&rt64);
        assert_ne!(rt64_baseline, baseline, "renderer class collided");

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 {
            execution_policy, ..
        } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        *execution_policy = ReleaseGraphicsExecutionPolicy::HleOptimized;
        assert_ne!(digest(&changed), rt64_baseline, "RT64 policy collided");

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 { graphics_api, .. } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        *graphics_api = match *graphics_api {
            ReleaseGraphicsApi::D3d12 => ReleaseGraphicsApi::Vulkan,
            ReleaseGraphicsApi::Vulkan | ReleaseGraphicsApi::Metal => ReleaseGraphicsApi::D3d12,
        };
        assert_ne!(digest(&changed), rt64_baseline, "graphics API collided");

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 {
            backend_identity, ..
        } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        backend_identity.push_str("-changed");
        assert_ne!(digest(&changed), rt64_baseline, "backend identity collided");

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 {
            source_authoritative,
            ..
        } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        *source_authoritative = false;
        assert_ne!(digest(&changed), rt64_baseline, "source authority collided");

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 {
            settings_sha256, ..
        } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        *settings_sha256 = "22".repeat(32);
        assert_ne!(
            digest(&changed),
            rt64_baseline,
            "settings identity collided"
        );

        let mut changed = rt64.clone();
        let ReleaseRendererEvidence::Rt64 {
            replacement_packs_active,
            ..
        } = &mut changed.environment.renderer
        else {
            unreachable!()
        };
        *replacement_packs_active = true;
        assert_ne!(
            digest(&changed),
            rt64_baseline,
            "replacement-pack activity collided"
        );
    }

    #[test]
    fn frozen_environment_derivation_fails_closed() {
        let platform = crate::release_host_platform().expect("supported test platform");
        let reference = || fn64_abi::RenderEnvironmentEvidenceSnapshot {
            backend: fn64_abi::RenderBackendEvidence::Reference {
                tv_type: TvType::Ntsc,
            },
            execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
        };

        let mut host = host_snapshot();
        host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::Unidentified;
        assert!(matches!(
            environment_from_frozen(platform, None, &host, reference()),
            Err(GateError::UnidentifiedCartridgeSave)
        ));

        host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;
        assert!(matches!(
            environment_from_frozen(
                platform,
                None,
                &host,
                fn64_abi::RenderEnvironmentEvidenceSnapshot {
                    backend: fn64_abi::RenderBackendEvidence::Unidentified,
                    execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
                },
            ),
            Err(GateError::UnidentifiedRenderBackend)
        ));
        assert!(matches!(
            environment_from_frozen(
                platform,
                None,
                &host,
                fn64_abi::RenderEnvironmentEvidenceSnapshot {
                    backend: fn64_abi::RenderBackendEvidence::Reference {
                        tv_type: TvType::Ntsc,
                    },
                    execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized,
                },
            ),
            Err(GateError::NonAccuracyRenderPolicy)
        ));
    }

    #[test]
    fn frozen_environment_derivation_preserves_each_concrete_graphics_api() {
        let platform = crate::release_host_platform().expect("supported test platform");
        let mut host = host_snapshot();
        host.cartridge_save = fn64_abi::CartridgeSaveEvidenceSnapshot::NoCartridgeSave;

        for (active, expected) in [
            (
                fn64_abi::ActiveRenderGraphicsApi::D3d12,
                ReleaseGraphicsApi::D3d12,
            ),
            (
                fn64_abi::ActiveRenderGraphicsApi::Vulkan,
                ReleaseGraphicsApi::Vulkan,
            ),
            (
                fn64_abi::ActiveRenderGraphicsApi::Metal,
                ReleaseGraphicsApi::Metal,
            ),
        ] {
            let environment = environment_from_frozen(
                platform,
                None,
                &host,
                fn64_abi::RenderEnvironmentEvidenceSnapshot {
                    backend: fn64_abi::RenderBackendEvidence::Rt64 {
                        tv_type: TvType::Ntsc,
                        backend_identity: authoritative_rt64_identity_for(expected),
                        source_authoritative: true,
                        graphics_api: active,
                        settings_sha256: [0x11; 32],
                        replacement_packs_active: false,
                    },
                    execution_policy: fn64_abi::GraphicsTaskExecutionPolicy::LleAccuracy,
                },
            )
            .unwrap();
            assert!(matches!(
                environment.renderer,
                ReleaseRendererEvidence::Rt64 { graphics_api, .. } if graphics_api == expected
            ));
        }
    }

    #[test]
    fn release_renderer_json_requires_a_concrete_rt64_api_only() {
        let rt64 = serde_json::json!({
            "kind": "rt64",
            "execution_policy": "lle_accuracy",
            "graphics_api": "automatic",
            "backend_identity": "identity",
            "source_authoritative": true,
            "settings_sha256": "11".repeat(32),
            "replacement_packs_active": false,
        });
        assert!(serde_json::from_value::<ReleaseRendererEvidence>(rt64).is_err());

        let reference_with_api = serde_json::json!({
            "kind": "reference",
            "execution_policy": "lle_accuracy",
            "graphics_api": "vulkan",
        });
        assert!(
            serde_json::from_value::<ReleaseRendererEvidence>(reference_with_api).is_err(),
            "reference evidence must reject an RT64-only graphics API field"
        );
    }

    #[test]
    fn release_environment_rejects_cross_platform_graphics_api_pair() {
        let platform = crate::release_host_platform().expect("supported test platform");
        let valid_api = current_test_graphics_api();
        let environment = ReleaseEnvironmentEvidence {
            platform,
            windows_version: crate::test_release_windows_version(),
            controller_ports: [ReleaseControllerPort::Absent; 4],
            cartridge_save: ReleaseCartridgeSave::NoCartridgeSave,
            renderer: ReleaseRendererEvidence::Rt64 {
                execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
                tv_type: ReleaseTvStandard::Ntsc,
                graphics_api: valid_api,
                backend_identity: authoritative_rt64_identity_for(valid_api),
                source_authoritative: true,
                settings_sha256: "11".repeat(32),
                replacement_packs_active: false,
            },
        };
        validate_environment_evidence(&environment).unwrap();

        let invalid_api = match platform {
            ReleaseHostPlatform::MacosArm64 | ReleaseHostPlatform::LinuxX86_64 => {
                ReleaseGraphicsApi::D3d12
            }
            ReleaseHostPlatform::WindowsX86_64 => ReleaseGraphicsApi::Metal,
        };
        let mut invalid = environment;
        let ReleaseRendererEvidence::Rt64 {
            graphics_api,
            backend_identity,
            ..
        } = &mut invalid.renderer
        else {
            unreachable!()
        };
        *graphics_api = invalid_api;
        *backend_identity = authoritative_rt64_identity_for(invalid_api);
        assert!(matches!(
            validate_environment_evidence(&invalid),
            Err(GateError::RendererObservationMismatch(_))
        ));
    }

    #[test]
    fn report_rejects_untrusted_or_mismatched_renderer_evidence() {
        let mut environment = test_release_environment(&observations());
        environment.renderer = ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: current_test_graphics_api(),
            backend_identity: "rt64-test".to_owned(),
            source_authoritative: false,
            settings_sha256: "11".repeat(32),
            replacement_packs_active: false,
        };
        assert!(matches!(
            ReleaseGateReport::new_with_test_environment(
                "untrusted-renderer",
                b"input",
                complete_digest(),
                observations(),
                environment,
                Vec::new(),
            ),
            Err(GateError::RendererObservationMismatch(_))
        ));

        let mut environment = test_release_environment(&observations());
        environment.renderer = ReleaseRendererEvidence::Rt64 {
            execution_policy: ReleaseGraphicsExecutionPolicy::LleAccuracy,
            tv_type: ReleaseTvStandard::Ntsc,
            graphics_api: current_test_graphics_api(),
            backend_identity: "rt64-test".to_owned(),
            source_authoritative: true,
            settings_sha256: "11".repeat(32),
            replacement_packs_active: false,
        };
        assert!(matches!(
            ReleaseGateReport::new_with_test_environment(
                "mismatched-renderer",
                b"input",
                complete_digest(),
                observations(),
                environment,
                Vec::new(),
            ),
            Err(GateError::RendererObservationMismatch(_))
        ));
    }

    #[test]
    fn timing_digest_ignores_ambient_sequence_but_rejects_future_events() {
        let event = |seq| TraceEvent {
            seq,
            sim_time: 41,
            kind: TraceKind::ThreadSwitch {
                from: Some(1),
                to: 2,
                reason: SwitchReason::PauseSelf,
            },
        };
        assert_eq!(
            encode_timing_trace(&[event(1)]),
            encode_timing_trace(&[event(9_999)])
        );

        let mut gate = FixedCycleDigestGate::new(42);
        assert!(matches!(
            gate.capture_timing_trace(
                42,
                &[TraceEvent {
                    sim_time: 43,
                    ..event(0)
                }],
            ),
            Err(GateError::FutureTraceEvent { .. })
        ));
    }

    #[test]
    fn live_timing_digest_binds_typed_device_dma_and_rejects_future_events() {
        let pi = DeviceTraceEvent {
            at: Cycles::new(41),
            sequence: 500,
            kind: DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x200),
                cart_addr: 0x1000,
                len: 64,
            }),
        };
        let mut left = FixedCycleDigestGate::new(42);
        left.capture_live_timing_trace(42, &[], &[pi]).unwrap();
        let left = left.artifacts[&ArtifactKind::TimingTrace].sha256.clone();

        let mut changed_sequence = pi;
        changed_sequence.sequence = 999;
        let mut right = FixedCycleDigestGate::new(42);
        right
            .capture_live_timing_trace(42, &[], &[changed_sequence])
            .unwrap();
        assert_eq!(left, right.artifacts[&ArtifactKind::TimingTrace].sha256);

        let mut changed_request = pi;
        changed_request.kind = DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
            len: 128,
            ..match pi.kind {
                DeviceTraceKind::PiBytesCommitted(request) => request,
                _ => unreachable!(),
            }
        });
        let mut changed = FixedCycleDigestGate::new(42);
        changed
            .capture_live_timing_trace(42, &[], &[changed_request])
            .unwrap();
        assert_ne!(left, changed.artifacts[&ArtifactKind::TimingTrace].sha256);

        let mut future = pi;
        future.at = Cycles::new(43);
        let mut gate = FixedCycleDigestGate::new(42);
        assert!(matches!(
            gate.capture_live_timing_trace(42, &[], &[future]),
            Err(GateError::FutureDeviceTraceEvent { .. })
        ));
    }

    #[test]
    fn wrong_cycle_and_missing_channel_fail_loudly() {
        let mut gate = FixedCycleDigestGate::new(10);
        assert!(matches!(
            gate.capture(9, ArtifactKind::Audio, b"late"),
            Err(GateError::WrongCycle { .. })
        ));
        assert!(matches!(gate.finish(), Err(GateError::MissingArtifacts(_))));
    }

    #[test]
    fn report_distinguishes_unexercised_zero_and_unsupported() {
        let mut closure = ClosureGate::default();
        closure.declare("cpu.dynamic-target").unwrap();
        closure.declare("rsp.custom-ucode").unwrap();
        closure.declare("rdp.raw-command").unwrap();
        closure.observe_supported("rsp.custom-ucode").unwrap();
        closure
            .observe_unsupported(
                "rdp.raw-command",
                "render",
                "rdp.opcode.0x3f",
                "task=7 word=12",
                Some(42),
                "loud_trap",
            )
            .unwrap();
        let report = ReleaseGateReport::new(
            "synthetic-unsupported",
            b"synthetic input",
            complete_digest(),
            observations(),
            closure.finish(),
        )
        .unwrap();
        let error = report.require_closed().unwrap_err().to_string();
        assert!(error.contains("cpu.dynamic-target"));
        assert!(error.contains("rdp.raw-command:rdp.opcode.0x3f"));

        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["closure"][0]["status"], "unexercised");
        assert_eq!(json["closure"][1]["status"], "exercised_unsupported");
        assert_eq!(json["closure"][2]["status"], "exercised_zero_unsupported");
    }

    #[test]
    fn live_closure_binds_typed_unsupported_source_or_proves_zero() {
        let zero = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"",
            audio_bytes: b"",
            memory_bytes: b"",
            trace: &[],
            device_trace: &[],
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();
        let source = zero
            .iter()
            .find(|path| path.name == "execution.unsupported-event-source")
            .unwrap();
        assert_eq!(source.observations, 1);
        assert!(matches!(
            source.status,
            ClosurePathStatus::ExercisedZeroUnsupported
        ));

        let reached = [fn64_runtime::UnsupportedEvent {
            sequence: 99,
            subsystem: fn64_runtime::UnsupportedSubsystem::Render,
            operation: "render.hle-ucode.needs-lle".to_owned(),
            context: "microcode_sha256=0011".to_owned(),
            guest_cycle: Some(Cycles::new(42)),
            disposition: fn64_runtime::UnsupportedDisposition::NeedsLle,
        }];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"",
            audio_bytes: b"",
            memory_bytes: b"",
            trace: &[],
            device_trace: &[],
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &reached,
        })
        .unwrap();
        let source = closure
            .iter()
            .find(|path| path.name == "execution.unsupported-event-source")
            .unwrap();
        assert!(matches!(
            source.status,
            ClosurePathStatus::ExercisedUnsupported
        ));
        assert_eq!(source.unsupported[0].subsystem, "render");
        assert_eq!(
            source.unsupported[0].operation,
            "render.hle-ucode.needs-lle"
        );
        assert_eq!(source.unsupported[0].guest_cycle, Some(42));
        assert_eq!(source.unsupported[0].disposition, "needs_lle");
    }

    #[test]
    fn all_exercised_and_supported_is_release_closed() {
        let mut closure = ClosureGate::default();
        for path in ["cpu", "devices", "render"] {
            closure.declare(path).unwrap();
            closure.observe_supported(path).unwrap();
        }
        ReleaseGateReport::new(
            "synthetic-closed",
            b"synthetic input",
            complete_digest(),
            observations(),
            closure.finish(),
        )
        .unwrap()
        .require_closed()
        .unwrap();
    }

    #[test]
    fn empty_closure_cannot_claim_zero_unsupported() {
        let report = ReleaseGateReport::new(
            "synthetic-empty",
            b"synthetic input",
            complete_digest(),
            observations(),
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            report.require_closed(),
            Err(GateError::NoClosurePaths)
        ));
    }

    #[test]
    fn live_closure_is_derived_from_artifacts_and_typed_trace_events() {
        let trace = [
            TraceEvent {
                seq: 1,
                sim_time: 1,
                kind: TraceKind::ThreadSwitch {
                    from: None,
                    to: 1,
                    reason: SwitchReason::Scheduled,
                },
            },
            TraceEvent {
                seq: 2,
                sim_time: 2,
                kind: TraceKind::QueueOp {
                    queue: RdramAddr::from_offset(0x100),
                    op: QueueOpKind::Send,
                    thread: 1,
                },
            },
            TraceEvent {
                seq: 3,
                sim_time: 3,
                kind: TraceKind::Dma {
                    direction: DmaDirection::ToRdram,
                    dram: RdramAddr::from_offset(0x200),
                    dev_addr: 0x1000,
                    len: 64,
                },
            },
            TraceEvent {
                seq: 4,
                sim_time: 4,
                kind: TraceKind::TaskSubmit {
                    task_kind: TaskKind::Graphics,
                    ucode: 0x300,
                },
            },
            TraceEvent {
                seq: 5,
                sim_time: 5,
                kind: TraceKind::TaskSubmit {
                    task_kind: TaskKind::Audio,
                    ucode: 0x400,
                },
            },
        ];
        let device_trace = [
            DeviceTraceEvent {
                at: Cycles::new(5),
                sequence: 1,
                kind: DeviceTraceKind::PiBytesCommitted(PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: RdramAddr::from_offset(0x200),
                    cart_addr: 0x1000,
                    len: 64,
                }),
            },
            DeviceTraceEvent {
                at: Cycles::new(6),
                sequence: 2,
                kind: DeviceTraceKind::SiBytesCommitted(SiDmaRequest {
                    kind: SiDmaKind::PifToDram,
                    dram_addr: RdramAddr::from_offset(0x300),
                }),
            },
            DeviceTraceEvent {
                at: Cycles::new(7),
                sequence: 3,
                kind: DeviceTraceKind::AiDmaComplete(AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x400),
                    len: 2240,
                    sample_rate_hz: 32_000,
                }),
            },
            DeviceTraceEvent {
                at: Cycles::new(8),
                sequence: 4,
                kind: DeviceTraceKind::SpTaskAdmitted {
                    task_addr: RdramAddr::from_offset(0x500),
                    header: OsTaskHeader {
                        task_type: fn64_runtime::M_GFXTASK,
                        ucode_boot: 0x8000_1000,
                        ucode_boot_size: 0x100,
                        ..OsTaskHeader::default()
                    },
                },
            },
        ];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"fb",
            audio_bytes: b"pcm",
            memory_bytes: b"memory",
            trace: &trace,
            device_trace: &device_trace,
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();
        assert_eq!(closure.len(), LIVE_MINIMUM_CLOSURE_PATHS.len());
        assert!(closure.iter().all(|path| {
            path.observations > 0
                && matches!(path.status, ClosurePathStatus::ExercisedZeroUnsupported)
        }));
    }

    #[test]
    fn live_closure_derives_positive_save_paths_by_authoritative_device_type() {
        let save_operations = [
            SaveOperationEvent {
                at: Cycles::new(2),
                device: SaveType::Eeprom4k,
                operation: SaveOperationKind::Read,
                offset: 0,
                len: 8,
            },
            SaveOperationEvent {
                at: Cycles::new(3),
                device: SaveType::Eeprom4k,
                operation: SaveOperationKind::Write,
                offset: 8,
                len: 8,
            },
            SaveOperationEvent {
                at: Cycles::new(4),
                device: SaveType::Eeprom16k,
                operation: SaveOperationKind::Read,
                offset: 0,
                len: 8,
            },
            SaveOperationEvent {
                at: Cycles::new(5),
                device: SaveType::SramBanked,
                operation: SaveOperationKind::Write,
                offset: 0x20,
                len: 32,
            },
            SaveOperationEvent {
                at: Cycles::new(6),
                device: SaveType::FlashRam,
                operation: SaveOperationKind::Erase,
                offset: 0,
                len: 16 * 1024,
            },
            SaveOperationEvent {
                at: Cycles::new(7),
                device: SaveType::ControllerPak,
                operation: SaveOperationKind::Read,
                offset: 0,
                len: 32,
            },
        ];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"",
            audio_bytes: b"",
            memory_bytes: b"",
            trace: &[],
            device_trace: &[],
            save_operations: &save_operations,
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();

        let eeprom = closure
            .iter()
            .find(|path| path.name == "save.eeprom-4k-operation")
            .unwrap();
        assert_eq!(eeprom.observations, 2);
        assert!(matches!(
            eeprom.status,
            ClosurePathStatus::ExercisedZeroUnsupported
        ));
        assert!(closure
            .iter()
            .any(|path| path.name == "save.flashram-operation" && path.observations == 1));
        assert!(closure
            .iter()
            .any(|path| path.name == "save.eeprom-16k-operation" && path.observations == 1));
        assert!(closure
            .iter()
            .any(|path| path.name == "save.sram-operation" && path.observations == 1));
        assert!(closure
            .iter()
            .any(|path| path.name == "save.pfs-operation" && path.observations == 1));
    }

    #[test]
    fn live_closure_derives_controller_paths_only_from_successful_operations() {
        let controller_operations = [
            ControllerOperationEvent {
                at: Cycles::new(2),
                port: 0,
                device: ControllerOperationDevice::StandardController,
                operation: fn64_runtime::ControllerOperationKind::Read,
            },
            ControllerOperationEvent {
                at: Cycles::new(3),
                port: 0,
                device: ControllerOperationDevice::RumblePak,
                operation: fn64_runtime::ControllerOperationKind::Control,
            },
            ControllerOperationEvent {
                at: Cycles::new(4),
                port: 1,
                device: ControllerOperationDevice::TransferPak,
                operation: fn64_runtime::ControllerOperationKind::Write,
            },
            ControllerOperationEvent {
                at: Cycles::new(5),
                port: 2,
                device: ControllerOperationDevice::VoiceRecognitionUnit,
                operation: fn64_runtime::ControllerOperationKind::Read,
            },
        ];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"",
            audio_bytes: b"",
            memory_bytes: b"",
            trace: &[],
            device_trace: &[],
            save_operations: &[],
            controller_operations: &controller_operations,
            unsupported_events: &[],
        })
        .unwrap();

        for (_, path) in LIVE_CONTROLLER_OPERATION_CLOSURE_PATHS {
            let evidence = closure
                .iter()
                .find(|candidate| candidate.name == path)
                .unwrap();
            assert_eq!(evidence.observations, 1);
            assert_eq!(evidence.status, ClosurePathStatus::ExercisedZeroUnsupported);
        }
    }

    #[test]
    fn schema_v20_rsp_rdp_wire_rejects_tamper_future_cycles_and_false_graphics_closure() {
        let geometry = observations();
        let graphics_closure = vec![ClosurePath {
            name: "rsp.graphics-task".to_owned(),
            observations: 1,
            status: ClosurePathStatus::ExercisedZeroUnsupported,
            unsupported: Vec::new(),
        }];
        let ordered = vec![
            RspRdpObservationEventEvidence {
                guest_cycle: 40,
                observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                    task_address: 0x1000,
                    imem_generation: 3,
                    text_sha256: "11".repeat(32),
                    data_address: 0x2000,
                    data_bytes: 0x80,
                    data_sha256: "12".repeat(32),
                    family: Some(ReleaseMicrocodeFamily::F3dex2),
                },
            },
            RspRdpObservationEventEvidence {
                guest_cycle: 41,
                observation: RspRdpObservationKindEvidence::DramDpcCommitted {
                    start: 0x100,
                    end: 0x108,
                    command_sha256: "22".repeat(32),
                },
            },
            RspRdpObservationEventEvidence {
                guest_cycle: 42,
                observation: RspRdpObservationKindEvidence::ImemReplacementCommitted {
                    task_address: 0x1000,
                    imem_generation: 4,
                    text_sha256: "33".repeat(32),
                },
            },
        ];
        let report = ReleaseGateReport::new_with_environment(
            "rsp-rdp-wire",
            b"input",
            complete_digest(),
            ReleaseBoundaryReportEvidence {
                rom: None,
                observations: geometry.clone(),
                environment: test_release_environment(&geometry),
                execution_destinations: ExecutionDestinationEvidence::no_program(),
                rsp_rdp: RspRdpEvidence::from_ordered(ordered).unwrap(),
            },
            graphics_closure.clone(),
        )
        .unwrap();
        report.verify_integrity().unwrap();

        let mut changed_data_events = report.rsp_rdp.ordered.clone();
        let RspRdpObservationKindEvidence::MicrocodeRecognition { data_sha256, .. } =
            &mut changed_data_events[0].observation
        else {
            panic!("first fixture event must be microcode recognition");
        };
        *data_sha256 = "13".repeat(32);
        let mut changed_data = report.clone();
        changed_data.rsp_rdp = RspRdpEvidence::from_ordered(changed_data_events).unwrap();
        assert!(matches!(
            changed_data.verify_integrity(),
            Err(GateError::ReportIntegrityMismatch { .. })
        ));

        let mut reordered = report.clone();
        reordered.rsp_rdp.ordered.swap(0, 1);
        assert!(matches!(
            reordered.verify_integrity(),
            Err(GateError::NonMonotonicRspRdpObservationCycle {
                previous: 41,
                observed: 40
            })
        ));

        let mut future = report.clone();
        future.rsp_rdp.ordered[0].guest_cycle = 43;
        assert!(matches!(
            future.verify_integrity(),
            Err(GateError::FutureRspRdpObservation {
                gate_cycle: 42,
                event_cycle: 43
            })
        ));

        let mut nonmonotonic_cycle_events = report.rsp_rdp.ordered.clone();
        nonmonotonic_cycle_events[1].guest_cycle = 39;
        let mut nonmonotonic_cycle = report.clone();
        nonmonotonic_cycle.rsp_rdp =
            RspRdpEvidence::from_ordered(nonmonotonic_cycle_events).unwrap();
        assert!(matches!(
            nonmonotonic_cycle.verify_integrity(),
            Err(GateError::NonMonotonicRspRdpObservationCycle {
                previous: 40,
                observed: 39
            })
        ));

        let mut regressing_generation_events = report.rsp_rdp.ordered.clone();
        if let RspRdpObservationKindEvidence::ImemReplacementCommitted {
            imem_generation, ..
        } = &mut regressing_generation_events[2].observation
        {
            *imem_generation = 2;
        }
        let mut regressing_generation = report.clone();
        regressing_generation.rsp_rdp =
            RspRdpEvidence::from_ordered(regressing_generation_events).unwrap();
        assert!(matches!(
            regressing_generation.verify_integrity(),
            Err(GateError::NonMonotonicImemReplacementGeneration {
                previous: 3,
                observed: 2
            })
        ));

        let mut conflicting_digest_events = report.rsp_rdp.ordered.clone();
        conflicting_digest_events.push(RspRdpObservationEventEvidence {
            guest_cycle: 42,
            observation: RspRdpObservationKindEvidence::MicrocodeRecognition {
                task_address: 0x1000,
                imem_generation: 4,
                text_sha256: "44".repeat(32),
                data_address: 0x2000,
                data_bytes: 0x80,
                data_sha256: "12".repeat(32),
                family: None,
            },
        });
        let mut conflicting_digest = report.clone();
        conflicting_digest.rsp_rdp =
            RspRdpEvidence::from_ordered(conflicting_digest_events).unwrap();
        assert!(matches!(
            conflicting_digest.verify_integrity(),
            Err(GateError::ConflictingImemGenerationDigest { generation: 4, .. })
        ));

        let mut invalid_range = report.clone();
        invalid_range.rsp_rdp.ordered[1].observation =
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start: 0x101,
                end: 0x108,
                command_sha256: "22".repeat(32),
            };
        assert!(matches!(
            invalid_range.verify_integrity(),
            Err(GateError::InvalidDpcObservationRange { source: "DRAM", .. })
        ));

        let mut host_only_dram_range = report.clone();
        host_only_dram_range.rsp_rdp.ordered[1].observation =
            RspRdpObservationKindEvidence::DramDpcCommitted {
                start: crate::DEFAULT_RDRAM_SIZE as u32,
                end: crate::DEFAULT_RDRAM_SIZE as u32 + 8,
                command_sha256: "22".repeat(32),
            };
        assert!(matches!(
            host_only_dram_range.verify_integrity(),
            Err(GateError::InvalidDpcObservationRange { source: "DRAM", .. })
        ));

        for (data_address, data_bytes) in [
            (0x2000, 0),
            (crate::DEFAULT_RDRAM_SIZE as u32, 1),
            (crate::DEFAULT_RDRAM_SIZE as u32 - 0x40, 0x80),
            (u32::MAX - 3, 8),
        ] {
            let mut invalid_data_range = report.clone();
            let RspRdpObservationKindEvidence::MicrocodeRecognition {
                data_address: address,
                data_bytes: bytes,
                ..
            } = &mut invalid_data_range.rsp_rdp.ordered[0].observation
            else {
                panic!("first fixture event must be microcode recognition");
            };
            *address = data_address;
            *bytes = data_bytes;
            assert!(matches!(
                invalid_data_range.verify_integrity(),
                Err(GateError::InvalidMicrocodeDataObservationRange { .. })
            ));
        }

        let mut invalid_recognition_task = report.clone();
        let RspRdpObservationKindEvidence::MicrocodeRecognition { task_address, .. } =
            &mut invalid_recognition_task.rsp_rdp.ordered[0].observation
        else {
            panic!("first fixture event must be microcode recognition");
        };
        *task_address = crate::DEFAULT_RDRAM_SIZE as u32 - 63;
        assert!(matches!(
            invalid_recognition_task.verify_integrity(),
            Err(GateError::InvalidRspTaskObservationAddress { .. })
        ));

        let mut invalid_replacement_task = report.clone();
        let RspRdpObservationKindEvidence::ImemReplacementCommitted { task_address, .. } =
            &mut invalid_replacement_task.rsp_rdp.ordered[2].observation
        else {
            panic!("third fixture event must be IMEM replacement");
        };
        *task_address = u32::MAX;
        assert!(matches!(
            invalid_replacement_task.verify_integrity(),
            Err(GateError::InvalidRspTaskObservationAddress { .. })
        ));

        assert!(matches!(
            ReleaseGateReport::new_with_environment(
                "false-graphics-closure",
                b"input",
                complete_digest(),
                ReleaseBoundaryReportEvidence {
                    rom: None,
                    observations: geometry.clone(),
                    environment: test_release_environment(&geometry),
                    execution_destinations: ExecutionDestinationEvidence::no_program(),
                    rsp_rdp: RspRdpEvidence::from_ordered(Vec::new()).unwrap(),
                },
                graphics_closure,
            ),
            Err(GateError::MissingGraphicsMicrocodeRecognition)
        ));
    }

    #[test]
    fn controller_operation_cycle_validation_rejects_future_evidence() {
        let operation = ControllerOperationEvent {
            at: Cycles::new(43),
            port: 2,
            device: ControllerOperationDevice::TransferPak,
            operation: fn64_runtime::ControllerOperationKind::Read,
        };
        assert!(matches!(
            validate_controller_operation_cycles(42, &[operation]),
            Err(GateError::FutureControllerOperationEvent {
                gate_cycle: 42,
                event_cycle: 43,
                port: 2,
            })
        ));
        assert!(validate_controller_operation_cycles(43, &[operation]).is_ok());
    }

    #[test]
    fn empty_live_artifact_remains_unexercised() {
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"fb",
            audio_bytes: b"",
            memory_bytes: b"memory",
            trace: &[],
            device_trace: &[],
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();
        let audio = closure.iter().find(|path| path.name == "ai.pcm").unwrap();
        assert_eq!(audio.observations, 0);
        assert!(matches!(audio.status, ClosurePathStatus::Unexercised));
    }

    #[test]
    fn generic_executor_dma_cannot_satisfy_device_qualified_closure() {
        let trace = [TraceEvent {
            seq: 1,
            sim_time: 1,
            kind: TraceKind::Dma {
                direction: DmaDirection::ToRdram,
                dram: RdramAddr::from_offset(0x200),
                dev_addr: 0x1000,
                len: 64,
            },
        }];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"fb",
            audio_bytes: b"pcm",
            memory_bytes: b"memory",
            trace: &trace,
            device_trace: &[],
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();
        assert!(closure
            .iter()
            .filter(|path| path.name.starts_with("device."))
            .all(|path| matches!(path.status, ClosurePathStatus::Unexercised)));
    }

    #[test]
    fn accepted_device_dma_does_not_claim_committed_bytes() {
        let device_trace = [DeviceTraceEvent {
            at: Cycles::new(1),
            sequence: 1,
            kind: DeviceTraceKind::PiDmaStarted(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x200),
                cart_addr: 0x1000,
                len: 64,
            }),
        }];
        let closure = derive_live_closure(LiveClosureInputs {
            framebuffer_bytes: b"fb",
            audio_bytes: b"pcm",
            memory_bytes: b"memory",
            trace: &[],
            device_trace: &device_trace,
            save_operations: &[],
            controller_operations: &[],
            unsupported_events: &[],
        })
        .unwrap();
        let pi = closure
            .iter()
            .find(|path| path.name == "device.pi-dma-commit")
            .unwrap();
        assert_eq!(pi.observations, 0);
        assert!(matches!(pi.status, ClosurePathStatus::Unexercised));
    }

    #[test]
    fn live_capture_without_arm_fails_before_sampling_ambient_state() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("fn64-unarmed-release-{}.json", std::process::id()));
        let result = LiveReleaseGate::new(0).capture_and_write_observed(
            crate::CommittedViBoundary::synthetic_for_test(0),
            "unarmed",
            b"input",
            None,
            LiveObservedArtifacts {
                framebuffer_artifact_bytes: b"fb",
                framebuffer_payload_bytes: 2,
                memory_bytes: b"memory",
                observations: observations(),
            },
            path,
        );
        assert!(matches!(result, Err(GateError::LiveGateNotArmed)));
    }
}
