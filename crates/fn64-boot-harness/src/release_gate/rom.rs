#![allow(clippy::module_inception)]
use super::*;

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
    pub(super) const fn tag(self) -> u8 {
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
    pub(super) const fn tag(self) -> u8 {
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

    pub(super) const fn tag(self) -> u8 {
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

    pub(super) const fn tag(self) -> u8 {
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

    pub(super) fn verify_integrity(&self) -> Result<(), GateError> {
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

pub(super) const ROM_HEADER_BYTES: u64 = 0x40;
pub(super) const MAGIC_Z64: u32 = 0x8037_1240;
pub(super) const MAGIC_N64: u32 = 0x4012_3780;
pub(super) const MAGIC_V64: u32 = 0x3780_4012;

pub(super) fn normalize_rom_bytes(input: &[u8]) -> Result<(ReleaseRomByteOrder, Vec<u8>), GateError> {
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

pub(super) fn has_recognized_rom_magic(input: &[u8]) -> bool {
    input.get(..4).is_some_and(|bytes| {
        matches!(
            u32::from_be_bytes(bytes.try_into().expect("four-byte ROM magic")),
            MAGIC_Z64 | MAGIC_N64 | MAGIC_V64
        )
    })
}

pub(super) fn validate_installed_rom_identity(
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

pub(super) fn decode_rom_tv_region(destination_code: u8) -> Result<ReleaseTvRegion, GateError> {
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

pub(super) fn validate_rom_environment(
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

pub(super) fn validate_rom_input(
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


pub(super) fn environment_from_frozen(
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
        fn64_abi::GraphicsTaskExecutionPolicy::HleOptimized
        | fn64_abi::GraphicsTaskExecutionPolicy::DiagnosticSkip => {
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
    let audio_task_execution = match host.audio_task_execution {
        fn64_abi::AudioTaskExecutionPolicy::Unconfigured => {
            ReleaseAudioTaskExecutionPolicy::Unconfigured
        }
        fn64_abi::AudioTaskExecutionPolicy::Translated { artifact_sha256 } => {
            ReleaseAudioTaskExecutionPolicy::Translated {
                artifact_sha256: hex(&artifact_sha256),
            }
        }
        fn64_abi::AudioTaskExecutionPolicy::LleAccuracy => {
            ReleaseAudioTaskExecutionPolicy::LleAccuracy
        }
        fn64_abi::AudioTaskExecutionPolicy::DiagnosticSkip => {
            ReleaseAudioTaskExecutionPolicy::DiagnosticSkip
        }
    };
    Ok(ReleaseEnvironmentEvidence {
        platform,
        windows_version,
        controller_ports,
        cartridge_save,
        audio_task_execution,
        renderer,
    })
}

pub(super) fn validate_environment_observation(
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

pub(super) fn validate_environment_evidence(
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
    match &environment.audio_task_execution {
        ReleaseAudioTaskExecutionPolicy::LleAccuracy => {}
        ReleaseAudioTaskExecutionPolicy::Translated { artifact_sha256 } => {
            decode_sha256(artifact_sha256).ok_or(GateError::InvalidReportSha256(
                "environment.audio_task_execution.artifact_sha256",
            ))?;
            return Err(GateError::NonAccuracyAudioTaskPolicy);
        }
        ReleaseAudioTaskExecutionPolicy::Unconfigured
        | ReleaseAudioTaskExecutionPolicy::DiagnosticSkip => {
            return Err(GateError::NonAccuracyAudioTaskPolicy);
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
